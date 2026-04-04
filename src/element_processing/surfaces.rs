use crate::block_definitions::{
    Block, BLACK_CONCRETE, BRICK, COBBLESTONE, DIRT, GRASS_BLOCK, GRAVEL, LIGHT_GRAY_CONCRETE,
    OAK_PLANKS, PODZOL, RED_TERRACOTTA, SAND, STONE_BRICKS, TERRACOTTA,
};
use crate::osm_parser::ProcessedWay;

pub fn get_block_for_surface(surface_type: &str) -> Option<Block> {
    match surface_type {
        "clay" => Some(TERRACOTTA),
        "sand" => Some(SAND),
        "tartan" => Some(RED_TERRACOTTA),
        "grass" => Some(GRASS_BLOCK),
        "dirt" | "ground" | "earth" => Some(DIRT),
        "mulch" => Some(PODZOL),
        "pebblestone" | "cobblestone" | "unhewn_cobblestone" => Some(COBBLESTONE),
        "paving_stones" | "sett" => Some(STONE_BRICKS),
        "bricks" => Some(BRICK),
        "wood" => Some(OAK_PLANKS),
        "asphalt" => Some(BLACK_CONCRETE),
        "gravel" | "fine_gravel" => Some(GRAVEL),
        "concrete" => Some(LIGHT_GRAY_CONCRETE),
        _ => None,
    }
}

pub fn get_block_for_surface_way(way: &ProcessedWay, default: Block) -> Block {
    way.tags
        .get("surface")
        .and_then(|s| get_block_for_surface(s))
        .unwrap_or(default)
}
