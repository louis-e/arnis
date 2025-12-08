// Sutherland-Hodgman polygon clipping and related geometry utilities.
//
// Provides bbox clipping for polygons, polylines, and water rings with
// proper corner insertion for closed shapes.

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::osm_parser::ProcessedNode;
use std::collections::HashMap;

/// Clips a way to the bounding box using Sutherland-Hodgman for polygons or
/// simple line clipping for polylines. Preserves endpoint IDs for ring assembly.
pub fn clip_way_to_bbox(nodes: &[ProcessedNode], xzbbox: &XZBBox) -> Vec<ProcessedNode> {
    if nodes.is_empty() {
        return Vec::new();
    }

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

    let polygon = insert_bbox_corners(polygon, min_x, min_z, max_x, max_z);
    let polygon = remove_consecutive_duplicates(polygon);
    if polygon.len() < 3 {
        return Vec::new();
    }

    let way_id = nodes.first().map(|n| n.id).unwrap_or(0);
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

    let polygon = insert_bbox_corners(polygon, min_x, min_z, max_x, max_z);
    if polygon.len() < 3 {
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

        for i in 0..(polygon.len().saturating_sub(1)) {
            let current = polygon[i];
            let next = polygon[i + 1];

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

/// Returns which bbox edge a point lies on: 0=bottom, 1=right, 2=top, 3=left, -1=interior.
fn get_bbox_edge(point: (f64, f64), min_x: f64, min_z: f64, max_x: f64, max_z: f64) -> i32 {
    let eps = 0.5;

    let on_left = (point.0 - min_x).abs() < eps;
    let on_right = (point.0 - max_x).abs() < eps;
    let on_bottom = (point.1 - min_z).abs() < eps;
    let on_top = (point.1 - max_z).abs() < eps;

    // Handle corners (assign to edge in counter-clockwise order)
    if on_bottom && on_left {
        return 3;
    }
    if on_bottom && on_right {
        return 0;
    }
    if on_top && on_right {
        return 1;
    }
    if on_top && on_left {
        return 2;
    }

    if on_bottom {
        return 0;
    }
    if on_right {
        return 1;
    }
    if on_top {
        return 2;
    }
    if on_left {
        return 3;
    }

    -1
}

/// Returns corners to insert when traversing from edge1 to edge2 via shorter path.
fn get_corners_between_edges(
    edge1: i32,
    edge2: i32,
    min_x: f64,
    min_z: f64,
    max_x: f64,
    max_z: f64,
) -> Vec<(f64, f64)> {
    if edge1 == edge2 || edge1 < 0 || edge2 < 0 {
        return Vec::new();
    }

    let corners = [
        (max_x, min_z), // 0: bottom-right
        (max_x, max_z), // 1: top-right
        (min_x, max_z), // 2: top-left
        (min_x, min_z), // 3: bottom-left
    ];

    let ccw_dist = ((edge2 - edge1 + 4) % 4) as usize;
    let cw_dist = ((edge1 - edge2 + 4) % 4) as usize;

    // Opposite edges: don't insert corners
    if ccw_dist == 2 && cw_dist == 2 {
        return Vec::new();
    }

    let mut result = Vec::new();

    if ccw_dist <= cw_dist {
        let mut current = edge1;
        for _ in 0..ccw_dist {
            result.push(corners[current as usize]);
            current = (current + 1) % 4;
        }
    } else {
        let mut current = edge1;
        for _ in 0..cw_dist {
            current = (current + 4 - 1) % 4;
            result.push(corners[current as usize]);
        }
    }

    result
}

/// Inserts bbox corners where polygon transitions between different bbox edges.
fn insert_bbox_corners(
    polygon: Vec<(f64, f64)>,
    min_x: f64,
    min_z: f64,
    max_x: f64,
    max_z: f64,
) -> Vec<(f64, f64)> {
    if polygon.len() < 3 {
        return polygon;
    }

    let mut result = Vec::with_capacity(polygon.len() + 4);

    for i in 0..polygon.len() {
        let current = polygon[i];
        let next = polygon[(i + 1) % polygon.len()];

        result.push(current);

        let edge1 = get_bbox_edge(current, min_x, min_z, max_x, max_z);
        let edge2 = get_bbox_edge(next, min_x, min_z, max_x, max_z);

        if edge1 >= 0 && edge2 >= 0 && edge1 != edge2 {
            for corner in get_corners_between_edges(edge1, edge2, min_x, min_z, max_x, max_z) {
                result.push(corner);
            }
        }
    }

    result
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

/// Assigns node IDs to clipped coordinates, preserving original endpoint IDs.
fn assign_node_ids_preserving_endpoints(
    original_nodes: &[ProcessedNode],
    clipped_coords: Vec<(f64, f64)>,
    way_id: u64,
) -> Vec<ProcessedNode> {
    if clipped_coords.is_empty() {
        return Vec::new();
    }

    let original_first = original_nodes.first();
    let original_last = original_nodes.last();
    let tolerance = 50.0;
    let last_index = clipped_coords.len() - 1;

    clipped_coords
        .into_iter()
        .enumerate()
        .map(|(i, coord)| {
            let is_first = i == 0;
            let is_last = i == last_index;

            if is_first || is_last {
                if let Some(first) = original_first {
                    if matches_endpoint(coord, first, tolerance) {
                        return ProcessedNode {
                            id: first.id,
                            x: coord.0.round() as i32,
                            z: coord.1.round() as i32,
                            tags: HashMap::new(),
                        };
                    }
                }
                if let Some(last) = original_last {
                    if matches_endpoint(coord, last, tolerance) {
                        return ProcessedNode {
                            id: last.id,
                            x: coord.0.round() as i32,
                            z: coord.1.round() as i32,
                            tags: HashMap::new(),
                        };
                    }
                }
            }

            ProcessedNode {
                id: way_id.wrapping_mul(10000000).wrapping_add(i as u64),
                x: coord.0.round() as i32,
                z: coord.1.round() as i32,
                tags: HashMap::new(),
            }
        })
        .collect()
}
