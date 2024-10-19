use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;

pub fn generate_barriers(editor: &mut WorldEditor, element: &ProcessedElement, ground_level: i32) {
    if let Some(barrier_type) = element.tags().get("barrier") {
        if barrier_type == "bollard" {
            if let ProcessedElement::Node(node) = element {
                editor.set_block(
                    &COBBLESTONE_WALL,
                    node.x,
                    ground_level + 1,
                    node.z,
                    None,
                    None,
                );
                // Place bollard
            }
        } else if let ProcessedElement::Way(way) = element {
            // Determine wall height
            let wall_height: i32 = element
                .tags()
                .get("height")
                .and_then(|height: &String| height.parse::<f32>().ok())
                .map(|height: f32| f32::min(3.0, height).round() as i32)
                .unwrap_or(2); // Default height is 2 if not specified or invalid

            // Process nodes to create the barrier wall
            for i in 1..way.nodes.len() {
                let prev = &way.nodes[i - 1];
                let x1 = prev.x;
                let z1 = prev.z;

                let cur = &way.nodes[i];
                let x2 = cur.x;
                let z2 = cur.z;

                // Generate the line of coordinates between the two nodes
                let bresenham_points: Vec<(i32, i32, i32)> =
                    bresenham_line(x1, ground_level, z1, x2, ground_level, z2);

                for (bx, _, bz) in bresenham_points {
                    // Build the barrier wall to the specified height
                    for y in (ground_level + 1)..=(ground_level + wall_height) {
                        editor.set_block(&COBBLESTONE_WALL, bx, y, bz, None, None);
                        // Barrier wall
                    }

                    // Add an optional top to the barrier if the height is more than 1
                    if wall_height > 1 {
                        editor.set_block(
                            &STONE_BRICK_SLAB,
                            bx,
                            ground_level + wall_height + 1,
                            bz,
                            None,
                            None,
                        ); // Top of the barrier
                    }
                }
            }
        }
    }
}
