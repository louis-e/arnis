use crate::args::Args;
use crate::coordinate_system::{cartesian::XZPoint, geographic::LLBBox};
use crate::elevation_data::{fetch_elevation_data, ElevationData};
use crate::land_cover::{self, LandCoverData};
use crate::progress::emit_gui_progress_update;
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use colored::Colorize;
use image::{Rgb, RgbImage};

/// Parameters describing the inverse-rotation needed to check whether a world
/// coordinate falls inside the original (pre-rotation) bounding box.
#[derive(Clone)]
pub struct RotationMask {
    /// Center of rotation (world coordinates)
    pub cx: f64,
    pub cz: f64,
    /// sin/cos of the *negative* angle (inverse rotation)
    pub neg_sin: f64,
    pub cos: f64,
    /// Original axis-aligned bounding box before rotation
    pub orig_min_x: i32,
    pub orig_max_x: i32,
    pub orig_min_z: i32,
    pub orig_max_z: i32,
}

/// Represents terrain data, land cover classification, and elevation settings
#[derive(Clone)]
pub struct Ground {
    pub elevation_enabled: bool,
    ground_level: i32,
    elevation_data: Option<ElevationData>,
    land_cover: Option<LandCoverData>,
    /// When set, coordinates outside the rotated original bbox are skipped.
    rotation_mask: Option<RotationMask>,
}

impl Ground {
    pub fn new_flat(ground_level: i32) -> Self {
        Self {
            elevation_enabled: false,
            ground_level,
            elevation_data: None,
            land_cover: None,
            rotation_mask: None,
        }
    }

    pub fn new_enabled(
        bbox: &LLBBox,
        scale: f64,
        ground_level: i32,
        fetch_land_cover: bool,
        disable_height_limit: bool,
    ) -> Self {
        match fetch_elevation_data(bbox, scale, ground_level, disable_height_limit) {
            Ok(elevation_data) => {
                // Fetch land cover data with the same grid dimensions as elevation
                let land_cover = if fetch_land_cover {
                    let lc = land_cover::fetch_land_cover_data(
                        bbox,
                        elevation_data.width,
                        elevation_data.height,
                    );
                    if lc.is_some() {
                        println!("Land cover data loaded successfully");
                    } else {
                        eprintln!(
                            "Warning: Land cover data unavailable, using default ground blocks"
                        );
                    }
                    lc
                } else {
                    None
                };

                Self {
                    elevation_enabled: true,
                    ground_level,
                    elevation_data: Some(elevation_data),
                    land_cover,
                    rotation_mask: None,
                }
            }
            Err(e) => {
                eprintln!("Failed to fetch elevation data: {}", e);
                #[cfg(feature = "gui")]
                send_log(
                    LogLevel::Warning,
                    "Elevation unavailable, using flat ground",
                );
                // Graceful fallback: disable elevation and keep provided ground_level
                Self {
                    elevation_enabled: false,
                    ground_level,
                    elevation_data: None,
                    land_cover: None,
                    rotation_mask: None,
                }
            }
        }
    }

    /// Returns whether land cover data is available
    #[inline(always)]
    pub fn has_land_cover(&self) -> bool {
        self.land_cover.is_some()
    }

    /// Returns the ESA WorldCover land cover class at the given coordinates.
    /// Returns 0 if land cover data is not available.
    #[inline(always)]
    pub fn cover_class(&self, coord: XZPoint) -> u8 {
        if let Some(ref lc) = self.land_cover {
            let x_ratio = (coord.x as f64 / lc.width as f64).clamp(0.0, 1.0);
            let z_ratio = (coord.z as f64 / lc.height as f64).clamp(0.0, 1.0);
            let x = ((x_ratio * (lc.width - 1) as f64).round() as usize).min(lc.width - 1);
            let z = ((z_ratio * (lc.height - 1) as f64).round() as usize).min(lc.height - 1);
            lc.grid[z][x]
        } else {
            0
        }
    }

    /// Returns the water distance-to-shore value at the given coordinates.
    /// 0 = non-water, 1 = shore, 2+ = progressively deeper water.
    #[inline(always)]
    pub fn water_distance(&self, coord: XZPoint) -> u8 {
        if let Some(ref lc) = self.land_cover {
            let x_ratio = (coord.x as f64 / lc.width as f64).clamp(0.0, 1.0);
            let z_ratio = (coord.z as f64 / lc.height as f64).clamp(0.0, 1.0);
            let x = ((x_ratio * (lc.width - 1) as f64).round() as usize).min(lc.width - 1);
            let z = ((z_ratio * (lc.height - 1) as f64).round() as usize).min(lc.height - 1);
            lc.water_distance[z][x]
        } else {
            0
        }
    }

    /// Computes terrain slope at the given coordinates.
    ///
    /// Slope is the difference between the maximum and minimum elevation of
    /// 4 cardinal neighbors sampled at a step distance. Higher values indicate
    /// steeper terrain.
    ///
    /// Returns 0 if elevation data is not available.
    #[inline(always)]
    pub fn slope(&self, coord: XZPoint) -> i32 {
        if !self.elevation_enabled {
            return 0;
        }

        const STEP: i32 = 4;
        let east = self.level(XZPoint::new(coord.x + STEP, coord.z));
        let west = self.level(XZPoint::new(coord.x - STEP, coord.z));
        let north = self.level(XZPoint::new(coord.x, coord.z - STEP));
        let south = self.level(XZPoint::new(coord.x, coord.z + STEP));

        let max_val = east.max(west).max(north).max(south);
        let min_val = east.min(west).min(north).min(south);
        max_val - min_val
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

    /// Replace the elevation grid with new rotated/transformed data.
    /// Used by the rotation operator to update elevation after rotating.
    pub fn set_elevation_data(&mut self, heights: Vec<Vec<i32>>, width: usize, height: usize) {
        if let Some(ref mut data) = self.elevation_data {
            data.heights = heights;
            data.width = width;
            data.height = height;
        }
    }

    /// Replace the land-cover grids with new rotated/transformed data.
    /// Used by the rotation operator to keep land cover aligned with elevation.
    pub fn set_land_cover_data(
        &mut self,
        grid: Vec<Vec<u8>>,
        water_distance: Vec<Vec<u8>>,
        width: usize,
        height: usize,
    ) {
        if let Some(ref mut lc) = self.land_cover {
            lc.grid = grid;
            lc.water_distance = water_distance;
            lc.width = width;
            lc.height = height;
        }
    }

    /// Store rotation parameters so we can mask out-of-bounds blocks later.
    pub fn set_rotation_mask(&mut self, mask: RotationMask) {
        self.rotation_mask = Some(mask);
    }

    /// Returns `true` if the coordinate is inside the rotated original bbox.
    /// When no rotation was applied, always returns `true`.
    #[inline(always)]
    pub fn is_in_rotated_bounds(&self, x: i32, z: i32) -> bool {
        let mask = match self.rotation_mask {
            Some(ref m) => m,
            None => return true,
        };
        // Inverse-rotate (x, z) back to original space
        let dx = x as f64 - mask.cx;
        let dz = z as f64 - mask.cz;
        let orig_x = dx * mask.cos + dz * mask.neg_sin + mask.cx;
        let orig_z = -dx * mask.neg_sin + dz * mask.cos + mask.cz;
        // Allow a tiny tolerance so points that land infinitesimally outside the
        // integer bbox due to floating-point rounding are still considered inside.
        const EPSILON: f64 = 1.0e-9;
        orig_x >= mask.orig_min_x as f64 - EPSILON
            && orig_x <= mask.orig_max_x as f64 + EPSILON
            && orig_z >= mask.orig_min_z as f64 - EPSILON
            && orig_z <= mask.orig_max_z as f64 + EPSILON
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
        emit_gui_progress_update(14.0, "Fetching elevation...");
        let ground = Ground::new_enabled(
            &args.bbox,
            args.scale,
            args.ground_level,
            args.land_cover,
            args.disable_height_limit,
        );
        if args.debug {
            ground.save_debug_image("elevation_debug");
        }
        return ground;
    }
    Ground::new_flat(args.ground_level)
}
