#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
#[cfg(feature = "bedrock")]
mod bedrock_block_map;
mod block_definitions;
mod clipping;
mod colors;
mod data_processing;
mod element_processing;
mod elevation;
mod elevation_data;
mod floodfill_cache;
mod ground;
mod ground_generation;
mod land_cover;
mod map_renderer;
mod map_transformation;
mod osm_parser;
mod overture;
#[cfg(feature = "gui")]
mod progress;
mod retrieve_data;
#[cfg(feature = "gui")]
mod telemetry;
#[cfg(test)]
mod test_utilities;
mod version_check;
mod world_editor;
mod world_utils;

use args::Args;
use clap::Parser;
use colored::*;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "gui")]
mod gui;

// If the user does not want the GUI, it's easiest to just mock the progress module to do nothing
#[cfg(not(feature = "gui"))]
mod progress {
    pub fn emit_gui_error(_message: &str) {}
    pub fn emit_gui_progress_update(_progress: f64, _message: &str) {}
    pub fn emit_map_preview_ready() {}
    pub fn emit_show_in_folder(_path: &str) {}
    pub fn is_running_with_gui() -> bool {
        false
    }
}
use crate::data_processing::GenerationOptions;
use crate::retrieve_data::{get_spawn_point, prepare_data};
use arnis_math::coordinate_system::cartesian::XZPoint;
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
        if args.disable_height_limit {
            if let Err(e) = world_utils::install_tall_datapack(&world_path) {
                eprintln!(
                    "{} Failed to install tall-world datapack: {}",
                    "Error:".red().bold(),
                    e
                );
                std::process::exit(1);
            }
            eprintln!(
                "Note: tall-world datapack installed (requires Minecraft 1.21.4+). \
                 First load will prompt 'Experimental Features'; world can't be uploaded to Realms."
            );
        }
        (world_path, None)
    };

    let (parsed_elements, xzbbox, ground) = prepare_data(&args).unwrap();

    let spawn_point: (i32, i32) = get_spawn_point(
        args.spawn_lat,
        args.spawn_lng,
        &args.bbox,
        args.scale,
        args.rotation,
    );

    // Build generation options
    let generation_options = GenerationOptions {
        path: generation_path.clone(),
        format: world_format,
        level_name,
        spawn_point,
    };

    let ground = Arc::new(ground);
    // Generate world
    match data_processing::generate_world_with_options(
        parsed_elements,
        &xzbbox,
        args.bbox,
        ground.clone(),
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
                // Derive terrain-aware spawn Y while `ground` is still in scope (it gets
                // moved into `generate_world_with_options` below). Used only for Java's
                // post-generation `set_spawn_in_level_dat` call — Bedrock derives spawn Y
                // independently inside `BedrockWriter::write_level_dat`.
                let spawn_y = ground.level(XZPoint::new(
                    spawn_point.0 - xzbbox.min_x(),
                    spawn_point.1 - xzbbox.min_z(),
                )) + 3;
                if let Err(e) = world_utils::set_spawn_in_level_dat(
                    &generation_path,
                    spawn_point.0,
                    spawn_y,
                    spawn_point.1,
                ) {
                    eprintln!(
                        "{} Failed to set spawn point in level.dat: {}",
                        "Warning:".yellow().bold(),
                        e
                    );
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
