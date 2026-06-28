//! Climate axis: a bundled Koppen grid, sampled once per generation, drives arid/polar surfaces and biomes; temperate is unchanged.

use crate::block_definitions::*;
use crate::coordinate_system::geographic::LLBBox;
use crate::land_cover::{
    coord_hash, LC_BARE, LC_CROPLAND, LC_GRASSLAND, LC_MOSS, LC_SHRUBLAND, LC_SNOW_ICE,
    LC_TREE_COVER,
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

    fn is_arid(self) -> bool {
        matches!(
            self,
            Climate::HotDesert | Climate::HotSteppe | Climate::ColdDesert | Climate::ColdSteppe
        )
    }

    /// Surface palette (surface, under) for veg/bare cover, or None to keep the baseline.
    pub fn surface_palette(self, cover: u8, x: i32, z: i32) -> Option<(Block, Block)> {
        // DryContinental (Grand Canyon) keeps baseline blocks; only its biome is adapted.
        if matches!(
            self,
            Climate::Temperate | Climate::TropicalSavanna | Climate::DryContinental
        ) {
            return None;
        }
        let veg = matches!(
            cover,
            LC_TREE_COVER | LC_SHRUBLAND | LC_GRASSLAND | LC_CROPLAND | LC_MOSS
        );
        let bare = cover == LC_BARE || cover == LC_SNOW_ICE;
        if !veg && !bare {
            return None;
        }
        let h = coord_hash(x, z);
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
            Climate::HotSteppe => match h % 10 {
                0..=2 => (SAND, SANDSTONE),
                3..=5 => (COARSE_DIRT, DIRT),
                _ => (GRASS_BLOCK, DIRT),
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
            Climate::Temperate | Climate::TropicalSavanna | Climate::DryContinental => return None,
        };
        Some(pal)
    }

    /// Arid steep-terrain palette (tan canyon walls); None for non-arid.
    pub fn slope_palette(self, x: i32, z: i32) -> Option<(Block, Block)> {
        if !self.is_arid() {
            return None;
        }
        let pal = match coord_hash(x, z) % 12 {
            0..=4 => (SANDSTONE, SANDSTONE),
            5..=7 => (SMOOTH_SANDSTONE, SANDSTONE),
            8..=9 => (ORANGE_TERRACOTTA, TERRACOTTA),
            _ => (TERRACOTTA, TERRACOTTA),
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
    fn temperate_never_overrides() {
        assert!(Climate::Temperate.surface_palette(LC_BARE, 1, 2).is_none());
        assert!(Climate::Temperate.slope_palette(1, 2).is_none());
    }

    #[test]
    fn desert_overrides_to_sand() {
        let (s, _) = Climate::HotDesert
            .surface_palette(LC_GRASSLAND, 7, 7)
            .unwrap();
        assert!(matches!(s, SAND | SANDSTONE | SMOOTH_SANDSTONE));
        assert!(Climate::HotDesert.slope_palette(3, 4).is_some());
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
