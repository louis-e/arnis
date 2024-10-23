use std::time::Duration;

use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::cartesian::XZPoint;
use crate::element_processing::tree::create_tree;
use crate::floodfill::flood_fill_area;
use crate::ground::Ground;
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;
use rand::Rng;

pub fn generate_natural(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    ground: &Ground,
    floodfill_timeout: Option<&Duration>,
) {
    if let Some(natural_type) = element.tags().get("natural") {
        if natural_type == "tree" {
            if let ProcessedElement::Node(node) = element {
                let x = node.x;
                let z = node.z;

                let mut rng: rand::prelude::ThreadRng = rand::thread_rng();
                create_tree(
                    editor,
                    x,
                    ground.level(node.xz()) + 1,
                    z,
                    rng.gen_range(1..=3),
                );
            }
        } else {
            let mut previous_node: Option<(i32, i32)> = None;
            let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
            let mut current_natural: Vec<(i32, i32)> = vec![];

            // Determine block type based on natural tag
            let block_type = match natural_type.as_str() {
                "scrub" | "grassland" | "wood" => GRASS_BLOCK,
                "beach" | "sand" => SAND,
                "tree_row" => GRASS_BLOCK,
                "wetland" | "water" => WATER,
                _ => GRASS_BLOCK,
            };

            let ProcessedElement::Way(way) = element else {
                return;
            };

            // Process natural nodes to fill the area
            for node in &way.nodes {
                let x = node.x;
                let z = node.z;

                if let Some(prev) = previous_node {
                    // Generate the line of coordinates between the two nodes
                    let bresenham_points: Vec<(i32, i32, i32)> =
                        bresenham_line(prev.0, 0, prev.1, x, 0, z);
                    for (bx, _, bz) in bresenham_points {
                        editor.set_block(
                            block_type,
                            bx,
                            ground.level(XZPoint::new(bx, bz)),
                            bz,
                            None,
                            None,
                        );
                    }

                    current_natural.push((x, z));
                    corner_addup = (corner_addup.0 + x, corner_addup.1 + z, corner_addup.2 + 1);
                }

                previous_node = Some((x, z));
            }

            // If there are natural nodes, flood-fill the area
            if corner_addup != (0, 0, 0) {
                let polygon_coords: Vec<(i32, i32)> =
                    way.nodes.iter().map(|n| (n.x, n.z)).collect();
                let filled_area: Vec<(i32, i32)> =
                    flood_fill_area(&polygon_coords, floodfill_timeout);

                let mut rng: rand::prelude::ThreadRng = rand::thread_rng();

                for (x, z) in filled_area {
                    let y = ground.level(XZPoint::new(x, z));

                    editor.set_block(block_type, x, y, z, None, None);

                    // Generate elements for "wood" and "tree_row"
                    if natural_type == "wood" || natural_type == "tree_row" {
                        if editor.check_for_block(x, y, z, None, Some(&[WATER])) {
                            continue;
                        }

                        let random_choice: i32 = rng.gen_range(0..26);
                        if random_choice == 25 {
                            create_tree(editor, x, y + 1, z, rng.gen_range(1..=3));
                        } else if random_choice == 2 {
                            let flower_block = match rng.gen_range(1..=4) {
                                1 => RED_FLOWER,
                                2 => BLUE_FLOWER,
                                3 => YELLOW_FLOWER,
                                _ => WHITE_FLOWER,
                            };
                            editor.set_block(flower_block, x, y + 1, z, None, None);
                        } else if random_choice <= 1 {
                            editor.set_block(GRASS, x, y + 1, z, None, None);
                        }
                    }
                }
            }
        }
    }
}
