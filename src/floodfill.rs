use itertools::Itertools;
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Maximum bounding box area (in blocks) for the bitmap flood fill.
/// 25 million blocks ≈ 5000×5000; bitmap uses only ~3 MB at this size.
/// Past this the scanline path takes over, which needs no bitmap.
pub const MAX_FLOOD_FILL_AREA: i64 = 25_000_000;

/// Work cap for the scanline fill, whose cost is rows × edges rather than area.
const MAX_SCANLINE_EDGE_TESTS: i64 = 200_000_000;

/// One bit per cell of a polygon's bounding box, ~3 MB at 5000×5000.
struct FloodBitmap {
    bits: Vec<u64>,
    min_x: i32,
    min_z: i32,
    width: usize,
}

impl FloodBitmap {
    #[inline]
    fn new(min_x: i32, max_x: i32, min_z: i32, max_z: i32) -> Self {
        let width = (max_x - min_x + 1) as usize;
        let height = (max_z - min_z + 1) as usize;
        Self {
            bits: vec![0u64; (width * height).div_ceil(64)],
            min_x,
            min_z,
            width,
        }
    }

    #[inline]
    fn index(&self, x: i32, z: i32) -> usize {
        (z - self.min_z) as usize * self.width + (x - self.min_x) as usize
    }

    /// Clears bit `i` and returns whether it was set.
    #[inline]
    fn take(&mut self, i: usize) -> bool {
        let word = &mut self.bits[i / 64];
        let mask = 1u64 << (i % 64);
        let was_set = *word & mask != 0;
        *word &= !mask;
        was_set
    }

    #[inline]
    fn unset(&mut self, x: i32, z: i32) {
        let i = self.index(x, z);
        self.bits[i / 64] &= !(1u64 << (i % 64));
    }

    /// Sets or clears row `z` from `x0` to `x1` inclusive.
    fn write_span(&mut self, z: i32, x0: i32, x1: i32, value: bool) {
        let (lo, hi) = (self.index(x0, z), self.index(x1, z));
        let (lo_word, hi_word) = (lo / 64, hi / 64);
        let lo_mask = u64::MAX << (lo % 64);
        let hi_mask = u64::MAX >> (63 - hi % 64);
        let mut apply = |word: usize, mask: u64| {
            if value {
                self.bits[word] |= mask;
            } else {
                self.bits[word] &= !mask;
            }
        };
        if lo_word == hi_word {
            apply(lo_word, lo_mask & hi_mask);
        } else {
            apply(lo_word, lo_mask);
            for word in lo_word + 1..hi_word {
                apply(word, u64::MAX);
            }
            apply(hi_word, hi_mask);
        }
    }

    fn count(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }

    #[cfg(test)]
    fn insert(&mut self, x: i32, z: i32) -> bool {
        let i = self.index(x, z);
        let fresh = self.bits[i / 64] & (1u64 << (i % 64)) == 0;
        self.bits[i / 64] |= 1u64 << (i % 64);
        fresh
    }

    #[cfg(test)]
    fn contains(&self, x: i32, z: i32) -> bool {
        let i = self.index(x, z);
        (self.bits[i / 64] >> (i % 64)) & 1 == 1
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

    let bbox = (min_x, max_x, min_z, max_z);
    if area < 50000 {
        // Denser seed grid for small shapes, with timeout checks only every 100 cells.
        seeded_fill(polygon_coords, timeout, bbox, (6, 8), false)
    } else {
        // Coarser seed grid for large shapes, checking the timeout at every seed.
        seeded_fill(polygon_coords, timeout, bbox, (8, 12), true)
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

/// A non-horizontal ring edge, stored from its lower end.
#[derive(Clone, Copy)]
struct Edge {
    z0: i32,
    z1: i32,
    x0: i64,
    dx: i64,
    dz: i64,
    winding: i32,
}

/// The cells `geo::Contains` reports inside the ring, by geo's own rule: nonzero winding, edges
/// counted on rows `[low z, high z)`, cells on an edge excluded. Exact integer crossings.
fn inside_mask(
    ring: &[(i32, i32)],
    (min_x, max_x, min_z, max_z): (i32, i32, i32, i32),
    timeout: Option<&Duration>,
    start_time: Instant,
) -> Option<FloodBitmap> {
    let mut mask = FloodBitmap::new(min_x, max_x, min_z, max_z);

    let mut edges: Vec<Edge> = ring
        .windows(2)
        .filter_map(|w| {
            let ((ax, az), (bx, bz)) = (w[0], w[1]);
            let ((x0, z0), (x1, z1), winding) = match az.cmp(&bz) {
                Ordering::Less => ((ax, az), (bx, bz), 1),
                Ordering::Greater => ((bx, bz), (ax, az), -1),
                Ordering::Equal => return None,
            };
            Some(Edge {
                z0,
                z1,
                x0: x0 as i64,
                dx: x1 as i64 - x0 as i64,
                dz: z1 as i64 - z0 as i64,
                winding,
            })
        })
        .collect();
    edges.sort_unstable_by_key(|e| e.z0);

    let mut active: Vec<Edge> = Vec::new();
    let mut next_edge = 0;
    // (first cell right of the crossing, winding, crossing lands exactly on a cell)
    let mut crossings: Vec<(i64, i32, bool)> = Vec::new();
    for z in min_z..=max_z {
        if let Some(timeout) = timeout {
            if (z - min_z) % 64 == 0 && start_time.elapsed() > *timeout {
                return None;
            }
        }
        active.retain(|e| e.z1 > z);
        while next_edge < edges.len() && edges[next_edge].z0 == z {
            active.push(edges[next_edge]);
            next_edge += 1;
        }

        crossings.clear();
        for e in &active {
            let t = (z - e.z0) as i64 * e.dx;
            crossings.push((e.x0 + t.div_euclid(e.dz) + 1, e.winding, t % e.dz == 0));
        }
        crossings.sort_unstable_by_key(|c| c.0);

        // The row's windings sum to zero, so nothing runs past the last crossing.
        let mut winding = 0;
        for pair in crossings.windows(2) {
            winding += pair[0].1;
            if winding != 0 && pair[0].0 < pair[1].0 {
                mask.write_span(z, pair[0].0 as i32, (pair[1].0 - 1) as i32, true);
            }
        }
        for &(x, _, on_cell) in &crossings {
            if on_cell {
                mask.unset((x - 1) as i32, z);
            }
        }
    }

    // Crossings never land on an edge's top vertex or on a horizontal edge, so clear those here.
    for w in ring.windows(2) {
        let ((ax, az), (bx, bz)) = (w[0], w[1]);
        if az == bz {
            mask.write_span(az, ax.min(bx), ax.max(bx), false);
        } else {
            mask.unset(ax, az);
        }
    }
    Some(mask)
}

/// Breadth-first fill from a grid of seeds. Same cells in the same order as testing each one
/// with `geo::Contains` when reached, since outside cells never get queued or filled either way.
fn seeded_fill(
    polygon_coords: &[(i32, i32)],
    timeout: Option<&Duration>,
    bbox: (i32, i32, i32, i32),
    (step_div, step_cap): (i32, i32),
    check_timeout_every_seed: bool,
) -> Vec<(i32, i32)> {
    let start_time = Instant::now();
    let (min_x, max_x, min_z, max_z) = bbox;
    // Inside cells not yet queued; each is taken out once, when it is queued.
    let Some(mut remaining) = inside_mask(polygon_coords, bbox, timeout, start_time) else {
        return Vec::new();
    };
    let row = remaining.width;

    let step_x = ((max_x - min_x + 1) / step_div).clamp(1, step_cap);
    let step_z = ((max_z - min_z + 1) / step_div).clamp(1, step_cap);

    // Sized from the mask, so the result is not left with up to half its capacity unused.
    let mut filled_area: Vec<(i32, i32)> = Vec::with_capacity(remaining.count());
    let mut queue: VecDeque<(i32, i32)> = VecDeque::with_capacity(1024);

    'seeds: for z in (min_z..=max_z).step_by(step_z as usize) {
        for x in (min_x..=max_x).step_by(step_x as usize) {
            if check_timeout_every_seed || filled_area.len().is_multiple_of(100) {
                if let Some(timeout) = timeout {
                    if start_time.elapsed() > *timeout {
                        break 'seeds;
                    }
                }
            }

            if !remaining.take(remaining.index(x, z)) {
                continue;
            }

            queue.clear();
            queue.push_back((x, z));
            while let Some((cx, cz)) = queue.pop_front() {
                filled_area.push((cx, cz));
                let i = remaining.index(cx, cz);
                if cx > min_x && remaining.take(i - 1) {
                    queue.push_back((cx - 1, cz));
                }
                if cx < max_x && remaining.take(i + 1) {
                    queue.push_back((cx + 1, cz));
                }
                if cz > min_z && remaining.take(i - row) {
                    queue.push_back((cx, cz - 1));
                }
                if cz < max_z && remaining.take(i + row) {
                    queue.push_back((cx, cz + 1));
                }
            }
        }
    }

    // Pockets no seed reached, or a timeout, leave capacity unused.
    filled_area.shrink_to_fit();
    filled_area
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::orient::{Direction, Orient};
    use geo::{Contains, LineString, Point, Polygon};
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    // The fill as it was before the inside mask, testing each cell with geo::Contains.
    fn reference_flood_fill_area(
        polygon_coords: &[(i32, i32)],
        timeout: Option<&Duration>,
    ) -> Vec<(i32, i32)> {
        if polygon_coords.len() < 3 || polygon_coords[0] != polygon_coords[polygon_coords.len() - 1]
        {
            return vec![];
        }
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
        if area > MAX_FLOOD_FILL_AREA {
            return scanline_fill_area(polygon_coords, min_x, max_x, min_z, max_z);
        }
        if area < 50000 {
            reference_optimized_flood_fill_area(polygon_coords, timeout, min_x, max_x, min_z, max_z)
        } else {
            reference_original_flood_fill_area(polygon_coords, timeout, min_x, max_x, min_z, max_z)
        }
    }

    fn reference_optimized_flood_fill_area(
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
        let polygon = polygon_of(polygon_coords);
        let width = max_x - min_x + 1;
        let height = max_z - min_z + 1;
        let step_x = (width / 6).clamp(1, 8);
        let step_z = (height / 6).clamp(1, 8);
        let mut queue = VecDeque::with_capacity(1024);

        for z in (min_z..=max_z).step_by(step_z as usize) {
            for x in (min_x..=max_x).step_by(step_x as usize) {
                if filled_area.len() % 100 == 0 {
                    if let Some(timeout) = timeout {
                        if start_time.elapsed() > *timeout {
                            return filled_area;
                        }
                    }
                }
                if visited.contains(x, z) || !polygon.contains(&Point::new(x as f64, z as f64)) {
                    continue;
                }
                queue.clear();
                queue.push_back((x, z));
                visited.insert(x, z);
                while let Some((curr_x, curr_z)) = queue.pop_front() {
                    filled_area.push((curr_x, curr_z));
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
                            && polygon.contains(&Point::new(nx as f64, nz as f64))
                        {
                            queue.push_back((nx, nz));
                        }
                    }
                }
            }
        }
        filled_area
    }

    fn reference_original_flood_fill_area(
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
        let polygon = polygon_of(polygon_coords);
        let width = max_x - min_x + 1;
        let height = max_z - min_z + 1;
        let step_x: i32 = (width / 8).clamp(1, 12);
        let step_z: i32 = (height / 8).clamp(1, 12);
        let mut queue: VecDeque<(i32, i32)> = VecDeque::with_capacity(2048);

        for z in (min_z..=max_z).step_by(step_z as usize) {
            for x in (min_x..=max_x).step_by(step_x as usize) {
                if let Some(timeout) = timeout {
                    if &start_time.elapsed() > timeout {
                        return filled_area;
                    }
                }
                if visited.contains(x, z) || !polygon.contains(&Point::new(x as f64, z as f64)) {
                    continue;
                }
                queue.clear();
                queue.push_back((x, z));
                visited.insert(x, z);
                while let Some((curr_x, curr_z)) = queue.pop_front() {
                    if polygon.contains(&Point::new(curr_x as f64, curr_z as f64)) {
                        filled_area.push((curr_x, curr_z));
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

    fn polygon_of(ring: &[(i32, i32)]) -> Polygon<f64> {
        let exterior = LineString::from(
            ring.iter()
                .map(|&(x, z)| (x as f64, z as f64))
                .collect::<Vec<_>>(),
        );
        Polygon::new(exterior, vec![]).orient(Direction::Default)
    }

    fn closed(mut ring: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
        ring.push(ring[0]);
        ring
    }

    fn shifted(ring: &[(i32, i32)], dx: i32, dz: i32) -> Vec<(i32, i32)> {
        ring.iter().map(|&(x, z)| (x + dx, z + dz)).collect()
    }

    fn reversed(ring: &[(i32, i32)]) -> Vec<(i32, i32)> {
        ring.iter().rev().copied().collect()
    }

    // Same cells in the same order as the reference, and a mask equal to geo on every cell.
    fn assert_matches_reference(ring: &[(i32, i32)]) -> Vec<(i32, i32)> {
        let label = if ring.len() <= 40 {
            format!("{ring:?}")
        } else {
            format!("{} vertices from {:?}", ring.len(), ring[0])
        };
        let got = flood_fill_area(ring, None);
        let want = reference_flood_fill_area(ring, None);
        if let Some(i) = (0..got.len().max(want.len())).find(|&i| got.get(i) != want.get(i)) {
            panic!(
                "fill diverges at {i}: {:?} vs reference {:?} ({} vs {} cells) for {label}",
                got.get(i),
                want.get(i),
                got.len(),
                want.len()
            );
        }

        if ring.len() < 3 || ring[0] != ring[ring.len() - 1] {
            return got;
        }
        let (min_x, max_x) = ring.iter().map(|p| p.0).minmax().into_option().unwrap();
        let (min_z, max_z) = ring.iter().map(|p| p.1).minmax().into_option().unwrap();
        if (max_x - min_x + 1) as i64 * (max_z - min_z + 1) as i64 > MAX_FLOOD_FILL_AREA {
            return got;
        }
        // Covers cells no seed reaches too, which the fill comparison alone would not see.
        let mask = inside_mask(ring, (min_x, max_x, min_z, max_z), None, Instant::now()).unwrap();
        let polygon = polygon_of(ring);
        for z in min_z..=max_z {
            for x in min_x..=max_x {
                let inside = polygon.contains(&Point::new(x as f64, z as f64));
                assert_eq!(mask.contains(x, z), inside, "({x}, {z}) for {label}");
            }
        }
        got
    }

    // Star-shaped around its centre, so simple up to rounding.
    fn random_star(
        rng: &mut ChaCha8Rng,
        vertices: std::ops::Range<usize>,
        r_min: f64,
        r_max: f64,
    ) -> Vec<(i32, i32)> {
        let vertices = rng.random_range(vertices);
        let mut angles: Vec<f64> = (0..vertices)
            .map(|_| rng.random::<f64>() * std::f64::consts::TAU)
            .collect();
        angles.sort_by(f64::total_cmp);
        closed(
            angles
                .iter()
                .map(|a| {
                    let r = rng.random_range(r_min..r_max);
                    ((r * a.cos()).round() as i32, (r * a.sin()).round() as i32)
                })
                .collect(),
        )
    }

    // A histogram outline: only horizontal and vertical edges, many sharing rows.
    fn random_skyline(
        rng: &mut ChaCha8Rng,
        bars: std::ops::Range<i32>,
        bar_width: std::ops::Range<i32>,
        max_height: i32,
    ) -> Vec<(i32, i32)> {
        let (bars, bar_width) = (rng.random_range(bars), rng.random_range(bar_width));
        let mut ring = vec![(0, 0)];
        for i in 0..bars {
            let h = rng.random_range(1..=max_height);
            ring.push((i * bar_width, h));
            ring.push(((i + 1) * bar_width, h));
        }
        ring.push((bars * bar_width, 0));
        closed(ring)
    }

    #[test]
    fn random_simple_polygons_fill_like_contains() {
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        for i in 0..40 {
            // Every fourth one has a bbox past 50k, so both seed grids are covered.
            let (r_min, r_max) = if i % 4 == 0 {
                (60.0, 160.0)
            } else {
                (5.0, 70.0)
            };
            let ring = random_star(&mut rng, 3..40, r_min, r_max);
            let ring = if i % 2 == 0 { reversed(&ring) } else { ring };
            assert_matches_reference(&ring);
        }
        for i in 0..12 {
            let ring = random_skyline(&mut rng, 2..60, 1..6, 120);
            let ring = if i % 2 == 0 { reversed(&ring) } else { ring };
            assert_matches_reference(&ring);
        }
    }

    #[test]
    fn self_intersecting_polygons_fill_like_contains() {
        // Star polygons {n/k}; the core has winding k, which nonzero still fills.
        for (n, k, r) in [
            (5, 2, 60.0),
            (7, 3, 90.0),
            (8, 3, 40.0),
            (9, 4, 170.0),
            (12, 5, 30.0),
        ] {
            let ring: Vec<(i32, i32)> = (0..n)
                .map(|i| {
                    let a = (i * k) as f64 / n as f64 * std::f64::consts::TAU;
                    ((r * a.cos()).round() as i32, (r * a.sin()).round() as i32)
                })
                .collect();
            let filled = assert_matches_reference(&closed(ring));
            assert!(filled.contains(&(0, 0)), "{n}/{k} star core is filled");
        }

        // Figure eight, a loop walked twice, and a loop with a backwards inner loop.
        assert_matches_reference(&closed(vec![(0, 0), (40, 30), (40, 0), (0, 30)]));
        let square = [(0, 0), (30, 0), (30, 30), (0, 30), (0, 0)];
        assert_matches_reference(&[&square[..], &square[1..]].concat());
        let looped = [
            (0, 0),
            (60, 0),
            (60, 60),
            (10, 60),
            (10, 20),
            (40, 20),
            (40, 40),
            (0, 40),
        ];
        assert_matches_reference(&closed(looped.to_vec()));

        // Random vertex soup crosses itself everywhere.
        let mut rng = ChaCha8Rng::seed_from_u64(2);
        for i in 0..30 {
            let size = if i % 5 == 0 { 300 } else { 90 };
            let ring: Vec<(i32, i32)> = (0..rng.random_range(3..25))
                .map(|_| (rng.random_range(0..size), rng.random_range(0..size)))
                .collect();
            assert_matches_reference(&closed(ring));
        }
    }

    #[test]
    fn polygons_with_holes_fill_like_contains() {
        // Holes reach the fill as one ring: out along a bridge, round the hole, back the same way.
        let outer = [(0, 0), (80, 0), (80, 80), (0, 80), (0, 0)];
        let hole_cw = [(20, 20), (20, 60), (60, 60), (60, 20), (20, 20)];
        let hole_ccw = reversed(&hole_cw);

        let with_hole = [&outer[..], &hole_cw[..], &outer[..1]].concat();
        let filled = assert_matches_reference(&with_hole);
        assert!(!filled.contains(&(40, 40)), "the hole stays empty");
        assert!(filled.contains(&(70, 70)));
        assert_matches_reference(&reversed(&with_hole));

        // The same ring wound the same way adds winding instead, so it stays filled.
        let doubled = [&outer[..], &hole_ccw[..], &outer[..1]].concat();
        assert!(assert_matches_reference(&doubled).contains(&(40, 40)));

        // Two holes, an island inside a hole, and a hole touching the outer ring at a corner.
        let second_hole = [(65, 5), (65, 15), (75, 15), (75, 5), (65, 5)];
        assert_matches_reference(
            &[
                &outer[..],
                &hole_cw[..],
                &outer[..1],
                &second_hole[..],
                &outer[..1],
            ]
            .concat(),
        );
        let island = [(30, 30), (50, 30), (50, 50), (30, 50), (30, 30)];
        assert_matches_reference(
            &[
                &outer[..],
                &hole_cw[..],
                &island[..],
                &hole_cw[..1],
                &outer[..1],
            ]
            .concat(),
        );
        let corner_hole = [(0, 0), (0, 30), (30, 30), (30, 0), (0, 0)];
        assert_matches_reference(&[&outer[..], &corner_hole[1..]].concat());

        // A bigger keyhole past 50k, on the coarse seed grid.
        let big = [(0, 0), (400, 0), (400, 300), (0, 300), (0, 0)];
        let big_hole = [(100, 100), (100, 200), (300, 200), (300, 100), (100, 100)];
        assert_matches_reference(&[&big[..], &big_hole[..], &big[..1]].concat());
    }

    #[test]
    fn collinear_and_degenerate_edges_fill_like_contains() {
        #[rustfmt::skip]
        let cases: Vec<Vec<(i32, i32)>> = vec![
            // Repeated vertices and zero-length edges.
            vec![(0, 0), (0, 0), (20, 0), (20, 0), (20, 20), (0, 20), (0, 20), (0, 0)],
            // Collinear midpoints along every side.
            vec![(0, 0), (7, 0), (13, 0), (20, 0), (20, 9), (20, 20), (10, 20), (0, 20), (0, 10), (0, 0)],
            vec![(0, 0), (10, 5), (20, 10), (30, 15), (0, 30), (0, 0)],
            // Spikes walked out and straight back, outward and inward.
            vec![(0, 0), (20, 0), (20, 10), (35, 10), (20, 10), (20, 20), (0, 20), (0, 0)],
            vec![(0, 0), (20, 0), (20, 10), (8, 13), (20, 10), (20, 20), (0, 20), (0, 0)],
            // Notches whose tip points into the interior, up and down.
            vec![(0, 0), (10, 0), (15, 12), (20, 0), (30, 0), (30, 20), (0, 20), (0, 0)],
            vec![(0, 0), (30, 0), (30, 20), (20, 20), (15, 7), (10, 20), (0, 20), (0, 0)],
            // A vertex resting on another edge, and two lobes meeting at one point.
            vec![(0, 0), (30, 0), (30, 20), (15, 0), (0, 20), (0, 0)],
            vec![(0, 0), (10, 10), (20, 0), (20, 20), (10, 10), (0, 20), (0, 0)],
            // Horizontal edges stacked on shared rows.
            vec![(0, 0), (40, 0), (40, 10), (30, 10), (30, 5), (20, 5), (20, 10), (10, 10), (10, 5), (0, 5), (0, 0)],
            // No area at all.
            vec![(0, 0), (30, 15), (60, 30), (0, 0)],
            vec![(0, 0), (25, 0), (0, 0)],
            vec![(5, 5), (5, 5), (5, 5)],
            // Not closed, or too short.
            vec![(0, 0), (20, 0), (20, 20), (0, 20)],
            vec![(0, 0), (0, 0)],
        ];
        for ring in &cases {
            assert_matches_reference(ring);
            assert_matches_reference(&reversed(ring));
        }
    }

    #[test]
    fn huge_coordinates_fill_like_contains() {
        let mut rng = ChaCha8Rng::seed_from_u64(3);
        let corners = [
            (i32::MAX - 200, i32::MAX - 200),
            (i32::MIN + 200, i32::MIN + 200),
            (i32::MAX - 200, i32::MIN + 200),
            (2_000_000_123, -1_999_999_877),
        ];
        for &(cx, cz) in &corners {
            for _ in 0..3 {
                let star = random_star(&mut rng, 3..20, 20.0, 200.0);
                assert_matches_reference(&shifted(&star, cx, cz));
            }
            let thin = closed(vec![(-200, -200), (200, 199), (200, 200), (-200, -199)]);
            assert_matches_reference(&shifted(&thin, cx, cz));
        }
        // Touching the very edge of the i32 range on both axes.
        assert_matches_reference(&closed(vec![
            (i32::MAX, i32::MAX),
            (i32::MAX - 150, i32::MAX - 20),
            (i32::MAX - 60, i32::MAX - 140),
        ]));
        assert_matches_reference(&closed(vec![
            (i32::MIN, i32::MIN),
            (i32::MIN + 150, i32::MIN + 20),
            (i32::MIN + 60, i32::MIN + 140),
        ]));
    }

    #[test]
    fn thin_slivers_fill_like_contains() {
        #[rustfmt::skip]
        let cases: Vec<Vec<(i32, i32)>> = vec![
            // Under a block wide, so the inside cells are scattered singletons.
            closed(vec![(0, 0), (300, 100), (300, 101), (0, 1)]),
            closed(vec![(0, 0), (997, 1000), (998, 1000)]),
            closed(vec![(0, 0), (1, 0), (400, 399)]),
            // Needles and a one-cell corridor.
            closed(vec![(0, 0), (500, 3), (0, 6)]),
            closed(vec![(0, 0), (2, 0), (2, 300), (0, 300)]),
            closed(vec![(0, 0), (600, 1), (600, 3), (0, 2)]),
            // Long diagonal bands, with a bbox past 50k so the coarse grid seeds them.
            closed(vec![(0, 0), (420, 400), (420, 403), (0, 3)]),
            closed(vec![(0, 0), (700, 300), (701, 302), (1, 2)]),
            // A zigzag of slivers.
            closed(vec![(0, 0), (50, 200), (100, 0), (150, 200), (200, 0), (200, 2), (150, 203), (100, 3), (50, 203), (0, 3)]),
        ];
        for ring in &cases {
            assert_matches_reference(ring);
            assert_matches_reference(&reversed(ring));
        }
    }

    #[test]
    fn timeouts_keep_their_meaning() {
        let small = closed(vec![(0, 0), (100, 0), (100, 100), (0, 100)]);
        let large = closed(vec![(0, 0), (400, 0), (400, 400), (0, 400)]);
        for ring in [&small, &large] {
            assert!(flood_fill_area(ring, Some(&Duration::ZERO)).is_empty());
            let generous = Duration::from_secs(600);
            assert_eq!(
                flood_fill_area(ring, Some(&generous)),
                flood_fill_area(ring, None)
            );
        }
    }

    #[test]
    fn fill_capacity_matches_its_length() {
        let ring = closed(vec![(0, 0), (300, 100), (300, 101), (0, 1)]);
        let filled = flood_fill_area(&ring, None);
        assert_eq!(filled.capacity(), filled.len());
    }

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
