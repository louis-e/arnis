use crate::args::Args;
use crate::block_definitions::*;
use crate::element_processing::tree::Tree;
use crate::floodfill::flood_fill_area;
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::Rng;

pub fn generate_landuse(editor: &mut WorldEditor, element: &ProcessedWay, args: &Args) {
    // Determine block type based on landuse tag
    let binding: String = "".to_string();
    let landuse_tag: &String = element.tags.get("landuse").unwrap_or(&binding);

    let block_type = match landuse_tag.as_str() {
        "greenfield" | "meadow" | "grass" | "orchard" | "forest" => GRASS_BLOCK,
        "farmland" => FARMLAND,
        "cemetery" => PODZOL,
        "construction" => COARSE_DIRT,
        "traffic_island" => STONE_BLOCK_SLAB,
        "residential" => {
            let residential_tag = element.tags.get("residential").unwrap_or(&binding);
            if residential_tag == "rural" {
                GRASS_BLOCK
            } else {
                STONE_BRICKS
            }
        }
        "commercial" => SMOOTH_STONE,
        "education" => POLISHED_ANDESITE,
        "religious" => POLISHED_ANDESITE,
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
        _ => GRASS_BLOCK,
    };

    // Get the area of the landuse element
    let polygon_coords: Vec<(i32, i32)> = element.nodes.iter().map(|n| (n.x, n.z)).collect();
    let floor_area: Vec<(i32, i32)> = flood_fill_area(&polygon_coords, args.timeout.as_ref());

    let mut rng: rand::prelude::ThreadRng = rand::thread_rng();

    for (x, z) in floor_area {
        if landuse_tag == "traffic_island" {
            editor.set_block(block_type, x, 1, z, None, None);
        } else if landuse_tag == "construction" || landuse_tag == "railway" {
            editor.set_block(block_type, x, 0, z, None, Some(&[SPONGE]));
        } else {
            editor.set_block(block_type, x, 0, z, None, None);
        }

        // Add specific features for different landuse types
        match landuse_tag.as_str() {
            "cemetery" => {
                if (x % 3 == 0) && (z % 3 == 0) {
                    let random_choice: i32 = rng.gen_range(0..100);
                    if random_choice < 15 {
                        // Place graves
                        if editor.check_for_block(x, 0, z, Some(&[PODZOL])) {
                            if rng.gen_bool(0.5) {
                                editor.set_block(COBBLESTONE, x - 1, 1, z, None, None);
                                editor.set_block(STONE_BRICK_SLAB, x - 1, 2, z, None, None);
                                editor.set_block(STONE_BRICK_SLAB, x, 1, z, None, None);
                                editor.set_block(STONE_BRICK_SLAB, x + 1, 1, z, None, None);
                            } else {
                                editor.set_block(COBBLESTONE, x, 1, z - 1, None, None);
                                editor.set_block(STONE_BRICK_SLAB, x, 2, z - 1, None, None);
                                editor.set_block(STONE_BRICK_SLAB, x, 1, z, None, None);
                                editor.set_block(STONE_BRICK_SLAB, x, 1, z + 1, None, None);
                            }
                        }
                    } else if random_choice < 30 {
                        if editor.check_for_block(x, 0, z, Some(&[PODZOL])) {
                            editor.set_block(RED_FLOWER, x, 1, z, None, None);
                        }
                    } else if random_choice < 33 {
                        Tree::create(editor, (x, 1, z));
                    } else if random_choice < 35 {
                        editor.set_block(OAK_LEAVES, x, 1, z, None, None);
                    }
                }
            }
            "forest" => {
                if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                    let random_choice: i32 = rng.gen_range(0..30);
                    if random_choice == 20 {
                        Tree::create(editor, (x, 1, z));
                    } else if random_choice == 2 {
                        let flower_block: Block = match rng.gen_range(1..=5) {
                            1 => OAK_LEAVES,
                            2 => RED_FLOWER,
                            3 => BLUE_FLOWER,
                            4 => YELLOW_FLOWER,
                            _ => WHITE_FLOWER,
                        };
                        editor.set_block(flower_block, x, 1, z, None, None);
                    } else if random_choice <= 12 {
                        editor.set_block(GRASS, x, 1, z, None, None);
                    }
                }
            }
            "farmland" => {
                // Check if the current block is not water or another undesired block
                if !editor.check_for_block(x, 0, z, Some(&[WATER])) {
                    if x % 9 == 0 && z % 9 == 0 {
                        // Place water in dot pattern
                        editor.set_block(WATER, x, 0, z, Some(&[FARMLAND]), None);
                    } else if rng.gen_range(0..76) == 0 {
                        let special_choice: i32 = rng.gen_range(1..=10);
                        if special_choice <= 4 {
                            editor.set_block(HAY_BALE, x, 1, z, None, Some(&[SPONGE]));
                        } else {
                            editor.set_block(OAK_LEAVES, x, 1, z, None, Some(&[SPONGE]));
                        }
                    } else {
                        // Set crops only if the block below is farmland
                        if editor.check_for_block(x, 0, z, Some(&[FARMLAND])) {
                            let crop_choice = [WHEAT, CARROTS, POTATOES][rng.gen_range(0..3)];
                            editor.set_block(crop_choice, x, 1, z, None, None);
                        }
                    }
                }
            }
            "construction" => {
                let random_choice: i32 = rng.gen_range(0..1501);
                if random_choice < 15 {
                    editor.set_block(SCAFFOLDING, x, 1, z, None, None);
                    if random_choice < 2 {
                        editor.set_block(SCAFFOLDING, x, 2, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 3, z, None, None);
                    } else if random_choice < 4 {
                        editor.set_block(SCAFFOLDING, x, 2, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 3, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 4, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 1, z + 1, None, None);
                    } else {
                        editor.set_block(SCAFFOLDING, x, 2, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 3, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 4, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 5, z, None, None);
                        editor.set_block(SCAFFOLDING, x - 1, 1, z, None, None);
                        editor.set_block(SCAFFOLDING, x + 1, 1, z - 1, None, None);
                    }
                } else if random_choice < 55 {
                    let construction_items: [Block; 13] = [
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
                        CRAFTING_TABLE,
                        FURNACE,
                    ];
                    editor.set_block(
                        construction_items[rng.gen_range(0..construction_items.len())],
                        x,
                        1,
                        z,
                        None,
                        None,
                    );
                } else if random_choice < 65 {
                    if random_choice < 60 {
                        editor.set_block(DIRT, x, 1, z, None, None);
                        editor.set_block(DIRT, x, 2, z, None, None);
                        editor.set_block(DIRT, x + 1, 1, z, None, None);
                        editor.set_block(DIRT, x, 1, z + 1, None, None);
                    } else {
                        editor.set_block(DIRT, x, 1, z, None, None);
                        editor.set_block(DIRT, x, 2, z, None, None);
                        editor.set_block(DIRT, x - 1, 1, z, None, None);
                        editor.set_block(DIRT, x, 1, z - 1, None, None);
                    }
                } else if random_choice < 100 {
                    editor.set_block(GRAVEL, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 115 {
                    editor.set_block(SAND, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 125 {
                    editor.set_block(DIORITE, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 145 {
                    editor.set_block(BRICK, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 155 {
                    editor.set_block(GRANITE, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 180 {
                    editor.set_block(ANDESITE, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 565 {
                    editor.set_block(COBBLESTONE, x, 0, z, None, Some(&[SPONGE]));
                }
            }
            "grass" => {
                if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                    match rng.gen_range(0..200) {
                        0 => editor.set_block(OAK_LEAVES, x, 1, z, None, None),
                        1..=170 => editor.set_block(GRASS, x, 1, z, None, None),
                        _ => {}
                    }
                }
            }
            "greenfield" => {
                if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                    match rng.gen_range(0..200) {
                        0 => editor.set_block(OAK_LEAVES, x, 1, z, None, None),
                        1..=17 => editor.set_block(GRASS, x, 1, z, None, None),
                        _ => {}
                    }
                }
            }
            "meadow" => {
                if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                    let random_choice: i32 = rng.gen_range(0..1001);
                    if random_choice < 5 {
                        Tree::create(editor, (x, 1, z));
                    } else if random_choice < 6 {
                        editor.set_block(RED_FLOWER, x, 1, z, None, None);
                    } else if random_choice < 9 {
                        editor.set_block(OAK_LEAVES, x, 1, z, None, None);
                    } else if random_choice < 800 {
                        editor.set_block(GRASS, x, 1, z, None, None);
                    }
                }
            }
            "orchard" => {
                if x % 18 == 0 && z % 10 == 0 {
                    Tree::create(editor, (x, 1, z));
                } else if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                    match rng.gen_range(0..100) {
                        0 => editor.set_block(OAK_LEAVES, x, 1, z, None, None),
                        1..=20 => editor.set_block(GRASS, x, 1, z, None, None),
                        _ => {}
                    }
                }
            }
            "quarry" => {
                // Add stone layer under it
                editor.set_block(STONE, x, -1, z, Some(&[STONE]), None);
                editor.set_block(STONE, x, -2, z, Some(&[STONE]), None);
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
                    let random_choice: i32 = rng.gen_range(0..100 + editor.get_absolute_y(x, 0, z)); // The deeper it is the more resources are there
                    if random_choice < 5 {
                        editor.set_block(ore_block, x, 0, z, Some(&[STONE]), None);
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn generate_landuse_from_relation(
    editor: &mut WorldEditor,
    rel: &ProcessedRelation,
    args: &Args,
) {
    if rel.tags.contains_key("landuse") {
        // Generate individual ways with their original tags
        for member in &rel.members {
            if member.role == ProcessedMemberRole::Outer {
                generate_landuse(editor, &member.way.clone(), args);
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

            // Generate landuse area from combined way
            generate_landuse(editor, &combined_way, args);
        }
    }
}
