//! Level crossings: where a road crosses, at grade, a track that trains run on. Each gets
//! a warning mast and a barrier boom on every approach, models from `assets/vehicles`
//! (`models/crossings.py`), and the extras data pack closes it for its trains: the lamps
//! flash and a bell rings, the booms come down, and they rise again once the train is past.
//!
//! The trains run on timetables fixed in advance, all on one clock, so nothing looks for
//! a train in the game: when each crossing closes is worked out here from the routes, the
//! timetables and the length of the trains, and the pack is given the result as stretches
//! of each timetable. Cars wait at a crossing as at a red light, on stop points of their
//! own signal group, and cross it as they cross a junction, only with room to stand clear
//! past it (`cars.rs`). The tags, the scores and the state numbers are the contract with
//! `assets/minecraft/datapack_extras/data/arnis/function/xing`.
//!
//! A crossing no train closes stands open: one on a track without trains, and every one in
//! a world without the pack. There the masts and booms are built as the traffic lights are
//! without it, of scaled vanilla blocks on Java and of plain blocks elsewhere.

use crate::block_definitions::{
    BRICK_WALL, COBBLESTONE_WALL, DIORITE_WALL, IRON_BARS, LIGHT_GRAY_CONCRETE, WHITE_WALL_BANNER,
};
use crate::element_processing::cars::drivable;
use crate::element_processing::highways::highway_block_range;
use crate::element_processing::road_markings::oneway_sign;
use crate::element_processing::traffic_lights::{kerb_side, yaw_of};
use crate::element_processing::trains::{Mode, TrackAt, TrainNetwork};
use crate::floodfill_cache::RoadMaskBitmap;
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::vehicle_models::{self, Look};
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use fnv::{FnvHashMap, FnvHashSet};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

/// The signal group of a crossing's stop points, after the two of the traffic lights.
pub(crate) const STOP_GROUP: u8 = 3;
/// Crossing nodes this close along a road are one crossing, as the two tracks of a
/// double line are, in metres.
const CLUSTER_M: f64 = 16.0;
/// From the middle of a track to where a car or a mast is clear of its trains, in metres.
const TRACK_HALF_M: f64 = 2.0;
/// How far outside that the booms come down, and how far short of a boom the front of a
/// car stops, in metres.
const BOOM_M: f64 = 2.0;
const LINE_M: f64 = 1.5;
/// How far past the tracks a car is clear of them, in metres.
const EXIT_M: f64 = 0.5;
/// A track meeting a road at less than this runs along it, as a tram in a street: no
/// crossing to close.
const MIN_ANGLE_DEG: f64 = 25.0;
/// How far from a crossing node a train's route may run and still be its track, in blocks.
const NODE_REACH: f64 = 2.0;
/// How far past the road's edge a mast may stand, in blocks.
const KERB_REACH: i32 = 3;
/// The shortest and the longest boom, in blocks.
const BOOM_REACH: (f64, f64) = (1.5, 14.0);

/// A closure in steps of a tenth of a second, as the timetables count: the lamps flash
/// and the bell rings, the booms come down, they are down this long before the train's
/// nose and after its tail, and they take this long to rise.
const WARN: i64 = 30;
const LOWER: i64 = 40;
const HOLD: i64 = 50;
const CLEAR: i64 = 10;
const RAISE: i64 = 30;
/// Two trains closer together than this keep the booms down between them.
const MIN_OPEN: i64 = 80;

/// What a crossing shows, as the pack numbers it. The highest any of its tracks asks for
/// holds: cars wait from RISING up.
const RISING: u8 = 1;
const WARNING: u8 = 2;
const LOWERING: u8 = 3;
const DOWN: u8 = 4;

/// The models are drawn at 32 units to the metre, so at half a vehicle's size.
const MODEL_SIZE: f64 = 0.5;
/// The boom's hinge on the mast's model: this far up and in front of the mast, in metres.
const HINGE_UP_M: f64 = 1.0;
const HINGE_FRONT_M: f64 = 15.0 / 32.0;
/// The boom model's length from its hinge, in metres.
const BOOM_MODEL_M: f64 = 4.0;
/// A boom at rest stands this far up from level, in degrees.
const BOOM_UP_DEG: f64 = 80.0;
/// The most stripes a standing boom is built with.
const MAX_STRIPES: usize = 12;

/// How a plan's masts and booms are built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Draw {
    /// Models from the world's resource pack, which the data pack switches.
    Models,
    /// Scaled vanilla blocks as displays, standing open: Java without the pack.
    Displays,
    /// A post with the cross on a banner and the boom up beside it, of plain blocks.
    Blocks,
}

/// Where a road's lanes break for a crossing, measured from one of its nodes along the
/// road's way: how far short of the node the front of a car stops, and how far past it a
/// car is clear of the tracks, on the side below the node along the way and above it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Break {
    pub(crate) crossing: u32,
    below_stop: f64,
    below_exit: f64,
    above_stop: f64,
    above_exit: f64,
}

impl Break {
    /// How far short of the node the front of a car stops, driving `forward` along the way
    /// or against it.
    pub(crate) fn stop(&self, forward: bool) -> f64 {
        if forward {
            self.below_stop
        } else {
            self.above_stop
        }
    }

    /// How far past the node a car driving on from it is clear of the tracks.
    pub(crate) fn exit(&self, forward: bool) -> f64 {
        if forward {
            self.above_exit
        } else {
            self.below_exit
        }
    }
}

/// A mast beside one approach, and the boom it lowers.
#[derive(Clone, Copy, Debug)]
struct Gate {
    crossing: usize,
    post: (i32, i32),
    /// Towards the traffic it stops.
    facing: (f64, f64),
    /// Towards the road, the way the boom lies.
    reach: (f64, f64),
    /// From the hinge to the boom's tip, in blocks.
    boom: f64,
    /// The cell behind the mast, where its boom stands while up, is off the road, or past
    /// its kerb: in neither its lanes nor the parking rows beside them.
    boom_clear: bool,
}

/// A track of a crossing: the service that runs on it, where along its route the road
/// is, and how far either way along the track the road reaches.
#[derive(Clone, Copy, Debug)]
struct Track {
    service: usize,
    at: f64,
    half: f64,
}

struct Crossing {
    tracks: Vec<Track>,
    gates: Vec<usize>,
    /// Spots on the tracks within the road, where a car must not be left standing.
    spots: Vec<(f64, f64)>,
    booms: bool,
    lamps: bool,
}

/// The crossings of a road way with the tracks, before those that share a node are joined.
struct Cluster {
    nodes: Vec<u64>,
    gates: Vec<Gate>,
    spots: Vec<(f64, f64)>,
    half: i32,
    /// The sine of the angle the tracks cross at, at its smallest.
    sine: f64,
}

/// Where walkers wait for a crossing: a spot on its tracks, and the way they run there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WalkStop {
    pub(crate) crossing: u32,
    pub(crate) at: (f64, f64),
    pub(crate) track: (f64, f64),
}

/// The level crossings of an area.
pub struct CrossingPlan {
    crossings: Vec<Crossing>,
    gates: Vec<Gate>,
    /// The gates built with each crossing node.
    by_node: FnvHashMap<u64, Vec<usize>>,
    /// By road way and node.
    breaks: FnvHashMap<(u64, u64), Break>,
    /// Nodes whose crossing has a mast, which carries the cross a sign would.
    signed: FnvHashSet<u64>,
    /// Blocks per metre the masts and booms are built at.
    size: f64,
    /// How near a spot on the tracks no car may be left standing, in blocks: anything
    /// nearer than the line cars stop at, which a car in its place never is.
    keep_clear: f64,
    draw: Draw,
    /// Where walkers wait, at the crossings trains close.
    walk_stops: Vec<WalkStop>,
}

fn is_crossing(tags: &HashMap<String, String>) -> bool {
    tags.get("railway").map(String::as_str) == Some("level_crossing")
}

fn is_track(way: &ProcessedWay) -> bool {
    matches!(
        way.tags.get("railway").map(String::as_str),
        Some("rail" | "light_rail" | "tram" | "subway" | "narrow_gauge" | "preserved")
    )
}

/// A railway's track, rather than a tramway's or a metro's.
fn is_heavy(way: &ProcessedWay) -> bool {
    matches!(
        way.tags.get("railway").map(String::as_str),
        Some("rail" | "narrow_gauge" | "preserved")
    )
}

fn unit(dx: f64, dz: f64) -> Option<(f64, f64)> {
    let len = dx.hypot(dz);
    (len > 1e-9).then(|| (dx / len, dz / len))
}

/// Where the lines from `a` to `b` and from `c` to `d` cross, if they do.
fn meet(a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)) -> Option<(f64, f64)> {
    let (r, s) = ((b.0 - a.0, b.1 - a.1), (d.0 - c.0, d.1 - c.1));
    let den = r.0 * s.1 - r.1 * s.0;
    if den.abs() < 1e-9 {
        return None;
    }
    let (qx, qz) = (c.0 - a.0, c.1 - a.1);
    let t = (qx * s.1 - qz * s.0) / den;
    let u = (qx * r.1 - qz * r.0) / den;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some((a.0 + r.0 * t, a.1 + r.1 * t))
}

/// A way's line through the middles of its nodes' blocks.
struct Line {
    points: Vec<(f64, f64)>,
    arcs: Vec<f64>,
}

impl Line {
    fn new(nodes: &[ProcessedNode]) -> Self {
        let points: Vec<(f64, f64)> = nodes
            .iter()
            .map(|n| (n.x as f64 + 0.5, n.z as f64 + 0.5))
            .collect();
        let mut arcs = vec![0.0];
        for w in points.windows(2) {
            let last = *arcs.last().unwrap_or(&0.0);
            arcs.push(last + (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1));
        }
        Self { points, arcs }
    }

    fn length(&self) -> f64 {
        self.arcs.last().copied().unwrap_or(0.0)
    }

    /// The point `s` along the line, which runs straight on past either end.
    fn at(&self, s: f64) -> (f64, f64) {
        let n = self.points.len();
        let along = |a: (f64, f64), b: (f64, f64), far: f64| {
            let (dx, dz) = unit(b.0 - a.0, b.1 - a.1).unwrap_or((1.0, 0.0));
            (a.0 + dx * far, a.1 + dz * far)
        };
        if s <= 0.0 {
            return along(self.points[0], self.points[1.min(n - 1)], s);
        }
        if s >= self.length() {
            return along(
                self.points[n.saturating_sub(2)],
                self.points[n - 1],
                s - self.length() + self.arcs[n - 1] - self.arcs[n.saturating_sub(2)],
            );
        }
        let k = self.arcs.partition_point(|&a| a <= s).clamp(1, n - 1);
        along(self.points[k - 1], self.points[k], s - self.arcs[k - 1])
    }

    /// Which way the line runs at `s`.
    fn heading(&self, s: f64) -> (f64, f64) {
        let (a, b) = (self.at(s - 1.5), self.at(s + 1.5));
        unit(b.0 - a.0, b.1 - a.1).unwrap_or((1.0, 0.0))
    }
}

impl CrossingPlan {
    /// The crossings of the drivable roads with the tracks, for the pack to close for the
    /// trains `trains` runs; those on a track without trains stand open. `road_mask` says
    /// where the roads are, so the masts stand beside them.
    pub fn build(
        elements: &[ProcessedElement],
        trains: &TrainNetwork,
        scale: f64,
        drives_on_left: bool,
        road_mask: &RoadMaskBitmap,
    ) -> Self {
        Self::plan(
            elements,
            Some(trains),
            scale,
            drives_on_left,
            road_mask,
            Draw::Models,
        )
    }

    /// The crossings of the drivable roads with the tracks, all standing open, built as
    /// `draw` says.
    pub fn standing(
        elements: &[ProcessedElement],
        scale: f64,
        drives_on_left: bool,
        road_mask: &RoadMaskBitmap,
        draw: Draw,
    ) -> Self {
        Self::plan(elements, None, scale, drives_on_left, road_mask, draw)
    }

    fn plan(
        elements: &[ProcessedElement],
        trains: Option<&TrainNetwork>,
        scale: f64,
        drives_on_left: bool,
        road_mask: &RoadMaskBitmap,
        draw: Draw,
    ) -> Self {
        let size = scale.clamp(0.6, 2.0);
        let metres = scale.max(0.6);
        let ways = || {
            elements.iter().filter_map(|e| match e {
                ProcessedElement::Way(w) if w.nodes.len() >= 2 => Some(w),
                _ => None,
            })
        };
        // Which way each track runs over its crossing nodes, and those a railway runs over.
        let mut rails: FnvHashMap<u64, Vec<(f64, f64)>> = FnvHashMap::default();
        let mut on_railway: FnvHashSet<u64> = FnvHashSet::default();
        for way in ways().filter(|w| is_track(w)) {
            let line = Line::new(&way.nodes);
            for (k, node) in way.nodes.iter().enumerate() {
                if is_crossing(&node.tags) {
                    rails
                        .entry(node.id)
                        .or_default()
                        .push(line.heading(line.arcs[k]));
                    if is_heavy(way) {
                        on_railway.insert(node.id);
                    }
                }
            }
        }
        let roads: Vec<(&ProcessedWay, i32, i8)> = ways()
            .filter(|w| drivable(&w.tags))
            .map(|w| {
                let highway = &w.tags["highway"];
                (
                    w,
                    highway_block_range(highway, &w.tags, scale),
                    oneway_sign(highway, &w.tags),
                )
            })
            .collect();
        // The crossing nodes on the roads, and the services running over each.
        let mut spot_of: FnvHashMap<u64, usize> = FnvHashMap::default();
        let mut spots: Vec<(f64, f64)> = Vec::new();
        let mut tags_of: FnvHashMap<u64, &HashMap<String, String>> = FnvHashMap::default();
        for (way, _, _) in &roads {
            for node in way.nodes.iter().filter(|n| is_crossing(&n.tags)) {
                spot_of.entry(node.id).or_insert_with(|| {
                    spots.push((node.x as f64 + 0.5, node.z as f64 + 0.5));
                    tags_of.insert(node.id, &node.tags);
                    spots.len() - 1
                });
            }
        }
        let served = match trains {
            Some(trains) => trains.tracks_at(&spots, NODE_REACH),
            None => vec![Vec::new(); spots.len()],
        };
        let tracks_of = |id: u64| -> &[TrackAt] { &served[spot_of[&id]] };

        let min_sine = MIN_ANGLE_DEG.to_radians().sin();
        let mut clusters: Vec<Cluster> = Vec::new();
        let mut breaks: Vec<((u64, u64), usize, [f64; 4])> = Vec::new();
        for &(way, half, oneway) in &roads {
            let line = Line::new(&way.nodes);
            let last = way.nodes.len() - 1;
            let mut groups: Vec<Vec<usize>> = Vec::new();
            for k in (0..=last).filter(|&k| is_crossing(&way.nodes[k].tags)) {
                match groups.last_mut() {
                    Some(g) if line.arcs[k] - line.arcs[g[g.len() - 1]] <= CLUSTER_M * metres => {
                        g.push(k)
                    }
                    _ => groups.push(vec![k]),
                }
            }
            for group in groups {
                // Over a track, with trains on it or without.
                if group.iter().all(|&k| {
                    let id = way.nodes[k].id;
                    tracks_of(id).is_empty() && !rails.contains_key(&id)
                }) {
                    continue;
                }
                let (lo, hi) = (group[0], group[group.len() - 1]);
                // The node its lanes break at: where the way ends on the tracks, the lanes
                // of the way that goes on from there take the far side.
                let split = if hi == last { hi } else { lo };
                let (a, b) = (line.at(line.arcs[lo] - 4.0), line.at(line.arcs[hi] + 4.0));
                let Some(road) = unit(b.0 - a.0, b.1 - a.1) else {
                    continue;
                };
                // How far along the road the tracks reach, anywhere across it and its
                // kerbs: further the more askew they cross.
                let across = f64::from(half) + f64::from(KERB_REACH);
                let clear = TRACK_HALF_M * metres;
                let (mut from, mut to, mut sine) = (f64::MAX, f64::MIN, 1.0f64);
                for &k in &group {
                    let id = way.nodes[k].id;
                    let at = line.arcs[k] - line.arcs[split];
                    let mut ways_over: Vec<(f64, f64)> =
                        rails.get(&id).cloned().unwrap_or_default();
                    ways_over.extend(tracks_of(id).iter().map(|t| t.dir));
                    if ways_over.is_empty() {
                        ways_over.push((-road.1, road.0));
                    }
                    for (ux, uz) in ways_over {
                        let sin = (road.0 * uz - road.1 * ux).abs();
                        let cos = (road.0 * ux + road.1 * uz).abs();
                        if sin < min_sine {
                            continue;
                        }
                        let reach = across * cos / sin + clear / sin;
                        from = from.min(at - reach);
                        to = to.max(at + reach);
                        sine = sine.min(sin);
                    }
                }
                if from > to {
                    continue;
                }
                let c = clusters.len();
                let (boom, line_gap, exit) = (BOOM_M * metres, LINE_M * metres, EXIT_M * metres);
                breaks.push((
                    (way.id, way.nodes[split].id),
                    c,
                    [
                        (-from + boom + line_gap).max(0.0),
                        (-from + exit).max(0.0),
                        (to + boom + line_gap).max(0.0),
                        (to + exit).max(0.0),
                    ],
                ));
                let mut gates = Vec::new();
                // One mast for the traffic coming up the way, one for that coming down it,
                // each where the way has the road for it.
                for (along, sign, has_road) in [
                    (from - boom, 1.0, oneway >= 0 && split > 0),
                    (to + boom, -1.0, oneway <= 0 && split < last),
                ] {
                    if !has_road {
                        continue;
                    }
                    let s = line.arcs[split] + along;
                    let heading = line.heading(s);
                    let travel = (heading.0 * sign, heading.1 * sign);
                    let origin = line.at(s);
                    let side = kerb_side(travel, drives_on_left);
                    let cell = |off: i32| {
                        (
                            (origin.0 + side.0 * f64::from(off)).floor() as i32,
                            (origin.1 + side.1 * f64::from(off)).floor() as i32,
                        )
                    };
                    let post = (half + 1..=half + KERB_REACH)
                        .map(cell)
                        .find(|&(x, z)| !road_mask.contains(x, z))
                        .unwrap_or_else(|| cell(half + 1));
                    // From the mast in to the middle of a two-way road, or right across a
                    // one-way one.
                    let out = (f64::from(post.0) + 0.5 - origin.0) * side.0
                        + (f64::from(post.1) + 0.5 - origin.1) * side.1;
                    let boom = if oneway == 0 {
                        out - 0.25
                    } else {
                        out + f64::from(half) + 0.5
                    };
                    let facing = (-travel.0, -travel.1);
                    let (_, (fx, fz)) = cardinal(facing);
                    gates.push(Gate {
                        crossing: c,
                        post,
                        facing,
                        reach: (-side.0, -side.1),
                        boom: boom.clamp(BOOM_REACH.0, BOOM_REACH.1),
                        boom_clear: {
                            let (bx, bz) = (post.0 - fx, post.1 - fz);
                            let out = (f64::from(bx) + 0.5 - origin.0) * side.0
                                + (f64::from(bz) + 0.5 - origin.1) * side.1;
                            !road_mask.contains(bx, bz) || out > f64::from(half) + 0.5
                        },
                    });
                }
                // Spots across the road on each track, a couple of blocks apart.
                let mut on_tracks = Vec::new();
                for &k in &group {
                    let s = line.arcs[k];
                    let (p, h) = (line.at(s), line.heading(s));
                    let mut off = -f64::from(half);
                    while off <= f64::from(half) {
                        on_tracks.push((p.0 - h.1 * off, p.1 + h.0 * off));
                        off += 2.0;
                    }
                }
                clusters.push(Cluster {
                    nodes: group.iter().map(|&k| way.nodes[k].id).collect(),
                    gates,
                    spots: on_tracks,
                    half,
                    sine,
                });
            }
        }

        // Clusters that share a node are one crossing: a road mapped as two ways that
        // meet on the tracks.
        let mut root: Vec<usize> = (0..clusters.len()).collect();
        fn find(root: &mut [usize], mut k: usize) -> usize {
            while root[k] != k {
                root[k] = root[root[k]];
                k = root[k];
            }
            k
        }
        let mut first_at: FnvHashMap<u64, usize> = FnvHashMap::default();
        for (c, cluster) in clusters.iter().enumerate() {
            for &id in &cluster.nodes {
                match first_at.get(&id) {
                    Some(&other) => {
                        let (a, b) = (find(&mut root, c), find(&mut root, other));
                        root[a.max(b)] = a.min(b);
                    }
                    None => {
                        first_at.insert(id, c);
                    }
                }
            }
        }
        let mut number: BTreeMap<usize, usize> = BTreeMap::new();
        for c in 0..clusters.len() {
            let r = find(&mut root, c);
            let next = number.len();
            number.entry(r).or_insert(next);
        }
        let crossing_of = |root: &mut [usize], c: usize| number[&find(root, c)];

        let mut plan = Self {
            crossings: (0..number.len())
                .map(|_| Crossing {
                    tracks: Vec::new(),
                    gates: Vec::new(),
                    spots: Vec::new(),
                    booms: false,
                    lamps: true,
                })
                .collect(),
            gates: Vec::new(),
            by_node: FnvHashMap::default(),
            breaks: FnvHashMap::default(),
            signed: FnvHashSet::default(),
            size,
            keep_clear: ((TRACK_HALF_M + BOOM_M + LINE_M) * metres - 0.5).max(2.0),
            draw,
            walk_stops: Vec::new(),
        };
        // Whether a railway's trains run over it, and whether a railway's track does.
        let mut heavy = vec![(false, false); number.len()];
        let mut tagged: Vec<(Option<bool>, Option<bool>)> = vec![(None, None); number.len()];
        for (c, cluster) in clusters.iter().enumerate() {
            let x = crossing_of(&mut root, c);
            let crossing = &mut plan.crossings[x];
            for &id in &cluster.nodes {
                heavy[x].1 |= on_railway.contains(&id);
                for t in tracks_of(id) {
                    heavy[x].0 |= matches!(t.mode, Mode::Rail | Mode::SBahn);
                    let known = crossing
                        .tracks
                        .iter()
                        .any(|k| k.service == t.service && (k.at - t.at).abs() < 1.0);
                    if !known {
                        crossing.tracks.push(Track {
                            service: t.service,
                            at: t.at,
                            // The train is on the crossing from the road's near edge to
                            // its far one, measured along the track.
                            half: (f64::from(cluster.half) + 2.0) / cluster.sine,
                        });
                    }
                }
                let tags = tags_of[&id];
                let said = |key: &str| tags.get(key).map(|v| v != "no");
                tagged[x].0 = tagged[x].0.or(said("crossing:barrier"));
                tagged[x].1 = tagged[x].1.or(said("crossing:light"));
            }
            crossing.spots.extend(&cluster.spots);
        }
        for (x, crossing) in plan.crossings.iter_mut().enumerate() {
            // A railway's crossing has booms and a tramway's only lamps, unless mapped
            // otherwise; one mapped with neither is left as it was, though cars still give
            // its trains the way. Without trains the track says which it is.
            let railway = if crossing.tracks.is_empty() {
                heavy[x].1
            } else {
                heavy[x].0
            };
            crossing.booms = tagged[x].0.unwrap_or(railway);
            crossing.lamps = crossing.booms || tagged[x].1.unwrap_or(true);
        }
        for (c, cluster) in clusters.iter().enumerate() {
            let x = crossing_of(&mut root, c);
            if !plan.crossings[x].lamps {
                continue;
            }
            plan.signed.extend(&cluster.nodes);
            for gate in &cluster.gates {
                let g = plan.gates.len();
                plan.gates.push(Gate {
                    crossing: x,
                    ..*gate
                });
                plan.crossings[x].gates.push(g);
                // Built with the cluster's first node, which is never far.
                plan.by_node.entry(cluster.nodes[0]).or_default().push(g);
            }
        }
        // Lanes break only where trains close the crossing.
        for (key, c, [below_stop, below_exit, above_stop, above_exit]) in breaks {
            let x = crossing_of(&mut root, c);
            if plan.crossings[x].tracks.is_empty() {
                continue;
            }
            plan.breaks.insert(
                key,
                Break {
                    crossing: x as u32,
                    below_stop,
                    below_exit,
                    above_stop,
                    above_exit,
                },
            );
        }
        // Walkers on a road wait where its cars do, on every track it crosses.
        for (c, cluster) in clusters.iter().enumerate() {
            let x = crossing_of(&mut root, c);
            if plan.crossings[x].tracks.is_empty() {
                continue;
            }
            for &id in &cluster.nodes {
                let track = tracks_of(id)
                    .first()
                    .map(|t| t.dir)
                    .or_else(|| rails.get(&id).and_then(|r| r.first().copied()));
                if let Some(track) = track {
                    plan.walk_stops.push(WalkStop {
                        crossing: x as u32,
                        at: spots[spot_of[&id]],
                        track,
                    });
                }
            }
        }
        if let Some(trains) = trains {
            plan.add_footpaths(elements, trains, metres);
        }
        plan
    }

    /// Where a path crosses a track trains run on: a crossing of its own, with no masts,
    /// closed for its trains as a road's is so that walkers wait at it.
    fn add_footpaths(&mut self, elements: &[ProcessedElement], trains: &TrainNetwork, metres: f64) {
        let ways = || {
            elements.iter().filter_map(|e| match e {
                ProcessedElement::Way(w) if w.nodes.len() >= 2 => Some(w),
                _ => None,
            })
        };
        let level = |w: &ProcessedWay| {
            w.tags.get("bridge").is_none_or(|v| v == "no")
                && w.tags.get("tunnel").is_none_or(|v| v == "no")
        };
        // The tracks on the ground, piece by piece, by the cells of a coarse grid.
        const CELL: f64 = 64.0;
        let cell = |v: f64| (v / CELL).floor() as i32;
        let mut pieces: Vec<((f64, f64), (f64, f64))> = Vec::new();
        let mut grid: FnvHashMap<(i32, i32), Vec<usize>> = FnvHashMap::default();
        let mut on_track: FnvHashMap<u64, (f64, f64)> = FnvHashMap::default();
        for way in ways()
            .filter(|w| is_track(w) && crate::element_processing::railways::is_at_grade_track(w))
        {
            let line = Line::new(&way.nodes);
            for (k, node) in way.nodes.iter().enumerate() {
                on_track.insert(node.id, line.heading(line.arcs[k]));
            }
            for w in line.points.windows(2) {
                let (a, b) = (w[0], w[1]);
                for gx in cell(a.0.min(b.0))..=cell(a.0.max(b.0)) {
                    for gz in cell(a.1.min(b.1))..=cell(a.1.max(b.1)) {
                        grid.entry((gx, gz)).or_default().push(pieces.len());
                    }
                }
                pieces.push((a, b));
            }
        }
        if pieces.is_empty() {
            return;
        }
        // Each spot a path crosses a track at: where it is, the way the track runs and the
        // sine of the angle they cross at.
        type Spot = ((f64, f64), (f64, f64), f64);
        let min_sine = MIN_ANGLE_DEG.to_radians().sin();
        let mut found: Vec<Spot> = Vec::new();
        for way in ways().filter(|w| {
            crate::element_processing::pedestrians::walk_density(&w.tags).is_some()
                && !drivable(&w.tags)
                && level(w)
        }) {
            let line = Line::new(&way.nodes);
            let mut add = |at: (f64, f64), path: (f64, f64), track: (f64, f64)| {
                let sine = (path.0 * track.1 - path.1 * track.0).abs();
                if sine >= min_sine
                    && !found
                        .iter()
                        .any(|f| (f.0 .0 - at.0).hypot(f.0 .1 - at.1) < 1.0)
                {
                    found.push((at, track, sine));
                }
            };
            for (k, node) in way.nodes.iter().enumerate() {
                if let Some(&track) = on_track.get(&node.id) {
                    add(line.points[k], line.heading(line.arcs[k]), track);
                }
            }
            for w in line.points.windows(2) {
                let (a, b) = (w[0], w[1]);
                let Some(path) = unit(b.0 - a.0, b.1 - a.1) else {
                    continue;
                };
                let mut seen: FnvHashSet<usize> = FnvHashSet::default();
                for gx in cell(a.0.min(b.0))..=cell(a.0.max(b.0)) {
                    for gz in cell(a.1.min(b.1))..=cell(a.1.max(b.1)) {
                        for &t in grid.get(&(gx, gz)).into_iter().flatten() {
                            if !seen.insert(t) {
                                continue;
                            }
                            let (c, d) = pieces[t];
                            if let Some(at) = meet(a, b, c, d) {
                                let track = unit(d.0 - c.0, d.1 - c.1).unwrap_or((1.0, 0.0));
                                add(at, path, track);
                            }
                        }
                    }
                }
            }
        }
        if found.is_empty() {
            return;
        }
        let spots: Vec<(f64, f64)> = found.iter().map(|f| f.0).collect();
        let served = trains.tracks_at(&spots, NODE_REACH);
        // Spots close together are one crossing, as a path over a double line is.
        let reach = CLUSTER_M * metres / 2.0;
        let mut crossing_at: Vec<Option<usize>> = vec![None; found.len()];
        for i in 0..found.len() {
            if served[i].is_empty() {
                continue;
            }
            let near = (0..i).find(|&j| {
                crossing_at[j].is_some()
                    && (spots[i].0 - spots[j].0).hypot(spots[i].1 - spots[j].1) <= reach
            });
            let x = match near.and_then(|j| crossing_at[j]) {
                Some(x) => x,
                None => {
                    self.crossings.push(Crossing {
                        tracks: Vec::new(),
                        gates: Vec::new(),
                        spots: Vec::new(),
                        booms: false,
                        lamps: false,
                    });
                    self.crossings.len() - 1
                }
            };
            crossing_at[i] = Some(x);
            let (at, track, sine) = found[i];
            let crossing = &mut self.crossings[x];
            for t in &served[i] {
                if !crossing
                    .tracks
                    .iter()
                    .any(|k| k.service == t.service && (k.at - t.at).abs() < 1.0)
                {
                    crossing.tracks.push(Track {
                        service: t.service,
                        at: t.at,
                        // A path is a couple of blocks across.
                        half: 3.0 / sine,
                    });
                }
            }
            self.walk_stops.push(WalkStop {
                crossing: x as u32,
                at,
                track,
            });
        }
    }

    /// Where walkers wait, at the crossings trains close.
    pub(crate) fn walk_stops(&self) -> &[WalkStop] {
        &self.walk_stops
    }

    pub fn is_empty(&self) -> bool {
        self.crossings.is_empty()
    }

    #[cfg(test)]
    pub fn crossing_count(&self) -> usize {
        self.crossings.len()
    }

    /// The crossings with trains to close for.
    pub fn closing_count(&self) -> usize {
        self.crossings
            .iter()
            .filter(|c| !c.tracks.is_empty())
            .count()
    }

    /// Where the lanes of `way` break for a crossing at `node`.
    pub(crate) fn break_at(&self, way: u64, node: u64) -> Option<Break> {
        self.breaks.get(&(way, node)).copied()
    }

    /// Whether a crossing's masts stand for `node`, so it needs no sign of its own.
    pub fn signed(&self, node: u64) -> bool {
        self.signed.contains(&node)
    }

    /// Every look the masts and booms show: the data pack names the same ids. None when
    /// they are not models.
    pub fn looks(&self) -> Vec<Look> {
        if self.gates.is_empty() || self.draw != Draw::Models {
            return Vec::new();
        }
        [
            "xing_light_off",
            "xing_light_a",
            "xing_light_b",
            "xing_boom",
        ]
        .into_iter()
        .map(look)
        .collect()
    }

    /// Builds the masts and booms that stand for `node`, the masts dark and the booms up:
    /// as models, each a display the pack finds by its crossing's tag, or standing open.
    pub fn place(&self, editor: &mut WorldEditor, node: &ProcessedNode) {
        if !editor.owns(node.x, node.z) {
            return;
        }
        for &g in self.by_node.get(&node.id).into_iter().flatten() {
            let gate = &self.gates[g];
            let crossing = &self.crossings[gate.crossing];
            match self.draw {
                Draw::Models => {}
                Draw::Displays => {
                    self.place_displays(editor, gate, crossing.booms, 0x5A_0000 + g as i64 * 64);
                    continue;
                }
                Draw::Blocks => {
                    place_blocks(editor, gate, crossing.booms, self.size);
                    continue;
                }
            }
            let (x, z) = gate.post;
            let y = f64::from(editor.get_absolute_y(x, 0, z)) + 1.0;
            let (cx, cz) = (f64::from(x) + 0.5, f64::from(z) + 0.5);
            let seed = 0x58_0000 + g as i64 * 4;
            let part = |kind: &str, bell: bool| {
                let mut tags = vec![
                    Value::String("arnis.xp".to_string()),
                    Value::String(format!("arnis.x{}", gate.crossing)),
                    Value::String(kind.to_string()),
                ];
                if bell {
                    tags.push(Value::String("arnis.xbell".to_string()));
                }
                Value::List(tags)
            };
            let k = self.size;

            let mut mast = display(
                "xing_light_off",
                MODEL_SIZE * k,
                gate.crossing,
                yaw_of(gate.facing),
            );
            // One mast of each crossing rings its bell.
            mast.insert(
                "Tags".to_string(),
                part("arnis.xlamp", crossing.gates.first() == Some(&g)),
            );
            editor.add_entity_exact("minecraft:item_display", cx, y, cz, seed, mast);

            if !crossing.booms {
                continue;
            }
            // The model's +x, the way the boom lies, is to the right of its front.
            let front = (gate.reach.1, -gate.reach.0);
            let mut boom = display("xing_boom", MODEL_SIZE * k, gate.crossing, yaw_of(front));
            boom.insert("Tags".to_string(), part("arnis.xboom", false));
            if let Some(Value::Compound(t)) = boom.get_mut("transformation") {
                let floats =
                    |v: &[f64]| Value::List(v.iter().map(|f| Value::Float(*f as f32)).collect());
                // Stretched along its length to the lanes it closes, and standing up.
                if let Some(Value::List(scale)) = t.get_mut("scale") {
                    if let Some(Value::Float(sx)) = scale.first_mut() {
                        *sx *= (gate.boom / (BOOM_MODEL_M * k)) as f32;
                    }
                }
                t.insert("left_rotation".to_string(), floats(&boom_turn(true)));
            }
            // Its box is drawn in wherever the boom swings.
            let room = gate.boom + BOOM_MODEL_M * k * 0.25;
            boom.insert("width".to_string(), Value::Float((2.0 * room + 1.0) as f32));
            boom.insert("height".to_string(), Value::Float((room + 1.0) as f32));
            editor.add_entity_exact(
                "minecraft:item_display",
                cx + gate.facing.0 * HINGE_FRONT_M * k,
                y + HINGE_UP_M * k,
                cz + gate.facing.1 * HINGE_FRONT_M * k,
                seed + 1,
                boom,
            );
        }
    }

    /// One gate standing open, of scaled vanilla blocks: each piece a display at the foot
    /// of the mast, turned to face its traffic.
    fn place_displays(&self, editor: &mut WorldEditor, gate: &Gate, booms: bool, seed: i64) {
        let (x, z) = gate.post;
        let y = f64::from(editor.get_absolute_y(x, 0, z)) + 1.0;
        let (cx, cz) = (f64::from(x) + 0.5, f64::from(z) + 0.5);
        let k = self.size;
        // The display's +x, to the right of its front, and whether the road is that way.
        let right = (gate.facing.1, -gate.facing.0);
        let road_right = gate.reach.0 * right.0 + gate.reach.1 * right.1 > 0.0;
        let floats = |v: &[f64]| Value::List(v.iter().map(|f| Value::Float(*f as f32)).collect());
        for (i, piece) in standing_pieces(booms, gate.boom / k, road_right)
            .iter()
            .enumerate()
        {
            let size = piece.size.map(|s| s * k);
            let (sin, cos) = (piece.roll.to_radians() / 2.0).sin_cos();
            // The block spans 0..1 before it is turned, so its middle is turned with it.
            let (a, b) = piece.roll.to_radians().sin_cos();
            let half = [size[0] / 2.0, size[1] / 2.0];
            let middle = [half[0] * b - half[1] * a, half[0] * a + half[1] * b];
            let translation = [
                piece.at[0] * k - middle[0],
                piece.at[1] * k - middle[1],
                piece.at[2] * k - size[2] / 2.0,
            ];
            let mut nbt = HashMap::from([
                (
                    "block_state".to_string(),
                    Value::Compound(HashMap::from([(
                        "Name".to_string(),
                        Value::String(piece.block.to_string()),
                    )])),
                ),
                (
                    "transformation".to_string(),
                    Value::Compound(HashMap::from([
                        ("left_rotation".to_string(), floats(&[0.0, 0.0, sin, cos])),
                        ("right_rotation".to_string(), floats(&[0.0, 0.0, 0.0, 1.0])),
                        ("translation".to_string(), floats(&translation)),
                        ("scale".to_string(), floats(&size)),
                    ])),
                ),
                (
                    "Rotation".to_string(),
                    Value::List(vec![Value::Float(yaw_of(gate.facing)), Value::Float(0.0)]),
                ),
            ]);
            if piece.dim {
                // A dark lamp: its red shows by day and fades by night.
                nbt.insert(
                    "brightness".to_string(),
                    Value::Compound(HashMap::from([
                        ("sky".to_string(), Value::Int(11)),
                        ("block".to_string(), Value::Int(4)),
                    ])),
                );
            }
            editor.add_block_display(cx, y, cz, seed + i as i64, nbt);
        }
    }

    /// The pack functions: `check`, which works out twice a second what each crossing
    /// shows from where the timetables are, and per crossing the function that switches
    /// it. `runs` says whether the pack runs a service at all. Empty when no crossing has
    /// a train to close for.
    pub fn pack_functions(
        &self,
        trains: &TrainNetwork,
        runs: impl Fn(usize) -> bool,
    ) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut body = String::new();
        let mut clocks: BTreeMap<usize, usize> = BTreeMap::new();
        for (x, crossing) in self.crossings.iter().enumerate() {
            // (state, service, from, to), the lowest state first so the highest holds.
            let mut lines: Vec<(u8, usize, usize, usize)> = Vec::new();
            for track in crossing.tracks.iter().filter(|t| runs(t.service)) {
                let busy = trains.busy(track.service, track.at, track.half);
                if busy.is_empty() {
                    continue;
                }
                clocks.insert(track.service, busy.len());
                lines.extend(
                    windows(&busy)
                        .into_iter()
                        .map(|(state, from, to)| (state, track.service, from, to)),
                );
            }
            if lines.is_empty() {
                continue;
            }
            lines.sort_unstable();
            lines.dedup();
            let _ = writeln!(
                body,
                "# Crossing {x}\nscoreboard players set #n arnis.xing 0"
            );
            for (state, service, from, to) in lines {
                let _ = writeln!(
                    body,
                    "execute if score #i{service} arnis.xing matches {from}..{to} run scoreboard players set #n arnis.xing {state}"
                );
            }
            let _ = writeln!(
                body,
                "execute unless score #n arnis.xing = #x{x} arnis.xing run function arnis:xing/c/{x}"
            );
            out.push((
                format!("data/arnis/function/xing/c/{x}.mcfunction"),
                self.switch_function(x),
            ));
        }
        if out.is_empty() {
            return out;
        }
        let mut check = String::from(
            "# Where each timetable is now, then what each crossing on it shows: 0 open, 1 its\n\
             # booms rising, 2 warning, 3 its booms coming down, 4 down.\n",
        );
        for (service, steps) in clocks {
            let _ = writeln!(
                check,
                "scoreboard players operation #i{service} arnis.xing = #now arnis.train\n\
                 scoreboard players set #k arnis.xing {steps}\n\
                 scoreboard players operation #i{service} arnis.xing %= #k arnis.xing"
            );
        }
        check.push_str(&body);
        out.push((
            "data/arnis/function/xing/check.mcfunction".to_string(),
            check,
        ));
        out.push((
            "data/arnis/function/xing/boom.mcfunction".to_string(),
            boom_function(),
        ));
        out
    }

    /// The function that switches crossing `x` to the state in `#n`: its masts and booms
    /// in view take it, and as the booms start down a car left standing on the tracks is
    /// taken off them.
    fn switch_function(&self, x: usize) -> String {
        let crossing = &self.crossings[x];
        let mut out = format!("scoreboard players operation #x{x} arnis.xing = #n arnis.xing\n");
        let posts: Vec<(i32, i32)> = crossing.gates.iter().map(|&g| self.gates[g].post).collect();
        if let (Some(x0), Some(x1), Some(z0), Some(z1)) = (
            posts.iter().map(|p| p.0).min(),
            posts.iter().map(|p| p.0).max(),
            posts.iter().map(|p| p.1).min(),
            posts.iter().map(|p| p.1).max(),
        ) {
            let _ = writeln!(
                out,
                "execute as @e[type=item_display,tag=arnis.x{x},x={},y=-512,z={},dx={},dy=2048,dz={}] run function arnis:xing/apply",
                x0 - 2,
                z0 - 2,
                x1 - x0 + 4,
                z1 - z0 + 4,
            );
        }
        // A car that had to stop past the line, as where a junction lies right before the
        // tracks, stands with its front on them: everything short of the line goes.
        let (near, side) = (self.keep_clear, (2.0 * self.keep_clear - 1.0).round());
        for &(sx, sz) in &crossing.spots {
            let _ = writeln!(
                out,
                "execute if score #n arnis.xing matches {LOWERING} as @e[type=marker,tag=arnis.car,tag=arnis.car.still,x={:.1},y=-512,z={:.1},dx={side},dy=2048,dz={side}] at @s run function arnis:cars/despawn",
                sx - near,
                sz - near,
            );
        }
        out.push_str("function arnis:xing/changed\n");
        out
    }
}

/// The look a mast or a boom is drawn with.
fn look(model: &'static str) -> Look {
    Look::plain(model, "std", "minecraft:black_concrete")
}

/// The display of one part: its model, turned to `yaw`, with its crossing's number where
/// the pack reads it.
fn display(model: &'static str, size: f64, crossing: usize, yaw: f32) -> HashMap<String, Value> {
    let mut nbt = vehicle_models::display_nbt(
        model,
        &vehicle_models::item_model(&look(model)),
        size,
        false,
    );
    if let Some(Value::Compound(item)) = nbt.get_mut("item") {
        if let Some(Value::Compound(components)) = item.get_mut("components") {
            components.insert(
                "minecraft:custom_data".to_string(),
                Value::Compound(HashMap::from([(
                    "x".to_string(),
                    Value::Int(crossing as i32),
                )])),
            );
        }
    }
    nbt.insert(
        "Rotation".to_string(),
        Value::List(vec![Value::Float(yaw), Value::Float(0.0)]),
    );
    nbt
}

/// The turn that lifts a boom (`up`) or lays it level, as a display's left rotation. The
/// display draws the model's +x at its own -x, so the lift is a turn back about z.
fn boom_turn(up: bool) -> [f64; 4] {
    if up {
        let half = BOOM_UP_DEG.to_radians() / 2.0;
        [0.0, 0.0, -half.sin(), half.cos()]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// A box of a gate standing open: its middle across, up and towards the traffic from the
/// foot of the mast, and its size, in metres, turned `roll` degrees about the way the
/// gate faces.
#[derive(Clone, Copy, Debug)]
struct Piece {
    at: [f64; 3],
    size: [f64; 3],
    roll: f64,
    block: &'static str,
    /// A lamp, dark.
    dim: bool,
}

/// The mast of `models/crossings.py` in vanilla blocks, its lamps dark, and its boom of
/// `boom_m` metres standing up, leaning to the road on the right or the left. Only the
/// side facing the traffic has lamps and the cross.
fn standing_pieces(booms: bool, boom_m: f64, road_right: bool) -> Vec<Piece> {
    const U: f64 = 1.0 / 32.0;
    let piece = |at: [f64; 3], size: [f64; 3], block: &'static str| Piece {
        at: at.map(|v| v * U),
        size: size.map(|v| v * U),
        roll: 0.0,
        block,
        dim: false,
    };
    let mut out = vec![
        piece([0.0, 2.5, 0.0], [12.0, 5.0, 12.0], "minecraft:smooth_stone"),
        piece(
            [0.0, 55.5, 0.0],
            [4.0, 101.0, 4.0],
            "minecraft:light_gray_concrete",
        ),
        piece(
            [0.0, 74.0, 0.0],
            [24.0, 3.0, 3.0],
            "minecraft:light_gray_concrete",
        ),
    ];
    for x in [-12.0, 12.0] {
        out.push(piece(
            [x, 74.0, 2.25],
            [16.0, 16.0, 1.5],
            "minecraft:black_concrete",
        ));
        out.push(Piece {
            dim: true,
            ..piece([x, 74.0, 3.125], [9.0, 9.0, 0.25], "minecraft:red_concrete")
        });
    }
    // The cross: two boards, white with red ends, a hair apart.
    for (roll, z) in [(45.0f64, 3.0), (-45.0, 3.25)] {
        let (sin, cos) = roll.to_radians().sin_cos();
        for (along, len, block) in [
            (-16.0, 6.0, "minecraft:red_concrete"),
            (0.0, 26.0, "minecraft:white_concrete"),
            (16.0, 6.0, "minecraft:red_concrete"),
        ] {
            out.push(Piece {
                roll,
                ..piece([along * cos, 98.0 + along * sin, z], [len, 6.0, 1.0], block)
            });
        }
    }
    if !booms {
        return out;
    }
    // The drive, and the boom up from its hinge, red and white with a red tip, its
    // counterweight behind.
    out.push(piece(
        [0.0, 30.0, 7.75],
        [14.0, 32.0, 11.5],
        "minecraft:smooth_stone",
    ));
    out.push(piece(
        [0.0, 9.5, 7.5],
        [10.0, 9.0, 7.0],
        "minecraft:light_gray_concrete",
    ));
    let roll = if road_right {
        BOOM_UP_DEG
    } else {
        180.0 - BOOM_UP_DEG
    };
    let (sin, cos) = roll.to_radians().sin_cos();
    let hinge = [0.0, HINGE_UP_M, HINGE_FRONT_M];
    let along = |d: f64| [hinge[0] + d * cos, hinge[1] + d * sin, hinge[2]];
    let stripes = ((boom_m / 0.5).round() as usize).clamp(2, MAX_STRIPES);
    let stripe = boom_m / stripes as f64;
    for i in 0..stripes {
        let red = i % 2 == 1 || i == stripes - 1;
        out.push(Piece {
            at: along(stripe * (i as f64 + 0.5)),
            size: [stripe, 2.5 * U, 1.5 * U],
            roll,
            block: if red {
                "minecraft:red_concrete"
            } else {
                "minecraft:white_concrete"
            },
            dim: false,
        });
    }
    out.push(Piece {
        at: along(-12.0 * U),
        size: [24.0 * U, 5.0 * U, 4.0 * U],
        roll,
        block: "minecraft:gray_concrete",
        dim: false,
    });
    out
}

/// The side of a block that faces most nearly along `dir`, and the step to the next one
/// that way.
fn cardinal((dx, dz): (f64, f64)) -> (&'static str, (i32, i32)) {
    if dx.abs() >= dz.abs() {
        if dx >= 0.0 {
            ("east", (1, 0))
        } else {
            ("west", (-1, 0))
        }
    } else if dz >= 0.0 {
        ("south", (0, 1))
    } else {
        ("north", (0, -1))
    }
}

/// One gate standing open, of plain blocks: a post as the traffic lights have, the cross on
/// a banner facing the traffic, and the boom standing up behind it, white and red walls.
fn place_blocks(editor: &mut WorldEditor, gate: &Gate, booms: bool, size: f64) {
    let (x, z) = gate.post;
    let ground = editor.get_absolute_y(x, 0, z);
    let head = ground + ((3.3 * size).round() as i32).max(3);
    editor.set_block_absolute(COBBLESTONE_WALL, x, ground + 1, z, None, None);
    for y in ground + 2..head {
        editor.set_block_absolute(IRON_BARS, x, y, z, None, None);
    }
    editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, head, z, None, None);
    let (face, (fx, fz)) = cardinal(gate.facing);
    editor.place_wall_banner(
        WHITE_WALL_BANNER,
        x + fx,
        head,
        z + fz,
        face,
        "white",
        &[("red", "minecraft:cross")],
    );
    if !booms {
        return;
    }
    let (bx, bz) = (x - fx, z - fz);
    if !gate.boom_clear || editor.on_carriageway(bx, bz) {
        return;
    }
    let base = editor.get_absolute_y(bx, 0, bz);
    let rows = (gate.boom.round() as i32).clamp(2, 8);
    for row in 1..=rows {
        let red = row % 2 == 0 || row == rows;
        let block = if red { BRICK_WALL } else { DIORITE_WALL };
        editor.set_block_absolute(block, bx, base + row, bz, None, None);
    }
}

/// The pack function that swings a boom to where the state in `#n` wants it: down while
/// the booms come down and stay down, up otherwise, over the time the timetable allows.
pub fn boom_function() -> String {
    let turn = |up: bool| {
        let q = boom_turn(up);
        format!("[{:.4}f,{:.4}f,{:.4}f,{:.4}f]", q[0], q[1], q[2], q[3])
    };
    format!(
        "# A boom is down from when the booms start down until the train has gone.\n\
         scoreboard players set #want arnis.xing 0\n\
         execute if score #n arnis.xing matches {LOWERING}..{DOWN} run scoreboard players set #want arnis.xing 1\n\
         execute if score @s arnis.xb = #want arnis.xing run return 0\n\
         scoreboard players operation @s arnis.xb = #want arnis.xing\n\
         execute if score #want arnis.xing matches 1 run data merge entity @s {{start_interpolation:0,interpolation_duration:{},transformation:{{left_rotation:{}}}}}\n\
         execute if score #want arnis.xing matches 0 run data merge entity @s {{start_interpolation:0,interpolation_duration:{},transformation:{{left_rotation:{}}}}}\n",
        LOWER * 2,
        turn(false),
        RAISE * 2,
        turn(true),
    )
}

/// What a crossing shows through one service's timetable, from when a train is over it
/// (`busy`, a step each): stretches of the timetable as (state, first step, last step),
/// none running over its end.
fn windows(busy: &[bool]) -> Vec<(u8, usize, usize)> {
    let n = busy.len() as i64;
    let Some(free) = busy.iter().position(|b| !b) else {
        return vec![(DOWN, 0, busy.len() - 1)];
    };
    // The stretches a train is there, counted on from a step when none is, so that one
    // over the end of the list is whole.
    let mut runs: Vec<(i64, i64)> = Vec::new();
    for k in 1..=n {
        let i = free as i64 + k;
        if !busy[(i % n) as usize] {
            continue;
        }
        match runs.last_mut() {
            Some(run) if run.1 == i - 1 => run.1 = i,
            _ => runs.push((i, i)),
        }
    }
    if runs.is_empty() {
        return Vec::new();
    }
    let lead = WARN + LOWER + HOLD;
    let opens = |run: (i64, i64)| run.1 + 1 + CLEAR + RAISE;
    // Trains too close together are one closure.
    let mut closures: Vec<(i64, i64)> = Vec::new();
    for run in runs {
        match closures.last_mut() {
            Some(last) if run.0 - lead - opens(*last) < MIN_OPEN => last.1 = run.1,
            _ => closures.push(run),
        }
    }
    // And so are the last and the first, round the end of the timetable.
    while closures.len() > 1 {
        let (first, last) = (closures[0], closures[closures.len() - 1]);
        if first.0 + n - lead - opens(last) >= MIN_OPEN {
            break;
        }
        closures.pop();
        closures.remove(0);
        closures.push((last.0, first.1 + n));
    }
    if let [only] = closures[..] {
        if only.0 + n - lead - opens(only) < MIN_OPEN {
            return vec![(DOWN, 0, busy.len() - 1)];
        }
    }
    let mut out = Vec::new();
    let mut stretch = |state: u8, from: i64, until: i64| {
        // Steps from..until, wrapped into the list.
        let (a, b) = (from.rem_euclid(n), (until - 1).rem_euclid(n));
        if until - from >= n {
            out.push((state, 0, (n - 1) as usize));
        } else if a <= b {
            out.push((state, a as usize, b as usize));
        } else {
            out.push((state, a as usize, (n - 1) as usize));
            out.push((state, 0, b as usize));
        }
    };
    for (a, b) in closures {
        stretch(WARNING, a - lead, a - LOWER - HOLD);
        stretch(LOWERING, a - LOWER - HOLD, a - HOLD);
        stretch(DOWN, a - HOLD, b + 1 + CLEAR);
        stretch(RISING, b + 1 + CLEAR, b + 1 + CLEAR + RAISE);
    }
    out
}

/// A road over a railway for the tests here and of the cars: a line with trains on it
/// from west to east through a 2000 block square, and a street from north to south that
/// crosses it in the middle, at node 9.
#[cfg(test)]
pub(crate) mod scene {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;

    pub(crate) fn node(id: u64, x: i32, z: i32, tags: &[(&str, &str)]) -> ProcessedNode {
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

    pub(crate) fn way(
        id: u64,
        nodes: Vec<ProcessedNode>,
        tags: &[(&str, &str)],
    ) -> ProcessedElement {
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    pub(crate) fn world() -> XZBBox {
        XZBBox::rect_from_min_max(0, 0, 2000, 2000).unwrap()
    }

    /// The crossing node, with what else it is tagged.
    pub(crate) fn crossing(id: u64, x: i32, z: i32, extra: &[(&str, &str)]) -> ProcessedNode {
        let mut tags = vec![("railway", "level_crossing")];
        tags.extend_from_slice(extra);
        node(id, x, z, &tags)
    }

    /// The railway along z = 1000 and the street along x = 1000, tagged as given.
    pub(crate) fn elements(
        road: &[(&str, &str)],
        node_tags: &[(&str, &str)],
    ) -> Vec<ProcessedElement> {
        let mut tags = vec![("highway", "residential")];
        tags.extend_from_slice(road);
        vec![
            way(
                50,
                vec![
                    node(100, 0, 1000, &[]),
                    crossing(9, 1000, 1000, node_tags),
                    node(101, 2000, 1000, &[]),
                ],
                &[("railway", "rail")],
            ),
            way(
                1,
                vec![
                    node(1, 1000, 900, &[]),
                    crossing(9, 1000, 1000, node_tags),
                    node(2, 1000, 1100, &[]),
                ],
                &tags,
            ),
        ]
    }

    /// The street's surface: two blocks either side of its middle.
    pub(crate) fn mask() -> RoadMaskBitmap {
        let mut mask = RoadMaskBitmap::new(&world());
        for z in 900..=1100 {
            for x in 998..=1002 {
                mask.set(x, z);
            }
        }
        mask
    }

    pub(crate) fn trains(elements: &[ProcessedElement]) -> TrainNetwork {
        TrainNetwork::build(elements, 1.0, &world())
    }

    pub(crate) fn plan(elements: &[ProcessedElement], drives_on_left: bool) -> CrossingPlan {
        CrossingPlan::build(elements, &trains(elements), 1.0, drives_on_left, &mask())
    }

    pub(crate) fn standing(elements: &[ProcessedElement], draw: Draw) -> CrossingPlan {
        CrossingPlan::standing(elements, 1.0, false, &mask(), draw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_street_over_a_railway_gets_a_mast_and_a_boom_on_each_approach() {
        let elements = scene::elements(&[], &[]);
        let plan = scene::plan(&elements, false);
        assert_eq!(plan.crossing_count(), 1);
        assert!(plan.crossings[0].booms && plan.crossings[0].lamps);
        assert!(plan.signed(9));
        assert_eq!(plan.gates.len(), 2);
        let mask = scene::mask();
        for gate in &plan.gates {
            let (x, z) = gate.post;
            assert!(!mask.contains(x, z), "mast on the road at {x},{z}");
            // Four blocks short of the track's middle: two clear of it and two more.
            assert_eq!((f64::from(z) + 0.5 - 1000.5).abs(), 4.0, "{gate:?}");
            // It faces the traffic coming towards the tracks, from its own side of them.
            let towards_tracks = (1000.5 - (f64::from(z) + 0.5)).signum();
            assert_eq!(gate.facing, (0.0, -towards_tracks));
            // Traffic keeps right, so the mast is on the right of those it faces, and its
            // boom lies in towards the road and ends at the road's middle.
            let right_of_travel = -towards_tracks;
            assert_eq!((f64::from(x) + 0.5 - 1000.5).signum(), right_of_travel);
            assert_eq!(gate.reach, (-right_of_travel, 0.0));
            assert_eq!(gate.boom, 2.75);
        }
        // Where traffic keeps left the masts stand on the other kerbs.
        let left = scene::plan(&elements, true);
        for (a, b) in plan.gates.iter().zip(&left.gates) {
            assert_eq!(a.post.1, b.post.1);
            assert_eq!(a.post.0 - 1000, 1000 - b.post.0);
            assert_eq!(b.reach, (-a.reach.0, 0.0));
        }
    }

    #[test]
    fn a_one_way_street_gets_one_boom_right_across_it() {
        let plan = scene::plan(&scene::elements(&[("oneway", "yes")], &[]), false);
        assert_eq!(plan.gates.len(), 1);
        let gate = plan.gates[0];
        // Southbound traffic only: the mast north of the tracks, its boom out over both
        // halves of the road to its far edge.
        assert_eq!(gate.post, (997, 996));
        assert_eq!(gate.boom, 5.5);
        let back = scene::plan(&scene::elements(&[("oneway", "-1")], &[]), false);
        assert_eq!(back.gates.len(), 1);
        assert_eq!(back.gates[0].post, (1003, 1004));
    }

    #[test]
    fn only_a_track_that_really_crosses_makes_a_crossing_and_only_trains_close_it() {
        // A siding carries no trains: its crossing stands open, and cars drive on over it.
        let mut idle = scene::elements(&[], &[]);
        if let ProcessedElement::Way(track) = &mut idle[0] {
            track.tags.insert("service".into(), "siding".into());
        }
        let open = scene::plan(&idle, false);
        assert_eq!((open.crossing_count(), open.closing_count()), (1, 0));
        assert!(open.crossings[0].booms && open.gates.len() == 2 && open.signed(9));
        assert!(open.break_at(1, 9).is_none());
        assert!(open
            .pack_functions(&scene::trains(&idle), |_| true)
            .is_empty());
        let closing = scene::plan(&scene::elements(&[], &[]), false);
        assert_eq!(closing.closing_count(), 1);
        // A road beside the line, sharing a node tagged as a crossing, is not crossed by it.
        let along = vec![
            scene::way(
                50,
                vec![
                    scene::node(100, 0, 1000, &[]),
                    scene::crossing(9, 1000, 1000, &[]),
                    scene::node(101, 2000, 1000, &[]),
                ],
                &[("railway", "rail")],
            ),
            scene::way(
                1,
                vec![
                    scene::node(1, 900, 1010, &[]),
                    scene::crossing(9, 1000, 1000, &[]),
                    scene::node(2, 1100, 990, &[]),
                ],
                &[("highway", "residential")],
            ),
        ];
        assert!(scene::plan(&along, false).is_empty());
        // No track at all: nothing to plan.
        let road_only = vec![scene::elements(&[], &[]).remove(1)];
        assert!(scene::plan(&road_only, false).is_empty());
        assert!(scene::standing(&road_only, Draw::Displays).is_empty());
    }

    #[test]
    fn without_the_pack_every_crossing_stands_open() {
        let elements = scene::elements(&[], &[]);
        let plan = scene::standing(&elements, Draw::Displays);
        assert_eq!((plan.crossing_count(), plan.closing_count()), (1, 0));
        assert!(plan.crossings[0].booms && plan.signed(9));
        // The masts stand where the pack's would, and cars have nothing to wait for.
        let models = scene::plan(&elements, false);
        let posts = |p: &CrossingPlan| p.gates.iter().map(|g| g.post).collect::<Vec<_>>();
        assert_eq!(posts(&plan), posts(&models));
        assert!(plan.break_at(1, 9).is_none() && plan.looks().is_empty());
        // The track says what it is: a tramway's has lamps and no booms.
        let mut tram = scene::elements(&[], &[]);
        if let ProcessedElement::Way(track) = &mut tram[0] {
            track.tags.insert("railway".into(), "tram".into());
        }
        let tram = scene::standing(&tram, Draw::Blocks);
        assert!(!tram.crossings[0].booms && tram.crossings[0].lamps);
        let barred = scene::standing(
            &scene::elements(&[], &[("crossing:barrier", "no"), ("crossing:light", "no")]),
            Draw::Displays,
        );
        assert!(barred.gates.is_empty() && !barred.signed(9));
    }

    #[test]
    fn a_path_over_the_tracks_is_a_crossing_walkers_wait_at() {
        let mut elements = scene::elements(&[], &[]);
        // One path crosses the line between its nodes, one at a node mapped as a crossing,
        // one on a bridge over it, and one runs along beside it.
        if let ProcessedElement::Way(track) = &mut elements[0] {
            let node = scene::node(300, 1500, 1000, &[("railway", "crossing")]);
            track.nodes.insert(2, node);
        }
        let footway = &[("highway", "footway")];
        elements.extend([
            scene::way(
                60,
                vec![
                    scene::node(200, 500, 950, &[]),
                    scene::node(201, 500, 1050, &[]),
                ],
                footway,
            ),
            scene::way(
                61,
                vec![
                    scene::node(210, 1500, 950, &[]),
                    scene::node(300, 1500, 1000, &[("railway", "crossing")]),
                    scene::node(211, 1500, 1050, &[]),
                ],
                footway,
            ),
            scene::way(
                62,
                vec![
                    scene::node(220, 700, 950, &[]),
                    scene::node(221, 700, 1050, &[]),
                ],
                &[("highway", "footway"), ("bridge", "yes")],
            ),
            scene::way(
                63,
                vec![
                    scene::node(230, 200, 1003, &[]),
                    scene::node(231, 400, 1004, &[]),
                ],
                footway,
            ),
        ]);
        let plan = scene::plan(&elements, false);
        // The road's crossing, and one of its own for each path walkers cross.
        assert_eq!((plan.crossing_count(), plan.closing_count()), (3, 3));
        let stop_at = |x: f64| {
            plan.walk_stops()
                .iter()
                .find(|s| (s.at.0 - x).abs() < 1.0)
                .copied()
        };
        assert!(stop_at(700.5).is_none() && stop_at(300.5).is_none());
        let functions = plan.pack_functions(&scene::trains(&elements), |_| true);
        for x in [500.5, 1500.5, 1000.5] {
            let stop = stop_at(x).unwrap_or_else(|| panic!("no stop at {x}"));
            assert!((stop.at.1 - 1000.5).abs() < 0.6, "{stop:?}");
            assert!(stop.track.0.abs() > 0.99, "{stop:?}");
            // Closed for the trains as the road's is, with nothing to build.
            let c = stop.crossing as usize;
            assert!(!plan.crossings[c].tracks.is_empty());
            let path = format!("data/arnis/function/xing/c/{c}.mcfunction");
            assert!(functions.iter().any(|(p, _)| *p == path), "{path}");
            if x != 1000.5 {
                assert!(plan.crossings[c].gates.is_empty());
                assert!(plan.breaks.values().all(|b| b.crossing as usize != c));
            }
        }
        // Without trains nobody waits.
        assert!(scene::standing(&elements, Draw::Displays)
            .walk_stops()
            .is_empty());
    }

    #[test]
    fn standing_open_on_java_is_vanilla_blocks_as_displays() {
        use crate::coordinate_system::geographic::LLBBox;

        let elements = scene::elements(&[], &[]);
        let plan = scene::standing(&elements, Draw::Displays);
        let world = scene::world();
        let llbbox = LLBBox::new(47.0, 8.0, 47.01, 8.01).unwrap();
        let mut editor = WorldEditor::new(std::env::temp_dir(), &world, llbbox);
        plan.place(&mut editor, &scene::crossing(9, 1000, 1000, &[]));
        assert!(editor.item_displays().is_empty());
        let parts = editor.entities_of("minecraft:block_display");
        let per_gate = standing_pieces(true, 2.75, true).len();
        assert_eq!(parts.len(), 2 * per_gate);
        let floats = |v: Option<&Value>| match v {
            Some(Value::List(v)) => v
                .iter()
                .map(|f| match f {
                    Value::Float(f) => f64::from(*f),
                    Value::Double(d) => *d,
                    _ => f64::NAN,
                })
                .collect::<Vec<f64>>(),
            _ => Vec::new(),
        };
        let mask = scene::mask();
        let mut boom_tops = Vec::new();
        for part in &parts {
            // Nothing the pack would look for.
            assert!(part.get("Tags").is_none());
            let pos = floats(part.get("Pos"));
            let gate = plan
                .gates
                .iter()
                .find(|g| {
                    (f64::from(g.post.0) + 0.5 - pos[0]).abs() < 1e-6
                        && (f64::from(g.post.1) + 0.5 - pos[2]).abs() < 1e-6
                })
                .expect("every piece stands at a mast");
            assert_eq!(floats(part.get("Rotation"))[0] as f32, yaw_of(gate.facing));
            let Some(Value::Compound(t)) = part.get("transformation") else {
                panic!("no transformation");
            };
            let (q, tr, s) = (
                floats(t.get("left_rotation")),
                floats(t.get("translation")),
                floats(t.get("scale")),
            );
            // A boom stripe is red or white and turned most: up from level, not the cross's
            // 45 degrees.
            let block = match part.get("block_state") {
                Some(Value::Compound(b)) => match b.get("Name") {
                    Some(Value::String(name)) => name.clone(),
                    _ => String::new(),
                },
                _ => String::new(),
            };
            let striped = matches!(
                block.as_str(),
                "minecraft:red_concrete" | "minecraft:white_concrete"
            );
            if q[2].abs() < 0.6 || !striped {
                continue;
            }
            // Where its far end is, across from the mast and up.
            let a = 2.0 * q[2].atan2(q[3]);
            let end = (tr[0] + s[0] * a.cos(), tr[1] + s[0] * a.sin());
            let right = (gate.facing.1, -gate.facing.0);
            let toward_road = gate.reach.0 * right.0 + gate.reach.1 * right.1;
            assert!(end.0 * toward_road >= 0.0, "a boom leans to the road");
            let (x, z) = (
                pos[0] + right.0 * end.0 + gate.facing.0 * tr[2],
                pos[2] + right.1 * end.0 + gate.facing.1 * tr[2],
            );
            boom_tops.push(end.1);
            // Up, it stays over its own kerb.
            assert!(!mask.contains(x.floor() as i32, z.floor() as i32));
        }
        assert!(!boom_tops.is_empty());
        assert!(
            boom_tops.iter().cloned().fold(0.0, f64::max) > 3.0,
            "{boom_tops:?}"
        );
    }

    #[test]
    fn standing_open_elsewhere_is_a_post_a_banner_and_a_boom_of_walls() {
        use crate::coordinate_system::geographic::LLBBox;

        let elements = scene::elements(&[], &[]);
        let plan = scene::standing(&elements, Draw::Blocks);
        let world = scene::world();
        let llbbox = LLBBox::new(47.0, 8.0, 47.01, 8.01).unwrap();
        let mut editor = WorldEditor::new(std::env::temp_dir(), &world, llbbox);
        plan.place(&mut editor, &scene::crossing(9, 1000, 1000, &[]));
        assert!(editor.entities_of("minecraft:block_display").is_empty());
        let mask = scene::mask();
        for gate in &plan.gates {
            let (x, z) = gate.post;
            let ground = editor.get_absolute_y(x, 0, z);
            let at = |x: i32, y: i32, z: i32| editor.get_block_absolute(x, y, z);
            assert_eq!(at(x, ground + 1, z), Some(COBBLESTONE_WALL));
            assert_eq!(at(x, ground + 2, z), Some(IRON_BARS));
            assert_eq!(at(x, ground + 3, z), Some(LIGHT_GRAY_CONCRETE));
            let (_, (fx, fz)) = cardinal(gate.facing);
            assert_eq!(at(x + fx, ground + 3, z + fz), Some(WHITE_WALL_BANNER));
            // The boom behind the mast, off the road, white at the foot and red at the top.
            let (bx, bz) = (x - fx, z - fz);
            assert!(!mask.contains(bx, bz));
            assert_eq!(at(bx, ground + 1, bz), Some(DIORITE_WALL));
            let rows = (gate.boom.round() as i32).clamp(2, 8);
            assert_eq!(at(bx, ground + rows, bz), Some(BRICK_WALL));
        }
    }

    #[test]
    fn what_a_crossing_is_mapped_with_decides_its_booms_and_lamps() {
        let lamps_only = scene::plan(&scene::elements(&[], &[("crossing:barrier", "no")]), false);
        assert!(!lamps_only.crossings[0].booms && lamps_only.crossings[0].lamps);
        assert_eq!(lamps_only.gates.len(), 2);
        // Neither: no masts, and the sign it had stays; cars still wait for its trains.
        let bare = scene::plan(
            &scene::elements(&[], &[("crossing:barrier", "no"), ("crossing:light", "no")]),
            false,
        );
        assert!(bare.gates.is_empty() && !bare.signed(9) && bare.looks().is_empty());
        assert!(bare.break_at(1, 9).is_some());
        assert_eq!(bare.crossing_count(), 1);
        // A tramway's crossing has lamps and no booms, unless mapped with them.
        let tram = |node_tags: &[(&str, &str)]| {
            let mut elements = scene::elements(&[], node_tags);
            if let ProcessedElement::Way(track) = &mut elements[0] {
                track.tags.insert("railway".into(), "tram".into());
            }
            scene::plan(&elements, false)
        };
        assert!(!tram(&[]).crossings[0].booms && tram(&[]).crossings[0].lamps);
        assert!(tram(&[("crossing:barrier", "half")]).crossings[0].booms);
    }

    #[test]
    fn lanes_break_far_enough_from_the_tracks_however_they_cross() {
        let square = scene::plan(&scene::elements(&[], &[]), false);
        let b = square.break_at(1, 9).unwrap();
        // The front of a car stops 5.5 blocks short: two clear of the track, two to the
        // boom, one and a half short of it. Past the tracks it is clear after 2.5.
        assert_eq!((b.stop(true), b.stop(false)), (5.5, 5.5));
        assert_eq!((b.exit(true), b.exit(false)), (2.5, 2.5));
        assert_eq!(b.crossing, 0);
        assert!(square.break_at(1, 1).is_none() && square.break_at(50, 9).is_none());

        // A track askew reaches further along the road, at its kerbs and beyond them.
        let askew = vec![
            scene::way(
                50,
                vec![
                    scene::node(100, 0, 0, &[]),
                    scene::crossing(9, 1000, 1000, &[]),
                    scene::node(101, 2000, 2000, &[]),
                ],
                &[("railway", "rail")],
            ),
            scene::elements(&[], &[]).remove(1),
        ];
        let plan = scene::plan(&askew, false);
        let b = plan.break_at(1, 9).unwrap();
        // Five blocks across to the furthest kerb, at 45 degrees as far along, and the
        // track's two blocks as seen along the road.
        let reach = 5.0 + 2.0 * std::f64::consts::SQRT_2;
        assert!((b.stop(true) - (reach + 3.5)).abs() < 0.05, "{b:?}");
        assert!((b.exit(true) - (reach + 0.5)).abs() < 0.05, "{b:?}");
        for gate in &plan.gates {
            let from_track = (f64::from(gate.post.1) + 0.5 - 1000.5).abs();
            assert!(from_track > reach, "{gate:?}");
        }
    }

    #[test]
    fn the_two_tracks_of_a_double_line_are_one_crossing() {
        // A second track five blocks south of the first, both over the same street.
        let mut elements = scene::elements(&[], &[]);
        elements.push(scene::way(
            51,
            vec![
                scene::node(102, 0, 1005, &[]),
                scene::crossing(10, 1000, 1005, &[]),
                scene::node(103, 2000, 1005, &[]),
            ],
            &[("railway", "rail")],
        ));
        if let ProcessedElement::Way(road) = &mut elements[1] {
            road.nodes.insert(2, scene::crossing(10, 1000, 1005, &[]));
        }
        let plan = scene::plan(&elements, false);
        assert_eq!(plan.crossing_count(), 1);
        assert_eq!(plan.gates.len(), 2);
        assert!(plan.signed(9) && plan.signed(10));
        assert!(plan.crossings[0].tracks.len() >= 2);
        // The lanes break once, at the first track, and clear the second too.
        let b = plan.break_at(1, 9).unwrap();
        assert!(plan.break_at(1, 10).is_none());
        assert_eq!((b.stop(true), b.exit(false)), (5.5, 2.5));
        assert_eq!((b.stop(false), b.exit(true)), (10.5, 7.5));
        let posts: Vec<i32> = plan.gates.iter().map(|g| g.post.1).collect();
        assert_eq!(posts, vec![996, 1009]);
    }

    #[test]
    fn a_street_mapped_as_two_ways_that_meet_on_the_tracks_is_one_crossing() {
        let mut elements = scene::elements(&[], &[]);
        elements[1] = scene::way(
            1,
            vec![
                scene::node(1, 1000, 900, &[]),
                scene::crossing(9, 1000, 1000, &[]),
            ],
            &[("highway", "residential")],
        );
        elements.push(scene::way(
            2,
            vec![
                scene::crossing(9, 1000, 1000, &[]),
                scene::node(2, 1000, 1100, &[]),
            ],
            &[("highway", "residential")],
        ));
        let plan = scene::plan(&elements, false);
        assert_eq!(plan.crossing_count(), 1);
        // Each way brings the mast for the traffic that comes along it.
        let mut posts: Vec<(i32, i32)> = plan.gates.iter().map(|g| g.post).collect();
        posts.sort_unstable();
        assert_eq!(posts, vec![(997, 996), (1003, 1004)]);
        let (a, b) = (plan.break_at(1, 9).unwrap(), plan.break_at(2, 9).unwrap());
        assert_eq!((a.crossing, b.crossing), (0, 0));
        assert_eq!((a.stop(true), b.stop(false)), (5.5, 5.5));
    }

    #[test]
    fn masts_and_booms_are_displays_the_pack_finds_by_their_crossing() {
        use crate::coordinate_system::geographic::LLBBox;

        let elements = scene::elements(&[], &[]);
        let plan = scene::plan(&elements, false);
        let llbbox = LLBBox::new(47.0, 8.0, 47.01, 8.01).unwrap();
        let world = scene::world();
        let mut editor = WorldEditor::new(std::env::temp_dir(), &world, llbbox);
        plan.place(&mut editor, &scene::crossing(9, 1000, 1000, &[]));
        // Another node builds nothing.
        plan.place(&mut editor, &scene::node(1, 1000, 900, &[]));
        let parts = editor.item_displays();
        assert_eq!(parts.len(), 4);
        let tags = |p: &HashMap<String, Value>| -> Vec<String> {
            match p.get("Tags") {
                Some(Value::List(tags)) => tags
                    .iter()
                    .filter_map(|t| match t {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            }
        };
        let item = |p: &HashMap<String, Value>| match p.get("item") {
            Some(Value::Compound(item)) => match item.get("components") {
                Some(Value::Compound(c)) => c.clone(),
                _ => HashMap::new(),
            },
            _ => HashMap::new(),
        };
        let drawn: Vec<String> = plan
            .looks()
            .iter()
            .map(vehicle_models::item_model)
            .collect();
        let (mut masts, mut booms, mut bells) = (0, 0, 0);
        for part in &parts {
            let tags = tags(part);
            assert!(
                tags.contains(&"arnis.xp".to_string()) && tags.contains(&"arnis.x0".to_string())
            );
            // Its crossing's number is where catch_up reads it, its model one the pack draws.
            let components = item(part);
            assert_eq!(
                components.get("minecraft:custom_data"),
                Some(&Value::Compound(HashMap::from([(
                    "x".to_string(),
                    Value::Int(0)
                )])))
            );
            let Some(Value::String(model)) = components.get("minecraft:item_model") else {
                panic!("no model");
            };
            assert!(drawn.contains(model), "{model}");
            bells += usize::from(tags.contains(&"arnis.xbell".to_string()));
            if tags.contains(&"arnis.xboom".to_string()) {
                booms += 1;
                assert_eq!(model, "arnis:vehicle/xing_boom_std");
                let Some(Value::Compound(t)) = part.get("transformation") else {
                    panic!("no transformation");
                };
                // Up, and stretched from the model's four metres to the 2.75 it spans.
                let floats = |key: &str| match t.get(key) {
                    Some(Value::List(v)) => v
                        .iter()
                        .map(|f| match f {
                            Value::Float(f) => *f,
                            _ => f32::NAN,
                        })
                        .collect::<Vec<f32>>(),
                    _ => Vec::new(),
                };
                let up = boom_turn(true).map(|v| v as f32);
                assert_eq!(floats("left_rotation"), up.to_vec());
                let scale = floats("scale");
                assert!((scale[0] / scale[1] - 2.75 / 4.0).abs() < 1e-4, "{scale:?}");
                assert_eq!(scale[1], scale[2]);
                assert_eq!(floats("translation"), vec![0.0, 0.0, 0.0]);
            } else {
                masts += 1;
                assert!(tags.contains(&"arnis.xlamp".to_string()));
                assert_eq!(model, "arnis:vehicle/xing_light_off_std");
            }
        }
        assert_eq!((masts, booms, bells), (2, 2, 1));
    }

    /// What the pack's `check` makes of a clock reading: the state of crossing 0.
    fn checked(check: &str, now: i64) -> u8 {
        let mut scores: HashMap<String, i64> = HashMap::new();
        scores.insert("#now".into(), now);
        let mut state = 0;
        let mut seen = false;
        for line in check.lines() {
            let words: Vec<&str> = line.split(' ').collect();
            match words.as_slice() {
                ["scoreboard", "players", "operation", a, _, "=", b, _] => {
                    let v = scores[*b];
                    scores.insert(a.to_string(), v);
                }
                ["scoreboard", "players", "operation", a, _, "%=", b, _] => {
                    let v = scores[*a].rem_euclid(scores[*b]);
                    scores.insert(a.to_string(), v);
                }
                ["scoreboard", "players", "set", a, _, v] => {
                    scores.insert(a.to_string(), v.parse().unwrap());
                }
                ["execute", "if", "score", a, _, "matches", range, "run", "scoreboard", "players", "set", "#n", _, v] =>
                {
                    let (lo, hi) = range.split_once("..").unwrap();
                    let (lo, hi): (i64, i64) = (lo.parse().unwrap(), hi.parse().unwrap());
                    if (lo..=hi).contains(&scores[*a]) {
                        scores.insert("#n".into(), v.parse().unwrap());
                    }
                }
                ["execute", "unless", "score", "#n", _, "=", "#x0", _, "run", "function", "arnis:xing/c/0"] =>
                {
                    state = scores["#n"] as u8;
                    seen = true;
                }
                [first, ..] if first.starts_with('#') || first.is_empty() => {}
                other => panic!("check has a line the test does not read: {other:?}"),
            }
        }
        assert!(seen, "crossing 0 is not checked");
        state
    }

    #[test]
    fn the_pack_closes_a_crossing_for_every_train_of_its_timetable() {
        let elements = scene::elements(&[], &[]);
        let trains = scene::trains(&elements);
        let plan = CrossingPlan::build(&elements, &trains, 1.0, false, &scene::mask());
        let functions = plan.pack_functions(&trains, |_| true);
        let text = |name: &str| {
            functions
                .iter()
                .find(|(path, _)| path == &format!("data/arnis/function/xing/{name}.mcfunction"))
                .map(|(_, text)| text.as_str())
                .unwrap_or_else(|| panic!("no {name}"))
        };
        let check = text("check");
        let track = plan.crossings[0].tracks[0];
        let busy = trains.busy(track.service, track.at, track.half);
        let n = busy.len() as i64;
        assert!(busy.iter().any(|b| *b) && busy.iter().any(|b| !*b));
        assert!(check.contains(&format!("scoreboard players set #k arnis.xing {n}\n")));
        let mut states = [0usize; 5];
        for now in 0..n {
            let state = checked(check, now);
            states[state as usize] += 1;
            let at = |k: i64| busy[(now + k).rem_euclid(n) as usize];
            if at(0) {
                assert_eq!(state, DOWN, "a train on an open crossing at step {now}");
            }
            // Five seconds before a train the booms are down, twelve before the lamps flash.
            if at(HOLD) {
                assert_eq!(state, DOWN, "step {now}");
            }
            if at(WARN + LOWER + HOLD) {
                assert!(state >= WARNING, "step {now}");
            }
            // With no train within half a minute either way it is open.
            if (-300..=300).all(|k| !at(k)) {
                assert_eq!(state, 0, "step {now}");
            }
        }
        // Every state comes up, and the clock runs on past the end of the timetable.
        assert!(states.iter().all(|&steps| steps > 0), "{states:?}");
        assert_eq!(checked(check, 3 * n + 17), checked(check, 17));

        let switch = text("c/0");
        assert!(switch.starts_with("scoreboard players operation #x0 arnis.xing = #n arnis.xing\n"));
        assert!(switch.contains(
            "execute as @e[type=item_display,tag=arnis.x0,x=995,y=-512,z=994,dx=10,dy=2048,dz=12] run function arnis:xing/apply\n"
        ));
        // As the booms start down, cars standing on the track across the street go.
        let cleared = switch
            .lines()
            .filter(|l| l.starts_with("execute if score #n arnis.xing matches 3 as @e[type=marker,tag=arnis.car,tag=arnis.car.still,"))
            .count();
        assert_eq!(cleared, 3);
        // Everything short of the line cars stop at, 5.5 blocks from the track's middle:
        // a car that had to stop past it has its front on the track.
        assert!(switch.contains("tag=arnis.car.still,x=995.5,y=-512,z=995.5,dx=9,dy=2048,dz=9]"));
        assert!(switch.ends_with("function arnis:xing/changed\n"));
        assert_eq!(text("boom"), boom_function());

        // A service the pack does not run closes nothing, and with none there is no pack.
        assert!(plan.pack_functions(&trains, |_| false).is_empty());
    }

    /// The pack names the masts' models as it switches them: each must be one drawn.
    #[test]
    fn the_lamps_switch_between_models_the_pack_draws() {
        let plan = scene::plan(&scene::elements(&[], &[]), false);
        let drawn: Vec<String> = plan
            .looks()
            .iter()
            .map(vehicle_models::item_model)
            .collect();
        let named = [
            include_str!(
                "../../assets/minecraft/datapack_extras/data/arnis/function/xing/dark.mcfunction"
            ),
            include_str!(
                "../../assets/minecraft/datapack_extras/data/arnis/function/xing/flash.mcfunction"
            ),
        ];
        let mut found = 0;
        for line in named.iter().flat_map(|text| text.lines()) {
            if let Some((_, id)) = line.split_once("set value ") {
                assert!(drawn.contains(&id.trim_matches('"').to_string()), "{id}");
                found += 1;
            }
        }
        assert_eq!(found, 3);
        // The states the functions test for are the ones numbered here.
        let apply = include_str!(
            "../../assets/minecraft/datapack_extras/data/arnis/function/xing/apply.mcfunction"
        );
        assert!(apply.contains(&format!(
            "matches {WARNING}..{LOWERING} if entity @s[tag=arnis.xbell]"
        )));
        assert!(boom_function().contains(&format!("matches {LOWERING}..{DOWN} run")));
        let stop = include_str!(
            "../../assets/minecraft/datapack_extras/data/arnis/function/cars/crossing.mcfunction"
        );
        assert!(stop.contains(&format!(
            "matches {RISING}.. run scoreboard players set #red"
        )));
        let light = include_str!(
            "../../assets/minecraft/datapack_extras/data/arnis/function/cars/light.mcfunction"
        );
        assert!(light.contains(&format!(
            "data{{g:{STOP_GROUP}b}} run return run function arnis:cars/crossing"
        )));
    }

    /// The state a list of stretches gives at a step: the highest that holds there.
    fn state_at(stretches: &[(u8, usize, usize)], step: usize) -> u8 {
        stretches
            .iter()
            .filter(|&&(_, from, to)| (from..=to).contains(&step))
            .map(|&(state, _, _)| state)
            .max()
            .unwrap_or(0)
    }

    fn busy(n: usize, runs: &[(usize, usize)]) -> Vec<bool> {
        let mut out = vec![false; n];
        for &(a, b) in runs {
            for step in a..=b {
                out[step % n] = true;
            }
        }
        out
    }

    #[test]
    fn a_train_closes_its_crossing_ahead_of_it_and_opens_it_behind() {
        let stretches = windows(&busy(2000, &[(500, 560)]));
        // Open until twelve seconds before the train, then lamps, booms coming down, down.
        assert_eq!(state_at(&stretches, 379), 0);
        assert_eq!(state_at(&stretches, 380), WARNING);
        assert_eq!(state_at(&stretches, 409), WARNING);
        assert_eq!(state_at(&stretches, 410), LOWERING);
        assert_eq!(state_at(&stretches, 449), LOWERING);
        assert_eq!(state_at(&stretches, 450), DOWN);
        // Down for the five seconds before it, all the while it is there and a second after.
        assert!((450..=570).all(|s| state_at(&stretches, s) == DOWN));
        assert_eq!(state_at(&stretches, 571), RISING);
        assert_eq!(state_at(&stretches, 600), RISING);
        assert_eq!(state_at(&stretches, 601), 0);
        // No stretch runs over the end of the timetable.
        assert!(stretches.iter().all(|&(_, a, b)| a <= b && b < 2000));
    }

    #[test]
    fn a_train_over_the_end_of_the_timetable_closes_it_round_the_end() {
        // There from step 1990 round to step 20.
        let stretches = windows(&busy(2000, &[(1990, 2020)]));
        assert_eq!(state_at(&stretches, 1869), 0);
        assert_eq!(state_at(&stretches, 1870), WARNING);
        assert_eq!(state_at(&stretches, 1995), DOWN);
        assert_eq!(state_at(&stretches, 0), DOWN);
        assert_eq!(state_at(&stretches, 30), DOWN);
        assert_eq!(state_at(&stretches, 31), RISING);
        assert_eq!(state_at(&stretches, 60), RISING);
        assert_eq!(state_at(&stretches, 61), 0);
        // A closure that starts before step 0 warns at the end of the list.
        let early = windows(&busy(2000, &[(50, 90)]));
        assert_eq!(state_at(&early, 1930), WARNING);
        assert_eq!(state_at(&early, 1929), 0);
        assert_eq!(state_at(&early, 20), DOWN);
    }

    #[test]
    fn trains_close_together_keep_the_booms_down_between_them() {
        // The second comes fifteen seconds after the first has gone: the booms would be up
        // for under eight seconds, so they stay down.
        let close = windows(&busy(4000, &[(500, 560), (711, 770)]));
        assert!((450..=780).all(|s| state_at(&close, s) == DOWN));
        assert_eq!(state_at(&close, 781), RISING);
        // Thirty seconds apart they open in between.
        let apart = windows(&busy(4000, &[(500, 560), (861, 920)]));
        assert_eq!(state_at(&apart, 650), 0);
        assert_eq!(state_at(&apart, 741), WARNING);
        // Nothing but trains, or a ring too short to open on: down throughout.
        assert_eq!(windows(&[true; 50]), vec![(DOWN, 0, 49)]);
        assert_eq!(windows(&busy(200, &[(10, 60)])), vec![(DOWN, 0, 199)]);
        assert!(windows(&[false; 50]).is_empty());
    }

    #[test]
    fn a_boom_stands_up_and_lies_towards_the_road() {
        // The display draws the model's +x, the boom, at its own -x; up, it points upwards
        // and still a little towards the road.
        let turned = |q: [f64; 4], v: [f64; 3]| {
            // v + 2w(u x v) + 2 u x (u x v) for the quaternion (u, w).
            let (u, w) = ([q[0], q[1], q[2]], q[3]);
            let cross = |a: [f64; 3], b: [f64; 3]| {
                [
                    a[1] * b[2] - a[2] * b[1],
                    a[2] * b[0] - a[0] * b[2],
                    a[0] * b[1] - a[1] * b[0],
                ]
            };
            let (c, cc) = (cross(u, v), cross(u, cross(u, v)));
            [0, 1, 2].map(|i| v[i] + 2.0 * w * c[i] + 2.0 * cc[i])
        };
        let level = turned(boom_turn(false), [-1.0, 0.0, 0.0]);
        assert_eq!(level, [-1.0, 0.0, 0.0]);
        let up = turned(boom_turn(true), [-1.0, 0.0, 0.0]);
        assert!(up[1] > 0.98 && up[0] < -0.1 && up[2].abs() < 1e-9, "{up:?}");
        // The model is fitted about its hinge, so the display turns it on the spot.
        let (t, _, _) = vehicle_models::display_transform("xing_boom", 1.0, false);
        assert_eq!(t, [0.0, 0.0, 0.0]);
        // And the hinge on the mast is where the boom's display is put.
        let bounds = vehicle_models::model("xing_light_off").meta().bounds;
        assert!(bounds[0][2] / 32.0 <= -HINGE_FRONT_M + 0.1, "{bounds:?}");
        let text = boom_function();
        assert!(text.contains("interpolation_duration:80,transformation:{left_rotation:[0.0000f,0.0000f,0.0000f,1.0000f]}"));
        assert!(text.contains("interpolation_duration:60,transformation:{left_rotation:[0.0000f,0.0000f,-0.6428f,0.7660f]}"));
    }
}
