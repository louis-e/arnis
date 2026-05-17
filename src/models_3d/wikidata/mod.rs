//! Wikidata-driven 3D model fetcher. Wikidata items tagged on OSM elements
//! (`wikidata=Q*`) that have a P4896 (3D model) statement resolve to a .stl on
//! Commons, which is voxelized and placed at the OSM element's footprint.

mod client;
mod index;
mod placement;
mod stl;

pub use index::PERMISSIVE_ATTRIBUTIONS;
pub use placement::{place_wikidata_models, prescan};
