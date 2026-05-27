//! 3D model substitution pipelines (3DMR glTF, Wikidata P4896 STL, Arnis-hosted archetypes).

pub(crate) mod custom;
pub(crate) mod palette;
pub(crate) mod pipeline;
pub(crate) mod three_dmr;
pub(crate) mod voxelize;
pub(crate) mod wikidata;

pub use pipeline::Models3dPipeline;

use crate::elevation::cache::{clear_cache_dir, CacheClearStats};

/// Clears on-disk caches for every 3D-model fetcher.
pub fn clear_model_caches() -> CacheClearStats {
    clear_cache_dir(&three_dmr::client::cache_root())
        .combined(clear_cache_dir(&wikidata::client::cache_root()))
        .combined(clear_cache_dir(&custom::client::cache_root()))
}
