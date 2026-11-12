//! People walking the mapped footways, sidewalks, squares, pedestrian streets and paths,
//! run by the extras data pack. A lane each way along every way, more on a wide pedestrian
//! street, lines across squares, the ways on at each junction, where walkers wait at a kerb
//! or a light and how busy each way is are worked out here and written into the pack as
//! storage. Each walker is a mannequin (a villager before 1.21.9) the pack carries from
//! point to point; the storage layout and the scores are the contract with
//! `assets/minecraft/datapack_extras/data/arnis/function/walk`.

use crate::clipping::is_invented_node_id;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::datapack::macro_coord;
use crate::element_processing::cars::{drivable, lane_line};
use crate::element_processing::highways::highway_block_range;
use crate::element_processing::traffic_lights::group_of;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedWay};
use crate::water_probe::WalkProbe;
use fnv::{FnvHashMap, FnvHashSet};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

/// Longest stretch between two points; shorter where the ground bends.
const MAX_SPAN: f64 = 16.0;
/// Most walkers around one player by day and by night, which is what a tick costs.
const DAY_NEAR: usize = 60;
const NIGHT_NEAR: usize = 15;
/// Side of the map cells the spots walkers start from are stored by, in blocks.
const SPOT_CELL: f64 = 32.0;
/// Lanes and spot cells the pack stores per command.
const LANES_PER_LINE: usize = 256;
const CELLS_PER_LINE: usize = 256;
/// Most points the pack holds, about 40 bytes each in its storage.
const MAX_POINTS: usize = 300_000;
/// An unhurried walk, in metres a second, and how much faster or slower people go.
const WALK_MPS: f64 = 1.5;
const PACES: [f64; 7] = [0.75, 0.85, 0.93, 1.0, 1.07, 1.16, 1.3];
/// Room a walker keeps from street furniture it steps round, and the most it steps aside.
const CLEAR: f64 = 0.35;
const DETOUR_MAX: f64 = 1.2;
/// How far from a way the shops, cafés and sights that bring people to it count.
const FOOTFALL_M: f64 = 75.0;
/// How far a walker on a way that meets a road waits back from its middle, past the kerb.
const KERB_BACK: f64 = 1.5;
/// How far a walker may sink into the ground between two points, and float over it,
/// before a point is put in where the ground bends, in blocks.
const SINK: f64 = 0.3;
const FLOAT: f64 = 0.6;
/// How far a walker turning round looks for the lane back, in blocks.
const BESIDE_M: f64 = 6.0;
/// Pieces of footway cut off from everything else that add up to less than this, in
/// metres, get no walkers: a stub in a back yard, a step in front of a door.
const SCRAP_M: f64 = 30.0;

/// People per metre of lane, each way, on a way of this kind; None where nobody walks.
pub(crate) fn walk_density(tags: &HashMap<String, String>) -> Option<f64> {
    let get = |k: &str| tags.get(k).map(String::as_str);
    let highway = get("highway")?;
    let yes = |k: &str| matches!(get(k), Some("yes" | "designated" | "permissive"));
    if off_limits(tags) || get("area") == Some("yes") {
        return None;
    }
    Some(match highway {
        "pedestrian" => 1.0 / 8.0,
        "living_street" => 1.0 / 25.0,
        "footway" => match get("footway") {
            Some("sidewalk" | "crossing") => 1.0 / 30.0,
            _ => 1.0 / 45.0,
        },
        "steps" => 1.0 / 40.0,
        "path" if yes("foot") => 1.0 / 60.0,
        "path" => 1.0 / 120.0,
        "cycleway" if yes("foot") => 1.0 / 150.0,
        "track" => 1.0 / 300.0,
        "bridleway" => 1.0 / 400.0,
        _ => return None,
    })
}

/// Underground, indoor, other levels and closed to people on foot.
fn off_limits(tags: &HashMap<String, String>) -> bool {
    let get = |k: &str| tags.get(k).map(String::as_str);
    let yes = |k: &str| matches!(get(k), Some("yes" | "designated" | "permissive"));
    let closed = |k: &str| matches!(get(k), Some("no" | "private" | "use_sidepath"));
    let bridge = matches!(get("bridge"), Some(v) if v != "no");
    // Underground and indoor ways are not where the ground says they are; a pier a layer
    // up or a way under a roof is found on what was built, like any other way.
    matches!(get("tunnel"), Some(v) if v != "no")
        || get("layer")
            .and_then(|l| l.parse::<i32>().ok())
            .is_some_and(|l| l < 0 && !bridge)
        || get("level").is_some_and(|l| l != "0")
        || get("indoor") == Some("yes")
        || tags.contains_key("conveying")
        || closed("access") && !yes("foot")
        || closed("foot")
}

/// A piece of street furniture as built: its centre, half extents and, for a bench or a
/// billboard set along the way, how much longer it is along the lane.
#[derive(Clone, Copy)]
struct Item {
    x: f64,
    z: f64,
    hx: f64,
    hz: f64,
    long: f64,
}

/// The street furniture Arnis builds from a mapped node, as walkers should pass it.
fn furniture(n: &ProcessedNode) -> Option<Item> {
    let get = |k: &str| n.tags.get(k).map(String::as_str);
    if get("layer").is_some_and(|l| l.starts_with('-'))
        || get("level").is_some_and(|l| l.starts_with('-'))
    {
        return None;
    }
    let (half, long, dx) = match (get("amenity"), get("highway"), get("advertising")) {
        (Some("waste_basket" | "waste_disposal" | "vending_machine" | "atm"), ..) => {
            (0.5, 0.0, 0.0)
        }
        (Some("recycling"), ..) if get("recycling_type") == Some("container") => (0.5, 0.0, 0.0),
        (Some("bench"), ..) => (0.5, 1.0, 0.0),
        (Some("drinking_water"), ..) => (0.3, 0.0, 0.0),
        (_, Some("street_lamp"), _) => (0.5, 0.0, 0.0),
        (.., Some("column")) => (0.5, 0.0, 0.0),
        (.., Some("billboard")) => (0.5, 1.0, 0.0),
        (.., Some("poster_box")) => (0.5, 0.0, 0.5),
        (.., Some("flag")) => (0.3, 0.0, 0.0),
        _ if get("natural") == Some("tree") => (0.5, 0.0, 0.0),
        _ if get("barrier") == Some("bollard") => (0.3, 0.0, 0.0),
        _ if get("power") == Some("pole") => (0.4, 0.0, 0.0),
        _ if get("emergency") == Some("fire_hydrant")
            && !matches!(
                get("fire_hydrant:type"),
                Some("underground" | "wall" | "pond")
            ) =>
        {
            (0.5, 0.0, 0.0)
        }
        _ => return None,
    };
    Some(Item {
        x: n.x as f64 + 0.5 + dx,
        z: n.z as f64 + 0.5,
        hx: half + dx,
        hz: half,
        long,
    })
}

/// Mapped street furniture by map cell, for lanes to step round.
struct Furniture {
    items: Vec<Item>,
    cells: FnvHashMap<(i32, i32), Vec<u32>>,
}

impl Furniture {
    const CELL: f64 = 8.0;

    fn new(items: Vec<Item>) -> Self {
        let mut cells: FnvHashMap<(i32, i32), Vec<u32>> = FnvHashMap::default();
        for (i, o) in items.iter().enumerate() {
            cells
                .entry(Self::cell(o.x, o.z))
                .or_default()
                .push(i as u32);
        }
        Self { items, cells }
    }

    fn cell(x: f64, z: f64) -> (i32, i32) {
        (
            (x / Self::CELL).floor() as i32,
            (z / Self::CELL).floor() as i32,
        )
    }

    /// The lane bent a step aside round each piece it would brush, its ends left where
    /// they are; a piece needing more than `DETOUR_MAX` aside is left to the probe.
    fn go_round(&self, points: Vec<Point>) -> Vec<Point> {
        if self.cells.is_empty() || points.len() < 2 {
            return points;
        }
        let mut at = vec![0.0];
        for w in points.windows(2) {
            at.push(at[at.len() - 1] + (w[1].x - w[0].x).hypot(w[1].z - w[0].z));
        }
        let total = at[at.len() - 1];
        let dir = |i: usize| {
            let (a, b) = (points[i], points[i + 1]);
            let len = (b.x - a.x).hypot(b.z - a.z).max(1e-9);
            ((b.x - a.x) / len, (b.z - a.z) / len)
        };
        let mut near: Vec<u32> = Vec::new();
        for w in points.windows(2) {
            let (lo, hi) = (
                Self::cell(w[0].x.min(w[1].x) - 3.0, w[0].z.min(w[1].z) - 3.0),
                Self::cell(w[0].x.max(w[1].x) + 3.0, w[0].z.max(w[1].z) + 3.0),
            );
            for cx in lo.0..=hi.0 {
                for cz in lo.1..=hi.1 {
                    near.extend(self.cells.get(&(cx, cz)).into_iter().flatten());
                }
            }
        }
        near.sort_unstable();
        near.dedup();
        // (where along, signed step across, half length at full step, ramp)
        let mut bends: Vec<(f64, f64, f64, f64)> = Vec::new();
        for &i in &near {
            let o = self.items[i as usize];
            let mut best: Option<(f64, f64, f64, usize)> = None;
            for k in 0..points.len() - 1 {
                let (a, (ux, uz)) = (points[k], dir(k));
                let len = at[k + 1] - at[k];
                if len < 1e-6 {
                    continue;
                }
                let t = ((o.x - a.x) * ux + (o.z - a.z) * uz).clamp(0.0, len);
                let (px, pz) = (a.x + ux * t, a.z + uz * t);
                let dist = (o.x - px).hypot(o.z - pz);
                if best.is_none_or(|b| dist < b.0) {
                    let left = (o.z - a.z) * ux - (o.x - a.x) * uz;
                    best = Some((dist, at[k] + t, left, k));
                }
            }
            let Some((dist, s, left, k)) = best else {
                continue;
            };
            let (ux, uz) = dir(k);
            let wide = o.hx * uz.abs() + o.hz * ux.abs() + CLEAR;
            // Off the lane's end or round its corner counts as far as it really is.
            if dist >= wide {
                continue;
            }
            let aside = wide - dist + 0.1;
            let plateau = o.hx * ux.abs() + o.hz * uz.abs() + o.long + CLEAR;
            let room = s.min(total - s) - plateau - 0.1;
            if aside > DETOUR_MAX || room < 0.6 {
                continue;
            }
            let ramp = (1.5 * aside).max(1.0).min(room);
            bends.push((s, if left > 0.0 { -aside } else { aside }, plateau, ramp));
        }
        if bends.is_empty() {
            return points;
        }
        let off = |q: f64| {
            bends.iter().fold(0.0f64, |acc, &(s, step, plateau, ramp)| {
                let d = (q - s).abs();
                let v = step * (1.0 - ((d - plateau) / ramp).max(0.0)).max(0.0);
                if v.abs() > acc.abs() {
                    v
                } else {
                    acc
                }
            })
        };
        let mut keys: Vec<(f64, Option<usize>)> =
            at.iter().enumerate().map(|(k, &q)| (q, Some(k))).collect();
        for &(s, _, plateau, ramp) in &bends {
            for q in [
                s - plateau - ramp,
                s - plateau,
                s + plateau,
                s + plateau + ramp,
            ] {
                keys.push((q, None));
            }
        }
        keys.sort_by(|p, q| p.0.total_cmp(&q.0).then(q.1.is_some().cmp(&p.1.is_some())));
        let mut out: Vec<Point> = Vec::with_capacity(keys.len());
        let mut last = f64::NEG_INFINITY;
        for (q, vertex) in keys {
            if vertex.is_none() && q - last < 0.05 {
                continue;
            }
            last = q;
            let k = at.partition_point(|&a| a <= q).clamp(1, points.len() - 1) - 1;
            let (ux, uz) = match vertex {
                Some(v) if v > 0 && v + 1 < points.len() => {
                    let (p, n) = (dir(v - 1), dir(v));
                    let (x, z) = (p.0 + n.0, p.1 + n.1);
                    // Along the bisector, longer at a corner so both sides keep the step.
                    let len = x.hypot(z);
                    if len < 1e-6 {
                        n
                    } else {
                        let k = (2.0 / len).min(2.0) / len;
                        (x * k, z * k)
                    }
                }
                Some(v) => dir(v.min(points.len() - 2)),
                None => dir(k),
            };
            let mut p = match vertex {
                Some(v) => points[v],
                None => {
                    let (a, b) = (points[k], points[k + 1]);
                    let f = ((q - at[k]) / (at[k + 1] - at[k]).max(1e-9)).clamp(0.0, 1.0);
                    Point {
                        x: a.x + (b.x - a.x) * f,
                        y: a.y + (b.y - a.y) * f,
                        z: a.z + (b.z - a.z) * f,
                        light: None,
                        kerb: false,
                    }
                }
            };
            let step = off(q);
            p.x -= uz * step;
            p.z += ux * step;
            out.push(p);
        }
        out
    }
}

/// Square metres for each person on a square or a pedestrian area, before footfall.
const SQUARE_M2: f64 = 100.0;
/// Shortest and longest line across a square, in metres, and most lines from each way in.
const ACROSS_MIN_M: f64 = 8.0;
const ACROSS_MAX_M: f64 = 120.0;
const ACROSS_EACH: usize = 4;
/// Way ids of the lines across squares: bit 62, past every OSM id, with Overture's bit 63 clear.
const ACROSS_ID: u64 = 1 << 62;

fn is_across(id: u64) -> bool {
    id >> 62 == 1
}

/// A square or a pedestrian area, crossed straight between the ways into it.
struct Square {
    rings: Vec<Vec<(f64, f64)>>,
    outline: FnvHashSet<u64>,
    min: (f64, f64),
    max: (f64, f64),
    people: f64,
}

impl Square {
    fn of(e: &ProcessedElement, scale: f64) -> Option<Self> {
        let tags = e.tags();
        let get = |k: &str| tags.get(k).map(String::as_str);
        let per_m2 = match (get("highway"), get("place")) {
            (Some("pedestrian"), _) | (None, Some("square")) => 1.0 / SQUARE_M2,
            (Some("footway"), _) => 0.5 / SQUARE_M2,
            _ => return None,
        };
        if off_limits(tags) || tags.contains_key("building") {
            return None;
        }
        // A closed pedestrian way without area=yes is a loop walked round, not an area.
        let rings: Vec<(&[ProcessedNode], bool)> = match e {
            ProcessedElement::Way(w) if get("area") == Some("yes") || get("highway").is_none() => {
                vec![(&w.nodes[..], false)]
            }
            ProcessedElement::Relation(r) if get("type") == Some("multipolygon") => r
                .members
                .iter()
                .filter_map(|m| match m.role {
                    ProcessedMemberRole::Outer => Some((&m.way.nodes[..], false)),
                    ProcessedMemberRole::Inner => Some((&m.way.nodes[..], true)),
                    ProcessedMemberRole::Part => None,
                })
                .collect(),
            _ => return None,
        };
        let mut square = Square {
            rings: Vec::new(),
            outline: Default::default(),
            min: (f64::MAX, f64::MAX),
            max: (f64::MIN, f64::MIN),
            people: 0.0,
        };
        let mut area = 0.0;
        for (nodes, inner) in rings {
            let (Some(a), Some(b)) = (nodes.first(), nodes.last()) else {
                continue;
            };
            if nodes.len() < 4 || (a.x, a.z) != (b.x, b.z) {
                continue;
            }
            let ring: Vec<(f64, f64)> = nodes
                .iter()
                .map(|n| (n.x as f64 + 0.5, n.z as f64 + 0.5))
                .collect();
            let twice: f64 = ring
                .windows(2)
                .map(|w| w[0].0 * w[1].1 - w[1].0 * w[0].1)
                .sum();
            area += if inner { -0.5 } else { 0.5 } * twice.abs();
            for &(x, z) in &ring {
                square.min = (square.min.0.min(x), square.min.1.min(z));
                square.max = (square.max.0.max(x), square.max.1.max(z));
            }
            square.outline.extend(nodes.iter().map(|n| n.id));
            square.rings.push(ring);
        }
        square.people = area / (scale * scale) * per_m2;
        (square.people > 0.0).then_some(square)
    }

    /// Inside, by the even-odd rule over every ring, holes included.
    fn contains(&self, (x, z): (f64, f64)) -> bool {
        let mut inside = false;
        for w in self.rings.iter().flat_map(|r| r.windows(2)) {
            let ((x1, z1), (x2, z2)) = (w[0], w[1]);
            if (z1 > z) != (z2 > z) && x < x1 + (z - z1) / (z2 - z1) * (x2 - x1) {
                inside = !inside;
            }
        }
        inside
    }

    fn near(&self, p: (f64, f64), reach: f64) -> bool {
        p.0 >= self.min.0 - reach
            && p.0 <= self.max.0 + reach
            && p.1 >= self.min.1 - reach
            && p.1 <= self.max.1 + reach
    }

    fn by_edge(&self, p: (f64, f64), reach: f64) -> bool {
        self.rings
            .iter()
            .flat_map(|r| r.windows(2))
            .any(|w| seg_dist(p, w[0], w[1]) <= reach)
    }
}

fn seg_dist(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dz) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dz * dz;
    let t = if len2 > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dz) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dz)
}

/// Straight lines across squares and pedestrian areas between the ways that come into
/// them, as people cut across, each a footway of two nodes with the people a metre it
/// carries each way. No line leaves the area, crosses a road or joins two points of one
/// way; whatever stands on the square is found once it is built.
fn across_squares(
    elements: &[ProcessedElement],
    ways: &[(&ProcessedWay, i32, f64)],
    uses: &FnvHashMap<u64, usize>,
    roads: &FnvHashMap<u64, (i32, u64, bool)>,
    scale: f64,
) -> Vec<(ProcessedWay, f64)> {
    let squares: Vec<Square> = elements
        .iter()
        .filter_map(|e| Square::of(e, scale))
        .collect();
    if squares.is_empty() {
        return Vec::new();
    }
    const CELL: f64 = 64.0;
    let cell = |(x, z): (f64, f64)| ((x / CELL).floor() as i32, (z / CELL).floor() as i32);
    let mut grid: FnvHashMap<(i32, i32), Vec<usize>> = FnvHashMap::default();
    for (i, sq) in squares.iter().enumerate() {
        let (a, b) = (cell(sq.min), cell(sq.max));
        for cx in a.0 - 1..=b.0 + 1 {
            for cz in a.1 - 1..=b.1 + 1 {
                grid.entry((cx, cz)).or_default().push(i);
            }
        }
    }
    let centre = |n: &ProcessedNode| (n.x as f64 + 0.5, n.z as f64 + 0.5);
    // The roads near each square, by their middle line and half width.
    type Road = ((f64, f64), (f64, f64), f64);
    let mut near_roads: Vec<Vec<Road>> = vec![Vec::new(); squares.len()];
    for e in elements {
        let ProcessedElement::Way(w) = e else {
            continue;
        };
        if !drivable(&w.tags) {
            continue;
        }
        let half = f64::from(highway_block_range(&w.tags["highway"], &w.tags, scale));
        for seg in w.nodes.windows(2) {
            let (a, b) = (centre(&seg[0]), centre(&seg[1]));
            let mut hit: Vec<usize> = Vec::new();
            let steps = ((b.0 - a.0).hypot(b.1 - a.1) / CELL).ceil() as usize + 1;
            for k in 0..=steps {
                let t = k as f64 / steps as f64;
                let p = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                for &i in grid.get(&cell(p)).into_iter().flatten() {
                    if !hit.contains(&i) && squares[i].near(p, CELL + half) {
                        hit.push(i);
                        near_roads[i].push((a, b, half));
                    }
                }
            }
        }
    }
    // The ways in: nodes on the outline, and where a way ends, meets another or comes in.
    let mut doors: Vec<Vec<(&ProcessedNode, Vec<u64>)>> = vec![Vec::new(); squares.len()];
    for &(w, ..) in ways {
        let last = w.nodes.len() - 1;
        for (k, n) in w.nodes.iter().enumerate() {
            if roads.contains_key(&n.id) || is_invented_node_id(n.id) {
                continue;
            }
            let p = centre(n);
            for &i in grid.get(&cell(p)).into_iter().flatten() {
                let sq = &squares[i];
                if !sq.near(p, 2.0) {
                    continue;
                }
                let on = sq.outline.contains(&n.id);
                let inside = |n: &ProcessedNode| sq.contains(centre(n));
                let edge = k == 0
                    || k == last
                    || uses.get(&n.id).is_some_and(|&u| u > 1)
                    || !inside(&w.nodes[k - 1])
                    || !inside(&w.nodes[k + 1]);
                if !(on || edge && (inside(n) || sq.by_edge(p, 2.0))) {
                    continue;
                }
                let list = &mut doors[i];
                if let Some(d) = list.iter_mut().find(|d| d.0.id == n.id) {
                    d.1.push(w.id);
                } else if list
                    .iter()
                    .all(|d| (centre(d.0).0 - p.0).hypot(centre(d.0).1 - p.1) > 3.0)
                {
                    list.push((n, vec![w.id]));
                }
            }
        }
    }
    let mut lines: Vec<(ProcessedWay, f64)> = Vec::new();
    for (i, sq) in squares.iter().enumerate() {
        let doors = &doors[i];
        let clear = |a: (f64, f64), b: (f64, f64)| {
            let len = (b.0 - a.0).hypot(b.1 - a.1);
            // Clear of the edge away from its ends, or one way's lane runs outside.
            let inside = (2..(len - 1.0) as usize).all(|k| {
                let p = (
                    a.0 + (b.0 - a.0) * k as f64 / len,
                    a.1 + (b.1 - a.1) * k as f64 / len,
                );
                sq.contains(p) && (k < 4 || k as f64 > len - 4.0 || !sq.by_edge(p, 1.5))
            });
            inside
                && near_roads[i]
                    .iter()
                    .all(|&(p, q, half)| seg_seg_dist(a, b, p, q) > half + 1.0)
        };
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        for (j, a) in doors.iter().enumerate() {
            let pa = centre(a.0);
            let mut others: Vec<(usize, f64)> = doors
                .iter()
                .enumerate()
                .filter(|(k, b)| *k != j && !b.1.iter().any(|w| a.1.contains(w)))
                .map(|(k, b)| (k, (centre(b.0).0 - pa.0).hypot(centre(b.0).1 - pa.1)))
                .filter(|&(_, d)| (ACROSS_MIN_M * scale..=ACROSS_MAX_M * scale).contains(&d))
                .collect();
            others.sort_by(|x, y| x.1.total_cmp(&y.1));
            // Enough to fan out from; a vast area is not walked end to end anyway.
            others.truncate(4 * ACROSS_EACH);
            let mut headings: Vec<(f64, f64)> = Vec::new();
            for (k, _) in others {
                if headings.len() >= ACROSS_EACH {
                    break;
                }
                let pb = centre(doors[k].0);
                let Some(h) = unit(pb.0 - pa.0, pb.1 - pa.1) else {
                    continue;
                };
                // Fanned out, not three lines to the same corner.
                if headings.iter().any(|g| g.0 * h.0 + g.1 * h.1 > 0.866) || !clear(pa, pb) {
                    continue;
                }
                headings.push(h);
                let pair = (j.min(k), j.max(k));
                if !pairs.contains(&pair) {
                    pairs.push(pair);
                }
            }
        }
        let metres: f64 = pairs
            .iter()
            .map(|&(a, b)| {
                let (pa, pb) = (centre(doors[a].0), centre(doors[b].0));
                (pb.0 - pa.0).hypot(pb.1 - pa.1) / scale
            })
            .sum();
        if metres <= 0.0 {
            continue;
        }
        let density = (sq.people / (2.0 * metres)).min(1.0 / 8.0);
        for (a, b) in pairs {
            let bare = |n: &ProcessedNode| ProcessedNode {
                id: n.id,
                tags: HashMap::new(),
                x: n.x,
                z: n.z,
            };
            let way = ProcessedWay {
                id: ACROSS_ID | lines.len() as u64,
                nodes: vec![bare(doors[a].0), bare(doors[b].0)],
                tags: [("highway".to_string(), "footway".to_string())].into(),
            };
            lines.push((way, density));
        }
    }
    lines
}

fn seg_seg_dist(a: (f64, f64), b: (f64, f64), p: (f64, f64), q: (f64, f64)) -> f64 {
    let cross = |o: (f64, f64), u: (f64, f64), v: (f64, f64)| {
        (u.0 - o.0) * (v.1 - o.1) - (u.1 - o.1) * (v.0 - o.0)
    };
    let (d1, d2) = (cross(a, b, p), cross(a, b, q));
    let (d3, d4) = (cross(p, q, a), cross(p, q, b));
    if (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0) {
        return 0.0;
    }
    seg_dist(a, p, q)
        .min(seg_dist(b, p, q))
        .min(seg_dist(p, a, b))
        .min(seg_dist(q, a, b))
}

/// Shops, cafés, sights and stations, which bring people onto the ways near them.
fn draws_people(tags: &HashMap<String, String>) -> bool {
    let get = |k: &str| tags.get(k).map(String::as_str);
    tags.contains_key("shop")
        || matches!(
            get("amenity"),
            Some(
                "restaurant"
                    | "cafe"
                    | "fast_food"
                    | "bar"
                    | "pub"
                    | "ice_cream"
                    | "bank"
                    | "pharmacy"
                    | "marketplace"
                    | "theatre"
                    | "cinema"
                    | "library"
                    | "townhall"
                    | "place_of_worship"
                    | "university"
                    | "school"
            )
        )
        || matches!(
            get("tourism"),
            Some("attraction" | "museum" | "gallery" | "viewpoint" | "hotel")
        )
        || matches!(get("railway"), Some("station" | "halt"))
        || get("public_transport") == Some("station")
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    x: f64,
    y: f64,
    z: f64,
    /// The light to wait for before going on: 1 while east-west traffic has green, 2 while
    /// north-south traffic has.
    light: Option<u8>,
    /// A kerb, where a walker waits for a gap in the traffic.
    kerb: bool,
}

struct Lane {
    points: Vec<Point>,
    /// The same points along the middle of the way, walked if the side is blocked.
    alt: Vec<Point>,
    /// The lane before it was bent round street furniture, if it was: walked if the bend
    /// runs into something the straight lane passes.
    plain: Vec<Point>,
    next: Vec<usize>,
    /// The lane back the other way, turned round into at a dead end or a blocked path.
    back: Option<usize>,
    /// Cut short of its first or its last node by something in the way.
    cut: (bool, bool),
    /// For each point, the nearest point of the lane back, where a walker turning round
    /// carries on from; None where that is too far off, past whatever cut the lanes.
    beside: Vec<Option<usize>>,
    /// Runs off the edge of the area, where walkers leave.
    out: bool,
    density: f64,
    /// Cuts across a square: dropped if anything stands in its way.
    across: bool,
}

/// The walkable ways of an area.
pub struct WalkNetwork {
    lanes: Vec<Lane>,
    /// Where walkers can join, as (lane, point) by map cell.
    spots: BTreeMap<(i32, i32), Vec<(usize, usize)>>,
    meta: Vec<Meta>,
    size: f64,
    scale: f64,
    keep_left: bool,
}

/// A lane's piece, whether it runs backward along it, its first and last node and whether
/// it is a crossing.
type Meta = (usize, bool, u64, u64, bool);

/// A stretch of a way between two junctions.
struct Piece<'a> {
    way: &'a ProcessedWay,
    nodes: &'a [ProcessedNode],
    half: i32,
    density: f64,
}

/// Mark along a lane: distance, light and kerb.
type Mark = (f64, Option<u8>, bool);

impl WalkNetwork {
    /// `surface_y` is the height of a way's surface block at a column.
    pub fn build(
        elements: &[ProcessedElement],
        scale: f64,
        keep_left: bool,
        signals: bool,
        world: &XZBBox,
        surface_y: impl Fn(&ProcessedWay, i32, i32) -> i32,
    ) -> Self {
        let size = scale.clamp(0.4, 3.0);
        let found: Vec<(&ProcessedWay, (i32, f64))> = elements
            .iter()
            .filter_map(|e| match e {
                ProcessedElement::Way(w) if w.nodes.len() >= 2 => {
                    let density = walk_density(&w.tags)?;
                    let half = highway_block_range(&w.tags["highway"], &w.tags, scale);
                    Some((w, (half, density)))
                }
                _ => None,
            })
            .collect();
        // A drawn outline cuts the paths where they leave it, which walkers treat as the edge.
        let found = crate::clipping::cut_ways_to_outline(found, world);
        let ways: Vec<(&ProcessedWay, i32, f64)> = found
            .iter()
            .map(|(w, (half, density))| (w.as_ref(), *half, *density))
            .collect();
        // How wide the road is at every node a road runs through, where a walker crosses:
        // the widest, the first road there and whether any other meets it.
        let mut roads: FnvHashMap<u64, (i32, u64, bool)> = FnvHashMap::default();
        let mut draws: Vec<(f64, f64)> = Vec::new();
        let mut obstacles: Vec<Item> = Vec::new();
        for e in elements {
            match e {
                ProcessedElement::Way(w) if drivable(&w.tags) => {
                    let half = highway_block_range(&w.tags["highway"], &w.tags, scale);
                    for n in &w.nodes {
                        let r = roads.entry(n.id).or_insert((half, w.id, false));
                        r.0 = r.0.max(half);
                        r.2 |= r.1 != w.id;
                    }
                }
                _ => {}
            }
            if let ProcessedElement::Node(n) = e {
                obstacles.extend(furniture(n));
            }
            match e {
                ProcessedElement::Node(n) if draws_people(&n.tags) => {
                    draws.push((n.x as f64, n.z as f64));
                }
                ProcessedElement::Way(w) if draws_people(&w.tags) && !w.nodes.is_empty() => {
                    let k = w.nodes.len() as f64;
                    draws.push((
                        w.nodes.iter().map(|n| n.x as f64).sum::<f64>() / k,
                        w.nodes.iter().map(|n| n.z as f64).sum::<f64>() / k,
                    ));
                }
                _ => {}
            }
        }
        let footfall = Footfall::new(&draws, FOOTFALL_M * scale);
        let furniture = Furniture::new(obstacles);

        let mut uses: FnvHashMap<u64, usize> = FnvHashMap::default();
        for (w, _, _) in &ways {
            for n in &w.nodes {
                *uses.entry(n.id).or_default() += 1;
            }
        }
        let across: Vec<(ProcessedWay, f64)> =
            across_squares(elements, &ways, &uses, &roads, scale)
                .into_iter()
                .flat_map(|(w, d)| {
                    crate::clipping::cut_way_to_outline(&w, world)
                        .unwrap_or_else(|| vec![w])
                        .into_iter()
                        .map(move |p| (p, d))
                })
                .collect();
        for (w, _) in &across {
            for n in &w.nodes {
                *uses.entry(n.id).or_default() += 1;
            }
        }
        let ways: Vec<(&ProcessedWay, i32, f64)> = ways
            .into_iter()
            .chain(
                across
                    .iter()
                    .map(|(w, d)| (w, highway_block_range("footway", &w.tags, scale), *d)),
            )
            .collect();
        let mut pieces: Vec<Piece> = Vec::new();
        for &(way, half, density) in &ways {
            let mut start = 0;
            for k in 1..way.nodes.len() {
                if k == way.nodes.len() - 1 || uses[&way.nodes[k].id] > 1 {
                    pieces.push(Piece {
                        way,
                        nodes: &way.nodes[start..=k],
                        half,
                        density,
                    });
                    start = k;
                }
            }
        }

        // The road a walker on `way` crosses at a node; none where that is only the way
        // itself, as along a living street.
        let road_at = |n: &ProcessedNode, way: u64| match roads.get(&n.id) {
            Some(&(_, only, false)) if only == way => None,
            Some(&(half, ..)) => Some(half),
            None => None,
        };
        let at_edge = |n: &ProcessedNode| {
            is_invented_node_id(n.id)
                || n.x - world.min_x() < 3
                || world.max_x() - n.x < 3
                || n.z - world.min_z() < 3
                || world.max_z() - n.z < 3
        };
        // Lanes as (piece, backward, nodes), both ways along each piece.
        let mut lanes: Vec<Lane> = Vec::new();
        let mut meta: Vec<Meta> = Vec::new();
        let mut points = 0usize;
        for (p, piece) in pieces.iter().enumerate() {
            let highway = piece.way.tags["highway"].as_str();
            let count = if matches!(highway, "pedestrian" | "living_street") {
                ((2 * piece.half + 1) / 4).clamp(1, 3) as usize
            } else {
                1
            };
            let crossing = is_crossing_way(&piece.way.tags);
            let signalled = signals
                && crossing
                && (crossing_signalled(&piece.way.tags)
                    || piece.nodes.iter().any(|n| node_signalled(&n.tags)));
            for backward in [false, true] {
                let nodes: Vec<&ProcessedNode> = if backward {
                    piece.nodes.iter().rev().collect()
                } else {
                    piece.nodes.iter().collect()
                };
                for k in 0..count {
                    let offset =
                        (f64::from(piece.half) + 0.5) * (k as f64 + 0.5) / count as f64 * 0.8;
                    let line = lane_line(&nodes, offset.max(0.4), keep_left);
                    let path = Measured::new(line);
                    let middle = Measured::new(lane_line(&nodes, 0.0, keep_left));
                    let stretch = if path.length() > 0.0 {
                        middle.length() / path.length()
                    } else {
                        1.0
                    };
                    let mut marks: Vec<Mark> =
                        vec![(0.0, None, false), (path.length(), None, false)];
                    let inner = path.at.iter().take(path.points.len().saturating_sub(1));
                    marks.extend(inner.skip(1).map(|&at| (at, None, false)));
                    let heading = |k: usize| {
                        let (a, b) = (
                            path.points[k.saturating_sub(1).min(path.points.len() - 2)],
                            path.points[(k + 1).min(path.points.len() - 1)],
                        );
                        unit(b.0 - a.0, b.1 - a.1).unwrap_or((1.0, 0.0))
                    };
                    let group = |dir: (f64, f64)| if group_of(dir) == 'a' { 1 } else { 2 };
                    if crossing && road_at(nodes[0], piece.way.id).is_none() {
                        // A crossing way starts at the kerb, unless mapped from the road's middle.
                        marks[0].1 = signalled.then(|| group(heading(0)));
                        marks[0].2 = !signalled;
                    }
                    // Every road crossed on the way, and one at the end where the way carries
                    // on across it; one it ends on is cut short of below.
                    for (k, n) in nodes.iter().enumerate().skip(1) {
                        if k == nodes.len() - 1 && uses[&n.id] < 2 {
                            continue;
                        }
                        let Some(road) = road_at(n, piece.way.id) else {
                            continue;
                        };
                        if crossing {
                            continue;
                        }
                        let s = (path.at[k] - (f64::from(road) + KERB_BACK)).max(0.0);
                        let light = (signals && node_signalled(&n.tags)).then(|| group(heading(k)));
                        marks.push((s, light, light.is_none()));
                    }
                    // A way that stops on a road, as many are mapped, stops at its kerb.
                    let kerb = |n: &ProcessedNode| match road_at(n, piece.way.id) {
                        Some(half) if uses[&n.id] == 1 => f64::from(half) + KERB_BACK,
                        _ => 0.0,
                    };
                    let (lo, hi) = (kerb(nodes[0]), path.length() - kerb(nodes[nodes.len() - 1]));
                    if lo > 0.0 || hi < path.length() {
                        marks.retain(|m| (lo..=hi).contains(&m.0));
                        if hi - lo >= 2.0 {
                            marks.extend([(lo, None, false), (hi, None, false)]);
                        } else {
                            marks.clear();
                        }
                    }
                    marks.sort_by(|a, b| a.0.total_cmp(&b.0));
                    let mut merged: Vec<Mark> = Vec::new();
                    for m in marks {
                        match merged.last_mut() {
                            Some(last) if m.0 - last.0 < 0.5 => {
                                last.1 = last.1.or(m.1);
                                last.2 |= m.2;
                            }
                            _ => merged.push(m),
                        }
                    }
                    let y_at = |s: f64| {
                        let (x, z) = path.point_at(s);
                        (surface_y(piece.way, x.floor() as i32, z.floor() as i32) + 1) as f64
                    };
                    let mut stations: Vec<Mark> = Vec::new();
                    for &m in &merged {
                        if let Some(&(prev, _, _)) = stations.last() {
                            refine(prev, m.0, &y_at, 0, &mut stations);
                        }
                        stations.push(m);
                    }
                    let (mut pts, mut alt) = (Vec::new(), Vec::new());
                    for &(s, light, kerb) in &stations {
                        let at = |(x, z): (f64, f64)| Point {
                            x: (x * 10.0).round() / 10.0,
                            y: y_at(s),
                            z: (z * 10.0).round() / 10.0,
                            light,
                            kerb,
                        };
                        let p = at(path.point_at(s));
                        if world.contains(&XZPoint::new(p.x.floor() as i32, p.z.floor() as i32)) {
                            pts.push(p);
                            alt.push(at(middle.point_at(s * stretch)));
                        }
                    }
                    let across = is_across(piece.way.id);
                    let plain = pts.clone();
                    let pts = furniture.go_round(pts);
                    let plain = if plain == pts { Vec::new() } else { plain };
                    // A line across a square has no middle to fall back on.
                    let alt = if across {
                        Vec::new()
                    } else {
                        furniture.go_round(alt)
                    };
                    points += pts.len();
                    let mid = path.point_at(path.length() / 2.0);
                    lanes.push(Lane {
                        points: pts,
                        alt,
                        plain,
                        next: Vec::new(),
                        back: None,
                        cut: (false, false),
                        beside: Vec::new(),
                        out: at_edge(nodes[nodes.len() - 1]),
                        density: piece.density * footfall.at(mid) * 2.0 / (count as f64 + 1.0),
                        across,
                    });
                    meta.push((
                        p,
                        backward,
                        nodes[0].id,
                        nodes[nodes.len() - 1].id,
                        crossing,
                    ));
                }
            }
        }
        if points > MAX_POINTS {
            prune(&mut lanes);
        }
        link(&mut lanes, &meta, scale);
        untrap(&mut lanes, scale);
        let spots = spots(&lanes, &meta, scale);
        Self {
            lanes,
            spots,
            meta,
            size,
            scale,
            keep_left,
        }
    }

    /// The columns every block along each lane, and along the middle of its way, whose
    /// ground is read once the world is built.
    pub fn probe(&self) -> WalkProbe {
        WalkProbe::new(self.lanes.iter().flat_map(|l| {
            steps(&l.points)
                .into_iter()
                .chain(steps(&l.alt))
                .chain(steps(&l.plain))
                .map(|s| s.column())
        }))
    }

    /// Puts every lane on the ground as it was built, past a post or a tree but never
    /// through a wall or a house: a lane that would walks the middle of its way instead,
    /// or stops short of it, where its walkers turn round.
    pub fn finish(&mut self, probe: &WalkProbe) {
        for lane in &mut self.lanes {
            if lane.points.len() >= 2 {
                let mut side = settle(&lane.points, probe);
                // Bent round street furniture into a wall: straight on, if that gets further.
                if side.as_ref().is_none_or(|s| s.cut != (false, false)) {
                    if let Some(p) = settle(&lane.plain, probe) {
                        if side.as_ref().is_none_or(|s| p.length() > s.length()) {
                            side = Some(p);
                        }
                    }
                }
                let best = match side {
                    Some(s) if s.cut == (false, false) => Some(s),
                    _ if lane.across => None,
                    side => match (side, settle(&lane.alt, probe)) {
                        (Some(s), Some(m))
                            if s.length() >= m.length() && m.cut != (false, false) =>
                        {
                            Some(s)
                        }
                        (side, middle) => middle.or(side),
                    },
                };
                let best = best.unwrap_or(Settled {
                    points: Vec::new(),
                    cut: (false, false),
                    posts: Vec::new(),
                });
                // Round the posts it met, as round mapped furniture, onto the ground read
                // round them; where that is not clear, past them.
                let posts: Vec<Item> = best
                    .posts
                    .iter()
                    .map(|&(x, z)| Item {
                        x: x as f64 + 0.5,
                        z: z as f64 + 0.5,
                        hx: 0.4,
                        hz: 0.4,
                        long: 0.0,
                    })
                    .collect();
                lane.points = if posts.is_empty() {
                    best.points
                } else {
                    let bent = Furniture::new(posts).go_round(best.points.clone());
                    match settle(&bent, probe) {
                        Some(s) if s.cut == (false, false) => s.points,
                        _ => best.points,
                    }
                };
                lane.cut = best.cut;
                lane.out &= !best.cut.1;
            }
            lane.alt = Vec::new();
            lane.plain = Vec::new();
        }
        if self.lanes.iter().map(|l| l.points.len()).sum::<usize>() > MAX_POINTS {
            prune(&mut self.lanes);
        }
        link(&mut self.lanes, &self.meta, self.scale);
        untrap(&mut self.lanes, self.scale);
        self.spots = spots(&self.lanes, &self.meta, self.scale);
    }

    pub fn is_empty(&self) -> bool {
        self.spots.is_empty()
    }

    /// Lanes walkers can be on.
    pub fn lane_count(&self) -> usize {
        self.lanes.iter().filter(|l| l.points.len() >= 2).count()
    }

    /// Thousandths of a block a walker of each pace covers in a step of two ticks.
    fn paces(&self) -> [i32; 7] {
        PACES.map(|p| (WALK_MPS * p * 0.1 * self.scale * 1000.0).round().max(20.0) as i32)
    }

    /// The pack functions: the lanes in storage, the moves at each pace and what a walker
    /// looks out for, all at the world's scale.
    pub fn pack_functions(&self) -> Vec<(String, String)> {
        let k = self.size;
        let d = |m: f64| format!("{:.2}", m * k);
        let d3 = |m: f64| format!("{:.3}", m * k);
        let mut network = format!(
            "scoreboard players set #day_cap arnis.walk {DAY_NEAR}\n\
             scoreboard players set #night_cap arnis.walk {NIGHT_NEAR}\n\
             scoreboard players set #spot_cell arnis.walk {}\n\
             data modify storage arnis:walk seg set value []\n",
            SPOT_CELL as i32
        );
        let paces = self.paces();
        let _ = writeln!(
            network,
            "data modify storage arnis:walk paces set value [I;{}]",
            paces.map(|p| p.to_string()).join(",")
        );
        // Grown-ups from a little under to a little over the player's height; children.
        let _ = writeln!(
            network,
            "data modify storage arnis:walk sizes set value [{}]",
            [0.86, 0.9, 0.92, 0.94, 0.96, 0.98, 1.0, 1.02, 0.6, 0.68]
                .map(|s| format!("{:.3}d", s * k))
                .join(",")
        );
        for batch in self.lanes.chunks(LANES_PER_LINE) {
            network.push_str("data modify storage arnis:walk batch set value [");
            for (b, lane) in batch.iter().enumerate() {
                if b > 0 {
                    network.push(',');
                }
                // A lane nobody walks only holds its place in the list.
                if lane.points.len() < 2 {
                    network.push_str("{p:[],n:[I;],b:-1,r:[I;],o:0b}");
                    continue;
                }
                network.push_str("{p:[");
                for (i, p) in lane.points.iter().enumerate() {
                    if i > 0 {
                        network.push(',');
                    }
                    let _ = write!(
                        network,
                        "{{x:{:.2}d,y:{:.1}d,z:{:.2}d",
                        macro_coord(p.x),
                        p.y,
                        macro_coord(p.z)
                    );
                    if let Some(g) = p.light {
                        let _ = write!(network, ",g:{g}b");
                    }
                    if p.kerb {
                        network.push_str(",c:1b");
                    }
                    network.push('}');
                }
                let next: Vec<String> = lane.next.iter().map(|n| n.to_string()).collect();
                let beside: Vec<String> = lane
                    .beside
                    .iter()
                    .map(|k| k.map_or(-1, |k| k as i64).to_string())
                    .collect();
                let _ = write!(
                    network,
                    "],n:[I;{}],b:{},r:[I;{}],o:{}b}}",
                    next.join(","),
                    lane.back.map_or(-1, |b| b as i64),
                    beside.join(","),
                    u8::from(lane.out)
                );
            }
            network.push_str(
                "]\ndata modify storage arnis:walk seg append from storage arnis:walk batch[]\n",
            );
        }
        network.push_str("data remove storage arnis:walk batch\n");
        network.push_str("data modify storage arnis:walk grid set value {}\n");
        let cells: Vec<_> = self.spots.iter().collect();
        for batch in cells.chunks(CELLS_PER_LINE) {
            let cells: Vec<String> = batch
                .iter()
                .map(|((x, z), spots)| {
                    let spots: Vec<String> = spots
                        .iter()
                        .map(|(lane, point)| format!("[I;{lane},{point}]"))
                        .collect();
                    format!("\"{x}_{z}\":[{}]", spots.join(","))
                })
                .collect();
            let _ = writeln!(
                network,
                "data modify storage arnis:walk grid merge value {{{}}}",
                cells.join(",")
            );
        }

        let mut moves = String::new();
        // The usual paces first, as most walk at them.
        for p in [3, 2, 4, 1, 5, 0, 6].map(|i| paces[i]) {
            let _ = writeln!(
                moves,
                "execute if score @s arnis.wv matches {p} run return run tp @s ^ ^ ^{:.3}",
                f64::from(p) / 1000.0
            );
        }
        // Waits on someone held up themselves go to the older walker, so no two ever wait
        // on each other.
        let ahead = format!(
            "# Someone right in front: 1 to wait for a player; 4 to queue behind a walker who\n\
             # stands, is all but touching, crosses or queues; 3 to step aside for one coming\n\
             # the other way or held up behind; 2 to fall in behind one going the same way.\n\
             # Queued 20 s, a walker goes on to the next point.\n\
             execute positioned ^ ^ ^{near} if entity @a[gamemode=!spectator,tag=!arnis.riding,distance=..{round}] run return 1\n\
             execute if score @s arnis.wb matches 200.. run return 0\n\
             scoreboard players operation #me arnis.walk = @s arnis.wk\n\
             execute positioned ^ ^ ^{close} as @e[tag=arnis.walker,distance=..{touch}] unless score @s arnis.wb matches 1.. run return 4\n\
             execute positioned ^ ^ ^{close} as @e[tag=arnis.walker,distance=..{touch}] if score @s arnis.wk < #me arnis.walk run return 4\n\
             execute positioned ^ ^ ^{near} unless entity @e[tag=arnis.walker,distance=..{walker}] run return 0\n\
             scoreboard players operation #yaw arnis.walk = @s arnis.wy\n\
             execute positioned ^ ^ ^{near} as @e[tag=arnis.walker,distance=..{walker},sort=nearest,limit=1] run function arnis:walk/lead\n\
             execute unless score #turn arnis.walk matches -135..135 run return 3\n\
             execute positioned ^ ^ ^{near} if entity @e[tag=arnis.walker,distance=..{walker},scores={{arnis.ww=1..}}] run return 4\n\
             execute positioned ^ ^ ^{near} as @e[tag=arnis.walker,distance=..{walker},scores={{arnis.wb=1..199}}] if score @s arnis.wk < #me arnis.walk run return 4\n\
             execute if score #turn arnis.walk matches -45..45 run return 2\n\
             execute if score #free arnis.walk matches 1 run return 4\n\
             return 3\n",
            near = d(0.9),
            round = d(0.7),
            walker = d(0.45),
            // Closer in, short of the walker's own place, so it never counts itself.
            close = d3(0.35),
            touch = d3(0.32),
        );
        // Each to their own side, as on the roads.
        let aside = format!(
            "tag @s add arnis.w_aside\ntp @s ^{} ^ ^\nfunction arnis:walk/aim\n",
            if self.keep_left {
                d(0.4)
            } else {
                format!("-{}", d(0.4))
            }
        );
        // Only traffic across the walker's way counts; cars alongside pass by.
        let mut gap =
            String::from("# A car crossing the way ahead coming close: wait for the gap.\n");
        let (along, across) = (["-45..45", "135..-135"], ["45..135", "-135..-45"]);
        for (walker, cars) in [(along, across), (across, along)] {
            for w in walker {
                for c in cars {
                    let _ = writeln!(
                        gap,
                        "execute if entity @s[y_rotation={w}] if entity @e[type=marker,tag=arnis.car,tag=!arnis.car.still,distance=..{},y_rotation={c}] run return 0",
                        d(9.0)
                    );
                }
            }
        }
        gap.push_str("return 1\n");
        // Company keeps clear of what a walker looks out for ahead.
        let behind = format!(
            "execute at @s run tp @s ^ ^ ^-{}\nfunction arnis:walk/aim\n",
            d(1.7)
        );
        vec![
            (
                "data/arnis/function/walk/network.mcfunction".to_string(),
                network,
            ),
            (
                "data/arnis/function/walk/move.mcfunction".to_string(),
                moves,
            ),
            (
                "data/arnis/function/walk/ahead.mcfunction".to_string(),
                ahead,
            ),
            ("data/arnis/function/walk/gap.mcfunction".to_string(), gap),
            (
                "data/arnis/function/walk/behind.mcfunction".to_string(),
                behind,
            ),
            (
                "data/arnis/function/walk/aside.mcfunction".to_string(),
                aside,
            ),
        ]
    }
}

fn is_crossing_way(tags: &HashMap<String, String>) -> bool {
    tags.get("footway").map(String::as_str) == Some("crossing")
        || tags.get("path").map(String::as_str) == Some("crossing")
        || tags.get("cycleway").map(String::as_str) == Some("crossing")
}

fn crossing_signalled(tags: &HashMap<String, String>) -> bool {
    tags.get("crossing").map(String::as_str) == Some("traffic_signals")
        || tags.get("crossing:signals").map(String::as_str) == Some("yes")
}

fn node_signalled(tags: &HashMap<String, String>) -> bool {
    crossing_signalled(tags) || tags.get("highway").map(String::as_str) == Some("traffic_signals")
}

fn unit(dx: f64, dz: f64) -> Option<(f64, f64)> {
    let len = dx.hypot(dz);
    (len > 1e-9).then(|| (dx / len, dz / len))
}

/// A polyline measured along its length.
struct Measured {
    points: Vec<(f64, f64)>,
    at: Vec<f64>,
}

impl Measured {
    fn new(points: Vec<(f64, f64)>) -> Self {
        let mut at = vec![0.0];
        for w in points.windows(2) {
            let last = *at.last().unwrap();
            at.push(last + (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1));
        }
        Self { points, at }
    }

    fn length(&self) -> f64 {
        *self.at.last().unwrap_or(&0.0)
    }

    fn point_at(&self, s: f64) -> (f64, f64) {
        let s = s.clamp(0.0, self.length());
        let k = self
            .at
            .partition_point(|&a| a < s)
            .clamp(1, self.points.len() - 1);
        let (a, b) = (self.points[k - 1], self.points[k]);
        let span = self.at[k] - self.at[k - 1];
        let t = if span > 0.0 {
            (s - self.at[k - 1]) / span
        } else {
            0.0
        };
        (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
    }
}

/// Splits a stretch until no point is more than `MAX_SPAN` from the next and the ground
/// in between is within a block of the straight line.
fn refine(a: f64, b: f64, y_at: &impl Fn(f64) -> f64, depth: u32, out: &mut Vec<Mark>) {
    if depth >= 6 || b - a < 2.0 {
        return;
    }
    let pieces = ((b - a) / MAX_SPAN).ceil() as usize;
    if pieces > 1 {
        let step = (b - a) / pieces as f64;
        for j in 0..pieces {
            let end = a + step * (j + 1) as f64;
            refine(end - step, end, y_at, depth + 1, out);
            if j + 1 < pieces {
                out.push((end, None, false));
            }
        }
        return;
    }
    let mid = (a + b) / 2.0;
    if (y_at(mid) - (y_at(a) + y_at(b)) / 2.0).abs() >= 1.0 {
        refine(a, mid, y_at, depth + 1, out);
        out.push((mid, None, false));
        refine(mid, b, y_at, depth + 1, out);
    }
}

/// A spot about every block along a lane, and the lane point it is, if one.
struct Step {
    x: f64,
    y: f64,
    z: f64,
    point: Option<usize>,
}

impl Step {
    /// The column and the ground block expected under the walker's feet.
    fn column(&self) -> (i32, i32, i32) {
        (
            self.x.floor() as i32,
            self.z.floor() as i32,
            self.y.round() as i32 - 1,
        )
    }
}

fn steps(points: &[Point]) -> Vec<Step> {
    let mut out = Vec::new();
    for (i, w) in points.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        let n = (b.x - a.x).hypot(b.z - a.z).ceil().max(1.0) as usize;
        for j in 0..n {
            let t = j as f64 / n as f64;
            out.push(Step {
                x: a.x + (b.x - a.x) * t,
                y: a.y + (b.y - a.y) * t,
                z: a.z + (b.z - a.z) * t,
                point: (j == 0).then_some(i),
            });
        }
    }
    if let Some(last) = points.last() {
        out.push(Step {
            x: last.x,
            y: last.y,
            z: last.z,
            point: Some(points.len() - 1),
        });
    }
    out
}

/// A lane put on the ground: its points and whether it was cut short at its start or end.
struct Settled {
    points: Vec<Point>,
    cut: (bool, bool),
    /// Columns of the posts it passes, as a lamp or a tree trunk nobody mapped.
    posts: Vec<(i32, i32)>,
}

impl Settled {
    fn length(&self) -> f64 {
        self.points
            .windows(2)
            .map(|w| (w[1].x - w[0].x).hypot(w[1].z - w[0].z))
            .sum()
    }
}

/// The lane on the ground the probe found along it, with points put in where that ground
/// bends. Something wider than a post in the way, or a climb of more than a step, cuts it:
/// the longest stretch clear of those is kept, if it is a few blocks long.
fn settle(points: &[Point], probe: &WalkProbe) -> Option<Settled> {
    let steps = steps(points);
    let mut feet: Vec<Option<f64>> = Vec::with_capacity(steps.len());
    let mut prev: Option<f64> = None;
    let mut post: Option<(i32, i32)> = None;
    let mut posts = Vec::new();
    for s in &steps {
        let (x, z, level) = s.column();
        let footing = probe.get(x, z, level);
        // A column never read, off the area, keeps the height it was planned at.
        let found = match footing {
            None => Some(s.y),
            Some(f) => f
                .nearest(prev.unwrap_or(s.y))
                .filter(|&g| prev.is_none_or(|p| (g - p).abs() <= 1.0)),
        };
        let at = match found {
            Some(g) => {
                post = None;
                prev = Some(g);
                Some(g)
            }
            // One post is walked past, but not a wall, nor posts side by side.
            None if footing.is_some_and(|f| f.thin()) && post.is_none_or(|p| p == (x, z)) => {
                if post.is_none() {
                    posts.push((x, z));
                }
                post = Some((x, z));
                Some(prev.unwrap_or(s.y))
            }
            None => {
                post = None;
                prev = None;
                None
            }
        };
        feet.push(at);
    }
    // The longest stretch of steps with ground under them.
    let (mut a, mut b) = (0, 0);
    let mut start = None;
    for k in 0..=feet.len() {
        match (feet.get(k).copied().flatten(), start) {
            (Some(_), None) => start = Some(k),
            (None, Some(s)) => {
                if k - s > b - a {
                    (a, b) = (s, k);
                }
                start = None;
            }
            _ => {}
        }
    }
    // A few blocks at least, as far as the steps go.
    if b - a < 4 {
        return None;
    }
    let b = b - 1;
    // Each step as high as its neighbours, so a walker climbs over the lower block of a
    // step and never sinks into the higher one.
    let high: Vec<f64> = (0..feet.len())
        .map(|k| {
            (k.saturating_sub(1).max(a)..=(k + 1).min(b))
                .filter_map(|j| feet[j])
                .fold(f64::MIN, f64::max)
        })
        .collect();
    let y = |k: usize| high[k];
    let at: Vec<usize> = (a..=b)
        .filter(|&k| k == a || k == b || steps[k].point.is_some())
        .collect();
    let point = |k: usize| match steps[k].point {
        Some(i) => Point {
            y: y(k),
            ..points[i]
        },
        None => step_point(&steps[k], y(k)),
    };
    let mut out = Vec::with_capacity(at.len());
    for w in at.windows(2) {
        out.push(point(w[0]));
        bend(&steps, &y, w[0], w[1], &mut out);
    }
    out.push(point(b));
    Some(Settled {
        points: out,
        cut: (a > 0, b + 1 < steps.len()),
        posts,
    })
}

/// A point at a step, to a tenth but in the column the ground was read in.
fn step_point(s: &Step, y: f64) -> Point {
    let tenth = |v: f64| ((v * 10.0).round() / 10.0).clamp(v.floor(), v.floor() + 0.9);
    Point {
        x: tenth(s.x),
        y,
        z: tenth(s.z),
        light: None,
        kerb: false,
    }
}

/// Puts in the steps between `a` and `b` where the ground leaves the line between them.
fn bend(steps: &[Step], y: &impl Fn(usize) -> f64, a: usize, b: usize, out: &mut Vec<Point>) {
    let line = |k: usize| y(a) + (y(b) - y(a)) * (k - a) as f64 / (b - a) as f64;
    let off = |k: usize| (y(k) - line(k) - SINK).max(line(k) - y(k) - FLOAT);
    let Some(k) = (a + 1..b).max_by(|&i, &j| off(i).total_cmp(&off(j))) else {
        return;
    };
    if off(k) <= 0.0 {
        return;
    }
    bend(steps, y, a, k, out);
    out.push(step_point(&steps[k], y(k)));
    bend(steps, y, k, b, out);
}

/// How many times busier than its kind a way is for the shops and sights around it.
struct Footfall {
    cell: f64,
    counts: FnvHashMap<(i32, i32), u32>,
}

impl Footfall {
    fn new(draws: &[(f64, f64)], cell: f64) -> Self {
        let cell = cell.max(8.0);
        let mut counts: FnvHashMap<(i32, i32), u32> = FnvHashMap::default();
        for &(x, z) in draws {
            *counts
                .entry(((x / cell).floor() as i32, (z / cell).floor() as i32))
                .or_default() += 1;
        }
        Self { cell, counts }
    }

    fn at(&self, (x, z): (f64, f64)) -> f64 {
        let (cx, cz) = (
            (x / self.cell).floor() as i32,
            (z / self.cell).floor() as i32,
        );
        let near: u32 = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| (cx + dx, cz + dz)))
            .filter_map(|c| self.counts.get(&c))
            .sum();
        1.0 + f64::from(near.min(30)) / 6.0
    }
}

/// Too big a network to hold: the busiest ways keep their walkers.
fn prune(lanes: &mut [Lane]) {
    let mut order: Vec<usize> = (0..lanes.len()).collect();
    order.sort_by(|&a, &b| lanes[b].density.total_cmp(&lanes[a].density));
    let mut kept = 0usize;
    for i in order {
        kept += lanes[i].points.len();
        if kept > MAX_POINTS {
            lanes[i].points.clear();
            lanes[i].alt.clear();
        }
    }
}

/// Where each lane leads on: every lane leaving its end but the way back, or the way
/// back at a dead end. A lane off the edge of the area leads nowhere: its walkers leave.
fn link(lanes: &mut [Lane], meta: &[Meta], scale: f64) {
    let usable = |l: &Lane| l.points.len() >= 2;
    // A lane cut short of its first node is not reached from there, only turned into.
    let mut starting: FnvHashMap<u64, Vec<usize>> = FnvHashMap::default();
    let mut pieces: FnvHashMap<usize, Vec<usize>> = FnvHashMap::default();
    for (i, m) in meta.iter().enumerate() {
        if usable(&lanes[i]) && !lanes[i].cut.0 {
            starting.entry(m.2).or_default().push(i);
        }
        pieces.entry(m.0).or_default().push(i);
    }
    // The lane of the same piece going the other way, as near as there is.
    let backs: Vec<Option<usize>> = (0..lanes.len())
        .map(|i| {
            let (piece, backward, ..) = meta[i];
            let end = lanes[i].points.last().map(|p| (p.x, p.z));
            pieces[&piece]
                .iter()
                .copied()
                .filter(|&j| meta[j].1 != backward && usable(&lanes[j]))
                .min_by(|&a, &b| {
                    let gap = |j: usize| {
                        let s = lanes[j].points[0];
                        end.map_or(0.0, |e| (s.x - e.0).hypot(s.z - e.1))
                    };
                    gap(a).total_cmp(&gap(b))
                })
        })
        .collect();
    let reach = (BESIDE_M * scale).max(2.0);
    let cell = |x: f64, z: f64| ((x / reach).floor() as i32, (z / reach).floor() as i32);
    let besides: Vec<Vec<Option<usize>>> = (0..lanes.len())
        .map(|i| {
            let Some(b) = backs[i] else {
                return Vec::new();
            };
            let back = &lanes[b].points;
            let mut grid: FnvHashMap<(i32, i32), Vec<usize>> = FnvHashMap::default();
            for (k, q) in back.iter().enumerate() {
                grid.entry(cell(q.x, q.z)).or_default().push(k);
            }
            lanes[i]
                .points
                .iter()
                .map(|p| {
                    let (cx, cz) = cell(p.x, p.z);
                    (-1..=1)
                        .flat_map(|dx| (-1..=1).map(move |dz| (cx + dx, cz + dz)))
                        .filter_map(|c| grid.get(&c))
                        .flatten()
                        .map(|&k| (k, (back[k].x - p.x).hypot(back[k].z - p.z)))
                        .filter(|&(_, d)| d <= reach)
                        .min_by(|a, c| a.1.total_cmp(&c.1))
                        .map(|(k, _)| k)
                })
                .collect()
        })
        .collect();
    for (i, beside) in besides.into_iter().enumerate() {
        let (piece, _, _, end, _) = meta[i];
        let mut next: Vec<usize> = starting
            .get(&end)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&j| meta[j].0 != piece)
            .collect();
        next.sort_unstable();
        lanes[i].back = backs[i];
        // Turned round where the way back is out of reach, walkers would cross whatever
        // is between: they leave there instead.
        let near = beside.last().is_some_and(Option::is_some);
        lanes[i].beside = beside;
        if lanes[i].out {
            next.clear();
        } else if next.is_empty() || lanes[i].cut.1 {
            next = backs[i].filter(|_| near).into_iter().collect();
        }
        lanes[i].next = next;
    }
}

/// Takes the ways into a few lanes walkers could never leave, as a stub cut short at both
/// ends that is only turned round in, so that nobody paces it for ever.
fn untrap(lanes: &mut [Lane], scale: f64) {
    let n = lanes.len();
    let length = |l: &Lane| {
        l.points
            .windows(2)
            .map(|w| (w[1].x - w[0].x).hypot(w[1].z - w[0].z))
            .sum::<f64>()
    };
    loop {
        // Kosaraju: finish order along the links, then groups along them reversed.
        let mut order = Vec::with_capacity(n);
        let mut seen = vec![false; n];
        for start in 0..n {
            if seen[start] {
                continue;
            }
            seen[start] = true;
            let mut stack = vec![(start, 0usize)];
            while let Some((i, k)) = stack.pop() {
                if let Some(&j) = lanes[i].next.get(k) {
                    stack.push((i, k + 1));
                    if !seen[j] {
                        seen[j] = true;
                        stack.push((j, 0));
                    }
                } else {
                    order.push(i);
                }
            }
        }
        let mut into: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, lane) in lanes.iter().enumerate() {
            for &j in &lane.next {
                into[j].push(i);
            }
        }
        let mut group = vec![usize::MAX; n];
        let mut groups = 0;
        for &start in order.iter().rev() {
            if group[start] != usize::MAX {
                continue;
            }
            group[start] = groups;
            let mut stack = vec![start];
            while let Some(i) = stack.pop() {
                for &j in &into[i] {
                    if group[j] == usize::MAX {
                        group[j] = groups;
                        stack.push(j);
                    }
                }
            }
            groups += 1;
        }
        // A group is closed when nothing leads out of it, not even off the area.
        let mut open = vec![false; groups];
        let mut size = vec![0.0; groups];
        for (i, lane) in lanes.iter().enumerate() {
            let g = group[i];
            size[g] += length(lane);
            if lane.out || lane.next.is_empty() || lane.next.iter().any(|&j| group[j] != g) {
                open[g] = true;
            }
        }
        // Entered from a bigger network it is a stub; alone, or bigger than whatever leads
        // in, it is a network of its own.
        let trap = |from: usize, g: usize| {
            from != g && !open[g] && (size[g] < 2.0 * SCRAP_M * scale || size[g] < size[from])
        };
        let mut changed = false;
        for i in 0..n {
            let g = group[i];
            let before = lanes[i].next.len();
            lanes[i].next.retain(|&j| !trap(g, group[j]));
            if lanes[i].next.len() != before {
                changed = true;
                // Turned round only where the way back is in reach, as `link` does.
                let near = lanes[i].beside.last().is_some_and(Option::is_some);
                if lanes[i].next.is_empty() && near {
                    lanes[i].next = lanes[i]
                        .back
                        .filter(|&b| !trap(g, group[b]))
                        .into_iter()
                        .collect();
                }
            }
        }
        if !changed {
            return;
        }
    }
}

/// Where walkers can join: every lane's spots as far apart as its people are on a busy
/// day, each at a point of the lane, leaving out networks too small to walk.
fn spots(lanes: &[Lane], meta: &[Meta], scale: f64) -> BTreeMap<(i32, i32), Vec<(usize, usize)>> {
    let length = |l: &Lane| {
        l.points
            .windows(2)
            .map(|w| (w[1].x - w[0].x).hypot(w[1].z - w[0].z))
            .sum::<f64>()
    };
    // Lanes joined through their junctions, and the length each network adds up to.
    let mut root: Vec<usize> = (0..lanes.len()).collect();
    fn find(root: &mut [usize], mut k: usize) -> usize {
        while root[k] != k {
            root[k] = root[root[k]];
            k = root[k];
        }
        k
    }
    for (i, lane) in lanes.iter().enumerate() {
        for &j in &lane.next {
            let (a, b) = (find(&mut root, i), find(&mut root, j));
            root[a] = b;
        }
    }
    let mut total: FnvHashMap<usize, f64> = FnvHashMap::default();
    for (i, lane) in lanes.iter().enumerate() {
        *total.entry(find(&mut root, i)).or_default() += length(lane);
    }
    let mut grid: BTreeMap<(i32, i32), Vec<(usize, usize)>> = BTreeMap::new();
    let mut owed = 0.5;
    for (i, lane) in lanes.iter().enumerate() {
        // Crossings are short and walked across, never started on.
        if lane.points.len() < 2 || lane.next.is_empty() || meta[i].4 {
            continue;
        }
        if total[&find(&mut root, i)] < 2.0 * SCRAP_M * scale {
            continue;
        }
        let len = length(lane);
        owed += len / scale * lane.density;
        let n = owed.floor() as usize;
        owed -= n as f64;
        let mut at = 0.0;
        let mut k = 0;
        for j in 0..n {
            let s = len * (j as f64 + 0.5) / n as f64;
            while k + 2 < lane.points.len() {
                let (p, q) = (lane.points[k], lane.points[k + 1]);
                let seg = (q.x - p.x).hypot(q.z - p.z);
                if at + seg >= s {
                    break;
                }
                at += seg;
                k += 1;
            }
            // A busy lane's point may hold several spots: more chances to start there.
            if lane.points[k].light.is_some() || lane.points[k].kerb {
                continue;
            }
            let p = lane.points[k];
            let cell = (
                (p.x / SPOT_CELL).floor() as i32,
                (p.z / SPOT_CELL).floor() as i32,
            );
            grid.entry(cell).or_default().push((i, k));
        }
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u64, x: i32, z: i32, tags: &[(&str, &str)]) -> ProcessedNode {
        ProcessedNode {
            id,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            x,
            z,
        }
    }

    fn way(id: u64, nodes: Vec<ProcessedNode>, tags: &[(&str, &str)]) -> ProcessedElement {
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    fn world() -> XZBBox {
        XZBBox::rect_from_min_max(-2000, -2000, 2000, 2000).unwrap()
    }

    fn build(elements: &[ProcessedElement], signals: bool) -> WalkNetwork {
        WalkNetwork::build(elements, 1.0, false, signals, &world(), |_, _, _| 64)
    }

    #[test]
    fn who_walks_where() {
        let tags = |kv: &[(&str, &str)]| -> HashMap<String, String> {
            kv.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let busy = walk_density(&tags(&[("highway", "pedestrian")])).unwrap();
        let sidewalk =
            walk_density(&tags(&[("highway", "footway"), ("footway", "sidewalk")])).unwrap();
        let trail = walk_density(&tags(&[("highway", "path")])).unwrap();
        assert!(busy > sidewalk && sidewalk > trail);
        for none in [
            &[("highway", "residential")][..],
            &[("highway", "footway"), ("indoor", "yes")],
            &[("highway", "footway"), ("tunnel", "yes")],
            &[("highway", "footway"), ("layer", "-1")],
            &[("highway", "footway"), ("level", "1")],
            &[("highway", "footway"), ("access", "private")],
            &[("highway", "steps"), ("conveying", "yes")],
            &[("highway", "cycleway")],
            &[("highway", "footway"), ("area", "yes")],
        ] {
            assert!(walk_density(&tags(none)).is_none(), "{none:?}");
        }
        assert!(walk_density(&tags(&[
            ("highway", "footway"),
            ("access", "no"),
            ("foot", "yes")
        ]))
        .is_some());
        assert!(walk_density(&tags(&[("highway", "footway"), ("layer", "1")])).is_some());
    }

    #[test]
    fn walkers_keep_right_each_way_and_turn_at_a_junction() {
        let elements = vec![
            way(
                1,
                vec![node(1, 0, 0, &[]), node(2, 100, 0, &[])],
                &[("highway", "footway")],
            ),
            way(
                2,
                vec![node(2, 100, 0, &[]), node(3, 100, 100, &[])],
                &[("highway", "footway")],
            ),
        ];
        let net = build(&elements, false);
        assert_eq!(net.lane_count(), 4);
        // Eastbound runs south of the middle, as the right of east is south (+z).
        let east = &net.lanes[0];
        assert!(east.points.iter().all(|p| p.z > 0.5), "{:?}", east.points);
        let west = &net.lanes[1];
        assert!(west.points.iter().all(|p| p.z < 0.5));
        // East leads on south, never straight back; at the dead end west turns round.
        assert_eq!(east.next, vec![2]);
        assert_eq!(west.next, vec![0]);
        assert_eq!(east.back, Some(1));
        // Points stay within a span of each other.
        for l in &net.lanes {
            for w in l.points.windows(2) {
                assert!((w[1].x - w[0].x).hypot(w[1].z - w[0].z) <= MAX_SPAN + 1e-6);
            }
        }
    }

    #[test]
    fn lanes_go_round_posts_and_walls_onto_the_ground_as_built() {
        use crate::water_probe::{footing, Spot};
        let elements = vec![
            way(
                1,
                vec![node(1, 0, 0, &[]), node(2, 100, 0, &[])],
                &[("highway", "footway")],
            ),
            way(
                2,
                vec![node(2, 100, 0, &[]), node(3, 100, 100, &[])],
                &[("highway", "footway")],
            ),
        ];
        let mut net = build(&elements, false);
        // A step up at x 70, a wall along the eastbound side, a lamp and a slab on the
        // westbound one and a house right across the second way.
        let block = |x: i32, z: i32, y: i32| {
            let top = if x >= 70 { 65 } else { 64 };
            let wall = (40..=45).contains(&x) && z == 1
                || (95..=105).contains(&x) && (50..=52).contains(&z);
            match (x, z, y) {
                _ if y <= top || wall && y <= 70 => Spot::Solid,
                (20, -1, 65) => Spot::Solid,
                (20, -1, 66..=68) => Spot::Tall,
                (30, -1, 65) => Spot::Half,
                _ => Spot::Open,
            }
        };
        let probe = net.probe();
        for (rx, rz) in probe.regions().collect::<Vec<_>>() {
            probe.read_region(rx, rz, |x, z, level| footing(|y| block(x, z, y), level));
        }
        net.finish(&probe);
        let (east, west) = (&net.lanes[0], &net.lanes[1]);
        assert!(east.points.iter().all(|p| p.z < 1.0), "{:?}", east.points);
        assert!(west.points.iter().all(|p| p.z < 1.0), "{:?}", west.points);
        // The westbound lane steps round the lamp, back on its side either way of it.
        let passing = Measured::new(west.points.iter().map(|p| (p.x, p.z)).collect());
        for k in 0..=1000 {
            let (x, z) = passing.point_at(passing.length() * f64::from(k) / 1000.0);
            if (20.1..=20.9).contains(&x) {
                assert!(z >= -0.5 + 0.4 + CLEAR - 1e-9, "{x} {z}");
            }
            if !(17.0..24.0).contains(&x) {
                assert!(z < 0.0, "{x} {z}");
            }
        }
        // Walking each lane, nobody sinks into the step or the slab, nor floats far.
        let feet = |x: f64, z: f64| match (x.floor() as i32, z.floor() as i32) {
            (30, -1) => 65.5,
            (x, _) if x >= 70 => 66.0,
            _ => 65.0,
        };
        for lane in [east, west] {
            for w in lane.points.windows(2) {
                for t in 0..=20 {
                    let t = f64::from(t) / 20.0;
                    let (x, z) = (
                        w[0].x + (w[1].x - w[0].x) * t,
                        w[0].z + (w[1].z - w[0].z) * t,
                    );
                    let off = w[0].y + (w[1].y - w[0].y) * t - feet(x, z);
                    assert!((-SINK - 1e-9..=1.0).contains(&off), "{w:?} {t} {off}");
                }
            }
        }
        // North of the house the second way is cut off: its walkers turn round before it.
        let (north, south) = (&net.lanes[2], &net.lanes[3]);
        assert!(
            north.points.iter().all(|p| p.z < 50.0),
            "{:?}",
            north.points
        );
        assert!(
            south.points.iter().all(|p| p.z < 50.0),
            "{:?}",
            south.points
        );
        assert_eq!((north.cut, south.cut), ((false, true), (true, false)));
        assert_eq!(north.next, vec![3]);
        assert_eq!(south.next, vec![1]);
        // Turned round, a walker carries on from beside where it stopped.
        let (stop, on) = (
            north.points[north.points.len() - 1],
            north.beside[north.points.len() - 1].unwrap(),
        );
        assert!(
            (south.points[on].z - stop.z).abs() < 3.0,
            "{stop:?} {:?}",
            south.points[on]
        );
        assert_eq!(east.next, vec![2]);
        assert!(!net.is_empty());
    }

    #[test]
    fn living_streets_have_no_kerbs_and_a_way_ending_on_a_road_stops_at_its_kerb() {
        let elements = vec![
            way(
                1,
                vec![
                    node(1, 0, 0, &[]),
                    node(2, 50, 0, &[]),
                    node(3, 100, 0, &[]),
                ],
                &[("highway", "living_street")],
            ),
            way(
                2,
                vec![
                    node(10, -50, 100, &[]),
                    node(11, 0, 100, &[]),
                    node(12, 50, 100, &[]),
                ],
                &[("highway", "residential")],
            ),
            way(
                3,
                vec![node(20, 0, 160, &[]), node(11, 0, 100, &[])],
                &[("highway", "footway")],
            ),
        ];
        let net = build(&elements, false);
        let (street, path): (Vec<&Lane>, Vec<&Lane>) = net
            .lanes
            .iter()
            .filter(|l| l.points.len() >= 2)
            .partition(|l| l.points[0].z < 50.0);
        assert!(!street.is_empty() && !path.is_empty());
        assert!(street
            .iter()
            .all(|l| l.points.iter().all(|p| !p.kerb && p.light.is_none())));
        for lane in &path {
            assert!(
                lane.points.iter().all(|p| p.z > 100.5 + KERB_BACK),
                "{:?}",
                lane.points
            );
            assert_eq!(lane.next.len(), 1, "turns round at the kerb");
        }
    }

    #[test]
    fn a_crossing_split_at_the_road_waits_only_at_the_kerbs() {
        let crossing = &[("highway", "footway"), ("footway", "crossing")];
        let elements = vec![
            way(
                1,
                vec![
                    node(1, -50, 0, &[]),
                    node(2, 0, 0, &[]),
                    node(3, 50, 0, &[]),
                ],
                &[("highway", "residential")],
            ),
            way(2, vec![node(10, 0, -10, &[]), node(2, 0, 0, &[])], crossing),
            way(3, vec![node(2, 0, 0, &[]), node(11, 0, 10, &[])], crossing),
        ];
        let net = build(&elements, false);
        let usable: Vec<&Lane> = net.lanes.iter().filter(|l| l.points.len() >= 2).collect();
        assert!(!usable.is_empty());
        for lane in usable {
            let first = lane.points[0];
            let kerb = first.kerb || first.light.is_some();
            // On the road's middle nobody stops; at either kerb they do.
            assert_eq!(kerb, first.z.abs() > 5.0, "{:?}", lane.points);
        }
    }

    #[test]
    fn a_crossing_waits_at_the_kerb_and_at_a_light_for_its_own_green() {
        let elements = vec![
            way(
                9,
                vec![node(20, 50, -30, &[]), node(21, 50, 30, &[])],
                &[("highway", "residential")],
            ),
            way(
                1,
                vec![node(1, 0, 0, &[]), node(2, 44, 0, &[])],
                &[("highway", "footway")],
            ),
            way(
                2,
                vec![node(2, 44, 0, &[]), node(3, 56, 0, &[])],
                &[
                    ("highway", "footway"),
                    ("footway", "crossing"),
                    ("crossing", "traffic_signals"),
                ],
            ),
            way(
                3,
                vec![node(3, 56, 0, &[]), node(4, 120, 0, &[])],
                &[("highway", "footway")],
            ),
        ];
        let lit = build(&elements, true);
        let crossing = &lit.lanes[2];
        // Walking east crosses a north-south road, alongside east-west traffic.
        assert_eq!(crossing.points[0].light, Some(1));
        let plain = build(&elements, false);
        assert!(plain.lanes[2].points[0].kerb);
        assert_eq!(plain.lanes[2].points[0].light, None);
        // A footway over a road at a node waits short of the road.
        let over = vec![
            way(
                9,
                vec![
                    node(20, 50, -30, &[]),
                    node(22, 50, 0, &[]),
                    node(21, 50, 30, &[]),
                ],
                &[("highway", "residential")],
            ),
            way(
                5,
                vec![
                    node(1, 0, 0, &[]),
                    node(22, 50, 0, &[("crossing", "traffic_signals")]),
                    node(4, 120, 0, &[]),
                ],
                &[("highway", "footway")],
            ),
        ];
        let net = build(&over, true);
        let stop = net.lanes[0]
            .points
            .iter()
            .find(|p| p.light.is_some())
            .unwrap();
        assert!(stop.x < 48.0 && stop.x > 44.0, "{stop:?}");
    }

    #[test]
    fn busy_streets_get_more_walkers_than_quiet_paths_and_scraps_none() {
        let mut elements = vec![
            way(
                1,
                vec![node(1, 0, 0, &[]), node(2, 400, 0, &[])],
                &[("highway", "pedestrian")],
            ),
            way(
                2,
                vec![node(3, 0, 500, &[]), node(4, 400, 500, &[])],
                &[("highway", "path")],
            ),
            way(
                3,
                vec![node(5, 0, 900, &[]), node(6, 10, 900, &[])],
                &[("highway", "footway")],
            ),
        ];
        let net = build(&elements, false);
        let count = |net: &WalkNetwork, z: i32| -> usize {
            net.spots
                .iter()
                .filter(|((_, cz), _)| (*cz * SPOT_CELL as i32 - z).abs() < 40)
                .map(|(_, s)| s.len())
                .sum()
        };
        let (street, path) = (count(&net, 0), count(&net, 500));
        assert!(street > 10 * path.max(1), "{street} {path}");
        assert_eq!(count(&net, 900), 0);
        // Shops along the street bring more people still.
        for k in 0..20 {
            elements.push(ProcessedElement::Node(node(
                100 + k,
                20 * k as i32,
                8,
                &[("shop", "clothes")],
            )));
        }
        assert!(count(&build(&elements, false), 0) > street * 2);
    }

    #[test]
    fn nobody_is_led_into_a_stub_they_can_never_leave() {
        let lane = |len: f64, next: Vec<usize>, back: usize| Lane {
            points: [0.0, len]
                .map(|x| Point {
                    x,
                    y: 64.0,
                    z: 0.0,
                    light: None,
                    kerb: false,
                })
                .to_vec(),
            alt: Vec::new(),
            plain: Vec::new(),
            next,
            back: Some(back),
            cut: (false, true),
            beside: vec![Some(0); 2],
            out: false,
            density: 1.0,
            across: false,
        };
        // A long way both ways (0, 1), a lane off it (4, 5), and a stub cut at both ends
        // (2, 3) that 1 and 4 lead into but that only ever turns round into itself.
        let mut lanes = vec![
            lane(100.0, vec![1], 1),
            lane(100.0, vec![0, 2, 4], 0),
            lane(4.0, vec![3], 3),
            lane(4.0, vec![2], 2),
            lane(50.0, vec![2], 5),
            lane(50.0, vec![0], 4),
        ];
        untrap(&mut lanes, 1.0);
        assert_eq!(lanes[1].next, vec![0, 4]);
        assert_eq!(lanes[4].next, vec![5]);
        assert_eq!(lanes[2].next, vec![3]);
        // A longer stub off a bigger network is a trap too; a network bigger than the way
        // into it is one of its own.
        let mut lanes = vec![
            lane(100.0, vec![1], 1),
            lane(100.0, vec![0, 2], 0),
            lane(80.0, vec![3], 3),
            lane(80.0, vec![2], 2),
        ];
        untrap(&mut lanes, 1.0);
        assert_eq!(lanes[1].next, vec![0]);
        let mut lanes = vec![
            lane(40.0, vec![1], 1),
            lane(40.0, vec![0, 2], 0),
            lane(80.0, vec![3], 3),
            lane(80.0, vec![2], 2),
        ];
        untrap(&mut lanes, 1.0);
        assert_eq!(lanes[1].next, vec![0, 2]);
    }

    #[test]
    fn paces_spread_round_an_unhurried_walk() {
        let net = build(&[], false);
        let paces = net.paces();
        assert!(paces.windows(2).all(|w| w[0] < w[1]), "{paces:?}");
        // Weighted 1, 2, 3, 4, 3, 2, 1 as the pack draws them.
        let mean = paces
            .iter()
            .zip([1, 2, 3, 4, 3, 2, 1])
            .map(|(p, w)| p * w)
            .sum::<i32>() as f64
            / 16.0;
        assert!((mean - WALK_MPS * 100.0).abs() < 1.0, "{mean}");
        assert!(paces[0] >= 100 && paces[6] <= 200, "{paces:?}");
    }

    /// A 40 block square with a way in at the middle of each side, and the lanes across it.
    fn square(extra: Vec<ProcessedElement>) -> (Vec<ProcessedElement>, WalkNetwork) {
        let n = |id: u64, x: i32, z: i32| node(id, x, z, &[]);
        let ring = vec![
            n(1, 100, 100),
            n(11, 120, 100),
            n(2, 140, 100),
            n(12, 140, 120),
            n(3, 140, 140),
            n(13, 120, 140),
            n(4, 100, 140),
            n(14, 100, 120),
            n(1, 100, 100),
        ];
        let mut elements = vec![way(50, ring, &[("highway", "pedestrian"), ("area", "yes")])];
        for (id, door, x, z) in [
            (61, 11, 120, 60),
            (62, 12, 180, 120),
            (63, 13, 120, 180),
            (64, 14, 60, 120),
        ] {
            let at = match door {
                11 => (120, 100),
                12 => (140, 120),
                13 => (120, 140),
                _ => (100, 120),
            };
            elements.push(way(
                id,
                vec![n(id + 100, x, z), n(door, at.0, at.1)],
                &[("highway", "footway")],
            ));
        }
        elements.extend(extra);
        let net = build(&elements, false);
        (elements, net)
    }

    fn across(net: &WalkNetwork) -> Vec<&Lane> {
        net.lanes
            .iter()
            .filter(|l| l.across && l.points.len() >= 2)
            .collect()
    }

    #[test]
    fn people_cut_across_squares_between_the_ways_in() {
        let (_, mut net) = square(Vec::new());
        // Each way in to the three others, a lane each way.
        assert_eq!(across(&net).len(), 12);
        let into_north = net
            .lanes
            .iter()
            .position(|l| !l.across && l.points.last().is_some_and(|p| p.z < 101.0))
            .unwrap();
        assert!(net.lanes[into_north]
            .next
            .iter()
            .all(|&j| net.lanes[j].across));
        assert_eq!(net.lanes[into_north].next.len(), 3);
        // A statue in the middle takes the straight lines over; the diagonals stay.
        use crate::water_probe::{footing, Spot};
        let probe = net.probe();
        for (rx, rz) in probe.regions().collect::<Vec<_>>() {
            probe.read_region(rx, rz, |x, z, level| {
                let statue = (116..=124).contains(&x) && (116..=124).contains(&z);
                footing(
                    |y| match y {
                        _ if y <= 64 || statue && y <= 70 => Spot::Solid,
                        _ => Spot::Open,
                    },
                    level,
                )
            });
        }
        net.finish(&probe);
        let left = across(&net);
        assert_eq!(left.len(), 8);
        for l in left {
            assert!(l
                .points
                .iter()
                .all(|p| p.x < 115.0 || p.x > 125.0 || p.z < 115.0 || p.z > 125.0));
        }
    }

    #[test]
    fn lines_across_a_square_cross_no_road_and_no_hole() {
        let road = way(
            70,
            vec![node(71, 50, 125, &[]), node(72, 190, 125, &[])],
            &[("highway", "residential")],
        );
        let (_, net) = square(vec![road]);
        let lanes = across(&net);
        // North to east, north to west and east to west, never over the road to the south.
        assert_eq!(lanes.len(), 6);
        assert!(lanes.iter().all(|l| l.points.iter().all(|p| p.z < 123.0)));

        let (mut elements, _) = square(Vec::new());
        let ProcessedElement::Way(outline) = elements.remove(0) else {
            unreachable!()
        };
        let hole: Vec<ProcessedNode> = [(112, 112), (128, 112), (128, 128), (112, 128), (112, 112)]
            .iter()
            .enumerate()
            .map(|(k, &(x, z))| node(80 + (k as u64 % 4), x, z, &[]))
            .collect();
        let member =
            |id: u64, nodes: Vec<ProcessedNode>, role| crate::osm_parser::ProcessedMember {
                role,
                way: std::sync::Arc::new(ProcessedWay {
                    id,
                    nodes,
                    tags: HashMap::new(),
                }),
            };
        elements.push(ProcessedElement::Relation(
            crate::osm_parser::ProcessedRelation {
                id: 90,
                tags: [("type", "multipolygon"), ("highway", "pedestrian")]
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .into(),
                members: vec![
                    member(91, outline.nodes, ProcessedMemberRole::Outer),
                    member(92, hole, ProcessedMemberRole::Inner),
                ],
            },
        ));
        let net = build(&elements, false);
        // Only the diagonals go round the hole.
        assert_eq!(across(&net).len(), 8);
    }

    #[test]
    fn a_post_is_stepped_round_only_onto_ground_read_round_it() {
        use crate::water_probe::{footing, Spot};
        let walled = |wall: bool| {
            let elements = vec![way(
                1,
                vec![node(1, 0, 0, &[]), node(2, 60, 0, &[])],
                &[("highway", "footway")],
            )];
            let mut net = build(&elements, false);
            // A lamp on the eastbound lane, and maybe a wall beside it where it would step.
            let block = |x: i32, z: i32, y: i32| match (x, z) {
                _ if y <= 64 => Spot::Solid,
                (30, 1) if y <= 67 => Spot::Tall,
                (27..=33, 0) if wall && y <= 70 => Spot::Solid,
                _ => Spot::Open,
            };
            let probe = net.probe();
            for (rx, rz) in probe.regions().collect::<Vec<_>>() {
                probe.read_region_round(
                    rx,
                    rz,
                    |x, z, level| footing(|y| block(x, z, y), level),
                    |f| f.post(),
                );
            }
            net.finish(&probe);
            net.lanes[0].points.clone()
        };
        let east = walled(false);
        assert!(
            east.iter()
                .any(|p| (29.0..32.0).contains(&p.x) && p.z < 1.0),
            "{east:?}"
        );
        let east = walled(true);
        assert!(east.iter().all(|p| p.z > 1.0), "{east:?}");
        assert_eq!(east.first().unwrap().x, 0.5);
        assert_eq!(east.last().unwrap().x, 60.5);
    }

    #[test]
    fn lanes_step_round_bins_benches_and_trees_but_keep_their_ends() {
        let line = |z: f64| -> Vec<Point> {
            [0.5, 40.5]
                .map(|x| Point {
                    x,
                    y: 64.0,
                    z,
                    light: None,
                    kerb: true,
                })
                .to_vec()
        };
        let item = |x: i32, z: i32, tags: &[(&str, &str)]| furniture(&node(1, x, z, tags)).unwrap();
        let bin = item(10, 0, &[("amenity", "waste_basket")]);
        let bench = item(20, 1, &[("amenity", "bench")]);
        let tree = item(30, -2, &[("natural", "tree")]);
        let street = Furniture::new(vec![bin, bench, tree]);
        let lane = street.go_round(line(0.9));
        assert_eq!(lane.first().unwrap().x, 0.5);
        assert_eq!(lane.last().unwrap().x, 40.5);
        assert!(lane.first().unwrap().kerb && lane.last().unwrap().kerb);
        // Nowhere along it within reach of the bin or the bench.
        let measured = Measured::new(lane.iter().map(|p| (p.x, p.z)).collect());
        for k in 0..=400 {
            let (x, z) = measured.point_at(measured.length() * k as f64 / 400.0);
            for o in [bin, bench] {
                let (gx, gz) = ((x - o.x).abs() - o.hx - o.long, (z - o.z).abs() - o.hz);
                assert!(gx > 0.0 || gz >= CLEAR, "{x} {z} near {} {}", o.x, o.z);
            }
            // Straight on where nothing stands, and the tree well off it is passed as is.
            if !(6.0..34.0).contains(&x) || (27.0..34.0).contains(&x) {
                assert!((z - 0.9).abs() < 1e-9, "{x} {z}");
            }
        }
        // Clear of the bin on the side it is on, never more than a step aside.
        assert!(lane.iter().all(|p| (p.z - 0.9).abs() <= DETOUR_MAX + 1e-9));
        assert!(lane.iter().any(|p| p.z > 1.3));
        // Something filling the way is left for the probe.
        let wide = Furniture::new(vec![item(10, 10, &[("advertising", "poster_box")])]);
        let along_z = line(11.0)
            .into_iter()
            .map(|p| Point {
                x: 11.0,
                z: p.x,
                ..p
            })
            .collect();
        assert_eq!(wide.go_round(along_z).len(), 2);
        // A lamp past a corner, two blocks off the lane, leaves it be.
        let corner: Vec<Point> = [(0.5, 0.0), (20.5, 0.0), (20.5, -20.0)]
            .map(|(x, z)| Point {
                x,
                y: 64.0,
                z,
                light: None,
                kerb: false,
            })
            .to_vec();
        let lamp = Furniture::new(vec![item(22, 0, &[("highway", "street_lamp")])]);
        assert_eq!(lamp.go_round(corner.clone()), corner);
        assert!(furniture(&node(2, 0, 0, &[("amenity", "post_box")])).is_none());
        assert!(furniture(&node(
            3,
            0,
            0,
            &[
                ("emergency", "fire_hydrant"),
                ("fire_hydrant:type", "underground")
            ]
        ))
        .is_none());
    }

    #[test]
    fn the_pack_functions_hold_every_lane_and_move_at_each_pace() {
        let elements = vec![way(
            1,
            vec![node(1, 0, 0, &[]), node(2, 300, 0, &[])],
            &[("highway", "footway"), ("footway", "sidewalk")],
        )];
        let net = build(&elements, false);
        let functions = net.pack_functions();
        let text = |name: &str| &functions.iter().find(|(p, _)| p.ends_with(name)).unwrap().1;
        let network = text("walk/network.mcfunction");
        assert_eq!(network.matches("{p:[").count(), 2);
        assert!(network.contains("grid merge value"));
        assert_eq!(text("walk/move.mcfunction").lines().count(), 7);
        assert!(text("walk/aside.mcfunction").contains("tp @s ^-0.40 ^ ^"));
        for (_, body) in &functions {
            for line in body.lines().filter(|l| !l.starts_with('#')) {
                assert!(!line.contains("$("), "{line}");
            }
        }
    }
}
