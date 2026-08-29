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
pub use postprocess::AbsoluteVerticalMapping;
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
    /// Affine params of the metre->Minecraft-Y scaling: the elevation in metres that maps
    /// to `ground_level`, and the blocks-per-metre slope. Used to map a real-world
    /// elevation (e.g. the snow line) to a Minecraft Y threshold:
    /// `y = ground_level + (h_m - min_height_m) * blocks_per_meter`.
    /// On the normalised path this is the grid's own minimum; under an absolute vertical
    /// mapping it is 0 m, because that affine is anchored at sea level instead.
    pub(crate) min_height_m: f64,
    pub(crate) blocks_per_meter: f64,
    /// Terrain base actually used: the requested ground level, or lower if the relief
    /// needed the extended floor. Every consumer of the affine must use this, not args.
    pub(crate) ground_level: i32,
    /// Raw source elevation range in metres, independent of the Y mapping above.
    pub(crate) source_min_m: f64,
    pub(crate) source_max_m: f64,
    /// Set when at least one cell was clamped against the world's Y bounds.
    pub(crate) clipped: bool,
}

impl ElevationData {
    /// Raw source elevation range in metres, `(min, max)`, before any Minecraft Y mapping.
    /// Recorded during the fetch, so reading it costs nothing.
    // Consumed by the stream server's elevation-range query; no in-process caller yet.
    #[allow(dead_code)]
    pub fn source_elevation_range_m(&self) -> (f64, f64) {
        (self.source_min_m, self.source_max_m)
    }

    /// True when at least one terrain cell was clamped against the world's Y bounds, so the
    /// caller can report the clipping instead of silently serving truncated terrain. Only
    /// ever set under an absolute vertical mapping — the normalised path fits itself to the
    /// available budget by construction.
    // Reported over the wire by the stream server; no in-process caller yet.
    #[allow(dead_code)]
    pub fn clipped(&self) -> bool {
        self.clipped
    }
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

/// Grid dimension cap per axis for [`query_elevation_range`]. That query answers "how tall
/// does this world need to be", which a coarse sample settles just as well as a native one,
/// so it deliberately fetches far less than generation would.
pub const ELEVATION_QUERY_MAX_DIM: usize = 256;

/// Compute world and grid dimensions for the given bbox and scale.
///
/// The world extent is derived from the bbox's haversine ground distance, matching
/// `CoordTransformer::llbbox_to_xzbbox` for the Local projection. Callers working under a
/// projection that does not preserve ground distance (Web Mercator, whose northing stretches
/// by 1/cos(lat)) must use [`compute_grid_dims_for_extent`] with their real XZ extent
/// instead: everything downstream maps a block to a grid cell by the ratio
/// `block / (world_extent - 1)`, so a world taller than the extent believed here would have
/// its whole northern band clamped onto the last grid row.
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
    compute_grid_dims_for_extent(world_width, world_height)
}

/// Grid dimensions for a world whose extent in blocks is already known.
///
/// This is the projection-independent half of [`compute_grid_dims`]: it only decides how
/// finely to sample a world of the given size, never how big that world is.
///
/// Returns `(world_width, world_height, grid_width, grid_height)`, echoing the extent back
/// so both call sites destructure the same shape.
pub fn compute_grid_dims_for_extent(
    world_width: usize,
    world_height: usize,
) -> (usize, usize, usize, usize) {
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

/// Optional overrides for a fetch. `Default::default()` reproduces the historical behaviour
/// exactly: the world extent comes from the bbox's haversine size, and the vertical mapping
/// is normalised per request.
#[derive(Clone, Copy, Debug, Default)]
pub struct ElevationOptions {
    /// The world's real extent in blocks, `(width_x, height_z)`. Supply it whenever the
    /// caller has already projected the bbox — see [`compute_grid_dims`] for why deriving it
    /// from haversine distance goes wrong under a non-equidistant projection.
    pub world_extent: Option<(usize, usize)>,
    /// Absolute metre->Y mapping. `None` normalises the relief per request, which makes the
    /// vertical affine depend on the bbox and therefore differ between neighbouring tiles.
    pub vertical: Option<AbsoluteVerticalMapping>,
}

/// Fetch elevation data for the given bounding box, with the historical
/// bbox-derived extent and per-request vertical normalisation.
///
/// See [`fetch_elevation_data_with`] for the full documentation; this is that
/// function with [`ElevationOptions::default()`].
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
    fetch_elevation_data_with(
        bbox,
        scale,
        ground_level,
        min_ground_level,
        disable_height_limit,
        extended_max_y,
        land_cover,
        source_mode,
        benchmark,
        ElevationOptions::default(),
    )
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
/// `opts` carries the two things a tiled caller must control: the world's real extent in
/// blocks (so block->grid lookups stay correct under any projection) and an absolute
/// vertical mapping (so neighbouring tiles share one metre->Y affine).
///
/// The returned ElevationData contains heights in Minecraft Y coordinates.
#[allow(clippy::too_many_arguments)]
pub fn fetch_elevation_data_with(
    bbox: &LLBBox,
    scale: f64,
    ground_level: i32,
    min_ground_level: i32,
    disable_height_limit: bool,
    extended_max_y: i32,
    land_cover: Option<&mut LandCoverData>,
    source_mode: SourceMode,
    benchmark: bool,
    opts: ElevationOptions,
) -> Result<ElevationData, Box<dyn std::error::Error>> {
    let mut bench = crate::bench::Bench::new(benchmark);
    let (world_width, world_height, grid_width, grid_height) = match opts.world_extent {
        Some((w, h)) => compute_grid_dims_for_extent(w, h),
        None => compute_grid_dims(bbox, scale),
    };

    let chain = build_provider_chain(bbox, source_mode);

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
    //
    // Both are ground metres per grid cell, so this stays keyed to the bbox's real size on
    // the ground — never to the projected world extent, which may stretch it.
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

    let scaled = scale_to_minecraft(
        &height_grid,
        scale,
        ground_level,
        min_ground_level,
        disable_height_limit,
        extended_max_y,
        opts.vertical,
    );
    bench.mark("elev_scale_to_mc");

    // Log min/max block heights
    let mut min_block_height = f64::MAX;
    let mut max_block_height = f64::MIN;
    for row in &scaled.heights {
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
    let mc_heights_f32: Vec<Vec<f32>> = scaled
        .heights
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
        min_height_m: scaled.reference_m,
        blocks_per_meter: scaled.blocks_per_meter,
        ground_level: scaled.ground_level,
        source_min_m: scaled.source_min_m,
        source_max_m: scaled.source_max_m,
        clipped: scaled.clipped,
    })
}

/// Real-world elevation range for an area, in metres, as `(min, max)`.
///
/// Answers "how much vertical room does this area need" without generating anything: same
/// provider chain and same on-disk tile cache as a real fetch, but on a grid capped at
/// [`ELEVATION_QUERY_MAX_DIM`] per axis and with no post-processing, land-cover repair or
/// Y scaling. A sizing query does not need native resolution, and outlier filtering would
/// only narrow the answer — the caller wants the extremes the terrain actually contains.
// Consumed by the stream server's QueryElevationRange; no in-process caller yet.
#[allow(dead_code)]
pub fn query_elevation_range(bbox: &LLBBox) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    let (world_width, world_height, _, _) = compute_grid_dims(bbox, 1.0);
    // Shrink both axes by one factor so the coarse grid keeps the area's aspect ratio.
    let shrink = (world_width.max(world_height) as f64 / ELEVATION_QUERY_MAX_DIM as f64).max(1.0);
    let grid_width =
        ((world_width as f64 / shrink).round() as usize).clamp(2, ELEVATION_QUERY_MAX_DIM);
    let grid_height =
        ((world_height as f64 / shrink).round() as usize).clamp(2, ELEVATION_QUERY_MAX_DIM);

    let chain = build_provider_chain(bbox, SourceMode::Auto);
    let raw = fetch_raw_with_fallback(&chain, bbox, grid_width, grid_height)?;

    let mut min_m = f64::MAX;
    let mut max_m = f64::MIN;
    for row in &raw.heights_meters {
        for &h in row {
            if h.is_finite() {
                min_m = min_m.min(h);
                max_m = max_m.max(h);
            }
        }
    }
    if min_m > max_m {
        return Err("elevation providers returned no usable samples for this area".into());
    }
    Ok((min_m, max_m))
}

/// The provider fallback chain for a bbox: the selected provider first, then Mapterhorn,
/// then AWS Terrain Tiles, skipping whichever of those the selection already is.
fn build_provider_chain(bbox: &LLBBox, source_mode: SourceMode) -> Vec<Box<dyn ElevationProvider>> {
    let provider = select_provider(bbox, source_mode);
    let mut chain: Vec<Box<dyn ElevationProvider>> = vec![provider];
    if chain[0].name() != "mapterhorn" && chain[0].name() != "aws" {
        chain.push(Box::new(providers::mapterhorn::Mapterhorn));
    }
    if chain[0].name() != "aws" {
        chain.push(Box::new(providers::aws_terrain::AwsTerrain));
    }
    chain
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
    /// Verbatim copy of `compute_grid_dims` from before it was split, kept as the oracle
    /// for the refactor: existing callers must get byte-identical dimensions.
    fn legacy_compute_grid_dims(bbox: &LLBBox, scale: f64) -> (usize, usize, usize, usize) {
        let (base_scale_z, base_scale_x) = geo_distance(bbox.min(), bbox.max());
        let scale_factor_z: f64 = base_scale_z.floor() * scale;
        let scale_factor_x: f64 = base_scale_x.floor() * scale;
        let world_width: usize = scale_factor_x as usize + 1;
        let world_height: usize = scale_factor_z as usize + 1;

        let mut grid_width: usize = world_width.max(2);
        let mut grid_height: usize = world_height.max(2);
        let cells = grid_width as f64 * grid_height as f64;
        let budget_shrink = (cells / MAX_ELEVATION_GRID_CELLS as f64).sqrt();
        let axis_shrink = grid_width.max(grid_height) as f64 / MAX_ELEVATION_GRID_DIM as f64;
        let shrink = budget_shrink.max(axis_shrink);
        if shrink > 1.0 {
            grid_width =
                ((grid_width as f64 / shrink).floor() as usize).clamp(2, world_width.max(2));
            grid_height =
                ((grid_height as f64 / shrink).floor() as usize).clamp(2, world_height.max(2));
        }
        (world_width, world_height, grid_width, grid_height)
    }

    #[test]
    fn splitting_out_the_extent_left_existing_callers_untouched() {
        let bboxes = [
            // City block, well under every limit.
            LLBBox::from_str("47.3700,8.5350,47.3790,8.5480").unwrap(),
            // Country, over the cell budget.
            switzerland(),
            // Long thin strip, over the per-axis cap but inside the budget.
            LLBBox::from_str("46.00,5.95,46.045,6.60").unwrap(),
            // Near-equatorial and southern-hemisphere boxes, different aspect ratios.
            LLBBox::from_str("-0.05,36.80,0.05,36.95").unwrap(),
            LLBBox::from_str("-33.88,151.18,-33.85,151.24").unwrap(),
            // Degenerate sliver: the .max(2) floors have to survive too.
            LLBBox::from_str("52.5200,13.4050,52.5201,13.4051").unwrap(),
        ];
        for bbox in &bboxes {
            for scale in [0.001, 0.05, 0.1, 0.5, 1.0, 2.5] {
                assert_eq!(
                    compute_grid_dims(bbox, scale),
                    legacy_compute_grid_dims(bbox, scale),
                    "dims changed for {bbox:?} at scale {scale}"
                );
            }
        }
    }

    #[test]
    fn the_haversine_extent_delegates_to_the_extent_variant() {
        // The wrapper must be exactly "derive the extent, then defer".
        for scale in [0.1, 1.0, 2.0] {
            let (world_w, world_h, grid_w, grid_h) = compute_grid_dims(&switzerland(), scale);
            assert_eq!(
                compute_grid_dims_for_extent(world_w, world_h),
                (world_w, world_h, grid_w, grid_h)
            );
        }
    }

    #[test]
    fn an_explicit_extent_overrides_the_haversine_guess() {
        // A 0.02x0.02 degree box at 52N: Web Mercator stretches northing by 1/cos(lat), so
        // the real world is ~1.6x taller than the haversine distance. Feeding that real
        // extent in has to produce a grid that covers it — otherwise every block past the
        // haversine height clamps onto the last elevation row.
        let bbox = LLBBox::from_str("52.00,13.00,52.02,13.02").unwrap();
        let (haversine_w, haversine_h, _, haversine_grid_h) = compute_grid_dims(&bbox, 1.0);
        let projected_h = haversine_h * 8 / 5;
        let (world_w, world_h, _, grid_h) = compute_grid_dims_for_extent(haversine_w, projected_h);
        assert_eq!(
            (world_w, world_h),
            (haversine_w, projected_h),
            "the extent is passed through as-is"
        );
        assert!(
            grid_h > haversine_grid_h,
            "explicit extent {projected_h} must sample more rows than the haversine {haversine_h}"
        );
    }

    #[test]
    fn an_explicit_extent_still_respects_the_limits() {
        // Same clamping contract as the bbox path, driven purely by the extent.
        let (_, _, grid_w, grid_h) = compute_grid_dims_for_extent(80_000, 40_000);
        assert!(grid_w <= MAX_ELEVATION_GRID_DIM && grid_h <= MAX_ELEVATION_GRID_DIM);
        assert!(grid_w * grid_h <= MAX_ELEVATION_GRID_CELLS);
        let smear_x = 80_000.0 / grid_w as f64;
        let smear_z = 40_000.0 / grid_h as f64;
        assert!(
            (smear_x - smear_z).abs() / smear_x < 0.01,
            "sampling must stay isotropic, got {smear_x:.2} vs {smear_z:.2}"
        );
        // A degenerate extent still yields a usable grid.
        assert_eq!(compute_grid_dims_for_extent(0, 1), (0, 1, 2, 2));
    }
}
