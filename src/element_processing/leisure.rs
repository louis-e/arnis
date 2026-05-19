use std::collections::HashSet;

use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZPoint;
use crate::deterministic_rng::element_rng;
use crate::element_processing::surfaces::get_blocks_for_surface;
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
        let min_coords = editor.get_min_coords();

        // Determine block type based on leisure type
        let mut block_type: Block = match leisure_type.as_str() {
            "park" | "nature_reserve" | "garden" | "disc_golf_course" | "golf_course" => {
                GRASS_BLOCK
            }
            "schoolyard" => BLACK_CONCRETE,
            "playground" | "recreation_ground" | "pitch" | "beach_resort" | "dog_park" => {
                GREEN_STAINED_HARDENED_CLAY
            }
            "swimming_pool" | "swimming_area" => WATER,
            "bathing_place" => SMOOTH_SANDSTONE,
            "outdoor_seating" => SMOOTH_STONE,
            "water_park" | "slipway" => LIGHT_GRAY_CONCRETE,
            "ice_rink" => PACKED_ICE,
            _ => GRASS_BLOCK,
        };

        // Explicit surface=* overrides the category default. Leave
        // `block_type` untouched for unknown surface values so existing
        // behaviour is preserved.
        if let Some(surface) = element.tags.get("surface") {
            if let Some(blocks) = get_blocks_for_surface(surface) {
                block_type = blocks[0];
            }
        }

        // Process leisure area nodes
        let filled_area = flood_fill_cache.get_or_compute(element, args.timeout.as_ref());

        let mut all_points = HashSet::new();

        let mut previous_node: Option<(i32, i32)> = None;
        let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
        for node in &element.nodes {
            if let Some(prev) = previous_node {
                // Draw a line between the current and previous node
                let bresenham_points: Vec<(i32, i32, i32)> =
                    bresenham_line(prev.0, 0, prev.1, node.x, 0, node.z);
                for (bx, _, bz) in bresenham_points {
                    all_points.insert((bx, bz));
                }
            }
            previous_node = Some((node.x, node.z));
        }

        for &(x, z) in filled_area.iter() {
            all_points.insert((x, z));
        }

        if all_points.is_empty() {
            return;
        }

        let mut total_height: i64 = 0;
        let mut count: i64 = 0;
        for &(x, z) in &all_points {
            let p = XZPoint::new(x - min_coords.0, z - min_coords.1);
            let h = if let Some(ground) = editor.get_ground() {
                ground.level(p)
            } else {
                args.ground_level
            };
            total_height += h as i64;
            count += 1;
        }
        let start_y_abs = (total_height / count) as i32;

        let mut previous_node: Option<(i32, i32)> = None;
        for node in &element.nodes {
            if let Some(prev) = previous_node {
                let bresenham_points: Vec<(i32, i32, i32)> =
                    bresenham_line(prev.0, start_y_abs, prev.1, node.x, start_y_abs, node.z);

                for (bx, _, bz) in bresenham_points {
                    let local_ground_abs = if let Some(ground) = editor.get_ground() {
                        ground.level(XZPoint::new(bx - min_coords.0, bz - min_coords.1))
                    } else {
                        args.ground_level
                    };

                    if local_ground_abs < start_y_abs {
                        for y_abs in local_ground_abs..start_y_abs {
                            editor.set_block_absolute(STONE, bx, y_abs, bz, None, None);
                        }
                    }

                    editor.set_block_absolute(block_type, bx, start_y_abs, bz, None, None);
                    editor.register_surface_y(bx, bz, start_y_abs);
                }

                corner_addup.0 += node.x;
                corner_addup.1 += node.z;
                corner_addup.2 += 1;
            }
            previous_node = Some((node.x, node.z));
        }

        // Flood-fill the interior of the leisure area using cache
        if corner_addup != (0, 0, 0) {
            // Use deterministic RNG seeded by element ID for consistent results across region boundaries
            let mut rng = element_rng(element.id);

            for &(x, z) in filled_area.iter() {
                let local_ground_abs = if let Some(ground) = editor.get_ground() {
                    ground.level(XZPoint::new(x - min_coords.0, z - min_coords.1))
                } else {
                    args.ground_level
                };

                if local_ground_abs < start_y_abs {
                    for y_abs in local_ground_abs..start_y_abs {
                        editor.set_block_absolute(STONE, x, y_abs, z, None, None);
                    }
                }

                editor.set_block_absolute(block_type, x, start_y_abs, z, None, None);
                editor.register_surface_y(x, z, start_y_abs);

                if matches!(leisure_type.as_str(), "park" | "garden" | "nature_reserve")
                    && block_type == GRASS_BLOCK
                {
                    let random_choice: i32 = rng.random_range(0..1000);

                    match random_choice {
                        0..30 => {
                            let plant_choice = match random_choice {
                                0..5 => RED_FLOWER,
                                5..10 => YELLOW_FLOWER,
                                10..16 => BLUE_FLOWER,
                                16..22 => WHITE_FLOWER,
                                22..30 => FERN,
                                _ => unreachable!(),
                            };
                            editor.set_block_absolute(
                                plant_choice,
                                x,
                                start_y_abs + 1,
                                z,
                                None,
                                None,
                            );
                        }
                        30..90 => {
                            editor.set_block_absolute(GRASS, x, start_y_abs + 1, z, None, None);
                        }
                        90..105 => {
                            editor.set_block_absolute(
                                OAK_LEAVES,
                                x,
                                start_y_abs + 1,
                                z,
                                None,
                                None,
                            );
                        }
                        105..120 => {
                            Tree::create(
                                editor,
                                (x, start_y_abs + 1, z),
                                Some(building_footprints),
                            );
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
                            for y_off in 1..=3 {
                                editor.set_block_absolute(
                                    OAK_FENCE,
                                    x - 1,
                                    start_y_abs + y_off,
                                    z,
                                    None,
                                    None,
                                );
                                editor.set_block_absolute(
                                    OAK_FENCE,
                                    x + 1,
                                    start_y_abs + y_off,
                                    z,
                                    None,
                                    None,
                                );
                            }
                            editor.set_block_absolute(
                                OAK_PLANKS,
                                x - 1,
                                start_y_abs + 4,
                                z,
                                None,
                                None,
                            );
                            editor.set_block_absolute(OAK_SLAB, x, start_y_abs + 4, z, None, None);
                            editor.set_block_absolute(
                                OAK_PLANKS,
                                x + 1,
                                start_y_abs + 4,
                                z,
                                None,
                                None,
                            );
                            editor.set_block_absolute(
                                STONE_BLOCK_SLAB,
                                x,
                                start_y_abs + 2,
                                z,
                                None,
                                None,
                            );
                        }
                        10..20 => {
                            // Slide
                            editor.set_block_absolute(OAK_SLAB, x, start_y_abs + 1, z, None, None);
                            editor.set_block_absolute(
                                OAK_SLAB,
                                x + 1,
                                start_y_abs + 2,
                                z,
                                None,
                                None,
                            );
                            editor.set_block_absolute(
                                OAK_SLAB,
                                x + 2,
                                start_y_abs + 3,
                                z,
                                None,
                                None,
                            );

                            editor.set_block_absolute(
                                OAK_PLANKS,
                                x + 2,
                                start_y_abs + 2,
                                z,
                                None,
                                None,
                            );
                            editor.set_block_absolute(
                                OAK_PLANKS,
                                x + 2,
                                start_y_abs + 1,
                                z,
                                None,
                                None,
                            );

                            editor.set_block_absolute(
                                LADDER,
                                x + 2,
                                start_y_abs + 2,
                                z - 1,
                                None,
                                None,
                            );
                            editor.set_block_absolute(
                                LADDER,
                                x + 2,
                                start_y_abs + 1,
                                z - 1,
                                None,
                                None,
                            );
                        }
                        20..30 => {
                            // Sandpit
                            for dx in -3..=3 {
                                for dz in -3..=3 {
                                    editor.set_block_absolute(
                                        SAND,
                                        x + dx,
                                        start_y_abs,
                                        z + dz,
                                        None,
                                        None,
                                    );
                                }
                            }
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
