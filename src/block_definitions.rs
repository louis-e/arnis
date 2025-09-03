#![allow(unused)]

use fastnbt::Value;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::colors::RGBTuple;

// Enums for stair properties
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum StairFacing {
    North,
    East,
    South,
    West,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum StairShape {
    Straight,
    InnerLeft,
    InnerRight,
    OuterLeft,
    OuterRight,
}

impl StairFacing {
    #[inline(always)]
    pub fn as_str(&self) -> &'static str {
        match self {
            StairFacing::North => "north",
            StairFacing::East => "east",
            StairFacing::South => "south",
            StairFacing::West => "west",
        }
    }
}

impl StairShape {
    #[inline(always)]
    pub fn as_str(&self) -> &'static str {
        match self {
            StairShape::Straight => "straight",
            StairShape::InnerLeft => "inner_left",
            StairShape::InnerRight => "inner_right",
            StairShape::OuterLeft => "outer_left",
            StairShape::OuterRight => "outer_right",
        }
    }
}

// Type definitions for better readability
type ColorTuple = (u8, u8, u8);
type BlockOptions = &'static [Block];
type ColorBlockMapping = (ColorTuple, BlockOptions);

#[derive(Copy, Clone, PartialEq, Eq, Ord, PartialOrd, Hash, Debug)]
pub struct Block {
    id: u8,
}

// Extended block with dynamic properties
#[derive(Clone, Debug)]
pub struct BlockWithProperties {
    pub block: Block,
    pub properties: Option<Value>,
}

impl BlockWithProperties {
    pub fn new(block: Block, properties: Option<Value>) -> Self {
        Self { block, properties }
    }

    pub fn simple(block: Block) -> Self {
        Self {
            block,
            properties: None,
        }
    }
}

impl Block {
    #[inline(always)]
    const fn new(id: u8) -> Self {
        Self { id }
    }

    #[inline(always)]
    pub fn id(&self) -> u8 {
        self.id
    }

    #[inline(always)]
    pub fn namespace(&self) -> &str {
        "minecraft"
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
            14 => "polished_blackstone_bricks",
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
            25 => "glass",
            26 => "glowstone",
            27 => "granite",
            28 => "grass_block",
            29 => "short_grass",
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
            47 => "netherite_block",
            48 => "oak_fence",
            49 => "oak_leaves",
            50 => "oak_log",
            51 => "oak_planks",
            52 => "oak_slab",
            53 => "orange_terracotta",
            54 => "podzol",
            55 => "polished_andesite",
            56 => "polished_basalt",
            57 => "quartz_block",
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
            111 => "snow_block",
            112 => "snow",
            113 => "oak_sign",
            114 => "andesite_wall",
            115 => "stone_brick_wall",
            116..=125 => "rail",
            126 => "coarse_dirt",
            127 => "iron_ore",
            128 => "coal_ore",
            129 => "gold_ore",
            130 => "copper_ore",
            131 => "clay",
            132 => "dirt_path",
            133 => "ice",
            134 => "packed_ice",
            135 => "mud",
            136 => "dead_bush",
            137..=138 => "tall_grass",
            139 => "crafting_table",
            140 => "furnace",
            141 => "white_carpet",
            142 => "bookshelf",
            143 => "oak_pressure_plate",
            144 => "oak_stairs",
            155 => "chest",
            156 => "red_carpet",
            157 => "anvil",
            158 => "note_block",
            159 => "oak_door",
            160 => "brewing_stand",
            161 => "red_bed", // North head
            162 => "red_bed", // North foot
            163 => "red_bed", // East head
            164 => "red_bed", // East foot
            165 => "red_bed", // South head
            166 => "red_bed", // South foot
            167 => "red_bed", // West head
            168 => "red_bed", // West foot
            169 => "gray_stained_glass",
            170 => "light_gray_stained_glass",
            171 => "brown_stained_glass",
            172 => "tinted_glass",
            173 => "oak_trapdoor",
            174 => "brown_concrete",
            175 => "black_terracotta",
            176 => "brown_terracotta",
            177 => "stone_brick_stairs",
            178 => "mud_brick_stairs",
            179 => "polished_blackstone_brick_stairs",
            180 => "brick_stairs",
            181 => "polished_granite_stairs",
            182 => "end_stone_brick_stairs",
            183 => "polished_diorite_stairs",
            184 => "smooth_sandstone_stairs",
            185 => "quartz_stairs",
            186 => "polished_andesite_stairs",
            187 => "nether_brick_stairs",
            _ => panic!("Invalid id"),
        }
    }

    pub fn properties(&self) -> Option<Value> {
        match self.id {
            3 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("persistent".to_string(), Value::String("true".to_string()));
                map
            })),

            49 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("persistent".to_string(), Value::String("true".to_string()));
                map
            })),

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

            113 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("rotation".to_string(), Value::String("6".to_string()));
                map.insert(
                    "waterlogged".to_string(),
                    Value::String("false".to_string()),
                );
                map
            })),

            116 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert(
                    "shape".to_string(),
                    Value::String("north_south".to_string()),
                );
                map
            })),

            117 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("shape".to_string(), Value::String("east_west".to_string()));
                map
            })),

            118 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert(
                    "shape".to_string(),
                    Value::String("ascending_east".to_string()),
                );
                map
            })),

            119 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert(
                    "shape".to_string(),
                    Value::String("ascending_west".to_string()),
                );
                map
            })),

            120 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert(
                    "shape".to_string(),
                    Value::String("ascending_north".to_string()),
                );
                map
            })),

            121 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert(
                    "shape".to_string(),
                    Value::String("ascending_south".to_string()),
                );
                map
            })),

            122 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("shape".to_string(), Value::String("north_east".to_string()));
                map
            })),

            123 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("shape".to_string(), Value::String("north_west".to_string()));
                map
            })),

            124 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("shape".to_string(), Value::String("south_east".to_string()));
                map
            })),

            125 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("shape".to_string(), Value::String("south_west".to_string()));
                map
            })),
            137 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("lower".to_string()));
                map
            })),
            138 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("upper".to_string()));
                map
            })),

            // Red bed variations by direction and part
            161 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("north".to_string()));
                map.insert("part".to_string(), Value::String("head".to_string()));
                map
            })),
            162 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("north".to_string()));
                map.insert("part".to_string(), Value::String("foot".to_string()));
                map
            })),
            163 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("east".to_string()));
                map.insert("part".to_string(), Value::String("head".to_string()));
                map
            })),
            164 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("east".to_string()));
                map.insert("part".to_string(), Value::String("foot".to_string()));
                map
            })),
            165 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("south".to_string()));
                map.insert("part".to_string(), Value::String("head".to_string()));
                map
            })),
            166 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("south".to_string()));
                map.insert("part".to_string(), Value::String("foot".to_string()));
                map
            })),
            167 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("west".to_string()));
                map.insert("part".to_string(), Value::String("head".to_string()));
                map
            })),
            168 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("west".to_string()));
                map.insert("part".to_string(), Value::String("foot".to_string()));
                map
            })),
            173 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("top".to_string()));
                map
            })),
            _ => None,
        }
    }
}

// Cache for stair blocks with properties
use std::sync::Mutex;

#[allow(clippy::type_complexity)]
static STAIR_CACHE: Lazy<Mutex<HashMap<(u8, StairFacing, StairShape), BlockWithProperties>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

// General function to create any stair block with facing and shape properties
pub fn create_stair_with_properties(
    base_stair_block: Block,
    facing: StairFacing,
    shape: StairShape,
) -> BlockWithProperties {
    let cache_key = (base_stair_block.id(), facing, shape);

    // Check cache first
    {
        let cache = STAIR_CACHE.lock().unwrap();
        if let Some(cached_block) = cache.get(&cache_key) {
            return cached_block.clone();
        }
    }

    // Create properties
    let mut map = HashMap::new();
    map.insert(
        "facing".to_string(),
        Value::String(facing.as_str().to_string()),
    );

    // Only add shape if it's not straight (default)
    if !matches!(shape, StairShape::Straight) {
        map.insert(
            "shape".to_string(),
            Value::String(shape.as_str().to_string()),
        );
    }

    let properties = Value::Compound(map);
    let block_with_props = BlockWithProperties::new(base_stair_block, Some(properties));

    // Cache the result
    {
        let mut cache = STAIR_CACHE.lock().unwrap();
        cache.insert(cache_key, block_with_props.clone());
    }

    block_with_props
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
pub const POLISHED_BLACKSTONE_BRICKS: Block = Block::new(14);
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
pub const NETHERITE_BLOCK: Block = Block::new(47);
pub const OAK_FENCE: Block = Block::new(48);
pub const OAK_LEAVES: Block = Block::new(49);
pub const OAK_LOG: Block = Block::new(50);
pub const OAK_PLANKS: Block = Block::new(51);
pub const OAK_SLAB: Block = Block::new(52);
pub const ORANGE_TERRACOTTA: Block = Block::new(53);
pub const PODZOL: Block = Block::new(54);
pub const POLISHED_ANDESITE: Block = Block::new(55);
pub const POLISHED_BASALT: Block = Block::new(56);
pub const QUARTZ_BLOCK: Block = Block::new(57);
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
pub const RED_NETHER_BRICK: Block = Block::new(68);
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
pub const SNOW_BLOCK: Block = Block::new(111);
pub const SNOW_LAYER: Block = Block::new(112);
pub const SIGN: Block = Block::new(113);
pub const ANDESITE_WALL: Block = Block::new(114);
pub const STONE_BRICK_WALL: Block = Block::new(115);
pub const CARROTS: Block = Block::new(105);
pub const DARK_OAK_DOOR_LOWER: Block = Block::new(106);
pub const DARK_OAK_DOOR_UPPER: Block = Block::new(107);
pub const POTATOES: Block = Block::new(108);
pub const WHEAT: Block = Block::new(109);
pub const BEDROCK: Block = Block::new(110);
pub const RAIL_NORTH_SOUTH: Block = Block::new(116);
pub const RAIL_EAST_WEST: Block = Block::new(117);
pub const RAIL_ASCENDING_EAST: Block = Block::new(118);
pub const RAIL_ASCENDING_WEST: Block = Block::new(119);
pub const RAIL_ASCENDING_NORTH: Block = Block::new(120);
pub const RAIL_ASCENDING_SOUTH: Block = Block::new(121);
pub const RAIL_NORTH_EAST: Block = Block::new(122);
pub const RAIL_NORTH_WEST: Block = Block::new(123);
pub const RAIL_SOUTH_EAST: Block = Block::new(124);
pub const RAIL_SOUTH_WEST: Block = Block::new(125);
pub const COARSE_DIRT: Block = Block::new(126);
pub const IRON_ORE: Block = Block::new(127);
pub const COAL_ORE: Block = Block::new(128);
pub const GOLD_ORE: Block = Block::new(129);
pub const COPPER_ORE: Block = Block::new(130);
pub const CLAY: Block = Block::new(131);
pub const DIRT_PATH: Block = Block::new(132);
pub const ICE: Block = Block::new(133);
pub const PACKED_ICE: Block = Block::new(134);
pub const MUD: Block = Block::new(135);
pub const DEAD_BUSH: Block = Block::new(136);
pub const TALL_GRASS_BOTTOM: Block = Block::new(137);
pub const TALL_GRASS_TOP: Block = Block::new(138);
pub const CRAFTING_TABLE: Block = Block::new(139);
pub const FURNACE: Block = Block::new(140);
pub const WHITE_CARPET: Block = Block::new(141);
pub const BOOKSHELF: Block = Block::new(142);
pub const OAK_PRESSURE_PLATE: Block = Block::new(143);
pub const OAK_STAIRS: Block = Block::new(144);
pub const CHEST: Block = Block::new(155);
pub const RED_CARPET: Block = Block::new(156);
pub const ANVIL: Block = Block::new(157);
pub const NOTE_BLOCK: Block = Block::new(158);
pub const OAK_DOOR: Block = Block::new(159);
pub const BREWING_STAND: Block = Block::new(160);
pub const RED_BED_NORTH_HEAD: Block = Block::new(161);
pub const RED_BED_NORTH_FOOT: Block = Block::new(162);
pub const RED_BED_EAST_HEAD: Block = Block::new(163);
pub const RED_BED_EAST_FOOT: Block = Block::new(164);
pub const RED_BED_SOUTH_HEAD: Block = Block::new(165);
pub const RED_BED_SOUTH_FOOT: Block = Block::new(166);
pub const RED_BED_WEST_HEAD: Block = Block::new(167);
pub const RED_BED_WEST_FOOT: Block = Block::new(168);
pub const GRAY_STAINED_GLASS: Block = Block::new(169);
pub const LIGHT_GRAY_STAINED_GLASS: Block = Block::new(170);
pub const BROWN_STAINED_GLASS: Block = Block::new(171);
pub const TINTED_GLASS: Block = Block::new(172);
pub const OAK_TRAPDOOR: Block = Block::new(173);
pub const BROWN_CONCRETE: Block = Block::new(174);
pub const BLACK_TERRACOTTA: Block = Block::new(175);
pub const BROWN_TERRACOTTA: Block = Block::new(176);
pub const STONE_BRICK_STAIRS: Block = Block::new(177);
pub const MUD_BRICK_STAIRS: Block = Block::new(178);
pub const POLISHED_BLACKSTONE_BRICK_STAIRS: Block = Block::new(179);
pub const BRICK_STAIRS: Block = Block::new(180);
pub const POLISHED_GRANITE_STAIRS: Block = Block::new(181);
pub const END_STONE_BRICK_STAIRS: Block = Block::new(182);
pub const POLISHED_DIORITE_STAIRS: Block = Block::new(183);
pub const SMOOTH_SANDSTONE_STAIRS: Block = Block::new(184);
pub const QUARTZ_STAIRS: Block = Block::new(185);
pub const POLISHED_ANDESITE_STAIRS: Block = Block::new(186);
pub const NETHER_BRICK_STAIRS: Block = Block::new(187);

/// Maps a block to its corresponding stair variant
#[inline]
pub fn get_stair_block_for_material(material: Block) -> Block {
    match material {
        STONE_BRICKS => STONE_BRICK_STAIRS,
        MUD_BRICKS => MUD_BRICK_STAIRS,
        OAK_PLANKS => OAK_STAIRS,
        POLISHED_ANDESITE => STONE_BRICK_STAIRS,
        SMOOTH_STONE => POLISHED_ANDESITE_STAIRS,
        OAK_PLANKS => OAK_STAIRS,
        ANDESITE => STONE_BRICK_STAIRS,
        CHISELED_STONE_BRICKS => STONE_BRICK_STAIRS,
        BLACK_TERRACOTTA => POLISHED_BLACKSTONE_BRICK_STAIRS,
        BLACKSTONE => POLISHED_BLACKSTONE_BRICK_STAIRS,
        BLUE_TERRACOTTA => MUD_BRICK_STAIRS,
        BRICK => BRICK_STAIRS,
        BROWN_CONCRETE => MUD_BRICK_STAIRS,
        BROWN_TERRACOTTA => MUD_BRICK_STAIRS,
        DEEPSLATE_BRICKS => STONE_BRICK_STAIRS,
        END_STONE_BRICKS => END_STONE_BRICK_STAIRS,
        GRAY_CONCRETE => POLISHED_BLACKSTONE_BRICK_STAIRS,
        GRAY_TERRACOTTA => MUD_BRICK_STAIRS,
        LIGHT_BLUE_TERRACOTTA => STONE_BRICK_STAIRS,
        LIGHT_GRAY_CONCRETE => STONE_BRICK_STAIRS,
        NETHER_BRICK => NETHER_BRICK_STAIRS,
        POLISHED_BLACKSTONE => POLISHED_BLACKSTONE_BRICK_STAIRS,
        POLISHED_BLACKSTONE_BRICKS => POLISHED_BLACKSTONE_BRICK_STAIRS,
        POLISHED_DEEPSLATE => STONE_BRICK_STAIRS,
        POLISHED_GRANITE => POLISHED_GRANITE_STAIRS,
        QUARTZ_BLOCK => POLISHED_DIORITE_STAIRS,
        QUARTZ_BRICKS => POLISHED_DIORITE_STAIRS,
        SANDSTONE => SMOOTH_SANDSTONE_STAIRS,
        SMOOTH_SANDSTONE => SMOOTH_SANDSTONE_STAIRS,
        WHITE_CONCRETE => QUARTZ_STAIRS,
        WHITE_TERRACOTTA => MUD_BRICK_STAIRS,
        _ => STONE_BRICK_STAIRS,
    }
}

// Window variations for different building types
pub static WINDOW_VARIATIONS: [Block; 7] = [
    GLASS,
    GRAY_STAINED_GLASS,
    LIGHT_GRAY_STAINED_GLASS,
    GRAY_STAINED_GLASS,
    BROWN_STAINED_GLASS,
    WHITE_STAINED_GLASS,
    TINTED_GLASS,
];

// Window types for different building styles
pub fn get_window_block_for_building_type(building_type: &str) -> Block {
    use rand::Rng;
    let mut rng = rand::thread_rng();

    match building_type {
        "residential" | "house" | "apartment" => {
            let residential_windows = [
                GLASS,
                WHITE_STAINED_GLASS,
                LIGHT_GRAY_STAINED_GLASS,
                BROWN_STAINED_GLASS,
            ];
            residential_windows[rng.gen_range(0..residential_windows.len())]
        }
        "hospital" | "school" | "university" => {
            let institutional_windows = [GLASS, WHITE_STAINED_GLASS, LIGHT_GRAY_STAINED_GLASS];
            institutional_windows[rng.gen_range(0..institutional_windows.len())]
        }
        "hotel" | "restaurant" => {
            let hospitality_windows = [GLASS, WHITE_STAINED_GLASS];
            hospitality_windows[rng.gen_range(0..hospitality_windows.len())]
        }
        "industrial" | "warehouse" => {
            let industrial_windows = [
                GLASS,
                GRAY_STAINED_GLASS,
                LIGHT_GRAY_STAINED_GLASS,
                BROWN_STAINED_GLASS,
            ];
            industrial_windows[rng.gen_range(0..industrial_windows.len())]
        }
        _ => WINDOW_VARIATIONS[rng.gen_range(0..WINDOW_VARIATIONS.len())],
    }
}

// Random floor block selection
pub fn get_random_floor_block() -> Block {
    use rand::Rng;
    let mut rng = rand::thread_rng();

    let floor_options = [
        WHITE_CONCRETE,
        GRAY_CONCRETE,
        LIGHT_GRAY_CONCRETE,
        POLISHED_ANDESITE,
        SMOOTH_STONE,
        STONE_BRICKS,
        MUD_BRICKS,
        OAK_PLANKS,
    ];
    floor_options[rng.gen_range(0..floor_options.len())]
}

// Define all predefined colors with their blocks
static DEFINED_COLORS: &[ColorBlockMapping] = &[
    ((233, 107, 57), &[BRICK, NETHER_BRICK]),
    (
        (18, 12, 13),
        &[POLISHED_BLACKSTONE_BRICKS, BLACKSTONE, DEEPSLATE_BRICKS],
    ),
    ((76, 127, 153), &[LIGHT_BLUE_TERRACOTTA]),
    (
        (0, 0, 0),
        &[DEEPSLATE_BRICKS, BLACKSTONE, POLISHED_BLACKSTONE],
    ),
    (
        (186, 195, 142),
        &[
            END_STONE_BRICKS,
            SANDSTONE,
            SMOOTH_SANDSTONE,
            LIGHT_GRAY_CONCRETE,
        ],
    ),
    (
        (57, 41, 35),
        &[BROWN_TERRACOTTA, BROWN_CONCRETE, MUD_BRICKS, BRICK],
    ),
    (
        (112, 108, 138),
        &[LIGHT_BLUE_TERRACOTTA, GRAY_TERRACOTTA, GRAY_CONCRETE],
    ),
    (
        (122, 92, 66),
        &[MUD_BRICKS, BROWN_TERRACOTTA, SANDSTONE, BRICK],
    ),
    ((24, 13, 14), &[NETHER_BRICK, BLACKSTONE, DEEPSLATE_BRICKS]),
    (
        (159, 82, 36),
        &[
            BROWN_TERRACOTTA,
            BRICK,
            POLISHED_GRANITE,
            BROWN_CONCRETE,
            NETHERITE_BLOCK,
            POLISHED_DEEPSLATE,
        ],
    ),
    (
        (128, 128, 128),
        &[
            POLISHED_ANDESITE,
            LIGHT_GRAY_CONCRETE,
            SMOOTH_STONE,
            STONE_BRICKS,
        ],
    ),
    (
        (174, 173, 174),
        &[
            POLISHED_ANDESITE,
            LIGHT_GRAY_CONCRETE,
            SMOOTH_STONE,
            STONE_BRICKS,
        ],
    ),
    ((141, 101, 142), &[STONE_BRICKS, BRICK, MUD_BRICKS]),
    (
        (142, 60, 46),
        &[
            BLACK_TERRACOTTA,
            NETHERITE_BLOCK,
            NETHER_BRICK,
            POLISHED_GRANITE,
            POLISHED_DEEPSLATE,
            BROWN_TERRACOTTA,
        ],
    ),
    (
        (153, 83, 28),
        &[
            BLACK_TERRACOTTA,
            POLISHED_GRANITE,
            BROWN_CONCRETE,
            BROWN_TERRACOTTA,
            STONE_BRICKS,
        ],
    ),
    (
        (224, 216, 175),
        &[
            SMOOTH_SANDSTONE,
            LIGHT_GRAY_CONCRETE,
            POLISHED_ANDESITE,
            SMOOTH_STONE,
        ],
    ),
    (
        (188, 182, 179),
        &[
            SMOOTH_SANDSTONE,
            LIGHT_GRAY_CONCRETE,
            QUARTZ_BRICKS,
            POLISHED_ANDESITE,
            SMOOTH_STONE,
        ],
    ),
    (
        (35, 86, 85),
        &[
            POLISHED_BLACKSTONE_BRICKS,
            BLUE_TERRACOTTA,
            LIGHT_BLUE_TERRACOTTA,
        ],
    ),
    (
        (255, 255, 255),
        &[WHITE_CONCRETE, QUARTZ_BRICKS, QUARTZ_BLOCK],
    ),
    (
        (209, 177, 161),
        &[
            WHITE_TERRACOTTA,
            SMOOTH_SANDSTONE,
            SMOOTH_STONE,
            SANDSTONE,
            LIGHT_GRAY_CONCRETE,
        ],
    ),
    ((191, 147, 42), &[SMOOTH_SANDSTONE, SANDSTONE, SMOOTH_STONE]),
];

// Function to randomly select building wall block with alternatives
pub fn get_building_wall_block_for_color(color: RGBTuple) -> Block {
    use rand::Rng;
    let mut rng = rand::thread_rng();

    // Find the closest color match
    let closest_color = DEFINED_COLORS
        .iter()
        .min_by_key(|(defined_color, _)| crate::colors::rgb_distance(&color, defined_color));

    if let Some((_, options)) = closest_color {
        options[rng.gen_range(0..options.len())]
    } else {
        // This should never happen, but fallback just in case
        get_fallback_building_block()
    }
}

// Function to get a random fallback building block when no color attribute is specified
pub fn get_fallback_building_block() -> Block {
    use rand::Rng;
    let mut rng = rand::thread_rng();

    let fallback_options = [
        BLACKSTONE,
        BLACK_TERRACOTTA,
        BRICK,
        BROWN_CONCRETE,
        BROWN_TERRACOTTA,
        DEEPSLATE_BRICKS,
        END_STONE_BRICKS,
        GRAY_CONCRETE,
        GRAY_TERRACOTTA,
        LIGHT_BLUE_TERRACOTTA,
        LIGHT_GRAY_CONCRETE,
        MUD_BRICKS,
        NETHER_BRICK,
        POLISHED_ANDESITE,
        POLISHED_BLACKSTONE,
        POLISHED_BLACKSTONE_BRICKS,
        POLISHED_DEEPSLATE,
        POLISHED_GRANITE,
        QUARTZ_BLOCK,
        QUARTZ_BRICKS,
        SANDSTONE,
        SMOOTH_SANDSTONE,
        SMOOTH_STONE,
        STONE_BRICKS,
        WHITE_CONCRETE,
        WHITE_TERRACOTTA,
        OAK_PLANKS,
    ];
    fallback_options[rng.gen_range(0..fallback_options.len())]
}

// Function to get a random castle wall block
pub fn get_castle_wall_block() -> Block {
    use rand::Rng;
    let mut rng = rand::thread_rng();

    let castle_wall_options = [
        STONE_BRICKS,
        CHISELED_STONE_BRICKS,
        CRACKED_STONE_BRICKS,
        COBBLESTONE,
        MOSSY_COBBLESTONE,
        DEEPSLATE_BRICKS,
        POLISHED_ANDESITE,
        ANDESITE,
        SMOOTH_STONE,
        BRICK,
    ];
    castle_wall_options[rng.gen_range(0..castle_wall_options.len())]
}
