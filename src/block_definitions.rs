#![allow(unused)]

use fastnbt::Value;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::colors::RGBTuple;

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
pub static ACACIA_PLANKS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "acacia_planks", None));
pub static AIR: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "air", None));
pub static ANDESITE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "andesite", None));
pub static BIRCH_LEAVES: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "birch_leaves", None));
pub static BIRCH_LOG: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "birch_log", None));
pub static BLACK_CONCRETE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "black_concrete", None));
pub static BLACKSTONE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "blackstone", None));
pub static BLUE_FLOWER: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "blue_orchid", None));
pub static BLUE_TERRACOTTA: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "blue_terracotta", None));
pub static BRICK: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "bricks", None));
pub static CAULDRON: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "cauldron", None));
pub static CHISELED_STONE_BRICKS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "chiseled_stone_bricks", None));
pub static COBBLESTONE_WALL: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "cobblestone_wall", None));
pub static COBBLESTONE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "cobblestone", None));
pub static CRACKED_POLISHED_BLACKSTONE_BRICKS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "cracked_polished_blackstone_bricks", None));
pub static CRACKED_STONE_BRICKS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "cracked_stone_bricks", None));
pub static CRIMSON_PLANKS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "crimson_planks", None));
pub static CUT_SANDSTONE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "cut_sandstone", None));
pub static CYAN_CONCRETE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "cyan_concrete", None));
pub static DARK_OAK_PLANKS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "dark_oak_planks", None));
pub static DEEPSLATE_BRICKS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "deepslate_bricks", None));
pub static DIORITE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "diorite", None));
pub static DIRT: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "dirt", None));
pub static END_STONE_BRICKS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "end_stone_bricks", None));
pub static FARMLAND: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "farmland", None));
pub static GLASS: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "glass_pane", None));
pub static GLOWSTONE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "glowstone", None));
pub static GRANITE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "granite", None));
pub static GRASS_BLOCK: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "grass_block", None));
pub static GRASS: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "tall_grass", None));
pub static GRAVEL: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "gravel", None));
pub static GRAY_CONCRETE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "gray_concrete", None));
pub static GRAY_TERRACOTTA: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "gray_terracotta", None));
pub static GREEN_STAINED_HARDENED_CLAY: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "green_terracotta", None));
pub static GREEN_WOOL: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "green_wool", None));
pub static HAY_BALE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "hay_block", None));
pub static IRON_BARS: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "iron_bars", None));
pub static IRON_BLOCK: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "iron_block", None));
pub static JUNGLE_PLANKS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "jungle_planks", None));
pub static LADDER: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "ladder", None));
pub static LIGHT_BLUE_CONCRETE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "light_blue_concrete", None));
pub static LIGHT_BLUE_TERRACOTTA: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "light_blue_terracotta", None));
pub static LIGHT_GRAY_CONCRETE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "light_gray_concrete", None));
pub static MOSS_BLOCK: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "moss_block", None));
pub static MOSSY_COBBLESTONE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "mossy_cobblestone", None));
pub static MUD_BRICKS: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "mud_bricks", None));
pub static NETHER_BRICK: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "nether_bricks", None));
pub static NETHER_BRICKS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "nether_bricks", None));
pub static OAK_FENCE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "oak_fence", None));
pub static OAK_LEAVES: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "oak_leaves", None));
pub static OAK_LOG: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "oak_log", None));
pub static OAK_PLANKS: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "oak_planks", None));
pub static OAK_SLAB: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "oak_slab", None));
pub static ORANGE_TERRACOTTA: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "orange_terracotta", None));
pub static PODZOL: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "podzol", None));
pub static POLISHED_ANDESITE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "polished_andesite", None));
pub static POLISHED_BASALT: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "polished_basalt", None));
pub static POLISHED_BLACKSTONE_BRICKS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "polished_blackstone_bricks", None));
pub static POLISHED_BLACKSTONE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "polished_blackstone", None));
pub static POLISHED_DEEPSLATE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "polished_deepslate", None));
pub static POLISHED_DIORITE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "polished_diorite", None));
pub static POLISHED_GRANITE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "polished_granite", None));
pub static PRISMARINE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "prismarine", None));
pub static PURPUR_BLOCK: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "purpur_block", None));
pub static PURPUR_PILLAR: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "purpur_pillar", None));
pub static QUARTZ_BRICKS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "quartz_bricks", None));
pub static RAIL: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "rail", None));
pub static RED_FLOWER: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "poppy", None));
pub static RED_NETHER_BRICKS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "red_nether_bricks", None));
pub static RED_TERRACOTTA: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "red_terracotta", None));
pub static RED_WOOL: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "red_wool", None));
pub static SAND: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "sand", None));
pub static SANDSTONE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "sandstone", None));
pub static SCAFFOLDING: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "scaffolding", None));
pub static SMOOTH_QUARTZ: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "smooth_quartz", None));
pub static SMOOTH_RED_SANDSTONE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "smooth_red_sandstone", None));
pub static SMOOTH_SANDSTONE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "smooth_sandstone", None));
pub static SMOOTH_STONE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "smooth_stone", None));
pub static SPONGE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "sponge", None));
pub static SPRUCE_LOG: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "spruce_log", None));
pub static SPRUCE_PLANKS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "spruce_planks", None));
pub static STONE_BLOCK_SLAB: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "stone_slab", None));
pub static STONE_BRICK_SLAB: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "stone_brick_slab", None));
pub static STONE_BRICKS: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "stone_bricks", None));
pub static STONE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "stone", None));
pub static TERRACOTTA: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "terracotta", None));
pub static WARPED_PLANKS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "warped_planks", None));
pub static WATER: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "water", None));
pub static WHITE_CONCRETE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "white_concrete", None));
pub static WHITE_FLOWER: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "azure_bluet", None));
pub static WHITE_STAINED_GLASS: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "white_stained_glass", None));
pub static WHITE_TERRACOTTA: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "white_terracotta", None));
pub static WHITE_WOOL: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "white_wool", None));
pub static YELLOW_CONCRETE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "yellow_concrete", None));
pub static YELLOW_FLOWER: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "dandelion", None));
pub static YELLOW_WOOL: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "yellow_wool", None));
pub static LIME_CONCRETE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "lime_concrete", None));
pub static CYAN_WOOL: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "cyan_wool", None));
pub static BLUE_CONCRETE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "blue_concrete", None));
pub static PURPLE_CONCRETE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "purple_concrete", None));
pub static RED_CONCRETE: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "red_concrete", None));
pub static MAGENTA_CONCRETE: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "magenta_concrete", None));
pub static BROWN_WOOL: Lazy<Block> = Lazy::new(|| Block::new("minecraft", "brown_wool", None));
pub static OXIDIZED_COPPER: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "oxidized_copper", None));
pub static YELLOW_TERRACOTTA: Lazy<Block> =
    Lazy::new(|| Block::new("minecraft", "yellow_terracotta", None));

pub static CARROTS: Lazy<Block> = Lazy::new(|| {
    Block::new(
        "minecraft",
        "carrots",
        Some(Value::Compound({
            let mut map = HashMap::new();
            map.insert("age".to_string(), Value::String("7".to_string()));
            map
        })),
    )
});
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
pub static POTATOES: Lazy<Block> = Lazy::new(|| {
    Block::new(
        "minecraft",
        "potatoes",
        Some(Value::Compound({
            let mut map = HashMap::new();
            map.insert("age".to_string(), Value::String("7".to_string()));
            map
        })),
    )
});
pub static WHEAT: Lazy<Block> = Lazy::new(|| {
    Block::new(
        "minecraft",
        "wheat",
        Some(Value::Compound({
            let mut map = HashMap::new();
            map.insert("age".to_string(), Value::String("7".to_string()));
            map
        })),
    )
});

// Variations for building corners
pub fn building_corner_variations() -> Vec<&'static Lazy<Block>> {
    vec![
        &STONE_BRICKS,
        &COBBLESTONE,
        &BRICK,
        &MOSSY_COBBLESTONE,
        &SANDSTONE,
        &RED_NETHER_BRICKS,
        &BLACKSTONE,
        &SMOOTH_QUARTZ,
        &CHISELED_STONE_BRICKS,
        &POLISHED_BASALT,
        &CUT_SANDSTONE,
        &POLISHED_BLACKSTONE_BRICKS,
        &ANDESITE,
        &GRANITE,
        &DIORITE,
        &CRACKED_STONE_BRICKS,
        &PRISMARINE,
        &BLUE_TERRACOTTA,
        &NETHER_BRICK,
        &QUARTZ_BRICKS,
    ]
}

// Variations for building walls
pub fn building_wall_variations() -> Vec<&'static Lazy<Block>> {
    building_wall_color_map()
        .into_iter()
        .map(|(_, block)| block)
        .collect()
}

// https://wiki.openstreetmap.org/wiki/Key:building:colour
pub fn building_wall_color_map() -> Vec<(RGBTuple, &'static Lazy<Block>)> {
    vec![
        ((233, 107, 57), &BRICK),
        ((18, 12, 13), &CRACKED_POLISHED_BLACKSTONE_BRICKS),
        ((76, 127, 153), &CYAN_CONCRETE),
        ((0, 0, 0), &DEEPSLATE_BRICKS),
        ((186, 195, 142), &END_STONE_BRICKS),
        ((57, 41, 35), &GRAY_TERRACOTTA),
        ((112, 108, 138), &LIGHT_BLUE_TERRACOTTA),
        ((122, 92, 66), &MUD_BRICKS),
        ((24, 13, 14), &NETHER_BRICKS),
        ((159, 82, 36), &ORANGE_TERRACOTTA),
        ((128, 128, 128), &POLISHED_ANDESITE),
        ((174, 173, 174), &POLISHED_DIORITE),
        ((141, 101, 142), &PURPUR_PILLAR),
        ((142, 60, 46), &RED_TERRACOTTA),
        ((153, 83, 28), &SMOOTH_RED_SANDSTONE),
        ((224, 216, 175), &SMOOTH_SANDSTONE),
        ((188, 182, 179), &SMOOTH_STONE),
        ((35, 86, 85), &WARPED_PLANKS),
        ((255, 255, 255), &WHITE_CONCRETE),
        ((209, 177, 161), &WHITE_TERRACOTTA),
        ((191, 147, 42), &YELLOW_TERRACOTTA),
    ]
}

// Variations for building floors
pub fn building_floor_variations() -> Vec<&'static Lazy<Block>> {
    building_wall_color_map()
        .into_iter()
        .map(|(_, block)| block)
        .collect()
}

pub fn building_floor_color_map() -> Vec<(RGBTuple, &'static Lazy<Block>)> {
    vec![
        ((181, 101, 59), &ACACIA_PLANKS),
        ((22, 15, 16), &BLACKSTONE),
        ((104, 51, 74), &CRIMSON_PLANKS),
        ((82, 55, 26), &DARK_OAK_PLANKS),
        ((182, 133, 99), &JUNGLE_PLANKS),
        ((33, 128, 185), &LIGHT_BLUE_CONCRETE),
        ((78, 103, 43), &MOSS_BLOCK),
        ((171, 138, 88), &OAK_PLANKS),
        ((0, 128, 0), &OXIDIZED_COPPER),
        ((18, 12, 13), &POLISHED_BLACKSTONE),
        ((64, 64, 64), &POLISHED_DEEPSLATE),
        ((255, 255, 255), &POLISHED_DIORITE),
        ((143, 96, 79), &POLISHED_GRANITE),
        ((141, 101, 142), &PURPUR_BLOCK),
        ((128, 0, 0), &RED_NETHER_BRICKS),
        ((153, 83, 28), &SMOOTH_RED_SANDSTONE),
        ((128, 96, 57), &SPRUCE_PLANKS),
        ((128, 128, 128), &STONE_BRICKS),
        ((150, 93, 68), &TERRACOTTA),
        ((35, 86, 85), &WARPED_PLANKS),
    ]
}
