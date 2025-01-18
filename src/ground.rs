use crate::cartesian::XZPoint;
use image::{RgbImage, Rgb};
use reqwest;

/// Minimum Y coordinate in Minecraft (bedrock level)
const MIN_Y: i32 = -62;
/// Maximum Y coordinate in Minecraft (build height limit)
const MAX_Y: i32 = 256;
/// Scale factor for converting real elevation to Minecraft heights
const DEFAULT_HEIGHT_SCALE: f64 = 0.6; //0.3
/// Mapbox API access token for terrain data
const MAPBOX_PUBKEY: &str = "";
/// Minimum zoom level for terrain tiles
const MIN_ZOOM: u8 = 10;
/// Maximum zoom level for terrain tiles
const MAX_ZOOM: u8 = 15;

/// Represents terrain data and elevation settings
pub struct Ground {
    elevation_enabled: bool,
    ground_level: i32,
    elevation_data: Option<ElevationData>
}

/// Holds processed elevation data and metadata
#[derive(Clone)]
struct ElevationData {
    /// Height values in Minecraft Y coordinates
    heights: Vec<Vec<i32>>,
    /// Geographic bounds (min_lat, min_lng, max_lat, max_lng)
    bounds: (f64, f64, f64, f64),
    /// Tile bounds (min_lat, min_lng, max_lat, max_lng)
    tile_bounds: (f64, f64, f64, f64),
    /// Width of the elevation grid
    width: usize,
    /// Height of the elevation grid
    height: usize,
}

impl Ground {
    pub fn new(elevation_enabled: bool, ground_level: i32, bbox: Option<&str>) -> Self {
        let elevation_data = if elevation_enabled && bbox.is_some() {
            match Self::fetch_elevation_data(bbox.unwrap()) {
                Ok(data) => {
                    Self::save_debug_image(&data.heights, "elevation_debug", data.bounds);
                    Some(data)
                }
                Err(e) => {
                    eprintln!("Warning: Failed to fetch elevation data: {}", e);
                    None
                }
            }
        } else {
            None
        };

        Self {
            elevation_enabled,
            ground_level,
            elevation_data,
        }
    }

    /// Returns the ground level at the given coordinates
    #[inline(always)]
    pub fn level(&self, coord: XZPoint) -> i32 {
        if !self.elevation_enabled || self.elevation_data.is_none() {
            return self.ground_level;
        }

        let data = self.elevation_data.as_ref().unwrap();
        let (x_ratio, z_ratio) = self.get_data_coordinates(coord, data);
        self.interpolate_height(x_ratio, z_ratio, data)
    }

    #[inline(always)]
    pub fn min_level<I: Iterator<Item = XZPoint>>(&self, coords: I) -> Option<i32> {
        if !self.elevation_enabled {
            return Some(self.ground_level);
        }
        coords.map(|c| self.level(c)).min()
    }

    #[inline(always)]
    pub fn max_level<I: Iterator<Item = XZPoint>>(&self, coords: I) -> Option<i32> {
        if !self.elevation_enabled {
            return Some(self.ground_level);
        }
        coords.map(|c| self.level(c)).max()
    }

    /// Converts game coordinates to elevation data coordinates
    fn get_data_coordinates(&self, coord: XZPoint, data: &ElevationData) -> (f64, f64) {
        let x_ratio = coord.x as f64 / data.width as f64;
        let z_ratio = coord.z as f64 / data.height as f64;
        (x_ratio.clamp(0.0, 1.0), z_ratio.clamp(0.0, 1.0))
    }

    /// Interpolates height value from the elevation grid
    fn interpolate_height(&self, x_ratio: f64, z_ratio: f64, data: &ElevationData) -> i32 {
        let x = ((x_ratio * (data.width - 1) as f64).round() as usize).min(data.width - 1);
        let z = ((z_ratio * (data.height - 1) as f64).round() as usize).min(data.height - 1);
        data.heights[z][x]
    }

    /// Calculates appropriate zoom level for the given bounding box
    fn calculate_zoom_level(bbox: (f64, f64, f64, f64)) -> u8 {
        let lat_diff = (bbox.2 - bbox.0).abs();
        let lng_diff = (bbox.3 - bbox.1).abs();
        let max_diff = lat_diff.max(lng_diff);
        let zoom = (-max_diff.log2() + 20.0) as u8;
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    }

    fn lat_lng_to_tile(lat: f64, lng: f64, zoom: u8) -> (u32, u32) {
        let lat_rad = lat.to_radians();
        let n = 2.0_f64.powi(zoom as i32);
        let x = ((lng + 180.0) / 360.0 * n).floor() as u32;
        let y = ((1.0 - lat_rad.tan().asinh() / std::f64::consts::PI) / 2.0 * n).floor() as u32;
        (x, y)
    }

    fn tile_to_lat_lng(x: u32, y: u32, zoom: u8) -> (f64, f64) {
        let n = 2.0_f64.powi(zoom as i32);
        let lon_deg = x as f64 / n * 360.0 - 180.0;
        let lat_rad = (std::f64::consts::PI * (1.0 - 2.0 * y as f64 / n)).sinh().atan();
        (lat_rad.to_degrees(), lon_deg)
    }

    fn fetch_elevation_data(bbox_str: &str) -> Result<ElevationData, Box<dyn std::error::Error>> {
        let coords: Vec<f64> = bbox_str
            .split_whitespace()
            .map(|s| s.parse::<f64>())
            .collect::<Result<Vec<f64>, _>>()?;

        let (min_lat, min_lng, max_lat, max_lng) = (coords[0], coords[1], coords[2], coords[3]);
        
        // Use OSM parser's scale calculation
        let (scale_factor_z, scale_factor_x) = crate::osm_parser::geo_distance(
            min_lat, max_lat, min_lng, max_lng
        );

        // Calculate zoom and tiles
        let zoom = Self::calculate_zoom_level((min_lat, min_lng, max_lat, max_lng));
        let tiles = Self::get_tile_coordinates(min_lat, min_lng, max_lat, max_lng, zoom);

        // Calculate tile boundaries
        let x_min = tiles.iter().map(|(x, _)| x).min().unwrap();
        let x_max = tiles.iter().map(|(x, _)| x).max().unwrap();
        let y_min = tiles.iter().map(|(_, y)| y).min().unwrap();
        let y_max = tiles.iter().map(|(_, y)| y).max().unwrap();
        
        let (tile_min_lat, tile_min_lng) = Self::tile_to_lat_lng(*x_min, *y_max, zoom);
        let (tile_max_lat, tile_max_lng) = Self::tile_to_lat_lng(*x_max + 1, *y_min, zoom);

        // Match grid dimensions with Minecraft world size
        let grid_width = scale_factor_x.round() as usize;
        let grid_height = scale_factor_z.round() as usize;
        
        // Calculate total tile dimensions
        let total_tile_width = (x_max - x_min + 1) * 256;
        let total_tile_height = (y_max - y_min + 1) * 256;

        // Calculate scaling factors to match the desired grid dimensions
        let x_scale = grid_width as f64 / total_tile_width as f64;
        let y_scale = grid_height as f64 / total_tile_height as f64;
        
        // Initialize height grid with proper dimensions
        let mut height_grid = vec![vec![f64::NAN; grid_width]; grid_height];

        let client = reqwest::blocking::Client::new();
        let access_token = MAPBOX_PUBKEY;

        // Fetch and process each tile
        for (tile_x, tile_y) in &tiles {
            let url = format!(
                "https://api.mapbox.com/v4/mapbox.terrain-rgb/{}/{}/{}.pngraw?access_token={}",
                zoom, tile_x, tile_y, access_token
            );

            let response = client.get(&url).send()?;
            let img = image::load_from_memory(&response.bytes()?)?;
            let rgb_img = img.to_rgb8();

            // Calculate position in the scaled grid
            let base_x = ((*tile_x - x_min) * 256) as f64;
            let base_y = ((*tile_y - y_min) * 256) as f64;

            // Process tile data with scaling
            for (y, row) in rgb_img.rows().enumerate() {
                for (x, pixel) in row.enumerate() {
                    let scaled_x = ((base_x + x as f64) * x_scale) as usize;
                    let scaled_y = ((base_y + y as f64) * y_scale) as usize;

                    if scaled_y >= grid_height || scaled_x >= grid_width {
                        continue;
                    }

                    let height = -10000.0 + ((pixel[0] as f64 * 256.0 * 256.0 
                        + pixel[1] as f64 * 256.0 
                        + pixel[2] as f64) * 0.1);
                    
                    height_grid[scaled_y][scaled_x] = height;
                }
            }
        }

        // Fill in any NaN values by interpolating from nearest valid values
        Self::fill_nan_values(&mut height_grid);

        // Continue with the existing blur and conversion to Minecraft heights...
        let blurred_heights = Self::apply_gaussian_blur(&height_grid, 1.0);

        let mut mc_heights = Vec::with_capacity(blurred_heights.len());
        
        // Find min/max in raw data
        let mut min_height = f64::MAX;
        let mut max_height = f64::MIN;
        for row in &blurred_heights {
            for &height in row {
                min_height = min_height.min(height);
                max_height = max_height.max(height);
            }
        }

        let height_range = max_height - min_height;
        let scaled_range = height_range * DEFAULT_HEIGHT_SCALE;

        // Convert to scaled Minecraft Y coordinates
        for row in blurred_heights {
            let mc_row: Vec<i32> = row.iter()
                .map(|&h| {
                    // Scale the height differences
                    let relative_height = (h - min_height) / height_range;
                    let scaled_height = relative_height * scaled_range;
                    ((MIN_Y as f64 + scaled_height).round() as i32).clamp(MIN_Y, MAX_Y)
                })
                .collect();
            mc_heights.push(mc_row);
        }

        Ok(ElevationData {
            heights: mc_heights,
            bounds: (min_lat, min_lng, max_lat, max_lng),
            tile_bounds: (tile_min_lat, tile_min_lng, tile_max_lat, tile_max_lng),
            width: grid_width,
            height: grid_height,
        })
    }

    fn get_tile_coordinates(min_lat: f64, min_lng: f64, max_lat: f64, max_lng: f64, zoom: u8) 
        -> Vec<(u32, u32)> {
        // Convert lat/lng to tile coordinates
        let (x1, y1) = Self::lat_lng_to_tile(min_lat, min_lng, zoom);
        let (x2, y2) = Self::lat_lng_to_tile(max_lat, max_lng, zoom);

        let mut tiles = Vec::new();
        for x in x1.min(x2)..=x1.max(x2) {
            for y in y1.min(y2)..=y1.max(y2) {
                tiles.push((x, y));
            }
        }
        tiles
    }

    fn apply_gaussian_blur(heights: &Vec<Vec<f64>>, sigma: f64) -> Vec<Vec<f64>> {
        let kernel_size = (sigma * 3.0).ceil() as usize * 2 + 1;
        let kernel = Self::create_gaussian_kernel(kernel_size, sigma);

        // Apply blur
        let mut blurred = heights.clone();
        
        // Horizontal pass
        for row in blurred.iter_mut() {
            let mut temp = row.clone();
            for i in 0..row.len() {
                let mut sum = 0.0;
                let mut weight_sum = 0.0;
                for (j, k) in kernel.iter().enumerate() {
                    let idx = i as i32 + j as i32 - kernel_size as i32 / 2;
                    if idx >= 0 && idx < row.len() as i32 {
                        sum += row[idx as usize] * k;
                        weight_sum += k;
                    }
                }
                temp[i] = sum / weight_sum;
            }
            *row = temp;
        }

        // Vertical pass
        let height = blurred.len();
        let width = blurred[0].len();
        for x in 0..width {
            let mut temp = Vec::new();
            for y in 0..height {
                temp.push(blurred[y][x]);
            }
            
            for y in 0..height {
                let mut sum = 0.0;
                let mut weight_sum = 0.0;
                for (j, k) in kernel.iter().enumerate() {
                    let idx = y as i32 + j as i32 - kernel_size as i32 / 2;
                    if idx >= 0 && idx < height as i32 {
                        sum += temp[idx as usize] * k;
                        weight_sum += k;
                    }
                }
                blurred[y][x] = sum / weight_sum;
            }
        }

        blurred
    }

    fn create_gaussian_kernel(size: usize, sigma: f64) -> Vec<f64> {
        let mut kernel = vec![0.0; size];
        let center = size as f64 / 2.0;
        
        for i in 0..size {
            let x = i as f64 - center;
            kernel[i] = (-x * x / (2.0 * sigma * sigma)).exp();
        }
        
        let sum: f64 = kernel.iter().sum();
        for k in kernel.iter_mut() {
            *k /= sum;
        }
        
        kernel
    }

    fn fill_nan_values(height_grid: &mut Vec<Vec<f64>>) {
        let height = height_grid.len();
        let width = height_grid[0].len();
        
        let mut changes_made = true;
        while changes_made {
            changes_made = false;
            
            for y in 0..height {
                for x in 0..width {
                    if height_grid[y][x].is_nan() {
                        let mut sum = 0.0;
                        let mut count = 0;
                        
                        // Check neighboring cells
                        for dy in -1..=1 {
                            for dx in -1..=1 {
                                let ny = y as i32 + dy;
                                let nx = x as i32 + dx;
                                
                                if ny >= 0 && ny < height as i32 && 
                                   nx >= 0 && nx < width as i32 {
                                    let val = height_grid[ny as usize][nx as usize];
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

    fn save_debug_image(heights: &Vec<Vec<i32>>, filename: &str, bounds: (f64, f64, f64, f64)) {
        if heights.is_empty() || heights[0].is_empty() {
            return;
        }

        let height = heights.len();
        let width = heights[0].len();
        let mut img = RgbImage::new(width as u32, height as u32);
        
        let mut min_height = i32::MAX;
        let mut max_height = i32::MIN;
        
        for row in heights {
            for &h in row {
                min_height = min_height.min(h);
                max_height = max_height.max(h);
            }
        }

        for (y, row) in heights.iter().enumerate() {
            for (x, &h) in row.iter().enumerate() {
                let normalized = (((h - min_height) as f64 
                    / (max_height - min_height) as f64) * 255.0) as u8;
                img.put_pixel(x as u32, y as u32, Rgb([normalized, normalized, normalized]));
            }
        }

        // Ensure filename has .png extension
        let filename = if !filename.ends_with(".png") {
            format!("{}.png", filename)
        } else {
            filename.to_string()
        };

        if let Err(e) = img.save(&filename) {
            eprintln!("Failed to save debug image: {}", e);
        }
    }
}