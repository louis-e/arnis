//! Shared HTTP / TIFF helpers + the Japan GSI provider.
//!
//! Historically this file hosted every non-AWS provider (USGS, IGN FR,
//! IGN ES, Japan GSI) and the `tiled_fetch` ad-hoc bbox-splitter they
//! shared. USGS, IGN France and IGN Spain all moved to their own
//! modules backed by [`super::fixed_tile`] because `tiled_fetch`'s
//! user-bbox-relative sub-tile boundaries exposed inter-flight
//! elevation offsets as visible seams in the rendered world.
//!
//! What lives here now:
//!
//! - [`JapanGsi`] — already uses fixed Web-Mercator XYZ tiles at a
//!   preset zoom level, so it doesn't have the seam issue. Left in
//!   place because its pixel-decoding path is GSI-specific.
//! - [`fetch_or_cache`] and [`decode_geotiff_f32`] — the shared
//!   HTTP-with-disk-cache + GeoTIFF decode used by `fixed_tile`'s
//!   tile downloader and by Japan GSI's own fetch loop.
//! - [`is_valid_payload`] and `resample_nearest` — internals used by
//!   the two above.

use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::cache::get_cache_dir;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};

/// Japan GSI Elevation Tiles — Japan.
/// Resolution: 5m (DEM5A/B/C), 10m fallback.
/// License: GSI Terms of Use (attribution required).
pub struct JapanGsi;

impl ElevationProvider for JapanGsi {
    fn name(&self) -> &'static str {
        "japan_gsi"
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        Some(vec![
            // Japan
            LLBBox::new(24.0, 122.0, 46.0, 154.0).unwrap(),
        ])
    }

    fn native_resolution_m(&self) -> f64 {
        5.0
    }

    fn fetch_raw(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
        // Use DEM5A tiles (5m resolution, best available)
        // These are XYZ PNG tiles with encoded elevation values
        let zoom: u8 = 15; // Fixed zoom for highest DEM5A resolution

        let tiles = get_xyz_tile_coordinates(bbox, zoom);
        let mut height_grid: Vec<Vec<f64>> = vec![vec![f64::NAN; grid_width]; grid_height];

        let cache_dir = get_cache_dir(self.name());
        std::fs::create_dir_all(&cache_dir)?;

        // Try DEM5A first, fall back through DEM5B, DEM5C, DEM10B
        let dem_layers = ["dem5a_png", "dem5b_png", "dem5c_png", "dem_png"];

        println!("Fetching {} elevation tiles from GSI Japan...", tiles.len());

        // Build a shared HTTP client for all tile downloads
        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!(
                "Arnis/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/louis-e/arnis)"
            ))
            .timeout(std::time::Duration::from_secs(120))
            .build()?;

        // Download all tiles into a HashMap
        let mut tile_map: std::collections::HashMap<(u32, u32), image::RgbaImage> =
            std::collections::HashMap::new();

        for (tile_x, tile_y) in &tiles {
            for layer in &dem_layers {
                let url = format!(
                    "https://cyberjapandata.gsi.go.jp/xyz/{}/{}/{}/{}.png",
                    layer, zoom, tile_x, tile_y
                );
                let cache_path = cache_dir.join(format!("{layer}_z{zoom}_x{tile_x}_y{tile_y}.png"));

                match fetch_or_cache(&url, &cache_path, Some(&client)) {
                    Ok(bytes) => {
                        if let Ok(img) = image::load_from_memory(&bytes) {
                            tile_map.insert((*tile_x, *tile_y), img.to_rgba8());
                            break;
                        }
                    }
                    Err(_) => continue,
                }
            }
        }

        let n = 2.0_f64.powi(zoom as i32);

        // Grid-iteration with bilinear sampling
        #[allow(clippy::needless_range_loop)]
        for gy in 0..grid_height {
            for gx in 0..grid_width {
                let lat = bbox.max().lat()
                    - (gy as f64 / (grid_height - 1).max(1) as f64)
                        * (bbox.max().lat() - bbox.min().lat());
                let lng = bbox.min().lng()
                    + (gx as f64 / (grid_width - 1).max(1) as f64)
                        * (bbox.max().lng() - bbox.min().lng());

                let lat_rad = lat.to_radians();
                let fx_global = (lng + 180.0) / 360.0 * n * 256.0;
                let fy_global =
                    (1.0 - lat_rad.tan().asinh() / std::f64::consts::PI) / 2.0 * n * 256.0;

                // Clamp via i64 so ±180° lng / ±90° lat can't wrap a bare
                // `as u32` cast; see aws_terrain.rs for the full rationale.
                let n_tiles = n as i64;
                let tile_x = ((fx_global / 256.0).floor() as i64).clamp(0, n_tiles - 1) as u32;
                let tile_y = ((fy_global / 256.0).floor() as i64).clamp(0, n_tiles - 1) as u32;
                let px = fx_global - tile_x as f64 * 256.0;
                let py = fy_global - tile_y as f64 * 256.0;

                let x0 = px.floor() as i32;
                let y0 = py.floor() as i32;
                let dx = px - x0 as f64;
                let dy = py - y0 as f64;

                let v00 = sample_gsi_pixel(&tile_map, tile_x, tile_y, x0, y0);
                let v10 = sample_gsi_pixel(&tile_map, tile_x, tile_y, x0 + 1, y0);
                let v01 = sample_gsi_pixel(&tile_map, tile_x, tile_y, x0, y0 + 1);
                let v11 = sample_gsi_pixel(&tile_map, tile_x, tile_y, x0 + 1, y0 + 1);

                if let (Some(v00), Some(v10), Some(v01), Some(v11)) = (v00, v10, v01, v11) {
                    let lerp_top = v00 + (v10 - v00) * dx;
                    let lerp_bot = v01 + (v11 - v01) * dx;
                    height_grid[gy][gx] = lerp_top + (lerp_bot - lerp_top) * dy;
                }
            }
        }

        Ok(RawElevationGrid {
            heights_meters: height_grid,
        })
    }
}

/// Sample a single pixel from GSI tile map, handling tile boundary crossover.
/// GSI PNG encoding: h = (R*65536 + G*256 + B) * 0.01, nodata = RGB(128,0,0).
fn sample_gsi_pixel(
    tile_map: &std::collections::HashMap<(u32, u32), image::RgbaImage>,
    base_tile_x: u32,
    base_tile_y: u32,
    px: i32,
    py: i32,
) -> Option<f64> {
    let (tx, x) = if px < 0 {
        (base_tile_x.wrapping_sub(1), (px + 256) as u32)
    } else if px >= 256 {
        (base_tile_x + 1, (px - 256) as u32)
    } else {
        (base_tile_x, px as u32)
    };
    let (ty, y) = if py < 0 {
        (base_tile_y.wrapping_sub(1), (py + 256) as u32)
    } else if py >= 256 {
        (base_tile_y + 1, (py - 256) as u32)
    } else {
        (base_tile_y, py as u32)
    };

    let tile = tile_map.get(&(tx, ty))?;
    if x >= tile.width() || y >= tile.height() {
        return None;
    }
    let pixel = tile.get_pixel(x, y);
    let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
    if r == 128 && g == 0 && b == 0 {
        return None; // nodata
    }
    let raw = r as f64 * 65536.0 + g as f64 * 256.0 + b as f64;
    let height = if raw < 8388608.0 {
        raw * 0.01
    } else {
        (raw - 16777216.0) * 0.01
    };
    Some(height)
}

/// Maximum total attempts per request, including the initial one.
///
/// 3 attempts + exponential backoff gives roughly 0.75 + 1.5 s ≈ 2.25 s
/// of wait between the first and last attempt (plus ±50% jitter). Any
/// tile still failing after that is handed off to the AWS Terrarium
/// fallback in `fetch_fixed_tile_grid`, which is cheap enough that we
/// prefer getting there quickly over retrying the primary provider
/// for longer.
const MAX_RETRIES: u32 = 3;
/// Base delay for exponential backoff between retries (ms).
const RETRY_BASE_DELAY_MS: u64 = 750;

/// Fetch data from URL or load from cache.
///
/// Retries on 5xx responses and network errors with exponential backoff.
/// 4xx responses are returned immediately (request is malformed).
/// If `client` is provided, reuse it; otherwise build a new one.
pub(super) fn fetch_or_cache(
    url: &str,
    cache_path: &std::path::Path,
    client: Option<&reqwest::blocking::Client>,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if cache_path.exists() {
        let bytes = std::fs::read(cache_path)?;
        if bytes.len() > 100 && is_valid_payload(&bytes) {
            return Ok(bytes);
        }
        // Invalid or too small, re-download
        let _ = std::fs::remove_file(cache_path);
    }

    let owned_client;
    let client = match client {
        Some(c) => c,
        None => {
            // 180s: tiled single-request providers (USGS 3DEP, IGN WMS/WCS)
            // generate GeoTIFFs server-side; 120s was occasionally tight
            // under load even at cap 4096. Japan GSI keeps its own 120s
            // client since it fetches 256 px PNGs.
            owned_client = reqwest::blocking::Client::builder()
                .user_agent(concat!(
                    "Arnis/",
                    env!("CARGO_PKG_VERSION"),
                    " (+https://github.com/louis-e/arnis)"
                ))
                .timeout(std::time::Duration::from_secs(180))
                .build()?;
            &owned_client
        }
    };

    let mut last_error: Option<Box<dyn std::error::Error>> = None;

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            // Exponential backoff with ±50% jitter. Without jitter, all
            // concurrent tile fetches that failed together retry on the
            // same millisecond and re-synchronise pressure on the same
            // flaky endpoint (thundering herd). The jitter multiplier
            // picks from [0.5, 1.5), so two concurrent retries end up
            // offset by up to one base-delay.
            use rand::Rng;
            let base = RETRY_BASE_DELAY_MS * (1 << (attempt - 1));
            let jitter = rand::rng().random_range(0.5_f64..1.5_f64);
            let delay_ms = (base as f64 * jitter).round() as u64;
            eprintln!(
                "Elevation request retry {}/{} after {}ms...",
                attempt,
                MAX_RETRIES - 1,
                delay_ms
            );
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        }

        match client.get(url).send() {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let bytes = response.bytes()?.to_vec();
                    if bytes.len() <= 100 || !is_valid_payload(&bytes) {
                        // Server returned 200 but with an error page / empty body -
                        // treat as transient and retry.
                        last_error =
                            Some(format!("Invalid payload ({}B) from {url}", bytes.len()).into());
                        continue;
                    }
                    if let Some(parent) = cache_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(cache_path, &bytes)?;
                    return Ok(bytes);
                }
                if status.is_server_error() {
                    last_error = Some(format!("HTTP {status} from elevation service").into());
                    continue;
                }
                // 4xx: client error, no point retrying.
                return Err(format!("HTTP {status} from elevation service").into());
            }
            Err(e) => {
                last_error = Some(Box::new(e));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "Elevation request failed".into()))
}

/// Check if a payload looks like a valid image (TIFF or PNG), not an HTML error page.
fn is_valid_payload(bytes: &[u8]) -> bool {
    if bytes.len() < 4 {
        return false;
    }
    // TIFF: little-endian "II\x2A\x00" or big-endian "MM\x00\x2A"
    let is_tiff = (bytes[0] == b'I' && bytes[1] == b'I' && bytes[2] == 0x2A && bytes[3] == 0x00)
        || (bytes[0] == b'M' && bytes[1] == b'M' && bytes[2] == 0x00 && bytes[3] == 0x2A);
    // PNG: "\x89PNG"
    let is_png = bytes[0] == 0x89 && bytes[1] == b'P' && bytes[2] == b'N' && bytes[3] == b'G';
    is_tiff || is_png
}

/// Decode a GeoTIFF containing float32 elevation values.
/// Attempts to read the raster data and resample to requested grid dimensions.
pub(super) fn decode_geotiff_f32(
    bytes: &[u8],
    target_width: usize,
    target_height: usize,
) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
    use std::io::Cursor;
    use tiff::decoder::Decoder;

    let cursor = Cursor::new(bytes);
    let mut decoder = Decoder::new(cursor)?;

    let (src_width, _src_height) = decoder.dimensions()?;
    let src_width = src_width as usize;

    // Read the raster data. The decoder hands us its own typed buffer;
    // we resample directly from it in `resample_nearest` below — casting
    // on the fly per sample — instead of first materialising a whole
    // Vec<f64> copy of the raster. On a 4096² F32 raster that saves
    // ~128 MB of peak memory.
    let result = decoder.read_image()?;

    let height_grid = match result {
        tiff::decoder::DecodingResult::F32(data) => {
            resample_nearest(&data, src_width, target_width, target_height, |v| v as f64)
        }
        tiff::decoder::DecodingResult::F64(data) => {
            resample_nearest(&data, src_width, target_width, target_height, |v| v)
        }
        tiff::decoder::DecodingResult::U8(data) => {
            resample_nearest(&data, src_width, target_width, target_height, |v| v as f64)
        }
        tiff::decoder::DecodingResult::U16(data) => {
            resample_nearest(&data, src_width, target_width, target_height, |v| v as f64)
        }
        tiff::decoder::DecodingResult::I16(data) => {
            resample_nearest(&data, src_width, target_width, target_height, |v| v as f64)
        }
        tiff::decoder::DecodingResult::U32(data) => {
            resample_nearest(&data, src_width, target_width, target_height, |v| v as f64)
        }
        tiff::decoder::DecodingResult::I32(data) => {
            resample_nearest(&data, src_width, target_width, target_height, |v| v as f64)
        }
        _ => return Err("Unsupported TIFF pixel type".into()),
    };

    Ok(RawElevationGrid {
        heights_meters: height_grid,
    })
}

/// Nearest-neighbour resample from a typed source slice into an
/// `Vec<Vec<f64>>` target grid, casting each sampled pixel on the fly.
///
/// Keeping the source in its native type (F32/U16/...) and casting only
/// the pixels we actually sample avoids an intermediate full-raster
/// `Vec<f64>` copy. For a 4096² F32 raster that's ~128 MB of peak
/// memory saved — the difference between \"fits in 512 MB\" and
/// \"OOM on memory-constrained systems\".
///
/// `cast` is monomorphised per call site, so there's no runtime dispatch
/// overhead versus the old match-then-collect version.
fn resample_nearest<T: Copy>(
    src: &[T],
    src_width: usize,
    target_width: usize,
    target_height: usize,
    cast: impl Fn(T) -> f64,
) -> Vec<Vec<f64>> {
    let mut height_grid: Vec<Vec<f64>> = vec![vec![f64::NAN; target_width]; target_height];
    let src_height = src.len().checked_div(src_width).unwrap_or(0);
    let target_y_den = target_height.saturating_sub(1).max(1);
    let target_x_den = target_width.saturating_sub(1).max(1);
    let src_y_extent = src_height.saturating_sub(1);
    let src_x_extent = src_width.saturating_sub(1);

    for (ty, row) in height_grid.iter_mut().enumerate().take(target_height) {
        let sy = (ty as f64 / target_y_den as f64 * src_y_extent as f64) as usize;
        let sy = sy.min(src_y_extent);
        for (tx, slot) in row.iter_mut().enumerate().take(target_width) {
            let sx = (tx as f64 / target_x_den as f64 * src_x_extent as f64) as usize;
            let sx = sx.min(src_x_extent);
            let idx = sy * src_width + sx;
            if let Some(&raw) = src.get(idx) {
                let val = cast(raw);
                // Common nodata values filtered here — keep only finite,
                // in-range elevations.
                if val > -9999.0 && val < 100000.0 && val.is_finite() {
                    *slot = val;
                }
            }
        }
    }

    height_grid
}

/// Get XYZ tile coordinates covering a bbox at the given zoom level.
fn get_xyz_tile_coordinates(bbox: &LLBBox, zoom: u8) -> Vec<(u32, u32)> {
    let n = 2.0_f64.powi(zoom as i32);

    // Clamp via i64 so ±90° lat / +180° lng can't wrap the u32 cast —
    // same rationale as in `aws_terrain::lat_lng_to_tile`.
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

    let mut tiles = Vec::new();
    for x in x1.min(x2)..=x1.max(x2) {
        for y in y1.min(y2)..=y1.max(y2) {
            tiles.push((x, y));
        }
    }
    tiles
}
