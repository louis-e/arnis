//! Wikidata `wikidata=Q*` → P4896 → Commons .stl → voxelized, placed at the OSM anchor.

pub(crate) mod client;
mod index;
mod placement;
mod stl;

pub use index::PERMISSIVE_ATTRIBUTIONS;
pub use placement::{place_wikidata_models, prescan, PrescanResult};
