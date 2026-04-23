use crate::land_cover::{LandCoverData, LC_BUILT_UP, LC_WATER};
use rayon::prelude::*;
use std::collections::VecDeque;

/// Maximum Y coordinate in Minecraft (vanilla build height limit).
const MAX_Y: i32 = 319;

/// Buffer at the top for buildings, trees, and other structures
const TERRAIN_HEIGHT_BUFFER: i32 = 15;

/// Repair terrain anomalies (LiDAR classification errors, tile seams, provider glitches).
///
/// Uses a 5x5 median-based filter with MAD (median absolute deviation) to detect
/// outliers while preserving real terrain features like mountain ridges and canyons.
/// Runs iteratively so that multi-pixel artifact clusters are eroded from the outside
/// in — each pass fixes boundary pixels that have enough normal neighbors.
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
            for (dst, src) in snapshot.iter_mut().zip(heights.iter()) {
                dst.clone_from(src);
            }
        }

        let mut repaired = 0;
        let mut neighbors: Vec<f64> = Vec::with_capacity(24);
        let mut abs_devs: Vec<f64> = Vec::with_capacity(24);

        for y in r..grid_h - r {
            for x in r..grid_w - r {
                let center = snapshot[y][x];
                if !center.is_finite() {
                    continue;
                }

                // Collect finite neighbors in the 5x5 window.
                neighbors.clear();
                for dy in -RADIUS..=RADIUS {
                    for dx in -RADIUS..=RADIUS {
                        if dy == 0 && dx == 0 {
                            continue;
                        }
                        let v = snapshot[(y as i32 + dy) as usize][(x as i32 + dx) as usize];
                        if v.is_finite() {
                            neighbors.push(v);
                        }
                    }
                }
                if neighbors.len() < 8 {
                    continue;
                }

                // Median of neighbors — O(n) via select_nth (vs sort O(n log n)).
                let mid = neighbors.len() / 2;
                neighbors.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
                let median = neighbors[mid];

                // MAD (median absolute deviation) — robust scale estimator.
                // High MAD = real terrain variation (slopes, ridges) → large deviations allowed.
                // Low MAD = flat area → even moderate spikes get caught.
                abs_devs.clear();
                abs_devs.extend(neighbors.iter().map(|&v| (v - median).abs()));
                let mad_mid = abs_devs.len() / 2;
                abs_devs.select_nth_unstable_by(mad_mid, |a, b| a.partial_cmp(b).unwrap());
                let mad = abs_devs[mad_mid];

                let deviation = (center - median).abs();
                // Flag as anomaly only if deviation exceeds BOTH:
                // - An absolute floor (prevents fixing gentle slopes)
                // - A multiple of local variation (preserves canyons, mountain ridges)
                if deviation > ABS_THRESHOLD && deviation > RELATIVE_FACTOR * mad.max(1.0) {
                    heights[y][x] = median;
                    repaired += 1;
                }
            }
        }

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

    // Returns a bool grid marking which cells were actually flattened to the
    // water-surface level. Misclassified wall cells inside narrow canyon
    // rivers are skipped, so downstream passes (pull-down BFS, Gaussian
    // source-masking) use the real water surface and not the contaminated
    // classification.
    let is_water_surface = level_water_surfaces(heights, &land_cover.grid);

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
    if reclassified > 0 {
        land_cover.water_distance =
            crate::land_cover::compute_water_distance(&land_cover.grid, grid_w, grid_h);
        // The water-blend smoothing was derived from the pre-reclassify
        // grid — refresh it so the softened shoreline reflects the updated
        // classification.
        land_cover.refresh_water_blend_grid();
    }

    if coastal_pull_distance_cells > 0 {
        pull_coastal_builtup_toward_water(
            heights,
            &land_cover.grid,
            &is_water_surface,
            coastal_pull_distance_cells,
        );
    }
    smooth_built_up_gaussian(
        heights,
        &land_cover.grid,
        &is_water_surface,
        built_up_sigma_cells,
    );
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
/// The returned bool grid marks which cells actually became water surface,
/// so the coastal pull-down and Gaussian source-masking operate on the
/// real water surface rather than the ESA classification.
fn level_water_surfaces(heights: &mut [Vec<f64>], lc_grid: &[Vec<u8>]) -> Vec<Vec<bool>> {
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

                    let at_or_below = orig <= local_surface + WATER_UP_TOLERANCE_M;
                    let flatten = at_or_below || !has_non_water_neighbor(lc_grid, cx, cy);
                    if flatten {
                        heights[cy][cx] = local_surface;
                        is_water_surface[cy][cx] = true;
                        cells_leveled += 1;
                    } else {
                        cells_skipped += 1;
                    }
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

            // Expanding ring search for nearest non-water, non-zero class.
            let mut found: Option<u8> = None;
            'outer: for r in 1..=SEARCH_RADIUS {
                for dy in -r..=r {
                    for dx in -r..=r {
                        // Only sample cells on the ring at distance exactly `r`.
                        if dy.abs() != r && dx.abs() != r {
                            continue;
                        }
                        let nx = x as i32 + dx;
                        let ny = y as i32 + dy;
                        if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                            continue;
                        }
                        let c = lc_grid[ny as usize][nx as usize];
                        if c != LC_WATER && c != 0 {
                            found = Some(c);
                            break 'outer;
                        }
                    }
                }
            }
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

/// Pull built-up cells near water down toward the local water surface.
///
/// DSMs (AWS Terrain Tiles, Copernicus DSM, SRTM) include buildings and
/// waterfront structures in the elevation. In Minecraft that appears as a
/// flat strip of "city" sitting ~10–30 m above the water it's right next to
/// — a cliff at the shoreline. A plain Gaussian blur can't fix this
/// cleanly: with water included in the source it creates a broad ramp
/// instead of a cliff; with water excluded it keeps the cliff.
///
/// This pass walks outward from every water cell (4-connected multi-source
/// BFS bounded at `max_distance`) and, for each built-up cell inside that
/// distance, lerps its elevation toward the nearest water cell's surface
/// level with a **linear distance falloff**:
///
///     weight = (max_distance − distance) / max_distance
///     new    = (1 − weight) · original + weight · water_level
///
/// Net effect: a controlled, known-width ramp from water up to the city
/// interior that replaces the uncontrolled Gaussian-induced ramp.
fn pull_coastal_builtup_toward_water(
    heights: &mut [Vec<f64>],
    lc_grid: &[Vec<u8>],
    is_water_surface: &[Vec<bool>],
    max_distance: u32,
) {
    if max_distance == 0 {
        return;
    }
    let h = heights.len();
    let w = heights[0].len();

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
                // Heights at water-surface cells have been flattened to the
                // component water level, so this is the target we pull
                // coastal built-up cells toward.
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

    // Apply the linear pull-down to built-up cells in range.
    let mut affected = 0usize;
    let denom = max_distance as f64;
    for y in 0..h {
        for x in 0..w {
            if lc_grid[y][x] != LC_BUILT_UP {
                continue;
            }
            let d = dist[y][x];
            if d == 0 || d > max_distance {
                continue;
            }
            let wl = water_level[y][x];
            let orig = heights[y][x];
            if !wl.is_finite() || !orig.is_finite() {
                continue;
            }
            // Linear falloff: weight ≈ 1 at d=1 (right next to water),
            // weight → 0 as d → max_distance.
            let weight = ((max_distance - d) as f64 / denom).clamp(0.0, 1.0);
            heights[y][x] = orig * (1.0 - weight) + wl * weight;
            affected += 1;
        }
    }

    if affected > 0 {
        eprintln!(
            "Land cover repair: pulled {} coastal built-up cells toward water (within {} cells)",
            affected, max_distance
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
    let feathered_mask = gaussian_blur_grid(&mask, sigma_cells);
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
    let blurred_heights = gaussian_blur_grid(&heights_for_blur, sigma_cells);
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

    // Horizontal pass — rows are independent.
    let after_h: Vec<Vec<f64>> = grid
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

    // Vertical pass — columns are independent. Work column-at-a-time to keep
    // memory access sequential within each parallel task.
    let blurred_columns: Vec<Vec<f64>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let column: Vec<f64> = after_h.iter().map(|row| row[x]).collect();
            let col_len = column.len() as i32;
            (0..column.len())
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
                .collect()
        })
        .collect();

    // Transpose columns back to row-major.
    let mut out: Vec<Vec<f64>> = vec![vec![0.0; w]; h];
    for (x, col) in blurred_columns.into_iter().enumerate() {
        for (y, v) in col.into_iter().enumerate() {
            out[y][x] = v;
        }
    }
    out
}

fn create_gaussian_kernel(size: usize, sigma: f64) -> Vec<f64> {
    let mut kernel = vec![0.0; size];
    let center = size as f64 / 2.0;
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
pub fn fill_nan_values(height_grid: &mut [Vec<f64>]) {
    let height: usize = height_grid.len();
    if height == 0 {
        return;
    }
    let width: usize = height_grid[0].len();

    let mut changes_made: bool = true;
    while changes_made {
        changes_made = false;
        let snapshot: Vec<Vec<f64>> = height_grid.to_vec();

        #[allow(clippy::needless_range_loop)]
        for y in 0..height {
            for x in 0..width {
                if height_grid[y][x].is_nan() {
                    let mut sum: f64 = 0.0;
                    let mut count: i32 = 0;

                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let ny: i32 = y as i32 + dy;
                            let nx: i32 = x as i32 + dx;

                            if ny >= 0 && ny < height as i32 && nx >= 0 && nx < width as i32 {
                                let val: f64 = snapshot[ny as usize][nx as usize];
                                if !val.is_nan() {
                                    sum += val;
                                    count += 1;
                                }
                            }
                        }
                    }

                    if count > 0 {
                        height_grid[y][x] = sum / count as f64;
                        changes_made = true;
                    }
                }
            }
        }
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

    let mut all_heights: Vec<f64> = Vec::new();
    for row in height_grid.iter() {
        for &h in row {
            if !h.is_nan() && h.is_finite() {
                all_heights.push(h);
            }
        }
    }

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
    let below_count = all_heights.iter().filter(|&&h| h < min_reasonable).count();
    let above_count = all_heights.iter().filter(|&&h| h > max_reasonable).count();
    let threshold = (len as f64 * 0.05) as usize;
    let filter_lower = below_count > 0 && below_count <= threshold;
    let filter_upper = above_count > 0 && above_count <= threshold;

    if !filter_lower && !filter_upper {
        return;
    }

    let mut outliers_filtered = 0;

    for row in height_grid.iter_mut().take(height) {
        for h in row.iter_mut().take(width) {
            if !h.is_nan() {
                let is_outlier =
                    (filter_lower && *h < min_reasonable) || (filter_upper && *h > max_reasonable);
                if is_outlier {
                    *h = f64::NAN;
                    outliers_filtered += 1;
                }
            }
        }
    }

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
pub fn scale_to_minecraft(
    blurred_heights: &[Vec<f64>],
    scale: f64,
    ground_level: i32,
    disable_height_limit: bool,
    extended_max_y: i32,
) -> Vec<Vec<f64>> {
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

    let (min_height, _max_height, height_range) =
        if !min_height.is_finite() || !max_height.is_finite() || min_height >= max_height {
            (0.0_f64, 0.0_f64, 0.0_f64)
        } else {
            (min_height, max_height, max_height - min_height)
        };

    let effective_max_y = if disable_height_limit {
        extended_max_y
    } else {
        MAX_Y
    };
    let upper_clamp = (effective_max_y - TERRAIN_HEIGHT_BUFFER) as f64;

    let ideal_scaled_range: f64 = height_range * scale;
    let available_y_range: f64 = (effective_max_y - TERRAIN_HEIGHT_BUFFER - ground_level) as f64;

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

    mc_heights
}

#[cfg(test)]
mod tests {
    use super::*;

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
