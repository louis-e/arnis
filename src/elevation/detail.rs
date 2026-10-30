//! Rock detail for steep ground that only a coarse source covers.
//!
//! Where a place has no LiDAR, Mapterhorn falls back to GLO-30, whose 30 m pixels resample
//! into even ramps that Minecraft draws as long, regular staircases. Measured on Swiss
//! LiDAR (limestone, granite and gneiss faces) cut down to 30 m, such a source misses about
//! 3-4 m RMS of relief 15-60 m across on slopes it shows at 45-55°, mostly as bands along
//! the contours: walls and ledges. On slopes it shows under about 35° it misses well under
//! a block, so that ground keeps its heights, as does all ground a fine source covers.
//!
//! The detail is 3D value noise over the place and its height, squashed in height so its
//! features run along the contours. It averages to nothing, so a mountain keeps its shape,
//! and it fades out towards fine sources, water, built-up land and the ways graded into
//! the ground.

use rayon::prelude::*;

use super::provider::SourcePixels;
use crate::building_facades::choose::hash64;
use crate::coordinate_system::{geographic::LLBBox, transformation::geo_distance};
use crate::land_cover::{LandCoverData, LC_BUILT_UP, LC_WATER};
use crate::projection::web_mercator::mercator_y_m;

/// Source pixels up to this many blocks across resample smoothly enough: a 10 m DEM stays
/// as it is at scale 1. The detail fades in from here to `RES_FULL_BLOCKS`.
const RES_FROM_BLOCKS: f64 = 10.0;
const RES_FULL_BLOCKS: f64 = 25.0;
/// Slope as the source shows it where the detail starts, and where it is full.
const SLOPE_FROM_DEG: f64 = 32.0;
const SLOPE_FULL_DEG: f64 = 48.0;
/// Scale of the summed octaves: about 3 m RMS on a full face, what a 30 m source misses there.
const AMPLITUDE_M: f64 = 9.0;
/// Feature size along the contour in metres, and weight.
const OCTAVES: [(f64, f64); 3] = [(40.0, 1.0), (16.0, 0.5), (6.0, 0.3)];
/// Features are this many times longer along the contour than up the slope, like rock bands.
const CONTOUR_STRETCH: f64 = 1.8;
/// Size of the patches the detail runs stronger or weaker in, so faces differ.
const PATCH_M: f64 = 250.0;
/// Ground size of the blocks the detail's strength is worked out on: fine enough to follow
/// roads and shores, coarse enough to cost nothing beside the height grid.
const BLOCK_M: f64 = 8.0;
/// Kept clear either side of a graded way's centreline: a motorway with its shoulders.
const GRADED_CLEAR_M: f64 = 12.0;
/// Blocks the detail fades in over away from ground a fine source covers. Wide, as
/// Mapterhorn's coverage changes along straight tile edges that would otherwise show.
const SOURCE_FADE_BLOCKS: usize = 4;
/// Blocks it fades in over away from water, built-up land and graded ways. Narrow, so a
/// face a road crosses keeps its detail.
const CLEAR_FADE_BLOCKS: usize = 1;
/// Rows each parallel band works through, holding its own copy of the rows either side.
const BAND_ROWS: usize = 256;
const EARTH_RADIUS_M: f64 = 6_371_000.0;
const SALT_PATCH: u64 = 0x5EED_0001;
const SALT_OCTAVE: u64 = 0x5EED_0100;

/// Adds rock detail to `heights` (metres, rows north to south over `bbox`) where `source`
/// had only coarse data and the ground is steep, and returns the cells it changed.
///
/// `one_world_origin` is the world block of cell (0, 0) in a One World: the detail's
/// blocks then sit on the world's block grid and the area's own height range is left
/// free, so neighbouring areas work out the same detail over their shared margin.
/// Otherwise the detail stays within the area's height range, which the fit maps the build
/// height by. Land cover of another size than the heights is not read.
pub(crate) fn add_steep_detail(
    heights: &mut [Vec<f64>],
    bbox: &LLBBox,
    source: &SourcePixels,
    scale: f64,
    one_world_origin: Option<(i64, i64)>,
    land_cover: Option<&LandCoverData>,
    graded: &[Vec<(f64, f64)>],
) -> usize {
    let rows = heights.len();
    let cols = heights.first().map_or(0, Vec::len);
    if rows < 3 || cols < 3 || source.height() != rows {
        return 0;
    }
    let (bbox_h_m, bbox_w_m) = geo_distance(bbox.min(), bbox.max());
    let cell_x = bbox_w_m / (cols - 1) as f64;
    let cell_z = bbox_h_m / (rows - 1) as f64;
    // Octaves the grid is too coarse to draw would only alias into cell-sized bumps.
    let octaves =
        OCTAVES.map(|(size, w)| (size, w * smoothstep(2.0, 4.0, size / cell_x.max(cell_z))));
    if octaves.iter().all(|&(_, w)| w <= 0.0) {
        return 0;
    }
    let placement = RowPlacement::new(bbox, rows, one_world_origin.is_some());
    let lattice = match one_world_origin {
        // A One World's cells are its blocks, so the step is the same in every area.
        Some(origin) => Lattice::new(origin, ((BLOCK_M * scale).round() as usize).max(1)),
        None => Lattice::new(
            (0, 0),
            ((BLOCK_M / cell_x.min(cell_z)).round() as usize).max(1),
        ),
    }
    .over(cols, rows);
    let row_taps: Vec<Taps> = (0..rows)
        .map(|gz| lattice.taps_z(placement.at_row(gz as f64)))
        .collect();
    let land_cover = land_cover.filter(|lc| lc.width == cols && lc.height == rows);
    let Some(gate) = Gate::new(
        source,
        lattice,
        &placement,
        (cols, rows),
        scale,
        bbox,
        cell_x.min(cell_z),
        land_cover,
        graded,
    ) else {
        return 0;
    };
    let range = if one_world_origin.is_some() {
        None
    } else {
        let (lo, hi) = heights
            .par_iter()
            .flatten()
            .filter(|h| h.is_finite())
            .fold(
                || (f64::INFINITY, f64::NEG_INFINITY),
                |(lo, hi), &h| (lo.min(h), hi.max(h)),
            )
            .reduce(
                || (f64::INFINITY, f64::NEG_INFINITY),
                |a, b| (a.0.min(b.0), a.1.max(b.1)),
            );
        if lo > hi {
            return 0;
        }
        Some((lo, hi))
    };

    let lats: Vec<(f64, f64)> = (0..rows)
        .map(|gz| {
            let lat = bbox.max().lat()
                - gz as f64 / (rows - 1) as f64 * (bbox.max().lat() - bbox.min().lat());
            lat.to_radians().sin_cos()
        })
        .collect();
    let lngs: Vec<(f64, f64)> = (0..cols)
        .map(|gx| {
            let lng = bbox.min().lng()
                + gx as f64 / (cols - 1) as f64 * (bbox.max().lng() - bbox.min().lng());
            lng.to_radians().sin_cos()
        })
        .collect();
    let col_taps: Vec<Taps> = (0..cols).map(|gx| gate.lattice.taps_x(gx)).collect();
    let field = Field {
        gate: &gate,
        col_taps: &col_taps,
        row_taps: &row_taps,
        lats: &lats,
        lngs: &lngs,
        octaves,
        cell_x,
        cell_z,
        range,
    };

    // A band reads its neighbours' unchanged edge rows, so these are copied first.
    let edges: Vec<EdgeRows> = (0..rows)
        .step_by(BAND_ROWS)
        .map(|start| {
            let end = (start + BAND_ROWS).min(rows);
            (
                start.checked_sub(1).map(|r| heights[r].clone()),
                heights.get(end).cloned(),
            )
        })
        .collect();
    heights
        .par_chunks_mut(BAND_ROWS)
        .zip(edges)
        .enumerate()
        .map(|(band, (rows_in_band, (above, below)))| {
            let mut changed = 0;
            let mut prev = above;
            let mut spare = Vec::with_capacity(cols);
            for i in 0..rows_in_band.len() {
                let gz = band * BAND_ROWS + i;
                let (done, rest) = rows_in_band.split_at_mut(i + 1);
                let row = &mut done[i];
                let next = rest.first().map(Vec::as_slice).or(below.as_deref());
                let mut orig = std::mem::take(&mut spare);
                orig.clear();
                orig.extend_from_slice(row);
                changed += field.row(gz, row, &orig, prev.as_deref(), next);
                spare = prev.replace(orig).unwrap_or_default();
            }
            changed
        })
        .sum()
}

/// A band's neighbouring rows above and below, as they were before any band changed.
type EdgeRows = (Option<Vec<f64>>, Option<Vec<f64>>);

/// What the per-cell pass reads.
struct Field<'a> {
    gate: &'a Gate,
    col_taps: &'a [Taps],
    row_taps: &'a [Taps],
    /// Sine and cosine of each row's latitude and each column's longitude.
    lats: &'a [(f64, f64)],
    lngs: &'a [(f64, f64)],
    octaves: [(f64, f64); 3],
    cell_x: f64,
    cell_z: f64,
    /// Lowest and highest height the detail stays within, if any.
    range: Option<(f64, f64)>,
}

impl Field<'_> {
    /// Adds the detail to grid row `gz` from its unchanged heights `orig` and those of the
    /// rows either side, where they exist. Returns the cells changed.
    fn row(
        &self,
        gz: usize,
        row: &mut [f64],
        orig: &[f64],
        above: Option<&[f64]>,
        below: Option<&[f64]>,
    ) -> usize {
        let taps_z = self.row_taps[gz];
        if !self.gate.any_in(taps_z) {
            return 0;
        }
        let cols = orig.len();
        let (above, rows_up) = above.map_or((orig, 0.0), |r| (r, 1.0));
        let (below, rows_down) = below.map_or((orig, 0.0), |r| (r, 1.0));
        let span_z = (rows_up + rows_down) * self.cell_z;
        let (sin_lat, cos_lat) = self.lats[gz];
        let mut changed = 0;
        for gx in 0..cols {
            let g = self.gate.at(self.col_taps[gx], taps_z);
            if g <= 0.0 {
                continue;
            }
            let (x0, x1) = (gx.saturating_sub(1), (gx + 1).min(cols - 1));
            let dx = (orig[x1] - orig[x0]) / ((x1 - x0) as f64 * self.cell_x);
            let dz = (below[gx] - above[gx]) / span_z;
            let slope = dx.hypot(dz).atan().to_degrees();
            let s = smoothstep(SLOPE_FROM_DEG, SLOPE_FULL_DEG, slope);
            if s.is_nan() || s <= 0.0 {
                continue;
            }
            let h = orig[gx];
            let (sin_lng, cos_lng) = self.lngs[gx];
            // A point on the globe, so the detail is the same whatever area it is part of,
            // pushed out by the height so the features follow the contours.
            let up = [cos_lat * cos_lng, cos_lat * sin_lng, sin_lat];
            let at = |r: f64| up.map(|u| u * r);
            let patch = 0.35
                + 0.65
                    * smoothstep(
                        0.2,
                        0.7,
                        0.5 + 0.5 * value_noise(at(EARTH_RADIUS_M / PATCH_M), SALT_PATCH),
                    );
            let r = EARTH_RADIUS_M + CONTOUR_STRETCH * h;
            let f: f64 = self
                .octaves
                .iter()
                .zip(0..)
                .filter(|((_, w), _)| *w > 0.0)
                .map(|(&(size, w), i)| w * value_noise(at(r / size), SALT_OCTAVE + i))
                .sum();
            let mut detailed = h + AMPLITUDE_M * g * s * patch * f;
            if let Some((lo, hi)) = self.range {
                detailed = detailed.clamp(lo, hi);
            }
            if detailed.is_finite() && detailed != h {
                row[gx] = detailed;
                changed += 1;
            }
        }
        changed
    }
}

/// The blocks of `step` x `step` cells over a grid whose cell (0, 0) is cell `origin` of a
/// larger grid the blocks are counted from.
#[derive(Clone, Copy)]
struct Lattice {
    origin: (i64, i64),
    step: usize,
    /// The block holding cell (0, 0), and the blocks across and down from it.
    first: (i64, i64),
    width: usize,
    height: usize,
}

impl Lattice {
    fn new(origin: (i64, i64), step: usize) -> Self {
        Self {
            origin,
            step,
            first: (0, 0),
            width: 0,
            height: 0,
        }
    }

    /// Covering a grid of `cols` x `rows` cells.
    fn over(self, cols: usize, rows: usize) -> Self {
        let block = |cell: i64| cell.div_euclid(self.step as i64);
        let first = (block(self.origin.0), block(self.origin.1));
        Self {
            first,
            width: (block(self.origin.0 + cols as i64 - 1) - first.0 + 1) as usize,
            height: (block(self.origin.1 + rows as i64 - 1) - first.1 + 1) as usize,
            ..self
        }
    }

    /// The block holding grid column `gx`, clamped to the lattice.
    fn block_x(&self, gx: i64) -> usize {
        ((gx + self.origin.0).div_euclid(self.step as i64) - self.first.0)
            .clamp(0, self.width as i64 - 1) as usize
    }

    /// The block row holding the row at `pos` (see `RowPlacement`), clamped to the lattice.
    fn block_z(&self, pos: f64) -> usize {
        ((pos.round() as i64 + self.origin.1).div_euclid(self.step as i64) - self.first.1)
            .clamp(0, self.height as i64 - 1) as usize
    }

    fn taps(cell: f64, origin: i64, step: usize, first: i64, blocks: usize) -> Taps {
        let centred = cell + origin as f64 - (step as f64 - 1.0) / 2.0;
        let f = (centred / step as f64 - first as f64).clamp(0.0, (blocks - 1) as f64);
        let lo = f.floor() as usize;
        Taps {
            lo,
            hi: (lo + 1).min(blocks - 1),
            t: (f - lo as f64) as f32,
        }
    }

    fn taps_x(&self, gx: usize) -> Taps {
        Self::taps(
            gx as f64,
            self.origin.0,
            self.step,
            self.first.0,
            self.width,
        )
    }

    /// Taps for the row at `pos` (see `RowPlacement`).
    fn taps_z(&self, pos: f64) -> Taps {
        Self::taps(pos, self.origin.1, self.step, self.first.1, self.height)
    }
}

/// Where the grid's rows lie on the lattice's rows. The providers sample equal steps of
/// latitude; a One World lays them onto its Mercator blocks only after the detail, so
/// there each row counts at the block row its latitude will land on, as the neighbouring
/// areas count it.
struct RowPlacement {
    lat_top: f64,
    lat_bottom: f64,
    /// Mercator northing of the top and bottom row, in a One World.
    mercator: Option<(f64, f64)>,
    last: f64,
}

impl RowPlacement {
    fn new(bbox: &LLBBox, rows: usize, mercator: bool) -> Self {
        let (lat_top, lat_bottom) = (bbox.max().lat(), bbox.min().lat());
        Self {
            lat_top,
            lat_bottom,
            mercator: mercator.then(|| (mercator_y_m(lat_top), mercator_y_m(lat_bottom))),
            last: rows.saturating_sub(1) as f64,
        }
    }

    /// Lattice row (fractional) of latitude `lat`.
    fn at_lat(&self, lat: f64) -> f64 {
        match self.mercator {
            Some((top, bottom)) if top != bottom => {
                (mercator_y_m(lat) - top) / (bottom - top) * self.last
            }
            _ if self.lat_top != self.lat_bottom => {
                (self.lat_top - lat) / (self.lat_top - self.lat_bottom) * self.last
            }
            _ => 0.0,
        }
    }

    /// Lattice row of grid row `gz` (fractional, as a way's points fall between rows).
    fn at_row(&self, gz: f64) -> f64 {
        if self.last <= 0.0 {
            return gz;
        }
        self.at_lat(self.lat_top - gz / self.last * (self.lat_top - self.lat_bottom))
    }
}

/// Two neighbouring blocks and the weight of the second, for blending along one axis.
#[derive(Clone, Copy)]
struct Taps {
    lo: usize,
    hi: usize,
    t: f32,
}

/// How much detail each block takes, 0 to 1: the coarser its source the more, none on or
/// beside ground a fine source covers or that is kept clear, blended between block centres.
struct Gate {
    lattice: Lattice,
    strength: Vec<f32>,
    /// Whether a row of blocks has any strength.
    live_rows: Vec<bool>,
}

impl Gate {
    /// None where no block takes any detail, so a fine source costs one pass over its runs.
    #[allow(clippy::too_many_arguments)]
    fn new(
        source: &SourcePixels,
        lattice: Lattice,
        placement: &RowPlacement,
        grid: (usize, usize),
        scale: f64,
        bbox: &LLBBox,
        cell_m: f64,
        land_cover: Option<&LandCoverData>,
        graded: &[Vec<(f64, f64)>],
    ) -> Option<Self> {
        let (width, height) = (lattice.width, lattice.height);
        let (cols, rows) = grid;
        // The grid rows of each block row; the rows run down the lattice in order.
        let mut block_rows = vec![0..0; height];
        for gz in 0..rows {
            let span = &mut block_rows[lattice.block_z(placement.at_row(gz as f64))];
            *span = if span.start == span.end {
                gz..gz + 1
            } else {
                span.start..gz + 1
            };
        }
        // The finest source pixel in each block decides it, so a block a fine source
        // reaches into takes nothing.
        let coarse: Vec<f32> = (0..height)
            .into_par_iter()
            .flat_map_iter(|bz| {
                let mut finest = vec![f32::INFINITY; width];
                for gz in block_rows[bz].clone() {
                    let runs = source.row(gz).runs();
                    for (i, &(start, metres)) in runs.iter().enumerate() {
                        let end = runs
                            .get(i + 1)
                            .map_or(cols as i64, |&(next, _)| i64::from(next));
                        let last = lattice.block_x(end - 1);
                        for block in &mut finest[lattice.block_x(i64::from(start))..=last] {
                            *block = block.min(metres);
                        }
                    }
                }
                finest.into_iter().map(|px| {
                    let px = f64::from(px);
                    if px.is_finite() {
                        smoothstep(RES_FROM_BLOCKS, RES_FULL_BLOCKS, px * scale) as f32
                    } else {
                        0.0
                    }
                })
            })
            .collect();
        if coarse.iter().all(|&s| s <= 0.0) {
            return None;
        }

        let mut keep = vec![1.0_f32; width * height];
        if let Some(lc) = land_cover {
            let cleared: Vec<Vec<bool>> = (0..height)
                .into_par_iter()
                .map(|bz| {
                    let mut blocks = vec![false; width];
                    for row in &lc.grid[block_rows[bz].clone()] {
                        for (gx, &class) in row.iter().enumerate() {
                            if class == LC_WATER || class == LC_BUILT_UP {
                                blocks[lattice.block_x(gx as i64)] = true;
                            }
                        }
                    }
                    blocks
                })
                .collect();
            for (k, clear) in keep.iter_mut().zip(cleared.into_iter().flatten()) {
                if clear {
                    *k = 0.0;
                }
            }
        }
        let reach = GRADED_CLEAR_M / cell_m;
        // Half a block apart, so the corridor has no gaps between its points.
        let spacing = (lattice.step as f64 / 2.0).max(0.5);
        for way in graded {
            let lone = (way.len() == 1).then(|| (way[0], way[0]));
            for (a, b) in way.windows(2).map(|w| (w[0], w[1])).chain(lone) {
                let [a, b] = [a, b].map(|p| cell_coords(bbox, grid, p));
                for (x, z) in points_on_segment(a, b, grid, reach, spacing) {
                    let xs = lattice.block_x((x - reach).floor() as i64)
                        ..=lattice.block_x((x + reach).ceil() as i64);
                    let z = placement.at_row(z);
                    for bz in lattice.block_z(z - reach)..=lattice.block_z(z + reach) {
                        keep[bz * width + xs.start()..=bz * width + xs.end()].fill(0.0);
                    }
                }
            }
        }

        let strength: Vec<f32> = feather(&coarse, width, height, SOURCE_FADE_BLOCKS)
            .into_iter()
            .zip(feather(&keep, width, height, CLEAR_FADE_BLOCKS))
            .map(|(c, k)| c * k)
            .collect();
        let live_rows = strength
            .chunks(width)
            .map(|r| r.iter().any(|&s| s > 0.0))
            .collect();
        Some(Self {
            lattice,
            strength,
            live_rows,
        })
    }

    fn any_in(&self, z: Taps) -> bool {
        self.live_rows[z.lo] || self.live_rows[z.hi]
    }

    fn at(&self, x: Taps, z: Taps) -> f64 {
        let width = self.lattice.width;
        let s = |bx: usize, bz: usize| self.strength[bz * width + bx];
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        f64::from(lerp(
            lerp(s(x.lo, z.lo), s(x.hi, z.lo), x.t),
            lerp(s(x.lo, z.hi), s(x.hi, z.hi), x.t),
            z.t,
        ))
    }
}

/// Grid coordinates (column, row; fractional) of a `(lat, lon)` point, rows north to south.
fn cell_coords(bbox: &LLBBox, (cols, rows): (usize, usize), (lat, lon): (f64, f64)) -> (f64, f64) {
    let (min, max) = (bbox.min(), bbox.max());
    (
        (lon - min.lng()) / (max.lng() - min.lng()) * (cols - 1) as f64,
        (max.lat() - lat) / (max.lat() - min.lat()) * (rows - 1) as f64,
    )
}

/// Points every `spacing` cells and at both ends along the part of the segment from `a` to
/// `b` (grid coordinates) that comes within `margin` cells of a grid of `cols` x `rows`.
fn points_on_segment(
    a: (f64, f64),
    b: (f64, f64),
    (cols, rows): (usize, usize),
    margin: f64,
    spacing: f64,
) -> impl Iterator<Item = (f64, f64)> {
    let (dx, dz) = (b.0 - a.0, b.1 - a.1);
    let (lo, hi_x, hi_z) = (
        -0.5 - margin,
        cols as f64 - 0.5 + margin,
        rows as f64 - 0.5 + margin,
    );
    // Liang-Barsky: the part of the segment inside the grid's widened edges.
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [
        (-dx, a.0 - lo),
        (dx, hi_x - a.0),
        (-dz, a.1 - lo),
        (dz, hi_z - a.1),
    ] {
        if p == 0.0 {
            if q.is_nan() || q < 0.0 {
                t1 = -1.0;
            }
        } else if p < 0.0 {
            t0 = t0.max(q / p);
        } else {
            t1 = t1.min(q / p);
        }
    }
    let steps = if t0 <= t1 {
        ((t1 - t0) * dx.hypot(dz) / spacing).ceil() as usize
    } else {
        0
    };
    let points = if t0 <= t1 { steps + 1 } else { 0 };
    (0..points).map(move |k| {
        let t = t0 + (t1 - t0) * k as f64 / steps.max(1) as f64;
        (a.0 + dx * t, a.1 + dz * t)
    })
}

/// `v` with what is zero widened by a block and faded in over `blocks` more, never rising
/// above the widened map, so a zero block and the next stay exactly zero.
fn feather(v: &[f32], width: usize, height: usize, blocks: usize) -> Vec<f32> {
    let hard = separable(v, width, height, |line| {
        (0..line.len())
            .map(|i| {
                line[i.saturating_sub(1)..(i + 2).min(line.len())]
                    .iter()
                    .copied()
                    .fold(f32::INFINITY, f32::min)
            })
            .collect()
    });
    let soft = separable(&hard, width, height, |line| {
        let mut sums = vec![0.0_f32; line.len() + 1];
        for (i, &x) in line.iter().enumerate() {
            sums[i + 1] = sums[i] + x;
        }
        (0..line.len())
            .map(|i| {
                let (lo, hi) = (i.saturating_sub(blocks), (i + blocks + 1).min(line.len()));
                (sums[hi] - sums[lo]) / (hi - lo) as f32
            })
            .collect()
    });
    soft.into_iter().zip(hard).map(|(s, h)| s.min(h)).collect()
}

/// `line` applied along every row of a `width` x `height` grid, then along every column.
fn separable(
    v: &[f32],
    width: usize,
    height: usize,
    line: impl Fn(&[f32]) -> Vec<f32>,
) -> Vec<f32> {
    let along_rows: Vec<f32> = v.chunks(width).flat_map(&line).collect();
    let mut out = vec![0.0; v.len()];
    let mut column = vec![0.0; height];
    for x in 0..width {
        for (z, c) in column.iter_mut().enumerate() {
            *c = along_rows[z * width + x];
        }
        for (z, value) in line(&column).into_iter().enumerate() {
            out[z * width + x] = value;
        }
    }
    out
}

fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Value noise in [-1, 1] at `p` on a unit lattice, smoothstep-blended between corners.
fn value_noise(p: [f64; 3], salt: u64) -> f64 {
    let base = p.map(f64::floor);
    let [tx, ty, tz] = [0, 1, 2].map(|i| {
        let f = p[i] - base[i];
        f * f * (3.0 - 2.0 * f)
    });
    let [x, y, z] = base.map(|v| v as i64);
    let corner = |dx: i64, dy: i64, dz: i64| lattice_value(x + dx, y + dy, z + dz, salt);
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let near = lerp(
        lerp(corner(0, 0, 0), corner(1, 0, 0), tx),
        lerp(corner(0, 1, 0), corner(1, 1, 0), tx),
        ty,
    );
    let far = lerp(
        lerp(corner(0, 0, 1), corner(1, 0, 1), tx),
        lerp(corner(0, 1, 1), corner(1, 1, 1), tx),
        ty,
    );
    lerp(near, far, tz)
}

/// A lattice point's value in [-1, 1].
fn lattice_value(x: i64, y: i64, z: i64, salt: u64) -> f64 {
    let h = hash64(hash64(hash64(salt ^ x as u64) ^ y as u64) ^ z as u64);
    (h >> 11) as f64 * (2.0 / (1u64 << 53) as f64) - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elevation::provider::SourceRow;

    /// A square about 450 m across near 46° N; 401 cells make them about 1.1 m.
    fn bbox() -> LLBBox {
        LLBBox::new(46.0, 8.0, 46.004, 8.0058).unwrap()
    }

    /// Heights rising north at `slope_deg` over a `side` x `side` grid of `bbox`.
    fn incline(bbox: &LLBBox, side: usize, slope_deg: f64) -> Vec<Vec<f64>> {
        let (h_m, _) = geo_distance(bbox.min(), bbox.max());
        let rise = slope_deg.to_radians().tan() * h_m / (side - 1) as f64;
        (0..side)
            .map(|gz| vec![1500.0 + (side - 1 - gz) as f64 * rise; side])
            .collect()
    }

    /// A source of `metres_at(column, row)` pixels.
    fn source_by(side: usize, metres_at: impl Fn(usize, usize) -> f32) -> SourcePixels {
        SourcePixels::new(
            (0..side)
                .map(|gz| {
                    let mut row = SourceRow::default();
                    (0..side).for_each(|gx| row.push(gx, metres_at(gx, gz)));
                    row
                })
                .collect(),
        )
    }

    fn source(side: usize, metres: f32) -> SourcePixels {
        source_by(side, |_, _| metres)
    }

    fn land_cover(side: usize, class_at: impl Fn(usize, usize) -> u8) -> LandCoverData {
        LandCoverData {
            grid: (0..side)
                .map(|z| (0..side).map(|x| class_at(x, z)).collect())
                .collect(),
            water_distance: vec![vec![0u8; side]; side],
            water_blend_cache: once_cell::sync::OnceCell::new(),
            beach_tiles: once_cell::sync::OnceCell::new(),
            inland_water: Default::default(),
            width: side,
            height: side,
            cells_per_meter: 1.0,
        }
    }

    /// Detail added to a 401-cell incline, cell by cell.
    fn added(
        slope_deg: f64,
        src: &SourcePixels,
        scale: f64,
        lc: Option<&LandCoverData>,
        graded: &[Vec<(f64, f64)>],
    ) -> Vec<Vec<f64>> {
        let b = bbox();
        let before = incline(&b, 401, slope_deg);
        let mut after = before.clone();
        add_steep_detail(&mut after, &b, src, scale, None, lc, graded);
        after
            .iter()
            .zip(&before)
            .map(|(a, b)| a.iter().zip(b).map(|(a, b)| a - b).collect())
            .collect()
    }

    fn rms(d: &[Vec<f64>]) -> f64 {
        let n = d.iter().map(Vec::len).sum::<usize>() as f64;
        (d.iter().flatten().map(|v| v * v).sum::<f64>() / n).sqrt()
    }

    #[test]
    fn steep_ground_from_glo30_gets_bounded_zero_mean_detail() {
        let d = added(50.0, &source(401, 30.0), 1.0, None, &[]);
        let all: Vec<f64> = d.iter().flatten().copied().collect();
        let mean = all.iter().sum::<f64>() / all.len() as f64;
        let peak = all.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
        assert!((1.0..5.0).contains(&rms(&d)), "rms {}", rms(&d));
        assert!(mean.abs() < 0.5, "mean {mean}");
        let octave_sum: f64 = OCTAVES.iter().map(|(_, w)| w).sum();
        assert!(peak <= AMPLITUDE_M * octave_sum, "peak {peak}");
    }

    #[test]
    fn fine_sources_keep_their_heights() {
        // LiDAR at 1 m, a 5 m DEM, and a 10 m DEM all resample smoothly enough.
        for source_m in [1.0, 5.0, 10.0] {
            let d = added(55.0, &source(401, source_m), 1.0, None, &[]);
            assert!(d.iter().flatten().all(|&v| v == 0.0), "{source_m} m source");
        }
    }

    #[test]
    fn gentle_ground_keeps_its_heights() {
        for slope in [0.0, 15.0, 30.0] {
            let d = added(slope, &source(401, 30.0), 1.0, None, &[]);
            assert!(d.iter().flatten().all(|&v| v == 0.0), "{slope}°");
        }
    }

    #[test]
    fn a_small_scale_makes_glo30_fine_enough() {
        // At a third of real size a 30 m pixel spans 10 blocks.
        let d = added(55.0, &source(401, 30.0), 1.0 / 3.0, None, &[]);
        assert!(d.iter().flatten().all(|&v| v == 0.0));
    }

    #[test]
    fn half_lidar_half_glo30_gets_detail_only_on_the_glo30_half() {
        // LiDAR west of column 200, GLO-30 east of it, across one steep face.
        let src = source_by(401, |gx, _| if gx < 200 { 0.8 } else { 30.0 });
        let d = added(50.0, &src, 1.0, None, &[]);
        for row in &d {
            // The LiDAR half and a block past it keep their heights exactly.
            assert!(row[..207].iter().all(|&v| v == 0.0));
        }
        // Past the fade the GLO-30 half has its full detail.
        let east: Vec<Vec<f64>> = d.iter().map(|r| r[270..].to_vec()).collect();
        assert!(rms(&east) > 1.0, "rms {}", rms(&east));
        // And it fades in between rather than starting at full strength.
        let fade: Vec<Vec<f64>> = d.iter().map(|r| r[207..230].to_vec()).collect();
        assert!(rms(&fade) < 0.7 * rms(&east));
    }

    #[test]
    fn a_fine_source_on_a_coarse_grid_is_left_alone() {
        let b = bbox();
        // 41 cells over ~450 m: 11 m cells, a grid that still draws the 40 m octave.
        let run = |metres: f32| {
            let mut heights = incline(&b, 41, 55.0);
            add_steep_detail(&mut heights, &b, &source(41, metres), 1.0, None, None, &[])
        };
        assert_eq!(run(0.5), 0);
        assert!(run(30.0) > 0);
    }

    #[test]
    fn a_grid_too_coarse_for_the_octaves_is_left_alone() {
        let b = bbox();
        let mut heights = incline(&b, 21, 55.0);
        let before = heights.clone();
        // 21 cells over ~450 m: 22 m cells, too coarse to draw even the 40 m octave.
        add_steep_detail(&mut heights, &b, &source(21, 30.0), 1.0, None, None, &[]);
        assert_eq!(heights, before);
    }

    #[test]
    fn water_and_built_up_land_stay_clear_with_a_margin() {
        // A lake across rows 180-219 and a village over columns 0-39.
        let lc = land_cover(401, |x, z| match (x, z) {
            (_, 180..=219) => LC_WATER,
            (0..=39, _) => LC_BUILT_UP,
            _ => 30,
        });
        let d = added(50.0, &source(401, 30.0), 1.0, Some(&lc), &[]);
        for (z, row) in d.iter().enumerate() {
            for (x, &v) in row.iter().enumerate() {
                // The cleared cells and a few blocks' worth around them.
                if (176..224).contains(&z) || x < 44 {
                    assert_eq!(v, 0.0, "detail at ({x}, {z})");
                }
            }
        }
        // Away from both the face still gets its detail.
        assert!(d[50].iter().skip(100).any(|&v| v != 0.0));
        assert!(d[350].iter().skip(100).any(|&v| v != 0.0));
    }

    #[test]
    fn a_graded_way_is_kept_clear_its_whole_width() {
        let b = bbox();
        // A road climbing the face along the middle column.
        let lon = (b.min().lng() + b.max().lng()) / 2.0;
        let road = vec![(b.min().lat() - 0.001, lon), (b.max().lat() + 0.001, lon)];
        let d = added(50.0, &source(401, 30.0), 1.0, None, &[road]);
        // Cells about 1.1 m across: a motorway and its shoulders either side stay clear.
        for row in &d {
            assert!(row[190..=210].iter().all(|&v| v == 0.0));
        }
        assert!(d.iter().any(|row| row[50] != 0.0));
    }

    #[test]
    fn the_area_keeps_its_lowest_and_highest_point() {
        // A face running right to the area's edges, so its ends are the extremes the fit
        // places the build height by.
        let b = bbox();
        let mut heights = incline(&b, 401, 55.0);
        let extremes = |h: &[Vec<f64>]| {
            h.iter()
                .flatten()
                .fold((f64::MAX, f64::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)))
        };
        let before = extremes(&heights);
        let changed = add_steep_detail(&mut heights, &b, &source(401, 30.0), 1.0, None, None, &[]);
        assert!(changed > 0);
        assert_eq!(extremes(&heights), before);
    }

    #[test]
    fn a_grid_without_heights_is_left_alone() {
        let mut heights = vec![vec![f64::NAN; 41]; 41];
        let src = source(41, 30.0);
        assert_eq!(
            add_steep_detail(&mut heights, &bbox(), &src, 1.0, None, None, &[]),
            0
        );
    }

    #[test]
    fn one_world_areas_agree_over_their_shared_margin() {
        // Two areas of one world, offset by 37 and 23 blocks: neither a whole number of the
        // detail's blocks. A road and a lake run through both, so the fades matter.
        let side = 301;
        let (lat0, lng0, dlat, dlng) = (46.0, 8.0, 0.00001, 0.0000145);
        let road_x = 190;
        let area = |ox: usize, oz: usize| {
            let b = LLBBox::new(
                lat0 - (oz + side - 1) as f64 * dlat,
                lng0 + ox as f64 * dlng,
                lat0 - oz as f64 * dlat,
                lng0 + (ox + side - 1) as f64 * dlng,
            )
            .unwrap();
            let mut h: Vec<Vec<f64>> = (0..side)
                .map(|z| vec![3000.0 - (oz + z) as f64 * 1.3; side])
                .collect();
            let lc = land_cover(side, |x, z| {
                if (150..170).contains(&(z + oz)) && x + ox < 120 {
                    LC_WATER
                } else {
                    30
                }
            });
            let lng = lng0 + road_x as f64 * dlng;
            let road = vec![(lat0 + 0.01, lng), (lat0 - 0.01, lng)];
            let origin = Some((ox as i64, oz as i64));
            add_steep_detail(
                &mut h,
                &b,
                &source(side, 30.0),
                1.0,
                origin,
                Some(&lc),
                &[road],
            );
            h
        };
        let (a, b) = (area(0, 0), area(37, 23));
        let mut beside_road = 0;
        // Inside both and clear of both areas' edges, as a One World's margin is.
        for (z, row) in b.iter().enumerate().take(200).skip(80) {
            for (x, &here) in row.iter().enumerate().take(220).skip(80) {
                let (ax, az) = (x + 37, z + 23);
                // Each area measures its cells from its own bbox, a rounding error apart; a
                // fade that differed would be centimetres to metres.
                assert!(
                    (a[az][ax] - here).abs() < 1e-4,
                    "differs at block ({ax}, {az})"
                );
                let detailed = (here - (3000.0 - az as f64 * 1.3)).abs() > 1e-6;
                if detailed && (ax as i64 - road_x as i64).abs() < 30 {
                    beside_road += 1;
                }
            }
        }
        assert!(beside_road > 0, "the road's fade is not covered");
    }

    #[test]
    fn one_world_rows_count_on_the_world_block_they_land_on() {
        use crate::projection::web_mercator::mercator_lat_deg;
        // A world whose blocks are 1.4 Mercator metres tall, counted south from 46° N. An
        // area's rows run at equal latitude steps between its first and last block centre.
        let (y0, block) = (mercator_y_m(46.0), 1.4);
        let lat_of = |centre: f64| mercator_lat_deg(y0 - centre * block);
        let area = |oz: f64, rows: usize| {
            let top = lat_of(oz + 0.5);
            let bottom = lat_of(oz + rows as f64 - 0.5);
            RowPlacement::new(&LLBBox::new(bottom, 8.0, top, 8.1).unwrap(), rows, true)
        };
        // Two areas about 6 km tall, the second starting 5000 blocks below the first.
        let (a, b) = (area(0.0, 6001), area(5000.0, 6001));
        for centre in [5000.5, 5600.25, 6000.5] {
            let lat = lat_of(centre);
            assert!((a.at_lat(lat) - (b.at_lat(lat) + 5000.0)).abs() < 1e-6);
            assert!((a.at_lat(lat) - (centre - 0.5)).abs() < 1e-6);
        }
        // Halfway down an area a row lands most of a block off its index.
        let off = (a.at_row(3000.0) - 3000.0).abs();
        assert!(off > 0.5, "{off}");
        // Outside a One World the rows are their index.
        let flat = RowPlacement::new(&bbox(), 401, false);
        assert!((flat.at_row(123.0) - 123.0).abs() < 1e-9);
    }

    #[test]
    fn a_lattice_counts_its_blocks_from_the_world_grid() {
        let l = Lattice::new((37, -5), 8).over(100, 50);
        // World cells 37..=136 and -5..=44: blocks 4..=17 and -1..=5.
        assert_eq!((l.first, l.width, l.height), ((4, -1), 14, 7));
        assert_eq!((l.block_x(0), l.block_x(2), l.block_x(3)), (0, 0, 1));
        assert_eq!((l.block_z(4.0), l.block_z(4.6)), (0, 1));
        // World cell 46 lies 2.5 cells past the centre of block 5 (cells 40-47), on the way
        // to block 6's.
        let t = l.taps_x(46 - 37);
        assert_eq!((t.lo, t.hi, t.t), (1, 2, 2.5 / 8.0));
    }

    #[test]
    fn feathering_keeps_a_cleared_block_and_the_next_at_zero() {
        let mut v = vec![1.0_f32; 20];
        v[5] = 0.0;
        let f = feather(&v, 20, 1, 4);
        assert_eq!(&f[4..=6], &[0.0, 0.0, 0.0]);
        assert!(f[7] > 0.0 && f[7] < f[9] && f[9] < 1.0);
        assert_eq!(f[12], 1.0);
    }

    #[test]
    fn segments_are_clipped_to_the_grid_and_its_margin() {
        let grid = (10, 10);
        let pts: Vec<_> = points_on_segment((-50.0, 5.0), (60.0, 5.0), grid, 2.0, 0.5).collect();
        assert!(pts
            .iter()
            .all(|&(x, z)| (-2.5..=11.5).contains(&x) && z == 5.0));
        assert!(pts.first().unwrap().0 < -2.4 && pts.last().unwrap().0 > 11.4);
        assert_eq!(
            points_on_segment((-9.0, -9.0), (-5.0, 20.0), grid, 2.0, 0.5).count(),
            0
        );
        assert_eq!(
            points_on_segment((3.2, 4.0), (3.2, 4.0), grid, 2.0, 0.5).collect::<Vec<_>>(),
            vec![(3.2, 4.0)]
        );
    }

    #[test]
    fn value_noise_is_continuous_and_in_range() {
        let mut prev = value_noise([0.0, 0.3, 0.7], 1);
        for i in 1..=4000 {
            let v = value_noise([i as f64 * 0.001, 0.3, 0.7], 1);
            assert!((-1.0..=1.0).contains(&v));
            assert!((v - prev).abs() < 0.01, "jump at {i}");
            prev = v;
        }
    }
}
