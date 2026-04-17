#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use rayon::prelude::*;

/// Maximum Y coordinate in Minecraft (vanilla build height limit).
const MAX_Y: i32 = 319;

/// Buffer at the top for buildings, trees, and other structures
const TERRAIN_HEIGHT_BUFFER: i32 = 15;

/// Repair single-pixel terrain anomalies (tile seams, provider glitches).
/// Only flags pixels whose deviation from the neighbor mean significantly exceeds
/// the local neighbor-to-neighbor variation (i.e. isolated spikes, not slopes).
pub fn repair_terrain_anomalies(heights: &mut [Vec<f64>]) {
    let grid_h = heights.len();
    if grid_h < 3 {
        return;
    }
    let grid_w = heights[0].len();
    if grid_w < 3 {
        return;
    }

    // Snapshot for consistent neighbor reads
    let snapshot: Vec<Vec<f64>> = heights.to_vec();
    let mut repaired = 0;

    for y in 1..grid_h - 1 {
        for x in 1..grid_w - 1 {
            let center = snapshot[y][x];
            if !center.is_finite() {
                continue;
            }
            let mut sum = 0.0;
            let mut min_n = f64::MAX;
            let mut max_n = f64::MIN;
            let mut count = 0;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dy == 0 && dx == 0 {
                        continue;
                    }
                    let v = snapshot[(y as i32 + dy) as usize][(x as i32 + dx) as usize];
                    if v.is_finite() {
                        sum += v;
                        min_n = min_n.min(v);
                        max_n = max_n.max(v);
                        count += 1;
                    }
                }
            }
            if count > 0 {
                let mean = sum / count as f64;
                let deviation = (center - mean).abs();
                let neighbor_range = max_n - min_n;
                // Only repair if deviation from mean exceeds both an absolute
                // threshold AND is much larger than the local neighbor variation.
                // This preserves slopes (high range, proportional deviation) and
                // catches isolated spikes (low range, disproportionate deviation).
                if deviation > 10.0 && deviation > neighbor_range * 2.0 {
                    heights[y][x] = mean;
                    repaired += 1;
                }
            }
        }
    }

    if repaired > 0 {
        eprintln!("Repaired {repaired} terrain anomalies (isolated spikes)");
    }
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
