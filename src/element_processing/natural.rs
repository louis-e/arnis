use crate::world_editor::WorldEditor;
use crate::osm_parser::ProcessedElement;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::floodfill::flood_fill_area;
use crate::element_processing::tree::create_tree;
use rand::Rng;

pub fn generate_natural(editor: &mut WorldEditor, element: &ProcessedElement, ground_level: i32) {
    if let Some(natural_type) = element.tags.get("natural") {
        if natural_type == "tree" {
            if let Some(first_node) = element.nodes.first() {
                let (x, z) = *first_node;
                let mut rng: rand::prelude::ThreadRng = rand::thread_rng();
                create_tree(editor, x, ground_level + 1, z, rng.gen_range(1..=3));
            }
        } else {
            let mut previous_node: Option<(i32, i32)> = None;
            let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
            let mut current_natural: Vec<(i32, i32)> = vec![];

            // Determine block type based on natural tag
            let block_type: &once_cell::sync::Lazy<Block> = match natural_type.as_str() {
                "scrub" | "grassland" | "wood" => &GRASS_BLOCK,
                "beach" | "sand" => &SAND,
                "tree_row" => &GRASS_BLOCK,
                "wetland" | "water" => &WATER,
                _ => &GRASS_BLOCK,
            };

            // Process natural nodes to fill the area
            for &node in &element.nodes {
                let (x, z) = node;

                if let Some(prev) = previous_node {
                    // Generate the line of coordinates between the two nodes
                    let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(prev.0, ground_level, prev.1, x, ground_level, z);
                    for (bx, _, bz) in bresenham_points {
                        editor.set_block(block_type, bx, ground_level, bz, None, None);
                    }

                    current_natural.push((x, z));
                    corner_addup = (corner_addup.0 + x, corner_addup.1 + z, corner_addup.2 + 1);
                }

                previous_node = Some(node);
            }

            // If there are natural nodes, flood-fill the area
            if corner_addup != (0, 0, 0) {
                let polygon_coords: Vec<(i32, i32)> = element.nodes.iter().copied().collect();
                let filled_area: Vec<(i32, i32)> = flood_fill_area(&polygon_coords, 2);

                let mut rng: rand::prelude::ThreadRng = rand::thread_rng();

                for (x, z) in filled_area {
                    editor.set_block(block_type, x, ground_level, z, None, None);

                    // Generate elements for "wood" and "tree_row"
                    if natural_type == "wood" || natural_type == "tree_row" {
                        if check_for_water(x, z) {
                            continue;
                        }

                        let random_choice: i32 = rng.gen_range(0..26);
                        if random_choice == 25 {
                            create_tree(editor, x, ground_level + 1, z, rng.gen_range(1..=3));
                        } else if random_choice == 2 {
                            let flower_block = match rng.gen_range(1..=4) {
                                1 => &RED_FLOWER,
                                2 => &BLUE_FLOWER,
                                3 => &YELLOW_FLOWER,
                                _ => &WHITE_FLOWER,
                            };
                            editor.set_block(flower_block, x, ground_level + 1, z, None, None);
                        } else if random_choice <= 1 {
                            editor.set_block(&GRASS, x, ground_level + 1, z, None, None);
                        }
                    }
                }
            }
        }
    }
}

// Placeholder function for checking water presence
fn check_for_water(_x: i32, _z: i32) -> bool {
    false // Replace with your actual logic
}
