use crate::world_editor::WorldEditor;
use crate::osm_parser::ProcessedElement;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;

pub fn generate_highways(editor: &mut WorldEditor, element: &ProcessedElement, ground_level: i32) {
    if let Some(highway_type) = element.tags.get("highway") {
        if highway_type == "street_lamp" {
            // Handle street lamps separately
            if let Some(first_node) = element.nodes.first() {
                let (x, z) = *first_node;
                for y in 1..=4 {
                    editor.set_block(&OAK_FENCE, x, ground_level + y, z);
                }
                editor.set_block(&GLOWSTONE, x, ground_level + 5, z);
            }
        } else {
            let mut previous_node: Option<(i32, i32)> = None;
            let mut block_type: &once_cell::sync::Lazy<Block> = &BLACK_CONCRETE;
            let mut block_range: i32 = 2;

            // Determine block type and range based on highway type
            match highway_type.as_str() {
                "footway" => {
                    block_type = &GRAY_CONCRETE;
                    block_range = 1;
                }
                "pedestrian" => {
                    block_type = &GRAY_CONCRETE;
                    block_range = 1;
                }
                "path" => {
                    block_type = &LIGHT_GRAY_CONCRETE;
                    block_range = 1;
                }
                "motorway" => {
                    block_range = 5;
                }
                "track" => {
                    block_range = 1;
                }
                _ => {
                    if let Some(lanes) = element.tags.get("lanes") {
                        if lanes != "1" && lanes != "2" {
                            block_range = 4;
                        }
                    }
                }
            }

            // Iterate over nodes to create the highway
            for &node in &element.nodes {
                if let Some(prev) = previous_node {
                    let (x1, z1) = prev;
                    let (x2, z2) = node;

                    // Generate the line of coordinates between the two nodes
                    let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(x1, ground_level, z1, x2, ground_level, z2);

                    for (x, _, z) in bresenham_points {
                        for dx in -block_range..=block_range {
                            for dz in -block_range..=block_range {
                                let set_x = x + dx;
                                let set_z = z + dz;

                                if highway_type == "footway" && element.tags.get("footway") == Some(&"crossing".to_string()) {
                                    let is_horizontal = (x2 - x1).abs() >= (z2 - z1).abs();
                                    if is_horizontal {
                                        if set_x % 2 < 1 {
                                            editor.set_block(&WHITE_CONCRETE, set_x, ground_level, set_z);
                                        } else {
                                            editor.set_block(&BLACK_CONCRETE, set_x, ground_level, set_z);
                                        }
                                    } else {
                                        if set_z % 2 < 1 {
                                            editor.set_block(&WHITE_CONCRETE, set_x, ground_level, set_z);
                                        } else {
                                            editor.set_block(&BLACK_CONCRETE, set_x, ground_level, set_z);
                                        }
                                    }
                                } else if highway_type == "bridge" {
                                    let height = ground_level + 1 + ((z2 - z1).abs() / 16);
                                    editor.set_block(&LIGHT_GRAY_CONCRETE, set_x, height, set_z);
                                    editor.set_block(&COBBLESTONE_WALL, set_x, height + 1, set_z);
                                } else if highway_type == "steps" {
                                    let height = ground_level + ((z2 - z1).abs() / 16);
                                    editor.set_block(&STONE, set_x, height, set_z);
                                } else {
                                    editor.set_block(block_type, set_x, ground_level, set_z);
                                }
                            }
                        }
                    }
                }
                previous_node = Some(node);
            }
        }
    }
}
