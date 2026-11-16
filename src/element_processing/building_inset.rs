//! Walls stepped in under a pitched roof's eave.
//!
//! Most outlines in OSM are traced from aerial imagery, so they follow the
//! roof edge, not the wall. A pitched roof hangs its eave a block out from
//! the walls, and with the walls on the outline that eave lands a block
//! outside it: every building grew a block on every side and every street
//! lost two (measured against laser roofs in Bavaria, where the scanned
//! roofs cover 0.95-1.01 of the OSM outline area). Here the free sides of
//! such a building step their walls in by one block, so the eave covers the
//! outline instead of the street. Party walls stay where they are, shared
//! with the neighbour.

use crate::bresenham::bresenham_line;
use crate::element_processing::buildings::{compute_outward_normal, outward_side};
use crate::floodfill::flood_fill_area;
use crate::floodfill_cache::CoordinateBitmap;
use crate::osm_parser::ProcessedNode;
use fnv::FnvHashSet;
use std::time::Duration;

/// Narrowest the stepped walls may stand apart, walls included, in blocks:
/// a shed or a garage would have little left inside. A terraced house
/// narrower than this keeps its width, as only its front and back step in.
pub(crate) const MIN_INSET_SPAN: i32 = 5;

/// Share of the footprint the walls may give up to the eave.
const MAX_INSET_LOSS: f64 = 0.5;

/// Two consecutive edges closer to straight than this (sine of the angle
/// between them) are one wall line, offset together.
const STRAIGHT_SINE: f64 = 0.17;

/// Farthest a moved corner may go from where it was, in blocks: a sharp
/// spike would otherwise shoot its mitred corner across the building.
const MAX_CORNER_SHIFT: f64 = 2.5;

/// A building's walls stepped in where the eave goes, and the cells inside them.
pub(crate) struct OutlineInset {
    /// The stepped ring, closed like the outline it came from.
    pub nodes: Vec<ProcessedNode>,
    /// Footprint inside the stepped walls, as the flood fill gives it.
    pub fill: Vec<(i32, i32)>,
    /// The outline's cells left outside the stepped walls, which the eave covers.
    pub strip: FnvHashSet<(i32, i32)>,
}

/// The cells of a ring's edges.
fn ring_cells(nodes: &[ProcessedNode]) -> impl Iterator<Item = (i32, i32)> + '_ {
    nodes.windows(2).flat_map(|w| {
        bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z)
            .into_iter()
            .map(|(x, _, z)| (x, z))
    })
}

/// Which edges of a closed ring stand free, with no other building within two
/// cells outward along most of them: those step in under the eave, the others
/// are party walls and stay on the outline. `own` is the building's own fill.
pub(crate) fn free_edges(
    nodes: &[ProcessedNode],
    own: &FnvHashSet<(i32, i32)>,
    footprints: &CoordinateBitmap,
) -> Vec<bool> {
    let outward = outward_side(nodes);
    nodes
        .windows(2)
        .map(|w| {
            let (a, b) = (&w[0], &w[1]);
            let (nx, nz) = compute_outward_normal(a.x, a.z, b.x, b.z, outward);
            if (nx, nz) == (0, 0) {
                return true;
            }
            let cells = bresenham_line(a.x, 0, a.z, b.x, 0, b.z);
            let party = cells
                .iter()
                .filter(|&&(x, _, z)| {
                    (1..=2).any(|d| {
                        let c = (x + nx * d, z + nz * d);
                        footprints.contains(c.0, c.1) && !own.contains(&c)
                    })
                })
                .count();
            party * 2 < cells.len()
        })
        .collect()
}

/// The ring with every edge flagged in `step` moved one block inward, the
/// corners where two moved edges meet mitred, and a step where a moved edge
/// runs on straight into one that stays. `step[i]` is the edge from
/// `nodes[i]` to `nodes[i + 1]`. None where the ring is open, degenerate or too
/// sharp to move cleanly.
pub(crate) fn inset_ring(nodes: &[ProcessedNode], step: &[bool]) -> Option<Vec<ProcessedNode>> {
    let n = nodes.len().checked_sub(1)?;
    if n < 3 || step.len() != n {
        return None;
    }
    let (first, last) = (&nodes[0], &nodes[n]);
    if (first.x, first.z) != (last.x, last.z) {
        return None;
    }
    // Distinct corners with the step flag of the edge leaving each.
    let mut corners: Vec<(usize, bool)> = Vec::with_capacity(n);
    for i in 0..n {
        let (a, b) = (&nodes[i], &nodes[i + 1]);
        if (a.x, a.z) == (b.x, b.z) {
            continue;
        }
        corners.push((i, step[i]));
    }
    let m = corners.len();
    if m < 3 {
        return None;
    }
    let outward = f64::from(outward_side(nodes));
    let at = |k: usize| {
        let node = &nodes[corners[k % m].0];
        (f64::from(node.x), f64::from(node.z))
    };
    // Unit direction and inward normal of the edge leaving corner k.
    let edge = |k: usize| {
        let (a, b) = (at(k), at(k + 1));
        let (dx, dz) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dz);
        (
            (dx / len, dz / len),
            (dz / len * outward, -dx / len * outward),
        )
    };
    let offset = |k: usize| if corners[k % m].1 { 1.0 } else { 0.0 };

    // Where each corner goes: (on the incoming edge's line, on the outgoing one).
    let mut moved: Vec<((f64, f64), (f64, f64))> = Vec::with_capacity(m);
    for k in 0..m {
        let v = at(k);
        let p = (k + m - 1) % m;
        let ((tpx, tpz), (npx, npz)) = edge(p);
        let ((tix, tiz), (nix, niz)) = edge(k);
        let (dp, di) = (offset(p), offset(k));
        let on_p = (v.0 + npx * dp, v.1 + npz * dp);
        let on_i = (v.0 + nix * di, v.1 + niz * di);
        let cross = tpx * tiz - tpz * tix;
        let dot = tpx * tix + tpz * tiz;
        if cross.abs() < STRAIGHT_SINE {
            if dot < 0.0 {
                // The ring doubles back on itself.
                return None;
            }
            moved.push((on_p, on_i));
            continue;
        }
        // Meet of the two offset lines.
        let s = ((on_i.0 - on_p.0) * tiz - (on_i.1 - on_p.1) * tix) / cross;
        let meet = (on_p.0 + tpx * s, on_p.1 + tpz * s);
        if (meet.0 - v.0).hypot(meet.1 - v.1) > MAX_CORNER_SHIFT {
            return None;
        }
        moved.push((meet, meet));
    }
    // A moved edge must still run the way it did: one shorter than the
    // inset collapses and turns round.
    for k in 0..m {
        let (t, _) = edge(k);
        let a = moved[k].1;
        let b = moved[(k + 1) % m].0;
        if (b.0 - a.0) * t.0 + (b.1 - a.1) * t.1 < -0.25 {
            return None;
        }
    }

    let mut out: Vec<ProcessedNode> = Vec::with_capacity(m + 4);
    let mut push = |node: &ProcessedNode, p: (f64, f64), tags: bool| {
        let (x, z) = (p.0.round() as i32, p.1.round() as i32);
        if let Some(prev) = out.last_mut() {
            if (prev.x, prev.z) == (x, z) {
                if tags && prev.tags.is_empty() {
                    prev.tags = node.tags.clone();
                }
                return;
            }
        }
        out.push(ProcessedNode {
            id: node.id,
            tags: if tags {
                node.tags.clone()
            } else {
                Default::default()
            },
            x,
            z,
        });
    };
    for (k, &(on_p, on_i)) in moved.iter().enumerate() {
        let node = &nodes[corners[k].0];
        push(node, on_p, true);
        push(node, on_i, false);
    }
    // A node part way along a straight wall (a mapped entrance) moved with
    // it and kept its tags above.
    let close = out.first()?.clone();
    if out.len() > 1 && (close.x, close.z) == (out[out.len() - 1].x, out[out.len() - 1].z) {
        out.pop();
    }
    if out.len() < 3 {
        return None;
    }
    out.push(close);
    Some(out)
}

/// The building's walls stepped in along its free edges, or None where it
/// keeps them on the outline: no edge free, stepped walls closer than `MIN_INSET_SPAN`, a
/// ring too sharp to move, or more than half the footprint lost. `fill` is the
/// outline's footprint (courtyards removed).
pub(crate) fn inset_outline(
    nodes: &[ProcessedNode],
    fill: &[(i32, i32)],
    footprints: &CoordinateBitmap,
    timeout: Option<&Duration>,
) -> Option<OutlineInset> {
    if fill.is_empty() {
        return None;
    }
    let own: FnvHashSet<(i32, i32)> = fill.iter().copied().collect();
    // The outline's own cells: the fill and the wall line, which a fill
    // need not cover where it runs off the grid.
    let mut outline = own.clone();
    outline.extend(ring_cells(nodes));
    let step = free_edges(nodes, &outline, footprints);
    if !step.iter().any(|&s| s) {
        return None;
    }
    let ring = inset_ring(nodes, &step)?;
    // Every wall stays inside the outline.
    if ring_cells(&ring).any(|c| !outline.contains(&c)) {
        return None;
    }
    let coords: Vec<(i32, i32)> = ring.iter().map(|n| (n.x, n.z)).collect();
    let inner: Vec<(i32, i32)> = flood_fill_area(&coords, timeout)
        .into_iter()
        .filter(|c| own.contains(c))
        .collect();
    // The stepped footprint, walls included, against the outline's.
    let mut stepped: FnvHashSet<(i32, i32)> = inner.iter().copied().collect();
    stepped.extend(ring_cells(&ring));
    let (mut min_x, mut max_x, mut min_z, mut max_z) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for &(x, z) in &stepped {
        (min_x, max_x) = (min_x.min(x), max_x.max(x));
        (min_z, max_z) = (min_z.min(z), max_z.max(z));
    }
    if inner.is_empty()
        || (max_x - min_x + 1).min(max_z - min_z + 1) < MIN_INSET_SPAN
        || (stepped.len() as f64) < (1.0 - MAX_INSET_LOSS) * outline.len() as f64
    {
        return None;
    }
    let strip: FnvHashSet<(i32, i32)> = outline.difference(&stepped).copied().collect();
    Some(OutlineInset {
        nodes: ring,
        fill: inner,
        strip,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{bitmap_with_rect, rect_way};

    fn bbox() -> XZBBox {
        XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap()
    }

    fn fill_of(nodes: &[ProcessedNode]) -> Vec<(i32, i32)> {
        let coords: Vec<(i32, i32)> = nodes.iter().map(|n| (n.x, n.z)).collect();
        flood_fill_area(&coords, None)
    }

    fn corners(nodes: &[ProcessedNode]) -> Vec<(i32, i32)> {
        nodes.iter().map(|n| (n.x, n.z)).collect()
    }

    #[test]
    fn a_free_standing_house_steps_in_one_block_on_every_side() {
        let way = rect_way(1, 10, 10, 20, 18, &[("building", "house")]);
        let fill = fill_of(&way.nodes);
        let footprints = bitmap_with_rect(&bbox(), 10, 10, 20, 18);
        let inset = inset_outline(&way.nodes, &fill, &footprints, None).unwrap();
        assert_eq!(
            corners(&inset.nodes),
            vec![(11, 11), (19, 11), (19, 17), (11, 17), (11, 11)]
        );
        // Inside the walls, as a flood fill gives it.
        assert_eq!(inset.fill.len(), 7 * 5);
        assert_eq!(inset.strip.len(), 11 * 9 - 9 * 7);
    }

    #[test]
    fn a_party_wall_stays_on_the_shared_line() {
        // A terraced house with its neighbour east of it, sharing x = 20.
        let way = rect_way(1, 10, 10, 20, 18, &[("building", "house")]);
        let fill = fill_of(&way.nodes);
        let mut footprints = bitmap_with_rect(&bbox(), 10, 10, 20, 18);
        for x in 20..=30 {
            for z in 10..=18 {
                footprints.set(x, z);
            }
        }
        let inset = inset_outline(&way.nodes, &fill, &footprints, None).unwrap();
        assert_eq!(
            corners(&inset.nodes),
            vec![(11, 11), (20, 11), (20, 17), (11, 17), (11, 11)]
        );
    }

    #[test]
    fn small_and_enclosed_buildings_keep_their_walls() {
        let shed = rect_way(1, 10, 10, 14, 20, &[("building", "shed")]);
        let fill = fill_of(&shed.nodes);
        let footprints = bitmap_with_rect(&bbox(), 10, 10, 14, 20);
        assert!(inset_outline(&shed.nodes, &fill, &footprints, None).is_none());

        let mid = rect_way(2, 10, 10, 20, 18, &[("building", "house")]);
        let fill = fill_of(&mid.nodes);
        let footprints = bitmap_with_rect(&bbox(), 0, 0, 40, 40);
        assert!(inset_outline(&mid.nodes, &fill, &footprints, None).is_none());
    }

    #[test]
    fn a_narrow_terraced_house_steps_in_front_and_back_only() {
        let way = rect_way(1, 10, 10, 15, 22, &[("building", "terrace")]);
        let fill = fill_of(&way.nodes);
        let footprints = bitmap_with_rect(&bbox(), 0, 10, 40, 22);
        let inset = inset_outline(&way.nodes, &fill, &footprints, None).unwrap();
        assert_eq!(
            corners(&inset.nodes),
            vec![(10, 11), (15, 11), (15, 21), (10, 21), (10, 11)]
        );
        // The eave covers the front and back rows the walls left.
        assert!(inset.strip.contains(&(12, 10)) && inset.strip.contains(&(12, 22)));
        assert!(!inset.strip.contains(&(10, 15)));
    }

    #[test]
    fn a_wall_running_on_past_a_neighbour_steps_where_it_leaves_it() {
        // Two straight edges along z = 10, the first party, the second free.
        let nodes: Vec<ProcessedNode> =
            [(10, 10), (20, 10), (30, 10), (30, 20), (10, 20), (10, 10)]
                .iter()
                .enumerate()
                .map(|(i, &(x, z))| ProcessedNode {
                    id: i as u64,
                    tags: Default::default(),
                    x,
                    z,
                })
                .collect();
        let ring = inset_ring(&nodes, &[false, true, true, true, true]).unwrap();
        assert_eq!(
            corners(&ring),
            vec![
                (11, 10),
                (20, 10),
                (20, 11),
                (29, 11),
                (29, 19),
                (11, 19),
                (11, 10)
            ]
        );
    }

    #[test]
    fn a_sliver_too_thin_to_step_in_is_left_alone() {
        let nodes: Vec<ProcessedNode> = [(10, 10), (40, 10), (40, 11), (10, 11), (10, 10)]
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: i as u64,
                tags: Default::default(),
                x,
                z,
            })
            .collect();
        assert!(inset_ring(&nodes, &[true; 4]).is_none());
    }
}
