//! Correcting the rectangle from the picture. Port of `tools/facade_lab/refine.py`.
//!
//! One view's loose crop goes in and the wall's rectangle comes out. Everything
//! here is evidence taken from the pixels rather than from OSM: the lean and
//! keystone of the facade from its vertical edges, the roofline from the sky and
//! from a roof-like horizontal edge, the ground row, the two ends of the wall
//! from the column energy profile, and the phase of the one metre block grid.
//! [`decide_wall`] then merges what every view of a wall said.
//!
//! Conventions, the same as the Python:
//!
//! * A vertical segment's **lean** is positive when its top is to the right
//!   (+x, +s); a horizontal segment's **tilt** is positive when it rises to the
//!   right. The correction is `s' = s - tan(lean(s)) (h - h_cam)`, built as a
//!   homography through the four crop corners and kept in metric `(s, h)` so it
//!   can be folded into any pixel grid later.
//! * Heights are metres above the crop's `z_base`. The ground row is measured
//!   first and its `dz` moves that base, so every other height in a
//!   [`Refinement`] is relative to the corrected one (`z_base + dz`).
//! * The column profile is resampled to [`PROFILE_PPM`] columns per metre from
//!   `profile_x0_m`, so it does not depend on the crop's resolution.
//!
//! ## The line segment detector
//!
//! The Python calls OpenCV's `createLineSegmentDetector(LSD_REFINE_STD)`, which
//! is not available here, so [`detect_segments`] is **LSD itself** (Grompone von
//! Gioi, Jakubowicz, Morel and Randall, 2012) written out: the 0.8 Gaussian
//! downscale, the level-line field from 2x2 differences, the gradient magnitude
//! pseudo-ordering into 1024 bins, region growing at a 22.5 degree tolerance,
//! the rectangle from the region's gradient weighted inertia, and the density
//! based refinement that tightens the angle tolerance and then cuts the region's
//! radius. It is the same algorithm rather than a lookalike because the pipeline
//! reads three things off these segments, the lean, the plane gate and the
//! column profile's vertical energy, and each one is sensitive to a different
//! property: the angles, the total length, and where the segments sit.
//!
//! What is deliberately **not** here is the a-contrario NFA validation, and that
//! is not a shortcut: OpenCV computes it only for `LSD_REFINE_ADV`, and
//! `LSD_REFINE_STD`, which is what the Python asks for, keeps every region that
//! survives the density refinement. Measured on a fixture crop, OpenCV returns
//! 325 segments for `LSD_REFINE_NONE`, 391 for `LSD_REFINE_STD` and 231 for
//! `LSD_REFINE_ADV`; this implementation returns 246 with the validation and 391
//! without it. (STD returning more than NONE is not a typo: the refinement frees
//! the pixels it drops from a region, and they go on to seed segments of their
//! own.) The only piece of that model kept is `min_reg_size`, which
//! `LSD_REFINE_STD` uses too.
//!
//! ## Where this differs from the Python, and why
//!
//! * **The roof vote is the fixed one.** `decide_wall` used to take an
//!   unweighted median over the ROOF_SKY views and ignore the ROOF_EDGE ones
//!   whenever any sky view existed, so one bad view outvoted two that agreed.
//!   [`roof_vote`] instead runs a provenance ladder (a sky boundary the C21 rule
//!   could confirm, else the edge views, else an unconfirmed sky boundary) and
//!   takes the view score weighted median inside the family that wins. The fix
//!   is in the Python too and the golden fixtures carry it; on the Munich box it
//!   moves four walls, `r6035286_10` among them, which is the wall MEASURED.md
//!   names.
//! * **The image primitives are private to this module.** `imgops.rs` owns
//!   morphology, connected components, the median filter and OkLab, and this
//!   module uses them; Sobel, the Gaussian, HSV, CIE Lab, Otsu and the
//!   perspective warp are only needed here so far, so they live here rather than
//!   widening a shared file two other stages depend on.
//! * **The shear warp lives here**, although `rectify.py` is where the Python
//!   keeps it, because it is a crop-space operation `refine_view` is the only
//!   caller of. If `rectify.rs` grows one of its own they should be merged.
//! * `refine_view` takes what it needs as [`ViewInputs`] rather than a wall, a
//!   plane fit and a camera, so it does not depend on stages that are not ported
//!   yet: the caller passes the fitted wall's length and the cloud height it
//!   would have read off them. `extent_joint` loses the profile argument the
//!   Python takes and never reads.
//! * [`decide_height`] falls back to `Params::default_height_m` where the Python
//!   hardcodes 9.0. It is the same number today, and going through the parameter
//!   keeps it the same number tomorrow.
//!
//! ## What that comes to, measured
//!
//! Against `tests/golden/facade/refine/`, which is the reference run's own
//! products: over all 111 walls the merge is exact (worst height difference
//! 3e-9 m, worst wall end 5e-7 m, every flag list identical), and over the 14
//! sampled views refined from their crops the roofline is within 0.04 m, the
//! ground row within 0.001 m, the lean within 0.03 degrees where the shear was
//! accepted and every flag agrees. End to end on the seven walls that carry
//! their crops the extent is exact and the height is within 0.03 m.
//!
//! The one place the two answers part is a coin flip in the algorithm rather
//! than a difference in the port. The ground row is the strongest horizontal
//! edge in a 1.5 m window, and on one view of `w81190157_2` that window holds
//! two edges 0.9 m apart whose energy differs by 0.2 per cent; a 0.008 degree
//! difference in the lean, which is what a 96 per cent identical segment list
//! comes to, tips it the other way. The base and the heights measured from it
//! move together, so the facade's top stays where it is in the world.

#![allow(dead_code)]

use image::{Rgb, RgbImage};

use super::imgops::{self, Mask};
use super::rectify::LooseCrop;
use super::types::{
    GroundSource, Params, Wall, WallDecision, OCC_CLOUD, OCC_FOOTPRINT, OCC_NADIR, OCC_OUTSIDE,
    OCC_SEG, OCC_ZENITH,
};

// --------------------------------------------------------------------------- constants

// line families
const MIN_SEG_M: f64 = 0.6;
const VERT_FAMILY_DEG: f64 = 10.0;
const HORIZ_FAMILY_DEG: f64 = 15.0;
const HORIZ_MIN_M: f64 = 1.0;
/// Every bit that means "this texel is not the wall".
pub const OCCLUDED_BITS: u8 =
    OCC_FOOTPRINT | OCC_CLOUD | OCC_NADIR | OCC_ZENITH | OCC_SEG | OCC_OUTSIDE;
// lean
const HUBER_DEG: f64 = 1.0;
const LEAN_INLIER_DEG: f64 = 1.5;
const IRLS_ITERS: usize = 10;
/// Below this the keystone term is not identifiable and `c1` is pinned to zero.
const MIN_X_SPREAD_M: f64 = 1.5;
const GRAD_MAX_DEG: f64 = 12.0;
const GRAD_BIN_DEG: f64 = 0.25;
// plane gate
const GATE_MIN_LINES: usize = 8;
const GATE_MIN_TOTAL_M: f64 = 15.0;
// sky, roof, ground
const SKY_SOBEL_MAX: f32 = 25.0;
/// OpenCV hue units, 0 to 179.
const SKY_H_RANGE: (i32, i32) = (90, 135);
const SKY_TOP_FRAC: f64 = 0.15;
const SKY_RULE_MIN: f64 = 0.05;
const SKY_RULE_MAX: f64 = 0.60;
const ROOF_MIN_REACH: f64 = 0.5;
const ROOF_SPREAD_FRAC: f64 = 0.15;
const ROOF_EDGE_WINDOW: (f64, f64) = (0.8, 1.6);
const ROOF_EDGE_RATIO: f64 = 2.0;
const ROOF_DEFAULT_RANGE: (f64, f64) = (3.0, 80.0);
const ROOF_BEHIND_M: f64 = 2.0;
/// The metre of "sky" a roofline rests on, and the share of it that may be the
/// crop's own no-data before the column stops counting as sky.
const ROOF_NODATA_BAND_M: f64 = 1.0;
const ROOF_NODATA_MAX: f64 = 0.5;
const EDGE_ENERGY_FLOOR: f64 = 3.0;
const GROUND_WINDOW_M: f64 = 0.75;
const GROUND_EDGE_RATIO: f64 = 3.0;
/// The floor under a view's score in the roof vote, so a candidate that barely
/// scraped through still counts for something.
const ROOF_SCORE_FLOOR: f64 = 0.05;
// extent
pub const PROFILE_PPM: f64 = 10.0;
const PROFILE_SIGMA_M: f64 = 0.2;
const PEAK_MIN_E: f64 = 3.0;
const PEAK_MIN_VLEN_M: f64 = 1.5;
const PEAK_MIN_SEP_M: f64 = 0.5;
const WINDOW_LIMIT_M: f64 = 0.3;
const RHYTHM_RANGE_M: (f64, f64) = (0.8, 6.0);
const RHYTHM_MIN_PEAK: f64 = 0.3;
const RHYTHM_MAX_DIFF: f64 = 0.2;
const COLOUR_MAX_DE: f64 = 12.0;
// phase, a private copy of the block rule
const DARK_THR: f64 = 0.12;
const GLASS_B: f64 = -0.04;
const GLASS_L: f64 = 0.05;
const BLOCK_MIN_VALID: f64 = 0.3;

// Flag strings. They travel into `WallDecision::flags` and out to the export, so
// they are the Python's spelling exactly.
pub const SHEAR_NONE: &str = "SHEAR_NONE";
pub const SHEAR_OK: &str = "SHEAR_OK";
pub const SHEAR_REJECTED: &str = "SHEAR_REJECTED";
pub const ON_PLANE: &str = "ON_PLANE";
pub const PLANE_MISMATCH: &str = "PLANE_MISMATCH";
pub const UNVERIFIED: &str = "UNVERIFIED";
pub const ROOF_SKY: &str = "ROOF_SKY";
pub const ROOF_EDGE: &str = "ROOF_EDGE";
pub const ROOF_OSM: &str = "ROOF_OSM";
pub const ROOF_BEHIND: &str = "ROOF_BEHIND";
pub const ROOF_UNCONFIRMED: &str = "ROOF_UNCONFIRMED";
pub const ROOF_NODATA: &str = "ROOF_NODATA";
pub const GROUND_EDGE: &str = "GROUND_EDGE";
pub const GROUND_CLOUD: &str = "GROUND_CLOUD";
pub const GROUND_DEFAULT: &str = "GROUND_DEFAULT";

// --------------------------------------------------------------------------- types

/// One line segment in crop pixels.
///
/// `angle_deg` is the lean for a member of the vertical family and the tilt for
/// a horizontal one, which is why the endpoints are ordered: verticals run
/// bottom to top (`y0 >= y1`) and horizontals left to right (`x0 <= x1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineSegment {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub length: f64,
    pub angle_deg: f64,
}

impl LineSegment {
    pub fn mid_x(&self) -> f64 {
        0.5 * (self.x0 + self.x1)
    }

    pub fn mid_y(&self) -> f64 {
        0.5 * (self.y0 + self.y1)
    }
}

/// One local maximum of the column energy profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    /// Wall coordinate of the peak, in metres.
    pub x_m: f64,
    pub energy: f64,
    /// Vertical line length gathered within two sigma of the peak, in metres.
    pub vlen_m: f64,
}

/// What one view says about the wall's geometry.
#[derive(Clone, Debug)]
pub struct Refinement {
    pub view_key: String,
    /// Lean at the wall foot and how it grows along the wall.
    pub c0_deg: f64,
    pub c1_deg_per_m: f64,
    pub lean_flag: String,
    /// Metric `(s, h)` shear, the identity unless the lean was accepted.
    pub h_shear: [[f64; 3]; 3],
    pub plane_slope: f64,
    pub plane_flag: String,
    /// Roofline height above the corrected base.
    pub h_sky: Option<f64>,
    pub roof_flag: String,
    /// `ROOF_BEHIND` and `ROOF_UNCONFIRMED`, which the wall vote reads.
    pub roof_flags: Vec<String>,
    pub roof_spread_m: f64,
    pub ground_dz: f64,
    pub ground_flag: String,
    /// The column energy profile at [`PROFILE_PPM`] columns per metre.
    pub profile: Vec<f32>,
    pub profile_x0_m: f64,
    pub peaks: Vec<Peak>,
    /// The crop's `z_base` plus the ground row's `dz`.
    pub z_base: f64,
}

impl Refinement {
    /// An empty refinement for a view that could not be read.
    pub fn empty(view_key: &str, z_base: f64) -> Self {
        Refinement {
            view_key: view_key.to_string(),
            c0_deg: 0.0,
            c1_deg_per_m: 0.0,
            lean_flag: SHEAR_NONE.into(),
            h_shear: IDENTITY3,
            plane_slope: 0.0,
            plane_flag: UNVERIFIED.into(),
            h_sky: None,
            roof_flag: ROOF_OSM.into(),
            roof_flags: Vec::new(),
            roof_spread_m: 0.0,
            ground_dz: 0.0,
            ground_flag: GROUND_DEFAULT.into(),
            profile: Vec::new(),
            profile_x0_m: 0.0,
            peaks: Vec::new(),
            z_base,
        }
    }
}

/// The numbers behind a [`Refinement`], for the golden test and the debug dump.
/// The Python writes the same set into the view JSON as `details`.
#[derive(Clone, Debug, Default)]
pub struct RefineDetails {
    pub n_vertical: usize,
    pub n_horizontal: usize,
    pub lean_n: usize,
    pub lean_inliers: f64,
    pub lean_rms_deg: f64,
    pub lean_x_spread_m: f64,
    pub c0_after_deg: f64,
    pub grad_c0_deg: f64,
    pub grad_c1_deg_per_m: f64,
    pub grad_support: f64,
    pub plane_gate_n: usize,
    pub sky_mode: &'static str,
    pub h_sky_raw: Option<f64>,
    /// The cloud height corrected by the ground row's `dz`, which is what the
    /// wall decision consumes.
    pub h_cloud: Option<f64>,
    pub s_range: [f64; 2],
    pub box_px: [f64; 4],
    pub l_fit: f64,
    pub h_osm: f64,
}

/// Everything [`refine_view`] needs beyond the crop itself.
///
/// `l_fit` is the fitted wall's length (`plane::fitted_wall(wall, fit).length`)
/// and `h_cloud` the cloud roof height relative to the crop's `z_base`; both
/// come from stages this module does not depend on.
#[derive(Clone, Copy, Debug)]
pub struct ViewInputs<'a> {
    pub wall: &'a Wall,
    pub pano_id: &'a str,
    pub l_fit: f64,
    pub s_vis: Option<(f64, f64)>,
    pub h_cloud: Option<f64>,
    pub ground_source: GroundSource,
}

/// One view's contribution to the wall decision.
#[derive(Clone, Copy, Debug)]
pub struct ViewEvidence<'a> {
    pub refinement: &'a Refinement,
    /// The visibility score, which weights this view's roof estimate.
    pub score: f64,
    /// Maps this view's `s` into the wall frame (CRITIQUE A5).
    pub ds_m: f64,
    /// Cloud roof height above the corrected base.
    pub h_cloud: Option<f64>,
    /// The crop, when it is still in memory: it enables the left/right
    /// consistency test on the extent.
    pub crop: Option<&'a LooseCrop>,
}

const IDENTITY3: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

// --------------------------------------------------------------------------- small numerics

/// `numpy.interp`: linear between the samples, clamped outside.
fn interp(x: f64, xs: &[f64], ys: &[f64]) -> f64 {
    if xs.is_empty() {
        return f64::NAN;
    }
    if x <= xs[0] {
        return ys[0];
    }
    let n = xs.len();
    if x >= xs[n - 1] {
        return ys[n - 1];
    }
    // xs is sorted, so a binary search finds the interval
    let mut lo = 0usize;
    let mut hi = n - 1;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if xs[mid] <= x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let t = (x - xs[lo]) / (xs[hi] - xs[lo]).max(1e-300);
    ys[lo] + t * (ys[hi] - ys[lo])
}

/// `numpy.gradient` along one axis: central differences inside, one sided at
/// the ends, unit spacing.
fn gradient(v: &[f64]) -> Vec<f64> {
    let n = v.len();
    if n < 2 {
        return vec![0.0; n];
    }
    let mut g = vec![0.0; n];
    g[0] = v[1] - v[0];
    g[n - 1] = v[n - 1] - v[n - 2];
    for i in 1..n - 1 {
        g[i] = 0.5 * (v[i + 1] - v[i - 1]);
    }
    g
}

/// `numpy.convolve(v, k, mode="same")`: zero padded, so the ends are damped.
fn convolve_same(v: &[f64], k: &[f64]) -> Vec<f64> {
    let n = v.len();
    let m = k.len();
    let mut out = vec![0.0; n];
    // full convolution has n + m - 1 entries and "same" takes the middle n
    let off = (m - 1) / 2;
    for (i, o) in out.iter_mut().enumerate() {
        let mut acc = 0.0;
        for (j, kj) in k.iter().enumerate() {
            let idx = i as isize + off as isize - j as isize;
            if idx >= 0 && (idx as usize) < n {
                acc += v[idx as usize] * kj;
            }
        }
        *o = acc;
    }
    out
}

/// `numpy.argmax`: the index of the first maximum, where Rust's `max_by` would
/// return the last. Two rows of a smoothed energy profile tie often enough for
/// the difference to move a ground row by a metre.
fn argmax_first(v: impl Iterator<Item = f64>) -> usize {
    let mut best = f64::NEG_INFINITY;
    let mut idx = 0usize;
    for (i, x) in v.enumerate() {
        if x > best {
            best = x;
            idx = i;
        }
    }
    idx
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.iter().sum::<f64>() / v.len() as f64
}

fn weighted_mean(v: &[f64], w: &[f64]) -> f64 {
    let sw: f64 = w.iter().sum();
    if sw <= 0.0 {
        return mean(v);
    }
    v.iter().zip(w).map(|(a, b)| a * b).sum::<f64>() / sw
}

/// The weighted median with linear interpolation between the two straddling
/// samples. Equal weights reproduce `numpy.median` exactly, which is what keeps
/// a wall whose views score alike deciding as the unweighted rule did.
pub fn weighted_median(values: &[f64], weights: &[f64]) -> f64 {
    debug_assert_eq!(values.len(), weights.len());
    if values.is_empty() {
        return f64::NAN;
    }
    let mut idx: Vec<usize> = (0..values.len()).collect();
    idx.sort_by(|a, b| values[*a].total_cmp(&values[*b]));
    let v: Vec<f64> = idx.iter().map(|i| values[*i]).collect();
    let w: Vec<f64> = idx.iter().map(|i| weights[*i]).collect();
    let total: f64 = w.iter().sum();
    if !total.is_finite() || total <= 0.0 {
        return imgops::median(&v);
    }
    let mut p = Vec::with_capacity(v.len());
    let mut cum = 0.0;
    for wi in &w {
        p.push((cum + 0.5 * wi) / total);
        cum += wi;
    }
    interp(0.5, &p, &v)
}

/// A square median filter over `f64` with the border replicated, which is
/// `scipy.ndimage.median_filter(..., mode="nearest")`.
fn median_filter_f64(src: &[f64], w: usize, h: usize, k: usize) -> Vec<f64> {
    let r = (k / 2) as isize;
    let mut out = vec![0.0; w * h];
    let mut win: Vec<f64> = Vec::with_capacity(k * k);
    for y in 0..h {
        for x in 0..w {
            win.clear();
            for dy in -r..=r {
                let sy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                for dx in -r..=r {
                    let sx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                    win.push(src[sy * w + sx]);
                }
            }
            win.sort_by(f64::total_cmp);
            out[y * w + x] = win[win.len() / 2];
        }
    }
    out
}

/// `scipy.signal.find_peaks(x, height, distance)`.
///
/// Local maxima with plateau support (the peak of a flat top is its middle,
/// rounded down), then the height filter, then the distance filter: peaks are
/// taken strongest first and anything within `distance` of a kept peak goes.
fn find_peaks(x: &[f32], height: f64, distance: usize) -> Vec<usize> {
    let n = x.len();
    let mut peaks: Vec<usize> = Vec::new();
    let mut i = 1usize;
    while i + 1 < n {
        if x[i - 1] < x[i] {
            // walk the plateau
            let mut j = i;
            while j + 1 < n && x[j + 1] == x[i] {
                j += 1;
            }
            if j + 1 < n && x[j + 1] < x[i] {
                peaks.push((i + j) / 2);
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    peaks.retain(|p| x[*p] as f64 >= height);
    if distance <= 1 || peaks.is_empty() {
        return peaks;
    }
    // strongest first, ties by the later index, which is what
    // `numpy.argsort(...)[::-1]` on a stable sort gives
    let mut order: Vec<usize> = (0..peaks.len()).collect();
    order.sort_by(|a, b| {
        (x[peaks[*a]], *a)
            .partial_cmp(&(x[peaks[*b]], *b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    order.reverse();
    let mut keep = vec![true; peaks.len()];
    for i in order {
        if !keep[i] {
            continue;
        }
        let mut k = i as isize - 1;
        while k >= 0 && peaks[i] - peaks[k as usize] < distance {
            keep[k as usize] = false;
            k -= 1;
        }
        let mut k = i + 1;
        while k < peaks.len() && peaks[k] - peaks[i] < distance {
            keep[k] = false;
            k += 1;
        }
    }
    peaks
        .into_iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(p, _)| p)
        .collect()
}

// --------------------------------------------------------------------------- image primitives

/// `BORDER_REFLECT_101`: the index a sample outside the image folds onto.
#[inline]
fn reflect101(i: isize, n: usize) -> usize {
    if n == 1 {
        return 0;
    }
    let n = n as isize;
    let mut i = i;
    loop {
        if i < 0 {
            i = -i;
        } else if i >= n {
            i = 2 * n - 2 - i;
        } else {
            return i as usize;
        }
    }
}

/// `cv2.cvtColor(RGB2GRAY)` on bytes, including OpenCV's fixed point rounding.
pub fn to_gray(img: &RgbImage) -> Vec<u8> {
    img.pixels()
        .map(|p| {
            let [r, g, b] = p.0;
            (((r as u32) * 4899 + (g as u32) * 9617 + (b as u32) * 1868 + 8192) >> 14) as u8
        })
        .collect()
}

/// `cv2.Sobel(src, CV_32F, dx, dy, ksize=3)` with the default reflecting
/// border. `dx == 1` differentiates along x, anything else along y.
fn sobel(src: &[f32], w: usize, h: usize, dx: usize, _dy: usize) -> Vec<f32> {
    // separable: the derivative kernel along the differentiated axis, the
    // smoothing kernel along the other
    let d: [f32; 3] = [-1.0, 0.0, 1.0];
    let s: [f32; 3] = [1.0, 2.0, 1.0];
    let (kx, ky) = if dx == 1 { (d, s) } else { (s, d) };
    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0f32;
            for (t, k) in kx.iter().enumerate() {
                let sx = reflect101(x as isize + t as isize - 1, w);
                acc += src[y * w + sx] * k;
            }
            tmp[y * w + x] = acc;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0f32;
            for (t, k) in ky.iter().enumerate() {
                let sy = reflect101(y as isize + t as isize - 1, h);
                acc += tmp[sy * w + x] * k;
            }
            out[y * w + x] = acc;
        }
    }
    out
}

/// `cv2.getGaussianKernel`, including the small fixed kernels OpenCV uses when
/// sigma is not given.
fn gaussian_kernel(n: usize, sigma: f64) -> Vec<f64> {
    const SMALL: [&[f64]; 4] = [
        &[1.0],
        &[0.25, 0.5, 0.25],
        &[0.0625, 0.25, 0.375, 0.25, 0.0625],
        &[
            0.03125, 0.109375, 0.21875, 0.28125, 0.21875, 0.109375, 0.03125,
        ],
    ];
    if sigma <= 0.0 && n <= 7 && n % 2 == 1 {
        return SMALL[n / 2].to_vec();
    }
    let sigma = if sigma > 0.0 {
        sigma
    } else {
        0.3 * ((n as f64 - 1.0) * 0.5 - 1.0) + 0.8
    };
    let scale = -0.5 / (sigma * sigma);
    let c = (n as f64 - 1.0) * 0.5;
    let mut k: Vec<f64> = (0..n)
        .map(|i| {
            let x = i as f64 - c;
            (scale * x * x).exp()
        })
        .collect();
    let sum: f64 = k.iter().sum();
    for v in &mut k {
        *v /= sum;
    }
    k
}

/// The kernel side `cv2.GaussianBlur` picks for `ksize=0` on a float image.
///
/// Every index and size in this module that mirrors a `round()` in the Python
/// goes through [`imgops::round_half_even`], because Python rounds halves to the
/// even neighbour and Rust rounds them away from zero: `0.15 * 630` is 94.5, and
/// the two answers differ by a row of the sky mask.
fn gaussian_ksize(sigma: f64) -> usize {
    let n = imgops::round_half_even(sigma * 4.0 * 2.0 + 1.0) as i64;
    (n | 1).max(1) as usize
}

/// `cv2.GaussianBlur` with a reflecting border, separable.
fn gaussian_blur(src: &[f32], w: usize, h: usize, ksize: usize, sigma: f64) -> Vec<f32> {
    let k = gaussian_kernel(ksize, sigma);
    let r = (ksize / 2) as isize;
    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0f64;
            for (t, kt) in k.iter().enumerate() {
                let sx = reflect101(x as isize + t as isize - r, w);
                acc += src[y * w + sx] as f64 * kt;
            }
            tmp[y * w + x] = acc as f32;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0f64;
            for (t, kt) in k.iter().enumerate() {
                let sy = reflect101(y as isize + t as isize - r, h);
                acc += tmp[sy * w + x] as f64 * kt;
            }
            out[y * w + x] = acc as f32;
        }
    }
    out
}

/// One dimensional Gaussian blur of a row, the `GaussianBlur` of a 1 by N image.
fn gaussian_blur_1d(src: &[f64], sigma: f64) -> Vec<f64> {
    if sigma <= 0.0 || src.len() < 2 {
        return src.to_vec();
    }
    let n = gaussian_ksize(sigma);
    let k = gaussian_kernel(n, sigma);
    let r = (n / 2) as isize;
    (0..src.len())
        .map(|x| {
            let mut acc = 0.0;
            for (t, kt) in k.iter().enumerate() {
                let sx = reflect101(x as isize + t as isize - r, src.len());
                acc += src[sx] * kt;
            }
            acc
        })
        .collect()
}

/// `cv2.cvtColor(RGB2HSV)` on bytes: hue 0 to 179, the integer path OpenCV
/// takes, because the sky rule compares hue against fixed bounds and the float
/// formula lands a unit either side of it.
fn rgb_to_hsv(img: &RgbImage) -> Vec<[u8; 3]> {
    const SHIFT: i32 = 12;
    let sdiv = |i: usize| -> i64 {
        if i == 0 {
            0
        } else {
            ((255i64 << SHIFT) as f64 / i as f64).round() as i64
        }
    };
    let hdiv = |i: usize| -> i64 {
        if i == 0 {
            0
        } else {
            ((180i64 << SHIFT) as f64 / (6.0 * i as f64)).round() as i64
        }
    };
    let sdiv_table: Vec<i64> = (0..256).map(sdiv).collect();
    let hdiv_table: Vec<i64> = (0..256).map(hdiv).collect();
    img.pixels()
        .map(|p| {
            let (r, g, b) = (p.0[0] as i64, p.0[1] as i64, p.0[2] as i64);
            let v = r.max(g).max(b);
            let vmin = r.min(g).min(b);
            let diff = v - vmin;
            let s = (diff * sdiv_table[v as usize] + (1 << (SHIFT - 1))) >> SHIFT;
            let mut hh = if v == r {
                g - b
            } else if v == g {
                b - r + 2 * diff
            } else {
                r - g + 4 * diff
            };
            hh = (hh * hdiv_table[diff as usize] + (1 << (SHIFT - 1))) >> SHIFT;
            if hh < 0 {
                hh += 180;
            }
            [
                hh.clamp(0, 255) as u8,
                s.clamp(0, 255) as u8,
                v.clamp(0, 255) as u8,
            ]
        })
        .collect()
}

/// sRGB to CIE Lab in OpenCV's byte encoding: L in 0 to 255, a and b offset by
/// 128. `_column_lab` rescales them, and it is the quantised values it rescales.
fn rgb_to_lab_bytes(px: [u8; 3]) -> [u8; 3] {
    #[inline]
    fn lin(c: f64) -> f64 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    #[inline]
    fn f(t: f64) -> f64 {
        if t > 0.008856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    }
    let r = lin(px[0] as f64 / 255.0);
    let g = lin(px[1] as f64 / 255.0);
    let b = lin(px[2] as f64 / 255.0);
    // the sRGB D65 matrix OpenCV carries
    let x = (0.412453 * r + 0.357580 * g + 0.180423 * b) / 0.950456;
    let y = 0.212671 * r + 0.715160 * g + 0.072169 * b;
    let z = (0.019334 * r + 0.119193 * g + 0.950227 * b) / 1.088754;
    let (fx, fy, fz) = (f(x), f(y), f(z));
    let l = if y > 0.008856 {
        116.0 * fy - 16.0
    } else {
        903.3 * y
    };
    let a = 500.0 * (fx - fy);
    let bb = 200.0 * (fy - fz);
    [
        imgops::round_half_even(l * 255.0 / 100.0).clamp(0.0, 255.0) as u8,
        imgops::round_half_even(a + 128.0).clamp(0.0, 255.0) as u8,
        imgops::round_half_even(bb + 128.0).clamp(0.0, 255.0) as u8,
    ]
}

/// CIE Lab with L in 0 to 100 and a, b about zero, through the byte encoding the
/// Python goes through.
fn rgb_to_lab(px: [u8; 3]) -> [f64; 3] {
    let q = rgb_to_lab_bytes(px);
    [
        q[0] as f64 * 100.0 / 255.0,
        q[1] as f64 - 128.0,
        q[2] as f64 - 128.0,
    ]
}

/// Otsu's threshold over a byte histogram, the value
/// `cv2.threshold(..., THRESH_OTSU)` returns.
fn otsu_threshold(hist: &[usize; 256]) -> f64 {
    let total: usize = hist.iter().sum();
    if total == 0 {
        return 0.0;
    }
    let inv = 1.0 / total as f64;
    let mut mu = 0.0;
    for (i, c) in hist.iter().enumerate() {
        mu += i as f64 * (*c as f64 * inv);
    }
    let (mut q1, mut mu1_acc) = (0.0f64, 0.0f64);
    let (mut max_sigma, mut max_val) = (0.0f64, 0.0f64);
    for (i, count) in hist.iter().enumerate() {
        let p_i = *count as f64 * inv;
        mu1_acc += i as f64 * p_i;
        q1 += p_i;
        let q2 = 1.0 - q1;
        if q1 < f64::EPSILON || q2 < f64::EPSILON {
            continue;
        }
        let mu1 = mu1_acc / q1;
        let mu2 = (mu - q1 * mu1) / q2;
        let sigma = q1 * q2 * (mu1 - mu2) * (mu1 - mu2);
        if sigma > max_sigma {
            max_sigma = sigma;
            max_val = i as f64;
        }
    }
    max_val
}

fn histogram(v: impl Iterator<Item = u8>) -> [usize; 256] {
    let mut h = [0usize; 256];
    for x in v {
        h[x as usize] += 1;
    }
    h
}

/// Bilinear resize with OpenCV's half pixel centres and a replicated border.
///
/// `scale` is the source pixels per destination pixel and is passed in rather
/// than derived from the sizes, because `cv2.resize` given a scale factor and
/// no destination size uses the factor itself, not the ratio of the rounded
/// sizes. A quarter of a pixel of drift over the width of the crop is enough to
/// move the marginal line segments.
fn resize_bilinear(src: &[u8], w: usize, h: usize, dw: usize, dh: usize, scale: f64) -> Vec<u8> {
    let sx = scale;
    let sy = scale;
    let mut out = vec![0u8; dw * dh];
    for y in 0..dh {
        let fy = ((y as f64 + 0.5) * sy - 0.5).max(0.0);
        let y0 = (fy.floor() as usize).min(h - 1);
        let y1 = (y0 + 1).min(h - 1);
        let ty = fy - y0 as f64;
        for x in 0..dw {
            let fx = ((x as f64 + 0.5) * sx - 0.5).max(0.0);
            let x0 = (fx.floor() as usize).min(w - 1);
            let x1 = (x0 + 1).min(w - 1);
            let tx = fx - x0 as f64;
            let a = src[y0 * w + x0] as f64 * (1.0 - tx) + src[y0 * w + x1] as f64 * tx;
            let b = src[y1 * w + x0] as f64 * (1.0 - tx) + src[y1 * w + x1] as f64 * tx;
            out[y * dw + x] = (a * (1.0 - ty) + b * ty).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

// --------------------------------------------------------------------------- 3x3 linear algebra

fn mat3_mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut o = [[0.0; 3]; 3];
    for (i, row) in o.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

fn mat3_inv(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let d = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d = if d.abs() < 1e-300 { 1e-300 } else { d };
    let mut o = [[0.0; 3]; 3];
    o[0][0] = (m[1][1] * m[2][2] - m[1][2] * m[2][1]) / d;
    o[0][1] = (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / d;
    o[0][2] = (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / d;
    o[1][0] = (m[1][2] * m[2][0] - m[1][0] * m[2][2]) / d;
    o[1][1] = (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / d;
    o[1][2] = (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / d;
    o[2][0] = (m[1][0] * m[2][1] - m[1][1] * m[2][0]) / d;
    o[2][1] = (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / d;
    o[2][2] = (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / d;
    o
}

/// `cv2.getPerspectiveTransform`: the homography through four point pairs, by
/// Gaussian elimination on the 8 by 8 system.
fn perspective_transform(src: &[[f64; 2]; 4], dst: &[[f64; 2]; 4]) -> [[f64; 3]; 3] {
    let mut a = [[0.0f64; 9]; 8];
    for i in 0..4 {
        let (x, y) = (src[i][0], src[i][1]);
        let (u, v) = (dst[i][0], dst[i][1]);
        a[i] = [x, y, 1.0, 0.0, 0.0, 0.0, -x * u, -y * u, u];
        a[i + 4] = [0.0, 0.0, 0.0, x, y, 1.0, -x * v, -y * v, v];
    }
    // partial pivoting
    for col in 0..8 {
        let mut piv = col;
        for r in col + 1..8 {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        a.swap(col, piv);
        let d = a[col][col];
        if d.abs() < 1e-300 {
            return IDENTITY3;
        }
        for v in a[col].iter_mut().skip(col) {
            *v /= d;
        }
        for r in 0..8 {
            if r == col {
                continue;
            }
            let f = a[r][col];
            if f == 0.0 {
                continue;
            }
            let pivot = a[col];
            for (c, v) in a[r].iter_mut().enumerate().skip(col) {
                *v -= f * pivot[c];
            }
        }
    }
    [
        [a[0][8], a[1][8], a[2][8]],
        [a[3][8], a[4][8], a[5][8]],
        [a[6][8], a[7][8], 1.0],
    ]
}

#[inline]
fn apply_h(h: &[[f64; 3]; 3], x: f64, y: f64) -> (f64, f64) {
    let w = h[2][0] * x + h[2][1] * y + h[2][2];
    let w = if w.abs() < 1e-12 { 1e-12 } else { w };
    (
        (h[0][0] * x + h[0][1] * y + h[0][2]) / w,
        (h[1][0] * x + h[1][1] * y + h[1][2]) / w,
    )
}

// --------------------------------------------------------------------------- LSD

// The published parameters, which are also OpenCV's defaults for
// `LSD_REFINE_STD`.
const LSD_SCALE: f64 = 0.8;
const LSD_SIGMA_SCALE: f64 = 0.6;
const LSD_QUANT: f64 = 2.0;
const LSD_ANG_TH: f64 = 22.5;
const LSD_DENSITY_TH: f64 = 0.7;
const LSD_N_BINS: usize = 1024;
/// The level-line angle is undefined where the gradient is too weak.
const NOTDEF: f64 = -1024.0;

/// One pixel of a growing region.
#[derive(Clone, Copy)]
struct RegionPoint {
    x: i32,
    y: i32,
    angle: f64,
    modgrad: f64,
}

/// The rectangle a region is approximated by.
#[derive(Clone, Copy, Default)]
struct LsdRect {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    width: f64,
    x: f64,
    y: f64,
    theta: f64,
    dx: f64,
    dy: f64,
    prec: f64,
    p: f64,
}

#[inline]
fn angle_diff_signed(a: f64, b: f64) -> f64 {
    let mut d = a - b;
    while d <= -std::f64::consts::PI {
        d += std::f64::consts::TAU;
    }
    while d > std::f64::consts::PI {
        d -= std::f64::consts::TAU;
    }
    d
}

/// The level-line field, in the layout the region growing wants.
struct AngleField {
    w: usize,
    h: usize,
    angles: Vec<f64>,
    modgrad: Vec<f64>,
}

impl AngleField {
    #[inline]
    fn aligned(&self, x: i32, y: i32, theta: f64, prec: f64) -> bool {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return false;
        }
        let a = self.angles[y as usize * self.w + x as usize];
        if a == NOTDEF {
            return false;
        }
        let mut n = (theta - a).abs();
        if n > 1.5 * std::f64::consts::PI {
            n -= std::f64::consts::TAU;
            n = n.abs();
        }
        n <= prec
    }
}

/// The 2x2 gradient and the level-line angle of every pixel, plus the pixels
/// ordered by gradient magnitude through 1024 bins, which is LSD's pseudo
/// ordering: exact sorting is not needed, only that strong pixels seed first.
fn ll_angle(img: &[u8], w: usize, h: usize, threshold: f64) -> (AngleField, Vec<(usize, usize)>) {
    let mut angles = vec![NOTDEF; w * h];
    let mut modgrad = vec![0.0f64; w * h];
    let mut max_grad = -1.0f64;
    for y in 0..h.saturating_sub(1) {
        for x in 0..w.saturating_sub(1) {
            let a = img[y * w + x] as i32;
            let b = img[y * w + x + 1] as i32;
            let c = img[(y + 1) * w + x] as i32;
            let d = img[(y + 1) * w + x + 1] as i32;
            let da = d - a;
            let bc = b - c;
            let gx = (da + bc) as f64;
            let gy = (da - bc) as f64;
            let norm = ((gx * gx + gy * gy) / 4.0).sqrt();
            modgrad[y * w + x] = norm;
            if norm > threshold {
                angles[y * w + x] = gx.atan2(-gy).rem_euclid(std::f64::consts::TAU);
                if norm > max_grad {
                    max_grad = norm;
                }
            }
        }
    }
    let mut bins: Vec<Vec<(usize, usize)>> = vec![Vec::new(); LSD_N_BINS];
    let coef = if max_grad > 0.0 {
        (LSD_N_BINS - 1) as f64 / max_grad
    } else {
        0.0
    };
    for y in 0..h.saturating_sub(1) {
        for x in 0..w.saturating_sub(1) {
            let i = ((modgrad[y * w + x] * coef) as usize).min(LSD_N_BINS - 1);
            bins[i].push((x, y));
        }
    }
    let mut ordered = Vec::with_capacity(w * h);
    for bin in bins.iter().rev() {
        ordered.extend_from_slice(bin);
    }
    (
        AngleField {
            w,
            h,
            angles,
            modgrad,
        },
        ordered,
    )
}

/// Grows a region of pixels whose level-line angles stay within `prec` of the
/// region's running mean angle.
fn region_grow(
    f: &AngleField,
    used: &mut [bool],
    seed: (usize, usize),
    prec: f64,
    reg: &mut Vec<RegionPoint>,
) -> f64 {
    reg.clear();
    let (sx, sy) = seed;
    let mut reg_angle = f.angles[sy * f.w + sx];
    reg.push(RegionPoint {
        x: sx as i32,
        y: sy as i32,
        angle: reg_angle,
        modgrad: f.modgrad[sy * f.w + sx],
    });
    used[sy * f.w + sx] = true;
    let mut sumdx = reg_angle.cos();
    let mut sumdy = reg_angle.sin();
    let mut i = 0usize;
    while i < reg.len() {
        let p = reg[i];
        let xx_min = (p.x - 1).max(0);
        let xx_max = (p.x + 1).min(f.w as i32 - 1);
        let yy_min = (p.y - 1).max(0);
        let yy_max = (p.y + 1).min(f.h as i32 - 1);
        for yy in yy_min..=yy_max {
            for xx in xx_min..=xx_max {
                let idx = yy as usize * f.w + xx as usize;
                if used[idx] || !f.aligned(xx, yy, reg_angle, prec) {
                    continue;
                }
                used[idx] = true;
                let angle = f.angles[idx];
                reg.push(RegionPoint {
                    x: xx,
                    y: yy,
                    angle,
                    modgrad: f.modgrad[idx],
                });
                sumdx += angle.cos();
                sumdy += angle.sin();
                reg_angle = sumdy.atan2(sumdx).rem_euclid(std::f64::consts::TAU);
            }
        }
        i += 1;
    }
    reg_angle
}

/// The region's main direction, from the gradient weighted inertia matrix.
fn region_theta(reg: &[RegionPoint], x: f64, y: f64, reg_angle: f64, prec: f64) -> f64 {
    let (mut ixx, mut iyy, mut ixy) = (0.0f64, 0.0f64, 0.0f64);
    for p in reg {
        let dx = p.x as f64 - x;
        let dy = p.y as f64 - y;
        ixx += dy * dy * p.modgrad;
        iyy += dx * dx * p.modgrad;
        ixy -= dx * dy * p.modgrad;
    }
    let lambda = 0.5 * (ixx + iyy - ((ixx - iyy) * (ixx - iyy) + 4.0 * ixy * ixy).sqrt());
    let mut theta = if ixx.abs() > iyy.abs() {
        (lambda - ixx).atan2(ixy)
    } else {
        ixy.atan2(lambda - iyy)
    };
    theta = theta.rem_euclid(std::f64::consts::TAU);
    if angle_diff_signed(theta, reg_angle).abs() > prec {
        theta += std::f64::consts::PI;
    }
    theta
}

/// The rectangle covering a region: centroid, main direction, and the extremes
/// along and across it.
fn region_to_rect(reg: &[RegionPoint], reg_angle: f64, prec: f64, p: f64) -> LsdRect {
    let (mut x, mut y, mut sum) = (0.0f64, 0.0f64, 0.0f64);
    for pt in reg {
        x += pt.x as f64 * pt.modgrad;
        y += pt.y as f64 * pt.modgrad;
        sum += pt.modgrad;
    }
    if sum <= 0.0 {
        return LsdRect::default();
    }
    x /= sum;
    y /= sum;
    let theta = region_theta(reg, x, y, reg_angle, prec);
    let (dx, dy) = (theta.cos(), theta.sin());
    let (mut l_min, mut l_max, mut w_min, mut w_max) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for pt in reg {
        let rdx = pt.x as f64 - x;
        let rdy = pt.y as f64 - y;
        let l = rdx * dx + rdy * dy;
        let ww = -rdx * dy + rdy * dx;
        if l > l_max {
            l_max = l;
        } else if l < l_min {
            l_min = l;
        }
        if ww > w_max {
            w_max = ww;
        } else if ww < w_min {
            w_min = ww;
        }
    }
    LsdRect {
        x1: x + l_min * dx,
        y1: y + l_min * dy,
        x2: x + l_max * dx,
        y2: y + l_max * dy,
        width: (w_max - w_min).max(1.0),
        x,
        y,
        theta,
        dx,
        dy,
        prec,
        p,
    }
}

fn dist(x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    ((x2 - x1) * (x2 - x1) + (y2 - y1) * (y2 - y1)).sqrt()
}

/// Shrinks the region until the aligned points are dense enough in their
/// rectangle, first by tightening the angle tolerance and then by cutting the
/// radius; false when nothing dense enough is left.
fn refine_region(
    f: &AngleField,
    used: &mut [bool],
    reg: &mut Vec<RegionPoint>,
    reg_angle: f64,
    prec: f64,
    p: f64,
    rec: &mut LsdRect,
) -> bool {
    let mut density =
        reg.len() as f64 / (dist(rec.x1, rec.y1, rec.x2, rec.y2) * rec.width).max(1e-12);
    if density >= LSD_DENSITY_TH {
        return true;
    }
    let (xc, yc) = (reg[0].x as f64, reg[0].y as f64);
    let ang_c = reg[0].angle;
    let (mut sum, mut s_sum, mut n) = (0.0f64, 0.0f64, 0usize);
    for pt in reg.iter() {
        used[pt.y as usize * f.w + pt.x as usize] = false;
        if dist(xc, yc, pt.x as f64, pt.y as f64) < rec.width {
            let d = angle_diff_signed(pt.angle, ang_c);
            sum += d;
            s_sum += d * d;
            n += 1;
        }
    }
    if n == 0 {
        return false;
    }
    let mean_angle = sum / n as f64;
    let tau = 2.0 * ((s_sum - 2.0 * mean_angle * sum) / n as f64 + mean_angle * mean_angle).sqrt();
    let seed = (reg[0].x as usize, reg[0].y as usize);
    let angle = region_grow(f, used, seed, tau, reg);
    if reg.len() < 2 {
        return false;
    }
    *rec = region_to_rect(reg, angle, prec, p);
    density = reg.len() as f64 / (dist(rec.x1, rec.y1, rec.x2, rec.y2) * rec.width).max(1e-12);
    if density >= LSD_DENSITY_TH {
        return true;
    }
    // cut the region's radius back until it is dense enough
    let mut rad_sq = dist(xc, yc, rec.x1, rec.y1)
        .powi(2)
        .max(dist(xc, yc, rec.x2, rec.y2).powi(2));
    while density < LSD_DENSITY_TH {
        rad_sq *= 0.75 * 0.75;
        let mut i = 0;
        while i < reg.len() {
            let d = (reg[i].x as f64 - xc).powi(2) + (reg[i].y as f64 - yc).powi(2);
            if d > rad_sq {
                used[reg[i].y as usize * f.w + reg[i].x as usize] = false;
                let last = reg.len() - 1;
                reg.swap(i, last);
                reg.pop();
            } else {
                i += 1;
            }
        }
        if reg.len() < 2 {
            return false;
        }
        *rec = region_to_rect(reg, reg_angle, prec, p);
        density = reg.len() as f64 / (dist(rec.x1, rec.y1, rec.x2, rec.y2) * rec.width).max(1e-12);
    }
    true
}

/// Line segments of a grayscale image by LSD, at least `min_len_px` long.
///
/// See the module header for what is and is not taken from the published
/// algorithm. The endpoints are in the input image's pixels, with the half
/// pixel offset and the scale undone the way OpenCV returns them.
pub fn detect_segments(gray: &[u8], w: usize, h: usize, min_len_px: f64) -> Vec<LineSegment> {
    if w < 4 || h < 4 {
        return Vec::new();
    }
    // downscale, which is what makes LSD robust to the quantisation of the
    // gradient direction on sharp edges
    let sigma = LSD_SIGMA_SCALE / LSD_SCALE;
    let hh = (sigma * (2.0 * 3.0 * 10.0f64.ln()).sqrt()).ceil() as usize;
    let ksize = 1 + 2 * hh;
    let src: Vec<f32> = gray.iter().map(|v| *v as f32).collect();
    let blurred = gaussian_blur(&src, w, h, ksize, sigma);
    let sw = imgops::round_half_even(w as f64 * LSD_SCALE) as usize;
    let sh = imgops::round_half_even(h as f64 * LSD_SCALE) as usize;
    if sw < 4 || sh < 4 {
        return Vec::new();
    }
    let bytes: Vec<u8> = blurred
        .iter()
        .map(|v| v.round().clamp(0.0, 255.0) as u8)
        .collect();
    let small = resize_bilinear(&bytes, w, h, sw, sh, 1.0 / LSD_SCALE);

    let prec = std::f64::consts::PI * LSD_ANG_TH / 180.0;
    let p = LSD_ANG_TH / 180.0;
    let threshold = LSD_QUANT / prec.sin();
    let (field, ordered) = ll_angle(&small, sw, sh, threshold);
    // the smallest region that can mean anything under the a-contrario model,
    // which is the one number of that model LSD_REFINE_STD keeps
    let log_nt = 5.0 * ((sw as f64).log10() + (sh as f64).log10()) / 2.0 + 11.0f64.log10();
    let min_reg_size = (-log_nt / p.log10()) as usize;

    let mut used = vec![false; sw * sh];
    let mut reg: Vec<RegionPoint> = Vec::new();
    let mut out = Vec::new();
    for (x, y) in ordered {
        let idx = y * sw + x;
        if used[idx] || field.angles[idx] == NOTDEF {
            continue;
        }
        let reg_angle = region_grow(&field, &mut used, (x, y), prec, &mut reg);
        if reg.len() < min_reg_size {
            continue;
        }
        let mut rec = region_to_rect(&reg, reg_angle, prec, p);
        if !refine_region(&field, &mut used, &mut reg, reg_angle, prec, p, &mut rec) {
            continue;
        }
        let (x0, y0) = ((rec.x1 + 0.5) / LSD_SCALE, (rec.y1 + 0.5) / LSD_SCALE);
        let (x1, y1) = ((rec.x2 + 0.5) / LSD_SCALE, (rec.y2 + 0.5) / LSD_SCALE);
        let length = dist(x0, y0, x1, y1);
        if length >= min_len_px {
            out.push(LineSegment {
                x0,
                y0,
                x1,
                y1,
                length,
                angle_deg: 0.0,
            });
        }
    }
    out
}

// --------------------------------------------------------------------------- line families

/// The vertical and horizontal families of a crop.
///
/// Verticals run bottom to top with `angle_deg` the lean (positive when the top
/// is to the right) and are at least 0.6 m long; horizontals run left to right
/// with `angle_deg` the tilt (positive when rising to the right) and are at
/// least 1 m long. A segment whose midpoint carries an occlusion bit is
/// dropped, because it is not on the wall.
pub fn lsd_families(
    gray: &[u8],
    w: usize,
    h: usize,
    ppm: f64,
    occl: Option<&[u8]>,
) -> (Vec<LineSegment>, Vec<LineSegment>) {
    // the shorter of the two minima decides what the detector has to return
    let segs = detect_segments(gray, w, h, 0.0);
    let mut vert = Vec::new();
    let mut hor = Vec::new();
    for s in segs {
        if let Some(occ) = occl {
            let xm = (s.mid_x() as i64).clamp(0, w as i64 - 1) as usize;
            let ym = (s.mid_y() as i64).clamp(0, h as i64 - 1) as usize;
            if occ[ym * w + xm] & OCCLUDED_BITS != 0 {
                continue;
            }
        }
        // vertical family, bottom to top
        let v = if s.y0 < s.y1 {
            (s.x1, s.y1, s.x0, s.y0)
        } else {
            (s.x0, s.y0, s.x1, s.y1)
        };
        let lean = (v.2 - v.0).atan2(v.1 - v.3).to_degrees();
        if lean.abs() < VERT_FAMILY_DEG && s.length >= MIN_SEG_M * ppm {
            vert.push(LineSegment {
                x0: v.0,
                y0: v.1,
                x1: v.2,
                y1: v.3,
                length: s.length,
                angle_deg: lean,
            });
        }
        // horizontal family, left to right
        let q = if s.x0 > s.x1 {
            (s.x1, s.y1, s.x0, s.y0)
        } else {
            (s.x0, s.y0, s.x1, s.y1)
        };
        let tilt = (q.1 - q.3).atan2(q.2 - q.0).to_degrees();
        if tilt.abs() < HORIZ_FAMILY_DEG && s.length >= HORIZ_MIN_M * ppm {
            hor.push(LineSegment {
                x0: q.0,
                y0: q.1,
                x1: q.2,
                y1: q.3,
                length: s.length,
                angle_deg: tilt,
            });
        }
    }
    (vert, hor)
}

// --------------------------------------------------------------------------- IRLS

/// Huber IRLS of `y = c0 + c1 u`, or of `y = c0` when `fit_slope` is false.
/// Returns `(c0, c1, residuals)`.
fn irls_line(
    u: &[f64],
    y: &[f64],
    w0: &[f64],
    fit_slope: bool,
    delta: f64,
    iters: usize,
) -> (f64, f64, Vec<f64>) {
    let mut w: Vec<f64> = w0.to_vec();
    let mut c0 = if w.iter().sum::<f64>() > 0.0 {
        weighted_mean(y, &w)
    } else {
        0.0
    };
    let mut c1 = 0.0;
    for _ in 0..iters {
        if fit_slope {
            // the 2x2 normal equations of the weighted fit, with the same
            // ridge term the Python adds
            let (mut a00, mut a01, mut a11, mut b0, mut b1) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for i in 0..u.len() {
                a00 += w[i];
                a01 += w[i] * u[i];
                a11 += w[i] * u[i] * u[i];
                b0 += w[i] * y[i];
                b1 += w[i] * u[i] * y[i];
            }
            a00 += 1e-9;
            a11 += 1e-9;
            let det = a00 * a11 - a01 * a01;
            if det.abs() < 1e-300 {
                break;
            }
            c0 = (b0 * a11 - b1 * a01) / det;
            c1 = (a00 * b1 - a01 * b0) / det;
        } else {
            let sw: f64 = w.iter().sum();
            c0 = w.iter().zip(y).map(|(a, b)| a * b).sum::<f64>() / sw.max(1e-12);
        }
        for i in 0..u.len() {
            let r = y[i] - (c0 + c1 * u[i]);
            let hub = if r.abs() <= delta {
                1.0
            } else {
                delta / r.abs().max(1e-9)
            };
            w[i] = w0[i] * hub;
        }
    }
    let res = (0..u.len()).map(|i| y[i] - (c0 + c1 * u[i])).collect();
    (c0, c1, res)
}

// --------------------------------------------------------------------------- lean and keystone

/// The metric `(s, h)` homography of the lean correction, fitted through the
/// four corners of the crop.
pub fn shear_homography(crop: &LooseCrop, c0_deg: f64, c1_deg_per_m: f64) -> [[f64; 3]; 3] {
    let s_cam = crop.x_to_s(crop.x_foot);
    let h_cam = crop.y_to_h(crop.y_cam);
    let s_a = crop.s0;
    let s_b = crop.x_to_s(crop.rgb.width() as f64);
    let src = [
        [s_a, crop.h_bot],
        [s_b, crop.h_bot],
        [s_b, crop.h_top],
        [s_a, crop.h_top],
    ];
    let mut dst = src;
    for k in 0..4 {
        let (s, h) = (src[k][0], src[k][1]);
        let lean = (c0_deg + c1_deg_per_m * (s - s_cam)).to_radians();
        dst[k][0] = s - lean.tan() * (h - h_cam);
    }
    perspective_transform(&src, &dst)
}

/// What the lean fit saw, for the record.
#[derive(Clone, Copy, Debug, Default)]
pub struct LeanStats {
    pub n: usize,
    pub inliers: f64,
    pub rms_deg: f64,
    pub x_spread_m: f64,
}

/// Lean and keystone from the vertical family: IRLS of
/// `lean(x) = c0 + c1 (x - x_foot)` with a 1 degree Huber delta and the segment
/// lengths as weights.
///
/// The keystone term is pinned to zero when the segments do not span 1.5 m,
/// because two nearly coincident columns cannot tell a lean from a keystone.
pub fn lean_keystone(
    crop: &LooseCrop,
    verticals: &[LineSegment],
    params: &Params,
) -> (f64, f64, String, [[f64; 3]; 3], LeanStats) {
    let n = verticals.len();
    let mut stats = LeanStats {
        n,
        ..Default::default()
    };
    if n < 2 {
        return (0.0, 0.0, SHEAR_NONE.into(), IDENTITY3, stats);
    }
    let u: Vec<f64> = verticals
        .iter()
        .map(|s| (s.mid_x() - crop.x_foot) / crop.ppm)
        .collect();
    let y: Vec<f64> = verticals.iter().map(|s| s.angle_deg).collect();
    let w0: Vec<f64> = verticals.iter().map(|s| s.length).collect();
    let um = weighted_mean(&u, &w0);
    let spread = {
        let d: Vec<f64> = u.iter().map(|v| (v - um) * (v - um)).collect();
        weighted_mean(&d, &w0).sqrt()
    };
    stats.x_spread_m = spread;
    let (c0, c1, r) = irls_line(&u, &y, &w0, spread >= MIN_X_SPREAD_M, HUBER_DEG, IRLS_ITERS);
    stats.inliers = r.iter().filter(|v| v.abs() <= LEAN_INLIER_DEG).count() as f64 / n as f64;
    let sq: Vec<f64> = r.iter().map(|v| v * v).collect();
    stats.rms_deg = weighted_mean(&sq, &w0).sqrt();
    if n < params.lean_min_lines as usize {
        return (c0, c1, SHEAR_NONE.into(), IDENTITY3, stats);
    }
    let ok = stats.inliers >= params.lean_min_inliers
        && c0.abs() <= params.lean_max_deg
        && c1.abs() <= params.lean_max_keystone;
    if !ok {
        return (c0, c1, SHEAR_REJECTED.into(), IDENTITY3, stats);
    }
    let h = shear_homography(crop, c0, c1);
    (c0, c1, SHEAR_OK.into(), h, stats)
}

/// The lean from the gradient field alone, the estimator that does not need a
/// line detector: strong near-vertical gradients, a 0.25 degree orientation
/// histogram per column band, then a weighted line fit across the bands.
///
/// It is the fallback when the segments are too few, and the number to compare
/// the segment based one against when they disagree.
pub fn lean_from_gradients(
    gray: &[u8],
    w: usize,
    h: usize,
    x_foot: f64,
    ppm: f64,
    occl: Option<&[u8]>,
) -> (f64, f64, f64) {
    let src: Vec<f32> = gray.iter().map(|v| *v as f32).collect();
    let g = gaussian_blur(&src, w, h, gaussian_ksize(1.0), 1.0);
    let gx = sobel(&g, w, h, 1, 0);
    let gy = sobel(&g, w, h, 0, 1);
    let mag: Vec<f64> = gx
        .iter()
        .zip(&gy)
        .map(|(a, b)| (*a as f64).hypot(*b as f64))
        .collect();
    let mut sorted = mag.clone();
    let thr = imgops::percentile_in_place(&mut sorted, 85.0).max(20.0);
    let theta: Vec<f64> = (0..w * h)
        .map(|i| {
            let sgn = if gx[i] >= 0.0 { 1.0 } else { -1.0 };
            (gy[i] as f64 * sgn)
                .atan2((gx[i] as f64).abs())
                .to_degrees()
        })
        .collect();
    let mut xs = Vec::new();
    let mut th = Vec::new();
    let mut mg = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if mag[i] <= thr || theta[i].abs() > GRAD_MAX_DEG {
                continue;
            }
            if let Some(o) = occl {
                if o[i] & OCCLUDED_BITS != 0 {
                    continue;
                }
            }
            xs.push(x as f64);
            th.push(theta[i]);
            mg.push(mag[i]);
        }
    }
    let support = xs.len() as f64;
    if support < 50.0 {
        return (0.0, 0.0, support);
    }
    let nb = (imgops::round_half_even(w as f64 / (3.0 * ppm)) as usize).clamp(3, 12);
    let n_bins = imgops::round_half_even((2.0 * GRAD_MAX_DEG) / GRAD_BIN_DEG) as usize;
    let (mut band_x, mut band_th, mut band_w) = (Vec::new(), Vec::new(), Vec::new());
    for b in 0..nb {
        let lo = w as f64 * b as f64 / nb as f64;
        let hi = w as f64 * (b + 1) as f64 / nb as f64;
        let sel: Vec<usize> = (0..xs.len())
            .filter(|i| xs[*i] >= lo && xs[*i] < hi)
            .collect();
        if sel.len() < 50 {
            continue;
        }
        let mut hist = vec![0.0f64; n_bins];
        for i in &sel {
            let k = ((th[*i] + GRAD_MAX_DEG) / GRAD_BIN_DEG).floor();
            if k >= 0.0 && (k as usize) < n_bins {
                hist[k as usize] += mg[*i];
            }
        }
        let smooth = convolve_same(&hist, &[0.25, 0.5, 0.25]);
        let k = argmax_first(smooth.iter().copied());
        let centre = -GRAD_MAX_DEG + k as f64 * GRAD_BIN_DEG + 0.5 * GRAD_BIN_DEG;
        let near: Vec<usize> = sel
            .into_iter()
            .filter(|i| (th[*i] - centre).abs() <= 1.0)
            .collect();
        if near.len() < 20 {
            continue;
        }
        let ww: Vec<f64> = near.iter().map(|i| mg[*i]).collect();
        band_th.push(weighted_mean(
            &near.iter().map(|i| th[*i]).collect::<Vec<_>>(),
            &ww,
        ));
        band_x.push(weighted_mean(
            &near.iter().map(|i| xs[*i]).collect::<Vec<_>>(),
            &ww,
        ));
        band_w.push(ww.iter().sum::<f64>());
    }
    if band_th.is_empty() {
        return (0.0, 0.0, support);
    }
    let u: Vec<f64> = band_x.iter().map(|x| (x - x_foot) / ppm).collect();
    let spread = if u.len() > 1 {
        let um = weighted_mean(&u, &band_w);
        let d: Vec<f64> = u.iter().map(|v| (v - um) * (v - um)).collect();
        weighted_mean(&d, &band_w).sqrt()
    } else {
        0.0
    };
    let (c0, c1, _) = irls_line(
        &u,
        &band_th,
        &band_w,
        u.len() >= 3 && spread >= MIN_X_SPREAD_M,
        0.5,
        5,
    );
    (c0, c1, support)
}

/// Applies the metric shear to a crop, keeping its pixel grid.
///
/// `H_px = T H T^-1` with `T` the crop's `(s, h)` to `(x, y)` map, then an
/// inverse bilinear warp with the border replicated, which is what
/// `cv2.warpPerspective` does.
fn warp_crop(crop: &LooseCrop, h_metric: &[[f64; 3]; 3]) -> (RgbImage, Vec<u8>) {
    let t = [
        [crop.ppm, 0.0, -crop.s0 * crop.ppm],
        [0.0, -crop.ppm, crop.h_top * crop.ppm],
        [0.0, 0.0, 1.0],
    ];
    let h_px = mat3_mul(&mat3_mul(&t, h_metric), &mat3_inv(&t));
    let inv = mat3_inv(&h_px);
    let (w, h) = (crop.rgb.width(), crop.rgb.height());
    let mut rgb = RgbImage::new(w, h);
    let mut occl = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = apply_h(&inv, x as f64, y as f64);
            let cx = sx.clamp(0.0, w as f64 - 1.0);
            let cy = sy.clamp(0.0, h as f64 - 1.0);
            let x0 = cx.floor() as u32;
            let y0 = cy.floor() as u32;
            let x1 = (x0 + 1).min(w - 1);
            let y1 = (y0 + 1).min(h - 1);
            let tx = cx - x0 as f64;
            let ty = cy - y0 as f64;
            let mut px = [0u8; 3];
            for (c, out) in px.iter_mut().enumerate() {
                let a = crop.rgb.get_pixel(x0, y0).0[c] as f64 * (1.0 - tx)
                    + crop.rgb.get_pixel(x1, y0).0[c] as f64 * tx;
                let b = crop.rgb.get_pixel(x0, y1).0[c] as f64 * (1.0 - tx)
                    + crop.rgb.get_pixel(x1, y1).0[c] as f64 * tx;
                *out = (a * (1.0 - ty) + b * ty).round().clamp(0.0, 255.0) as u8;
            }
            rgb.put_pixel(x, y, Rgb(px));
            // nearest neighbour, because an occlusion bit field must not be
            // interpolated
            let nx = cx.round().clamp(0.0, w as f64 - 1.0) as u32;
            let ny = cy.round().clamp(0.0, h as f64 - 1.0) as u32;
            occl[(y * w + x) as usize] = crop.occl[(ny * w + nx) as usize];
        }
    }
    (rgb, occl)
}

// --------------------------------------------------------------------------- plane gate

/// Whether the horizontal edges agree with the plane the crop was rectified
/// onto: their tilt should not grow with height above the camera.
///
/// This is a detector and not a correction. `box` is the OSM rectangle in crop
/// pixels and only its inner 80 per cent counts, since the edges of a loose
/// crop are the neighbours.
pub fn plane_gate(
    crop: &LooseCrop,
    horizontals: &[LineSegment],
    bbox: (f64, f64, f64, f64),
    params: &Params,
) -> (f64, String, usize) {
    if horizontals.is_empty() {
        return (0.0, UNVERIFIED.into(), 0);
    }
    let (x0, x1, y0, y1) = bbox;
    let (wx, wy) = (x1 - x0, y1 - y0);
    let keep: Vec<&LineSegment> = horizontals
        .iter()
        .filter(|s| {
            let xm = s.mid_x();
            let ym = s.mid_y();
            xm >= x0 + 0.1 * wx
                && xm <= x1 - 0.1 * wx
                && ym >= y0 + 0.1 * wy
                && ym <= y1 - 0.1 * wy
                && (ym - crop.y_cam).abs() >= crop.ppm
                && s.length >= HORIZ_MIN_M * crop.ppm
        })
        .collect();
    let n = keep.len();
    if n == 0 {
        return (0.0, UNVERIFIED.into(), 0);
    }
    let total_m: f64 = keep.iter().map(|s| s.length).sum::<f64>() / crop.ppm;
    let hh: Vec<f64> = keep
        .iter()
        .map(|s| (crop.y_cam - s.mid_y()) / crop.ppm)
        .collect();
    let tilt: Vec<f64> = keep.iter().map(|s| s.angle_deg).collect();
    let base: Vec<f64> = keep.iter().map(|s| s.length).collect();
    let mut w = base.clone();
    let mut s = 0.0;
    for _ in 0..IRLS_ITERS {
        let num: f64 = (0..n).map(|i| w[i] * tilt[i] * hh[i]).sum();
        let den: f64 = (0..n).map(|i| w[i] * hh[i] * hh[i]).sum();
        s = num / den.max(1e-9);
        for i in 0..n {
            let r = tilt[i] - s * hh[i];
            w[i] = base[i]
                * if r.abs() <= HUBER_DEG {
                    1.0
                } else {
                    HUBER_DEG / r.abs().max(1e-9)
                };
        }
    }
    if n < GATE_MIN_LINES || total_m < GATE_MIN_TOTAL_M {
        return (s, UNVERIFIED.into(), n);
    }
    if s.abs() <= params.plane_gate_deg_per_m {
        (s, ON_PLANE.into(), n)
    } else {
        (s, PLANE_MISMATCH.into(), n)
    }
}

// --------------------------------------------------------------------------- sky, roof, ground

/// The magnitude and the vertical part of the per channel maximum Sobel
/// response. A sky against a wall of the same brightness but a different hue
/// still shows up, which a grey gradient would miss.
fn colour_gradient(img: &RgbImage) -> (Vec<f32>, Vec<f32>) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut mag = vec![0.0f32; w * h];
    let mut gyy = vec![0.0f32; w * h];
    for c in 0..3 {
        let plane: Vec<f32> = img.pixels().map(|p| p.0[c] as f32).collect();
        let gx = sobel(&plane, w, h, 1, 0);
        let gy = sobel(&plane, w, h, 0, 1);
        for i in 0..w * h {
            mag[i] = mag[i].max(gx[i].hypot(gy[i]));
            gyy[i] = gyy[i].max(gy[i].abs());
        }
    }
    (mag, gyy)
}

/// The HSV sky rule: a blue enough hue with some saturation, or bright and
/// nearly grey, which is an overcast sky.
fn sky_rule(hsv: &[[u8; 3]]) -> Vec<bool> {
    hsv.iter()
        .map(|p| {
            let (hh, s, v) = (p[0] as i32, p[1] as i32, p[2] as i32);
            (hh >= SKY_H_RANGE.0 && hh <= SKY_H_RANGE.1 && s > 30) || (v > 150 && s < 60)
        })
        .collect()
}

/// Otsu over the top rows, keeping the class that holds most of row 0.
///
/// When that class is itself clearly bimodal, which is a wall nearly as bright
/// as the sky above it, it is split once more and only row 0's part survives.
fn otsu_sky_side(v_plane: &[u8], w: usize, h: usize, region_rows: usize) -> Vec<bool> {
    let region = &v_plane[..region_rows * w];
    let thr = otsu_threshold(&histogram(region.iter().copied()));
    let hi: Vec<bool> = v_plane.iter().map(|v| *v as f64 > thr).collect();
    let row0_share = hi[..w].iter().filter(|b| **b).count() as f64 / w as f64;
    let mut side: Vec<bool> = if row0_share >= 0.5 {
        hi.clone()
    } else {
        hi.iter().map(|b| !*b).collect()
    };
    let vals: Vec<u8> = region
        .iter()
        .enumerate()
        .filter(|(i, _)| side[*i])
        .map(|(_, v)| *v)
        .collect();
    if vals.len() < 100 {
        return side;
    }
    let (vmin, vmax) = (
        *vals.iter().min().unwrap_or(&0),
        *vals.iter().max().unwrap_or(&0),
    );
    if vmin == vmax {
        return side;
    }
    let thr2 = otsu_threshold(&histogram(vals.iter().copied()));
    let c1: Vec<bool> = vals.iter().map(|v| *v as f64 > thr2).collect();
    let w1 = c1.iter().filter(|b| **b).count() as f64 / c1.len() as f64;
    if !(0.15..=0.85).contains(&w1) {
        return side;
    }
    let m1 = mean(
        &vals
            .iter()
            .zip(&c1)
            .filter(|(_, k)| **k)
            .map(|(v, _)| *v as f64)
            .collect::<Vec<_>>(),
    );
    let m2 = mean(
        &vals
            .iter()
            .zip(&c1)
            .filter(|(_, k)| !**k)
            .map(|(v, _)| *v as f64)
            .collect::<Vec<_>>(),
    );
    let mu = mean(&vals.iter().map(|v| *v as f64).collect::<Vec<_>>());
    let var = mean(
        &vals
            .iter()
            .map(|v| (*v as f64 - mu) * (*v as f64 - mu))
            .collect::<Vec<_>>(),
    );
    let sep = w1 * (1.0 - w1) * (m1 - m2) * (m1 - m2) / var.max(1e-9);
    if (m1 - m2).abs() <= 25.0 || sep <= 0.8 {
        return side;
    }
    let hi2: Vec<bool> = v_plane.iter().map(|v| *v as f64 > thr2).collect();
    let a = (0..w).filter(|x| hi2[*x] && side[*x]).count();
    let b = (0..w).filter(|x| !hi2[*x] && side[*x]).count();
    let part: Vec<bool> = if a >= b {
        hi2
    } else {
        hi2.iter().map(|v| !*v).collect()
    };
    for i in 0..w * h {
        side[i] = side[i] && part[i];
    }
    side
}

/// The sky mask: the flat, sky coloured components that touch the top row.
///
/// The colour rule is replaced by Otsu on V when it marks almost nothing at the
/// top of the crop or when it marks most of the rows that must be facade, which
/// is what a white rendered wall under a white sky does to it.
pub fn sky_mask(
    img: &RgbImage,
    facade_rows: Option<(f64, f64)>,
) -> (Mask, &'static str, Vec<bool>) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let (mag, _) = colour_gradient(img);
    let smooth = gaussian_blur(&mag, w, h, 5, 0.0);
    let flat: Vec<bool> = smooth.iter().map(|v| *v < SKY_SOBEL_MAX).collect();
    let hsv = rgb_to_hsv(img);
    let rule = sky_rule(&hsv);
    let mut cand: Vec<bool> = (0..w * h).map(|i| flat[i] && rule[i]).collect();
    let n_top = (imgops::round_half_even(SKY_TOP_FRAC * h as f64) as usize).max(1);
    let frac_top = cand[..(n_top * w).min(cand.len())]
        .iter()
        .filter(|b| **b)
        .count() as f64
        / (n_top * w) as f64;
    let mut over = false;
    let mut y_lim = (0.5 * h as f64) as usize;
    if let Some((a, b)) = facade_rows {
        let ya = (a.min(b).max(0.0)) as usize;
        let yb = (a.max(b)).min(h as f64) as usize;
        if yb > ya {
            let band = &rule[ya * w..yb * w];
            over = band.iter().filter(|v| **v).count() as f64 / band.len() as f64 > SKY_RULE_MAX;
            y_lim = yb;
        }
    }
    let mut mode = "rule";
    let mut colour = rule;
    if frac_top < SKY_RULE_MIN || over {
        let v_plane: Vec<u8> = hsv.iter().map(|p| p[2]).collect();
        let rows = y_lim.max(n_top + 1).min(h);
        colour = otsu_sky_side(&v_plane, w, h, rows);
        cand = (0..w * h).map(|i| flat[i] && colour[i]).collect();
        mode = "otsu";
    }
    let mask = Mask::from_bits(w, h, cand);
    let labels = imgops::connected_components(&mask);
    let mut top: Vec<u32> = (0..w)
        .filter(|x| mask.at(*x, 0))
        .map(|x| labels.at(x, 0))
        .filter(|l| *l > 0)
        .collect();
    top.sort_unstable();
    top.dedup();
    let bits: Vec<bool> = labels
        .labels
        .iter()
        .map(|l| top.binary_search(l).is_ok())
        .collect();
    (Mask::from_bits(w, h, bits), mode, colour)
}

/// Horizontal edge energy per row over the given columns, smoothed over three
/// rows. Occluded pixels do not contribute.
pub fn row_energy(img: &RgbImage, occl: Option<&[u8]>, cols: Option<&[usize]>) -> Vec<f64> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let (_, gyy) = colour_gradient(img);
    let mut prof = vec![0.0f64; h];
    let all: Vec<usize> = (0..w).collect();
    let cols = cols.unwrap_or(&all);
    for (y, p) in prof.iter_mut().enumerate() {
        let (mut num, mut den) = (0.0f64, 0usize);
        for c in cols {
            let i = y * w + c;
            let free = occl.map(|o| o[i] & OCCLUDED_BITS == 0).unwrap_or(true);
            if occl.is_some() {
                if free {
                    num += gyy[i] as f64 / 8.0;
                    den += 1;
                }
            } else {
                num += gyy[i] as f64 / 8.0;
                den += 1;
            }
        }
        *p = if occl.is_some() {
            num / den.max(1) as f64
        } else {
            num / cols.len().max(1) as f64
        };
    }
    convolve_same(&prof, &[0.25, 0.5, 0.25])
}

/// The strongest row of `energy` in `[r0, r1)`, if it stands out enough against
/// the profile's median.
///
/// `seen` restricts the rows the median is measured on. A row the camera never
/// covered carries no energy by construction, so in a crop that is mostly
/// no-data the median over every row is zero and only the absolute floor is
/// left, which lets any edge through (see [`nodata_columns`]).
fn strongest_row(
    energy: &[f64],
    r0: isize,
    r1: isize,
    ratio: f64,
    seen: Option<&[bool]>,
) -> Option<usize> {
    let r0 = r0.max(0) as usize;
    let r1 = (r1.max(0) as usize).min(energy.len());
    if r1 <= r0 {
        return None;
    }
    let best = r0 + argmax_first(energy[r0..r1].iter().copied());
    let kept: Vec<f64> = match seen {
        Some(s) if s.iter().any(|v| *v) => energy
            .iter()
            .zip(s)
            .filter(|(_, keep)| **keep)
            .map(|(e, _)| *e)
            .collect(),
        _ => energy.to_vec(),
    };
    let reference = (ratio * imgops::median(&kept)).max(EDGE_ENERGY_FLOOR);
    if energy[best] >= reference {
        Some(best)
    } else {
        None
    }
}

/// The sub-pixel position of an edge from the energy centroid over three rows.
///
/// A step between rows `r` and `r + 1` excites both equally, so the centroid at
/// `r + 0.5` in row-centre units is the boundary `y = r + 1`.
pub fn edge_y(energy: &[f64], row: usize) -> f64 {
    let r0 = row.saturating_sub(1);
    let r1 = (row + 2).min(energy.len());
    let s: f64 = energy[r0..r1].iter().sum();
    if s <= 0.0 {
        return row as f64 + 0.5;
    }
    let num: f64 = (r0..r1).map(|i| energy[i] * (i as f64 + 0.5)).sum();
    num / s
}

/// Per column mean CIE Lab over the given rows and the unoccluded pixels, with
/// empty columns interpolated from their neighbours.
fn column_lab(
    img: &RgbImage,
    occl: Option<&[u8]>,
    x0: usize,
    x1: usize,
    rows: (f64, f64),
) -> Vec<[f64; 3]> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    // the Python clamps each end on its own and falls back to the whole crop
    // when they cross, so an inverted pair means "all rows", not "no rows"
    let mut r0 = rows.0.max(0.0) as usize;
    let mut r1 = if rows.1 < 0.0 {
        0
    } else {
        (rows.1 as usize).min(h)
    };
    if r1 <= r0 {
        r0 = 0;
        r1 = h;
    }
    let cols = x1 - x0;
    let mut out = vec![[0.0f64; 3]; cols];
    let mut den = vec![0.0f64; cols];
    for y in r0..r1 {
        for (j, o) in out.iter_mut().enumerate() {
            let i = y * w + x0 + j;
            let free = occl.map(|oc| oc[i] & OCCLUDED_BITS == 0).unwrap_or(true);
            if !free {
                continue;
            }
            let lab = rgb_to_lab(img.get_pixel((x0 + j) as u32, y as u32).0);
            for c in 0..3 {
                o[c] += lab[c];
            }
            den[j] += 1.0;
        }
    }
    for (j, o) in out.iter_mut().enumerate() {
        for v in o.iter_mut() {
            *v /= den[j].max(1e-6);
        }
    }
    let good: Vec<usize> = (0..cols).filter(|j| den[*j] >= 1.0).collect();
    if !good.is_empty() && good.len() < cols {
        let gx: Vec<f64> = good.iter().map(|j| *j as f64).collect();
        for c in 0..3 {
            let gy: Vec<f64> = good.iter().map(|j| out[*j][c]).collect();
            for (j, o) in out.iter_mut().enumerate() {
                if den[j] < 1.0 {
                    o[c] = interp(j as f64, &gx, &gy);
                }
            }
        }
    }
    out
}

/// A roof-like edge below the sky boundary: the strongest horizontal edge in
/// the band with a colour change across it.
///
/// This is what tells this building's roof from the facade of a taller building
/// standing behind it (CRITIQUE C21).
fn roof_edge_below(crop: &LooseCrop, energy: &[f64], cols: &[usize], h_sky: f64) -> Option<f64> {
    let lo = (0.4 * h_sky).max(3.0);
    let hi = h_sky - 0.75;
    if hi - lo < 1.0 {
        return None;
    }
    let r0 = (crop.h_to_y(hi) as isize).max(0) as usize;
    let r1 = ((crop.h_to_y(lo) as isize).max(0) as usize).min(energy.len());
    if r1 < r0 + 3 {
        return None;
    }
    let band = &energy[r0..r1];
    let best = r0 + argmax_first(band.iter().copied());
    if energy[best] < (GROUND_EDGE_RATIO * imgops::median(band)).max(EDGE_ENERGY_FLOOR) {
        return None;
    }
    let h_e = crop.y_to_h(edge_y(energy, best));
    let px = imgops::round_half_even(crop.ppm) as isize;
    let sub = sub_columns(&crop.rgb, cols);
    let occ = sub_columns_occl(&crop.occl, crop.rgb.width() as usize, cols);
    let above = column_lab(
        &sub,
        Some(&occ),
        0,
        cols.len(),
        ((best as isize - px - 2) as f64, (best as isize - 2) as f64),
    );
    let below = column_lab(
        &sub,
        Some(&occ),
        0,
        cols.len(),
        ((best + 3) as f64, (best as isize + px + 3) as f64),
    );
    let de = lab_distance(&median_lab(&above), &median_lab(&below));
    if de > COLOUR_MAX_DE {
        Some(h_e)
    } else {
        None
    }
}

/// The per channel median of a column-wise Lab array.
fn median_lab(v: &[[f64; 3]]) -> [f64; 3] {
    let mut out = [0.0; 3];
    for (c, o) in out.iter_mut().enumerate() {
        let mut ch: Vec<f64> = v.iter().map(|p| p[c]).collect();
        *o = if ch.is_empty() {
            0.0
        } else {
            imgops::median_in_place(&mut ch)
        };
    }
    out
}

fn lab_distance(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The sub-image of the given columns, which is `img[:, cols]`.
fn sub_columns(img: &RgbImage, cols: &[usize]) -> RgbImage {
    let h = img.height();
    let mut out = RgbImage::new(cols.len().max(1) as u32, h);
    for y in 0..h {
        for (j, c) in cols.iter().enumerate() {
            out.put_pixel(j as u32, y, *img.get_pixel(*c as u32, y));
        }
    }
    out
}

fn sub_columns_occl(occl: &[u8], w: usize, cols: &[usize]) -> Vec<u8> {
    let h = occl.len() / w.max(1);
    let mut out = vec![0u8; cols.len() * h];
    for y in 0..h {
        for (j, c) in cols.iter().enumerate() {
            out[y * cols.len() + j] = occl[y * w + c];
        }
    }
    out
}

/// The roofline of one view.
#[derive(Clone, Debug)]
pub struct Roofline {
    pub h: Option<f64>,
    pub flag: String,
    pub spread_m: f64,
    pub flags: Vec<String>,
}

/// Whether a column's sky is the crop's own no-data region rather than sky.
///
/// A perspective camera covers only part of the rectangle; the rest is sampled
/// with a constant border, comes back black and carries `OCC_OUTSIDE`. That
/// region is smooth and dark, so the Otsu branch of the sky mask takes it for
/// sky and the "roofline" is reported where the visible masonry stops, which is
/// the edge of what the camera saw and not a measurement of the building. A
/// column is rejected when more than `ROOF_NODATA_MAX` of the
/// `ROOF_NODATA_BAND_M` metres of sky directly above its boundary carries the
/// bit: the boundary rests on nothing. Panoramas never carry `OCC_OUTSIDE`
/// (their maps wrap), so this cannot reach them.
fn nodata_column(crop: &LooseCrop, col: usize, last: usize) -> bool {
    let w = crop.rgb.width() as usize;
    // half to even, which is what `numpy.round` does on the tie
    let k = (imgops::round_half_even(ROOF_NODATA_BAND_M * crop.ppm) as usize).max(1);
    let r1 = last + 1;
    let r0 = r1.saturating_sub(k);
    if r1 <= r0 {
        return false;
    }
    let n = (r0..r1)
        .filter(|y| crop.occl[y * w + col] & OCC_OUTSIDE != 0)
        .count();
    n as f64 / (r1 - r0) as f64 > ROOF_NODATA_MAX
}

/// The roofline from the sky mask, with the cloud and a roof-like edge as the
/// two ways of confirming it.
///
/// Per column the last sky row is refined to the sub-pixel centroid of the
/// colour Sobel response just below it, which undoes the smoothness erosion of
/// the mask and the resampling blur; the 30th percentile over the columns is
/// the roofline, low enough to ignore chimneys and aerials. A column whose
/// boundary rests on the crop's own no-data does not reach sky
/// ([`nodata_column`]); `ROOF_NODATA` is flagged when every column that reached
/// was dropped for it, so the view saw no sky at all above this wall.
#[allow(clippy::too_many_arguments)]
pub fn sky_roofline(
    crop: &LooseCrop,
    sky: &Mask,
    s_range: (f64, f64),
    h_osm: Option<f64>,
    height_source_default: bool,
    h_cloud: Option<f64>,
    params: &Params,
) -> Roofline {
    let (w, h) = (crop.rgb.width() as usize, crop.rgb.height() as usize);
    let mut cols: Vec<usize> = (0..w)
        .filter(|x| {
            let s = crop.x_to_s(*x as f64 + 0.5);
            s >= s_range.0 && s <= s_range.1
        })
        .collect();
    if cols.is_empty() {
        cols = (0..w).collect();
    }
    let (_, gyy) = colour_gradient(&crop.rgb);
    let mut h_cols: Vec<f64> = Vec::new();
    let mut h_dropped: Vec<f64> = Vec::new();
    let mut reached = 0usize;
    for c in &cols {
        let last = (0..h).rev().find(|y| sky.at(*c, *y));
        let Some(r0) = last else { continue };
        reached += 1;
        let lo = r0.saturating_sub(1);
        let hi = (r0 + 6).min(h);
        let mut best = None;
        let mut best_v = f32::NEG_INFINITY;
        for y in lo..hi {
            let v = gyy[y * w + c];
            if v > best_v {
                best_v = v;
                best = Some(y);
            }
        }
        let mut y_edge = match best {
            Some(rm) if best_v >= 16.0 => {
                let column: Vec<f64> = (0..h).map(|y| gyy[y * w + c] as f64).collect();
                edge_y(&column, rm)
            }
            _ => r0 as f64 + 1.0,
        };
        y_edge = y_edge.clamp(r0 as f64, r0 as f64 + 6.0);
        if nodata_column(crop, *c, r0) {
            h_dropped.push(crop.y_to_h(y_edge));
        } else {
            h_cols.push(crop.y_to_h(y_edge));
        }
    }
    let blind = reached > 0 && h_dropped.len() == reached;
    let reach = h_cols.len() as f64 / cols.len() as f64;
    let mut flags: Vec<String> = if blind {
        vec![ROOF_NODATA.into()]
    } else {
        Vec::new()
    };
    let h_def = h_osm
        .filter(|v| *v != 0.0)
        .unwrap_or(params.default_height_m);
    let default = height_source_default || h_osm.filter(|v| *v != 0.0).is_none();
    let mut spread = 0.0;
    let mut h_sky = None;
    if !h_cols.is_empty() {
        let mut buf = h_cols.clone();
        h_sky = Some(imgops::percentile_in_place(&mut buf, 30.0));
        let p90 = imgops::percentile_in_place(&mut buf, 90.0);
        let p10 = imgops::percentile_in_place(&mut buf, 10.0);
        spread = p90 - p10;
    }
    let energy = row_energy(&crop.rgb, Some(&crop.occl), Some(&cols));
    let mut accepted = false;
    if let Some(hs) = h_sky {
        if reach >= ROOF_MIN_REACH && spread < params.roof_spread_m.max(ROOF_SPREAD_FRAC * hs) {
            let (lo, hi) = if default {
                ROOF_DEFAULT_RANGE
            } else {
                (params.roof_ratio[0] * h_def, params.roof_ratio[1] * h_def)
            };
            accepted = hs >= lo && hs <= hi;
        }
    }
    // A blind view still says where its picture stopped, and everything below that
    // is masonry it did see, so it can still contradict a cloud that ends well
    // under it.
    let h_behind =
        h_sky.or_else(|| blind.then(|| imgops::percentile_in_place(&mut h_dropped.clone(), 30.0)));
    if let (Some(hb), Some(hc)) = (h_behind, h_cloud) {
        if hb > hc + ROOF_BEHIND_M {
            flags.push(ROOF_BEHIND.into());
        }
    }
    if accepted && default {
        let hs = h_sky.unwrap_or(0.0);
        let cloud_ok = h_cloud.is_some_and(|hc| (hs - hc).abs() <= ROOF_BEHIND_M);
        if !cloud_ok {
            if let Some(h_edge) = roof_edge_below(crop, &energy, &cols, hs) {
                for f in [ROOF_BEHIND, ROOF_UNCONFIRMED] {
                    if !flags.iter().any(|x| x == f) {
                        flags.push(f.into());
                    }
                }
                return Roofline {
                    h: Some(h_edge),
                    flag: ROOF_EDGE.into(),
                    spread_m: spread,
                    flags,
                };
            }
            if h_cloud.is_none() {
                flags.push(ROOF_UNCONFIRMED.into());
            }
        }
    }
    if accepted {
        return Roofline {
            h: h_sky,
            flag: ROOF_SKY.into(),
            spread_m: spread,
            flags,
        };
    }
    let r_hi = crop.h_to_y(ROOF_EDGE_WINDOW.1 * h_def) as isize;
    let r_lo = crop.h_to_y(ROOF_EDGE_WINDOW.0 * h_def) as isize;
    let seen: Vec<bool> = (0..h)
        .map(|y| {
            let n = cols
                .iter()
                .filter(|c| crop.occl[y * w + **c] & OCC_OUTSIDE != 0)
                .count();
            n as f64 / cols.len() as f64 <= ROOF_NODATA_MAX
        })
        .collect();
    if let Some(best) = strongest_row(&energy, r_hi, r_lo + 1, ROOF_EDGE_RATIO, Some(&seen)) {
        return Roofline {
            h: Some(crop.y_to_h(edge_y(&energy, best))),
            flag: ROOF_EDGE.into(),
            spread_m: spread,
            flags,
        };
    }
    Roofline {
        h: None,
        flag: ROOF_OSM.into(),
        spread_m: spread,
        flags,
    }
}

/// The ground row: the strongest horizontal edge within 0.75 m of where the
/// cloud puts the wall foot. Its `dz` moves the base every other height in the
/// refinement is measured from (CRITIQUE B15).
pub fn ground_row(crop: &LooseCrop, z_g_row: f64, ground_source: GroundSource) -> (f64, String) {
    let energy = row_energy(&crop.rgb, Some(&crop.occl), None);
    let win = imgops::round_half_even(GROUND_WINDOW_M * crop.ppm) as isize;
    let r = imgops::round_half_even(z_g_row) as isize;
    if let Some(best) = strongest_row(&energy, r - win, r + win + 1, GROUND_EDGE_RATIO, None) {
        return (crop.y_to_h(edge_y(&energy, best)), GROUND_EDGE.into());
    }
    (
        0.0,
        if matches!(ground_source, GroundSource::Cloud) {
            GROUND_CLOUD.into()
        } else {
            GROUND_DEFAULT.into()
        },
    )
}

// --------------------------------------------------------------------------- column profile

/// The column energy profile and its peaks.
///
/// `E(x) = P(x) / median(P) + 2 C(x)` at [`PROFILE_PPM`] columns per metre,
/// where `P` is vertical line length per column spread with a 0.2 m sigma and
/// `C` the normalised rate of colour change along the wall. A building corner
/// shows in both.
pub fn column_profile(
    crop: &LooseCrop,
    verticals: &[LineSegment],
    box_rows: (f64, f64),
) -> (Vec<f32>, Vec<Peak>) {
    let w = crop.rgb.width() as usize;
    let s_a = crop.s0;
    let s_b = crop.x_to_s(w as f64);
    let n = (imgops::round_half_even((s_b - s_a) * PROFILE_PPM) as usize).max(2);
    let xs: Vec<f64> = (0..n)
        .map(|i| s_a + (i as f64 + 0.5) / PROFILE_PPM)
        .collect();
    let mut p = vec![0.0f64; n];
    let (row_lo, row_hi) = (box_rows.0.min(box_rows.1), box_rows.0.max(box_rows.1));
    for seg in verticals {
        let ym = seg.mid_y();
        if ym < row_lo || ym > row_hi {
            continue;
        }
        let s_mid = crop.x_to_s(seg.mid_x());
        let k =
            imgops::round_half_even((s_mid - s_a) * PROFILE_PPM - 0.5).clamp(0.0, n as f64 - 1.0);
        p[k as usize] += seg.length / crop.ppm;
    }
    let sig = PROFILE_SIGMA_M * PROFILE_PPM;
    let p = gaussian_blur_1d(&p, sig);
    let lab = column_lab(&crop.rgb, Some(&crop.occl), 0, w, box_rows);
    let src_x: Vec<f64> = (0..w).map(|i| crop.x_to_s(i as f64 + 0.5)).collect();
    let mut lab_s = [Vec::new(), Vec::new(), Vec::new()];
    for c in 0..3 {
        let ys: Vec<f64> = lab.iter().map(|l| l[c]).collect();
        let resampled: Vec<f64> = xs.iter().map(|x| interp(*x, &src_x, &ys)).collect();
        lab_s[c] = gaussian_blur_1d(&resampled, sig);
    }
    let mut c_raw = vec![0.0f64; n];
    for channel in &lab_s {
        let g = gradient(channel);
        for (i, v) in c_raw.iter_mut().enumerate() {
            *v += (g[i] * PROFILE_PPM).powi(2);
        }
    }
    for v in &mut c_raw {
        *v = v.sqrt();
    }
    let mut sorted = c_raw.clone();
    let p99 = imgops::percentile_in_place(&mut sorted, 99.0).max(1e-6);
    let c_norm: Vec<f64> = c_raw.iter().map(|v| (v / p99).clamp(0.0, 1.0)).collect();
    let p_max = p.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let norm = imgops::median(&p).max(0.1 * p_max).max(0.3);
    let e: Vec<f32> = (0..n)
        .map(|i| (p[i] / norm + 2.0 * c_norm[i]) as f32)
        .collect();
    let distance = ((PEAK_MIN_SEP_M * PROFILE_PPM) as usize).max(1);
    let half = imgops::round_half_even(2.0 * sig) as usize;
    let peaks = find_peaks(&e, PEAK_MIN_E, distance)
        .into_iter()
        .map(|i| Peak {
            x_m: xs[i],
            energy: e[i] as f64,
            vlen_m: p[i.saturating_sub(half)..(i + half + 1).min(n)]
                .iter()
                .sum(),
        })
        .collect();
    (e, peaks)
}

/// The first autocorrelation peak of a signal, in metres, inside the range a
/// facade rhythm can plausibly have.
fn autocorr_period(signal: &[f64], ppm: f64) -> Option<f64> {
    let n = signal.len();
    if n < 4 {
        return None;
    }
    let m = mean(signal);
    let x: Vec<f64> = signal.iter().map(|v| v - m).collect();
    let var = mean(&x.iter().map(|v| v * v).collect::<Vec<_>>());
    if var.sqrt() < 1e-6 {
        return None;
    }
    let mut ac = vec![0.0f64; n];
    for (lag, a) in ac.iter_mut().enumerate() {
        *a = (0..n - lag).map(|i| x[i] * x[i + lag]).sum();
    }
    let a0 = ac[0].max(1e-9);
    for a in &mut ac {
        *a /= a0;
    }
    let l0 = (RHYTHM_RANGE_M.0 * ppm) as usize;
    let l1 = ((RHYTHM_RANGE_M.1 * ppm) as usize).min(n - 1);
    if l1 <= l0 + 2 {
        return None;
    }
    let seg: Vec<f32> = ac[l0..=l1].iter().map(|v| *v as f32).collect();
    let pk = find_peaks(&seg, RHYTHM_MIN_PEAK, 1);
    pk.first().map(|i| (i + l0) as f64 / ppm)
}

/// Whether the two halves of the candidate rectangle look like one building:
/// the colour and the window rhythm of the left half against the right.
fn half_consistency(
    rgb: &RgbImage,
    occl: Option<&[u8]>,
    c0: isize,
    c1: isize,
    ppm: f64,
) -> (bool, f64) {
    let w = rgb.width() as usize;
    let c0 = c0.max(0) as usize;
    let c1 = (c1.max(0) as usize).min(w);
    if c1 <= c0 || ((c1 - c0) as f64) < 4.0 * ppm {
        return (false, 0.0);
    }
    let lab = column_lab(rgb, occl, c0, c1, (0.0, rgb.height() as f64));
    let mid = (c1 - c0) / 2;
    let ml = median_lab(&lab[..mid]);
    let mr = median_lab(&lab[mid..]);
    let de = lab_distance(&ml, &mr);
    let pl = autocorr_period(&lab[..mid].iter().map(|l| l[0]).collect::<Vec<_>>(), ppm);
    let pr = autocorr_period(&lab[mid..].iter().map(|l| l[0]).collect::<Vec<_>>(), ppm);
    let mut fires = de > COLOUR_MAX_DE;
    if let (Some(a), Some(b)) = (pl, pr) {
        if (a - b).abs() / a.max(b) > RHYTHM_MAX_DIFF {
            fires = true;
        }
    }
    (fires, de)
}

/// Where one view puts the two ends of the wall.
#[derive(Clone, Debug)]
pub struct Extent {
    pub s_l: Option<f64>,
    pub s_r: Option<f64>,
    pub src_a: String,
    pub src_b: String,
    pub flags: Vec<String>,
}

/// The joint extent solve over both ends.
///
/// Candidate peaks near each provisional end, then the pair that maximises
/// `E_l + E_r - |(x_r - x_l) - L_osm| / lambda`, so the OSM length is a
/// constraint on the pair rather than a hard bound on either end. With only one
/// end found the other is translated by the same amount, never resized, which
/// is what keeps a facade the width OSM says it is when only one corner is
/// visible.
#[allow(clippy::too_many_arguments)]
pub fn extent_joint(
    peaks: &[Peak],
    x_a: f64,
    x_b: f64,
    l_osm: f64,
    crop: Option<(&RgbImage, Option<&[u8]>, f64, f64)>,
    fix_a: bool,
    fix_b: bool,
    params: &Params,
) -> Extent {
    let win = params.extent_window_m;
    let lam = params.extent_lambda.max(1e-6);
    let candidates = |x_end: f64| -> Vec<&Peak> {
        peaks
            .iter()
            .filter(|p| {
                (p.x_m - x_end).abs() <= win
                    && p.energy >= PEAK_MIN_E
                    && p.vlen_m >= PEAK_MIN_VLEN_M
                    && (p.x_m - (x_end - win)).abs() >= WINDOW_LIMIT_M
                    && (p.x_m - (x_end + win)).abs() >= WINDOW_LIMIT_M
            })
            .collect()
    };
    let ca = if fix_a { Vec::new() } else { candidates(x_a) };
    let cb = if fix_b { Vec::new() } else { candidates(x_b) };
    let mut s_l: Option<f64> = None;
    let mut s_r: Option<f64> = None;
    let mut src_a = "osm".to_string();
    let mut src_b = "osm".to_string();
    let mut flags: Vec<String> = Vec::new();
    if !ca.is_empty() && !cb.is_empty() {
        let mut best: Option<(f64, f64)> = None;
        let mut best_score = f64::NEG_INFINITY;
        for pa in &ca {
            for pb in &cb {
                if pb.x_m - pa.x_m < 1.0 {
                    continue;
                }
                let sc = pa.energy + pb.energy - ((pb.x_m - pa.x_m) - l_osm).abs() / lam;
                if sc > best_score {
                    best_score = sc;
                    best = Some((pa.x_m, pb.x_m));
                }
            }
        }
        if let Some((a, b)) = best {
            s_l = Some(a);
            s_r = Some(b);
            src_a = "edge-joint".into();
            src_b = "edge-joint".into();
        }
    }
    if s_l.is_none() && s_r.is_none() {
        // `max` in Python keeps the first of equal keys
        let strongest = |v: &[&Peak]| -> Option<f64> {
            if v.is_empty() {
                None
            } else {
                Some(v[argmax_first(v.iter().map(|p| p.energy))].x_m)
            }
        };
        if let Some(a) = strongest(&ca) {
            s_l = Some(a);
            src_a = "edge-joint".into();
            if !fix_b {
                s_r = Some(x_b + (a - x_a));
                src_b = "edge-shift".into();
            }
        } else if let Some(b) = strongest(&cb) {
            s_r = Some(b);
            src_b = "edge-joint".into();
            if !fix_a {
                s_l = Some(x_a + (b - x_b));
                src_a = "edge-shift".into();
            }
        }
    }
    if s_l.is_none() && s_r.is_none() {
        return Extent {
            s_l: None,
            s_r: None,
            src_a: "osm".into(),
            src_b: "osm".into(),
            flags,
        };
    }
    let xl = s_l.unwrap_or(x_a);
    let xr = s_r.unwrap_or(x_b);
    if let Some((rgb, occl, ppm, s0)) = crop {
        let c0 = imgops::round_half_even((xl - s0) * ppm) as isize;
        let c1 = imgops::round_half_even((xr - s0) * ppm) as isize;
        let (fires, de) = half_consistency(rgb, occl, c0, c1, ppm);
        if fires {
            let inside: Vec<&Peak> = peaks
                .iter()
                .filter(|p| {
                    p.x_m > xl + 1.0
                        && p.x_m < xr - 1.0
                        && p.energy >= PEAK_MIN_E
                        && p.vlen_m >= PEAK_MIN_VLEN_M
                })
                .collect();
            let interior = if inside.is_empty() {
                None
            } else {
                Some(inside[argmax_first(inside.iter().map(|p| p.energy))])
            };
            match interior {
                Some(p) => flags.push(format!("PARTY_WALL_INSIDE {:.1}", p.x_m)),
                None => {
                    flags.push(format!("EXTENT_INCONSISTENT dE {de:.1}"));
                    return Extent {
                        s_l: None,
                        s_r: None,
                        src_a: "osm".into(),
                        src_b: "osm".into(),
                        flags,
                    };
                }
            }
        }
    }
    Extent {
        s_l,
        s_r,
        src_a,
        src_b,
        flags,
    }
}

// --------------------------------------------------------------------------- colour trim

/// Per block column, the median OkLab over the valid pixels and how many of
/// them there were.
fn block_column_lab(
    tex: &RgbImage,
    valid: &[bool],
    ppb: usize,
) -> (Vec<Option<[f64; 3]>>, Vec<f64>) {
    let (w, h) = (tex.width() as usize, tex.height() as usize);
    let cols = w / ppb.max(1);
    let mut med = Vec::with_capacity(cols);
    let mut share = Vec::with_capacity(cols);
    for j in 0..cols {
        let mut vals: Vec<[f64; 3]> = Vec::new();
        let mut n = 0usize;
        for y in 0..h {
            for x in j * ppb..(j + 1) * ppb {
                n += 1;
                if valid[y * w + x] {
                    vals.push(imgops::srgb_to_oklab(tex.get_pixel(x as u32, y as u32).0));
                }
            }
        }
        share.push(if n > 0 {
            vals.len() as f64 / n as f64
        } else {
            0.0
        });
        med.push(if vals.is_empty() {
            None
        } else {
            Some(median_lab(&vals))
        });
    }
    (med, share)
}

/// The neighbour-bleed trim: block columns at either end whose colour is not
/// the wall's own get dropped, at most two per end.
pub fn colour_trim(
    tex: &RgbImage,
    valid: &[bool],
    ppb: usize,
    wall_medoid_lab: [f64; 3],
    max_blocks: usize,
    de: f64,
) -> (usize, usize) {
    let (med, share) = block_column_lab(tex, valid, ppb);
    let cols = med.len();
    if cols < 2 * max_blocks + 1 {
        return (0, 0);
    }
    let count = |order: Box<dyn Iterator<Item = usize>>| -> usize {
        let mut n = 0usize;
        for j in order {
            if n >= max_blocks || share[j] < 0.2 {
                break;
            }
            match med[j] {
                Some(m) if lab_distance(&m, &wall_medoid_lab) > de => n += 1,
                _ => break,
            }
        }
        n
    };
    (count(Box::new(0..cols)), count(Box::new((0..cols).rev())))
}

/// The median OkLab of the middle 60 per cent of the texture, the reference the
/// colour trim measures the ends against.
pub fn wall_medoid(tex: &RgbImage, valid: &[bool]) -> [f64; 3] {
    let (w, h) = (tex.width() as usize, tex.height() as usize);
    let c0 = (0.2 * w as f64) as usize;
    let c1 = ((0.8 * w as f64) as usize).max(c0 + 1).min(w);
    let mut vals: Vec<[f64; 3]> = Vec::new();
    for y in 0..h {
        for x in c0..c1 {
            if valid[y * w + x] {
                vals.push(imgops::srgb_to_oklab(tex.get_pixel(x as u32, y as u32).0));
            }
        }
    }
    if vals.is_empty() {
        return [0.6, 0.0, 0.0];
    }
    median_lab(&vals)
}

// --------------------------------------------------------------------------- phase search

/// How two-sided the dark fraction of the blocks is on a grid starting at
/// `origin`, a private copy of the block rule so the phase search can score a
/// grid before the blocks exist.
fn bimodality(
    l: &[f64],
    b: &[f64],
    valid: &[bool],
    w: usize,
    h: usize,
    ppb: usize,
    origin: (usize, usize),
) -> f64 {
    let (ox, oy) = origin;
    if h <= oy || w <= ox {
        return 0.0;
    }
    let nby = (h - oy) / ppb;
    let nbx = (w - ox) / ppb;
    if nby < 1 || nbx < 1 {
        return 0.0;
    }
    let mut med = vec![f64::NAN; nby * nbx];
    let mut share = vec![0.0f64; nby * nbx];
    for by in 0..nby {
        for bx in 0..nbx {
            let mut vals = Vec::with_capacity(ppb * ppb);
            for y in 0..ppb {
                for x in 0..ppb {
                    let i = (oy + by * ppb + y) * w + ox + bx * ppb + x;
                    if valid[i] {
                        vals.push(l[i]);
                    }
                }
            }
            share[by * nbx + bx] = vals.len() as f64 / (ppb * ppb) as f64;
            if !vals.is_empty() {
                med[by * nbx + bx] = imgops::median_in_place(&mut vals);
            }
        }
    }
    let observed: Vec<bool> = share.iter().map(|s| *s >= BLOCK_MIN_VALID).collect();
    if !observed.iter().any(|v| *v) {
        return 0.0;
    }
    let mut seen: Vec<f64> = med
        .iter()
        .zip(&observed)
        .filter(|(m, o)| **o && m.is_finite())
        .map(|(m, _)| *m)
        .collect();
    let fill = if seen.is_empty() {
        0.0
    } else {
        imgops::median_in_place(&mut seen)
    };
    let med_f: Vec<f64> = med
        .iter()
        .map(|m| if m.is_finite() { *m } else { fill })
        .collect();
    let l_ref = median_filter_f64(&med_f, nbx, nby, 5);
    let mut acc = 0.0;
    let mut n = 0usize;
    for by in 0..nby {
        for bx in 0..nbx {
            if !observed[by * nbx + bx] {
                continue;
            }
            let lr = l_ref[by * nbx + bx];
            let thr = DARK_THR * (lr / 0.55).clamp(0.5, 1.0);
            let (mut hit, mut tot) = (0usize, 0usize);
            for y in 0..ppb {
                for x in 0..ppb {
                    let i = (oy + by * ppb + y) * w + ox + bx * ppb + x;
                    if !valid[i] {
                        continue;
                    }
                    tot += 1;
                    let dark = l[i] < lr - thr;
                    let glass = b[i] < GLASS_B && l[i] > lr + GLASS_L;
                    if dark || glass {
                        hit += 1;
                    }
                }
            }
            let dk = hit as f64 / tot.max(1) as f64;
            acc += (dk - 0.5).abs();
            n += 1;
        }
    }
    if n == 0 {
        0.0
    } else {
        acc / n as f64 * 2.0
    }
}

/// Where the one metre block grid should start, so the blocks land on the
/// facade's own rhythm instead of on an arbitrary offset.
pub fn phase_search(
    tex: &RgbImage,
    valid: &[bool],
    ppb: usize,
    params: &Params,
) -> (f64, f64, f64) {
    let (w, h) = (tex.width() as usize, tex.height() as usize);
    let lab: Vec<[f64; 3]> = tex.pixels().map(|p| imgops::srgb_to_oklab(p.0)).collect();
    let l: Vec<f64> = lab.iter().map(|v| v[0]).collect();
    let b: Vec<f64> = lab.iter().map(|v| v[2]).collect();
    let k = imgops::round_half_even(params.phase_range_m / params.phase_step_m) as i32;
    let shifts: Vec<f64> = (-k..=k).map(|i| i as f64 * params.phase_step_m).collect();
    let mut best = (0.0f64, 0.0f64, -1.0f64);
    for dy in &shifts {
        for dx in &shifts {
            let ox =
                (imgops::round_half_even(dx * ppb as f64) as i64).rem_euclid(ppb as i64) as usize;
            let oy =
                (imgops::round_half_even(dy * ppb as f64) as i64).rem_euclid(ppb as i64) as usize;
            let bm = bimodality(&l, &b, valid, w, h, ppb, (ox, oy));
            let key = (round6(bm), -(dx.abs() + dy.abs()));
            let cur = (round6(best.2), -(best.0.abs() + best.1.abs()));
            if key > cur {
                best = (*dx, *dy, bm);
            }
        }
    }
    best
}

/// `round(x, 6)`, the tie break the phase search compares on.
fn round6(x: f64) -> f64 {
    imgops::round_half_even(x * 1e6) / 1e6
}

// --------------------------------------------------------------------------- the height rule

/// The wall's roofline from its views: which family of estimates decides, and
/// what it says.
///
/// Provenance ladder: the ROOF_SKY views the C21 rule could confirm, failing
/// those the ROOF_EDGE views, failing those the ROOF_SKY views it could not.
/// Inside the family that wins, the view score weighted median.
///
/// This is the fixed rule. The Python used to take an unweighted median over
/// the ROOF_SKY views and ignore the ROOF_EDGE ones whenever any sky view
/// existed, so one bad view outvoted two that agreed: on `r6035286_10` a sky
/// boundary that was really the top of the crop's no-data region beat an edge
/// estimate on the tower's real setback, 8.47 m against 13.55 m, where the
/// 360 only run says 13.75 m. Two things were measured and not taken. Pooling
/// the edge views in with the sky ones at half weight fixes that wall too, but
/// on `w79817227_4` it lets two edge views that agree at 11.6 and 11.8 m on a
/// string course outvote the sky view that has the roofline, and the wall loses
/// six metres. Weighting without the ladder does not reach the motivating wall
/// at all, because there the bad sky view is also the better scoring one.
pub fn roof_vote(views: &[ViewEvidence]) -> (Option<f64>, String) {
    // rank 2 confirmed sky, 1 edge, 0 unconfirmed sky
    let mut ranked: [Vec<(f64, f64)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for v in views {
        let r = v.refinement;
        let Some(h) = r.h_sky else { continue };
        if !h.is_finite() {
            continue;
        }
        let sky = r.roof_flag == ROOF_SKY;
        if !sky && r.roof_flag != ROOF_EDGE {
            continue;
        }
        let unconfirmed = sky && r.roof_flags.iter().any(|f| f == ROOF_UNCONFIRMED);
        let rank = if unconfirmed {
            0
        } else if sky {
            2
        } else {
            1
        };
        ranked[rank].push((h, v.score.max(ROOF_SCORE_FLOOR)));
    }
    for rank in [2usize, 1, 0] {
        let grp = &ranked[rank];
        if grp.is_empty() {
            continue;
        }
        let values: Vec<f64> = grp.iter().map(|g| g.0).collect();
        let weights: Vec<f64> = grp.iter().map(|g| g.1).collect();
        let flag = if rank == 1 { ROOF_EDGE } else { ROOF_SKY };
        return (Some(weighted_median(&values, &weights)), flag.to_string());
    }
    (None, ROOF_OSM.to_string())
}

/// The height ladder: which of the roofline, the cloud and the OSM tag is used.
///
/// Sky and cloud within 2 m of each other means the sky measurement is
/// confirmed and is the answer. When they disagree the tagged height wins if
/// there is one, because two disagreeing measurements are worth less than a
/// surveyed number, and the disagreement is flagged.
///
/// `roof_blind` says a view saw masonry up to where its own picture stops, saw
/// no sky at all above it, and stops more than 2 m above the cloud
/// (`ROOF_NODATA` with `ROOF_BEHIND`). The cloud is then known not to be the
/// roof - the camera saw wall above it - so with no sky estimate left the
/// tagged height decides rather than a cloud that stopped short.
pub fn decide_height(
    h_sky: Option<f64>,
    h_cloud: Option<f64>,
    h_osm: Option<f64>,
    osm_default: bool,
    osm_source: &str,
    params: &Params,
    roof_blind: bool,
) -> (f64, String, Vec<String>) {
    let has_osm = h_osm.filter(|v| *v != 0.0).is_some();
    let default = osm_default || !has_osm;
    // the Python hardcodes 9.0 here, which is `Params::default_height_m`; going
    // through the parameter keeps the two in step if it is ever changed
    let h_o = h_osm
        .filter(|v| *v != 0.0)
        .unwrap_or(params.default_height_m);
    let src_o = if default { "default" } else { osm_source };
    if let (Some(hs), Some(hc)) = (h_sky, h_cloud) {
        if (hs - hc).abs() <= 2.0 {
            return (hs, "sky+cloud".into(), Vec::new());
        }
        if !default {
            return (h_o, src_o.into(), vec!["HEIGHT_CONFLICT".into()]);
        }
        return (hc, "cloud".into(), vec!["HEIGHT_CONFLICT".into()]);
    }
    if let Some(hs) = h_sky {
        return (hs, "sky".into(), Vec::new());
    }
    if let Some(hc) = h_cloud {
        if roof_blind && !default {
            return (h_o, src_o.into(), Vec::new());
        }
        if default || (hc - h_o).abs() > 2.5 {
            return (hc, "cloud".into(), Vec::new());
        }
        return (h_o, src_o.into(), Vec::new());
    }
    (h_o, src_o.into(), Vec::new())
}

// --------------------------------------------------------------------------- per view

/// A copy of the crop with the shear applied.
fn corrected_crop(crop: &LooseCrop, h_metric: &[[f64; 3]; 3]) -> LooseCrop {
    let (rgb, occl) = warp_crop(crop, h_metric);
    LooseCrop {
        rgb,
        occl,
        ..crop.clone()
    }
}

/// What one view says about one wall.
///
/// The order matters and is the Python's: the lean is measured and, if it is
/// accepted, the crop is warped and the segments re-detected so everything
/// after works in the corrected frame; then the plane gate, then the ground row
/// whose `dz` moves the base, then the roofline and the column profile against
/// that corrected base.
pub fn refine_view(
    crop: &LooseCrop,
    input: &ViewInputs,
    params: &Params,
) -> (Refinement, RefineDetails) {
    let wall = input.wall;
    let l = input.l_fit;
    let h_osm_m = wall
        .height_osm
        .filter(|v| *v != 0.0)
        .unwrap_or(params.default_height_m);
    let (w, h) = (crop.rgb.width() as usize, crop.rgb.height() as usize);
    let mut gray = to_gray(&crop.rgb);
    let (mut vert, mut hor) = lsd_families(&gray, w, h, crop.ppm, Some(&crop.occl));
    let (c0, c1, lean_flag, h_shear, lstats) = lean_keystone(crop, &vert, params);
    let (g0, g1, gsup) = lean_from_gradients(&gray, w, h, crop.x_foot, crop.ppm, Some(&crop.occl));
    let mut c0_after = c0;
    let warped;
    let wc: &LooseCrop = if lean_flag == SHEAR_OK {
        warped = corrected_crop(crop, &h_shear);
        gray = to_gray(&warped.rgb);
        let f = lsd_families(&gray, w, h, warped.ppm, Some(&warped.occl));
        vert = f.0;
        hor = f.1;
        c0_after = lean_keystone(&warped, &vert, params).0;
        &warped
    } else {
        crop
    };
    let bbox = (
        wc.s_to_x(0.0),
        wc.s_to_x(l),
        wc.h_to_y(h_osm_m),
        wc.h_to_y(0.0),
    );
    let (slope, plane_flag, n_gate) = plane_gate(wc, &hor, bbox, params);
    let (dz, ground_flag) = ground_row(wc, wc.h_to_y(0.0), input.ground_source);
    let facade_rows = (wc.h_to_y(0.8 * h_osm_m), wc.h_to_y(0.3 * h_osm_m));
    let (sky, sky_mode, _colour) = sky_mask(&wc.rgb, Some(facade_rows));
    let s_range = input.s_vis.unwrap_or((0.0, l));
    let roof = sky_roofline(
        wc,
        &sky,
        s_range,
        wall.height_osm,
        wall.height_source.as_str() == "default",
        input.h_cloud,
        params,
    );
    let h_sky = roof.h.map(|v| v - dz);
    let h_cloud_c = input.h_cloud.map(|v| v - dz);
    let h_guess = match roof.h {
        Some(v) if roof.flag == ROOF_SKY => v,
        _ => h_osm_m + dz,
    };
    let box_rows = (wc.h_to_y(h_guess - 0.3), wc.h_to_y(dz + 0.3));
    let (profile, peaks) = column_profile(wc, &vert, box_rows);
    let details = RefineDetails {
        n_vertical: vert.len(),
        n_horizontal: hor.len(),
        lean_n: lstats.n,
        lean_inliers: lstats.inliers,
        lean_rms_deg: lstats.rms_deg,
        lean_x_spread_m: lstats.x_spread_m,
        c0_after_deg: c0_after,
        grad_c0_deg: g0,
        grad_c1_deg_per_m: g1,
        grad_support: gsup,
        plane_gate_n: n_gate,
        sky_mode,
        h_sky_raw: roof.h,
        h_cloud: h_cloud_c,
        s_range: [s_range.0, s_range.1],
        box_px: [bbox.0, bbox.1, bbox.2, bbox.3],
        l_fit: l,
        h_osm: h_osm_m,
    };
    let refinement = Refinement {
        view_key: super::types::view_key(&wall.key, input.pano_id),
        c0_deg: c0,
        c1_deg_per_m: c1,
        lean_flag,
        h_shear,
        plane_slope: slope,
        plane_flag,
        h_sky,
        roof_flag: roof.flag,
        roof_flags: roof.flags,
        roof_spread_m: roof.spread_m,
        ground_dz: dz,
        ground_flag,
        profile,
        profile_x0_m: wc.s0 + 0.5 / PROFILE_PPM,
        peaks,
        z_base: crop.z_base + dz,
    };
    (refinement, details)
}

// --------------------------------------------------------------------------- per wall

/// The final rectangle and height for a wall, from all its views together.
///
/// Extent: every view solves both ends on its own profile and the wall takes
/// the median of what they said, with the source the majority one; a plane that
/// found a real corner in the cloud pins that end instead. Height: the roof
/// vote, the median cloud height and the OSM tag through the ladder.
pub fn decide_wall(
    wall: &Wall,
    l_fit: f64,
    fix_a: bool,
    fix_b: bool,
    views: &[ViewEvidence],
    extra_flags: &[String],
    params: &Params,
) -> WallDecision {
    let (x_a, x_b) = (0.0, l_fit);
    let mut ends_a: Vec<f64> = Vec::new();
    let mut ends_b: Vec<f64> = Vec::new();
    let mut srcs_a: Vec<String> = Vec::new();
    let mut srcs_b: Vec<String> = Vec::new();
    let mut flags: Vec<String> = Vec::new();
    for v in views {
        let ds = v.ds_m;
        let peaks: Vec<Peak> = v
            .refinement
            .peaks
            .iter()
            .map(|p| Peak {
                x_m: p.x_m - ds,
                ..*p
            })
            .collect();
        let crop = v
            .crop
            .map(|c| (&c.rgb, Some(c.occl.as_slice()), c.ppm, c.s0 - ds));
        let e = extent_joint(&peaks, x_a, x_b, wall.length, crop, fix_a, fix_b, params);
        flags.extend(e.flags);
        if let Some(sl) = e.s_l {
            ends_a.push(sl);
            srcs_a.push(e.src_a);
        }
        if let Some(sr) = e.s_r {
            ends_b.push(sr);
            srcs_b.push(e.src_b);
        }
    }
    let (mut s_l, mut src_a) = if fix_a {
        (x_a, "cloud-corner".to_string())
    } else if !ends_a.is_empty() {
        (imgops::median(&ends_a), majority(&srcs_a))
    } else {
        (x_a, "osm".to_string())
    };
    let (mut s_r, mut src_b) = if fix_b {
        (x_b, "cloud-corner".to_string())
    } else if !ends_b.is_empty() {
        (imgops::median(&ends_b), majority(&srcs_b))
    } else {
        (x_b, "osm".to_string())
    };
    if s_r - s_l < 1.0f64.max(0.5 * l_fit) {
        s_l = x_a;
        s_r = x_b;
        src_a = "osm".into();
        src_b = "osm".into();
        flags.push("EXTENT_DEGENERATE".into());
    }
    let (h_sky, roof_used) = roof_vote(views);
    let clouds: Vec<f64> = views
        .iter()
        .filter_map(|v| v.h_cloud)
        .filter(|v| v.is_finite())
        .collect();
    let h_cloud = if clouds.is_empty() {
        None
    } else {
        Some(imgops::median(&clouds))
    };
    let h_osm = wall.height_osm.filter(|v| *v != 0.0);
    // a view that saw only masonry up to where its picture stops, and stops well
    // above the cloud, says the cloud did not reach this roof either
    let blind = views.iter().any(|v| {
        let f = &v.refinement.roof_flags;
        f.iter().any(|x| x == ROOF_NODATA) && f.iter().any(|x| x == ROOF_BEHIND)
    });
    let (h_used, h_src, hflags) = decide_height(
        h_sky,
        h_cloud,
        h_osm,
        wall.height_source.as_str() == "default",
        wall.height_source.as_str(),
        params,
        blind,
    );
    flags.extend(hflags);
    flags.push(roof_used);
    if let Some(best) = views.first() {
        flags.push(best.refinement.lean_flag.clone());
        flags.push(best.refinement.ground_flag.clone());
        let n_mis = views
            .iter()
            .filter(|v| v.refinement.plane_flag == PLANE_MISMATCH)
            .count();
        if n_mis > 0 && n_mis * 2 >= views.len() {
            flags.push(PLANE_MISMATCH.into());
        } else {
            flags.push(best.refinement.plane_flag.clone());
        }
    }
    flags.extend(extra_flags.iter().cloned());
    let mut seen: Vec<String> = Vec::new();
    for f in flags {
        if !seen.contains(&f) {
            seen.push(f);
        }
    }
    WallDecision {
        wall_key: wall.key.clone(),
        s_l,
        s_r,
        src_a,
        src_b,
        trimmed_a: 0,
        trimmed_b: 0,
        h_used,
        height_source: h_src,
        h_sky,
        h_cloud,
        h_osm,
        phase: [0.0, 0.0],
        bimodality: 0.0,
        flags: seen,
    }
}

/// The most common string, ties going to the one seen first, which is
/// `collections.Counter.most_common(1)`.
fn majority(v: &[String]) -> String {
    let mut best = String::new();
    let mut best_n = 0usize;
    for s in v {
        let n = v.iter().filter(|o| *o == s).count();
        if n > best_n {
            best_n = n;
            best = s.clone();
        }
    }
    best
}

/// The steps that need the finished texture: the neighbour-bleed trim on the
/// ends OSM decided, and the phase of the block grid.
pub fn finalise_wall(
    dec: &mut WallDecision,
    tex: &RgbImage,
    valid: &[bool],
    ppb: usize,
    wall_medoid_lab: Option<[f64; 3]>,
    params: &Params,
) {
    let medoid = wall_medoid_lab.unwrap_or_else(|| wall_medoid(tex, valid));
    let (mut n_a, mut n_b) = colour_trim(tex, valid, ppb, medoid, 2, 0.08);
    if dec.src_a != "osm" {
        n_a = 0;
    }
    if dec.src_b != "osm" {
        n_b = 0;
    }
    let cols = imgops::round_half_even(dec.s_r - dec.s_l) as usize;
    if n_a + n_b + 1 >= cols {
        n_a = 0;
        n_b = 0;
    }
    dec.trimmed_a = n_a as i32;
    dec.trimmed_b = n_b as i32;
    dec.s_l += n_a as f64;
    dec.s_r -= n_b as f64;
    let w = tex.width() as usize;
    let x0 = n_a * ppb;
    let x1 = w.saturating_sub(n_b * ppb);
    let (sub, sub_valid) = if x0 == 0 && x1 == w {
        (tex.clone(), valid.to_vec())
    } else {
        let h = tex.height() as usize;
        let mut s = RgbImage::new((x1 - x0) as u32, h as u32);
        let mut v = Vec::with_capacity((x1 - x0) * h);
        for y in 0..h {
            for x in x0..x1 {
                s.put_pixel(
                    (x - x0) as u32,
                    y as u32,
                    *tex.get_pixel(x as u32, y as u32),
                );
                v.push(valid[y * w + x]);
            }
        }
        (s, v)
    };
    let (dx, dy, bm) = phase_search(&sub, &sub_valid, ppb, params);
    dec.phase = [dx, dy];
    dec.bimodality = bm;
    if n_a > 0 || n_b > 0 {
        dec.flags.push(format!("TRIMMED {n_a}/{n_b}"));
    }
}

/// `(s_l, s_r, 0, h_used)` in corrected wall coordinates, the rectangle the
/// final resampling renders.
pub fn final_rect(dec: &WallDecision) -> (f64, f64, f64, f64) {
    (dec.s_l, dec.s_r, 0.0, dec.h_used)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapillary::golden;
    use crate::mapillary::types::HeightSource;

    /// A wall carrying only what `refine_view` and `decide_wall` read off it.
    fn wall_of(key: &str, length: f64, height_osm: Option<f64>, source: &str) -> Wall {
        Wall {
            key: key.to_string(),
            building_key: String::new(),
            idx: 0,
            node_a: 0,
            node_b: 0,
            a: [0.0, 0.0],
            b: [length, 0.0],
            n: [0.0, -1.0],
            length,
            merged_idx: vec![],
            piece: 0,
            n_pieces: 1,
            height_osm,
            height_source: HeightSource::from_str_or_default(source),
            reachable: true,
            unreachable_reason: String::new(),
            edges: vec![],
            s_offset: 0.0,
        }
    }

    fn refinement_of(v: &golden::GoldenWallViewInput) -> Refinement {
        Refinement {
            view_key: v.pano_id.clone(),
            c0_deg: 0.0,
            c1_deg_per_m: 0.0,
            lean_flag: v.lean_flag.clone(),
            h_shear: IDENTITY3,
            plane_slope: 0.0,
            plane_flag: v.plane_flag.clone(),
            h_sky: v.h_sky,
            roof_flag: v.roof_flag.clone(),
            roof_flags: v.roof_flags.clone(),
            roof_spread_m: 0.0,
            ground_dz: v.ground_dz,
            ground_flag: v.ground_flag.clone(),
            profile: v.profile.clone(),
            profile_x0_m: v.profile_x0_m,
            peaks: v
                .peaks
                .iter()
                .map(|p| Peak {
                    x_m: p.x_m,
                    energy: p.energy,
                    vlen_m: p.vlen_m,
                })
                .collect(),
            z_base: 0.0,
        }
    }

    /// Every wall of the reference run, through the extent and height rules.
    ///
    /// The per view evidence comes from the fixture rather than from the crops,
    /// so what is under test is the merge: the joint extent solve on each
    /// view's own peaks, the median over the views, the roof vote and the
    /// height ladder. Those are the same arithmetic in both languages, so the
    /// tolerance is numerical noise and not a measurement tolerance.
    #[test]
    fn the_wall_decisions_reproduce_the_python() {
        if golden::absent() {
            return;
        }

        let params = Params::default();
        let walls = golden::refine_walls();
        assert!(walls.len() >= 100, "the fixture is too small");
        let mut worst_h = 0.0f64;
        let mut worst_s = 0.0f64;
        let mut n_flag = 0usize;
        for w in &walls {
            let wall = wall_of(
                &w.wall_key,
                w.input.length_m,
                w.input.height_osm,
                &w.input.height_source,
            );
            let refs: Vec<Refinement> = w.input.views.iter().map(refinement_of).collect();
            let views: Vec<ViewEvidence> = w
                .input
                .views
                .iter()
                .zip(&refs)
                .map(|(v, r)| ViewEvidence {
                    refinement: r,
                    score: v.score,
                    ds_m: v.ds_m,
                    h_cloud: v.h_cloud,
                    crop: None,
                })
                .collect();
            let dec = decide_wall(
                &wall,
                w.input.l_fit,
                w.input.fit_src_a == "cloud-corner",
                w.input.fit_src_b == "cloud-corner",
                &views,
                &[],
                &params,
            );
            worst_h = worst_h.max((dec.h_used - w.height.h_used).abs());
            worst_s = worst_s
                .max((dec.s_l - w.extent_nocrop.s_l).abs())
                .max((dec.s_r - w.extent_nocrop.s_r).abs());
            assert_eq!(
                dec.height_source, w.height.source,
                "{}: height source",
                w.wall_key
            );
            assert_eq!(dec.src_a, w.extent_nocrop.src_a, "{}: src_a", w.wall_key);
            assert_eq!(dec.src_b, w.extent_nocrop.src_b, "{}: src_b", w.wall_key);
            if dec.flags != w.flags {
                n_flag += 1;
                println!("{} flags {:?} against {:?}", w.wall_key, dec.flags, w.flags);
            }
        }
        println!(
            "decide_wall over {} walls: worst |dh| {worst_h:.3e} m, worst |ds| {worst_s:.3e} m, \
             {n_flag} flag lists differing",
            walls.len()
        );
        assert!(worst_h < 1e-6, "worst height difference {worst_h}");
        assert!(worst_s < 1e-6, "worst extent difference {worst_s}");
        assert_eq!(n_flag, 0);
    }

    /// The height fix, on the walls it moves.
    ///
    /// The reference run carries the fixed vote itself now, so
    /// `height_fix_moved` is empty and the old shape of this test, "the manifest
    /// lists where the re-run vote disagrees with the run's own answer", has
    /// nothing left to read. Two checks replace it. First, the run and the
    /// fixture agree on every wall: that disagreement was a real defect, it made
    /// the port look wrong for being right, and it can come back the moment
    /// anyone re-exports from a run made with an older `refine.py`. Second, the
    /// ladder is compared against the rule it replaced on the walls MEASURED.md
    /// names, from the fixture's own per view evidence, so what is checked is
    /// the rule and not a number copied out of a file.
    #[test]
    fn the_roof_vote_is_the_fixed_one() {
        if golden::absent() {
            return;
        }

        let manifest = golden::refine_manifest();
        let walls = golden::refine_walls();
        for m in &manifest.height_fix_moved {
            let w = walls
                .iter()
                .find(|w| w.wall_key == m.wall_key)
                .unwrap_or_else(|| panic!("{} is not in walls.json", m.wall_key));
            assert!(
                (w.height.h_used - m.now).abs() < 1e-6,
                "{}: the fixture disagrees with its own manifest",
                m.wall_key
            );
            assert!(
                (m.now - m.was).abs() > 0.5,
                "{}: listed as moved but did not",
                m.wall_key
            );
        }

        // The fixture set no longer disagrees with itself.
        let mut worst = 0.0f64;
        for w in &walls {
            if let Some(h) = w.reference.h_used {
                worst = worst.max((h - w.height.h_used).abs());
            }
        }
        assert!(
            worst < 1e-6,
            "the run and the fixture differ by {worst} m on some wall, so the run predates the fix"
        );

        // The rule it replaced: the unweighted median of the ROOF_SKY estimates,
        // with the ROOF_EDGE ones ignored whenever any sky view exists.
        let old_rule = |w: &golden::GoldenRefineWall| -> Option<f64> {
            for want_sky in [true, false] {
                let mut hs: Vec<f64> = w
                    .input
                    .views
                    .iter()
                    .filter(|v| v.roof_flag == if want_sky { ROOF_SKY } else { ROOF_EDGE })
                    .filter_map(|v| v.h_sky)
                    .collect();
                if !hs.is_empty() {
                    return Some(imgops::median_in_place(&mut hs));
                }
            }
            None
        };

        // Every wall: the port's own vote over the fixture's evidence is what
        // the fixture records, so the cases below are the rule's and not one
        // implementation's.
        for w in &walls {
            let refs: Vec<Refinement> = w.input.views.iter().map(refinement_of).collect();
            let views: Vec<ViewEvidence> = w
                .input
                .views
                .iter()
                .zip(&refs)
                .map(|(v, r)| ViewEvidence {
                    refinement: r,
                    score: v.score,
                    ds_m: v.ds_m,
                    h_cloud: v.h_cloud,
                    crop: None,
                })
                .collect();
            let (h, _source) = roof_vote(&views);
            match (h, w.height.h_sky) {
                (Some(a), Some(b)) => assert!(
                    (a - b).abs() < 1e-6,
                    "{}: the vote says {a} where the fixture says {b}",
                    w.wall_key
                ),
                (None, None) => {}
                (a, b) => panic!(
                    "{}: the vote says {a:?} where the fixture says {b:?}",
                    w.wall_key
                ),
            }
        }

        // The two rules separate on 77 of the 111 walls, and these are the ones
        // they separate on by more than half a metre. The list is short because
        // most of what the ladder used to be rescuing were sky boundaries that
        // were really the edge of a photograph, and ROOF_NODATA takes those out
        // of the vote before either rule sees them.
        //
        // | wall | the old rule | the ladder | why |
        // | `w97508963_0` | 14.92 | 13.80 | three edge views; the weighted median leans to the two that score best |
        // | `w87406530_10` | 13.05 | 12.08 | two sky views 6 m apart and an edge view between them |
        // | `w61565147_2p0` | 16.47 | 15.79 | two edge views 3.8 m apart, one scoring twice the other |
        //
        // `w81190198_4` used to be the third of these at 15.29 against 16.03 and
        // is not any more: the exact plane fit moved its rectangle, its three sky
        // views now read 15.27, 15.28 and 15.51 m instead of spanning 7 m, and
        // the two rules land 3 cm apart.
        for (key, want_old, want_new) in [
            ("w97508963_0", 14.917, 13.803),
            ("w87406530_10", 13.045, 12.076),
            ("w61565147_2p0", 16.475, 15.791),
        ] {
            let w = walls
                .iter()
                .find(|w| w.wall_key == key)
                .unwrap_or_else(|| panic!("{key} is in the fixture"));
            let old = old_rule(w).unwrap_or_else(|| panic!("{key} has a roof estimate"));
            let new = w.height.h_sky.unwrap_or_else(|| panic!("{key} has a vote"));
            assert!(
                (old - want_old).abs() < 0.01,
                "{key}: the old rule is {old}"
            );
            assert!((new - want_new).abs() < 0.01, "{key}: the ladder is {new}");
        }

        // The wall MEASURED.md leads with, all the way to the height used. Its
        // sky view reported 8.47 m, which was the top of the crop's no-data
        // region rather than a roofline; the ladder used to reach past it through
        // ROOF_UNCONFIRMED and now the view brings no estimate at all. The wall
        // used to keep 13.75 m from an edge view; the exact plane fit moved that
        // view's crop across the absolute blur floor and the wall is down to this
        // one view and the default height, which is the blur floor's defect and
        // not the vote's (MEASURED.md, "the plane fit decided by a random
        // sample").
        let w = walls
            .iter()
            .find(|w| w.wall_key == "r6035286_10")
            .expect("r6035286_10 is in the fixture");
        assert_eq!(w.height.source, "default");
        assert!(
            (w.height.h_used - 9.0).abs() < 1e-9,
            "r6035286_10 is {} m",
            w.height.h_used
        );
        assert!(
            w.input.views.iter().any(|v| {
                v.pano_id == "1624369611671770"
                    && v.h_sky.is_none()
                    && v.roof_flags.iter().any(|f| f == ROOF_NODATA)
            }),
            "the Sendlinger Tor view brings no roofline: {:?}",
            w.input.views
        );

        // A wall the no-data rule carries on its own: one perspective view of a
        // building tagged 21 m through levels, of which the camera sees the
        // bottom 11 m and nothing above. The boundary between the masonry and
        // the black used to be the height.
        let w = walls
            .iter()
            .find(|w| w.wall_key == "w81190185_2")
            .expect("w81190185_2 is in the fixture");
        assert_eq!(
            (w.height.source.as_str(), w.height.h_used),
            ("levels", 21.0)
        );
        assert!(w.height.h_sky.is_none());

        // And a wall where the flag pair carries into the height ladder: the
        // camera saw wall up to 18.7 m and the cloud stops at 4.9 m, so the
        // cloud is not this roof and the tag decides.
        let w = walls
            .iter()
            .find(|w| w.wall_key == "w81190155_1")
            .expect("w81190155_1 is in the fixture");
        assert_eq!(
            (w.height.source.as_str(), w.height.h_used),
            ("levels", 21.0)
        );
        assert!(w.input.views[0].roof_flags.iter().any(|f| f == ROOF_BEHIND));
    }

    /// The sampled walls end to end: every view refined from its crop, then the
    /// wall decided from those refinements with the crops in hand, against what
    /// the Python decided.
    ///
    /// This is the measurement the line segment detector is held to, and the
    /// tolerances are the fixture's: 0.3 degrees of lean, 0.2 m of height, 0.2 m
    /// on either end of the wall.
    ///
    /// One allowance, and it is in the algorithm rather than in the port: the
    /// ground row is the strongest horizontal edge in a 1.5 m window, and on one
    /// view of `w97509016_0` that window holds two edges a quarter of a metre
    /// apart whose energy is within a per cent. The two implementations tip that
    /// coin differently. What moves is the base the heights are measured from, so
    /// the facade's top stays where it is in the world; a wall whose ground row
    /// went the other way is therefore measured on its height above the crop's
    /// own base, and its lean is not measured at all, because the shear is fitted
    /// over the band that base defines.
    #[test]
    fn the_sampled_walls_decide_as_the_python_does() {
        if golden::pixels_absent() {
            return;
        }

        let params = Params::default();
        let manifest = golden::refine_manifest();
        let tol = &manifest.tolerances;
        let walls = golden::refine_walls();
        let views: Vec<golden::GoldenRefineView> = golden::refine_views();
        let (mut worst_lean, mut worst_h, mut worst_s) = (0.0f64, 0.0f64, 0.0f64);
        let mut worst_lean_all = 0.0f64;
        let (mut n, mut n_tie) = (0usize, 0usize);
        for key in &manifest.walls_with_crops {
            let w = walls
                .iter()
                .find(|w| w.wall_key == *key)
                .unwrap_or_else(|| panic!("{key} is not in walls.json"));
            let wall = wall_of(
                key,
                w.input.length_m,
                w.input.height_osm,
                &w.input.height_source,
            );
            let mut refs = Vec::new();
            let mut crops = Vec::new();
            let mut clouds = Vec::new();
            for v in &w.input.views {
                let fixture = views
                    .iter()
                    .find(|f| f.wall_key == *key && f.pano_id == v.pano_id)
                    .unwrap_or_else(|| panic!("{key}: no crop for {}", v.pano_id));
                let crop = fixture.crop();
                let input = ViewInputs {
                    wall: &wall,
                    pano_id: &v.pano_id,
                    l_fit: fixture.input.l_fit,
                    s_vis: Some((fixture.input.s_vis[0], fixture.input.s_vis[1])),
                    h_cloud: fixture.input.h_cloud_raw,
                    ground_source: GroundSource::from_str_or_default(&fixture.input.ground_source),
                };
                let (r, d) = refine_view(&crop, &input, &params);
                refs.push(r);
                crops.push(crop);
                clouds.push(d.h_cloud);
            }
            let evidence: Vec<ViewEvidence> = w
                .input
                .views
                .iter()
                .enumerate()
                .map(|(i, v)| ViewEvidence {
                    refinement: &refs[i],
                    score: v.score,
                    ds_m: v.ds_m,
                    h_cloud: clouds[i],
                    crop: Some(&crops[i]),
                })
                .collect();
            let dec = decide_wall(
                &wall,
                w.input.l_fit,
                w.input.fit_src_a == "cloud-corner",
                w.input.fit_src_b == "cloud-corner",
                &evidence,
                &[],
                &params,
            );
            let ext = w.extent_crop.as_ref().unwrap_or(&w.extent_nocrop);
            // the run records the median of its views' lean, so that is what a
            // wall's lean is compared on
            let lean = imgops::median(&refs.iter().map(|r| r.c0_deg).collect::<Vec<_>>());
            let d_lean = w
                .reference
                .lean_deg
                .map(|l| (lean - l).abs())
                .unwrap_or(0.0);
            // the largest ground row disagreement over this wall's views
            let d_dz = w
                .input
                .views
                .iter()
                .zip(&refs)
                .map(|(v, r)| r.ground_dz - v.ground_dz)
                .fold(0.0f64, |a, b| if b.abs() > a.abs() { b } else { a });
            let raw_h = (dec.h_used - w.height.h_used).abs();
            let tied = d_dz.abs() > tol.height_m;
            // A tied wall is allowed the base shift and no more, either way: a
            // height read off the crop moves with the base and a tagged one does
            // not, and which of the two decided is the wall's business.
            let d_h = if tied {
                raw_h.min((raw_h - d_dz.abs()).abs())
            } else {
                raw_h
            };
            if tied {
                n_tie += 1;
            }
            let d_s = (dec.s_l - ext.s_l).abs().max((dec.s_r - ext.s_r).abs());
            println!(
                "{key:<16} lean {:>7.3} (py {:>7.3})  h {:>6.2} (py {:>6.2}) {:<10} \
                 s [{:>7.2},{:>7.2}] (py [{:>7.2},{:>7.2}]) {}/{}{}",
                lean,
                w.reference.lean_deg.unwrap_or(f64::NAN),
                dec.h_used,
                w.height.h_used,
                dec.height_source,
                dec.s_l,
                dec.s_r,
                ext.s_l,
                ext.s_r,
                dec.src_a,
                dec.src_b,
                if tied {
                    format!("  ground row tie, base moved {d_dz:.2} m")
                } else {
                    String::new()
                }
            );
            // the lean only reaches the geometry when the shear was accepted; on
            // a rejected fit it is an extrapolation to the camera's own column
            // that nothing downstream reads, and on a wall whose ground row tied
            // it is fitted over a band that starts a quarter of a metre away
            if refs[0].lean_flag == SHEAR_OK && !tied {
                worst_lean = worst_lean.max(d_lean);
            }
            worst_lean_all = worst_lean_all.max(d_lean);
            worst_h = worst_h.max(d_h);
            worst_s = worst_s.max(d_s);
            n += 1;
        }
        println!(
            "{n} walls end to end: worst lean {worst_lean:.3} deg where the shear was applied \
             ({worst_lean_all:.3} counting the walls where it was not), worst height \
             {worst_h:.3} m, worst extent {worst_s:.3} m, {n_tie} ground row ties"
        );
        assert!(n >= 5, "too few walls with crops");
        assert!(worst_lean <= tol.lean_deg, "worst lean {worst_lean} deg");
        assert!(worst_h <= tol.height_m, "worst height {worst_h} m");
        assert!(worst_s <= tol.extent_m, "worst extent {worst_s} m");
        assert!(n_tie <= 1, "{n_tie} walls turned on a ground row tie");
    }

    /// One view at a time, from its crop, against what the Python got.
    ///
    /// The roofline is compared above the crop's own base (`h_sky_raw`) as well
    /// as above the corrected one, because those two differ by exactly the
    /// ground row and the ground row can be a coin flip; see
    /// [`the_sampled_walls_decide_as_the_python_does`].
    #[test]
    fn refine_view_matches_the_python_on_the_sampled_views() {
        if golden::pixels_absent() {
            return;
        }

        let params = Params::default();
        let manifest = golden::refine_manifest();
        let tol = manifest.tolerances;
        let views = golden::refine_views();
        assert!(views.len() >= 10, "the fixture is too small");
        let (mut worst_lean, mut worst_raw, mut worst_dz) = (0.0f64, 0.0f64, 0.0f64);
        let (mut flags_differ, mut n_tie, mut n_shear) = (0usize, 0usize, 0usize);
        for v in &views {
            let crop = v.crop();
            let wall = wall_of(
                &v.wall_key,
                v.input.l_fit,
                v.input.height_osm,
                &v.input.height_source,
            );
            let input = ViewInputs {
                wall: &wall,
                pano_id: &v.pano_id,
                l_fit: v.input.l_fit,
                s_vis: Some((v.input.s_vis[0], v.input.s_vis[1])),
                h_cloud: v.input.h_cloud_raw,
                ground_source: GroundSource::from_str_or_default(&v.input.ground_source),
            };
            let (r, d) = refine_view(&crop, &input, &params);
            let d_lean = (r.c0_deg - v.lean.c0_deg).abs();
            let d_dz = (r.ground_dz - v.ground.dz_m).abs();
            let d_raw = match (d.h_sky_raw, v.roof.h_sky_raw) {
                (Some(a), Some(b)) => (a - b).abs(),
                (None, None) => 0.0,
                _ => f64::INFINITY,
            };
            // A view whose ground row tied is read in a different frame: the
            // roofline is measured from that base and the shear is fitted over
            // the band above it, so neither is comparable with the run's. The
            // tie itself is what is bounded, at one view.
            let tied = d_dz > tol.height_m;
            if tied {
                n_tie += 1;
            } else {
                worst_dz = worst_dz.max(d_dz);
                worst_raw = worst_raw.max(d_raw);
            }
            if r.lean_flag == SHEAR_OK {
                if !tied {
                    worst_lean = worst_lean.max(d_lean);
                }
                n_shear += 1;
            }
            let same_flags = r.lean_flag == v.lean.flag
                && r.roof_flag == v.roof.flag
                && r.ground_flag == v.ground.flag
                && r.plane_flag == v.plane.flag;
            if !same_flags {
                flags_differ += 1;
            }
            println!(
                "{:<38} lines {:>4}/{:>4} (py {:>4}/{:>4}) lean {:>7.3} (py {:>7.3}) {:<26} \
                 {:<24} dz {:>6.3} (py {:>6.3}) {:<28} roof {:>9.3} (py {:>9.3}) peaks {:>3} (py {:>3})",
                v.view_key,
                d.n_vertical,
                d.n_horizontal,
                v.lines.n_vertical,
                v.lines.n_horizontal,
                r.c0_deg,
                v.lean.c0_deg,
                format!("{}/{}", r.lean_flag, v.lean.flag),
                format!("{}/{}", r.roof_flag, v.roof.flag),
                r.ground_dz,
                v.ground.dz_m,
                format!("{}/{}", r.ground_flag, v.ground.flag),
                d.h_sky_raw.unwrap_or(f64::NAN),
                v.roof.h_sky_raw.unwrap_or(f64::NAN),
                r.peaks.len(),
                v.profile.peaks.len(),
            );
        }
        println!(
            "refine_view over {} views ({n_shear} with the shear applied): worst lean \
             {worst_lean:.3} deg, worst roofline {worst_raw:.3} m, worst ground row \
             {worst_dz:.3} m, {n_tie} ground row ties, {flags_differ} flag sets differing",
            views.len()
        );
        assert!(worst_lean <= tol.lean_deg, "worst lean {worst_lean} deg");
        assert!(worst_dz <= tol.height_m, "worst ground row {worst_dz} m");
        assert!(worst_raw <= tol.height_m, "worst roofline {worst_raw} m");
        assert_eq!(flags_differ, 0, "flags differ on {flags_differ} views");
        assert!(n_tie <= 1, "{n_tie} views turned on a ground row tie");
    }

    // ----------------------------------------------------------------- units

    /// A synthetic facade: a dark rectangle on a light ground, so the four
    /// edges are known exactly.
    fn synthetic(w: usize, h: usize, rect: (usize, usize, usize, usize)) -> RgbImage {
        let mut img = RgbImage::new(w as u32, h as u32);
        for y in 0..h {
            for x in 0..w {
                let inside = x >= rect.0 && x < rect.2 && y >= rect.1 && y < rect.3;
                let v = if inside { 40 } else { 220 };
                img.put_pixel(x as u32, y as u32, Rgb([v, v, v]));
            }
        }
        img
    }

    #[test]
    fn lsd_finds_the_edges_of_a_rectangle() {
        let img = synthetic(120, 160, (30, 40, 90, 130));
        let gray = to_gray(&img);
        let segs = detect_segments(&gray, 120, 160, 10.0);
        assert!(segs.len() >= 4, "found {} segments", segs.len());
        // the two vertical edges, at x = 30 and x = 90, are at least 80 px long
        let verticals: Vec<&LineSegment> = segs
            .iter()
            .filter(|s| (s.x1 - s.x0).abs() < 2.0 && s.length > 60.0)
            .collect();
        assert!(verticals.len() >= 2, "verticals {verticals:?}");
        for s in &verticals {
            let x = s.mid_x();
            assert!(
                (x - 30.0).abs() < 1.5 || (x - 90.0).abs() < 1.5,
                "a vertical at x {x}"
            );
            assert!(s.length > 80.0, "length {}", s.length);
        }
        let horizontals: Vec<&LineSegment> = segs
            .iter()
            .filter(|s| (s.y1 - s.y0).abs() < 2.0 && s.length > 40.0)
            .collect();
        assert!(horizontals.len() >= 2, "horizontals {horizontals:?}");
    }

    #[test]
    fn the_families_carry_the_lean_and_the_tilt() {
        // a rectangle leaning to the right: the top edge is shifted by +8 px
        let mut img = RgbImage::new(120, 160);
        for y in 0..160u32 {
            let shift = (8.0 * (160.0 - y as f64) / 160.0) as u32;
            for x in 0..120u32 {
                let inside = x >= 30 + shift && x < 90 + shift && (40..130).contains(&y);
                let v = if inside { 40 } else { 220 };
                img.put_pixel(x, y, Rgb([v, v, v]));
            }
        }
        let gray = to_gray(&img);
        let (vert, hor) = lsd_families(&gray, 120, 160, 10.0, None);
        assert!(!vert.is_empty() && !hor.is_empty());
        // 8 px over 160 rows is 2.86 degrees, and the vertical family measures
        // it positive because the top is to the right
        let lean = imgops::median(&vert.iter().map(|s| s.angle_deg).collect::<Vec<_>>());
        assert!((lean - 2.86).abs() < 0.6, "lean {lean}");
        for s in &vert {
            assert!(s.y0 >= s.y1, "verticals run bottom to top");
        }
        for s in &hor {
            assert!(s.x0 <= s.x1, "horizontals run left to right");
            assert!(s.angle_deg.abs() < 1.0, "tilt {}", s.angle_deg);
        }
    }

    #[test]
    fn the_weighted_median_reduces_to_the_plain_one() {
        for v in [
            vec![1.0, 2.0, 3.0],
            vec![1.0, 2.0, 3.0, 4.0],
            vec![5.0],
            vec![2.0, 1.0],
        ] {
            let w = vec![1.0; v.len()];
            assert!(
                (weighted_median(&v, &w) - imgops::median(&v)).abs() < 1e-12,
                "{v:?}"
            );
        }
        // a heavier sample pulls the answer toward itself but never past it
        let v = vec![10.0, 20.0];
        assert!((weighted_median(&v, &[3.0, 1.0]) - 12.5).abs() < 1e-9);
        assert!((weighted_median(&v, &[1.0, 3.0]) - 17.5).abs() < 1e-9);
        assert!((weighted_median(&v, &[1.0, 1000.0]) - 20.0).abs() < 0.1);
    }

    #[test]
    fn find_peaks_follows_scipy() {
        // a plateau's peak is its middle, rounded down
        let x: Vec<f32> = vec![0.0, 1.0, 5.0, 5.0, 5.0, 1.0, 0.0];
        assert_eq!(find_peaks(&x, 0.0, 1), vec![3]);
        // the height filter
        let x: Vec<f32> = vec![0.0, 4.0, 0.0, 2.0, 0.0];
        assert_eq!(find_peaks(&x, 3.0, 1), vec![1]);
        // the distance filter keeps the strongest and drops its neighbours
        let x: Vec<f32> = vec![0.0, 3.0, 0.0, 5.0, 0.0, 4.0, 0.0];
        assert_eq!(find_peaks(&x, 0.0, 1), vec![1, 3, 5]);
        assert_eq!(find_peaks(&x, 0.0, 3), vec![3]);
        assert!(find_peaks(&[1.0, 2.0], 0.0, 1).is_empty());
    }

    #[test]
    fn the_height_ladder_follows_the_evidence() {
        let p = Params::default();
        // sky and cloud agreeing is the best case there is
        let (h, src, f) = decide_height(
            Some(12.0),
            Some(13.0),
            Some(9.0),
            false,
            "levels",
            &p,
            false,
        );
        assert_eq!((h, src.as_str(), f.len()), (12.0, "sky+cloud", 0));
        // they disagree and the wall is tagged: the tag wins and says so
        let (h, src, f) = decide_height(
            Some(20.0),
            Some(13.0),
            Some(9.0),
            false,
            "levels",
            &p,
            false,
        );
        assert_eq!((h, src.as_str()), (9.0, "levels"));
        assert_eq!(f, vec!["HEIGHT_CONFLICT".to_string()]);
        // they disagree and there is no tag: the cloud wins
        let (h, src, _) = decide_height(Some(20.0), Some(13.0), None, true, "default", &p, false);
        assert_eq!((h, src.as_str()), (13.0, "cloud"));
        // the cloud alone, close to the tag, leaves the tag alone
        let (h, src, _) = decide_height(None, Some(10.0), Some(9.0), false, "tag", &p, false);
        assert_eq!((h, src.as_str()), (9.0, "tag"));
        // and far from it, replaces it
        let (h, src, _) = decide_height(None, Some(15.0), Some(9.0), false, "tag", &p, false);
        assert_eq!((h, src.as_str()), (15.0, "cloud"));
        // nothing at all falls back to the default
        let (h, src, _) = decide_height(None, None, None, true, "default", &p, false);
        assert_eq!((h, src.as_str()), (p.default_height_m, "default"));
        // a blind view (ROOF_NODATA with ROOF_BEHIND) says the camera saw wall
        // above the cloud, so a cloud alone does not decide against the tag;
        // without a tag it still does
        let (h, src, _) = decide_height(None, Some(5.0), Some(21.0), false, "levels", &p, true);
        assert_eq!((h, src.as_str()), (21.0, "levels"));
        let (h, src, _) = decide_height(None, Some(5.0), Some(21.0), false, "levels", &p, false);
        assert_eq!((h, src.as_str()), (5.0, "cloud"));
        let (h, src, _) = decide_height(None, Some(5.0), None, true, "default", &p, true);
        assert_eq!((h, src.as_str()), (5.0, "cloud"));
    }

    fn roof_view(h: f64, flag: &str, unconfirmed: bool, score: f64) -> (Refinement, f64) {
        let mut r = Refinement::empty("v", 0.0);
        r.h_sky = Some(h);
        r.roof_flag = flag.to_string();
        if unconfirmed {
            r.roof_flags.push(ROOF_UNCONFIRMED.into());
        }
        (r, score)
    }

    fn vote_of(views: &[(Refinement, f64)]) -> (Option<f64>, String) {
        let ev: Vec<ViewEvidence> = views
            .iter()
            .map(|(r, s)| ViewEvidence {
                refinement: r,
                score: *s,
                ds_m: 0.0,
                h_cloud: None,
                crop: None,
            })
            .collect();
        roof_vote(&ev)
    }

    #[test]
    fn the_roof_vote_ranks_the_evidence() {
        // a confirmed sky view decides on its own, edges or no edges
        let v = vec![
            roof_view(20.0, ROOF_SKY, false, 0.3),
            roof_view(12.0, ROOF_EDGE, false, 0.9),
        ];
        assert_eq!(vote_of(&v), (Some(20.0), ROOF_SKY.to_string()));
        // the MEASURED.md case: the only sky view is one the C21 rule could not
        // confirm, so the edge view decides instead
        let v = vec![
            roof_view(8.47, ROOF_SKY, true, 0.398),
            roof_view(13.55, ROOF_EDGE, false, 0.091),
        ];
        assert_eq!(vote_of(&v), (Some(13.55), ROOF_EDGE.to_string()));
        // with nothing else, an unconfirmed sky view is still better than no
        // answer at all
        let v = vec![roof_view(8.47, ROOF_SKY, true, 0.4)];
        assert_eq!(vote_of(&v), (Some(8.47), ROOF_SKY.to_string()));
        // inside a family the views are weighted by score
        let v = vec![
            roof_view(10.0, ROOF_SKY, false, 0.9),
            roof_view(20.0, ROOF_SKY, false, 0.1),
        ];
        let (h, _) = vote_of(&v);
        assert!(h.unwrap() < 12.0, "{h:?}");
        // and equal scores reproduce the unweighted median the rule used to take
        let v = vec![
            roof_view(10.0, ROOF_SKY, false, 0.5),
            roof_view(20.0, ROOF_SKY, false, 0.5),
        ];
        assert_eq!(vote_of(&v).0, Some(15.0));
        // no roofline at all
        assert_eq!(vote_of(&[]), (None, ROOF_OSM.to_string()));
    }

    /// A crop with a known frame, for the geometry helpers.
    fn crop_of(w: u32, h: u32, ppm: f64) -> LooseCrop {
        LooseCrop {
            wall_key: "w1_0".into(),
            pano_id: "p".into(),
            rgb: RgbImage::new(w, h),
            occl: vec![0; (w * h) as usize],
            ppm,
            s0: -2.0,
            h_bot: -1.0,
            h_top: h as f64 / ppm - 1.0,
            x_foot: 0.5 * w as f64,
            y_cam: h as f64 - 2.0 * ppm,
            z_base: 0.0,
            z_base_source: "pano".into(),
        }
    }

    #[test]
    fn the_shear_moves_the_top_of_the_wall_and_leaves_the_camera_row() {
        let crop = crop_of(200, 300, 10.0);
        let h = shear_homography(&crop, 2.0, 0.0);
        let s_cam = crop.x_to_s(crop.x_foot);
        let h_cam = crop.y_to_h(crop.y_cam);
        // at the camera's own height nothing moves
        let (s, _) = apply_h(&h, s_cam + 3.0, h_cam);
        assert!((s - (s_cam + 3.0)).abs() < 1e-9, "{s}");
        // ten metres above it, a 2 degree lean moves a point by tan(2) * 10
        let (s, hh) = apply_h(&h, s_cam, h_cam + 10.0);
        assert!(
            (s - (s_cam - 2.0f64.to_radians().tan() * 10.0)).abs() < 1e-6,
            "{s}"
        );
        assert!((hh - (h_cam + 10.0)).abs() < 1e-6, "{hh}");
    }

    #[test]
    fn the_joint_extent_prefers_the_pair_that_is_the_osm_length_apart() {
        let p = Params::default();
        let peaks = vec![
            Peak {
                x_m: 0.2,
                energy: 4.0,
                vlen_m: 3.0,
            },
            Peak {
                x_m: 1.4,
                energy: 4.5,
                vlen_m: 3.0,
            },
            Peak {
                x_m: 10.1,
                energy: 4.0,
                vlen_m: 3.0,
            },
        ];
        // both ends found, and the pair 9.9 m apart beats the slightly stronger
        // one that would make the wall 8.7 m
        let e = extent_joint(&peaks, 0.0, 10.0, 9.9, None, false, false, &p);
        assert_eq!((e.s_l, e.s_r), (Some(0.2), Some(10.1)));
        assert_eq!(
            (e.src_a.as_str(), e.src_b.as_str()),
            ("edge-joint", "edge-joint")
        );
        // only the left end: the right one is translated, not resized
        let e = extent_joint(&peaks[..2], 0.0, 10.0, 9.9, None, false, false, &p);
        assert_eq!(e.s_l, Some(1.4));
        assert_eq!(e.s_r, Some(11.4));
        assert_eq!(e.src_b, "edge-shift");
        // a cloud corner pins its end
        let e = extent_joint(&peaks, 0.0, 10.0, 9.9, None, true, false, &p);
        assert_eq!(e.s_l, None);
        assert_eq!(e.s_r, Some(10.1));
        // nothing near either end keeps both
        let e = extent_joint(&[], 0.0, 10.0, 9.9, None, false, false, &p);
        assert_eq!((e.s_l, e.s_r, e.src_a.as_str()), (None, None, "osm"));
    }

    #[test]
    fn the_phase_search_lands_the_grid_on_the_facade() {
        // a texture with a dark band every 8 px starting 3 px in, which is a
        // grid the block edges should land between
        let ppb = 8usize;
        let (w, h) = (ppb * 8, ppb * 4);
        let mut tex = RgbImage::new(w as u32, h as u32);
        for y in 0..h {
            for x in 0..w {
                let dark = (x + ppb - 3) % ppb < 3 && y % ppb > 1;
                let v = if dark { 30 } else { 200 };
                tex.put_pixel(x as u32, y as u32, Rgb([v, v, v]));
            }
        }
        let valid = vec![true; w * h];
        let (dx, _dy, bm) = phase_search(&tex, &valid, ppb, &Params::default());
        assert!(bm > 0.0, "bimodality {bm}");
        assert!(dx.abs() <= 0.5, "dx {dx}");
    }

    #[test]
    fn the_colour_trim_only_eats_the_ends() {
        let ppb = 4usize;
        let (w, h) = (ppb * 9, ppb);
        let mut tex = RgbImage::new(w as u32, h as u32);
        for y in 0..h {
            for x in 0..w {
                // the first block column is a different colour, the rest is wall
                let c = if x < ppb {
                    Rgb([200, 40, 40])
                } else {
                    Rgb([150, 150, 150])
                };
                tex.put_pixel(x as u32, y as u32, c);
            }
        }
        let valid = vec![true; w * h];
        let medoid = wall_medoid(&tex, &valid);
        let (a, b) = colour_trim(&tex, &valid, ppb, medoid, 2, 0.08);
        assert_eq!((a, b), (1, 0));
        // a wall of one colour loses nothing
        let flat = RgbImage::from_pixel(w as u32, h as u32, Rgb([150, 150, 150]));
        assert_eq!(
            colour_trim(&flat, &valid, ppb, wall_medoid(&flat, &valid), 2, 0.08),
            (0, 0)
        );
    }

    #[test]
    fn finalise_wall_trims_only_the_ends_osm_decided() {
        let params = Params::default();
        let ppb = 8usize;
        let (w, h) = (ppb * 6, ppb * 3);
        let mut tex = RgbImage::new(w as u32, h as u32);
        for y in 0..h {
            for x in 0..w {
                let c = if x < ppb {
                    Rgb([40, 200, 60])
                } else {
                    Rgb([160, 158, 155])
                };
                tex.put_pixel(x as u32, y as u32, c);
            }
        }
        let valid = vec![true; w * h];
        let mut dec = WallDecision {
            wall_key: "w1_0".into(),
            s_l: 0.0,
            s_r: 6.0,
            src_a: "osm".into(),
            src_b: "osm".into(),
            trimmed_a: 0,
            trimmed_b: 0,
            h_used: 3.0,
            height_source: "levels".into(),
            h_sky: None,
            h_cloud: None,
            h_osm: Some(3.0),
            phase: [0.0, 0.0],
            bimodality: 0.0,
            flags: vec![],
        };
        finalise_wall(&mut dec, &tex, &valid, ppb, None, &params);
        assert_eq!((dec.trimmed_a, dec.trimmed_b), (1, 0));
        assert!((dec.s_l - 1.0).abs() < 1e-9);
        assert!(dec.flags.iter().any(|f| f == "TRIMMED 1/0"));
        assert_eq!(final_rect(&dec), (1.0, 6.0, 0.0, 3.0));
        // an end the picture decided is never trimmed
        let mut dec2 = WallDecision {
            src_a: "edge-joint".into(),
            s_l: 0.0,
            ..dec.clone()
        };
        dec2.flags.clear();
        finalise_wall(&mut dec2, &tex, &valid, ppb, None, &params);
        assert_eq!(dec2.trimmed_a, 0);
    }

    #[test]
    fn the_image_primitives_match_opencv() {
        // one pixel through each conversion, against what cv2.cvtColor answers
        let mut img = RgbImage::new(1, 1);
        img.put_pixel(0, 0, Rgb([154, 160, 171]));
        assert_eq!(to_gray(&img)[0], 159);
        assert_eq!(rgb_to_hsv(&img)[0], [109, 25, 171]);
        img.put_pixel(0, 0, Rgb([20, 120, 200]));
        assert_eq!(to_gray(&img)[0], 99);
        assert_eq!(rgb_to_hsv(&img)[0], [103, 229, 200]);
        // Lab within a unit: OpenCV's byte path goes through interpolated
        // lookup tables where this one evaluates the formula
        for (rgb, want) in [
            ([154u8, 160, 171], [168i32, 128, 122]),
            ([20, 120, 200], [126, 131, 79]),
            ([0, 0, 0], [0, 128, 128]),
            ([255, 255, 255], [255, 128, 128]),
        ] {
            let got = rgb_to_lab_bytes(rgb);
            for c in 0..3 {
                assert!(
                    (got[c] as i32 - want[c]).abs() <= 1,
                    "{rgb:?} lab {got:?} against {want:?}"
                );
            }
        }
        // Otsu on a two valued histogram splits it between the two values
        let mut hist = [0usize; 256];
        hist[10] = 100;
        hist[200] = 100;
        let t = otsu_threshold(&hist);
        assert!((10.0..200.0).contains(&t), "{t}");
        // the Sobel of a step is the step, with the reflecting border
        let src: Vec<f32> = vec![0.0, 0.0, 4.0, 4.0, 0.0, 0.0, 4.0, 4.0];
        let gx = sobel(&src, 4, 2, 1, 0);
        assert_eq!(gx[0], 0.0);
        assert_eq!(gx[1], 16.0);
        assert_eq!(gx[2], 16.0);
    }

    #[test]
    fn the_row_energy_finds_a_horizontal_edge() {
        let img = synthetic(60, 40, (0, 20, 60, 40));
        let e = row_energy(&img, None, None);
        let best = strongest_row(&e, 0, 40, 2.0, None).expect("an edge");
        assert!((19..=20).contains(&best), "row {best}");
        // the sub-pixel position of a step between rows 19 and 20 is the
        // boundary at 20
        let y = edge_y(&e, best);
        assert!((y - 20.0).abs() < 0.6, "y {y}");
    }
}
