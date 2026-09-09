//! Windows and doors found on the texture. Port of `tools/facade_lab/openings.py`.
//!
//! There is no object detection anywhere in this pipeline and there is none
//! here either. An opening is a patch of the texture that is darker than the
//! local wall, or of a different chroma without being sky, and never foliage.
//! The rules were set by a review of 40 walls by four independent reviewers, and
//! the order matters:
//!
//! 1. a local wall reference over a 3.5 m window, the upper quartile of L;
//! 2. opening pixels against that reference, then blobs;
//! 3. blobs are **split at sill rows and pier columns before any rejection**, so
//!    a pair of joined windows is cut apart rather than thrown away as too big;
//! 4. then the rejections, each with its own reason so a wall can be argued
//!    with: small, strip, eave, band, too big, faint, shadow, plinth, base
//!    strip, tall, tree, parapet, lone;
//! 5. doors and shop glass at the base: a door touches the ground and is at most
//!    2.2 m wide, a wider base opening is shop glass, and a flat dark band at the
//!    base with the wall's own chroma is a plinth, not glass at all;
//! 6. floor and bay regularisation, then the bay rhythm: the pitch comes from
//!    the clusters of gaps between neighbours on one floor, scored against
//!    chance hits, then the phase that catches the most centres. With a known
//!    pitch a window is never as wide as its bay, so a pier of at least one
//!    block always stays between neighbours.
//!
//! Template matching across a wall exists in the Python and is **off**
//! (`TEMPLATE_PASS`). It was fixed and swept: at the loosest threshold where
//! every addition is real it adds 3 windows in 1853 on 2 walls, and it does not
//! help the bright-window walls it was written for at any threshold, because
//! the correlation divides by the local spread so a flat sunlit wall scores as
//! high as a window and it is the colour guards rather than the score that turn
//! those down. On a wall whose texture is smeared the windows already found are
//! junk, so the template is junk, and every guard is relative to the template.
//! The constants below are the record of that measurement; the pass itself is
//! deliberately not ported.

#![allow(dead_code)]

use std::collections::HashMap;

use super::imgops::{self, Mask};

/// Texture pixels per metre.
pub const PPM: usize = 8;
/// Reference statistics are taken on 0.5 m sub-cells.
const SUB: usize = 4;
/// Sub-cells across the local wall reference: 3.5 m.
const REF_WINDOW: usize = 7;
/// Darker than the wall by this much, scaled by the local lightness.
const DARK_THR: f64 = 0.12;
/// OkLab chroma distance from the wall that reads as glass.
const CHROMA_THR: f64 = 0.045;
/// Attic windows are short but rarely narrow.
const MIN_SIZE_W: f64 = 0.5;
const MIN_SIZE_H: f64 = 0.4;
const MIN_FILL: f64 = 0.45;
const DOOR_MIN_W: f64 = 0.8;
const DOOR_MAX_W: f64 = 2.2;
const DOOR_MIN_H: f64 = 1.5;
const SHOP_MIN_H: f64 = 1.2;
/// A wide base blob taller than this is not a shop front.
const SHOP_MAX_H: f64 = 5.0;
/// A flat dark base band taller than this is a dark shop front.
const PLINTH_MAX_H: f64 = 2.0;
/// Openings whose bottom lies within this of the base are ground floor.
const BASE_TOL_M: f64 = 1.5;
/// Larger blobs above the base are shadows or awnings, unless they are glass.
const BIG_W: f64 = 4.5;
const BIG_H: f64 = 3.5;
/// A blob larger than one window is first tried as several joined ones.
const SPLIT_W: f64 = 2.5;
const SPLIT_H: f64 = 2.8;
/// Narrow blobs may be tall: staircase and French windows.
const NARROW_W: f64 = 1.8;
/// A large blob this much darker than the wall is glass, not a shadow.
const DEEP_GLASS: f64 = 0.32;
const FAINT_CONTRAST: f64 = 0.10;
const SHADOW_CHROMA: f64 = 0.025;
const SHADOW_CONTRAST: f64 = 0.22;
/// Lightness spread inside a blob: displays, frames, reflections.
const LIVELY_STD: f64 = 0.07;
const FLOOR_GAP_M: f64 = 1.2;
const BAY_GAP_M: f64 = 0.5;
const UNKNOWN_VALID: f64 = 0.7;
const MIN_EVIDENCE: f64 = 0.12;

/// Metres a window centre may sit off its bay line.
const RHYTHM_TOL: f64 = 0.35;
const RHYTHM_MIN_WINDOWS: usize = 4;
const RHYTHM_MIN_SUPPORT: f64 = 0.6;
/// A floor needs this share of its bays filled before the rest is added.
const EXTEND_MIN_SUPPORT: f64 = 0.5;
/// And the texture must show at least a trace of an opening there.
const EXTEND_EVIDENCE: f64 = 0.03;

/// The shape-matching pass, measured off. See the module header: at 0.70, the
/// loosest threshold where every addition is real, it adds 3 windows in 1853 on
/// 2 of 103 walls; at 0.62 it adds 4 real and 5 false. Kept as a constant so
/// the measurement is not repeated by someone who reads the Python and wonders.
pub const TEMPLATE_PASS: bool = false;
/// The correlation a peak would have had to reach.
pub const TM_SCORE: f64 = 0.70;

/// Cell classes, the values `tools/facade_lab/common.py` uses and the export
/// writes into the alpha channel.
pub const CLS_WALL: u8 = 255;
pub const CLS_WINDOW: u8 = 192;
pub const CLS_DOOR: u8 = 128;
pub const CLS_UNKNOWN: u8 = 64;
pub const CLS_NODATA: u8 = 0;

// --------------------------------------------------------------------------- types

/// What a blob turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Window,
    Door,
    Shop,
    Rejected,
}

/// One blob of opening pixels, in texture pixels, with the rule that kept or
/// dropped it. The rejected ones are kept so a wall that came out wrong can be
/// argued with instead of guessed at.
#[derive(Clone, Debug)]
pub struct Rect {
    pub kind: Kind,
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub reason: &'static str,
    pub contrast: f64,
    pub chroma: f64,
    pub std_l: f64,
    pub fill: f64,
    pub n_cols: usize,
    pub n_rows: usize,
}

impl Rect {
    fn new(kind: Kind, x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self {
            kind,
            x0,
            y0,
            x1,
            y1,
            reason: "",
            contrast: 0.0,
            chroma: 0.0,
            std_l: 0.0,
            fill: 1.0,
            n_cols: 0,
            n_rows: 0,
        }
    }

    pub fn w_m(&self) -> f64 {
        (self.x1 - self.x0) / PPM as f64
    }

    pub fn h_m(&self) -> f64 {
        (self.y1 - self.y0) / PPM as f64
    }

    pub fn cx(&self) -> f64 {
        0.5 * (self.x0 + self.x1)
    }

    pub fn cy(&self) -> f64 {
        0.5 * (self.y0 + self.y1)
    }
}

/// The bay rhythm of a facade.
#[derive(Clone, Copy, Debug, Default)]
pub struct Rhythm {
    /// Metres between bay centres.
    pub pitch: f64,
    /// Metres, the first bay centre modulo the pitch.
    pub phase: f64,
    /// Share of windows within `RHYTHM_TOL` of a bay line.
    pub support: f64,
    pub n_windows: usize,
    pub accepted: bool,
}

impl Rhythm {
    /// Bay centres across the wall, in texture pixels.
    fn lines(&self, w_px: f64) -> Vec<f64> {
        if !self.accepted {
            return Vec::new();
        }
        let mut out = Vec::new();
        let limit = w_px / PPM as f64;
        let mut x = self.phase;
        while x < limit + self.pitch {
            if (0.0..=limit).contains(&x) {
                out.push(x * PPM as f64);
            }
            x += self.pitch;
        }
        out
    }
}

/// The fused texture as this pass reads it: 8 px per metre, row major, with a
/// validity bit per texel.
#[derive(Clone, Debug)]
pub struct WallTexture {
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<[u8; 3]>,
    pub valid: Vec<bool>,
}

impl WallTexture {
    pub fn new(w: usize, h: usize, rgb: Vec<[u8; 3]>, valid: Vec<bool>) -> Self {
        assert_eq!(rgb.len(), w * h, "texture pixels do not fit {w}x{h}");
        assert_eq!(valid.len(), w * h, "validity bits do not fit {w}x{h}");
        Self { w, h, rgb, valid }
    }
}

/// Everything the opening pass found and everything it turned down.
#[derive(Clone, Debug)]
pub struct Openings {
    pub rows: usize,
    pub cols: usize,
    /// Row major, one class per 1 m cell.
    pub cls: Vec<u8>,
    /// Row major, the colour of each cell.
    pub rgb: Vec<[u8; 3]>,
    /// Per cell, the share of texels that look at least a little like an
    /// opening. `bands` needs it before it will complete a window.
    pub evidence: Vec<f64>,
    pub rects: Vec<Rect>,
    /// The opening pixel mask after morphology.
    pub opening: Mask,
    /// `(y0, y1)` of each window floor, in texture pixels.
    pub floors: Vec<(f64, f64)>,
    pub rhythm: Rhythm,
}

// --------------------------------------------------------------------------- reference

/// Median OkLab of the valid texels in each 0.5 m sub-cell, NaN where a
/// sub-cell has none.
fn sub_medians(
    lab: &[[f64; 3]],
    valid: &[bool],
    w: usize,
    h: usize,
) -> (Vec<[f64; 3]>, usize, usize) {
    let (nsr, nsc) = (h / SUB, w / SUB);
    let mut out = vec![[f64::NAN; 3]; nsr * nsc];
    let mut buf: Vec<[f64; 3]> = Vec::with_capacity(SUB * SUB);
    let mut chan: Vec<f64> = Vec::with_capacity(SUB * SUB);
    for sr in 0..nsr {
        for sc in 0..nsc {
            buf.clear();
            for y in sr * SUB..sr * SUB + SUB {
                for x in sc * SUB..sc * SUB + SUB {
                    if valid[y * w + x] {
                        buf.push(lab[y * w + x]);
                    }
                }
            }
            if buf.is_empty() {
                continue;
            }
            let mut med = [0.0f64; 3];
            for (c, m) in med.iter_mut().enumerate() {
                chan.clear();
                chan.extend(buf.iter().map(|p| p[c]));
                *m = imgops::median_in_place(&mut chan);
            }
            out[sr * nsc + sc] = med;
        }
    }
    (out, nsr, nsc)
}

/// The local wall colour around each sub-cell, at sub-cell resolution.
///
/// The lightness is the upper quartile of a 3.5 m neighbourhood rather than its
/// median, so a wall covered in windows does not pull its own reference down;
/// the chroma is then the median of only the sub-cells that are about that
/// light, which is what keeps a dark window out of the wall's hue.
fn wall_reference(
    lab: &[[f64; 3]],
    valid: &[bool],
    w: usize,
    h: usize,
) -> (Vec<f64>, Vec<f64>, Vec<f64>, usize, usize) {
    let (med, nsr, nsc) = sub_medians(lab, valid, w, h);
    let r = (REF_WINDOW / 2) as isize;

    let mut l_ref = vec![f64::NAN; nsr * nsc];
    let mut a_ref = vec![f64::NAN; nsr * nsc];
    let mut b_ref = vec![f64::NAN; nsr * nsc];
    let mut win_l: Vec<f64> = Vec::with_capacity(REF_WINDOW * REF_WINDOW);
    let mut win_a: Vec<f64> = Vec::with_capacity(REF_WINDOW * REF_WINDOW);
    let mut win_b: Vec<f64> = Vec::with_capacity(REF_WINDOW * REF_WINDOW);
    let mut scratch: Vec<f64> = Vec::with_capacity(REF_WINDOW * REF_WINDOW);
    for i in 0..nsr {
        for j in 0..nsc {
            win_l.clear();
            win_a.clear();
            win_b.clear();
            for di in -r..=r {
                for dj in -r..=r {
                    let (yi, xj) = (i as isize + di, j as isize + dj);
                    if yi < 0 || xj < 0 || yi as usize >= nsr || xj as usize >= nsc {
                        continue; // the pad is NaN, and NaN drops out of every statistic below
                    }
                    let p = med[yi as usize * nsc + xj as usize];
                    if p[0].is_nan() {
                        continue;
                    }
                    win_l.push(p[0]);
                    win_a.push(p[1]);
                    win_b.push(p[2]);
                }
            }
            if win_l.is_empty() {
                continue;
            }
            scratch.clear();
            scratch.extend_from_slice(&win_l);
            let lq = imgops::percentile_in_place(&mut scratch, 75.0);
            l_ref[i * nsc + j] = lq;
            let cut = lq - 0.08;
            let mut wa: Vec<f64> = Vec::new();
            let mut wb: Vec<f64> = Vec::new();
            for k in 0..win_l.len() {
                if win_l[k] >= cut {
                    wa.push(win_a[k]);
                    wb.push(win_b[k]);
                }
            }
            if !wa.is_empty() {
                a_ref[i * nsc + j] = imgops::median_in_place(&mut wa);
                b_ref[i * nsc + j] = imgops::median_in_place(&mut wb);
            }
        }
    }

    // A sub-cell whose whole neighbourhood is unobserved falls back to the
    // wall's own global colour rather than to nothing.
    let mut all_l: Vec<f64> = med.iter().map(|p| p[0]).filter(|v| v.is_finite()).collect();
    let mut all_a: Vec<f64> = med.iter().map(|p| p[1]).filter(|v| v.is_finite()).collect();
    let mut all_b: Vec<f64> = med.iter().map(|p| p[2]).filter(|v| v.is_finite()).collect();
    let gl = if all_l.is_empty() {
        0.6
    } else {
        imgops::percentile_in_place(&mut all_l, 75.0)
    };
    let ga = if all_a.is_empty() {
        0.0
    } else {
        imgops::median_in_place(&mut all_a)
    };
    let gb = if all_b.is_empty() {
        0.0
    } else {
        imgops::median_in_place(&mut all_b)
    };
    for v in l_ref.iter_mut() {
        if !v.is_finite() {
            *v = gl;
        }
    }
    for v in a_ref.iter_mut() {
        if !v.is_finite() {
            *v = ga;
        }
    }
    for v in b_ref.iter_mut() {
        if !v.is_finite() {
            *v = gb;
        }
    }
    (l_ref, a_ref, b_ref, nsr, nsc)
}

// --------------------------------------------------------------------------- blobs

/// Zeroes every horizontal run longer than `max_h` and then every vertical run
/// longer than `max_v`. A run that long would make its part too wide or too
/// tall to be a window anyway, so this cannot damage a valid part.
fn clear_long_runs(mask: &mut Mask, max_h: usize, max_v: usize) {
    for y in 0..mask.h {
        let mut i = 0;
        while i < mask.w {
            if mask.at(i, y) {
                let mut j = i;
                while j < mask.w && mask.at(j, y) {
                    j += 1;
                }
                if j - i > max_h {
                    for x in i..j {
                        mask.set(x, y, false);
                    }
                }
                i = j;
            } else {
                i += 1;
            }
        }
    }
    for x in 0..mask.w {
        let mut i = 0;
        while i < mask.h {
            if mask.at(x, i) {
                let mut j = i;
                while j < mask.h && mask.at(x, j) {
                    j += 1;
                }
                if j - i > max_v {
                    for y in i..j {
                        mask.set(x, y, false);
                    }
                }
                i = j;
            } else {
                i += 1;
            }
        }
    }
}

/// Cuts a blob apart where windows are joined: rows the blob fills almost
/// completely are a sill or string course, columns it barely occupies are the
/// piers, a 0.6 m opening removes thin bridges and long runs go too. Keeps the
/// parts that look like single windows and needs at least two of them; a band
/// with one dark spot must not become a window.
#[allow(clippy::too_many_arguments)]
fn split_blob(
    labels: &imgops::Labels,
    id: u32,
    x: usize,
    y: usize,
    bw: usize,
    bh: usize,
    dl: &[f64],
    dchroma: &[f64],
    w: usize,
    parent_ragged: bool,
) -> Vec<Rect> {
    let mut cut = Mask::new(bw, bh);
    for yy in 0..bh {
        for xx in 0..bw {
            cut.set(xx, yy, labels.at(x + xx, y + yy) == id);
        }
    }
    if bw as f64 > 2.0 * PPM as f64 {
        let occ: Vec<usize> = (0..bh)
            .map(|yy| (0..bw).filter(|&xx| cut.at(xx, yy)).count())
            .collect();
        let band: Vec<bool> = occ.iter().map(|&o| o as f64 >= 0.85 * bw as f64).collect();
        if band.iter().any(|b| *b) && !band.iter().all(|b| *b) {
            for (yy, &is_band) in band.iter().enumerate() {
                if is_band {
                    for xx in 0..bw {
                        cut.set(xx, yy, false);
                    }
                }
            }
        }
    }
    let occ_c: Vec<usize> = (0..bw)
        .map(|xx| (0..bh).filter(|&yy| cut.at(xx, yy)).count())
        .collect();
    let max_c = occ_c.iter().copied().max().unwrap_or(0);
    if max_c > 0 {
        let bridge: Vec<bool> = occ_c
            .iter()
            .map(|&o| (o as f64) < 0.4 * max_c as f64)
            .collect();
        if bridge.iter().any(|b| *b) && !bridge.iter().all(|b| *b) {
            for (xx, &is_bridge) in bridge.iter().enumerate() {
                if is_bridge {
                    for yy in 0..bh {
                        cut.set(xx, yy, false);
                    }
                }
            }
        }
    }
    let mut opened = imgops::open(&cut, 5, 5);
    clear_long_runs(
        &mut opened,
        (SPLIT_W * PPM as f64) as usize,
        (BIG_H * PPM as f64) as usize,
    );
    let parts_lab = imgops::connected_components(&opened);

    let mut parts: Vec<Rect> = Vec::new();
    let min_fill = if parent_ragged { 0.6 } else { MIN_FILL };
    for j in 1..parts_lab.count() {
        let c = parts_lab.components[j];
        let (w_m, h_m) = (c.w as f64 / PPM as f64, c.h as f64 / PPM as f64);
        let max_h = if w_m <= NARROW_W { BIG_H } else { SPLIT_H };
        if w_m < MIN_SIZE_W || h_m < MIN_SIZE_H || w_m > SPLIT_W || h_m > max_h {
            continue;
        }
        let fill = c.area as f64 / (c.w * c.h).max(1) as f64;
        if fill < min_fill {
            continue;
        }
        let (mut sum_dl, mut sum_ch, mut n) = (0.0f64, 0.0f64, 0usize);
        for yy in 0..bh {
            for xx in 0..bw {
                if parts_lab.at(xx, yy) == j as u32 {
                    sum_dl += dl[(y + yy) * w + x + xx];
                    sum_ch += dchroma[(y + yy) * w + x + xx];
                    n += 1;
                }
            }
        }
        let (contrast, chroma) = (sum_dl / n as f64, sum_ch / n as f64);
        if contrast < FAINT_CONTRAST && chroma < 0.04 {
            continue;
        }
        let mut r = Rect::new(
            Kind::Window,
            (x + c.x) as f64,
            (y + c.y) as f64,
            (x + c.x + c.w) as f64,
            (y + c.y + c.h) as f64,
        );
        r.reason = "split";
        r.contrast = contrast;
        r.chroma = chroma;
        r.fill = fill;
        parts.push(r);
    }
    if parts.len() >= 2 {
        parts
    } else {
        Vec::new()
    }
}

/// Per-blob statistics gathered in one pass over the label map.
struct BlobSums {
    count: f64,
    dl: f64,
    l: f64,
    l2: f64,
    chroma: f64,
    a: f64,
    b: f64,
}

#[allow(clippy::too_many_arguments)]
fn find_rects(
    opening: &Mask,
    dl: &[f64],
    l: &[f64],
    a: &[f64],
    b: &[f64],
    dchroma: &[f64],
    w: usize,
    h: usize,
) -> Vec<Rect> {
    let labels = imgops::connected_components(opening);
    let n = labels.count();
    let mut rects: Vec<Rect> = Vec::new();
    if n <= 1 {
        return rects;
    }
    let fin = |v: f64| if v.is_finite() { v } else { 0.0 };
    let mut sums: Vec<BlobSums> = (0..n)
        .map(|_| BlobSums {
            count: 0.0,
            dl: 0.0,
            l: 0.0,
            l2: 0.0,
            chroma: 0.0,
            a: 0.0,
            b: 0.0,
        })
        .collect();
    for i in 0..w * h {
        let id = labels.labels[i] as usize;
        let s = &mut sums[id];
        s.count += 1.0;
        s.dl += fin(dl[i]);
        s.l += fin(l[i]);
        s.l2 += fin(l[i] * l[i]);
        s.chroma += fin(dchroma[i]);
        s.a += fin(a[i]);
        s.b += fin(b[i]);
    }

    let wall_w_m = w as f64 / PPM as f64;
    for (i, (comp, s)) in labels
        .components
        .iter()
        .zip(sums.iter())
        .enumerate()
        .skip(1)
    {
        let (x, y, bw, bh, area) = (comp.x, comp.y, comp.w, comp.h, comp.area);
        let c = s.count.max(1.0);
        let contrast = s.dl / c;
        let chroma = s.chroma / c;
        let mean_l = s.l / c;
        let std_l = (s.l2 / c - mean_l * mean_l).max(0.0).sqrt();
        let mean_a = s.a / c;
        let mean_b = s.b / c;
        let (w_m, h_m) = (bw as f64 / PPM as f64, bh as f64 / PPM as f64);
        let fill = area as f64 / (bw * bh).max(1) as f64;
        let base = (y + bh) as f64 >= h as f64 - BASE_TOL_M * PPM as f64;
        let touches_bottom = (y + bh) as f64 >= h as f64 - 0.6 * PPM as f64;
        let top = y as f64 <= 0.3 * PPM as f64;

        let mut r = Rect::new(
            Kind::Rejected,
            x as f64,
            y as f64,
            (x + bw) as f64,
            (y + bh) as f64,
        );
        r.contrast = contrast;
        r.chroma = chroma;
        r.std_l = std_l;
        r.fill = fill;

        if top && mean_l > 0.7 && mean_b < -0.04 && !base {
            // a bright blue strip on the top edge is sky over a roof step, not glass
            r.reason = "sky";
            rects.push(r);
            continue;
        }
        if mean_a < -0.02 && mean_b > 0.02 && (w_m > 2.0 || h_m > 2.0) {
            // a large greenish blob is canopy the masks let through, never a shop front
            r.reason = "tree";
            rects.push(r);
            continue;
        }
        let small = w_m < MIN_SIZE_W || h_m < MIN_SIZE_H;
        if base {
            let lively = std_l >= LIVELY_STD || chroma >= 0.04;
            if w_m >= 3.0 && h_m > SHOP_MAX_H {
                // a dark region wider than a shop and taller than a shop front is a
                // tree, an arcade in deep shadow or a curtain wall; glass that tall at
                // street level costs more in the game than a missed front
                r.reason = "tall";
            } else if w_m >= 3.0 && h_m >= SHOP_MIN_H {
                // wide ground-floor blobs are judged before the fill test: a glass
                // front behind cars and a canopy is ragged by nature. Liveliness is
                // read above the bottom metre, where the cars and the pavement are.
                let rows_above = (bh.saturating_sub(PPM)).max(1);
                let (mut sum_l, mut sum_l2, mut sum_ch, mut n_up) =
                    (0.0f64, 0.0f64, 0.0f64, 0usize);
                for yy in 0..rows_above.min(bh) {
                    for xx in 0..bw {
                        if labels.at(x + xx, y + yy) == i as u32 {
                            let p = (y + yy) * w + x + xx;
                            sum_l += l[p];
                            sum_l2 += l[p] * l[p];
                            sum_ch += dchroma[p];
                            n_up += 1;
                        }
                    }
                }
                let (std_up, ch_up) = if n_up > 0 {
                    let m = sum_l / n_up as f64;
                    (
                        (sum_l2 / n_up as f64 - m * m).max(0.0).sqrt(),
                        sum_ch / n_up as f64,
                    )
                } else {
                    (std_l, chroma)
                };
                let lively_up = std_up >= LIVELY_STD || ch_up >= 0.04;
                if fill >= 0.3 && lively_up && contrast >= FAINT_CONTRAST {
                    r.kind = Kind::Shop;
                } else if h_m <= PLINTH_MAX_H && !lively_up {
                    r.reason = "plinth";
                } else if fill < MIN_FILL {
                    r.reason = "ragged";
                } else if h_m > PLINTH_MAX_H {
                    r.kind = Kind::Shop;
                } else {
                    r.reason = "plinth";
                }
            } else if small {
                r.reason = "small";
            } else if fill < MIN_FILL {
                r.reason = "ragged";
            } else if (DOOR_MIN_W..=DOOR_MAX_W).contains(&w_m)
                && h_m >= DOOR_MIN_H
                && x as f64 >= 0.3 * PPM as f64
                && (x + bw) as f64 <= w as f64 - 0.3 * PPM as f64
                && (touches_bottom || h_m >= 2.0)
            {
                r.kind = Kind::Door;
            } else if w_m > DOOR_MAX_W && h_m >= SHOP_MIN_H && lively && contrast >= FAINT_CONTRAST
            {
                r.kind = Kind::Shop;
            } else if w_m > DOOR_MAX_W && h_m >= SHOP_MIN_H {
                if h_m > PLINTH_MAX_H {
                    r.kind = Kind::Shop;
                } else {
                    r.reason = "plinth";
                }
            } else if !touches_bottom
                && h_m >= 0.8
                && w_m <= 2.0
                && fill >= 0.6
                && contrast >= FAINT_CONTRAST
            {
                // a window row just above the pavement band
                r.kind = Kind::Window;
            } else {
                r.reason = "base strip";
            }
            rects.push(r);
            continue;
        }
        if small {
            r.reason = "small";
        } else if h_m < 0.8 && w_m > 2.0 {
            r.reason = "strip";
        } else if (y as f64) < 0.5 * PPM as f64 && w_m > 2.5 && h_m < 1.3 {
            r.reason = "eave";
        } else if w_m <= NARROW_W && h_m <= 3.4 && fill >= 0.6 && (std_l >= 0.04 || chroma >= 0.03)
        {
            r.kind = Kind::Window;
        } else {
            if w_m > SPLIT_W || h_m > SPLIT_H || fill < MIN_FILL {
                let parts = split_blob(
                    &labels,
                    i as u32,
                    x,
                    y,
                    bw,
                    bh,
                    dl,
                    dchroma,
                    w,
                    fill < MIN_FILL,
                );
                if !parts.is_empty() {
                    rects.extend(parts);
                    continue;
                }
            }
            let glassy = fill >= 0.6
                && h_m <= BIG_H
                && (std_l >= LIVELY_STD + 0.01 || contrast >= DEEP_GLASS);
            if fill < MIN_FILL {
                r.reason = "ragged";
            } else if (w_m > SPLIT_W || h_m > SPLIT_H) && !glassy {
                // wider than a window, did not come apart, and neither lively nor deep:
                // a balcony band, a string course, a shadow. A shadow takes a fifth of
                // the lightness off a wall, a curtain wall a third or more.
                r.reason = if w_m <= BIG_W && h_m <= BIG_H {
                    "band"
                } else {
                    "too big"
                };
            } else if contrast < FAINT_CONTRAST && chroma < 0.04 {
                r.reason = "faint";
            } else if chroma < SHADOW_CHROMA
                && contrast < SHADOW_CONTRAST
                && std_l < LIVELY_STD
                && (w_m > 2.5 || h_m > 2.5)
            {
                r.reason = "shadow";
            } else {
                r.kind = Kind::Window;
            }
        }
        rects.push(r);
    }
    // a single full-width glassy band is a parapet or a balcony; a ribbon facade has several
    let wide: Vec<usize> = rects
        .iter()
        .enumerate()
        .filter(|(_, r)| r.kind == Kind::Window && r.w_m() > BIG_W.max(0.7 * wall_w_m))
        .map(|(i, _)| i)
        .collect();
    if wide.len() == 1 {
        rects[wide[0]].kind = Kind::Rejected;
        rects[wide[0]].reason = "parapet";
    }
    rects
}

// --------------------------------------------------------------------------- floors and bays

/// Windows grouped by vertical centre; a new floor starts when the next
/// window's centre is more than `FLOOR_GAP_M` below the group's mean.
fn floors_of(rects: &[Rect]) -> Vec<Vec<usize>> {
    let mut wins: Vec<usize> = (0..rects.len())
        .filter(|&i| rects[i].kind == Kind::Window)
        .collect();
    wins.sort_by(|&a, &b| rects[a].cy().partial_cmp(&rects[b].cy()).unwrap());
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in wins {
        let joins = match groups.last() {
            Some(g) => {
                let mean: f64 = g.iter().map(|&k| rects[k].cy()).sum::<f64>() / g.len() as f64;
                (rects[i].cy() - mean).abs() <= FLOOR_GAP_M * PPM as f64
            }
            None => false,
        };
        if joins {
            groups.last_mut().unwrap().push(i);
        } else {
            groups.push(vec![i]);
        }
    }
    groups
}

/// Mean of a mask over a rectangle of texture pixels, the `soft[ys:ye, xs:xe].mean()`
/// of the Python. Returns `None` when the rectangle is empty.
fn mask_mean(mask: &Mask, x0: f64, y0: f64, x1: f64, y1: f64) -> Option<f64> {
    let ys = y0.max(0.0).trunc() as usize;
    let ye = (y1.min(mask.h as f64)).max(0.0).trunc() as usize;
    let xs = x0.max(0.0).trunc() as usize;
    let xe = (x1.min(mask.w as f64)).max(0.0).trunc() as usize;
    if ye <= ys || xe <= xs {
        return None;
    }
    let mut hit = 0usize;
    for y in ys..ye {
        for x in xs..xe {
            if mask.at(x, y) {
                hit += 1;
            }
        }
    }
    Some(hit as f64 / ((ye - ys) * (xe - xs)) as f64)
}

/// Every window on a floor gets the floor's median top, height and width when
/// it is close to them; a short strip on a floor of tall windows (the lintel
/// shadow of a bright window) is stretched to the floor size when the texture
/// shows at least a trace of an opening there. A window alone on its floor that
/// is tiny and faint is dropped. Returns the surviving floor groups.
fn regularise_floors(rects: &mut [Rect], soft: &Mask) -> Vec<Vec<usize>> {
    let groups = floors_of(rects);
    for group in &groups {
        if group.len() < 2 {
            let i = group[0];
            if rects[i].w_m() * rects[i].h_m() < 1.0 && rects[i].contrast < DEEP_GLASS {
                rects[i].kind = Kind::Rejected;
                rects[i].reason = "lone";
            }
            continue;
        }
        let top = imgops::median(&group.iter().map(|&i| rects[i].y0).collect::<Vec<_>>());
        let hh = imgops::median(
            &group
                .iter()
                .map(|&i| rects[i].y1 - rects[i].y0)
                .collect::<Vec<_>>(),
        );
        let ww = imgops::median(
            &group
                .iter()
                .map(|&i| rects[i].x1 - rects[i].x0)
                .collect::<Vec<_>>(),
        );
        for &i in group {
            let (y0, y1, x0, x1) = (rects[i].y0, rects[i].y1, rects[i].x0, rects[i].x1);
            if (y0 - top).abs() <= 0.5 * PPM as f64 && ((y1 - y0) - hh).abs() <= 0.6 * PPM as f64 {
                rects[i].y0 = top;
                rects[i].y1 = top + hh;
            } else if hh >= 1.2 * PPM as f64
                && (y0 - top).abs() <= 0.5 * PPM as f64
                && (y1 - y0) < 0.6 * hh
            {
                let new_y1 = top + hh;
                if mask_mean(soft, x0, top, x1, new_y1).is_some_and(|m| m >= MIN_EVIDENCE) {
                    rects[i].y0 = top;
                    rects[i].y1 = new_y1;
                }
            }
            if ((x1 - x0) - ww).abs() <= 0.5 * PPM as f64 {
                let cx = rects[i].cx();
                rects[i].x0 = cx - 0.5 * ww;
                rects[i].x1 = cx + 0.5 * ww;
            }
        }
    }
    groups
        .into_iter()
        .filter(|g| g.iter().any(|&i| rects[i].kind == Kind::Window))
        .collect()
}

/// Windows stacked above each other share a bay: their centres are pulled to
/// the bay's median so a column does not jump a block sideways between floors.
fn regularise_columns(rects: &mut [Rect]) {
    let mut wins: Vec<usize> = (0..rects.len())
        .filter(|&i| rects[i].kind == Kind::Window)
        .collect();
    wins.sort_by(|&a, &b| rects[a].cx().partial_cmp(&rects[b].cx()).unwrap());
    let mut bays: Vec<Vec<usize>> = Vec::new();
    for i in wins {
        let joins = match bays.last() {
            Some(bay) => {
                let med = imgops::median(&bay.iter().map(|&k| rects[k].cx()).collect::<Vec<_>>());
                (rects[i].cx() - med).abs() <= BAY_GAP_M * PPM as f64
            }
            None => false,
        };
        if joins {
            bays.last_mut().unwrap().push(i);
        } else {
            bays.push(vec![i]);
        }
    }
    for bay in bays {
        if bay.len() < 2 {
            continue;
        }
        let cx = imgops::median(&bay.iter().map(|&i| rects[i].cx()).collect::<Vec<_>>());
        for i in bay {
            let half = 0.5 * (rects[i].x1 - rects[i].x0);
            rects[i].x0 = cx - half;
            rects[i].x1 = cx + half;
        }
    }
}

/// Floor groups above the ground zone; shop fronts and doors set no rhythm.
fn upper_floors(groups: &[Vec<usize>], rects: &[Rect], h: usize) -> Vec<usize> {
    groups
        .iter()
        .enumerate()
        .filter(|(_, g)| {
            let y1 = imgops::median(&g.iter().map(|&i| rects[i].y1).collect::<Vec<_>>());
            y1 < h as f64 - 2.0 * PPM as f64
        })
        .map(|(k, _)| k)
        .collect()
}

/// `numpy.arange(0.0, stop, step)`: the count is `ceil(stop / step)` and value
/// `i` is `i * step`, not an accumulated sum.
fn arange(stop: f64, step: f64) -> Vec<f64> {
    let n = (stop / step).ceil().max(0.0) as usize;
    (0..n).map(|i| i as f64 * step).collect()
}

/// The bay pitch and phase of a facade from its detected windows: the pitch is
/// the smallest cluster of horizontal gaps between neighbours on one floor, the
/// phase the offset that puts most window centres on a bay line.
fn fit_rhythm(groups: &[Vec<usize>], rects: &[Rect], h: usize) -> Rhythm {
    let mut rh = Rhythm::default();
    let floors = upper_floors(groups, rects, h);
    let wins: Vec<usize> = floors
        .iter()
        .flat_map(|&gi| groups[gi].iter().copied())
        .filter(|&i| rects[i].kind == Kind::Window)
        .collect();
    rh.n_windows = wins.len();
    if wins.len() < RHYTHM_MIN_WINDOWS {
        return rh;
    }
    let mut gaps: Vec<f64> = Vec::new();
    for &gi in &floors {
        let mut xs: Vec<f64> = groups[gi]
            .iter()
            .filter(|&&i| rects[i].kind == Kind::Window)
            .map(|&i| rects[i].cx() / PPM as f64)
            .collect();
        xs.sort_by(f64::total_cmp);
        for pair in xs.windows(2) {
            let d = pair[1] - pair[0];
            if (1.2..=8.0).contains(&d) {
                gaps.push(d);
            }
        }
    }
    if gaps.len() < 2 {
        return rh;
    }
    gaps.sort_by(f64::total_cmp);
    // clusters of gaps (0.5 m wide); the pitch is the candidate whose bay lines
    // catch the most window centres, which is usually the most populated cluster.
    // Fragments of one window give small gaps, missed windows give doubles, so
    // neither the smallest nor the largest cluster can be trusted on its own.
    let mut clusters: Vec<(usize, f64)> = Vec::new();
    let mut i = 0;
    while i < gaps.len() {
        let mut j = i;
        while j + 1 < gaps.len() && gaps[j + 1] - gaps[i] <= 0.5 {
            j += 1;
        }
        clusters.push((j - i + 1, imgops::median(&gaps[i..=j])));
        i = j + 1;
    }
    // Python sorts the (count, median) tuples in reverse, so the most populated
    // cluster wins and the wider median breaks a tie.
    clusters.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.total_cmp(&a.1)));
    let mut cands: Vec<f64> = clusters
        .iter()
        .take(3)
        .filter(|(n, _)| *n >= 2)
        .map(|(_, p)| *p)
        .collect();
    if cands.is_empty() {
        cands.push(clusters[0].1);
    }
    let cx: Vec<f64> = wins.iter().map(|&i| rects[i].cx() / PPM as f64).collect();
    let mut best: Option<(f64, usize, f64, f64)> = None;
    for &pitch in &cands {
        // a short pitch catches centres by chance (the tolerance covers 2 tol / pitch
        // of every bay), so the hits are scored against that expectation
        let chance = wins.len() as f64 * (2.0 * RHYTHM_TOL / pitch).min(1.0);
        for ph in arange(pitch, 0.05) {
            let n_hit = cx
                .iter()
                .filter(|&&x| {
                    let m = (x - ph + 0.5 * pitch).rem_euclid(pitch) - 0.5 * pitch;
                    m.abs() <= RHYTHM_TOL
                })
                .count();
            let score = n_hit as f64 - chance;
            if best.is_none() || score > best.unwrap().0 {
                best = Some((score, n_hit, pitch, ph));
            }
        }
    }
    let (_, n_hit, pitch, phase) = best.unwrap();
    rh.pitch = pitch;
    rh.phase = phase;
    rh.support = n_hit as f64 / wins.len() as f64;
    rh.accepted = rh.support >= RHYTHM_MIN_SUPPORT;
    rh
}

/// Snaps window centres to their bay line and adds the windows a floor is
/// missing: on a floor with at least three windows and half of its bays filled,
/// every empty bay line inside the wall gets a window of the floor's size when
/// the texture shows a trace of an opening there. Returns the number added.
fn apply_rhythm(
    rects: &mut Vec<Rect>,
    groups: &mut [Vec<usize>],
    rh: &Rhythm,
    soft: &Mask,
    h: usize,
    w: usize,
) -> usize {
    if !rh.accepted {
        return 0;
    }
    let lines = rh.lines(w as f64);
    if lines.len() < 2 {
        return 0;
    }
    let upper = upper_floors(groups, rects, h);
    let all_cx: Vec<f64> = upper
        .iter()
        .flat_map(|&gi| groups[gi].iter().copied())
        .filter(|&i| rects[i].kind == Kind::Window)
        .map(|i| rects[i].cx())
        .collect();
    if all_cx.is_empty() {
        return 0;
    }
    // the span the detections cover; bays inside it are judged against the floor's
    // support there, bays outside it (a sunlit or blurred part of the wall) need
    // both a strong floor and clearer evidence
    let lo_span =
        all_cx.iter().copied().fold(f64::INFINITY, f64::min) - 0.5 * rh.pitch * PPM as f64;
    let hi_span =
        all_cx.iter().copied().fold(f64::NEG_INFINITY, f64::max) + 0.5 * rh.pitch * PPM as f64;
    let in_span: Vec<bool> = lines
        .iter()
        .map(|&x| lo_span <= x && x <= hi_span)
        .collect();

    // first pass: snap every floor's windows to their bay and note which bays each
    // floor holds; a bay held on another floor is a column of windows, which is
    // the cross-floor evidence for filling it where this floor missed it
    let mut floors: Vec<(usize, Vec<usize>, Vec<bool>)> = Vec::new();
    let mut bays_any: HashMap<usize, usize> = HashMap::new();
    for &gi in &upper {
        let wins: Vec<usize> = groups[gi]
            .iter()
            .copied()
            .filter(|&i| rects[i].kind == Kind::Window)
            .collect();
        let mut taken = vec![false; lines.len()];
        for &i in &wins {
            let cx = rects[i].cx();
            let mut k = 0;
            let mut best = (cx - lines[0]).abs();
            for (j, &x) in lines.iter().enumerate().skip(1) {
                let d = (cx - x).abs();
                if d < best {
                    best = d;
                    k = j;
                }
            }
            if best <= RHYTHM_TOL * PPM as f64 {
                let half = 0.5 * (rects[i].x1 - rects[i].x0);
                rects[i].x0 = lines[k] - half;
                rects[i].x1 = lines[k] + half;
                taken[k] = true;
            }
        }
        for (k, &t) in taken.iter().enumerate() {
            if t {
                *bays_any.entry(k).or_insert(0) += 1;
            }
        }
        floors.push((gi, wins, taken));
    }

    let mut added = 0usize;
    for (gi, wins, taken) in floors {
        let n_taken = taken.iter().filter(|t| **t).count();
        if wins.len() < 3 || n_taken < 2 {
            continue;
        }
        let ww = imgops::median(
            &wins
                .iter()
                .map(|&i| rects[i].x1 - rects[i].x0)
                .collect::<Vec<_>>(),
        );
        let hh = imgops::median(
            &wins
                .iter()
                .map(|&i| rects[i].y1 - rects[i].y0)
                .collect::<Vec<_>>(),
        );
        let top = imgops::median(&wins.iter().map(|&i| rects[i].y0).collect::<Vec<_>>());
        let lo = taken.iter().position(|t| *t).unwrap();
        let hi = taken.iter().rposition(|t| *t).unwrap();
        let support = n_taken as f64 / (hi - lo + 1) as f64;
        if support < EXTEND_MIN_SUPPORT {
            continue;
        }
        for (k, &x) in lines.iter().enumerate() {
            if taken[k] || !in_span[k] {
                continue;
            }
            let (x0, x1) = (x - 0.5 * ww, x + 0.5 * ww);
            if x0 < 0.3 * PPM as f64 || x1 > w as f64 - 0.3 * PPM as f64 {
                continue;
            }
            if rects.iter().any(|r| {
                r.kind != Kind::Rejected && r.x0 < x1 && r.x1 > x0 && r.y0 < top + hh && r.y1 > top
            }) {
                continue;
            }
            let need = if lo <= k && k <= hi {
                EXTEND_EVIDENCE
            } else if bays_any.get(&k).copied().unwrap_or(0) >= 1 {
                // beyond this floor's own windows: only where another floor has a
                // window in the same bay, and with clearer evidence
                0.06
            } else {
                continue;
            };
            match mask_mean(soft, x0, top, x1, top + hh) {
                Some(m) if m >= need => {}
                _ => continue,
            }
            let mut new = Rect::new(Kind::Window, x0, top, x1, top + hh);
            new.reason = "rhythm";
            rects.push(new);
            groups[gi].push(rects.len() - 1);
            added += 1;
        }
    }
    added
}

// --------------------------------------------------------------------------- cells

/// A 1.4 m window is one block, a 1.8 m one two: rounding up only past 1.7 m
/// keeps the pier between neighbouring windows where the pitch allows it.
fn cells(size_m: f64) -> usize {
    ((size_m + 0.3).floor() as i64).max(1) as usize
}

/// `n` cells centred on the rectangle (plain rounding, not banker's).
fn span(lo: f64, hi: f64, origin: i32, n_cells: usize, n: usize) -> (usize, usize) {
    let n = n.clamp(1, n_cells);
    let c = (((lo + hi) * 0.5 - origin as f64) / PPM as f64 - n as f64 * 0.5 + 0.5).floor() as i64;
    let c = c.clamp(0, (n_cells - n) as i64) as usize;
    (c, c + n)
}

/// Cell counts per rectangle: on a floor, members within a metre of the median
/// size take the median's cell count, so one floor has one window size; a
/// genuinely taller opening keeps its own. With a known bay pitch a window is
/// never as wide as the pitch, so a pier of at least one block stays between
/// neighbours.
fn assign_cells(rects: &mut [Rect], groups: &[Vec<usize>], pitch_m: f64) {
    let max_cols = if pitch_m >= 2.0 {
        cells(pitch_m).saturating_sub(1).max(1)
    } else {
        0
    };
    for r in rects.iter_mut() {
        r.n_cols = cells(r.w_m());
        r.n_rows = cells(r.h_m());
    }
    for group in groups {
        let wins: Vec<usize> = group
            .iter()
            .copied()
            .filter(|&i| rects[i].kind == Kind::Window)
            .collect();
        if wins.len() < 2 {
            continue;
        }
        let w_med = imgops::median(&wins.iter().map(|&i| rects[i].w_m()).collect::<Vec<_>>());
        let h_med = imgops::median(&wins.iter().map(|&i| rects[i].h_m()).collect::<Vec<_>>());
        for &i in &wins {
            if (rects[i].w_m() - w_med).abs() <= 1.0 {
                rects[i].n_cols = cells(w_med);
            }
            if (rects[i].h_m() - h_med).abs() <= 1.0 {
                rects[i].n_rows = cells(h_med);
            }
        }
    }
    if max_cols > 0 {
        for r in rects.iter_mut() {
            if r.kind == Kind::Window {
                r.n_cols = r.n_cols.min(max_cols);
            }
        }
    }
}

// --------------------------------------------------------------------------- the pass

/// Texture (`rows * 8` by `cols * 8`) to classes, colours and evidence per cell.
///
/// `origin_px` is the sub-block grid phase the wall was cut with, in texture
/// pixels, so cell `(0, 0)` starts at `(origin.0, origin.1)` and not at the
/// corner of the image.
pub fn classify(tex: &WallTexture, rows: usize, cols: usize, origin_px: (i32, i32)) -> Openings {
    let (w, h) = (tex.w, tex.h);
    assert!(
        w >= SUB && h >= SUB,
        "a {w}x{h} texture is smaller than one reference sub-cell"
    );
    let n = w * h;
    let mut l = vec![0.0f64; n];
    let mut a = vec![0.0f64; n];
    let mut b = vec![0.0f64; n];
    let mut lab = vec![[0.0f64; 3]; n];
    for i in 0..n {
        let p = imgops::srgb_to_oklab(tex.rgb[i]);
        lab[i] = p;
        l[i] = p[0];
        a[i] = p[1];
        b[i] = p[2];
    }

    // foliage that leaked through the masks: never wall reference, never an opening
    let veg: Vec<bool> = (0..n)
        .map(|i| tex.valid[i] && a[i] < -0.03 && b[i] > 0.03 && l[i] < 0.6)
        .collect();
    let ref_valid: Vec<bool> = (0..n).map(|i| tex.valid[i] && !veg[i]).collect();
    let (l_ref, a_ref, b_ref, nsr, nsc) = wall_reference(&lab, &ref_valid, w, h);
    let sub_at = |x: usize, y: usize| (y / SUB).min(nsr - 1) * nsc + (x / SUB).min(nsc - 1);

    let mut dl = vec![0.0f64; n];
    let mut dchroma = vec![0.0f64; n];
    let mut opening = Mask::new(w, h);
    let mut soft = Mask::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let s = sub_at(x, y);
            // a dark wall cannot put as much lightness between itself and its
            // windows as a white one, so the threshold follows the local wall
            let thr = DARK_THR * (l_ref[s] / 0.55).clamp(0.5, 1.0);
            dl[i] = l_ref[s] - l[i];
            dchroma[i] = ((a[i] - a_ref[s]).powi(2) + (b[i] - b_ref[s]).powi(2)).sqrt();
            let dark = dl[i] > thr;
            // sky in the picture is bright and blue; sky reflected in glass on a
            // beige wall is the same blue but not bright, and that is a window
            let sky = l[i] > 0.75 && b[i] < -0.06;
            let glass = dchroma[i] > CHROMA_THR && dl[i] > -0.08 && !sky;
            let live = tex.valid[i] && !veg[i];
            opening.set(x, y, live && (dark || glass));
            // half the threshold: not enough to call an opening, enough to say
            // there is something there, which is all the rhythm and the window
            // completion ask before they fill a bay in
            soft.set(x, y, live && (dl[i] > 0.5 * thr || dchroma[i] > 0.03));
        }
    }
    // open then close, both 3x3: the opening drops the single texel speckle a
    // fused texture always carries, the closing puts a window back together
    // across its own glazing bar
    let m = imgops::close(&imgops::open(&opening, 3, 3), 3, 3);

    let mut rects = find_rects(&m, &dl, &l, &a, &b, &dchroma, w, h);
    let mut groups = regularise_floors(&mut rects, &soft);
    let rhythm = fit_rhythm(&groups, &rects, h);
    if rhythm.accepted {
        apply_rhythm(&mut rects, &mut groups, &rhythm, &soft, h, w);
    } else {
        regularise_columns(&mut rects);
    }
    assign_cells(
        &mut rects,
        &groups,
        if rhythm.accepted { rhythm.pitch } else { 0.0 },
    );

    let (ox, oy) = origin_px;
    let mut cls = vec![CLS_WALL; rows * cols];
    // windows floor by floor, so touching spans with real wall between them can be pulled apart
    for group in &groups {
        let mut wins: Vec<usize> = group
            .iter()
            .copied()
            .filter(|&i| rects[i].kind == Kind::Window)
            .collect();
        wins.sort_by(|&p, &q| rects[p].x0.partial_cmp(&rects[q].x0).unwrap());
        let mut spans: Vec<(usize, usize)> = wins
            .iter()
            .map(|&i| span(rects[i].x0, rects[i].x1, ox, cols, rects[i].n_cols))
            .collect();
        for i in 1..wins.len() {
            let overlap = spans[i].0 < spans[i - 1].1;
            let touch = spans[i].0 == spans[i - 1].1
                && rects[wins[i]].x0 - rects[wins[i - 1]].x1 >= 0.3 * PPM as f64;
            if overlap || touch {
                let wider = if rects[wins[i]].n_cols >= rects[wins[i - 1]].n_cols {
                    i
                } else {
                    i - 1
                };
                if rects[wins[wider]].n_cols > 1 {
                    rects[wins[wider]].n_cols -= 1;
                    let r = &rects[wins[wider]];
                    spans[wider] = span(r.x0, r.x1, ox, cols, r.n_cols);
                }
            }
        }
        for (&i, &(c0, c1)) in wins.iter().zip(spans.iter()) {
            let (r0, r1) = span(rects[i].y0, rects[i].y1, oy, rows, rects[i].n_rows);
            for rr in r0..r1 {
                for cc in c0..c1 {
                    cls[rr * cols + cc] = CLS_WINDOW;
                }
            }
        }
    }
    for r in &rects {
        match r.kind {
            Kind::Door => {
                let (c0, c1) = span(r.x0, r.x1, ox, cols, r.n_cols);
                // a door is drawn from the ground up, whatever the detector's
                // own top row was: the pavement is where a door starts
                let nr = (imgops::round_half_even(r.h_m()) as i64).clamp(1, rows as i64) as usize;
                for rr in rows - nr..rows {
                    for cc in c0..c1 {
                        cls[rr * cols + cc] = CLS_DOOR;
                    }
                }
            }
            Kind::Shop => {
                let (c0, c1) = span(r.x0, r.x1, ox, cols, r.n_cols);
                let (r0, _) = span(r.y0, r.y1, oy, rows, r.n_rows);
                for rr in r0..rows {
                    for cc in c0..c1 {
                        if cls[rr * cols + cc] != CLS_DOOR {
                            cls[rr * cols + cc] = CLS_WINDOW;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // colours and no-data per cell on the shifted grid
    let mut rgb = vec![[0u8; 3]; rows * cols];
    let mut evidence = vec![0.0f64; rows * cols];
    let mut sel_lab: Vec<[f64; 3]> = Vec::with_capacity(PPM * PPM);
    let mut chan: Vec<f64> = Vec::with_capacity(PPM * PPM);
    for rr in 0..rows {
        for cc in 0..cols {
            let y0 = oy as i64 + (rr * PPM) as i64;
            let x0 = ox as i64 + (cc * PPM) as i64;
            // (texel index, whether the opening mask claims it), observed texels only
            let mut px: Vec<(usize, bool)> = Vec::with_capacity(PPM * PPM);
            let mut n_soft = 0usize;
            let mut n_veg = 0usize;
            for dy in 0..PPM as i64 {
                for dx in 0..PPM as i64 {
                    let (x, y) = (x0 + dx, y0 + dy);
                    if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                        continue;
                    }
                    let i = y as usize * w + x as usize;
                    if !tex.valid[i] {
                        continue;
                    }
                    if soft.at(x as usize, y as usize) {
                        n_soft += 1;
                    }
                    if veg[i] {
                        n_veg += 1;
                    }
                    px.push((i, m.at(x as usize, y as usize)));
                }
            }
            let cell = rr * cols + cc;
            let nv = px.len();
            if nv == 0 {
                cls[cell] = CLS_NODATA;
                continue;
            }
            evidence[cell] = n_soft as f64 / nv as f64;
            // foliage over half the cell, or a cell whose wall was barely seen:
            // either way the block is unknown rather than guessed at
            let mostly_veg = n_veg as f64 > 0.5 * nv as f64;
            let barely_seen =
                (nv as f64) < UNKNOWN_VALID * (PPM * PPM) as f64 && cls[cell] == CLS_WALL;
            if mostly_veg || barely_seen {
                cls[cell] = CLS_UNKNOWN;
            }
            // A window cell takes the colour of its opening texels and a wall cell
            // the colour of everything but them, so a window frame does not tint
            // the wall and a bright sill does not tint the glass.
            let want_open = cls[cell] == CLS_WINDOW || cls[cell] == CLS_DOOR;
            let n_side = px.iter().filter(|p| p.1 == want_open).count();
            sel_lab.clear();
            if n_side > 0 {
                sel_lab.extend(px.iter().filter(|p| p.1 == want_open).map(|p| lab[p.0]));
            } else {
                sel_lab.extend(px.iter().map(|p| lab[p.0]));
            }
            let mut med = [0.0f64; 3];
            for (c, out) in med.iter_mut().enumerate() {
                chan.clear();
                chan.extend(sel_lab.iter().map(|p| p[c]));
                *out = imgops::median_in_place(&mut chan);
            }
            rgb[cell] = imgops::oklab_to_rgb8(med);
        }
    }

    let floors = groups
        .iter()
        .map(|g| {
            (
                imgops::median(&g.iter().map(|&i| rects[i].y0).collect::<Vec<_>>()),
                imgops::median(&g.iter().map(|&i| rects[i].y1).collect::<Vec<_>>()),
            )
        })
        .collect();

    Openings {
        rows,
        cols,
        cls,
        rgb,
        evidence,
        rects,
        opening: m,
        floors,
        rhythm,
    }
}

/// The openings of one fused wall texture.
pub fn detect(
    tex: &super::fuse::FusedTexture,
    rows: usize,
    cols: usize,
    origin_px: (i32, i32),
) -> Openings {
    let (w, h) = (tex.rgb.width() as usize, tex.rgb.height() as usize);
    let rgb: Vec<[u8; 3]> = tex.rgb.pixels().map(|p| p.0).collect();
    classify(
        &WallTexture::new(w, h, rgb, tex.valid.clone()),
        rows,
        cols,
        origin_px,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain wall with three dark windows on one floor, drawn at 8 px per
    /// metre, is the smallest input that exercises the whole pass.
    fn painted_wall() -> (WallTexture, usize, usize) {
        let (cols, rows) = (10usize, 6usize);
        let (w, h) = (cols * PPM, rows * PPM);
        let mut rgb = vec![[190u8, 185, 175]; w * h];
        for k in 0..3 {
            let x0 = 8 + k * 24;
            for y in 16..32 {
                for x in x0..x0 + 10 {
                    rgb[y * w + x] = [40, 42, 48];
                }
            }
        }
        (WallTexture::new(w, h, rgb, vec![true; w * h]), rows, cols)
    }

    #[test]
    fn three_dark_patches_become_three_windows() {
        let (tex, rows, cols) = painted_wall();
        let out = classify(&tex, rows, cols, (0, 0));
        let wins: Vec<&Rect> = out
            .rects
            .iter()
            .filter(|r| r.kind == Kind::Window)
            .collect();
        assert_eq!(wins.len(), 3, "{:?}", out.rects);
        assert_eq!(out.floors.len(), 1);
        let window_cells = out.cls.iter().filter(|c| **c == CLS_WINDOW).count();
        assert_eq!(window_cells, 6, "two rows of three one metre windows");
    }

    #[test]
    fn unobserved_texels_become_no_data_cells() {
        let (mut tex, rows, cols) = painted_wall();
        for y in 0..PPM {
            for x in 0..tex.w {
                tex.valid[y * tex.w + x] = false;
            }
        }
        let out = classify(&tex, rows, cols, (0, 0));
        assert!(out.cls[..cols].iter().all(|c| *c == CLS_NODATA));
        assert!(out.cls[cols..2 * cols].iter().all(|c| *c != CLS_NODATA));
    }

    #[test]
    fn a_lone_tiny_faint_blob_is_dropped() {
        let (cols, rows) = (6usize, 5usize);
        let (w, h) = (cols * PPM, rows * PPM);
        let mut rgb = vec![[190u8, 185, 175]; w * h];
        for y in 12..16 {
            for x in 12..16 {
                rgb[y * w + x] = [150, 147, 140];
            }
        }
        let out = classify(
            &WallTexture::new(w, h, rgb, vec![true; w * h]),
            rows,
            cols,
            (0, 0),
        );
        assert!(
            out.rects.iter().all(|r| r.kind != Kind::Window),
            "{:?}",
            out.rects
        );
        assert!(out.cls.iter().all(|c| *c == CLS_WALL));
    }

    /// The two rejection reasons the Munich fixture never reaches, so they are
    /// not untested code. Both cases were run through the Python first and it
    /// gives the same reason on the same input.
    #[test]
    fn a_light_green_blob_is_canopy_and_never_a_shop_front() {
        // Bright enough not to be caught by the foliage mask (which needs
        // L < 0.6), green enough that the chroma rule calls it an opening.
        let (cols, rows) = (12usize, 6usize);
        let (w, h) = (cols * PPM, rows * PPM);
        let mut rgb = vec![[200u8, 190, 170]; w * h];
        for y in 16..32 {
            for x in 24..48 {
                rgb[y * w + x] = [150, 200, 140];
            }
        }
        let out = classify(
            &WallTexture::new(w, h, rgb, vec![true; w * h]),
            rows,
            cols,
            (0, 0),
        );
        assert_eq!(out.rects.len(), 1, "{:?}", out.rects);
        assert_eq!(out.rects[0].kind, Kind::Rejected);
        assert_eq!(out.rects[0].reason, "tree");
        assert!(out.cls.iter().all(|c| *c == CLS_WALL));
    }

    #[test]
    fn one_full_width_glassy_band_is_a_parapet() {
        // A ribbon facade has several of these; a single one across most of the
        // wall is a balcony slab or a parapet, and a false glass band costs more
        // in the game than a missed window.
        let (cols, rows) = (9usize, 6usize);
        let (w, h) = (cols * PPM, rows * PPM);
        let mut rgb = vec![[215u8, 210, 200]; w * h];
        for y in 16..28 {
            for x in 8..64 {
                rgb[y * w + x] = [60, 62, 68];
            }
        }
        let out = classify(
            &WallTexture::new(w, h, rgb, vec![true; w * h]),
            rows,
            cols,
            (0, 0),
        );
        assert_eq!(out.rects.len(), 1, "{:?}", out.rects);
        assert_eq!(out.rects[0].kind, Kind::Rejected);
        assert_eq!(out.rects[0].reason, "parapet");
        assert_eq!(out.rects[0].w_m(), 7.0);
        assert!(out.cls.iter().all(|c| *c == CLS_WALL));
    }

    #[test]
    fn the_cell_span_centres_and_clamps() {
        assert_eq!(span(0.0, 8.0, 0, 4, 1), (0, 1));
        assert_eq!(span(8.0, 16.0, 0, 4, 1), (1, 2));
        // Two cells under a window whose centre sits on a cell boundary: the
        // half cell rounds up, so the pair starts at the cell the centre is in.
        assert_eq!(span(12.0, 28.0, 0, 4, 2), (2, 4));
        assert_eq!(span(11.0, 27.0, 0, 4, 2), (1, 3));
        // Never past the wall.
        assert_eq!(span(100.0, 120.0, 0, 4, 2), (2, 4));
    }

    #[test]
    fn cell_counts_round_up_only_past_one_point_seven_metres() {
        assert_eq!(cells(0.4), 1);
        assert_eq!(cells(1.4), 1);
        assert_eq!(cells(1.69), 1);
        assert_eq!(cells(1.8), 2);
        assert_eq!(cells(2.9), 3);
    }

    #[test]
    fn arange_counts_like_numpy() {
        assert_eq!(arange(1.0, 0.05).len(), 20);
        assert_eq!(arange(2.4, 0.05).len(), 48);
        assert_eq!(arange(0.3, 0.05).len(), 6);
        assert!((arange(1.0, 0.05)[3] - 0.15).abs() < 1e-12);
    }

    /// Every wall of the fixture through `classify`, against the Python's own
    /// products. The classes are the thing the game sees, so they are the
    /// assertion; the rectangles are printed on a failure because they say
    /// which rule went a different way.
    #[test]
    fn the_opening_pass_reproduces_the_python() {
        if golden::pixels_absent() {
            return;
        }

        use crate::mapillary::golden;

        let walls = golden::openings_walls();
        assert!(walls.len() >= 30, "the fixture must span at least 30 walls");
        let mut worst = (1.0f64, String::new());
        let mut rows = Vec::new();
        for wall in &walls {
            let tex = wall.texture();
            let out = classify(
                &tex,
                wall.rows,
                wall.cols,
                (wall.origin_px[0], wall.origin_px[1]),
            );
            assert_eq!(out.rows, wall.rows);
            assert_eq!(out.cols, wall.cols);
            let n = wall.rows * wall.cols;
            let same = (0..n)
                .filter(|&i| out.cls[i] == wall.openings.cls[i])
                .count();
            let agree = same as f64 / n as f64;

            // the cell colours and the opening evidence are the rest of what
            // this stage hands to `bands`, so they are asserted exactly rather
            // than left to the class grid to imply
            for i in 0..n {
                let theirs = [
                    wall.openings.rgb[3 * i],
                    wall.openings.rgb[3 * i + 1],
                    wall.openings.rgb[3 * i + 2],
                ];
                assert_eq!(
                    out.rgb[i], theirs,
                    "{}: cell {} of {} has a different colour",
                    wall.key, i, n
                );
                assert!(
                    (out.evidence[i] - wall.openings.evidence[i]).abs() < 2e-5,
                    "{}: cell {} evidence {} against {}",
                    wall.key,
                    i,
                    out.evidence[i],
                    wall.openings.evidence[i]
                );
            }

            // and every rectangle with the rule that kept or dropped it, in the
            // same order, because the order is what breaks ties in the floor
            // grouping and in the rasteriser
            assert_eq!(
                out.rects.len(),
                wall.openings.rects.len(),
                "{}: rectangle count",
                wall.key
            );
            for (k, (ours, theirs)) in out.rects.iter().zip(wall.openings.rects.iter()).enumerate()
            {
                let kind = match ours.kind {
                    Kind::Window => "window",
                    Kind::Door => "door",
                    Kind::Shop => "shop",
                    Kind::Rejected => "rejected",
                };
                assert_eq!(kind, theirs.kind, "{}: rect {k} kind", wall.key);
                assert_eq!(ours.reason, theirs.reason, "{}: rect {k} reason", wall.key);
                for (a, b, what) in [
                    (ours.x0, theirs.x0, "x0"),
                    (ours.y0, theirs.y0, "y0"),
                    (ours.x1, theirs.x1, "x1"),
                    (ours.y1, theirs.y1, "y1"),
                ] {
                    assert!(
                        (a - b).abs() < 1e-4,
                        "{}: rect {k} {what} {a} against {b}",
                        wall.key
                    );
                }
                assert_eq!(ours.n_cols, theirs.n_cols, "{}: rect {k} cols", wall.key);
                assert_eq!(ours.n_rows, theirs.n_rows, "{}: rect {k} rows", wall.key);
            }
            assert_eq!(
                out.rhythm.accepted, wall.openings.rhythm.accepted,
                "{}: rhythm accepted",
                wall.key
            );
            assert!(
                (out.rhythm.pitch - wall.openings.rhythm.pitch).abs() < 1e-6
                    && (out.rhythm.phase - wall.openings.rhythm.phase).abs() < 1e-6
                    && (out.rhythm.support - wall.openings.rhythm.support).abs() < 1e-6,
                "{}: rhythm {:?} against pitch {} phase {} support {}",
                wall.key,
                out.rhythm,
                wall.openings.rhythm.pitch,
                wall.openings.rhythm.phase,
                wall.openings.rhythm.support
            );
            assert_eq!(
                out.floors.len(),
                wall.openings.floors.len(),
                "{}: floor count",
                wall.key
            );
            let py_wins = wall
                .openings
                .rects
                .iter()
                .filter(|r| r.kind == "window")
                .count();
            let rs_wins = out.rects.iter().filter(|r| r.kind == Kind::Window).count();
            rows.push(format!(
                "  {:<16} {} {:>3}x{:<3} cells {:6.2}%  rects {:>3}/{:>3}  windows {:>3}/{:>3}  \
                 rhythm {}{:.2}/{:.2}",
                wall.key,
                wall.tier,
                wall.cols,
                wall.rows,
                100.0 * agree,
                out.rects.len(),
                wall.openings.rects.len(),
                rs_wins,
                py_wins,
                if out.rhythm.accepted == wall.openings.rhythm.accepted {
                    " "
                } else {
                    "!"
                },
                out.rhythm.pitch,
                wall.openings.rhythm.pitch,
            ));
            if agree < worst.0 {
                worst = (agree, wall.key.clone());
            }
        }
        println!("openings against Python, {} walls:", walls.len());
        for r in &rows {
            println!("{r}");
        }
        println!("  worst {} at {:.2}%", worst.1, 100.0 * worst.0);
        assert!(
            worst.0 >= 0.97,
            "wall {} agrees on only {:.2}% of cells",
            worst.1,
            100.0 * worst.0
        );
    }

    #[test]
    fn the_template_pass_stays_off() {
        // MEASURED.md: at TM_SCORE it added 3 windows in 1853 on 2 of 103 walls
        // and helped none of the bright-window walls it was written for.
        const { assert!(!TEMPLATE_PASS) };
        assert!((TM_SCORE - 0.70).abs() < 1e-12);
    }
}
