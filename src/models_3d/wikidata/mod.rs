//! Wikidata `wikidata=Q*` → P4896 → Commons .stl → voxelized, placed at the OSM anchor.

mod client;
mod index;
mod placement;
mod stl;

pub use index::PERMISSIVE_ATTRIBUTIONS;
pub use placement::{place_wikidata_models, prescan};
