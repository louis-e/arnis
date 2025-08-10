use geo::{Contains, LineString, Point, Polygon};
use itertools::Itertools;
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

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

    // For small and medium areas, use optimized flood fill with span filling
    if area < 50000 {
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

    let mut filled_area = Vec::new();
    let mut global_visited = HashSet::new();

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
    let step_x = (width / 6).clamp(1, 8); // Balance between coverage and speed
    let step_z = (height / 6).clamp(1, 8);

    // Pre-allocate queue with reasonable capacity to avoid reallocations
    let mut queue = VecDeque::with_capacity(1024);

    for z in (min_z..=max_z).step_by(step_z as usize) {
        for x in (min_x..=max_x).step_by(step_x as usize) {
            // Fast timeout check - only every few iterations
            if filled_area.len() % 100 == 0 {
                if let Some(timeout) = timeout {
                    if start_time.elapsed() > *timeout {
                        return filled_area;
                    }
                }
            }

            // Skip if already visited or not inside polygon
            if global_visited.contains(&(x, z))
                || !polygon.contains(&Point::new(x as f64, z as f64))
            {
                continue;
            }

            // Start flood fill from this seed point
            queue.clear(); // Reuse queue instead of creating new one
            queue.push_back((x, z));
            global_visited.insert((x, z));

            while let Some((curr_x, curr_z)) = queue.pop_front() {
                // Add current point to filled area
                filled_area.push((curr_x, curr_z));

                // Check all four directions with optimized bounds checking
                let neighbors = [
                    (curr_x - 1, curr_z),
                    (curr_x + 1, curr_z),
                    (curr_x, curr_z - 1),
                    (curr_x, curr_z + 1),
                ];

                for (nx, nz) in neighbors.iter() {
                    if *nx >= min_x
                        && *nx <= max_x
                        && *nz >= min_z
                        && *nz <= max_z
                        && !global_visited.contains(&(*nx, *nz))
                    {
                        // Only check polygon containment for unvisited points
                        if polygon.contains(&Point::new(*nx as f64, *nz as f64)) {
                            global_visited.insert((*nx, *nz));
                            queue.push_back((*nx, *nz));
                        }
                    }
                }
            }
        }
    }

    filled_area
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
    let mut filled_area: Vec<(i32, i32)> = Vec::new();
    let mut global_visited: HashSet<(i32, i32)> = HashSet::new();

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
    let step_x: i32 = (width / 8).clamp(1, 12); // Cap max step size for coverage
    let step_z: i32 = (height / 8).clamp(1, 12);

    // Pre-allocate queue and reserve space for filled_area
    let mut queue: VecDeque<(i32, i32)> = VecDeque::with_capacity(2048);
    filled_area.reserve(1000); // Reserve space to reduce reallocations

    // Scan for multiple seed points to handle U-shapes and concave polygons
    for z in (min_z..=max_z).step_by(step_z as usize) {
        for x in (min_x..=max_x).step_by(step_x as usize) {
            // Reduced timeout checking frequency for better performance
            if global_visited.len() % 200 == 0 {
                if let Some(timeout) = timeout {
                    if &start_time.elapsed() > timeout {
                        return filled_area;
                    }
                }
            }

            // Skip if already processed or not inside polygon
            if global_visited.contains(&(x, z))
                || !polygon.contains(&Point::new(x as f64, z as f64))
            {
                continue;
            }

            // Start flood-fill from this seed point
            queue.clear(); // Reuse queue
            queue.push_back((x, z));
            global_visited.insert((x, z));

            while let Some((curr_x, curr_z)) = queue.pop_front() {
                // Only check polygon containment once per point when adding to filled_area
                if polygon.contains(&Point::new(curr_x as f64, curr_z as f64)) {
                    filled_area.push((curr_x, curr_z));

                    // Check adjacent points with optimized iteration
                    let neighbors = [
                        (curr_x - 1, curr_z),
                        (curr_x + 1, curr_z),
                        (curr_x, curr_z - 1),
                        (curr_x, curr_z + 1),
                    ];

                    for (nx, nz) in neighbors.iter() {
                        if *nx >= min_x
                            && *nx <= max_x
                            && *nz >= min_z
                            && *nz <= max_z
                            && !global_visited.contains(&(*nx, *nz))
                        {
                            global_visited.insert((*nx, *nz));
                            queue.push_back((*nx, *nz));
                        }
                    }
                }
            }
        }
    }

    filled_area
}
