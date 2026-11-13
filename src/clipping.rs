// Sutherland-Hodgman polygon clipping and related geometry utilities.
//
// Provides bbox clipping for polygons, polylines, and water rings.

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::osm_parser::ProcessedNode;
use std::collections::HashMap;

/// Where the ids invented for clipped corners live.
///
/// A vertex the clipper creates has no OSM node behind it and still needs an
/// id, and the facade projection now identifies a wall by the ids of its two
/// ends. `way_id * 10_000_000 + i` alone lands inside the real node id range
/// for a low way id (OSM is around 1.3e10 today), so an invented corner could
/// collide with a real node somewhere else in the world. Everything invented
/// sits above this instead, which is four orders of magnitude clear of OSM and
/// still inside the positive half of an i64 for anything that serialises ids.
const INVENTED_NODE_BASE: u64 = 1 << 62;

/// The id for the `i`th vertex the clipper invented on `way_id`.
fn invented_node_id(way_id: u64, i: u64) -> u64 {
    INVENTED_NODE_BASE | (way_id.wrapping_mul(10000000).wrapping_add(i) & (INVENTED_NODE_BASE - 1))
}

/// Whether `id` was invented by the clipper rather than read from OSM.
pub fn is_invented_node_id(id: u64) -> bool {
    id >= INVENTED_NODE_BASE
}

/// Gives a clipped open way's ends that moved off `ends` an invented id. The
/// polyline clip lends a cut end the id of the node up to 50 blocks beyond it.
pub fn mark_cut_ends(nodes: &mut [ProcessedNode], ends: [(i32, i32); 2], way_id: u64) {
    let last = nodes.len().saturating_sub(1);
    for (i, end) in [(0, ends[0]), (last, ends[1])] {
        if let Some(n) = nodes.get_mut(i).filter(|n| (n.x, n.z) != end) {
            n.id = invented_node_id(way_id, i as u64);
        }
    }
}

/// Invented id indices for outline cuts, clear of a rectangle's, which count the way's nodes.
const OUTLINE_CUT_BASE: u64 = 9_000_000;

/// How far a line traced along the outline may waver across it without leaving.
const OUTLINE_SLACK: i32 = 2;

/// Shortest run cut at both ends worth keeping, in blocks; a grazed corner leaves nowhere to go.
const MIN_CUT_RUN: f64 = 12.0;

/// The runs of a line inside a drawn outline, cut ends getting invented ids as at a rectangle's
/// edge; `None` when nothing is cut. `cuts` numbers the cuts of this way across calls.
pub fn cut_line_to_outline(
    way_id: u64,
    points: &[(u64, f64, f64)],
    world: &XZBBox,
    cuts: &mut u64,
) -> Option<Vec<Vec<(u64, f64, f64)>>> {
    world.outline()?;
    let block = |x: f64, z: f64| (x.floor() as i32, z.floor() as i32);
    let inside = |x: f64, z: f64| {
        let (bx, bz) = block(x, z);
        world.contains(&XZPoint::new(bx, bz))
    };
    let r = OUTLINE_SLACK;
    // A column with the outline on two opposite sides is a gap between areas, not a waver.
    let between = |bx: i32, bz: i32| {
        let hit = |dx: i32, dz: i32| {
            (1..=2 * r).any(|k| world.contains(&XZPoint::new(bx + dx * k, bz + dz * k)))
        };
        (hit(1, 0) && hit(-1, 0)) || (hit(0, 1) && hit(0, -1))
    };
    let near = |x: f64, z: f64| {
        let (bx, bz) = block(x, z);
        world.touches_rect(bx - r, bz - r, bx + r, bz + r) && !between(bx, bz)
    };
    let before = *cuts;
    // Invented ids drop a way id's top bits, which mark lines of our own, as across squares.
    let base = OUTLINE_CUT_BASE + (way_id >> 62) * 250_000;
    let mut cut_at = |x: f64, z: f64| {
        *cuts += 1;
        (invented_node_id(way_id, base + *cuts), x, z)
    };
    let mut runs: Vec<Vec<(u64, f64, f64)>> = Vec::new();
    let mut run: Vec<(u64, f64, f64)> = Vec::new();
    let mut starts_cut = false;
    let Some(&first) = points.first() else {
        return Some(runs);
    };
    // A line starting just outside is cut where it enters, as one ending there is where it leaves.
    let mut is_in = inside(first.1, first.2);
    // Last point truly inside with the run length there, and the first sample out after it.
    let mut last_in = (first.1, first.2, 1usize);
    let mut after_in: Option<(f64, f64)> = None;
    if is_in {
        run.push(first);
    }
    let boundary = |(ax, az): (f64, f64), (bx, bz): (f64, f64)| {
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..16 {
            let mid = (lo + hi) / 2.0;
            if inside(ax + (bx - ax) * mid, az + (bz - az) * mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (ax + (bx - ax) * lo, az + (bz - az) * lo)
    };
    let mut close = |run: &mut Vec<(u64, f64, f64)>, starts_cut: bool, ends_cut: bool| {
        let length: f64 = run
            .windows(2)
            .map(|w| (w[1].1 - w[0].1).hypot(w[1].2 - w[0].2))
            .sum();
        if run.len() >= 2 && !(starts_cut && ends_cut && length < MIN_CUT_RUN) {
            runs.push(std::mem::take(run));
        }
        run.clear();
    };
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let at = |t: f64| (a.1 + (b.1 - a.1) * t, a.2 + (b.2 - a.2) * t);
        // Half a block apart, so no block the line crosses is stepped over.
        let steps = ((b.1 - a.1).hypot(b.2 - a.2) * 2.0).ceil().max(1.0) as usize;
        let mut t0 = 0.0;
        for s in 1..=steps {
            let t1 = s as f64 / steps as f64;
            let (x, z) = at(t1);
            if is_in {
                if inside(x, z) {
                    // The last sample is the node itself, recorded once it is in the run.
                    if s < steps {
                        last_in = (x, z, run.len());
                    }
                    after_in = None;
                } else {
                    let out = *after_in.get_or_insert((x, z));
                    if !near(x, z) {
                        let (lx, lz, len) = last_in;
                        let (cx, cz) = boundary((lx, lz), out);
                        run.truncate(len);
                        run.push(cut_at(cx, cz));
                        close(&mut run, starts_cut, true);
                        is_in = false;
                    }
                }
            } else if inside(x, z) {
                let (cx, cz) = boundary((x, z), at(t0));
                run.push(cut_at(cx, cz));
                starts_cut = true;
                last_in = (x, z, run.len());
                after_in = None;
                is_in = true;
            }
            t0 = t1;
        }
        if is_in {
            run.push(b);
            if inside(b.1, b.2) {
                last_in = (b.1, b.2, run.len());
                after_in = None;
            }
        }
    }
    if is_in {
        match after_in {
            // A line ending just outside ends on the boundary.
            Some(out) => {
                let (lx, lz, len) = last_in;
                let (cx, cz) = boundary((lx, lz), out);
                run.truncate(len);
                run.push(cut_at(cx, cz));
                close(&mut run, starts_cut, true);
            }
            None => close(&mut run, starts_cut, false),
        }
    }
    if *cuts == before && runs.len() == 1 && runs[0].len() == points.len() {
        return None;
    }
    Some(runs)
}

/// [`cut_line_to_outline`] for a way, cut ends as untagged nodes of their own.
pub fn cut_way_to_outline(
    way: &crate::osm_parser::ProcessedWay,
    world: &XZBBox,
) -> Option<Vec<crate::osm_parser::ProcessedWay>> {
    let points: Vec<(u64, f64, f64)> = way
        .nodes
        .iter()
        .map(|n| (n.id, n.x as f64 + 0.5, n.z as f64 + 0.5))
        .collect();
    let runs = cut_line_to_outline(way.id, &points, world, &mut 0)?;
    let by_id: HashMap<u64, &ProcessedNode> = way.nodes.iter().map(|n| (n.id, n)).collect();
    Some(
        runs.into_iter()
            .map(|run| crate::osm_parser::ProcessedWay {
                id: way.id,
                tags: way.tags.clone(),
                nodes: run
                    .into_iter()
                    .map(|(id, x, z)| match by_id.get(&id) {
                        Some(&n) if !is_invented_node_id(id) => n.clone(),
                        _ => ProcessedNode {
                            id,
                            tags: HashMap::new(),
                            x: x.floor() as i32,
                            z: z.floor() as i32,
                        },
                    })
                    .collect(),
            })
            .collect(),
    )
}

/// Ways cut to a drawn outline in their own order; uncut ways pass through borrowed.
pub fn cut_ways_to_outline<'a, T: Copy>(
    ways: Vec<(&'a crate::osm_parser::ProcessedWay, T)>,
    world: &XZBBox,
) -> Vec<(std::borrow::Cow<'a, crate::osm_parser::ProcessedWay>, T)> {
    use std::borrow::Cow;
    if world.outline().is_none() {
        return ways
            .into_iter()
            .map(|(w, t)| (Cow::Borrowed(w), t))
            .collect();
    }
    let mut out = Vec::with_capacity(ways.len());
    for (w, t) in ways {
        match cut_way_to_outline(w, world) {
            None => out.push((Cow::Borrowed(w), t)),
            Some(pieces) => out.extend(pieces.into_iter().map(|p| (Cow::Owned(p), t))),
        }
    }
    out
}

/// Clips a way to the bounding box using Sutherland-Hodgman for polygons or
/// simple line clipping for polylines. Preserves endpoint IDs for ring assembly.
/// `clip_way_to_bbox` for an owned way: one lying wholly inside comes back as is, uncopied.
pub fn clip_way_to_bbox_owned(nodes: Vec<ProcessedNode>, xzbbox: &XZBBox) -> Vec<ProcessedNode> {
    if nodes
        .iter()
        .all(|node| xzbbox.contains(&XZPoint::new(node.x, node.z)))
    {
        return nodes;
    }
    clip_way_to_bbox(&nodes, xzbbox)
}

pub fn clip_way_to_bbox(nodes: &[ProcessedNode], xzbbox: &XZBBox) -> Vec<ProcessedNode> {
    if nodes.is_empty() {
        return Vec::new();
    }

    // Get way ID for ID generation
    let way_id = nodes.first().map(|n| n.id).unwrap_or(0);

    let is_closed = is_closed_polygon(nodes);

    if !is_closed {
        return clip_polyline_to_bbox(nodes, xzbbox);
    }

    // If all nodes are inside the bbox, return unchanged
    let has_nodes_outside = nodes
        .iter()
        .any(|node| !xzbbox.contains(&XZPoint::new(node.x, node.z)));

    if !has_nodes_outside {
        return nodes.to_vec();
    }

    let min_x = xzbbox.min_x() as f64;
    let min_z = xzbbox.min_z() as f64;
    let max_x = xzbbox.max_x() as f64;
    let max_z = xzbbox.max_z() as f64;

    let mut polygon: Vec<(f64, f64)> = nodes.iter().map(|n| (n.x as f64, n.z as f64)).collect();

    polygon = clip_polygon_sutherland_hodgman(polygon, min_x, min_z, max_x, max_z);

    if polygon.len() < 3 {
        return Vec::new();
    }

    // Final clamping for floating-point errors
    for p in &mut polygon {
        p.0 = p.0.clamp(min_x, max_x);
        p.1 = p.1.clamp(min_z, max_z);
    }

    let polygon = remove_consecutive_duplicates(polygon);
    if polygon.len() < 3 {
        return Vec::new();
    }

    // Re-close the polygon: SH output is implicitly closed, and dedup may
    // have removed the explicit closing point. Re-adding it preserves the
    // closure signal so downstream code (flood fill) can distinguish closed
    // polygons from open polylines -- open polylines must never be flood-filled
    // because geo::Polygon auto-closure would create a diagonal artifact edge.
    let mut polygon = polygon;
    if polygon.len() >= 3 {
        polygon.push(polygon[0]);
    }

    assign_node_ids_preserving_endpoints(nodes, polygon, way_id)
}

/// Clips a water polygon ring to bbox using Sutherland-Hodgman (post-ring-merge).
pub fn clip_water_ring_to_bbox(
    ring: &[ProcessedNode],
    xzbbox: &XZBBox,
) -> Option<Vec<ProcessedNode>> {
    if ring.is_empty() {
        return None;
    }

    let min_x = xzbbox.min_x() as f64;
    let min_z = xzbbox.min_z() as f64;
    let max_x = xzbbox.max_x() as f64;
    let max_z = xzbbox.max_z() as f64;

    // Check if entire ring is inside bbox
    let all_inside = ring.iter().all(|n| {
        n.x as f64 >= min_x && n.x as f64 <= max_x && n.z as f64 >= min_z && n.z as f64 <= max_z
    });

    if all_inside {
        return Some(ring.to_vec());
    }

    // Check if entire ring is outside bbox
    if is_ring_outside_bbox(ring, min_x, min_z, max_x, max_z) {
        return None;
    }

    // Refuse open fragments; chord-closing them here would fabricate a closed
    // wedge whose synthetic ids defeat the callers' post-clip closure checks.
    let first = &ring[0];
    let last = ring.last().unwrap();
    let closed =
        first.id == last.id || ((first.x - last.x).abs() <= 1 && (first.z - last.z).abs() <= 1);
    if !closed {
        return None;
    }

    // Convert to f64 coordinates and ensure closed
    let mut polygon: Vec<(f64, f64)> = ring.iter().map(|n| (n.x as f64, n.z as f64)).collect();
    if !polygon.is_empty() && polygon.first() != polygon.last() {
        polygon.push(polygon[0]);
    }

    // Clip with full-range clamping (water uses simpler approach)
    polygon = clip_polygon_sutherland_hodgman_simple(polygon, min_x, min_z, max_x, max_z);

    if polygon.len() < 3 {
        return None;
    }

    // Verify all points are within bbox
    let all_points_inside = polygon
        .iter()
        .all(|&(x, z)| x >= min_x && x <= max_x && z >= min_z && z <= max_z);

    if !all_points_inside {
        eprintln!("ERROR: clip_water_ring_to_bbox produced points outside bbox!");
        return None;
    }

    // Convert back to ProcessedNode with synthetic IDs
    let mut result: Vec<ProcessedNode> = polygon
        .iter()
        .enumerate()
        .map(|(i, &(x, z))| ProcessedNode {
            id: 1_000_000_000 + i as u64,
            tags: HashMap::new(),
            x: x.clamp(min_x, max_x).round() as i32,
            z: z.clamp(min_z, max_z).round() as i32,
        })
        .collect();

    // Close the loop by matching first and last ID
    if !result.is_empty() {
        let first_id = result[0].id;
        result.last_mut().unwrap().id = first_id;
    }

    Some(result)
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Checks if a way forms a closed polygon.
fn is_closed_polygon(nodes: &[ProcessedNode]) -> bool {
    if nodes.len() < 3 {
        return false;
    }
    let first = nodes.first().unwrap();
    let last = nodes.last().unwrap();
    first.id == last.id || (first.x == last.x && first.z == last.z)
}

/// Checks if an entire ring is outside the bbox.
fn is_ring_outside_bbox(
    ring: &[ProcessedNode],
    min_x: f64,
    min_z: f64,
    max_x: f64,
    max_z: f64,
) -> bool {
    let all_left = ring.iter().all(|n| (n.x as f64) < min_x);
    let all_right = ring.iter().all(|n| (n.x as f64) > max_x);
    let all_top = ring.iter().all(|n| (n.z as f64) < min_z);
    let all_bottom = ring.iter().all(|n| (n.z as f64) > max_z);
    all_left || all_right || all_top || all_bottom
}

/// Clips a polyline (open path) to the bounding box.
fn clip_polyline_to_bbox(nodes: &[ProcessedNode], xzbbox: &XZBBox) -> Vec<ProcessedNode> {
    if nodes.is_empty() {
        return Vec::new();
    }

    let min_x = xzbbox.min_x() as f64;
    let min_z = xzbbox.min_z() as f64;
    let max_x = xzbbox.max_x() as f64;
    let max_z = xzbbox.max_z() as f64;

    let mut result = Vec::new();

    for i in 0..nodes.len() {
        let current = &nodes[i];
        let current_point = (current.x as f64, current.z as f64);
        let current_inside = point_in_bbox(current_point, min_x, min_z, max_x, max_z);

        if current_inside {
            result.push(current.clone());
        }

        if i + 1 < nodes.len() {
            let next = &nodes[i + 1];
            let next_point = (next.x as f64, next.z as f64);
            let next_inside = point_in_bbox(next_point, min_x, min_z, max_x, max_z);

            if current_inside != next_inside {
                // One endpoint inside, one outside, find single intersection
                let intersections =
                    find_bbox_intersections(current_point, next_point, min_x, min_z, max_x, max_z);

                for intersection in intersections {
                    let synthetic_id = invented_node_id(nodes[0].id, result.len() as u64);
                    result.push(ProcessedNode {
                        id: synthetic_id,
                        x: intersection.0.round() as i32,
                        z: intersection.1.round() as i32,
                        tags: HashMap::new(),
                    });
                }
            } else if !current_inside && !next_inside {
                // Both endpoints outside, segment might still cross through bbox
                let mut intersections =
                    find_bbox_intersections(current_point, next_point, min_x, min_z, max_x, max_z);

                if intersections.len() >= 2 {
                    // Sort intersections by distance from current point
                    intersections.sort_by(|a, b| {
                        let dist_a =
                            (a.0 - current_point.0).powi(2) + (a.1 - current_point.1).powi(2);
                        let dist_b =
                            (b.0 - current_point.0).powi(2) + (b.1 - current_point.1).powi(2);
                        dist_a
                            .partial_cmp(&dist_b)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });

                    for intersection in intersections {
                        let synthetic_id = invented_node_id(nodes[0].id, result.len() as u64);
                        result.push(ProcessedNode {
                            id: synthetic_id,
                            x: intersection.0.round() as i32,
                            z: intersection.1.round() as i32,
                            tags: HashMap::new(),
                        });
                    }
                }
            }
        }
    }

    // Preserve endpoint IDs where possible
    if result.len() >= 2 {
        let tolerance = 50.0;
        if let Some(first_orig) = nodes.first() {
            if matches_endpoint(
                (result[0].x as f64, result[0].z as f64),
                first_orig,
                tolerance,
            ) {
                result[0].id = first_orig.id;
            }
        }
        if let Some(last_orig) = nodes.last() {
            let last_idx = result.len() - 1;
            if matches_endpoint(
                (result[last_idx].x as f64, result[last_idx].z as f64),
                last_orig,
                tolerance,
            ) {
                result[last_idx].id = last_orig.id;
            }
        }
    }

    result
}

/// Sutherland-Hodgman polygon clipping with edge-specific clamping.
fn clip_polygon_sutherland_hodgman(
    mut polygon: Vec<(f64, f64)>,
    min_x: f64,
    min_z: f64,
    max_x: f64,
    max_z: f64,
) -> Vec<(f64, f64)> {
    // Edges: bottom, right, top, left (counter-clockwise traversal)
    let bbox_edges = [
        (min_x, min_z, max_x, min_z, 0), // Bottom: clamp z
        (max_x, min_z, max_x, max_z, 1), // Right: clamp x
        (max_x, max_z, min_x, max_z, 2), // Top: clamp z
        (min_x, max_z, min_x, min_z, 3), // Left: clamp x
    ];

    for (edge_x1, edge_z1, edge_x2, edge_z2, edge_idx) in bbox_edges {
        if polygon.is_empty() {
            break;
        }

        let mut clipped = Vec::new();
        let is_closed = !polygon.is_empty() && polygon.first() == polygon.last();
        let edge_count = if is_closed {
            polygon.len().saturating_sub(1)
        } else {
            polygon.len()
        };

        for i in 0..edge_count {
            let current = polygon[i];
            let next = polygon.get(i + 1).copied().unwrap_or(polygon[0]);

            let current_inside = point_inside_edge(current, edge_x1, edge_z1, edge_x2, edge_z2);
            let next_inside = point_inside_edge(next, edge_x1, edge_z1, edge_x2, edge_z2);

            if next_inside {
                if !current_inside {
                    if let Some(mut intersection) = line_edge_intersection(
                        current.0, current.1, next.0, next.1, edge_x1, edge_z1, edge_x2, edge_z2,
                    ) {
                        // Clamp to current edge only
                        match edge_idx {
                            0 => intersection.1 = min_z,
                            1 => intersection.0 = max_x,
                            2 => intersection.1 = max_z,
                            3 => intersection.0 = min_x,
                            _ => {}
                        }
                        clipped.push(intersection);
                    }
                }
                clipped.push(next);
            } else if current_inside {
                if let Some(mut intersection) = line_edge_intersection(
                    current.0, current.1, next.0, next.1, edge_x1, edge_z1, edge_x2, edge_z2,
                ) {
                    match edge_idx {
                        0 => intersection.1 = min_z,
                        1 => intersection.0 = max_x,
                        2 => intersection.1 = max_z,
                        3 => intersection.0 = min_x,
                        _ => {}
                    }
                    clipped.push(intersection);
                }
            }
        }

        polygon = clipped;
    }

    polygon
}

/// Sutherland-Hodgman with full bbox clamping (simpler, for water rings).
fn clip_polygon_sutherland_hodgman_simple(
    mut polygon: Vec<(f64, f64)>,
    min_x: f64,
    min_z: f64,
    max_x: f64,
    max_z: f64,
) -> Vec<(f64, f64)> {
    let bbox_edges = [
        (min_x, min_z, max_x, min_z),
        (max_x, min_z, max_x, max_z),
        (max_x, max_z, min_x, max_z),
        (min_x, max_z, min_x, min_z),
    ];

    for (edge_x1, edge_z1, edge_x2, edge_z2) in bbox_edges {
        if polygon.is_empty() {
            break;
        }

        let mut clipped = Vec::new();
        let is_closed = !polygon.is_empty() && polygon.first() == polygon.last();
        let edge_count = if is_closed {
            polygon.len().saturating_sub(1)
        } else {
            polygon.len()
        };

        for i in 0..edge_count {
            let current = polygon[i];
            let next = polygon.get(i + 1).copied().unwrap_or(polygon[0]);

            let current_inside = point_inside_edge(current, edge_x1, edge_z1, edge_x2, edge_z2);
            let next_inside = point_inside_edge(next, edge_x1, edge_z1, edge_x2, edge_z2);

            if next_inside {
                if !current_inside {
                    if let Some(mut intersection) = line_edge_intersection(
                        current.0, current.1, next.0, next.1, edge_x1, edge_z1, edge_x2, edge_z2,
                    ) {
                        intersection.0 = intersection.0.clamp(min_x, max_x);
                        intersection.1 = intersection.1.clamp(min_z, max_z);
                        clipped.push(intersection);
                    }
                }
                clipped.push(next);
            } else if current_inside {
                if let Some(mut intersection) = line_edge_intersection(
                    current.0, current.1, next.0, next.1, edge_x1, edge_z1, edge_x2, edge_z2,
                ) {
                    intersection.0 = intersection.0.clamp(min_x, max_x);
                    intersection.1 = intersection.1.clamp(min_z, max_z);
                    clipped.push(intersection);
                }
            }
        }

        polygon = clipped;
    }

    polygon
}

/// Checks if point is inside bbox.
fn point_in_bbox(point: (f64, f64), min_x: f64, min_z: f64, max_x: f64, max_z: f64) -> bool {
    point.0 >= min_x && point.0 <= max_x && point.1 >= min_z && point.1 <= max_z
}

/// Checks if point is on the "inside" side of an edge (cross product test).
fn point_inside_edge(
    point: (f64, f64),
    edge_x1: f64,
    edge_z1: f64,
    edge_x2: f64,
    edge_z2: f64,
) -> bool {
    let edge_dx = edge_x2 - edge_x1;
    let edge_dz = edge_z2 - edge_z1;
    let point_dx = point.0 - edge_x1;
    let point_dz = point.1 - edge_z1;
    (edge_dx * point_dz - edge_dz * point_dx) >= 0.0
}

/// Finds intersection between a line segment and an edge.
#[allow(clippy::too_many_arguments)]
fn line_edge_intersection(
    line_x1: f64,
    line_z1: f64,
    line_x2: f64,
    line_z2: f64,
    edge_x1: f64,
    edge_z1: f64,
    edge_x2: f64,
    edge_z2: f64,
) -> Option<(f64, f64)> {
    let line_dx = line_x2 - line_x1;
    let line_dz = line_z2 - line_z1;
    let edge_dx = edge_x2 - edge_x1;
    let edge_dz = edge_z2 - edge_z1;

    let denom = line_dx * edge_dz - line_dz * edge_dx;
    if denom.abs() < 1e-10 {
        return None;
    }

    let dx = edge_x1 - line_x1;
    let dz = edge_z1 - line_z1;
    let t = (dx * edge_dz - dz * edge_dx) / denom;

    if (0.0..=1.0).contains(&t) {
        Some((line_x1 + t * line_dx, line_z1 + t * line_dz))
    } else {
        None
    }
}

/// Finds intersections between a line segment and bbox edges.
fn find_bbox_intersections(
    start: (f64, f64),
    end: (f64, f64),
    min_x: f64,
    min_z: f64,
    max_x: f64,
    max_z: f64,
) -> Vec<(f64, f64)> {
    let mut intersections = Vec::new();

    let bbox_edges = [
        (min_x, min_z, max_x, min_z),
        (max_x, min_z, max_x, max_z),
        (max_x, max_z, min_x, max_z),
        (min_x, max_z, min_x, min_z),
    ];

    for (edge_x1, edge_z1, edge_x2, edge_z2) in bbox_edges {
        if let Some(intersection) = line_edge_intersection(
            start.0, start.1, end.0, end.1, edge_x1, edge_z1, edge_x2, edge_z2,
        ) {
            let on_edge = point_in_bbox(intersection, min_x, min_z, max_x, max_z)
                && ((intersection.0 == min_x || intersection.0 == max_x)
                    || (intersection.1 == min_z || intersection.1 == max_z));

            if on_edge {
                intersections.push(intersection);
            }
        }
    }

    intersections
}

/// Removes consecutive duplicate points (within epsilon tolerance).
fn remove_consecutive_duplicates(polygon: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    if polygon.is_empty() {
        return polygon;
    }

    let eps = 0.1;
    let mut result: Vec<(f64, f64)> = Vec::with_capacity(polygon.len());

    for p in &polygon {
        if let Some(last) = result.last() {
            if (p.0 - last.0).abs() < eps && (p.1 - last.1).abs() < eps {
                continue;
            }
        }
        result.push(*p);
    }

    // Check first/last duplicates for closed polygons
    if result.len() > 1 {
        let first = result.first().unwrap();
        let last = result.last().unwrap();
        if (first.0 - last.0).abs() < eps && (first.1 - last.1).abs() < eps {
            result.pop();
        }
    }

    result
}

/// Checks if a clipped coordinate matches an original endpoint.
fn matches_endpoint(coord: (f64, f64), endpoint: &ProcessedNode, tolerance: f64) -> bool {
    let dx = (coord.0 - endpoint.x as f64).abs();
    let dz = (coord.1 - endpoint.z as f64).abs();
    dx * dx + dz * dz < tolerance * tolerance
}

/// Assigns node IDs to clipped coordinates, keeping the ID of every original
/// node that survived the clip and inventing one only for the vertices
/// Sutherland-Hodgman actually created.
///
/// A vertex the clipper kept is the same OSM node it was, at the same block, so
/// it keeps its ID: anything downstream that identifies an edge by its two node
/// IDs (the facade projection does, and it is the only consumer that can tell)
/// still finds the edges of a ring that only lost a corner. Inventing an ID for
/// every vertex, as this used to, renamed a whole ring the moment one node fell
/// outside the world.
///
/// It replaces a 50-block proximity rule that named the first and last clipped
/// vertex after the way's own first and last node. With the exact match, a
/// vertex that really is that node has already been named by it, so all the
/// proximity rule could still reach was a corner the clipper invented, which is
/// not that node and must not answer to its id: on the Munich test box it put
/// one way's first node id on two corners 23 blocks away, leaving that id on
/// three vertices at two different blocks. What the old rule bought is kept
/// explicitly instead: the caller re-closes the ring by repeating its first
/// vertex, so the repeat carries the first vertex's id and the `first == last`
/// closure signal survives.
fn assign_node_ids_preserving_endpoints(
    original_nodes: &[ProcessedNode],
    clipped_coords: Vec<(f64, f64)>,
    way_id: u64,
) -> Vec<ProcessedNode> {
    if clipped_coords.is_empty() {
        return Vec::new();
    }

    // First node wins where two original nodes share a block, so the answer does
    // not depend on iteration order.
    let mut by_block: HashMap<(i32, i32), &ProcessedNode> =
        HashMap::with_capacity(original_nodes.len());
    for n in original_nodes {
        by_block.entry((n.x, n.z)).or_insert(n);
    }

    // A kept vertex keeps its tags too, so a gate on a clipped fence still opens.
    let mut out: Vec<ProcessedNode> = clipped_coords
        .into_iter()
        .enumerate()
        .map(|(i, coord)| {
            let (x, z) = (coord.0.round() as i32, coord.1.round() as i32);
            let (id, tags) = match by_block.get(&(x, z)) {
                Some(n) => (n.id, n.tags.clone()),
                None => (invented_node_id(way_id, i as u64), HashMap::new()),
            };
            ProcessedNode { id, x, z, tags }
        })
        .collect();

    if out.len() >= 2 {
        let first = &out[0];
        let (fx, fz, fid) = (first.x, first.z, first.id);
        let last = out.last_mut().expect("out has at least two nodes");
        if last.x == fx && last.z == fz {
            last.id = fid;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;

    fn node(id: u64, x: i32, z: i32) -> ProcessedNode {
        ProcessedNode {
            id,
            tags: HashMap::new(),
            x,
            z,
        }
    }

    /// A clipped ring is still made of the same OSM nodes where it survived,
    /// and only the corners the clipper cut are new. Renaming the whole ring,
    /// as this used to, cost every building the world's edge touched all of
    /// its facades, the walls nowhere near the edge included.
    #[test]
    fn a_clipped_ring_keeps_the_ids_of_the_nodes_that_survived() {
        let bbox = XZBBox::rect_from_min_max(0, 0, 30, 30).unwrap();
        let ring = vec![
            node(1, 10, 10),
            node(2, 40, 10),
            node(3, 40, 25),
            node(4, 10, 25),
            node(1, 10, 10),
        ];
        let clipped = clip_way_to_bbox(&ring, &bbox);

        let at = |x: i32, z: i32| clipped.iter().find(|n| n.x == x && n.z == z).map(|n| n.id);
        assert_eq!(at(10, 10), Some(1), "a node inside the world is itself");
        assert_eq!(at(10, 25), Some(4));
        // The two corners the clip created stand where nodes 2 and 3 were cut
        // off, on the world's edge, and are nobody: an id no OSM node has.
        for (x, z) in [(30, 10), (30, 25)] {
            let id = at(x, z).expect("the clip put a corner here");
            assert!(
                ![1, 2, 3, 4].contains(&id),
                "the corner at ({x}, {z}) took the id {id} of a node it is not"
            );
        }
        assert!(!clipped.iter().any(|n| n.id == 2 || n.id == 3));
        // Still closed by id, which is the signal the ring assembly reads.
        assert_eq!(clipped.first().map(|n| n.id), clipped.last().map(|n| n.id));
    }

    /// A gate on a fence the world's edge cuts must still be a gate.
    #[test]
    fn a_clipped_ring_keeps_the_tags_of_the_nodes_that_survived() {
        let bbox = XZBBox::rect_from_min_max(0, 0, 30, 30).unwrap();
        let mut gate = node(4, 10, 25);
        gate.tags.insert("barrier".to_string(), "gate".to_string());
        let ring = vec![
            node(1, 10, 10),
            node(2, 40, 10),
            node(3, 40, 25),
            gate,
            node(1, 10, 10),
        ];
        let clipped = clip_way_to_bbox(&ring, &bbox);
        let kept = clipped.iter().find(|n| n.id == 4).expect("node 4 survives");
        assert_eq!(kept.tags.get("barrier").map(String::as_str), Some("gate"));
        assert!(clipped
            .iter()
            .filter(|n| n.id != 4)
            .all(|n| n.tags.is_empty()));
    }

    #[test]
    fn a_cut_end_does_not_answer_to_the_node_beyond_it() {
        let bbox = XZBBox::rect_from_min_max(0, 0, 30, 30).unwrap();
        let line = vec![node(1, 10, 10), node(2, 40, 10)];
        let mut clipped = clip_way_to_bbox(&line, &bbox);
        assert_eq!(clipped[1].id, 2, "the clip lends the cut end node 2's id");
        mark_cut_ends(&mut clipped, [(10, 10), (40, 10)], 7);
        assert_eq!(clipped[0].id, 1);
        assert!(is_invented_node_id(clipped[1].id));
    }

    #[test]
    fn open_fragment_crossing_bbox_is_rejected() {
        let bbox = XZBBox::rect_from_min_max(0, 0, 15, 15).unwrap();
        let fragment = vec![node(1, -5, 3), node(2, 8, 3), node(3, 30, 12)];
        assert!(clip_water_ring_to_bbox(&fragment, &bbox).is_none());
    }

    #[test]
    fn nearly_closed_ring_crossing_bbox_is_clipped() {
        let bbox = XZBBox::rect_from_min_max(0, 0, 15, 15).unwrap();
        let ring = vec![
            node(1, 4, 4),
            node(2, 30, 4),
            node(3, 30, 11),
            node(4, 4, 11),
            node(5, 4, 5),
        ];
        let clipped = clip_water_ring_to_bbox(&ring, &bbox).unwrap();
        assert!(clipped
            .iter()
            .all(|n| (0..=15).contains(&n.x) && (0..=15).contains(&n.z)));
    }

    #[test]
    fn closed_ring_crossing_bbox_is_clipped() {
        let bbox = XZBBox::rect_from_min_max(0, 0, 15, 15).unwrap();
        let ring = vec![
            node(1, 4, 4),
            node(2, 30, 4),
            node(3, 30, 11),
            node(4, 4, 11),
            node(1, 4, 4),
        ];
        assert!(clip_water_ring_to_bbox(&ring, &bbox).is_some());
    }

    /// An id the clipper invents must never be one OSM could hand out. The old
    /// scheme, `way_id * 10_000_000 + i`, put way 100's first corner at
    /// 1_000_000_000, which is an ordinary node id.
    #[test]
    fn an_invented_node_id_cannot_collide_with_a_real_one() {
        // Comfortably above OSM's highest node id today, about 1.3e10.
        const HIGHEST_PLAUSIBLE_OSM_ID: u64 = 1_000_000_000_000;
        for way_id in [1u64, 100, 12_345, 987_654_321, 1_234_567_890_123] {
            for i in 0..64u64 {
                let id = invented_node_id(way_id, i);
                assert!(
                    id > HIGHEST_PLAUSIBLE_OSM_ID,
                    "way {way_id} corner {i} invented {id}, inside the OSM range"
                );
                assert!(is_invented_node_id(id));
                assert!(id < u64::MAX / 2, "must stay positive as an i64");
            }
        }
        assert!(!is_invented_node_id(1_000_000_000));
        // Deterministic, and distinct per corner of a way.
        assert_eq!(invented_node_id(42, 7), invented_node_id(42, 7));
        assert_ne!(invented_node_id(42, 7), invented_node_id(42, 8));
    }

    #[test]
    fn a_line_is_cut_where_it_leaves_a_drawn_outline() {
        // Two squares with a gap between them, as two areas merged into one world.
        let world = XZBBox::rect_from_min_max(0, 0, 100, 20)
            .unwrap()
            .with_outline(vec![
                vec![vec![(0.0, 0.0), (30.0, 0.0), (30.0, 20.0), (0.0, 20.0)]],
                vec![vec![(70.0, 0.0), (100.0, 0.0), (100.0, 20.0), (70.0, 20.0)]],
            ])
            .unwrap();
        let line = [(1, 5.5, 10.5), (2, 95.5, 10.5)];
        let mut cuts = 0;
        let runs = cut_line_to_outline(7, &line, &world, &mut cuts).unwrap();
        assert_eq!(runs.len(), 2, "{runs:?}");
        assert_eq!(cuts, 2);
        // Each run keeps its own node and ends inside, on a cut of its own.
        assert_eq!(runs[0][0].0, 1);
        assert_eq!(runs[1][1].0, 2);
        let (a, b) = (runs[0][1], runs[1][0]);
        assert!(is_invented_node_id(a.0) && is_invented_node_id(b.0) && a.0 != b.0);
        assert!(world.contains(&XZPoint::new(a.1.floor() as i32, 10)));
        assert!(world.contains(&XZPoint::new(b.1.floor() as i32, 10)));
        assert!(
            (a.1 - 31.0).abs() < 0.01 && (b.1 - 70.0).abs() < 0.01,
            "{a:?} {b:?}"
        );

        // A second piece of the same way gets new ids, and a line of our own keeps apart.
        let again = cut_line_to_outline(7, &line, &world, &mut cuts).unwrap();
        assert!(again.iter().flatten().all(|p| p.0 != a.0 && p.0 != b.0));
        let ours = cut_line_to_outline((1 << 62) | 7, &line, &world, &mut 0).unwrap();
        let osm = cut_line_to_outline(7, &line, &world, &mut 0).unwrap();
        let cut_ids = |runs: &Vec<Vec<(u64, f64, f64)>>| -> Vec<u64> {
            runs.iter()
                .flatten()
                .map(|p| p.0)
                .filter(|&id| is_invented_node_id(id))
                .collect()
        };
        let (ours, osm) = (cut_ids(&ours), cut_ids(&osm));
        assert_eq!(ours.len(), 2);
        assert!(ours.iter().all(|id| !osm.contains(id)), "{ours:?} {osm:?}");

        // Inside, outside, and off an outline.
        let inside = [(1, 2.5, 2.5), (2, 25.5, 15.5)];
        assert!(cut_line_to_outline(7, &inside, &world, &mut 0).is_none());
        let outside = [(1, 40.5, 2.5), (2, 60.5, 15.5)];
        assert_eq!(
            cut_line_to_outline(7, &outside, &world, &mut 0),
            Some(vec![])
        );
        let rect = XZBBox::rect_from_min_max(0, 0, 100, 20).unwrap();
        assert!(cut_line_to_outline(7, &line, &rect, &mut 0).is_none());
    }

    #[test]
    fn a_line_along_the_outline_is_not_cut_to_pieces() {
        let world = XZBBox::rect_from_min_max(0, 0, 200, 200)
            .unwrap()
            .with_outline(vec![vec![vec![
                (0.0, 0.0),
                (100.0, 0.0),
                (100.0, 200.0),
                (0.0, 200.0),
            ]]])
            .unwrap();
        // A road traced as the outline's east edge, wavering a block or two across it.
        let road: Vec<(u64, f64, f64)> = (1..40)
            .map(|k| {
                let x = 100.5 + if k % 2 == 0 { 1.5 } else { -1.5 };
                (k as u64 + 1, x, 5.5 + k as f64 * 4.5)
            })
            .collect();
        assert!(cut_line_to_outline(3, &road, &world, &mut 0).is_none());

        // A line that only grazes a corner leaves nothing to drive on.
        let graze = [(1, 95.5, -8.5), (2, 108.5, 4.5)];
        assert_eq!(cut_line_to_outline(3, &graze, &world, &mut 0), Some(vec![]));

        // A stray node just outside before the line leaves is dropped for a cut on the boundary.
        let out = [
            (1, 50.5, 50.5),
            (2, 99.5, 50.5),
            (3, 101.5, 50.5),
            (4, 160.5, 50.5),
        ];
        let runs = cut_line_to_outline(3, &out, &world, &mut 0).unwrap();
        assert_eq!(runs.len(), 1);
        let ids: Vec<u64> = runs[0].iter().map(|p| p.0).collect();
        assert_eq!(&ids[..2], &[1, 2]);
        let end = runs[0][2];
        assert!(
            is_invented_node_id(end.0) && end.1 > 99.5 && end.1 < 101.0,
            "{end:?}"
        );
    }

    #[test]
    fn a_narrow_gap_cuts_and_a_line_starting_just_outside_starts_on_a_cut() {
        // Two areas four columns apart, closer than a waver may stray.
        let world = XZBBox::rect_from_min_max(0, 0, 60, 30)
            .unwrap()
            .with_outline(vec![
                vec![vec![(0.0, 0.0), (30.0, 0.0), (30.0, 20.0), (0.0, 20.0)]],
                vec![vec![(35.0, 0.0), (60.0, 0.0), (60.0, 20.0), (35.0, 20.0)]],
            ])
            .unwrap();
        let line = [(1, 5.5, 10.5), (2, 55.5, 10.5)];
        let runs = cut_line_to_outline(7, &line, &world, &mut 0).unwrap();
        assert_eq!(runs.len(), 2, "{runs:?}");
        assert!(runs
            .iter()
            .flatten()
            .all(|p| world.contains(&XZPoint::new(p.1.floor() as i32, p.2.floor() as i32))));

        let start = [(1, 45.5, 21.5), (2, 45.5, 5.5)];
        let runs = cut_line_to_outline(7, &start, &world, &mut 0).unwrap();
        assert_eq!(runs.len(), 1);
        let (a, b) = (runs[0][0], runs[0][1]);
        assert!(
            is_invented_node_id(a.0) && (a.2 - 21.0).abs() < 0.01,
            "{a:?}"
        );
        assert_eq!(b.0, 2);
    }

    #[test]
    fn ways_keep_their_order_and_pass_through_a_rectangle_untouched() {
        use crate::osm_parser::ProcessedWay;
        let node = |id, x, z| ProcessedNode {
            id,
            tags: HashMap::new(),
            x,
            z,
        };
        let way = |id, nodes| ProcessedWay {
            id,
            nodes,
            tags: HashMap::new(),
        };
        let across = way(1, vec![node(1, 5, 10), node(2, 95, 10)]);
        let within = way(2, vec![node(3, 75, 5), node(4, 95, 15)]);
        let rect = XZBBox::rect_from_min_max(0, 0, 100, 20).unwrap();
        let same = cut_ways_to_outline(vec![(&across, 'a'), (&within, 'b')], &rect);
        assert!(same
            .iter()
            .all(|(w, _)| matches!(w, std::borrow::Cow::Borrowed(_))));

        let world = rect
            .with_outline(vec![
                vec![vec![(0.0, 0.0), (30.0, 0.0), (30.0, 20.0), (0.0, 20.0)]],
                vec![vec![(70.0, 0.0), (100.0, 0.0), (100.0, 20.0), (70.0, 20.0)]],
            ])
            .unwrap();
        let cut = cut_ways_to_outline(vec![(&across, 'a'), (&within, 'b')], &world);
        let tags: Vec<char> = cut.iter().map(|(_, t)| *t).collect();
        assert_eq!(tags, vec!['a', 'a', 'b']);
        assert!(matches!(cut[2].0, std::borrow::Cow::Borrowed(_)));
        assert!(cut.iter().all(|(w, _)| w
            .nodes
            .iter()
            .all(|n| world.contains(&XZPoint::new(n.x, n.z)))));
    }
}
