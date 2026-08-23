use geo::orient::{Direction, Orient};
use geo::{Contains, LineString, Point, Polygon};
use itertools::Itertools;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Maximum bounding box area (in blocks) for the visited-bitmap flood fill.
/// 25 million blocks ≈ 5000×5000; bitmap uses only ~3 MB at this size.
/// Past this the scanline path takes over, which needs no bitmap.
pub const MAX_FLOOD_FILL_AREA: i64 = 25_000_000;

/// Work cap for the scanline fill, whose cost is rows × edges rather than area.
const MAX_SCANLINE_EDGE_TESTS: i64 = 200_000_000;

/// A compact bitmap for visited-coordinate tracking during flood fill.
///
/// Uses 1 bit per coordinate instead of ~48 bytes per entry in a `HashSet`.
/// For a 5000×5000 bounding box this is ~3 MB instead of ~1.2 GB.
struct FloodBitmap {
    bits: Vec<u8>,
    min_x: i32,
    min_z: i32,
    width: usize,
}

impl FloodBitmap {
    #[inline]
    fn new(min_x: i32, max_x: i32, min_z: i32, max_z: i32) -> Self {
        let width = (max_x - min_x + 1) as usize;
        let height = (max_z - min_z + 1) as usize;
        let num_bytes = (width * height).div_ceil(8);
        Self {
            bits: vec![0u8; num_bytes],
            min_x,
            min_z,
            width,
        }
    }

    /// Mark (x, z) as visited. Returns `true` if it was NOT already visited
    /// (i.e. this is the first visit).
    #[inline]
    fn insert(&mut self, x: i32, z: i32) -> bool {
        let idx = (z - self.min_z) as usize * self.width + (x - self.min_x) as usize;
        let byte = idx / 8;
        let bit = idx % 8;
        let mask = 1u8 << bit;
        if self.bits[byte] & mask != 0 {
            false // already visited
        } else {
            self.bits[byte] |= mask;
            true
        }
    }

    #[inline]
    fn contains(&self, x: i32, z: i32) -> bool {
        let idx = (z - self.min_z) as usize * self.width + (x - self.min_x) as usize;
        let byte = idx / 8;
        let bit = idx % 8;
        (self.bits[byte] >> bit) & 1 == 1
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

    // Reject open polylines: geo::Polygon auto-closes by connecting last to
    // first, which creates a diagonal artifact edge for genuinely open ways
    // (e.g. ridges, cliffs). Closed polygons from SH clipping always have
    // first == last preserved by clip_way_to_bbox.
    let first = polygon_coords[0];
    let last = polygon_coords[polygon_coords.len() - 1];
    if first != last {
        return vec![];
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

    // Too big for the visited bitmap, which scales with the bounding box. Scanline needs none.
    if area > MAX_FLOOD_FILL_AREA {
        return scanline_fill_area(polygon_coords, min_x, max_x, min_z, max_z);
    }

    // For small and medium areas, use optimized flood fill with span filling
    if area < 50000 {
        optimized_flood_fill_area(polygon_coords, timeout, min_x, max_x, min_z, max_z)
    } else {
        // For larger areas, use original flood fill with grid sampling
        original_flood_fill_area(polygon_coords, timeout, min_x, max_x, min_z, max_z)
    }
}

/// Even-odd scanline fill for polygons whose bounding box is past the bitmap cap.
///
/// Callers draw the polygon's edge before they ask for the fill, so returning nothing left
/// an outline with untouched ground inside it. A large `natural=sand` area at 1:1 is the
/// visible case. Spans are counted before anything is allocated, so the output vector is
/// sized once and peak memory stays at the result itself.
///
/// Rows are sampled on the integer lattice and spans exclude their endpoints, so the cells
/// this returns are the ones the bitmap paths would test with `geo::Contains`. The one case
/// the two still disagree on is a cell lying exactly on a horizontal edge: ray casting cannot
/// see that, and detecting it would cost a per-cell edge test, which is the whole reason this
/// path exists. Only rings with a bounding box over 25M blocks get here, so that is a block of
/// slack on a shape thousands of blocks across.
fn scanline_fill_area(
    polygon_coords: &[(i32, i32)],
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) -> Vec<(i32, i32)> {
    let rows = max_z as i64 - min_z as i64 + 1;
    let edges = polygon_coords.len() as i64 - 1;
    if rows.saturating_mul(edges) > MAX_SCANLINE_EDGE_TESTS {
        return vec![];
    }

    let mut spans: Vec<(i32, i32, i32)> = Vec::new();
    let mut crossings: Vec<f64> = Vec::new();
    let mut cells: i64 = 0;

    for z in min_z..=max_z {
        let zf = z as f64;
        crossings.clear();
        for w in polygon_coords.windows(2) {
            let (x0, z0) = (w[0].0 as f64, w[0].1 as f64);
            let (x1, z1) = (w[1].0 as f64, w[1].1 as f64);
            // Half-open in z, so a vertex sitting exactly on the row counts once, not twice.
            if (z0 <= zf) == (z1 <= zf) {
                continue;
            }
            let t = (zf - z0) / (z1 - z0);
            crossings.push(x0 + t * (x1 - x0));
        }
        if crossings.len() < 2 {
            continue;
        }
        crossings.sort_unstable_by(f64::total_cmp);

        for &[left, right] in crossings.as_chunks::<2>().0 {
            // Strictly between the crossings, so a cell sitting exactly on the edge is left to
            // the caller's outline pass, the same way geo::Contains treats the boundary.
            let xs = (left.floor() as i32).saturating_add(1).max(min_x);
            let xe = (right.ceil() as i32).saturating_sub(1).min(max_x);
            if xe < xs {
                continue;
            }
            cells += xe as i64 - xs as i64 + 1;
            if cells > MAX_FLOOD_FILL_AREA {
                return vec![];
            }
            spans.push((z, xs, xe));
        }
    }

    let mut filled: Vec<(i32, i32)> = Vec::with_capacity(cells as usize);
    for (z, xs, xe) in spans {
        for x in xs..=xe {
            filled.push((x, z));
        }
    }
    filled
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
    let mut visited = FloodBitmap::new(min_x, max_x, min_z, max_z);

    // Create polygon for containment testing, with normalized winding order
    // to avoid "polygon had no winding order" warnings from geo::Contains
    let exterior_coords: Vec<(f64, f64)> = polygon_coords
        .iter()
        .map(|&(x, z)| (x as f64, z as f64))
        .collect();
    let exterior = LineString::from(exterior_coords);
    let polygon = Polygon::new(exterior, vec![]).orient(Direction::Default);

    // Optimized step sizes: larger steps for efficiency, but still catch U-shapes
    let width = max_x - min_x + 1;
    let height = max_z - min_z + 1;
    let step_x = (width / 6).clamp(1, 8); // Balance between coverage and speed
    let step_z = (height / 6).clamp(1, 8);

    // Pre-allocate queue with reasonable capacity to avoid reallocations
    let mut queue = VecDeque::with_capacity(1024);

    for z in (min_z..=max_z).step_by(step_z as usize) {
        for x in (min_x..=max_x).step_by(step_x as usize) {
            // Fast timeout check, only every few iterations
            if filled_area.len() % 100 == 0 {
                if let Some(timeout) = timeout {
                    if start_time.elapsed() > *timeout {
                        return filled_area;
                    }
                }
            }

            // Skip if already visited or not inside polygon
            if visited.contains(x, z) || !polygon.contains(&Point::new(x as f64, z as f64)) {
                continue;
            }

            // Start flood fill from this seed point
            queue.clear(); // Reuse queue instead of creating new one
            queue.push_back((x, z));
            visited.insert(x, z);

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

                for &(nx, nz) in &neighbors {
                    if nx >= min_x
                        && nx <= max_x
                        && nz >= min_z
                        && nz <= max_z
                        && visited.insert(nx, nz)
                    {
                        // Only check polygon containment for unvisited points
                        if polygon.contains(&Point::new(nx as f64, nz as f64)) {
                            queue.push_back((nx, nz));
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
    let mut visited = FloodBitmap::new(min_x, max_x, min_z, max_z);

    // Convert input to a geo::Polygon for efficient point-in-polygon testing,
    // with normalized winding order to avoid undefined Contains results
    let exterior_coords: Vec<(f64, f64)> = polygon_coords
        .iter()
        .map(|&(x, z)| (x as f64, z as f64))
        .collect::<Vec<_>>();
    let exterior: LineString = LineString::from(exterior_coords);
    let polygon: Polygon<f64> = Polygon::new(exterior, vec![]).orient(Direction::Default);

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
            // Use manual % check since is_multiple_of() is unstable on stable Rust
            if let Some(timeout) = timeout {
                if &start_time.elapsed() > timeout {
                    return filled_area;
                }
            }

            // Skip if already processed or not inside polygon
            if visited.contains(x, z) || !polygon.contains(&Point::new(x as f64, z as f64)) {
                continue;
            }

            // Start flood-fill from this seed point
            queue.clear(); // Reuse queue
            queue.push_back((x, z));
            visited.insert(x, z);

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

                    for &(nx, nz) in &neighbors {
                        if nx >= min_x
                            && nx <= max_x
                            && nz >= min_z
                            && nz <= max_z
                            && visited.insert(nx, nz)
                        {
                            queue.push_back((nx, nz));
                        }
                    }
                }
            }
        }
    }

    filled_area
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_polygon_still_uses_the_bitmap_path() {
        let square = [(0, 0), (10, 0), (10, 10), (0, 10), (0, 0)];
        let filled = flood_fill_area(&square, None);
        assert!(!filled.is_empty());
        assert!(filled
            .iter()
            .all(|&(x, z)| (0..=10).contains(&x) && (0..=10).contains(&z)));
    }

    #[test]
    fn huge_bbox_but_thin_polygon_is_filled_not_dropped() {
        // 6001 x 6011 bounding box is past the bitmap cap, but the band itself is ~60k cells.
        let band = [(0, 0), (6000, 6000), (6000, 6010), (0, 10), (0, 0)];
        let bbox = 6001_i64 * 6011;
        assert!(bbox > MAX_FLOOD_FILL_AREA);

        let filled = flood_fill_area(&band, None);
        assert!(filled.len() > 50_000, "got {} cells", filled.len());
        assert!(filled.len() < 100_000, "got {} cells", filled.len());
        assert!(filled
            .iter()
            .all(|&(x, z)| (0..=6000).contains(&x) && (0..=6010).contains(&z)));
    }

    #[test]
    fn scanline_agrees_with_contains_on_the_integer_lattice() {
        // Called directly, since a polygon small enough to brute force never has a bounding
        // box large enough to reach this path through flood_fill_area.
        let ring = [(0, 0), (40, 7), (55, 30), (25, 48), (3, 25), (0, 0)];
        let (min_x, max_x, min_z, max_z) = (0, 55, 0, 48);

        let exterior = LineString::from(
            ring.iter()
                .map(|&(x, z)| (x as f64, z as f64))
                .collect::<Vec<_>>(),
        );
        let polygon = Polygon::new(exterior, vec![]).orient(Direction::Default);

        let scan: std::collections::HashSet<(i32, i32)> =
            scanline_fill_area(&ring, min_x, max_x, min_z, max_z)
                .into_iter()
                .collect();

        for z in min_z..=max_z {
            for x in min_x..=max_x {
                let inside = polygon.contains(&Point::new(x as f64, z as f64));
                assert_eq!(scan.contains(&(x, z)), inside, "disagreed at ({x}, {z})");
            }
        }
    }

    #[test]
    fn a_zero_area_ring_fills_nothing_however_big_its_bbox() {
        // Traced out and straight back, so there is no interior at any row. The bounding box
        // is 36M, well past the cap, but area is what decides the output.
        let doubled_back = [(0, 0), (6000, 6000), (0, 0)];
        assert!(flood_fill_area(&doubled_back, None).is_empty());
    }

    #[test]
    fn a_one_block_high_wedge_never_overfills() {
        // The scanline samples whole rows, so a shape with no interior lattice row must not
        // pick one up. Padded to clear the bbox cap without giving it real height.
        let wedge = [(0, 0), (26_000, 0), (13_000, 1), (0, 1000), (0, 0)];
        let filled = flood_fill_area(&wedge, None);
        assert!(filled.iter().all(|&(_, z)| (0..=1000).contains(&z)));
        assert!(filled.len() < 26_000 * 1001, "got {} cells", filled.len());
    }

    #[test]
    fn polygon_covering_more_than_the_budget_is_refused() {
        let triangle = [(0, 0), (9000, 0), (0, 9000), (0, 0)];
        assert!(flood_fill_area(&triangle, None).is_empty());
    }

    #[test]
    fn absurd_row_count_is_refused_before_any_work() {
        let sliver = [(0, 0), (10, 0), (10, 4_000_000), (0, 4_000_000), (0, 0)];
        assert!(flood_fill_area(&sliver, None).is_empty());
    }
}
