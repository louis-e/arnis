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

use super::grid::{box_share, component_median, dilate, label, median3_within, shapes};
use super::photo::PhotoCell;

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
/// Open ground neither planted nor sealed: bare earth, sand, gravel, a dry or
/// shaded patch. Left to the land cover.
pub const BARE: u8 = 11;

/// Measurements over a rectangle of cells, row-major, row 0 north, kept as the
/// tile caches quantise them (17 bytes a cell). The accessors give the values
/// the rules compare: metres, and the photo's measures on their own scales.
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
    /// Whether a pool needs cells the laser returned nothing from.
    pub strict_pools: bool,
    /// Whether paving returns a brighter ground intensity than lawn (see
    /// [`Source::paving_brighter`]): [`Inputs::i_gnd`] then reads it turned
    /// over, and paving is kept near buildings only.
    ///
    /// [`Source::paving_brighter`]: super::source::Source::paving_brighter
    pub paving_brighter: bool,
    /// Heights above the ground, in fifths of a metre.
    pub h_obj_q: Vec<u8>,
    pub h_bld_q: Vec<u8>,
    /// Median intensity of the ground returns.
    pub i_gnd_q: Vec<u16>,
    /// The photo's measures; `present` is false where there is no photo.
    pub photo: Vec<PhotoCell>,
}

impl Inputs {
    /// Nothing measured yet, for a source with neither flag set.
    pub fn new(w: usize, h: usize) -> Self {
        let n = w * h;
        Inputs {
            w,
            h,
            n_all: vec![0; n],
            n_gnd: vec![0; n],
            n_obj: vec![0; n],
            n_bld: vec![0; n],
            n_veg: vec![0; n],
            laser_plants: false,
            strict_pools: false,
            paving_brighter: false,
            h_obj_q: vec![0; n],
            h_bld_q: vec![0; n],
            i_gnd_q: vec![0; n],
            photo: vec![PhotoCell::default(); n],
        }
    }

    /// Metres above the ground.
    #[inline]
    pub fn h_obj(&self, c: usize) -> f32 {
        f32::from(self.h_obj_q[c]) / 5.0
    }

    #[inline]
    pub fn h_bld(&self, c: usize) -> f32 {
        f32::from(self.h_bld_q[c]) / 5.0
    }

    /// The ground returns' median intensity, turned over where paving reads
    /// the brighter: planted ground reads the brighter either way.
    #[inline]
    pub fn i_gnd(&self, c: usize) -> f32 {
        let i = f32::from(self.i_gnd_q[c]);
        if self.paving_brighter {
            TURN - i
        } else {
            i
        }
    }

    #[inline]
    pub fn has_photo(&self, c: usize) -> bool {
        self.photo[c].present
    }

    #[inline]
    pub fn exg(&self, c: usize) -> f32 {
        f32::from(self.photo[c].exg) / 200.0
    }

    /// Share of the cell's pixels that read as plants, 0..1.
    #[inline]
    pub fn green_f(&self, c: usize) -> f32 {
        f32::from(self.photo[c].green) / 25.0
    }

    #[inline]
    pub fn lum(&self, c: usize) -> f32 {
        f32::from(self.photo[c].lum)
    }

    #[inline]
    pub fn blue(&self, c: usize) -> f32 {
        f32::from(self.photo[c].blue) / 200.0
    }

    #[inline]
    pub fn rgb(&self, c: usize) -> [u8; 3] {
        self.photo[c].rgb
    }
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
    let mut hist = [0usize; 256];
    for &v in &inp.n_all {
        hist[usize::from(v)] += 1;
    }
    let reached: usize = hist[1..].iter().sum();
    if reached == 0 {
        return 0.0;
    }
    // The value at index `reached / 2` of the sorted counts.
    let mid = reached / 2;
    let mut below = 0;
    for (v, &k) in hist.iter().enumerate().skip(1) {
        below += k;
        if below > mid {
            return v as f32;
        }
    }
    255.0
}

/// How the intensity thresholds were found, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Calibration {
    Photo,
    Laser,
    Defaults,
}

/// Thresholds on the ground-return intensity as [`Inputs::i_gnd`] reads it:
/// from the photo where it can tell lawn from paving, else from the split in the
/// ground intensities themselves, else the source's defaults.
pub fn calibrate(inp: &Inputs, defaults: (f32, f32)) -> ((f32, f32), Calibration) {
    let k = Counts::at_density(median_density(inp));
    let mut grass = Vec::new();
    let mut pave = Vec::new();
    // As measured, not turned over: see the split below.
    let mut all = Vec::new();
    for c in 0..inp.w * inp.h {
        let flat = inp.n_obj[c] == 0 && inp.n_bld[c] == 0 && inp.n_gnd[c] >= k.flat;
        if !flat {
            continue;
        }
        all.push(f32::from(inp.i_gnd_q[c]));
        if !inp.has_photo(c) {
            continue;
        }
        if inp.exg(c) > 0.08 && inp.lum(c) > 70.0 {
            grass.push(inp.i_gnd(c));
        } else if inp.exg(c) < -0.02 && inp.lum(c) > 90.0 && inp.blue(c) < 0.02 {
            pave.push(inp.i_gnd(c));
        }
    }
    if grass.len() >= 200 && pave.len() >= 200 {
        grass.sort_by(f32::total_cmp);
        pave.sort_by(f32::total_cmp);
        // The gap is held to the brighter one's median as measured: lawn's,
        // or where paving reads the brighter, paving's, which sits at the
        // same place counted from the top of the intensities turned over.
        let scale = if inp.paving_brighter {
            TURN - pave[pave.len() / 2]
        } else {
            grass[(grass.len() - 1) / 2]
        };
        if let Some(t) = bands(&grass, &pave, scale) {
            return (t, Calibration::Photo);
        }
    }
    // The split is found on the intensities as measured, whose median it
    // asks the two sides to lie well apart against, and its threshold turned
    // over after; so are the defaults (see `Source::intensity_defaults`).
    let turn = |t: f32| if inp.paving_brighter { TURN - t } else { t };
    match split_intensity(&mut all) {
        Some(t) => ((turn(t), turn(t)), Calibration::Laser),
        None if inp.paving_brighter => {
            ((turn(defaults.1), turn(defaults.0)), Calibration::Defaults)
        }
        None => (defaults, Calibration::Defaults),
    }
}

/// Share of the ring around a shed or a pool that must have returned ground.
const MIN_GROUND_AROUND: f64 = 0.4;

/// Whether a photo colour is earth's or sand's: warm, but not a brick's red.
fn earthy([r, g, b]: [u8; 3]) -> bool {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    r - b >= 12 && r - g <= 25
}

/// What a turned-over intensity is subtracted from.
const TURN: f32 = 65_535.0;

/// How far from a building, in cells, paving is kept where it reads the
/// brighter.
const PAVING_REACH: usize = 30;

/// The intensity bands under which a cell is paving and over which it is
/// planted, from the sorted intensities of flat cells the photo shows as
/// lawn and as paving; `None` unless lawn reads the brighter by a clear part
/// of `scale`, the brighter one's median intensity.
fn bands(grass: &[f32], pave: &[f32], scale: f32) -> Option<(f32, f32)> {
    let at = |v: &[f32], q: f32| v[((v.len() - 1) as f32 * q) as usize];
    let (g, p) = (at(grass, 0.5), at(pave, 0.5));
    let gap = g - p;
    // Bavaria's 2022 scans: lawn about 1770, a usable gap 80 or more.
    if gap < 0.045 * scale {
        return None;
    }
    // Where the two overlap (scans whose strips differ in brightness), the
    // intensity alone only decides past where nearly all lawn, or nearly all
    // paving, lies; in between the photo does. Cleanly separated scans keep
    // the band at a fifth and three fifths.
    let lo = (p + 0.2 * gap).min(at(grass, 0.1));
    let hi = (p + 0.6 * gap).max(at(pave, 0.9));
    Some((lo, hi))
}

/// Where flat ground's intensities split in two (Otsu's threshold over their
/// histogram): planted ground returns a near-infrared pulse brighter than
/// asphalt and most paving, or in a few scans darker. `None` unless the split
/// is clear: enough cells, each side at least a tenth of them, the two sides
/// well apart and most of the spread between them rather than within.
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

/// Building points the photo sees green are crowns: for sources whose roofs
/// may be guessed from the pulse (see `laser::find_roofs`), where an evergreen
/// crown returns as few pulses through as a roof, such cells count their
/// building points as objects again.
pub fn crowns_not_roofs(inp: &mut Inputs) {
    for c in 0..inp.w * inp.h {
        let green = inp.has_photo(c) && (inp.green_f(c) >= 0.35 || inp.exg(c) > 0.04);
        if green && inp.n_bld[c] > 0 {
            inp.n_obj[c] = inp.n_obj[c].saturating_add(inp.n_bld[c]);
            // The higher of two heights is the higher in fifths of a metre.
            inp.h_obj_q[c] = inp.h_obj_q[c].max(inp.h_bld_q[c]);
            inp.n_bld[c] = 0;
            inp.h_bld_q[c] = 0;
        }
    }
}

pub fn classify(inp: &Inputs, (i_lo, i_hi): (f32, f32)) -> Vec<Out> {
    let (w, h) = (inp.w, inp.h);
    let n = w * h;
    let mut cls = vec![NONE; n];
    let mut hdm = vec![0.0f32; n];
    let median_all = median_density(inp);
    let need = Counts::at_density(median_all);
    // Each mask below is dropped after its last use: on a town's few square
    // kilometres they are megabytes apiece.

    // Plants: green in the photo, or where there is none, objects most of whose
    // hits look like plants to the laser, which counts beside the photo too
    // where the source trusts it.
    let veg = |c: usize| inp.n_obj[c] > 0 && 2 * u32::from(inp.n_veg[c]) >= u32::from(inp.n_obj[c]);
    let green_at = |c: usize| {
        if inp.has_photo(c) {
            inp.green_f(c) >= 0.35 || inp.exg(c) > 0.04 || (inp.laser_plants && veg(c))
        } else {
            veg(c)
        }
    };
    let green: Vec<bool> = (0..n).map(green_at).collect();
    // Photo and laser are years apart and the photo leans: a cell counts as green
    // when it or a neighbour is.
    let green_near = dilate(&green, w, h, 1);
    let building: Vec<bool> = inp.n_bld.iter().map(|&v| v >= need.bld).collect();
    let obj: Vec<bool> = (0..n)
        .map(|c| inp.n_obj[c] >= need.obj && inp.h_obj(c) >= 0.4 && !building[c])
        .collect();

    // Ground cover everywhere the laser reached, smoothed by a 5x5 majority so a
    // terrace is one terrace and not a dither of flagstones and lawn.
    let planted: Vec<bool> = (0..n)
        .map(|c| {
            let has_i = inp.n_gnd[c] >= need.few;
            if has_i && inp.i_gnd(c) >= i_hi {
                true
            } else if has_i && inp.i_gnd(c) < i_lo {
                false
            } else {
                green[c]
            }
        })
        .collect();
    // Between the two bands the photo decides, and not green there is not yet
    // sealed: bare earth, sand and dry ground read so too, while paving mostly
    // reads plainly below the band. Earth-coloured ground there is open ground
    // where it outnumbers the sealed cells around it; grey stays paving.
    let unsure: Vec<bool> = (0..n)
        .map(|c| {
            let i = inp.i_gnd(c);
            inp.n_gnd[c] >= need.few
                && inp.has_photo(c)
                && (i_lo..i_hi).contains(&i)
                && !green[c]
                && earthy(inp.rgb(c))
        })
        .collect();
    drop(green);
    // The planted share in fortieths, kept for the open ground below.
    let mut planted_share = vec![0u8; n];
    box_share(&planted, w, h, 2, |c, share| {
        planted_share[c] = (share * 40.0).round() as u8;
        if inp.n_all[c] > 0 {
            cls[c] = if share >= 0.5 { LAWN } else { PAVED };
        }
    });
    box_share(&unsure, w, h, 2, |c, share| {
        let sealed = 1.0 - f32::from(planted_share[c]) / 40.0 - share;
        if cls[c] == PAVED && share > sealed {
            cls[c] = BARE;
        }
    });
    drop(unsure);
    drop(planted_share);
    // Where paving reads brighter than lawn, bare ground and trodden meadow
    // read as bright: paving is kept only within reach of a building, where
    // yards and drives are, and open ground farther out stays lawn. Streets
    // and car parks are mapped and win anyway.
    if inp.paving_brighter {
        box_share(&building, w, h, PAVING_REACH, |c, share| {
            if share <= 0.0 && cls[c] == PAVED {
                cls[c] = LAWN;
            }
        });
    }

    // Cells with ground returns: what a shed or a pool stands among.
    let ground_hit: Vec<bool> = inp.n_gnd.iter().map(|&v| v >= need.few).collect();

    // Pools: clearly blue in the photo, and water swallows the laser, so a pool
    // also has far fewer returns than the ground around it. A blue car has neither.
    // Where the source says so (see [`Source::strict_pools`]) only a cell
    // with no return at all counts as water.
    //
    // [`Source::strict_pools`]: super::source::Source::strict_pools
    {
        let blue: Vec<bool> = (0..n)
            .map(|c| {
                inp.has_photo(c)
                    && inp.blue(c) > 0.03
                    && inp.lum(c) > 80.0
                    && !obj[c]
                    && !building[c]
            })
            .collect();
        let dry_from = if inp.strict_pools {
            0.0
        } else {
            0.6 * median_all
        };
        let (lab, k) = label(&blue, w, h, true);
        drop(blue);
        if k > 0 {
            let sh = shapes(&lab, k, w);
            // A pool stands in a garden the laser reached: blue on open water, or
            // where a laser tile is missing, has no ground around it.
            let grounded = ring_planted_share(&lab, k, w, h, &ground_hit);
            let mut wet = vec![0.0f64; k as usize + 1];
            for c in 0..n {
                if lab[c] != 0 && f32::from(inp.n_all[c]) <= dry_from {
                    wet[lab[c] as usize] += 1.0;
                }
            }
            for c in 0..n {
                let l = lab[c] as usize;
                if l != 0
                    && (6.0..=120.0).contains(&sh[l].area)
                    && wet[l] >= 0.4 * sh[l].area
                    && grounded[l] >= MIN_GROUND_AROUND
                {
                    cls[c] = POOL;
                }
            }
        }
    }

    // Trees. Scans are often flown leaf-off, when a bare crown returns only a few
    // points a square metre, so two will do above 3 m, and gaps inside a crown are
    // closed. Each cell keeps its own height, so a crown slopes down to its edge
    // and trees lean short rather than tall.
    let tree: Vec<bool> = {
        let tree_raw: Vec<bool> = (0..n)
            .map(|c| {
                inp.n_obj[c] >= need.few && inp.h_obj(c) >= 3.0 && green_near[c] && !building[c]
            })
            .collect();
        let mut not_grown = dilate(&tree_raw, w, h, 1);
        drop(tree_raw);
        not_grown.iter_mut().for_each(|b| *b = !*b);
        dilate(&not_grown, w, h, 1)
            .into_iter()
            .enumerate()
            .map(|(c, outside)| !outside && !building[c])
            .collect()
    };

    // Sheds and garages: building points, or solid objects that are not green.
    // A house's eaves and porch roofs are low building points too, so nothing
    // within two cells of a tall building counts.
    let shed: Vec<bool> = {
        let tall: Vec<bool> = (0..n).map(|c| building[c] && inp.h_bld(c) > 5.0).collect();
        let near_house = dilate(&tall, w, h, 2);
        drop(tall);
        let shed_cand: Vec<bool> = (0..n)
            .map(|c| {
                ((building[c] && (1.8..=5.0).contains(&inp.h_bld(c)))
                    || (obj[c] && !green_near[c] && (1.9..=4.5).contains(&inp.h_obj(c))))
                    && !near_house[c]
            })
            .collect();
        drop(near_house);
        let (lab, k) = label(&shed_cand, w, h, false);
        drop(shed_cand);
        let mut shed = vec![false; n];
        if k > 0 {
            let sh = shapes(&lab, k, w);
            // A shed stands on ground: a boat, a weir or a pontoon has water
            // around it, which returns the laser nothing.
            let grounded = ring_planted_share(&lab, k, w, h, &ground_hit);
            for c in 0..n {
                let l = lab[c] as usize;
                if l != 0
                    && (5.0..=70.0).contains(&sh[l].area)
                    && sh[l].width >= 1.8
                    && grounded[l] >= MIN_GROUND_AROUND
                {
                    shed[c] = true;
                    cls[c] = SHED;
                    hdm[c] = if building[c] {
                        inp.h_bld(c)
                    } else {
                        inp.h_obj(c)
                    } * 5.0;
                }
            }
        }
        shed
    };
    drop(ground_hit);

    // Low vegetation: long thin components are hedges, the rest bushes, and the
    // knee-high ones beds of flowers and perennials.
    {
        let lowveg: Vec<bool> = (0..n)
            .map(|c| obj[c] && green_near[c] && inp.h_obj(c) < 3.5 && !shed[c] && !tree[c])
            .collect();
        let (lab, k) = label(&lowveg, w, h, true);
        drop(lowveg);
        if k > 0 {
            let sh = shapes(&lab, k, w);
            // A car parked beside a lawn borrows the lawn's green at its edge
            // and reads as a hedge or bush: neither green itself nor coloured
            // (grey, white, black or metal in the photo), knee high or more,
            // and standing mostly on paving. Beds, and plants in a lawn, are
            // not cars whatever their colour.
            let ring = ring_planted_share(&lab, k, w, h, &planted);
            let k = k as usize;
            let mut cells = vec![0u32; k + 1];
            let mut greens = vec![0u32; k + 1];
            let mut neutral = vec![0u32; k + 1];
            let mut tall = vec![0u32; k + 1];
            for (c, &l) in lab.iter().enumerate() {
                let l = l as usize;
                if l == 0 {
                    continue;
                }
                cells[l] += 1;
                greens[l] += u32::from(green_at(c));
                let [r, g, b] = inp.rgb(c);
                let spread = r.max(g).max(b) - r.min(g).min(b);
                neutral[l] += u32::from(inp.has_photo(c) && spread <= 24);
                tall[l] += u32::from(inp.h_obj(c) >= 1.0);
            }
            let parked = |l: usize| {
                4 * greens[l] < cells[l]
                    && 5 * neutral[l] >= 3 * cells[l]
                    && 2 * tall[l] >= cells[l]
                    && ring[l] < 0.5
            };
            for c in 0..n {
                let l = lab[c] as usize;
                if l == 0 || sh[l].area < 2.0 || parked(l) {
                    continue;
                }
                let hedge = sh[l].length >= 5.0 && sh[l].width <= 2.2;
                let hh = inp.h_obj(c);
                cls[c] = if hedge {
                    HEDGE
                } else if hh < 0.8 {
                    BED
                } else {
                    BUSH
                };
                hdm[c] = hh * 5.0;
            }
        }
    }
    drop(obj);

    // Fences and garden walls: thin, straight, not green, knee to head high, with
    // planted ground beside them. Parked cars make short non-green rows too, but
    // they stand on paving and are wider.
    {
        let fence_cand: Vec<bool> = (0..n)
            .map(|c| {
                inp.n_obj[c] >= need.few
                    && (0.7..=2.3).contains(&inp.h_obj(c))
                    && !green_near[c]
                    && !building[c]
                    && !matches!(cls[c], SHED | HEDGE | BUSH | BED)
            })
            .collect();
        let (lab, k) = label(&fence_cand, w, h, true);
        drop(fence_cand);
        if k > 0 {
            let sh = shapes(&lab, k, w);
            let ring = ring_planted_share(&lab, k, w, h, &planted);
            for c in 0..n {
                let l = lab[c] as usize;
                if l != 0 && sh[l].length >= 5.0 && sh[l].width <= 1.4 && ring[l] >= 0.35 {
                    cls[c] = FENCE;
                    hdm[c] = inp.h_obj(c) * 5.0;
                }
            }
        }
    }
    drop(planted);
    drop(green_near);
    for c in 0..n {
        if tree[c] {
            cls[c] = TREE;
            hdm[c] = inp.h_obj(c).max(3.0) * 5.0;
        }
        if building[c] && !shed[c] {
            cls[c] = BUILDING;
            hdm[c] = inp.h_bld(c) * 5.0;
        }
    }
    drop(tree);
    drop(shed);
    drop(building);

    close_hedge_gaps(&mut cls, &mut hdm, w, h);
    smooth_heights(&cls, &mut hdm, w, h);
    let mut out: Vec<Out> = cls
        .iter()
        .zip(&hdm)
        .map(|(&class, &v)| Out {
            class,
            height_q: v.round().clamp(0.0, 255.0) as u8,
            color: [0; 3],
        })
        .collect();
    drop(cls);
    drop(hdm);
    paving_colors(&mut out, inp);
    out
}

/// For each component, the share of planted cells in the two-cell ring around it.
fn ring_planted_share(lab: &[u32], k: u32, w: usize, h: usize, planted: &[bool]) -> Vec<f64> {
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
                        hit[l as usize] += f64::from(u8::from(planted[j]));
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
    // Medians of the heights as they stand, written back once all are taken.
    let smoothed = median3_within(hdm, &bush, w, h);
    for (c, v) in (0..cls.len()).filter(|&c| bush[c]).zip(smoothed) {
        hdm[c] = v.max(2.0);
    }
}

/// One colour per paved patch, the median of its sunlit cells, so a driveway is
/// one material and a parked car or a shadow does not paint a patch of its own.
/// The photo runs warm: only a clearly red or brown patch keeps its tint (brick
/// pavers, red gravel), everything else is a grey of its own brightness.
fn paving_colors(out: &mut [Out], inp: &Inputs) {
    let (w, h) = (inp.w, inp.h);
    let mask: Vec<bool> = out
        .iter()
        .map(|o| o.class == PAVED || o.class == SHED)
        .collect();
    let (lab, k) = label(&mask, w, h, false);
    drop(mask);
    let mut lit: Vec<Vec<[u8; 3]>> = vec![Vec::new(); k as usize + 1];
    let mut any: Vec<Vec<[u8; 3]>> = vec![Vec::new(); k as usize + 1];
    for (c, &l) in lab.iter().enumerate() {
        let l = l as usize;
        if l == 0 || !inp.has_photo(c) {
            continue;
        }
        any[l].push(inp.rgb(c));
        if inp.lum(c) > 70.0 {
            lit[l].push(inp.rgb(c));
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
    for (o, &l) in out.iter_mut().zip(&lab) {
        if l != 0 {
            o.color = color[l as usize];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A photo cell on the caches' scales: excess green and blueness times 200,
    /// green pixels out of 25.
    fn photo(exg: i8, green: u8, lum: u8, blue: i8, rgb: [u8; 3]) -> PhotoCell {
        PhotoCell {
            exg,
            green,
            lum,
            blue,
            rgb,
            present: true,
        }
    }

    /// A lawn: excess green 0.12, nine in ten pixels green, lit, not blue.
    fn blank(w: usize, h: usize) -> Inputs {
        let n = w * h;
        let mut inp = Inputs::new(w, h);
        inp.n_all = vec![18; n];
        inp.n_gnd = vec![12; n];
        inp.i_gnd_q = vec![1780; n];
        inp.photo = vec![photo(24, 23, 110, -10, [90, 130, 70]); n];
        inp
    }

    #[test]
    fn density_is_the_middle_count_of_the_cells_reached() {
        let mut inp = blank(8, 1);
        inp.n_all = vec![0, 3, 3, 7, 1, 9, 0, 2];
        // Reached: 1 2 3 3 7 9; index 3 of them.
        assert_eq!(median_density(&inp), 3.0);
        inp.n_all = vec![0; 8];
        assert_eq!(median_density(&inp), 0.0);
        inp.n_all = vec![255; 8];
        assert_eq!(median_density(&inp), 255.0);
    }

    #[test]
    fn accessors_read_the_cached_scales() {
        let mut inp = blank(1, 1);
        inp.h_obj_q[0] = 7;
        assert_eq!(inp.h_obj(0), 1.4);
        assert_eq!(inp.exg(0), 0.12);
        assert_eq!(inp.blue(0), -0.05);
        assert_eq!(inp.green_f(0), 0.92);
        assert_eq!(inp.i_gnd(0), 1780.0);
    }

    #[test]
    fn a_lawn_with_a_hedge_and_a_drive() {
        let (w, h) = (30, 20);
        let mut inp = blank(w, h);
        // A hedge along row 5, 1.4 m tall, with a paved drive cutting it at x 12..15.
        for x in 2..28 {
            let c = 5 * w + x;
            inp.n_obj[c] = 10;
            inp.h_obj_q[c] = 7;
        }
        for z in 0..h {
            for x in 12..15 {
                let c = z * w + x;
                inp.i_gnd_q[c] = 1400;
                inp.photo[c] = photo(-10, 0, 150, -10, [150, 150, 150]);
                inp.n_obj[c] = 0;
                inp.h_obj_q[c] = 0;
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
    fn a_car_parked_beside_a_lawn_is_no_hedge() {
        let (w, h) = (24, 16);
        let mut inp = blank(w, h);
        // A drive at x 10..18 beside the lawn, a silver car on it along the
        // lawn's edge, 1.6 m high.
        for z in 0..h {
            for x in 10..18 {
                let c = z * w + x;
                inp.i_gnd_q[c] = 1400;
                inp.photo[c] = photo(-10, 0, 150, -10, [150, 150, 150]);
            }
        }
        for z in 5..11 {
            for x in 10..12 {
                let c = z * w + x;
                inp.n_obj[c] = 10;
                inp.h_obj_q[c] = 8;
                inp.photo[c] = photo(-4, 0, 185, -2, [184, 186, 190]);
            }
        }
        let out = classify(&inp, (1450.0, 1600.0));
        for z in 5..11 {
            assert!(!matches!(out[z * w + 10].class, HEDGE | BUSH), "{z}");
        }
        // The same row green in the photo is a hedge.
        for z in 5..11 {
            for x in 10..12 {
                inp.photo[z * w + x] = photo(24, 23, 110, -10, [80, 120, 60]);
            }
        }
        let out = classify(&inp, (1450.0, 1600.0));
        assert_eq!(out[7 * w + 10].class, HEDGE);
    }

    #[test]
    fn a_blue_car_is_not_a_pool() {
        let (w, h) = (20, 20);
        let mut inp = blank(w, h);
        for z in 8..11 {
            for x in 8..12 {
                let c = z * w + x;
                inp.photo[c].blue = 12;
                inp.photo[c].exg = -10;
                inp.photo[c].green = 0;
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
    fn earth_between_the_bands_is_open_ground_and_grey_paving() {
        let (w, h) = (30, 20);
        let mut inp = blank(w, h);
        // Neither band's intensity, not green: a building site's earth on the
        // left, light grey paving on the right.
        for z in 0..h {
            for x in 0..w {
                let c = z * w + x;
                inp.i_gnd_q[c] = 1520;
                inp.photo[c] = if x < 15 {
                    photo(0, 0, 175, -4, [190, 175, 157])
                } else {
                    photo(-4, 0, 170, -2, [172, 170, 168])
                };
            }
        }
        let out = classify(&inp, (1450.0, 1600.0));
        assert_eq!(out[10 * w + 5].class, BARE);
        assert_eq!(out[10 * w + 25].class, PAVED);
        // Red brick paving is no earth.
        inp.photo.fill(photo(-6, 0, 120, -8, [160, 95, 80]));
        let out = classify(&inp, (1450.0, 1600.0));
        assert_eq!(out[10 * w + 5].class, PAVED);
    }

    #[test]
    fn blue_on_open_water_is_no_pool_and_a_boat_no_shed() {
        let (w, h) = (24, 24);
        let mut inp = blank(w, h);
        // Water all round, or a tile the laser is missing: no returns at all.
        for c in 0..w * h {
            inp.n_all[c] = 0;
            inp.n_gnd[c] = 0;
            inp.photo[c] = photo(-10, 0, 60, 0, [55, 60, 65]);
        }
        // A blue tarp or a dinghy, pool-sized.
        for z in 14..18 {
            for x in 14..18 {
                inp.photo[z * w + x] = photo(-10, 0, 120, 14, [60, 110, 170]);
            }
        }
        // A boat: a hull 3 m over the water, 6 x 3 blocks.
        for z in 4..7 {
            for x in 4..10 {
                let c = z * w + x;
                inp.n_all[c] = 12;
                inp.n_bld[c] = 6;
                inp.h_bld_q[c] = 15;
                inp.photo[c] = photo(-6, 0, 200, -4, [220, 220, 215]);
            }
        }
        let out = classify(&inp, (1450.0, 1600.0));
        assert!(out.iter().all(|o| o.class != POOL), "no pool on the sea");
        assert!(out.iter().all(|o| o.class != SHED), "no shed on the sea");
        // The same shed in a garden stands.
        let mut garden = blank(w, h);
        for z in 4..7 {
            for x in 4..10 {
                let c = z * w + x;
                garden.n_bld[c] = 6;
                garden.h_bld_q[c] = 15;
                garden.photo[c] = photo(-6, 0, 150, -4, [150, 140, 130]);
            }
        }
        let out = classify(&garden, (1450.0, 1600.0));
        assert_eq!(out[5 * w + 6].class, SHED);
    }

    #[test]
    fn a_strict_source_needs_no_returns_at_all_for_a_pool() {
        let (w, h) = (20, 20);
        let mut inp = blank(w, h);
        inp.strict_pools = true;
        inp.n_all = vec![4; w * h];
        for z in 8..11 {
            for x in 8..12 {
                let c = z * w + x;
                inp.photo[c].blue = 12;
                inp.photo[c].exg = -10;
                inp.photo[c].green = 0;
                // Two returns of four: chance at this density, not water.
                inp.n_all[c] = 2;
            }
        }
        let out = classify(&inp, (1450.0, 1600.0));
        assert!(out.iter().all(|o| o.class != POOL));
        for z in 8..11 {
            for x in 8..12 {
                inp.n_all[z * w + x] = 0;
            }
        }
        let out = classify(&inp, (1450.0, 1600.0));
        assert_eq!(out[9 * w + 9].class, POOL);
    }

    #[test]
    fn green_roofs_of_a_guess_are_crowns() {
        let (w, h) = (10, 10);
        let mut inp = blank(w, h);
        // A green cell and a grey one, both with building points 9 m high
        // and an object 4 m high.
        for (c, green) in [(11, true), (55, false)] {
            inp.n_bld[c] = 5;
            inp.h_bld_q[c] = 45;
            inp.n_obj[c] = 1;
            inp.h_obj_q[c] = 20;
            if !green {
                inp.photo[c].exg = -4;
                inp.photo[c].green = 0;
            }
        }
        crowns_not_roofs(&mut inp);
        assert_eq!((inp.n_bld[11], inp.n_obj[11], inp.h_obj(11)), (0, 6, 9.0));
        assert_eq!((inp.n_bld[55], inp.n_obj[55]), (5, 1));
    }

    #[test]
    fn eaves_are_not_sheds_but_a_garden_house_is() {
        let (w, h) = (40, 30);
        let mut inp = blank(w, h);
        // Building points `hb` fifths of a metre high.
        let mut put = |x0: usize, x1: usize, z0: usize, z1: usize, hb: u8| {
            for z in z0..z1 {
                for x in x0..x1 {
                    let c = z * w + x;
                    inp.n_bld[c] = 10;
                    inp.h_bld_q[c] = hb;
                }
            }
        };
        put(5, 15, 5, 15, 40); // a house, 8 m
        put(15, 17, 5, 15, 15); // its eaves, low building points along a wall
        put(28, 31, 20, 23, 12); // a garden house on its own, 2.4 m
        let out = classify(&inp, (1450.0, 1600.0));
        assert_ne!(out[10 * w + 16].class, SHED);
        assert_eq!(out[21 * w + 29].class, SHED);
        assert_eq!(out[21 * w + 29].height_q, 12);
    }

    #[test]
    fn paving_brighter_than_lawn_is_turned_over() {
        let (w, h) = (40, 40);
        let mut inp = blank(w, h);
        // Lawn darker than paving, as Latvia's scans read.
        inp.i_gnd_q = vec![50; w * h];
        for c in 0..w * h / 2 {
            inp.i_gnd_q[c] = 90;
            inp.photo[c].exg = -10;
            inp.photo[c].lum = 150;
        }
        // Read as measured, lawn would have to be the brighter.
        assert_eq!(calibrate(&inp, (0.0, 0.0)).1, Calibration::Laser);
        inp.paving_brighter = true;
        let ((lo, hi), how) = calibrate(&inp, (0.0, 0.0));
        assert_eq!(how, Calibration::Photo);
        // The paving's turned-over 65445 lies under the band, the lawn's
        // 65485 over it.
        assert!(lo > 65_445.0 && hi < 65_485.0, "{lo} {hi}");
        // A house by the paving, which keeps it (see the next test).
        for c in 0..3 * w {
            if c % w < 6 {
                inp.n_bld[c] = 10;
                inp.h_bld_q[c] = 40;
            }
        }
        let out = classify(&inp, (lo, hi));
        assert_eq!(out[5 * w + 5].class, PAVED);
        assert_eq!(out[35 * w + 5].class, LAWN);
    }

    #[test]
    fn turned_over_paving_stays_near_the_buildings() {
        let (w, h) = (80, 10);
        let mut inp = blank(w, h);
        // Bare ground all along, read as paving, and a house at the west end.
        inp.i_gnd_q = vec![1400; w * h];
        for p in &mut inp.photo {
            p.exg = -10;
        }
        for z in 2..8 {
            for x in 0..6 {
                inp.n_bld[z * w + x] = 10;
                inp.h_bld_q[z * w + x] = 40;
            }
        }
        let out = classify(&inp, (1450.0, 1600.0));
        assert_eq!(out[5 * w + 60].class, PAVED, "unturned, paving stays");
        // The same intensity where paving reads the brighter.
        inp.paving_brighter = true;
        inp.i_gnd_q.fill(u16::MAX - 1400);
        let out = classify(&inp, (1450.0, 1600.0));
        assert_eq!(out[5 * w + 20].class, PAVED, "the yard");
        assert_eq!(out[5 * w + 60].class, LAWN, "open ground");
    }

    #[test]
    fn without_a_photo_paving_brighter_is_split_as_measured_and_turned_over() {
        let (w, h) = (40, 40);
        let mut inp = blank(w, h);
        inp.paving_brighter = true;
        inp.photo = vec![PhotoCell::default(); w * h];
        // Riga's ground: paving 80 to 88, lawn 0 to 2, mostly 0.
        for (c, v) in inp.i_gnd_q.iter_mut().enumerate() {
            *v = if c < w * h / 2 {
                80 + (c % 9) as u16
            } else {
                (c % 3) as u16
            };
        }
        let ((lo, hi), how) = calibrate(&inp, (40.0, 60.0));
        assert_eq!(how, Calibration::Laser);
        // Turned over: the lawn's 65533 and up over it, the paving's 65455
        // and down under it.
        assert!(lo == hi && lo > 65_455.0 && lo <= 65_533.0, "{lo}");
        for c in 0..3 * w {
            if c % w < 6 {
                inp.n_bld[c] = 10;
                inp.h_bld_q[c] = 40;
            }
        }
        let out = classify(&inp, (lo, hi));
        assert_eq!(out[5 * w + 5].class, PAVED);
        assert_eq!(out[35 * w + 5].class, LAWN);
        // Ground of one intensity does not split: the defaults, lawn up to 40
        // and paving past 60, turned over.
        inp.i_gnd_q.fill(50);
        assert_eq!(
            calibrate(&inp, (40.0, 60.0)),
            ((65_475.0, 65_495.0), Calibration::Defaults)
        );
    }

    #[test]
    fn calibration_finds_the_midway_band() {
        let (w, h) = (40, 40);
        let mut inp = blank(w, h);
        for c in 0..w * h / 2 {
            inp.i_gnd_q[c] = 1400;
            inp.photo[c].exg = -10;
            inp.photo[c].lum = 150;
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
            inp.h_obj_q[c] = 7;
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
        inp.photo = vec![PhotoCell::default(); w * h];
        // A hedge along row 5 (most hits not the last of their pulse) and a
        // fence along row 14 (solid).
        for x in 2..28 {
            let c = 5 * w + x;
            inp.n_obj[c] = 10;
            inp.n_veg[c] = 7;
            inp.h_obj_q[c] = 7;
            let c = 14 * w + x;
            inp.n_obj[c] = 6;
            inp.n_veg[c] = 0;
            inp.h_obj_q[c] = 8;
        }
        let out = classify(&inp, (1450.0, 1600.0));
        assert_eq!(out[5 * w + 5].class, HEDGE);
        assert_eq!(out[14 * w + 5].class, FENCE);
    }
}
