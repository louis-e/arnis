pub mod amenities;
pub mod barriers;
pub mod boundaries;
pub mod bridges;
pub mod buildings;
pub mod doors;
pub mod highways;
pub mod landuse;
pub mod leisure;
pub mod man_made;
pub mod natural;
pub mod railways;
pub mod subprocessor;
pub mod tourisms;
pub mod tree;
pub mod water_areas;
pub mod waterways;

use crate::osm_parser::ProcessedNode;

/// Merges way segments that share endpoints into closed rings.
/// Used by water_areas.rs and boundaries.rs for assembling relation members.
pub fn merge_way_segments(rings: &mut Vec<Vec<ProcessedNode>>) {
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
