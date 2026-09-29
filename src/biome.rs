//! Land-cover-driven biome assignment for Java Anvil chunks (1.18+).

use crate::climate::Climate;
use crate::coordinate_system::cartesian::XZPoint;
use crate::ecoregion::{EcoBiome, Ecoregion};
use crate::ground::Ground;
use crate::land_cover::{
    LC_BARE, LC_BEACH, LC_BUILT_UP, LC_CROPLAND, LC_GRASSLAND, LC_MANGROVES, LC_MOSS, LC_SHRUBLAND,
    LC_SNOW_ICE, LC_TREE_COVER, LC_WATER, LC_WETLAND,
};
use fastnbt::{LongArray, Value};
use std::collections::HashMap;

/// Minecraft biome for an ESA class + climate; temperate keeps the latitude-driven mapping.
pub fn biome_for_class(lc: u8, climate: Climate, lat_deg: f64, water_dist: u8) -> &'static str {
    if lc == LC_WATER {
        let abs_lat = lat_deg.abs();
        let cold = matches!(climate, Climate::IceCap | Climate::Tundra | Climate::Boreal);
        let deep = water_dist >= 12;
        if water_dist < 8 {
            return if cold {
                "minecraft:frozen_river"
            } else {
                "minecraft:river"
            };
        }
        return if cold {
            if deep {
                "minecraft:deep_frozen_ocean"
            } else {
                "minecraft:frozen_ocean"
            }
        } else if abs_lat < 23.5
            || matches!(
                climate,
                Climate::HotDesert | Climate::HotSteppe | Climate::TropicalSavanna
            )
        {
            "minecraft:warm_ocean"
        } else if abs_lat < 45.0 {
            if deep {
                "minecraft:deep_lukewarm_ocean"
            } else {
                "minecraft:lukewarm_ocean"
            }
        } else if deep {
            "minecraft:deep_cold_ocean"
        } else {
            "minecraft:cold_ocean"
        };
    }
    match climate {
        Climate::HotDesert | Climate::ColdDesert => "minecraft:desert",
        Climate::HotSteppe | Climate::TropicalSavanna | Climate::DryContinental => {
            "minecraft:savanna"
        }
        Climate::ColdSteppe => "minecraft:plains",
        Climate::Tundra | Climate::IceCap => "minecraft:snowy_plains",
        Climate::Boreal => match lc {
            LC_TREE_COVER | LC_MOSS => "minecraft:taiga",
            LC_WETLAND => "minecraft:swamp",
            LC_BEACH => "minecraft:snowy_beach",
            _ => "minecraft:snowy_plains",
        },
        Climate::Temperate => biome_temperate(lc, lat_deg, water_dist),
    }
}

/// Land biome from the ecoregion where it beats latitude; snowy and arid climates keep theirs.
pub fn ecoregion_biome(lc: u8, climate: Climate, eco: Option<Ecoregion>) -> Option<&'static str> {
    use EcoBiome::*;
    let eco = eco?;
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

/// The latitude-driven baseline mapping (temperate behaviour).
fn biome_temperate(lc: u8, lat_deg: f64, water_dist: u8) -> &'static str {
    let abs_lat = lat_deg.abs();
    match lc {
        LC_TREE_COVER => {
            if abs_lat > 55.0 {
                "minecraft:taiga"
            } else if abs_lat < 23.5 {
                "minecraft:jungle"
            } else {
                "minecraft:forest"
            }
        }
        LC_SHRUBLAND => {
            if abs_lat < 23.5 {
                "minecraft:sparse_jungle"
            } else {
                "minecraft:savanna"
            }
        }
        LC_GRASSLAND | LC_CROPLAND | LC_BUILT_UP => "minecraft:plains",
        LC_BARE => "minecraft:desert",
        LC_BEACH => "minecraft:beach",
        LC_SNOW_ICE => "minecraft:snowy_plains",
        LC_WATER => {
            if water_dist >= 8 {
                "minecraft:ocean"
            } else {
                "minecraft:river"
            }
        }
        LC_WETLAND => "minecraft:swamp",
        LC_MANGROVES => "minecraft:mangrove_swamp",
        LC_MOSS => "minecraft:taiga",
        _ => "minecraft:plains",
    }
}

/// High-mountain biome for a cell `above_m` metres above the snow line, or `None`
/// below the alpine band, where the land-cover mapping holds.
///
/// Above the line everything is snow country, which also lets the game snow and
/// freeze there. Up to a kilometre below it, open ground is alpine meadow and bare
/// ground is stony peaks.
pub fn mountain_biome(
    lc: u8,
    climate: Climate,
    water_dist: u8,
    above_m: f64,
    slope: i32,
) -> Option<&'static str> {
    if lc == LC_WATER {
        // Mountain lakes and streams; open sea keeps its ocean biome.
        return (above_m >= 0.0 && water_dist < 8).then_some("minecraft:frozen_river");
    }
    if above_m >= 0.0 {
        return Some(match slope {
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
    match lc {
        LC_GRASSLAND | LC_SHRUBLAND | LC_MOSS => Some("minecraft:meadow"),
        LC_BARE => Some("minecraft:stony_peaks"),
        LC_SNOW_ICE => Some("minecraft:snowy_slopes"),
        _ => None,
    }
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
            for zi in 0..4i32 {
                for xi in 0..4i32 {
                    let world_x = chunk_x * 16 + xi * 4 + 2;
                    let world_z = chunk_z * 16 + zi * 4 + 2;
                    let coord = XZPoint::new(world_x - ground_origin.0, world_z - ground_origin.1);
                    let lc = g.cover_class(coord);
                    let wd = g.water_distance(coord);
                    let mountain = (snow_y != i32::MAX)
                        .then(|| {
                            let above_m = match snow_y {
                                i32::MIN => f64::INFINITY,
                                t => f64::from(g.level(coord) - t) / g.blocks_per_meter(),
                            };
                            mountain_biome(lc, climate, wd, above_m, g.slope(coord))
                        })
                        .flatten();
                    names[(zi * 4 + xi) as usize] = mountain
                        .or_else(|| ecoregion_biome(lc, climate, g.ecoregion(coord)))
                        .unwrap_or_else(|| biome_for_class(lc, climate, center_lat_deg, wd));
                }
            }
        }
    }

    names
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

    #[test]
    fn latitude_drives_tree_biome() {
        let t = Climate::Temperate;
        assert_eq!(
            biome_for_class(LC_TREE_COVER, t, 0.0, 0),
            "minecraft:jungle"
        );
        assert_eq!(
            biome_for_class(LC_TREE_COVER, t, 40.0, 0),
            "minecraft:forest"
        );
        assert_eq!(
            biome_for_class(LC_TREE_COVER, t, 60.0, 0),
            "minecraft:taiga"
        );
        assert_eq!(
            biome_for_class(LC_TREE_COVER, t, -60.0, 0),
            "minecraft:taiga"
        );
    }

    #[test]
    fn climate_drives_arid_polar_biome() {
        assert_eq!(
            biome_for_class(LC_GRASSLAND, Climate::HotDesert, 25.0, 0),
            "minecraft:desert"
        );
        assert_eq!(
            biome_for_class(LC_GRASSLAND, Climate::IceCap, 75.0, 0),
            "minecraft:snowy_plains"
        );
    }

    #[test]
    fn water_biomes_by_climate_and_distance() {
        let t = Climate::Temperate;
        assert_eq!(biome_for_class(LC_WATER, t, 0.0, 1), "minecraft:river");
        assert_eq!(biome_for_class(LC_WATER, t, 0.0, 8), "minecraft:warm_ocean");
        assert_eq!(
            biome_for_class(LC_WATER, t, 35.0, 8),
            "minecraft:lukewarm_ocean"
        );
        assert_eq!(
            biome_for_class(LC_WATER, t, 35.0, 12),
            "minecraft:deep_lukewarm_ocean"
        );
        assert_eq!(
            biome_for_class(LC_WATER, t, 50.0, 8),
            "minecraft:cold_ocean"
        );
        assert_eq!(
            biome_for_class(LC_WATER, Climate::IceCap, 70.0, 1),
            "minecraft:frozen_river"
        );
        assert_eq!(
            biome_for_class(LC_WATER, Climate::IceCap, 70.0, 8),
            "minecraft:frozen_ocean"
        );
    }

    #[test]
    fn mountains_band_under_the_snow_line() {
        let t = Climate::Temperate;
        assert_eq!(
            mountain_biome(LC_GRASSLAND, t, 0, -400.0, 2),
            Some("minecraft:meadow")
        );
        assert_eq!(
            mountain_biome(LC_BARE, t, 0, -200.0, 5),
            Some("minecraft:stony_peaks")
        );
        assert_eq!(
            mountain_biome(LC_BARE, t, 0, 150.0, 4),
            Some("minecraft:snowy_slopes")
        );
        assert_eq!(
            mountain_biome(LC_BARE, t, 0, 150.0, 12),
            Some("minecraft:jagged_peaks")
        );
        // Below the alpine band, and forests at any height, keep the land-cover mapping.
        assert_eq!(mountain_biome(LC_GRASSLAND, t, 0, -1500.0, 2), None);
        assert_eq!(mountain_biome(LC_TREE_COVER, t, 0, -300.0, 2), None);
        assert_eq!(
            mountain_biome(LC_WATER, t, 2, 50.0, 0),
            Some("minecraft:frozen_river")
        );
        assert_eq!(mountain_biome(LC_WATER, t, 12, 50.0, 0), None);
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
    fn tropical_shrub_is_sparse_jungle() {
        assert_eq!(
            biome_for_class(LC_SHRUBLAND, Climate::Temperate, 5.0, 0),
            "minecraft:sparse_jungle"
        );
        assert_eq!(
            biome_for_class(LC_SHRUBLAND, Climate::Temperate, 45.0, 0),
            "minecraft:savanna"
        );
    }
}
