#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
mod block_definitions;
mod bresenham;
mod colors;
mod data_processing;
mod element_processing;
mod floodfill;
mod osm_parser;
mod progress;
mod retrieve_data;
mod version_check;
mod world_editor;

use args::Args;
use clap::Parser;
use colored::*;
use fastnbt::Value;
use flate2::read::GzDecoder;
use fs2::FileExt;
use log::{error, LevelFilter};
use rfd::FileDialog;
use std::{
    env,
    fs::{self, File},
    io::{Read, Write},
    panic,
    path::{Path, PathBuf},
};
use tauri_plugin_log::{Builder as LogBuilder, Target, TargetKind};

fn print_banner() {
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
}

fn main() {
    // Parse arguments to decide whether to launch the UI or CLI
    let raw_args: Vec<String> = std::env::args().collect();

    // Check if either `--help` or `--path` is present to run command-line mode
    let is_help: bool = raw_args.iter().any(|arg: &String| arg == "--help");
    let is_path_provided: bool = raw_args
        .iter()
        .any(|arg: &String| arg.starts_with("--path"));

    if is_help || is_path_provided {
        print_banner();

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
        args.run();

        let bbox: Vec<f64> = args
            .bbox
            .as_ref()
            .expect("Bounding box is required")
            .split(',')
            .map(|s: &str| s.parse::<f64>().expect("Invalid bbox coordinate"))
            .collect::<Vec<f64>>();

        let bbox_tuple: (f64, f64, f64, f64) = (bbox[0], bbox[1], bbox[2], bbox[3]);

        // Fetch data
        let raw_data: serde_json::Value =
            retrieve_data::fetch_data(bbox_tuple, args.file.as_deref(), args.debug, "requests")
                .expect("Failed to fetch data");

        // Parse raw data
        let (mut parsed_elements, scale_factor_x, scale_factor_z) =
            osm_parser::parse_osm_data(&raw_data, bbox_tuple, &args);
        parsed_elements.sort_by_key(|element: &osm_parser::ProcessedElement| {
            osm_parser::get_priority(element)
        });

        // Write the parsed OSM data to a file for inspection
        if args.debug {
            let mut output_file: File =
                File::create("parsed_osm_data.txt").expect("Failed to create output file");
            for element in &parsed_elements {
                writeln!(
                    output_file,
                    "Element ID: {}, Type: {}, Tags: {:?}",
                    element.id(),
                    element.kind(),
                    element.tags(),
                )
                .expect("Failed to write to output file");
            }
        }

        // Generate world
        let _ =
            data_processing::generate_world(parsed_elements, &args, scale_factor_x, scale_factor_z);
    } else {
        // Launch the UI
        println!("Launching UI...");

        // Set a custom panic hook to log panic information
        panic::set_hook(Box::new(|panic_info| {
            let message = format!("Application panicked: {:?}", panic_info);
            error!("{}", message);
        }));

        tauri::Builder::default()
            .plugin(
                LogBuilder::default()
                    .level(LevelFilter::Trace)
                    .targets([
                        Target::new(TargetKind::LogDir {
                            file_name: Some("arnis.log".into()),
                        }),
                        Target::new(TargetKind::Stdout),
                    ])
                    .build(),
            )
            .plugin(tauri_plugin_shell::init())
            .invoke_handler(tauri::generate_handler![
                gui_select_world,
                gui_start_generation,
                gui_get_version,
                gui_check_for_updates
            ])
            .setup(|app| {
                let app_handle = app.handle();
                let main_window = tauri::Manager::get_webview_window(app_handle, "main")
                    .expect("Failed to get main window");
                progress::set_main_window(main_window);
                Ok(())
            })
            .run(tauri::generate_context!())
            .expect("Error while starting the application UI (Tauri)");
    }
}

#[tauri::command]
fn gui_select_world(generate_new: bool) -> Result<String, i32> {
    // Determine the default Minecraft 'saves' directory based on the OS
    let default_dir: Option<PathBuf> = if cfg!(target_os = "windows") {
        env::var("APPDATA")
            .ok()
            .map(|appdata: String| PathBuf::from(appdata).join(".minecraft").join("saves"))
    } else if cfg!(target_os = "macos") {
        dirs::home_dir().map(|home: PathBuf| {
            home.join("Library/Application Support/minecraft")
                .join("saves")
        })
    } else if cfg!(target_os = "linux") {
        dirs::home_dir().map(|home: PathBuf| home.join(".minecraft").join("saves"))
    } else {
        None
    };

    if generate_new {
        // Handle new world generation
        if let Some(default_path) = &default_dir {
            if default_path.exists() {
                // Call create_new_world and return the result
                create_new_world(default_path).map_err(|_| 1) // Error code 1: Minecraft directory not found
            } else {
                Err(1) // Error code 1: Minecraft directory not found
            }
        } else {
            Err(1) // Error code 1: Minecraft directory not found
        }
    } else {
        // Handle existing world selection
        // Open the directory picker dialog
        let dialog: FileDialog = FileDialog::new();
        let dialog: FileDialog = if let Some(start_dir) = default_dir.filter(|dir| dir.exists()) {
            dialog.set_directory(start_dir)
        } else {
            dialog
        };

        if let Some(path) = dialog.pick_folder() {
            // Check if the "region" folder exists within the selected directory
            if path.join("region").exists() {
                // Check the 'session.lock' file
                let session_lock_path = path.join("session.lock");
                if session_lock_path.exists() {
                    // Try to acquire a lock on the session.lock file
                    if let Ok(file) = File::open(&session_lock_path) {
                        if file.try_lock_shared().is_err() {
                            return Err(2); // Error code 2: The selected world is currently in use
                        } else {
                            // Release the lock immediately
                            let _ = file.unlock();
                        }
                    }
                }

                return Ok(path.display().to_string());
            } else {
                // No Minecraft directory found, generating new world in custom user selected directory
                return create_new_world(&path).map_err(|_| 3); // Error code 3: Failed to create new world
            }
        }

        // If no folder was selected, return an error message
        Err(4) // Error code 4: No world selected
    }
}

fn create_new_world(base_path: &Path) -> Result<String, String> {
    // Generate a unique world name
    let mut counter: i32 = 1;
    let unique_name: String = loop {
        let candidate_name: String = format!("Arnis World {}", counter);
        let candidate_path: PathBuf = base_path.join(&candidate_name);
        if !candidate_path.exists() {
            break candidate_name;
        }
        counter += 1;
    };

    let new_world_path: PathBuf = base_path.join(&unique_name);

    // Create the new world directory structure
    fs::create_dir_all(new_world_path.join("region"))
        .map_err(|e| format!("Failed to create world directory: {}", e))?;

    // Copy the region template file
    const REGION_TEMPLATE: &[u8] = include_bytes!("../mcassets/region.template");
    let region_path = new_world_path.join("region").join("r.0.0.mca");
    fs::write(&region_path, REGION_TEMPLATE)
        .map_err(|e| format!("Failed to create region file: {}", e))?;

    // Add the level.dat file
    const LEVEL_TEMPLATE: &[u8] = include_bytes!("../mcassets/level.dat");

    // Decompress the gzipped level.template
    let mut decoder = GzDecoder::new(LEVEL_TEMPLATE);
    let mut decompressed_data = Vec::new();
    decoder
        .read_to_end(&mut decompressed_data)
        .map_err(|e| format!("Failed to decompress level.template: {}", e))?;

    // Parse the decompressed NBT data
    let mut level_data: Value = fastnbt::from_bytes(&decompressed_data)
        .map_err(|e| format!("Failed to parse level.dat template: {}", e))?;

    // Modify the LevelName and LastPlayed fields
    if let Value::Compound(ref mut root) = level_data {
        if let Some(Value::Compound(ref mut data)) = root.get_mut("Data") {
            // Update LevelName
            data.insert("LevelName".to_string(), Value::String(unique_name.clone()));

            // Update LastPlayed to the current Unix time in milliseconds
            let current_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| format!("Failed to get current time: {}", e))?;
            let current_time_millis = current_time.as_millis() as i64;
            data.insert("LastPlayed".to_string(), Value::Long(current_time_millis));
        }
    }

    // Serialize the updated NBT data back to bytes
    let serialized_level_data: Vec<u8> = fastnbt::to_bytes(&level_data)
        .map_err(|e| format!("Failed to serialize updated level.dat: {}", e))?;

    // Compress the serialized data back to gzip
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized_level_data)
        .map_err(|e| format!("Failed to compress updated level.dat: {}", e))?;
    let compressed_level_data = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize compression for level.dat: {}", e))?;

    // Write the level.dat file
    fs::write(new_world_path.join("level.dat"), compressed_level_data)
        .map_err(|e| format!("Failed to create level.dat file: {}", e))?;

    // Add the icon.png file
    const ICON_TEMPLATE: &[u8] = include_bytes!("../mcassets/icon.png");
    fs::write(new_world_path.join("icon.png"), ICON_TEMPLATE)
        .map_err(|e| format!("Failed to create icon.png file: {}", e))?;

    Ok(new_world_path.display().to_string())
}

#[tauri::command]
fn gui_get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
fn gui_check_for_updates() -> Result<bool, String> {
    match version_check::check_for_updates() {
        Ok(is_newer) => Ok(is_newer),
        Err(e) => Err(format!("Error checking for updates: {}", e)),
    }
}

#[tauri::command]
fn gui_start_generation(
    bbox_text: String,
    selected_world: String,
    world_scale: f64,
    ground_level: i32,
    winter_mode: bool,
    floodfill_timeout: u64,
) -> Result<(), String> {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = tokio::task::spawn_blocking(move || {
            // Utility function to reorder bounding box coordinates
            fn reorder_bbox(bbox: &[f64]) -> (f64, f64, f64, f64) {
                (bbox[1], bbox[0], bbox[3], bbox[2])
            }

            // Parse bounding box string and validate it
            let bbox: Vec<f64> = bbox_text
                .split_whitespace()
                .map(|s| s.parse::<f64>().expect("Invalid bbox coordinate"))
                .collect();

            if bbox.len() != 4 {
                return Err("Invalid bounding box format".to_string());
            }

            // Create an Args instance with the chosen bounding box and world directory path
            let args: Args = Args {
                bbox: Some(bbox_text),
                file: None,
                path: selected_world,
                downloader: "requests".to_string(),
                scale: world_scale,
                ground_level,
                winter: winter_mode,
                debug: false,
                timeout: Some(std::time::Duration::from_secs(floodfill_timeout)),
            };

            // Reorder bounding box coordinates for further processing
            let reordered_bbox: (f64, f64, f64, f64) = reorder_bbox(&bbox);

            // Run data fetch and world generation
            match retrieve_data::fetch_data(reordered_bbox, None, args.debug, "requests") {
                Ok(raw_data) => {
                    let (mut parsed_elements, scale_factor_x, scale_factor_z) =
                        osm_parser::parse_osm_data(&raw_data, reordered_bbox, &args);
                    parsed_elements.sort_by_key(|element: &osm_parser::ProcessedElement| {
                        osm_parser::get_priority(element)
                    });

                    let _ = data_processing::generate_world(
                        parsed_elements,
                        &args,
                        scale_factor_x,
                        scale_factor_z,
                    );
                    Ok(())
                }
                Err(e) => Err(format!("Failed to start generation: {}", e)),
            }
        })
        .await
        {
            eprintln!("Error in blocking task: {}", e);
        }
    });

    Ok(())
}
