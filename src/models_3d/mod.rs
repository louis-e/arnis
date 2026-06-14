//! 3D model substitution pipelines (3DMR glTF, Wikidata P4896 STL, Arnis-hosted archetypes).

pub(crate) mod custom;
pub(crate) mod palette;
pub(crate) mod pipeline;
pub(crate) mod three_dmr;
pub(crate) mod voxelize;
pub(crate) mod wikidata;

pub use pipeline::Models3dPipeline;

use crate::elevation::cache::{clear_cache_dir, CacheClearStats};
use crate::world_editor::WorldEditor;

/// Minimum ground Y across an XZ bbox: stride-sampled for ~16×16 samples plus explicit corners.
/// Shared by every model placer so they all snap to identical terrain.
pub(crate) fn lowest_ground_in_bbox(
    editor: &WorldEditor,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
) -> i32 {
    let stride = ((max_x - min_x).max(max_z - min_z) / 16).clamp(1, 8);
    let mut lowest = i32::MAX;
    let mut x = min_x;
    while x <= max_x {
        let mut z = min_z;
        while z <= max_z {
            lowest = lowest.min(editor.get_ground_level(x, z));
            z += stride;
        }
        x += stride;
    }
    for (x, z) in [
        (min_x, min_z),
        (max_x, min_z),
        (min_x, max_z),
        (max_x, max_z),
    ] {
        lowest = lowest.min(editor.get_ground_level(x, z));
    }
    if lowest == i32::MAX {
        editor.get_ground_level((min_x + max_x) / 2, (min_z + max_z) / 2)
    } else {
        lowest
    }
}

/// Region keys (x>>9, z>>9) overlapping [cx-r, cx+r]×[cz-r, cz+r] plus a 1-region
/// safety ring; used to defer the regions a 3D placement may write to (stream-to-disk).
pub(crate) fn region_keys_around(cx: i32, cz: i32, r: i32) -> Vec<(i32, i32)> {
    let (rx0, rx1) = (((cx - r) >> 9) - 1, ((cx + r) >> 9) + 1);
    let (rz0, rz1) = (((cz - r) >> 9) - 1, ((cz + r) >> 9) + 1);
    let mut out = Vec::new();
    for rx in rx0..=rx1 {
        for rz in rz0..=rz1 {
            out.push((rx, rz));
        }
    }
    out
}

/// Clears on-disk caches for every 3D-model fetcher.
pub fn clear_model_caches() -> CacheClearStats {
    clear_cache_dir(&three_dmr::client::cache_root())
        .combined(clear_cache_dir(&wikidata::client::cache_root()))
        .combined(clear_cache_dir(&custom::client::cache_root()))
}
