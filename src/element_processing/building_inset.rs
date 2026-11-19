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

/// Narrowest the stepped walls may stand apart, walls included, in blocks:
/// a shed or a garage would have little left inside. A terraced house
/// narrower than this keeps its width, as only its front and back step in.
pub(crate) const MIN_INSET_SPAN: i32 = 5;

/// Fewest rings of cells, from the edge in to the middle, a building the step
/// made narrower must keep: nine blocks across, walls included. A detached
/// house a block in on every side read as a narrow tower under its roof and
/// lost the door on its gable end, so below this it keeps its walls and the
/// eave hangs past them, as before the step.
pub(crate) const MIN_SHRUNK_RINGS: i32 = 5;

/// Fewest blocks per metre the walls step in at. Coarser, a block of eave is
/// more than a real one and the step eats small buildings and narrow wings,
/// so the walls stay on the outline.
pub(crate) const MIN_INSET_SCALE: f64 = 0.75;

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
pub(crate) fn ring_cells(nodes: &[ProcessedNode]) -> impl Iterator<Item = (i32, i32)> + '_ {
    nodes.windows(2).flat_map(|w| {
        bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z)
            .into_iter()
            .map(|(x, _, z)| (x, z))
    })
}

/// Which edges of a closed ring stand free, with no other building within two
/// cells outward anywhere along them: those step in under the eave. The
/// others are party walls, or run past a neighbour for part of their length,
/// and stay on the outline, so no gap opens beside the neighbour. `own` is
/// the building's own cells, its outline included.
///
/// `footprints` holds what the flood fills give, the inside of each outline
/// without its wall line, so a neighbour's wall two cells out is found by the
/// inside of it beside that cell. Two houses a cell apart both stepped in,
/// three cells between their walls, and their eaves left a slot open to the
/// sky.
pub(crate) fn free_edges(
    nodes: &[ProcessedNode],
    own: &FnvHashSet<(i32, i32)>,
    footprints: &CoordinateBitmap,
) -> Vec<bool> {
    free_edges_by(nodes, own, footprints, true)
}

/// `free_edges`, finding a neighbour's wall line two cells out by the inside
/// beside it where `walls` is set, by its inside alone where it is not.
fn free_edges_by(
    nodes: &[ProcessedNode],
    own: &FnvHashSet<(i32, i32)>,
    footprints: &CoordinateBitmap,
    walls: bool,
) -> Vec<bool> {
    const SIDES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    let outward = outward_side(nodes);
    let taken = |c: (i32, i32)| footprints.contains(c.0, c.1) && !own.contains(&c);
    nodes
        .windows(2)
        .map(|w| {
            let (a, b) = (&w[0], &w[1]);
            let (nx, nz) = compute_outward_normal(a.x, a.z, b.x, b.z, outward);
            if (nx, nz) == (0, 0) {
                return true;
            }
            !bresenham_line(a.x, 0, a.z, b.x, 0, b.z)
                .iter()
                .any(|&(x, _, z)| {
                    (1..=2).any(|d| {
                        let c = (x + nx * d, z + nz * d);
                        taken(c)
                            || (walls
                                && d == 2
                                && SIDES.iter().any(|&(sx, sz)| taken((c.0 + sx, c.1 + sz))))
                    })
                })
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

/// Rings of cells from the edge of a set in to its middle: half its narrowest
/// width, rounded up, whichever way the outline is turned; no more than `cap`,
/// where peeling stops.
fn rings(cells: &FnvHashSet<(i32, i32)>, cap: i32) -> i32 {
    let Some((min_x, min_z, max_x, max_z)) = cells.iter().fold(None, |b, &(x, z)| {
        Some(
            b.map_or((x, z, x, z), |(x0, z0, x1, z1): (i32, i32, i32, i32)| {
                (x0.min(x), z0.min(z), x1.max(x), z1.max(z))
            }),
        )
    }) else {
        return 0;
    };
    // The set on a grid over its bounds, with a free border so every cell has four sides.
    let w = (max_x - min_x + 3) as usize;
    let h = (max_z - min_z + 3) as usize;
    let mut inside = vec![false; w * h];
    for &(x, z) in cells {
        inside[(z - min_z + 1) as usize * w + (x - min_x + 1) as usize] = true;
    }
    let sides = [1, w, 0usize.wrapping_sub(1), 0usize.wrapping_sub(w)];
    let near = |i: usize| sides.map(|s| i.wrapping_add(s));
    let mut ring: Vec<usize> = (0..w * h)
        .filter(|&i| inside[i] && near(i).iter().any(|&n| !inside[n]))
        .collect();
    let mut seen = vec![false; w * h];
    for &i in &ring {
        seen[i] = true;
    }
    let mut depth = 0;
    while !ring.is_empty() && depth < cap {
        depth += 1;
        let mut inner = Vec::new();
        for &i in &ring {
            for n in near(i) {
                if inside[n] && !seen[n] {
                    seen[n] = true;
                    inner.push(n);
                }
            }
        }
        ring = inner;
    }
    depth
}

/// Width along x and along z of a set of cells, in blocks.
fn spans(cells: &FnvHashSet<(i32, i32)>) -> (i32, i32) {
    let (mut min_x, mut max_x, mut min_z, mut max_z) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for &(x, z) in cells {
        (min_x, max_x) = (min_x.min(x), max_x.max(x));
        (min_z, max_z) = (min_z.min(z), max_z.max(z));
    }
    (max_x - min_x + 1, max_z - min_z + 1)
}

/// The building's walls stepped in along its free edges, or None where it
/// keeps them on the outline: no edge free, stepped walls closer than `MIN_INSET_SPAN`, a
/// small building made narrower than `MIN_SHRUNK_RINGS`, a wing so
/// narrow its walls would meet with no room between, a ring too sharp to move,
/// or more than half the footprint lost. `fill` is the outline's footprint
/// (courtyards removed).
pub(crate) fn inset_outline(
    nodes: &[ProcessedNode],
    fill: &[(i32, i32)],
    footprints: &CoordinateBitmap,
) -> Option<OutlineInset> {
    let (own, outline) = own_and_outline(nodes, fill)?;
    decide(nodes, &own, &outline, footprints, &[]).map(|(_, inset)| inset)
}

/// Which edges `inset_outline` steps in, with the edges flagged in `off` kept
/// on the outline whatever their neighbours: None where it keeps them all.
/// Buildings sharing a wall decide together, a front that runs on past a
/// party wall stepping on both sides of it or on neither.
pub(crate) fn step_plan(
    nodes: &[ProcessedNode],
    fill: &[(i32, i32)],
    footprints: &CoordinateBitmap,
    off: &[bool],
) -> Option<Vec<bool>> {
    let (own, outline) = own_and_outline(nodes, fill)?;
    decide(nodes, &own, &outline, footprints, off).map(|(step, _)| step)
}

/// The walls stepped in along the edges a `step_plan` gave.
pub(crate) fn inset_planned(
    nodes: &[ProcessedNode],
    fill: &[(i32, i32)],
    step: &[bool],
) -> Option<OutlineInset> {
    let (own, outline) = own_and_outline(nodes, fill)?;
    inset_with(nodes, &own, &outline, step)
}

/// The fill as a set, and the outline's own cells: the fill and the wall
/// line, which a fill need not cover where it runs off the grid.
#[allow(clippy::type_complexity)]
fn own_and_outline(
    nodes: &[ProcessedNode],
    fill: &[(i32, i32)],
) -> Option<(FnvHashSet<(i32, i32)>, FnvHashSet<(i32, i32)>)> {
    if fill.is_empty() {
        return None;
    }
    let own: FnvHashSet<(i32, i32)> = fill.iter().copied().collect();
    let mut outline = own.clone();
    outline.extend(ring_cells(nodes));
    Some((own, outline))
}

/// The edges that step and the walls they give, edges in `off` kept.
fn decide(
    nodes: &[ProcessedNode],
    own: &FnvHashSet<(i32, i32)>,
    outline: &FnvHashSet<(i32, i32)>,
    footprints: &CoordinateBitmap,
    off: &[bool],
) -> Option<(Vec<bool>, OutlineInset)> {
    let masked = |mut flags: Vec<bool>| {
        for (f, &o) in flags.iter_mut().zip(off) {
            *f &= !o;
        }
        flags
    };
    // A building steps in only where it does with its neighbours read from
    // their insides alone, so keeping a wall a cell from a neighbour on the
    // line never lets one that kept all its walls step some of them in.
    let near_free = free_edges_by(nodes, outline, footprints, false);
    let near = masked(near_free.clone());
    let step = masked(free_edges(nodes, outline, footprints));
    let attempt = |near: &[bool], step: &[bool]| -> Option<(Vec<bool>, OutlineInset)> {
        let first = inset_with(nodes, own, outline, near)?;
        if step == near {
            return Some((near.to_vec(), first));
        }
        inset_with(nodes, own, outline, step).map(|inset| (step.to_vec(), inset))
    };
    attempt(&near, &step).or_else(|| {
        // A house against its neighbour that the step would make too
        // narrow keeps its width, as a terraced one does: only the walls
        // across the party wall, its front and back, step in. Its twin of a
        // block more steps there too, and the pair reads as one.
        let keep = width_kept(nodes, &near_free);
        if !keep.iter().any(|&k| k) {
            return None;
        }
        let kept = |flags: &[bool]| -> Vec<bool> {
            flags.iter().zip(&keep).map(|(&f, &k)| f && !k).collect()
        };
        attempt(&kept(&near), &kept(&step))
    })
}

/// Edges that run alongside a party wall (an edge not free in `free`): the
/// far gable of a house against its neighbour, which keeps its width when
/// it stays on the outline.
fn width_kept(nodes: &[ProcessedNode], free: &[bool]) -> Vec<bool> {
    let dirs: Vec<Option<(f64, f64)>> = nodes
        .windows(2)
        .map(|w| {
            let (dx, dz) = (f64::from(w[1].x - w[0].x), f64::from(w[1].z - w[0].z));
            let len = dx.hypot(dz);
            (len > 0.0).then(|| (dx / len, dz / len))
        })
        .collect();
    let party: Vec<(f64, f64)> = dirs
        .iter()
        .zip(free)
        .filter(|(_, &f)| !f)
        .filter_map(|(d, _)| *d)
        .collect();
    dirs.iter()
        .zip(free)
        .map(|(d, &f)| {
            f && d.is_some_and(|(x, z)| {
                party
                    .iter()
                    .any(|&(px, pz)| (x * pz - z * px).abs() < STRAIGHT_SINE)
            })
        })
        .collect()
}

/// The walls stepped in along the edges flagged in `step`, or None where the
/// rules of `inset_outline` keep them on the outline.
fn inset_with(
    nodes: &[ProcessedNode],
    own: &FnvHashSet<(i32, i32)>,
    outline: &FnvHashSet<(i32, i32)>,
    step: &[bool],
) -> Option<OutlineInset> {
    if !step.iter().any(|&s| s) {
        return None;
    }
    let ring = inset_ring(nodes, step)?;
    // Every wall stays inside the outline.
    if ring_cells(&ring).any(|c| !outline.contains(&c)) {
        return None;
    }
    // The stepped footprint spans what its corners do, its walls running between them and
    // the fill inside them, so one narrower than `MIN_INSET_SPAN` is turned down unfilled.
    let corner_spans = |axis: fn(&ProcessedNode) -> i32| {
        let (lo, hi) = ring
            .iter()
            .map(axis)
            .fold((i32::MAX, i32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        hi - lo + 1
    };
    if corner_spans(|n| n.x).min(corner_spans(|n| n.z)) < MIN_INSET_SPAN {
        return None;
    }
    let coords: Vec<(i32, i32)> = ring.iter().map(|n| (n.x, n.z)).collect();
    // No wall-clock timeout: the ring is no bigger than the outline, whose
    // fill is already cached, and a fill cut short in one tile but not the
    // next would step half a building in.
    let inner: Vec<(i32, i32)> = flood_fill_area(&coords, None)
        .into_iter()
        .filter(|c| own.contains(c))
        .collect();
    // The stepped footprint, walls included, against the outline's.
    let walls: FnvHashSet<(i32, i32)> = ring_cells(&ring).collect();
    let mut stepped: FnvHashSet<(i32, i32)> = inner.iter().copied().collect();
    // A wall with no room beside it is a fin: the two sides of a wing three
    // or four blocks wide stepped in onto each other.
    let fin = walls.iter().any(|&(x, z)| {
        !(-1..=1).any(|dx| {
            (-1..=1).any(|dz| {
                let c = (x + dx, z + dz);
                stepped.contains(&c) && !walls.contains(&c)
            })
        })
    });
    stepped.extend(walls);
    let (sx, sz) = spans(&stepped);
    // Made narrower than a house the step suits.
    // Only how the two compare below `MIN_SHRUNK_RINGS` counts, so neither is peeled past it.
    let stepped_rings = rings(&stepped, MIN_SHRUNK_RINGS);
    let cramped =
        stepped_rings < MIN_SHRUNK_RINGS && stepped_rings < rings(outline, stepped_rings + 1);
    if inner.is_empty()
        || fin
        || cramped
        || sx.min(sz) < MIN_INSET_SPAN
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

    /// Rings peel a set from its edge in, half its narrowest width rounded up, a wing
    /// as narrow as it is, and stop at the cap.
    #[test]
    fn rings_count_from_the_edge_in() {
        let rect = |x0: i32, z0: i32, x1: i32, z1: i32| -> FnvHashSet<(i32, i32)> {
            (x0..=x1)
                .flat_map(|x| (z0..=z1).map(move |z| (x, z)))
                .collect()
        };
        assert_eq!(rings(&rect(0, 0, 8, 4), 99), 3);
        assert_eq!(rings(&rect(-5, -5, 14, 14), 99), 10);
        assert_eq!(rings(&rect(-5, -5, 14, 14), 4), 4);
        // An L whose arm is three wide and whose body is twelve.
        let mut l = rect(0, 0, 11, 11);
        l.extend(rect(12, 0, 30, 2));
        assert_eq!(rings(&l, 99), 6);
        assert_eq!(rings(&rect(3, 3, 3, 3), 99), 1);
        assert_eq!(rings(&FnvHashSet::default(), 99), 0);
    }

    fn corners(nodes: &[ProcessedNode]) -> Vec<(i32, i32)> {
        nodes.iter().map(|n| (n.x, n.z)).collect()
    }

    #[test]
    fn a_free_standing_house_steps_in_one_block_on_every_side() {
        let way = rect_way(1, 10, 10, 24, 22, &[("building", "house")]);
        let fill = fill_of(&way.nodes);
        let footprints = bitmap_with_rect(&bbox(), 10, 10, 24, 22);
        let inset = inset_outline(&way.nodes, &fill, &footprints).unwrap();
        assert_eq!(
            corners(&inset.nodes),
            vec![(11, 11), (23, 11), (23, 21), (11, 21), (11, 11)]
        );
        // Inside the walls, as a flood fill gives it.
        assert_eq!(inset.fill.len(), 11 * 9);
        assert_eq!(inset.strip.len(), 15 * 13 - 13 * 11);
    }

    // Nine blocks wide against its twin, a half stepped on three sides would
    // be eight, too narrow for a house: it keeps its width and steps its
    // front and back, as its twin of ten does, so the pair keeps one front.
    #[test]
    fn a_half_too_narrow_to_step_all_round_keeps_its_width() {
        let way = rect_way(1, 10, 10, 18, 22, &[("building", "house")]);
        let fill = fill_of(&way.nodes);
        let mut footprints = bitmap_with_rect(&bbox(), 10, 10, 18, 22);
        for x in 18..=27 {
            for z in 10..=22 {
                footprints.set(x, z);
            }
        }
        let inset = inset_outline(&way.nodes, &fill, &footprints).unwrap();
        assert_eq!(
            corners(&inset.nodes),
            vec![(10, 11), (18, 11), (18, 21), (10, 21), (10, 11)]
        );
        // Standing free, it is too small to step at all.
        let alone = bitmap_with_rect(&bbox(), 10, 10, 18, 22);
        assert!(inset_outline(&way.nodes, &fill, &alone).is_none());
    }

    #[test]
    fn a_party_wall_stays_on_the_shared_line() {
        // A terraced house with its neighbour east of it, sharing x = 22.
        let way = rect_way(1, 10, 10, 22, 22, &[("building", "house")]);
        let fill = fill_of(&way.nodes);
        let mut footprints = bitmap_with_rect(&bbox(), 10, 10, 22, 22);
        for x in 22..=32 {
            for z in 10..=22 {
                footprints.set(x, z);
            }
        }
        let inset = inset_outline(&way.nodes, &fill, &footprints).unwrap();
        assert_eq!(
            corners(&inset.nodes),
            vec![(11, 11), (22, 11), (22, 21), (11, 21), (11, 11)]
        );
    }

    // As in generation, the footprints hold only the inside of each outline.
    // A row house a cell from its neighbour keeps that side on the line, so
    // the eaves close the gap; two cells off, it steps in.
    #[test]
    fn a_wall_a_cell_from_a_neighbour_stays_on_the_line() {
        let way = rect_way(1, 10, 10, 22, 22, &[("building", "house")]);
        let fill = fill_of(&way.nodes);
        let with_neighbour_at = |x0: i32| {
            let neighbour = rect_way(2, x0, 10, x0 + 12, 22, &[("building", "house")]);
            let mut footprints = CoordinateBitmap::new(&bbox());
            for (x, z) in fill.iter().copied().chain(fill_of(&neighbour.nodes)) {
                footprints.set(x, z);
            }
            footprints
        };
        let inset = inset_outline(&way.nodes, &fill, &with_neighbour_at(24)).unwrap();
        assert_eq!(
            corners(&inset.nodes),
            vec![(11, 11), (22, 11), (22, 21), (11, 21), (11, 11)]
        );
        let inset = inset_outline(&way.nodes, &fill, &with_neighbour_at(25)).unwrap();
        assert_eq!(
            corners(&inset.nodes),
            vec![(11, 11), (21, 11), (21, 21), (11, 21), (11, 11)]
        );
    }

    // A house too narrow to step in on both sides keeps all its walls:
    // keeping the side a cell from a neighbour on the line never lets the
    // other step in alone.
    #[test]
    fn keeping_a_wall_on_the_line_steps_no_other_in() {
        let way = rect_way(1, 10, 10, 19, 23, &[("building", "house")]);
        let fill = fill_of(&way.nodes);
        let neighbour = rect_way(2, 21, 10, 31, 23, &[("building", "house")]);
        let mut footprints = CoordinateBitmap::new(&bbox());
        for (x, z) in fill.iter().copied().chain(fill_of(&neighbour.nodes)) {
            footprints.set(x, z);
        }
        assert!(inset_outline(&way.nodes, &fill, &footprints).is_none());
    }

    #[test]
    fn a_wall_partly_beside_a_neighbour_stays_on_the_line() {
        // A garage against the north end of the house's east side only.
        let way = rect_way(1, 10, 10, 22, 22, &[("building", "house")]);
        let fill = fill_of(&way.nodes);
        let mut footprints = bitmap_with_rect(&bbox(), 10, 10, 22, 22);
        for x in 23..=27 {
            for z in 10..=12 {
                footprints.set(x, z);
            }
        }
        let inset = inset_outline(&way.nodes, &fill, &footprints).unwrap();
        assert_eq!(
            corners(&inset.nodes),
            vec![(11, 11), (22, 11), (22, 21), (11, 21), (11, 11)]
        );
    }

    #[test]
    fn a_wing_too_narrow_for_a_room_keeps_its_walls() {
        // An L whose wing is three, then four, blocks wide: stepped in, its
        // two side walls would meet or stand back to back.
        for wing in [12, 13] {
            let nodes: Vec<ProcessedNode> = [
                (10, 10),
                (30, 10),
                (30, 20),
                (wing, 20),
                (wing, 30),
                (10, 30),
                (10, 10),
            ]
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: i as u64,
                tags: Default::default(),
                x,
                z,
            })
            .collect();
            let fill = fill_of(&nodes);
            let mut footprints = bitmap_with_rect(&bbox(), 10, 10, 30, 20);
            for &(x, z) in &fill {
                footprints.set(x, z);
            }
            assert!(
                inset_outline(&nodes, &fill, &footprints).is_none(),
                "{wing}"
            );
        }
    }

    #[test]
    fn small_and_enclosed_buildings_keep_their_walls() {
        let shed = rect_way(1, 10, 10, 14, 20, &[("building", "shed")]);
        let fill = fill_of(&shed.nodes);
        let footprints = bitmap_with_rect(&bbox(), 10, 10, 14, 20);
        assert!(inset_outline(&shed.nodes, &fill, &footprints).is_none());

        let mid = rect_way(2, 10, 10, 20, 18, &[("building", "house")]);
        let fill = fill_of(&mid.nodes);
        let footprints = bitmap_with_rect(&bbox(), 0, 0, 40, 40);
        assert!(inset_outline(&mid.nodes, &fill, &footprints).is_none());
    }

    #[test]
    fn a_small_detached_house_keeps_its_walls() {
        // 11 by 9 would stand 9 by 7, 15 by 10 would be 8 deep: both keep
        // their walls, and a turned 9 m house too, wider on the grid.
        for (id, x1, z1) in [(1, 20, 18), (2, 24, 19)] {
            let way = rect_way(id, 10, 10, x1, z1, &[("building", "house")]);
            let fill = fill_of(&way.nodes);
            let footprints = bitmap_with_rect(&bbox(), 10, 10, x1, z1);
            assert!(
                inset_outline(&way.nodes, &fill, &footprints).is_none(),
                "{id}"
            );
        }
        let nodes: Vec<ProcessedNode> = [(10, 10), (18, 10), (19, 17), (11, 18), (10, 10)]
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: i as u64,
                tags: Default::default(),
                x,
                z,
            })
            .collect();
        let fill = fill_of(&nodes);
        let mut footprints = bitmap_with_rect(&bbox(), 0, 0, 0, 0);
        for &(x, z) in &fill {
            footprints.set(x, z);
        }
        assert!(inset_outline(&nodes, &fill, &footprints).is_none());
    }

    #[test]
    fn a_narrow_terraced_house_steps_in_front_and_back_only() {
        let way = rect_way(1, 10, 10, 15, 22, &[("building", "terrace")]);
        let fill = fill_of(&way.nodes);
        let footprints = bitmap_with_rect(&bbox(), 0, 10, 40, 22);
        let inset = inset_outline(&way.nodes, &fill, &footprints).unwrap();
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
