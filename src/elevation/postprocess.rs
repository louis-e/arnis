use crate::land_cover::{LandCoverData, LC_BUILT_UP, LC_WATER};
use fnv::{FnvHashMap, FnvHashSet};
use rayon::prelude::*;
use std::collections::VecDeque;

/// Maximum Y coordinate in Minecraft (vanilla build height limit).
const MAX_Y: i32 = 319;

/// Buffer at the top for buildings, trees, and other structures
pub(crate) const TERRAIN_HEIGHT_BUFFER: i32 = 15;

/// Largest water component a steep-slope shadow blob can be. Real bodies are bigger.
const MAX_STEEP_WATER_AREA_M2: f64 = 250_000.0;

/// Median slope above which small water is shadow, not water. 19 degrees.
const MIN_STEEP_WATER_SLOPE: f64 = 0.35;

/// How far below a cell the ground a step away must sit for that cell to count as perched.
const STEEP_WATER_LAND_BELOW_M: f64 = 2.0;

/// Share of a component's edge cells that must be perched. Only a blob's downhill side
/// is, so this is low; the slope gate above is what does the separating.
const MIN_PERCHED_FRACTION: f64 = 0.10;

/// Water cells up to this far above the surface are noise or mixed ESA pixels; higher are walls.
const WATER_UP_TOLERANCE_M: f64 = 2.0;

/// Fall per metre toward the middle of a channel past which a cell is its wall. 19 degrees,
/// steeper than the banks a coarse DEM smears into a wide river.
const CHANNEL_WALL_SLOPE: f64 = 0.35;

/// Land smaller than this inside water is a rock or an islet, not a bank of the channel.
const MIN_BANK_AREA_M2: f64 = 25_000.0;

/// About how far a coarse DEM spreads a waterfall below its lip.
const FALL_REACH_M: f64 = 128.0;

/// Level water this long above a falling cell makes it a falls face; a gorge wall's rim is shorter.
const LEVEL_RUN_M: f64 = 30.0;

/// Repair terrain anomalies (LiDAR classification errors, tile seams, provider glitches).
///
/// Uses a 5x5 median-based filter with MAD (median absolute deviation) to detect
/// outliers while preserving real terrain features like mountain ridges and canyons.
/// Runs iteratively so that multi-pixel artifact clusters are eroded from the outside
/// in — each pass fixes boundary pixels that have enough normal neighbors.
///
/// Each pass reads from a row-snapshot of the grid taken at the top of the pass and
/// writes only into the inner cells of `heights`, so the per-row work is independent
/// and parallelised with rayon. On a 16k² grid (the worst case the elevation
/// pipeline allows) this is the dominant elevation post-processing cost.
///
/// `m_per_cell` keeps the erosion reach at a constant physical scale: the window is
/// cell-based, so at tens of metres per cell (a capped grid, or a very low `--scale`)
/// the default 10 passes eat real landforms hundreds of metres across.
pub fn repair_terrain_anomalies(heights: &mut [Vec<f64>], m_per_cell: f64) {
    let grid_h = heights.len();
    if grid_h < 5 {
        return;
    }
    let grid_w = heights[0].len();
    if grid_w < 5 {
        return;
    }

    const RADIUS: i32 = 2; // 5x5 window (24 neighbors)
    const RELATIVE_FACTOR: f64 = 3.0; // deviation must exceed this × MAD

    // At block resolution this is the 6 m / 10 pass behaviour; past a few metres per cell the
    // window covers real landforms, so widen the gate and stop early.
    let abs_threshold = 6.0f64.max(0.25 * m_per_cell);
    let passes = if m_per_cell > 4.0 { 2 } else { 10 };

    let r = RADIUS as usize;
    // Reuse the snapshot buffer across passes (saves ~128 MB/pass of allocs
    // on a 4096² grid). The inner `clone_from` copies in place.
    let mut snapshot: Vec<Vec<f64>> = heights.to_vec();
    let mut total_repaired = 0usize;
    let mut passes_ran = 0usize;

    for pass in 0..passes {
        if pass > 0 {
            // Refresh the snapshot to last pass's writes — also done in
            // parallel because both sides are large contiguous allocs and
            // the row-pair copy is independent.
            snapshot
                .par_iter_mut()
                .zip(heights.par_iter())
                .for_each(|(dst, src)| dst.clone_from(src));
        }

        // Stream writes directly into `heights` per row in parallel, reading
        // from the immutable snapshot. Avoids buffering all changes in a Vec.
        let snapshot_ref: &[Vec<f64>] = &snapshot;
        let repaired: usize = heights
            .par_iter_mut()
            .enumerate()
            .filter(|(y, _)| *y >= r && *y < grid_h - r)
            .map_init(
                || (Vec::with_capacity(24), Vec::with_capacity(24)),
                |(neighbors, abs_devs), (y, row)| {
                    let mut row_repaired = 0usize;
                    for x in r..grid_w - r {
                        let center = snapshot_ref[y][x];
                        if !center.is_finite() {
                            continue;
                        }

                        neighbors.clear();
                        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
                        for dy in -RADIUS..=RADIUS {
                            for dx in -RADIUS..=RADIUS {
                                if dy == 0 && dx == 0 {
                                    continue;
                                }
                                let v = snapshot_ref[(y as i32 + dy) as usize]
                                    [(x as i32 + dx) as usize];
                                if v.is_finite() {
                                    neighbors.push(v);
                                    lo = lo.min(v);
                                    hi = hi.max(v);
                                }
                            }
                        }
                        if neighbors.len() < 8 {
                            continue;
                        }
                        // The median lies within the neighbours' range, so a centre within
                        // the threshold of both ends cannot deviate past it. Most terrain is
                        // that smooth, and this skips both selects there.
                        if center - lo <= abs_threshold && hi - center <= abs_threshold {
                            continue;
                        }

                        let mid = neighbors.len() / 2;
                        neighbors.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
                        let median = neighbors[mid];

                        abs_devs.clear();
                        abs_devs.extend(neighbors.iter().map(|&v| (v - median).abs()));
                        let mad_mid = abs_devs.len() / 2;
                        abs_devs.select_nth_unstable_by(mad_mid, |a, b| a.partial_cmp(b).unwrap());
                        let mad = abs_devs[mad_mid];

                        let deviation = (center - median).abs();
                        if deviation > abs_threshold && deviation > RELATIVE_FACTOR * mad.max(1.0) {
                            row[x] = median;
                            row_repaired += 1;
                        }
                    }
                    row_repaired
                },
            )
            .sum();

        if repaired == 0 {
            break;
        }
        total_repaired += repaired;
        passes_ran = pass + 1;
    }

    if total_repaired > 0 {
        eprintln!(
            "Repaired {} terrain anomalies in {} pass{}",
            total_repaired,
            passes_ran,
            if passes_ran == 1 { "" } else { "es" }
        );
    }
}

/// Apply land-cover-aware repair to the raw elevation grid (in meters).
///
/// This runs after the general MAD cleanup to target artifacts that are
/// too coherent for a small-window outlier filter:
///
/// - **Small water blobs on steep terrain** (ESA shadow on cliff faces) are dropped
///   from the mask first, so nothing below levels terrain around them.
/// - **Water cells** are flattened to the median elevation of their connected
///   component. This kills coastal tile-boundary "rectangular spikes" offshore
///   and ensures oceans/lakes sit at a consistent surface level.
/// - **Built-up cells** are smoothed with a Gaussian blur, blended through a
///   feathered mask so the transition to natural terrain is seamless. This
///   deliberately drops edge detail in urban areas to soften the visually
///   distracting LiDAR classification artifacts (tunnel portals, overpasses,
///   parking decks) while preserving hills at the macro scale.
/// - **Natural terrain** (forests, grassland, bare ground, cropland, snow,
///   wetland, mangroves) is bit-identical to the input — Grand Canyon walls,
///   mountain ridges and coastal cliffs keep full detail.
///
/// `built_up_sigma_cells` is the Gaussian σ in grid cells. Pass `0.0` or a
/// value under the internal minimum to skip built-up smoothing entirely.
///
/// `coastal_pull_distance_cells` is how far (in grid cells) the water-level
/// pull-down reaches into built-up shorelines. This counteracts the DSM
/// building-height bias at the waterfront that a Gaussian alone would turn
/// into a visible "rising ramp" between water and the city interior.
pub fn apply_land_cover_repair(
    heights: &mut [Vec<f64>],
    land_cover: &mut LandCoverData,
    built_up_sigma_cells: f64,
    coastal_pull_distance_cells: u32,
    m_per_cell: f64,
    report: &dyn Fn(f64),
) {
    let grid_h = heights.len();
    if grid_h == 0 {
        return;
    }
    let grid_w = heights[0].len();
    if grid_w == 0 {
        return;
    }
    // Grid dimensions must match - both are built from compute_grid_dims().
    if land_cover.height != grid_h || land_cover.width != grid_w {
        eprintln!(
            "Warning: land cover grid ({}x{}) does not match elevation grid ({}x{}); skipping land-cover-aware repair",
            land_cover.width, land_cover.height, grid_w, grid_h
        );
        return;
    }

    // Shadow blobs on slopes are not water; drop them before anything levels around them.
    let dropped = drop_water_on_steep_terrain(heights, &mut land_cover.grid, m_per_cell);

    // Returns a bool grid marking which cells were actually flattened to the
    // water-surface level. Misclassified wall cells inside narrow canyon
    // rivers are skipped, so downstream passes (pull-down BFS, Gaussian
    // source-masking) use the real water surface and not the contaminated
    // classification.
    let is_water_surface = level_water_surfaces(heights, &land_cover.grid, m_per_cell);

    // Reclassify LC_WATER cells that weren't actually flattened to water
    // surface (ESA-misclassified riverbank walls / piers / shoreline
    // structures kept at their DSM elevation). Without this, the downstream
    // renderer still sees them as water, can't place water above the real
    // water level, and falls through to grass + shoreline sand — producing
    // visible embankments and grid-aligned ridges INSIDE the water body.
    //
    // After reclassification the water_distance grid must be refreshed so
    // `grid_is_water = water_distance > 0` in ground.rs doesn't still treat
    // these cells as water.
    let reclassified = reclassify_non_surface_water_cells(&mut land_cover.grid, &is_water_surface);
    if reclassified + dropped > 0 {
        land_cover.water_distance =
            crate::land_cover::compute_water_distance(&land_cover.grid, grid_w, grid_h);
        // The water-blend smoothing was derived from the pre-reclassify
        // grid — refresh it so the softened shoreline reflects the updated
        // classification.
        land_cover.invalidate_water_blend_grid();
    }

    // Smooth before pull; otherwise the Gaussian raises pulled cells back up.
    // The Gaussian dominates this step's cost, so it drives the reported progress.
    smooth_built_up_gaussian(
        heights,
        &land_cover.grid,
        &is_water_surface,
        built_up_sigma_cells,
        report,
    );
    if coastal_pull_distance_cells > 0 {
        pull_coastal_land_toward_water(heights, &is_water_surface, coastal_pull_distance_cells);
    }
}

/// Flatten the water surface of each connected `LC_WATER` component and
/// return a grid marking which cells were actually treated as water.
///
/// A single water body (ocean, lake, bay, river) should have a uniform
/// surface. But DEM/DSM data contaminates `LC_WATER` components from two
/// opposite directions:
///
/// - **Above water (narrow rivers in canyons):** ESA 10 m pixels at the
///   shoreline get mixed water/wall and snap to "water". Their DSM
///   elevation is 2–30 m *above* the river surface.
/// - **Below water (oceans/fjords with AWS Terrarium / bathymetric blends):**
///   cells over deep water have DSM elevations 5–50 m *below* the surface.
///
/// We handle both by:
///
/// 1. Estimating the water surface via the **histogram mode** (densest 1 m
///    elevation bin). Wall-contaminated components have a peak at the real
///    water surface and a long *upper* tail; bathymetric components have a
///    peak at the real surface and a long *lower* tail. The mode picks the
///    peak regardless of which side the tail is on — robust to both cases
///    unlike a percentile, which implicitly assumes the bias direction.
///
/// 2. Applying an **asymmetric tolerance**: cells at-or-below `surface + 2 m`
///    are flattened to the surface (catches true surface cells *and* all
///    bathymetric cells; Minecraft renders water as a single-block layer
///    so the depth variation we'd otherwise preserve never shows up
///    anyway). Cells more than 2 m above surface are kept at their DSM
///    elevation — they are real walls / piers / embankments and should
///    render as terrain, reclassified away from LC_WATER by the next pass.
///
/// Flowing components get a per-cell local median instead of one level, smoothed at
/// 40 m where that only removes DEM seams and left sharp at real drops.
///
/// Water below or above a waterfall is split off a still body and leveled on its own.
///
/// The returned bool grid marks which cells actually became water surface,
/// so the coastal pull-down and Gaussian source-masking operate on the
/// real water surface rather than the ESA classification.
fn level_water_surfaces(
    heights: &mut [Vec<f64>],
    lc_grid: &[Vec<u8>],
    m_per_cell: f64,
) -> Vec<Vec<bool>> {
    // Histogram bin width for mode estimation. 1 m is tight enough to
    // resolve a distinct water-surface peak vs bathymetric tail.
    const MODE_BIN_SIZE_M: f64 = 1.0;
    // Components smaller than this fall back to the median (mode is unstable
    // with too few samples).
    const MIN_MODE_SAMPLES: usize = 16;
    // A water component whose interquartile elevation range exceeds this
    // threshold is classified as **flowing** water (river with gradient)
    // rather than a still body (lake, fjord, ocean). Flowing components
    // use a per-cell local-median surface so the gradient is preserved
    // instead of collapsing to a single flat Y.
    const FLOWING_IQR_THRESHOLD_M: f64 = 5.0;
    // Radius (in grid cells) for the per-cell local-median surface on
    // flowing water. Big enough to average out LiDAR noise and DSM tile
    // seams, small enough to follow a river's gradient at the scale of
    // a meander or pool. At 1-to-1 grid-to-world mapping this is also
    // the smoothing radius in blocks.
    const LOCAL_SURFACE_RADIUS: i32 = 12;
    // Minimum neighbour water cells required to compute a stable local
    // median for a flowing-component cell. Cells with fewer fall back
    // to the component's own median.
    const MIN_LOCAL_SAMPLES: usize = 8;
    // A local median keeps every edge, and hydro-flattened DEMs are full of them:
    // TIN facets and LiDAR project seams step 1-4 m along straight lines, which render
    // as walls of water across open river. Blur at this width to take those out.
    const FLOW_SMOOTH_SIGMA_M: f64 = 40.0;
    // Below this many cells the blur would not do anything visible.
    const FLOW_SMOOTH_MIN_SIGMA_CELLS: f64 = 1.5;
    // Hard cap on how far the blur may move a cell. The blur sits near the middle of a
    // step, so this removes seams up to twice as tall and only dents a real waterfall.
    const FLOW_SMOOTH_MAX_M: f64 = 1.5;

    let h = heights.len();
    let w = heights[0].len();
    let mut visited = vec![0u64; (w * h).div_ceil(64)];
    let mut is_water_surface = vec![vec![false; w]; h];

    // Leveled in place: a body reads only its own cells and its banks.
    // Cells of the body being leveled, and scratch marks for splitting it.
    let mut in_body = vec![0u64; (w * h).div_ceil(64)];
    let mut marks = vec![0u64; (w * h).div_ceil(64)];
    // Channel walls, kept as terrain; marked first with every cell that falls from its bank.
    let mut walls_mask = vec![0u64; (w * h).div_ceil(64)];
    // Bank distances in chamfer units (three per cell), built once a flowing body needs them.
    let bank_dist: std::cell::OnceCell<Vec<u8>> = std::cell::OnceCell::new();
    let wall_step = slope_step(m_per_cell);
    let fall_reach = ((FALL_REACH_M / m_per_cell).round() as u16).max(1);
    let level_run = (LEVEL_RUN_M / m_per_cell).max(2.0);
    // Local surface of each cell of a flowing body, and the count of channel walls that left it.
    let mut flowing_surfaces = |heights: &[Vec<f64>],
                                cells: &[(usize, usize)],
                                members: &mut [u64],
                                fallback: f64,
                                reach: bool|
     -> (Vec<(u32, u32, f32)>, usize) {
        let bank_dist = bank_dist.get_or_init(|| bank_distances(lc_grid, m_per_cell));
        // A reach's walls that fall away from the body it left are that body's waterfall.
        let from_body = reach.then(|| steps_from_other_water(cells, members, lc_grid, fall_reach));
        let falls: Vec<bool> = {
            let members: &[u64] = members;
            cells
                .par_iter()
                .map(|&(x, y)| {
                    let walk = walk_from_bank(bank_dist, members, w, x, y, wall_step);
                    on_channel_wall(heights, &walk, x, y, m_per_cell)
                        && !from_body
                            .as_ref()
                            .is_some_and(|d| falls_away(d, &walk, w, x, y))
                })
                .collect()
        };
        for (&(x, y), &f) in cells.iter().zip(&falls) {
            if f {
                set_bit(&mut walls_mask, y * w + x);
            }
        }
        // Below level water the ground falls from a lip, not from a bank: a falls face.
        let mut walls: Vec<bool> = {
            let falling: &[u64] = &walls_mask;
            cells
                .par_iter()
                .zip(&falls)
                .map(|(&(x, y), &f)| {
                    f && !under_level_water(heights, lc_grid, bank_dist, falling, x, y, level_run)
                })
                .collect()
        };
        for ((&(x, y), &f), &wall) in cells.iter().zip(&falls).zip(&walls) {
            if f && !wall {
                clear_bit(&mut walls_mask, y * w + x);
            }
        }
        keep_anchored_walls(&mut walls_mask, lc_grid, cells, &mut walls);
        let mut wall_count = 0;
        for (&(x, y), &wall) in cells.iter().zip(&walls) {
            if wall {
                clear_bit(members, y * w + x);
                wall_count += 1;
            }
        }
        let members: &[u64] = members;
        let surfaces = cells
            .par_iter()
            .zip(&walls)
            .filter_map(|(&(cx, cy), &wall)| {
                (!wall && heights[cy][cx].is_finite()).then(|| {
                    let (r, n) = (LOCAL_SURFACE_RADIUS, MIN_LOCAL_SAMPLES);
                    let surface = if reach {
                        local_reach_median(heights, members, lc_grid, cx, cy, r, n)
                    } else {
                        local_water_median(heights, members, cx, cy, r, n)
                    }
                    .unwrap_or(fallback);
                    (cx as u32, cy as u32, surface as f32)
                })
            })
            .collect::<Vec<(u32, u32, f32)>>();
        (surfaces, wall_count)
    };

    let mut components_leveled = 0usize;
    let mut still_components = 0usize;
    let mut flowing_components = 0usize;
    let mut split_reaches = 0usize;
    let mut cells_leveled = 0usize;
    let mut cells_skipped = 0usize;
    let mut max_flowing_iqr = 0.0f64;
    // Flattened after the scan, once smoothed: (x, y, local surface), per component.
    let mut flowing_cells: Vec<Vec<(u32, u32, f32)>> = Vec::new();

    for start_y in 0..h {
        for start_x in 0..w {
            if get_bit(&visited, start_y * w + start_x) || lc_grid[start_y][start_x] != LC_WATER {
                continue;
            }

            // Flood-fill this water component (4-connected).
            let mut component: Vec<(usize, usize)> = Vec::new();
            let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
            queue.push_back((start_x, start_y));
            set_bit(&mut visited, start_y * w + start_x);

            while let Some((x, y)) = queue.pop_front() {
                component.push((x, y));
                for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let nxu = nx as usize;
                    let nyu = ny as usize;
                    if !get_bit(&visited, nyu * w + nxu) && lc_grid[nyu][nxu] == LC_WATER {
                        set_bit(&mut visited, nyu * w + nxu);
                        queue.push_back((nxu, nyu));
                    }
                }
            }

            // Collect finite elevations.
            let mut values: Vec<f64> = component
                .iter()
                .filter_map(|&(x, y)| {
                    let v = heights[y][x];
                    if v.is_finite() {
                        Some(v)
                    } else {
                        None
                    }
                })
                .collect();
            if values.is_empty() {
                continue;
            }

            // IQR-based flowing/still classification. IQR is robust to
            // bathymetric tails (fjords) and outlier pits — it measures the
            // width of the *bulk* of the distribution. A still lake has a
            // tight bulk (near-zero IQR) even with a few noisy cells; a
            // river descending 5+ m over the bbox has a broad bulk because
            // roughly half the cells are at each end of the gradient.
            let iqr = interquartile_range(&values);

            // In place: the mode below does not depend on the order.
            let fallback_median = {
                let mid = values.len() / 2;
                values.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
                values[mid]
            };

            let mut body = component;
            for &(x, y) in &body {
                set_bit(&mut in_body, y * w + x);
            }
            if iqr > FLOWING_IQR_THRESHOLD_M {
                // ── Flowing water (river-like) ─────────────────────────
                // Use a per-cell local median surface so the gradient is
                // preserved. Skip the adjacent-land clamp — that's meant
                // for still water where the whole body must have a single
                // surface level; for a river it would clamp the entire
                // gradient to the low-percentile wall elevation at the
                // downstream end, producing exactly the flat-band-across-
                // the-canyon artifact we're fixing.
                flowing_components += 1;
                if iqr > max_flowing_iqr {
                    max_flowing_iqr = iqr;
                }
                drop(values);
                let (surfaces, walls) =
                    flowing_surfaces(heights, &body, &mut in_body, fallback_median, false);
                flowing_cells.push(surfaces);
                cells_skipped += walls;
            } else {
                // ── Still water (lake / fjord / ocean) ─────────────────
                // Estimate a single surface for the whole component via
                // histogram mode (robust to both upper and lower tails),
                // then clamp by adjacent land p25 so the body can't sit
                // above its own shore (Arnis Baltic fjord case).
                still_components += 1;
                let raw_surface = if values.len() >= MIN_MODE_SAMPLES {
                    histogram_mode(&values, MODE_BIN_SIZE_M)
                } else {
                    fallback_median
                };
                drop(values);
                let mut other = split_off_other_levels(
                    &body,
                    raw_surface,
                    heights,
                    lc_grid,
                    &mut in_body,
                    &mut marks,
                    m_per_cell,
                );
                if !other.reaches.is_empty() || !other.shadow.is_empty() {
                    body.retain(|&(x, y)| get_bit(&in_body, y * w + x));
                }
                // Clamped by its own shore only, not by the banks of what split off.
                let surface =
                    clamp_by_adjacent_land(raw_surface, &body, heights, lc_grid, &mut marks);
                if !other.reaches.is_empty() {
                    grow_reaches(
                        &mut other.reaches,
                        surface,
                        raw_surface,
                        heights,
                        &mut in_body,
                    );
                    body.retain(|&(x, y)| get_bit(&in_body, y * w + x));
                }

                for &(cx, cy) in &body {
                    let orig = heights[cy][cx];
                    if !orig.is_finite() {
                        continue;
                    }
                    let at_or_below = orig <= surface + WATER_UP_TOLERANCE_M;
                    let flatten = at_or_below || !has_non_water_neighbor(lc_grid, cx, cy);
                    if flatten {
                        heights[cy][cx] = surface;
                        is_water_surface[cy][cx] = true;
                        cells_leveled += 1;
                    } else {
                        cells_skipped += 1;
                    }
                }
                // Shadow keeps its terrain, like the walls.
                cells_skipped += other.shadow.len();
                // A reach of a waterfall is part of a river, so it keeps its gradient.
                for Reach { cells: reach, .. } in other.reaches {
                    split_reaches += 1;
                    for &(x, y) in &reach {
                        set_bit(&mut marks, y * w + x);
                    }
                    // Every cell of a reach is finite: it was picked by its height.
                    let reach_median = {
                        let mut v: Vec<f64> = reach.iter().map(|&(x, y)| heights[y][x]).collect();
                        let mid = v.len() / 2;
                        v.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
                        v[mid]
                    };
                    let (surfaces, walls) =
                        flowing_surfaces(heights, &reach, &mut marks, reach_median, true);
                    flowing_cells.push(surfaces);
                    cells_skipped += walls;
                    for &(x, y) in &reach {
                        clear_bit(&mut marks, y * w + x);
                    }
                }
            }
            for &(x, y) in &body {
                clear_bit(&mut in_body, y * w + x);
            }

            components_leveled += 1;
        }
    }

    // Smooth the local-median surface, then flatten as still water does.
    // The scan is done, so the scratch grids go before the blur allocates.
    drop(visited);
    drop(in_body);
    drop(marks);
    drop(bank_dist);
    let sigma_cells = if m_per_cell > 0.0 && m_per_cell.is_finite() {
        (FLOW_SMOOTH_SIGMA_M / m_per_cell).min(64.0)
    } else {
        0.0
    };
    for component in &flowing_cells {
        if component.is_empty() {
            continue;
        }
        // One blur per component, so a canal beside a river is not averaged into it.
        let smoothed = (sigma_cells >= FLOW_SMOOTH_MIN_SIGMA_CELLS)
            .then(|| smooth_sparse_field(component, sigma_cells));
        for (i, &(cx, cy, local_surface)) in component.iter().enumerate() {
            let (cx, cy) = (cx as usize, cy as usize);
            let local_surface = f64::from(local_surface);
            let mut surface = match &smoothed {
                Some(g) if g[i].is_finite() => {
                    local_surface
                        + (g[i] - local_surface).clamp(-FLOW_SMOOTH_MAX_M, FLOW_SMOOTH_MAX_M)
                }
                _ => local_surface,
            };
            // Flowing cells are untouched until here, so heights still holds the input.
            let orig = heights[cy][cx];
            // Never raise water over the land beside it, which is what a one-sided kernel
            // at the ends of the ribbon would otherwise do.
            if let Some(land) = lowest_adjacent_land(heights, lc_grid, cx, cy) {
                surface = surface.min(orig.max(land));
            }
            let at_or_below = orig <= surface + WATER_UP_TOLERANCE_M;
            // A wall is land here, so water it encloses above the surface is no pit to dig.
            let shore =
                has_non_water_neighbor(lc_grid, cx, cy) || next_to(&walls_mask, w, h, cx, cy);
            if at_or_below || !shore {
                heights[cy][cx] = surface;
                is_water_surface[cy][cx] = true;
                cells_leveled += 1;
            } else {
                cells_skipped += 1;
            }
        }
    }
    drop(walls_mask);

    if components_leveled > 0 {
        if flowing_components > 0 {
            eprintln!(
                "Land cover repair: leveled {} water component(s) ({} still, {} flowing, max IQR {:.1}m), {} surface cells flattened, {} off-surface cells kept as terrain",
                components_leveled,
                still_components,
                flowing_components,
                max_flowing_iqr,
                cells_leveled,
                cells_skipped
            );
        } else {
            eprintln!(
                "Land cover repair: leveled {} water component(s), {} surface cells flattened, {} off-surface cells kept as terrain",
                components_leveled, cells_leveled, cells_skipped
            );
        }
        if split_reaches > 0 {
            eprintln!(
                "Land cover repair: leveled {} reach(es) below or above a waterfall at their own level",
                split_reaches
            );
        }
    }

    is_water_surface
}

/// Downsampling step for the coarse field: fine enough for the blur, coarse enough that a
/// component spanning the grid cannot materialize its whole bounding box.
///
/// A river crossing the map has a bounding box the size of the grid while holding only a
/// ribbon of samples. At a coarse metres-per-cell the sigma alone leaves the step at 1, so
/// the budget has to set it instead.
fn coarse_step(bbox_w: usize, bbox_h: usize, sigma_cells: f64) -> usize {
    // Coarse cells per sigma; below this the downsampling shows.
    const COARSE_PER_SIGMA: f64 = 8.0;
    const MAX_COARSE_CELLS: f64 = (4 << 20) as f64;

    let from_sigma = (sigma_cells / COARSE_PER_SIGMA).floor().max(1.0);
    let from_budget = (bbox_w as f64 * bbox_h as f64 / MAX_COARSE_CELLS)
        .sqrt()
        .ceil()
        .max(1.0);
    from_sigma.max(from_budget) as usize
}

/// Blur `cells` (grid x, grid z, value) at `sigma_cells`, one output per input cell.
///
/// The samples are a thin ribbon in a grid up to 16k a side, so the blur runs on their
/// downsampled bounding box and is sampled back. It is a low-pass either way, and the
/// cost follows the ribbon instead of the grid.
fn smooth_sparse_field(cells: &[(u32, u32, f32)], sigma_cells: f64) -> Vec<f64> {
    let (mut x0, mut x1, mut y0, mut y1) = (u32::MAX, 0u32, u32::MAX, 0u32);
    for &(x, y, _) in cells {
        x0 = x0.min(x);
        x1 = x1.max(x);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    let bw = (x1 - x0) as usize + 1;
    let bh = (y1 - y0) as usize + 1;
    let step = coarse_step(bw, bh, sigma_cells);
    let cw = (bw - 1) / step + 1;
    let ch = (bh - 1) / step + 1;
    let mut sum = vec![vec![0.0f64; cw]; ch];
    let mut count = vec![vec![0u32; cw]; ch];
    for &(x, y, v) in cells {
        let cx = (x - x0) as usize / step;
        let cy = (y - y0) as usize / step;
        sum[cy][cx] += f64::from(v);
        count[cy][cx] += 1;
    }
    // Consumed, so the two accumulators are gone before the blur allocates.
    let coarse: Vec<Vec<f64>> = sum
        .into_iter()
        .zip(count)
        .map(|(srow, crow)| {
            srow.into_iter()
                .zip(crow)
                .map(|(s, c)| if c > 0 { s / f64::from(c) } else { f64::NAN })
                .collect()
        })
        .collect();
    let blurred = gaussian_blur_grid(&coarse, sigma_cells / step as f64);

    cells
        .iter()
        .map(|&(x, y, v)| {
            // Bilinear in coarse coordinates, renormalised over finite corners.
            let fx = (x - x0) as f64 / step as f64 - 0.5;
            let fy = (y - y0) as f64 / step as f64 - 0.5;
            let ix = fx.floor();
            let iy = fy.floor();
            let (tx, ty) = (fx - ix, fy - iy);
            let (mut acc, mut wsum) = (0.0, 0.0);
            for (dy, wy) in [(0i64, 1.0 - ty), (1, ty)] {
                for (dx, wx) in [(0i64, 1.0 - tx), (1, tx)] {
                    let sx = ix as i64 + dx;
                    let sy = iy as i64 + dy;
                    if sx < 0 || sy < 0 || sx >= cw as i64 || sy >= ch as i64 {
                        continue;
                    }
                    let val = blurred[sy as usize][sx as usize];
                    if val.is_finite() {
                        acc += val * wx * wy;
                        wsum += wx * wy;
                    }
                }
            }
            if wsum > 0.0 {
                acc / wsum
            } else {
                f64::from(v)
            }
        })
        .collect()
}

/// Compute the interquartile range of a slice of elevations.
/// Uses `select_nth_unstable_by` twice — O(n) total, no full sort.
/// Returns 0.0 for slices with fewer than 4 elements.
fn interquartile_range(values: &[f64]) -> f64 {
    if values.len() < 4 {
        return 0.0;
    }
    let mut v = values.to_vec();
    let q1_idx = v.len() / 4;
    let q3_idx = (v.len() * 3) / 4;
    v.select_nth_unstable_by(q1_idx, |a, b| a.partial_cmp(b).unwrap());
    let q1 = v[q1_idx];
    v.select_nth_unstable_by(q3_idx, |a, b| a.partial_cmp(b).unwrap());
    let q3 = v[q3_idx];
    (q3 - q1).max(0.0)
}

/// Return the median elevation of the body's cells (set bits of `body`) within
/// `radius` of `(cx, cy)`, or `None` if fewer than `min_samples` are finite.
///
/// Used by the flowing-water path in `level_water_surfaces` to build a
/// per-cell water surface that follows the river's gradient at scales
/// longer than the radius, while still averaging out local DSM noise.
fn local_water_median(
    heights: &[Vec<f64>],
    body: &[u64],
    cx: usize,
    cy: usize,
    radius: i32,
    min_samples: usize,
) -> Option<f64> {
    let h = heights.len() as i32;
    if h == 0 {
        return None;
    }
    let w = heights[0].len() as i32;
    let kernel_side = (radius * 2 + 1) as usize;
    let mut samples: Vec<f64> = Vec::with_capacity(kernel_side * kernel_side);
    for dy in -radius..=radius {
        let ny = cy as i32 + dy;
        if ny < 0 || ny >= h {
            continue;
        }
        for dx in -radius..=radius {
            let nx = cx as i32 + dx;
            if nx < 0 || nx >= w {
                continue;
            }
            if !get_bit(body, ny as usize * w as usize + nx as usize) {
                continue;
            }
            let v = heights[ny as usize][nx as usize];
            if v.is_finite() {
                samples.push(v);
            }
        }
    }
    if samples.len() < min_samples {
        return None;
    }
    let mid = samples.len() / 2;
    samples.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
    Some(samples[mid])
}

/// Steps from each cell of a reach (set bits of `members`) to water outside it, up to `cap`.
fn steps_from_other_water(
    reach: &[(usize, usize)],
    members: &[u64],
    lc_grid: &[Vec<u8>],
    cap: u16,
) -> FnvHashMap<u32, u16> {
    let h = lc_grid.len();
    let w = lc_grid[0].len();
    let neighbours = |x: usize, y: usize| {
        [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)]
            .into_iter()
            .filter_map(move |(dx, dy)| {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                (nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32)
                    .then_some((nx as usize, ny as usize))
            })
    };
    let mut steps: FnvHashMap<u32, u16> = FnvHashMap::default();
    let mut frontier: Vec<(usize, usize)> = reach
        .iter()
        .copied()
        .filter(|&(x, y)| {
            neighbours(x, y)
                .any(|(nx, ny)| lc_grid[ny][nx] == LC_WATER && !get_bit(members, ny * w + nx))
        })
        .collect();
    for &(x, y) in &frontier {
        steps.insert((y * w + x) as u32, 0);
    }
    let mut d = 0u16;
    while !frontier.is_empty() && d < cap {
        d += 1;
        let mut next = Vec::new();
        for (x, y) in frontier {
            for (nx, ny) in neighbours(x, y) {
                if !get_bit(members, ny * w + nx) {
                    continue;
                }
                if let std::collections::hash_map::Entry::Vacant(e) =
                    steps.entry((ny * w + nx) as u32)
                {
                    e.insert(d);
                    next.push((nx, ny));
                }
            }
        }
        frontier = next;
    }
    steps
}

/// Whether `walk` leads away from the water the reach left: a fall from it, not a wall.
fn falls_away(steps: &FnvHashMap<u32, u16>, walk: &Walk, w: usize, x: usize, y: usize) -> bool {
    let Some(&start) = steps.get(&((y * w + x) as u32)) else {
        return false;
    };
    let (ex, ey) = walk.end;
    let end = steps
        .get(&((ey * w + ex) as u32))
        .copied()
        .unwrap_or(u16::MAX);
    walk.run > 0.0 && f64::from(end.saturating_sub(start)) >= 0.5 * walk.run
}

/// A walk from a cell away from its nearest bank, with its halfway point. Lengths in cells.
struct Walk {
    end: (usize, usize),
    run: f64,
    mid: (usize, usize),
    mid_run: f64,
}

/// Walk from `(x, y)` away from its nearest bank for at most `step` cells, staying in `body`.
fn walk_from_bank(bank_dist: &[u8], body: &[u64], w: usize, x: usize, y: usize, step: i64) -> Walk {
    let h = bank_dist.len() / w;
    // Where each step ended and the length walked by then; `slope_step` allows at most 16.
    let mut path = [((x, y), 0.0f64); 16];
    let mut taken = 0;
    let (mut cx, mut cy) = (x, y);
    let mut run = 0.0;
    while taken < (step as usize).min(path.len()) {
        let cur = f64::from(bank_dist[cy * w + cx]);
        // Gain per unit length, so diagonal steps do not drift along the channel.
        let mut best: Option<(f64, usize, usize, f64)> = None;
        for (dx, dy) in [
            (1i32, 0i32),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ] {
            let nx = cx as i32 + dx;
            let ny = cy as i32 + dy;
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let (nxu, nyu) = (nx as usize, ny as usize);
            let idx = nyu * w + nxu;
            if !get_bit(body, idx) {
                continue;
            }
            let len = if dx != 0 && dy != 0 {
                std::f64::consts::SQRT_2
            } else {
                1.0
            };
            let gain = (f64::from(bank_dist[idx]) - cur) / len;
            if gain > 0.0 && best.is_none_or(|b| gain > b.0) {
                best = Some((gain, nxu, nyu, len));
            }
        }
        let Some((_, nx, ny, len)) = best else {
            break;
        };
        (cx, cy) = (nx, ny);
        run += len;
        path[taken] = ((cx, cy), run);
        taken += 1;
    }
    // Halfway along the steps taken, so a walk that ends early still has two halves.
    let (mid, mid_run) = if taken >= 2 {
        path[taken / 2 - 1]
    } else {
        ((x, y), 0.0)
    };
    Walk {
        end: (cx, cy),
        run,
        mid,
        mid_run,
    }
}

/// Drop the walls of `cells` no chain of walls ties to land, as a wall hangs from its bank.
fn keep_anchored_walls(
    mask: &mut [u64],
    lc_grid: &[Vec<u8>],
    cells: &[(usize, usize)],
    walls: &mut [bool],
) {
    let h = lc_grid.len();
    let w = lc_grid[0].len();
    let neighbours = |x: usize, y: usize| {
        (-1i32..=1)
            .flat_map(|dy| (-1i32..=1).map(move |dx| (dx, dy)))
            .filter(|&d| d != (0, 0))
            .map(move |(dx, dy)| (x as i32 + dx, y as i32 + dy))
    };
    let mut reached: FnvHashSet<u32> = FnvHashSet::default();
    let mut queue: Vec<(usize, usize)> = Vec::new();
    for (&(x, y), _) in cells.iter().zip(walls.iter()).filter(|(_, &wall)| wall) {
        // The grid edge counts as land, as the bank may lie beyond it.
        let on_land = neighbours(x, y).any(|(nx, ny)| {
            nx < 0
                || ny < 0
                || nx >= w as i32
                || ny >= h as i32
                || lc_grid[ny as usize][nx as usize] != LC_WATER
        });
        if on_land && reached.insert((y * w + x) as u32) {
            queue.push((x, y));
        }
    }
    while let Some((x, y)) = queue.pop() {
        for (nx, ny) in neighbours(x, y) {
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let idx = ny as usize * w + nx as usize;
            if get_bit(mask, idx) && reached.insert(idx as u32) {
                queue.push((nx as usize, ny as usize));
            }
        }
    }
    for (&(x, y), wall) in cells.iter().zip(walls.iter_mut()) {
        if *wall && !reached.contains(&((y * w + x) as u32)) {
            *wall = false;
            clear_bit(mask, y * w + x);
        }
    }
}

/// Whether level water `run` cells long, at or above `(x, y)`, lies on its way to the nearest
/// bank, where a wall rises to the bank. `falling` marks the cells that fall away from their bank.
fn under_level_water(
    heights: &[Vec<f64>],
    lc_grid: &[Vec<u8>],
    bank_dist: &[u8],
    falling: &[u64],
    x: usize,
    y: usize,
    run: f64,
) -> bool {
    let h = heights.len();
    let w = heights[0].len();
    let here = heights[y][x];
    let (mut cx, mut cy) = (x, y);
    let mut level = 0.0;
    // Bank distance falls every step, so the walk ends.
    loop {
        let cur = bank_dist[cy * w + cx];
        let mut best: Option<(f64, usize, usize, f64)> = None;
        for (dx, dy) in [
            (1i32, 0i32),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ] {
            let nx = cx as i32 + dx;
            let ny = cy as i32 + dy;
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            let d = bank_dist[ny * w + nx];
            if d >= cur {
                continue;
            }
            let len = if dx != 0 && dy != 0 {
                std::f64::consts::SQRT_2
            } else {
                1.0
            };
            let drop = f64::from(cur - d) / len;
            if best.is_none_or(|b| drop > b.0) {
                best = Some((drop, nx, ny, len));
            }
        }
        let Some((_, nx, ny, len)) = best else {
            return false;
        };
        let idx = ny * w + nx;
        if bank_dist[idx] == 0 {
            return false;
        }
        let is_level =
            lc_grid[ny][nx] == LC_WATER && !get_bit(falling, idx) && heights[ny][nx] >= here;
        level = if is_level { level + len } else { 0.0 };
        if level >= run {
            return true;
        }
        (cx, cy) = (nx, ny);
    }
}

/// Whether the cell is a gorge wall a coarse DEM smeared into the water: walking away from
/// the bank it keeps falling over both halves of the walk, where a DEM seam falls on one.
fn on_channel_wall(heights: &[Vec<f64>], walk: &Walk, x: usize, y: usize, m_per_cell: f64) -> bool {
    let here = heights[y][x];
    let (there, mid) = (
        heights[walk.end.1][walk.end.0],
        heights[walk.mid.1][walk.mid.0],
    );
    if walk.run == 0.0 || !here.is_finite() || !there.is_finite() || !mid.is_finite() {
        return false;
    }
    let falls = |from: f64, to: f64, length: f64, slope: f64| {
        length <= 0.0 || (from - to) / (length * m_per_cell) > slope
    };
    falls(here, there, walk.run, CHANNEL_WALL_SLOPE)
        && falls(here, mid, walk.mid_run, CHANNEL_WALL_SLOPE / 2.0)
        && falls(
            mid,
            there,
            walk.run - walk.mid_run,
            CHANNEL_WALL_SLOPE / 2.0,
        )
}

/// Distance from every cell to the nearest bank in chamfer units, three to a cell.
fn bank_distances(lc_grid: &[Vec<u8>], m_per_cell: f64) -> Vec<u8> {
    let h = lc_grid.len();
    let w = lc_grid[0].len();
    let mut d: Vec<u8> = lc_grid
        .iter()
        .flat_map(|row| row.iter().map(|&c| if c == LC_WATER { u8::MAX } else { 0 }))
        .collect();
    let min_bank_cells = if m_per_cell > 0.0 && m_per_cell.is_finite() {
        (MIN_BANK_AREA_M2 / (m_per_cell * m_per_cell)) as usize
    } else {
        0
    };
    drop_islets(&mut d, w, h, min_bank_cells);
    crate::water_depth::chamfer_3_4_dt(&mut d, w, h);
    d
}

/// Turn land patches under `min_cells`, clear of the grid edge, into water in a bank-distance grid.
fn drop_islets(d: &mut [u8], w: usize, h: usize, min_cells: usize) {
    if min_cells == 0 {
        return;
    }
    let mut seen = vec![0u64; (w * h).div_ceil(64)];
    // Cells of searches that ran into a patch too big for an islet.
    let mut big = vec![0u64; (w * h).div_ceil(64)];
    let mut patch: Vec<u32> = Vec::new();
    for start in 0..w * h {
        if d[start] != 0 || get_bit(&seen, start) {
            continue;
        }
        // An islet has a shore, so only searches from one can find it.
        let (x, y) = (start % w, start / w);
        let shore = (x > 0 && d[start - 1] != 0)
            || (x + 1 < w && d[start + 1] != 0)
            || (y > 0 && d[start - w] != 0)
            || (y + 1 < h && d[start + w] != 0);
        if !shore {
            continue;
        }
        patch.clear();
        patch.push(start as u32);
        set_bit(&mut seen, start);
        let mut too_big = false;
        let mut head = 0;
        'search: while head < patch.len() {
            let i = patch[head] as usize;
            head += 1;
            let (x, y) = (i % w, i / w);
            if x == 0 || y == 0 || x + 1 == w || y + 1 == h || patch.len() >= min_cells {
                too_big = true;
                break;
            }
            for n in [i - 1, i + 1, i - w, i + w] {
                if d[n] != 0 {
                    continue;
                }
                if get_bit(&big, n) {
                    too_big = true;
                    break 'search;
                }
                if !get_bit(&seen, n) {
                    set_bit(&mut seen, n);
                    patch.push(n as u32);
                }
            }
        }
        if too_big {
            for &i in &patch {
                set_bit(&mut big, i as usize);
            }
        } else {
            for &i in &patch {
                d[i as usize] = u8::MAX;
            }
        }
    }
}

/// `local_water_median` for a reach, its window shrunk clear of other water so no step forms.
fn local_reach_median(
    heights: &[Vec<f64>],
    reach: &[u64],
    lc_grid: &[Vec<u8>],
    cx: usize,
    cy: usize,
    radius: i32,
    min_samples: usize,
) -> Option<f64> {
    let h = heights.len() as i32;
    let w = heights[0].len() as i32;
    let kernel_side = (radius * 2 + 1) as usize;
    let mut samples: Vec<(f64, i32)> = Vec::with_capacity(kernel_side * kernel_side);
    let mut clear = radius;
    for dy in -radius..=radius {
        let ny = cy as i32 + dy;
        if ny < 0 || ny >= h {
            continue;
        }
        for dx in -radius..=radius {
            let nx = cx as i32 + dx;
            if nx < 0 || nx >= w {
                continue;
            }
            let d = dx.abs().max(dy.abs());
            if get_bit(reach, ny as usize * w as usize + nx as usize) {
                let v = heights[ny as usize][nx as usize];
                if v.is_finite() {
                    samples.push((v, d));
                }
            } else if lc_grid[ny as usize][nx as usize] == LC_WATER {
                clear = clear.min(d - 1);
            }
        }
    }
    let own = heights[cy][cx];
    if clear < radius {
        samples.retain(|&(_, d)| d <= clear);
        let side = (clear * 2 + 1) as usize;
        if clear < 1 || samples.len() < min_samples.min(side * side / 2) {
            return own.is_finite().then_some(own);
        }
    } else if samples.len() < min_samples {
        return None;
    }
    let mid = samples.len() / 2;
    samples.select_nth_unstable_by(mid, |a, b| a.0.partial_cmp(&b.0).unwrap());
    Some(samples[mid].0)
}

/// Lowest finite height among the cell's non-water 4-neighbours, if it has any.
fn lowest_adjacent_land(
    heights: &[Vec<f64>],
    lc_grid: &[Vec<u8>],
    x: usize,
    y: usize,
) -> Option<f64> {
    let h = heights.len();
    let w = heights[0].len();
    let mut lowest: Option<f64> = None;
    for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
        let nx = x as i32 + dx;
        let ny = y as i32 + dy;
        if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
            continue;
        }
        let (nxu, nyu) = (nx as usize, ny as usize);
        if lc_grid[nyu][nxu] == LC_WATER {
            continue;
        }
        let v = heights[nyu][nxu];
        if v.is_finite() {
            lowest = Some(lowest.map_or(v, |l: f64| l.min(v)));
        }
    }
    lowest
}

/// Whether cell `(x, y)` has at least one 4-connected neighbor that is not
/// classified as `LC_WATER`. Used to distinguish real shore walls (border
/// cells, keep as terrain) from interior DSM artifacts (surrounded by water,
/// flatten).
fn has_non_water_neighbor(lc_grid: &[Vec<u8>], x: usize, y: usize) -> bool {
    let h = lc_grid.len();
    if h == 0 {
        return false;
    }
    let w = lc_grid[0].len();
    for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
        let nx = x as i32 + dx;
        let ny = y as i32 + dy;
        if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
            // Grid edge is treated as "outside the component" → counts as a
            // non-water neighbor, so a component touching the grid edge can
            // keep its edge cells as wall if they stick above the surface.
            return true;
        }
        if lc_grid[ny as usize][nx as usize] != LC_WATER {
            return true;
        }
    }
    false
}

/// Estimate the mode of a set of elevation values by finding the densest
/// bin of a fixed-width histogram. Robust to both upper tails (walls above
/// water) and lower tails (bathymetric depths below water) — the surface
/// cluster is the dense peak in either case.
fn histogram_mode(values: &[f64], bin_size: f64) -> f64 {
    debug_assert!(!values.is_empty() && bin_size > 0.0);
    let (mut min_v, mut max_v) = (f64::INFINITY, f64::NEG_INFINITY);
    for &v in values {
        if v < min_v {
            min_v = v;
        }
        if v > max_v {
            max_v = v;
        }
    }
    // Degenerate: all equal / near-equal → just return the minimum.
    if max_v - min_v < bin_size {
        return min_v;
    }
    let bin_count = ((max_v - min_v) / bin_size).ceil() as usize + 1;
    let mut hist = vec![0usize; bin_count];
    for &v in values {
        let idx = (((v - min_v) / bin_size) as usize).min(bin_count - 1);
        hist[idx] += 1;
    }
    let peak_idx = hist
        .iter()
        .enumerate()
        .max_by_key(|(_, c)| *c)
        .map(|(i, _)| i)
        .unwrap_or(0);
    min_v + (peak_idx as f64 + 0.5) * bin_size
}

/// Clamp a proposed water surface level so the body doesn't sit above the
/// land around it.
///
/// A mode / median over water-cell elevations alone can come out above the
/// adjacent terrain when the DSM has a systematic upward bias on the water
/// (observed with AWS Terrarium mixing bathymetric and coastal averages in
/// Baltic fjords). Flattening every water cell to that biased value then
/// produces a visible water-on-plateau with a cliff down to the real shore.
///
/// We fix it by measuring the 25th percentile of the elevations of every
/// *non-water* cell that touches the component (4-connected boundary, one
/// sample per adjacent cell, deduplicated in the cleared scratch bitset `seen`)
/// and taking the lower of that and the proposed surface.
///
/// - 25th percentile instead of **min**: robust to one DSM-artifact pit in
///   the shoreline dragging the whole body down.
/// - 25th percentile instead of **median**: honest respect for any real low
///   land around the body (tidal flats, coastal meadows).
///
/// If the component has no adjacent non-water cells (bbox entirely inside
/// one water body), there's nothing to clamp against — fall back to the
/// mode estimate.
fn clamp_by_adjacent_land(
    proposed: f64,
    component: &[(usize, usize)],
    heights: &[Vec<f64>],
    lc_grid: &[Vec<u8>],
    seen: &mut [u64],
) -> f64 {
    let h = heights.len();
    if h == 0 {
        return proposed;
    }
    let w = heights[0].len();

    let mut adjacent: Vec<u32> = Vec::new();
    let mut adjacent_land: Vec<f64> = Vec::new();
    for &(x, y) in component {
        for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let nxu = nx as usize;
            let nyu = ny as usize;
            if lc_grid[nyu][nxu] == LC_WATER {
                continue;
            }
            let idx = nyu * w + nxu;
            if get_bit(seen, idx) {
                continue;
            }
            set_bit(seen, idx);
            adjacent.push(idx as u32);
            let v = heights[nyu][nxu];
            if v.is_finite() {
                adjacent_land.push(v);
            }
        }
    }
    for &idx in &adjacent {
        clear_bit(seen, idx as usize);
    }

    if adjacent_land.is_empty() {
        return proposed;
    }

    let p25_idx = (adjacent_land.len() / 4).min(adjacent_land.len() - 1);
    adjacent_land.select_nth_unstable_by(p25_idx, |a, b| a.partial_cmp(b).unwrap());
    let land_p25 = adjacent_land[p25_idx];

    proposed.min(land_p25)
}

/// Water of a still body that lies at another level than the body.
struct OtherLevels {
    /// Reaches below or above a waterfall, leveled as flowing water.
    reaches: Vec<Reach>,
    /// Steep patches above the level: shadow, kept as terrain.
    shadow: Vec<(usize, usize)>,
}

struct Reach {
    below: bool,
    cells: Vec<(usize, usize)>,
}

/// Take water at other levels out of a still body (`in_body`): below it where the body would
/// spill over the banks, above it where its own banks hold it.
fn split_off_other_levels(
    body: &[(usize, usize)],
    level: f64,
    heights: &[Vec<f64>],
    lc_grid: &[Vec<u8>],
    in_body: &mut [u64],
    marks: &mut [u64],
    m_per_cell: f64,
) -> OtherLevels {
    // Past this a cell is not surface noise or a hydro-flattening step of the body.
    const LEVEL_SPLIT_M: f64 = 5.0;
    // Banks this far below the level would let the body spill over them.
    const SPILL_BANK_M: f64 = 2.5;
    // Fewest spilling bank edges that can split water off.
    const MIN_SPILL_EDGES: usize = 4;

    let h = heights.len();
    let w = heights[0].len();
    let side = |v: f64| -> i8 {
        if v < level - LEVEL_SPLIT_M {
            -1
        } else if v > level + LEVEL_SPLIT_M {
            1
        } else {
            0
        }
    };
    let max_shadow_cells = if m_per_cell > 0.0 && m_per_cell.is_finite() {
        (MAX_STEEP_WATER_AREA_M2 / (m_per_cell * m_per_cell)) as usize
    } else {
        0
    };
    let step = slope_step(m_per_cell);

    let mut out = OtherLevels {
        reaches: Vec::new(),
        shadow: Vec::new(),
    };
    let mut part: Vec<(usize, usize)> = Vec::new();
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
    for &(sx, sy) in body {
        let s = side(heights[sy][sx]);
        if s == 0 || get_bit(marks, sy * w + sx) {
            continue;
        }
        set_bit(marks, sy * w + sx);
        part.clear();
        queue.push_back((sx, sy));
        // Edges from the patch to land, to land under the spill line, and to other water.
        let (mut land, mut spill, mut water) = (0usize, 0usize, 0usize);
        while let Some((x, y)) = queue.pop_front() {
            part.push((x, y));
            for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                    continue;
                }
                let (nxu, nyu) = (nx as usize, ny as usize);
                let v = heights[nyu][nxu];
                if lc_grid[nyu][nxu] != LC_WATER {
                    land += 1;
                    if v < level - SPILL_BANK_M {
                        spill += 1;
                    }
                    continue;
                }
                let idx = nyu * w + nxu;
                if get_bit(in_body, idx) && side(v) == s {
                    if !get_bit(marks, idx) {
                        set_bit(marks, idx);
                        queue.push_back((nxu, nyu));
                    }
                    continue;
                }
                water += 1;
            }
        }
        if s < 0 {
            if spill >= MIN_SPILL_EDGES && 2 * spill >= land && 2 * land >= water {
                out.reaches.push(Reach {
                    below: true,
                    cells: std::mem::take(&mut part),
                });
            }
            continue;
        }
        if land == 0 || land < 2 * water {
            continue;
        }
        // Shadow on a slope, which `drop_water_on_steep_terrain` misses inside a body.
        let steep = part.len() <= max_shadow_cells && {
            let mut slopes: Vec<f64> = part
                .iter()
                .filter_map(|&(x, y)| {
                    water_surface_slope(heights, x, y, step, m_per_cell, |nx, ny| {
                        get_bit(in_body, ny * w + nx)
                    })
                })
                .collect();
            !slopes.is_empty() && {
                let mid = slopes.len() / 2;
                slopes.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
                slopes[mid] > MIN_STEEP_WATER_SLOPE
            }
        };
        if steep {
            out.shadow.extend_from_slice(&part);
        } else {
            out.reaches.push(Reach {
                below: false,
                cells: std::mem::take(&mut part),
            });
        }
    }
    for &(x, y) in body {
        clear_bit(marks, y * w + x);
    }

    for &(x, y) in out.reaches.iter().flat_map(|r| &r.cells).chain(&out.shadow) {
        clear_bit(in_body, y * w + x);
    }
    out
}

/// Hand each reach the body cells beside it past the flatten tolerance of `surface`, so no
/// taller step is left. Cells at the body's own `level` stay, as the clamp can sit far below.
fn grow_reaches(
    reaches: &mut [Reach],
    surface: f64,
    level: f64,
    heights: &[Vec<f64>],
    in_body: &mut [u64],
) {
    let h = heights.len();
    let w = heights[0].len();
    let (below_from, above_from) = (
        surface - WATER_UP_TOLERANCE_M,
        surface.max(level - 1.0) + WATER_UP_TOLERANCE_M,
    );
    for reach in reaches {
        let mut i = 0;
        while i < reach.cells.len() {
            let (x, y) = reach.cells[i];
            i += 1;
            for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                    continue;
                }
                let (nxu, nyu) = (nx as usize, ny as usize);
                let idx = nyu * w + nxu;
                let v = heights[nyu][nxu];
                let past = if reach.below {
                    v < below_from
                } else {
                    v > above_from
                };
                if past && get_bit(in_body, idx) {
                    clear_bit(in_body, idx);
                    reach.cells.push((nxu, nyu));
                }
            }
        }
    }
}

/// Whether a 4-neighbour of `(x, y)` is set in `mask` over a `w` x `h` grid.
fn next_to(mask: &[u64], w: usize, h: usize, x: usize, y: usize) -> bool {
    (x > 0 && get_bit(mask, y * w + x - 1))
        || (x + 1 < w && get_bit(mask, y * w + x + 1))
        || (y > 0 && get_bit(mask, (y - 1) * w + x))
        || (y + 1 < h && get_bit(mask, (y + 1) * w + x))
}

#[inline(always)]
fn get_bit(mask: &[u64], idx: usize) -> bool {
    (mask[idx >> 6] >> (idx & 63)) & 1 != 0
}

#[inline(always)]
fn set_bit(mask: &mut [u64], idx: usize) {
    mask[idx >> 6] |= 1u64 << (idx & 63);
}

#[inline(always)]
fn clear_bit(mask: &mut [u64], idx: usize) {
    mask[idx >> 6] &= !(1u64 << (idx & 63));
}

/// Class of the nearest non-water, non-nodata cell within `radius`, if any.
fn nearest_non_water_class(lc_grid: &[Vec<u8>], x: usize, y: usize, radius: i32) -> Option<u8> {
    let h = lc_grid.len() as i32;
    let w = lc_grid.first().map_or(0, Vec::len) as i32;
    for r in 1..=radius {
        for dy in -r..=r {
            for dx in -r..=r {
                // Only sample cells on the ring at distance exactly `r`.
                if dy.abs() != r && dx.abs() != r {
                    continue;
                }
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 || nx >= w || ny >= h {
                    continue;
                }
                let c = lc_grid[ny as usize][nx as usize];
                if c != LC_WATER && c != 0 {
                    return Some(c);
                }
            }
        }
    }
    None
}

/// About one ESA pixel, so DEM noise finer than the land cover cannot pass for slope.
fn slope_step(m_per_cell: f64) -> i64 {
    ((10.0 / m_per_cell).round() as usize).clamp(1, 16) as i64
}

/// Slope of the water surface at `(x, y)` from the farthest water within `step` each way, so
/// canyon walls are not read and patches smaller than the step still get measured.
fn water_surface_slope(
    heights: &[Vec<f64>],
    x: usize,
    y: usize,
    step: i64,
    m_per_cell: f64,
    is_water: impl Fn(usize, usize) -> bool,
) -> Option<f64> {
    let h = heights.len();
    let w = heights[0].len();
    let here = heights[y][x];
    if !here.is_finite() {
        return None;
    }
    let (x, y) = (x as i64, y as i64);
    let at = |ux: i64, uy: i64| -> Option<(f64, f64)> {
        for d in (1..=step).rev() {
            let (nx, ny) = (x + ux * d, y + uy * d);
            if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                continue;
            }
            if !is_water(nx as usize, ny as usize) {
                continue;
            }
            let v = heights[ny as usize][nx as usize];
            if v.is_finite() {
                return Some((v, d as f64 * m_per_cell));
            }
        }
        None
    };
    let axis = |lo: Option<(f64, f64)>, hi: Option<(f64, f64)>| match (lo, hi) {
        (Some((a, da)), Some((b, db))) => Some((b - a) / (da + db)),
        (Some((a, da)), None) => Some((here - a) / da),
        (None, Some((b, db))) => Some((b - here) / db),
        (None, None) => None,
    };
    let gx = axis(at(-1, 0), at(1, 0));
    let gz = axis(at(0, -1), at(0, 1));
    if gx.is_none() && gz.is_none() {
        return None;
    }
    let (gx, gz) = (gx.unwrap_or(0.0), gz.unwrap_or(0.0));
    Some((gx * gx + gz * gz).sqrt())
}

/// Drop small `LC_WATER` components that sit on steep terrain.
///
/// ESA mistakes deeply shadowed slopes for water (canyon walls, alpine north faces).
/// Left alone these get leveled into a flat ledge with the terrain pulled down around
/// them, a pond hanging on a cliff. A component is dropped when it is small, the surface
/// it claims is itself steep, and most of it is perched with its own land below it. A
/// real watercourse fails both: its surface follows a gentle grade and its banks rise.
/// Dropped cells take the nearest land class. Returns cells reclassified.
fn drop_water_on_steep_terrain(
    heights: &[Vec<f64>],
    lc_grid: &mut [Vec<u8>],
    m_per_cell: f64,
) -> usize {
    const SEARCH_RADIUS: i32 = 8;
    const FALLBACK_CLASS: u8 = crate::land_cover::LC_BARE;
    let h = heights.len();
    if h == 0 || m_per_cell <= 0.0 || !m_per_cell.is_finite() {
        return 0;
    }
    let w = heights[0].len();
    if w == 0 || lc_grid.len() != h || lc_grid[0].len() != w {
        return 0;
    }
    let step = slope_step(m_per_cell);
    let max_cells = (MAX_STEEP_WATER_AREA_M2 / (m_per_cell * m_per_cell)) as usize;
    let cell_slope = |x: usize, y: usize| {
        water_surface_slope(heights, x, y, step, m_per_cell, |nx, ny| {
            lc_grid[ny][nx] == LC_WATER
        })
    };
    let median = |v: &mut Vec<f64>| -> f64 {
        let mid = v.len() / 2;
        v.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
        v[mid]
    };

    let mut visited = vec![0u64; (w * h).div_ceil(64)];
    // Reused per component, always cleared again after the basin test below.
    let mut in_component = vec![0u64; (w * h).div_ceil(64)];
    let mut dropped_cells: Vec<(usize, usize)> = Vec::new();
    let mut dropped_components = 0usize;
    let mut component: Vec<(u32, u32)> = Vec::new();
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
    for start_y in 0..h {
        for start_x in 0..w {
            if get_bit(&visited, start_y * w + start_x) || lc_grid[start_y][start_x] != LC_WATER {
                continue;
            }
            component.clear();
            let mut size = 0usize;
            queue.push_back((start_x, start_y));
            set_bit(&mut visited, start_y * w + start_x);
            while let Some((x, y)) = queue.pop_front() {
                size += 1;
                // Oversize components are rejected below, so stop storing their cells.
                if size <= max_cells + 1 {
                    component.push((x as u32, y as u32));
                }
                for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let (nxu, nyu) = (nx as usize, ny as usize);
                    if !get_bit(&visited, nyu * w + nxu) && lc_grid[nyu][nxu] == LC_WATER {
                        set_bit(&mut visited, nyu * w + nxu);
                        queue.push_back((nxu, nyu));
                    }
                }
            }
            if size > max_cells {
                continue;
            }
            let mut slopes: Vec<f64> = component
                .iter()
                .filter_map(|&(x, y)| cell_slope(x as usize, y as usize))
                .collect();
            if slopes.is_empty() || median(&mut slopes) <= MIN_STEEP_WATER_SLOPE {
                continue;
            }
            // Water sits in a basin, so nothing near it is lower. A blob on a slope has the
            // hillside continuing below it. Measured per cell against ground a step away,
            // because comparing whole components confuses a slope with a gradient: a
            // stream's source is far above its mouth while its banks still rise beside it.
            // Only this component's own cells are excluded, so a blob perched over a river
            // reads that river as the ground below it while a stream only finds itself.
            for &(x, y) in &component {
                set_bit(&mut in_component, y as usize * w + x as usize);
            }
            let (mut edge_cells, mut perched) = (0usize, 0usize);
            for &(x, y) in &component {
                let here = heights[y as usize][x as usize];
                if !here.is_finite() {
                    continue;
                }
                let (x, y) = (x as i64, y as i64);
                let mut lowest = f64::INFINITY;
                for (dx, dy) in [(step, 0), (-step, 0), (0, step), (0, -step)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    if get_bit(&in_component, ny as usize * w + nx as usize) {
                        continue;
                    }
                    let v = heights[ny as usize][nx as usize];
                    if v.is_finite() {
                        lowest = lowest.min(v);
                    }
                }
                if !lowest.is_finite() {
                    continue;
                }
                edge_cells += 1;
                if lowest < here - STEEP_WATER_LAND_BELOW_M {
                    perched += 1;
                }
            }
            for &(x, y) in &component {
                clear_bit(&mut in_component, y as usize * w + x as usize);
            }
            if edge_cells == 0 || (perched as f64) < MIN_PERCHED_FRACTION * edge_cells as f64 {
                continue;
            }

            dropped_components += 1;
            dropped_cells.extend(component.iter().map(|&(x, y)| (x as usize, y as usize)));
        }
    }

    // Reclassify after the scan so one blob's replacement never feeds
    // another's neighbour search.
    let replacements: Vec<(usize, usize, u8)> = dropped_cells
        .iter()
        .map(|&(x, y)| {
            let c = nearest_non_water_class(lc_grid, x, y, SEARCH_RADIUS).unwrap_or(FALLBACK_CLASS);
            (x, y, c)
        })
        .collect();
    for (x, y, c) in &replacements {
        lc_grid[*y][*x] = *c;
    }
    if dropped_components > 0 {
        eprintln!(
            "Land cover repair: dropped {} water blob(s) ({} cells) sitting on steep terrain (ESA shadow misclassification)",
            dropped_components,
            replacements.len()
        );
    }
    replacements.len()
}

/// Reclassify `LC_WATER` cells that `level_water_surfaces` left at their
/// original DSM elevation (because they were more than ±2 m off the
/// component water-surface estimate — ESA shoreline misclassification of
/// riverbank walls, piers, bridge footings, embankments, etc.).
///
/// Without this the downstream renderer sees them as water, can't place
/// water above the real water level at their elevation, and falls through
/// to the `LC_WATER` match-default which is `GRASS_BLOCK`. The shoreline
/// blender then adds sand around them. Visible result: thin linear grass
/// + sand ridges cutting across a water body at a ~3 m elevation step.
///
/// Each misclassified cell adopts its nearest non-water neighbor's class
/// so rendering is continuous with the surrounding terrain. If no
/// non-water neighbor exists within the search radius (rare: an island of
/// misclassified water completely surrounded by real water), falls back
/// to `LC_BARE` which renders as a natural stone/gravel mix.
///
/// Returns the number of cells reclassified.
fn reclassify_non_surface_water_cells(
    lc_grid: &mut [Vec<u8>],
    is_water_surface: &[Vec<bool>],
) -> usize {
    const SEARCH_RADIUS: i32 = 8;
    const FALLBACK_CLASS: u8 = crate::land_cover::LC_BARE;

    let h = lc_grid.len();
    if h == 0 {
        return 0;
    }
    let w = lc_grid[0].len();
    if w == 0 {
        return 0;
    }

    // Two-pass: compute replacements from the ORIGINAL grid first, then
    // apply them. Otherwise earlier mutations influence later lookups and
    // the classification ripples unpredictably.
    let mut replacements: Vec<(usize, usize, u8)> = Vec::new();

    for y in 0..h {
        for x in 0..w {
            if lc_grid[y][x] != LC_WATER || is_water_surface[y][x] {
                continue;
            }

            let found = nearest_non_water_class(lc_grid, x, y, SEARCH_RADIUS);
            replacements.push((x, y, found.unwrap_or(FALLBACK_CLASS)));
        }
    }

    let n = replacements.len();
    for (x, y, c) in replacements {
        lc_grid[y][x] = c;
    }

    if n > 0 {
        eprintln!(
            "Land cover repair: reclassified {} LC_WATER cells not on the water surface (embankments / piers / shoreline walls)",
            n
        );
    }
    n
}

// Linearly pull land cells within max_distance toward the local water surface; skip > MAX_PULL_DROP_M above water (real cliffs).
fn pull_coastal_land_toward_water(
    heights: &mut [Vec<f64>],
    is_water_surface: &[Vec<bool>],
    max_distance: u32,
) {
    if max_distance == 0 {
        return;
    }
    let h = heights.len();
    let w = heights[0].len();

    // Cells above this threshold are treated as real cliffs and not pulled.
    const MAX_PULL_DROP_M: f64 = 15.0;

    // Multi-source BFS: seed with confirmed water-surface cells (not just
    // LC_WATER, so a canyon-wall cell misclassified as water doesn't
    // propagate its wall elevation as the pull-down target), propagate
    // (distance, water_level) outward to at most `max_distance` steps.
    let mut dist = vec![vec![u32::MAX; w]; h];
    let mut water_level = vec![vec![f64::NAN; w]; h];
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new();

    for y in 0..h {
        for x in 0..w {
            if is_water_surface[y][x] {
                dist[y][x] = 0;
                water_level[y][x] = heights[y][x];
                queue.push_back((x, y));
            }
        }
    }

    while let Some((x, y)) = queue.pop_front() {
        let d = dist[y][x];
        if d >= max_distance {
            continue;
        }
        let wl = water_level[y][x];
        for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let nxu = nx as usize;
            let nyu = ny as usize;
            if d + 1 < dist[nyu][nxu] {
                dist[nyu][nxu] = d + 1;
                water_level[nyu][nxu] = wl;
                queue.push_back((nxu, nyu));
            }
        }
    }

    let mut affected = 0usize;
    let mut skipped_cliff = 0usize;
    let denom = max_distance as f64;
    for y in 0..h {
        for x in 0..w {
            let d = dist[y][x];
            if d == 0 || d > max_distance {
                continue;
            }
            let wl = water_level[y][x];
            let orig = heights[y][x];
            if !wl.is_finite() || !orig.is_finite() {
                continue;
            }
            if orig - wl > MAX_PULL_DROP_M {
                skipped_cliff += 1;
                continue;
            }
            let weight = ((max_distance - d) as f64 / denom).clamp(0.0, 1.0);
            heights[y][x] = orig * (1.0 - weight) + wl * weight;
            affected += 1;
        }
    }

    if affected > 0 || skipped_cliff > 0 {
        eprintln!(
            "Land cover repair: pulled {} coastal land cells toward water (within {} cells); kept {} cells above {} m as real cliffs",
            affected, max_distance, skipped_cliff, MAX_PULL_DROP_M
        );
    }
}

/// Gaussian-blur the heights and blend back through a feathered built-up mask.
///
/// Sharp LiDAR classification artifacts in urban areas (tunnel portals,
/// overpasses, parking decks) don't translate cleanly to Minecraft block
/// resolution — we'd rather lose the detail and get smooth ground than
/// render a visually jarring spike. Median filters preserve edges, which is
/// not what we want for cities. A Gaussian blur drops the high-frequency
/// noise and preserves the macro shape (city on a hill still has the hill).
///
/// To avoid a visible seam at the boundary between built-up and natural
/// terrain, the binary classification mask is itself blurred with the same
/// kernel, yielding a soft 0–1 weight that we lerp with:
///
///     out[y][x] = (1 − mask[y][x]) · original[y][x] + mask[y][x] · blurred[y][x]
///
/// A very small sigma (< 1.5 cells) produces no visible smoothing, so we
/// skip the whole pass in that case (e.g. on coarse AWS fallback where the
/// native resolution already exceeds our target smoothing scale).
fn smooth_built_up_gaussian(
    heights: &mut [Vec<f64>],
    lc_grid: &[Vec<u8>],
    is_water_surface: &[Vec<bool>],
    sigma_cells: f64,
    report: &dyn Fn(f64),
) {
    const MIN_SIGMA: f64 = 1.5;
    if sigma_cells < MIN_SIGMA {
        return;
    }

    let h = heights.len();
    let w = heights[0].len();

    // Early out: if there are no built-up cells, nothing to do.
    let built_up_count: usize = lc_grid
        .iter()
        .flat_map(|row| row.iter())
        .filter(|&&c| c == LC_BUILT_UP)
        .count();
    if built_up_count == 0 {
        return;
    }

    // Blur the binary built-up mask -> feathered weights with a smooth 0..1 falloff
    // across the built-up boundary. Without this we'd get a visible seam. The mask is
    // read straight out of lc_grid and kept as f32: it is only ever a lerp weight.
    // Mask blur is the first half of this step's progress, heights blur the second.
    let feathered_mask =
        gaussian_blur_mask_to_f32_reported(lc_grid, LC_BUILT_UP, w, h, sigma_cells, &|f| {
            report(0.5 * f)
        });

    // Blur the heights with *water-surface* cells read as NaN so they don't
    // contribute. Without this the blur averages water (low) into nearby built-up
    // cells and produces a visible "rising ramp" from water into the city, the
    // coastal artifact we already fix with the explicit pull-down pass. Using
    // is_water_surface (not LC_WATER) means canyon wall cells misclassified as
    // water still contribute like the terrain they actually are.
    let blurred_heights =
        gaussian_blur_heights_masked(heights, is_water_surface, sigma_cells, &|f| {
            report(0.5 + 0.5 * f)
        });

    // Blend through the feathered mask. Water-surface cells are skipped so
    // the leveled water surface from the previous pass survives intact.
    let mut total_influenced = 0usize;
    for y in 0..h {
        for x in 0..w {
            if is_water_surface[y][x] {
                continue;
            }
            let m = (feathered_mask[y][x] as f64).clamp(0.0, 1.0);
            if m <= 1.0e-4 {
                continue;
            }
            let orig = heights[y][x];
            let blur = blurred_heights[y][x];
            if !orig.is_finite() || !blur.is_finite() {
                continue;
            }
            heights[y][x] = (1.0 - m) * orig + m * blur;
            total_influenced += 1;
        }
    }

    eprintln!(
        "Land cover repair: built-up Gaussian smoothing σ={:.2} cells applied to {} built-up + feathered cells ({} core built-up cells)",
        sigma_cells, total_influenced, built_up_count
    );
}

/// 2D Gaussian blur (separable: horizontal then vertical pass).
/// Edges are handled by renormalizing weights over the valid samples so the
/// blur doesn't darken the border of the grid.
pub(crate) fn gaussian_blur_grid(grid: &[Vec<f64>], sigma: f64) -> Vec<Vec<f64>> {
    gaussian_blur_grid_reported(grid, sigma, &|_| {})
}

/// Same blur and output as `gaussian_blur_grid`, but calls `report(fraction)`
/// (0.0..1.0) a handful of times from the calling thread as it works. On a
/// city-sized grid each pass takes seconds, so this lets a progress bar advance
/// instead of freezing. Rows/columns are processed in chunks purely so progress
/// can be reported between them — both axes stay fully independent, so the
/// result is identical to processing them all at once.
fn gaussian_blur_grid_reported(
    grid: &[Vec<f64>],
    sigma: f64,
    report: &dyn Fn(f64),
) -> Vec<Vec<f64>> {
    let kernel_size: usize = (sigma * 3.0).ceil() as usize * 2 + 1;
    let kernel = create_gaussian_kernel(kernel_size, sigma);
    let half = kernel_size as i32 / 2;

    let h = grid.len();
    if h == 0 {
        return Vec::new();
    }
    let w = grid[0].len();
    if w == 0 {
        return vec![Vec::new(); h];
    }

    let row_chunk = h.div_ceil(BLUR_CHUNKS);
    let mut after_h: Vec<Vec<f64>> = Vec::with_capacity(h);
    for rows in grid.chunks(row_chunk) {
        let mut part: Vec<Vec<f64>> = rows
            .par_iter()
            .map(|row| blur_line(row.len(), &kernel, half, |i| row[i]))
            .collect();
        after_h.append(&mut part);
        report(0.5 * (after_h.len() as f64 / h as f64));
    }

    gaussian_blur_vertical_in_place(&mut after_h, &kernel, half, w, report);
    after_h
}

/// `gaussian_blur_grid_reported` over `heights` with every `masked` cell read as NaN,
/// so it contributes nothing. Identical output to blurring a pre-built masked copy,
/// without allocating one.
fn gaussian_blur_heights_masked(
    heights: &[Vec<f64>],
    masked: &[Vec<bool>],
    sigma: f64,
    report: &dyn Fn(f64),
) -> Vec<Vec<f64>> {
    let kernel_size: usize = (sigma * 3.0).ceil() as usize * 2 + 1;
    let kernel = create_gaussian_kernel(kernel_size, sigma);
    let half = kernel_size as i32 / 2;

    let h = heights.len().min(masked.len());
    if h == 0 {
        return Vec::new();
    }
    let w = heights[0].len().min(masked[0].len());
    if w == 0 {
        return vec![Vec::new(); h];
    }

    let row_chunk = h.div_ceil(BLUR_CHUNKS);
    let mut after_h: Vec<Vec<f64>> = Vec::with_capacity(h);
    let mut y0 = 0usize;
    while y0 < h {
        let y1 = (y0 + row_chunk).min(h);
        let mut part: Vec<Vec<f64>> = (y0..y1)
            .into_par_iter()
            .map(|y| {
                let (row, m_row) = (&heights[y], &masked[y]);
                let len = row.len().min(m_row.len());
                blur_line(
                    len,
                    &kernel,
                    half,
                    |i| if m_row[i] { f64::NAN } else { row[i] },
                )
            })
            .collect();
        after_h.append(&mut part);
        y0 = y1;
        report(0.5 * (after_h.len() as f64 / h as f64));
    }

    gaussian_blur_vertical_in_place(&mut after_h, &kernel, half, w, report);
    after_h
}

/// ~10 chunks per pass: enough to animate the bar, few enough that the extra
/// rayon barriers cost nothing measurable.
const BLUR_CHUNKS: usize = 10;

/// One separable pass over a line of `len` samples. `get` supplies the source value so
/// a caller can synthesise masked-out cells without materialising a copy of the grid.
/// Edges are handled by renormalizing over the valid samples.
#[inline]
fn blur_line(len: usize, kernel: &[f64], half: i32, get: impl Fn(usize) -> f64) -> Vec<f64> {
    let line_len = len as i32;
    let vals: Vec<f64> = (0..len).map(get).collect();
    let bad = prefix_counts(&vals, |v| !v.is_finite());
    // Same additions in the same order as the edge path's `wsum`, so interior output is bit-identical.
    let full_wsum = kernel.iter().fold(0.0, |acc, &k| acc + k);
    let h = half as usize;
    (0..len)
        .map(|i| {
            if i >= h && i + h < len && bad[i + h + 1] == bad[i - h] {
                let mut sum = 0.0;
                for (&k, &v) in kernel.iter().zip(&vals[i - h..=i + h]) {
                    sum += v * k;
                }
                return sum / full_wsum;
            }
            let mut sum = 0.0;
            let mut wsum = 0.0;
            for (j, &k) in kernel.iter().enumerate() {
                let idx = i as i32 + j as i32 - half;
                if idx >= 0 && idx < line_len {
                    let v = vals[idx as usize];
                    if v.is_finite() {
                        sum += v * k;
                        wsum += k;
                    }
                }
            }
            if wsum > 0.0 {
                sum / wsum
            } else {
                f64::NAN
            }
        })
        .collect()
}

/// Vertical half of the separable blur, written back over the horizontal result.
/// Every column is copied out before it is computed and only reads its own column, and
/// a chunk's writes land after all of its columns have been read, so nothing reads a
/// cell that was already overwritten and no second full-grid buffer is needed.
fn gaussian_blur_vertical_in_place(
    after_h: &mut [Vec<f64>],
    kernel: &[f64],
    half: i32,
    w: usize,
    report: &dyn Fn(f64),
) {
    let col_chunk = w.div_ceil(BLUR_CHUNKS);
    let mut x0 = 0usize;
    while x0 < w {
        let x1 = (x0 + col_chunk).min(w);
        let blurred: Vec<(usize, Vec<f64>)> = {
            let src: &[Vec<f64>] = after_h;
            // Columns are gathered eight at a time: one cache line per row instead of one per cell.
            const GROUP: usize = 8;
            (x0..x1)
                .step_by(GROUP)
                .collect::<Vec<_>>()
                .into_par_iter()
                .flat_map_iter(|gx| {
                    let gw = GROUP.min(x1 - gx);
                    let mut cols: Vec<Vec<f64>> = vec![Vec::with_capacity(src.len()); gw];
                    for row in src {
                        for (c, col) in cols.iter_mut().enumerate() {
                            col.push(row[gx + c]);
                        }
                    }
                    cols.into_iter().enumerate().map(move |(c, column)| {
                        (gx + c, blur_line(column.len(), kernel, half, |i| column[i]))
                    })
                })
                .collect()
        };
        for (x, col) in blurred {
            for (y, v) in col.into_iter().enumerate() {
                after_h[y][x] = v;
            }
        }
        x0 = x1;
        report(0.5 + 0.5 * (x0 as f64 / w as f64));
    }
}

/// Blur a binary `grid == target` mask straight to f32. Same kernel and f64
/// arithmetic as `gaussian_blur_grid` on the equivalent mask, so it is bit-identical;
/// it just skips the full-grid f64 copies that were the process memory peak.
/// `width`/`height` clamp per row exactly as the caller's `.take()` did, which the
/// edge renormalisation depends on.
pub(crate) fn gaussian_blur_mask_to_f32(
    grid: &[Vec<u8>],
    target: u8,
    width: usize,
    height: usize,
    sigma: f64,
) -> Vec<Vec<f32>> {
    gaussian_blur_mask_to_f32_reported(grid, target, width, height, sigma, &|_| {})
}

/// `gaussian_blur_mask_to_f32` with the same progress reporting as
/// `gaussian_blur_grid_reported`.
pub(crate) fn gaussian_blur_mask_to_f32_reported(
    grid: &[Vec<u8>],
    target: u8,
    width: usize,
    height: usize,
    sigma: f64,
    report: &dyn Fn(f64),
) -> Vec<Vec<f32>> {
    let kernel_size: usize = (sigma * 3.0).ceil() as usize * 2 + 1;
    let kernel = create_gaussian_kernel(kernel_size, sigma);
    let half = kernel_size as i32 / 2;

    let h = grid.len().min(height);
    if h == 0 {
        return Vec::new();
    }
    let w = grid[0].len().min(width);
    if w == 0 {
        return vec![Vec::new(); h];
    }

    const CHUNKS: usize = 10;

    // Horizontal pass: rows are independent, mask read on the fly.
    let row_chunk = h.div_ceil(CHUNKS);
    let mut after_h: Vec<Vec<f64>> = Vec::with_capacity(h);
    for rows in grid[..h].chunks(row_chunk) {
        let mut part: Vec<Vec<f64>> = rows
            .par_iter()
            .map(|row| {
                let len = row.len().min(width);
                let row_len = len as i32;
                let hits = prefix_counts(&row[..len], |&c| c == target);
                (0..len)
                    .map(|i| {
                        match window_hits(&hits, i, half, len) {
                            (0, _) => return 0.0,
                            (n, window) if n == window => return 1.0,
                            _ => {}
                        }
                        let mut sum = 0.0;
                        let mut wsum = 0.0;
                        for (j, &k) in kernel.iter().enumerate() {
                            let idx = i as i32 + j as i32 - half;
                            if idx >= 0 && idx < row_len {
                                let v = if row[idx as usize] == target {
                                    1.0
                                } else {
                                    0.0
                                };
                                sum += v * k;
                                wsum += k;
                            }
                        }
                        if wsum > 0.0 {
                            sum / wsum
                        } else {
                            f64::NAN
                        }
                    })
                    .collect()
            })
            .collect();
        after_h.append(&mut part);
        report(0.5 * (after_h.len() as f64 / h as f64));
    }

    // Vertical pass: f64 throughout, cast only on store.
    let col_chunk = w.div_ceil(CHUNKS);
    let mut out: Vec<Vec<f32>> = vec![vec![0.0; w]; h];
    let mut x0 = 0usize;
    while x0 < w {
        let x1 = (x0 + col_chunk).min(w);
        let blurred: Vec<(usize, Vec<f32>)> = (x0..x1)
            .into_par_iter()
            .map(|x| {
                let column: Vec<f64> = after_h.iter().map(|row| row[x]).collect();
                let col_len = column.len() as i32;
                let ones = prefix_counts(&column, |&v| v == 1.0);
                let zeros = prefix_counts(&column, |&v| v == 0.0);
                let col: Vec<f32> = (0..column.len())
                    .map(|y| {
                        let (n, window) = window_hits(&ones, y, half, column.len());
                        if n == window {
                            return 1.0;
                        }
                        let (n, window) = window_hits(&zeros, y, half, column.len());
                        if n == window {
                            return 0.0;
                        }
                        let mut sum = 0.0;
                        let mut wsum = 0.0;
                        for (j, &k) in kernel.iter().enumerate() {
                            let idx = y as i32 + j as i32 - half;
                            if idx >= 0 && idx < col_len {
                                let v = column[idx as usize];
                                if v.is_finite() {
                                    sum += v * k;
                                    wsum += k;
                                }
                            }
                        }
                        if wsum > 0.0 {
                            (sum / wsum) as f32
                        } else {
                            f64::NAN as f32
                        }
                    })
                    .collect();
                (x, col)
            })
            .collect();
        for (x, col) in blurred {
            for (y, v) in col.into_iter().enumerate() {
                out[y][x] = v;
            }
        }
        x0 = x1;
        report(0.5 + 0.5 * (x0 as f64 / w as f64));
    }
    out
}

/// `out[i]` = how many of `values[..i]` satisfy `hit`.
fn prefix_counts<T>(values: &[T], hit: impl Fn(&T) -> bool) -> Vec<u32> {
    let mut out = Vec::with_capacity(values.len() + 1);
    let mut n = 0u32;
    out.push(n);
    for v in values {
        n += u32::from(hit(v));
        out.push(n);
    }
    out
}

/// Hits among the kernel window around `i`, and the window's length, from `prefix_counts`.
///
/// A mask blur whose window is all 0 or all 1 has an exact answer: with every sample 0 the
/// sum stays 0, and with every sample 1 it adds the same weights in the same order as the
/// weight sum it is divided by. Uniform windows are most of a land-cover mask, and each
/// would otherwise cost the full kernel.
#[inline]
fn window_hits(hits: &[u32], i: usize, half: i32, len: usize) -> (usize, usize) {
    let lo = i.saturating_sub(half as usize);
    let hi = (i + half as usize + 1).min(len);
    ((hits[hi] - hits[lo]) as usize, hi - lo)
}

fn create_gaussian_kernel(size: usize, sigma: f64) -> Vec<f64> {
    let mut kernel = vec![0.0; size];
    // Centre tap, so the kernel is symmetric and the blur does not shift by half a cell.
    let center = (size - 1) as f64 / 2.0;
    for (i, value) in kernel.iter_mut().enumerate() {
        let x = i as f64 - center;
        *value = (-x * x / (2.0 * sigma * sigma)).exp();
    }
    let sum: f64 = kernel.iter().sum();
    for k in kernel.iter_mut() {
        *k /= sum;
    }
    kernel
}

/// Fill in any NaN values by iteratively interpolating from nearest valid neighbors.
/// Uses a snapshot each iteration to avoid directional bias from scan order.
///
/// Within one iteration each row's writes only depend on the read-only snapshot,
/// so the row sweep is parallelised. The convergence loop itself stays serial
/// because each iteration's snapshot must include the previous iteration's fills.
pub fn fill_nan_values(height_grid: &mut [Vec<f64>]) {
    let height: usize = height_grid.len();
    if height == 0 {
        return;
    }
    let width: usize = height_grid[0].len();

    if !height_grid
        .par_iter()
        .any(|row| row.iter().any(|v| v.is_nan()))
    {
        return;
    }

    // One snapshot buffer for the whole loop; refreshed in place per iteration.
    let mut snapshot: Vec<Vec<f64>> = height_grid.to_vec();
    loop {
        let snapshot_ref: &[Vec<f64>] = &snapshot;

        let any_changed = height_grid
            .par_iter_mut()
            .enumerate()
            .map(|(y, row)| {
                let mut row_changed = false;
                for (x, cell) in row.iter_mut().enumerate().take(width) {
                    if !cell.is_nan() {
                        continue;
                    }
                    let mut sum: f64 = 0.0;
                    let mut count: i32 = 0;
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let ny: i32 = y as i32 + dy;
                            let nx: i32 = x as i32 + dx;
                            if ny >= 0 && ny < height as i32 && nx >= 0 && nx < width as i32 {
                                let val: f64 = snapshot_ref[ny as usize][nx as usize];
                                if !val.is_nan() {
                                    sum += val;
                                    count += 1;
                                }
                            }
                        }
                    }
                    if count > 0 {
                        *cell = sum / count as f64;
                        row_changed = true;
                    }
                }
                row_changed
            })
            .reduce(|| false, |a, b| a || b);

        if !any_changed {
            break;
        }
        snapshot
            .par_iter_mut()
            .zip(height_grid.par_iter())
            .for_each(|(dst, src)| dst.clone_from(src));
    }
}

/// NaN out physically impossible elevations (provider sentinels, sea-floor artifacts),
/// then interpolate the holes shut.
///
/// A fixed gate, not a statistical one: a global IQR band flags any lone landform that
/// stands well clear of its surroundings, so an island or an inselberg reads as
/// corruption and gets levelled. Isolated spikes are `repair_terrain_anomalies`' job.
/// The bounds sit outside the real range (Dead Sea shore ~-430 m, Everest 8849 m) with
/// enough margin for providers that report ellipsoidal instead of orthometric heights.
pub fn filter_elevation_outliers(height_grid: &mut [Vec<f64>]) {
    const MIN_REASONABLE_M: f64 = -500.0;
    const MAX_REASONABLE_M: f64 = 9000.0;

    let height = height_grid.len();
    if height == 0 {
        return;
    }
    let width = height_grid[0].len();

    // Per-row NaN-out, then sum the per-row counts back together. Each row
    // is mutated independently so this is data-race-free.
    let outliers_filtered: usize = height_grid
        .par_iter_mut()
        .take(height)
        .map(|row| {
            let mut row_count = 0usize;
            for h in row.iter_mut().take(width) {
                if !h.is_nan() && (*h < MIN_REASONABLE_M || *h > MAX_REASONABLE_M) {
                    *h = f64::NAN;
                    row_count += 1;
                }
            }
            row_count
        })
        .sum();

    if outliers_filtered > 0 {
        eprintln!(
            "Filtered {} impossible elevations (outside {:.0}m..{:.0}m)",
            outliers_filtered, MIN_REASONABLE_M, MAX_REASONABLE_M
        );
        fill_nan_values(height_grid);
    }
}

/// Scale raw elevation (meters) to Minecraft Y coordinates, keeping f64 precision.
/// `extended_max_y` is the cap when `disable_height_limit` is on (Java datapack:
/// 2031; Bedrock BP: 512; Luanti has no pack, so it keeps the vanilla ceiling);
/// ignored otherwise.
/// Scales real-world metre heights to Minecraft Y. Also returns the affine
/// parameters `(min_height_m, blocks_per_meter)` so a real-world elevation can
/// be converted back to a Minecraft Y threshold (e.g. for the snow line), plus the
/// terrain base actually used (see `min_ground_level`).
///
/// `min_ground_level` is the lowest base the terrain may sink to. The base only sinks when
/// the relief genuinely does not fit above `ground_level`, so a small bbox is not dropped
/// into the basement just because the world floor was extended.
#[cfg(test)]
pub fn scale_to_minecraft(
    blurred_heights: &[Vec<f64>],
    scale: f64,
    ground_level: i32,
    min_ground_level: i32,
    disable_height_limit: bool,
    extended_max_y: i32,
) -> (Vec<Vec<f64>>, f64, f64, i32) {
    let (heights, affine) = scale_to_minecraft_with(
        blurred_heights,
        scale,
        ground_level,
        min_ground_level,
        disable_height_limit,
        extended_max_y,
        AffinePolicy::Fit,
    );
    (
        heights,
        affine.min_height_m,
        affine.blocks_per_meter,
        affine.ground_level,
    )
}

/// `y = ground_level + (h_m - min_height_m) * blocks_per_meter`, bent above
/// `soft_top` if set.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct ElevationAffine {
    pub min_height_m: f64,
    pub blocks_per_meter: f64,
    pub ground_level: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_top: Option<SoftTop>,
}

/// Above `knee_m` heights are compressed more the higher they are instead of
/// clamped: `y = y(knee) + width * asinh((h - knee) * blocks_per_meter / width)`.
/// The slope at the knee matches the straight part.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct SoftTop {
    pub knee_m: f64,
    pub width_blocks: f64,
}

/// Lowest and highest land on Earth: the Dead Sea shore and Everest.
const LOWEST_LAND_M: f64 = -430.0;
const HIGHEST_LAND_M: f64 = 8849.0;
/// Blocks under the ceiling that the soft top may use.
const SOFT_TOP_BLOCKS: f64 = 800.0;

impl ElevationAffine {
    #[inline]
    pub fn y_for_metres(&self, h_m: f64) -> f64 {
        match self.soft_top {
            Some(top) if h_m > top.knee_m => {
                let knee_y = self.ground_level as f64
                    + (top.knee_m - self.min_height_m) * self.blocks_per_meter;
                let rise = (h_m - top.knee_m) * self.blocks_per_meter;
                knee_y + top.width_blocks * (rise / top.width_blocks).asinh()
            }
            _ => self.ground_level as f64 + (h_m - self.min_height_m) * self.blocks_per_meter,
        }
    }

    /// One mapping for all land on Earth: `scale` blocks per metre from the
    /// Dead Sea at `floor` upwards, with a soft top if Everest would otherwise
    /// end above the ceiling.
    pub fn whole_earth(scale: f64, floor: i32, extended_max_y: i32) -> Self {
        let ceiling = terrain_ceiling(true, extended_max_y) as f64;
        let mut affine = Self {
            min_height_m: LOWEST_LAND_M,
            blocks_per_meter: scale,
            ground_level: floor,
            soft_top: None,
        };
        let knee_y = ceiling - SOFT_TOP_BLOCKS;
        if affine.y_for_metres(HIGHEST_LAND_M) <= ceiling || knee_y <= floor as f64 {
            return affine;
        }
        let knee_m = LOWEST_LAND_M + (knee_y - floor as f64) / scale;
        let rise = (HIGHEST_LAND_M - knee_m) * scale;
        // width * asinh(rise / width) grows with the width towards `rise`.
        let (mut lo, mut hi) = (1e-3, 1e9);
        for _ in 0..200 {
            let mid = (lo + hi) / 2.0;
            if mid * (rise / mid).asinh() > SOFT_TOP_BLOCKS {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        affine.soft_top = Some(SoftTop {
            knee_m,
            width_blocks: lo,
        });
        affine
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AffinePolicy {
    /// Fit this grid's own relief (the ordinary mode).
    Fit,
    /// Fit, then leave room below for lower neighbours. First One World area.
    FitWithHeadroom,
    /// Reuse a stored mapping. Later One World areas.
    Fixed(ElevationAffine),
}

/// Most headroom `FitWithHeadroom` keeps below the lowest cell.
const HEADROOM_MAX_BLOCKS: f64 = 96.0;

fn terrain_ceiling(disable_height_limit: bool, extended_max_y: i32) -> i32 {
    let effective_max_y = if disable_height_limit {
        extended_max_y
    } else {
        MAX_Y
    };
    effective_max_y - TERRAIN_HEIGHT_BUFFER
}

fn derive_affine(
    blurred_heights: &[Vec<f64>],
    scale: f64,
    ground_level: i32,
    min_ground_level: i32,
    disable_height_limit: bool,
    extended_max_y: i32,
) -> (ElevationAffine, FitRange) {
    // Derive min/max
    let (min_height, max_height) = blurred_heights
        .par_iter()
        .map(|row| {
            let mut lo = f64::MAX;
            let mut hi = f64::MIN;
            for &h in row {
                if h.is_finite() {
                    lo = lo.min(h);
                    hi = hi.max(h);
                }
            }
            (lo, hi)
        })
        .reduce(
            || (f64::MAX, f64::MIN),
            |(lo1, hi1), (lo2, hi2)| (lo1.min(lo2), hi1.max(hi2)),
        );

    let (min_height, height_range) =
        if !min_height.is_finite() || !max_height.is_finite() || min_height >= max_height {
            // Zero-relief/degenerate: keep the real min height (the snow line
            // needs it) but flatten the range so every cell maps to ground_level.
            // `min <= max` distinguishes true flat terrain from an all-NaN grid,
            // whose reduce leaves min = f64::MAX (finite but bogus) -> use 0.
            let real_min = if min_height.is_finite() && min_height <= max_height {
                min_height
            } else {
                0.0
            };
            (real_min, 0.0_f64)
        } else {
            (min_height, max_height - min_height)
        };

    let ideal_scaled_range: f64 = height_range * scale;
    let ceiling = terrain_ceiling(disable_height_limit, extended_max_y);

    // Sink the terrain base to reach the extended floor, but only as far as the relief
    // actually needs. Blindly sinking would drop a low-relief bbox thousands of blocks down
    // with an empty sky above it; not sinking at all would waste the pack's lower half.
    // Only ever fires with the extended floor: callers pass min_ground_level == ground_level
    // otherwise, so an explicit --ground-level is never silently overridden.
    let ground_level = if disable_height_limit
        && min_ground_level < ground_level
        && ideal_scaled_range.is_finite()
    {
        let needed = ideal_scaled_range.ceil() as i32;
        (ceiling.saturating_sub(needed)).clamp(min_ground_level, ground_level)
    } else {
        ground_level
    };

    let available_y_range: f64 = (ceiling - ground_level) as f64;

    let scaled_range: f64 = if ideal_scaled_range <= available_y_range {
        eprintln!(
            "Realistic elevation: {:.1}m range fits in {} available blocks",
            height_range, available_y_range as i32
        );
        ideal_scaled_range
    } else {
        let compression_factor: f64 = available_y_range / height_range;
        let compressed_range: f64 = height_range * compression_factor;
        eprintln!(
            "Elevation compressed: {:.1}m range -> {:.0} blocks ({:.2}:1 ratio, 1 block = {:.2}m)",
            height_range,
            compressed_range,
            height_range / compressed_range,
            compressed_range / height_range
        );
        compressed_range
    };

    let blocks_per_meter = if height_range > 0.0 {
        scaled_range / height_range
    } else {
        0.0
    };
    (
        ElevationAffine {
            min_height_m: min_height,
            blocks_per_meter,
            ground_level,
            soft_top: None,
        },
        FitRange {
            height_range,
            scaled_range,
        },
    )
}

#[derive(Clone, Copy)]
struct FitRange {
    height_range: f64,
    scaled_range: f64,
}

/// Kept in its original arithmetic so default worlds round exactly as before.
fn apply_fit(
    blurred_heights: &[Vec<f64>],
    affine: &ElevationAffine,
    fit: FitRange,
    upper_clamp: f64,
) -> Vec<Vec<f64>> {
    let base = affine.ground_level as f64;
    blurred_heights
        .par_iter()
        .map(|row| {
            row.iter()
                .map(|&h| {
                    let relative_height: f64 = if fit.height_range > 0.0 {
                        (h - affine.min_height_m) / fit.height_range
                    } else {
                        0.0
                    };
                    (base + relative_height * fit.scaled_range).clamp(base, upper_clamp)
                })
                .collect()
        })
        .collect()
}

fn apply_affine(
    blurred_heights: &[Vec<f64>],
    affine: &ElevationAffine,
    upper_clamp: f64,
) -> Vec<Vec<f64>> {
    let base = affine.ground_level as f64;
    blurred_heights
        .par_iter()
        .map(|row| {
            row.iter()
                .map(|&h| affine.y_for_metres(h).clamp(base, upper_clamp))
                .collect()
        })
        .collect()
}

pub fn scale_to_minecraft_with(
    blurred_heights: &[Vec<f64>],
    scale: f64,
    ground_level: i32,
    min_ground_level: i32,
    disable_height_limit: bool,
    extended_max_y: i32,
    policy: AffinePolicy,
) -> (Vec<Vec<f64>>, ElevationAffine) {
    let ceiling = terrain_ceiling(disable_height_limit, extended_max_y);
    let upper_clamp = ceiling as f64;

    let mut fit = None;
    let affine = match policy {
        AffinePolicy::Fit => {
            let (affine, range) = derive_affine(
                blurred_heights,
                scale,
                ground_level,
                min_ground_level,
                disable_height_limit,
                extended_max_y,
            );
            fit = Some(range);
            affine
        }
        AffinePolicy::FitWithHeadroom => {
            let (mut affine, range) = derive_affine(
                blurred_heights,
                scale,
                ground_level,
                min_ground_level,
                disable_height_limit,
                extended_max_y,
            );
            // A flat first area chose no slope; later areas need one.
            if affine.blocks_per_meter <= 0.0 {
                affine.blocks_per_meter = scale;
            }
            let free = (ceiling - affine.ground_level) as f64 - range.scaled_range;
            let margin_blocks = (free * 0.5).min(HEADROOM_MAX_BLOCKS).floor().max(0.0);
            if margin_blocks > 0.0 {
                affine.min_height_m -= margin_blocks / affine.blocks_per_meter;
                eprintln!(
                    "One World: keeping {} blocks below the lowest terrain for neighbouring areas",
                    margin_blocks as i32
                );
            }
            affine
        }
        AffinePolicy::Fixed(affine) => {
            eprintln!(
                "One World: using the world's elevation mapping (1 block = {:.2} m, {:.0} m at Y {}{})",
                if affine.blocks_per_meter > 0.0 {
                    1.0 / affine.blocks_per_meter
                } else {
                    f64::INFINITY
                },
                affine.min_height_m,
                affine.ground_level,
                match affine.soft_top {
                    Some(top) => format!(", compressed above {:.0} m", top.knee_m),
                    None => String::new(),
                }
            );
            affine
        }
    };

    let mc_heights = match fit {
        Some(range) => apply_fit(blurred_heights, &affine, range, upper_clamp),
        None => apply_affine(blurred_heights, &affine, upper_clamp),
    };

    if let AffinePolicy::Fixed(_) = policy {
        let base = affine.ground_level as f64;
        let (clamped, total) = blurred_heights
            .par_iter()
            .map(|row| {
                let mut c = 0usize;
                let mut n = 0usize;
                for &h in row {
                    if h.is_finite() {
                        n += 1;
                        let y = affine.y_for_metres(h);
                        // Anything below the lowest land is sea floor, under water anyway.
                        if (y < base && h > LOWEST_LAND_M) || y > upper_clamp {
                            c += 1;
                        }
                    }
                }
                (c, n)
            })
            .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        if total > 0 && clamped * 200 > total {
            let pct = clamped as f64 * 100.0 / total as f64;
            eprintln!(
                "Warning: {pct:.1}% of this area lies outside the world's height band (Y {} to {}) and is flattened there.{}",
                affine.ground_level,
                upper_clamp as i32,
                if disable_height_limit {
                    ""
                } else {
                    " The band was fixed by the first area; a new One World has room for all of Earth."
                }
            );
            crate::progress::emit_gui_progress_update(
                crate::progress::MESSAGE_ONLY,
                &format!("Note: {pct:.0}% of the area exceeds the world's height band"),
            );
        }
    }

    // The map preview shades over the band the terrain fills, so publish where it ended up.
    let top = match fit {
        Some(range) => affine.ground_level as f64 + range.scaled_range,
        None => mc_heights
            .par_iter()
            .map(|row| row.iter().cloned().fold(f64::MIN, f64::max))
            .reduce(|| f64::MIN, f64::max),
    };
    let top = if top.is_finite() {
        top
    } else {
        affine.ground_level as f64
    };
    crate::world_editor::common::set_terrain_top_y(top.min(upper_clamp).round() as i32);

    (mc_heights, affine)
}

#[cfg(test)]
mod mask_blur_tests {
    use super::*;

    fn reference(
        grid: &[Vec<u8>],
        target: u8,
        width: usize,
        height: usize,
        sigma: f64,
    ) -> Vec<Vec<f32>> {
        let binary: Vec<Vec<f64>> = grid
            .iter()
            .take(height)
            .map(|row| {
                row.iter()
                    .take(width)
                    .map(|&c| if c == target { 1.0 } else { 0.0 })
                    .collect()
            })
            .collect();
        gaussian_blur_grid(&binary, sigma)
            .into_iter()
            .map(|row| row.into_iter().map(|v| v as f32).collect())
            .collect()
    }

    fn check(grid: &[Vec<u8>], width: usize, height: usize, sigma: f64) {
        let got = gaussian_blur_mask_to_f32(grid, 1, width, height, sigma);
        let want = reference(grid, 1, width, height, sigma);
        assert_eq!(got.len(), want.len());
        for (y, (a, b)) in got.iter().zip(want.iter()).enumerate() {
            assert_eq!(a.len(), b.len(), "row {y} length");
            for (x, (p, q)) in a.iter().zip(b.iter()).enumerate() {
                assert_eq!(p.to_bits(), q.to_bits(), "cell ({x},{y})");
            }
        }
    }

    fn grid(w: usize, h: usize, seed: u64) -> Vec<Vec<u8>> {
        let mut s = seed;
        (0..h)
            .map(|_| {
                (0..w)
                    .map(|_| {
                        s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
                        ((s >> 60) % 2) as u8
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn mask_blur_is_bit_identical_to_building_the_f64_mask() {
        check(&grid(37, 29, 1), 37, 29, 3.0);
        check(&grid(64, 8, 7), 64, 8, 1.5);
    }

    #[test]
    fn mask_blur_clamps_oversized_grids_like_the_take_did() {
        // Both dimensions larger than the requested extent.
        check(&grid(50, 40, 3), 37, 29, 3.0);
    }

    /// Large solid blobs, so most windows are all-0 or all-1 and take the shortcut.
    fn blobs(w: usize, h: usize) -> Vec<Vec<u8>> {
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| {
                        let (dx, dy) = (x as f64 - w as f64 * 0.4, y as f64 - h as f64 * 0.5);
                        let disc = dx * dx + dy * dy < (w as f64 * 0.25).powi(2);
                        let band = (x + 2 * y) % 97 < 30;
                        u8::from(disc || band)
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn uniform_windows_match_the_full_kernel() {
        check(&blobs(120, 90), 120, 90, 3.0);
        check(&blobs(200, 60), 200, 60, 12.0);
        // Everything one class, and nothing of it.
        check(&vec![vec![1u8; 40]; 30], 40, 30, 3.0);
        check(&vec![vec![0u8; 40]; 30], 40, 30, 3.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::land_cover::LC_GRASSLAND;

    /// 200x200 m grid at 1 m/cell: a hillside rising 0.6 m per metre along
    /// x (31 degrees) with land everywhere.
    fn hillside(n: usize) -> (Vec<Vec<f64>>, Vec<Vec<u8>>) {
        let heights = (0..n)
            .map(|_| (0..n).map(|x| x as f64 * 0.6).collect())
            .collect();
        (heights, vec![vec![LC_GRASSLAND; n]; n])
    }

    #[test]
    fn steep_water_blob_on_hillside_is_dropped() {
        let (heights, mut lc) = hillside(200);
        for row in lc.iter_mut().take(120).skip(80) {
            for c in row.iter_mut().take(130).skip(90) {
                *c = LC_WATER;
            }
        }
        let dropped = drop_water_on_steep_terrain(&heights, &mut lc, 1.0);
        assert_eq!(dropped, 40 * 40);
        assert!(lc.iter().flatten().all(|&c| c != LC_WATER));
        // Near the edge the surrounding class is adopted; deep inside the
        // blob nothing is within reach and bare ground stands in.
        assert_eq!(lc[82][92], LC_GRASSLAND);
        assert_eq!(lc[100][110], crate::land_cover::LC_BARE);
    }

    #[test]
    fn flat_lake_in_a_bowl_is_kept() {
        // Bowl: terrain rises away from the centre; the lake floor is flat.
        let n = 200usize;
        let mut heights: Vec<Vec<f64>> = (0..n)
            .map(|z| {
                (0..n)
                    .map(|x| {
                        let d = (((x as f64 - 100.0).powi(2) + (z as f64 - 100.0).powi(2)).sqrt()
                            - 30.0)
                            .max(0.0);
                        d * 0.5
                    })
                    .collect()
            })
            .collect();
        let mut lc = vec![vec![LC_GRASSLAND; n]; n];
        for z in 70..130 {
            for x in 70..130 {
                if ((x as f64 - 100.0).powi(2) + (z as f64 - 100.0).powi(2)).sqrt() <= 30.0 {
                    lc[z][x] = LC_WATER;
                    heights[z][x] = 0.0;
                }
            }
        }
        let dropped = drop_water_on_steep_terrain(&heights, &mut lc, 1.0);
        assert_eq!(dropped, 0);
        assert_eq!(lc[100][100], LC_WATER);
    }

    #[test]
    fn steep_narrow_watercourse_is_kept() {
        // An 11 m wide V-notch stream at 4 % grade with 50 degree walls: steep terrain,
        // but the water surface is flat across it and the banks rise on both sides.
        let n = 400usize;
        let heights: Vec<Vec<f64>> = (0..n)
            .map(|z| {
                (0..n)
                    .map(|x| {
                        let wall = (((z as f64 - 200.0).abs()) - 5.0).max(0.0) * 1.2;
                        1000.0 - x as f64 * 0.04 + wall
                    })
                    .collect()
            })
            .collect();
        let mut lc = vec![vec![LC_GRASSLAND; n]; n];
        for row in lc.iter_mut().take(206).skip(195) {
            for c in row.iter_mut() {
                *c = LC_WATER;
            }
        }
        assert_eq!(drop_water_on_steep_terrain(&heights, &mut lc, 1.0), 0);
        assert_eq!(lc[200][200], LC_WATER);
    }

    #[test]
    fn flowing_surface_never_rises_over_its_banks() {
        // A 4 m fall mid-river: the reach below it must not be lifted over its own bed.
        let (mut heights, lc) = river(|x| x * 0.03 + if x >= 200.0 { 4.0 } else { 0.0 });
        let before = heights.clone();
        level_water_surfaces(&mut heights, &lc, 1.0);
        for z in 170..231 {
            for x in 0..400 {
                let lowest = lowest_adjacent_land(&before, &lc, x, z);
                let cap = before[z][x].max(lowest.unwrap_or(f64::INFINITY));
                assert!(
                    heights[z][x] <= cap + 1e-6,
                    "({x},{z}) rose to {} over cap {cap}",
                    heights[z][x]
                );
            }
        }
    }

    #[test]
    fn steep_water_wider_than_the_cap_is_kept() {
        // Same hillside, but the water covers 0.5 km^2 (an ESA blob never
        // does; a fjord with a steep bathymetric DEM might).
        let (heights, mut lc) = hillside(1000);
        for row in lc.iter_mut().take(900).skip(100) {
            for c in row.iter_mut().take(900).skip(100) {
                *c = LC_WATER;
            }
        }
        assert_eq!(drop_water_on_steep_terrain(&heights, &mut lc, 1.0), 0);
    }

    #[test]
    fn river_with_gradient_is_kept() {
        // A river 20 m wide dropping 2 % along its length, banks rising
        // steeply on both sides: median slope is the flat surface.
        let n = 200usize;
        let heights: Vec<Vec<f64>> = (0..n)
            .map(|z| {
                (0..n)
                    .map(|x| {
                        let along = x as f64 * 0.02;
                        let across = ((z as f64 - 100.0).abs() - 10.0).max(0.0) * 0.8;
                        along + across
                    })
                    .collect()
            })
            .collect();
        let mut lc = vec![vec![LC_GRASSLAND; n]; n];
        for row in lc.iter_mut().take(111).skip(90) {
            for c in row.iter_mut() {
                *c = LC_WATER;
            }
        }
        assert_eq!(drop_water_on_steep_terrain(&heights, &mut lc, 1.0), 0);
    }

    /// A river 60 m wide running along x across a 400 m grid at 1 m/cell,
    /// with the given surface profile along x; banks rise steeply.
    fn river(profile: impl Fn(f64) -> f64) -> (Vec<Vec<f64>>, Vec<Vec<u8>>) {
        let n = 400usize;
        let heights: Vec<Vec<f64>> = (0..n)
            .map(|z| {
                (0..n)
                    .map(|x| {
                        let across = ((z as f64 - 200.0).abs() - 30.0).max(0.0) * 0.8;
                        profile(x as f64) + across
                    })
                    .collect()
            })
            .collect();
        let mut lc = vec![vec![LC_GRASSLAND; n]; n];
        for row in lc.iter_mut().take(231).skip(170) {
            for c in row.iter_mut() {
                *c = LC_WATER;
            }
        }
        (heights, lc)
    }

    #[test]
    fn coarse_field_stays_bounded_for_a_grid_spanning_river() {
        // A thin river across the largest supported grid, at the metres-per-cell where the
        // sigma alone leaves the step at 1 and the whole bounding box gets allocated.
        const N: usize = 16384;
        let step = coarse_step(N, N, 6.6);
        let cw = (N - 1) / step + 1;
        assert!(cw * cw <= 4 << 20, "coarse grid is {} cells", cw * cw);
    }

    #[test]
    fn small_components_keep_the_sigma_step() {
        // City scale must keep the sampling the blur was tuned for.
        assert_eq!(coarse_step(1583, 2217, 40.0), 5);
    }

    #[test]
    fn flowing_surface_smooths_dem_seam_steps() {
        // 3 % gradient (flowing) with a 3 m project seam at x = 200.
        let (mut heights, lc) = river(|x| x * 0.03 + if x >= 200.0 { 3.0 } else { 0.0 });
        level_water_surfaces(&mut heights, &lc, 1.0);
        let row = &heights[200];
        let max_step = (150..250)
            .map(|x| (row[x + 1] - row[x]).abs())
            .fold(0.0, f64::max);
        assert!(max_step < 0.5, "seam still steps by {max_step} m");
        // The overall gradient survives.
        assert!(row[350] - row[50] > 8.0);
    }

    #[test]
    fn flowing_surface_keeps_a_real_drop() {
        // 3 % gradient with a 30 m fall at x = 200: the edge must stay sharp.
        let (mut heights, lc) = river(|x| x * 0.03 + if x >= 200.0 { 30.0 } else { 0.0 });
        level_water_surfaces(&mut heights, &lc, 1.0);
        let row = &heights[200];
        let max_step = (150..250)
            .map(|x| (row[x + 1] - row[x]).abs())
            .fold(0.0, f64::max);
        assert!(max_step > 20.0, "drop was smeared to {max_step} m/cell");
    }

    /// A lake at 100 m, most of the water, spilling over a falls face into a 40 m gorge.
    fn lake_over_a_gorge() -> (Vec<Vec<f64>>, Vec<Vec<u8>>) {
        let (w, h) = (300usize, 200usize);
        let mut heights = vec![vec![103.0; w]; h];
        let mut lc = vec![vec![LC_GRASSLAND; w]; h];
        for z in 0..h {
            for x in 0..w {
                let lake = x < 200 && (40..160).contains(&z);
                let face = (200..230).contains(&x) && (90..110).contains(&z);
                let gorge = x >= 230 && (90..110).contains(&z);
                if lake {
                    heights[z][x] = 100.0;
                } else if face {
                    heights[z][x] = 100.0 - (x - 200) as f64 * 1.3;
                } else if gorge {
                    heights[z][x] = 60.0 - (x - 230) as f64 * 0.02;
                } else if x >= 200 && (70..130).contains(&z) {
                    // Gorge walls, rising from its water to the plateau.
                    let off = if z < 90 { 90 - z } else { z - 109 } as f64;
                    heights[z][x] = (62.0 + off * 2.0).min(103.0);
                }
                if lake || face || gorge {
                    lc[z][x] = LC_WATER;
                }
            }
        }
        (heights, lc)
    }

    #[test]
    fn a_lake_keeps_its_level_above_the_gorge_it_spills_into() {
        // Read as one still lake, its level must not fill the gorge nor the gorge banks clamp it.
        let (mut heights, lc) = lake_over_a_gorge();
        let surface = level_water_surfaces(&mut heights, &lc, 1.0);
        assert!(
            (heights[100][100] - 100.0).abs() < 0.5,
            "lake at {}",
            heights[100][100]
        );
        for x in 240..300 {
            assert!(
                heights[100][x] < 62.0,
                "gorge raised to {} at x={x}",
                heights[100][x]
            );
            assert!(surface[100][x], "gorge left dry at x={x}");
        }
    }

    #[test]
    fn a_pit_under_a_lake_is_still_flattened_to_its_surface() {
        // A deep bed held by the lake's own shore is bathymetry, not water below a fall.
        let n = 200usize;
        let mut heights = vec![vec![102.0; n]; n];
        let mut lc = vec![vec![LC_GRASSLAND; n]; n];
        for z in 20..180 {
            for x in 20..180 {
                lc[z][x] = LC_WATER;
                heights[z][x] = if (80..120).contains(&x) && (80..120).contains(&z) {
                    80.0
                } else {
                    100.0
                };
            }
        }
        let surface = level_water_surfaces(&mut heights, &lc, 1.0);
        assert_eq!(heights[100][100], heights[40][40]);
        assert!((heights[40][40] - 100.0).abs() <= 1.0);
        assert!(surface[100][100]);
    }

    #[test]
    fn a_river_running_into_a_lake_keeps_its_gradient() {
        // The river above the lake is held by its own banks and must not be cut down to it.
        let (w, h) = (320usize, 200usize);
        let mut heights = vec![vec![104.0; w]; h];
        let mut lc = vec![vec![LC_GRASSLAND; w]; h];
        for z in 0..h {
            for x in 0..w {
                if x < 200 && (20..180).contains(&z) {
                    lc[z][x] = LC_WATER;
                    heights[z][x] = 100.0;
                } else if x >= 200 {
                    let bed = 100.0 + (x - 200) as f64 * 0.15;
                    if (95..105).contains(&z) {
                        lc[z][x] = LC_WATER;
                        heights[z][x] = bed;
                    } else {
                        heights[z][x] = bed + 3.0 + (z as f64 - 100.0).abs() * 0.1;
                    }
                }
            }
        }
        let surface = level_water_surfaces(&mut heights, &lc, 1.0);
        for x in [260, 290, 310] {
            let bed = 100.0 + (x - 200) as f64 * 0.15;
            assert!(surface[100][x], "river dropped at x={x}");
            assert!(
                (heights[100][x] - bed).abs() < 2.0,
                "x={x}: {} for {bed}",
                heights[100][x]
            );
        }
    }

    #[test]
    fn a_deck_across_a_lake_is_flattened_with_it() {
        // Mostly surrounded by the lake, a raised strip is no water of its own.
        let n = 200usize;
        let mut heights = vec![vec![103.0; n]; n];
        let mut lc = vec![vec![LC_GRASSLAND; n]; n];
        for z in 20..180 {
            for x in 0..n {
                lc[z][x] = LC_WATER;
                heights[z][x] = if (90..110).contains(&x) { 110.0 } else { 100.0 };
            }
        }
        let surface = level_water_surfaces(&mut heights, &lc, 1.0);
        assert_eq!(heights[100][100], heights[100][40]);
        assert!((heights[100][40] - 100.0).abs() <= 1.0);
        assert!(surface[100][100]);
    }

    /// A 21 m river falling 3 % along x, its land cover water climbing `wall` m/m up both sides.
    fn v_channel(wall: f64) -> (Vec<Vec<f64>>, Vec<Vec<u8>>) {
        let (w, h) = (400usize, 100usize);
        let heights = (0..h)
            .map(|z| {
                (0..w)
                    .map(|x| 500.0 - x as f64 * 0.03 + (z as f64 - 50.0).abs() * wall)
                    .collect()
            })
            .collect();
        let mut lc = vec![vec![LC_GRASSLAND; w]; h];
        for row in lc.iter_mut().take(61).skip(40) {
            row.fill(LC_WATER);
        }
        (heights, lc)
    }

    #[test]
    fn water_does_not_climb_the_walls_of_a_smeared_gorge() {
        let (mut heights, lc) = v_channel(0.6);
        let surface = level_water_surfaces(&mut heights, &lc, 1.0);
        assert!(surface[50][200], "the channel floor is water");
        assert!(!surface[43][200], "a wall 4 m up holds no water");
        assert!(!surface[57][200], "a wall 4 m up holds no water");
    }

    #[test]
    fn a_level_river_has_no_walls() {
        let (mut heights, lc) = v_channel(0.0);
        let surface = level_water_surfaces(&mut heights, &lc, 1.0);
        assert!((40..61).all(|z| surface[z][200]));
    }

    #[test]
    fn a_seam_along_a_narrow_river_is_no_wall() {
        // Walks across a 7 m river end early; the step is on one half of them only.
        let (w, h) = (400usize, 100usize);
        let mut heights = vec![vec![0.0; w]; h];
        let mut lc = vec![vec![LC_GRASSLAND; w]; h];
        for (z, (row, lc_row)) in heights.iter_mut().zip(lc.iter_mut()).enumerate() {
            for (x, (v, c)) in row.iter_mut().zip(lc_row.iter_mut()).enumerate() {
                let bed = 500.0 - x as f64 * 0.03;
                *v = if (47..54).contains(&z) {
                    *c = LC_WATER;
                    bed + if z < 49 { 1.5 } else { 0.0 }
                } else {
                    bed + 10.0
                };
            }
        }
        let surface = level_water_surfaces(&mut heights, &lc, 1.0);
        assert!(
            surface[47][200] && surface[48][200],
            "the seam's high side is water"
        );
    }

    #[test]
    fn a_falls_face_inside_a_river_keeps_its_water() {
        // The bank lies upstream, so the face falls away from it like a wall, under level water.
        let (w, h) = (300usize, 260usize);
        let mut heights = vec![vec![125.0; w]; h];
        let mut lc = vec![vec![LC_GRASSLAND; w]; h];
        for z in 20..240 {
            for x in 20..280 {
                lc[z][x] = LC_WATER;
                heights[z][x] = 120.0 - (z.clamp(80, 110) - 80) as f64 * 20.0 / 30.0;
            }
        }
        let surface = level_water_surfaces(&mut heights, &lc, 1.0);
        for z in 82..108 {
            assert!(surface[z][150], "face left dry at z={z}");
            assert!(
                (100.0..=120.5).contains(&heights[z][150]),
                "z={z}: {}",
                heights[z][150]
            );
        }
    }

    /// Swiss relief: Lake Maggiore 193 m to Dufourspitze 4634 m.
    fn swiss_grid() -> Vec<Vec<f64>> {
        vec![vec![193.0, 4634.0], vec![193.0, 4634.0]]
    }

    #[test]
    fn terrain_sinks_only_as_far_as_the_relief_needs() {
        // Java datapack: floor -2032, so the base may sink to -2030.
        let grid = swiss_grid();

        // At scale 0.1 the relief needs 444 blocks, which already fits above -62.
        // The base must NOT sink: doing so would bury a shallow world in the basement.
        let (_mc, _min_m, bpm, base) = scale_to_minecraft(&grid, 0.1, -62, -2030, true, 2031);
        assert_eq!(base, -62, "base must not sink when the relief already fits");
        assert!(
            (bpm - 0.1).abs() < 1e-9,
            "relief fits, so it must map 1:1 with the horizontal scale (got {bpm})"
        );

        // At scale 1.0 the relief needs 4441 blocks; only 2078 exist above -62, so the
        // base sinks to reach the datapack's lower half.
        let (_mc, _min_m, bpm, base) = scale_to_minecraft(&grid, 1.0, -62, -2030, true, 2031);
        assert_eq!(
            base, -2030,
            "base must sink to the floor when the relief needs it"
        );
        // Headroom is now 2031 - 15 + 2030 = 4046 against 4441 m of relief.
        assert!(
            (0.90..0.92).contains(&bpm),
            "sinking must give near-1:1 vertical (got {bpm})"
        );
    }

    #[test]
    fn vanilla_never_sinks_an_explicit_ground_level() {
        // With the vanilla floor the caller passes min_ground_level == ground_level, and the
        // disable_height_limit gate holds regardless: an explicit --ground-level is honoured
        // and the relief is compressed to fit, as before.
        let grid = swiss_grid();
        let (_mc, _min_m, _bpm, base) = scale_to_minecraft(&grid, 1.0, 100, -62, false, 0);
        assert_eq!(
            base, 100,
            "vanilla must not sink below an explicit ground level"
        );
    }

    #[test]
    fn without_the_extended_floor_the_alps_are_compressed() {
        // Vanilla: 319 - 15 + 62 = 366 blocks for 4441 m of relief.
        let grid = swiss_grid();
        let (_mc, _min_m, bpm, base) = scale_to_minecraft(&grid, 1.0, -62, -62, false, 0);
        assert_eq!(base, -62);
        assert!(
            (bpm - 366.0 / 4441.0).abs() < 1e-6,
            "vanilla must compress 4441 m into 366 blocks (got {bpm})"
        );
    }

    #[test]
    fn scale_flat_terrain_keeps_real_min_height() {
        // Zero-relief terrain must still report its true elevation so the snow
        // line can tell a high plateau from a low one.
        let grid = vec![vec![4500.0_f64; 4]; 4];
        let (mc, min_m, blocks_per_meter, _base) = scale_to_minecraft(&grid, 1.0, 64, 64, false, 0);
        assert_eq!(min_m, 4500.0);
        assert_eq!(blocks_per_meter, 0.0);
        // Every cell flattens to ground level.
        assert!(mc.iter().flatten().all(|&y| (y - 64.0).abs() < 1e-9));
    }

    #[test]
    fn scale_all_nan_grid_min_height_zero() {
        // No finite samples must not leak the f64::MAX reduce sentinel as min.
        let grid = vec![vec![f64::NAN; 4]; 4];
        let (_mc, min_m, blocks_per_meter, _base) =
            scale_to_minecraft(&grid, 1.0, 64, 64, false, 0);
        assert_eq!(min_m, 0.0);
        assert_eq!(blocks_per_meter, 0.0);
    }

    #[test]
    fn test_fill_nan_values() {
        let mut grid = vec![
            vec![1.0, f64::NAN, 3.0],
            vec![f64::NAN, f64::NAN, f64::NAN],
            vec![7.0, f64::NAN, 9.0],
        ];
        fill_nan_values(&mut grid);
        for row in &grid {
            for &h in row {
                assert!(!h.is_nan(), "NaN values should be filled");
            }
        }
    }

    fn wobbly(w: usize, h: usize) -> Vec<Vec<f64>> {
        let mut s = 0x2545_f491_4f6c_dd1du64;
        (0..h)
            .map(|_| {
                (0..w)
                    .map(|_| {
                        s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
                        ((s >> 40) % 2000) as f64 * 0.5
                    })
                    .collect()
            })
            .collect()
    }

    fn assert_bits_eq(got: &[Vec<f64>], want: &[Vec<f64>]) {
        assert_eq!(got.len(), want.len());
        for (y, (a, b)) in got.iter().zip(want.iter()).enumerate() {
            assert_eq!(a.len(), b.len(), "row {y} length");
            for (x, (p, q)) in a.iter().zip(b.iter()).enumerate() {
                assert_eq!(p.to_bits(), q.to_bits(), "cell ({x},{y})");
            }
        }
    }

    /// Separable blur written with two buffers and no parallelism, in the same tap
    /// order as the production kernel loop.
    fn two_buffer_blur(grid: &[Vec<f64>], sigma: f64) -> Vec<Vec<f64>> {
        let kernel_size: usize = (sigma * 3.0).ceil() as usize * 2 + 1;
        let kernel = create_gaussian_kernel(kernel_size, sigma);
        let half = kernel_size as i32 / 2;
        let h = grid.len();
        let w = grid[0].len();
        let tap = |get: &dyn Fn(usize) -> f64, len: usize, i: usize| {
            let mut sum = 0.0;
            let mut wsum = 0.0;
            for (j, &k) in kernel.iter().enumerate() {
                let idx = i as i32 + j as i32 - half;
                if idx >= 0 && idx < len as i32 {
                    let v = get(idx as usize);
                    if v.is_finite() {
                        sum += v * k;
                        wsum += k;
                    }
                }
            }
            if wsum > 0.0 {
                sum / wsum
            } else {
                f64::NAN
            }
        };
        let after_h: Vec<Vec<f64>> = (0..h)
            .map(|y| (0..w).map(|x| tap(&|i| grid[y][i], w, x)).collect())
            .collect();
        (0..h)
            .map(|y| (0..w).map(|x| tap(&|i| after_h[i][x], h, y)).collect())
            .collect()
    }

    #[test]
    fn the_vertical_blur_pass_writing_over_its_own_input_changes_nothing() {
        let g = wobbly(37, 29);
        assert_bits_eq(&gaussian_blur_grid(&g, 2.5), &two_buffer_blur(&g, 2.5));
        let tall = wobbly(9, 61);
        assert_bits_eq(
            &gaussian_blur_grid(&tall, 1.5),
            &two_buffer_blur(&tall, 1.5),
        );
    }

    #[test]
    fn blurring_through_a_mask_matches_blurring_a_masked_copy() {
        let heights = wobbly(41, 33);
        let masked: Vec<Vec<bool>> = (0..33)
            .map(|y| (0..41).map(|x| (x * 7 + y * 3) % 5 == 0).collect())
            .collect();
        let materialised: Vec<Vec<f64>> = heights
            .iter()
            .zip(masked.iter())
            .map(|(hr, mr)| {
                hr.iter()
                    .zip(mr.iter())
                    .map(|(&v, &m)| if m { f64::NAN } else { v })
                    .collect()
            })
            .collect();
        assert_bits_eq(
            &gaussian_blur_heights_masked(&heights, &masked, 3.0, &|_| {}),
            &gaussian_blur_grid(&materialised, 3.0),
        );
    }

    #[test]
    fn the_reported_mask_blur_returns_the_same_grid_and_a_monotone_fraction() {
        let lc: Vec<Vec<u8>> = (0..29)
            .map(|y| {
                (0..37)
                    .map(|x| {
                        if (x + y) % 3 == 0 {
                            LC_BUILT_UP
                        } else {
                            LC_GRASSLAND
                        }
                    })
                    .collect()
            })
            .collect();
        let seen = std::cell::RefCell::new(Vec::new());
        let got = gaussian_blur_mask_to_f32_reported(&lc, LC_BUILT_UP, 37, 29, 2.0, &|f| {
            seen.borrow_mut().push(f)
        });
        let want = gaussian_blur_mask_to_f32(&lc, LC_BUILT_UP, 37, 29, 2.0);
        assert_eq!(got, want);
        let seen = seen.into_inner();
        assert!(seen.windows(2).all(|p| p[0] <= p[1]), "{seen:?}");
        assert_eq!(seen.last().copied(), Some(1.0));
    }

    #[test]
    fn a_nan_free_grid_comes_back_untouched() {
        let g = wobbly(19, 17);
        let mut filled = g.clone();
        fill_nan_values(&mut filled);
        assert_bits_eq(&filled, &g);
    }

    #[test]
    fn a_nan_blob_wider_than_one_dilation_ring_is_filled_from_the_previous_pass() {
        let mut g = vec![vec![10.0; 13]; 13];
        for row in g.iter_mut().take(9).skip(4) {
            for c in row.iter_mut().take(9).skip(4) {
                *c = f64::NAN;
            }
        }
        fill_nan_values(&mut g);
        assert!(g.iter().flatten().all(|v| *v == 10.0));
    }

    #[test]
    fn a_lone_island_survives_the_elevation_gate() {
        let mut g = vec![vec![0.0f64; 100]; 100];
        for row in g.iter_mut().take(60).skip(40) {
            for c in row.iter_mut().take(60).skip(40) {
                *c = 900.0;
            }
        }
        filter_elevation_outliers(&mut g);
        assert_eq!(g[50][50], 900.0);
        assert_eq!(g[0][0], 0.0);
    }

    #[test]
    fn only_physically_impossible_elevations_are_gated() {
        let mut g = vec![vec![100.0f64; 21]; 21];
        g[2][2] = -9999.0;
        g[2][18] = 1.0e38;
        g[18][2] = -600.0;
        g[18][18] = 9500.0;
        // Real extremes, well outside any IQR band this grid could produce.
        g[4][10] = -450.0;
        g[16][10] = 8800.0;

        filter_elevation_outliers(&mut g);

        for (y, x) in [(2, 2), (2, 18), (18, 2), (18, 18)] {
            assert_eq!(g[y][x], 100.0, "({x},{y}) should have been interpolated");
        }
        assert_eq!(g[4][10], -450.0);
        assert_eq!(g[16][10], 8800.0);
    }

    /// 41x41 plateau at 100 m with a centred square tower of `width` cells at 180 m.
    fn plateau_with_tower(width: usize) -> Vec<Vec<f64>> {
        let mut g = vec![vec![100.0f64; 41]; 41];
        let lo = 20 - width / 2;
        for row in g.iter_mut().skip(lo).take(width) {
            for c in row.iter_mut().skip(lo).take(width) {
                *c = 180.0;
            }
        }
        g
    }

    #[test]
    fn anomaly_repair_keeps_its_six_metre_deviation_at_block_resolution() {
        let spike = |d: f64| {
            let mut g = vec![vec![100.0f64; 21]; 21];
            g[10][10] += d;
            g
        };
        for m_per_cell in [1.0, 4.0] {
            let mut kept = spike(5.0);
            repair_terrain_anomalies(&mut kept, m_per_cell);
            assert_eq!(kept[10][10], 105.0, "at {m_per_cell} m/cell");

            let mut repaired = spike(7.0);
            repair_terrain_anomalies(&mut repaired, m_per_cell);
            assert_eq!(repaired[10][10], 100.0, "at {m_per_cell} m/cell");
        }
    }

    #[test]
    fn anomaly_repair_raises_its_deviation_threshold_with_the_cell_size() {
        let spike = |d: f64| {
            let mut g = vec![vec![100.0f64; 21]; 21];
            g[10][10] += d;
            g
        };
        // 40 m/cell puts the gate at 0.25 * 40 = 10 m.
        let mut kept = spike(7.0);
        repair_terrain_anomalies(&mut kept, 40.0);
        assert_eq!(kept[10][10], 107.0);

        let mut repaired = spike(12.0);
        repair_terrain_anomalies(&mut repaired, 40.0);
        assert_eq!(repaired[10][10], 100.0);
    }

    /// The repair without its range shortcut: every cell takes both selects.
    fn repair_by_medians(heights: &mut [Vec<f64>], m_per_cell: f64) {
        let (h, w) = (heights.len(), heights[0].len());
        let thr = 6.0f64.max(0.25 * m_per_cell);
        let passes = if m_per_cell > 4.0 { 2 } else { 10 };
        for _ in 0..passes {
            let snap = heights.to_vec();
            let mut repaired = 0;
            for y in 2..h - 2 {
                for x in 2..w - 2 {
                    let center = snap[y][x];
                    if !center.is_finite() {
                        continue;
                    }
                    let mut nb: Vec<f64> = (-2i32..=2)
                        .flat_map(|dy| (-2i32..=2).map(move |dx| (dy, dx)))
                        .filter(|&d| d != (0, 0))
                        .map(|(dy, dx)| snap[(y as i32 + dy) as usize][(x as i32 + dx) as usize])
                        .filter(|v| v.is_finite())
                        .collect();
                    if nb.len() < 8 {
                        continue;
                    }
                    let mid = nb.len() / 2;
                    nb.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
                    let median = nb[mid];
                    let mut dev: Vec<f64> = nb.iter().map(|v| (v - median).abs()).collect();
                    let dmid = dev.len() / 2;
                    dev.select_nth_unstable_by(dmid, |a, b| a.partial_cmp(b).unwrap());
                    let d = (center - median).abs();
                    if d > thr && d > 3.0 * dev[dmid].max(1.0) {
                        heights[y][x] = median;
                        repaired += 1;
                    }
                }
            }
            if repaired == 0 {
                break;
            }
        }
    }

    #[test]
    fn the_range_shortcut_repairs_exactly_what_the_medians_do() {
        // Gentle wobble with spikes, pits, a NaN hole and a real cliff.
        let mut base = wobbly(60, 50);
        for (y, row) in base.iter_mut().enumerate() {
            for (x, v) in row.iter_mut().enumerate() {
                *v = *v * 0.004 + 100.0 + if x > 40 { 30.0 } else { 0.0 };
                if (x * 7 + y * 13) % 53 == 0 {
                    *v += if x % 2 == 0 { 25.0 } else { -18.0 };
                }
                if (20..23).contains(&x) && (10..12).contains(&y) {
                    *v = f64::NAN;
                }
            }
        }
        for m_per_cell in [1.0, 40.0] {
            let mut got = base.clone();
            let mut want = base.clone();
            repair_terrain_anomalies(&mut got, m_per_cell);
            repair_by_medians(&mut want, m_per_cell);
            assert_bits_eq(&got, &want);
            let changed = got
                .iter()
                .flatten()
                .zip(base.iter().flatten())
                .filter(|(a, b)| a.to_bits() != b.to_bits())
                .count();
            assert!(changed > 0, "the fixture has spikes to repair");
        }
    }

    #[test]
    fn anomaly_repair_stops_eroding_landforms_when_a_cell_is_tens_of_metres() {
        let mut fine = plateau_with_tower(9);
        repair_terrain_anomalies(&mut fine, 1.0);
        assert_eq!(fine[20][20], 100.0, "block resolution must erode as before");

        let mut coarse = plateau_with_tower(9);
        repair_terrain_anomalies(&mut coarse, 24.4);
        assert_eq!(coarse[20][20], 180.0, "a 220 m landform must survive");
    }
}

#[cfg(test)]
mod affine_policy_tests {
    use super::*;

    fn legacy(grid: &[Vec<f64>], ground_level: i32, upper: f64) -> Vec<Vec<f64>> {
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for &h in grid.iter().flatten() {
            if h.is_finite() {
                lo = lo.min(h);
                hi = hi.max(h);
            }
        }
        let range = hi - lo;
        grid.iter()
            .map(|row| {
                row.iter()
                    .map(|&h| {
                        let rel = if range > 0.0 { (h - lo) / range } else { 0.0 };
                        (ground_level as f64 + rel * range).clamp(ground_level as f64, upper)
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_default_mapping_is_bit_identical_to_the_legacy_one() {
        let grid: Vec<Vec<f64>> = (0..64)
            .map(|z| {
                (0..64)
                    .map(|x| 412.37 + (x as f64 * 0.731).sin() * 9.13 + z as f64 * 0.217)
                    .collect()
            })
            .collect();
        let (mc, _) = scale_to_minecraft_with(&grid, 1.0, -62, -62, false, 0, AffinePolicy::Fit);
        let upper = (MAX_Y - TERRAIN_HEIGHT_BUFFER) as f64;
        let want = legacy(&grid, -62, upper);
        for (a, b) in mc.iter().flatten().zip(want.iter().flatten()) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
        let mut flat = vec![vec![300.0; 4]; 4];
        flat[1][2] = f64::NAN;
        let (mc, _) = scale_to_minecraft_with(&flat, 1.0, -62, -62, false, 0, AffinePolicy::Fit);
        assert!(mc.iter().flatten().all(|&y| y == -62.0));
    }

    fn ramp() -> Vec<Vec<f64>> {
        (0..4)
            .map(|z| (0..4).map(|x| 500.0 + (z * 4 + x) as f64).collect())
            .collect()
    }

    #[test]
    fn fit_puts_the_lowest_cell_on_the_ground_level() {
        let (mc, affine) =
            scale_to_minecraft_with(&ramp(), 1.0, -62, -62, false, 0, AffinePolicy::Fit);
        assert_eq!(mc[0][0], -62.0);
        assert_eq!(mc[3][3], -62.0 + 15.0);
        assert_eq!(affine.min_height_m, 500.0);
        assert_eq!(affine.blocks_per_meter, 1.0);
        assert_eq!(affine.ground_level, -62);
    }

    #[test]
    fn headroom_lifts_the_first_area_and_keeps_the_mapping_invertible() {
        let (mc, affine) = scale_to_minecraft_with(
            &ramp(),
            1.0,
            -62,
            -62,
            false,
            0,
            AffinePolicy::FitWithHeadroom,
        );
        assert_eq!(mc[0][0], -62.0 + 96.0);
        assert!((affine.min_height_m - (500.0 - 96.0)).abs() < 1e-9);
        assert_eq!(affine.y_for_metres(500.0), -62.0 + 96.0);
        assert_eq!(affine.y_for_metres(515.0), mc[3][3]);
    }

    #[test]
    fn a_flat_first_area_still_gets_a_slope_for_its_neighbours() {
        let flat = vec![vec![420.0; 4]; 4];
        let (_, affine) = scale_to_minecraft_with(
            &flat,
            2.0,
            -62,
            -62,
            false,
            0,
            AffinePolicy::FitWithHeadroom,
        );
        assert_eq!(affine.blocks_per_meter, 2.0);
        assert!((affine.min_height_m - (420.0 - 48.0)).abs() < 1e-9);
    }

    #[test]
    fn a_fixed_mapping_is_reused_and_clamped() {
        let stored = ElevationAffine {
            min_height_m: 404.0,
            blocks_per_meter: 1.0,
            ground_level: -62,
            soft_top: None,
        };
        let mut grid = ramp();
        grid[0][0] = 300.0; // below the world's lowest: clamps at the base
        grid[3][3] = 2000.0; // above the ceiling: clamps below it
        grid[1][1] = f64::NAN;
        let (mc, affine) =
            scale_to_minecraft_with(&grid, 1.0, -62, -62, false, 0, AffinePolicy::Fixed(stored));
        assert_eq!(affine, stored);
        assert_eq!(mc[0][0], -62.0);
        assert_eq!(mc[3][3], (MAX_Y - TERRAIN_HEIGHT_BUFFER) as f64);
        assert!(mc[1][1].is_nan());
        assert_eq!(mc[0][1], -62.0 + (501.0 - 404.0));
    }

    #[test]
    fn the_whole_earth_fits_one_to_one_up_to_the_soft_top() {
        let e = ElevationAffine::whole_earth(1.0, -2014, 2031);
        let top = e.soft_top.unwrap();
        assert_eq!(top.knee_m, 2800.0);
        assert_eq!(e.y_for_metres(-430.0), -2014.0);
        assert_eq!(e.y_for_metres(0.0), -1584.0);
        assert_eq!(e.y_for_metres(2500.0), 916.0);
        assert!((e.y_for_metres(8849.0) - 2016.0).abs() < 1e-6);

        // Continuous with the same slope at the knee, and still rising at Everest.
        let below = e.y_for_metres(top.knee_m - 0.01);
        let above = e.y_for_metres(top.knee_m + 0.01);
        assert!((above - below - 0.02).abs() < 1e-6);
        let mut last = f64::MIN;
        for h in (0..=8849).step_by(7) {
            let y = e.y_for_metres(h as f64);
            assert!(y > last);
            last = y;
        }
        assert!(e.y_for_metres(8849.0) - e.y_for_metres(8800.0) > 1.0);
    }

    #[test]
    fn the_whole_earth_needs_no_soft_top_when_it_fits() {
        let e = ElevationAffine::whole_earth(0.4, -2014, 2031);
        assert_eq!(e.soft_top, None);
        assert!(e.y_for_metres(8849.0) <= 2016.0);
    }

    #[test]
    fn a_whole_earth_mapping_only_clamps_the_sea_floor() {
        let stored = ElevationAffine::whole_earth(1.0, -2014, 2031);
        let grid = vec![vec![-1500.0, -10.0, 520.0], vec![3000.0, 4808.0, 8849.0]];
        let (mc, _) = scale_to_minecraft_with(
            &grid,
            1.0,
            -62,
            -2030,
            true,
            2031,
            AffinePolicy::Fixed(stored),
        );
        assert_eq!(mc[0][0], -2014.0);
        assert_eq!(mc[0][1], -1594.0);
        assert_eq!(mc[0][2], -1064.0);
        assert!(mc[1][0] < mc[1][1] && mc[1][1] < mc[1][2] && mc[1][2] <= 2016.0);
    }
}
