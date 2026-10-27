//! Cars that drive the mapped streets, run by the extras data pack. Lanes, the turns at
//! each junction, speed limits and the stop lines at signals are worked out here and
//! written into the pack as storage; each car is a marker that holds its place on that
//! network, carried along with the block displays that draw it. The tags, the storage
//! layout and the car data are the contract with
//! `assets/minecraft/datapack_extras/data/arnis/function/cars`.
//!
//! Speeds are in thousandths of a block per step, a step being the two ticks between
//! moves, so 1000 is 36 km/h on a 1:1 world.

use crate::clipping::is_invented_node_id;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::datapack::macro_coord;
use crate::element_processing::highways::highway_block_range;
use crate::element_processing::road_markings::oneway_sign;
use crate::element_processing::traffic_lights::{
    group_of, is_signal, is_vehicular, signal_upstream, UPSTREAM_M,
};
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use fnv::{FnvHashMap, FnvHashSet};
use std::collections::HashMap;
use std::fmt::Write;

/// Longest stretch between two waypoints; shorter where the ground bends.
const MAX_SPAN: f64 = 24.0;
/// The world's cars, spread over all of its lanes. Only those near a player move, so the
/// cap is about what the world files hold, not what a tick costs.
const MAX_CARS: usize = 1500;
/// Lanes the pack stores per command.
const LANES_PER_LINE: usize = 256;
/// How far back from a mid-road signal the car stops.
const STOP_BACK: f64 = 1.5;
/// Speed across a junction, and over the last stretch of a lane into one.
const JUNCTION_KMH: f64 = 25.0;
const LANE_END_KMH: f64 = 32.0;
const LANE_END_REACH: f64 = 10.0;
/// Turning round at a dead end.
const U_TURN_KMH: f64 = 12.0;
/// Tightest circle a car turns round on at a dead end, in metres. Where the lanes lie
/// closer than that it swings out first, so it never spins on the spot.
const U_TURN_RADIUS_M: f64 = 3.0;
/// Dead ends shorter than this, in metres, are stubs past a junction or the mouth of a
/// driveway: no one turns into one only to turn round while there is anywhere else to go.
const STUB_M: f64 = 25.0;
/// Roads cut off from the rest that add up to less than this, in metres, carry no cars.
const SCRAP_M: f64 = 100.0;
/// A lane ending this close to the edge of the area runs off it, onto roads outside.
const EDGE_REACH: f64 = 3.0;
/// Most a car turns in all onto a lane's entry point and on along the lane, beyond which
/// it would be turning round on the spot.
const ENTRY_TURN_DEG: f64 = 135.0;
/// A turn past this into a lane's only way on is driven along a curve.
const SHARP_DEG: f64 = 100.0;
/// Lanes shorter than this, in blocks, are crossed as part of their junction.
const SHORT_LANE: f64 = 3.0;
/// Furthest a lane's entry point lies across a junction, in blocks, as for a left turn
/// over a boulevard: beyond it the car is back in a queue.
const CROSSING_REACH: f64 = 40.0;
/// Drivers keep a little under the limit.
const LIMIT_SHARE: f64 = 0.92;
/// 2.5 m/s², a calm start from the lights.
const ACCEL_MPS2: f64 = 2.5;

/// Thousandths of a block per two-tick step at `kmh` on a world of `scale` blocks per metre.
fn step_speed(kmh: f64, scale: f64) -> i32 {
    (kmh / 3.6 * 0.1 * scale * 1000.0).round() as i32
}

/// The posted limit, or what roads of the class usually allow.
fn limit_kmh(highway: &str, tags: &HashMap<String, String>) -> f64 {
    // Junk like `inf` or `-5 mph` falls back to the class rather than overflow the speeds.
    let posted = tags
        .get("maxspeed")
        .and_then(|m| parse_maxspeed(m))
        .filter(|k| (1.0..=300.0).contains(k));
    posted.unwrap_or(match highway {
        "motorway" => 110.0,
        "trunk" => 80.0,
        "motorway_link" | "trunk_link" => 60.0,
        "primary" | "secondary" | "tertiary" => 50.0,
        "primary_link" | "secondary_link" | "tertiary_link" | "unclassified" | "road" => 40.0,
        "residential" => 30.0,
        "service" => 20.0,
        "living_street" => 10.0,
        _ => 30.0,
    })
}

/// `maxspeed` in km/h: a number, a number in mph, or an implicit limit like `DE:urban`.
fn parse_maxspeed(raw: &str) -> Option<f64> {
    let v = raw.trim().to_ascii_lowercase();
    if let Some(mph) = v.strip_suffix("mph") {
        return mph.trim().parse::<f64>().ok().map(|m| m * 1.609);
    }
    if let Ok(kmh) = v.parse::<f64>() {
        return (kmh > 0.0).then_some(kmh);
    }
    let zone: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
    if v.contains("zone") && !zone.is_empty() {
        return zone.parse::<f64>().ok();
    }
    match v.rsplit(':').next()? {
        "walk" | "living_street" => Some(10.0),
        "urban" => Some(50.0),
        "rural" => Some(90.0),
        "trunk" => Some(100.0),
        "motorway" => Some(120.0),
        // An unlimited autobahn: what the fast lane does.
        "none" => Some(130.0),
        _ => None,
    }
}

/// Cars per metre of lane in daytime traffic: flow over speed, so a main road at
/// 800 vehicles an hour and 30 km/h holds about 25 per kilometre and direction, and a
/// side street at 100 an hour about 5.
fn density(highway: &str) -> f64 {
    match highway {
        // One simulated lane, kept about two seconds apart at speed.
        "motorway" | "trunk" | "motorway_link" | "trunk_link" => 1.0 / 60.0,
        "primary" | "secondary" | "primary_link" | "secondary_link" => 1.0 / 40.0,
        "tertiary" | "tertiary_link" => 1.0 / 60.0,
        "unclassified" | "road" => 1.0 / 150.0,
        "residential" => 1.0 / 200.0,
        "service" => 1.0 / 400.0,
        _ => 1.0 / 500.0,
    }
}

fn drivable(tags: &HashMap<String, String>) -> bool {
    let get = |k: &str| tags.get(k).map(String::as_str);
    let Some(highway) = get("highway") else {
        return false;
    };
    // Underground and stacked roads are not where the ground says they are.
    let bridge = matches!(get("bridge"), Some(v) if v != "no");
    let off_grade = matches!(get("tunnel"), Some(v) if v != "no")
        || get("layer")
            .and_then(|l| l.parse::<i32>().ok())
            .is_some_and(|l| l != 0 && !bridge);
    let closed = |k: &str| matches!(get(k), Some("no" | "private"));
    is_vehicular(highway)
        && highway != "busway"
        && !off_grade
        && get("area") != Some("yes")
        && get("indoor") != Some("yes")
        && !closed("access")
        && !closed("vehicle")
        && !closed("motor_vehicle")
        && !closed("motorcar")
        && !matches!(get("service"), Some("parking_aisle" | "driveway"))
}

fn unit(dx: f64, dz: f64) -> Option<(f64, f64)> {
    let len = dx.hypot(dz);
    (len > 1e-9).then(|| (dx / len, dz / len))
}

/// A polyline measured along its length.
struct Path {
    points: Vec<(f64, f64)>,
    /// Distance along the path at each point.
    at: Vec<f64>,
}

impl Path {
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

    /// How far the path turns at point `k`, in degrees.
    fn bend(&self, k: usize) -> f64 {
        if k == 0 || k + 1 >= self.points.len() {
            return 0.0;
        }
        let (p, q, r) = (self.points[k - 1], self.points[k], self.points[k + 1]);
        match (unit(q.0 - p.0, q.1 - p.1), unit(r.0 - q.0, r.1 - q.1)) {
            (Some(a), Some(b)) => (a.0 * b.0 + a.1 * b.1).clamp(-1.0, 1.0).acos().to_degrees(),
            _ => 0.0,
        }
    }
}

/// Shortest step between nodes that gives a lane its heading, in blocks.
const MIN_LEG: f64 = 2.0;

/// The lane's centre line: the node path pushed `offset` to the driving side.
fn lane_line(nodes: &[&ProcessedNode], offset: f64, drives_on_left: bool) -> Vec<(f64, f64)> {
    let centre: Vec<(f64, f64)> = nodes
        .iter()
        .map(|n| (n.x as f64 + 0.5, n.z as f64 + 0.5))
        .collect();
    let side = |(dx, dz): (f64, f64)| {
        if drives_on_left {
            (dz, -dx)
        } else {
            (-dz, dx)
        }
    };
    let dir = |a: (f64, f64), b: (f64, f64)| unit(b.0 - a.0, b.1 - a.1);
    // Nodes a block or so apart only give the heading rounding to whole blocks made up, so
    // each node takes its headings from the nearest node at least that far either way.
    let far = |a: (f64, f64), b: (f64, f64)| (b.0 - a.0).hypot(b.1 - a.1) >= MIN_LEG;
    (0..centre.len())
        .map(|k| {
            let into = (0..k)
                .rev()
                .find(|&j| far(centre[j], centre[k]))
                .and_then(|j| dir(centre[j], centre[k]));
            let out = (k + 1..centre.len())
                .find(|&j| far(centre[k], centre[j]))
                .and_then(|j| dir(centre[k], centre[j]));
            let near = || {
                (k > 0)
                    .then(|| dir(centre[k - 1], centre[k]))
                    .flatten()
                    .or_else(|| {
                        (k + 1 < centre.len())
                            .then(|| dir(centre[k], centre[k + 1]))
                            .flatten()
                    })
            };
            let d = match (into, out) {
                (Some(a), Some(b)) => unit(a.0 + b.0, a.1 + b.1).unwrap_or(b),
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (None, None) => near().unwrap_or((1.0, 0.0)),
            };
            let (sx, sz) = side(d);
            (centre[k].0 + sx * offset, centre[k].1 + sz * offset)
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Waypoint {
    x: f64,
    y: f64,
    z: f64,
    /// Signal group whose green this stop line waits for: 1 east-west, 2 north-south.
    stop: Option<u8>,
    /// Speed cap while driving towards this point, for a bend or a junction.
    cap: Option<i32>,
}

/// A lane a car may turn into from the end of another.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Turn {
    lane: usize,
    /// The first of its points ahead of the car.
    entry: usize,
    /// Across a junction, where a car close ahead is crossing traffic rather than a queue.
    crossing: bool,
}

struct Segment {
    way: u64,
    points: Vec<Waypoint>,
    /// Distance along the lane at each point.
    arcs: Vec<f64>,
    next: Vec<Turn>,
    /// Ends at a junction rather than where the road just goes on or stops.
    into_junction: bool,
    /// A dead end, where the lane bends round into the way back.
    turns_back: bool,
    /// A dead end too short to drive into, which no car starts on either.
    stub: bool,
    /// Cruising speed at the limit.
    speed: i32,
    density: f64,
}

impl Segment {
    /// Whether cars start on the lane, placed or respawned: one they can drive on from.
    fn carries_cars(&self) -> bool {
        self.points.len() >= 2 && !self.next.is_empty() && !self.stub
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Sedan,
    Hatchback,
    Van,
}

struct Car {
    segment: usize,
    /// The waypoint the car is heading for.
    target: usize,
    pos: (f64, f64, f64),
    kind: Kind,
    colour: &'static str,
    /// The driver's own pace, in thousandths of the lane speed.
    pace: i32,
}

/// The drivable lanes of an area and the cars placed on them.
pub struct CarNetwork {
    segments: Vec<Segment>,
    cars: Vec<Car>,
    /// Cars by the way their lane comes from, so each is placed when that way is built.
    by_way: FnvHashMap<u64, Vec<usize>>,
    /// Blocks per metre the cars are built and spaced at.
    size: f64,
    scale: f64,
}

/// A stretch of one way between two junctions.
struct Piece<'a> {
    way: &'a ProcessedWay,
    nodes: &'a [ProcessedNode],
    half: i32,
    oneway: i8,
}

/// One direction of travel along a piece.
struct Lane<'a> {
    piece: usize,
    nodes: Vec<&'a ProcessedNode>,
    reverse: Option<usize>,
    two_way: bool,
}

/// Deterministic noise for picking colours, kinds and paces.
fn hash(a: u64, b: u64) -> u64 {
    let mut h = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^ (h >> 32)
}

fn pick<T: Copy>(choices: &[(T, u32)], roll: u64) -> T {
    let total: u32 = choices.iter().map(|c| c.1).sum();
    let mut at = (roll % total as u64) as u32;
    for &(choice, weight) in choices {
        if at < weight {
            return choice;
        }
        at -= weight;
    }
    choices[0].0
}

/// Body colours, weighted roughly as they are on the road.
const CAR_COLOURS: [(&str, u32); 8] = [
    ("minecraft:white_concrete", 24),
    ("minecraft:black_concrete", 20),
    ("minecraft:gray_concrete", 17),
    ("minecraft:light_gray_concrete", 14),
    ("minecraft:blue_concrete", 9),
    ("minecraft:red_concrete", 8),
    ("minecraft:cyan_concrete", 4),
    ("minecraft:green_concrete", 4),
];
const VAN_COLOURS: [(&str, u32); 3] = [
    ("minecraft:white_concrete", 70),
    ("minecraft:light_gray_concrete", 20),
    ("minecraft:gray_concrete", 10),
];

impl CarNetwork {
    /// `surface_y` is the height of the road surface block at a column.
    pub fn build(
        elements: &[ProcessedElement],
        scale: f64,
        drives_on_left: bool,
        stop_at_signals: bool,
        world: &XZBBox,
        surface_y: impl Fn(i32, i32) -> i32,
    ) -> Self {
        let roads: Vec<(&ProcessedWay, i32, i8)> = elements
            .iter()
            .filter_map(|e| match e {
                ProcessedElement::Way(w) if w.nodes.len() >= 2 && drivable(&w.tags) => {
                    let highway = &w.tags["highway"];
                    Some((
                        w,
                        highway_block_range(highway, &w.tags, scale),
                        oneway_sign(highway, &w.tags),
                    ))
                }
                _ => None,
            })
            .collect();
        // How many ways lead out of each node: three or more make a junction, two are just
        // a road that goes on, one is a dead end.
        let mut arms: FnvHashMap<u64, u32> = FnvHashMap::default();
        let mut widest: FnvHashMap<u64, i32> = FnvHashMap::default();
        for (way, half, _) in &roads {
            let closed =
                way.nodes.len() > 2 && way.nodes[0].id == way.nodes[way.nodes.len() - 1].id;
            let nodes = if closed {
                &way.nodes[1..]
            } else {
                &way.nodes[..]
            };
            for (k, n) in nodes.iter().enumerate() {
                let end = !closed && (k == 0 || k == nodes.len() - 1);
                *arms.entry(n.id).or_default() += if end { 1 } else { 2 };
                let w = widest.entry(n.id).or_default();
                *w = (*w).max(*half);
            }
        }
        let arms = |id: u64| arms.get(&id).copied().unwrap_or(0);
        let junction = |id: u64| arms(id) >= 3;

        let mut pieces: Vec<Piece> = Vec::new();
        for (way, half, oneway) in &roads {
            let mut start = 0;
            for k in 1..way.nodes.len() {
                if k == way.nodes.len() - 1 || junction(way.nodes[k].id) {
                    pieces.push(Piece {
                        way,
                        nodes: &way.nodes[start..=k],
                        half: *half,
                        oneway: *oneway,
                    });
                    start = k;
                }
            }
        }

        let mut lanes: Vec<Lane> = Vec::new();
        for (p, piece) in pieces.iter().enumerate() {
            let mut directions: Vec<Vec<&ProcessedNode>> = Vec::new();
            if piece.oneway >= 0 {
                directions.push(piece.nodes.iter().collect());
            }
            if piece.oneway <= 0 {
                directions.push(piece.nodes.iter().rev().collect());
            }
            let first = lanes.len();
            let both = directions.len() == 2;
            for nodes in directions {
                lanes.push(Lane {
                    piece: p,
                    nodes,
                    reverse: None,
                    two_way: piece.oneway == 0,
                });
            }
            if both {
                lanes[first].reverse = Some(first + 1);
                lanes[first + 1].reverse = Some(first);
            }
        }

        let upstream = (UPSTREAM_M * scale).max(6.0);
        let mut segments: Vec<Segment> = lanes
            .iter()
            .map(|lane| {
                let piece = &pieces[lane.piece];
                let highway = piece.way.tags["highway"].as_str();
                let speed = step_speed(limit_kmh(highway, &piece.way.tags) * LIMIT_SHARE, scale);
                let capped = |kmh: f64| {
                    let cap = step_speed(kmh, scale);
                    (cap < speed).then_some(cap)
                };
                let offset = if lane.two_way {
                    ((piece.half as f64 + 1.0) / 2.0).max(1.0)
                } else {
                    0.0
                };
                let path = Path::new(lane_line(&lane.nodes, offset, drives_on_left));
                let first = lane.nodes[0].id;
                let end = lane.nodes[lane.nodes.len() - 1];
                let last = end.id;
                // Where the area cuts a road off, cars drive out of it rather than turn.
                let edge = is_invented_node_id(last)
                    || end.x - world.min_x() < 3
                    || world.max_x() - end.x < 3
                    || end.z - world.min_z() < 3
                    || world.max_z() - end.z < 3;
                let turns_back = lane.reverse.is_some() && arms(last) <= 1 && !edge;
                // Lanes stop short of a junction, so turning cars cut across it.
                let trim = |id: u64| {
                    if junction(id) {
                        widest.get(&id).copied().unwrap_or(1) as f64 + 1.0
                    } else {
                        0.0
                    }
                };
                let (mut from, mut to) = (trim(first), path.length() - trim(last));
                if to - from < 2.0 {
                    // Too short to stop short of both ends: a stub between them, so the lanes
                    // into and out of it still lead forwards.
                    let mid = ((from + to) / 2.0).clamp(0.0, path.length());
                    (from, to) = ((mid - 1.0).max(0.0), (mid + 1.0).min(path.length()));
                }

                // (distance along, stop group, cap) for every point the lane must have.
                let mut marks: Vec<(f64, Option<u8>, Option<i32>)> = vec![
                    (
                        from,
                        None,
                        if junction(first) {
                            capped(JUNCTION_KMH)
                        } else {
                            None
                        },
                    ),
                    (
                        to,
                        None,
                        if junction(last) || turns_back {
                            capped(LANE_END_KMH)
                        } else {
                            None
                        },
                    ),
                ];
                if (junction(last) || turns_back) && to - from > LANE_END_REACH + 2.0 {
                    marks.push((to - LANE_END_REACH, None, None));
                }
                for k in 1..path.points.len().saturating_sub(1) {
                    let s = path.at[k];
                    if s > from && s < to {
                        let bend = path.bend(k);
                        // Slower the sharper the bend, down to walking pace on a hairpin.
                        let cap = (bend > 25.0)
                            .then(|| capped((70.0 - 0.6 * bend).max(12.0)))
                            .flatten();
                        marks.push((s, None, cap));
                    }
                }
                if stop_at_signals {
                    let along: Vec<ProcessedNode> =
                        lane.nodes.iter().map(|n| (*n).clone()).collect();
                    let forward_way = piece.nodes[0].id == first;
                    for (k, node) in lane.nodes.iter().enumerate().skip(1) {
                        if !is_signal(&node.tags) {
                            continue;
                        }
                        let held = match node
                            .tags
                            .get("traffic_signals:direction")
                            .map(String::as_str)
                        {
                            Some("forward") => forward_way,
                            Some("backward") => !forward_way,
                            _ => true,
                        };
                        if !held || signal_upstream(&along, k, -1, upstream) {
                            continue;
                        }
                        let dir = unit(
                            node.x as f64 - lane.nodes[k - 1].x as f64,
                            node.z as f64 - lane.nodes[k - 1].z as f64,
                        )
                        .unwrap_or((1.0, 0.0));
                        let group = if group_of(dir) == 'a' { 1 } else { 2 };
                        let s = if k == lane.nodes.len() - 1 {
                            to
                        } else {
                            path.at[k] - STOP_BACK * scale.max(0.5)
                        };
                        marks.push((s.clamp(from, to), Some(group), None));
                    }
                }
                marks.sort_by(|a, b| a.0.total_cmp(&b.0));
                // One point per spot, keeping the stop and the lowest cap any of its marks had.
                let mut merged: Vec<(f64, Option<u8>, Option<i32>)> = Vec::new();
                for (s, stop, cap) in marks {
                    match merged.last_mut() {
                        Some(last) if s - last.0 < 0.5 => {
                            last.1 = last.1.or(stop);
                            last.2 = lower(last.2, cap);
                        }
                        _ => merged.push((s, stop, cap)),
                    }
                }

                let y_at = |s: f64| {
                    let (x, z) = path.point_at(s);
                    (surface_y(x.floor() as i32, z.floor() as i32) + 1) as f64
                };
                let mut stations: Vec<(f64, Option<u8>, Option<i32>)> = Vec::new();
                for &(s, stop, cap) in &merged {
                    if let Some(&(prev, _, _)) = stations.last() {
                        refine(prev, s, &y_at, 0, &mut stations);
                    }
                    stations.push((s, stop, cap));
                }

                let (points, arcs): (Vec<Waypoint>, Vec<f64>) = stations
                    .into_iter()
                    .map(|(s, stop, cap)| {
                        let (x, z) = path.point_at(s);
                        (
                            Waypoint {
                                x: (x * 10.0).round() / 10.0,
                                y: y_at(s),
                                z: (z * 10.0).round() / 10.0,
                                stop,
                                cap,
                            },
                            s,
                        )
                    })
                    .filter(|(p, _)| {
                        world.contains(&XZPoint::new(p.x.floor() as i32, p.z.floor() as i32))
                    })
                    .unzip();
                Segment {
                    way: piece.way.id,
                    points,
                    arcs,
                    next: Vec::new(),
                    into_junction: junction(last),
                    turns_back,
                    stub: false,
                    speed,
                    density: density(highway),
                }
            })
            .collect();

        let size = scale.clamp(0.4, 3.0);
        segments
            .iter_mut()
            .for_each(|s| tidy(s, U_TURN_RADIUS_M * size));
        link(
            &mut segments,
            &lanes,
            world,
            Reach {
                stub: STUB_M * scale,
                scrap: SCRAP_M * scale,
                turn_round: U_TURN_RADIUS_M * size,
            },
            |kmh| step_speed(kmh, scale),
        );
        segments.iter_mut().for_each(pace);
        let cars = place_cars(&segments, scale);
        let mut by_way: FnvHashMap<u64, Vec<usize>> = FnvHashMap::default();
        for (c, car) in cars.iter().enumerate() {
            by_way.entry(segments[car.segment].way).or_default().push(c);
        }
        Self {
            segments,
            cars,
            by_way,
            size,
            scale,
        }
    }

    pub fn car_count(&self) -> usize {
        self.cars.len()
    }

    /// The pack function that loads the lanes into storage, a batch of lanes per line, and
    /// the constants that depend on the world's scale.
    pub fn storage_function(&self) -> String {
        let mut out = format!(
            "scoreboard players set #accel arnis {}\ndata modify storage arnis:cars seg set value []\n",
            // Per step, so a tenth of a second of it.
            ((ACCEL_MPS2 * 0.1 * 0.1 * self.scale * 1000.0).round() as i32).max(1)
        );
        // Colour lists a respawned car draws from, each colour as often as its weight.
        let list = |colours: &[(&str, u32)]| {
            colours
                .iter()
                .flat_map(|(c, w)| {
                    std::iter::repeat_n(format!("\"{c}\""), (*w as usize).div_ceil(4))
                })
                .collect::<Vec<_>>()
                .join(",")
        };
        let _ = writeln!(
            out,
            "data modify storage arnis:cars colours set value {{car:[{}],van:[{}]}}",
            list(&CAR_COLOURS),
            list(&VAN_COLOURS)
        );
        // Batched, as one load runs at most maxCommandChainLength commands.
        for batch in self.segments.chunks(LANES_PER_LINE) {
            out.push_str("data modify storage arnis:cars batch set value [");
            for (b, seg) in batch.iter().enumerate() {
                if b > 0 {
                    out.push(',');
                }
                out.push_str("{p:[");
                for (k, p) in seg.points.iter().enumerate() {
                    if k > 0 {
                        out.push(',');
                    }
                    let _ = write!(
                        out,
                        "{{x:{:.2}d,y:{:.1}d,z:{:.2}d",
                        macro_coord(p.x),
                        p.y,
                        macro_coord(p.z)
                    );
                    if let Some(g) = p.stop {
                        let _ = write!(out, ",g:{g}b");
                    }
                    if let Some(m) = p.cap {
                        let _ = write!(out, ",m:{m}");
                    }
                    out.push('}');
                }
                let next: Vec<String> = seg
                    .next
                    .iter()
                    .map(|t| {
                        let crossing = if t.crossing { ",j:1b" } else { "" };
                        format!("{{s:{},i:{}{crossing}}}", t.lane, t.entry)
                    })
                    .collect();
                // A lane no car starts on is skipped by the respawn too.
                let off = if seg.carries_cars() { "" } else { ",o:1b" };
                let _ = write!(out, "],n:[{}],v:{}{off}}}", next.join(","), seg.speed);
            }
            out.push_str(
                "]\ndata modify storage arnis:cars seg append from storage arnis:cars batch[]\n",
            );
        }
        out.push_str("data remove storage arnis:cars batch\n");
        out
    }

    /// The pack functions that depend on the size of the cars: what a driver checks ahead.
    pub fn look_functions(&self) -> Vec<(String, String)> {
        let k = self.size;
        let d = |m: f64| format!("{:.2}", m * k);
        let mut out = Vec::new();
        let mut dispatch = String::from(
            "# Which way the car points, in eighths of a turn, picks who counts as the traffic ahead.\n\
             execute store result score #h arnis.car run data get entity @s Rotation[0]\n\
             scoreboard players add #h arnis.car 202\n\
             scoreboard players operation #h arnis.car /= #45 arnis\n\
             scoreboard players operation #h arnis.car %= #8 arnis\n",
        );
        for b in 0..8 {
            let _ = writeln!(
                dispatch,
                "execute if score #h arnis.car matches {b} run function arnis:cars/look/h{b}"
            );
            // Cars heading within 67.5 degrees of this one are in its lane or turning into it.
            let centre = b as f64 * 45.0 - 180.0;
            let wrap = |a: f64| (a + 180.0).rem_euclid(360.0) - 180.0;
            let range = format!("{:.1}..{:.1}", wrap(centre - 67.5), wrap(centre + 67.5));
            let ahead = |at: f64, r: f64| {
                format!(
                    "execute rotated as @s positioned ^ ^ ^{} if entity @e[type=marker,tag=arnis.car,distance=..{},y_rotation={range}]",
                    d(at),
                    d(r)
                )
            };
            let body = format!(
                "# How close the car ahead is: 3 stop a couple of metres behind it, 2 and 1 ease off.\n\
                 {} run return run scoreboard players set #ahead arnis.car 3\n\
                 {} run return run scoreboard players set #ahead arnis.car 3\n\
                 {} run return run scoreboard players set #ahead arnis.car 2\n\
                 {} run scoreboard players set #ahead arnis.car 1\n",
                ahead(3.0, 1.7),
                ahead(6.0, 1.7),
                ahead(10.0, 2.0),
                ahead(14.5, 2.5),
            );
            out.push((
                format!("data/arnis/function/cars/look/h{b}.mcfunction"),
                body,
            ));
        }
        out.push((
            "data/arnis/function/cars/look.mcfunction".to_string(),
            dispatch,
        ));

        let clear = format!(
            "# Crossing traffic right in front, or anyone ahead inside a junction: give way, but\n\
             # not for ever, since cars can block each other in a circle there. Which goes first\n\
             # is down to its number. Cars standing at a red line are no one's business.\n\
             scoreboard players set #yield arnis.car 0\n\
             execute if score #jam arnis.car matches 1 run scoreboard players set #yield arnis.car 1\n\
             execute if score #t arnis.car matches 1.. rotated as @s positioned ^ ^ ^{near} if entity @e[type=marker,tag=arnis.car,tag=!arnis.car.held,distance=..{r}] run scoreboard players set #yield arnis.car 1\n\
             execute if score #yield arnis.car matches 0 run scoreboard players set #w arnis.car 0\n\
             execute if score #yield arnis.car matches 1 run scoreboard players add #w arnis.car 1\n\
             scoreboard players operation #lim arnis.car = #id arnis.car\n\
             scoreboard players operation #lim arnis.car %= #32 arnis\n\
             scoreboard players add #lim arnis.car 30\n\
             execute if score #yield arnis.car matches 1 if score #w arnis.car <= #lim arnis.car run scoreboard players set #t arnis.car 0\n\
             # Never through a player.\n\
             execute rotated as @s positioned ^ ^ ^{near} if entity @a[gamemode=!spectator,distance=..{pr}] run scoreboard players set #t arnis.car 0\n\
             execute rotated as @s positioned ^ ^ ^{far} if entity @a[gamemode=!spectator,distance=..{pr}] run scoreboard players set #t arnis.car 0\n",
            near = d(3.0),
            far = d(6.0),
            r = d(1.7),
            pr = d(2.0),
        );
        out.push((
            "data/arnis/function/cars/clear.mcfunction".to_string(),
            clear,
        ));

        let ease = format!(
            "# Red ahead: brake towards the stop line and stand on it.\n\
             $execute if entity @s[x=$(x),y=$(y),z=$(z),distance=..{}] run scoreboard players operation #t arnis.car < #q50 arnis.car\n\
             $execute if entity @s[x=$(x),y=$(y),z=$(z),distance=..{}] run scoreboard players operation #t arnis.car < #q25 arnis.car\n\
             $execute if entity @s[x=$(x),y=$(y),z=$(z),distance=..0.05] run scoreboard players set #t arnis.car 0\n",
            d(16.0),
            d(6.0),
        );
        out.push(("data/arnis/function/cars/ease.mcfunction".to_string(), ease));
        out
    }

    /// The functions that build a respawned car's body: the block displays of each kind,
    /// as (path in the pack, text). 26.3 reads a display's block as a bare id, so it gets
    /// its own copies in the `ov121` overlay.
    pub fn body_functions(&self) -> Vec<(String, String)> {
        let k = self.size as f32;
        let mut out = Vec::new();
        for (kind, name) in [
            (Kind::Sedan, "sedan"),
            (Kind::Hatchback, "hatchback"),
            (Kind::Van, "van"),
        ] {
            for (prefix, bare) in [("", false), ("ov121/", true)] {
                let mut text = String::new();
                for p in parts(kind, "$(colour)") {
                    let block = if bare {
                        format!("\"{}\"", p.block)
                    } else {
                        format!("{{Name:\"{}\"}}", p.block)
                    };
                    let v3 = |v: [f32; 3]| {
                        format!("[{:.3}f,{:.3}f,{:.3}f]", v[0] * k, v[1] * k, v[2] * k)
                    };
                    let (tags, glow) = match p.light {
                        Light::Head => ("\"arnis.car.part\"", ",brightness:{sky:15,block:15}"),
                        Light::Tail => (
                            "\"arnis.car.part\",\"arnis.car.tail\"",
                            ",brightness:{sky:7,block:7}",
                        ),
                        Light::None => ("\"arnis.car.part\"", ""),
                    };
                    let _ = writeln!(
                        text,
                        "$summon block_display $(x) $(y) $(z) {{Tags:[{tags}],teleport_duration:2,block_state:{block},transformation:{{left_rotation:[0f,0f,0f,1f],right_rotation:[0f,0f,0f,1f],translation:{},scale:{}}}{glow}}}",
                        v3(p.corner),
                        v3(p.size),
                    );
                }
                out.push((
                    format!("{prefix}data/arnis/function/cars/body/{name}.mcfunction"),
                    text,
                ));
            }
        }
        out
    }

    /// Places the cars whose lanes come from this way.
    pub fn place_way(&self, editor: &mut WorldEditor, way: u64) {
        for &c in self.by_way.get(&way).into_iter().flatten() {
            let car = &self.cars[c];
            let seg = &self.segments[car.segment];
            let t = seg.points[car.target];
            let (x, y, z) = car.pos;
            let yaw = (-(t.x - x)).atan2(t.z - z).to_degrees() as f32;
            let rotation = Value::List(vec![Value::Float(yaw), Value::Float(0.0)]);
            let speed = seg.speed * car.pace / 1000;

            let mut data = HashMap::from([
                ("s".to_string(), Value::Int(car.segment as i32)),
                ("i".to_string(), Value::Int(car.target as i32)),
                ("x".to_string(), Value::Double(macro_coord(t.x))),
                ("y".to_string(), Value::Double(t.y)),
                ("z".to_string(), Value::Double(macro_coord(t.z))),
                ("v".to_string(), Value::Int(speed)),
                (
                    "c".to_string(),
                    Value::Int(t.cap.map_or(speed, |m| m.min(speed))),
                ),
                ("p".to_string(), Value::Int(car.pace)),
                ("w".to_string(), Value::Int(0)),
                ("q".to_string(), Value::Int(0)),
                ("b".to_string(), Value::Byte(0)),
            ]);
            if let Some(g) = t.stop {
                data.insert("g".to_string(), Value::Byte(g as i8));
            }
            if let Some(m) = t.cap {
                data.insert("m".to_string(), Value::Int(m));
            }
            let marker = HashMap::from([
                ("data".to_string(), Value::Compound(data)),
                (
                    "Tags".to_string(),
                    Value::List(vec![Value::String("arnis.car".to_string())]),
                ),
                ("Rotation".to_string(), rotation.clone()),
            ]);
            let seed = c as i64 * 16;
            editor.add_entity_exact("minecraft:marker", x, y, z, seed, marker);
            for (k, mut nbt) in body(car.kind, car.colour, self.size)
                .into_iter()
                .enumerate()
            {
                nbt.insert("Rotation".to_string(), rotation.clone());
                editor.add_entity_exact(
                    "minecraft:block_display",
                    x,
                    y,
                    z,
                    seed + 1 + k as i64,
                    nbt,
                );
            }
        }
    }
}

/// Splits a stretch until no waypoint is more than `MAX_SPAN` from the next and the
/// ground in between is within a block of the straight line, so cars keep to the road.
fn refine(
    a: f64,
    b: f64,
    y_at: &impl Fn(f64) -> f64,
    depth: u32,
    out: &mut Vec<(f64, Option<u8>, Option<i32>)>,
) {
    if depth >= 6 || b - a < 3.0 {
        return;
    }
    let pieces = ((b - a) / MAX_SPAN).ceil() as usize;
    if pieces > 1 {
        let step = (b - a) / pieces as f64;
        for j in 0..pieces {
            let end = a + step * (j + 1) as f64;
            refine(end - step, end, y_at, depth + 1, out);
            if j + 1 < pieces {
                out.push((end, None, None));
            }
        }
        return;
    }
    let mid = (a + b) / 2.0;
    if (y_at(mid) - (y_at(a) + y_at(b)) / 2.0).abs() >= 1.0 {
        refine(a, mid, y_at, depth + 1, out);
        out.push((mid, None, None));
        refine(mid, b, y_at, depth + 1, out);
    }
}

/// The stricter of two speed caps.
fn lower(a: Option<i32>, b: Option<i32>) -> Option<i32> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Takes the kinks out of a lane: drops a point the offset line folded back over, and
/// rounds sharp corners so cars swing round them rather than turn on the spot, a hairpin
/// on a circle of at least `radius`.
fn tidy(seg: &mut Segment, radius: f64) {
    let dir = |p: &Waypoint, q: &Waypoint| unit(q.x - p.x, q.z - p.z);
    let gap = |p: &Waypoint, q: &Waypoint| (q.x - p.x).hypot(q.z - p.z);
    let turn =
        |pts: &[Waypoint], k: usize| match (dir(&pts[k - 1], &pts[k]), dir(&pts[k], &pts[k + 1])) {
            (Some(a), Some(b)) => (a.0 * b.0 + a.1 * b.1).clamp(-1.0, 1.0).acos().to_degrees(),
            _ => 180.0,
        };
    while let Some(k) = (1..seg.points.len().saturating_sub(1)).find(|&k| {
        let pts = &seg.points;
        pts[k].stop.is_none()
            && turn(pts, k) > 120.0
            && gap(&pts[k - 1], &pts[k]).min(gap(&pts[k], &pts[k + 1])) < 3.0
    }) {
        let gone = seg.points.remove(k);
        seg.arcs.remove(k);
        let kept = &mut seg.points[k];
        kept.cap = lower(kept.cap, gone.cap);
    }

    let (old, old_arcs) = (
        std::mem::take(&mut seg.points),
        std::mem::take(&mut seg.arcs),
    );
    for (k, (&p, &s)) in old.iter().zip(&old_arcs).enumerate() {
        let corner = (k > 0 && k + 1 < old.len() && p.stop.is_none())
            .then(|| {
                let (a, b) = (dir(&old[k - 1], &p)?, dir(&p, &old[k + 1])?);
                let d = (0.45 * gap(&old[k - 1], &p).min(gap(&p, &old[k + 1]))).min(3.0);
                (turn(&old, k) > 40.0 && d >= 0.5).then_some((a, b, d))
            })
            .flatten();
        let Some((a, b, d)) = corner else {
            seg.points.push(p);
            seg.arcs.push(s);
            continue;
        };
        let hairpin = turn(&old, k) > 150.0;
        // A hairpin's legs leave no room to turn between, so it turns round on its own
        // circle from as far back as that reaches, rather than past the tip.
        let d = if hairpin {
            let legs = gap(&old[k - 1], &p).min(gap(&p, &old[k + 1]));
            (0.45 * legs)
                .min(turn_round_reach(0.0, radius) - 1.0)
                .max(d)
        } else {
            d
        };
        // From `d` before the corner to `d` after it.
        let (from, to) = (
            (p.x - a.0 * d, p.z - a.1 * d),
            (p.x + b.0 * d, p.z + b.1 * d),
        );
        let mut points = vec![(from.0, from.1, 0.0)];
        if !hairpin {
            let r = d * 2.0 / 3.0;
            points.extend(curve(
                from,
                (from.0 + a.0 * r, from.1 + a.1 * r),
                (to.0 - b.0 * r, to.1 - b.1 * r),
                to,
            ));
        } else {
            // Short legs leave room only for a tighter circle, one that still ends by the tip.
            let across = ((to.0 - from.0) * -a.1 + (to.1 - from.1) * a.0).abs();
            let mut r = radius;
            while r > 0.5 && turn_round_reach(across, r) > d + 1.0 {
                r -= 0.25;
            }
            points.extend(turn_round(from, a, to, r));
        }
        points.push((to.0, to.1, 1.0));
        for (x, z, t) in points {
            seg.points.push(Waypoint {
                x: (x * 10.0).round() / 10.0,
                z: (z * 10.0).round() / 10.0,
                ..p
            });
            seg.arcs.push(s + d * (2.0 * t - 1.0));
        }
    }
}

/// Points along the cubic curve from `from` to `to` pulled by `c1` and `c2`: one for about
/// every 25 degrees it turns, or every few blocks where it hardly does, none within half a
/// block of the last, and neither end. Each comes with how far along it lies, 0 to 1.
fn curve(from: (f64, f64), c1: (f64, f64), c2: (f64, f64), to: (f64, f64)) -> Vec<(f64, f64, f64)> {
    const FINE: usize = 128;
    let at = |t: f64| {
        let u = 1.0 - t;
        let bez = |p0: f64, p1: f64, p2: f64, p3: f64| {
            u * u * u * p0 + 3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t * p3
        };
        (bez(from.0, c1.0, c2.0, to.0), bez(from.1, c1.1, c2.1, to.1))
    };
    let gap = |p: (f64, f64), q: (f64, f64)| (p.0 - q.0).hypot(p.1 - q.1);
    let mut out = Vec::new();
    let (mut last, mut was) = (from, from);
    let mut heading: Option<(f64, f64)> = None;
    let (mut turned, mut run) = (0.0, 0.0);
    for k in 1..FINE {
        let t = k as f64 / FINE as f64;
        let p = at(t);
        run += gap(p, was);
        if let Some(h) = unit(p.0 - was.0, p.1 - was.1) {
            if let Some(g) = heading {
                turned += (g.0 * h.0 + g.1 * h.1).clamp(-1.0, 1.0).acos().to_degrees();
            }
            heading = Some(h);
        }
        was = p;
        if (turned >= 25.0 || run >= 4.0) && gap(p, last) >= 0.5 && gap(p, to) >= 0.5 {
            out.push((p.0, p.1, t));
            (last, turned, run) = (p, 0.0, 0.0);
        }
    }
    out
}

/// Slows a car after a turn to less than a step per point, since a point it reaches in
/// one step is passed without turning towards it and its turn adds to the next.
fn pace(seg: &mut Segment) {
    for k in 2..seg.points.len() {
        let (a, b, c) = (seg.points[k - 2], seg.points[k - 1], seg.points[k]);
        let (Some(into), Some(out)) = (unit(b.x - a.x, b.z - a.z), unit(c.x - b.x, c.z - b.z))
        else {
            continue;
        };
        if into.0 * out.0 + into.1 * out.1 > 30.0_f64.to_radians().cos() {
            continue;
        }
        let room = ((c.x - b.x).hypot(c.z - b.z) * 950.0) as i32;
        let cap = room.max(100);
        if cap < seg.speed {
            seg.points[k].cap = lower(c.cap, Some(cap));
        }
    }
}

/// Lengths in blocks that decide how cars use dead ends.
#[derive(Clone, Copy)]
struct Reach {
    /// A dead end shorter than this is no way on.
    stub: f64,
    /// A network of roads shorter than this in all carries no cars.
    scrap: f64,
    /// Smallest radius a car turns round on.
    turn_round: f64,
}

/// Where each lane may go on from its end: any lane leaving that junction but the way
/// back, or the way back at a dead end. With nowhere to go, as off the edge of the area,
/// the car leaves.
fn link(
    segments: &mut [Segment],
    lanes: &[Lane],
    world: &XZBBox,
    reach: Reach,
    speed_at: impl Fn(f64) -> i32,
) {
    let usable = |s: &Segment| s.points.len() >= 2;
    let mut starting: FnvHashMap<u64, Vec<usize>> = FnvHashMap::default();
    for (i, lane) in lanes.iter().enumerate() {
        if usable(&segments[i]) {
            starting.entry(lane.nodes[0].id).or_default().push(i);
        }
    }
    let ways_on: Vec<Vec<usize>> = lanes
        .iter()
        .enumerate()
        .map(|(i, lane)| {
            let end = lane.nodes[lane.nodes.len() - 1].id;
            let mut next: Vec<usize> = starting
                .get(&end)
                .into_iter()
                .flatten()
                .copied()
                .filter(|&j| Some(j) != lane.reverse)
                .collect();
            if next.is_empty() && segments[i].turns_back {
                next.extend(lane.reverse.filter(|&r| usable(&segments[r])));
            }
            next
        })
        .collect();
    let length = |s: &Segment| s.arcs.last().unwrap_or(&0.0) - s.arcs.first().unwrap_or(&0.0);
    let short: Vec<bool> = segments.iter().map(|s| length(s) < SHORT_LANE).collect();
    // Measured before any lane is bent round into its way back, which lengthens it.
    for s in segments.iter_mut() {
        s.stub = s.turns_back && length(s) < reach.stub;
    }
    let stub: Vec<bool> = segments.iter().map(|s| s.stub).collect();
    for (i, lane) in lanes.iter().enumerate() {
        // A lane too short to drive is part of its junction: cars cross it onto the ones after.
        let mut next = Vec::new();
        let mut through_short = false;
        let mut todo: Vec<(usize, u32)> = ways_on[i].iter().map(|&j| (j, 0)).collect();
        while let Some((j, depth)) = todo.pop() {
            if short[j] && depth < 3 && !ways_on[j].is_empty() {
                through_short = true;
                todo.extend(
                    ways_on[j]
                        .iter()
                        .filter(|&&k| k != i && Some(k) != lane.reverse)
                        .map(|&k| (k, depth + 1)),
                );
            } else if !next.contains(&j) {
                next.push(j);
            }
        }
        // In and straight back out of a stub would spin the car round at the junction.
        if next.iter().any(|&j| !stub[j]) {
            next.retain(|&j| !stub[j]);
        }
        let entries: Vec<(usize, Option<(usize, f64)>)> = next
            .iter()
            .map(|&j| (j, entry(&segments[i].points, &segments[j].points)))
            .collect();
        let forward: Vec<(usize, usize)> = entries
            .iter()
            .filter_map(|&(j, e)| e.map(|(e, _)| (j, e)))
            .collect();
        let turns = match entries.as_slice() {
            // The only way on, and a sharp one: the lane itself bends round into it.
            &[(j, e)] if e.is_none_or(|(_, turn)| turn > SHARP_DEG) => {
                if bend_into(segments, i, j, world, reach.turn_round, &speed_at) {
                    let e = entry(&segments[i].points, &segments[j].points);
                    vec![(j, e.map_or(0, |(e, _)| e))]
                } else {
                    forward
                }
            }
            // Every way on is too sharp to drive onto: bend round into the gentlest of them
            // rather than turn on the spot.
            _ if forward.is_empty() => {
                let gentlest = next.iter().copied().min_by(|&p, &q| {
                    sharpness(segments, i, p).total_cmp(&sharpness(segments, i, q))
                });
                match gentlest {
                    Some(j) if bend_into(segments, i, j, world, reach.turn_round, &speed_at) => {
                        let e = entry(&segments[i].points, &segments[j].points);
                        vec![(j, e.map_or(0, |(e, _)| e))]
                    }
                    _ => entries.iter().map(|&(j, _)| (j, 0)).collect(),
                }
            }
            _ => forward,
        };
        let seg = &segments[i];
        let crossing = |lane: usize, entry: usize| {
            let (a, b) = (seg.points.last(), segments[lane].points.get(entry));
            (seg.into_junction || through_short)
                && a.zip(b)
                    .is_some_and(|(a, b)| (b.x - a.x).hypot(b.z - a.z) <= CROSSING_REACH)
        };
        let turns: Vec<Turn> = turns
            .into_iter()
            .map(|(lane, entry)| Turn {
                lane,
                entry,
                crossing: crossing(lane, entry),
            })
            .collect();
        segments[i].next = turns;
    }

    // A piece of road cut off from the rest, in a car park or behind a gate, leaves a car
    // nothing to do but go round and round: there is no way on anywhere in it.
    let mut root: Vec<usize> = (0..segments.len()).collect();
    fn find(root: &mut [usize], mut k: usize) -> usize {
        while root[k] != k {
            root[k] = root[root[k]];
            k = root[k];
        }
        k
    }
    for (i, seg) in segments.iter().enumerate() {
        for t in &seg.next {
            let (a, b) = (find(&mut root, i), find(&mut root, t.lane));
            root[a] = b;
        }
    }
    let mut driven: FnvHashMap<usize, f64> = FnvHashMap::default();
    for (i, seg) in segments.iter().enumerate() {
        *driven.entry(find(&mut root, i)).or_default() += length(seg);
    }
    // Unless it runs off the area, where it goes on into roads left out.
    let near =
        |v: f64, lo: i32, hi: i32| v < lo as f64 + EDGE_REACH || v > (hi + 1) as f64 - EDGE_REACH;
    let at_edge = |p: &Waypoint| {
        near(p.x, world.min_x(), world.max_x()) || near(p.z, world.min_z(), world.max_z())
    };
    let mut open: FnvHashSet<usize> = FnvHashSet::default();
    for (i, seg) in segments.iter().enumerate() {
        if seg.points.first().is_some_and(at_edge) || seg.points.last().is_some_and(at_edge) {
            open.insert(find(&mut root, i));
        }
    }
    for (i, seg) in segments.iter_mut().enumerate() {
        let network = find(&mut root, i);
        // Both ways along each road.
        if !open.contains(&network) && driven[&network] < 2.0 * reach.scrap {
            seg.next.clear();
        }
    }
}

/// The first point of lane `to` a car at the end of `from` can drive to without turning
/// round: ahead of it, within `CROSSING_REACH`, and not so far round that the lane then
/// leads back. Also how far the car turns in all, in degrees.
fn entry(from: &[Waypoint], to: &[Waypoint]) -> Option<(usize, f64)> {
    let (&end, rest) = from.split_last()?;
    let heading = rest
        .iter()
        .rev()
        .find_map(|p| unit(end.x - p.x, end.z - p.z))?;
    let turn =
        |a: (f64, f64), b: (f64, f64)| (a.0 * b.0 + a.1 * b.1).clamp(-1.0, 1.0).acos().to_degrees();
    let fits = |e: usize, room: f64| {
        let (dx, dz) = (to[e].x - end.x, to[e].z - end.z);
        let far = dx.hypot(dz);
        let leg = unit(dx, dz).filter(|_| far >= room && far <= CROSSING_REACH)?;
        let onto = turn(heading, leg);
        let on = to
            .get(e + 1)
            .and_then(|n| unit(n.x - to[e].x, n.z - to[e].z))
            .map_or(0.0, |on| turn(leg, on));
        (onto < 90.0 && on <= 90.0 && onto + on <= ENTRY_TURN_DEG).then_some((e, onto + on))
    };
    // Preferably a step or more away, so the car turns onto it before turning on.
    (0..to.len())
        .find_map(|e| fits(e, 1.0))
        .or_else(|| (0..to.len()).find_map(|e| fits(e, 0.0)))
}

/// How far round, in degrees, lane `j` starts off from the way lane `i` ends.
fn sharpness(segments: &[Segment], i: usize, j: usize) -> f64 {
    let heading = |p: &[Waypoint]| {
        p.windows(2)
            .find_map(|w| unit(w[1].x - w[0].x, w[1].z - w[0].z))
    };
    let mut end = segments[i].points.clone();
    end.reverse();
    match (heading(&end), heading(&segments[j].points)) {
        (Some(a), Some(b)) => (-a.0 * b.0 - a.1 * b.1)
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees(),
        _ => 180.0,
    }
}

/// Extends lane `i` along a curve that leaves it straight on and meets lane `j` going its
/// way, for a turn too sharp to take on the spot. False, with the lane as it was, where
/// the curve would leave the area.
fn bend_into(
    segments: &mut [Segment],
    i: usize,
    j: usize,
    world: &XZBBox,
    radius: f64,
    speed_at: impl Fn(f64) -> i32,
) -> bool {
    let seg = &segments[i];
    let Some((&a, rest)) = seg.points.split_last() else {
        return false;
    };
    let Some(h) = rest.iter().rev().find_map(|p| unit(a.x - p.x, a.z - p.z)) else {
        return false;
    };
    let to = &segments[j].points;
    let along = |k: usize| {
        let (p, q) = if k + 1 < to.len() {
            (to[k], to[k + 1])
        } else {
            (to[k.saturating_sub(1)], to[k])
        };
        unit(q.x - p.x, q.z - p.z)
    };
    let Some(out) = along(0) else {
        return false;
    };
    let turn = (h.0 * out.0 + h.1 * out.1)
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees();
    // Turning round starts early enough that its loop ends about where the lane does.
    let (a, h, keep, held) = if turn > 150.0 {
        let b = to[0];
        let gap = ((b.x - a.x) * -h.1 + (b.z - a.z) * h.0).abs();
        let first = seg.arcs.first().copied().unwrap_or(0.0);
        let last = seg.arcs.last().copied().unwrap_or(0.0);
        let shift = (turn_round_reach(gap, radius) - 1.0).clamp(0.0, (last - first - 0.5).max(0.0));
        let at = last - shift;
        let k = seg
            .arcs
            .partition_point(|&s| s < at)
            .clamp(1, seg.points.len() - 1);
        let (p, q) = (seg.points[k - 1], seg.points[k]);
        let span = seg.arcs[k] - seg.arcs[k - 1];
        let t = if span > 0.0 {
            ((at - seg.arcs[k - 1]) / span).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let start = Waypoint {
            x: p.x + (q.x - p.x) * t,
            y: p.y + (q.y - p.y) * t,
            z: p.z + (q.z - p.z) * t,
            stop: None,
            cap: q.cap,
        };
        (
            start,
            unit(q.x - p.x, q.z - p.z).unwrap_or(h),
            Some((k, at)),
            // A stop line on the part cut off moves to where the turn starts.
            seg.points[k..].iter().find_map(|p| p.stop),
        )
    } else {
        (a, h, None, None)
    };
    let (meet, out, onto_meet) = if turn > 150.0 {
        // Turning round: alongside, then up the far side to where the lane starts.
        let b = to[0];
        let back = ((b.x - a.x) * out.0 + (b.z - a.z) * out.1).clamp(0.0, 30.0);
        let meet = (b.x - out.0 * back, b.y, b.z - out.1 * back);
        (meet, out, back > 0.5)
    } else {
        // Onto the lane where there is room to swing round and arrive going its way.
        let k = (0..to.len())
            .find(|&k| {
                let (dx, dz) = (to[k].x - a.x, to[k].z - a.z);
                dx.hypot(dz) >= 3.0 && along(k).is_some_and(|o| dx * o.0 + dz * o.1 > 0.0)
            })
            .unwrap_or(to.len() - 1);
        let Some(out) = along(k) else {
            return false;
        };
        ((to[k].x, to[k].y, to[k].z), out, false)
    };
    let cap = speed_at((70.0 - 0.6 * turn).max(U_TURN_KMH));
    let cap = (cap < seg.speed).then_some(cap);
    let mut points = if turn > 150.0 {
        turn_round((a.x, a.z), h, (meet.0, meet.2), radius)
    } else {
        // Control points two thirds of the gap out.
        let reach = ((meet.0 - a.x).hypot(meet.2 - a.z) * 2.0 / 3.0).clamp(1.0, 5.0);
        curve(
            (a.x, a.z),
            (a.x + h.0 * reach, a.z + h.1 * reach),
            (meet.0 - out.0 * reach, meet.2 - out.1 * reach),
            (meet.0, meet.2),
        )
    };
    if onto_meet {
        points.push((meet.0, meet.2, 1.0));
    }
    // Where the lane is cut short, the turn round starts on a point of its own.
    if keep.is_some() {
        points.insert(0, (a.x, a.z, 0.0));
    }
    let mut along = keep.map_or(seg.arcs.last().copied().unwrap_or(0.0), |(_, at)| at);
    let mut was = (a.x, a.z);
    let mut added = Vec::new();
    for (x, z, t) in points {
        if !world.contains(&XZPoint::new(x.floor() as i32, z.floor() as i32)) {
            return false;
        }
        along += (x - was.0).hypot(z - was.1);
        was = (x, z);
        let p = Waypoint {
            x: (x * 10.0).round() / 10.0,
            y: a.y + (meet.1 - a.y) * t,
            z: (z * 10.0).round() / 10.0,
            stop: None,
            cap,
        };
        added.push((p, along));
    }
    if let Some((first, _)) = added.first_mut() {
        first.stop = first.stop.or(held);
    }
    let seg = &mut segments[i];
    if let Some((k, _)) = keep {
        seg.points.truncate(k);
        seg.arcs.truncate(k);
    }
    for (p, s) in added {
        seg.points.push(p);
        seg.arcs.push(s);
    }
    true
}

/// Radius, swing out and the run while swinging out of a turn round between lanes `gap`
/// apart, on a circle of at least `radius`.
fn turn_round_shape(gap: f64, radius: f64) -> (f64, f64, f64) {
    let r = radius.max(gap / 2.0);
    let swing = r - gap / 2.0;
    // Long enough that swinging out barely turns the car the wrong way first.
    let run = if swing > 0.25 {
        (3.0 * swing).max(1.5)
    } else {
        0.0
    };
    (r, swing, run)
}

/// How far ahead of its start a turn round between lanes `gap` apart reaches.
fn turn_round_reach(gap: f64, radius: f64) -> f64 {
    let (r, _, run) = turn_round_shape(gap, radius);
    run + r
}

/// Points round a dead end from `a`, heading `h`, onto the way back at `meet`, neither end
/// included, each with how far along it lies, 0 to 1: a half circle of at least `radius`
/// round the middle of the road. Where the lanes lie closer than that, the car first
/// swings out to its own side to make room.
fn turn_round(a: (f64, f64), h: (f64, f64), meet: (f64, f64), radius: f64) -> Vec<(f64, f64, f64)> {
    let across = (meet.0 - a.0) * -h.1 + (meet.1 - a.1) * h.0;
    let n = if across >= 0.0 {
        (-h.1, h.0)
    } else {
        (h.1, -h.0)
    };
    let gap = across.abs();
    let (r, swing, run) = turn_round_shape(gap, radius);
    let at = |along: f64, side: f64| {
        (
            a.0 + h.0 * along + n.0 * side,
            a.1 + h.1 * along + n.1 * side,
        )
    };
    let bend = |from, c1, c2, to| curve(from, c1, c2, to).into_iter().map(|(x, z, _)| (x, z));
    let mut path = vec![a];
    if run > 0.0 {
        path.extend(bend(
            a,
            at(run / 2.0, 0.0),
            at(run / 2.0, -swing),
            at(run, -swing),
        ));
    }
    const ARC_STEPS: usize = 8;
    for k in 0..=ARC_STEPS {
        let t = std::f64::consts::PI * k as f64 / ARC_STEPS as f64;
        path.push(at(run + r * t.sin(), gap / 2.0 - r * t.cos()));
    }
    if run > 0.0 {
        path.extend(bend(
            at(run, gap + swing),
            at(run / 2.0, gap + swing),
            at(run / 2.0, gap),
            at(0.0, gap),
        ));
    }
    path.push(meet);
    path.dedup_by(|p, q| (p.0 - q.0).hypot(p.1 - q.1) < 0.5);
    let gap_to = |p: (f64, f64), q: (f64, f64)| (q.0 - p.0).hypot(q.1 - p.1);
    let total: f64 = path.windows(2).map(|w| gap_to(w[0], w[1])).sum();
    let mut along = 0.0;
    let mut out = Vec::new();
    for k in 1..path.len().saturating_sub(1) {
        along += gap_to(path[k - 1], path[k]);
        out.push((path[k].0, path[k].1, along / total.max(1e-9)));
    }
    out
}

/// Spreads cars over every lane by how busy its road is, thinning them all evenly when
/// the area would hold more than `MAX_CARS`.
fn place_cars(segments: &[Segment], scale: f64) -> Vec<Car> {
    let length = |s: &Segment| {
        s.arcs.last().copied().unwrap_or(0.0) - s.arcs.first().copied().unwrap_or(0.0)
    };
    let wanted: f64 = segments
        .iter()
        .filter(|s| s.carries_cars())
        .map(|s| length(s) / scale * s.density)
        .sum();
    let thin = if wanted > MAX_CARS as f64 {
        MAX_CARS as f64 / wanted
    } else {
        1.0
    };
    let mut cars = Vec::new();
    let mut owed = 0.5;
    for (i, seg) in segments.iter().enumerate() {
        if !seg.carries_cars() {
            continue;
        }
        let len = length(seg);
        owed += len / scale * seg.density * thin;
        let n = owed.floor() as usize;
        owed -= n as f64;
        for j in 0..n {
            if cars.len() >= MAX_CARS {
                return cars;
            }
            let s = seg.arcs[0] + len * (j as f64 + 0.5) / n as f64;
            let target = seg
                .arcs
                .partition_point(|&a| a <= s)
                .clamp(1, seg.points.len() - 1);
            let (a, b) = (seg.points[target - 1], seg.points[target]);
            let span = seg.arcs[target] - seg.arcs[target - 1];
            let t = if span > 0.0 {
                ((s - seg.arcs[target - 1]) / span).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let roll = hash(i as u64, j as u64);
            let kind = pick(
                &[(Kind::Sedan, 60), (Kind::Hatchback, 26), (Kind::Van, 14)],
                roll,
            );
            let colours: &[(&str, u32)] = if kind == Kind::Van {
                &VAN_COLOURS
            } else {
                &CAR_COLOURS
            };
            cars.push(Car {
                segment: i,
                target,
                pos: (
                    a.x + (b.x - a.x) * t,
                    a.y + (b.y - a.y) * t,
                    a.z + (b.z - a.z) * t,
                ),
                kind,
                colour: pick(colours, roll >> 16),
                // From 12 % under the lane speed to 12 % over it.
                pace: 880 + ((roll >> 40) % 241) as i32,
            });
        }
    }
    cars
}

/// One block of a vehicle: its block, size and lowest corner in the vehicle's frame, and
/// how it is lit.
struct Part {
    block: &'static str,
    size: [f32; 3],
    corner: [f32; 3],
    light: Light,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Light {
    None,
    Head,
    /// Dim while driving; the pack turns it up while the car brakes or stands.
    Tail,
}

const fn part(block: &'static str, size: [f32; 3], corner: [f32; 3], light: Light) -> Part {
    Part {
        block,
        size,
        corner,
        light,
    }
}

/// A vehicle's parts at 1:1, +z ahead and the ground at 0.
fn parts(kind: Kind, colour: &'static str) -> Vec<Part> {
    // (length, width, axle from each end, wheel size, light height)
    let (l, w, axle, wheel, lights) = match kind {
        Kind::Sedan => (4.6, 1.82, 0.95, 0.64, 0.66),
        Kind::Hatchback => (4.0, 1.76, 0.75, 0.62, 0.68),
        Kind::Van => (4.9, 1.95, 0.85, 0.68, 0.75),
    };
    let wheels = |z: f32| {
        part(
            "minecraft:black_concrete",
            [w + 0.08, wheel, wheel],
            [-(w + 0.08) / 2.0, 0.0, z - wheel / 2.0],
            Light::None,
        )
    };
    let lamp = |block, z: f32, light| {
        part(
            block,
            [w - 0.3, 0.13, 0.04],
            [-(w - 0.3) / 2.0, lights, z],
            light,
        )
    };
    let mut out = vec![
        wheels(l / 2.0 - axle),
        wheels(-l / 2.0 + axle),
        lamp("minecraft:white_concrete", l / 2.0, Light::Head),
        lamp("minecraft:red_concrete", -l / 2.0 - 0.04, Light::Tail),
    ];
    match kind {
        Kind::Sedan => out.extend([
            part(
                colour,
                [w, 0.72, l],
                [-w / 2.0, 0.22, -l / 2.0],
                Light::None,
            ),
            part(
                "minecraft:tinted_glass",
                [w - 0.2, 0.52, 2.3],
                [-(w - 0.2) / 2.0, 0.94, -1.3],
                Light::None,
            ),
            part(
                colour,
                [w - 0.18, 0.07, 2.0],
                [-(w - 0.18) / 2.0, 1.43, -1.15],
                Light::None,
            ),
        ]),
        Kind::Hatchback => out.extend([
            part(
                colour,
                [w, 0.72, l],
                [-w / 2.0, 0.22, -l / 2.0],
                Light::None,
            ),
            part(
                "minecraft:tinted_glass",
                [w - 0.18, 0.56, 2.3],
                [-(w - 0.18) / 2.0, 0.94, -1.78],
                Light::None,
            ),
            part(
                colour,
                [w - 0.16, 0.07, 2.1],
                [-(w - 0.16) / 2.0, 1.47, -1.72],
                Light::None,
            ),
        ]),
        // A tall box, glazed at the front where the cab is.
        Kind::Van => out.extend([
            part(colour, [w, 1.5, l], [-w / 2.0, 0.3, -l / 2.0], Light::None),
            part(
                "minecraft:tinted_glass",
                [w + 0.04, 0.55, 1.0],
                [-(w + 0.04) / 2.0, 1.15, l / 2.0 - 0.98],
                Light::None,
            ),
        ]),
    }
    out
}

/// The block displays of one vehicle at `size` blocks per metre.
fn body(kind: Kind, colour: &'static str, size: f64) -> Vec<HashMap<String, Value>> {
    let k = size as f32;
    let floats = |v: [f32; 4]| Value::List(v.iter().map(|f| Value::Float(*f)).collect());
    let vec3 = |v: [f32; 3]| Value::List(v.iter().map(|f| Value::Float(*f * k)).collect());
    let glow = |level: i32| {
        Value::Compound(HashMap::from([
            ("sky".to_string(), Value::Int(level)),
            ("block".to_string(), Value::Int(level)),
        ]))
    };
    parts(kind, colour)
        .into_iter()
        .map(|p| {
            let mut tags = vec![Value::String("arnis.car.part".to_string())];
            if p.light == Light::Tail {
                tags.push(Value::String("arnis.car.tail".to_string()));
            }
            let mut nbt = HashMap::from([
                (
                    "block_state".to_string(),
                    Value::Compound(HashMap::from([(
                        "Name".to_string(),
                        Value::String(p.block.to_string()),
                    )])),
                ),
                (
                    "transformation".to_string(),
                    Value::Compound(HashMap::from([
                        ("left_rotation".to_string(), floats([0.0, 0.0, 0.0, 1.0])),
                        ("right_rotation".to_string(), floats([0.0, 0.0, 0.0, 1.0])),
                        ("translation".to_string(), vec3(p.corner)),
                        ("scale".to_string(), vec3(p.size)),
                    ])),
                ),
                ("Tags".to_string(), Value::List(tags)),
                // Steps come every second tick; the client glides between them.
                ("teleport_duration".to_string(), Value::Int(2)),
            ]);
            match p.light {
                Light::Head => {
                    nbt.insert("brightness".to_string(), glow(15));
                }
                Light::Tail => {
                    nbt.insert("brightness".to_string(), glow(7));
                }
                Light::None => {}
            }
            nbt
        })
        .collect()
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

    fn road(id: u64, nodes: Vec<ProcessedNode>, extra: &[(&str, &str)]) -> ProcessedElement {
        let mut tags: HashMap<String, String> = extra
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        tags.entry("highway".into()).or_insert("residential".into());
        ProcessedElement::Way(ProcessedWay { id, nodes, tags })
    }

    fn world() -> XZBBox {
        XZBBox::rect_from_min_max(-2000, -2000, 2000, 2000).unwrap()
    }

    fn flat(_: i32, _: i32) -> i32 {
        64
    }

    fn crossroads(signal: bool) -> Vec<ProcessedElement> {
        let mid: &[(&str, &str)] = if signal {
            &[("highway", "traffic_signals")]
        } else {
            &[]
        };
        vec![
            road(
                1,
                vec![
                    node(1, -100, 0, &[]),
                    node(9, 0, 0, mid),
                    node(2, 100, 0, &[]),
                ],
                &[],
            ),
            road(
                2,
                vec![
                    node(3, 0, -100, &[]),
                    node(9, 0, 0, mid),
                    node(4, 0, 100, &[]),
                ],
                &[],
            ),
        ]
    }

    #[test]
    fn a_crossroads_gives_eight_lanes_that_turn_into_each_other_but_never_straight_back() {
        let net = CarNetwork::build(&crossroads(false), 1.0, false, false, &world(), flat);
        assert_eq!(net.segments.len(), 8);
        for (i, seg) in net.segments.iter().enumerate() {
            assert!(!seg.next.is_empty(), "lane {i} leads nowhere");
            assert!(seg.points.iter().all(|p| p.y == 65.0));
        }
        assert_eq!(net.segments.iter().filter(|s| s.next.len() == 3).count(), 4);
        // Turns at the crossing cross it; those at the dead ends just turn round.
        for seg in &net.segments {
            assert!(seg.next.iter().all(|t| t.crossing == (seg.next.len() == 3)));
        }
    }

    #[test]
    fn eastbound_traffic_keeps_right_which_is_south_and_left_where_they_drive_on_the_left() {
        let east = |left: bool| {
            let net = CarNetwork::build(&crossroads(false), 1.0, left, false, &world(), flat);
            net.segments
                .iter()
                .find(|s| s.points[1].x > s.points[0].x && s.points[0].z.abs() < 3.0)
                .map(|s| s.points[0].z)
                .unwrap()
        };
        assert!(east(false) > 0.5);
        assert!(east(true) < -0.5);
    }

    #[test]
    fn lanes_into_a_signal_end_on_a_stop_line_for_their_group() {
        let net = CarNetwork::build(&crossroads(true), 1.0, false, true, &world(), flat);
        let ends: Vec<&Waypoint> = net
            .segments
            .iter()
            .filter_map(|s| s.points.last().filter(|p| p.stop.is_some()))
            .collect();
        assert_eq!(ends.len(), 4);
        assert_eq!(ends.iter().filter(|p| p.stop == Some(1)).count(), 2);
        let free = CarNetwork::build(&crossroads(true), 1.0, false, false, &world(), flat);
        assert!(free
            .segments
            .iter()
            .flat_map(|s| &s.points)
            .all(|p| p.stop.is_none()));
    }

    #[test]
    fn speeds_follow_maxspeed_and_the_road_class() {
        assert_eq!(parse_maxspeed("50"), Some(50.0));
        assert_eq!(parse_maxspeed("30 mph").map(|v| v.round()), Some(48.0));
        assert_eq!(parse_maxspeed("DE:zone30"), Some(30.0));
        assert_eq!(parse_maxspeed("DE:urban"), Some(50.0));
        assert_eq!(parse_maxspeed("signals"), None);
        // 36 km/h is one block per two-tick step on a 1:1 world.
        assert_eq!(step_speed(36.0, 1.0), 1000);
        let primary = road(
            5,
            vec![node(1, 0, 0, &[]), node(2, 200, 0, &[])],
            &[("highway", "primary")],
        );
        let net = CarNetwork::build(&[primary], 1.0, false, false, &world(), flat);
        assert_eq!(net.segments[0].speed, step_speed(50.0 * LIMIT_SHARE, 1.0));
        for junk in ["inf", "NaN", "100000", "-5 mph", "zone0"] {
            let tags = HashMap::from([("maxspeed".to_string(), junk.to_string())]);
            assert_eq!(limit_kmh("primary", &tags), 50.0, "{junk}");
        }
    }

    #[test]
    fn a_right_angle_bend_is_taken_slowly() {
        let bend = road(
            5,
            vec![
                node(1, 0, 0, &[]),
                node(2, 60, 0, &[]),
                node(3, 60, 60, &[]),
            ],
            &[("highway", "primary")],
        );
        let net = CarNetwork::build(&[bend], 1.0, false, false, &world(), flat);
        let cap = net.segments[0]
            .points
            .iter()
            .filter_map(|p| p.cap)
            .min()
            .unwrap();
        assert!(cap <= step_speed(20.0, 1.0), "{cap}");
    }

    #[test]
    fn hills_get_waypoints_and_flat_straights_do_not() {
        let straight = road(5, vec![node(1, 0, 0, &[]), node(2, 200, 0, &[])], &[]);
        let net = CarNetwork::build(
            std::slice::from_ref(&straight),
            1.0,
            false,
            false,
            &world(),
            flat,
        );
        // Leaving out the turn at the dead end.
        let turning = Some(step_speed(U_TURN_KMH, 1.0));
        let on_road = |net: &CarNetwork| {
            net.segments[0]
                .points
                .iter()
                .filter(|p| p.cap != turning)
                .count()
        };
        let flat_points = on_road(&net);
        assert!(flat_points <= 11, "{flat_points}");
        let hill = |x: i32, _: i32| 64 + ((x as f64 / 15.0).sin() * 6.0) as i32;
        let net = CarNetwork::build(&[straight], 1.0, false, false, &world(), hill);
        assert!(on_road(&net) > flat_points);
    }

    #[test]
    fn cars_spread_over_the_whole_area_and_never_exceed_the_cap() {
        // A grid far bigger than the cap allows for.
        let mut roads = Vec::new();
        for i in 0..40u64 {
            let at = i as i32 * 90 - 1800;
            roads.push(road(
                100 + i,
                vec![
                    node(1000 + i, -1800, at, &[]),
                    node(2000 + i, 1800, at, &[]),
                ],
                &[("highway", "primary")],
            ));
        }
        let net = CarNetwork::build(&roads, 1.0, false, false, &world(), flat);
        assert!(net.car_count() <= MAX_CARS);
        assert!(net.car_count() > MAX_CARS * 9 / 10);
        let first_lane = net.cars.iter().map(|c| c.segment).min().unwrap();
        let last_lane = net.cars.iter().map(|c| c.segment).max().unwrap();
        assert!(last_lane - first_lane > net.segments.len() / 2);
    }

    #[test]
    fn closed_and_underground_roads_carry_no_cars() {
        for extra in [
            [("tunnel", "yes"), ("layer", "-1")],
            [("access", "private"), ("name", "x")],
            [("highway", "busway"), ("name", "x")],
        ] {
            let way = road(7, vec![node(1, -100, 0, &[]), node(2, 100, 0, &[])], &extra);
            let net = CarNetwork::build(&[way], 1.0, false, false, &world(), flat);
            assert_eq!(net.car_count(), 0, "{extra:?}");
        }
    }

    #[test]
    fn the_storage_function_writes_every_lane_with_its_speed() {
        let net = CarNetwork::build(&crossroads(true), 1.0, false, true, &world(), flat);
        let text = net.storage_function();
        assert_eq!(text.lines().count(), 3 + 2 + 1);
        assert_eq!(text.matches("{p:[").count(), net.segments.len());
        assert!(text.contains(",g:1b"));
        assert!(text.contains(&format!(",v:{}}}", net.segments[0].speed)));
    }

    #[test]
    fn a_big_network_loads_in_a_few_commands_with_its_lanes_in_order() {
        let point = Waypoint {
            x: 0.5,
            y: 65.0,
            z: 0.5,
            stop: None,
            cap: None,
        };
        let n: usize = 100_000;
        let net = CarNetwork {
            segments: (0..n)
                .map(|i| Segment {
                    way: 1,
                    points: vec![point, point],
                    arcs: vec![0.0, 0.0],
                    next: Vec::new(),
                    into_junction: false,
                    turns_back: false,
                    stub: false,
                    speed: i as i32,
                    density: 0.0,
                })
                .collect(),
            cars: Vec::new(),
            by_way: FnvHashMap::default(),
            size: 1.0,
            scale: 1.0,
        };
        let text = net.storage_function();
        assert_eq!(text.lines().count(), 3 + 2 * n.div_ceil(LANES_PER_LINE) + 1);
        let speeds: Vec<i32> = text
            .split(",v:")
            .skip(1)
            .map(|s| s.split(|c: char| !c.is_ascii_digit()).next().unwrap())
            .map(|v| v.parse().unwrap())
            .collect();
        assert_eq!(speeds, (0..n as i32).collect::<Vec<_>>());
    }

    #[test]
    fn no_waypoint_lies_on_a_whole_x_or_z_for_the_pack_to_shift() {
        // A crossroads on whole blocks puts many lane points on whole coordinates.
        let net = CarNetwork::build(&crossroads(true), 1.0, false, true, &world(), flat);
        let text = net.storage_function();
        let mut seen = 0;
        for key in ["x:", "z:"] {
            for part in text.split(key).skip(1) {
                let v: f64 = part.split('d').next().unwrap().parse().unwrap();
                assert!(v.fract() != 0.0, "{key}{v} is whole");
                seen += 1;
            }
        }
        assert!(seen > 20);
        for car in &net.cars {
            let t = net.segments[car.segment].points[car.target];
            assert!(macro_coord(t.x).fract() != 0.0 && macro_coord(t.z).fract() != 0.0);
        }
        assert_eq!(macro_coord(48.0), 48.01);
        assert_eq!(macro_coord(-12.0), -11.99);
        assert_eq!(macro_coord(48.104), 48.1);
    }

    #[test]
    fn cars_stand_on_the_road_facing_their_lane_with_a_tail_light_the_pack_can_find() {
        let net = CarNetwork::build(&crossroads(false), 1.0, false, false, &world(), flat);
        assert!(net.car_count() > 0);
        for car in &net.cars {
            assert_eq!(car.pos.1, 65.0);
            assert!((880..=1120).contains(&car.pace));
        }
        for kind in [Kind::Sedan, Kind::Hatchback, Kind::Van] {
            let tails = parts(kind, "minecraft:red_concrete")
                .iter()
                .filter(|p| p.light == Light::Tail)
                .count();
            assert_eq!(tails, 1);
        }
    }

    #[test]
    fn respawned_bodies_match_the_placed_ones_in_both_block_formats() {
        let net = CarNetwork::build(&crossroads(false), 1.0, false, false, &world(), flat);
        let bodies = net.body_functions();
        assert_eq!(bodies.len(), 6);
        let sedan = |prefix: &str| {
            &bodies
                .iter()
                .find(|(p, _)| {
                    p == &format!("{prefix}data/arnis/function/cars/body/sedan.mcfunction")
                })
                .unwrap()
                .1
        };
        assert_eq!(sedan("").lines().count(), parts(Kind::Sedan, "x").len());
        assert!(sedan("").contains("block_state:{Name:\"$(colour)\"}"));
        assert!(sedan("ov121/").contains("block_state:\"$(colour)\""));
        assert!(sedan("").contains("\"arnis.car.tail\"],teleport_duration:2"));
        assert!(net
            .storage_function()
            .contains("colours set value {car:[\"minecraft:white_concrete\""));
    }

    #[test]
    fn the_look_functions_scale_with_the_cars() {
        let net = CarNetwork::build(&crossroads(false), 2.0, false, false, &world(), flat);
        let looks = net.look_functions();
        assert_eq!(looks.len(), 11);
        let h0 = &looks
            .iter()
            .find(|(p, _)| p.ends_with("h0.mcfunction"))
            .unwrap()
            .1;
        assert!(h0.contains("^ ^ ^6.00"), "{h0}");
        // Bucket 0 points north, so its range wraps round the half turn.
        assert!(h0.contains("y_rotation=112.5..-112.5"), "{h0}");
    }

    /// Drives every turn of the network as the pack does, `step` blocks a move and slower
    /// where a point caps it, and returns the largest swing between two moves in degrees.
    fn sharpest_swing(net: &CarNetwork, step: f64) -> f64 {
        let mut worst: f64 = 0.0;
        for (s, seg) in net.segments.iter().enumerate() {
            for &Turn { lane: n, entry, .. } in seg.next.iter().filter(|_| seg.points.len() >= 2) {
                let mut pos = (seg.points[0].x, seg.points[0].z);
                let mut heading: Option<f64> = None;
                for (lane, first) in [(s, 1), (n, entry)] {
                    for p in &net.segments[lane].points[first..] {
                        loop {
                            let d = step.min(p.cap.map_or(f64::MAX, |m| m as f64 / 1000.0));
                            let (dx, dz) = (p.x - pos.0, p.z - pos.1);
                            let dist = dx.hypot(dz);
                            if dist <= d {
                                pos = (p.x, p.z);
                                break;
                            }
                            let yaw = (-dx).atan2(dz).to_degrees();
                            if let Some(h) = heading {
                                let swing = (yaw - h + 540.0).rem_euclid(360.0) - 180.0;
                                worst = worst.max(swing.abs());
                            }
                            heading = Some(yaw);
                            pos = (pos.0 + dx / dist * d, pos.1 + dz / dist * d);
                        }
                    }
                }
            }
        }
        worst
    }

    /// A dual carriageway crossed by a two-way road, a road split into two ways at a corner,
    /// a hairpin bend and a fork at a sharp angle, all ending in dead ends.
    fn awkward_roads() -> Vec<ProcessedElement> {
        let main: &[(&str, &str)] = &[("highway", "primary"), ("oneway", "yes")];
        vec![
            road(
                10,
                vec![
                    node(1, -200, 3, &[]),
                    node(2, 0, 3, &[]),
                    node(3, 200, 3, &[]),
                ],
                main,
            ),
            road(
                11,
                vec![
                    node(4, 200, -3, &[]),
                    node(5, 0, -3, &[]),
                    node(6, -200, -3, &[]),
                ],
                main,
            ),
            road(
                12,
                vec![
                    node(7, 0, -150, &[]),
                    node(5, 0, -3, &[]),
                    node(2, 0, 3, &[]),
                    node(8, 0, 150, &[]),
                ],
                &[],
            ),
            road(
                13,
                vec![node(20, -300, 100, &[]), node(21, -150, 100, &[])],
                &[],
            ),
            road(
                14,
                vec![node(21, -150, 100, &[]), node(22, -150, 190, &[])],
                &[],
            ),
            road(
                15,
                vec![
                    node(30, 100, 100, &[]),
                    node(31, 180, 100, &[]),
                    node(32, 100, 112, &[]),
                ],
                &[],
            ),
            road(
                16,
                vec![node(40, 200, -100, &[]), node(41, 300, -100, &[])],
                &[],
            ),
            road(
                17,
                vec![node(41, 300, -100, &[]), node(42, 200, -90, &[])],
                &[],
            ),
            road(
                18,
                vec![node(41, 300, -100, &[]), node(43, 400, -100, &[])],
                &[],
            ),
        ]
    }

    #[test]
    fn no_turn_makes_a_car_swing_round_on_the_spot() {
        for left in [false, true] {
            let net = CarNetwork::build(&awkward_roads(), 1.0, left, false, &world(), flat);
            for step in [0.1, 0.3, 0.6, 1.0] {
                let swing = sharpest_swing(&net, step);
                assert!(swing <= 100.0, "{swing} at {step} a move, left {left}");
            }
        }
    }

    /// Drives every chain of three lanes as the pack does, 0.3 blocks a move, and returns
    /// the most the car turns, in degrees, within any `window` blocks of driving.
    fn tightest_turn(net: &CarNetwork, window: f64) -> f64 {
        let mut chains: Vec<Vec<(usize, usize)>> = Vec::new();
        for s in 0..net.segments.len() {
            for t in &net.segments[s].next {
                let tail = &net.segments[t.lane].next;
                if tail.is_empty() {
                    chains.push(vec![(s, 1), (t.lane, t.entry)]);
                }
                for u in tail {
                    chains.push(vec![(s, 1), (t.lane, t.entry), (u.lane, u.entry)]);
                }
            }
        }
        let mut worst: f64 = 0.0;
        for chain in chains {
            let first = &net.segments[chain[0].0].points[0];
            let mut pos = (first.x, first.z);
            // (heading change, distance) per move
            let mut moves: Vec<(f64, f64)> = Vec::new();
            let mut heading: Option<f64> = None;
            for (lane, from) in chain {
                for p in net.segments[lane].points.iter().skip(from) {
                    loop {
                        let (dx, dz) = (p.x - pos.0, p.z - pos.1);
                        let dist = dx.hypot(dz);
                        if dist <= 0.3 {
                            pos = (p.x, p.z);
                            break;
                        }
                        let yaw = (-dx).atan2(dz).to_degrees();
                        let swing =
                            heading.map_or(0.0, |h| (yaw - h + 540.0).rem_euclid(360.0) - 180.0);
                        heading = Some(yaw);
                        moves.push((swing, 0.3));
                        pos = (pos.0 + dx / dist * 0.3, pos.1 + dz / dist * 0.3);
                    }
                }
            }
            for start in 0..moves.len() {
                let (mut net_turn, mut driven) = (0.0f64, 0.0);
                for &(swing, d) in &moves[start..] {
                    net_turn += swing;
                    driven += d;
                    if driven > window {
                        break;
                    }
                    worst = worst.max(net_turn.abs());
                }
            }
        }
        worst
    }

    /// A road with a short stub off one side of it and a long dead end off the other.
    fn stub_roads() -> Vec<ProcessedElement> {
        vec![
            road(
                20,
                vec![
                    node(1, -150, 0, &[]),
                    node(2, 0, 0, &[]),
                    node(3, 150, 0, &[]),
                ],
                &[],
            ),
            road(21, vec![node(2, 0, 0, &[]), node(4, 0, 12, &[])], &[]),
            road(22, vec![node(2, 0, 0, &[]), node(5, 0, -120, &[])], &[]),
        ]
    }

    #[test]
    fn no_one_turns_into_a_short_stub_while_there_is_anywhere_else_to_go() {
        let net = CarNetwork::build(&stub_roads(), 1.0, false, false, &world(), flat);
        let into_stub = |j: usize| {
            let p = &net.segments[j].points;
            p[0].z > 0.5 && p.last().unwrap().z > p[0].z
        };
        let stub = (0..net.segments.len()).find(|&j| into_stub(j)).unwrap();
        assert!(net.segments[stub].turns_back);
        for seg in &net.segments {
            assert!(seg.next.iter().all(|t| t.lane != stub));
        }
        // The long dead end is still driven into, and the cars come back out of it.
        assert!(net
            .segments
            .iter()
            .any(|s| s.next.iter().any(|t| net.segments[t.lane].turns_back)));
    }

    // Two ways meeting where their last nodes lie a fraction of a block apart, as on the
    // Champs-Elysees, which rounds them onto one block or onto a step across the road.
    #[test]
    fn nodes_rounded_onto_neighbouring_blocks_do_not_fold_the_lane() {
        for (p, x) in [
            ((785, 600), (784, 600)),
            ((784, 599), (784, 599)),
            ((784, 600), (784, 599)),
        ] {
            let roads = vec![
                road(
                    1,
                    vec![
                        node(1, 900, 656, &[]),
                        node(2, 834, 624, &[]),
                        node(3, 786, 600, &[]),
                        node(4, p.0, p.1, &[]),
                        node(5, x.0, x.1, &[]),
                    ],
                    &[("highway", "primary")],
                ),
                road(
                    2,
                    vec![
                        node(5, x.0, x.1, &[]),
                        node(6, 770, 593, &[]),
                        node(7, 693, 556, &[]),
                        node(8, 600, 511, &[]),
                    ],
                    &[("highway", "primary")],
                ),
            ];
            let net = CarNetwork::build(&roads, 1.0, false, false, &world(), flat);
            for seg in &net.segments {
                let pts: Vec<&Waypoint> = seg
                    .points
                    .iter()
                    .filter(|q| q.x > 760.0 && q.x < 810.0)
                    .collect();
                let west = seg.points[0].x > seg.points.last().unwrap().x;
                for w in pts.windows(2) {
                    let dx = w[1].x - w[0].x;
                    assert!(
                        if west { dx <= 0.05 } else { dx >= -0.05 },
                        "a fold at {:?}",
                        w
                    );
                }
            }
        }
    }

    // A switchback mapped with one node at its tip and a short leg after it: the turn
    // round has to fit before the road ends there. The outer lane rounds the tip on its
    // own side of the road, a couple of blocks out.
    #[test]
    fn a_hairpin_with_a_short_leg_turns_round_before_the_tip() {
        let roads = vec![road(
            1,
            vec![
                node(1, 0, 0, &[]),
                node(2, 40, 0, &[]),
                node(3, 34, 2, &[]),
                node(4, -10, 10, &[]),
            ],
            &[],
        )];
        for left in [false, true] {
            let net = CarNetwork::build(&roads, 1.0, left, false, &world(), flat);
            let far = net
                .segments
                .iter()
                .flat_map(|s| &s.points)
                .map(|p| p.x)
                .fold(f64::MIN, f64::max);
            assert!(
                far <= 43.5,
                "a car runs on to x={far}, past the tip at 40, left {left}"
            );
        }
    }

    // A cul-de-sac whose junction lies just outside the area is a road going on, not a
    // scrap: cars still drive in, turn round and leave. Clipped to the edge, as parsed.
    #[test]
    fn a_dead_end_running_off_the_area_is_no_scrap() {
        let roads = vec![road(
            1,
            vec![node(1, 2000, 0, &[]), node(2, 1930, 0, &[])],
            &[("highway", "primary")],
        )];
        let net = CarNetwork::build(&roads, 1.0, false, false, &world(), flat);
        assert!(net.segments.iter().any(|s| !s.next.is_empty()));
        assert!(net.car_count() > 0);
    }

    // A signalised junction whose every exit doubles back: the lane turns round from short
    // of its end, and its stop line goes with it rather than with the part cut off.
    #[test]
    fn a_lane_turning_round_early_keeps_its_stop_line() {
        let main: &[(&str, &str)] = &[("highway", "primary"), ("oneway", "yes")];
        let roads = vec![
            road(
                1,
                vec![
                    node(1, 0, 0, &[]),
                    node(2, 100, 0, &[("highway", "traffic_signals")]),
                ],
                main,
            ),
            road(2, vec![node(2, 100, 0, &[]), node(3, 0, 8, &[])], main),
            road(3, vec![node(2, 100, 0, &[]), node(4, 0, -8, &[])], main),
        ];
        let net = CarNetwork::build(&roads, 1.0, false, true, &world(), flat);
        let lane = net
            .segments
            .iter()
            .find(|s| s.way == 1)
            .expect("the approach has a lane");
        assert!(!lane.next.is_empty(), "the approach leads on");
        assert!(
            lane.points.iter().any(|p| p.stop.is_some()),
            "the stop line is gone"
        );
    }

    // Respawned cars draw any lane, so the ones no car starts on are marked for them.
    #[test]
    fn lanes_no_car_starts_on_are_marked_off_for_the_respawn() {
        let net = CarNetwork::build(&stub_roads(), 1.0, false, false, &world(), flat);
        let text = net.storage_function();
        let lanes: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("batch set value"))
            .flat_map(|l| l.split("{p:[").skip(1))
            .collect();
        assert_eq!(lanes.len(), net.segments.len());
        for (seg, lane) in net.segments.iter().zip(&lanes) {
            assert_eq!(lane.contains(",o:1b"), !seg.carries_cars(), "{lane}");
        }
        assert!(net.segments.iter().any(|s| s.stub));
    }

    #[test]
    fn a_scrap_of_road_with_a_dead_end_at_each_end_sends_no_one_round_it() {
        let scrap = road(30, vec![node(1, 0, 0, &[]), node(2, 8, 0, &[])], &[]);
        let net = CarNetwork::build(&[scrap], 1.0, false, false, &world(), flat);
        assert!(net.segments.iter().all(|s| s.next.is_empty()));
        assert_eq!(net.car_count(), 0);
    }

    #[test]
    fn a_car_turning_round_never_spins_on_the_spot() {
        for left in [false, true] {
            for roads in [stub_roads(), awkward_roads()] {
                let net = CarNetwork::build(&roads, 1.0, left, false, &world(), flat);
                // Half a turn inside a car length and a bit is spinning on the spot.
                let turn = tightest_turn(&net, 5.0);
                assert!(turn <= 170.0, "{turn} degrees in 5 blocks, left {left}");
            }
        }
    }

    #[test]
    fn a_dead_end_is_turned_round_in_a_curve_onto_the_way_back() {
        let way = road(5, vec![node(1, 0, 0, &[]), node(2, 100, 0, &[])], &[]);
        let net = CarNetwork::build(&[way], 1.0, false, false, &world(), flat);
        let east = net
            .segments
            .iter()
            .position(|s| s.points[1].x > s.points[0].x)
            .unwrap();
        let seg = &net.segments[east];
        assert!(seg.turns_back);
        assert_eq!(seg.next.len(), 1);
        assert_ne!(seg.next[0].lane, east);
        // The curve turns round at the end of the road, not past it, and slowly.
        let reach = seg.points.iter().map(|p| p.x).fold(f64::MIN, f64::max);
        assert!(reach > 100.0 && reach < 102.5, "{reach}");
        assert!(seg.points.last().unwrap().cap.is_some());
    }

    #[test]
    fn a_road_split_into_two_ways_is_driven_on_without_slowing() {
        let roads = vec![
            road(5, vec![node(1, 0, 0, &[]), node(2, 100, 0, &[])], &[]),
            road(6, vec![node(2, 100, 0, &[]), node(3, 200, 0, &[])], &[]),
        ];
        let net = CarNetwork::build(&roads, 1.0, false, false, &world(), flat);
        let east: Vec<usize> = (0..net.segments.len())
            .filter(|&i| net.segments[i].points[1].x > net.segments[i].points[0].x)
            .collect();
        let first = east
            .iter()
            .copied()
            .find(|&i| net.segments[i].points[0].x < 50.0)
            .unwrap();
        let seg = &net.segments[first];
        assert!(seg.points.iter().all(|p| p.cap.is_none()));
        assert!(seg.points.last().unwrap().x >= 100.0);
        assert_eq!(seg.next.len(), 1);
        assert!(east.contains(&seg.next[0].lane));
        // Driving on along a road is no junction to give way at.
        assert!(!seg.next[0].crossing);
    }

    #[test]
    fn cars_leave_where_the_area_cuts_a_road_off() {
        let world = XZBBox::rect_from_min_max(0, 0, 400, 400).unwrap();
        let way = road(5, vec![node(1, 0, 200, &[]), node(2, 200, 200, &[])], &[]);
        let net = CarNetwork::build(&[way], 1.0, false, false, &world, flat);
        let (out, back): (Vec<&Segment>, Vec<&Segment>) = net
            .segments
            .iter()
            .partition(|s| s.points[1].x < s.points[0].x);
        assert!(out[0].next.is_empty());
        assert_eq!(back[0].next.len(), 1);
    }

    #[test]
    fn lanes_too_short_to_drive_are_crossed_as_part_of_the_junction() {
        let net = CarNetwork::build(&awkward_roads(), 1.0, false, false, &world(), flat);
        let short = |j: usize| {
            let a = &net.segments[j].arcs;
            a.last().unwrap_or(&0.0) - a.first().unwrap_or(&0.0) < SHORT_LANE
        };
        assert!((0..net.segments.len()).any(short));
        for seg in &net.segments {
            assert!(seg.next.iter().all(|t| !short(t.lane)));
        }
    }
}
