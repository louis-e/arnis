use geo::orient::{Direction, Orient};
use geo::{Contains, Intersects, LineString, Point, Polygon, Rect};
use std::time::Instant;

use crate::{
    block_definitions::WATER,
    coordinate_system::cartesian::{XZBBox, XZPoint},
    osm_parser::{ProcessedMemberRole, ProcessedNode, ProcessedRelation, ProcessedWay},
    world_editor::WorldEditor,
};

pub fn generate_water_area_from_way(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    _xzbbox: &XZBBox,
) {
    let start_time = Instant::now();

    let outers = [element.nodes.clone()];
    if !verify_loopy_loops(&outers) {
        println!("Skipping way {} due to invalid polygon", element.id);
        return;
    }

    generate_water_areas(editor, &outers, &[], start_time);
}

pub fn generate_water_areas_from_relation(
    editor: &mut WorldEditor,
    element: &ProcessedRelation,
    xzbbox: &XZBBox,
) {
    let start_time = Instant::now();

    // Check if this is a water relation (either with water tag or natural=water)
    let is_water = element.tags.contains_key("water")
        || element
            .tags
            .get("natural")
            .map(|val| val == "water" || val == "bay")
            .unwrap_or(false);

    if !is_water {
        return;
    }

    // Don't handle water below layer 0
    if let Some(layer) = element.tags.get("layer") {
        if layer.parse::<i32>().map(|x| x < 0).unwrap_or(false) {
            return;
        }
    }

    let mut outers: Vec<Vec<ProcessedNode>> = vec![];
    let mut inners: Vec<Vec<ProcessedNode>> = vec![];

    for mem in &element.members {
        match mem.role {
            ProcessedMemberRole::Outer => outers.push(mem.way.nodes.clone()),
            ProcessedMemberRole::Inner => inners.push(mem.way.nodes.clone()),
        }
    }

    // DON'T auto-swap outer/inner - this causes more problems than it solves
    // OSM data should already have correct roles; if it's wrong in OSM, fix it there
    // The previous heuristic was causing water to fill on land

    merge_loopy_loops(&mut outers);

    // NOW clip the assembled complete rings to bbox
    // This is crucial: we merged complete rings first, THEN clip them
    outers = outers
        .into_iter()
        .filter_map(|ring| clip_polygon_ring_to_bbox(&ring, xzbbox))
        .collect();
    merge_loopy_loops(&mut inners);
    inners = inners
        .into_iter()
        .filter_map(|ring| clip_polygon_ring_to_bbox(&ring, xzbbox))
        .collect();

    if !verify_loopy_loops(&outers) {
        // For clipped multipolygons, some loops may not close perfectly
        // Instead of force-closing with straight lines (which creates wedges),
        // filter out unclosed loops and only render the properly closed ones

        // Filter: Keep only loops that are already closed OR can be closed within 1 block
        outers.retain(|loop_nodes| {
            if loop_nodes.len() < 3 {
                return false;
            }
            let first = &loop_nodes[0];
            let last = loop_nodes.last().unwrap();
            let dx = (first.x - last.x).abs();
            let dz = (first.z - last.z).abs();

            // Keep if already closed by ID or endpoints are within 1 block
            first.id == last.id || (dx <= 1 && dz <= 1)
        });

        // Now close the remaining loops that are within 1 block tolerance
        for loop_nodes in outers.iter_mut() {
            let first = loop_nodes[0].clone();
            let last_idx = loop_nodes.len() - 1;
            if loop_nodes[0].id != loop_nodes[last_idx].id {
                // Endpoints are close (within tolerance), close the loop
                loop_nodes.push(first);
            }
        }

        // If no valid outer loops remain, skip the relation
        if outers.is_empty() {
            return;
        }

        // Verify again after filtering and closing
        if !verify_loopy_loops(&outers) {
            println!("Skipping relation {} due to invalid polygon", element.id);
            return;
        }
    }

    merge_loopy_loops(&mut inners);
    if !verify_loopy_loops(&inners) {
        println!("Skipping relation {} due to invalid polygon", element.id);
        return;
    }

    generate_water_areas(editor, &outers, &inners, start_time);
}

fn generate_water_areas(
    editor: &mut WorldEditor,
    outers: &[Vec<ProcessedNode>],
    inners: &[Vec<ProcessedNode>],
    start_time: Instant,
) {
    // Calculate the actual bounding box of the polygon nodes
    // This is CRITICAL for performance - we only need to scan the area covered by the polygons,
    // not the entire world!
    let mut poly_min_x = i32::MAX;
    let mut poly_min_z = i32::MAX;
    let mut poly_max_x = i32::MIN;
    let mut poly_max_z = i32::MIN;

    for outer in outers {
        for node in outer {
            poly_min_x = poly_min_x.min(node.x);
            poly_min_z = poly_min_z.min(node.z);
            poly_max_x = poly_max_x.max(node.x);
            poly_max_z = poly_max_z.max(node.z);
        }
    }

    // If no valid bounds, nothing to fill
    if poly_min_x == i32::MAX || poly_max_x == i32::MIN {
        return;
    }

    // Clamp to world bounds just in case
    let (world_min_x, world_min_z) = editor.get_min_coords();
    let (world_max_x, world_max_z) = editor.get_max_coords();
    let min_x = poly_min_x.max(world_min_x);
    let min_z = poly_min_z.max(world_min_z);
    let max_x = poly_max_x.min(world_max_x);
    let max_z = poly_max_z.min(world_max_z);

    let outers_xz: Vec<Vec<XZPoint>> = outers
        .iter()
        .map(|x| x.iter().map(|y| y.xz()).collect::<Vec<_>>())
        .collect();
    let inners_xz: Vec<Vec<XZPoint>> = inners
        .iter()
        .map(|x| x.iter().map(|y| y.xz()).collect::<Vec<_>>())
        .collect();

    inverse_floodfill(
        min_x, min_z, max_x, max_z, outers_xz, inners_xz, editor, start_time,
    );
}

// Merges ways that share nodes into full loops
fn merge_loopy_loops(loops: &mut Vec<Vec<ProcessedNode>>) {
    let mut removed: Vec<usize> = vec![];
    let mut merged: Vec<Vec<ProcessedNode>> = vec![];

    // Helper function to check if two nodes match (by ID or proximity)
    let nodes_match = |a: &ProcessedNode, b: &ProcessedNode| -> bool {
        if a.id == b.id {
            return true;
        }
        // Also match if coordinates are very close (within 1 block)
        // This handles synthetic nodes created at bbox edges
        let dx = (a.x - b.x).abs();
        let dz = (a.z - b.z).abs();
        dx <= 1 && dz <= 1
    };

    for i in 0..loops.len() {
        for j in 0..loops.len() {
            if i == j {
                continue;
            }

            if removed.contains(&i) || removed.contains(&j) {
                continue;
            }

            let x: &Vec<ProcessedNode> = &loops[i];
            let y: &Vec<ProcessedNode> = &loops[j];

            // Skip empty loops (can happen after clipping)
            if x.is_empty() || y.is_empty() {
                continue;
            }

            let x_first = &x[0];
            let x_last = x.last().unwrap();
            let y_first = &y[0];
            let y_last = y.last().unwrap();

            // it's looped already
            if nodes_match(x_first, x_last) {
                continue;
            }

            // it's looped already
            if nodes_match(y_first, y_last) {
                continue;
            }

            if nodes_match(x_first, y_first) {
                removed.push(i);
                removed.push(j);

                let mut x: Vec<ProcessedNode> = x.clone();
                x.reverse();
                x.extend(y.iter().skip(1).cloned());
                merged.push(x);
            } else if nodes_match(x_last, y_last) {
                removed.push(i);
                removed.push(j);

                let mut x: Vec<ProcessedNode> = x.clone();
                x.extend(y.iter().rev().skip(1).cloned());

                merged.push(x);
            } else if nodes_match(x_first, y_last) {
                removed.push(i);
                removed.push(j);

                let mut y: Vec<ProcessedNode> = y.clone();
                y.extend(x.iter().skip(1).cloned());

                merged.push(y);
            } else if nodes_match(x_last, y_first) {
                removed.push(i);
                removed.push(j);

                let mut x: Vec<ProcessedNode> = x.clone();
                x.extend(y.iter().skip(1).cloned());

                merged.push(x);
            }
        }
    }

    removed.sort();

    for r in removed.iter().rev() {
        loops.remove(*r);
    }

    let merged_len: usize = merged.len();
    for m in merged {
        loops.push(m);
    }

    if merged_len > 0 {
        merge_loopy_loops(loops);
    }
}

fn verify_loopy_loops(loops: &[Vec<ProcessedNode>]) -> bool {
    let mut valid: bool = true;
    for l in loops {
        let first = &l[0];
        let last = l.last().unwrap();

        // Check if loop is closed (by ID or proximity)
        let is_closed = first.id == last.id || {
            let dx = (first.x - last.x).abs();
            let dz = (first.z - last.z).abs();
            dx <= 1 && dz <= 1
        };

        if !is_closed {
            eprintln!("WARN: Disconnected loop");
            valid = false;
        }
    }

    valid
}

/// Force-close loops that have endpoints very close to each other
/// This handles cases where clipping creates nearly-closed loops
fn close_open_loops(loops: &mut Vec<Vec<ProcessedNode>>) {
    for loop_nodes in loops.iter_mut() {
        if loop_nodes.len() < 2 {
            continue;
        }

        let first = &loop_nodes[0];
        let last = &loop_nodes[loop_nodes.len() - 1];

        // Check if already closed
        if first.id == last.id {
            continue;
        }

        // Check if endpoints are very close - just duplicate first node to close
        let dx = (first.x - last.x).abs();
        let dz = (first.z - last.z).abs();

        if dx <= 1 && dz <= 1 {
            // Already essentially closed, just duplicate first node
            loop_nodes.push(first.clone());
        } else {
            // Endpoints are far apart - this is likely a clipped multipolygon
            // that enters/exits the bbox. Close it by connecting endpoints directly.
            // This creates a "closed polygon within bbox" representation.
            loop_nodes.push(first.clone());
        }
    }
}

/// Clip a complete polygon ring to the bbox using Sutherland-Hodgman algorithm
/// Returns None if the polygon is completely outside the bbox
fn clip_polygon_ring_to_bbox(
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

    // Check if entire ring is inside bbox - if so, return unchanged
    let all_inside = ring.iter().all(|n| {
        n.x as f64 >= min_x && n.x as f64 <= max_x && n.z as f64 >= min_z && n.z as f64 <= max_z
    });

    if all_inside {
        // Ring is entirely inside bbox, no clipping needed
        return Some(ring.to_vec());
    }

    // Check if entire ring is outside bbox
    let all_outside_left = ring.iter().all(|n| (n.x as f64) < min_x);
    let all_outside_right = ring.iter().all(|n| (n.x as f64) > max_x);
    let all_outside_top = ring.iter().all(|n| (n.z as f64) < min_z);
    let all_outside_bottom = ring.iter().all(|n| (n.z as f64) > max_z);

    if all_outside_left || all_outside_right || all_outside_top || all_outside_bottom {
        // Ring is entirely outside bbox
        return None;
    }

    // Ring crosses bbox boundary, need to clip
    // Convert to f64 coordinates for clipping
    let mut polygon: Vec<(f64, f64)> = ring.iter().map(|n| (n.x as f64, n.z as f64)).collect();

    // Ensure polygon is closed
    if !polygon.is_empty() && polygon.first() != polygon.last() {
        polygon.push(polygon[0]);
    }

    // Clip against each edge of the bounding box
    // Edges are traversed COUNTER-CLOCKWISE, so "inside" (left of edge) = inside bbox
    let bbox_edges = [
        (min_x, min_z, max_x, min_z), // Bottom edge: left to right
        (max_x, min_z, max_x, max_z), // Right edge: bottom to top
        (max_x, max_z, min_x, max_z), // Top edge: right to left
        (min_x, max_z, min_x, min_z), // Left edge: top to bottom
    ];

    for (edge_x1, edge_z1, edge_x2, edge_z2) in bbox_edges {
        let mut clipped = Vec::new();

        if polygon.is_empty() {
            return None;
        }

        // Process edges: iterate through adjacent pairs
        // For a closed polygon, we process n-1 edges (since last point == first point)
        for i in 0..(polygon.len() - 1) {
            let current = polygon[i];
            let next = polygon[i + 1];

            let current_inside = point_inside_edge(current, edge_x1, edge_z1, edge_x2, edge_z2);
            let next_inside = point_inside_edge(next, edge_x1, edge_z1, edge_x2, edge_z2);

            if next_inside {
                if !current_inside {
                    // Entering: add intersection
                    if let Some(mut intersection) = line_edge_intersection(
                        current.0, current.1, next.0, next.1, edge_x1, edge_z1, edge_x2, edge_z2,
                    ) {
                        // Clamp intersection to bbox to handle floating-point errors
                        intersection.0 = intersection.0.clamp(min_x, max_x);
                        intersection.1 = intersection.1.clamp(min_z, max_z);
                        clipped.push(intersection);
                    }
                }
                clipped.push(next);
            } else if current_inside {
                // Exiting: add intersection
                if let Some(mut intersection) = line_edge_intersection(
                    current.0, current.1, next.0, next.1, edge_x1, edge_z1, edge_x2, edge_z2,
                ) {
                    // Clamp intersection to bbox to handle floating-point errors
                    intersection.0 = intersection.0.clamp(min_x, max_x);
                    intersection.1 = intersection.1.clamp(min_z, max_z);
                    clipped.push(intersection);
                }
            }
        }

        polygon = clipped;
    }

    if polygon.len() < 3 {
        return None; // Not a valid polygon
    }

    // Verify all points are within bbox before returning
    let all_points_inside = polygon
        .iter()
        .all(|&(x, z)| x >= min_x && x <= max_x && z >= min_z && z <= max_z);

    if !all_points_inside {
        eprintln!("ERROR: clip_polygon_ring_to_bbox produced points outside bbox!");
        eprintln!("  Bbox: x=[{}, {}], z=[{}, {}]", min_x, max_x, min_z, max_z);
        for (i, &(x, z)) in polygon.iter().enumerate() {
            if x < min_x || x > max_x || z < min_z || z > max_z {
                eprintln!("  Point {}: ({}, {}) is OUTSIDE", i, x, z);
            }
        }
        return None; // Reject invalid result
    }

    // Convert back to ProcessedNode with synthetic IDs
    // IMPORTANT: Clamp coordinates to bbox boundaries to handle floating-point edge cases
    let mut result: Vec<ProcessedNode> = polygon
        .iter()
        .enumerate()
        .map(|(i, &(x, z))| {
            let clamped_x = x.clamp(min_x, max_x);
            let clamped_z = z.clamp(min_z, max_z);
            ProcessedNode {
                id: 1_000_000_000 + i as u64, // Synthetic ID for clipped nodes
                tags: std::collections::HashMap::new(),
                x: clamped_x.round() as i32,
                z: clamped_z.round() as i32,
            }
        })
        .collect();

    // Ensure first and last have same ID to close the loop
    if !result.is_empty() {
        let first_id = result[0].id;
        result.last_mut().unwrap().id = first_id;
    }

    Some(result)
}

fn point_inside_edge(
    point: (f64, f64),
    edge_x1: f64,
    edge_z1: f64,
    edge_x2: f64,
    edge_z2: f64,
) -> bool {
    // Cross product to determine if point is on the "inside" (left) of the edge
    let dx = edge_x2 - edge_x1;
    let dz = edge_z2 - edge_z1;
    let px = point.0 - edge_x1;
    let pz = point.1 - edge_z1;
    (dx * pz - dz * px) >= 0.0
}

fn line_edge_intersection(
    x1: f64,
    z1: f64,
    x2: f64,
    z2: f64,
    edge_x1: f64,
    edge_z1: f64,
    edge_x2: f64,
    edge_z2: f64,
) -> Option<(f64, f64)> {
    let dx = x2 - x1;
    let dz = z2 - z1;
    let edge_dx = edge_x2 - edge_x1;
    let edge_dz = edge_z2 - edge_z1;

    let denominator = dx * edge_dz - dz * edge_dx;
    if denominator.abs() < 1e-10 {
        return None; // Parallel lines
    }

    let t = ((edge_x1 - x1) * edge_dz - (edge_z1 - z1) * edge_dx) / denominator;
    if !(0.0..=1.0).contains(&t) {
        return None;
    }

    Some((x1 + t * dx, z1 + t * dz))
}

// Water areas are absolutely huge. We can't easily flood fill the entire thing.
// Instead, we'll iterate over all the blocks in our MC world, and check if each
// one is in the river or not
#[allow(clippy::too_many_arguments)]
fn inverse_floodfill(
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    outers: Vec<Vec<XZPoint>>,
    inners: Vec<Vec<XZPoint>>,
    editor: &mut WorldEditor,
    start_time: Instant,
) {
    // Convert to geo Polygons and ORIENT them correctly
    // The geo crate expects exterior rings to be CCW and interior rings to be CW
    // Our coordinate transformation inverts the Z axis, which reverses winding order
    // Using orient(Direction::Default) normalizes this to the expected convention
    let inners: Vec<_> = inners
        .into_iter()
        .map(|x| {
            Polygon::new(
                LineString::from(
                    x.iter()
                        .map(|pt| (pt.x as f64, pt.z as f64))
                        .collect::<Vec<_>>(),
                ),
                vec![],
            )
            .orient(Direction::Default) // Normalize winding order
        })
        .collect();

    let outers: Vec<_> = outers
        .into_iter()
        .map(|x| {
            Polygon::new(
                LineString::from(
                    x.iter()
                        .map(|pt| (pt.x as f64, pt.z as f64))
                        .collect::<Vec<_>>(),
                ),
                vec![],
            )
            .orient(Direction::Default) // Normalize winding order
        })
        .collect();

    inverse_floodfill_recursive(
        (min_x, min_z),
        (max_x, max_z),
        &outers,
        &inners,
        editor,
        start_time,
    );
}

fn inverse_floodfill_recursive(
    min: (i32, i32),
    max: (i32, i32),
    outers: &[Polygon],
    inners: &[Polygon],
    editor: &mut WorldEditor,
    start_time: Instant,
) {
    // Check if we've exceeded 25 seconds
    if start_time.elapsed().as_secs() > 25 {
        println!("Water area generation exceeded 25 seconds, continuing anyway");
    }

    const ITERATIVE_THRES: i64 = 10_000;

    if min.0 > max.0 || min.1 > max.1 {
        return;
    }

    // Multiply as i64 to avoid overflow; in release builds where unchecked math is
    // enabled, this could cause the rest of this code to end up in an infinite loop.
    if ((max.0 - min.0) as i64) * ((max.1 - min.1) as i64) < ITERATIVE_THRES {
        inverse_floodfill_iterative(min, max, 0, outers, inners, editor);
        return;
    }

    let center_x: i32 = (min.0 + max.0) / 2;
    let center_z: i32 = (min.1 + max.1) / 2;
    let quadrants: [(i32, i32, i32, i32); 4] = [
        (min.0, center_x, min.1, center_z),
        (center_x, max.0, min.1, center_z),
        (min.0, center_x, center_z, max.1),
        (center_x, max.0, center_z, max.1),
    ];

    for (min_x, max_x, min_z, max_z) in quadrants {
        let rect: Rect = Rect::new(
            Point::new(min_x as f64, min_z as f64),
            Point::new(max_x as f64, max_z as f64),
        );

        if outers.iter().any(|outer: &Polygon| outer.contains(&rect))
            && !inners.iter().any(|inner: &Polygon| inner.intersects(&rect))
        {
            rect_fill(min_x, max_x, min_z, max_z, 0, editor);
            continue;
        }

        let outers_intersects: Vec<_> = outers
            .iter()
            .filter(|poly| poly.intersects(&rect))
            .cloned()
            .collect();
        let inners_intersects: Vec<_> = inners
            .iter()
            .filter(|poly| poly.intersects(&rect))
            .cloned()
            .collect();

        if !outers_intersects.is_empty() {
            inverse_floodfill_recursive(
                (min_x, min_z),
                (max_x, max_z),
                &outers_intersects,
                &inners_intersects,
                editor,
                start_time,
            );
        }
    }
}

// once we "zoom in" enough, it's more efficient to switch to iteration
fn inverse_floodfill_iterative(
    min: (i32, i32),
    max: (i32, i32),
    ground_level: i32,
    outers: &[Polygon],
    inners: &[Polygon],
    editor: &mut WorldEditor,
) {
    for x in min.0..max.0 {
        for z in min.1..max.1 {
            let p: Point = Point::new(x as f64, z as f64);

            if outers.iter().any(|poly: &Polygon| poly.contains(&p))
                && inners.iter().all(|poly: &Polygon| !poly.contains(&p))
            {
                editor.set_block(WATER, x, ground_level, z, None, None);
            }
        }
    }
}

fn rect_fill(
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    ground_level: i32,
    editor: &mut WorldEditor,
) {
    for x in min_x..max_x {
        for z in min_z..max_z {
            editor.set_block(WATER, x, ground_level, z, None, None);
        }
    }
}
