//! Climate axis: a bundled Koppen grid, sampled once per generation, drives arid/polar surfaces and biomes; temperate is unchanged.

use crate::block_definitions::*;
use crate::coordinate_system::geographic::LLBBox;
use crate::geo_grid::TiledGrid;
use crate::ground_generation::{ground_noise, patch_noise, value_noise_salted};
use crate::land_cover::{
    LC_BARE, LC_CROPLAND, LC_GRASSLAND, LC_MOSS, LC_SHRUBLAND, LC_SNOW_ICE, LC_TREE_COVER,
};
use std::sync::{LazyLock, OnceLock};

// Global Koppen-Geiger grid, 0.1 deg, class 1..30 per cell (0 = ocean/nodata).
static KOPPEN_BYTES: &[u8] = include_bytes!("../assets/climate/koppen.grid");
static KOPPEN: LazyLock<Option<TiledGrid>> = LazyLock::new(|| TiledGrid::parse(KOPPEN_BYTES));
// Decoded tiles stay cached; each is 10 KB and a run touches one or two.
type KoppenTile = OnceLock<Option<Box<[u8]>>>;
static KOPPEN_TILES: LazyLock<Vec<KoppenTile>> = LazyLock::new(|| {
    let tiles = KOPPEN.as_ref().map_or(0, TiledGrid::tile_count);
    (0..tiles).map(|_| OnceLock::new()).collect()
});

fn koppen_class(lat: f64, lon: f64) -> u8 {
    let Some(grid) = KOPPEN.as_ref() else {
        return 0;
    };
    let (col, row) = grid.position(lat, lon);
    let cell = grid.cell(col, row);
    let Some(slot) = KOPPEN_TILES.get(grid.tile_of(cell)) else {
        return 0;
    };
    let tile = slot.get_or_init(|| grid.decode(grid.tile_of(cell)).map(Vec::into_boxed_slice));
    tile.as_deref()
        .map_or(0, |t| grid.value(t, grid.index_in_tile(cell)) as u8)
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
    /// Deserts, steppes and dry-summer continental climates.
    pub fn is_dry(self) -> bool {
        matches!(
            self,
            Climate::HotDesert
                | Climate::ColdDesert
                | Climate::HotSteppe
                | Climate::ColdSteppe
                | Climate::DryContinental
        )
    }

    fn from_class(c: u8) -> Climate {
        match c {
            3 => Climate::TropicalSavanna,                  // Aw
            4 => Climate::HotDesert,                        // BWh
            5 => Climate::ColdDesert,                       // BWk
            6 => Climate::HotSteppe,                        // BSh
            7 => Climate::ColdSteppe,                       // BSk
            17 | 18 => Climate::DryContinental,             // Dsa/Dsb
            19 | 20 | 23 | 24 | 27 | 28 => Climate::Boreal, // Dsc/Dsd, Dwc/Dwd, Dfc/Dfd
            29 => Climate::Tundra,                          // ET
            30 => Climate::IceCap,                          // EF
            _ => Climate::Temperate,                        // Af/Am, C*, Dfa/Dfb, Dwa/Dwb, 0
        }
    }

    /// Sample the climate at the bbox center (one lookup per generation).
    pub fn classify(bbox: &LLBBox) -> Climate {
        let lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
        let lon = (bbox.min().lng() + bbox.max().lng()) / 2.0;
        Climate::from_class(koppen_class(lat, lon))
    }

    /// Surface palette (surface, under) for veg/bare cover, or None to keep the baseline.
    /// `red` asks for red earth, which only dry climates take.
    pub fn surface_palette(self, cover: u8, x: i32, z: i32, red: bool) -> Option<(Block, Block)> {
        let veg = matches!(
            cover,
            LC_TREE_COVER | LC_SHRUBLAND | LC_GRASSLAND | LC_CROPLAND | LC_MOSS
        );
        let bare = cover == LC_BARE || cover == LC_SNOW_ICE;
        if !veg && !bare {
            return None;
        }
        // Shares along one irregular field, so each material forms patches and the
        // listed order is also the order they grade into one another.
        let n = ground_noise(x, z, 9, 0x00C1_1A7E);
        let pick = |bands: &[(f64, (Block, Block))]| {
            let (surface, under) = bands
                .iter()
                .find(|(upto, _)| n < *upto)
                .map_or(bands[bands.len() - 1].1, |(_, p)| *p);
            (speckle(surface, x, z), under)
        };
        if red && cover != LC_SNOW_ICE {
            return Some(if bare {
                pick(&[
                    (0.55, (RED_SAND, RED_SANDSTONE)),
                    (0.75, (TERRACOTTA, RED_SANDSTONE)),
                    (0.87, (COARSE_DIRT, DIRT)),
                    (1.0, (RED_SANDSTONE, RED_SANDSTONE)),
                ])
            } else {
                pick(&[
                    (0.5, (RED_SAND, RED_SANDSTONE)),
                    (0.8, (COARSE_DIRT, DIRT)),
                    (0.9, (GRASS_BLOCK, DIRT)),
                    (1.0, (TERRACOTTA, RED_SANDSTONE)),
                ])
            });
        }
        let pal = match self {
            // DryContinental (Grand Canyon) keeps baseline blocks; only its biome is adapted.
            Climate::Temperate | Climate::TropicalSavanna | Climate::DryContinental => return None,
            Climate::IceCap => {
                if patch_noise(x, z, 20, 0x001C_ECA9) < 0.17 {
                    (PACKED_ICE, PACKED_ICE)
                } else {
                    (SNOW_BLOCK, SNOW_BLOCK)
                }
            }
            Climate::HotDesert => {
                // Sandstone crops out in patches; per-block sandstone read as speckle.
                if ground_noise(x, z, 12, 0x00DE_5E27) < 0.06 {
                    (SANDSTONE, SANDSTONE)
                } else {
                    (speckle(SAND, x, z), SANDSTONE)
                }
            }
            Climate::HotSteppe if bare => {
                pick(&[(0.35, (SAND, SANDSTONE)), (1.0, (COARSE_DIRT, DIRT))])
            }
            Climate::HotSteppe => pick(&[
                (0.15, (SAND, SANDSTONE)),
                (0.5, (COARSE_DIRT, DIRT)),
                (1.0, (GRASS_BLOCK, DIRT)),
            ]),
            Climate::ColdDesert if bare => pick(&[
                (0.42, (GRAVEL, STONE)),
                (0.75, (COARSE_DIRT, DIRT)),
                (1.0, (STONE, STONE)),
            ]),
            Climate::ColdDesert => pick(&[
                (0.5, (COARSE_DIRT, DIRT)),
                (0.8, (GRAVEL, STONE)),
                (1.0, (GRASS_BLOCK, DIRT)),
            ]),
            Climate::ColdSteppe if bare => {
                pick(&[(0.6, (COARSE_DIRT, DIRT)), (1.0, (GRAVEL, STONE))])
            }
            Climate::ColdSteppe => pick(&[(0.3, (COARSE_DIRT, DIRT)), (1.0, (GRASS_BLOCK, DIRT))]),
            Climate::Boreal if bare => pick(&[(0.5, (COARSE_DIRT, DIRT)), (1.0, (GRAVEL, STONE))]),
            Climate::Boreal => pick(&[
                (0.4, (PODZOL, DIRT)),
                (0.6, (COARSE_DIRT, DIRT)),
                (1.0, (GRASS_BLOCK, DIRT)),
            ]),
            Climate::Tundra if bare => pick(&[
                (0.5, (GRAVEL, STONE)),
                (0.8, (COARSE_DIRT, DIRT)),
                (1.0, (STONE, STONE)),
            ]),
            Climate::Tundra => pick(&[
                (0.4, (COARSE_DIRT, DIRT)),
                (0.6, (MOSS_BLOCK, DIRT)),
                (1.0, (GRASS_BLOCK, DIRT)),
            ]),
        };
        Some(pal)
    }
}

/// Small clumps of a similar block inside a patch, so it is not one flat texture.
pub(crate) fn speckle(block: Block, x: i32, z: i32) -> Block {
    // Raw value noise: its tails below 0.21 and above 0.85 hold about a tenth each.
    let n = value_noise_salted(x, z, 2, 0x5EC1_7A3B);
    match block {
        GRAVEL if n < 0.21 => ANDESITE,
        GRAVEL if n > 0.86 => COBBLESTONE,
        STONE if n < 0.23 => ANDESITE,
        STONE if n > 0.85 => TUFF,
        COARSE_DIRT if n < 0.21 => DIRT,
        SAND if n > 0.89 => SMOOTH_SANDSTONE,
        RED_SAND if n < 0.17 => TERRACOTTA,
        RED_SAND if n > 0.87 => RED_SANDSTONE,
        TERRACOTTA if n > 0.84 => ORANGE_TERRACOTTA,
        _ => block,
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

        // Monsoon summers are wet (Seoul, Dwa); only summer-dry Ds* counts as dry.
        assert_eq!(Climate::from_class(21), Climate::Temperate);
        assert_eq!(Climate::from_class(18), Climate::DryContinental);
    }

    #[test]
    fn temperate_never_overrides() {
        assert!(Climate::Temperate
            .surface_palette(LC_BARE, 1, 2, false)
            .is_none());
    }

    #[test]
    fn desert_overrides_to_sand() {
        let (s, _) = Climate::HotDesert
            .surface_palette(LC_GRASSLAND, 7, 7, false)
            .unwrap();
        assert!(matches!(s, SAND | SANDSTONE | SMOOTH_SANDSTONE));
    }

    #[test]
    fn red_earth_is_red_sand_country() {
        let mut red = 0;
        for x in 0..40 {
            for z in 0..40 {
                let (s, _) = Climate::ColdDesert
                    .surface_palette(LC_BARE, x * 7, z * 7, true)
                    .unwrap();
                assert!(!matches!(s, GRAVEL | STONE | SAND), "{s:?}");
                red += usize::from(matches!(s, RED_SAND | RED_SANDSTONE | TERRACOTTA));
            }
        }
        assert!(red > 1100, "{red} of 1600 red");
    }

    #[test]
    fn embedded_grid_parses() {
        // If this fails the embedded grid is wrong; koppen_class then safely returns 0.
        let grid = KOPPEN.as_ref().expect("koppen.grid");
        assert_eq!(grid.cells_per_degree().round(), 10.0);
        assert_eq!(KOPPEN_TILES.len(), 648);
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
