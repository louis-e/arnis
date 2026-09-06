//! How much a wall is to be trusted. Port of `tools/facade_lab/confidence.py`.
//!
//! Ten factors, each in `(0, 1]`, combined by their geometric mean, so one bad
//! factor pulls the whole wall down instead of being averaged away. The tables
//! are exact values, not a formula, so that a sheet can print what was used:
//!
//! ```text
//! pose        sfm 1.0   sequence 0.8   autolevel 0.65   heading 0.4
//! registration local 1.0  global 0.8    none 0.6         no_cluster 0.5
//! plane       cloud 1.0   osm-registered 0.75            osm-raw 0.5
//! extent      cloud-corner 1.0  edge-joint 0.9  edge-shift 0.8  osm 0.6
//! height      sky+cloud 1.0  sky 0.9  cloud 0.85  tag 0.75  levels 0.6  default 0.35
//! lean        SHEAR_OK 1.0   SHEAR_NONE 0.85   SHEAR_REJECTED 0.7
//! plane_gate  ON_PLANE 1.0   UNVERIFIED 0.8    PLANE_MISMATCH 0.3
//! ```
//!
//! plus `views` (1.0 at agreement under 0.5 m falling to 0.4 at 2 m, 0.7 for a
//! single view, 0.4 for none), `occlusion` (one minus the unknown share) and
//! `image` (quality score times the blur factor). Registration is multiplied by
//! 0.8 on a scale-suspect cluster and height by 0.7 on `HEIGHT_CONFLICT`.
//!
//! The tier follows `tier_a`/`tier_b`/`tier_c`, and then the coverage caps
//! apply: a wall whose texture is mostly no-data must not carry a tier that
//! promises a texture, so tier A needs at least 60 per cent observed blocks and
//! tier B at least 20 per cent.

#![allow(dead_code)]

use super::refine::Refinement;
use super::types::{
    Params, PlaneFit, PlaneSource, PoseSource, RegResult, RegSource, Tier, ViewCandidate,
    WallDecision,
};

/// The factor floor. Nothing is ever worth exactly zero, or the geometric mean
/// would collapse the whole wall on one missing input.
pub const FACTOR_FLOOR: f64 = 0.05;

const SCALE_SUSPECT_MULT: f64 = 0.8;
const HEIGHT_CONFLICT_MULT: f64 = 0.7;
const SINGLE_VIEW_FACTOR: f64 = 0.7;
const NO_VIEW_FACTOR: f64 = 0.4;
/// 1.0 at an agreement of `.0` metres or better, `VIEWS_MIN` at `.1`.
const VIEWS_AGREE_M: (f64, f64) = (0.5, 2.0);
const VIEWS_MIN: f64 = 0.4;

/// The ten factors, in the order the sheets print them.
#[derive(Clone, Copy, Debug)]
pub struct Factors {
    pub pose: f64,
    pub registration: f64,
    pub plane: f64,
    pub extent: f64,
    pub height: f64,
    pub lean: f64,
    pub plane_gate: f64,
    pub views: f64,
    pub occlusion: f64,
    pub image: f64,
}

impl Factors {
    pub fn as_array(&self) -> [f64; 10] {
        [
            self.pose,
            self.registration,
            self.plane,
            self.extent,
            self.height,
            self.lean,
            self.plane_gate,
            self.views,
            self.occlusion,
            self.image,
        ]
    }

    /// Every value clipped to `[FACTOR_FLOOR, 1]`, which is what
    /// `factors_for` hands out and what `score` assumes.
    pub fn clipped(self) -> Self {
        let c = |v: f64| v.clamp(FACTOR_FLOOR, 1.0);
        Self {
            pose: c(self.pose),
            registration: c(self.registration),
            plane: c(self.plane),
            extent: c(self.extent),
            height: c(self.height),
            lean: c(self.lean),
            plane_gate: c(self.plane_gate),
            views: c(self.views),
            occlusion: c(self.occlusion),
            image: c(self.image),
        }
    }
}

/// The pose factor. `PoseSource::factor` is the same table, already in
/// `types.rs`, so it is used rather than repeated.
pub fn pose_factor(src: PoseSource) -> f64 {
    src.factor()
}

/// The registration factor, with the scale-suspect multiplier. A wall with no
/// cluster at all scores lower than one whose cluster simply failed to
/// register, because there was never anything to check the pose against.
pub fn registration_factor(reg: Option<&RegResult>) -> f64 {
    let Some(reg) = reg else {
        return 0.5;
    };
    let base = match reg.source {
        RegSource::Local => 1.0,
        RegSource::Global => 0.8,
        RegSource::Unregistered => 0.6,
        RegSource::NoCluster => 0.5,
    };
    if reg.scale_suspect {
        base * SCALE_SUSPECT_MULT
    } else {
        base
    }
}

pub fn plane_factor(fit: Option<&PlaneFit>) -> f64 {
    match fit.map(|f| f.source) {
        Some(PlaneSource::Cloud) => 1.0,
        Some(PlaneSource::OsmRegistered) => 0.75,
        _ => 0.5,
    }
}

fn extent_one(src: &str) -> f64 {
    match src {
        "cloud-corner" => 1.0,
        "edge-joint" => 0.9,
        "edge-shift" => 0.8,
        _ => 0.6,
    }
}

/// The mean of the two ends: a wall can know one end from the cloud and the
/// other only from OSM.
pub fn extent_factor(src_a: &str, src_b: &str) -> f64 {
    0.5 * (extent_one(src_a) + extent_one(src_b))
}

fn height_one(src: &str) -> Option<f64> {
    Some(match src {
        "sky+cloud" => 1.0,
        "sky" => 0.9,
        "cloud" => 0.85,
        "tag" => 0.75,
        "levels" => 0.6,
        "default" => 0.35,
        _ => return None,
    })
}

/// The height factor.
///
/// `refine.decide_height` reports `osm` when the cloud was present but the OSM
/// tag was kept, which says nothing about how good the number is, so that case
/// is resolved through the OSM source (tag, levels or the 9 m default) and the
/// factor reflects where the number actually came from.
pub fn height_factor(height_source: &str, flags: &[String], osm_source: Option<&str>) -> f64 {
    let f = match height_one(height_source) {
        Some(v) if height_source != "osm" => v,
        _ => osm_source.and_then(height_one).unwrap_or(0.35),
    };
    if flags.iter().any(|s| s == "HEIGHT_CONFLICT") {
        f * HEIGHT_CONFLICT_MULT
    } else {
        f
    }
}

/// The best (first) view's lean flag decides; no refinement means `SHEAR_NONE`.
pub fn lean_factor(refs: &[Refinement]) -> f64 {
    match refs.first().map(|r| r.lean_flag.as_str()) {
        Some("SHEAR_OK") => 1.0,
        Some("SHEAR_REJECTED") => 0.7,
        _ => 0.85,
    }
}

/// `PLANE_MISMATCH` on any view wins, because it is a detector and one view
/// seeing the wall off its plane is enough; then `ON_PLANE`, else unverified.
pub fn plane_gate_factor(refs: &[Refinement]) -> f64 {
    if refs.iter().any(|r| r.plane_flag == "PLANE_MISMATCH") {
        return 0.3;
    }
    if refs.iter().any(|r| r.plane_flag == "ON_PLANE") {
        return 1.0;
    }
    0.8
}

/// The views factor: 1.0 at an agreement of 0.5 m or better, falling linearly
/// to 0.4 at 2 m; 0.7 for a single view and 0.4 for none.
pub fn views_factor(agreement_m: f64, n_views: usize) -> f64 {
    if n_views == 0 {
        return NO_VIEW_FACTOR;
    }
    if n_views == 1 {
        return SINGLE_VIEW_FACTOR;
    }
    let (lo, hi) = VIEWS_AGREE_M;
    let a = if agreement_m.is_finite() {
        agreement_m
    } else {
        hi
    };
    if a <= lo {
        return 1.0;
    }
    if a >= hi {
        return VIEWS_MIN;
    }
    1.0 - (1.0 - VIEWS_MIN) * (a - lo) / (hi - lo)
}

/// Every factor for one wall, clipped to `[FACTOR_FLOOR, 1]`.
#[allow(clippy::too_many_arguments)]
pub fn factors_for(
    pose_source: Option<PoseSource>,
    reg: Option<&RegResult>,
    fit: Option<&PlaneFit>,
    dec: &WallDecision,
    refs: &[Refinement],
    agreement_m: f64,
    n_views: usize,
    unknown_share: f64,
    best_quality: f64,
    blur_factor: f64,
    osm_source: Option<&str>,
) -> Factors {
    Factors {
        pose: pose_factor(pose_source.unwrap_or(PoseSource::Heading)),
        registration: registration_factor(reg),
        plane: plane_factor(fit),
        extent: extent_factor(&dec.src_a, &dec.src_b),
        height: height_factor(&dec.height_source, &dec.flags, osm_source),
        lean: lean_factor(refs),
        plane_gate: plane_gate_factor(refs),
        views: views_factor(agreement_m, n_views),
        occlusion: 1.0 - unknown_share,
        image: best_quality * blur_factor,
    }
    .clipped()
}

/// The tier a confidence alone would give, before the coverage caps.
pub fn tier_of(conf: f64, params: &Params) -> Tier {
    if conf >= params.tier_a {
        Tier::A
    } else if conf >= params.tier_b {
        Tier::B
    } else if conf >= params.tier_c {
        Tier::C
    } else {
        Tier::D
    }
}

/// The geometric mean of the factors and the tier after the coverage caps.
///
/// The caps are an audit finding: a wall whose texture is mostly no-data must
/// not carry a tier that promises a texture. The occlusion factor is one minus
/// the unknown share, so the share is recovered from it rather than passed in
/// twice and allowed to disagree with itself.
pub fn score(factors: &Factors, params: &Params) -> (f64, Tier) {
    let vals = factors.as_array();
    let conf =
        (vals.iter().map(|v| v.max(FACTOR_FLOOR).ln()).sum::<f64>() / vals.len() as f64).exp();
    let unknown = 1.0 - factors.occlusion;
    let mut tier = tier_of(conf, params);
    if tier == Tier::A && unknown > params.tier_a_max_unknown {
        tier = Tier::B;
    }
    if matches!(tier, Tier::A | Tier::B) && unknown > params.tier_b_max_unknown {
        tier = Tier::C;
    }
    (conf, tier)
}

/// `GATE_<first token>` of a rejection reason, so a wall's absence reads as a
/// code rather than as a sentence.
fn gate_code(reason: &str) -> String {
    let token: String = reason
        .trim()
        .split(' ')
        .next()
        .unwrap_or("")
        .split('<')
        .next()
        .unwrap_or("")
        .trim()
        .to_uppercase()
        .replace('-', "_");
    format!(
        "GATE_{}",
        if token.is_empty() { "REJECTED" } else { &token }
    )
}

/// Sorted union of the decision flags, the per view refinement flags, the
/// registration outcome and the candidate gate rejections, for the JSON and the
/// sheet footer.
pub fn reason_codes(
    cands: &[ViewCandidate],
    dec: Option<&WallDecision>,
    refs: &[Refinement],
    reg: Option<&RegResult>,
    extra: &[String],
) -> Vec<String> {
    let mut codes: Vec<String> = Vec::new();
    let mut push = |c: String| {
        if !c.is_empty() && !codes.contains(&c) {
            codes.push(c);
        }
    };
    if let Some(d) = dec {
        for f in &d.flags {
            push(f.clone());
        }
    }
    for r in refs {
        for flag in [&r.lean_flag, &r.plane_flag, &r.roof_flag, &r.ground_flag] {
            push(flag.clone());
        }
    }
    if let Some(reg) = reg {
        match reg.source {
            RegSource::Global => push("REG_GLOBAL".to_string()),
            RegSource::Unregistered => push("REG_NONE".to_string()),
            RegSource::NoCluster => push("REG_NO_CLUSTER".to_string()),
            RegSource::Local => {}
        }
        if reg.scale_suspect {
            push("SCALE_SUSPECT".to_string());
        }
    }
    for c in cands {
        if let Some(reason) = &c.rejected_reason {
            push(gate_code(reason));
        }
    }
    for e in extra {
        push(e.clone());
    }
    codes.sort();
    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(v: f64) -> Factors {
        Factors {
            pose: v,
            registration: v,
            plane: v,
            extent: v,
            height: v,
            lean: v,
            plane_gate: v,
            views: v,
            occlusion: v,
            image: v,
        }
    }

    #[test]
    fn the_geometric_mean_of_equal_factors_is_that_factor() {
        let p = Params::default();
        let (conf, tier) = score(&flat(0.8), &p);
        assert!((conf - 0.8).abs() < 1e-12, "{conf}");
        // occlusion 0.8 means a fifth unknown, which is inside the tier A cap
        assert_eq!(tier, Tier::A);
    }

    #[test]
    fn one_bad_factor_pulls_the_whole_wall_down() {
        let p = Params::default();
        let mut f = flat(1.0);
        f.height = 0.35;
        let (conf, _) = score(&f, &p);
        // the arithmetic mean would be 0.935; the geometric one is much lower
        assert!((conf - 0.35f64.powf(0.1)).abs() < 1e-12, "{conf}");
        assert!(conf < 0.91);
    }

    #[test]
    fn the_coverage_caps_demote_an_otherwise_confident_wall() {
        let p = Params::default();
        let mut f = flat(1.0);
        // 45 per cent of the blocks unknown: a confident wall, but not one that
        // may promise a texture
        f.occlusion = 0.55;
        let (conf, tier) = score(&f, &p);
        assert!(conf > p.tier_a, "{conf}");
        assert_eq!(tier, Tier::B);
        f.occlusion = 0.15;
        assert_eq!(score(&f, &p).1, Tier::C);
    }

    #[test]
    fn views_factor_matches_the_table() {
        assert_eq!(views_factor(0.0, 0), 0.4);
        assert_eq!(views_factor(0.0, 1), 0.7);
        assert_eq!(views_factor(0.4, 3), 1.0);
        assert_eq!(views_factor(0.5, 2), 1.0);
        assert_eq!(views_factor(2.0, 2), 0.4);
        assert_eq!(views_factor(9.0, 2), 0.4);
        assert!((views_factor(1.25, 2) - 0.7).abs() < 1e-12);
        // a non-finite agreement is treated as the worst one
        assert_eq!(views_factor(f64::NAN, 2), 0.4);
    }

    #[test]
    fn height_osm_is_resolved_through_the_osm_source() {
        assert_eq!(height_factor("sky+cloud", &[], None), 1.0);
        assert_eq!(height_factor("osm", &[], Some("tag")), 0.75);
        assert_eq!(height_factor("osm", &[], Some("levels")), 0.6);
        assert_eq!(height_factor("osm", &[], None), 0.35);
        let conflict = vec!["HEIGHT_CONFLICT".to_string()];
        assert!((height_factor("levels", &conflict, None) - 0.42).abs() < 1e-12);
    }

    #[test]
    fn registration_and_extent_follow_the_table() {
        assert_eq!(registration_factor(None), 0.5);
        let mut reg = RegResult {
            source: RegSource::Local,
            ..Default::default()
        };
        assert_eq!(registration_factor(Some(&reg)), 1.0);
        reg.scale_suspect = true;
        assert_eq!(registration_factor(Some(&reg)), 0.8);
        assert_eq!(extent_factor("cloud-corner", "osm"), 0.8);
        assert_eq!(extent_factor("edge-joint", "edge-joint"), 0.9);
        assert_eq!(extent_factor("nonsense", "nonsense"), 0.6);
    }

    #[test]
    fn tiers_follow_the_params() {
        let p = Params::default();
        assert_eq!(tier_of(0.61, &p), Tier::A);
        assert_eq!(tier_of(0.6, &p), Tier::A);
        assert_eq!(tier_of(0.59, &p), Tier::B);
        assert_eq!(tier_of(0.38, &p), Tier::B);
        assert_eq!(tier_of(0.2, &p), Tier::C);
        assert_eq!(tier_of(0.19, &p), Tier::D);
    }

    /// Every wall the reference run scored, through the same arithmetic.
    ///
    /// The factors are taken from the fixture rather than rebuilt, because the
    /// stages that produce them are not all ported yet; what is checked here is
    /// the part that is, which is the combination and the caps. The two factors
    /// that are computed rather than looked up are recomputed from their own
    /// inputs on top, so the table is not the only thing under test.
    #[test]
    fn the_confidence_reproduces_the_python() {
        if golden::absent() {
            return;
        }

        use crate::mapillary::golden;

        let p = Params::default();
        let fixture = golden::confidence_walls();
        assert_eq!(fixture.count, fixture.walls.len());
        assert!(fixture.walls.len() >= 100, "the fixture is too small");
        let mut worst_conf = 0.0f64;
        let mut tiers = std::collections::BTreeMap::new();
        for w in &fixture.walls {
            let f = Factors {
                pose: w.factors.pose,
                registration: w.factors.registration,
                plane: w.factors.plane,
                extent: w.factors.extent,
                height: w.factors.height,
                lean: w.factors.lean,
                plane_gate: w.factors.plane_gate,
                views: w.factors.views,
                occlusion: w.factors.occlusion,
                image: w.factors.image,
            };
            let (conf, tier) = score(&f, &p);
            worst_conf = worst_conf.max((conf - w.confidence).abs());
            assert_eq!(tier.as_str(), w.tier, "{}: tier", w.key);
            *tiers.entry(tier.as_str()).or_insert(0usize) += 1;

            // the occlusion factor must really be one minus the unknown share,
            // which is what the coverage caps read it back out of
            assert!(
                (f.occlusion - (1.0 - w.unknown_share)).abs() < 1e-9,
                "{}: occlusion against unknown share",
                w.key
            );
            assert!(
                (views_factor(w.agreement_m, w.n_views) - f.views).abs() < 1e-9,
                "{}: views factor",
                w.key
            );
            let h = height_factor(&w.height_source, &w.flags, w.osm_source.as_deref());
            assert!(
                (h.clamp(FACTOR_FLOOR, 1.0) - f.height).abs() < 1e-9,
                "{}: height factor {h} against {}",
                w.key,
                f.height
            );
        }
        println!(
            "confidence over {} walls: worst |dconf| {worst_conf:.2e}, tiers {tiers:?}",
            fixture.walls.len()
        );
        assert!(
            worst_conf < 1e-9,
            "worst confidence difference {worst_conf}"
        );
        // The reference run's own tiers. `w1273939824_4` is the wall that moves
        // between 102/9 and 103/8: the roof vote sent it to the tag's 12 m and
        // the two rows that came off the rectangle took enough of its texture
        // with them to drop it to B, and the no-data rule sends it back, because
        // the sky boundary that started the argument was the edge of a
        // photograph and neither view proposes one any more.
        assert_eq!(tiers.get("A").copied().unwrap_or(0), 103);
        assert_eq!(tiers.get("B").copied().unwrap_or(0), 8);
    }

    #[test]
    fn gate_codes_take_the_first_token() {
        assert_eq!(gate_code("blur 12 < 40"), "GATE_BLUR");
        assert_eq!(gate_code("no-cluster"), "GATE_NO_CLUSTER");
        assert_eq!(gate_code(""), "GATE_REJECTED");
    }
}
