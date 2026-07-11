use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::deterministic_rng::element_rng;
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::element_processing::tree::{Tree, TreeType};
use crate::floodfill_cache::{BuildingFootprintBitmap, FloodFillCache};
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::{prelude::IndexedRandom, Rng};

pub fn generate_natural(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    bridge_surface: &BridgeSurfaceMap,
) {
    if let Some(natural_type) = element.tags().get("natural") {
        if natural_type == "tree" {
            if let ProcessedElement::Node(node) = element {
                let x: i32 = node.x;
                let z: i32 = node.z;

                let mut trees_ok_to_generate: Vec<TreeType> = vec![];
                if let Some(species) = element.tags().get("species") {
                    if species.contains("Betula") {
                        trees_ok_to_generate.push(TreeType::Birch);
                    }
                    if species.contains("Quercus") {
                        trees_ok_to_generate.push(TreeType::Oak);
                    }
                    if species.contains("Picea") {
                        trees_ok_to_generate.push(TreeType::Spruce);
                    }
                } else if let Some(genus_wikidata) = element.tags().get("genus:wikidata") {
                    match genus_wikidata.as_str() {
                        "Q12004" => trees_ok_to_generate.push(TreeType::Birch),
                        "Q26782" => trees_ok_to_generate.push(TreeType::Oak),
                        "Q25243" => trees_ok_to_generate.push(TreeType::Spruce),
                        _ => {
                            trees_ok_to_generate.push(TreeType::Oak);
                            trees_ok_to_generate.push(TreeType::Spruce);
                            trees_ok_to_generate.push(TreeType::Birch);
                        }
                    }
                } else if let Some(genus) = element.tags().get("genus") {
                    match genus.as_str() {
                        "Betula" => trees_ok_to_generate.push(TreeType::Birch),
                        "Quercus" => trees_ok_to_generate.push(TreeType::Oak),
                        "Picea" => trees_ok_to_generate.push(TreeType::Spruce),
                        _ => trees_ok_to_generate.push(TreeType::Oak),
                    }
                } else if let Some(leaf_type) = element.tags().get("leaf_type") {
                    match leaf_type.as_str() {
                        "broadleaved" => {
                            trees_ok_to_generate.push(TreeType::Oak);
                            trees_ok_to_generate.push(TreeType::Birch);
                            trees_ok_to_generate.push(TreeType::TallOak);
                        }
                        "needleleaved" => {
                            trees_ok_to_generate.push(TreeType::Spruce);
                            trees_ok_to_generate.push(TreeType::Pine);
                        }
                        _ => {
                            trees_ok_to_generate.push(TreeType::Oak);
                            trees_ok_to_generate.push(TreeType::Spruce);
                            trees_ok_to_generate.push(TreeType::Birch);
                            trees_ok_to_generate.push(TreeType::TallOak);
                            trees_ok_to_generate.push(TreeType::Pine);
                        }
                    }
                } else {
                    trees_ok_to_generate.push(TreeType::Oak);
                    trees_ok_to_generate.push(TreeType::Spruce);
                    trees_ok_to_generate.push(TreeType::Birch);
                    trees_ok_to_generate.push(TreeType::TallOak);
                }

                if trees_ok_to_generate.is_empty() {
                    trees_ok_to_generate.push(TreeType::Oak);
                    trees_ok_to_generate.push(TreeType::Spruce);
                    trees_ok_to_generate.push(TreeType::Birch);
                }

                let mut rng = element_rng(element.id());
                let tree_type = *trees_ok_to_generate
                    .choose(&mut rng)
                    .unwrap_or(&TreeType::Oak);

                // Deliberately-mapped `natural=tree` node: allow it to stand on paving.
                Tree::create_of_type(
                    editor,
                    (x, 1, z),
                    tree_type,
                    Some(building_footprints),
                    Some(bridge_surface),
                    true,
                );
            }
        } else {
            let mut previous_node: Option<(i32, i32)> = None;
            let mut corner_count: i32 = 0;
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
                "water" | "reef" | "bay" => WATER,
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

            // Whether this natural type should have per-block rock variation
            // via `vary_rock_block`. Note: "bare_rock" is deliberately NOT in
            // this list — it has its own dedicated 6-class mix in the match
            // arm below (STONE/ANDESITE/COBBLESTONE/GRAVEL/TUFF/COARSE_DIRT)
            // which overwrites whatever we put here. Including it in
            // rock_variation would mean two different mixes race against each
            // other at the same cell, where the match-arm mix wins but the
            // first placement is wasted work.
            let rock_variation = matches!(
                natural_type.as_str(),
                "blockfield" | "cliff" | "saddle" | "ridge" | "mountain_range"
            );

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
                        // Don't overwrite road blocks with natural ground
                        if !editor.check_for_block(
                            bx,
                            0,
                            bz,
                            Some(&[
                                BLACK_CONCRETE,
                                GRAY_CONCRETE_POWDER,
                                CYAN_TERRACOTTA,
                                GRAY_CONCRETE,
                                LIGHT_GRAY_CONCRETE,
                                WHITE_CONCRETE,
                                DIRT_PATH,
                                SMOOTH_STONE,
                            ]),
                        ) {
                            let b = if rock_variation {
                                vary_rock_block(block_type, bx, bz)
                            } else {
                                block_type
                            };
                            editor.set_block(b, bx, 0, bz, None, None);
                        }
                    }

                    current_natural.push((x, z));
                    corner_count += 1;
                }

                previous_node = Some((x, z));
            }

            // If there are natural nodes, flood-fill the area using cache
            if corner_count > 0 {
                let filled_area = flood_fill_cache.get_or_compute(way, args.timeout.as_ref());

                let trees_ok_to_generate: Vec<TreeType> = {
                    let mut trees: Vec<TreeType> = vec![];
                    if let Some(leaf_type) = element.tags().get("leaf_type") {
                        match leaf_type.as_str() {
                            "broadleaved" => {
                                trees.push(TreeType::Oak);
                                trees.push(TreeType::Birch);
                                trees.push(TreeType::TallOak);
                                trees.push(TreeType::Bush);
                                trees.push(TreeType::AzaleaBush);
                            }
                            "needleleaved" => {
                                trees.push(TreeType::Spruce);
                                trees.push(TreeType::Pine);
                            }
                            _ => {
                                trees.push(TreeType::Oak);
                                trees.push(TreeType::Spruce);
                                trees.push(TreeType::Birch);
                                trees.push(TreeType::TallOak);
                                trees.push(TreeType::Pine);
                                trees.push(TreeType::Bush);
                                trees.push(TreeType::AzaleaBush);
                            }
                        }
                    } else {
                        trees.push(TreeType::Oak);
                        trees.push(TreeType::Spruce);
                        trees.push(TreeType::Birch);
                        trees.push(TreeType::TallOak);
                        trees.push(TreeType::Bush);
                        trees.push(TreeType::AzaleaBush);
                    }
                    trees
                };

                // Use deterministic RNG seeded by element ID for consistent results across region boundaries
                let mut rng = element_rng(way.id);

                // Blocks that natural areas should not overwrite
                let protected_blocks: &[Block] = &[
                    BLACK_CONCRETE,
                    GRAY_CONCRETE_POWDER,
                    CYAN_TERRACOTTA,
                    GRAY_CONCRETE,
                    LIGHT_GRAY_CONCRETE,
                    WHITE_CONCRETE,
                    DIRT_PATH,
                    SMOOTH_STONE,
                    WATER,
                ];

                let mut wetland_puddles: Vec<(i32, i32)> = Vec::new();

                for &(x, z) in filled_area.iter() {
                    // Don't overwrite road/path blocks with natural ground
                    if !editor.check_for_block(x, 0, z, Some(protected_blocks)) {
                        let b = if rock_variation {
                            vary_rock_block(block_type, x, z)
                        } else {
                            block_type
                        };
                        editor.set_block(b, x, 0, z, None, None);
                    }
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
                            // Varied rock surface: stone base with natural variation
                            let h = crate::land_cover::coord_hash(x, z) % 12;
                            let rock = match h {
                                0..=4 => STONE,       // ~42% stone
                                5..=6 => ANDESITE,    // ~17% andesite
                                7..=8 => COBBLESTONE, // ~17% cobblestone
                                9 => GRAVEL,          // ~8% gravel
                                10 => TUFF,           // ~8% tuff
                                _ => COARSE_DIRT,     // ~8% coarse dirt
                            };
                            editor.set_block(rock, x, 0, z, None, None);
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
                            if rng.random_bool(0.6) {
                                editor.set_block(GRASS, x, 1, z, None, None);
                            }
                        }
                        "heath" => {
                            if !editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                                continue;
                            }
                            let random_choice = rng.random_range(0..500);
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
                            let random_choice = rng.random_range(0..500);
                            if random_choice == 0 {
                                Tree::create(
                                    editor,
                                    (x, 1, z),
                                    Some(building_footprints),
                                    Some(bridge_surface),
                                );
                            } else if random_choice == 1 {
                                let flower_block = match rng.random_range(1..=4) {
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
                            let density = crate::ground_generation::value_noise_01(x, z, 32);
                            let tree_threshold = ((60.0 - density * 45.0) as i32).max(5);
                            let spawn_tree = rng.random_range(0..tree_threshold) == 0;
                            let random_choice: i32 = rng.random_range(0..30);
                            if spawn_tree {
                                let tree_type = *trees_ok_to_generate
                                    .choose(&mut rng)
                                    .unwrap_or(&TreeType::Oak);
                                Tree::create_of_type(
                                    editor,
                                    (x, 1, z),
                                    tree_type,
                                    Some(building_footprints),
                                    Some(bridge_surface),
                                    false,
                                );
                            } else if random_choice == 1 {
                                let flower_block = match rng.random_range(1..=4) {
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
                        "sand"
                            if editor.check_for_block(x, 0, z, Some(&[SAND]))
                                && rng.random_range(0..100) == 1 =>
                        {
                            editor.set_block(DEAD_BUSH, x, 1, z, None, None);
                        }
                        "shoal" if rng.random_bool(0.05) => {
                            editor.set_block(WATER, x, 0, z, Some(&[SAND, GRAVEL]), None);
                        }
                        "wetland" => {
                            let wetland_type = element
                                .tags()
                                .get("wetland")
                                .map(String::as_str)
                                .unwrap_or("");
                            // Wetland without water blocks
                            if matches!(wetland_type, "wet_meadow" | "fen") {
                                if rng.random_bool(0.3) {
                                    editor.set_block(GRASS_BLOCK, x, 0, z, Some(&[MUD]), None);
                                }
                                editor.set_block(GRASS, x, 1, z, None, None);
                                continue;
                            }
                            // Tidalflat stays bare mud with scattered water, no mosaic
                            if wetland_type == "tidalflat" {
                                if rng.random_bool(0.3) {
                                    editor.set_block(WATER, x, 0, z, Some(&[MUD]), None);
                                }
                                continue;
                            }
                            // Positional wet/dry mosaic; puddle cells take water and skip vegetation
                            let wet = wetland_wet_zone(x, z);
                            if wet && wetland_puddle_noise(x, z) {
                                if try_place_wetland_puddle(editor, x, z) {
                                    wetland_puddles.push((x, z));
                                }
                                continue;
                            }
                            if wet {
                                if crate::ground_generation::value_noise_01(x + 53, z + 71, 8)
                                    > 0.55
                                {
                                    editor.set_block(COARSE_DIRT, x, 0, z, Some(&[MUD]), None);
                                }
                            } else if rng.random_bool(0.4) {
                                editor.set_block(GRASS_BLOCK, x, 0, z, Some(&[MUD]), None);
                            }
                            if !editor.check_for_block(
                                x,
                                0,
                                z,
                                Some(&[MUD, MOSS_BLOCK, COARSE_DIRT, GRASS_BLOCK, DIRT]),
                            ) {
                                continue;
                            }
                            match wetland_type {
                                "reedbed" => {
                                    if rng.random_range(0..100) < 45 {
                                        editor.set_block(TALL_GRASS_BOTTOM, x, 1, z, None, None);
                                        editor.set_block(TALL_GRASS_TOP, x, 2, z, None, None);
                                    }
                                }
                                "swamp" | "mangrove" => {
                                    let r: i32 = rng.random_range(0..40);
                                    if r == 0 {
                                        let tree_type = if wetland_type == "mangrove" {
                                            TreeType::Mangrove
                                        } else if rng.random_bool(0.6) {
                                            TreeType::Willow
                                        } else {
                                            TreeType::Mangrove
                                        };
                                        Tree::create_of_type(
                                            editor,
                                            (x, 1, z),
                                            tree_type,
                                            Some(building_footprints),
                                            Some(bridge_surface),
                                            false,
                                        );
                                    } else if r < 15 {
                                        place_grass_or_tall(editor, &mut rng, x, z);
                                    }
                                }
                                "bog" => {
                                    if rng.random_bool(0.2) {
                                        editor.set_block(MOSS_BLOCK, x, 0, z, Some(&[MUD]), None);
                                    }
                                    if rng.random_bool(0.08) {
                                        place_grass_or_tall(editor, &mut rng, x, z);
                                    }
                                }
                                _ => place_grass_or_tall(editor, &mut rng, x, z),
                            }
                        }
                        "mountain_range" => {
                            // Create block clusters instead of random placement
                            let cluster_chance = rng.random_range(0..1000);

                            if cluster_chance < 50 {
                                // 5% chance to start a new cluster
                                let cluster_block = match rng.random_range(0..7) {
                                    0 => DIRT,
                                    1 => STONE,
                                    2 => GRAVEL,
                                    3 => GRANITE,
                                    4 => DIORITE,
                                    5 => ANDESITE,
                                    _ => GRASS_BLOCK,
                                };

                                // Generate cluster size (5-10 blocks radius)
                                let cluster_size = rng.random_range(5..=10);

                                // Create cluster around current position
                                for dx in -cluster_size..=cluster_size {
                                    for dz in -cluster_size..=cluster_size {
                                        let cluster_x = x + dx;
                                        let cluster_z = z + dz;

                                        // Use distance to create more natural cluster shape
                                        let distance = ((dx * dx + dz * dz) as f32).sqrt();
                                        if distance <= cluster_size as f32 {
                                            // Probability decreases with distance from center
                                            let place_prob = 1.0 - (distance / cluster_size as f32);
                                            if rng.random::<f32>() < place_prob {
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
                                                    let vegetation_chance =
                                                        rng.random_range(0..100);
                                                    if vegetation_chance == 0 {
                                                        // 1% chance for rare trees
                                                        Tree::create(
                                                            editor,
                                                            (cluster_x, 1, cluster_z),
                                                            Some(building_footprints),
                                                            Some(bridge_surface),
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
                            let terrain_chance = rng.random_range(0..100);
                            if terrain_chance < 30 {
                                // 30% chance for exposed stone
                                editor.set_block(STONE, x, 0, z, None, None);
                            } else if terrain_chance < 50 {
                                // 20% chance for gravel/rocky terrain
                                editor.set_block(GRAVEL, x, 0, z, None, None);
                            } else {
                                // 50% chance for grass
                                editor.set_block(GRASS_BLOCK, x, 0, z, None, None);
                                if rng.random_bool(0.4) {
                                    // 40% chance for grass on top
                                    editor.set_block(GRASS, x, 1, z, None, None);
                                }
                            }
                        }
                        "ridge" => {
                            // Ridge areas - elevated crest, mostly rocky with some vegetation
                            let ridge_chance = rng.random_range(0..100);
                            if ridge_chance < 60 {
                                // 60% chance for stone/rocky terrain
                                let rock_type = match rng.random_range(0..4) {
                                    0 => STONE,
                                    1 => COBBLESTONE,
                                    2 => GRANITE,
                                    _ => ANDESITE,
                                };
                                editor.set_block(rock_type, x, 0, z, None, None);
                            } else {
                                // 40% chance for grass with sparse vegetation
                                editor.set_block(GRASS_BLOCK, x, 0, z, None, None);
                                let vegetation_chance = rng.random_range(0..100);
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
                            let tundra_chance = rng.random_range(0..100);
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
                            let cliff_chance = rng.random_range(0..100);
                            if cliff_chance < 90 {
                                // 90% chance for stone variants
                                let stone_type = match rng.random_range(0..4) {
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
                            let hill_chance = rng.random_range(0..1000);
                            if hill_chance == 0 {
                                // 0.1% chance for rare trees
                                Tree::create(
                                    editor,
                                    (x, 1, z),
                                    Some(building_footprints),
                                    Some(bridge_surface),
                                );
                            } else if hill_chance < 50 {
                                // 5% chance for flowers
                                let flower_block = match rng.random_range(1..=4) {
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

                // Rings and cane must stay inside the polygon; 1-bit-per-cell
                // bitmap over the polygon rect instead of a hashset (~48B/cell).
                if !wetland_puddles.is_empty() {
                    let (mut min_x, mut min_z, mut max_x, mut max_z) = {
                        let &(x0, z0) = &filled_area[0];
                        (x0, z0, x0, z0)
                    };
                    for &(x, z) in filled_area.iter() {
                        min_x = min_x.min(x);
                        max_x = max_x.max(x);
                        min_z = min_z.min(z);
                        max_z = max_z.max(z);
                    }
                    let mut area = crate::floodfill_cache::CoordinateBitmap::new_empty();
                    if let Ok(rect) = crate::coordinate_system::cartesian::XZBBox::rect_from_min_max(
                        min_x, min_z, max_x, max_z,
                    ) {
                        area = crate::floodfill_cache::CoordinateBitmap::new(&rect);
                        for &(x, z) in filled_area.iter() {
                            area.set(x, z);
                        }
                    }
                    // Puddle rings, order-independent: Chebyshev 1 = moss, 2 = coarse dirt
                    for &(px, pz) in &wetland_puddles {
                        for dx in -2i32..=2 {
                            for dz in -2i32..=2 {
                                let d = dx.abs().max(dz.abs());
                                let (nx, nz) = (px + dx, pz + dz);
                                if d == 0 || !area.contains(nx, nz) {
                                    continue;
                                }
                                if d == 1 {
                                    editor.set_block(
                                        MOSS_BLOCK,
                                        nx,
                                        0,
                                        nz,
                                        Some(&[MUD, GRASS_BLOCK, DIRT, COARSE_DIRT]),
                                        None,
                                    );
                                } else {
                                    editor.set_block(
                                        COARSE_DIRT,
                                        nx,
                                        0,
                                        nz,
                                        Some(&[MUD, GRASS_BLOCK, DIRT]),
                                        None,
                                    );
                                }
                            }
                        }
                    }
                    // Sugar cane at puddle edges, positional so it is seam-safe and idempotent
                    for &(px, pz) in &wetland_puddles {
                        for &(dx, dz) in &[(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                            let (nx, nz) = (px + dx, pz + dz);
                            if !area.contains(nx, nz) {
                                continue;
                            }
                            if crate::land_cover::coord_hash(
                                nx.wrapping_add(89),
                                nz.wrapping_add(97),
                            ) % 100
                                >= 20
                            {
                                continue;
                            }
                            if !editor.check_for_block(
                                nx,
                                0,
                                nz,
                                Some(&[GRASS_BLOCK, MUD, DIRT, COARSE_DIRT, MOSS_BLOCK]),
                            ) {
                                continue;
                            }
                            let h = 1
                                + (crate::land_cover::coord_hash(
                                    nx.wrapping_add(131),
                                    nz.wrapping_add(137),
                                ) % 3) as i32;
                            // Stop at the first occupied level so cane never floats
                            for y in 1..=h {
                                if editor.block_at(nx, y, nz) {
                                    break;
                                }
                                editor.set_block(SUGAR_CANE, nx, y, nz, None, None);
                            }
                        }
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
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    bridge_surface: &BridgeSurfaceMap,
) {
    if rel.tags.contains_key("natural") {
        // Process each outer member way individually using cached flood fill.
        // We intentionally do not combine all outer nodes into one mega-way,
        // because that creates a nonsensical polygon spanning the whole relation
        // extent, misses the flood fill cache, and can cause multi-GB allocations.
        for member in &rel.members {
            if member.role == ProcessedMemberRole::Outer {
                // Use relation tags so the member inherits the relation's natural=* type
                let way_with_rel_tags = ProcessedWay {
                    id: member.way.id,
                    nodes: member.way.nodes.clone(),
                    tags: rel.tags.clone(),
                };
                generate_natural(
                    editor,
                    &ProcessedElement::Way(way_with_rel_tags),
                    args,
                    flood_fill_cache,
                    building_footprints,
                    bridge_surface,
                );
            }
        }
    }
}

/// Vary a rock block type per-coordinate for natural rock areas.
/// Uses coord_hash for deterministic, spatially-coherent variation.
fn vary_rock_block(base: Block, x: i32, z: i32) -> Block {
    let h = crate::land_cover::coord_hash(x, z) % 10;
    match base {
        STONE => match h {
            0..=4 => STONE,
            5..=6 => ANDESITE,
            7 => COBBLESTONE,
            _ => GRAVEL,
        },
        COBBLESTONE => match h {
            0..=4 => COBBLESTONE,
            5..=6 => ANDESITE,
            7 => STONE,
            _ => GRAVEL,
        },
        _ => base,
    }
}

// Wet/dry mosaic gate for wetland cells, positional so it is seam-safe
fn wetland_wet_zone(x: i32, z: i32) -> bool {
    crate::ground_generation::value_noise_01(x + 11, z + 7, 28) > 0.55
}

fn wetland_puddle_noise(x: i32, z: i32) -> bool {
    crate::ground_generation::value_noise_01(x + 31, z + 17, 6) > 0.78
}

#[cfg(test)]
fn wetland_puddle_at(x: i32, z: i32) -> bool {
    wetland_wet_zone(x, z) && wetland_puddle_noise(x, z)
}

// Water only over wetland ground; roads, buildings and existing water stay
fn try_place_wetland_puddle(editor: &mut WorldEditor, x: i32, z: i32) -> bool {
    if editor.check_for_block(x, 0, z, Some(&[MUD, GRASS_BLOCK])) {
        editor.set_block(WATER, x, 0, z, Some(&[MUD, GRASS_BLOCK]), None);
        true
    } else {
        false
    }
}

fn place_grass_or_tall(editor: &mut WorldEditor, rng: &mut impl Rng, x: i32, z: i32) {
    let r = rng.random_range(0..100);
    if r < 10 {
        editor.set_block(TALL_GRASS_BOTTOM, x, 1, z, None, None);
        editor.set_block(TALL_GRASS_TOP, x, 2, z, None, None);
    } else if r < 25 {
        editor.set_block(GRASS, x, 1, z, None, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;

    #[test]
    fn wetland_mosaic_deterministic_and_nondegenerate() {
        let (mut wet, mut dry) = (0u32, 0u32);
        for x in -100..100 {
            for z in -100..100 {
                let p = wetland_puddle_at(x, z);
                assert_eq!(p, wetland_puddle_at(x, z));
                if p {
                    assert!(wetland_wet_zone(x, z));
                    wet += 1;
                } else {
                    dry += 1;
                }
            }
        }
        assert!(wet > 0 && dry > 0);
    }

    #[test]
    fn puddle_respects_protected_ground() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 15, 15).unwrap();
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let mut editor = WorldEditor::new(std::env::temp_dir(), &xzbbox, llbbox);
        editor.set_block(BLACK_CONCRETE, 3, 0, 3, None, None);
        editor.set_block(WATER, 4, 0, 4, None, None);
        editor.set_block(MUD, 5, 0, 5, None, None);
        assert!(!try_place_wetland_puddle(&mut editor, 3, 3));
        assert!(editor.check_for_block(3, 0, 3, Some(&[BLACK_CONCRETE])));
        assert!(!try_place_wetland_puddle(&mut editor, 4, 4));
        assert!(try_place_wetland_puddle(&mut editor, 5, 5));
        assert!(editor.check_for_block(5, 0, 5, Some(&[WATER])));
    }
}
