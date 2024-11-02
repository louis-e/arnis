#![allow(unused)]

use fastnbt::Value;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::colors::RGBTuple;

#[derive(Copy, Clone, PartialEq, Eq, Ord, PartialOrd, Hash, Debug)]
pub struct Block {
    id: u8,
}

impl Block {
    const fn new(id: u8) -> Self {
        Self { id }
    }

    pub fn id(&self) -> u8 {
        self.id
    }

    pub fn namespace(&self) -> &str {
        "mincraft"
    }

    pub fn name(&self) -> &str {
        match self.id {
            0 => "acacia_planks",
            1 => "air",
            2 => "andesite",
            3 => "birch_leaves",
            4 => "birch_log",
            5 => "black_concrete",
            6 => "blackstone",
            7 => "blue_orchid",
            8 => "blue_terracotta",
            9 => "bricks",
            10 => "cauldron",
            11 => "chiseled_stone_bricks",
            12 => "cobblestone_wall",
            13 => "cobblestone",
            14 => "cracked_polished_blackstone_bricks",
            15 => "cracked_stone_bricks",
            16 => "crimson_planks",
            17 => "cut_sandstone",
            18 => "cyan_concrete",
            19 => "dark_oak_planks",
            20 => "deepslate_bricks",
            21 => "diorite",
            22 => "dirt",
            23 => "end_stone_bricks",
            24 => "farmland",
            25 => "glass_pane",
            26 => "glowstone",
            27 => "granite",
            28 => "grass_block",
            29 => "tall_grass",
            30 => "gravel",
            31 => "gray_concrete",
            32 => "gray_terracotta",
            33 => "green_terracotta",
            34 => "green_wool",
            35 => "hay_block",
            36 => "iron_bars",
            37 => "iron_block",
            38 => "jungle_planks",
            39 => "ladder",
            40 => "light_blue_concrete",
            41 => "light_blue_terracotta",
            42 => "light_gray_concrete",
            43 => "moss_block",
            44 => "mossy_cobblestone",
            45 => "mud_bricks",
            46 => "nether_bricks",
            47 => "nether_bricks",
            48 => "oak_fence",
            49 => "oak_leaves",
            50 => "oak_log",
            51 => "oak_planks",
            52 => "oak_slab",
            53 => "orange_terracotta",
            54 => "podzol",
            55 => "polished_andesite",
            56 => "polished_basalt",
            57 => "polished_blackstone_bricks",
            58 => "polished_blackstone",
            59 => "polished_deepslate",
            60 => "polished_diorite",
            61 => "polished_granite",
            62 => "prismarine",
            63 => "purpur_block",
            64 => "purpur_pillar",
            65 => "quartz_bricks",
            66 => "rail",
            67 => "poppy",
            68 => "red_nether_bricks",
            69 => "red_terracotta",
            70 => "red_wool",
            71 => "sand",
            72 => "sandstone",
            73 => "scaffolding",
            74 => "smooth_quartz",
            75 => "smooth_red_sandstone",
            76 => "smooth_sandstone",
            77 => "smooth_stone",
            78 => "sponge",
            79 => "spruce_log",
            80 => "spruce_planks",
            81 => "stone_slab",
            82 => "stone_brick_slab",
            83 => "stone_bricks",
            84 => "stone",
            85 => "terracotta",
            86 => "warped_planks",
            87 => "water",
            88 => "white_concrete",
            89 => "azure_bluet",
            90 => "white_stained_glass",
            91 => "white_terracotta",
            92 => "white_wool",
            93 => "yellow_concrete",
            94 => "dandelion",
            95 => "yellow_wool",
            96 => "lime_concrete",
            97 => "cyan_wool",
            98 => "blue_concrete",
            99 => "purple_concrete",
            100 => "red_concrete",
            101 => "magenta_concrete",
            102 => "brown_wool",
            103 => "oxidized_copper",
            104 => "yellow_terracotta",
            105 => "carrots",
            106 => "dark_oak_door",
            107 => "dark_oak_door",
            108 => "potatoes",
            109 => "wheat",
            110 => "bedrock",
            _ => panic!("Invalid id"),
        }
    }

    pub fn properties(&self) -> Option<Value> {
        match self.id {
            105 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("age".to_string(), Value::String("7".to_string()));
                map
            })),

            106 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("half".to_string(), Value::String("lower".to_string()));
                map
            })),

            107 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("half".to_string(), Value::String("upper".to_string()));
                map
            })),

            108 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("age".to_string(), Value::String("7".to_string()));
                map
            })),

            109 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("age".to_string(), Value::String("7".to_string()));
                map
            })),

            _ => None,
        }
    }
}

// Lazy static blocks
pub const ACACIA_PLANKS: Block = Block::new(0);
pub const AIR: Block = Block::new(1);
pub const ANDESITE: Block = Block::new(2);
pub const BIRCH_LEAVES: Block = Block::new(3);
pub const BIRCH_LOG: Block = Block::new(4);
pub const BLACK_CONCRETE: Block = Block::new(5);
pub const BLACKSTONE: Block = Block::new(6);
pub const BLUE_FLOWER: Block = Block::new(7);
pub const BLUE_TERRACOTTA: Block = Block::new(8);
pub const BRICK: Block = Block::new(9);
pub const CAULDRON: Block = Block::new(10);
pub const CHISELED_STONE_BRICKS: Block = Block::new(11);
pub const COBBLESTONE_WALL: Block = Block::new(12);
pub const COBBLESTONE: Block = Block::new(13);
pub const CRACKED_POLISHED_BLACKSTONE_BRICKS: Block = Block::new(14);
pub const CRACKED_STONE_BRICKS: Block = Block::new(15);
pub const CRIMSON_PLANKS: Block = Block::new(16);
pub const CUT_SANDSTONE: Block = Block::new(17);
pub const CYAN_CONCRETE: Block = Block::new(18);
pub const DARK_OAK_PLANKS: Block = Block::new(19);
pub const DEEPSLATE_BRICKS: Block = Block::new(20);
pub const DIORITE: Block = Block::new(21);
pub const DIRT: Block = Block::new(22);
pub const END_STONE_BRICKS: Block = Block::new(23);
pub const FARMLAND: Block = Block::new(24);
pub const GLASS: Block = Block::new(25);
pub const GLOWSTONE: Block = Block::new(26);
pub const GRANITE: Block = Block::new(27);
pub const GRASS_BLOCK: Block = Block::new(28);
pub const GRASS: Block = Block::new(29);
pub const GRAVEL: Block = Block::new(30);
pub const GRAY_CONCRETE: Block = Block::new(31);
pub const GRAY_TERRACOTTA: Block = Block::new(32);
pub const GREEN_STAINED_HARDENED_CLAY: Block = Block::new(33);
pub const GREEN_WOOL: Block = Block::new(34);
pub const HAY_BALE: Block = Block::new(35);
pub const IRON_BARS: Block = Block::new(36);
pub const IRON_BLOCK: Block = Block::new(37);
pub const JUNGLE_PLANKS: Block = Block::new(38);
pub const LADDER: Block = Block::new(39);
pub const LIGHT_BLUE_CONCRETE: Block = Block::new(40);
pub const LIGHT_BLUE_TERRACOTTA: Block = Block::new(41);
pub const LIGHT_GRAY_CONCRETE: Block = Block::new(42);
pub const MOSS_BLOCK: Block = Block::new(43);
pub const MOSSY_COBBLESTONE: Block = Block::new(44);
pub const MUD_BRICKS: Block = Block::new(45);
pub const NETHER_BRICK: Block = Block::new(46);
pub const NETHER_BRICKS: Block = Block::new(47);
pub const OAK_FENCE: Block = Block::new(48);
pub const OAK_LEAVES: Block = Block::new(49);
pub const OAK_LOG: Block = Block::new(50);
pub const OAK_PLANKS: Block = Block::new(51);
pub const OAK_SLAB: Block = Block::new(52);
pub const ORANGE_TERRACOTTA: Block = Block::new(53);
pub const PODZOL: Block = Block::new(54);
pub const POLISHED_ANDESITE: Block = Block::new(55);
pub const POLISHED_BASALT: Block = Block::new(56);
pub const POLISHED_BLACKSTONE_BRICKS: Block = Block::new(57);
pub const POLISHED_BLACKSTONE: Block = Block::new(58);
pub const POLISHED_DEEPSLATE: Block = Block::new(59);
pub const POLISHED_DIORITE: Block = Block::new(60);
pub const POLISHED_GRANITE: Block = Block::new(61);
pub const PRISMARINE: Block = Block::new(62);
pub const PURPUR_BLOCK: Block = Block::new(63);
pub const PURPUR_PILLAR: Block = Block::new(64);
pub const QUARTZ_BRICKS: Block = Block::new(65);
pub const RAIL: Block = Block::new(66);
pub const RED_FLOWER: Block = Block::new(67);
pub const RED_NETHER_BRICKS: Block = Block::new(68);
pub const RED_TERRACOTTA: Block = Block::new(69);
pub const RED_WOOL: Block = Block::new(70);
pub const SAND: Block = Block::new(71);
pub const SANDSTONE: Block = Block::new(72);
pub const SCAFFOLDING: Block = Block::new(73);
pub const SMOOTH_QUARTZ: Block = Block::new(74);
pub const SMOOTH_RED_SANDSTONE: Block = Block::new(75);
pub const SMOOTH_SANDSTONE: Block = Block::new(76);
pub const SMOOTH_STONE: Block = Block::new(77);
pub const SPONGE: Block = Block::new(78);
pub const SPRUCE_LOG: Block = Block::new(79);
pub const SPRUCE_PLANKS: Block = Block::new(80);
pub const STONE_BLOCK_SLAB: Block = Block::new(81);
pub const STONE_BRICK_SLAB: Block = Block::new(82);
pub const STONE_BRICKS: Block = Block::new(83);
pub const STONE: Block = Block::new(84);
pub const TERRACOTTA: Block = Block::new(85);
pub const WARPED_PLANKS: Block = Block::new(86);
pub const WATER: Block = Block::new(87);
pub const WHITE_CONCRETE: Block = Block::new(88);
pub const WHITE_FLOWER: Block = Block::new(89);
pub const WHITE_STAINED_GLASS: Block = Block::new(90);
pub const WHITE_TERRACOTTA: Block = Block::new(91);
pub const WHITE_WOOL: Block = Block::new(92);
pub const YELLOW_CONCRETE: Block = Block::new(93);
pub const YELLOW_FLOWER: Block = Block::new(94);
pub const YELLOW_WOOL: Block = Block::new(95);
pub const LIME_CONCRETE: Block = Block::new(96);
pub const CYAN_WOOL: Block = Block::new(97);
pub const BLUE_CONCRETE: Block = Block::new(98);
pub const PURPLE_CONCRETE: Block = Block::new(99);
pub const RED_CONCRETE: Block = Block::new(100);
pub const MAGENTA_CONCRETE: Block = Block::new(101);
pub const BROWN_WOOL: Block = Block::new(102);
pub const OXIDIZED_COPPER: Block = Block::new(103);
pub const YELLOW_TERRACOTTA: Block = Block::new(104);

pub const CARROTS: Block = Block::new(105);
pub const DARK_OAK_DOOR_LOWER: Block = Block::new(106);
pub const DARK_OAK_DOOR_UPPER: Block = Block::new(107);
pub const POTATOES: Block = Block::new(108);
pub const WHEAT: Block = Block::new(109);

pub const BEDROCK: Block = Block::new(110);

// Variations for building corners
pub fn building_corner_variations() -> Vec<Block> {
    vec![
        STONE_BRICKS,
        COBBLESTONE,
        BRICK,
        MOSSY_COBBLESTONE,
        SANDSTONE,
        RED_NETHER_BRICKS,
        BLACKSTONE,
        SMOOTH_QUARTZ,
        CHISELED_STONE_BRICKS,
        POLISHED_BASALT,
        CUT_SANDSTONE,
        POLISHED_BLACKSTONE_BRICKS,
        ANDESITE,
        GRANITE,
        DIORITE,
        CRACKED_STONE_BRICKS,
        PRISMARINE,
        BLUE_TERRACOTTA,
        NETHER_BRICK,
        QUARTZ_BRICKS,
    ]
}

// Variations for building walls
pub fn building_wall_variations() -> Vec<Block> {
    building_wall_color_map()
        .into_iter()
        .map(|(_, block)| block)
        .collect()
}

// https://wiki.openstreetmap.org/wiki/Key:building:colour
pub fn building_wall_color_map() -> Vec<(RGBTuple, Block)> {
    vec![
        ((233, 107, 57), BRICK),
        ((18, 12, 13), CRACKED_POLISHED_BLACKSTONE_BRICKS),
        ((76, 127, 153), CYAN_CONCRETE),
        ((0, 0, 0), DEEPSLATE_BRICKS),
        ((186, 195, 142), END_STONE_BRICKS),
        ((57, 41, 35), GRAY_TERRACOTTA),
        ((112, 108, 138), LIGHT_BLUE_TERRACOTTA),
        ((122, 92, 66), MUD_BRICKS),
        ((24, 13, 14), NETHER_BRICKS),
        ((159, 82, 36), ORANGE_TERRACOTTA),
        ((128, 128, 128), POLISHED_ANDESITE),
        ((174, 173, 174), POLISHED_DIORITE),
        ((141, 101, 142), PURPUR_PILLAR),
        ((142, 60, 46), RED_TERRACOTTA),
        ((153, 83, 28), SMOOTH_RED_SANDSTONE),
        ((224, 216, 175), SMOOTH_SANDSTONE),
        ((188, 182, 179), SMOOTH_STONE),
        ((35, 86, 85), WARPED_PLANKS),
        ((255, 255, 255), WHITE_CONCRETE),
        ((209, 177, 161), WHITE_TERRACOTTA),
        ((191, 147, 42), YELLOW_TERRACOTTA),
    ]
}

// Variations for building floors
pub fn building_floor_variations() -> Vec<Block> {
    building_wall_color_map()
        .into_iter()
        .map(|(_, block)| block)
        .collect()
}

pub fn building_floor_color_map() -> Vec<(RGBTuple, Block)> {
    vec![
        ((181, 101, 59), ACACIA_PLANKS),
        ((22, 15, 16), BLACKSTONE),
        ((104, 51, 74), CRIMSON_PLANKS),
        ((82, 55, 26), DARK_OAK_PLANKS),
        ((182, 133, 99), JUNGLE_PLANKS),
        ((33, 128, 185), LIGHT_BLUE_CONCRETE),
        ((78, 103, 43), MOSS_BLOCK),
        ((171, 138, 88), OAK_PLANKS),
        ((0, 128, 0), OXIDIZED_COPPER),
        ((18, 12, 13), POLISHED_BLACKSTONE),
        ((64, 64, 64), POLISHED_DEEPSLATE),
        ((255, 255, 255), POLISHED_DIORITE),
        ((143, 96, 79), POLISHED_GRANITE),
        ((141, 101, 142), PURPUR_BLOCK),
        ((128, 0, 0), RED_NETHER_BRICKS),
        ((153, 83, 28), SMOOTH_RED_SANDSTONE),
        ((128, 96, 57), SPRUCE_PLANKS),
        ((128, 128, 128), STONE_BRICKS),
        ((150, 93, 68), TERRACOTTA),
        ((35, 86, 85), WARPED_PLANKS),
    ]
}
