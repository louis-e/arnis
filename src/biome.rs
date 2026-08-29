//! Land-cover-driven biome assignment for Java Anvil chunks (1.18+).

use crate::climate::Climate;
use crate::coordinate_system::cartesian::XZPoint;
use crate::ground::Ground;
use crate::land_cover::{
    LC_BARE, LC_BUILT_UP, LC_CROPLAND, LC_GRASSLAND, LC_MANGROVES, LC_MOSS, LC_SHRUBLAND,
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
            _ => "minecraft:snowy_plains",
        },
        Climate::Temperate => biome_temperate(lc, lat_deg, water_dist),
    }
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

pub type ChunkBiomeNbt = Value;

/// The 4x4 (XZ) biome sample for one chunk, in row-major order (index = zi * 4 + xi).
///
/// Each cell is sampled at the centre of its 4x4 block quad. `ground_origin` is the
/// world-space origin the shared `Ground` grid is indexed from
/// (`WorldEditor::ground_origin`), and is subtracted before every lookup — the grid is
/// addressed relative to the world bbox, not in absolute world coordinates.
///
/// It is `(0, 0)` only for an unrotated world under the default Local projection. A rotated or
/// projected world has a non-zero bbox minimum, and passing `(0, 0)` there would sample the land
/// cover at the wrong offset — which is exactly what this function used to do, leaving biomes
/// misaligned with the land cover they are derived from.
pub fn chunk_biome_palette(
    chunk_x: i32,
    chunk_z: i32,
    ground: Option<&Ground>,
    center_lat_deg: f64,
    ground_origin: (i32, i32),
) -> [&'static str; 16] {
    let mut names: [&'static str; 16] = ["minecraft:plains"; 16];

    if let Some(g) = ground {
        let (origin_x, origin_z) = ground_origin;
        let climate = g.climate();
        for zi in 0..4i32 {
            for xi in 0..4i32 {
                let world_x = chunk_x * 16 + xi * 4 + 2;
                let world_z = chunk_z * 16 + zi * 4 + 2;
                let coord = XZPoint::new(world_x - origin_x, world_z - origin_z);
                let lc = g.cover_class(coord);
                let wd = g.water_distance(coord);
                names[(zi * 4 + xi) as usize] = biome_for_class(lc, climate, center_lat_deg, wd);
            }
        }
    }

    names
}

/// Build the `biomes` compound for one chunk, sampling LC at a 4x4 grid
/// (4-block resolution) and packing into the Anvil 1.18+ palette+data layout.
///
/// See `chunk_biome_palette` for the meaning of `ground_origin`.
pub fn build_chunk_biome_nbt(
    chunk_x: i32,
    chunk_z: i32,
    ground: Option<&Ground>,
    center_lat_deg: f64,
    ground_origin: (i32, i32),
) -> ChunkBiomeNbt {
    let names = chunk_biome_palette(chunk_x, chunk_z, ground, center_lat_deg, ground_origin);

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
        let nbt = build_chunk_biome_nbt(0, 0, None, 0.0, (0, 0));
        match nbt {
            Value::Compound(map) => {
                assert!(map.contains_key("palette"));
                assert!(!map.contains_key("data"));
            }
            _ => panic!("expected compound"),
        }
    }

    #[test]
    fn no_ground_palette_is_all_plains() {
        let names = chunk_biome_palette(7, -3, None, 51.0, (128, -64));
        assert_eq!(names.len(), 16);
        assert!(names.iter().all(|n| *n == "minecraft:plains"));
    }

    /// A 64x64 world over a 64x64 land-cover grid, so a `Ground` lookup at grid
    /// coordinate `n` reads cell `n` exactly (no resampling to reason about).
    ///
    /// Columns x < 20 are water, the rest tree cover; rows z >= 20 are snow/ice.
    /// Neither run length divides 16, so a one-chunk shift of the origin is
    /// visible in the sample.
    fn striped_ground() -> crate::ground::Ground {
        use crate::land_cover::LandCoverData;
        const N: usize = 64;
        let grid: Vec<Vec<u8>> = (0..N)
            .map(|z| {
                (0..N)
                    .map(|x| {
                        if z >= 20 {
                            LC_SNOW_ICE
                        } else if x < 20 {
                            LC_WATER
                        } else {
                            LC_TREE_COVER
                        }
                    })
                    .collect()
            })
            .collect();
        let lc = LandCoverData {
            grid,
            water_distance: vec![vec![0u8; N]; N],
            water_blend_cache: once_cell::sync::OnceCell::new(),
            width: N,
            height: N,
            cells_per_meter: 1.0,
        };
        crate::ground::Ground::new_flat_land_cover_test(lc, N, N)
    }

    /// The pre-fix sampling loop: raw world coordinates, no origin subtraction.
    /// Kept verbatim so the (0, 0)-origin case can be pinned to old behaviour.
    fn legacy_palette(
        chunk_x: i32,
        chunk_z: i32,
        ground: Option<&Ground>,
        center_lat_deg: f64,
    ) -> [&'static str; 16] {
        let mut names: [&'static str; 16] = ["minecraft:plains"; 16];
        if let Some(g) = ground {
            let climate = g.climate();
            for zi in 0..4i32 {
                for xi in 0..4i32 {
                    let coord = XZPoint::new(chunk_x * 16 + xi * 4 + 2, chunk_z * 16 + zi * 4 + 2);
                    let lc = g.cover_class(coord);
                    let wd = g.water_distance(coord);
                    names[(zi * 4 + xi) as usize] =
                        biome_for_class(lc, climate, center_lat_deg, wd);
                }
            }
        }
        names
    }

    /// The pre-fix NBT wrapper, so the compound can be compared field for field.
    fn legacy_nbt(
        chunk_x: i32,
        chunk_z: i32,
        ground: Option<&Ground>,
        center_lat_deg: f64,
    ) -> Value {
        let names = legacy_palette(chunk_x, chunk_z, ground, center_lat_deg);
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
        let data = pack_biome_indices(&indices, bits_per_index(palette.len()));
        let mut map = HashMap::with_capacity(2);
        map.insert("palette".to_string(), palette_value);
        map.insert("data".to_string(), Value::LongArray(LongArray::new(data)));
        Value::Compound(map)
    }

    // Index = zi * 4 + xi: the x stripe varies along the row, the z stripe across rows.
    #[test]
    fn palette_is_row_major_over_the_4x4_quad_centres() {
        let g = striped_ground();
        // Chunk (1, 1) spans blocks 16..=31, sampled at 18/22/26/30 on both axes.
        // x: only 18 < 20 (water); z: only 18 < 20 (not snow).
        let names = chunk_biome_palette(1, 1, Some(&g), 40.0, (0, 0));
        assert_eq!(names.len(), 16);
        assert_eq!(
            names,
            [
                // zi = 0 (world z 18): the x stripe shows through.
                "minecraft:river",
                "minecraft:forest",
                "minecraft:forest",
                "minecraft:forest",
                // zi = 1..3 (world z 22/26/30): snow overrides everything.
                "minecraft:snowy_plains",
                "minecraft:snowy_plains",
                "minecraft:snowy_plains",
                "minecraft:snowy_plains",
                "minecraft:snowy_plains",
                "minecraft:snowy_plains",
                "minecraft:snowy_plains",
                "minecraft:snowy_plains",
                "minecraft:snowy_plains",
                "minecraft:snowy_plains",
                "minecraft:snowy_plains",
                "minecraft:snowy_plains",
            ]
        );
    }

    // The default Local projection puts the bbox at (0, 0), so output is unchanged.
    #[test]
    fn zero_origin_matches_legacy_raw_coordinates() {
        let g = striped_ground();
        for chunk_x in -1..4i32 {
            for chunk_z in -1..4i32 {
                assert_eq!(
                    chunk_biome_palette(chunk_x, chunk_z, Some(&g), 40.0, (0, 0)),
                    legacy_palette(chunk_x, chunk_z, Some(&g), 40.0),
                    "chunk ({chunk_x}, {chunk_z})"
                );
            }
        }
    }

    // A projected/anchored world offsets the bbox; the origin must cancel it out.
    #[test]
    fn nonzero_origin_shifts_the_sample_by_that_offset() {
        let g = striped_ground();
        // One chunk of origin is one chunk of index: sampling chunk 1 on a world
        // whose grid starts at block 16 reads exactly what chunk 0 reads at (0, 0).
        assert_eq!(
            chunk_biome_palette(1, 1, Some(&g), 40.0, (16, 16)),
            chunk_biome_palette(0, 0, Some(&g), 40.0, (0, 0)),
        );
        assert_eq!(
            chunk_biome_palette(3, 2, Some(&g), 40.0, (32, 16)),
            chunk_biome_palette(1, 1, Some(&g), 40.0, (0, 0)),
        );
        // And it really is a different answer than the unshifted lookup: chunk (1, 1)
        // at origin (16, 16) samples grid 2/6/10/14, all water and all below the snow row.
        let shifted = chunk_biome_palette(1, 1, Some(&g), 40.0, (16, 16));
        assert!(shifted.iter().all(|n| *n == "minecraft:river"));
        assert_ne!(shifted, chunk_biome_palette(1, 1, Some(&g), 40.0, (0, 0)));
    }

    // The Java writer's compound is byte-for-byte what it was before the parameter existed.
    #[test]
    fn nbt_wrapper_unchanged_for_zero_origin() {
        let g = striped_ground();
        for (chunk_x, chunk_z) in [(0, 0), (1, 1), (2, 0), (0, 2), (-1, 3)] {
            assert_eq!(
                build_chunk_biome_nbt(chunk_x, chunk_z, Some(&g), 40.0, (0, 0)),
                legacy_nbt(chunk_x, chunk_z, Some(&g), 40.0),
                "chunk ({chunk_x}, {chunk_z})"
            );
        }
        // Uniform chunks keep the palette-only shape, mixed ones still carry data.
        match build_chunk_biome_nbt(1, 1, Some(&g), 40.0, (16, 16)) {
            Value::Compound(map) => assert!(!map.contains_key("data")),
            _ => panic!("expected compound"),
        }
        match build_chunk_biome_nbt(1, 1, Some(&g), 40.0, (0, 0)) {
            Value::Compound(map) => assert!(map.contains_key("data")),
            _ => panic!("expected compound"),
        }
    }

    // The NBT palette is exactly the distinct names of the plain palette, first-seen order.
    #[test]
    fn nbt_palette_is_the_plain_palette_deduplicated() {
        let g = striped_ground();
        let names = chunk_biome_palette(1, 1, Some(&g), 40.0, (0, 0));
        let mut expected: Vec<String> = Vec::new();
        for n in names {
            if !expected.iter().any(|e| e == n) {
                expected.push(n.to_string());
            }
        }
        match build_chunk_biome_nbt(1, 1, Some(&g), 40.0, (0, 0)) {
            Value::Compound(map) => match map.get("palette") {
                Some(Value::List(list)) => {
                    let got: Vec<String> = list
                        .iter()
                        .map(|v| match v {
                            Value::String(s) => s.clone(),
                            _ => panic!("expected string"),
                        })
                        .collect();
                    assert_eq!(got, expected);
                }
                _ => panic!("expected palette list"),
            },
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
