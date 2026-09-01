//! Climate axis: a bundled Koppen grid, sampled once per generation, drives arid/polar surfaces and biomes; temperate is unchanged.

use crate::block_definitions::*;
use crate::coordinate_system::geographic::LLBBox;
use crate::land_cover::{
    coord_hash, LC_BARE, LC_CROPLAND, LC_GRASSLAND, LC_MOSS, LC_SHRUBLAND, LC_TREE_COVER,
};

// Global Koppen-Geiger grid, 0.1 deg, 1 byte/cell (class 1..30, 0 = ocean/nodata).
static KOPPEN: &[u8] = include_bytes!("../assets/climate/koppen_0p1.bin");
const KOPPEN_COLS: usize = 3600;
const KOPPEN_ROWS: usize = 1800;
const KOPPEN_RES: f64 = 0.1;

fn koppen_class(lat: f64, lon: f64) -> u8 {
    if KOPPEN.len() != KOPPEN_COLS * KOPPEN_ROWS {
        return 0;
    }
    let col = (((lon + 180.0) / KOPPEN_RES).floor() as isize).clamp(0, KOPPEN_COLS as isize - 1);
    let row = (((90.0 - lat) / KOPPEN_RES).floor() as isize).clamp(0, KOPPEN_ROWS as isize - 1);
    KOPPEN[row as usize * KOPPEN_COLS + col as usize]
}

/// Pick 0..n once per patch of roughly `size` blocks, warped so the patches
/// are not axis-aligned squares.
#[inline]
pub(crate) fn patch_pick(x: i32, z: i32, size: i32, n: u64) -> u64 {
    let w = coord_hash(x >> 2, z >> 2);
    let ox = (w % 7) as i32 - 3;
    let oz = ((w >> 8) % 7) as i32 - 3;
    let size = size.max(1);
    coord_hash((x + ox).div_euclid(size), (z + oz).div_euclid(size)) % n
}

/// Dryland character of the bbox. Koppen cannot separate grass steppe from
/// rock desert: Moab and the Kazakh steppe are both BSk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dryland {
    /// Not a dryland, or one whose own palette already suits it.
    None,
    /// Barren arid rock country: banded sedimentary strata, no turf.
    Rock,
}

/// Probed: Kazakh steppe 0.1%, Anatolian grassland 8.8%, Zion 20%, Moab 65%.
pub const BARE_FRACTION_DRY: f64 = 0.12;

/// Metres per km of bbox diagonal. Probed: Taklamakan 12.6 and Craters of the
/// Moon 12.2 below, Painted Desert 17.0 and Badlands NP 25.3 above.
pub const RELIEF_GRADIENT_ROCK: f64 = 15.0;

/// Above this a barren dissected bbox is alpine. Only altitude separates
/// Zard Kuh 2713 m and Damavand 4265 m from Zion 1297 m.
pub const MAX_ROCK_BASE_M: f64 = 2200.0;

/// Ds*/Dw* canyon country keeps scrub, so bare stays under half. Near-total
/// bare there is scree or volcanics: St Helens 0.71, Craters 0.94.
pub const DRY_CONTINENTAL_BARE_CEILING: f64 = 0.55;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Climate {
    /// C*, humid-continental D*, tropical rainforest, and ocean/nodata: existing behaviour.
    Temperate,
    TropicalSavanna,
    HotDesert,
    HotSteppe,
    ColdDesert,
    ColdSteppe,
    DryContinental,
    Boreal,
    Tundra,
    IceCap,
}

impl Climate {
    fn from_class(c: u8) -> Climate {
        match c {
            3 => Climate::TropicalSavanna,                  // Aw
            4 => Climate::HotDesert,                        // BWh
            5 => Climate::ColdDesert,                       // BWk
            6 => Climate::HotSteppe,                        // BSh
            7 => Climate::ColdSteppe,                       // BSk
            17 | 18 | 21 | 22 => Climate::DryContinental,   // Dsa/Dsb, Dwa/Dwb
            19 | 20 | 23 | 24 | 27 | 28 => Climate::Boreal, // Dsc/Dsd, Dwc/Dwd, Dfc/Dfd
            29 => Climate::Tundra,                          // ET
            30 => Climate::IceCap,                          // EF
            _ => Climate::Temperate,                        // Af/Am, C*, Dfa/Dfb, 0
        }
    }

    /// Sample the climate at the bbox center (one lookup per generation).
    pub fn classify(bbox: &LLBBox) -> Climate {
        let lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
        let lon = (bbox.min().lng() + bbox.max().lng()) / 2.0;
        Climate::from_class(koppen_class(lat, lon))
    }

    /// Bareness separates the Sahara from the Kazakh steppe, relief separates
    /// the Taklamakan from Monument Valley. Polar and temperate stay out.
    pub fn dryland(self, bare_fraction: f64, relief_gradient: f64, base_m: f64) -> Dryland {
        if bare_fraction < BARE_FRACTION_DRY {
            return Dryland::None;
        }
        // Hot desert keeps its own sand palette, which probing never beat.
        if matches!(self, Climate::HotDesert) {
            return Dryland::None;
        }
        // Higher than this and a barren dissected bbox is alpine scree.
        if base_m > MAX_ROCK_BASE_M || relief_gradient < RELIEF_GRADIENT_ROCK {
            return Dryland::None;
        }
        match self {
            Climate::DryContinental if bare_fraction >= DRY_CONTINENTAL_BARE_CEILING => {
                Dryland::None
            }
            Climate::ColdSteppe | Climate::ColdDesert | Climate::DryContinental => Dryland::Rock,
            _ => Dryland::None,
        }
    }

    /// Surface palette (surface, under) for veg/bare cover, or None to keep the baseline.
    pub fn surface_palette(self, cover: u8, x: i32, z: i32) -> Option<(Block, Block)> {
        // DryContinental keeps baseline blocks; only its biome is adapted.
        if matches!(self, Climate::Temperate | Climate::DryContinental) {
            return None;
        }
        // Overwriting cropland also stops the crop scatter firing, so Finnish
        // and Kazakh fields came out as bare ground.
        let veg = matches!(cover, LC_TREE_COVER | LC_SHRUBLAND | LC_GRASSLAND | LC_MOSS)
            || (cover == LC_CROPLAND
                && !matches!(
                    self,
                    Climate::Boreal | Climate::ColdSteppe | Climate::HotSteppe
                ));
        // Ice is handled in the cascade, which owns the isolated-pixel guard.
        let bare = cover == LC_BARE;
        if !veg && !bare {
            return None;
        }
        // Sand against its own sandstones is a small enough colour step to
        // draw per block. Every other mix would read as speckle.
        let h = if matches!(self, Climate::HotDesert) {
            coord_hash(x, z)
        } else {
            patch_pick(x, z, 9, 60)
        };
        let pal = match self {
            Climate::IceCap => {
                if h.is_multiple_of(6) {
                    (PACKED_ICE, PACKED_ICE)
                } else {
                    (SNOW_BLOCK, SNOW_BLOCK)
                }
            }
            Climate::HotDesert => match h % 12 {
                0 => (SANDSTONE, SANDSTONE),
                1 => (SMOOTH_SANDSTONE, SANDSTONE),
                _ => (SAND, SANDSTONE),
            },
            Climate::HotSteppe if bare => match h % 10 {
                0..=4 => (SAND, SANDSTONE),
                _ => (COARSE_DIRT, DIRT),
            },
            // A soil ramp instead: sand against coarse dirt is dE 0.37, and
            // sand carries no plants.
            Climate::HotSteppe => match h % 20 {
                0..=10 => (GRASS_BLOCK, DIRT),
                11..=16 => (COARSE_DIRT, DIRT),
                _ => (DIRT, DIRT),
            },
            Climate::ColdDesert if bare => match h % 12 {
                0..=4 => (GRAVEL, STONE),
                5..=8 => (COARSE_DIRT, DIRT),
                _ => (STONE, STONE),
            },
            Climate::ColdDesert => match h % 10 {
                0..=4 => (COARSE_DIRT, DIRT),
                5..=7 => (GRAVEL, STONE),
                _ => (GRASS_BLOCK, DIRT),
            },
            Climate::ColdSteppe if bare => match h % 10 {
                0..=5 => (COARSE_DIRT, DIRT),
                _ => (GRAVEL, STONE),
            },
            Climate::ColdSteppe => match h % 10 {
                0..=2 => (COARSE_DIRT, DIRT),
                _ => (GRASS_BLOCK, DIRT),
            },
            Climate::Boreal if bare => match h % 10 {
                0..=4 => (COARSE_DIRT, DIRT),
                _ => (GRAVEL, STONE),
            },
            Climate::Boreal => match h % 10 {
                0..=3 => (PODZOL, DIRT),
                4..=5 => (COARSE_DIRT, DIRT),
                _ => (GRASS_BLOCK, DIRT),
            },
            Climate::Tundra if bare => match h % 10 {
                0..=4 => (GRAVEL, STONE),
                5..=7 => (COARSE_DIRT, DIRT),
                _ => (STONE, STONE),
            },
            Climate::Tundra => match h % 10 {
                0..=3 => (COARSE_DIRT, DIRT),
                4..=5 => (MOSS_BLOCK, DIRT),
                _ => (GRASS_BLOCK, DIRT),
            },
            // Savanna dry season: soil shows between the tussocks. Keyed on the
            // column's own cover class, and every block passes the scatter gates.
            Climate::TropicalSavanna => match cover {
                LC_TREE_COVER => match h % 20 {
                    0..=15 => (GRASS_BLOCK, DIRT),
                    16..=17 => (COARSE_DIRT, DIRT),
                    _ => (DIRT, DIRT),
                },
                LC_GRASSLAND | LC_SHRUBLAND | LC_MOSS => match h % 20 {
                    0..=12 => (GRASS_BLOCK, DIRT),
                    13..=16 => (COARSE_DIRT, DIRT),
                    _ => (DIRT, DIRT),
                },
                _ => return None,
            },
            Climate::Temperate | Climate::DryContinental => return None,
        };
        Some(pal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_groups() {
        assert_eq!(Climate::from_class(4), Climate::HotDesert);
        assert_eq!(Climate::from_class(7), Climate::ColdSteppe);
        assert_eq!(Climate::from_class(30), Climate::IceCap);
        assert_eq!(Climate::from_class(15), Climate::Temperate); // Cfb
        assert_eq!(Climate::from_class(0), Climate::Temperate); // ocean
    }

    #[test]
    fn dryland_separates_the_probed_landscapes() {
        use Climate::*;
        // (climate, bare, gradient m/km, base elevation m, expected).
        // Every row is a real `--probe` measurement against a named place.
        let cases = [
            // Rock desert: the landforms this exists for.
            (DryContinental, 0.409, 137.9, 729.0, Dryland::Rock), // Grand Canyon
            (DryContinental, 0.200, 218.2, 1297.0, Dryland::Rock), // Zion
            (ColdDesert, 0.915, 105.7, 1579.0, Dryland::Rock),    // Monument Valley
            (ColdSteppe, 0.522, 48.2, 1365.0, Dryland::Rock),     // Moab / Arches
            (ColdSteppe, 0.281, 17.0, 1712.0, Dryland::Rock),     // Painted Desert
            (ColdSteppe, 0.843, 25.3, 827.0, Dryland::Rock),      // Badlands NP
            (ColdSteppe, 0.228, 42.6, 1069.0, Dryland::Rock),     // Cappadocia
            (ColdDesert, 0.731, 58.6, 873.0, Dryland::Rock),      // Petra
            (ColdDesert, 0.963, 118.3, 1643.0, Dryland::Rock),    // Sinai
            // Hot desert keeps its own sand palette, dissected or not.
            (HotDesert, 0.997, 24.0, 704.0, Dryland::None), // Erg Chebbi
            (HotDesert, 0.988, 37.5, 546.0, Dryland::None), // Sossusvlei
            (HotDesert, 0.917, 97.0, 900.0, Dryland::None), // Wadi Rum
            // Each is caught by a different gate.
            (DryContinental, 0.706, 140.5, 1128.0, Dryland::None), // St Helens pumice
            (ColdSteppe, 0.428, 60.3, 2432.0, Dryland::None),      // Great Sand Dunes
            (DryContinental, 0.935, 12.2, 1715.0, Dryland::None),  // Craters of the Moon
            (DryContinental, 1.000, 322.9, 4265.0, Dryland::None), // Damavand
            (DryContinental, 0.526, 211.4, 2713.0, Dryland::None), // Zard Kuh
            (ColdDesert, 1.000, 12.6, 1115.0, Dryland::None),      // Taklamakan sand sea
            (ColdDesert, 1.000, 0.0, 3654.0, Dryland::None),       // Salar de Uyuni
            (ColdSteppe, 1.000, 0.0, 903.0, Dryland::None),        // Lake Tuz salt pan
            (ColdSteppe, 0.001, 7.0, 440.0, Dryland::None),        // Kazakh steppe
            (ColdSteppe, 0.088, 0.6, 1000.0, Dryland::None),       // Anatolian grassland
            (Tundra, 0.035, 231.7, 1523.0, Dryland::None),         // Zermatt
            (Tundra, 0.998, 91.2, 700.0, Dryland::None),           // Askja basalt
            (IceCap, 0.000, 86.7, 100.0, Dryland::None),           // Antarctic dry valleys
            (Temperate, 0.000, 67.5, 192.0, Dryland::None),        // Yorkshire Dales
            (Temperate, 0.001, 5.3, 502.0, Dryland::None),         // Munich
            (HotDesert, 0.143, 7.3, 443.0, Dryland::None),         // Phoenix suburbs
        ];
        for (climate, bare, grad, base, want) in cases {
            assert_eq!(
                climate.dryland(bare, grad, base),
                want,
                "{climate:?} bare={bare} grad={grad} base={base}"
            );
        }
    }

    #[test]
    fn temperate_never_overrides() {
        assert!(Climate::Temperate.surface_palette(LC_BARE, 1, 2).is_none());
    }

    #[test]
    fn desert_overrides_to_sand() {
        let (s, _) = Climate::HotDesert
            .surface_palette(LC_GRASSLAND, 7, 7)
            .unwrap();
        assert!(matches!(s, SAND | SANDSTONE | SMOOTH_SANDSTONE));
    }

    #[test]
    fn clustering_preserves_the_mix_proportions() {
        // patch_pick's range is 60, which every arm's modulus divides, so the
        // move from a per-block hash to a per-patch one keeps the ratios.
        let mut grass = 0;
        let mut n = 0;
        for x in 0..600 {
            for z in 0..600 {
                if let Some((surf, _)) = Climate::ColdSteppe.surface_palette(LC_GRASSLAND, x, z) {
                    n += 1;
                    if surf == GRASS_BLOCK {
                        grass += 1;
                    }
                }
            }
        }
        let frac = grass as f64 / n as f64;
        assert!(
            (0.60..0.80).contains(&frac),
            "grass share drifted to {frac}"
        );
    }

    #[test]
    fn patches_are_contiguous_not_speckle() {
        // A per-block draw flips material on almost every step; a patch draw holds.
        let mut flips = 0;
        for x in 0..400 {
            let a = Climate::Tundra.surface_palette(LC_GRASSLAND, x, 7);
            let b = Climate::Tundra.surface_palette(LC_GRASSLAND, x + 1, 7);
            if a != b {
                flips += 1;
            }
        }
        assert!(flips < 80, "still speckling: {flips} flips over 400 blocks");
    }

    #[test]
    fn embedded_grid_size_matches() {
        // If this fails the embedded grid is wrong; koppen_class then safely returns 0.
        assert_eq!(KOPPEN.len(), KOPPEN_COLS * KOPPEN_ROWS);
    }

    #[test]
    fn classify_real_locations() {
        use crate::coordinate_system::geographic::LLBBox;
        let cases = [
            ("22.9,12.9,23.1,13.1", Climate::HotDesert),   // Sahara
            ("48.1,8.1,48.3,8.3", Climate::Temperate),     // Black Forest
            ("71.9,-40.1,72.1,-39.9", Climate::IceCap),    // Greenland
            ("-3.2,-60.1,-3.0,-59.9", Climate::Temperate), // Amazon (Af -> latitude jungle)
        ];
        for (bb, want) in cases {
            let bbox = LLBBox::from_str(bb).unwrap();
            assert_eq!(Climate::classify(&bbox), want, "bbox {bb}");
        }
    }
}
