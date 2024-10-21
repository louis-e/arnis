use std::time::Duration;

use geo::{Contains, LineString, MultiPolygon, Point, Polygon};

use crate::{
    block_definitions::WATER,
    floodfill::flood_fill_area,
    osm_parser::{ProcessedMemberRole, ProcessedNode, ProcessedRelation},
    world_editor::WorldEditor,
};

pub fn generate_water_areas(
    editor: &mut WorldEditor,
    element: &ProcessedRelation,
    ground_level: i32,
    floodfill_timeout: Option<&Duration>,
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

    let mut outers = vec![];
    let mut inners = vec![];

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
    let outers = outers
        .iter()
        .map(|x| x.iter().map(|y| (y.x as f64, y.z as f64)).collect())
        .collect();
    let inners = inners
        .iter()
        .map(|x| x.iter().map(|y| (y.x as f64, y.z as f64)).collect())
        .collect();

    inverse_floodfill(max_x, max_z, outers, inners, editor, ground_level);
}

fn merge_loopy_loops(loops: &mut Vec<Vec<ProcessedNode>>) {
    let mut removed = vec![];
    let mut merged = vec![];

    for i in 0..loops.len() {
        for j in 0..loops.len() {
            if i == j {
                continue;
            }

            if removed.contains(&i) || removed.contains(&j) {
                continue;
            }

            let x = &loops[i];
            let y = &loops[j];

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

                let mut x = x.clone();
                x.reverse();
                x.extend(y.iter().skip(1).cloned());
                merged.push(x);
            } else if x.last().unwrap().id == y.last().unwrap().id {
                removed.push(i);
                removed.push(j);

                let mut x = x.clone();
                x.extend(y.iter().rev().skip(1).cloned());

                merged.push(x);
            } else if x[0].id == y.last().unwrap().id {
                removed.push(i);
                removed.push(j);

                let mut y = y.clone();
                y.extend(x.iter().skip(1).cloned());

                merged.push(y);
            }
        }
    }

    removed.sort();

    for r in removed.iter().rev() {
        loops.remove(*r);
    }

    let merged_len = merged.len();
    for m in merged {
        loops.push(m);
    }

    if merged_len > 0 {
        merge_loopy_loops(loops);
    }
}

fn verify_loopy_loops(loops: &[Vec<ProcessedNode>]) -> bool {
    let mut valid = true;
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
    let min_x = 0;
    let min_z = 0;

    let inners: Vec<_> = inners.into_iter().map(LineString::from).collect();

    let polygons = outers
        .into_iter()
        .map(|o| Polygon::new(LineString::from(o), inners.clone()));

    let multipolygon = MultiPolygon::from_iter(polygons);

    println!("inverse floodfill start");

    for x in min_x..max_x {
        for z in min_z..max_z {
            if multipolygon.contains(&Point::new(x as f64, z as f64)) {
                editor.set_block(&WATER, x, ground_level, z, None, None);
            }
        }
    }

    println!("inverse floodfill end");
}
