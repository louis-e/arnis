//! The on-disk cache for the facade pipeline: imagery, clusters, finished walls.
//!
//! Everything lives under `dirs::cache_dir()/arnis-tile-cache/mapillary/`, which
//! is the same root the elevation and land cover tiles use. That is deliberate
//! and is the whole of the cache clearing story: `clear_all_cached_tiles` wipes
//! the root recursively and the daily sweep in `elevation::cache` ages files out
//! at 30 days, so both already reach this tree without a second entry point.
//! `the_mapillary_cache_is_inside_the_arnis_tile_cache` pins that, because it
//! is a property of the path and nothing would otherwise notice it breaking.
//!
//! Four trees, and the split is about what a key is worth:
//!
//! * `meta/<shard>/<id>.json` one Graph API record per image. Keyed by
//!   Mapillary id rather than by search cell, because a cell key is a function
//!   of the bbox the user happened to draw and so is useless to the next
//!   generation, while an id is the same everywhere. The record is also where
//!   the signed URLs and the credit fields live, so a run served entirely from
//!   cache can still download and still attribute what it used.
//! * `img/<shard>/<id>_<size>.jpg` the thumbnail bytes.
//! * `sfm/<shard>/<cluster>.json.zz` the OpenSfM cluster, stored exactly as the
//!   CDN sends it, zlib compressed. The Python lab inflates before writing;
//!   here it does not, because the Munich test box alone is 85 MB of clusters
//!   inflated against 31 MB on the wire and this tree is in the user's cache
//!   directory. The `.zz` in the name says the bytes are not JSON yet.
//! * `facades/e<epoch>-<params digest>/<shard>/<wall digest>.{json,png,_tex.png}`
//!   the finished per wall product. The directory is a digest of every tunable,
//!   so changing one threshold cannot serve a stale facade; the file name is a
//!   digest of the wall's OSM node ids, so the same wall is found again
//!   whatever bbox the next generation uses. A wall the pipeline proved has no
//!   facade is stored too, as the JSON alone: that verdict is as expensive to
//!   reach as a texture and there is nowhere else it can be kept per wall.
//!
//! Sharding is the last two characters of the key. Mapillary ids are decimal
//! and our digests are hex, so both spread evenly, and a city with a hundred
//! thousand images gets a thousand files per directory instead of all of them
//! in one.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use crate::elevation::cache::{clear_cache_dir, CacheClearStats};

use super::types::{Params, Tier, WallEdge, WallProduct};

/// Provider name under the shared tile cache root.
const PROVIDER: &str = "mapillary";

/// Bump this and every cached wall is rebuilt.
///
/// The facade directory is named after [`Params::digest`], which covers the 85
/// tunables and nothing else. The pipeline also carries roughly two hundred
/// module constants that no `Params` value reaches (`BLUR_MIN`,
/// `HIRES_MAX_DIST_M`, the fusion and opening thresholds, the band tolerance),
/// and the code itself, so a fix to any of them would otherwise be served the
/// old answer out of the cache for thirty days and look like it had not worked.
/// Hashing the source is not on the table for a shipped binary; a number a
/// change to the pipeline has to remember to raise is, and it costs one line.
pub const EPOCH: u32 = 1;

/// Which thumbnail was downloaded. The pipeline runs on 2048 everywhere; the
/// original is only for a future detail mode and 1024 only for a fallback,
/// so the size is part of the file name and the three can coexist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ImageSize {
    W1024,
    #[default]
    W2048,
    Original,
}

impl ImageSize {
    /// The file name suffix, which is also the Graph API field stem.
    pub fn suffix(self) -> &'static str {
        match self {
            ImageSize::W1024 => "1024",
            ImageSize::W2048 => "2048",
            ImageSize::Original => "orig",
        }
    }

    /// The Graph API field carrying the signed URL for this size.
    pub fn url_field(self) -> &'static str {
        match self {
            ImageSize::W1024 => "thumb_1024_url",
            ImageSize::W2048 => "thumb_2048_url",
            ImageSize::Original => "thumb_original_url",
        }
    }
}

/// Root of the facade cache, next to the elevation and land cover tiles.
pub fn root() -> PathBuf {
    crate::elevation::cache::get_cache_dir(PROVIDER)
}

/// Where the four trees live under one root.
///
/// The root is a value rather than a constant because a test must never write
/// into the user's real cache directory, and because a caller that wants a
/// throwaway cache for one run should be able to say so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub root: PathBuf,
}

impl Default for Layout {
    fn default() -> Self {
        Self::new(root())
    }
}

impl Layout {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn meta_dir(&self) -> PathBuf {
        self.root.join("meta")
    }

    pub fn image_dir(&self) -> PathBuf {
        self.root.join("img")
    }

    pub fn cluster_dir(&self) -> PathBuf {
        self.root.join("sfm")
    }

    /// The facade tree for one set of tunables, under one [`EPOCH`].
    pub fn facade_dir(&self, params: &Params) -> PathBuf {
        self.root
            .join("facades")
            .join(format!("e{EPOCH}-{}", params.digest()))
    }

    /// `meta/<shard>/<id>.json`.
    pub fn meta_path(&self, id: &str) -> Result<PathBuf, String> {
        let id = safe_key(id)?;
        Ok(self.meta_dir().join(shard(id)).join(format!("{id}.json")))
    }

    /// `img/<shard>/<id>_<size>.jpg`.
    pub fn image_path(&self, id: &str, size: ImageSize) -> Result<PathBuf, String> {
        let id = safe_key(id)?;
        Ok(self
            .image_dir()
            .join(shard(id))
            .join(format!("{id}_{}.jpg", size.suffix())))
    }

    /// `sfm/<shard>/<cluster>.json.zz`, holding the zlib bytes the CDN served.
    pub fn cluster_path(&self, cluster_id: &str) -> Result<PathBuf, String> {
        let id = safe_key(cluster_id)?;
        Ok(self
            .cluster_dir()
            .join(shard(id))
            .join(format!("{id}.json.zz")))
    }

    /// Total bytes under this root.
    pub fn size_bytes(&self) -> u64 {
        dir_size_bytes(&self.root)
    }
}

/// Where a facade tree's per-run export directories live.
///
/// Spelled here rather than only in `pipeline` because the 3D preview reads
/// these without ever building a `PipelineConfig`, and two spellings of one
/// path is how a preview ends up looking at a directory nothing writes.
pub fn exports_root(facade_dir: &Path) -> PathBuf {
    facade_dir.join("exports")
}

/// Rejects anything that is not a plain identifier before it becomes a path.
///
/// Mapillary ids are decimal and cluster ids are too, but they arrive from the
/// network and are pasted straight into a file name, so a `..` or a separator
/// would escape the cache directory. Nothing else in the pipeline is in a
/// position to catch that.
fn safe_key(key: &str) -> Result<&str, String> {
    if key.is_empty() || key.len() > 128 {
        return Err(format!("unusable cache key {key:?}"));
    }
    if key
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Ok(key)
    } else {
        Err(format!("unusable cache key {key:?}"))
    }
}

/// The two character subdirectory a key lives in.
fn shard(key: &str) -> String {
    let tail: Vec<char> = key.chars().rev().take(2).collect();
    match tail.len() {
        2 => format!("{}{}", tail[1], tail[0]).to_ascii_lowercase(),
        1 => format!("0{}", tail[0]).to_ascii_lowercase(),
        _ => "00".to_string(),
    }
}

/// The cache key of a wall: a digest of its OSM node ids in ring order and of
/// which piece of that span the wall is.
///
/// Node ids and not the wall key, because a wall key carries the edge index
/// within its building's ring and that index shifts as soon as a mapper inserts
/// a node somewhere earlier in the ring. The node ids are what `facades.rs`
/// matches walls to the world with, so they are the identity that actually has
/// to survive between two generations.
///
/// The piece is in the key because the node ids alone are not unique: a span
/// longer than `split_wall_m` is cut into pieces that all carry the whole
/// span's edges, so without it every piece hashes alike and the second one is
/// served the first one's facade.
pub fn wall_cache_key(node_ids: &[i64], piece: usize) -> String {
    use std::hash::Hasher;
    let mut h = fnv::FnvHasher::default();
    for id in node_ids {
        h.write(&id.to_le_bytes());
    }
    h.write(&(piece as u64).to_le_bytes());
    format!("{:016x}", h.finish())
}

/// Every node id of a wall product, in order: the first edge's start and then
/// each edge's end, falling back to the wall's own two nodes.
pub fn wall_node_ids(product: &WallProduct) -> Vec<i64> {
    if product.edges.is_empty() {
        return vec![product.node_a, product.node_b];
    }
    let mut ids = vec![product.edges[0].node_a];
    for edge in &product.edges {
        ids.push(edge.node_b);
    }
    ids
}

/// `(json, blocks png, texture png)` for one wall in one facade tree.
pub fn wall_paths(dir: &Path, key: &str) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let key = safe_key(key)?;
    let sub = dir.join(shard(key));
    Ok((
        sub.join(format!("{key}.json")),
        sub.join(format!("{key}.png")),
        sub.join(format!("{key}_tex.png")),
    ))
}

// --------------------------------------------------------------------------- bytes

/// Writes through a temporary file in the same directory.
///
/// A generation that is killed mid-write would otherwise leave a truncated JPEG
/// or a half a JSON object behind, and the next run would read it as a cache
/// hit. Renaming a complete file into place means an entry either exists whole
/// or does not exist.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    let tmp = path.with_extension(format!(
        "tmp{}{}",
        std::process::id(),
        // Two threads of one process can write the same entry at once.
        TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    // A full disk leaves a partial temporary behind, and nothing else will ever
    // look at it, so it has to go here or the next attempt only makes another.
    if let Err(e) = std::fs::write(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("write {}: {e}", tmp.display()));
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(format!("rename into {}: {e}", path.display()))
        }
    }
}

static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The bytes of a cache entry, or `None` when it is not there.
pub fn read_cached(path: &Path) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) if !bytes.is_empty() => Some(bytes),
        _ => None,
    }
}

/// Whether a cache entry is there, answered without reading it.
///
/// The same question [`read_cached`] answers, for a caller that wants nothing
/// but the answer. The download batches ask it of every file they are about to
/// skip, so on an area whose imagery is already here the check used to read the
/// whole download off the disk and throw it away: the Munich box's 470
/// thumbnails and 218 originals are 246 MB and 401 MB (see the `pipeline`
/// header), and the texture stage then read the same files again to decode them.
pub fn is_cached(path: &Path) -> bool {
    // `is_file` because `read_cached` fails on a directory, and a directory has
    // a non-zero length on some filesystems.
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0)
}

// --------------------------------------------------------------------------- the wall product

/// A wall product as it is stored: the scalars in JSON next to the two PNGs.
///
/// The block grid is the same RGB plus class in alpha layout the Arnis export
/// uses and `facades::parse_wall` already reads, so the cache and the export are
/// the same picture. `observed` is the one thing the export does not carry, and
/// it is a character per cell here rather than a JSON array of booleans because
/// a 60 by 40 wall is 2.4 KB either way in the first form and 12 KB in the
/// second.
///
/// A record with `cols == 0` is a wall the pipeline proved carries no facade.
/// It has no PNGs, and `reach_m` says how much of the wall's surroundings the
/// run that wrote it had actually searched, which is the only thing that makes
/// such a verdict a fact about the wall rather than about the box somebody
/// drew around it.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct WallRecord {
    wall_key: String,
    building_key: String,
    node_a: i64,
    node_b: i64,
    #[serde(default)]
    piece: usize,
    #[serde(default = "one")]
    n_pieces: usize,
    edges: Vec<EdgeRecord>,
    col0_m: f64,
    cols: u32,
    rows: u32,
    observed: String,
    bands: Vec<[u8; 3]>,
    tier: String,
    confidence: f64,
    height_used_m: f64,
    unknown_share: f64,
    views: Vec<String>,
    flags: Vec<String>,
    has_tex: bool,
    /// Only meaningful on a `cols == 0` record. See the struct comment.
    #[serde(default)]
    reach_m: f64,
}

/// Serde's default for a wall that was cached before the piece was part of the
/// record: an unsplit span is one piece.
fn one() -> usize {
    1
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct EdgeRecord {
    edge_idx: usize,
    node_a: i64,
    node_b: i64,
    s0: f64,
    s1: f64,
}

/// Writes one finished wall into a facade tree.
///
/// `reach_m` is only read back for a wall with no facade; see [`WallRecord`].
///
/// A blank verdict keeps the **furthest** reach any run has proved it against,
/// and never the one this run happens to carry. A run judges the walls just
/// outside its box too (`geometry::WALL_MARGIN_M`) and reaches short of the cap
/// round those, so without this a precompute of one piece overwrote the record
/// its neighbour had just paid the whole align stage for, and how much the
/// cache remembered came down to the order the boxes were drawn in. Reach is
/// monotone information, "this wall was proved blank against everything within
/// R metres", so the larger R is the one that was actually proved; the thirty
/// day sweep, and not a smaller number written over it later, is what eventually
/// makes the pipeline look again.
pub fn store_wall(dir: &Path, product: &WallProduct, reach_m: f64) -> Result<String, String> {
    let key = wall_cache_key(&wall_node_ids(product), product.piece);
    let (json_path, png_path, tex_path) = wall_paths(dir, &key)?;
    let reach_m = if product.cols == 0 {
        reach_m.max(stored_reach_m(&json_path))
    } else {
        reach_m
    };

    let cells = (product.cols as usize) * (product.rows as usize);
    if product.rgb.len() != cells || product.cls.len() != cells {
        return Err(format!(
            "wall {}: {}x{} grid but {} colours and {} classes",
            product.wall_key,
            product.cols,
            product.rows,
            product.rgb.len(),
            product.cls.len()
        ));
    }

    // A wall with no facade is the JSON alone: `image` cannot encode a zero
    // sized PNG, and there is nothing in one to encode.
    if cells == 0 {
        let _ = std::fs::remove_file(&png_path);
        let _ = std::fs::remove_file(&tex_path);
    } else {
        let mut png = image::RgbaImage::new(product.cols, product.rows);
        for (i, pixel) in png.pixels_mut().enumerate() {
            let [r, g, b] = product.rgb[i];
            *pixel = image::Rgba([r, g, b, product.cls[i]]);
        }
        write_atomic(&png_path, &encode_png(&png)?)?;

        match &product.tex {
            Some(tex) => write_atomic(&tex_path, &encode_png(tex)?)?,
            // A wall that lost its texture must not keep the previous run's.
            None => {
                let _ = std::fs::remove_file(&tex_path);
            }
        }
    }

    let record = WallRecord {
        wall_key: product.wall_key.clone(),
        building_key: product.building_key.clone(),
        node_a: product.node_a,
        node_b: product.node_b,
        piece: product.piece,
        n_pieces: product.n_pieces,
        edges: product
            .edges
            .iter()
            .map(|e| EdgeRecord {
                edge_idx: e.edge_idx,
                node_a: e.node_a,
                node_b: e.node_b,
                s0: e.s0,
                s1: e.s1,
            })
            .collect(),
        col0_m: product.col0_m,
        cols: product.cols,
        rows: product.rows,
        observed: product
            .observed
            .iter()
            .map(|&seen| if seen { '1' } else { '0' })
            .collect(),
        bands: product.bands.clone(),
        tier: product.tier.as_str().to_string(),
        confidence: product.confidence,
        height_used_m: product.height_used_m,
        unknown_share: product.unknown_share,
        views: product.views.clone(),
        flags: product.flags.clone(),
        has_tex: cells > 0 && product.tex.is_some(),
        reach_m,
    };
    let json = serde_json::to_vec(&record).map_err(|e| format!("wall {key}: {e}"))?;
    write_atomic(&json_path, &json)?;
    Ok(key)
}

/// The reach already on record for a wall, or 0 when there is none.
///
/// Only the one number, because it is read on the write path of every blank
/// verdict and the grid beside it is not wanted there.
fn stored_reach_m(json_path: &Path) -> f64 {
    read_cached(json_path)
        .and_then(|bytes| serde_json::from_slice::<WallRecord>(&bytes).ok())
        .filter(|record| record.cols == 0)
        .map(|record| record.reach_m)
        .unwrap_or(0.0)
}

/// One wall as the cache holds it.
#[derive(Debug)]
pub struct CachedWall {
    pub product: WallProduct,
    /// Only meaningful when `product.cols == 0`. See [`WallRecord`].
    pub reach_m: f64,
}

/// Reads one wall back, or `None` when it is not cached.
///
/// A cache entry that does not read back cleanly is reported as a miss rather
/// than as an error: the caller can always rebuild the wall, and a single
/// damaged file must not stop a generation.
pub fn load_wall(dir: &Path, key: &str) -> Option<CachedWall> {
    let (json_path, png_path, tex_path) = wall_paths(dir, key).ok()?;
    let record: WallRecord = serde_json::from_slice(&read_cached(&json_path)?).ok()?;

    let cells = (record.cols as usize) * (record.rows as usize);
    // Counted in characters, not bytes: a damaged file whose `observed` holds a
    // multibyte character would pass a byte length check and then hand back a
    // mask shorter than the grid, which the export indexes cell by cell.
    let observed: Vec<bool> = record.observed.chars().map(|c| c == '1').collect();
    if observed.len() != cells {
        return None;
    }
    // Sized only once the record has proven it holds that many cells: the file
    // is user-writable, and `cols` and `rows` alone could ask for gigabytes.
    let mut rgb = Vec::with_capacity(cells);
    let mut cls = Vec::with_capacity(cells);
    // A wall with no facade carries no PNG, so there is nothing to open and
    // nothing to check against the record.
    if cells > 0 {
        let png = image::load_from_memory(&read_cached(&png_path)?)
            .ok()?
            .to_rgba8();
        if png.width() != record.cols || png.height() != record.rows {
            return None;
        }
        for pixel in png.pixels() {
            rgb.push([pixel[0], pixel[1], pixel[2]]);
            cls.push(pixel[3]);
        }
    }

    let tex = if record.has_tex {
        // The JSON says there is a texture, so a missing or unreadable one is a
        // half written entry, not a wall without a texture.
        Some(
            image::load_from_memory(&read_cached(&tex_path)?)
                .ok()?
                .to_rgba8(),
        )
    } else {
        None
    };

    let product = WallProduct {
        wall_key: record.wall_key,
        building_key: record.building_key,
        node_a: record.node_a,
        node_b: record.node_b,
        piece: record.piece,
        n_pieces: record.n_pieces,
        edges: record
            .edges
            .iter()
            .map(|e| WallEdge {
                edge_idx: e.edge_idx,
                node_a: e.node_a,
                node_b: e.node_b,
                s0: e.s0,
                s1: e.s1,
            })
            .collect(),
        col0_m: record.col0_m,
        cols: record.cols,
        rows: record.rows,
        rgb,
        cls,
        observed,
        bands: record.bands,
        tex,
        tier: Tier::from_str_or_default(&record.tier),
        confidence: record.confidence,
        height_used_m: record.height_used_m,
        unknown_share: record.unknown_share,
        views: record.views,
        flags: record.flags,
    };
    Some(CachedWall {
        product,
        reach_m: record.reach_m,
    })
}

fn encode_png<P, C>(img: &image::ImageBuffer<P, C>) -> Result<Vec<u8>, String>
where
    P: image::Pixel<Subpixel = u8> + image::PixelWithColorType,
    C: std::ops::Deref<Target = [u8]>,
{
    let mut bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )
    .map_err(|e| format!("encode png: {e}"))?;
    Ok(bytes)
}

// --------------------------------------------------------------------------- size and clearing

/// Total bytes under the facade cache, for the clear cache setting to show.
pub fn size_bytes() -> u64 {
    dir_size_bytes(&root())
}

/// Bytes of every regular file under `dir`, symlinks not followed.
///
/// Same rule as `elevation::cache::clear_recursive`: a symlink inside the cache
/// points at something we neither own nor are about to delete, so counting it
/// would report a size that clearing cannot free.
pub fn dir_size_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            total = total.saturating_add(dir_size_bytes(&entry.path()));
        } else if file_type.is_file() {
            total = total.saturating_add(entry.metadata().map(|m| m.len()).unwrap_or(0));
        }
    }
    total
}

/// A byte count in the unit that fits, for the cache setting line.
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Empties the facade cache on its own.
///
/// The GUI does not need this: its clear button calls `clear_all_cached_tiles`,
/// which wipes the whole tile cache root and so takes this tree with it. It is
/// here for a caller that wants to drop only the imagery, and for the test that
/// pins the two are the same tree.
pub fn clear() -> CacheClearStats {
    clear_cache_dir(&root())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn product(cols: u32, rows: u32) -> WallProduct {
        let cells = (cols * rows) as usize;
        WallProduct {
            wall_key: "w81190157_2".into(),
            building_key: "w81190157".into(),
            node_a: 947_182_331,
            node_b: 947_182_332,
            piece: 0,
            n_pieces: 1,
            edges: vec![
                WallEdge {
                    edge_idx: 2,
                    node_a: 947_182_331,
                    node_b: 947_182_332,
                    s0: 0.0,
                    s1: 6.5,
                },
                WallEdge {
                    edge_idx: 3,
                    node_a: 947_182_332,
                    node_b: 947_182_333,
                    s0: 6.5,
                    s1: 12.0,
                },
            ],
            col0_m: 0.25,
            cols,
            rows,
            rgb: (0..cells)
                .map(|i| [(i % 256) as u8, 17, (255 - i % 256) as u8])
                .collect(),
            cls: (0..cells)
                .map(|i| if i % 3 == 0 { 255 } else { 192 })
                .collect(),
            observed: (0..cells).map(|i| i % 4 != 0).collect(),
            bands: (0..rows).map(|r| [(r * 7) as u8, 8, 9]).collect(),
            tex: Some(image::RgbaImage::from_pixel(
                cols * 8,
                rows * 8,
                image::Rgba([1, 2, 3, 255]),
            )),
            tier: Tier::A,
            confidence: 0.734,
            height_used_m: 12.5,
            unknown_share: 0.125,
            views: vec!["1234".into(), "5678".into()],
            flags: vec!["roof_from_sky".into()],
        }
    }

    #[test]
    fn the_mapillary_cache_is_inside_the_arnis_tile_cache() {
        // This is the whole of the cache clearing story: the GUI button calls
        // clear_all_cached_tiles on the root below, and the daily sweep walks
        // the same root, so both reach the facade cache without a second wiring.
        let base = crate::elevation::cache::get_base_cache_dir();
        let layout = Layout::default();
        assert!(
            layout.root.starts_with(&base),
            "{:?} is not under {base:?}",
            layout.root
        );
        assert_eq!(layout.root, root());
        assert!(layout.meta_dir().starts_with(&layout.root));
        assert!(layout.image_dir().starts_with(&layout.root));
        assert!(layout.cluster_dir().starts_with(&layout.root));
        assert!(layout
            .facade_dir(&Params::default())
            .starts_with(&layout.root));
    }

    /// A wall the pipeline proved carries no facade: no grid, no texture.
    fn blank(tier: Tier) -> WallProduct {
        WallProduct {
            cols: 0,
            rows: 0,
            rgb: Vec::new(),
            cls: Vec::new(),
            observed: Vec::new(),
            bands: Vec::new(),
            tex: None,
            tier,
            confidence: 0.0,
            height_used_m: 0.0,
            views: Vec::new(),
            flags: vec!["NO_LINE_OF_SIGHT".into()],
            ..product(1, 1)
        }
    }

    #[test]
    fn a_wall_product_round_trips_through_the_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let before = product(9, 6);
        let key = store_wall(tmp.path(), &before, 45.0).unwrap();
        let after = load_wall(tmp.path(), &key)
            .expect("the wall just written must read back")
            .product;

        assert_eq!(after.wall_key, before.wall_key);
        assert_eq!(after.building_key, before.building_key);
        assert_eq!(after.node_a, before.node_a);
        assert_eq!(after.node_b, before.node_b);
        assert_eq!(after.edges, before.edges);
        assert_eq!(after.cols, before.cols);
        assert_eq!(after.rows, before.rows);
        assert_eq!(after.rgb, before.rgb);
        assert_eq!(after.cls, before.cls);
        assert_eq!(after.observed, before.observed);
        assert_eq!(after.bands, before.bands);
        assert_eq!(after.tier, before.tier);
        assert_eq!(after.views, before.views);
        assert_eq!(after.flags, before.flags);
        assert!((after.confidence - before.confidence).abs() < 1e-12);
        assert!((after.col0_m - before.col0_m).abs() < 1e-12);
        assert!((after.height_used_m - before.height_used_m).abs() < 1e-12);
        assert!((after.unknown_share - before.unknown_share).abs() < 1e-12);
        let tex = after.tex.expect("the texture must come back");
        assert_eq!(tex.dimensions(), (72, 48));
        assert_eq!(*tex.get_pixel(0, 0), image::Rgba([1, 2, 3, 255]));
    }

    /// The blank verdict is as expensive to reach as a texture, so it lives in
    /// the same per wall entry and comes back the same way. Without it a later
    /// generation has no per wall answer for the walls that carry no facade,
    /// which on the Munich box is 912 of 1030.
    #[test]
    fn a_wall_with_no_facade_is_cached_as_the_record_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let before = blank(Tier::D);
        let key = store_wall(tmp.path(), &before, 45.0).unwrap();
        let (json_path, png_path, tex_path) = wall_paths(tmp.path(), &key).unwrap();
        assert!(json_path.exists(), "the verdict is the JSON");
        assert!(!png_path.exists(), "there is no grid to write");
        assert!(!tex_path.exists());

        let after = load_wall(tmp.path(), &key).expect("the verdict must read back");
        assert!((after.reach_m - 45.0).abs() < 1e-12);
        assert_eq!(after.product.cols, 0);
        assert_eq!(after.product.rows, 0);
        assert_eq!(after.product.tier, Tier::D);
        assert_eq!(after.product.flags, vec!["NO_LINE_OF_SIGHT".to_string()]);

        // And a verdict from a box that only half looked at the wall says how
        // far it did look, which is what stops it being reused by a box that
        // reaches further round that wall. Its own tree, because a second write
        // of the same wall now keeps the furthest reach on record; that is the
        // next test.
        let half = tempfile::tempdir().unwrap();
        let key = store_wall(half.path(), &blank(Tier::C), 12.5).unwrap();
        let after = load_wall(half.path(), &key).unwrap();
        assert!((after.reach_m - 12.5).abs() < 1e-12);
        assert_eq!(after.product.tier, Tier::C);
    }

    /// A blank verdict keeps the furthest reach anything has proved it against.
    ///
    /// A run judges the walls just outside its box too, and reaches short of
    /// the cap round those, so a precompute of one piece used to write a 0 over
    /// the 45 the piece beside it had just paid the whole align stage for. What
    /// survived in the cache then depended on the order the boxes were drawn
    /// in, and a generation over the pieces together found no answer for the
    /// walls along every seam and did the area's align work again.
    #[test]
    fn a_neighbouring_run_cannot_shorten_a_reach_already_proved() {
        let tmp = tempfile::tempdir().unwrap();
        // The piece that contains the wall proves it against everything.
        let key = store_wall(tmp.path(), &blank(Tier::D), 45.0).unwrap();
        // The piece next door, for which the same wall is only an occluder.
        assert_eq!(store_wall(tmp.path(), &blank(Tier::D), 0.0).unwrap(), key);
        let after = load_wall(tmp.path(), &key).expect("the verdict is still there");
        assert!(
            (after.reach_m - 45.0).abs() < 1e-12,
            "the occluder's view of the wall overwrote the proof, reach={}",
            after.reach_m
        );

        // A wall that comes back with a facade is not a verdict at all, and its
        // reach is never read, so nothing is carried over into one.
        let key = store_wall(tmp.path(), &product(5, 4), 0.0).unwrap();
        assert!((load_wall(tmp.path(), &key).unwrap().reach_m - 0.0).abs() < 1e-12);
    }

    /// A wall that had a facade and then lost it must not keep serving the old
    /// pixels out of files the blank record no longer mentions.
    #[test]
    fn a_wall_that_loses_its_facade_loses_its_images() {
        let tmp = tempfile::tempdir().unwrap();
        let key = store_wall(tmp.path(), &product(5, 4), 45.0).unwrap();
        let (_, png_path, tex_path) = wall_paths(tmp.path(), &key).unwrap();
        assert!(png_path.exists() && tex_path.exists());

        assert_eq!(store_wall(tmp.path(), &blank(Tier::D), 45.0).unwrap(), key);
        assert!(!png_path.exists());
        assert!(!tex_path.exists());
        assert_eq!(load_wall(tmp.path(), &key).unwrap().product.cols, 0);
    }

    #[test]
    fn a_wall_is_found_again_by_its_node_ids_not_its_wall_key() {
        let tmp = tempfile::tempdir().unwrap();
        let mut before = product(4, 3);
        let key = store_wall(tmp.path(), &before, 45.0).unwrap();
        // The mapper renumbered the ring: same nodes, later edge index.
        before.wall_key = "w81190157_7".into();
        before.edges[0].edge_idx = 7;
        before.edges[1].edge_idx = 8;
        assert_eq!(wall_cache_key(&wall_node_ids(&before), before.piece), key);
    }

    #[test]
    fn a_missing_or_damaged_entry_reads_as_a_miss() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_wall(tmp.path(), "0123456789abcdef").is_none());

        let key = store_wall(tmp.path(), &product(5, 4), 45.0).unwrap();
        let (_, png_path, _) = wall_paths(tmp.path(), &key).unwrap();
        std::fs::write(&png_path, b"not a png").unwrap();
        assert!(load_wall(tmp.path(), &key).is_none());
    }

    #[test]
    fn the_facade_directory_changes_when_a_tunable_changes() {
        let layout = Layout::new(PathBuf::from("/cache"));
        let base = Params::default();
        let tweaked = Params {
            tex_ppb: base.tex_ppb + 1,
            ..Params::default()
        };
        assert_ne!(base.digest(), tweaked.digest());
        assert_ne!(layout.facade_dir(&base), layout.facade_dir(&tweaked));
        // And nothing else moved: the same tunables give the same directory.
        assert_eq!(
            layout.facade_dir(&base),
            layout.facade_dir(&Params::default())
        );

        let softer = Params {
            max_incidence_deg: 55.0,
            ..Params::default()
        };
        assert_ne!(layout.facade_dir(&base), layout.facade_dir(&softer));

        // The epoch is in the name, because the digest covers the tunables and
        // not the two hundred module constants beside them: raising it is how a
        // pipeline change stops being served the old answer.
        let name = layout.facade_dir(&base);
        let name = name.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            name.starts_with(&format!("e{EPOCH}-")),
            "the facade directory must carry the epoch: {name}"
        );
    }

    #[test]
    fn cache_paths_are_sharded_and_reject_a_key_that_would_escape() {
        let layout = Layout::new(PathBuf::from("/cache"));
        assert!(layout
            .meta_path("1071917607838019")
            .unwrap()
            .ends_with("19/1071917607838019.json"));
        assert!(layout
            .image_path("1071917607838019", ImageSize::W2048)
            .unwrap()
            .ends_with("19/1071917607838019_2048.jpg"));
        assert!(layout
            .cluster_path("768264339100060")
            .unwrap()
            .ends_with("60/768264339100060.json.zz"));

        for bad in ["../../etc/passwd", "a/b", "", "with space", ".."] {
            assert!(
                layout.meta_path(bad).is_err(),
                "{bad:?} must not become a path"
            );
            assert!(layout.image_path(bad, ImageSize::W2048).is_err());
            assert!(layout.cluster_path(bad).is_err());
        }
    }

    #[test]
    fn size_walks_the_tree_and_reports_the_unit_that_fits() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("img").join("19");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("a.jpg"), vec![0u8; 3000]).unwrap();
        std::fs::write(tmp.path().join("b.json"), vec![0u8; 96]).unwrap();
        assert_eq!(dir_size_bytes(tmp.path()), 3096);
        assert_eq!(dir_size_bytes(&tmp.path().join("nope")), 0);

        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(900), "900 B");
        assert_eq!(format_size(2048), "2 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }

    #[test]
    fn an_interrupted_write_leaves_no_entry_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("deep").join("nested").join("x.json");
        write_atomic(&path, b"hello").unwrap();
        assert_eq!(read_cached(&path).unwrap(), b"hello");
        // Nothing but the finished file: the temporary is renamed, not left.
        let siblings: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(siblings.len(), 1);
        // An empty file is a miss, not an empty hit.
        std::fs::write(&path, b"").unwrap();
        assert!(read_cached(&path).is_none());
    }
}
