use crate::args::Args;
use crate::block_definitions::{
    ANDESITE, BEDROCK, BLUE_FLOWER, CARROTS, CLAY, COARSE_DIRT, COBBLESTONE, CRACKED_STONE_BRICKS,
    DEAD_BUSH, DIRT, FARMLAND, GRASS, GRASS_BLOCK, GRAVEL, HAY_BALE, MUD, OAK_LEAVES, POTATOES,
    RED_FLOWER, SAND, SANDSTONE, SMOOTH_STONE, SNOW_BLOCK, STONE, STONE_BRICKS, TALL_GRASS_BOTTOM,
    TALL_GRASS_TOP, WATER, WHEAT, WHITE_FLOWER, YELLOW_FLOWER,
};
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::LLBBox;
use crate::element_processing::*;
use crate::floodfill_cache::FloodFillCache;
use crate::ground::Ground;
use crate::land_cover;
use crate::map_renderer;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole};
use crate::progress::{emit_gui_progress_update, emit_map_preview_ready, emit_open_mcworld_file};
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use crate::world_editor::{WorldEditor, WorldFormat};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use rand::Rng;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

pub const MIN_Y: i32 = -64;

/// Generation options that can be passed separately from CLI Args
#[derive(Clone)]
pub struct GenerationOptions {
    pub path: PathBuf,
    pub format: WorldFormat,
    pub level_name: Option<String>,
    pub spawn_point: Option<(i32, i32)>,
}

/// Generate world with explicit format options (used by GUI for Bedrock support)
pub fn generate_world_with_options(
    elements: Vec<ProcessedElement>,
    xzbbox: XZBBox,
    llbbox: LLBBox,
    ground: Ground,
    args: &Args,
    options: GenerationOptions,
) -> Result<PathBuf, String> {
    let output_path = options.path.clone();
    let world_format = options.format;

    // Create editor with appropriate format
    let mut editor: WorldEditor = WorldEditor::new_with_format_and_name(
        options.path,
        &xzbbox,
        llbbox,
        options.format,
        options.level_name.clone(),
        options.spawn_point,
    );
    let ground = Arc::new(ground);

    println!("{} Processing data...", "[4/7]".bold());

    // Build highway connectivity map once before processing
    let highway_connectivity = highways::build_highway_connectivity_map(&elements);

    // Set ground reference in the editor to enable elevation-aware block placement
    editor.set_ground(Arc::clone(&ground));

    println!("{} Processing terrain...", "[5/7]".bold());
    emit_gui_progress_update(25.0, "Processing terrain...");

    // Pre-compute all flood fills in parallel for better CPU utilization
    let mut flood_fill_cache = FloodFillCache::precompute(&elements, args.timeout.as_ref());

    // Collect building footprints to prevent trees from spawning inside buildings
    // Uses a memory-efficient bitmap (~1 bit per coordinate) instead of a HashSet (~24 bytes per coordinate)
    let building_footprints = flood_fill_cache.collect_building_footprints(&elements, &xzbbox);

    // Process all elements (no longer need to partition boundaries)
    let elements_count: usize = elements.len();
    let process_pb: ProgressBar = ProgressBar::new(elements_count as u64);
    process_pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:45.white/black}] {pos}/{len} elements ({eta}) {msg}")
        .unwrap()
        .progress_chars("█▓░"));

    let progress_increment_prcs: f64 = 45.0 / elements_count as f64;
    let mut current_progress_prcs: f64 = 25.0;
    let mut last_emitted_progress: f64 = current_progress_prcs;

    // Pre-scan: detect building relation outlines that should be suppressed.
    // Only applies to type=building relations (NOT type=multipolygon).
    // When a type=building relation has "part" members, the outline way should not
    // render as a standalone building, the individual parts render instead.
    let suppressed_building_outlines: HashSet<u64> = {
        let mut outlines = HashSet::new();
        for element in &elements {
            if let ProcessedElement::Relation(rel) = element {
                let is_building_type = rel.tags.get("type").map(|t| t.as_str()) == Some("building");
                if is_building_type {
                    let has_parts = rel
                        .members
                        .iter()
                        .any(|m| m.role == ProcessedMemberRole::Part);
                    if has_parts {
                        for member in &rel.members {
                            if member.role == ProcessedMemberRole::Outer {
                                outlines.insert(member.way.id);
                            }
                        }
                    }
                }
            }
        }
        outlines
    };

    // Process all elements
    for element in elements.into_iter() {
        process_pb.inc(1);
        current_progress_prcs += progress_increment_prcs;
        if (current_progress_prcs - last_emitted_progress).abs() > 0.25 {
            emit_gui_progress_update(current_progress_prcs, "");
            last_emitted_progress = current_progress_prcs;
        }

        if args.debug {
            process_pb.set_message(format!(
                "(Element ID: {} / Type: {})",
                element.id(),
                element.kind()
            ));
        } else {
            process_pb.set_message("");
        }

        match &element {
            ProcessedElement::Way(way) => {
                if way.tags.contains_key("building") || way.tags.contains_key("building:part") {
                    // Skip building outlines that are suppressed by building relations with parts.
                    // The individual building:part ways will render instead.
                    if !suppressed_building_outlines.contains(&way.id) {
                        buildings::generate_buildings(
                            &mut editor,
                            way,
                            args,
                            None,
                            None,
                            &flood_fill_cache,
                        );
                    }
                } else if way.tags.contains_key("highway") {
                    highways::generate_highways(
                        &mut editor,
                        &element,
                        args,
                        &highway_connectivity,
                        &flood_fill_cache,
                    );
                } else if way.tags.contains_key("landuse") {
                    landuse::generate_landuse(
                        &mut editor,
                        way,
                        args,
                        &flood_fill_cache,
                        &building_footprints,
                    );
                } else if way.tags.contains_key("natural") {
                    natural::generate_natural(
                        &mut editor,
                        &element,
                        args,
                        &flood_fill_cache,
                        &building_footprints,
                    );
                } else if way.tags.contains_key("amenity") {
                    amenities::generate_amenities(&mut editor, &element, args, &flood_fill_cache);
                } else if way.tags.contains_key("leisure") {
                    leisure::generate_leisure(
                        &mut editor,
                        way,
                        args,
                        &flood_fill_cache,
                        &building_footprints,
                    );
                } else if way.tags.contains_key("barrier") {
                    barriers::generate_barriers(&mut editor, &element);
                } else if let Some(val) = way.tags.get("waterway") {
                    if val == "dock" {
                        // docks count as water areas
                        water_areas::generate_water_area_from_way(&mut editor, way, &xzbbox);
                    } else {
                        waterways::generate_waterways(&mut editor, way);
                    }
                } else if way.tags.contains_key("bridge") {
                    //bridges::generate_bridges(&mut editor, way, ground_level); // TODO FIX
                } else if way.tags.contains_key("railway") {
                    railways::generate_railways(&mut editor, way);
                } else if way.tags.contains_key("roller_coaster") {
                    railways::generate_roller_coaster(&mut editor, way);
                } else if way.tags.contains_key("aeroway") || way.tags.contains_key("area:aeroway")
                {
                    highways::generate_aeroway(&mut editor, way, args);
                } else if way.tags.get("service") == Some(&"siding".to_string()) {
                    highways::generate_siding(&mut editor, way);
                } else if way.tags.get("tomb") == Some(&"pyramid".to_string()) {
                    historic::generate_pyramid(&mut editor, way, args, &flood_fill_cache);
                } else if way.tags.contains_key("man_made") {
                    man_made::generate_man_made(&mut editor, &element, args);
                } else if way.tags.contains_key("power") {
                    power::generate_power(&mut editor, &element);
                } else if way.tags.contains_key("place") {
                    landuse::generate_place(&mut editor, way, args, &flood_fill_cache);
                }
                // Release flood fill cache entry for this way
                flood_fill_cache.remove_way(way.id);
            }
            ProcessedElement::Node(node) => {
                if node.tags.contains_key("door") || node.tags.contains_key("entrance") {
                    doors::generate_doors(&mut editor, node);
                } else if node.tags.contains_key("natural")
                    && node.tags.get("natural") == Some(&"tree".to_string())
                {
                    natural::generate_natural(
                        &mut editor,
                        &element,
                        args,
                        &flood_fill_cache,
                        &building_footprints,
                    );
                } else if node.tags.contains_key("amenity") {
                    amenities::generate_amenities(&mut editor, &element, args, &flood_fill_cache);
                } else if node.tags.contains_key("barrier") {
                    barriers::generate_barrier_nodes(&mut editor, node);
                } else if node.tags.contains_key("highway") {
                    highways::generate_highways(
                        &mut editor,
                        &element,
                        args,
                        &highway_connectivity,
                        &flood_fill_cache,
                    );
                } else if node.tags.contains_key("tourism") {
                    tourisms::generate_tourisms(&mut editor, node);
                } else if node.tags.contains_key("man_made") {
                    man_made::generate_man_made_nodes(&mut editor, node);
                } else if node.tags.contains_key("power") {
                    power::generate_power_nodes(&mut editor, node);
                } else if node.tags.contains_key("historic") {
                    historic::generate_historic(&mut editor, node);
                } else if node.tags.contains_key("emergency") {
                    emergency::generate_emergency(&mut editor, node);
                } else if node.tags.contains_key("advertising") {
                    advertising::generate_advertising(&mut editor, node);
                }
            }
            ProcessedElement::Relation(rel) => {
                let is_building_relation = rel.tags.contains_key("building")
                    || rel.tags.contains_key("building:part")
                    || rel.tags.get("type").map(|t| t.as_str()) == Some("building");
                if is_building_relation {
                    buildings::generate_building_from_relation(
                        &mut editor,
                        rel,
                        args,
                        &flood_fill_cache,
                        &xzbbox,
                    );
                } else if rel.tags.contains_key("water")
                    || rel
                        .tags
                        .get("natural")
                        .map(|val| val == "water" || val == "bay")
                        .unwrap_or(false)
                {
                    water_areas::generate_water_areas_from_relation(&mut editor, rel, &xzbbox);
                } else if rel.tags.contains_key("natural") {
                    natural::generate_natural_from_relation(
                        &mut editor,
                        rel,
                        args,
                        &flood_fill_cache,
                        &building_footprints,
                    );
                } else if rel.tags.contains_key("landuse") {
                    landuse::generate_landuse_from_relation(
                        &mut editor,
                        rel,
                        args,
                        &flood_fill_cache,
                        &building_footprints,
                    );
                } else if rel.tags.get("leisure") == Some(&"park".to_string()) {
                    leisure::generate_leisure_from_relation(
                        &mut editor,
                        rel,
                        args,
                        &flood_fill_cache,
                        &building_footprints,
                    );
                } else if rel.tags.contains_key("man_made") {
                    man_made::generate_man_made(&mut editor, &element, args);
                }
                // Release flood fill cache entries for all ways in this relation
                let way_ids: Vec<u64> = rel.members.iter().map(|m| m.way.id).collect();
                flood_fill_cache.remove_relation_ways(&way_ids);
            }
        }
        // Element is dropped here, freeing its memory immediately
    }

    process_pb.finish();

    // Check if ESA WorldCover land cover data is available for surface block selection
    let has_land_cover = ground.has_land_cover();

    // Drop remaining caches
    drop(highway_connectivity);
    drop(flood_fill_cache);

    // Generate ground layer
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

    // Check if terrain elevation is enabled; when disabled, we can skip ground level lookups entirely
    let terrain_enabled = ground.elevation_enabled;

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
                        let is_esa_water = has_land_cover
                            && ground.cover_class(coord) == land_cover::LC_WATER;

                        if is_esa_water {
                            // Variable water depth based on distance to shore
                            let dist = ground.water_distance(coord);
                            let depth = land_cover::water_depth_from_distance(dist);

                            // Fill water column from surface downward
                            for dy in 0..=depth {
                                editor.set_block_if_absent_absolute(
                                    WATER,
                                    x,
                                    ground_y - dy,
                                    z,
                                );
                            }

                            // Ocean floor: sand on top, sandstone foundation below
                            // so the floor doesn't float when fillground is off
                            let floor_y = ground_y - depth - 1;
                            let h = land_cover::coord_hash(x, z);
                            let floor_block = match depth {
                                0..=2 => SAND,
                                3..=4 => {
                                    if h.is_multiple_of(3) {
                                        GRAVEL
                                    } else {
                                        SAND
                                    }
                                }
                                _ => match h % 4 {
                                    0 => CLAY,
                                    1 => GRAVEL,
                                    _ => SAND,
                                },
                            };
                            editor.set_block_if_absent_absolute(floor_block, x, floor_y, z);
                            editor.set_block_if_absent_absolute(SANDSTONE, x, floor_y - 1, z);
                            editor.set_block_if_absent_absolute(SANDSTONE, x, floor_y - 2, z);
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
                                                    ground.cover_class(XZPoint::new(x + dx, z + dz))
                                                        == land_cover::LC_BARE
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
                                    land_cover::LC_SNOW_ICE => (SNOW_BLOCK, DIRT),
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

                        // Shoreline blending: land blocks adjacent to ESA water get
                        // sand surface for a natural beach/shore transition
                        let (surface_block, under_block) = if has_land_cover
                            && surface_block != WATER
                            && ground.water_distance(coord) == 0
                        {
                            // Check if any cardinal neighbor is water
                            let near_water = [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)]
                                .iter()
                                .any(|(dx, dz)| {
                                    ground.cover_class(XZPoint::new(x + dx, z + dz))
                                        == land_cover::LC_WATER
                                });
                            if near_water {
                                (SAND, SAND)
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
                            editor.set_block_if_absent_absolute(under_block, x, ground_y - 1, z);
                            editor.set_block_if_absent_absolute(under_block, x, ground_y - 2, z);
                        } else {
                            // Under water: sand floor + sandstone foundation
                            editor.set_block_if_absent_absolute(SAND, x, ground_y - 1, z);
                            editor.set_block_if_absent_absolute(SANDSTONE, x, ground_y - 2, z);
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
                                land_cover::LC_TREE_COVER if slope <= 4 && ground_allows_trees => {
                                    let choice = rng.random_range(0..30);
                                    if choice == 0 {
                                        tree::Tree::create(
                                            &mut editor,
                                            (x, 1, z),
                                            Some(&building_footprints),
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
                                        let flower =
                                            [RED_FLOWER, BLUE_FLOWER, YELLOW_FLOWER, WHITE_FLOWER]
                                                [rng.random_range(0..4)];
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
                                                WATER, x, ground_y, z, None, None,
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
                                            let crop =
                                                [WHEAT, CARROTS, POTATOES][rng.random_range(0..3)];
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
                                        editor
                                            .set_block_absolute(WATER, x, ground_y, z, None, None);
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
    }

    // Set sign for player orientation
    /*editor.set_sign(
        "↑".to_string(),
        "Generated World".to_string(),
        "This direction".to_string(),
        "".to_string(),
        9,
        -61,
        9,
        6,
    );*/

    ground_pb.inc(block_counter % batch_size);
    ground_pb.finish();

    // Save world
    if let Err(e) = editor.save() {
        return Err(e.to_string());
    }

    emit_gui_progress_update(99.0, "Finalizing world...");

    // Update player spawn Y coordinate based on terrain height after generation
    #[cfg(feature = "gui")]
    if world_format == WorldFormat::JavaAnvil {
        use crate::gui::update_player_spawn_y_after_generation;
        // Reconstruct bbox string to match the format that GUI originally provided.
        // This ensures LLBBox::from_str() can parse it correctly.
        let bbox_string = format!(
            "{},{},{},{}",
            args.bbox.min().lat(),
            args.bbox.min().lng(),
            args.bbox.max().lat(),
            args.bbox.max().lng()
        );

        // Always update spawn Y since we now always set a spawn point (user-selected or default)
        if let Some(ref world_path) = args.path {
            if let Err(e) = update_player_spawn_y_after_generation(
                world_path,
                bbox_string,
                args.scale,
                ground.as_ref(),
            ) {
                let warning_msg = format!("Failed to update spawn point Y coordinate: {}", e);
                eprintln!("Warning: {}", warning_msg);
                #[cfg(feature = "gui")]
                send_log(LogLevel::Warning, &warning_msg);
            }
        }
    }

    // For Bedrock format, emit event to open the mcworld file
    if world_format == WorldFormat::BedrockMcWorld {
        if let Some(path_str) = output_path.to_str() {
            emit_open_mcworld_file(path_str);
        }
    }

    Ok(output_path)
}

/// Information needed to generate a map preview after world generation is complete
#[derive(Clone)]
pub struct MapPreviewInfo {
    pub world_path: PathBuf,
    pub min_x: i32,
    pub max_x: i32,
    pub min_z: i32,
    pub max_z: i32,
    pub world_area: i64,
}

impl MapPreviewInfo {
    /// Create MapPreviewInfo from world bounds
    pub fn new(world_path: PathBuf, xzbbox: &XZBBox) -> Self {
        let world_width = (xzbbox.max_x() - xzbbox.min_x()) as i64;
        let world_height = (xzbbox.max_z() - xzbbox.min_z()) as i64;
        Self {
            world_path,
            min_x: xzbbox.min_x(),
            max_x: xzbbox.max_x(),
            min_z: xzbbox.min_z(),
            max_z: xzbbox.max_z(),
            world_area: world_width * world_height,
        }
    }
}

/// Maximum area for which map preview generation is allowed (to avoid memory issues)
pub const MAX_MAP_PREVIEW_AREA: i64 = 6400 * 6900;

/// Start map preview generation in a background thread.
/// This should be called AFTER the world generation is complete, the session lock is released,
/// and the GUI has been notified of 100% completion.
///
/// For Java worlds only, and only if the world area is within limits.
pub fn start_map_preview_generation(info: MapPreviewInfo) {
    if info.world_area > MAX_MAP_PREVIEW_AREA {
        return;
    }

    std::thread::spawn(move || {
        // Use catch_unwind to prevent any panic from affecting the application
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            map_renderer::render_world_map(
                &info.world_path,
                info.min_x,
                info.max_x,
                info.min_z,
                info.max_z,
            )
        }));

        match result {
            Ok(Ok(_path)) => {
                // Notify the GUI that the map preview is ready
                emit_map_preview_ready();
            }
            Ok(Err(e)) => {
                eprintln!("Warning: Failed to generate map preview: {}", e);
            }
            Err(_) => {
                eprintln!("Warning: Map preview generation panicked unexpectedly");
            }
        }
    });
}
