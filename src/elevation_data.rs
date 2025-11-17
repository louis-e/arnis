use crate::coordinate_system::{geographic::LLBBox, transformation::geo_distance};
use crate::telemetry::{send_log, LogLevel};
use image::Rgb;
use std::path::Path;

/// Maximum Y coordinate in Minecraft (build height limit)
const MAX_Y: i32 = 319;
/// Scale factor for converting real elevation to Minecraft heights
const BASE_HEIGHT_SCALE: f64 = 0.7;
/// AWS S3 Terrarium tiles endpoint (no API key required)
const AWS_TERRARIUM_URL: &str =
    "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png";
/// Terrarium format offset for height decoding
const TERRARIUM_OFFSET: f64 = 32768.0;
/// Minimum zoom level for terrain tiles
const MIN_ZOOM: u8 = 10;
/// Maximum zoom level for terrain tiles
const MAX_ZOOM: u8 = 15;

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

/// Downloads a tile from AWS Terrain Tiles service
fn download_tile(
    client: &reqwest::blocking::Client,
    tile_x: u32,
    tile_y: u32,
    zoom: u8,
    tile_path: &Path,
) -> Result<image::ImageBuffer<Rgb<u8>, Vec<u8>>, Box<dyn std::error::Error>> {
    println!("Fetching tile x={tile_x},y={tile_y},z={zoom} from AWS Terrain Tiles");
    let url: String = AWS_TERRARIUM_URL
        .replace("{z}", &zoom.to_string())
        .replace("{x}", &tile_x.to_string())
        .replace("{y}", &tile_y.to_string());

    let response: reqwest::blocking::Response = client.get(&url).send()?;
    response.error_for_status_ref()?;
    let bytes = response.bytes()?;
    std::fs::write(tile_path, &bytes)?;
    let img: image::DynamicImage = image::load_from_memory(&bytes)?;
    Ok(img.to_rgb8())
}

pub fn fetch_elevation_data(
    bbox: &LLBBox,
    scale: f64,
    ground_level: i32,
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

    let client: reqwest::blocking::Client = reqwest::blocking::Client::new();

    let tile_cache_dir = Path::new("./arnis-tile-cache");
    if !tile_cache_dir.exists() {
        std::fs::create_dir_all(tile_cache_dir)?;
    }

    // Fetch and process each tile
    for (tile_x, tile_y) in &tiles {
        // Check if tile is already cached
        let tile_path = tile_cache_dir.join(format!("z{zoom}_x{tile_x}_y{tile_y}.png"));

        let rgb_img: image::ImageBuffer<Rgb<u8>, Vec<u8>> = if tile_path.exists() {
            // Check if the cached file has a reasonable size (PNG files should be at least a few KB)
            let file_size = match std::fs::metadata(&tile_path) {
                Ok(metadata) => metadata.len(),
                Err(_) => 0,
            };

            if file_size < 1000 {
                eprintln!("Warning: Cached tile at {} appears to be too small ({} bytes). Refetching tile.",
                         tile_path.display(), file_size);
                send_log(LogLevel::Warning, "Cached tile appears to be too small. Refetching tile.");

                // Remove the potentially corrupted file
                if let Err(remove_err) = std::fs::remove_file(&tile_path) {
                    eprintln!(
                        "Warning: Failed to remove corrupted tile file: {}",
                        remove_err
                    );
                    send_log(
                        LogLevel::Warning,
                        "Failed to remove corrupted tile file during refetching.",
                    );
                }

                // Re-download the tile
                download_tile(&client, *tile_x, *tile_y, zoom, &tile_path)?
            } else {
                println!(
                    "Loading cached tile x={tile_x},y={tile_y},z={zoom} from {}",
                    tile_path.display()
                );

                // Try to load cached tile, but handle corruption gracefully
                match image::open(&tile_path) {
                    Ok(img) => img.to_rgb8(),
                    Err(e) => {
                        eprintln!("Cached tile at {} is corrupted or invalid: {}. Re-downloading...",
                            tile_path.display(),
                            e);
                        send_log(LogLevel::Warning, "Cached tile is corrupted or invalid. Re-downloading...");

                        // Remove the corrupted file
                        if let Err(remove_err) = std::fs::remove_file(&tile_path) {
                            eprintln!(
                                "Warning: Failed to remove corrupted tile file: {}",
                                remove_err
                            );
                            send_log(LogLevel::Warning, "Failed to remove corrupted tile file during re-download.");
                        }

                        // Re-download the tile
                        download_tile(&client, *tile_x, *tile_y, zoom, &tile_path)?
                    }
                }
            }
        } else {
            // Download the tile for the first time
            download_tile(&client, *tile_x, *tile_y, zoom, &tile_path)?
        };

        // Only process pixels that fall within the requested bbox
        for (y, row) in rgb_img.rows().enumerate() {
            for (x, pixel) in row.enumerate() {
                // Convert tile pixel coordinates back to geographic coordinates
                let pixel_lng = ((*tile_x as f64 + x as f64 / 256.0) / (2.0_f64.powi(zoom as i32)))
                    * 360.0
                    - 180.0;
                let pixel_lat_rad = std::f64::consts::PI
                    * (1.0
                        - 2.0 * (*tile_y as f64 + y as f64 / 256.0) / (2.0_f64.powi(zoom as i32)));
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
    // Reference points for tuning:
    const SMALL_GRID_REF: f64 = 100.0; // Reference grid size
    const SMALL_SIGMA_REF: f64 = 15.0; // Sigma for 100x100 grid
    const LARGE_GRID_REF: f64 = 1000.0; // Reference grid size
    const LARGE_SIGMA_REF: f64 = 7.0; // Sigma for 1000x1000 grid

    let grid_size: f64 = (grid_width.min(grid_height) as f64).max(1.0);

    let sigma: f64 = if grid_size <= SMALL_GRID_REF {
        // Linear scaling for small grids
        SMALL_SIGMA_REF * (grid_size / SMALL_GRID_REF)
    } else {
        // Logarithmic scaling for larger grids
        let ln_small: f64 = SMALL_GRID_REF.ln();
        let ln_large: f64 = LARGE_GRID_REF.ln();
        let log_grid_size: f64 = grid_size.ln();
        let t: f64 = (log_grid_size - ln_small) / (ln_large - ln_small);
        SMALL_SIGMA_REF + t * (LARGE_SIGMA_REF - SMALL_SIGMA_REF)
    };

    /* eprintln!(
        "Grid: {}x{}, Blur sigma: {:.2}",
        grid_width, grid_height, sigma
    ); */

    // Continue with the existing blur and conversion to Minecraft heights...
    let blurred_heights: Vec<Vec<f64>> = apply_gaussian_blur(&height_grid, sigma);

    let mut mc_heights: Vec<Vec<i32>> = Vec::with_capacity(blurred_heights.len());

    // Find min/max in raw data
    let mut min_height: f64 = f64::MAX;
    let mut max_height: f64 = f64::MIN;
    let mut extreme_low_count = 0;
    let mut extreme_high_count = 0;

    for row in &blurred_heights {
        for &height in row {
            min_height = min_height.min(height);
            max_height = max_height.max(height);

            // Count extreme values that might indicate data issues
            if height < -1000.0 {
                extreme_low_count += 1;
            }
            if height > 10000.0 {
                extreme_high_count += 1;
            }
        }
    }

    eprintln!("Height data range: {min_height} to {max_height} m");
    if extreme_low_count > 0 {
        eprintln!(
            "WARNING: Found {extreme_low_count} pixels with extremely low elevations (< -1000m)"
        );
    }
    if extreme_high_count > 0 {
        eprintln!(
            "WARNING: Found {extreme_high_count} pixels with extremely high elevations (> 10000m)"
        );
    }

    let height_range: f64 = max_height - min_height;
    // Apply scale factor to height scaling
    let mut height_scale: f64 = BASE_HEIGHT_SCALE * scale.sqrt(); // sqrt to make height scaling less extreme
    let mut scaled_range: f64 = height_range * height_scale;

    // Adaptive scaling: ensure we don't exceed reasonable Y range
    let available_y_range = (MAX_Y - ground_level) as f64;
    let safety_margin = 0.9; // Use 90% of available range
    let max_allowed_range = available_y_range * safety_margin;

    if scaled_range > max_allowed_range {
        let adjustment_factor = max_allowed_range / scaled_range;
        height_scale *= adjustment_factor;
        scaled_range = height_range * height_scale;
        eprintln!(
            "Height range too large, applying scaling adjustment factor: {adjustment_factor:.3}"
        );
        eprintln!("Adjusted scaled range: {scaled_range:.1} blocks");
    }

    // Convert to scaled Minecraft Y coordinates
    for row in blurred_heights {
        let mc_row: Vec<i32> = row
            .iter()
            .map(|&h| {
                // Scale the height differences
                let relative_height: f64 = (h - min_height) / height_range;
                let scaled_height: f64 = relative_height * scaled_range;
                // With terrain enabled, ground_level is used as the MIN_Y for terrain
                ((ground_level as f64 + scaled_height).round() as i32).clamp(ground_level, MAX_Y)
            })
            .collect();
        mc_heights.push(mc_row);
    }

    let mut min_block_height: i32 = i32::MAX;
    let mut max_block_height: i32 = i32::MIN;
    for row in &mc_heights {
        for &height in row {
            min_block_height = min_block_height.min(height);
            max_block_height = max_block_height.max(height);
        }
    }
    eprintln!("Minecraft height data range: {min_block_height} to {max_block_height} blocks");

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

    // Apply blur
    let mut blurred: Vec<Vec<f64>> = heights.to_owned();

    // Horizontal pass
    for row in blurred.iter_mut() {
        let mut temp: Vec<f64> = row.clone();
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
        *row = temp;
    }

    // Vertical pass
    let height: usize = blurred.len();
    let width: usize = blurred[0].len();
    for x in 0..width {
        let temp: Vec<_> = blurred
            .iter()
            .take(height)
            .map(|row: &Vec<f64>| row[x])
            .collect();

        for (y, row) in blurred.iter_mut().enumerate().take(height) {
            let mut sum: f64 = 0.0;
            let mut weight_sum: f64 = 0.0;
            for (j, k) in kernel.iter().enumerate() {
                let idx: i32 = y as i32 + j as i32 - kernel_size as i32 / 2;
                if idx >= 0 && idx < height as i32 {
                    sum += temp[idx as usize] * k;
                    weight_sum += k;
                }
            }
            row[x] = sum / weight_sum;
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

    // Sort to find percentiles
    all_heights.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let len = all_heights.len();

    // Use 1st and 99th percentiles to define reasonable bounds
    let p1_idx = (len as f64 * 0.01) as usize;
    let p99_idx = (len as f64 * 0.99) as usize;
    let min_reasonable = all_heights[p1_idx];
    let max_reasonable = all_heights[p99_idx];

    eprintln!("Filtering outliers outside range: {min_reasonable:.1}m to {max_reasonable:.1}m");

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
        eprintln!("Filtered {outliers_filtered} elevation outliers, interpolating replacements...");
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
