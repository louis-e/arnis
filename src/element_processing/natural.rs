use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::climate::Climate;
use crate::deterministic_rng::element_rng;
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::element_processing::bush::{self, place_bush, BushKind};
use crate::element_processing::tree::{Tree, TreeType};
use crate::floodfill_cache::{is_oversized_ring, BuildingFootprintBitmap, FloodFillCache};
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::trees::mapped::{tree_row_positions, MappedTree};
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
                let mapped = MappedTree::from_tags(&node.tags, node.id);
                Tree::create_mapped(
                    editor,
                    (node.x, 1, node.z),
                    &mapped,
                    Some(building_footprints),
                    Some(bridge_surface),
                );
            }
        } else if natural_type == "tree_row" {
            if let ProcessedElement::Way(way) = element {
                let mapped = MappedTree::from_tags(&way.tags, way.id);
                for (x, z) in tree_row_positions(&way.nodes, editor.scale()) {
                    Tree::create_mapped(
                        editor,
                        (x, 1, z),
                        &mapped,
                        Some(building_footprints),
                        Some(bridge_surface),
                    );
                }
            }
        } else {
            let mut previous_node: Option<(i32, i32)> = None;
            let mut corner_count: i32 = 0;
            let mut current_natural: Vec<(i32, i32)> = vec![];

            // Determine block type based on natural tag
            let block_type: Block = match natural_type.as_str() {
                "scrub" | "grassland" | "wood" | "heath" => GRASS_BLOCK,
                "sand" | "dune" => SAND,
                "beach" | "shoal" => beach_block(element.tags().get("surface")),
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

            // Resolve the fill before painting the edge. It is cached, so this costs nothing
            // extra. A closed ring that comes back empty is one the fill refused for size, and
            // drawing its edge anyway leaves a border around ground nothing ever filled.
            let filled_area = flood_fill_cache.get_or_compute(way, args.timeout.as_ref());
            if filled_area.is_empty() && is_oversized_ring(way) {
                return;
            }

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
                        if !editor.surface_is_sealed(bx, bz)
                            && !editor.check_for_block(
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
                            )
                        {
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
                let leaf_type_tagged = matches!(
                    element.tags().get("leaf_type").map(String::as_str),
                    Some("broadleaved" | "needleleaved")
                );
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
                let arid = natural_type == "sand"
                    && matches!(
                        editor.climate(),
                        Climate::HotDesert
                            | Climate::HotSteppe
                            | Climate::ColdDesert
                            | Climate::ColdSteppe
                    );

                let mut shrubbery_species: Option<Block> = None;
                for &(x, z) in filled_area.iter() {
                    // Roads, paths and paved areas keep their own surface. Checked
                    // by mask because a gravel or dirt road is not in the block list.
                    let sealed = editor.surface_is_sealed(x, z);
                    if !sealed && !editor.check_for_block(x, 0, z, Some(protected_blocks)) {
                        let b = if rock_variation {
                            vary_rock_block(block_type, x, z)
                        } else {
                            block_type
                        };
                        editor.set_block(b, x, 0, z, None, None);
                    }
                    if sealed {
                        continue;
                    }
                    // Generate custom layer instead of dirt, must be stone on the lowest level
                    match natural_type.as_str() {
                        "beach" | "sand" | "dune" | "shoal" => {
                            editor.set_block(block_type, x, 0, z, None, None);
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
                    // Only this tile's own cells get plants and trees: a neighbouring
                    // tile's halo copy draws its own random sequence, so its plants and
                    // trees would land on this tile's water and under its trunks. Wetland
                    // puddles are positional, so halo cells still record them, and the
                    // ring and cane around a puddle just across the seam come out whole.
                    if !editor.owns(x, z) {
                        if natural_type == "wetland"
                            && wetland_puddle_cell(element.tags(), x, z)
                            && try_place_wetland_puddle(editor, x, z)
                        {
                            wetland_puddles.push((x, z));
                        }
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
                                    place_bush(editor, x, z, BushKind::Low);
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
                            if random_choice == 0 && editor.land_cover_backs_trees(x, z) {
                                Tree::create(
                                    editor,
                                    (x, 1, z),
                                    Some(building_footprints),
                                    Some(bridge_surface),
                                );
                            } else if random_choice == 1 {
                                crate::ground_decoration::place_scattered_flower(
                                    editor,
                                    x,
                                    z,
                                    crate::ground_decoration::FlowerSetting::Meadow,
                                );
                            } else if random_choice < 40 {
                                place_bush(editor, x, z, BushKind::Wild);
                            } else if random_choice < 300 {
                                if random_choice < 250 {
                                    editor.set_block(GRASS, x, 1, z, None, None);
                                } else {
                                    editor.set_block(TALL_GRASS_BOTTOM, x, 1, z, None, None);
                                    editor.set_block(TALL_GRASS_TOP, x, 2, z, None, None);
                                }
                            }
                        }
                        "wood" => {
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
                                    leaf_type_tagged,
                                );
                            } else if random_choice == 1 {
                                crate::ground_decoration::place_scattered_flower(
                                    editor,
                                    x,
                                    z,
                                    crate::ground_decoration::FlowerSetting::Forest,
                                );
                            } else if random_choice <= 12 {
                                editor.set_block(GRASS, x, 1, z, None, None);
                            }
                        }
                        // Dead bushes belong to deserts; coastal dunes elsewhere stay bare.
                        "sand"
                            if arid
                                && editor.check_for_block(x, 0, z, Some(&[SAND]))
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
                            if wetland_puddle_cell(element.tags(), x, z) {
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
                                            true,
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
                                                    if vegetation_chance == 0
                                                        && editor.land_cover_backs_trees(
                                                            cluster_x, cluster_z,
                                                        )
                                                    {
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
                                                        // 10% chance for a low bush
                                                        place_bush(
                                                            editor,
                                                            cluster_x,
                                                            cluster_z,
                                                            BushKind::Low,
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
                                    place_bush(editor, x, z, BushKind::Low);
                                }
                            }
                        }
                        "shrubbery" => {
                            // Manicured shrubs, one species per bed
                            let species = *shrubbery_species.get_or_insert_with(|| {
                                let anchor = way.nodes.first().map_or((x, z), |n| (n.x, n.z));
                                bush::shrubbery_species(editor, anchor.0, anchor.1)
                            });
                            for y in 1..=2 {
                                editor.set_block(
                                    bush::shrubbery_leaf(species, x, y, z),
                                    x,
                                    y,
                                    z,
                                    None,
                                    None,
                                );
                            }
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
                            if hill_chance == 0 && editor.land_cover_backs_trees(x, z) {
                                // 0.1% chance for rare trees
                                Tree::create(
                                    editor,
                                    (x, 1, z),
                                    Some(building_footprints),
                                    Some(bridge_surface),
                                );
                            } else if hill_chance < 50 {
                                // 5% chance for flowers
                                crate::ground_decoration::place_scattered_flower(
                                    editor,
                                    x,
                                    z,
                                    crate::ground_decoration::FlowerSetting::Meadow,
                                );
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
                            if !area.contains(nx, nz) || !editor.owns(nx, nz) {
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

/// Ground of a `natural=beach` or `shoal`, from its `surface=*`: sand unless mapped otherwise.
fn beach_block(surface: Option<&String>) -> Block {
    match surface.map(String::as_str) {
        Some("gravel" | "fine_gravel" | "pebblestone" | "pebbles" | "shingle" | "stones") => GRAVEL,
        Some("rock" | "stone" | "bare_rock") => STONE,
        Some("mud") => MUD,
        _ => SAND,
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
/// A cell of a water-bearing wetland that holds a puddle; positional, so every
/// tile reaching the cell agrees.
fn wetland_puddle_cell(tags: &std::collections::HashMap<String, String>, x: i32, z: i32) -> bool {
    let wetland_type = tags.get("wetland").map(String::as_str).unwrap_or("");
    !matches!(wetland_type, "wet_meadow" | "fen" | "tidalflat")
        && wetland_wet_zone(x, z)
        && wetland_puddle_noise(x, z)
}

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

    #[test]
    fn a_beach_takes_its_ground_from_the_surface_tag() {
        let surface = |s: &str| Some(s.to_string());
        assert_eq!(beach_block(None), SAND);
        assert_eq!(beach_block(surface("sand").as_ref()), SAND);
        for pebbles in ["gravel", "pebblestone", "shingle", "fine_gravel"] {
            assert_eq!(beach_block(surface(pebbles).as_ref()), GRAVEL, "{pebbles}");
        }
        assert_eq!(beach_block(surface("rock").as_ref()), STONE);
    }

    /// Renders one natural area over x/z 5..=54 at the given place and returns the editor.
    fn render_natural(llbbox: LLBBox, tags: &[(&str, &str)]) -> WorldEditor<'static> {
        use crate::element_processing::bridge_styles::BridgeOutlineIndex;
        use crate::element_processing::bridges::BridgeStructureMap;
        use crate::element_processing::building_test_support::rect_way;
        use clap::Parser as _;

        let xzbbox = Box::leak(Box::new(XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap()));
        let mut editor = WorldEditor::new(std::env::temp_dir(), xzbbox, llbbox);
        let outlines = BridgeOutlineIndex::build(&[]);
        let structures = BridgeStructureMap::build(&[], &editor, &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&[], &structures, 1.0);
        let args = Args::parse_from([
            "arnis",
            "--bbox",
            "1,2,3,4",
            "--mode",
            "geo-only",
            "--ground-level",
            "0",
        ]);
        let way = rect_way(9, 5, 5, 54, 54, tags);
        generate_natural(
            &mut editor,
            &ProcessedElement::Way(way),
            &args,
            &FloodFillCache::new(),
            &BuildingFootprintBitmap::new_empty(),
            &surface,
        );
        editor
    }

    fn count(editor: &WorldEditor, y: i32, blocks: &[Block]) -> usize {
        (10..50)
            .flat_map(|x| (10..50).map(move |z| (x, z)))
            .filter(|&(x, z)| editor.check_for_block(x, y, z, Some(blocks)))
            .count()
    }

    #[test]
    fn a_gravel_beach_is_gravel_and_a_plain_beach_is_sand() {
        let place = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let gravel = render_natural(place, &[("natural", "beach"), ("surface", "gravel")]);
        assert_eq!(count(&gravel, 0, &[GRAVEL]), 1600);
        let sand = render_natural(place, &[("natural", "beach")]);
        assert_eq!(count(&sand, 0, &[SAND]), 1600);
    }

    #[test]
    fn dead_bushes_grow_on_desert_sand_but_not_on_coastal_sand() {
        let baltic = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let coast = render_natural(baltic, &[("natural", "sand")]);
        assert_eq!(count(&coast, 1, &[DEAD_BUSH]), 0);

        let sahara = LLBBox::new(25.0, 25.0, 25.01, 25.01).unwrap();
        let desert = render_natural(sahara, &[("natural", "sand")]);
        assert!(count(&desert, 1, &[DEAD_BUSH]) > 0);
    }
}
