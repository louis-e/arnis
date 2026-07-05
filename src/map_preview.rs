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

// (epoch, result); the epoch discards stale results from a previous run's finalize thread.
static PREVIEW_STATE: Mutex<(u64, Option<PreviewResult>)> = Mutex::new((0, None));

/// Invalidates any previous or in-flight preview and returns the new epoch.
pub fn begin_preview_epoch() -> u64 {
    let mut state = PREVIEW_STATE.lock().unwrap();
    state.0 += 1;
    state.1 = None;
    state.0
}

pub fn epoch_is_current(epoch: u64) -> bool {
    PREVIEW_STATE.lock().unwrap().0 == epoch
}

/// Stores the result unless a newer generation started meanwhile.
pub fn record_preview_result(epoch: u64, result: PreviewResult) -> bool {
    let mut state = PREVIEW_STATE.lock().unwrap();
    if state.0 != epoch {
        return false;
    }
    state.1 = Some(result);
    true
}

#[cfg(feature = "gui")]
pub fn last_preview_result() -> Option<PreviewResult> {
    PREVIEW_STATE.lock().unwrap().1.clone()
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
