//! Map preview wiring: output location and GUI handoff of the last result.

use crate::world_editor::WorldFormat;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Result of the last preview render, consumed by the GUI overlay transport.
#[derive(Clone)]
pub struct PreviewResult {
    pub png_path: PathBuf,
    pub min_lat: f64,
    pub max_lat: f64,
    pub min_lon: f64,
    pub max_lon: f64,
    pub min_mc_x: i32,
    pub max_mc_x: i32,
    pub min_mc_z: i32,
    pub max_mc_z: i32,
}

static LAST_PREVIEW: Mutex<Option<PreviewResult>> = Mutex::new(None);

pub fn record_preview_result(result: PreviewResult) {
    *LAST_PREVIEW.lock().unwrap() = Some(result);
}

pub fn clear_preview_result() {
    *LAST_PREVIEW.lock().unwrap() = None;
}

#[cfg(feature = "gui")]
pub fn last_preview_result() -> Option<PreviewResult> {
    LAST_PREVIEW.lock().unwrap().clone()
}

/// Java: inside the world dir; Bedrock: "<name> map.png" next to the .mcworld.
pub fn preview_output_path(world_output: &Path, format: WorldFormat) -> PathBuf {
    match format {
        WorldFormat::BedrockMcWorld => {
            let stem = world_output
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("arnis_world");
            world_output.with_file_name(format!("{stem} map.png"))
        }
        _ => world_output.join("arnis_world_map.png"),
    }
}
