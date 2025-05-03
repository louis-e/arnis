use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZPoint;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

pub fn generate_waterways(editor: &mut WorldEditor, element: &ProcessedWay) {
    if let Some(_waterway_type) = element.tags.get("waterway") {
        let mut previous_node: Option<XZPoint> = None;
        let mut waterway_width: i32 = 4; // Default waterway width

        // Check for custom width in tags
        if let Some(width_str) = element.tags.get("width") {
            waterway_width = width_str.parse::<i32>().unwrap_or_else(|_| {
                width_str
                    .parse::<f32>()
                    .map(|f: f32| f as i32)
                    .unwrap_or(waterway_width)
            });
        }

        // Process nodes to create waterways
        for node in &element.nodes {
            let current_node = node.xz();

            if let Some(prev) = previous_node {
                // Skip layers below the ground level
                if !matches!(
                    element.tags.get("layer").map(|s| s.as_str()),
                    Some("-1") | Some("-2") | Some("-3")
                ) {
                    // Draw a line between the current and previous node
                    let bresenham_points: Vec<(i32, i32, i32)> =
                        bresenham_line(prev.x, 0, prev.z, current_node.x, 0, current_node.z);

                    for (bx, _, bz) in bresenham_points {
                        for x in (bx - waterway_width / 2)..=(bx + waterway_width / 2) {
                            for z in (bz - waterway_width / 2)..=(bz + waterway_width / 2) {
                                // Set water block at the ground level
                                editor.set_block(WATER, x, 0, z, None, None);
                                // Clear vegetation above the water
                                editor.set_block(
                                    AIR,
                                    x,
                                    1,
                                    z,
                                    Some(&[GRASS, WHEAT, CARROTS, POTATOES]),
                                    None,
                                );
                            }
                        }
                    }
                }
            }
            previous_node = Some(current_node);
        }
    }
}
