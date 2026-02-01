use crate::args::Args;
use crate::block_definitions::{BEDROCK, DIRT, GRASS_BLOCK, STONE};
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::element_processing::*;
use crate::floodfill_cache::FloodFillCache;
use crate::ground::Ground;
use crate::map_renderer;
use crate::osm_parser::ProcessedElement;
use crate::progress::{emit_gui_progress_update, emit_map_preview_ready, emit_open_mcworld_file};
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use crate::world_editor::{WorldEditor, WorldFormat};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
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

pub fn generate_world(
    elements: Vec<ProcessedElement>,
    xzbbox: XZBBox,
    llbbox: LLBBox,
    ground: Ground,
    args: &Args,
) -> Result<(), String> {
    // Default to Java format when called from CLI
    let options = GenerationOptions {
        path: args.path.clone(),
        format: WorldFormat::JavaAnvil,
        level_name: None,
        spawn_point: None,
    };
    generate_world_with_options(elements, xzbbox, llbbox, ground, args, options).map(|_| ())
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

    // Only compute urban coverage and density grid if city boundaries are enabled
    // This saves significant processing time when the feature is disabled
    let urban_density_grid = if args.city_boundaries {
        // Collect urban coverage to determine if boundary areas are truly urbanized
        // This helps avoid placing stone ground in rural areas within city boundaries
        let urban_coverage = flood_fill_cache.collect_urban_coverage(&elements, &xzbbox);

        // Build urban density grid for efficient per-coordinate urban checks with rounded edges
        Some(crate::floodfill_cache::UrbanDensityGrid::from_coverage(
            &urban_coverage,
            &xzbbox,
        ))
    } else {
        None
    };

    // Partition elements: separate boundary elements for deferred processing
    // This avoids cloning by moving elements instead of copying them
    let (boundary_elements, other_elements): (Vec<_>, Vec<_>) = if args.city_boundaries {
        elements
            .into_iter()
            .partition(|element| element.tags().contains_key("boundary"))
    } else {
        // If city boundaries disabled, treat all elements as non-boundary
        (Vec::new(), elements)
    };

    // Process data
    let elements_count: usize = other_elements.len() + boundary_elements.len();
    let process_pb: ProgressBar = ProgressBar::new(elements_count as u64);
    process_pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:45.white/black}] {pos}/{len} elements ({eta}) {msg}")
        .unwrap()
        .progress_chars("█▓░"));

    let progress_increment_prcs: f64 = 45.0 / elements_count as f64;
    let mut current_progress_prcs: f64 = 25.0;
    let mut last_emitted_progress: f64 = current_progress_prcs;

    // Process non-boundary elements first
    for element in other_elements.into_iter() {
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
                    buildings::generate_buildings(&mut editor, way, args, None, &flood_fill_cache);
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
                } else if way.tags.contains_key("man_made") {
                    man_made::generate_man_made(&mut editor, &element, args);
                } else if way.tags.contains_key("power") {
                    power::generate_power(&mut editor, &element);
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
                if rel.tags.contains_key("building") || rel.tags.contains_key("building:part") {
                    buildings::generate_building_from_relation(
                        &mut editor,
                        rel,
                        args,
                        &flood_fill_cache,
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

    // Process deferred boundary elements after all other elements (only if city boundaries enabled)
    // This ensures boundaries only fill empty areas, they won't overwrite
    // any ground blocks set by landuse, leisure, natural, etc.
    if let Some(ref density_grid) = urban_density_grid {
        for element in boundary_elements.into_iter() {
            match &element {
                ProcessedElement::Way(way) => {
                    boundaries::generate_boundary(
                        &mut editor,
                        way,
                        args,
                        &flood_fill_cache,
                        density_grid,
                    );
                    // Clean up cache entry for consistency with other element processing
                    flood_fill_cache.remove_way(way.id);
                }
                ProcessedElement::Relation(rel) => {
                    boundaries::generate_boundary_from_relation(
                        &mut editor,
                        rel,
                        args,
                        &flood_fill_cache,
                        density_grid,
                        &xzbbox,
                    );
                    // Clean up cache entries for consistency with other element processing
                    let way_ids: Vec<u64> = rel.members.iter().map(|m| m.way.id).collect();
                    flood_fill_cache.remove_relation_ways(&way_ids);
                }
                _ => {}
            }
            // Element is dropped here, freeing its memory immediately
        }
    }

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

                    // Add default dirt and grass layer if there isn't a stone layer already
                    if !editor.check_for_block_absolute(x, ground_y, z, Some(&[STONE]), None) {
                        editor.set_block_absolute(GRASS_BLOCK, x, ground_y, z, None, None);
                        editor.set_block_absolute(DIRT, x, ground_y - 1, z, None, None);
                        editor.set_block_absolute(DIRT, x, ground_y - 2, z, None, None);
                    }

                    // Fill underground with stone
                    if args.fillground {
                        // Fill from bedrock+1 to 3 blocks below ground with stone
                        editor.fill_blocks_absolute(
                            STONE,
                            x,
                            MIN_Y + 1,
                            z,
                            x,
                            ground_y - 3,
                            z,
                            None,
                            None,
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
    editor.save();

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
        if let Err(e) = update_player_spawn_y_after_generation(
            &args.path,
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
