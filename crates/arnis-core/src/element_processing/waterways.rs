use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

pub fn generate_waterways(editor: &mut WorldEditor, element: &ProcessedWay) {
    if let Some(waterway_type) = element.tags.get("waterway") {
        let (mut waterway_width, waterway_depth) = get_waterway_dimensions(waterway_type);

        // Check for custom width in tags
        if let Some(width_str) = element.tags.get("width") {
            waterway_width = width_str.parse::<i32>().unwrap_or_else(|_| {
                width_str
                    .parse::<f32>()
                    .map(|f: f32| f as i32)
                    .unwrap_or(waterway_width)
            });
        }

        // Skip layers below the ground level
        if matches!(
            element.tags.get("layer").map(|s| s.as_str()),
            Some("-1") | Some("-2") | Some("-3")
        ) {
            return;
        }

        // Process consecutive node pairs to create waterways
        // Use windows(2) to avoid connecting last node back to first
        for nodes_pair in element.nodes.windows(2) {
            let prev_node = nodes_pair[0].xz();
            let current_node = nodes_pair[1].xz();

            // Draw a line between the current and previous node
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(
                prev_node.x,
                0,
                prev_node.z,
                current_node.x,
                0,
                current_node.z,
            );

            for (bx, _, bz) in bresenham_points {
                // Create water channel with proper depth and sloped banks
                create_water_channel(editor, bx, bz, waterway_width, waterway_depth);
            }
        }
    }
}

/// Determines width and depth based on waterway type
fn get_waterway_dimensions(waterway_type: &str) -> (i32, i32) {
    match waterway_type {
        "river" => (8, 3),    // Large rivers: 8 blocks wide, 3 blocks deep
        "canal" => (6, 2),    // Canals: 6 blocks wide, 2 blocks deep
        "stream" => (3, 2),   // Streams: 3 blocks wide, 2 blocks deep
        "fairway" => (12, 3), // Shipping fairways: 12 blocks wide, 3 blocks deep
        "flowline" => (2, 1), // Water flow lines: 2 blocks wide, 1 block deep
        "brook" => (2, 1),    // Small brooks: 2 blocks wide, 1 block deep
        "ditch" => (2, 1),    // Ditches: 2 blocks wide, 1 block deep
        "drain" => (1, 1),    // Drainage: 1 block wide, 1 block deep
        _ => (4, 2),          // Default: 4 blocks wide, 2 blocks deep
    }
}

/// Creates a water channel with proper depth and sloped banks
fn create_water_channel(
    editor: &mut WorldEditor,
    center_x: i32,
    center_z: i32,
    width: i32,
    depth: i32,
) {
    let half_width = width / 2;

    for x in (center_x - half_width - 1)..=(center_x + half_width + 1) {
        for z in (center_z - half_width - 1)..=(center_z + half_width + 1) {
            let dx = (x - center_x).abs();
            let dz = (z - center_z).abs();
            let distance_from_center = dx.max(dz);

            if distance_from_center <= half_width {
                // Main water channel
                for y in (1 - depth)..=0 {
                    editor.set_block(WATER, x, y, z, None, None);
                }

                // Place one layer of dirt below the water channel
                editor.set_block(DIRT, x, -depth, z, None, None);

                // Clear vegetation above the water
                editor.set_block(AIR, x, 1, z, Some(&[GRASS, WHEAT, CARROTS, POTATOES]), None);
            } else if distance_from_center == half_width + 1 && depth > 1 {
                // Create sloped banks (one block interval slopes)
                let slope_depth = (depth - 1).max(1);
                for y in (1 - slope_depth)..=0 {
                    if y == 0 {
                        // Surface level - place water or air
                        editor.set_block(WATER, x, y, z, None, None);
                    } else {
                        // Below surface - dig out for slope
                        editor.set_block(AIR, x, y, z, None, None);
                    }
                }

                // Place one layer of dirt below the sloped areas
                editor.set_block(DIRT, x, -slope_depth, z, None, None);

                // Clear vegetation above sloped areas
                editor.set_block(AIR, x, 1, z, Some(&[GRASS, WHEAT, CARROTS, POTATOES]), None);
            }
        }
    }
}
