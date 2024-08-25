#![allow(unused)]

use fastnbt::Value;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Clone)]
pub struct Block {
    pub namespace: String,
    pub name: String,
    pub properties: Option<Value>,
}

impl Block {
    pub fn new(namespace: &str, name: &str, properties: Option<Value>) -> Self {
        Self {
            namespace: namespace.to_string(),
            name: name.to_string(),
            properties,
        }
    }
}

// Lazy static blocks
pub static AIR: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "air", None));
pub static BIRCH_LEAVES: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "birch_leaves", None));
pub static BIRCH_LOG: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "birch_log", None));
pub static BLACK_CONCRETE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "black_concrete", None));
pub static BLUE_FLOWER: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "blue_orchid", None));
pub static BRICK: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "bricks", None));
pub static CARROTS: Lazy<Block> = Lazy::new(|| {
    Block::new(
        "minecraft",
        "carrots",
        Some(Value::Compound({
            let mut map = HashMap::new();
            map.insert("age".to_string(), Value::Int(7));
            map
        })),
    )
});
pub static CAULDRON: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "cauldron", None));
pub static COBBLESTONE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "cobblestone", None));
pub static COBBLESTONE_WALL: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "cobblestone_wall", None));
pub static DARK_OAK_DOOR_LOWER: Lazy<Block> = Lazy::new(|| {
    Block::new(
        "minecraft",
        "dark_oak_door",
        Some(Value::Compound({
            let mut map = HashMap::new();
            map.insert("half".to_string(), Value::String("lower".to_string()));
            map
        })),
    )
});
pub static DARK_OAK_DOOR_UPPER: Lazy<Block> = Lazy::new(|| {
    Block::new(
        "minecraft",
        "dark_oak_door",
        Some(Value::Compound({
            let mut map = HashMap::new();
            map.insert("half".to_string(), Value::String("upper".to_string()));
            map
        })),
    )
});
pub static DIRT: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "dirt", None));
pub static FARMLAND: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "farmland", None));
pub static GLASS: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "glass_pane", None));
pub static GLOWSTONE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "glowstone", None));
pub static GRASS: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "grass_block", None));
pub static GRASS_BLOCK: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "grass_block", None));
pub static GRAVEL: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "gravel", None));
pub static GRAY_CONCRETE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "gray_concrete", None));
pub static GREEN_STAINED_HARDENED_CLAY: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "green_terracotta", None));
pub static HAY_BALE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "hay_block", None));
pub static IRON_BLOCK: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "iron_block", None));
pub static LIGHT_GRAY_CONCRETE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "light_gray_concrete", None));
pub static OAK_FENCE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "oak_fence", None));
pub static OAK_LEAVES: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "oak_leaves", None));
pub static OAK_LOG: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "oak_log", None));
pub static OAK_PLANKS: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "oak_planks", None));
pub static PODZOL: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "podzol", None));
pub static POTATOES: Lazy<Block> = Lazy::new(|| {
    Block::new(
        "minecraft",
        "potatoes",
        Some(Value::Compound({
            let mut map = HashMap::new();
            map.insert("age".to_string(), Value::Int(7));
            map
        })),
    )
});
pub static RAIL: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "rail", None));
pub static RED_FLOWER: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "poppy", None));
pub static SAND: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "sand", None));
pub static SCAFFOLDING: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "scaffolding", None));
pub static SPONGE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "sponge", None));
pub static SPRUCE_LOG: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "spruce_log", None));
pub static STONE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "stone", None));
pub static STONE_BLOCK_SLAB: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "stone_slab", None));
pub static STONE_BRICK_SLAB: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "stone_brick_slab", None));
pub static WATER: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "water", None));
pub static WHEAT: Lazy<Block> = Lazy::new(|| {
    Block::new(
        "minecraft",
        "wheat",
        Some(Value::Compound({
            let mut map = HashMap::new();
            map.insert("age".to_string(), Value::Int(7));
            map
        })),
    )
});
pub static WHITE_CONCRETE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "white_concrete", None));
pub static WHITE_FLOWER: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "azure_bluet", None));
pub static WHITE_STAINED_GLASS: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "white_stained_glass", None));
pub static YELLOW_FLOWER: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "dandelion", None));

// Variations for building corners
pub fn building_corner_variations() -> Vec<Block> {
    vec![
        Block::new("minecraft", "stone_bricks", None),
        Block::new("minecraft", "cobblestone", None),
        Block::new("minecraft", "bricks", None),
        Block::new("minecraft", "mossy_cobblestone", None),
        Block::new("minecraft", "sandstone", None),
        Block::new("minecraft", "red_nether_bricks", None),
        Block::new("minecraft", "blackstone", None),
        Block::new("minecraft", "smooth_quartz", None),
        Block::new("minecraft", "chiseled_stone_bricks", None),
        Block::new("minecraft", "polished_basalt", None),
        Block::new("minecraft", "cut_sandstone", None),
        Block::new("minecraft", "polished_blackstone_bricks", None),
    ]
}

// Variations for building walls
pub fn building_wall_variations() -> Vec<Block> {
    vec![
        Block::new("minecraft", "white_terracotta", None),
        Block::new("minecraft", "gray_terracotta", None),
        Block::new("minecraft", "bricks", None),
        Block::new("minecraft", "smooth_sandstone", None),
        Block::new("minecraft", "red_terracotta", None),
        Block::new("minecraft", "polished_diorite", None),
        Block::new("minecraft", "smooth_stone", None),
        Block::new("minecraft", "polished_andesite", None),
        Block::new("minecraft", "warped_planks", None),
        Block::new("minecraft", "end_stone_bricks", None),
        Block::new("minecraft", "smooth_red_sandstone", None),
        Block::new("minecraft", "nether_bricks", None),
    ]
}

// Variations for building floors
pub fn building_floor_variations() -> Vec<Block> {
    vec![
        Block::new("minecraft", "oak_planks", None),
        Block::new("minecraft", "spruce_planks", None),
        Block::new("minecraft", "dark_oak_planks", None),
        Block::new("minecraft", "stone_bricks", None),
        Block::new("minecraft", "polished_granite", None),
        Block::new("minecraft", "polished_diorite", None),
        Block::new("minecraft", "acacia_planks", None),
        Block::new("minecraft", "jungle_planks", None),
        Block::new("minecraft", "warped_planks", None),
        Block::new("minecraft", "purpur_block", None),
        Block::new("minecraft", "smooth_red_sandstone", None),
        Block::new("minecraft", "polished_blackstone", None),
    ]
}
