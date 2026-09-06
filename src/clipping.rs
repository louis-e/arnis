// Sutherland-Hodgman polygon clipping and related geometry utilities.
//
// Provides bbox clipping for polygons, polylines, and water rings.

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::osm_parser::ProcessedNode;
use std::collections::HashMap;

/// Clips a way to the bounding box using Sutherland-Hodgman for polygons or
/// simple line clipping for polylines. Preserves endpoint IDs for ring assembly.
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
                    let synthetic_id = nodes[0]
                        .id
                        .wrapping_mul(10000000)
                        .wrapping_add(result.len() as u64);
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
                        let synthetic_id = nodes[0]
                            .id
                            .wrapping_mul(10000000)
                            .wrapping_add(result.len() as u64);
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

    // First ID wins where two original nodes share a block, so the answer does
    // not depend on iteration order.
    let mut by_block: HashMap<(i32, i32), u64> = HashMap::with_capacity(original_nodes.len());
    for n in original_nodes {
        by_block.entry((n.x, n.z)).or_insert(n.id);
    }

    let mut out: Vec<ProcessedNode> = clipped_coords
        .into_iter()
        .enumerate()
        .map(|(i, coord)| {
            let (x, z) = (coord.0.round() as i32, coord.1.round() as i32);
            let id = match by_block.get(&(x, z)) {
                Some(&id) => id,
                None => way_id.wrapping_mul(10000000).wrapping_add(i as u64),
            };
            ProcessedNode {
                id,
                x,
                z,
                tags: HashMap::new(),
            }
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
}
