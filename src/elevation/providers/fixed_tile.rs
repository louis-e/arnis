//! Shared infrastructure for elevation providers that fetch from a
//! **fixed global Web Mercator tile grid**.
//!
//! Three providers currently use this pattern:
//!
//! - [`usgs_3dep`](super::usgs_3dep) — USGS 3D Elevation Program
//! - [`ign_france`](super::ign_france) — IGN France RGE ALTI
//! - [`ign_spain`](super::ign_spain)  — IGN España MDT
//!
//! AWS Terrain Tiles and Japan GSI already follow their own fixed-tile
//! conventions (XYZ-style Slippy Map tiles at specific zoom levels) and
//! don't need this module.
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
//! [`fetch_fixed_tile_grid`] handles the rest: choosing the resolution,
//! enumerating the covering tiles, downloading them with a capped-
//! concurrency thread pool, decoding the per-tile TIFFs, and bilinear-
//! sampling into the caller's output grid.

use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::cache::get_cache_dir;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};
use fnv::FnvHashMap;
use rayon::prelude::*;
use std::fmt::Debug;
use std::hash::Hash;
use std::path::{Path, PathBuf};

/// Pixels per tile edge. 512 matches the Tellus reference mod and the
/// classic slippy-map conventions; small enough that a tile usually
/// sits within a single LiDAR flight, large enough that total tile
/// counts stay reasonable for city-scale bboxes.
pub(super) const TILE_PIXELS: usize = 512;

/// Half-extent of the Web Mercator world in meters (EPSG:3857).
/// Longitude ±180° maps to mercator X ±MERCATOR_LIMIT.
pub(super) const MERCATOR_LIMIT: f64 = 20_037_508.342_789_244;

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

/// Enumerate every tile whose mercator bbox intersects the user's bbox.
pub(super) fn covering_tiles<R: Resolution>(bbox: &LLBBox, level: R) -> Vec<TileKey<R>> {
    let sw_mx = lon_to_mercator_x(bbox.min().lng());
    let ne_mx = lon_to_mercator_x(bbox.max().lng());
    let sw_my = lat_to_mercator_y(bbox.min().lat());
    let ne_my = lat_to_mercator_y(bbox.max().lat());
    let span = level.tile_span_meters();
    if span <= 0.0 {
        return Vec::new();
    }
    let min_tx = ((sw_mx + MERCATOR_LIMIT) / span).floor() as i32;
    let max_tx = (((ne_mx + MERCATOR_LIMIT) / span).ceil() as i32 - 1).max(min_tx);
    let min_ty = ((MERCATOR_LIMIT - ne_my) / span).floor() as i32;
    let max_ty = (((MERCATOR_LIMIT - sw_my) / span).ceil() as i32 - 1).max(min_ty);

    let capacity = ((max_tx - min_tx + 1) * (max_ty - min_ty + 1)).max(0) as usize;
    let mut tiles = Vec::with_capacity(capacity);
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
/// the coarsest is returned as a fallback.
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

pub(super) fn sample_tile_bilinear<R: Resolution>(
    tile: &[Vec<f64>],
    mx: f64,
    my: f64,
    key: &TileKey<R>,
) -> f64 {
    if tile.is_empty() || tile[0].is_empty() {
        return f64::NAN;
    }
    let mpp = key.level.meters_per_pixel();
    let local_x = (mx - key.min_mx()) / mpp;
    let local_y = (key.max_my() - my) / mpp;

    let height = tile.len() as i32;
    let width = tile[0].len() as i32;
    let max_x = width - 1;
    let max_y = height - 1;

    let x0 = (local_x.floor() as i32).clamp(0, max_x);
    let y0 = (local_y.floor() as i32).clamp(0, max_y);
    let x1 = (x0 + 1).min(max_x);
    let y1 = (y0 + 1).min(max_y);
    let dx = (local_x - x0 as f64).clamp(0.0, 1.0);
    let dy = (local_y - y0 as f64).clamp(0.0, 1.0);
    let v00 = tile[y0 as usize][x0 as usize];
    let v10 = tile[y0 as usize][x1 as usize];
    let v01 = tile[y1 as usize][x0 as usize];
    let v11 = tile[y1 as usize][x1 as usize];
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
    let level = select_level_for_cell_size(levels, cell_size_m);
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
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

    type FetchResult<R> = (TileKey<R>, Result<Vec<Vec<f64>>, String>);
    let tile_results: Vec<FetchResult<P::Level>> = pool.install(|| {
        tile_keys
            .par_iter()
            .map(|key| {
                let url = provider.tile_url(key);
                let cache_path = key.cache_path(&cache_dir);
                let res = fetch_tile_raster(&url, &cache_path, &client);
                (*key, res)
            })
            .collect()
    });

    let mut tile_cache: FnvHashMap<TileKey<P::Level>, Vec<Vec<f64>>> = FnvHashMap::default();
    let mut failed_keys: Vec<TileKey<P::Level>> = Vec::new();
    for (key, res) in tile_results {
        match res {
            Ok(raster) => {
                tile_cache.insert(key, raster);
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
    // AWS Terrain XYZ service. Lower resolution than USGS/IGN LiDAR but
    // far better than a NaN hole — and only runs for the small minority
    // of tiles that permanently failed upstream. Results stay in-memory
    // (no disk cache under the primary provider's path) so the next run
    // gets a fresh attempt at the primary source.
    let primary_failures = failed_keys.len();
    let mut fallback_recovered = 0usize;
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
                    tile_cache.insert(*key, raster);
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

    // Sampling runs on Rayon's global pool, NOT the download pool.
    // `pool` is sized to `MAX_CONCURRENT_DOWNLOADS` (4) to be polite to
    // upstream providers, but bilinear sampling is CPU-bound and has no
    // reason to be capped to 4 threads on high-core machines — doing so
    // needlessly slows multi-megapixel grids. The global pool defaults
    // to `num_cpus::get()` threads, which is what we want here.
    let height_grid: Vec<Vec<f64>> = (0..grid_height)
        .into_par_iter()
        .map(|gy| {
            let lat_frac = gy as f64 / h_denom;
            let lat = max_lat - lat_frac * lat_span;
            let my = lat_to_mercator_y(lat);
            let tile_y = TileKey::<P::Level>::for_mercator(level, 0.0, my).tile_y;
            let mut row = vec![f64::NAN; grid_width];
            // Carry the current tile reference across cells; tile_x
            // changes every ~TILE_PIXELS cells, so most cells reuse
            // the same tile and skip the hashmap lookup entirely.
            let mut cur_tile_x: i32 = i32::MIN;
            let mut cur_tile: Option<&Vec<Vec<f64>>> = None;
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
                    cur_tile = tile_cache.get(&cur_key);
                }
                if let Some(tile) = cur_tile {
                    *cell = sample_tile_bilinear(tile, col_mx[gx], my, &cur_key);
                }
            }
            row
        })
        .collect();

    Ok(RawElevationGrid {
        heights_meters: height_grid,
    })
}

// ─── Tile fetch + TIFF decode ──────────────────────────────────────────

fn fetch_tile_raster(
    url: &str,
    cache_path: &Path,
    client: &reqwest::blocking::Client,
) -> Result<Vec<Vec<f64>>, String> {
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = super::regional::fetch_or_cache(url, cache_path, Some(client))
        .map_err(|e| e.to_string())?;
    let raw = super::regional::decode_geotiff_f32(&bytes, TILE_PIXELS, TILE_PIXELS)
        .map_err(|e| e.to_string())?;
    Ok(raw.heights_meters)
}

/// Fill one fixed-grid tile from AWS Terrarium as a last-resort
/// fallback. Reprojects the tile's mercator bbox into lat/lng and asks
/// `AwsTerrain::fetch_raw` for a `TILE_PIXELS`-square grid; AWS handles
/// its own XYZ tile download and bilinear sampling internally. Returns
/// the result as the same `Vec<Vec<f64>>` shape the primary provider's
/// TIFF decoder would have produced, so the caller can insert it into
/// the shared tile cache and the downstream sampler is unaware the
/// data came from a different source.
fn fetch_aws_fallback_tile<R: Resolution>(
    key: &TileKey<R>,
) -> Result<Vec<Vec<f64>>, Box<dyn std::error::Error>> {
    let min_lng = mercator_x_to_lon(key.min_mx());
    let max_lng = mercator_x_to_lon(key.max_mx());
    let min_lat = mercator_y_to_lat(key.min_my());
    let max_lat = mercator_y_to_lat(key.max_my());
    let bbox = LLBBox::new(min_lat, min_lng, max_lat, max_lng)
        .map_err(|e| format!("Invalid AWS fallback bbox: {e}"))?;
    let raw = super::aws_terrain::AwsTerrain.fetch_raw(&bbox, TILE_PIXELS, TILE_PIXELS)?;
    Ok(raw.heights_meters)
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
        let tile = vec![vec![42.0; TILE_PIXELS]; TILE_PIXELS];
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
        let tile: Vec<Vec<f64>> = (0..TILE_PIXELS)
            .map(|y| {
                (0..TILE_PIXELS)
                    .map(|x| x as f64 + y as f64 * 1000.0)
                    .collect()
            })
            .collect();
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
}
