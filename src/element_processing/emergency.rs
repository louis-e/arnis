//! Processing of emergency infrastructure elements.
//!
//! This module handles emergency-related OSM elements including:
//! - `emergency=fire_hydrant` - Fire hydrants

use crate::block_definitions::*;
use crate::osm_parser::ProcessedNode;
use crate::world_editor::WorldEditor;

/// Generate emergency infrastructure from node elements
pub fn generate_emergency(editor: &mut WorldEditor, node: &ProcessedNode) {
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

    if let Some(emergency_type) = node.tags.get("emergency") {
        match emergency_type.as_str() {
            "fire_hydrant" => generate_fire_hydrant(editor, node),
            _ => {}
        }
    }
}

/// Generate a fire hydrant
///
/// Creates a simple fire hydrant structure using brick wall with redstone block on top.
/// Skips underground, wall-mounted, and pond hydrant types.
fn generate_fire_hydrant(editor: &mut WorldEditor, node: &ProcessedNode) {
    let x = node.x;
    let z = node.z;

    // Get hydrant type - skip underground, wall, and pond types
    let hydrant_type = node
        .tags
        .get("fire_hydrant:type")
        .map(|s| s.as_str())
        .unwrap_or("pillar");

    // Skip non-visible hydrant types
    if matches!(hydrant_type, "underground" | "wall" | "pond") {
        return;
    }

    // Simple hydrant: brick wall with redstone block on top
    editor.set_block(BRICK_WALL, x, 1, z, None, None);
    editor.set_block(REDSTONE_BLOCK, x, 2, z, None, None);
}
