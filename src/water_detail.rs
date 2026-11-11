//! `--water-detail scaled`: water fixes that matter most on scaled-down maps.
//!
//! - The sqrt bed curve rounds instead of flooring, at every scale.
//! - Below scale 0.5 a body's bed is a linear bowl keyed to its own half-width, so a river
//!   a few blocks wide still carves a channel instead of the shoal ring flattening it.
//! - Line waterways are capped at 5 blocks wide below scale 0.7.
//! - At scale 0.5 and below, a road not tagged as a bridge is not drawn on land-cover
//!   water, so the water stays continuous instead of being cut by a 1-block causeway.
//!
//! All of it is a pure function of block position, tags and scale.

use crate::water_depth::MAX_WATER_DEPTH;
use std::collections::HashMap;

/// Scale below which the bowl replaces the tiered sqrt bed.
const BOWL_BELOW_SCALE: f64 = 0.5;

/// Deepest bowl carve, in blocks.
pub const BOWL_MAX_DEPTH: i32 = 5;

/// Scale at or below which untagged road crossings over water are drowned.
const DROWN_AT_OR_BELOW_SCALE: f64 = 0.5;

/// `--water-detail` value.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WaterDetail {
    /// The water every scale gets today
    #[default]
    Default,
    /// Rounded bed depths, plus bowls, narrower streams and drowned causeways at small scale
    Scaled,
}

impl WaterDetail {
    /// Round the sqrt bed curve instead of flooring it.
    pub fn rounds_depth(self) -> bool {
        self == Self::Scaled
    }

    pub fn uses_bowl(self, scale: f64) -> bool {
        self == Self::Scaled && scale < BOWL_BELOW_SCALE
    }

    /// Widest line waterway to draw, or `None` for the tagged width. `create_water_channel`
    /// paints `width / 2 + 1` either side, so 3 draws 5 wide. Below `OBJECT_SKIP_SCALE`
    /// there are no waterways to draw.
    pub fn channel_width_cap(self, scale: f64) -> Option<i32> {
        (self == Self::Scaled && scale < 0.7).then_some(3)
    }

    /// True when a road with these tags is left off land-cover water cells. The road
    /// draw and the road mask both ask this, so the carve fills exactly the cells the
    /// road skipped.
    pub fn drowns_crossing(self, scale: f64, tags: &HashMap<String, String>) -> bool {
        self == Self::Scaled
            && scale <= DROWN_AT_OR_BELOW_SCALE
            && tags.get("bridge").is_none_or(|b| b == "no")
    }

    /// Deepest carve this mode can produce, given the legacy model's estimate.
    pub fn max_carve_depth(self, estimate: i32, scale: f64) -> i32 {
        match self {
            Self::Default => estimate,
            // Rounding adds at most one block to the floored estimate.
            Self::Scaled if !self.uses_bowl(scale) => (estimate + 1).min(MAX_WATER_DEPTH),
            Self::Scaled => BOWL_MAX_DEPTH,
        }
    }
}

/// What the water options add to the land-cover estimate of the deepest carve, so the
/// ground datum leaves room for it. The default leaves the estimate alone.
#[derive(Clone, Copy, Debug, Default)]
pub struct CarveDepth {
    pub floor: i32,
    pub detail: WaterDetail,
    pub scale: f64,
}

impl CarveDepth {
    pub fn bound(self, estimate: i32) -> i32 {
        // No water in the land cover, nothing to carve: the datum stays where flags-off puts it.
        if estimate == 0 {
            return 0;
        }
        self.detail
            .max_carve_depth(estimate, self.scale)
            .max(self.floor)
    }
}

/// Bowl depth for one cell: one block deeper per block inward past a 1-block shoal, up to
/// a target set by the body's half-width and never steeper than the bank can climb.
/// `dt_units` and `component_max_units` are chamfer-3-4 units (3 per block).
pub fn bowl_depth(x: i32, z: i32, dt_units: u16, component_max_units: u16, scale: f64) -> i32 {
    // The bank wobble shrinks with scale so it never swamps a body a few blocks wide.
    let s = scale.min(1.0);
    let wavelength = (12.0 / s).round().max(12.0) as i32;
    let wobble = (crate::ground_generation::value_noise_01(x, z, wavelength) - 0.5) * (4.0 * s);
    let d_blocks = (f64::from(dt_units) + wobble) / 3.0;
    bowl_steps(d_blocks, f64::from(component_max_units) / 3.0)
}

fn bowl_steps(d_blocks: f64, hw_blocks: f64) -> i32 {
    const SHOAL_BLOCKS: f64 = 1.0;
    let run = (hw_blocks - SHOAL_BLOCKS).max(0.0);
    let target = ((hw_blocks * 0.7).round().min(run.floor()) as i32).clamp(0, BOWL_MAX_DEPTH);
    ((d_blocks - SHOAL_BLOCKS).floor() as i32).clamp(0, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_changes_nothing() {
        let d = WaterDetail::Default;
        let tags = HashMap::new();
        for scale in [0.1, 0.25, 0.5, 1.0, 2.0] {
            assert!(!d.rounds_depth());
            assert!(!d.uses_bowl(scale));
            assert_eq!(d.channel_width_cap(scale), None);
            assert!(!d.drowns_crossing(scale, &tags));
            assert_eq!(d.max_carve_depth(3, scale), 3);
        }
        assert_eq!(CarveDepth::default().bound(3), 3);
    }

    #[test]
    fn scaled_gates_follow_scale() {
        let s = WaterDetail::Scaled;
        assert!(s.uses_bowl(0.25) && !s.uses_bowl(0.5));
        assert_eq!(s.channel_width_cap(0.5), Some(3));
        assert_eq!(s.channel_width_cap(1.0), None);
        let mut tags = HashMap::new();
        assert!(s.drowns_crossing(0.5, &tags) && !s.drowns_crossing(0.6, &tags));
        tags.insert("bridge".to_string(), "yes".to_string());
        assert!(!s.drowns_crossing(0.25, &tags));
        assert_eq!(s.max_carve_depth(3, 1.0), 4);
        assert_eq!(s.max_carve_depth(6, 1.0), 6);
        assert_eq!(s.max_carve_depth(0, 0.25), BOWL_MAX_DEPTH);
    }

    #[test]
    fn bowl_climbs_one_block_per_block_and_caps() {
        // A wide body: depth grows by one per block past the shoal, then caps.
        let steps: Vec<i32> = (0..10).map(|d| bowl_steps(f64::from(d), 20.0)).collect();
        assert_eq!(steps, vec![0, 0, 1, 2, 3, 4, 5, 5, 5, 5]);
        for w in steps.windows(2) {
            assert!(w[1] - w[0] <= 1);
        }
        // A 5-wide river (hw 2.5) still carves, but only as deep as its run allows.
        assert_eq!(bowl_steps(2.5, 2.5), 1);
        // A 1-wide trickle stays surface-only.
        assert_eq!(bowl_steps(0.5, 0.5), 0);
    }

    #[test]
    fn no_water_keeps_the_datum() {
        for detail in [WaterDetail::Default, WaterDetail::Scaled] {
            for scale in [0.25, 1.0] {
                for floor in [0, crate::water_depth::MAX_WATER_DEPTH] {
                    let c = CarveDepth {
                        floor,
                        detail,
                        scale,
                    };
                    assert_eq!(c.bound(0), 0, "{detail:?} {scale} {floor}");
                }
            }
        }
        // With water the floor and the +1 still apply.
        let c = CarveDepth {
            floor: 6,
            detail: WaterDetail::Scaled,
            scale: 1.0,
        };
        assert_eq!(c.bound(2), 6);
    }

    #[test]
    fn default_is_v340() {
        use clap::Parser;
        let args =
            crate::args::Args::parse_from(["arnis", "--output-dir", ".", "--bbox", "1,2,3,4"]);
        assert_eq!(args.water.water_detail, WaterDetail::Default);
        // The default carves exactly what v3.4.0 carved.
        assert_eq!(args.water.carve_depth(1.0).bound(4), 4);
    }

    #[test]
    fn bowl_depth_is_a_pure_function_of_position() {
        for (x, z) in [(0, 0), (-1000, 77), (123_456, -9)] {
            let a = bowl_depth(x, z, 12, 30, 0.25);
            assert_eq!(a, bowl_depth(x, z, 12, 30, 0.25));
            assert!((0..=BOWL_MAX_DEPTH).contains(&a));
        }
    }
}
