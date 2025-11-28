use geo::orient::{Direction, Orient};
use geo::{Contains, Intersects, LineString, Point, Polygon, Rect};
use std::time::Instant;

use crate::clipping::clip_water_ring_to_bbox;
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
    if !verify_closed_rings(&outers) {
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

    // Preserve OSM-defined outer/inner roles without modification
    merge_way_segments(&mut outers);

    // Clip assembled rings to bbox (must happen after merging to preserve ring connectivity)
    outers = outers
        .into_iter()
        .filter_map(|ring| clip_water_ring_to_bbox(&ring, xzbbox))
        .collect();
    merge_way_segments(&mut inners);
    inners = inners
        .into_iter()
        .filter_map(|ring| clip_water_ring_to_bbox(&ring, xzbbox))
        .collect();

    if !verify_closed_rings(&outers) {
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
        if !verify_closed_rings(&outers) {
            println!("Skipping relation {} due to invalid polygon", element.id);
            return;
        }
    }

    merge_way_segments(&mut inners);
    if !verify_closed_rings(&inners) {
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
    // Calculate polygon bounding box to limit fill area
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

/// Merges way segments that share endpoints into closed rings.
fn merge_way_segments(rings: &mut Vec<Vec<ProcessedNode>>) {
    let mut removed: Vec<usize> = vec![];
    let mut merged: Vec<Vec<ProcessedNode>> = vec![];

    // Match nodes by ID or proximity (handles synthetic nodes from bbox clipping)
    let nodes_match = |a: &ProcessedNode, b: &ProcessedNode| -> bool {
        if a.id == b.id {
            return true;
        }
        let dx = (a.x - b.x).abs();
        let dz = (a.z - b.z).abs();
        dx <= 1 && dz <= 1
    };

    for i in 0..rings.len() {
        for j in 0..rings.len() {
            if i == j {
                continue;
            }

            if removed.contains(&i) || removed.contains(&j) {
                continue;
            }

            let x: &Vec<ProcessedNode> = &rings[i];
            let y: &Vec<ProcessedNode> = &rings[j];

            // Skip empty rings (can happen after clipping)
            if x.is_empty() || y.is_empty() {
                continue;
            }

            let x_first = &x[0];
            let x_last = x.last().unwrap();
            let y_first = &y[0];
            let y_last = y.last().unwrap();

            // Skip already-closed rings
            if nodes_match(x_first, x_last) {
                continue;
            }

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
        rings.remove(*r);
    }

    let merged_len: usize = merged.len();
    for m in merged {
        rings.push(m);
    }

    if merged_len > 0 {
        merge_way_segments(rings);
    }
}

/// Verifies all rings are properly closed (first node matches last).
fn verify_closed_rings(rings: &[Vec<ProcessedNode>]) -> bool {
    let mut valid = true;
    for ring in rings {
        let first = &ring[0];
        let last = ring.last().unwrap();

        // Check if ring is closed (by ID or proximity)
        let is_closed = first.id == last.id || {
            let dx = (first.x - last.x).abs();
            let dz = (first.z - last.z).abs();
            dx <= 1 && dz <= 1
        };

        if !is_closed {
            eprintln!("WARN: Disconnected ring");
            valid = false;
        }
    }

    valid
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
    // Convert to geo Polygons with normalized winding order
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
            .orient(Direction::Default)
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
            .orient(Direction::Default)
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
