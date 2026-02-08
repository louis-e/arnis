//! Bedrock Block Mapping
//!
//! This module provides translation between the internal Block representation
//! and Bedrock Edition block format. Bedrock uses string identifiers with
//! state properties that differ slightly from Java Edition.

use crate::block_definitions::Block;
use std::collections::HashMap;

/// Represents a Bedrock block with its identifier and state properties.
#[derive(Debug, Clone)]
pub struct BedrockBlock {
    /// The Bedrock block identifier (e.g., "minecraft:stone")
    pub name: String,
    /// Block state properties as key-value pairs
    pub states: HashMap<String, BedrockBlockStateValue>,
}

/// Bedrock block state values can be strings, booleans, or integers.
#[derive(Debug, Clone)]
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
            states: HashMap::new(),
        }
    }

    /// Creates a block with state properties.
    pub fn with_states(name: &str, states: Vec<(&str, BedrockBlockStateValue)>) -> Self {
        let mut state_map = HashMap::new();
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

        // Oak leaves with persistence
        "oak_leaves" => BedrockBlock::with_states(
            "leaves",
            vec![
                (
                    "old_leaf_type",
                    BedrockBlockStateValue::String("oak".to_string()),
                ),
                ("persistent_bit", BedrockBlockStateValue::Bool(true)),
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
            ],
        ),

        // Stone slab (bottom half by default)
        "stone_slab" => BedrockBlock::with_states(
            "stone_block_slab",
            vec![
                (
                    "stone_slab_type",
                    BedrockBlockStateValue::String("smooth_stone".to_string()),
                ),
                ("top_slot_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),

        // Stone brick slab
        "stone_brick_slab" => BedrockBlock::with_states(
            "stone_block_slab",
            vec![
                (
                    "stone_slab_type",
                    BedrockBlockStateValue::String("stone_brick".to_string()),
                ),
                ("top_slot_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),

        // Oak slab
        "oak_slab" => BedrockBlock::with_states(
            "wooden_slab",
            vec![
                (
                    "wood_type",
                    BedrockBlockStateValue::String("oak".to_string()),
                ),
                ("top_slot_bit", BedrockBlockStateValue::Bool(false)),
            ],
        ),

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
            vec![("height", BedrockBlockStateValue::Int(0))],
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
        // Plain terracotta
        "terracotta" => BedrockBlock::simple("hardened_clay"),

        // Wool colors
        "white_wool" => BedrockBlock::with_states(
            "wool",
            vec![("color", BedrockBlockStateValue::String("white".to_string()))],
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
        "daylight_detector" => BedrockBlock::simple("daylight_detector"),

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

        // Oak items mapped to dark_oak in Bedrock (or generic equivalents)
        "oak_pressure_plate" => BedrockBlock::simple("wooden_pressure_plate"),
        "oak_door" => BedrockBlock::simple("wooden_door"),
        "spruce_door" => BedrockBlock::simple("spruce_door"),
        "dark_oak_door" => BedrockBlock::simple("dark_oak_door"),
        "oak_trapdoor" => BedrockBlock::simple("trapdoor"),

        // Bed (Bedrock uses single "bed" block with color state)
        "red_bed" => BedrockBlock::with_states(
            "bed",
            vec![("color", BedrockBlockStateValue::String("red".to_string()))],
        ),

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

    // Handle slabs with type property (top/bottom/double)
    if java_name.ends_with("_slab") {
        return convert_slab(java_name, props_map);
    }

    // Handle logs with axis property
    if java_name.ends_with("_log") || java_name.ends_with("_wood") {
        return convert_log(java_name, props_map);
    }

    // Handle doors with half property (upper/lower → upper_block_bit)
    if java_name.ends_with("_door") && java_name != "iron_door" {
        return convert_door(java_name, props_map);
    }

    // Fall back to basic conversion without properties
    to_bedrock_block(block)
}

/// Convert Java stair block to Bedrock format with proper orientation.
fn convert_stairs(
    java_name: &str,
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    // Map Java stair names to Bedrock equivalents
    let bedrock_name = match java_name {
        "end_stone_brick_stairs" => "end_brick_stairs",
        _ => java_name, // Most stairs have the same name
    };

    let mut states = HashMap::new();

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
    let mut states = HashMap::new();

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

/// Convert Java slab block to Bedrock format with proper type.
fn convert_slab(
    java_name: &str,
    props: Option<&std::collections::HashMap<String, fastnbt::Value>>,
) -> BedrockBlock {
    let mut states = HashMap::new();

    // Convert type: Java uses "top/bottom/double", Bedrock uses "top_slot_bit"
    if let Some(props) = props {
        if let Some(fastnbt::Value::String(slab_type)) = props.get("type") {
            let top_slot = slab_type == "top";
            states.insert(
                "top_slot_bit".to_string(),
                BedrockBlockStateValue::Bool(top_slot),
            );
            // Note: "double" slabs in Java become full blocks in Bedrock (different block ID)
        }
    }

    // Default to bottom if not specified
    if !states.contains_key("top_slot_bit") {
        states.insert(
            "top_slot_bit".to_string(),
            BedrockBlockStateValue::Bool(false),
        );
    }

    // Handle special slab name mappings (same as in to_bedrock_block)
    let bedrock_name = match java_name {
        "stone_slab" => "stone_block_slab",
        "stone_brick_slab" => "stone_block_slab",
        "oak_slab" => "wooden_slab",
        "spruce_slab" => "wooden_slab",
        "birch_slab" => "wooden_slab",
        "jungle_slab" => "wooden_slab",
        "acacia_slab" => "wooden_slab",
        "dark_oak_slab" => "wooden_slab",
        _ => java_name,
    };

    // Add wood_type for wooden slabs
    if bedrock_name == "wooden_slab" {
        let wood_type = java_name.trim_end_matches("_slab");
        states.insert(
            "wood_type".to_string(),
            BedrockBlockStateValue::String(wood_type.to_string()),
        );
    }

    // Add stone_slab_type for stone slabs
    if bedrock_name == "stone_block_slab" {
        let slab_type = if java_name == "stone_brick_slab" {
            "stone_brick"
        } else {
            "stone"
        };
        states.insert(
            "stone_slab_type".to_string(),
            BedrockBlockStateValue::String(slab_type.to_string()),
        );
    }

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
    let mut states = HashMap::new();

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

    let mut states = HashMap::new();

    if let Some(props) = props {
        // Convert half: Java "upper"/"lower" → Bedrock upper_block_bit true/false
        if let Some(fastnbt::Value::String(half)) = props.get("half") {
            let is_upper = half == "upper";
            states.insert(
                "upper_block_bit".to_string(),
                BedrockBlockStateValue::Bool(is_upper),
            );
        }

        // Convert facing if present
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
        states.insert(
            "direction".to_string(),
            BedrockBlockStateValue::Int(0),
        );
    }

    BedrockBlock {
        name: format!("minecraft:{bedrock_name}"),
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
}
