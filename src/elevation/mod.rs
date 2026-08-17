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
use provider::{ElevationProvider, RawElevationGrid};
use selector::select_provider;
pub use selector::SourceMode;

/// Holds processed elevation data and metadata
#[derive(Clone)]
pub struct ElevationData {
    /// Height values in Minecraft Y coordinates.
    ///
    /// Stored as `f32` on purpose: heights are already rounded to integer
    /// block Ys at placement time, so the full f64 precision was wasted on a
    /// grid that can easily hit 10+ million cells on a city-sized bbox
    /// (≈80 MB at f64, halved at f32). Postprocess still runs in f64 for
    /// numerical stability; the downcast happens once at construction.
    pub(crate) heights: Vec<Vec<f32>>,
    /// Width of the elevation grid (may be smaller than world width due to capping)
    pub(crate) width: usize,
    /// Height of the elevation grid (may be smaller than world height due to capping)
    pub(crate) height: usize,
    /// Width of the world in blocks (used for coordinate mapping)
    pub(crate) world_width: usize,
    /// Height of the world in blocks (used for coordinate mapping)
    pub(crate) world_height: usize,
    /// Affine params of the metre->Minecraft-Y scaling: minimum source height
    /// in metres and the blocks-per-metre slope. Used to map a real-world
    /// elevation (e.g. the snow line) to a Minecraft Y threshold.
    pub(crate) min_height_m: f64,
    pub(crate) blocks_per_meter: f64,
    /// Terrain base actually used: the requested ground level, or lower if the relief
    /// needed the extended floor. Every consumer of the affine must use this, not args.
    pub(crate) ground_level: i32,
}

/// Maximum elevation grid dimension requested from providers per axis.
/// Providers fetch at their own tile granularity and resample onto the
/// requested grid, so this cap only bounds the output grid itself.
///
/// Chosen value 16384 covers bboxes up to ~256 km² at the default
/// `--scale 1.0` without losing native resolution. The precision
/// boundary rose from ~16.8 km² (4096²) → ~64 km² (8000²) → ~268 km²
/// (16384²) across these revisions. Above 16384 per axis the grid is
/// capped and block-level elevation is filled via bilinear interpolation
/// — terrain remains generated, just with sub-native sampling.
///
/// Memory note: a full 16384 × 16384 f64 grid is ~2 GB; with the
/// water_blend_grid and a snapshot during repair we can peak around
/// 6 GB for the maximum case. Target deployment (MapSmith) has >20 GB
/// available. Typical user bboxes stay well below the cap.
pub const MAX_ELEVATION_GRID_DIM: usize = 16384;

/// Total elevation-grid cell budget. The per-axis cap alone is not enough: clamping each
/// axis independently changes the grid's aspect ratio, so a wide bbox ends up sampled more
/// coarsely along X than along Z and the terrain smears directionally. Budgeting total cells
/// and shrinking both axes by the same factor keeps sampling isotropic at the same memory.
pub const MAX_ELEVATION_GRID_CELLS: usize = MAX_ELEVATION_GRID_DIM * MAX_ELEVATION_GRID_DIM;

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

    // One elevation sample per block is the ideal: finer buys nothing (a block is the
    // smallest representable unit), coarser blurs the terrain. Only shrink below that when
    // the grid would breach a limit.
    let mut grid_width: usize = world_width.max(2);
    let mut grid_height: usize = world_height.max(2);
    let cells = grid_width as f64 * grid_height as f64;
    let budget_shrink = (cells / MAX_ELEVATION_GRID_CELLS as f64).sqrt();
    // A long thin bbox stays under the cell budget while still running far past the per-axis
    // cap, so both limits have to feed the same factor.
    let axis_shrink = grid_width.max(grid_height) as f64 / MAX_ELEVATION_GRID_DIM as f64;
    let shrink = budget_shrink.max(axis_shrink);
    if shrink > 1.0 {
        // Shrink both axes by the same factor so the sampling stays isotropic.
        grid_width = ((grid_width as f64 / shrink).floor() as usize).clamp(2, world_width.max(2));
        grid_height =
            ((grid_height as f64 / shrink).floor() as usize).clamp(2, world_height.max(2));
    }
    (world_width, world_height, grid_width, grid_height)
}

/// Fetch elevation data for the given bounding box.
///
/// Selects the best provider for the region, with a fetch-time fallback
/// chain: regional provider, then Mapterhorn, then AWS Terrain Tiles.
///
/// If `land_cover` is provided, applies land-cover-aware artifact repair
/// (water leveling, built-up smoothing) before scaling. This fixes LiDAR
/// classification errors at urban structures (tunnel portals, overpasses)
/// and coastal tile-boundary artifacts.
///
/// The returned ElevationData contains heights in Minecraft Y coordinates.
#[allow(clippy::too_many_arguments)]
pub fn fetch_elevation_data(
    bbox: &LLBBox,
    scale: f64,
    ground_level: i32,
    min_ground_level: i32,
    disable_height_limit: bool,
    extended_max_y: i32,
    land_cover: Option<&mut LandCoverData>,
    source_mode: SourceMode,
    benchmark: bool,
) -> Result<ElevationData, Box<dyn std::error::Error>> {
    let mut bench = crate::bench::Bench::new(benchmark);
    let (world_width, world_height, grid_width, grid_height) = compute_grid_dims(bbox, scale);

    // Fallback chain: selected provider, then Mapterhorn, then AWS.
    let provider = select_provider(bbox, source_mode);
    let mut chain: Vec<Box<dyn ElevationProvider>> = vec![provider];
    if chain[0].name() != "mapterhorn" && chain[0].name() != "aws" {
        chain.push(Box::new(providers::mapterhorn::Mapterhorn));
    }
    if chain[0].name() != "aws" {
        chain.push(Box::new(providers::aws_terrain::AwsTerrain));
    }

    emit_gui_progress_update(10.0, "Downloading data...");

    let raw = fetch_raw_with_fallback(&chain, bbox, grid_width, grid_height)?;

    bench.mark("elev_raw_fetch");
    emit_gui_progress_update(12.0, "Processing elevation...");

    // Shared post-processing pipeline
    let mut height_grid = raw.heights_meters;
    filter_elevation_outliers(&mut height_grid);
    bench.mark("elev_filter_outliers");
    repair_terrain_anomalies(&mut height_grid);
    bench.mark("elev_repair_anomalies");
    emit_gui_progress_update(14.0, "Processing elevation...");
    // Safety net: fill any remaining NaN from tile gaps or partial provider coverage
    fill_nan_values(&mut height_grid);
    bench.mark("elev_fill_nan");

    // Land-cover-aware repair: built-up Gaussian smoothing targets urban
    // LiDAR/DSM classification errors, coastal pull-down flattens the
    // shoreline cliff across all land classes.
    //
    // Both scales are in meters and converted to grid cells via the actual
    // meters-per-cell, so the smoothing covers the same physical scale
    // regardless of world size or provider resolution.
    //
    // σ = 30 m for the built-up Gaussian: wide enough that a typical
    // 20 m-wide DSM artifact (tunnel portal, overpass, parking deck) is
    // reduced to a residual indistinguishable from one Minecraft block.
    // Hilly cities (SF, Pittsburgh) still keep their macro shape — the
    // kernel falls off long before a real urban slope does. On coarse
    // providers (AWS fallback when σ < 1.5 cells) the Gaussian pass is
    // skipped internally.
    //
    // 25 m coastal pull range: short enough to leave the inland interior
    // alone, long enough that a 7-10 m urban embankment (Munich Isar,
    // Vienna Donaukanal) becomes a slope-tier-free ramp instead of a
    // cliff with stepped stone walls.
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
        // The land-cover Gaussian is the slowest elevation step on big areas;
        // animate the bar across 14->16% as it runs instead of freezing.
        apply_land_cover_repair(
            &mut height_grid,
            lc,
            built_up_sigma_cells,
            coastal_pull_cells,
            m_per_cell,
            &|f| emit_gui_progress_update(14.0 + f * 2.0, "Processing elevation..."),
        );
    }
    bench.mark("elev_landcover_repair");
    emit_gui_progress_update(16.0, "Processing elevation...");

    let (mc_heights, min_height_m, blocks_per_meter, effective_ground_level) = scale_to_minecraft(
        &height_grid,
        scale,
        ground_level,
        min_ground_level,
        disable_height_limit,
        extended_max_y,
    );
    bench.mark("elev_scale_to_mc");

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

    // Downcast the f64 postprocess output to the f32 storage format. One-time
    // cost paid here so the large grid sits at half the memory for the rest
    // of the generation run. NaN/infinity preservation is a requirement —
    // downstream `is_finite` checks rely on non-finite sentinels surviving.
    let mc_heights_f32: Vec<Vec<f32>> = mc_heights
        .into_iter()
        .map(|row| row.into_iter().map(|v| v as f32).collect())
        .collect();
    bench.mark("elev_downcast");
    emit_gui_progress_update(18.0, "Processing elevation...");

    Ok(ElevationData {
        heights: mc_heights_f32,
        width: grid_width,
        height: grid_height,
        world_width,
        world_height,
        min_height_m,
        blocks_per_meter,
        ground_level: effective_ground_level,
    })
}

/// Clean up old cached elevation tiles in the background, at most once per day.
pub fn cleanup_old_cached_tiles() {
    cache::spawn_throttled_cleanup();
}

/// Try each provider in `chain` until one delivers usable data. Non-final
/// providers are skipped on error or mostly empty data (over-claiming bboxes).
fn fetch_raw_with_fallback(
    chain: &[Box<dyn ElevationProvider>],
    bbox: &LLBBox,
    grid_width: usize,
    grid_height: usize,
) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
    let last = chain.len() - 1;
    for (i, provider) in chain.iter().enumerate() {
        let name = provider.name();
        match provider.fetch_raw(bbox, grid_width, grid_height) {
            Ok(raw) => {
                if i < last {
                    let nan_ratio = compute_nan_ratio(&raw.heights_meters);
                    if nan_ratio > 0.5 {
                        eprintln!(
                            "Warning: Elevation provider '{}' returned {:.0}% empty data. Falling back to '{}'.",
                            name,
                            nan_ratio * 100.0,
                            chain[i + 1].name()
                        );
                        #[cfg(feature = "gui")]
                        crate::telemetry::send_log(
                            crate::telemetry::LogLevel::Warning,
                            &format!(
                                "Elevation provider '{}' returned mostly empty data, falling back to '{}'.",
                                name,
                                chain[i + 1].name()
                            ),
                        );
                        emit_gui_progress_update(10.0, "Downloading data...");
                        continue;
                    }
                }
                return Ok(raw);
            }
            Err(e) if i < last => {
                eprintln!(
                    "Warning: Elevation provider '{}' failed: {}. Falling back to '{}'.",
                    name,
                    e,
                    chain[i + 1].name()
                );
                #[cfg(feature = "gui")]
                crate::telemetry::send_log(
                    crate::telemetry::LogLevel::Warning,
                    &format!(
                        "Elevation provider '{}' failed, falling back to '{}'.",
                        name,
                        chain[i + 1].name()
                    ),
                );
                emit_gui_progress_update(10.0, "Downloading data...");
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("provider chain is never empty")
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

#[cfg(test)]
mod grid_dim_tests {
    use super::*;
    use crate::coordinate_system::geographic::LLBBox;

    /// The Switzerland bbox: ~346 km x ~225 km, a 1.54:1 aspect.
    fn switzerland() -> LLBBox {
        LLBBox::from_str("45.80,5.95,47.82,10.50").unwrap()
    }

    #[test]
    fn grid_is_one_sample_per_block_when_it_fits() {
        // A city-sized bbox is far under the budget, so the grid must match the world
        // exactly: one elevation sample per block, no blur, no wasted memory.
        let bbox = LLBBox::from_str("47.3700,8.5350,47.3790,8.5480").unwrap();
        let (world_w, world_h, grid_w, grid_h) = compute_grid_dims(&bbox, 1.0);
        assert_eq!((grid_w, grid_h), (world_w, world_h));
    }

    #[test]
    fn oversized_grid_keeps_the_world_aspect_ratio() {
        // Switzerland at scale 0.1 is ~34.6k x 22.5k blocks, well over the cell budget.
        // The old per-axis clamp produced a square 16384x16384 grid, smearing X ~1.5x
        // more than Z. Both axes must now shrink by the same factor.
        let (world_w, world_h, grid_w, grid_h) = compute_grid_dims(&switzerland(), 0.1);
        assert!(grid_w * grid_h <= MAX_ELEVATION_GRID_CELLS);

        let world_aspect = world_w as f64 / world_h as f64;
        let grid_aspect = grid_w as f64 / grid_h as f64;
        assert!(
            (world_aspect - grid_aspect).abs() / world_aspect < 0.01,
            "grid aspect {grid_aspect:.3} must track world aspect {world_aspect:.3}"
        );

        // Equivalently: blocks-per-sample must be the same on both axes (isotropic).
        let smear_x = world_w as f64 / grid_w as f64;
        let smear_z = world_h as f64 / grid_h as f64;
        assert!(
            (smear_x - smear_z).abs() < 0.05,
            "sampling must be isotropic, got {smear_x:.2} blocks/sample in X vs {smear_z:.2} in Z"
        );
    }

    #[test]
    fn grid_never_exceeds_the_cell_budget() {
        for scale in [0.05, 0.1, 0.3, 0.5, 1.0, 2.5] {
            let (_, _, grid_w, grid_h) = compute_grid_dims(&switzerland(), scale);
            assert!(
                grid_w * grid_h <= MAX_ELEVATION_GRID_CELLS,
                "scale {scale} blew the cell budget: {grid_w}x{grid_h}"
            );
            assert!(grid_w >= 2 && grid_h >= 2);
        }
    }

    #[test]
    fn long_thin_bbox_respects_the_per_axis_cap() {
        // 50188 x 5004 blocks: 251M cells, comfortably inside the 268M budget, yet 3x past
        // the per-axis cap. Clamping on total cells alone would leave the full 50k-wide grid
        // and triple the old peak allocation for this shape.
        let strip = LLBBox::from_str("46.00,5.95,46.045,6.60").unwrap();
        let (world_w, world_h, grid_w, grid_h) = compute_grid_dims(&strip, 1.0);
        assert!(
            world_w > MAX_ELEVATION_GRID_DIM,
            "test bbox must actually exceed the per-axis cap, got {world_w}"
        );
        assert!(
            (world_w * world_h) < MAX_ELEVATION_GRID_CELLS,
            "bbox must sit inside the cell budget so this isolates the per-axis cap"
        );
        assert!(grid_w <= MAX_ELEVATION_GRID_DIM && grid_h <= MAX_ELEVATION_GRID_DIM);
        assert!(grid_w * grid_h <= MAX_ELEVATION_GRID_CELLS);

        // Capping must not reintroduce the smear: both axes still shrink by one factor.
        let smear_x = world_w as f64 / grid_w as f64;
        let smear_z = world_h as f64 / grid_h as f64;
        assert!(
            (smear_x - smear_z).abs() / smear_x < 0.01,
            "sampling must stay isotropic, got {smear_x:.2} blocks/sample in X vs {smear_z:.2} in Z"
        );
    }
}
