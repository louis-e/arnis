use std::time::Duration;

use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::tree::create_tree;
use crate::floodfill::flood_fill_area;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;
use rand::Rng;

pub fn generate_landuse(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    ground_level: i32,
    floodfill_timeout: Option<&Duration>,
) {
    let mut previous_node: Option<(i32, i32)> = None;
    let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
    let mut current_landuse: Vec<(i32, i32)> = vec![];

    // Determine block type based on landuse tag
    let binding: String = "".to_string();
    let landuse_tag: &String = element.tags.get("landuse").unwrap_or(&binding);

    let block_type = match landuse_tag.as_str() {
        "greenfield" | "meadow" | "grass" => GRASS_BLOCK,
        "farmland" => FARMLAND,
        "forest" => GRASS_BLOCK,
        "cemetery" => PODZOL,
        "beach" => SAND,
        "construction" => DIRT,
        "traffic_island" => STONE_BLOCK_SLAB,
        "residential" => STONE_BRICKS,
        "commercial" => SMOOTH_STONE,
        "education" => LIGHT_GRAY_CONCRETE,
        "industrial" => COBBLESTONE,
        "military" => GRAY_CONCRETE,
        "railway" => GRAVEL,
        _ => GRASS_BLOCK,
    };

    // Process landuse nodes to fill the area
    for node in &element.nodes {
        let x: i32 = node.x;
        let z: i32 = node.z;

        if let Some(prev) = previous_node {
            // Generate the line of coordinates between the two nodes
            let bresenham_points: Vec<(i32, i32, i32)> =
                bresenham_line(prev.0, ground_level, prev.1, x, ground_level, z);
            for (bx, _, bz) in bresenham_points {
                editor.set_block(GRASS_BLOCK, bx, ground_level, bz, None, None);
            }

            current_landuse.push((x, z));
            corner_addup = (corner_addup.0 + x, corner_addup.1 + z, corner_addup.2 + 1);
        }

        previous_node = Some((x, z));
    }

    // If there are landuse nodes, flood-fill the area
    if !current_landuse.is_empty() {
        let polygon_coords: Vec<(i32, i32)> = element.nodes.iter().map(|n| (n.x, n.z)).collect();
        let floor_area: Vec<(i32, i32)> = flood_fill_area(&polygon_coords, floodfill_timeout);

        let mut rng: rand::prelude::ThreadRng = rand::thread_rng();

        for (x, z) in floor_area {
            if landuse_tag == "traffic_island" {
                editor.set_block(block_type, x, ground_level + 1, z, None, None);
            } else if landuse_tag == "construction" || landuse_tag == "railway" {
                editor.set_block(block_type, x, ground_level, z, None, Some(&[SPONGE]));
            } else {
                editor.set_block(block_type, x, ground_level, z, None, None);
            }

            // Add specific features for different landuse types
            match landuse_tag.as_str() {
                "cemetery" => {
                    if (x % 3 == 0) && (z % 3 == 0) {
                        let random_choice: i32 = rng.gen_range(0..100);
                        if random_choice < 15 {
                            // Place graves
                            if editor.check_for_block(x, ground_level, z, Some(&[PODZOL]), None) {
                                if rng.gen_bool(0.5) {
                                    editor.set_block(
                                        COBBLESTONE,
                                        x - 1,
                                        ground_level + 1,
                                        z,
                                        None,
                                        None,
                                    );
                                    editor.set_block(
                                        STONE_BRICK_SLAB,
                                        x - 1,
                                        ground_level + 2,
                                        z,
                                        None,
                                        None,
                                    );
                                    editor.set_block(
                                        STONE_BRICK_SLAB,
                                        x,
                                        ground_level + 1,
                                        z,
                                        None,
                                        None,
                                    );
                                    editor.set_block(
                                        STONE_BRICK_SLAB,
                                        x + 1,
                                        ground_level + 1,
                                        z,
                                        None,
                                        None,
                                    );
                                } else {
                                    editor.set_block(
                                        COBBLESTONE,
                                        x,
                                        ground_level + 1,
                                        z - 1,
                                        None,
                                        None,
                                    );
                                    editor.set_block(
                                        STONE_BRICK_SLAB,
                                        x,
                                        ground_level + 2,
                                        z - 1,
                                        None,
                                        None,
                                    );
                                    editor.set_block(
                                        STONE_BRICK_SLAB,
                                        x,
                                        ground_level + 1,
                                        z,
                                        None,
                                        None,
                                    );
                                    editor.set_block(
                                        STONE_BRICK_SLAB,
                                        x,
                                        ground_level + 1,
                                        z + 1,
                                        None,
                                        None,
                                    );
                                }
                            }
                        } else if random_choice < 30 {
                            if editor.check_for_block(x, ground_level, z, Some(&[PODZOL]), None) {
                                editor.set_block(RED_FLOWER, x, ground_level + 1, z, None, None);
                            }
                        } else if random_choice < 33 {
                            create_tree(editor, x, ground_level + 1, z, rng.gen_range(1..=3));
                        }
                    }
                }
                "forest" => {
                    if !editor.check_for_block(x, ground_level, z, None, Some(&[WATER])) {
                        let random_choice: i32 = rng.gen_range(0..21);
                        if random_choice == 20 {
                            create_tree(editor, x, ground_level + 1, z, rng.gen_range(1..=3));
                        } else if random_choice == 2 {
                            let flower_block: Block = match rng.gen_range(1..=4) {
                                1 => RED_FLOWER,
                                2 => BLUE_FLOWER,
                                3 => YELLOW_FLOWER,
                                _ => WHITE_FLOWER,
                            };
                            editor.set_block(flower_block, x, ground_level + 1, z, None, None);
                        } else if random_choice <= 1 {
                            editor.set_block(GRASS, x, ground_level + 1, z, None, None);
                        }
                    }
                }
                "farmland" => {
                    // Check if the current block is not water or another undesired block
                    if !editor.check_for_block(x, ground_level, z, None, Some(&[WATER])) {
                        if x % 15 == 0 || z % 15 == 0 {
                            // Place water on the edges
                            editor.set_block(WATER, x, ground_level, z, Some(&[FARMLAND]), None);
                            editor.set_block(
                                AIR,
                                x,
                                ground_level + 1,
                                z,
                                Some(&[GRASS, WHEAT, CARROTS, POTATOES]),
                                None,
                            );
                        } else {
                            // Set the block below as farmland
                            editor.set_block(FARMLAND, x, ground_level, z, None, None);

                            // If a random condition is met, place a special object
                            if rng.gen_range(0..76) == 0 {
                                let special_choice: i32 = rng.gen_range(1..=10);
                                if special_choice <= 2 {
                                    create_tree(
                                        editor,
                                        x,
                                        ground_level + 1,
                                        z,
                                        rng.gen_range(1..=3),
                                    );
                                } else if special_choice <= 6 {
                                    editor.set_block(HAY_BALE, x, ground_level + 1, z, None, None);
                                } else {
                                    editor.set_block(
                                        OAK_LEAVES,
                                        x,
                                        ground_level + 1,
                                        z,
                                        None,
                                        None,
                                    );
                                }
                            } else {
                                // Set crops only if the block below is farmland
                                if editor.check_for_block(
                                    x,
                                    ground_level,
                                    z,
                                    Some(&[FARMLAND]),
                                    None,
                                ) {
                                    let crop_choice =
                                        [WHEAT, CARROTS, POTATOES][rng.gen_range(0..3)];
                                    editor.set_block(
                                        crop_choice,
                                        x,
                                        ground_level + 1,
                                        z,
                                        None,
                                        None,
                                    );
                                }
                            }
                        }
                    }
                }
                "construction" => {
                    let random_choice: i32 = rng.gen_range(0..1501);
                    if random_choice < 6 {
                        editor.set_block(SCAFFOLDING, x, ground_level + 1, z, None, None);
                        if random_choice < 2 {
                            editor.set_block(SCAFFOLDING, x, ground_level + 2, z, None, None);
                            editor.set_block(SCAFFOLDING, x, ground_level + 3, z, None, None);
                        } else if random_choice < 4 {
                            editor.set_block(SCAFFOLDING, x, ground_level + 2, z, None, None);
                            editor.set_block(SCAFFOLDING, x, ground_level + 3, z, None, None);
                            editor.set_block(SCAFFOLDING, x, ground_level + 4, z, None, None);
                            editor.set_block(SCAFFOLDING, x, ground_level + 1, z + 1, None, None);
                        } else {
                            editor.set_block(SCAFFOLDING, x, ground_level + 2, z, None, None);
                            editor.set_block(SCAFFOLDING, x, ground_level + 3, z, None, None);
                            editor.set_block(SCAFFOLDING, x, ground_level + 4, z, None, None);
                            editor.set_block(SCAFFOLDING, x, ground_level + 5, z, None, None);
                            editor.set_block(SCAFFOLDING, x - 1, ground_level + 1, z, None, None);
                            editor.set_block(
                                SCAFFOLDING,
                                x + 1,
                                ground_level + 1,
                                z - 1,
                                None,
                                None,
                            );
                        }
                    } else if random_choice < 30 {
                        let construction_items: [Block; 11] = [
                            OAK_LOG,
                            COBBLESTONE,
                            GRAVEL,
                            GLOWSTONE,
                            STONE,
                            COBBLESTONE_WALL,
                            BLACK_CONCRETE,
                            SAND,
                            OAK_PLANKS,
                            DIRT,
                            BRICK,
                        ];
                        editor.set_block(
                            construction_items[rng.gen_range(0..construction_items.len())],
                            x,
                            ground_level + 1,
                            z,
                            None,
                            None,
                        );
                    } else if random_choice < 35 {
                        if random_choice < 30 {
                            editor.set_block(DIRT, x, ground_level + 1, z, None, None);
                            editor.set_block(DIRT, x, ground_level + 2, z, None, None);
                            editor.set_block(DIRT, x + 1, ground_level + 1, z, None, None);
                            editor.set_block(DIRT, x, ground_level + 1, z + 1, None, None);
                        } else {
                            editor.set_block(DIRT, x, ground_level + 1, z, None, None);
                            editor.set_block(DIRT, x, ground_level + 2, z, None, None);
                            editor.set_block(DIRT, x - 1, ground_level + 1, z, None, None);
                            editor.set_block(DIRT, x, ground_level + 1, z - 1, None, None);
                        }
                    } else if random_choice < 150 {
                        editor.set_block(AIR, x, ground_level, z, None, Some(&[SPONGE]));
                    }
                }
                "grass" => {
                    if rng.gen_range(1..=7) != 1
                        && !editor.check_for_block(x, ground_level, z, None, Some(&[WATER]))
                    {
                        editor.set_block(GRASS, x, ground_level + 1, z, None, None);
                    }
                }
                "meadow" => {
                    if !editor.check_for_block(x, ground_level, z, None, Some(&[WATER])) {
                        let random_choice: i32 = rng.gen_range(0..1001);
                        if random_choice < 5 {
                            create_tree(editor, x, ground_level + 1, z, rng.gen_range(1..=3));
                        } else if random_choice < 800 {
                            editor.set_block(GRASS, x, ground_level + 1, z, None, None);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
