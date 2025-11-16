use crate::args::Args;
use crate::coordinate_system::cartesian::XZPoint;
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer;
use crate::data_processing;
use crate::ground::{self, Ground};
use crate::map_transformation;
use crate::osm_parser;
use crate::progress;
use crate::retrieve_data;
use crate::telemetry;
use crate::version_check;
use fastnbt::Value;
use flate2::read::GzDecoder;
use fs2::FileExt;
use log::LevelFilter;
use rfd::FileDialog;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::{env, fs, io::Write};
use tauri_plugin_log::{Builder as LogBuilder, Target, TargetKind};

/// Manages the session.lock file for a Minecraft world directory
struct SessionLock {
    file: fs::File,
    path: PathBuf,
}

impl SessionLock {
    /// Creates and locks a session.lock file in the specified world directory
    fn acquire(world_path: &Path) -> Result<Self, String> {
        let session_lock_path = world_path.join("session.lock");

        // Create or open the session.lock file
        let file = fs::File::create(&session_lock_path)
            .map_err(|e| format!("Failed to create session.lock file: {e}"))?;

        // Write the snowman character (U+2603) as specified by Minecraft format
        let snowman_bytes = "☃".as_bytes(); // This is UTF-8 encoded E2 98 83
        (&file)
            .write_all(snowman_bytes)
            .map_err(|e| format!("Failed to write to session.lock file: {e}"))?;

        // Acquire an exclusive lock on the file
        file.try_lock_exclusive()
            .map_err(|e| format!("Failed to acquire lock on session.lock file: {e}"))?;

        Ok(SessionLock {
            file,
            path: session_lock_path,
        })
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        // Release the lock and remove the session.lock file
        let _ = self.file.unlock();
        let _ = fs::remove_file(&self.path);
    }
}

pub fn run_gui() {
    // Launch the UI
    println!("Launching UI...");

    // Install panic hook for crash reporting
    telemetry::install_panic_hook();

    // Workaround WebKit2GTK issue with NVIDIA drivers and graphics issues
    // Source: https://github.com/tauri-apps/tauri/issues/10702
    #[cfg(target_os = "linux")]
    unsafe {
        // Disable problematic GPU features that cause map loading issues
        env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");

        // Force software rendering for better compatibility
        env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
        env::set_var("GALLIUM_DRIVER", "softpipe");

        // Note: Removed sandbox disabling for security reasons
        // Note: Removed Qt WebEngine flags as they don't apply to Tauri
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

fn create_new_world(base_path: &Path) -> Result<String, String> {
    // Generate a unique world name with proper counter
    // Check for both "Arnis World X" and "Arnis World X: Location" patterns
    let mut counter: i32 = 1;
    let unique_name: String = loop {
        let candidate_name: String = format!("Arnis World {counter}");
        let candidate_path: PathBuf = base_path.join(&candidate_name);

        // Check for exact match (no location suffix)
        let exact_match_exists = candidate_path.exists();

        // Check for worlds with location suffix (Arnis World X: Location)
        let location_pattern = format!("Arnis World {counter}: ");
        let location_match_exists = fs::read_dir(base_path)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .any(|name| name.starts_with(&location_pattern))
            })
            .unwrap_or(false);

        if !exact_match_exists && !location_match_exists {
            break candidate_name;
        }
        counter += 1;
    };

    let new_world_path: PathBuf = base_path.join(&unique_name);

    // Create the new world directory structure
    fs::create_dir_all(new_world_path.join("region"))
        .map_err(|e| format!("Failed to create world directory: {e}"))?;

    // Copy the region template file
    const REGION_TEMPLATE: &[u8] = include_bytes!("../assets/minecraft/region.template");
    let region_path = new_world_path.join("region").join("r.0.0.mca");
    fs::write(&region_path, REGION_TEMPLATE)
        .map_err(|e| format!("Failed to create region file: {e}"))?;

    // Add the level.dat file
    const LEVEL_TEMPLATE: &[u8] = include_bytes!("../assets/minecraft/level.dat");

    // Decompress the gzipped level.template
    let mut decoder = GzDecoder::new(LEVEL_TEMPLATE);
    let mut decompressed_data = Vec::new();
    decoder
        .read_to_end(&mut decompressed_data)
        .map_err(|e| format!("Failed to decompress level.template: {e}"))?;

    // Parse the decompressed NBT data
    let mut level_data: Value = fastnbt::from_bytes(&decompressed_data)
        .map_err(|e| format!("Failed to parse level.dat template: {e}"))?;

    // Modify the LevelName, LastPlayed and player position fields
    if let Value::Compound(ref mut root) = level_data {
        if let Some(Value::Compound(ref mut data)) = root.get_mut("Data") {
            // Update LevelName
            data.insert("LevelName".to_string(), Value::String(unique_name.clone()));

            // Update LastPlayed to the current Unix time in milliseconds
            let current_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| format!("Failed to get current time: {e}"))?;
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
        .map_err(|e| format!("Failed to serialize updated level.dat: {e}"))?;

    // Compress the serialized data back to gzip
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized_level_data)
        .map_err(|e| format!("Failed to compress updated level.dat: {e}"))?;
    let compressed_level_data = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize compression for level.dat: {e}"))?;

    // Write the level.dat file
    fs::write(new_world_path.join("level.dat"), compressed_level_data)
        .map_err(|e| format!("Failed to create level.dat file: {e}"))?;

    // Add the icon.png file
    const ICON_TEMPLATE: &[u8] = include_bytes!("../assets/minecraft/icon.png");
    fs::write(new_world_path.join("icon.png"), ICON_TEMPLATE)
        .map_err(|e| format!("Failed to create icon.png file: {e}"))?;

    Ok(new_world_path.display().to_string())
}

/// Adds localized area name to the world name in level.dat
fn add_localized_world_name(world_path: PathBuf, bbox: &LLBBox) -> PathBuf {
    // Only proceed if the path exists
    if !world_path.exists() {
        return world_path;
    }

    // Check the level.dat file first to get the current name
    let level_path = world_path.join("level.dat");

    if !level_path.exists() {
        return world_path;
    }

    // Try to read the current world name from level.dat
    let Ok(level_data) = std::fs::read(&level_path) else {
        return world_path;
    };

    let mut decoder = GzDecoder::new(level_data.as_slice());
    let mut decompressed_data = Vec::new();
    if decoder.read_to_end(&mut decompressed_data).is_err() {
        return world_path;
    }

    let Ok(Value::Compound(ref root)) = fastnbt::from_bytes::<Value>(&decompressed_data) else {
        return world_path;
    };

    let Some(Value::Compound(ref data)) = root.get("Data") else {
        return world_path;
    };

    let Some(Value::String(current_name)) = data.get("LevelName") else {
        return world_path;
    };

    // Only modify if it's an Arnis world and doesn't already have an area name
    if !current_name.starts_with("Arnis World ") || current_name.contains(": ") {
        return world_path;
    }

    // Calculate center coordinates of bbox
    let center_lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
    let center_lon = (bbox.min().lng() + bbox.max().lng()) / 2.0;

    // Try to fetch the area name
    let area_name = match retrieve_data::fetch_area_name(center_lat, center_lon) {
        Ok(Some(name)) => name,
        _ => return world_path, // Keep original name if no area name found
    };

    // Create new name with localized area name, ensuring total length doesn't exceed 30 characters
    let base_name = current_name.clone();
    let max_area_name_len = 30 - base_name.len() - 2; // 2 chars for ": "

    let truncated_area_name =
        if area_name.chars().count() > max_area_name_len && max_area_name_len > 0 {
            // Truncate the area name to fit within the 30 character limit
            area_name
                .chars()
                .take(max_area_name_len)
                .collect::<String>()
        } else if max_area_name_len == 0 {
            // If base name is already too long, don't add area name
            return world_path;
        } else {
            area_name
        };

    let new_name = format!("{base_name}: {truncated_area_name}");

    // Update the level.dat file with the new name
    if let Ok(level_data) = std::fs::read(&level_path) {
        let mut decoder = GzDecoder::new(level_data.as_slice());
        let mut decompressed_data = Vec::new();
        if decoder.read_to_end(&mut decompressed_data).is_ok() {
            if let Ok(mut nbt_data) = fastnbt::from_bytes::<Value>(&decompressed_data) {
                // Update the level name in NBT data
                if let Value::Compound(ref mut root) = nbt_data {
                    if let Some(Value::Compound(ref mut data)) = root.get_mut("Data") {
                        data.insert("LevelName".to_string(), Value::String(new_name));

                        // Save the updated NBT data
                        if let Ok(serialized_data) = fastnbt::to_bytes(&nbt_data) {
                            let mut encoder = flate2::write::GzEncoder::new(
                                Vec::new(),
                                flate2::Compression::default(),
                            );
                            if encoder.write_all(&serialized_data).is_ok() {
                                if let Ok(compressed_data) = encoder.finish() {
                                    if let Err(e) = std::fs::write(&level_path, compressed_data) {
                                        eprintln!("Failed to update level.dat with area name: {e}");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Return the original path since we didn't change the directory name
    world_path
}

// Function to update player position in level.dat based on spawn point coordinates
fn update_player_position(
    world_path: &str,
    spawn_point: Option<(f64, f64)>,
    bbox_text: String,
    scale: f64,
) -> Result<(), String> {
    use crate::coordinate_system::transformation::CoordTransformer;

    let Some((lat, lng)) = spawn_point else {
        return Ok(()); // No spawn point selected, exit early
    };

    // Parse geometrical point and bounding box
    let llpoint =
        LLPoint::new(lat, lng).map_err(|e| format!("Failed to parse spawn point:\n{e}"))?;
    let llbbox = LLBBox::from_str(&bbox_text)
        .map_err(|e| format!("Failed to parse bounding box for spawn point:\n{e}"))?;

    // Check if spawn point is within the bbox
    if !llbbox.contains(&llpoint) {
        return Err("Spawn point is outside the selected area".to_string());
    }

    // Convert lat/lng to Minecraft coordinates
    let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(&llbbox, scale)
        .map_err(|e| format!("Failed to build transformation on coordinate systems:\n{e}"))?;

    let xzpoint = transformer.transform_point(llpoint);

    // Default y spawn position since terrain elevation cannot be determined yet
    let y = 150.0;

    // Read and update the level.dat file
    let level_path = PathBuf::from(world_path).join("level.dat");
    if !level_path.exists() {
        return Err(format!("Level.dat not found at {level_path:?}"));
    }

    // Read the level.dat file
    let level_data = match std::fs::read(&level_path) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to read level.dat: {e}")),
    };

    // Decompress and parse the NBT data
    let mut decoder = GzDecoder::new(level_data.as_slice());
    let mut decompressed_data = Vec::new();
    if let Err(e) = decoder.read_to_end(&mut decompressed_data) {
        return Err(format!("Failed to decompress level.dat: {e}"));
    }

    let mut nbt_data = match fastnbt::from_bytes::<Value>(&decompressed_data) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to parse level.dat NBT data: {e}")),
    };

    // Update player position and world spawn point
    if let Value::Compound(ref mut root) = nbt_data {
        if let Some(Value::Compound(ref mut data)) = root.get_mut("Data") {
            // Set world spawn point
            data.insert("SpawnX".to_string(), Value::Int(xzpoint.x));
            data.insert("SpawnY".to_string(), Value::Int(y as i32));
            data.insert("SpawnZ".to_string(), Value::Int(xzpoint.z));

            // Update player position
            if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
                if let Some(Value::List(ref mut pos)) = player.get_mut("Pos") {
                    if let Value::Double(ref mut pos_x) = pos.get_mut(0).unwrap() {
                        *pos_x = xzpoint.x as f64;
                    }
                    if let Value::Double(ref mut pos_y) = pos.get_mut(1).unwrap() {
                        *pos_y = y;
                    }
                    if let Value::Double(ref mut pos_z) = pos.get_mut(2).unwrap() {
                        *pos_z = xzpoint.z as f64;
                    }
                }
            }
        }
    }

    // Serialize and save the updated level.dat
    let serialized_data = match fastnbt::to_bytes(&nbt_data) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to serialize updated level.dat: {e}")),
    };

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    if let Err(e) = encoder.write_all(&serialized_data) {
        return Err(format!("Failed to compress updated level.dat: {e}"));
    }

    let compressed_data = match encoder.finish() {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to finalize compression for level.dat: {e}")),
    };

    // Write the updated level.dat file
    if let Err(e) = std::fs::write(level_path, compressed_data) {
        return Err(format!("Failed to write updated level.dat: {e}"));
    }

    Ok(())
}

// Function to update player spawn Y coordinate based on terrain height after generation
pub fn update_player_spawn_y_after_generation(
    world_path: &Path,
    spawn_point: Option<(f64, f64)>,
    bbox_text: String,
    scale: f64,
    ground: &Ground,
) -> Result<(), String> {
    use crate::coordinate_system::transformation::CoordTransformer;

    let Some((_lat, _lng)) = spawn_point else {
        return Ok(()); // No spawn point selected, exit early
    };

    // Read the current level.dat file to get existing spawn coordinates
    let level_path = PathBuf::from(world_path).join("level.dat");
    if !level_path.exists() {
        return Err(format!("Level.dat not found at {level_path:?}"));
    }

    // Read the level.dat file
    let level_data = match std::fs::read(&level_path) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to read level.dat: {e}")),
    };

    // Decompress and parse the NBT data
    let mut decoder = GzDecoder::new(level_data.as_slice());
    let mut decompressed_data = Vec::new();
    if let Err(e) = decoder.read_to_end(&mut decompressed_data) {
        return Err(format!("Failed to decompress level.dat: {e}"));
    }

    let mut nbt_data = match fastnbt::from_bytes::<Value>(&decompressed_data) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to parse level.dat NBT data: {e}")),
    };

    // Get existing spawn coordinates and calculate new Y based on terrain
    let (existing_spawn_x, existing_spawn_z) = if let Value::Compound(ref root) = nbt_data {
        if let Some(Value::Compound(ref data)) = root.get("Data") {
            let spawn_x = data.get("SpawnX").and_then(|v| {
                if let Value::Int(x) = v {
                    Some(*x)
                } else {
                    None
                }
            });
            let spawn_z = data.get("SpawnZ").and_then(|v| {
                if let Value::Int(z) = v {
                    Some(*z)
                } else {
                    None
                }
            });

            match (spawn_x, spawn_z) {
                (Some(x), Some(z)) => (x, z),
                _ => {
                    return Err("Spawn coordinates not found in level.dat".to_string());
                }
            }
        } else {
            return Err("Invalid level.dat structure: no Data compound".to_string());
        }
    } else {
        return Err("Invalid level.dat structure: root is not a compound".to_string());
    };

    // Calculate terrain-based Y coordinate
    let spawn_y = if ground.elevation_enabled {
        // Parse coordinates for terrain lookup
        let llbbox = LLBBox::from_str(&bbox_text)
            .map_err(|e| format!("Failed to parse bounding box for spawn point:\n{e}"))?;
        let (_, xzbbox) = CoordTransformer::llbbox_to_xzbbox(&llbbox, scale)
            .map_err(|e| format!("Failed to build transformation:\n{e}"))?;

        // Calculate relative coordinates for ground system
        let relative_x = existing_spawn_x - xzbbox.min_x();
        let relative_z = existing_spawn_z - xzbbox.min_z();
        let terrain_point = XZPoint::new(relative_x, relative_z);

        ground.level(terrain_point) + 2
    } else {
        -61 // Default Y if no terrain
    };

    // Update player position and world spawn point
    if let Value::Compound(ref mut root) = nbt_data {
        if let Some(Value::Compound(ref mut data)) = root.get_mut("Data") {
            // Only update the Y coordinate, keep existing X and Z
            data.insert("SpawnY".to_string(), Value::Int(spawn_y));

            // Update player position - only Y coordinate
            if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
                if let Some(Value::List(ref mut pos)) = player.get_mut("Pos") {
                    // Keep existing X and Z, only update Y
                    if let Value::Double(ref mut pos_y) = pos.get_mut(1).unwrap() {
                        *pos_y = spawn_y as f64;
                    }
                }
            }
        }
    }

    // Serialize and save the updated level.dat
    let serialized_data = match fastnbt::to_bytes(&nbt_data) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to serialize updated level.dat: {e}")),
    };

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    if let Err(e) = encoder.write_all(&serialized_data) {
        return Err(format!("Failed to compress updated level.dat: {e}"));
    }

    let compressed_data = match encoder.finish() {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to finalize compression for level.dat: {e}")),
    };

    // Write the updated level.dat file
    if let Err(e) = std::fs::write(level_path, compressed_data) {
        return Err(format!("Failed to write updated level.dat: {e}"));
    }

    Ok(())
}

#[tauri::command]
fn gui_get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
fn gui_check_for_updates() -> Result<bool, String> {
    match version_check::check_for_updates() {
        Ok(is_newer) => Ok(is_newer),
        Err(e) => Err(format!("Error checking for updates: {e}")),
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
#[allow(unused_variables)]
fn gui_start_generation(
    bbox_text: String,
    selected_world: String,
    world_scale: f64,
    ground_level: i32,
    floodfill_timeout: u64,
    terrain_enabled: bool,
    skip_osm_objects: bool,
    interior_enabled: bool,
    roof_enabled: bool,
    fillground_enabled: bool,
    is_new_world: bool,
    spawn_point: Option<(f64, f64)>,
    telemetry_consent: bool,
) -> Result<(), String> {
    use progress::emit_gui_error;
    use LLBBox;

    // Store telemetry consent for crash reporting
    telemetry::set_telemetry_consent(telemetry_consent);

    // If spawn point was chosen and the world is new, check and set the spawn point
    if is_new_world && spawn_point.is_some() {
        // Verify the spawn point is within bounds
        if let Some(coords) = spawn_point {
            let llbbox = match LLBBox::from_str(&bbox_text) {
                Ok(bbox) => bbox,
                Err(e) => {
                    let error_msg = format!("Failed to parse bounding box: {e}");
                    eprintln!("{error_msg}");
                    emit_gui_error(&error_msg);
                    return Err(error_msg);
                }
            };

            let llpoint = LLPoint::new(coords.0, coords.1)
                .map_err(|e| format!("Failed to parse spawn point: {e}"))?;

            if llbbox.contains(&llpoint) {
                // Spawn point is valid, update the player position
                update_player_position(
                    &selected_world,
                    spawn_point,
                    bbox_text.clone(),
                    world_scale,
                )
                .map_err(|e| format!("Failed to set spawn point: {e}"))?;
            }
        }
    }

    tauri::async_runtime::spawn(async move {
        if let Err(e) = tokio::task::spawn_blocking(move || {
            // Acquire session lock for the world directory before starting generation
            let world_path = PathBuf::from(&selected_world);
            let _session_lock = match SessionLock::acquire(&world_path) {
                Ok(lock) => lock,
                Err(e) => {
                    let error_msg = format!("Failed to acquire session lock: {e}");
                    eprintln!("{error_msg}");
                    emit_gui_error(&error_msg);
                    return Err(error_msg);
                }
            };

            // Parse the bounding box from the text with proper error handling
            let bbox = match LLBBox::from_str(&bbox_text) {
                Ok(bbox) => bbox,
                Err(e) => {
                    let error_msg = format!("Failed to parse bounding box: {e}");
                    eprintln!("{error_msg}");
                    emit_gui_error(&error_msg);
                    return Err(error_msg);
                }
            };

            // Add localized name to the world if user generated a new world
            let updated_world_path = if is_new_world {
                add_localized_world_name(world_path, &bbox)
            } else {
                world_path
            };

            // Create an Args instance with the chosen bounding box and world directory path
            let args: Args = Args {
                bbox,
                file: None,
                save_json_file: None,
                path: updated_world_path,
                downloader: "requests".to_string(),
                scale: world_scale,
                ground_level,
                terrain: terrain_enabled,
                interior: interior_enabled,
                roof: roof_enabled,
                fillground: fillground_enabled,
                debug: false,
                timeout: Some(std::time::Duration::from_secs(floodfill_timeout)),
                spawn_point,
                telemetry_consent,
            };

            // If skip_osm_objects is true (terrain-only mode), skip fetching and processing OSM data
            if skip_osm_objects {
                // Generate ground data (terrain) for terrain-only mode
                let ground = ground::generate_ground_data(&args);

                // Create empty parsed_elements and xzbbox for terrain-only mode
                let parsed_elements = Vec::new();
                let (_coord_transformer, xzbbox) =
                    CoordTransformer::llbbox_to_xzbbox(&args.bbox, args.scale)
                        .map_err(|e| format!("Failed to create coordinate transformer: {}", e))?;

                let _ = data_processing::generate_world(
                    parsed_elements,
                    xzbbox,
                    args.bbox,
                    ground,
                    &args,
                );
                // Session lock will be automatically released when _session_lock goes out of scope
                return Ok(());
            }

            // Run data fetch and world generation (standard mode: objects + terrain, or objects only)
            match retrieve_data::fetch_data_from_overpass(args.bbox, args.debug, "requests", None) {
                Ok(raw_data) => {
                    let (mut parsed_elements, mut xzbbox) =
                        osm_parser::parse_osm_data(raw_data, args.bbox, args.scale, args.debug);
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

                    let mut ground = ground::generate_ground_data(&args);

                    // Transform map (parsed_elements). Operations are defined in a json file
                    map_transformation::transform_map(
                        &mut parsed_elements,
                        &mut xzbbox,
                        &mut ground,
                    );

                    let _ = data_processing::generate_world(
                        parsed_elements,
                        xzbbox,
                        args.bbox,
                        ground,
                        &args,
                    );
                    // Session lock will be automatically released when _session_lock goes out of scope
                    Ok(())
                }
                Err(e) => {
                    let error_msg = format!("Failed to fetch data: {e}");
                    emit_gui_error(&error_msg);
                    // Session lock will be automatically released when _session_lock goes out of scope
                    Err(error_msg)
                }
            }
        })
        .await
        {
            let error_msg = format!("Error in blocking task: {e}");
            eprintln!("{error_msg}");
            emit_gui_error(&error_msg);
            // Session lock will be automatically released when the task fails
        }
    });

    Ok(())
}
