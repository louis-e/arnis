//! Mapterhorn terrain tiles — the global primary elevation provider.
//!
//! Mapterhorn (<https://mapterhorn.com>) publishes terrarium-encoded
//! elevation tiles as 512×512 **lossless** WebP images on a standard
//! Web-Mercator XYZ grid, built from open national DEMs:
//!
//! - Global base layer from Copernicus GLO-30 (30 m, TanDEM-X) up to z12.
//!   Substantially more accurate than the SRTM/GMTED mix behind AWS
//!   Terrain Tiles, especially above 60°N where SRTM has no data.
//! - National LiDAR DTMs (0.25–10 m) at z13–z18 for ~60 regions:
//!   most of Europe country-wide at 1 m, Japan, USA (10 m), and more.
//!   Bare-earth terrain models, so buildings and tree canopy don't
//!   inflate the ground level the way SRTM's surface model does.
//!
//! Tile availability rules this provider must handle:
//!
//! - Tiles above z12 exist only where a high-resolution source was
//!   ingested; elsewhere the server returns 404.
//! - Tiles that are **pure ocean return 404 at every zoom level** —
//!   Mapterhorn ships no bathymetry. Sea pixels inside coastal tiles
//!   are encoded as exactly 0 m.
//!
//! Both cases are served by the same mechanism: a per-tile pyramid
//! fallback. Every requested tile that 404s is replaced by its parent
//! tile (down to [`MIN_ZOOM`]); grid cells whose pyramid walk finds no
//! tile at all are genuinely mid-ocean and filled with 0.0 (sea level).
//! 404 responses are negative-cached on disk so repeated generations of
//! coastal areas don't re-probe the same absent tiles (the elevation
//! cache's 30-day age sweep bounds the staleness of those markers).
//!
//! Attribution: © Mapterhorn (<https://mapterhorn.com/attribution>),
//! aggregating open data sources (Copernicus, national mapping
//! agencies) under CC-BY-4.0-family licenses.

use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::cache::get_cache_dir;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};
use fnv::{FnvHashMap, FnvHashSet};
use rayon::prelude::*;
use std::path::{Path, PathBuf};

use super::fixed_tile::{bbox_dimensions_m, blend_finite_samples};

/// Mapterhorn XYZ endpoint (no API key required; Cloudflare-served).
const MAPTERHORN_URL: &str = "https://tiles.mapterhorn.com/{z}/{x}/{y}.webp";
/// Pixels per tile edge (Mapterhorn serves 512 px tiles — twice the
/// linear resolution of AWS Terrain Tiles' 256 px, so one zoom level
/// lower fetches the same ground resolution).
const TILE_PX: u32 = 512;
/// Terrarium format offset for height decoding.
const TERRARIUM_OFFSET: f64 = 32768.0;
/// Finest zoom ever requested. z17 ≈ 0.4 m/px at mid latitudes — only
/// reachable when the user upscales (`--scale` > 1) over a sub-meter
/// source region (Switzerland, Denmark, ...). Regions without data at
/// the chosen zoom cost one cheap round of 404s before the pyramid
/// falls back a level.
const MAX_ZOOM: u8 = 17;
/// Pyramid-walk floor. Any tile containing land exists at every zoom
/// level up to 12, so by z6 (≈ 5.6°/tile) only genuinely mid-ocean
/// areas are still missing — walking further would waste requests.
const MIN_ZOOM: u8 = 6;
/// Maximum concurrent tile downloads (same courtesy cap as AWS).
const MAX_CONCURRENT_DOWNLOADS: usize = 8;
/// Hard budget on tiles per fetch; the zoom is lowered until the
/// covering-tile count fits. Worst case (a ~16 km bbox over a 1 m
/// source region): ~400 MB downloaded, and ~1.6 GB of DECODED 512×512
/// RGB tiles (786 KB each) held transiently during sampling —
/// proportionate to the up-to-2 GB f64 height grid they feed, and
/// released before post-processing.
const MAX_TILES_PER_FETCH: usize = 2048;
/// Tiles per download chunk; the outage circuit breaker checks between
/// chunks so a dead network is detected mid-run, not after burning the
/// retry budget on every tile.
const DOWNLOAD_CHUNK_SIZE: usize = 64;
/// Abort the fetch (falling back to AWS) after this many consecutive
/// failures with no interleaved success or 404.
const OUTAGE_FAILURE_THRESHOLD: usize = 128;
/// Levels larger than this get a spread-probe before full fan-out, so
/// a zoom with no local data costs ~16 requests instead of hundreds
/// of 404s.
const PROBE_MIN_LEVEL_TILES: usize = 64;
/// Number of spread tiles probed per level.
const PROBE_TILE_COUNT: usize = 16;
/// A tile guaranteed to contain land data at every pyramid level
/// (Swiss Alps near Zermatt, z10). Used as a sanity check before
/// trusting an all-404 "this is ocean" verdict.
const KNOWN_LAND_TILE: TileKey = TileKey {
    z: 10,
    x: 534,
    y: 364,
};
/// Zoom selection tolerance: accept tile pixels up to this factor
/// coarser than the output grid cell before stepping the zoom up.
/// Mirrors the 1.5× rule in `fixed_tile::select_level_for_cell_size`
/// but tighter, so 1 m grid cells in Germany (cell ≈ 1.0 m, z15 px
/// ≈ 1.47 m) still select z16 (0.73 m) for parity with the national
/// 1 m DTM this provider replaced.
const ZOOM_CELL_TOLERANCE: f64 = 1.2;
/// Equatorial circumference of the Web-Mercator world in meters.
const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.686;
/// Negative-cache markers older than this are re-probed — Mapterhorn
/// actively adds coverage, so absent tiles may appear over time.
const MISSING_MARKER_MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60;
/// Maximum number of attempts for tile downloads (transient failures).
const TILE_DOWNLOAD_MAX_RETRIES: u32 = 3;
/// Base delay in milliseconds for exponential backoff between retries.
const TILE_DOWNLOAD_RETRY_BASE_DELAY_MS: u64 = 500;

/// RGB image buffer type for elevation tiles.
type TileImage = image::ImageBuffer<image::Rgb<u8>, Vec<u8>>;

/// Mapterhorn terrain tiles provider — global coverage.
///
/// Effective resolution varies by region: 30 m (GLO-30) as the global
/// floor, down to sub-meter where national LiDAR has been ingested.
pub struct Mapterhorn;

impl ElevationProvider for Mapterhorn {
    fn name(&self) -> &'static str {
        "mapterhorn"
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        None // Global coverage
    }

    /// Worst-case (global floor) resolution. Regional LiDAR areas are
    /// much finer; the actual fetch zoom adapts to the output grid.
    fn native_resolution_m(&self) -> f64 {
        30.0
    }

    fn fetch_raw(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
        let zoom = choose_zoom(bbox, grid_width, grid_height);

        let cache_dir = get_cache_dir(self.name());
        std::fs::create_dir_all(&cache_dir)?;

        let outcome = fetch_tile_pyramid(bbox, zoom, &cache_dir)?;

        if outcome.tiles.is_empty() && outcome.failed_downloads > 0 {
            // Nothing usable and at least one genuine failure: the
            // network/CDN is down, not an all-ocean bbox. Erroring out
            // lets the caller fall back to AWS Terrain Tiles.
            return Err(format!(
                "All Mapterhorn tile downloads failed ({} errors)",
                outcome.failed_downloads
            )
            .into());
        }

        println!(
            "Bilinear sampling {} Mapterhorn tiles into {}x{} grid...",
            outcome.tiles.len(),
            grid_width,
            grid_height
        );

        let tiles = &outcome.tiles;
        let confirmed_missing = &outcome.confirmed_missing;
        let floor = MIN_ZOOM.min(zoom);
        let height_grid: Vec<Vec<f64>> = (0..grid_height)
            .into_par_iter()
            .map(|gy| {
                let mut row = vec![f64::NAN; grid_width];
                for (gx, cell) in row.iter_mut().enumerate() {
                    let lat = bbox.max().lat()
                        - (gy as f64 / (grid_height - 1).max(1) as f64)
                            * (bbox.max().lat() - bbox.min().lat());
                    let lng = bbox.min().lng()
                        + (gx as f64 / (grid_width - 1).max(1) as f64)
                            * (bbox.max().lng() - bbox.min().lng());

                    // Zoom-independent normalized Web-Mercator coordinates
                    // in [0, 1]; per-zoom pixel coords derive by scaling.
                    let norm_x = (lng + 180.0) / 360.0;
                    let norm_y =
                        (1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0;

                    *cell = match sample_height(tiles, zoom, norm_x, norm_y) {
                        Some(v) => v,
                        // No tile at any pyramid level covers this cell.
                        // If the floor-level ancestor is a confirmed 404,
                        // no data can exist anywhere below it (a tile at
                        // zoom z always has ancestors at every coarser
                        // zoom) — the cell is definitively mid-ocean, so
                        // fill with 0.0 (sea level), matching Mapterhorn's
                        // convention for sea pixels inside coastal tiles.
                        // Otherwise the chain ended on a download failure:
                        // keep NaN so the caller's empty-data check / NaN
                        // fill can judge.
                        None => {
                            let key = tile_key_at(floor, norm_x, norm_y);
                            if confirmed_missing.contains(&key) {
                                0.0
                            } else {
                                f64::NAN
                            }
                        }
                    };
                }
                row
            })
            .collect();

        Ok(RawElevationGrid {
            heights_meters: height_grid,
        })
    }
}

// ─── Zoom selection ────────────────────────────────────────────────────

/// Pick the tile zoom whose ground resolution matches the output grid
/// cell size, within [`ZOOM_CELL_TOLERANCE`], bounded by the per-fetch
/// tile budget.
fn choose_zoom(bbox: &LLBBox, grid_width: usize, grid_height: usize) -> u8 {
    let (w_m, h_m) = bbox_dimensions_m(bbox);
    // (grid - 1) matches the sampling convention (`gx / (grid_width-1)`).
    let cell_x = w_m / (grid_width.saturating_sub(1)).max(1) as f64;
    let cell_y = h_m / (grid_height.saturating_sub(1)).max(1) as f64;
    // Use the finer axis so thin-strip bboxes don't get under-resolved.
    let cell_m = cell_x.min(cell_y).max(0.05);

    let mid_lat = (bbox.min().lat() + bbox.max().lat()) * 0.5;
    let cos_lat = mid_lat.to_radians().cos().abs().max(1e-6);

    // Ground meters per tile pixel at zoom z (Web Mercator):
    //   px_m(z) = EARTH_CIRCUMFERENCE_M * cos(lat) / (TILE_PX * 2^z)
    // Smallest z with px_m(z) <= cell_m * tolerance:
    let need = EARTH_CIRCUMFERENCE_M * cos_lat / (TILE_PX as f64 * cell_m * ZOOM_CELL_TOLERANCE);
    let mut zoom: u8 = if need <= 1.0 {
        0
    } else {
        (need.log2().ceil() as i64).clamp(0, MAX_ZOOM as i64) as u8
    };

    // Enforce the tile budget by stepping the zoom down; each step
    // quarters the tile count.
    while zoom > 0 && covering_tile_count(bbox, zoom) > MAX_TILES_PER_FETCH {
        zoom -= 1;
    }
    zoom
}

// ─── Tile keys and coordinates ─────────────────────────────────────────

/// One tile in the Web-Mercator XYZ pyramid.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct TileKey {
    z: u8,
    x: u32,
    y: u32,
}

impl TileKey {
    /// The tile one zoom level up that contains this tile.
    fn parent(&self) -> Option<TileKey> {
        if self.z == 0 {
            None
        } else {
            Some(TileKey {
                z: self.z - 1,
                x: self.x / 2,
                y: self.y / 2,
            })
        }
    }

    fn url(&self) -> String {
        MAPTERHORN_URL
            .replace("{z}", &self.z.to_string())
            .replace("{x}", &self.x.to_string())
            .replace("{y}", &self.y.to_string())
    }

    fn cache_path(&self, cache_dir: &Path) -> PathBuf {
        cache_dir.join(format!("z{}_x{}_y{}.webp", self.z, self.x, self.y))
    }

    /// Negative-cache marker recording a 404 for this tile.
    fn marker_path(&self, cache_dir: &Path) -> PathBuf {
        cache_dir.join(format!("z{}_x{}_y{}.missing", self.z, self.x, self.y))
    }
}

/// Inclusive tile-coordinate range covering the bbox at the given zoom.
fn covering_tile_range(bbox: &LLBBox, zoom: u8) -> (u32, u32, u32, u32) {
    let n = 2.0_f64.powi(zoom as i32);
    // Clamp via i64 so ±90° lat / +180° lng (legal LLBBox values) can't
    // wrap the u32 cast — same rationale as aws_terrain::lat_lng_to_tile.
    let n_tiles = n as i64;
    let clamp_tile = |v: f64| (v.floor() as i64).clamp(0, n_tiles - 1) as u32;
    let x1 = clamp_tile((bbox.min().lng() + 180.0) / 360.0 * n);
    let x2 = clamp_tile((bbox.max().lng() + 180.0) / 360.0 * n);
    let y1 = clamp_tile(
        (1.0 - bbox.max().lat().to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n,
    );
    let y2 = clamp_tile(
        (1.0 - bbox.min().lat().to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n,
    );
    (x1.min(x2), x1.max(x2), y1.min(y2), y1.max(y2))
}

fn covering_tile_count(bbox: &LLBBox, zoom: u8) -> usize {
    let (x1, x2, y1, y2) = covering_tile_range(bbox, zoom);
    ((x2 - x1 + 1) as usize) * ((y2 - y1 + 1) as usize)
}

fn covering_tile_keys(bbox: &LLBBox, zoom: u8) -> Vec<TileKey> {
    let (x1, x2, y1, y2) = covering_tile_range(bbox, zoom);
    let mut keys = Vec::with_capacity(covering_tile_count(bbox, zoom));
    for x in x1..=x2 {
        for y in y1..=y2 {
            keys.push(TileKey { z: zoom, x, y });
        }
    }
    keys
}

// ─── Fetching ──────────────────────────────────────────────────────────

/// Result of resolving one tile.
enum TileFetch {
    /// Tile downloaded (or loaded from cache) and decoded.
    Hit(TileImage),
    /// Server says the tile does not exist (404) — expected for ocean
    /// tiles and for zooms above the local source resolution. `cached`
    /// is true when answered from a fresh negative-cache marker rather
    /// than the network.
    Missing { cached: bool },
    /// Transient or permanent failure after retries.
    Failed(String),
}

struct FetchOutcome {
    tiles: FnvHashMap<TileKey, TileImage>,
    /// Tiles the server confirmed absent (404). Distinguishes "no data
    /// exists here" (ocean → sea-level fill) from "download failed"
    /// (→ NaN) when a grid cell's pyramid walk finds no tile.
    confirmed_missing: FnvHashSet<TileKey>,
    /// Count of genuine download failures (network / 5xx after retries).
    /// 404s are NOT failures — they mean "no tile here".
    failed_downloads: usize,
}

/// Mutable state threaded through every tile-fetch round.
struct PyramidState {
    tiles: FnvHashMap<TileKey, TileImage>,
    confirmed_missing: FnvHashSet<TileKey>,
    /// Network 404s observed this run. Persisted as `.missing` markers
    /// only at the END of the run, and only if at least one request
    /// returned 200 — a CDN that transiently 404s EXISTING tiles must
    /// not poison the negative cache for a month.
    pending_markers: Vec<TileKey>,
    /// Set when any network request returned a decodable 200.
    saw_network_success: std::sync::atomic::AtomicBool,
    failed_downloads: usize,
    /// Failures with no interleaved success/404, across chunks. Used as
    /// a circuit breaker for mid-run outages.
    consecutive_failures: usize,
}

/// Fetch one batch of keys (chunked, breaker-aware). Absent tiles are
/// appended to `absent`. Errors out when [`OUTAGE_FAILURE_THRESHOLD`]
/// consecutive requests failed — the service is down and every further
/// tile would burn the full retry budget before the AWS fallback runs.
fn fetch_level(
    keys: &[TileKey],
    client: &reqwest::blocking::Client,
    pool: &rayon::ThreadPool,
    cache_dir: &Path,
    state: &mut PyramidState,
    absent: &mut Vec<TileKey>,
) -> Result<(), Box<dyn std::error::Error>> {
    for chunk in keys.chunks(DOWNLOAD_CHUNK_SIZE) {
        let results: Vec<(TileKey, TileFetch)> = pool.install(|| {
            chunk
                .par_iter()
                .map(|key| {
                    (
                        *key,
                        fetch_tile(client, key, cache_dir, &state.saw_network_success),
                    )
                })
                .collect()
        });

        let mut chunk_failed = 0usize;
        for (key, result) in results {
            match result {
                TileFetch::Hit(img) => {
                    state.tiles.insert(key, img);
                }
                TileFetch::Missing { cached } => {
                    state.confirmed_missing.insert(key);
                    if !cached {
                        state.pending_markers.push(key);
                    }
                    absent.push(key);
                }
                TileFetch::Failed(e) => {
                    eprintln!(
                        "Warning: Mapterhorn tile z{} x{} y{} failed: {e}",
                        key.z, key.x, key.y
                    );
                    state.failed_downloads += 1;
                    chunk_failed += 1;
                    // Try the parent anyway — coarser data beats a hole,
                    // and the parent may already be cached locally.
                    absent.push(key);
                }
            }
        }

        if chunk_failed == chunk.len() {
            state.consecutive_failures += chunk_failed;
        } else {
            state.consecutive_failures = 0;
        }
        if state.consecutive_failures >= OUTAGE_FAILURE_THRESHOLD {
            return Err(format!(
                "Mapterhorn tile service unreachable mid-fetch ({} consecutive failures)",
                state.consecutive_failures
            )
            .into());
        }
    }
    Ok(())
}

/// Fetch all tiles covering `bbox` at `zoom`, then repeatedly re-request
/// the parents of whatever is absent, down to [`MIN_ZOOM`]. The result
/// map can therefore contain tiles at several zoom levels; sampling
/// walks the same pyramid top-down.
fn fetch_tile_pyramid(
    bbox: &LLBBox,
    zoom: u8,
    cache_dir: &Path,
) -> Result<FetchOutcome, Box<dyn std::error::Error>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!(
            "Arnis/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/louis-e/arnis)"
        ))
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(MAX_CONCURRENT_DOWNLOADS)
        .build()
        .map_err(|e| format!("Failed to create tile-fetch thread pool: {e}"))?;

    let mut state = PyramidState {
        tiles: FnvHashMap::default(),
        confirmed_missing: FnvHashSet::default(),
        pending_markers: Vec::new(),
        saw_network_success: std::sync::atomic::AtomicBool::new(false),
        failed_downloads: 0,
        consecutive_failures: 0,
    };

    let floor = MIN_ZOOM.min(zoom);
    let mut z = zoom;
    let mut level_keys = covering_tile_keys(bbox, z);

    // Reachability canary: probe one tile serially before unleashing
    // the fleet. A total outage (DNS failure, CDN down) is detected
    // within one request's worth of retries instead of burning the
    // full retry budget on every tile; the caller then falls back to
    // AWS. A 404 counts as proof of reachability — the server answered.
    // The canary's result lands in the disk cache, so the parallel
    // round below re-resolves it for free.
    if let Some(first) = level_keys.first() {
        if let TileFetch::Failed(e) =
            fetch_tile(&client, first, cache_dir, &state.saw_network_success)
        {
            return Err(format!("Mapterhorn tile service unreachable: {e}").into());
        }
    }

    // Zoom probe: before fanning a LARGE level out over a region that
    // may have no data at this zoom (e.g. an upscaled world over a
    // GLO-30-only area, where z16 would be ~2048 pointless 404s), test
    // a spread of tiles first. If every probe 404s, assume the level is
    // empty and step down a zoom — each step quarters the fan-out.
    // Sampling walks the pyramid anyway, so a small high-res pocket the
    // probes missed only costs one zoom level of detail, not a hole.
    while state.tiles.is_empty() && z > floor && level_keys.len() > PROBE_MIN_LEVEL_TILES {
        let stride = (level_keys.len() / PROBE_TILE_COUNT).max(1);
        let probe_keys: Vec<TileKey> = level_keys
            .iter()
            .copied()
            .step_by(stride)
            .take(PROBE_TILE_COUNT)
            .collect();
        let failed_before = state.failed_downloads;
        let mut probe_absent = Vec::new();
        fetch_level(
            &probe_keys,
            &client,
            &pool,
            cache_dir,
            &mut state,
            &mut probe_absent,
        )?;
        if !state.tiles.is_empty() || state.failed_downloads > failed_before {
            // Found data (fan out at this zoom) or hit errors (let the
            // main loop's breaker logic see the full picture).
            break;
        }
        println!(
            "Mapterhorn: no data at z{} in this area; trying z{}...",
            z,
            z - 1
        );
        z -= 1;
        level_keys = covering_tile_keys(bbox, z);
    }

    println!(
        "Downloading {} elevation tiles from Mapterhorn at z{} (up to {} concurrent)...",
        level_keys.len(),
        z,
        MAX_CONCURRENT_DOWNLOADS
    );

    let mut narrated = false;
    loop {
        let mut absent: Vec<TileKey> = Vec::new();
        fetch_level(
            &level_keys,
            &client,
            &pool,
            cache_dir,
            &mut state,
            &mut absent,
        )?;

        if absent.is_empty() || z <= floor {
            break;
        }

        let mut parents: Vec<TileKey> = absent.iter().filter_map(|k| k.parent()).collect();
        parents.sort_unstable_by_key(|k| (k.z, k.x, k.y));
        parents.dedup();
        parents.retain(|k| !state.tiles.contains_key(k));
        if parents.is_empty() {
            break;
        }

        if !narrated {
            // Only narrate the first fallback; ocean bboxes would
            // otherwise print one line per pyramid level.
            narrated = true;
            println!(
                "Mapterhorn: {} of {} tiles absent at z{} (coarser region or ocean); sampling parent tiles...",
                absent.len(),
                level_keys.len(),
                z
            );
        }

        level_keys = parents;
        z -= 1;
    }

    // All-404 sanity check: nothing found, nothing failed — every tile
    // 404'd. Before trusting the "this is all ocean" verdict, probe a
    // tile guaranteed to exist (Swiss Alps at z10). If even that 404s,
    // the CDN is misbehaving: error out so the caller falls back to
    // AWS, and persist no negative-cache markers. Warm-cache ocean
    // reruns resolve the sentinel from disk, so this usually costs
    // zero extra requests.
    if state.tiles.is_empty() && state.failed_downloads == 0 {
        match fetch_tile(
            &client,
            &KNOWN_LAND_TILE,
            cache_dir,
            &state.saw_network_success,
        ) {
            TileFetch::Hit(_) => {}
            TileFetch::Missing { .. } | TileFetch::Failed(_) => {
                return Err(
                    "Mapterhorn returned no tiles for this area and the known-land sanity \
                     tile also failed; treating as a service problem"
                        .into(),
                );
            }
        }
    }

    // Persist negative-cache markers only when this run proved the
    // service healthy (at least one decodable 200). Skipping persistence
    // on cache-only runs just means the next run re-probes a few 404s.
    if state
        .saw_network_success
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        for key in &state.pending_markers {
            let _ = std::fs::write(key.marker_path(cache_dir), b"");
        }
    }

    Ok(FetchOutcome {
        tiles: state.tiles,
        confirmed_missing: state.confirmed_missing,
        failed_downloads: state.failed_downloads,
    })
}

/// Resolve one tile: negative-cache marker → disk cache → download.
/// Sets `net_ok` when a network request returned a decodable 200 —
/// used to gate negative-cache persistence at the end of the run.
fn fetch_tile(
    client: &reqwest::blocking::Client,
    key: &TileKey,
    cache_dir: &Path,
    net_ok: &std::sync::atomic::AtomicBool,
) -> TileFetch {
    // Fresh negative-cache marker: known 404, skip the request.
    let marker = key.marker_path(cache_dir);
    if let Ok(meta) = std::fs::metadata(&marker) {
        let fresh = meta
            .modified()
            .ok()
            .and_then(|m| std::time::SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age.as_secs() < MISSING_MARKER_MAX_AGE_SECS);
        if fresh {
            return TileFetch::Missing { cached: true };
        }
        let _ = std::fs::remove_file(&marker);
    }

    // Disk cache. Validation is decode-based, NOT size-based: a flat
    // lossless-WebP tile (e.g. all-sea 0.0) is legitimately only a few
    // hundred bytes, so aws_terrain's <1000-byte heuristic would loop
    // on re-downloading perfectly valid tiles.
    let tile_path = key.cache_path(cache_dir);
    if tile_path.exists() {
        match image::open(&tile_path) {
            Ok(img) => return TileFetch::Hit(img.to_rgb8()),
            Err(e) => {
                eprintln!(
                    "Cached Mapterhorn tile at {} is corrupted: {e}. Re-downloading...",
                    tile_path.display()
                );
                let _ = std::fs::remove_file(&tile_path);
            }
        }
    }

    download_tile(client, key, &tile_path, net_ok)
}

fn download_tile(
    client: &reqwest::blocking::Client,
    key: &TileKey,
    tile_path: &Path,
    net_ok: &std::sync::atomic::AtomicBool,
) -> TileFetch {
    let url = key.url();
    let mut last_error = String::new();

    for attempt in 0..TILE_DOWNLOAD_MAX_RETRIES {
        if attempt > 0 {
            let delay_ms = TILE_DOWNLOAD_RETRY_BASE_DELAY_MS * (1 << (attempt - 1));
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        }

        let response = match client.get(&url).send() {
            Ok(r) => r,
            Err(e) => {
                last_error = e.to_string();
                continue;
            }
        };

        let status = response.status();
        if status.as_u16() == 404 {
            // Legitimate absence (ocean, or zoom above local source
            // resolution). The marker write is deferred to the end of
            // the run so a misbehaving CDN can't poison the cache.
            return TileFetch::Missing { cached: false };
        }
        if status.as_u16() == 429 || status.as_u16() == 403 {
            // Rate limiting / edge blocks: back off and retry rather
            // than escalating to the parent tile (which would send MORE
            // traffic exactly when the server asks for less).
            last_error = format!("HTTP {status} from {url}");
            continue;
        }
        if status.is_client_error() {
            // Other 4xx: malformed request, retrying won't help.
            return TileFetch::Failed(format!("HTTP {status} from {url}"));
        }
        if !status.is_success() {
            last_error = format!("HTTP {status} from {url}");
            continue;
        }

        let bytes = match response.bytes() {
            Ok(b) => b,
            Err(e) => {
                last_error = e.to_string();
                continue;
            }
        };
        let img = match image::load_from_memory(&bytes) {
            Ok(i) => i,
            Err(e) => {
                // 200 with an undecodable body (CDN error page): transient.
                last_error = format!("Invalid image payload: {e}");
                continue;
            }
        };
        net_ok.store(true, std::sync::atomic::Ordering::Relaxed);

        // Write-then-rename so a concurrent reader can never observe a
        // half-written tile. The process id in the suffix keeps two
        // Arnis processes sharing the cache from clobbering each
        // other's tmp file mid-write.
        static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp_path = tile_path.with_extension(format!("tmp{}-{}", std::process::id(), unique));
        if std::fs::write(&tmp_path, &bytes).is_ok() {
            if let Err(e) = std::fs::rename(&tmp_path, tile_path) {
                let _ = std::fs::remove_file(&tmp_path);
                // A concurrent writer stores identical bytes; a lost
                // cache write is not a fetch failure (see aws_terrain).
                if !tile_path.exists() {
                    eprintln!(
                        "Warning: failed to cache Mapterhorn tile {}: {e}",
                        tile_path.display()
                    );
                }
            }
        }
        return TileFetch::Hit(img.to_rgb8());
    }

    TileFetch::Failed(format!(
        "after {TILE_DOWNLOAD_MAX_RETRIES} attempts: {last_error}"
    ))
}

// ─── Sampling ──────────────────────────────────────────────────────────

/// Decode one Terrarium pixel to meters.
#[inline]
fn decode_terrarium(pixel: &image::Rgb<u8>) -> f64 {
    (pixel[0] as f64 * 256.0 + pixel[1] as f64 + pixel[2] as f64 / 256.0) - TERRARIUM_OFFSET
}

/// Tile key containing the given normalized Mercator coordinates at a
/// zoom level. Clamps via i64 so ±90° lat / +180° lng (legal LLBBox
/// values, mapping to norm coords at or beyond [0, 1]) can't wrap the
/// u32 cast — same rationale as `covering_tile_range`.
fn tile_key_at(z: u8, norm_x: f64, norm_y: f64) -> TileKey {
    let n = 2.0_f64.powi(z as i32);
    let n_tiles = n as i64;
    let x = (((norm_x * n).floor() as i64).clamp(0, n_tiles - 1)) as u32;
    let y = (((norm_y * n).floor() as i64).clamp(0, n_tiles - 1)) as u32;
    TileKey { z, x, y }
}

/// Bilinear-sample the height at normalized Mercator coordinates,
/// walking the pyramid from `top_zoom` down to [`MIN_ZOOM`] until a
/// fetched tile covers the point. Returns `None` when no tile at any
/// level covers it (mid-ocean).
fn sample_height(
    tiles: &FnvHashMap<TileKey, TileImage>,
    top_zoom: u8,
    norm_x: f64,
    norm_y: f64,
) -> Option<f64> {
    let floor = MIN_ZOOM.min(top_zoom);
    for z in (floor..=top_zoom).rev() {
        let key = tile_key_at(z, norm_x, norm_y);
        if !tiles.contains_key(&key) {
            continue;
        }

        let n = 2.0_f64.powi(z as i32);
        let fx = norm_x * n * TILE_PX as f64;
        let fy = norm_y * n * TILE_PX as f64;
        // At the exact +180° east / south world edge the clamped tile
        // key leaves px/py at or beyond TILE_PX (no tile exists further
        // out); snap onto the last real pixel so the edge column/row
        // samples data instead of NaN. In-range coordinates keep full
        // cross-tile bilinear behavior.
        let mut px = fx - key.x as f64 * TILE_PX as f64;
        let mut py = fy - key.y as f64 * TILE_PX as f64;
        if px >= TILE_PX as f64 {
            px = TILE_PX as f64 - 1.0;
        }
        if py >= TILE_PX as f64 {
            py = TILE_PX as f64 - 1.0;
        }
        let x0 = px.floor() as i32;
        let y0 = py.floor() as i32;
        let dx = (px - x0 as f64).clamp(0.0, 1.0);
        let dy = (py - y0 as f64).clamp(0.0, 1.0);

        let v00 = sample_tile_pixel(tiles, &key, x0, y0);
        let v10 = sample_tile_pixel(tiles, &key, x0 + 1, y0);
        let v01 = sample_tile_pixel(tiles, &key, x0, y0 + 1);
        let v11 = sample_tile_pixel(tiles, &key, x0 + 1, y0 + 1);

        // NaN-aware blend: corners that fall into an absent neighbor
        // tile (e.g. across a coastline where the neighbor is ocean)
        // just drop out of the weighting instead of poisoning the cell.
        let value = blend_finite_samples(v00, v10, v01, v11, dx, dy);
        if value.is_finite() {
            return Some(value);
        }
    }
    None
}

/// Read one pixel at the given zoom level, crossing into the adjacent
/// tile when the coordinate falls outside `key`'s 512×512 extent.
/// Returns NaN when the target tile isn't in the map.
fn sample_tile_pixel(
    tiles: &FnvHashMap<TileKey, TileImage>,
    key: &TileKey,
    px: i32,
    py: i32,
) -> f64 {
    let size = TILE_PX as i32;
    let (tx, x) = if px < 0 {
        (key.x.wrapping_sub(1), (px + size) as u32)
    } else if px >= size {
        (key.x + 1, (px - size) as u32)
    } else {
        (key.x, px as u32)
    };
    let (ty, y) = if py < 0 {
        (key.y.wrapping_sub(1), (py + size) as u32)
    } else if py >= size {
        (key.y + 1, (py - size) as u32)
    } else {
        (key.y, py as u32)
    };

    let Some(tile) = tiles.get(&TileKey {
        z: key.z,
        x: tx,
        y: ty,
    }) else {
        return f64::NAN;
    };
    if x >= tile.width() || y >= tile.height() {
        return f64::NAN;
    }
    decode_terrarium(tile.get_pixel(x, y))
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 512×512 tile whose every pixel encodes `height_m`.
    fn flat_tile(height_m: f64) -> TileImage {
        let raw = (height_m + TERRARIUM_OFFSET).round() as u32;
        let (r, g, b) = ((raw / 256) as u8, (raw % 256) as u8, 0u8);
        TileImage::from_pixel(TILE_PX, TILE_PX, image::Rgb([r, g, b]))
    }

    #[test]
    fn test_url_generation() {
        let key = TileKey {
            z: 12,
            x: 2138,
            y: 1430,
        };
        assert_eq!(key.url(), "https://tiles.mapterhorn.com/12/2138/1430.webp");
    }

    #[test]
    fn test_terrarium_decoding() {
        assert_eq!(decode_terrarium(&image::Rgb([128, 0, 0])), 0.0);
        assert_eq!(decode_terrarium(&image::Rgb([131, 232, 0])), 1000.0);
        assert_eq!(decode_terrarium(&image::Rgb([127, 156, 0])), -100.0);
    }

    #[test]
    fn test_parent_key() {
        let key = TileKey {
            z: 12,
            x: 2138,
            y: 1431,
        };
        assert_eq!(
            key.parent(),
            Some(TileKey {
                z: 11,
                x: 1069,
                y: 715
            })
        );
        assert_eq!(TileKey { z: 0, x: 0, y: 0 }.parent(), None);
    }

    #[test]
    fn test_choose_zoom_one_meter_cells_in_germany() {
        // ~1.1 km × 0.7 km bbox at Berlin's latitude with a 1 block/m
        // grid → 1 m cells → z16 (0.73 m/px), matching the 1 m national
        // DTM quality this provider replaces.
        let bbox = LLBBox::new(52.50, 13.40, 52.51, 13.41).unwrap();
        let (w_m, h_m) = bbox_dimensions_m(&bbox);
        let zoom = choose_zoom(&bbox, w_m as usize + 1, h_m as usize + 1);
        assert_eq!(zoom, 16);
    }

    #[test]
    fn test_choose_zoom_capped_grid_lowers_zoom() {
        // ~55 km bbox with the grid capped at 16384 → ~3.4 m cells →
        // z13–z14 territory, NOT z16.
        let bbox = LLBBox::new(52.0, 13.0, 52.5, 13.8).unwrap();
        let zoom = choose_zoom(&bbox, 16384, 16384);
        assert!(
            (12..=14).contains(&zoom),
            "expected mid zoom for capped grid, got z{zoom}"
        );
    }

    #[test]
    fn test_choose_zoom_respects_tile_budget() {
        // Force a fine grid over a huge bbox: the budget cap must pull
        // the zoom down until the covering tile count fits.
        let bbox = LLBBox::new(0.0, 0.0, 1.0, 1.0).unwrap();
        let zoom = choose_zoom(&bbox, 16384, 16384);
        assert!(covering_tile_count(&bbox, zoom) <= MAX_TILES_PER_FETCH);
    }

    #[test]
    fn test_choose_zoom_high_latitude_needs_lower_zoom() {
        // Mercator stretches at high latitude: ground m/px shrinks by
        // cos(lat), so Tromsø needs a lower zoom than Berlin for the
        // same cell size.
        let berlin = LLBBox::new(52.50, 13.40, 52.51, 13.41).unwrap();
        let tromso = LLBBox::new(69.64, 18.95, 69.65, 18.96).unwrap();
        let (bw, bh) = bbox_dimensions_m(&berlin);
        let (tw, th) = bbox_dimensions_m(&tromso);
        let zb = choose_zoom(&berlin, bw as usize + 1, bh as usize + 1);
        let zt = choose_zoom(&tromso, tw as usize + 1, th as usize + 1);
        assert!(zt <= zb);
    }

    #[test]
    fn test_sample_height_from_top_zoom() {
        let mut tiles: FnvHashMap<TileKey, TileImage> = FnvHashMap::default();
        tiles.insert(
            TileKey {
                z: 12,
                x: 2138,
                y: 1430,
            },
            flat_tile(1000.0),
        );
        // Point in the middle of that tile.
        let norm_x = (2138.0 + 0.5) / 4096.0;
        let norm_y = (1430.0 + 0.5) / 4096.0;
        let v = sample_height(&tiles, 12, norm_x, norm_y).unwrap();
        assert!((v - 1000.0).abs() < 0.51, "got {v}");
    }

    #[test]
    fn test_sample_height_pyramid_fallback() {
        // Only a z11 tile present; sampling at top_zoom 12 must fall
        // through to it.
        let mut tiles: FnvHashMap<TileKey, TileImage> = FnvHashMap::default();
        tiles.insert(
            TileKey {
                z: 11,
                x: 1069,
                y: 715,
            },
            flat_tile(250.0),
        );
        // A point inside z12 tile (2138, 1430), whose parent is (1069, 715).
        let norm_x = (2138.0 + 0.5) / 4096.0;
        let norm_y = (1430.0 + 0.5) / 4096.0;
        let v = sample_height(&tiles, 12, norm_x, norm_y).unwrap();
        assert!((v - 250.0).abs() < 0.51, "got {v}");
    }

    #[test]
    fn test_sample_height_ocean_returns_none() {
        let tiles: FnvHashMap<TileKey, TileImage> = FnvHashMap::default();
        assert!(sample_height(&tiles, 12, 0.4, 0.6).is_none());
    }

    #[test]
    fn test_covering_tile_range_matches_aws_tile_math() {
        // Same formula as aws_terrain::lat_lng_to_tile — verify a known
        // fix point: Zermatt (46.0207° N, 7.7491° E) lies in z12 tile
        // (2136, 1456).
        let bbox = LLBBox::new(46.01, 7.74, 46.03, 7.76).unwrap();
        let (x1, x2, y1, y2) = covering_tile_range(&bbox, 12);
        assert!(x1 <= 2136 && 2136 <= x2);
        assert!(y1 <= 1456 && 1456 <= y2);
    }

    #[test]
    #[ignore]
    fn test_live_tile_fetch_and_decode() {
        let client = reqwest::blocking::Client::new();
        // z12 tile containing Zermatt (46.0207° N, 7.7491° E).
        let key = TileKey {
            z: 12,
            x: 2136,
            y: 1456,
        };
        let response = client.get(key.url()).send().unwrap();
        assert!(response.status().is_success());
        let bytes = response.bytes().unwrap();
        let img = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(img.width(), TILE_PX);
        assert_eq!(img.height(), TILE_PX);
        // Zermatt area: Alpine heights, sane range.
        let heights: Vec<f64> = img.pixels().map(decode_terrarium).collect();
        let max = heights.iter().cloned().fold(f64::MIN, f64::max);
        let min = heights.iter().cloned().fold(f64::MAX, f64::min);
        assert!(max > 1500.0 && max < 5000.0, "max {max}");
        assert!(min > -100.0, "min {min}");
    }
}
