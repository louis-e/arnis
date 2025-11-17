#![allow(clippy::module_inception)]

pub mod args;
pub mod block_definitions;
pub mod bresenham;
pub mod colors;
pub mod coordinate_system;
pub mod cpu_info;
pub mod data_processing;
pub mod element_processing;
pub mod elevation_data;
pub mod floodfill;
pub mod ground;
#[cfg(feature = "gui")]
pub mod gui;
pub mod map_transformation;
#[cfg(feature = "metrics")]
pub mod metrics;
pub mod osm_parser;
pub mod perf_config;
#[cfg(feature = "gui")]
pub mod progress;
pub mod retrieve_data;
#[cfg(test)]
pub mod test_utilities;
pub mod version_check;
pub mod world_editor;

#[cfg(not(feature = "gui"))]
pub mod progress {
    pub fn emit_gui_error(_message: &str) {}
    pub fn emit_gui_progress_update(_progress: f64, _message: &str) {}
    pub fn is_running_with_gui() -> bool {
        false
    }
}

pub use args::Args;
#[cfg(feature = "metrics")]
pub use metrics::{MetricsRecorder, MetricsSnapshot};
pub use perf_config::PerformanceConfig;
