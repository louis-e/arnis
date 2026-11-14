//! `--river-bed v1`: a U-shaped bed for rivers, canals and streams, deeper where they are wider.
//! Lakes, the sea and unmapped water keep their bed. Everything is a function of element
//! geometry, block coordinates and scale, so pieces and tiles agree.

use crate::block_definitions::{
    Block, ANDESITE, CLAY, COARSE_DIRT, DIRT, GRAVEL, MOSSY_COBBLESTONE, MUD, SAND, STONE,
};
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::element_processing::water_areas::PolygonEdges;
use crate::element_processing::waterways::{
    capped_waterway_width, is_channel_waterway, is_underground_waterway,
};
use crate::ground::Ground;
use crate::ground_generation::ground_noise;
use crate::land_cover::{coord_hash, LC_WATER};
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedWay};
use crate::water_depth::{chamfer_3_4_dt, BigWaterField, MAX_WATER_DEPTH};
use rayon::prelude::*;
use std::collections::HashMap;

/// `waterway=*` values that get a river profile. Ditches and drains are too narrow to matter.
const RIVER_WATERWAYS: &[&str] = &["river", "stream", "canal", "brook", "fairway", "flowline"];

/// `water=*` values that make a polygon a river. An oxbow is a lake.
const RIVER_WATER_VALUES: &[&str] = &["river", "canal", "stream"];

/// Confluence blend band, in blocks: `2 * half-width` clamped to this range.
const BAND_MIN: f64 = 8.0;
const BAND_MAX: f64 = 32.0;

/// Slack over the tagged half-width before a centreline counts as inside a wider water body.
const EMBED_MARGIN: f64 = 4.0;

/// u8 chamfer distances saturate here (85 blocks); the depth cap saturates at hw 30.
const DT_MAX: u8 = u8::MAX;

/// Chamfer units per block (the 3-4 chamfer's straight step).
const DT_UNITS_PER_BLOCK: f64 = 3.0;

/// The field is built and kept in tiles this wide, and only where a river runs.
const TILE_SHIFT: u32 = 9;
const TILE: usize = 1 << TILE_SHIFT;

/// A tile cell the river mask does not cover.
const NO_RIVER: u8 = u8::MAX;

/// `--river-bed` value.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RiverBed {
    /// The bed every water body gets
    #[default]
    Off,
    /// U-shaped river beds whose depth follows the river's width
    V1,
}

/// Per-block river bed depth, one byte a cell in the tiles a river touches.
#[derive(Default)]
pub struct RiverBedField {
    tx0: i32,
    tz0: i32,
    tw: usize,
    th: usize,
    tiles: Vec<Option<Box<[u8]>>>,
}

impl RiverBedField {
    pub fn is_empty(&self) -> bool {
        self.tiles.iter().all(Option::is_none)
    }

    /// River bed depth at this column, or `None` where the legacy bed owns it.
    #[inline]
    pub fn depth_override(&self, x: i32, z: i32) -> Option<i32> {
        let tx = (x >> TILE_SHIFT) - self.tx0;
        let tz = (z >> TILE_SHIFT) - self.tz0;
        if tx < 0 || tz < 0 || tx as usize >= self.tw || tz as usize >= self.th {
            return None;
        }
        let tile = self.tiles[tz as usize * self.tw + tx as usize].as_deref()?;
        let m = TILE as i32 - 1;
        let d = tile[(((z & m) as usize) << TILE_SHIFT) | (x & m) as usize];
        (d != NO_RIVER).then_some(i32::from(d))
    }

    /// A river of one depth over a rectangle, for tests of the carve.
    #[cfg(test)]
    pub(crate) fn uniform(min_x: i32, min_z: i32, w: usize, h: usize, depth: u8) -> Self {
        let (max_x, max_z) = (min_x + w as i32 - 1, min_z + h as i32 - 1);
        let (tx0, tz0) = (min_x >> TILE_SHIFT, min_z >> TILE_SHIFT);
        let tw = ((max_x >> TILE_SHIFT) - tx0 + 1) as usize;
        let th = ((max_z >> TILE_SHIFT) - tz0 + 1) as usize;
        let mut tiles: Vec<Option<Box<[u8]>>> = vec![None; tw * th];
        let m = TILE as i32 - 1;
        for z in min_z..=max_z {
            for x in min_x..=max_x {
                let t =
                    ((z >> TILE_SHIFT) - tz0) as usize * tw + ((x >> TILE_SHIFT) - tx0) as usize;
                let tile = tiles[t].get_or_insert_with(|| vec![NO_RIVER; TILE * TILE].into());
                tile[(((z & m) as usize) << TILE_SHIFT) | (x & m) as usize] = depth;
            }
        }
        Self {
            tx0,
            tz0,
            tw,
            th,
            tiles,
        }
    }

    #[cfg(test)]
    fn overrides(&self) -> impl Iterator<Item = u8> + '_ {
        self.tiles
            .iter()
            .flatten()
            .flat_map(|t| t.iter().copied())
            .filter(|&d| d != NO_RIVER)
    }

    #[cfg(test)]
    fn override_count(&self) -> usize {
        self.overrides().count()
    }

    #[cfg(test)]
    fn max_override_depth(&self) -> i32 {
        self.overrides().map(i32::from).max().unwrap_or(0)
    }
}

/// Build the field once per run. Element geometry is complete inside `clip_bbox`.
#[allow(clippy::too_many_arguments)]
pub fn compute_river_bed_field(
    mode: RiverBed,
    elements: &[ProcessedElement],
    ground: &Ground,
    bwf: &BigWaterField,
    xzbbox: &XZBBox,
    clip_bbox: &XZBBox,
    scale: f64,
    channel_width_cap: Option<i32>,
) -> RiverBedField {
    if mode == RiverBed::Off {
        return RiverBedField::default();
    }
    let (off_x, off_z) = (xzbbox.min_x(), xzbbox.min_z());
    // Reads past the area repeat its border row.
    let is_lc_water =
        |x: i32, z: i32| ground.cover_class(XZPoint::new(x - off_x, z - off_z)) == LC_WATER;
    let legacy_depth = |x: i32, z: i32| bwf.legacy_depth_at(x, z);
    build_field(
        elements,
        &FieldInputs {
            is_lc_water: &is_lc_water,
            legacy_depth: &legacy_depth,
            bb: (
                clip_bbox.min_x(),
                clip_bbox.max_x(),
                clip_bbox.min_z(),
                clip_bbox.max_z(),
            ),
            scale,
            channel_width_cap,
        },
    )
}

/// Everything the build reads besides geometry, behind closures so tests need no `Ground`.
struct FieldInputs<'a> {
    is_lc_water: &'a (dyn Fn(i32, i32) -> bool + Sync),
    legacy_depth: &'a (dyn Fn(i32, i32) -> i32 + Sync),
    /// (min_x, max_x, min_z, max_z) of the region whose geometry is complete.
    bb: (i32, i32, i32, i32),
    scale: f64,
    /// Widest line waterway the draw paints (`--water-detail scaled` at small scale).
    channel_width_cap: Option<i32>,
}

struct PolyRings {
    edges: PolygonEdges,
    /// (min_x, max_x, min_z, max_z) of the outer rings.
    bounds: (i32, i32, i32, i32),
    /// Filled x-spans of each row from `z0` on, set by `scan`.
    z0: i32,
    rows: Vec<Vec<(i32, i32)>>,
}

impl PolyRings {
    fn new(outers: Vec<Vec<XZPoint>>, inners: Vec<Vec<XZPoint>>) -> Self {
        let mut b = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
        for p in outers.iter().flatten() {
            b = (b.0.min(p.x), b.1.max(p.x), b.2.min(p.z), b.3.max(p.z));
        }
        Self {
            edges: PolygonEdges::new(&outers, &inners),
            bounds: b,
            z0: 0,
            rows: Vec::new(),
        }
    }

    /// Scan the polygon once inside `bb`; every tile then reads its rows from the spans.
    fn scan(&mut self, bb: (i32, i32, i32, i32)) {
        self.z0 = self.bounds.2.max(bb.2);
        let z1 = self.bounds.3.min(bb.3);
        let edges = &self.edges;
        self.rows = (self.z0..=z1)
            .into_par_iter()
            .map(|z| edges.row_spans(z, bb.0, bb.1))
            .collect();
    }
}

struct LineRiver<'a> {
    way: &'a ProcessedWay,
    /// Half-width of the drawn ribbon (`create_water_channel` reaches `hw + 1`).
    hw_stamp: i32,
}

/// What one tile has to read: indices into the classified geometry.
#[derive(Default, Clone)]
struct TileWork {
    has_river: bool,
    lines: Vec<u32>,
    rivers: Vec<u32>,
    lakes: Vec<u32>,
}

/// Window of the polygon half-width estimate, in blocks.
fn hw_window(scale: f64) -> usize {
    ((16.0 * scale).round() as usize).clamp(1, 48)
}

/// How far a cell's depth reads: the saturated bank distance, the window and the tent.
fn reach(scale: f64) -> i32 {
    (f64::from(DT_MAX) / DT_UNITS_PER_BLOCK).ceil() as i32 + hw_window(scale) as i32 + 3
}

fn build_field(elements: &[ProcessedElement], inp: &FieldInputs) -> RiverBedField {
    build_field_tiled(elements, inp, reach(inp.scale))
}

fn build_field_tiled(elements: &[ProcessedElement], inp: &FieldInputs, halo: i32) -> RiverBedField {
    let mut lines = Vec::new();
    let mut river_polys = Vec::new();
    let mut nonriver_polys = Vec::new();
    classify(
        elements,
        inp.channel_width_cap,
        &mut lines,
        &mut river_polys,
        &mut nonriver_polys,
    );
    let bb = inp.bb;
    if (lines.is_empty() && river_polys.is_empty()) || bb.0 > bb.1 || bb.2 > bb.3 {
        return RiverBedField::default();
    }
    let (tx0, tz0) = (bb.0 >> TILE_SHIFT, bb.2 >> TILE_SHIFT);
    let tw = ((bb.1 >> TILE_SHIFT) - tx0 + 1) as usize;
    let th = ((bb.3 >> TILE_SHIFT) - tz0 + 1) as usize;

    // Tiles a (min_x, max_x, min_z, max_z) box grown by `pad` overlaps.
    let tiles_of = |b: (i32, i32, i32, i32), pad: i32| {
        let x0 = (b.0.saturating_sub(pad).max(bb.0) >> TILE_SHIFT) - tx0;
        let x1 = (b.1.saturating_add(pad).min(bb.1) >> TILE_SHIFT) - tx0;
        let z0 = (b.2.saturating_sub(pad).max(bb.2) >> TILE_SHIFT) - tz0;
        let z1 = (b.3.saturating_add(pad).min(bb.3) >> TILE_SHIFT) - tz0;
        (z0..=z1).flat_map(move |tz| (x0..=x1).map(move |tx| tz as usize * tw + tx as usize))
    };
    let mut work = vec![TileWork::default(); tw * th];
    for (i, lr) in lines.iter().enumerate() {
        let r = lr.hw_stamp + 1;
        for pair in lr.way.nodes.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            let seg = (a.x.min(b.x), a.x.max(b.x), a.z.min(b.z), a.z.max(b.z));
            for t in tiles_of(seg, r) {
                work[t].has_river = true;
            }
            for t in tiles_of(seg, r + halo) {
                if work[t].lines.last() != Some(&(i as u32)) {
                    work[t].lines.push(i as u32);
                }
            }
        }
    }
    river_polys.iter_mut().for_each(|p| p.scan(bb));
    for (i, p) in river_polys.iter().enumerate() {
        for (z, row) in (p.z0..).zip(&p.rows) {
            for &(a, b) in row {
                for t in tiles_of((a, b, z, z), 0) {
                    work[t].has_river = true;
                }
            }
        }
        for t in tiles_of(p.bounds, halo) {
            work[t].rivers.push(i as u32);
        }
    }
    // Only a lake near a river is read.
    for (i, p) in nonriver_polys.iter_mut().enumerate() {
        if tiles_of(p.bounds, halo).any(|t| work[t].has_river) {
            p.scan(bb);
            for t in tiles_of(p.bounds, halo) {
                work[t].lakes.push(i as u32);
            }
        }
    }

    let geometry = Geometry {
        lines: &lines,
        river_polys: &river_polys,
        nonriver_polys: &nonriver_polys,
    };
    let built: Vec<(usize, Box<[u8]>)> = work
        .par_iter()
        .enumerate()
        .filter(|(_, w)| w.has_river)
        .filter_map(|(t, w)| {
            let core_x = (tx0 + (t % tw) as i32) << TILE_SHIFT;
            let core_z = (tz0 + (t / tw) as i32) << TILE_SHIFT;
            build_tile(&geometry, w, inp, core_x, core_z, halo).map(|d| (t, d))
        })
        .collect();
    if built.is_empty() {
        return RiverBedField::default();
    }
    let mut tiles: Vec<Option<Box<[u8]>>> = vec![None; tw * th];
    for (t, d) in built {
        tiles[t] = Some(d);
    }
    RiverBedField {
        tx0,
        tz0,
        tw,
        th,
        tiles,
    }
}

struct Geometry<'a> {
    lines: &'a [LineRiver<'a>],
    river_polys: &'a [PolyRings],
    nonriver_polys: &'a [PolyRings],
}

/// Depths of one tile, or `None` without a river. The tile grown by `halo` holds everything
/// its cells read, so the result does not depend on the tiling.
fn build_tile(
    geo: &Geometry,
    work: &TileWork,
    inp: &FieldInputs,
    core_x: i32,
    core_z: i32,
    halo: i32,
) -> Option<Box<[u8]>> {
    let scale = inp.scale;
    let bb = inp.bb;
    let last = TILE as i32 - 1;
    // Clipped where geometry ends, so a truncated river is never a seeded bank.
    let min_x = (core_x - halo).max(bb.0);
    let max_x = (core_x + last + halo).min(bb.1);
    let min_z = (core_z - halo).max(bb.2);
    let max_z = (core_z + last + halo).min(bb.3);
    let lat = Lat {
        min_x,
        min_z,
        w: (max_x - min_x + 1) as usize,
        h: (max_z - min_z + 1) as usize,
    };
    let n = lat.w * lat.h;
    // The tile's own cells, in lattice coordinates.
    let ci = (core_x.max(bb.0) - min_x) as usize..=((core_x + last).min(bb.1) - min_x) as usize;
    let cj = (core_z.max(bb.2) - min_z) as usize..=((core_z + last).min(bb.3) - min_z) as usize;
    let any_in_core = |m: &[bool]| {
        cj.clone()
            .any(|j| m[j * lat.w + ci.start()..=j * lat.w + ci.end()].contains(&true))
    };

    let mut line_mask = vec![false; n];
    let mut hw_line = vec![0u8; n];
    for &i in &work.lines {
        stamp_line(&geo.lines[i as usize], &lat, &mut line_mask, &mut hw_line);
    }
    let mut poly_mask = vec![false; n];
    for &i in &work.rivers {
        rasterize_poly(&geo.river_polys[i as usize], &lat, &mut poly_mask);
    }
    if !any_in_core(&line_mask) && !any_in_core(&poly_mask) {
        return None;
    }
    let mut nonriver_mask = vec![false; n];
    for &i in &work.lakes {
        rasterize_poly(&geo.nonriver_polys[i as usize], &lat, &mut nonriver_mask);
    }
    let mut lc_water = vec![false; n];
    for j in 0..lat.h {
        for i in 0..lat.w {
            if (inp.is_lc_water)(lat.min_x + i as i32, lat.min_z + j as i32) {
                lc_water[j * lat.w + i] = true;
            }
        }
    }

    // A centreline drawn through a mapped lake must not override the lake bed.
    let mut removed_stamp = vec![false; n];
    for i in 0..n {
        if line_mask[i] && nonriver_mask[i] {
            line_mask[i] = false;
            removed_stamp[i] = true;
        }
    }

    // A centreline deep inside wider land-cover water would carve a stripe through it.
    if line_mask.contains(&true) {
        let mut land_dt: Vec<u8> = (0..n)
            .map(|i| if lc_water[i] { DT_MAX } else { 0 })
            .collect();
        chamfer_3_4_dt(&mut land_dt, lat.w, lat.h);
        for i in 0..n {
            if !line_mask[i] || poly_mask[i] {
                continue;
            }
            let land_d = f64::from(land_dt[i]) / DT_UNITS_PER_BLOCK;
            if land_d > f64::from(hw_line[i]) + EMBED_MARGIN {
                line_mask[i] = false;
                removed_stamp[i] = true;
            }
        }
    }

    let mask: Vec<bool> = (0..n).map(|i| line_mask[i] || poly_mask[i]).collect();
    drop(line_mask);
    if !any_in_core(&mask) {
        return None;
    }

    // Distance from the bank.
    let mut dt: Vec<u8> = (0..n).map(|i| if mask[i] { DT_MAX } else { 0 }).collect();
    chamfer_3_4_dt(&mut dt, lat.w, lat.h);

    // Local half-width: tagged for lines, a windowed max of the bank distance for polygons.
    let d_in_mask: Vec<u8> = (0..n)
        .map(|i| {
            if mask[i] {
                (f64::from(dt[i]) / DT_UNITS_PER_BLOCK).round() as u8
            } else {
                0
            }
        })
        .collect();
    let poly_hw = if poly_mask.contains(&true) {
        windowed_max_2d(&d_in_mask, lat.w, lat.h, hw_window(scale))
    } else {
        Vec::new()
    };

    let mut hw_field = vec![0f32; n];
    let mut depth_f = vec![0f32; n];
    for i in 0..n {
        if !mask[i] {
            continue;
        }
        let mut hw = f64::from(hw_line[i]);
        if poly_mask[i] {
            hw = hw.max(f64::from(poly_hw[i].max(d_in_mask[i])));
        }
        let hw = hw.max(1.0);
        hw_field[i] = hw as f32;
        let d_blocks = f64::from(dt[i]) / DT_UNITS_PER_BLOCK;
        depth_f[i] = river_profile_depth(d_blocks, hw) as f32;
    }
    drop((dt, d_in_mask, poly_hw, poly_mask, hw_line));
    // Rounds the chamfer's octagonal facets off the depth contours.
    let depth_f = tent3(&depth_f, lat.w, lat.h);

    // Distance to foreign water, so the river arrives at a lake's bed instead of a step.
    let mut m_dt = vec![DT_MAX; n];
    for j in 0..lat.h {
        for i in 0..lat.w {
            let idx = j * lat.w + i;
            if mask[idx] {
                continue;
            }
            let foreign = nonriver_mask[idx] || removed_stamp[idx] || lc_water[idx];
            if foreign && neighbours_mask(&mask, &lat, i, j) {
                m_dt[idx] = 0;
            }
        }
    }
    drop((nonriver_mask, removed_stamp, lc_water));
    let has_seed = m_dt.contains(&0);
    if has_seed {
        chamfer_3_4_dt(&mut m_dt, lat.w, lat.h);
    }

    let mut out: Box<[u8]> = vec![NO_RIVER; TILE * TILE].into();
    for j in cj {
        for i in ci.clone() {
            let idx = j * lat.w + i;
            if !mask[idx] {
                continue;
            }
            let (x, z) = (lat.min_x + i as i32, lat.min_z + j as i32);
            let river = natural_depth(f64::from(depth_f[idx]), x, z);
            let blended = if has_seed {
                let band = (2.0 * f64::from(hw_field[idx])).clamp(BAND_MIN, BAND_MAX);
                let m = f64::from(m_dt[idx]) / DT_UNITS_PER_BLOCK;
                let wgt = smoothstep((m / band).clamp(0.0, 1.0));
                let legacy = f64::from((inp.legacy_depth)(x, z));
                legacy + (river - legacy) * wgt
            } else {
                river
            };
            let depth = blended.round().clamp(0.0, f64::from(MAX_WATER_DEPTH)) as u8;
            out[(((z - core_z) as usize) << TILE_SHIFT) | (x - core_x) as usize] = depth;
        }
    }
    Some(out)
}

const SALT_POOL: u32 = 0x5256_0001;
const SALT_RIPPLE: u32 = 0x5256_0002;
const SALT_ZONE: u32 = 0x5256_0003;
const SALT_EARTH: u32 = 0x5256_0004;
const SALT_SAND: u32 = 0x5256_0005;
const SALT_ROCK: u32 = 0x5256_0006;
const SALT_CLAY: u32 = 0x5256_0007;
const SALT_SILT: u32 = 0x5256_0008;

/// Pools along the river and a ripple that bends the depth contours off the bank lines.
fn natural_depth(d: f64, x: i32, z: i32) -> f64 {
    let pool = 0.88 + 0.24 * ground_noise(x, z, 36, SALT_POOL);
    let ripple = (ground_noise(x, z, 5, SALT_RIPPLE) - 0.5) * 0.5;
    d * pool + ripple * d.min(1.0)
}

/// Bed and the block under it for a river column; `sandy` beside a beach. Earth, sand, gravel
/// and rock each have a noise field over a noisy depth: ragged patches, no bands along the banks.
pub fn bed_blocks(x: i32, z: i32, depth: i32, sandy: bool) -> (Block, Block) {
    let dither = (coord_hash(x ^ 0x52B3, z ^ 0x1F7D) % 1000) as f64 / 1000.0 - 0.5;
    let zone = f64::from(depth) + (ground_noise(x, z, 9, SALT_ZONE) - 0.5) * 2.4 + dither * 0.8;
    // 0 at the bank, 1 in the deep channel.
    let s = ((zone - 0.5) / 4.0).clamp(0.0, 1.0);
    let field = |scale: i32, salt: u32| ground_noise(x, z, scale, salt) + dither * 0.14;
    if zone > 0.8 && field(10, SALT_CLAY) > 0.94 {
        return (CLAY, STONE);
    }
    let earth = 0.56 - 0.52 * s.powf(0.7);
    if field(9, SALT_EARTH) < if sandy { earth * 0.3 } else { earth } {
        let silt = ground_noise(x, z, 3, SALT_SILT);
        let top = if silt < 0.55 {
            DIRT
        } else if silt < 0.88 {
            COARSE_DIRT
        } else {
            MUD
        };
        return (top, DIRT);
    }
    let sand = 0.25 + 0.6 * s * (1.0 - s) - 0.13 * s;
    if field(10, SALT_SAND) < if sandy { 0.85 } else { sand } {
        return (SAND, STONE);
    }
    if field(6, SALT_ROCK) > 1.0 - 0.12 * s {
        let top = match coord_hash(x ^ 0x0C0B, z ^ 0x7E55) % 20 {
            0..=10 => STONE,
            11..=15 => ANDESITE,
            _ => MOSSY_COBBLESTONE,
        };
        return (top, STONE);
    }
    (GRAVEL, STONE)
}

#[inline]
fn smoothstep(u: f64) -> f64 {
    3.0 * u * u - 2.0 * u * u * u
}

/// Centre depth by half-width: hw 3 -> ~1.2, 10 -> ~2.8, 20 -> ~4.5, >= 30 -> 6.
fn river_depth_cap_for_hw(hw: f64) -> f64 {
    (6.0 * (hw / 30.0).powf(0.7)).clamp(1.0, f64::from(MAX_WATER_DEPTH))
}

/// Bed depth `d_blocks` from the bank: an ease-out, so the water is one block deep at the bank,
/// goes down from the next column on and flattens into a broad bottom. A stream keeps a parabola,
/// a river gets a cubic; steepest at the bank, at most `3 * D / hw`, under a block a column.
fn river_profile_depth(d_blocks: f64, hw: f64) -> f64 {
    // The bank column itself stays one deep; a narrow stream has less room for that.
    let t = ((d_blocks - (hw / 4.0).min(1.0)) / hw.max(1.0)).clamp(0.0, 1.0);
    let power = 2.0 + ((hw - 3.0) / 7.0).clamp(0.0, 1.0);
    river_depth_cap_for_hw(hw) * (1.0 - (1.0 - t).powf(power))
}

fn tag<'a>(tags: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    tags.get(k).map(String::as_str)
}

fn is_river_polygon_tags(tags: &HashMap<String, String>) -> bool {
    tag(tags, "water").is_some_and(|v| RIVER_WATER_VALUES.contains(&v))
        || tag(tags, "waterway") == Some("riverbank")
}

/// Any other water polygon: bare `natural=water`, `natural=bay`, `water=lake`, oxbows.
fn is_nonriver_water_tags(tags: &HashMap<String, String>) -> bool {
    !is_river_polygon_tags(tags)
        && (tags.contains_key("water") || matches!(tag(tags, "natural"), Some("water" | "bay")))
}

/// Closed by node id, or by endpoints within a block (the `water_areas` tolerance).
fn to_ring(nodes: &[ProcessedNode]) -> Option<Vec<XZPoint>> {
    let (first, last) = (nodes.first()?, nodes.last()?);
    let closed = nodes.len() >= 4
        && (first.id == last.id
            || ((first.x - last.x).abs() <= 1 && (first.z - last.z).abs() <= 1));
    closed.then(|| nodes.iter().map(ProcessedNode::xz).collect())
}

fn relation_rings(rel: &crate::osm_parser::ProcessedRelation) -> Option<PolyRings> {
    let mut outers: Vec<Vec<ProcessedNode>> = Vec::new();
    let mut inners: Vec<Vec<ProcessedNode>> = Vec::new();
    for mem in &rel.members {
        match mem.role {
            ProcessedMemberRole::Outer => outers.push(mem.way.nodes.clone()),
            ProcessedMemberRole::Inner => inners.push(mem.way.nodes.clone()),
            ProcessedMemberRole::Part => {}
        }
    }
    crate::element_processing::merge_way_segments(&mut outers);
    crate::element_processing::merge_way_segments(&mut inners);
    let outers: Vec<_> = outers.iter().filter_map(|r| to_ring(r)).collect();
    let inners = inners.iter().filter_map(|r| to_ring(r)).collect();
    (!outers.is_empty()).then(|| PolyRings::new(outers, inners))
}

/// Tag scan of every element; the dispatch sends `natural=water` + `water=river` elsewhere.
fn classify<'a>(
    elements: &'a [ProcessedElement],
    channel_width_cap: Option<i32>,
    lines: &mut Vec<LineRiver<'a>>,
    river_polys: &mut Vec<PolyRings>,
    nonriver_polys: &mut Vec<PolyRings>,
) {
    for el in elements {
        match el {
            ProcessedElement::Way(way) => {
                let polys = if is_river_polygon_tags(&way.tags) {
                    Some(&mut *river_polys)
                } else if is_nonriver_water_tags(&way.tags) {
                    Some(&mut *nonriver_polys)
                } else {
                    None
                };
                if let Some(polys) = polys {
                    if let Some(ring) = to_ring(&way.nodes) {
                        polys.push(PolyRings::new(vec![ring], Vec::new()));
                    }
                    continue;
                }
                let Some(wt) = tag(&way.tags, "waterway") else {
                    continue;
                };
                // The draw's own gates, so the mask never claims a culvert or a weir.
                if !RIVER_WATERWAYS.contains(&wt)
                    || !is_channel_waterway(wt)
                    || is_underground_waterway(&way.tags)
                    || way.nodes.len() < 2
                {
                    continue;
                }
                let width = capped_waterway_width(wt, &way.tags, channel_width_cap);
                lines.push(LineRiver {
                    way,
                    hw_stamp: width / 2,
                });
            }
            ProcessedElement::Relation(rel) => {
                let polys = if is_river_polygon_tags(&rel.tags) {
                    &mut *river_polys
                } else if is_nonriver_water_tags(&rel.tags) {
                    &mut *nonriver_polys
                } else {
                    continue;
                };
                polys.extend(relation_rings(rel));
            }
            ProcessedElement::Node(_) => {}
        }
    }
}

struct Lat {
    min_x: i32,
    min_z: i32,
    w: usize,
    h: usize,
}

impl Lat {
    #[inline]
    fn idx(&self, x: i32, z: i32) -> Option<usize> {
        let lx = i64::from(x) - i64::from(self.min_x);
        let lz = i64::from(z) - i64::from(self.min_z);
        if lx < 0 || lz < 0 || lx as usize >= self.w || lz as usize >= self.h {
            return None;
        }
        Some(lz as usize * self.w + lx as usize)
    }
}

/// The ribbon `create_water_channel` draws, geometry only.
fn stamp_line(lr: &LineRiver, lat: &Lat, mask: &mut [bool], hw_line: &mut [u8]) {
    let r = lr.hw_stamp + 1;
    // The ribbon's centre is this far from its bank.
    let hw = (r + 1).clamp(1, 255) as u8;
    for pair in lr.way.nodes.windows(2) {
        let (a, b) = (pair[0].xz(), pair[1].xz());
        for (bx, _, bz) in bresenham_line(a.x, 0, a.z, b.x, 0, b.z) {
            for x in (bx - r)..=(bx + r) {
                for z in (bz - r)..=(bz + r) {
                    if let Some(i) = lat.idx(x, z) {
                        mask[i] = true;
                        // Max, so overlapping ways give the same field in any order.
                        hw_line[i] = hw_line[i].max(hw);
                    }
                }
            }
        }
    }
}

fn rasterize_poly(p: &PolyRings, lat: &Lat, out: &mut [bool]) {
    let (lo_x, hi_x) = (lat.min_x, lat.min_x + lat.w as i32 - 1);
    let first = lat.min_z.max(p.z0);
    let last = (lat.min_z + lat.h as i32 - 1).min(p.z0 + p.rows.len() as i32 - 1);
    for z in first..=last {
        let row = (z - lat.min_z) as usize * lat.w;
        for &(s, e) in &p.rows[(z - p.z0) as usize] {
            let (s, e) = (s.max(lo_x), e.min(hi_x));
            if s <= e {
                out[row + (s - lo_x) as usize..=row + (e - lo_x) as usize].fill(true);
            }
        }
    }
}

fn neighbours_mask(mask: &[bool], lat: &Lat, i: usize, j: usize) -> bool {
    let (i, j) = (i as i32, j as i32);
    (-1..=1).any(|dj| {
        (-1..=1).any(|di| {
            (di, dj) != (0, 0)
                && lat
                    .idx(lat.min_x + i + di, lat.min_z + j + dj)
                    .is_some_and(|n| mask[n])
        })
    })
}

/// van Herk / Gil-Werman sliding maximum of radius `r`, O(n) in the window size.
fn sliding_max_1d(src: &[u8], r: usize) -> Vec<u8> {
    let n = src.len();
    if r == 0 || n == 0 {
        return src.to_vec();
    }
    let k = 2 * r + 1;
    let ext_len = (n + 2 * r).div_ceil(k) * k;
    let mut ext = vec![0u8; ext_len];
    ext[r..r + n].copy_from_slice(src);
    let mut pre = vec![0u8; ext_len];
    let mut suf = vec![0u8; ext_len];
    for b in (0..ext_len).step_by(k) {
        let mut m = 0u8;
        for i in b..b + k {
            m = m.max(ext[i]);
            pre[i] = m;
        }
        let mut m = 0u8;
        for i in (b..b + k).rev() {
            m = m.max(ext[i]);
            suf[i] = m;
        }
    }
    (0..n).map(|i| suf[i].max(pre[i + 2 * r])).collect()
}

/// Separable square-window maximum of radius `r`.
fn windowed_max_2d(src: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let mut tmp = Vec::with_capacity(src.len());
    for row in src.chunks(w) {
        tmp.extend(sliding_max_1d(row, r));
    }
    let mut out = vec![0u8; src.len()];
    let mut col = vec![0u8; h];
    for i in 0..w {
        for (j, slot) in col.iter_mut().enumerate() {
            *slot = tmp[j * w + i];
        }
        for (j, v) in sliding_max_1d(&col, r).into_iter().enumerate() {
            out[j * w + i] = v;
        }
    }
    out
}

/// One separable 3x3 tent (`[1,2,1]/4` twice), edges replicated.
fn tent3(src: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut tmp = vec![0f32; src.len()];
    for j in 0..h {
        let row = j * w;
        for i in 0..w {
            let l = src[row + i.saturating_sub(1)];
            let r = src[row + (i + 1).min(w - 1)];
            tmp[row + i] = 0.25 * l + 0.5 * src[row + i] + 0.25 * r;
        }
    }
    let mut out = vec![0f32; src.len()];
    for j in 0..h {
        let up = j.saturating_sub(1) * w;
        let dn = (j + 1).min(h - 1) * w;
        for i in 0..w {
            out[j * w + i] = 0.25 * tmp[up + i] + 0.5 * tmp[j * w + i] + 0.25 * tmp[dn + i];
        }
    }
    out
}

#[cfg(test)]
mod tests;
