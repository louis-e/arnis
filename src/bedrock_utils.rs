use crate::coordinate_system::geographic::LLBBox;
use crate::retrieve_data;
use std::path::PathBuf;

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
