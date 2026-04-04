use crate::block_definitions::{
    Block, BRICK, COBBLESTONE, CYAN_TERRACOTTA, DIRT, GRASS_BLOCK, GRAVEL, GRAY_CONCRETE_POWDER,
    LIGHT_GRAY_CONCRETE, OAK_PLANKS, PODZOL, RED_TERRACOTTA, SAND, STONE_BRICKS, TERRACOTTA,
};
use crate::osm_parser::ProcessedWay;

pub fn get_blocks_for_surface(surface_type: &str) -> Option<&[Block]> {
    match surface_type {
        "clay" => Some(&[TERRACOTTA]),
        "sand" => Some(&[SAND]),
        "tartan" => Some(&[RED_TERRACOTTA]),
        "grass" => Some(&[GRASS_BLOCK]),
        "dirt" | "ground" | "earth" => Some(&[DIRT]),
        "mulch" => Some(&[PODZOL]),
        "pebblestone" | "cobblestone" | "unhewn_cobblestone" => Some(&[COBBLESTONE]),
        "paving_stones" | "sett" => Some(&[STONE_BRICKS]),
        "bricks" => Some(&[BRICK]),
        "wood" => Some(&[OAK_PLANKS]),
        "asphalt" => Some(&[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA]),
        "gravel" | "fine_gravel" => Some(&[GRAVEL]),
        "concrete" => Some(&[LIGHT_GRAY_CONCRETE]),
        _ => None,
    }
}

pub fn get_blocks_for_surface_way(way: &ProcessedWay, default: Vec<Block>) -> Vec<Block> {
    way.tags
        .get("surface")
        .and_then(|s| get_blocks_for_surface(s))
        .unwrap_or(&*default)
        .to_vec()
}

/// Pick a surface block deterministically based on coordinates.
/// Returns a random-looking mix of gray_concrete_powder and cyan_terracotta.
#[inline]
pub fn semirandom_surface(x: i32, z: i32, block_types: &[Block]) -> Block {
    // Combine coordinates into a single value and apply bit mixing for a scattered look
    let mut h = (x as u32).wrapping_mul(0x9E3779B9) ^ (z as u32).wrapping_mul(0x517CC1B7);
    h ^= h >> 16;
    h = h.wrapping_mul(0x45D9F3B);
    h ^= h >> 16;
    block_types[(h as usize) % block_types.len()]
}
