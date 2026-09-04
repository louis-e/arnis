//! Moon and Mars worlds: fixed low scale, PDS elevation, no OpenStreetMap.
//!
//! geo_distance always uses Earth's radius, and both haversines scale linearly
//! with it, so the error is one uniform factor on both axes and cancels in
//! `scale`. No coordinate-system code needs to know about bodies.

use crate::block_definitions::{
    Block, BROWN_TERRACOTTA, DIRT, END_STONE, GRANITE, GRASS_BLOCK, GRAVEL, ORANGE_TERRACOTTA,
    RED_TERRACOTTA, SNOW_BLOCK, TERRACOTTA, WHITE_CONCRETE, WHITE_TERRACOTTA,
};
use clap::ValueEnum;

/// The radius baked into `geo_distance`.
pub const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// Vanilla build headroom: MAX_Y 319 - TERRAIN_HEIGHT_BUFFER 15 - ground level -62.
const VANILLA_HEADROOM_BLOCKS: f64 = 366.0;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum CelestialBody {
    #[default]
    Earth,
    Moon,
    Mars,
}

impl CelestialBody {
    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "moon" | "luna" => Self::Moon,
            "mars" => Self::Mars,
            _ => Self::Earth,
        }
    }

    /// Capitalised name, used in world titles.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Earth => "Earth",
            Self::Moon => "Moon",
            Self::Mars => "Mars",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Earth => "earth",
            Self::Moon => "moon",
            Self::Mars => "mars",
        }
    }

    #[inline(always)]
    pub fn is_earth(self) -> bool {
        matches!(self, Self::Earth)
    }

    pub fn radius_m(self) -> f64 {
        match self {
            Self::Earth => EARTH_RADIUS_M,
            // LOLA reference sphere.
            Self::Moon => 1_737_400.0,
            // MOLA reference ellipsoid (spherical for our purposes).
            Self::Mars => 3_396_000.0,
        }
    }

    /// Metres per block, fixed per body and deliberately coarse so a careless
    /// whole-planet selection still finishes. Craters stay readable because
    /// `vertical_exaggeration` keeps relief-to-width constant at any ground scale.
    pub fn meters_per_block(self) -> f64 {
        match self {
            Self::Earth => 1.0,
            Self::Moon => 200.0,
            Self::Mars => 500.0,
        }
    }

    /// Correction for `geo_distance` overstating this body's ground distances.
    #[inline(always)]
    pub fn scale_ratio(self) -> f64 {
        self.radius_m() / EARTH_RADIUS_M
    }

    /// Cancels the horizontal correction on the vertical axis, so metres map 1:1.
    #[inline(always)]
    pub fn height_gain(self) -> f64 {
        EARTH_RADIUS_M / self.radius_m()
    }

    /// 1:1 relief is unreadable here: Copernicus is a 1:50 dish, so it renders as
    /// a faint dent. 4x brings that to ~1:12 and still fits vanilla build height.
    pub fn vertical_exaggeration(self) -> f64 {
        match self {
            Self::Earth => 1.0,
            Self::Moon | Self::Mars => 4.0,
        }
    }

    /// What the provider multiplies raw metres by: correction times exaggeration.
    #[inline(always)]
    pub fn terrain_gain(self) -> f64 {
        self.height_gain() * self.vertical_exaggeration()
    }

    /// Blocks per real metre, radius-corrected. Below `OBJECT_SKIP_SCALE`, so OSM
    /// and Overture skip themselves.
    pub fn world_scale(self) -> f64 {
        if self.is_earth() {
            return 1.0;
        }
        self.scale_ratio() / self.meters_per_block()
    }

    /// Real relief that fits vanilla build height uncompressed; beyond it the
    /// scaler compresses, so the world still fits without a datapack.
    pub fn vanilla_relief_headroom_m(self) -> f64 {
        VANILLA_HEADROOM_BLOCKS * self.meters_per_block() / self.vertical_exaggeration()
    }

    /// Biome for every chunk. Both are rainless and bare; badlands also tints the
    /// Mars sky the right rusty orange.
    pub fn biome(self) -> &'static str {
        match self {
            Self::Earth => "minecraft:plains",
            Self::Moon => "minecraft:stony_peaks",
            Self::Mars => "minecraft:badlands",
        }
    }
}

/// Surface and sub-surface block for a column. `slope` uses the same tiers as the
/// Earth path in `ground_generation`, where `slope = 8 * tan`.
pub fn surface_palette(
    body: CelestialBody,
    slope: i32,
    lat_deg: f64,
    ground_y: i32,
    x: i32,
    z: i32,
) -> (Block, Block) {
    match body {
        CelestialBody::Earth => (GRASS_BLOCK, DIRT),
        // Closest vanilla has to regolith, and one material keeps a body-sized
        // world uniform enough to stay cheap.
        CelestialBody::Moon => (END_STONE, END_STONE),
        CelestialBody::Mars => mars_palette(
            slope,
            lat_deg,
            ground_y,
            crate::land_cover::coord_hash(x, z),
            x,
            z,
        ),
    }
}

/// Rusty dust over red sub-surface, with banded scarps and polar caps.
fn mars_palette(slope: i32, lat_deg: f64, ground_y: i32, h: u64, x: i32, z: i32) -> (Block, Block) {
    if lat_deg.abs() > MARS_POLAR_CAP_LAT && slope <= 4 {
        return match h % 12 {
            0..=6 => (SNOW_BLOCK, WHITE_TERRACOTTA),
            7..=9 => (WHITE_CONCRETE, WHITE_TERRACOTTA),
            _ => (WHITE_TERRACOTTA, WHITE_TERRACOTTA),
        };
    }

    if slope > 8 {
        // Banding off Y makes a canyon wall read as layered rock, as real scarps do.
        return match ground_y.rem_euclid(9) {
            0..=3 => (RED_TERRACOTTA, RED_TERRACOTTA),
            4..=6 => (BROWN_TERRACOTTA, BROWN_TERRACOTTA),
            _ => (TERRACOTTA, TERRACOTTA),
        };
    }
    if slope > 6 {
        return match h % 20 {
            0..=10 => (TERRACOTTA, RED_TERRACOTTA),
            11..=15 => (GRANITE, RED_TERRACOTTA),
            _ => (RED_TERRACOTTA, RED_TERRACOTTA),
        };
    }
    if slope > 4 {
        return match h % 12 {
            0..=4 => (ORANGE_TERRACOTTA, RED_TERRACOTTA),
            5..=7 => (TERRACOTTA, RED_TERRACOTTA),
            8..=9 => (GRANITE, RED_TERRACOTTA),
            _ => (GRAVEL, RED_TERRACOTTA),
        };
    }

    // Noise pools the drifts into patches instead of per-block static.
    let drift = crate::ground_generation::value_noise_01(x, z, 6);
    match h % 20 {
        _ if drift > 0.72 => (TERRACOTTA, RED_TERRACOTTA),
        _ if drift < 0.26 => (RED_TERRACOTTA, RED_TERRACOTTA),
        0..=13 => (ORANGE_TERRACOTTA, RED_TERRACOTTA),
        14..=17 => (TERRACOTTA, RED_TERRACOTTA),
        _ => (BROWN_TERRACOTTA, RED_TERRACOTTA),
    }
}

/// Beyond this latitude Mars keeps a permanent cap.
const MARS_POLAR_CAP_LAT: f64 = 74.0;

/// Common planetary ground blocks, kept in sync with `surface_palette` so the
/// storage regression test covers every branch that can become bulk terrain.
#[cfg(test)]
pub const PLANETARY_SURFACE_BLOCKS: &[Block] = &[
    BROWN_TERRACOTTA,
    GRANITE,
    GRAVEL,
    ORANGE_TERRACOTTA,
    RED_TERRACOTTA,
    SNOW_BLOCK,
    TERRACOTTA,
    WHITE_CONCRETE,
    WHITE_TERRACOTTA,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::geographic::LLPoint;
    use crate::coordinate_system::transformation::geo_distance;

    /// The design rests on this: correcting `scale` reproduces true ground
    /// distances on both axes.
    #[test]
    fn scale_correction_recovers_true_ground_distance() {
        for body in [CelestialBody::Moon, CelestialBody::Mars] {
            let a = LLPoint::new(20.0, 340.0 - 360.0).unwrap();
            let b = LLPoint::new(20.5, 340.5 - 360.0).unwrap();
            let (earth_z, earth_x) = geo_distance(a, b);

            // What the body's own radius would give.
            let true_z = earth_z * body.scale_ratio();
            let true_x = earth_x * body.scale_ratio();

            let blocks_z = earth_z * body.world_scale();
            let blocks_x = earth_x * body.world_scale();

            assert!((blocks_z - true_z / body.meters_per_block()).abs() < 1e-6);
            assert!((blocks_x - true_x / body.meters_per_block()).abs() < 1e-6);
        }
    }

    /// Must cancel exactly, or Moon terrain comes out 3.67x too flat.
    #[test]
    fn height_gain_cancels_the_scale_correction() {
        for body in [CelestialBody::Moon, CelestialBody::Mars] {
            let real_relief_m = 3000.0;
            let served = real_relief_m * body.height_gain();
            let blocks = served * body.world_scale();
            assert!((blocks - real_relief_m / body.meters_per_block()).abs() < 1e-9);
        }
    }

    #[test]
    fn fixed_scales_skip_osm_objects() {
        for body in [CelestialBody::Moon, CelestialBody::Mars] {
            assert!(body.world_scale() < crate::args::OBJECT_SKIP_SCALE);
        }
    }

    /// Measured against the live archives: Copernicus spans 1862 m of relief,
    /// Melas Chasma about 9600 m. Both must fit uncompressed.
    #[test]
    fn vanilla_headroom_covers_ordinary_selections() {
        assert!(CelestialBody::Moon.vanilla_relief_headroom_m() > 3_000.0);
        assert!(CelestialBody::Mars.vanilla_relief_headroom_m() > 15_000.0);
    }

    /// Present, but not cartoonish.
    #[test]
    fn exaggeration_is_modest() {
        for body in [CelestialBody::Moon, CelestialBody::Mars] {
            let e = body.vertical_exaggeration();
            assert!((2.0..=5.0).contains(&e), "{body:?} exaggeration {e}");
            assert!((body.terrain_gain() - body.height_gain() * e).abs() < 1e-9);
        }
        assert_eq!(CelestialBody::Earth.vertical_exaggeration(), 1.0);
        assert_eq!(CelestialBody::Earth.terrain_gain(), 1.0);
    }
}
