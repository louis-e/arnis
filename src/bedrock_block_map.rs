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

        // Default: use the same name (works for many blocks)
        _ => BedrockBlock::simple(java_name),
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
}
