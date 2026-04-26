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
    AIR, ANDESITE, BEDROCK, BLACK_CONCRETE, BLUE_FLOWER, BRICK, CARROTS, CLAY, COARSE_DIRT,
    COBBLED_DEEPSLATE, COBBLESTONE, CRACKED_STONE_BRICKS, CYAN_TERRACOTTA, DEAD_BUSH, DEEPSLATE,
    DIRT, DIRT_PATH, FARMLAND, GRASS, GRASS_BLOCK, GRAVEL, GRAY_CONCRETE, GRAY_CONCRETE_POWDER,
    HAY_BALE, LIGHT_GRAY_CONCRETE, MUD, OAK_LEAVES, OAK_PLANKS, POTATOES, RED_FLOWER, SAND,
    SANDSTONE, SMOOTH_STONE, STONE, STONE_BRICKS, TALL_GRASS_BOTTOM, TALL_GRASS_TOP, TUFF, WATER,
    WHEAT, WHITE_CONCRETE, WHITE_FLOWER, YELLOW_FLOWER,
};
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::element_processing::tree;
use crate::floodfill_cache::BuildingFootprintBitmap;
use crate::ground::Ground;
use crate::land_cover;
use crate::progress::emit_gui_progress_update;
use crate::world_editor::WorldEditor;
use crate::world_editor::MIN_Y;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use rand::Rng;

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
) -> Result<(), String> {
    let has_land_cover = ground.has_land_cover();
    let terrain_enabled = ground.elevation_enabled;

    let total_blocks: u64 = xzbbox.bounding_rect().total_blocks();
    let desired_updates: u64 = 1500;
    let batch_size: u64 = (total_blocks / desired_updates).max(1);

    let mut block_counter: u64 = 0;

    println!("{} Generating ground...", "[6/7]".bold());
    emit_gui_progress_update(70.0, "Generating ground...");

    let ground_pb: ProgressBar = ProgressBar::new(total_blocks);
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
    let min_chunk_x = xzbbox.min_x() >> 4;
    let max_chunk_x = xzbbox.max_x() >> 4;
    let min_chunk_z = xzbbox.min_z() >> 4;
    let max_chunk_z = xzbbox.max_z() >> 4;

    for chunk_x in min_chunk_x..=max_chunk_x {
        for chunk_z in min_chunk_z..=max_chunk_z {
            // Calculate the block range for this chunk, clamped to bbox
            let chunk_min_x = (chunk_x << 4).max(xzbbox.min_x());
            let chunk_max_x = ((chunk_x << 4) + 15).min(xzbbox.max_x());
            let chunk_min_z = (chunk_z << 4).max(xzbbox.min_z());
            let chunk_max_z = ((chunk_z << 4) + 15).min(xzbbox.max_z());

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

                    // Compute slope once for this column (used for surface selection and depth)
                    let slope = if terrain_enabled {
                        ground.slope(coord)
                    } else {
                        0
                    };

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
                    let steep_override = terrain_enabled && slope > 4;
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
                        let placed_water = has_water_in_column(x, z);
                        let osm_gap = if placed_water {
                            false
                        } else {
                            let water_n = has_water_in_column(x, z - 1);
                            let water_s = has_water_in_column(x, z + 1);
                            let water_w = has_water_in_column(x - 1, z);
                            let water_e = has_water_in_column(x + 1, z);
                            let water_ne = has_water_in_column(x + 1, z - 1);
                            let water_nw = has_water_in_column(x - 1, z - 1);
                            let water_se = has_water_in_column(x + 1, z + 1);
                            let water_sw = has_water_in_column(x - 1, z + 1);

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
                            }
                        }

                        if place_esa_water {
                            // Single block of water at snapped level
                            editor.set_block_if_absent_absolute(WATER, x, water_y, z);

                            // Floor: sand/gravel/clay + sandstone below
                            let h = land_cover::coord_hash(x, z);
                            let floor_block = match h % 5 {
                                0 => GRAVEL,
                                1 => CLAY,
                                _ => SAND,
                            };
                            if water_y - 1 > MIN_Y {
                                editor.set_block_if_absent_absolute(floor_block, x, water_y - 1, z);
                            }
                            if water_y - 2 > MIN_Y {
                                editor.set_block_if_absent_absolute(SANDSTONE, x, water_y - 2, z);
                            }
                        } else {
                            // Determine surface and sub-surface blocks based on available data
                            let (surface_block, under_block) = if has_land_cover {
                                // ESA WorldCover + slope-based material selection
                                let cover = ground.cover_class(coord);

                                // Steep terrain overrides land cover classification.
                                //
                                // slope is max-min of 4 cardinal neighbours sampled
                                // STEP=4 away, so `slope = 8 · tan(incline)`. Thresholds:
                                //
                                //   slope > 8  → ≥ 45° : sheer cliff face
                                //   slope > 6  → ≥ 37° : very steep rocky face
                                //   slope > 4  → ≥ 27° : steep slope with scree
                                //   slope ≤ 4  → < 27° : falls through to land cover
                                //                        (alpine meadow, forest, etc.)
                                //
                                // We don't force rock materials onto 21–27° slopes
                                // any more — that's a normal hiking incline where
                                // grass and trees belong.
                                if slope > 8 {
                                    // Sheer cliff: each column is 100% one material
                                    // so the downward under-fill matches the surface,
                                    // producing vertical stripes of cobbled/deepslate.
                                    let h = land_cover::coord_hash(x, z);
                                    if h.is_multiple_of(2) {
                                        (COBBLED_DEEPSLATE, COBBLED_DEEPSLATE)
                                    } else {
                                        (DEEPSLATE, DEEPSLATE)
                                    }
                                } else if slope > 6 {
                                    // Very steep rock face: stone-dominant with
                                    // weathered cobblestone chunks and occasional
                                    // andesite banding. Deepslate stays below-surface
                                    // only — it would read as "cliff" if exposed here.
                                    let h = land_cover::coord_hash(x, z) % 20;
                                    if h < 12 {
                                        (STONE, DEEPSLATE) // 60%
                                    } else if h < 17 {
                                        (COBBLESTONE, DEEPSLATE) // 25%
                                    } else {
                                        (ANDESITE, DEEPSLATE) // 15%
                                    }
                                } else if slope > 4 {
                                    // Steep slope with natural scree: rocky mix where
                                    // the gravel is a minority patch (not the whole
                                    // surface) so it looks like real scree rather
                                    // than a grey slope.
                                    let h = land_cover::coord_hash(x, z) % 12;
                                    match h {
                                        0..=3 => (ANDESITE, STONE),    // 33%
                                        4..=5 => (TUFF, STONE),        // 17%
                                        6..=7 => (STONE, STONE),       // 17%
                                        8..=9 => (COBBLESTONE, STONE), // 17%
                                        _ => (GRAVEL, STONE),          // 17% scree
                                    }
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
                                                    })
                                                    .count();
                                            if neighbors_bare == 0 {
                                                // Isolated pixel - blend with surroundings
                                                (GRASS_BLOCK, DIRT)
                                            } else {
                                                // Bare/sparse terrain: soil patches
                                                // interspersed with varied rock. Value
                                                // noise at ~6-block resolution groups
                                                // coarse dirt into organic earth
                                                // patches (rather than scattering it
                                                // as single pixels whose warm brown
                                                // stands out against grey rock), then
                                                // a finer per-block hash picks the
                                                // specific block within each zone.
                                                let noise = value_noise_01(x, z, 6);
                                                let h = land_cover::coord_hash(x, z);
                                                // Threshold 0.45 → roughly 30 % dirt
                                                // coverage given the bell-shaped
                                                // distribution of bilinear-interpolated
                                                // uniform samples.
                                                if noise < 0.45 {
                                                    match h % 10 {
                                                        0..=7 => (COARSE_DIRT, DIRT), // 80% inside dirt patch
                                                        _ => (STONE, STONE), // 20% stone poking through
                                                    }
                                                } else {
                                                    match h % 12 {
                                                        0..=3 => (STONE, STONE),       // 33%
                                                        4..=5 => (ANDESITE, STONE),    // 17%
                                                        6..=7 => (COBBLESTONE, STONE), // 17%
                                                        8..=9 => (GRAVEL, STONE),      // 17% scree
                                                        _ => (ANDESITE, STONE), // 17% more andesite
                                                    }
                                                }
                                            }
                                        }
                                        // LC_WATER handled above with variable depth
                                        land_cover::LC_WETLAND => (MUD, DIRT),
                                        land_cover::LC_MANGROVES => (MUD, DIRT),
                                        _ => (GRASS_BLOCK, DIRT),
                                    }
                                }
                            } else if terrain_enabled {
                                // No land cover data: same slope-based cascade
                                // as the has_land_cover path, falling through
                                // to plain grass for the ≤4 slopes (no ESA
                                // class to pick instead).
                                if slope > 8 {
                                    let h = land_cover::coord_hash(x, z);
                                    if h.is_multiple_of(2) {
                                        (COBBLED_DEEPSLATE, COBBLED_DEEPSLATE)
                                    } else {
                                        (DEEPSLATE, DEEPSLATE)
                                    }
                                } else if slope > 6 {
                                    let h = land_cover::coord_hash(x, z) % 20;
                                    if h < 12 {
                                        (STONE, DEEPSLATE)
                                    } else if h < 17 {
                                        (COBBLESTONE, DEEPSLATE)
                                    } else {
                                        (ANDESITE, DEEPSLATE)
                                    }
                                } else if slope > 4 {
                                    let h = land_cover::coord_hash(x, z) % 12;
                                    match h {
                                        0..=3 => (ANDESITE, STONE),
                                        4..=5 => (TUFF, STONE),
                                        6..=7 => (STONE, STONE),
                                        8..=9 => (COBBLESTONE, STONE),
                                        _ => (GRAVEL, STONE),
                                    }
                                } else {
                                    (GRASS_BLOCK, DIRT)
                                }
                            } else {
                                (GRASS_BLOCK, DIRT)
                            };

                            // Shoreline blending: land blocks near water get sand
                            // surface for a natural beach/shore transition.
                            // Uses water_blend gradient for ESA water (scales with
                            // grid resolution) plus neighbor check for OSM water.
                            // Skip on steep terrain — canyon walls should stay rock.
                            let (surface_block, under_block) = if surface_block != WATER
                                && slope <= 3
                            {
                                // Transition-zone blocks that noise decided are "not
                                // water" get sand via water_blend > 0.01 (at least one
                                // surrounding grid cell is water).  For blocks fully
                                // outside the blend zone, fall back to neighbor check.
                                let near_esa_water = has_land_cover
                                    && !is_esa_water
                                    && (water_blend > 0.01
                                        || [
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
                                        }));

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
                                        DIRT_PATH,
                                        STONE_BRICKS,
                                        BRICK,
                                        OAK_PLANKS,
                                        BLACK_CONCRETE,
                                    ]),
                                );
                            } else {
                                editor.set_block_if_absent_absolute(surface_block, x, ground_y, z);
                            }

                            // Don't place dirt/under blocks below water surfaces.
                            // OSM water (rivers, lakes) is placed during element processing;
                            // placing dirt underneath would show through shallow water.
                            let surface_is_water = editor.check_for_block_absolute(
                                x,
                                ground_y,
                                z,
                                Some(&[WATER]),
                                None,
                            );
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
                                if y_max > MIN_Y {
                                    let y_min = (ground_y - depth).max(MIN_Y + 1);
                                    editor.fill_column_absolute(
                                        under_block,
                                        x,
                                        z,
                                        y_min,
                                        y_max,
                                        true,
                                    );
                                }
                                did_underfill = true;
                            } else {
                                // Under OSM water: find bottom of water column,
                                // place sand/gravel/clay floor + sandstone below.
                                let mut water_bottom = ground_y;
                                while water_bottom - 1 > MIN_Y
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
                                if floor_y > MIN_Y {
                                    let h = land_cover::coord_hash(x, z);
                                    let floor_block = match h % 5 {
                                        0 => GRAVEL,
                                        1 => CLAY,
                                        _ => SAND,
                                    };
                                    editor.set_block_if_absent_absolute(floor_block, x, floor_y, z);
                                    if floor_y - 1 > MIN_Y {
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
                            let ground_is_natural = editor.check_for_block_absolute(
                                x,
                                ground_y,
                                z,
                                Some(&[GRASS_BLOCK, COARSE_DIRT, DIRT, MUD, FARMLAND]),
                                None,
                            );
                            // Trees can also grow through stone surfaces (urban tree cover)
                            let ground_allows_trees = ground_is_natural
                                || editor.check_for_block_absolute(
                                    x,
                                    ground_y,
                                    z,
                                    Some(&[SMOOTH_STONE, STONE_BRICKS, CRACKED_STONE_BRICKS]),
                                    None,
                                );
                            if has_land_cover && !editor.block_exists_absolute(x, ground_y + 1, z) {
                                let cover = ground.cover_class(coord);
                                let mut rng = crate::deterministic_rng::coord_rng(x, z, 0);

                                match cover {
                                    land_cover::LC_TREE_COVER
                                        if slope <= 4 && ground_allows_trees =>
                                    {
                                        let choice = rng.random_range(0..30);
                                        if choice == 0 {
                                            tree::Tree::create(
                                                editor,
                                                (x, 1, z),
                                                Some(building_footprints),
                                            );
                                        } else if ground_is_natural {
                                            // Undergrowth only on natural surfaces
                                            if choice == 1 {
                                                let flower = [
                                                    RED_FLOWER,
                                                    BLUE_FLOWER,
                                                    YELLOW_FLOWER,
                                                    WHITE_FLOWER,
                                                ][rng.random_range(0..4)];
                                                editor.set_block_absolute(
                                                    flower,
                                                    x,
                                                    ground_y + 1,
                                                    z,
                                                    None,
                                                    None,
                                                );
                                            } else if choice <= 13 {
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
                                    }
                                    land_cover::LC_SHRUBLAND if ground_is_natural => {
                                        let choice = rng.random_range(0..100);
                                        if choice < 2 {
                                            editor.set_block_absolute(
                                                OAK_LEAVES,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                        } else if choice < 30 {
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
                                    land_cover::LC_GRASSLAND if ground_is_natural => {
                                        // Short grass on grassland (~55%)
                                        let choice = rng.random_range(0..100);
                                        if choice < 50 {
                                            editor.set_block_absolute(
                                                GRASS,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                        } else if choice < 55 {
                                            // Occasional tall grass
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
                                        } else if choice == 55 {
                                            let flower = [
                                                RED_FLOWER,
                                                BLUE_FLOWER,
                                                YELLOW_FLOWER,
                                                WHITE_FLOWER,
                                            ][rng.random_range(0..4)];
                                            editor.set_block_absolute(
                                                flower,
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
                                        if x % 9 == 0 && z % 9 == 0 {
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
                                        if choice < 30 {
                                            // Water patches in wetlands
                                            editor.set_block_absolute(
                                                WATER,
                                                x,
                                                ground_y,
                                                z,
                                                Some(&[MUD, GRASS_BLOCK]),
                                                None,
                                            );
                                        } else if choice < 65 {
                                            editor.set_block_absolute(
                                                GRASS,
                                                x,
                                                ground_y + 1,
                                                z,
                                                None,
                                                None,
                                            );
                                        } else if choice < 75 {
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
                                                6..=8 => editor.set_block_absolute(
                                                    OAK_LEAVES,
                                                    x,
                                                    ground_y + 1,
                                                    z,
                                                    None,
                                                    None,
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
                            let y_min = (ground_y - depth).max(MIN_Y + 1);
                            if y_min <= y_max {
                                editor.fill_column_absolute(STONE, x, z, y_min, y_max, true);
                            }
                        }
                    }

                    // Post-processing: remove stray vegetation from road surfaces.
                    // Despite guards in natural/landuse processing, overlapping elements
                    // with the same priority can still place vegetation on roads depending
                    // on sort order. This cleanup pass catches any remaining cases.
                    if editor.check_for_block_absolute(
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
                            DIRT_PATH,
                        ]),
                        None,
                    ) && editor.check_for_block_absolute(
                        x,
                        ground_y + 1,
                        z,
                        Some(&[
                            GRASS,
                            OAK_LEAVES,
                            DEAD_BUSH,
                            TALL_GRASS_BOTTOM,
                            RED_FLOWER,
                            BLUE_FLOWER,
                            WHITE_FLOWER,
                            YELLOW_FLOWER,
                        ]),
                        None,
                    ) {
                        editor.set_block_absolute(
                            AIR,
                            x,
                            ground_y + 1,
                            z,
                            Some(&[
                                GRASS,
                                OAK_LEAVES,
                                DEAD_BUSH,
                                TALL_GRASS_BOTTOM,
                                RED_FLOWER,
                                BLUE_FLOWER,
                                WHITE_FLOWER,
                                YELLOW_FLOWER,
                            ]),
                            None,
                        );
                        // Also clear tall grass top if it was a two-block plant
                        if editor.check_for_block_absolute(
                            x,
                            ground_y + 2,
                            z,
                            Some(&[TALL_GRASS_TOP]),
                            None,
                        ) {
                            editor.set_block_absolute(
                                AIR,
                                x,
                                ground_y + 2,
                                z,
                                Some(&[TALL_GRASS_TOP]),
                                None,
                            );
                        }
                    }

                    // Fill underground with stone
                    if args.fillground {
                        editor.fill_column_absolute(
                            STONE,
                            x,
                            z,
                            MIN_Y + 1,
                            ground_y - 3,
                            true, // skip_existing: don't overwrite blocks placed by element processing
                        );
                    }
                    // Generate a bedrock level at MIN_Y
                    editor.set_block_absolute(BEDROCK, x, MIN_Y, z, None, Some(&[BEDROCK]));

                    block_counter += 1;
                    #[allow(clippy::manual_is_multiple_of)]
                    if block_counter % batch_size == 0 {
                        ground_pb.inc(batch_size);
                    }

                    gui_progress_grnd += progress_increment_grnd;
                    if (gui_progress_grnd - last_emitted_progress).abs() > 0.25 {
                        emit_gui_progress_update(gui_progress_grnd, "");
                        last_emitted_progress = gui_progress_grnd;
                    }
                }
            }
        }

        // Regions stay in memory and are saved in parallel by save_java()
        // at the end of generation for maximum throughput.
    }

    ground_pb.inc(block_counter % batch_size);
    ground_pb.finish();

    Ok(())
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
fn value_noise_01(x: i32, z: i32, scale: i32) -> f64 {
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
