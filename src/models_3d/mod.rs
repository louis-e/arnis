//! 3D model substitution pipelines (3DMR glTF, Wikidata P4896 STL).

pub(crate) mod pipeline;
pub(crate) mod three_dmr;
pub(crate) mod voxelize;
pub(crate) mod wikidata;

pub use pipeline::Models3dPipeline;

use crate::block_definitions::Block;
use crate::elevation::cache::{clear_cache_dir, CacheClearStats};
use crate::osm_parser::ProcessedNode;
use crate::world_editor::WorldEditor;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// Tag the parser leaves in a One World on an element a model may stand in for, and on a
/// building the area's edge cuts: its anchor and bounds as mapped, "x z min_x min_z max_x
/// max_z". Every area or piece then puts the model in one place at one size and hides the
/// same buildings under it.
const FOOTPRINT_TAG: &str = "arnis:model_footprint";

/// Whether a 3D model may stand in for an element with these tags.
pub(crate) fn may_replace(tags: &HashMap<String, String>) -> bool {
    tags.contains_key("3dmr")
        || tags
            .get("wikidata")
            .is_some_and(|q| wikidata::has_model(q.trim()))
}

/// Whether an element with these tags is hidden when its anchor lies in a model's footprint.
pub(crate) fn may_be_hidden(tags: &HashMap<String, String>) -> bool {
    tags.contains_key("building") || tags.contains_key("building:part")
}

/// `FOOTPRINT_TAG` for an element made of `nodes` as mapped: their mean and bounds.
pub(crate) fn footprint_tag<'a>(
    nodes: impl Iterator<Item = &'a ProcessedNode>,
) -> Option<(String, String)> {
    let (mut sx, mut sz, mut n) = (0i64, 0i64, 0i64);
    let (mut x0, mut z0, mut x1, mut z1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for p in nodes {
        sx += i64::from(p.x);
        sz += i64::from(p.z);
        n += 1;
        (x0, z0, x1, z1) = (x0.min(p.x), z0.min(p.z), x1.max(p.x), z1.max(p.z));
    }
    (n > 0).then(|| {
        let value = format!("{} {} {x0} {z0} {x1} {z1}", sx / n, sz / n);
        (FOOTPRINT_TAG.to_string(), value)
    })
}

/// The anchor and bounds `[min_x, min_z, max_x, max_z]` the parser left in `FOOTPRINT_TAG`.
pub(crate) fn whole_footprint(tags: &HashMap<String, String>) -> Option<((i32, i32), [i32; 4])> {
    let v: Vec<i32> = tags
        .get(FOOTPRINT_TAG)?
        .split(' ')
        .map(|s| s.parse().ok())
        .collect::<Option<_>>()?;
    let [x, z, x0, z0, x1, z1] = v[..] else {
        return None;
    };
    Some(((x, z), [x0, z0, x1, z1]))
}

/// Why a model download gave nothing.
#[derive(Debug)]
pub(crate) enum FetchError {
    /// The server has none (404) or it is over the size cap: no cache miss, nothing to retry.
    Absent(String),
    Failed(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absent(e) | Self::Failed(e) => f.write_str(e),
        }
    }
}

impl From<String> for FetchError {
    fn from(e: String) -> Self {
        Self::Failed(e)
    }
}

/// `fetch` for a model not in the cache, unless `marker` says the server has none. An absent
/// answer leaves the marker, so neither a later run nor `--offline` asks again.
pub(crate) fn unless_absent<T>(
    marker: &Path,
    what: &str,
    fetch: impl FnOnce() -> Result<T, FetchError>,
) -> Result<T, FetchError> {
    if crate::elevation::cache::missing_marker_holds(marker) {
        return Err(FetchError::Absent(format!(
            "{what}: unavailable when last asked"
        )));
    }
    let got = fetch();
    if let Err(FetchError::Absent(_)) = got {
        crate::elevation::cache::write_atomic(marker, b"");
    }
    got
}

/// `resp` if it succeeded; a 404 is absent.
pub(crate) fn ok_status(
    resp: reqwest::blocking::Response,
    what: &str,
) -> Result<reqwest::blocking::Response, FetchError> {
    let status = resp.status();
    let e = format!("{what}: HTTP {status}");
    match status {
        s if s.is_success() => Ok(resp),
        reqwest::StatusCode::NOT_FOUND => Err(FetchError::Absent(e)),
        _ => Err(FetchError::Failed(e)),
    }
}

/// The body; one over `cap` bytes is absent.
pub(crate) fn read_capped(
    mut resp: reqwest::blocking::Response,
    cap: u64,
) -> Result<Vec<u8>, FetchError> {
    if let Some(len) = resp.content_length() {
        if len > cap {
            return Err(FetchError::Absent(format!(
                "model exceeds {cap}-byte cap (advertised {len})"
            )));
        }
    }
    let mut buf = Vec::new();
    let mut taken = (&mut resp).take(cap + 1);
    taken.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if buf.len() as u64 > cap {
        return Err(FetchError::Absent(format!("model exceeds {cap}-byte cap")));
    }
    Ok(buf)
}

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

/// Tallest tree the footprint sweep looks for above the ground.
const TREE_SWEEP_HEIGHT: i32 = 64;

/// How far past the footprint the sweep follows a tree; a crown reaches no further.
pub(crate) const TREE_SWEEP_REACH: i32 = 10;

/// Writes model voxels over whatever stands there. Trees inside the footprint go
/// first, whole, so none grows through a lattice or a hollow statue.
pub(crate) fn stamp_voxels(editor: &mut WorldEditor, voxels: &[([i32; 3], Block)]) {
    clear_trees_in_footprint(editor, voxels);
    for &([x, y, z], block) in voxels {
        editor.set_block_absolute(block, x, y, z, None, Some(&[]));
    }
}

fn clear_trees_in_footprint(editor: &mut WorldEditor, voxels: &[([i32; 3], Block)]) {
    let Some((min_x, max_x, min_z, max_z)) = voxels.iter().fold(None, |acc, ([x, _, z], _)| {
        let (a, b, c, d) = acc.unwrap_or((*x, *x, *z, *z));
        Some((a.min(*x), b.max(*x), c.min(*z), d.max(*z)))
    }) else {
        return;
    };
    let top = voxels.iter().map(|(p, _)| p[1]).max().unwrap_or(0);
    let (span_x, span_z) = ((max_x - min_x + 1) as usize, (max_z - min_z + 1) as usize);
    let mut mask = vec![false; span_x * span_z];
    for ([x, _, z], _) in voxels {
        mask[(x - min_x) as usize + (z - min_z) as usize * span_x] = true;
    }
    clear_trees_in_mask(editor, min_x, min_z, span_x, span_z, &mask, top);
}

/// Takes out every tree growing in `mask` (row-major from `min_x`, `min_z`) below `top`,
/// and what it encloses, whole.
pub(crate) fn clear_trees_in_mask(
    editor: &mut WorldEditor,
    min_x: i32,
    min_z: i32,
    span_x: usize,
    span_z: usize,
    mask: &[bool],
    top: i32,
) {
    let (max_x, max_z) = (min_x + span_x as i32 - 1, min_z + span_z as i32 - 1);
    // Under a ring of legs or a platform edge counts as inside.
    let mask = crate::landmarks::fill_enclosed(mask, span_x, span_z);
    // Crowns touch across a wood, so an unbounded flood would clear the trees around it too.
    let (reach_x, reach_z) = (
        min_x - TREE_SWEEP_REACH..=max_x + TREE_SWEEP_REACH,
        min_z - TREE_SWEEP_REACH..=max_z + TREE_SWEEP_REACH,
    );
    let near = |x: i32, z: i32| reach_x.contains(&x) && reach_z.contains(&z);
    for (i, _) in mask.iter().enumerate().filter(|(_, &m)| m) {
        let (x, z) = (min_x + (i % span_x) as i32, min_z + (i / span_x) as i32);
        let ground = editor.get_ground_level(x, z);
        for y in ground..=(ground + TREE_SWEEP_HEIGHT).min(top) {
            if editor
                .get_block_absolute(x, y, z)
                .is_some_and(crate::water_depth::is_tree_part)
            {
                crate::water_depth::clear_tree_within(editor, x, y, z, near);
            }
        }
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

/// Where the retired stadium model was cached; still listed so the panel can clear it.
fn retired_archetype_cache_root() -> std::path::PathBuf {
    crate::elevation::cache::user_cache_dir()
        .map(|d| d.join("arnis/custom_models"))
        .unwrap_or_else(|| std::path::PathBuf::from("./.arnis_custom_cache"))
}

/// Every root the 3D model fetchers cache into, for the settings panel's size
/// readout.
pub fn model_cache_roots() -> Vec<std::path::PathBuf> {
    vec![
        three_dmr::client::cache_root(),
        wikidata::client::cache_root(),
        retired_archetype_cache_root(),
    ]
}

/// Clears on-disk caches for every 3D-model fetcher.
pub fn clear_model_caches() -> CacheClearStats {
    clear_cache_dir(&three_dmr::client::cache_root())
        .combined(clear_cache_dir(&wikidata::client::cache_root()))
        .combined(clear_cache_dir(&retired_archetype_cache_root()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_the_server_lacks_is_not_asked_for_again() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("m.missing");
        let gone = unless_absent::<()>(&marker, "m", || Err(FetchError::Absent("404".into())));
        assert!(matches!(gone, Err(FetchError::Absent(_))));
        assert!(marker.exists());
        let again = unless_absent::<()>(&marker, "m", || panic!("asked again"));
        assert!(matches!(again, Err(FetchError::Absent(_))));
        // A download that failed may pass next time, so it leaves no marker.
        let other = tmp.path().join("o.missing");
        let failed = unless_absent::<()>(&other, "o", || Err(FetchError::Failed("reset".into())));
        assert!(matches!(failed, Err(FetchError::Failed(_))));
        assert!(!other.exists());
    }

    #[test]
    fn a_footprint_is_the_mean_and_bounds_of_the_nodes() {
        let node = |x, z| ProcessedNode {
            id: 1,
            tags: HashMap::new(),
            x,
            z,
        };
        let nodes = [node(-7, 3), node(20, 3), node(20, 40), node(-7, 3)];
        let tags: HashMap<_, _> = footprint_tag(nodes.iter()).into_iter().collect();
        // Truncated towards zero, as the placers took the mean of the nodes they were given.
        assert_eq!(whole_footprint(&tags), Some(((6, 12), [-7, 3, 20, 40])));
        assert!(footprint_tag(std::iter::empty()).is_none());
        assert!(may_replace(&HashMap::from([(
            "wikidata".to_string(),
            " Q10288".to_string()
        )])));
        assert!(!may_replace(&HashMap::from([(
            "wikidata".to_string(),
            "Q1".to_string()
        )])));
    }
}
