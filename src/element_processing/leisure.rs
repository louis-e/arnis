use crate::world_editor::WorldEditor;
use crate::osm_parser::ProcessedElement;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::floodfill::flood_fill_area;
use crate::element_processing::tree::create_tree;
use rand::Rng;

pub fn generate_leisure(editor: &mut WorldEditor, element: &ProcessedElement, ground_level: i32) {
    if let Some(leisure_type) = element.tags.get("leisure") {
        let mut previous_node: Option<(i32, i32)> = None;
        let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
        let mut current_leisure: Vec<(i32, i32)> = vec![];

        // Determine block type based on leisure type
        let block_type: &once_cell::sync::Lazy<Block> = match leisure_type.as_str() {
            "park" => &GRASS_BLOCK,
            "playground" | "recreation_ground" | "pitch" => &GREEN_STAINED_HARDENED_CLAY,
            "garden" => &GRASS_BLOCK,
            "swimming_pool" => &WATER,
            _ => &GRASS_BLOCK,
        };

        // Process leisure area nodes
        for &node in &element.nodes {
            if let Some(prev) = previous_node {
                // Draw a line between the current and previous node
                let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(prev.0, ground_level, prev.1, node.0, ground_level, node.1);
                for (bx, _, bz) in bresenham_points {
                    editor.set_block(block_type, bx, ground_level, bz);
                }

                current_leisure.push((node.0, node.1));
                corner_addup.0 += node.0;
                corner_addup.1 += node.1;
                corner_addup.2 += 1;
            }
            previous_node = Some(node);
        }

        // Flood-fill the interior of the leisure area
        if corner_addup != (0, 0, 0) {
            let polygon_coords: Vec<(i32, i32)> = element.nodes.iter().copied().collect();
            let filled_area: Vec<(i32, i32)> = flood_fill_area(&polygon_coords, 2);

            for (x, z) in filled_area {
                editor.set_block(block_type, x, ground_level, z);

                // Add decorative elements for parks and gardens
                if matches!(leisure_type.as_str(), "park" | "garden") {
                    /*if editor.check_for_water(x, z) { // TODO
                        continue;
                    }*/
                    let mut rng: rand::prelude::ThreadRng = rand::thread_rng();
                    let random_choice: i32 = rng.gen_range(0..1000);

                    match random_choice {
                        0 => { // Benches
                            editor.set_block(&OAK_LOG, x, ground_level + 1, z);
                            editor.set_block(&OAK_LOG, x + 1, ground_level + 1, z);
                            editor.set_block(&OAK_LOG, x - 1, ground_level + 1, z);
                        }
                        1..=30 => { // Flowers
                            let flower_choice: &once_cell::sync::Lazy<Block> = match rng.gen_range(0..4) {
                                0 => &RED_FLOWER,
                                1 => &YELLOW_FLOWER,
                                2 => &BLUE_FLOWER,
                                _ => &WHITE_FLOWER,
                            };
                            editor.set_block(flower_choice, x, ground_level + 1, z);
                        }
                        31..=70 => { // Grass
                            editor.set_block(&GRASS, x, ground_level + 1, z);
                        }
                        71..=80 => { // Tree
                            create_tree(editor, x, ground_level + 1, z, rng.gen_range(1..=3));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}
