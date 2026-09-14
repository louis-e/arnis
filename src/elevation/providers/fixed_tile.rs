//! Shared infrastructure for elevation providers that fetch from a
//! **fixed global Web Mercator tile grid**.
//!
//! One provider currently uses this pattern:
//!
//! - [`usgs_3dep`](super::usgs_3dep) — USGS 3D Elevation Program
//!
//! Mapterhorn and AWS serve pre-rendered XYZ tiles and don't need this;
//! it exists for services that render a requested bbox server-side.
//!
//! # Why fixed tiles (summarised from the USGS module)
//!
//! When an upstream service composites multiple flights / surveys into
//! a single virtual raster, adjacent *user-bbox-relative* sub-requests
//! that cross a flight boundary disagree by tens-to-hundreds of metres.
//! Client-side averaging turns that step into a wide constant-slope
//! ramp, which triggers uniform slope-based material selection in
//! `ground_generation` and renders as a visible stripe cutting across
//! the generated world.
//!
//! Anchoring every request to a fixed global Mercator tile grid —
//! identified by `(level_id, tile_x, tile_y)` regardless of the user's
//! bbox — means:
//!
//! 1. Two users with different bboxes over the same area hit the same
//!    tile files on disk (cacheable, reproducible).
//! 2. Each 512-pixel tile covers a narrow enough physical area
//!    (512 m at 1 m/px) that most tiles live within a single flight,
//!    so upstream compositing produces consistent boundary values.
//! 3. Adjacent cells across a tile boundary read from different tiles
//!    independently — no averaging, no ramp, at worst a single-block
//!    step where the rendered terrain changes source.
//!
//! # How to use this module
//!
//! Each provider implements [`FixedTileProvider`] with:
//!
//! - A `CACHE_NAME` (for `<cache>/<CACHE_NAME>/<level>/<ty>/<tx>.tiff`).
//! - A `Resolution` enum that lists the discrete native pixel sizes the
//!   upstream supports.
//! - A `tile_url` that formats the upstream request for one tile's
//!   Mercator bbox. The request CRS is up to the provider — some
//!   services want EPSG:3857 directly, others want EPSG:4326 and we
//!   hand them the reprojected tile corners via [`mercator_x_to_lon`]
//!   and [`mercator_y_to_lat`].
//!
//! [`fetch_fixed_tile_grid`] handles the rest: choosing the resolution
//! (within a tile budget), enumerating the covering tiles, downloading
//! them with a capped-concurrency thread pool, then decoding the per-tile
//! TIFFs one tile row at a time while bilinear-sampling into the caller's
//! output grid.

use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::cache::get_cache_dir;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};
use fnv::{FnvHashMap, FnvHashSet};
use rayon::prelude::*;
use std::fmt::Debug;
use std::hash::Hash;
use std::path::{Path, PathBuf};

/// Pixels per tile edge. 512 matches classic slippy-map conventions;
/// small enough that a tile usually sits within a single LiDAR flight,
/// large enough that total tile counts stay reasonable for city-scale
/// bboxes.
pub(super) const TILE_PIXELS: usize = 512;

/// Half-extent of the Web Mercator world in meters (EPSG:3857).
/// Longitude ±180° maps to mercator X ±MERCATOR_LIMIT.
pub(super) const MERCATOR_LIMIT: f64 = 20_037_508.342_789_244;

/// Tile budget per fetch, shared with the zoom budgets in `aws_terrain` and
/// `mapterhorn`; the resolution level (or zoom) is coarsened until the
/// covering count fits.
pub(super) const MAX_TILES_PER_FETCH: usize = 2048;

/// Web Mercator has a usable latitude range of approximately ±85.051°.
pub(super) const MERCATOR_LAT_LIMIT: f64 = 85.051_128_78;

/// Earth radius used by the Web Mercator projection (EPSG:3857).
pub(super) const EARTH_RADIUS_M: f64 = 6_378_137.0;

// ─── Projection helpers ────────────────────────────────────────────────

#[inline]
pub(super) fn lon_to_mercator_x(lng: f64) -> f64 {
    lng.to_radians() * EARTH_RADIUS_M
}

#[inline]
pub(super) fn lat_to_mercator_y(lat: f64) -> f64 {
    let clamped = lat.clamp(-MERCATOR_LAT_LIMIT, MERCATOR_LAT_LIMIT);
    let rad = clamped.to_radians();
    EARTH_RADIUS_M * (std::f64::consts::FRAC_PI_4 + rad * 0.5).tan().ln()
}

#[inline]
pub(super) fn mercator_x_to_lon(mx: f64) -> f64 {
    mx / EARTH_RADIUS_M * 180.0 / std::f64::consts::PI
}

#[inline]
pub(super) fn mercator_y_to_lat(my: f64) -> f64 {
    (2.0 * (my / EARTH_RADIUS_M).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees()
}

// ─── Resolution + tile key ─────────────────────────────────────────────

/// One of a provider's supported native resolutions. A tiny trait so
/// each provider can keep its own strongly-typed enum while still
/// plugging into the shared tile infrastructure.
pub(super) trait Resolution:
    Copy + Clone + Debug + Hash + Eq + Send + Sync + 'static
{
    /// Identifier used in cache paths: e.g. `"r1"`, `"r3"`. Must be
    /// unique per resolution within a single provider.
    fn level_id(&self) -> &'static str;
    /// Native meters-per-pixel for this level, at the equator.
    fn meters_per_pixel(&self) -> f64;
    /// Size of one `TILE_PIXELS`-wide tile in mercator meters.
    #[inline]
    fn tile_span_meters(&self) -> f64 {
        TILE_PIXELS as f64 * self.meters_per_pixel()
    }
}

/// Identifies one tile in the global Mercator grid at a specific
/// resolution. Same real-world location always produces the same
/// `TileKey` — the property that makes cross-user, cross-session
/// caching work.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) struct TileKey<R: Resolution> {
    pub level: R,
    pub tile_x: i32,
    pub tile_y: i32,
}

impl<R: Resolution> TileKey<R> {
    pub fn for_mercator(level: R, mx: f64, my: f64) -> Self {
        let span = level.tile_span_meters();
        // Clamp to the same [0, max_tile] range covering_tiles emits.
        // At the exact east edge (mx == MERCATOR_LIMIT, i.e. lng = +180°)
        // or south edge (my == -MERCATOR_LIMIT, i.e. latitudes clamped
        // below the Mercator south limit), the raw floor produces `N`
        // while covering_tiles caps at `N - 1` via ceil()-1. Without the
        // clamp, those boundary samples would miss the cache and the
        // eastmost/southmost column/row would stay NaN.
        let max_tile = (((2.0 * MERCATOR_LIMIT) / span).ceil() as i32 - 1).max(0);
        let tile_x = (((mx + MERCATOR_LIMIT) / span).floor() as i32).clamp(0, max_tile);
        let tile_y = (((MERCATOR_LIMIT - my) / span).floor() as i32).clamp(0, max_tile);
        Self {
            level,
            tile_x,
            tile_y,
        }
    }

    pub fn min_mx(&self) -> f64 {
        -MERCATOR_LIMIT + self.tile_x as f64 * self.level.tile_span_meters()
    }

    pub fn max_mx(&self) -> f64 {
        self.min_mx() + self.level.tile_span_meters()
    }

    pub fn max_my(&self) -> f64 {
        MERCATOR_LIMIT - self.tile_y as f64 * self.level.tile_span_meters()
    }

    pub fn min_my(&self) -> f64 {
        self.max_my() - self.level.tile_span_meters()
    }

    pub fn cache_path(&self, cache_root: &Path) -> PathBuf {
        cache_root
            .join(self.level.level_id())
            .join(format!("{}", self.tile_y))
            .join(format!("{}.tiff", self.tile_x))
    }
}

// ─── Coverage + level selection ────────────────────────────────────────

/// Inclusive `(min_tx, max_tx, min_ty, max_ty)` tile range covering the
/// bbox; `None` for a degenerate level span.
fn covering_tile_range<R: Resolution>(bbox: &LLBBox, level: R) -> Option<(i32, i32, i32, i32)> {
    let span = level.tile_span_meters();
    if span <= 0.0 {
        return None;
    }
    let sw_mx = lon_to_mercator_x(bbox.min().lng());
    let ne_mx = lon_to_mercator_x(bbox.max().lng());
    let sw_my = lat_to_mercator_y(bbox.min().lat());
    let ne_my = lat_to_mercator_y(bbox.max().lat());
    let min_tx = ((sw_mx + MERCATOR_LIMIT) / span).floor() as i32;
    let max_tx = (((ne_mx + MERCATOR_LIMIT) / span).ceil() as i32 - 1).max(min_tx);
    let min_ty = ((MERCATOR_LIMIT - ne_my) / span).floor() as i32;
    let max_ty = (((MERCATOR_LIMIT - sw_my) / span).ceil() as i32 - 1).max(min_ty);
    Some((min_tx, max_tx, min_ty, max_ty))
}

/// Tile count from the range only; allocates nothing, so level selection
/// can probe it per candidate level.
pub(super) fn covering_tile_count<R: Resolution>(bbox: &LLBBox, level: R) -> usize {
    match covering_tile_range(bbox, level) {
        Some((min_tx, max_tx, min_ty, max_ty)) => {
            let cols = (max_tx - min_tx + 1).max(0) as usize;
            let rows = (max_ty - min_ty + 1).max(0) as usize;
            cols * rows
        }
        None => 0,
    }
}

/// Enumerate every tile whose mercator bbox intersects the user's bbox.
pub(super) fn covering_tiles<R: Resolution>(bbox: &LLBBox, level: R) -> Vec<TileKey<R>> {
    let Some((min_tx, max_tx, min_ty, max_ty)) = covering_tile_range(bbox, level) else {
        return Vec::new();
    };
    let mut tiles = Vec::with_capacity(covering_tile_count(bbox, level));
    for ty in min_ty..=max_ty {
        for tx in min_tx..=max_tx {
            tiles.push(TileKey {
                level,
                tile_x: tx,
                tile_y: ty,
            });
        }
    }
    tiles
}

/// Pick the finest level such that
/// `level.meters_per_pixel() * 1.5 >= cell_size_m`, i.e. the level
/// whose native pixels are no more than 1.5× finer than the output
/// cell. The factor tolerates a modest amount of *downsampling* from
/// the source (up to 1.5× finer-than-needed) before we give up on it
/// and step to the next coarser level, which avoids pulling dense
/// LiDAR tiles we'd immediately average away. Upsampling the other
/// direction (output finer than source) is unbounded by this rule —
/// if the user asks for 0.4 m cells on a 1 m source, the condition
/// `1.0 * 1.5 ≥ 0.4` holds easily and we use the 1 m level with
/// bilinear fill-in.
///
/// `levels` must be ordered finest-to-coarsest. When no level qualifies
/// the coarsest is returned as a fallback. Resolution alone ignores how
/// many tiles the bbox spans; callers pass the result through
/// [`coarsen_to_tile_budget`].
pub(super) fn select_level_for_cell_size<R: Resolution + Copy>(
    levels: &[R],
    cell_size_m: f64,
) -> R {
    if levels.is_empty() {
        // Caller must configure at least one level; this is a bug.
        panic!("select_level_for_cell_size called with empty levels");
    }
    if !cell_size_m.is_finite() || cell_size_m <= 0.0 {
        return levels[0];
    }
    for &level in levels {
        if level.meters_per_pixel() * 1.5 >= cell_size_m {
            return level;
        }
    }
    *levels.last().unwrap()
}

/// Step to coarser levels until the covering-tile count fits
/// [`MAX_TILES_PER_FETCH`], mirroring the zoom budget `aws_terrain` and
/// `mapterhorn` apply. Returns the coarsest level when none fits.
pub(super) fn coarsen_to_tile_budget<R: Resolution>(levels: &[R], start: R, bbox: &LLBBox) -> R {
    let start_idx = levels.iter().position(|l| *l == start).unwrap_or(0);
    let mut level = start;
    for &candidate in &levels[start_idx..] {
        level = candidate;
        if covering_tile_count(bbox, candidate) <= MAX_TILES_PER_FETCH {
            break;
        }
    }
    level
}

/// Approximate physical bbox dimensions in meters. Precise enough for
/// resolution-level selection.
pub(super) fn bbox_dimensions_m(bbox: &LLBBox) -> (f64, f64) {
    let mid_lat = (bbox.min().lat() + bbox.max().lat()) * 0.5;
    let mid_lat_cos = mid_lat.to_radians().cos().abs().max(1e-6);
    let width_deg = bbox.max().lng() - bbox.min().lng();
    let height_deg = bbox.max().lat() - bbox.min().lat();
    let width_m = width_deg.to_radians() * EARTH_RADIUS_M * mid_lat_cos;
    let height_m = height_deg.to_radians() * EARTH_RADIUS_M;
    (width_m.abs(), height_m.abs())
}

// ─── Bilinear tile sampling ────────────────────────────────────────────

/// One decoded tile, flat row-major. The upstream samples are f32, so
/// keeping them as f64 would quadruple the RAM for no extra precision.
pub(super) struct TileRaster {
    width: usize,
    height: usize,
    data: Vec<f32>,
}

impl TileRaster {
    fn from_rows(rows: Vec<Vec<f64>>) -> Self {
        let height = rows.len();
        let width = rows.first().map_or(0, |r| r.len());
        let mut data = vec![f32::NAN; width * height];
        for (y, row) in rows.iter().enumerate() {
            for (x, &v) in row.iter().take(width).enumerate() {
                data[y * width + x] = v as f32;
            }
        }
        Self {
            width,
            height,
            data,
        }
    }

    #[inline]
    fn get(&self, x: usize, y: usize) -> f64 {
        self.data[y * self.width + x] as f64
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

pub(super) fn sample_tile_bilinear<R: Resolution>(
    tile: &TileRaster,
    mx: f64,
    my: f64,
    key: &TileKey<R>,
) -> f64 {
    if tile.is_empty() {
        return f64::NAN;
    }
    let mpp = key.level.meters_per_pixel();
    let local_x = (mx - key.min_mx()) / mpp;
    let local_y = (key.max_my() - my) / mpp;

    let max_x = tile.width as i32 - 1;
    let max_y = tile.height as i32 - 1;

    let x0 = (local_x.floor() as i32).clamp(0, max_x);
    let y0 = (local_y.floor() as i32).clamp(0, max_y);
    let x1 = (x0 + 1).min(max_x);
    let y1 = (y0 + 1).min(max_y);
    let dx = (local_x - x0 as f64).clamp(0.0, 1.0);
    let dy = (local_y - y0 as f64).clamp(0.0, 1.0);
    let v00 = tile.get(x0 as usize, y0 as usize);
    let v10 = tile.get(x1 as usize, y0 as usize);
    let v01 = tile.get(x0 as usize, y1 as usize);
    let v11 = tile.get(x1 as usize, y1 as usize);
    blend_finite_samples(v00, v10, v01, v11, dx, dy)
}

pub(super) fn blend_finite_samples(
    v00: f64,
    v10: f64,
    v01: f64,
    v11: f64,
    dx: f64,
    dy: f64,
) -> f64 {
    let w00 = (1.0 - dx) * (1.0 - dy);
    let w10 = dx * (1.0 - dy);
    let w01 = (1.0 - dx) * dy;
    let w11 = dx * dy;
    let mut sum = 0.0;
    let mut weight = 0.0;
    if v00.is_finite() {
        sum += v00 * w00;
        weight += w00;
    }
    if v10.is_finite() {
        sum += v10 * w10;
        weight += w10;
    }
    if v01.is_finite() {
        sum += v01 * w01;
        weight += w01;
    }
    if v11.is_finite() {
        sum += v11 * w11;
        weight += w11;
    }
    if weight <= 0.0 {
        f64::NAN
    } else {
        sum / weight
    }
}

// ─── Provider trait + shared fetch driver ──────────────────────────────

/// Implement for any provider that fetches from a fixed global Mercator
/// tile grid. [`fetch_fixed_tile_grid`] uses this trait to handle
/// covering-tile enumeration, parallel download, and sampling; the
/// provider only has to answer "what's the request URL for this tile?".
pub(super) trait FixedTileProvider: Send + Sync {
    /// Resolution type used by this provider (usually a small enum).
    type Level: Resolution;

    /// Cache subdirectory name under the shared elevation cache root.
    /// Must be stable across releases so disk caches survive upgrades.
    const CACHE_NAME: &'static str;

    /// Maximum concurrent tile downloads. Default 4 is a sensible
    /// polite ceiling for most upstreams; providers on particularly
    /// flaky or strict services can lower it further.
    const MAX_CONCURRENT_DOWNLOADS: usize = 4;

    /// Tiles per download batch before a pause; 0 disables. Paces strict upstreams (USGS ArcGIS) so large bboxes don't trip rate limiting.
    const DOWNLOAD_BATCH_SIZE: usize = 256;

    /// Pause between download batches, in ms (applied only between batches when batching is on).
    const BATCH_PAUSE_MS: u64 = 1000;

    /// Resolution levels from finest (smallest m/px) to coarsest. The
    /// level-selection logic walks this list.
    fn resolution_levels(&self) -> &'static [Self::Level];

    /// Upstream URL for one tile. The implementation decides whether to
    /// request in EPSG:3857 (feed `min_mx..max_mx`, `min_my..max_my`
    /// directly) or EPSG:4326 (reproject the tile corners using
    /// [`mercator_x_to_lon`] / [`mercator_y_to_lat`]).
    fn tile_url(&self, key: &TileKey<Self::Level>) -> String;

    /// Provider-friendly log prefix for `fetch_fixed_tile_grid`. A
    /// default implementation derives it from `CACHE_NAME`.
    #[inline]
    fn log_prefix(&self) -> &'static str {
        Self::CACHE_NAME
    }
}

/// End-to-end fetch: pick resolution → enumerate covering tiles →
/// download+decode in parallel → bilinear-sample into the output grid.
pub(super) fn fetch_fixed_tile_grid<P: FixedTileProvider>(
    provider: &P,
    bbox: &LLBBox,
    grid_width: usize,
    grid_height: usize,
) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
    if grid_width == 0 || grid_height == 0 {
        return Err("Zero-dimensioned fixed-tile request".into());
    }

    let (bbox_w_m, bbox_h_m) = bbox_dimensions_m(bbox);
    // Divide by (grid - 1) to match the sampling convention below
    // (`lng_frac = gx / (grid_width - 1)`): actual per-cell spacing is
    // bbox_w_m / (grid_width - 1), not bbox_w_m / grid_width. Using the
    // wrong denominator here slightly underestimates cell size and can
    // push a borderline request into a finer level (more downloads).
    let w_div = (grid_width - 1).max(1) as f64;
    let h_div = (grid_height - 1).max(1) as f64;
    let cell_x = bbox_w_m / w_div;
    let cell_y = bbox_h_m / h_div;
    // Use the finer axis so we don't pick a coarse level when one
    // axis is much more sampled than the other (e.g. a thin strip).
    let cell_size_m = cell_x.min(cell_y);

    let levels = provider.resolution_levels();
    let requested = select_level_for_cell_size(levels, cell_size_m);
    let level = coarsen_to_tile_budget(levels, requested, bbox);
    if level != requested {
        eprintln!(
            "{}: {} would need {} tiles (budget {}), falling back to {} ({:.2} m/px)",
            provider.log_prefix(),
            requested.level_id(),
            covering_tile_count(bbox, requested),
            MAX_TILES_PER_FETCH,
            level.level_id(),
            level.meters_per_pixel(),
        );
    }
    let tile_keys = covering_tiles(bbox, level);
    if tile_keys.is_empty() {
        return Err("Fixed-tile bbox outside Mercator coverage".into());
    }

    let cache_dir = get_cache_dir(P::CACHE_NAME);
    std::fs::create_dir_all(&cache_dir)?;

    eprintln!(
        "{}: fetching {} fixed-grid tile{} at {} ({:.2} m/px), {} px/tile",
        provider.log_prefix(),
        tile_keys.len(),
        if tile_keys.len() == 1 { "" } else { "s" },
        level.level_id(),
        level.meters_per_pixel(),
        TILE_PIXELS
    );

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(P::MAX_CONCURRENT_DOWNLOADS)
        .build()
        .map_err(|e| format!("Failed to create tile-fetch thread pool: {e}"))?;

    // One blocking HTTP client shared across every tile in this run.
    // Builds the TLS stack + connection pool once; subsequent requests
    // reuse keep-alive connections, which matters when fetching dozens
    // of tiles from the same host.
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!(
            "Arnis/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/louis-e/arnis)"
        ))
        // A single tile is a few MB; a short connect timeout fails fast on a
        // stalled link so the retry fires instead of hanging the full request.
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

    type FetchResult<R> = (TileKey<R>, Result<(), String>);
    // Download in paced batches (cache hits short-circuit; result set unchanged) so large strict-upstream requests don't trip rate limiting.
    let batch_size = if P::DOWNLOAD_BATCH_SIZE == 0 {
        tile_keys.len().max(1)
    } else {
        P::DOWNLOAD_BATCH_SIZE
    };
    let total_batches = tile_keys.len().div_ceil(batch_size);
    let mut tile_results: Vec<FetchResult<P::Level>> = Vec::with_capacity(tile_keys.len());
    let mut prev_hit_network = false;
    for (batch_idx, batch) in tile_keys.chunks(batch_size).enumerate() {
        // Pause only between batches, and only after one that hit the network (warm-cache re-runs aren't slowed).
        if batch_idx > 0 && prev_hit_network && P::BATCH_PAUSE_MS > 0 {
            std::thread::sleep(std::time::Duration::from_millis(P::BATCH_PAUSE_MS));
        }
        // A missing cache file means this batch will download.
        prev_hit_network = batch.iter().any(|key| !key.cache_path(&cache_dir).exists());
        if total_batches > 1 {
            eprintln!(
                "{}: downloading tile batch {}/{} ({} tiles)...",
                provider.log_prefix(),
                batch_idx + 1,
                total_batches,
                batch.len(),
            );
        }
        let batch_results: Vec<FetchResult<P::Level>> = pool.install(|| {
            batch
                .par_iter()
                .map(|key| {
                    let url = provider.tile_url(key);
                    let cache_path = key.cache_path(&cache_dir);
                    let res = download_tile(&url, &cache_path, &client);
                    (*key, res)
                })
                .collect()
        });
        tile_results.extend(batch_results);
    }
    let mut available: FnvHashSet<TileKey<P::Level>> = FnvHashSet::default();
    let mut failed_keys: Vec<TileKey<P::Level>> = Vec::new();
    for (key, res) in tile_results {
        match res {
            Ok(()) => {
                available.insert(key);
            }
            Err(e) => {
                eprintln!(
                    "  {} tile ({}, {}) failed: {e}",
                    provider.log_prefix(),
                    key.tile_x,
                    key.tile_y
                );
                failed_keys.push(key);
            }
        }
    }

    // AWS Terrarium fallback: for any tile the primary provider couldn't
    // deliver after all retries, synthesise a replacement from the global
    // AWS Terrain XYZ service. Lower resolution than USGS LiDAR but
    // far better than a NaN hole — and only runs for the small minority
    // of tiles that permanently failed upstream. Results stay in-memory
    // (no disk cache under the primary provider's path) so the next run
    // gets a fresh attempt at the primary source.
    let primary_failures = failed_keys.len();
    let mut fallback_recovered = 0usize;
    let mut fallback_tiles: FnvHashMap<TileKey<P::Level>, TileRaster> = FnvHashMap::default();
    if !failed_keys.is_empty() {
        eprintln!(
            "{}: {} tile{} failed; attempting AWS Terrarium fallback...",
            provider.log_prefix(),
            primary_failures,
            if primary_failures == 1 { "" } else { "s" },
        );
        for key in &failed_keys {
            match fetch_aws_fallback_tile(key) {
                Ok(raster) => {
                    fallback_tiles.insert(*key, raster);
                    fallback_recovered += 1;
                }
                Err(e) => {
                    eprintln!(
                        "  AWS fallback for {} tile ({}, {}) failed: {e}",
                        provider.log_prefix(),
                        key.tile_x,
                        key.tile_y
                    );
                }
            }
        }
        let still_failed = primary_failures - fallback_recovered;
        if fallback_recovered > 0 {
            eprintln!(
                "{}: recovered {}/{} failed tile{} via AWS Terrarium",
                provider.log_prefix(),
                fallback_recovered,
                primary_failures,
                if primary_failures == 1 { "" } else { "s" },
            );
        }
        if still_failed > 0 {
            eprintln!(
                "{}: {}/{} tiles still failed; affected regions will be NaN-filled by post-processing",
                provider.log_prefix(),
                still_failed,
                tile_keys.len(),
            );
        }
    }

    let min_lat = bbox.min().lat();
    let max_lat = bbox.max().lat();
    let min_lng = bbox.min().lng();
    let max_lng = bbox.max().lng();
    let lng_span = max_lng - min_lng;
    let lat_span = max_lat - min_lat;
    let w_denom = (grid_width - 1).max(1) as f64;
    let h_denom = (grid_height - 1).max(1) as f64;

    // Precompute mercator X + tile_x once per output column. `tile_x`
    // depends only on `mx` (i.e. `gx`), so the per-cell hash-lookup key
    // is `(level, tile_x[gx], tile_y[gy])` — both components are column-
    // or row-constant. Hoisting the lon→mercator and tile-index work out
    // of the inner loop saves a few percent on multi-megapixel grids.
    // `TileKey::for_mercator` keeps the world-edge clamp logic in one
    // place; we pass a dummy `my = 0.0` here because we only need its
    // `tile_x` component (same trick on the row side below for tile_y).
    let col_mx: Vec<f64> = (0..grid_width)
        .map(|gx| {
            let lng_frac = gx as f64 / w_denom;
            let lng = min_lng + lng_frac * lng_span;
            lon_to_mercator_x(lng)
        })
        .collect();
    let col_tile_x: Vec<i32> = col_mx
        .iter()
        .map(|&mx| TileKey::<P::Level>::for_mercator(level, mx, 0.0).tile_x)
        .collect();

    // Same precompute on the row side: mercator Y and its tile_y per
    // output row, which also groups the rows into tile-row bands below.
    let row_my: Vec<f64> = (0..grid_height)
        .map(|gy| {
            let lat_frac = gy as f64 / h_denom;
            lat_to_mercator_y(max_lat - lat_frac * lat_span)
        })
        .collect();
    let row_tile_y: Vec<i32> = row_my
        .iter()
        .map(|&my| TileKey::<P::Level>::for_mercator(level, 0.0, my).tile_y)
        .collect();

    // Distinct tile columns, in order; col_tile_x is monotonic in gx.
    let mut band_tile_x: Vec<i32> = Vec::new();
    for &tx in &col_tile_x {
        if band_tile_x.last() != Some(&tx) {
            band_tile_x.push(tx);
        }
    }

    // Sample one tile row at a time. `sample_tile_bilinear` clamps inside
    // the tile, so a tile only ever serves the output rows whose tile_y it
    // is: bands are disjoint, every tile still decodes exactly once, and
    // peak decoded memory is one tile row instead of the whole fetch.
    //
    // Sampling runs on Rayon's global pool, NOT the download pool.
    // `pool` is sized to `MAX_CONCURRENT_DOWNLOADS` (4) to be polite to
    // upstream providers, but bilinear sampling is CPU-bound and has no
    // reason to be capped to 4 threads on high-core machines — doing so
    // needlessly slows multi-megapixel grids. The global pool defaults
    // to `num_cpus::get()` threads, which is what we want here.
    let mut height_grid: Vec<Vec<f64>> = Vec::with_capacity(grid_height);
    let mut band_start = 0usize;
    while band_start < grid_height {
        let tile_y = row_tile_y[band_start];
        let mut band_end = band_start + 1;
        while band_end < grid_height && row_tile_y[band_end] == tile_y {
            band_end += 1;
        }
        let band = load_tile_row(
            level,
            tile_y,
            &band_tile_x,
            &available,
            &fallback_tiles,
            &cache_dir,
            provider.log_prefix(),
        );

        let mut rows: Vec<Vec<f64>> = (band_start..band_end)
            .into_par_iter()
            .map(|gy| {
                let my = row_my[gy];
                let mut row = vec![f64::NAN; grid_width];
                // Carry the current tile reference across cells; tile_x
                // changes every ~TILE_PIXELS cells, so most cells reuse
                // the same tile and skip the hashmap lookup entirely.
                let mut cur_tile_x: i32 = i32::MIN;
                let mut cur_tile: Option<&TileRaster> = None;
                let mut cur_key = TileKey {
                    level,
                    tile_x: 0,
                    tile_y,
                };
                for (gx, cell) in row.iter_mut().enumerate() {
                    let tile_x = col_tile_x[gx];
                    if tile_x != cur_tile_x {
                        cur_tile_x = tile_x;
                        cur_key = TileKey {
                            level,
                            tile_x,
                            tile_y,
                        };
                        cur_tile = band.get(&cur_key).or_else(|| fallback_tiles.get(&cur_key));
                    }
                    if let Some(tile) = cur_tile {
                        *cell = sample_tile_bilinear(tile, col_mx[gx], my, &cur_key);
                    }
                }
                row
            })
            .collect();
        height_grid.append(&mut rows);
        band_start = band_end;
    }

    Ok(RawElevationGrid {
        heights_meters: height_grid,
    })
}

/// Decode one tile row from the disk cache. Keys already served by an
/// in-memory fallback raster, or never downloaded, are skipped.
fn load_tile_row<R: Resolution>(
    level: R,
    tile_y: i32,
    tile_xs: &[i32],
    available: &FnvHashSet<TileKey<R>>,
    fallback: &FnvHashMap<TileKey<R>, TileRaster>,
    cache_dir: &Path,
    log_prefix: &str,
) -> FnvHashMap<TileKey<R>, TileRaster> {
    let decoded: Vec<(TileKey<R>, Result<TileRaster, String>)> = tile_xs
        .par_iter()
        .filter_map(|&tile_x| {
            let key = TileKey {
                level,
                tile_x,
                tile_y,
            };
            if !available.contains(&key) || fallback.contains_key(&key) {
                return None;
            }
            Some((key, decode_cached_tile(&key.cache_path(cache_dir))))
        })
        .collect();

    let mut tiles: FnvHashMap<TileKey<R>, TileRaster> = FnvHashMap::default();
    for (key, res) in decoded {
        match res {
            Ok(raster) => {
                tiles.insert(key, raster);
            }
            Err(e) => {
                eprintln!(
                    "  {log_prefix} tile ({}, {}) unreadable during sampling: {e}",
                    key.tile_x, key.tile_y
                );
                // Same last resort a failed download gets.
                if let Ok(raster) = fetch_aws_fallback_tile(&key) {
                    tiles.insert(key, raster);
                }
            }
        }
    }
    tiles
}

// ─── Tile fetch + TIFF decode ──────────────────────────────────────────

/// Ensure the tile is in the disk cache. The payload is dropped here;
/// sampling decodes it later, one tile row at a time.
fn download_tile(
    url: &str,
    cache_path: &Path,
    client: &reqwest::blocking::Client,
) -> Result<(), String> {
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    super::regional::fetch_or_cache(url, cache_path, Some(client))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn decode_cached_tile(cache_path: &Path) -> Result<TileRaster, String> {
    let bytes = std::fs::read(cache_path).map_err(|e| e.to_string())?;
    let raw = super::regional::decode_geotiff_f32(&bytes, TILE_PIXELS, TILE_PIXELS)
        .map_err(|e| e.to_string())?;
    Ok(TileRaster::from_rows(raw.heights_meters))
}

/// Fill one fixed-grid tile from AWS Terrarium as a last-resort
/// fallback. Reprojects the tile's mercator bbox into lat/lng and asks
/// `AwsTerrain::fetch_raw` for a `TILE_PIXELS`-square grid; AWS handles
/// its own XYZ tile download and bilinear sampling internally. Returns
/// the same `TileRaster` the primary provider's TIFF decoder would have
/// produced, so the downstream sampler is unaware the data came from a
/// different source.
fn fetch_aws_fallback_tile<R: Resolution>(
    key: &TileKey<R>,
) -> Result<TileRaster, Box<dyn std::error::Error>> {
    let min_lng = mercator_x_to_lon(key.min_mx());
    let max_lng = mercator_x_to_lon(key.max_mx());
    let min_lat = mercator_y_to_lat(key.min_my());
    let max_lat = mercator_y_to_lat(key.max_my());
    let bbox = LLBBox::new(min_lat, min_lng, max_lat, max_lng)
        .map_err(|e| format!("Invalid AWS fallback bbox: {e}"))?;
    let raw = super::aws_terrain::AwsTerrain.fetch_raw(&bbox, TILE_PIXELS, TILE_PIXELS)?;
    Ok(TileRaster::from_rows(raw.heights_meters))
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal fake resolution type for unit tests.
    #[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
    enum TestLevel {
        M1,
        M3,
        M10,
    }

    impl Resolution for TestLevel {
        fn level_id(&self) -> &'static str {
            match self {
                Self::M1 => "r1",
                Self::M3 => "r3",
                Self::M10 => "r10",
            }
        }
        fn meters_per_pixel(&self) -> f64 {
            match self {
                Self::M1 => 1.0,
                Self::M3 => 3.435_973_836_8,
                Self::M10 => 10.307_921_510_4,
            }
        }
    }

    const LEVELS: &[TestLevel] = &[TestLevel::M1, TestLevel::M3, TestLevel::M10];

    /// Square bbox of the given side in kilometres, anchored at the given latitude.
    fn square_km(lat: f64, side_km: f64) -> LLBBox {
        let dlat = side_km * 1000.0 / 111_320.0;
        let dlon = dlat / lat.to_radians().cos();
        LLBBox::new(lat, -105.0, lat + dlat, -105.0 + dlon).unwrap()
    }

    /// What `fetch_fixed_tile_grid` does: resolution first, then the tile budget.
    fn selected_level(bbox: &LLBBox, cell_size_m: f64) -> TestLevel {
        let requested = select_level_for_cell_size(LEVELS, cell_size_m);
        coarsen_to_tile_budget(LEVELS, requested, bbox)
    }

    #[test]
    fn mercator_round_trip() {
        for (lat, lng) in [
            (36.88, -111.52),
            (48.13, 11.57),
            (-33.87, 151.21),
            (0.0, 0.0),
        ] {
            let mx = lon_to_mercator_x(lng);
            let my = lat_to_mercator_y(lat);
            let lng_back = mercator_x_to_lon(mx);
            let lat_back = mercator_y_to_lat(my);
            assert!((lng - lng_back).abs() < 1e-9);
            assert!((lat - lat_back).abs() < 1e-6);
        }
    }

    #[test]
    fn tile_bbox_contains_point_per_level() {
        for level in [TestLevel::M1, TestLevel::M3, TestLevel::M10] {
            let mx = -5_000_000.0;
            let my = 4_321_000.0;
            let k: TileKey<TestLevel> = TileKey::for_mercator(level, mx, my);
            assert!(k.min_mx() <= mx && mx < k.max_mx());
            assert!(k.min_my() < my && my <= k.max_my());
        }
    }

    /// Regression: samples exactly on the world's east / south Mercator
    /// edge used to produce a tile index one past `covering_tiles`' max,
    /// leaving the edge row/column NaN. `for_mercator`'s clamp now keeps
    /// those points inside `[0, max_tile]`, matching the range
    /// `covering_tiles` populates via `.ceil() - 1`.
    #[test]
    fn for_mercator_clamps_at_world_edges() {
        for level in [TestLevel::M1, TestLevel::M3, TestLevel::M10] {
            let span = level.tile_span_meters();
            let max_tile = ((2.0 * MERCATOR_LIMIT) / span).ceil() as i32 - 1;

            // East edge: mx == +MERCATOR_LIMIT. Without the clamp, the
            // raw floor produced max_tile + 1.
            let k_east: TileKey<TestLevel> = TileKey::for_mercator(level, MERCATOR_LIMIT, 0.0);
            assert!(k_east.tile_x >= 0 && k_east.tile_x <= max_tile);

            // South edge: my == -MERCATOR_LIMIT.
            let k_south: TileKey<TestLevel> = TileKey::for_mercator(level, 0.0, -MERCATOR_LIMIT);
            assert!(k_south.tile_y >= 0 && k_south.tile_y <= max_tile);

            // Interior points are untouched — the tile still contains
            // the sample coordinate.
            let mx = -5_000_000.0;
            let my = 4_321_000.0;
            let k_mid: TileKey<TestLevel> = TileKey::for_mercator(level, mx, my);
            assert!(k_mid.tile_x >= 0 && k_mid.tile_x <= max_tile);
            assert!(k_mid.tile_y >= 0 && k_mid.tile_y <= max_tile);
            assert!(k_mid.min_mx() <= mx && mx < k_mid.max_mx());
            assert!(k_mid.min_my() < my && my <= k_mid.max_my());
        }
    }

    #[test]
    fn covering_tiles_matches_expected_rectangle() {
        // Small bbox → exactly one tile at coarse resolution.
        let bbox = LLBBox::new(40.0, -105.0, 40.001, -104.999).unwrap();
        let tiles = covering_tiles(&bbox, TestLevel::M10);
        assert_eq!(tiles.len(), 1);

        // Larger bbox → multiple fine tiles.
        let bbox2 = LLBBox::new(36.870352, -111.535864, 36.892046, -111.505566).unwrap();
        let tiles2 = covering_tiles(&bbox2, TestLevel::M1);
        assert!(tiles2.len() >= 6);
        let min_tx = tiles2.iter().map(|k| k.tile_x).min().unwrap();
        let max_tx = tiles2.iter().map(|k| k.tile_x).max().unwrap();
        let min_ty = tiles2.iter().map(|k| k.tile_y).min().unwrap();
        let max_ty = tiles2.iter().map(|k| k.tile_y).max().unwrap();
        assert_eq!(
            tiles2.len(),
            ((max_tx - min_tx + 1) * (max_ty - min_ty + 1)) as usize
        );
    }

    #[test]
    fn covering_tile_count_matches_enumeration() {
        let bbox = LLBBox::new(36.870352, -111.535864, 36.892046, -111.505566).unwrap();
        for level in [TestLevel::M1, TestLevel::M3, TestLevel::M10] {
            assert_eq!(
                covering_tile_count(&bbox, level),
                covering_tiles(&bbox, level).len()
            );
        }
    }

    #[test]
    fn tile_budget_coarsens_only_when_exceeded() {
        let levels = &[TestLevel::M1, TestLevel::M3, TestLevel::M10];

        // Small bbox: well inside the budget, level untouched.
        let small = LLBBox::new(40.0, -105.0, 40.001, -104.999).unwrap();
        assert_eq!(
            coarsen_to_tile_budget(levels, TestLevel::M1, &small),
            TestLevel::M1
        );

        // ~33 x 33 km: over 5000 M1 tiles, so it steps to M3.
        let large = LLBBox::new(40.0, -105.0, 40.3, -104.7).unwrap();
        assert!(covering_tile_count(&large, TestLevel::M1) > MAX_TILES_PER_FETCH);
        let picked = coarsen_to_tile_budget(levels, TestLevel::M1, &large);
        assert_eq!(picked, TestLevel::M3);
        assert!(covering_tile_count(&large, picked) <= MAX_TILES_PER_FETCH);
    }

    #[test]
    fn level_selection_follows_cell_size() {
        let levels = &[TestLevel::M1, TestLevel::M3, TestLevel::M10];
        assert_eq!(select_level_for_cell_size(levels, 0.4), TestLevel::M1);
        assert_eq!(select_level_for_cell_size(levels, 1.0), TestLevel::M1);
        assert_eq!(select_level_for_cell_size(levels, 1.5), TestLevel::M1);
        assert_eq!(select_level_for_cell_size(levels, 3.0), TestLevel::M3);
        assert_eq!(select_level_for_cell_size(levels, 5.0), TestLevel::M3);
        assert_eq!(select_level_for_cell_size(levels, 9.0), TestLevel::M10);
        // Anything coarser than the coarsest level falls back to it.
        assert_eq!(select_level_for_cell_size(levels, 100.0), TestLevel::M10);
        // Pathological input defaults to the finest.
        assert_eq!(select_level_for_cell_size(levels, f64::NAN), TestLevel::M1);
        assert_eq!(select_level_for_cell_size(levels, -1.0), TestLevel::M1);
    }

    #[test]
    fn bilinear_on_constant_tile() {
        let key = TileKey {
            level: TestLevel::M1,
            tile_x: 0,
            tile_y: 0,
        };
        let tile = TileRaster::from_rows(vec![vec![42.0; TILE_PIXELS]; TILE_PIXELS]);
        let mx = (key.min_mx() + key.max_mx()) * 0.5;
        let my = (key.min_my() + key.max_my()) * 0.5;
        assert_eq!(sample_tile_bilinear(&tile, mx, my, &key), 42.0);
    }

    #[test]
    fn bilinear_on_linear_tile_exact_and_mid_pixel() {
        let key = TileKey {
            level: TestLevel::M1,
            tile_x: 0,
            tile_y: 0,
        };
        let tile = TileRaster::from_rows(
            (0..TILE_PIXELS)
                .map(|y| {
                    (0..TILE_PIXELS)
                        .map(|x| x as f64 + y as f64 * 1000.0)
                        .collect()
                })
                .collect(),
        );
        let mpp = key.level.meters_per_pixel();
        let mx_corner = key.min_mx() + 100.0 * mpp;
        let my_corner = key.max_my() - 50.0 * mpp;
        assert!((sample_tile_bilinear(&tile, mx_corner, my_corner, &key) - 50_100.0).abs() < 1e-6);
        let mx_mid = key.min_mx() + 100.5 * mpp;
        let my_mid = key.max_my() - 50.5 * mpp;
        let expected = (50_100.0 + 50_101.0 + 51_100.0 + 51_101.0) / 4.0;
        assert!((sample_tile_bilinear(&tile, mx_mid, my_mid, &key) - expected).abs() < 1e-6);
    }

    #[test]
    fn blend_finite_samples_is_nan_aware() {
        let v = blend_finite_samples(10.0, f64::NAN, 30.0, 40.0, 0.5, 0.5);
        assert!((v - 80.0 / 3.0).abs() < 1e-9);
        assert!(blend_finite_samples(f64::NAN, f64::NAN, f64::NAN, f64::NAN, 0.5, 0.5).is_nan());
    }

    #[test]
    fn coarsening_skips_every_level_that_is_still_over_budget() {
        // One degree square: M1 and M3 both blow the budget, M10 fits.
        let wide = LLBBox::new(40.0, -105.0, 41.0, -104.0).unwrap();
        assert!(covering_tile_count(&wide, TestLevel::M3) > MAX_TILES_PER_FETCH);
        assert_eq!(selected_level(&wide, 1.0), TestLevel::M10);
        assert!(covering_tile_count(&wide, TestLevel::M10) <= MAX_TILES_PER_FETCH);

        // Nothing fits: the coarsest level is the floor, not an out-of-range index.
        let huge = LLBBox::new(40.0, -105.0, 42.0, -103.0).unwrap();
        assert!(covering_tile_count(&huge, TestLevel::M10) > MAX_TILES_PER_FETCH);
        assert_eq!(selected_level(&huge, 1.0), TestLevel::M10);
    }

    #[test]
    fn mercator_inflation_coarsens_the_high_latitude_copy_of_one_bbox() {
        let span = 0.15;
        let equator = LLBBox::new(0.0, 0.0, span, span).unwrap();
        let high = LLBBox::new(60.0, 0.0, 60.0 + span, span).unwrap();
        assert!(
            covering_tile_count(&high, TestLevel::M1)
                > covering_tile_count(&equator, TestLevel::M1),
            "tile count must grow with latitude"
        );
        assert_eq!(selected_level(&equator, 1.0), TestLevel::M1);
        assert_eq!(selected_level(&high, 1.0), TestLevel::M3);
    }

    #[test]
    fn the_budget_never_returns_a_level_finer_than_the_resolution_choice() {
        let index = |l: TestLevel| LEVELS.iter().position(|c| *c == l).unwrap();
        for lat in [0.0, 39.0, 60.0] {
            for side_km in [1.0, 16.0, 24.5, 100.0] {
                for cell in [0.5, 1.0, 4.0, 12.0] {
                    let bbox = square_km(lat, side_km);
                    let requested = select_level_for_cell_size(LEVELS, cell);
                    let picked = coarsen_to_tile_budget(LEVELS, requested, &bbox);
                    assert!(
                        index(picked) >= index(requested),
                        "lat {lat}, {side_km} km, {cell} m cells: {picked:?} is finer than {requested:?}"
                    );
                }
            }
        }
    }
}
