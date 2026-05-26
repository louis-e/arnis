//! Land-cover-driven biome assignment for Java Anvil chunks (1.18+).
//!
//! Each section in a chunk stores biomes as a 4×4×4 grid (64 cells) referencing
//! a per-section palette of biome IDs. The data array is omitted entirely when
//! the palette has only one entry, which is the common case for arnis chunks
//! that sit inside a uniform land-cover region. See
//! <https://minecraft.wiki/w/Chunk_format#NBT_structure>.

use crate::coordinate_system::cartesian::XZPoint;
use crate::ground::Ground;
use crate::land_cover::{
    LC_BARE, LC_BUILT_UP, LC_CROPLAND, LC_GRASSLAND, LC_MANGROVES, LC_MOSS, LC_SHRUBLAND,
    LC_SNOW_ICE, LC_TREE_COVER, LC_WATER, LC_WETLAND,
};
use fastnbt::{LongArray, Value};
use std::collections::HashMap;

/// Map an ESA WorldCover class to a Minecraft biome ID.
///
/// `lat_deg` is the absolute world-center latitude; tree cover narrows to
/// `taiga` above ~55° and to `jungle` below ~23.5°. `water_dist` is the LC
/// distance-to-shore — large open water becomes `ocean`, anything narrower
/// stays `river`.
pub fn biome_for_class(lc: u8, lat_deg: f64, water_dist: u8) -> &'static str {
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
        LC_SHRUBLAND => "minecraft:savanna",
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
        // LC == 0 (no data) or any unknown value
        _ => "minecraft:plains",
    }
}

/// A precomputed biome compound the same chunk can share across all of its
/// sections. The fastnbt `Value` is cloned per-section at NBT-build time.
pub type ChunkBiomeNbt = Value;

/// Build the `biomes` compound for one chunk.
///
/// Samples land cover at a 4×4 grid (one sample per biome cell, centred in
/// each 4-block footprint) and packs it into the Anvil 1.18+ palette+data
/// layout. The resulting compound is identical for every section in the
/// chunk because biomes are y-invariant under our 2D land-cover model.
///
/// When `ground` is `None` (no land cover loaded) or all 16 samples reduce to
/// a single biome, the compound contains just the palette and no data array,
/// keeping memory and NBT size minimal.
pub fn build_chunk_biome_nbt(
    chunk_x: i32,
    chunk_z: i32,
    ground: Option<&Ground>,
    center_lat_deg: f64,
) -> ChunkBiomeNbt {
    let mut names: [&'static str; 16] = ["minecraft:plains"; 16];

    if let Some(g) = ground {
        for zi in 0..4i32 {
            for xi in 0..4i32 {
                // Sample at the geometric centre of each 4×4-block footprint.
                let world_x = chunk_x * 16 + xi * 4 + 2;
                let world_z = chunk_z * 16 + zi * 4 + 2;
                let coord = XZPoint::new(world_x, world_z);
                let lc = g.cover_class(coord);
                let wd = g.water_distance(coord);
                names[(zi * 4 + xi) as usize] = biome_for_class(lc, center_lat_deg, wd);
            }
        }
    }

    // Dedup palette in first-occurrence order; record per-cell index in parallel.
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
        // Uniform-biome chunk: data array omitted, all 64 cells default to palette[0].
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

/// Bits required to index `palette_size` distinct biomes. Biome data has no
/// 4-bit minimum (unlike block_states); a 2-biome chunk uses 1 bit per cell.
fn bits_per_index(palette_size: usize) -> u32 {
    if palette_size <= 1 {
        0
    } else {
        (palette_size - 1).ilog2() + 1
    }
}

/// Pack 64 biome cell indices into i64 longs using the post-1.16 layout
/// (values do NOT straddle long boundaries; remaining bits are zero-padded).
///
/// `indices_16` holds the 4×4 xz biome map; it is replicated across the 4
/// y-layers of the section's 4×4×4 grid because LC is two-dimensional.
fn pack_biome_indices(indices_16: &[u8; 16], bits: u32) -> Vec<i64> {
    debug_assert!((1..=6).contains(&bits));
    let bits = bits as usize;
    let vals_per_long = 64 / bits;
    let num_longs = 64usize.div_ceil(vals_per_long);
    let mask: u64 = (1u64 << bits) - 1;

    let mut longs = vec![0u64; num_longs];
    for cell in 0..64usize {
        // cell index = y*16 + z*4 + x; xz biomes repeat across y, so xz_idx = cell % 16.
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
        let expected: u64 = (0..64u64).fold(0, |acc, c| acc | (((c % 2) as u64) << c));
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
        // bits=3, vals_per_long = 64/3 = 21, num_longs = ceil(64/21) = 4
        let indices = [4u8; 16];
        let longs = pack_biome_indices(&indices, 3);
        assert_eq!(longs.len(), 4);
    }

    #[test]
    fn no_ground_yields_plains_palette() {
        let nbt = build_chunk_biome_nbt(0, 0, None, 0.0);
        match nbt {
            Value::Compound(map) => {
                assert!(map.contains_key("palette"));
                assert!(!map.contains_key("data")); // uniform → no data array
            }
            _ => panic!("expected compound"),
        }
    }

    #[test]
    fn latitude_drives_tree_biome() {
        assert_eq!(biome_for_class(LC_TREE_COVER, 0.0, 0), "minecraft:jungle");
        assert_eq!(biome_for_class(LC_TREE_COVER, 40.0, 0), "minecraft:forest");
        assert_eq!(biome_for_class(LC_TREE_COVER, 60.0, 0), "minecraft:taiga");
        // Hemisphere symmetry
        assert_eq!(biome_for_class(LC_TREE_COVER, -60.0, 0), "minecraft:taiga");
    }

    #[test]
    fn water_distance_drives_river_vs_ocean() {
        assert_eq!(biome_for_class(LC_WATER, 0.0, 1), "minecraft:river");
        assert_eq!(biome_for_class(LC_WATER, 0.0, 7), "minecraft:river");
        assert_eq!(biome_for_class(LC_WATER, 0.0, 8), "minecraft:ocean");
    }
}
