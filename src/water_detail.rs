//! `--water-detail scaled`: water fixes that matter most on scaled-down maps.
//!
//! - The sqrt bed curve rounds instead of flooring, at every scale.
//! - Below scale 0.5 a body's bed is a linear bowl keyed to its own half-width.
//! - Below scale 0.7 line rivers and canals are 5 blocks wide at most, streams and ditches 3.
//! - At scale 0.5 and below, water runs over a road where it crosses a short stretch of
//!   water without a bridge tag.

use crate::water_depth::MAX_WATER_DEPTH;
use std::collections::HashMap;

/// Scale below which the bowl replaces the tiered sqrt bed.
const BOWL_BELOW_SCALE: f64 = 0.5;

/// Deepest bowl carve, in blocks.
pub const BOWL_MAX_DEPTH: i32 = 5;

/// Scale at or below which untagged road crossings over water are drowned.
const DROWN_AT_OR_BELOW_SCALE: f64 = 0.5;

/// Longest drowned crossing, in blocks; a longer one is a dam, a causeway or a shore road.
const MAX_DROWNED_RUN: usize = 12;

/// Dry road a crossing needs on each end, in centre points.
const DRY_ENDS: usize = 2;

/// `--water-detail` value.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WaterDetail {
    /// The water every scale gets today
    #[default]
    Default,
    /// Rounded bed depths; on small maps also bowls, narrower streams and water over short unbridged crossings
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
    /// paints `width / 2 + 1` either side, so 3 draws 5 wide; streams and ditches draw 3 wide.
    pub fn channel_width_cap(self, scale: f64) -> Option<i32> {
        (self == Self::Scaled && scale < 0.7).then_some(3)
    }

    /// True when a road with these tags may be left off a short water crossing. The road
    /// mask decides which cells; the road draw follows the mask.
    pub fn drowns_crossing(self, scale: f64, tags: &HashMap<String, String>) -> bool {
        let set = |key: &str| tags.get(key).is_some_and(|v| v != "no");
        self == Self::Scaled
            && scale <= DROWN_AT_OR_BELOW_SCALE
            // Bridges, tunnels and raised or banked ways keep their whole surface.
            && !["bridge", "tunnel", "covered", "embankment", "causeway"].into_iter().any(set)
            && tags.get("layer").is_none_or(|l| l.trim() == "0")
            && !matches!(
                tags.get("man_made").map(String::as_str),
                Some("dyke" | "embankment" | "pier" | "breakwater" | "groyne")
            )
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

/// Centre points of a road on a crossing to drown: a short run of `wet` points (columns the
/// carve fills) with dry road at both ends and water on both sides of the road.
pub fn drowned_points(
    points: &[(i32, i32)],
    half_width: i32,
    wet: impl Fn(i32, i32) -> bool,
) -> Vec<bool> {
    let n = points.len();
    let mut drowned = vec![false; n];
    let is_wet: Vec<bool> = points.iter().map(|&(x, z)| wet(x, z)).collect();
    let mut i = 0;
    while i < n {
        if !is_wet[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && is_wet[i] {
            i += 1;
        }
        // A run that reaches the way's end is a pier, a ramp or the area's edge.
        if start < DRY_ENDS || i + DRY_ENDS > n || i - start > MAX_DROWNED_RUN {
            continue;
        }
        let both_sides = (start..i)
            .filter(|&k| {
                let (ax, az) = points[k.saturating_sub(2)];
                let (bx, bz) = points[(k + 2).min(n - 1)];
                let (dx, dz) = (f64::from(bx - ax), f64::from(bz - az));
                let len = dx.hypot(dz).max(1.0);
                let reach = f64::from(half_width + 2);
                let (px, pz) = (
                    (-dz / len * reach).round() as i32,
                    (dx / len * reach).round() as i32,
                );
                let (x, z) = points[k];
                wet(x + px, z + pz) && wet(x - px, z - pz)
            })
            .count();
        if both_sides * 3 >= (i - start) * 2 {
            drowned[start..i].fill(true);
        }
    }
    drowned
}

/// What the water options add to the estimate of the deepest carve the datum leaves room for.
#[derive(Clone, Copy, Debug, Default)]
pub struct CarveDepth {
    pub floor: i32,
    pub detail: WaterDetail,
    pub scale: f64,
}

impl CarveDepth {
    pub const NONE: Self = Self {
        floor: 0,
        detail: WaterDetail::Default,
        scale: 1.0,
    };

    /// Deepest carve where a tunnel crosses water the default bed digs `estimate` deep.
    pub fn crossing(self, estimate: i32) -> i32 {
        self.detail
            .max_carve_depth(estimate, self.scale)
            .max(self.floor)
    }

    pub fn bound(self, estimate: i32) -> i32 {
        // No water in the land cover: only a river bed, dug without it, needs room.
        if estimate == 0 {
            return self.floor;
        }
        self.detail
            .max_carve_depth(estimate, self.scale)
            .max(self.floor)
    }
}

/// Bowl depth for one cell: a block deeper per block inward past a 1-block shoal, up to a
/// target set by the body's half-width. Distances are chamfer-3-4 units (3 per block).
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
        assert_eq!(CarveDepth::NONE.crossing(3), 3);
    }

    #[test]
    fn scaled_gates_follow_scale() {
        let s = WaterDetail::Scaled;
        assert!(s.uses_bowl(0.25) && !s.uses_bowl(0.5));
        assert_eq!(s.channel_width_cap(0.5), Some(3));
        assert_eq!(s.channel_width_cap(1.0), None);
        let tags = HashMap::new();
        assert!(s.drowns_crossing(0.5, &tags) && !s.drowns_crossing(0.6, &tags));
        for key in ["bridge", "tunnel", "embankment", "layer"] {
            let mut tags = HashMap::new();
            tags.insert(key.to_string(), "1".to_string());
            assert!(!s.drowns_crossing(0.25, &tags), "{key}");
        }
        assert_eq!(s.max_carve_depth(3, 1.0), 4);
        assert_eq!(s.max_carve_depth(6, 1.0), 6);
        assert_eq!(s.max_carve_depth(0, 0.25), BOWL_MAX_DEPTH);
    }

    /// A road along x at z = 0, over water where `wet` says so.
    fn drowned_along_x(len: i32, wet: impl Fn(i32, i32) -> bool) -> Vec<bool> {
        let road: Vec<(i32, i32)> = (0..len).map(|x| (x, 0)).collect();
        drowned_points(&road, 1, wet)
    }

    #[test]
    fn only_a_short_crossing_with_water_on_both_sides_is_drowned() {
        // A 6-block river across the road.
        let river = |x: i32, _z: i32| (20..26).contains(&x);
        let d = drowned_along_x(60, river);
        assert_eq!(d.iter().filter(|&&b| b).count(), 6);
        assert!(d[20] && d[25] && !d[19] && !d[26]);

        // A causeway through a lake: too long.
        let lake = |x: i32, _z: i32| (10..50).contains(&x);
        assert!(!drowned_along_x(60, lake).contains(&true));

        // A quay or a dam: water under the road and on one side only.
        let quay = |x: i32, z: i32| (20..26).contains(&x) && z <= 0;
        assert!(!drowned_along_x(60, quay).contains(&true));

        // A road that ends in the water: a ramp, a pier, the edge of the area.
        let end = |x: i32, _z: i32| x >= 54;
        assert!(!drowned_along_x(60, end).contains(&true));
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
    fn no_water_leaves_room_for_a_river_bed_only() {
        for detail in [WaterDetail::Default, WaterDetail::Scaled] {
            for scale in [0.25, 1.0] {
                for floor in [0, crate::water_depth::MAX_WATER_DEPTH] {
                    let c = CarveDepth {
                        floor,
                        detail,
                        scale,
                    };
                    assert_eq!(c.bound(0), floor, "{detail:?} {scale} {floor}");
                }
            }
        }
        // With water the floor and the +1 still apply, and a tunnel counts with them.
        let c = CarveDepth {
            floor: 6,
            detail: WaterDetail::Scaled,
            scale: 1.0,
        };
        assert_eq!(c.bound(2), 6);
        assert_eq!(c.crossing(0), 6);
        let rounded = CarveDepth { floor: 0, ..c };
        assert_eq!(rounded.crossing(3), 4);
    }

    #[test]
    fn default_leaves_the_carve_alone() {
        use clap::Parser;
        let args =
            crate::args::Args::parse_from(["arnis", "--output-dir", ".", "--bbox", "1,2,3,4"]);
        assert_eq!(args.water.water_detail, WaterDetail::Default);
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
