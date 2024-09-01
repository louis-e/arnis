use crate::world_editor::WorldEditor;
use crate::osm_parser::ProcessedElement;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;

pub fn generate_bridges(editor: &mut WorldEditor, element: &ProcessedElement, ground_level: i32) {
    if let Some(_bridge_type) = element.tags.get("bridge") {
        let bridge_height: i32 = element
            .tags
            .get("layer")
            .and_then(|layer: &String| layer.parse::<i32>().ok())
            .unwrap_or(1); // Default height if not specified

        // Calculate the total length of the bridge
        let total_steps: usize = element
            .nodes
            .windows(2)
            .map(|nodes: &[(i32, i32)]| {
                let (x1, z1) = nodes[0];
                let (x2, z2) = nodes[1];
                bresenham_line(x1, ground_level, z1, x2, ground_level, z2).len()
            })
            .sum();

        let half_steps = total_steps / 2; // Calculate midpoint for descending after rising
        let mut current_step = 0;

        for i in 1..element.nodes.len() {
            let (x1, z1) = element.nodes[i - 1];
            let (x2, z2) = element.nodes[i];

            // Generate the line of coordinates between the two nodes
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(x1, ground_level, z1, x2, ground_level, z2);

            for (bx, _, bz) in bresenham_points {
                // Calculate the current height of the bridge
                let current_height: i32 = if current_step <= half_steps {
                    ground_level + bridge_height + current_step as i32 / 5 // Rise for the first half
                } else {
                    ground_level
                        + bridge_height
                        + (half_steps as i32 / 5)
                        - ((current_step - half_steps) as i32 / 5) // Descend for the second half
                };

                // Set bridge blocks
                editor.set_block(&LIGHT_GRAY_CONCRETE, bx, current_height, bz);
                for (offset_x, offset_z) in &[(-1, -1), (1, -1), (1, 1), (-1, 1)] {
                    editor.set_block(
                        &LIGHT_GRAY_CONCRETE,
                        bx + offset_x,
                        current_height,
                        bz + offset_z,
                    );
                }

                current_step += 1;
            }
        }
    }
}
