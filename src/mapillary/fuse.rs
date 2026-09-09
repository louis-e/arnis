//! Combining the views of one wall. Port of `tools/facade_lab/fuse.py`.
//!
//! Parked cars and pedestrians move between captures and the wall does not, so
//! several views median-combined lose them. Every view is already rendered onto
//! the same wall rectangle in wall coordinates, so fusion never compares z
//! values across clusters: a view rendered from another cluster with a different
//! z datum still lands on the same rectangle because its own `z_base` was used
//! to render it. What is left over (plane offset, along-wall shift, datum error)
//! is measured here by phase correlation and either corrected, when it is under
//! 1.5 m, or reported as a disagreement.
//!
//! The order is: align every view to the best one, exposure match it in OkLab
//! inside the overlap so seams vanish, take a per texel weighted median, then
//! fill the holes narrower than 1.5 m. A texel no view could see stays no-data
//! rather than being invented; a hole wider than that stays a hole.
//!
//! Two numbers decide the shape of the answer. Above 0.75 m of median residual
//! the views are not describing the same wall and the texture is the best view
//! alone, with the classes combined later at block level. And the weight floor
//! under the median is **relative to the best weight in play**, not absolute:
//! with an absolute 0.25 two valid but low scoring views fused to an empty
//! texture, which the tree audit found by losing whole walls.
//!
//! Phase correlation is the one signal-processing piece the port has to supply
//! itself, and `cv2.phaseCorrelate` is reproduced rather than reinvented,
//! because the shift it returns is applied to the pixels and a different
//! convention would move every view by half a texel. That means: pad both
//! images to the next size whose factors are 2, 3 and 5; multiply by
//! `sqrt(hann_rows * hann_cols)`, which is what `createHanningWindow` actually
//! returns; cross power spectrum normalised bin by bin; inverse transform
//! unscaled; roll by half the size; and take the weighted centroid of a five by
//! five box about the peak, clamped at the image edge.
//!
//! Where this differs from the Python, and why:
//!
//! * `cv2.warpAffine`'s 1/32 pixel quantisation of the shift is reproduced,
//!   because it is systematic, but the interpolation weights are exact rather
//!   than its 15 bit fixed point table; that is worth at most one grey level.
//! * The fast marching queue is a binary heap ordered by `(T, insertion)`. Fast
//!   marching itself is order independent, so the distance field is the same,
//!   but Telea's colour of a texel can depend on the order equal-distance
//!   neighbours were filled in, and OpenCV's own heap breaks those ties its own
//!   way. Holes are 1.8 per cent of a texture on average.
//! * Nothing else. `cv2.cvtColor(RGB2GRAY)` is the 15 bit fixed point form the
//!   vectorised OpenCV path uses, as in `rectify.rs`.
//!
//! One thing found while porting **and** fixed in the Python first, so the
//! fixtures carry it: [`align_views`] used to add a view's residual to the
//! agreement median whether or not the correlation peak that produced it meant
//! anything. The shift was applied only above `MIN_PHASE_RESPONSE`, but a peak
//! with a response of -0.003, which is noise, still voted on whether the wall
//! was fused or handed over as the best view alone, and the position of a noise
//! maximum moves anywhere when the input moves by a grey level. Now a residual
//! below the floor is not a measurement: it does not vote, and the view it came
//! from takes no part in the median either, since a view that could not be
//! aligned cannot be trusted onto the rectangle. Simply dropping it from the
//! vote and fusing it in anyway was measured and is worse: seven Munich walls
//! then blend an unalignable view in and four of them ghost visibly.
//! `MEASURED.md`, "The residual of a peak that means nothing", has the numbers.

#![allow(dead_code)]

use std::f64::consts::PI;

use image::RgbImage;

use super::imgops::{self, Mask};
use super::types::Params;

/// Above this median residual between the views the texture is the best view
/// alone and the caller combines classes at block level instead.
pub const AGREEMENT_MAX_M: f64 = 0.75;
/// A residual larger than this is measured but not applied.
pub const MAX_SHIFT_M: f64 = 1.5;
/// The border weight rises from its floor to 1 over this distance inside the
/// valid area.
pub const BORDER_FALLOFF_M: f64 = 1.0;
pub const BORDER_FLOOR: f64 = 0.15;
/// The weight floor of the median, as a share of the best weight in play.
pub const MIN_WEIGHT_SUM: f64 = 0.25;
/// A phase correlation peak weaker than this says nothing about the shift.
pub const MIN_PHASE_RESPONSE: f64 = 0.03;
/// Two views have to share this many texels before their shift is worth
/// measuring.
pub const MIN_OVERLAP_PX: usize = 256;
/// Holes wider than this stay no-data.
pub const HOLE_MAX_M: f64 = 1.5;
pub const INPAINT_RADIUS_PX: i32 = 3;

/// How the texture of a wall was arrived at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FuseMode {
    /// Several views, aligned, exposure matched and median combined.
    Fused,
    /// The views disagreed by more than `AGREEMENT_MAX_M`, so only the best one
    /// is on the texture.
    BestView,
    /// One view, passed through.
    Single,
}

impl FuseMode {
    pub fn as_str(self) -> &'static str {
        match self {
            FuseMode::Fused => "fused",
            FuseMode::BestView => "best_view",
            FuseMode::Single => "single",
        }
    }
}

/// The fused texture of one wall.
#[derive(Clone, Debug)]
pub struct FusedTexture {
    pub wall_key: String,
    pub rgb: image::RgbImage,
    /// Row major, true where at least one view could see the texel.
    pub valid: Vec<bool>,
    pub ppm: f64,
    /// Per view, the shift phase correlation found, in metres.
    pub shifts: Vec<[f64; 2]>,
    /// Median disagreement between the views, in metres. This is the number the
    /// confidence's `views` factor is built on.
    pub agreement_m: f64,
    pub mode: FuseMode,
    /// Index of the best view in the input order.
    pub best_index: usize,
    /// Share of the texture that was inpainted rather than seen.
    pub hole_fraction: f64,
    /// Row major, true on the texels the hole filling invented.
    pub filled: Vec<bool>,
    pub flags: Vec<String>,
}

/// One view's contribution: its texture on the wall rectangle, the texels it
/// could see, and how much the selection trusted it.
#[derive(Clone, Debug)]
pub struct ViewTexture {
    pub pano_id: String,
    pub rgb: RgbImage,
    pub valid: Vec<bool>,
    pub score: f64,
}

// --------------------------------------------------------------------------- gradients and shifts

/// `cv2.cvtColor(RGB2GRAY)` on bytes: the 15 bit fixed point weights of the
/// vectorised path, which is the one OpenCV 4 actually takes. The 14 bit
/// constants in its own header are a rounding away on a quarter of a per cent
/// of pixels, which was measured over 160 000 random colours.
#[inline]
fn rgb_to_gray(p: [u8; 3]) -> f64 {
    let acc = i64::from(p[0]) * 9798 + i64::from(p[1]) * 19235 + i64::from(p[2]) * 3735 + 16384;
    (acc >> 15) as f64
}

#[inline]
fn reflect101(i: isize, n: isize) -> usize {
    if n == 1 {
        return 0;
    }
    let period = 2 * (n - 1);
    let mut j = i.rem_euclid(period);
    if j >= n {
        j = period - j;
    }
    j as usize
}

/// Sobel gradient magnitude of the grey image on a 0..1 scale, zero outside the
/// valid area and on its one pixel border so a mask edge does not correlate as
/// if it were a window frame.
pub fn gradient_magnitude(rgb: &RgbImage, valid: Option<&[bool]>) -> Vec<f64> {
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let gray: Vec<f64> = rgb.pixels().map(|p| rgb_to_gray(p.0) / 255.0).collect();
    let kx = [-1.0f64, 0.0, 1.0];
    let ky = [1.0f64, 2.0, 1.0];
    let mut mag = vec![0.0f64; w * h];
    for y in 0..h {
        for x in 0..w {
            let (mut gx, mut gy) = (0.0f64, 0.0f64);
            for (i, &wy) in ky.iter().enumerate() {
                let sy = reflect101(y as isize + i as isize - 1, h as isize);
                for (j, &wx) in kx.iter().enumerate() {
                    let sx = reflect101(x as isize + j as isize - 1, w as isize);
                    let v = gray[sy * w + sx];
                    gx += wy * wx * v;
                    gy += kx[i] * ky[j] * v;
                }
            }
            mag[y * w + x] = gx.hypot(gy);
        }
    }
    if let Some(v) = valid {
        let eroded = imgops::erode(&Mask::from_bits(w, h, v.to_vec()), 3, 3);
        for (cell, on) in mag.iter_mut().zip(eroded.bits.iter()) {
            if !on {
                *cell = 0.0;
            }
        }
    }
    mag
}

/// OpenCV's `warpAffine` quantises a translation to 1/32 of a pixel before it
/// interpolates, and the quantised value is what actually moves the picture.
#[inline]
fn quantised_shift(d: f64) -> f64 {
    let fixed = imgops::round_half_even(-d * 1024.0) as i64;
    ((fixed + 16) >> 5) as f64 / 32.0
}

/// Translate an image by `(dx, dy)` pixels, positive right and down; outside is
/// black.
pub fn shift_rgb(img: &RgbImage, dx: f64, dy: f64) -> RgbImage {
    if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
        return img.clone();
    }
    let (w, h) = (img.width() as i64, img.height() as i64);
    let (qx, qy) = (quantised_shift(dx), quantised_shift(dy));
    let mut out = RgbImage::new(img.width(), img.height());
    for y in 0..h {
        for x in 0..w {
            let sx = x as f64 + qx;
            let sy = y as f64 + qy;
            let (x0, y0) = (sx.floor() as i64, sy.floor() as i64);
            let (tx, ty) = (sx - x0 as f64, sy - y0 as f64);
            let mut px = [0u8; 3];
            for (c, cell) in px.iter_mut().enumerate() {
                let at = |xx: i64, yy: i64| -> f64 {
                    if xx < 0 || yy < 0 || xx >= w || yy >= h {
                        0.0
                    } else {
                        f64::from(img.get_pixel(xx as u32, yy as u32).0[c])
                    }
                };
                let v = at(x0, y0) * (1.0 - tx) * (1.0 - ty)
                    + at(x0 + 1, y0) * tx * (1.0 - ty)
                    + at(x0, y0 + 1) * (1.0 - tx) * ty
                    + at(x0 + 1, y0 + 1) * tx * ty;
                *cell = (v + 0.5).floor().clamp(0.0, 255.0) as u8;
            }
            out.put_pixel(x as u32, y as u32, image::Rgb(px));
        }
    }
    out
}

/// The same translation on a mask, nearest neighbour, which is how a validity
/// mask has to move: a half-covered texel is either seen or it is not.
pub fn shift_mask(mask: &[bool], w: usize, h: usize, dx: f64, dy: f64) -> Vec<bool> {
    if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
        return mask.to_vec();
    }
    // INTER_NEAREST rounds the source coordinate instead of quantising it.
    let qx = -(imgops::round_half_even(-dx * 1024.0) as i64 + 512).div_euclid(1024);
    let qy = -(imgops::round_half_even(-dy * 1024.0) as i64 + 512).div_euclid(1024);
    let mut out = vec![false; w * h];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let (sx, sy) = (x - qx, y - qy);
            if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                out[y as usize * w + x as usize] = mask[sy as usize * w + sx as usize];
            }
        }
    }
    out
}

// --------------------------------------------------------------------------- the transform

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Cx {
    re: f64,
    im: f64,
}

impl Cx {
    #[inline]
    fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    #[inline]
    fn add(self, o: Self) -> Self {
        Self::new(self.re + o.re, self.im + o.im)
    }
    #[inline]
    fn mul(self, o: Self) -> Self {
        Self::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
    #[inline]
    fn conj(self) -> Self {
        Self::new(self.re, -self.im)
    }
    #[inline]
    fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
}

/// The smallest size at least `n` whose only prime factors are 2, 3 and 5,
/// which is what `cv2.getOptimalDFTSize` returns and what the padding uses.
pub fn optimal_dft_size(n: usize) -> usize {
    let mut m = n.max(1);
    loop {
        let mut r = m;
        for p in [2usize, 3, 5] {
            while r.is_multiple_of(p) {
                r /= p;
            }
        }
        if r == 1 {
            return m;
        }
        m += 1;
    }
}

fn smallest_factor(n: usize) -> usize {
    let mut p = 2;
    while p * p <= n {
        if n.is_multiple_of(p) {
            return p;
        }
        p += 1;
    }
    n
}

/// Mixed radix discrete Fourier transform, `sign` -1 forward and +1 inverse,
/// unscaled in both directions the way OpenCV's `dft`/`idft` are without
/// `DFT_SCALE`.
fn dft_1d(x: &[Cx], sign: f64, roots: &[Cx]) -> Vec<Cx> {
    let n = x.len();
    if n == 1 {
        return x.to_vec();
    }
    let p = smallest_factor(n);
    let m = n / p;
    let step = roots.len() / n;
    if p == n {
        // A prime length: the direct sum, which is what the recursion bottoms
        // out on and is never large here.
        let mut out = vec![Cx::default(); n];
        for (k, cell) in out.iter_mut().enumerate() {
            let mut acc = Cx::default();
            for (j, v) in x.iter().enumerate() {
                acc = acc.add(v.mul(roots[(j * k * step) % roots.len()]));
            }
            *cell = acc;
        }
        let _ = sign;
        return out;
    }
    let mut subs: Vec<Vec<Cx>> = Vec::with_capacity(p);
    for r in 0..p {
        let sub: Vec<Cx> = (0..m).map(|k| x[k * p + r]).collect();
        subs.push(dft_1d(&sub, sign, roots));
    }
    let mut out = vec![Cx::default(); n];
    for (k, cell) in out.iter_mut().enumerate() {
        let mut acc = Cx::default();
        for (r, sub) in subs.iter().enumerate() {
            acc = acc.add(sub[k % m].mul(roots[(r * k * step) % roots.len()]));
        }
        *cell = acc;
    }
    out
}

/// `exp(sign * 2 pi i k / n)` for every `k`, so the recursion never calls a
/// trigonometric function.
fn roots_of_unity(n: usize, sign: f64) -> Vec<Cx> {
    (0..n)
        .map(|k| {
            let a = sign * 2.0 * PI * k as f64 / n as f64;
            Cx::new(a.cos(), a.sin())
        })
        .collect()
}

fn dft_2d(data: &mut [Cx], w: usize, h: usize, sign: f64) {
    let rw = roots_of_unity(w, sign);
    let rh = roots_of_unity(h, sign);
    for y in 0..h {
        let row: Vec<Cx> = data[y * w..(y + 1) * w].to_vec();
        let out = dft_1d(&row, sign, &rw);
        data[y * w..(y + 1) * w].copy_from_slice(&out);
    }
    let mut col = vec![Cx::default(); h];
    for x in 0..w {
        for (y, cell) in col.iter_mut().enumerate() {
            *cell = data[y * w + x];
        }
        let out = dft_1d(&col, sign, &rh);
        for (y, v) in out.iter().enumerate() {
            data[y * w + x] = *v;
        }
    }
}

/// `cv2.createHanningWindow`, which is the square root of the product of the
/// two one dimensional Hann windows.
pub fn hanning_window(w: usize, h: usize) -> Vec<f64> {
    let cw: Vec<f64> = (0..w)
        .map(|j| {
            if w <= 1 {
                0.0
            } else {
                0.5 * (1.0 - (2.0 * PI * j as f64 / (w - 1) as f64).cos())
            }
        })
        .collect();
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        let wr = if h <= 1 {
            0.0
        } else {
            0.5 * (1.0 - (2.0 * PI * y as f64 / (h - 1) as f64).cos())
        };
        for x in 0..w {
            out[y * w + x] = (wr * cw[x]).max(0.0).sqrt();
        }
    }
    out
}

/// `(dx, dy, response)`: the displacement of `other` relative to `ref` in
/// pixels, positive meaning other's content lies further right and down, as
/// `cv2.phaseCorrelate` defines it. Shifting `other` by `(-dx, -dy)` aligns it.
pub fn phase_correlate(a: &[f64], b: &[f64], w: usize, h: usize) -> (f64, f64, f64) {
    if w * h < 16 {
        return (0.0, 0.0, 0.0);
    }
    let amax = a.iter().copied().fold(f64::MIN, f64::max);
    let bmax = b.iter().copied().fold(f64::MIN, f64::max);
    if amax <= 0.0 || bmax <= 0.0 {
        return (0.0, 0.0, 0.0);
    }
    let (n, m) = (optimal_dft_size(w), optimal_dft_size(h));
    let win = hanning_window(w, h);
    let mut fa = vec![Cx::default(); n * m];
    let mut fb = vec![Cx::default(); n * m];
    for y in 0..h {
        for x in 0..w {
            let g = win[y * w + x];
            fa[y * n + x] = Cx::new(a[y * w + x] * g, 0.0);
            fb[y * n + x] = Cx::new(b[y * w + x] * g, 0.0);
        }
    }
    dft_2d(&mut fa, n, m, -1.0);
    dft_2d(&mut fb, n, m, -1.0);
    // The cross power spectrum, every bin normalised by its own magnitude. The
    // eps is OpenCV's, and it is what keeps a bin with no energy at zero rather
    // than at a nan.
    let mut c = vec![Cx::default(); n * m];
    let eps = f64::EPSILON;
    for cell in 0..n * m {
        let p = fa[cell].mul(fb[cell].conj());
        let mag = p.abs();
        c[cell] = Cx::new(
            p.re * mag / (mag * mag + eps),
            p.im * mag / (mag * mag + eps),
        );
    }
    dft_2d(&mut c, n, m, 1.0);
    // fftShift is a roll by half the size in each direction.
    let mut surf = vec![0.0f64; n * m];
    for y in 0..m {
        for x in 0..n {
            surf[((y + m / 2) % m) * n + (x + n / 2) % n] = c[y * n + x].re;
        }
    }
    let mut peak = 0usize;
    for i in 1..surf.len() {
        if surf[i] > surf[peak] {
            peak = i;
        }
    }
    let (px, py) = ((peak % n) as isize, (peak / n) as isize);
    let (minr, maxr) = (py.max(2) - 2, (py + 2).min(m as isize - 1));
    let (minc, maxc) = (px.max(2) - 2, (px + 2).min(n as isize - 1));
    let (mut cx, mut cy, mut total) = (0.0f64, 0.0f64, 0.0f64);
    for y in minr..=maxr {
        for x in minc..=maxc {
            let v = surf[y as usize * n + x as usize];
            cx += x as f64 * v;
            cy += y as f64 * v;
            total += v;
        }
    }
    let response = total / (n * m) as f64;
    let denom = total + f64::EPSILON;
    let (cx, cy) = (cx / denom, cy / denom);
    (n as f64 / 2.0 - cx, m as f64 / 2.0 - cy, response)
}

// --------------------------------------------------------------------------- weights

/// The exact Euclidean distance to the nearest false texel, by the squared
/// distance transform with the lower envelope of parabolas.
pub fn distance_to_false(mask: &[bool], w: usize, h: usize) -> Vec<f64> {
    let inf = f64::INFINITY;
    let mut f = vec![0.0f64; w * h];
    for i in 0..w * h {
        f[i] = if mask[i] { inf } else { 0.0 };
    }
    let mut col = vec![0.0f64; h.max(w)];
    let mut v = vec![0usize; h.max(w) + 1];
    let mut z = vec![0.0f64; h.max(w) + 2];
    // Down the columns, then along the rows: the two passes of the separable
    // transform.
    for x in 0..w {
        for y in 0..h {
            col[y] = f[y * w + x];
        }
        let out = envelope(&col[..h], &mut v, &mut z);
        for y in 0..h {
            f[y * w + x] = out[y];
        }
    }
    for y in 0..h {
        let row: Vec<f64> = f[y * w..(y + 1) * w].to_vec();
        let out = envelope(&row, &mut v, &mut z);
        for x in 0..w {
            f[y * w + x] = out[x].max(0.0).sqrt();
        }
    }
    f
}

fn envelope(f: &[f64], v: &mut [usize], z: &mut [f64]) -> Vec<f64> {
    let n = f.len();
    let mut out = vec![0.0f64; n];
    if n == 0 {
        return out;
    }
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        if !f[q].is_finite() {
            continue;
        }
        loop {
            let p = v[k];
            let s = if !f[p].is_finite() {
                f64::NEG_INFINITY
            } else {
                ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64))
                    / (2.0 * q as f64 - 2.0 * p as f64)
            };
            if s <= z[k] && k > 0 {
                k -= 1;
                continue;
            }
            if s <= z[k] && k == 0 {
                v[0] = q;
                z[0] = f64::NEG_INFINITY;
                z[1] = f64::INFINITY;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = f64::INFINITY;
            }
            break;
        }
    }
    let mut k = 0usize;
    for (q, cell) in out.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        *cell = if f[p].is_finite() {
            let d = q as f64 - p as f64;
            d * d + f[p]
        } else {
            f64::INFINITY
        };
    }
    out
}

/// Cosine fall-off from the floor at the valid border to 1 a metre inside, and
/// zero outside. A single view still contributes at its own edge, which is what
/// the floor is for.
pub fn border_weight(valid: &[bool], w: usize, h: usize, ppb: u32) -> Vec<f64> {
    if !valid.iter().any(|v| *v) {
        return vec![0.0; w * h];
    }
    // The pad is what makes the image border count as a border.
    let (pw, ph) = (w + 2, h + 2);
    let mut padded = vec![false; pw * ph];
    for y in 0..h {
        for x in 0..w {
            padded[(y + 1) * pw + x + 1] = valid[y * w + x];
        }
    }
    let dist = distance_to_false(&padded, pw, ph);
    let scale = (BORDER_FALLOFF_M * f64::from(ppb)).max(1.0);
    let mut out = vec![0.0f64; w * h];
    for y in 0..h {
        for x in 0..w {
            if !valid[y * w + x] {
                continue;
            }
            let r = (dist[(y + 1) * pw + x + 1] / scale).clamp(0.0, 1.0);
            let c = 0.5 - 0.5 * (PI * r).cos();
            out[y * w + x] = BORDER_FLOOR + (1.0 - BORDER_FLOOR) * c;
        }
    }
    out
}

// --------------------------------------------------------------------------- exposure and median

/// Match `other` to `ref` inside the overlap in OkLab: the L mean and spread and
/// the a and b means move onto the reference's, so a seam between two views
/// vanishes in the median instead of surviving as a step.
pub fn match_exposure(reference: &RgbImage, other: &RgbImage, overlap: &[bool]) -> RgbImage {
    let n = overlap.iter().filter(|v| **v).count();
    if n < 64 {
        return other.clone();
    }
    let lab_o: Vec<[f64; 3]> = other.pixels().map(|p| imgops::srgb_to_oklab(p.0)).collect();
    let lab_r: Vec<[f64; 3]> = reference
        .pixels()
        .map(|p| imgops::srgb_to_oklab(p.0))
        .collect();
    let mut mean_r = [0.0f64; 3];
    let mut mean_o = [0.0f64; 3];
    for (i, on) in overlap.iter().enumerate() {
        if !on {
            continue;
        }
        for c in 0..3 {
            mean_r[c] += lab_r[i][c];
            mean_o[c] += lab_o[i][c];
        }
    }
    for c in 0..3 {
        mean_r[c] /= n as f64;
        mean_o[c] /= n as f64;
    }
    let (mut vr, mut vo) = (0.0f64, 0.0f64);
    for (i, on) in overlap.iter().enumerate() {
        if !on {
            continue;
        }
        vr += (lab_r[i][0] - mean_r[0]).powi(2);
        vo += (lab_o[i][0] - mean_o[0]).powi(2);
    }
    let sd_r = (vr / n as f64).sqrt();
    let sd_o = (vo / n as f64).sqrt();
    let gain = if sd_o > 1e-4 && sd_r > 1e-4 {
        (sd_r / sd_o).clamp(0.5, 2.0)
    } else {
        1.0
    };
    let mut out = RgbImage::new(other.width(), other.height());
    for (i, px) in out.pixels_mut().enumerate() {
        let lab = [
            (lab_o[i][0] - mean_o[0]) * gain + mean_r[0],
            lab_o[i][1] - mean_o[1] + mean_r[1],
            lab_o[i][2] - mean_o[2] + mean_r[2],
        ];
        px.0 = imgops::oklab_to_rgb8(lab);
    }
    out
}

/// The per texel, per channel weighted median over the views.
///
/// The floor under the weight sum is a share of the best weight in play rather
/// than an absolute: with an absolute floor two valid but low scoring views
/// fused to a texture with nothing on it.
pub fn weighted_median(
    textures: &[RgbImage],
    weights: &[Vec<f64>],
    min_weight: f64,
) -> (RgbImage, Vec<bool>) {
    assert!(!textures.is_empty(), "the median needs at least one view");
    let (w, h) = (textures[0].width() as usize, textures[0].height() as usize);
    let n = w * h;
    let mut total = vec![0.0f64; n];
    let mut best = 0.0f64;
    for wt in weights {
        for (i, v) in wt.iter().enumerate() {
            total[i] += v;
            if *v > best {
                best = *v;
            }
        }
    }
    let floor = if best > 0.0 {
        min_weight.min(min_weight * best)
    } else {
        min_weight
    };
    let valid: Vec<bool> = total.iter().map(|t| *t > 0.0 && *t >= floor).collect();
    if textures.len() == 1 {
        return (textures[0].clone(), valid);
    }
    let mut out = RgbImage::new(w as u32, h as u32);
    let mut entries: Vec<(f64, f64)> = Vec::with_capacity(textures.len());
    for i in 0..n {
        let half = 0.5 * total[i];
        let px = out.get_pixel_mut((i % w) as u32, (i / w) as u32);
        for c in 0..3 {
            entries.clear();
            for (v, tex) in textures.iter().enumerate() {
                let value = f64::from(tex.as_raw()[i * 3 + c]);
                entries.push((value, weights[v][i]));
            }
            entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut cum = 0.0;
            let mut pick = entries[entries.len() - 1].0;
            for (value, weight) in entries.iter() {
                cum += weight;
                if cum >= half {
                    pick = *value;
                    break;
                }
            }
            px.0[c] = imgops::round_half_even(pick).clamp(0.0, 255.0) as u8;
        }
        if !valid[i] {
            px.0 = [0, 0, 0];
        }
    }
    (out, valid)
}

// --------------------------------------------------------------------------- hole filling

const TELEA_KNOWN: u8 = 0;
const TELEA_BAND: u8 = 1;
const TELEA_INSIDE: u8 = 2;
const TELEA_CHANGE: u8 = 3;

/// The fast marching queue: a binary heap on `(T, insertion order)`.
#[derive(Default)]
struct Fmm {
    heap: std::collections::BinaryHeap<std::cmp::Reverse<(ordered::F32, u64, usize, usize)>>,
    seq: u64,
}

mod ordered {
    /// A total order on `f32` for the queue. Fast marching never produces a NaN
    /// distance, so the only job here is to satisfy `Ord`.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct F32(pub f32);
    impl Eq for F32 {}
    #[allow(clippy::derive_ord_xor_partial_ord)]
    impl Ord for F32 {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.0
                .partial_cmp(&other.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        }
    }
    impl PartialOrd for F32 {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
}

impl Fmm {
    fn push(&mut self, i: usize, j: usize, t: f32) {
        self.heap
            .push(std::cmp::Reverse((ordered::F32(t), self.seq, i, j)));
        self.seq += 1;
    }

    fn pop(&mut self) -> Option<(usize, usize)> {
        self.heap
            .pop()
            .map(|std::cmp::Reverse((_, _, i, j))| (i, j))
    }
}

/// One arrival time from two already known neighbours, the eikonal solve Telea
/// and the narrow band both use.
fn fmm_solve(i1: usize, j1: usize, i2: usize, j2: usize, flags: &[u8], t: &[f32], w: usize) -> f32 {
    let a11 = t[i1 * w + j1];
    let a22 = t[i2 * w + j2];
    let m12 = a11.min(a22);
    let k1 = flags[i1 * w + j1] != TELEA_INSIDE;
    let k2 = flags[i2 * w + j2] != TELEA_INSIDE;
    let sol = if k1 && k2 {
        if (a11 - a22).abs() >= 1.0 {
            1.0 + f64::from(m12)
        } else {
            (f64::from(a11)
                + f64::from(a22)
                + (2.0 - f64::from(a11 - a22) * f64::from(a11 - a22)).sqrt())
                * 0.5
        }
    } else if k1 {
        1.0 + f64::from(a11)
    } else if k2 {
        1.0 + f64::from(a22)
    } else {
        1.0 + f64::from(m12)
    };
    sol as f32
}

/// The narrow band march that fills `t` outside the hole, so Telea's level term
/// can compare arrival times across the boundary.
fn fmm_march(flags: &mut [u8], t: &mut [f32], queue: &mut Fmm, w: usize, h: usize, negate: bool) {
    while let Some((ii, jj)) = queue.pop() {
        flags[ii * w + jj] = if negate { TELEA_CHANGE } else { TELEA_KNOWN };
        for q in 0..4 {
            let (i, j) = match q {
                0 => (ii as isize - 1, jj as isize),
                1 => (ii as isize, jj as isize - 1),
                2 => (ii as isize + 1, jj as isize),
                _ => (ii as isize, jj as isize + 1),
            };
            if i <= 0 || j <= 0 || i > h as isize - 1 || j > w as isize - 1 {
                continue;
            }
            let (i, j) = (i as usize, j as usize);
            if flags[i * w + j] != TELEA_INSIDE {
                continue;
            }
            let d = fmm_solve(i - 1, j, i, j - 1, flags, t, w)
                .min(fmm_solve(i + 1, j, i, j - 1, flags, t, w))
                .min(fmm_solve(i - 1, j, i, j + 1, flags, t, w))
                .min(fmm_solve(i + 1, j, i, j + 1, flags, t, w));
            t[i * w + j] = d;
            flags[i * w + j] = TELEA_BAND;
            queue.push(i, j, d);
        }
    }
    if negate {
        for i in 0..w * h {
            if flags[i] == TELEA_CHANGE {
                flags[i] = TELEA_KNOWN;
                t[i] = -t[i];
            }
        }
    }
}

/// `cv2.inpaint` with `INPAINT_TELEA`: every hole texel is the weighted average
/// of the known pixels within `range`, weighted by direction, distance and how
/// close their arrival time is, with a first order term from the local gradient.
fn inpaint_telea(rgb: &mut RgbImage, hole: &[bool], range: i32) {
    let (iw, ih) = (rgb.width() as usize, rgb.height() as usize);
    let (w, h) = (iw + 2, ih + 2);
    let mut mask = vec![TELEA_KNOWN; w * h];
    for y in 0..ih {
        for x in 0..iw {
            if hole[y * iw + x] {
                mask[(y + 1) * w + x + 1] = TELEA_INSIDE;
            }
        }
    }
    let mut t = vec![1.0e6f32; w * h];
    // The narrow band is the four-neighbour dilation of the hole minus itself.
    let mut band = vec![false; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            if mask[y * w + x] == TELEA_INSIDE {
                continue;
            }
            if mask[(y - 1) * w + x] == TELEA_INSIDE
                || mask[(y + 1) * w + x] == TELEA_INSIDE
                || mask[y * w + x - 1] == TELEA_INSIDE
                || mask[y * w + x + 1] == TELEA_INSIDE
                || mask[y * w + x] == TELEA_INSIDE
            {
                band[y * w + x] = true;
            }
        }
    }
    let mut inward = Fmm::default();
    for y in 0..h {
        for x in 0..w {
            if band[y * w + x] {
                t[y * w + x] = 0.0;
                inward.push(y, x, 0.0);
            }
        }
    }
    // Outward: the ring from the band out to `range`, marched and then negated,
    // which is what gives the level term a signed arrival time.
    let r = range as usize;
    let mut out_flags = vec![TELEA_KNOWN; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            if mask[y * w + x] == TELEA_INSIDE || band[y * w + x] {
                continue;
            }
            let (y0, y1) = (y.saturating_sub(r), (y + r).min(h - 1));
            let (x0, x1) = (x.saturating_sub(r), (x + r).min(w - 1));
            'k: for sy in y0..=y1 {
                for sx in x0..=x1 {
                    if mask[sy * w + sx] == TELEA_INSIDE {
                        out_flags[y * w + x] = TELEA_INSIDE;
                        break 'k;
                    }
                }
            }
        }
    }
    let mut outward = Fmm::default();
    for y in 0..h {
        for x in 0..w {
            if band[y * w + x] {
                outward.push(y, x, 0.0);
            }
        }
    }
    fmm_march(&mut out_flags, &mut t, &mut outward, w, h, true);
    // Inward: the hole itself, filled as the front reaches each texel.
    let raw = rgb.as_raw().clone();
    let mut pixels = raw;
    let get =
        |px: &[u8], y: usize, x: usize, c: usize| -> f64 { f64::from(px[(y * iw + x) * 3 + c]) };
    while let Some((ii, jj)) = inward.pop() {
        mask[ii * w + jj] = TELEA_KNOWN;
        for q in 0..4 {
            let (i, j) = match q {
                0 => (ii as isize - 1, jj as isize),
                1 => (ii as isize, jj as isize - 1),
                2 => (ii as isize + 1, jj as isize),
                _ => (ii as isize, jj as isize + 1),
            };
            if i <= 0 || j <= 0 || i > h as isize - 1 || j > w as isize - 1 {
                continue;
            }
            let (i, j) = (i as usize, j as usize);
            if mask[i * w + j] != TELEA_INSIDE {
                continue;
            }
            let dist = fmm_solve(i - 1, j, i, j - 1, &mask, &t, w)
                .min(fmm_solve(i + 1, j, i, j - 1, &mask, &t, w))
                .min(fmm_solve(i - 1, j, i, j + 1, &mask, &t, w))
                .min(fmm_solve(i + 1, j, i, j + 1, &mask, &t, w));
            t[i * w + j] = dist;
            let grad_t = telea_gradient(&mask, &t, w, i, j);
            for c in 0..3 {
                let (mut ia, mut jx, mut jy, mut s) = (0.0f64, 0.0f64, 0.0f64, 1.0e-20f64);
                let k0 = (i as isize - range as isize).max(0) as usize;
                let k1 = ((i as isize + range as isize) as usize).min(h - 1);
                let l0 = (j as isize - range as isize).max(0) as usize;
                let l1 = ((j as isize + range as isize) as usize).min(w - 1);
                for k in k0..=k1 {
                    if k == 0 || k >= h - 1 {
                        continue;
                    }
                    let km = k - 1 + usize::from(k == 1);
                    for l in l0..=l1 {
                        if l == 0 || l >= w - 1 {
                            continue;
                        }
                        if mask[k * w + l] == TELEA_INSIDE {
                            continue;
                        }
                        let (dy, dx) = (i as f64 - k as f64, j as f64 - l as f64);
                        if dx * dx + dy * dy > (range * range) as f64 {
                            continue;
                        }
                        let lm = l - 1 + usize::from(l == 1);
                        let lp = l - 1 - usize::from(l == w - 2);
                        let kp = k - 1 - usize::from(k == h - 2);
                        let len2 = dx * dx + dy * dy;
                        let dst = 1.0 / (len2 * len2.sqrt());
                        let lev = 1.0 / (1.0 + f64::from((t[k * w + l] - t[i * w + j]).abs()));
                        let mut dir = dx * grad_t.0 + dy * grad_t.1;
                        if dir.abs() <= 0.01 {
                            dir = 0.000_001;
                        }
                        let weight = (dst * lev * dir).abs();
                        let gx = if mask[k * w + l + 1] != TELEA_INSIDE {
                            if mask[k * w + l - 1] != TELEA_INSIDE {
                                (get(&pixels, km, lp + 1, c) - get(&pixels, km, lm - 1, c)) * 2.0
                            } else {
                                get(&pixels, km, lp + 1, c) - get(&pixels, km, lm, c)
                            }
                        } else if mask[k * w + l - 1] != TELEA_INSIDE {
                            get(&pixels, km, lm, c) - get(&pixels, km, lm - 1, c)
                        } else {
                            0.0
                        };
                        let gy = if mask[(k + 1) * w + l] != TELEA_INSIDE {
                            if mask[(k - 1) * w + l] != TELEA_INSIDE {
                                (get(&pixels, kp + 1, lm, c) - get(&pixels, km - 1, lm, c)) * 2.0
                            } else {
                                get(&pixels, kp + 1, lm, c) - get(&pixels, km, lm, c)
                            }
                        } else if mask[(k - 1) * w + l] != TELEA_INSIDE {
                            get(&pixels, km, lm, c) - get(&pixels, km - 1, lm, c)
                        } else {
                            0.0
                        };
                        ia += weight * get(&pixels, km, lm, c);
                        jx -= weight * gx * dx;
                        jy -= weight * gy * dy;
                        s += weight;
                    }
                }
                let sat = ia / s + (jx + jy) / ((jx * jx + jy * jy).sqrt() + 1.0e-20) + 0.5;
                pixels[((i - 1) * iw + (j - 1)) * 3 + c] =
                    imgops::round_half_even(sat).clamp(0.0, 255.0) as u8;
            }
            mask[i * w + j] = TELEA_BAND;
            inward.push(i, j, dist);
        }
    }
    for (i, px) in rgb.pixels_mut().enumerate() {
        px.0 = [pixels[i * 3], pixels[i * 3 + 1], pixels[i * 3 + 2]];
    }
}

/// The one sided or centred difference of the arrival time at a hole texel,
/// which is the direction the front is travelling in.
fn telea_gradient(mask: &[u8], t: &[f32], w: usize, i: usize, j: usize) -> (f64, f64) {
    let known = |y: usize, x: usize| mask[y * w + x] != TELEA_INSIDE;
    let tv = |y: usize, x: usize| f64::from(t[y * w + x]);
    let gx = if known(i, j + 1) {
        if known(i, j - 1) {
            (tv(i, j + 1) - tv(i, j - 1)) * 0.5
        } else {
            tv(i, j + 1) - tv(i, j)
        }
    } else if known(i, j - 1) {
        tv(i, j) - tv(i, j - 1)
    } else {
        0.0
    };
    let gy = if known(i + 1, j) {
        if known(i - 1, j) {
            (tv(i + 1, j) - tv(i - 1, j)) * 0.5
        } else {
            tv(i + 1, j) - tv(i, j)
        }
    } else if known(i - 1, j) {
        tv(i, j) - tv(i - 1, j)
    } else {
        0.0
    };
    (gx, gy)
}

/// What [`fill_holes`] answers: the texture with the narrow holes painted in,
/// the mask that now calls them valid, the share of the texture that was
/// invented, and which texels those were.
pub type HoleFill = (RgbImage, Vec<bool>, f64, Vec<bool>);

/// Fill the holes narrower than `max_hole_m`; wider ones stay no-data.
///
/// A hole's width is twice its largest distance to valid data, which is its
/// inscribed diameter: a long thin gap between two windows is filled, a whole
/// missing storey is not.
pub fn fill_holes(rgb: &RgbImage, valid: &[bool], ppb: u32, max_hole_m: f64) -> HoleFill {
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let n = w * h;
    let none = vec![false; n];
    if valid.iter().all(|v| *v) || !valid.iter().any(|v| *v) {
        return (rgb.clone(), valid.to_vec(), 0.0, none);
    }
    let holes: Vec<bool> = valid.iter().map(|v| !*v).collect();
    let labels = imgops::connected_components(&Mask::from_bits(w, h, holes.clone()));
    let dist = distance_to_false(&holes, w, h);
    let mut max_r = vec![0.0f64; labels.count()];
    for (id, d) in labels.labels.iter().zip(dist.iter()) {
        let id = *id as usize;
        if id != 0 && *d > max_r[id] {
            max_r[id] = *d;
        }
    }
    let limit = max_hole_m * f64::from(ppb);
    let filled: Vec<bool> = labels
        .labels
        .iter()
        .map(|id| *id != 0 && 2.0 * max_r[*id as usize] <= limit)
        .collect();
    if !filled.iter().any(|v| *v) {
        return (rgb.clone(), valid.to_vec(), 0.0, filled);
    }
    let mut out = rgb.clone();
    inpaint_telea(&mut out, &filled, INPAINT_RADIUS_PX);
    let new_valid: Vec<bool> = (0..n).map(|i| valid[i] || filled[i]).collect();
    let frac = filled.iter().filter(|v| **v).count() as f64 / n as f64;
    (out, new_valid, frac, filled)
}

// --------------------------------------------------------------------------- the public pass

/// What [`align_views`] answers: every view moved onto the best one, the valid
/// masks moved with them, the residual of each view in metres, the median of the
/// residuals that were measurable, and which views could be aligned at all.
pub type Alignment = (Vec<RgbImage>, Vec<Vec<bool>>, Vec<[f64; 2]>, f64, Vec<bool>);

/// Align every view to the best one by phase correlation on the masked gradient
/// magnitude.
///
/// A residual only exists where the correlation peak means something. Below
/// [`MIN_PHASE_RESPONSE`] the `(dx, dy)` the correlation reports is wherever the
/// noise happened to be highest: it is still reported in the shifts for the
/// debug dump, but the view is marked not usable, it is left where it is, and
/// its residual takes no part in the agreement. Above the floor the residual
/// votes whatever its size; it is applied only when it is under `max_shift_m`, a
/// larger one being a mis-registration to report rather than to paper over.
///
/// The agreement is the median of the residuals that were measured. When views
/// were correlated and none produced a peak worth trusting it is `max_shift_m`,
/// the largest residual the pipeline would ever repair: we cannot say how far
/// apart the views are, only that we could not bring them together. It is 0 when
/// there was nothing to correlate (a single view, or no overlap).
pub fn align_views(
    views: &[ViewTexture],
    ppb: u32,
    max_shift_m: f64,
    ref_index: usize,
) -> Alignment {
    let n = views.len();
    if n == 0 {
        return (Vec::new(), Vec::new(), Vec::new(), 0.0, Vec::new());
    }
    let (w, h) = (
        views[ref_index].rgb.width() as usize,
        views[ref_index].rgb.height() as usize,
    );
    let ref_valid = &views[ref_index].valid;
    let ref_mag = gradient_magnitude(&views[ref_index].rgb, Some(ref_valid));
    let mut aligned = Vec::with_capacity(n);
    let mut aligned_valid = Vec::with_capacity(n);
    let mut shifts = Vec::with_capacity(n);
    let mut residuals = Vec::new();
    let mut usable = vec![true; n];
    let mut correlated = 0usize;
    for (i, view) in views.iter().enumerate() {
        if i == ref_index {
            aligned.push(view.rgb.clone());
            aligned_valid.push(view.valid.clone());
            shifts.push([0.0, 0.0]);
            continue;
        }
        let overlap: Vec<bool> = (0..w * h).map(|k| ref_valid[k] && view.valid[k]).collect();
        if overlap.iter().filter(|v| **v).count() < MIN_OVERLAP_PX {
            // nothing to correlate on, so nothing is claimed either way: the view
            // stays where it is and covers what the reference cannot see
            aligned.push(view.rgb.clone());
            aligned_valid.push(view.valid.clone());
            shifts.push([0.0, 0.0]);
            continue;
        }
        let mag = gradient_magnitude(&view.rgb, Some(&view.valid));
        let a: Vec<f64> = (0..w * h)
            .map(|k| if overlap[k] { ref_mag[k] } else { 0.0 })
            .collect();
        let b: Vec<f64> = (0..w * h)
            .map(|k| if overlap[k] { mag[k] } else { 0.0 })
            .collect();
        let (dx, dy, resp) = phase_correlate(&a, &b, w, h);
        let (du, dv) = (dx / f64::from(ppb), dy / f64::from(ppb));
        let mag_m = du.hypot(dv);
        correlated += 1;
        if resp < MIN_PHASE_RESPONSE {
            usable[i] = false;
        } else {
            residuals.push(mag_m);
        }
        if usable[i] && mag_m <= max_shift_m {
            aligned.push(shift_rgb(&view.rgb, -dx, -dy));
            aligned_valid.push(shift_mask(&view.valid, w, h, -dx, -dy));
        } else {
            aligned.push(view.rgb.clone());
            aligned_valid.push(view.valid.clone());
        }
        shifts.push([du, dv]);
    }
    let agreement = if residuals.is_empty() {
        if correlated > 0 {
            max_shift_m
        } else {
            0.0
        }
    } else {
        imgops::median_in_place(&mut residuals)
    };
    (aligned, aligned_valid, shifts, agreement, usable)
}

/// The fused texture of one wall from its views.
///
/// One view is passed through; views that agree are aligned, exposure matched
/// and median combined; views that do not agree leave the best one alone and
/// tell the caller to combine the classes at block level instead.
pub fn fuse(wall_key: &str, views: &[ViewTexture], ppb: u32, _params: &Params) -> FusedTexture {
    assert!(!views.is_empty(), "fusion needs at least one view");
    let (w, h) = (
        views[0].rgb.width() as usize,
        views[0].rgb.height() as usize,
    );
    for v in views {
        assert_eq!(
            (v.rgb.width() as usize, v.rgb.height() as usize),
            (w, h),
            "every view must share the rectangle size"
        );
        assert_eq!(
            v.valid.len(),
            w * h,
            "the valid mask must match the texture"
        );
    }
    // Best is the highest score, the pano id breaking a tie so a run is
    // reproducible whatever order the views arrived in.
    let mut order: Vec<usize> = (0..views.len()).collect();
    order.sort_by(|&a, &b| {
        views[b]
            .score
            .partial_cmp(&views[a].score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| views[a].pano_id.cmp(&views[b].pano_id))
    });
    let best = order[0];
    let scores: Vec<f64> = views.iter().map(|v| v.score.max(1e-3)).collect();

    if views.len() == 1 {
        let (rgb, valid, frac, filled) =
            fill_holes(&views[0].rgb, &views[0].valid, ppb, HOLE_MAX_M);
        return FusedTexture {
            wall_key: wall_key.to_string(),
            rgb,
            valid,
            ppm: f64::from(ppb),
            shifts: vec![[0.0, 0.0]],
            agreement_m: 0.0,
            mode: FuseMode::Single,
            best_index: 0,
            hole_fraction: frac,
            filled,
            flags: if frac > 0.0 {
                vec!["HOLES_FILLED".to_string()]
            } else {
                Vec::new()
            },
        };
    }

    let (aligned, aligned_valid, shifts, agreement, usable) =
        align_views(views, ppb, MAX_SHIFT_M, best);
    // A view whose correlation peak meant nothing was never aligned, so it is not
    // median-combined either: blending an unaligned view in ghosts the facade.
    let kept: Vec<usize> = (0..views.len()).filter(|i| usable[*i]).collect();
    let mut flags: Vec<String> = Vec::new();
    if kept.len() < 2 || agreement > AGREEMENT_MAX_M {
        let (rgb, valid, frac, filled) =
            fill_holes(&views[best].rgb, &views[best].valid, ppb, HOLE_MAX_M);
        flags.push("VIEWS_DISAGREE".to_string());
        if frac > 0.0 {
            flags.push("HOLES_FILLED".to_string());
        }
        return FusedTexture {
            wall_key: wall_key.to_string(),
            rgb,
            valid,
            ppm: f64::from(ppb),
            shifts,
            agreement_m: agreement,
            mode: FuseMode::BestView,
            best_index: best,
            hole_fraction: frac,
            filled,
            flags,
        };
    }
    let mut matched = Vec::with_capacity(kept.len());
    let mut weights = Vec::with_capacity(kept.len());
    for &i in &kept {
        let rgb = if i == best {
            aligned[i].clone()
        } else {
            let overlap: Vec<bool> = (0..w * h)
                .map(|k| aligned_valid[best][k] && aligned_valid[i][k])
                .collect();
            match_exposure(&aligned[best], &aligned[i], &overlap)
        };
        matched.push(rgb);
        let bw = border_weight(&aligned_valid[i], w, h, ppb);
        weights.push(bw.iter().map(|v| v * scores[i]).collect::<Vec<f64>>());
    }
    let (rgb, valid) = weighted_median(&matched, &weights, MIN_WEIGHT_SUM);
    let (rgb, valid, frac, filled) = fill_holes(&rgb, &valid, ppb, HOLE_MAX_M);
    if frac > 0.0 {
        flags.push("HOLES_FILLED".to_string());
    }
    FusedTexture {
        wall_key: wall_key.to_string(),
        rgb,
        valid,
        ppm: f64::from(ppb),
        shifts,
        agreement_m: agreement,
        mode: FuseMode::Fused,
        best_index: best,
        hole_fraction: frac,
        filled,
        flags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, c: [u8; 3]) -> RgbImage {
        RgbImage::from_pixel(w, h, image::Rgb(c))
    }

    #[test]
    fn the_padding_size_only_has_the_factors_two_three_and_five() {
        assert_eq!(optimal_dft_size(224), 225);
        assert_eq!(optimal_dft_size(144), 144);
        assert_eq!(optimal_dft_size(104), 108);
        assert_eq!(optimal_dft_size(88), 90);
        assert_eq!(optimal_dft_size(152), 160);
        assert_eq!(optimal_dft_size(176), 180);
    }

    #[test]
    fn the_transform_inverts_itself() {
        let (w, h) = (12usize, 9usize);
        let mut data: Vec<Cx> = (0..w * h)
            .map(|i| Cx::new((i as f64 * 0.37).sin(), 0.0))
            .collect();
        let original = data.clone();
        dft_2d(&mut data, w, h, -1.0);
        dft_2d(&mut data, w, h, 1.0);
        for i in 0..w * h {
            let v = data[i].re / (w * h) as f64;
            assert!((v - original[i].re).abs() < 1e-9, "{i}: {v}");
        }
    }

    #[test]
    fn phase_correlation_finds_a_translation() {
        let (w, h) = (64usize, 48usize);
        let mut a = vec![0.0f64; w * h];
        let mut b = vec![0.0f64; w * h];
        for y in 12..30 {
            for x in 14..34 {
                a[y * w + x] = 1.0;
                // b's content sits five columns further right and three rows down.
                b[(y + 3) * w + x + 5] = 1.0;
            }
        }
        let (dx, dy, resp) = phase_correlate(&a, &b, w, h);
        assert!(resp > 0.03, "the peak is real: {resp}");
        assert!((dx - 5.0).abs() < 0.6, "dx {dx}");
        assert!((dy - 3.0).abs() < 0.6, "dy {dy}");
    }

    #[test]
    fn a_shift_moves_the_picture_the_way_the_sign_says() {
        let mut img = flat(8, 8, [0, 0, 0]);
        img.put_pixel(2, 3, image::Rgb([200, 200, 200]));
        let out = shift_rgb(&img, 2.0, 1.0);
        assert_eq!(out.get_pixel(4, 4).0, [200, 200, 200]);
        assert_eq!(out.get_pixel(2, 3).0, [0, 0, 0]);
        let m = shift_mask(&[true, false, false, false], 2, 2, 1.0, 0.0);
        assert_eq!(m, vec![false, true, false, false]);
    }

    #[test]
    fn the_border_weight_rises_from_its_floor_to_one() {
        let (w, h) = (40usize, 40usize);
        let valid = vec![true; w * h];
        let bw = border_weight(&valid, w, h, 8);
        // The corner texel is one pixel from the padded border, an eighth of the
        // metre the fall-off spans, so it is barely above the floor.
        assert!(
            bw[0] > BORDER_FLOOR && bw[0] < BORDER_FLOOR + 0.05,
            "{}",
            bw[0]
        );
        // A metre in, at 8 px per block, the weight is 1.
        assert!((bw[20 * w + 20] - 1.0).abs() < 1e-9);
        assert!(bw[8 * w + 8] > bw[2 * w + 2]);
        assert!(border_weight(&vec![false; w * h], w, h, 8)
            .iter()
            .all(|v| *v == 0.0));
    }

    #[test]
    fn the_median_takes_the_middle_view_and_the_floor_is_relative() {
        let (w, h) = (2u32, 1u32);
        let a = flat(w, h, [10, 10, 10]);
        let b = flat(w, h, [100, 100, 100]);
        let c = flat(w, h, [200, 200, 200]);
        let ones = vec![1.0f64; 2];
        let (out, valid) = weighted_median(
            &[a.clone(), b.clone(), c.clone()],
            &[ones.clone(), ones.clone(), ones.clone()],
            MIN_WEIGHT_SUM,
        );
        assert_eq!(out.get_pixel(0, 0).0, [100, 100, 100]);
        assert!(valid.iter().all(|v| *v));
        // Weight the outer views heavily and the median moves to them.
        let heavy = vec![5.0f64; 2];
        let (out2, _) = weighted_median(
            &[a.clone(), b, c],
            &[heavy, ones.clone(), ones.clone()],
            MIN_WEIGHT_SUM,
        );
        assert_eq!(out2.get_pixel(0, 0).0, [10, 10, 10]);
        // Two weak views still make a valid texel, because the floor is a share
        // of the best weight and not an absolute.
        let weak = vec![0.02f64; 2];
        let (_, valid2) = weighted_median(&[a.clone(), a], &[weak.clone(), weak], MIN_WEIGHT_SUM);
        assert!(valid2.iter().all(|v| *v), "a weak pair is not emptied");
    }

    #[test]
    fn exposure_matching_moves_the_mean_onto_the_reference() {
        let (w, h) = (16u32, 16u32);
        let reference = flat(w, h, [150, 150, 150]);
        let other = flat(w, h, [90, 90, 90]);
        let overlap = vec![true; (w * h) as usize];
        let out = match_exposure(&reference, &other, &overlap);
        let v = out.get_pixel(0, 0).0[0];
        assert!(v.abs_diff(150) <= 1, "{v}");
        // With almost no overlap the view is left alone.
        let mut sparse = vec![false; (w * h) as usize];
        sparse[0] = true;
        assert_eq!(
            match_exposure(&reference, &other, &sparse)
                .get_pixel(0, 0)
                .0,
            [90, 90, 90]
        );
    }

    #[test]
    fn a_narrow_hole_is_filled_and_a_wide_one_is_not() {
        let (w, h) = (64usize, 32usize);
        let rgb = flat(w as u32, h as u32, [120, 130, 140]);
        let mut valid = vec![true; w * h];
        // A 4 px hole, half a metre at 8 px per block, and a 24 px one, three.
        for y in 10..14 {
            for x in 6..10 {
                valid[y * w + x] = false;
            }
        }
        for y in 4..28 {
            for x in 30..54 {
                valid[y * w + x] = false;
            }
        }
        let (out, new_valid, frac, filled) = fill_holes(&rgb, &valid, 8, HOLE_MAX_M);
        assert!(new_valid[11 * w + 7], "the small hole is filled");
        assert!(!new_valid[16 * w + 40], "the large one stays no-data");
        assert!(filled[11 * w + 7] && !filled[16 * w + 40]);
        assert!((frac - 16.0 / (w * h) as f64).abs() < 1e-12);
        // A flat surround inpaints to the same flat colour.
        let p = out.get_pixel(7, 11).0;
        assert!(
            p[0].abs_diff(120) <= 2 && p[1].abs_diff(130) <= 2 && p[2].abs_diff(140) <= 2,
            "{p:?}"
        );
    }

    #[test]
    fn one_view_is_passed_through_and_two_that_agree_are_fused() {
        let (w, h) = (48u32, 32u32);
        let mut a = flat(w, h, [100, 100, 100]);
        for y in 8..24u32 {
            for x in 10..20u32 {
                a.put_pixel(x, y, image::Rgb([30, 30, 30]));
            }
        }
        let valid = vec![true; (w * h) as usize];
        let one = vec![ViewTexture {
            pano_id: "a".into(),
            rgb: a.clone(),
            valid: valid.clone(),
            score: 0.8,
        }];
        let single = fuse("w1_0", &one, 8, &Params::default());
        assert_eq!(single.mode, FuseMode::Single);
        assert_eq!(single.rgb.get_pixel(12, 12).0, [30, 30, 30]);

        // A second view of the same wall with a parked car in front of it: the
        // median should drop the car and keep the window.
        let mut b = a.clone();
        for y in 24..32u32 {
            for x in 0..48u32 {
                b.put_pixel(x, y, image::Rgb([200, 20, 20]));
            }
        }
        let mut c = a.clone();
        for y in 24..32u32 {
            for x in 0..48u32 {
                c.put_pixel(x, y, image::Rgb([20, 20, 200]));
            }
        }
        let three = vec![
            ViewTexture {
                pano_id: "a".into(),
                rgb: a,
                valid: valid.clone(),
                score: 0.9,
            },
            ViewTexture {
                pano_id: "b".into(),
                rgb: b,
                valid: valid.clone(),
                score: 0.8,
            },
            ViewTexture {
                pano_id: "c".into(),
                rgb: c,
                valid,
                score: 0.7,
            },
        ];
        let fused = fuse("w1_0", &three, 8, &Params::default());
        assert_eq!(fused.mode, FuseMode::Fused);
        assert_eq!(fused.best_index, 0);
        assert!(fused.agreement_m <= AGREEMENT_MAX_M);
        // The window survives and the two different cars cancel.
        assert!(fused.rgb.get_pixel(12, 12).0[0] < 60);
        let ground = fused.rgb.get_pixel(24, 28).0;
        assert!(ground[0] < 120 && ground[2] < 120, "{ground:?}");
    }

    /// A view with nothing in common with the reference correlates to wherever
    /// the noise was highest. That peak must not vote on the agreement, and the
    /// view must not be blended into the median.
    #[test]
    fn a_peak_that_means_nothing_neither_votes_nor_fuses() {
        let (w, h) = (64u32, 48u32);
        // a facade with structure to correlate on
        let mut a = flat(w, h, [120, 118, 115]);
        for y in (6..44u32).step_by(12) {
            for x in (5..60u32).step_by(9) {
                for dy in 0..6u32 {
                    for dx in 0..5u32 {
                        a.put_pixel(x + dx, y + dy, image::Rgb([35, 40, 55]));
                    }
                }
            }
        }
        // and a view of something else entirely: white noise
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut noise = RgbImage::new(w, h);
        for p in noise.pixels_mut() {
            for c in 0..3 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                p.0[c] = (state >> 33) as u8;
            }
        }
        let valid = vec![true; (w * h) as usize];
        let views = vec![
            ViewTexture {
                pano_id: "a".into(),
                rgb: a.clone(),
                valid: valid.clone(),
                score: 0.9,
            },
            ViewTexture {
                pano_id: "b".into(),
                rgb: noise,
                valid: valid.clone(),
                score: 0.6,
            },
        ];
        let (aligned, _av, shifts, agreement, usable) = align_views(&views, 8, MAX_SHIFT_M, 0);
        assert_eq!(usable, vec![true, false]);
        assert!(
            shifts[1][0].hypot(shifts[1][1]) > 0.5,
            "the noise peak should be far from the origin: {:?}",
            shifts[1]
        );
        // nothing was measurable, so the agreement is the largest repairable
        // residual rather than a number taken from the noise
        assert_eq!(agreement, MAX_SHIFT_M);
        assert_eq!(aligned[1].as_raw(), views[1].rgb.as_raw());
        let out = fuse("w1_0", &views, 8, &Params::default());
        assert_eq!(out.mode, FuseMode::BestView);
        assert!(out.flags.iter().any(|f| f == "VIEWS_DISAGREE"));
        assert_eq!(out.rgb.as_raw(), a.as_raw());

        // a third view that does align decides the wall on its own, and the
        // noise stays out of the median
        let mut three = views;
        three.push(ViewTexture {
            pano_id: "c".into(),
            rgb: shift_rgb(&a, 2.0, 0.0),
            valid,
            score: 0.5,
        });
        let out = fuse("w1_0", &three, 8, &Params::default());
        assert_eq!(out.mode, FuseMode::Fused);
        assert!(out.agreement_m < 0.5, "{}", out.agreement_m);
        let centre = out.rgb.get_pixel(30, 24).0;
        let want = a.get_pixel(30, 24).0;
        assert!(
            centre.iter().zip(want).all(|(p, q)| p.abs_diff(q) < 40),
            "{centre:?} against {want:?}"
        );
    }

    /// Every wall of the texture fixture through `fuse`, against the run's own
    /// fused texture. The tolerance is the one `PORT_TO_RUST.md` states for this
    /// stage: a mean absolute difference under 4 grey levels over the texels
    /// both call valid, and the same valid mask to an intersection over union
    /// of 0.98.
    #[test]
    fn the_fusion_reproduces_the_python() {
        if golden::pixels_absent() {
            return;
        }

        use crate::mapillary::golden;

        let walls = golden::texture_walls();
        let tol = golden::texture_manifest().tolerances;
        assert!(
            walls.len() >= 20,
            "the fixture must span at least 20 walls, not {}",
            walls.len()
        );
        let mut rows = Vec::new();
        let mut bad = Vec::new();
        let (mut worst_mad, mut worst_iou) = (0.0f64, 1.0f64);
        for wall in &walls {
            let views = wall.view_textures();
            let out = fuse(&wall.key, &views, wall.ppb, &Params::default());
            let (rgb, valid) = wall.fused();
            let (mad, iou) = golden::texture_agreement(&out.rgb, &out.valid, &rgb, &valid);
            rows.push(format!(
                "{:16} {:9} {} views  mad {:6.3}  iou {:6.4}  agreement {:6.3} against {:6.3}",
                wall.key,
                out.mode.as_str(),
                views.len(),
                mad,
                iou,
                out.agreement_m,
                wall.fuse.agreement_m
            ));
            if out.mode.as_str() != wall.fuse.mode {
                bad.push(format!(
                    "{}: mode {} against {}",
                    wall.key,
                    out.mode.as_str(),
                    wall.fuse.mode
                ));
            }
            if out.best_index != wall.fuse.best_index {
                bad.push(format!(
                    "{}: best view {} against {}",
                    wall.key, out.best_index, wall.fuse.best_index
                ));
            }
            if (out.agreement_m - wall.fuse.agreement_m).abs() > tol.agreement_m {
                let mine: Vec<String> = out
                    .shifts
                    .iter()
                    .map(|s| format!("({:.3},{:.3})", s[0], s[1]))
                    .collect();
                let theirs: Vec<String> = wall
                    .fuse
                    .shifts_m
                    .iter()
                    .map(|s| format!("({:.3},{:.3})", s[0], s[1]))
                    .collect();
                bad.push(format!(
                    "{}: agreement {:.4} against {:.4}; shifts {} against {}",
                    wall.key,
                    out.agreement_m,
                    wall.fuse.agreement_m,
                    mine.join(" "),
                    theirs.join(" ")
                ));
            }
            if mad > tol.texture_mean_abs_diff {
                bad.push(format!("{}: mean absolute difference {mad:.3}", wall.key));
            }
            if iou < tol.valid_iou {
                bad.push(format!("{}: valid mask iou {iou:.4}", wall.key));
            }
            worst_mad = worst_mad.max(mad);
            worst_iou = worst_iou.min(iou);
        }
        println!(
            "{}",
            rows.join(
                "
"
            )
        );
        println!("worst mean absolute difference {worst_mad:.3}, worst iou {worst_iou:.4}");
        assert!(
            bad.is_empty(),
            "{}",
            bad.join(
                "
"
            )
        );
    }

    /// One view's texture rendered by the Rust rectifier, from the fixture's own
    /// files, ready for `fuse`.
    fn rust_view_texture(
        entry: &crate::mapillary::golden::GoldenTextureWall,
        wall: &crate::mapillary::types::Wall,
        buildings: &[crate::mapillary::types::Building],
        view_doc: &crate::mapillary::golden::GoldenTextureView,
        rectify_doc: &crate::mapillary::golden::GoldenTextureRectify,
        image: &RgbImage,
        params: &Params,
    ) -> ViewTexture {
        use crate::mapillary::rectify::{loose_crop, resample_rect, View};
        let cam = view_doc.camera();
        let fit = view_doc.plane_fit(&entry.key);
        let view = View {
            cam: &cam,
            fit: &fit,
            z_base: view_doc.z_base,
            dist_m: Some(view_doc.dist_m),
            s_vis: Some(view_doc.s_vis),
        };
        let depth = entry.depth(rectify_doc);
        let crop = loose_crop(wall, &view, image, depth.as_ref(), buildings, params);
        let out = resample_rect(
            wall,
            &view,
            image,
            entry.view_rect(view_doc),
            view_doc.h_shear.as_ref(),
            entry.ppb,
            Some(&crop),
            params,
        );
        ViewTexture {
            pano_id: view_doc.pano_id.clone(),
            rgb: out.rgb,
            valid: out.valid,
            score: view_doc.score,
        }
    }

    /// The seam between `rectify` and `fuse`, as far as the fixture can carry it
    /// on its own.
    ///
    /// [`the_fusion_reproduces_the_python`] feeds this stage the run's **own**
    /// per view PNGs, so it never sees what the Rust rectifier makes of the same
    /// photograph. That matters, because a difference far below one grey level
    /// on the way in used to decide the whole texture: it moved a phase
    /// correlation peak that had no real peak to begin with, and the noise
    /// position it landed on voted on `fused` against `best_view`. Here the
    /// views are rendered by the Rust rectifier from the JPEGs the fixture
    /// carries, and the fused result is held to the Python's.
    ///
    /// Only three walls travel with their photograph, and all three have a
    /// single view, so what this covers is the single-view path through
    /// `fill_holes` on the Rust's own texture. The multi-view seam needs the
    /// Python lab's image cache and lives in
    /// [`the_fusion_seam_over_the_whole_lab_run`].
    #[test]
    fn the_fusion_seam_on_the_walls_that_carry_their_photograph() {
        if golden::pixels_absent() {
            return;
        }

        use crate::mapillary::golden;

        let walls = golden::texture_walls();
        let by_key = golden::walls_by_key();
        let buildings = golden::buildings();
        let tol = golden::texture_manifest().tolerances;
        let params = Params::default();
        let mut seen = 0usize;
        let mut bad = Vec::new();
        for entry in &walls {
            let Some(rectify) = entry.rectify.as_ref() else {
                continue;
            };
            let wall = &by_key[&entry.key];
            let mut views = Vec::new();
            for view_doc in &entry.views {
                let Some(r) = rectify.iter().find(|r| r.pano_id == view_doc.pano_id) else {
                    break;
                };
                let Some(image) = entry.image(r) else {
                    break;
                };
                views.push(rust_view_texture(
                    entry, wall, &buildings, view_doc, r, &image, &params,
                ));
            }
            if views.len() != entry.views.len() {
                continue;
            }
            let out = fuse(&entry.key, &views, entry.ppb, &params);
            let (rgb, valid) = entry.fused();
            let (mad, iou) = golden::texture_agreement(&out.rgb, &out.valid, &rgb, &valid);
            seen += 1;
            println!(
                "{:16} {:9} {} views  mad {mad:6.3}  iou {iou:.4}  holes {:.4} against {:.4}",
                entry.key,
                out.mode.as_str(),
                views.len(),
                out.hole_fraction,
                entry.fuse.hole_fraction
            );
            if out.mode.as_str() != entry.fuse.mode {
                bad.push(format!(
                    "{}: mode {} against {}",
                    entry.key,
                    out.mode.as_str(),
                    entry.fuse.mode
                ));
            }
            if (out.agreement_m - entry.fuse.agreement_m).abs() > tol.agreement_m {
                bad.push(format!(
                    "{}: agreement {:.4} against {:.4}",
                    entry.key, out.agreement_m, entry.fuse.agreement_m
                ));
            }
            if mad > tol.texture_mean_abs_diff {
                bad.push(format!(
                    "{}: the fused texture differs by {mad:.3}",
                    entry.key
                ));
            }
            if iou < tol.valid_iou {
                bad.push(format!("{}: the fused mask iou is {iou:.4}", entry.key));
            }
        }
        assert!(
            seen >= 3,
            "the fixture must carry at least three walls to render, not {seen}"
        );
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    /// The multi-view half of the same seam, over every wall of the fixture.
    ///
    /// Each view is rendered by the Rust rectifier from the reference run's own
    /// photograph and handed to the Rust `fuse`; what comes out is compared with
    /// the Python's fused texture. This is the measurement the response gate was
    /// built for. It costs nothing on the way in: the worst per view texture is
    /// 0.25 grey levels from the Python's, which is asserted per wall. What it
    /// buys is on the way out: the fuse branch moved on **three** of these 45
    /// walls before the gate and moves on **one** after it.
    ///
    /// The three numbers that are pooled rather than asserted per wall, and why:
    ///
    /// * **The branch.** `r6035286_12` is the wall still moving, and its peak is
    ///   not the weak kind the gate catches. It is a five block wide pier of the
    ///   Sendlinger Tor whose stonework repeats vertically, so its correlation
    ///   surface carries a rival: 84 per cent of the winner 1.9 m away for one
    ///   view and 78 per cent 2.4 m away for the other, at responses of 0.33 and
    ///   0.40. A quarter of a grey level picks the rival, the second view's
    ///   residual moves from 0.13 m to 1.25 m, and the median of the two crosses
    ///   0.75 m. That is a different defect from the one fixed here, it is
    ///   measured in `MEASURED.md`, and it is not fixed.
    /// * **The fused texture.** A residual that moves by half a texel is applied
    ///   as a different sub-texel shift, and a facade full of window frames then
    ///   differs by more than four grey levels although nothing was decided
    ///   differently. Four walls do that; the median wall is under a fifth of a
    ///   grey level, and that is what is asserted.
    /// * **The agreement.** Same cause. `w81190182_9` is the other shape of it:
    ///   its single peak sits at a response of 0.082 in the Python and under the
    ///   0.03 floor here, so the gate reports `MAX_SHIFT_M` instead of 0.94 m.
    ///   Both are over the threshold, the wall takes the same branch, and the
    ///   confidence sees a slightly worse number. One measurement in ninety
    ///   crosses that floor.
    ///
    /// 281 of the run's 292 views were rendered from a 2.5 MB original, which is
    /// 190 MB of photographs the fixture cannot carry, so this reads the Python
    /// lab's own cache and steps aside when that is not on the machine. It is
    /// ignored by default because it decodes a hundred originals; run it with
    ///
    /// ```text
    /// cargo test --release -- --ignored mapillary::fuse::tests::the_fusion_seam_over --nocapture
    /// ```
    #[test]
    #[ignore = "reads the Python lab's image cache, which is not part of the fixture"]
    fn the_fusion_seam_over_the_whole_lab_run() {
        if golden::pixels_absent() {
            return;
        }

        use crate::mapillary::golden;
        use rayon::prelude::*;

        if golden::lab_cache_dir().is_none() {
            println!("the facade lab cache is not on this machine, so nothing was measured");
            return;
        }
        let walls = golden::texture_walls();
        let by_key = golden::walls_by_key();
        let buildings = golden::buildings();
        let tol = golden::texture_manifest().tolerances;
        let params = Params::default();

        type Row = (String, usize, String, String, f64, f64, f64, f64, f64);
        let rows: Vec<Option<Row>> = walls
            .par_iter()
            .map(|entry| {
                let rectify = entry.rectify.as_ref()?;
                let wall = by_key.get(&entry.key)?;
                let mut views = Vec::new();
                let mut view_mad = 0.0f64;
                for view_doc in &entry.views {
                    let r = rectify.iter().find(|r| r.pano_id == view_doc.pano_id)?;
                    let image = entry.lab_image(r)?;
                    let mine =
                        rust_view_texture(entry, wall, &buildings, view_doc, r, &image, &params);
                    let want = entry.view_texture(view_doc);
                    let (mad, _) =
                        golden::texture_agreement(&mine.rgb, &mine.valid, &want.rgb, &want.valid);
                    view_mad = view_mad.max(mad);
                    views.push(mine);
                }
                let out = fuse(&entry.key, &views, entry.ppb, &params);
                let (rgb, valid) = entry.fused();
                let (mad, iou) = golden::texture_agreement(&out.rgb, &out.valid, &rgb, &valid);
                Some((
                    entry.key.clone(),
                    views.len(),
                    out.mode.as_str().to_string(),
                    entry.fuse.mode.clone(),
                    out.agreement_m,
                    entry.fuse.agreement_m,
                    view_mad,
                    mad,
                    iou,
                ))
            })
            .collect();

        let mut bad = Vec::new();
        let (mut seen, mut moved) = (0usize, 0usize);
        let (mut worst_view, mut worst_mad, mut worst_iou) = (0.0f64, 0.0f64, 1.0f64);
        let (mut mads, mut agrs) = (Vec::new(), Vec::new());
        let mut branch_moved = Vec::new();
        for row in rows.into_iter().flatten() {
            let (key, n, mode, want_mode, agr, want_agr, vmad, mad, iou) = row;
            seen += 1;
            worst_view = worst_view.max(vmad);
            worst_mad = worst_mad.max(mad);
            worst_iou = worst_iou.min(iou);
            mads.push(mad);
            agrs.push((agr - want_agr).abs());
            println!(
                "{key:16} {mode:9} {n} views  views mad {vmad:6.3}  fused mad {mad:6.3} \
                 iou {iou:.4}  agreement {agr:7.3} against {want_agr:7.3}{}",
                if mode == want_mode {
                    ""
                } else {
                    "   [branch moved]"
                }
            );
            if mode != want_mode {
                moved += 1;
                branch_moved.push(key.clone());
            }
            // The per view texture is the rectifier's own answer and is held to
            // the stated bound; so is the fused mask, which no sub-texel shift
            // moves. The fused texture and the agreement are pooled, for the
            // reasons in the doc comment.
            if vmad > tol.texture_mean_abs_diff {
                bad.push(format!(
                    "{key}: a view texture is {vmad:.4} against {}",
                    tol.texture_mean_abs_diff
                ));
            }
            if iou < tol.valid_iou {
                bad.push(format!("{key}: the fused mask iou is {iou:.4}"));
            }
        }
        assert!(seen > 0, "the lab cache is there but carried no photograph");
        let mad_mid = imgops::median_in_place(&mut mads);
        let agr_mid = imgops::median_in_place(&mut agrs);
        println!(
            "{seen} walls, worst view difference {worst_view:.3}, median fused difference \
             {mad_mid:.3} and worst {worst_mad:.3}, worst fused mask iou {worst_iou:.4}, median \
             agreement difference {agr_mid:.4} m, {moved} walls on the other fuse branch \
             {branch_moved:?}"
        );
        // Three walls moved branch before the response gate; one moves after it,
        // for a reason the doc comment names and MEASURED.md has the numbers for.
        assert!(
            moved <= 1,
            "{moved} walls take the other fuse branch: {branch_moved:?}"
        );
        assert!(
            mad_mid <= tol.texture_mean_abs_diff,
            "the median wall's fused texture differs by {mad_mid:.3}"
        );
        assert!(
            agr_mid <= tol.agreement_m,
            "the median wall's agreement differs by {agr_mid:.4} m"
        );
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    /// The number that matters end to end: the fused textures, put through the
    /// already ported opening pass, still make the block grid the run made.
    #[test]
    fn the_fused_textures_still_make_the_same_blocks() {
        if golden::pixels_absent() {
            return;
        }

        use crate::mapillary::golden;
        use crate::mapillary::openings;

        let walls = golden::texture_walls();
        let tol = golden::texture_manifest().tolerances;
        let reference: std::collections::BTreeMap<String, golden::GoldenOpeningsWall> =
            golden::openings_walls()
                .into_iter()
                .map(|w| (w.key.clone(), w))
                .collect();
        let mut rows = Vec::new();
        let (mut worst, mut worst_key) = (1.0f64, String::new());
        let (mut same_total, mut cells_total) = (0usize, 0usize);
        for wall in &walls {
            let Some(want) = reference.get(&wall.key) else {
                continue;
            };
            let out = fuse(
                &wall.key,
                &wall.view_textures(),
                wall.ppb,
                &Params::default(),
            );
            let tex = openings::WallTexture::new(
                out.rgb.width() as usize,
                out.rgb.height() as usize,
                out.rgb.pixels().map(|p| p.0).collect(),
                out.valid.clone(),
            );
            let got = openings::classify(
                &tex,
                want.rows,
                want.cols,
                (want.origin_px[0], want.origin_px[1]),
            );
            let n = want.rows * want.cols;
            let same = (0..n)
                .filter(|&i| got.cls[i] == want.openings.cls[i])
                .count();
            let agree = same as f64 / n as f64;
            same_total += same;
            cells_total += n;
            rows.push(format!(
                "{:16} {:3}x{:<3} cells {:4}  agreement {:6.4}",
                wall.key, want.cols, want.rows, n, agree
            ));
            if agree < worst {
                worst = agree;
                worst_key = wall.key.clone();
            }
        }
        println!("{}", rows.join("\n"));
        let pooled = same_total as f64 / cells_total as f64;
        println!(
            "{cells_total} cells over {} walls, pooled agreement {pooled:.4}, worst {worst:.4} on {worst_key}",
            walls.len()
        );
        assert!(
            worst >= tol.class_grid_agreement,
            "{worst_key}: only {worst} of the cells agree"
        );
    }
}
