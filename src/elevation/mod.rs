pub mod cache;
mod detail;
pub mod peaks;
pub mod postprocess;
pub mod provider;
pub mod providers;
pub mod selector;

use crate::{
    coordinate_system::{geographic::LLBBox, transformation::geo_distance},
    land_cover::LandCoverData,
    progress::emit_gui_progress_update,
};
pub use peaks::{PeakSource, TallModel, TerrainHints};
use postprocess::{
    apply_land_cover_repair, fill_nan_values, filter_elevation_outliers, repair_terrain_anomalies,
    scale_to_minecraft_reserving, top_reserve_for,
};
pub use postprocess::{AffinePolicy, ElevationAffine, SoftTop};
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
    /// Converts a Minecraft-Y difference into the `8 * tan(incline)` units the
    /// slope thresholds are documented in: `args.scale / blocks_per_meter`.
    /// 1.0 unless the relief was compressed or `--height-multiplier` stretched it.
    pub(crate) slope_correction: f64,
    /// Terrain base actually used: the requested ground level, or lower if the relief
    /// needed the extended floor. Every consumer of the affine must use this, not args.
    pub(crate) ground_level: i32,
    pub(crate) soft_top: Option<SoftTop>,
    /// The fit used a ceiling a pack raised above vanilla's.
    pub(crate) extended_ceiling: bool,
    /// When the fit raised the build height for this area itself: the highest terrain Y and
    /// the tallest building or model on it, in blocks. They size the raised dimension.
    pub(crate) raised_for: Option<RaisedFor>,
    /// The margin `crop` cut away, read only past the area.
    pub(crate) halo: Option<Halo>,
}

/// Heights of the margin a grid was fetched with, so geometry kept past a One World seam
/// stands on the same terrain the neighbouring area builds there instead of on the edge.
#[derive(Clone)]
pub(crate) struct Halo {
    x0: usize,
    z0: usize,
    inner: (usize, usize),
    outer: (usize, usize),
    /// Uncropped rows; those beside the area keep only the cells either side of it.
    rows: Vec<Vec<f32>>,
}

impl Halo {
    fn cut(grid: &[Vec<f32>], x0: usize, z0: usize, width: usize, height: usize) -> Self {
        let rows = grid
            .iter()
            .enumerate()
            .map(|(z, row)| {
                if (z0..z0 + height).contains(&z) {
                    row[..x0]
                        .iter()
                        .chain(&row[x0 + width..])
                        .copied()
                        .collect()
                } else {
                    row.clone()
                }
            })
            .collect();
        Self {
            x0,
            z0,
            inner: (width, height),
            outer: (grid.first().map_or(0, Vec::len), grid.len()),
            rows,
        }
    }

    /// Height at `(x, z)` from the area's corner, clamped to the margin; `None` inside the area.
    #[inline(always)]
    fn at(&self, x: i32, z: i32) -> Option<f32> {
        let (x, z) = (i64::from(x), i64::from(z));
        if (0..self.inner.0 as i64).contains(&x) && (0..self.inner.1 as i64).contains(&z) {
            return None;
        }
        let px = (x + self.x0 as i64).clamp(0, self.outer.0 as i64 - 1) as usize;
        let pz = (z + self.z0 as i64).clamp(0, self.outer.1 as i64 - 1) as usize;
        let row = self.rows.get(pz)?;
        if row.len() == self.outer.0 || px < self.x0 {
            row.get(px).copied()
        } else if px >= self.x0 + self.inner.0 {
            row.get(px - self.inner.0).copied()
        } else {
            None
        }
    }
}

/// What an area that needed more than vanilla's height reaches, so its dimension is
/// raised only that far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RaisedFor {
    pub terrain_top_y: i32,
    pub tallest_blocks: i32,
    /// Overture's structures still have to be counted, by `with_overture`.
    pub overture_pending: bool,
}

impl RaisedFor {
    /// Counts Overture's structures too, waiting for them if the fit did not. Waited for
    /// here rather than in the fit, so the terrain work after it runs while they download.
    pub(crate) fn with_overture(self, peaks: &PeakSource, scale: f64) -> Self {
        if !self.overture_pending {
            return self;
        }
        let tallest = (tallest_m(&peaks.take_overture()) * scale).ceil() as i32;
        Self {
            tallest_blocks: self.tallest_blocks.max(tallest),
            overture_pending: false,
            ..self
        }
    }
}

/// Height of the tallest of `models`, in metres.
fn tallest_m(models: &[TallModel]) -> f64 {
    models.iter().map(|m| m.height_m).fold(0.0, f64::max)
}

/// Room the height fit has: the base it starts from, the lowest base it may sink to and the
/// ceiling (`extended_max_y` when `disable_height_limit`, vanilla's otherwise).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeightBand {
    pub ground_level: i32,
    pub min_ground_level: i32,
    pub disable_height_limit: bool,
    pub extended_max_y: i32,
    /// The world's floor, which a tunnel under carved water has to keep off.
    pub floor_y: i32,
}

impl HeightBand {
    fn extended_ceiling(&self) -> bool {
        self.disable_height_limit && self.extended_max_y > crate::world_editor::DEFAULT_MAX_Y
    }

    /// This band, its base high enough that a tunnel under each of `crossings` keeps its
    /// cover under the carved bed, `BORE_ROOM` over the floor. A body's surface stands as
    /// far over the base as its lowest terrain over the area's lowest, `lo_m`, at `bpm`
    /// blocks per metre, so water up the relief needs less of a lift, and a shallow body
    /// less than the deepest. Only as much as the deepest of them needs, then.
    fn with_bore_room(
        self,
        crossings: &[crate::water_depth::BoreCrossing],
        lo_m: f64,
        range_m: f64,
        bpm: f64,
    ) -> Self {
        let room = |c: &crate::water_depth::BoreCrossing, over_base: i32| {
            self.floor_y + c.depth + 2 + crate::water_depth::BORE_ROOM - over_base
        };
        let Some(worst) = crossings.iter().map(|c| room(c, 0)).max() else {
            return self;
        };
        // The fit keeps the full scale while the relief takes at most half the column over
        // the base, all a top reserve leaves it. Past that it may compress the relief, and
        // a body up it then stands lower than its terrain says: count none of its height.
        let ceiling = postprocess::terrain_ceiling(self.disable_height_limit, self.extended_max_y);
        let exact = range_m * bpm <= f64::from(ceiling - worst.max(self.ground_level)) / 2.0;
        let needed = crossings
            .iter()
            .map(|c| {
                // Two blocks short, for the levelling and the snap of a water surface.
                let over_base = if exact && c.lowest_m.is_finite() {
                    (((c.lowest_m - lo_m) * bpm).floor() as i32 - 2).max(0)
                } else {
                    0
                };
                room(c, over_base)
            })
            .max()
            .unwrap_or(worst);
        let ground_level = self.ground_level.max(needed);
        Self {
            ground_level,
            min_ground_level: self.min_ground_level.max(needed).min(ground_level),
            ..self
        }
    }
}

/// `band`, or `taller` when the area does not fit `band` at full height. `band` is then the
/// vanilla one, and the tall structures count as well as the models. Overture's structures
/// come from `overture`, which waits for them, and only when everything else fits.
#[allow(clippy::too_many_arguments)]
fn pick_band(
    heights: &[Vec<f64>],
    bbox: &LLBBox,
    hints: &TerrainHints,
    overture: impl FnOnce() -> Vec<TallModel>,
    scale: f64,
    height_multiplier: f64,
    band: HeightBand,
    taller: Option<HeightBand>,
) -> HeightBand {
    let Some(taller) = taller else {
        return band;
    };
    let why = vanilla_misses(
        hints,
        overture,
        |structures| peaks::model_cells(heights, bbox, structures),
        |tops| {
            postprocess::vanilla_overflow(
                heights,
                tops,
                scale * height_multiplier,
                scale,
                band.ground_level,
            )
        },
    );
    match why {
        Some(why) => {
            eprintln!("Extended build height: {why}");
            taller
        }
        None => band,
    }
}

/// Why the vanilla height cannot hold an area: `overflow` on the models and structures OSM
/// gave, turned into (ground metres, height metres) by `tops`, and only while those fit and
/// Overture's follow, on those as well.
pub(crate) fn vanilla_misses(
    hints: &TerrainHints,
    overture: impl FnOnce() -> Vec<TallModel>,
    tops: impl Fn(&[TallModel]) -> Vec<(f64, f64)>,
    overflow: impl Fn(&[(f64, f64)]) -> Option<String>,
) -> Option<String> {
    let mut all = tops(&hints.tall_models);
    all.extend(tops(&hints.tall_structures));
    match overflow(&all) {
        None if hints.overture_follows => {
            all.extend(tops(&overture()));
            overflow(&all)
        }
        why => why,
    }
}

impl ElevationData {
    pub fn affine(&self) -> ElevationAffine {
        ElevationAffine {
            min_height_m: self.min_height_m,
            blocks_per_meter: self.blocks_per_meter,
            ground_level: self.ground_level,
            soft_top: self.soft_top,
        }
    }

    /// How much the soft top compresses heights at `y`: 1 up to the knee, then
    /// `cosh((y - knee_y) / width)`, the inverse slope of the curve there.
    #[inline]
    pub fn soft_top_stretch(&self, y: i32) -> f64 {
        let Some(top) = self.soft_top else {
            return 1.0;
        };
        let knee_y =
            self.ground_level as f64 + (top.knee_m - self.min_height_m) * self.blocks_per_meter;
        let above = f64::from(y) - knee_y;
        if above <= 0.0 {
            1.0
        } else {
            (above / top.width_blocks).cosh()
        }
    }

    /// Lowest terrain Y in the grid, rounded down.
    pub fn lowest_y(&self) -> Option<i32> {
        self.heights
            .iter()
            .flatten()
            .filter(|h| h.is_finite())
            .map(|&h| h.floor() as i32)
            .min()
    }

    pub fn remap_rows_to_mercator(&mut self, lat_top: f64, lat_bottom: f64) {
        let rows = self.height;
        crate::grid_ops::remap_rows_in_place(
            &mut self.heights,
            |gz| crate::grid_ops::mercator_source_row(lat_top, lat_bottom, rows, gz),
            |a, b, t| (a as f64 * (1.0 - t) + b as f64 * t) as f32,
        );
    }

    /// Only meaningful while one cell is one block.
    pub fn crop(&mut self, x0: usize, z0: usize, width: usize, height: usize) {
        self.halo = Some(Halo::cut(&self.heights, x0, z0, width, height));
        crate::grid_ops::crop_rows(&mut self.heights, x0, z0, width, height);
        self.width = width;
        self.height = height;
        self.world_width = width;
        self.world_height = height;
    }

    /// Height of a block past the cropped area, or `None` inside it or without a margin.
    #[inline(always)]
    pub fn beyond(&self, x: i32, z: i32) -> Option<f32> {
        self.halo.as_ref()?.at(x, z)
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
/// Memory note: a full 16384 × 16384 f64 grid is ~2.1 GB. The peak is the
/// land-cover Gaussian, which holds the height grid, one blur buffer of the
/// same size and an f32 feathered mask alongside the land-cover grids: about
/// 6.5 GB for the maximum case. Target deployment (MapSmith) has >20 GB
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
    compute_grid_dims_for_world(world_width, world_height)
}

/// `compute_grid_dims` for a world whose block size is already known.
pub fn compute_grid_dims_for_world(
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

/// A One World area's margin: the grid runs `pad` cells past the area on every side. With
/// one cell per block its cell (0, 0) is world block `origin`, so neighbouring areas can
/// line up what they work out over the margin they share.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seam {
    pub pad: usize,
    pub origin: Option<(i64, i64)>,
}

/// Fetch elevation data for the given bounding box.
///
/// Fetches from Mapterhorn, falling back to AWS Terrain Tiles if it fails.
///
/// If `land_cover` is provided, applies land-cover-aware artifact repair
/// (water leveling, built-up smoothing) before scaling. This fixes LiDAR
/// classification errors at urban structures (tunnel portals, overpasses)
/// and coastal tile-boundary artifacts.
///
/// The returned ElevationData contains heights in Minecraft Y coordinates.
///
/// `dims` comes from `compute_grid_dims`, so it matches the land cover grid.
/// `height_multiplier` scales blocks per metre on the vertical axis only.
/// The fit uses `band`, or `taller` when the area needs more room than `band` has.
/// `seam` is set for a One World's area, fetched with a margin past its edges.
#[allow(clippy::too_many_arguments)]
pub fn fetch_elevation_data(
    bbox: &LLBBox,
    scale: f64,
    height_multiplier: f64,
    band: HeightBand,
    taller: Option<HeightBand>,
    mut land_cover: Option<&mut LandCoverData>,
    source_mode: SourceMode,
    benchmark: bool,
    dims: (usize, usize, usize, usize),
    affine: AffinePolicy,
    peaks: &PeakSource,
    seam: Option<Seam>,
) -> Result<ElevationData, Box<dyn std::error::Error>> {
    let mut bench = crate::bench::Bench::new(benchmark);
    let (world_width, world_height, grid_width, grid_height) = dims;

    // Fallback chain: Mapterhorn, then AWS if it fails outright.
    let provider = select_provider(bbox, source_mode);
    let mut chain: Vec<Box<dyn ElevationProvider>> = vec![provider];
    if source_mode.allows_earth_fallback() && chain[0].name() != "aws" {
        chain.push(Box::new(providers::aws_terrain::AwsTerrain));
    }

    emit_gui_progress_update(10.0, "Downloading data...");

    let raw = fetch_raw_with_fallback(&chain, bbox, grid_width, grid_height)?;

    bench.mark("elev_raw_fetch");
    emit_gui_progress_update(12.0, "Processing elevation...");

    // Shared post-processing pipeline
    let RawElevationGrid {
        heights_meters: mut height_grid,
        source_pixels,
    } = raw;
    let (bbox_height_m, bbox_width_m) = geo_distance(bbox.min(), bbox.max());
    let m_per_cell = (bbox_width_m / grid_width as f64 + bbox_height_m / grid_height as f64) * 0.5;
    // Both passes target Earth DSM defects and actively damage altimetry: the outlier
    // gate is calibrated on Earth's elevation range, and a lunar mare is flat to within
    // metres so a real central peak reads as an anomaly to the MAD filter. PDS gaps are
    // already NaN, so fill is all we need.
    if source_mode.allows_earth_fallback() {
        filter_elevation_outliers(&mut height_grid);
        bench.mark("elev_filter_outliers");
        repair_terrain_anomalies(&mut height_grid, m_per_cell);
        bench.mark("elev_repair_anomalies");
    }
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
    let (built_up_sigma_cells, coastal_pull_cells) = if m_per_cell > 0.0 {
        (
            BUILT_UP_SIGMA_M / m_per_cell,
            (COASTAL_PULL_M / m_per_cell).round() as u32,
        )
    } else {
        (0.0, 0)
    };

    if let Some(lc) = land_cover.as_deref_mut() {
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
    let mut hints = peaks.take();
    let graded = std::mem::take(&mut hints.graded);
    // After the repairs, which would smooth it away, and before the peaks, which set their
    // summits over it. Only where the source was coarse.
    if let Some(source) = source_pixels {
        let changed = detail::add_steep_detail(
            &mut height_grid,
            bbox,
            &source,
            scale,
            seam.and_then(|s| s.origin),
            land_cover.as_deref(),
            &graded,
        );
        if changed > 0 {
            println!(
                "Steep detail: roughened {:.1}% of the terrain, steep ground with only coarse elevation data",
                changed as f64 * 100.0 / (grid_width * grid_height) as f64
            );
        }
    }
    drop(graded);
    bench.mark("elev_steep_detail");
    // After the repairs, which would smooth a restored needle back down, and
    // before the fit, so the summit counts towards the height range.
    peaks::raise_peaks(
        &mut height_grid,
        bbox,
        &hints.peaks,
        m_per_cell,
        seam.map(|s| s.pad),
    );
    // A tunnel under the water has to get under its carved bed, and the floor the carve
    // was given may leave no room for one.
    let crossings = land_cover.as_deref().map_or_else(Vec::new, |lc| {
        let cells = peaks::bore_water_cells(lc, bbox, &hints.bores);
        // The repair skips a land cover of another size than the heights; so do these.
        crate::water_depth::bore_crossings(&lc.grid, world_width, world_height, &cells, |x, y| {
            height_grid
                .get(y)
                .and_then(|row| row.get(x))
                .copied()
                .unwrap_or(f64::NAN)
        })
    });
    let (band, taller) = if crossings.is_empty() {
        (band, taller)
    } else {
        let (lo, hi) = height_grid
            .iter()
            .flatten()
            .filter(|h| h.is_finite())
            .fold((f64::MAX, f64::MIN), |(a, b), &h| (a.min(h), b.max(h)));
        let lift = |b: HeightBand| {
            b.with_bore_room(
                &crossings,
                lo,
                (hi - lo).max(0.0),
                scale * height_multiplier,
            )
        };
        (lift(band), taller.map(lift))
    };
    // Overture's list can be read only once, so its tallest is kept on the way past.
    let overture_tallest = std::cell::Cell::new(None);
    let band = pick_band(
        &height_grid,
        bbox,
        &hints,
        || {
            let structures = peaks.take_overture();
            overture_tallest.set(Some(tallest_m(&structures)));
            structures
        },
        scale,
        height_multiplier,
        band,
        taller,
    );
    // A band raised for this area is sized by the tallest thing standing on it. Overture's
    // count too, even where OSM's alone made the case; those the fit did not wait for are
    // left to `RaisedFor::with_overture`.
    let overture_pending = hints.overture_follows && overture_tallest.get().is_none();
    let tallest_structure_m = (taller == Some(band)).then(|| {
        tallest_m(&hints.tall_models)
            .max(tallest_m(&hints.tall_structures))
            .max(overture_tallest.get().unwrap_or(0.0))
    });
    // Only the band needed them, and a city's worth would sit through the fit.
    hints.tall_structures = Vec::new();
    let HeightBand {
        ground_level,
        min_ground_level,
        disable_height_limit,
        extended_max_y,
        ..
    } = band;
    // The models stand at world scale, not at the terrain's compressed one.
    let top_reserve = match affine {
        AffinePolicy::Fixed(_) => 0,
        _ => top_reserve_for(
            &height_grid,
            &peaks::model_cells(&height_grid, bbox, &hints.tall_models),
            scale * height_multiplier,
            scale,
            ground_level,
            min_ground_level,
            disable_height_limit,
            extended_max_y,
            matches!(affine, AffinePolicy::FitWithHeadroom),
        ),
    };
    emit_gui_progress_update(16.0, "Processing elevation...");

    // The multiplier stretches only the vertical axis. `slope_correction` below still
    // divides by the horizontal scale, so slopes keep their real incline and a stretched
    // hillside stays grass instead of turning to rock.
    let (mc_heights, affine) = scale_to_minecraft_reserving(
        &height_grid,
        scale * height_multiplier,
        ground_level,
        min_ground_level,
        disable_height_limit,
        extended_max_y,
        affine,
        top_reserve,
    );
    // The metre grid is done with; freeing it first keeps the downcast below from holding
    // three full grids at once.
    drop(height_grid);
    let ElevationAffine {
        min_height_m,
        blocks_per_meter,
        ground_level: effective_ground_level,
        soft_top,
    } = affine;
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
        slope_correction: if blocks_per_meter > 0.0 {
            scale / blocks_per_meter
        } else {
            1.0
        },
        ground_level: effective_ground_level,
        soft_top,
        extended_ceiling: band.extended_ceiling(),
        // The models stand at world scale, as in the reserve above.
        raised_for: tallest_structure_m.map(|m| RaisedFor {
            terrain_top_y: if max_block_height >= min_block_height {
                max_block_height.ceil() as i32
            } else {
                effective_ground_level
            },
            tallest_blocks: (m * scale).ceil() as i32,
            overture_pending,
        }),
        halo: None,
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
mod band_tests {
    use super::*;

    fn band(extended: bool) -> HeightBand {
        HeightBand {
            ground_level: -62,
            min_ground_level: if extended { -2030 } else { -62 },
            disable_height_limit: extended,
            extended_max_y: 2031,
            floor_y: if extended { -2032 } else { -64 },
        }
    }

    fn bbox() -> LLBBox {
        LLBBox::new(46.0, 7.0, 46.01, 7.01).unwrap()
    }

    fn tower(lat: f64, height_m: f64) -> TallModel {
        TallModel {
            lat,
            lon: 7.005,
            height_m,
        }
    }

    fn pick(grid: &[Vec<f64>], hints: &TerrainHints, taller: bool) -> HeightBand {
        pick_band(
            grid,
            &bbox(),
            hints,
            Vec::new,
            1.0,
            1.0,
            band(false),
            taller.then(|| band(true)),
        )
    }

    #[test]
    fn overture_is_waited_for_only_while_vanilla_still_fits() {
        let hills = vec![vec![500.0, 600.0], vec![550.0, 650.0]];
        let alps = vec![vec![500.0, 1500.0], vec![900.0, 2500.0]];
        let follows = TerrainHints {
            overture_follows: true,
            ..Default::default()
        };
        let with = |grid: &[Vec<f64>], overture: Vec<TallModel>| {
            pick_band(
                grid,
                &bbox(),
                &follows,
                move || overture,
                1.0,
                1.0,
                band(false),
                Some(band(true)),
            )
        };
        assert_eq!(with(&hills, vec![tower(46.005, 541.0)]), band(true));
        assert_eq!(with(&hills, vec![tower(46.005, 200.0)]), band(false));
        // The relief decides on its own, so Overture is never asked.
        let never = || -> Vec<TallModel> { panic!("waited for Overture") };
        let picked = pick_band(
            &alps,
            &bbox(),
            &follows,
            never,
            1.0,
            1.0,
            band(false),
            Some(band(true)),
        );
        assert_eq!(picked, band(true));
    }

    #[test]
    fn the_taller_band_is_picked_only_when_vanilla_cannot_hold_the_area() {
        let hills = vec![vec![500.0, 600.0], vec![550.0, 650.0]];
        let alps = vec![vec![500.0, 1500.0], vec![900.0, 2500.0]];
        let none = TerrainHints::default();
        assert_eq!(pick(&hills, &none, true), band(false));
        assert_eq!(pick(&alps, &none, true), band(true));
        // A run that settled its height keeps it.
        assert_eq!(pick(&alps, &none, false), band(false));

        let skyscraper = TerrainHints {
            tall_structures: vec![tower(46.005, 541.0)],
            ..Default::default()
        };
        assert_eq!(pick(&hills, &skyscraper, true), band(true));
        let landmark = TerrainHints {
            tall_models: vec![tower(46.005, 541.0)],
            ..Default::default()
        };
        assert_eq!(pick(&hills, &landmark, true), band(true));
        // Outside the area, or short enough, it changes nothing.
        let elsewhere = TerrainHints {
            tall_structures: vec![tower(47.0, 541.0), tower(46.005, 200.0)],
            ..Default::default()
        };
        assert_eq!(pick(&hills, &elsewhere, true), band(false));
    }

    /// A tunnel needs room under its water's carved bed: on the vanilla floor the deepest
    /// lies on bedrock, so the base lifts by what the crossed water needs, and no more.
    #[test]
    fn a_bore_under_the_water_lifts_the_base_by_what_its_water_needs() {
        use crate::water_depth::BoreCrossing;
        let lift = |b: HeightBand, crossings: &[BoreCrossing], range_m: f64| {
            let b = b.with_bore_room(crossings, 0.0, range_m, 1.0);
            (b.ground_level, b.min_ground_level)
        };
        let deep = BoreCrossing {
            depth: 6,
            lowest_m: 0.0,
        };
        let shallow = BoreCrossing {
            depth: 2,
            lowest_m: 0.0,
        };
        // A carve six deep at the area's lowest point: the floor, the bed and its block
        // under it, the depth and the room.
        assert_eq!(lift(band(false), &[deep], 50.0), (-48, -48));
        assert_eq!(lift(band(false), &[shallow, deep], 50.0), (-48, -48));
        assert_eq!(lift(band(false), &[shallow], 50.0), (-52, -52));
        // Water 10 m up the relief stands 10 blocks over the base, less two to be safe.
        let up = BoreCrossing {
            depth: 6,
            lowest_m: 10.0,
        };
        assert_eq!(lift(band(false), &[up], 50.0), (-56, -56));
        // ...unless the relief is tall enough that the fit may compress it.
        assert_eq!(lift(band(false), &[up], 300.0), (-48, -48));
        // The extended floor has room, and a base already over the need stays.
        let extended = band(true).with_bore_room(&[deep], 0.0, 50.0, 1.0);
        assert_eq!(
            (extended.ground_level, extended.min_ground_level),
            (-62, -2016)
        );
        let high = HeightBand {
            ground_level: 10,
            min_ground_level: 10,
            ..band(false)
        };
        assert_eq!(high.with_bore_room(&[deep], 0.0, 50.0, 1.0), high);
        assert_eq!(band(false).with_bore_room(&[], 0.0, 50.0, 1.0), band(false));
    }

    #[test]
    fn the_extended_ceiling_needs_a_raised_cap() {
        assert!(band(true).extended_ceiling());
        assert!(!band(false).extended_ceiling());
        // Luanti asks for the extended height but keeps vanilla's ceiling.
        let luanti = HeightBand {
            extended_max_y: crate::world_editor::DEFAULT_MAX_Y,
            ..band(true)
        };
        assert!(!luanti.extended_ceiling());
    }
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
