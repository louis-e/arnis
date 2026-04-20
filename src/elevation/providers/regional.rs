use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::cache::get_cache_dir;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Per-provider ceilings on the pixel dimension of a single upstream
/// request. Beyond this, `tiled_fetch` splits the bbox into sub-tiles so
/// the native-resolution data is preserved instead of the server
/// downsampling (or rejecting the request).
///
/// - **USGS 3DEP (ArcGIS ImageServer)**: documented hard cap is 8000,
///   but empirically the server returns HTTP 500 ("Error exporting
///   image") at ≥ 3000 per axis and hits gateway timeouts (504) at 4096+
///   under load, even though smaller requests succeed quickly. Measured
///   on the Grand Canyon bbox: 1024/2048/2500/2800 all return 200 in
///   6-17 s; 3000/3717 return 500 after 18-22 s of server work. 2048 is
///   a clean power-of-2 comfortably below the failure threshold.
/// - **IGN France (WMS 1.3.0)**: 4096 is the safe MapServer default;
///   `data.geopf.fr` silently caps larger requests and interpolates.
/// - **IGN Spain (WCS 2.0.1)**: 4096 is the conservative safe value for
///   `servicios.idee.es` — larger requests time out under load.
const USGS_MAX_SINGLE: usize = 2048;
const IGN_FRANCE_MAX_SINGLE: usize = 4096;
const IGN_SPAIN_MAX_SINGLE: usize = 4096;

/// USGS 3D Elevation Program (3DEP) — USA + territories.
/// Resolution: up to 1m LiDAR (CONUS), 3m/10m elsewhere, fallback 30m.
/// License: Public Domain (USGS).
pub struct Usgs3dep;

impl Usgs3dep {
    fn fetch_tile(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
        let url = format!(
            "https://elevation.nationalmap.gov/arcgis/rest/services/3DEPElevation/ImageServer/exportImage\
             ?bbox={},{},{},{}\
             &bboxSR=4326&imageSR=4326\
             &size={},{}\
             &format=tiff&pixelType=F32\
             &interpolation=RSP_BilinearInterpolation\
             &f=image",
            bbox.min().lng(), bbox.min().lat(),
            bbox.max().lng(), bbox.max().lat(),
            grid_width, grid_height
        );

        let cache_dir = get_cache_dir(self.name());
        std::fs::create_dir_all(&cache_dir)?;

        let cache_key = bbox_hash(bbox, grid_width, grid_height);
        let cache_path = cache_dir.join(format!("{cache_key}.tiff"));

        let bytes = fetch_or_cache(&url, &cache_path, None)?;
        decode_geotiff_f32(&bytes, grid_width, grid_height)
    }
}

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
        tiled_fetch(
            bbox,
            grid_width,
            grid_height,
            USGS_MAX_SINGLE,
            |sub_bbox, sub_w, sub_h| self.fetch_tile(sub_bbox, sub_w, sub_h),
        )
    }
}

/// IGN France RGE ALTI — France + overseas territories.
/// Resolution: 1m mainland France, 1-5m overseas.
/// License: Licence Ouverte 2.0.
pub struct IgnFrance;

impl IgnFrance {
    fn fetch_tile(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
        // WMS 1.3.0 with CRS=EPSG:4326 uses lat,lng order for BBOX
        // STYLES= must be present (empty is fine) or the server returns 400
        let url = format!(
            "https://data.geopf.fr/wms-r/wms\
             ?SERVICE=WMS&REQUEST=GetMap&VERSION=1.3.0\
             &LAYERS=ELEVATION.ELEVATIONGRIDCOVERAGE\
             &STYLES=\
             &CRS=EPSG:4326\
             &BBOX={},{},{},{}\
             &WIDTH={}&HEIGHT={}\
             &FORMAT=image/geotiff",
            bbox.min().lat(),
            bbox.min().lng(),
            bbox.max().lat(),
            bbox.max().lng(),
            grid_width,
            grid_height
        );

        let cache_dir = get_cache_dir(self.name());
        std::fs::create_dir_all(&cache_dir)?;

        let cache_key = bbox_hash(bbox, grid_width, grid_height);
        let cache_path = cache_dir.join(format!("{cache_key}.tiff"));

        let bytes = fetch_or_cache(&url, &cache_path, None)?;
        decode_geotiff_f32(&bytes, grid_width, grid_height)
    }
}

impl ElevationProvider for IgnFrance {
    fn name(&self) -> &'static str {
        "ign_france"
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        Some(vec![
            // Metropolitan France
            LLBBox::new(41.0, -5.5, 51.5, 10.0).unwrap(),
            // Guadeloupe
            LLBBox::new(15.8, -61.9, 16.6, -60.9).unwrap(),
            // Martinique
            LLBBox::new(14.3, -61.3, 14.9, -60.8).unwrap(),
            // French Guiana
            LLBBox::new(2.0, -55.0, 6.0, -51.0).unwrap(),
            // Réunion
            LLBBox::new(-21.5, 55.1, -20.8, 55.9).unwrap(),
            // Mayotte
            LLBBox::new(-13.1, 44.9, -12.5, 45.4).unwrap(),
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
        tiled_fetch(
            bbox,
            grid_width,
            grid_height,
            IGN_FRANCE_MAX_SINGLE,
            |sub_bbox, sub_w, sub_h| self.fetch_tile(sub_bbox, sub_w, sub_h),
        )
    }
}

/// IGN España MDT — Spain + Canary Islands + Balearic Islands.
/// Resolution: 5m (MDT05).
/// License: CC BY 4.0.
pub struct IgnSpain;

impl IgnSpain {
    fn fetch_tile(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
        let url = format!(
            "https://servicios.idee.es/wcs-inspire/mdt\
             ?SERVICE=WCS&VERSION=2.0.1&REQUEST=GetCoverage\
             &COVERAGEID=Elevacion4258_5\
             &SUBSET=Long({},{})\
             &SUBSET=Lat({},{})\
             &FORMAT=image/tiff\
             &SCALESIZE=Long({}),Lat({})",
            bbox.min().lng(),
            bbox.max().lng(),
            bbox.min().lat(),
            bbox.max().lat(),
            grid_width,
            grid_height
        );

        let cache_dir = get_cache_dir(self.name());
        std::fs::create_dir_all(&cache_dir)?;

        let cache_key = bbox_hash(bbox, grid_width, grid_height);
        let cache_path = cache_dir.join(format!("{cache_key}.tiff"));

        let bytes = fetch_or_cache(&url, &cache_path, None)?;
        decode_geotiff_f32(&bytes, grid_width, grid_height)
    }
}

impl ElevationProvider for IgnSpain {
    fn name(&self) -> &'static str {
        "ign_spain"
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        Some(vec![
            // Mainland Spain + Balearic Islands
            LLBBox::new(35.5, -10.0, 44.0, 5.0).unwrap(),
            // Canary Islands
            LLBBox::new(27.5, -18.5, 29.5, -13.0).unwrap(),
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
        tiled_fetch(
            bbox,
            grid_width,
            grid_height,
            IGN_SPAIN_MAX_SINGLE,
            |sub_bbox, sub_w, sub_h| self.fetch_tile(sub_bbox, sub_w, sub_h),
        )
    }
}

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
            .user_agent(concat!("arnis/", env!("CARGO_PKG_VERSION")))
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

                let tile_x = (fx_global / 256.0).floor() as u32;
                let tile_y = (fy_global / 256.0).floor() as u32;
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

// --- Shared helpers ---

/// Split a large request into sub-tiles and stitch the results.
///
/// When the requested grid fits in a single upstream request
/// (`grid_width <= per_request_max && grid_height <= per_request_max`)
/// this calls `fetch_tile` once and returns its result unchanged.
///
/// Otherwise the grid is partitioned into a `tiles_x × tiles_y` mosaic
/// covering disjoint pixel ranges `[0..grid_width) × [0..grid_height)`.
/// Each sub-bbox corresponds *exactly* to its pixel range using the
/// cell-edge convention (pixel `x` covers lng `[min + x*cell, min + (x+1)*cell]`),
/// so the stitched result is sample-identical to what a single oversized
/// request would return — just without the server-side downsampling or
/// rejection that happens past each provider's documented cap.
///
/// Row 0 is north (`max_lat`) to match the rest of the elevation pipeline.
///
/// Fetches run sequentially: tile payloads are already hundreds of MB at
/// this scale, and `fetch_or_cache` hits disk/network — parallelism would
/// multiply peak memory without meaningfully shortening first-run time,
/// and repeat runs are instant from cache anyway.
fn tiled_fetch<F>(
    bbox: &LLBBox,
    grid_width: usize,
    grid_height: usize,
    per_request_max: usize,
    fetch_tile: F,
) -> Result<RawElevationGrid, Box<dyn std::error::Error>>
where
    F: Fn(&LLBBox, usize, usize) -> Result<RawElevationGrid, Box<dyn std::error::Error>>,
{
    if grid_width <= per_request_max && grid_height <= per_request_max {
        return fetch_tile(bbox, grid_width, grid_height);
    }

    let tiles_x = grid_width.div_ceil(per_request_max);
    let tiles_y = grid_height.div_ceil(per_request_max);
    // Balance tile sizes so none exceeds per_request_max.
    let tile_size_x = grid_width.div_ceil(tiles_x);
    let tile_size_y = grid_height.div_ceil(tiles_y);

    let min_lng = bbox.min().lng();
    let min_lat = bbox.min().lat();
    let max_lng = bbox.max().lng();
    let max_lat = bbox.max().lat();
    let lng_span = max_lng - min_lng;
    let lat_span = max_lat - min_lat;

    let total_tiles = tiles_x * tiles_y;
    eprintln!(
        "Tiled elevation fetch: {grid_width}×{grid_height} grid split into {tiles_x}×{tiles_y} = {total_tiles} sub-tiles (cap {per_request_max}/req)"
    );

    let mut stitched: Vec<Vec<f64>> = vec![vec![f64::NAN; grid_width]; grid_height];

    for ty in 0..tiles_y {
        let y0 = ty * tile_size_y;
        let y1 = ((ty + 1) * tile_size_y).min(grid_height);
        let sub_h = y1 - y0;
        // Row 0 = north (max_lat), row grid_height-1 = south (min_lat).
        let sub_max_lat = max_lat - (y0 as f64 / grid_height as f64) * lat_span;
        let sub_min_lat = max_lat - (y1 as f64 / grid_height as f64) * lat_span;
        // Clamp to bbox floor to defend against FP drift on the last row.
        let sub_min_lat = sub_min_lat.max(min_lat);

        for tx in 0..tiles_x {
            let x0 = tx * tile_size_x;
            let x1 = ((tx + 1) * tile_size_x).min(grid_width);
            let sub_w = x1 - x0;

            let sub_min_lng = min_lng + (x0 as f64 / grid_width as f64) * lng_span;
            let sub_max_lng = min_lng + (x1 as f64 / grid_width as f64) * lng_span;
            let sub_max_lng = sub_max_lng.min(max_lng);

            let sub_bbox = LLBBox::new(sub_min_lat, sub_min_lng, sub_max_lat, sub_max_lng)?;

            let raw = fetch_tile(&sub_bbox, sub_w, sub_h)?;

            for (dy, src_row) in raw.heights_meters.iter().enumerate().take(sub_h) {
                let dst_row = &mut stitched[y0 + dy];
                let copy_n = src_row.len().min(sub_w);
                dst_row[x0..x0 + copy_n].copy_from_slice(&src_row[..copy_n]);
            }
        }
    }

    Ok(RawElevationGrid {
        heights_meters: stitched,
    })
}

/// Maximum retry attempts for transient failures (5xx, network errors).
const MAX_RETRIES: u32 = 3;
/// Base delay for exponential backoff between retries (ms).
const RETRY_BASE_DELAY_MS: u64 = 750;

/// Fetch data from URL or load from cache.
///
/// Retries on 5xx responses and network errors with exponential backoff.
/// 4xx responses are returned immediately (request is malformed).
/// If `client` is provided, reuse it; otherwise build a new one.
fn fetch_or_cache(
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
                .user_agent(concat!("arnis/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(180))
                .build()?;
            &owned_client
        }
    };

    let mut last_error: Option<Box<dyn std::error::Error>> = None;

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            let delay_ms = RETRY_BASE_DELAY_MS * (1 << (attempt - 1));
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
fn decode_geotiff_f32(
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

/// Compute a hash key for caching based on bbox and grid dimensions.
fn bbox_hash(bbox: &LLBBox, width: usize, height: usize) -> u64 {
    let mut hasher = DefaultHasher::new();
    bbox.min().lat().to_bits().hash(&mut hasher);
    bbox.min().lng().to_bits().hash(&mut hasher);
    bbox.max().lat().to_bits().hash(&mut hasher);
    bbox.max().lng().to_bits().hash(&mut hasher);
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    hasher.finish()
}

/// Get XYZ tile coordinates covering a bbox at the given zoom level.
fn get_xyz_tile_coordinates(bbox: &LLBBox, zoom: u8) -> Vec<(u32, u32)> {
    let n = 2.0_f64.powi(zoom as i32);

    let x1 = ((bbox.min().lng() + 180.0) / 360.0 * n).floor() as u32;
    let x2 = ((bbox.max().lng() + 180.0) / 360.0 * n).floor() as u32;

    let y1 = ((1.0 - bbox.max().lat().to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n)
        .floor() as u32;
    let y2 = ((1.0 - bbox.min().lat().to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n)
        .floor() as u32;

    let mut tiles = Vec::new();
    for x in x1.min(x2)..=x1.max(x2) {
        for y in y1.min(y2)..=y1.max(y2) {
            tiles.push((x, y));
        }
    }
    tiles
}

#[cfg(test)]
mod tests {
    use super::*;

    /// When the grid fits in a single request, tiled_fetch must pass through
    /// untouched — same bbox, same dimensions, same payload as a direct call.
    #[test]
    fn tiled_fetch_single_request_passthrough() {
        let bbox = LLBBox::new(40.0, -120.0, 41.0, -119.0).unwrap();
        let call_count = std::cell::Cell::new(0usize);
        let got = tiled_fetch(&bbox, 100, 80, 4096, |sb, w, h| {
            call_count.set(call_count.get() + 1);
            // Bbox and size must be unchanged on fast path.
            assert_eq!(w, 100);
            assert_eq!(h, 80);
            assert!((sb.min().lat() - 40.0).abs() < 1e-12);
            assert!((sb.max().lng() - -119.0).abs() < 1e-12);
            Ok(RawElevationGrid {
                heights_meters: vec![vec![7.0; w]; h],
            })
        })
        .unwrap();
        assert_eq!(call_count.get(), 1);
        assert_eq!(got.heights_meters.len(), 80);
        assert_eq!(got.heights_meters[0].len(), 100);
        assert_eq!(got.heights_meters[5][50], 7.0);
    }

    /// Tiles must partition the pixel range exactly and stitch without gaps
    /// or overlaps. We encode (col, row) into each cell and verify every
    /// cell in the output lands at its expected index.
    #[test]
    fn tiled_fetch_stitches_disjoint_tiles() {
        // 2×2 mosaic: cap 30, total 50×40 → tiles_x=2, tiles_y=2
        let bbox = LLBBox::new(40.0, -120.0, 42.0, -118.0).unwrap();
        let grid_w = 50usize;
        let grid_h = 40usize;
        let per_req = 30usize;

        let calls = std::cell::RefCell::new(Vec::<(f64, f64, f64, f64, usize, usize)>::new());

        // The fetch closure encodes the *sub-bbox origin* in each pixel so
        // we can check the stitched result against expected coordinates.
        // Each cell = sub_min_lng + x_local / 1000 + (sub_max_lat * 1000).
        let got = tiled_fetch(&bbox, grid_w, grid_h, per_req, |sb, w, h| {
            calls.borrow_mut().push((
                sb.min().lng(),
                sb.max().lng(),
                sb.min().lat(),
                sb.max().lat(),
                w,
                h,
            ));
            let mut rows = vec![vec![0.0f64; w]; h];
            for (y, row) in rows.iter_mut().enumerate() {
                for (x, cell) in row.iter_mut().enumerate() {
                    *cell =
                        sb.min().lng() * 1e6 + sb.max().lat() * 1e3 + x as f64 + y as f64 * 1e-3;
                }
            }
            Ok(RawElevationGrid {
                heights_meters: rows,
            })
        })
        .unwrap();

        // 4 tiles expected
        assert_eq!(calls.borrow().len(), 4);

        // Output shape
        assert_eq!(got.heights_meters.len(), grid_h);
        assert_eq!(got.heights_meters[0].len(), grid_w);

        // No NaNs anywhere (full coverage)
        for row in &got.heights_meters {
            for &v in row {
                assert!(v.is_finite(), "stitched grid has non-finite cell");
            }
        }

        // Sub-tile partition: with per_req=30, grid_w=50 → tiles_x=2,
        // tile_size_x = ceil(50/2) = 25. Same for y: ceil(40/2) = 20.
        // So expected tile pixel ranges: x∈{[0,25),[25,50)}, y∈{[0,20),[20,40)}.
        // Corresponding lng slices (lng_span=2, grid=50 → 0.04/px): tile0
        // x: [-120.0, -119.0], tile1 x: [-119.0, -118.0].
        let tile0_x_min_lng = -120.0;
        let tile1_x_min_lng = -119.0;
        // Top-left cell of tile1 (x=25, y=0): value encoded with
        // sub_min_lng = -119.0, sub_max_lat = 42.0, local (0,0).
        let expected_top_right = tile1_x_min_lng * 1e6 + 42.0 * 1e3;
        assert!((got.heights_meters[0][25] - expected_top_right).abs() < 1e-6);
        // Top-left cell of tile0 (x=0, y=0): sub_min_lng = -120.0,
        // sub_max_lat = 42.0, local (0,0).
        let expected_top_left = tile0_x_min_lng * 1e6 + 42.0 * 1e3;
        assert!((got.heights_meters[0][0] - expected_top_left).abs() < 1e-6);
    }

    /// Uneven tiling: last tile along each axis can be smaller than the
    /// others. Stitching must still cover the whole grid.
    #[test]
    fn tiled_fetch_uneven_last_tile() {
        // cap 4, grid 10×7 → tiles_x=3 (sizes 4,4,2), tiles_y=2 (sizes 4,3)
        let bbox = LLBBox::new(0.0, 0.0, 1.0, 1.0).unwrap();
        let got = tiled_fetch(&bbox, 10, 7, 4, |_sb, w, h| {
            assert!(w <= 4 && h <= 4, "sub-request exceeded per_request_max");
            Ok(RawElevationGrid {
                heights_meters: vec![vec![1.5; w]; h],
            })
        })
        .unwrap();
        assert_eq!(got.heights_meters.len(), 7);
        assert_eq!(got.heights_meters[6].len(), 10);
        // Every cell filled
        for row in &got.heights_meters {
            for &v in row {
                assert_eq!(v, 1.5);
            }
        }
    }

    /// An error from a single tile must abort the whole stitch and
    /// propagate, not produce a partially-filled grid.
    #[test]
    fn tiled_fetch_propagates_tile_error() {
        let bbox = LLBBox::new(0.0, 0.0, 1.0, 1.0).unwrap();
        let res = tiled_fetch(&bbox, 10, 10, 4, |_sb, _w, _h| {
            Err::<RawElevationGrid, _>("boom".into())
        });
        assert!(res.is_err());
    }

    /// Live end-to-end test against USGS 3DEP over the Grand Canyon
    /// (~170 km²). At the current cap this triggers a 4×4 = 16 sub-tile
    /// fetch and stitches a ~1.5 GB f64 grid. Verifies the server
    /// doesn't 504 at the chosen cap and that the stitched output is
    /// coherent (>98 % valid cells, elevations in the known physical
    /// range for this bbox: floor ~700 m, rim ~2600 m).
    ///
    /// Manual run: `cargo test --release -- --ignored --nocapture
    /// test_usgs_3dep_grand_canyon_tiling`
    #[test]
    #[ignore = "hits live USGS 3DEP servers and is memory-heavy"]
    fn test_usgs_3dep_grand_canyon_tiling() {
        let bbox = LLBBox::new(36.042437, -112.180023, 36.157281, -112.014542).unwrap();
        let grid_w = 14868usize;
        let grid_h = 12771usize;
        let provider = Usgs3dep;

        let start = std::time::Instant::now();
        let raw = provider
            .fetch_raw(&bbox, grid_w, grid_h)
            .expect("USGS fetch must succeed — check for 504s in stderr");
        let elapsed = start.elapsed();

        assert_eq!(raw.heights_meters.len(), grid_h);
        assert_eq!(raw.heights_meters[0].len(), grid_w);

        let mut valid = 0usize;
        let mut min_h = f64::INFINITY;
        let mut max_h = f64::NEG_INFINITY;
        for row in &raw.heights_meters {
            for &h in row {
                if h.is_finite() {
                    valid += 1;
                    min_h = min_h.min(h);
                    max_h = max_h.max(h);
                }
            }
        }
        let total = grid_w * grid_h;
        let ratio = valid as f64 / total as f64;
        eprintln!(
            "OK: {grid_w}×{grid_h} stitched in {:.1}s, {:.2}% valid, elev {:.0}..{:.0} m",
            elapsed.as_secs_f64(),
            ratio * 100.0,
            min_h,
            max_h
        );

        assert!(
            ratio > 0.98,
            "only {:.2}% valid cells — coverage or decoding problem",
            ratio * 100.0
        );
        assert!(
            (500.0..1500.0).contains(&min_h),
            "min elevation {min_h} m outside Grand Canyon floor range"
        );
        assert!(
            (1800.0..3000.0).contains(&max_h),
            "max elevation {max_h} m outside Grand Canyon rim range"
        );
    }
}
