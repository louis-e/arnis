//! Processing of power infrastructure elements.
//!
//! This module handles power-related OSM elements including:
//! - `power=tower` - Large electricity pylons
//! - `power=pole` - Smaller wooden/concrete poles
//! - `power=line` - Power lines connecting towers/poles

use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;

/// Generate power infrastructure from way elements (power lines)
pub fn generate_power(editor: &mut WorldEditor, element: &ProcessedElement) {
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

    // Skip underground power infrastructure
    if element
        .tags()
        .get("location")
        .map(|v| v == "underground" || v == "underwater")
        .unwrap_or(false)
    {
        return;
    }
    if element
        .tags()
        .get("tunnel")
        .map(|v| v == "yes")
        .unwrap_or(false)
    {
        return;
    }

    if let Some(power_type) = element.tags().get("power") {
        match power_type.as_str() {
            "line" | "minor_line" => {
                if let ProcessedElement::Way(way) = element {
                    generate_power_line(editor, way);
                }
            }
            "tower" => generate_power_tower(editor, element),
            "pole" => generate_power_pole(editor, element),
            _ => {}
        }
    }
}

/// Generate power infrastructure from node elements
pub fn generate_power_nodes(editor: &mut WorldEditor, node: &ProcessedNode) {
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

    // Skip underground power infrastructure
    if node
        .tags
        .get("location")
        .map(|v| v == "underground" || v == "underwater")
        .unwrap_or(false)
    {
        return;
    }
    if node.tags.get("tunnel").map(|v| v == "yes").unwrap_or(false) {
        return;
    }

    if let Some(power_type) = node.tags.get("power") {
        let element = ProcessedElement::Node(node.clone());
        match power_type.as_str() {
            "tower" => generate_power_tower(editor, &element),
            "pole" => generate_power_pole(editor, &element),
            _ => {}
        }
    }
}

/// Generate a high-voltage transmission tower (pylon)
///
/// Creates a realistic lattice tower structure using iron bars and iron blocks.
/// The design is a tapered lattice tower with cross-bracing and insulators.
fn generate_power_tower(editor: &mut WorldEditor, element: &ProcessedElement) {
    let Some(first_node) = element.nodes().next() else {
        return;
    };

    let x = first_node.x;
    let z = first_node.z;

    // Extract height from tags, default to 25 blocks (represents ~25-40m real towers)
    let height = element
        .tags()
        .get("height")
        .and_then(|h| h.parse::<i32>().ok())
        .unwrap_or(25)
        .clamp(15, 40);

    // Tower design constants
    let base_width = 3; // Half-width at base (so 7x7 footprint)
    let top_width = 1; // Half-width at top (so 3x3)
    let arm_height = height - 4; // Height where arms extend
    let arm_length = 5; // How far arms extend horizontally

    // Build the four corner legs with tapering
    for y in 1..=height {
        // Calculate taper: legs get closer together as we go up
        let progress = y as f32 / height as f32;
        let current_width = base_width - ((base_width - top_width) as f32 * progress) as i32;

        // Four corner positions
        let corners = [
            (x - current_width, z - current_width),
            (x + current_width, z - current_width),
            (x - current_width, z + current_width),
            (x + current_width, z + current_width),
        ];

        for (cx, cz) in corners {
            editor.set_block(IRON_BLOCK, cx, y, cz, None, None);
        }

        // Add horizontal cross-bracing every 5 blocks
        if y % 5 == 0 && y < height - 2 {
            // Connect corners horizontally
            for dx in -current_width..=current_width {
                editor.set_block(IRON_BLOCK, x + dx, y, z - current_width, None, None);
                editor.set_block(IRON_BLOCK, x + dx, y, z + current_width, None, None);
            }
            for dz in -current_width..=current_width {
                editor.set_block(IRON_BLOCK, x - current_width, y, z + dz, None, None);
                editor.set_block(IRON_BLOCK, x + current_width, y, z + dz, None, None);
            }
        }

        // Add diagonal bracing between cross-brace levels
        if y % 5 >= 1 && y % 5 <= 4 && y > 1 && y < height - 2 {
            let prev_width = base_width
                - ((base_width - top_width) as f32 * ((y - 1) as f32 / height as f32)) as i32;

            // Only add center vertical support if the width changed
            if current_width != prev_width || y % 5 == 2 {
                editor.set_block(IRON_BARS, x, y, z, None, None);
            }
        }
    }

    // Create the cross-arms at arm_height for holding power lines
    // These extend outward in two directions (perpendicular to typical line direction)
    for arm_offset in [-arm_length, arm_length] {
        // Main arm beam (iron blocks for strength)
        for dx in 0..=arm_length {
            let arm_x = if arm_offset < 0 { x - dx } else { x + dx };
            editor.set_block(IRON_BLOCK, arm_x, arm_height, z, None, None);
            // Add second arm perpendicular
            editor.set_block(
                IRON_BLOCK,
                x,
                arm_height,
                z + if arm_offset < 0 { -dx } else { dx },
                None,
                None,
            );
        }

        // Insulators hanging from arm ends (end rods to simulate ceramic insulators)
        let end_x = if arm_offset < 0 {
            x - arm_length
        } else {
            x + arm_length
        };
        editor.set_block(END_ROD, end_x, arm_height - 1, z, None, None);
        editor.set_block(END_ROD, x, arm_height - 1, z + arm_offset, None, None);
    }

    // Add a second, smaller arm set lower for additional circuits
    let lower_arm_height = arm_height - 6;
    if lower_arm_height > 5 {
        let lower_arm_length = arm_length - 1;
        for arm_offset in [-lower_arm_length, lower_arm_length] {
            for dx in 0..=lower_arm_length {
                let arm_x = if arm_offset < 0 { x - dx } else { x + dx };
                editor.set_block(IRON_BLOCK, arm_x, lower_arm_height, z, None, None);
            }
            let end_x = if arm_offset < 0 {
                x - lower_arm_length
            } else {
                x + lower_arm_length
            };
            editor.set_block(END_ROD, end_x, lower_arm_height - 1, z, None, None);
        }
    }

    // Top finial/lightning rod
    editor.set_block(IRON_BLOCK, x, height, z, None, None);
    editor.set_block(LIGHTNING_ROD, x, height + 1, z, None, None);

    // Concrete foundation at base
    for dx in -3..=3 {
        for dz in -3..=3 {
            editor.set_block(GRAY_CONCRETE, x + dx, 0, z + dz, None, None);
        }
    }
}

/// Generate a wooden/concrete power pole
///
/// Creates a simpler single-pole structure for lower voltage distribution lines.
fn generate_power_pole(editor: &mut WorldEditor, element: &ProcessedElement) {
    let Some(first_node) = element.nodes().next() else {
        return;
    };

    let x = first_node.x;
    let z = first_node.z;

    // Extract height from tags, default to 10 blocks
    let height = element
        .tags()
        .get("height")
        .and_then(|h| h.parse::<i32>().ok())
        .unwrap_or(10)
        .clamp(6, 15);

    // Determine pole material from tags
    let pole_material = element
        .tags()
        .get("material")
        .map(|m| m.as_str())
        .unwrap_or("wood");

    let pole_block = match pole_material {
        "concrete" => LIGHT_GRAY_CONCRETE,
        "steel" | "metal" => IRON_BLOCK,
        _ => OAK_LOG, // Default to wood
    };

    // Build the main pole
    for y in 1..=height {
        editor.set_block(pole_block, x, y, z, None, None);
    }

    // Cross-arm at top (perpendicular beam for wires)
    let arm_length = 2;
    for dx in -arm_length..=arm_length {
        editor.set_block(OAK_FENCE, x + dx, height, z, None, None);
    }

    // Insulators at arm ends
    editor.set_block(END_ROD, x - arm_length, height + 1, z, None, None);
    editor.set_block(END_ROD, x + arm_length, height + 1, z, None, None);
    editor.set_block(END_ROD, x, height + 1, z, None, None); // Center insulator
}

/// Generate power lines connecting towers/poles
///
/// Creates a catenary-like curve (simplified) between nodes to simulate
/// the natural sag of power cables.
fn generate_power_line(editor: &mut WorldEditor, way: &ProcessedWay) {
    if way.nodes.len() < 2 {
        return;
    }

    // Determine line height based on voltage (higher voltage = taller structures)
    let base_height = way
        .tags
        .get("voltage")
        .and_then(|v| v.parse::<i32>().ok())
        .map(|voltage| {
            if voltage >= 220000 {
                22 // High voltage transmission
            } else if voltage >= 110000 {
                18
            } else if voltage >= 33000 {
                14
            } else {
                10 // Distribution lines
            }
        })
        .unwrap_or(15);

    // Process consecutive node pairs
    for i in 1..way.nodes.len() {
        let start = &way.nodes[i - 1];
        let end = &way.nodes[i];

        // Calculate distance between nodes
        let dx = (end.x - start.x) as f64;
        let dz = (end.z - start.z) as f64;
        let distance = (dx * dx + dz * dz).sqrt();

        // Calculate sag based on span length (longer spans = more sag)
        let max_sag = (distance / 15.0).clamp(1.0, 6.0) as i32;

        // Determine chain orientation based on line direction
        // If the line runs more along X-axis, use CHAIN_X; if more along Z-axis, use CHAIN_Z
        let chain_block = if dx.abs() >= dz.abs() {
            CHAIN_X // Line runs primarily along X-axis
        } else {
            CHAIN_Z // Line runs primarily along Z-axis
        };

        // Generate points along the line using Bresenham
        let line_points = bresenham_line(start.x, 0, start.z, end.x, 0, end.z);

        for (idx, (lx, _, lz)) in line_points.iter().enumerate() {
            // Calculate position along the span (0.0 to 1.0)
            let t = idx as f64 / line_points.len().max(1) as f64;

            // Catenary approximation: sag is maximum at center, zero at ends
            // Using parabola: sag = 4 * max_sag * t * (1 - t)
            let sag = (4.0 * max_sag as f64 * t * (1.0 - t)) as i32;

            let wire_y = base_height - sag;

            // Place the wire block (chain aligned with line direction)
            editor.set_block(chain_block, *lx, wire_y, *lz, None, None);

            // For high voltage lines, add parallel wires offset to sides
            if base_height >= 18 {
                // Three-phase power: 3 parallel lines
                // Offset perpendicular to the line direction
                if dx.abs() >= dz.abs() {
                    // Line runs along X, offset in Z
                    editor.set_block(chain_block, *lx, wire_y, *lz + 1, None, None);
                    editor.set_block(chain_block, *lx, wire_y, *lz - 1, None, None);
                } else {
                    // Line runs along Z, offset in X
                    editor.set_block(chain_block, *lx + 1, wire_y, *lz, None, None);
                    editor.set_block(chain_block, *lx - 1, wire_y, *lz, None, None);
                }
            }
        }
    }
}
