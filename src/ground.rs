use crate::args::Args;
use crate::coordinate_system::{cartesian::XZPoint, geographic::LLBBox};
use crate::elevation_data::{fetch_elevation_data, ElevationData};
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use image::{Rgb, RgbImage};

/// Represents terrain data and elevation settings
#[derive(Clone)]
pub struct Ground {
    pub elevation_enabled: bool,
    ground_level: i32,
    elevation_data: Option<ElevationData>,
}

impl Ground {
    pub fn new_flat(ground_level: i32) -> Self {
        Self {
            elevation_enabled: false,
            ground_level,
            elevation_data: None,
        }
    }

    pub fn new_enabled(
        bbox: &LLBBox,
        scale: f64,
        ground_level: i32,
        mapbox_access_token: &Option<String>,
    ) -> Self {
        let elevation_data = fetch_elevation_data(bbox, scale, ground_level, mapbox_access_token)
            .expect("Failed to fetch elevation data");
        Self {
            elevation_enabled: true,
            ground_level,
            elevation_data: Some(elevation_data),
        }
    }

    /// Returns the ground level at the given coordinates
    #[inline(always)]
    pub fn level(&self, coord: XZPoint) -> i32 {
        if !self.elevation_enabled || self.elevation_data.is_none() {
            return self.ground_level;
        }

        let data: &ElevationData = self.elevation_data.as_ref().unwrap();
        let (x_ratio, z_ratio) = self.get_data_coordinates(coord, data);
        self.interpolate_height(x_ratio, z_ratio, data)
    }

    #[allow(unused)]
    #[inline(always)]
    pub fn min_level<I: Iterator<Item = XZPoint>>(&self, coords: I) -> Option<i32> {
        if !self.elevation_enabled {
            return Some(self.ground_level);
        }
        coords.map(|c: XZPoint| self.level(c)).min()
    }

    #[allow(unused)]
    #[inline(always)]
    pub fn max_level<I: Iterator<Item = XZPoint>>(&self, coords: I) -> Option<i32> {
        if !self.elevation_enabled {
            return Some(self.ground_level);
        }
        coords.map(|c: XZPoint| self.level(c)).max()
    }

    /// Converts game coordinates to elevation data coordinates
    #[inline(always)]
    fn get_data_coordinates(&self, coord: XZPoint, data: &ElevationData) -> (f64, f64) {
        let x_ratio: f64 = coord.x as f64 / data.width as f64;
        let z_ratio: f64 = coord.z as f64 / data.height as f64;
        (x_ratio.clamp(0.0, 1.0), z_ratio.clamp(0.0, 1.0))
    }

    /// Interpolates height value from the elevation grid
    #[inline(always)]
    fn interpolate_height(&self, x_ratio: f64, z_ratio: f64, data: &ElevationData) -> i32 {
        let x: usize = ((x_ratio * (data.width - 1) as f64).round() as usize).min(data.width - 1);
        let z: usize = ((z_ratio * (data.height - 1) as f64).round() as usize).min(data.height - 1);
        data.heights[z][x]
    }

    fn save_debug_image(&self, filename: &str) {
        let heights = &self
            .elevation_data
            .as_ref()
            .expect("Elevation data not available")
            .heights;
        if heights.is_empty() || heights[0].is_empty() {
            return;
        }

        let height: usize = heights.len();
        let width: usize = heights[0].len();
        let mut img: image::ImageBuffer<Rgb<u8>, Vec<u8>> =
            RgbImage::new(width as u32, height as u32);

        let mut min_height: i32 = i32::MAX;
        let mut max_height: i32 = i32::MIN;

        for row in heights {
            for &h in row {
                min_height = min_height.min(h);
                max_height = max_height.max(h);
            }
        }

        for (y, row) in heights.iter().enumerate() {
            for (x, &h) in row.iter().enumerate() {
                let normalized: u8 =
                    (((h - min_height) as f64 / (max_height - min_height) as f64) * 255.0) as u8;
                img.put_pixel(
                    x as u32,
                    y as u32,
                    Rgb([normalized, normalized, normalized]),
                );
            }
        }

        // Ensure filename has .png extension
        let filename: String = if !filename.ends_with(".png") {
            format!("{filename}.png")
        } else {
            filename.to_string()
        };

        if let Err(e) = img.save(&filename) {
            eprintln!("Failed to save debug image: {e}");
        }
    }
}

pub fn generate_ground_data(args: &Args) -> Ground {
    if args.terrain {
        println!("{} Fetching elevation...", "[3/7]".bold());
        emit_gui_progress_update(15.0, "Fetching elevation...");
        let ground = Ground::new_enabled(
            &args.bbox,
            args.scale,
            args.ground_level,
            &args.mapbox_access_token,
        );
        if args.debug {
            ground.save_debug_image("elevation_debug");
        }
        return ground;
    }
    Ground::new_flat(args.ground_level)
}
