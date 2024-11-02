use geo::{Contains, Intersects, LineString, Point, Polygon, Rect};

use crate::{
    block_definitions::WATER,
    osm_parser::{ProcessedMemberRole, ProcessedNode, ProcessedRelation},
    world_editor::WorldEditor,
};

pub fn generate_water_areas(
    editor: &mut WorldEditor,
    element: &ProcessedRelation,
    ground_level: i32,
) {
    if !element.tags.contains_key("water") {
        return;
    }

    // don't handle water below layer 0
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

    let (max_x, max_z) = editor.get_max_coords();
    let outers: Vec<Vec<(f64, f64)>> = outers
        .iter()
        .map(|x: &Vec<ProcessedNode>| x.iter().map(|y: &ProcessedNode| (y.x as f64, y.z as f64)).collect())
        .collect();
    let inners: Vec<Vec<(f64, f64)>> = inners
        .iter()
        .map(|x: &Vec<ProcessedNode>| x.iter().map(|y: &ProcessedNode| (y.x as f64, y.z as f64)).collect())
        .collect();

    inverse_floodfill(max_x, max_z, outers, inners, editor, ground_level);
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

// Water areas are absolutely huge. We can't easily flood fill the entire thing.
// Instead, we'll iterate over all the blocks in our MC world, and check if each
// one is in the river or not
fn inverse_floodfill(
    max_x: i32,
    max_z: i32,
    outers: Vec<Vec<(f64, f64)>>,
    inners: Vec<Vec<(f64, f64)>>,
    editor: &mut WorldEditor,
    ground_level: i32,
) {
    let min_x: i32 = 0;
    let min_z: i32 = 0;

    let inners: Vec<_> = inners
        .into_iter()
        .map(|x: Vec<(f64, f64)>| Polygon::new(LineString::from(x), vec![]))
        .collect();

    let outers: Vec<_> = outers
        .into_iter()
        .map(|x: Vec<(f64, f64)>| Polygon::new(LineString::from(x), vec![]))
        .collect();

    inverse_floodfill_recursive(
        (min_x, min_z),
        (max_x, max_z),
        ground_level,
        &outers,
        &inners,
        editor,
    );
}

fn inverse_floodfill_recursive(
    min: (i32, i32),
    max: (i32, i32),
    ground_level: i32,
    outers: &[Polygon],
    inners: &[Polygon],
    editor: &mut WorldEditor,
) {
    const ITERATIVE_THRES: i32 = 10_000;

    if min.0 > max.0 || min.1 > max.1 {
        return;
    }

    if (max.0 - min.0) * (max.1 - min.1) < ITERATIVE_THRES {
        inverse_floodfill_iterative(min, max, ground_level, outers, inners, editor);

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
            // every block in rect is water
            // so we can safely just set the whole thing to water

            rect_fill(min_x, max_x, min_z, max_z, ground_level, editor);

            continue;
        }

        // When we recurse, we only really need the polygons we potentially intersect with
        // This saves on processing time
        let outers_intersects: Vec<_> = outers
            .iter()
            .filter(|poly: &&Polygon| poly.intersects(&rect))
            .cloned()
            .collect();

        // Moving this inside the below `if` statement makes it slower for some reason.
        // I assume it changes how the compiler is able to optimize it
        let inners_intersects: Vec<_> = inners
            .iter()
            .filter(|poly: &&Polygon| poly.intersects(&rect))
            .cloned()
            .collect();

        if !outers_intersects.is_empty() {
            // recurse

            inverse_floodfill_recursive(
                (min_x, min_z),
                (max_x, max_z),
                ground_level,
                &outers_intersects,
                &inners_intersects,
                editor,
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
