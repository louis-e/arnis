#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
mod bbox;
mod block_definitions;
mod bresenham;
mod colors;
mod coordinate_system;
mod data_processing;
mod element_processing;
mod floodfill;
mod geo_coord;
mod ground;
mod map_transformation;
mod osm_parser;
#[cfg(feature = "gui")]
mod progress;
// If the user does not want the GUI, it's easiest to just mock the progress module to do nothing
#[cfg(not(feature = "gui"))]
mod progress {
    pub fn emit_gui_error(_message: &str) {}
    pub fn emit_gui_progress_update(_progress: f64, _message: &str) {}
    pub fn is_running_with_gui() -> bool {
        false
    }
}
mod retrieve_data;
#[cfg(test)]
mod test_utilities;
mod version_check;
mod world_editor;

use args::Args;
use clap::Parser;
use colored::*;
use coordinate_system::cartesian::XZBBox;
#[cfg(feature = "gui")]
use fastnbt::Value;
#[cfg(feature = "gui")]
use flate2::read::GzDecoder;
#[cfg(feature = "gui")]
use log::{error, LevelFilter};
#[cfg(feature = "gui")]
use rfd::FileDialog;
#[cfg(feature = "gui")]
use std::io::Read;
#[cfg(feature = "gui")]
use std::path::{Path, PathBuf};
use std::{env, fs, io::Write, panic};
#[cfg(feature = "gui")]
use tauri_plugin_log::{Builder as LogBuilder, Target, TargetKind};
#[cfg(target_os = "windows")]
use windows::Win32::System::Console::{AttachConsole, FreeConsole, ATTACH_PARENT_PROCESS};

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
    // If on Windows, free and reattach to the parent console when using as a CLI tool
    // Either of these can fail, but if they do it is not an issue, so the return value is ignored
    #[cfg(target_os = "windows")]
    unsafe {
        let _ = FreeConsole();
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }

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

        // Fetch data
        let raw_data: serde_json::Value =
            retrieve_data::fetch_data(args.bbox, args.file.as_deref(), args.debug, "requests")
                .expect("Failed to fetch data");

        // Parse raw data
        let (mut parsed_elements, scale_factor_x, scale_factor_z) =
            osm_parser::parse_osm_data(&raw_data, args.bbox, args.scale, args.debug);
        parsed_elements.sort_by_key(|element: &osm_parser::ProcessedElement| {
            osm_parser::get_priority(element)
        });

        // Write the parsed OSM data to a file for inspection
        if args.debug {
            let mut output_file: fs::File =
                fs::File::create("parsed_osm_data.txt").expect("Failed to create output file");
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

        let mut xzbbox = XZBBox::rect_from_xz_lengths(scale_factor_x, scale_factor_z)
            .expect("Parsed world lengths < 1");

        // Edit map (parsed_elements). Operations are defined in a json file, if exists
        map_transformation::transform_map(&mut parsed_elements, &mut xzbbox);

        // Generate world
        let _ = data_processing::generate_world(parsed_elements, xzbbox, &args);
    } else {
        #[cfg(not(feature = "gui"))]
        {
            panic!("This version of arnis was not built with GUI enabled");
        }

        #[cfg(feature = "gui")]
        {
            // Launch the UI
            println!("Launching UI...");

            // Set a custom panic hook to log panic information
            panic::set_hook(Box::new(|panic_info| {
                let message = format!("Application panicked: {:?}", panic_info);
                error!("{}", message);
                std::process::exit(1);
            }));

            // Workaround WebKit2GTK issue with NVIDIA drivers (likely explicit sync related?)
            // Source: https://github.com/tauri-apps/tauri/issues/10702 (TODO: Remove this later)
            #[cfg(target_os = "linux")]
            unsafe {
                env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
            }

            tauri::Builder::default()
                .plugin(
                    LogBuilder::default()
                        .level(LevelFilter::Warn)
                        .targets([
                            Target::new(TargetKind::LogDir {
                                file_name: Some("arnis".into()),
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
}

#[cfg(feature = "gui")]
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
        dirs::home_dir().map(|home| {
            let flatpak_path = home.join(".var/app/com.mojang.Minecraft/.minecraft/saves");
            if flatpak_path.exists() {
                flatpak_path
            } else {
                home.join(".minecraft/saves")
            }
        })
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
                    if let Ok(file) = fs::File::open(&session_lock_path) {
                        if fs2::FileExt::try_lock_shared(&file).is_err() {
                            return Err(2); // Error code 2: The selected world is currently in use
                        } else {
                            // Release the lock immediately
                            let _ = fs2::FileExt::unlock(&file);
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

#[cfg(feature = "gui")]
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

    // Modify the LevelName, LastPlayed and player position fields
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

            // Update player position and rotation
            if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
                if let Some(Value::List(ref mut pos)) = player.get_mut("Pos") {
                    if let Value::Double(ref mut x) = pos.get_mut(0).unwrap() {
                        *x = -5.0;
                    }
                    if let Value::Double(ref mut y) = pos.get_mut(1).unwrap() {
                        *y = -61.0;
                    }
                    if let Value::Double(ref mut z) = pos.get_mut(2).unwrap() {
                        *z = -5.0;
                    }
                }

                if let Some(Value::List(ref mut rot)) = player.get_mut("Rotation") {
                    if let Value::Float(ref mut x) = rot.get_mut(0).unwrap() {
                        *x = -45.0;
                    }
                }
            }
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

#[cfg(feature = "gui")]
#[tauri::command]
fn gui_get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(feature = "gui")]
#[tauri::command]
fn gui_check_for_updates() -> Result<bool, String> {
    match version_check::check_for_updates() {
        Ok(is_newer) => Ok(is_newer),
        Err(e) => Err(format!("Error checking for updates: {}", e)),
    }
}

#[cfg(feature = "gui")]
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn gui_start_generation(
    bbox_text: String,
    selected_world: String,
    world_scale: f64,
    ground_level: i32,
    floodfill_timeout: u64,
    terrain_enabled: bool,
    fillground_enabled: bool,
) -> Result<(), String> {
    use bbox::BBox;
    use progress::emit_gui_error;

    tauri::async_runtime::spawn(async move {
        if let Err(e) = tokio::task::spawn_blocking(move || {
            // Parse the bounding box from the text with proper error handling
            let bbox = match BBox::from_str(&bbox_text) {
                Ok(bbox) => bbox,
                Err(e) => {
                    let error_msg = format!("Failed to parse bounding box: {}", e);
                    eprintln!("{}", error_msg);
                    emit_gui_error(&error_msg);
                    return Err(error_msg);
                }
            };

            // Create an Args instance with the chosen bounding box and world directory path
            let args: Args = Args {
                bbox,
                file: None,
                path: selected_world,
                downloader: "requests".to_string(),
                scale: world_scale,
                ground_level,
                terrain: terrain_enabled,
                fillground: fillground_enabled,
                debug: false,
                timeout: Some(std::time::Duration::from_secs(floodfill_timeout)),
            };

            // Run data fetch and world generation
            match retrieve_data::fetch_data(args.bbox, None, args.debug, "requests") {
                Ok(raw_data) => {
                    let (mut parsed_elements, scale_factor_x, scale_factor_z) =
                        osm_parser::parse_osm_data(&raw_data, args.bbox, args.scale, args.debug);
                    parsed_elements.sort_by(|el1, el2| {
                        let (el1_priority, el2_priority) =
                            (osm_parser::get_priority(el1), osm_parser::get_priority(el2));
                        match (
                            el1.tags().contains_key("landuse"),
                            el2.tags().contains_key("landuse"),
                        ) {
                            (true, false) => std::cmp::Ordering::Greater,
                            (false, true) => std::cmp::Ordering::Less,
                            _ => el1_priority.cmp(&el2_priority),
                        }
                    });

                    let mut xzbbox = XZBBox::rect_from_xz_lengths(scale_factor_x, scale_factor_z)
                        .expect("Parsed world lengths < 1");

                    // Edit map (parsed_elements). Operations are defined in a json file, if exists
                    map_transformation::transform_map(&mut parsed_elements, &mut xzbbox);

                    let _ = data_processing::generate_world(parsed_elements, xzbbox, &args);
                    Ok(())
                }
                Err(e) => {
                    let error_msg = format!("Failed to fetch data: {}", e);
                    emit_gui_error(&error_msg);
                    Err(error_msg)
                }
            }
        })
        .await
        {
            let error_msg = format!("Error in generation task: {}", e);
            eprintln!("{}", error_msg);
            emit_gui_error(&error_msg);
        }
    });

    Ok(())
}
