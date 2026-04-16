use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::cache::get_cache_dir;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use rayon::prelude::*;
use std::path::Path;

/// AWS S3 Terrarium tiles endpoint (no API key required)
const AWS_TERRARIUM_URL: &str =
    "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png";
/// Terrarium format offset for height decoding
const TERRARIUM_OFFSET: f64 = 32768.0;
/// Minimum zoom level for terrain tiles
const MIN_ZOOM: u8 = 10;
/// Maximum zoom level for terrain tiles
const MAX_ZOOM: u8 = 15;
/// Maximum concurrent tile downloads to be respectful to AWS
const MAX_CONCURRENT_DOWNLOADS: usize = 8;

/// Maximum number of retry attempts for tile downloads
const TILE_DOWNLOAD_MAX_RETRIES: u32 = 3;
/// Base delay in milliseconds for exponential backoff between retries
const TILE_DOWNLOAD_RETRY_BASE_DELAY_MS: u64 = 500;

/// RGB image buffer type for elevation tiles
type TileImage = image::ImageBuffer<image::Rgb<u8>, Vec<u8>>;
/// Result type for tile download operations
type TileDownloadResult = Result<((u32, u32), TileImage), String>;

/// AWS Terrain Tiles provider (~30m global, up to ~10m in some areas).
/// Global fallback provider.
pub struct AwsTerrain;

impl ElevationProvider for AwsTerrain {
    fn name(&self) -> &'static str {
        "aws"
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        None // Global coverage
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
        let zoom: u8 = calculate_zoom_level(bbox);
        let tiles: Vec<(u32, u32)> = get_tile_coordinates(bbox, zoom);

        let tile_cache_dir = get_cache_dir(self.name());
        if !tile_cache_dir.exists() {
            std::fs::create_dir_all(&tile_cache_dir)?;
        }

        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!("arnis/", env!("CARGO_PKG_VERSION")))
            .build()?;

        let num_tiles = tiles.len();
        println!(
            "Downloading {num_tiles} elevation tiles from AWS (up to {MAX_CONCURRENT_DOWNLOADS} concurrent)..."
        );

        let thread_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(MAX_CONCURRENT_DOWNLOADS)
            .build()
            .map_err(|e| format!("Failed to create thread pool: {e}"))?;

        let downloaded_tiles: Vec<TileDownloadResult> = thread_pool.install(|| {
            tiles
                .par_iter()
                .map(|(tile_x, tile_y)| {
                    let tile_path = tile_cache_dir.join(format!("z{zoom}_x{tile_x}_y{tile_y}.png"));
                    let rgb_img = fetch_or_load_tile(&client, *tile_x, *tile_y, zoom, &tile_path)?;
                    Ok(((*tile_x, *tile_y), rgb_img))
                })
                .collect()
        });

        // Collect tiles into a HashMap for random access during grid sampling
        let mut tile_map: std::collections::HashMap<(u32, u32), TileImage> =
            std::collections::HashMap::new();
        for result in downloaded_tiles {
            match result {
                Ok((key, img)) => {
                    tile_map.insert(key, img);
                }
                Err(e) => {
                    eprintln!("Warning: Failed to download tile: {e}");
                }
            }
        }

        println!(
            "Bilinear sampling {} tiles into {}x{} grid...",
            tile_map.len(),
            grid_width,
            grid_height
        );

        let n = 2.0_f64.powi(zoom as i32);
        let mut height_grid: Vec<Vec<f64>> = vec![vec![f64::NAN; grid_width]; grid_height];

        // Iterate over target grid and bilinearly sample from tiles
        #[allow(clippy::needless_range_loop)]
        for gy in 0..grid_height {
            for gx in 0..grid_width {
                // Map grid cell to geographic coordinates
                let lat = bbox.max().lat()
                    - (gy as f64 / (grid_height - 1).max(1) as f64)
                        * (bbox.max().lat() - bbox.min().lat());
                let lng = bbox.min().lng()
                    + (gx as f64 / (grid_width - 1).max(1) as f64)
                        * (bbox.max().lng() - bbox.min().lng());

                // Convert lat/lng to fractional tile pixel coordinates
                let lat_rad = lat.to_radians();
                let fx_global = (lng + 180.0) / 360.0 * n * 256.0;
                let fy_global =
                    (1.0 - lat_rad.tan().asinh() / std::f64::consts::PI) / 2.0 * n * 256.0;

                // Determine tile and fractional pixel within tile
                let tile_x = (fx_global / 256.0).floor() as u32;
                let tile_y = (fy_global / 256.0).floor() as u32;
                let px = fx_global - tile_x as f64 * 256.0;
                let py = fy_global - tile_y as f64 * 256.0;

                // Bilinear interpolation from the 4 surrounding pixels
                let x0 = px.floor() as i32;
                let y0 = py.floor() as i32;
                let dx = px - x0 as f64;
                let dy = py - y0 as f64;

                let v00 = sample_tile_pixel(&tile_map, tile_x, tile_y, x0, y0);
                let v10 = sample_tile_pixel(&tile_map, tile_x, tile_y, x0 + 1, y0);
                let v01 = sample_tile_pixel(&tile_map, tile_x, tile_y, x0, y0 + 1);
                let v11 = sample_tile_pixel(&tile_map, tile_x, tile_y, x0 + 1, y0 + 1);

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

/// Sample a single pixel from the tile map, handling tile boundary crossover.
/// Returns the decoded Terrarium height in meters, or None if the tile is missing.
fn sample_tile_pixel(
    tile_map: &std::collections::HashMap<(u32, u32), TileImage>,
    base_tile_x: u32,
    base_tile_y: u32,
    px: i32,
    py: i32,
) -> Option<f64> {
    // Handle tile boundary crossover
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
    let height =
        (pixel[0] as f64 * 256.0 + pixel[1] as f64 + pixel[2] as f64 / 256.0) - TERRARIUM_OFFSET;
    Some(height)
}

fn calculate_zoom_level(bbox: &LLBBox) -> u8 {
    let lat_diff: f64 = (bbox.max().lat() - bbox.min().lat()).abs();
    let lng_diff: f64 = (bbox.max().lng() - bbox.min().lng()).abs();
    let max_diff: f64 = lat_diff.max(lng_diff);
    let zoom: u8 = (-max_diff.log2() + 20.0) as u8;
    zoom.clamp(MIN_ZOOM, MAX_ZOOM)
}

fn lat_lng_to_tile(lat: f64, lng: f64, zoom: u8) -> (u32, u32) {
    let lat_rad: f64 = lat.to_radians();
    let n: f64 = 2.0_f64.powi(zoom as i32);
    let x: u32 = ((lng + 180.0) / 360.0 * n).floor() as u32;
    let y: u32 = ((1.0 - lat_rad.tan().asinh() / std::f64::consts::PI) / 2.0 * n).floor() as u32;
    (x, y)
}

fn get_tile_coordinates(bbox: &LLBBox, zoom: u8) -> Vec<(u32, u32)> {
    let (x1, y1) = lat_lng_to_tile(bbox.min().lat(), bbox.min().lng(), zoom);
    let (x2, y2) = lat_lng_to_tile(bbox.max().lat(), bbox.max().lng(), zoom);

    let mut tiles: Vec<(u32, u32)> = Vec::new();
    for x in x1.min(x2)..=x1.max(x2) {
        for y in y1.min(y2)..=y1.max(y2) {
            tiles.push((x, y));
        }
    }
    tiles
}

fn download_tile(
    client: &reqwest::blocking::Client,
    tile_x: u32,
    tile_y: u32,
    zoom: u8,
    tile_path: &Path,
) -> Result<TileImage, String> {
    println!("Fetching tile x={tile_x},y={tile_y},z={zoom} from AWS Terrain Tiles");
    let url: String = AWS_TERRARIUM_URL
        .replace("{z}", &zoom.to_string())
        .replace("{x}", &tile_x.to_string())
        .replace("{y}", &tile_y.to_string());

    let mut last_error: String = String::new();

    for attempt in 0..TILE_DOWNLOAD_MAX_RETRIES {
        if attempt > 0 {
            let delay_ms = TILE_DOWNLOAD_RETRY_BASE_DELAY_MS * (1 << (attempt - 1));
            eprintln!(
                "Retry attempt {}/{} for tile x={},y={},z={} after {}ms delay",
                attempt,
                TILE_DOWNLOAD_MAX_RETRIES - 1,
                tile_x,
                tile_y,
                zoom,
                delay_ms
            );
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        }

        match download_tile_once(client, &url, tile_path) {
            Ok(img) => return Ok(img),
            Err(e) => {
                last_error = e;
                if attempt < TILE_DOWNLOAD_MAX_RETRIES - 1 {
                    eprintln!(
                        "Tile download failed for x={},y={},z={}: {}",
                        tile_x, tile_y, zoom, last_error
                    );
                }
            }
        }
    }

    Err(format!(
        "Failed to download tile x={},y={},z={} after {} attempts: {}",
        tile_x, tile_y, zoom, TILE_DOWNLOAD_MAX_RETRIES, last_error
    ))
}

fn download_tile_once(
    client: &reqwest::blocking::Client,
    url: &str,
    tile_path: &Path,
) -> Result<TileImage, String> {
    let response = client.get(url).send().map_err(|e| e.to_string())?;
    response.error_for_status_ref().map_err(|e| e.to_string())?;
    let bytes = response.bytes().map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    std::fs::write(tile_path, &bytes).map_err(|e| e.to_string())?;
    Ok(img.to_rgb8())
}

fn fetch_or_load_tile(
    client: &reqwest::blocking::Client,
    tile_x: u32,
    tile_y: u32,
    zoom: u8,
    tile_path: &Path,
) -> Result<TileImage, String> {
    if tile_path.exists() {
        let file_size = std::fs::metadata(tile_path).map(|m| m.len()).unwrap_or(0);
        if file_size < 1000 {
            eprintln!(
                "Warning: Cached tile at {} is too small ({file_size} bytes). Re-downloading...",
                tile_path.display(),
            );
            let _ = std::fs::remove_file(tile_path);
            return download_tile(client, tile_x, tile_y, zoom, tile_path);
        }

        match image::open(tile_path) {
            Ok(img) => {
                println!(
                    "Loading cached tile x={tile_x},y={tile_y},z={zoom} from {}",
                    tile_path.display()
                );
                Ok(img.to_rgb8())
            }
            Err(e) => {
                eprintln!(
                    "Cached tile at {} is corrupted or invalid: {}. Re-downloading...",
                    tile_path.display(),
                    e
                );
                #[cfg(feature = "gui")]
                send_log(
                    LogLevel::Warning,
                    "Cached tile is corrupted or invalid. Re-downloading...",
                );

                if let Err(e) = std::fs::remove_file(tile_path) {
                    eprintln!("Warning: Failed to remove corrupted tile file: {e}");
                    #[cfg(feature = "gui")]
                    send_log(
                        LogLevel::Warning,
                        "Failed to remove corrupted tile file during re-download.",
                    );
                }

                download_tile(client, tile_x, tile_y, zoom, tile_path)
            }
        }
    } else {
        download_tile(client, tile_x, tile_y, zoom, tile_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terrarium_height_decoding() {
        let sea_level_pixel = [128u8, 0, 0];
        let height = (sea_level_pixel[0] as f64 * 256.0
            + sea_level_pixel[1] as f64
            + sea_level_pixel[2] as f64 / 256.0)
            - TERRARIUM_OFFSET;
        assert_eq!(height, 0.0);

        let test_pixel = [131u8, 232, 0];
        let height =
            (test_pixel[0] as f64 * 256.0 + test_pixel[1] as f64 + test_pixel[2] as f64 / 256.0)
                - TERRARIUM_OFFSET;
        assert_eq!(height, 1000.0);

        let below_sea_pixel = [127u8, 156, 0];
        let height = (below_sea_pixel[0] as f64 * 256.0
            + below_sea_pixel[1] as f64
            + below_sea_pixel[2] as f64 / 256.0)
            - TERRARIUM_OFFSET;
        assert_eq!(height, -100.0);
    }

    #[test]
    fn test_aws_url_generation() {
        let url = AWS_TERRARIUM_URL
            .replace("{z}", "15")
            .replace("{x}", "17436")
            .replace("{y}", "11365");
        assert_eq!(
            url,
            "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/15/17436/11365.png"
        );
    }

    #[test]
    #[ignore]
    fn test_aws_tile_fetch() {
        use reqwest::blocking::Client;

        let client = Client::new();
        let url = "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/15/17436/11365.png";

        let response = client.get(url).send();
        assert!(response.is_ok());

        let response = response.unwrap();
        assert!(response.status().is_success());
        assert!(response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("image"));
    }
}
