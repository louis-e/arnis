//! 3D model substitution pipelines (3DMR glTF + Wikidata P4896 STL) sharing voxelizer + palette.

pub(crate) mod palette;
pub(crate) mod three_dmr;
pub(crate) mod voxelize;
pub(crate) mod wikidata;

use crate::elevation::cache::{clear_cache_dir, CacheClearStats};

/// Clears on-disk caches for the 3D-model fetchers (3DMR + Wikidata).
pub fn clear_model_caches() -> CacheClearStats {
    clear_cache_dir(&three_dmr::client::cache_root())
        .combined(clear_cache_dir(&wikidata::client::cache_root()))
}
