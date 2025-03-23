use crate::args::Args;
use crate::block_definitions::BLOCKS;
use crate::cartesian::XZPoint;
use crate::data_processing::MIN_Y;
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
                &*BLOCKS.by_name("snow_block").unwrap()
            } else {
                &*BLOCKS.by_name("grass_block").unwrap()
            }
        }
        "farmland" => &*BLOCKS.by_name("farmland").unwrap(),
        "forest" => {
            if args.winter {
                &*BLOCKS.by_name("snow_block").unwrap()
            } else {
                &*BLOCKS.by_name("grass_block").unwrap()
            }
        }
        "cemetery" => &*BLOCKS.by_name("podzol").unwrap(),
        "beach" => &*BLOCKS.by_name("sand").unwrap(),
        "construction" => &*BLOCKS.by_name("dirt").unwrap(),
        "traffic_island" => &*BLOCKS.by_name("stone_block_slab").unwrap(),
        "residential" => {
            let residential_tag = element.tags.get("residential").unwrap_or(&binding);
            if residential_tag == "rural" {
                if args.winter {
                    &*BLOCKS.by_name("snow_block").unwrap()
                } else {
                    &*BLOCKS.by_name("grass_block").unwrap()
                }
            } else {
                &*BLOCKS.by_name("stone_bricks").unwrap()
            }
        }
        "commercial" => &*BLOCKS.by_name("smooth_stone").unwrap(),
        "education" => &*BLOCKS.by_name("light_gray_concrete").unwrap(),
        "industrial" => &*BLOCKS.by_name("cobblestone").unwrap(),
        "military" => &*BLOCKS.by_name("gray_concrete").unwrap(),
        "railway" => &*BLOCKS.by_name("gravel").unwrap(),
        "landfill" => {
            // Gravel if man_made = spoil_heap or heap, coarse dirt else
            let manmade = element.tags.get("man_made").unwrap_or(&binding);
            if manmade_tag == "spoil_heap" || manmade_tag == "heap" {
                &*BLOCKS.by_name("gravel").unwrap()
            } else {
                &*BLOCKS.by_name("coarse_dirt").unwrap()
            }
        }
        "quarry" => &*BLOCKS.by_name("stone").unwrap(),
        _ => {
            if args.winter {
                &*BLOCKS.by_name("snow_block").unwrap()
            } else {
                &*BLOCKS.by_name("grass_block").unwrap()
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
            editor.set_block(block_type, x, ground_level, z, None, Some(&[&*BLOCKS.by_name("sponge").unwrap()]));
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
                        if editor.check_for_block(x, ground_level, z, Some(&[&*BLOCKS.by_name("podzol").unwrap()]), None) {
                            if rng.gen_bool(0.5) {
                                editor.set_block(
                                    &*BLOCKS.by_name("cobblestone").unwrap(),
                                    x - 1,
                                    ground_level + 1,
                                    z,
                                    None,
                                    None,
                                );
                                editor.set_block(
                                    &*BLOCKS.by_name("stone_brick_slab").unwrap(),
                                    x - 1,
                                    ground_level + 2,
                                    z,
                                    None,
                                    None,
                                );
                                editor.set_block(
                                    &*BLOCKS.by_name("stone_brick_slab").unwrap(),
                                    x,
                                    ground_level + 1,
                                    z,
                                    None,
                                    None,
                                );
                                editor.set_block(
                                    &*BLOCKS.by_name("stone_brick_slab").unwrap(),
                                    x + 1,
                                    ground_level + 1,
                                    z,
                                    None,
                                    None,
                                );
                            } else {
                                editor.set_block(
                                    &*BLOCKS.by_name("cobblestone").unwrap(),
                                    x,
                                    ground_level + 1,
                                    z - 1,
                                    None,
                                    None,
                                );
                                editor.set_block(
                                    &*BLOCKS.by_name("stone_brick_slab").unwrap(),
                                    x,
                                    ground_level + 2,
                                    z - 1,
                                    None,
                                    None,
                                );
                                editor.set_block(
                                    &*BLOCKS.by_name("stone_brick_slab").unwrap(),
                                    x,
                                    ground_level + 1,
                                    z,
                                    None,
                                    None,
                                );
                                editor.set_block(
                                    &*BLOCKS.by_name("stone_brick_slab").unwrap(),
                                    x,
                                    ground_level + 1,
                                    z + 1,
                                    None,
                                    None,
                                );
                            }
                        }
                    } else if random_choice < 30 {
                        if editor.check_for_block(x, ground_level, z, Some(&[&*BLOCKS.by_name("podzol").unwrap()]), None) {
                            editor.set_block(&*BLOCKS.by_name("red_flower").unwrap(), x, ground_level + 1, z, None, None);
                        }
                    } else if random_choice < 33 {
                        Tree::create(editor, (x, ground_level + 1, z), args.winter);
                    }
                }
            }
            "forest" => {
                if !editor.check_for_block(x, ground_level, z, None, Some(&[&*BLOCKS.by_name("water").unwrap()])) {
                    let random_choice: i32 = rng.gen_range(0..21);
                    if random_choice == 20 {
                        Tree::create(editor, (x, ground_level + 1, z), args.winter);
                    } else if random_choice == 2 {
                        let flower_block = match rng.gen_range(1..=4) {
                            1 => &*BLOCKS.by_name("red_flower").unwrap(),
                            2 => &*BLOCKS.by_name("blue_flower").unwrap(),
                            3 => &*BLOCKS.by_name("yellow_flower").unwrap(),
                            _ => &*BLOCKS.by_name("white_flower").unwrap(),
                        };
                        editor.set_block(flower_block, x, ground_level + 1, z, None, None);
                    } else if random_choice <= 1 {
                        editor.set_block(&*BLOCKS.by_name("grass").unwrap(), x, ground_level + 1, z, None, None);
                    }
                }
            }
            "farmland" => {
                // Check if the current block is not water or another undesired block
                if !editor.check_for_block(x, ground_level, z, None, Some(&[&*BLOCKS.by_name("water").unwrap()])) {
                    if x % 15 == 0 || z % 15 == 0 {
                        // Place water on the edges
                        editor.set_block(&*BLOCKS.by_name("water").unwrap(), x, ground_level, z, Some(&[&*BLOCKS.by_name("farmland").unwrap()]), None);
                        editor.set_block(
                            &*BLOCKS.by_name("air").unwrap(),
                            x,
                            ground_level + 1,
                            z,
                            Some(&[&*BLOCKS.by_name("grass").unwrap(), &*BLOCKS.by_name("wheat").unwrap(), &*BLOCKS.by_name("carrots").unwrap(), &*BLOCKS.by_name("potatoes").unwrap()]),
                            None,
                        );
                    } else {
                        // Set the block below as farmland
                        editor.set_block(&*BLOCKS.by_name("farmland").unwrap(), x, ground_level, z, None, None);

                        // If a random condition is met, place a special object
                        if rng.gen_range(0..76) == 0 {
                            let special_choice: i32 = rng.gen_range(1..=10);
                            if special_choice <= 2 {
                                Tree::create(editor, (x, ground_level + 1, z), args.winter);
                            } else if special_choice <= 6 {
                                editor.set_block(
                                    &*BLOCKS.by_name("hay_bale").unwrap(),
                                    x,
                                    ground_level + 1,
                                    z,
                                    None,
                                    Some(&[&*BLOCKS.by_name("sponge").unwrap()]),
                                );
                            } else {
                                editor.set_block(
                                    &*BLOCKS.by_name("oak_leaves").unwrap(),
                                    x,
                                    ground_level + 1,
                                    z,
                                    None,
                                    Some(&[&*BLOCKS.by_name("sponge").unwrap()]),
                                );
                            }
                        } else {
                            // Set crops only if the block below is farmland
                            if editor.check_for_block(x, ground_level, z, Some(&[&*BLOCKS.by_name("farmland").unwrap()]), None) {
                                let crop_choice = [&*BLOCKS.by_name("wheat").unwrap(), &*BLOCKS.by_name("carrots").unwrap(), &*BLOCKS.by_name("potatoes").unwrap()][rng.gen_range(0..3)];
                                editor.set_block(crop_choice, x, ground_level + 1, z, None, None);
                            }
                        }
                    }
                }
            }
            "construction" => {
                let random_choice: i32 = rng.gen_range(0..1501);
                if random_choice < 6 {
                    editor.set_block(&*BLOCKS.by_name("scaffolding").unwrap(), x, ground_level + 1, z, None, None);
                    if random_choice < 2 {
                        editor.set_block(&*BLOCKS.by_name("scaffolding").unwrap(), x, ground_level + 2, z, None, None);
                        editor.set_block(&*BLOCKS.by_name("scaffolding").unwrap(), x, ground_level + 3, z, None, None);
                    } else if random_choice < 4 {
                        editor.set_block(&*BLOCKS.by_name("scaffolding").unwrap(), x, ground_level + 2, z, None, None);
                        editor.set_block(&*BLOCKS.by_name("scaffolding").unwrap(), x, ground_level + 3, z, None, None);
                        editor.set_block(&*BLOCKS.by_name("scaffolding").unwrap(), x, ground_level + 4, z, None, None);
                        editor.set_block(&*BLOCKS.by_name("scaffolding").unwrap(), x, ground_level + 1, z + 1, None, None);
                    } else {
                        editor.set_block(&*BLOCKS.by_name("scaffolding").unwrap(), x, ground_level + 2, z, None, None);
                        editor.set_block(&*BLOCKS.by_name("scaffolding").unwrap(), x, ground_level + 3, z, None, None);
                        editor.set_block(&*BLOCKS.by_name("scaffolding").unwrap(), x, ground_level + 4, z, None, None);
                        editor.set_block(&*BLOCKS.by_name("scaffolding").unwrap(), x, ground_level + 5, z, None, None);
                        editor.set_block(&*BLOCKS.by_name("scaffolding").unwrap(), x - 1, ground_level + 1, z, None, None);
                        editor.set_block(&*BLOCKS.by_name("scaffolding").unwrap(), x + 1, ground_level + 1, z - 1, None, None);
                    }
                } else if random_choice < 30 {
                    let construction_items = [
                        &*BLOCKS.by_name("oak_log").unwrap(),
                        &*BLOCKS.by_name("cobblestone").unwrap(),
                        &*BLOCKS.by_name("gravel").unwrap(),
                        &*BLOCKS.by_name("glowstone").unwrap(),
                        &*BLOCKS.by_name("stone").unwrap(),
                        &*BLOCKS.by_name("cobblestone_wall").unwrap(),
                        &*BLOCKS.by_name("black_concrete").unwrap(),
                        &*BLOCKS.by_name("sand").unwrap(),
                        &*BLOCKS.by_name("oak_planks").unwrap(),
                        &*BLOCKS.by_name("dirt").unwrap(),
                        &*BLOCKS.by_name("brick").unwrap(),
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
                        editor.set_block(&*BLOCKS.by_name("dirt").unwrap(), x, ground_level + 1, z, None, None);
                        editor.set_block(&*BLOCKS.by_name("dirt").unwrap(), x, ground_level + 2, z, None, None);
                        editor.set_block(&*BLOCKS.by_name("dirt").unwrap(), x + 1, ground_level + 1, z, None, None);
                        editor.set_block(&*BLOCKS.by_name("dirt").unwrap(), x, ground_level + 1, z + 1, None, None);
                    } else {
                        editor.set_block(&*BLOCKS.by_name("dirt").unwrap(), x, ground_level + 1, z, None, None);
                        editor.set_block(&*BLOCKS.by_name("dirt").unwrap(), x, ground_level + 2, z, None, None);
                        editor.set_block(&*BLOCKS.by_name("dirt").unwrap(), x - 1, ground_level + 1, z, None, None);
                        editor.set_block(&*BLOCKS.by_name("dirt").unwrap(), x, ground_level + 1, z - 1, None, None);
                    }
                } else if random_choice < 150 {
                    editor.set_block(&*BLOCKS.by_name("air").unwrap(), x, ground_level, z, None, Some(&[&*BLOCKS.by_name("sponge").unwrap()]));
                }
            }
            "grass" => {
                if rng.gen_range(1..=7) != 1
                    && editor.check_for_block(
                        x,
                        ground_level,
                        z,
                        Some(&[&*BLOCKS.by_name("grass_block").unwrap(), &*BLOCKS.by_name("snow_block").unwrap()]),
                        None,
                    )
                {
                    editor.set_block(&*BLOCKS.by_name("grass").unwrap(), x, ground_level + 1, z, None, None);
                }
            }
            "meadow" => {
                if editor.check_for_block(
                    x,
                    ground_level,
                    z,
                    Some(&[&*BLOCKS.by_name("grass_block").unwrap(), &*BLOCKS.by_name("snow_block").unwrap()]),
                    None,
                ) {
                    let random_choice: i32 = rng.gen_range(0..1001);
                    if random_choice < 5 {
                        Tree::create(editor, (x, ground_level + 1, z), args.winter);
                    } else if random_choice < 800 {
                        editor.set_block(&*BLOCKS.by_name("grass").unwrap(), x, ground_level + 1, z, None, None);
                    }
                }
            }
            "quarry" => {
                if let Some(resource) = element.tags.get("resource") {
                    let ore_block = match resource.as_str() {
                        "iron_ore" => IRON_ORE,
                        "coal" => COAL_ORE,
                        "copper" => COPPER_ORE,
                        "gold" => GOLD_ORE,
                        "clay" | "kaolinite" => CLAY,
                        _ => STONE,
                    };
                    let random_choice: i32 = rng.gen_range(0..100 + ground_level); // with more depth there's more resources
                    if random_choice < 5 {
                        editor.set_block(ore_block, x, ground_level, z, Some(&[STONE]), None);
                    }
                    // Fill everything with stone so dirt won't be there
                    if args.fillground {
                        editor.fill_blocks(STONE, x, MIN_Y + 1, z, x, ground_level, z, None, None);
                    }
                }
            }
            _ => {}
        }
    }
}
