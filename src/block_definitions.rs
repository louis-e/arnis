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
    name: &'static str,
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
    const fn new(namespaced_name: &'static str) -> Self {
        // Names are expected to include the namespace, e.g. "minecraft:oak_planks"
        Self {
            name: namespaced_name,
        }
    }

    #[inline(always)]
    pub fn name(&self) -> &str {
        self.name
    }

    pub fn properties(&self) -> Option<Value> {
        match self.name {
            "minecraft:birch_leaves" => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("persistent".to_string(), Value::String("true".to_string()));
                map
            })),
            "minecraft:oak_leaves" => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("persistent".to_string(), Value::String("true".to_string()));
                map
            })),
            "minecraft:carrots" => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("age".to_string(), Value::String("7".to_string()));
                map
            })),
            "minecraft:potatoes" => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("age".to_string(), Value::String("7".to_string()));
                map
            })),
            "minecraft:wheat" => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("age".to_string(), Value::String("7".to_string()));
                map
            })),
            "minecraft:oak_sign" => Some(Value::Compound({
                let mut map: HashMap<String, Value> = HashMap::new();
                map.insert("rotation".to_string(), Value::String("6".to_string()));
                map.insert(
                    "waterlogged".to_string(),
                    Value::String("false".to_string()),
                );
                map
            })),
            "minecraft:oak_trapdoor" => Some(Value::Compound({
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
static STAIR_CACHE: Lazy<Mutex<HashMap<(Block, StairFacing, StairShape), BlockWithProperties>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

// General function to create any stair block with facing and shape properties
pub fn create_stair_with_properties(
    base_stair_block: Block,
    facing: StairFacing,
    shape: StairShape,
) -> BlockWithProperties {
    let cache_key = (base_stair_block, facing, shape);

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
pub const ACACIA_PLANKS: Block = Block::new("minecraft:acacia_planks");
pub const AIR: Block = Block::new("minecraft:air");
pub const ANDESITE: Block = Block::new("minecraft:andesite");
pub const BIRCH_LEAVES: Block = Block::new("minecraft:birch_leaves");
pub const BIRCH_LOG: Block = Block::new("minecraft:birch_log");
pub const BLACK_CONCRETE: Block = Block::new("minecraft:black_concrete");
pub const BLACKSTONE: Block = Block::new("minecraft:blackstone");
pub const BLUE_FLOWER: Block = Block::new("minecraft:blue_orchid");
pub const BLUE_TERRACOTTA: Block = Block::new("minecraft:blue_terracotta");
pub const BRICK: Block = Block::new("minecraft:bricks");
pub const CAULDRON: Block = Block::new("minecraft:cauldron");
pub const CHISELED_STONE_BRICKS: Block = Block::new("minecraft:chiseled_stone_bricks");
pub const COBBLESTONE_WALL: Block = Block::new("minecraft:cobblestone_wall");
pub const COBBLESTONE: Block = Block::new("minecraft:cobblestone");
pub const POLISHED_BLACKSTONE_BRICKS: Block = Block::new("minecraft:polished_blackstone_bricks");
pub const CRACKED_STONE_BRICKS: Block = Block::new("minecraft:cracked_stone_bricks");
pub const CRIMSON_PLANKS: Block = Block::new("minecraft:crimson_planks");
pub const CUT_SANDSTONE: Block = Block::new("minecraft:cut_sandstone");
pub const CYAN_CONCRETE: Block = Block::new("minecraft:cyan_concrete");
pub const DARK_OAK_PLANKS: Block = Block::new("minecraft:dark_oak_planks");
pub const DEEPSLATE_BRICKS: Block = Block::new("minecraft:deepslate_bricks");
pub const DIORITE: Block = Block::new("minecraft:diorite");
pub const DIRT: Block = Block::new("minecraft:dirt");
pub const END_STONE_BRICKS: Block = Block::new("minecraft:end_stone_bricks");
pub const FARMLAND: Block = Block::new("minecraft:farmland");
pub const GLASS: Block = Block::new("minecraft:glass");
pub const GLOWSTONE: Block = Block::new("minecraft:glowstone");
pub const GRANITE: Block = Block::new("minecraft:granite");
pub const GRASS_BLOCK: Block = Block::new("minecraft:grass_block");
pub const GRASS: Block = Block::new("minecraft:short_grass");
pub const GRAVEL: Block = Block::new("minecraft:gravel");
pub const GRAY_CONCRETE: Block = Block::new("minecraft:gray_concrete");
pub const GRAY_TERRACOTTA: Block = Block::new("minecraft:gray_terracotta");
pub const GREEN_STAINED_HARDENED_CLAY: Block = Block::new("minecraft:green_terracotta");
pub const GREEN_WOOL: Block = Block::new("minecraft:green_wool");
pub const HAY_BALE: Block = Block::new("minecraft:hay_block");
pub const IRON_BARS: Block = Block::new("minecraft:iron_bars");
pub const IRON_BLOCK: Block = Block::new("minecraft:iron_block");
pub const JUNGLE_PLANKS: Block = Block::new("minecraft:jungle_planks");
pub const LADDER: Block = Block::new("minecraft:ladder");
pub const LIGHT_BLUE_CONCRETE: Block = Block::new("minecraft:light_blue_concrete");
pub const LIGHT_BLUE_TERRACOTTA: Block = Block::new("minecraft:light_blue_terracotta");
pub const LIGHT_GRAY_CONCRETE: Block = Block::new("minecraft:light_gray_concrete");
pub const MOSS_BLOCK: Block = Block::new("minecraft:moss_block");
pub const MOSSY_COBBLESTONE: Block = Block::new("minecraft:mossy_cobblestone");
pub const MUD_BRICKS: Block = Block::new("minecraft:mud_bricks");
pub const NETHER_BRICK: Block = Block::new("minecraft:nether_bricks");
pub const NETHERITE_BLOCK: Block = Block::new("minecraft:netherite_block");
pub const OAK_FENCE: Block = Block::new("minecraft:oak_fence");
pub const OAK_LEAVES: Block = Block::new("minecraft:oak_leaves");
pub const OAK_LOG: Block = Block::new("minecraft:oak_log");
pub const OAK_PLANKS: Block = Block::new("minecraft:oak_planks");
pub const OAK_SLAB: Block = Block::new("minecraft:oak_slab");
pub const ORANGE_TERRACOTTA: Block = Block::new("minecraft:orange_terracotta");
pub const PODZOL: Block = Block::new("minecraft:podzol");
pub const POLISHED_ANDESITE: Block = Block::new("minecraft:polished_andesite");
pub const POLISHED_BASALT: Block = Block::new("minecraft:polished_basalt");
pub const QUARTZ_BLOCK: Block = Block::new("minecraft:quartz_block");
pub const POLISHED_BLACKSTONE: Block = Block::new("minecraft:polished_blackstone");
pub const POLISHED_DEEPSLATE: Block = Block::new("minecraft:polished_deepslate");
pub const POLISHED_DIORITE: Block = Block::new("minecraft:polished_diorite");
pub const POLISHED_GRANITE: Block = Block::new("minecraft:polished_granite");
pub const PRISMARINE: Block = Block::new("minecraft:prismarine");
pub const PURPUR_BLOCK: Block = Block::new("minecraft:purpur_block");
pub const PURPUR_PILLAR: Block = Block::new("minecraft:purpur_pillar");
pub const QUARTZ_BRICKS: Block = Block::new("minecraft:quartz_bricks");
pub const RAIL: Block = Block::new("minecraft:rail");
pub const RED_FLOWER: Block = Block::new("minecraft:poppy");
pub const RED_NETHER_BRICK: Block = Block::new("minecraft:red_nether_bricks");
pub const RED_TERRACOTTA: Block = Block::new("minecraft:red_terracotta");
pub const RED_WOOL: Block = Block::new("minecraft:red_wool");
pub const SAND: Block = Block::new("minecraft:sand");
pub const SANDSTONE: Block = Block::new("minecraft:sandstone");
pub const SCAFFOLDING: Block = Block::new("minecraft:scaffolding");
pub const SMOOTH_QUARTZ: Block = Block::new("minecraft:smooth_quartz");
pub const SMOOTH_RED_SANDSTONE: Block = Block::new("minecraft:smooth_red_sandstone");
pub const SMOOTH_SANDSTONE: Block = Block::new("minecraft:smooth_sandstone");
pub const SMOOTH_STONE: Block = Block::new("minecraft:smooth_stone");
pub const SPONGE: Block = Block::new("minecraft:sponge");
pub const SPRUCE_LOG: Block = Block::new("minecraft:spruce_log");
pub const SPRUCE_PLANKS: Block = Block::new("minecraft:spruce_planks");
pub const STONE_BLOCK_SLAB: Block = Block::new("minecraft:stone_slab");
pub const STONE_BRICK_SLAB: Block = Block::new("minecraft:stone_brick_slab");
pub const STONE_BRICKS: Block = Block::new("minecraft:stone_bricks");
pub const STONE: Block = Block::new("minecraft:stone");
pub const TERRACOTTA: Block = Block::new("minecraft:terracotta");
pub const WARPED_PLANKS: Block = Block::new("minecraft:warped_planks");
pub const WATER: Block = Block::new("minecraft:water");
pub const WHITE_CONCRETE: Block = Block::new("minecraft:white_concrete");
pub const WHITE_FLOWER: Block = Block::new("minecraft:azure_bluet");
pub const WHITE_STAINED_GLASS: Block = Block::new("minecraft:white_stained_glass");
pub const WHITE_TERRACOTTA: Block = Block::new("minecraft:white_terracotta");
pub const WHITE_WOOL: Block = Block::new("minecraft:white_wool");
pub const YELLOW_CONCRETE: Block = Block::new("minecraft:yellow_concrete");
pub const YELLOW_FLOWER: Block = Block::new("minecraft:dandelion");
pub const YELLOW_WOOL: Block = Block::new("minecraft:yellow_wool");
pub const LIME_CONCRETE: Block = Block::new("minecraft:lime_concrete");
pub const CYAN_WOOL: Block = Block::new("minecraft:cyan_wool");
pub const BLUE_CONCRETE: Block = Block::new("minecraft:blue_concrete");
pub const PURPLE_CONCRETE: Block = Block::new("minecraft:purple_concrete");
pub const RED_CONCRETE: Block = Block::new("minecraft:red_concrete");
pub const MAGENTA_CONCRETE: Block = Block::new("minecraft:magenta_concrete");
pub const BROWN_WOOL: Block = Block::new("minecraft:brown_wool");
pub const OXIDIZED_COPPER: Block = Block::new("minecraft:oxidized_copper");
pub const YELLOW_TERRACOTTA: Block = Block::new("minecraft:yellow_terracotta");
pub const SNOW_BLOCK: Block = Block::new("minecraft:snow_block");
pub const SNOW_LAYER: Block = Block::new("minecraft:snow");
pub const SIGN: Block = Block::new("minecraft:oak_sign");
pub const ANDESITE_WALL: Block = Block::new("minecraft:andesite_wall");
pub const STONE_BRICK_WALL: Block = Block::new("minecraft:stone_brick_wall");
pub const CARROTS: Block = Block::new("minecraft:carrots");
pub const DARK_OAK_DOOR_LOWER: Block = Block::new("minecraft:dark_oak_door");
pub const DARK_OAK_DOOR_UPPER: Block = Block::new("minecraft:dark_oak_door");
pub const POTATOES: Block = Block::new("minecraft:potatoes");
pub const WHEAT: Block = Block::new("minecraft:wheat");
pub const BEDROCK: Block = Block::new("minecraft:bedrock");
pub const RAIL_NORTH_SOUTH: Block = Block::new("minecraft:rail");
pub const RAIL_EAST_WEST: Block = Block::new("minecraft:rail");
pub const RAIL_ASCENDING_EAST: Block = Block::new("minecraft:rail");
pub const RAIL_ASCENDING_WEST: Block = Block::new("minecraft:rail");
pub const RAIL_ASCENDING_NORTH: Block = Block::new("minecraft:rail");
pub const RAIL_ASCENDING_SOUTH: Block = Block::new("minecraft:rail");
pub const RAIL_NORTH_EAST: Block = Block::new("minecraft:rail");
pub const RAIL_NORTH_WEST: Block = Block::new("minecraft:rail");
pub const RAIL_SOUTH_EAST: Block = Block::new("minecraft:rail");
pub const RAIL_SOUTH_WEST: Block = Block::new("minecraft:rail");
pub const COARSE_DIRT: Block = Block::new("minecraft:coarse_dirt");
pub const IRON_ORE: Block = Block::new("minecraft:iron_ore");
pub const COAL_ORE: Block = Block::new("minecraft:coal_ore");
pub const GOLD_ORE: Block = Block::new("minecraft:gold_ore");
pub const COPPER_ORE: Block = Block::new("minecraft:copper_ore");
pub const CLAY: Block = Block::new("minecraft:clay");
pub const DIRT_PATH: Block = Block::new("minecraft:dirt_path");
pub const ICE: Block = Block::new("minecraft:ice");
pub const PACKED_ICE: Block = Block::new("minecraft:packed_ice");
pub const MUD: Block = Block::new("minecraft:mud");
pub const DEAD_BUSH: Block = Block::new("minecraft:dead_bush");
pub const TALL_GRASS_BOTTOM: Block = Block::new("minecraft:tall_grass");
pub const TALL_GRASS_TOP: Block = Block::new("minecraft:tall_grass");
pub const CRAFTING_TABLE: Block = Block::new("minecraft:crafting_table");
pub const FURNACE: Block = Block::new("minecraft:furnace");
pub const WHITE_CARPET: Block = Block::new("minecraft:white_carpet");
pub const BOOKSHELF: Block = Block::new("minecraft:bookshelf");
pub const OAK_PRESSURE_PLATE: Block = Block::new("minecraft:oak_pressure_plate");
pub const OAK_STAIRS: Block = Block::new("minecraft:oak_stairs");
pub const CHEST: Block = Block::new("minecraft:chest");
pub const RED_CARPET: Block = Block::new("minecraft:red_carpet");
pub const ANVIL: Block = Block::new("minecraft:anvil");
pub const NOTE_BLOCK: Block = Block::new("minecraft:note_block");
pub const OAK_DOOR: Block = Block::new("minecraft:oak_door");
pub const BREWING_STAND: Block = Block::new("minecraft:brewing_stand");
pub const RED_BED_NORTH_HEAD: Block = Block::new("minecraft:red_bed");
pub const RED_BED_NORTH_FOOT: Block = Block::new("minecraft:red_bed");
pub const RED_BED_EAST_HEAD: Block = Block::new("minecraft:red_bed");
pub const RED_BED_EAST_FOOT: Block = Block::new("minecraft:red_bed");
pub const RED_BED_SOUTH_HEAD: Block = Block::new("minecraft:red_bed");
pub const RED_BED_SOUTH_FOOT: Block = Block::new("minecraft:red_bed");
pub const RED_BED_WEST_HEAD: Block = Block::new("minecraft:red_bed");
pub const RED_BED_WEST_FOOT: Block = Block::new("minecraft:red_bed");
pub const GRAY_STAINED_GLASS: Block = Block::new("minecraft:gray_stained_glass");
pub const LIGHT_GRAY_STAINED_GLASS: Block = Block::new("minecraft:light_gray_stained_glass");
pub const BROWN_STAINED_GLASS: Block = Block::new("minecraft:brown_stained_glass");
pub const TINTED_GLASS: Block = Block::new("minecraft:tinted_glass");
pub const OAK_TRAPDOOR: Block = Block::new("minecraft:oak_trapdoor");
pub const BROWN_CONCRETE: Block = Block::new("minecraft:brown_concrete");
pub const BLACK_TERRACOTTA: Block = Block::new("minecraft:black_terracotta");
pub const BROWN_TERRACOTTA: Block = Block::new("minecraft:brown_terracotta");
pub const STONE_BRICK_STAIRS: Block = Block::new("minecraft:stone_brick_stairs");
pub const MUD_BRICK_STAIRS: Block = Block::new("minecraft:mud_brick_stairs");
pub const POLISHED_BLACKSTONE_BRICK_STAIRS: Block =
    Block::new("minecraft:polished_blackstone_brick_stairs");
pub const BRICK_STAIRS: Block = Block::new("minecraft:brick_stairs");
pub const POLISHED_GRANITE_STAIRS: Block = Block::new("minecraft:polished_granite_stairs");
pub const END_STONE_BRICK_STAIRS: Block = Block::new("minecraft:end_stone_brick_stairs");
pub const POLISHED_DIORITE_STAIRS: Block = Block::new("minecraft:polished_diorite_stairs");
pub const SMOOTH_SANDSTONE_STAIRS: Block = Block::new("minecraft:smooth_sandstone_stairs");
pub const QUARTZ_STAIRS: Block = Block::new("minecraft:quartz_stairs");
pub const POLISHED_ANDESITE_STAIRS: Block = Block::new("minecraft:polished_andesite_stairs");
pub const NETHER_BRICK_STAIRS: Block = Block::new("minecraft:nether_brick_stairs");

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_new_stores_namespace_qualified_name() {
        let block = Block::new("minecraft:oak_planks");
        assert_eq!(block.name(), "minecraft:oak_planks");
    }

    #[test]
    fn block_constant_returns_namespaced_name() {
        assert_eq!(OAK_PLANKS.name(), "minecraft:oak_planks");
    }
}
