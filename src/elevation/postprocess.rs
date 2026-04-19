use crate::land_cover::{LandCoverData, LC_BUILT_UP, LC_WATER};
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
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

    for pass in 0..PASSES {
        let snapshot: Vec<Vec<f64>> = heights.to_vec();
        let mut repaired = 0;

        for y in r..grid_h - r {
            for x in r..grid_w - r {
                let center = snapshot[y][x];
                if !center.is_finite() {
                    continue;
                }

                // Collect finite neighbors in the 5x5 window
                let mut neighbors: Vec<f64> = Vec::with_capacity(24);
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

                // Compute median of neighbors
                neighbors.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
                let median = neighbors[neighbors.len() / 2];

                // Compute MAD (median absolute deviation) — robust scale estimator.
                // High MAD = real terrain variation (slopes, ridges) → large deviations allowed.
                // Low MAD = flat area → even moderate spikes get caught.
                let mad: f64 = {
                    let mut abs_devs: Vec<f64> =
                        neighbors.iter().map(|&v| (v - median).abs()).collect();
                    abs_devs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
                    abs_devs[abs_devs.len() / 2]
                };

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

        if repaired > 0 {
            eprintln!(
                "Repaired {} terrain anomalies (pass {}/{})",
                repaired,
                pass + 1,
                PASSES
            );
        } else {
            break; // No more anomalies found
        }
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
    land_cover: &LandCoverData,
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
/// surface. But ESA WorldCover is 10 m resolution: on narrow rivers in
/// steep canyons (Grand Canyon, Swiss valleys, etc.) the pixels straddling
/// the shoreline are mixed water/wall and get snapped to "water" by the
/// classifier. A naïve median over the whole component then drags canyon
/// wall cells down to river level (visible as terrain "cut off") or drags
/// real river cells up toward wall height (visible as "uplifted").
///
/// This pass:
/// - Uses the **25th percentile** of elevations (not the median) as the
///   water-surface estimate. Walls are always *above* water, so a low
///   percentile stays in real-water territory even with substantial
///   wall contamination.
/// - Only overwrites cells whose DSM elevation is within ±2 m of that
///   level. Cells that stick well above are misclassified wall — we leave
///   their DSM value alone and they render as normal terrain. **Terrain
///   wins over water classification**, which is what the user expects.
///
/// The returned bool grid lets downstream passes (coastal pull-down,
/// Gaussian blur source-masking) see the true water surface rather than
/// the ESA classification so misclassified wall cells propagate correctly.
fn level_water_surfaces(heights: &mut [Vec<f64>], lc_grid: &[Vec<u8>]) -> Vec<Vec<bool>> {
    const WATER_LEVEL_PERCENTILE: usize = 25;
    const WATER_LEVEL_TOLERANCE_M: f64 = 2.0;

    let h = heights.len();
    let w = heights[0].len();
    let mut visited = vec![vec![false; w]; h];
    let mut is_water_surface = vec![vec![false; w]; h];
    let mut components_leveled = 0usize;
    let mut cells_leveled = 0usize;
    let mut cells_skipped = 0usize;

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

            // 25th percentile = robust low estimate of the water surface.
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
            let pct_idx = ((values.len() * WATER_LEVEL_PERCENTILE) / 100).min(values.len() - 1);
            values.select_nth_unstable_by(pct_idx, |a, b| a.partial_cmp(b).unwrap());
            let water_level = values[pct_idx];

            // Only flatten cells close to that level. Wall cells stay at
            // their original DSM elevation and render as terrain.
            for &(x, y) in &component {
                let orig = heights[y][x];
                if !orig.is_finite() {
                    continue;
                }
                if (orig - water_level).abs() <= WATER_LEVEL_TOLERANCE_M {
                    heights[y][x] = water_level;
                    is_water_surface[y][x] = true;
                    cells_leveled += 1;
                } else {
                    cells_skipped += 1;
                }
            }
            components_leveled += 1;
        }
    }

    if components_leveled > 0 {
        eprintln!(
            "Land cover repair: leveled {} water component(s), {} surface cells flattened, {} off-surface cells kept as terrain",
            components_leveled, cells_leveled, cells_skipped
        );
    }

    is_water_surface
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
fn gaussian_blur_grid(grid: &[Vec<f64>], sigma: f64) -> Vec<Vec<f64>> {
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
pub fn scale_to_minecraft(
    blurred_heights: &[Vec<f64>],
    scale: f64,
    ground_level: i32,
    disable_height_limit: bool,
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

    let ideal_scaled_range: f64 = height_range * scale;
    let available_y_range: f64 = (MAX_Y - TERRAIN_HEIGHT_BUFFER - ground_level) as f64;

    let scaled_range: f64 = if disable_height_limit {
        eprintln!(
            "Height limit disabled: {:.1}m range => {:.0} blocks (no compression)",
            height_range, ideal_scaled_range
        );
        ideal_scaled_range
    } else if ideal_scaled_range <= available_y_range {
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
                    if disable_height_limit {
                        mc_y
                    } else {
                        mc_y.clamp(ground_level as f64, (MAX_Y - TERRAIN_HEIGHT_BUFFER) as f64)
                    }
                })
                .collect()
        })
        .collect();

    // Warn if terrain exceeds the absolute Minecraft data pack maximum
    const DATA_PACK_MAX_Y: i32 = 2031;
    if disable_height_limit {
        let max_block_height = mc_heights
            .iter()
            .flat_map(|row| row.iter())
            .copied()
            .fold(f64::MIN, f64::max)
            .round() as i32;
        if max_block_height > DATA_PACK_MAX_Y {
            eprintln!(
                "Warning: Terrain peak reaches Y={}, which exceeds the maximum data pack height (Y={}). \
                 Blocks above Y={} will be truncated.",
                max_block_height, DATA_PACK_MAX_Y, DATA_PACK_MAX_Y
            );
            #[cfg(feature = "gui")]
            send_log(
                LogLevel::Warning,
                &format!(
                    "Terrain peak Y={} exceeds data pack max Y={}. Blocks will be truncated.",
                    max_block_height, DATA_PACK_MAX_Y
                ),
            );
        }
    }

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
