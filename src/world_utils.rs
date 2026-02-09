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
        sanitized.truncate(MAX_LEN);
        sanitized = sanitized.trim_end().to_string();
    }

    if sanitized.is_empty() {
        "Unknown Location".to_string()
    } else {
        sanitized
    }
}

/// Builds the Bedrock output path and level name for a given bounding box.
/// Combines area name lookup, sanitization, and path construction.
pub fn build_bedrock_output(bbox: &LLBBox, output_dir: PathBuf) -> (PathBuf, String) {
    let area_name = get_area_name_for_bedrock(bbox);
    let safe_name = sanitize_for_filename(&area_name);
    let filename = format!("Arnis {safe_name}.mcworld");
    let lvl_name = format!("Arnis World: {safe_name}");
    (output_dir.join(&filename), lvl_name)
}

/// Creates a new Java Edition world in the given base directory.
///
/// Generates a unique "Arnis World N" name, creates the directory structure
/// (with a `region/` subdirectory), writes the region template, level.dat
/// (with updated name, timestamp, and spawn position), and icon.png.
///
/// Returns the full path to the newly created world directory.
pub fn create_new_world(base_path: &Path) -> Result<String, String> {
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
