use crate::coordinate_system::{
    geographic::LLBBox,
    transformation::geo_distance,
};
use image::Rgb;
use std::path::Path;

/// Maximum Y coordinate in Minecraft (build height limit)
const MAX_Y: i32 = 319;
/// Scale factor for converting real elevation to Minecraft heights
const BASE_HEIGHT_SCALE: f64 = 0.72;
/// Default Mapbox API access token for terrain data
const MAPBOX_PUBKEY: &str =
    "pk.eyJ1IjoibG91aXMtZSIsImEiOiJjbWF0cWlycjEwYWNvMmtxeHFwdDQ5NnJoIn0.6A0AKg0iucvoGhYuCkeOjA";
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

pub fn fetch_elevation_data(
    bbox: &LLBBox,
    scale: f64,
    ground_level: i32,
    mapbox_access_token: &Option<String>,
) -> Result<ElevationData, Box<dyn std::error::Error>> {
    let default_mapbox_access_token = MAPBOX_PUBKEY.to_string();
    // Use OSM parser's scale calculation and apply user scale factor
    let (scale_factor_z, scale_factor_x) = geo_distance(bbox.min(), bbox.max());
    let scale_factor_x: f64 = scale_factor_x * scale;
    let scale_factor_z: f64 = scale_factor_z * scale;

    // Calculate zoom and tiles
    let zoom: u8 = calculate_zoom_level(bbox);
    let tiles: Vec<(u32, u32)> = get_tile_coordinates(bbox, zoom);

    // Calculate tile boundaries
    let x_min: &u32 = tiles.iter().map(|(x, _)| x).min().unwrap();
    let x_max: &u32 = tiles.iter().map(|(x, _)| x).max().unwrap();
    let y_min: &u32 = tiles.iter().map(|(_, y)| y).min().unwrap();
    let y_max: &u32 = tiles.iter().map(|(_, y)| y).max().unwrap();

    // Match grid dimensions with Minecraft world size
    let grid_width: usize = scale_factor_x.round() as usize;
    let grid_height: usize = scale_factor_z.round() as usize;

    // Calculate total tile dimensions
    let total_tile_width: u32 = (x_max - x_min + 1) * 256;
    let total_tile_height: u32 = (y_max - y_min + 1) * 256;

    // Calculate scaling factors to match the desired grid dimensions
    let x_scale: f64 = grid_width as f64 / total_tile_width as f64;
    let y_scale: f64 = grid_height as f64 / total_tile_height as f64;

    // Initialize height grid with proper dimensions
    let mut height_grid: Vec<Vec<f64>> = vec![vec![f64::NAN; grid_width]; grid_height];

    let client: reqwest::blocking::Client = reqwest::blocking::Client::new();

    let tile_cache_dir = Path::new("./terrain-tile-cache");
    if !tile_cache_dir.exists() {
        std::fs::create_dir_all(tile_cache_dir)?;
    }

    // Fetch and process each tile
    for (tile_x, tile_y) in &tiles {
        // Check if tile is already cached
        let tile_path = tile_cache_dir.join(format!("z{zoom}_x{tile_x}_y{tile_y}.png"));

        let rgb_img: image::ImageBuffer<Rgb<u8>, Vec<u8>> = if tile_path.exists() {
            println!(
                "Loading tile x={tile_x},y={tile_y},z={zoom} from {}",
                tile_path.display()
            );
            let img: image::DynamicImage = image::open(&tile_path)?;
            img.to_rgb8()
        } else {
            let mapbox_access_token = mapbox_access_token
                .as_ref()
                .unwrap_or(&default_mapbox_access_token);
            println!("Fetching tile x={tile_x},y={tile_y},z={zoom} from Mapbox API");
            let url: String = format!(
                "https://api.mapbox.com/v4/mapbox.terrain-rgb/{}/{}/{}.pngraw?access_token={}",
                zoom, tile_x, tile_y, mapbox_access_token
            );

            let response: reqwest::blocking::Response = client.get(&url).send()?;
            response.error_for_status_ref()?;
            let bytes = response.bytes()?;
            std::fs::write(&tile_path, &bytes)?;
            let img: image::DynamicImage = image::load_from_memory(&bytes)?;
            img.to_rgb8()
        };

        // Calculate position in the scaled grid
        let base_x: f64 = ((*tile_x - x_min) * 256) as f64;
        let base_y: f64 = ((*tile_y - y_min) * 256) as f64;

        // Process tile data with scaling
        for (y, row) in rgb_img.rows().enumerate() {
            for (x, pixel) in row.enumerate() {
                let scaled_x: usize = ((base_x + x as f64) * x_scale) as usize;
                let scaled_y: usize = ((base_y + y as f64) * y_scale) as usize;

                if scaled_y >= grid_height || scaled_x >= grid_width {
                    continue;
                }

                let height: f64 = -10000.0
                    + ((pixel[0] as f64 * 256.0 * 256.0
                        + pixel[1] as f64 * 256.0
                        + pixel[2] as f64)
                        * 0.1);

                height_grid[scaled_y][scaled_x] = height;
            }
        }
    }

    // Fill in any NaN values by interpolating from nearest valid values
    fill_nan_values(&mut height_grid);

    // Continue with the existing blur and conversion to Minecraft heights...
    let blurred_heights: Vec<Vec<f64>> = apply_gaussian_blur(&height_grid, 1.0);

    let mut mc_heights: Vec<Vec<i32>> = Vec::with_capacity(blurred_heights.len());

    // Find min/max in raw data
    let mut min_height: f64 = f64::MAX;
    let mut max_height: f64 = f64::MIN;
    for row in &blurred_heights {
        for &height in row {
            min_height = min_height.min(height);
            max_height = max_height.max(height);
        }
    }

    eprintln!("Height data range: {min_height} to {max_height} m");

    let height_range: f64 = max_height - min_height;
    // Apply scale factor to height scaling
    let height_scale: f64 = BASE_HEIGHT_SCALE * scale.sqrt(); // sqrt to make height scaling less extreme
    let scaled_range: f64 = height_range * height_scale;

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
