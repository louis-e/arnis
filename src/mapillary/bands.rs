//! Rows into floor bands. Port of `tools/facade_lab/bands.py`.
//!
//! One block per metre cell, each picked by its own colour, reads as noise in
//! the game. This pass rebuilds the structure a facade actually has before the
//! grid is handed over:
//!
//! * rows are grouped into colour bands by a change-point search over the row
//!   medians in `(0.5 L, a, b)`, so a differently rendered ground floor or a
//!   cornice keeps its own colour and everything else shares one. Splits that
//!   are lightness-only get merged back, because that is shading, not material;
//! * sky rows above the roofline and the dark eave shadow, which the classifier
//!   reads as a row of windows, are dropped or turned back into wall;
//! * windows are completed per floor with observed-only support and no
//!   widening: a column gets one when its floor has windows in most window
//!   columns, the column has windows on most floors, and the texture shows at
//!   least a trace of an opening there. Nothing is ever removed: every rule
//!   tried for stray windows also hit real ones at the wall ends, and a spare
//!   window costs less than a missing one;
//! * the block picker weights chroma double, which `facade_block_for_color` in
//!   `block_palette.rs` already does, so this module calls it rather than
//!   carrying a second palette.
//!
//! Measured on the Munich walls the band colours sit a median of 0.034 OkLab
//! from the nearest Arnis block, and no vanilla block addition moves that by
//! more than 0.002. The palette is not the bottleneck; the per-cell decision
//! was.

#![allow(dead_code)]

use super::imgops;
use super::openings::{
    self, Openings, WallTexture, CLS_DOOR, CLS_NODATA, CLS_UNKNOWN, CLS_WALL, CLS_WINDOW,
};
use crate::block_definitions::Block;

/// OkLab distance that reads as a different material at block scale.
pub const BAND_TAU: f64 = 0.05;
const BAND_MAX: usize = 5;
/// Window pitch in metres.
const LATTICE_PERIODS: std::ops::Range<usize> = 2..10;
/// Window width in metres.
const LATTICE_WIDTHS: [usize; 3] = [1, 2, 3];
/// Floor height in metres.
const ROW_PERIODS: std::ops::Range<usize> = 2..6;
/// On-lattice minus off-lattice window share.
const LATTICE_MIN_SCORE: f64 = 0.15;
/// Mean window share of the on-lattice columns.
const LATTICE_MIN_ON: f64 = 0.25;
/// Share of the detected windows the lattice must explain.
const LATTICE_MIN_COVER: f64 = 0.7;
/// A row with this window share is a window row.
const FLOOR_SHARE: f64 = 0.2;
/// A column with this window share is a window column.
const COL_SHARE: f64 = 0.25;
/// A row this glassy is a shop front: kept as detected.
const SHOP_SHARE: f64 = 0.6;
/// Floor and column support needed before a window is added.
const SUPPORT: f64 = 0.5;
/// And, with the texture at hand, this share of opening-like pixels in the cell.
const MIN_EVIDENCE: f64 = 0.12;

const WINDOW_FALLBACK: [u8; 3] = [60, 65, 75];
const DOOR_FALLBACK: [u8; 3] = [70, 50, 35];

// --------------------------------------------------------------------------- types

/// One horizontal band of rows that share a colour. `r1` is inclusive, the way
/// the Python records it.
#[derive(Clone, Copy, Debug)]
pub struct Band {
    pub r0: usize,
    pub r1: usize,
    /// `None` when the band has no wall cell at all.
    pub lab: Option<[f64; 3]>,
    pub rgb: Option<[u8; 3]>,
    pub window_rgb: Option<[u8; 3]>,
    pub rows_with_wall: usize,
}

impl Band {
    pub fn height(&self) -> usize {
        self.r1 - self.r0 + 1
    }
}

/// The periodic model of the window columns and rows.
#[derive(Clone, Debug, Default)]
pub struct Lattice {
    pub period: Option<usize>,
    pub width: usize,
    pub phase: usize,
    pub score: f64,
    pub on_mean: f64,
    pub cover: f64,
    pub accepted: bool,
    /// The periodic model's columns.
    pub cols_on: Vec<bool>,
    /// Window columns actually used: the observed ones, plus the model's when
    /// it was accepted.
    pub cols_win: Vec<bool>,
    pub rows_on: Vec<bool>,
    pub row_period: Option<usize>,
    pub row_score: f64,
    /// Window share per column.
    pub p: Vec<f64>,
    /// Window share per row.
    pub q: Vec<f64>,
}

impl Lattice {
    /// The columns the periodic model claims, as indices. The export writes
    /// these three lists, so they are named here rather than re-derived from
    /// the masks at every call site.
    pub fn model_columns(&self) -> Vec<usize> {
        indices(&self.cols_on)
    }

    /// The columns that actually carry windows: the observed ones, plus the
    /// model's when it was accepted.
    pub fn window_columns(&self) -> Vec<usize> {
        indices(&self.cols_win)
    }

    /// The rows a floor of windows sits on.
    pub fn floor_rows(&self) -> Vec<usize> {
        indices(&self.rows_on)
    }
}

fn indices(mask: &[bool]) -> Vec<usize> {
    mask.iter()
        .enumerate()
        .filter(|(_, v)| **v)
        .map(|(i, _)| i)
        .collect()
}

/// The structure of one wall.
#[derive(Clone, Debug)]
pub struct Structure {
    pub rows: usize,
    pub cols: usize,
    pub bands: Vec<Band>,
    pub lattice: Lattice,
    pub cls: Vec<u8>,
    pub rgb: Vec<[u8; 3]>,
    pub added: Vec<bool>,
    pub removed: Vec<bool>,
    pub door_rgb: [u8; 3],
    pub sky_rows: usize,
    pub sky_cells: usize,
    /// The opening pass, when a texture was available.
    pub openings: Option<Openings>,
}

/// Where the per cell classes come from.
pub enum CellSource<'a> {
    /// The pipeline's own path: `openings` runs on the 8 px per metre texture
    /// and its classes replace anything a per cell classifier would produce.
    Texture {
        tex: &'a WallTexture,
        /// The sub-block grid phase in texture pixels.
        origin_px: (i32, i32),
    },
    /// A grid already classified per cell, which is what the review tools feed
    /// in when the texture is not on disk.
    Cells { rgb: &'a [[u8; 3]], cls: &'a [u8] },
}

// --------------------------------------------------------------------------- colour bands

/// Median OkLab of the observed wall cells per row (`None` where a row has
/// none) and the row weight, the share of the row that is wall.
fn row_colours(
    rgb: &[[u8; 3]],
    cls: &[u8],
    observed: &[bool],
    rows: usize,
    cols: usize,
) -> (Vec<Option<[f64; 3]>>, Vec<f64>) {
    let mut out = vec![None; rows];
    let mut w = vec![0.0f64; rows];
    let mut chan: Vec<f64> = Vec::with_capacity(cols);
    for r in 0..rows {
        let picked: Vec<[f64; 3]> = (0..cols)
            .filter(|&c| {
                let i = r * cols + c;
                (cls[i] == CLS_WALL || cls[i] == CLS_UNKNOWN) && observed[i]
            })
            .map(|c| imgops::srgb_to_oklab(rgb[r * cols + c]))
            .collect();
        if picked.is_empty() {
            continue;
        }
        let mut med = [0.0f64; 3];
        for (k, m) in med.iter_mut().enumerate() {
            chan.clear();
            chan.extend(picked.iter().map(|p| p[k]));
            *m = imgops::median_in_place(&mut chan);
        }
        out[r] = Some(med);
        w[r] = picked.len() as f64 / cols as f64;
    }
    (out, w)
}

/// Splits the rows into at most `BAND_MAX` colour bands.
///
/// A band costs the weighted squared OkLab spread of its rows about their mean,
/// every band costs `1.5 * tau^2` on top, so two neighbouring floors only get
/// separate colours when they differ by about `tau` over a few rows, and a
/// single row (a plinth, a cornice) only when it stands out by roughly
/// `1.2 * tau`. Rows without wall cells are free and take the colour of the
/// band they land in.
pub fn segment_bands(row_lab: &[Option<[f64; 3]>], row_w: &[f64], tau: f64) -> Vec<Band> {
    let r = row_w.len();
    if r == 0 {
        return Vec::new();
    }
    let finite: Vec<bool> = row_lab.iter().map(|v| v.is_some()).collect();
    let x: Vec<[f64; 3]> = row_lab.iter().map(|v| v.unwrap_or([0.0; 3])).collect();
    let w: Vec<f64> = (0..r)
        .map(|i| if finite[i] { row_w[i] } else { 0.0 })
        .collect();

    let mut s0 = vec![0.0f64; r + 1];
    let mut s1 = vec![[0.0f64; 3]; r + 1];
    // the split cost weighs lightness at half: sun and shade change L, a material
    // change shows in a and b as well, and only the second deserves a new band
    let mut s1c = vec![[0.0f64; 3]; r + 1];
    let mut s2c = vec![[0.0f64; 3]; r + 1];
    for i in 0..r {
        s0[i + 1] = s0[i] + w[i];
        for k in 0..3 {
            let scale = if k == 0 { 0.5 } else { 1.0 };
            let xc = x[i][k] * scale;
            s1[i + 1][k] = s1[i][k] + w[i] * x[i][k];
            s1c[i + 1][k] = s1c[i][k] + w[i] * xc;
            s2c[i + 1][k] = s2c[i][k] + w[i] * xc * xc;
        }
    }

    // rows i..=j
    let cost = |i: usize, j: usize| -> f64 {
        let n = s0[j + 1] - s0[i];
        if n <= 1e-9 {
            return 0.0;
        }
        let mut acc = 0.0;
        for k in 0..3 {
            let a = s1c[j + 1][k] - s1c[i][k];
            let b = s2c[j + 1][k] - s2c[i][k];
            acc += b - a * a / n;
        }
        acc.max(0.0)
    };

    let lam = 1.5 * tau * tau;
    let k_max = BAND_MAX.min(r).max(1);
    let inf = f64::INFINITY;
    // one band count at a time: only the previous one is ever read
    let mut prev = vec![inf; r + 1];
    prev[0] = 0.0;
    let mut back = vec![vec![0usize; r + 1]; k_max + 1];
    // the whole wall in k bands, which is what the per band penalty is added to
    let mut whole = vec![inf; k_max + 1];
    for k in 1..=k_max {
        let mut cur = vec![inf; r + 1];
        for j in 1..=r {
            let (mut best, mut bi) = (inf, 0usize);
            for (i, &p) in prev.iter().enumerate().take(j) {
                if p.is_infinite() {
                    continue;
                }
                let c = p + cost(i, j - 1);
                if c < best {
                    best = c;
                    bi = i;
                }
            }
            cur[j] = best;
            back[k][j] = bi;
        }
        whole[k] = cur[r];
        prev = cur;
    }
    let mut k_best = 1usize;
    let mut best_total = whole[1] + lam;
    for (k, &cost_k) in whole.iter().enumerate().skip(2) {
        let total = cost_k + lam * k as f64;
        if total < best_total {
            best_total = total;
            k_best = k;
        }
    }
    let mut bounds: Vec<(usize, usize)> = Vec::new();
    let mut j = r;
    for k in (1..=k_best).rev() {
        let i = back[k][j];
        bounds.push((i, j - 1));
        j = i;
    }
    bounds.reverse();

    let lab_of = |i: usize, j: usize| -> Option<[f64; 3]> {
        let n = s0[j + 1] - s0[i];
        if n <= 1e-9 {
            None
        } else {
            let mut out = [0.0f64; 3];
            for (k, o) in out.iter_mut().enumerate() {
                *o = (s1[j + 1][k] - s1[i][k]) / n;
            }
            Some(out)
        }
    };

    let mut bands: Vec<Band> = bounds
        .iter()
        .map(|&(i, j)| Band {
            r0: i,
            r1: j,
            lab: lab_of(i, j),
            rgb: None,
            window_rgb: None,
            rows_with_wall: finite[i..=j].iter().filter(|f| **f).count(),
        })
        .collect();

    // a band without any wall takes the colour of the nearest band that has one
    let coloured: Vec<(usize, usize, [f64; 3])> = bands
        .iter()
        .filter_map(|b| b.lab.map(|l| (b.r0, b.r1, l)))
        .collect();
    if !coloured.is_empty() {
        for b in bands.iter_mut() {
            if b.lab.is_none() {
                let (_, _, l) = coloured
                    .iter()
                    .min_by_key(|(cr0, cr1, _)| (cr0.abs_diff(b.r1)).min(b.r0.abs_diff(*cr1)))
                    .unwrap();
                b.lab = Some(*l);
            }
        }
    }

    let mean_of =
        |b0: &Band, b1: &Band| -> Option<[f64; 3]> { lab_of(b0.r0, b1.r1).or(b0.lab).or(b1.lab) };
    // the row-to-row colour step across a boundary; rows with no wall count as no step
    let step_at = |row: usize| -> f64 {
        if row == 0 || row >= r || !finite[row - 1] || !finite[row] {
            return 0.0;
        }
        let (a, b) = (row_lab[row].unwrap(), row_lab[row - 1].unwrap());
        imgops::oklab_distance(a, b)
    };

    // bands that differ in lightness only are sun and shade on one material, unless
    // the boundary itself is a sharp step (a cornice, the edge of a dark plinth)
    let mut merged = true;
    while merged && bands.len() > 1 {
        merged = false;
        for k in 0..bands.len() - 1 {
            let (b0, b1) = (bands[k], bands[k + 1]);
            let (Some(l0), Some(l1)) = (b0.lab, b1.lab) else {
                continue;
            };
            let d_ab = ((l0[1] - l1[1]).powi(2) + (l0[2] - l1[2]).powi(2)).sqrt();
            let d_l = (l0[0] - l1[0]).abs();
            if d_ab < 0.02 && d_l < 0.12 && step_at(b1.r0) < tau {
                bands[k] = Band {
                    r0: b0.r0,
                    r1: b1.r1,
                    lab: mean_of(&b0, &b1),
                    rgb: None,
                    window_rgb: None,
                    rows_with_wall: b0.rows_with_wall + b1.rows_with_wall,
                };
                bands.remove(k + 1);
                merged = true;
                break;
            }
        }
    }
    // an interior band of a single row is a shadow line unless it really stands out
    let mut changed = true;
    while changed && bands.len() > 2 {
        changed = false;
        for k in 1..bands.len() - 1 {
            let b = bands[k];
            if b.height() != 1 || b.lab.is_none() {
                continue;
            }
            let bl = b.lab.unwrap();
            let (prev_b, next_b) = (bands[k - 1], bands[k + 1]);
            let d = |o: &Band| o.lab.map_or(9.0, |l| imgops::oklab_distance(l, bl));
            // Python's min keeps the first on a tie, which is the band above.
            let use_prev = d(&prev_b) <= d(&next_b);
            let near = if use_prev { prev_b } else { next_b };
            if near.lab.is_none() || d(&near) > 2.0 * tau {
                continue;
            }
            if use_prev {
                bands[k - 1] = Band {
                    r0: prev_b.r0,
                    r1: b.r1,
                    lab: mean_of(&prev_b, &b),
                    rgb: None,
                    window_rgb: None,
                    rows_with_wall: prev_b.rows_with_wall + b.rows_with_wall,
                };
            } else {
                bands[k + 1] = Band {
                    r0: b.r0,
                    r1: next_b.r1,
                    lab: mean_of(&b, &next_b),
                    rgb: None,
                    window_rgb: None,
                    rows_with_wall: b.rows_with_wall + next_b.rows_with_wall,
                };
            }
            bands.remove(k);
            changed = true;
            break;
        }
    }
    // a near-black bottom band of up to two rows is the pavement or its shadow, not
    // a plinth; a dark band of up to two rows right under the top is the eave shadow
    let n = bands.len();
    if n >= 2 && bands[n - 1].height() <= 2 {
        if let (Some(last), Some(prev)) = (bands[n - 1].lab, bands[n - 2].lab) {
            if last[0] < 0.35 && prev[0] - last[0] > 0.25 {
                bands[n - 1].lab = Some(prev);
            }
        }
    }
    if n >= 2 && bands[0].height() <= 2 {
        if let (Some(first), Some(second)) = (bands[0].lab, bands[1].lab) {
            if first[0] < 0.4 && second[0] - first[0] > 0.25 {
                bands[0].lab = Some(second);
            }
        }
    }
    for b in bands.iter_mut() {
        b.rgb = b.lab.map(imgops::oklab_to_rgb8);
    }
    bands
}

// --------------------------------------------------------------------------- lattice

/// Python's `round(x, 6)`: the nearest multiple of 1e-6, ties to even. It is
/// there so two candidates that are equal up to float noise compare equal and
/// the narrower, shorter one wins the tie; without it the lattice picks a
/// multiple of the true period on some walls.
fn round6(x: f64) -> f64 {
    imgops::round_half_even(x * 1e6) / 1e6
}

/// One candidate periodic model: the columns (or rows) it claims and the margin
/// by which they carry more window share than everything else.
struct LatticeFit {
    score: f64,
    period: usize,
    width: usize,
    phase: usize,
    on: Vec<bool>,
}

/// The lattice whose members carry the most window share compared with
/// everything else. A period that is a multiple of the true one leaves real
/// windows off the lattice, which lowers its score, so the fundamental wins
/// without a special rule.
fn best_lattice(
    profile: &[f64],
    periods: std::ops::Range<usize>,
    widths: &[usize],
) -> Option<LatticeFit> {
    let n = profile.len();
    // the Python's sort key: the rounded score first, then the narrower and the
    // shorter period, so a tie goes to the simpler model
    let mut best: Option<((f64, i64, i64), LatticeFit)> = None;
    for t in periods {
        if t > n {
            break;
        }
        for &wd in widths {
            if wd >= t {
                continue;
            }
            for phi in 0..t {
                // `(i - phi) % t` with Python's non-negative modulo, which is
                // what puts the lattice's first member at `phi`
                let on: Vec<bool> = (0..n).map(|i| (i + t - phi) % t < wd).collect();
                let n_on = on.iter().filter(|v| **v).count();
                if n_on < 2 || n_on == n {
                    continue;
                }
                let sum_on: f64 = (0..n).filter(|&i| on[i]).map(|i| profile[i]).sum();
                let sum_off: f64 = (0..n).filter(|&i| !on[i]).map(|i| profile[i]).sum();
                let score = sum_on / n_on as f64 - sum_off / (n - n_on) as f64;
                let key = (round6(score), -(wd as i64), -(t as i64));
                let better = match &best {
                    None => true,
                    Some((bk, _)) => {
                        key.0 > bk.0
                            || (key.0 == bk.0 && (key.1 > bk.1 || (key.1 == bk.1 && key.2 > bk.2)))
                    }
                };
                if better {
                    best = Some((
                        key,
                        LatticeFit {
                            score: key.0,
                            period: t,
                            width: wd,
                            phase: phi,
                            on,
                        },
                    ));
                }
            }
        }
    }
    best.map(|(_, fit)| fit)
}

fn mean_where(profile: &[f64], on: &[bool]) -> f64 {
    let n = on.iter().filter(|v| **v).count();
    if n == 0 {
        return f64::NAN;
    }
    (0..profile.len())
        .filter(|&i| on[i])
        .map(|i| profile[i])
        .sum::<f64>()
        / n as f64
}

pub fn find_lattice(cls: &[u8], observed: &[bool], rows: usize, cols: usize) -> Lattice {
    let win: Vec<bool> = cls.iter().map(|c| *c == CLS_WINDOW).collect();
    let mut p = vec![0.0f64; cols];
    for (c, item) in p.iter_mut().enumerate() {
        let obs = (0..rows).filter(|&r| observed[r * cols + c]).count();
        if obs > 0 {
            let w = (0..rows).filter(|&r| win[r * cols + c]).count();
            *item = w as f64 / obs as f64;
        }
    }
    let mut q = vec![0.0f64; rows];
    for (r, item) in q.iter_mut().enumerate() {
        let obs = (0..cols).filter(|&c| observed[r * cols + c]).count();
        if obs > 0 {
            let w = (0..cols).filter(|&c| win[r * cols + c]).count();
            *item = w as f64 / obs as f64;
        }
    }

    let mut lat = Lattice {
        cols_on: vec![false; cols],
        rows_on: q.iter().map(|v| *v >= FLOOR_SHARE).collect(),
        p: p.clone(),
        q: q.clone(),
        ..Default::default()
    };
    if let Some(fit) = best_lattice(&p, LATTICE_PERIODS, &LATTICE_WIDTHS) {
        lat.period = Some(fit.period);
        lat.width = fit.width;
        lat.phase = fit.phase;
        lat.score = fit.score;
        lat.on_mean = mean_where(&p, &fit.on);
        // a lattice that leaves a third of the windows unexplained has the wrong
        // period, and acting on it would move real windows around
        let total: usize = win.iter().filter(|v| **v).count();
        let on_count: usize = (0..rows * cols)
            .filter(|&i| win[i] && fit.on[i % cols])
            .count();
        lat.cover = on_count as f64 / total.max(1) as f64;
        lat.cols_on = fit.on;
        lat.accepted = fit.score >= LATTICE_MIN_SCORE
            && lat.on_mean >= LATTICE_MIN_ON
            && lat.cover >= LATTICE_MIN_COVER;
    }
    // floors: the row model only adds rows that already show some window evidence
    if let Some(fit) = best_lattice(&q, ROW_PERIODS, &[1, 2]) {
        lat.row_period = Some(fit.period);
        lat.row_score = fit.score;
        if fit.score >= LATTICE_MIN_SCORE && mean_where(&q, &fit.on) >= 0.2 {
            for ((slot, on), share) in lat.rows_on.iter_mut().zip(fit.on).zip(q.iter()) {
                *slot = *slot || (on && *share >= 0.08);
            }
        }
    }
    // Real window pitches are rarely whole metres, so the columns that matter are
    // the observed ones; the periodic model only adds columns when it explains
    // nearly all windows (a column hidden behind a tree in every view).
    lat.cols_win = p.iter().map(|v| *v >= COL_SHARE).collect();
    if lat.accepted {
        for (slot, on) in lat.cols_win.iter_mut().zip(lat.cols_on.iter()) {
            *slot = *slot || *on;
        }
    }
    lat
}

// --------------------------------------------------------------------------- completion

/// Fills the windows the classifier missed. Works per floor (a run of window
/// rows), so an added window gets the floor's height. A cell is only added when
/// its floor already has windows in most window columns, its column has windows
/// on most floors and, when the texture is at hand, the cell itself shows at
/// least a trace of an opening. Returns `(classes, added, removed)`.
pub fn complete_windows(
    cls: &[u8],
    observed: &[bool],
    lat: &Lattice,
    evidence: Option<&[f64]>,
    rows: usize,
    cols: usize,
) -> (Vec<u8>, Vec<bool>, Vec<bool>) {
    let win: Vec<bool> = cls.iter().map(|c| *c == CLS_WINDOW).collect();
    let door: Vec<bool> = cls.iter().map(|c| *c == CLS_DOOR).collect();
    let mut out = cls.to_vec();
    let mut added = vec![false; rows * cols];
    let removed = vec![false; rows * cols];
    let q = &lat.q;

    // shop fronts: one missing cell between two glass cells is glass, ground floor only
    for (r, &share) in q.iter().enumerate().skip(rows.saturating_sub(4)) {
        if share >= SHOP_SHARE {
            for c in 1..cols.saturating_sub(1) {
                let i = r * cols + c;
                if !win[i]
                    && win[i - 1]
                    && win[i + 1]
                    && (out[i] == CLS_WALL || out[i] == CLS_UNKNOWN)
                {
                    out[i] = CLS_WINDOW;
                    added[i] = true;
                }
            }
        }
    }

    let floor_rows: Vec<bool> = (0..rows)
        .map(|r| lat.rows_on[r] && q[r] < SHOP_SHARE)
        .collect();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut r = 0;
    while r < rows {
        if floor_rows[r] {
            let mut r1 = r;
            while r1 + 1 < rows && floor_rows[r1 + 1] {
                r1 += 1;
            }
            groups.push((r..=r1).collect());
            r = r1 + 1;
        } else {
            r += 1;
        }
    }
    if groups.len() < 2 {
        return (out, added, removed);
    }

    let has: Vec<Vec<bool>> = groups
        .iter()
        .map(|g| {
            (0..cols)
                .map(|c| g.iter().any(|&rr| win[rr * cols + c]))
                .collect()
        })
        .collect();
    // a window column is one with windows on at least two floors (a shop row must
    // not vouch for a column), plus the periodic model's columns when it is trusted
    let mut on_c: Vec<bool> = (0..cols)
        .map(|c| has.iter().filter(|h| h[c]).count() >= 2)
        .collect();
    if lat.accepted {
        for (slot, on) in on_c.iter_mut().zip(lat.cols_on.iter()) {
            *slot = *slot || *on;
        }
    }
    if on_c.iter().filter(|v| **v).count() < 2 {
        return (out, added, removed);
    }
    // support is measured on observed cells only, so a tree does not count against
    // a floor or a column it hides
    let obs_f: Vec<Vec<bool>> = groups
        .iter()
        .map(|g| {
            (0..cols)
                .map(|c| g.iter().any(|&rr| observed[rr * cols + c]))
                .collect()
        })
        .collect();
    let s_f: Vec<f64> = (0..groups.len())
        .map(|f| {
            let sel: Vec<usize> = (0..cols).filter(|&c| on_c[c] && obs_f[f][c]).collect();
            if sel.is_empty() {
                0.0
            } else {
                sel.iter().filter(|&&c| has[f][c]).count() as f64 / sel.len() as f64
            }
        })
        .collect();
    let s_c: Vec<f64> = (0..cols)
        .map(|c| {
            let sel: Vec<usize> = (0..groups.len()).filter(|&f| obs_f[f][c]).collect();
            if sel.is_empty() {
                0.0
            } else {
                sel.iter().filter(|&&f| has[f][c]).count() as f64 / sel.len() as f64
            }
        })
        .collect();

    for (f, g) in groups.iter().enumerate() {
        if (0..cols).filter(|&c| on_c[c] && has[f][c]).count() < 3 {
            continue;
        }
        // the rows this floor's windows actually occupy: the most common
        // (top, bottom) extent among the floor's window columns, followed beyond
        // the group's rows so an added window is as tall as its neighbours
        let mut extents: Vec<((usize, usize), usize)> = Vec::new();
        for c in 0..cols {
            if !has[f][c] {
                continue;
            }
            let Some(&rr) = g.iter().find(|&&r0| win[r0 * cols + c]) else {
                continue;
            };
            let (mut top_r, mut bot_r) = (rr, rr);
            while top_r > 0 && win[(top_r - 1) * cols + c] {
                top_r -= 1;
            }
            while bot_r + 1 < rows && win[(bot_r + 1) * cols + c] {
                bot_r += 1;
            }
            match extents.iter_mut().find(|(k, _)| *k == (top_r, bot_r)) {
                Some((_, n)) => *n += 1,
                None => extents.push(((top_r, bot_r), 1)),
            }
        }
        // The most common extent, and on a tie the first one met walking the
        // columns: Python's max over a dict keeps the first key of the highest
        // count. `max_by_key` keeps the last instead, which moved a whole floor
        // of added windows one row down on w79817192_6p0, so the fold is
        // written out.
        let best = extents
            .iter()
            .fold(None::<&((usize, usize), usize)>, |acc, e| match acc {
                Some(b) if b.1 >= e.1 => Some(b),
                _ => Some(e),
            });
        let use_rows: Vec<usize> = match best {
            Some(((top_r, bot_r), _)) => (*top_r..=*bot_r).collect(),
            None => g.clone(),
        };

        for c in 0..cols {
            if !on_c[c] || has[f][c] || g.iter().any(|&rr| door[rr * cols + c]) {
                continue;
            }
            // never widen a neighbour: a window next to an existing one on this floor
            // is the pier between two windows, not a missing window
            let left = c > 0 && use_rows.iter().any(|&rr| win[rr * cols + c - 1]);
            let right = c + 1 < cols && use_rows.iter().any(|&rr| win[rr * cols + c + 1]);
            if left || right {
                continue;
            }
            if s_f[f] < SUPPORT || s_c[c] < SUPPORT {
                continue;
            }
            let seen = use_rows.iter().any(|&rr| observed[rr * cols + c]);
            if seen {
                if let Some(ev) = evidence {
                    let mean = use_rows.iter().map(|&rr| ev[rr * cols + c]).sum::<f64>()
                        / use_rows.len() as f64;
                    if mean < MIN_EVIDENCE {
                        continue;
                    }
                }
            } else {
                // nothing observed here (a tree in every view): the floor and the
                // column must each carry at least two observed windows before the
                // rhythm is trusted without evidence
                let floor_seen = (0..cols).filter(|&cc| has[f][cc] && obs_f[f][cc]).count();
                let col_seen = (0..groups.len())
                    .filter(|&ff| has[ff][c] && obs_f[ff][c])
                    .count();
                if floor_seen < 2 || col_seen < 2 {
                    continue;
                }
            }
            for &rr in &use_rows {
                let i = rr * cols + c;
                if out[i] == CLS_WALL || out[i] == CLS_UNKNOWN {
                    out[i] = CLS_WINDOW;
                    added[i] = true;
                }
            }
        }
    }
    // Nothing is removed. Every rule tried for stray windows also hit real ones at
    // the wall ends, and a spare window costs less than a missing one.
    (out, added, removed)
}

/// The per cell classifier calls every dark cell in the bottom three metres a
/// door. A dark run three or more cells wide at ground level is a shop window, a
/// garage or an arcade, so it becomes glass; runs of one or two cells stay
/// doors. Only the no-texture path needs this: `openings` decides doors from the
/// rectangle's own width.
pub fn doors_and_shopfronts(cls: &[u8], rows: usize, cols: usize) -> Vec<u8> {
    let mut out = cls.to_vec();
    for r in 0..rows {
        let mut c = 0;
        while c < cols {
            if out[r * cols + c] != CLS_DOOR {
                c += 1;
                continue;
            }
            let mut c1 = c;
            while c1 < cols && out[r * cols + c1] == CLS_DOOR {
                c1 += 1;
            }
            if c1 - c >= 3 {
                for cc in c..c1 {
                    out[r * cols + cc] = CLS_WINDOW;
                }
            }
            c = c1;
        }
    }
    out
}

// --------------------------------------------------------------------------- sky and eave

/// Sky-coloured wall cells in the top rows are not wall: the roofline sits below
/// the height the texture was cut at. They become no data, and a top row that is
/// mostly sky goes entirely, so the game never gets a light blue row of blocks
/// above a facade. Returns `(classes, rows dropped, cells dropped)`.
pub fn drop_sky(rgb: &[[u8; 3]], cls: &[u8], rows: usize, cols: usize) -> (Vec<u8>, usize, usize) {
    let mut out = cls.to_vec();
    let lab: Vec<[f64; 3]> = rgb.iter().map(|c| imgops::srgb_to_oklab(*c)).collect();
    let wallish: Vec<bool> = cls
        .iter()
        .map(|c| *c == CLS_WALL || *c == CLS_UNKNOWN)
        .collect();
    let lower = rows / 2;
    if rows < 4 || !(lower * cols..rows * cols).any(|i| wallish[i]) {
        return (out, 0, 0);
    }
    // the facade's own colour comes from the lower half, which is never sky; the
    // upper rows are tested one by one from the top, as far down as the wall
    // texture could be over-tall (a mis-tagged height can put half of it in the sky)
    let body = {
        let picked: Vec<[f64; 3]> = (lower * cols..rows * cols)
            .filter(|&i| wallish[i])
            .map(|i| lab[i])
            .collect();
        median_lab(&picked).unwrap()
    };
    let mut dropped_rows = 0usize;
    for r in 0..rows.saturating_sub(3) {
        let row_wall: Vec<[f64; 3]> = (0..cols)
            .filter(|&c| wallish[r * cols + c])
            .map(|c| lab[r * cols + c])
            .collect();
        if row_wall.is_empty() {
            let all_nodata = (0..cols).all(|c| out[r * cols + c] == CLS_NODATA);
            if all_nodata && dropped_rows == r {
                dropped_rows += 1;
                continue;
            }
            break;
        }
        let m = median_lab(&row_wall).unwrap();
        let bluer = (m[2] < -0.06 && m[2] < body[2] - 0.03) || m[2] < body[2] - 0.06;
        if bluer && m[0] > body[0] - 0.08 && dropped_rows == r {
            for c in 0..cols {
                out[r * cols + c] = CLS_NODATA;
            }
            dropped_rows += 1;
        } else {
            break;
        }
    }
    // single strongly blue and bright cells: the sky triangle beside a gable or
    // over a roof step in the top third, and down the two outer columns to half
    // height where a lower neighbour's roofline leaves sky beside the wall. Window
    // cells count too: sky is never glass.
    let mut cells = (0..dropped_rows * cols)
        .filter(|&i| cls[i] != CLS_NODATA)
        .count();
    for r in 0..rows {
        for c in 0..cols {
            let i = r * cols + c;
            let strong = lab[i][2] < -0.07
                && lab[i][0] > 0.70
                && lab[i][2] < body[2] - 0.06
                && cls[i] != CLS_NODATA;
            let zone = r < (rows / 3).max(1) || (r < lower && (c < 2 || c + 2 >= cols));
            if strong && zone {
                cells += 1;
                out[i] = CLS_NODATA;
            }
        }
    }
    // The shadow under the eaves is dark all along the top row, and the classifier
    // reads dark as window. A top row that is almost all window, sitting on a row
    // that is not, is that shadow: it becomes wall (dark, which is what the game
    // should show for a cornice), never a row of glass.
    for r in dropped_rows..(rows.saturating_sub(1)).min((rows / 3).max(1)) {
        let obs = (0..cols)
            .filter(|&c| out[r * cols + c] != CLS_NODATA)
            .count();
        if obs == 0 {
            continue;
        }
        let share = (0..cols)
            .filter(|&c| out[r * cols + c] == CLS_WINDOW)
            .count() as f64
            / obs as f64;
        let below = (0..cols)
            .filter(|&c| out[(r + 1) * cols + c] != CLS_NODATA)
            .count();
        let share_below = (0..cols)
            .filter(|&c| out[(r + 1) * cols + c] == CLS_WINDOW)
            .count() as f64
            / below.max(1) as f64;
        if share >= 0.9 && share_below < 0.6 {
            for c in 0..cols {
                if out[r * cols + c] == CLS_WINDOW {
                    out[r * cols + c] = CLS_WALL;
                }
            }
        } else {
            break;
        }
    }
    (out, dropped_rows, cells)
}

fn median_lab(v: &[[f64; 3]]) -> Option<[f64; 3]> {
    if v.is_empty() {
        return None;
    }
    let mut out = [0.0f64; 3];
    let mut chan: Vec<f64> = Vec::with_capacity(v.len());
    for (k, o) in out.iter_mut().enumerate() {
        chan.clear();
        chan.extend(v.iter().map(|p| p[k]));
        *o = imgops::median_in_place(&mut chan);
    }
    Some(out)
}

fn median_rgb(rgb: &[[u8; 3]], mask: impl Iterator<Item = usize>) -> Option<[u8; 3]> {
    let picked: Vec<[f64; 3]> = mask.map(|i| imgops::srgb_to_oklab(rgb[i])).collect();
    median_lab(&picked).map(imgops::oklab_to_rgb8)
}

/// Every wall cell takes its band colour, every window cell its band's window
/// colour, doors one colour for the wall. Returns `(rgb, door colour)`.
pub fn apply_bands(
    rgb: &[[u8; 3]],
    cls: &[u8],
    bands: &mut [Band],
    cols: usize,
) -> (Vec<[u8; 3]>, [u8; 3]) {
    let mut out = rgb.to_vec();
    let win_all = median_rgb(rgb, (0..cls.len()).filter(|&i| cls[i] == CLS_WINDOW))
        .unwrap_or(WINDOW_FALLBACK);
    let door_rgb =
        median_rgb(rgb, (0..cls.len()).filter(|&i| cls[i] == CLS_DOOR)).unwrap_or(DOOR_FALLBACK);
    for b in bands.iter_mut() {
        let range = b.r0 * cols..(b.r1 + 1) * cols;
        if let Some(c) = b.rgb {
            for i in range.clone() {
                if cls[i] == CLS_WALL || cls[i] == CLS_UNKNOWN {
                    out[i] = c;
                }
            }
        }
        let wrgb =
            median_rgb(rgb, range.clone().filter(|&i| cls[i] == CLS_WINDOW)).unwrap_or(win_all);
        b.window_rgb = Some(wrgb);
        for i in range {
            if cls[i] == CLS_WINDOW {
                out[i] = wrgb;
            }
        }
    }
    for i in 0..cls.len() {
        if cls[i] == CLS_DOOR {
            out[i] = door_rgb;
        }
    }
    (out, door_rgb)
}

// --------------------------------------------------------------------------- the pass

/// Bands, lattice, completed classes and the banded colours for one wall.
pub fn analyse(
    src: CellSource,
    rows: usize,
    cols: usize,
    observed_in: Option<&[bool]>,
    tau: f64,
) -> Structure {
    let (mut cls, rgb, evidence, op) = match src {
        CellSource::Texture { tex, origin_px } => {
            let op = openings::classify(tex, rows, cols, origin_px);
            (
                op.cls.clone(),
                op.rgb.clone(),
                Some(op.evidence.clone()),
                Some(op),
            )
        }
        CellSource::Cells { rgb, cls } => (cls.to_vec(), rgb.to_vec(), None, None),
    };

    let mut observed: Vec<bool> = match observed_in {
        Some(o) => (0..rows * cols)
            .map(|i| o[i] && cls[i] != CLS_NODATA)
            .collect(),
        None => cls.iter().map(|c| *c != CLS_NODATA).collect(),
    };
    let (sky_cls, sky_rows, sky_cells) = drop_sky(&rgb, &cls, rows, cols);
    cls = sky_cls;
    if op.is_none() {
        cls = doors_and_shopfronts(&cls, rows, cols);
    }
    for i in 0..rows * cols {
        observed[i] = observed[i] && cls[i] != CLS_NODATA;
    }

    let (row_lab, row_w) = row_colours(&rgb, &cls, &observed, rows, cols);
    let mut bands = segment_bands(&row_lab, &row_w, tau);
    let lattice = find_lattice(&cls, &observed, rows, cols);
    let (cls2, added, removed) =
        complete_windows(&cls, &observed, &lattice, evidence.as_deref(), rows, cols);
    let (rgb2, door_rgb) = apply_bands(&rgb, &cls2, &mut bands, cols);

    Structure {
        rows,
        cols,
        bands,
        lattice,
        cls: cls2,
        rgb: rgb2,
        added,
        removed,
        door_rgb,
        sky_rows,
        sky_cells,
        openings: op,
    }
}

/// The Arnis block a facade colour lands on.
///
/// The Python `Look.block_for` weights chroma double against lightness, which
/// is what keeps two beiges a shade apart on the same stone instead of sending
/// one to pink terracotta; `block_palette::facade_block_for_color` already does
/// exactly that, so this is a one line delegation and a test that the two
/// palettes agree rather than a second copy of the picker.
pub fn block_for(rgb: [u8; 3]) -> Block {
    crate::block_palette::facade_block_for_color((rgb[0], rgb[1], rgb[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: usize, cols: usize, fill: u8) -> Vec<u8> {
        vec![fill; rows * cols]
    }

    #[test]
    fn one_flat_colour_gives_one_band() {
        let (rows, cols) = (8usize, 6usize);
        let rgb = vec![[170u8, 165, 150]; rows * cols];
        let cls = grid(rows, cols, CLS_WALL);
        let observed = vec![true; rows * cols];
        let (row_lab, row_w) = row_colours(&rgb, &cls, &observed, rows, cols);
        let bands = segment_bands(&row_lab, &row_w, BAND_TAU);
        assert_eq!(bands.len(), 1);
        assert_eq!(bands[0].r0, 0);
        assert_eq!(bands[0].r1, rows - 1);
        assert_eq!(bands[0].rgb, Some([170, 165, 150]));
    }

    #[test]
    fn a_differently_coloured_plinth_gets_its_own_band() {
        let (rows, cols) = (8usize, 6usize);
        let mut rgb = vec![[190u8, 180, 165]; rows * cols];
        // red brick, different in a and b, not only in L
        for px in rgb.iter_mut().skip(6 * cols) {
            *px = [110, 70, 60];
        }
        let cls = grid(rows, cols, CLS_WALL);
        let observed = vec![true; rows * cols];
        let (row_lab, row_w) = row_colours(&rgb, &cls, &observed, rows, cols);
        let bands = segment_bands(&row_lab, &row_w, BAND_TAU);
        assert_eq!(bands.len(), 2, "{bands:?}");
        assert_eq!((bands[1].r0, bands[1].r1), (6, 7));
    }

    #[test]
    fn a_lightness_only_split_is_merged_back() {
        // Sun and shade on one render: the same hue, a step in L under 0.12.
        let (rows, cols) = (8usize, 6usize);
        let mut rgb = vec![[200u8, 195, 185]; rows * cols];
        for px in rgb.iter_mut().skip(4 * cols) {
            *px = [178, 173, 164];
        }
        let cls = grid(rows, cols, CLS_WALL);
        let observed = vec![true; rows * cols];
        let (row_lab, row_w) = row_colours(&rgb, &cls, &observed, rows, cols);
        assert_eq!(segment_bands(&row_lab, &row_w, BAND_TAU).len(), 1);
    }

    #[test]
    fn the_lattice_score_is_rounded_before_the_tie_break() {
        // Two lattices that are equal in exact arithmetic differ by an ulp in
        // floats; the rounding is what lets such a tie fall to the narrower and
        // shorter period rather than to whichever accumulated less error.
        assert_eq!(round6(0.1 + 0.2), round6(0.3));
        assert_eq!(round6(0.2929999999), 0.293);
        assert_eq!(round6(1.0 / 3.0), 0.333333);
        assert_eq!(round6(-0.0), 0.0);
    }

    #[test]
    fn the_lattice_finds_a_three_metre_pitch() {
        let (rows, cols) = (6usize, 12usize);
        let mut cls = grid(rows, cols, CLS_WALL);
        for r in 1..5 {
            for c in (0..cols).step_by(3) {
                cls[r * cols + c] = CLS_WINDOW;
            }
        }
        let observed = vec![true; rows * cols];
        let lat = find_lattice(&cls, &observed, rows, cols);
        assert_eq!(lat.period, Some(3));
        assert_eq!(lat.width, 1);
        assert_eq!(lat.phase, 0);
        assert!(lat.accepted, "{lat:?}");
    }

    #[test]
    fn a_missing_window_is_completed_but_a_neighbour_is_never_widened() {
        let (rows, cols) = (7usize, 11usize);
        let mut cls = grid(rows, cols, CLS_WALL);
        // three floors of windows every third column, with one hole
        for r in [1usize, 3, 5] {
            for c in (1..cols).step_by(3) {
                cls[r * cols + c] = CLS_WINDOW;
            }
        }
        cls[3 * cols + 4] = CLS_WALL;
        let observed = vec![true; rows * cols];
        let lat = find_lattice(&cls, &observed, rows, cols);
        let (out, added, removed) = complete_windows(&cls, &observed, &lat, None, rows, cols);
        assert_eq!(out[3 * cols + 4], CLS_WINDOW, "the hole is filled");
        assert!(added[3 * cols + 4]);
        assert!(!removed.iter().any(|v| *v), "nothing is ever removed");
        // the piers stay wall
        assert_eq!(out[3 * cols + 3], CLS_WALL);
        assert_eq!(out[3 * cols + 5], CLS_WALL);
    }

    #[test]
    fn a_sky_row_above_the_roofline_is_dropped() {
        let (rows, cols) = (8usize, 6usize);
        let mut rgb = vec![[180u8, 172, 158]; rows * cols];
        // sky: much bluer than the body, about as light
        for px in rgb.iter_mut().take(cols) {
            *px = [150, 190, 235];
        }
        let cls = grid(rows, cols, CLS_WALL);
        let (out, dropped, cells) = drop_sky(&rgb, &cls, rows, cols);
        assert_eq!(dropped, 1);
        assert!(out[..cols].iter().all(|c| *c == CLS_NODATA));
        assert!(cells >= cols);
    }

    #[test]
    fn the_eave_shadow_row_becomes_wall_not_glass() {
        let (rows, cols) = (9usize, 8usize);
        let rgb = vec![[120u8, 115, 110]; rows * cols];
        let mut cls = grid(rows, cols, CLS_WALL);
        for cell in cls.iter_mut().take(cols) {
            *cell = CLS_WINDOW;
        }
        let (out, dropped, _) = drop_sky(&rgb, &cls, rows, cols);
        assert_eq!(dropped, 0);
        assert!(out[..cols].iter().all(|c| *c == CLS_WALL));
    }

    /// The whole block product of every fixture wall against the Python.
    ///
    /// Two assertions, both from `PORT_TO_RUST.md`: the class grid agrees on at
    /// least 97 per cent of cells, and every band colour is within 3 units of
    /// 0.01 in OkLab. The colours are compared row by row rather than band by
    /// band, because a band boundary that lands one row differently is not a
    /// wrong colour anywhere; the band count and boundaries are reported
    /// alongside so a real structural difference is still visible.
    #[test]
    fn the_block_product_reproduces_the_python() {
        if golden::pixels_absent() {
            return;
        }

        use crate::mapillary::golden;

        let walls = golden::openings_walls();
        assert!(walls.len() >= 30, "the fixture must span at least 30 walls");
        let mut worst_cells = (1.0f64, String::new());
        let mut worst_colour = (0.0f64, String::new());
        let mut lines = Vec::new();
        let mut exact = 0usize;
        for wall in &walls {
            let tex = wall.texture();
            let st = analyse(
                CellSource::Texture {
                    tex: &tex,
                    origin_px: (wall.origin_px[0], wall.origin_px[1]),
                },
                wall.rows,
                wall.cols,
                wall.observed_mask().as_deref(),
                BAND_TAU,
            );
            let n = wall.rows * wall.cols;
            let same = (0..n)
                .filter(|&i| st.cls[i] == wall.structure.cls[i])
                .count();
            let agree = same as f64 / n as f64;
            if same == n {
                exact += 1;
            }

            // the colour every row's wall cells were painted, ours against theirs
            let mut d_max = 0.0f64;
            for r in 0..wall.rows {
                let ours = st
                    .bands
                    .iter()
                    .find(|b| b.r0 <= r && r <= b.r1)
                    .and_then(|b| b.rgb);
                let theirs = wall
                    .structure
                    .bands
                    .iter()
                    .find(|b| b.r0 <= r && r <= b.r1)
                    .and_then(|b| b.rgb);
                if let (Some(a), Some(b)) = (ours, theirs) {
                    let d =
                        imgops::oklab_distance(imgops::srgb_to_oklab(a), imgops::srgb_to_oklab(b));
                    d_max = d_max.max(d);
                }
            }
            // the rest of what the export writes: the painted colours, which
            // cells the completion added, the sky and eave counts and the
            // lattice the review sheets print
            for i in 0..n {
                let theirs = [
                    wall.structure.rgb[3 * i],
                    wall.structure.rgb[3 * i + 1],
                    wall.structure.rgb[3 * i + 2],
                ];
                assert_eq!(st.rgb[i], theirs, "{}: cell {i} colour", wall.key);
                assert_eq!(
                    st.added[i],
                    wall.structure.added[i] != 0,
                    "{}: cell {i} added",
                    wall.key
                );
            }
            assert!(!st.removed.iter().any(|v| *v), "nothing is ever removed");
            assert_eq!(
                st.sky_rows, wall.structure.sky_rows,
                "{}: sky rows",
                wall.key
            );
            assert_eq!(
                st.sky_cells, wall.structure.sky_cells,
                "{}: sky cells",
                wall.key
            );
            assert_eq!(
                st.door_rgb, wall.structure.door_rgb,
                "{}: door colour",
                wall.key
            );
            assert_eq!(
                st.bands.len(),
                wall.structure.bands.len(),
                "{}: band count",
                wall.key
            );
            for (ours, theirs) in st.bands.iter().zip(wall.structure.bands.iter()) {
                assert_eq!(
                    (ours.r0, ours.r1),
                    (theirs.r0, theirs.r1),
                    "{}: band rows",
                    wall.key
                );
                assert_eq!(ours.rgb, theirs.rgb, "{}: band colour", wall.key);
                assert_eq!(
                    ours.window_rgb, theirs.window_rgb,
                    "{}: window colour",
                    wall.key
                );
            }
            let lat = &wall.structure.lattice;
            assert_eq!(
                st.lattice.period, lat.period_m,
                "{}: lattice period",
                wall.key
            );
            assert_eq!(st.lattice.width, lat.width, "{}: lattice width", wall.key);
            assert_eq!(st.lattice.phase, lat.phase, "{}: lattice phase", wall.key);
            assert_eq!(
                st.lattice.accepted, lat.accepted,
                "{}: lattice accepted",
                wall.key
            );
            assert_eq!(
                st.lattice.row_period, lat.row_period_m,
                "{}: row period",
                wall.key
            );
            assert_eq!(
                st.lattice.model_columns(),
                lat.cols,
                "{}: lattice columns",
                wall.key
            );
            assert_eq!(
                st.lattice.window_columns(),
                lat.window_cols,
                "{}: window columns",
                wall.key
            );
            assert_eq!(
                st.lattice.floor_rows(),
                lat.rows,
                "{}: lattice rows",
                wall.key
            );
            // the fixture rounds the scores to three decimals
            assert!(
                (st.lattice.score - lat.score).abs() < 1e-3
                    && (st.lattice.on_mean - lat.on_mean).abs() < 1e-3
                    && (st.lattice.cover - lat.cover).abs() < 1e-3
                    && (st.lattice.row_score - lat.row_score).abs() < 1e-3,
                "{}: lattice scores",
                wall.key
            );

            // and the block each band colour lands on, which is what the game gets
            for (ours, theirs) in st.bands.iter().zip(wall.structure.bands.iter()) {
                if st.bands.len() != wall.structure.bands.len() {
                    break;
                }
                if let (Some(rgb), Some(name)) = (ours.rgb, theirs.block.as_deref()) {
                    assert_eq!(
                        block_for(rgb).name(),
                        name,
                        "{}: band {}..{} picks a different block",
                        wall.key,
                        ours.r0,
                        ours.r1
                    );
                }
            }

            lines.push(format!(
                "  {:<16} {} {:>3}x{:<3} cells {:6.2}%  dE {:.4}  bands {}/{}  sky {}/{}  \
                 +win {}/{}  lattice {:?}{}/{:?}{}",
                wall.key,
                wall.tier,
                wall.cols,
                wall.rows,
                100.0 * agree,
                d_max,
                st.bands.len(),
                wall.structure.bands.len(),
                st.sky_rows,
                wall.structure.sky_rows,
                st.added.iter().filter(|v| **v).count(),
                wall.structure.added.iter().filter(|v| **v != 0).count(),
                st.lattice.period,
                if st.lattice.accepted { "+" } else { "-" },
                wall.structure.lattice.period_m,
                if wall.structure.lattice.accepted {
                    "+"
                } else {
                    "-"
                },
            ));
            if agree < worst_cells.0 {
                worst_cells = (agree, wall.key.clone());
            }
            if d_max > worst_colour.0 {
                worst_colour = (d_max, wall.key.clone());
            }
        }
        println!("block product against Python, {} walls:", walls.len());
        for l in &lines {
            println!("{l}");
        }
        println!(
            "  {exact} of {} walls identical; worst grid {} at {:.2}%; worst band colour {} at {:.4} OkLab",
            walls.len(),
            worst_cells.1,
            100.0 * worst_cells.0,
            worst_colour.1,
            worst_colour.0
        );
        assert!(
            worst_cells.0 >= 0.97,
            "wall {} agrees on only {:.2}% of cells",
            worst_cells.1,
            100.0 * worst_cells.0
        );
        assert!(
            worst_colour.0 <= 0.03,
            "wall {} band colour is {:.4} OkLab away",
            worst_colour.1,
            worst_colour.0
        );
    }

    #[test]
    fn the_block_picker_is_the_generator_s_own() {
        // `Look.block_for` in bands.py and `facade_block_for_color` are the same
        // rule: nearest in OkLab over the facade-eligible palette with chroma
        // counted double, so two beiges a shade apart land on the same stone
        // rather than on pink terracotta against stone bricks. That they agree
        // is asserted on every band of all 45 fixture walls in the golden test
        // above; these two pin the delegation itself so a future refactor that
        // grows a second palette here fails loudly.
        assert_eq!(block_for([170, 165, 150]).name(), "smooth_stone");
        assert_eq!(block_for([210, 205, 195]).name(), "white_concrete");
    }
}
