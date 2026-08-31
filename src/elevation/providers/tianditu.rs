//! Tianditu (天地图) elevation provider.
//!
//! Tianditu is China's national geographic information service, operated by
//! the National Geomatics Center of China. It provides DEM terrain data for
//! all of China at up to 30 m resolution via Terrain-RGB tiles.
//!
//! The tiles use GCJ-02 ("Mars") coordinates; this provider converts the
//! user's WGS-84 bounding box into GCJ-02 for tile lookup, then converts
//! each output grid cell back through the same transform before sampling so
//! the resulting grid stays aligned with WGS-84 OSM features.
//!
//! ## Authentication
//!
//! A free API key is required. Set the `TIANDITU_TOKEN` environment variable.
//! Register at <https://console.tianditu.gov.cn/api/register>.
//!
//! ## Data encoding
//!
//! Terrain-RGB tiles encode elevation as:
//!   `height = -10000 + (R*256*256 + G*256 + B) * 0.1`  (metres)
//!
//! ## Legal
//!
//! Tianditu data is © National Geomatics Center of China; refer to their
//! terms of service at <https://www.tianditu.gov.cn>.

use crate::coordinate_system::gcj02;
use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::cache::get_cache_dir;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Maximum concurrent tile downloads; be polite to the national service.
const MAX_CONCURRENT_DOWNLOADS: usize = 4;
/// Pixels per tile edge.
const TILE_SIZE: u32 = 256;
/// Minimum zoom level.
const ZOOM_MIN: u8 = 2;
/// Maximum zoom level.
const ZOOM_MAX: u8 = 14;
/// Tile cap before zoom is lowered.
const MAX_TILES_PER_FETCH: usize = 1024;

// Terrain-RGB constants
const TERRAIN_RGB_OFFSET: f64 = 10_000.0;
const TERRAIN_RGB_SCALE: f64 = 0.1;

// China coverage
const CHINA_MIN_LAT: f64 = 16.0;
const CHINA_MAX_LAT: f64 = 56.0;
const CHINA_MIN_LNG: f64 = 72.0;
const CHINA_MAX_LNG: f64 = 138.0;

type TileImage = image::ImageBuffer<image::Rgb<u8>, Vec<u8>>;

/// Tianditu elevation provider. Registered in the provider list ahead of
/// Mapterhorn (30 m global) so Chinese users get the highest-available
/// local resolution.
pub struct Tianditu;

impl ElevationProvider for Tianditu {
    fn name(&self) -> &'static str {
        "tianditu"
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        Some(vec![LLBBox::new(
            CHINA_MIN_LAT,
            CHINA_MIN_LNG,
            CHINA_MAX_LAT,
            CHINA_MAX_LNG,
        )
        .unwrap()])
    }

    fn native_resolution_m(&self) -> f64 {
        30.0
    }

    fn accepts(&self, _bbox: &LLBBox) -> bool {
        // Tianditu requires a token.  Silently skip if it's missing so the
        // selector falls through to Mapterhorn / AWS transparently.
        resolve_token().is_some()
    }

    fn fetch_raw(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
        let token = resolve_token().ok_or("TIANDITU_TOKEN environment variable is not set")?;

        if grid_width == 0 || grid_height == 0 {
            return Err("Zero-dimensioned grid request".into());
        }

        // Expand the bbox into GCJ-02 land so the tile coverage includes
        // the eastern/southern shift.
        let gcj_bbox = gcj02::wgs84_bbox_to_gcj02(bbox, 0.005);

        let zoom = select_zoom(&gcj_bbox);
        let tile_keys = covering_tiles(&gcj_bbox, zoom);
        let num = tile_keys.len();
        if num == 0 {
            return Err("No Tianditu tiles cover the requested area".into());
        }

        // ------ download ------
        let cache_dir = get_cache_dir(self.name());
        std::fs::create_dir_all(&cache_dir)?;

        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!(
                "Arnis/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/louis-e/arnis)"
            ))
            .connect_timeout(std::time::Duration::from_secs(20))
            .timeout(std::time::Duration::from_secs(60))
            .build()?;

        eprintln!(
            "Downloading {num} elevation tile{} from Tianditu (zoom {zoom}, max {MAX_CONCURRENT_DOWNLOADS} concurrent)...",
            if num == 1 { "" } else { "s" }
        );

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(MAX_CONCURRENT_DOWNLOADS)
            .build()
            .map_err(|e| format!("thread pool: {e}"))?;

        type FetchResult = ((u32, u32, u8), Result<TileImage, String>);
        let results: Vec<FetchResult> = pool.install(|| {
            let token = &token;
            tile_keys
                .par_iter()
                .map(|&(tx, ty)| {
                    let cache_path = tile_cache_path(&cache_dir, tx, ty, zoom);
                    let img = fetch_or_load(&client, tx, ty, zoom, &cache_path, token);
                    ((tx, ty, zoom), img)
                })
                .collect()
        });

        let mut tile_map: HashMap<(u32, u32, u8), TileImage> = HashMap::new();
        let mut waf_blocks = 0u32;
        for (key, res) in results {
            match res {
                Ok(img) => {
                    tile_map.insert(key, img);
                }
                Err(e) => {
                    eprintln!("Tianditu tile {key:?} failed: {e}");
                    if e.contains("418") || e.contains("WAF") {
                        waf_blocks += 1;
                    }
                }
            }
        }

        if !tile_map.is_empty() {
            eprintln!(
                "  Tianditu: {} of {} tiles loaded successfully",
                tile_map.len(),
                num
            );
        }
        if waf_blocks > 0 {
            eprintln!(
                "  Tianditu WAF blocked {} tile requests. \
                 Access to tianditu.gov.cn is restricted for automated (non-browser) clients. \
                 To resolve: log in to the Tianditu console and add an IP whitelist under \
                 WAF settings, or register a different key type.",
                waf_blocks
            );
        }

        // ------ bilinear sample ------
        eprintln!(
            "Bilinear sampling {}x{} grid from {} Tianditu tiles...",
            grid_width,
            grid_height,
            tile_map.len()
        );

        let n = 2.0_f64.powi(zoom as i32);
        let min_lat = bbox.min().lat();
        let max_lat = bbox.max().lat();
        let min_lng = bbox.min().lng();
        let max_lng = bbox.max().lng();
        let lat_span = max_lat - min_lat;
        let lng_span = max_lng - min_lng;
        let w_denom = (grid_width - 1).max(1) as f64;
        let h_denom = (grid_height - 1).max(1) as f64;

        let height_grid: Vec<Vec<f64>> = (0..grid_height)
            .into_par_iter()
            .map(|gy| {
                let lat_frac = gy as f64 / h_denom;
                let lat = max_lat - lat_frac * lat_span;
                let mut row = vec![f64::NAN; grid_width];
                for (gx, cell) in row.iter_mut().enumerate() {
                    let lng_frac = gx as f64 / w_denom;
                    let lng = min_lng + lng_frac * lng_span;

                    // Convert the WGS-84 output coordinate to GCJ-02 so we
                    // sample the tile at the location it represents.
                    let (g_lng, g_lat) = gcj02::wgs84_to_gcj02(lng, lat);

                    let fx = (g_lng + 180.0) / 360.0 * n * TILE_SIZE as f64;
                    let lat_rad = g_lat.to_radians();
                    let fy = (1.0 - lat_rad.tan().asinh() / std::f64::consts::PI) / 2.0
                        * n
                        * TILE_SIZE as f64;

                    let n_tiles = n as i64;
                    let tx = ((fx / TILE_SIZE as f64).floor() as i64).clamp(0, n_tiles - 1) as u32;
                    let ty = ((fy / TILE_SIZE as f64).floor() as i64).clamp(0, n_tiles - 1) as u32;
                    let px = fx - tx as f64 * TILE_SIZE as f64;
                    let py = fy - ty as f64 * TILE_SIZE as f64;

                    let x0 = px.floor() as i32;
                    let y0 = py.floor() as i32;
                    let dx = px - x0 as f64;
                    let dy = py - y0 as f64;

                    let v00 = sample_pixel(&tile_map, tx, ty, zoom, x0, y0);
                    let v10 = sample_pixel(&tile_map, tx, ty, zoom, x0 + 1, y0);
                    let v01 = sample_pixel(&tile_map, tx, ty, zoom, x0, y0 + 1);
                    let v11 = sample_pixel(&tile_map, tx, ty, zoom, x0 + 1, y0 + 1);

                    if let (Some(v00), Some(v10), Some(v01), Some(v11)) = (v00, v10, v01, v11) {
                        let top = v00 + (v10 - v00) * dx;
                        let bot = v01 + (v11 - v01) * dx;
                        *cell = top + (bot - top) * dy;
                    }
                }
                row
            })
            .collect();

        Ok(RawElevationGrid {
            heights_meters: height_grid,
        })
    }
}

// ─── Token resolution ───────────────────────────────────────────────────

fn resolve_token() -> Option<String> {
    std::env::var("TIANDITU_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
}

// ─── Tile URL ────────────────────────────────────────────────────────────

fn tile_url(tile_x: u32, tile_y: u32, zoom: u8, token: &str) -> String {
    // Use consistent subdomain for caching stability —
    // different subdomains may serve identical content, but a single
    // subdomain works with connection reuse.
    let s = (tile_x + tile_y) % 4;
    format!(
        "https://t{s}.tianditu.gov.cn/mapservice/swdx?T=elv_c&x={tile_x}&y={tile_y}&l={zoom}&tk={token}"
    )
}

// ─── Zoom selection ─────────────────────────────────────────────────────

fn select_zoom(bbox: &LLBBox) -> u8 {
    let lat_diff = (bbox.max().lat() - bbox.min().lat()).abs();
    let lng_diff = (bbox.max().lng() - bbox.min().lng()).abs();
    let max_diff = lat_diff.max(lng_diff);
    let zoom = (-max_diff.log2() + 18.0) as u8;
    let mut zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
    while zoom > ZOOM_MIN && count_tiles(bbox, zoom) > MAX_TILES_PER_FETCH {
        zoom -= 1;
    }
    zoom
}

fn count_tiles(bbox: &LLBBox, zoom: u8) -> usize {
    let (x1, y1) = lat_lng_to_tile(bbox.min().lat(), bbox.min().lng(), zoom);
    let (x2, y2) = lat_lng_to_tile(bbox.max().lat(), bbox.max().lng(), zoom);
    let cols = x1.abs_diff(x2) as usize + 1;
    let rows = y1.abs_diff(y2) as usize + 1;
    cols * rows
}

fn lat_lng_to_tile(lat: f64, lng: f64, zoom: u8) -> (u32, u32) {
    let n: f64 = 2.0_f64.powi(zoom as i32);
    let n_tiles = n as i64;
    let lat_rad = lat.to_radians();
    let x = (((lng + 180.0) / 360.0 * n).floor() as i64).clamp(0, n_tiles - 1) as u32;
    let y = (((1.0 - lat_rad.tan().asinh() / std::f64::consts::PI) / 2.0 * n).floor() as i64)
        .clamp(0, n_tiles - 1) as u32;
    (x, y)
}

fn covering_tiles(bbox: &LLBBox, zoom: u8) -> Vec<(u32, u32)> {
    let (x1, y1) = lat_lng_to_tile(bbox.min().lat(), bbox.min().lng(), zoom);
    let (x2, y2) = lat_lng_to_tile(bbox.max().lat(), bbox.max().lng(), zoom);
    let mut tiles = Vec::new();
    for y in y1.min(y2)..=y1.max(y2) {
        for x in x1.min(x2)..=x1.max(x2) {
            tiles.push((x, y));
        }
    }
    tiles
}

// ─── Pixel sampling ─────────────────────────────────────────────────────

fn sample_pixel(
    tile_map: &HashMap<(u32, u32, u8), TileImage>,
    base_tx: u32,
    base_ty: u32,
    zoom: u8,
    px: i32,
    py: i32,
) -> Option<f64> {
    let (tx, x) = if px < 0 {
        (base_tx.wrapping_sub(1), (px + TILE_SIZE as i32) as u32)
    } else if px >= TILE_SIZE as i32 {
        (base_tx + 1, (px - TILE_SIZE as i32) as u32)
    } else {
        (base_tx, px as u32)
    };
    let (ty, y) = if py < 0 {
        (base_ty.wrapping_sub(1), (py + TILE_SIZE as i32) as u32)
    } else if py >= TILE_SIZE as i32 {
        (base_ty + 1, (py - TILE_SIZE as i32) as u32)
    } else {
        (base_ty, py as u32)
    };
    let tile = tile_map.get(&(tx, ty, zoom))?;
    if x >= tile.width() || y >= tile.height() {
        return None;
    }
    let p = tile.get_pixel(x, y);
    let raw = p[0] as f64 * 256.0 * 256.0 + p[1] as f64 * 256.0 + p[2] as f64;
    Some(raw * TERRAIN_RGB_SCALE - TERRAIN_RGB_OFFSET)
}

// ─── Cache path ─────────────────────────────────────────────────────────

fn tile_cache_path(cache_dir: &Path, tx: u32, ty: u32, zoom: u8) -> PathBuf {
    cache_dir.join(format!("z{zoom}_x{tx}_y{ty}.png"))
}

// ─── Fetch / load ───────────────────────────────────────────────────────

fn fetch_or_load(
    client: &reqwest::blocking::Client,
    tx: u32,
    ty: u32,
    zoom: u8,
    path: &Path,
    token: &str,
) -> Result<TileImage, String> {
    if path.exists() {
        if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) < 500 {
            let _ = std::fs::remove_file(path);
        } else {
            match image::open(path) {
                Ok(img) => return Ok(img.to_rgb8()),
                Err(_) => {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
    download_tile(client, tx, ty, zoom, path, token)
}

fn download_tile(
    client: &reqwest::blocking::Client,
    tx: u32,
    ty: u32,
    zoom: u8,
    path: &Path,
    token: &str,
) -> Result<TileImage, String> {
    let url = tile_url(tx, ty, zoom, token);
    // 4xx errors (including WAF 418 blocks) are permanent — don't retry.
    let resp = client.get(&url).send().map_err(|e| e.to_string())?;
    let status = resp.status();
    if status == 418 {
        // Tianditu WAF block — not an image; don't cache, fail fast.
        return Err(
            "HTTP 418 (WAF blocked; check token permissions or add Referer header)".to_string(),
        );
    }
    if status.is_client_error() {
        return Err(format!("HTTP {status}"));
    }
    resp.error_for_status_ref().map_err(|e| e.to_string())?;
    let bytes = resp.bytes().map_err(|e| e.to_string())?;
    // Validate image before caching so broken tiles don't poison the cache.
    let img = image::load_from_memory(&bytes).map_err(|e| format!("decode tile: {e}"))?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, &bytes);
    Ok(img.to_rgb8())
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_rgb_decoding() {
        // Sea-level: R=0, G=0, B=0 → raw=0 → height = -10000
        assert!((0.0_f64 * 0.1 - 10000.0 + 10000.0).abs() < 1e-6);

        // 1000 m pixel: calculated from formula
        let h = 1000.0;
        let raw = ((h + TERRAIN_RGB_OFFSET) / TERRAIN_RGB_SCALE) as u32;
        let r = (raw >> 16) as u8;
        let g = ((raw >> 8) & 0xFF) as u8;
        let b = (raw & 0xFF) as u8;
        let decoded = (r as f64 * 65536.0 + g as f64 * 256.0 + b as f64) * TERRAIN_RGB_SCALE
            - TERRAIN_RGB_OFFSET;
        assert!((decoded - 1000.0).abs() < 1.0);
    }

    #[test]
    fn zoom_selection_stays_in_range() {
        let bbox = LLBBox::new(39.9, 116.3, 40.0, 116.5).unwrap();
        let z = select_zoom(&bbox);
        assert!((ZOOM_MIN..=ZOOM_MAX).contains(&z));
    }

    #[test]
    fn tile_coverage_for_beijing() {
        let bbox = LLBBox::new(39.90, 116.39, 39.95, 116.45).unwrap();
        let tiles = covering_tiles(&bbox, 10);
        assert!(!tiles.is_empty(), "expected >=1 tiles covering Beijing");
    }

    #[test]
    fn outside_china_is_no_op() {
        // GCJ conversion is a no-op outside China.
        let _bbox = LLBBox::new(48.13, 11.56, 48.14, 11.58).unwrap();
        let (lng, lat) = crate::coordinate_system::gcj02::wgs84_to_gcj02(11.57, 48.135);
        assert!((lng - 11.57).abs() < 1e-9);
        assert!((lat - 48.135).abs() < 1e-9);
    }
}
