//! Per-metre garden classes from the laser and photo measurements.
//!
//! Port of the garden lab's `classify.py` and `export.py`. Every rule works on
//! whole components rather than single cells, because a single cell is where the
//! noise lives: one laser hit on a garden chair, one sunlit flagstone.
//!
//! The rules were tuned on Bavaria's scans, about 20 points a square metre. Their
//! point counts scale down with the area's measured density ([`Counts`]), so a
//! scan of 5 or 10 points a square metre asks proportionally fewer hits of a
//! hedge; at Bavaria's density nothing changes.

use super::grid::{box_mean, component_median, dilate, label, median3_within, shapes};

pub const NONE: u8 = 0;
pub const LAWN: u8 = 1;
pub const PAVED: u8 = 2;
pub const HEDGE: u8 = 3;
pub const BUSH: u8 = 4;
pub const TREE: u8 = 5;
pub const SHED: u8 = 6;
pub const POOL: u8 = 7;
pub const BUILDING: u8 = 8;
pub const BED: u8 = 9;
pub const FENCE: u8 = 10;

/// Measurements over a rectangle of cells, row-major, row 0 north.
pub struct Inputs {
    pub w: usize,
    pub h: usize,
    pub n_all: Vec<u8>,
    pub n_gnd: Vec<u8>,
    pub n_obj: Vec<u8>,
    pub n_bld: Vec<u8>,
    /// Object points that look like plants (vegetation class, or not the last
    /// return of their pulse).
    pub n_veg: Vec<u8>,
    /// Whether `n_veg` counts beside the photo (see [`Source::laser_plants`]);
    /// otherwise it only stands in where there is no photo.
    ///
    /// [`Source::laser_plants`]: super::source::Source::laser_plants
    pub laser_plants: bool,
    /// Metres above the ground.
    pub h_obj: Vec<f32>,
    pub h_bld: Vec<f32>,
    pub i_gnd: Vec<f32>,
    pub photo: Vec<bool>,
    pub exg: Vec<f32>,
    /// Share of the cell's pixels that read as plants, 0..1.
    pub green_f: Vec<f32>,
    pub lum: Vec<f32>,
    pub blue: Vec<f32>,
    pub rgb: Vec<[u8; 3]>,
}

/// One classified cell: class, height of what stands on it in fifths of a metre
/// (buildings and trees included), paving colour.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Out {
    pub class: u8,
    pub height_q: u8,
    pub color: [u8; 3],
}

/// Point density the rules' counts were tuned at: Bavaria's median returns a
/// square metre over laser-covered cells sit above it.
pub const TUNED_DENSITY: f32 = 15.0;

/// The rules' point counts at the area's density.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counts {
    /// Object hits for a solid low object (hedge, bush, shed).
    pub obj: u8,
    /// Building hits for a roof.
    pub bld: u8,
    /// Object hits for a tree crown, a fence, and ground hits for an intensity.
    pub few: u8,
    /// Ground hits for a cell to calibrate on.
    pub flat: u8,
}

impl Counts {
    /// Scaled by `median_all / TUNED_DENSITY`, never above the tuned counts and
    /// never under one hit.
    pub fn at_density(median_all: f32) -> Self {
        let s = (median_all / TUNED_DENSITY).clamp(0.0, 1.0);
        let k = |n: f32| (n * s).round().max(1.0) as u8;
        Counts {
            obj: k(4.0),
            bld: k(3.0),
            few: k(2.0),
            flat: k(4.0),
        }
    }
}

/// The median of the returns a square metre over cells the laser reached.
pub fn median_density(inp: &Inputs) -> f32 {
    let mut counts: Vec<u8> = inp.n_all.iter().copied().filter(|&v| v > 0).collect();
    if counts.is_empty() {
        return 0.0;
    }
    let mid = counts.len() / 2;
    f32::from(*counts.select_nth_unstable(mid).1)
}

/// How the intensity thresholds were found, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Calibration {
    Photo,
    Laser,
    Defaults,
}

/// Thresholds on the ground-return intensity: from the photo where it can tell
/// lawn from paving, else from the split in the ground intensities themselves,
/// else the source's defaults.
pub fn calibrate(inp: &Inputs, defaults: (f32, f32)) -> ((f32, f32), Calibration) {
    let k = Counts::at_density(median_density(inp));
    let mut grass = Vec::new();
    let mut pave = Vec::new();
    let mut all = Vec::new();
    for c in 0..inp.w * inp.h {
        let flat = inp.n_obj[c] == 0 && inp.n_bld[c] == 0 && inp.n_gnd[c] >= k.flat;
        if !flat {
            continue;
        }
        all.push(inp.i_gnd[c]);
        if !inp.photo[c] {
            continue;
        }
        if inp.exg[c] > 0.08 && inp.lum[c] > 70.0 {
            grass.push(inp.i_gnd[c]);
        } else if inp.exg[c] < -0.02 && inp.lum[c] > 90.0 && inp.blue[c] < 0.02 {
            pave.push(inp.i_gnd[c]);
        }
    }
    if grass.len() >= 200 && pave.len() >= 200 {
        grass.sort_by(f32::total_cmp);
        pave.sort_by(f32::total_cmp);
        let at = |v: &[f32], q: f32| v[((v.len() - 1) as f32 * q) as usize];
        let (g, p) = (at(&grass, 0.5), at(&pave, 0.5));
        let gap = g - p;
        // Bavaria's 2022 scans: lawn about 1770, a usable gap 80 or more.
        if gap >= 0.045 * g {
            // Where the two overlap (scans whose strips differ in brightness),
            // the intensity alone only decides past where nearly all lawn, or
            // nearly all paving, lies; in between the photo does. Cleanly
            // separated scans keep the band at a fifth and three fifths.
            let lo = (p + 0.2 * gap).min(at(&grass, 0.1));
            let hi = (p + 0.6 * gap).max(at(&pave, 0.9));
            return ((lo, hi), Calibration::Photo);
        }
    }
    match split_intensity(&mut all) {
        Some(t) => ((t, t), Calibration::Laser),
        None => (defaults, Calibration::Defaults),
    }
}

/// Where flat ground's intensities split in two (Otsu's threshold over their
/// histogram): planted ground returns a near-infrared pulse brighter than
/// asphalt and most paving. `None` unless the split is clear: enough cells, each
/// side at least a tenth of them, the two sides well apart and most of the
/// spread between them rather than within.
pub fn split_intensity(vals: &mut [f32]) -> Option<f32> {
    if vals.len() < 500 {
        return None;
    }
    vals.sort_by(f32::total_cmp);
    let n = vals.len();
    let (lo, hi) = (vals[n / 100], vals[n - 1 - n / 100]);
    if hi <= lo {
        return None;
    }
    const BINS: usize = 128;
    let bin = |v: f32| (((v - lo) / (hi - lo) * BINS as f32) as usize).min(BINS - 1);
    let mut hist = [0f64; BINS];
    let mut sum = [0f64; BINS];
    for &v in vals.iter() {
        let v = v.clamp(lo, hi);
        hist[bin(v)] += 1.0;
        sum[bin(v)] += f64::from(v);
    }
    let total: f64 = hist.iter().sum();
    let total_sum: f64 = sum.iter().sum();
    let mean = total_sum / total;
    let var: f64 = vals
        .iter()
        .map(|&v| (f64::from(v.clamp(lo, hi)) - mean).powi(2))
        .sum::<f64>()
        / total;
    let (mut w0, mut s0) = (0.0, 0.0);
    let mut best = (0.0f64, 0usize, 0.0f64, 0.0f64, 0.0f64);
    for t in 0..BINS - 1 {
        w0 += hist[t];
        s0 += sum[t];
        let w1 = total - w0;
        if w0 == 0.0 || w1 == 0.0 {
            continue;
        }
        let (m0, m1) = (s0 / w0, (total_sum - s0) / w1);
        let between = w0 * w1 * (m1 - m0).powi(2) / (total * total);
        if between > best.0 {
            best = (between, t, w0 / total, m0, m1);
        }
    }
    let (between, t, share, m0, m1) = best;
    let median = f64::from(vals[n / 2]);
    let clear = var > 0.0
        && between / var >= 0.55
        && (0.1..=0.9).contains(&share)
        && m1 - m0 >= 0.25 * median;
    clear.then(|| lo + (t + 1) as f32 * (hi - lo) / BINS as f32)
}

pub fn classify(inp: &Inputs, (i_lo, i_hi): (f32, f32)) -> Vec<Out> {
    let (w, h) = (inp.w, inp.h);
    let n = w * h;
    let mut cls = vec![NONE; n];
    let mut hdm = vec![0.0f32; n];
    let median_all = median_density(inp);
    let need = Counts::at_density(median_all);

    // Plants: green in the photo, or where there is none, objects most of whose
    // hits look like plants to the laser, which counts beside the photo too
    // where the source trusts it.
    let veg = |c: usize| inp.n_obj[c] > 0 && 2 * u32::from(inp.n_veg[c]) >= u32::from(inp.n_obj[c]);
    let green: Vec<bool> = (0..n)
        .map(|c| {
            if inp.photo[c] {
                inp.green_f[c] >= 0.35 || inp.exg[c] > 0.04 || (inp.laser_plants && veg(c))
            } else {
                veg(c)
            }
        })
        .collect();
    // Photo and laser are years apart and the photo leans: a cell counts as green
    // when it or a neighbour is.
    let green_near = dilate(&green, w, h, 1);
    let building: Vec<bool> = inp.n_bld.iter().map(|&v| v >= need.bld).collect();
    let obj: Vec<bool> = (0..n)
        .map(|c| inp.n_obj[c] >= need.obj && inp.h_obj[c] >= 0.4 && !building[c])
        .collect();
    let hh = &inp.h_obj;

    // Ground cover everywhere the laser reached, smoothed by a 5x5 majority so a
    // terrace is one terrace and not a dither of flagstones and lawn.
    let planted: Vec<f32> = (0..n)
        .map(|c| {
            let has_i = inp.n_gnd[c] >= need.few;
            let p = if has_i && inp.i_gnd[c] >= i_hi {
                true
            } else if has_i && inp.i_gnd[c] < i_lo {
                false
            } else {
                green[c]
            };
            p as u8 as f32
        })
        .collect();
    let share = box_mean(&planted, w, h, 2);
    for c in 0..n {
        if inp.n_all[c] > 0 {
            cls[c] = if share[c] >= 0.5 { LAWN } else { PAVED };
        }
    }

    // Pools: clearly blue in the photo, and water swallows the laser, so a pool
    // also has far fewer returns than the ground around it. A blue car has neither.
    let blue: Vec<bool> = (0..n)
        .map(|c| inp.photo[c] && inp.blue[c] > 0.03 && inp.lum[c] > 80.0 && !obj[c] && !building[c])
        .collect();
    let (lab, k) = label(&blue, w, h, true);
    if k > 0 {
        let sh = shapes(&lab, k, w);
        let mut wet = vec![0.0f64; k as usize + 1];
        for c in 0..n {
            if lab[c] != 0 && f32::from(inp.n_all[c]) <= 0.6 * median_all {
                wet[lab[c] as usize] += 1.0;
            }
        }
        for c in 0..n {
            let l = lab[c] as usize;
            if l != 0 && (6.0..=120.0).contains(&sh[l].area) && wet[l] >= 0.4 * sh[l].area {
                cls[c] = POOL;
            }
        }
    }

    // Trees. Scans are often flown leaf-off, when a bare crown returns only a few
    // points a square metre, so two will do above 3 m, and gaps inside a crown are
    // closed. Each cell keeps its own height, so a crown slopes down to its edge
    // and trees lean short rather than tall.
    let tree_raw: Vec<bool> = (0..n)
        .map(|c| inp.n_obj[c] >= need.few && hh[c] >= 3.0 && green_near[c] && !building[c])
        .collect();
    let grown = dilate(&tree_raw, w, h, 1);
    let not_grown: Vec<bool> = grown.iter().map(|&b| !b).collect();
    let tree: Vec<bool> = dilate(&not_grown, w, h, 1)
        .into_iter()
        .enumerate()
        .map(|(c, outside)| !outside && !building[c])
        .collect();

    // Sheds and garages: building points, or solid objects that are not green.
    // A house's eaves and porch roofs are low building points too, so nothing
    // within two cells of a tall building counts.
    let tall: Vec<bool> = (0..n).map(|c| building[c] && inp.h_bld[c] > 5.0).collect();
    let near_house = dilate(&tall, w, h, 2);
    let shed_cand: Vec<bool> = (0..n)
        .map(|c| {
            ((building[c] && (1.8..=5.0).contains(&inp.h_bld[c]))
                || (obj[c] && !green_near[c] && (1.9..=4.5).contains(&hh[c])))
                && !near_house[c]
        })
        .collect();
    let (lab, k) = label(&shed_cand, w, h, false);
    let mut shed = vec![false; n];
    if k > 0 {
        let sh = shapes(&lab, k, w);
        for c in 0..n {
            let l = lab[c] as usize;
            if l != 0 && (5.0..=70.0).contains(&sh[l].area) && sh[l].width >= 1.8 {
                shed[c] = true;
                cls[c] = SHED;
                hdm[c] = if building[c] { inp.h_bld[c] } else { hh[c] } * 5.0;
            }
        }
    }

    // Low vegetation: long thin components are hedges, the rest bushes, and the
    // knee-high ones beds of flowers and perennials.
    let lowveg: Vec<bool> = (0..n)
        .map(|c| obj[c] && green_near[c] && hh[c] < 3.5 && !shed[c] && !tree[c])
        .collect();
    let (lab, k) = label(&lowveg, w, h, true);
    if k > 0 {
        let sh = shapes(&lab, k, w);
        for c in 0..n {
            let l = lab[c] as usize;
            if l == 0 || sh[l].area < 2.0 {
                continue;
            }
            let hedge = sh[l].length >= 5.0 && sh[l].width <= 2.2;
            cls[c] = if hedge {
                HEDGE
            } else if hh[c] < 0.8 {
                BED
            } else {
                BUSH
            };
            hdm[c] = hh[c] * 5.0;
        }
    }

    // Fences and garden walls: thin, straight, not green, knee to head high, with
    // planted ground beside them. Parked cars make short non-green rows too, but
    // they stand on paving and are wider.
    let fence_cand: Vec<bool> = (0..n)
        .map(|c| {
            inp.n_obj[c] >= need.few
                && (0.7..=2.3).contains(&hh[c])
                && !green_near[c]
                && !building[c]
                && !matches!(cls[c], SHED | HEDGE | BUSH | BED)
        })
        .collect();
    let (lab, k) = label(&fence_cand, w, h, true);
    if k > 0 {
        let sh = shapes(&lab, k, w);
        let ring = ring_planted_share(&lab, k, w, h, &planted);
        for c in 0..n {
            let l = lab[c] as usize;
            if l != 0 && sh[l].length >= 5.0 && sh[l].width <= 1.4 && ring[l] >= 0.35 {
                cls[c] = FENCE;
                hdm[c] = hh[c] * 5.0;
            }
        }
    }
    for c in 0..n {
        if tree[c] {
            cls[c] = TREE;
            hdm[c] = hh[c].max(3.0) * 5.0;
        }
        if building[c] && !shed[c] {
            cls[c] = BUILDING;
            hdm[c] = inp.h_bld[c] * 5.0;
        }
    }

    close_hedge_gaps(&mut cls, &mut hdm, w, h);
    smooth_heights(&cls, &mut hdm, w, h);
    let color = paving_colors(&cls, inp);
    (0..n)
        .map(|c| Out {
            class: cls[c],
            height_q: hdm[c].round().clamp(0.0, 255.0) as u8,
            color: color[c],
        })
        .collect()
}

/// For each component, the share of planted cells in the two-cell ring around it.
fn ring_planted_share(lab: &[u32], k: u32, w: usize, h: usize, planted: &[f32]) -> Vec<f64> {
    let mut hit = vec![0.0f64; k as usize + 1];
    let mut all = vec![0.0f64; k as usize + 1];
    let mut seen: Vec<u32> = vec![0; w * h];
    for z in 0..h {
        for x in 0..w {
            let l = lab[z * w + x];
            if l == 0 {
                continue;
            }
            for dz in -2isize..=2 {
                for dx in -2isize..=2 {
                    let (nx, nz) = (x as isize + dx, z as isize + dz);
                    if nx < 0 || nz < 0 || nx >= w as isize || nz >= h as isize {
                        continue;
                    }
                    let j = nz as usize * w + nx as usize;
                    if lab[j] == 0 && seen[j] != l {
                        seen[j] = l;
                        all[l as usize] += 1.0;
                        hit[l as usize] += f64::from(planted[j]);
                    }
                }
            }
        }
    }
    (0..=k as usize)
        .map(|l| if all[l] > 0.0 { hit[l] / all[l] } else { 0.0 })
        .collect()
}

/// Bridge gaps of one or two cells in a hedge, but only over planted ground: a
/// gap over paving is the gate or the drive and stays open.
fn close_hedge_gaps(cls: &mut [u8], hdm: &mut [f32], w: usize, h: usize) {
    let snapshot = cls.to_vec();
    let at = |x: isize, z: isize| -> Option<usize> {
        (x >= 0 && z >= 0 && x < w as isize && z < h as isize).then(|| z as usize * w + x as usize)
    };
    for z in 0..h as isize {
        for x in 0..w as isize {
            let c = at(x, z).unwrap();
            if !matches!(snapshot[c], LAWN | BED | BUSH) {
                continue;
            }
            'dirs: for (dx, dz) in [(1isize, 0isize), (0, 1), (1, 1), (1, -1)] {
                for gap in 1..=2isize {
                    // Hedge `gap` cells one way and within reach the other way.
                    for back in 1..=(3 - gap) {
                        let (a, b) = (
                            at(x - dx * back, z - dz * back),
                            at(x + dx * gap, z + dz * gap),
                        );
                        if let (Some(a), Some(b)) = (a, b) {
                            if snapshot[a] == HEDGE && snapshot[b] == HEDGE {
                                cls[c] = HEDGE;
                                hdm[c] = (hdm[a] + hdm[b]) / 2.0;
                                break 'dirs;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One height per hedge and per shed (their median), bushes smoothed within
/// themselves and never under 0.4 m.
fn smooth_heights(cls: &[u8], hdm: &mut [f32], w: usize, h: usize) {
    for class in [HEDGE, SHED] {
        let mask: Vec<bool> = cls.iter().map(|&c| c == class).collect();
        let (lab, k) = label(&mask, w, h, true);
        if k == 0 {
            continue;
        }
        let med = component_median(hdm, &lab, k);
        for (c, &l) in lab.iter().enumerate() {
            if l != 0 {
                hdm[c] = med[l as usize];
            }
        }
    }
    let bush: Vec<bool> = cls.iter().map(|&c| c == BUSH).collect();
    let smoothed = median3_within(hdm, &bush, w, h);
    for c in 0..cls.len() {
        if bush[c] {
            hdm[c] = smoothed[c].max(2.0);
        }
    }
}

/// One colour per paved patch, the median of its sunlit cells, so a driveway is
/// one material and a parked car or a shadow does not paint a patch of its own.
/// The photo runs warm: only a clearly red or brown patch keeps its tint (brick
/// pavers, red gravel), everything else is a grey of its own brightness.
fn paving_colors(cls: &[u8], inp: &Inputs) -> Vec<[u8; 3]> {
    let (w, h) = (inp.w, inp.h);
    let mut out = vec![[0u8; 3]; w * h];
    let mask: Vec<bool> = cls.iter().map(|&c| c == PAVED || c == SHED).collect();
    let (lab, k) = label(&mask, w, h, false);
    let mut lit: Vec<Vec<[u8; 3]>> = vec![Vec::new(); k as usize + 1];
    let mut any: Vec<Vec<[u8; 3]>> = vec![Vec::new(); k as usize + 1];
    for (c, &l) in lab.iter().enumerate() {
        let l = l as usize;
        if l == 0 || !inp.photo[c] {
            continue;
        }
        any[l].push(inp.rgb[c]);
        if inp.lum[c] > 70.0 {
            lit[l].push(inp.rgb[c]);
        }
    }
    let color: Vec<[u8; 3]> = (0..=k as usize)
        .map(|l| {
            let src = if lit[l].len() >= 3 { &lit[l] } else { &any[l] };
            if src.is_empty() {
                // No photo: a mid grey.
                return [150, 150, 150];
            }
            let med = |i: usize| {
                let mut v: Vec<u8> = src.iter().map(|p| p[i]).collect();
                v.sort_unstable();
                f32::from(v[v.len() / 2])
            };
            let (r, g, b) = (med(0), med(1), med(2));
            if r - b > 35.0 && r > g + 12.0 {
                [r as u8, g as u8, b as u8]
            } else {
                let m = ((r + g + b) / 3.0).round() as u8;
                [m, m, m]
            }
        })
        .collect();
    for c in 0..w * h {
        let l = lab[c] as usize;
        if l != 0 {
            out[c] = color[l];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank(w: usize, h: usize) -> Inputs {
        let n = w * h;
        Inputs {
            w,
            h,
            n_all: vec![18; n],
            n_gnd: vec![12; n],
            n_obj: vec![0; n],
            n_bld: vec![0; n],
            n_veg: vec![0; n],
            laser_plants: false,
            h_obj: vec![0.0; n],
            h_bld: vec![0.0; n],
            i_gnd: vec![1780.0; n],
            photo: vec![true; n],
            exg: vec![0.12; n],
            green_f: vec![0.9; n],
            lum: vec![110.0; n],
            blue: vec![-0.05; n],
            rgb: vec![[90, 130, 70]; n],
        }
    }

    #[test]
    fn a_lawn_with_a_hedge_and_a_drive() {
        let (w, h) = (30, 20);
        let mut inp = blank(w, h);
        // A hedge along row 5, 1.4 m tall, with a paved drive cutting it at x 12..15.
        for x in 2..28 {
            let c = 5 * w + x;
            inp.n_obj[c] = 10;
            inp.h_obj[c] = 1.4;
        }
        for z in 0..h {
            for x in 12..15 {
                let c = z * w + x;
                inp.i_gnd[c] = 1400.0;
                inp.exg[c] = -0.05;
                inp.green_f[c] = 0.0;
                inp.lum[c] = 150.0;
                inp.rgb[c] = [150, 150, 150];
                inp.n_obj[c] = 0;
                inp.h_obj[c] = 0.0;
            }
        }
        let out = classify(&inp, (1450.0, 1600.0));
        assert_eq!(out[5 * w + 5].class, HEDGE);
        assert_eq!(out[5 * w + 5].height_q, 7);
        assert_eq!(out[10 * w + 5].class, LAWN);
        assert_eq!(out[10 * w + 13].class, PAVED);
        // The drive keeps its opening.
        assert_eq!(out[5 * w + 13].class, PAVED);
    }

    #[test]
    fn a_blue_car_is_not_a_pool() {
        let (w, h) = (20, 20);
        let mut inp = blank(w, h);
        for z in 8..11 {
            for x in 8..12 {
                let c = z * w + x;
                inp.blue[c] = 0.06;
                inp.exg[c] = -0.05;
                inp.green_f[c] = 0.0;
            }
        }
        let out = classify(&inp, (1450.0, 1600.0));
        assert!(out.iter().all(|o| o.class != POOL));
        // The same blue with the laser swallowed is a pool.
        for z in 8..11 {
            for x in 8..12 {
                inp.n_all[z * w + x] = 6;
            }
        }
        let out = classify(&inp, (1450.0, 1600.0));
        assert_eq!(out[9 * w + 9].class, POOL);
    }

    #[test]
    fn eaves_are_not_sheds_but_a_garden_house_is() {
        let (w, h) = (40, 30);
        let mut inp = blank(w, h);
        let mut put = |x0: usize, x1: usize, z0: usize, z1: usize, hb: f32| {
            for z in z0..z1 {
                for x in x0..x1 {
                    let c = z * w + x;
                    inp.n_bld[c] = 10;
                    inp.h_bld[c] = hb;
                }
            }
        };
        put(5, 15, 5, 15, 8.0); // a house
        put(15, 17, 5, 15, 3.0); // its eaves, low building points along a wall
        put(28, 31, 20, 23, 2.4); // a garden house on its own
        let out = classify(&inp, (1450.0, 1600.0));
        assert_ne!(out[10 * w + 16].class, SHED);
        assert_eq!(out[21 * w + 29].class, SHED);
        assert_eq!(out[21 * w + 29].height_q, 12);
    }

    #[test]
    fn calibration_finds_the_midway_band() {
        let (w, h) = (40, 40);
        let mut inp = blank(w, h);
        for c in 0..w * h / 2 {
            inp.i_gnd[c] = 1400.0;
            inp.exg[c] = -0.05;
            inp.lum[c] = 150.0;
        }
        let ((lo, hi), how) = calibrate(&inp, (0.0, 0.0));
        assert_eq!(how, Calibration::Photo);
        assert!(
            (lo - 1476.0).abs() < 1.0 && (hi - 1628.0).abs() < 1.0,
            "{lo} {hi}"
        );
    }

    #[test]
    fn counts_scale_with_density_and_not_past_the_tuning() {
        let tuned = Counts {
            obj: 4,
            bld: 3,
            few: 2,
            flat: 4,
        };
        assert_eq!(Counts::at_density(18.0), tuned);
        assert_eq!(Counts::at_density(40.0), tuned);
        assert_eq!(
            Counts::at_density(7.5),
            Counts {
                obj: 2,
                bld: 2,
                few: 1,
                flat: 2
            }
        );
        let sparse = Counts::at_density(2.0);
        assert_eq!((sparse.obj, sparse.bld, sparse.few), (1, 1, 1));
    }

    #[test]
    fn a_sparse_scan_still_finds_the_hedge() {
        // The hedge test at a quarter of the density: a hedge cell has two hits.
        let (w, h) = (30, 20);
        let mut inp = blank(w, h);
        for c in 0..w * h {
            inp.n_all[c] = 4;
            inp.n_gnd[c] = 3;
        }
        for x in 2..28 {
            let c = 5 * w + x;
            inp.n_obj[c] = 2;
            inp.h_obj[c] = 1.4;
        }
        let out = classify(&inp, (1450.0, 1600.0));
        assert_eq!(out[5 * w + 5].class, HEDGE);
        // At full density two hits are too few for a hedge.
        inp.n_all.fill(18);
        let out = classify(&inp, (1450.0, 1600.0));
        assert_ne!(out[5 * w + 5].class, HEDGE);
    }

    #[test]
    fn laser_alone_splits_lawn_from_asphalt() {
        // Two clear groups: asphalt about 400, lawn about 1300.
        let mut v: Vec<f32> = (0..3000)
            .map(|i| {
                if i % 3 == 0 {
                    380.0 + (i % 50) as f32
                } else {
                    1250.0 + (i % 100) as f32
                }
            })
            .collect();
        let t = split_intensity(&mut v).expect("a clear split");
        assert!((430.0..1250.0).contains(&t), "{t}");
        // One group with noise has no split.
        let mut one: Vec<f32> = (0..3000).map(|i| 1000.0 + (i % 97) as f32).collect();
        assert_eq!(split_intensity(&mut one), None);
    }

    #[test]
    fn without_a_photo_the_returns_tell_a_hedge_from_a_fence() {
        let (w, h) = (30, 20);
        let mut inp = blank(w, h);
        inp.photo = vec![false; w * h];
        // A hedge along row 5 (most hits not the last of their pulse) and a
        // fence along row 14 (solid).
        for x in 2..28 {
            let c = 5 * w + x;
            inp.n_obj[c] = 10;
            inp.n_veg[c] = 7;
            inp.h_obj[c] = 1.4;
            let c = 14 * w + x;
            inp.n_obj[c] = 6;
            inp.n_veg[c] = 0;
            inp.h_obj[c] = 1.6;
        }
        let out = classify(&inp, (1450.0, 1600.0));
        assert_eq!(out[5 * w + 5].class, HEDGE);
        assert_eq!(out[14 * w + 5].class, FENCE);
    }
}
