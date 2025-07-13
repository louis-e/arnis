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
/// The progress updates are mapped to specific steps in the pipeline:
///
/// [1/8] Fetching data... - Starts at: 0% / Completes at: 5%
/// [2/8] Parsing data... - Starts at: 5% / Completes at: 15%
/// [3/8] Fetching elevation... - Starts at: 15% / Completes at: 20%
/// [5/8] Transforming map... - Starts at: 20% / Completes at: 25%
/// [6/8] Processing data... - Starts at: 25% / Completes at: 70%
/// [7/8] Generating ground layer... - Starts at: 70% / Completes at: 90%
/// [8/8] Saving world... - Starts at: 90% / Completes at: 100%
///
/// The function `emit_gui_progress_update` is used to send real-time progress updates to the UI.
pub fn emit_gui_progress_update(progress: f64, message: &str) {
    if let Some(window) = get_main_window() {
        let payload = json!({
            "progress": progress,
            "message": message
        });

        if let Err(e) = window.emit("progress-update", payload) {
            eprintln!("Failed to emit progress event: {e}");
        }
    }
}

pub fn emit_gui_error(message: &str) {
    emit_gui_progress_update(0.0, &format!("Error! {message}"));
}
