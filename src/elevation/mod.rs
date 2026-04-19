pub mod cache;
pub mod postprocess;
pub mod provider;
pub mod providers;
pub mod selector;

use crate::{
    coordinate_system::{geographic::LLBBox, transformation::geo_distance},
    land_cover::LandCoverData,
    progress::emit_gui_progress_update,
};
use postprocess::{
    apply_land_cover_repair, fill_nan_values, filter_elevation_outliers, repair_terrain_anomalies,
    scale_to_minecraft,
};
use provider::ElevationProvider;
use selector::select_provider;

/// Holds processed elevation data and metadata
#[derive(Clone)]
pub struct ElevationData {
    /// Height values in Minecraft Y coordinates (as f64, rounded to i32 at final block placement)
    pub(crate) heights: Vec<Vec<f64>>,
    /// Width of the elevation grid (may be smaller than world width due to capping)
    pub(crate) width: usize,
    /// Height of the elevation grid (may be smaller than world height due to capping)
    pub(crate) height: usize,
    /// Width of the world in blocks (used for coordinate mapping)
    pub(crate) world_width: usize,
    /// Height of the world in blocks (used for coordinate mapping)
    pub(crate) world_height: usize,
}

/// Maximum elevation grid dimension to request from providers.
/// WMS servers typically cap at 4096x4096. AWS tile-based providers handle
/// any size by downloading multiple tiles, but WMS providers would reject
/// oversized requests. The bilinear interpolation in ground.rs handles
/// upscaling to full block resolution.
pub const MAX_ELEVATION_GRID_DIM: usize = 4096;

/// Compute world and grid dimensions for the given bbox and scale.
///
/// Exposed so callers (e.g. `Ground::new_enabled`) can fetch land cover at the
/// same dimensions as the elevation grid before elevation fetch starts.
///
/// Returns `(world_width, world_height, grid_width, grid_height)`.
pub fn compute_grid_dims(bbox: &LLBBox, scale: f64) -> (usize, usize, usize, usize) {
    let (base_scale_z, base_scale_x) = geo_distance(bbox.min(), bbox.max());
    // Apply same floor() and scale operations as CoordTransformer.llbbox_to_xzbbox()
    let scale_factor_z: f64 = base_scale_z.floor() * scale;
    let scale_factor_x: f64 = base_scale_x.floor() * scale;
    // World block positions span 0..=scale_factor (inclusive), so there are
    // scale_factor+1 distinct positions.
    let world_width: usize = scale_factor_x as usize + 1;
    let world_height: usize = scale_factor_z as usize + 1;
    // Cap grid dimensions to avoid WMS server rejections.
    let grid_width: usize = world_width.clamp(2, MAX_ELEVATION_GRID_DIM);
    let grid_height: usize = world_height.clamp(2, MAX_ELEVATION_GRID_DIM);
    (world_width, world_height, grid_width, grid_height)
}

/// Fetch elevation data for the given bounding box.
///
/// Automatically selects the best available elevation provider for the region,
/// falling back to AWS Terrain Tiles for global coverage.
///
/// If `land_cover` is provided, applies land-cover-aware artifact repair
/// (water leveling, built-up smoothing) before scaling. This fixes LiDAR
/// classification errors at urban structures (tunnel portals, overpasses)
/// and coastal tile-boundary artifacts.
///
/// The returned ElevationData contains heights in Minecraft Y coordinates.
pub fn fetch_elevation_data(
    bbox: &LLBBox,
    scale: f64,
    ground_level: i32,
    disable_height_limit: bool,
    land_cover: Option<&mut LandCoverData>,
) -> Result<ElevationData, Box<dyn std::error::Error>> {
    let (world_width, world_height, grid_width, grid_height) = compute_grid_dims(bbox, scale);

    // Select the best provider for this region
    let provider = select_provider(bbox);
    let provider_name = provider.name();
    let is_fallback = provider_name == "aws";

    emit_gui_progress_update(16.0, "Fetching elevation...");

    // Fetch raw elevation data in meters, falling back to AWS on regional provider failure
    let raw = match provider.fetch_raw(bbox, grid_width, grid_height) {
        Ok(raw) if !is_fallback => {
            // Check if the regional provider returned mostly empty data (out-of-coverage area).
            // This catches cases where the provider's rectangular bbox over-claims coverage
            // (e.g., IGN France bbox covers Belgium, but returns no data for Belgian coordinates).
            let nan_ratio = compute_nan_ratio(&raw.heights_meters);
            if nan_ratio > 0.5 {
                eprintln!(
                    "Warning: Regional provider '{}' returned {:.0}% empty data. Falling back to AWS Terrain Tiles.",
                    provider_name, nan_ratio * 100.0
                );
                #[cfg(feature = "gui")]
                crate::telemetry::send_log(
                    crate::telemetry::LogLevel::Warning,
                    &format!(
                        "Regional provider '{}' returned mostly empty data, using AWS fallback.",
                        provider_name
                    ),
                );
                let fallback = providers::aws_terrain::AwsTerrain;
                fallback.fetch_raw(bbox, grid_width, grid_height)?
            } else {
                raw
            }
        }
        Ok(raw) => raw,
        Err(e) if !is_fallback => {
            eprintln!(
                "Warning: Regional provider '{}' failed: {}. Falling back to AWS Terrain Tiles.",
                provider_name, e
            );
            #[cfg(feature = "gui")]
            crate::telemetry::send_log(
                crate::telemetry::LogLevel::Warning,
                &format!(
                    "Regional elevation provider '{}' failed, using AWS fallback.",
                    provider_name
                ),
            );
            let fallback = providers::aws_terrain::AwsTerrain;
            emit_gui_progress_update(16.0, "Regional provider failed, fetching from AWS...");
            fallback.fetch_raw(bbox, grid_width, grid_height)?
        }
        Err(e) => return Err(e),
    };

    emit_gui_progress_update(17.0, "Processing elevation...");

    // Shared post-processing pipeline
    let mut height_grid = raw.heights_meters;
    filter_elevation_outliers(&mut height_grid);
    repair_terrain_anomalies(&mut height_grid);
    // Safety net: fill any remaining NaN from tile gaps or partial provider coverage
    fill_nan_values(&mut height_grid);

    // Land-cover-aware repair (targets urban LiDAR/DSM classification
    // errors and coastal tile-boundary artifacts; leaves natural terrain
    // untouched).
    //
    // Both scales are expressed in *meters* and converted to grid cells
    // via the actual meters-per-cell for this bbox/grid, so the smoothing
    // covers the same physical scale regardless of world size or provider
    // resolution.
    //
    // σ = 30 m for the built-up Gaussian: wide enough that a typical
    // 20 m-wide DSM artifact (tunnel portal, overpass, parking deck) is
    // reduced to a residual the user can't distinguish from one Minecraft
    // block. Hilly cities (SF, Pittsburgh) still keep their macro shape —
    // the kernel falls off long before a real urban slope does. On coarse
    // providers (AWS fallback when σ < 1.5 cells) the Gaussian pass is
    // skipped internally.
    //
    // 25 m coastal pull range: reaches far enough to cover DSM-captured
    // piers/warehouses at the shoreline, short enough to leave the actual
    // inland city alone.
    const BUILT_UP_SIGMA_M: f64 = 30.0;
    const COASTAL_PULL_M: f64 = 25.0;
    let (bbox_height_m, bbox_width_m) = geo_distance(bbox.min(), bbox.max());
    let m_per_cell = (bbox_width_m / grid_width as f64 + bbox_height_m / grid_height as f64) * 0.5;
    let (built_up_sigma_cells, coastal_pull_cells) = if m_per_cell > 0.0 {
        (
            BUILT_UP_SIGMA_M / m_per_cell,
            (COASTAL_PULL_M / m_per_cell).round() as u32,
        )
    } else {
        (0.0, 0)
    };

    if let Some(lc) = land_cover {
        apply_land_cover_repair(
            &mut height_grid,
            lc,
            built_up_sigma_cells,
            coastal_pull_cells,
        );
    }

    let mc_heights = scale_to_minecraft(&height_grid, scale, ground_level, disable_height_limit);

    // Log min/max block heights
    let mut min_block_height = f64::MAX;
    let mut max_block_height = f64::MIN;
    for row in &mc_heights {
        for &height in row {
            if height.is_finite() {
                min_block_height = min_block_height.min(height);
                max_block_height = max_block_height.max(height);
            }
        }
    }

    Ok(ElevationData {
        heights: mc_heights,
        width: grid_width,
        height: grid_height,
        world_width,
        world_height,
    })
}

/// Clean up old cached elevation tiles/files from all providers.
pub fn cleanup_old_cached_tiles() {
    cache::cleanup_old_cached_files();
}

/// Compute the fraction of NaN/non-finite values in a height grid (0.0 to 1.0).
fn compute_nan_ratio(heights: &[Vec<f64>]) -> f64 {
    let mut total = 0usize;
    let mut nan_count = 0usize;
    for row in heights {
        for &h in row {
            total += 1;
            if !h.is_finite() {
                nan_count += 1;
            }
        }
    }
    if total == 0 {
        return 1.0;
    }
    nan_count as f64 / total as f64
}
