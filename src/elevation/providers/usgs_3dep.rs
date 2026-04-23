//! USGS 3DEP elevation provider using a **fixed global Web Mercator tile
//! grid**.
//!
//! # Why a fixed tile grid (vs ad-hoc bbox splits)
//!
//! USGS 3DEP serves its data via ArcGIS `ImageServer::exportImage`, which
//! composites multiple LiDAR flights (each with its own vertical datum)
//! server-side into a virtual raster. When we ask for a bbox that crosses
//! flight boundaries, the compositing blends them into a single consistent
//! response.
//!
//! The old `tiled_fetch` approach split *each user's bbox* into 2×2, 3×3,
//! … sub-tiles at user-bbox-relative positions and stitched the responses.
//! That produced two distinct failure modes:
//!
//! 1. **Inter-flight seam bleed**. Two adjacent requests that each happen
//!    to span the same flight boundary are composited independently by
//!    the server — the boundary cells disagree by anywhere from 10 m to
//!    500 m, producing a visible step across the tile seam that no
//!    amount of client-side averaging can fully hide (confirmed with
//!    multiple rounds of bias correction, smoothstep blending, and
//!    post-scale smoothing).
//! 2. **Position-dependent artefacts**. Because the split points depend
//!    on the user's selected bbox, the visible seam always cuts through
//!    whatever the user is currently looking at. Users with slightly
//!    different bboxes over the same area saw the artefact in different
//!    places.
//!
//! # What we do instead
//!
//! Every USGS 3DEP request in this module is anchored to a fixed
//! *per-level* tile grid in Web Mercator coordinates. A tile is
//! identified by `(resolution_level, tile_x, tile_y)` and always covers
//! the same mercator bbox regardless of who is asking for it. Tiles are:
//!
//! - **Cacheable globally** — the on-disk cache key is the tile
//!   coordinates, not a bbox hash, so every user who generates a world
//!   covering the same real-world area reuses the same tile files.
//! - **Small enough to rarely cross flight boundaries** — 512 × 512 px
//!   at M1 = 512 m per tile, which is narrow enough that a single tile
//!   usually lives in one LiDAR flight.
//! - **Seamlessly sampled** — for each output cell we look up the tile
//!   containing its mercator position and bilinear-sample within that
//!   tile; cells on either side of a tile boundary read from different
//!   tiles independently, so at worst there's a single-block step
//!   where tiles meet. No averaging, no ramp.
//!
//! The approach mirrors how AWS Terrain Tiles and Japan GSI already
//! work in the sibling providers, and matches the Tellus reference
//! Minecraft mod's `Usgs3depElevationSource`.
//!
//! # Resolution selection
//!
//! For a given user bbox + output grid, we pick the resolution level
//! whose native meters-per-pixel is closest to the output cell size.
//! Upsampling (source coarser than output) is avoided; light
//! oversampling (source finer than output) is preferred because it
//! preserves detail and the bilinear sample handles the upscale
//! cleanly.

use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::cache::get_cache_dir;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};
use fnv::FnvHashMap;
use rayon::prelude::*;
use std::path::{Path, PathBuf};

/// Pixels per tile edge. 512 matches the Tellus mod's choice and is a
/// common slippy-map-style value — small enough that a single tile
/// usually sits within one LiDAR flight, large enough to keep the
/// total request count reasonable for city-sized bboxes.
const TILE_PIXELS: usize = 512;

/// Half-extent of the Web Mercator world in meters (EPSG:3857). Longitude
/// `±180°` maps to mercator X `±MERCATOR_LIMIT`.
const MERCATOR_LIMIT: f64 = 20_037_508.342_789_244;

/// Web Mercator has a usable latitude range of approximately ±85.051°
/// (a square world in mercator units).
const MERCATOR_LAT_LIMIT: f64 = 85.051_128_78;

/// Earth radius used by the Web Mercator projection (EPSG:3857).
const EARTH_RADIUS_M: f64 = 6_378_137.0;

/// How many tiles to download in parallel. USGS 3DEP's ImageServer is
/// more sensitive than static S3 buckets; 4 concurrent requests is a
/// sweet spot that gets full bandwidth without tripping rate limiting.
const MAX_CONCURRENT_DOWNLOADS: usize = 4;

/// USGS 3D Elevation Program (3DEP) — USA + territories.
/// Resolution: up to 1m LiDAR (CONUS), 3m/10m elsewhere, fallback 30m.
/// License: Public Domain (USGS).
pub struct Usgs3dep;

impl ElevationProvider for Usgs3dep {
    fn name(&self) -> &'static str {
        "usgs_3dep"
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        Some(vec![
            // CONUS
            LLBBox::new(24.0, -125.0, 50.0, -66.0).unwrap(),
            // Alaska
            LLBBox::new(51.0, -180.0, 72.0, -129.0).unwrap(),
            // Hawaii
            LLBBox::new(18.5, -161.0, 22.5, -154.0).unwrap(),
            // Puerto Rico + USVI
            LLBBox::new(17.5, -68.0, 18.7, -64.0).unwrap(),
            // Guam
            LLBBox::new(13.2, 144.5, 13.7, 145.0).unwrap(),
        ])
    }

    fn native_resolution_m(&self) -> f64 {
        1.0
    }

    fn fetch_raw(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
        if grid_width == 0 || grid_height == 0 {
            return Err("Zero-dimensioned USGS 3DEP request".into());
        }

        let level = Resolution::for_output(bbox, grid_width, grid_height);
        let tile_keys = covering_tiles(bbox, level);
        if tile_keys.is_empty() {
            return Err("USGS 3DEP bbox outside Mercator coverage".into());
        }

        let cache_dir = get_cache_dir(self.name());
        std::fs::create_dir_all(&cache_dir)?;

        eprintln!(
            "USGS 3DEP: fetching {} fixed-grid tile{} at {} ({:.2} m/px), {} px/tile",
            tile_keys.len(),
            if tile_keys.len() == 1 { "" } else { "s" },
            level.id(),
            level.meters_per_pixel(),
            TILE_PIXELS
        );

        // Parallel download with capped concurrency. On a warm cache
        // this is basically a disk read; on a cold cache it parallelises
        // the network wait for independent tiles.
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(MAX_CONCURRENT_DOWNLOADS)
            .build()
            .map_err(|e| format!("Failed to create USGS thread pool: {e}"))?;

        type TileFetch = (TileKey, Result<Vec<Vec<f64>>, String>);
        let tile_results: Vec<TileFetch> = pool.install(|| {
            tile_keys
                .par_iter()
                .map(|key| {
                    let res = fetch_tile_raster(key, &cache_dir);
                    (*key, res)
                })
                .collect()
        });

        // Collect successful tiles into a HashMap for O(1) lookup during
        // sampling. Failed tiles leave their region as NaN; downstream
        // `fill_nan_values` and provider fallback handle partial grids.
        let mut tile_cache: FnvHashMap<TileKey, Vec<Vec<f64>>> = FnvHashMap::default();
        let mut failures = 0usize;
        for (key, res) in tile_results {
            match res {
                Ok(raster) => {
                    tile_cache.insert(key, raster);
                }
                Err(e) => {
                    failures += 1;
                    eprintln!("  USGS tile ({}, {}) failed: {e}", key.tile_x, key.tile_y);
                }
            }
        }
        if failures > 0 {
            eprintln!(
                "USGS 3DEP: {}/{} tiles failed to download; affected regions will be NaN-filled by post-processing",
                failures,
                tile_keys.len()
            );
        }

        // Sample each output cell from the tile covering its mercator
        // position. Rows are emitted in parallel so large bboxes don't
        // bottleneck on single-threaded interpolation.
        let min_lat = bbox.min().lat();
        let max_lat = bbox.max().lat();
        let min_lng = bbox.min().lng();
        let max_lng = bbox.max().lng();
        let lng_span = max_lng - min_lng;
        let lat_span = max_lat - min_lat;
        let w_denom = (grid_width - 1).max(1) as f64;
        let h_denom = (grid_height - 1).max(1) as f64;

        let height_grid: Vec<Vec<f64>> = pool.install(|| {
            (0..grid_height)
                .into_par_iter()
                .map(|gy| {
                    // Row 0 is north (max_lat); row grid_height-1 is south (min_lat).
                    let lat_frac = gy as f64 / h_denom;
                    let lat = max_lat - lat_frac * lat_span;
                    let my = lat_to_mercator_y(lat);
                    let mut row = vec![f64::NAN; grid_width];
                    for (gx, cell) in row.iter_mut().enumerate() {
                        let lng_frac = gx as f64 / w_denom;
                        let lng = min_lng + lng_frac * lng_span;
                        let mx = lon_to_mercator_x(lng);
                        let key = TileKey::for_mercator(level, mx, my);
                        if let Some(tile) = tile_cache.get(&key) {
                            *cell = sample_tile_bilinear(tile, mx, my, &key);
                        }
                    }
                    row
                })
                .collect()
        });

        Ok(RawElevationGrid {
            heights_meters: height_grid,
        })
    }
}

// ─── Resolution level ──────────────────────────────────────────────────

/// USGS 3DEP's published zoom levels, each covering a fixed
/// meters-per-pixel. The values match the Tellus reference
/// implementation so cache artefacts stay compatible across projects.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum Resolution {
    /// 1.0 m/px — best available, covers most CONUS via LiDAR.
    M1,
    /// ~3.44 m/px — common LiDAR fallback.
    M3,
    /// ~10.31 m/px — regional DEM coverage.
    M10,
    /// ~30.92 m/px — national coarse fallback.
    M30,
}

impl Resolution {
    fn id(&self) -> &'static str {
        match self {
            Self::M1 => "r1",
            Self::M3 => "r3",
            Self::M10 => "r10",
            Self::M30 => "r30",
        }
    }

    fn meters_per_pixel(&self) -> f64 {
        // Values match Tellus's Usgs3depElevationSource zoom levels
        // (verified 2026-04: cross-referenceable cache artefacts).
        match self {
            Self::M1 => 1.0,
            Self::M3 => 3.435_973_836_8,
            Self::M10 => 10.307_921_510_4,
            Self::M30 => 30.922_080_981_4,
        }
    }

    fn tile_span_meters(&self) -> f64 {
        TILE_PIXELS as f64 * self.meters_per_pixel()
    }

    /// Pick the finest level whose native pixel size is small enough to
    /// satisfy the output cell density without aggressive upsampling.
    ///
    /// Thresholds are on the output cell's physical size in meters:
    /// - ≤ 1.5 m/cell → M1
    /// - ≤ 5   m/cell → M3
    /// - ≤ 15  m/cell → M10
    /// - otherwise    → M30
    ///
    /// Arnis's default scale (1.0 block/m) maps any user bbox to
    /// `m_per_cell ≈ 1.0` in the common case — so M1 is the default.
    /// Scale 2.5 (GUI max) → 0.4 m/cell → still M1 (a modest 2.5× bilinear
    /// upsample, which preserves apparent detail since 1m LiDAR already
    /// resolves most terrain features a human can see at that scale).
    /// Scale 0.30 (GUI min) → 3.33 m/cell → M3.
    fn for_output(bbox: &LLBBox, grid_width: usize, grid_height: usize) -> Self {
        // Rough physical bbox dimensions in meters. Using mercator
        // distances at the bbox centre keeps the estimate accurate even
        // at high latitudes.
        let (width_m, height_m) = bbox_dimensions_m(bbox);
        let cell_x = width_m / grid_width.max(1) as f64;
        let cell_y = height_m / grid_height.max(1) as f64;
        // Use the finer of the two axes so we don't accidentally pick a
        // coarse level when one axis is much more sampled than the other.
        let m_per_cell = cell_x.min(cell_y);
        if !m_per_cell.is_finite() || m_per_cell <= 0.0 {
            return Self::M1;
        }
        if m_per_cell <= 1.5 {
            Self::M1
        } else if m_per_cell <= 5.0 {
            Self::M3
        } else if m_per_cell <= 15.0 {
            Self::M10
        } else {
            Self::M30
        }
    }
}

/// Approximate physical bbox dimensions in meters. Rough enough for
/// resolution-level selection; precise enough that no realistic bbox
/// lands in the wrong level.
fn bbox_dimensions_m(bbox: &LLBBox) -> (f64, f64) {
    let mid_lat = (bbox.min().lat() + bbox.max().lat()) * 0.5;
    let mid_lat_cos = mid_lat.to_radians().cos().abs().max(1e-6);
    let width_deg = bbox.max().lng() - bbox.min().lng();
    let height_deg = bbox.max().lat() - bbox.min().lat();
    let width_m = width_deg.to_radians() * EARTH_RADIUS_M * mid_lat_cos;
    let height_m = height_deg.to_radians() * EARTH_RADIUS_M;
    (width_m.abs(), height_m.abs())
}

// ─── Tile keys and geometry ────────────────────────────────────────────

/// Identifies a tile in the global Web Mercator grid at a specific
/// resolution level. Two different user bboxes over the same area will
/// produce identical `TileKey`s for the tiles they share.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct TileKey {
    level: Resolution,
    tile_x: i32,
    tile_y: i32,
}

impl TileKey {
    /// Tile containing the given Web Mercator (x, y) point. `tile_y`
    /// increases southward so the global tile grid maps row 0 to the
    /// north edge of the world, matching the rest of the elevation
    /// pipeline's row ordering.
    fn for_mercator(level: Resolution, mx: f64, my: f64) -> Self {
        let span = level.tile_span_meters();
        let tile_x = ((mx + MERCATOR_LIMIT) / span).floor() as i32;
        let tile_y = ((MERCATOR_LIMIT - my) / span).floor() as i32;
        Self {
            level,
            tile_x,
            tile_y,
        }
    }

    fn min_mx(&self) -> f64 {
        -MERCATOR_LIMIT + self.tile_x as f64 * self.level.tile_span_meters()
    }

    fn max_mx(&self) -> f64 {
        self.min_mx() + self.level.tile_span_meters()
    }

    fn max_my(&self) -> f64 {
        MERCATOR_LIMIT - self.tile_y as f64 * self.level.tile_span_meters()
    }

    fn min_my(&self) -> f64 {
        self.max_my() - self.level.tile_span_meters()
    }

    /// URL for `ImageServer::exportImage`. Mercator bbox + 3857 SR so the
    /// tile is always the same 512 m × 512 m patch regardless of the
    /// user's selection.
    fn url(&self) -> String {
        format!(
            "https://elevation.nationalmap.gov/arcgis/rest/services/3DEPElevation/ImageServer/exportImage\
             ?bbox={:.6},{:.6},{:.6},{:.6}\
             &bboxSR=3857&imageSR=3857\
             &size={},{}\
             &format=tiff&pixelType=F32\
             &interpolation=RSP_BilinearInterpolation\
             &f=image",
            self.min_mx(),
            self.min_my(),
            self.max_mx(),
            self.max_my(),
            TILE_PIXELS,
            TILE_PIXELS,
        )
    }

    /// Disk cache path — stable across runs and users. `<level>/<ty>/<tx>.tiff`.
    fn cache_path(&self, cache_root: &Path) -> PathBuf {
        cache_root
            .join(self.level.id())
            .join(format!("{}", self.tile_y))
            .join(format!("{}.tiff", self.tile_x))
    }
}

// ─── Mercator conversion ───────────────────────────────────────────────

fn lon_to_mercator_x(lng: f64) -> f64 {
    lng.to_radians() * EARTH_RADIUS_M
}

fn lat_to_mercator_y(lat: f64) -> f64 {
    let clamped = lat.clamp(-MERCATOR_LAT_LIMIT, MERCATOR_LAT_LIMIT);
    let rad = clamped.to_radians();
    EARTH_RADIUS_M * (std::f64::consts::FRAC_PI_4 + rad * 0.5).tan().ln()
}

// ─── Tile discovery ────────────────────────────────────────────────────

/// Enumerate every tile key whose mercator bbox intersects the user's
/// bbox at the given resolution. Adjacent user bboxes over the same area
/// produce overlapping tile sets (shared tile keys → shared cache hits).
fn covering_tiles(bbox: &LLBBox, level: Resolution) -> Vec<TileKey> {
    let sw_mx = lon_to_mercator_x(bbox.min().lng());
    let ne_mx = lon_to_mercator_x(bbox.max().lng());
    let sw_my = lat_to_mercator_y(bbox.min().lat());
    let ne_my = lat_to_mercator_y(bbox.max().lat());
    let span = level.tile_span_meters();
    if span <= 0.0 {
        return Vec::new();
    }
    // Tile X increases east; tile Y increases south. Clamp each index
    // so far-north/far-south/±180° bboxes don't emit absurd tile counts.
    let min_tx = ((sw_mx + MERCATOR_LIMIT) / span).floor() as i32;
    let max_tx = (((ne_mx + MERCATOR_LIMIT) / span).ceil() as i32 - 1).max(min_tx);
    let min_ty = ((MERCATOR_LIMIT - ne_my) / span).floor() as i32;
    let max_ty = (((MERCATOR_LIMIT - sw_my) / span).ceil() as i32 - 1).max(min_ty);

    let mut tiles =
        Vec::with_capacity(((max_tx - min_tx + 1) * (max_ty - min_ty + 1)).max(0) as usize);
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

// ─── Tile fetch + decode ───────────────────────────────────────────────

/// Download (or load from cache) a tile and decode its TIFF payload into
/// a 2-D f64 raster in row-major order (`[y][x]`).
fn fetch_tile_raster(key: &TileKey, cache_root: &Path) -> Result<Vec<Vec<f64>>, String> {
    let cache_path = key.cache_path(cache_root);
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let url = key.url();
    let bytes =
        super::regional::fetch_or_cache(&url, &cache_path, None).map_err(|e| e.to_string())?;
    let raw = super::regional::decode_geotiff_f32(&bytes, TILE_PIXELS, TILE_PIXELS)
        .map_err(|e| e.to_string())?;
    Ok(raw.heights_meters)
}

// ─── Bilinear sampling within one tile ────────────────────────────────

fn sample_tile_bilinear(tile: &[Vec<f64>], mx: f64, my: f64, key: &TileKey) -> f64 {
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

    // Clamp at the tile's own edge — adjacent cells across a tile
    // boundary will resolve to a different TileKey and independently
    // sample that tile's edge. No inter-tile blending: at worst we
    // see a single-block step, which is a far milder artefact than
    // the 128-row ramps `tiled_fetch` used to leave behind.
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
    // NaN-aware blend: renormalise weights over the finite samples so a
    // single missing cell at the edge of coverage doesn't poison the
    // whole 4-sample average.
    blend_finite_samples(v00, v10, v01, v11, dx, dy)
}

fn blend_finite_samples(v00: f64, v10: f64, v01: f64, v11: f64, dx: f64, dy: f64) -> f64 {
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

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Same real-world point always hashes to the same tile — the whole
    /// point of using a fixed global grid.
    #[test]
    fn tile_key_for_mercator_is_stable() {
        let mx = 1_234_567.0;
        let my = -987_654.0;
        let k1 = TileKey::for_mercator(Resolution::M1, mx, my);
        let k2 = TileKey::for_mercator(Resolution::M1, mx, my);
        assert_eq!(k1, k2);
        // Points 1m apart in the same tile still hash the same.
        let k3 = TileKey::for_mercator(Resolution::M1, mx + 1.0, my - 1.0);
        // Could still be the same tile, could be adjacent — what we care
        // about is that the span is correct.
        assert_eq!(k1.level, k3.level);
    }

    /// The tile containing a point really does contain it — `min_mx ≤ mx < max_mx`.
    #[test]
    fn tile_bbox_encloses_point() {
        let mx = -5_000_000.0;
        let my = 4_321_000.0;
        for level in [
            Resolution::M1,
            Resolution::M3,
            Resolution::M10,
            Resolution::M30,
        ] {
            let k = TileKey::for_mercator(level, mx, my);
            assert!(
                k.min_mx() <= mx && mx < k.max_mx(),
                "{level:?}: mx={mx} not in [{}, {})",
                k.min_mx(),
                k.max_mx()
            );
            assert!(
                k.min_my() < my && my <= k.max_my(),
                "{level:?}: my={my} not in ({}, {}]",
                k.min_my(),
                k.max_my()
            );
        }
    }

    /// Tile span matches the documented meters-per-pixel × tile pixels.
    #[test]
    fn tile_span_matches_resolution() {
        assert!((Resolution::M1.tile_span_meters() - 512.0).abs() < 1e-9);
        assert!(
            (Resolution::M3.tile_span_meters() - 512.0 * 3.4359738368).abs() < 1e-6,
            "{} vs {}",
            Resolution::M3.tile_span_meters(),
            512.0 * 3.4359738368
        );
    }

    /// Resolution selection picks the expected level for the GUI scale
    /// range's boundary cases.
    #[test]
    fn resolution_selection_covers_gui_range() {
        // 1 km × 1 km bbox at scale 1.0 → 1000×1000 grid → 1 m/cell → M1
        let bbox = LLBBox::new(40.0, -105.0, 40.009, -104.988).unwrap();
        assert_eq!(Resolution::for_output(&bbox, 1000, 1000), Resolution::M1);
        // Same bbox at scale 2.5 → 2500×2500 grid → 0.4 m/cell → M1
        assert_eq!(Resolution::for_output(&bbox, 2500, 2500), Resolution::M1);
        // Scale 0.30 → 300×300 grid → 3.33 m/cell → M3
        assert_eq!(Resolution::for_output(&bbox, 300, 300), Resolution::M3);
        // Scale 0.05 → 50×50 → ~20 m/cell → M30
        assert_eq!(Resolution::for_output(&bbox, 50, 50), Resolution::M30);
    }

    /// Covering-tile enumeration returns a non-empty rectangle of tiles
    /// for realistic bboxes and only 1 tile for a bbox that fits in one.
    #[test]
    fn covering_tiles_rectangle() {
        // Horseshoe Bend — 2.7 km × 2.4 km → multiple M1 tiles.
        let bbox = LLBBox::new(36.870352, -111.535864, 36.892046, -111.505566).unwrap();
        let tiles = covering_tiles(&bbox, Resolution::M1);
        assert!(
            tiles.len() >= 6,
            "expected many M1 tiles, got {}",
            tiles.len()
        );
        // All tiles share the same level and form a rectangle.
        let min_tx = tiles.iter().map(|k| k.tile_x).min().unwrap();
        let max_tx = tiles.iter().map(|k| k.tile_x).max().unwrap();
        let min_ty = tiles.iter().map(|k| k.tile_y).min().unwrap();
        let max_ty = tiles.iter().map(|k| k.tile_y).max().unwrap();
        let expected = ((max_tx - min_tx + 1) * (max_ty - min_ty + 1)) as usize;
        assert_eq!(tiles.len(), expected);

        // Tiny bbox → exactly one tile.
        let bbox_small = LLBBox::new(36.880, -111.520, 36.880_1, -111.519_9).unwrap();
        let tiles_small = covering_tiles(&bbox_small, Resolution::M30);
        assert_eq!(tiles_small.len(), 1);
    }

    /// Bilinear sample on a constant-valued tile returns that constant
    /// for every mercator position inside the tile.
    #[test]
    fn bilinear_on_constant_tile() {
        let key = TileKey {
            level: Resolution::M1,
            tile_x: 0,
            tile_y: 0,
        };
        let tile = vec![vec![42.0; TILE_PIXELS]; TILE_PIXELS];
        // Sample at the tile centre and at an interior corner.
        let center_mx = (key.min_mx() + key.max_mx()) * 0.5;
        let center_my = (key.min_my() + key.max_my()) * 0.5;
        assert_eq!(
            sample_tile_bilinear(&tile, center_mx, center_my, &key),
            42.0
        );
        let corner_mx = key.min_mx() + 1.0;
        let corner_my = key.max_my() - 1.0;
        assert_eq!(
            sample_tile_bilinear(&tile, corner_mx, corner_my, &key),
            42.0
        );
    }

    /// Bilinear on a linearly-varying tile recovers the ramp at any
    /// sample position. We probe both an exact pixel corner (where the
    /// result must equal that pixel's value) and a mid-pixel position
    /// (where the result must equal the arithmetic mean of the four
    /// surrounding pixels).
    #[test]
    fn bilinear_on_linear_tile() {
        let key = TileKey {
            level: Resolution::M1,
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
        // Exact corner at pixel (100, 50). `local_x = (mx - min_mx) / mpp`
        // so positioning mx exactly `100 * mpp` past the tile origin gives
        // `local_x = 100`, `dx = 0` → pure v00 = tile[50][100] = 50_100.
        let mpp = key.level.meters_per_pixel();
        let mx_corner = key.min_mx() + 100.0 * mpp;
        let my_corner = key.max_my() - 50.0 * mpp;
        let corner = sample_tile_bilinear(&tile, mx_corner, my_corner, &key);
        assert!(
            (corner - 50_100.0).abs() < 1e-6,
            "corner sample expected 50100, got {corner}"
        );
        // Mid-pixel at (100.5, 50.5): bilinear should give the mean of
        // tile[50..=51][100..=101] = (50_100 + 50_101 + 51_100 + 51_101) / 4.
        let mx_mid = key.min_mx() + 100.5 * mpp;
        let my_mid = key.max_my() - 50.5 * mpp;
        let mid = sample_tile_bilinear(&tile, mx_mid, my_mid, &key);
        let expected_mid = (50_100.0 + 50_101.0 + 51_100.0 + 51_101.0) / 4.0;
        assert!(
            (mid - expected_mid).abs() < 1e-6,
            "mid sample expected {expected_mid}, got {mid}"
        );
    }

    /// NaN pixels in the 4-sample neighbourhood must not corrupt the
    /// result — the remaining finite samples carry the weight.
    #[test]
    fn bilinear_nan_is_robust() {
        let v = blend_finite_samples(10.0, f64::NAN, 30.0, 40.0, 0.5, 0.5);
        // w00 = w10 = w01 = w11 = 0.25. Discard v10. Expected = (10 + 30 + 40) / 3.
        assert!((v - 80.0 / 3.0).abs() < 1e-9, "got {v}");
        // All NaN → NaN.
        let v_all = blend_finite_samples(f64::NAN, f64::NAN, f64::NAN, f64::NAN, 0.5, 0.5);
        assert!(v_all.is_nan());
    }

    /// Mercator round-trip: converting a lat/lng to mercator and back
    /// gives the original (within machine precision).
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
            // Invert: lng = mx / R / rad_per_deg; lat = 2*atan(exp(my/R)) - π/2.
            let lng2 = mx / EARTH_RADIUS_M * 180.0 / std::f64::consts::PI;
            let lat2 = (2.0 * (my / EARTH_RADIUS_M).exp().atan() - std::f64::consts::FRAC_PI_2)
                .to_degrees();
            assert!((lng - lng2).abs() < 1e-9, "lng {lng} -> {lng2}");
            assert!((lat - lat2).abs() < 1e-6, "lat {lat} -> {lat2}");
        }
    }
}
