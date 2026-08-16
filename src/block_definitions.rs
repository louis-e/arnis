#![allow(unused)]

use fastnbt::Value;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

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

#[derive(Copy, Clone, PartialEq, Eq, Ord, PartialOrd, Hash, Debug)]
pub struct Block {
    id: u16,
}

/// Block with NBT properties shared via Arc so identical compounds reuse one allocation.
#[derive(Clone, Debug)]
pub struct BlockWithProperties {
    pub block: Block,
    pub properties: Option<Arc<Value>>,
}

impl BlockWithProperties {
    pub fn new(block: Block, properties: Option<Value>) -> Self {
        Self {
            block,
            properties: properties.map(Arc::new),
        }
    }

    pub fn from_arc(block: Block, properties: Option<Arc<Value>>) -> Self {
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
    const fn new(id: u16) -> Self {
        Self { id }
    }

    #[inline(always)]
    pub fn id(&self) -> u16 {
        self.id
    }

    /// Rebuild a block from a raw id, for the packed section storage.
    #[inline(always)]
    pub(crate) const fn from_raw_id(id: u16) -> Self {
        Self::new(id)
    }

    #[inline(always)]
    pub fn namespace(&self) -> &str {
        "minecraft"
    }

    pub fn name(&self) -> &str {
        self.try_name().expect("Invalid id")
    }

    /// Non-panicking variant of `name` (None for unassigned ids).
    pub fn try_name(&self) -> Option<&str> {
        Some(match self.id {
            0 => "mangrove_log",
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
            16 => "cyan_concrete",
            17 => "cobblestone_stairs",
            18 => "magma_block",
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
            38 => "waxed_cut_copper_stairs",
            39 => "yellow_concrete",
            40 => "snow",
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
            56 => "mossy_stone_brick_stairs",
            57 => "quartz_block",
            58 => "polished_blackstone",
            59 => "polished_deepslate",
            60 => "polished_diorite",
            61 => "polished_granite",
            62 => "mossy_cobblestone_stairs",
            63 => "deepslate_brick_stairs",
            64 => "polished_deepslate_stairs",
            65 => "quartz_bricks",
            66 => "kelp",
            67 => "poppy",
            68 => "red_nether_bricks",
            69 => "red_terracotta",
            70 => "tall_seagrass",
            71 => "sand",
            72 => "sandstone",
            73 => "scaffolding",
            74 => "smooth_quartz",
            75 => "spruce_stairs",
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
            86 => "dark_oak_stairs",
            87 => "water",
            88 => "white_concrete",
            89 => "azure_bluet",
            90 => "white_stained_glass",
            91 => "white_terracotta",
            92 => "white_wool",
            93 => "tall_seagrass",
            94 => "dandelion",
            95 => "sea_pickle",
            96 => "soul_sand",
            97 => "red_nether_brick_stairs",
            98 => "sandstone_wall",
            99 => "cut_sandstone_slab",
            100 => "red_concrete",
            101 => "iron_trapdoor",
            102 => "waxed_oxidized_cut_copper_stairs",
            103 => "waxed_oxidized_copper",
            104 => "yellow_terracotta",
            105 => "carrots",
            106..=107 => "dark_oak_door",
            108 => "potatoes",
            109 => "wheat",
            110 => "bedrock",
            111 => "snow_block",
            112 => "andesite_stairs",
            113 => "jungle_trapdoor",
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
            133 => "waxed_exposed_cut_copper_stairs",
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
            145 => "orange_concrete",
            146 => "purple_concrete",
            147 => "birch_fence_gate",
            148 => "dark_oak_fence_gate",
            149 => "light_blue_concrete",
            150 => "mossy_stone_bricks",
            151 => "deepslate",
            152 => "tuff",
            153 => "cobbled_deepslate",
            154 => "lantern",
            155 => "chest",
            156 => "stone_button",
            157 => "anvil",
            158 => "note_block",
            159 => "polished_deepslate_wall",
            160 => "brewing_stand",
            161 => "red_bed", // North head
            162 => "red_bed", // North foot
            163 => "black_stained_glass",
            164 => "polished_andesite_slab",
            165 => "red_bed", // South head
            166 => "red_bed", // South foot
            167 => "red_bed", // West head
            168 => "red_bed", // West foot
            169 => "gray_stained_glass",
            170 => "light_gray_stained_glass",
            171 => "brown_stained_glass",
            172 => "tinted_glass",
            173 => "magenta_concrete",
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
            188 => "barrel",
            189 => "fern",
            190 => "lime_concrete",
            191 => "blue_concrete",
            192 => "gray_stained_glass_pane",
            193 => "oak_fence_gate",
            194 => "spruce_fence_gate",
            195 => "waxed_copper_block",
            196 => "glass_pane",
            197..=198 => "large_fern",
            199 => "waxed_exposed_copper",
            200 => "stone_stairs",
            201 => "lightning_rod",
            202 => "flower_pot",
            203 => "sea_lantern",
            204 => "waxed_exposed_chiseled_copper",
            205 => "warped_slab",
            206 => "warped_stairs",
            207 => "green_concrete",
            208 => "brick_wall",
            209 => "redstone_block",
            210 => "chain",
            211 => "warped_trapdoor",
            212 => "stripped_warped_stem",
            213 => "stripped_warped_hyphae",
            214 => "smooth_stone_slab",
            215 => "waxed_exposed_cut_copper",
            216 => "light_gray_terracotta",
            217 => "oak_slab",
            218 => "redstone_lamp",
            219 => "dark_oak_log",
            220 => "dark_oak_leaves",
            221 => "jungle_log",
            222 => "jungle_leaves",
            223 => "acacia_log",
            224 => "acacia_leaves",
            225 => "spruce_leaves",
            226 => "cyan_stained_glass",
            227 => "blue_stained_glass",
            228 => "light_blue_stained_glass",
            229 => "daylight_detector",
            230 => "cherry_log",
            231 => "cherry_leaves",
            232 => "brown_concrete_powder",
            233 => "mangrove_leaves",
            234 => "azalea_leaves",
            235 => "potted_poppy",
            236 => "oak_trapdoor",
            237 => "sugar_cane",
            238 => "seagrass",
            239 => "kelp_plant",
            240 => "quartz_slab",
            241 => "dark_oak_trapdoor",
            242 => "spruce_trapdoor",
            243 => "birch_trapdoor",
            244 => "mud_brick_slab",
            245 => "brick_slab",
            246 => "potted_red_tulip",
            247 => "potted_dandelion",
            248 => "potted_blue_orchid",
            249 => "diamond_ore",
            250 => "redstone_ore",
            251 => "lapis_ore",
            252 => "gray_concrete_powder",
            253 => "cyan_terracotta",
            254 => "black_wool",
            255 => "light_gray_wall_banner",
            256 => "lever",
            257 => "grindstone",
            258 => "rail",
            259 => "red_wool",
            260 => "ladder",
            261 => "yellow_wool",
            265 => "cobblestone_slab",
            266 => "nether_brick_fence",
            267 => "birch_fence",
            268 => "smooth_quartz_slab",
            269 => "smooth_quartz_stairs",
            270 => "blackstone_stairs",
            271 => "blackstone_wall",
            272 => "diorite_wall",
            273 => "polished_deepslate_slab",
            274 => "oak_sign",
            275 => "blue_wall_banner",
            276 => "jungle_fence",
            277 => "black_wall_banner",
            278 => "red_wall_banner",
            279 => "birch_door",
            280 => "birch_pressure_plate",
            281 => "stone_pressure_plate",
            282 => "blast_furnace",
            283 => "dispenser",
            284 => "hopper",
            285 => "green_wall_banner",
            286 => "water_cauldron",
            287 => "lodestone",
            288 => "redstone_torch",
            289 => "red_carpet",
            290 => "chiseled_polished_blackstone",
            291 => "mossy_stone_brick_wall",
            292 => "bamboo_stairs",
            293 => "oak_door",
            294 => "red_bed", // East head
            295 => "red_bed", // East foot
            296 => "end_stone_brick_wall",
            297 => "bamboo_slab",
            298 => "chiseled_deepslate",
            299 => "oak_trapdoor",
            300 => "birch_button",
            301 => "cobweb",
            302 => "dark_oak_slab",
            303 => "jungle_slab",
            304 => "jungle_stairs",
            305 => "chiseled_bookshelf",
            306 => "oak_button",
            307 => "powered_rail",
            308 => "spruce_fence",
            309 => "spruce_slab",
            310 => "andesite_slab",
            311 => "cobbled_deepslate_slab",
            312 => "cobbled_deepslate_stairs",
            313 => "dark_oak_fence",
            314 => "dark_oak_pressure_plate",
            316 => "chiseled_bookshelf",
            317 => "gray_wall_banner",
            318 => "gray_wool",
            319 => "nether_wart_block",
            320 => "chiseled_bookshelf",
            321 => "polished_basalt",
            322 => "polished_blackstone_button",
            323 => "polished_blackstone_pressure_plate",
            324 => "red_nether_brick_slab",
            325 => "spruce_button",
            326 => "chiseled_bookshelf",
            327 => "acacia_trapdoor",
            328 => "composter",
            329 => "cyan_carpet",
            330 => "dark_oak_button",
            331 => "end_stone_brick_slab",
            332 => "damaged_anvil",
            333 => "green_carpet",
            334 => "light_blue_carpet",
            335 => "nether_brick_wall",
            336 => "smoker",
            337 => "smooth_red_sandstone",
            338 => "smooth_red_sandstone_slab",
            339 => "blue_stained_glass_pane",
            340 => "cyan_wool",
            341 => "light_gray_carpet",
            342 => "mossy_cobblestone_slab",
            343 => "mossy_stone_brick_slab",
            344 => "prismarine",
            345 => "end_rod",
            346 => "tripwire_hook",
            347 => "spruce_wall_sign",
            348 => "granite_stairs",
            349 => "diorite_stairs",
            350 => "deepslate_tiles",
            351 => "deepslate_tile_slab",
            352 => "deepslate_tile_wall",
            353 => "polished_blackstone_slab",
            354 => "polished_diorite_slab",
            355 => "soul_lantern",
            356 => "chiseled_quartz_block",
            357 => "quartz_pillar",
            358 => "redstone_wall_torch",
            359 => "gold_block",
            360 => "orange_wool",
            361 => "blue_wool",
            362 => "chain",
            363 => "white_wall_banner",
            364..=365 => "spruce_door",
            366 => "oak_door",
            _ => return None,
        })
        // Note: ids are u16, but the split at BYTE_ID_LIMIT is load-bearing --
        // see the comment above the constant list before picking an id.
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
            // Tall seagrass lower/upper halves.
            70 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("lower".to_string()));
                map
            })),
            93 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("upper".to_string()));
                map
            })),
            // Waterlogged sea pickle cluster.
            95 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("pickles".to_string(), Value::String("2".to_string()));
                map.insert("waterlogged".to_string(), Value::String("true".to_string()));
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
                map.insert("occupied".to_string(), Value::String("false".to_string()));
                map
            })),
            162 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("north".to_string()));
                map.insert("part".to_string(), Value::String("foot".to_string()));
                map.insert("occupied".to_string(), Value::String("false".to_string()));
                map
            })),
            165 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("south".to_string()));
                map.insert("part".to_string(), Value::String("head".to_string()));
                map.insert("occupied".to_string(), Value::String("false".to_string()));
                map
            })),
            166 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("south".to_string()));
                map.insert("part".to_string(), Value::String("foot".to_string()));
                map.insert("occupied".to_string(), Value::String("false".to_string()));
                map
            })),
            167 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("west".to_string()));
                map.insert("part".to_string(), Value::String("head".to_string()));
                map.insert("occupied".to_string(), Value::String("false".to_string()));
                map
            })),
            168 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("west".to_string()));
                map.insert("part".to_string(), Value::String("foot".to_string()));
                map.insert("occupied".to_string(), Value::String("false".to_string()));
                map
            })),
            197 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("lower".to_string()));
                map
            })),
            198 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("upper".to_string()));
                map
            })),
            210 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("axis".to_string(), Value::String("x".to_string()));
                map
            })),
            // Smooth stone slab (bottom by default)
            214 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("type".to_string(), Value::String("bottom".to_string()));
                map
            })),
            // Oak slab top
            217 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("type".to_string(), Value::String("top".to_string()));
                map
            })),
            // Dark oak leaves
            220 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("persistent".to_string(), Value::String("true".to_string()));
                map
            })),
            // Jungle leaves
            222 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("persistent".to_string(), Value::String("true".to_string()));
                map
            })),
            // Acacia leaves
            224 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("persistent".to_string(), Value::String("true".to_string()));
                map
            })),
            // Spruce leaves
            225 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("persistent".to_string(), Value::String("true".to_string()));
                map
            })),
            231 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("persistent".to_string(), Value::String("true".to_string()));
                map
            })),
            // Open oak trapdoor facing north (hangs flat against wall, looks like shutter)
            236 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("facing".to_string(), Value::String("north".to_string()));
                map.insert("open".to_string(), Value::String("true".to_string()));
                map.insert("half".to_string(), Value::String("top".to_string()));
                map
            })),
            // Quartz slab (top half) used as window sill
            240 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("type".to_string(), Value::String("top".to_string()));
                map
            })),
            274 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("rotation".to_string(), Value::String("6".to_string()));
                map.insert(
                    "waterlogged".to_string(),
                    Value::String("false".to_string()),
                );
                map
            })),

            // Oak door lower
            293 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("lower".to_string()));
                map
            })),
            294 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("east".to_string()));
                map.insert("part".to_string(), Value::String("head".to_string()));
                map.insert("occupied".to_string(), Value::String("false".to_string()));
                map
            })),
            295 => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("facing".to_string(), Value::String("east".to_string()));
                map.insert("part".to_string(), Value::String("foot".to_string()));
                map.insert("occupied".to_string(), Value::String("false".to_string()));
                map
            })),
            299 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("top".to_string()));
                map
            })),
            305 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("facing".to_string(), Value::String("north".to_string()));
                map
            })),
            316 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("facing".to_string(), Value::String("east".to_string()));
                map
            })),
            320 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("facing".to_string(), Value::String("south".to_string()));
                map
            })),
            326 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("facing".to_string(), Value::String("west".to_string()));
                map
            })),
            362 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("axis".to_string(), Value::String("z".to_string()));
                map
            })),
            // Spruce door lower
            364 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("lower".to_string()));
                map
            })),
            // Spruce door upper
            365 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("upper".to_string()));
                map
            })),
            // Oak door upper
            366 => Some(Value::Compound({
                let mut map = HashMap::new();
                map.insert("half".to_string(), Value::String("upper".to_string()));
                map
            })),

            _ => None,
        }
    }
}

// Cache of stair NBT compounds shared across placements via Arc.
use std::sync::Mutex;

#[allow(clippy::type_complexity)]
static STAIR_CACHE: Lazy<Mutex<HashMap<(u16, StairFacing, StairShape), Arc<Value>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

// General function to create any stair block with facing and shape properties
pub fn create_stair_with_properties(
    base_stair_block: Block,
    facing: StairFacing,
    shape: StairShape,
) -> BlockWithProperties {
    let cache_key = (base_stair_block.id(), facing, shape);

    {
        let cache = STAIR_CACHE.lock().unwrap();
        if let Some(cached_props) = cache.get(&cache_key) {
            return BlockWithProperties::from_arc(base_stair_block, Some(cached_props.clone()));
        }
    }

    let mut map = HashMap::new();
    map.insert(
        "facing".to_string(),
        Value::String(facing.as_str().to_string()),
    );
    if !matches!(shape, StairShape::Straight) {
        map.insert(
            "shape".to_string(),
            Value::String(shape.as_str().to_string()),
        );
    }

    let properties = Arc::new(Value::Compound(map));
    {
        let mut cache = STAIR_CACHE.lock().unwrap();
        cache.insert(cache_key, properties.clone());
    }

    BlockWithProperties::from_arc(base_stair_block, Some(properties))
}
// Add half=top to make it upside-down.
pub fn top_stair(mut stair: BlockWithProperties) -> BlockWithProperties {
    if let Some(props) = stair.properties.as_ref() {
        if let Value::Compound(map) = props.as_ref() {
            let mut new_map = map.clone();
            new_map.insert("half".to_string(), Value::String("top".to_string()));
            stair.properties = Some(Arc::new(Value::Compound(new_map)));
        }
    }
    stair
}

/// Ids below this are stored one byte per cell; a single id at or above it
/// widens the whole 16x16x16 section to two bytes per cell.
///
/// See [`crate::world_editor::BlockStorage`], which keeps the narrow `Full`
/// representation only while every id in the section stays under this limit.
pub const BYTE_ID_LIMIT: u16 = 256;

// Lazy static blocks
//
// The id space is split at BYTE_ID_LIMIT, and the split is deliberate:
//
//   0..255   blocks the world generator places in bulk -- terrain, land cover,
//            climate, ores, roads, building shells and their fittings.
//   256..    the long decorative tail: blocks that only arrive through a
//            bundled .schem prop, a one-off landmark, or a rare interior
//            detail, plus entries kept solely so a block name resolves.
//
// A section pays for the wide representation as soon as it holds one id from
// the upper range, so a single common block sitting up there costs 4 KB in
// every section it touches. Measured over seven sample areas, keeping the
// split honest holds the wide-section rate near 0.5%; letting it drift took it
// to 8.7% (up to 26% in dense cities), roughly 90 MB of avoidable peak RAM.
//
// When adding a block: give it a low id only if the generator can emit it
// across many chunks. Props, landmark materials and one-off decorations belong
// above the limit. `palette_split_is_sane` in the tests below guards the parts
// of this that can be checked mechanically.
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
pub const LEVER: Block = Block::new(256);

pub const CYAN_CONCRETE: Block = Block::new(16);
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

pub const LADDER: Block = Block::new(260);
pub const LIGHT_BLUE_CONCRETE: Block = Block::new(149);
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

pub const QUARTZ_BLOCK: Block = Block::new(57);
pub const POLISHED_BLACKSTONE: Block = Block::new(58);
pub const POLISHED_DEEPSLATE: Block = Block::new(59);
pub const POLISHED_DIORITE: Block = Block::new(60);
pub const POLISHED_GRANITE: Block = Block::new(61);

pub const QUARTZ_BRICKS: Block = Block::new(65);
pub const RAIL: Block = Block::new(258);
pub const RED_FLOWER: Block = Block::new(67);

pub const RED_TERRACOTTA: Block = Block::new(69);
pub const RED_WOOL: Block = Block::new(259);
pub const SAND: Block = Block::new(71);
pub const SANDSTONE: Block = Block::new(72);
pub const SCAFFOLDING: Block = Block::new(73);
pub const SMOOTH_QUARTZ: Block = Block::new(74);

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

pub const WATER: Block = Block::new(87);
pub const WHITE_CONCRETE: Block = Block::new(88);
pub const WHITE_FLOWER: Block = Block::new(89);
pub const WHITE_STAINED_GLASS: Block = Block::new(90);
pub const WHITE_TERRACOTTA: Block = Block::new(91);
pub const WHITE_WOOL: Block = Block::new(92);
pub const YELLOW_CONCRETE: Block = Block::new(39);
pub const YELLOW_FLOWER: Block = Block::new(94);
pub const YELLOW_WOOL: Block = Block::new(261);
pub const LIME_CONCRETE: Block = Block::new(190);

pub const BLUE_CONCRETE: Block = Block::new(191);
pub const PURPLE_CONCRETE: Block = Block::new(146);
pub const RED_CONCRETE: Block = Block::new(100);
pub const MAGENTA_CONCRETE: Block = Block::new(173);

pub const YELLOW_TERRACOTTA: Block = Block::new(104);
pub const WAXED_OXIDIZED_COPPER: Block = Block::new(103);
pub const WAXED_COPPER_BLOCK: Block = Block::new(195);
pub const WAXED_EXPOSED_COPPER: Block = Block::new(199);
pub const WAXED_EXPOSED_CHISELED_COPPER: Block = Block::new(204);
pub const WAXED_EXPOSED_CUT_COPPER: Block = Block::new(215);
pub const RED_NETHER_BRICKS: Block = Block::new(68);
pub const CHERRY_LOG: Block = Block::new(230);
pub const CHERRY_LEAVES: Block = Block::new(231);
pub const COBBLESTONE_STAIRS: Block = Block::new(17);
pub const MOSSY_STONE_BRICK_STAIRS: Block = Block::new(56);
pub const MOSSY_COBBLESTONE_STAIRS: Block = Block::new(62);
pub const DEEPSLATE_BRICK_STAIRS: Block = Block::new(63);
pub const POLISHED_DEEPSLATE_STAIRS: Block = Block::new(64);
pub const SPRUCE_STAIRS: Block = Block::new(75);
pub const DARK_OAK_STAIRS: Block = Block::new(86);
pub const RED_NETHER_BRICK_STAIRS: Block = Block::new(97);
pub const ANDESITE_STAIRS: Block = Block::new(112);
pub const WAXED_EXPOSED_CUT_COPPER_STAIRS: Block = Block::new(133);
pub const WAXED_CUT_COPPER_STAIRS: Block = Block::new(38);
pub const WAXED_OXIDIZED_CUT_COPPER_STAIRS: Block = Block::new(102);
pub const SNOW_BLOCK: Block = Block::new(111);

pub const SIGN: Block = Block::new(274);
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
pub const WHITE_WALL_BANNER: Block = Block::new(363);
pub const BLUE_WALL_BANNER: Block = Block::new(275);
pub const BLACK_WALL_BANNER: Block = Block::new(277);
pub const RED_WALL_BANNER: Block = Block::new(278);
pub const GREEN_WALL_BANNER: Block = Block::new(285);
pub const MOSSY_STONE_BRICKS: Block = Block::new(150);
pub const DEEPSLATE: Block = Block::new(151);
pub const TUFF: Block = Block::new(152);
pub const COBBLED_DEEPSLATE: Block = Block::new(153);
pub const WATER_CAULDRON: Block = Block::new(286);
pub const CHEST: Block = Block::new(155);
pub const RED_CARPET: Block = Block::new(289);
pub const ANVIL: Block = Block::new(157);
pub const NOTE_BLOCK: Block = Block::new(158);
pub const OAK_DOOR: Block = Block::new(293);
pub const BREWING_STAND: Block = Block::new(160);
pub const RED_BED_NORTH_HEAD: Block = Block::new(161);
pub const RED_BED_NORTH_FOOT: Block = Block::new(162);
pub const RED_BED_EAST_HEAD: Block = Block::new(294);
pub const RED_BED_EAST_FOOT: Block = Block::new(295);
pub const RED_BED_SOUTH_HEAD: Block = Block::new(165);
pub const RED_BED_SOUTH_FOOT: Block = Block::new(166);
pub const RED_BED_WEST_HEAD: Block = Block::new(167);
pub const RED_BED_WEST_FOOT: Block = Block::new(168);
pub const GRAY_STAINED_GLASS: Block = Block::new(169);
pub const LIGHT_GRAY_STAINED_GLASS: Block = Block::new(170);
pub const BROWN_STAINED_GLASS: Block = Block::new(171);
pub const TINTED_GLASS: Block = Block::new(172);
pub const OAK_TRAPDOOR: Block = Block::new(299);
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
pub const BARREL: Block = Block::new(188);
pub const FERN: Block = Block::new(189);
pub const COBWEB: Block = Block::new(301);
pub const CHISELLED_BOOKSHELF_NORTH: Block = Block::new(305);
pub const CHISELLED_BOOKSHELF_EAST: Block = Block::new(316);
pub const CHISELLED_BOOKSHELF_SOUTH: Block = Block::new(320);
pub const CHISELLED_BOOKSHELF_WEST: Block = Block::new(326);
// Backwards-compatible alias (defaults to north-facing)
pub const CHISELLED_BOOKSHELF: Block = CHISELLED_BOOKSHELF_NORTH;

pub const DAMAGED_ANVIL: Block = Block::new(332);
pub const LARGE_FERN_LOWER: Block = Block::new(197);
pub const LARGE_FERN_UPPER: Block = Block::new(198);

pub const END_ROD: Block = Block::new(345);
pub const LIGHTNING_ROD: Block = Block::new(201);
pub const GOLD_BLOCK: Block = Block::new(359);
pub const SEA_LANTERN: Block = Block::new(203);

pub const ORANGE_WOOL: Block = Block::new(360);
pub const BLUE_WOOL: Block = Block::new(361);
pub const GREEN_CONCRETE: Block = Block::new(207);
pub const BRICK_WALL: Block = Block::new(208);
pub const REDSTONE_BLOCK: Block = Block::new(209);
pub const CHAIN_X: Block = Block::new(210);
pub const CHAIN_Z: Block = Block::new(362);
pub const SPRUCE_DOOR_LOWER: Block = Block::new(364);
pub const SPRUCE_DOOR_UPPER: Block = Block::new(365);
pub const SMOOTH_STONE_SLAB: Block = Block::new(214);

pub const LIGHT_GRAY_TERRACOTTA: Block = Block::new(216);
pub const OAK_SLAB_TOP: Block = Block::new(217);
pub const OAK_DOOR_UPPER: Block = Block::new(366);
pub const DARK_OAK_LOG: Block = Block::new(219);
pub const DARK_OAK_LEAVES: Block = Block::new(220);
pub const JUNGLE_LOG: Block = Block::new(221);
pub const JUNGLE_LEAVES: Block = Block::new(222);
pub const ACACIA_LOG: Block = Block::new(223);
pub const ACACIA_LEAVES: Block = Block::new(224);
pub const SPRUCE_LEAVES: Block = Block::new(225);
pub const CYAN_STAINED_GLASS: Block = Block::new(226);
pub const BLUE_STAINED_GLASS: Block = Block::new(227);
pub const LIGHT_BLUE_STAINED_GLASS: Block = Block::new(228);
pub const DAYLIGHT_DETECTOR: Block = Block::new(229);

pub const FLOWER_POT: Block = Block::new(235);
pub const OAK_TRAPDOOR_OPEN_NORTH: Block = Block::new(236);

pub const QUARTZ_SLAB_TOP: Block = Block::new(240);
pub const DARK_OAK_TRAPDOOR: Block = Block::new(241);
pub const SPRUCE_TRAPDOOR: Block = Block::new(242);
pub const BIRCH_TRAPDOOR: Block = Block::new(243);
pub const MUD_BRICK_SLAB: Block = Block::new(244);
pub const BRICK_SLAB: Block = Block::new(245);
pub const POTTED_RED_TULIP: Block = Block::new(246);
pub const POTTED_DANDELION: Block = Block::new(247);
pub const POTTED_BLUE_ORCHID: Block = Block::new(248);

pub const GRAY_CONCRETE_POWDER: Block = Block::new(252);
pub const BROWN_CONCRETE_POWDER: Block = Block::new(232);
pub const CYAN_TERRACOTTA: Block = Block::new(253);
pub const BLACK_WOOL: Block = Block::new(254);
pub const LIGHT_GRAY_WALL_BANNER: Block = Block::new(255);

pub const MANGROVE_LOG: Block = Block::new(0);
pub const MANGROVE_LEAVES: Block = Block::new(233);
pub const AZALEA_LEAVES: Block = Block::new(234);

pub const DIAMOND_ORE: Block = Block::new(249);
pub const REDSTONE_ORE: Block = Block::new(250);
pub const LAPIS_ORE: Block = Block::new(251);

// Underwater bed palette + vegetation (ported from the Teddy fork; ids kept verbatim).
pub const SEAGRASS: Block = Block::new(238);
pub const KELP_PLANT: Block = Block::new(239);
pub const MAGMA_BLOCK: Block = Block::new(18);
pub const SNOW_LAYER: Block = Block::new(40);
pub const KELP: Block = Block::new(66);
pub const TALL_SEAGRASS_BOTTOM: Block = Block::new(70);
pub const TALL_SEAGRASS_TOP: Block = Block::new(93);
pub const SEA_PICKLE: Block = Block::new(95);
pub const SOUL_SAND: Block = Block::new(96);
// Structure-schematic blocks, placed with their original block-states.
pub const SANDSTONE_WALL: Block = Block::new(98);
pub const CUT_SANDSTONE_SLAB: Block = Block::new(99);
pub const SMOOTH_QUARTZ_SLAB: Block = Block::new(268);
pub const SMOOTH_QUARTZ_STAIRS: Block = Block::new(269);
pub const BLACKSTONE_STAIRS: Block = Block::new(270);
pub const BLACKSTONE_WALL: Block = Block::new(271);
pub const DIORITE_WALL: Block = Block::new(272);
pub const IRON_TRAPDOOR: Block = Block::new(101);
pub const JUNGLE_TRAPDOOR: Block = Block::new(113);
pub const BIRCH_FENCE: Block = Block::new(267);
pub const JUNGLE_FENCE: Block = Block::new(276);
pub const BIRCH_FENCE_GATE: Block = Block::new(147);
pub const DARK_OAK_FENCE_GATE: Block = Block::new(148);
pub const BIRCH_DOOR: Block = Block::new(279);
pub const BIRCH_PRESSURE_PLATE: Block = Block::new(280);
pub const STONE_PRESSURE_PLATE: Block = Block::new(281);
pub const BLAST_FURNACE: Block = Block::new(282);
pub const DISPENSER: Block = Block::new(283);
pub const HOPPER: Block = Block::new(284);
pub const GRINDSTONE: Block = Block::new(257);
pub const LANTERN: Block = Block::new(154);
pub const LODESTONE: Block = Block::new(287);
pub const REDSTONE_TORCH: Block = Block::new(288);
pub const STONE_BUTTON: Block = Block::new(156);
pub const CHISELED_POLISHED_BLACKSTONE: Block = Block::new(290);
pub const MOSSY_STONE_BRICK_WALL: Block = Block::new(291);
pub const BAMBOO_STAIRS: Block = Block::new(292);
pub const POLISHED_DEEPSLATE_WALL: Block = Block::new(159);
pub const BLACK_STAINED_GLASS: Block = Block::new(163);
pub const POLISHED_ANDESITE_SLAB: Block = Block::new(164);
pub const END_STONE_BRICK_WALL: Block = Block::new(296);
pub const BAMBOO_SLAB: Block = Block::new(297);
pub const CHISELED_DEEPSLATE: Block = Block::new(298);
pub const POLISHED_DEEPSLATE_SLAB: Block = Block::new(273);
pub const BIRCH_BUTTON: Block = Block::new(300);
pub const COBBLESTONE_SLAB: Block = Block::new(265);
pub const DARK_OAK_SLAB: Block = Block::new(302);
pub const JUNGLE_SLAB: Block = Block::new(303);
pub const JUNGLE_STAIRS: Block = Block::new(304);
pub const NETHER_BRICK_FENCE: Block = Block::new(266);
pub const OAK_BUTTON: Block = Block::new(306);
pub const POWERED_RAIL: Block = Block::new(307);
pub const SPRUCE_FENCE: Block = Block::new(308);
pub const SPRUCE_SLAB: Block = Block::new(309);
pub const ANDESITE_SLAB: Block = Block::new(310);
pub const COBBLED_DEEPSLATE_SLAB: Block = Block::new(311);
pub const COBBLED_DEEPSLATE_STAIRS: Block = Block::new(312);
pub const DARK_OAK_FENCE: Block = Block::new(313);
pub const DARK_OAK_PRESSURE_PLATE: Block = Block::new(314);
pub const GRAY_STAINED_GLASS_PANE: Block = Block::new(192);
pub const GRAY_WALL_BANNER: Block = Block::new(317);
pub const GRAY_WOOL: Block = Block::new(318);
pub const NETHER_WART_BLOCK: Block = Block::new(319);
pub const OAK_FENCE_GATE: Block = Block::new(193);
pub const POLISHED_BASALT: Block = Block::new(321);
pub const POLISHED_BLACKSTONE_BUTTON: Block = Block::new(322);
pub const POLISHED_BLACKSTONE_PRESSURE_PLATE: Block = Block::new(323);
pub const RED_NETHER_BRICK_SLAB: Block = Block::new(324);
pub const SPRUCE_BUTTON: Block = Block::new(325);
pub const SPRUCE_FENCE_GATE: Block = Block::new(194);
pub const ACACIA_TRAPDOOR: Block = Block::new(327);
pub const COMPOSTER: Block = Block::new(328);
pub const CYAN_CARPET: Block = Block::new(329);
pub const DARK_OAK_BUTTON: Block = Block::new(330);
pub const END_STONE_BRICK_SLAB: Block = Block::new(331);
pub const GLASS_PANE: Block = Block::new(196);
pub const GREEN_CARPET: Block = Block::new(333);
pub const LIGHT_BLUE_CARPET: Block = Block::new(334);
pub const NETHER_BRICK_WALL: Block = Block::new(335);
pub const SMOKER: Block = Block::new(336);
pub const SMOOTH_RED_SANDSTONE: Block = Block::new(337);
pub const SMOOTH_RED_SANDSTONE_SLAB: Block = Block::new(338);
pub const BLUE_STAINED_GLASS_PANE: Block = Block::new(339);
pub const CYAN_WOOL: Block = Block::new(340);
pub const LIGHT_GRAY_CARPET: Block = Block::new(341);
pub const MOSSY_COBBLESTONE_SLAB: Block = Block::new(342);
pub const MOSSY_STONE_BRICK_SLAB: Block = Block::new(343);
pub const PRISMARINE: Block = Block::new(344);
pub const STONE_STAIRS: Block = Block::new(200);
pub const TRIPWIRE_HOOK: Block = Block::new(346);
// Tombstone and wind-turbine schematic blocks.
pub const SPRUCE_WALL_SIGN: Block = Block::new(347);
pub const GRANITE_STAIRS: Block = Block::new(348);
pub const DIORITE_STAIRS: Block = Block::new(349);
pub const DEEPSLATE_TILES: Block = Block::new(350);
pub const DEEPSLATE_TILE_SLAB: Block = Block::new(351);
pub const DEEPSLATE_TILE_WALL: Block = Block::new(352);
pub const POLISHED_BLACKSTONE_SLAB: Block = Block::new(353);
pub const POLISHED_DIORITE_SLAB: Block = Block::new(354);
pub const SOUL_LANTERN: Block = Block::new(355);
pub const CHISELED_QUARTZ_BLOCK: Block = Block::new(356);
pub const QUARTZ_PILLAR: Block = Block::new(357);
pub const REDSTONE_WALL_TORCH: Block = Block::new(358);
pub const EMPTY_FLOWER_POT: Block = Block::new(202);
pub const WARPED_SLAB: Block = Block::new(205);
pub const WARPED_STAIRS: Block = Block::new(206);
pub const WARPED_TRAPDOOR: Block = Block::new(211);
pub const STRIPPED_WARPED_STEM: Block = Block::new(212);
pub const STRIPPED_WARPED_HYPHAE: Block = Block::new(213);
pub const ORANGE_CONCRETE: Block = Block::new(145);
pub const REDSTONE_LAMP: Block = Block::new(218);
// Reuses the retired open-trapdoor slot; sub-256 ids keep sections one byte per cell.
pub const SUGAR_CANE: Block = Block::new(237);

/// Maps a block to a stair variant in the same colour family.
#[inline]
pub fn get_stair_block_for_material(material: Block) -> Block {
    match material {
        // Stone family
        STONE_BRICKS => STONE_BRICK_STAIRS,
        STONE => COBBLESTONE_STAIRS,
        COBBLESTONE => COBBLESTONE_STAIRS,
        MOSSY_COBBLESTONE => MOSSY_COBBLESTONE_STAIRS,
        MOSSY_STONE_BRICKS => MOSSY_STONE_BRICK_STAIRS,
        CRACKED_STONE_BRICKS => STONE_BRICK_STAIRS,
        CHISELED_STONE_BRICKS => STONE_BRICK_STAIRS,
        TUFF => COBBLESTONE_STAIRS,
        ANDESITE => ANDESITE_STAIRS,
        POLISHED_ANDESITE => POLISHED_ANDESITE_STAIRS,
        SMOOTH_STONE => POLISHED_ANDESITE_STAIRS,
        DIORITE => POLISHED_DIORITE_STAIRS,
        POLISHED_DIORITE => POLISHED_DIORITE_STAIRS,

        // Dark stone family
        DEEPSLATE => DEEPSLATE_BRICK_STAIRS,
        DEEPSLATE_BRICKS => DEEPSLATE_BRICK_STAIRS,
        POLISHED_DEEPSLATE => POLISHED_DEEPSLATE_STAIRS,
        COBBLED_DEEPSLATE => DEEPSLATE_BRICK_STAIRS,
        BLACKSTONE => POLISHED_BLACKSTONE_BRICK_STAIRS,
        POLISHED_BLACKSTONE => POLISHED_BLACKSTONE_BRICK_STAIRS,
        POLISHED_BLACKSTONE_BRICKS => POLISHED_BLACKSTONE_BRICK_STAIRS,
        BLACK_TERRACOTTA => POLISHED_BLACKSTONE_BRICK_STAIRS,

        // Warm reds and browns
        BRICK => BRICK_STAIRS,
        TERRACOTTA => BRICK_STAIRS,
        ORANGE_TERRACOTTA => BRICK_STAIRS,
        RED_TERRACOTTA => BRICK_STAIRS,
        GRANITE => POLISHED_GRANITE_STAIRS,
        POLISHED_GRANITE => POLISHED_GRANITE_STAIRS,

        // Mud and earth tones
        MUD_BRICKS => MUD_BRICK_STAIRS,
        MUD => MUD_BRICK_STAIRS,
        BROWN_CONCRETE => MUD_BRICK_STAIRS,
        BROWN_CONCRETE_POWDER => MUD_BRICK_STAIRS,
        BROWN_TERRACOTTA => MUD_BRICK_STAIRS,
        GRAY_TERRACOTTA => MUD_BRICK_STAIRS,
        LIGHT_GRAY_TERRACOTTA => MUD_BRICK_STAIRS,

        // White and pale tones
        WHITE_TERRACOTTA => QUARTZ_STAIRS,
        LIGHT_BLUE_TERRACOTTA => POLISHED_DIORITE_STAIRS,

        // Cool blues / cyan get dark stone stairs
        BLUE_TERRACOTTA => DEEPSLATE_BRICK_STAIRS,
        CYAN_TERRACOTTA => DEEPSLATE_BRICK_STAIRS,

        // Yellow and sand tones
        END_STONE_BRICKS => END_STONE_BRICK_STAIRS,
        YELLOW_TERRACOTTA => SMOOTH_SANDSTONE_STAIRS,
        SANDSTONE => SMOOTH_SANDSTONE_STAIRS,
        SMOOTH_SANDSTONE => SMOOTH_SANDSTONE_STAIRS,

        // Whites and quartz
        QUARTZ_BLOCK => POLISHED_DIORITE_STAIRS,
        QUARTZ_BRICKS => POLISHED_DIORITE_STAIRS,
        SMOOTH_QUARTZ => POLISHED_DIORITE_STAIRS,
        WHITE_CONCRETE => QUARTZ_STAIRS,
        GLASS => QUARTZ_STAIRS,

        // Greys and concretes
        GRAY_CONCRETE => POLISHED_BLACKSTONE_BRICK_STAIRS,
        LIGHT_GRAY_CONCRETE => STONE_BRICK_STAIRS,
        BLACK_CONCRETE => POLISHED_BLACKSTONE_BRICK_STAIRS,
        LIGHT_BLUE_CONCRETE => POLISHED_DIORITE_STAIRS,
        CYAN_CONCRETE => DEEPSLATE_BRICK_STAIRS,
        GREEN_CONCRETE => MOSSY_COBBLESTONE_STAIRS,

        // Nether brick family
        NETHER_BRICK => NETHER_BRICK_STAIRS,
        RED_NETHER_BRICKS => RED_NETHER_BRICK_STAIRS,

        // Copper family
        WAXED_OXIDIZED_COPPER => WAXED_OXIDIZED_CUT_COPPER_STAIRS,
        WAXED_COPPER_BLOCK => WAXED_CUT_COPPER_STAIRS,
        WAXED_EXPOSED_COPPER => WAXED_EXPOSED_CUT_COPPER_STAIRS,
        WAXED_EXPOSED_CHISELED_COPPER => WAXED_EXPOSED_CUT_COPPER_STAIRS,
        WAXED_EXPOSED_CUT_COPPER => WAXED_EXPOSED_CUT_COPPER_STAIRS,

        // Wood family
        OAK_PLANKS => OAK_STAIRS,
        SPRUCE_PLANKS => SPRUCE_STAIRS,
        DARK_OAK_PLANKS => DARK_OAK_STAIRS,
        OAK_LOG => OAK_STAIRS,
        SPRUCE_LOG => SPRUCE_STAIRS,

        // Misc
        IRON_BLOCK => POLISHED_DIORITE_STAIRS,
        NETHERITE_BLOCK => POLISHED_BLACKSTONE_BRICK_STAIRS,
        HAY_BALE => OAK_STAIRS,
        GRAVEL => COBBLESTONE_STAIRS,
        GRASS_BLOCK => MOSSY_COBBLESTONE_STAIRS,
        MOSS_BLOCK => MOSSY_COBBLESTONE_STAIRS,

        _ => STONE_BRICK_STAIRS,
    }
}

/// Returns a matching slab block for the given wall material.
/// Used for floor-level ledges and cornices in building depth features.
pub fn get_slab_block_for_material(material: Block) -> Block {
    match material {
        STONE_BRICKS | CHISELED_STONE_BRICKS | CRACKED_STONE_BRICKS => STONE_BRICK_SLAB,
        BRICK | BROWN_TERRACOTTA | BROWN_CONCRETE_POWDER => BRICK_SLAB,
        MUD_BRICKS | WHITE_TERRACOTTA | GRAY_TERRACOTTA | LIGHT_BLUE_TERRACOTTA => MUD_BRICK_SLAB,
        OAK_PLANKS | OAK_LOG | SPRUCE_PLANKS | DARK_OAK_PLANKS => OAK_SLAB,
        QUARTZ_BLOCK | QUARTZ_BRICKS | WHITE_CONCRETE => QUARTZ_SLAB_TOP,
        SMOOTH_STONE | POLISHED_ANDESITE | ANDESITE | GRAY_CONCRETE | LIGHT_GRAY_CONCRETE => {
            SMOOTH_STONE_SLAB
        }
        SANDSTONE | SMOOTH_SANDSTONE => STONE_BLOCK_SLAB,
        _ => STONE_BRICK_SLAB,
    }
}

/// Returns a matching wall piece block (thin wall) for the given wall material.
/// Used for parapets and decorative wall elements in building depth features.
pub fn get_wall_piece_for_material(material: Block) -> Block {
    match material {
        STONE_BRICKS
        | CHISELED_STONE_BRICKS
        | CRACKED_STONE_BRICKS
        | POLISHED_ANDESITE
        | SMOOTH_STONE
        | POLISHED_DEEPSLATE
        | DEEPSLATE_BRICKS => STONE_BRICK_WALL,
        BRICK | BROWN_TERRACOTTA | BROWN_CONCRETE_POWDER | MUD_BRICKS | WHITE_TERRACOTTA => {
            BRICK_WALL
        }
        ANDESITE | GRAY_CONCRETE | LIGHT_GRAY_CONCRETE => ANDESITE_WALL,
        _ => COBBLESTONE_WALL,
    }
}

// Window variations for different building types
pub static WINDOW_VARIATIONS: [Block; 11] = [
    GLASS,
    GRAY_STAINED_GLASS,
    LIGHT_GRAY_STAINED_GLASS,
    GRAY_STAINED_GLASS,
    BROWN_STAINED_GLASS,
    WHITE_STAINED_GLASS,
    TINTED_GLASS,
    LIGHT_BLUE_STAINED_GLASS,
    CYAN_STAINED_GLASS,
    BLACK_STAINED_GLASS,
    BROWN_STAINED_GLASS,
];

// Residential window options
pub static RESIDENTIAL_WINDOW_OPTIONS: [Block; 6] = [
    GLASS,
    WHITE_STAINED_GLASS,
    LIGHT_GRAY_STAINED_GLASS,
    BROWN_STAINED_GLASS,
    TINTED_GLASS,
    LIGHT_BLUE_STAINED_GLASS,
];

// Institutional window options (hospital, school, etc.)
pub static INSTITUTIONAL_WINDOW_OPTIONS: [Block; 4] = [
    GLASS,
    WHITE_STAINED_GLASS,
    LIGHT_GRAY_STAINED_GLASS,
    LIGHT_BLUE_STAINED_GLASS,
];

// Hospitality window options (hotel, restaurant).
pub static HOSPITALITY_WINDOW_OPTIONS: [Block; 5] = [
    GLASS,
    WHITE_STAINED_GLASS,
    GRAY_STAINED_GLASS,
    LIGHT_BLUE_STAINED_GLASS,
    BROWN_STAINED_GLASS,
];

// Industrial window options
pub static INDUSTRIAL_WINDOW_OPTIONS: [Block; 4] = [
    GLASS,
    GRAY_STAINED_GLASS,
    LIGHT_GRAY_STAINED_GLASS,
    BROWN_STAINED_GLASS,
];

// Religious window options (stained glass).
pub static RELIGIOUS_WINDOW_OPTIONS: [Block; 5] = [
    BLUE_STAINED_GLASS,
    CYAN_STAINED_GLASS,
    LIGHT_BLUE_STAINED_GLASS,
    BROWN_STAINED_GLASS,
    WHITE_STAINED_GLASS,
];

// Farm window options, plain glazing since barns don't get tinted variety.
pub static FARM_WINDOW_OPTIONS: [Block; 3] = [GLASS, WHITE_STAINED_GLASS, LIGHT_GRAY_STAINED_GLASS];

// Historic window options (clear, slightly aged glazing).
pub static HISTORIC_WINDOW_OPTIONS: [Block; 3] =
    [GLASS, LIGHT_GRAY_STAINED_GLASS, WHITE_STAINED_GLASS];

// Floor block options for buildings
pub static FLOOR_BLOCK_OPTIONS: [Block; 8] = [
    WHITE_CONCRETE,
    GRAY_CONCRETE,
    LIGHT_GRAY_CONCRETE,
    POLISHED_ANDESITE,
    SMOOTH_STONE,
    STONE_BRICKS,
    MUD_BRICKS,
    OAK_PLANKS,
];

// Random floor block selection (non-deterministic, for backwards compatibility)
pub fn get_random_floor_block() -> Block {
    use rand::Rng;
    let mut rng = rand::rng();
    FLOOR_BLOCK_OPTIONS[rng.random_range(0..FLOOR_BLOCK_OPTIONS.len())]
}

/// Deterministic floor block selection using provided RNG
pub fn get_floor_block_with_rng(rng: &mut impl rand::Rng) -> Block {
    FLOOR_BLOCK_OPTIONS[rng.random_range(0..FLOOR_BLOCK_OPTIONS.len())]
}

// Function to get a random fallback building block when no color attribute is specified
pub fn get_fallback_building_block(rng: &mut impl rand::Rng) -> Block {
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
    ];
    fallback_options[rng.random_range(0..fallback_options.len())]
}

// Function to get a random castle wall block
pub fn get_castle_wall_block(rng: &mut impl rand::Rng) -> Block {
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
    castle_wall_options[rng.random_range(0..castle_wall_options.len())]
}

/// Maps an OSM building:material to a wall block, or None if unrecognized.
pub fn get_wall_block_for_material(material: &str, rng: &mut impl rand::Rng) -> Option<Block> {
    let normalized: String = material
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect();

    let options: &[Block] = match normalized.as_str() {
        "brick" | "bricks" | "redbrick" => &[BRICK, NETHER_BRICK],
        // `hard` and `block` are among the most common building:material values.
        "stone" | "naturalstone" | "hard" => &[STONE_BRICKS, COBBLESTONE, SMOOTH_STONE, ANDESITE],
        "limestone" => &[SMOOTH_STONE, POLISHED_ANDESITE, WHITE_TERRACOTTA],
        "sandstone" => &[SANDSTONE, SMOOTH_SANDSTONE],
        "marble" => &[QUARTZ_BLOCK, POLISHED_DIORITE, WHITE_CONCRETE],
        "granite" => &[POLISHED_GRANITE, POLISHED_DIORITE, QUARTZ_BLOCK],
        "slate" => &[POLISHED_BLACKSTONE, DEEPSLATE_BRICKS, BLACKSTONE],
        "concrete"
        | "reinforcedconcrete"
        | "cementblock"
        | "cement"
        | "breezeblock"
        | "concreteblock"
        | "concreteblocks"
        | "block"
        | "concretemasonryunit" => &[
            GRAY_CONCRETE,
            LIGHT_GRAY_CONCRETE,
            WHITE_CONCRETE,
            SMOOTH_STONE,
        ],
        "plaster" | "stucco" | "render" | "rendering" | "limerender" | "plastered" => &[
            WHITE_CONCRETE,
            LIGHT_GRAY_CONCRETE,
            QUARTZ_BLOCK,
            SMOOTH_SANDSTONE,
        ],
        "wood" | "timber" | "timberframe" | "halftimber" | "halftimbered" | "loghouse" | "logs"
        | "bamboo" => &[OAK_PLANKS, SPRUCE_PLANKS, DARK_OAK_PLANKS, OAK_LOG],
        "reed" => &[HAY_BALE],
        "metal" | "steel" | "iron" | "aluminium" | "aluminum" | "corrugatedsteel"
        | "corrugatediron" | "corrugatedmetal" | "tin" | "sheetmetal" | "metalsheet"
        | "metalplates" => &[IRON_BLOCK, LIGHT_GRAY_CONCRETE, GRAY_CONCRETE],
        "copper" | "oxidisedcopper" | "oxidizedcopper" | "patina" | "verdigris" => &[
            WAXED_OXIDIZED_COPPER,
            WAXED_EXPOSED_COPPER,
            WAXED_COPPER_BLOCK,
        ],
        "glass" => &[
            GLASS,
            LIGHT_GRAY_STAINED_GLASS,
            WHITE_STAINED_GLASS,
            TINTED_GLASS,
        ],
        "mirror" | "solarpanels" => &[GLASS, BLUE_STAINED_GLASS, LIGHT_BLUE_STAINED_GLASS],
        "tiles" | "tile" | "rooftiles" | "ceramictiles" | "ceramic" | "terracotta" => &[
            WHITE_TERRACOTTA,
            BROWN_TERRACOTTA,
            RED_TERRACOTTA,
            ORANGE_TERRACOTTA,
        ],
        "mud" | "adobe" | "earth" | "clay" | "rammedearth" | "cob" | "loam" => {
            &[MUD_BRICKS, BROWN_TERRACOTTA, BROWN_CONCRETE]
        }
        "thatch" | "straw" => &[HAY_BALE],
        "asbestos" | "asbestoscement" | "fibrecement" | "fibercement" => {
            &[LIGHT_GRAY_CONCRETE, GRAY_CONCRETE]
        }
        "vinyl" | "siding" | "vinylsiding" | "weatherboard" | "weatherboarding" | "clapboard" => {
            &[OAK_PLANKS, SPRUCE_PLANKS, WHITE_CONCRETE]
        }
        "panel" | "panels" | "panelling" | "paneling" | "panelhouse" | "prefab"
        | "prefabricated" => &[LIGHT_GRAY_CONCRETE, GRAY_CONCRETE, WHITE_CONCRETE],
        "plastic" | "light" => &[WHITE_CONCRETE, LIGHT_GRAY_CONCRETE, QUARTZ_BLOCK, GLASS],
        "mixed" | "masonry" => &[STONE_BRICKS, BRICK, SMOOTH_STONE, COBBLESTONE],
        "pebbledash" => &[ANDESITE, COBBLESTONE, STONE_BRICKS, GRAVEL],
        _ => return None,
    };

    Some(options[rng.random_range(0..options.len())])
}

/// Maps an OSM roof:material to a roof block, or None if unrecognized.
pub fn get_roof_block_for_material(material: &str, rng: &mut impl rand::Rng) -> Option<Block> {
    let normalized: String = material
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect();

    let options: &[Block] = match normalized.as_str() {
        "glass" | "glazing" | "acrylicglass" => {
            &[GLASS, WHITE_STAINED_GLASS, LIGHT_GRAY_STAINED_GLASS]
        }
        "tile" | "tiles" | "rooftiles" | "ceramic" | "ceramictiles" | "claytile" | "claytiles"
        | "terracotta" => &[BRICK, NETHER_BRICK, RED_NETHER_BRICKS, MUD_BRICKS],
        "slate" | "slates" => &[POLISHED_BLACKSTONE, DEEPSLATE_BRICKS, BLACKSTONE],
        "metal" | "steel" | "aluminium" | "aluminum" | "corrugatedsteel" | "corrugatediron"
        | "corrugatedmetal" | "tin" | "zinc" | "lead" | "sheetmetal" | "metalsheet" => {
            &[LIGHT_GRAY_CONCRETE, GRAY_CONCRETE, IRON_BLOCK]
        }
        "copper" => &[
            WAXED_OXIDIZED_COPPER,
            WAXED_EXPOSED_COPPER,
            WAXED_COPPER_BLOCK,
        ],
        "concrete" | "reinforcedconcrete" => &[LIGHT_GRAY_CONCRETE, GRAY_CONCRETE, SMOOTH_STONE],
        "wood" | "timber" | "shingle" | "shingles" | "woodshingle" | "woodshingles" => {
            &[OAK_PLANKS, SPRUCE_PLANKS, DARK_OAK_PLANKS]
        }
        "thatch" | "straw" | "reed" | "reeds" | "palmleaves" => &[HAY_BALE],
        "asphalt" | "bitumen" | "tar" | "tarpaper" | "rolledasphalt" | "rolledroofing"
        | "asphaltshingle" => &[BLACKSTONE, POLISHED_BLACKSTONE, POLISHED_BLACKSTONE_BRICKS],
        "stone" => &[STONE_BRICKS, SMOOTH_STONE, ANDESITE],
        "gravel" => &[GRAVEL],
        "grass" | "green" | "vegetation" | "greenroof" | "sod" => &[GRASS_BLOCK, MOSS_BLOCK],
        "eternit" | "asbestos" | "fibrecement" | "fibercement" => {
            &[LIGHT_GRAY_CONCRETE, GRAY_CONCRETE]
        }
        "plastic" => &[LIGHT_GRAY_CONCRETE, GRAY_CONCRETE, WHITE_CONCRETE, GLASS],
        _ => return None,
    };

    Some(options[rng.random_range(0..options.len())])
}

#[cfg(test)]
mod material_tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    fn rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(1)
    }

    #[test]
    fn newly_added_wall_materials_resolve() {
        // High-use building:material values that previously returned None.
        for m in [
            "hard",
            "block",
            "plastered",
            "metal_plates",
            "concrete masonry unit",
            "slate",
            "sandstone",
            "limestone",
            "marble",
            "mixed",
            "masonry",
            "pebbledash",
            "mirror",
        ] {
            assert!(
                get_wall_block_for_material(m, &mut rng()).is_some(),
                "wall material {m} should resolve"
            );
        }
    }

    #[test]
    fn newly_added_roof_materials_resolve() {
        for m in [
            "copper",
            "palm_leaves",
            "asphalt_shingle",
            "plastic",
            "acrylic_glass",
        ] {
            assert!(
                get_roof_block_for_material(m, &mut rng()).is_some(),
                "roof material {m} should resolve"
            );
        }
    }

    #[test]
    fn underscore_and_space_normalization_holds() {
        // The normalizer strips spaces/underscores/hyphens and lowercases.
        assert_eq!(
            get_wall_block_for_material("Metal_Plates", &mut rng()),
            get_wall_block_for_material("metalplates", &mut rng()),
        );
        assert_eq!(
            get_roof_block_for_material("asphalt_shingle", &mut rng()),
            get_roof_block_for_material("asphaltshingle", &mut rng()),
        );
    }

    #[test]
    fn unknown_materials_still_return_none() {
        assert!(get_wall_block_for_material("notamaterial", &mut rng()).is_none());
        assert!(get_roof_block_for_material("notamaterial", &mut rng()).is_none());
    }

    /// Blocks the generator can lay down across whole buildings and streets must
    /// stay under [`BYTE_ID_LIMIT`]; one stray high id widens every section that
    /// holds it from 4 KB to 8 KB. This covers the tables that can be enumerated
    /// mechanically -- the material palettes, the window and floor pickers, and
    /// the material -> stair/slab/wall derivations over the whole id space.
    #[test]
    fn palette_split_is_sane() {
        fn check(block: Block, source: &str) {
            assert!(
                block.id() < BYTE_ID_LIMIT,
                "{source} can place {} (id {}), at or above BYTE_ID_LIMIT; \
                 that widens every section it lands in",
                block.name(),
                block.id(),
            );
        }

        for (table, source) in [
            (&WINDOW_VARIATIONS[..], "WINDOW_VARIATIONS"),
            (
                &RESIDENTIAL_WINDOW_OPTIONS[..],
                "RESIDENTIAL_WINDOW_OPTIONS",
            ),
            (
                &INSTITUTIONAL_WINDOW_OPTIONS[..],
                "INSTITUTIONAL_WINDOW_OPTIONS",
            ),
            (
                &HOSPITALITY_WINDOW_OPTIONS[..],
                "HOSPITALITY_WINDOW_OPTIONS",
            ),
            (&INDUSTRIAL_WINDOW_OPTIONS[..], "INDUSTRIAL_WINDOW_OPTIONS"),
            (&RELIGIOUS_WINDOW_OPTIONS[..], "RELIGIOUS_WINDOW_OPTIONS"),
            (&FLOOR_BLOCK_OPTIONS[..], "FLOOR_BLOCK_OPTIONS"),
        ] {
            for &block in table {
                check(block, source);
            }
        }

        for block in crate::block_palette::all_building_palette_blocks() {
            check(block, "block_palette");
        }

        // Every wall material derives its own stairs, slabs and wall pieces, so
        // walk the whole low range rather than a sample of it. Materials that
        // already sit in the wide range need no check: they widen the section
        // themselves, so where their trim lands makes no difference.
        for id in 0..BYTE_ID_LIMIT {
            let material = Block::from_raw_id(id);
            if material.try_name().is_none() {
                continue;
            }
            check(
                get_stair_block_for_material(material),
                "get_stair_block_for_material",
            );
            check(
                get_slab_block_for_material(material),
                "get_slab_block_for_material",
            );
            check(
                get_wall_piece_for_material(material),
                "get_wall_piece_for_material",
            );
        }

        // The random pickers draw from inline option arrays, so sample each one
        // often enough to reach every entry.
        for seed in 0..64 {
            let mut r = ChaCha8Rng::seed_from_u64(seed);
            check(
                get_fallback_building_block(&mut r),
                "get_fallback_building_block",
            );
            check(get_castle_wall_block(&mut r), "get_castle_wall_block");
            check(get_floor_block_with_rng(&mut r), "get_floor_block_with_rng");
            for material in WALL_MATERIALS {
                if let Some(block) = get_wall_block_for_material(material, &mut r) {
                    check(block, "get_wall_block_for_material");
                }
            }
            for material in ROOF_MATERIALS {
                if let Some(block) = get_roof_block_for_material(material, &mut r) {
                    check(block, "get_roof_block_for_material");
                }
            }
        }
    }

    /// Every `building:material` / `roof:material` value the mappers recognise.
    /// Keep in sync when adding an arm, so `palette_split_is_sane` keeps covering it.
    const WALL_MATERIALS: &[&str] = &[
        "brick",
        "bricks",
        "redbrick",
        "stone",
        "naturalstone",
        "hard",
        "limestone",
        "sandstone",
        "marble",
        "granite",
        "slate",
        "concrete",
        "reinforcedconcrete",
        "cementblock",
        "cement",
        "breezeblock",
        "concreteblock",
        "concreteblocks",
        "block",
        "concretemasonryunit",
        "plaster",
        "stucco",
        "render",
        "rendering",
        "limerender",
        "plastered",
        "wood",
        "timber",
        "timberframe",
        "halftimber",
        "halftimbered",
        "loghouse",
        "logs",
        "bamboo",
        "reed",
        "metal",
        "steel",
        "iron",
        "aluminium",
        "aluminum",
        "corrugatedsteel",
        "corrugatediron",
        "corrugatedmetal",
        "tin",
        "sheetmetal",
        "metalsheet",
        "metalplates",
        "copper",
        "oxidisedcopper",
        "oxidizedcopper",
        "patina",
        "verdigris",
        "glass",
        "mirror",
        "solarpanels",
        "tiles",
        "tile",
        "rooftiles",
        "ceramictiles",
        "ceramic",
        "terracotta",
        "mud",
        "adobe",
        "earth",
        "clay",
        "rammedearth",
        "cob",
        "loam",
        "thatch",
        "straw",
        "asbestos",
        "asbestoscement",
        "fibrecement",
        "fibercement",
        "vinyl",
        "siding",
        "vinylsiding",
        "weatherboard",
        "weatherboarding",
        "clapboard",
        "panel",
        "panels",
        "panelling",
        "paneling",
        "panelhouse",
        "prefab",
        "prefabricated",
        "plastic",
        "light",
        "mixed",
        "masonry",
        "pebbledash",
    ];

    const ROOF_MATERIALS: &[&str] = &[
        "glass",
        "glazing",
        "acrylicglass",
        "tile",
        "tiles",
        "rooftiles",
        "ceramic",
        "ceramictiles",
        "claytile",
        "claytiles",
        "terracotta",
        "slate",
        "slates",
        "metal",
        "steel",
        "aluminium",
        "aluminum",
        "corrugatedsteel",
        "corrugatediron",
        "corrugatedmetal",
        "tin",
        "zinc",
        "lead",
        "sheetmetal",
        "metalsheet",
        "copper",
        "concrete",
        "reinforcedconcrete",
        "wood",
        "timber",
        "shingle",
        "shingles",
        "woodshingle",
        "woodshingles",
        "thatch",
        "straw",
        "reed",
        "reeds",
        "palmleaves",
        "asphalt",
        "bitumen",
        "tar",
        "tarpaper",
        "rolledasphalt",
        "rolledroofing",
        "asphaltshingle",
        "stone",
        "gravel",
        "grass",
        "green",
        "vegetation",
        "greenroof",
        "sod",
        "eternit",
        "asbestos",
        "fibrecement",
        "fibercement",
        "plastic",
    ];
}
