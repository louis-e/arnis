use crate::args::Args;
use crate::block_definitions::BLOCKS;
use crate::bresenham::bresenham_line;
use crate::cartesian::XZPoint;
use crate::element_processing::tree::create_tree;
use crate::floodfill::flood_fill_area;
use crate::ground::Ground;
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::Rng;

pub fn generate_leisure(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    ground: &Ground,
    args: &Args,
) {
    if let Some(leisure_type) = element.tags.get("leisure") {
        let mut previous_node: Option<(i32, i32)> = None;
        let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
        let mut current_leisure: Vec<(i32, i32)> = vec![];

        // Determine block type based on leisure type
        let block_type = match leisure_type.as_str() {
            "park" => {
                if args.winter {
                    &*BLOCKS.by_name("snow_block").unwrap()
                } else {
                    &*BLOCKS.by_name("grass_block").unwrap()
                }
            }
            "playground" | "recreation_ground" | "pitch" => {
                if let Some(surface) = element.tags.get("surface") {
                    match surface.as_str() {
                        "clay" => &*BLOCKS.by_name("terracotta").unwrap(),
                        "sand" => &*BLOCKS.by_name("sand").unwrap(),
                        "tartan" => &*BLOCKS.by_name("red_terracotta").unwrap(),
                        _ => &*BLOCKS.by_name("green_stained_hardened_clay").unwrap(),
                    }
                } else {
                    &*BLOCKS.by_name("green_stained_hardened_clay").unwrap()
                }
            }
            "garden" => {
                if args.winter {
                    &*BLOCKS.by_name("snow_block").unwrap()
                } else {
                    &*BLOCKS.by_name("grass_block").unwrap()
                }
            }
            "swimming_pool" => &*BLOCKS.by_name("water").unwrap(),
            _ => {
                if args.winter {
                    &*BLOCKS.by_name("snow_block").unwrap()
                } else {
                    &*BLOCKS.by_name("grass_block").unwrap()
                }
            }
        };

        // Process leisure area nodes
        for node in &element.nodes {
            if let Some(prev) = previous_node {
                // Draw a line between the current and previous node
                let bresenham_points: Vec<(i32, i32, i32)> =
                    bresenham_line(prev.0, 0, prev.1, node.x, 0, node.z);
                for (bx, _, bz) in bresenham_points {
                    editor.set_block(
                        block_type,
                        bx,
                        ground.level(XZPoint::new(bx, bz)),
                        bz,
                        Some(&[
                            &*BLOCKS.by_name("grass_block").unwrap(),
                            &*BLOCKS.by_name("stone_bricks").unwrap(),
                            &*BLOCKS.by_name("smooth_stone").unwrap(),
                            &*BLOCKS.by_name("light_gray_concrete").unwrap(),
                            &*BLOCKS.by_name("cobblestone").unwrap(),
                            &*BLOCKS.by_name("gray_concrete").unwrap(),
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
                let ground_level = ground.level(XZPoint::new(x, z));
                editor.set_block(block_type, x, ground_level, z, Some(&[&*BLOCKS.by_name("grass_block").unwrap()]), None);

                // Add decorative elements for parks and gardens
                if matches!(leisure_type.as_str(), "park" | "garden")
                    && editor.check_for_block(x, ground_level, z, Some(&[&*BLOCKS.by_name("grass_block").unwrap()]), None)
                {
                    let mut rng: rand::prelude::ThreadRng = rand::thread_rng();
                    let random_choice: i32 = rng.gen_range(0..1000);

                    match random_choice {
                        0 => {
                            // Benches
                            editor.set_block(&*BLOCKS.by_name("oak_log").unwrap(), x, ground_level + 1, z, None, None);
                            editor.set_block(&*BLOCKS.by_name("oak_log").unwrap(), x + 1, ground_level + 1, z, None, None);
                            editor.set_block(&*BLOCKS.by_name("oak_log").unwrap(), x - 1, ground_level + 1, z, None, None);
                        }
                        1..=30 => {
                            // Flowers
                            let flower_choice = match rng.gen_range(0..4) {
                                0 => &*BLOCKS.by_name("red_flower").unwrap(),
                                1 => &*BLOCKS.by_name("yellow_flower").unwrap(),
                                2 => &*BLOCKS.by_name("blue_flower").unwrap(),
                                _ => &*BLOCKS.by_name("white_flower").unwrap(),
                            };
                            editor.set_block(flower_choice, x, ground_level + 1, z, None, None);
                        }
                        31..=70 => {
                            // Grass
                            editor.set_block(&*BLOCKS.by_name("grass").unwrap(), x, ground_level + 1, z, None, None);
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
                                editor.set_block(&*BLOCKS.by_name("oak_fence").unwrap(), x - 1, ground_level + y, z, None, None);
                                editor.set_block(&*BLOCKS.by_name("oak_fence").unwrap(), x + 1, ground_level + y, z, None, None);
                            }
                            editor.set_block(&*BLOCKS.by_name("oak_fence").unwrap(), x, ground_level + 4, z, None, None);
                            editor.set_block(&*BLOCKS.by_name("stone_block_slab").unwrap(), x, ground_level + 2, z, None, None);
                        }
                        11..=20 => {
                            // Slide
                            editor.set_block(&*BLOCKS.by_name("oak_slab").unwrap(), x, ground_level + 1, z, None, None);
                            editor.set_block(&*BLOCKS.by_name("oak_slab").unwrap(), x + 1, ground_level + 2, z, None, None);
                            editor.set_block(&*BLOCKS.by_name("oak_slab").unwrap(), x + 2, ground_level + 3, z, None, None);

                            editor.set_block(&*BLOCKS.by_name("oak_planks").unwrap(), x + 2, ground_level + 2, z, None, None);
                            editor.set_block(&*BLOCKS.by_name("oak_planks").unwrap(), x + 2, ground_level + 1, z, None, None);

                            editor.set_block(&*BLOCKS.by_name("ladder").unwrap(), x + 2, ground_level + 2, z - 1, None, None);
                            editor.set_block(&*BLOCKS.by_name("ladder").unwrap(), x + 2, ground_level + 1, z - 1, None, None);
                        }
                        21..=30 => {
                            // Sandpit
                            editor.fill_blocks(
                                &*BLOCKS.by_name("sand").unwrap(),
                                x - 3,
                                ground_level,
                                z - 3,
                                x + 3,
                                ground_level,
                                z + 3,
                                Some(&[&*BLOCKS.by_name("green_stained_hardened_clay").unwrap()]),
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

pub fn generate_leisure_from_relation(
    editor: &mut WorldEditor,
    rel: &ProcessedRelation,
    ground: &Ground,
    args: &Args,
) {
    if rel.tags.get("leisure") == Some(&"park".to_string()) {
        // First generate individual ways with their original tags
        for member in &rel.members {
            if member.role == ProcessedMemberRole::Outer {
                generate_leisure(editor, &member.way, ground, args);
            }
        }

        // Then combine all outer ways into one
        let mut combined_nodes = Vec::new();
        for member in &rel.members {
            if member.role == ProcessedMemberRole::Outer {
                combined_nodes.extend(member.way.nodes.clone());
            }
        }

        // Create combined way with relation tags
        let combined_way = ProcessedWay {
            id: rel.id,
            nodes: combined_nodes,
            tags: rel.tags.clone(),
        };

        // Generate leisure area from combined way
        generate_leisure(editor, &combined_way, ground, args);
    }
}
