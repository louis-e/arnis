//! Images rendered into locked filled maps, shown by invisible item frames.
//! Placement lives in `element_processing::signage`.

pub mod draw;
pub mod font;
pub mod pictograms;
pub mod posters;
pub mod region;
pub mod registry;
pub mod render;
pub mod templates;

pub use registry::{DecalKey, DecalRegistry, ShieldStyle, TextStyle, TrafficSign};
