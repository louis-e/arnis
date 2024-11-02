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
    let exterior: LineString = LineString::from(exterior_coords); // Create LineString from coordinates
    let polygon: Polygon<f64> = Polygon::new(exterior, vec![]); // Create Polygon using LineString

    // Determine safe step sizes for grid sampling
    let step_x: i32 = ((max_x - min_x) / 10).max(1); // Ensure step is at least 1
    let step_z: i32 = ((max_z - min_z) / 10).max(1); // Ensure step is at least 1

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
                eprintln!("Floodfill timeout"); // TODO only print when debug arg is set?
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
                        eprintln!("Floodfill timeout"); // TODO only print when debug arg is set?
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
