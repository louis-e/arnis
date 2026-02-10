//! Processing of historic elements.
//!
//! This module handles historic OSM elements including:
//! - `historic=memorial` - Memorials, monuments, and commemorative structures

use crate::args::Args;
use crate::block_definitions::*;
use crate::deterministic_rng::element_rng;
use crate::floodfill_cache::FloodFillCache;
use crate::osm_parser::{ProcessedNode, ProcessedWay};
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
            let statue_block = if rng.random_bool(0.5) {
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
            editor.set_block(STONE_BRICKS, x, 1, z, None, None);
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

    // Horizontal beam (cross arm) at approximately 2/3 height, but at least 2 and at most height-1
    let arm_y = ((height * 2 + 2) / 3).clamp(2, height - 1);
    // Only place horizontal arms if height allows for them (height >= 3)
    if height >= 3 {
        editor.set_block(STONE_BRICK_WALL, x - 1, arm_y, z, None, None);
        editor.set_block(STONE_BRICK_WALL, x + 1, arm_y, z, None, None);
    }
}

// ============================================================================
// Pyramid Generation (tomb=pyramid)
// ============================================================================

/// Generates a solid sandstone pyramid from a way outline.
///
/// The pyramid is built by flood-filling the footprint at ground level,
/// then shrinking the filled area inward by one block per layer until
/// only a single apex block (or nothing) remains.
pub fn generate_pyramid(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
) {
    if element.nodes.len() < 3 {
        return;
    }

    // Get the footprint via flood fill
    let footprint: Vec<(i32, i32)> =
        flood_fill_cache.get_or_compute(element, args.timeout.as_ref());
    if footprint.is_empty() {
        return;
    }

    // Determine base Y from terrain or ground level
    // Use the MINIMUM ground level so the pyramid sits on the lowest point
    // and doesn't float in areas with elevation differences
    let base_y = if args.terrain {
        footprint
            .iter()
            .map(|&(x, z)| editor.get_ground_level(x, z))
            .min()
            .unwrap_or(args.ground_level)
    } else {
        args.ground_level
    };

    // Bounding box of the footprint
    let min_x = footprint.iter().map(|&(x, _)| x).min().unwrap();
    let max_x = footprint.iter().map(|&(x, _)| x).max().unwrap();
    let min_z = footprint.iter().map(|&(_, z)| z).min().unwrap();
    let max_z = footprint.iter().map(|&(_, z)| z).max().unwrap();

    let center_x = (min_x + max_x) as f64 / 2.0;
    let center_z = (min_z + max_z) as f64 / 2.0;

    // The pyramid height is half the shorter side of the bounding box (classic proportions)
    let width = (max_x - min_x + 1) as f64;
    let length = (max_z - min_z + 1) as f64;
    let half_base = width.min(length) / 2.0;
    // Height = half the shorter side (classic pyramid proportions).
    // Footprint is already in scaled Minecraft coordinates, so no extra scale factor needed.
    let pyramid_height = half_base.max(3.0) as i32;

    // Build the pyramid layer by layer.
    // For each layer, only place blocks whose Chebyshev distance from the
    // footprint centre is within the shrinking radius AND that were in the
    // original footprint.
    let mut last_placed_layer: Option<i32> = None;
    for layer in 0..pyramid_height {
        // The allowed radius shrinks linearly from half_base at layer 0 to 0
        let radius = half_base * (1.0 - layer as f64 / pyramid_height as f64);
        if radius < 0.0 {
            break;
        }

        let y = base_y + 1 + layer;
        let mut placed = false;

        for &(x, z) in &footprint {
            let dx = (x as f64 - center_x).abs();
            let dz = (z as f64 - center_z).abs();

            // Use Chebyshev distance (max of dx, dz) for a square-footprint pyramid
            if dx <= radius && dz <= radius {
                // Allow overwriting common terrain blocks so the pyramid is
                // solid even when it intersects higher ground.
                editor.set_block_absolute(
                    SANDSTONE,
                    x,
                    y,
                    z,
                    Some(&[
                        GRASS_BLOCK,
                        DIRT,
                        STONE,
                        SAND,
                        GRAVEL,
                        COARSE_DIRT,
                        PODZOL,
                        DIRT_PATH,
                        SANDSTONE,
                    ]),
                    None,
                );
                placed = true;
            }
        }

        if placed {
            last_placed_layer = Some(y);
        } else {
            break; // Nothing placed, we've reached the apex
        }
    }

    // Cap with smooth sandstone one block above the last placed layer
    if let Some(top_y) = last_placed_layer {
        editor.set_block_absolute(
            SMOOTH_SANDSTONE,
            center_x.round() as i32,
            top_y + 1,
            center_z.round() as i32,
            Some(&[
                GRASS_BLOCK,
                DIRT,
                STONE,
                SAND,
                GRAVEL,
                COARSE_DIRT,
                PODZOL,
                DIRT_PATH,
                SANDSTONE,
            ]),
            None,
        );
    }
}
