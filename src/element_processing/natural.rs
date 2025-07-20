use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::tree::Tree;
use crate::floodfill::flood_fill_area;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedRelation, ProcessedWay};
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
            let binding: String = "".to_string();

            // Determine block type based on natural tag
            let block_type: Block = match natural_type.as_str() {
                "scrub" | "grassland" | "wood" | "heath" | "tree_row" => GRASS_BLOCK,
                "sand" | "dune" => SAND,
                "beach" | "shoal" => {
                    let surface = element.tags().get("natural").unwrap_or(&binding);
                    match surface.as_str() {
                        "gravel" => GRAVEL,
                        _ => SAND,
                    }
                }
                "water" | "reef" => WATER,
                "bare_rock" => STONE,
                "blockfield" => COBBLESTONE,
                "glacier" => PACKED_ICE,
                "mud" | "wetland" => MUD,
                "mountain_range" => COBBLESTONE,
                "saddle" | "ridge" => STONE,
                "shrubbery" | "tundra" | "hill" => GRASS_BLOCK,
                "cliff" => STONE,
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
                    // Generate custom layer instead of dirt, must be stone on the lowest level
                    match natural_type.as_str() {
                        "beach" | "sand" | "dune" | "shoal" => {
                            editor.set_block(SAND, x, 0, z, None, None);
                        }
                        "glacier" => {
                            editor.set_block(PACKED_ICE, x, 0, z, None, None);
                            editor.set_block(STONE, x, -1, z, None, None);
                        }
                        "bare_rock" => {
                            editor.set_block(STONE, x, 0, z, None, None);
                        }
                        _ => {}
                    }

                    // Generate surface elements
                    if editor.check_for_block(x, 0, z, Some(&[WATER])) {
                        continue;
                    }
                    match natural_type.as_str() {
                        "grassland" => {
                            if !editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                                continue;
                            }
                            if rng.gen_bool(0.6) {
                                editor.set_block(GRASS, x, 1, z, None, None);
                            }
                        }
                        "heath" => {
                            if !editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                                continue;
                            }
                            let random_choice = rng.gen_range(0..500);
                            if random_choice < 33 {
                                if random_choice <= 2 {
                                    editor.set_block(COBBLESTONE, x, 0, z, None, None);
                                } else if random_choice < 6 {
                                    editor.set_block(OAK_LEAVES, x, 1, z, None, None);
                                } else {
                                    editor.set_block(GRASS, x, 1, z, None, None);
                                }
                            }
                        }
                        "scrub" => {
                            if !editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                                continue;
                            }
                            let random_choice = rng.gen_range(0..500);
                            if random_choice == 0 {
                                Tree::create(editor, (x, 1, z));
                            } else if random_choice == 1 {
                                let flower_block = match rng.gen_range(1..=4) {
                                    1 => RED_FLOWER,
                                    2 => BLUE_FLOWER,
                                    3 => YELLOW_FLOWER,
                                    _ => WHITE_FLOWER,
                                };
                                editor.set_block(flower_block, x, 1, z, None, None);
                            } else if random_choice < 40 {
                                editor.set_block(OAK_LEAVES, x, 1, z, None, None);
                                if random_choice < 15 {
                                    editor.set_block(OAK_LEAVES, x, 2, z, None, None);
                                }
                            } else if random_choice < 300 {
                                if random_choice < 250 {
                                    editor.set_block(GRASS, x, 1, z, None, None);
                                } else {
                                    editor.set_block(TALL_GRASS_BOTTOM, x, 1, z, None, None);
                                    editor.set_block(TALL_GRASS_TOP, x, 2, z, None, None);
                                }
                            }
                        }
                        "tree_row" | "wood" => {
                            if !editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                                continue;
                            }
                            let random_choice: i32 = rng.gen_range(0..30);
                            if random_choice == 0 {
                                Tree::create(editor, (x, 1, z));
                            } else if random_choice == 1 {
                                let flower_block = match rng.gen_range(1..=4) {
                                    1 => RED_FLOWER,
                                    2 => BLUE_FLOWER,
                                    3 => YELLOW_FLOWER,
                                    _ => WHITE_FLOWER,
                                };
                                editor.set_block(flower_block, x, 1, z, None, None);
                            } else if random_choice <= 12 {
                                editor.set_block(GRASS, x, 1, z, None, None);
                            }
                        }
                        "sand" => {
                            if editor.check_for_block(x, 0, z, Some(&[SAND]))
                                && rng.gen_range(0..100) == 1
                            {
                                editor.set_block(DEAD_BUSH, x, 1, z, None, None);
                            }
                        }
                        "shoal" => {
                            if rng.gen_bool(0.05) {
                                editor.set_block(WATER, x, 0, z, Some(&[SAND, GRAVEL]), None);
                            }
                        }
                        "wetland" => {
                            if let Some(wetland_type) = element.tags().get("wetland") {
                                // Wetland without water blocks
                                if matches!(wetland_type.as_str(), "wet_meadow" | "fen") {
                                    if rng.gen_bool(0.3) {
                                        editor.set_block(GRASS_BLOCK, x, 0, z, Some(&[MUD]), None);
                                    }
                                    editor.set_block(GRASS, x, 1, z, None, None);
                                    continue;
                                }
                                // All the other types of wetland
                                if rng.gen_bool(0.3) {
                                    editor.set_block(
                                        WATER,
                                        x,
                                        0,
                                        z,
                                        Some(&[MUD, GRASS_BLOCK]),
                                        None,
                                    );
                                    continue;
                                }
                                if !editor.check_for_block(x, 0, z, Some(&[MUD, MOSS_BLOCK])) {
                                    continue;
                                }
                                match wetland_type.as_str() {
                                    "reedbed" => {
                                        editor.set_block(TALL_GRASS_BOTTOM, x, 1, z, None, None);
                                        editor.set_block(TALL_GRASS_TOP, x, 2, z, None, None);
                                    }
                                    "swamp" | "mangrove" => {
                                        // TODO implement mangrove
                                        let random_choice: i32 = rng.gen_range(0..40);
                                        if random_choice == 0 {
                                            Tree::create(editor, (x, 1, z));
                                        } else if random_choice < 35 {
                                            editor.set_block(GRASS, x, 1, z, None, None);
                                        }
                                    }
                                    "bog" => {
                                        if rng.gen_bool(0.2) {
                                            editor.set_block(
                                                MOSS_BLOCK,
                                                x,
                                                0,
                                                z,
                                                Some(&[MUD]),
                                                None,
                                            );
                                        }
                                        if rng.gen_bool(0.15) {
                                            editor.set_block(GRASS, x, 1, z, None, None);
                                        }
                                    }
                                    "tidalflat" => {
                                        continue; // No vegetation here
                                    }
                                    _ => {
                                        editor.set_block(GRASS, x, 1, z, None, None);
                                    }
                                }
                            } else {
                                // Generic natural=wetland without wetland=... tag
                                if rng.gen_bool(0.3) {
                                    editor.set_block(WATER, x, 0, z, Some(&[MUD]), None);
                                    continue;
                                }
                                editor.set_block(GRASS, x, 1, z, None, None);
                            }
                        }
                        "mountain_range" => {
                            // Create block clusters instead of random placement
                            let cluster_chance = rng.gen_range(0..1000);

                            if cluster_chance < 50 {
                                // 5% chance to start a new cluster
                                let cluster_block = match rng.gen_range(0..7) {
                                    0 => DIRT,
                                    1 => STONE,
                                    2 => GRAVEL,
                                    3 => GRANITE,
                                    4 => DIORITE,
                                    5 => ANDESITE,
                                    _ => GRASS_BLOCK,
                                };

                                // Generate cluster size (5-10 blocks radius)
                                let cluster_size = rng.gen_range(5..=10);

                                // Create cluster around current position
                                for dx in -(cluster_size as i32)..=(cluster_size as i32) {
                                    for dz in -(cluster_size as i32)..=(cluster_size as i32) {
                                        let cluster_x = x + dx;
                                        let cluster_z = z + dz;

                                        // Use distance to create more natural cluster shape
                                        let distance = ((dx * dx + dz * dz) as f32).sqrt();
                                        if distance <= cluster_size as f32 {
                                            // Probability decreases with distance from center
                                            let place_prob = 1.0 - (distance / cluster_size as f32);
                                            if rng.gen::<f32>() < place_prob {
                                                editor.set_block(
                                                    cluster_block,
                                                    cluster_x,
                                                    0,
                                                    cluster_z,
                                                    None,
                                                    None,
                                                );

                                                // Add vegetation on grass blocks
                                                if cluster_block == GRASS_BLOCK {
                                                    let vegetation_chance = rng.gen_range(0..100);
                                                    if vegetation_chance == 0 {
                                                        // 1% chance for rare trees
                                                        Tree::create(
                                                            editor,
                                                            (cluster_x, 1, cluster_z),
                                                        );
                                                    } else if vegetation_chance < 15 {
                                                        // 15% chance for grass
                                                        editor.set_block(
                                                            GRASS, cluster_x, 1, cluster_z, None,
                                                            None,
                                                        );
                                                    } else if vegetation_chance < 25 {
                                                        // 10% chance for oak leaves
                                                        editor.set_block(
                                                            OAK_LEAVES, cluster_x, 1, cluster_z,
                                                            None, None,
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        "saddle" => {
                            // Saddle areas - lowest point between peaks, mix of stone and grass
                            let terrain_chance = rng.gen_range(0..100);
                            if terrain_chance < 30 {
                                // 30% chance for exposed stone
                                editor.set_block(STONE, x, 0, z, None, None);
                            } else if terrain_chance < 50 {
                                // 20% chance for gravel/rocky terrain
                                editor.set_block(GRAVEL, x, 0, z, None, None);
                            } else {
                                // 50% chance for grass
                                editor.set_block(GRASS_BLOCK, x, 0, z, None, None);
                                if rng.gen_bool(0.4) {
                                    // 40% chance for grass on top
                                    editor.set_block(GRASS, x, 1, z, None, None);
                                }
                            }
                        }
                        "ridge" => {
                            // Ridge areas - elevated crest, mostly rocky with some vegetation
                            let ridge_chance = rng.gen_range(0..100);
                            if ridge_chance < 60 {
                                // 60% chance for stone/rocky terrain
                                let rock_type = match rng.gen_range(0..4) {
                                    0 => STONE,
                                    1 => COBBLESTONE,
                                    2 => GRANITE,
                                    _ => ANDESITE,
                                };
                                editor.set_block(rock_type, x, 0, z, None, None);
                            } else {
                                // 40% chance for grass with sparse vegetation
                                editor.set_block(GRASS_BLOCK, x, 0, z, None, None);
                                let vegetation_chance = rng.gen_range(0..100);
                                if vegetation_chance < 20 {
                                    // 20% chance for grass
                                    editor.set_block(GRASS, x, 1, z, None, None);
                                } else if vegetation_chance < 25 {
                                    // 5% chance for small shrubs
                                    editor.set_block(OAK_LEAVES, x, 1, z, None, None);
                                }
                            }
                        }
                        "shrubbery" => {
                            // Manicured shrubs and decorative vegetation
                            editor.set_block(OAK_LEAVES, x, 1, z, None, None);
                            editor.set_block(OAK_LEAVES, x, 2, z, None, None);
                        }
                        "tundra" => {
                            // Treeless habitat with low vegetation, mosses, lichens
                            if !editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                                continue;
                            }
                            let tundra_chance = rng.gen_range(0..100);
                            if tundra_chance < 40 {
                                // 40% chance for grass (sedges, grasses)
                                editor.set_block(GRASS, x, 1, z, None, None);
                            } else if tundra_chance < 60 {
                                // 20% chance for moss
                                editor.set_block(MOSS_BLOCK, x, 0, z, Some(&[GRASS_BLOCK]), None);
                            } else if tundra_chance < 70 {
                                // 10% chance for dead bush (lichens)
                                editor.set_block(DEAD_BUSH, x, 1, z, None, None);
                            }
                            // 30% chance for bare ground (no surface block)
                        }
                        "cliff" => {
                            // Cliff areas - predominantly stone with minimal vegetation
                            let cliff_chance = rng.gen_range(0..100);
                            if cliff_chance < 90 {
                                // 90% chance for stone variants
                                let stone_type = match rng.gen_range(0..4) {
                                    0 => STONE,
                                    1 => COBBLESTONE,
                                    2 => ANDESITE,
                                    _ => DIORITE,
                                };
                                editor.set_block(stone_type, x, 0, z, None, None);
                            } else {
                                // 10% chance for gravel/loose rock
                                editor.set_block(GRAVEL, x, 0, z, None, None);
                            }
                        }
                        "hill" => {
                            // Hill areas - elevated terrain with sparse trees and mostly grass
                            if !editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                                continue;
                            }
                            let hill_chance = rng.gen_range(0..1000);
                            if hill_chance == 0 {
                                // 0.1% chance for rare trees
                                Tree::create(editor, (x, 1, z));
                            } else if hill_chance < 50 {
                                // 5% chance for flowers
                                let flower_block = match rng.gen_range(1..=4) {
                                    1 => RED_FLOWER,
                                    2 => BLUE_FLOWER,
                                    3 => YELLOW_FLOWER,
                                    _ => WHITE_FLOWER,
                                };
                                editor.set_block(flower_block, x, 1, z, None, None);
                            } else if hill_chance < 600 {
                                // 55% chance for grass
                                editor.set_block(GRASS, x, 1, z, None, None);
                            } else if hill_chance < 650 {
                                // 5% chance for tall grass
                                editor.set_block(TALL_GRASS_BOTTOM, x, 1, z, None, None);
                                editor.set_block(TALL_GRASS_TOP, x, 2, z, None, None);
                            }
                            // 35% chance for bare grass block
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

pub fn generate_natural_from_relation(
    editor: &mut WorldEditor,
    rel: &ProcessedRelation,
    args: &Args,
) {
    if rel.tags.contains_key("natural") {
        // Generate individual ways with their original tags
        for member in &rel.members {
            if member.role == ProcessedMemberRole::Outer {
                generate_natural(editor, &ProcessedElement::Way(member.way.clone()), args);
            }
        }

        // Combine all outer ways into one with relation tags
        let mut combined_nodes = Vec::new();
        for member in &rel.members {
            if member.role == ProcessedMemberRole::Outer {
                combined_nodes.extend(member.way.nodes.clone());
            }
        }

        // Only process if we have nodes
        if !combined_nodes.is_empty() {
            // Create combined way with relation tags
            let combined_way = ProcessedWay {
                id: rel.id,
                nodes: combined_nodes,
                tags: rel.tags.clone(),
            };

            // Generate natural area from combined way
            generate_natural(editor, &ProcessedElement::Way(combined_way), args);
        }
    }
}
