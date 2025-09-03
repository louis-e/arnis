use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::osm_parser::{ProcessedElement, ProcessedNode};
use crate::world_editor::WorldEditor;

pub fn generate_man_made(editor: &mut WorldEditor, element: &ProcessedElement, _args: &Args) {
    // Skip if 'layer' or 'level' is negative in the tags
    if let Some(layer) = element.tags().get("layer") {
        if layer.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(level) = element.tags().get("level") {
        if level.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(man_made_type) = element.tags().get("man_made") {
        match man_made_type.as_str() {
            "pier" => generate_pier(editor, element),
            "antenna" => generate_antenna(editor, element),
            "chimney" => generate_chimney(editor, element),
            "water_well" => generate_water_well(editor, element),
            "water_tower" => generate_water_tower(editor, element),
            "mast" => generate_antenna(editor, element),
            _ => {} // Unknown man_made type, ignore
        }
    }
}

/// Generate a pier structure with OAK_SLAB planks and OAK_LOG support pillars
fn generate_pier(editor: &mut WorldEditor, element: &ProcessedElement) {
    if let ProcessedElement::Way(way) = element {
        let nodes = &way.nodes;
        if nodes.len() < 2 {
            return;
        }

        // Extract pier dimensions from tags
        let pier_width = element
            .tags()
            .get("width")
            .and_then(|w| w.parse::<i32>().ok())
            .unwrap_or(3); // Default 3 blocks wide

        let pier_height = 1; // Pier deck height above ground
        let support_spacing = 4; // Support pillars every 4 blocks

        // Generate the pier walkway using bresenham line algorithm
        for i in 0..nodes.len() - 1 {
            let start_node = &nodes[i];
            let end_node = &nodes[i + 1];

            let line_points =
                bresenham_line(start_node.x, 0, start_node.z, end_node.x, 0, end_node.z);

            for (index, (center_x, _y, center_z)) in line_points.iter().enumerate() {
                // Create pier deck (3 blocks wide)
                let half_width = pier_width / 2;
                for x in (center_x - half_width)..=(center_x + half_width) {
                    for z in (center_z - half_width)..=(center_z + half_width) {
                        editor.set_block(OAK_SLAB, x, pier_height, z, None, None);
                    }
                }

                // Add support pillars every few blocks
                if index % support_spacing == 0 {
                    let half_width = pier_width / 2;

                    // Place support pillars at the edges of the pier
                    let support_positions = [
                        (center_x - half_width, center_z), // Left side
                        (center_x + half_width, center_z), // Right side
                    ];

                    for (pillar_x, pillar_z) in support_positions {
                        // Support pillars going down from pier level
                        editor.set_block(OAK_LOG, pillar_x, 0, *pillar_z, None, None);
                    }
                }
            }
        }
    }
}

/// Generate an antenna/radio tower
fn generate_antenna(editor: &mut WorldEditor, element: &ProcessedElement) {
    if let Some(first_node) = element.nodes().next() {
        let x = first_node.x;
        let z = first_node.z;

        // Extract antenna configuration from tags
        let height = match element.tags().get("height") {
            Some(h) => h.parse::<i32>().unwrap_or(20).min(40), // Max 40 blocks
            None => match element.tags().get("tower:type").map(|s| s.as_str()) {
                Some("communication") => 20,
                Some("cellular") => 15,
                _ => 20,
            },
        };

        // Build the main tower pole
        editor.set_block(IRON_BLOCK, x, 3, z, None, None);
        for y in 4..height {
            editor.set_block(IRON_BARS, x, y, z, None, None);
        }

        // Add structural supports every 7 blocks
        for y in (7..height).step_by(7) {
            editor.set_block(IRON_BLOCK, x, y, z, Some(&[IRON_BARS]), None);
            let support_positions = [(1, 0), (-1, 0), (0, 1), (0, -1)];
            for (dx, dz) in support_positions {
                editor.set_block(IRON_BLOCK, x + dx, y, z + dz, None, None);
            }
        }

        // Equipment housing at base
        editor.fill_blocks(
            GRAY_CONCRETE,
            x - 1,
            1,
            z - 1,
            x + 1,
            2,
            z + 1,
            Some(&[GRAY_CONCRETE]),
            None,
        );
    }
}

/// Generate a chimney structure
fn generate_chimney(editor: &mut WorldEditor, element: &ProcessedElement) {
    if let Some(first_node) = element.nodes().next() {
        let x = first_node.x;
        let z = first_node.z;
        let height = 25;

        // Build 3x3 brick chimney with hole in the middle
        for y in 0..height {
            for dx in -1..=1 {
                for dz in -1..=1 {
                    // Skip center block to create hole
                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    editor.set_block(BRICK, x + dx, y, z + dz, None, None);
                }
            }
        }
    }
}

/// Generate a water well structure
fn generate_water_well(editor: &mut WorldEditor, element: &ProcessedElement) {
    if let Some(first_node) = element.nodes().next() {
        let x = first_node.x;
        let z = first_node.z;

        // Build stone well structure (3x3 base with water in center)
        for dx in -1..=1 {
            for dz in -1..=1 {
                if dx == 0 && dz == 0 {
                    // Water in the center
                    editor.set_block(WATER, x, -1, z, None, None);
                    editor.set_block(WATER, x, 0, z, None, None);
                } else {
                    // Stone well walls
                    editor.set_block(STONE_BRICKS, x + dx, 0, z + dz, None, None);
                    editor.set_block(STONE_BRICKS, x + dx, 1, z + dz, None, None);
                }
            }
        }

        // Add wooden well frame structure
        editor.fill_blocks(OAK_LOG, x - 2, 1, z, x - 2, 4, z, None, None);
        editor.fill_blocks(OAK_LOG, x + 2, 1, z, x + 2, 4, z, None, None);

        // Crossbeam with pulley system
        editor.set_block(OAK_SLAB, x - 1, 5, z, None, None);
        editor.set_block(OAK_FENCE, x, 4, z, None, None);
        editor.set_block(OAK_SLAB, x, 5, z, None, None);
        editor.set_block(OAK_SLAB, x + 1, 5, z, None, None);

        // Bucket hanging from center
        editor.set_block(IRON_BLOCK, x, 3, z, None, None);
    }
}

/// Generate a water tower structure
fn generate_water_tower(editor: &mut WorldEditor, element: &ProcessedElement) {
    if let Some(first_node) = element.nodes().next() {
        let x = first_node.x;
        let z = first_node.z;
        let tower_height = 20;
        let tank_height = 6;

        // Build support legs (4 corner pillars)
        let leg_positions = [(-2, -2), (2, -2), (-2, 2), (2, 2)];
        for (dx, dz) in leg_positions {
            for y in 0..tower_height {
                editor.set_block(IRON_BLOCK, x + dx, y, z + dz, None, None);
            }
        }

        // Add cross-bracing every 5 blocks for stability
        for y in (5..tower_height).step_by(5) {
            // Horizontal bracing
            for dx in -1..=1 {
                editor.set_block(SMOOTH_STONE, x + dx, y, z - 2, None, None);
                editor.set_block(SMOOTH_STONE, x + dx, y, z + 2, None, None);
            }
            for dz in -1..=1 {
                editor.set_block(SMOOTH_STONE, x - 2, y, z + dz, None, None);
                editor.set_block(SMOOTH_STONE, x + 2, y, z + dz, None, None);
            }
        }

        // Build water tank at the top - simple rectangular tank
        editor.fill_blocks(
            POLISHED_ANDESITE,
            x - 3,
            tower_height,
            z - 3,
            x + 3,
            tower_height + tank_height,
            z + 3,
            None,
            None,
        );

        // Add polished andesite pipe going down from the tank
        for y in 0..tower_height {
            editor.set_block(POLISHED_ANDESITE, x, y, z, None, None);
        }
    }
}

/// Generate man_made structures for node elements
pub fn generate_man_made_nodes(editor: &mut WorldEditor, node: &ProcessedNode) {
    if let Some(man_made_type) = node.tags.get("man_made") {
        let element = ProcessedElement::Node(node.clone());

        match man_made_type.as_str() {
            "antenna" => generate_antenna(editor, &element),
            "chimney" => generate_chimney(editor, &element),
            "water_well" => generate_water_well(editor, &element),
            "water_tower" => generate_water_tower(editor, &element),
            "mast" => generate_antenna(editor, &element),
            _ => {} // Unknown man_made type, ignore
        }
    }
}
