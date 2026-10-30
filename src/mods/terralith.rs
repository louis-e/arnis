//! Terralith biomes for the land cover, climate and ecoregion Arnis already reads, for worlds
//! played with the Terralith datapack or mod (any recent version, no mod loader needed).
//!
//! Only biomes that fit the real place are used, and only where they snow when the vanilla
//! biome does: a colder Terralith biome would freeze lakes and pile snow where Arnis does not.

use crate::climate::Climate;
use crate::ecoregion::{EcoBiome, Ecoregion, Realm};
use crate::land_cover::{
    LC_CROPLAND, LC_GRASSLAND, LC_MOSS, LC_SHRUBLAND, LC_TREE_COVER, LC_WETLAND,
};

/// What Arnis knows about one 4x4 biome cell.
pub struct Cell {
    pub cover: u8,
    pub climate: Climate,
    pub eco: Option<Ecoregion>,
    pub water_dist: u8,
    pub slope: i32,
    pub lat: f64,
}

/// The Terralith biome for a cell Arnis gave `vanilla`, or `vanilla` where none fits better.
pub fn biome(vanilla: &'static str, c: &Cell) -> &'static str {
    use Climate::*;
    let hot = matches!(c.climate, HotDesert | HotSteppe | TropicalSavanna) || c.lat.abs() < 23.5;
    let realm = c.eco.map(|e| e.realm);
    let eco = c.eco.map(|e| e.biome);
    let open = matches!(c.cover, LC_GRASSLAND | LC_SHRUBLAND);
    match vanilla {
        "minecraft:warm_ocean" if c.water_dist >= 12 => "terralith:deep_warm_ocean",
        "minecraft:river" if hot => "terralith:warm_river",
        "minecraft:taiga" if c.climate == Boreal => match realm {
            Some(Realm::Nearctic) => "terralith:shield",
            _ => "terralith:birch_taiga",
        },
        "minecraft:taiga"
            if eco == Some(EcoBiome::TemperateConifer) && realm == Some(Realm::Nearctic) =>
        {
            "terralith:yosemite_lowlands"
        }
        "minecraft:snowy_plains" if matches!(c.climate, Tundra) && (open || c.cover == LC_MOSS) => {
            "terralith:cold_shrubland"
        }
        "minecraft:plains" if c.climate == ColdSteppe && open => "terralith:steppe",
        "minecraft:savanna" if c.climate == DryContinental && open => "terralith:steppe",
        "minecraft:savanna" if c.climate == HotSteppe => match c.cover {
            LC_SHRUBLAND => "terralith:hot_shrubland",
            LC_GRASSLAND => "terralith:brushland",
            _ => vanilla,
        },
        "minecraft:savanna" if c.cover == LC_SHRUBLAND && eco == Some(EcoBiome::Mediterranean) => {
            "terralith:shrubland"
        }
        "minecraft:savanna" if c.cover == LC_SHRUBLAND && c.climate == Temperate => {
            "terralith:highlands"
        }
        "minecraft:swamp" if hot => "terralith:orchid_swamp",
        "minecraft:desert" if c.climate == HotDesert => {
            if c.slope >= 3 {
                "terralith:desert_canyon"
            } else if matches!(
                c.cover,
                LC_TREE_COVER | LC_SHRUBLAND | LC_GRASSLAND | LC_CROPLAND | LC_WETLAND
            ) {
                // Green ground in a hot desert is an oasis or an irrigated valley.
                "terralith:desert_oasis"
            } else {
                vanilla
            }
        }
        _ => vanilla,
    }
}

/// Every Terralith biome Arnis writes, for the check against the datapack's own list.
#[cfg(test)]
pub const USED: [&str; 14] = [
    "deep_warm_ocean",
    "warm_river",
    "shield",
    "birch_taiga",
    "yosemite_lowlands",
    "cold_shrubland",
    "steppe",
    "hot_shrubland",
    "brushland",
    "shrubland",
    "highlands",
    "orchid_swamp",
    "desert_canyon",
    "desert_oasis",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::land_cover::{LC_BARE, LC_BUILT_UP, LC_WATER};

    fn cell(cover: u8, climate: Climate, lat: f64) -> Cell {
        Cell {
            cover,
            climate,
            eco: None,
            water_dist: 0,
            slope: 0,
            lat,
        }
    }

    #[test]
    fn cities_and_temperate_farmland_keep_vanilla_biomes() {
        let c = cell(LC_BUILT_UP, Climate::Temperate, 48.1);
        assert_eq!(biome("minecraft:plains", &c), "minecraft:plains");
        let c = cell(LC_TREE_COVER, Climate::Temperate, 48.1);
        assert_eq!(biome("minecraft:forest", &c), "minecraft:forest");
    }

    #[test]
    fn landscapes_get_the_terralith_biome_that_fits() {
        let steppe = cell(LC_GRASSLAND, Climate::ColdSteppe, 47.0);
        assert_eq!(biome("minecraft:plains", &steppe), "terralith:steppe");
        let oasis = cell(LC_TREE_COVER, Climate::HotDesert, 25.0);
        assert_eq!(biome("minecraft:desert", &oasis), "terralith:desert_oasis");
        let sand = cell(LC_BARE, Climate::HotDesert, 25.0);
        assert_eq!(biome("minecraft:desert", &sand), "minecraft:desert");
        let nile = cell(LC_WATER, Climate::HotDesert, 25.0);
        assert_eq!(biome("minecraft:river", &nile), "terralith:warm_river");
        let taiga = cell(LC_TREE_COVER, Climate::Boreal, 62.0);
        assert_eq!(biome("minecraft:taiga", &taiga), "terralith:birch_taiga");
    }

    #[test]
    fn every_used_biome_is_one_terralith_ships() {
        // Terralith 2.6.5 (26.3); 2.5.8 (1.21 to 1.21.4) and 2.6.2 ship every one Arnis uses too.
        const SHIPPED: &str = "alpha_islands alpha_islands_winter alpine_grove alpine_highlands \
            amethyst_canyon amethyst_rainforest ancient_sands arid_highlands ashen_savanna \
            basalt_cliffs birch_taiga blooming_plateau blooming_valley brushland bryce_canyon \
            caldera cloud_forest cold_shrubland deep_warm_ocean desert_canyon desert_oasis \
            desert_spires emerald_peaks forested_highlands fractured_savanna frozen_cliffs \
            glacial_chasm granite_cliffs gravel_beach gravel_desert haze_mountain highlands \
            hot_shrubland ice_marsh jungle_mountains lavender_forest lavender_valley lush_desert \
            lush_valley mirage_isles moonlight_grove moonlight_valley orchid_swamp \
            painted_mountains red_oasis rocky_jungle rocky_mountains rocky_shrubland \
            sakura_grove sakura_valley sandstone_valley savanna_badlands savanna_slopes \
            scarlet_mountains shield shield_clearing shrubland siberian_grove siberian_taiga \
            skylands_autumn skylands_spring skylands_summer skylands_winter snowy_badlands \
            snowy_cherry_grove snowy_maple_forest snowy_shield steppe stony_spires \
            temperate_highlands tropical_jungle valley_clearing volcanic_crater volcanic_peaks \
            warm_river warped_mesa white_cliffs white_mesa windswept_spires wintry_forest \
            wintry_lowlands yellowstone yosemite_cliffs yosemite_lowlands";
        let shipped: Vec<&str> = SHIPPED.split_whitespace().collect();
        for used in USED {
            assert!(shipped.contains(&used), "{used}");
        }
    }
}
