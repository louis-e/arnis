//! Ground layer generation — surface blocks, vegetation, shorelines, and underground fill.
//!
//! This module handles the final terrain pass that runs after all OSM element
//! processing is complete. It iterates over every block in the bounding box and:
//!
//! - Selects surface and sub-surface blocks based on ESA WorldCover land cover
//!   classification and terrain slope.
//! - Blends shorelines between water and land.
//! - Places vegetation (grass, flowers, trees, crops) according to land cover class.
//! - Cleans up stray vegetation from road surfaces.
//! - Fills underground columns with stone and places a bedrock floor.
//!
//! The generation is done chunk-by-chunk for better cache locality when writing
//! to region files.

use crate::args::Args;
use crate::block_definitions::{
    AIR, BEDROCK, BLACK_CONCRETE, BRICK, CARROTS, CLAY, COARSE_DIRT, COBBLESTONE,
    CRACKED_STONE_BRICKS, CYAN_TERRACOTTA, DEAD_BUSH, DIRT, DIRT_PATH, FARMLAND, FERN, GRASS,
    GRASS_BLOCK, GRAVEL, GRAY_CONCRETE, GRAY_CONCRETE_POWDER, HAY_BALE, LIGHT_GRAY_CONCRETE,
    MOSS_BLOCK, MUD, OAK_PLANKS, PACKED_ICE, PODZOL, POTATOES, SAND, SANDSTONE, SMOOTH_STONE,
    SNOW_BLOCK, STONE, STONE_BRICKS, TALL_GRASS_BOTTOM, TALL_GRASS_TOP, WATER, WHEAT,
    WHITE_CONCRETE, YELLOW_CONCRETE,
};
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::element_processing::tree;
use crate::floodfill_cache::BuildingFootprintBitmap;
use crate::ground::Ground;
use crate::ground_decoration::{LOOSE_PLANTS, STACKED_PLANT_PARTS, WOOD};
use crate::land_cover;
use crate::progress::emit_gui_progress_update;
use crate::terrain_surface;
use crate::world_editor::WorldEditor;
use crate::world_editor::{min_y, terrain_floor_y};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use rand::Rng;

// Salts that keep the undergrowth fields independent of each other.
const SALT_FOREST_FLOOR: u32 = 0xF0E5_7F10;
const SALT_SHRUB_FLOOR: u32 = 0x5B7B_F10A;
const SALT_SWARD: u32 = 0x5A7D_0001;
const SALT_TALL_SWARD: u32 = 0x5A7D_0002;
const SALT_WETLAND_POOLS: u32 = 0x9001_5E75;

/// Per-chunk cache of ground Y values.
///
/// Each Minecraft-chunk worth of surface/vegetation/depth logic fires
/// roughly 20-plus `get_ground_level` lookups per cell (own column + 8
/// water-column neighbours + 8 depth-fill neighbours + a handful of
/// slope/surface checks). At a typical city bbox that's ~10⁸ calls,
/// each touching the road-override map, an elevation-grid bilinear, and
/// a few f32→f64 casts. Precomputing one Y per cell up front — via a
/// flat 256-entry stack array aligned to the chunk's 16×16 footprint —
/// turns the 20-plus per-cell calls into stack array reads for
/// everything inside the chunk; neighbours that escape the chunk
/// boundary fall back to `editor.get_ground_level`. The cache is
/// populated once per chunk, read many times, then dropped.
struct ChunkGroundCache {
    /// Row-major `16*lz + lx` where `lx = x - base_x`, `lz = z - base_z`.
    /// Positions outside `[min_x..=max_x, min_z..=max_z]` are never read.
    grid: [i32; 256],
    base_x: i32,
    base_z: i32,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
}

impl ChunkGroundCache {
    #[inline]
    fn populate(
        editor: &WorldEditor,
        chunk_x: i32,
        chunk_z: i32,
        min_x: i32,
        max_x: i32,
        min_z: i32,
        max_z: i32,
    ) -> Self {
        let base_x = chunk_x << 4;
        let base_z = chunk_z << 4;
        let mut grid = [0i32; 256];
        for x in min_x..=max_x {
            for z in min_z..=max_z {
                let lx = (x - base_x) as usize;
                let lz = (z - base_z) as usize;
                grid[lz * 16 + lx] = editor.get_ground_level(x, z);
            }
        }
        ChunkGroundCache {
            grid,
            base_x,
            base_z,
            min_x,
            max_x,
            min_z,
            max_z,
        }
    }

    /// Get the ground Y at `(nx, nz)`. Cached for cells inside this chunk's
    /// populated range; falls through to `editor.get_ground_level` for
    /// neighbour reads that cross a chunk boundary.
    #[inline]
    fn get(&self, editor: &WorldEditor, nx: i32, nz: i32) -> i32 {
        if nx >= self.min_x && nx <= self.max_x && nz >= self.min_z && nz <= self.max_z {
            let lx = (nx - self.base_x) as usize;
            let lz = (nz - self.base_z) as usize;
            self.grid[lz * 16 + lx]
        } else {
            editor.get_ground_level(nx, nz)
        }
    }
}

/// Water answers for one chunk and the ring of columns around it.
///
/// Every column asks about itself and its eight neighbours, so each answer was worked
/// out up to nine times, three block lookups apiece. The ground pass only writes water
/// into the column it is on (trees keep water on their blacklist), so an answer holds
/// until that column is processed, and is forgotten then.
struct WaterColumnMemo {
    base_x: i32,
    base_z: i32,
    /// Row-major over the 18x18 window: 0 not asked yet, 1 dry, 2 water.
    state: [u8; 18 * 18],
}

impl WaterColumnMemo {
    fn new(chunk_x: i32, chunk_z: i32) -> Self {
        Self {
            base_x: (chunk_x << 4) - 1,
            base_z: (chunk_z << 4) - 1,
            state: [0; 18 * 18],
        }
    }

    #[inline]
    fn slot(&self, x: i32, z: i32) -> Option<usize> {
        let (dx, dz) = (x - self.base_x, z - self.base_z);
        ((0..18).contains(&dx) && (0..18).contains(&dz)).then_some((dz * 18 + dx) as usize)
    }

    #[inline]
    fn get(&mut self, x: i32, z: i32, probe: impl FnOnce() -> bool) -> bool {
        let Some(i) = self.slot(x, z) else {
            return probe();
        };
        match self.state[i] {
            1 => false,
            2 => true,
            _ => {
                let water = probe();
                self.state[i] = 1 + u8::from(water);
                water
            }
        }
    }

    /// Called once the column has been processed, which may have put water in it.
    #[inline]
    fn forget(&mut self, x: i32, z: i32) {
        if let Some(i) = self.slot(x, z) {
            self.state[i] = 0;
        }
    }
}

/// Whether the canopy map covers this column's lattice cell, and whether it
/// wants a trunk rooted here. Decided once per cell, at the cell's own trunk
/// slot, since every column in a cell snaps to that slot anyway. An unmeasured
/// cell reports `(false, false)` so the land cover keeps its say.
#[allow(clippy::too_many_arguments)]
fn canopy_verdict(
    ground: &Ground,
    x: i32,
    z: i32,
    origin_x: i32,
    origin_z: i32,
    spacing: i32,
    schematic_trees: bool,
) -> (bool, bool) {
    let cell = XZPoint::new(
        x.div_euclid(spacing) * spacing - origin_x,
        z.div_euclid(spacing) * spacing - origin_z,
    );
    let Some(fraction) = ground.canopy_fraction(cell, spacing) else {
        return (false, false);
    };
    if (x, z) != crate::trees::schematic::trunk_slot_s(x, z, spacing) {
        return (true, false);
    }
    let p = crate::canopy::slot_probability(fraction, spacing, schematic_trees);
    // Its own salt, so no existing draw shifts when the option is off.
    let roll = (land_cover::coord_hash(x ^ 0x434D, z ^ 0x484D) % 10_000) as f64 / 10_000.0;
    (true, roll < p)
}

/// Generate the ground layer for the entire bounding box.
///
/// This must be called after all OSM element processing is complete and the
/// flood-fill / highway caches have been dropped. Regions remain in memory
/// and are saved in parallel by `save_java()` after generation completes.
pub fn generate_ground_layer(
    editor: &mut WorldEditor,
    ground: &Ground,
    args: &Args,
    xzbbox: &XZBBox,
    building_footprints: &BuildingFootprintBitmap,
    tunnel_footprint: &BuildingFootprintBitmap,
    bridge_surface: &BridgeSurfaceMap,
) -> Result<(), String> {
    generate_ground_region(
        editor,
        ground,
        args,
        xzbbox,
        building_footprints,
        tunnel_footprint,
        bridge_surface,
        xzbbox.min_x(),
        xzbbox.max_x(),
        xzbbox.min_z(),
        xzbbox.max_z(),
        true,
    );
    Ok(())
}

/// Generate ground for `[iter_min_*..=iter_max_*]`. The shared grid is indexed against
/// `xzbbox` (main origin); per-tile callers pass the main bbox + strict tile iter bounds.
#[allow(clippy::too_many_arguments)]
pub fn generate_ground_region(
    editor: &mut WorldEditor,
    ground: &Ground,
    args: &Args,
    xzbbox: &XZBBox,
    building_footprints: &BuildingFootprintBitmap,
    tunnel_footprint: &BuildingFootprintBitmap,
    bridge_surface: &BridgeSurfaceMap,
    iter_min_x: i32,
    iter_max_x: i32,
    iter_min_z: i32,
    iter_max_z: i32,
    show_progress: bool,
) {
    // `Some` only off Earth, where the body's palette replaces land cover and
    // every Earth-only surface pass is off.
    let planetary_body = (!args.body.is_earth()).then_some(args.body);
    let center_lat = args
        .bbox
        .as_ref()
        .map(|b| (b.min().lat() + b.max().lat()) * 0.5)
        .unwrap_or(0.0);
    let has_land_cover = ground.has_land_cover();
    let has_canopy = ground.has_canopy();
    let tree_spacing = editor.tree_slot_spacing();
    let schematic_trees = editor.tree_pack().is_some();
    let terrain_enabled = ground.elevation_enabled;
    let climate = ground.climate();

    let total_blocks: u64 =
        (iter_max_x - iter_min_x + 1).max(0) as u64 * (iter_max_z - iter_min_z + 1).max(0) as u64;
    let desired_updates: u64 = 1500;
    let batch_size: u64 = (total_blocks / desired_updates).max(1);

    let mut block_counter: u64 = 0;

    if show_progress {
        println!("{} Generating ground...", "[6/7]".bold());
        emit_gui_progress_update(70.0, "Generating ground...");
    }

    let ground_pb: ProgressBar = if show_progress {
        ProgressBar::new(total_blocks)
    } else {
        ProgressBar::hidden()
    };
    ground_pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:45}] {pos}/{len} blocks ({eta})")
            .unwrap()
            .progress_chars("█▓░"),
    );

    let mut gui_progress_grnd: f64 = 70.0;
    let mut last_emitted_progress: f64 = gui_progress_grnd;
    let total_iterations_grnd: f64 = total_blocks as f64;
    let progress_increment_grnd: f64 = 20.0 / total_iterations_grnd;

    // Process ground generation chunk-by-chunk for better cache locality.
    // This keeps the same region/chunk HashMap entries hot in CPU cache,
    // rather than jumping between regions on every Z iteration.
    let min_chunk_x = iter_min_x >> 4;
    let max_chunk_x = iter_max_x >> 4;
    let min_chunk_z = iter_min_z >> 4;
    let max_chunk_z = iter_max_z >> 4;

    // Snow line and the band over which snow thickens into full cover.
    let snow_line = terrain_surface::SnowLine::new(ground, center_lat, args.rotation);
    // Share of forest-floor grass that grows as ferns, by the habitat the forest is in.
    // Undergrowth thins out with dryness: sparse in deserts, thinner on steppe,
    // full in savanna, temperate and boreal country.
    let climate_sward = match climate {
        crate::climate::Climate::HotDesert => 0.25,
        crate::climate::Climate::ColdDesert => 0.3,
        crate::climate::Climate::IceCap => 0.3,
        crate::climate::Climate::HotSteppe => 0.55,
        crate::climate::Climate::ColdSteppe => 0.7,
        crate::climate::Climate::Tundra => 0.7,
        crate::climate::Climate::DryContinental => 0.85,
        crate::climate::Climate::Boreal => 0.9,
        crate::climate::Climate::Temperate | crate::climate::Climate::TropicalSavanna => 1.0,
    };
    // Read per chunk, as the ecoregion under the forest can change across the area.
    let fern_share_at = |x: i32, z: i32| match crate::ground_decoration::habitat(
        land_cover::LC_TREE_COVER,
        climate,
        center_lat.abs(),
        false,
        ground.ecoregion(XZPoint::new(x - xzbbox.min_x(), z - xzbbox.min_z())),
    ) {
        Some(crate::ground_decoration::Habitat::Taiga) => 0.45,
        Some(crate::ground_decoration::Habitat::Jungle) => 0.3,
        _ => 0.12,
    };

    for chunk_x in min_chunk_x..=max_chunk_x {
        for chunk_z in min_chunk_z..=max_chunk_z {
            // Calculate the block range for this chunk, clamped to bbox
            let chunk_min_x = (chunk_x << 4).max(iter_min_x);
            let chunk_max_x = ((chunk_x << 4) + 15).min(iter_max_x);
            let chunk_min_z = (chunk_z << 4).max(iter_min_z);
            let chunk_max_z = ((chunk_z << 4) + 15).min(iter_max_z);
            let forest_fern_share = fern_share_at((chunk_x << 4) + 8, (chunk_z << 4) + 8);
            // Fallen rock is only looked for where the relief could hold a cliff.
            let talus_field = (terrain_enabled && planetary_body.is_none())
                .then(|| {
                    terrain_surface::TalusField::new(
                        ground,
                        chunk_x,
                        chunk_z,
                        (xzbbox.min_x(), xzbbox.min_z()),
                    )
                })
                .flatten();

            // Precompute a per-chunk ground-Y cache so subsequent lookups
            // (main column + water-column + depth-fill neighbours, ~20+ per
            // cell) hit a stack array instead of re-running the bilinear
            // elevation interpolation. Only populated when terrain is on —
            // the flat-ground path never calls `editor.get_ground_level`.
            let chunk_ground_cache = terrain_enabled.then(|| {
                ChunkGroundCache::populate(
                    editor,
                    chunk_x,
                    chunk_z,
                    chunk_min_x,
                    chunk_max_x,
                    chunk_min_z,
                    chunk_max_z,
                )
            });

            let mut water_memo = WaterColumnMemo::new(chunk_x, chunk_z);

            // --fillground fast path: bulk-fill fully-buried sections to
            // Uniform(STONE) so the per-column loop only walks the boundary
            // section. Gated on full bbox coverage so out-of-bbox columns
            // don't get stone underneath.
            let mut column_fill_y_min = terrain_floor_y() + 1;
            if args.fillground {
                let chunk_fully_in_bbox = chunk_min_x == chunk_x << 4
                    && chunk_max_x == (chunk_x << 4) + 15
                    && chunk_min_z == chunk_z << 4
                    && chunk_max_z == (chunk_z << 4) + 15;
                let rotated_in = chunk_fully_in_bbox
                    && ground.is_in_rotated_bounds(chunk_min_x, chunk_min_z)
                    && ground.is_in_rotated_bounds(chunk_max_x, chunk_min_z)
                    && ground.is_in_rotated_bounds(chunk_min_x, chunk_max_z)
                    && ground.is_in_rotated_bounds(chunk_max_x, chunk_max_z);
                if rotated_in {
                    let min_ground_y: i32 = if let Some(ref cache) = chunk_ground_cache {
                        cache
                            .grid
                            .iter()
                            .copied()
                            .min()
                            .unwrap_or(args.ground_level)
                    } else {
                        args.ground_level
                    };
                    // Compare in i32: `as i8` on a section index below -128 wraps positive,
                    // which would pass the ordering check and stone-fill the whole column.
                    // Bounded below by the terrain floor, so an extended world floor does not
                    // stone-fill ~127 sections per chunk down to Y=-2032.
                    //
                    // This fills WHOLE sections, so it relies on the terrain floor sitting on a
                    // section boundary (set_terrain_floor_y snaps it). Otherwise the part of the
                    // bottom section below the floor would be stone under the bedrock plane.
                    debug_assert_eq!(terrain_floor_y().rem_euclid(16), 0);
                    let bottom_section = terrain_floor_y().div_euclid(16);
                    // section_top = section_y*16 + 15 <= min_ground_y - 3
                    let top_section = (min_ground_y - 18).div_euclid(16);
                    let in_i8 = (i8::MIN as i32..=i8::MAX as i32).contains(&bottom_section)
                        && (i8::MIN as i32..=i8::MAX as i32).contains(&top_section);
                    if in_i8 && top_section >= bottom_section {
                        let all_clean = editor.bulk_fill_chunk_sections_below(
                            chunk_x,
                            chunk_z,
                            bottom_section as i8,
                            top_section as i8,
                            STONE,
                        );
                        if all_clean {
                            column_fill_y_min = (top_section + 1) * 16;
                        }
                    }
                }
            }

            for x in chunk_min_x..=chunk_max_x {
                for z in chunk_min_z..=chunk_max_z {
                    // Skip blocks outside the rotated original bounding box
                    if !ground.is_in_rotated_bounds(x, z) {
                        block_counter += 1;
                        if block_counter.is_multiple_of(batch_size) {
                            ground_pb.set_position(block_counter);
                        }
                        continue;
                    }

                    // Get ground level. When terrain is enabled, pull from the
                    // per-chunk cache (one populated lookup, no bilinear); when
                    // disabled, use the constant ground_level.
                    let ground_y = if let Some(ref cache) = chunk_ground_cache {
                        cache.get(editor, x, z)
                    } else {
                        args.ground_level
                    };

                    let coord = XZPoint::new(x - xzbbox.min_x(), z - xzbbox.min_z());

                    // Slope once per column (used for surface selection and depth), from
                    // unrounded heights so a contour doesn't flicker between tiers.
                    let (slope_f, gradient) = if terrain_enabled {
                        ground.slope_and_gradient(coord)
                    } else {
                        (0.0, (0.0, 0.0))
                    };
                    let slope = slope_f.round() as i32;

                    // On steep terrain, override any existing OSM surface block
                    // (e.g., a quarry's stone, a park's grass) with slope-appropriate
                    // rock material. Steep cliffs should always look like rock.
                    //
                    // Threshold must match the first "rock" tier below (`slope > 4`).
                    // At `slope == 4`, the material cascade falls through to land-
                    // cover selection (grass / farmland / etc.), so force-replacing
                    // at that slope would wipe e.g. a `landuse=quarry` STONE surface
                    // with GRASS_BLOCK for no good reason — it's only a 27° hiking
                    // slope, not a cliff.
                    //
                    // Mapped sand holds up to its angle of repose (~34°, below the
                    // `slope > 6` tier), so a dune's slip face stays sand, not a stripe of
                    // scree down the dune field.
                    let steep_override = terrain_enabled
                        && slope > 4
                        && (slope > 6
                            || !editor.check_for_block_absolute(
                                x,
                                ground_y,
                                z,
                                Some(&[SAND]),
                                None,
                            ));
                    let mut did_underfill = false;

                    // Determine surface and under-block material for this column.
                    // steep_override means we always compute & place the right blocks,
                    // even if OSM already placed something here.
                    let has_existing_stone =
                        editor.check_for_block_absolute(x, ground_y, z, Some(&[STONE]), None);

                    if steep_override || !has_existing_stone {
                        // Handle ESA water with variable depth as a special case.
                        // Use bilinear interpolation of the water grid to produce
                        // organic shorelines instead of rectangular grid-cell edges.
                        //
                        // water_distance > 0 acts as a floor: cells the grid already
                        // classifies as water are ALWAYS treated as water.  The blend
                        // can only EXTEND water into land (organic fringe), never
                        // retract it — so OSM rivers that overlap ESA water pixels
                        // are never overwritten with grass.
                        let water_blend = if has_land_cover {
                            ground.water_blend(coord)
                        } else {
                            0.0
                        };
                        let grid_is_water = has_land_cover && ground.water_distance(coord) > 0;
                        // Probe a column for water at its *own* ground level.
                        // Previously this closed over the outer-cell ground_y,
                        // so probing a neighbour column whose terrain sits at
                        // a different elevation (common on any sloped terrain)
                        // scanned the wrong Y range and silently missed water
                        // that OSM had placed at the neighbour's own ground
                        // level. Per-probe get_ground_level is a cheap
                        // bilinear lookup and fixes the false negatives in
                        // the osm_gap detection below.
                        let has_water_in_column = |wx: i32, wz: i32| {
                            // Pull from the chunk cache so the 9-neighbour
                            // fan-out around each cell doesn't trigger nine
                            // bilinear interpolations per cell. In flat-ground
                            // mode every column has the same constant Y, so
                            // we skip the `editor.get_ground_level` fallback
                            // (road overrides in flat mode always resolve to
                            // the same `args.ground_level` anyway).
                            let gy = match chunk_ground_cache {
                                Some(ref cache) => cache.get(editor, wx, wz),
                                None => args.ground_level,
                            };
                            for dy in 0..=2 {
                                if editor.check_for_block_absolute(
                                    wx,
                                    gy + dy,
                                    wz,
                                    Some(&[WATER]),
                                    None,
                                ) {
                                    return true;
                                }
                            }
                            false
                        };
                        let mut water_at = |wx: i32, wz: i32| {
                            water_memo.get(wx, wz, || has_water_in_column(wx, wz))
                        };
                        let placed_water = water_at(x, z);
                        let osm_gap = if placed_water {
                            false
                        } else {
                            let water_n = water_at(x, z - 1);
                            let water_s = water_at(x, z + 1);
                            let water_w = water_at(x - 1, z);
                            let water_e = water_at(x + 1, z);
                            let water_ne = water_at(x + 1, z - 1);
                            let water_nw = water_at(x - 1, z - 1);
                            let water_se = water_at(x + 1, z + 1);
                            let water_sw = water_at(x - 1, z + 1);

                            // Fill single-cell gaps when water spans opposite neighbors.
                            (water_n && water_s)
                                || (water_e && water_w)
                                || (water_ne && water_sw)
                                || (water_nw && water_se)
                        };
                        // Water classification: hard threshold on the
                        // Gaussian-smoothed water_blend_grid. Combined with
                        // the grid-level smoothing in `smooth_class_boundaries`
                        // this produces a clean curved shoreline contour —
                        // the 0.5 isoline of the smoothed water mask —
                        // instead of either the raw ESA 10 m rectangle grid
                        // or a stochastic noise-dithered transition.
                        let is_esa_water =
                            grid_is_water || placed_water || osm_gap || water_blend > 0.5;

                        let mut water_y = 0;
                        let mut place_esa_water = false;
                        if is_esa_water && !steep_override {
                            // Snap water to local minimum on steep terrain to compensate
                            // for ESA/DEM spatial misalignment in canyons
                            let wy = ground.water_level(coord);
                            // Skip columns that sit above the water surface to avoid
                            // buried water pockets inside slopes.
                            if ground_y <= wy {
                                water_y = wy;
                                place_esa_water = true;
                            } else if grid_is_water && ground.is_interior_water(coord) {
                                // A step inside the body, not a bank the snap pulled
                                // below: water here, not a line of grass across the river.
                                water_y = ground_y;
                                place_esa_water = true;
                            }
                        }

                        if place_esa_water {
                            // Pre-paint; carve_lc_water_pass later overwrites with depth.
                            editor.set_block_if_absent_absolute(WATER, x, water_y, z);
                            if water_y - 1 > min_y() {
                                editor.set_block_if_absent_absolute(SAND, x, water_y - 1, z);
                            }
                            if water_y - 2 > min_y() {
                                editor.set_block_if_absent_absolute(SANDSTONE, x, water_y - 2, z);
                            }
                        } else {
                            let cover_here = if has_land_cover {
                                ground.cover_class(coord)
                            } else {
                                0
                            };
                            // Snow by altitude and terrain shape. An ESA snow/ice cell near
                            // the snow line or in a cold climate is a glacier, which keeps
                            // patches of old snow even below the line.
                            let mut snow_depth = snow_line.depth(x, z, ground_y);
                            let glacier = planetary_body.is_none()
                                && terrain_surface::is_glacier_cover(cover_here)
                                && terrain_surface::is_plausible_ice(snow_depth, climate);
                            if glacier {
                                snow_depth = terrain_surface::glacier_depth(snow_depth);
                            }
                            let snow = if planetary_body.is_some()
                                || snow_depth < terrain_surface::SNOW_MIN_DEPTH
                            {
                                terrain_surface::Snow::None
                            } else {
                                terrain_surface::snow_cover(
                                    snow_depth,
                                    slope_f,
                                    || ground.convexity(coord),
                                    snow_line.shade(gradient, slope_f),
                                    x,
                                    z,
                                )
                            };

                            // Below a cliff, from gentle ground up to the talus's own angle.
                            let talus = match &talus_field {
                                Some(field)
                                    if slope <= 6 && terrain_surface::takes_talus(cover_here) =>
                                {
                                    field.near(x, z, ground.level_exact(coord))
                                }
                                _ => 0.0,
                            };
                            let talus_block =
                                terrain_surface::talus_palette(x, z, talus, cover_here);

                            // Determine surface and sub-surface blocks based on available data
                            let (surface_block, under_block) = if let Some(body) = planetary_body {
                                // No land cover off Earth, so this replaces the
                                // whole ESA cascade below.
                                crate::celestial::surface_palette(
                                    body, slope, center_lat, ground_y, x, z,
                                )
                            } else if has_land_cover {
                                // ESA WorldCover + slope-based material selection
                                let cover = cover_here;

                                // Steep terrain overrides land cover classification.
                                //
                                // slope is max-min of 4 cardinal neighbours sampled
                                // STEP=4 away, so `slope = 8 · tan(incline)`. Thresholds:
                                //
                                //   slope > 8  → ≥ 45° : sheer cliff face
                                //   slope > 6  → ≥ 37° : very steep rocky face
                                //   slope > 4  → ≥ 27° : steep slope, soil between outcrops
                                //   slope ≤ 4  → < 27° : falls through to land cover
                                //                        (alpine meadow, forest, etc.)
                                //
                                // We don't force rock materials onto 21–27° slopes
                                // any more — that's a normal hiking incline where
                                // grass and trees belong.
                                if let Some(p) = talus_block {
                                    p
                                } else if slope > 4 {
                                    terrain_surface::steep_palette(x, z, ground_y, slope, cover)
                                } else if glacier {
                                    terrain_surface::GLACIER_ICE
                                } else if let Some(p) = climate.surface_palette(cover, x, z) {
                                    p
                                } else {
                                    // Select surface block based on ESA land cover class
                                    match cover {
                                        land_cover::LC_TREE_COVER => (GRASS_BLOCK, DIRT),
                                        land_cover::LC_SHRUBLAND => {
                                            // Primarily grass with coarse-dirt patches.
                                            // Uses value noise (bilinear + smoothstep)
                                            // at ~5-block resolution so patch contours
                                            // are organic blobs, not axis-aligned
                                            // rectangles that an integer-division zone
                                            // hash would produce. A finer per-block
                                            // hash adds occasional grass peek-through
                                            // inside each blob so they don't look
                                            // stamped.
                                            let noise = value_noise_01(x, z, 5);
                                            let h = land_cover::coord_hash(x, z);
                                            // Threshold 0.4 yields roughly 20 % dirt
                                            // coverage (value noise from uniform
                                            // samples concentrates around 0.5, so 0.4
                                            // catches a band below that).
                                            if noise < 0.4 {
                                                if h.is_multiple_of(5) {
                                                    (GRASS_BLOCK, DIRT) // grass peek-through
                                                } else {
                                                    (COARSE_DIRT, DIRT) // dirt patch interior
                                                }
                                            } else {
                                                (GRASS_BLOCK, DIRT)
                                            }
                                        }
                                        land_cover::LC_GRASSLAND => (GRASS_BLOCK, DIRT),
                                        land_cover::LC_CROPLAND => (FARMLAND, DIRT),
                                        land_cover::LC_BUILT_UP => {
                                            let h = land_cover::coord_hash(x, z) % 100;
                                            if h < 72 {
                                                (STONE_BRICKS, STONE)
                                            } else if h < 87 {
                                                (CRACKED_STONE_BRICKS, STONE)
                                            } else if h < 92 {
                                                (STONE, STONE)
                                            } else {
                                                (COBBLESTONE, STONE)
                                            }
                                        }
                                        land_cover::LC_BARE | land_cover::LC_SNOW_ICE => {
                                            // Skip isolated bare pixels (surrounded by non-bare)
                                            // to avoid random single-block patches
                                            let neighbors_bare =
                                                [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)]
                                                    .iter()
                                                    .filter(|(dx, dz)| {
                                                        let cc = ground.cover_class(XZPoint::new(
                                                            x + dx - xzbbox.min_x(),
                                                            z + dz - xzbbox.min_z(),
                                                        ));
                                                        cc == land_cover::LC_BARE
                                                            || cc == land_cover::LC_SNOW_ICE
                                                            || cc == land_cover::LC_BEACH
                                                    })
                                                    .count();
                                            if neighbors_bare == 0 {
                                                // Isolated pixel - blend with surroundings
                                                (GRASS_BLOCK, DIRT)
                                            } else if value_noise_01(x, z, 6) < 0.45 {
                                                // Bare/sparse terrain: earth patches at
                                                // ~6-block resolution between rock, whose
                                                // own patches come from a separate field.
                                                (COARSE_DIRT, DIRT)
                                            } else {
                                                terrain_surface::bare_rock_palette(x, z)
                                            }
                                        }
                                        // Sand, or shingle where it is cold. No slope
                                        // check needed: steep shores took the rock
                                        // tiers above.
                                        land_cover::LC_BEACH => {
                                            if matches!(
                                                climate,
                                                crate::climate::Climate::Tundra
                                                    | crate::climate::Climate::IceCap
                                            ) && value_noise_01(x + 41, z + 5, 6) < 0.7
                                            {
                                                (GRAVEL, STONE)
                                            } else {
                                                (SAND, SANDSTONE)
                                            }
                                        }
                                        // LC_WATER handled above with variable depth
                                        land_cover::LC_WETLAND => (MUD, DIRT),
                                        land_cover::LC_MANGROVES => (MUD, DIRT),
                                        _ => (GRASS_BLOCK, DIRT),
                                    }
                                }
                            } else if let Some(p) = talus_block {
                                p
                            } else if terrain_enabled && slope > 4 {
                                // No land cover data: the same slope cascade, falling
                                // through to plain grass for the ≤4 slopes.
                                terrain_surface::steep_palette(x, z, ground_y, slope, 0)
                            } else {
                                (GRASS_BLOCK, DIRT)
                            };

                            // Shoreline blending: land blocks near water get sand
                            // surface for a natural beach/shore transition.
                            // Uses water_blend gradient for ESA water (scales with
                            // grid resolution) plus neighbor check for OSM water.
                            // Skip on steep terrain — canyon walls should stay rock.
                            // Nothing to blend into off Earth: no water, no beaches.
                            let (surface_block, under_block) = if planetary_body.is_none()
                                && surface_block != WATER
                                && slope <= 3
                            {
                                // Sand only at the immediate 1-cell ring around LC_WATER
                                // (plus near_placed_water below for OSM-rendered water).
                                let near_esa_water = has_land_cover
                                    && !is_esa_water
                                    && [
                                        (-1i32, 0i32),
                                        (1, 0),
                                        (0, -1),
                                        (0, 1),
                                        (-1, -1),
                                        (-1, 1),
                                        (1, -1),
                                        (1, 1),
                                    ]
                                    .iter()
                                    .any(|(dx, dz)| {
                                        ground.cover_class(XZPoint::new(
                                            x + dx - xzbbox.min_x(),
                                            z + dz - xzbbox.min_z(),
                                        )) == land_cover::LC_WATER
                                    });

                                // Also check placed water blocks (OSM rivers, etc.)
                                let near_placed_water = [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)]
                                    .iter()
                                    .any(|(dx, dz)| {
                                        editor.check_for_block_absolute(
                                            x + dx,
                                            ground_y,
                                            z + dz,
                                            Some(&[WATER]),
                                            None,
                                        )
                                    });
                                if near_esa_water || near_placed_water {
                                    (SAND, SANDSTONE)
                                } else {
                                    (surface_block, under_block)
                                }
                            } else {
                                (surface_block, under_block)
                            };

                            // Full snow cover takes over the surface, and so does any snow
                            // on ice, which the game won't hold as a layer. The under-block
                            // stays, so the faces of steps show the rock or ice below.
                            let surface_block = if (snow == terrain_surface::Snow::Block
                                || snow != terrain_surface::Snow::None
                                    && terrain_surface::is_ice(surface_block))
                                && water_blend <= 0.5
                                && surface_block != WATER
                            {
                                SNOW_BLOCK
                            } else {
                                surface_block
                            };

                            if steep_override {
                                // Force-replace existing OSM blocks on steep terrain
                                // Use blacklist to avoid replacing water/bedrock and
                                // common hard surfaces (roads/buildings). WHITE_CONCRETE
                                // protects lane-centre stripes and zebra crossings —
                                // without it, every dashed line on a hillside street
                                // gets buried under andesite/stone bricks by the
                                // slope-tier rock selector above.
                                editor.set_block_absolute(
                                    surface_block,
                                    x,
                                    ground_y,
                                    z,
                                    None,
                                    Some(&[
                                        WATER,
                                        BEDROCK,
                                        GRAY_CONCRETE_POWDER,
                                        CYAN_TERRACOTTA,
                                        GRAY_CONCRETE,
                                        LIGHT_GRAY_CONCRETE,
                                        WHITE_CONCRETE,
                                        YELLOW_CONCRETE,
                                        DIRT_PATH,
                                        STONE_BRICKS,
                                        BRICK,
                                        OAK_PLANKS,
                                        BLACK_CONCRETE,
                                    ]),
                                );
                            } else if talus_block.is_some_and(|(top, _)| top == surface_block) {
                                // Fallen rock buries a mapped meadow at the wall's foot too,
                                // never its roads, fields or paths.
                                editor.set_block_absolute(
                                    surface_block,
                                    x,
                                    ground_y,
                                    z,
                                    Some(terrain_surface::TALUS_BURIES),
                                    None,
                                );
                            } else {
                                editor.set_block_if_absent_absolute(surface_block, x, ground_y, z);
                            }

                            // Don't place dirt/under blocks below water surfaces.
                            // OSM water (rivers, lakes) is placed during element processing;
                            // placing dirt underneath would show through shallow water.
                            let top = editor.get_block_absolute(x, ground_y, z);
                            let surface_is_water = top == Some(WATER);

                            // Snow layers, also over any surface a mapped feature set before
                            // full cover could. Skip water (placed block or ESA-classified,
                            // e.g. a steep lake edge where rock sits at ground_y). On natural
                            // ground they rise with the unrounded terrain, while mapped
                            // surfaces only get a dusting.
                            if snow != terrain_surface::Snow::None
                                && !surface_is_water
                                && water_blend <= 0.5
                            {
                                let eighths =
                                    if top == Some(surface_block) || top == Some(PACKED_ICE) {
                                        let rise = terrain_enabled.then(|| {
                                            ground.level_exact(coord) - f64::from(ground_y) + 0.5
                                        });
                                        terrain_surface::snow_eighths(snow, rise)
                                    } else {
                                        1
                                    };
                                terrain_surface::place_snow(editor, x, ground_y, z, top, eighths);
                            }

                            if !surface_is_water {
                                // Fill under-blocks deep enough to seal any visible
                                // gap on cliff faces. Check all 8 neighbors (cardinal
                                // + diagonal) and fill down to the lowest neighbor's
                                // ground level so no void is ever visible.
                                let depth = if let Some(ref cache) = chunk_ground_cache {
                                    let mut min_neighbor_y = ground_y;
                                    for &(dx, dz) in &[
                                        (-1i32, 0i32),
                                        (1, 0),
                                        (0, -1),
                                        (0, 1),
                                        (-1, -1),
                                        (-1, 1),
                                        (1, -1),
                                        (1, 1),
                                    ] {
                                        let ny = cache.get(editor, x + dx, z + dz);
                                        if ny < min_neighbor_y {
                                            min_neighbor_y = ny;
                                        }
                                    }
                                    // Fill from ground_y-1 down toward the lowest
                                    // neighbor, capped to avoid excessive work on
                                    // extreme elevation changes (same cap as the
                                    // universal depth fill below).
                                    (ground_y - min_neighbor_y + 1).clamp(2, 64)
                                } else {
                                    2
                                };
                                let y_max = ground_y - 1;
                                if y_max > min_y() {
                                    let y_min = (ground_y - depth).max(min_y() + 1);
                                    // Rock faces show their bedding down the whole step.
                                    // Off Earth the column stays plain stone.
                                    if slope > 4 && under_block == STONE && planetary_body.is_none()
                                    {
                                        terrain_surface::fill_strata(
                                            editor,
                                            x,
                                            z,
                                            y_min,
                                            y_max,
                                            slope > 6,
                                        );
                                    } else {
                                        editor.fill_column_absolute(
                                            under_block,
                                            x,
                                            z,
                                            y_min,
                                            y_max,
                                            true,
                                        );
                                    }
                                }
                                did_underfill = true;
                            } else {
                                // Under OSM water: find bottom of water column,
                                // place sand/gravel/clay floor + sandstone below.
                                let mut water_bottom = ground_y;
                                while water_bottom - 1 > min_y()
                                    && editor.check_for_block_absolute(
                                        x,
                                        water_bottom - 1,
                                        z,
                                        Some(&[WATER]),
                                        None,
                                    )
                                {
                                    water_bottom -= 1;
                                }
                                let floor_y = water_bottom - 1;
                                if floor_y > min_y() {
                                    let h = land_cover::coord_hash(x, z);
                                    let floor_block = match h % 5 {
                                        0 => GRAVEL,
                                        1 => CLAY,
                                        _ => SAND,
                                    };
                                    editor.set_block_if_absent_absolute(floor_block, x, floor_y, z);
                                    if floor_y - 1 > min_y() {
                                        editor.set_block_if_absent_absolute(
                                            SANDSTONE,
                                            x,
                                            floor_y - 1,
                                            z,
                                        );
                                    }
                                }
                            }

                            // Place vegetation from ESA land cover classification
                            // Only if nothing was already placed above ground by OSM processing
                            // and the ground block is a natural surface (not a road, building slab, etc.)
                            // A road or pitch owns its column whatever block it ended up
                            // with, and surface=dirt or surface=grass make the block check
                            // below say "natural" on both.
                            let sealed = editor.surface_is_sealed(x, z);
                            // Podzol and moss come from the boreal and tundra palettes.
                            let ground_is_natural = !sealed
                                && editor.check_for_block_absolute(
                                    x,
                                    ground_y,
                                    z,
                                    Some(&[
                                        GRASS_BLOCK,
                                        COARSE_DIRT,
                                        DIRT,
                                        MUD,
                                        FARMLAND,
                                        PODZOL,
                                        MOSS_BLOCK,
                                    ]),
                                    None,
                                );
                            // Trees can also grow through stone surfaces (urban tree cover)
                            let ground_allows_trees = ground_is_natural
                                || (!sealed
                                    && editor.check_for_block_absolute(
                                        x,
                                        ground_y,
                                        z,
                                        Some(&[SMOOTH_STONE, STONE_BRICKS, CRACKED_STONE_BRICKS]),
                                        None,
                                    ));
                            // Where the canopy map reaches, it decides which columns get
                            // trees on any class, and land cover keeps the surface and the
                            // undergrowth. Its roll uses its own hash, so turning the option
                            // off leaves every other draw untouched.
                            let (canopy_covered, canopy_tree) = if has_canopy && has_land_cover {
                                canopy_verdict(
                                    ground,
                                    x,
                                    z,
                                    xzbbox.min_x(),
                                    xzbbox.min_z(),
                                    tree_spacing,
                                    schematic_trees,
                                )
                            } else {
                                (false, false)
                            };
                            // Placed before the vegetation pass, whose own guard then sees
                            // the trunk and leaves the column alone.
                            if canopy_tree
                                && slope <= 4
                                && ground_allows_trees
                                && !tunnel_footprint.contains(x, z)
                                && !editor.block_exists_absolute(x, ground_y + 1, z)
                                && !editor.under_mapped_crown(x, z)
                            {
                                tree::Tree::create_from_canopy(
                                    editor,
                                    (x, 1, z),
                                    Some(building_footprints),
                                    Some(bridge_surface),
                                );
                            }
                            if has_land_cover
                                && !sealed
                                && !editor.block_exists_absolute(x, ground_y + 1, z)
                            {
                                let cover = ground.cover_class(coord);
                                let mut rng = crate::deterministic_rng::coord_rng(x, z, 0);
                                // Worn coarse-dirt ground carries half the grass of a lawn.
                                let worn = editor.check_for_block_absolute(
                                    x,
                                    ground_y,
                                    z,
                                    Some(&[COARSE_DIRT]),
                                    None,
                                );
                                let sward = climate_sward * if worn { 0.5 } else { 1.0 };

                                match cover {
                                    land_cover::LC_TREE_COVER
                                        if slope <= 4
                                            && ground_allows_trees
                                            && !tunnel_footprint.contains(x, z) =>
                                    {
                                        // Micro trees cover ~5 blocks instead of ~80, so the
                                        // normal rate would read as bare ground from altitude.
                                        let tree_rate = if args.scale
                                            < crate::element_processing::tree::MICRO_TREE_MAX_SCALE
                                        {
                                            4
                                        } else {
                                            30
                                        };
                                        let choice = rng.random_range(0..tree_rate);
                                        if choice == 0
                                            && !canopy_covered
                                            && !editor.under_mapped_crown(x, z)
                                        {
                                            tree::Tree::create(
                                                editor,
                                                (x, 1, z),
                                                Some(building_footprints),
                                                Some(bridge_surface),
                                            );
                                        } else if ground_is_natural
                                            && undergrowth_roll(
                                                x,
                                                z,
                                                0.4 * sward,
                                                SALT_FOREST_FLOOR,
                                            )
                                        {
                                            // Undergrowth only on natural surfaces. Flowers
                                            // come in patches from the decoration pass.
                                            let fern = (land_cover::coord_hash(x ^ 0xFE, z ^ 0x4E)
                                                % 100)
                                                as f64
                                                / 100.0
                                                < forest_fern_share;
                                            editor.set_block_absolute(
                                                if fern { FERN } else { GRASS },
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                        }
                                    }
                                    land_cover::LC_SHRUBLAND if ground_is_natural => {
                                        let choice = rng.random_range(0..100);
                                        if choice < 2 {
                                            crate::element_processing::bush::place_bush(
                                                editor,
                                                x,
                                                z,
                                                crate::element_processing::bush::BushKind::Wild,
                                            );
                                        } else if undergrowth_roll(
                                            x,
                                            z,
                                            0.28 * sward,
                                            SALT_SHRUB_FLOOR,
                                        ) {
                                            editor.set_block_absolute(
                                                GRASS,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                        }
                                    }
                                    // Short grass on grassland (~55%), thick in some
                                    // stretches and thin in others, with tall grass
                                    // gathered in its own stands.
                                    land_cover::LC_GRASSLAND
                                        if ground_is_natural
                                            && undergrowth_roll(x, z, 0.55 * sward, SALT_SWARD) =>
                                    {
                                        let stand = patch_noise(x, z, 9, SALT_TALL_SWARD) > 0.8;
                                        let tall_share = if stand { 35 } else { 4 };
                                        if rng.random_range(0..100) < tall_share {
                                            editor.set_block_absolute(
                                                TALL_GRASS_BOTTOM,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                            editor.set_block_absolute(
                                                TALL_GRASS_TOP,
                                                x,
                                                ground_y + 2,
                                                z,
                                                None,
                                                None,
                                            );
                                        } else {
                                            editor.set_block_absolute(
                                                GRASS,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                        }
                                    }
                                    land_cover::LC_CROPLAND
                                        if editor.check_for_block_absolute(
                                            x,
                                            ground_y,
                                            z,
                                            Some(&[FARMLAND]),
                                            None,
                                        ) =>
                                    {
                                        // Irrigation dots, but only where boxed in so they can't flow downhill and wash out crops.
                                        if x % 9 == 0
                                            && z % 9 == 0
                                            && editor.water_source_is_enclosed(x, z)
                                        {
                                            editor.set_block_absolute(
                                                WATER,
                                                x,
                                                ground_y,
                                                z,
                                                Some(&[FARMLAND]),
                                                None,
                                            );
                                        } else if rng.random_range(0..76) == 0 {
                                            if rng.random_range(1..=10) <= 4 {
                                                editor.set_block_absolute(
                                                    HAY_BALE,
                                                    x,
                                                    ground_y + 1,
                                                    z,
                                                    None,
                                                    None,
                                                );
                                            }
                                        } else {
                                            let crop =
                                                [WHEAT, CARROTS, POTATOES][rng.random_range(0..3)];
                                            editor.set_block_absolute(
                                                crop,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                        }
                                    }
                                    land_cover::LC_WETLAND | land_cover::LC_MANGROVES
                                        if ground_is_natural =>
                                    {
                                        let choice = rng.random_range(0..100);
                                        // Standing water in pools rather than single-block
                                        // holes, and only where it cannot run off downhill.
                                        if patch_noise(x, z, 5, SALT_WETLAND_POOLS) < 0.28
                                            && editor.water_source_is_enclosed(x, z)
                                        {
                                            editor.set_block_absolute(
                                                WATER,
                                                x,
                                                ground_y,
                                                z,
                                                Some(&[MUD, GRASS_BLOCK]),
                                                None,
                                            );
                                        } else if choice < 50 {
                                            editor.set_block_absolute(
                                                GRASS,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                        } else if choice < 64 {
                                            editor.set_block_absolute(
                                                TALL_GRASS_BOTTOM,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                            editor.set_block_absolute(
                                                TALL_GRASS_TOP,
                                                x,
                                                ground_y + 2,
                                                z,
                                                None,
                                                None,
                                            );
                                        }
                                    }
                                    land_cover::LC_BARE if ground_is_natural => {
                                        // Coarse-dirt patches (from the bare-terrain soil
                                        // blobs above) get a light scatter of weeds: a bit
                                        // of grass with rare fallen-leaf clumps and dead
                                        // bushes. Kept sparse on purpose so the terrain
                                        // still reads as bare/arid rather than shrubland.
                                        // Other bare surfaces (stone/gravel scree) keep
                                        // only the original occasional dead bush.
                                        let on_coarse_dirt = editor.check_for_block_absolute(
                                            x,
                                            ground_y,
                                            z,
                                            Some(&[COARSE_DIRT]),
                                            None,
                                        );
                                        if on_coarse_dirt {
                                            match rng.random_range(0..100) {
                                                0..=5 => editor.set_block_absolute(
                                                    GRASS,
                                                    x,
                                                    ground_y + 1,
                                                    z,
                                                    None,
                                                    None,
                                                ),
                                                6..=8 => crate::element_processing::bush::place_bush(
                                                    editor,
                                                    x,
                                                    z,
                                                    crate::element_processing::bush::BushKind::Low,
                                                ),
                                                9 => editor.set_block_absolute(
                                                    DEAD_BUSH,
                                                    x,
                                                    ground_y + 1,
                                                    z,
                                                    None,
                                                    None,
                                                ),
                                                _ => {}
                                            }
                                        } else if rng.random_range(0..100) == 0 {
                                            editor.set_block_absolute(
                                                DEAD_BUSH,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        } // end else (non-water)
                    }

                    // Depth fill: ensure ALL columns have under-blocks deep enough
                    // to seal cliff faces. This runs unconditionally (even for columns
                    // skipped above because OSM already placed a surface block) so that
                    // quarries, landuse areas, and other OSM elements on slopes don't
                    // leave visible gaps. Uses set_block_if_absent so it won't overwrite
                    // material-specific under-blocks already placed above.
                    if let Some(ref cache) = chunk_ground_cache {
                        if !editor.check_for_block_absolute(x, ground_y, z, Some(&[WATER]), None)
                            && !did_underfill
                        {
                            let mut min_neighbor_y = ground_y;
                            for &(dx, dz) in &[
                                (-1i32, 0i32),
                                (1, 0),
                                (0, -1),
                                (0, 1),
                                (-1, -1),
                                (-1, 1),
                                (1, -1),
                                (1, 1),
                            ] {
                                let ny = cache.get(editor, x + dx, z + dz);
                                if ny < min_neighbor_y {
                                    min_neighbor_y = ny;
                                }
                            }
                            let depth = (ground_y - min_neighbor_y + 1).clamp(2, 64);
                            let y_max = ground_y - 1;
                            let y_min = (ground_y - depth).max(min_y() + 1);
                            if y_min <= y_max {
                                editor.fill_column_absolute(STONE, x, z, y_min, y_max, true);
                            }
                        }
                    }

                    // Post-processing: remove stray vegetation from road and water surfaces.
                    // Despite guards in natural/landuse processing, overlapping elements
                    // with the same priority can still place vegetation on roads depending
                    // on sort order, and an area that floods its ground later (a wetland
                    // puddle, a shoal) leaves another area's plants floating on the water.
                    // A plant with a trunk or branch right above it reads as having
                    // taken the wood's place, so it goes too. This cleanup pass
                    // catches any remaining cases.
                    let stray_surface = editor.check_for_block_absolute(
                        x,
                        ground_y,
                        z,
                        Some(&[
                            BLACK_CONCRETE,
                            GRAY_CONCRETE_POWDER,
                            CYAN_TERRACOTTA,
                            GRAY_CONCRETE,
                            LIGHT_GRAY_CONCRETE,
                            WHITE_CONCRETE,
                            YELLOW_CONCRETE,
                            DIRT_PATH,
                            WATER,
                        ]),
                        None,
                    );
                    if editor.check_for_block_absolute(x, ground_y + 1, z, Some(LOOSE_PLANTS), None)
                        && (stray_surface
                            || editor.check_for_block_absolute(
                                x,
                                ground_y + 2,
                                z,
                                Some(WOOD),
                                None,
                            ))
                    {
                        editor.set_block_absolute(
                            AIR,
                            x,
                            ground_y + 1,
                            z,
                            Some(LOOSE_PLANTS),
                            None,
                        );
                        // Also clear the top of a two-block plant, or a stack of cane.
                        for y in ground_y + 2..=ground_y + 3 {
                            if !editor.check_for_block_absolute(
                                x,
                                y,
                                z,
                                Some(STACKED_PLANT_PARTS),
                                None,
                            ) {
                                break;
                            }
                            editor.set_block_absolute(
                                AIR,
                                x,
                                y,
                                z,
                                Some(STACKED_PLANT_PARTS),
                                None,
                            );
                        }
                    }

                    // Fill underground; column_fill_y_min skips already-Uniform sections.
                    if args.fillground {
                        editor.fill_column_absolute(
                            STONE,
                            x,
                            z,
                            column_fill_y_min,
                            ground_y - 3,
                            true, // skip_existing: don't overwrite blocks placed by element processing
                        );
                    }
                    // Bedrock is a flat plane at the terrain floor, as in vanilla.
                    editor.set_block_absolute(
                        BEDROCK,
                        x,
                        terrain_floor_y(),
                        z,
                        None,
                        Some(&[BEDROCK]),
                    );

                    water_memo.forget(x, z);
                    block_counter += 1;
                    #[allow(clippy::manual_is_multiple_of)]
                    if block_counter % batch_size == 0 {
                        ground_pb.inc(batch_size);
                    }

                    if show_progress {
                        gui_progress_grnd += progress_increment_grnd;
                        if (gui_progress_grnd - last_emitted_progress).abs() > 0.25 {
                            emit_gui_progress_update(gui_progress_grnd, "");
                            last_emitted_progress = gui_progress_grnd;
                        }
                    }
                }
            }
        }

        // Regions stay in memory and are saved in parallel by save_java()
        // at the end of generation for maximum throughput.
    }

    // Plant patches need every column of the region finished first.
    crate::ground_decoration::decorate_region(
        editor, ground, args, xzbbox, iter_min_x, iter_max_x, iter_min_z, iter_max_z,
    );

    ground_pb.inc(block_counter % batch_size);
    ground_pb.finish();
}

/// Whether a column carries undergrowth: `mean` of the ground on average, but
/// thick in some stretches and thin in others, as the game's grass patches are.
fn undergrowth_roll(x: i32, z: i32, mean: f64, salt: u32) -> bool {
    let density = (mean * (0.3 + 1.4 * patch_noise(x, z, 11, salt))).min(0.95);
    let roll = land_cover::coord_hash(x ^ salt as i32, z ^ salt.rotate_left(9) as i32) % 1000;
    (roll as f64) < density * 1000.0
}

/// Smooth scalar noise in `[0, 1]` at approximately `scale`-block resolution.
///
/// Used for organic surface-material patches (coarse-dirt vs rock, etc.).
/// Works by sampling the deterministic coord_hash at the four corners of
/// a `scale × scale` lattice cell containing `(x, z)`, then bilinearly
/// interpolating with a cubic Hermite smoothstep so the boundaries between
/// high- and low-noise regions curve rather than snap along axis-aligned
/// lattice edges. Compared to `coord_hash(x / scale, z / scale)` (which
/// produces the rectangular patches seen in the first iteration of the
/// patch code) the output has organic blob-shaped contours.
///
/// Cost: 4 hash calls + a few f64 ops per block — still well under 100 ns,
/// negligible over the whole ground pass.
pub(crate) fn value_noise_01(x: i32, z: i32, scale: i32) -> f64 {
    let s = scale.max(1);
    // Integer lattice cell containing (x, z). div_euclid gives floor
    // division for negative coordinates too, so patches tile uniformly
    // across the origin.
    let x0 = x.div_euclid(s) * s;
    let z0 = z.div_euclid(s) * s;
    let x1 = x0 + s;
    let z1 = z0 + s;
    // Fractional position inside the cell.
    let tx = (x - x0) as f64 / s as f64;
    let tz = (z - z0) as f64 / s as f64;
    // Cubic Hermite smoothstep: derivative = 0 at both ends, so neighbouring
    // cells join smoothly instead of with a visible slope change.
    let fx = tx * tx * (3.0 - 2.0 * tx);
    let fz = tz * tz * (3.0 - 2.0 * tz);
    let sample = |x: i32, z: i32| (land_cover::coord_hash(x, z) % 1000) as f64 / 1000.0;
    let v00 = sample(x0, z0);
    let v10 = sample(x1, z0);
    let v01 = sample(x0, z1);
    let v11 = sample(x1, z1);
    let a = v00 * (1.0 - fx) + v10 * fx;
    let b = v01 * (1.0 - fx) + v11 * fx;
    a * (1.0 - fz) + b * fz
}

/// `value_noise_01` on its own lattice, so layers with different salts are
/// independent instead of shifted copies of one field. The lattice is turned
/// about 27 degrees to the block grid, so patches don't come out boxy and
/// lined up with the axes.
pub(crate) fn value_noise_salted(x: i32, z: i32, scale: i32, salt: u32) -> f64 {
    const COS: f64 = 0.891_006_524_188_368;
    const SIN: f64 = 0.453_990_499_739_547;
    let s = f64::from(scale.max(1));
    let (fx, fz) = (f64::from(x), f64::from(z));
    let u = (fx * COS - fz * SIN) / s;
    let v = (fx * SIN + fz * COS) / s;
    let (u0, v0) = (u.floor(), v.floor());
    let (tu, tv) = (u - u0, v - v0);
    let su = tu * tu * (3.0 - 2.0 * tu);
    let sv = tv * tv * (3.0 - 2.0 * tv);
    let (cu, cv) = (u0 as i32, v0 as i32);
    let salt_x = salt as i32;
    let salt_z = salt.rotate_left(16) as i32;
    let sample = |cx: i32, cz: i32| {
        (land_cover::coord_hash(cx ^ salt_x, cz ^ salt_z) % 1000) as f64 / 1000.0
    };
    let a = sample(cu, cv) * (1.0 - su) + sample(cu + 1, cv) * su;
    let b = sample(cu, cv + 1) * (1.0 - su) + sample(cu + 1, cv + 1) * su;
    a * (1.0 - sv) + b * sv
}

/// Smooth noise remapped to a roughly uniform `[0, 1]`, so `patch_noise(..) < 0.2`
/// covers about a fifth of the ground in patches about `scale` blocks across.
///
/// Bilinear value noise piles up around 0.5; the table is its measured quantiles.
pub(crate) fn patch_noise(x: i32, z: i32, scale: i32, salt: u32) -> f64 {
    const QUANTILES: [(f64, f64); 13] = [
        (0.0, 0.0),
        (0.15, 0.05),
        (0.21, 0.1),
        (0.30, 0.2),
        (0.37, 0.3),
        (0.435, 0.4),
        (0.496, 0.5),
        (0.558, 0.6),
        (0.625, 0.7),
        (0.702, 0.8),
        (0.796, 0.9),
        (0.858, 0.95),
        (1.0, 1.0),
    ];
    let v = value_noise_salted(x, z, scale, salt);
    for pair in QUANTILES.windows(2) {
        let ((v0, q0), (v1, q1)) = (pair[0], pair[1]);
        if v <= v1 {
            return q0 + (v - v0) / (v1 - v0) * (q1 - q0);
        }
    }
    1.0
}
