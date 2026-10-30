//! Moon and Mars worlds: fixed low scale, PDS elevation, no OpenStreetMap.
//!
//! geo_distance always uses Earth's radius, and both haversines scale linearly
//! with it, so the error is one uniform factor on both axes and cancels in
//! `scale`. No coordinate-system code needs to know about bodies.

use crate::block_definitions::{
    Block, ANDESITE, BASALT, BLACKSTONE, BROWN_TERRACOTTA, DIRT, GRANITE, GRASS_BLOCK, GRAVEL,
    LIGHT_GRAY_CONCRETE_POWDER, ORANGE_TERRACOTTA, PACKED_ICE, RED_TERRACOTTA, SAND,
    SANDSTONE, SMOOTH_BASALT, SMOOTH_RED_SANDSTONE, SNOW_BLOCK, STONE, TERRACOTTA,
    WHITE_CONCRETE, WHITE_TERRACOTTA,
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
    ground_y: i32,
    x: i32,
    z: i32,
    features: u16,
    convexity: f64,
) -> (Block, Block) {
    match body {
        CelestialBody::Earth => (GRASS_BLOCK, DIRT),
        // Always stone underneath, so gravel and powder never fall.
        CelestialBody::Moon => (
            moon_surface(
                slope,
                crate::land_cover::coord_hash(x, z),
                x,
                z,
                features,
                convexity,
            ),
            STONE,
        ),
        CelestialBody::Mars => mars_palette(
            slope,
            ground_y,
            crate::land_cover::coord_hash(x, z),
            x,
            z,
            features,
            convexity,
        ),
    }
}

/// Grey highland regolith, mapped basaltic maria, and rocky crater/ridge relief.
fn moon_surface(slope: i32, h: u64, x: i32, z: i32, features: u16, convexity: f64) -> Block {
    if slope > 6 {
        return match h % 10 {
            0..=4 => ANDESITE,
            5..=7 => STONE,
            _ => GRAVEL,
        };
    }

    if features & crate::planetary_geology::MOON_RILLE != 0 && convexity > 0.05 {
        return SMOOTH_BASALT;
    }

    if features & crate::planetary_geology::MOON_MARE != 0 {
        return match h % 10 {
            0..=5 => BASALT,
            6..=8 => SMOOTH_BASALT,
            _ => ANDESITE,
        };
    }

    if features
        & (crate::planetary_geology::MOON_CRATER_RIM | crate::planetary_geology::MOON_RIDGE)
        != 0
        || convexity < -0.25
    {
        return match h % 10 {
            0..=4 => ANDESITE,
            5..=7 => STONE,
            _ => GRAVEL,
        };
    }

    // Noise pools gravel and rock into patches instead of per-block static.
    let patch = crate::ground_generation::value_noise_01(x, z, 7);
    if patch > 0.72 {
        return match h % 10 {
            0..=6 => GRAVEL,
            7..=8 => ANDESITE,
            _ => LIGHT_GRAY_CONCRETE_POWDER,
        };
    }
    if patch < 0.24 {
        return match h % 10 {
            0..=5 => ANDESITE,
            6..=8 => STONE,
            _ => GRAVEL,
        };
    }
    match h % 20 {
        0..=13 => LIGHT_GRAY_CONCRETE_POWDER,
        14..=16 => GRAVEL,
        17..=18 => ANDESITE,
        _ => STONE,
    }
}

/// Rusty dust over red sub-surface with mapped volcanic, dune, fluvial, and
/// polar units. The elevation grid supplies relief; flags tune materials only.
fn mars_palette(
    slope: i32,
    ground_y: i32,
    h: u64,
    x: i32,
    z: i32,
    features: u16,
    convexity: f64,
) -> (Block, Block) {
    if features & crate::planetary_geology::MARS_POLAR_CAP != 0 && slope <= 4 {
        return match h % 12 {
            0..=5 => (SNOW_BLOCK, PACKED_ICE),
            6..=8 => (PACKED_ICE, WHITE_TERRACOTTA),
            9..=10 => (WHITE_CONCRETE, PACKED_ICE),
            _ => (WHITE_TERRACOTTA, PACKED_ICE),
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

    if features & crate::planetary_geology::MARS_DUNE != 0 && slope <= 6 {
        return match h % 12 {
            0..=5 => (SMOOTH_RED_SANDSTONE, RED_TERRACOTTA),
            6..=9 => (SANDSTONE, RED_TERRACOTTA),
            _ => (SAND, SANDSTONE),
        };
    }

    if features & crate::planetary_geology::MARS_VOLCANIC != 0 && slope <= 6 {
        return match h % 10 {
            0..=4 => (BASALT, BLACKSTONE),
            5..=7 => (SMOOTH_BASALT, BASALT),
            _ => (BLACKSTONE, BASALT),
        };
    }

    if features & (crate::planetary_geology::MARS_FLUVIAL | crate::planetary_geology::MARS_RILLE)
        != 0
        && slope <= 6
    {
        if convexity > 0.05 {
            return (GRAVEL, SANDSTONE);
        }
        return match h % 8 {
            0..=3 => (SANDSTONE, RED_TERRACOTTA),
            4..=6 => (SMOOTH_RED_SANDSTONE, RED_TERRACOTTA),
            _ => (GRAVEL, SANDSTONE),
        };
    }

    if features & crate::planetary_geology::MARS_POLAR_LAYER != 0 && slope <= 4 {
        return match ground_y.rem_euclid(8) {
            0..=2 => (WHITE_TERRACOTTA, RED_TERRACOTTA),
            3..=5 => (TERRACOTTA, RED_TERRACOTTA),
            _ => (WHITE_TERRACOTTA, TERRACOTTA),
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

/// Common planetary ground blocks, kept in sync with `surface_palette` so the
/// storage regression test covers every branch that can become bulk terrain.
#[cfg(test)]
pub const PLANETARY_SURFACE_BLOCKS: &[Block] = &[
    ANDESITE,
    BASALT,
    BLACKSTONE,
    BROWN_TERRACOTTA,
    GRANITE,
    GRAVEL,
    LIGHT_GRAY_CONCRETE_POWDER,
    ORANGE_TERRACOTTA,
    PACKED_ICE,
    RED_TERRACOTTA,
    SAND,
    SANDSTONE,
    SMOOTH_BASALT,
    SMOOTH_RED_SANDSTONE,
    SNOW_BLOCK,
    STONE,
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

    #[test]
    fn mapped_maria_use_basalt_while_highlands_keep_the_regolith_palette() {
        let mare = surface_palette(
            CelestialBody::Moon,
            0,
            0,
            0,
            0,
            crate::planetary_geology::MOON_MARE,
            0.0,
        );
        assert!([BASALT, SMOOTH_BASALT, ANDESITE].contains(&mare.0));
        assert_eq!(mare.1, STONE);

        let highland = surface_palette(CelestialBody::Moon, 0, 0, 0, 0, 0, 0.0);
        assert!(![BASALT, SMOOTH_BASALT].contains(&highland.0));
    }

    #[test]
    fn mapped_mars_units_choose_distinct_deterministic_palettes() {
        let volcanic = surface_palette(
            CelestialBody::Mars,
            0,
            0,
            0,
            0,
            crate::planetary_geology::MARS_VOLCANIC,
            0.0,
        );
        assert!([BASALT, SMOOTH_BASALT, BLACKSTONE].contains(&volcanic.0));

        let dune = surface_palette(
            CelestialBody::Mars,
            0,
            0,
            0,
            0,
            crate::planetary_geology::MARS_DUNE,
            0.0,
        );
        assert!([SAND, SANDSTONE, SMOOTH_RED_SANDSTONE].contains(&dune.0));

        let fluvial = surface_palette(
            CelestialBody::Mars,
            0,
            0,
            0,
            0,
            crate::planetary_geology::MARS_FLUVIAL,
            0.0,
        );
        assert!([SANDSTONE, SMOOTH_RED_SANDSTONE, GRAVEL].contains(&fluvial.0));

        let channel_depression = surface_palette(
            CelestialBody::Mars,
            0,
            0,
            0,
            0,
            crate::planetary_geology::MARS_FLUVIAL,
            0.2,
        );
        assert_eq!(channel_depression, (GRAVEL, SANDSTONE));
    }

    #[test]
    fn martian_ice_blocks_require_the_mapped_polar_cap_flag() {
        let mapped_cap = surface_palette(
            CelestialBody::Mars,
            0,
            0,
            0,
            0,
            crate::planetary_geology::MARS_POLAR_CAP,
            0.0,
        );
        assert!([SNOW_BLOCK, PACKED_ICE, WHITE_CONCRETE, WHITE_TERRACOTTA]
            .contains(&mapped_cap.0));

        let fallback = surface_palette(CelestialBody::Mars, 0, 0, 0, 0, 0, 0.0);
        assert!(![SNOW_BLOCK, PACKED_ICE, WHITE_CONCRETE, WHITE_TERRACOTTA]
            .contains(&fallback.0));
    }
}

