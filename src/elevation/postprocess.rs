use crate::land_cover::{LandCoverData, LC_BUILT_UP, LC_WATER};
use rayon::prelude::*;
use std::collections::VecDeque;

/// Maximum Y coordinate in Minecraft (vanilla build height limit).
const MAX_Y: i32 = 319;

/// Buffer at the top for buildings, trees, and other structures
const TERRAIN_HEIGHT_BUFFER: i32 = 15;

/// Largest water component a steep-slope shadow blob can be. Real bodies are bigger.
const MAX_STEEP_WATER_AREA_M2: f64 = 250_000.0;

/// Median slope above which small water is shadow, not water. 19 degrees.
const MIN_STEEP_WATER_SLOPE: f64 = 0.35;

/// How far below a cell the ground a step away must sit for that cell to count as perched.
const STEEP_WATER_LAND_BELOW_M: f64 = 2.0;

/// Share of a component's edge cells that must be perched. Only a blob's downhill side
/// is, so this is low; the slope gate above is what does the separating.
const MIN_PERCHED_FRACTION: f64 = 0.10;

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
pub fn repair_terrain_anomalies(heights: &mut [Vec<f64>]) {
    let grid_h = heights.len();
    if grid_h < 5 {
        return;
    }
    let grid_w = heights[0].len();
    if grid_w < 5 {
        return;
    }

    const RADIUS: i32 = 2; // 5x5 window (24 neighbors)
    const PASSES: usize = 10; // max passes; early-break when no more anomalies found
    const ABS_THRESHOLD: f64 = 6.0; // minimum deviation in meters
    const RELATIVE_FACTOR: f64 = 3.0; // deviation must exceed this × MAD

    let r = RADIUS as usize;
    // Reuse the snapshot buffer across passes (saves ~128 MB/pass of allocs
    // on a 4096² grid). The inner `clone_from` copies in place.
    let mut snapshot: Vec<Vec<f64>> = heights.to_vec();
    let mut total_repaired = 0usize;
    let mut passes_ran = 0usize;

    for pass in 0..PASSES {
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
                        for dy in -RADIUS..=RADIUS {
                            for dx in -RADIUS..=RADIUS {
                                if dy == 0 && dx == 0 {
                                    continue;
                                }
                                let v = snapshot_ref[(y as i32 + dy) as usize]
                                    [(x as i32 + dx) as usize];
                                if v.is_finite() {
                                    neighbors.push(v);
                                }
                            }
                        }
                        if neighbors.len() < 8 {
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
                        if deviation > ABS_THRESHOLD && deviation > RELATIVE_FACTOR * mad.max(1.0) {
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
/// This runs after the general MAD/IQR cleanup to target artifacts that are
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
/// The returned bool grid marks which cells actually became water surface,
/// so the coastal pull-down and Gaussian source-masking operate on the
/// real water surface rather than the ESA classification.
fn level_water_surfaces(
    heights: &mut [Vec<f64>],
    lc_grid: &[Vec<u8>],
    m_per_cell: f64,
) -> Vec<Vec<bool>> {
    // Cells up to this many metres above the estimated surface are still
    // treated as water (covers noise / wave chop / 10 m ESA mixed-pixel
    // bleed). Beyond this they are real walls and kept as terrain.
    const WATER_UP_TOLERANCE_M: f64 = 2.0;
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
    let mut visited = vec![vec![false; w]; h];
    let mut is_water_surface = vec![vec![false; w]; h];

    // Snapshot for reading so local-median / mode / clamp computations never
    // see already-mutated heights from the current pass.
    let heights_snapshot: Vec<Vec<f64>> = heights.to_vec();

    let mut components_leveled = 0usize;
    let mut still_components = 0usize;
    let mut flowing_components = 0usize;
    let mut cells_leveled = 0usize;
    let mut cells_skipped = 0usize;
    let mut max_flowing_iqr = 0.0f64;
    // Flattened after the scan, once smoothed: (x, y, local surface), per component.
    let mut flowing_cells: Vec<Vec<(u32, u32, f32)>> = Vec::new();

    for start_y in 0..h {
        for start_x in 0..w {
            if visited[start_y][start_x] || lc_grid[start_y][start_x] != LC_WATER {
                continue;
            }

            // Flood-fill this water component (4-connected).
            let mut component: Vec<(usize, usize)> = Vec::new();
            let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
            queue.push_back((start_x, start_y));
            visited[start_y][start_x] = true;

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
                    if !visited[nyu][nxu] && lc_grid[nyu][nxu] == LC_WATER {
                        visited[nyu][nxu] = true;
                        queue.push_back((nxu, nyu));
                    }
                }
            }

            // Collect finite elevations.
            let values: Vec<f64> = component
                .iter()
                .filter_map(|&(x, y)| {
                    let v = heights_snapshot[y][x];
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

            let fallback_median = {
                let mut v = values.clone();
                let mid = v.len() / 2;
                v.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
                v[mid]
            };

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
                flowing_cells.push(Vec::new());
                for &(cx, cy) in &component {
                    let orig = heights_snapshot[cy][cx];
                    if !orig.is_finite() {
                        continue;
                    }
                    let local_surface = local_water_median(
                        &heights_snapshot,
                        lc_grid,
                        cx,
                        cy,
                        LOCAL_SURFACE_RADIUS,
                        MIN_LOCAL_SAMPLES,
                    )
                    .unwrap_or(fallback_median);
                    let last = flowing_cells.last_mut().expect("component pushed above");
                    last.push((cx as u32, cy as u32, local_surface as f32));
                }
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
                let surface =
                    clamp_by_adjacent_land(raw_surface, &component, &heights_snapshot, lc_grid);

                for &(cx, cy) in &component {
                    let orig = heights_snapshot[cy][cx];
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
            }

            components_leveled += 1;
        }
    }

    // Smooth the local-median surface, then flatten as still water does.
    // The scan is done, so the scratch grids go before the blur allocates.
    drop(heights_snapshot);
    drop(visited);
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
            let flatten = at_or_below || !has_non_water_neighbor(lc_grid, cx, cy);
            if flatten {
                heights[cy][cx] = surface;
                is_water_surface[cy][cx] = true;
                cells_leveled += 1;
            } else {
                cells_skipped += 1;
            }
        }
    }

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

/// Return the median elevation of water cells within `radius` of `(cx, cy)`,
/// or `None` if fewer than `min_samples` finite water heights are in range.
///
/// Used by the flowing-water path in `level_water_surfaces` to build a
/// per-cell water surface that follows the river's gradient at scales
/// longer than the radius, while still averaging out local DSM noise.
fn local_water_median(
    heights: &[Vec<f64>],
    lc_grid: &[Vec<u8>],
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
            if lc_grid[ny as usize][nx as usize] != LC_WATER {
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
/// sample per adjacent cell — dedup'd via HashSet) and taking the lower of
/// that and the proposed surface.
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
) -> f64 {
    let h = heights.len();
    if h == 0 {
        return proposed;
    }
    let w = heights[0].len();

    let mut seen = std::collections::HashSet::with_capacity(component.len());
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
            if !seen.insert((nxu, nyu)) {
                continue;
            }
            let v = heights[nyu][nxu];
            if v.is_finite() {
                adjacent_land.push(v);
            }
        }
    }

    if adjacent_land.is_empty() {
        return proposed;
    }

    let p25_idx = (adjacent_land.len() / 4).min(adjacent_land.len() - 1);
    adjacent_land.select_nth_unstable_by(p25_idx, |a, b| a.partial_cmp(b).unwrap());
    let land_p25 = adjacent_land[p25_idx];

    proposed.min(land_p25)
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
    // Gradient step: about one ESA pixel, so DEM noise finer than the
    // classification cannot masquerade as slope.
    let step = ((10.0 / m_per_cell).round() as usize).clamp(1, 16) as i64;
    let max_cells = (MAX_STEEP_WATER_AREA_M2 / (m_per_cell * m_per_cell)) as usize;
    // Slope of the claimed water surface, sampled between water cells only. Sampling the
    // terrain instead would read the canyon walls across any channel narrower than the step.
    // The farthest water within the step is used, so a patch smaller than the step still
    // gets measured rather than falling through unjudged.
    let cell_slope = |x: usize, y: usize| -> Option<f64> {
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
                if lc_grid[ny as usize][nx as usize] != LC_WATER {
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

    // Binary built-up mask (1.0 = built-up, 0.0 = everything else).
    let mask: Vec<Vec<f64>> = lc_grid
        .par_iter()
        .map(|row| {
            row.iter()
                .map(|&c| if c == LC_BUILT_UP { 1.0 } else { 0.0 })
                .collect()
        })
        .collect();

    // Blur the mask itself -> feathered weights with a smooth 0..1 falloff
    // across the built-up boundary. Without this we'd get a visible seam.
    // Mask blur is the first half of this step's progress, heights blur the second.
    let feathered_mask = gaussian_blur_grid_reported(&mask, sigma_cells, &|f| report(0.5 * f));
    drop(mask);

    // Build the source for the heights blur with *water-surface* cells set
    // to NaN so they don't contribute. Without this the blur averages water
    // (low) into nearby built-up cells and produces a visible "rising ramp"
    // from water into the city — the coastal artifact we already fix with
    // the explicit pull-down pass. Using is_water_surface (not LC_WATER)
    // means canyon wall cells misclassified as water still contribute like
    // the terrain they actually are.
    let heights_for_blur: Vec<Vec<f64>> = heights
        .par_iter()
        .zip(is_water_surface.par_iter())
        .map(|(h_row, ws_row)| {
            h_row
                .iter()
                .zip(ws_row.iter())
                .map(|(&v, &is_ws)| if is_ws { f64::NAN } else { v })
                .collect()
        })
        .collect();
    let blurred_heights =
        gaussian_blur_grid_reported(&heights_for_blur, sigma_cells, &|f| report(0.5 + 0.5 * f));
    drop(heights_for_blur);

    // Blend through the feathered mask. Water-surface cells are skipped so
    // the leveled water surface from the previous pass survives intact.
    let mut total_influenced = 0usize;
    for y in 0..h {
        for x in 0..w {
            if is_water_surface[y][x] {
                continue;
            }
            let m = feathered_mask[y][x].clamp(0.0, 1.0);
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

    // ~10 chunks per pass: enough to animate the bar, few enough that the extra
    // rayon barriers cost nothing measurable.
    const CHUNKS: usize = 10;

    // Horizontal pass — rows are independent.
    let row_chunk = h.div_ceil(CHUNKS);
    let mut after_h: Vec<Vec<f64>> = Vec::with_capacity(h);
    for rows in grid.chunks(row_chunk) {
        let mut part: Vec<Vec<f64>> = rows
            .par_iter()
            .map(|row| {
                let row_len = row.len() as i32;
                (0..row.len())
                    .map(|i| {
                        let mut sum = 0.0;
                        let mut wsum = 0.0;
                        for (j, &k) in kernel.iter().enumerate() {
                            let idx = i as i32 + j as i32 - half;
                            if idx >= 0 && idx < row_len {
                                let v = row[idx as usize];
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
            })
            .collect();
        after_h.append(&mut part);
        report(0.5 * (after_h.len() as f64 / h as f64));
    }

    // Vertical pass — columns are independent. Work column-at-a-time to keep
    // memory access sequential within each parallel task.
    let col_chunk = w.div_ceil(CHUNKS);
    let mut out: Vec<Vec<f64>> = vec![vec![0.0; w]; h];
    let mut x0 = 0usize;
    while x0 < w {
        let x1 = (x0 + col_chunk).min(w);
        let blurred: Vec<(usize, Vec<f64>)> = (x0..x1)
            .into_par_iter()
            .map(|x| {
                let column: Vec<f64> = after_h.iter().map(|row| row[x]).collect();
                let col_len = column.len() as i32;
                let col: Vec<f64> = (0..column.len())
                    .map(|y| {
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
                            sum / wsum
                        } else {
                            f64::NAN
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

    let mut changes_made: bool = true;
    while changes_made {
        let snapshot: Vec<Vec<f64>> = height_grid.to_vec();
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

        changes_made = any_changed;
    }
}

/// Filter extreme elevation outliers using IQR-based detection.
/// Uses 3× the interquartile range beyond Q1/Q3 to identify true outliers
/// (corrupted data, sea-floor artifacts) without clipping real terrain on
/// mountains or deep valleys.
///
/// A count guard prevents filtering when >5% of values fall outside the bounds,
/// which indicates bimodal terrain (e.g., deep canyons) rather than corruption.
pub fn filter_elevation_outliers(height_grid: &mut [Vec<f64>]) {
    let height = height_grid.len();
    if height == 0 {
        return;
    }
    let width = height_grid[0].len();

    // Collect finite heights in parallel — flat-mapping per row, each thread
    // builds its own Vec, then rayon stitches the segments together. Avoids
    // a single sequential sweep over the whole grid.
    let mut all_heights: Vec<f64> = height_grid
        .par_iter()
        .flat_map_iter(|row| row.iter().filter(|h| !h.is_nan() && h.is_finite()).copied())
        .collect();

    if all_heights.len() < 4 {
        return;
    }

    let len = all_heights.len();
    let q1_idx = len / 4;
    let q3_idx = (len * 3) / 4;

    let (_, q1_val, _) =
        all_heights.select_nth_unstable_by(q1_idx, |a, b| a.partial_cmp(b).unwrap());
    let q1 = *q1_val;

    let (_, q3_val, _) =
        all_heights.select_nth_unstable_by(q3_idx, |a, b| a.partial_cmp(b).unwrap());
    let q3 = *q3_val;

    let iqr = q3 - q1;
    let min_reasonable = q1 - 3.0 * iqr;
    let max_reasonable = q3 + 3.0 * iqr;

    // Count guard: if >5% of values fall outside a bound, that tail represents
    // real terrain (e.g., canyon floor), not corrupted data — skip that bound.
    let (below_count, above_count) = all_heights
        .par_iter()
        .map(|&h| ((h < min_reasonable) as usize, (h > max_reasonable) as usize))
        .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    let threshold = (len as f64 * 0.05) as usize;
    let filter_lower = below_count > 0 && below_count <= threshold;
    let filter_upper = above_count > 0 && above_count <= threshold;

    if !filter_lower && !filter_upper {
        return;
    }

    // Per-row NaN-out, then sum the per-row counts back together. Each row
    // is mutated independently so this is data-race-free.
    let outliers_filtered: usize = height_grid
        .par_iter_mut()
        .take(height)
        .map(|row| {
            let mut row_count = 0usize;
            for h in row.iter_mut().take(width) {
                if !h.is_nan() {
                    let is_outlier = (filter_lower && *h < min_reasonable)
                        || (filter_upper && *h > max_reasonable);
                    if is_outlier {
                        *h = f64::NAN;
                        row_count += 1;
                    }
                }
            }
            row_count
        })
        .sum();

    if outliers_filtered > 0 {
        eprintln!(
            "Filtered {} extreme outliers (IQR bounds: {:.1}m to {:.1}m, lower={}, upper={})",
            outliers_filtered, min_reasonable, max_reasonable, filter_lower, filter_upper
        );
        fill_nan_values(height_grid);
    }
}

/// Scale raw elevation (meters) to Minecraft Y coordinates, keeping f64 precision.
/// `extended_max_y` is the cap when `disable_height_limit` is on (Java datapack:
/// 2031; Bedrock BP: 512); ignored otherwise.
/// Scales real-world metre heights to Minecraft Y. Also returns the affine
/// parameters `(min_height_m, blocks_per_meter)` so a real-world elevation can
/// be converted back to a Minecraft Y threshold (e.g. for the snow line), plus the
/// terrain base actually used (see `min_ground_level`).
///
/// `min_ground_level` is the lowest base the terrain may sink to. The base only sinks when
/// the relief genuinely does not fit above `ground_level`, so a small bbox is not dropped
/// into the basement just because the world floor was extended.
pub fn scale_to_minecraft(
    blurred_heights: &[Vec<f64>],
    scale: f64,
    ground_level: i32,
    min_ground_level: i32,
    disable_height_limit: bool,
    extended_max_y: i32,
) -> (Vec<Vec<f64>>, f64, f64, i32) {
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

    let effective_max_y = if disable_height_limit {
        extended_max_y
    } else {
        MAX_Y
    };
    let upper_clamp = (effective_max_y - TERRAIN_HEIGHT_BUFFER) as f64;

    let ideal_scaled_range: f64 = height_range * scale;
    let ceiling = effective_max_y - TERRAIN_HEIGHT_BUFFER;

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

    let mc_heights: Vec<Vec<f64>> = blurred_heights
        .par_iter()
        .map(|row| {
            row.iter()
                .map(|&h| {
                    let relative_height: f64 = if height_range > 0.0 {
                        (h - min_height) / height_range
                    } else {
                        0.0
                    };
                    let scaled_height: f64 = relative_height * scaled_range;
                    let mc_y = ground_level as f64 + scaled_height;
                    mc_y.clamp(ground_level as f64, upper_clamp)
                })
                .collect()
        })
        .collect();

    let blocks_per_meter = if height_range > 0.0 {
        scaled_range / height_range
    } else {
        0.0
    };
    (mc_heights, min_height, blocks_per_meter, ground_level)
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
}
