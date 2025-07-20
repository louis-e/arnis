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

/// Optimized flood fill for larger polygons with smart start point detection and span filling
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
    let mut visited = HashSet::new();

    // Create polygon for containment testing
    let exterior_coords: Vec<(f64, f64)> = polygon_coords
        .iter()
        .map(|&(x, z)| (x as f64, z as f64))
        .collect();
    let exterior = LineString::from(exterior_coords);
    let polygon = Polygon::new(exterior, vec![]);

    // Smart start point detection - find centroid first
    let centroid_x =
        polygon_coords.iter().map(|&(x, _)| x).sum::<i32>() / polygon_coords.len() as i32;
    let centroid_z =
        polygon_coords.iter().map(|&(_, z)| z).sum::<i32>() / polygon_coords.len() as i32;

    // Try centroid first, then expand search if needed
    let search_points = vec![
        (centroid_x, centroid_z),
        (min_x + (max_x - min_x) / 2, min_z + (max_z - min_z) / 2),
        (min_x + (max_x - min_x) / 3, min_z + (max_z - min_z) / 3),
        (
            min_x + 2 * (max_x - min_x) / 3,
            min_z + 2 * (max_z - min_z) / 3,
        ),
    ];

    for &(start_x, start_z) in &search_points {
        if polygon.contains(&Point::new(start_x as f64, start_z as f64)) {
            // Found valid start point, begin optimized flood fill
            let mut queue = VecDeque::new();
            queue.push_back((start_x, start_z));
            visited.insert((start_x, start_z));

            while let Some((x, z)) = queue.pop_front() {
                if let Some(timeout) = timeout {
                    if start_time.elapsed() > *timeout {
                        eprintln!("Optimized flood fill timeout");
                        return filled_area;
                    }
                }

                // Add current point to filled area
                filled_area.push((x, z));

                // Check all four directions
                for (nx, nz) in [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)].iter() {
                    if *nx >= min_x
                        && *nx <= max_x
                        && *nz >= min_z
                        && *nz <= max_z
                        && !visited.contains(&(*nx, *nz))
                        && polygon.contains(&Point::new(*nx as f64, *nz as f64))
                    {
                        visited.insert((*nx, *nz));
                        queue.push_back((*nx, *nz));
                    }
                }
            }

            break; // Found and processed a valid start point
        }
    }

    filled_area
}

/// Original flood fill algorithm for smaller polygons
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
    let mut visited: HashSet<(i32, i32)> = HashSet::new();

    // Convert input to a geo::Polygon for efficient point-in-polygon testing
    let exterior_coords: Vec<(f64, f64)> = polygon_coords
        .iter()
        .map(|&(x, z)| (x as f64, z as f64))
        .collect::<Vec<_>>();
    let exterior: LineString = LineString::from(exterior_coords);
    let polygon: Polygon<f64> = Polygon::new(exterior, vec![]);

    // Determine safe step sizes for grid sampling
    let step_x: i32 = ((max_x - min_x) / 10).max(1);
    let step_z: i32 = ((max_z - min_z) / 10).max(1);

    // Sample multiple starting points within the bounding box
    let mut candidate_points: VecDeque<(i32, i32)> = VecDeque::new();
    for x in (min_x..=max_x).step_by(step_x as usize) {
        for z in (min_z..=max_z).step_by(step_z as usize) {
            candidate_points.push_back((x, z));
        }
    }

    // Attempt flood-fill from each candidate point
    while let Some((start_x, start_z)) = candidate_points.pop_front() {
        if let Some(timeout) = timeout {
            if &start_time.elapsed() > timeout {
                eprintln!("Floodfill timeout");
                break;
            }
        }

        if polygon.contains(&Point::new(start_x as f64, start_z as f64)) {
            // Start flood-fill from the valid interior point
            let mut queue: VecDeque<(i32, i32)> = VecDeque::new();
            queue.push_back((start_x, start_z));
            visited.insert((start_x, start_z));

            while let Some((x, z)) = queue.pop_front() {
                if let Some(timeout) = timeout {
                    if &start_time.elapsed() > timeout {
                        eprintln!("Floodfill timeout");
                        break;
                    }
                }

                if polygon.contains(&Point::new(x as f64, z as f64)) {
                    filled_area.push((x, z));

                    // Check adjacent points
                    for (nx, nz) in [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)].iter() {
                        if *nx >= min_x
                            && *nx <= max_x
                            && *nz >= min_z
                            && *nz <= max_z
                            && !visited.contains(&(*nx, *nz))
                        {
                            visited.insert((*nx, *nz));
                            queue.push_back((*nx, *nz));
                        }
                    }
                }
            }

            if !filled_area.is_empty() {
                break; // Exit if a valid area has been flood-filled
            }
        }
    }

    filled_area
}
