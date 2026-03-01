#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
#[cfg(feature = "bedrock")]
mod bedrock_block_map;
mod block_definitions;
mod bresenham;
mod clipping;
mod colors;
mod coordinate_system;
mod data_processing;
mod deterministic_rng;
mod element_processing;
mod elevation_data;
mod floodfill;
mod floodfill_cache;
mod ground;
mod map_renderer;
mod map_transformation;
mod osm_parser;
#[cfg(feature = "gui")]
mod progress;
mod retrieve_data;
#[cfg(feature = "gui")]
mod telemetry;
#[cfg(test)]
mod test_utilities;
mod urban_ground;
mod version_check;
mod world_editor;
mod world_utils;

use args::Args;
use clap::Parser;
use colored::*;
use std::path::PathBuf;
use std::{env, fs, io::Write};

#[cfg(feature = "gui")]
mod gui;

// If the user does not want the GUI, it's easiest to just mock the progress module to do nothing
#[cfg(not(feature = "gui"))]
mod progress {
    pub fn emit_gui_error(_message: &str) {}
    pub fn emit_gui_progress_update(_progress: f64, _message: &str) {}
    pub fn emit_map_preview_ready() {}
    pub fn emit_open_mcworld_file(_path: &str) {}
    pub fn is_running_with_gui() -> bool {
        false
    }
}
#[cfg(target_os = "windows")]
use windows::Win32::System::Console::{AttachConsole, FreeConsole, ATTACH_PARENT_PROCESS};

fn run_cli() {
    // Configure thread pool with 90% CPU cap to keep system responsive
    floodfill_cache::configure_rayon_thread_pool(0.9);

    // Clean up old cached elevation tiles on startup
    elevation_data::cleanup_old_cached_tiles();

    let version: &str = env!("CARGO_PKG_VERSION");
    let repository: &str = env!("CARGO_PKG_REPOSITORY");
    println!(
        r#"
        ▄████████    ▄████████ ███▄▄▄▄    ▄█     ▄████████
        ███    ███   ███    ███ ███▀▀▀██▄ ███    ███    ███
        ███    ███   ███    ███ ███   ███ ███▌   ███    █▀
        ███    ███  ▄███▄▄▄▄██▀ ███   ███ ███▌   ███
      ▀███████████ ▀▀███▀▀▀▀▀   ███   ███ ███▌ ▀███████████
        ███    ███ ▀███████████ ███   ███ ███           ███
        ███    ███   ███    ███ ███   ███ ███     ▄█    ███
        ███    █▀    ███    ███  ▀█   █▀  █▀    ▄████████▀
                     ███    ███

                          version {}
                {}
        "#,
        version,
        repository.bright_white().bold()
    );

    // Check for updates
    if let Err(e) = version_check::check_for_updates() {
        eprintln!(
            "{}: {}",
            "Error checking for version updates".red().bold(),
            e
        );
    }

    // Parse input arguments
    let args: Args = Args::parse();

    // Validate arguments (path requirements differ between Java and Bedrock)
    if let Err(e) = args::validate_args(&args) {
        eprintln!("{}: {}", "Error".red().bold(), e);
        std::process::exit(1);
    }

    // Early guard: --bedrock requires the bedrock cargo feature
    if args.bedrock && !cfg!(feature = "bedrock") {
        eprintln!(
            "{}: The --bedrock flag requires the 'bedrock' feature. Rebuild with: cargo build --features bedrock",
            "Error".red().bold()
        );
        std::process::exit(1);
    }

    // Determine world format and output path
    let world_format = if args.bedrock {
        world_editor::WorldFormat::BedrockMcWorld
    } else {
        world_editor::WorldFormat::JavaAnvil
    };

    // Build the generation output path and level name
    let (generation_path, level_name) = if args.bedrock {
        // Bedrock: generate .mcworld file in user-specified path or Desktop
        let output_dir = args
            .path
            .clone()
            .unwrap_or_else(world_utils::get_bedrock_output_directory);
        let (output_path, lvl_name) = world_utils::build_bedrock_output(&args.bbox, output_dir);
        (output_path, Some(lvl_name))
    } else {
        // Java: create a new world in the provided output directory
        let base_dir = args.path.clone().unwrap();
        let world_path = match world_utils::create_new_world(&base_dir) {
            Ok(path) => PathBuf::from(path),
            Err(e) => {
                eprintln!("{} {}", "Error:".red().bold(), e);
                std::process::exit(1);
            }
        };
        println!(
            "Created new world at: {}",
            world_path.display().to_string().bright_white().bold()
        );
        (world_path, None)
    };

    // Fetch data
    let raw_data = match &args.file {
        Some(file) => retrieve_data::fetch_data_from_file(file),
        None => retrieve_data::fetch_data_from_overpass(
            args.bbox,
            args.debug,
            args.downloader.as_str(),
            args.save_json_file.as_deref(),
        ),
    }
    .expect("Failed to fetch data");

    let mut ground = ground::generate_ground_data(&args);

    // Parse raw data
    let (mut parsed_elements, mut xzbbox) =
        osm_parser::parse_osm_data(raw_data, args.bbox, args.scale, args.debug);
    parsed_elements
        .sort_by_key(|element: &osm_parser::ProcessedElement| osm_parser::get_priority(element));

    // Write the parsed OSM data to a file for inspection
    if args.debug {
        let mut buf = std::io::BufWriter::new(
            fs::File::create("parsed_osm_data.txt").expect("Failed to create output file"),
        );
        for element in &parsed_elements {
            writeln!(
                buf,
                "Element ID: {}, Type: {}, Tags: {:?}",
                element.id(),
                element.kind(),
                element.tags(),
            )
            .expect("Failed to write to output file");
        }
    }

    // Transform map (parsed_elements). Operations are defined in a json file
    map_transformation::transform_map(&mut parsed_elements, &mut xzbbox, &mut ground);

    // Convert spawn lat/lng to Minecraft XZ coordinates if provided
    let spawn_point: Option<(i32, i32)> = match (args.spawn_lat, args.spawn_lng) {
        (Some(lat), Some(lng)) => {
            use coordinate_system::geographic::LLPoint;
            use coordinate_system::transformation::CoordTransformer;

            let llpoint = LLPoint::new(lat, lng).unwrap_or_else(|e| {
                eprintln!("{} Invalid spawn coordinates: {}", "Error:".red().bold(), e);
                std::process::exit(1);
            });

            let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(&args.bbox, args.scale)
                .unwrap_or_else(|e| {
                    eprintln!(
                        "{} Failed to convert spawn point: {}",
                        "Error:".red().bold(),
                        e
                    );
                    std::process::exit(1);
                });

            let xzpoint = transformer.transform_point(llpoint);
            Some((xzpoint.x, xzpoint.z))
        }
        _ => None,
    };

    // Build generation options
    let generation_options = data_processing::GenerationOptions {
        path: generation_path.clone(),
        format: world_format,
        level_name,
        spawn_point,
    };

    // Generate world
    match data_processing::generate_world_with_options(
        parsed_elements,
        xzbbox,
        args.bbox,
        ground,
        &args,
        generation_options,
    ) {
        Ok(_) => {
            if args.bedrock {
                println!(
                    "{} Bedrock world saved to: {}",
                    "Done!".green().bold(),
                    generation_path.display()
                );
            }

            // For Java Edition, update spawn point in level.dat if provided
            if !args.bedrock {
                if let Some((spawn_x, spawn_z)) = spawn_point {
                    if let Err(e) =
                        world_utils::set_spawn_in_level_dat(&generation_path, spawn_x, spawn_z)
                    {
                        eprintln!(
                            "{} Failed to set spawn point in level.dat: {}",
                            "Warning:".yellow().bold(),
                            e
                        );
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("{} {}", "Error:".red().bold(), e);
            std::process::exit(1);
        }
    }
}

fn main() {
    // If on Windows, free and reattach to the parent console when using as a CLI tool
    // Either of these can fail, but if they do it is not an issue, so the return value is ignored
    #[cfg(target_os = "windows")]
    unsafe {
        let _ = FreeConsole();
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }

    // Only run CLI mode if the user supplied args.
    #[cfg(feature = "gui")]
    {
        let gui_mode = std::env::args().len() == 1; // Just "arnis" with no args
        if gui_mode {
            gui::run_gui();
        }
    }

    run_cli();
}
