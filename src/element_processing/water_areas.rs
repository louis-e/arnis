use geo::{Contains, Intersects, LineString, Point, Polygon, Rect};
use std::time::Instant;

use crate::{
    block_definitions::WATER,
    coordinate_system::cartesian::XZPoint,
    osm_parser::{ProcessedMemberRole, ProcessedNode, ProcessedRelation},
    world_editor::WorldEditor,
};

pub fn generate_water_areas(editor: &mut WorldEditor, element: &ProcessedRelation) {
    let start_time = Instant::now();

    if !element.tags.contains_key("water") {
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

    merge_loopy_loops(&mut outers);
    if !verify_loopy_loops(&outers) {
        return;
    }

    merge_loopy_loops(&mut inners);
    if !verify_loopy_loops(&inners) {
        return;
    }

    let outers: Vec<Vec<XZPoint>> = outers
        .iter()
        .map(|x| x.iter().map(|y| y.xz()).collect::<Vec<_>>())
        .collect();
    let inners: Vec<Vec<XZPoint>> = inners
        .iter()
        .map(|x| x.iter().map(|y| y.xz()).collect::<Vec<_>>())
        .collect();

    // Calculate the actual bounding box of the water area instead of using world bounds
    let (water_min_x, water_max_x, water_min_z, water_max_z) =
        calculate_water_area_bounds(&outers, &inners);

    // Get world bounds for intersection
    let (world_min_x, world_min_z) = editor.get_min_coords();
    let (world_max_x, world_max_z) = editor.get_max_coords();

    // Calculate intersection of water area with world bounds
    let min_x = water_min_x.max(world_min_x);
    let max_x = water_max_x.min(world_max_x);
    let min_z = water_min_z.max(world_min_z);
    let max_z = water_max_z.min(world_max_z);

    // Skip if no intersection or invalid bounds
    if min_x >= max_x || min_z >= max_z {
        println!("Water area does not intersect with world bounds, skipping");
        return;
    }

    // Skip processing if the intersected area is too large (> 10M blocks)
    let area_blocks = ((max_x - min_x) as i64) * ((max_z - min_z) as i64);
    if area_blocks > 10_000_000 {
        println!("Skipping large water area with {area_blocks} blocks (exceeds 10M limit)");
        return;
    }

    inverse_floodfill(
        min_x, min_z, max_x, max_z, outers, inners, editor, start_time,
    );
}

// Merges ways that share nodes into full loops
fn merge_loopy_loops(loops: &mut Vec<Vec<ProcessedNode>>) {
    let mut removed: Vec<usize> = vec![];
    let mut merged: Vec<Vec<ProcessedNode>> = vec![];

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

            // it's looped already
            if x[0].id == x.last().unwrap().id {
                continue;
            }

            // it's looped already
            if y[0].id == y.last().unwrap().id {
                continue;
            }

            if x[0].id == y[0].id {
                removed.push(i);
                removed.push(j);

                let mut x: Vec<ProcessedNode> = x.clone();
                x.reverse();
                x.extend(y.iter().skip(1).cloned());
                merged.push(x);
            } else if x.last().unwrap().id == y.last().unwrap().id {
                removed.push(i);
                removed.push(j);

                let mut x: Vec<ProcessedNode> = x.clone();
                x.extend(y.iter().rev().skip(1).cloned());

                merged.push(x);
            } else if x[0].id == y.last().unwrap().id {
                removed.push(i);
                removed.push(j);

                let mut y: Vec<ProcessedNode> = y.clone();
                y.extend(x.iter().skip(1).cloned());

                merged.push(y);
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
        if l[0].id != l.last().unwrap().id {
            eprintln!("WARN: Disconnected loop");
            valid = false;
        }
    }

    valid
}

// Calculate the actual bounding box of the water area from its coordinates
fn calculate_water_area_bounds(
    outers: &[Vec<XZPoint>],
    inners: &[Vec<XZPoint>],
) -> (i32, i32, i32, i32) {
    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_z = i32::MAX;
    let mut max_z = i32::MIN;

    // Process outer boundaries
    for outer in outers {
        for point in outer {
            min_x = min_x.min(point.x);
            max_x = max_x.max(point.x);
            min_z = min_z.min(point.z);
            max_z = max_z.max(point.z);
        }
    }

    // Process inner boundaries (holes)
    for inner in inners {
        for point in inner {
            min_x = min_x.min(point.x);
            max_x = max_x.max(point.x);
            min_z = min_z.min(point.z);
            max_z = max_z.max(point.z);
        }
    }

    // If no coordinates found, return zero bounds
    if min_x == i32::MAX {
        return (0, 0, 0, 0);
    }

    (min_x, max_x, min_z, max_z)
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

    let area_size = ((max.0 - min.0) as i64) * ((max.1 - min.1) as i64);

    // Skip processing if this sub-area is excessively large (> 1M blocks)
    if area_size > 1_000_000 {
        println!("Skipping large water sub-area with {area_size} blocks");
        return;
    }

    // Multiply as i64 to avoid overflow; in release builds where unchecked math is
    // enabled, this could cause the rest of this code to end up in an infinite loop.
    if area_size < ITERATIVE_THRES {
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
    let area_size = ((max.0 - min.0) as i64) * ((max.1 - min.1) as i64);

    // Use sampling for very large areas to avoid performance issues
    let step = if area_size > 100_000 {
        // For areas > 100k blocks, use every 4th block
        4
    } else if area_size > 10_000 {
        // For areas > 10k blocks, use every 2nd block
        2
    } else {
        // For smaller areas, process every block
        1
    };

    for x in (min.0..max.0).step_by(step as usize) {
        for z in (min.1..max.1).step_by(step as usize) {
            let p: Point = Point::new(x as f64, z as f64);

            if outers.iter().any(|poly: &Polygon| poly.contains(&p))
                && inners.iter().all(|poly: &Polygon| !poly.contains(&p))
            {
                if step == 1 {
                    // Normal processing for small areas
                    editor.set_block(WATER, x, ground_level, z, None, None);
                } else {
                    // Fill a small area for sampled processing
                    for dx in 0..step {
                        for dz in 0..step {
                            let fill_x = x + dx;
                            let fill_z = z + dz;
                            if fill_x < max.0 && fill_z < max.1 {
                                editor.set_block(WATER, fill_x, ground_level, fill_z, None, None);
                            }
                        }
                    }
                }
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
