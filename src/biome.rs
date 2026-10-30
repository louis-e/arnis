//! Land-cover-driven biome assignment for Java Anvil chunks (1.18+).

use crate::climate::Climate;
use crate::coordinate_system::cartesian::XZPoint;
use crate::ecoregion::{EcoBiome, Ecoregion};
use crate::ground::Ground;
use crate::land_cover::{
    LC_BARE, LC_BEACH, LC_GRASSLAND, LC_MANGROVES, LC_MOSS, LC_SHRUBLAND, LC_SNOW_ICE,
    LC_TREE_COVER, LC_WATER, LC_WETLAND,
};
use fastnbt::{LongArray, Value};
use std::collections::HashMap;

/// What one 4x4 biome cell is classified from.
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub lc: u8,
    /// Water that is a lake, river or canal rather than the sea.
    pub inland: bool,
    /// Bare ground with water close by.
    pub shore: bool,
    /// Metres above the snow line, negative below it; `None` without terrain.
    pub above_snow_m: Option<f64>,
    pub slope: i32,
    pub y: i32,
    pub eco: Option<Ecoregion>,
}

#[cfg(test)]
impl Cell {
    fn land(lc: u8) -> Self {
        Self {
            lc,
            inland: false,
            shore: false,
            above_snow_m: None,
            slope: 0,
            y: -62,
            eco: None,
        }
    }
}

/// Red sand and rock country: Australia's interior deserts and the Colorado Plateau.
const RED_EARTH: [u16; 9] = [207, 208, 209, 210, 211, 213, 214, 215, 429];

/// Slope from which bare ground reads as a rock face, in `Ground::slope` units.
const ROCK_FACE_SLOPE: i32 = 4;

/// Minecraft biome for one cell.
pub fn cell_biome(c: &Cell, climate: Climate, lat_deg: f64) -> &'static str {
    // Below 60° ET/EF come from peaks or subpolar islands, whose lowlands are green in summer.
    let climate = match climate {
        Climate::Tundra | Climate::IceCap if lat_deg.abs() < 60.0 => Climate::Boreal,
        c => c,
    };
    if c.lc == LC_WATER {
        return water_biome(c, climate, lat_deg);
    }
    if let Some(name) = mountain_biome(c, climate) {
        return name;
    }
    let name =
        ecoregion_biome(c.lc, climate, c.eco).unwrap_or_else(|| land_biome(c, climate, lat_deg));
    match c.above_snow_m {
        Some(_) => snow_safe(name, c.y, c.lc == LC_TREE_COVER),
        None => name,
    }
}

/// Lakes and rivers take the river biome, the sea an ocean by latitude.
fn water_biome(c: &Cell, climate: Climate, lat_deg: f64) -> &'static str {
    let frozen = matches!(climate, Climate::Tundra | Climate::IceCap)
        || c.above_snow_m.is_some_and(|m| m >= 0.0);
    if c.inland {
        return if frozen {
            "minecraft:frozen_river"
        } else {
            "minecraft:river"
        };
    }
    let abs_lat = lat_deg.abs();
    if frozen {
        "minecraft:frozen_ocean"
    } else if abs_lat < 23.5
        || matches!(
            climate,
            Climate::HotDesert | Climate::HotSteppe | Climate::TropicalSavanna
        )
    {
        "minecraft:warm_ocean"
    } else if abs_lat < 45.0 {
        "minecraft:lukewarm_ocean"
    } else {
        "minecraft:cold_ocean"
    }
}

/// Snow biomes above the snow line, meadow and stony peaks in the alpine band below it.
fn mountain_biome(c: &Cell, climate: Climate) -> Option<&'static str> {
    let above_m = c.above_snow_m?;
    if above_m >= 0.0 {
        return Some(match c.slope {
            i32::MIN..=1 => "minecraft:snowy_plains",
            2..=6 => "minecraft:snowy_slopes",
            _ => "minecraft:jagged_peaks",
        });
    }
    if above_m < -crate::ground_decoration::ALPINE_BAND_METRES
        || matches!(climate, Climate::HotDesert | Climate::ColdDesert)
    {
        return None;
    }
    match c.lc {
        LC_GRASSLAND | LC_SHRUBLAND | LC_MOSS => Some("minecraft:meadow"),
        LC_BARE => Some("minecraft:stony_peaks"),
        LC_SNOW_ICE => Some("minecraft:snowy_slopes"),
        _ => None,
    }
}

/// Land biome from the ecoregion where it beats the climate alone.
fn ecoregion_biome(lc: u8, climate: Climate, eco: Option<Ecoregion>) -> Option<&'static str> {
    use EcoBiome::*;
    let eco = eco?;
    let dry = matches!(
        climate,
        Climate::HotDesert
            | Climate::ColdDesert
            | Climate::HotSteppe
            | Climate::ColdSteppe
            | Climate::DryContinental
    );
    let open = matches!(lc, LC_TREE_COVER | LC_SHRUBLAND | LC_GRASSLAND | LC_BARE);
    if dry && open && RED_EARTH.contains(&eco.id) {
        return Some(if lc == LC_TREE_COVER {
            "minecraft:wooded_badlands"
        } else {
            "minecraft:badlands"
        });
    }
    if matches!(
        climate,
        Climate::HotDesert
            | Climate::ColdDesert
            | Climate::Boreal
            | Climate::Tundra
            | Climate::IceCap
    ) {
        return None;
    }
    // Cold deserts and steppes snow in winter, which a savanna never does.
    let dry_warm = !matches!(climate, Climate::ColdSteppe);
    Some(match (lc, eco.biome) {
        (LC_TREE_COVER, MoistTropical) => "minecraft:jungle",
        (LC_TREE_COVER, DryTropical) => "minecraft:sparse_jungle",
        (
            LC_TREE_COVER,
            TropicalConifer | TemperateBroadleaf | TemperateGrassland | MontaneGrassland
            | Mediterranean,
        ) => "minecraft:forest",
        (LC_TREE_COVER, TemperateConifer | Boreal) => "minecraft:taiga",
        (LC_TREE_COVER, TropicalGrassland | Desert) if dry_warm => "minecraft:savanna",
        (LC_SHRUBLAND, MoistTropical | DryTropical) => "minecraft:sparse_jungle",
        (LC_SHRUBLAND, Mediterranean) if dry_warm => "minecraft:savanna",
        (LC_SHRUBLAND | LC_GRASSLAND, TropicalGrassland | Desert) if dry_warm => {
            "minecraft:savanna"
        }
        (LC_SHRUBLAND | LC_GRASSLAND, MontaneGrassland) => "minecraft:meadow",
        (LC_SHRUBLAND, TemperateConifer | Boreal | Tundra) => "minecraft:taiga",
        _ => return None,
    })
}

/// Land biome from the climate and land cover alone.
fn land_biome(c: &Cell, climate: Climate, lat_deg: f64) -> &'static str {
    let abs_lat = lat_deg.abs();
    // Dry highland steppe near the equator is no jungle.
    let tropical = abs_lat < 23.5 && climate == Climate::Temperate;
    if c.lc == LC_MANGROVES {
        return "minecraft:mangrove_swamp";
    }
    match climate {
        Climate::HotDesert | Climate::ColdDesert => "minecraft:desert",
        Climate::HotSteppe | Climate::TropicalSavanna | Climate::DryContinental => match c.lc {
            LC_WETLAND => "minecraft:swamp",
            _ => "minecraft:savanna",
        },
        Climate::Tundra | Climate::IceCap => match c.lc {
            LC_TREE_COVER => "minecraft:snowy_taiga",
            LC_BEACH => "minecraft:snowy_beach",
            _ => "minecraft:snowy_plains",
        },
        Climate::ColdSteppe | Climate::Boreal | Climate::Temperate => match c.lc {
            LC_TREE_COVER if climate == Climate::Boreal || abs_lat > 55.0 => "minecraft:taiga",
            LC_TREE_COVER if tropical => "minecraft:jungle",
            LC_TREE_COVER => "minecraft:forest",
            LC_SHRUBLAND if climate == Climate::Boreal => "minecraft:taiga",
            LC_SHRUBLAND if tropical => "minecraft:sparse_jungle",
            LC_MOSS => "minecraft:taiga",
            LC_BARE if c.shore => "minecraft:stony_shore",
            LC_BARE if c.slope >= ROCK_FACE_SLOPE => "minecraft:stony_peaks",
            LC_BEACH => "minecraft:beach",
            LC_SNOW_ICE => "minecraft:snowy_plains",
            LC_WETLAND => "minecraft:swamp",
            _ => "minecraft:plains",
        },
    }
}

/// Base temperature of the biomes `snow_safe` swaps between.
fn game_temperature(name: &str) -> Option<f32> {
    Some(match name {
        "minecraft:stony_shore" => 0.2,
        "minecraft:taiga" => 0.25,
        "minecraft:old_growth_pine_taiga" => 0.3,
        "minecraft:meadow" => 0.5,
        "minecraft:forest" => 0.7,
        "minecraft:plains" => 0.8,
        "minecraft:stony_peaks" => 1.0,
        _ => return None,
    })
}

/// Lowest Y the game snows on a biome: flat worlds cool 0.05 per 40 blocks above Y -46.
fn game_snow_y(name: &str) -> Option<i32> {
    game_temperature(name).map(|t| -46 + ((t - 0.15) / 0.00125) as i32)
}

/// Swaps a biome the game would snow on at `y` for a warmer one of its kind, else the warmest.
fn snow_safe(name: &'static str, y: i32, trees: bool) -> &'static str {
    // Height noise shifts the onset by up to 8 blocks; canopies stand above the ground.
    let top = y + 8 + if trees { 24 } else { 2 };
    let mut pick = name;
    while game_snow_y(pick).is_some_and(|s| top >= s) {
        pick = match pick {
            "minecraft:taiga" => "minecraft:old_growth_pine_taiga",
            "minecraft:old_growth_pine_taiga" => "minecraft:forest",
            "minecraft:meadow" => "minecraft:plains",
            "minecraft:stony_shore" => "minecraft:stony_peaks",
            _ => break,
        };
    }
    pick
}

pub type ChunkBiomeNbt = Value;

/// Biome per 4x4 horizontal cell of one chunk, in `zi * 4 + xi` order.
///
/// Minecraft stores biomes per 4x4x4 cell, but Arnis classifies by land cover,
/// which is flat: every cell in a column gets the same biome. The voxy LOD
/// writer wants the names rather than the packed NBT, so both callers share
/// this.
///
/// `ground_origin` is the world position of the ground grid's first cell.
pub fn chunk_biome_names(
    chunk_x: i32,
    chunk_z: i32,
    ground: Option<&Ground>,
    center_lat_deg: f64,
    ground_origin: (i32, i32),
) -> [&'static str; 16] {
    let mut names: [&'static str; 16] = ["minecraft:plains"; 16];

    if let Some(g) = ground {
        if !g.body().is_earth() {
            // One barren biome for the whole world; no land cover to classify.
            names = [g.body().biome(); 16];
        } else {
            let climate = g.climate();
            let snow_y = if g.elevation_enabled {
                g.snow_threshold_y()
            } else {
                i32::MAX
            };
            let sea_max_y = g.sea_surface_max_y();
            for zi in 0..4i32 {
                for xi in 0..4i32 {
                    let world_x = chunk_x * 16 + xi * 4 + 2;
                    let world_z = chunk_z * 16 + zi * 4 + 2;
                    let coord = XZPoint::new(world_x - ground_origin.0, world_z - ground_origin.1);
                    let cell = classify_input(g, coord, snow_y, sea_max_y);
                    let name = cell_biome(&cell, climate, center_lat_deg);
                    names[(zi * 4 + xi) as usize] =
                        if g.winter() { winter_biome(name) } else { name };
                }
            }
        }
    }

    names
}

/// Reach in blocks within which bare ground counts as shore.
const SHORE_REACH: i32 = 8;

fn classify_input(g: &Ground, coord: XZPoint, snow_y: i32, sea_max_y: Option<i32>) -> Cell {
    let lc = g.cover_class(coord);
    let y = g.level(coord);
    let water_at =
        |dx: i32, dz: i32| g.cover_class(XZPoint::new(coord.x + dx, coord.z + dz)) == LC_WATER;
    Cell {
        lc,
        inland: lc == LC_WATER
            && (g.inland_water_mapped(coord) || sea_max_y.is_some_and(|s| y > s)),
        shore: lc == LC_BARE
            && [
                (SHORE_REACH, 0),
                (-SHORE_REACH, 0),
                (0, SHORE_REACH),
                (0, -SHORE_REACH),
            ]
            .iter()
            .any(|&(dx, dz)| water_at(dx, dz)),
        above_snow_m: (snow_y != i32::MAX).then(|| match snow_y {
            i32::MIN => f64::INFINITY,
            t => f64::from(y - t) / g.blocks_per_meter(),
        }),
        slope: g.slope(coord),
        y,
        eco: g.ecoregion(coord),
    }
}

/// The snowy counterpart of a biome, so a winter world keeps snowing in game. The sea
/// stays cold but unfrozen: the game would otherwise ice it over from shore to shore.
pub fn winter_biome(name: &'static str) -> &'static str {
    match name {
        "minecraft:river" => "minecraft:frozen_river",
        "minecraft:beach" | "minecraft:stony_shore" => "minecraft:snowy_beach",
        "minecraft:ocean" | "minecraft:lukewarm_ocean" | "minecraft:warm_ocean" => {
            "minecraft:cold_ocean"
        }
        "minecraft:deep_ocean" | "minecraft:deep_lukewarm_ocean" => "minecraft:deep_cold_ocean",
        "minecraft:forest"
        | "minecraft:taiga"
        | "minecraft:old_growth_pine_taiga"
        | "minecraft:wooded_badlands"
        | "minecraft:birch_forest"
        | "minecraft:dark_forest"
        | "minecraft:jungle"
        | "minecraft:sparse_jungle"
        | "minecraft:mangrove_swamp"
        | "minecraft:swamp" => "minecraft:snowy_taiga",
        "minecraft:meadow" | "minecraft:stony_peaks" => "minecraft:snowy_slopes",
        _ if name.contains("snowy")
            || name.contains("frozen")
            || name.contains("cold")
            || name.ends_with("_peaks")
            || name == "minecraft:grove"
            || name == "minecraft:ice_spikes" =>
        {
            name
        }
        _ => "minecraft:snowy_plains",
    }
}

/// Packs an already-classified 4x4 biome grid into the Anvil container.
pub fn biome_nbt_from_names(names: &[&'static str; 16]) -> ChunkBiomeNbt {
    let mut palette: Vec<&'static str> = Vec::with_capacity(4);
    let mut indices: [u8; 16] = [0; 16];
    for (i, &name) in names.iter().enumerate() {
        let idx = match palette.iter().position(|p| *p == name) {
            Some(idx) => idx,
            None => {
                palette.push(name);
                palette.len() - 1
            }
        };
        indices[i] = idx as u8;
    }

    let palette_value = Value::List(
        palette
            .iter()
            .map(|&s| Value::String(s.to_string()))
            .collect(),
    );

    if palette.len() <= 1 {
        let mut map = HashMap::with_capacity(1);
        map.insert("palette".to_string(), palette_value);
        return Value::Compound(map);
    }

    let bits = bits_per_index(palette.len());
    let data = pack_biome_indices(&indices, bits);

    let mut map = HashMap::with_capacity(2);
    map.insert("palette".to_string(), palette_value);
    map.insert("data".to_string(), Value::LongArray(LongArray::new(data)));
    Value::Compound(map)
}

fn bits_per_index(palette_size: usize) -> u32 {
    if palette_size <= 1 {
        0
    } else {
        (palette_size - 1).ilog2() + 1
    }
}

// Post-1.16 packing: values do not straddle long boundaries.
fn pack_biome_indices(indices_16: &[u8; 16], bits: u32) -> Vec<i64> {
    debug_assert!((1..=6).contains(&bits));
    let bits = bits as usize;
    let vals_per_long = 64 / bits;
    let num_longs = 64usize.div_ceil(vals_per_long);
    let mask: u64 = (1u64 << bits) - 1;

    let mut longs = vec![0u64; num_longs];
    for cell in 0..64usize {
        // xz biomes repeat across y, so xz_idx = cell % 16.
        let xz_idx = cell % 16;
        let value = (indices_16[xz_idx] as u64) & mask;
        let long_idx = cell / vals_per_long;
        let bit_offset = (cell % vals_per_long) * bits;
        longs[long_idx] |= value << bit_offset;
    }
    longs.into_iter().map(|u| u as i64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_per_index_table() {
        assert_eq!(bits_per_index(1), 0);
        assert_eq!(bits_per_index(2), 1);
        assert_eq!(bits_per_index(3), 2);
        assert_eq!(bits_per_index(4), 2);
        assert_eq!(bits_per_index(5), 3);
        assert_eq!(bits_per_index(8), 3);
        assert_eq!(bits_per_index(9), 4);
        assert_eq!(bits_per_index(16), 4);
    }

    #[test]
    fn pack_alternating_1bit_fits_one_long() {
        let mut indices = [0u8; 16];
        for (i, v) in indices.iter_mut().enumerate() {
            *v = (i % 2) as u8;
        }
        let longs = pack_biome_indices(&indices, 1);
        assert_eq!(longs.len(), 1);
        let expected: u64 = (0..64u64).fold(0, |acc, c| acc | ((c % 2) << c));
        assert_eq!(longs[0] as u64, expected);
    }

    #[test]
    fn pack_three_biomes_uses_two_longs() {
        let mut indices = [0u8; 16];
        for (i, v) in indices.iter_mut().enumerate() {
            *v = (i % 3) as u8;
        }
        let longs = pack_biome_indices(&indices, 2);
        assert_eq!(longs.len(), 2);
    }

    #[test]
    fn pack_three_bit_pads_to_four_longs() {
        let indices = [4u8; 16];
        let longs = pack_biome_indices(&indices, 3);
        assert_eq!(longs.len(), 4);
    }

    #[test]
    fn no_ground_yields_plains_palette() {
        let nbt = biome_nbt_from_names(&chunk_biome_names(0, 0, None, 0.0, (0, 0)));
        match nbt {
            Value::Compound(map) => {
                assert!(map.contains_key("palette"));
                assert!(!map.contains_key("data"));
            }
            _ => panic!("expected compound"),
        }
    }

    use crate::land_cover::LC_BUILT_UP;

    fn at(lc: u8, climate: Climate, lat: f64) -> &'static str {
        cell_biome(&Cell::land(lc), climate, lat)
    }

    fn water(inland: bool, climate: Climate, lat: f64) -> &'static str {
        let c = Cell {
            inland,
            ..Cell::land(LC_WATER)
        };
        cell_biome(&c, climate, lat)
    }

    fn high(lc: u8, above_m: f64, slope: i32, y: i32) -> &'static str {
        let c = Cell {
            above_snow_m: Some(above_m),
            slope,
            y,
            ..Cell::land(lc)
        };
        cell_biome(&c, Climate::Temperate, 46.0)
    }

    #[test]
    fn latitude_drives_tree_biome() {
        let t = Climate::Temperate;
        assert_eq!(at(LC_TREE_COVER, t, 0.0), "minecraft:jungle");
        assert_eq!(at(LC_TREE_COVER, t, 40.0), "minecraft:forest");
        assert_eq!(at(LC_TREE_COVER, t, 60.0), "minecraft:taiga");
        assert_eq!(at(LC_TREE_COVER, t, -60.0), "minecraft:taiga");
    }

    #[test]
    fn climate_drives_arid_polar_biome() {
        assert_eq!(
            at(LC_GRASSLAND, Climate::HotDesert, 25.0),
            "minecraft:desert"
        );
        assert_eq!(
            at(LC_GRASSLAND, Climate::IceCap, 75.0),
            "minecraft:snowy_plains"
        );
        assert_eq!(
            at(LC_TREE_COVER, Climate::Tundra, 68.0),
            "minecraft:snowy_taiga"
        );
    }

    #[test]
    fn boreal_summer_is_green() {
        let b = Climate::Boreal;
        assert_eq!(at(LC_BUILT_UP, b, 66.0), "minecraft:plains");
        assert_eq!(at(LC_GRASSLAND, b, 66.0), "minecraft:plains");
        assert_eq!(at(LC_TREE_COVER, b, 66.0), "minecraft:taiga");
        assert_eq!(at(LC_BEACH, b, 66.0), "minecraft:beach");
        assert_eq!(water(true, b, 66.0), "minecraft:river");
        assert_eq!(water(false, b, 66.0), "minecraft:cold_ocean");
    }

    #[test]
    fn lakes_and_rivers_are_rivers_the_sea_is_ocean() {
        let t = Climate::Temperate;
        assert_eq!(water(true, t, 47.5), "minecraft:river");
        assert_eq!(water(false, t, 0.0), "minecraft:warm_ocean");
        assert_eq!(water(false, t, 35.0), "minecraft:lukewarm_ocean");
        assert_eq!(water(false, t, 50.0), "minecraft:cold_ocean");
        assert_eq!(
            water(false, Climate::HotDesert, 27.0),
            "minecraft:warm_ocean"
        );
        assert_eq!(water(true, Climate::IceCap, 70.0), "minecraft:frozen_river");
        assert_eq!(
            water(false, Climate::IceCap, 70.0),
            "minecraft:frozen_ocean"
        );
    }

    #[test]
    fn mountains_band_under_the_snow_line() {
        assert_eq!(high(LC_GRASSLAND, -400.0, 2, 0), "minecraft:meadow");
        assert_eq!(high(LC_BARE, -200.0, 5, 0), "minecraft:stony_peaks");
        assert_eq!(high(LC_BARE, 150.0, 4, 0), "minecraft:snowy_slopes");
        assert_eq!(high(LC_BARE, 150.0, 12, 0), "minecraft:jagged_peaks");
        // Below the alpine band, and forests at any height, keep the land-cover mapping.
        assert_eq!(high(LC_GRASSLAND, -1500.0, 2, 0), "minecraft:plains");
        assert_eq!(high(LC_TREE_COVER, -300.0, 2, 0), "minecraft:forest");
        let tarn = Cell {
            inland: true,
            above_snow_m: Some(50.0),
            ..Cell::land(LC_WATER)
        };
        assert_eq!(
            cell_biome(&tarn, Climate::Temperate, 46.0),
            "minecraft:frozen_river"
        );
    }

    #[test]
    fn mountain_tundra_keeps_green_valleys() {
        let c = Cell {
            above_snow_m: Some(-1300.0),
            ..Cell::land(LC_GRASSLAND)
        };
        assert_eq!(cell_biome(&c, Climate::Tundra, 46.0), "minecraft:plains");
        let c = Cell {
            above_snow_m: Some(-1300.0),
            ..Cell::land(LC_TREE_COVER)
        };
        assert_eq!(cell_biome(&c, Climate::Tundra, 46.0), "minecraft:taiga");
        // Arctic tundra stays snowy.
        assert_eq!(
            cell_biome(&c, Climate::Tundra, 70.0),
            "minecraft:snowy_taiga"
        );
        // Without terrain too, so the result does not hinge on it.
        assert_eq!(at(LC_GRASSLAND, Climate::Tundra, 52.0), "minecraft:plains");
    }

    #[test]
    fn steppe_near_the_equator_is_no_jungle() {
        assert_eq!(
            at(LC_SHRUBLAND, Climate::ColdSteppe, -16.0),
            "minecraft:plains"
        );
        assert_eq!(
            at(LC_TREE_COVER, Climate::ColdSteppe, -16.0),
            "minecraft:forest"
        );
    }

    #[test]
    fn sea_above_the_snow_line_is_frozen_ocean() {
        let c = Cell {
            above_snow_m: Some(10.0),
            ..Cell::land(LC_WATER)
        };
        assert_eq!(
            cell_biome(&c, Climate::Temperate, 50.0),
            "minecraft:frozen_ocean"
        );
    }

    #[test]
    fn game_snow_starts_where_the_flat_generator_cools() {
        assert_eq!(game_snow_y("minecraft:taiga"), Some(34));
        assert_eq!(game_snow_y("minecraft:plains"), Some(474));
        assert_eq!(game_snow_y("minecraft:savanna"), None);
    }

    #[test]
    fn high_ground_swaps_to_a_biome_that_stays_green() {
        assert_eq!(high(LC_TREE_COVER, -1500.0, 2, -50), "minecraft:forest");
        let taiga = |y| {
            let c = Cell {
                above_snow_m: Some(-1500.0),
                y,
                ..Cell::land(LC_TREE_COVER)
            };
            cell_biome(&c, Climate::Boreal, 66.0)
        };
        assert_eq!(taiga(-60), "minecraft:taiga");
        assert_eq!(taiga(20), "minecraft:old_growth_pine_taiga");
        assert_eq!(taiga(100), "minecraft:forest");
        // Nothing stays green this high, so the warmest of the kind is kept.
        assert_eq!(taiga(900), "minecraft:forest");
        // The alpine band keeps its meadows: snow showers belong up there.
        assert_eq!(high(LC_GRASSLAND, -400.0, 2, 300), "minecraft:meadow");
        let shore = Cell {
            shore: true,
            above_snow_m: Some(-1500.0),
            y: 40,
            ..Cell::land(LC_BARE)
        };
        assert_eq!(
            cell_biome(&shore, Climate::Temperate, 60.0),
            "minecraft:stony_peaks"
        );
    }

    #[test]
    fn bare_ground_is_shore_rock_or_plain() {
        let t = Climate::Temperate;
        assert_eq!(at(LC_BARE, t, 50.0), "minecraft:plains");
        let shore = Cell {
            shore: true,
            ..Cell::land(LC_BARE)
        };
        assert_eq!(cell_biome(&shore, t, 50.0), "minecraft:stony_shore");
        let face = Cell {
            slope: 6,
            ..Cell::land(LC_BARE)
        };
        assert_eq!(cell_biome(&face, t, 50.0), "minecraft:stony_peaks");
    }

    #[test]
    fn ecoregion_beats_latitude_where_it_knows_better() {
        use crate::ecoregion::lookup;
        let t = Climate::Temperate;
        // Central Mexican matorral and Ethiopian montane grasslands: highlands, not jungle.
        assert_eq!(
            ecoregion_biome(LC_TREE_COVER, t, lookup(427)),
            Some("minecraft:savanna")
        );
        assert_eq!(
            ecoregion_biome(LC_GRASSLAND, t, lookup(79)),
            Some("minecraft:meadow")
        );
        // Eastern Mediterranean scrub dries out, its grass keeps the rain.
        assert_eq!(
            ecoregion_biome(LC_SHRUBLAND, t, lookup(791)),
            Some("minecraft:savanna")
        );
        assert_eq!(ecoregion_biome(LC_GRASSLAND, t, lookup(791)), None);
        // Arid climates, water and towns keep their own mapping.
        assert_eq!(
            ecoregion_biome(LC_TREE_COVER, Climate::HotDesert, lookup(427)),
            None
        );
        assert_eq!(ecoregion_biome(LC_WATER, t, lookup(427)), None);
        assert_eq!(ecoregion_biome(LC_BUILT_UP, t, lookup(1)), None);
        assert_eq!(ecoregion_biome(LC_TREE_COVER, t, None), None);
        // Great Basin shrub steppe snows in winter: no savanna.
        assert_eq!(
            ecoregion_biome(LC_SHRUBLAND, Climate::ColdSteppe, lookup(430)),
            None
        );
        // A lawn inside a flooded-grassland ecoregion (Miami) is no swamp; ESA marks the marsh.
        assert_eq!(ecoregion_biome(LC_GRASSLAND, t, lookup(581)), None);
    }

    #[test]
    fn red_rock_country_is_badlands() {
        use crate::ecoregion::lookup;
        // Great Sandy-Tanami desert (Uluru) and the Colorado Plateau.
        assert_eq!(
            ecoregion_biome(LC_SHRUBLAND, Climate::HotDesert, lookup(210)),
            Some("minecraft:badlands")
        );
        assert_eq!(
            ecoregion_biome(LC_TREE_COVER, Climate::ColdDesert, lookup(429)),
            Some("minecraft:wooded_badlands")
        );
        // Towns, wetlands and the wetter rim around the plateau are no badlands.
        assert_eq!(
            ecoregion_biome(LC_BUILT_UP, Climate::HotDesert, lookup(210)),
            None
        );
        assert_eq!(
            ecoregion_biome(LC_WETLAND, Climate::HotDesert, lookup(210)),
            None
        );
        assert_eq!(
            ecoregion_biome(LC_BARE, Climate::Temperate, lookup(429)),
            None
        );
    }

    #[test]
    fn heath_and_scrub_outside_the_tropics_keep_the_rain() {
        let t = Climate::Temperate;
        assert_eq!(at(LC_SHRUBLAND, t, 5.0), "minecraft:sparse_jungle");
        assert_eq!(at(LC_SHRUBLAND, t, 53.0), "minecraft:plains");
    }
}
