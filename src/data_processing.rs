use crate::args::Args;
use crate::block_definitions::{BEDROCK, DIRT, GRASS_BLOCK, STONE};
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::element_processing::*;
use crate::floodfill_cache::FloodFillCache;
use crate::ground::Ground;
use crate::map_renderer;
use crate::osm_parser::ProcessedElement;
use crate::parallel_processing::{
    calculate_parallel_threads, compute_processing_units, distribute_elements_to_units_indices,
    ParallelConfig, ProcessingStats,
};
use crate::progress::{emit_gui_progress_update, emit_map_preview_ready, emit_open_mcworld_file};
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use crate::unit_processing::{process_unit_refs, SharedProcessingData};
use crate::world_editor::{WorldEditor, WorldFormat};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
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

    // Use sequential by default (parallel has correctness issues)
    // Use --force-parallel to enable experimental parallel mode
    let parallel_config = if args.force_parallel {
        ParallelConfig {
            num_threads: args.threads,
            buffer_blocks: 64,
            enabled: true,
            region_batch_size: args.region_batch_size,
        }
    } else {
        ParallelConfig::sequential()
    };

    generate_world_with_options(elements, xzbbox, llbbox, ground, args, options, parallel_config)
        .map(|_| ())
}

/// Generate world with explicit format options (used by GUI for Bedrock support)
pub fn generate_world_with_options(
    elements: Vec<ProcessedElement>,
    xzbbox: XZBBox,
    llbbox: LLBBox,
    ground: Ground,
    args: &Args,
    options: GenerationOptions,
    parallel_config: ParallelConfig,
) -> Result<PathBuf, String> {
    let _output_path = options.path.clone();
    let _world_format = options.format;

    // Determine if we should use parallel processing
    let num_threads = calculate_parallel_threads(parallel_config.num_threads);
    
    // Calculate region count to decide if parallel is worth the overhead
    let min_region_x = xzbbox.min_x() >> 9;
    let max_region_x = xzbbox.max_x() >> 9;
    let min_region_z = xzbbox.min_z() >> 9;
    let max_region_z = xzbbox.max_z() >> 9;
    let region_count = ((max_region_x - min_region_x + 1) * (max_region_z - min_region_z + 1)) as usize;
    
    // Auto-disable parallel for small areas (< 6 regions) - overhead isn't worth it
    // User can still force parallel with explicit --threads > 1 and region count check
    let use_parallel = parallel_config.enabled && num_threads > 1 && region_count >= 6;
    
    let mode_reason = if !parallel_config.enabled {
        "disabled by --no-parallel"
    } else if num_threads <= 1 {
        "single thread"
    } else if region_count < 6 {
        "small area (< 6 regions)"
    } else {
        "parallel"
    };

    println!(
        "{} Processing data ({} mode, {} thread(s), {} regions)...",
        "[4/7]".bold(),
        if use_parallel { "parallel" } else { "sequential" },
        num_threads,
        region_count
    );
    
    if !use_parallel && parallel_config.enabled && region_count < 6 {
        println!("  (auto-selected sequential: {})", mode_reason);
    }

    // Build highway connectivity map once before processing (needed for all units)
    let highway_connectivity = Arc::new(highways::build_highway_connectivity_map(&elements));

    let ground = Arc::new(ground);

    println!("{} Processing terrain...", "[5/7]".bold());
    emit_gui_progress_update(25.0, "Processing terrain...");

    // Pre-compute all flood fills in parallel for better CPU utilization
    let flood_fill_cache = Arc::new(FloodFillCache::precompute(
        &elements,
        args.timeout.as_ref(),
    ));

    // Collect building footprints to prevent trees from spawning inside buildings
    let building_footprints =
        Arc::new(flood_fill_cache.collect_building_footprints(&elements, &xzbbox));

    if use_parallel {
        // === PARALLEL PROCESSING PATH ===
        generate_world_parallel(
            elements,
            xzbbox,
            llbbox,
            ground,
            highway_connectivity,
            flood_fill_cache,
            building_footprints,
            args,
            options,
            parallel_config,
        )
    } else {
        // === SEQUENTIAL PROCESSING PATH (original logic) ===
        generate_world_sequential(
            elements,
            xzbbox,
            llbbox,
            ground,
            highway_connectivity,
            flood_fill_cache,
            building_footprints,
            args,
            options,
        )
    }
}

/// Parallel world generation - processes regions in parallel, saving each immediately
#[allow(clippy::too_many_arguments)]
fn generate_world_parallel(
    elements: Vec<ProcessedElement>,
    xzbbox: XZBBox,
    llbbox: LLBBox,
    ground: Arc<Ground>,
    highway_connectivity: Arc<highways::HighwayConnectivityMap>,
    flood_fill_cache: Arc<FloodFillCache>,
    building_footprints: Arc<crate::floodfill_cache::BuildingFootprintBitmap>,
    args: &Args,
    options: GenerationOptions,
    parallel_config: ParallelConfig,
) -> Result<PathBuf, String> {
    let output_path = options.path.clone();
    let world_format = options.format;

    // Compute processing units (one or more regions per unit depending on batch size)
    let units = compute_processing_units(
        &xzbbox, 
        parallel_config.buffer_blocks, 
        parallel_config.region_batch_size
    );
    let total_units = units.len();

    println!(
        "  {} unit(s) to process across {} thread(s) (batch size: {})",
        total_units,
        calculate_parallel_threads(parallel_config.num_threads),
        parallel_config.region_batch_size
    );

    // Distribute elements to units based on spatial intersection
    // Returns indices into the elements vector for each unit
    let unit_element_indices = distribute_elements_to_units_indices(&elements, &units);

    // Wrap elements in Arc for shared access across threads
    let elements = Arc::new(elements);

    // Create shared data for all units
    let shared = Arc::new(SharedProcessingData {
        ground: Arc::clone(&ground),
        highway_connectivity: Arc::clone(&highway_connectivity),
        building_footprints: Arc::clone(&building_footprints),
        floodfill_cache: Arc::clone(&flood_fill_cache),
        llbbox,
        world_dir: options.path.clone(),
        format: options.format,
        level_name: options.level_name.clone(),
        terrain_enabled: args.terrain,
        ground_level: args.ground_level,
        fill_ground: args.fillground,
        interior: args.interior,
        roof: args.roof,
        debug: args.debug,
        timeout: args.timeout,
    });

    // Set up progress tracking
    let stats = Arc::new(ProcessingStats::new(total_units, 0));
    let process_pb = ProgressBar::new(total_units as u64);
    process_pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:45}] {pos}/{len} regions ({eta})")
            .unwrap()
            .progress_chars("█▓░"),
    );

    // Process units in parallel
    println!("{} Processing regions in parallel...", "[5/7]".bold());

    // Log element distribution stats
    let total_element_refs: usize = unit_element_indices.iter().map(|v| v.len()).sum();
    let avg_elements_per_unit = total_element_refs as f64 / total_units as f64;
    println!(
        "  Total element references: {} (avg {:.1} per unit, original: {})",
        total_element_refs, avg_elements_per_unit, elements.len()
    );
    println!(
        "  Element processing overhead: {:.1}x (elements processed multiple times across regions)",
        total_element_refs as f64 / elements.len() as f64
    );

    // Configure thread pool to use requested number of threads
    let num_threads = calculate_parallel_threads(parallel_config.num_threads);

    // Process each unit: generate blocks, save region, free memory
    let units_with_indices: Vec<_> = units
        .into_iter()
        .zip(unit_element_indices.into_iter())
        .collect();

    // Track timing for each unit
    let unit_times = std::sync::Mutex::new(Vec::with_capacity(total_units));
    let parallel_start = std::time::Instant::now();
    
    // Track which thread processes each unit
    let thread_ids = std::sync::Mutex::new(std::collections::HashSet::new());

    // Use rayon's parallel iterator with configured thread count
    println!("  Starting parallel processing with {} threads...", num_threads);
    rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build()
        .unwrap()
        .install(|| {
            units_with_indices
                .par_iter()
                .for_each(|(unit, element_indices)| {
                    // Track thread usage
                    let thread_id = std::thread::current().id();
                    thread_ids.lock().unwrap().insert(format!("{:?}", thread_id));
                    
                    let unit_start = std::time::Instant::now();
                    
                    // Collect elements for this unit using indices - only clone what's needed
                    let unit_elements: Vec<&ProcessedElement> = element_indices
                        .iter()
                        .map(|&idx| &elements[idx])
                        .collect();

                    // Create bbox for this specific unit
                    let unit_bbox = unit.bbox();

                    // Process this unit and save immediately
                    let process_start = std::time::Instant::now();
                    let mut editor = process_unit_refs(unit, &unit_elements, &shared, &unit_bbox, args);
                    let process_time = process_start.elapsed();
                    
                    // Save this region silently (no progress output)
                    let save_start = std::time::Instant::now();
                    editor.save_silent();
                    let save_time = save_start.elapsed();
                    
                    // editor is dropped here, freeing its memory
                    let total_time = unit_start.elapsed();

                    // Update progress
                    let completed = stats.increment_completed();
                    process_pb.inc(1);
                    
                    // Store timing info
                    unit_times.lock().unwrap().push((
                        unit.region_x,
                        unit.region_z,
                        element_indices.len(),
                        process_time,
                        save_time,
                        total_time,
                    ));

                    // Progress: 25% (terrain done) to 90% (regions done)
                    // This covers the full parallel processing phase
                    let progress = 25.0 + (completed as f64 / total_units as f64) * 65.0;
                    emit_gui_progress_update(progress, &format!("Processing unit {}/{}...", completed, total_units));
                });
        });

    process_pb.finish();
    let parallel_duration = parallel_start.elapsed();
    
    // Report thread usage
    let unique_threads = thread_ids.into_inner().unwrap();
    println!("  Threads actually used: {} (requested: {})", unique_threads.len(), num_threads);

    // Print timing summary
    let times = unit_times.into_inner().unwrap();
    println!("\n  === Unit Processing Times ===");
    
    let mut total_process = std::time::Duration::ZERO;
    let mut total_save = std::time::Duration::ZERO;
    
    // Sort by total time descending to show slowest first
    let mut sorted_times = times.clone();
    sorted_times.sort_by(|a, b| b.5.cmp(&a.5));
    
    for (rx, rz, elem_count, process, save, total) in sorted_times.iter().take(10) {
        println!(
            "    Region ({:3},{:3}): {} elements, process: {:>6.2}s, save: {:>5.2}s, total: {:>6.2}s",
            rx, rz, elem_count, 
            process.as_secs_f64(), 
            save.as_secs_f64(), 
            total.as_secs_f64()
        );
        total_process += *process;
        total_save += *save;
    }
    
    if times.len() > 10 {
        for (_, _, _, process, save, _) in times.iter().skip(10) {
            total_process += *process;
            total_save += *save;
        }
        println!("    ... and {} more units", times.len() - 10);
    }
    
    let sum_total: std::time::Duration = times.iter().map(|t| t.5).sum();
    println!("  Sum of all unit times: {:.2}s (process: {:.2}s, save: {:.2}s)",
        sum_total.as_secs_f64(), total_process.as_secs_f64(), total_save.as_secs_f64());
    println!("  Actual wall time: {:.2}s", parallel_duration.as_secs_f64());
    println!("  Parallelism factor: {:.2}x (sum/wall)", sum_total.as_secs_f64() / parallel_duration.as_secs_f64());
    println!();

    // Final save for any remaining metadata or global operations
    println!("{} Finalizing world...", "[7/7]".bold());
    emit_gui_progress_update(90.0, "Finalizing world...");

    // Save metadata file (regions already saved individually during processing)
    let mut metadata_editor = WorldEditor::new_with_format_and_name(
        options.path.clone(),
        &xzbbox,
        llbbox,
        options.format,
        options.level_name,
        options.spawn_point,
    );
    metadata_editor.set_ground(Arc::clone(&ground));
    // Only save metadata, not the world data (already saved per-region)
    if let Err(e) = metadata_editor.save_metadata() {
        eprintln!("Warning: Failed to save metadata: {}", e);
    }

    emit_gui_progress_update(99.0, "World generation complete!");

    // Handle spawn point update for GUI
    #[cfg(feature = "gui")]
    if world_format == WorldFormat::JavaAnvil {
        use crate::gui::update_player_spawn_y_after_generation;
        let bbox_string = format!(
            "{},{},{},{}",
            args.bbox.min().lat(),
            args.bbox.min().lng(),
            args.bbox.max().lat(),
            args.bbox.max().lng()
        );

        if let Err(e) =
            update_player_spawn_y_after_generation(&args.path, bbox_string, args.scale, &ground)
        {
            let warning_msg = format!("Failed to update spawn point Y coordinate: {}", e);
            eprintln!("Warning: {}", warning_msg);
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

/// Sequential world generation - original logic preserved for debugging/comparison
#[allow(clippy::too_many_arguments)]
fn generate_world_sequential(
    elements: Vec<ProcessedElement>,
    xzbbox: XZBBox,
    llbbox: LLBBox,
    ground: Arc<Ground>,
    highway_connectivity: Arc<highways::HighwayConnectivityMap>,
    flood_fill_cache: Arc<FloodFillCache>,
    building_footprints: Arc<crate::floodfill_cache::BuildingFootprintBitmap>,
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

    // Set ground reference in the editor to enable elevation-aware block placement
    editor.set_ground(Arc::clone(&ground));

    // Process data
    let elements_count: usize = elements.len();
    let mut elements = elements; // Take ownership for consuming
    let process_pb: ProgressBar = ProgressBar::new(elements_count as u64);
    process_pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:45.white/black}] {pos}/{len} elements ({eta}) {msg}")
        .unwrap()
        .progress_chars("█▓░"));

    let progress_increment_prcs: f64 = 45.0 / elements_count as f64;
    let mut current_progress_prcs: f64 = 25.0;
    let mut last_emitted_progress: f64 = current_progress_prcs;

    // Process elements by draining in insertion order
    for element in elements.drain(..) {
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
                }
                // Note: flood fill cache entries are managed by Arc, not removed per-element in Arc version
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
                // Note: flood fill cache entries are managed by Arc, dropped when no longer referenced
            }
        }
        // Element is dropped here, freeing its memory immediately
    }

    process_pb.finish();

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
