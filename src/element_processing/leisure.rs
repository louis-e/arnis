use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::tree::create_tree;
use crate::floodfill::flood_fill_area;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;
use rand::Rng;

pub fn generate_leisure(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    ground_level: i32,
    args: &Args,
) {
    if let Some(leisure_type) = element.tags.get("leisure") {
        let mut previous_node: Option<(i32, i32)> = None;
        let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
        let mut current_leisure: Vec<(i32, i32)> = vec![];

        // Determine block type based on leisure type
        let block_type: Block = match leisure_type.as_str() {
            "park" => {
                if args.winter {
                    SNOW_BLOCK
                } else {
                    GRASS_BLOCK
                }
            }
            "playground" | "recreation_ground" | "pitch" => {
                if let Some(surface) = element.tags.get("surface") {
                    match surface.as_str() {
                        "clay" => TERRACOTTA,
                        "sand" => SAND,
                        "tartan" => RED_TERRACOTTA,
                        _ => GREEN_STAINED_HARDENED_CLAY,
                    }
                } else {
                    GREEN_STAINED_HARDENED_CLAY
                }
            }
            "garden" => {
                if args.winter {
                    SNOW_BLOCK
                } else {
                    GRASS_BLOCK
                }
            }
            "swimming_pool" => WATER,
            _ => {
                if args.winter {
                    SNOW_BLOCK
                } else {
                    GRASS_BLOCK
                }
            }
        };

        // Process leisure area nodes
        for node in &element.nodes {
            if let Some(prev) = previous_node {
                // Draw a line between the current and previous node
                let bresenham_points: Vec<(i32, i32, i32)> =
                    bresenham_line(prev.0, ground_level, prev.1, node.x, ground_level, node.z);
                for (bx, _, bz) in bresenham_points {
                    editor.set_block(
                        block_type,
                        bx,
                        ground_level,
                        bz,
                        Some(&[
                            GRASS_BLOCK,
                            STONE_BRICKS,
                            SMOOTH_STONE,
                            LIGHT_GRAY_CONCRETE,
                            COBBLESTONE,
                            GRAY_CONCRETE,
                        ]),
                        None,
                    );
                }

                current_leisure.push((node.x, node.z));
                corner_addup.0 += node.x;
                corner_addup.1 += node.z;
                corner_addup.2 += 1;
            }
            previous_node = Some((node.x, node.z));
        }

        // Flood-fill the interior of the leisure area
        if corner_addup != (0, 0, 0) {
            let polygon_coords: Vec<(i32, i32)> = element
                .nodes
                .iter()
                .map(|n: &crate::osm_parser::ProcessedNode| (n.x, n.z))
                .collect();
            let filled_area: Vec<(i32, i32)> =
                flood_fill_area(&polygon_coords, args.timeout.as_ref());

            for (x, z) in filled_area {
                editor.set_block(block_type, x, ground_level, z, Some(&[GRASS_BLOCK]), None);

                // Add decorative elements for parks and gardens
                if matches!(leisure_type.as_str(), "park" | "garden")
                    && editor.check_for_block(x, ground_level, z, Some(&[GRASS_BLOCK]), None)
                {
                    let mut rng: rand::prelude::ThreadRng = rand::thread_rng();
                    let random_choice: i32 = rng.gen_range(0..1000);

                    match random_choice {
                        0 => {
                            // Benches
                            editor.set_block(OAK_LOG, x, ground_level + 1, z, None, None);
                            editor.set_block(OAK_LOG, x + 1, ground_level + 1, z, None, None);
                            editor.set_block(OAK_LOG, x - 1, ground_level + 1, z, None, None);
                        }
                        1..=30 => {
                            // Flowers
                            let flower_choice = match rng.gen_range(0..4) {
                                0 => RED_FLOWER,
                                1 => YELLOW_FLOWER,
                                2 => BLUE_FLOWER,
                                _ => WHITE_FLOWER,
                            };
                            editor.set_block(flower_choice, x, ground_level + 1, z, None, None);
                        }
                        31..=70 => {
                            // Grass
                            editor.set_block(GRASS, x, ground_level + 1, z, None, None);
                        }
                        71..=80 => {
                            // Tree
                            create_tree(
                                editor,
                                x,
                                ground_level + 1,
                                z,
                                rng.gen_range(1..=3),
                                args.winter,
                            );
                        }
                        _ => {}
                    }
                }

                // Add playground or recreation ground features
                if matches!(leisure_type.as_str(), "playground" | "recreation_ground") {
                    let mut rng: rand::prelude::ThreadRng = rand::thread_rng();
                    let random_choice: i32 = rng.gen_range(0..5000);

                    match random_choice {
                        0..=10 => {
                            // Swing set
                            for y in 1..=4 {
                                editor.set_block(OAK_FENCE, x - 1, ground_level + y, z, None, None);
                                editor.set_block(OAK_FENCE, x + 1, ground_level + y, z, None, None);
                            }
                            editor.set_block(OAK_FENCE, x, ground_level + 4, z, None, None);
                            editor.set_block(STONE_BLOCK_SLAB, x, ground_level + 2, z, None, None);
                        }
                        11..=20 => {
                            // Slide
                            editor.set_block(OAK_SLAB, x, ground_level + 1, z, None, None);
                            editor.set_block(OAK_SLAB, x + 1, ground_level + 2, z, None, None);
                            editor.set_block(OAK_SLAB, x + 2, ground_level + 3, z, None, None);

                            editor.set_block(OAK_PLANKS, x + 2, ground_level + 2, z, None, None);
                            editor.set_block(OAK_PLANKS, x + 2, ground_level + 1, z, None, None);

                            editor.set_block(LADDER, x + 2, ground_level + 2, z - 1, None, None);
                            editor.set_block(LADDER, x + 2, ground_level + 1, z - 1, None, None);
                        }
                        21..=30 => {
                            // Sandpit
                            editor.fill_blocks(
                                SAND,
                                x - 3,
                                ground_level,
                                z - 3,
                                x + 3,
                                ground_level,
                                z + 3,
                                Some(&[GREEN_STAINED_HARDENED_CLAY]),
                                None,
                            );
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}
