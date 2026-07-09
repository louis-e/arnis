//! Mapterhorn terrain tiles, the global elevation provider: terrarium 512px
//! lossless WebP, GLO-30 to z12, national LiDAR at z13-z18 (mapterhorn.com).
//!
//! Absent tiles 404 (including all pure-ocean tiles at every zoom) and fall
//! back per-tile to their pyramid parent; 404s are negative-cached on disk.

use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::cache::get_cache_dir;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};
use fnv::{FnvHashMap, FnvHashSet};
use rayon::prelude::*;
use std::path::{Path, PathBuf};

use super::fixed_tile::{bbox_dimensions_m, blend_finite_samples};

const MAPTERHORN_URL: &str = "https://tiles.mapterhorn.com/{z}/{x}/{y}.webp";
/// 512px tiles, so one zoom lower fetches the same ground resolution as AWS 256px.
const TILE_PX: u32 = 512;
const TERRARIUM_OFFSET: f64 = 32768.0;
/// z17 (~0.4 m/px) is only reachable with --scale > 1 over sub-meter source regions.
const MAX_ZOOM: u8 = 17;
/// Pyramid floor; any land tile exists at z6, only mid-ocean is absent there.
const MIN_ZOOM: u8 = 6;
const MAX_CONCURRENT_DOWNLOADS: usize = 8;
/// Tile budget per fetch; zoom is lowered until the covering count fits.
/// Worst case ~400 MB downloaded; decoding stays bounded by chunked sampling.
const MAX_TILES_PER_FETCH: usize = 2048;
/// Grid rows sampled per chunk; bounds decoded tiles to a few tile rows.
const SAMPLE_CHUNK_ROWS: usize = 1024;
/// Chunk size between outage-breaker checks during downloads.
const DOWNLOAD_CHUNK_SIZE: usize = 64;
/// Abort to the AWS fallback after this many consecutive failures.
const OUTAGE_FAILURE_THRESHOLD: usize = 128;
/// Levels larger than this get a spread probe before full fan-out.
const PROBE_MIN_LEVEL_TILES: usize = 64;
const PROBE_TILE_COUNT: usize = 16;
/// Accept tile pixels up to this factor coarser than a grid cell.
/// Tight enough that 1m grid cells in Germany still pick z16 (0.73 m/px).
const ZOOM_CELL_TOLERANCE: f64 = 1.2;
const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.686;
/// Re-probe negative-cache markers after this age; coverage grows over time.
const MISSING_MARKER_MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60;
const TILE_DOWNLOAD_MAX_RETRIES: u32 = 3;
const TILE_DOWNLOAD_RETRY_BASE_DELAY_MS: u64 = 500;
/// Swiss Alps tile that exists at every level; sanity check for all-404 results.
const KNOWN_LAND_TILE: TileKey = TileKey {
    z: 10,
    x: 534,
    y: 364,
};

type TileImage = image::ImageBuffer<image::Rgb<u8>, Vec<u8>>;

/// Global provider: 30m GLO-30 floor, sub-meter where national LiDAR exists.
pub struct Mapterhorn;

impl ElevationProvider for Mapterhorn {
    fn name(&self) -> &'static str {
        "mapterhorn"
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        None
    }

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

        if outcome.available.is_empty() && outcome.failed_downloads > 0 {
            return Err(format!(
                "All Mapterhorn tile downloads failed ({} errors)",
                outcome.failed_downloads
            )
            .into());
        }

        println!(
            "Sampling {} Mapterhorn tiles into {}x{} grid...",
            outcome.available.len(),
            grid_width,
            grid_height
        );

        let height_grid = sample_grid(bbox, zoom, &outcome, &cache_dir, grid_width, grid_height);

        Ok(RawElevationGrid {
            heights_meters: height_grid,
        })
    }
}

// ─── Zoom selection ────────────────────────────────────────────────────

/// Smallest zoom whose pixels match the grid cell size, within budget.
fn choose_zoom(bbox: &LLBBox, grid_width: usize, grid_height: usize) -> u8 {
    let (w_m, h_m) = bbox_dimensions_m(bbox);
    let cell_x = w_m / (grid_width.saturating_sub(1)).max(1) as f64;
    let cell_y = h_m / (grid_height.saturating_sub(1)).max(1) as f64;
    // Finer axis wins so thin strips don't get under-resolved.
    let cell_m = cell_x.min(cell_y).max(0.05);

    let mid_lat = (bbox.min().lat() + bbox.max().lat()) * 0.5;
    let cos_lat = mid_lat.to_radians().cos().abs().max(1e-6);

    // Ground m/px at zoom z is EARTH_CIRCUMFERENCE_M * cos(lat) / (512 * 2^z).
    let need = EARTH_CIRCUMFERENCE_M * cos_lat / (TILE_PX as f64 * cell_m * ZOOM_CELL_TOLERANCE);
    let mut zoom: u8 = if need <= 1.0 {
        0
    } else {
        (need.log2().ceil() as i64).clamp(0, MAX_ZOOM as i64) as u8
    };

    while zoom > 0 && covering_tile_count(bbox, zoom) > MAX_TILES_PER_FETCH {
        zoom -= 1;
    }
    zoom
}

// ─── Tile keys and coordinates ─────────────────────────────────────────

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct TileKey {
    z: u8,
    x: u32,
    y: u32,
}

impl TileKey {
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

    fn marker_path(&self, cache_dir: &Path) -> PathBuf {
        cache_dir.join(format!("z{}_x{}_y{}.missing", self.z, self.x, self.y))
    }
}

/// Inclusive tile range covering the bbox; i64 clamp so ±90°/±180° can't wrap.
fn covering_tile_range(bbox: &LLBBox, zoom: u8) -> (u32, u32, u32, u32) {
    let n = 2.0_f64.powi(zoom as i32);
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

/// Tile containing the given normalized mercator coords at a zoom level.
fn tile_key_at(z: u8, norm_x: f64, norm_y: f64) -> TileKey {
    let n = 2.0_f64.powi(z as i32);
    let n_tiles = n as i64;
    let x = (((norm_x * n).floor() as i64).clamp(0, n_tiles - 1)) as u32;
    let y = (((norm_y * n).floor() as i64).clamp(0, n_tiles - 1)) as u32;
    TileKey { z, x, y }
}

// ─── Fetching ──────────────────────────────────────────────────────────

enum TileFetch {
    /// Tile validated and present in the disk cache; sampling decodes it later.
    Hit,
    /// 404; `cached` when answered from a fresh negative-cache marker.
    Missing {
        cached: bool,
    },
    Failed(String),
}

struct FetchOutcome {
    /// Tiles fetched and present in the disk cache, across zoom levels.
    available: FnvHashSet<TileKey>,
    /// Server-confirmed 404s; distinguishes ocean (0.0 fill) from failures (NaN).
    confirmed_missing: FnvHashSet<TileKey>,
    failed_downloads: usize,
}

struct PyramidState {
    available: FnvHashSet<TileKey>,
    confirmed_missing: FnvHashSet<TileKey>,
    /// Network 404s, persisted as markers only after a proven-healthy run.
    pending_markers: Vec<TileKey>,
    saw_network_success: std::sync::atomic::AtomicBool,
    failed_downloads: usize,
    consecutive_failures: usize,
}

/// Fetch a batch of keys in chunks; errors out when the breaker trips.
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
                TileFetch::Hit => {
                    state.available.insert(key);
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
                    // Parent may still succeed or be cached; coarse data beats a hole.
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

/// Fetch bbox coverage at `zoom`, then parents of whatever is absent, down to MIN_ZOOM.
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
        available: FnvHashSet::default(),
        confirmed_missing: FnvHashSet::default(),
        pending_markers: Vec::new(),
        saw_network_success: std::sync::atomic::AtomicBool::new(false),
        failed_downloads: 0,
        consecutive_failures: 0,
    };

    let floor = MIN_ZOOM.min(zoom);
    let mut z = zoom;
    let mut level_keys = covering_tile_keys(bbox, z);

    // Serial canary detects a total outage in one request instead of thousands.
    if let Some(first) = level_keys.first() {
        if let TileFetch::Failed(e) =
            fetch_tile(&client, first, cache_dir, &state.saw_network_success)
        {
            return Err(format!("Mapterhorn tile service unreachable: {e}").into());
        }
    }

    // Spread-probe large levels first so a zoom with no local data costs
    // ~16 requests instead of hundreds of 404s.
    while state.available.is_empty() && z > floor && level_keys.len() > PROBE_MIN_LEVEL_TILES {
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
        if !state.available.is_empty() || state.failed_downloads > failed_before {
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
        parents.retain(|k| !state.available.contains(k));
        if parents.is_empty() {
            break;
        }

        if !narrated {
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

    // Verify an all-404 result against a known-land tile so a misbehaving
    // CDN falls back to AWS instead of producing a silent flat world.
    if state.available.is_empty() && state.failed_downloads == 0 {
        match fetch_tile(
            &client,
            &KNOWN_LAND_TILE,
            cache_dir,
            &state.saw_network_success,
        ) {
            TileFetch::Hit => {}
            TileFetch::Missing { .. } | TileFetch::Failed(_) => {
                return Err(
                    "Mapterhorn returned no tiles and the known-land sanity tile also failed"
                        .into(),
                );
            }
        }
    }

    // Persist 404 markers only when this run saw at least one good download.
    if state
        .saw_network_success
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        for key in &state.pending_markers {
            let _ = std::fs::write(key.marker_path(cache_dir), b"");
        }
    }

    Ok(FetchOutcome {
        available: state.available,
        confirmed_missing: state.confirmed_missing,
        failed_downloads: state.failed_downloads,
    })
}

/// Resolve one tile: negative-cache marker, disk cache, then download.
fn fetch_tile(
    client: &reqwest::blocking::Client,
    key: &TileKey,
    cache_dir: &Path,
    net_ok: &std::sync::atomic::AtomicBool,
) -> TileFetch {
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

    // Decode-based validation; flat lossless WebP tiles are legitimately tiny,
    // so a size heuristic like aws_terrain's would re-download valid tiles.
    let tile_path = key.cache_path(cache_dir);
    if tile_path.exists() {
        match image::open(&tile_path) {
            Ok(_) => return TileFetch::Hit,
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
            // Marker write deferred so a misbehaving CDN can't poison the cache.
            return TileFetch::Missing { cached: false };
        }
        if status.as_u16() == 429 || status.as_u16() == 403 {
            // Back off instead of escalating to the parent tile.
            last_error = format!("HTTP {status} from {url}");
            continue;
        }
        if status.is_client_error() {
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
        if let Err(e) = image::load_from_memory(&bytes) {
            last_error = format!("Invalid image payload: {e}");
            continue;
        }
        net_ok.store(true, std::sync::atomic::Ordering::Relaxed);

        // Sampling decodes from disk, so the write must land; write-then-rename
        // with a pid suffix keeps readers and other processes safe.
        static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp_path = tile_path.with_extension(format!("tmp{}-{}", std::process::id(), unique));
        let renamed = std::fs::write(&tmp_path, &bytes).is_ok()
            && std::fs::rename(&tmp_path, tile_path).is_ok();
        if !renamed {
            let _ = std::fs::remove_file(&tmp_path);
            // A concurrent writer may have stored identical bytes already.
            if !tile_path.exists() {
                return TileFetch::Failed(format!(
                    "cannot write tile cache at {}",
                    tile_path.display()
                ));
            }
        }
        return TileFetch::Hit;
    }

    TileFetch::Failed(format!(
        "after {TILE_DOWNLOAD_MAX_RETRIES} attempts: {last_error}"
    ))
}

// ─── Sampling ──────────────────────────────────────────────────────────

#[inline]
fn decode_terrarium(pixel: &image::Rgb<u8>) -> f64 {
    (pixel[0] as f64 * 256.0 + pixel[1] as f64 + pixel[2] as f64 / 256.0) - TERRARIUM_OFFSET
}

fn row_lat(bbox: &LLBBox, gy: usize, grid_height: usize) -> f64 {
    bbox.max().lat()
        - (gy as f64 / (grid_height - 1).max(1) as f64) * (bbox.max().lat() - bbox.min().lat())
}

fn norm_y_for_lat(lat: f64) -> f64 {
    (1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0
}

/// Sample the grid in row chunks, decoding only the tiles each chunk needs,
/// so decoded-tile memory stays at a few tile rows instead of the whole fetch.
fn sample_grid(
    bbox: &LLBBox,
    zoom: u8,
    outcome: &FetchOutcome,
    cache_dir: &Path,
    grid_width: usize,
    grid_height: usize,
) -> Vec<Vec<f64>> {
    let floor = MIN_ZOOM.min(zoom);
    let mut height_grid: Vec<Vec<f64>> = Vec::with_capacity(grid_height);
    let mut unreadable: FnvHashSet<TileKey> = FnvHashSet::default();

    for chunk_start in (0..grid_height).step_by(SAMPLE_CHUNK_ROWS) {
        let chunk_end = (chunk_start + SAMPLE_CHUNK_ROWS).min(grid_height);
        let tiles = load_chunk_tiles(
            outcome,
            cache_dir,
            bbox,
            zoom,
            floor,
            chunk_start,
            chunk_end,
            grid_height,
            &mut unreadable,
        );

        let mut rows: Vec<Vec<f64>> = (chunk_start..chunk_end)
            .into_par_iter()
            .map(|gy| {
                let lat = row_lat(bbox, gy, grid_height);
                let norm_y = norm_y_for_lat(lat);
                let mut row = vec![f64::NAN; grid_width];
                for (gx, cell) in row.iter_mut().enumerate() {
                    let lng = bbox.min().lng()
                        + (gx as f64 / (grid_width - 1).max(1) as f64)
                            * (bbox.max().lng() - bbox.min().lng());
                    let norm_x = (lng + 180.0) / 360.0;

                    *cell = match sample_height(&tiles, zoom, norm_x, norm_y) {
                        Some(v) => v,
                        // Confirmed-404 floor ancestor means ocean (0.0);
                        // otherwise a download failed and NaN lets the caller judge.
                        None => {
                            let key = tile_key_at(floor, norm_x, norm_y);
                            if outcome.confirmed_missing.contains(&key) {
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
        height_grid.append(&mut rows);
    }

    height_grid
}

/// Decode the available tiles a row chunk can touch, across all pyramid levels.
/// Bilinear crossings only ever go +1 tile, so only the max side needs margin.
#[allow(clippy::too_many_arguments)]
fn load_chunk_tiles(
    outcome: &FetchOutcome,
    cache_dir: &Path,
    bbox: &LLBBox,
    zoom: u8,
    floor: u8,
    chunk_start: usize,
    chunk_end: usize,
    grid_height: usize,
    unreadable: &mut FnvHashSet<TileKey>,
) -> FnvHashMap<TileKey, TileImage> {
    let norm_y_top = norm_y_for_lat(row_lat(bbox, chunk_start, grid_height));
    let norm_y_bot = norm_y_for_lat(row_lat(bbox, chunk_end.saturating_sub(1), grid_height));

    let mut tiles: FnvHashMap<TileKey, TileImage> = FnvHashMap::default();
    for z in floor..=zoom {
        let n_tiles = (1i64 << z).max(1);
        let ty_top = tile_key_at(z, 0.0, norm_y_top).y as i64;
        let ty_bot = tile_key_at(z, 0.0, norm_y_bot).y as i64;
        let ty_min = ty_top.min(ty_bot) as u32;
        let ty_max = (ty_top.max(ty_bot) + 1).clamp(0, n_tiles - 1) as u32;
        let (tx_min, x2, _, _) = covering_tile_range(bbox, z);
        let tx_max = (x2 as i64 + 1).clamp(0, n_tiles - 1) as u32;

        for ty in ty_min..=ty_max {
            for tx in tx_min..=tx_max {
                let key = TileKey { z, x: tx, y: ty };
                if !outcome.available.contains(&key) || unreadable.contains(&key) {
                    continue;
                }
                match image::open(key.cache_path(cache_dir)) {
                    Ok(img) => {
                        tiles.insert(key, img.to_rgb8());
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: cached Mapterhorn tile z{} x{} y{} unreadable during sampling: {e}",
                            key.z, key.x, key.y
                        );
                        unreadable.insert(key);
                    }
                }
            }
        }
    }
    tiles
}

/// Bilinear sample walking the pyramid top-down; None means no tile covers it.
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
        // At the exact +180/south world edge the clamped key leaves px/py
        // at TILE_PX; snap to the last real pixel instead of NaN.
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

        // NaN corners (absent neighbor tiles) drop out of the blend weighting.
        let value = blend_finite_samples(v00, v10, v01, v11, dx, dy);
        if value.is_finite() {
            return Some(value);
        }
    }
    None
}

/// One pixel at a zoom level, crossing into the adjacent tile at edges.
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
        let bbox = LLBBox::new(52.50, 13.40, 52.51, 13.41).unwrap();
        let (w_m, h_m) = bbox_dimensions_m(&bbox);
        let zoom = choose_zoom(&bbox, w_m as usize + 1, h_m as usize + 1);
        assert_eq!(zoom, 16);
    }

    #[test]
    fn test_choose_zoom_capped_grid_lowers_zoom() {
        let bbox = LLBBox::new(52.0, 13.0, 52.5, 13.8).unwrap();
        let zoom = choose_zoom(&bbox, 16384, 16384);
        assert!(
            (12..=14).contains(&zoom),
            "expected mid zoom for capped grid, got z{zoom}"
        );
    }

    #[test]
    fn test_choose_zoom_respects_tile_budget() {
        let bbox = LLBBox::new(0.0, 0.0, 1.0, 1.0).unwrap();
        let zoom = choose_zoom(&bbox, 16384, 16384);
        assert!(covering_tile_count(&bbox, zoom) <= MAX_TILES_PER_FETCH);
    }

    #[test]
    fn test_choose_zoom_high_latitude_needs_lower_zoom() {
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
        let norm_x = (2138.0 + 0.5) / 4096.0;
        let norm_y = (1430.0 + 0.5) / 4096.0;
        let v = sample_height(&tiles, 12, norm_x, norm_y).unwrap();
        assert!((v - 1000.0).abs() < 0.51, "got {v}");
    }

    #[test]
    fn test_sample_height_pyramid_fallback() {
        let mut tiles: FnvHashMap<TileKey, TileImage> = FnvHashMap::default();
        tiles.insert(
            TileKey {
                z: 11,
                x: 1069,
                y: 715,
            },
            flat_tile(250.0),
        );
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
        // Zermatt (46.0207 N, 7.7491 E) lies in z12 tile (2136, 1456).
        let bbox = LLBBox::new(46.01, 7.74, 46.03, 7.76).unwrap();
        let (x1, x2, y1, y2) = covering_tile_range(&bbox, 12);
        assert!(x1 <= 2136 && 2136 <= x2);
        assert!(y1 <= 1456 && 1456 <= y2);
    }

    #[test]
    fn test_sample_grid_chunked_from_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let bbox = LLBBox::new(46.0, 7.7, 46.05, 7.75).unwrap();
        // One z6 tile covers the whole bbox; z7/z8 stay absent so sampling walks down.
        let key = TileKey { z: 6, x: 33, y: 22 };
        flat_tile(300.0).save(key.cache_path(tmp.path())).unwrap();

        let mut available = FnvHashSet::default();
        available.insert(key);
        let outcome = FetchOutcome {
            available,
            confirmed_missing: FnvHashSet::default(),
            failed_downloads: 0,
        };

        // 2500 rows forces three sampling chunks.
        let grid = sample_grid(&bbox, 8, &outcome, tmp.path(), 64, 2500);
        assert_eq!(grid.len(), 2500);
        for row in &grid {
            for &v in row {
                assert!((v - 300.0).abs() < 0.51, "got {v}");
            }
        }
    }

    #[test]
    fn test_sample_grid_across_tile_row_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        // The z6 tile rows 22/23 meet at lat 45.0879; this bbox straddles it.
        let bbox = LLBBox::new(45.0, 7.7, 45.2, 7.72).unwrap();
        let north = TileKey { z: 6, x: 33, y: 22 };
        let south = TileKey { z: 6, x: 33, y: 23 };
        flat_tile(300.0).save(north.cache_path(tmp.path())).unwrap();
        flat_tile(100.0).save(south.cache_path(tmp.path())).unwrap();

        let mut available = FnvHashSet::default();
        available.insert(north);
        available.insert(south);
        let outcome = FetchOutcome {
            available,
            confirmed_missing: FnvHashSet::default(),
            failed_downloads: 0,
        };

        let grid = sample_grid(&bbox, 6, &outcome, tmp.path(), 32, 2500);
        assert!(
            (grid[0][0] - 300.0).abs() < 0.51,
            "north row got {}",
            grid[0][0]
        );
        assert!(
            (grid[2499][0] - 100.0).abs() < 0.51,
            "south row got {}",
            grid[2499][0]
        );
        for row in &grid {
            for &v in row {
                assert!(v.is_finite() && (99.0..=301.0).contains(&v), "got {v}");
            }
        }
    }

    #[test]
    fn test_sample_grid_ocean_fill_vs_failure_nan() {
        let tmp = tempfile::tempdir().unwrap();
        let bbox = LLBBox::new(46.0, 7.7, 46.01, 7.71).unwrap();
        let floor_key = tile_key_at(6, (7.705 + 180.0) / 360.0, norm_y_for_lat(46.005));

        let mut confirmed_missing = FnvHashSet::default();
        confirmed_missing.insert(floor_key);
        let ocean = FetchOutcome {
            available: FnvHashSet::default(),
            confirmed_missing,
            failed_downloads: 0,
        };
        let grid = sample_grid(&bbox, 8, &ocean, tmp.path(), 8, 8);
        assert!(grid.iter().flatten().all(|v| *v == 0.0));

        let failed = FetchOutcome {
            available: FnvHashSet::default(),
            confirmed_missing: FnvHashSet::default(),
            failed_downloads: 3,
        };
        let grid = sample_grid(&bbox, 8, &failed, tmp.path(), 8, 8);
        assert!(grid.iter().flatten().all(|v| v.is_nan()));
    }

    #[test]
    #[ignore]
    fn test_live_tile_fetch_and_decode() {
        let client = reqwest::blocking::Client::new();
        // z12 tile containing Zermatt.
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
        let heights: Vec<f64> = img.pixels().map(decode_terrarium).collect();
        let max = heights.iter().cloned().fold(f64::MIN, f64::max);
        let min = heights.iter().cloned().fold(f64::MAX, f64::min);
        assert!(max > 1500.0 && max < 5000.0, "max {max}");
        assert!(min > -100.0, "min {min}");
    }

    // Run manually: cargo test test_live_500km2 -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_live_500km2_scale_fetch() {
        // ~23x23 km around Munich at the 16384 grid cap, like a real 500 km2 run.
        let bbox = LLBBox::new(48.03, 11.40, 48.24, 11.71).unwrap();
        let raw = Mapterhorn.fetch_raw(&bbox, 16384, 16384).unwrap();
        assert_eq!(raw.heights_meters.len(), 16384);

        let mut finite = 0usize;
        let mut total = 0usize;
        let (mut min, mut max) = (f64::MAX, f64::MIN);
        for row in &raw.heights_meters {
            for &v in row {
                total += 1;
                if v.is_finite() {
                    finite += 1;
                    min = min.min(v);
                    max = max.max(v);
                }
            }
        }
        assert!(
            finite as f64 / total as f64 > 0.99,
            "finite {finite}/{total}"
        );
        assert!(min > 300.0 && max < 1200.0, "range {min}..{max}");
    }
}
