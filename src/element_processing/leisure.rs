use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::deterministic_rng::element_rng;
use crate::element_processing::tree::Tree;
use crate::floodfill_cache::{BuildingFootprintBitmap, FloodFillCache};
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::Rng;

pub fn generate_leisure(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
) {
    if let Some(leisure_type) = element.tags.get("leisure") {
        let mut previous_node: Option<(i32, i32)> = None;
        let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
        let mut current_leisure: Vec<(i32, i32)> = vec![];

        // Determine block type based on leisure type
        let block_type: Block = match leisure_type.as_str() {
            "park" | "nature_reserve" | "garden" | "disc_golf_course" | "golf_course" => {
                GRASS_BLOCK
            }
            "schoolyard" => BLACK_CONCRETE,
            "playground" | "recreation_ground" | "pitch" | "beach_resort" | "dog_park" => {
                if let Some(surface) = element.tags.get("surface") {
                    match surface.as_str() {
                        "clay" => TERRACOTTA,
                        "sand" => SAND,
                        "tartan" => RED_TERRACOTTA,
                        "grass" => GRASS_BLOCK,
                        "dirt" => DIRT,
                        "pebblestone" | "cobblestone" | "unhewn_cobblestone" => COBBLESTONE,
                        _ => GREEN_STAINED_HARDENED_CLAY,
                    }
                } else {
                    GREEN_STAINED_HARDENED_CLAY
                }
            }
            "swimming_pool" | "swimming_area" => WATER, //Swimming area: Area in a larger body of water for swimming
            "bathing_place" => SMOOTH_SANDSTONE,        // Could be sand or concrete
            "outdoor_seating" => SMOOTH_STONE,          //Usually stone or stone bricks
            "water_park" | "slipway" => LIGHT_GRAY_CONCRETE, // Water park area, not the pool. Usually is concrete
            "ice_rink" => PACKED_ICE, // TODO: Ice for Ice Rink, needs building defined
            _ => GRASS_BLOCK,
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
                        0,
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

        // Flood-fill the interior of the leisure area using cache
        if corner_addup != (0, 0, 0) {
            let filled_area: Vec<(i32, i32)> =
                flood_fill_cache.get_or_compute(element, args.timeout.as_ref());

            // Use deterministic RNG seeded by element ID for consistent results across region boundaries
            let mut rng = element_rng(element.id);

            for (x, z) in filled_area {
                editor.set_block(block_type, x, 0, z, Some(&[GRASS_BLOCK]), None);

                // Add decorative elements for parks and gardens
                if matches!(leisure_type.as_str(), "park" | "garden" | "nature_reserve")
                    && editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK]))
                {
                    let random_choice: i32 = rng.random_range(0..1000);

                    match random_choice {
                        0..30 => {
                            // Plants
                            let plant_choice = match random_choice {
                                0..5 => RED_FLOWER,
                                5..10 => YELLOW_FLOWER,
                                10..16 => BLUE_FLOWER,
                                16..22 => WHITE_FLOWER,
                                22..30 => FERN,
                                _ => unreachable!(),
                            };
                            editor.set_block(plant_choice, x, 1, z, None, None);
                        }
                        30..90 => {
                            // Grass
                            editor.set_block(GRASS, x, 1, z, None, None);
                        }
                        90..105 => {
                            // Oak leaves
                            editor.set_block(OAK_LEAVES, x, 1, z, None, None);
                        }
                        105..120 => {
                            // Tree
                            Tree::create(editor, (x, 1, z), Some(building_footprints));
                        }
                        _ => {}
                    }
                }

                // Add playground or recreation ground features
                if matches!(leisure_type.as_str(), "playground" | "recreation_ground") {
                    let random_choice: i32 = rng.random_range(0..5000);

                    match random_choice {
                        0..10 => {
                            // Swing set
                            for y in 1..=3 {
                                editor.set_block(OAK_FENCE, x - 1, y, z, None, None);
                                editor.set_block(OAK_FENCE, x + 1, y, z, None, None);
                            }
                            editor.set_block(OAK_PLANKS, x - 1, 4, z, None, None);
                            editor.set_block(OAK_SLAB, x, 4, z, None, None);
                            editor.set_block(OAK_PLANKS, x + 1, 4, z, None, None);
                            editor.set_block(STONE_BLOCK_SLAB, x, 2, z, None, None);
                        }
                        10..20 => {
                            // Slide
                            editor.set_block(OAK_SLAB, x, 1, z, None, None);
                            editor.set_block(OAK_SLAB, x + 1, 2, z, None, None);
                            editor.set_block(OAK_SLAB, x + 2, 3, z, None, None);

                            editor.set_block(OAK_PLANKS, x + 2, 2, z, None, None);
                            editor.set_block(OAK_PLANKS, x + 2, 1, z, None, None);

                            editor.set_block(LADDER, x + 2, 2, z - 1, None, None);
                            editor.set_block(LADDER, x + 2, 1, z - 1, None, None);
                        }
                        20..30 => {
                            // Sandpit
                            editor.fill_blocks(
                                SAND,
                                x - 3,
                                0,
                                z - 3,
                                x + 3,
                                0,
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

pub fn generate_leisure_from_relation(
    editor: &mut WorldEditor,
    rel: &ProcessedRelation,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
) {
    if rel.tags.get("leisure") == Some(&"park".to_string()) {
        // Process each outer member way individually using cached flood fill.
        // We intentionally do not combine all outer nodes into one mega-way,
        // because that creates a nonsensical polygon spanning the whole relation
        // extent, misses the flood fill cache, and can cause multi-GB allocations.
        for member in &rel.members {
            if member.role == ProcessedMemberRole::Outer {
                // Use relation tags so the member inherits the relation's leisure=* type
                let way_with_rel_tags = ProcessedWay {
                    id: member.way.id,
                    nodes: member.way.nodes.clone(),
                    tags: rel.tags.clone(),
                };
                generate_leisure(
                    editor,
                    &way_with_rel_tags,
                    args,
                    flood_fill_cache,
                    building_footprints,
                );
            }
        }
    }
}
