//! Ground layer generation — surface blocks, vegetation, shorelines, and underground fill.
//!
//! This module handles the final terrain pass that runs after all OSM element
//! processing is complete. It iterates over every block in the bounding box and:
//!
//! - Selects surface and sub-surface blocks based on ESA WorldCover land cover
//!   classification and terrain slope.
//! - Blends shorelines between water and land.
//! - Places vegetation (grass, flowers, trees, crops) according to land cover class.
//! - Cleans up stray vegetation from road surfaces.
//! - Fills underground columns with stone and places a bedrock floor.
//!
//! The generation is done chunk-by-chunk for better cache locality when writing
//! to region files.

use crate::args::Args;
use crate::block_definitions::{
    AIR, ANDESITE, BEDROCK, BLACK_CONCRETE, BLUE_FLOWER, CARROTS, CLAY, COARSE_DIRT, COBBLESTONE,
    CRACKED_STONE_BRICKS, DEAD_BUSH, DIRT, DIRT_PATH, FARMLAND, GRASS, GRASS_BLOCK, GRAVEL,
    GRAY_CONCRETE, HAY_BALE, LIGHT_GRAY_CONCRETE, MUD, OAK_LEAVES, PACKED_ICE, POTATOES,
    RED_FLOWER, SAND, SANDSTONE, SMOOTH_STONE, SNOW_BLOCK, STONE, STONE_BRICKS, TALL_GRASS_BOTTOM,
    TALL_GRASS_TOP, WATER, WHEAT, WHITE_CONCRETE, WHITE_FLOWER, YELLOW_FLOWER,
};
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::element_processing::tree;
use crate::floodfill_cache::BuildingFootprintBitmap;
use crate::ground::Ground;
use crate::land_cover;
use crate::progress::emit_gui_progress_update;
use crate::world_editor::WorldEditor;
use crate::world_editor::MIN_Y;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use rand::Rng;

/// Generate the ground layer for the entire bounding box.
///
/// This must be called after all OSM element processing is complete and the
/// flood-fill / highway caches have been dropped. Regions remain in memory
/// and are saved in parallel by `save_java()` after generation completes.
pub fn generate_ground_layer(
    editor: &mut WorldEditor,
    ground: &Ground,
    args: &Args,
    xzbbox: &XZBBox,
    building_footprints: &BuildingFootprintBitmap,
) -> Result<(), String> {
    let has_land_cover = ground.has_land_cover();
    let terrain_enabled = ground.elevation_enabled;

    let total_blocks: u64 = xzbbox.bounding_rect().total_blocks();
    let desired_updates: u64 = 1500;
    let batch_size: u64 = (total_blocks / desired_updates).max(1);

    let mut block_counter: u64 = 0;

    println!("{} Generating ground...", "[6/7]".bold());
    emit_gui_progress_update(70.0, "Generating ground...");

    let ground_pb: ProgressBar = ProgressBar::new(total_blocks);
    ground_pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:45}] {pos}/{len} blocks ({eta})")
            .unwrap()
            .progress_chars("█▓░"),
    );

    let mut gui_progress_grnd: f64 = 70.0;
    let mut last_emitted_progress: f64 = gui_progress_grnd;
    let total_iterations_grnd: f64 = total_blocks as f64;
    let progress_increment_grnd: f64 = 20.0 / total_iterations_grnd;

    // Process ground generation chunk-by-chunk for better cache locality.
    // This keeps the same region/chunk HashMap entries hot in CPU cache,
    // rather than jumping between regions on every Z iteration.
    let min_chunk_x = xzbbox.min_x() >> 4;
    let max_chunk_x = xzbbox.max_x() >> 4;
    let min_chunk_z = xzbbox.min_z() >> 4;
    let max_chunk_z = xzbbox.max_z() >> 4;

    for chunk_x in min_chunk_x..=max_chunk_x {
        for chunk_z in min_chunk_z..=max_chunk_z {
            // Calculate the block range for this chunk, clamped to bbox
            let chunk_min_x = (chunk_x << 4).max(xzbbox.min_x());
            let chunk_max_x = ((chunk_x << 4) + 15).min(xzbbox.max_x());
            let chunk_min_z = (chunk_z << 4).max(xzbbox.min_z());
            let chunk_max_z = ((chunk_z << 4) + 15).min(xzbbox.max_z());

            for x in chunk_min_x..=chunk_max_x {
                for z in chunk_min_z..=chunk_max_z {
                    // Get ground level, when terrain is enabled, look it up once per block
                    // When disabled, use constant ground_level (no function call overhead)
                    let ground_y = if terrain_enabled {
                        editor.get_ground_level(x, z)
                    } else {
                        args.ground_level
                    };

                    let coord = XZPoint::new(x, z);

                    // Add default dirt and grass layer if there isn't a stone layer already
                    if !editor.check_for_block_absolute(x, ground_y, z, Some(&[STONE]), None) {
                        // Handle ESA water with variable depth as a special case
                        let is_esa_water =
                            has_land_cover && ground.cover_class(coord) == land_cover::LC_WATER;

                        if is_esa_water {
                            // Single block of water at ground level
                            editor.set_block_if_absent_absolute(WATER, x, ground_y, z);

                            // Floor: sand/gravel/clay + sandstone below
                            let h = land_cover::coord_hash(x, z);
                            let floor_block = match h % 5 {
                                0 => GRAVEL,
                                1 => CLAY,
                                _ => SAND,
                            };
                            if ground_y - 1 > MIN_Y {
                                editor.set_block_if_absent_absolute(
                                    floor_block,
                                    x,
                                    ground_y - 1,
                                    z,
                                );
                            }
                            if ground_y - 2 > MIN_Y {
                                editor.set_block_if_absent_absolute(SANDSTONE, x, ground_y - 2, z);
                            }
                        } else {
                            // Determine surface and sub-surface blocks based on available data
                            let (surface_block, under_block) = if has_land_cover {
                                // ESA WorldCover + slope-based material selection
                                let cover = ground.cover_class(coord);
                                let slope = if terrain_enabled {
                                    ground.slope(coord)
                                } else {
                                    0
                                };

                                // Steep terrain overrides land cover classification
                                if slope > 6 {
                                    (STONE, STONE)
                                } else if slope > 4 {
                                    (ANDESITE, STONE)
                                } else if slope > 3 {
                                    (GRAVEL, STONE)
                                } else {
                                    // Select surface block based on ESA land cover class
                                    match cover {
                                        land_cover::LC_TREE_COVER => (GRASS_BLOCK, DIRT),
                                        land_cover::LC_SHRUBLAND => {
                                            // Primarily grass with some coarse dirt patches
                                            let h = land_cover::coord_hash(x, z);
                                            if h.is_multiple_of(5) {
                                                (COARSE_DIRT, DIRT) // 20% coarse dirt
                                            } else {
                                                (GRASS_BLOCK, DIRT) // 80% grass
                                            }
                                        }
                                        land_cover::LC_GRASSLAND => (GRASS_BLOCK, DIRT),
                                        land_cover::LC_CROPLAND => (FARMLAND, DIRT),
                                        land_cover::LC_BUILT_UP => {
                                            let h = land_cover::coord_hash(x, z) % 100;
                                            if h < 72 {
                                                (STONE_BRICKS, STONE)
                                            } else if h < 87 {
                                                (CRACKED_STONE_BRICKS, STONE)
                                            } else if h < 92 {
                                                (STONE, STONE)
                                            } else {
                                                (COBBLESTONE, STONE)
                                            }
                                        }
                                        land_cover::LC_BARE => {
                                            // Skip isolated bare pixels (surrounded by non-bare)
                                            // to avoid random single-block patches
                                            let neighbors_bare =
                                                [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)]
                                                    .iter()
                                                    .filter(|(dx, dz)| {
                                                        ground.cover_class(XZPoint::new(
                                                            x + dx,
                                                            z + dz,
                                                        )) == land_cover::LC_BARE
                                                    })
                                                    .count();
                                            if neighbors_bare == 0 {
                                                // Isolated pixel - blend with surroundings
                                                (GRASS_BLOCK, DIRT)
                                            } else {
                                                // Bare/sparse: coarse dirt, gravel, stone mix
                                                let h = land_cover::coord_hash(x, z);
                                                match h % 10 {
                                                    0..=3 => (COARSE_DIRT, DIRT), // 40% coarse dirt
                                                    4..=5 => (GRAVEL, STONE),     // 20% gravel
                                                    6..=7 => (STONE, STONE),      // 20% stone
                                                    _ => (ANDESITE, STONE),       // 20% andesite
                                                }
                                            }
                                        }
                                        land_cover::LC_SNOW_ICE => {
                                            let h = land_cover::coord_hash(x, z) % 10;
                                            if h < 7 {
                                                (SNOW_BLOCK, DIRT)
                                            } else {
                                                (PACKED_ICE, PACKED_ICE)
                                            }
                                        }
                                        // LC_WATER handled above with variable depth
                                        land_cover::LC_WETLAND => (MUD, DIRT),
                                        land_cover::LC_MANGROVES => (MUD, DIRT),
                                        _ => (GRASS_BLOCK, DIRT),
                                    }
                                }
                            } else if terrain_enabled {
                                // No land cover data but terrain is enabled: apply slope-based materials
                                let slope = ground.slope(coord);
                                if slope > 6 {
                                    (STONE, STONE)
                                } else if slope > 4 {
                                    (ANDESITE, STONE)
                                } else if slope > 3 {
                                    (GRAVEL, STONE)
                                } else {
                                    (GRASS_BLOCK, DIRT)
                                }
                            } else {
                                (GRASS_BLOCK, DIRT)
                            };

                            // Shoreline blending: land blocks adjacent to water get
                            // sand surface for a natural beach/shore transition.
                            // Check both ESA water classification AND placed water
                            // blocks (from OSM) to bridge any gap between the two.
                            let (surface_block, under_block) = if surface_block != WATER {
                                let near_esa_water = has_land_cover
                                    && ground.water_distance(coord) == 0
                                    && [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)].iter().any(
                                        |(dx, dz)| {
                                            ground.cover_class(XZPoint::new(x + dx, z + dz))
                                                == land_cover::LC_WATER
                                        },
                                    );
                                let near_placed_water = [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)]
                                    .iter()
                                    .any(|(dx, dz)| {
                                        editor.check_for_block_absolute(
                                            x + dx,
                                            ground_y,
                                            z + dz,
                                            Some(&[WATER]),
                                            None,
                                        )
                                    });
                                if near_esa_water || near_placed_water {
                                    (SAND, SANDSTONE)
                                } else {
                                    (surface_block, under_block)
                                }
                            } else {
                                (surface_block, under_block)
                            };

                            editor.set_block_if_absent_absolute(surface_block, x, ground_y, z);

                            // Don't place dirt/under blocks below water surfaces.
                            // OSM water (rivers, lakes) is placed during element processing;
                            // placing dirt underneath would show through shallow water.
                            let surface_is_water = editor.check_for_block_absolute(
                                x,
                                ground_y,
                                z,
                                Some(&[WATER]),
                                None,
                            );
                            if !surface_is_water {
                                editor.set_block_if_absent_absolute(
                                    under_block,
                                    x,
                                    ground_y - 1,
                                    z,
                                );
                                editor.set_block_if_absent_absolute(
                                    under_block,
                                    x,
                                    ground_y - 2,
                                    z,
                                );
                            } else {
                                // Under OSM water: find bottom of water column,
                                // place sand/gravel/clay floor + sandstone below.
                                let mut water_bottom = ground_y;
                                while water_bottom - 1 > MIN_Y
                                    && editor.check_for_block_absolute(
                                        x,
                                        water_bottom - 1,
                                        z,
                                        Some(&[WATER]),
                                        None,
                                    )
                                {
                                    water_bottom -= 1;
                                }
                                let floor_y = water_bottom - 1;
                                if floor_y > MIN_Y {
                                    let h = land_cover::coord_hash(x, z);
                                    let floor_block = match h % 5 {
                                        0 => GRAVEL,
                                        1 => CLAY,
                                        _ => SAND,
                                    };
                                    editor.set_block_if_absent_absolute(floor_block, x, floor_y, z);
                                    if floor_y - 1 > MIN_Y {
                                        editor.set_block_if_absent_absolute(
                                            SANDSTONE,
                                            x,
                                            floor_y - 1,
                                            z,
                                        );
                                    }
                                }
                            }

                            // Place vegetation from ESA land cover classification
                            // Only if nothing was already placed above ground by OSM processing
                            // and the ground block is a natural surface (not a road, building slab, etc.)
                            let ground_is_natural = editor.check_for_block_absolute(
                                x,
                                ground_y,
                                z,
                                Some(&[GRASS_BLOCK, COARSE_DIRT, DIRT, MUD, FARMLAND]),
                                None,
                            );
                            // Trees can also grow through stone surfaces (urban tree cover)
                            let ground_allows_trees = ground_is_natural
                                || editor.check_for_block_absolute(
                                    x,
                                    ground_y,
                                    z,
                                    Some(&[SMOOTH_STONE, STONE_BRICKS, CRACKED_STONE_BRICKS]),
                                    None,
                                );
                            if has_land_cover && !editor.block_exists_absolute(x, ground_y + 1, z) {
                                let cover = ground.cover_class(coord);
                                let slope = if terrain_enabled {
                                    ground.slope(coord)
                                } else {
                                    0
                                };
                                let mut rng = crate::deterministic_rng::coord_rng(x, z, 0);

                                match cover {
                                    land_cover::LC_TREE_COVER
                                        if slope <= 4 && ground_allows_trees =>
                                    {
                                        let choice = rng.random_range(0..30);
                                        if choice == 0 {
                                            tree::Tree::create(
                                                editor,
                                                (x, 1, z),
                                                Some(building_footprints),
                                            );
                                        } else if ground_is_natural {
                                            // Undergrowth only on natural surfaces
                                            if choice == 1 {
                                                let flower = [
                                                    RED_FLOWER,
                                                    BLUE_FLOWER,
                                                    YELLOW_FLOWER,
                                                    WHITE_FLOWER,
                                                ][rng.random_range(0..4)];
                                                editor.set_block_absolute(
                                                    flower,
                                                    x,
                                                    ground_y + 1,
                                                    z,
                                                    None,
                                                    None,
                                                );
                                            } else if choice <= 13 {
                                                editor.set_block_absolute(
                                                    GRASS,
                                                    x,
                                                    ground_y + 1,
                                                    z,
                                                    None,
                                                    None,
                                                );
                                            }
                                        }
                                    }
                                    land_cover::LC_SHRUBLAND if ground_is_natural => {
                                        let choice = rng.random_range(0..100);
                                        if choice < 2 {
                                            editor.set_block_absolute(
                                                OAK_LEAVES,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                        } else if choice < 30 {
                                            editor.set_block_absolute(
                                                GRASS,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                        }
                                    }
                                    land_cover::LC_GRASSLAND if ground_is_natural => {
                                        // Short grass on grassland (~55%)
                                        let choice = rng.random_range(0..100);
                                        if choice < 50 {
                                            editor.set_block_absolute(
                                                GRASS,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                        } else if choice < 55 {
                                            // Occasional tall grass
                                            editor.set_block_absolute(
                                                TALL_GRASS_BOTTOM,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                            editor.set_block_absolute(
                                                TALL_GRASS_TOP,
                                                x,
                                                ground_y + 2,
                                                z,
                                                None,
                                                None,
                                            );
                                        } else if choice == 55 {
                                            let flower = [
                                                RED_FLOWER,
                                                BLUE_FLOWER,
                                                YELLOW_FLOWER,
                                                WHITE_FLOWER,
                                            ][rng.random_range(0..4)];
                                            editor.set_block_absolute(
                                                flower,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                        }
                                    }
                                    land_cover::LC_CROPLAND => {
                                        // Only place crops if the ground is actually farmland
                                        if editor.check_for_block_absolute(
                                            x,
                                            ground_y,
                                            z,
                                            Some(&[FARMLAND]),
                                            None,
                                        ) {
                                            if x % 9 == 0 && z % 9 == 0 {
                                                editor.set_block_absolute(
                                                    WATER,
                                                    x,
                                                    ground_y,
                                                    z,
                                                    Some(&[FARMLAND]),
                                                    None,
                                                );
                                            } else if rng.random_range(0..76) == 0 {
                                                if rng.random_range(1..=10) <= 4 {
                                                    editor.set_block_absolute(
                                                        HAY_BALE,
                                                        x,
                                                        ground_y + 1,
                                                        z,
                                                        None,
                                                        None,
                                                    );
                                                }
                                            } else {
                                                let crop = [WHEAT, CARROTS, POTATOES]
                                                    [rng.random_range(0..3)];
                                                editor.set_block_absolute(
                                                    crop,
                                                    x,
                                                    ground_y + 1,
                                                    z,
                                                    None,
                                                    None,
                                                );
                                            }
                                        }
                                    }
                                    land_cover::LC_WETLAND | land_cover::LC_MANGROVES
                                        if ground_is_natural =>
                                    {
                                        let choice = rng.random_range(0..100);
                                        if choice < 30 {
                                            // Water patches in wetlands
                                            editor.set_block_absolute(
                                                WATER,
                                                x,
                                                ground_y,
                                                z,
                                                Some(&[MUD, GRASS_BLOCK]),
                                                None,
                                            );
                                        } else if choice < 65 {
                                            editor.set_block_absolute(
                                                GRASS,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                        } else if choice < 75 {
                                            editor.set_block_absolute(
                                                TALL_GRASS_BOTTOM,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                            editor.set_block_absolute(
                                                TALL_GRASS_TOP,
                                                x,
                                                ground_y + 2,
                                                z,
                                                None,
                                                None,
                                            );
                                        }
                                    }
                                    land_cover::LC_BARE if ground_is_natural => {
                                        // Sparse dead bushes
                                        if rng.random_range(0..100) == 0 {
                                            editor.set_block_absolute(
                                                DEAD_BUSH,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        } // end else (non-water)
                    }

                    // Post-processing: remove stray vegetation from road surfaces.
                    // Despite guards in natural/landuse processing, overlapping elements
                    // with the same priority can still place vegetation on roads depending
                    // on sort order. This cleanup pass catches any remaining cases.
                    if editor.check_for_block_absolute(
                        x,
                        ground_y,
                        z,
                        Some(&[
                            BLACK_CONCRETE,
                            GRAY_CONCRETE,
                            LIGHT_GRAY_CONCRETE,
                            WHITE_CONCRETE,
                            DIRT_PATH,
                        ]),
                        None,
                    ) && editor.check_for_block_absolute(
                        x,
                        ground_y + 1,
                        z,
                        Some(&[
                            GRASS,
                            OAK_LEAVES,
                            DEAD_BUSH,
                            TALL_GRASS_BOTTOM,
                            RED_FLOWER,
                            BLUE_FLOWER,
                            WHITE_FLOWER,
                            YELLOW_FLOWER,
                        ]),
                        None,
                    ) {
                        editor.set_block_absolute(
                            AIR,
                            x,
                            ground_y + 1,
                            z,
                            Some(&[
                                GRASS,
                                OAK_LEAVES,
                                DEAD_BUSH,
                                TALL_GRASS_BOTTOM,
                                RED_FLOWER,
                                BLUE_FLOWER,
                                WHITE_FLOWER,
                                YELLOW_FLOWER,
                            ]),
                            None,
                        );
                        // Also clear tall grass top if it was a two-block plant
                        if editor.check_for_block_absolute(
                            x,
                            ground_y + 2,
                            z,
                            Some(&[TALL_GRASS_TOP]),
                            None,
                        ) {
                            editor.set_block_absolute(
                                AIR,
                                x,
                                ground_y + 2,
                                z,
                                Some(&[TALL_GRASS_TOP]),
                                None,
                            );
                        }
                    }

                    // Fill underground with stone
                    if args.fillground {
                        editor.fill_column_absolute(
                            STONE,
                            x,
                            z,
                            MIN_Y + 1,
                            ground_y - 3,
                            true, // skip_existing: don't overwrite blocks placed by element processing
                        );
                    }
                    // Generate a bedrock level at MIN_Y
                    editor.set_block_absolute(BEDROCK, x, MIN_Y, z, None, Some(&[BEDROCK]));

                    block_counter += 1;
                    #[allow(clippy::manual_is_multiple_of)]
                    if block_counter % batch_size == 0 {
                        ground_pb.inc(batch_size);
                    }

                    gui_progress_grnd += progress_increment_grnd;
                    if (gui_progress_grnd - last_emitted_progress).abs() > 0.25 {
                        emit_gui_progress_update(gui_progress_grnd, "");
                        last_emitted_progress = gui_progress_grnd;
                    }
                }
            }
        }

        // Regions stay in memory and are saved in parallel by save_java()
        // at the end of generation for maximum throughput.
    }

    ground_pb.inc(block_counter % batch_size);
    ground_pb.finish();

    Ok(())
}
