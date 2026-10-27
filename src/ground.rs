use crate::args::{Args, GenerationMode};
use crate::canopy::{self, CanopyData};
use crate::celestial::CelestialBody;
use crate::coordinate_system::{
    cartesian::{XZBBox, XZPoint},
    geographic::LLBBox,
};
use crate::ecoregion::{EcoMap, Ecoregion};
use crate::elevation::{
    compute_grid_dims, compute_grid_dims_for_world, AffinePolicy, ElevationAffine,
};
use crate::elevation_data::{fetch_elevation_data, ElevationData};
use crate::land_cover::{self, LandCoverData};
use crate::osm_parser::ProcessedElement;
use crate::projection::WebMercatorProjection;
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use colored::Colorize;
use image::{Rgb, RgbImage};
use std::sync::Arc;

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
    /// The output format's pack raised the build ceiling, so relief stays close to metre scale.
    extended_ceiling: bool,
    ground_level: i32,
    elevation_data: Option<ElevationData>,
    land_cover: Option<LandCoverData>,
    /// `None` when the option is off or the fetch failed, which restores the
    /// land-cover-only behaviour.
    canopy: Option<CanopyData>,
    /// World size in blocks, used to map coords onto the land-cover grid when elevation data is absent (flat mode).
    world_width: usize,
    world_height: usize,
    /// When set, coordinates outside the rotated original bbox are skipped.
    rotation_mask: Option<RotationMask>,
    /// Minecraft Y at/above which terrain is snow-capped; `i32::MAX` disables it.
    snow_threshold_y: i32,
    /// Climate at the bbox center, driving arid/polar surface palettes and biomes.
    climate: crate::climate::Climate,
    /// Earth unless this is a Moon/Mars world, which take their own surface palette.
    body: CelestialBody,
    ecoregions: Option<Arc<EcoMap>>,
    /// A winter run: every biome becomes its snowy counterpart.
    winter: bool,
    /// Absolute block of the grids' first cell, so passes that draw patterns line up
    /// across One World seams. (0, 0) in a local run.
    origin: (i32, i32),
}

/// Layout of a run's ground grids. A projected run takes its size from the
/// projected rectangle and re-spaces rows to Mercator, so every cell sits on
/// its block.
#[derive(Clone, Debug)]
pub struct GroundFrame {
    pub world_dims: Option<(usize, usize)>,
    pub mercator: Option<WebMercatorProjection>,
    pub pad_blocks: usize,
    pub affine: AffinePolicy,
}

impl GroundFrame {
    pub fn local() -> Self {
        Self {
            world_dims: None,
            mercator: None,
            pad_blocks: 0,
            affine: AffinePolicy::Fit,
        }
    }

    pub fn from_args(args: &Args, bbox: &LLBBox) -> Self {
        let spec = crate::projection::ProjectionSpec::from_args(args);
        if spec.kind == crate::projection::ProjectionKind::Local {
            return Self::local();
        }
        let projection = spec.mercator(bbox);
        let (world_w, world_h) = match spec.transformer(bbox) {
            Ok((_, rect)) => (
                (rect.max_x() - rect.min_x() + 1) as usize,
                (rect.max_z() - rect.min_z() + 1) as usize,
            ),
            Err(_) => return Self::local(),
        };
        match &args.one_world_run {
            Some(run) => Self {
                world_dims: Some((world_w, world_h)),
                mercator: Some(projection),
                pad_blocks: crate::one_world::ground_pad_blocks(args.scale) as usize,
                affine: match run.elevation {
                    Some(affine) => AffinePolicy::Fixed(affine),
                    None => AffinePolicy::FitWithHeadroom,
                },
            },
            None => Self {
                world_dims: Some((world_w, world_h)),
                mercator: Some(projection),
                pad_blocks: 0,
                affine: AffinePolicy::Fit,
            },
        }
    }

    /// Absolute block of the area's north-west corner, where its grids start.
    fn origin(&self, bbox: &LLBBox) -> (i32, i32) {
        self.mercator.as_ref().map_or((0, 0), |proj| {
            (
                crate::projection::snap_edge(proj.x_for_lon(bbox.min().lng()), false),
                crate::projection::snap_edge(proj.z_for_lat(bbox.max().lat()), false),
            )
        })
    }

    /// Ecoregions over the area, at the geography the land cover is laid out on.
    fn ecoregions(&self, bbox: &LLBBox, (world_w, world_h): (usize, usize)) -> Option<Arc<EcoMap>> {
        let map = match &self.mercator {
            Some(proj) => {
                let (x0, z0) = self.origin(bbox);
                EcoMap::build(world_w, world_h, (x0, z0), |gx, gz| {
                    (
                        proj.lat_for_z(f64::from(z0) + gz),
                        proj.lon_for_x(f64::from(x0) + gx),
                    )
                })
            }
            None => {
                let (top, left) = (bbox.max().lat(), bbox.min().lng());
                let (dlat, dlon) = (top - bbox.min().lat(), bbox.max().lng() - left);
                let (w, h) = (world_w as f64, world_h as f64);
                EcoMap::build(world_w, world_h, (0, 0), move |gx, gz| {
                    (top - gz / h * dlat, left + gx / w * dlon)
                })
            }
        };
        map.map(Arc::new)
    }

    /// The padding is dropped when the padded grid would be capped, since the
    /// crop is only exact at one cell per block.
    fn fetch_plan(&self, bbox: &LLBBox, scale: f64) -> FetchPlan {
        let Some((world_w, world_h)) = self.world_dims else {
            let (ww, wh, gw, gh) = compute_grid_dims(bbox, scale);
            return FetchPlan {
                bbox: *bbox,
                dims: (ww, wh, gw, gh),
                pad: 0,
                final_dims: (ww, wh),
            };
        };
        let pad = self.pad_blocks;
        let (pw, ph) = (world_w + 2 * pad, world_h + 2 * pad);
        let (_, _, gw, gh) = compute_grid_dims_for_world(pw, ph);
        if let (Some(proj), true) = (&self.mercator, gw == pw && gh == ph) {
            // Providers sample a bbox edge to edge, so with one cell per block a
            // bbox running from first to last block centre samples every block
            // at its centre.
            let x0 = crate::projection::snap_edge(proj.x_for_lon(bbox.min().lng()), false) as f64
                - pad as f64
                + 0.5;
            let z0 = crate::projection::snap_edge(proj.z_for_lat(bbox.max().lat()), false) as f64
                - pad as f64
                + 0.5;
            let (x1, z1) = (x0 + (pw - 1) as f64, z0 + (ph - 1) as f64);
            if let Ok(centres) = LLBBox::new(
                proj.lat_for_z(z1),
                proj.lon_for_x(x0),
                proj.lat_for_z(z0),
                proj.lon_for_x(x1),
            ) {
                return FetchPlan {
                    bbox: centres,
                    dims: (pw, ph, gw, gh),
                    pad,
                    final_dims: (world_w, world_h),
                };
            }
        }
        if pad > 0 {
            println!(
                "Note: this area is too wide for the seam margin, so terrain near its edges may \
                 not match the neighbouring areas exactly."
            );
        }
        let (_, _, gw, gh) = compute_grid_dims_for_world(world_w, world_h);
        FetchPlan {
            bbox: *bbox,
            dims: (world_w, world_h, gw, gh),
            pad: 0,
            final_dims: (world_w, world_h),
        }
    }
}

struct FetchPlan {
    bbox: LLBBox,
    /// `(world_width, world_height, grid_width, grid_height)`
    dims: (usize, usize, usize, usize),
    pad: usize,
    final_dims: (usize, usize),
}

impl FetchPlan {
    fn grid_dims(&self) -> (usize, usize) {
        (self.dims.2, self.dims.3)
    }
}

/// Climatic snow line in metres by absolute latitude, piecewise-linear through
/// the cited anchors: equator 4500, subtropics (25 deg) 5700, mid-latitudes
/// (46 deg) 3000, poles 0. Source: Wikipedia "Snow line".
fn snow_line_meters(lat_deg: f64) -> f64 {
    let a = lat_deg.abs().min(90.0);
    if a <= 25.0 {
        4500.0 + (5700.0 - 4500.0) * (a / 25.0)
    } else if a <= 46.0 {
        5700.0 + (3000.0 - 5700.0) * ((a - 25.0) / (46.0 - 25.0))
    } else {
        (3000.0 * (1.0 - (a - 46.0) / (90.0 - 46.0))).max(0.0)
    }
}

/// Minecraft Y threshold for the snow line at this latitude, inverting the
/// affine metre->Y scaling. Returns `i32::MAX` (never) / `i32::MIN` (always)
/// for the flat-terrain extremes.
fn snow_threshold_for(ed: &ElevationData, lat_deg: f64, ground_level: i32) -> i32 {
    let snowline = snow_line_meters(lat_deg);
    if ed.blocks_per_meter <= 0.0 {
        return if ed.min_height_m >= snowline {
            i32::MIN
        } else {
            i32::MAX
        };
    }
    let affine = ElevationAffine {
        ground_level,
        ..ed.affine()
    };
    affine.y_for_metres(snowline).round() as i32
}

impl Ground {
    /// Terrain base actually in use. Differs from `args.ground_level` when the elevation
    /// scaler sank the base to reach the extended floor, so anything inverting the
    /// metre->Y affine (snow line, montane trees, filler chunks) must read it from here.
    pub fn base_level(&self) -> i32 {
        self.ground_level
    }

    #[cfg(test)]
    pub fn new_flat(ground_level: i32) -> Self {
        Self {
            elevation_enabled: false,
            extended_ceiling: false,
            ground_level,
            elevation_data: None,
            land_cover: None,
            canopy: None,
            world_width: 0,
            world_height: 0,
            rotation_mask: None,
            snow_threshold_y: i32::MAX,
            climate: crate::climate::Climate::Temperate,
            body: CelestialBody::Earth,
            ecoregions: None,
            winter: false,
            origin: (0, 0),
        }
    }

    /// Flat ground (no elevation) that still carries land cover, so water bodies and land-cover surfaces render at the flat surface level.
    pub fn new_flat_with_land_cover(
        bbox: &LLBBox,
        scale: f64,
        ground_level: i32,
        canopy_height: bool,
        frame: &GroundFrame,
    ) -> Self {
        let plan = frame.fetch_plan(bbox, scale);
        let fetch_bbox = plan.bbox;
        let (grid_w, grid_h) = plan.grid_dims();
        // Canopy depends on neither, so it downloads alongside the land cover.
        let (mut land_cover, mut canopy) = std::thread::scope(|s| {
            let job = canopy_height
                .then(|| s.spawn(move || canopy::fetch_canopy_data(&fetch_bbox, grid_w, grid_h)));
            let lc = land_cover::fetch_land_cover_data(&fetch_bbox, grid_w, grid_h);
            (lc, job.and_then(|h| h.join().ok()).flatten())
        });
        if land_cover.is_none() {
            eprintln!("Land cover fetch failed; generating flat ground without it.");
        }
        if frame.mercator.is_some() {
            if let Some(lc) = land_cover.as_mut() {
                lc.remap_rows_to_mercator(fetch_bbox.max().lat(), fetch_bbox.min().lat());
            }
            if let Some(c) = canopy.as_mut() {
                c.remap_rows_to_mercator(fetch_bbox.max().lat(), fetch_bbox.min().lat());
            }
        }
        let (world_w, world_h) = plan.final_dims;
        if plan.pad > 0 {
            if let Some(lc) = land_cover.as_mut() {
                lc.crop(plan.pad, plan.pad, world_w, world_h);
            }
            if let Some(c) = canopy.as_mut() {
                c.crop(plan.pad, plan.pad, world_w, world_h);
            }
        }
        Self {
            elevation_enabled: false,
            extended_ceiling: false,
            ground_level,
            elevation_data: None,
            land_cover,
            canopy,
            world_width: world_w,
            world_height: world_h,
            rotation_mask: None,
            snow_threshold_y: i32::MAX,
            climate: crate::climate::Climate::classify(bbox),
            body: CelestialBody::Earth,
            ecoregions: frame.ecoregions(bbox, (world_w, world_h)),
            winter: false,
            origin: frame.origin(bbox),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_flat_land_cover_test(
        land_cover: LandCoverData,
        world_width: usize,
        world_height: usize,
    ) -> Self {
        Self {
            elevation_enabled: false,
            extended_ceiling: false,
            ground_level: 0,
            elevation_data: None,
            land_cover: Some(land_cover),
            canopy: None,
            world_width,
            world_height,
            rotation_mask: None,
            snow_threshold_y: i32::MAX,
            climate: crate::climate::Climate::Temperate,
            body: CelestialBody::Earth,
            ecoregions: None,
            winter: false,
            origin: (0, 0),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_flat_canopy_test(
        canopy: CanopyData,
        world_width: usize,
        world_height: usize,
    ) -> Self {
        Self {
            elevation_enabled: false,
            extended_ceiling: false,
            ground_level: 0,
            elevation_data: None,
            land_cover: None,
            canopy: Some(canopy),
            world_width,
            world_height,
            rotation_mask: None,
            snow_threshold_y: i32::MAX,
            climate: crate::climate::Climate::Temperate,
            body: CelestialBody::Earth,
            ecoregions: None,
            winter: false,
            origin: (0, 0),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_elevation_test(
        heights: Vec<Vec<f32>>,
        world_width: usize,
        world_height: usize,
    ) -> Self {
        let grid_height = heights.len();
        let grid_width = heights.first().map(Vec::len).unwrap_or(0);
        assert!(
            grid_height > 0 && grid_width > 0,
            "heights must be non-empty"
        );
        assert!(
            world_width > 0 && world_height > 0,
            "world dims must be > 0"
        );
        assert!(
            heights.iter().all(|r| r.len() == grid_width),
            "heights must be rectangular"
        );
        Self {
            elevation_enabled: true,
            extended_ceiling: false,
            ground_level: 0,
            elevation_data: Some(crate::elevation::ElevationData {
                heights,
                width: grid_width,
                height: grid_height,
                world_width,
                world_height,
                min_height_m: 0.0,
                blocks_per_meter: 1.0,
                slope_correction: 1.0,
                ground_level: 0,
                soft_top: None,
                extended_ceiling: false,
                halo: None,
            }),
            land_cover: None,
            canopy: None,
            world_width,
            world_height,
            rotation_mask: None,
            snow_threshold_y: i32::MAX,
            climate: crate::climate::Climate::Temperate,
            body: CelestialBody::Earth,
            ecoregions: None,
            winter: false,
            origin: (0, 0),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_enabled(
        bbox: &LLBBox,
        scale: f64,
        height_multiplier: f64,
        ground_level: i32,
        height: WorldHeight,
        taller: Option<WorldHeight>,
        aws_only_elevation: bool,
        benchmark: bool,
        canopy_height: bool,
        body: CelestialBody,
        frame: &GroundFrame,
        peaks: &crate::elevation::PeakSource,
    ) -> Self {
        let mut bench = crate::bench::Bench::new(benchmark);
        // Land cover, canopy and the snow line are Earth datasets keyed by
        // terrestrial lat/lon, so off Earth they return plausible nonsense.
        let canopy_height = canopy_height && body.is_earth();
        // Fetch land cover FIRST so we can feed it into the elevation
        // post-processing pipeline for land-cover-aware artifact repair.
        // The elevation grid is built from the same (bbox, scale) so both
        // grids share dimensions (both use compute_grid_dims).
        let plan = frame.fetch_plan(bbox, scale);
        let requested_bbox = *bbox;
        let bbox = &plan.bbox;
        let (world_w, world_h, grid_w, grid_h) = plan.dims;
        // Canopy needs neither of the other two, so it downloads behind both.
        std::thread::scope(|scope| {
            let canopy_job = canopy_height
                .then(|| scope.spawn(|| canopy::fetch_canopy_data(bbox, grid_w, grid_h)));
            let mut land_cover = if body.is_earth() {
                let lc = land_cover::fetch_land_cover_data(bbox, grid_w, grid_h);
                if lc.is_some() {
                    println!("Land cover data loaded successfully");
                } else {
                    eprintln!("Warning: Land cover data unavailable, using default ground blocks");
                }
                lc
            } else {
                None
            };
            bench.mark("elev_landcover_fetch");

            // Raise the floor for the deepest water carve (elevation path only).
            let max_depth = land_cover
                .as_ref()
                .map(|lc| crate::water_depth::estimate_max_carve_depth(&lc.grid, world_w, world_h));
            let band = |h: WorldHeight| {
                let carve_floor = match max_depth {
                    Some(depth) => h.min_y + depth + 2,
                    None => h.min_y,
                };
                let water_floor = ground_level.max(carve_floor);
                // The terrain may sink to reach an extended floor, but never below the carve
                // floor: water would otherwise be cut straight through the bedrock layer.
                crate::elevation::HeightBand {
                    ground_level: water_floor,
                    min_ground_level: h.min_ground_level.max(carve_floor).min(water_floor),
                    disable_height_limit: h.extended,
                    extended_max_y: h.extended_max_y,
                }
            };

            let source_mode = if !body.is_earth() {
                crate::elevation::SourceMode::Planetary(body)
            } else if aws_only_elevation {
                crate::elevation::SourceMode::AwsOnly
            } else {
                crate::elevation::SourceMode::Auto
            };
            match fetch_elevation_data(
                bbox,
                scale,
                height_multiplier,
                band(height),
                taller.map(band),
                land_cover.as_mut(),
                source_mode,
                benchmark,
                (world_w, world_h, grid_w, grid_h),
                frame.affine,
                peaks,
                (frame.pad_blocks > 0).then_some(plan.pad),
            ) {
                Ok(mut elevation_data) => {
                    let lat = (requested_bbox.min().lat() + requested_bbox.max().lat()) / 2.0;
                    // Must use the base the scaler actually settled on: snow_threshold_for
                    // inverts that exact affine, so a mismatched base misplaces every snow cap.
                    let base = elevation_data.ground_level;
                    let snow_threshold_y = if body.is_earth() {
                        snow_threshold_for(&elevation_data, lat, base)
                    } else {
                        i32::MAX
                    };
                    let mut canopy = canopy_job.and_then(|h| h.join().ok()).flatten();
                    // Providers sample equal latitude steps.
                    if frame.mercator.is_some() {
                        let (top, bottom) = (bbox.max().lat(), bbox.min().lat());
                        elevation_data.remap_rows_to_mercator(top, bottom);
                        if let Some(lc) = land_cover.as_mut() {
                            lc.remap_rows_to_mercator(top, bottom);
                        }
                        if let Some(c) = canopy.as_mut() {
                            c.remap_rows_to_mercator(top, bottom);
                        }
                    }
                    let (final_w, final_h) = plan.final_dims;
                    if plan.pad > 0 {
                        elevation_data.crop(plan.pad, plan.pad, final_w, final_h);
                        if let Some(lc) = land_cover.as_mut() {
                            lc.crop(plan.pad, plan.pad, final_w, final_h);
                        }
                        if let Some(c) = canopy.as_mut() {
                            c.crop(plan.pad, plan.pad, final_w, final_h);
                        }
                    }
                    Self {
                        elevation_enabled: true,
                        extended_ceiling: elevation_data.extended_ceiling,
                        ground_level: base,
                        elevation_data: Some(elevation_data),
                        land_cover,
                        canopy,
                        world_width: final_w,
                        world_height: final_h,
                        rotation_mask: None,
                        snow_threshold_y,
                        climate: crate::climate::Climate::classify(&requested_bbox),
                        body,
                        ecoregions: body
                            .is_earth()
                            .then(|| frame.ecoregions(&requested_bbox, (final_w, final_h)))
                            .flatten(),
                        winter: false,
                        origin: frame.origin(&requested_bbox),
                    }
                }
                Err(e) => {
                    eprintln!("Failed to fetch elevation data: {}", e);
                    #[cfg(feature = "gui")]
                    {
                        let short: String = e.to_string().chars().take(200).collect();
                        send_log(
                            LogLevel::Warning,
                            &format!("Elevation unavailable, using flat ground ({short})"),
                        );
                    }
                    // Graceful fallback: disable elevation and keep provided ground_level.
                    // Land cover we already fetched is discarded since it has no
                    // elevation grid to align against.
                    // Still has to be collected before the scope can close.
                    drop(canopy_job.and_then(|h| h.join().ok()));
                    Self {
                        elevation_enabled: false,
                        extended_ceiling: false,
                        ground_level,
                        elevation_data: None,
                        land_cover: None,
                        canopy: None,
                        world_width: 0,
                        world_height: 0,
                        rotation_mask: None,
                        snow_threshold_y: i32::MAX,
                        climate: crate::climate::Climate::classify(&requested_bbox),
                        body,
                        ecoregions: body
                            .is_earth()
                            .then(|| frame.ecoregions(&requested_bbox, plan.final_dims))
                            .flatten(),
                        winter: false,
                        origin: frame.origin(&requested_bbox),
                    }
                }
            }
        })
    }

    /// Minecraft Y at/above which terrain is snow-capped (`i32::MAX` = never,
    /// `i32::MIN` = always, e.g. a flat plateau above the snow line).
    #[inline(always)]
    pub fn snow_threshold_y(&self) -> i32 {
        self.snow_threshold_y
    }

    /// Climate at the bbox center (Temperate keeps the existing surface/biome behaviour).
    #[inline(always)]
    pub fn climate(&self) -> crate::climate::Climate {
        self.climate
    }

    /// Body this world is on; Earth keeps every existing behaviour.
    #[inline(always)]
    pub fn body(&self) -> CelestialBody {
        self.body
    }

    /// RESOLVE ecoregion under a ground coordinate, if the area has one there.
    #[inline]
    pub fn ecoregion(&self, coord: XZPoint) -> Option<Ecoregion> {
        self.ecoregions.as_ref()?.at(coord)
    }

    pub fn ecoregion_map(&self) -> Option<&EcoMap> {
        self.ecoregions.as_deref()
    }

    /// Replaces the ecoregion map after the ground was rotated.
    pub fn set_ecoregion_map(&mut self, map: EcoMap) {
        self.ecoregions = Some(Arc::new(map));
    }

    /// Whether this is a winter run, whose biomes are all snowy.
    #[inline(always)]
    pub fn winter(&self) -> bool {
        self.winter
    }

    pub fn set_winter(&mut self, winter: bool) {
        self.winter = winter;
    }

    /// Returns whether land cover data is available
    #[inline(always)]
    pub fn has_land_cover(&self) -> bool {
        self.land_cover.is_some()
    }

    /// Returns whether canopy height data is available.
    #[inline(always)]
    pub fn has_canopy(&self) -> bool {
        self.canopy.is_some()
    }

    /// Canopy top in metres, or `None` where the map has no measurement.
    /// A measured zero is bare ground, not missing.
    #[inline(always)]
    pub fn canopy_height_m(&self, coord: XZPoint) -> Option<u8> {
        let ch = self.canopy.as_ref()?;
        let (world_w, world_h) = self.world_dims();
        let x_ratio = (coord.x as f64 / (world_w - 1).max(1) as f64).clamp(0.0, 1.0);
        let z_ratio = (coord.z as f64 / (world_h - 1).max(1) as f64).clamp(0.0, 1.0);
        let x = ((x_ratio * (ch.width - 1) as f64).round() as usize).min(ch.width - 1);
        let z = ((z_ratio * (ch.height - 1) as f64).round() as usize).min(ch.height - 1);
        match ch.at(x, z) {
            canopy::CANOPY_NODATA => None,
            h => Some(h),
        }
    }

    /// Share of a `span` square at `origin` that carries canopy, or `None` where
    /// nothing in it was measured. Unmeasured columns stay out of the average,
    /// so a cell with none at all hands the decision back to the land cover.
    #[cfg(test)]
    pub fn canopy_fraction(&self, origin: XZPoint, span: i32) -> Option<f64> {
        self.canopy_cell(origin, span).map(|(fraction, _)| fraction)
    }

    /// Whether `canopy_cell` would answer, stopping at the first measured column.
    pub fn canopy_measured(&self, origin: XZPoint, span: i32) -> bool {
        (0..span).any(|dz| {
            (0..span).any(|dx| {
                self.canopy_height_m(XZPoint::new(origin.x + dx, origin.z + dz))
                    .is_some()
            })
        })
    }

    /// The canopy share of a `span` square at `origin` and its tallest crown in
    /// metres, or `None` where nothing in it was measured.
    pub fn canopy_cell(&self, origin: XZPoint, span: i32) -> Option<(f64, u8)> {
        self.canopy.as_ref()?;
        if span <= 0 {
            return None;
        }
        let (mut measured, mut wooded, mut top) = (0u32, 0u32, 0u8);
        for dz in 0..span {
            for dx in 0..span {
                if let Some(h) = self.canopy_height_m(XZPoint::new(origin.x + dx, origin.z + dz)) {
                    measured += 1;
                    if h >= canopy::CANOPY_MIN_M {
                        wooded += 1;
                        top = top.max(h);
                    }
                }
            }
        }
        (measured > 0).then(|| (f64::from(wooded) / f64::from(measured), top))
    }

    /// Force LC_WATER inside OSM water, sinking those cells onto that water's surface.
    pub fn apply_osm_water_override(&mut self, elements: &[ProcessedElement], xzbbox: &XZBBox) {
        let Ground {
            land_cover,
            elevation_data,
            ..
        } = self;
        let (Some(lc), Some(data)) = (land_cover.as_mut(), elevation_data.as_mut()) else {
            return;
        };
        let (world_width, world_height) = (data.world_width, data.world_height);
        crate::land_cover::osm_water_override::apply_osm_water_override(
            lc,
            &mut data.heights,
            world_width,
            world_height,
            elements,
            xzbbox,
        );
    }

    /// Trim ESA water back to land where OSM shows roads, buildings or its own shoreline.
    pub fn apply_osm_land_override(
        &mut self,
        elements: &[ProcessedElement],
        xzbbox: &XZBBox,
        scale: f64,
    ) {
        let (world_width, world_height) = self.world_dims();
        let Some(lc) = self.land_cover.as_mut() else {
            return;
        };
        crate::land_cover::osm_land_override::apply_osm_land_override(
            lc,
            world_width,
            world_height,
            elements,
            xzbbox,
            scale,
        );
    }

    /// Last land cover pass, once the OSM and bridge repairs have moved the water: returns
    /// built-up land and fields to nature where the mode asks, then marks the beaches.
    pub fn finish_land_cover(&mut self, mode: GenerationMode) {
        if mode.natural_land_cover() {
            // A blueprint still draws roads and outlines on narrow land by the water.
            self.naturalize_land_cover(mode.skip_objects());
        }
        self.mark_beaches();
    }

    /// Returns built-up land and fields to natural cover, for terrain-only worlds.
    fn naturalize_land_cover(&mut self, reclaim: bool) {
        let (climate, origin) = (self.climate, self.origin);
        let Ground {
            land_cover,
            elevation_data,
            ..
        } = self;
        let Some(lc) = land_cover.as_mut() else {
            return;
        };
        let heights = elevation_data.as_mut().map(|d| d.heights.as_mut_slice());
        crate::land_cover::naturalize::naturalize(lc, heights, climate, origin, reclaim);
    }

    /// Marks bare ground at the water's edge as beach. Run after every pass that moves water.
    pub fn mark_beaches(&mut self) {
        if let Some(lc) = self.land_cover.as_mut() {
            land_cover::mark_beaches(lc);
        }
    }

    /// Reclassify cells under bridges to the surrounding class, sinking new water.
    pub fn apply_bridge_land_cover_repair(
        &mut self,
        elements: &[ProcessedElement],
        xzbbox: &XZBBox,
        scale: f64,
    ) {
        let Ground {
            land_cover,
            elevation_data,
            ..
        } = self;
        let (Some(lc), Some(data)) = (land_cover.as_mut(), elevation_data.as_mut()) else {
            return;
        };
        let (world_width, world_height) = (data.world_width, data.world_height);
        crate::land_cover::bridge_repair::apply_bridge_land_cover_repair(
            lc,
            &mut data.heights,
            world_width,
            world_height,
            elements,
            xzbbox,
            scale,
        );
    }

    /// Local block bbox (min_x, min_z, max_x, max_z) covering all LC_WATER cells,
    /// derived from the land-cover grid; None if no land cover or no water.
    pub fn lc_water_block_bounds(&self) -> Option<(i32, i32, i32, i32)> {
        let (lc, data) = match (&self.land_cover, &self.elevation_data) {
            (Some(lc), Some(data)) => (lc, data),
            _ => return None,
        };
        let (mut gx0, mut gz0, mut gx1, mut gz1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        let mut any = false;
        for (z, row) in lc.grid.iter().enumerate() {
            for (x, &c) in row.iter().enumerate() {
                if c == land_cover::LC_WATER {
                    gx0 = gx0.min(x);
                    gx1 = gx1.max(x);
                    gz0 = gz0.min(z);
                    gz1 = gz1.max(z);
                    any = true;
                }
            }
        }
        if !any {
            return None;
        }
        let (x0, x1) =
            crate::water_depth::grid_span_to_block_span(gx0, gx1, data.world_width, lc.width);
        let (z0, z1) =
            crate::water_depth::grid_span_to_block_span(gz0, gz1, data.world_height, lc.height);
        Some((x0, z0, x1, z1))
    }

    /// World size in blocks for land-cover mapping: from elevation data when present, else the stored flat-mode dims.
    #[inline(always)]
    pub(crate) fn world_dims(&self) -> (usize, usize) {
        match &self.elevation_data {
            Some(d) => (d.world_width, d.world_height),
            None => (self.world_width, self.world_height),
        }
    }

    /// Returns the ESA WorldCover land cover class at the given coordinates.
    /// Returns 0 if land cover data is not available.
    #[inline(always)]
    pub fn cover_class(&self, coord: XZPoint) -> u8 {
        if let Some(ref lc) = self.land_cover {
            let (world_w, world_h) = self.world_dims();
            let x_ratio = (coord.x as f64 / (world_w - 1).max(1) as f64).clamp(0.0, 1.0);
            let z_ratio = (coord.z as f64 / (world_h - 1).max(1) as f64).clamp(0.0, 1.0);
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
            let (world_w, world_h) = self.world_dims();
            let x_ratio = (coord.x as f64 / (world_w - 1).max(1) as f64).clamp(0.0, 1.0);
            let z_ratio = (coord.z as f64 / (world_h - 1).max(1) as f64).clamp(0.0, 1.0);
            let x = ((x_ratio * (lc.width - 1) as f64).round() as usize).min(lc.width - 1);
            let z = ((z_ratio * (lc.height - 1) as f64).round() as usize).min(lc.height - 1);
            lc.water_distance[z][x]
        } else {
            0
        }
    }

    /// True for a water cell at least four cells from shore. `water_distance` is 0 past
    /// its cap of 15, so 0 counts as interior here. Tells a step inside a body from a bank.
    #[inline(always)]
    pub fn is_interior_water(&self, coord: XZPoint) -> bool {
        if self.cover_class(coord) != land_cover::LC_WATER {
            return false;
        }
        let d = self.water_distance(coord);
        d == 0 || d >= 4
    }

    /// Whether the Gaussian-smoothed water mask, sampled bilinearly, makes this block water.
    /// Noise-free thresholding of the smooth field traces organic shorelines where
    /// nearest-neighbour `cover_class` lookups would leave rectangular grid edges.
    #[inline(always)]
    pub fn water_blend_is_water(&self, coord: XZPoint) -> bool {
        match self.land_cover {
            Some(ref lc) => {
                let (world_w, world_h) = self.world_dims();
                lc.water_blend_mask(world_w, world_h)
                    .contains(coord.x, coord.z)
            }
            None => false,
        }
    }

    /// Computes terrain slope at the given coordinates.
    ///
    /// Slope is the difference between the maximum and minimum elevation of
    /// 4 cardinal neighbors sampled at a step distance, corrected back out of
    /// compressed block space into the `8 * tan(incline)` units the thresholds
    /// downstream are documented in. Higher values indicate steeper terrain.
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
        // Saturate: pathological CLI input (e.g. very negative ground_level)
        // can push max - min past i32::MAX.
        let raw = max_val.saturating_sub(min_val);
        let correction = match &self.elevation_data {
            Some(d) => d.slope_correction * d.soft_top_stretch(min_val.saturating_add(raw / 2)),
            None => 1.0,
        };
        (raw as f64 * correction).round() as i32
    }

    /// `slope` from unrounded heights. Same units, but without the jitter of
    /// whole-block steps, which flips columns along a contour between the
    /// material tiers and draws stripes on hillsides. Also returns the rise across
    /// the column along x and along z from the same samples, in blocks.
    pub fn slope_and_gradient(&self, coord: XZPoint) -> (f64, (f64, f64)) {
        if !self.elevation_enabled {
            return (0.0, (0.0, 0.0));
        }

        const STEP: i32 = 4;
        let samples = [
            self.level_exact(XZPoint::new(coord.x + STEP, coord.z)),
            self.level_exact(XZPoint::new(coord.x - STEP, coord.z)),
            self.level_exact(XZPoint::new(coord.x, coord.z - STEP)),
            self.level_exact(XZPoint::new(coord.x, coord.z + STEP)),
        ];
        let max_val = samples.iter().copied().fold(f64::MIN, f64::max);
        let min_val = samples.iter().copied().fold(f64::MAX, f64::min);
        let raw = max_val - min_val;
        let slope = (raw * self.slope_units(((min_val + max_val) * 0.5).round() as i32)).max(0.0);
        (slope, (samples[0] - samples[1], samples[3] - samples[2]))
    }

    /// How far the ground sits below its surroundings: the mean height eight
    /// blocks out minus the column's own, from unrounded heights, in `slope`
    /// units. Positive in hollows and gullies, negative on ridges and knolls.
    pub fn convexity(&self, coord: XZPoint) -> f64 {
        if !self.elevation_enabled {
            return 0.0;
        }

        const RADIUS: i32 = 8;
        const DIAGONAL: i32 = 6;
        let ring = [
            (RADIUS, 0),
            (-RADIUS, 0),
            (0, RADIUS),
            (0, -RADIUS),
            (DIAGONAL, DIAGONAL),
            (DIAGONAL, -DIAGONAL),
            (-DIAGONAL, DIAGONAL),
            (-DIAGONAL, -DIAGONAL),
        ];
        let center = self.level_exact(coord);
        let mean = ring
            .iter()
            .map(|&(dx, dz)| self.level_exact(XZPoint::new(coord.x + dx, coord.z + dz)))
            .sum::<f64>()
            / ring.len() as f64;
        // Slope units count the rise over four blocks, the ring sits twice as far.
        (mean - center) * 0.5 * self.slope_units(center.round() as i32)
    }

    /// Converts a block-space height difference into the `8 * tan(incline)`
    /// units the slope thresholds are written in.
    fn slope_units(&self, y: i32) -> f64 {
        match &self.elevation_data {
            Some(d) => d.slope_correction * d.soft_top_stretch(y),
            None => 1.0,
        }
    }

    pub fn elevation_affine(&self) -> Option<ElevationAffine> {
        if !self.elevation_enabled {
            return None;
        }
        self.elevation_data.as_ref().map(|d| d.affine())
    }

    /// Turns a height step in blocks over a run in blocks into the real incline.
    pub(crate) fn slope_correction(&self) -> f64 {
        self.elevation_data
            .as_ref()
            .map_or(1.0, |d| d.slope_correction)
    }

    /// Vertical blocks per real-world metre, 1.0 without elevation (or with zero
    /// relief), so callers inverting the metre->Y affine can divide unconditionally.
    #[inline(always)]
    pub fn blocks_per_meter(&self) -> f64 {
        match &self.elevation_data {
            Some(d) if self.elevation_enabled && d.blocks_per_meter > 0.0 => d.blocks_per_meter,
            _ => 1.0,
        }
    }

    /// Returns the ground level at the given coordinates
    #[inline(always)]
    pub fn level(&self, coord: XZPoint) -> i32 {
        if !self.elevation_enabled || self.elevation_data.is_none() {
            return self.ground_level;
        }

        let data: &ElevationData = self.elevation_data.as_ref().unwrap();
        if let Some(h) = data.beyond(coord.x, coord.z) {
            return f64::from(h).round() as i32;
        }
        let (x_ratio, z_ratio) = self.get_data_coordinates(coord, data);
        self.interpolate_height(x_ratio, z_ratio, data)
    }

    /// `level` before rounding to a whole block.
    #[inline(always)]
    pub fn level_exact(&self, coord: XZPoint) -> f64 {
        match &self.elevation_data {
            Some(data) if self.elevation_enabled => {
                if let Some(h) = data.beyond(coord.x, coord.z) {
                    return f64::from(h);
                }
                let (x_ratio, z_ratio) = self.get_data_coordinates(coord, data);
                Self::interpolate_height_exact(x_ratio, z_ratio, data)
            }
            _ => f64::from(self.ground_level),
        }
    }

    /// Returns the appropriate Y level for water placement.
    /// On steep terrain, snaps to the local minimum within a small radius to
    /// correct spatial misalignment between water classification (OSM/ESA) and
    /// the elevation DEM. The snap is skipped across a real cliff/falls, where the
    /// cell keeps its own level so the waterfront isn't terraced into a step.
    pub fn water_level(&self, coord: XZPoint) -> i32 {
        let center = self.level(coord);
        if !self.elevation_enabled {
            return center;
        }
        // Check if terrain is steep here; if flat, no snapping needed
        let slope = self.slope(coord);
        if slope <= 2 {
            return center;
        }
        // On steep terrain, snap to the local minimum within SNAP_RADIUS to
        // correct small DEM-vs-water misalignment.
        const SNAP_RADIUS: i32 = 3;
        let mut min_y = center;
        for r in 1..=SNAP_RADIUS {
            for &(dx, dz) in &[
                (-r, 0),
                (r, 0),
                (0, -r),
                (0, r),
                (-r, -r),
                (-r, r),
                (r, -r),
                (r, r),
            ] {
                let neighbor = self.level(XZPoint::new(coord.x + dx, coord.z + dz));
                min_y = min_y.min(neighbor);
            }
        }
        // A real cliff/falls is not misalignment; snapping across it terraces the
        // waterfront into a step. Under a raised ceiling the drop is a metre-space concept,
        // so scale it by blocks-per-metre; the vanilla ceiling keeps the block-space radius
        // it was tuned against, where an 8-block quay wall is a wall and not a rounding error.
        // saturating_sub guards against overflow on pathological elevations.
        const CLIFF_DROP_M: f64 = 25.0;
        let cliff_drop = if self.extended_ceiling {
            ((CLIFF_DROP_M * self.blocks_per_meter()).round() as i32).max(SNAP_RADIUS)
        } else {
            SNAP_RADIUS
        };
        if center.saturating_sub(min_y) > cliff_drop {
            return center;
        }
        min_y
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

    /// Converts game coordinates to elevation data coordinates (0.0 to 1.0 ratio)
    #[inline(always)]
    fn get_data_coordinates(&self, coord: XZPoint, data: &ElevationData) -> (f64, f64) {
        let x_ratio: f64 = coord.x as f64 / (data.world_width - 1).max(1) as f64;
        let z_ratio: f64 = coord.z as f64 / (data.world_height - 1).max(1) as f64;
        (x_ratio.clamp(0.0, 1.0), z_ratio.clamp(0.0, 1.0))
    }

    /// Bilinearly interpolates height value from the elevation grid
    #[inline(always)]
    fn interpolate_height(&self, x_ratio: f64, z_ratio: f64, data: &ElevationData) -> i32 {
        Self::interpolate_height_exact(x_ratio, z_ratio, data).round() as i32
    }

    #[inline(always)]
    fn interpolate_height_exact(x_ratio: f64, z_ratio: f64, data: &ElevationData) -> f64 {
        let fx = x_ratio * (data.width - 1) as f64;
        let fz = z_ratio * (data.height - 1) as f64;
        let x0 = fx.floor() as usize;
        let z0 = fz.floor() as usize;
        let x1 = (x0 + 1).min(data.width - 1);
        let z1 = (z0 + 1).min(data.height - 1);
        let dx = fx - x0 as f64;
        let dz = fz - z0 as f64;
        // Widen f32 storage to f64 for the bilinear arithmetic. The real
        // property we rely on: across the Minecraft Y range (roughly −64 up
        // through a few thousand even with --disable-height-limit), f32's
        // mantissa gives ~10⁻⁷ precision per stored cell, which is far
        // smaller than the 0.5-block half-width used by `round()` below.
        // So for any value that isn't pathologically close to a half-integer
        // boundary, the final `result.round() as i32` matches the f64 path.
        let v00 = data.heights[z0][x0] as f64;
        let v10 = data.heights[z0][x1] as f64;
        let v01 = data.heights[z1][x0] as f64;
        let v11 = data.heights[z1][x1] as f64;
        let lerp_top = v00 + (v10 - v00) * dx;
        let lerp_bot = v01 + (v11 - v01) * dx;
        lerp_top + (lerp_bot - lerp_top) * dz
    }

    /// Replace the elevation grid with new rotated/transformed data.
    /// Used by the rotation operator to update elevation after rotating.
    pub fn set_elevation_data(
        &mut self,
        heights: Vec<Vec<f64>>,
        grid_width: usize,
        grid_height: usize,
        world_width: usize,
        world_height: usize,
    ) {
        if let Some(ref mut data) = self.elevation_data {
            // Rotation operators build a fresh f64 work grid; downcast here to
            // match `ElevationData::heights`'s f32 storage layout.
            data.heights = heights
                .into_iter()
                .map(|row| row.into_iter().map(|v| v as f32).collect())
                .collect();
            data.width = grid_width;
            data.height = grid_height;
            data.world_width = world_width;
            data.world_height = world_height;
            data.halo = None;
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
            // The water-blend mask was derived from the pre-rotation grid —
            // refresh it from the rotated grid so the shoreline softening
            // stays aligned with the new classification.
            lc.invalidate_water_blend_grid();
        }
    }

    /// Replace the canopy grid after a rotation resamples it.
    pub fn set_canopy_data(&mut self, grid: Vec<u8>, width: usize, height: usize) {
        if self.canopy.is_some() {
            self.canopy = Some(CanopyData::from_grid(grid, width, height));
        }
    }

    /// Computes the lazy water-blend mask now; all grid mutations must be done.
    pub fn warm_water_blend(&self) {
        if let Some(ref lc) = self.land_cover {
            let (world_w, world_h) = self.world_dims();
            let _ = lc.water_blend_mask(world_w, world_h);
        }
    }

    /// Update the stored world size after a rotation resizes the bbox, so flat-mode land-cover lookups stay aligned.
    pub fn set_world_dims(&mut self, world_width: usize, world_height: usize) {
        self.world_width = world_width;
        self.world_height = world_height;
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

    pub fn save_land_cover_debug_image(&self, filename: &str) {
        let Some(ref lc) = self.land_cover else {
            return;
        };
        if lc.height == 0 || lc.width == 0 {
            return;
        }
        let mut img: image::ImageBuffer<Rgb<u8>, Vec<u8>> =
            RgbImage::new(lc.width as u32, lc.height as u32);
        for (y, row) in lc.grid.iter().enumerate() {
            for (x, &class) in row.iter().enumerate() {
                let color = match class {
                    land_cover::LC_TREE_COVER => Rgb([0x00, 0x6e, 0x00]),
                    land_cover::LC_SHRUBLAND => Rgb([0xff, 0xbb, 0x22]),
                    land_cover::LC_GRASSLAND => Rgb([0xff, 0xff, 0x4c]),
                    land_cover::LC_CROPLAND => Rgb([0xf0, 0x96, 0xff]),
                    land_cover::LC_BUILT_UP => Rgb([0xfa, 0x00, 0x00]),
                    land_cover::LC_BARE => Rgb([0xb4, 0xb4, 0xb4]),
                    land_cover::LC_SNOW_ICE => Rgb([0xf0, 0xf0, 0xf0]),
                    land_cover::LC_WATER => Rgb([0x00, 0x64, 0xc8]),
                    land_cover::LC_WETLAND => Rgb([0x00, 0x96, 0xa0]),
                    land_cover::LC_MANGROVES => Rgb([0x00, 0xcf, 0x75]),
                    land_cover::LC_MOSS => Rgb([0xfa, 0xe6, 0xa0]),
                    land_cover::LC_BEACH => Rgb([0xe8, 0xd8, 0x9c]),
                    _ => Rgb([0x00, 0x00, 0x00]),
                };
                img.put_pixel(x as u32, y as u32, color);
            }
        }
        let filename: String = if !filename.ends_with(".png") {
            format!("{filename}.png")
        } else {
            filename.to_string()
        };
        if let Err(e) = img.save(&filename) {
            eprintln!("Failed to save land cover debug image: {e}");
        }
    }

    /// Grey ramp of canopy heights, black where nothing was measured.
    pub fn save_canopy_debug_image(&self, filename: &str) {
        let Some(ref ch) = self.canopy else {
            return;
        };
        if ch.height == 0 || ch.width == 0 {
            return;
        }
        let mut img: image::ImageBuffer<Rgb<u8>, Vec<u8>> =
            RgbImage::new(ch.width as u32, ch.height as u32);
        for z in 0..ch.height {
            for x in 0..ch.width {
                let color = match ch.at(x, z) {
                    canopy::CANOPY_NODATA => Rgb([0x00, 0x00, 0x00]),
                    h if h < canopy::CANOPY_MIN_M => Rgb([0x30, 0x30, 0x30]),
                    // 3 m to 40 m over the green ramp, saturating at the top.
                    h => {
                        let t = (f32::from(h) / 40.0).min(1.0);
                        Rgb([(64.0 * (1.0 - t)) as u8, (64.0 + 191.0 * t) as u8, 48])
                    }
                };
                img.put_pixel(x as u32, z as u32, color);
            }
        }
        let filename: String = if !filename.ends_with(".png") {
            format!("{filename}.png")
        } else {
            filename.to_string()
        };
        if let Err(e) = img.save(&filename) {
            eprintln!("Failed to save canopy debug image: {e}");
        }
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

        let mut min_height: f32 = f32::MAX;
        let mut max_height: f32 = f32::MIN;

        for row in heights {
            for &h in row {
                if h.is_finite() {
                    min_height = min_height.min(h);
                    max_height = max_height.max(h);
                }
            }
        }

        let range = max_height - min_height;
        for (y, row) in heights.iter().enumerate() {
            for (x, &h) in row.iter().enumerate() {
                let normalized: u8 = if range > 0.0 {
                    (((h - min_height) / range) * 255.0) as u8
                } else {
                    128
                };
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

/// `peaks` delivers OSM summits once the parallel OSM fetch has them.
pub fn generate_ground_data(
    args: &Args,
    bbox: LLBBox,
    peaks: &crate::elevation::PeakSource,
) -> Ground {
    // Cleared before the scaler publishes its own: in the GUI a previous run's terrain top
    // would misgrade this world's map preview.
    crate::world_editor::common::set_terrain_top_y(args.ground_level);
    let frame = GroundFrame::from_args(args, &bbox);
    let auto = auto_height(args);
    let (height, taller) = height_choice(args);
    if args.terrain() {
        println!("{} Fetching elevation...", "[3/7]".bold());
        let mut ground = Ground::new_enabled(
            &bbox,
            args.scale,
            args.height_multiplier,
            args.ground_level,
            height,
            taller,
            args.aws_only_elevation,
            args.benchmark,
            args.canopy_height,
            args.body,
            &frame,
            peaks,
        );
        if auto && !ground.elevation_enabled {
            ground.extended_ceiling = height.extended || !flat_fits_vanilla(args, &bbox, peaks);
        }
        if auto && ground.extended_ceiling {
            extend_world_bounds(args);
        }
        ground.set_winter(args.winter);
        // The scaler may have sunk the base to reach the extended floor. The bedrock plane and
        // the out-of-bbox filler chunks both key off that base, so pin them to it now.
        let floor = area_floor_for(&ground, args);
        crate::world_editor::set_base_chunk_y(floor);
        crate::world_editor::set_terrain_floor_y(floor);
        // A grass plane around a lunar crater would be the most visible thing in it.
        crate::world_editor::set_base_chunk_block(filler_block_for(args.body));
        if args.debug {
            ground.save_debug_image("elevation_debug");
            ground.save_land_cover_debug_image("landcover_debug");
            ground.save_canopy_debug_image("canopy_debug");
        }
        return ground;
    }
    println!("{} Fetching land cover...", "[3/7]".bold());
    let mut ground = Ground::new_flat_with_land_cover(
        &bbox,
        args.scale,
        args.ground_level,
        args.canopy_height,
        &frame,
    );
    if auto && (height.extended || !flat_fits_vanilla(args, &bbox, peaks)) {
        ground.extended_ceiling = true;
        extend_world_bounds(args);
    }
    ground.set_winter(args.winter);
    crate::world_editor::set_base_chunk_y(ground.base_level());
    crate::world_editor::set_terrain_floor_y(ground.base_level());
    crate::world_editor::set_base_chunk_block(filler_block_for(args.body));
    ground
}

/// The terrain base, except in a One World with the extended floor: its base is the
/// lowest land on Earth, so bedrock, the filler plane and montane trees follow the
/// area's own lowest point instead.
pub(crate) fn area_floor_for(ground: &Ground, args: &Args) -> i32 {
    match &ground.elevation_data {
        Some(d) if args.one_world_run.is_some() && args.disable_height_limit => {
            d.lowest_y().unwrap_or(ground.base_level())
        }
        _ => ground.base_level(),
    }
}

/// Surface block for the out-of-bbox filler plane that borders the world.
fn filler_block_for(body: CelestialBody) -> crate::block_definitions::Block {
    match body {
        CelestialBody::Earth => crate::block_definitions::GRASS_BLOCK,
        CelestialBody::Moon => crate::block_definitions::ANDESITE,
        CelestialBody::Mars => crate::block_definitions::RED_TERRACOTTA,
    }
}

/// Per-format build-height cap when the user opts into extended build height:
/// 2031 for the Java datapack, 512 for the Bedrock behavior pack, and the vanilla
/// ceiling for Luanti, which has no pack and whose spawn search only scans the
/// vanilla range. Must stay gated like `world_top_y_for` / `extended_min_y_for`.
pub(crate) fn extended_max_y_for(args: &Args) -> i32 {
    if args.bedrock {
        512
    } else if args.luanti {
        crate::world_editor::DEFAULT_MAX_Y
    } else {
        2031
    }
}

/// World floor. The bundled Java datapack already declares the full range the engine allows
/// (dimension_type min_y=-2032, height=4064), so the only thing keeping Arnis at -64 was the
/// old constant. Java only: the Bedrock behavior pack declares -512, but the LevelDB subchunk
/// writer is unverified below -64, and Luanti has no such pack at all.
/// Dimension ceiling actually declared to the engine: the tall datapack's 2031, the Bedrock
/// behavior pack's 512 (511 here, since an editor ceiling has to end a section), or vanilla's
/// 319. Must never sit below the ceiling the scaler aims at (`extended_max_y_for`), or the
/// block store drops the terrain the scaler just placed. On Java it also pairs with
/// `extended_min_y_for`: chunk serialization sizes heightmaps from the span between the two.
pub(crate) fn world_top_y_for(args: &Args) -> i32 {
    world_top_y(args, args.disable_height_limit)
}

/// `world_top_y_for` with the extended height on or off.
pub(crate) fn world_top_y(args: &Args, extended: bool) -> i32 {
    if !extended || args.luanti {
        crate::world_editor::DEFAULT_MAX_Y
    } else if args.bedrock {
        511
    } else {
        2031
    }
}

pub(crate) fn extended_min_y_for(args: &Args) -> i32 {
    world_min_y(args, args.disable_height_limit)
}

/// `extended_min_y_for` with the extended height on or off.
pub(crate) fn world_min_y(args: &Args, extended: bool) -> i32 {
    if extended && !args.bedrock && !args.luanti {
        -2032
    } else {
        crate::world_editor::DEFAULT_MIN_Y
    }
}

/// Lowest terrain base the elevation scaler may sink to, leaving room for the bedrock layer
/// beneath it (mirroring the vanilla -64 floor / -62 base relationship). With a vanilla floor
/// this returns the requested ground level, which disables the sink entirely — an explicit
/// --ground-level must not be silently overridden.
pub(crate) fn min_ground_level_for(args: &Args) -> i32 {
    min_ground_level(args, args.disable_height_limit)
}

fn min_ground_level(args: &Args, extended: bool) -> i32 {
    let floor = world_min_y(args, extended);
    if floor >= crate::world_editor::DEFAULT_MIN_Y {
        args.ground_level
    } else {
        floor + 2
    }
}

/// Whether the terrain fit picks this run's build height, extending it only where the area
/// needs more room than vanilla. Not when the user kept the vanilla height or forced the
/// pack, in a One World (its height is fixed when it is made), on Luanti (no pack) or off
/// Earth. Asked before the fit only: `settle_build_height` resolves it.
pub(crate) fn auto_height(args: &Args) -> bool {
    !args.vanilla_height
        && !args.disable_height_limit
        && !args.luanti
        && args.body.is_earth()
        && !args.one_world
        && args.one_world_run.is_none()
}

/// A build height the terrain can be fitted into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldHeight {
    /// Bottom of the dimension.
    pub min_y: i32,
    /// Lowest terrain base the scaler may sink to.
    pub min_ground_level: i32,
    pub extended: bool,
    /// Scaler ceiling when `extended`.
    pub extended_max_y: i32,
}

impl WorldHeight {
    fn of(args: &Args, extended: bool) -> Self {
        Self {
            min_y: world_min_y(args, extended),
            min_ground_level: min_ground_level(args, extended),
            extended,
            extended_max_y: extended_max_y_for(args),
        }
    }
}

/// Terrain bases a world can stand on: two above its floor, up to its ceiling less the
/// terrain buffer, or the scaler's clamp would have its min above its max.
pub(crate) fn ground_level_range(args: &Args, extended: bool) -> std::ops::RangeInclusive<i32> {
    world_min_y(args, extended) + 2
        ..=world_top_y(args, extended) - crate::elevation::postprocess::TERRAIN_HEIGHT_BUFFER
}

/// The build height the fit starts from, and the extended one it may switch to.
fn height_choice(args: &Args) -> (WorldHeight, Option<WorldHeight>) {
    let current = WorldHeight::of(args, args.disable_height_limit);
    if !auto_height(args) {
        return (current, None);
    }
    let extended = WorldHeight::of(args, true);
    if !ground_level_range(args, false).contains(&args.ground_level) {
        eprintln!(
            "Extended build height: ground level {} is outside the vanilla range",
            args.ground_level
        );
        return (extended, None);
    }
    (current, Some(extended))
}

/// Gives the dimension the extended height, once an auto-height run turns out to need it.
fn extend_world_bounds(args: &Args) {
    crate::world_editor::set_world_bounds(world_min_y(args, true), world_top_y(args, true));
}

/// Without a relief to fit: whether every tall structure stays under vanilla's ceiling.
/// Takes the hints, so it is only for a run whose elevation fetch did not take them.
fn flat_fits_vanilla(args: &Args, bbox: &LLBBox, peaks: &crate::elevation::PeakSource) -> bool {
    let why = crate::elevation::vanilla_misses(
        &peaks.take(),
        || peaks.take_overture(),
        |structures| {
            structures
                .iter()
                .filter(|m| crate::elevation::peaks::inside(bbox, m.lat, m.lon))
                .map(|m| (0.0, m.height_m))
                .collect()
        },
        |tops| {
            crate::elevation::postprocess::vanilla_overflow(
                &[],
                tops,
                1.0,
                args.scale,
                args.ground_level,
            )
        },
    );
    match why {
        Some(why) => {
            eprintln!("Extended build height: {why}");
            false
        }
        None => true,
    }
}

/// After the terrain fit: a run that picked its build height from the area takes the one
/// its ground settled on, checked again against the disk, and a new Java world gets the
/// datapack that declares an extended height. A One World installs its pack when it is made.
pub fn settle_build_height(
    args: &mut Args,
    ground: &Ground,
    bbox: &LLBBox,
    format: crate::world_editor::WorldFormat,
    disk_dir: &std::path::Path,
    new_java_world: Option<&std::path::Path>,
) -> Result<(), String> {
    let auto = auto_height(args) && ground.extended_ceiling;
    // Whether the terrain itself outgrew vanilla's column, rather than a building on it.
    let vanilla = ground_level_range(args, false);
    let terrain_taller = ground.base_level() < *vanilla.start()
        || crate::world_editor::common::terrain_top_y() > *vanilla.end();
    // Only a height the area picked could be kept vanilla instead: the forcing flag or a base
    // outside vanilla's range chose theirs.
    let compressible = auto && vanilla.contains(&args.ground_level);
    let instead = match (compressible, terrain_taller) {
        (false, _) => "",
        (true, true) => " Use --vanilla-height to compress the terrain instead.",
        (true, false) => {
            " Use --vanilla-height to keep the vanilla height, which cuts taller buildings off at Y 319."
        }
    };
    if auto {
        args.disable_height_limit = true;
        // Caves and filled ground grow with the terrain; under a tower alone they reach only
        // 64 blocks deeper, which the disk headroom covers.
        if terrain_taller {
            crate::world_utils::check_disk_for_extended_height(args, bbox, format, disk_dir)
                .map_err(|e| {
                    let keep = if compressible {
                        "; keep the vanilla height to compress the terrain instead"
                    } else {
                        ""
                    };
                    format!(
                        "{e} The area needs the extended build height, which makes the world \
                         larger{keep}."
                    )
                })?;
        }
        if args.bedrock {
            println!(
                "Note: the world carries a behavior pack that extends the build height \
                 (Bedrock 1.21.120+).{instead}"
            );
        }
        crate::progress::emit_gui_progress_update(
            crate::progress::MESSAGE_ONLY,
            "Extended build height: this area needs more room than vanilla Minecraft has",
        );
    }
    let Some(dir) = new_java_world else {
        return Ok(());
    };
    if !args.disable_height_limit || args.one_world_run.is_some() {
        return Ok(());
    }
    crate::world_utils::install_tall_datapack(dir)
        .map_err(|e| format!("Failed to install tall-world datapack: {e}"))?;
    println!(
        "Note: added the tall-world datapack (Minecraft 1.21.4+). The first load asks about \
         experimental features, and the world can't be uploaded to Realms.{instead}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZPoint;
    use crate::elevation_data::ElevationData;

    fn ground_with(heights: Vec<Vec<f32>>) -> Ground {
        let h = heights.len();
        let w = heights[0].len();
        Ground {
            elevation_enabled: true,
            extended_ceiling: false,
            ground_level: 0,
            elevation_data: Some(ElevationData {
                heights,
                width: w,
                height: h,
                world_width: w,
                world_height: h,
                min_height_m: 0.0,
                blocks_per_meter: 1.0,
                slope_correction: 1.0,
                ground_level: 0,
                soft_top: None,
                extended_ceiling: false,
                halo: None,
            }),
            land_cover: None,
            canopy: None,
            world_width: w,
            world_height: h,
            rotation_mask: None,
            snow_threshold_y: i32::MAX,
            climate: crate::climate::Climate::Temperate,
            body: CelestialBody::Earth,
            ecoregions: None,
            winter: false,
            origin: (0, 0),
        }
    }

    // An unmeasured cell hands the decision back to the land cover.
    #[test]
    fn canopy_fraction_separates_bare_from_unmeasured() {
        let nd = canopy::CANOPY_NODATA;
        let mut ground = ground_with(vec![vec![0.0; 4]; 4]);
        // Left half wooded, right half measured bare, bottom row unmeasured.
        ground.canopy = Some(CanopyData::from_grid(
            vec![
                12, 12, 0, 0, //
                12, 12, 0, 0, //
                12, 12, 0, 0, //
                nd, nd, nd, nd,
            ],
            4,
            4,
        ));
        assert_eq!(ground.canopy_fraction(XZPoint::new(0, 0), 2), Some(1.0));
        assert_eq!(ground.canopy_fraction(XZPoint::new(2, 0), 2), Some(0.0));
        assert_eq!(
            ground.canopy_fraction(XZPoint::new(0, 3), 1),
            None,
            "unmeasured is not bare"
        );
        // A cell straddling the two averages only over what was measured.
        assert_eq!(ground.canopy_fraction(XZPoint::new(0, 2), 2), Some(1.0));
        // The early-out probe agrees with the full scan.
        for (x, z, span) in [(0, 0, 2), (2, 0, 2), (0, 3, 1), (0, 0, 0)] {
            let o = XZPoint::new(x, z);
            assert_eq!(
                ground.canopy_measured(o, span),
                ground.canopy_cell(o, span).is_some()
            );
        }
        // Without a canopy grid there is nothing to say.
        ground.canopy = None;
        assert_eq!(ground.canopy_fraction(XZPoint::new(0, 0), 2), None);
        assert!(!ground.canopy_measured(XZPoint::new(0, 0), 2));
    }

    // Flat mode (no elevation) still maps land-cover lookups via the stored world dims, with edge clamping.
    #[test]
    fn flat_land_cover_maps_and_clamps() {
        use crate::land_cover::{LandCoverData, LC_WATER};
        let lc = LandCoverData {
            grid: vec![vec![LC_WATER, 10], vec![10, 10]],
            water_distance: vec![vec![1, 0], vec![0, 0]],
            water_blend_cache: once_cell::sync::OnceCell::new(),
            width: 2,
            height: 2,
            cells_per_meter: 1.0,
        };
        // world 4x4 over a 2x2 grid: x<=1 samples column 0, x>=2 samples column 1.
        let ground = Ground::new_flat_land_cover_test(lc, 4, 4);
        assert_eq!(ground.cover_class(XZPoint::new(0, 0)), LC_WATER);
        assert_eq!(ground.cover_class(XZPoint::new(3, 0)), 10);
        assert_eq!(ground.water_distance(XZPoint::new(0, 0)), 1);
        // Out-of-range coords clamp to the last grid cell instead of panicking.
        assert_eq!(ground.cover_class(XZPoint::new(1000, 1000)), 10);
        assert_eq!(ground.water_distance(XZPoint::new(1000, 1000)), 0);
    }

    // So a building across a One World seam stands on the same ground in both areas.
    #[test]
    fn lookups_past_a_cropped_area_read_the_margin_it_was_fetched_with() {
        // x + 100 z over a 10 x 8 fetch, cropped by 2 on every side.
        let padded: Vec<Vec<f32>> = (0..8)
            .map(|z| (0..10).map(|x| (x + 100 * z) as f32).collect())
            .collect();
        let mut g = ground_with(padded);
        let data = g.elevation_data.as_mut().unwrap();
        data.crop(2, 2, 6, 4);
        // Inside, the cropped grid answers, as the later passes left it.
        data.heights[0][0] = -1.0;
        assert_eq!(g.level(XZPoint::new(0, 0)), -1);
        assert_eq!(g.level(XZPoint::new(3, 2)), 405);
        assert_eq!(g.level(XZPoint::new(-1, 0)), 201);
        assert_eq!(g.level(XZPoint::new(6, 1)), 308);
        assert_eq!(g.level(XZPoint::new(2, -2)), 4);
        assert_eq!(g.level_exact(XZPoint::new(7, 5)), 709.0);
        assert_eq!(g.level(XZPoint::new(-50, 1)), 300);

        // Without a margin the edge repeats.
        let edge = ground_with(ramp(1.0));
        assert_eq!(
            edge.level(XZPoint::new(-3, 4)),
            edge.level(XZPoint::new(0, 4))
        );
    }

    // Water snaps to the local floor over small DEM steps, but not across a real cliff.
    #[test]
    fn water_level_snaps_small_steps_not_cliffs() {
        // Flat terrain: no snap, returns the cell's own level.
        let flat = ground_with(vec![vec![5.0; 16]; 16]);
        assert_eq!(flat.water_level(XZPoint::new(8, 8)), 5);

        // 3-block step: snaps down to the nearby floor.
        let step = ground_with(
            (0..16)
                .map(|_| (0..16).map(|x| if x <= 7 { 10.0 } else { 7.0 }).collect())
                .collect(),
        );
        assert_eq!(step.water_level(XZPoint::new(7, 8)), 7);

        // Real cliff (30-block drop): keeps its own level, no terracing.
        let cliff = ground_with(
            (0..16)
                .map(|_| (0..16).map(|x| if x <= 7 { 30.0 } else { 0.0 }).collect())
                .collect(),
        );
        assert_eq!(cliff.water_level(XZPoint::new(7, 8)), 30);
    }

    #[test]
    fn snow_line_follows_latitude() {
        assert!((snow_line_meters(0.0) - 4500.0).abs() < 1.0);
        assert!((snow_line_meters(25.0) - 5700.0).abs() < 1.0);
        assert!((snow_line_meters(46.0) - 3000.0).abs() < 1.0);
        assert!(snow_line_meters(90.0).abs() < 1.0);
        // Symmetric across the equator.
        assert_eq!(snow_line_meters(-46.0), snow_line_meters(46.0));
    }

    #[test]
    fn snow_threshold_inverts_the_scale() {
        let ed = |min_m: f64, bpm: f64| ElevationData {
            heights: vec![vec![0.0; 2]; 2],
            width: 2,
            height: 2,
            world_width: 2,
            world_height: 2,
            min_height_m: min_m,
            blocks_per_meter: bpm,
            slope_correction: 1.0,
            ground_level: 0,
            soft_top: None,
            extended_ceiling: false,
            halo: None,
        };
        // 46 deg snow line is 3000 m; at 0.1 block/m from min 0 m, ground 64 => Y 364.
        assert_eq!(snow_threshold_for(&ed(0.0, 0.1), 46.0, 64), 364);
        // Flat terrain: never below the line, always above it.
        assert_eq!(snow_threshold_for(&ed(100.0, 0.0), 46.0, 64), i32::MAX);
        assert_eq!(snow_threshold_for(&ed(4000.0, 0.0), 46.0, 64), i32::MIN);
    }

    fn scaled_ground(
        heights: Vec<Vec<f32>>,
        blocks_per_meter: f64,
        slope_correction: f64,
    ) -> Ground {
        let mut g = ground_with(heights);
        let d = g.elevation_data.as_mut().unwrap();
        d.blocks_per_meter = blocks_per_meter;
        d.slope_correction = slope_correction;
        g
    }

    /// heights[z][x] = x * per_block, so `level` is exact at integer coordinates.
    fn ramp(per_block: f64) -> Vec<Vec<f32>> {
        (0..16)
            .map(|_| (0..16).map(|x| (x as f64 * per_block) as f32).collect())
            .collect()
    }

    #[test]
    fn slope_reports_the_same_incline_whatever_the_vertical_compression() {
        // 8 blocks of rise across the 8-block sampling span is 45 degrees, which the
        // downstream thresholds spell 8 * tan(incline) = 8.
        let uncompressed = scaled_ground(ramp(1.0), 1.0, 1.0);
        assert_eq!(uncompressed.slope(XZPoint::new(8, 8)), 8);

        // Same hillside with the relief squeezed 4:1 into the vanilla ceiling.
        let compressed = scaled_ground(ramp(0.25), 0.25, 4.0);
        assert_eq!(compressed.slope(XZPoint::new(8, 8)), 8);
    }

    #[test]
    fn slope_undoes_the_soft_top() {
        // A 45 degree face at 3500 m, where the soft top squeezes it to about a quarter.
        let affine = ElevationAffine::whole_earth(1.0, -2014, 2031);
        let heights: Vec<Vec<f32>> = (0..17)
            .map(|_| {
                (0..17)
                    .map(|x| affine.y_for_metres(3500.0 + x as f64) as f32)
                    .collect()
            })
            .collect();
        let mut g = scaled_ground(heights, 1.0, 1.0);
        let d = g.elevation_data.as_mut().unwrap();
        d.min_height_m = affine.min_height_m;
        d.ground_level = affine.ground_level;
        d.soft_top = affine.soft_top;
        // Steep, as a real 45 degree face is, instead of the 2 the blocks alone show.
        let slope = g.slope(XZPoint::new(8, 8));
        assert!(slope > 4, "{slope}");
    }

    #[test]
    fn blocks_per_metre_falls_back_to_one_without_a_vertical_affine() {
        let mut g = scaled_ground(vec![vec![0.0; 4]; 4], 0.25, 4.0);
        assert_eq!(g.blocks_per_meter(), 0.25);

        g.elevation_data.as_mut().unwrap().blocks_per_meter = 0.0;
        assert_eq!(
            g.blocks_per_meter(),
            1.0,
            "zero relief leaves no affine to invert"
        );

        g.elevation_data.as_mut().unwrap().blocks_per_meter = 0.25;
        g.elevation_enabled = false;
        assert_eq!(g.blocks_per_meter(), 1.0);
    }

    #[test]
    fn the_water_snap_cliff_cutoff_only_scales_under_a_raised_ceiling() {
        let step = |drop: f64| -> Vec<Vec<f32>> {
            (0..16)
                .map(|_| {
                    (0..16)
                        .map(|x| if x <= 7 { drop as f32 } else { 0.0 })
                        .collect()
                })
                .collect()
        };
        let at = |drop: f64, bpm: f64, correction: f64, extended: bool| {
            let mut g = scaled_ground(step(drop), bpm, correction);
            g.extended_ceiling = extended;
            g.water_level(XZPoint::new(7, 8))
        };

        // Vanilla ceiling: the cutoff is the 3-block snap radius at any scaling, so a quay
        // wall a few blocks above the basin still keeps its own level.
        assert_eq!(at(3.0, 1.0, 1.0, false), 0);
        assert_eq!(at(4.0, 1.0, 1.0, false), 4);
        assert_eq!(at(4.0, 0.12, 8.33, false), 4);

        // Raised ceiling, compressed: 25 m is under the snap radius, so the cutoff stays 3.
        assert_eq!(at(3.0, 0.12, 8.33, true), 0);
        assert_eq!(at(4.0, 0.12, 8.33, true), 4);

        // Raised ceiling, 1:1 vertical: the cutoff is the 25 m cliff the guard was written for.
        assert_eq!(at(10.0, 1.0, 1.0, true), 0);
        assert_eq!(at(26.0, 1.0, 1.0, true), 26);
    }

    #[test]
    fn the_extended_ceiling_agrees_with_the_declared_world_top() {
        use clap::Parser;
        let args = |extra: &[&str]| {
            let mut cmd: Vec<&str> = vec![
                "arnis",
                "--output-dir",
                ".",
                "--bbox",
                "1,2,3,4",
                "--disable-height-limit",
            ];
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };

        let java = args(&[]);
        assert_eq!(extended_max_y_for(&java), 2031);
        assert_eq!(world_top_y_for(&java), 2031);

        let bedrock = args(&["--bedrock"]);
        assert_eq!(extended_max_y_for(&bedrock), 512);
        // Ends a section, and still covers the 497 the scaler clamps to.
        assert_eq!(world_top_y_for(&bedrock), 511);

        let luanti = args(&["--luanti"]);
        assert_eq!(
            extended_max_y_for(&luanti),
            crate::world_editor::DEFAULT_MAX_Y
        );
        assert_eq!(world_top_y_for(&luanti), crate::world_editor::DEFAULT_MAX_Y);
    }

    fn parse(extra: &[&str]) -> Args {
        use clap::Parser;
        let mut cmd = vec!["arnis", "--output-dir", ".", "--bbox", "1,2,3,4"];
        cmd.extend_from_slice(extra);
        Args::parse_from(cmd.iter())
    }

    #[test]
    fn the_terrain_fit_picks_the_height_unless_it_is_fixed() {
        let (height, taller) = height_choice(&parse(&[]));
        assert!(!height.extended);
        assert_eq!(height.min_y, crate::world_editor::DEFAULT_MIN_Y);
        assert_eq!(
            taller,
            Some(WorldHeight {
                min_y: -2032,
                min_ground_level: -2030,
                extended: true,
                extended_max_y: 2031,
            })
        );
        // Bedrock's pack raises only the ceiling.
        assert_eq!(
            height_choice(&parse(&["--bedrock"])).1,
            Some(WorldHeight {
                min_y: crate::world_editor::DEFAULT_MIN_Y,
                min_ground_level: -62,
                extended: true,
                extended_max_y: 512,
            })
        );

        for fixed in [
            &["--vanilla-height"][..],
            &["--disable-height-limit"][..],
            &["--luanti"][..],
            &["--body", "moon"][..],
            &["--one-world"][..],
        ] {
            assert!(!auto_height(&parse(fixed)), "{fixed:?}");
            assert_eq!(height_choice(&parse(fixed)).1, None, "{fixed:?}");
        }
        assert!(
            height_choice(&parse(&["--disable-height-limit"]))
                .0
                .extended
        );
        assert!(!height_choice(&parse(&["--vanilla-height"])).0.extended);

        // A base vanilla cannot hold settles it before the fetch.
        for level in ["305", "-63"] {
            let (height, taller) = height_choice(&parse(&["--ground-level", level]));
            assert!(height.extended && taller.is_none(), "{level}");
        }
    }

    #[test]
    fn settling_takes_the_fit_and_installs_the_pack_only_when_needed() {
        let tmp = tempfile::tempdir().unwrap();
        let bbox = LLBBox::new(48.0, 11.0, 48.001, 11.001).unwrap();
        let java = crate::world_editor::WorldFormat::JavaAnvil;
        let pack = crate::world_utils::has_tall_datapack;
        let settle = |extra: &[&str], tall: bool| {
            let world =
                std::path::PathBuf::from(crate::world_utils::create_new_world(tmp.path()).unwrap());
            let mut args = parse(extra);
            let mut ground = Ground::new_flat(-62);
            ground.extended_ceiling = tall;
            settle_build_height(&mut args, &ground, &bbox, java, tmp.path(), Some(&world)).unwrap();
            (args.disable_height_limit, pack(&world))
        };

        assert_eq!(settle(&[], false), (false, false));
        assert_eq!(settle(&[], true), (true, true));
        assert_eq!(settle(&["--vanilla-height"], true), (false, false));
        assert_eq!(settle(&["--disable-height-limit"], false), (true, true));

        // Bedrock carries its pack in the .mcworld, so only the flag changes.
        let mut args = parse(&["--bedrock"]);
        let mut ground = Ground::new_flat(-62);
        ground.extended_ceiling = true;
        let bedrock = crate::world_editor::WorldFormat::BedrockMcWorld;
        settle_build_height(&mut args, &ground, &bbox, bedrock, tmp.path(), None).unwrap();
        assert!(args.disable_height_limit);
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// Ground carrying an elevation grid *and* land cover; the water-field tests need
    /// both and neither single-purpose test constructor supplies the pair.
    pub(crate) fn ground_with_land_cover_and_elevation(
        land_cover: LandCoverData,
        world_width: usize,
        world_height: usize,
    ) -> Ground {
        let (gw, gh) = (land_cover.width, land_cover.height);
        ground_with_land_cover_and_heights(
            land_cover,
            vec![vec![0.0f32; gw]; gh],
            world_width,
            world_height,
        )
    }

    /// `ground_with_land_cover_and_elevation` over the given terrain, in blocks.
    pub(crate) fn ground_with_land_cover_and_heights(
        land_cover: LandCoverData,
        heights: Vec<Vec<f32>>,
        world_width: usize,
        world_height: usize,
    ) -> Ground {
        let (gw, gh) = (land_cover.width, land_cover.height);
        Ground {
            elevation_enabled: true,
            extended_ceiling: false,
            ground_level: 0,
            elevation_data: Some(ElevationData {
                heights,
                width: gw,
                height: gh,
                world_width,
                world_height,
                min_height_m: 0.0,
                blocks_per_meter: 1.0,
                slope_correction: 1.0,
                ground_level: 0,
                soft_top: None,
                extended_ceiling: false,
                halo: None,
            }),
            land_cover: Some(land_cover),
            canopy: None,
            world_width,
            world_height,
            rotation_mask: None,
            snow_threshold_y: i32::MAX,
            climate: crate::climate::Climate::Temperate,
            body: CelestialBody::Earth,
            ecoregions: None,
            winter: false,
            origin: (0, 0),
        }
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use crate::projection::{llbbox_for_rect, snap_bbox_to_chunks, WebMercatorProjection};

    fn projected_frame(pad: usize, w: usize, h: usize) -> GroundFrame {
        GroundFrame {
            world_dims: Some((w, h)),
            mercator: Some(WebMercatorProjection::new(48.1372, 11.5755, 1.0)),
            pad_blocks: pad,
            affine: AffinePolicy::Fit,
        }
    }

    #[test]
    fn the_local_frame_reads_everything_off_the_bbox() {
        let bbox = LLBBox::new(48.13, 11.56, 48.14, 11.58).unwrap();
        let plan = GroundFrame::local().fetch_plan(&bbox, 1.0);
        let (ww, wh, gw, gh) = compute_grid_dims(&bbox, 1.0);
        assert_eq!(plan.dims, (ww, wh, gw, gh));
        assert_eq!(plan.pad, 0);
        assert_eq!(plan.final_dims, (ww, wh));
        assert_eq!(plan.bbox, bbox);
    }

    #[test]
    fn a_padded_plan_fetches_the_rect_plus_the_margin_and_crops_back() {
        let proj = WebMercatorProjection::new(48.1372, 11.5755, 1.0);
        let req = LLBBox::new(48.13, 11.56, 48.14, 11.58).unwrap();
        let (rect, eff) = snap_bbox_to_chunks(&proj, &req).unwrap();
        let w = (rect.max_x() - rect.min_x() + 1) as usize;
        let h = (rect.max_z() - rect.min_z() + 1) as usize;
        let plan = projected_frame(100, w, h).fetch_plan(&eff, 1.0);
        assert_eq!(plan.pad, 100);
        assert_eq!(plan.dims, (w + 200, h + 200, w + 200, h + 200));
        assert_eq!(plan.final_dims, (w, h));
        let grown = XZBBox::rect_from_min_max(
            rect.min_x() - 100,
            rect.min_z() - 100,
            rect.max_x() + 100,
            rect.max_z() + 100,
        )
        .unwrap();
        let outer = llbbox_for_rect(&proj, &grown).unwrap();
        assert!(plan.bbox.min().lat() > outer.min().lat());
        assert!(plan.bbox.max().lng() < outer.max().lng());
        // First and last samples sit on the outermost block centres.
        let x0 = proj.x_for_lon(plan.bbox.min().lng());
        let x1 = proj.x_for_lon(plan.bbox.max().lng());
        assert!((x0 - (grown.min_x() as f64 + 0.5)).abs() < 1e-6);
        assert!((x1 - (grown.max_x() as f64 + 0.5)).abs() < 1e-6);
        let z0 = proj.z_for_lat(plan.bbox.max().lat());
        assert!((z0 - (grown.min_z() as f64 + 0.5)).abs() < 1e-6);
    }

    #[test]
    fn padding_is_dropped_when_the_grid_would_be_capped() {
        let side = crate::elevation::MAX_ELEVATION_GRID_DIM;
        let proj = WebMercatorProjection::new(0.0, 0.0, 1.0);
        let rect = XZBBox::rect_from_min_max(0, 0, side as i32 - 1, 15).unwrap();
        let eff = llbbox_for_rect(&proj, &rect).unwrap();
        let plan = projected_frame(100, side, 16).fetch_plan(&eff, 1.0);
        assert_eq!(plan.pad, 0);
        assert_eq!(plan.final_dims, (side, 16));
    }

    #[test]
    fn the_frame_follows_the_args() {
        use clap::Parser;
        let local =
            crate::args::Args::parse_from(["arnis", "--output-dir", ".", "--bbox", "1,2,3,4"]);
        let bbox = LLBBox::new(1.0, 2.0, 3.0, 4.0).unwrap();
        assert!(GroundFrame::from_args(&local, &bbox).world_dims.is_none());

        let mercator = crate::args::Args::parse_from([
            "arnis",
            "--output-dir",
            ".",
            "--bbox",
            "48.13,11.56,48.14,11.58",
            "--projection",
            "web_mercator",
        ]);
        let bbox = LLBBox::new(48.13, 11.56, 48.14, 11.58).unwrap();
        let frame = GroundFrame::from_args(&mercator, &bbox);
        assert!(frame.mercator.is_some());
        assert_eq!(frame.pad_blocks, 0);
        assert_eq!(frame.affine, AffinePolicy::Fit);
        let (_, rect) = crate::projection::ProjectionSpec::from_args(&mercator)
            .transformer(&bbox)
            .unwrap();
        assert_eq!(
            frame.world_dims,
            Some((
                (rect.max_x() - rect.min_x() + 1) as usize,
                (rect.max_z() - rect.min_z() + 1) as usize
            ))
        );
    }
}
