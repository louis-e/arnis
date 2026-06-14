#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use once_cell::sync::OnceCell;
use serde_json::json;
use tauri::{Emitter, WebviewWindow};

pub static MAIN_WINDOW: OnceCell<WebviewWindow> = OnceCell::new();

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
/// Percentages are monotonic in the ACTUAL execution order (download -> overture ->
/// land cover -> elevation -> parse -> transform -> generate -> ground -> save):
///
/// Downloading map data...    1-5%
/// Adding extra buildings...  6%        (Overture; fetched right after download)
/// Detecting surface types... 9%        (land cover; skipped if disabled)
/// Fetching elevation...      10%
/// Processing elevation...    12-18%
/// (parsing, silent)          18.5%
/// Transforming map...        19%
/// Generating area...         20-70%
/// Generating ground...       70-90%
/// Saving world...            90-100%
///
/// The function `emit_gui_progress_update` is used to send real-time progress updates to the UI.
pub fn emit_gui_progress_update(progress: f64, message: &str) {
    if let Some(window) = get_main_window() {
        let payload = json!({
            "progress": progress,
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
            "progress": progress,
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
