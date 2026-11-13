//! Cars that drive the mapped streets, run by the extras data pack. Lanes, the turns at
//! each junction, speed limits and the stop lines at signals are worked out here and
//! written into the pack as storage; each car is a marker that holds its place on that
//! network, carried along with the block displays that draw it. The tags, the storage
//! layout and the car data are the contract with
//! `assets/minecraft/datapack_extras/data/arnis/function/cars`.
//!
//! Speeds are in thousandths of a block per step, a step being the two ticks between
//! moves, so 1000 is 36 km/h on a 1:1 world.

use crate::bedrock_addon::{self, Body, Fleet, Lamp, Shape};
use crate::clipping::is_invented_node_id;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::datapack::macro_coord;
use crate::element_processing::highways::{highway_block_range, highway_default_lanes};
use crate::element_processing::road_markings::{is_roundabout, oneway_sign};
use crate::element_processing::traffic_lights::{
    group_of, is_signal, is_vehicular, signal_upstream, Junctions, UPSTREAM_M,
};
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use fnv::{FnvHashMap, FnvHashSet};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

/// Longest stretch between two waypoints; shorter where the ground bends.
const MAX_SPAN: f64 = 24.0;
/// The cars placed in a world, spread over all of its lanes. Only those near a player move,
/// so the cap is about what the world files hold, not what a tick costs.
const MAX_CARS: usize = 1500;
/// Most cars the roads around one player are filled up to, which is what a tick costs.
const MAX_NEAR: usize = 120;
/// Side of the map cells the spots new cars start from are stored by, in blocks.
const SPOT_CELL: f64 = 64.0;
/// Lanes the pack stores per command.
const LANES_PER_LINE: usize = 256;
/// Spot cells the pack stores per command.
const SPOT_CELLS_PER_LINE: usize = 256;
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
/// Where a driver looks for the car ahead, in metres at 1:1: how far ahead, how far round
/// that spot, and how close it then is (3 stop a couple of metres behind it, 2 and 1 ease
/// off). The Java pack and the Bedrock script both drive by these and the rules below.
const LOOK: [(f64, f64, u8); 4] = [(3.0, 1.7, 3), (6.0, 1.7, 3), (10.0, 2.0, 2), (14.5, 2.5, 1)];
/// How much further than a car's a bus or truck reaches ahead of and behind its middle.
const LONG_EXTRA: f64 = 2.7;
/// Right in front of a car, closer than the look ahead starts: how far ahead and how far
/// round. The car's own middle is outside it.
const CLOSE_AHEAD: (f64, f64) = (0.7, 0.65);
/// The same for a bus or truck, from there on to where its longer look ahead starts.
const LONG_CLOSE_AHEAD: (f64, f64) = (2.7, 1.4);
/// Cars whose middles are this close stand on one spot, and the newer one waits, unless the
/// older one is right behind it and so waits itself.
const STACKED: f64 = 1.2;
/// Cars heading within this many degrees of one are in its lane or turning into it.
const SAME_WAY_DEG: f64 = 67.5;
/// Where crossing traffic right in front is looked for: how far ahead and how far round.
const CROSSING_AT: (f64, f64) = (3.0, 1.7);
/// How far over the block a player rides on their eyes are, at any scale.
const RIDER_EYE_OVER_SEAT: f64 = 1.02;
/// From a car's middle to its side, in blocks.
const CAR_HALF_WIDTH: f64 = 0.9;
/// The narrowest a lane is laid out, in blocks: cars side by side must not touch.
const MIN_LANE_WIDTH: i32 = 3;
/// The most lanes laid out each way.
const MAX_LANES: usize = 4;
/// The closest to the middle of a road a lane comes where buildings crowd it.
const NARROW_LANE_OFFSET: f64 = 0.5;
/// The closest a lane of a two-way road comes to the middle: oncoming cars still pass.
const ONCOMING_OFFSET: f64 = 1.0;
/// A player this close to either of two spots ahead stops the car: the spots, and how close.
const PLAYER_AT: (f64, f64, f64) = (3.0, 6.0, 2.0);
/// How far round those spots a walker stops the car: about the lane, not the pavement.
const WALKER_AT: f64 = 1.2;
/// Steps a car gives way at a junction before it goes anyway, and up to 31 more by its number.
const GIVE_WAY_STEPS: i32 = 30;
/// A driver's pace, in thousandths of the lane speed, both ends included.
const PACE: (i32, i32) = (880, 1120);
/// How often each kind of car turns up.
const KINDS: [(Kind, u32); 6] = [
    (Kind::Sedan, 52),
    (Kind::Hatchback, 24),
    (Kind::Van, 10),
    (Kind::Pickup, 6),
    (Kind::Truck, 5),
    (Kind::Bus, 3),
];

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
/// 1200 vehicles an hour and 30 km/h holds 40 per kilometre and direction, and a side
/// street at 200 an hour and 20 km/h 10.
fn density(highway: &str) -> f64 {
    match highway {
        // One simulated lane, kept about two seconds apart at speed.
        "motorway" | "trunk" | "motorway_link" | "trunk_link" => 1.0 / 40.0,
        "primary" | "secondary" | "primary_link" | "secondary_link" => 1.0 / 25.0,
        "tertiary" | "tertiary_link" => 1.0 / 40.0,
        "unclassified" | "road" => 1.0 / 80.0,
        "residential" => 1.0 / 100.0,
        "service" => 1.0 / 250.0,
        _ => 1.0 / 300.0,
    }
}

pub(crate) fn drivable(tags: &HashMap<String, String>) -> bool {
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

/// The line a car follows: `offset` from the middle, or nearer to it where a building
/// stands on the road, as it does in narrow old streets. A lane with others beside it moves
/// in only so far that cars side by side still pass.
fn fitted_lane_line(
    nodes: &[&ProcessedNode],
    offset: f64,
    floor: f64,
    drives_on_left: bool,
    built: &impl Fn(i32, i32) -> bool,
) -> Vec<(f64, f64)> {
    if offset.abs() <= floor {
        return lane_line(nodes, offset, drives_on_left);
    }
    let sign = offset.signum();
    let mut offsets = vec![offset.abs()];
    while offsets[offsets.len() - 1] > floor {
        offsets.push((offsets[offsets.len() - 1] - 0.5).max(floor));
    }
    let lines: Vec<_> = offsets
        .iter()
        .map(|&o| lane_line(nodes, sign * o, drives_on_left))
        .collect();
    let edges: Vec<_> = offsets
        .iter()
        .map(|&o| lane_line(nodes, sign * (o + CAR_HALF_WIDTH), drives_on_left))
        .collect();
    let leg_is_clear = |a: (f64, f64), b: (f64, f64)| {
        let steps = (b.0 - a.0).hypot(b.1 - a.1).ceil().max(1.0) as usize;
        (0..=steps).all(|i| {
            let t = i as f64 / steps as f64;
            !built(
                (a.0 + (b.0 - a.0) * t).floor() as i32,
                (a.1 + (b.1 - a.1) * t).floor() as i32,
            )
        })
    };
    (0..nodes.len())
        .map(|k| {
            let legs = k.saturating_sub(1)..(k + 1).min(nodes.len() - 1);
            let fit = edges
                .iter()
                .position(|edge| legs.clone().all(|j| leg_is_clear(edge[j], edge[j + 1])))
                .unwrap_or(offsets.len() - 1);
            lines[fit][k]
        })
        .collect()
}

/// The lane's centre line: the node path pushed `offset` to the driving side.
pub(crate) fn lane_line(
    nodes: &[&ProcessedNode],
    offset: f64,
    drives_on_left: bool,
) -> Vec<(f64, f64)> {
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
    Pickup,
    Truck,
    Bus,
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
    /// Where new cars can join the traffic, as (lane, point) by map cell.
    spots: BTreeMap<(i32, i32), Vec<(usize, usize)>>,
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

/// One lane of travel along a piece.
struct Lane<'a> {
    piece: usize,
    nodes: Vec<&'a ProcessedNode>,
    reverse: Option<usize>,
    two_way: bool,
    /// Against the way's own direction.
    backward: bool,
    /// From the way's middle, towards the driver's side.
    offset: f64,
    /// How many lanes go this way along the piece.
    count: usize,
}

/// How many lanes a way carries along it and against it, as far as its width has room.
fn lanes_each_way(way: &ProcessedWay, half: i32, oneway: i8) -> (usize, usize) {
    let tag = |k: &str| {
        way.tags
            .get(k)
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0)
    };
    let highway = way.tags.get("highway").map_or("", String::as_str);
    let total = tag("lanes").unwrap_or_else(|| {
        if is_roundabout(&way.tags) {
            1
        } else {
            highway_default_lanes(highway).max(1) as usize
        }
    });
    if oneway != 0 {
        let room = ((2 * half + 1) / MIN_LANE_WIDTH).max(1) as usize;
        let n = total.min(room).min(MAX_LANES);
        return if oneway > 0 { (n, 0) } else { (0, n) };
    }
    let room = (half / MIN_LANE_WIDTH).max(1) as usize;
    let fit = |n: usize| n.min(room).clamp(1, MAX_LANES);
    (
        fit(tag("lanes:forward").unwrap_or(total.div_ceil(2))),
        fit(tag("lanes:backward").unwrap_or(total / 2)),
    )
}

/// The nearest to the road's middle a lane moves in where a building stands on the road.
/// Cars side by side, or meeting on a two-way road, still pass each other.
fn lane_floor(half: i32, offset: f64, count: usize, two_way: bool) -> f64 {
    let beside = if count <= 1 {
        0.0
    } else if two_way {
        offset.abs() - f64::from(MIN_LANE_WIDTH) + 2.0 * CAR_HALF_WIDTH
    } else {
        // Clear of where the next lane in runs. The two either side of a one-way road's
        // middle both move towards it, so each keeps half a car's room from it.
        let spacing = f64::from(2 * half + 1) / count as f64;
        (offset.abs() - spacing + 2.0 * CAR_HALF_WIDTH).max(CAR_HALF_WIDTH)
    };
    let oncoming = if two_way { ONCOMING_OFFSET } else { 0.0 };
    beside.max(oncoming).max(NARROW_LANE_OFFSET)
}

/// How far from a road's middle the `k`th of `count` lanes one way along it runs, counted
/// from the middle of a two-way road or the far kerb of a one-way one.
fn lane_offset(half: i32, count: usize, k: usize, two_way: bool) -> f64 {
    let half = f64::from(half);
    let share = (k as f64 + 0.5) / count.max(1) as f64;
    if !two_way {
        let width = 2.0 * half + 1.0;
        -width / 2.0 + width * share
    } else if count <= 1 {
        ((half + 1.0) / 2.0).max(1.0)
    } else {
        0.5 + half * share
    }
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
const PICKUP_COLOURS: [(&str, u32); 5] = [
    ("minecraft:white_concrete", 30),
    ("minecraft:black_concrete", 25),
    ("minecraft:gray_concrete", 20),
    ("minecraft:red_concrete", 15),
    ("minecraft:blue_concrete", 10),
];
const TRUCK_COLOURS: [(&str, u32); 3] = [
    ("minecraft:white_concrete", 50),
    ("minecraft:blue_concrete", 25),
    ("minecraft:red_concrete", 25),
];
const BUS_COLOURS: [(&str, u32); 4] = [
    ("minecraft:white_concrete", 35),
    ("minecraft:red_concrete", 25),
    ("minecraft:blue_concrete", 20),
    ("minecraft:yellow_concrete", 20),
];

impl CarNetwork {
    /// `surface_y` is the height of a way's road surface block at a column.
    pub fn build(
        elements: &[ProcessedElement],
        scale: f64,
        drives_on_left: bool,
        stop_at_signals: bool,
        world: &XZBBox,
        surface_y: impl Fn(&ProcessedWay, i32, i32) -> i32,
        built: impl Fn(i32, i32) -> bool,
    ) -> Self {
        let found: Vec<(&ProcessedWay, (i32, i8))> = elements
            .iter()
            .filter_map(|e| match e {
                ProcessedElement::Way(w) if w.nodes.len() >= 2 && drivable(&w.tags) => {
                    let highway = &w.tags["highway"];
                    Some((
                        w,
                        (
                            highway_block_range(highway, &w.tags, scale),
                            oneway_sign(highway, &w.tags),
                        ),
                    ))
                }
                _ => None,
            })
            .collect();
        // A drawn outline cuts the roads where they leave it, which cars treat as the edge.
        let found = crate::clipping::cut_ways_to_outline(found, world);
        let roads: Vec<(&ProcessedWay, i32, i8)> = found
            .iter()
            .map(|(w, (half, oneway))| (w.as_ref(), *half, *oneway))
            .collect();
        let junctions = Junctions::build(roads.iter().map(|&(w, half, _)| (w, half)), scale);
        let arms = |id: u64| junctions.arms(id);
        let junction = |id: u64| junctions.is_junction(id);

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
            let two_way = piece.oneway == 0;
            let (along, against) = lanes_each_way(piece.way, piece.half, piece.oneway);
            let first = lanes.len();
            for (count, backward) in [(along, false), (against, true)] {
                let nodes: Vec<&ProcessedNode> = if backward {
                    piece.nodes.iter().rev().collect()
                } else {
                    piece.nodes.iter().collect()
                };
                for k in 0..count {
                    lanes.push(Lane {
                        piece: p,
                        nodes: nodes.clone(),
                        reverse: None,
                        two_way,
                        backward,
                        offset: lane_offset(piece.half, count, k, two_way),
                        count,
                    });
                }
            }
            // A dead end turns each lane round into its opposite number.
            if two_way {
                for k in 0..along {
                    lanes[first + k].reverse = Some(first + along + k.min(against - 1));
                }
                for k in 0..against {
                    lanes[first + along + k].reverse = Some(first + k.min(along - 1));
                }
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
                let path = Path::new(fitted_lane_line(
                    &lane.nodes,
                    lane.offset,
                    lane_floor(piece.half, lane.offset, lane.count, lane.two_way),
                    drives_on_left,
                    &built,
                ));
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
                        junctions.widest(id) as f64 + 1.0
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
                        if lane.two_way && junctions.passed(&along, k, -1) {
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
                    (surface_y(piece.way, x.floor() as i32, z.floor() as i32) + 1) as f64
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
                    // A road's lanes do not all fill up alike.
                    density: density(highway) * 2.0 / (lane.count as f64 + 1.0),
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
        let spots = spots(&segments, scale);
        let mut by_way: FnvHashMap<u64, Vec<usize>> = FnvHashMap::default();
        for (c, car) in cars.iter().enumerate() {
            by_way.entry(segments[car.segment].way).or_default().push(c);
        }
        Self {
            segments,
            cars,
            by_way,
            spots,
            size,
            scale,
        }
    }

    pub fn car_count(&self) -> usize {
        self.cars.len()
    }

    /// How much faster a car gets each step, in thousandths of a block a step.
    fn accel(&self) -> i32 {
        // Per step, so a tenth of a second of it.
        ((ACCEL_MPS2 * 0.1 * 0.1 * self.scale * 1000.0).round() as i32).max(1)
    }

    /// The pack function that loads the lanes into storage, a batch of lanes per line, and
    /// the constants that depend on the world's scale.
    pub fn storage_function(&self) -> String {
        let mut out = format!(
            "scoreboard players set #accel arnis {}\nscoreboard players set #near_cap arnis.car {MAX_NEAR}\nscoreboard players set #spot_cell arnis.car {}\ndata modify storage arnis:cars seg set value []\n",
            self.accel(),
            SPOT_CELL as i32,
        );
        let colours: Vec<String> = KINDS
            .iter()
            .map(|&(kind, _)| {
                let list: Vec<String> = respawn_colours(kind.colours())
                    .iter()
                    .map(|c| format!("\"{c}\""))
                    .collect();
                format!("{}:[{}]", kind.name(), list.join(","))
            })
            .collect();
        let _ = writeln!(
            out,
            "data modify storage arnis:cars colours set value {{{}}}",
            colours.join(",")
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
                let _ = write!(out, "],n:[{}],v:{}}}", next.join(","), seg.speed);
            }
            out.push_str(
                "]\ndata modify storage arnis:cars seg append from storage arnis:cars batch[]\n",
            );
        }
        out.push_str("data remove storage arnis:cars batch\n");
        out.push_str("data modify storage arnis:cars grid set value {}\n");
        let cells: Vec<_> = self.spots.iter().collect();
        for batch in cells.chunks(SPOT_CELLS_PER_LINE) {
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
                out,
                "data modify storage arnis:cars grid merge value {{{}}}",
                cells.join(",")
            );
        }
        out
    }

    /// The pack functions that depend on the size of the cars: what a driver checks ahead.
    pub fn look_functions(&self, walkers: bool) -> Vec<(String, String)> {
        let k = self.size;
        let d = |m: f64| format!("{:.2}", m * k);
        let mut out = Vec::new();
        let mut dispatch = format!(
            "# Right in front, closer than the look ahead below starts: wait. Two on one spot, as\n\
             # where two lanes merge into one, do not see each other there: the newer one waits.\n\
             execute rotated as @s positioned ^ ^ ^{close} if entity @e[type=marker,tag=arnis.car,distance=..{round}] run return run scoreboard players set #ahead arnis.car 3\n\
             execute if entity @s[tag=arnis.car.long] rotated as @s positioned ^ ^ ^{long_close} if entity @e[type=marker,tag=arnis.car,distance=..{long_round}] run return run scoreboard players set #ahead arnis.car 3\n\
             execute rotated as @s as @e[type=marker,tag=arnis.car,distance=..{stacked}] if score @s arnis.car < #id arnis.car positioned ^ ^ ^-{close} unless entity @s[distance=..{round}] run return run scoreboard players set #ahead arnis.car 3\n\
             # Which way the car points, in eighths of a turn, picks who counts as the traffic ahead.\n\
             execute store result score #h arnis.car run data get entity @s Rotation[0]\n\
             scoreboard players add #h arnis.car 202\n\
             scoreboard players operation #h arnis.car /= #45 arnis\n\
             scoreboard players operation #h arnis.car %= #8 arnis\n",
            close = d(CLOSE_AHEAD.0),
            round = d(CLOSE_AHEAD.1),
            long_close = d(LONG_CLOSE_AHEAD.0),
            long_round = d(LONG_CLOSE_AHEAD.1),
            stacked = d(STACKED),
        );
        for b in 0..8 {
            let _ = writeln!(
                dispatch,
                "execute if score #h arnis.car matches {b} unless entity @s[tag=arnis.car.long] run function arnis:cars/look/h{b}\n\
                 execute if score #h arnis.car matches {b} if entity @s[tag=arnis.car.long] run function arnis:cars/look/l{b}"
            );
            let centre = b as f64 * 45.0 - 180.0;
            let wrap = |a: f64| (a + 180.0).rem_euclid(360.0) - 180.0;
            let range = format!(
                "{:.1}..{:.1}",
                wrap(centre - SAME_WAY_DEG),
                wrap(centre + SAME_WAY_DEG)
            );
            let ahead = |at: f64, r: f64, tag: &str| {
                format!(
                    "execute rotated as @s positioned ^ ^ ^{} if entity @e[type=marker,tag={tag},distance=..{},y_rotation={range}]",
                    d(at),
                    d(r)
                )
            };
            // A bus or truck ahead is further off for the same gap, and one looking is too.
            for (name, own) in [("h", 0.0), ("l", LONG_EXTRA)] {
                let reach = LOOK
                    .iter()
                    .map(|&(at, r, _)| at + own + LONG_EXTRA + r)
                    .fold(0.0, f64::max);
                let mut body = format!(
                    "# How close the car ahead is: 3 stop a couple of metres behind it, 2 and 1 ease off.\n\
                     # Buses and trucks are few, so they are looked for only with one about.\n\
                     scoreboard players set #longs arnis.car 0\n\
                     execute if entity @e[type=marker,tag=arnis.car.long,distance=..{}] run scoreboard players set #longs arnis.car 1\n",
                    d(reach)
                );
                for (n, &(at, r, close)) in LOOK.iter().enumerate() {
                    let then = if n + 1 < LOOK.len() {
                        "return run "
                    } else {
                        ""
                    };
                    for (extra, tag, only) in [
                        (0.0, "arnis.car", ""),
                        (
                            LONG_EXTRA,
                            "arnis.car.long",
                            "if score #longs arnis.car matches 1 ",
                        ),
                    ] {
                        let _ = writeln!(
                            body,
                            "{} run {then}scoreboard players set #ahead arnis.car {close}",
                            ahead(at + own + extra, r, tag).replacen(
                                "execute ",
                                &format!("execute {only}"),
                                1
                            )
                        );
                    }
                }
                out.push((
                    format!("data/arnis/function/cars/look/{name}{b}.mcfunction"),
                    body,
                ));
            }
        }
        out.push((
            "data/arnis/function/cars/look.mcfunction".to_string(),
            dispatch,
        ));

        let mut clear = format!(
            "# Crossing traffic right in front, or anyone ahead inside a junction: give way, but\n\
             # not for ever, since cars can block each other in a circle there. Which goes first\n\
             # is down to its number. Cars standing at a red line are no one's business.\n\
             scoreboard players set #yield arnis.car 0\n\
             execute if score #jam arnis.car matches 1 run scoreboard players set #yield arnis.car 1\n\
             execute if score #t arnis.car matches 1.. if entity @s[tag=!arnis.car.long] rotated as @s positioned ^ ^ ^{crossing} if entity @e[type=marker,tag=arnis.car,tag=!arnis.car.held,distance=..{r}] run scoreboard players set #yield arnis.car 1\n\
             execute if score #t arnis.car matches 1.. if entity @s[tag=arnis.car.long] rotated as @s positioned ^ ^ ^{crossing_l} if entity @e[type=marker,tag=arnis.car,tag=!arnis.car.held,distance=..{r}] run scoreboard players set #yield arnis.car 1\n\
             execute if score #yield arnis.car matches 0 run scoreboard players set #w arnis.car 0\n\
             execute if score #yield arnis.car matches 1 run scoreboard players add #w arnis.car 1\n\
             scoreboard players operation #lim arnis.car = #id arnis.car\n\
             scoreboard players operation #lim arnis.car %= #32 arnis\n\
             scoreboard players add #lim arnis.car {GIVE_WAY_STEPS}\n\
             execute if score #yield arnis.car matches 1 if score #w arnis.car <= #lim arnis.car run scoreboard players set #t arnis.car 0\n\
             # Never through a player. A bus or truck looks from its longer front.\n\
             execute if entity @s[tag=!arnis.car.long] rotated as @s positioned ^ ^ ^{near} if entity @a[gamemode=!spectator,tag=!arnis.riding,distance=..{pr}] run scoreboard players set #t arnis.car 0\n\
             execute if entity @s[tag=!arnis.car.long] rotated as @s positioned ^ ^ ^{far} if entity @a[gamemode=!spectator,tag=!arnis.riding,distance=..{pr}] run scoreboard players set #t arnis.car 0\n\
             execute if entity @s[tag=arnis.car.long] rotated as @s positioned ^ ^ ^{near_l} if entity @a[gamemode=!spectator,tag=!arnis.riding,distance=..{pr}] run scoreboard players set #t arnis.car 0\n\
             execute if entity @s[tag=arnis.car.long] rotated as @s positioned ^ ^ ^{far_l} if entity @a[gamemode=!spectator,tag=!arnis.riding,distance=..{pr}] run scoreboard players set #t arnis.car 0\n",
            crossing = d(CROSSING_AT.0),
            crossing_l = d(CROSSING_AT.0 + LONG_EXTRA),
            r = d(CROSSING_AT.1),
            near = d(PLAYER_AT.0),
            far = d(PLAYER_AT.1),
            near_l = d(PLAYER_AT.0 + LONG_EXTRA),
            far_l = d(PLAYER_AT.1 + LONG_EXTRA),
            pr = d(PLAYER_AT.2),
        );
        if walkers {
            // Closer round than for players, so someone on the pavement alongside is no reason.
            let _ = write!(
                clear,
                "# Nor through anyone walking across.\n\
                 execute if entity @s[tag=!arnis.car.long] rotated as @s positioned ^ ^ ^{near} if entity @e[tag=arnis.walker,distance=..{wr}] run scoreboard players set #t arnis.car 0\n\
                 execute if entity @s[tag=!arnis.car.long] rotated as @s positioned ^ ^ ^{far} if entity @e[tag=arnis.walker,distance=..{wr}] run scoreboard players set #t arnis.car 0\n\
                 execute if entity @s[tag=arnis.car.long] rotated as @s positioned ^ ^ ^{near_l} if entity @e[tag=arnis.walker,distance=..{wr}] run scoreboard players set #t arnis.car 0\n\
                 execute if entity @s[tag=arnis.car.long] rotated as @s positioned ^ ^ ^{far_l} if entity @e[tag=arnis.walker,distance=..{wr}] run scoreboard players set #t arnis.car 0\n",
                near = d(PLAYER_AT.0),
                far = d(PLAYER_AT.1),
                near_l = d(PLAYER_AT.0 + LONG_EXTRA),
                far_l = d(PLAYER_AT.1 + LONG_EXTRA),
                wr = d(WALKER_AT),
            );
        }
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
        for (kind, _) in KINDS {
            let name = kind.name();
            for (prefix, bare) in [("", false), ("ov121/", true)] {
                let (width, height) = kind.hitbox();
                let mut text = format!(
                    "$summon interaction $(x) $(y) $(z) {{Tags:[\"arnis.car.part\",\"arnis.k_{name}\"],width:{:.2}f,height:{:.2}f}}\n",
                    width * k,
                    height * k,
                );
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
                        "$summon block_display $(x) $(y) $(z) {{Tags:[{tags}],teleport_duration:2,view_range:{VIEW_RANGE}f,block_state:{block},transformation:{{left_rotation:[0f,0f,0f,1f],right_rotation:[0f,0f,0f,1f],translation:{},scale:{}}}{glow}}}",
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

    /// The function that picks a new car's kind, each as often as its weight.
    pub fn kind_function(&self) -> (String, String) {
        let total: u32 = KINDS.iter().map(|&(_, weight)| weight).sum();
        let mut text = format!(
            "execute store result score #r arnis.car run random value 0..{}\n",
            total - 1
        );
        let mut from = 0;
        for (kind, weight) in KINDS {
            let _ = writeln!(
                text,
                "execute if score #r arnis.car matches {from}..{} run data modify storage arnis:cars spawn merge value {{kind:\"{}\",long:{}b}}",
                from + weight - 1,
                kind.name(),
                u8::from(kind.is_long()),
            );
            from += weight;
        }
        (
            "data/arnis/function/cars/respawn_kind.mcfunction".to_string(),
            text,
        )
    }

    /// The function that says where a rider sits in the car of the box it runs as.
    pub fn seat_function(&self) -> (String, String) {
        let mut text = String::new();
        for (kind, _) in KINDS {
            let (ahead, eye) = kind.seat();
            let _ = writeln!(
                text,
                "execute if entity @s[tag=arnis.k_{}] run data modify storage arnis:cars seat set value {{f:{:.2}d,u:{:.2}d}}",
                kind.name(),
                ahead * self.size,
                (eye * self.size - RIDER_EYE_OVER_SEAT).max(0.0),
            );
        }
        (
            "data/arnis/function/cars/seat_kind.mcfunction".to_string(),
            text,
        )
    }

    /// What the Bedrock script drives the cars by: every lane, the cars placed at the start
    /// and the scale constants. A lane's points are in tenths of a block each from the
    /// last, its turns are (lane, entry, crossing) and its caps (point, cap), all flat lists.
    /// The placed cars name their body by its place in `bodies`.
    pub fn bedrock_data(&self) -> serde_json::Value {
        let tenths = |v: f64| (v * 10.0).round() as i64;
        let lanes: Vec<serde_json::Value> = self
            .segments
            .iter()
            .map(|seg| {
                let mut p = Vec::with_capacity(seg.points.len() * 3);
                let mut last = (0, 0, 0);
                for q in &seg.points {
                    let at = (tenths(q.x), tenths(q.y), tenths(q.z));
                    p.extend([at.0 - last.0, at.1 - last.1, at.2 - last.2]);
                    last = at;
                }
                let next = seg
                    .next
                    .iter()
                    .flat_map(|t| [t.lane, t.entry, usize::from(t.crossing)]);
                let mut lane = json!({
                    "p": bedrock_addon::list(p),
                    "n": bedrock_addon::list(next),
                    "v": seg.speed,
                });
                let caps: Vec<i32> = seg
                    .points
                    .iter()
                    .enumerate()
                    .filter_map(|(k, q)| q.cap.map(|c| [k as i32, c]))
                    .flatten()
                    .collect();
                if !caps.is_empty() {
                    lane["m"] = json!(bedrock_addon::list(caps));
                }
                lane
            })
            .collect();
        let bodies = all_bodies();
        let hundredths = |v: f64| (v * 100.0).round() / 100.0;
        let start: Vec<serde_json::Value> = self
            .cars
            .iter()
            .map(|car| {
                let body = bodies
                    .iter()
                    .position(|&(kind, colour)| kind == car.kind && colour == car.colour)
                    .expect("every car is one of the bodies");
                json!([
                    car.segment,
                    car.target,
                    hundredths(car.pos.0),
                    hundredths(car.pos.1),
                    hundredths(car.pos.2),
                    body,
                    car.pace,
                ])
            })
            .collect();
        let names = |colours: &[(&'static str, u32)]| -> Vec<&'static str> {
            respawn_colours(colours)
                .into_iter()
                .map(colour_name)
                .collect()
        };
        json!({
            "size": self.size,
            "accel": self.accel(),
            "lanes": lanes,
            "start": start,
            "spots": self
                .spots
                .iter()
                .map(|((x, z), spots)| {
                    let flat = spots.iter().flat_map(|&(lane, point)| [lane, point]);
                    (format!("{x}_{z}"), json!(bedrock_addon::list(flat)))
                })
                .collect::<serde_json::Map<_, _>>(),
            "bodies": bodies.iter().map(|&(k, c)| body_name(k, c)).collect::<Vec<_>>(),
            "colours": KINDS
                .iter()
                .map(|&(kind, _)| (kind.name().to_string(), json!(names(kind.colours()))))
                .collect::<serde_json::Map<_, _>>(),
            "rules": {
                "look": LOOK.map(|(at, r, close)| json!([at, r, close])),
                "same_way": SAME_WAY_DEG,
                "long_extra": LONG_EXTRA,
                "stacked": STACKED,
                "close_ahead": [CLOSE_AHEAD.0, CLOSE_AHEAD.1],
                "long_close_ahead": [LONG_CLOSE_AHEAD.0, LONG_CLOSE_AHEAD.1],
                "long_kinds": KINDS
                    .iter()
                    .filter(|(kind, _)| kind.is_long())
                    .map(|(kind, _)| kind.name())
                    .collect::<Vec<_>>(),
                "crossing": [CROSSING_AT.0, CROSSING_AT.1],
                "player": [PLAYER_AT.0, PLAYER_AT.1, PLAYER_AT.2],
                "give_way": GIVE_WAY_STEPS,
                "pace": [PACE.0, PACE.1],
                "near": MAX_NEAR,
                "spot_cell": SPOT_CELL,
                "kinds": KINDS.map(|(kind, weight)| json!([kind.name(), weight])),
            },
        })
    }

    /// Every body a car can have on Bedrock, at the size the cars are built at.
    pub fn bedrock_bodies(&self) -> Vec<Body> {
        let k = self.size as f32;
        all_bodies()
            .into_iter()
            .map(|(kind, colour)| Body {
                name: body_name(kind, colour),
                fleet: Fleet::Car,
                shapes: parts(kind, colour)
                    .into_iter()
                    .map(|p| Shape {
                        block: p.block,
                        size: p.size.map(|v| v * k),
                        corner: p.corner.map(|v| v * k),
                        tilt: 0.0,
                        lamp: match p.light {
                            Light::None => Lamp::Off,
                            Light::Head => Lamp::On,
                            Light::Tail => Lamp::Brake,
                        },
                    })
                    .collect(),
            })
            .collect()
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
            let mut tags = vec![Value::String("arnis.car".to_string())];
            if car.kind.is_long() {
                tags.push(Value::String("arnis.car.long".to_string()));
            }
            let marker = HashMap::from([
                ("data".to_string(), Value::Compound(data)),
                ("Tags".to_string(), Value::List(tags)),
                ("Rotation".to_string(), rotation.clone()),
            ]);
            let seed = c as i64 * 32;
            editor.add_entity_exact("minecraft:marker", x, y, z, seed, marker);
            let (width, height) = car.kind.hitbox();
            let hitbox = HashMap::from([
                ("width".to_string(), Value::Float(width * self.size as f32)),
                (
                    "height".to_string(),
                    Value::Float(height * self.size as f32),
                ),
                (
                    "Tags".to_string(),
                    Value::List(vec![
                        Value::String("arnis.car.part".to_string()),
                        Value::String(format!("arnis.k_{}", car.kind.name())),
                    ]),
                ),
                ("Rotation".to_string(), rotation.clone()),
            ]);
            editor.add_entity_exact("minecraft:interaction", x, y, z, seed + 31, hitbox);
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

/// The colour list a respawned car draws from, each colour as often as its weight.
fn respawn_colours(colours: &[(&'static str, u32)]) -> Vec<&'static str> {
    colours
        .iter()
        .flat_map(|(c, w)| std::iter::repeat_n(*c, (*w as usize).div_ceil(4)))
        .collect()
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Sedan => "sedan",
            Kind::Hatchback => "hatchback",
            Kind::Van => "van",
            Kind::Pickup => "pickup",
            Kind::Truck => "truck",
            Kind::Bus => "bus",
        }
    }

    /// The colours this kind comes in, weighted.
    fn colours(self) -> &'static [(&'static str, u32)] {
        match self {
            Kind::Sedan | Kind::Hatchback => &CAR_COLOURS,
            Kind::Van => &VAN_COLOURS,
            Kind::Pickup => &PICKUP_COLOURS,
            Kind::Truck => &TRUCK_COLOURS,
            Kind::Bus => &BUS_COLOURS,
        }
    }

    /// Buses and trucks, which need more room in a queue.
    fn is_long(self) -> bool {
        matches!(self, Kind::Truck | Kind::Bus)
    }

    /// Where a rider sits, in metres: ahead of the car's middle, and how high their eyes
    /// are over the road, to look out of the windows of this kind.
    fn seat(self) -> (f64, f64) {
        match self {
            Kind::Sedan => (0.4, 1.15),
            Kind::Hatchback => (0.1, 1.2),
            Kind::Van => (1.25, 1.42),
            Kind::Pickup => (0.95, 1.28),
            Kind::Truck => (2.7, 1.95),
            Kind::Bus => (3.9, 2.0),
        }
    }

    /// Width and height at 1:1 of the box a player can click. A spectator sees from the
    /// middle of the car at 85% of the height.
    fn hitbox(self) -> (f32, f32) {
        match self {
            Kind::Sedan => (2.4, 1.45),
            Kind::Hatchback => (2.4, 1.5),
            Kind::Van => (2.6, 1.8),
            Kind::Pickup => (2.6, 1.6),
            Kind::Truck => (3.0, 2.7),
            Kind::Bus => (3.0, 2.6),
        }
    }
}

/// Every kind of car in each of its colours.
fn all_bodies() -> Vec<(Kind, &'static str)> {
    KINDS
        .iter()
        .flat_map(|&(kind, _)| kind.colours().iter().map(move |(c, _)| (kind, *c)))
        .collect()
}

/// `red` for red concrete.
fn colour_name(block: &str) -> &str {
    block
        .trim_start_matches("minecraft:")
        .trim_end_matches("_concrete")
}

/// The Bedrock entity name of a car body, as the script also puts it together.
fn body_name(kind: Kind, colour: &str) -> String {
    format!("car_{}_{}", kind.name(), colour_name(colour))
}

/// Splits a stretch until no waypoint is more than `MAX_SPAN` from the next and the
/// ground in between is within a block of the straight line, so cars keep to the road.
pub(crate) fn refine(
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
    // Any lane of the same piece going the other way.
    let goes_back = |i: usize, j: usize| {
        lanes[i].piece == lanes[j].piece && lanes[i].backward != lanes[j].backward
    };
    // A road's lanes all start at the same node, and a car keeps to the nearest one of each.
    let keep_to_nearest = |i: usize, mut next: Vec<usize>| {
        let from = segments[i].points.last().map_or((0.0, 0.0), |p| (p.x, p.z));
        let gap = |j: usize| {
            segments[j]
                .points
                .first()
                .map_or(f64::MAX, |p| (p.x - from.0).hypot(p.z - from.1))
        };
        next.sort_by(|&a, &b| gap(a).total_cmp(&gap(b)));
        let mut seen: Vec<(usize, bool)> = Vec::new();
        next.retain(|&j| {
            let road = (lanes[j].piece, lanes[j].backward);
            !seen.contains(&road) && {
                seen.push(road);
                true
            }
        });
        next
    };
    let ways_on: Vec<Vec<usize>> = lanes
        .iter()
        .enumerate()
        .map(|(i, lane)| {
            let end = lane.nodes[lane.nodes.len() - 1].id;
            let mut next: Vec<usize> = keep_to_nearest(
                i,
                starting
                    .get(&end)
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|&j| !goes_back(i, j))
                    .collect(),
            );
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
    for i in 0..lanes.len() {
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
                        .filter(|&&k| k != i && !goes_back(i, k))
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
    // A drawn outline's edge counts too: roads are cut there as at the rectangle's.
    let r = EDGE_REACH.ceil() as i32;
    let off_outline = |p: &Waypoint| {
        let (x, z) = (p.x.floor() as i32, p.z.floor() as i32);
        world.outline().is_some() && !world.covers_rect(x - r, z - r, x + r, z + r)
    };
    let at_edge = |p: &Waypoint| {
        near(p.x, world.min_x(), world.max_x())
            || near(p.z, world.min_z(), world.max_z())
            || off_outline(p)
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
            let kind = pick(&KINDS, roll);
            let colours = kind.colours();
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
                pace: PACE.0 + ((roll >> 40) % (PACE.1 - PACE.0 + 1) as u64) as i32,
            });
        }
    }
    cars
}

/// Where new cars can join the traffic: every lane's spots as far apart as its cars are in
/// full traffic, each at a point of the lane, so the count around a player is what the roads
/// there should hold however thin the placed cars had to be spread.
fn spots(segments: &[Segment], scale: f64) -> BTreeMap<(i32, i32), Vec<(usize, usize)>> {
    let mut grid: BTreeMap<(i32, i32), Vec<(usize, usize)>> = BTreeMap::new();
    let mut owed = 0.5;
    for (i, seg) in segments.iter().enumerate() {
        if !seg.carries_cars() {
            continue;
        }
        let (first, last) = (seg.arcs[0], seg.arcs[seg.arcs.len() - 1]);
        let len = last - first;
        owed += len / scale * seg.density;
        let n = owed.floor() as usize;
        owed -= n as f64;
        let mut was = None;
        for j in 0..n {
            let s = first + len * (j as f64 + 0.5) / n as f64;
            let point = seg
                .arcs
                .partition_point(|&a| a <= s)
                .clamp(1, seg.points.len() - 1)
                - 1;
            if was == Some(point) {
                continue;
            }
            was = Some(point);
            let p = seg.points[point];
            let cell = (
                (p.x / SPOT_CELL).floor() as i32,
                (p.z / SPOT_CELL).floor() as i32,
            );
            grid.entry(cell).or_default().push((i, point));
        }
    }
    grid
}

/// Display entities are drawn out to 64 blocks times this, and sent out to 160 blocks.
const VIEW_RANGE: f32 = 3.0;

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
        Kind::Pickup => (5.3, 1.95, 1.0, 0.74, 0.78),
        Kind::Truck => (7.6, 2.4, 1.25, 0.95, 0.8),
        Kind::Bus => (10.0, 2.5, 2.2, 0.98, 0.8),
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
    let plate = |z: f32| {
        part(
            "minecraft:white_concrete",
            [0.52, 0.11, 0.02],
            [-0.26, lights - 0.2, z],
            Light::None,
        )
    };
    let mut out = vec![
        wheels(l / 2.0 - axle),
        wheels(-l / 2.0 + axle),
        lamp("minecraft:white_concrete", l / 2.0, Light::Head),
        lamp("minecraft:red_concrete", -l / 2.0 - 0.04, Light::Tail),
        plate(l / 2.0),
        plate(-l / 2.0 - 0.02),
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
        // A cab up front and an open bed behind it.
        Kind::Pickup => out.extend([
            part(
                colour,
                [w, 0.62, l],
                [-w / 2.0, 0.36, -l / 2.0],
                Light::None,
            ),
            part(
                "minecraft:tinted_glass",
                [w - 0.2, 0.56, 2.1],
                [-(w - 0.2) / 2.0, 0.98, -0.4],
                Light::None,
            ),
            part(
                colour,
                [w - 0.18, 0.07, 1.9],
                [-(w - 0.18) / 2.0, 1.54, -0.3],
                Light::None,
            ),
        ]),
        // A coloured cab and a plain box behind it.
        Kind::Truck => out.extend([
            part(
                colour,
                [w, 1.95, 1.95],
                [-w / 2.0, 0.55, l / 2.0 - 1.95],
                Light::None,
            ),
            part(
                "minecraft:tinted_glass",
                [w + 0.04, 0.62, 0.95],
                [-(w + 0.04) / 2.0, 1.65, l / 2.0 - 0.93],
                Light::None,
            ),
            part(
                "minecraft:light_gray_concrete",
                [w + 0.06, 2.4, 5.5],
                [-(w + 0.06) / 2.0, 0.85, -l / 2.0],
                Light::None,
            ),
        ]),
        // One long box with a band of windows round it.
        Kind::Bus => out.extend([
            part(
                colour,
                [w, 2.55, l],
                [-w / 2.0, 0.35, -l / 2.0],
                Light::None,
            ),
            part(
                "minecraft:tinted_glass",
                [w + 0.04, 0.95, l - 0.28],
                [-(w + 0.04) / 2.0, 1.5, -l / 2.0 + 0.3],
                Light::None,
            ),
        ]),
    }
    // Windows and bumpers stand a hair proud of the body, so the two never flicker.
    let mut boxes: Vec<crate::element_processing::train_bodies::Part> = out
        .iter()
        .map(|p| crate::element_processing::train_bodies::Part {
            block: p.block,
            size: p.size,
            corner: p.corner,
            tilt: 0.0,
            glow: false,
        })
        .collect();
    crate::element_processing::ship_bodies::apart(&mut boxes);
    for (p, b) in out.iter_mut().zip(boxes) {
        (p.size, p.corner) = (b.size, b.corner);
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
                ("view_range".to_string(), Value::Float(VIEW_RANGE)),
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

    #[test]
    fn no_two_parts_of_a_car_flicker_against_each_other_or_hide_one_another() {
        use crate::element_processing::train_bodies::Part as Box;
        let mut found = Vec::new();
        for kind in [
            Kind::Sedan,
            Kind::Hatchback,
            Kind::Van,
            Kind::Pickup,
            Kind::Truck,
            Kind::Bus,
        ] {
            let parts: Vec<Box> = parts(kind, "minecraft:red_concrete")
                .iter()
                .map(|p| Box {
                    block: p.block,
                    size: p.size,
                    corner: p.corner,
                    tilt: 0.0,
                    glow: false,
                })
                .collect();
            for (i, a) in parts.iter().enumerate() {
                for b in &parts[i + 1..] {
                    for f in crate::element_processing::ship_bodies::shared_faces(a, b) {
                        found.push(format!("{kind:?} {} / {} {f:?}", a.block, b.block));
                    }
                }
            }
            for b in crate::element_processing::ship_bodies::buried(&parts) {
                found.push(format!("{kind:?} {b}"));
            }
        }
        assert!(found.is_empty(), "{}", found.join("\n"));
    }

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

    fn flat(_: &ProcessedWay, _: i32, _: i32) -> i32 {
        64
    }

    fn open(_: i32, _: i32) -> bool {
        false
    }

    #[test]
    fn a_wide_road_gets_a_lane_for_each_of_its_lanes() {
        let tagged = |tags: &[(&str, &str)], half: i32, oneway: i8| {
            let way = road(
                5,
                vec![node(1, 0, 0, &[]), node(2, 100, 0, &[])],
                &[&[("highway", "primary")], tags].concat(),
            );
            let ProcessedElement::Way(way) = &way else {
                unreachable!()
            };
            lanes_each_way(way, half, oneway)
        };
        // A wide one-way avenue, a two-way road with lanes each way, and the plain cases.
        assert_eq!(tagged(&[("lanes", "4")], 7, 1), (4, 0));
        assert_eq!(tagged(&[("lanes", "4")], 7, -1), (0, 4));
        assert_eq!(tagged(&[("lanes", "4")], 7, 0), (2, 2));
        assert_eq!(tagged(&[("lanes", "3")], 7, 0), (2, 1));
        assert_eq!(tagged(&[("lanes", "2")], 5, 0), (1, 1));
        assert_eq!(tagged(&[("lanes", "1")], 5, 1), (1, 0));
        // Tags that ask for more than the road has room for get what fits.
        assert_eq!(tagged(&[("lanes", "6")], 2, 1), (1, 0));
        assert_eq!(tagged(&[("lanes", "8")], 16, 1), (MAX_LANES, 0));
        assert_eq!(
            tagged(
                &[
                    ("lanes", "5"),
                    ("lanes:forward", "3"),
                    ("lanes:backward", "2")
                ],
                9,
                0
            ),
            (3, 2)
        );

        // A one-way road's lanes spread across it, with none on the kerb.
        let offsets: Vec<f64> = (0..3).map(|k| lane_offset(5, 3, k, false)).collect();
        assert!(offsets[0] < 0.0 && offsets[1].abs() < 0.01 && offsets[2] > 0.0);
        assert!(offsets[2] + CAR_HALF_WIDTH < 5.5);
        for pair in offsets.windows(2) {
            assert!(pair[1] - pair[0] >= f64::from(MIN_LANE_WIDTH));
        }
        // A single lane on a two-way road keeps its place, and one on a one-way road the middle.
        assert_eq!(lane_offset(5, 1, 0, true), 3.0);
        assert_eq!(lane_offset(5, 1, 0, false), 0.0);
    }

    #[test]
    fn cars_keep_to_their_lane_and_never_turn_back_from_one() {
        let way = |id, from: i32, to: i32| {
            road(
                id,
                vec![node(id * 10, from, 0, &[]), node(id * 10 + 1, to, 0, &[])],
                &[("lanes", "4")],
            )
        };
        // The second way goes on from where the first ends; the two share a node.
        let mut second = way(2, 200, 400);
        if let ProcessedElement::Way(w) = &mut second {
            w.nodes[0].id = 11;
        }
        let net = CarNetwork::build(
            &[way(1, 0, 200), second],
            1.0,
            false,
            false,
            &world(),
            flat,
            open,
        );
        assert_eq!(net.segments.len(), 8, "two lanes each way on each way");
        let side = |s: &Segment| s.points[0].z;
        for (i, seg) in net.segments.iter().enumerate() {
            for j in seg.next.iter().map(|t| t.lane) {
                let next = &net.segments[j];
                if next.way != seg.way {
                    assert!(
                        (side(seg) - side(next)).abs() < 0.6,
                        "lane {i} jumps across the road"
                    );
                }
                // Same way, going the other way, only as a turn round at a dead end.
                let heading = |s: &Segment| s.points[s.points.len() - 1].x - s.points[0].x;
                if next.way == seg.way && !seg.turns_back {
                    assert!(heading(seg) * heading(next) > 0.0, "lane {i} turns back");
                }
            }
        }
    }

    #[test]
    fn cars_stay_in_drawn_areas_and_never_cross_the_gap() {
        // Two areas with a gap between them, one long road through both.
        let world = world()
            .with_outline(vec![
                vec![vec![
                    (-2000.0, -50.0),
                    (-300.0, -50.0),
                    (-300.0, 50.0),
                    (-2000.0, 50.0),
                ]],
                vec![vec![
                    (300.0, -50.0),
                    (2000.0, -50.0),
                    (2000.0, 50.0),
                    (300.0, 50.0),
                ]],
            ])
            .unwrap();
        let way = road(
            1,
            vec![node(10, -1500, 0, &[]), node(11, 1500, 0, &[])],
            &[("lanes", "2")],
        );
        let net = CarNetwork::build(&[way], 1.0, false, false, &world, flat, open);
        assert!(!net.segments.is_empty());
        for seg in &net.segments {
            let (a, b) = (seg.points[0].x, seg.points[seg.points.len() - 1].x);
            assert!(
                a.signum() == b.signum(),
                "a lane runs across the gap: {a} -> {b}"
            );
            for w in seg.points.windows(2) {
                assert!(
                    (w[1].x - w[0].x).abs() < 50.0,
                    "a jump of {}",
                    w[1].x - w[0].x
                );
            }
            for p in &seg.points {
                let at = XZPoint::new(p.x.floor() as i32, p.z.floor() as i32);
                assert!(world.contains(&at), "{at:?}");
            }
        }
    }

    #[test]
    fn buses_and_trucks_keep_their_length_behind_what_is_ahead() {
        let net = CarNetwork::build(&crossroads(false), 1.0, false, false, &world(), flat, open);
        let looks = net.look_functions(false);
        let text = |name: &str| {
            looks
                .iter()
                .find(|(p, _)| p.ends_with(name))
                .map(|(_, t)| t.clone())
                .unwrap()
        };
        // A long one looks further ahead, and anyone looks further for a long one.
        let (car, long) = (text("look/h0.mcfunction"), text("look/l0.mcfunction"));
        assert!(car.contains(&format!(
            "^{:.2} if entity @e[type=marker,tag=arnis.car.long",
            LOOK[0].0 + LONG_EXTRA
        )));
        assert!(long.contains(&format!(
            "^{:.2} if entity @e[type=marker,tag=arnis.car,",
            LOOK[0].0 + LONG_EXTRA
        )));
        // Two buses: the follower is held from further off than their bodies need.
        let bus = 10.0;
        let held_from = LOOK
            .iter()
            .filter(|&&(_, _, close)| close == 3)
            .map(|&(at, r, _)| at + 2.0 * LONG_EXTRA + r)
            .fold(0.0, f64::max);
        assert!(held_from >= bus + 1.0, "{held_from}");
        assert!(
            text("look.mcfunction").contains("tag=arnis.car.long] run function arnis:cars/look/l0")
        );
    }

    #[test]
    fn a_rider_sits_towards_the_front_at_window_height() {
        for (kind, _) in KINDS {
            let (ahead, eye) = kind.seat();
            assert!(ahead >= 0.0, "{}", kind.name());
            assert!((1.0..2.2).contains(&eye), "{}", kind.name());
        }
        let net = CarNetwork::build(&crossroads(false), 1.0, false, false, &world(), flat, open);
        let (path, text) = net.seat_function();
        assert!(path.ends_with("cars/seat_kind.mcfunction"));
        assert_eq!(text.lines().count(), KINDS.len());
        assert!(text.contains("arnis.k_bus"));
    }

    #[test]
    fn a_lane_keeps_out_of_a_building_standing_on_the_road() {
        // Traffic drives on the right, here towards +z along x = 0, so a car's right is -x
        // going one way and +x the other. A wall along part of the road's +x side.
        let way = road(
            5,
            (0..=10)
                .map(|i| node(i + 1, 0, i as i32 * 20, &[]))
                .collect(),
            &[("width", "8")],
        );
        // An 8 m road: its lanes 2.5 either side of the middle, room to move in by one.
        let wall = |x: i32, z: i32| x >= 3 && (40..=100).contains(&z);
        let net = CarNetwork::build(
            std::slice::from_ref(&way),
            1.0,
            false,
            false,
            &world(),
            flat,
            wall,
        );
        let plain = CarNetwork::build(&[way], 1.0, false, false, &world(), flat, open);
        // The side away from the wall stays where it was, and the side beside it moved in.
        let side = |net: &CarNetwork, sign: f64| -> Vec<(f64, f64)> {
            net.segments
                .iter()
                .flat_map(|s| s.points.iter().map(|p| (p.x, p.z)))
                .filter(|p| p.0 * sign > 0.5)
                .collect()
        };
        let wall_side = side(&net, 1.0);
        assert!(!wall_side.is_empty());
        for &(x, z) in &wall_side {
            if (40.0..=100.0).contains(&z) {
                assert!(x + CAR_HALF_WIDTH < 3.0, "still in the wall at {x},{z}");
                assert!(
                    x - 0.5 >= ONCOMING_OFFSET,
                    "in the way of oncoming cars at {x},{z}"
                );
            }
        }
        let beside_the_wall = |net: &CarNetwork| -> Vec<(f64, f64)> {
            side(net, -1.0)
                .into_iter()
                .filter(|p| (40.0..=100.0).contains(&p.1))
                .collect()
        };
        assert_eq!(beside_the_wall(&net), beside_the_wall(&plain));
        // Away from the wall the lane keeps its usual place.
        let usual = 3.0;
        assert!(wall_side
            .iter()
            .any(|&(x, z)| (130.0..=180.0).contains(&z) && (x - usual).abs() < 0.01));
    }

    /// A kind's body: its length and width, in blocks at 1:1.
    fn footprint(kind: Kind) -> (f64, f64) {
        let parts = parts(kind, "minecraft:white_concrete");
        let extent = |axis: usize| {
            let lo = parts
                .iter()
                .map(|p| p.corner[axis])
                .fold(f32::MAX, f32::min);
            let hi = parts
                .iter()
                .map(|p| p.corner[axis] + p.size[axis])
                .fold(f32::MIN, f32::max);
            f64::from(hi - lo)
        };
        (extent(2), extent(0))
    }

    /// What a driver makes of the car ahead `gap` blocks off, middle to middle, as the
    /// pack's `look/h` and `look/l` functions lay it out: 3 stop, 2 and 1 ease off.
    fn sees_ahead(follower: Kind, leader: Kind, gap: f64) -> u8 {
        let own = if follower.is_long() { LONG_EXTRA } else { 0.0 };
        for &(at, r, close) in &LOOK {
            for (extra, applies) in [(0.0, true), (LONG_EXTRA, leader.is_long())] {
                if applies && (gap - (at + own + extra)).abs() <= r {
                    return close;
                }
            }
        }
        0
    }

    /// Drives a queue along one straight lane towards a car standing at a red light, a step
    /// at a time as `step.mcfunction` does, and returns the closest any two bodies came.
    fn drive_a_queue(kinds: &[Kind], kmh: f64) -> (f64, Vec<f64>) {
        let accel = ((ACCEL_MPS2 * 0.1 * 0.1 * 1000.0).round() as i32).max(1);
        // The fastest drivers, so they catch up with whatever is ahead.
        let lane = step_speed(kmh * LIMIT_SHARE, 1.0) * PACE.1 / 1000;
        // Middles along the lane, the one standing at the line first, 40 blocks apart.
        let mut at: Vec<f64> = (0..kinds.len()).map(|i| -40.0 * i as f64).collect();
        let mut speed: Vec<i32> = (0..kinds.len())
            .map(|i| if i == 0 { 0 } else { lane })
            .collect();
        let mut closest = f64::MAX;
        for _ in 0..3000 {
            for i in 1..kinds.len() {
                let gap = at[i - 1] - at[i];
                let mut t = lane;
                match sees_ahead(kinds[i], kinds[i - 1], gap) {
                    3 => t = 0,
                    2 => t = t.min(lane / 4),
                    1 => t = t.min(lane / 2),
                    _ => {}
                }
                speed[i] = (speed[i] + accel).min(t);
                at[i] += f64::from(speed[i].max(0)) / 1000.0;
            }
            for i in 1..kinds.len() {
                let room = (footprint(kinds[i]).0 + footprint(kinds[i - 1]).0) / 2.0;
                closest = closest.min(at[i - 1] - at[i] - room);
            }
        }
        let gaps = (1..kinds.len())
            .map(|i| at[i - 1] - at[i] - (footprint(kinds[i]).0 + footprint(kinds[i - 1]).0) / 2.0)
            .collect();
        (closest, gaps)
    }

    /// Drives cars along one lane from where they stand, in order of their ids, with the
    /// stacked rule and the look ahead, and returns where each one got to.
    fn drive_from(cars: &[(Kind, f64)], kmh: f64, steps: usize) -> Vec<f64> {
        let accel = ((ACCEL_MPS2 * 0.1 * 0.1 * 1000.0).round() as i32).max(1);
        let lane = step_speed(kmh * LIMIT_SHARE, 1.0);
        let mut at: Vec<f64> = cars.iter().map(|c| c.1).collect();
        let mut speed = vec![lane; cars.len()];
        for _ in 0..steps {
            for i in 0..cars.len() {
                let (c, r) = CLOSE_AHEAD;
                let (lc, lr) = LONG_CLOSE_AHEAD;
                let in_front = |j: usize| {
                    let d = at[j] - at[i];
                    j != i && ((d - c).abs() <= r || cars[i].0.is_long() && (d - lc).abs() <= lr)
                };
                let older_beside = |j: usize| {
                    let d = at[j] - at[i];
                    d.abs() <= STACKED && (d + c).abs() > r
                };
                let close = if (0..cars.len()).any(in_front) || (0..i).any(older_beside) {
                    3
                } else {
                    (0..cars.len())
                        .filter(|&j| j != i && at[j] > at[i])
                        .map(|j| sees_ahead(cars[i].0, cars[j].0, at[j] - at[i]))
                        .max()
                        .unwrap_or(0)
                };
                let t = match close {
                    3 => 0,
                    2 => lane / 4,
                    1 => lane / 2,
                    _ => lane,
                };
                speed[i] = (speed[i] + accel).min(t);
                at[i] += f64::from(speed[i].max(0)) / 1000.0;
            }
        }
        at
    }

    #[test]
    fn cars_on_one_spot_come_apart_and_none_is_left_waiting() {
        let mixes: [&[(Kind, f64)]; 4] = [
            &[(Kind::Sedan, 0.0), (Kind::Sedan, 0.0)],
            // The older one a little behind, as two lanes merge.
            &[
                (Kind::Van, -0.6),
                (Kind::Hatchback, 0.0),
                (Kind::Sedan, 0.3),
            ],
            &[(Kind::Bus, 0.0), (Kind::Sedan, 0.1), (Kind::Truck, -0.2)],
            &[
                (Kind::Sedan, 0.5),
                (Kind::Pickup, 0.0),
                (Kind::Bus, 0.2),
                (Kind::Sedan, -0.4),
            ],
        ];
        for cars in mixes {
            for kmh in [30.0, 50.0, 100.0] {
                let at = drive_from(cars, kmh, 1500);
                assert!(at.iter().all(|&a| a > 100.0), "{cars:?} at {kmh}: {at:?}");
                let mut order: Vec<usize> = (0..cars.len()).collect();
                order.sort_by(|&a, &b| at[b].total_cmp(&at[a]));
                for w in order.windows(2) {
                    let room = (footprint(cars[w[0]].0).0 + footprint(cars[w[1]].0).0) / 2.0;
                    let gap = at[w[0]] - at[w[1]] - room;
                    assert!(gap >= 0.3, "{cars:?} at {kmh}: {gap:.2} between {w:?}");
                }
            }
        }
        // Right in front reaches on to where the look ahead starts, leaves out the car's own
        // middle, and with the spot it shares takes in every gap from nothing up.
        let nearest_look = LOOK
            .iter()
            .map(|&(at, r, _)| at - r)
            .fold(f64::MAX, f64::min);
        let (c, r) = CLOSE_AHEAD;
        assert!(c + r >= nearest_look && c - r > 0.0);
        assert!(STACKED >= c - r);
        // A bus or truck's own window goes on to where its longer look ahead starts, and stays
        // beyond the spot it shares, so neither of two there waits for the other.
        let (lc, lr) = LONG_CLOSE_AHEAD;
        assert!(lc - lr <= c + r && lc + lr >= nearest_look + LONG_EXTRA);
        assert!(lc - lr > STACKED);
        // Lanes side by side and oncoming ones are neither right in front nor one spot.
        for reach in [STACKED, r, lr] {
            assert!(reach < 2.0 * CAR_HALF_WIDTH);
        }
        let net = CarNetwork::build(&crossroads(false), 1.0, false, false, &world(), flat, open);
        let look = net
            .look_functions(false)
            .into_iter()
            .find(|(p, _)| p.ends_with("cars/look.mcfunction"))
            .unwrap()
            .1;
        assert!(look.lines().next().is_some_and(|l| l.starts_with('#')));
        assert!(look.contains(&format!(
            "execute rotated as @s positioned ^ ^ ^{c:.2} if entity @e[type=marker,tag=arnis.car,distance=..{r:.2}] run return run"
        )));
        assert!(look.contains(&format!(
            "execute rotated as @s as @e[type=marker,tag=arnis.car,distance=..{STACKED:.2}] if score @s arnis.car < #id arnis.car positioned ^ ^ ^-{c:.2} unless entity @s[distance=..{r:.2}] run return run"
        )));
        let bedrock = include_str!("../../assets/minecraft/bedrock_addon/scripts/cars.js");
        assert!(bedrock
            .contains("const older = (o) => o.id < car.id && distance(o, behind) > closeRound;"));
        let rules = &net.bedrock_data()["rules"];
        assert_eq!(rules["stacked"], STACKED);
        assert_eq!(rules["close_ahead"][1], CLOSE_AHEAD.1);
    }

    #[test]
    fn a_queue_never_runs_into_what_is_ahead_whatever_the_kinds_and_speed() {
        for (leader, _) in KINDS {
            for (follower, _) in KINDS {
                for kmh in [30.0, 50.0, 70.0, 100.0, 130.0] {
                    let (closest, gaps) = drive_a_queue(&[leader, follower], kmh);
                    assert!(
                        closest >= 0.3,
                        "{follower:?} behind {leader:?} at {kmh} km/h came within {closest:.2}"
                    );
                    // And it does close up, rather than stop a street short.
                    assert!(
                        gaps[0] <= 6.0,
                        "{follower:?} behind {leader:?}: {:.1}",
                        gaps[0]
                    );
                }
            }
        }
        // A long queue of every kind, each braking for the one braking ahead of it.
        let mixed = [
            Kind::Sedan,
            Kind::Bus,
            Kind::Hatchback,
            Kind::Truck,
            Kind::Van,
            Kind::Bus,
            Kind::Pickup,
            Kind::Truck,
            Kind::Sedan,
        ];
        for kmh in [50.0, 100.0] {
            let (closest, gaps) = drive_a_queue(&mixed, kmh);
            assert!(closest >= 0.3, "{closest:.2} at {kmh} km/h");
            assert!(gaps.iter().all(|&g| g <= 6.0), "{gaps:?}");
        }
    }

    /// Points every block along a lane's line, between `from` and `to` in z.
    fn along(seg: &Segment, from: f64, to: f64) -> Vec<(f64, f64)> {
        seg.points
            .windows(2)
            .flat_map(|w| {
                let n = (w[1].x - w[0].x).hypot(w[1].z - w[0].z).ceil().max(1.0) as usize;
                (0..n).map(move |i| {
                    let t = i as f64 / n as f64;
                    (
                        w[0].x + (w[1].x - w[0].x) * t,
                        w[0].z + (w[1].z - w[0].z) * t,
                    )
                })
            })
            .filter(|p| (from..=to).contains(&p.1))
            .collect()
    }

    #[test]
    fn lanes_side_by_side_or_oncoming_never_touch() {
        let mut cases: Vec<Vec<(&str, &str)>> = vec![vec![]];
        for highway in ["residential", "tertiary", "secondary", "primary"] {
            for lanes in ["2", "3", "4", "6"] {
                cases.push(vec![("highway", highway), ("lanes", lanes)]);
                cases.push(vec![
                    ("highway", highway),
                    ("lanes", lanes),
                    ("oneway", "yes"),
                ]);
            }
        }
        cases.push(vec![
            ("highway", "primary"),
            ("lanes:forward", "2"),
            ("lanes:backward", "1"),
        ]);
        for tags in cases {
            let way = road(
                5,
                (0..=10)
                    .map(|i| node(i + 1, 0, i as i32 * 20, &[]))
                    .collect(),
                &tags,
            );
            // Open, and then with buildings standing on the road closer and closer in.
            for kerb in [
                None,
                Some(8),
                Some(6),
                Some(5),
                Some(4),
                Some(3),
                Some(2),
                Some(1),
            ] {
                let wall =
                    |x: i32, z: i32| kerb.is_some_and(|k| x.abs() >= k) && (40..=140).contains(&z);
                let net = CarNetwork::build(
                    std::slice::from_ref(&way),
                    1.0,
                    false,
                    false,
                    &world(),
                    flat,
                    wall,
                );
                for (i, a) in net.segments.iter().enumerate() {
                    for b in net.segments.iter().skip(i + 1) {
                        let theirs = along(b, 0.0, 200.0);
                        for (x, z) in along(a, 30.0, 170.0) {
                            let apart = theirs
                                .iter()
                                .map(|q| (q.0 - x).hypot(q.1 - z))
                                .fold(f64::MAX, f64::min);
                            assert!(
                                apart >= 2.0 * CAR_HALF_WIDTH - 1e-9,
                                "{tags:?}, walls from {kerb:?}: lanes {apart:.2} apart at {x:.1},{z:.1}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// A placed car: its middle, its heading and its kind.
    type Placed = ((f64, f64), (f64, f64), Kind);

    /// Whether two cars' bodies, each a rectangle about its middle along its heading, meet.
    fn bodies_meet(a: Placed, b: Placed) -> bool {
        let corners = |((x, z), (dx, dz), kind): Placed| {
            let (l, w) = footprint(kind);
            let (sx, sz) = (-dz, dx);
            [(1.0, 1.0), (1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0)].map(|(f, s)| {
                (
                    x + dx * f * l / 2.0 + sx * s * w / 2.0,
                    z + dz * f * l / 2.0 + sz * s * w / 2.0,
                )
            })
        };
        let (ca, cb) = (corners(a), corners(b));
        let axes = [a.1, (-a.1 .1, a.1 .0), b.1, (-b.1 .1, b.1 .0)];
        axes.iter().all(|&(ax, az)| {
            let span = |c: &[(f64, f64); 4]| {
                c.iter()
                    .map(|p| p.0 * ax + p.1 * az)
                    .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)))
            };
            let ((alo, ahi), (blo, bhi)) = (span(&ca), span(&cb));
            ahi > blo && bhi > alo
        })
    }

    #[test]
    fn cars_placed_at_the_start_stand_clear_of_each_other() {
        let wide = vec![road(
            90,
            vec![node(900, -300, 0, &[]), node(901, 300, 0, &[])],
            &[("highway", "primary"), ("lanes", "6")],
        )];
        for roads in [awkward_roads(), crossroads(true), wide] {
            let net = CarNetwork::build(&roads, 1.0, false, true, &world(), flat, open);
            assert!(net.car_count() > 5, "{}", net.car_count());
            let placed: Vec<Placed> = net
                .cars
                .iter()
                .map(|c| {
                    let seg = &net.segments[c.segment];
                    let (a, b) = (seg.points[c.target - 1], seg.points[c.target]);
                    let heading = unit(b.x - a.x, b.z - a.z).unwrap_or((1.0, 0.0));
                    ((c.pos.0, c.pos.2), heading, c.kind)
                })
                .collect();
            for (i, &a) in placed.iter().enumerate() {
                for &b in &placed[i + 1..] {
                    assert!(!bodies_meet(a, b), "{a:?} and {b:?} stand in each other");
                }
            }
        }
        // The check itself: two buses nose to tail a block apart meet, and side by side in
        // lanes 3.5 apart do not.
        let bus = |x: f64, z: f64| ((x, z), (1.0, 0.0), Kind::Bus);
        assert!(bodies_meet(bus(0.0, 0.0), bus(9.0, 0.0)));
        assert!(!bodies_meet(bus(0.0, 0.0), bus(0.0, 3.5)));
    }

    #[test]
    fn new_cars_start_clear_of_every_kind() {
        let longest = KINDS
            .iter()
            .map(|&(kind, _)| footprint(kind).0)
            .fold(0.0, f64::max);
        let java = include_str!(
            "../../assets/minecraft/datapack_extras/data/arnis/function/cars/respawn_at.mcfunction"
        );
        let gap: f64 = java
            .split("tag=arnis.car,distance=..")
            .nth(1)
            .and_then(|r| r.split(']').next())
            .and_then(|n| n.parse().ok())
            .expect("respawn_at keeps new cars apart");
        let bedrock = include_str!("../../assets/minecraft/bedrock_addon/scripts/cars.js");
        let bedrock_gap: f64 = bedrock
            .split("const RESPAWN_GAP = ")
            .nth(1)
            .and_then(|r| r.split(';').next())
            .and_then(|n| n.parse().ok())
            .expect("cars.js keeps new cars apart");
        // Middle to middle, two of the longest end to end.
        for g in [gap, bedrock_gap] {
            assert!(g >= longest + 1.0, "{g} for bodies {longest} long");
        }
    }

    #[test]
    fn a_rider_is_not_in_the_way_of_their_own_car() {
        let net = CarNetwork::build(&crossroads(false), 1.0, false, false, &world(), flat, open);
        let clear = net
            .look_functions(false)
            .into_iter()
            .find(|(p, _)| p.ends_with("cars/clear.mcfunction"))
            .unwrap()
            .1;
        let players: Vec<&str> = clear.lines().filter(|l| l.contains("@a[")).collect();
        assert_eq!(players.len(), 4);
        assert!(players.iter().all(|l| l.contains("tag=!arnis.riding")));
        // Who rides is known from the start of a ride and every second after.
        let seat = include_str!(
            "../../assets/minecraft/datapack_extras/data/arnis/function/cars/seat_new.mcfunction"
        );
        let respawn = include_str!(
            "../../assets/minecraft/datapack_extras/data/arnis/function/cars/respawn.mcfunction"
        );
        for text in [seat, respawn] {
            assert!(text.contains("on passengers run tag @s add arnis.riding"));
        }
        // Without the tag the seat of the longer kinds would stand in the car's own way.
        let (near, _, reach) = PLAYER_AT;
        let own = |kind: Kind| if kind.is_long() { LONG_EXTRA } else { 0.0 };
        assert!(KINDS
            .iter()
            .any(|&(kind, _)| (kind.seat().0 - near - own(kind)).abs() <= reach));
    }

    #[test]
    fn every_kind_looks_out_for_people_and_crossing_traffic_from_its_own_front() {
        let own = |kind: Kind| if kind.is_long() { LONG_EXTRA } else { 0.0 };
        for (kind, _) in KINDS {
            let front = footprint(kind).0 / 2.0;
            // From its bumper on, with no gap, to a few metres out.
            let (near, far, r) = PLAYER_AT;
            assert!(near + own(kind) - r <= front, "{kind:?}");
            assert!(far - near <= 2.0 * r);
            assert!(far + own(kind) + r - front >= 4.0, "{kind:?}");
            let (at, r) = CROSSING_AT;
            assert!(at + own(kind) + r - front >= 1.5, "{kind:?}");
        }
        let net = CarNetwork::build(&crossroads(false), 1.0, false, false, &world(), flat, open);
        let clear = net
            .look_functions(false)
            .into_iter()
            .find(|(p, _)| p.ends_with("cars/clear.mcfunction"))
            .unwrap()
            .1;
        for at in [CROSSING_AT.0, PLAYER_AT.0, PLAYER_AT.1] {
            assert!(clear.contains(&format!(
                "[tag=!arnis.car.long] rotated as @s positioned ^ ^ ^{at:.2} "
            )));
            assert!(clear.contains(&format!(
                "[tag=arnis.car.long] rotated as @s positioned ^ ^ ^{:.2} ",
                at + LONG_EXTRA
            )));
        }
        let bedrock = include_str!("../../assets/minecraft/bedrock_addon/scripts/cars.js");
        assert!(bedrock.contains("const front = car.long ? this.rules.long_extra * k : 0;"));
        assert_eq!(bedrock.matches("* k + front)").count(), 3);
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
        let net = CarNetwork::build(&crossroads(false), 1.0, false, false, &world(), flat, open);
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
            let net = CarNetwork::build(&crossroads(false), 1.0, left, false, &world(), flat, open);
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
        let net = CarNetwork::build(&crossroads(true), 1.0, false, true, &world(), flat, open);
        let ends: Vec<&Waypoint> = net
            .segments
            .iter()
            .filter_map(|s| s.points.last().filter(|p| p.stop.is_some()))
            .collect();
        assert_eq!(ends.len(), 4);
        assert_eq!(ends.iter().filter(|p| p.stop == Some(1)).count(), 2);
        let free = CarNetwork::build(&crossroads(true), 1.0, false, false, &world(), flat, open);
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
        let net = CarNetwork::build(&[primary], 1.0, false, false, &world(), flat, open);
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
        let net = CarNetwork::build(&[bend], 1.0, false, false, &world(), flat, open);
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
            open,
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
        let hill = |_: &ProcessedWay, x: i32, _: i32| 64 + ((x as f64 / 15.0).sin() * 6.0) as i32;
        let net = CarNetwork::build(&[straight], 1.0, false, false, &world(), hill, open);
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
        let net = CarNetwork::build(&roads, 1.0, false, false, &world(), flat, open);
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
            let net = CarNetwork::build(&[way], 1.0, false, false, &world(), flat, open);
            assert_eq!(net.car_count(), 0, "{extra:?}");
        }
    }

    #[test]
    fn the_storage_function_writes_every_lane_with_its_speed() {
        let net = CarNetwork::build(&crossroads(true), 1.0, false, true, &world(), flat, open);
        let text = net.storage_function();
        assert_eq!(text.lines().count(), 5 + 2 + 1 + 2);
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
            spots: BTreeMap::new(),
            size: 1.0,
            scale: 1.0,
        };
        let text = net.storage_function();
        assert_eq!(
            text.lines().count(),
            5 + 2 * n.div_ceil(LANES_PER_LINE) + 1 + 1
        );
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
        let net = CarNetwork::build(&crossroads(true), 1.0, false, true, &world(), flat, open);
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
        let net = CarNetwork::build(&crossroads(false), 1.0, false, false, &world(), flat, open);
        assert!(net.car_count() > 0);
        for car in &net.cars {
            assert_eq!(car.pos.1, 65.0);
            assert!((880..=1120).contains(&car.pace));
        }
        for (kind, _) in KINDS {
            let tails = parts(kind, "minecraft:red_concrete")
                .iter()
                .filter(|p| p.light == Light::Tail)
                .count();
            assert_eq!(tails, 1);
        }
    }

    #[test]
    fn respawned_bodies_match_the_placed_ones_in_both_block_formats() {
        let net = CarNetwork::build(&crossroads(false), 1.0, false, false, &world(), flat, open);
        let bodies = net.body_functions();
        assert_eq!(bodies.len(), KINDS.len() * 2);
        let sedan = |prefix: &str| {
            &bodies
                .iter()
                .find(|(p, _)| {
                    p == &format!("{prefix}data/arnis/function/cars/body/sedan.mcfunction")
                })
                .unwrap()
                .1
        };
        // Every part, and the box that can be clicked.
        assert_eq!(sedan("").lines().count(), parts(Kind::Sedan, "x").len() + 1);
        assert!(sedan("").starts_with("$summon interaction $(x) $(y) $(z) {Tags:[\"arnis.car.part\",\"arnis.k_sedan\"],width:2.40f,height:1.45f}"));
        assert!(sedan("").contains("block_state:{Name:\"$(colour)\"}"));
        assert!(sedan("ov121/").contains("block_state:\"$(colour)\""));
        assert!(sedan("").contains("\"arnis.car.tail\"],teleport_duration:2,view_range:3f"));
        assert!(net
            .storage_function()
            .contains("colours set value {sedan:[\"minecraft:white_concrete\""));
    }

    #[test]
    fn the_look_functions_scale_with_the_cars() {
        let net = CarNetwork::build(&crossroads(false), 2.0, false, false, &world(), flat, open);
        let looks = net.look_functions(false);
        assert_eq!(looks.len(), 19);
        let h0 = &looks
            .iter()
            .find(|(p, _)| p.ends_with("h0.mcfunction"))
            .unwrap()
            .1;
        assert!(h0.contains("^ ^ ^6.00"), "{h0}");
        // Bucket 0 points north, so its range wraps round the half turn.
        assert!(h0.contains("y_rotation=112.5..-112.5"), "{h0}");
    }

    #[test]
    fn a_new_car_is_of_every_kind_as_often_as_its_weight() {
        let net = CarNetwork::build(&crossroads(false), 1.0, false, false, &world(), flat, open);
        let (path, text) = net.kind_function();
        assert_eq!(path, "data/arnis/function/cars/respawn_kind.mcfunction");
        assert!(
            text.starts_with("execute store result score #r arnis.car run random value 0..99\n")
        );
        assert!(text.contains(
            "matches 0..51 run data modify storage arnis:cars spawn merge value {kind:\"sedan\",long:0b}"
        ));
        assert!(text.contains(
            "matches 97..99 run data modify storage arnis:cars spawn merge value {kind:\"bus\",long:1b}"
        ));
    }

    #[test]
    fn every_car_can_be_clicked_and_is_seen_as_far_as_it_is_sent() {
        let k = 2.0;
        let net = CarNetwork::build(&crossroads(false), k, false, false, &world(), flat, open);
        for (kind, _) in KINDS {
            let (width, height) = kind.hitbox();
            assert!(width >= 2.4 && height >= 1.4, "{kind:?}");
            let body = body(kind, "minecraft:white_concrete", net.size);
            assert!(body
                .iter()
                .all(|nbt| nbt["view_range"] == Value::Float(VIEW_RANGE)));
        }
        const { assert!(VIEW_RANGE * 64.0 >= 160.0) };
    }

    /// A crossroads whose every approach has its signal 8 blocks out, on two-way roads, with
    /// no signal in the middle and the direction of none tagged.
    fn approach_signals(tags: &[(&str, &str)]) -> Vec<ProcessedElement> {
        let mut signal = vec![("highway", "traffic_signals")];
        signal.extend_from_slice(tags);
        vec![
            road(
                1,
                vec![
                    node(1, -100, 0, &[]),
                    node(11, -8, 0, &signal),
                    node(9, 0, 0, &[]),
                    node(12, 8, 0, &signal),
                    node(2, 100, 0, &[]),
                ],
                &[],
            ),
            road(
                2,
                vec![
                    node(3, 0, -100, &[]),
                    node(13, 0, -8, &signal),
                    node(9, 0, 0, &[]),
                    node(14, 0, 8, &signal),
                    node(4, 0, 100, &[]),
                ],
                &[],
            ),
        ]
    }

    /// (lanes that hold a stop line, those that head into the junction among them).
    fn stops(net: &CarNetwork) -> (usize, usize) {
        let held: Vec<&Segment> = net
            .segments
            .iter()
            .filter(|s| s.points.iter().any(|p| p.stop.is_some()))
            .collect();
        (held.len(), held.iter().filter(|s| s.into_junction).count())
    }

    #[test]
    fn a_car_leaving_a_junction_ignores_the_signal_for_those_coming_the_other_way() {
        let net = CarNetwork::build(
            &approach_signals(&[]),
            1.0,
            false,
            true,
            &world(),
            flat,
            open,
        );
        assert_eq!(stops(&net), (4, 4));
    }

    #[test]
    fn a_signal_tagged_for_a_direction_is_taken_at_its_word() {
        let tagged = [("traffic_signals:direction", "both")];
        let net = CarNetwork::build(
            &approach_signals(&tagged),
            1.0,
            false,
            true,
            &world(),
            flat,
            open,
        );
        assert_eq!(stops(&net).0, 8);
    }

    #[test]
    fn a_signal_on_a_one_way_road_holds_its_only_traffic_however_close_to_the_junction() {
        let mut roads = approach_signals(&[]);
        if let ProcessedElement::Way(w) = &mut roads[0] {
            w.tags.insert("oneway".to_string(), "yes".to_string());
        }
        let net = CarNetwork::build(&roads, 1.0, false, true, &world(), flat, open);
        // Eastbound, the one past the junction still holds it; north-south as before.
        let east: Vec<&Segment> = net
            .segments
            .iter()
            .filter(|s| s.way == 1 && s.points.iter().any(|p| p.stop.is_some()))
            .collect();
        assert_eq!(east.len(), 2);
    }

    #[test]
    fn crosswalk_signals_round_a_junction_hold_only_those_coming_in() {
        // Mapped on each crosswalk rather than on the stop lines: leaving, cars go on.
        let net = CarNetwork::build(
            &approach_signals(&[("crossing", "traffic_signals")]),
            1.0,
            false,
            true,
            &world(),
            flat,
            open,
        );
        assert_eq!(stops(&net), (4, 4));
        // A crossing further down the road holds both ways, even with signals at the junction.
        let mut roads = approach_signals(&[]);
        if let ProcessedElement::Way(w) = &mut roads[0] {
            let mut far = node(15, 25, 0, &[("highway", "traffic_signals")]);
            far.tags
                .insert("crossing".to_string(), "traffic_signals".to_string());
            w.nodes.insert(4, far);
        }
        let net = CarNetwork::build(&roads, 1.0, false, true, &world(), flat, open);
        let at_the_crossing = net
            .segments
            .iter()
            .filter(|s| {
                s.points
                    .iter()
                    .any(|p| p.stop.is_some() && p.x > 18.0 && p.x < 32.0)
            })
            .count();
        assert_eq!(at_the_crossing, 2);
    }

    #[test]
    fn a_lone_signal_near_a_junction_is_a_crossing_that_holds_both_ways() {
        let signal = [("highway", "traffic_signals")];
        let roads = vec![
            road(
                1,
                vec![
                    node(1, -100, 0, &[]),
                    node(11, -8, 0, &signal),
                    node(9, 0, 0, &[]),
                    node(2, 100, 0, &[]),
                ],
                &[],
            ),
            road(2, vec![node(9, 0, 0, &[]), node(4, 0, 100, &[])], &[]),
        ];
        let net = CarNetwork::build(&roads, 1.0, false, true, &world(), flat, open);
        assert_eq!(stops(&net).0, 2);
    }

    #[test]
    fn a_signal_is_counted_for_the_nearer_of_two_junctions() {
        let signal = [("highway", "traffic_signals")];
        // A short block: 8 past one junction but only 6 before the next, whose own lane it holds.
        let roads = vec![
            road(
                1,
                vec![
                    node(1, -100, 0, &[]),
                    node(21, -8, 0, &signal),
                    node(9, 0, 0, &[]),
                    node(22, 8, 0, &signal),
                    node(10, 14, 0, &[]),
                    node(2, 100, 0, &[]),
                ],
                &[],
            ),
            road(2, vec![node(3, 0, -100, &[]), node(9, 0, 0, &[])], &[]),
            road(3, vec![node(10, 14, 0, &[]), node(5, 14, 100, &[])], &[]),
        ];
        let net = CarNetwork::build(&roads, 1.0, false, true, &world(), flat, open);
        let eastbound_held = net
            .segments
            .iter()
            .filter(|s| {
                s.points.iter().any(|p| p.stop.is_some())
                    && s.points[s.points.len() - 1].x > s.points[0].x
                    && s.points[0].x > -1.0
            })
            .count();
        assert_eq!(eastbound_held, 1);
    }

    #[test]
    fn every_way_is_driven_at_its_own_level_where_two_cross() {
        // A bridge at 80 over a road at 64, in the same columns.
        let roads = vec![
            road(1, vec![node(1, -100, 0, &[]), node(2, 100, 0, &[])], &[]),
            road(
                2,
                vec![node(3, 0, -100, &[]), node(4, 0, 100, &[])],
                &[("bridge", "yes")],
            ),
        ];
        let levels = |way: &ProcessedWay, _: i32, _: i32| if way.id == 2 { 80 } else { 64 };
        let net = CarNetwork::build(&roads, 1.0, false, false, &world(), levels, open);
        for seg in &net.segments {
            let want = if seg.way == 2 { 81.0 } else { 65.0 };
            assert!(seg.points.iter().all(|p| p.y == want), "way {}", seg.way);
        }
    }

    #[test]
    fn spots_lie_on_lanes_cars_can_start_on_in_the_cell_they_are_listed_by() {
        let net = CarNetwork::build(&awkward_roads(), 1.0, false, false, &world(), flat, open);
        assert!(!net.spots.is_empty());
        for (&(cx, cz), spots) in &net.spots {
            for &(lane, point) in spots {
                let seg = &net.segments[lane];
                assert!(seg.carries_cars());
                assert!(point + 1 < seg.points.len());
                let p = seg.points[point];
                assert_eq!((p.x / SPOT_CELL).floor() as i32, cx);
                assert_eq!((p.z / SPOT_CELL).floor() as i32, cz);
            }
        }
        let text = net.storage_function();
        assert!(text.contains("data modify storage arnis:cars grid set value {}"));
        let (&(cx, cz), spots) = net.spots.iter().next().unwrap();
        let (lane, point) = spots[0];
        assert!(text.contains(&format!("\"{cx}_{cz}\":[[I;{lane},{point}]")));
        let data = net.bedrock_data();
        assert_eq!(data["spots"].as_object().unwrap().len(), net.spots.len());
    }

    #[test]
    fn spots_follow_how_busy_a_road_is_not_how_thin_the_placed_cars_are() {
        let lane_spots = |highway: &str| {
            let way = road(
                5,
                vec![node(1, 0, 0, &[]), node(2, 1000, 0, &[])],
                &[("highway", highway)],
            );
            let net = CarNetwork::build(&[way], 1.0, false, false, &world(), flat, open);
            net.spots.values().map(Vec::len).sum::<usize>()
        };
        assert!(lane_spots("primary") > lane_spots("residential"));
        assert!(lane_spots("residential") > lane_spots("service"));
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
            let net = CarNetwork::build(&awkward_roads(), 1.0, left, false, &world(), flat, open);
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
        let net = CarNetwork::build(&stub_roads(), 1.0, false, false, &world(), flat, open);
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
            let net = CarNetwork::build(&roads, 1.0, false, false, &world(), flat, open);
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
            let net = CarNetwork::build(&roads, 1.0, left, false, &world(), flat, open);
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
        let net = CarNetwork::build(&roads, 1.0, false, false, &world(), flat, open);
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
        let net = CarNetwork::build(&roads, 1.0, false, true, &world(), flat, open);
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

    #[test]
    fn a_scrap_of_road_with_a_dead_end_at_each_end_sends_no_one_round_it() {
        let scrap = road(30, vec![node(1, 0, 0, &[]), node(2, 8, 0, &[])], &[]);
        let net = CarNetwork::build(&[scrap], 1.0, false, false, &world(), flat, open);
        assert!(net.segments.iter().all(|s| s.next.is_empty()));
        assert_eq!(net.car_count(), 0);
    }

    #[test]
    fn a_car_turning_round_never_spins_on_the_spot() {
        for left in [false, true] {
            for roads in [stub_roads(), awkward_roads()] {
                let net = CarNetwork::build(&roads, 1.0, left, false, &world(), flat, open);
                // Half a turn inside a car length and a bit is spinning on the spot.
                let turn = tightest_turn(&net, 5.0);
                assert!(turn <= 170.0, "{turn} degrees in 5 blocks, left {left}");
            }
        }
    }

    #[test]
    fn a_dead_end_is_turned_round_in_a_curve_onto_the_way_back() {
        let way = road(5, vec![node(1, 0, 0, &[]), node(2, 100, 0, &[])], &[]);
        let net = CarNetwork::build(&[way], 1.0, false, false, &world(), flat, open);
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
        let net = CarNetwork::build(&roads, 1.0, false, false, &world(), flat, open);
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
        let net = CarNetwork::build(&[way], 1.0, false, false, &world, flat, open);
        let (out, back): (Vec<&Segment>, Vec<&Segment>) = net
            .segments
            .iter()
            .partition(|s| s.points[1].x < s.points[0].x);
        assert!(out[0].next.is_empty());
        assert_eq!(back[0].next.len(), 1);
    }

    #[test]
    fn bedrock_data_holds_every_lane_point_and_names_a_body_for_every_car() {
        let net = CarNetwork::build(&awkward_roads(), 1.0, false, false, &world(), flat, open);
        let data = net.bedrock_data();
        let lanes = data["lanes"].as_array().unwrap();
        assert_eq!(lanes.len(), net.segments.len());
        let numbers = |v: &serde_json::Value| -> Vec<i64> {
            let text = v.as_str().unwrap_or("");
            if text.is_empty() {
                return Vec::new();
            }
            text.split(',').map(|n| n.parse().unwrap()).collect()
        };
        for (lane, seg) in lanes.iter().zip(&net.segments) {
            let p = numbers(&lane["p"]);
            assert_eq!(p.len(), seg.points.len() * 3);
            let mut at = [0i64; 3];
            for (k, q) in seg.points.iter().enumerate() {
                for a in 0..3 {
                    at[a] += p[k * 3 + a];
                }
                let back = (
                    at[0] as f64 / 10.0,
                    at[1] as f64 / 10.0,
                    at[2] as f64 / 10.0,
                );
                assert!((back.0 - q.x).abs() < 0.051 && (back.1 - q.y).abs() < 0.051);
                assert!((back.2 - q.z).abs() < 0.051);
            }
            assert_eq!(lane["v"], seg.speed);
            let turns = numbers(&lane["n"]);
            assert_eq!(turns.len(), seg.next.len() * 3);
            for (t, turn) in turns.chunks(3).zip(&seg.next) {
                assert_eq!(
                    t,
                    [turn.lane as i64, turn.entry as i64, turn.crossing as i64]
                );
            }
            let caps: Vec<i64> = seg
                .points
                .iter()
                .enumerate()
                .filter_map(|(k, q)| q.cap.map(|c| [k as i64, c as i64]))
                .flatten()
                .collect();
            assert_eq!(numbers(&lane["m"]), caps);
        }
        let bodies: Vec<&str> = data["bodies"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_str().unwrap())
            .collect();
        let start = data["start"].as_array().unwrap();
        assert_eq!(start.len(), net.car_count());
        for (car, entry) in net.cars.iter().zip(start) {
            let body = bodies[entry[5].as_u64().unwrap() as usize];
            assert_eq!(body, body_name(car.kind, car.colour));
            assert_eq!(entry[0], car.segment);
            assert_eq!(entry[1], car.target);
        }
        // A respawned car's name is put together from these, so each has to be a body.
        let rules = &data["rules"];
        let kinds = rules["kinds"].as_array().unwrap();
        assert_eq!(kinds.len(), KINDS.len());
        for kind in kinds {
            let kind = kind[0].as_str().unwrap();
            for colour in data["colours"][kind].as_array().unwrap() {
                let name = format!("car_{kind}_{}", colour.as_str().unwrap());
                assert!(bodies.contains(&name.as_str()), "{name}");
            }
        }
        assert_eq!(rules["look"].as_array().unwrap().len(), LOOK.len());
        assert_eq!(rules["pace"], json!([PACE.0, PACE.1]));
        assert_eq!(data["accel"], net.accel());
    }

    #[test]
    fn every_bedrock_car_body_has_one_brake_light_and_known_colours() {
        let net = CarNetwork::build(&crossroads(false), 2.0, false, false, &world(), flat, open);
        let bodies = net.bedrock_bodies();
        assert_eq!(bodies.len(), all_bodies().len());
        let names: FnvHashSet<&str> = bodies.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names.len(), bodies.len(), "names are unique");
        for body in &bodies {
            assert_eq!(
                body.shapes.iter().filter(|s| s.lamp == Lamp::Brake).count(),
                1,
                "{}",
                body.name
            );
            for s in &body.shapes {
                assert!(crate::bedrock_addon::palette_has(s.block), "{}", s.block);
                assert!(s.size.iter().all(|&v| v > 0.0));
            }
        }
        // Built at the size the world's cars are.
        let sedan = bodies.iter().find(|b| b.name == "car_sedan_white").unwrap();
        let long = sedan
            .shapes
            .iter()
            .map(|s| s.size[2])
            .fold(0.0f32, f32::max);
        assert!((long - 4.6 * 2.0).abs() < 1e-4, "{long}");
    }

    #[test]
    fn lanes_too_short_to_drive_are_crossed_as_part_of_the_junction() {
        let net = CarNetwork::build(&awkward_roads(), 1.0, false, false, &world(), flat, open);
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
