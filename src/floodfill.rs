use geo::{Contains, LineString, Point, Polygon};
use itertools::Itertools;
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

/// Perform a flood-fill to find the area inside a polygon.
/// Returns a vector of (x, z) coordinates representing the filled area.
pub fn flood_fill_area(
    polygon_coords: &[(i32, i32)],
    timeout: Option<&Duration>,
) -> Vec<(i32, i32)> {
    if polygon_coords.len() < 3 {
        return vec![]; // Not a valid polygon
    }

    let start_time: Instant = Instant::now();

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

    let mut filled_area: Vec<(i32, i32)> = Vec::new();
    let mut visited: HashSet<(i32, i32)> = HashSet::new();

    // Convert input to a geo::Polygon for efficient point-in-polygon testing
    let exterior_coords: Vec<(f64, f64)> = polygon_coords
        .iter()
        .map(|&(x, z)| (x as f64, z as f64))
        .collect::<Vec<_>>();
    let exterior: LineString = LineString::from(exterior_coords);
    let polygon: Polygon<f64> = Polygon::new(exterior, vec![]);

    // Optimized step size calculation - use adaptive grid based on polygon area
    let width = max_x - min_x;
    let height = max_z - min_z;
    let area_estimate = width * height;
    
    // Use smaller steps for smaller polygons, larger for bigger ones
    let step_x: i32 = if area_estimate < 100 { 1 } 
                     else if area_estimate < 10000 { (width / 20).max(1) }
                     else { (width / 10).max(1) };
    let step_z: i32 = if area_estimate < 100 { 1 }
                     else if area_estimate < 10000 { (height / 20).max(1) }
                     else { (height / 10).max(1) };

    // Find a good starting point using smarter sampling
    let start_point = find_interior_point(&polygon, min_x, max_x, min_z, max_z, step_x, step_z);
    
    if let Some((start_x, start_z)) = start_point {
        // Pre-allocate vectors with estimated capacity
        let estimated_capacity = ((width * height) / 4).min(10000) as usize;
        filled_area.reserve(estimated_capacity);
        visited.reserve(estimated_capacity);
        
        // Single flood-fill from the found interior point
        let mut queue: VecDeque<(i32, i32)> = VecDeque::with_capacity(1000);
        queue.push_back((start_x, start_z));
        visited.insert((start_x, start_z));

        // Batch timeout checking to reduce overhead
        let mut iteration_count = 0u32;
        const TIMEOUT_CHECK_INTERVAL: u32 = 1000;

        while let Some((x, z)) = queue.pop_front() {
            // Check timeout only every N iterations to reduce overhead
            iteration_count += 1;
            if iteration_count % TIMEOUT_CHECK_INTERVAL == 0 {
                if let Some(timeout) = timeout {
                    if &start_time.elapsed() > timeout {
                        eprintln!("Floodfill timeout");
                        break;
                    }
                }
            }

            // Pre-create point once for the containment check
            let point = Point::new(x as f64, z as f64);
            if polygon.contains(&point) {
                filled_area.push((x, z));

                // Check adjacent points with bounds checking first
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let nx = x + dx;
                    let nz = z + dz;
                    
                    if nx >= min_x && nx <= max_x && nz >= min_z && nz <= max_z {
                        let coord = (nx, nz);
                        if !visited.contains(&coord) {
                            visited.insert(coord);
                            queue.push_back(coord);
                        }
                    }
                }
            }
        }
    }

    filled_area
}

/// Find a good interior point for starting the flood fill
fn find_interior_point(
    polygon: &Polygon<f64>,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    step_x: i32,
    step_z: i32,
) -> Option<(i32, i32)> {
    // Start from center and work outward
    let center_x = (min_x + max_x) / 2;
    let center_z = (min_z + max_z) / 2;
    
    // Check center first
    if polygon.contains(&Point::new(center_x as f64, center_z as f64)) {
        return Some((center_x, center_z));
    }
    
    // Spiral search from center
    for radius in 1..=(((max_x - min_x).max(max_z - min_z)) / (step_x.min(step_z))) {
        for x in (center_x - radius * step_x..=center_x + radius * step_x).step_by(step_x as usize) {
            for z in (center_z - radius * step_z..=center_z + radius * step_z).step_by(step_z as usize) {
                if x >= min_x && x <= max_x && z >= min_z && z <= max_z {
                    if polygon.contains(&Point::new(x as f64, z as f64)) {
                        return Some((x, z));
                    }
                }
            }
        }
    }
    
    None
}
