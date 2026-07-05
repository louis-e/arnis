#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use once_cell::sync::OnceCell;
use serde_json::json;
use std::sync::atomic::{AtomicU32, Ordering};
use tauri::{Emitter, WebviewWindow};

pub static MAIN_WINDOW: OnceCell<WebviewWindow> = OnceCell::new();

// Highest progress emitted so far (percent * 100), keeps the bar monotonic.
static PROGRESS_FLOOR: AtomicU32 = AtomicU32::new(0);

/// Resets the monotonic progress floor at the start of a generation.
pub fn reset_progress_floor() {
    PROGRESS_FLOOR.store(0, Ordering::Relaxed);
}

// Error emits (0.0) pass through untouched.
fn clamp_progress(progress: f64) -> f64 {
    if progress <= 0.0 {
        return progress;
    }
    let p = (progress * 100.0) as u32;
    let prev = PROGRESS_FLOOR.fetch_max(p, Ordering::Relaxed);
    f64::from(prev.max(p)) / 100.0
}

pub fn set_main_window(window: WebviewWindow) {
    MAIN_WINDOW.set(window).ok();
}

pub fn get_main_window() -> Option<&'static WebviewWindow> {
    MAIN_WINDOW.get()
}

/// This function checks if the program is running with a GUI window.
/// Returns `true` if a GUI window is initialized, `false` otherwise.
pub fn is_running_with_gui() -> bool {
    get_main_window().is_some()
}

/// This code manages a multi-step process with a progress bar indicating the overall completion.
/// OSM download, Overture and elevation/land cover run in parallel within 1-18%; the shown
/// percentage is kept monotonic by `clamp_progress` since those stages emit out of order:
///
/// Downloading data...        1-10%    (OSM 1-5, Overture 6, land cover 9, elevation fetch 10)
/// Processing elevation...    12-18%   (runs parallel to the OSM download)
/// (parsing, silent)          18.5%
/// Transforming map...        19%
/// Processing data...         19.5%    (flood fills, footprints, road masks, 3D prescan)
/// Generating area...         20-70%
/// Generating ground...       70-90%
/// Saving world...            90-100%
///
/// The function `emit_gui_progress_update` is used to send real-time progress updates to the UI.
pub fn emit_gui_progress_update(progress: f64, message: &str) {
    if let Some(window) = get_main_window() {
        let payload = json!({
            "progress": clamp_progress(progress),
            "message": message
        });

        if let Err(e) = window.emit("progress-update", payload) {
            let error_msg = format!("Failed to emit progress event: {}", e);
            eprintln!("{}", error_msg);
            #[cfg(feature = "gui")]
            send_log(LogLevel::Warning, &error_msg);
        }
    }
}

/// Like `emit_gui_progress_update` but also carries the stream-to-disk regime so
/// the GUI ETA can pick the right time-weight profile (the post-70% tail is
/// ~instant when streaming, a real save otherwise). Additive payload field;
/// only the two terrain emits use it, all other sites stay on the plain fn.
pub fn emit_gui_progress_update_ex(progress: f64, message: &str, streaming: bool) {
    if let Some(window) = get_main_window() {
        let payload = json!({
            "progress": clamp_progress(progress),
            "message": message,
            "streaming": streaming
        });
        if let Err(e) = window.emit("progress-update", payload) {
            let error_msg = format!("Failed to emit progress event: {}", e);
            eprintln!("{}", error_msg);
            #[cfg(feature = "gui")]
            send_log(LogLevel::Warning, &error_msg);
        }
    }
}

pub fn emit_gui_error(message: &str) {
    // Truncate by characters (not bytes) to avoid panicking when the GUI
    // status bar receives an error containing multi-byte UTF-8. e.g.
    // localized OS error messages like "Недостаточно системных ресурсов…"
    // where byte 35 lands inside a Cyrillic character.
    const MAX_CHARS: usize = 35;
    let truncated: String = message.chars().take(MAX_CHARS).collect();
    emit_gui_progress_update(0.0, &format!("Error! {truncated}"));
}

/// Emits the final in-game level name (including localized area suffix for Java,
/// or the location-based name for Bedrock) so the GUI can display it.
pub fn emit_world_name_update(name: &str) {
    if let Some(window) = get_main_window() {
        if let Err(e) = window.emit("world-name-update", name) {
            eprintln!("Failed to emit world-name-update event: {e}");
        }
    }
}

/// Emits an event when the world map preview is ready
pub fn emit_map_preview_ready() {
    if let Some(window) = get_main_window() {
        if let Err(e) = window.emit("map-preview-ready", ()) {
            eprintln!("Failed to emit map-preview-ready event: {}", e);
        }
    }
}

/// Emits an event to reveal a file or folder in the system file explorer
pub fn emit_show_in_folder(path: &str) {
    if let Some(window) = get_main_window() {
        if let Err(e) = window.emit("show-in-folder", path) {
            eprintln!("Failed to emit show-in-folder event: {}", e);
        }
    }
}
