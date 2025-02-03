use crate::block_definitions::BLOCKS;
use crate::bresenham::bresenham_line;
use crate::cartesian::XZPoint;
use crate::ground::Ground;
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;

pub fn generate_barriers(editor: &mut WorldEditor, element: &ProcessedElement, ground: &Ground) {
    // Default values
    let mut barrier_material = &*BLOCKS.by_name("cobblestone_wall").unwrap();
    let mut barrier_height: i32 = 2;

    match element.tags().get("barrier").map(|s| s.as_str()) {
        Some("bollard") => {
            if let ProcessedElement::Node(node) = element {
                editor.set_block(
                    &*BLOCKS.by_name("cobblestone_wall").unwrap(),
                    node.x,
                    ground.level(node.xz()) + 1,
                    node.z,
                    None,
                    None,
                );
                // Place bollard
            }
            return;
        }
        Some("kerb") => {
            // Ignore kerbs
            return;
        }
        Some("hedge") => {
            barrier_material = &*BLOCKS.by_name("oak_leaves").unwrap();
            barrier_height = 2;
        }
        Some("fence") => {
            // Handle fence sub-types
            match element.tags().get("fence_type").map(|s| s.as_str()) {
                Some("railing" | "bars" | "krest") => {
                    barrier_material = &*BLOCKS.by_name("stone_brick_wall").unwrap();
                    barrier_height = 1;
                }
                Some("chain_link" | "metal" | "wire" | "barbed_wire" | "corrugated_metal") => {
                    barrier_material = &*BLOCKS.by_name("stone_brick_wall").unwrap();
                    barrier_height = 2;
                }
                Some("slatted" | "paling") => {
                    barrier_material = &*BLOCKS.by_name("oak_fence").unwrap();
                    barrier_height = 1;
                }
                Some("wood" | "split_rail" | "panel" | "pole") => {
                    barrier_material = &*BLOCKS.by_name("oak_fence").unwrap();
                    barrier_height = 2;
                }
                Some("concrete") => {
                    barrier_material = &*BLOCKS.by_name("andesite_wall").unwrap();
                    barrier_height = 2;
                }
                Some("glass") => {
                    barrier_material = &*BLOCKS.by_name("glass").unwrap();
                    barrier_height = 1;
                }
                _ => {}
            }
        }
        _ => {}
    }
    // Tagged material takes priority over inferred
    if let Some(barrier_mat) = element.tags().get("material") {
        if barrier_mat == "brick" {
            barrier_material = &*BLOCKS.by_name("brick").unwrap();
        }
    }

    if let ProcessedElement::Way(way) = element {
        // Determine wall height
        let wall_height: i32 = element
            .tags()
            .get("height")
            .and_then(|height: &String| height.parse::<f32>().ok())
            .map(|height: f32| f32::min(3.0, height).round() as i32)
            .unwrap_or(barrier_height);

        // Process nodes to create the barrier wall
        for i in 1..way.nodes.len() {
            let prev: &crate::osm_parser::ProcessedNode = &way.nodes[i - 1];
            let x1: i32 = prev.x;
            let z1: i32 = prev.z;

            let cur: &crate::osm_parser::ProcessedNode = &way.nodes[i];
            let x2: i32 = cur.x;
            let z2: i32 = cur.z;

            // Generate the line of coordinates between the two nodes
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(x1, 0, z1, x2, 0, z2);

            for (bx, _, bz) in bresenham_points {
                // Build the barrier wall to the specified height
                let ground_level = ground.level(XZPoint::new(bx, bz));
                for y in (ground_level + 1)..=(ground_level + wall_height) {
                    editor.set_block(barrier_material, bx, y, bz, None, None);
                    // Barrier wall
                }

                // Add an optional top to the barrier if the height is more than 1
                if wall_height > 1 {
                    editor.set_block(
                        &*BLOCKS.by_name("stone_brick_slab").unwrap(),
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
