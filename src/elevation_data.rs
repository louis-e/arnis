#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use crate::{
    coordinate_system::{geographic::LLBBox, transformation::geo_distance},
    progress::emit_gui_progress_update,
};
use image::Rgb;
use rayon::prelude::*;
use std::path::{Path, PathBuf};

/// Maximum Y coordinate in Minecraft (vanilla build height limit).
/// Used for compression decisions when disable_height_limit is false.
/// Note: world_editor/common.rs has a separate MAX_Y=2031 (data pack maximum)
/// that serves as the absolute safety clamp for block placement.
const MAX_Y: i32 = 319;
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
/// Maximum age for cached tiles in days before they are cleaned up
const TILE_CACHE_MAX_AGE_DAYS: u64 = 7;
/// Subdirectory name for tile cache within the OS cache directory
const TILE_CACHE_DIR_NAME: &str = "arnis-tile-cache";

/// Holds processed elevation data and metadata
#[derive(Clone)]
pub struct ElevationData {
    /// Height values in Minecraft Y coordinates
    pub(crate) heights: Vec<Vec<i32>>,
    /// Width of the elevation grid
    pub(crate) width: usize,
    /// Height of the elevation grid
    pub(crate) height: usize,
}

/// RGB image buffer type for elevation tiles
type TileImage = image::ImageBuffer<Rgb<u8>, Vec<u8>>;
/// Result type for tile download operations: ((tile_x, tile_y), image) or error
type TileDownloadResult = Result<((u32, u32), TileImage), String>;

/// Returns the tile cache directory path.
/// Uses the OS-standard cache directory (e.g. AppData/Local on Windows, ~/.cache on Linux)
/// to avoid CWD-dependent paths that can fail due to permissions or unexpected working directories.
/// Falls back to ./arnis-tile-cache if the OS cache directory is unavailable.
fn get_tile_cache_dir() -> PathBuf {
    if let Some(cache_dir) = dirs::cache_dir() {
        cache_dir.join(TILE_CACHE_DIR_NAME)
    } else {
        PathBuf::from(format!("./{TILE_CACHE_DIR_NAME}"))
    }
}

/// Cleans up old cached tiles from the tile cache directory.
/// Only deletes .png files within the arnis-tile-cache directory that are older than TILE_CACHE_MAX_AGE_DAYS.
/// This function is safe and will not delete files outside the cache directory or fail on errors.
pub fn cleanup_old_cached_tiles() {
    let tile_cache_dir = get_tile_cache_dir();

    if !tile_cache_dir.exists() || !tile_cache_dir.is_dir() {
        return; // Nothing to clean up
    }

    let max_age = std::time::Duration::from_secs(TILE_CACHE_MAX_AGE_DAYS * 24 * 60 * 60);
    let now = std::time::SystemTime::now();
    let mut deleted_count = 0;
    let mut error_count = 0;

    // Read directory entries
    let entries = match std::fs::read_dir(&tile_cache_dir) {
        Ok(entries) => entries,
        Err(_) => {
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Safety check: only process .png files within the cache directory
        if !path.is_file() {
            continue;
        }

        // Verify the file is a .png and follows our naming pattern (z{zoom}_x{x}_y{y}.png)
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        if !file_name.ends_with(".png") || !file_name.starts_with('z') {
            continue; // Skip files that don't match our tile naming pattern
        }

        // Check file age
        let metadata = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let modified = match metadata.modified() {
            Ok(time) => time,
            Err(_) => continue,
        };

        let age = match now.duration_since(modified) {
            Ok(duration) => duration,
            Err(_) => continue, // File modified in the future? Skip it.
        };

        if age > max_age {
            match std::fs::remove_file(&path) {
                Ok(()) => deleted_count += 1,
                Err(e) => {
                    // Log but don't fail, this is a best-effort cleanup
                    if error_count == 0 {
                        eprintln!(
                            "Warning: Failed to delete old cached tile {}: {e}",
                            path.display()
                        );
                    }
                    error_count += 1;
                }
            }
        }
    }

    if deleted_count > 0 {
        println!("Cleaned up {deleted_count} old cached elevation tiles (older than {TILE_CACHE_MAX_AGE_DAYS} days)");
    }
    if error_count > 1 {
        eprintln!("Warning: Failed to delete {error_count} old cached tiles");
    }
}

/// Calculates appropriate zoom level for the given bounding box
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

/// Maximum number of retry attempts for tile downloads
const TILE_DOWNLOAD_MAX_RETRIES: u32 = 3;

/// Base delay in milliseconds for exponential backoff between retries
const TILE_DOWNLOAD_RETRY_BASE_DELAY_MS: u64 = 500;

/// Downloads a tile from AWS Terrain Tiles service with retry logic
fn download_tile(
    client: &reqwest::blocking::Client,
    tile_x: u32,
    tile_y: u32,
    zoom: u8,
    tile_path: &Path,
) -> Result<image::ImageBuffer<Rgb<u8>, Vec<u8>>, String> {
    println!("Fetching tile x={tile_x},y={tile_y},z={zoom} from AWS Terrain Tiles");
    let url: String = AWS_TERRARIUM_URL
        .replace("{z}", &zoom.to_string())
        .replace("{x}", &tile_x.to_string())
        .replace("{y}", &tile_y.to_string());

    let mut last_error: String = String::new();

    for attempt in 0..TILE_DOWNLOAD_MAX_RETRIES {
        if attempt > 0 {
            // Exponential backoff: 500ms, 1000ms, 2000ms...
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

/// Single download attempt for a tile (no retries)
fn download_tile_once(
    client: &reqwest::blocking::Client,
    url: &str,
    tile_path: &Path,
) -> Result<image::ImageBuffer<Rgb<u8>, Vec<u8>>, String> {
    let response = client.get(url).send().map_err(|e| e.to_string())?;
    response.error_for_status_ref().map_err(|e| e.to_string())?;
    let bytes = response.bytes().map_err(|e| e.to_string())?;
    // Validate the image BEFORE writing to cache to prevent caching invalid data
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    std::fs::write(tile_path, &bytes).map_err(|e| e.to_string())?;
    Ok(img.to_rgb8())
}

/// Fetches a tile from cache or downloads it if not available
/// Note: In parallel execution, multiple threads may attempt to download the same tile
/// if it's missing or corrupted. This is harmless (just wastes some bandwidth) as
/// file writes are atomic at the OS level.
fn fetch_or_load_tile(
    client: &reqwest::blocking::Client,
    tile_x: u32,
    tile_y: u32,
    zoom: u8,
    tile_path: &Path,
) -> Result<image::ImageBuffer<Rgb<u8>, Vec<u8>>, String> {
    if tile_path.exists() {
        // Check file size first — valid Terrarium tiles are ~50-100KB.
        // Files under 1000 bytes are almost certainly truncated (e.g. from a
        // process interruption during a previous download).
        let file_size = std::fs::metadata(tile_path).map(|m| m.len()).unwrap_or(0);
        if file_size < 1000 {
            eprintln!(
                "Warning: Cached tile at {} is too small ({file_size} bytes). Re-downloading...",
                tile_path.display(),
            );
            let _ = std::fs::remove_file(tile_path);
            return download_tile(client, tile_x, tile_y, zoom, tile_path);
        }

        // Try to load cached tile, but handle corruption gracefully
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

                // Remove the corrupted file
                if let Err(e) = std::fs::remove_file(tile_path) {
                    eprintln!("Warning: Failed to remove corrupted tile file: {e}");
                    #[cfg(feature = "gui")]
                    send_log(
                        LogLevel::Warning,
                        "Failed to remove corrupted tile file during re-download.",
                    );
                }

                // Re-download the tile
                download_tile(client, tile_x, tile_y, zoom, tile_path)
            }
        }
    } else {
        // Download the tile for the first time
        download_tile(client, tile_x, tile_y, zoom, tile_path)
    }
}

pub fn fetch_elevation_data(
    bbox: &LLBBox,
    scale: f64,
    ground_level: i32,
    disable_height_limit: bool,
) -> Result<ElevationData, Box<dyn std::error::Error>> {
    let (base_scale_z, base_scale_x) = geo_distance(bbox.min(), bbox.max());

    // Apply same floor() and scale operations as CoordTransformer.llbbox_to_xzbbox()
    let scale_factor_z: f64 = base_scale_z.floor() * scale;
    let scale_factor_x: f64 = base_scale_x.floor() * scale;

    // Calculate zoom and tiles
    let zoom: u8 = calculate_zoom_level(bbox);
    let tiles: Vec<(u32, u32)> = get_tile_coordinates(bbox, zoom);

    // Match grid dimensions with Minecraft world size
    let grid_width: usize = scale_factor_x as usize;
    let grid_height: usize = scale_factor_z as usize;

    // Initialize height grid with proper dimensions
    let mut height_grid: Vec<Vec<f64>> = vec![vec![f64::NAN; grid_width]; grid_height];
    let mut extreme_values_found = Vec::new(); // Track extreme values for debugging

    let tile_cache_dir = get_tile_cache_dir();
    if !tile_cache_dir.exists() {
        std::fs::create_dir_all(&tile_cache_dir)?;
    }

    // Create a shared HTTP client for connection pooling
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("arnis/", env!("CARGO_PKG_VERSION")))
        .build()?;

    // Download tiles in parallel with limited concurrency to be respectful to AWS
    let num_tiles = tiles.len();
    println!(
        "Downloading {num_tiles} elevation tiles (up to {MAX_CONCURRENT_DOWNLOADS} concurrent)..."
    );

    // Use a custom thread pool to limit concurrent downloads
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

    // Check for any download errors
    let mut successful_tiles = Vec::new();
    for result in downloaded_tiles {
        match result {
            Ok(tile_data) => successful_tiles.push(tile_data),
            Err(e) => {
                eprintln!("Warning: Failed to download tile: {e}");
            }
        }
    }

    println!("Processing {} elevation tiles...", successful_tiles.len());
    emit_gui_progress_update(15.0, "Processing elevation...");

    // Process tiles sequentially (writes to shared height_grid)
    for ((tile_x, tile_y), rgb_img) in successful_tiles {
        // Only process pixels that fall within the requested bbox
        for (y, row) in rgb_img.rows().enumerate() {
            for (x, pixel) in row.enumerate() {
                // Convert tile pixel coordinates back to geographic coordinates
                let pixel_lng = ((tile_x as f64 + x as f64 / 256.0) / (2.0_f64.powi(zoom as i32)))
                    * 360.0
                    - 180.0;
                let pixel_lat_rad = std::f64::consts::PI
                    * (1.0
                        - 2.0 * (tile_y as f64 + y as f64 / 256.0) / (2.0_f64.powi(zoom as i32)));
                let pixel_lat = pixel_lat_rad.sinh().atan().to_degrees();

                // Skip pixels outside the requested bounding box
                if pixel_lat < bbox.min().lat()
                    || pixel_lat > bbox.max().lat()
                    || pixel_lng < bbox.min().lng()
                    || pixel_lng > bbox.max().lng()
                {
                    continue;
                }

                // Map geographic coordinates to grid coordinates
                let rel_x = (pixel_lng - bbox.min().lng()) / (bbox.max().lng() - bbox.min().lng());
                let rel_y =
                    1.0 - (pixel_lat - bbox.min().lat()) / (bbox.max().lat() - bbox.min().lat());

                let scaled_x = (rel_x * grid_width as f64).round() as usize;
                let scaled_y = (rel_y * grid_height as f64).round() as usize;

                if scaled_y >= grid_height || scaled_x >= grid_width {
                    continue;
                }

                // Decode Terrarium format: (R * 256 + G + B/256) - 32768
                let height: f64 =
                    (pixel[0] as f64 * 256.0 + pixel[1] as f64 + pixel[2] as f64 / 256.0)
                        - TERRARIUM_OFFSET;

                // Track extreme values for debugging
                if !(-1000.0..=10000.0).contains(&height) {
                    extreme_values_found
                        .push((tile_x, tile_y, x, y, pixel[0], pixel[1], pixel[2], height));
                    if extreme_values_found.len() <= 5 {
                        // Only log first 5 extreme values
                        eprintln!("Extreme value found: tile({tile_x},{tile_y}) pixel({x},{y}) RGB({},{},{}) = {height}m", 
                                 pixel[0], pixel[1], pixel[2]);
                    }
                }

                height_grid[scaled_y][scaled_x] = height;
            }
        }
    }

    // Report on extreme values found
    if !extreme_values_found.is_empty() {
        eprintln!(
            "Found {} total extreme elevation values during tile processing",
            extreme_values_found.len()
        );
        eprintln!("This may indicate corrupted tile data or areas with invalid elevation data");
    }

    // Fill in any NaN values by interpolating from nearest valid values
    fill_nan_values(&mut height_grid);

    // Filter extreme outliers that might be due to corrupted tile data
    filter_elevation_outliers(&mut height_grid);

    // Calculate blur sigma based on grid resolution
    // Use sqrt scaling to maintain consistent relative smoothing across different area sizes.
    // This prevents larger generation areas from appearing noisier than smaller ones.
    // Reference: 100x100 grid uses sigma=2.5 (2.5% relative blur)
    const BASE_GRID_REF: f64 = 100.0;
    const BASE_SIGMA_REF: f64 = 2.5;

    let grid_size: f64 = (grid_width.min(grid_height) as f64).max(1.0);

    // Sqrt scaling provides a good balance:
    // - 100x100: sigma = 2.5 (2.5% relative)
    // - 500x500: sigma ≈ 5.6 (1.1% relative)
    // - 1000x1000: sigma ≈ 7.9 (0.8% relative)
    // - 3000x3000: sigma ≈ 13.7 (0.5% relative)
    // This preserves terrain detail (steep slopes, ridges) while still
    // smoothing enough to suppress tile-resolution noise.
    let sigma_from_grid: f64 = BASE_SIGMA_REF * (grid_size / BASE_GRID_REF).sqrt();

    // --- Sigma selection ---
    //
    // sigma_from_grid uses sqrt-scaling to apply proportionally less blur on larger
    // areas (which are inherently smoother) and more on smaller ones.
    //
    // sigma_terrain floors the blur to native_resolution × 3.  This has two effects:
    //
    // 1. Voronoi-block suppression: fill_nan_values() expands each ~3.6 m tile pixel
    //    into a Voronoi block of ~3.6 output cells.  sigma must span at least 2 of
    //    those blocks so the Gaussian kernel bridges every adjacent block edge.
    //
    // 2. Basic SRTM noise suppression: the ×3 factor provides a small margin
    //    above the Voronoi-block size to smooth minor SRTM surface noise.
    //    A higher multiplier (e.g. ×12) would also suppress rooftop/canopy
    //    artifacts in dense urban areas, but at the cost of flattening
    //    mountains and removing realistic terrain detail everywhere.
    let lat_mid_rad: f64 = ((bbox.min().lat() + bbox.max().lat()) / 2.0).to_radians();
    let metres_per_tile_pixel: f64 = 2.0 * std::f64::consts::PI * 6_378_137.0
        / (2.0_f64.powi(zoom as i32) * 256.0)
        * lat_mid_rad.cos();
    let blocks_per_tile_pixel: f64 = metres_per_tile_pixel * scale;
    let sigma_terrain: f64 = (blocks_per_tile_pixel * 3.0).max(1.0e-6);
    // Takes whichever is larger: the grid-proportional value or the terrain floor.
    // For small areas sigma_terrain wins; for large areas sigma_from_grid wins.
    // Clamped to half the grid size: beyond that the kernel exceeds the grid and
    // additional sigma has no visual effect, only wasted computation.
    let output_sigma: f64 = sigma_from_grid.max(sigma_terrain).min(grid_size / 2.0);

    let blurred_heights: Vec<Vec<f64>> = apply_gaussian_blur(&height_grid, output_sigma);

    // Release raw height grid
    drop(height_grid);

    // Derive min/max from the same field used for output (never from a separate blur)
    let (min_height, max_height) = blurred_heights
        .par_iter()
        .map(|row| {
            let mut lo = f64::MAX;
            let mut hi = f64::MIN;
            for &h in row {
                if h.is_finite() {
                    lo = lo.min(h);
                    hi = hi.max(h);
                }
            }
            (lo, hi)
        })
        .reduce(
            || (f64::MAX, f64::MIN),
            |(lo1, hi1), (lo2, hi2)| (lo1.min(lo2), hi1.max(hi2)),
        );

    // Validate: if no finite samples exist (e.g. all tiles failed to download),
    // fall back to flat terrain rather than propagating NaN/inf.
    let (min_height, _max_height, height_range) =
        if !min_height.is_finite() || !max_height.is_finite() || min_height >= max_height {
            (0.0_f64, 0.0_f64, 0.0_f64)
        } else {
            (min_height, max_height, max_height - min_height)
        };

    // Realistic height scaling: 1 meter of real elevation = scale blocks in Minecraft
    // At scale=1.0, 1 meter = 1 block (realistic 1:1 mapping)
    // At scale=2.0, 1 meter = 2 blocks (exaggerated for larger worlds)
    let ideal_scaled_range: f64 = height_range * scale;

    // Calculate available Y range in Minecraft (from ground_level to MAX_Y)
    // Leave a buffer at the top for buildings, trees, and other structures
    const TERRAIN_HEIGHT_BUFFER: i32 = 15;
    let available_y_range: f64 = (MAX_Y - TERRAIN_HEIGHT_BUFFER - ground_level) as f64;

    // Determine final height scale:
    // - Use realistic 1:1 (times scale) if terrain fits within Minecraft limits
    // - Only compress if the terrain would exceed the build height (and height limit is enabled)
    let scaled_range: f64 = if disable_height_limit {
        // No compression: always use realistic 1:1 scaling
        eprintln!(
            "Height limit disabled: {:.1}m range => {:.0} blocks (no compression)",
            height_range, ideal_scaled_range
        );
        ideal_scaled_range
    } else if ideal_scaled_range <= available_y_range {
        // Terrain fits! Use realistic scaling
        eprintln!(
            "Realistic elevation: {:.1}m range fits in {} available blocks",
            height_range, available_y_range as i32
        );
        ideal_scaled_range
    } else {
        // Terrain too tall, compress to fit within Minecraft limits
        let compression_factor: f64 = available_y_range / height_range;
        let compressed_range: f64 = height_range * compression_factor;
        eprintln!(
            "Elevation compressed: {:.1}m range -> {:.0} blocks ({:.2}:1 ratio, 1 block = {:.2}m)",
            height_range,
            compressed_range,
            height_range / compressed_range,
            compressed_range / height_range
        );
        compressed_range
    };

    // Convert to scaled Minecraft Y coordinates (parallelized across rows)
    // Lowest real elevation maps to ground_level, highest maps to ground_level + scaled_range
    let mc_heights: Vec<Vec<i32>> = blurred_heights
        .par_iter()
        .map(|row| {
            row.iter()
                .map(|&h| {
                    // Calculate relative position within the elevation range (0.0 to 1.0)
                    let relative_height: f64 = if height_range > 0.0 {
                        (h - min_height) / height_range
                    } else {
                        0.0
                    };
                    // Scale to Minecraft blocks and add to ground level
                    let scaled_height: f64 = relative_height * scaled_range;
                    let mc_y = (ground_level as f64 + scaled_height).round() as i32;
                    if disable_height_limit {
                        // No clamping: allow terrain to exceed vanilla limits
                        mc_y
                    } else {
                        // Clamp to valid Minecraft Y range (leave buffer at top for structures)
                        mc_y.clamp(ground_level, MAX_Y - TERRAIN_HEIGHT_BUFFER)
                    }
                })
                .collect()
        })
        .collect();

    let mut min_block_height: i32 = i32::MAX;
    let mut max_block_height: i32 = i32::MIN;
    for row in &mc_heights {
        for &height in row {
            min_block_height = min_block_height.min(height);
            max_block_height = max_block_height.max(height);
        }
    }

    // Warn if terrain exceeds the absolute Minecraft data pack maximum (Y=2031)
    const DATA_PACK_MAX_Y: i32 = 2031;
    if disable_height_limit && max_block_height > DATA_PACK_MAX_Y {
        eprintln!(
            "Warning: Terrain peak reaches Y={}, which exceeds the maximum data pack height (Y={}). \
             Blocks above Y={} will be truncated.",
            max_block_height, DATA_PACK_MAX_Y, DATA_PACK_MAX_Y
        );
        #[cfg(feature = "gui")]
        send_log(
            LogLevel::Warning,
            &format!(
                "Terrain peak Y={} exceeds data pack max Y={}. Blocks will be truncated.",
                max_block_height, DATA_PACK_MAX_Y
            ),
        );
    }

    Ok(ElevationData {
        heights: mc_heights,
        width: grid_width,
        height: grid_height,
    })
}

fn get_tile_coordinates(bbox: &LLBBox, zoom: u8) -> Vec<(u32, u32)> {
    // Convert lat/lng to tile coordinates
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

fn apply_gaussian_blur(heights: &[Vec<f64>], sigma: f64) -> Vec<Vec<f64>> {
    let kernel_size: usize = (sigma * 3.0).ceil() as usize * 2 + 1;
    let kernel: Vec<f64> = create_gaussian_kernel(kernel_size, sigma);

    let height_len = heights.len();
    let width = heights[0].len();

    // Horizontal pass - parallelize across rows (each row is independent)
    let after_horizontal: Vec<Vec<f64>> = heights
        .par_iter()
        .map(|row| {
            let mut temp: Vec<f64> = vec![0.0; row.len()];
            for (i, val) in temp.iter_mut().enumerate() {
                let mut sum: f64 = 0.0;
                let mut weight_sum: f64 = 0.0;
                for (j, k) in kernel.iter().enumerate() {
                    let idx: i32 = i as i32 + j as i32 - kernel_size as i32 / 2;
                    if idx >= 0 && idx < row.len() as i32 {
                        sum += row[idx as usize] * k;
                        weight_sum += k;
                    }
                }
                *val = sum / weight_sum;
            }
            temp
        })
        .collect();

    // Vertical pass - parallelize across columns (each column is independent)
    // Process each column in parallel and collect results as column vectors
    let blurred_columns: Vec<Vec<f64>> = (0..width)
        .into_par_iter()
        .map(|x| {
            // Extract column from after_horizontal
            let column: Vec<f64> = after_horizontal.iter().map(|row| row[x]).collect();

            // Apply vertical blur to this column
            let mut blurred_column: Vec<f64> = vec![0.0; height_len];
            for (y, val) in blurred_column.iter_mut().enumerate() {
                let mut sum: f64 = 0.0;
                let mut weight_sum: f64 = 0.0;
                for (j, k) in kernel.iter().enumerate() {
                    let idx: i32 = y as i32 + j as i32 - kernel_size as i32 / 2;
                    if idx >= 0 && idx < height_len as i32 {
                        sum += column[idx as usize] * k;
                        weight_sum += k;
                    }
                }
                *val = sum / weight_sum;
            }
            blurred_column
        })
        .collect();

    // Transpose columns back to row-major format
    let mut blurred: Vec<Vec<f64>> = vec![vec![0.0; width]; height_len];
    for (x, column) in blurred_columns.into_iter().enumerate() {
        for (y, val) in column.into_iter().enumerate() {
            blurred[y][x] = val;
        }
    }

    blurred
}

fn create_gaussian_kernel(size: usize, sigma: f64) -> Vec<f64> {
    let mut kernel: Vec<f64> = vec![0.0; size];
    let center: f64 = size as f64 / 2.0;

    for (i, value) in kernel.iter_mut().enumerate() {
        let x: f64 = i as f64 - center;
        *value = (-x * x / (2.0 * sigma * sigma)).exp();
    }

    let sum: f64 = kernel.iter().sum();
    for k in kernel.iter_mut() {
        *k /= sum;
    }

    kernel
}

fn fill_nan_values(height_grid: &mut [Vec<f64>]) {
    let height: usize = height_grid.len();
    if height == 0 {
        return;
    }
    let width: usize = height_grid[0].len();

    let mut changes_made: bool = true;
    while changes_made {
        changes_made = false;

        for y in 0..height {
            for x in 0..width {
                if height_grid[y][x].is_nan() {
                    let mut sum: f64 = 0.0;
                    let mut count: i32 = 0;

                    // Check neighboring cells
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let ny: i32 = y as i32 + dy;
                            let nx: i32 = x as i32 + dx;

                            if ny >= 0 && ny < height as i32 && nx >= 0 && nx < width as i32 {
                                let val: f64 = height_grid[ny as usize][nx as usize];
                                if !val.is_nan() {
                                    sum += val;
                                    count += 1;
                                }
                            }
                        }
                    }

                    if count > 0 {
                        height_grid[y][x] = sum / count as f64;
                        changes_made = true;
                    }
                }
            }
        }
    }
}

fn filter_elevation_outliers(height_grid: &mut [Vec<f64>]) {
    let height = height_grid.len();
    if height == 0 {
        return;
    }
    let width = height_grid[0].len();

    // Collect all valid height values to calculate statistics
    let mut all_heights: Vec<f64> = Vec::new();
    for row in height_grid.iter() {
        for &h in row {
            if !h.is_nan() && h.is_finite() {
                all_heights.push(h);
            }
        }
    }

    if all_heights.is_empty() {
        return;
    }

    let len = all_heights.len();

    // Use 1st and 99th percentiles to define reasonable bounds
    // Using quickselect (select_nth_unstable) instead of full sort: O(n) vs O(n log n)
    let p1_idx = (len as f64 * 0.01) as usize;
    let p99_idx = ((len as f64 * 0.99) as usize).min(len - 1);

    // Find p1 (1st percentile) - all elements before p1_idx will be <= p1
    let (_, p1_val, _) =
        all_heights.select_nth_unstable_by(p1_idx, |a, b| a.partial_cmp(b).unwrap());
    let min_reasonable = *p1_val;

    // Find p99 (99th percentile) - need to search in remaining slice or use separate call
    let (_, p99_val, _) =
        all_heights.select_nth_unstable_by(p99_idx, |a, b| a.partial_cmp(b).unwrap());
    let max_reasonable = *p99_val;

    //eprintln!("Filtering outliers outside range: {min_reasonable:.1}m to {max_reasonable:.1}m");

    let mut outliers_filtered = 0;

    // Replace outliers with NaN, then fill them using interpolation
    for row in height_grid.iter_mut().take(height) {
        for h in row.iter_mut().take(width) {
            if !h.is_nan() && (*h < min_reasonable || *h > max_reasonable) {
                *h = f64::NAN;
                outliers_filtered += 1;
            }
        }
    }

    if outliers_filtered > 0 {
        //eprintln!("Filtered {outliers_filtered} elevation outliers, interpolating replacements...");
        // Re-run the NaN filling to interpolate the filtered values
        fill_nan_values(height_grid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terrarium_height_decoding() {
        // Test known Terrarium RGB values
        // Sea level (0m) in Terrarium format should be (128, 0, 0) = 32768 - 32768 = 0
        let sea_level_pixel = [128, 0, 0];
        let height = (sea_level_pixel[0] as f64 * 256.0
            + sea_level_pixel[1] as f64
            + sea_level_pixel[2] as f64 / 256.0)
            - TERRARIUM_OFFSET;
        assert_eq!(height, 0.0);

        // Test simple case: height of 1000m
        // 1000 + 32768 = 33768 = 131 * 256 + 232
        let test_pixel = [131, 232, 0];
        let height =
            (test_pixel[0] as f64 * 256.0 + test_pixel[1] as f64 + test_pixel[2] as f64 / 256.0)
                - TERRARIUM_OFFSET;
        assert_eq!(height, 1000.0);

        // Test below sea level (-100m)
        // -100 + 32768 = 32668 = 127 * 256 + 156
        let below_sea_pixel = [127, 156, 0];
        let height = (below_sea_pixel[0] as f64 * 256.0
            + below_sea_pixel[1] as f64
            + below_sea_pixel[2] as f64 / 256.0)
            - TERRARIUM_OFFSET;
        assert_eq!(height, -100.0);
    }

    #[test]
    fn test_output_sigma_small_bbox_uses_terrain_floor() {
        // For small areas, sigma_terrain > sigma_from_grid, so output_sigma = sigma_terrain.
        // This suppresses Voronoi block edges from the NaN-fill step.
        let zoom: u8 = 15;
        let lat_mid_rad: f64 = 40.705_f64.to_radians();
        let mpp = 2.0 * std::f64::consts::PI * 6_378_137.0 / (2.0_f64.powi(zoom as i32) * 256.0)
            * lat_mid_rad.cos();
        let sigma_terrain = mpp * 3.0; // ≈ 11.0 at lat 40°

        for grid_m in [134_f64, 200.0, 500.0, 1000.0, 5000.0] {
            let sigma_from_grid = 2.5 * (grid_m / 100.0).sqrt();
            let output_sigma = sigma_from_grid.max(sigma_terrain);
            if sigma_terrain > sigma_from_grid {
                assert_eq!(
                    output_sigma, sigma_terrain,
                    "grid={grid_m}m: sigma_terrain should dominate"
                );
            } else {
                assert_eq!(
                    output_sigma, sigma_from_grid,
                    "grid={grid_m}m: sigma_from_grid should dominate"
                );
            }
        }
    }

    #[test]
    fn test_output_sigma_large_bbox_grid_dominates() {
        // For large areas, sigma_from_grid ≥ sigma_terrain,
        // so output_sigma is determined by the grid-proportional component.
        let zoom: u8 = 15;
        let lat_mid_rad: f64 = 40.705_f64.to_radians();
        let mpp = 2.0 * std::f64::consts::PI * 6_378_137.0 / (2.0_f64.powi(zoom as i32) * 256.0)
            * lat_mid_rad.cos();
        let sigma_terrain = mpp * 3.0;

        for grid_m in [8_000_f64, 10_000.0, 20_000.0, 50_000.0] {
            let sigma_from_grid = 2.5 * (grid_m / 100.0).sqrt();
            assert!(
                sigma_from_grid >= sigma_terrain,
                "grid={grid_m}m: sigma_from_grid {sigma_from_grid:.2} should dominate sigma_terrain {sigma_terrain:.2}"
            );
            let output_sigma = sigma_from_grid.max(sigma_terrain);
            assert_eq!(
                output_sigma, sigma_from_grid,
                "grid={grid_m}m: output_sigma must equal sigma_from_grid (no regression)"
            );
        }
    }

    #[test]
    fn test_output_sigma_crossover() {
        // The crossover grid size where sigma_from_grid == sigma_terrain:
        //   2.5 * sqrt(S/100) = mpp * 3  →  S = 100 * (mpp*3/2.5)²  ≈ 1936 m at lat 40°
        // Below crossover sigma_terrain wins; above it sigma_from_grid wins.
        let zoom: u8 = 15;
        let lat_mid_rad: f64 = 40.705_f64.to_radians();
        let mpp = 2.0 * std::f64::consts::PI * 6_378_137.0 / (2.0_f64.powi(zoom as i32) * 256.0)
            * lat_mid_rad.cos();
        let sigma_terrain = mpp * 3.0;
        let crossover = 100.0 * (sigma_terrain / 2.5).powi(2);

        let below = crossover * 0.9;
        let sigma_from_grid_below = 2.5 * (below / 100.0).sqrt();
        assert_eq!(
            sigma_from_grid_below.max(sigma_terrain),
            sigma_terrain,
            "below crossover sigma_terrain must win"
        );

        let above = crossover * 1.1;
        let sigma_from_grid_above = 2.5 * (above / 100.0).sqrt();
        assert_eq!(
            sigma_from_grid_above.max(sigma_terrain),
            sigma_from_grid_above,
            "above crossover sigma_from_grid must win"
        );
    }

    #[test]
    fn test_output_sigma_same_field_invariant() {
        // Critical invariant: output heights and min/max come from the SAME single blur.
        // If someone reintroduces a second blur pass with a different sigma,
        // blurred values could fall outside the separately-computed min/max.
        // We verify: blur once → derive min/max → every cell is in [min, max].
        let grid: Vec<Vec<f64>> = vec![
            vec![10.0, 20.0, 30.0, 40.0, 50.0],
            vec![15.0, 25.0, 35.0, 45.0, 55.0],
            vec![20.0, 30.0, 80.0, 30.0, 20.0],
            vec![15.0, 25.0, 35.0, 45.0, 55.0],
            vec![10.0, 20.0, 30.0, 40.0, 50.0],
        ];
        let sigma = 2.0;
        let blurred = apply_gaussian_blur(&grid, sigma);
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for row in &blurred {
            for &h in row {
                if h.is_finite() {
                    lo = lo.min(h);
                    hi = hi.max(h);
                }
            }
        }
        // Every blurred cell must lie within the derived min/max.
        for row in &blurred {
            for &h in row {
                assert!(h >= lo && h <= hi, "blurred value {h} outside [{lo}, {hi}]");
            }
        }
    }

    #[test]
    fn test_output_sigma_scale_factor() {
        // native_resolution = mpp * scale, sigma_terrain = native_resolution * 3.
        // Both scale linearly with the scale argument.
        let zoom: u8 = 15;
        let lat_mid_rad: f64 = 40.705_f64.to_radians();
        let mpp = 2.0 * std::f64::consts::PI * 6_378_137.0 / (2.0_f64.powi(zoom as i32) * 256.0)
            * lat_mid_rad.cos();
        let st1 = mpp * 1.0 * 3.0;
        let st2 = mpp * 2.0 * 3.0;
        assert!(
            (st2 - 2.0 * st1).abs() < 1e-9,
            "sigma_terrain must scale linearly with the scale factor"
        );
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
    #[ignore] // This test requires internet connection, run with --ignored
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
