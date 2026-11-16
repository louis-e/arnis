//! Sheds, garages and pools from the survey as ordinary elements.
//!
//! They are built by the same generators as mapped ones, so they get real roofs,
//! pool coping and every edition's blocks, and they meet OSM the way Overture
//! buildings do: a survey shed whose outline reaches any mapped or Overture
//! building is dropped, a survey pool that reaches a mapped pool or any building
//! is dropped. OSM always wins.

use super::grid::{label, shapes};
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use std::collections::HashMap;

/// Id range of the survey's elements, clear of OSM's and of Overture's hashes.
const ID_BASE: u64 = 0x7A00_0000_0000_0000;

/// The survey's sheds and pools in the area, not yet checked against anything.
pub fn candidates(ground: &Ground, xzbbox: &XZBBox) -> Vec<ProcessedElement> {
    if !ground.has_survey() {
        return Vec::new();
    }
    let (x0, z0) = (xzbbox.min_x(), xzbbox.min_z());
    let w = (xzbbox.max_x() - x0 + 1).max(0) as usize;
    let h = (xzbbox.max_z() - z0 + 1).max(0) as usize;
    let mut out = Vec::new();
    let mut next_id = ID_BASE;
    for kind in [super::Kind::Shed, super::Kind::Pool] {
        let mut mask = vec![false; w * h];
        let mut height = vec![0.0f32; w * h];
        for z in 0..h {
            for x in 0..w {
                if let Some(c) = ground.survey_cell(XZPoint::new(x as i32, z as i32)) {
                    if c.kind == kind {
                        mask[z * w + x] = true;
                        height[z * w + x] = c.height;
                    }
                }
            }
        }
        let (lab, n) = label(&mask, w, h, false);
        if n == 0 {
            continue;
        }
        let sh = shapes(&lab, n, w);
        let mut cells: Vec<Vec<(i32, i32)>> = vec![Vec::new(); n as usize + 1];
        let mut heights: Vec<Vec<f32>> = vec![Vec::new(); n as usize + 1];
        for (i, &l) in lab.iter().enumerate() {
            if l != 0 {
                cells[l as usize].push(((i % w) as i32, (i / w) as i32));
                heights[l as usize].push(height[i]);
            }
        }
        for l in 1..=n as usize {
            // Sampling the metre grid onto blocks can leave a sliver; the survey's
            // own size limits were set on the metre grid.
            if sh[l].area < 4.0 {
                continue;
            }
            let ring = hull(&cells[l]);
            if ring.len() < 3 {
                continue;
            }
            let mut tags = HashMap::new();
            match kind {
                super::Kind::Shed => {
                    let mut hs = heights[l].clone();
                    hs.sort_by(f32::total_cmp);
                    let tall = hs[hs.len() / 2].clamp(2.0, 5.0);
                    let garage = sh[l].area >= 15.0 && tall >= 2.2;
                    tags.insert(
                        "building".to_string(),
                        if garage { "garage" } else { "shed" }.to_string(),
                    );
                    tags.insert("height".to_string(), format!("{tall:.1}"));
                }
                super::Kind::Pool => {
                    tags.insert("leisure".to_string(), "swimming_pool".to_string());
                    tags.insert("access".to_string(), "private".to_string());
                }
                _ => unreachable!(),
            }
            tags.insert("source".to_string(), "garden_detail".to_string());
            let id = next_id;
            next_id += 1 << 8;
            let mut nodes: Vec<ProcessedNode> = ring
                .iter()
                .enumerate()
                .map(|(k, &(x, z))| ProcessedNode {
                    id: id + 1 + k as u64,
                    tags: HashMap::new(),
                    x: x0 + x,
                    z: z0 + z,
                })
                .collect();
            let mut first = nodes[0].clone();
            first.id = id + 1;
            nodes.push(first);
            out.push(ProcessedElement::Way(ProcessedWay { id, nodes, tags }));
        }
    }
    out
}

/// Keep the candidates that meet nothing mapped: sheds clear of every building,
/// pools clear of every building and every mapped pool.
pub fn merge(
    candidates: Vec<ProcessedElement>,
    existing: &[ProcessedElement],
) -> Vec<ProcessedElement> {
    let bbox_of = |w: &ProcessedWay| {
        let xs = w.nodes.iter().map(|n| n.x);
        let zs = w.nodes.iter().map(|n| n.z);
        (
            xs.clone().min().unwrap_or(0),
            zs.clone().min().unwrap_or(0),
            xs.max().unwrap_or(0),
            zs.max().unwrap_or(0),
        )
    };
    let mut blockers: Vec<(i32, i32, i32, i32, bool)> = Vec::new();
    for el in existing {
        let mut push = |w: &ProcessedWay, building: bool| {
            if w.nodes.len() >= 3 {
                let (a, b, c, d) = bbox_of(w);
                blockers.push((a, b, c, d, building));
            }
        };
        let tags = el.tags();
        let building = tags.contains_key("building") || tags.contains_key("building:part");
        let pool = tags.get("leisure").map(String::as_str) == Some("swimming_pool");
        if !building && !pool {
            continue;
        }
        match el {
            ProcessedElement::Way(w) => push(w, building),
            ProcessedElement::Relation(r) => {
                for m in &r.members {
                    push(&m.way, building);
                }
            }
            ProcessedElement::Node(_) => {}
        }
    }
    candidates
        .into_iter()
        .filter(|el| {
            let ProcessedElement::Way(w) = el else {
                return false;
            };
            let is_pool = w.tags.contains_key("leisure");
            let (a, b, c, d) = bbox_of(w);
            !blockers.iter().any(|&(ba, bb, bc, bd, building)| {
                let overlaps = a <= bc && c >= ba && b <= bd && d >= bb;
                // A shed only yields to buildings; a pool to buildings and pools.
                overlaps && (building || is_pool)
            })
        })
        .collect()
}

/// Convex hull of the cells, counter-clockwise, through block coordinates: an
/// outline's nodes are blocks of its wall, so a shed four blocks wide runs from
/// its first block to its fourth. For a rectangular shed that is its rectangle;
/// for a round pool, its outline.
fn hull(cells: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut pts: Vec<(i32, i32)> = cells.to_vec();
    pts.sort_unstable();
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: (i32, i32), a: (i32, i32), b: (i32, i32)| {
        i64::from(a.0 - o.0) * i64::from(b.1 - o.1) - i64::from(a.1 - o.1) * i64::from(b.0 - o.0)
    };
    let mut lower: Vec<(i32, i32)> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<(i32, i32)> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_of_a_block_rectangle_is_its_corners() {
        let cells: Vec<(i32, i32)> = (0..4).flat_map(|x| (0..3).map(move |z| (x, z))).collect();
        let mut h = hull(&cells);
        h.sort_unstable();
        assert_eq!(h, vec![(0, 0), (0, 2), (3, 0), (3, 2)]);
    }

    fn way(id: u64, tags: &[(&str, &str)], x0: i32, z0: i32, x1: i32, z1: i32) -> ProcessedElement {
        let node = |k: u64, x: i32, z: i32| ProcessedNode {
            id: id + k,
            tags: HashMap::new(),
            x,
            z,
        };
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes: vec![
                node(1, x0, z0),
                node(2, x1, z0),
                node(3, x1, z1),
                node(4, x0, z1),
                node(1, x0, z0),
            ],
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    #[test]
    fn mapped_buildings_and_pools_win() {
        let existing = vec![
            way(1, &[("building", "house")], 0, 0, 10, 10),
            way(2, &[("leisure", "swimming_pool")], 30, 30, 36, 34),
            way(3, &[("highway", "residential")], 50, 0, 50, 40),
        ];
        let cands = vec![
            way(ID_BASE, &[("building", "shed")], 9, 9, 12, 12), // touches the house
            way(ID_BASE + 256, &[("building", "shed")], 20, 20, 23, 23), // free
            way(
                ID_BASE + 512,
                &[("leisure", "swimming_pool")],
                31,
                31,
                34,
                33,
            ), // mapped
            way(
                ID_BASE + 768,
                &[("leisure", "swimming_pool")],
                40,
                40,
                45,
                44,
            ), // free
            way(ID_BASE + 1024, &[("building", "shed")], 32, 31, 34, 33), // on a mapped pool
        ];
        let kept: Vec<u64> = merge(cands, &existing)
            .into_iter()
            .map(|e| match e {
                ProcessedElement::Way(w) => w.id,
                _ => 0,
            })
            .collect();
        assert_eq!(kept, vec![ID_BASE + 256, ID_BASE + 768, ID_BASE + 1024]);
    }
}
