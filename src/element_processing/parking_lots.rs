//! Surface parking lots: the asphalt, rows of stalls, lamps and parked cars. Rows run along
//! the world axis nearer to the parking aisles mapped in the lot, or to the long side of the
//! smallest rectangle around the lot where none are. A lot turned further from the axes is
//! laid out as lots always were, in rows along x on the world's grid.

use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::clipping::is_invented_node_id;
use crate::element_processing::surfaces::{get_blocks_for_surface, semirandom_surface};
use crate::floodfill_cache::{in_hole, FloodFillCache, Holes};
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use fnv::{FnvHashMap, FnvHashSet};
use rayon::prelude::*;
use std::collections::HashMap;

/// Width of a stall, its line included, and its length, which fits the bundled cars.
const STALL_WIDTH: i32 = 4;
const STALL_LENGTH: i32 = 8;
/// The aisle behind each row of stalls.
const AISLE_WIDTH: i32 = 5;
/// How far, in degrees, a lot may be turned from the nearer world axis and still have its
/// rows put on that axis.
const SNAP_DEG: f64 = 15.0;
const ROW_PERIOD: i32 = STALL_LENGTH + AISLE_WIDTH;

/// Tag `orient_parking_lots` leaves on a lot with a parking aisle in it: the direction its
/// rows run in, as a unit vector, and a point on the aisle, "ux uz x z".
const AISLE_TAG: &str = "arnis:parking_aisle";

/// Tag `lay_out_whole_lots` leaves on a lot: the grid its stalls are laid out on, worked out
/// from its whole mapped outline and the aisles in it before the area clips either, as
/// "ux uz ox oz aisle". Every piece of a One World the lot reaches into lays it out alike.
const GRID_TAG: &str = "arnis:stall_grid";

/// What a stall line or the asphalt may be laid over, a footway's asphalt drawn through the
/// lot among them.
const LOT_SURFACES: &[Block] = &[
    BLACK_CONCRETE,
    GRAY_CONCRETE_POWDER,
    CYAN_TERRACOTTA,
    GRAY_CONCRETE,
    POLISHED_ANDESITE,
];

/// The grid stalls are laid out on: rows run along `u`, stalls reach along `v`, and
/// `origin` is where a row of stalls begins.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StallGrid {
    origin: (f64, f64),
    u: (f64, f64),
    v: (f64, f64),
}

impl StallGrid {
    /// Grid coordinates of a cell's centre.
    fn local(&self, x: i32, z: i32) -> (f64, f64) {
        let (dx, dz) = (
            f64::from(x) + 0.5 - self.origin.0,
            f64::from(z) + 0.5 - self.origin.1,
        );
        (dx * self.u.0 + dz * self.u.1, dx * self.v.0 + dz * self.v.1)
    }

    /// The cell at grid coordinates `(a, b)`.
    fn cell(&self, a: f64, b: f64) -> (i32, i32) {
        (
            (self.origin.0 + a * self.u.0 + b * self.v.0 - 0.5).round() as i32,
            (self.origin.1 + a * self.u.1 + b * self.v.1 - 0.5).round() as i32,
        )
    }

    /// Facing of a car parked in a stall, as Minecraft yaw: its length runs along `v`.
    fn yaw(&self) -> f64 {
        (-self.v.0).atan2(self.v.1).to_degrees()
    }

    /// The world axis nearer to `u`, if `u` is within `SNAP_DEG` of it. Stall lines drawn
    /// at any other angle, even 45 degrees, break into steps of one and two blocks.
    fn axis(u: (f64, f64)) -> Option<(f64, f64)> {
        let a = if u.0.abs() >= u.1.abs() {
            (u.0.signum(), 0.0)
        } else {
            (0.0, u.1.signum())
        };
        let cos = (u.0 * a.0 + u.1 * a.1) / u.0.hypot(u.1);
        (cos >= SNAP_DEG.to_radians().cos()).then_some(a)
    }

    /// Rows along x on the world's grid, which a lot turned too far for its own axis takes.
    fn world() -> Self {
        Self::along((1.0, 0.0), (0.0, 0.0))
    }

    /// Rows along `u`, with a row of stalls starting at `origin`.
    fn along(u: (f64, f64), origin: (f64, f64)) -> Self {
        let len = u.0.hypot(u.1);
        let u = (u.0 / len, u.1 / len);
        Self {
            origin,
            u,
            v: (-u.1, u.0),
        }
    }

    /// The same rows with the stalls along them shifted onto the world's own grid: a stall
    /// side wherever the distance along `u` from the world's origin is a whole number of
    /// stalls. Where a lot is laid out from a different share of its outline, as by two pieces
    /// of a One World each clipping it at its own edge, the stall lines still meet.
    fn on_world_grid(self) -> Self {
        let a = self.origin.0 * self.u.0 + self.origin.1 * self.u.1;
        let shift = a.rem_euclid(f64::from(STALL_WIDTH));
        Self {
            origin: (
                self.origin.0 - shift * self.u.0,
                self.origin.1 - shift * self.u.1,
            ),
            ..self
        }
    }

    /// The grid as `GRID_TAG` records it, with whether it runs along a mapped aisle.
    fn encode(&self, on_aisle: bool) -> String {
        format!(
            "{} {} {} {} {}",
            self.u.0,
            self.u.1,
            self.origin.0,
            self.origin.1,
            u8::from(on_aisle)
        )
    }

    fn decode(tag: &str) -> Option<(Self, bool)> {
        let v: Vec<f64> = tag
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        match v[..] {
            [ux, uz, ox, oz, aisle] => Some((Self::along((ux, uz), (ox, oz)), aisle != 0.0)),
            _ => None,
        }
    }

    /// Rows along an aisle through `point`: the aisle's middle falls on the mapped line.
    fn from_aisle(u: (f64, f64), point: (f64, f64)) -> Self {
        let grid = Self::along(u, point);
        let back = f64::from(STALL_LENGTH) + f64::from(AISLE_WIDTH) / 2.0;
        Self {
            origin: (point.0 - grid.v.0 * back, point.1 - grid.v.1 * back),
            ..grid
        }
    }
}

/// Convex hull of points, counter-clockwise, by the monotone chain.
fn convex_hull(mut points: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    points.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let chain = |points: &mut dyn Iterator<Item = (f64, f64)>| {
        let mut half: Vec<(f64, f64)> = Vec::new();
        for p in points {
            while half.len() >= 2 && cross(half[half.len() - 2], half[half.len() - 1], p) <= 0.0 {
                half.pop();
            }
            half.push(p);
        }
        half.pop();
        half
    };
    let mut hull = chain(&mut points.iter().copied());
    hull.extend(chain(&mut points.iter().rev().copied()));
    hull
}

/// Rows along the world axis nearer to the long side of the smallest rectangle around the
/// outline, from where the lot begins, or the world's rows where that side is turned further
/// than `SNAP_DEG` from it. A lot cut by the edge of the area is measured as drawn, but
/// does not turn with that edge: a side the clip ran along it, between two points it added,
/// gives no direction. Turned by its mapped corners alone, a lot mostly outside the area
/// took whatever angle the few corners left inside gave, and its stalls ran across the
/// street it lines.
fn rectangle_grid(nodes: &[ProcessedNode]) -> Option<StallGrid> {
    // Cell centres.
    let xz = |n: &ProcessedNode| (f64::from(n.x) + 0.5, f64::from(n.z) + 0.5);
    let hull = convex_hull(nodes.iter().map(xz).collect());
    if hull.len() < 3 {
        return None;
    }
    let mapped: FnvHashSet<(i32, i32)> = nodes
        .iter()
        .filter(|n| !is_invented_node_id(n.id))
        .map(|n| (n.x, n.z))
        .collect();
    let is_mapped = |p: (f64, f64)| mapped.contains(&(p.0.floor() as i32, p.1.floor() as i32));
    let span = |d: (f64, f64)| {
        hull.iter()
            .map(|p| p.0 * d.0 + p.1 * d.1)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), s| {
                (lo.min(s), hi.max(s))
            })
    };
    // (area, long side's direction); the smallest rectangle has a side on the hull. The
    // sides along the area's edge count only where the hull has no other.
    let mut best: Option<(f64, (f64, f64))> = None;
    for along_cut in [false, true] {
        for i in 0..hull.len() {
            let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
            let len = (b.0 - a.0).hypot(b.1 - a.1);
            if len < 1e-6 || (!along_cut && !is_mapped(a) && !is_mapped(b)) {
                continue;
            }
            let e = ((b.0 - a.0) / len, (b.1 - a.1) / len);
            let n = (-e.1, e.0);
            let ((e0, e1), (n0, n1)) = (span(e), span(n));
            let area = (e1 - e0) * (n1 - n0);
            if best.is_none_or(|(a, _)| area < a - 1e-6) {
                best = Some((area, if e1 - e0 >= n1 - n0 { e } else { n }));
            }
        }
        if best.is_some() {
            break;
        }
    }
    let Some(u) = StallGrid::axis(best?.1) else {
        return Some(StallGrid::world());
    };
    let grid = StallGrid::along(u, (0.0, 0.0));
    let (a0, b0) = (span(grid.u).0, span(grid.v).0);
    Some(StallGrid::along(
        u,
        (grid.u.0 * a0 + grid.v.0 * b0, grid.u.1 * a0 + grid.v.1 * b0),
    ))
}

/// The grid an `AISLE_TAG` lays a lot out on: along the aisle where it runs near an axis,
/// else the world's rows.
fn aisle_grid(tag: &str) -> Option<StallGrid> {
    let v: Vec<f64> = tag
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    match v[..] {
        [ux, uz, x, z] => Some(
            StallGrid::axis((ux, uz))
                .map_or_else(StallGrid::world, |u| StallGrid::from_aisle(u, (x, z))),
        ),
        _ => None,
    }
}

/// The grid a lot's stalls are laid out on.
#[cfg(test)]
fn stall_grid(way: &ProcessedWay) -> Option<StallGrid> {
    stall_layout(way).map(|(grid, _)| grid)
}

/// The grid a lot's stalls are laid out on, and whether it runs along a mapped aisle: the
/// one worked out from the whole lot where the parser could, else from what the area left
/// of it.
fn stall_layout(way: &ProcessedWay) -> Option<(StallGrid, bool)> {
    if let Some(layout) = way.tags.get(GRID_TAG).and_then(|t| StallGrid::decode(t)) {
        return Some(layout);
    }
    match way.tags.get(AISLE_TAG).and_then(|t| aisle_grid(t)) {
        Some(grid) => Some((grid.on_world_grid(), grid != StallGrid::world())),
        None => rectangle_grid(&way.nodes).map(|grid| (grid.on_world_grid(), false)),
    }
}

/// Lays out every surface lot from its whole mapped outline, `lot_ways` for the lots mapped
/// as ways and the unclipped member ways `unclipped` for multipolygons, along the parking
/// aisles mapped in it, `aisles` as mapped, before the area clips any of them: two pieces of
/// a One World that each hold part of a lot draw its rows alike, meeting at the seam. The
/// grid goes in `GRID_TAG`.
pub(crate) fn lay_out_whole_lots(
    elements: &mut [ProcessedElement],
    lot_ways: &HashMap<u64, Vec<ProcessedNode>>,
    aisles: Vec<ProcessedWay>,
    unclipped: &HashMap<u64, std::sync::Arc<ProcessedWay>>,
) {
    let aisles: Vec<ProcessedElement> = aisles.into_iter().map(ProcessedElement::Way).collect();
    let index = AisleIndex::build(&aisles);
    for element in elements.iter_mut() {
        let rings: Vec<&[ProcessedNode]> = match &*element {
            ProcessedElement::Way(w) => match lot_ways.get(&w.id) {
                Some(nodes) => vec![&nodes[..]],
                None => continue,
            },
            ProcessedElement::Relation(r) if is_surface_lot(&r.tags) => r
                .members
                .iter()
                .filter(|m| m.role == ProcessedMemberRole::Outer)
                .map(|m| {
                    unclipped
                        .get(&m.way.id)
                        .filter(|w| is_closed(&w.nodes))
                        .map_or(&m.way.nodes[..], |w| &w.nodes[..])
                })
                .filter(|nodes| is_closed(nodes))
                .collect(),
            _ => continue,
        };
        if rings.is_empty() {
            continue;
        }
        let layout = match index.aisle_in(&rings).and_then(|t| aisle_grid(&t)) {
            Some(grid) => Some((grid.on_world_grid(), grid != StallGrid::world())),
            None => {
                let nodes: Vec<ProcessedNode> =
                    rings.iter().flat_map(|r| r.iter().cloned()).collect();
                rectangle_grid(&nodes).map(|grid| (grid.on_world_grid(), false))
            }
        };
        let Some((grid, on_aisle)) = layout else {
            continue;
        };
        let tag = grid.encode(on_aisle);
        match element {
            ProcessedElement::Way(w) => w.tags.insert(GRID_TAG.to_string(), tag),
            ProcessedElement::Relation(r) => r.tags.insert(GRID_TAG.to_string(), tag),
            ProcessedElement::Node(_) => None,
        };
    }
}

/// Whether (x, z) lies inside a closed ring, by crossings.
fn ring_contains(ring: &[ProcessedNode], x: f64, z: f64) -> bool {
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (xi, zi) = (f64::from(ring[i].x), f64::from(ring[i].z));
        let (xj, zj) = (f64::from(ring[j].x), f64::from(ring[j].z));
        if (zi > z) != (zj > z) && x < (xj - xi) * (z - zi) / (zj - zi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// A straight stretch of parking aisle.
struct Stretch {
    len: f64,
    /// Unit direction.
    u: (f64, f64),
    mid: (f64, f64),
    /// Both ends are mapped nodes, not points the clipper added.
    mapped: bool,
}

/// Side of the cells aisle segments are bucketed by, in blocks.
const AISLE_CELL: i32 = 64;

/// The parking aisles' segments, bucketed by the cells their bounds touch.
struct AisleIndex<'a> {
    segments: Vec<(&'a ProcessedNode, &'a ProcessedNode)>,
    cells: FnvHashMap<(i32, i32), Vec<u32>>,
}

impl<'a> AisleIndex<'a> {
    fn build(elements: &'a [ProcessedElement]) -> Self {
        let mut index = Self {
            segments: Vec::new(),
            cells: FnvHashMap::default(),
        };
        for element in elements {
            let ProcessedElement::Way(w) = element else {
                continue;
            };
            if w.tags.get("service").map(String::as_str) != Some("parking_aisle")
                || !w.tags.contains_key("highway")
            {
                continue;
            }
            for pair in w.nodes.windows(2) {
                let i = index.segments.len() as u32;
                index.segments.push((&pair[0], &pair[1]));
                let (x0, x1) = (pair[0].x.min(pair[1].x), pair[0].x.max(pair[1].x));
                let (z0, z1) = (pair[0].z.min(pair[1].z), pair[0].z.max(pair[1].z));
                for cx in x0.div_euclid(AISLE_CELL)..=x1.div_euclid(AISLE_CELL) {
                    for cz in z0.div_euclid(AISLE_CELL)..=z1.div_euclid(AISLE_CELL) {
                        index.cells.entry((cx, cz)).or_default().push(i);
                    }
                }
            }
        }
        index
    }

    /// Segments whose cells overlap the bounds, each once.
    fn near(&self, (x0, z0, x1, z1): (i32, i32, i32, i32)) -> Vec<u32> {
        let mut found: Vec<u32> = Vec::new();
        for cx in x0.div_euclid(AISLE_CELL)..=x1.div_euclid(AISLE_CELL) {
            for cz in z0.div_euclid(AISLE_CELL)..=z1.div_euclid(AISLE_CELL) {
                if let Some(list) = self.cells.get(&(cx, cz)) {
                    found.extend_from_slice(list);
                }
            }
        }
        found.sort_unstable();
        found.dedup();
        found
    }

    /// The direction most of the aisle inside the rings runs in, within 10 degrees, and the
    /// longest stretch that way to put an aisle on, as `AISLE_TAG` records them. A stretch
    /// between two mapped nodes wins over one the area's edge cut, so the aisle's line does not
    /// tilt with the rounding of a clipped end.
    fn aisle_in(&self, rings: &[&[ProcessedNode]]) -> Option<String> {
        let bounds = rings.iter().flat_map(|r| r.iter()).fold(
            (i32::MAX, i32::MAX, i32::MIN, i32::MIN),
            |(a, b, c, d), n| (a.min(n.x), b.min(n.z), c.max(n.x), d.max(n.z)),
        );
        let (x0, z0, x1, z1) = bounds;
        let in_box = |n: &ProcessedNode| n.x >= x0 && n.x <= x1 && n.z >= z0 && n.z <= z1;
        let mut stretches: Vec<Stretch> = Vec::new();
        for i in self.near(bounds) {
            let (a, b) = self.segments[i as usize];
            let mid = (f64::from(a.x + b.x) / 2.0, f64::from(a.z + b.z) / 2.0);
            let d = (f64::from(b.x - a.x), f64::from(b.z - a.z));
            let len = d.0.hypot(d.1);
            if len >= 1.0
                && (in_box(a) || in_box(b))
                && rings.iter().any(|r| ring_contains(r, mid.0, mid.1))
            {
                stretches.push(Stretch {
                    len,
                    u: (d.0 / len, d.1 / len),
                    mid,
                    mapped: !is_invented_node_id(a.id) && !is_invented_node_id(b.id),
                });
            }
        }
        let alike = 10f64.to_radians().cos();
        let support = |u: (f64, f64)| -> f64 {
            stretches
                .iter()
                .filter(|s| (s.u.0 * u.0 + s.u.1 * u.1).abs() >= alike)
                .map(|s| s.len)
                .sum()
        };
        let (_, best) = stretches
            .iter()
            .map(|s| ((support(s.u), s.mapped, s.len), s))
            .max_by(|a, b| {
                let ((sa, ma, la), (sb, mb, lb)) = (a.0, b.0);
                sa.total_cmp(&sb).then(ma.cmp(&mb)).then(la.total_cmp(&lb))
            })?;
        Some(format!(
            "{:.5} {:.5} {:.1} {:.1}",
            best.u.0,
            best.u.1,
            best.mid.0 + 0.5,
            best.mid.1 + 0.5
        ))
    }
}

fn is_closed(nodes: &[ProcessedNode]) -> bool {
    nodes.len() >= 4
        && nodes[0].x == nodes[nodes.len() - 1].x
        && nodes[0].z == nodes[nodes.len() - 1].z
}

/// Turns each parking lot to the parking aisles mapped inside it: the direction most of their
/// length runs in, recorded on the lot for `generate_parking_lot` with a point on the longest
/// aisle that way. Returns the number of lots turned.
pub fn orient_parking_lots(elements: &mut [ProcessedElement]) -> usize {
    let aisles = AisleIndex::build(elements);
    if aisles.segments.is_empty() {
        return 0;
    }
    // The outer rings of every surface lot, by element.
    let lots: Vec<(usize, Vec<&[ProcessedNode]>)> = elements
        .iter()
        .enumerate()
        .filter_map(|(i, element)| {
            let rings: Vec<&[ProcessedNode]> = match element {
                ProcessedElement::Way(w) if is_surface_lot(&w.tags) && is_closed(&w.nodes) => {
                    vec![&w.nodes[..]]
                }
                ProcessedElement::Relation(r) if is_surface_lot(&r.tags) => r
                    .members
                    .iter()
                    .filter(|m| m.role == ProcessedMemberRole::Outer && is_closed(&m.way.nodes))
                    .map(|m| &m.way.nodes[..])
                    .collect(),
                _ => Vec::new(),
            };
            (!rings.is_empty()).then_some((i, rings))
        })
        .collect();
    let turns: Vec<(usize, String)> = lots
        .par_iter()
        .filter_map(|(i, rings)| aisles.aisle_in(rings).map(|tag| (*i, tag)))
        .collect();
    for (i, tag) in &turns {
        match &mut elements[*i] {
            ProcessedElement::Way(w) => w.tags.insert(AISLE_TAG.to_string(), tag.clone()),
            ProcessedElement::Relation(r) => r.tags.insert(AISLE_TAG.to_string(), tag.clone()),
            ProcessedElement::Node(_) => None,
        };
    }
    turns.len()
}

/// A parking lot laid out on the ground, as opposed to a garage, a deck on a roof or one
/// under the ground, whose `amenity=parking` area must not pave the ground it is mapped over.
pub(crate) fn is_surface_lot(tags: &HashMap<String, String>) -> bool {
    let get = |k: &str| tags.get(k).map(String::as_str);
    let below = ["layer", "level"].iter().any(|k| {
        get(k)
            .and_then(|v| v.parse::<i32>().ok())
            .is_some_and(|l| l < 0)
    });
    get("amenity") == Some("parking")
        && !tags.contains_key("building")
        && !matches!(
            get("parking"),
            Some("underground" | "multi-storey" | "rooftop")
        )
        && !matches!(get("location"), Some("underground" | "roof" | "rooftop"))
        && !below
}

/// A parking lot: its asphalt, rows of stalls with a lamp every few, and parked cars.
/// `holes` are the inner rings of a multipolygon lot.
pub fn generate_parking_lot(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    holes: Option<&Holes>,
) {
    // Speckled asphalt mix like roads; honor an explicit surface=* tag.
    let block_types: &[Block] = way
        .tags
        .get("surface")
        .and_then(|s| get_blocks_for_surface(s))
        .unwrap_or(&[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA]);

    // The outline, in the lot's own surface.
    for pair in way.nodes.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        for (bx, _, bz) in bresenham_line(a.x, 0, a.z, b.x, 0, b.z) {
            if in_hole(holes, bx, bz) {
                continue;
            }
            editor.set_block(
                semirandom_surface(bx, bz, block_types),
                bx,
                0,
                bz,
                Some(&[BLACK_CONCRETE, GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA]),
                None,
            );
        }
    }

    let flood_area = flood_fill_cache.get_or_compute(way, args.timeout.as_ref());
    let mut lot: Vec<(i32, i32)> = flood_area
        .iter()
        .copied()
        .filter(|&(x, z)| !editor.nested_area_owns(x, z) && !in_hole(holes, x, z))
        .collect();
    for &(x, z) in &lot {
        editor.set_block(
            semirandom_surface(x, z, block_types),
            x,
            0,
            z,
            Some(LOT_SURFACES),
            None,
        );
    }
    // A lot too thin to lay out stays bare asphalt.
    // Laid out along mapped aisles, whose lanes run down the grid's aisles.
    let Some((grid, on_aisles)) = stall_layout(way) else {
        return;
    };
    let world = grid == StallGrid::world();

    for &(x, z) in &lot {
        // Stall sides on the left of each stall, a line across its front, and the stall
        // ends at the aisle; the aisles keep the bare asphalt, and so do the lanes of a road
        // the lot is mapped around. A mapped aisle's lanes reach to the stall ends' line.
        // The world's rows, which no lane runs along, are painted whole, as they always were.
        let (a, b) = grid.local(x, z);
        let across = (a.floor() as i32).rem_euclid(STALL_WIDTH);
        let along = (b.floor() as i32).rem_euclid(ROW_PERIOD);
        let line = if along < STALL_LENGTH {
            across == 0 || along == 0
        } else {
            along == STALL_LENGTH
        };
        let lanes = !world && editor.on_carriageway(x, z) && !(on_aisles && along == STALL_LENGTH);
        if line && !lanes {
            editor.set_block(WHITE_CONCRETE, x, 0, z, Some(LOT_SURFACES), None);
        }
    }

    // Lamps and cars, by the stall corners and middles the grid puts inside the lot, and
    // off the lanes of any road through it.
    lot.sort_unstable();
    let in_lot = |x: i32, z: i32| lot.binary_search(&(x, z)).is_ok();
    if lot.is_empty() {
        return;
    }
    let (lo, hi) = lot.iter().fold(
        (
            (f64::INFINITY, f64::INFINITY),
            (f64::NEG_INFINITY, f64::NEG_INFINITY),
        ),
        |(lo, hi), &(x, z)| {
            let (a, b) = grid.local(x, z);
            ((lo.0.min(a), lo.1.min(b)), (hi.0.max(a), hi.1.max(b)))
        },
    );
    let (w, p) = (f64::from(STALL_WIDTH), f64::from(ROW_PERIOD));
    let yaw = grid.yaw();
    for i in (lo.0 / w).floor() as i32..=(hi.0 / w).ceil() as i32 {
        for j in (lo.1 / p).floor() as i32..=(hi.1 / p).ceil() as i32 {
            let (a, b) = (f64::from(i) * w, f64::from(j) * p);
            // A slim metal lamp with a cool-white head, at every third stall of every other row.
            let corner = grid.cell(a + 0.5, b + 0.5);
            if i.rem_euclid(3) == 0
                && j.rem_euclid(2) == 0
                && in_lot(corner.0, corner.1)
                && !editor.on_carriageway(corner.0, corner.1)
            {
                let (x, z) = corner;
                editor.set_block(SMOOTH_STONE, x, 1, z, None, None);
                editor.set_block(ANDESITE_WALL, x, 2, z, None, None);
                for dy in 3..=5 {
                    editor.set_block(IRON_BARS, x, dy, z, None, None);
                }
                editor.set_block(SEA_LANTERN, x, 6, z, None, None);
                editor.set_block(SMOOTH_STONE_SLAB, x, 7, z, None, None);
            }
            // Park cars on some stalls wholly inside the lot, leaving plenty empty.
            let inside = (0..=STALL_WIDTH).all(|da| {
                (0..=STALL_LENGTH).all(|db| {
                    let (x, z) = grid.cell(a + f64::from(da), b + f64::from(db));
                    in_lot(x, z)
                        && (da == 0
                            || da == STALL_WIDTH
                            || db == 0
                            || db == STALL_LENGTH
                            || !editor.on_carriageway(x, z))
                })
            });
            if inside {
                let (x, z) = grid.cell(
                    a + f64::from(STALL_WIDTH) / 2.0,
                    b + f64::from(STALL_LENGTH) / 2.0,
                );
                crate::structures::car::maybe_park_car_facing(editor, x, z, yaw);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u64, x: i32, z: i32) -> ProcessedNode {
        ProcessedNode {
            id,
            tags: HashMap::new(),
            x,
            z,
        }
    }

    fn ring(points: &[(i32, i32)]) -> Vec<ProcessedNode> {
        let mut nodes: Vec<ProcessedNode> = points
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| node(i as u64 + 1, x, z))
            .collect();
        nodes.push(nodes[0].clone());
        nodes
    }

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    #[test]
    fn only_lots_on_the_ground_are_surface_lots() {
        let lot = |pairs: &[(&str, &str)]| {
            let mut tags: HashMap<String, String> = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            tags.insert("amenity".into(), "parking".into());
            is_surface_lot(&tags)
        };
        assert!(lot(&[]));
        assert!(lot(&[("parking", "surface")]));
        assert!(!lot(&[("parking", "underground")]));
        assert!(!lot(&[("parking", "multi-storey")]));
        assert!(!lot(&[("parking", "rooftop")]));
        assert!(!lot(&[("layer", "-1")]));
        assert!(!lot(&[("level", "-2")]));
        assert!(!lot(&[("building", "parking")]));
    }

    #[test]
    fn rows_run_along_the_long_side() {
        // Long north to south.
        let grid = rectangle_grid(&ring(&[(0, 0), (20, 0), (20, 60), (0, 60)])).unwrap();
        assert!(close((grid.u.0.abs(), grid.u.1.abs()), (0.0, 1.0)));
        // Long east to west.
        let grid = rectangle_grid(&ring(&[(0, 0), (60, 0), (60, 20), (0, 20)])).unwrap();
        assert!(close((grid.u.0.abs(), grid.u.1.abs()), (1.0, 0.0)));
    }

    #[test]
    fn a_turned_lot_keeps_its_rows_on_the_world_axes() {
        // A 40 x 20 lot turned 10 degrees, then 80: rows along the nearer axis, where the
        // stall lines stay straight.
        for (deg, along) in [(10f64, (1.0, 0.0)), (80.0, (0.0, 1.0))] {
            let (c, s) = (deg.to_radians().cos(), deg.to_radians().sin());
            let at = |a: f64, b: f64| {
                (
                    (a * c - b * s).round() as i32,
                    (a * s + b * c).round() as i32,
                )
            };
            let lot = ring(&[at(0.0, 0.0), at(40.0, 0.0), at(40.0, 20.0), at(0.0, 20.0)]);
            let grid = rectangle_grid(&lot).unwrap();
            assert!(close((grid.u.0.abs(), grid.u.1.abs()), along), "{deg}");
            // Every corner of the lot is at or past the grid's start.
            for n in &lot {
                let (a, b) = grid.local(n.x, n.z);
                assert!(a > -1.0 && b > -1.0, "{a} {b}");
            }
        }
        // Turned 30 or 60 degrees, it is laid out on the world's rows, as lots always were.
        for deg in [30f64, 45.0, 60.0] {
            let (c, s) = (deg.to_radians().cos(), deg.to_radians().sin());
            let at = |a: f64, b: f64| {
                (
                    (a * c - b * s).round() as i32,
                    (a * s + b * c).round() as i32,
                )
            };
            let lot = ring(&[at(0.0, 0.0), at(40.0, 0.0), at(40.0, 20.0), at(0.0, 20.0)]);
            assert_eq!(rectangle_grid(&lot), Some(StallGrid::world()), "{deg}");
        }
        // A mapped aisle turns the rows no more than the outline does.
        let aisle = |u: (f64, f64)| ProcessedWay {
            id: 1,
            nodes: ring(&[(0, 0), (40, 0), (40, 20), (0, 20)]),
            tags: HashMap::from([(AISLE_TAG.to_string(), format!("{} {} 10.5 20.5", u.0, u.1))]),
        };
        assert!(close(
            stall_grid(&aisle((0.99, 0.14))).unwrap().u,
            (1.0, 0.0)
        ));
        assert_eq!(stall_grid(&aisle((0.9, 0.44))), Some(StallGrid::world()));
    }

    #[test]
    fn a_lot_cut_down_to_a_corner_still_gets_a_grid() {
        // The area's edge cuts the lot: two mapped corners (the first doubled as the closing
        // node) and the points the clip added.
        let mut nodes = vec![
            node(1, 0, 0),
            node(2, 40, 0),
            node((1 << 62) + 1, 40, 20),
            node((1 << 62) + 2, 0, 20),
        ];
        nodes.push(nodes[0].clone());
        let grid = rectangle_grid(&nodes).expect("a grid from the clipped outline");
        assert!(close((grid.u.0.abs(), grid.u.1.abs()), (1.0, 0.0)));
        // Mapped in a line, it spans no area at all.
        assert!(rectangle_grid(&[node(1, 0, 0), node(2, 9, 0), node(1, 0, 0)]).is_none());
    }

    #[test]
    fn a_lot_cut_by_the_area_edge_keeps_its_rows_along_the_street() {
        // Street-side parking along both kerbs of a street running west to east, cut at
        // x = 0 by the edge of the area (Erding, Marienbader Straße). The eight mapped
        // corners left inside alone made a rectangle turned 23 degrees, and the stalls ran
        // across the street.
        let cut = |id: u64, x: i32, z: i32| node((1 << 62) + id, x, z);
        let mut nodes = vec![
            cut(1, 0, 118),
            node(2, 108, 114),
            node(3, 105, 119),
            node(4, 100, 120),
            node(5, 100, 136),
            node(6, 73, 137),
            node(7, 73, 147),
            node(8, 55, 148),
            node(9, 54, 137),
            cut(10, 0, 138),
        ];
        nodes.push(nodes[0].clone());
        let grid = rectangle_grid(&nodes).unwrap();
        let angle = grid.u.1.atan2(grid.u.0).to_degrees().rem_euclid(180.0);
        assert!(angle.min(180.0 - angle) < 3.0, "{angle}");
    }

    #[test]
    fn stall_lines_keep_to_the_world_grid_however_the_lot_is_cut() {
        // The same lot, whole and with its west end cut away by an area's edge.
        let whole = ring(&[(3, 7), (63, 7), (63, 37), (3, 37)]);
        let cut = vec![
            node((1 << 62) + 1, 21, 7),
            node(2, 63, 7),
            node(3, 63, 37),
            node((1 << 62) + 2, 21, 37),
            node((1 << 62) + 1, 21, 7),
        ];
        let lot = |nodes: Vec<ProcessedNode>| ProcessedWay {
            id: 1,
            nodes,
            tags: HashMap::new(),
        };
        let (a, b) = (
            stall_grid(&lot(whole)).unwrap(),
            stall_grid(&lot(cut)).unwrap(),
        );
        // Rows run east to west in both, with the stall sides on the same columns.
        assert!(close(a.u, b.u));
        let side =
            |g: &StallGrid, x: i32| (g.local(x, 20).0.floor() as i32).rem_euclid(STALL_WIDTH);
        for x in 25..60 {
            assert_eq!(side(&a, x), side(&b, x), "x = {x}");
        }
    }

    #[test]
    fn an_aisle_puts_the_aisle_on_its_line() {
        let grid = StallGrid::from_aisle((1.0, 0.0), (10.5, 20.5));
        let (_, b) = grid.local(10, 20);
        assert!((b - (f64::from(STALL_LENGTH) + f64::from(AISLE_WIDTH) / 2.0)).abs() < 1e-9);
        // Cars park across the aisle.
        assert!((grid.yaw().abs() % 180.0).abs() < 1e-9);
    }

    #[test]
    fn a_lot_turns_with_the_aisles_inside_it() {
        let lot = ProcessedWay {
            id: 1,
            nodes: ring(&[(0, 0), (60, 0), (60, 30), (0, 30)]),
            tags: [("amenity", "parking")]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let aisle = |id: u64, a: (i32, i32), b: (i32, i32)| ProcessedWay {
            id,
            nodes: vec![node(id * 10, a.0, a.1), node(id * 10 + 1, b.0, b.1)],
            tags: [("highway", "service"), ("service", "parking_aisle")]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let mut elements = vec![
            ProcessedElement::Way(lot),
            // A short one across the lot, a long one north to south, one outside.
            ProcessedElement::Way(aisle(2, (5, 10), (25, 10))),
            ProcessedElement::Way(aisle(3, (40, 2), (40, 28))),
            ProcessedElement::Way(aisle(4, (100, 0), (100, 90))),
        ];
        assert_eq!(orient_parking_lots(&mut elements), 1);
        let ProcessedElement::Way(lot) = &elements[0] else {
            unreachable!()
        };
        let grid = stall_grid(lot).unwrap();
        assert!(close((grid.u.0.abs(), grid.u.1.abs()), (0.0, 1.0)));
    }

    #[test]
    fn an_aisle_across_several_cells_counts_once() {
        let tags = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let aisle = |id: u64, a: (i32, i32), b: (i32, i32)| {
            ProcessedElement::Way(ProcessedWay {
                id,
                nodes: vec![node(id * 10, a.0, a.1), node(id * 10 + 1, b.0, b.1)],
                tags: tags(&[("highway", "service"), ("service", "parking_aisle")]),
            })
        };
        let mut elements = vec![
            ProcessedElement::Way(ProcessedWay {
                id: 1,
                nodes: ring(&[(50, 0), (150, 0), (150, 40), (50, 40)]),
                tags: tags(&[("amenity", "parking")]),
            }),
            // 90 blocks east to west, over three cells of the index...
            aisle(2, (55, 20), (145, 20)),
            // ...against 108 north to south.
            aisle(3, (70, 2), (70, 38)),
            aisle(4, (100, 2), (100, 38)),
            aisle(5, (130, 2), (130, 38)),
        ];
        assert_eq!(orient_parking_lots(&mut elements), 1);
        let ProcessedElement::Way(lot) = &elements[0] else {
            unreachable!()
        };
        let grid = stall_grid(lot).unwrap();
        assert!(close((grid.u.0.abs(), grid.u.1.abs()), (0.0, 1.0)));
    }

    /// The pieces of a lot a One World cuts at x = 20 or z = 20, either side kept, as the
    /// clipper leaves them: the corners inside and the points it added on the cut.
    fn pieces() -> Vec<Vec<ProcessedNode>> {
        let cut = |id: u64, x: i32, z: i32| node((1 << 62) + id, x, z);
        let closed = |mut nodes: Vec<ProcessedNode>| {
            nodes.push(nodes[0].clone());
            nodes
        };
        vec![
            // West of x = 20, and east of it.
            closed(vec![
                node(1, 0, 0),
                cut(1, 20, 0),
                cut(2, 20, 40),
                node(4, 0, 40),
            ]),
            closed(vec![
                cut(1, 20, 0),
                node(2, 60, 0),
                node(3, 60, 40),
                cut(2, 20, 40),
            ]),
            // North of z = 20, and south of it.
            closed(vec![
                node(1, 0, 0),
                node(2, 60, 0),
                cut(3, 60, 20),
                cut(4, 0, 20),
            ]),
            closed(vec![
                cut(4, 0, 20),
                cut(3, 60, 20),
                node(3, 60, 40),
                node(4, 0, 40),
            ]),
        ]
    }

    #[test]
    fn a_lot_one_world_cuts_lays_out_its_rows_alike_in_every_piece() {
        // A 60 x 40 lot, long east to west.
        let whole = ring(&[(0, 0), (60, 0), (60, 40), (0, 40)]);
        let lot = |nodes: Vec<ProcessedNode>| {
            ProcessedElement::Way(ProcessedWay {
                id: 7,
                nodes,
                tags: HashMap::from([("amenity".to_string(), "parking".to_string())]),
            })
        };
        let lot_ways = HashMap::from([(7u64, whole.clone())]);
        let layout = |nodes: Vec<ProcessedNode>| {
            let mut elements = vec![lot(nodes)];
            lay_out_whole_lots(&mut elements, &lot_ways, Vec::new(), &HashMap::new());
            match &elements[0] {
                ProcessedElement::Way(w) => stall_layout(w).unwrap(),
                _ => unreachable!(),
            }
        };
        let reference = layout(whole.clone());
        assert!(close(reference.0.u, (1.0, 0.0)) || close(reference.0.u, (-1.0, 0.0)));
        for piece in pieces() {
            let got = layout(piece.clone());
            // The same rows, the same way round, from the same start: the stall lines meet.
            assert_eq!(got, reference, "{piece:?}");
        }
        // Drawn from what the cut left, the pieces came out turned and shifted.
        let clipped: Vec<_> = pieces()
            .into_iter()
            .filter_map(|nodes| rectangle_grid(&nodes).map(StallGrid::on_world_grid))
            .collect();
        assert!(clipped.iter().any(|g| *g != reference.0), "{clipped:?}");
    }

    #[test]
    fn a_lot_mapped_as_a_multipolygon_is_laid_out_and_paved_round_its_hole() {
        use crate::element_processing::RelationArea;
        use crate::floodfill_cache::{FloodFillCache, InnerRings};
        use crate::osm_parser::{ProcessedMember, ProcessedRelation};
        use clap::Parser;
        use std::sync::Arc;
        let tags = HashMap::from([
            ("type".to_string(), "multipolygon".to_string()),
            ("amenity".to_string(), "parking".to_string()),
        ]);
        assert_eq!(RelationArea::of(&tags), Some(RelationArea::Parking));
        // The outer ring as mapped, and as the area's edge at z = 30 left it; a kiosk inside.
        let outer = Arc::new(ProcessedWay {
            id: 11,
            nodes: ring(&[(2, 2), (50, 2), (50, 40), (2, 40)]),
            tags: HashMap::new(),
        });
        let clipped = ProcessedWay {
            id: 11,
            nodes: {
                let cut = |id: u64, x: i32, z: i32| node((1 << 62) + id, x, z);
                let mut n = vec![node(1, 2, 2), node(2, 50, 2), cut(1, 50, 30), cut(2, 2, 30)];
                n.push(n[0].clone());
                n
            },
            tags: HashMap::new(),
        };
        let kiosk = ProcessedWay {
            id: 12,
            nodes: ring(&[(20, 10), (26, 10), (26, 16), (20, 16)]),
            tags: HashMap::new(),
        };
        let member = |way: ProcessedWay, role| ProcessedMember {
            role,
            way: Arc::new(way),
        };
        let mut elements = vec![ProcessedElement::Relation(ProcessedRelation {
            id: 3,
            tags,
            members: vec![
                member(clipped, ProcessedMemberRole::Outer),
                member(kiosk, ProcessedMemberRole::Inner),
            ],
        })];
        let unclipped = HashMap::from([(11u64, Arc::clone(&outer))]);
        lay_out_whole_lots(&mut elements, &HashMap::new(), Vec::new(), &unclipped);
        let ProcessedElement::Relation(rel) = &elements[0] else {
            unreachable!()
        };
        // Laid out from the whole outer ring, not the piece the edge left.
        let whole = ProcessedWay {
            id: 11,
            nodes: outer.nodes.clone(),
            tags: HashMap::new(),
        };
        let expected = rectangle_grid(&whole.nodes).map(StallGrid::on_world_grid);
        assert!(rel.tags.contains_key(GRID_TAG));
        // As data_processing draws it: each outer ring with the relation's tags, less its holes.
        let xzbbox =
            crate::coordinate_system::cartesian::XZBBox::rect_from_xz_lengths(64.0, 64.0).unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let cache = FloodFillCache::new();
        let inners = InnerRings::of(rel);
        for m in rel
            .members
            .iter()
            .filter(|m| m.role == ProcessedMemberRole::Outer)
        {
            let holes = cache.holes_in(&m.way, &inners, None);
            let way = ProcessedWay {
                id: m.way.id,
                nodes: m.way.nodes.clone(),
                tags: rel.tags.clone(),
            };
            assert_eq!(stall_grid(&way), expected);
            generate_parking_lot(&mut editor, &way, &args, &cache, holes.as_ref());
        }
        let asphalt: &[Block] = &[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA, WHITE_CONCRETE];
        assert!(editor.check_for_block(10, 0, 20, Some(asphalt)));
        assert!(
            !editor.check_for_block(23, 0, 13, Some(asphalt)),
            "the kiosk's hole"
        );
        let lines = (3..50)
            .flat_map(|x| (3..30).map(move |z| (x, z)))
            .filter(|&(x, z)| editor.check_for_block(x, 0, z, Some(&[WHITE_CONCRETE])))
            .count();
        assert!(lines > 20, "{lines} stall line cells");
    }
}
