//! Registering a point cloud against the footprints, and the align stage
//! driver. Port of `tools/facade_lab/register.py`.
//!
//! OSM outlines and Mapillary imagery drift apart by a few metres, and the
//! difference is not GPS error: `computed_geometry` already is the SfM camera
//! position and agrees with the cluster shot centre to under a metre. So the
//! cameras stay put and the cloud is what gets matched to the footprints. Each
//! pano's own cluster points in the facade band, 3 to 22 m above its ground and
//! within 40 m, are pushed over the distance transform of the OSM outlines on a
//! coarse 12 m grid at 1 m, then finely over 1.5 m at 0.25 m, minimising
//! `mean(min(d, 3 m)^2)`; a rotation of plus or minus 3 degrees is swept only
//! when the translation alone leaves the inlier share under 0.5.
//!
//! A solution is accepted when the inliers reach 0.5, the 25 m radius solution
//! agrees with the 40 m one within 1.5 m, the best minimum beats the best
//! *distinct* one (more than 4 m away) by 20 per cent of cost, and the shift is
//! under 12 m. That happens on about a quarter of the panos; the rest fall back
//! to one shift for the whole cluster, and failing that keep the raw pose.
//!
//! The shift is `(dx, dy, theta_deg)` about the raw camera centre, applied by
//! [`super::sfm::shift_points`]. OSM never moves.
//!
//! [`run_align`] is the stage driver: it registers every pano, builds the depth
//! maps, re-runs the visibility gates from the *registered* centres (CRITIQUE
//! C22), fits a plane per (wall, view) (A5) with a per view foot z (A3), renders
//! the preview crop of each surviving candidate to run the image gates, and then
//! calls [`super::visibility::select_views`] once per wall.
//!
//! Where this differs from the Python, and why:
//!
//! * **There is no subsample.** The Python caps the search at 4000 band points
//!   and the cluster sweep at 6000, drawn by `numpy.random.Generator.choice`
//!   from a seeded PCG64. The port searches every point instead, and the reason
//!   is measured rather than aesthetic.
//!
//!   The port cannot draw the Python's subset. `choice` picks *positions* in the
//!   band array, and that array is `cluster.near`'s output: in the Python, the
//!   cloud in `reconstruction.json` key order, filtered through the traversal
//!   order of a `scipy.spatial.cKDTree`. Neither order is portable, so the port
//!   necessarily samples a different subset of the same band. On the Munich box
//!   the two bands are the same *set* on all 1076 panos, point for point.
//!
//!   A different subset of the same band is harmless for the search and fatal
//!   for the verdict, because the acceptance gates are thresholds on statistics
//!   of whatever was sampled. Registering the box with a stride subsample flips
//!   **83 of 1076 panos** against the reference run: 40 of them accept or reject
//!   differently, and a pano that loses its registration keeps its raw pose,
//!   which is a median 5.15 m away. `1775856856766917` is the example the port
//!   was caught on: the same shift `(-3.5, -1.75)` on both, `inliers_after`
//!   0.5190 on the Python's 4000 points, 0.4990 on the port's 4000, 0.5028 on
//!   all 13 244, against a threshold of exactly 0.5. The wall it was the only
//!   roof view of came out 7.75 m short.
//!
//!   With the whole band the answer is a property of the cloud rather than of
//!   an array order, and it is exactly what `register.py` returns with its own
//!   `_subsample` disabled, so the two implementations agree by construction
//!   instead of by luck. It costs 1.28x the distance lookups in the local
//!   search: the mean band is 3057 points against the 2387 the cap allowed.
//!   [`subsample`] stays for callers that want a bounded point list, and
//!   `align/reg.json` records per pano what a 4000-point sample would have
//!   decided instead, so the size of the effect travels with the fixture.
//! * **The outline raster is OpenCV's.** `cv2.polylines` with `LINE_8` and a
//!   3-bit shift is a fixed point DDA, not a naive Bresenham, and the distance
//!   transform is read back bilinearly, so a pixel drawn one cell over moves a
//!   cost. [`draw_line`] is that DDA, [`edt`] is the exact Euclidean transform
//!   `scipy.ndimage.distance_transform_edt` computes, and [`OutlineDT::distance`]
//!   is `map_coordinates(order=1, mode='constant')`, which answers `DT_FAR`
//!   outside the grid rather than extrapolating.
//! * **The costs accumulate in `f64`** where numpy's `mean` runs in `float32`;
//!   the distance lookups themselves are rounded to `f32` as scipy rounds them,
//!   because that quantisation is the one big enough to move a minimum.
//! * **Depth maps are not written to disk** by [`write_products`]. The Python
//!   writes an `.npz` per pano for its own sheet stage; the Rust review dump uses
//!   the 16-bit PNG form `export_golden_sfm.py` already defined.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use image::RgbImage;
use rayon::prelude::*;

use super::imgops::median_in_place;
use super::plane::{self, EndSource};
use super::pose;
use super::sfm::{self, Cluster, DepthMap};
use super::types::{
    view_key, Building, Camera, PanoMeta, Params, PlaneFit, PlaneSource, RegResult, RegSource,
    ViewCandidate, Wall,
};
use super::visibility::{self, Footprints, PreviewOpts};

/// The cap the Python's search used to draw a random subset down to, kept
/// because the golden fixture records what that cap would have decided and the
/// module header says why it is gone.
pub const REG_MAX_POINTS: usize = 4000;
/// Fewer band points than this and the local search is not attempted.
pub const REG_MIN_POINTS: usize = 100;
/// The cost saturates here, so a point that is far from every outline weighs the
/// same as one that is very far.
pub const COST_CAP_M: f64 = 3.0;
/// A point this close to an outline counts as an inlier.
pub const INLIER_M: f64 = 1.0;
/// Two minima this far apart are different solutions rather than one.
pub const DISTINCT_MIN_M: f64 = 4.0;
pub const MAX_SHIFT_M: f64 = 12.0;
pub const GLOBAL_MARGIN_M: f64 = 50.0;
pub const GLOBAL_RANGE_M: f64 = 8.0;
pub const GLOBAL_STEP_M: f64 = 0.25;
pub const GLOBAL_MAX_POINTS: usize = 6000;
pub const GLOBAL_MIN_FRACTION: f64 = 0.08;
pub const GLOBAL_MIN_RATIO: f64 = 2.0;
/// What a lookup outside the distance transform's grid answers.
pub const DT_FAR: f64 = 1e5;

// --------------------------------------------------------------------------- outline distance transform

/// The rasterised OSM outlines with the Euclidean distance to them, in metres.
///
/// Exterior rings and holes are drawn into a `size_m` square grid about
/// `centre` at `res_m` per cell, exactly as `cv2.polylines` draws them, and
/// [`OutlineDT::distance`] reads the transform back bilinearly.
#[derive(Clone, Debug)]
pub struct OutlineDT {
    pub centre: [f64; 2],
    pub res: f64,
    pub size_m: f64,
    /// Cells a side.
    pub n: usize,
    pub origin: [f64; 2],
    /// Row major distance in metres, `f32` as the Python stores it.
    pub dt: Vec<f32>,
}

impl OutlineDT {
    pub fn new(buildings: &[Building], centre_xy: [f64; 2], size_m: f64, res_m: f64) -> Self {
        let n = (size_m / res_m).ceil() as usize + 1;
        let origin = [centre_xy[0] - 0.5 * size_m, centre_xy[1] - 0.5 * size_m];
        let mut mask = vec![false; n * n];
        for b in buildings {
            for ring in std::iter::once(&b.ring).chain(b.holes.iter()) {
                if ring.len() < 2 {
                    continue;
                }
                // The Python hands cv2 the ring in 1/8 pixel fixed point, which
                // is `shift = 3`, rounding halves to even the way numpy does.
                let px: Vec<[i64; 2]> = ring
                    .iter()
                    .map(|p| {
                        [
                            super::imgops::round_half_even((p[0] - origin[0]) / res_m * 8.0) as i64,
                            super::imgops::round_half_even((p[1] - origin[1]) / res_m * 8.0) as i64,
                        ]
                    })
                    .collect();
                let mut p0 = px[px.len() - 1];
                for p in &px {
                    draw_line(&mut mask, n, p0, *p);
                    p0 = *p;
                }
            }
        }
        let d2 = edt(&mask, n);
        let dt = d2
            .iter()
            .map(|v| (v.sqrt() * res_m) as f32)
            .collect::<Vec<f32>>();
        Self {
            centre: centre_xy,
            res: res_m,
            size_m,
            n,
            origin,
            dt,
        }
    }

    /// Distance to the nearest outline, at least [`DT_FAR`] outside the grid.
    ///
    /// `f32` because scipy interpolates a `float32` grid into a `float32`
    /// answer, and that rounding is the one the cost surface can feel.
    pub fn distance(&self, xy: [f64; 2]) -> f32 {
        let fx = (xy[0] - self.origin[0]) / self.res;
        let fy = (xy[1] - self.origin[1]) / self.res;
        let last = (self.n - 1) as f64;
        if !(0.0..=last).contains(&fx) || !(0.0..=last).contains(&fy) {
            return DT_FAR as f32;
        }
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as usize, y0 as usize);
        let x1 = (x0 + 1).min(self.n - 1);
        let y1 = (y0 + 1).min(self.n - 1);
        let at = |x: usize, y: usize| f64::from(self.dt[y * self.n + x]);
        let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
        let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
        (top * (1.0 - ty) + bottom * ty) as f32
    }

    pub fn contains(&self, xy: [f64; 2]) -> bool {
        let fx = (xy[0] - self.origin[0]) / self.res;
        let fy = (xy[1] - self.origin[1]) / self.res;
        let last = (self.n - 1) as f64;
        (0.0..=last).contains(&fx) && (0.0..=last).contains(&fy)
    }
}

/// One transform whose grid covers every point plus `margin_m`.
pub fn covering_dt(
    buildings: &[Building],
    points_xy: &[[f64; 2]],
    margin_m: f64,
    res_m: f64,
) -> OutlineDT {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in points_xy {
        for i in 0..2 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    if !lo[0].is_finite() {
        lo = [0.0, 0.0];
        hi = [0.0, 0.0];
    }
    let centre = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])];
    let size = (hi[0] - lo[0]).max(hi[1] - lo[1]) + 2.0 * margin_m;
    OutlineDT::new(buildings, centre, size, res_m)
}

const XY_SHIFT: u32 = 16;
const XY_ONE: i64 = 1 << XY_SHIFT;
/// `cv2.polylines` is handed 1/8 pixel fixed point, which OpenCV calls shift 3.
const POLY_SHIFT: u32 = 3;

/// The eight-connected line `cv2.polylines` draws for `LINE_8` with a subpixel
/// shift: a fixed point DDA, not a plain Bresenham.
///
/// The endpoints are clipped to the canvas in the shifted domain, walked one
/// pixel of the major axis at a time from the rounded start, and then the
/// rounded far endpoint is set as well. That last pixel is the one detail worth
/// naming: it is not another DDA step, it is the endpoint itself, and without it
/// 759 of this box's 1421 outline segments come out a pixel short. With it the
/// raster is identical to OpenCV's on every segment.
fn draw_line(mask: &mut [bool], n: usize, a: [i64; 2], b: [i64; 2]) {
    let mut p0 = [
        a[0] << (XY_SHIFT - POLY_SHIFT),
        a[1] << (XY_SHIFT - POLY_SHIFT),
    ];
    let mut p1 = [
        b[0] << (XY_SHIFT - POLY_SHIFT),
        b[1] << (XY_SHIFT - POLY_SHIFT),
    ];
    let scaled = (n as i64) << XY_SHIFT;
    if !clip_line(scaled, scaled, &mut p0, &mut p1) {
        return;
    }
    let mut dx = p1[0] - p0[0];
    let mut dy = p1[1] - p0[1];
    let j = if dx < 0 { -1i64 } else { 0 };
    let ax = (dx ^ j) - j;
    let i = if dy < 0 { -1i64 } else { 0 };
    let ay = (dy ^ i) - i;
    let (x_step, y_step, ecount);
    if ax > ay {
        dy = (dy ^ j) - j;
        if j != 0 {
            std::mem::swap(&mut p0, &mut p1);
        }
        x_step = XY_ONE;
        y_step = (dy << XY_SHIFT) / (ax | 1);
        ecount = (p1[0] - p0[0]) >> XY_SHIFT;
    } else {
        dx = (dx ^ i) - i;
        if i != 0 {
            std::mem::swap(&mut p0, &mut p1);
        }
        x_step = (dx << XY_SHIFT) / (ay | 1);
        y_step = XY_ONE;
        ecount = (p1[1] - p0[1]) >> XY_SHIFT;
    }
    let half = XY_ONE >> 1;
    let end = [(p1[0] + half) >> XY_SHIFT, (p1[1] + half) >> XY_SHIFT];
    p0[0] += half;
    p0[1] += half;
    let mut set = |x: i64, y: i64| {
        if x >= 0 && y >= 0 && (x as usize) < n && (y as usize) < n {
            mask[y as usize * n + x as usize] = true;
        }
    };
    for _ in 0..=ecount {
        set(p0[0] >> XY_SHIFT, p0[1] >> XY_SHIFT);
        p0[0] += x_step;
        p0[1] += y_step;
    }
    set(end[0], end[1]);
}

/// `cv::clipLine` on the scaled canvas, integer arithmetic and all.
fn clip_line(w: i64, h: i64, p1: &mut [i64; 2], p2: &mut [i64; 2]) -> bool {
    if w <= 0 || h <= 0 {
        return false;
    }
    let (right, bottom) = (w - 1, h - 1);
    let (mut x1, mut y1) = (p1[0], p1[1]);
    let (mut x2, mut y2) = (p2[0], p2[1]);
    let code = |x: i64, y: i64| {
        i32::from(x < 0)
            + i32::from(x > right) * 2
            + i32::from(y < 0) * 4
            + i32::from(y > bottom) * 8
    };
    let mut c1 = code(x1, y1);
    let mut c2 = code(x2, y2);
    if (c1 & c2) == 0 && (c1 | c2) != 0 {
        if c1 & 12 != 0 {
            let a = if c1 < 8 { 0 } else { bottom };
            x1 += (a - y1) * (x2 - x1) / (y2 - y1);
            y1 = a;
            c1 = i32::from(x1 < 0) + i32::from(x1 > right) * 2;
        }
        if c2 & 12 != 0 {
            let a = if c2 < 8 { 0 } else { bottom };
            x2 += (a - y2) * (x1 - x2) / (y1 - y2);
            y2 = a;
            c2 = i32::from(x2 < 0) + i32::from(x2 > right) * 2;
        }
        if (c1 & c2) == 0 && (c1 | c2) != 0 {
            if c1 != 0 {
                let a = if c1 == 1 { 0 } else { right };
                y1 += (a - x1) * (y2 - y1) / (x2 - x1);
                x1 = a;
                c1 = 0;
            }
            if c2 != 0 {
                let a = if c2 == 1 { 0 } else { right };
                y2 += (a - x2) * (y1 - y2) / (x1 - x2);
                x2 = a;
                c2 = 0;
            }
        }
        *p1 = [x1, y1];
        *p2 = [x2, y2];
    }
    (c1 | c2) == 0
}

/// The exact squared Euclidean distance transform in cells, to the nearest set
/// pixel of `mask`.
///
/// Felzenszwalb and Huttenlocher's lower envelope of parabolas, one pass down
/// the columns and one along the rows. Exact, so it agrees with scipy's own
/// exact transform cell for cell.
fn edt(mask: &[bool], n: usize) -> Vec<f64> {
    let inf = f64::INFINITY;
    let mut f = vec![0.0f64; n * n];
    for (k, m) in mask.iter().enumerate() {
        f[k] = if *m { 0.0 } else { inf };
    }
    let mut column = vec![0.0f64; n];
    let mut out = vec![0.0f64; n * n];
    // Down the columns.
    for x in 0..n {
        for y in 0..n {
            column[y] = f[y * n + x];
        }
        let d = envelope(&column);
        for y in 0..n {
            out[y * n + x] = d[y];
        }
    }
    // Along the rows.
    let mut row = vec![0.0f64; n];
    for y in 0..n {
        row.copy_from_slice(&out[y * n..(y + 1) * n]);
        let d = envelope(&row);
        out[y * n..(y + 1) * n].copy_from_slice(&d);
    }
    out
}

/// The 1D squared distance transform of a sampled function.
fn envelope(f: &[f64]) -> Vec<f64> {
    let n = f.len();
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f64; n + 1];
    let mut k = 0usize;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        if !f[q].is_finite() {
            continue;
        }
        loop {
            let p = v[k];
            let s = if f[p].is_finite() {
                ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64))
                    / (2.0 * q as f64 - 2.0 * p as f64)
            } else {
                f64::NEG_INFINITY
            };
            if s <= z[k] && k > 0 {
                k -= 1;
                continue;
            }
            let s = if f[p].is_finite() {
                s
            } else {
                f64::NEG_INFINITY
            };
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f64::INFINITY;
            break;
        }
    }
    let mut out = vec![0.0f64; n];
    let mut k = 0usize;
    for (q, slot) in out.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        *slot = if f[p].is_finite() {
            let d = q as f64 - p as f64;
            d * d + f[p]
        } else {
            f64::INFINITY
        };
    }
    out
}

// --------------------------------------------------------------------------- band and cost

/// The cloud points in the facade band about one camera: `z` between
/// `z_g + band[0]` and `z_g + band[1]`, within `radius_m` in plan.
pub fn facade_band(
    cluster: &Cluster,
    centre: [f64; 3],
    z_g: f64,
    radius_m: f64,
    band: [f64; 2],
) -> Vec<[f64; 3]> {
    cluster
        .near([centre[0], centre[1]], radius_m)
        .into_iter()
        .filter(|p| p[2] >= z_g + band[0] && p[2] <= z_g + band[1])
        .collect()
}

/// An evenly spaced subset of at most `n_max` points.
///
/// The Python draws a seeded random subset; see the module header for why this
/// is a stride instead and what the golden test measures about the difference.
pub fn subsample<T: Copy>(pts: &[T], n_max: usize) -> Vec<T> {
    if pts.len() <= n_max || n_max == 0 {
        return pts.to_vec();
    }
    (0..n_max).map(|i| pts[i * pts.len() / n_max]).collect()
}

fn rotated(xy: &[[f64; 2]], centre: [f64; 2], theta_deg: f64) -> Vec<[f64; 2]> {
    if theta_deg.abs() < 1e-12 {
        return xy.to_vec();
    }
    let (s, c) = theta_deg.to_radians().sin_cos();
    xy.iter()
        .map(|p| {
            let rel = [p[0] - centre[0], p[1] - centre[1]];
            [
                c * rel[0] - s * rel[1] + centre[0],
                s * rel[0] + c * rel[1] + centre[1],
            ]
        })
        .collect()
}

/// `mean(min(d, 3)^2)` for every offset applied to the points.
fn costs(dt: &OutlineDT, xy: &[[f64; 2]], offsets: &[[f64; 2]]) -> Vec<f64> {
    let cap = COST_CAP_M as f32;
    offsets
        .iter()
        .map(|off| {
            let mut acc = 0.0f64;
            for p in xy {
                let d = dt.distance([p[0] + off[0], p[1] + off[1]]).min(cap);
                acc += f64::from(d * d);
            }
            acc / xy.len().max(1) as f64
        })
        .collect()
}

/// The square offset grid `numpy.meshgrid` builds, x varying fastest, and its
/// side.
fn grid(range_m: f64, step_m: f64) -> (Vec<[f64; 2]>, usize) {
    let n = (range_m / step_m).round() as i64;
    let side = (2 * n + 1) as usize;
    let mut out = Vec::with_capacity(side * side);
    for i in -n..=n {
        for j in -n..=n {
            out.push([j as f64 * step_m, i as f64 * step_m]);
        }
    }
    (out, side)
}

/// Flat indices of the cells no larger than any of their eight neighbours.
fn local_minima(cost: &[f64], side: usize) -> Vec<usize> {
    let at = |i: i64, j: i64| -> f64 {
        if i < 0 || j < 0 || i >= side as i64 || j >= side as i64 {
            f64::INFINITY
        } else {
            cost[i as usize * side + j as usize]
        }
    };
    let mut out = Vec::new();
    for i in 0..side as i64 {
        for j in 0..side as i64 {
            let c = at(i, j);
            let mut best = true;
            for di in -1..=1 {
                for dj in -1..=1 {
                    if (di != 0 || dj != 0) && c > at(i + di, j + dj) {
                        best = false;
                    }
                }
            }
            if best {
                out.push(i as usize * side + j as usize);
            }
        }
    }
    out
}

fn fine(dt: &OutlineDT, xy: &[[f64; 2]], around: [f64; 2], params: &Params) -> (f64, f64, f64) {
    let (offs, _) = grid(params.reg_fine_m, params.reg_step_fine);
    let offs: Vec<[f64; 2]> = offs
        .iter()
        .map(|o| [o[0] + around[0], o[1] + around[1]])
        .collect();
    let cost = costs(dt, xy, &offs);
    let k = argmin(&cost);
    (offs[k][0], offs[k][1], cost[k])
}

fn argmin(v: &[f64]) -> usize {
    let mut best = 0usize;
    for (i, x) in v.iter().enumerate() {
        if *x < v[best] {
            best = i;
        }
    }
    best
}

fn argmax(v: &[f64]) -> usize {
    let mut best = 0usize;
    for (i, x) in v.iter().enumerate() {
        if *x > v[best] {
            best = i;
        }
    }
    best
}

/// One coarse search and one fine refinement, with the ranked coarse minima.
///
/// `theta_deg` turns the points about `centre` first. The ranked list is the
/// coarse local minima sorted by cost, with the best and the best distinct
/// runner-up both refined, which is what the uniqueness test compares.
pub fn search_shift(
    pts: &[[f64; 2]],
    dt: &OutlineDT,
    centre: [f64; 2],
    params: &Params,
    theta_deg: f64,
) -> (f64, f64, f64, Vec<[f64; 3]>) {
    let xy = rotated(pts, centre, theta_deg);
    let (offs, side) = grid(params.reg_coarse_m, 1.0);
    let cost = costs(dt, &xy, &offs);
    let mut mins = local_minima(&cost, side);
    mins.sort_by(|a, b| cost[*a].total_cmp(&cost[*b]));
    if mins.is_empty() {
        mins.push(argmin(&cost));
    }
    let mut ranked: Vec<[f64; 3]> = mins
        .iter()
        .map(|i| [offs[*i][0], offs[*i][1], cost[*i]])
        .collect();
    let (dx, dy, c) = fine(dt, &xy, offs[mins[0]], params);
    ranked[0] = [dx, dy, c];
    if let Some(runner_up) = ranked
        .iter_mut()
        .skip(1)
        .find(|r| (r[0] - dx).hypot(r[1] - dy) > DISTINCT_MIN_M)
    {
        let (fx, fy, fc) = fine(dt, &xy, [runner_up[0], runner_up[1]], params);
        *runner_up = [fx, fy, fc];
    }
    ranked.sort_by(|a, b| a[2].total_cmp(&b[2]));
    (dx, dy, c, ranked)
}

/// The share of the band within 1 m of an outline after a shift.
pub fn inlier_share(
    pts: &[[f64; 2]],
    dt: &OutlineDT,
    dx: f64,
    dy: f64,
    theta_deg: f64,
    centre: [f64; 2],
) -> f64 {
    if pts.is_empty() {
        return 0.0;
    }
    let xy = rotated(pts, centre, theta_deg);
    let hits = xy
        .iter()
        .filter(|p| f64::from(dt.distance([p[0] + dx, p[1] + dy])) < INLIER_M)
        .count();
    hits as f64 / xy.len() as f64
}

/// The cheapest ranked minimum more than 4 m from the winner.
pub fn second_best_distinct(ranked: &[[f64; 3]], dx: f64, dy: f64) -> Option<[f64; 3]> {
    ranked
        .iter()
        .find(|r| (r[0] - dx).hypot(r[1] - dy) > DISTINCT_MIN_M)
        .copied()
}

// --------------------------------------------------------------------------- per pano registration

/// When the rotation is swept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ThetaMode {
    /// Only when the translation alone leaves the inliers under the threshold.
    #[default]
    Auto,
    Never,
    Always,
}

/// One pano's registration with the runner-up the uniqueness test used.
///
/// `second_best` is `(dx, dy, cost)`; it lives here rather than on
/// [`RegResult`] because it is a diagnostic of the search, not of the answer.
#[derive(Clone, Debug, Default)]
pub struct Registration {
    pub pano_id: String,
    pub result: RegResult,
    pub second_best: Option<[f64; 3]>,
}

impl Registration {
    fn empty(pano_id: &str, source: RegSource) -> Self {
        Self {
            pano_id: pano_id.to_string(),
            result: RegResult {
                source,
                ..RegResult::default()
            },
            second_best: None,
        }
    }
}

/// Registers one pano's cluster points against the outlines.
///
/// `cam` is the raw camera: the unregistered centre, the cluster's z datum and
/// its ground level. `global_shift` is the cluster-wide fallback, used when the
/// local solution is rejected. Never moves OSM.
pub fn register_pano(
    cam: &Camera,
    cluster: Option<&Cluster>,
    dt: &OutlineDT,
    params: &Params,
    global_shift: Option<[f64; 3]>,
    theta_mode: ThetaMode,
) -> Registration {
    let Some(cluster) = cluster.filter(|c| !c.is_empty()) else {
        return Registration::empty(&cam.pano_id, RegSource::NoCluster);
    };
    let z_g = if cam.ground_z.is_finite() {
        cam.ground_z
    } else {
        cam.centre[2] - cam.cam_height_m
    };
    let band_all = facade_band(
        cluster,
        cam.centre,
        z_g,
        params.reg_radius_m,
        params.reg_band,
    );
    // Every band point, not a sample of `REG_MAX_POINTS`: see the module header.
    let band: Vec<[f64; 2]> = band_all.iter().map(|p| [p[0], p[1]]).collect();
    register_band(
        &cam.pano_id,
        &band,
        band_all.len(),
        [cam.centre[0], cam.centre[1]],
        dt,
        params,
        global_shift,
        theta_mode,
    )
}

/// The registration of one already extracted band, so a fixture can drive the
/// search and the acceptance ladder from stored points.
///
/// `band_points` is how many points the band held, which is what the minimum
/// size test looks at. It is the band's own length in the pipeline; a caller
/// measuring what a sample would have decided passes the whole band's length
/// with a sample of it.
#[allow(clippy::too_many_arguments)]
pub fn register_band(
    pano_id: &str,
    band: &[[f64; 2]],
    band_points: usize,
    centre: [f64; 2],
    dt: &OutlineDT,
    params: &Params,
    global_shift: Option<[f64; 3]>,
    theta_mode: ThetaMode,
) -> Registration {
    let mut reg = Registration::empty(pano_id, RegSource::Unregistered);
    reg.result.n_points = band_points;
    if band_points < REG_MIN_POINTS {
        return with_global(&reg, band, dt, centre, global_shift);
    }
    reg.result.inliers_before = inlier_share(band, dt, 0.0, 0.0, 0.0, centre);
    let (mut dx, mut dy, mut cost, mut ranked) = search_shift(band, dt, centre, params, 0.0);
    let mut theta = 0.0;
    let mut inl = inlier_share(band, dt, dx, dy, 0.0, centre);
    let sweep = theta_mode == ThetaMode::Always
        || (theta_mode == ThetaMode::Auto && inl < params.reg_min_inliers);
    if sweep {
        let n_th = (params.reg_theta_deg / params.reg_theta_step).round() as i64;
        let mut best = (cost, dx, dy, 0.0, ranked.clone());
        for k in -n_th..=n_th {
            if k == 0 {
                continue;
            }
            let th = k as f64 * params.reg_theta_step;
            let (dx_t, dy_t, cost_t, ranked_t) = search_shift(band, dt, centre, params, th);
            if cost_t < best.0 {
                best = (cost_t, dx_t, dy_t, th, ranked_t);
            }
        }
        cost = best.0;
        dx = best.1;
        dy = best.2;
        theta = best.3;
        ranked = best.4;
        inl = inlier_share(band, dt, dx, dy, theta, centre);
    }
    let second = second_best_distinct(&ranked, dx, dy);
    let ratio = match second {
        Some(s) => s[2] / cost.max(1e-9),
        None => f64::INFINITY,
    };
    let near: Vec<[f64; 2]> = band
        .iter()
        .filter(|p| (p[0] - centre[0]).hypot(p[1] - centre[1]) <= params.reg_check_radius_m)
        .copied()
        .collect();
    let agreement = if near.len() >= REG_MIN_POINTS / 2 {
        let (dx25, dy25, _, _) = search_shift(&near, dt, centre, params, theta);
        (dx - dx25).hypot(dy - dy25)
    } else {
        f64::INFINITY
    };
    let shift_len = dx.hypot(dy);
    let accepted = inl >= params.reg_min_inliers
        && agreement <= params.reg_fine_m
        && ratio >= params.reg_uniqueness
        && shift_len <= MAX_SHIFT_M;
    reg.result.dx = dx;
    reg.result.dy = dy;
    reg.result.theta_deg = theta;
    reg.result.inliers_after = inl;
    reg.result.ambiguity_ratio = if ratio.is_finite() { ratio } else { 99.0 };
    reg.result.radius_agreement_m = if agreement.is_finite() {
        agreement
    } else {
        99.0
    };
    reg.second_best = second;
    reg.result.accepted = accepted;
    reg.result.source = if accepted {
        RegSource::Local
    } else {
        RegSource::Unregistered
    };
    if accepted {
        return reg;
    }
    with_global(&reg, band, dt, centre, global_shift)
}

/// A rejected local solution turned into the cluster-wide fallback, keeping the
/// local diagnostics so a review sheet can still say why the local one lost.
fn with_global(
    local: &Registration,
    band: &[[f64; 2]],
    dt: &OutlineDT,
    centre: [f64; 2],
    global_shift: Option<[f64; 3]>,
) -> Registration {
    let Some(shift) = global_shift else {
        return local.clone();
    };
    Registration {
        pano_id: local.pano_id.clone(),
        result: RegResult {
            dx: shift[0],
            dy: shift[1],
            theta_deg: 0.0,
            inliers_before: local.result.inliers_before,
            inliers_after: if band.is_empty() {
                0.0
            } else {
                inlier_share(band, dt, shift[0], shift[1], 0.0, centre)
            },
            ambiguity_ratio: local.result.ambiguity_ratio,
            radius_agreement_m: local.result.radius_agreement_m,
            n_points: local.result.n_points,
            accepted: true,
            source: RegSource::Global,
            scale_suspect: false,
        },
        second_best: local.second_best,
    }
}

/// Why a local solution was rejected, for the review sheet footer.
pub fn rejection_reason(reg: &RegResult, params: &Params) -> String {
    if reg.source == RegSource::NoCluster {
        return "no cluster".into();
    }
    if reg.n_points < REG_MIN_POINTS {
        return format!("only {} band points", reg.n_points);
    }
    let mut parts: Vec<String> = Vec::new();
    if reg.inliers_after < params.reg_min_inliers {
        parts.push(format!("inliers {:.0} %", 100.0 * reg.inliers_after));
    }
    if reg.radius_agreement_m > params.reg_fine_m {
        parts.push(format!("25/40 m disagree {:.1} m", reg.radius_agreement_m));
    }
    if reg.ambiguity_ratio < params.reg_uniqueness {
        parts.push(format!("ambiguous {:.2}", reg.ambiguity_ratio));
    }
    let shift = reg.dx.hypot(reg.dy);
    if shift > MAX_SHIFT_M {
        parts.push(format!("shift {shift:.1} m"));
    }
    if parts.is_empty() {
        "accepted".into()
    } else {
        parts.join(", ")
    }
}

// --------------------------------------------------------------------------- global fallback

/// The cluster-wide fallback shift: the whole cluster's band points inside the
/// bbox plus 50 m, swept over 8 m at 0.25 m for the largest inlier share.
///
/// Accepted at a share of 0.08 that is also twice the share the raw pose already
/// had, or when the best shift is within half a metre of no shift at all.
/// `z_g` defaults to the median shot height less the rig prior.
pub fn register_cluster_global(
    cluster: &Cluster,
    buildings: &[Building],
    bbox_xy: [f64; 4],
    params: &Params,
    z_g: Option<f64>,
    dt: Option<&OutlineDT>,
) -> Option<[f64; 3]> {
    if cluster.is_empty() {
        return None;
    }
    let z_g = z_g.unwrap_or_else(|| {
        if cluster.shots.is_empty() {
            let mut z: Vec<f64> = cluster.points.iter().map(|p| p[2]).collect();
            super::imgops::percentile_in_place(&mut z, 5.0)
        } else {
            let mut z: Vec<f64> = cluster.shots.iter().map(|s| s.centre[2]).collect();
            median_in_place(&mut z) - params.rig_height_default_m
        }
    });
    let [xmin, ymin, xmax, ymax] = bbox_xy;
    let pts: Vec<[f64; 3]> = cluster
        .points
        .iter()
        .filter(|p| {
            p[0] >= xmin - GLOBAL_MARGIN_M
                && p[0] <= xmax + GLOBAL_MARGIN_M
                && p[1] >= ymin - GLOBAL_MARGIN_M
                && p[1] <= ymax + GLOBAL_MARGIN_M
                && p[2] >= z_g + params.reg_band[0]
                && p[2] <= z_g + params.reg_band[1]
        })
        .copied()
        .collect();
    if pts.len() < REG_MIN_POINTS {
        return None;
    }
    // Every point, not a sample of `GLOBAL_MAX_POINTS`: see the module header.
    let xy: Vec<[f64; 2]> = pts.iter().map(|p| [p[0], p[1]]).collect();
    let built;
    let dt = match dt {
        Some(dt) => dt,
        None => {
            built = covering_dt(
                buildings,
                &[[xmin, ymin], [xmax, ymax]],
                GLOBAL_MARGIN_M + GLOBAL_RANGE_M + 5.0,
                0.5,
            );
            &built
        }
    };
    global_shift_over(&xy, dt)
}

/// The 8 m sweep itself, split out so a test can drive it from stored points.
pub fn global_shift_over(xy: &[[f64; 2]], dt: &OutlineDT) -> Option<[f64; 3]> {
    let (offs, _) = grid(GLOBAL_RANGE_M, GLOBAL_STEP_M);
    let frac: Vec<f64> = offs
        .iter()
        .map(|off| {
            xy.iter()
                .filter(|p| f64::from(dt.distance([p[0] + off[0], p[1] + off[1]])) < INLIER_M)
                .count() as f64
                / xy.len() as f64
        })
        .collect();
    let k = argmax(&frac);
    let best = frac[k];
    let zero = xy
        .iter()
        .filter(|p| f64::from(dt.distance(**p)) < INLIER_M)
        .count() as f64
        / xy.len() as f64;
    let (dx, dy) = (offs[k][0], offs[k][1]);
    if best < GLOBAL_MIN_FRACTION {
        return None;
    }
    if best >= GLOBAL_MIN_RATIO * zero.max(1e-9) || dx.hypot(dy) < 0.5 {
        Some([dx, dy, best])
    } else {
        None
    }
}

// --------------------------------------------------------------------------- apply

/// A copy of the camera with the accepted shift applied: the centre moved by
/// `(dx, dy)` and the axes turned by `theta` about up. `z` is untouched.
pub fn apply_registration(cam: &Camera, reg: &RegResult) -> Camera {
    let mut out = cam.clone();
    if reg.accepted {
        out.centre[0] += reg.dx;
        out.centre[1] += reg.dy;
        if reg.theta_deg.abs() > 1e-12 {
            out.axes = pose::rotate_axes_about_z(&out.axes, reg.theta_deg);
        }
    }
    out.reg = Some(*reg);
    out
}

// --------------------------------------------------------------------------- stage driver

/// The transform the whole run registers against: one grid covering every
/// camera plus the search radius.
pub fn dt_for_cameras(
    buildings: &[Building],
    cameras: &BTreeMap<String, Camera>,
    params: &Params,
) -> OutlineDT {
    let xy: Vec<[f64; 2]> = cameras
        .values()
        .map(|c| [c.centre[0], c.centre[1]])
        .collect();
    let margin = params.reg_radius_m + params.reg_coarse_m + params.reg_fine_m + 5.0;
    covering_dt(
        buildings,
        if xy.is_empty() { &[[0.0, 0.0]] } else { &xy },
        margin,
        0.5,
    )
}

/// Everything the align stage reads.
pub struct AlignInput<'a> {
    pub buildings: &'a [Building],
    pub walls: &'a [Wall],
    /// The raw cameras out of the geometry stage.
    pub cameras: &'a BTreeMap<String, Camera>,
    pub metas: &'a BTreeMap<String, PanoMeta>,
    /// `(xmin, ymin, xmax, ymax)` of the run bbox in the run frame.
    pub bbox_xy: [f64; 4],
    pub params: &'a Params,
}

/// One wall's view list and the candidates behind it.
#[derive(Clone, Debug, Default)]
pub struct WallViews {
    pub wall_key: String,
    pub building_key: String,
    pub reachable: bool,
    pub unreachable_reason: String,
    pub plane_source: PlaneSource,
    pub best_pano: Option<String>,
    pub single_view: bool,
    pub n_candidates: usize,
    pub n_los_clean: usize,
    pub views: Vec<ViewCandidate>,
    pub candidates: Vec<ViewCandidate>,
}

/// How the registration went over the whole run (CRITIQUE C23).
#[derive(Clone, Debug, Default)]
pub struct RegStats {
    pub panos: usize,
    pub with_cluster: usize,
    pub local: usize,
    pub global: usize,
    pub none: usize,
    pub no_cluster: usize,
    pub acceptance_rate_local: f64,
    pub acceptance_rate_any: f64,
    pub median_shift_m: f64,
    pub median_inliers_after: f64,
    pub median_inliers_before: f64,
    pub theta_used: usize,
    pub local_rejection_reasons: BTreeMap<String, usize>,
}

/// What the align stage produced.
pub struct Align {
    /// The cameras with their accepted shift applied.
    pub cameras: BTreeMap<String, Camera>,
    pub regs: BTreeMap<String, Registration>,
    /// The local solution of every pano that fell back, kept for the sheets.
    pub local_attempts: BTreeMap<String, Registration>,
    pub global_shifts: BTreeMap<String, Option<[f64; 3]>>,
    pub depths: BTreeMap<String, DepthMap>,
    /// The wall frame plane per wall, and the fit per view key.
    pub planes: BTreeMap<String, PlaneFit>,
    pub corners: BTreeMap<String, (EndSource, EndSource)>,
    pub per_view: BTreeMap<String, PlaneFit>,
    /// Per view key, how that view's plane sits in the wall frame (CRITIQUE A5).
    pub per_view_offsets: BTreeMap<String, plane::ViewOffset>,
    /// Per view key, the wall foot z and where it came from.
    pub z_bases: BTreeMap<String, (f64, String)>,
    /// Per pano, how many cloud points the ground level was taken from.
    pub ground_points: BTreeMap<String, usize>,
    pub views: BTreeMap<String, WallViews>,
    pub stats: RegStats,
    pub walls_with_views: usize,
    pub reachable: usize,
    pub candidates_no_image: usize,
}

/// The reasons that mean a candidate got as far as the image gates.
fn reached_image_gates(reason: Option<&str>) -> bool {
    match reason {
        None => true,
        Some(r) => {
            r.starts_with("blur")
                || r.starts_with("night")
                || r.starts_with("dark")
                || r.starts_with("exposure")
                || r.starts_with("quality")
                || r.starts_with("occluded")
                || r.starts_with("no_image")
        }
    }
}

/// The align stage.
///
/// `cluster_of` hands over a pano's cluster and `image_of` its thumbnail; both
/// are closures so the driver does not care whether they come from the cache,
/// the network or a fixture. A pano whose image cannot be loaded has its
/// candidates rejected as `no_image` rather than dropped, so the wall can still
/// say why it has no views.
///
/// `request_images` is called exactly once, with every pano the image gates are
/// about to read, and it is the only place a run learns that list. It cannot be
/// known any earlier: the candidates are drawn from the **registered** centres,
/// and a registration moves a camera by up to 11.8 m on the Munich box, so the
/// same gates run against the raw Graph poses answer a different question. A
/// driver that downloaded from that earlier answer left 44 of the reference
/// run's 111 walls with a different view list and 6 of them with no view at
/// all, rejecting as `no_image` pixels that were sitting in the cache unasked
/// for. Registration itself reads only the point clouds, so nothing is delayed
/// by waiting for it.
pub fn run_align(
    input: &AlignInput,
    cluster_of: &(dyn Fn(&str) -> Option<Cluster> + Sync),
    request_images: &(dyn Fn(&BTreeSet<String>) + Sync),
    image_of: &(dyn Fn(&str) -> Option<RgbImage> + Sync),
) -> Align {
    let params = input.params;
    let fps = Footprints::new(input.buildings);
    let dt = dt_for_cameras(input.buildings, input.cameras, params);
    // Every cluster the run touches, loaded once. They are a few megabytes
    // each and every stage after this one wants them again, which is why the
    // Python caches them per process too.
    let mut clusters: HashMap<String, Option<Cluster>> = HashMap::new();
    for cam in input.cameras.values() {
        if let Some(id) = &cam.cluster_id {
            if !clusters.contains_key(id) {
                clusters.insert(id.clone(), cluster_of(id));
            }
        }
    }
    let cluster_for = |cam: &Camera| -> Option<&Cluster> {
        cam.cluster_id
            .as_ref()
            .and_then(|id| clusters.get(id))
            .and_then(|c| c.as_ref())
    };

    // ---- per pano: the local registration, then the cluster-wide fallback
    //
    // A pano's local solution is a grid search over its own cloud against the
    // shared distance transform and depends on nothing else, so the whole
    // stage is a parallel map collected back into pano order.
    let mut regs: BTreeMap<String, Registration> = input
        .cameras
        .par_iter()
        .map(|(pid, cam)| {
            (
                pid.clone(),
                register_pano(cam, cluster_for(cam), &dt, params, None, ThetaMode::Auto),
            )
        })
        .collect();
    // Every cluster that will need the fallback, solved once and in parallel.
    // The Python memoises the same thing lazily inside the per pano loop; doing
    // it up front is what lets that loop run in parallel too.
    let need_global: BTreeSet<String> = input
        .cameras
        .iter()
        .filter(|(pid, cam)| {
            !regs[*pid].result.accepted
                && cam
                    .cluster_id
                    .as_ref()
                    .is_some_and(|id| clusters.get(id).and_then(|c| c.as_ref()).is_some())
        })
        .filter_map(|(_, cam)| cam.cluster_id.clone())
        .collect();
    let global_shifts: BTreeMap<String, Option<[f64; 3]>> = need_global
        .par_iter()
        .map(|cid| {
            let cl = clusters[cid].as_ref().expect("only cached clusters listed");
            let mut zg: Vec<f64> = input
                .cameras
                .values()
                .filter(|c| c.cluster_id.as_ref() == Some(cid) && c.ground_z.is_finite())
                .map(|c| c.ground_z)
                .collect();
            let z_g = if zg.is_empty() {
                None
            } else {
                Some(median_in_place(&mut zg))
            };
            (
                cid.clone(),
                register_cluster_global(cl, input.buildings, input.bbox_xy, params, z_g, None),
            )
        })
        .collect();
    let mut local_attempts: BTreeMap<String, Registration> = BTreeMap::new();
    let mut cams_reg: BTreeMap<String, Camera> = BTreeMap::new();
    let mut depths: BTreeMap<String, DepthMap> = BTreeMap::new();
    let mut ground_points: BTreeMap<String, usize> = BTreeMap::new();
    // The fallback, the registered camera and its depth map, per pano. Each
    // one reads its own cluster and the shared distance transform and writes
    // nothing, so the whole thing is a parallel map; the depth map alone is a
    // 720 by 360 pass over the cloud for every camera of the run.
    type PanoOutcome = (
        String,
        Option<Registration>,
        Option<Registration>,
        Camera,
        Option<(usize, DepthMap)>,
    );
    let outcomes: Vec<PanoOutcome> = input
        .cameras
        .par_iter()
        .map(|(pid, cam)| {
            let cluster = cluster_for(cam);
            let mut reg = regs[pid].clone();
            let mut attempt = None;
            let mut replaced = None;
            if !reg.result.accepted {
                if let (Some(cl), Some(cid)) = (cluster, cam.cluster_id.as_ref()) {
                    attempt = Some(reg.clone());
                    let z_g = cam.ground_z;
                    let band_all =
                        facade_band(cl, cam.centre, z_g, params.reg_radius_m, params.reg_band);
                    // Every band point here too: this is the inlier share the
                    // fallback's own record carries.
                    let band: Vec<[f64; 2]> = band_all.iter().map(|p| [p[0], p[1]]).collect();
                    reg = with_global(
                        &reg,
                        &band,
                        &dt,
                        [cam.centre[0], cam.centre[1]],
                        global_shifts.get(cid).copied().flatten(),
                    );
                    if !reg.result.accepted {
                        // 'none': the raw pose is used and the rejected
                        // solution lives on only in the local attempt.
                        reg = Registration {
                            pano_id: pid.clone(),
                            result: RegResult {
                                inliers_before: reg.result.inliers_before,
                                inliers_after: reg.result.inliers_after,
                                ambiguity_ratio: reg.result.ambiguity_ratio,
                                radius_agreement_m: reg.result.radius_agreement_m,
                                n_points: reg.result.n_points,
                                accepted: false,
                                source: RegSource::Unregistered,
                                ..RegResult::default()
                            },
                            second_best: reg.second_best,
                        };
                    }
                    replaced = Some(reg.clone());
                }
            }
            let cam_reg = apply_registration(cam, &reg.result);
            let cloud = cluster.map(|cl| {
                (
                    sfm::ground_level(cl, cam.centre, params).1,
                    sfm::depth_map(cl, &cam_reg, None, params),
                )
            });
            (pid.clone(), attempt, replaced, cam_reg, cloud)
        })
        .collect();
    for (pid, attempt, replaced, cam_reg, cloud) in outcomes {
        if let Some(a) = attempt {
            local_attempts.insert(pid.clone(), a);
        }
        if let Some(r) = replaced {
            regs.insert(pid.clone(), r);
        }
        if let Some((points, depth)) = cloud {
            ground_points.insert(pid.clone(), points);
            depths.insert(pid.clone(), depth);
        }
        cams_reg.insert(pid, cam_reg);
    }
    let stats = reg_stats(&regs, &local_attempts, params);

    // ---- candidates and LOS from the registered centres (CRITIQUE C22)
    let mut walls: Vec<Wall> = input.walls.to_vec();
    let cands_by_wall = visibility::mark_reachable(&mut walls, &cams_reg, &fps, params, true);
    // The photographs the gates below will read, handed over at the first
    // moment the run knows them. The set is exactly `by_pano`'s keys: nothing
    // between here and there touches a candidate's rejection.
    let wanted: BTreeSet<String> = cands_by_wall
        .values()
        .flatten()
        .filter(|c| c.rejected_reason.is_none())
        .map(|c| c.pano_id.clone())
        .collect();
    request_images(&wanted);
    let mut walls_by_building: BTreeMap<String, Vec<Wall>> = BTreeMap::new();
    for w in &walls {
        walls_by_building
            .entry(w.building_key.clone())
            .or_default()
            .push(w.clone());
    }

    // ---- per (wall, view): points, foot z and a plane fit (CRITIQUE A3, A5)
    //
    // A cloud plane per accepted candidate, from the wall's own geometry and the
    // points near it and nothing else, so the loop is parallel over walls and the
    // answers do not depend on the order anything came back in.
    type WallFits = Vec<(String, PlaneFit, (f64, String))>;
    let fitted: Vec<WallFits> = walls
        .par_iter()
        .map(|w| {
            let mut out: WallFits = Vec::new();
            for c in &cands_by_wall[&w.key] {
                if c.rejected_reason.is_some() {
                    continue;
                }
                let cam = &cams_reg[&c.pano_id];
                let key = view_key(&w.key, &c.pano_id);
                let Some(cl) = cluster_for(cam) else {
                    out.push((
                        key,
                        plane::fallback_plane(w, false, Some(&c.pano_id), None),
                        (cam.ground_z, "pano".into()),
                    ));
                    continue;
                };
                let accepted = cam.reg.map(|r| r.accepted).unwrap_or(false);
                let shift = if accepted {
                    cam.reg.map(|r| r.shift()).unwrap_or([0.0; 3])
                } else {
                    [0.0; 3]
                };
                let pts = sfm::wall_points(
                    cl,
                    w,
                    shift,
                    cam.ground_z,
                    [cam.centre[0], cam.centre[1]],
                    params,
                );
                let fit = plane::fit_wall_plane(
                    w,
                    &pts,
                    params,
                    Some(&c.pano_id),
                    cam.cluster_id.as_deref(),
                    accepted,
                );
                let (z, src) = sfm::wall_foot_z(
                    cl,
                    w,
                    shift,
                    cam.ground_z,
                    [cam.centre[0], cam.centre[1]],
                    params,
                );
                out.push((key, fit, (z, src.to_string())));
            }
            out
        })
        .collect();
    let mut per_view: BTreeMap<String, PlaneFit> = BTreeMap::new();
    let mut z_bases: BTreeMap<String, (f64, String)> = BTreeMap::new();
    for wall_fits in fitted {
        for (key, fit, z) in wall_fits {
            per_view.insert(key.clone(), fit);
            z_bases.insert(key, z);
        }
    }

    // ---- previews and image gates, grouped by pano so each image decodes once
    let mut cands_by_wall = cands_by_wall;
    let mut by_pano: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    let wall_index: BTreeMap<&str, usize> = walls
        .iter()
        .enumerate()
        .map(|(i, w)| (w.key.as_str(), i))
        .collect();
    for w in &walls {
        for (ci, c) in cands_by_wall[&w.key].iter().enumerate() {
            if c.rejected_reason.is_none() {
                by_pano
                    .entry(c.pano_id.clone())
                    .or_default()
                    .push((wall_index[w.key.as_str()], ci));
            }
        }
    }
    // One photograph decodes once and is measured against every wall it can
    // see, so the parallelism is per pano rather than per candidate; the
    // verdicts are applied afterwards in pano order.
    type PanoGates = (bool, Vec<((usize, usize), Vec<super::types::Gate>)>);
    let judged: Vec<PanoGates> = by_pano
        .par_iter()
        .map(|(pid, pairs)| {
            let cam = &cams_reg[pid];
            let Some(image) = image_of(pid) else {
                return (false, Vec::new());
            };
            let gates = pairs
                .iter()
                .map(|(wi, ci)| {
                    let w = &walls[*wi];
                    let key = view_key(&w.key, pid);
                    let fit = &per_view[&key];
                    let zb = z_bases[&key].0;
                    let fw = plane::fitted_wall(w, fit);
                    let s_vis = cands_by_wall[&w.key][*ci].s_vis;
                    let preview = visibility::preview_crop(
                        &fw,
                        cam,
                        zb,
                        &image,
                        params,
                        &PreviewOpts {
                            depth: depths.get(pid),
                            s_vis: Some(s_vis),
                            ..PreviewOpts::default()
                        },
                    );
                    let stub;
                    let meta = match input.metas.get(pid) {
                        Some(m) => m,
                        None => {
                            stub = meta_stub(pid);
                            &stub
                        }
                    };
                    (
                        (*wi, *ci),
                        visibility::image_gates(&preview.rgb, &preview.occl, meta, None, None),
                    )
                })
                .collect();
            (true, gates)
        })
        .collect();
    let mut candidates_no_image = 0usize;
    for ((pid, pairs), (had_image, gates)) in by_pano.iter().zip(judged) {
        let _ = pid;
        if !had_image {
            for (wi, ci) in pairs {
                let key = walls[*wi].key.clone();
                cands_by_wall.get_mut(&key).unwrap()[*ci].rejected_reason = Some("no_image".into());
                candidates_no_image += 1;
            }
            continue;
        }
        for ((wi, ci), gates) in gates {
            let key = walls[wi].key.clone();
            visibility::apply_image_gates(&mut cands_by_wall.get_mut(&key).unwrap()[ci], &gates);
        }
    }

    // ---- per wall: the view list, the wall frame plane and the corners
    let mut planes: BTreeMap<String, PlaneFit> = BTreeMap::new();
    let mut views: BTreeMap<String, WallViews> = BTreeMap::new();
    let mut selected: BTreeMap<String, Vec<ViewCandidate>> = BTreeMap::new();
    for w in &walls {
        let cands = cands_by_wall.get_mut(&w.key).unwrap();
        let sel = visibility::select_views(cands, &cams_reg, params, 3);
        let clean: Vec<&ViewCandidate> = cands
            .iter()
            .filter(|c| reached_image_gates(c.rejected_reason.as_deref()))
            .collect();
        let best = if let Some(first) = sel.first() {
            Some(first.pano_id.clone())
        } else {
            // LOS clean but failed the image gates: the best of those still
            // gives the wall a plane to stand on.
            clean
                .iter()
                .max_by(|a, b| {
                    a.g_score
                        .total_cmp(&b.g_score)
                        .then_with(|| a.pano_id.cmp(&b.pano_id))
                })
                .map(|c| c.pano_id.clone())
        };
        let fit = match &best {
            Some(pid) => per_view[&view_key(&w.key, pid)].clone(),
            None => plane::fallback_plane(w, false, None, None),
        };
        planes.insert(w.key.clone(), fit);
        let n_clean = clean.len();
        views.insert(
            w.key.clone(),
            WallViews {
                wall_key: w.key.clone(),
                building_key: w.building_key.clone(),
                reachable: w.reachable,
                unreachable_reason: w.unreachable_reason.clone(),
                plane_source: PlaneSource::default(),
                best_pano: best,
                single_view: false,
                n_candidates: cands.len(),
                n_los_clean: n_clean,
                views: Vec::new(),
                candidates: Vec::new(),
            },
        );
        selected.insert(w.key.clone(), sel);
    }
    let mut corners: BTreeMap<String, (EndSource, EndSource)> = BTreeMap::new();
    let mut per_view_offsets: BTreeMap<String, plane::ViewOffset> = BTreeMap::new();
    for w in &walls {
        let fit = planes[&w.key].clone();
        let siblings = walls_by_building
            .get(&w.building_key)
            .cloned()
            .unwrap_or_default();
        let found = plane::refine_corners(w, &fit, &siblings, |adj| {
            // The adjacent wall's fit from this frame's own view first, then
            // its frame fit (CRITIQUE A5); `refine_corners` itself is what
            // insists on a cloud fit from the same registration.
            if let Some(pid) = fit.pano_id.as_ref() {
                if let Some(c) = per_view.get(&view_key(&adj.key, pid)) {
                    if c.source == PlaneSource::Cloud {
                        return Some(c.clone());
                    }
                }
            }
            planes.get(&adj.key).cloned()
        });
        let slot = planes.get_mut(&w.key).unwrap();
        slot.a_ref = Some(found.a_ref);
        slot.b_ref = Some(found.b_ref);
        corners.insert(w.key.clone(), (found.src_a, found.src_b));
    }
    for w in &walls {
        let frame_fit = &planes[&w.key];
        for c in cands_by_wall.get(&w.key).into_iter().flatten() {
            let key = view_key(&w.key, &c.pano_id);
            if let Some(f) = per_view.get(&key) {
                per_view_offsets.insert(key, plane::per_view_offset(frame_fit, f, w));
            }
        }
    }
    let mut walls_with_views = 0usize;
    for w in &walls {
        let sel = selected.remove(&w.key).unwrap_or_default();
        walls_with_views += usize::from(!sel.is_empty());
        let entry = views.get_mut(&w.key).unwrap();
        entry.plane_source = planes[&w.key].source;
        entry.single_view = !sel.is_empty()
            && !sel[0]
                .gate(super::types::GateName::Positions)
                .map(|g| g.passed)
                .unwrap_or(true);
        entry.views = sel;
        entry.candidates = cands_by_wall.remove(&w.key).unwrap_or_default();
    }
    let reachable = walls.iter().filter(|w| w.reachable).count();
    // A depth map is a megabyte a camera and the run made one for every camera
    // it registered, but past this point only the views a wall actually took
    // are ever asked for one again. On the Munich box that is 1.1 GB handed
    // back as about 60 MB.
    let selected_panos: std::collections::BTreeSet<&str> = views
        .values()
        .flat_map(|v| v.views.iter().map(|c| c.pano_id.as_str()))
        .collect();
    depths.retain(|pid, _| selected_panos.contains(pid.as_str()));
    Align {
        cameras: cams_reg,
        regs,
        local_attempts,
        global_shifts,
        depths,
        planes,
        corners,
        per_view,
        per_view_offsets,
        z_bases,
        ground_points,
        views,
        stats,
        walls_with_views,
        reachable,
        candidates_no_image,
    }
}

fn meta_stub(pano_id: &str) -> PanoMeta {
    use super::types::{CameraModel, GeometrySource};
    PanoMeta {
        id: pano_id.to_string(),
        lon: 0.0,
        lat: 0.0,
        alt: 0.0,
        compass: 0.0,
        rotation: None,
        atomic_scale: None,
        captured_at: 0,
        sequence: String::new(),
        quality: 1.0,
        width: 0,
        height: 0,
        cluster_id: None,
        geometry_source: GeometrySource::Computed,
        camera_type: CameraModel::Spherical,
        camera_params: Vec::new(),
    }
}

fn reg_stats(
    regs: &BTreeMap<String, Registration>,
    local_attempts: &BTreeMap<String, Registration>,
    params: &Params,
) -> RegStats {
    let count = |src: RegSource| regs.values().filter(|r| r.result.source == src).count();
    let with_cluster = regs
        .values()
        .filter(|r| r.result.source != RegSource::NoCluster)
        .count();
    let mut stats = RegStats {
        panos: regs.len(),
        with_cluster,
        local: count(RegSource::Local),
        global: count(RegSource::Global),
        none: count(RegSource::Unregistered),
        no_cluster: count(RegSource::NoCluster),
        ..RegStats::default()
    };
    if with_cluster > 0 {
        stats.acceptance_rate_local = stats.local as f64 / with_cluster as f64;
        stats.acceptance_rate_any = (stats.local + stats.global) as f64 / with_cluster as f64;
    }
    let accepted: Vec<&Registration> = regs.values().filter(|r| r.result.accepted).collect();
    if !accepted.is_empty() {
        let mut shifts: Vec<f64> = accepted
            .iter()
            .map(|r| r.result.dx.hypot(r.result.dy))
            .collect();
        stats.median_shift_m = median_in_place(&mut shifts);
        let mut inl: Vec<f64> = accepted.iter().map(|r| r.result.inliers_after).collect();
        stats.median_inliers_after = median_in_place(&mut inl);
    }
    if with_cluster > 0 {
        let mut before: Vec<f64> = regs
            .values()
            .filter(|r| r.result.source != RegSource::NoCluster)
            .map(|r| r.result.inliers_before)
            .collect();
        stats.median_inliers_before = median_in_place(&mut before);
    }
    stats.theta_used = accepted
        .iter()
        .filter(|r| r.result.theta_deg.abs() > 1e-9)
        .count();
    for (pid, r) in regs {
        if matches!(r.result.source, RegSource::Local | RegSource::NoCluster) {
            continue;
        }
        let local = local_attempts.get(pid).unwrap_or(r);
        for part in rejection_reason(&local.result, params).split(", ") {
            let key = part.split(' ').next().unwrap_or(part).to_string();
            *stats.local_rejection_reasons.entry(key).or_insert(0) += 1;
        }
    }
    stats
}

// --------------------------------------------------------------------------- products

/// Writes the align products in the shape `register.run_align` writes them, so
/// the Python review sheets read a Rust run unchanged.
pub fn write_products(dir: &Path, align: &Align, params: &Params) -> Result<(), String> {
    let write = |path: std::path::PathBuf, value: &serde_json::Value| -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, serde_json::to_vec(value).map_err(|e| e.to_string())?)
            .map_err(|e| format!("{}: {e}", path.display()))
    };
    for (pid, reg) in &align.regs {
        let mut rec = reg_record(reg, params);
        if let Some(local) = align.local_attempts.get(pid) {
            let attempt = reg_record(local, params);
            rec["reason"] = if reg.result.source == RegSource::Unregistered {
                attempt["reason"].clone()
            } else {
                serde_json::Value::from("accepted (global fallback)")
            };
            rec["local_attempt"] = attempt;
        }
        write(dir.join("reg").join(format!("{pid}.json")), &rec)?;
        let cam = &align.cameras[pid];
        write(
            dir.join("ground").join(format!("{pid}.json")),
            &serde_json::json!({
                "pano_id": pid,
                "ground_z": cam.ground_z,
                "cam_height_m": cam.cam_height_m,
                "ground_source": cam.ground_source.as_str(),
                "n_points": align.ground_points.get(pid).copied().unwrap_or(0),
                "cluster_id": cam.cluster_id,
            }),
        )?;
    }
    for (wkey, fit) in &align.planes {
        let mut rec = plane_record(fit, align.corners.get(wkey));
        let mut per_view = Vec::new();
        if let Some(w) = align.views.get(wkey) {
            for c in &w.candidates {
                let key = view_key(wkey, &c.pano_id);
                let Some(f) = align.per_view.get(&key) else {
                    continue;
                };
                let (zb, zsrc) = align.z_bases[&key].clone();
                let mut v = plane_record(f, None);
                v["pano_id"] = serde_json::Value::from(c.pano_id.clone());
                v["z_base"] = serde_json::Value::from(zb);
                v["z_base_source"] = serde_json::Value::from(zsrc);
                v["h_cloud"] = match plane::cloud_height(f, zb) {
                    Some(h) => serde_json::Value::from(h),
                    None => serde_json::Value::Null,
                };
                v["selected"] =
                    serde_json::Value::from(w.views.iter().any(|s| s.pano_id == c.pano_id));
                if let Some(off) = align.per_view_offsets.get(&key) {
                    v["dn_m"] = serde_json::Value::from(off.dn_m);
                    v["ds_m"] = serde_json::Value::from(off.ds_m);
                    v["dtheta_deg"] = serde_json::Value::from(off.dtheta_deg);
                }
                per_view.push(v);
            }
        }
        rec["per_view"] = serde_json::Value::Array(per_view);
        rec["frame_pano"] = match &fit.pano_id {
            Some(p) => serde_json::Value::from(p.clone()),
            None => serde_json::Value::Null,
        };
        write(dir.join("planes").join(format!("{wkey}.json")), &rec)?;
    }
    let mut views = serde_json::Map::new();
    for (wkey, w) in &align.views {
        views.insert(wkey.clone(), wall_views_record(w, align));
    }
    write(dir.join("views.json"), &serde_json::Value::Object(views))?;
    let mut cameras = serde_json::Map::new();
    for (pid, cam) in &align.cameras {
        cameras.insert(pid.clone(), camera_record(cam));
    }
    write(
        dir.join("cameras.json"),
        &serde_json::Value::Object(cameras),
    )?;
    write(dir.join("summary.json"), &summary_record(align))?;
    Ok(())
}

fn reg_record(reg: &Registration, params: &Params) -> serde_json::Value {
    let r = &reg.result;
    serde_json::json!({
        "pano_id": reg.pano_id,
        "dx": r.dx, "dy": r.dy, "theta_deg": r.theta_deg,
        "inliers_before": r.inliers_before, "inliers_after": r.inliers_after,
        "ambiguity_ratio": r.ambiguity_ratio, "radius_agreement_m": r.radius_agreement_m,
        "n_points": r.n_points, "accepted": r.accepted, "source": r.source.as_str(),
        "scale_suspect": r.scale_suspect,
        "second_best": reg.second_best.map(|s| vec![s[0], s[1], s[2]]),
        "shift_m": r.dx.hypot(r.dy),
        "reason": if r.accepted { "accepted".to_string() } else { rejection_reason(r, params) },
    })
}

fn plane_record(fit: &PlaneFit, ends: Option<&(EndSource, EndSource)>) -> serde_json::Value {
    serde_json::json!({
        "wall_key": fit.wall_key,
        "n": [fit.n[0], fit.n[1]], "d": fit.d, "source": fit.source.as_str(),
        "n_inliers": fit.n_inliers, "pts_per_m": fit.pts_per_m, "rms_m": fit.rms_m,
        "angle_vs_osm_deg": fit.angle_vs_osm_deg, "offset_vs_osm_m": fit.offset_vs_osm_m,
        "z_top98": fit.z_top98, "z_continuous": fit.z_continuous, "ambiguity": fit.ambiguity,
        "a_ref": fit.a_ref.map(|p| vec![p[0], p[1]]),
        "b_ref": fit.b_ref.map(|p| vec![p[0], p[1]]),
        "src_a": ends.map(|e| e.0.as_str()).unwrap_or("osm"),
        "src_b": ends.map(|e| e.1.as_str()).unwrap_or("osm"),
        "pano_id": fit.pano_id, "cluster_id": fit.cluster_id,
    })
}

fn candidate_record(c: &ViewCandidate) -> serde_json::Value {
    let mut gates = serde_json::Map::new();
    for g in &c.gates {
        gates.insert(
            g.name.as_str().to_string(),
            serde_json::json!([g.passed, g.value]),
        );
    }
    serde_json::json!({
        "wall_key": c.wall_key, "pano_id": c.pano_id, "dist_m": c.dist_m,
        "incidence_deg": c.incidence_deg, "angwidth_deg": c.angwidth_deg,
        "visible_frac": c.visible_frac, "s_vis": [c.s_vis[0], c.s_vis[1]],
        "g_score": c.g_score, "gates": serde_json::Value::Object(gates),
        "f_occ": c.f_occ, "blur": c.blur, "score": c.score,
        "rejected_reason": c.rejected_reason,
    })
}

fn wall_views_record(w: &WallViews, align: &Align) -> serde_json::Value {
    let views: Vec<serde_json::Value> = w
        .views
        .iter()
        .map(|c| {
            let key = view_key(&w.wall_key, &c.pano_id);
            let cam = &align.cameras[&c.pano_id];
            let (zb, zsrc) = align.z_bases[&key].clone();
            let source = align.per_view[&key].source;
            serde_json::json!({
                "pano_id": c.pano_id, "score": c.score, "g_score": c.g_score,
                "dist_m": c.dist_m, "incidence_deg": c.incidence_deg,
                "angwidth_deg": c.angwidth_deg, "visible_frac": c.visible_frac,
                "s_vis": [c.s_vis[0], c.s_vis[1]], "f_occ": c.f_occ, "blur": c.blur,
                "z_base": zb, "z_base_source": zsrc, "plane_source": source.as_str(),
                "cluster_id": cam.cluster_id, "cam_height_m": cam.cam_height_m,
                "reg": cam.reg.map(|r| serde_json::json!({
                    "dx": r.dx, "dy": r.dy, "theta_deg": r.theta_deg,
                    "inliers": r.inliers_after, "source": r.source.as_str(),
                })),
            })
        })
        .collect();
    serde_json::json!({
        "best_pano": w.best_pano, "wall_key": w.wall_key, "building_key": w.building_key,
        "reachable": w.reachable, "unreachable_reason": w.unreachable_reason,
        "plane_source": w.plane_source.as_str(), "single_view": w.single_view,
        "n_candidates": w.n_candidates, "n_los_clean": w.n_los_clean,
        "views": views,
        "candidates": w.candidates.iter().map(candidate_record).collect::<Vec<_>>(),
    })
}

fn camera_record(cam: &Camera) -> serde_json::Value {
    serde_json::json!({
        "pano_id": cam.pano_id,
        "C": [cam.centre[0], cam.centre[1], cam.centre[2]],
        "axes": [cam.axes[0], cam.axes[1], cam.axes[2]],
        "pose_source": cam.pose_source.as_str(), "pose_factor": cam.pose_factor,
        "roll_deg": cam.roll_deg, "pitch_deg": cam.pitch_deg, "compass_deg": cam.compass_deg,
        "ground_z": cam.ground_z, "cam_height_m": cam.cam_height_m,
        "ground_source": cam.ground_source.as_str(),
        "cluster_id": cam.cluster_id, "shot_id": cam.shot_id,
        "camera_type": cam.camera_type.as_str(),
        "camera_params": cam.camera_params, "width": cam.width, "height": cam.height,
        "reg": cam.reg.map(|r| serde_json::json!({
            "pano_id": cam.pano_id, "dx": r.dx, "dy": r.dy, "theta_deg": r.theta_deg,
            "inliers_before": r.inliers_before, "inliers_after": r.inliers_after,
            "ambiguity_ratio": r.ambiguity_ratio, "radius_agreement_m": r.radius_agreement_m,
            "n_points": r.n_points, "accepted": r.accepted, "source": r.source.as_str(),
            "scale_suspect": r.scale_suspect,
        })),
    })
}

fn summary_record(align: &Align) -> serde_json::Value {
    let s = &align.stats;
    let plane_sources: serde_json::Map<String, serde_json::Value> = [
        PlaneSource::Cloud,
        PlaneSource::OsmRegistered,
        PlaneSource::OsmRaw,
    ]
    .iter()
    .map(|src| {
        (
            src.as_str().to_string(),
            serde_json::Value::from(align.planes.values().filter(|f| f.source == *src).count()),
        )
    })
    .collect();
    serde_json::json!({
        "registration": {
            "panos": s.panos, "with_cluster": s.with_cluster, "local": s.local,
            "global": s.global, "none": s.none, "no_cluster": s.no_cluster,
            "acceptance_rate_local": s.acceptance_rate_local,
            "acceptance_rate_any": s.acceptance_rate_any,
            "median_shift_m": s.median_shift_m,
            "median_inliers_after": s.median_inliers_after,
            "median_inliers_before": s.median_inliers_before,
            "theta_used": s.theta_used,
            "local_rejection_reasons": s.local_rejection_reasons,
        },
        "walls": align.views.len(),
        "reachable": align.reachable,
        "walls_with_views": align.walls_with_views,
        "single_view": align.views.values().filter(|v| v.single_view).count(),
        "planes_cloud": align.planes.values().filter(|f| f.source == PlaneSource::Cloud).count(),
        "candidates_no_image": align.candidates_no_image,
        "plane_sources": plane_sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapillary::golden;
    use crate::mapillary::types::{HeightSource, OsmKind};

    fn square(key: &str, x0: f64, y0: f64, w: f64, h: f64) -> Building {
        Building {
            key: key.into(),
            osm_id: 1,
            kind: OsmKind::Way,
            ring: vec![[x0, y0], [x0 + w, y0], [x0 + w, y0 + h], [x0, y0 + h]],
            holes: vec![],
            node_ids: vec![1, 2, 3, 4],
            tags: Default::default(),
            height_osm: Some(12.0),
            height_source: HeightSource::Tag,
            min_height: 0.0,
            target: true,
            member_ways: vec![],
        }
    }

    #[test]
    fn the_transform_measures_the_distance_to_an_outline() {
        let dt = OutlineDT::new(
            &[square("w1", -10.0, -10.0, 20.0, 20.0)],
            [0.0, 0.0],
            60.0,
            0.5,
        );
        // On the outline itself, and half a metre out from it.
        assert!(f64::from(dt.distance([-10.0, 0.0])) < 0.3);
        assert!((f64::from(dt.distance([-12.0, 0.0])) - 2.0).abs() < 0.3);
        // The centre of a 20 m square is 10 m from every wall.
        assert!((f64::from(dt.distance([0.0, 0.0])) - 10.0).abs() < 0.3);
        // Outside the grid the answer is the far value, not an extrapolation.
        assert!(f64::from(dt.distance([1000.0, 0.0])) >= DT_FAR as f32 as f64);
        assert!(!dt.contains([1000.0, 0.0]));
    }

    #[test]
    fn the_search_finds_a_shift_it_was_given() {
        let buildings = [square("w1", -10.0, -10.0, 20.0, 20.0)];
        let dt = OutlineDT::new(&buildings, [0.0, 0.0], 80.0, 0.5);
        let params = Params::default();
        // Points on the square's outline, moved 2.5 m east and 1.25 m north.
        let mut pts = Vec::new();
        for i in 0..80 {
            let t = -10.0 + 20.0 * i as f64 / 79.0;
            pts.push([t + 2.5, -10.0 + 1.25]);
            pts.push([t + 2.5, 10.0 + 1.25]);
            pts.push([-10.0 + 2.5, t + 1.25]);
            pts.push([10.0 + 2.5, t + 1.25]);
        }
        let (dx, dy, cost, ranked) = search_shift(&pts, &dt, [0.0, 0.0], &params, 0.0);
        assert!((dx + 2.5).abs() < 0.26, "dx {dx}");
        assert!((dy + 1.25).abs() < 0.26, "dy {dy}");
        assert!(cost < 0.05, "cost {cost}");
        assert_eq!(ranked[0][2], cost, "the best minimum leads the ranking");
        let inliers = inlier_share(&pts, &dt, dx, dy, 0.0, [0.0, 0.0]);
        assert!(inliers > 0.99, "inliers {inliers}");
    }

    /// The search sees every band point, and a cap would be measurable.
    ///
    /// This is the seam that broke the port. `register_band` has a golden test,
    /// but it is handed a band, so nothing above it said which points the band
    /// holds; `register_pano` used to hand it a seeded random 4000 of them, and
    /// the acceptance gates are thresholds on statistics of whatever was handed
    /// over. The cloud below is built so the two answers cannot be confused: it
    /// is 5000 points of which the 1000 at index 4 mod 5 stand 5 m off the
    /// outline, and `subsample`'s stride at 4000 of 5000 steps 1.25 indices at a
    /// time and so lands on none of them. A sample says every point is an
    /// inlier; the band says four in five are.
    #[test]
    fn the_registration_searches_every_band_point() {
        let buildings = [square("w1", -20.0, -20.0, 40.0, 40.0)];
        let dt = OutlineDT::new(&buildings, [0.0, 0.0], 120.0, 0.5);
        let params = Params::default();
        let mut points = Vec::new();
        for i in 0..5000 {
            let t = -20.0 + 40.0 * (i / 5) as f64 / 999.0;
            // On the south edge, except every fifth point, which is 5 m out.
            let y = if i % 5 == 4 { -25.0 } else { -20.0 };
            points.push([t, y, 10.0]);
        }
        let n = points.len();
        let raw = crate::mapillary::fetch::RawCluster {
            cluster_id: "c".into(),
            ref_lla: (11.5795305, 48.13643, 0.0),
            shots: Vec::new(),
            colors: vec![[0, 0, 0]; n],
            points,
        };
        let cluster =
            Cluster::from_raw(&raw, &super::super::types::Frame::new(11.5795305, 48.13643));
        let cam = Camera {
            pano_id: "p".into(),
            centre: [0.0, -30.0, 3.0],
            axes: pose::level_axes(0.0, 0.0, 0.0),
            pose_source: super::super::types::PoseSource::Sfm,
            roll_deg: 0.0,
            pitch_deg: 0.0,
            ground_z: 0.5,
            cam_height_m: 2.5,
            ground_source: super::super::types::GroundSource::Default,
            cluster_id: Some("c".into()),
            shot_id: None,
            reg: None,
            compass_deg: 0.0,
            pose_factor: 1.0,
            width: 5760,
            height: 2880,
            camera_type: super::super::types::CameraModel::Spherical,
            camera_params: vec![],
        };

        let band = facade_band(
            &cluster,
            cam.centre,
            cam.ground_z,
            params.reg_radius_m,
            params.reg_band,
        );
        assert_eq!(band.len(), n, "the whole cloud is inside the band");
        let xy: Vec<[f64; 2]> = band.iter().map(|p| [p[0], p[1]]).collect();
        let whole = inlier_share(&xy, &dt, 0.0, 0.0, 0.0, [cam.centre[0], cam.centre[1]]);
        let sampled = inlier_share(
            &subsample(&xy, REG_MAX_POINTS),
            &dt,
            0.0,
            0.0,
            0.0,
            [cam.centre[0], cam.centre[1]],
        );
        assert!(
            (whole - 0.8).abs() < 0.01 && (sampled - 1.0).abs() < 1e-9,
            "the cloud must separate the two answers: whole {whole}, sampled {sampled}"
        );

        let reg = register_pano(&cam, Some(&cluster), &dt, &params, None, ThetaMode::Never);
        assert_eq!(reg.result.n_points, n, "the band the verdict was taken on");
        assert!(
            (reg.result.inliers_before - whole).abs() < 1e-12,
            "the search measured {} where the whole band is {whole}",
            reg.result.inliers_before
        );
        println!(
            "{n} band points: inliers before {:.4} over all of them, {sampled:.4} over a {} point \
             sample of them",
            reg.result.inliers_before, REG_MAX_POINTS
        );
    }

    /// The stage driver end to end on the real box, with no clouds and no
    /// images: every pano registers as `no_cluster`, every candidate that
    /// reaches the image gates fails as `no_image`, and the products still come
    /// out in the shape the Python writes them.
    ///
    /// The registration and the gates have golden tests of their own; what this
    /// covers is the wiring between them, which is the part with no fixture.
    #[test]
    fn the_align_driver_writes_the_python_shape() {
        if golden::absent() {
            return;
        }

        let buildings = golden::buildings();
        let walls: Vec<Wall> = golden::walls_by_key().into_values().collect();
        let cameras = golden::geometry_cameras();
        let metas: BTreeMap<String, PanoMeta> = golden::panos()
            .iter()
            .filter_map(PanoMeta::from_graph)
            .map(|m| (m.id.clone(), m))
            .collect();
        let params = Params::default();
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for b in &buildings {
            for p in &b.ring {
                for i in 0..2 {
                    lo[i] = lo[i].min(p[i]);
                    hi[i] = hi[i].max(p[i]);
                }
            }
        }
        // Every list the stage asked for, so the test can hold it to the once
        // its contract promises: a driver that downloads on this callback pays
        // a round trip for each one.
        let asked: std::sync::Mutex<Vec<BTreeSet<String>>> = std::sync::Mutex::new(Vec::new());
        let align = run_align(
            &AlignInput {
                buildings: &buildings,
                walls: &walls,
                cameras: &cameras,
                metas: &metas,
                bbox_xy: [lo[0], lo[1], hi[0], hi[1]],
                params: &params,
            },
            &|_| None,
            &|wanted| asked.lock().unwrap().push(wanted.clone()),
            &|_| None,
        );
        let asked = asked.into_inner().unwrap();
        assert_eq!(asked.len(), 1, "the stage asks for its photographs once");
        assert!(!asked[0].is_empty(), "the gates want photographs");
        assert!(
            asked[0].iter().all(|id| cameras.contains_key(id)),
            "the stage may only ask for panos it was given"
        );
        assert_eq!(align.views.len(), walls.len(), "one record per wall");
        assert_eq!(align.stats.panos, cameras.len());
        assert_eq!(
            align.stats.no_cluster,
            cameras.len(),
            "no clouds were given"
        );
        assert_eq!(align.walls_with_views, 0, "no images means no views");
        assert!(align.reachable > 50, "reachable walls {}", align.reachable);
        assert!(
            align.candidates_no_image > 500,
            "candidates that wanted an image {}",
            align.candidates_no_image
        );
        // Every wall gets a plane, and without a cloud it is the OSM line.
        for w in &walls {
            let fit = &align.planes[&w.key];
            assert_eq!(fit.source, PlaneSource::OsmRaw, "{}: plane source", w.key);
            assert!(
                fit.a_ref.is_some() && fit.b_ref.is_some(),
                "{}: ends",
                w.key
            );
        }

        let dir = std::env::temp_dir().join(format!(
            "arnis-align-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        write_products(&dir, &align, &params).expect("the products write");
        let views: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("views.json")).expect("views.json"))
                .expect("views.json parses");
        let one = &views[&walls[0].key];
        for key in [
            "best_pano",
            "wall_key",
            "building_key",
            "reachable",
            "unreachable_reason",
            "plane_source",
            "single_view",
            "n_candidates",
            "n_los_clean",
            "views",
            "candidates",
        ] {
            assert!(
                !one[key].is_null() || key == "best_pano",
                "views.json has {key}"
            );
        }
        let summary: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("summary.json")).expect("summary.json"))
                .expect("summary.json parses");
        assert_eq!(summary["walls"], walls.len());
        assert_eq!(summary["registration"]["no_cluster"], cameras.len());
        let plane = dir.join("planes").join(format!("{}.json", walls[0].key));
        let plane: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&plane).expect("a plane")).expect("it parses");
        assert!(
            plane["per_view"].is_array(),
            "planes carry their per view list"
        );
        assert!(plane["src_a"].is_string() && plane["src_b"].is_string());
        std::fs::remove_dir_all(&dir).ok();
        println!(
            "the driver ran {} walls and {} panos with no clouds: {} reachable, {} candidates \
             without an image",
            walls.len(),
            cameras.len(),
            align.reachable,
            align.candidates_no_image
        );
    }

    // ----------------------------------------------------------------- golden

    /// The whole registration, replayed from the band points of the run.
    #[test]
    fn golden_registration_matches_the_python_run() {
        if golden::absent() {
            return;
        }

        let file: golden::GoldenRegFile = golden::load("align/reg.json");
        let buildings = golden::buildings();
        let params = Params::default();
        // The grid the run used, rebuilt from the camera positions. The fixture
        // rounds those to 1e-4 m, which is why the origin is only compared to
        // that; the transform itself is then built on the fixture's own origin
        // so the raster comparison below is not a comparison of two grids.
        let covering = covering_dt(&buildings, &file.camera_xy, file.dt.margin_m, file.dt.res_m);
        assert_eq!(covering.n, file.dt.n, "grid size");
        assert!(
            golden::max_abs_diff(&covering.origin, &file.dt.origin) < 1e-3,
            "grid origin {:?} vs {:?}",
            covering.origin,
            file.dt.origin
        );
        let centre = [
            file.dt.origin[0] + 0.5 * file.dt.size_m,
            file.dt.origin[1] + 0.5 * file.dt.size_m,
        ];
        let dt = OutlineDT::new(&buildings, centre, file.dt.size_m, file.dt.res_m);
        assert_eq!(dt.n, file.dt.n, "grid size");
        assert_eq!(
            dt.dt.iter().filter(|d| **d == 0.0).count(),
            file.dt.outline_cells,
            "cells the outlines were drawn on"
        );
        // The raster first: a line drawn one cell over moves a cost, so the
        // fixture carries lookups of the run's own transform.
        let mut worst_dt = 0.0f64;
        for s in &file.dt.samples {
            worst_dt = worst_dt.max((f64::from(dt.distance([s[0], s[1]])) - s[2]).abs());
        }
        assert!(worst_dt < 3e-4, "worst distance lookup {worst_dt} m");

        let mut worst_shift = 0.0f64;
        let mut worst_theta = 0.0f64;
        let mut worst_inliers = 0.0f64;
        let mut worst_ratio = 0.0f64;
        let mut sources: BTreeMap<&str, usize> = BTreeMap::new();
        // What the sample used to cost, kept in the table because it is the
        // whole reason the search runs on every point now.
        let mut sampled_flips = 0usize;
        let mut sampled_moved = 0.0f64;
        for p in &file.panos {
            let band = golden::align_points(&p.points_file);
            assert_eq!(band.len(), p.points, "{}: point count", p.pano_id);
            // The fixture has to carry the whole band, or this test replays a
            // subset the running pipeline never sees and the verdict it checks
            // is not the verdict the run takes.
            assert_eq!(
                p.points, p.band_points,
                "{}: the fixture must carry every band point",
                p.pano_id
            );
            if let Some(s) = &p.sampled {
                if s.accepted != p.local.accepted || s.source != p.local.source {
                    sampled_flips += 1;
                }
                sampled_moved = sampled_moved
                    .max((s.dx - p.local.dx).abs())
                    .max((s.dy - p.local.dy).abs());
            }
            let got = register_band(
                &p.pano_id,
                &band,
                p.band_points,
                [p.centre[0], p.centre[1]],
                &dt,
                &params,
                None,
                ThetaMode::Auto,
            );
            let want = &p.local;
            let r = &got.result;
            assert_eq!(
                r.accepted, want.accepted,
                "{}: accepted, got {r:?} want {want:?}",
                p.pano_id
            );
            assert_eq!(r.source.as_str(), want.source, "{}: source", p.pano_id);
            worst_shift = worst_shift
                .max((r.dx - want.dx).abs())
                .max((r.dy - want.dy).abs());
            worst_theta = worst_theta.max((r.theta_deg - want.theta_deg).abs());
            if let Some(v) = want.inliers_after {
                worst_inliers = worst_inliers.max((r.inliers_after - v).abs());
            }
            if let Some(v) = want.inliers_before {
                worst_inliers = worst_inliers.max((r.inliers_before - v).abs());
            }
            if let Some(v) = want.ambiguity_ratio {
                worst_ratio = worst_ratio.max((r.ambiguity_ratio - v).abs());
            }
            if let Some(v) = want.radius_agreement_m {
                worst_shift = worst_shift.max((r.radius_agreement_m - v).abs());
            }
            assert_eq!(
                r.n_points,
                want.n_points.unwrap_or(0),
                "{}: band points",
                p.pano_id
            );
            if let Some(reason) = &want.reason {
                let got_reason = if r.accepted {
                    "accepted".to_string()
                } else {
                    rejection_reason(r, &params)
                };
                if &got_reason != reason {
                    println!(
                        "{}: reason {got_reason} vs {reason} | ratio {} vs {:?} | inl {} vs {:?} | agree {} vs {:?}",
                        p.pano_id, r.ambiguity_ratio, want.ambiguity_ratio,
                        r.inliers_after, want.inliers_after,
                        r.radius_agreement_m, want.radius_agreement_m
                    );
                }
            }
            match (&got.second_best, &want.second_best) {
                (Some(g), Some(w)) => {
                    worst_shift = worst_shift
                        .max((g[0] - w[0]).abs())
                        .max((g[1] - w[1]).abs());
                }
                (None, None) => {}
                (g, w) => panic!("{}: second best {g:?} vs {w:?}", p.pano_id),
            }
            // The fallback: a rejected local becomes the cluster-wide shift when
            // there is one, and the raw pose when there is not.
            let outcome = &p.outcome;
            let global = if outcome.source == "global" {
                Some([outcome.dx, outcome.dy, 0.0])
            } else {
                None
            };
            let with = register_band(
                &p.pano_id,
                &band,
                p.band_points,
                [p.centre[0], p.centre[1]],
                &dt,
                &params,
                global,
                ThetaMode::Auto,
            );
            assert_eq!(
                with.result.source.as_str(),
                outcome.source,
                "{}: outcome source",
                p.pano_id
            );
            assert_eq!(
                with.result.accepted, outcome.accepted,
                "{}: outcome",
                p.pano_id
            );
            if with.result.accepted {
                // A pano that ends up unregistered keeps its rejected local
                // solution here; run_align is what zeroes the shift, so only an
                // accepted outcome has a shift to compare.
                worst_shift = worst_shift
                    .max((with.result.dx - outcome.dx).abs())
                    .max((with.result.dy - outcome.dy).abs());
            }
            *sources.entry(outcome.source.as_str()).or_insert(0) += 1;
        }
        assert!(worst_shift < 0.05, "worst shift difference {worst_shift} m");
        assert!(
            worst_theta < 0.1,
            "worst rotation difference {worst_theta} deg"
        );
        assert!(
            worst_inliers < 1e-6,
            "worst inlier share difference {worst_inliers}"
        );
        assert!(
            worst_ratio < 1e-4,
            "worst uniqueness difference {worst_ratio}"
        );

        // The cluster-wide fallback, over the whole cluster's band.
        let bbox = file.bbox_xy;
        let global_dt = covering_dt(
            &buildings,
            &[[bbox[0], bbox[1]], [bbox[2], bbox[3]]],
            GLOBAL_MARGIN_M + GLOBAL_RANGE_M + 5.0,
            0.5,
        );
        let mut worst_global = 0.0f64;
        let mut accepted = 0usize;
        for c in &file.clusters {
            let xy = golden::align_points(&c.points_file);
            assert_eq!(xy.len(), c.points, "{}: point count", c.cluster_id);
            let got = global_shift_over(&xy, &global_dt);
            match (got, c.shift) {
                (Some(g), Some(w)) => {
                    worst_global = worst_global
                        .max((g[0] - w[0]).abs())
                        .max((g[1] - w[1]).abs());
                    assert!(
                        (g[2] - w[2]).abs() < 1e-6,
                        "{}: inlier share {} vs {}",
                        c.cluster_id,
                        g[2],
                        w[2]
                    );
                    accepted += 1;
                }
                (None, None) => {}
                (g, w) => panic!("{}: global shift {g:?} vs {w:?}", c.cluster_id),
            }
        }
        assert!(worst_global < 0.05, "worst global shift {worst_global} m");
        let sampled = file.panos.iter().filter(|p| p.sampled.is_some()).count();
        println!(
            "{} panos ({sources:?}) and {} clusters ({accepted} with a fallback shift): worst \
             distance lookup {worst_dt:.2e} m, shift {worst_shift:.2e} m, rotation \
             {worst_theta:.2e} deg, inliers {worst_inliers:.2e}, uniqueness {worst_ratio:.2e}, \
             global shift {worst_global:.2e} m; a 4000 point subsample would have changed the \
             verdict on {sampled_flips} of the {sampled} panos whose band is bigger than that and \
             moved a shift by up to {sampled_moved:.2} m",
            file.panos.len(),
            file.clusters.len()
        );
    }
}
