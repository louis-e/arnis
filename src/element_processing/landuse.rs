use crate::args::Args;
use crate::block_definitions::*;
use crate::cartesian::XZPoint;
use crate::element_processing::tree::Tree;
use crate::floodfill::flood_fill_area;
use crate::ground::Ground;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;
use rand::Rng;

pub fn generate_landuse(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    ground: &Ground,
    args: &Args,
) {
    // Determine block type based on landuse tag
    let binding: String = "".to_string();
    let landuse_tag: &String = element.tags.get("landuse").unwrap_or(&binding);

    let block_type = match landuse_tag.as_str() {
        "greenfield" | "meadow" | "grass" => {
            if args.winter {
                SNOW_BLOCK
            } else {
                GRASS_BLOCK
            }
        }
        "farmland" => FARMLAND,
        "forest" => {
            if args.winter {
                SNOW_BLOCK
            } else {
                GRASS_BLOCK
            }
        }
        "cemetery" => PODZOL,
        "beach" => SAND,
        "construction" => COARSE_DIRT,
        "traffic_island" => STONE_BLOCK_SLAB,
        "residential" => {
            let residential_tag = element.tags.get("residential").unwrap_or(&binding);
            if residential_tag == "rural" {
                if args.winter {
                    SNOW_BLOCK
                } else {
                    GRASS_BLOCK
                }
            } else {
                STONE_BRICKS
            }
        }
        "commercial" => SMOOTH_STONE,
        "education" => LIGHT_GRAY_CONCRETE,
        "industrial" => COBBLESTONE,
        "military" => GRAY_CONCRETE,
        "railway" => GRAVEL,
        "landfill" => {
            // Gravel if man_made = spoil_heap or heap, coarse dirt else
            let manmade_tag = element.tags.get("man_made").unwrap_or(&binding);
            if manmade_tag == "spoil_heap" || manmade_tag == "heap" {
                GRAVEL
            } else {
                COARSE_DIRT
            }
        }
        "quarry" => STONE,
        _ => {
            if args.winter {
                SNOW_BLOCK
            } else {
                GRASS_BLOCK
            }
        }
    };

    // Get the area of the landuse element
    let polygon_coords: Vec<(i32, i32)> = element.nodes.iter().map(|n| (n.x, n.z)).collect();
    let floor_area: Vec<(i32, i32)> = flood_fill_area(&polygon_coords, args.timeout.as_ref());

    let mut rng: rand::prelude::ThreadRng = rand::thread_rng();

    for (x, z) in floor_area {
        let ground_level = ground.level(XZPoint::new(x, z));
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
                        Tree::create(editor, (x, ground_level + 1, z), args.winter);
                    }
                }
            }
            "forest" => {
                if !editor.check_for_block(x, ground_level, z, None, Some(&[WATER])) {
                    let random_choice: i32 = rng.gen_range(0..21);
                    if random_choice == 20 {
                        Tree::create(editor, (x, ground_level + 1, z), args.winter);
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
                if !editor.check_for_block(x, ground_level, z, None, Some(&[WATER, ICE])) {
                    if x % 8 == 0 && z % 8 == 0 {
                        // Place water/ice in dot pattern
                        if args.winter {
                            // Less frozen water in winter
                            if rng.gen_range(0..8) == 0 {
                                editor.set_block(ICE, x, ground_level, z, Some(&[FARMLAND]), None);
                                editor.set_block(
                                    AIR,
                                    x,
                                    ground_level + 1,
                                    z,
                                    Some(&[GRASS, WHEAT, CARROTS, POTATOES, SNOW_LAYER]),
                                    None,
                                );
                            } else {
                                editor.set_block(DIRT, x, ground_level, z, Some(&[FARMLAND]), None);
                                editor.set_block(SNOW_LAYER, x, ground_level + 1, z, None, None);
                            }
                        } else {
                            editor.set_block(WATER, x, ground_level, z, Some(&[FARMLAND]), None);
                            editor.set_block(
                                AIR,
                                x,
                                ground_level + 1,
                                z,
                                Some(&[GRASS, WHEAT, CARROTS, POTATOES, SNOW_LAYER]),
                                None,
                            );
                        }
                    } else if args.winter {
                        editor.set_block(DIRT, x, ground_level, z, Some(&[FARMLAND]), None);
                        editor.set_block(SNOW_LAYER, x, ground_level + 1, z, None, None);
                    } else {
                        // Set the block below as farmland
                        editor.set_block(FARMLAND, x, ground_level, z, None, None);

                        // If a random condition is met, place a special object
                        if rng.gen_range(0..76) == 0 {
                            let special_choice: i32 = rng.gen_range(1..=25);
                            if special_choice <= 1 {
                                Tree::create(editor, (x, ground_level + 1, z), args.winter);
                            } else if special_choice <= 4 {
                                editor.set_block(
                                    HAY_BALE,
                                    x,
                                    ground_level + 1,
                                    z,
                                    None,
                                    Some(&[SPONGE]),
                                );
                            } else if special_choice <= 10 {
                                editor.set_block(
                                    OAK_LEAVES,
                                    x,
                                    ground_level + 1,
                                    z,
                                    None,
                                    Some(&[SPONGE]),
                                );
                            }
                        } else {
                            // Set crops only if the block below is farmland
                            if editor.check_for_block(x, ground_level, z, Some(&[FARMLAND]), None) {
                                let crop_choice = [WHEAT, CARROTS, POTATOES][rng.gen_range(0..3)];
                                editor.set_block(crop_choice, x, ground_level + 1, z, None, None);
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
                        editor.set_block(SCAFFOLDING, x + 1, ground_level + 1, z - 1, None, None);
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
                    && editor.check_for_block(
                        x,
                        ground_level,
                        z,
                        Some(&[GRASS_BLOCK, SNOW_BLOCK]),
                        None,
                    )
                {
                    editor.set_block(GRASS, x, ground_level + 1, z, None, None);
                }
            }
            "meadow" => {
                if editor.check_for_block(
                    x,
                    ground_level,
                    z,
                    Some(&[GRASS_BLOCK, SNOW_BLOCK]),
                    None,
                ) {
                    let random_choice: i32 = rng.gen_range(0..1001);
                    if random_choice < 5 {
                        Tree::create(editor, (x, ground_level + 1, z), args.winter);
                    } else if random_choice < 800 {
                        editor.set_block(GRASS, x, ground_level + 1, z, None, None);
                    }
                }
            }
            "quarry" => {
                // Add stone layer under it
                editor.set_block(STONE, x, ground_level - 1, z, Some(&[STONE]), None);
                editor.set_block(STONE, x, ground_level - 2, z, Some(&[STONE]), None);
                // Generate ore blocks
                if let Some(resource) = element.tags.get("resource") {
                    let ore_block = match resource.as_str() {
                        "iron_ore" => IRON_ORE,
                        "coal" => COAL_ORE,
                        "copper" => COPPER_ORE,
                        "gold" => GOLD_ORE,
                        "clay" | "kaolinite" => CLAY,
                        _ => STONE,
                    };
                    let random_choice: i32 = rng.gen_range(0..100 + ground_level); // With more depth there's more resources
                    if random_choice < 5 {
                        editor.set_block(ore_block, x, ground_level, z, Some(&[STONE]), None);
                    }
                }
            }
            _ => {}
        }
    }
}
