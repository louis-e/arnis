//! Processing of advertising elements.
//!
//! This module handles advertising-related OSM elements including:
//! - `advertising=column` - Cylindrical advertising columns (Litfaßsäule)
//! - `advertising=flag` - Advertising flags on poles
//! - `advertising=poster_box` - Illuminated poster display boxes

use crate::block_definitions::*;
use crate::deterministic_rng::element_rng;
use crate::osm_parser::ProcessedNode;
use crate::world_editor::WorldEditor;
use rand::Rng;

/// Generate advertising structures from node elements
pub fn generate_advertising(editor: &mut WorldEditor, node: &ProcessedNode) {
    // Skip if 'layer' or 'level' is negative in the tags
    if let Some(layer) = node.tags.get("layer") {
        if layer.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(level) = node.tags.get("level") {
        if level.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(advertising_type) = node.tags.get("advertising") {
        match advertising_type.as_str() {
            "column" => generate_advertising_column(editor, node),
            "flag" => generate_advertising_flag(editor, node),
            "poster_box" => generate_poster_box(editor, node),
            _ => {}
        }
    }
}

/// Generate an advertising column (Litfaßsäule)
///
/// Creates a simple advertising column.
fn generate_advertising_column(editor: &mut WorldEditor, node: &ProcessedNode) {
    let x = node.x;
    let z = node.z;

    // Two green concrete blocks stacked
    editor.set_block(GREEN_CONCRETE, x, 1, z, None, None);
    editor.set_block(GREEN_CONCRETE, x, 2, z, None, None);

    // Stone brick slab on top
    editor.set_block(STONE_BRICK_SLAB, x, 3, z, None, None);
}

/// Generate an advertising flag
///
/// Creates a flagpole with a banner/flag for advertising.
fn generate_advertising_flag(editor: &mut WorldEditor, node: &ProcessedNode) {
    let x = node.x;
    let z = node.z;

    // Use deterministic RNG for flag color
    let mut rng = element_rng(node.id);

    // Get height from tags or default
    let height = node
        .tags
        .get("height")
        .and_then(|h| h.parse::<i32>().ok())
        .unwrap_or(6)
        .clamp(4, 12);

    // Flagpole
    for y in 1..=height {
        editor.set_block(IRON_BARS, x, y, z, None, None);
    }

    // Flag/banner at top (using colored wool)
    // Random bright advertising colors
    let flag_colors = [
        RED_WOOL,
        YELLOW_WOOL,
        BLUE_WOOL,
        GREEN_WOOL,
        ORANGE_WOOL,
        WHITE_WOOL,
    ];
    let flag_block = flag_colors[rng.random_range(0..flag_colors.len())];

    // Flag extends to one side (2-3 blocks)
    let flag_length = 3;
    for dx in 1..=flag_length {
        editor.set_block(flag_block, x + dx, height, z, None, None);
        editor.set_block(flag_block, x + dx, height - 1, z, None, None);
    }

    // Finial at top
    editor.set_block(IRON_BLOCK, x, height + 1, z, None, None);
}

/// Generate a poster box (city light / lollipop display)
///
/// Creates an illuminated poster display box on a pole.
fn generate_poster_box(editor: &mut WorldEditor, node: &ProcessedNode) {
    let x = node.x;
    let z = node.z;

    // Y=1: Two iron bars next to each other
    editor.set_block(IRON_BARS, x, 1, z, None, None);
    editor.set_block(IRON_BARS, x + 1, 1, z, None, None);

    // Y=2 and Y=3: Two sea lanterns
    editor.set_block(SEA_LANTERN, x, 2, z, None, None);
    editor.set_block(SEA_LANTERN, x + 1, 2, z, None, None);
    editor.set_block(SEA_LANTERN, x, 3, z, None, None);
    editor.set_block(SEA_LANTERN, x + 1, 3, z, None, None);

    // Y=4: Two polished stone brick slabs
    editor.set_block(STONE_BRICK_SLAB, x, 4, z, None, None);
    editor.set_block(STONE_BRICK_SLAB, x + 1, 4, z, None, None);
}
