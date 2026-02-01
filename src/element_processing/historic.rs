//! Processing of historic elements.
//!
//! This module handles historic OSM elements including:
//! - `historic=memorial` - Memorials, monuments, and commemorative structures

use crate::block_definitions::*;
use crate::deterministic_rng::element_rng;
use crate::osm_parser::ProcessedNode;
use crate::world_editor::WorldEditor;
use rand::Rng;

/// Generate historic structures from node elements
pub fn generate_historic(editor: &mut WorldEditor, node: &ProcessedNode) {
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

    if let Some(historic_type) = node.tags.get("historic") {
        match historic_type.as_str() {
            "memorial" => generate_memorial(editor, node),
            "monument" => generate_monument(editor, node),
            "wayside_cross" => generate_wayside_cross(editor, node),
            _ => {}
        }
    }
}

/// Generate a memorial structure
///
/// Memorials come in many forms. We determine the type from the `memorial` tag:
/// - plaque: Simple wall-mounted or standing plaque
/// - statue: A statue on a pedestal
/// - sculpture: Artistic sculpture
/// - stone/stolperstein: Memorial stone
/// - bench: Memorial bench (already handled by amenity=bench typically)
/// - cross: Memorial cross
/// - obelisk: Tall pointed pillar
/// - stele: Upright stone slab
/// - bust: Bust on a pedestal
/// - Default: A general monument/pillar
fn generate_memorial(editor: &mut WorldEditor, node: &ProcessedNode) {
    let x = node.x;
    let z = node.z;

    // Use deterministic RNG for consistent results
    let mut rng = element_rng(node.id);

    // Get memorial subtype
    let memorial_type = node
        .tags
        .get("memorial")
        .map(|s| s.as_str())
        .unwrap_or("yes");

    match memorial_type {
        "plaque" => {
            // Simple plaque on a small stand
            editor.set_block(STONE_BRICKS, x, 1, z, None, None);
            editor.set_block(STONE_BRICK_SLAB, x, 2, z, None, None);
        }
        "statue" | "sculpture" | "bust" => {
            // Statue on a pedestal
            editor.set_block(STONE_BRICKS, x, 1, z, None, None);
            editor.set_block(CHISELED_STONE_BRICKS, x, 2, z, None, None);

            // Use polished andesite for bronze/metal statue appearance
            let statue_block = if rng.gen_bool(0.5) {
                POLISHED_ANDESITE
            } else {
                POLISHED_DIORITE
            };
            editor.set_block(statue_block, x, 3, z, None, None);
            editor.set_block(statue_block, x, 4, z, None, None);
            editor.set_block(STONE_BRICK_WALL, x, 5, z, None, None);
        }
        "stone" | "stolperstein" => {
            // Simple memorial stone embedded in ground
            let stone_block = if memorial_type == "stolperstein" {
                GOLD_BLOCK // Stolpersteine are brass/gold colored
            } else {
                STONE
            };
            editor.set_block(stone_block, x, 0, z, None, None);
        }
        "cross" | "war_memorial" => {
            // Memorial cross
            generate_cross(editor, x, z, 5);
        }
        "obelisk" => {
            // Tall pointed pillar with fixed height
            // Base layer at Y=1
            for dx in -1..=1 {
                for dz in -1..=1 {
                    editor.set_block(STONE_BRICKS, x + dx, 1, z + dz, None, None);
                }
            }

            // Second base layer at Y=2
            for dx in -1..=1 {
                for dz in -1..=1 {
                    editor.set_block(STONE_BRICKS, x + dx, 2, z + dz, None, None);
                }
            }
            // Stone brick slabs on the 4 corners at Y=3 (on top of corner blocks)
            editor.set_block(STONE_BRICK_SLAB, x - 1, 3, z - 1, None, None);
            editor.set_block(STONE_BRICK_SLAB, x + 1, 3, z - 1, None, None);
            editor.set_block(STONE_BRICK_SLAB, x - 1, 3, z + 1, None, None);
            editor.set_block(STONE_BRICK_SLAB, x + 1, 3, z + 1, None, None);

            // Main shaft, fixed height of 4 blocks (Y=3 to Y=6)
            for y in 3..=6 {
                editor.set_block(SMOOTH_QUARTZ, x, y, z, None, None);
            }

            editor.set_block(STONE_BRICK_SLAB, x, 7, z, None, None);
        }
        "stele" => {
            // Upright stone slab
            // Base
            editor.set_block(STONE_BRICKS, x, 1, z, None, None);

            // Upright slab (using wall blocks for thin appearance)
            for y in 2..=4 {
                editor.set_block(STONE_BRICK_WALL, x, y, z, None, None);
            }
            editor.set_block(STONE_BRICK_SLAB, x, 5, z, None, None);
        }
        _ => {
            // Default: simple stone pillar monument
            editor.set_block(STONE_BRICKS, x, 2, z, None, None);
            editor.set_block(CHISELED_STONE_BRICKS, x, 3, z, None, None);
            editor.set_block(STONE_BRICK_SLAB, x, 4, z, None, None);
        }
    }
}

/// Generate a monument (larger than memorial)
fn generate_monument(editor: &mut WorldEditor, node: &ProcessedNode) {
    let x = node.x;
    let z = node.z;

    // Monuments are typically larger structures
    let height = node
        .tags
        .get("height")
        .and_then(|h| h.parse::<i32>().ok())
        .unwrap_or(10)
        .clamp(5, 20);

    // Large base platform
    for dx in -2..=2 {
        for dz in -2..=2 {
            editor.set_block(STONE_BRICKS, x + dx, 1, z + dz, None, None);
        }
    }
    for dx in -1..=1 {
        for dz in -1..=1 {
            editor.set_block(STONE_BRICKS, x + dx, 2, z + dz, None, None);
        }
    }

    // Main structure
    for y in 3..height {
        editor.set_block(POLISHED_ANDESITE, x, y, z, None, None);
    }

    // Decorative top
    editor.set_block(CHISELED_STONE_BRICKS, x, height, z, None, None);
}

/// Generate a wayside cross
fn generate_wayside_cross(editor: &mut WorldEditor, node: &ProcessedNode) {
    let x = node.x;
    let z = node.z;

    // Simple roadside cross
    generate_cross(editor, x, z, 4);
}

/// Helper function to generate a cross structure
fn generate_cross(editor: &mut WorldEditor, x: i32, z: i32, height: i32) {
    // Base
    editor.set_block(STONE_BRICKS, x, 1, z, None, None);

    // Vertical beam
    for y in 2..=height {
        editor.set_block(STONE_BRICK_WALL, x, y, z, None, None);
    }

    // Horizontal beam (cross arm) at 2/3 height, but at least one block below top
    let arm_y = (2 + (height * 2 / 3)).min(height - 1);
    editor.set_block(STONE_BRICK_WALL, x - 1, arm_y, z, None, None);
    editor.set_block(STONE_BRICK_WALL, x + 1, arm_y, z, None, None);
}
