use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::cartesian::XZPoint;
use crate::element_processing::tree::Tree;
use crate::floodfill::flood_fill_area;
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;
use rand::Rng;

pub fn generate_natural(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    if let Some(natural_type) = element.tags().get("natural") {
        if natural_type == "tree" {
            if let ProcessedElement::Node(node) = element {
                let x: i32 = node.x;
                let z: i32 = node.z;

                Tree::create(editor, (x, 1, z));
            }
        } else {
            let mut previous_node: Option<(i32, i32)> = None;
            let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
            let mut current_natural: Vec<(i32, i32)> = vec![];

            // Determine block type based on natural tag
            let block_type: Block = match natural_type.as_str() {
                "scrub" | "grassland" | "wood" => GRASS_BLOCK,
                "beach" | "sand" | "dune" => SAND,
                "tree_row" => GRASS_BLOCK,
                "wetland" | "water" => WATER,
                "bare_rock" => STONE,
                "glacier" => PACKED_ICE,
                "mud" => MUD,
                _ => GRASS_BLOCK,
            };

            let ProcessedElement::Way(way) = element else {
                return;
            };

            // Process natural nodes to fill the area
            for node in &way.nodes {
                let x: i32 = node.x;
                let z: i32 = node.z;

                if let Some(prev) = previous_node {
                    // Generate the line of coordinates between the two nodes
                    let bresenham_points: Vec<(i32, i32, i32)> =
                        bresenham_line(prev.0, 0, prev.1, x, 0, z);
                    for (bx, _, bz) in bresenham_points {
                        editor.set_block(block_type, bx, 0, bz, None, None);
                    }

                    current_natural.push((x, z));
                    corner_addup = (corner_addup.0 + x, corner_addup.1 + z, corner_addup.2 + 1);
                }

                previous_node = Some((x, z));
            }

            // If there are natural nodes, flood-fill the area
            if corner_addup != (0, 0, 0) {
                let polygon_coords: Vec<(i32, i32)> = way
                    .nodes
                    .iter()
                    .map(|n: &crate::osm_parser::ProcessedNode| (n.x, n.z))
                    .collect();
                let filled_area: Vec<(i32, i32)> =
                    flood_fill_area(&polygon_coords, args.timeout.as_ref());

                let mut rng: rand::prelude::ThreadRng = rand::thread_rng();

                for (x, z) in filled_area {
                    editor.set_block(block_type, x, 0, z, None, None);
                    // Make custom layer instead of dirt
                    match natural_type.as_str() {
                        "beach" | "sand" | "dune" => {
                            editor.set_block(SAND, x, 1, z, None, None);
                            editor.set_block(STONE, x, 2, z, None, None);
                        }
                        "glacier" => {
                            editor.set_block(PACKED_ICE, x, 1, z, None, None);
                            editor.set_block(STONE, x, 2, z, None, None);
                        }
                        "bare_rock" => {
                            editor.set_block(STONE, x, 1, z, None, None);
                            editor.set_block(STONE, x, 2, z, None, None);
                        }
                        _ => {}
                    }

                    // Generate elements for "wood" and "tree_row"
                    if natural_type == "wood" || natural_type == "tree_row" {
                        if editor.check_for_block(x, 0, z, None, Some(&[WATER])) {
                            continue;
                        }

                        let random_choice: i32 = rng.gen_range(0..26);
                        if random_choice == 25 {
                            Tree::create(editor, (x, 1, z));
                        } else if random_choice == 2 {
                            let flower_block = match rng.gen_range(1..=4) {
                                1 => RED_FLOWER,
                                2 => BLUE_FLOWER,
                                3 => YELLOW_FLOWER,
                                _ => WHITE_FLOWER,
                            };
                            editor.set_block(flower_block, x, 1, z, None, None);
                        } else if random_choice <= 1 {
                            editor.set_block(GRASS, x, 1, z, None, None);
                        }
                    }
                }
            }
        }
    }
}
