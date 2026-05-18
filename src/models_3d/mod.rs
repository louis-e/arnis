//! 3D model substitution pipelines (3DMR glTF + Wikidata P4896 STL) sharing voxelizer + palette.

pub(crate) mod palette;
pub(crate) mod three_dmr;
pub(crate) mod voxelize;
pub(crate) mod wikidata;

use crate::elevation::cache::{clear_cache_dir, CacheClearStats};
use std::path::PathBuf;

/// Clears on-disk caches for the 3D-model fetchers (3DMR + Wikidata).
pub fn clear_model_caches() -> CacheClearStats {
    let base = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("."));
    clear_cache_dir(&base.join("arnis/3dmr"))
        .combined(clear_cache_dir(&base.join("arnis/wikidata_models")))
}
