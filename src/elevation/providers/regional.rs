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

// --- Shared helpers ---

/// Pixels of overlap between adjacent sub-tiles. Adjacent tile requests
/// are expanded by `BLEND_OVERLAP` pixels into the neighbour's
/// authoritative area, and the overlap region is linearly cross-faded
/// so that any discontinuity at the authoritative boundary is smeared
/// over `2 * BLEND_OVERLAP` pixels instead of appearing as a vertical
/// cliff.
///
/// This is needed because USGS 3DEP (and likely any provider that
/// stitches multi-flight LiDAR internally) returns slightly different
/// elevations for the SAME latitude depending on the surrounding
/// request bbox — measured ~50 m mean and up to 500 m max discontinuity
/// between adjacent sub-bboxes over the Grand Canyon, caused by
/// inter-flight vertical calibration offsets. Verified that the effect
/// is independent of the resampling method (bilinear vs nearest give
/// identical seams), so the fix has to happen client-side.
///
/// 64 is chosen to make a 170 m mean seam (typical case) transition
/// over ~128 rows ≈ 1.3 m / row — visually indistinguishable from a
/// real slope. Widening further consumes more bandwidth and extra
/// sub-tiles without meaningfully improving the common case.
const BLEND_OVERLAP: usize = 64;

/// Split a large request into overlapping sub-tiles and linearly
/// cross-fade the overlap regions.
///
/// When the requested grid fits in a single upstream request
/// (`grid_width <= per_request_max && grid_height <= per_request_max`)
/// this calls `fetch_tile` once and returns its result unchanged.
///
/// Otherwise the grid is partitioned into a `tiles_x × tiles_y` mosaic
/// of **authoritative** regions covering disjoint pixel ranges
/// `[0..grid_width) × [0..grid_height)`. Each tile's upstream request
/// covers its authoritative region **plus `BLEND_OVERLAP` pixels of
/// padding** on every side that borders another authoritative region
/// (the outer edges of the full grid have no padding). Those padded
/// pixels are fetched twice — once by each tile — and the two copies
/// are linearly blended in the overlap zone, so any per-tile seam is
/// smoothed instead of appearing as a sharp step.
///
/// Row 0 is north (`max_lat`) to match the rest of the elevation pipeline.
///
/// Fetches run sequentially: `fetch_or_cache` hits disk/network and
/// subsequent runs are instant from cache, so parallelism's modest
/// first-run speedup isn't worth the extra memory + risk of upstream
/// rate-limiting (USGS is already flaky under load).
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

    // Authoritative tile size must leave room for BLEND_OVERLAP padding
    // on both sides of the request (the extreme case: a middle tile that
    // has two internal borders). Shrink accordingly so each request
    // still fits under per_request_max.
    let overlap = BLEND_OVERLAP.min(per_request_max / 4);
    let auth_max = per_request_max.saturating_sub(2 * overlap).max(1);
    let tiles_x = grid_width.div_ceil(auth_max);
    let tiles_y = grid_height.div_ceil(auth_max);
    // Balance authoritative sizes so none exceeds auth_max.
    let auth_size_x = grid_width.div_ceil(tiles_x);
    let auth_size_y = grid_height.div_ceil(tiles_y);

    let min_lng = bbox.min().lng();
    let min_lat = bbox.min().lat();
    let max_lng = bbox.max().lng();
    let max_lat = bbox.max().lat();
    let lng_span = max_lng - min_lng;
    let lat_span = max_lat - min_lat;

    let total_tiles = tiles_x * tiles_y;
    eprintln!(
        "Tiled elevation fetch: {grid_width}×{grid_height} grid split into {tiles_x}×{tiles_y} = {total_tiles} sub-tiles (cap {per_request_max}/req, {overlap}px blended overlap)"
    );

    // Accumulator + weight grids for the weighted-average blend.
    let mut accum: Vec<Vec<f64>> = vec![vec![0.0; grid_width]; grid_height];
    let mut weight: Vec<Vec<f64>> = vec![vec![0.0; grid_width]; grid_height];

    for ty in 0..tiles_y {
        let auth_y0 = ty * auth_size_y;
        let auth_y1 = ((ty + 1) * auth_size_y).min(grid_height);
        let has_north_overlap = ty > 0;
        let has_south_overlap = ty + 1 < tiles_y && auth_y1 < grid_height;
        let req_y0 = if has_north_overlap {
            auth_y0 - overlap
        } else {
            auth_y0
        };
        let req_y1 = if has_south_overlap {
            (auth_y1 + overlap).min(grid_height)
        } else {
            auth_y1
        };
        let sub_h = req_y1 - req_y0;

        let sub_max_lat = max_lat - (req_y0 as f64 / grid_height as f64) * lat_span;
        let sub_min_lat = max_lat - (req_y1 as f64 / grid_height as f64) * lat_span;
        let sub_min_lat = sub_min_lat.max(min_lat);

        for tx in 0..tiles_x {
            let auth_x0 = tx * auth_size_x;
            let auth_x1 = ((tx + 1) * auth_size_x).min(grid_width);
            let has_west_overlap = tx > 0;
            let has_east_overlap = tx + 1 < tiles_x && auth_x1 < grid_width;
            let req_x0 = if has_west_overlap {
                auth_x0 - overlap
            } else {
                auth_x0
            };
            let req_x1 = if has_east_overlap {
                (auth_x1 + overlap).min(grid_width)
            } else {
                auth_x1
            };
            let sub_w = req_x1 - req_x0;

            let sub_min_lng = min_lng + (req_x0 as f64 / grid_width as f64) * lng_span;
            let sub_max_lng = min_lng + (req_x1 as f64 / grid_width as f64) * lng_span;
            let sub_max_lng = sub_max_lng.min(max_lng);

            let sub_bbox = LLBBox::new(sub_min_lat, sub_min_lng, sub_max_lat, sub_max_lng)?;

            let raw = fetch_tile(&sub_bbox, sub_w, sub_h)?;

            // Per-pixel weights: 1.0 in the authoritative interior; linear
            // ramp from 0 at the request edge to 1 at the authoritative
            // edge along any side that has blend overlap. When two tiles
            // cover the same pixel their weights blend smoothly because
            // one tile's ramp is decreasing as the neighbour's is
            // increasing over the shared region.
            let weight_axis =
                |g: usize, req_lo: usize, auth_lo: usize, auth_hi: usize, req_hi: usize| -> f64 {
                    if g < auth_lo {
                        // In north/west overlap ramp.
                        ((g - req_lo) + 1) as f64 / (auth_lo - req_lo + 1) as f64
                    } else if g >= auth_hi {
                        // In south/east overlap ramp.
                        ((req_hi - 1) - g + 1) as f64 / (req_hi - auth_hi + 1) as f64
                    } else {
                        1.0
                    }
                };

            for (dy, src_row) in raw.heights_meters.iter().enumerate().take(sub_h) {
                let global_y = req_y0 + dy;
                if global_y >= grid_height {
                    continue;
                }
                let wy = weight_axis(global_y, req_y0, auth_y0, auth_y1, req_y1);
                let accum_row = &mut accum[global_y];
                let weight_row = &mut weight[global_y];
                for (dx, &v) in src_row.iter().enumerate().take(sub_w) {
                    let global_x = req_x0 + dx;
                    if global_x >= grid_width || !v.is_finite() {
                        continue;
                    }
                    let wx = weight_axis(global_x, req_x0, auth_x0, auth_x1, req_x1);
                    let w = wy * wx;
                    accum_row[global_x] += v * w;
                    weight_row[global_x] += w;
                }
            }
        }
    }

    // Normalize accumulator into final blended grid.
    let mut stitched: Vec<Vec<f64>> = vec![vec![f64::NAN; grid_width]; grid_height];
    for y in 0..grid_height {
        for x in 0..grid_width {
            if weight[y][x] > 0.0 {
                stitched[y][x] = accum[y][x] / weight[y][x];
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

    /// Constant-value input: every cell reported by every sub-tile is
    /// the same number. The blend of two equal values is that value, so
    /// the stitched grid must be uniform — no gaps, no numerical drift
    /// from the weighted normalisation.
    #[test]
    fn tiled_fetch_blends_constant_tiles() {
        let bbox = LLBBox::new(40.0, -120.0, 42.0, -118.0).unwrap();
        // per_req=30 leaves overlap=7 (per_req/4), auth_max=16 → tiles
        // partition 50×40 into several pieces with meaningful overlap.
        let got = tiled_fetch(&bbox, 50, 40, 30, |_sb, w, h| {
            Ok(RawElevationGrid {
                heights_meters: vec![vec![42.0; w]; h],
            })
        })
        .unwrap();

        assert_eq!(got.heights_meters.len(), 40);
        assert_eq!(got.heights_meters[0].len(), 50);
        for row in &got.heights_meters {
            for &v in row {
                assert!(v.is_finite(), "unfilled cell in stitched grid");
                assert!(
                    (v - 42.0).abs() < 1e-9,
                    "blend produced unexpected value: {v}"
                );
            }
        }
    }

    /// Two tiles with constant values A and B on either side of a single
    /// authoritative boundary. Outside the blend zone the grid must
    /// contain exactly A or B; inside the blend zone the grid must
    /// transition monotonically from A to B.
    #[test]
    fn tiled_fetch_smooths_step_across_boundary() {
        // Shape: 20×60. per_req=30 → overlap=7, auth_max=16, so tiles_y
        // partitions 60 into two pieces of 30 each; the shared
        // authoritative boundary is at row 30.
        let bbox = LLBBox::new(0.0, 0.0, 60.0, 20.0).unwrap();
        let got = tiled_fetch(&bbox, 20, 60, 30, |sb, w, h| {
            // Tile with sub_max_lat >= 45 is the northern tile (lat span
            // 60, south tile ends near lat 30). Use max_lat to decide.
            let value = if sb.max().lat() > 31.0 { 100.0 } else { 0.0 };
            Ok(RawElevationGrid {
                heights_meters: vec![vec![value; w]; h],
            })
        })
        .unwrap();

        // Top rows (inside north tile's authoritative interior) must be
        // exactly 100; bottom rows (south authoritative interior) exactly 0.
        assert!((got.heights_meters[0][10] - 100.0).abs() < 1e-6);
        assert!((got.heights_meters[59][10] - 0.0).abs() < 1e-6);

        // Middle band should contain a monotonically decreasing column
        // (values go from ~100 above the boundary to ~0 below) with no
        // sharp step.
        let col_10: Vec<f64> = got.heights_meters.iter().map(|r| r[10]).collect();
        for i in 1..col_10.len() {
            assert!(
                col_10[i] <= col_10[i - 1] + 1e-9,
                "column is not monotonically non-increasing at row {i}: {} -> {}",
                col_10[i - 1],
                col_10[i]
            );
        }
        // No single-row step larger than half the original discontinuity
        // — the whole point of overlap+blend is to smear the transition
        // across many rows.
        let max_step = (1..col_10.len())
            .map(|i| (col_10[i - 1] - col_10[i]).abs())
            .fold(0.0f64, f64::max);
        assert!(
            max_step < 50.0,
            "largest single-row step {max_step} is larger than expected after blending a 100→0 boundary"
        );
    }

    /// Uneven tiling: last tile along each axis can be smaller than the
    /// others. Stitching must still cover the whole grid.
    #[test]
    fn tiled_fetch_uneven_last_tile() {
        // cap 4, grid 10×7 → overlap clamped to 1, auth_max=2.
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
        for row in &got.heights_meters {
            for &v in row {
                assert!((v - 1.5).abs() < 1e-9, "unfilled or drifted cell: {v}");
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

    /// Diagnose tile-boundary elevation discontinuities for the bbox that
    /// produced the "horizontal cliff" artifacts. Fetches the user's bbox
    /// via `tiled_fetch`, then measures the max absolute elevation jump
    /// between adjacent rows (a) within a tile interior and (b) across
    /// each horizontal tile boundary. If (b) >> (a), the raw stitched
    /// grid has seams. Also independently fetches two adjacent sub-tiles
    /// and checks whether their shared-boundary rows match — which isolates
    /// whether the issue is in my stitching math or in what the server
    /// returns.
    ///
    /// Manual run: `cargo test --release -- --ignored --nocapture
    /// diag_usgs_tile_boundary_seam`
    #[test]
    #[ignore = "hits live USGS 3DEP servers; diagnostic for tile-boundary seams"]
    fn diag_usgs_tile_boundary_seam() {
        // User-reported bbox (smaller Grand Canyon area, 3 horizontal seams).
        let bbox = LLBBox::new(36.061589, -112.148781, 36.135102, -112.038574).unwrap();
        let grid_w = 9902usize;
        let grid_h = 8175usize;
        let provider = Usgs3dep;

        eprintln!("=== Fetching stitched grid via tiled_fetch ===");
        let raw = provider.fetch_raw(&bbox, grid_w, grid_h).expect("fetch");
        let heights = raw.heights_meters;

        // Compute expected tile partition (same math as tiled_fetch).
        let tiles_y = grid_h.div_ceil(USGS_MAX_SINGLE);
        let tile_size_y = grid_h.div_ceil(tiles_y);
        let boundaries: Vec<usize> = (1..tiles_y).map(|ty| ty * tile_size_y).collect();
        eprintln!(
            "Expect {} horizontal boundaries at rows: {:?}",
            boundaries.len(),
            boundaries
        );

        // For each boundary, compute max |Δ| and mean |Δ| between the two
        // adjacent stitched rows across all columns. Also compute the same
        // for rows 5 above the boundary (within-tile reference).
        let column_diff = |y_a: usize, y_b: usize| -> (f64, f64, usize) {
            let mut max_abs = 0.0f64;
            let mut sum_abs = 0.0f64;
            let mut n = 0usize;
            for (a, b) in heights[y_a].iter().zip(heights[y_b].iter()) {
                if a.is_finite() && b.is_finite() {
                    let d = (a - b).abs();
                    if d > max_abs {
                        max_abs = d;
                    }
                    sum_abs += d;
                    n += 1;
                }
            }
            (max_abs, if n > 0 { sum_abs / n as f64 } else { 0.0 }, n)
        };

        eprintln!("\n=== Row-to-row elevation deltas ===");
        eprintln!(
            "{:<8} {:<15} {:<15} {:<15}",
            "row", "boundary?", "max|Δ|m", "mean|Δ|m"
        );
        for &yb in &boundaries {
            // 5 rows above (within-tile), the boundary, and 5 rows below (within-tile).
            if yb >= 5 {
                let (m, a, _) = column_diff(yb - 5 - 1, yb - 5);
                eprintln!("{:<8} {:<15} {:<15.3} {:<15.3}", yb - 5, "-", m, a);
            }
            let (m, a, n) = column_diff(yb - 1, yb);
            eprintln!(
                "{:<8} {:<15} {:<15.3} {:<15.3} (n={})",
                yb, "YES (seam?)", m, a, n
            );
            if yb + 5 < grid_h {
                let (m, a, _) = column_diff(yb + 5, yb + 5 + 1);
                eprintln!("{:<8} {:<15} {:<15.3} {:<15.3}", yb + 5, "-", m, a);
            }
            eprintln!();
        }

        // Now independently fetch two adjacent sub-tiles and compare their
        // shared-boundary rows.
        eprintln!("=== Independent fetch of two adjacent sub-tiles ===");
        let lat_span = 36.135102 - 36.061589;
        let lng_span = -112.038574_f64 - (-112.148781);
        let tile_size_x = grid_w.div_ceil(grid_w.div_ceil(USGS_MAX_SINGLE));
        let sub_w = tile_size_x;
        let sub_h = tile_size_y;

        // Tile (0,0) — top-left
        let t0_max_lat = 36.135102;
        let t0_min_lat = 36.135102 - (sub_h as f64 / grid_h as f64) * lat_span;
        let t0_min_lng = -112.148781;
        let t0_max_lng = -112.148781 + (sub_w as f64 / grid_w as f64) * lng_span;
        let t0_bbox = LLBBox::new(t0_min_lat, t0_min_lng, t0_max_lat, t0_max_lng).unwrap();
        let t0 = provider.fetch_tile(&t0_bbox, sub_w, sub_h).expect("t0");

        // Tile (1,0) — directly below tile 0 (same x column)
        let t1_max_lat = t0_min_lat;
        let t1_min_lat = 36.135102 - (2.0 * sub_h as f64 / grid_h as f64) * lat_span;
        let t1_bbox = LLBBox::new(t1_min_lat, t0_min_lng, t1_max_lat, t0_max_lng).unwrap();
        let t1 = provider.fetch_tile(&t1_bbox, sub_w, sub_h).expect("t1");

        // Compare: last row of t0 (south edge) vs first row of t1 (north edge).
        // With cell-edge convention, these sample different cells ~cell_h apart.
        // A big mean |Δ| here means the server's sampling convention disagrees
        // with my partition convention — real seam source.
        let t0_last = &t0.heights_meters[sub_h - 1];
        let t1_first = &t1.heights_meters[0];
        let mut max_abs = 0.0f64;
        let mut sum_abs = 0.0f64;
        let mut n = 0usize;
        for x in 0..sub_w {
            let a = t0_last[x];
            let b = t1_first[x];
            if a.is_finite() && b.is_finite() {
                let d = (a - b).abs();
                if d > max_abs {
                    max_abs = d;
                }
                sum_abs += d;
                n += 1;
            }
        }
        eprintln!(
            "Tile0.last_row vs Tile1.first_row — max|Δ|={:.3} m, mean|Δ|={:.3} m (n={})",
            max_abs,
            if n > 0 { sum_abs / n as f64 } else { 0.0 },
            n
        );

        // Also compare: what if we take tile0's SECOND-TO-LAST row (one cell
        // above the boundary) and tile1's first row? If values are closer,
        // the server is using inclusive-endpoints convention and there's a
        // 1-row overlap at boundaries.
        let t0_second_last = &t0.heights_meters[sub_h - 2];
        let mut max_abs_2 = 0.0f64;
        let mut sum_abs_2 = 0.0f64;
        let mut n_2 = 0usize;
        for x in 0..sub_w {
            let a = t0_second_last[x];
            let b = t1_first[x];
            if a.is_finite() && b.is_finite() {
                let d = (a - b).abs();
                if d > max_abs_2 {
                    max_abs_2 = d;
                }
                sum_abs_2 += d;
                n_2 += 1;
            }
        }
        eprintln!(
            "Tile0.second_last_row vs Tile1.first_row — max|Δ|={:.3} m, mean|Δ|={:.3} m (n={})",
            max_abs_2,
            if n_2 > 0 { sum_abs_2 / n_2 as f64 } else { 0.0 },
            n_2
        );

        // Orientation check. Tile 0 covers lat 36.117..36.135 (northern
        // half of the bbox), tile 1 covers 36.098..36.117. In the Grand
        // Canyon area this bbox straddles, northern latitudes are
        // higher-elevation (closer to the North Rim), southern are lower
        // (canyon interior). If the server returns rows top-down (row 0
        // = max_lat), tile0.row0.mean should be HIGHER than
        // tile0.row_last.mean. If bottom-up, the relationship inverts.
        let row_mean = |row: &[f64]| -> f64 {
            let (sum, n) = row
                .iter()
                .filter(|h| h.is_finite())
                .fold((0.0f64, 0usize), |(s, c), h| (s + h, c + 1));
            if n > 0 {
                sum / n as f64
            } else {
                f64::NAN
            }
        };
        eprintln!("\n=== Orientation probe ===");
        eprintln!(
            "Tile0 bbox: lat [{:.6}..{:.6}]   (row 0 SHOULD be north edge, row {} SHOULD be south edge if top-down)",
            t0_bbox.min().lat(),
            t0_bbox.max().lat(),
            sub_h - 1
        );
        eprintln!(
            "  row 0 mean: {:.1} m,   row {} mean: {:.1} m",
            row_mean(&t0.heights_meters[0]),
            sub_h - 1,
            row_mean(&t0.heights_meters[sub_h - 1])
        );
        eprintln!(
            "Tile1 bbox: lat [{:.6}..{:.6}]",
            t1_bbox.min().lat(),
            t1_bbox.max().lat()
        );
        eprintln!(
            "  row 0 mean: {:.1} m,   row {} mean: {:.1} m",
            row_mean(&t1.heights_meters[0]),
            sub_h - 1,
            row_mean(&t1.heights_meters[sub_h - 1])
        );

        // Also: if tiles are internally bottom-up, the stitched grid's
        // row 0 (tile 0's row 0) would actually sample near the boundary
        // between tile 0 and tile 1, NOT at the global max_lat. Check
        // stitched[0] vs stitched[grid_h - 1].
        eprintln!(
            "Stitched row 0 mean: {:.1} m,  stitched row {} mean: {:.1} m",
            row_mean(&heights[0]),
            grid_h - 1,
            row_mean(&heights[grid_h - 1])
        );
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
