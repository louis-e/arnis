use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

pub fn generate_waterways(editor: &mut WorldEditor, element: &ProcessedWay) {
    if let Some(waterway_type) = element.tags.get("waterway") {
        let mut waterway_width = get_waterway_width(waterway_type);

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

            // Compute flat water level for this segment (min of both endpoints)
            let seg_water_y = editor
                .get_water_level(prev_node.x, prev_node.z)
                .min(editor.get_water_level(current_node.x, current_node.z));

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
                create_water_channel(editor, bx, bz, waterway_width, seg_water_y);
            }
        }
    }
}

/// Determines channel width based on waterway type.
fn get_waterway_width(waterway_type: &str) -> i32 {
    match waterway_type {
        "river" => 8,
        "canal" => 6,
        "stream" => 3,
        "fairway" => 12,
        "flowline" => 2,
        "brook" => 2,
        "ditch" => 2,
        "drain" => 1,
        _ => 4,
    }
}

/// Creates a water channel at a flat water level with the given width.
/// Skips blocks where terrain is above the water surface (bank above waterline).
fn create_water_channel(
    editor: &mut WorldEditor,
    center_x: i32,
    center_z: i32,
    width: i32,
    flat_water_y: i32,
) {
    const BANK_TOLERANCE: i32 = 2;
    let half_width = width / 2;

    for x in (center_x - half_width - 1)..=(center_x + half_width + 1) {
        for z in (center_z - half_width - 1)..=(center_z + half_width + 1) {
            let dx = (x - center_x).abs();
            let dz = (z - center_z).abs();
            let distance_from_center = dx.max(dz);

            if distance_from_center <= half_width + 1 {
                let ground_y = editor.get_ground_level(x, z);
                // Only place water where terrain is at or below the water surface,
                // but allow small elevation steps to avoid gaps on gentle slopes.
                let water_y = if ground_y <= flat_water_y {
                    Some(flat_water_y)
                } else if ground_y <= flat_water_y + BANK_TOLERANCE
                    && !editor.block_exists_absolute(x, ground_y, z)
                {
                    Some(ground_y)
                } else {
                    None
                };

                if let Some(water_y) = water_y {
                    editor.set_block_absolute(WATER, x, water_y, z, None, None);

                    // Clear vegetation above the water
                    editor.set_block_absolute(
                        AIR,
                        x,
                        water_y + 1,
                        z,
                        Some(&[GRASS, WHEAT, CARROTS, POTATOES]),
                        None,
                    );
                }
            }
        }
    }
}
