use crate::coordinate_system::geographic::LLBBox;
use crate::retrieve_data;
use fastnbt::Value;
use flate2::read::GzDecoder;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::{fs, io::Write};

/// Returns the Desktop directory for Bedrock .mcworld file output.
/// Falls back to home directory, then current directory.
pub fn get_bedrock_output_directory() -> PathBuf {
    dirs::desktop_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Returns Luanti's worlds directory for the current OS.
/// Windows: %APPDATA%\Minetest\worlds
/// macOS:   ~/Library/Application Support/minetest/worlds
/// Linux:   ~/.minetest/worlds
/// Falls back to Desktop/Arnis Luanti Worlds if no path can be resolved.
pub fn get_luanti_worlds_directory() -> PathBuf {
    let base = if cfg!(target_os = "windows") {
        dirs::data_dir().map(|p| p.join("Minetest"))
    } else if cfg!(target_os = "macos") {
        dirs::data_dir().map(|p| p.join("minetest"))
    } else {
        dirs::home_dir().map(|p| p.join(".minetest"))
    };

    base.map(|p| p.join("worlds")).unwrap_or_else(|| {
        dirs::desktop_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Arnis Luanti Worlds")
    })
}

/// Gets the area name for a given bounding box using the center point.
pub fn get_area_name_for_bedrock(bbox: &LLBBox) -> String {
    let center_lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
    let center_lon = (bbox.min().lng() + bbox.max().lng()) / 2.0;

    match retrieve_data::fetch_area_name(center_lat, center_lon) {
        Ok(Some(name)) => name,
        _ => "Unknown Location".to_string(),
    }
}

/// Sanitizes an area name for safe use in filesystem paths.
/// Replaces characters that are invalid on Windows/macOS/Linux, trims whitespace,
/// and limits length to prevent excessively long filenames.
pub fn sanitize_for_filename(name: &str) -> String {
    let invalid_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let mut sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_control() || invalid_chars.contains(&c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    sanitized = sanitized.trim().to_string();

    // Limit length to avoid excessively long filenames
    const MAX_LEN: usize = 64;
    if sanitized.len() > MAX_LEN {
        // Find a valid UTF-8 char boundary at or before MAX_LEN bytes
        let cutoff = sanitized
            .char_indices()
            .take_while(|(idx, _)| *idx < MAX_LEN)
            .last()
            .map(|(idx, ch)| idx + ch.len_utf8())
            .unwrap_or(0);
        sanitized.truncate(cutoff);
        sanitized = sanitized.trim_end().to_string();
    }

    if sanitized.is_empty() {
        "Unknown Location".to_string()
    } else {
        sanitized
    }
}

/// Returns a trimmed custom name, or None if the input is missing or blank.
fn normalize_custom_name(custom_name: Option<&str>) -> Option<&str> {
    custom_name.map(str::trim).filter(|s| !s.is_empty())
}

/// Returns a world name that doesn't collide with an existing entry in
/// `base_dir` by appending an increasing counter (`Name`, `Name 2`, `Name 3`, ...).
pub fn unique_world_name(base_dir: &Path, desired: &str) -> String {
    if !base_dir.join(desired).exists() {
        return desired.to_string();
    }
    let mut counter = 2;
    loop {
        let candidate = format!("{desired} {counter}");
        if !base_dir.join(&candidate).exists() {
            return candidate;
        }
        counter += 1;
    }
}

/// Picks the Luanti world directory name: the sanitized custom name if given,
/// otherwise the next free "Arnis Luanti World N".
pub fn luanti_world_name(worlds_dir: &Path, custom_name: Option<&str>) -> String {
    if let Some(name) = normalize_custom_name(custom_name) {
        return unique_world_name(worlds_dir, &sanitize_for_filename(name));
    }
    let mut counter = 1;
    loop {
        let candidate = format!("Arnis Luanti World {counter}");
        if !worlds_dir.join(&candidate).exists() {
            return candidate;
        }
        counter += 1;
    }
}

/// Builds the Bedrock output path and level name for a given bounding box.
/// Uses the custom name if given, otherwise combines area name lookup,
/// sanitization, and path construction.
pub fn build_bedrock_output(
    bbox: &LLBBox,
    output_dir: PathBuf,
    custom_name: Option<&str>,
) -> (PathBuf, String) {
    if let Some(name) = normalize_custom_name(custom_name) {
        let safe_name = sanitize_for_filename(name);
        let filename = format!("{safe_name}.mcworld");
        return (output_dir.join(&filename), name.to_string());
    }
    let area_name = get_area_name_for_bedrock(bbox);
    let safe_name = sanitize_for_filename(&area_name);
    let filename = format!("Arnis {safe_name}.mcworld");
    let lvl_name = format!("Arnis World: {safe_name}");
    (output_dir.join(&filename), lvl_name)
}

/// Creates a new Java Edition world in the given base directory.
///
/// Uses the sanitized custom name if given (with a counter suffix on
/// collision), otherwise generates a unique "Arnis World N" name. Creates the
/// directory structure (with a `region/` subdirectory), writes the region
/// template, level.dat (with updated name, timestamp, and spawn position),
/// and icon.png.
///
/// Returns the full path to the newly created world directory.
pub fn create_new_world(base_path: &Path, custom_name: Option<&str>) -> Result<String, String> {
    let unique_name: String = if let Some(name) = normalize_custom_name(custom_name) {
        unique_world_name(base_path, &sanitize_for_filename(name))
    } else {
        // Generate a unique world name with proper counter
        // Check for both "Arnis World X" and "Arnis World X: Location" patterns
        let mut counter: i32 = 1;
        loop {
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
        }
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
                    if pos.len() < 3 {
                        return Err(
                            "Invalid level.dat template: Player Pos list has fewer than 3 elements"
                                .to_string(),
                        );
                    }
                    if let Value::Double(ref mut x) = pos[0] {
                        *x = -5.0;
                    }
                    if let Value::Double(ref mut y) = pos[1] {
                        *y = -61.0;
                    }
                    if let Value::Double(ref mut z) = pos[2] {
                        *z = -5.0;
                    }
                }

                if let Some(Value::List(ref mut rot)) = player.get_mut("Rotation") {
                    if rot.is_empty() {
                        return Err(
                            "Invalid level.dat template: Player Rotation list is empty".to_string()
                        );
                    }
                    if let Value::Float(ref mut x) = rot[0] {
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

/// Name of the bundled Java datapack that extends the Overworld build height.
pub const TALL_DATAPACK_NAME: &str = "arnis_tall";

/// Install the bundled tall-world datapack into a Java world and register it
/// in `level.dat`'s `Data.DataPacks.Enabled` so it auto-activates on first
/// load. The base `data/` tree uses the legacy flat dimension_type schema
/// (formats 61-88, i.e. 1.21.4-1.21.10); overlays carry the attributes schema
/// for 1.21.11-era (formats 90-100) and 26.1.x (format 101.x), since the
/// schema is mutually incompatible across those eras.
pub fn install_tall_datapack(world_path: &Path) -> Result<(), String> {
    const PACK_MCMETA: &[u8] = include_bytes!("../assets/minecraft/datapack_tall/pack.mcmeta");
    const OVERWORLD_JSON: &[u8] = include_bytes!(
        "../assets/minecraft/datapack_tall/data/minecraft/dimension_type/overworld.json"
    );
    const OVERLAY_ATTRIBUTES_JSON: &[u8] = include_bytes!(
        "../assets/minecraft/datapack_tall/overlay_attributes/data/minecraft/dimension_type/overworld.json"
    );
    const OVERLAY_2601_JSON: &[u8] = include_bytes!(
        "../assets/minecraft/datapack_tall/overlay_2601/data/minecraft/dimension_type/overworld.json"
    );

    let dp_root = world_path.join("datapacks").join(TALL_DATAPACK_NAME);

    // (overlay directory, embedded bytes); empty directory = base data/ tree.
    let dim_files: [(&str, &[u8]); 3] = [
        ("", OVERWORLD_JSON),
        ("overlay_attributes", OVERLAY_ATTRIBUTES_JSON),
        ("overlay_2601", OVERLAY_2601_JSON),
    ];
    for (overlay, bytes) in dim_files {
        let mut dim_dir = dp_root.clone();
        if !overlay.is_empty() {
            dim_dir.push(overlay);
        }
        let dim_dir = dim_dir
            .join("data")
            .join("minecraft")
            .join("dimension_type");
        fs::create_dir_all(&dim_dir)
            .map_err(|e| format!("Failed to create datapack directories: {e}"))?;
        fs::write(dim_dir.join("overworld.json"), bytes)
            .map_err(|e| format!("Failed to write overworld.json: {e}"))?;
    }

    fs::write(dp_root.join("pack.mcmeta"), PACK_MCMETA)
        .map_err(|e| format!("Failed to write pack.mcmeta: {e}"))?;

    register_tall_datapack_in_level_dat(world_path)?;

    Ok(())
}

/// Appends the pack entry if missing. Expected to run on a fresh level.dat
/// template whose Enabled list starts with `["vanilla"]`, so the appended
/// entry naturally lands after vanilla and our dimension_type override wins.
fn register_tall_datapack_in_level_dat(world_path: &Path) -> Result<(), String> {
    let level_path = world_path.join("level.dat");
    if !level_path.exists() {
        return Err(format!("level.dat not found at {level_path:?}"));
    }

    let raw = fs::read(&level_path).map_err(|e| format!("Failed to read level.dat: {e}"))?;
    let mut decoder = GzDecoder::new(raw.as_slice());
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| format!("Failed to decompress level.dat: {e}"))?;

    let mut root: Value = fastnbt::from_bytes(&decompressed)
        .map_err(|e| format!("Failed to parse level.dat NBT: {e}"))?;

    let entry = format!("file/{TALL_DATAPACK_NAME}");

    {
        let data = match root {
            Value::Compound(ref mut r) => match r.get_mut("Data") {
                Some(Value::Compound(ref mut d)) => d,
                _ => return Err("level.dat missing Data compound".to_string()),
            },
            _ => return Err("level.dat root is not a compound".to_string()),
        };

        let data_packs = data
            .entry("DataPacks".to_string())
            .or_insert_with(|| Value::Compound(Default::default()));
        let Value::Compound(ref mut dp) = data_packs else {
            return Err("level.dat Data.DataPacks is not a compound".to_string());
        };

        let enabled = dp
            .entry("Enabled".to_string())
            .or_insert_with(|| Value::List(Vec::new()));
        let Value::List(ref mut list) = enabled else {
            return Err("level.dat Data.DataPacks.Enabled is not a list".to_string());
        };

        let already_enabled = list
            .iter()
            .any(|v| matches!(v, Value::String(s) if s == &entry));
        if !already_enabled {
            list.push(Value::String(entry));
        }
    }

    let serialized =
        fastnbt::to_bytes(&root).map_err(|e| format!("Failed to serialize level.dat: {e}"))?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized)
        .map_err(|e| format!("Failed to compress level.dat: {e}"))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize level.dat compression: {e}"))?;
    fs::write(&level_path, compressed).map_err(|e| format!("Failed to write level.dat: {e}"))?;

    Ok(())
}

// Writes GameType, DayTime and the player's game mode into an existing level.dat.
pub fn apply_java_world_settings(
    world_path: &Path,
    game_mode: crate::args::GameMode,
    world_time: i64,
) -> Result<(), String> {
    let level_path = world_path.join("level.dat");
    if !level_path.exists() {
        return Err(format!("level.dat not found at {level_path:?}"));
    }

    let raw = fs::read(&level_path).map_err(|e| format!("Failed to read level.dat: {e}"))?;
    let mut decoder = GzDecoder::new(raw.as_slice());
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| format!("Failed to decompress level.dat: {e}"))?;

    let mut root: Value = fastnbt::from_bytes(&decompressed)
        .map_err(|e| format!("Failed to parse level.dat NBT: {e}"))?;

    {
        let data = match root {
            Value::Compound(ref mut r) => match r.get_mut("Data") {
                Some(Value::Compound(ref mut d)) => d,
                _ => return Err("level.dat missing Data compound".to_string()),
            },
            _ => return Err("level.dat root is not a compound".to_string()),
        };

        let game_type = game_mode.java_game_type();
        data.insert("GameType".to_string(), Value::Int(game_type));
        data.insert("DayTime".to_string(), Value::Long(world_time));
        if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
            player.insert("playerGameType".to_string(), Value::Int(game_type));
        }
    }

    let serialized =
        fastnbt::to_bytes(&root).map_err(|e| format!("Failed to serialize level.dat: {e}"))?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized)
        .map_err(|e| format!("Failed to compress level.dat: {e}"))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize level.dat compression: {e}"))?;
    fs::write(&level_path, compressed).map_err(|e| format!("Failed to write level.dat: {e}"))?;

    Ok(())
}

/// Sets the player spawn point in an existing Java Edition level.dat file.
///
/// Updates both the world spawn point (SpawnX/SpawnY/SpawnZ) and the player
/// position if a Player compound exists. Callers derive `spawn_y` from the
/// generated terrain so the player spawns above ground even in extended-height
/// worlds where terrain may reach Y≈2000.
pub fn set_spawn_in_level_dat(
    world_path: &Path,
    spawn_x: i32,
    spawn_y: i32,
    spawn_z: i32,
) -> Result<(), String> {
    let level_path = world_path.join("level.dat");
    if !level_path.exists() {
        return Err(format!("level.dat not found at {level_path:?}"));
    }

    // Read and decompress
    let level_data = fs::read(&level_path).map_err(|e| format!("Failed to read level.dat: {e}"))?;

    let mut decoder = GzDecoder::new(level_data.as_slice());
    let mut decompressed_data = Vec::new();
    decoder
        .read_to_end(&mut decompressed_data)
        .map_err(|e| format!("Failed to decompress level.dat: {e}"))?;

    let mut nbt_data: Value = fastnbt::from_bytes(&decompressed_data)
        .map_err(|e| format!("Failed to parse level.dat NBT data: {e}"))?;

    // Update spawn point
    let data = match nbt_data {
        Value::Compound(ref mut root) => match root.get_mut("Data") {
            Some(Value::Compound(ref mut data)) => data,
            _ => {
                return Err(
                    "Invalid level.dat structure: missing or non-compound \"Data\" section"
                        .to_string(),
                );
            }
        },
        _ => {
            return Err(
                "Invalid level.dat structure: root NBT value is not a compound".to_string(),
            );
        }
    };

    data.insert("SpawnX".to_string(), Value::Int(spawn_x));
    data.insert("SpawnY".to_string(), Value::Int(spawn_y));
    data.insert("SpawnZ".to_string(), Value::Int(spawn_z));

    // Update player position if Player compound exists
    if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
        if let Some(Value::List(ref mut pos)) = player.get_mut("Pos") {
            if pos.len() >= 3 {
                if let Some(Value::Double(ref mut pos_x)) = pos.get_mut(0) {
                    *pos_x = spawn_x as f64;
                }
                if let Some(Value::Double(ref mut pos_y)) = pos.get_mut(1) {
                    *pos_y = spawn_y as f64;
                }
                if let Some(Value::Double(ref mut pos_z)) = pos.get_mut(2) {
                    *pos_z = spawn_z as f64;
                }
            }
        }
    }

    // Serialize, compress, and write back
    let serialized_data = fastnbt::to_bytes(&nbt_data)
        .map_err(|e| format!("Failed to serialize updated level.dat: {e}"))?;

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized_data)
        .map_err(|e| format!("Failed to compress updated level.dat: {e}"))?;
    let compressed_data = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize compression for level.dat: {e}"))?;

    fs::write(&level_path, compressed_data)
        .map_err(|e| format!("Failed to write updated level.dat: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_new_world_uses_custom_name() {
        let tmp = tempfile::tempdir().unwrap();
        let world = PathBuf::from(create_new_world(tmp.path(), Some("My City")).unwrap());
        assert_eq!(world.file_name().unwrap(), "My City");

        // A second world with the same name gets a counter suffix
        let world2 = PathBuf::from(create_new_world(tmp.path(), Some("My City")).unwrap());
        assert_eq!(world2.file_name().unwrap(), "My City 2");

        // Blank names fall back to the default naming scheme
        let world3 = PathBuf::from(create_new_world(tmp.path(), Some("   ")).unwrap());
        assert_eq!(world3.file_name().unwrap(), "Arnis World 1");
    }

    #[test]
    fn custom_name_is_sanitized_for_filesystem() {
        let tmp = tempfile::tempdir().unwrap();
        let world = PathBuf::from(create_new_world(tmp.path(), Some("Berlin: Mitte")).unwrap());
        assert_eq!(world.file_name().unwrap(), "Berlin_ Mitte");
    }

    #[test]
    fn luanti_world_name_prefers_custom_name() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(luanti_world_name(tmp.path(), Some("My City")), "My City");
        assert_eq!(luanti_world_name(tmp.path(), None), "Arnis Luanti World 1");
        assert_eq!(
            luanti_world_name(tmp.path(), Some("  ")),
            "Arnis Luanti World 1"
        );
    }

    #[test]
    fn apply_java_world_settings_writes_gametype_and_daytime() {
        let tmp = tempfile::tempdir().unwrap();
        let world = PathBuf::from(create_new_world(tmp.path(), None).unwrap());
        apply_java_world_settings(&world, crate::args::GameMode::Survival, 13000).unwrap();

        let raw = fs::read(world.join("level.dat")).unwrap();
        let mut decompressed = Vec::new();
        GzDecoder::new(raw.as_slice())
            .read_to_end(&mut decompressed)
            .unwrap();
        let root: Value = fastnbt::from_bytes(&decompressed).unwrap();
        let Value::Compound(root) = root else {
            panic!("root not a compound");
        };
        let Some(Value::Compound(data)) = root.get("Data") else {
            panic!("missing Data");
        };
        assert_eq!(data.get("GameType"), Some(&Value::Int(0)));
        assert_eq!(data.get("DayTime"), Some(&Value::Long(13000)));
        if let Some(Value::Compound(player)) = data.get("Player") {
            assert_eq!(player.get("playerGameType"), Some(&Value::Int(0)));
        }
    }
}
