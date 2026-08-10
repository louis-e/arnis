//! Bedrock Block Mapping
//!
//! This module provides translation between the internal Block representation
//! and Bedrock Edition block format. Bedrock uses string identifiers with
//! state properties that differ slightly from Java Edition.

use crate::block_definitions::Block;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

/// Represents a Bedrock block with its identifier and state properties.
///
/// Uses `BTreeMap` for deterministic iteration order, which is required for
/// correct `Hash`/`Eq` implementations (used as palette dedup key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BedrockBlock {
    /// The Bedrock block identifier (e.g., "minecraft:stone")
    pub name: String,
    /// Block state properties as key-value pairs
    pub states: BTreeMap<String, BedrockBlockStateValue>,
}

/// `BTreeMap` does not implement `Hash`, so we hash entries in sorted-key order
/// (guaranteed by `BTreeMap::iter`).
impl Hash for BedrockBlock {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        for (k, v) in &self.states {
            k.hash(state);
            v.hash(state);
        }
    }
}

/// Bedrock block state values can be strings, booleans, or integers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BedrockBlockStateValue {
    String(String),
    Bool(bool),
    Int(i32),
}

impl BedrockBlock {
    /// Creates a simple block with no state properties.
    pub fn simple(name: &str) -> Self {
        Self {
            name: format!("minecraft:{name}"),
            states: BTreeMap::new(),
        }
    }

    /// Creates a block with state properties.
    pub fn with_states(name: &str, states: Vec<(&str, BedrockBlockStateValue)>) -> Self {
        let mut state_map = BTreeMap::new();
        for (key, value) in states {
            state_map.insert(key.to_string(), value);
        }
        Self {
            name: format!("minecraft:{name}"),
            states: state_map,
        }
    }
}

/// Converts an internal Block to a BedrockBlock representation.
///
/// This function handles the mapping between Java Edition block names/properties
/// and their Bedrock Edition equivalents. Many blocks are identical, but some
/// require translation of property names or values.
pub fn to_bedrock_block(block: Block) -> BedrockBlock {
    let java_name = block.name();

    // Most blocks have the same name in both editions
    // Handle special cases first, then fall back to direct mapping
    match java_name {
        // Grass block is just "grass_block" in both editions
        "grass_block" => BedrockBlock::simple("grass_block"),

        // Short grass is just "short_grass" in Java but "tallgrass" in Bedrock
        "short_grass" => BedrockBlock::with_states(
            "tallgrass",
            vec![(
                "tall_grass_type",
                BedrockBlockStateValue::String("tall".to_string()),
            )],
        ),

        // Tall grass needs height state
        "tall_grass" => BedrockBlock::with_states(
            "double_plant",
            vec![(
                "double_plant_type",
                BedrockBlockStateValue::String("grass".to_string()),
            )],
        ),

        // Bedrock never renamed sugar cane; "reeds" is still the current id.
        "sugar_cane" => {
            BedrockBlock::with_states("reeds", vec![("age", BedrockBlockStateValue::Int(0))])
        }

        // Oak leaves with persistence
        "oak_leaves" => BedrockBlock::with_states(
            "leaves",
            vec![
                (
                    "old_leaf_type",
                    BedrockBlockStateValue::String("oak".to_string()),
                ),
                ("persistent_bit", BedrockBlockStateValue::Bool(true)),
                ("update_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),

        // Birch leaves with persistence
        "birch_leaves" => BedrockBlock::with_states(
            "leaves",
            vec![
                (
                    "old_leaf_type",
                    BedrockBlockStateValue::String("birch".to_string()),
                ),
                ("persistent_bit", BedrockBlockStateValue::Bool(true)),
                ("update_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),

        // Oak log with axis (default up_down)
        "oak_log" => BedrockBlock::with_states(
            "oak_log",
            vec![(
                "pillar_axis",
                BedrockBlockStateValue::String("y".to_string()),
            )],
        ),

        // Birch log with axis
        "birch_log" => BedrockBlock::with_states(
            "birch_log",
            vec![(
                "pillar_axis",
                BedrockBlockStateValue::String("y".to_string()),
            )],
        ),

        // Spruce log with axis
        "spruce_log" => BedrockBlock::with_states(
            "spruce_log",
            vec![(
                "pillar_axis",
                BedrockBlockStateValue::String("y".to_string()),
            )],
        ),

        // Dark oak log with axis
        "dark_oak_log" => BedrockBlock::with_states(
            "dark_oak_log",
            vec![(
                "pillar_axis",
                BedrockBlockStateValue::String("y".to_string()),
            )],
        ),

        // Jungle log with axis
        "jungle_log" => BedrockBlock::with_states(
            "jungle_log",
            vec![(
                "pillar_axis",
                BedrockBlockStateValue::String("y".to_string()),
            )],
        ),

        // Acacia log with axis
        "acacia_log" => BedrockBlock::with_states(
            "acacia_log",
            vec![(
                "pillar_axis",
                BedrockBlockStateValue::String("y".to_string()),
            )],
        ),

        // Spruce leaves with persistence
        "spruce_leaves" => BedrockBlock::with_states(
            "leaves",
            vec![
                (
                    "old_leaf_type",
                    BedrockBlockStateValue::String("spruce".to_string()),
                ),
                ("persistent_bit", BedrockBlockStateValue::Bool(true)),
                ("update_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),

        // Dark oak leaves with persistence
        "dark_oak_leaves" => BedrockBlock::with_states(
            "leaves2",
            vec![
                (
                    "new_leaf_type",
                    BedrockBlockStateValue::String("dark_oak".to_string()),
                ),
                ("persistent_bit", BedrockBlockStateValue::Bool(true)),
                ("update_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),

        // Jungle leaves with persistence
        "jungle_leaves" => BedrockBlock::with_states(
            "leaves",
            vec![
                (
                    "old_leaf_type",
                    BedrockBlockStateValue::String("jungle".to_string()),
                ),
                ("persistent_bit", BedrockBlockStateValue::Bool(true)),
                ("update_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),

        // Acacia leaves with persistence
        "acacia_leaves" => BedrockBlock::with_states(
            "leaves2",
            vec![
                (
                    "new_leaf_type",
                    BedrockBlockStateValue::String("acacia".to_string()),
                ),
                ("persistent_bit", BedrockBlockStateValue::Bool(true)),
                ("update_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),

        // Cherry leaves with persistence (1.20+)
        "cherry_leaves" => BedrockBlock::with_states(
            "cherry_leaves",
            vec![
                ("persistent_bit", BedrockBlockStateValue::Bool(true)),
                ("update_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),

        // Mangrove leaves with persistence (1.19+)
        "mangrove_leaves" => BedrockBlock::with_states(
            "mangrove_leaves",
            vec![
                ("persistent_bit", BedrockBlockStateValue::Bool(true)),
                ("update_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),

        // Azalea leaves with persistence (1.17+)
        "azalea_leaves" => BedrockBlock::with_states(
            "azalea_leaves",
            vec![
                ("persistent_bit", BedrockBlockStateValue::Bool(true)),
                ("update_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),

        // Slabs and stairs share their name/state handling with the
        // property-aware path so both entry points stay in sync.
        name if name.ends_with("_slab") => convert_slab(name, None),
        name if name.ends_with("_stairs") => convert_stairs(name, None),

        // Water (flowing by default)
        "water" => BedrockBlock::with_states(
            "water",
            vec![("liquid_depth", BedrockBlockStateValue::Int(0))],
        ),

        // Rail with shape state
        "rail" => BedrockBlock::with_states(
            "rail",
            vec![("rail_direction", BedrockBlockStateValue::Int(0))],
        ),

        // Farmland with moisture
        "farmland" => BedrockBlock::with_states(
            "farmland",
            vec![("moisturized_amount", BedrockBlockStateValue::Int(7))],
        ),

        // Snow layer
        "snow" => BedrockBlock::with_states(
            "snow_layer",
            vec![
                ("height", BedrockBlockStateValue::Int(0)),
                ("covered_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),

        // Cobblestone wall
        "cobblestone_wall" => BedrockBlock::with_states(
            "cobblestone_wall",
            vec![(
                "wall_block_type",
                BedrockBlockStateValue::String("cobblestone".to_string()),
            )],
        ),

        // Andesite wall
        "andesite_wall" => BedrockBlock::with_states(
            "cobblestone_wall",
            vec![(
                "wall_block_type",
                BedrockBlockStateValue::String("andesite".to_string()),
            )],
        ),

        // Stone brick wall
        "stone_brick_wall" => BedrockBlock::with_states(
            "cobblestone_wall",
            vec![(
                "wall_block_type",
                BedrockBlockStateValue::String("stone_brick".to_string()),
            )],
        ),
        "brick_wall" => BedrockBlock::with_states(
            "cobblestone_wall",
            vec![(
                "wall_block_type",
                BedrockBlockStateValue::String("brick".to_string()),
            )],
        ),

        // Flowers - poppy is just "red_flower" in Bedrock
        "poppy" => BedrockBlock::with_states(
            "red_flower",
            vec![(
                "flower_type",
                BedrockBlockStateValue::String("poppy".to_string()),
            )],
        ),

        // Dandelion is "yellow_flower" in Bedrock
        "dandelion" => BedrockBlock::simple("yellow_flower"),

        // Blue orchid
        "blue_orchid" => BedrockBlock::with_states(
            "red_flower",
            vec![(
                "flower_type",
                BedrockBlockStateValue::String("orchid".to_string()),
            )],
        ),

        // Azure bluet
        "azure_bluet" => BedrockBlock::with_states(
            "red_flower",
            vec![(
                "flower_type",
                BedrockBlockStateValue::String("houstonia".to_string()),
            )],
        ),

        // Concrete colors (Bedrock uses a single block with color state)
        "white_concrete" => BedrockBlock::with_states(
            "concrete",
            vec![("color", BedrockBlockStateValue::String("white".to_string()))],
        ),
        "black_concrete" => BedrockBlock::with_states(
            "concrete",
            vec![("color", BedrockBlockStateValue::String("black".to_string()))],
        ),
        "gray_concrete" => BedrockBlock::with_states(
            "concrete",
            vec![("color", BedrockBlockStateValue::String("gray".to_string()))],
        ),
        "gray_concrete_powder" => BedrockBlock::with_states(
            "concretePowder",
            vec![("color", BedrockBlockStateValue::String("gray".to_string()))],
        ),
        "brown_concrete_powder" => BedrockBlock::with_states(
            "concretePowder",
            vec![("color", BedrockBlockStateValue::String("brown".to_string()))],
        ),
        "light_gray_concrete" => BedrockBlock::with_states(
            "concrete",
            vec![(
                "color",
                BedrockBlockStateValue::String("silver".to_string()),
            )],
        ),
        "light_blue_concrete" => BedrockBlock::with_states(
            "concrete",
            vec![(
                "color",
                BedrockBlockStateValue::String("light_blue".to_string()),
            )],
        ),
        "cyan_concrete" => BedrockBlock::with_states(
            "concrete",
            vec![("color", BedrockBlockStateValue::String("cyan".to_string()))],
        ),
        "blue_concrete" => BedrockBlock::with_states(
            "concrete",
            vec![("color", BedrockBlockStateValue::String("blue".to_string()))],
        ),
        "purple_concrete" => BedrockBlock::with_states(
            "concrete",
            vec![(
                "color",
                BedrockBlockStateValue::String("purple".to_string()),
            )],
        ),
        "magenta_concrete" => BedrockBlock::with_states(
            "concrete",
            vec![(
                "color",
                BedrockBlockStateValue::String("magenta".to_string()),
            )],
        ),
        "red_concrete" => BedrockBlock::with_states(
            "concrete",
            vec![("color", BedrockBlockStateValue::String("red".to_string()))],
        ),
        "orange_concrete" => BedrockBlock::with_states(
            "concrete",
            vec![(
                "color",
                BedrockBlockStateValue::String("orange".to_string()),
            )],
        ),
        "yellow_concrete" => BedrockBlock::with_states(
            "concrete",
            vec![(
                "color",
                BedrockBlockStateValue::String("yellow".to_string()),
            )],
        ),
        "lime_concrete" => BedrockBlock::with_states(
            "concrete",
            vec![("color", BedrockBlockStateValue::String("lime".to_string()))],
        ),
        "brown_concrete" => BedrockBlock::with_states(
            "concrete",
            vec![("color", BedrockBlockStateValue::String("brown".to_string()))],
        ),
        "green_concrete" => BedrockBlock::with_states(
            "concrete",
            vec![("color", BedrockBlockStateValue::String("green".to_string()))],
        ),

        // Terracotta colors
        "white_terracotta" => BedrockBlock::with_states(
            "stained_hardened_clay",
            vec![("color", BedrockBlockStateValue::String("white".to_string()))],
        ),
        "orange_terracotta" => BedrockBlock::with_states(
            "stained_hardened_clay",
            vec![(
                "color",
                BedrockBlockStateValue::String("orange".to_string()),
            )],
        ),
        "yellow_terracotta" => BedrockBlock::with_states(
            "stained_hardened_clay",
            vec![(
                "color",
                BedrockBlockStateValue::String("yellow".to_string()),
            )],
        ),
        "light_blue_terracotta" => BedrockBlock::with_states(
            "stained_hardened_clay",
            vec![(
                "color",
                BedrockBlockStateValue::String("light_blue".to_string()),
            )],
        ),
        "blue_terracotta" => BedrockBlock::with_states(
            "stained_hardened_clay",
            vec![("color", BedrockBlockStateValue::String("blue".to_string()))],
        ),
        "cyan_terracotta" => BedrockBlock::with_states(
            "stained_hardened_clay",
            vec![("color", BedrockBlockStateValue::String("cyan".to_string()))],
        ),
        "gray_terracotta" => BedrockBlock::with_states(
            "stained_hardened_clay",
            vec![("color", BedrockBlockStateValue::String("gray".to_string()))],
        ),
        "green_terracotta" => BedrockBlock::with_states(
            "stained_hardened_clay",
            vec![("color", BedrockBlockStateValue::String("green".to_string()))],
        ),
        "red_terracotta" => BedrockBlock::with_states(
            "stained_hardened_clay",
            vec![("color", BedrockBlockStateValue::String("red".to_string()))],
        ),
        "brown_terracotta" => BedrockBlock::with_states(
            "stained_hardened_clay",
            vec![("color", BedrockBlockStateValue::String("brown".to_string()))],
        ),
        "black_terracotta" => BedrockBlock::with_states(
            "stained_hardened_clay",
            vec![("color", BedrockBlockStateValue::String("black".to_string()))],
        ),
        "light_gray_terracotta" => BedrockBlock::with_states(
            "stained_hardened_clay",
            vec![(
                "color",
                BedrockBlockStateValue::String("silver".to_string()),
            )],
        ),
        // Plain terracotta
        "terracotta" => BedrockBlock::simple("hardened_clay"),
        // Wall banner — Bedrock uses "minecraft:wall_banner" with a
        // "facing_direction" int state: 2=north, 3=south, 4=west, 5=east.
        // The color is stored in the block entity (Base field), not the block state.
        // The facing string→int mapping is handled by to_bedrock_block_with_properties.
        // Every dyed variant collapses onto the same Bedrock block, so match
        // on the suffix instead of listing colours (a missing colour used to
        // fall through to a non-existent "minecraft:<colour>_wall_banner").
        name if name.ends_with("_wall_banner") => BedrockBlock::with_states(
            "wall_banner",
            vec![("facing_direction", BedrockBlockStateValue::Int(2))], // default north
        ),
        // Wool colors
        "white_wool" => BedrockBlock::with_states(
            "wool",
            vec![("color", BedrockBlockStateValue::String("white".to_string()))],
        ),
        "black_wool" => BedrockBlock::with_states(
            "wool",
            vec![("color", BedrockBlockStateValue::String("black".to_string()))],
        ),
        "red_wool" => BedrockBlock::with_states(
            "wool",
            vec![("color", BedrockBlockStateValue::String("red".to_string()))],
        ),
        "green_wool" => BedrockBlock::with_states(
            "wool",
            vec![("color", BedrockBlockStateValue::String("green".to_string()))],
        ),
        "brown_wool" => BedrockBlock::with_states(
            "wool",
            vec![("color", BedrockBlockStateValue::String("brown".to_string()))],
        ),
        "cyan_wool" => BedrockBlock::with_states(
            "wool",
            vec![("color", BedrockBlockStateValue::String("cyan".to_string()))],
        ),
        "yellow_wool" => BedrockBlock::with_states(
            "wool",
            vec![(
                "color",
                BedrockBlockStateValue::String("yellow".to_string()),
            )],
        ),
        "orange_wool" => BedrockBlock::with_states(
            "wool",
            vec![(
                "color",
                BedrockBlockStateValue::String("orange".to_string()),
            )],
        ),
        "blue_wool" => BedrockBlock::with_states(
            "wool",
            vec![("color", BedrockBlockStateValue::String("blue".to_string()))],
        ),

        // Carpets
        "white_carpet" => BedrockBlock::with_states(
            "carpet",
            vec![("color", BedrockBlockStateValue::String("white".to_string()))],
        ),
        "red_carpet" => BedrockBlock::with_states(
            "carpet",
            vec![("color", BedrockBlockStateValue::String("red".to_string()))],
        ),

        // Stained glass
        "white_stained_glass" => BedrockBlock::with_states(
            "stained_glass",
            vec![("color", BedrockBlockStateValue::String("white".to_string()))],
        ),
        "gray_stained_glass" => BedrockBlock::with_states(
            "stained_glass",
            vec![("color", BedrockBlockStateValue::String("gray".to_string()))],
        ),
        "light_gray_stained_glass" => BedrockBlock::with_states(
            "stained_glass",
            vec![(
                "color",
                BedrockBlockStateValue::String("silver".to_string()),
            )],
        ),
        "brown_stained_glass" => BedrockBlock::with_states(
            "stained_glass",
            vec![("color", BedrockBlockStateValue::String("brown".to_string()))],
        ),
        "cyan_stained_glass" => BedrockBlock::with_states(
            "stained_glass",
            vec![("color", BedrockBlockStateValue::String("cyan".to_string()))],
        ),
        "blue_stained_glass" => BedrockBlock::with_states(
            "stained_glass",
            vec![("color", BedrockBlockStateValue::String("blue".to_string()))],
        ),
        "light_blue_stained_glass" => BedrockBlock::with_states(
            "stained_glass",
            vec![(
                "color",
                BedrockBlockStateValue::String("light_blue".to_string()),
            )],
        ),
        "red_stained_glass" => BedrockBlock::with_states(
            "stained_glass",
            vec![("color", BedrockBlockStateValue::String("red".to_string()))],
        ),
        "yellow_stained_glass" => BedrockBlock::with_states(
            "stained_glass",
            vec![(
                "color",
                BedrockBlockStateValue::String("yellow".to_string()),
            )],
        ),
        "purple_stained_glass" => BedrockBlock::with_states(
            "stained_glass",
            vec![(
                "color",
                BedrockBlockStateValue::String("purple".to_string()),
            )],
        ),
        "orange_stained_glass" => BedrockBlock::with_states(
            "stained_glass",
            vec![(
                "color",
                BedrockBlockStateValue::String("orange".to_string()),
            )],
        ),
        "magenta_stained_glass" => BedrockBlock::with_states(
            "stained_glass",
            vec![(
                "color",
                BedrockBlockStateValue::String("magenta".to_string()),
            )],
        ),
        "daylight_detector" => BedrockBlock::simple("daylight_detector"),
        "redstone_lamp" => BedrockBlock::simple("redstone_lamp"),

        // Planks - Bedrock uses single "planks" block with wood_type state
        "oak_planks" => BedrockBlock::with_states(
            "planks",
            vec![(
                "wood_type",
                BedrockBlockStateValue::String("oak".to_string()),
            )],
        ),
        "spruce_planks" => BedrockBlock::with_states(
            "planks",
            vec![(
                "wood_type",
                BedrockBlockStateValue::String("spruce".to_string()),
            )],
        ),
        "birch_planks" => BedrockBlock::with_states(
            "planks",
            vec![(
                "wood_type",
                BedrockBlockStateValue::String("birch".to_string()),
            )],
        ),
        "jungle_planks" => BedrockBlock::with_states(
            "planks",
            vec![(
                "wood_type",
                BedrockBlockStateValue::String("jungle".to_string()),
            )],
        ),
        "acacia_planks" => BedrockBlock::with_states(
            "planks",
            vec![(
                "wood_type",
                BedrockBlockStateValue::String("acacia".to_string()),
            )],
        ),
        "dark_oak_planks" => BedrockBlock::with_states(
            "planks",
            vec![(
                "wood_type",
                BedrockBlockStateValue::String("dark_oak".to_string()),
            )],
        ),
        "crimson_planks" => BedrockBlock::simple("crimson_planks"),
        "warped_planks" => BedrockBlock::simple("warped_planks"),

        // Stone variants
        "stone" => BedrockBlock::simple("stone"),
        "granite" => BedrockBlock::with_states(
            "stone",
            vec![(
                "stone_type",
                BedrockBlockStateValue::String("granite".to_string()),
            )],
        ),
        "polished_granite" => BedrockBlock::with_states(
            "stone",
            vec![(
                "stone_type",
                BedrockBlockStateValue::String("granite_smooth".to_string()),
            )],
        ),
        "diorite" => BedrockBlock::with_states(
            "stone",
            vec![(
                "stone_type",
                BedrockBlockStateValue::String("diorite".to_string()),
            )],
        ),
        "polished_diorite" => BedrockBlock::with_states(
            "stone",
            vec![(
                "stone_type",
                BedrockBlockStateValue::String("diorite_smooth".to_string()),
            )],
        ),
        "andesite" => BedrockBlock::with_states(
            "stone",
            vec![(
                "stone_type",
                BedrockBlockStateValue::String("andesite".to_string()),
            )],
        ),
        "polished_andesite" => BedrockBlock::with_states(
            "stone",
            vec![(
                "stone_type",
                BedrockBlockStateValue::String("andesite_smooth".to_string()),
            )],
        ),

        // Blocks with different names in Bedrock
        "bricks" => BedrockBlock::simple("brick_block"),
        "end_stone_bricks" => BedrockBlock::simple("end_bricks"),
        "nether_bricks" => BedrockBlock::simple("nether_brick"),
        "red_nether_bricks" => BedrockBlock::simple("red_nether_brick"),
        "snow_block" => BedrockBlock::simple("snow"),
        "dirt_path" => BedrockBlock::simple("grass_path"),
        "dead_bush" => BedrockBlock::simple("deadbush"),
        "note_block" => BedrockBlock::simple("noteblock"),

        // Bedrock never renamed these to match Java's flattening, so the
        // Java name resolves to nothing at all.
        "magma_block" => BedrockBlock::simple("magma"),
        "waxed_copper_block" => BedrockBlock::simple("waxed_copper"),
        "oak_button" => BedrockBlock::with_states(
            "wooden_button",
            vec![
                ("facing_direction", BedrockBlockStateValue::Int(1)),
                ("button_pressed_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),
        "oak_fence_gate" => BedrockBlock::with_states(
            "fence_gate",
            vec![
                ("direction", BedrockBlockStateValue::Int(0)),
                ("in_wall_bit", BedrockBlockStateValue::Bool(false)),
                ("open_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),
        "powered_rail" => BedrockBlock::with_states(
            "golden_rail",
            vec![
                ("rail_direction", BedrockBlockStateValue::Int(0)),
                ("rail_data_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),

        // Cauldrons: Bedrock keeps one block and encodes the contents in states.
        "cauldron" => BedrockBlock::with_states(
            "cauldron",
            vec![
                (
                    "cauldron_liquid",
                    BedrockBlockStateValue::String("water".to_string()),
                ),
                ("fill_level", BedrockBlockStateValue::Int(0)),
            ],
        ),
        "water_cauldron" => BedrockBlock::with_states(
            "cauldron",
            vec![
                (
                    "cauldron_liquid",
                    BedrockBlockStateValue::String("water".to_string()),
                ),
                ("fill_level", BedrockBlockStateValue::Int(6)),
            ],
        ),

        // Bedrock has no separate kelp stem block: a column is all
        // minecraft:kelp, and kelp_age on the top block drives further
        // growth. 25 is fully grown, so generated columns stay as placed.
        "kelp_plant" | "kelp" => {
            BedrockBlock::with_states("kelp", vec![("kelp_age", BedrockBlockStateValue::Int(25))])
        }

        // Seagrass: one Bedrock block, the half is encoded in sea_grass_type.
        "seagrass" => BedrockBlock::with_states(
            "seagrass",
            vec![(
                "sea_grass_type",
                BedrockBlockStateValue::String("default".to_string()),
            )],
        ),

        // Oak items mapped to dark_oak in Bedrock (or generic equivalents)
        "oak_pressure_plate" => BedrockBlock::with_states(
            "wooden_pressure_plate",
            vec![("redstone_signal", BedrockBlockStateValue::Int(0))],
        ),
        "oak_door" => BedrockBlock::simple("wooden_door"),
        "spruce_door" => BedrockBlock::simple("spruce_door"),
        "dark_oak_door" => BedrockBlock::simple("dark_oak_door"),
        "oak_trapdoor" => BedrockBlock::simple("trapdoor"),

        // Vegetation with different Bedrock names
        "fern" => BedrockBlock::with_states(
            "tallgrass",
            vec![(
                "tall_grass_type",
                BedrockBlockStateValue::String("fern".to_string()),
            )],
        ),
        "large_fern" => BedrockBlock::with_states(
            "double_plant",
            vec![(
                "double_plant_type",
                BedrockBlockStateValue::String("fern".to_string()),
            )],
        ),
        "cobweb" => BedrockBlock::simple("web"),

        // Potted plants (Bedrock uses "flower_pot" for all variants;
        // the contained plant is a block entity, not a block state)
        "flower_pot" => BedrockBlock::with_states(
            "flower_pot",
            vec![("update_bit", BedrockBlockStateValue::Bool(false))],
        ),
        "potted_poppy" => BedrockBlock::with_states(
            "flower_pot",
            vec![("update_bit", BedrockBlockStateValue::Bool(false))],
        ),
        "potted_red_tulip" => BedrockBlock::with_states(
            "flower_pot",
            vec![("update_bit", BedrockBlockStateValue::Bool(false))],
        ),
        "potted_dandelion" => BedrockBlock::with_states(
            "flower_pot",
            vec![("update_bit", BedrockBlockStateValue::Bool(false))],
        ),
        "potted_blue_orchid" => BedrockBlock::with_states(
            "flower_pot",
            vec![("update_bit", BedrockBlockStateValue::Bool(false))],
        ),

        // Beds always carry facing/part properties, so convert_bed owns them;
        // Bedrock stores the colour in the Bed block entity, not the state.
        name if name.ends_with("_bed") => convert_bed(name, None),

        // Default: use the same name (works for many blocks)
        // Log unmapped blocks to help identify missing mappings
        _ => BedrockBlock::simple(java_name),
    }
}

/// Converts an internal Block with optional Java properties to a BedrockBlock.
///
/// This function extends `to_bedrock_block` by also handling block-specific properties
/// like stair facing/shape, slab type, etc. Java property names and values are converted
/// to their Bedrock equivalents.
pub fn to_bedrock_block_with_properties(
    block: Block,
    java_properties: Option<&fastnbt::Value>,
) -> BedrockBlock {
    let java_name = block.name();

    // If no stored properties were passed, fall back to block.properties()
    // so that blocks placed via set_block_absolute (e.g. doors with half=upper/lower)
    // still get their default properties forwarded to the Bedrock converter.
    let fallback_props = block.properties();
    let effective_properties = java_properties.or(fallback_props.as_ref());

    // Extract Java properties as a map if present
    let props_map = effective_properties.and_then(|v| {
        if let fastnbt::Value::Compound(map) = v {
            Some(map)
        } else {
            None
        }
    });

    // Handle stairs with facing/shape properties
    if java_name.ends_with("_stairs") {
        return convert_stairs(java_name, props_map);
    }

    // Handle barrel facing direction
    if java_name == "barrel" {
        return convert_barrel(java_name, props_map);
    }

    // Inverted daylight sensors and lit redstone lamps are separate blocks on Bedrock.
    if java_name == "daylight_detector"
        && props_map
            .and_then(|m| m.get("inverted"))
            .is_some_and(|v| matches!(v, fastnbt::Value::String(s) if s == "true"))
    {
        return BedrockBlock::with_states(
            "daylight_detector_inverted",
            vec![("redstone_signal", BedrockBlockStateValue::Int(11))],
        );
    }
    if java_name == "redstone_lamp"
        && props_map
            .and_then(|m| m.get("lit"))
            .is_some_and(|v| matches!(v, fastnbt::Value::String(s) if s == "true"))
    {
        return BedrockBlock::simple("lit_redstone_lamp");
    }

    // Handle slabs with type property (top/bottom/double)
    if java_name.ends_with("_slab") {
        return convert_slab(java_name, props_map);
    }

    // Handle logs (and chains, which use the same pillar_axis) with axis property
    if java_name.ends_with("_log") || java_name.ends_with("_wood") || java_name == "chain" {
        return convert_log(java_name, props_map);
    }

    // Handle doors with half property (upper/lower → upper_block_bit)
    if java_name.ends_with("_door") && java_name != "iron_door" {
        return convert_door(java_name, props_map);
    }

    // Handle trapdoors with facing/open/half properties
    if java_name.ends_with("_trapdoor") {
        return convert_trapdoor(java_name, props_map);
    }

    // Handle beds with facing/part/occupied properties
    if java_name.ends_with("_bed") {
        return convert_bed(java_name, props_map);
    }

    // Handle rails with shape property
    if java_name == "rail" {
        return convert_rail(props_map);
    }

    // Handle wall banners with facing property
    if java_name.ends_with("_wall_banner") {
        return convert_wall_banner(props_map);
    }

    // Blocks whose Java property has a differently named Bedrock counterpart
    if java_name == "redstone_wall_torch" {
        return convert_redstone_wall_torch(props_map);
    }
    if matches!(java_name, "wheat" | "carrots" | "potatoes") {
        return convert_crop(java_name, props_map);
    }
    if matches!(java_name, "tall_grass" | "large_fern") {
        return convert_double_plant(java_name, props_map);
    }
    if java_name == "tall_seagrass" {
        return convert_tall_seagrass(props_map);
    }
    if java_name == "oak_sign" {
        return convert_standing_sign(props_map);
    }
    if java_name == "sea_pickle" {
        return convert_sea_pickle(props_map);
    }
    if java_name == "chiseled_bookshelf" {
        return convert_chiseled_bookshelf(props_map);
    }

    // Fall back to basic conversion without properties
    to_bedrock_block(block)
}

/// Convert Java stair block to Bedrock format with proper orientation.
fn convert_stairs(
    java_name: &str,
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    // Map Java stair names to Bedrock equivalents.
    //
    // Bedrock never renamed its two oldest stair ids, so they read as swapped
    // next to Java: "minecraft:stone_stairs" IS cobblestone stairs, and the
    // stone ones are "minecraft:normal_stone_stairs". Taking the Java names
    // at face value gave cobblestone stairs a non-existent id and turned
    // stone stairs into cobblestone.
    let bedrock_name = match java_name {
        "end_stone_brick_stairs" => "end_brick_stairs",
        "cobblestone_stairs" => "stone_stairs",
        "stone_stairs" => "normal_stone_stairs",
        _ => java_name, // Most stairs have the same name
    };

    let mut states = BTreeMap::new();

    // Convert facing: Java uses "north/south/east/west", Bedrock uses "weirdo_direction" (0-3)
    // Bedrock: 0=east, 1=west, 2=south, 3=north
    if let Some(props) = props {
        if let Some(fastnbt::Value::String(facing)) = props.get("facing") {
            let direction = match facing.as_str() {
                "east" => 0,
                "west" => 1,
                "south" => 2,
                "north" => 3,
                _ => 0,
            };
            states.insert(
                "weirdo_direction".to_string(),
                BedrockBlockStateValue::Int(direction),
            );
        }

        // Convert half: Java uses "top/bottom", Bedrock uses "upside_down_bit"
        if let Some(fastnbt::Value::String(half)) = props.get("half") {
            let upside_down = half == "top";
            states.insert(
                "upside_down_bit".to_string(),
                BedrockBlockStateValue::Bool(upside_down),
            );
        }
    }

    // If no properties were set, use defaults
    if states.is_empty() {
        states.insert(
            "weirdo_direction".to_string(),
            BedrockBlockStateValue::Int(0),
        );
        states.insert(
            "upside_down_bit".to_string(),
            BedrockBlockStateValue::Bool(false),
        );
    }

    BedrockBlock {
        name: format!("minecraft:{bedrock_name}"),
        states,
    }
}

/// Convert Java barrel to Bedrock format with facing direction.
fn convert_barrel(
    java_name: &str,
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    let mut states = BTreeMap::new();

    if let Some(props) = props {
        if let Some(fastnbt::Value::String(facing)) = props.get("facing") {
            let facing_direction = match facing.as_str() {
                "down" => 0,
                "up" => 1,
                "north" => 2,
                "south" => 3,
                "west" => 4,
                "east" => 5,
                _ => 1,
            };
            states.insert(
                "facing_direction".to_string(),
                BedrockBlockStateValue::Int(facing_direction),
            );
        }
    }

    if !states.contains_key("facing_direction") {
        states.insert(
            "facing_direction".to_string(),
            BedrockBlockStateValue::Int(1),
        );
    }

    states.insert("open_bit".to_string(), BedrockBlockStateValue::Bool(false));

    BedrockBlock {
        name: format!("minecraft:{java_name}"),
        states,
    }
}

/// Mineral slabs live in four numbered Bedrock families, each with its own
/// `stone_slab_type*` state. The flattened per-material ids (`minecraft:andesite_slab`
/// and friends) only exist from 1.21.30 onwards and take `minecraft:vertical_half`
/// instead of `top_slot_bit`, so emitting one of those names together with
/// `top_slot_bit` yields a state set no Bedrock version can resolve. Going through
/// the legacy family ids keeps a single encoding that every version resolves.
///
/// Returns `(bedrock_id, state_name, state_value)`.
fn stone_slab_family(java_name: &str) -> Option<(&'static str, &'static str, &'static str)> {
    let (family, value) = match java_name {
        "smooth_stone_slab" => (1, "smooth_stone"),
        "sandstone_slab" => (1, "sandstone"),
        "petrified_oak_slab" => (1, "wood"),
        "cobblestone_slab" => (1, "cobblestone"),
        "brick_slab" => (1, "brick"),
        "stone_brick_slab" => (1, "stone_brick"),
        "quartz_slab" => (1, "quartz"),
        "nether_brick_slab" => (1, "nether_brick"),

        "red_sandstone_slab" => (2, "red_sandstone"),
        "purpur_slab" => (2, "purpur"),
        "prismarine_slab" => (2, "prismarine_rough"),
        "dark_prismarine_slab" => (2, "prismarine_dark"),
        "prismarine_brick_slab" => (2, "prismarine_brick"),
        "mossy_cobblestone_slab" => (2, "mossy_cobblestone"),
        "smooth_sandstone_slab" => (2, "smooth_sandstone"),
        "red_nether_brick_slab" => (2, "red_nether_brick"),

        "end_stone_brick_slab" => (3, "end_stone_brick"),
        "smooth_red_sandstone_slab" => (3, "smooth_red_sandstone"),
        "polished_andesite_slab" => (3, "polished_andesite"),
        "andesite_slab" => (3, "andesite"),
        "diorite_slab" => (3, "diorite"),
        "polished_diorite_slab" => (3, "polished_diorite"),
        "granite_slab" => (3, "granite"),
        "polished_granite_slab" => (3, "polished_granite"),

        "mossy_stone_brick_slab" => (4, "mossy_stone_brick"),
        "smooth_quartz_slab" => (4, "smooth_quartz"),
        // Java "stone_slab" is the plain stone slab, which lives in family 4.
        // (Family 1's "smooth_stone" is Java's smooth_stone_slab.)
        "stone_slab" => (4, "stone"),
        "cut_sandstone_slab" => (4, "cut_sandstone"),
        "cut_red_sandstone_slab" => (4, "cut_red_sandstone"),

        _ => return None,
    };

    Some(match family {
        1 => ("stone_block_slab", "stone_slab_type", value),
        2 => ("stone_block_slab2", "stone_slab_type_2", value),
        3 => ("stone_block_slab3", "stone_slab_type_3", value),
        _ => ("stone_block_slab4", "stone_slab_type_4", value),
    })
}

/// Convert Java slab block to Bedrock format with proper type.
///
/// Java encodes a full-height slab as `type=double` on the slab block itself.
/// Bedrock instead has a separate double-slab id per material, so `type=double`
/// selects a different block rather than a different state. Treating it as an
/// ordinary slab left half-height blocks in place of full ones — the bundled
/// schematics (cars, boats, bridge segments, playgrounds) rely on double slabs.
fn convert_slab(
    java_name: &str,
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    let slab_type = props.and_then(|p| p.get("type")).and_then(|v| match v {
        fastnbt::Value::String(s) => Some(s.as_str()),
        _ => None,
    });
    let is_double = slab_type == Some("double");

    let mut states = BTreeMap::new();

    // Convert type: Java uses "top"/"bottom", Bedrock uses "top_slot_bit".
    // Double slabs keep the state (it is inert for them) so that the legacy
    // family ids still carry their full state set.
    states.insert(
        "top_slot_bit".to_string(),
        BedrockBlockStateValue::Bool(slab_type == Some("top")),
    );

    let bedrock_name = if let Some((name, state, value)) = stone_slab_family(java_name) {
        states.insert(
            state.to_string(),
            BedrockBlockStateValue::String(value.to_string()),
        );
        if is_double {
            // stone_block_slab3 -> double_stone_block_slab3
            format!("double_{name}")
        } else {
            name.to_string()
        }
    } else if matches!(
        java_name,
        "oak_slab" | "spruce_slab" | "birch_slab" | "jungle_slab" | "acacia_slab" | "dark_oak_slab"
    ) {
        states.insert(
            "wood_type".to_string(),
            BedrockBlockStateValue::String(java_name.trim_end_matches("_slab").to_string()),
        );
        if is_double {
            "double_wooden_slab".to_string()
        } else {
            "wooden_slab".to_string()
        }
    } else {
        // Slabs added after the wood/stone families (blackstone, deepslate, mud
        // brick, bamboo, warped, cut copper, ...) already have their own id and
        // took top_slot_bit from the start, so the name carries over as-is.
        // Their double form is spelled "<material>_double_slab".
        match java_name.strip_suffix("_slab") {
            Some(material) if is_double => format!("{material}_double_slab"),
            _ => java_name.to_string(),
        }
    };

    BedrockBlock {
        name: format!("minecraft:{bedrock_name}"),
        states,
    }
}

/// Convert Java log/wood block to Bedrock format with proper axis.
fn convert_log(
    java_name: &str,
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    let bedrock_name = java_name;
    let mut states = BTreeMap::new();

    // Convert axis: Java uses "x/y/z", Bedrock uses "pillar_axis"
    if let Some(props) = props {
        if let Some(fastnbt::Value::String(axis)) = props.get("axis") {
            states.insert(
                "pillar_axis".to_string(),
                BedrockBlockStateValue::String(axis.clone()),
            );
        }
    }

    // Default to y-axis if not specified
    if states.is_empty() {
        states.insert(
            "pillar_axis".to_string(),
            BedrockBlockStateValue::String("y".to_string()),
        );
    }

    BedrockBlock {
        name: format!("minecraft:{bedrock_name}"),
        states,
    }
}

/// Convert Java door block to Bedrock format with upper_block_bit.
///
/// Java doors use `half=upper/lower`, Bedrock uses `upper_block_bit` (bool).
/// Also maps door names: `oak_door` → `wooden_door`, others keep their names.
fn convert_door(
    java_name: &str,
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    let bedrock_name = match java_name {
        "oak_door" => "wooden_door",
        _ => java_name, // spruce_door, dark_oak_door, etc. keep their name
    };

    let mut states = BTreeMap::new();

    if let Some(props) = props {
        // Convert half: Java "upper"/"lower" → Bedrock upper_block_bit true/false
        if let Some(fastnbt::Value::String(half)) = props.get("half") {
            let is_upper = half == "upper";
            states.insert(
                "upper_block_bit".to_string(),
                BedrockBlockStateValue::Bool(is_upper),
            );
        }

        // Convert facing if present.
        //
        // Bedrock's door `direction` uses the legacy cardinal order
        // 0=south, 1=west, 2=north, 3=east, but doors additionally store the
        // facing rotated 90° clockwise — a Bedrock door recorded as "east" is a
        // door that faces north. The two effects compose into the table below,
        // which is why it looks off-by-one next to convert_bed's:
        //   north → east  → 3      east → south → 0
        //   south → west  → 1      west → north → 2
        if let Some(fastnbt::Value::String(facing)) = props.get("facing") {
            let direction = match facing.as_str() {
                "east" => 0,
                "south" => 1,
                "west" => 2,
                "north" => 3,
                _ => 0,
            };
            states.insert(
                "direction".to_string(),
                BedrockBlockStateValue::Int(direction),
            );
        }

        // Convert hinge if present
        if let Some(fastnbt::Value::String(hinge)) = props.get("hinge") {
            let door_hinge = hinge == "right";
            states.insert(
                "door_hinge_bit".to_string(),
                BedrockBlockStateValue::Bool(door_hinge),
            );
        }

        // Convert open if present
        if let Some(fastnbt::Value::String(open)) = props.get("open") {
            let is_open = open == "true";
            states.insert(
                "open_bit".to_string(),
                BedrockBlockStateValue::Bool(is_open),
            );
        }
    }

    // Defaults if no properties were set
    if !states.contains_key("upper_block_bit") {
        states.insert(
            "upper_block_bit".to_string(),
            BedrockBlockStateValue::Bool(false),
        );
    }
    if !states.contains_key("direction") {
        states.insert("direction".to_string(), BedrockBlockStateValue::Int(0));
    }

    BedrockBlock {
        name: format!("minecraft:{bedrock_name}"),
        states,
    }
}

/// Convert Java trapdoor block to Bedrock format with facing/open/half states.
fn convert_trapdoor(
    java_name: &str,
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    // Map Java trapdoor names to Bedrock equivalents
    let bedrock_name = match java_name {
        "oak_trapdoor" => "trapdoor",
        "iron_trapdoor" => "iron_trapdoor",
        _ => java_name, // spruce_trapdoor, dark_oak_trapdoor, birch_trapdoor, etc.
    };

    let mut states = BTreeMap::new();

    if let Some(props) = props {
        // Convert facing: Java "north/south/east/west" → Bedrock "direction" (0-3)
        // Bedrock trapdoor direction: 0=east, 1=west, 2=south, 3=north
        // (same encoding as stairs weirdo_direction).
        if let Some(fastnbt::Value::String(facing)) = props.get("facing") {
            let direction = match facing.as_str() {
                "east" => 0,
                "west" => 1,
                "south" => 2,
                "north" => 3,
                _ => 0,
            };
            states.insert(
                "direction".to_string(),
                BedrockBlockStateValue::Int(direction),
            );
        }

        // Convert open: Java "true"/"false" → Bedrock open_bit
        if let Some(fastnbt::Value::String(open)) = props.get("open") {
            let is_open = open == "true";
            states.insert(
                "open_bit".to_string(),
                BedrockBlockStateValue::Bool(is_open),
            );
        }

        // Convert half: Java "top"/"bottom" → Bedrock upside_down_bit
        if let Some(fastnbt::Value::String(half)) = props.get("half") {
            let upside_down = half == "top";
            states.insert(
                "upside_down_bit".to_string(),
                BedrockBlockStateValue::Bool(upside_down),
            );
        }
    }

    // Defaults if no properties were set
    if !states.contains_key("direction") {
        states.insert("direction".to_string(), BedrockBlockStateValue::Int(0));
    }
    if !states.contains_key("open_bit") {
        states.insert("open_bit".to_string(), BedrockBlockStateValue::Bool(false));
    }
    if !states.contains_key("upside_down_bit") {
        states.insert(
            "upside_down_bit".to_string(),
            BedrockBlockStateValue::Bool(false),
        );
    }

    BedrockBlock {
        name: format!("minecraft:{bedrock_name}"),
        states,
    }
}

/// Convert Java bed block to Bedrock format with direction, head/foot, and occupied states.
fn convert_bed(
    _java_name: &str,
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    let mut states = BTreeMap::new();

    // Bedrock has a single "minecraft:bed" block and stores the dye colour in the
    // Bed block entity, NOT in the block state. Emitting a "color" state produced
    // a state set that resolves to no vanilla block at all.

    if let Some(props) = props {
        // Convert facing: Java "north/south/east/west" → Bedrock "direction" (0-3).
        // Beds use the legacy cardinal order 0=south, 1=west, 2=north, 3=east —
        // NOT the stairs/trapdoor "weirdo" order, and unlike doors they carry no
        // 90° rotation. Reusing the trapdoor table rotated most beds.
        if let Some(fastnbt::Value::String(facing)) = props.get("facing") {
            let direction = match facing.as_str() {
                "south" => 0,
                "west" => 1,
                "north" => 2,
                "east" => 3,
                _ => 0,
            };
            states.insert(
                "direction".to_string(),
                BedrockBlockStateValue::Int(direction),
            );
        }

        // Convert part: Java "head"/"foot" → Bedrock head_piece_bit
        if let Some(fastnbt::Value::String(part)) = props.get("part") {
            let is_head = part == "head";
            states.insert(
                "head_piece_bit".to_string(),
                BedrockBlockStateValue::Bool(is_head),
            );
        }

        // Convert occupied: Java "true"/"false" → Bedrock occupied_bit
        if let Some(fastnbt::Value::String(occupied)) = props.get("occupied") {
            let is_occupied = occupied == "true";
            states.insert(
                "occupied_bit".to_string(),
                BedrockBlockStateValue::Bool(is_occupied),
            );
        }
    }

    // Defaults if no properties were set
    if !states.contains_key("direction") {
        states.insert("direction".to_string(), BedrockBlockStateValue::Int(0));
    }
    if !states.contains_key("head_piece_bit") {
        states.insert(
            "head_piece_bit".to_string(),
            BedrockBlockStateValue::Bool(false),
        );
    }
    if !states.contains_key("occupied_bit") {
        states.insert(
            "occupied_bit".to_string(),
            BedrockBlockStateValue::Bool(false),
        );
    }

    BedrockBlock {
        name: "minecraft:bed".to_string(),
        states,
    }
}

/// Convert Java wall banner to Bedrock format.
///
/// Java stores facing as a string ("north"/"south"/"east"/"west") on the block state.
/// Bedrock uses `facing_direction` as an integer on `minecraft:wall_banner`:
///   2 = north, 3 = south, 4 = west, 5 = east
///
/// The banner color (light_gray = 7) and patterns live in the block entity, not here.
fn convert_wall_banner(
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    let facing_direction = props
        .and_then(|p| p.get("facing"))
        .and_then(|v| match v {
            fastnbt::Value::String(s) => Some(s.as_str()),
            _ => None,
        })
        .map(|f| match f {
            "north" => 2,
            "south" => 3,
            "west" => 4,
            "east" => 5,
            _ => 2, // default north
        })
        .unwrap_or(2);

    BedrockBlock::with_states(
        "wall_banner",
        vec![(
            "facing_direction",
            BedrockBlockStateValue::Int(facing_direction),
        )],
    )
}

/// Convert a Java wall torch to Bedrock's single `redstone_torch` block.
///
/// Bedrock encodes the mount as `torch_facing_direction`, but its horizontal
/// values are inverted relative to the direction the torch actually points
/// (MCPE-152036) — a torch pointing north is stored as "south". Copying the Java
/// `facing` name across unchanged mounts the torch on the opposite wall.
fn convert_redstone_wall_torch(
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    let facing = props
        .and_then(|p| p.get("facing"))
        .and_then(|v| match v {
            fastnbt::Value::String(s) => Some(s.as_str()),
            _ => None,
        })
        .unwrap_or("north");

    let bedrock_facing = match facing {
        "north" => "south",
        "south" => "north",
        "east" => "west",
        "west" => "east",
        _ => "south",
    };

    BedrockBlock::with_states(
        "redstone_torch",
        vec![(
            "torch_facing_direction",
            BedrockBlockStateValue::String(bedrock_facing.to_string()),
        )],
    )
}

/// Convert a Java crop, translating `age` (0-7) to Bedrock's `growth` (0-7).
///
/// Without this the growth stage is dropped and every field renders as freshly
/// sown seedlings instead of the ripe crop the generator asked for.
fn convert_crop(
    java_name: &str,
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    let growth = props
        .and_then(|p| p.get("age"))
        .and_then(parse_int_property)
        .unwrap_or(0)
        .clamp(0, 7);

    BedrockBlock::with_states(
        java_name,
        vec![("growth", BedrockBlockStateValue::Int(growth))],
    )
}

/// Convert Java's two-block plants to Bedrock's `double_plant`.
///
/// Java stores the half in `half=lower/upper`; Bedrock uses `upper_block_bit`.
/// Dropping it stacked two lower halves on top of each other.
fn convert_double_plant(
    java_name: &str,
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    let plant_type = if java_name == "large_fern" {
        "fern"
    } else {
        "grass"
    };

    BedrockBlock::with_states(
        "double_plant",
        vec![
            (
                "double_plant_type",
                BedrockBlockStateValue::String(plant_type.to_string()),
            ),
            (
                "upper_block_bit",
                BedrockBlockStateValue::Bool(is_upper_half(props)),
            ),
        ],
    )
}

/// Convert Java `tall_seagrass` to Bedrock's `seagrass`, which encodes both
/// halves of the plant in `sea_grass_type` rather than using a separate id.
fn convert_tall_seagrass(
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    let sea_grass_type = if is_upper_half(props) {
        "double_top"
    } else {
        "double_bot"
    };

    BedrockBlock::with_states(
        "seagrass",
        vec![(
            "sea_grass_type",
            BedrockBlockStateValue::String(sea_grass_type.to_string()),
        )],
    )
}

/// Convert a Java standing sign to Bedrock's `standing_sign`.
///
/// Java's `rotation` and Bedrock's `ground_sign_direction` share the same 0-15
/// scale and origin, so the value carries over unchanged. Hardcoding it made
/// every sign face the same way regardless of how it was placed.
fn convert_standing_sign(
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    let rotation = props
        .and_then(|p| p.get("rotation"))
        .and_then(parse_int_property)
        .unwrap_or(0)
        .rem_euclid(16);

    BedrockBlock::with_states(
        "standing_sign",
        vec![(
            "ground_sign_direction",
            BedrockBlockStateValue::Int(rotation),
        )],
    )
}

/// Convert a Java sea pickle, translating `pickles` (1-4) to Bedrock's
/// `cluster_count` (0-3, i.e. one less). Dropping it collapsed every cluster
/// down to a single pickle.
fn convert_sea_pickle(
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    let pickles = props
        .and_then(|p| p.get("pickles"))
        .and_then(parse_int_property)
        .unwrap_or(1);

    BedrockBlock::with_states(
        "sea_pickle",
        vec![
            (
                "cluster_count",
                BedrockBlockStateValue::Int((pickles - 1).clamp(0, 3)),
            ),
            // dead_bit marks a pickle that is NOT underwater; Arnis only places
            // them submerged.
            ("dead_bit", BedrockBlockStateValue::Bool(false)),
        ],
    )
}

/// Convert a Java chiseled bookshelf, translating `facing` to Bedrock's
/// `direction` (the legacy S-W-N-E order, same as beds).
fn convert_chiseled_bookshelf(
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    let direction = props
        .and_then(|p| p.get("facing"))
        .and_then(|v| match v {
            fastnbt::Value::String(s) => Some(s.as_str()),
            _ => None,
        })
        .map(|f| match f {
            "south" => 0,
            "west" => 1,
            "north" => 2,
            "east" => 3,
            _ => 0,
        })
        .unwrap_or(0);

    BedrockBlock::with_states(
        "chiseled_bookshelf",
        vec![
            ("direction", BedrockBlockStateValue::Int(direction)),
            ("books_stored", BedrockBlockStateValue::Int(0)),
        ],
    )
}

/// Java stores small integers in block properties as strings; accept the numeric
/// NBT tags too so stored properties round-trip either way.
fn parse_int_property(value: &fastnbt::Value) -> Option<i32> {
    match value {
        fastnbt::Value::String(s) => s.parse::<i32>().ok(),
        fastnbt::Value::Int(i) => Some(*i),
        fastnbt::Value::Byte(b) => Some(i32::from(*b)),
        fastnbt::Value::Short(v) => Some(i32::from(*v)),
        _ => None,
    }
}

/// True when a two-block plant's `half` property marks the upper block.
fn is_upper_half(props: Option<&std::collections::HashMap<String, fastnbt::Value>>) -> bool {
    matches!(
        props.and_then(|p| p.get("half")),
        Some(fastnbt::Value::String(h)) if h == "upper"
    )
}

/// Convert Java rail to Bedrock format with rail_direction from shape property.
///
/// Java uses `shape` strings ("north_south", "east_west", "ascending_east", etc.)
/// while Bedrock uses `rail_direction` integers (0–9).
fn convert_rail(props: Option<&std::collections::HashMap<String, fastnbt::Value>>) -> BedrockBlock {
    let direction = props
        .and_then(|p| p.get("shape"))
        .and_then(|v| match v {
            fastnbt::Value::String(s) => Some(s.as_str()),
            _ => None,
        })
        .map(|shape| match shape {
            "north_south" => 0,
            "east_west" => 1,
            "ascending_east" => 2,
            "ascending_west" => 3,
            "ascending_north" => 4,
            "ascending_south" => 5,
            "south_east" => 6,
            "south_west" => 7,
            "north_west" => 8,
            "north_east" => 9,
            _ => 0,
        })
        .unwrap_or(0);

    let mut states = BTreeMap::new();
    states.insert(
        "rail_direction".to_string(),
        BedrockBlockStateValue::Int(direction),
    );

    BedrockBlock {
        name: "minecraft:rail".to_string(),
        states,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_definitions::{AIR, GRASS_BLOCK, STONE};

    #[test]
    fn test_simple_blocks() {
        let bedrock = to_bedrock_block(STONE);
        assert_eq!(bedrock.name, "minecraft:stone");
        assert!(bedrock.states.is_empty());

        let bedrock = to_bedrock_block(AIR);
        assert_eq!(bedrock.name, "minecraft:air");
    }

    #[test]
    fn test_grass_block() {
        let bedrock = to_bedrock_block(GRASS_BLOCK);
        assert_eq!(bedrock.name, "minecraft:grass_block");
    }

    #[test]
    fn test_colored_blocks() {
        use crate::block_definitions::WHITE_CONCRETE;
        let bedrock = to_bedrock_block(WHITE_CONCRETE);
        assert_eq!(bedrock.name, "minecraft:concrete");
        assert!(matches!(
            bedrock.states.get("color"),
            Some(BedrockBlockStateValue::String(s)) if s == "white"
        ));
    }

    #[test]
    fn test_gray_concrete_powder_bedrock_mapping() {
        use crate::block_definitions::GRAY_CONCRETE_POWDER;
        let bedrock = to_bedrock_block(GRAY_CONCRETE_POWDER);
        assert_eq!(bedrock.name, "minecraft:concretePowder");
        assert!(matches!(
            bedrock.states.get("color"),
            Some(BedrockBlockStateValue::String(s)) if s == "gray"
        ));
    }

    #[test]
    fn test_brown_concrete_powder_bedrock_mapping() {
        use crate::block_definitions::BROWN_CONCRETE_POWDER;
        let bedrock = to_bedrock_block(BROWN_CONCRETE_POWDER);
        assert_eq!(bedrock.name, "minecraft:concretePowder");
        assert!(matches!(
            bedrock.states.get("color"),
            Some(BedrockBlockStateValue::String(s)) if s == "brown"
        ));
    }

    #[test]
    fn test_cyan_terracotta_bedrock_mapping() {
        use crate::block_definitions::CYAN_TERRACOTTA;
        let bedrock = to_bedrock_block(CYAN_TERRACOTTA);
        assert_eq!(bedrock.name, "minecraft:stained_hardened_clay");
        assert!(matches!(
            bedrock.states.get("color"),
            Some(BedrockBlockStateValue::String(s)) if s == "cyan"
        ));
    }

    #[test]
    fn test_stairs_with_properties() {
        use crate::block_definitions::OAK_STAIRS;
        use std::collections::HashMap as StdHashMap;

        // Create Java properties for a south-facing stair
        let mut props = StdHashMap::new();
        props.insert(
            "facing".to_string(),
            fastnbt::Value::String("south".to_string()),
        );
        props.insert(
            "half".to_string(),
            fastnbt::Value::String("bottom".to_string()),
        );
        let java_props = fastnbt::Value::Compound(props);

        let bedrock = to_bedrock_block_with_properties(OAK_STAIRS, Some(&java_props));
        assert_eq!(bedrock.name, "minecraft:oak_stairs");

        // Check weirdo_direction is set correctly (south = 2)
        assert!(matches!(
            bedrock.states.get("weirdo_direction"),
            Some(BedrockBlockStateValue::Int(2))
        ));

        // Check upside_down_bit is false for bottom half
        assert!(matches!(
            bedrock.states.get("upside_down_bit"),
            Some(BedrockBlockStateValue::Bool(false))
        ));
    }

    #[test]
    fn test_stairs_upside_down() {
        use crate::block_definitions::STONE_BRICK_STAIRS;
        use std::collections::HashMap as StdHashMap;

        // Create Java properties for an upside-down north-facing stair
        let mut props = StdHashMap::new();
        props.insert(
            "facing".to_string(),
            fastnbt::Value::String("north".to_string()),
        );
        props.insert(
            "half".to_string(),
            fastnbt::Value::String("top".to_string()),
        );
        let java_props = fastnbt::Value::Compound(props);

        let bedrock = to_bedrock_block_with_properties(STONE_BRICK_STAIRS, Some(&java_props));

        // Check weirdo_direction is set correctly (north = 3)
        assert!(matches!(
            bedrock.states.get("weirdo_direction"),
            Some(BedrockBlockStateValue::Int(3))
        ));

        // Check upside_down_bit is true for top half
        assert!(matches!(
            bedrock.states.get("upside_down_bit"),
            Some(BedrockBlockStateValue::Bool(true))
        ));
    }

    #[test]
    fn test_bed_with_properties() {
        use crate::block_definitions::RED_BED_NORTH_HEAD;
        use std::collections::HashMap as StdHashMap;

        let mut props = StdHashMap::new();
        props.insert(
            "facing".to_string(),
            fastnbt::Value::String("north".to_string()),
        );
        props.insert(
            "part".to_string(),
            fastnbt::Value::String("head".to_string()),
        );
        props.insert(
            "occupied".to_string(),
            fastnbt::Value::String("false".to_string()),
        );
        let java_props = fastnbt::Value::Compound(props);

        let bedrock = to_bedrock_block_with_properties(RED_BED_NORTH_HEAD, Some(&java_props));
        assert_eq!(bedrock.name, "minecraft:bed");

        // Bedrock stores the bed colour in the block entity, not the block state.
        assert!(!bedrock.states.contains_key("color"));
        // Bedrock bed direction is the legacy S-W-N-E order: facing=north → 2
        assert!(matches!(
            bedrock.states.get("direction"),
            Some(BedrockBlockStateValue::Int(2))
        ));
        assert!(matches!(
            bedrock.states.get("head_piece_bit"),
            Some(BedrockBlockStateValue::Bool(true))
        ));
        assert!(matches!(
            bedrock.states.get("occupied_bit"),
            Some(BedrockBlockStateValue::Bool(false))
        ));
    }

    #[test]
    fn test_bed_defaults_without_properties() {
        use crate::block_definitions::RED_BED_SOUTH_FOOT;

        let bedrock = to_bedrock_block_with_properties(RED_BED_SOUTH_FOOT, None);
        assert_eq!(bedrock.name, "minecraft:bed");

        // Should use defaults from block.properties() (south facing, foot)
        assert!(!bedrock.states.contains_key("color"));
        // facing=south → direction=0
        assert!(matches!(
            bedrock.states.get("direction"),
            Some(BedrockBlockStateValue::Int(0))
        ));
        assert!(matches!(
            bedrock.states.get("head_piece_bit"),
            Some(BedrockBlockStateValue::Bool(false))
        ));
        assert!(matches!(
            bedrock.states.get("occupied_bit"),
            Some(BedrockBlockStateValue::Bool(false))
        ));
    }

    #[test]
    fn test_rail_shape_conversion() {
        use crate::block_definitions::RAIL;

        let cases = [
            ("north_south", 0),
            ("east_west", 1),
            ("ascending_east", 2),
            ("ascending_west", 3),
            ("ascending_north", 4),
            ("ascending_south", 5),
            ("south_east", 6),
            ("south_west", 7),
            ("north_west", 8),
            ("north_east", 9),
        ];

        for (shape, expected_direction) in cases {
            let mut props = std::collections::HashMap::new();
            props.insert(
                "shape".to_string(),
                fastnbt::Value::String(shape.to_string()),
            );
            let java_props = fastnbt::Value::Compound(props);

            let bedrock = to_bedrock_block_with_properties(RAIL, Some(&java_props));
            assert_eq!(bedrock.name, "minecraft:rail");
            assert!(
                matches!(
                    bedrock.states.get("rail_direction"),
                    Some(BedrockBlockStateValue::Int(d)) if *d == expected_direction
                ),
                "shape={shape}: expected rail_direction={expected_direction}, got {:?}",
                bedrock.states.get("rail_direction")
            );
        }
    }

    #[test]
    fn test_rail_default_without_properties() {
        use crate::block_definitions::RAIL;

        let bedrock = to_bedrock_block_with_properties(RAIL, None);
        assert_eq!(bedrock.name, "minecraft:rail");
        // RAIL (id=66) has no built-in properties, so falls back to
        // to_bedrock_block which hardcodes rail_direction=0
        assert!(matches!(
            bedrock.states.get("rail_direction"),
            Some(BedrockBlockStateValue::Int(0))
        ));
    }

    /// Java block names that have no Bedrock counterpart under the same name.
    /// Every one of these used to be emitted verbatim and resolved to nothing.
    #[test]
    fn test_renamed_bedrock_ids() {
        use crate::block_definitions::*;

        let cases: &[(Block, &str)] = &[
            (MAGMA_BLOCK, "minecraft:magma"),
            (WAXED_COPPER_BLOCK, "minecraft:waxed_copper"),
            (OAK_BUTTON, "minecraft:wooden_button"),
            (OAK_FENCE_GATE, "minecraft:fence_gate"),
            (SIGN, "minecraft:standing_sign"),
            (POWERED_RAIL, "minecraft:golden_rail"),
            (WATER_CAULDRON, "minecraft:cauldron"),
            (KELP, "minecraft:kelp"),
            (KELP_PLANT, "minecraft:kelp"),
            (SEAGRASS, "minecraft:seagrass"),
            (TALL_SEAGRASS_BOTTOM, "minecraft:seagrass"),
            (TALL_SEAGRASS_TOP, "minecraft:seagrass"),
            (REDSTONE_WALL_TORCH, "minecraft:redstone_torch"),
            (GRAY_WALL_BANNER, "minecraft:wall_banner"),
        ];

        for (block, expected) in cases {
            let bedrock = to_bedrock_block_with_properties(*block, None);
            assert_eq!(
                &bedrock.name,
                expected,
                "{} mapped to {}",
                block.name(),
                bedrock.name
            );
        }
    }

    /// Bedrock never renamed its two oldest stair ids, so they read as swapped
    /// next to Java: minecraft:stone_stairs is cobblestone, and there is no
    /// minecraft:cobblestone_stairs at all.
    #[test]
    fn test_stone_and_cobblestone_stairs_are_not_swapped() {
        use crate::block_definitions::{COBBLESTONE_STAIRS, STONE_STAIRS};

        assert_eq!(
            to_bedrock_block_with_properties(COBBLESTONE_STAIRS, None).name,
            "minecraft:stone_stairs"
        );
        assert_eq!(
            to_bedrock_block_with_properties(STONE_STAIRS, None).name,
            "minecraft:normal_stone_stairs"
        );
    }

    /// The flattened per-material slab ids only accept minecraft:vertical_half,
    /// so they must be routed through the legacy stone_block_slab families that
    /// still take top_slot_bit.
    #[test]
    fn test_stone_slabs_use_legacy_families() {
        use crate::block_definitions::SMOOTH_STONE_SLAB;

        let bedrock = to_bedrock_block_with_properties(SMOOTH_STONE_SLAB, None);
        assert_eq!(bedrock.name, "minecraft:stone_block_slab");
        assert!(matches!(
            bedrock.states.get("stone_slab_type"),
            Some(BedrockBlockStateValue::String(t)) if t == "smooth_stone"
        ));
        assert!(matches!(
            bedrock.states.get("top_slot_bit"),
            Some(BedrockBlockStateValue::Bool(false))
        ));

        // Java stone_slab is family 4 ("stone"), not family 1 ("smooth_stone").
        assert_eq!(
            stone_slab_family("stone_slab"),
            Some(("stone_block_slab4", "stone_slab_type_4", "stone"))
        );

        // Blocks newer than the slab families keep their own id.
        assert_eq!(
            convert_slab("polished_blackstone_slab", None).name,
            "minecraft:polished_blackstone_slab"
        );
    }

    /// Crops carry age=7 in block_definitions; Bedrock calls it "growth".
    #[test]
    fn test_crops_keep_their_growth_stage() {
        use crate::block_definitions::{CARROTS, POTATOES, WHEAT};

        for block in [WHEAT, CARROTS, POTATOES] {
            let bedrock = to_bedrock_block_with_properties(block, None);
            assert!(
                matches!(
                    bedrock.states.get("growth"),
                    Some(BedrockBlockStateValue::Int(7))
                ),
                "{} lost its growth stage: {:?}",
                block.name(),
                bedrock.states
            );
        }
    }

    /// Both halves of a two-block plant share one Bedrock id and differ only in
    /// upper_block_bit; dropping it stacked two lower halves.
    #[test]
    fn test_double_plants_keep_their_half() {
        use crate::block_definitions::{
            LARGE_FERN_LOWER, LARGE_FERN_UPPER, TALL_GRASS_BOTTOM, TALL_GRASS_TOP,
        };

        for (lower, upper, plant) in [
            (TALL_GRASS_BOTTOM, TALL_GRASS_TOP, "grass"),
            (LARGE_FERN_LOWER, LARGE_FERN_UPPER, "fern"),
        ] {
            for (block, is_upper) in [(lower, false), (upper, true)] {
                let bedrock = to_bedrock_block_with_properties(block, None);
                assert_eq!(bedrock.name, "minecraft:double_plant");
                assert!(matches!(
                    bedrock.states.get("double_plant_type"),
                    Some(BedrockBlockStateValue::String(t)) if t == plant
                ));
                assert!(
                    matches!(
                        bedrock.states.get("upper_block_bit"),
                        Some(BedrockBlockStateValue::Bool(b)) if *b == is_upper
                    ),
                    "wrong half for {plant} (upper={is_upper})"
                );
            }
        }
    }

    /// Bedrock's torch_facing_direction is inverted relative to the direction
    /// the torch points (MCPE-152036).
    #[test]
    fn test_redstone_wall_torch_facing_is_inverted() {
        use std::collections::HashMap as StdHashMap;

        for (java_facing, bedrock_facing) in [
            ("north", "south"),
            ("south", "north"),
            ("east", "west"),
            ("west", "east"),
        ] {
            let mut props = StdHashMap::new();
            props.insert(
                "facing".to_string(),
                fastnbt::Value::String(java_facing.to_string()),
            );

            let bedrock = convert_redstone_wall_torch(Some(&props));
            assert_eq!(bedrock.name, "minecraft:redstone_torch");
            assert!(
                matches!(
                    bedrock.states.get("torch_facing_direction"),
                    Some(BedrockBlockStateValue::String(f)) if f == bedrock_facing
                ),
                "wall torch facing={java_facing} should store {bedrock_facing}"
            );
        }
    }

    /// Doors are stored rotated 90 degrees clockwise; beds are not. Pinning both
    /// tables stops one from being "corrected" into the other.
    #[test]
    fn test_door_and_bed_direction_tables_differ() {
        use std::collections::HashMap as StdHashMap;

        let facing = |f: &str| {
            let mut props = StdHashMap::new();
            props.insert("facing".to_string(), fastnbt::Value::String(f.to_string()));
            props
        };

        // Door: a Java north-facing door is stored as Bedrock "east" (3).
        for (java_facing, expected) in [("east", 0), ("south", 1), ("west", 2), ("north", 3)] {
            let bedrock = convert_door("oak_door", Some(&facing(java_facing)));
            assert!(
                matches!(
                    bedrock.states.get("direction"),
                    Some(BedrockBlockStateValue::Int(d)) if *d == expected
                ),
                "door facing={java_facing} should be direction={expected}"
            );
        }

        // Bed: plain S-W-N-E, no rotation.
        for (java_facing, expected) in [("south", 0), ("west", 1), ("north", 2), ("east", 3)] {
            let bedrock = convert_bed("red_bed", Some(&facing(java_facing)));
            assert!(
                matches!(
                    bedrock.states.get("direction"),
                    Some(BedrockBlockStateValue::Int(d)) if *d == expected
                ),
                "bed facing={java_facing} should be direction={expected}"
            );
        }
    }

    /// Sea pickle cluster_count is one less than Java's pickle count.
    #[test]
    fn test_sea_pickle_cluster_count() {
        use crate::block_definitions::SEA_PICKLE;

        let bedrock = to_bedrock_block_with_properties(SEA_PICKLE, None);
        assert_eq!(bedrock.name, "minecraft:sea_pickle");
        assert!(matches!(
            bedrock.states.get("cluster_count"),
            Some(BedrockBlockStateValue::Int(1))
        ));
    }

    /// Standing signs must keep their Java rotation.
    #[test]
    fn test_standing_sign_keeps_rotation() {
        use crate::block_definitions::SIGN;

        let bedrock = to_bedrock_block_with_properties(SIGN, None);
        assert_eq!(bedrock.name, "minecraft:standing_sign");
        assert!(matches!(
            bedrock.states.get("ground_sign_direction"),
            Some(BedrockBlockStateValue::Int(6))
        ));
    }

    /// Java encodes a full block as `type=double` on the slab; Bedrock has a
    /// separate id per material. The bundled schematics (cars, boats, bridge
    /// segments, playgrounds, crane, starship) all contain double slabs.
    #[test]
    fn test_double_slabs_become_double_slab_ids() {
        use std::collections::HashMap as StdHashMap;

        let double = || {
            let mut props = StdHashMap::new();
            props.insert(
                "type".to_string(),
                fastnbt::Value::String("double".to_string()),
            );
            props.insert(
                "waterlogged".to_string(),
                fastnbt::Value::String("false".to_string()),
            );
            props
        };

        // Every combination that actually appears in assets/structures/*.schem.
        let cases = [
            ("stone_slab", "minecraft:double_stone_block_slab4"),
            ("cobblestone_slab", "minecraft:double_stone_block_slab"),
            ("stone_brick_slab", "minecraft:double_stone_block_slab"),
            ("quartz_slab", "minecraft:double_stone_block_slab"),
            ("smooth_stone_slab", "minecraft:double_stone_block_slab"),
            ("smooth_quartz_slab", "minecraft:double_stone_block_slab4"),
            (
                "polished_andesite_slab",
                "minecraft:double_stone_block_slab3",
            ),
            ("jungle_slab", "minecraft:double_wooden_slab"),
            ("dark_oak_slab", "minecraft:double_wooden_slab"),
            ("warped_slab", "minecraft:warped_double_slab"),
            (
                "polished_blackstone_slab",
                "minecraft:polished_blackstone_double_slab",
            ),
        ];

        for (java_name, expected) in cases {
            let bedrock = convert_slab(java_name, Some(&double()));
            assert_eq!(
                bedrock.name, expected,
                "{java_name}[type=double] should map to {expected}"
            );
        }

        // The material state still has to ride along on the legacy families.
        let bedrock = convert_slab("cobblestone_slab", Some(&double()));
        assert!(matches!(
            bedrock.states.get("stone_slab_type"),
            Some(BedrockBlockStateValue::String(t)) if t == "cobblestone"
        ));
        let bedrock = convert_slab("jungle_slab", Some(&double()));
        assert!(matches!(
            bedrock.states.get("wood_type"),
            Some(BedrockBlockStateValue::String(t)) if t == "jungle"
        ));

        // ...and a half slab must NOT pick up a double id.
        let mut half = StdHashMap::new();
        half.insert(
            "type".to_string(),
            fastnbt::Value::String("bottom".to_string()),
        );
        assert_eq!(
            convert_slab("warped_slab", Some(&half)).name,
            "minecraft:warped_slab"
        );
        assert_eq!(
            convert_slab("stone_slab", Some(&half)).name,
            "minecraft:stone_block_slab4"
        );
    }
}
