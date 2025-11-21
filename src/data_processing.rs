use crate::args::Args;
use crate::block_definitions::{BEDROCK, DIRT, GRASS_BLOCK, STONE, WATER};
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::element_processing::*;
use crate::ground::Ground;
use crate::osm_parser::ProcessedElement;
use crate::progress::emit_gui_progress_update;
use crate::telemetry::{send_log, LogLevel};
use crate::world_editor::WorldEditor;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub const MIN_Y: i32 = -64;

pub fn generate_world(
    elements: Vec<ProcessedElement>,
    xzbbox: XZBBox,
    llbbox: LLBBox,
    ground: Ground,
    args: &Args,
) -> Result<(), String> {
    let mut editor: WorldEditor = WorldEditor::new(args.path.clone(), &xzbbox, llbbox);

    println!("{} Processing data...", "[4/7]".bold());

    // Set ground reference in the editor to enable elevation-aware block placement
    editor.set_ground(&ground);

    println!("{} Processing terrain...", "[5/7]".bold());
    emit_gui_progress_update(25.0, "Processing terrain...");

    // Process data
    let elements_count: usize = elements.len();
    let process_pb: ProgressBar = ProgressBar::new(elements_count as u64);
    process_pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:45.white/black}] {pos}/{len} elements ({eta}) {msg}")
        .unwrap()
        .progress_chars("█▓░"));

    let progress_increment_prcs: f64 = 45.0 / elements_count as f64;
    let mut current_progress_prcs: f64 = 25.0;
    let mut last_emitted_progress: f64 = current_progress_prcs;

    for element in &elements {
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
        }

        match element {
            ProcessedElement::Way(way) => {
                // Check for building first (most common)
                if way.tags.get("building").is_some() || way.tags.get("building:part").is_some() {
                    buildings::generate_buildings(&mut editor, way, args, None);
                } else if way.tags.get("highway").is_some() {
                    highways::generate_highways(&mut editor, element, args, &elements);
                } else if way.tags.get("landuse").is_some() {
                    landuse::generate_landuse(&mut editor, way, args);
                } else if way.tags.get("natural").is_some() {
                    natural::generate_natural(&mut editor, element, args);
                } else if way.tags.get("amenity").is_some() {
                    amenities::generate_amenities(&mut editor, element, args);
                } else if way.tags.get("leisure").is_some() {
                    leisure::generate_leisure(&mut editor, way, args);
                } else if way.tags.get("barrier").is_some() {
                    barriers::generate_barriers(&mut editor, element);
                } else if let Some(val) = way.tags.get("waterway") {
                    if val == "dock" {
                        water_areas::generate_water_area_from_way(&mut editor, way);
                    } else {
                        waterways::generate_waterways(&mut editor, way);
                    }
                } else if way.tags.get("railway").is_some() {
                    railways::generate_railways(&mut editor, way);
                } else if way.tags.get("roller_coaster").is_some() {
                    railways::generate_roller_coaster(&mut editor, way);
                } else if way.tags.get("aeroway").is_some() || way.tags.get("area:aeroway").is_some() {
                    highways::generate_aeroway(&mut editor, way, args);
                } else if way.tags.get("service").map(|s| s.as_str()) == Some("siding") {
                    highways::generate_siding(&mut editor, way);
                } else if way.tags.get("man_made").is_some() {
                    man_made::generate_man_made(&mut editor, element, args);
                }
            }
            ProcessedElement::Node(node) => {
                if node.tags.get("door").is_some() || node.tags.get("entrance").is_some() {
                    doors::generate_doors(&mut editor, node);
                } else if node.tags.get("natural").map(|v| v.as_str()) == Some("tree") {
                    natural::generate_natural(&mut editor, element, args);
                } else if node.tags.get("amenity").is_some() {
                    amenities::generate_amenities(&mut editor, element, args);
                } else if node.tags.get("barrier").is_some() {
                    barriers::generate_barrier_nodes(&mut editor, node);
                } else if node.tags.get("highway").is_some() {
                    highways::generate_highways(&mut editor, element, args, &elements);
                } else if node.tags.get("tourism").is_some() {
                    tourisms::generate_tourisms(&mut editor, node);
                } else if node.tags.get("man_made").is_some() {
                    man_made::generate_man_made_nodes(&mut editor, node);
                }
            }
            ProcessedElement::Relation(rel) => {
                if rel.tags.get("building").is_some() || rel.tags.get("building:part").is_some() {
                    buildings::generate_building_from_relation(&mut editor, rel, args);
                } else if let Some(natural_val) = rel.tags.get("natural") {
                    if natural_val == "water" || natural_val == "bay" {
                        water_areas::generate_water_areas_from_relation(&mut editor, rel);
                    } else {
                        natural::generate_natural_from_relation(&mut editor, rel, args);
                    }
                } else if rel.tags.get("water").is_some() {
                    water_areas::generate_water_areas_from_relation(&mut editor, rel);
                } else if rel.tags.get("landuse").is_some() {
                    landuse::generate_landuse_from_relation(&mut editor, rel, args);
                } else if rel.tags.get("leisure").map(|v| v.as_str()) == Some("park") {
                    leisure::generate_leisure_from_relation(&mut editor, rel, args);
                } else if rel.tags.get("man_made").is_some() {
                    man_made::generate_man_made(
                        &mut editor,
                        &ProcessedElement::Relation(rel.clone()),
                        args,
                    );
                }
            }
        }
    }

    process_pb.finish();

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

    let groundlayer_block = GRASS_BLOCK;
    
    // Calculate sea level Y coordinate to check for water blocks
    let sea_level_y = if ground.elevation_enabled {
        ground.sea_level()
    } else {
        0
    };

    // Chunk-based parallel processing
    const CHUNK_SIZE: i32 = 256;
    let min_x = xzbbox.min_x();
    let max_x = xzbbox.max_x();
    let min_z = xzbbox.min_z();
    let max_z = xzbbox.max_z();

    // Create chunks using iterators
    let chunks: Vec<(i32, i32, i32, i32)> = (min_x..=max_x)
        .step_by(CHUNK_SIZE as usize)
        .flat_map(|chunk_x| {
            (min_z..=max_z)
                .step_by(CHUNK_SIZE as usize)
                .map(move |chunk_z| {
                    let chunk_max_x = (chunk_x + CHUNK_SIZE - 1).min(max_x);
                    let chunk_max_z = (chunk_z + CHUNK_SIZE - 1).min(max_z);
                    (chunk_x, chunk_max_x, chunk_z, chunk_max_z)
                })
        })
        .collect();

    println!("Processing {} chunks in parallel...", chunks.len());

    // Shared progress tracking with atomics (lock-free)
    let progress_counter = Arc::new(AtomicU64::new(0));

    // Process chunks in parallel
    use crate::block_definitions::Block;
    type BlockOp = (i32, i32, i32, Block, bool); // (x, y, z, block, is_absolute)
    let chunk_results: Vec<Vec<BlockOp>> = chunks.par_iter().map(|&(chunk_min_x, chunk_max_x, chunk_min_z, chunk_max_z)| {
        let mut operations = Vec::new();

        for x in chunk_min_x..=chunk_max_x {
            for z in chunk_min_z..=chunk_max_z {
                // Check for existing blocks (this requires reading from editor, which is not thread-safe)
                // For now, we'll skip the check and place blocks unconditionally
                // TODO: Make editor thread-safe or pre-compute which blocks exist
                
                // Add grass/dirt layer
                operations.push((x, 0, z, groundlayer_block, false));
                operations.push((x, -1, z, DIRT, false));
                operations.push((x, -2, z, DIRT, false));

                // Add bedrock
                operations.push((x, MIN_Y, z, BEDROCK, true));

                // Note: Fillground stone filling is complex and requires get_absolute_y
                // We'll handle it separately after parallel processing
            }
        }

        // Update progress atomically (lock-free)
        let chunk_blocks = ((chunk_max_x - chunk_min_x + 1) * (chunk_max_z - chunk_min_z + 1)) as u64;
        progress_counter.fetch_add(chunk_blocks, Ordering::Relaxed);

        operations
    }).collect();

    // Update progress bar with accumulated count
    let total_processed = progress_counter.load(Ordering::Relaxed);
    ground_pb.set_position(total_processed);

    // Apply operations sequentially (batched for better performance)
    println!("Applying {} block operations...", chunk_results.iter().map(|ops| ops.len()).sum::<usize>());
    const PROGRESS_UPDATE_INTERVAL: u64 = 1000;
    
    for operations in chunk_results {
        for (x, y, z, block, is_absolute) in operations {
            // Check if we should place this block
            let absolute_y = if is_absolute {
                y
            } else {
                editor.get_absolute_y(x, y, z)
            };

            // Check if there's water at this exact location (only in ocean depth range)
            let should_place = if absolute_y >= sea_level_y - 2 && absolute_y <= sea_level_y {
                !editor.check_for_block_absolute(x, absolute_y, z, Some(&[WATER]), None)
            } else {
                true
            };

            if should_place {
                if is_absolute {
                    editor.set_block_absolute(block, x, y, z, None, Some(&[BEDROCK]));
                } else {
                    editor.set_block(block, x, y, z, None, None);
                }
            }

            block_counter += 1;
            
            // Update progress less frequently (every 1000 blocks)
            if block_counter % PROGRESS_UPDATE_INTERVAL == 0 {
                ground_pb.inc(PROGRESS_UPDATE_INTERVAL);
                
                gui_progress_grnd += progress_increment_grnd * PROGRESS_UPDATE_INTERVAL as f64;
                if (gui_progress_grnd - last_emitted_progress).abs() > 0.25 {
                    emit_gui_progress_update(gui_progress_grnd, "");
                    last_emitted_progress = gui_progress_grnd;
                }
            }
        }
    }

    // Handle fillground stone filling (parallelized for better performance)
    if args.fillground {
        println!("Filling underground with stone...");
        
        // Create coordinate pairs for parallel processing
        let coords: Vec<(i32, i32)> = (min_x..=max_x)
            .flat_map(|x| (min_z..=max_z).map(move |z| (x, z)))
            .collect();
        
        // Collect fill operations in parallel
        let fill_ops: Vec<(i32, i32, i32)> = coords
            .par_iter()
            .filter_map(|&(x, z)| {
                if !editor.check_for_block(x, sea_level_y, z, Some(&[WATER])) {
                    let target_y = editor.get_absolute_y(x, -3, z);
                    Some((x, z, target_y))
                } else {
                    None
                }
            })
            .collect();
        
        // Apply fill operations sequentially (editor is not thread-safe for writes)
        for (x, z, target_y) in fill_ops {
            editor.fill_blocks_absolute(
                STONE,
                x,
                MIN_Y + 1,
                z,
                x,
                target_y,
                z,
                None,
                None,
            );
        }
    }

    // Update final progress
    let remainder = block_counter % PROGRESS_UPDATE_INTERVAL;
    if remainder > 0 {
        ground_pb.inc(remainder);
    }
    ground_pb.finish();

    // Generate oceans if terrain is enabled (after ground layer)
    if ground.elevation_enabled {
        println!("Generating oceans...");
        oceans::generate_oceans(&mut editor, &elements, &ground, args);
    }

    // Save world
    editor.save();

    // Update player spawn Y coordinate based on terrain height after generation
    #[cfg(feature = "gui")]
    if let Some(spawn_coords) = &args.spawn_point {
        use crate::gui::update_player_spawn_y_after_generation;
        let bbox_string = format!(
            "{},{},{},{}",
            args.bbox.min().lng(),
            args.bbox.min().lat(),
            args.bbox.max().lng(),
            args.bbox.max().lat()
        );

        if let Err(e) = update_player_spawn_y_after_generation(
            &args.path,
            Some(*spawn_coords),
            bbox_string,
            args.scale,
            &ground,
        ) {
            let warning_msg = format!("Failed to update spawn point Y coordinate: {}", e);
            eprintln!("Warning: {}", warning_msg);
            send_log(LogLevel::Warning, &warning_msg);
        }
    }

    emit_gui_progress_update(100.0, "Done! World generation completed.");
    println!("{}", "Done! World generation completed.".green().bold());
    Ok(())
}
