use crate::args::Args;
use crate::block_definitions::{BEDROCK, DIRT, GRASS_BLOCK, STONE};
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::element_processing::*;
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
    let mut editor: WorldEditor = WorldEditor::new_with_format_and_name(
        options.path,
        &xzbbox,
        llbbox,
        options.format,
        options.level_name,
        options.spawn_point,
    );

    println!("{} Processing data...", "[4/7]".bold());

    // Build highway connectivity map once before processing
    let highway_connectivity = highways::build_highway_connectivity_map(&elements);

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
        } else {
            process_pb.set_message("");
        }

        match element {
            ProcessedElement::Way(way) => {
                if way.tags.contains_key("building") || way.tags.contains_key("building:part") {
                    buildings::generate_buildings(&mut editor, way, args, None);
                } else if way.tags.contains_key("highway") {
                    highways::generate_highways(&mut editor, element, args, &highway_connectivity);
                } else if way.tags.contains_key("landuse") {
                    landuse::generate_landuse(&mut editor, way, args);
                } else if way.tags.contains_key("natural") {
                    natural::generate_natural(&mut editor, element, args);
                } else if way.tags.contains_key("amenity") {
                    amenities::generate_amenities(&mut editor, element, args);
                } else if way.tags.contains_key("leisure") {
                    leisure::generate_leisure(&mut editor, way, args);
                } else if way.tags.contains_key("barrier") {
                    barriers::generate_barriers(&mut editor, element);
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
                    man_made::generate_man_made(&mut editor, element, args);
                }
            }
            ProcessedElement::Node(node) => {
                if node.tags.contains_key("door") || node.tags.contains_key("entrance") {
                    doors::generate_doors(&mut editor, node);
                } else if node.tags.contains_key("natural")
                    && node.tags.get("natural") == Some(&"tree".to_string())
                {
                    natural::generate_natural(&mut editor, element, args);
                } else if node.tags.contains_key("amenity") {
                    amenities::generate_amenities(&mut editor, element, args);
                } else if node.tags.contains_key("barrier") {
                    barriers::generate_barrier_nodes(&mut editor, node);
                } else if node.tags.contains_key("highway") {
                    highways::generate_highways(&mut editor, element, args, &highway_connectivity);
                } else if node.tags.contains_key("tourism") {
                    tourisms::generate_tourisms(&mut editor, node);
                } else if node.tags.contains_key("man_made") {
                    man_made::generate_man_made_nodes(&mut editor, node);
                }
            }
            ProcessedElement::Relation(rel) => {
                if rel.tags.contains_key("building") || rel.tags.contains_key("building:part") {
                    buildings::generate_building_from_relation(&mut editor, rel, args);
                } else if rel.tags.contains_key("water")
                    || rel
                        .tags
                        .get("natural")
                        .map(|val| val == "water" || val == "bay")
                        .unwrap_or(false)
                {
                    water_areas::generate_water_areas_from_relation(&mut editor, rel, &xzbbox);
                } else if rel.tags.contains_key("natural") {
                    natural::generate_natural_from_relation(&mut editor, rel, args);
                } else if rel.tags.contains_key("landuse") {
                    landuse::generate_landuse_from_relation(&mut editor, rel, args);
                } else if rel.tags.get("leisure") == Some(&"park".to_string()) {
                    leisure::generate_leisure_from_relation(&mut editor, rel, args);
                } else if rel.tags.contains_key("man_made") {
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

    for x in xzbbox.min_x()..=xzbbox.max_x() {
        for z in xzbbox.min_z()..=xzbbox.max_z() {
            // Add default dirt and grass layer if there isn't a stone layer already
            if !editor.check_for_block(x, 0, z, Some(&[STONE])) {
                editor.set_block(groundlayer_block, x, 0, z, None, None);
                editor.set_block(DIRT, x, -1, z, None, None);
                editor.set_block(DIRT, x, -2, z, None, None);
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
                    editor.get_absolute_y(x, -3, z),
                    z,
                    None,
                    None,
                );
            }
            // Generate a bedrock level at MIN_Y
            editor.set_block_absolute(BEDROCK, x, MIN_Y, z, None, Some(&[BEDROCK]));

            block_counter += 1;
            // Use manual % check since is_multiple_of() is unstable on stable Rust
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

    // Update player spawn Y coordinate based on terrain height after generation
    #[cfg(feature = "gui")]
    if world_format == WorldFormat::JavaAnvil {
        if let Some(spawn_coords) = &args.spawn_point {
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

            if let Err(e) = update_player_spawn_y_after_generation(
                &args.path,
                Some(*spawn_coords),
                bbox_string,
                args.scale,
                &ground,
            ) {
                let warning_msg = format!("Failed to update spawn point Y coordinate: {}", e);
                eprintln!("Warning: {}", warning_msg);
                #[cfg(feature = "gui")]
                send_log(LogLevel::Warning, &warning_msg);
            }
        }
    }

    emit_gui_progress_update(99.0, "Finalizing world...");

    // For Bedrock format, emit event to open the mcworld file
    if world_format == WorldFormat::BedrockMcWorld {
        if let Some(path_str) = output_path.to_str() {
            emit_open_mcworld_file(path_str);
        }
    }

    // Generate top-down map preview silently in background after completion (Java only)
    // Skip map preview for very large areas to avoid memory issues
    const MAX_MAP_PREVIEW_AREA: i64 = 6400 * 6900;
    let world_width = (xzbbox.max_x() - xzbbox.min_x()) as i64;
    let world_height = (xzbbox.max_z() - xzbbox.min_z()) as i64;
    let world_area = world_width * world_height;

    if world_format == WorldFormat::JavaAnvil && world_area <= MAX_MAP_PREVIEW_AREA {
        let world_path = args.path.clone();
        let bounds = (
            xzbbox.min_x(),
            xzbbox.max_x(),
            xzbbox.min_z(),
            xzbbox.max_z(),
        );
        std::thread::spawn(move || {
            // Use catch_unwind to prevent any panic from affecting the application
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                map_renderer::render_world_map(&world_path, bounds.0, bounds.1, bounds.2, bounds.3)
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

    Ok(output_path)
}
