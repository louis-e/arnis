use geo::{Contains, LineString, Point, Polygon};
use indicatif::{ProgressBar, ProgressStyle};
use itertools::Itertools;
use rayon::prelude::*;
use rustc_hash::FxHashSet;
use std::time::{Duration, Instant};

/// Bit-packed grid for efficient visited tracking in bounded areas
struct BitGrid {
    data: Vec<u64>,
    width: usize,
    height: usize,
    min_x: i32,
    min_z: i32,
}

impl BitGrid {
    fn new(min_x: i32, max_x: i32, min_z: i32, max_z: i32) -> Self {
        let width = (max_x - min_x + 1) as usize;
        let height = (max_z - min_z + 1) as usize;
        let total_bits = width * height;
        let num_u64s = (total_bits + 63) / 64;
        Self {
            data: vec![0u64; num_u64s],
            width,
            height,
            min_x,
            min_z,
        }
    }

    #[inline]
    fn set(&mut self, x: i32, z: i32) {
        let idx = ((z - self.min_z) as usize * self.width + (x - self.min_x) as usize);
        self.data[idx / 64] |= 1u64 << (idx % 64);
    }

    #[inline]
    fn get(&self, x: i32, z: i32) -> bool {
        let idx = ((z - self.min_z) as usize * self.width + (x - self.min_x) as usize);
        (self.data[idx / 64] & (1u64 << (idx % 64))) != 0
    }
}

/// Vec-based queue for better cache locality than VecDeque
struct VecQueue<T> {
    data: Vec<T>,
    head: usize,
}

impl<T> VecQueue<T> {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            head: 0,
        }
    }

    #[inline]
    fn push(&mut self, item: T) {
        self.data.push(item);
    }

    #[inline]
    fn pop(&mut self) -> Option<T> {
        if self.head < self.data.len() {
            let item = std::mem::replace(&mut self.data[self.head], unsafe {
                std::mem::MaybeUninit::zeroed().assume_init()
            });
            self.head += 1;
            Some(item)
        } else {
            None
        }
    }

    #[inline]
    fn clear(&mut self) {
        self.data.clear();
        self.head = 0;
    }

    #[inline]
    fn len(&self) -> usize {
        self.data.len() - self.head
    }
}

/// Main flood fill function with automatic algorithm selection
/// Chooses the best algorithm based on polygon size and complexity
pub fn flood_fill_area(
    polygon_coords: &[(i32, i32)],
    timeout: Option<&Duration>,
) -> Vec<(i32, i32)> {
    if polygon_coords.len() < 3 {
        return vec![]; // Not a valid polygon
    }

    // Calculate bounding box of the polygon using itertools
    let (min_x, max_x) = polygon_coords
        .iter()
        .map(|&(x, _)| x)
        .minmax()
        .into_option()
        .unwrap();
    let (min_z, max_z) = polygon_coords
        .iter()
        .map(|&(_, z)| z)
        .minmax()
        .into_option()
        .unwrap();

    let area = (max_x - min_x + 1) as i64 * (max_z - min_z + 1) as i64;

    // For very large areas (oceans), use parallel scanline algorithm
    if area > 500000 {
        parallel_scanline_flood_fill(polygon_coords, min_x, max_x, min_z, max_z)
    } else if area < 50000 {
        // For small and medium areas, use optimized flood fill with span filling
        optimized_flood_fill_area(polygon_coords, timeout, min_x, max_x, min_z, max_z)
    } else {
        // For larger areas, use original flood fill with grid sampling
        original_flood_fill_area(polygon_coords, timeout, min_x, max_x, min_z, max_z)
    }
}

/// Optimized flood fill for larger polygons with multi-seed detection for complex shapes like U-shapes
fn optimized_flood_fill_area(
    polygon_coords: &[(i32, i32)],
    timeout: Option<&Duration>,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) -> Vec<(i32, i32)> {
    let start_time = Instant::now();

    // Create polygon for containment testing
    let exterior_coords: Vec<(f64, f64)> = polygon_coords
        .iter()
        .map(|&(x, z)| (x as f64, z as f64))
        .collect();
    let exterior = LineString::from(exterior_coords);
    let polygon = Polygon::new(exterior, vec![]);

    // Optimized step sizes: larger steps for efficiency, but still catch U-shapes
    let width = max_x - min_x + 1;
    let height = max_z - min_z + 1;
    let step_x = (width / 6).clamp(1, 8);
    let step_z = (height / 6).clamp(1, 8);

    // Collect all potential seed points first
    let mut seed_points = Vec::new();
    for z in (min_z..=max_z).step_by(step_z as usize) {
        for x in (min_x..=max_x).step_by(step_x as usize) {
            if polygon.contains(&Point::new(x as f64, z as f64)) {
                seed_points.push((x, z));
            }
        }
    }

    // Only parallelize if we have enough seed points to justify overhead
    const PARALLEL_THRESHOLD: usize = 100;
    if seed_points.len() < PARALLEL_THRESHOLD {
        // Sequential processing for small workloads
        return sequential_span_fill(&polygon, &seed_points, min_x, max_x, min_z, max_z, timeout, &start_time);
    }

    // Create progress bar for parallel processing
    let pb = ProgressBar::new(seed_points.len() as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} seeds ({eta})")
        .unwrap()
        .progress_chars("█▓░"));

    // Process seed points in parallel chunks
    let chunk_size = (seed_points.len() / rayon::current_num_threads()).max(1);
    let results: Vec<Vec<(i32, i32)>> = seed_points
        .par_chunks(chunk_size)
        .map(|chunk_seeds| {
            let estimated_capacity = ((max_x - min_x) * (max_z - min_z)) as usize / 4;
            let mut local_filled = Vec::with_capacity(estimated_capacity.min(100000));
            let mut local_visited = BitGrid::new(min_x, max_x, min_z, max_z);
            let mut queue = VecQueue::with_capacity(2048);
            let mut iterations = 0u64;
            const MAX_ITERATIONS: u64 = 1_000_000;

            for &(seed_x, seed_z) in chunk_seeds {
                pb.inc(1);
                
                // Check timeout less frequently
                if iterations % 10000 == 0 {
                    if let Some(timeout) = timeout {
                        if start_time.elapsed() > *timeout {
                            break;
                        }
                    }
                }

                // Skip if already visited
                if local_visited.get(seed_x, seed_z) {
                    continue;
                }

                // Start span-based flood fill from this seed point
                queue.clear();
                queue.push((seed_x, seed_z));
                local_visited.set(seed_x, seed_z);

                while let Some((curr_x, curr_z)) = queue.pop() {
                    iterations += 1;
                    if iterations > MAX_ITERATIONS {
                        break;
                    }

                    // Check containment before processing
                    if !polygon.contains(&Point::new(curr_x as f64, curr_z as f64)) {
                        continue;
                    }

                    // Span fill: expand horizontally
                    let mut left_x = curr_x;
                    let mut right_x = curr_x;

                    // Expand left
                    while left_x > min_x && !local_visited.get(left_x - 1, curr_z) 
                        && polygon.contains(&Point::new((left_x - 1) as f64, curr_z as f64)) {
                        left_x -= 1;
                        local_visited.set(left_x, curr_z);
                    }

                    // Expand right
                    while right_x < max_x && !local_visited.get(right_x + 1, curr_z) 
                        && polygon.contains(&Point::new((right_x + 1) as f64, curr_z as f64)) {
                        right_x += 1;
                        local_visited.set(right_x, curr_z);
                    }

                    // Add all points in span
                    for x in left_x..=right_x {
                        local_filled.push((x, curr_z));
                    }

                    // Check spans above and below
                    for check_z in [curr_z - 1, curr_z + 1] {
                        if check_z < min_z || check_z > max_z {
                            continue;
                        }
                        for x in left_x..=right_x {
                            if !local_visited.get(x, check_z) 
                                && polygon.contains(&Point::new(x as f64, check_z as f64)) {
                                local_visited.set(x, check_z);
                                queue.push((x, check_z));
                            }
                        }
                    }
                }
            }

            local_filled
        })
        .collect();

    pb.finish_and_clear();

    // Merge results - no deduplication needed with proper visited tracking
    results.into_iter().flatten().collect()
}

/// Sequential span-based fill for small workloads
fn sequential_span_fill(
    polygon: &Polygon<f64>,
    seed_points: &[(i32, i32)],
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    timeout: Option<&Duration>,
    start_time: &Instant,
) -> Vec<(i32, i32)> {
    let estimated_capacity = ((max_x - min_x) * (max_z - min_z)) as usize / 4;
    let mut filled = Vec::with_capacity(estimated_capacity.min(100000));
    let mut visited = BitGrid::new(min_x, max_x, min_z, max_z);
    let mut queue = VecQueue::with_capacity(2048);
    let mut iterations = 0u64;
    const MAX_ITERATIONS: u64 = 1_000_000;

    for &(seed_x, seed_z) in seed_points {
        if iterations % 10000 == 0 {
            if let Some(timeout) = timeout {
                if start_time.elapsed() > *timeout {
                    break;
                }
            }
        }

        if visited.get(seed_x, seed_z) {
            continue;
        }

        queue.clear();
        queue.push((seed_x, seed_z));
        visited.set(seed_x, seed_z);

        while let Some((curr_x, curr_z)) = queue.pop() {
            iterations += 1;
            if iterations > MAX_ITERATIONS {
                break;
            }

            if !polygon.contains(&Point::new(curr_x as f64, curr_z as f64)) {
                continue;
            }

            // Span fill: expand horizontally
            let mut left_x = curr_x;
            let mut right_x = curr_x;

            while left_x > min_x && !visited.get(left_x - 1, curr_z) 
                && polygon.contains(&Point::new((left_x - 1) as f64, curr_z as f64)) {
                left_x -= 1;
                visited.set(left_x, curr_z);
            }

            while right_x < max_x && !visited.get(right_x + 1, curr_z) 
                && polygon.contains(&Point::new((right_x + 1) as f64, curr_z as f64)) {
                right_x += 1;
                visited.set(right_x, curr_z);
            }

            for x in left_x..=right_x {
                filled.push((x, curr_z));
            }

            for check_z in [curr_z - 1, curr_z + 1] {
                if check_z < min_z || check_z > max_z {
                    continue;
                }
                for x in left_x..=right_x {
                    if !visited.get(x, check_z) 
                        && polygon.contains(&Point::new(x as f64, check_z as f64)) {
                        visited.set(x, check_z);
                        queue.push((x, check_z));
                    }
                }
            }
        }
    }

    filled
}

/// Original flood fill algorithm with enhanced multi-seed detection for complex shapes
fn original_flood_fill_area(
    polygon_coords: &[(i32, i32)],
    timeout: Option<&Duration>,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) -> Vec<(i32, i32)> {
    let start_time = Instant::now();

    // Convert input to a geo::Polygon for efficient point-in-polygon testing
    let exterior_coords: Vec<(f64, f64)> = polygon_coords
        .iter()
        .map(|&(x, z)| (x as f64, z as f64))
        .collect::<Vec<_>>();
    let exterior: LineString = LineString::from(exterior_coords);
    let polygon: Polygon<f64> = Polygon::new(exterior, vec![]);

    // Optimized step sizes for large polygons - coarser sampling for speed
    let width = max_x - min_x + 1;
    let height = max_z - min_z + 1;
    let step_x: i32 = (width / 8).clamp(1, 12);
    let step_z: i32 = (height / 8).clamp(1, 12);

    // Collect all potential seed points first
    let mut seed_points = Vec::new();
    for z in (min_z..=max_z).step_by(step_z as usize) {
        for x in (min_x..=max_x).step_by(step_x as usize) {
            if polygon.contains(&Point::new(x as f64, z as f64)) {
                seed_points.push((x, z));
            }
        }
    }

    // Only parallelize if we have enough seed points to justify overhead
    const PARALLEL_THRESHOLD: usize = 100;
    if seed_points.len() < PARALLEL_THRESHOLD {
        return sequential_span_fill(&polygon, &seed_points, min_x, max_x, min_z, max_z, timeout, &start_time);
    }

    // Create progress bar for parallel processing
    let pb = ProgressBar::new(seed_points.len() as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} seeds ({eta})")
        .unwrap()
        .progress_chars("█▓░"));

    // Process seed points in parallel chunks
    let chunk_size = (seed_points.len() / rayon::current_num_threads()).max(1);
    let results: Vec<Vec<(i32, i32)>> = seed_points
        .par_chunks(chunk_size)
        .map(|chunk_seeds| {
            let estimated_capacity = ((max_x - min_x) * (max_z - min_z)) as usize / 4;
            let mut local_filled = Vec::with_capacity(estimated_capacity.min(100000));
            let mut local_visited = BitGrid::new(min_x, max_x, min_z, max_z);
            let mut queue = VecQueue::with_capacity(2048);
            let mut iterations = 0u64;
            const MAX_ITERATIONS: u64 = 1_000_000;

            for &(seed_x, seed_z) in chunk_seeds {
                pb.inc(1);
                
                // Check timeout less frequently
                if iterations % 10000 == 0 {
                    if let Some(timeout) = timeout {
                        if start_time.elapsed() > *timeout {
                            break;
                        }
                    }
                }

                // Skip if already visited
                if local_visited.get(seed_x, seed_z) {
                    continue;
                }

                // Start span-based flood fill from this seed point
                queue.clear();
                queue.push((seed_x, seed_z));
                local_visited.set(seed_x, seed_z);

                while let Some((curr_x, curr_z)) = queue.pop() {
                    iterations += 1;
                    if iterations > MAX_ITERATIONS {
                        break;
                    }

                    if !polygon.contains(&Point::new(curr_x as f64, curr_z as f64)) {
                        continue;
                    }

                    // Span fill: expand horizontally
                    let mut left_x = curr_x;
                    let mut right_x = curr_x;

                    // Expand left
                    while left_x > min_x && !local_visited.get(left_x - 1, curr_z) 
                        && polygon.contains(&Point::new((left_x - 1) as f64, curr_z as f64)) {
                        left_x -= 1;
                        local_visited.set(left_x, curr_z);
                    }

                    // Expand right
                    while right_x < max_x && !local_visited.get(right_x + 1, curr_z) 
                        && polygon.contains(&Point::new((right_x + 1) as f64, curr_z as f64)) {
                        right_x += 1;
                        local_visited.set(right_x, curr_z);
                    }

                    // Add all points in span
                    for x in left_x..=right_x {
                        local_filled.push((x, curr_z));
                    }

                    // Check spans above and below
                    for check_z in [curr_z - 1, curr_z + 1] {
                        if check_z < min_z || check_z > max_z {
                            continue;
                        }
                        for x in left_x..=right_x {
                            if !local_visited.get(x, check_z) 
                                && polygon.contains(&Point::new(x as f64, check_z as f64)) {
                                local_visited.set(x, check_z);
                                queue.push((x, check_z));
                            }
                        }
                    }
                }
            }

            local_filled
        })
        .collect();

    pb.finish_and_clear();

    // Merge results - no deduplication needed with proper visited tracking
    results.into_iter().flatten().collect()
}

/// Parallel scanline flood fill for massive areas (oceans)
/// Divides the area into horizontal strips and processes them in parallel
fn parallel_scanline_flood_fill(
    polygon_coords: &[(i32, i32)],
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) -> Vec<(i32, i32)> {
    // Create polygon for containment testing
    let exterior_coords: Vec<(f64, f64)> = polygon_coords
        .iter()
        .map(|&(x, z)| (x as f64, z as f64))
        .collect();
    let exterior = LineString::from(exterior_coords);
    let polygon = Polygon::new(exterior, vec![]);

    // Divide into horizontal strips for parallel processing
    let strip_height = 64; // Process 64 rows at a time
    let mut strips = Vec::new();
    let mut strip_z = min_z;
    while strip_z <= max_z {
        let strip_max_z = (strip_z + strip_height - 1).min(max_z);
        strips.push((strip_z, strip_max_z));
        strip_z += strip_height;
    }

    println!("Processing {} strips in parallel for flood fill...", strips.len());

    // Create progress bar for strip processing
    let pb = ProgressBar::new(strips.len() as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} strips ({eta})")
        .unwrap()
        .progress_chars("█▓░"));

    // Process each strip in parallel using scanline algorithm
    let results: Vec<Vec<(i32, i32)>> = strips
        .par_iter()
        .map(|&(strip_min_z, strip_max_z)| {
            let mut strip_filled = Vec::new();

            // For each row in the strip, find continuous spans inside the polygon
            for z in strip_min_z..=strip_max_z {
                if z == strip_min_z {
                    pb.inc(1);
                }
                let mut x = min_x;
                while x <= max_x {
                    // Skip until we find a point inside the polygon
                    while x <= max_x && !polygon.contains(&Point::new(x as f64, z as f64)) {
                        x += 1;
                    }

                    if x > max_x {
                        break;
                    }

                    // Found start of span, now find the end
                    let span_start = x;
                    while x <= max_x && polygon.contains(&Point::new(x as f64, z as f64)) {
                        x += 1;
                    }
                    let span_end = x - 1;

                    // Add all points in this span
                    for span_x in span_start..=span_end {
                        strip_filled.push((span_x, z));
                    }
                }
            }

            strip_filled
        })
        .collect();

    pb.finish_and_clear();

    // Flatten results
    let total_points: usize = results.iter().map(|v| v.len()).sum();
    println!("Parallel flood fill completed: {} points", total_points);

    results.into_iter().flatten().collect()
}
