//! River-only U-shaped bed (`--river-bed v1`).
//!
//! Overrides the carve depth of columns inside a river mask with a smoothstep profile scaled
//! by the river's width, and blends back to the legacy depth where the river meets water it
//! does not own (a lake, the sea, ESA-only water).
//!
//! - Off builds an empty field, so `BigWaterField::depth_at` is unchanged.
//! - Lakes, oceans and ESA-only water keep the legacy bed. A river centreline drawn through a
//!   mapped lake is clipped out of the mask, and one buried in wide land-cover water with no
//!   river polygon behind it is dropped.
//! - Depth only: the override is read by the existing carve paths, so it never carves a
//!   column that was not carved before.
//!
//! No RNG: the field is a pure function of element geometry, block coordinates and scale.

use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::element_processing::water_areas::PolygonEdges;
use crate::element_processing::waterways::{
    is_channel_waterway, is_underground_waterway, waterway_width,
};
use crate::ground::Ground;
use crate::land_cover::LC_WATER;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedWay};
use crate::water_depth::{chamfer_3_4_dt, BigWaterField, MAX_WATER_DEPTH};
use std::collections::HashMap;

/// `waterway=*` values that get a river profile. Ditches and drains are too narrow to matter.
const RIVER_WATERWAYS: &[&str] = &["river", "stream", "canal", "brook", "fairway", "flowline"];

/// `water=*` values that make a polygon a river. An oxbow is a lake.
const RIVER_WATER_VALUES: &[&str] = &["river", "canal", "stream"];

/// Confluence blend band, in blocks: `2 * half-width` clamped to this range.
const BAND_MIN: f64 = 8.0;
const BAND_MAX: f64 = 32.0;

/// Slack over the tagged half-width before a centreline-only column counts as embedded in a
/// wider water body.
const EMBED_MARGIN: f64 = 4.0;

/// u8 chamfer distances saturate here (85 blocks); the depth cap saturates at hw 30.
const DT_MAX: u8 = u8::MAX;

/// Chamfer units per block (the 3-4 chamfer's straight step).
const DT_UNITS_PER_BLOCK: f64 = 3.0;

/// Lattice cell cap. Past it the field is dropped and the legacy bed is used.
// ponytail: one lattice per run (~25 B/cell peak); per-tile lattices if single runs this big matter.
const MAX_LATTICE_CELLS: usize = 64_000_000;

/// `--river-bed` value. Versioned: a retune of the profile ships as a new value.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RiverBed {
    /// The bed every water body gets
    #[default]
    Off,
    /// U-shaped river beds whose depth follows the river's width
    V1,
}

/// Baked per-block river bed depth over a lattice around the river geometry.
#[derive(Default)]
pub struct RiverBedField {
    lat: Lat,
    depth: Vec<u8>,
    mask: Vec<bool>,
}

impl RiverBedField {
    /// River bed depth at this column, or `None` where the legacy bed owns it.
    #[inline]
    pub fn depth_override(&self, x: i32, z: i32) -> Option<i32> {
        let i = self.lat.idx(x, z)?;
        self.mask[i].then(|| i32::from(self.depth[i]))
    }

    /// A river of one depth over a rectangle, for tests of the carve.
    #[cfg(test)]
    pub(crate) fn uniform(min_x: i32, min_z: i32, w: usize, h: usize, depth: u8) -> Self {
        Self {
            lat: Lat { min_x, min_z, w, h },
            depth: vec![depth; w * h],
            mask: vec![true; w * h],
        }
    }

    #[cfg(test)]
    fn override_count(&self) -> usize {
        self.mask.iter().filter(|&&m| m).count()
    }

    #[cfg(test)]
    fn max_override_depth(&self) -> i32 {
        self.mask
            .iter()
            .zip(&self.depth)
            .filter(|(&m, _)| m)
            .map(|(_, &d)| i32::from(d))
            .max()
            .unwrap_or(0)
    }
}

/// Build the field once per run. `bwf` supplies the legacy depth the confluence band blends
/// to. `clip_bbox` is the area plus the One World clip pad: element geometry is complete
/// inside it, so the field reads the same banks on both sides of a seam.
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
    // Reads past the area repeat its border row; that only reaches the blend band and the
    // embedding test near a seam.
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
    is_lc_water: &'a dyn Fn(i32, i32) -> bool,
    legacy_depth: &'a dyn Fn(i32, i32) -> i32,
    /// (min_x, max_x, min_z, max_z) of the region whose geometry is complete.
    bb: (i32, i32, i32, i32),
    scale: f64,
    /// Widest line waterway the draw paints (`--water-detail scaled` at small scale).
    channel_width_cap: Option<i32>,
}

struct PolyRings {
    outers: Vec<Vec<XZPoint>>,
    inners: Vec<Vec<XZPoint>>,
}

struct LineRiver<'a> {
    way: &'a ProcessedWay,
    /// Half-width of the drawn ribbon (`create_water_channel` reaches `hw + 1`).
    hw_stamp: i32,
}

fn build_field(elements: &[ProcessedElement], inp: &FieldInputs) -> RiverBedField {
    let scale = inp.scale;
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
    if lines.is_empty() && river_polys.is_empty() {
        return RiverBedField::default();
    }
    // Room for the banks, the half-width window and the confluence band around the river.
    let halo = ((64.0 * scale).ceil() as i32 + 32).clamp(1, 96);
    let Some(lat) = lattice_for(&lines, &river_polys, halo, inp.bb) else {
        return RiverBedField::default();
    };
    let n = lat.w * lat.h;

    let mut line_mask = vec![false; n];
    let mut hw_line = vec![0u8; n];
    for lr in &lines {
        stamp_line(lr, &lat, &mut line_mask, &mut hw_line);
    }
    let mut poly_mask = vec![false; n];
    for p in &river_polys {
        rasterize_poly(p, &lat, &mut poly_mask);
    }
    let mut nonriver_mask = vec![false; n];
    for p in &nonriver_polys {
        rasterize_poly(p, &lat, &mut nonriver_mask);
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

    // A centreline deeper inside land-cover water than its tagged width explains is a wide
    // body the tags under-describe; a narrow ribbon there would leave a stripe.
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

    let mut mask = vec![false; n];
    for i in 0..n {
        if line_mask[i] || poly_mask[i] {
            mask[i] = true;
        }
    }
    if !mask.contains(&true) {
        return RiverBedField::default();
    }

    // Distance from the bank. The lattice ends where geometry is clipped, so a truncated
    // river end is the lattice edge, never a seeded bank that would pinch the bed shut.
    let mut dt: Vec<u8> = (0..n).map(|i| if mask[i] { DT_MAX } else { 0 }).collect();
    chamfer_3_4_dt(&mut dt, lat.w, lat.h);

    // Local half-width: line rivers carry a tagged one; polygons use a windowed max of the
    // bank distance.
    let d_in_mask: Vec<u8> = (0..n)
        .map(|i| {
            if mask[i] {
                (f64::from(dt[i]) / DT_UNITS_PER_BLOCK).round() as u8
            } else {
                0
            }
        })
        .collect();
    let win = ((16.0 * scale).round() as usize).max(1);
    let poly_hw = if poly_mask.contains(&true) {
        windowed_max_2d(&d_in_mask, lat.w, lat.h, win)
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
        depth_f[i] = river_profile_depth(d_blocks, hw, scale) as f32;
    }
    drop((dt, d_in_mask, poly_hw));
    // Rounds the chamfer's octagonal facets off the depth contours.
    let depth_f = tent3(&depth_f, lat.w, lat.h);

    // Distance to foreign water: at 0 the blend returns the legacy depth, so the river
    // arrives at the lake bed instead of stepping into it.
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
    let has_seed = m_dt.contains(&0);
    if has_seed {
        chamfer_3_4_dt(&mut m_dt, lat.w, lat.h);
    }

    let mut depth = vec![0u8; n];
    for j in 0..lat.h {
        for i in 0..lat.w {
            let idx = j * lat.w + i;
            if !mask[idx] {
                continue;
            }
            let river = f64::from(depth_f[idx]);
            let blended = if has_seed {
                let band = (2.0 * f64::from(hw_field[idx])).clamp(BAND_MIN, BAND_MAX);
                let m = f64::from(m_dt[idx]) / DT_UNITS_PER_BLOCK;
                let wgt = smoothstep((m / band).clamp(0.0, 1.0));
                let legacy = f64::from((inp.legacy_depth)(
                    lat.min_x + i as i32,
                    lat.min_z + j as i32,
                ));
                legacy + (river - legacy) * wgt
            } else {
                river
            };
            depth[idx] = blended.round().clamp(0.0, f64::from(MAX_WATER_DEPTH)) as u8;
        }
    }

    RiverBedField { lat, depth, mask }
}

#[inline]
fn smoothstep(u: f64) -> f64 {
    3.0 * u * u - 2.0 * u * u * u
}

/// Centre depth by half-width: hw 3 -> ~1.2, 10 -> ~2.8, 20 -> ~4.5, >= 30 -> 6.
fn river_depth_cap_for_hw(hw: f64) -> f64 {
    (6.0 * (hw / 30.0).powf(0.7)).clamp(1.0, f64::from(MAX_WATER_DEPTH))
}

/// Bed depth `d_blocks` from the bank of a channel of half-width `hw`. Smoothstep has zero
/// slope at both ends: a soft entry at the shore and a broad rounded bottom. Its peak slope
/// is `1.5 * D / hw` per block, so neighbouring columns never differ by more than a block.
fn river_profile_depth(d_blocks: f64, hw: f64, scale: f64) -> f64 {
    let hw = hw.max(1.0);
    let t = (d_blocks / hw).clamp(0.0, 1.0);
    // Wide rivers at full scale get longer, softer banks.
    let q = 1.0 + 0.5 * (hw / 30.0).min(1.0) * scale.min(1.0);
    river_depth_cap_for_hw(hw) * smoothstep(t.powf(q))
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
    (!outers.is_empty()).then_some(PolyRings { outers, inners })
}

/// Tag scan of every element. The dispatch can't be reused: it sends a way carrying any
/// `natural` key to `natural.rs`, including `natural=water` + `water=river`.
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
                        polys.push(PolyRings {
                            outers: vec![ring],
                            inners: Vec::new(),
                        });
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
                let mut width = waterway_width(wt, &way.tags);
                if let Some(cap) = channel_width_cap {
                    width = width.min(cap);
                }
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

#[derive(Default)]
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

/// The river geometry's bounds, padded by `halo` and intersected with `bb`.
fn lattice_for(
    lines: &[LineRiver],
    river_polys: &[PolyRings],
    halo: i32,
    bb: (i32, i32, i32, i32),
) -> Option<Lat> {
    let mut aabb = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    let mut grow = |x: i32, z: i32, pad: i32| {
        aabb.0 = aabb.0.min(x.saturating_sub(pad));
        aabb.1 = aabb.1.max(x.saturating_add(pad));
        aabb.2 = aabb.2.min(z.saturating_sub(pad));
        aabb.3 = aabb.3.max(z.saturating_add(pad));
    };
    for lr in lines {
        for n in &lr.way.nodes {
            grow(n.x, n.z, lr.hw_stamp + 1 + halo);
        }
    }
    for pt in river_polys.iter().flat_map(|p| p.outers.iter().flatten()) {
        grow(pt.x, pt.z, halo);
    }
    let min_x = aabb.0.max(bb.0);
    let max_x = aabb.1.min(bb.1);
    let min_z = aabb.2.max(bb.2);
    let max_z = aabb.3.min(bb.3);
    if min_x > max_x || min_z > max_z {
        return None;
    }
    let w = (i64::from(max_x) - i64::from(min_x) + 1) as usize;
    let h = (i64::from(max_z) - i64::from(min_z) + 1) as usize;
    if w.checked_mul(h).is_some_and(|t| t <= MAX_LATTICE_CELLS) {
        Some(Lat { min_x, min_z, w, h })
    } else {
        eprintln!("Warning: river area too large for --river-bed; using the default bed");
        None
    }
}

/// The ribbon `create_water_channel` draws, geometry only.
fn stamp_line(lr: &LineRiver, lat: &Lat, mask: &mut [bool], hw_line: &mut [u8]) {
    let r = lr.hw_stamp + 1;
    let hw = lr.hw_stamp.clamp(1, 255) as u8;
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
    let (z0, z1) = p
        .outers
        .iter()
        .flatten()
        .fold((i32::MAX, i32::MIN), |(lo, hi), pt| {
            (lo.min(pt.z), hi.max(pt.z))
        });
    let z0 = z0.max(lat.min_z);
    let z1 = z1.min(lat.min_z + lat.h as i32 - 1);
    let (lo_x, hi_x) = (lat.min_x, lat.min_x + lat.w as i32 - 1);
    let edges = PolygonEdges::new(&p.outers, &p.inners);
    for z in z0..=z1 {
        for (s, e) in edges.row_spans(z, lo_x, hi_x) {
            for x in s..=e {
                if let Some(i) = lat.idx(x, z) {
                    out[i] = true;
                }
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
