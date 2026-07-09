//! Shared HTTP / TIFF helpers for the elevation providers.
//!
//! Historically this file hosted every non-AWS provider (USGS, IGN FR,
//! IGN ES, Japan GSI). Those either moved to their own modules backed
//! by [`super::fixed_tile`] (USGS 3DEP) or were superseded by the
//! global Mapterhorn provider, which ingests the same upstream datasets
//! (state DGM1s, RGE ALTI, MDT02/05, GSI DEM) at equal or better
//! resolution.
//!
//! What lives here now:
//!
//! - [`fetch_or_cache`] and [`decode_geotiff_f32`] — the shared
//!   HTTP-with-disk-cache + GeoTIFF decode used by `fixed_tile`'s
//!   tile downloader.
//! - [`is_valid_payload`] and `resample_nearest` — internals used by
//!   the two above.

use crate::elevation::provider::RawElevationGrid;

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
            // 180s: tiled single-request providers (USGS 3DEP ArcGIS)
            // generate GeoTIFFs server-side; 120s was occasionally tight
            // under load.
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
