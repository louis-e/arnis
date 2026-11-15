//! Aircraft at the mapped airfields, driven by the extras data pack as the trains are.
//! Runways, taxiways and stands are read into one graph an airfield. Each aircraft has a
//! stand and goes round for ever: pushed back, it taxis to a runway, takes off, flies a
//! circuit above the clouds, lands, and taxis back in. The stands left over hold
//! aircraft that stay where they are.
//!
//! Everything an airfield's aircraft move along is cut into pieces that are stored once,
//! in one route: each stretch of taxiway in each direction, each turn between two, the
//! climb off a runway and the circuit back to it. A timetable gives an aircraft's place
//! along that route at each step and jumps from one piece to the next where the two
//! meet, at the same spot and heading, so the jump does not show. Backing along a piece
//! is a pushback. The timetables are worked out one aircraft after another, each kept
//! clear of those before it on the ground, in the air and on the runway, so nothing in
//! the pack has to look out for anything.
//!
//! Distances are in blocks and times in steps here, as in `trains.rs`, whose pack
//! functions run these routes; an aircraft's place is the middle of its main wheels.

use crate::clipping::is_invented_node_id;
use crate::element_processing::plane_bodies::{Airframe, ALL};
use crate::element_processing::train_bodies::{Coach, Livery};
use crate::element_processing::trains::{
    hash, Air, End, Mode, Route, Service, Shape, MAX_STEP, STEPS_PER_S,
};
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use fnv::{FnvHashMap, FnvHashSet};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// Most aircraft a world gets that fly, and that one airfield gets.
const MAX_FLYING: usize = 36;
const MAX_FLYING_PER_FIELD: usize = 14;
/// Most aircraft a world gets that only stand.
const MAX_PARKED: usize = 80;
/// Most route points the airfields take, of those the pack holds in all.
const MAX_POINTS: usize = 110_000;
/// Chance a stand nobody flies from holds an aircraft.
const STAND_OCCUPANCY: f64 = 0.6;
/// Seconds each flying aircraft adds to the airfield's round for the runway it uses, the
/// shortest round and the longest.
const SLOT_S: f64 = 150.0;
const MIN_ROUND_S: f64 = 600.0;
const MAX_ROUND_S: f64 = 3000.0;
/// Least time an aircraft stands at its stand between two flights, in seconds.
const MIN_DWELL_S: f64 = 90.0;
/// Taxi speed, speed on a stand's lead-in line, while pushed back and round a tight
/// turn, m/s; and the sideways pull a taxiing aircraft turns with, m/s².
const TAXI_MS: f64 = 11.0;
const STAND_MS: f64 = 4.0;
const PUSH_MS: f64 = 1.6;
const PIVOT_MS: f64 = 2.5;
const TURN_PULL: f64 = 1.1;
/// Speed along the runway once the landing roll is braked, m/s.
const ROLL_OUT_MS: f64 = 20.0;
/// Speeding up and braking while taxiing, m/s².
const TAXI_ACCEL: f64 = 0.9;
const TAXI_BRAKE: f64 = 1.1;
/// Radius a taxiing aircraft rounds a corner with where there is room, in metres.
const CORNER_M: f64 = 32.0;
/// Radius its main wheels turn about to face the other way, and the run it takes to be
/// back on the line, in metres.
const PIVOT_M: f64 = 7.0;
const PIVOT_MERGE_M: f64 = 45.0;
/// How far from a runway's middle an aircraft waits to enter it, in metres.
const HOLD_SHORT_M: f64 = 60.0;
/// How far past the threshold aircraft touch down, in metres, and the most of a short
/// runway that may be.
const TOUCHDOWN_M: f64 = 300.0;
const TOUCHDOWN_SHARE: f64 = 0.14;
/// Runway left beyond the point an aircraft lifts off at, at the least, in metres.
const LIFT_OFF_MARGIN_M: f64 = 120.0;
/// The glide angle down to the runway, the steeper descent above it, and the height the
/// two meet at, in degrees and metres.
const GLIDE_DEG: f64 = 3.0;
const DESCENT_DEG: f64 = 6.0;
const INTERCEPT_M: f64 = 35.0;
/// Over how far the sink is taken out before the wheels touch, and the nose comes down
/// after, in metres.
const FLARE_M: f64 = 60.0;
const NOSE_DOWN_M: f64 = 140.0;
/// Nose-up attitude at touchdown, level in the circuit and on the glide, in degrees.
const TOUCHDOWN_NOSE: f64 = 5.0;
const LEVEL_NOSE: f64 = 2.0;
/// How far before touchdown the wheels come down, along the path, and how high after
/// lift-off they go up, in metres.
const GEAR_DOWN_M: f64 = 1300.0;
const GEAR_UP_M: f64 = 12.0;
/// Seconds an aircraft waits after its pushback and lined up on the runway.
const START_UP_S: f64 = 10.0;
const LINE_UP_S: f64 = 3.0;
/// Longest wait before a runway, in seconds, and how finely waits and starts are tried.
const MAX_HOLD_S: usize = 160;
const HOLD_STEP_S: usize = 4;
const START_STEP_S: usize = 3;
/// Gap two aircraft keep between wing or hull, in metres, and how far apart in height
/// they have nothing to do with each other.
const CLEAR_M: f64 = 4.5;
const CLEAR_HEIGHT_M: f64 = 14.0;
/// Seconds a runway is kept free ahead of a landing and either side of any use, and the
/// most of the airfield's round a runway is in use for.
const FINAL_S: f64 = 25.0;
const RUNWAY_PAD_S: i64 = 6;
const RUNWAY_SHARE: f64 = 0.9;
/// How far past where it lifts off a departure leaves the runway to the next, in metres,
/// where the runway's far end is further.
const CLIMB_OUT_M: f64 = 900.0;
/// The longer way an aircraft takes to a runway than to another for each aircraft already
/// flying from it, in metres of taxiing.
const RUNWAY_TAKEN_M: f64 = 2000.0;
/// Most times an aircraft turns all the way round off the downwind leg to land later, and
/// the time either side of a landing, in seconds, past which a landing is not worth a
/// turn more for the runway to be used more evenly.
const ORBITS: usize = 2;
const EVEN_S: i64 = 60;
/// How much longer the airfield's clock is made each time its rounds do not all fit.
const CLOCK_STEP: f64 = 1.03;
/// Ground a path keeps over: close to the runway, and anywhere else, in metres.
const CLEAR_RUNWAY_M: f64 = 2.0;
const CLEAR_TERRAIN_M: f64 = 20.0;
/// Shortest runway worth reading, in metres, and how far it may bend, as a share of its length.
const MIN_RUNWAY_M: f64 = 300.0;
const MAX_BEND: f64 = 0.03;
/// A runway follows on from another at a shared node if it turns less than this.
const SAME_RUNWAY_DEG: f64 = 20.0;
/// How far a stand that touches no taxiway reaches for one, in metres.
const STAND_REACH_M: f64 = 150.0;
/// Lines whose nose ends are this near each other lead to one stand, in metres.
const SAME_STAND_M: f64 = 8.0;
/// Three lines that end within this of the next are one stand drawn for aircraft of
/// several sizes, in metres.
const ALTERNATE_LINES_M: f64 = 28.0;
/// How much of its length an aircraft that only stands needs its stand's line to be.
const PARKED_LINE_SHARE: f64 = 0.75;
/// A taxiway that ends in nothing is somewhere to park if its last stretch is this long,
/// and the last this much of it is the stand, in metres.
const DEAD_END_M: f64 = 30.0;
const DEAD_END_STAND_M: f64 = 45.0;
/// Room a stand's line leaves past an aircraft's nose, and has to have past its tail,
/// in metres.
const NOSE_GAP_M: f64 = 1.5;
const TAIL_ROOM_M: f64 = 8.0;
/// Slowest an aircraft creeps to where it stops, in blocks a step.
const CREEP: f64 = 0.03;
/// The highest aircraft fly on Bedrock, whose script cannot put an entity above the
/// world's top at y 320.
pub const BEDROCK_CEILING: f64 = 316.0;
/// The lowest a circuit is worth flying over its runway, in metres.
const MIN_CIRCUIT_M: f64 = 25.0;
/// A building this tall stands in the way of a circuit, in metres.
const TALL_M: f64 = 45.0;

/// The circuit flown: its height over the runway, the radius of its turns, and how far
/// out from the runway's ends it turns, past the far end at the least and before the
/// threshold, in metres.
#[derive(Clone, Copy)]
struct Circuit {
    height: f64,
    radius: f64,
    beyond: f64,
    before: f64,
}

/// Airliners and jets fly a wide circuit above the clouds, light aircraft a tight low one.
const WIDE_CIRCUIT: Circuit = Circuit {
    height: 130.0,
    radius: 450.0,
    beyond: 500.0,
    before: 700.0,
};
const TIGHT_CIRCUIT: Circuit = Circuit {
    height: 60.0,
    radius: 160.0,
    beyond: 220.0,
    before: 320.0,
};

type P = (f64, f64);

fn sub(a: P, b: P) -> P {
    (a.0 - b.0, a.1 - b.1)
}

fn add(a: P, b: P) -> P {
    (a.0 + b.0, a.1 + b.1)
}

fn mul(a: P, k: f64) -> P {
    (a.0 * k, a.1 * k)
}

fn dot(a: P, b: P) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

fn cross(a: P, b: P) -> f64 {
    a.0 * b.1 - a.1 * b.0
}

fn norm(a: P) -> f64 {
    a.0.hypot(a.1)
}

fn unit(a: P) -> P {
    let n = norm(a);
    if n > 0.0 {
        (a.0 / n, a.1 / n)
    } else {
        (0.0, 1.0)
    }
}

/// A quarter turn from `a` towards the side `cross(a, b) > 0` lies on.
fn left(a: P) -> P {
    (-a.1, a.0)
}

/// The yaw a display faces along `d` with, as the pack's `rotated` takes it.
fn yaw_of(d: P) -> f64 {
    (-d.0).atan2(d.1).to_degrees()
}

/// The angle between two directions, 0 to 180 degrees.
fn turn_deg(a: P, b: P) -> f64 {
    cross(a, b).atan2(dot(a, b)).to_degrees().abs()
}

/// A way's kind, the firmest first where two share a stretch.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Kind {
    Runway,
    Taxi,
    Stand,
}

struct Node {
    at: P,
    id: u64,
}

/// A runway as one line of nodes, from one end to the other.
struct Strip {
    nodes: Vec<usize>,
    /// How far along each node is, in blocks.
    at: Vec<f64>,
    paved: bool,
    /// Smallest way id of its parts, what its aircraft are told apart by.
    id: u64,
}

impl Strip {
    fn len(&self) -> f64 {
        self.at.last().copied().unwrap_or(0.0)
    }
}

/// A stand: the nodes of its line from the taxiway it leaves to where the nose stops.
struct Stand {
    id: u64,
    path: Vec<usize>,
}

/// An airfield's ways as a graph.
struct Field {
    nodes: Vec<Node>,
    /// Each node's neighbours and the kind of way to them.
    next: Vec<Vec<(usize, Kind)>>,
    strips: Vec<Strip>,
    stands: Vec<Stand>,
    /// The strip a node lies on, and its place in it.
    on_strip: Vec<Option<(usize, usize)>>,
}

fn tag<'a>(way: &'a ProcessedWay, key: &str) -> Option<&'a str> {
    way.tags.get(key).map(String::as_str)
}

fn unpaved(way: &ProcessedWay) -> bool {
    matches!(
        tag(way, "surface"),
        Some(
            "grass"
                | "dirt"
                | "ground"
                | "earth"
                | "soil"
                | "sand"
                | "mud"
                | "clay"
                | "unpaved"
                | "gravel"
                | "fine_gravel"
        )
    )
}

impl Field {
    fn read(elements: &[ProcessedElement], k: f64) -> Self {
        let mut nodes: Vec<Node> = Vec::new();
        let mut by_spot: FnvHashMap<(i32, i32), usize> = FnvHashMap::default();
        let mut links: FnvHashMap<(usize, usize), Kind> = FnvHashMap::default();
        let mut rough: FnvHashSet<(usize, usize)> = FnvHashSet::default();
        let mut way_of: FnvHashMap<(usize, usize), u64> = FnvHashMap::default();
        let mut stand_ways: Vec<(u64, Vec<usize>)> = Vec::new();
        let mut stand_nodes: Vec<(u64, P)> = Vec::new();
        for element in elements {
            let way = match element {
                ProcessedElement::Way(w) => w,
                ProcessedElement::Node(n) => {
                    if n.tags.get("aeroway").map(String::as_str) == Some("parking_position") {
                        stand_nodes.push((n.id, (f64::from(n.x) + 0.5, f64::from(n.z) + 0.5)));
                    }
                    continue;
                }
                _ => continue,
            };
            let kind = match tag(way, "aeroway") {
                Some("runway") => Kind::Runway,
                Some("taxiway" | "taxilane") => Kind::Taxi,
                Some("parking_position") => Kind::Stand,
                _ => continue,
            };
            if tag(way, "area") == Some("yes")
                || way.nodes.len() < 2
                || way.nodes[0].id == way.nodes[way.nodes.len() - 1].id
            {
                continue;
            }
            // Nodes on one block are one node, however the ways name them.
            let mut line: Vec<usize> = Vec::with_capacity(way.nodes.len());
            for n in &way.nodes {
                let i = *by_spot.entry((n.x, n.z)).or_insert_with(|| {
                    nodes.push(Node {
                        at: (f64::from(n.x) + 0.5, f64::from(n.z) + 0.5),
                        id: n.id,
                    });
                    nodes.len() - 1
                });
                if line.last() != Some(&i) {
                    line.push(i);
                }
            }
            if line.len() < 2 {
                continue;
            }
            if kind == Kind::Stand {
                stand_ways.push((way.id, line));
                continue;
            }
            for pair in line.windows(2) {
                let key = (pair[0].min(pair[1]), pair[0].max(pair[1]));
                let kept = links.entry(key).or_insert(kind);
                *kept = (*kept).min(kind);
                if kind == Kind::Runway {
                    let id = way_of.entry(key).or_insert(way.id);
                    *id = (*id).min(way.id);
                    if unpaved(way) {
                        rough.insert(key);
                    }
                }
            }
        }
        let mut field = Field {
            nodes,
            next: Vec::new(),
            strips: Vec::new(),
            stands: Vec::new(),
            on_strip: Vec::new(),
        };
        field.next = vec![Vec::new(); field.nodes.len()];
        let mut keys: Vec<(&(usize, usize), &Kind)> = links.iter().collect();
        keys.sort();
        for (&(a, b), &kind) in keys {
            field.next[a].push((b, kind));
            field.next[b].push((a, kind));
        }
        field.strips = field.read_strips(&way_of, &rough, k);
        field.on_strip = vec![None; field.nodes.len()];
        for (s, strip) in field.strips.iter().enumerate() {
            for (i, &n) in strip.nodes.iter().enumerate() {
                field.on_strip[n] = Some((s, i));
            }
        }
        stand_ways.sort();
        for (id, line) in stand_ways {
            field.add_stand(id, line, k);
        }
        stand_nodes.sort_by_key(|a| a.0);
        for (id, at) in stand_nodes {
            // A stand mapped as a way already stands here.
            if field
                .stands
                .iter()
                .any(|s| norm(sub(field.nodes[*s.path.last().unwrap()].at, at)) < 12.0 * k)
            {
                continue;
            }
            field.nodes.push(Node { at, id });
            field.next.push(Vec::new());
            field.on_strip.push(None);
            let n = field.nodes.len() - 1;
            if let Some(foot) = field.reach_taxiway(n, None, k) {
                field.link(foot, n, Kind::Stand);
                field.stands.push(Stand {
                    id,
                    path: vec![foot, n],
                });
            }
        }
        field.add_dead_ends(k);
        field.one_line_a_stand(k);
        field
    }

    /// A stand is often drawn with a lead-in line from either side, both ending where the
    /// nose stops. It is one stand all the same: the longest line to it is kept.
    fn one_line_a_stand(&mut self, k: f64) {
        let length = |me: &Self, stand: &Stand| -> f64 {
            stand.path.windows(2).map(|p| me.span(p[0], p[1])).sum()
        };
        let mut order: Vec<usize> = (0..self.stands.len()).collect();
        order.sort_by(|&a, &b| {
            length(self, &self.stands[b])
                .total_cmp(&length(self, &self.stands[a]))
                .then(self.stands[a].id.cmp(&self.stands[b].id))
        });
        let nose = |me: &Self, s: usize| {
            me.nodes[*me.stands[s].path.last().expect("a stand has nodes")].at
        };
        let mut kept: Vec<usize> = Vec::new();
        for s in order {
            if !kept
                .iter()
                .any(|&o| norm(sub(nose(self, o), nose(self, s))) < SAME_STAND_M * k)
            {
                kept.push(s);
            }
        }
        kept.sort_unstable();
        let mut stands = std::mem::take(&mut self.stands);
        let mut i = 0;
        stands.retain(|_| {
            i += 1;
            kept.binary_search(&(i - 1)).is_ok()
        });
        self.stands = stands;
    }

    /// Where a taxiway ends in nothing, as most do at a small airfield's apron, its end
    /// is a stand: aircraft park at it nose in. Not where the area's edge cut it.
    fn add_dead_ends(&mut self, k: f64) {
        for end in 0..self.nodes.len() {
            let [(from, Kind::Taxi)] = self.next[end][..] else {
                continue;
            };
            if self.on_strip[end].is_some()
                || is_invented_node_id(self.nodes[end].id)
                || self.span(from, end) < DEAD_END_M * k
            {
                continue;
            }
            // A long last stretch is taxiway up to the last of it.
            let len = self.span(from, end);
            let stand = DEAD_END_STAND_M * k;
            let junction = if len > stand + 15.0 * k {
                self.cut_taxiway(from, end, 1.0 - stand / len, end)
            } else {
                from
            };
            for (a, b) in [(junction, end), (end, junction)] {
                if let Some(link) = self.next[a].iter_mut().find(|(n, _)| *n == b) {
                    link.1 = Kind::Stand;
                }
            }
            self.stands.push(Stand {
                id: self.nodes[end].id,
                path: vec![junction, end],
            });
        }
    }

    fn dir(&self, a: usize, b: usize) -> P {
        unit(sub(self.nodes[b].at, self.nodes[a].at))
    }

    fn span(&self, a: usize, b: usize) -> f64 {
        norm(sub(self.nodes[b].at, self.nodes[a].at))
    }

    fn kind(&self, a: usize, b: usize) -> Option<Kind> {
        self.next[a]
            .iter()
            .find(|&&(n, _)| n == b)
            .map(|&(_, kind)| kind)
    }

    fn link(&mut self, a: usize, b: usize, kind: Kind) {
        if a != b && self.kind(a, b).is_none() {
            self.next[a].push((b, kind));
            self.next[b].push((a, kind));
        }
    }

    fn taxis(&self, n: usize) -> bool {
        self.next[n].iter().any(|&(_, kind)| kind != Kind::Stand)
    }

    /// The runways, each a chain of runway stretches that run on straight.
    fn read_strips(
        &self,
        way_of: &FnvHashMap<(usize, usize), u64>,
        rough: &FnvHashSet<(usize, usize)>,
        k: f64,
    ) -> Vec<Strip> {
        let runway = |n: usize| -> Vec<usize> {
            self.next[n]
                .iter()
                .filter(|&&(_, kind)| kind == Kind::Runway)
                .map(|&(m, _)| m)
                .collect()
        };
        let key = |a: usize, b: usize| (a.min(b), a.max(b));
        let mut used: FnvHashSet<(usize, usize)> = FnvHashSet::default();
        let mut strips = Vec::new();
        // From each node a runway ends at, along it as far as it runs on; then from where
        // runways fork or cross, for what is left. One crossed by another is so one runway.
        let mut starts: Vec<usize> = (0..self.nodes.len())
            .filter(|&n| runway(n).len() == 1)
            .collect();
        starts.extend((0..self.nodes.len()).filter(|&n| runway(n).len() > 2));
        starts.extend((0..self.nodes.len()).filter(|&n| runway(n).len() == 2));
        for start in starts {
            for first in runway(start) {
                if used.contains(&key(start, first)) {
                    continue;
                }
                let mut chain = vec![start, first];
                used.insert(key(start, first));
                loop {
                    let (a, b) = (chain[chain.len() - 2], chain[chain.len() - 1]);
                    let ahead = self.dir(a, b);
                    let on = runway(b)
                        .into_iter()
                        .filter(|&c| c != a && !used.contains(&key(b, c)))
                        .map(|c| (turn_deg(ahead, self.dir(b, c)), c))
                        .filter(|&(turn, _)| turn < SAME_RUNWAY_DEG)
                        .min_by(|x, y| x.0.total_cmp(&y.0));
                    let Some((_, c)) = on else { break };
                    used.insert(key(b, c));
                    chain.push(c);
                }
                let mut at = vec![0.0];
                for pair in chain.windows(2) {
                    at.push(at[at.len() - 1] + self.span(pair[0], pair[1]));
                }
                let len = at[at.len() - 1];
                let (from, to) = (
                    self.nodes[chain[0]].at,
                    self.nodes[chain[chain.len() - 1]].at,
                );
                let axis = unit(sub(to, from));
                let bend = chain
                    .iter()
                    .map(|&n| cross(axis, sub(self.nodes[n].at, from)).abs())
                    .fold(0.0, f64::max);
                if len < MIN_RUNWAY_M * k || bend > MAX_BEND * len {
                    continue;
                }
                let pairs: Vec<(usize, usize)> =
                    chain.windows(2).map(|p| key(p[0], p[1])).collect();
                strips.push(Strip {
                    paved: !pairs.iter().any(|p| rough.contains(p)),
                    id: pairs
                        .iter()
                        .filter_map(|p| way_of.get(p))
                        .copied()
                        .min()
                        .unwrap_or(0),
                    nodes: chain,
                    at,
                });
            }
        }
        strips.sort_by_key(|s| s.id);
        strips
    }

    /// Takes a stand's line in, from the taxiway it leaves to its nose end. A line drawn
    /// the other way round is turned, and one that touches no taxiway reaches for the
    /// nearest behind it.
    fn add_stand(&mut self, id: u64, mut line: Vec<usize>, k: f64) {
        let (first, last) = (line[0], line[line.len() - 1]);
        if !self.taxis(first) && self.taxis(last) {
            line.reverse();
        }
        if !self.taxis(line[0]) {
            // An end that lies on a taxiway between two of its nodes leaves it there.
            let on = [0, line.len() - 1].into_iter().find_map(|end| {
                self.nearest_taxiway(self.nodes[line[end]].at, None, 0.0, 1.5)
                    .map(|(a, b, t)| (end, a, b, t))
            });
            if let Some((end, a, b, t)) = on {
                let foot = self.cut_taxiway(a, b, t, line[end]);
                if end != 0 {
                    line.reverse();
                }
                line[0] = foot;
                line.dedup();
                if line.len() < 2 {
                    return;
                }
            } else {
                let back = self.dir(line[1], line[0]);
                let Some(foot) = self.reach_taxiway(line[0], Some(back), k) else {
                    return;
                };
                self.link(foot, line[0], Kind::Stand);
                line.insert(0, foot);
            }
        }
        for pair in line.windows(2) {
            self.link(pair[0], pair[1], Kind::Stand);
        }
        // A line that runs along a taxiway for its first nodes leaves it at the last of them.
        while line.len() > 2 && self.kind(line[0], line[1]) != Some(Kind::Stand) {
            line.remove(0);
        }
        if self.kind(line[0], line[1]) == Some(Kind::Stand) && self.on_strip[line[0]].is_none() {
            self.stands.push(Stand { id, path: line });
        }
    }

    /// The node on a taxiway nearest `n`, within reach and, given `towards`, that way
    /// from it: a node there already, or a new one where the nearest stretch is cut.
    fn reach_taxiway(&mut self, n: usize, towards: Option<P>, k: f64) -> Option<usize> {
        // Nearer than a few metres it stands on the taxiway, not off it.
        let near = if towards.is_some() { 1.5 } else { 10.0 * k };
        let (a, b, t) = self.nearest_taxiway(self.nodes[n].at, towards, near, STAND_REACH_M * k)?;
        Some(self.cut_taxiway(a, b, t, n))
    }

    /// The taxiway stretch nearest `at`, between `near` and `far` from it and, given
    /// `towards`, that way: its two nodes and how far from the first to the second.
    fn nearest_taxiway(
        &self,
        at: P,
        towards: Option<P>,
        near: f64,
        far: f64,
    ) -> Option<(usize, usize, f64)> {
        let mut best: Option<(f64, usize, usize, f64)> = None;
        for a in 0..self.nodes.len() {
            for &(b, kind) in &self.next[a] {
                if kind != Kind::Taxi || a > b {
                    continue;
                }
                let (p, q) = (self.nodes[a].at, self.nodes[b].at);
                let len = norm(sub(q, p));
                let t = (dot(sub(at, p), sub(q, p)) / (len * len)).clamp(0.0, 1.0);
                let foot = add(p, mul(sub(q, p), t));
                let away = sub(foot, at);
                let d = norm(away);
                if d > far || d < near {
                    continue;
                }
                if d > 1e-6 && towards.is_some_and(|w| turn_deg(w, away) > 50.0) {
                    continue;
                }
                if best.is_none_or(|(e, ..)| d < e) {
                    best = Some((d, a, b, t));
                }
            }
        }
        best.map(|(_, a, b, t)| (a, b, t))
    }

    /// The node `t` of the way from `a` to `b` along the taxiway between them: one of
    /// the two where it is nearly there, else a new one that cuts the stretch in two.
    /// `n` is the node it is made for.
    fn cut_taxiway(&mut self, a: usize, b: usize, t: f64, n: usize) -> usize {
        let len = self.span(a, b);
        if t * len < 2.0 {
            return a;
        }
        if (1.0 - t) * len < 2.0 {
            return b;
        }
        let (p, q) = (self.nodes[a].at, self.nodes[b].at);
        self.nodes.push(Node {
            at: add(p, mul(sub(q, p), t)),
            id: hash(self.nodes[a].id, self.nodes[b].id) ^ self.nodes[n].id,
        });
        self.next.push(Vec::new());
        self.on_strip.push(None);
        let foot = self.nodes.len() - 1;
        self.next[a].retain(|&(m, _)| m != b);
        self.next[b].retain(|&(m, _)| m != a);
        self.link(a, foot, Kind::Taxi);
        self.link(foot, b, Kind::Taxi);
        foot
    }

    /// Each node's airfield: the nodes joined by any way share one.
    fn fields(&self) -> Vec<usize> {
        let mut of = vec![usize::MAX; self.nodes.len()];
        let mut count = 0;
        for start in 0..self.nodes.len() {
            if of[start] != usize::MAX {
                continue;
            }
            let mut stack = vec![start];
            of[start] = count;
            while let Some(n) = stack.pop() {
                for &(m, _) in &self.next[n] {
                    if of[m] == usize::MAX {
                        of[m] = count;
                        stack.push(m);
                    }
                }
            }
            count += 1;
        }
        of
    }

    /// The cheapest way on from the stretch `from` (its two nodes, in the order taxied)
    /// to any node `goal` gives a cost to end at. Runway stretches cost `runway` times
    /// their length, those `free` lists their length alone, and turns cost extra, a turn
    /// back most. Stands are not taxied through.
    fn way(
        &self,
        from: (usize, usize),
        free: &FnvHashSet<(usize, usize)>,
        runway: f64,
        k: f64,
        goal: impl Fn(usize, usize) -> Option<f64>,
    ) -> Option<Vec<usize>> {
        #[derive(PartialEq)]
        struct Open(f64, usize, usize);
        impl Eq for Open {}
        impl Ord for Open {
            fn cmp(&self, other: &Self) -> Ordering {
                other
                    .0
                    .total_cmp(&self.0)
                    .then_with(|| (other.1, other.2).cmp(&(self.1, self.2)))
            }
        }
        impl PartialOrd for Open {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }
        let mut best: FnvHashMap<(usize, usize), (f64, (usize, usize))> = FnvHashMap::default();
        let mut open = BinaryHeap::new();
        best.insert(from, (0.0, from));
        open.push(Open(0.0, from.0, from.1));
        let mut found: Option<(f64, (usize, usize))> = None;
        while let Some(Open(cost, a, b)) = open.pop() {
            if best.get(&(a, b)).is_some_and(|&(c, _)| cost > c) {
                continue;
            }
            if found.is_some_and(|(c, _)| cost >= c) {
                break;
            }
            if let Some(extra) = goal(a, b) {
                if found.is_none_or(|(c, _)| cost + extra < c) {
                    found = Some((cost + extra, (a, b)));
                }
            }
            let ahead = self.dir(a, b);
            for &(c, kind) in &self.next[b] {
                if kind == Kind::Stand {
                    continue;
                }
                let turn = turn_deg(ahead, self.dir(b, c));
                let weight = match kind {
                    Kind::Runway if !free.contains(&(b, c)) => runway,
                    _ => 1.0,
                };
                let step = self.span(b, c) * weight + turn_cost(turn) * k;
                let total = cost + step;
                if best.get(&(b, c)).is_none_or(|&(old, _)| total < old) {
                    best.insert((b, c), (total, (a, b)));
                    open.push(Open(total, b, c));
                }
            }
        }
        let (_, mut at) = found?;
        let mut path = vec![at.1];
        while at != from {
            path.push(at.0);
            at = best[&at].1;
        }
        path.push(from.0);
        path.reverse();
        // The first stretch is where the aircraft is; it may come round to it again.
        Some(path)
    }
}

/// What a turn costs a taxi route, in metres of taxiing.
fn turn_cost(deg: f64) -> f64 {
    if deg > 150.0 {
        600.0
    } else if deg > 100.0 {
        90.0
    } else {
        deg * 0.2
    }
}

/// A point of a piece: where, and how the aircraft is there.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pt {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// The way the path runs on from here: its yaw, and how steeply it climbs.
    pub yaw: f64,
    pub climb: f64,
    /// How far the nose is up, in degrees.
    pub nose: f64,
    pub gear: bool,
}

/// A piece of path, its points a block apart.
struct Piece {
    pts: Vec<Pt>,
    len: f64,
}

impl Piece {
    /// Where an aircraft `s` along the piece is, as the pack puts it there.
    fn at(&self, s: f64) -> Pt {
        let i = (s.max(0.0).floor() as usize).min(self.pts.len() - 1);
        let f = s.max(0.0) - i as f64;
        let p = self.pts[i];
        let (yaw, climb) = (p.yaw.to_radians(), p.climb.to_radians());
        Pt {
            x: p.x - yaw.sin() * climb.cos() * f,
            y: p.y + climb.sin() * f,
            z: p.z + yaw.cos() * climb.cos() * f,
            ..p
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Key {
    /// A stretch from one node to the next.
    Edge(usize, usize),
    /// The corner at the middle node, and how far before it the corner starts, in
    /// hundredths of a block.
    Corner(usize, usize, usize, i64),
    /// The turn back at the middle node.
    Pivot(usize, usize, usize, i64),
    /// The circuit back to a runway: the strip, landing towards its last node or its
    /// first, and whether it is the tight one.
    Circuit(usize, bool, bool),
    /// The climb off a runway for an airframe that started its roll so far along it.
    Climb(usize, bool, Airframe, i64),
    /// A turn all the way round off a circuit's downwind leg, keyed as the circuit.
    Orbit(usize, bool, bool),
}

/// A stretch of a piece an aircraft covers, backwards where `to` is less than `from`.
#[derive(Clone, Copy, Debug)]
struct Leg {
    piece: usize,
    from: f64,
    to: f64,
    /// Fastest it is covered, in blocks a step, and how it speeds up and brakes there,
    /// in blocks a step each step.
    cap: f64,
    accel: f64,
    brake: f64,
    /// The strip it keeps to itself on this leg, in the air or rolling fast.
    holds: Option<usize>,
}

impl Leg {
    fn len(&self) -> f64 {
        (self.to - self.from).abs()
    }
}

/// A taxi route as legs, and for each of its nodes how far along the route its turn
/// starts and ends.
struct Taxi {
    legs: Vec<Leg>,
    turns: Vec<(f64, f64)>,
}

impl Taxi {
    fn len(&self) -> f64 {
        self.legs.iter().map(Leg::len).sum()
    }
}

/// The legs up to `d` along them, and those from there on.
fn cut(legs: &[Leg], d: f64) -> (Vec<Leg>, Vec<Leg>) {
    let (mut head, mut tail) = (Vec::new(), Vec::new());
    let mut done = 0.0;
    for &leg in legs {
        let len = leg.len();
        if done + len <= d {
            head.push(leg);
        } else if done >= d {
            tail.push(leg);
        } else {
            let mid = leg.from + (leg.to - leg.from).signum() * (d - done);
            head.push(Leg { to: mid, ..leg });
            tail.push(Leg { from: mid, ..leg });
        }
        done += len;
    }
    (head, tail)
}

/// The same legs covered the other way.
fn reversed(legs: &[Leg], cap: f64) -> Vec<Leg> {
    legs.iter()
        .rev()
        .map(|leg| Leg {
            from: leg.to,
            to: leg.from,
            cap,
            ..*leg
        })
        .collect()
}

/// Where an aircraft is at each step of covering `legs` from a stand to a stand: the leg
/// and how far along its piece.
fn run(legs: &[Leg]) -> Vec<(usize, f64)> {
    let legs: Vec<(usize, &Leg)> = legs
        .iter()
        .enumerate()
        .filter(|(_, leg)| leg.len() > 1e-6)
        .collect();
    let n = legs.len();
    let mut out = Vec::new();
    if n == 0 {
        return out;
    }
    // Fastest each leg may be left at, to keep the next one's cap and stop at the end.
    let mut exit = vec![0.0f64; n];
    for j in (0..n - 1).rev() {
        let after = legs[j + 1].1;
        exit[j] = after
            .cap
            .min((exit[j + 1].powi(2) + 2.0 * after.brake * after.len()).sqrt())
            .min(legs[j].1.cap);
    }
    let (mut j, mut s, mut v) = (0usize, 0.0f64, 0.0f64);
    loop {
        let leg = legs[j].1;
        let limit = leg
            .cap
            .min((exit[j].powi(2) + 2.0 * leg.brake * (leg.len() - s)).sqrt());
        v = (v + leg.accel).min(limit).clamp(CREEP, MAX_STEP);
        s += v;
        while s >= legs[j].1.len() {
            if j + 1 == n {
                out.push((legs[j].0, legs[j].1.to));
                return out;
            }
            s -= legs[j].1.len();
            j += 1;
        }
        let leg = legs[j].1;
        out.push((legs[j].0, leg.from + (leg.to - leg.from).signum() * s));
    }
}

/// The pieces of an airfield's paths, each made once.
struct Paths<'a> {
    field: &'a Field,
    /// Blocks a metre.
    k: f64,
    ground: &'a dyn Fn(i32, i32) -> Option<f64>,
    /// Height taken where the ground is not known.
    datum: f64,
    /// The highest anything may fly: a Bedrock world ends where its blocks do.
    ceiling: f64,
    pieces: Vec<Piece>,
    keys: Vec<Key>,
    by_key: FnvHashMap<Key, usize>,
    /// Tall buildings: where, and how high their tops are.
    tall: &'a [(P, f64)],
}

/// A path drawn finely, to be cut into points a block apart.
#[derive(Default)]
struct Fine {
    at: Vec<(f64, f64, f64)>,
    nose: Vec<f64>,
    gear: Vec<bool>,
}

impl Fine {
    fn push(&mut self, at: (f64, f64, f64), nose: f64, gear: bool) {
        self.at.push(at);
        self.nose.push(nose);
        self.gear.push(gear);
    }

    /// Evens the nose's attitude out over `reach` blocks either way, so it never moves
    /// at once from one attitude to the next. `step` is the spacing the path is drawn at.
    fn ease_nose(&mut self, reach: f64, step: f64) {
        let w = (reach / step).round() as usize;
        let n = self.nose.len();
        let mut sums = vec![0.0; n + 1];
        for i in 0..n {
            sums[i + 1] = sums[i] + self.nose[i];
        }
        for (i, nose) in self.nose.iter_mut().enumerate() {
            let (lo, hi) = (i.saturating_sub(w), (i + w + 1).min(n));
            *nose = (sums[hi] - sums[lo]) / (hi - lo) as f64;
        }
    }

    /// Its points a block apart along it, each facing the next.
    fn piece(&self) -> Piece {
        let mut pts: Vec<Pt> = Vec::new();
        let mut want = 0.0;
        let mut done = 0.0;
        for i in 0..self.at.len().saturating_sub(1) {
            let (a, b) = (self.at[i], self.at[i + 1]);
            let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2) + (b.2 - a.2).powi(2)).sqrt();
            while want <= done + len && len > 0.0 {
                let t = (want - done) / len;
                pts.push(Pt {
                    x: a.0 + (b.0 - a.0) * t,
                    y: a.1 + (b.1 - a.1) * t,
                    z: a.2 + (b.2 - a.2) * t,
                    yaw: 0.0,
                    climb: 0.0,
                    nose: self.nose[i] + (self.nose[i + 1] - self.nose[i]) * t,
                    gear: self.gear[i],
                });
                want += 1.0;
            }
            done += len;
        }
        let last = *self.at.last().expect("a path has points");
        for i in 0..pts.len() {
            let (p, q) = (
                pts[i],
                pts.get(i + 1).copied().unwrap_or(Pt {
                    x: last.0,
                    y: last.1,
                    z: last.2,
                    ..pts[i]
                }),
            );
            let flat = (q.x - p.x).hypot(q.z - p.z);
            if flat > 1e-6 || (q.y - p.y).abs() > 1e-6 {
                pts[i].yaw = yaw_of((q.x - p.x, q.z - p.z));
                pts[i].climb = (q.y - p.y).atan2(flat).to_degrees();
            } else if i > 0 {
                pts[i].yaw = pts[i - 1].yaw;
                pts[i].climb = pts[i - 1].climb;
            }
        }
        Piece { pts, len: done }
    }
}

/// A runway as a line to fly along: its nodes from the threshold on, and on past either
/// end the way its first and last stretches point.
struct Line {
    at: Vec<P>,
    cum: Vec<f64>,
}

impl Line {
    fn len(&self) -> f64 {
        self.cum[self.cum.len() - 1]
    }

    fn at(&self, s: f64) -> P {
        let n = self.at.len();
        if s <= 0.0 {
            return add(self.at[0], mul(unit(sub(self.at[1], self.at[0])), s));
        }
        if s >= self.len() {
            return add(
                self.at[n - 1],
                mul(unit(sub(self.at[n - 1], self.at[n - 2])), s - self.len()),
            );
        }
        let i = self.cum.partition_point(|&c| c <= s).clamp(1, n - 1);
        let t = (s - self.cum[i - 1]) / (self.cum[i] - self.cum[i - 1]);
        add(self.at[i - 1], mul(sub(self.at[i], self.at[i - 1]), t))
    }

    fn first(&self) -> P {
        unit(sub(self.at[1], self.at[0]))
    }

    fn last(&self) -> P {
        let n = self.at.len();
        unit(sub(self.at[n - 1], self.at[n - 2]))
    }
}

/// A runway used one way: its nodes from the threshold on, and where they are along it.
#[derive(Clone)]
struct Runway {
    strip: usize,
    /// Landing towards the strip's first node.
    back: bool,
    nodes: Vec<usize>,
    at: Vec<f64>,
}

impl Runway {
    fn of(field: &Field, strip: usize, back: bool) -> Self {
        let s = &field.strips[strip];
        if back {
            let len = s.len();
            Runway {
                strip,
                back,
                nodes: s.nodes.iter().rev().copied().collect(),
                at: s.at.iter().rev().map(|a| len - a).collect(),
            }
        } else {
            Runway {
                strip,
                back,
                nodes: s.nodes.clone(),
                at: s.at.clone(),
            }
        }
    }

    fn len(&self) -> f64 {
        self.at[self.at.len() - 1]
    }

    fn line(&self, field: &Field) -> Line {
        Line {
            at: self.nodes.iter().map(|&n| field.nodes[n].at).collect(),
            cum: self.at.clone(),
        }
    }

    /// The last node at or before `s` along it, short of its far end.
    fn node_before(&self, s: f64) -> usize {
        self.at
            .partition_point(|&a| a <= s)
            .saturating_sub(1)
            .min(self.nodes.len() - 2)
    }

    /// Its stretches in the order rolled, for a taxi route to roll along at no extra cost.
    fn stretches(&self) -> FnvHashSet<(usize, usize)> {
        self.nodes.windows(2).map(|p| (p[0], p[1])).collect()
    }
}

/// Height over the runway `d` along the path before touchdown, in blocks.
fn approach_height(d: f64, top: f64, k: f64) -> f64 {
    let glide = GLIDE_DEG.to_radians().tan();
    let flare = FLARE_M * k;
    if d <= 0.0 {
        return 0.0;
    }
    if d < flare {
        return glide * d * d / (2.0 * flare);
    }
    let on_glide = glide * (d - flare / 2.0);
    let meet = INTERCEPT_M * k;
    if on_glide <= meet {
        return on_glide.min(top);
    }
    let d_meet = meet / glide + flare / 2.0;
    (meet + DESCENT_DEG.to_radians().tan() * (d - d_meet)).min(top)
}

impl<'a> Paths<'a> {
    /// Height of the wheels at a spot on the ground: the ground about it, evened out.
    fn surface(&self, x: f64, z: f64) -> f64 {
        let (mut sum, mut n) = (0.0, 0.0);
        for dx in -2..=2 {
            for dz in -2..=2 {
                if let Some(y) = (self.ground)(
                    (x + f64::from(dx) * 3.0).floor() as i32,
                    (z + f64::from(dz) * 3.0).floor() as i32,
                ) {
                    sum += y;
                    n += 1.0;
                }
            }
        }
        if n > 0.0 {
            sum / n + 1.0
        } else {
            self.datum + 1.0
        }
    }

    fn piece(&mut self, key: Key, make: impl FnOnce(&Self) -> Piece) -> usize {
        if let Some(&i) = self.by_key.get(&key) {
            return i;
        }
        let piece = make(self);
        self.pieces.push(piece);
        self.keys.push(key);
        self.by_key.insert(key, self.pieces.len() - 1);
        self.pieces.len() - 1
    }

    fn edge(&mut self, a: usize, b: usize) -> usize {
        self.piece(Key::Edge(a, b), |me| {
            let (from, d) = (me.field.nodes[a].at, me.field.dir(a, b));
            let len = me.field.span(a, b);
            let yaw = yaw_of(d);
            let pts = (0..=len.floor() as usize)
                .map(|i| {
                    let p = add(from, mul(d, i as f64));
                    Pt {
                        x: p.0,
                        y: me.surface(p.0, p.1),
                        z: p.1,
                        yaw,
                        climb: 0.0,
                        nose: 0.0,
                        gear: true,
                    }
                })
                .collect();
            Piece { pts, len }
        })
    }

    /// A path on the ground drawn finely, as a piece.
    fn ground_piece(&self, line: &[P]) -> Piece {
        let mut fine = Fine::default();
        for &p in line {
            fine.push((p.0, self.surface(p.0, p.1), p.1), 0.0, true);
        }
        let mut piece = fine.piece();
        // The ground's small steps are not slopes to pitch along.
        for p in &mut piece.pts {
            p.climb = 0.0;
        }
        let flat: f64 = line.windows(2).map(|w| norm(sub(w[1], w[0]))).sum();
        piece.len = flat;
        piece.pts.truncate(flat.floor() as usize + 1);
        piece
    }

    /// The corner from `a` through `b` to `c`, starting and ending `trim` from `b`.
    fn corner(&mut self, a: usize, b: usize, c: usize, trim: f64) -> usize {
        self.piece(Key::Corner(a, b, c, (trim * 100.0).round() as i64), |me| {
            let (d1, d2) = (me.field.dir(a, b), me.field.dir(b, c));
            let theta = turn_deg(d1, d2).to_radians();
            let radius = trim / (theta / 2.0).tan();
            let side = mul(left(d1), cross(d1, d2).signum());
            let start = sub(me.field.nodes[b].at, mul(d1, trim));
            let centre = add(start, mul(side, radius));
            let n = (radius * theta / 0.2).ceil().max(2.0) as usize;
            let line: Vec<P> = (0..=n)
                .map(|i| {
                    let t = theta * i as f64 / n as f64;
                    add(
                        sub(centre, mul(side, radius * t.cos())),
                        mul(d1, radius * t.sin()),
                    )
                })
                .collect();
            me.ground_piece(&line)
        })
    }

    /// The turn back at `b`, coming from `a` and leaving for `c`: from `lead` before `b`
    /// round a tight arc and over onto the line out, met `out` along it.
    fn pivot(&mut self, a: usize, b: usize, c: usize, lead: f64, out: f64) -> usize {
        let k = self.k;
        self.piece(Key::Pivot(a, b, c, (lead * 100.0).round() as i64), |me| {
            let (d1, d2) = (me.field.dir(a, b), me.field.dir(b, c));
            let theta = turn_deg(d1, d2).to_radians();
            let turn = if cross(d1, d2).abs() < 1e-9 {
                1.0
            } else {
                cross(d1, d2).signum()
            };
            let side = mul(left(d1), turn);
            let at_b = me.field.nodes[b].at;
            let radius = PIVOT_M * k;
            let start = sub(at_b, mul(d1, lead));
            let centre = add(start, mul(side, radius));
            let n = (radius * theta / 0.2).ceil().max(2.0) as usize;
            let mut line: Vec<P> = (0..=n)
                .map(|i| {
                    let t = theta * i as f64 / n as f64;
                    add(
                        sub(centre, mul(side, radius * t.cos())),
                        mul(d1, radius * t.sin()),
                    )
                })
                .collect();
            // Round, it is beside the line out: ease over onto it.
            let end = line[line.len() - 1];
            let along = dot(sub(end, at_b), d2);
            let aside = dot(sub(end, at_b), left(d2));
            let merge = (out - along).max(1.0);
            let m = (merge / 0.2).ceil() as usize;
            for i in 1..=m {
                let t = i as f64 / m as f64;
                let ease = t * t * (3.0 - 2.0 * t);
                line.push(add(
                    add(at_b, mul(d2, along + merge * t)),
                    mul(left(d2), aside * (1.0 - ease)),
                ));
            }
            me.ground_piece(&line)
        })
    }

    /// The taxi route through `nodes`, from `start` along its first stretch to the last
    /// node: straight stretches joined by corners, or by a turn back where it doubles.
    fn taxi(&mut self, nodes: &[usize], start: f64) -> Taxi {
        let (field, k) = (self.field, self.k);
        let n = nodes.len();
        let speed = |ms: f64| (ms * k / STEPS_PER_S).min(MAX_STEP);
        let len: Vec<f64> = nodes.windows(2).map(|p| field.span(p[0], p[1])).collect();
        // What joins stretch i - 1 to stretch i: the piece, how much of each it takes
        // and how fast it is taken.
        let mut joins: Vec<Option<(usize, f64, f64, f64)>> = vec![None; n];
        for i in 1..n - 1 {
            let (a, b, c) = (nodes[i - 1], nodes[i], nodes[i + 1]);
            let turn = turn_deg(field.dir(a, b), field.dir(b, c));
            let room_in = if i == 1 {
                (len[0] - start).min(len[0] / 2.0)
            } else {
                len[i - 1] / 2.0
            };
            let room_out = len[i] / 2.0;
            if turn < 2.0 {
                continue;
            }
            if turn <= 150.0 {
                let half = (turn.to_radians() / 2.0).tan();
                let radius = (CORNER_M * k).min(room_in.min(room_out) / half);
                let trim = radius * half;
                if trim < 0.3 {
                    continue;
                }
                let pull = TURN_PULL * k / (STEPS_PER_S * STEPS_PER_S);
                let piece = self.corner(a, b, c, trim);
                joins[i] = Some((
                    piece,
                    trim,
                    trim,
                    (pull * radius).sqrt().min(speed(TAXI_MS)),
                ));
            } else {
                let lead =
                    (PIVOT_M * k + 4.0 * k).min(len[i - 1] - if i == 1 { start } else { 0.0 });
                let out = (lead + PIVOT_MERGE_M * k).min(len[i] - 1.0);
                if lead < 1.0 || out < lead + 2.0 {
                    continue;
                }
                let piece = self.pivot(a, b, c, lead, out);
                joins[i] = Some((piece, lead, out, speed(PIVOT_MS)));
            }
        }
        let per_step2 = k / (STEPS_PER_S * STEPS_PER_S);
        let (accel, brake) = (TAXI_ACCEL * per_step2, TAXI_BRAKE * per_step2);
        let mut legs = Vec::new();
        let mut turns = vec![(0.0, 0.0); n];
        let mut done = 0.0;
        let mut enter = start;
        for i in 0..n - 1 {
            let (a, b) = (nodes[i], nodes[i + 1]);
            let leave = len[i] - joins[i + 1].map_or(0.0, |j| j.1);
            // Along a runway briskly, to be off it.
            let cap = match field.kind(a, b) {
                Some(Kind::Stand) => speed(STAND_MS),
                Some(Kind::Runway) => speed(TAXI_MS * 1.6),
                _ => speed(TAXI_MS),
            };
            if leave > enter {
                legs.push(Leg {
                    piece: self.edge(a, b),
                    from: enter,
                    to: leave,
                    cap,
                    accel,
                    brake,
                    holds: None,
                });
                done += leave - enter;
            }
            turns[i + 1].0 = done;
            enter = 0.0;
            if let Some((piece, _, out, cap)) = joins[i + 1] {
                let len = self.pieces[piece].len;
                legs.push(Leg {
                    piece,
                    from: 0.0,
                    to: len,
                    cap,
                    accel,
                    brake,
                    holds: None,
                });
                done += len;
                enter = out;
            }
            turns[i + 1].1 = done;
        }
        Taxi { legs, turns }
    }

    /// How high over a runway whose wheels touch at `touch_y` a circuit is flown: its own
    /// height, or as high as the world lets anything be.
    fn top(&self, shape: Circuit, touch_y: f64) -> f64 {
        (shape.height * self.k).min(self.ceiling - touch_y)
    }

    /// Whether a point in the air is clear of the ground and of tall buildings by `gap`.
    fn clear(&self, at: (f64, f64, f64), gap: f64) -> bool {
        if (self.ground)(at.0.floor() as i32, at.2.floor() as i32)
            .is_some_and(|g| at.1 < g + 1.0 + gap)
        {
            return false;
        }
        !self
            .tall
            .iter()
            .any(|&(p, top)| at.1 < top + gap && norm(sub(p, (at.0, at.2))) < 40.0 * self.k)
    }

    /// The circuit back to a runway, drawn finely: from level over the point `beyond`
    /// past its far end, round to the side `side` of it, back past the threshold, round
    /// again and down to the wheels on the runway `touch` along it, and on until the nose
    /// is down. None where it would not clear the ground.
    fn circuit(
        &self,
        line: &Line,
        shape: Circuit,
        beyond: f64,
        touch: f64,
        side: f64,
    ) -> Option<Fine> {
        let k = self.k;
        let radius = shape.radius * k;
        let before = shape.before * k;
        // The path on the flat, then the heights along it back from the touchdown.
        let mut flat: Vec<P> = Vec::new();
        let step = 0.25;
        let arc = |flat: &mut Vec<P>, from: P, ahead: P| {
            let to_side = mul(left(ahead), side);
            let centre = add(from, mul(to_side, radius));
            let n = (std::f64::consts::PI * radius / step).ceil() as usize;
            for i in 1..=n {
                let t = std::f64::consts::PI * i as f64 / n as f64;
                flat.push(add(
                    sub(centre, mul(to_side, radius * t.cos())),
                    mul(ahead, radius * t.sin()),
                ));
            }
        };
        let far = line.at(line.len() + beyond);
        flat.push(far);
        arc(&mut flat, far, line.last());
        let downwind_from = flat[flat.len() - 1];
        let near = line.at(-before);
        // Abeam the point the final starts at, on the same side.
        let downwind_to = add(near, mul(left(line.first()), side * 2.0 * radius));
        let run = norm(sub(downwind_to, downwind_from));
        let n = (run / step).ceil().max(1.0) as usize;
        for i in 1..=n {
            flat.push(add(
                downwind_from,
                mul(sub(downwind_to, downwind_from), i as f64 / n as f64),
            ));
        }
        // The second turn comes back onto the runway's line: the same hand, seen along
        // the way flown, which is now the other way.
        arc(&mut flat, downwind_to, mul(line.first(), -1.0));
        let end = touch + NOSE_DOWN_M * k;
        let n = ((before + end) / step).ceil() as usize;
        let final_from = flat.len();
        for i in 1..=n {
            flat.push(line.at(-before + (before + end) * i as f64 / n as f64));
        }
        // Distance to go to the touchdown, along the path.
        let touch_i =
            final_from + ((before + touch) / (before + end) * n as f64).round() as usize - 1;
        let mut to_go = vec![0.0; flat.len()];
        for i in (0..touch_i).rev() {
            to_go[i] = to_go[i + 1] + norm(sub(flat[i + 1], flat[i]));
        }
        let touch_at = flat[touch_i];
        let touch_y = self.surface(touch_at.0, touch_at.1);
        let top = self.top(shape, touch_y);
        // Over the runway the height is over the runway as it lies there, which may slope;
        // before it, over the threshold's.
        let threshold = line.at(0.0);
        let threshold_y = self.surface(threshold.0, threshold.1);
        let over_runway = final_from + (before / (before + end) * n as f64).round() as usize;
        let mut fine = Fine::default();
        let glide_until = INTERCEPT_M * k / GLIDE_DEG.to_radians().tan() + FLARE_M * k / 2.0;
        let mut past = 0.0;
        for (i, &p) in flat.iter().enumerate() {
            if i > touch_i {
                past += norm(sub(flat[i], flat[i - 1]));
                let nose = TOUCHDOWN_NOSE * (1.0 - past / (NOSE_DOWN_M * k)).max(0.0);
                fine.push((p.0, self.surface(p.0, p.1), p.1), nose, true);
                continue;
            }
            let d = to_go[i];
            let under = if i >= over_runway {
                self.surface(p.0, p.1)
            } else {
                threshold_y
            };
            let y = (under + approach_height(d, f64::MAX, k)).min(touch_y + top);
            let h = y - under;
            let nose = if y >= touch_y + top - 1e-6 {
                LEVEL_NOSE
            } else if d > glide_until {
                -1.0
            } else if d > FLARE_M * k * 2.0 {
                LEVEL_NOSE
            } else {
                LEVEL_NOSE + (TOUCHDOWN_NOSE - LEVEL_NOSE) * (1.0 - d / (FLARE_M * k * 2.0))
            };
            // Ground that rises towards the runway may come up under the glide, to a point.
            // The runway itself is clear.
            let gap = (CLEAR_TERRAIN_M * k)
                .min(h * 0.6)
                .max((CLEAR_RUNWAY_M * k).min(h));
            if i % 8 == 0 && i < over_runway && !self.clear((p.0, y, p.1), gap) {
                return None;
            }
            fine.push((p.0, y, p.1), nose, d <= GEAR_DOWN_M * k);
        }
        fine.ease_nose(40.0 * k, step);
        Some(fine)
    }

    /// The climb off a runway, drawn finely: from where the nose starts to come up,
    /// `rotate` along it, off the ground at `lift`, up at `frame`'s angle to the height
    /// `top` over the block `touch_y`, and level to the point `beyond` past the far end.
    /// None where it would not get there, or not clear the ground.
    #[allow(clippy::too_many_arguments)]
    fn climb(
        &self,
        line: &Line,
        frame: Airframe,
        rotate: f64,
        lift: f64,
        beyond: f64,
        top: f64,
        touch_y: f64,
    ) -> Option<Fine> {
        let k = self.k;
        let (angle, nose_up, _) = frame.angles();
        let slope = angle.to_radians().tan();
        let lift_at = line.at(lift);
        let lift_y = self.surface(lift_at.0, lift_at.1);
        let rise = touch_y + top - lift_y;
        // The path steepens over `ease_in` and levels over `ease_out`, both along the ground.
        let ease_in = 110.0 * k;
        let ease_out = (220.0 * k).min(rise / slope);
        let run_up = rise / slope + ease_in / 2.0 + ease_out / 2.0;
        let end = line.len() + beyond;
        if rise <= 0.0 || lift + run_up > end {
            return None;
        }
        let height = |x: f64| -> f64 {
            if x <= 0.0 {
                0.0
            } else if x < ease_in {
                slope * x * x / (2.0 * ease_in)
            } else if x < run_up - ease_out {
                slope * (x - ease_in / 2.0)
            } else if x < run_up {
                rise - slope * (run_up - x).powi(2) / (2.0 * ease_out)
            } else {
                rise
            }
        };
        let step = 0.25;
        let n = ((end - rotate) / step).ceil() as usize;
        let mut fine = Fine::default();
        for i in 0..=n {
            let s = rotate + (end - rotate) * i as f64 / n as f64;
            let p = line.at(s);
            if s <= lift {
                let nose =
                    nose_up * 0.55 * ((s - rotate) / (lift - rotate).max(1.0)).clamp(0.0, 1.0);
                fine.push((p.0, self.surface(p.0, p.1), p.1), nose, true);
                continue;
            }
            let x = s - lift;
            let h = height(x);
            let nose = if x < ease_in {
                nose_up * (0.55 + 0.45 * x / ease_in)
            } else if x < run_up - ease_out {
                nose_up
            } else if x < run_up {
                LEVEL_NOSE + (nose_up - LEVEL_NOSE) * (run_up - x) / ease_out
            } else {
                LEVEL_NOSE
            };
            let y = lift_y + h;
            // Past the runway's end it keeps over what lies ahead, by more as it gets higher.
            let gap = (CLEAR_TERRAIN_M * k).min(h * 0.5);
            if i % 8 == 0 && s > line.len() && !self.clear((p.0, y, p.1), gap) {
                return None;
            }
            fine.push((p.0, y, p.1), nose, h < GEAR_UP_M * k);
        }
        // Its nose is level on the runway where the piece starts, whatever comes after.
        let first = fine.nose[0];
        fine.ease_nose(25.0 * k, (end - rotate) / n as f64);
        let lead = ((25.0 * k) / ((end - rotate) / n as f64)) as usize;
        for i in 0..lead.min(fine.nose.len()) {
            let t = i as f64 / lead as f64;
            fine.nose[i] = first + (fine.nose[i] - first) * t;
        }
        Some(fine)
    }
}

/// How far the climb at `frame`'s angle runs along the ground to rise `rise`.
fn climb_run(frame: Airframe, rise: f64, k: f64) -> f64 {
    let slope = frame.angles().0.to_radians().tan();
    rise / slope + 110.0 * k / 2.0 + (220.0 * k).min(rise / slope) / 2.0
}

/// A runway as an airfield uses it: which way, which side its circuits lie on, where
/// aircraft touch down, and which airframes it takes.
struct Use {
    runway: Runway,
    line: Line,
    side: f64,
    touch: f64,
    touch_y: f64,
    /// How far past the far end the wide circuit and the tight one start.
    beyond: [f64; 2],
    frames: Vec<Airframe>,
}

/// An aircraft's speeds on a world of `k` blocks a metre: lifting off, landing and in
/// the circuit, in blocks a step, and speeding up and braking, in blocks a step each step.
struct Pace {
    lift: f64,
    land: f64,
    air: f64,
    accel: f64,
    brake: f64,
}

impl Pace {
    fn of(frame: Airframe, k: f64) -> Self {
        let (lift, land) = frame.speeds();
        let (accel, brake) = frame.accel();
        let step = |ms: f64| (ms * k / STEPS_PER_S).min(MAX_STEP);
        let per_step2 = k / (STEPS_PER_S * STEPS_PER_S);
        Pace {
            lift: step(lift),
            land: step(land),
            air: step(land * 1.3),
            accel: accel * per_step2,
            brake: brake * per_step2,
        }
    }

    /// From where it lines up `dep` along the runway: where its nose starts to come up
    /// and where it lifts off.
    fn take_off(&self, dep: f64) -> (f64, f64) {
        let roll = self.lift * self.lift / (2.0 * self.accel);
        let turn = self.lift * 2.5 * STEPS_PER_S;
        let rotate = dep + roll - 0.4 * turn;
        (rotate, rotate + turn)
    }
}

/// An aircraft's routes on the ground, before the circuits are laid out.
struct Draft {
    stand: usize,
    frame: Airframe,
    used: usize,
    /// Pushed back from the stand; to the wait before the runway; onto it; along it to
    /// where the nose comes up.
    push: Vec<Leg>,
    out: Vec<Leg>,
    line_up: Vec<Leg>,
    roll: Vec<Leg>,
    dep: f64,
    /// From where the circuit sets it down to its stand.
    arrive: Vec<Leg>,
    park: (usize, f64),
    /// How far it taxis out and in, to choose a runway by.
    taxied: f64,
}

/// An aircraft's place at a step: the piece, how far along it, and the strip it keeps
/// to itself there.
#[derive(Clone, Copy)]
struct Step {
    piece: usize,
    at: f64,
    holds: Option<usize>,
}

/// Where an aircraft is and which way it faces, for keeping two apart.
#[derive(Clone, Copy)]
struct Pose {
    at: P,
    y: f64,
    ahead: P,
    frame: Airframe,
}

fn point_to_segment(p: P, a: P, b: P) -> f64 {
    let ab = sub(b, a);
    let len2 = dot(ab, ab);
    let t = if len2 > 0.0 {
        (dot(sub(p, a), ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    norm(sub(p, add(a, mul(ab, t))))
}

/// Least distance between two segments on the flat.
fn segment_gap(a0: P, a1: P, b0: P, b1: P) -> f64 {
    let (d1, d2) = (
        cross(sub(a1, a0), sub(b0, a0)),
        cross(sub(a1, a0), sub(b1, a0)),
    );
    let (d3, d4) = (
        cross(sub(b1, b0), sub(a0, b0)),
        cross(sub(b1, b0), sub(a1, b0)),
    );
    if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
        return 0.0;
    }
    point_to_segment(a0, b0, b1)
        .min(point_to_segment(a1, b0, b1))
        .min(point_to_segment(b0, a0, a1))
        .min(point_to_segment(b1, a0, a1))
}

impl Pose {
    /// Its hull and its wing as lines with a half-width each, at `size` blocks a metre.
    fn parts(&self, size: f64) -> [(P, P, f64); 2] {
        let (nose, tail) = self.frame.reach_m();
        let (_, span) = self.frame.size_m();
        let wing = add(self.at, mul(self.ahead, 0.1 * nose * size));
        let tip = mul(left(self.ahead), span / 2.0 * size);
        [
            (
                add(self.at, mul(self.ahead, nose * size)),
                sub(self.at, mul(self.ahead, tail * size)),
                self.frame.cabin_m() / 2.0 * size,
            ),
            (add(wing, tip), sub(wing, tip), 0.5 * size),
        ]
    }

    fn reach(&self, size: f64) -> f64 {
        let (nose, tail) = self.frame.reach_m();
        let (_, span) = self.frame.size_m();
        nose.max(tail).max(span / 2.0) * size
    }

    /// Whether two aircraft are nearer than they may come, hull or wing.
    fn too_close(&self, other: &Pose, size: f64, k: f64) -> bool {
        if (self.y - other.y).abs() > CLEAR_HEIGHT_M * k {
            return false;
        }
        let gap = CLEAR_M * k;
        if norm(sub(self.at, other.at)) > self.reach(size) + other.reach(size) + gap {
            return false;
        }
        self.parts(size).iter().any(|&(a0, a1, ra)| {
            other
                .parts(size)
                .iter()
                .any(|&(b0, b1, rb)| segment_gap(a0, a1, b0, b1) < ra + rb + gap)
        })
    }
}

/// An aircraft's round, from its pushback until it stands again, not yet given a time.
struct Round {
    stand: usize,
    frame: Airframe,
    /// The strip it takes off from and lands on.
    runway: usize,
    /// Its steps up to the wait before the runway, and from there on for each number of
    /// times it may turn round off the downwind leg.
    before: Vec<Step>,
    afters: Vec<Vec<Step>>,
    park: Step,
}

/// A round with its time: the second of the airfield's round its pushback starts at, how
/// long it waits before the runway, and how many times it turns round before landing.
struct Flight {
    round: Round,
    start: usize,
    wait: usize,
    orbits: usize,
}

impl Flight {
    fn after(&self) -> &[Step] {
        &self.round.afters[self.orbits]
    }
}

/// Whether two stretches of time on a clock of `round` seconds overlap.
fn overlap(a: (i64, i64), b: (i64, i64), round: i64) -> bool {
    let shift = (b.0 - a.0).rem_euclid(round) - (b.0 - a.0);
    let b = (b.0 + shift, b.1 + shift);
    [-round, 0, round]
        .iter()
        .any(|&m| a.0 <= b.1 + m && b.0 + m <= a.1)
}

/// What has been given a time so far at an airfield.
struct Sky {
    /// Seconds in the airfield's round.
    round: usize,
    /// Who is where at each second.
    poses: Vec<Vec<Pose>>,
    /// When each strip is in use, in seconds.
    busy: Vec<Vec<(i64, i64)>>,
}

/// The airfields' aircraft: for each airfield one route of all its paths, and what flies
/// and stands on it.
pub struct AirPlan {
    fields: Vec<(Route, Vec<Service>)>,
    /// The ground each of those airfields covers: its lowest and highest x and z.
    extents: Vec<[f64; 4]>,
    flying: usize,
    parked: usize,
}

impl AirPlan {
    /// Plans the aircraft of every airfield in `elements`. `ground` is the height of the
    /// ground's top block at a column, where it is known; `points` is how many route
    /// points the pack has room for, and `ceiling` the highest anything may fly.
    pub fn build(
        elements: &[ProcessedElement],
        scale: f64,
        points: usize,
        ceiling: f64,
        ground: impl Fn(i32, i32) -> Option<f64>,
    ) -> Self {
        let k = scale;
        let size = scale.clamp(0.4, 3.0);
        let field = Field::read(elements, k);
        let mut plan = AirPlan {
            fields: Vec::new(),
            extents: Vec::new(),
            flying: 0,
            parked: 0,
        };
        if field.strips.is_empty() {
            return plan;
        }
        let tall = tall_buildings(elements, k, &ground);
        let of = field.fields();
        // The airfield with the longest runway first, while there is room for more.
        let mut order: Vec<usize> = of.clone();
        order.sort_unstable();
        order.dedup();
        let longest = |f: usize| {
            field
                .strips
                .iter()
                .filter(|s| of[s.nodes[0]] == f)
                .map(Strip::len)
                .fold(0.0, f64::max)
        };
        order.retain(|&f| longest(f) > 0.0);
        order.sort_by(|&a, &b| longest(b).total_cmp(&longest(a)).then(a.cmp(&b)));
        let room_for = MAX_POINTS.min(points);
        let mut points = 0usize;
        for f in order {
            let strips: Vec<usize> = (0..field.strips.len())
                .filter(|&s| of[field.strips[s].nodes[0]] == f)
                .collect();
            let stands: Vec<usize> = (0..field.stands.len())
                .filter(|&s| of[field.stands[s].path[0]] == f)
                .collect();
            let room = Room {
                flying: MAX_FLYING - plan.flying,
                parked: MAX_PARKED - plan.parked,
                points: room_for - points,
                ceiling,
            };
            let Some((route, services, flying)) =
                plan_field(&field, &strips, &stands, k, size, &ground, &tall, room)
            else {
                continue;
            };
            points += route.points.len();
            plan.flying += flying;
            plan.parked += services.len() - flying;
            plan.fields.push((route, services));
            // All its ways, and a little round them.
            let pad = 30.0 * k;
            let mut extent = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
            for (n, node) in field.nodes.iter().enumerate() {
                if of[n] == f {
                    extent = [
                        extent[0].min(node.at.0 - pad),
                        extent[1].min(node.at.1 - pad),
                        extent[2].max(node.at.0 + pad),
                        extent[3].max(node.at.1 + pad),
                    ];
                }
            }
            plan.extents.push(extent);
        }
        plan
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// How many aircraft fly, and how many only stand.
    pub fn counts(&self) -> (usize, usize) {
        (self.flying, self.parked)
    }

    /// The ground of the airfields that have aircraft, each its lowest and highest x and
    /// z: where the block-built aeroplanes would stand in their way.
    pub fn extents(&self) -> &[[f64; 4]] {
        &self.extents
    }

    /// Each airfield's route and the aircraft on it.
    pub(crate) fn into_fields(self) -> Vec<(Route, Vec<Service>)> {
        self.fields
    }
}

/// What an airfield may still take of the world's aircraft and route points, and how
/// high it may fly.
struct Room {
    flying: usize,
    parked: usize,
    points: usize,
    ceiling: f64,
}

/// Buildings tall enough to stand in a circuit's way: where, and the height of their tops.
fn tall_buildings(
    elements: &[ProcessedElement],
    k: f64,
    ground: &impl Fn(i32, i32) -> Option<f64>,
) -> Vec<(P, f64)> {
    let mut out = Vec::new();
    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };
        if !way.tags.contains_key("building") && !way.tags.contains_key("building:part") {
            continue;
        }
        let metres = way
            .tags
            .get("height")
            .and_then(|h| h.trim().trim_end_matches('m').trim().parse::<f64>().ok())
            .or_else(|| {
                way.tags
                    .get("building:levels")
                    .and_then(|l| l.trim().parse::<f64>().ok())
                    .map(|l| l * 3.2)
            });
        let Some(metres) = metres.filter(|&m| m >= TALL_M) else {
            continue;
        };
        let n = way.nodes.len().max(1) as f64;
        let at = (
            way.nodes.iter().map(|p| f64::from(p.x)).sum::<f64>() / n,
            way.nodes.iter().map(|p| f64::from(p.z)).sum::<f64>() / n,
        );
        let base = ground(at.0.floor() as i32, at.1.floor() as i32).unwrap_or(0.0);
        out.push((at, base + metres * k));
    }
    out
}

/// The runways of an airfield as it uses them, each the way and with its circuits on the
/// side that clear the ground.
fn runway_uses(paths: &Paths, strips: &[usize], centre: Option<P>) -> Vec<Use> {
    let (field, k) = (paths.field, paths.k);
    let mut uses = Vec::new();
    for &s in strips {
        let strip = &field.strips[s];
        let metres = strip.len() / k;
        let frames: Vec<Airframe> = ALL
            .into_iter()
            .filter(|f| {
                let (needs, paved) = f.runway_m();
                metres >= needs && (strip.paved || !paved)
            })
            .collect();
        if frames.is_empty() {
            continue;
        }
        let touch = (TOUCHDOWN_M * k).min(TOUCHDOWN_SHARE * strip.len());
        // Into the west first, as the wind mostly has it, and the circuits on the side
        // away from the stands.
        let ahead = field.dir(strip.nodes[0], strip.nodes[strip.nodes.len() - 1]);
        let west_back = ahead.0 > 0.0 || (ahead.0 == 0.0 && ahead.1 > 0.0);
        let mut found = None;
        'tries: for back in [west_back, !west_back] {
            let runway = Runway::of(field, s, back);
            let line = runway.line(field);
            let away = centre.map_or(1.0, |c| {
                let to = sub(c, line.at(line.len() / 2.0));
                if dot(left(line.first()), to) > 0.0 {
                    -1.0
                } else {
                    1.0
                }
            });
            for side in [away, -away] {
                let at = line.at(touch);
                let touch_y = paths.surface(at.0, at.1);
                let wide = frames.iter().any(|f| !f.slow());
                let tight = frames.iter().any(|f| f.slow());
                // Where the world ends too low over the runway there is no circuit to fly.
                let room = |shape: Circuit| paths.top(shape, touch_y) >= MIN_CIRCUIT_M * k;
                if (wide && !room(WIDE_CIRCUIT)) || (tight && !room(TIGHT_CIRCUIT)) {
                    continue;
                }
                let clear = |shape: Circuit| {
                    paths
                        .circuit(&line, shape, shape.beyond * k, touch, side)
                        .is_some()
                };
                if (wide && !clear(WIDE_CIRCUIT)) || (tight && !clear(TIGHT_CIRCUIT)) {
                    continue;
                }
                // The shallowest climb, from halfway along, has to clear what lies ahead.
                let slowest = frames
                    .iter()
                    .copied()
                    .min_by(|a, b| a.angles().0.total_cmp(&b.angles().0))
                    .expect("there are frames");
                let shape = if slowest.slow() {
                    TIGHT_CIRCUIT
                } else {
                    WIDE_CIRCUIT
                };
                let lift = line.len() / 2.0;
                let at_lift = line.at(lift);
                let rise =
                    touch_y + paths.top(shape, touch_y) - paths.surface(at_lift.0, at_lift.1);
                let beyond = (shape.beyond * k)
                    .max(lift + climb_run(slowest, rise, k) + 40.0 * k - line.len());
                if paths
                    .climb(
                        &line,
                        slowest,
                        lift - 10.0,
                        lift,
                        beyond,
                        paths.top(shape, touch_y),
                        touch_y,
                    )
                    .is_none()
                {
                    continue;
                }
                found = Some((runway.clone(), side, touch_y));
                break 'tries;
            }
        }
        let Some((runway, side, touch_y)) = found else {
            continue;
        };
        uses.push(Use {
            line: runway.line(field),
            runway,
            side,
            touch,
            touch_y,
            beyond: [WIDE_CIRCUIT.beyond * k, TIGHT_CIRCUIT.beyond * k],
            frames,
        });
    }
    uses
}

/// The out-trim of the leg that starts `d` along `legs`: how far along its piece it begins.
fn starts_at(legs: &[Leg], d: f64) -> f64 {
    let mut done = 0.0;
    for leg in legs {
        if done >= d - 1e-6 {
            return leg.from;
        }
        done += leg.len();
    }
    0.0
}

impl<'a> Paths<'a> {
    fn speed(&self, ms: f64) -> f64 {
        (ms * self.k / STEPS_PER_S).min(MAX_STEP)
    }

    /// An aircraft's routes on the ground for a stand and a runway: its pushback, out to
    /// the runway and along it to where its nose comes up, and from where the circuit sets
    /// it down back to the stand. None where there is no way, or no room.
    fn draft(
        &mut self,
        s: usize,
        frame: Airframe,
        used: usize,
        u: &Use,
        size: f64,
    ) -> Option<Draft> {
        let (field, k) = (self.field, self.k);
        let stand = &field.stands[s];
        let (junction, into) = (stand.path[0], stand.path[1]);
        let pace = Pace::of(frame, k);
        let (nose, _) = frame.reach_m();
        let runway = &u.runway;
        let strip = runway.strip;
        let free = runway.stretches();
        let index: FnvHashMap<usize, usize> = runway
            .nodes
            .iter()
            .enumerate()
            .map(|(i, &n)| (n, i))
            .collect();
        let fits = |dep: f64| pace.take_off(dep).1 + LIFT_OFF_MARGIN_M * k <= runway.len();

        // Out: pushed back to one side of the stand's taxiway, whichever leaves the
        // shorter way to a point of the runway with enough of it ahead.
        let mut best: Option<(f64, Vec<usize>, usize)> = None;
        for &(back, kind) in &field.next[junction] {
            if kind != Kind::Taxi {
                continue;
            }
            let goal = |a: usize, b: usize| -> Option<f64> {
                let &i = index.get(&b)?;
                if i + 1 >= runway.nodes.len() || !fits(runway.at[i] + CORNER_M * k) {
                    return None;
                }
                let turn = turn_deg(field.dir(a, b), field.dir(b, runway.nodes[i + 1]));
                Some(turn_cost(turn) * k + 0.25 * runway.at[i])
            };
            let Some(path) = field.way((back, junction), &free, 3.0, k, goal) else {
                continue;
            };
            let cost: f64 = path.windows(2).map(|p| field.span(p[0], p[1])).sum::<f64>()
                + path
                    .windows(3)
                    .map(|p| turn_cost(turn_deg(field.dir(p[0], p[1]), field.dir(p[1], p[2]))) * k)
                    .sum::<f64>();
            let enters = index[path.last().expect("a way has nodes")];
            if best.as_ref().is_none_or(|(c, ..)| cost < *c) {
                best = Some((cost, path, enters));
            }
        }
        let (out_cost, path, enters) = best?;
        let back = path[0];

        // The pushback, as the way from behind the junction into the stand, backwards.
        let mut line = vec![back, junction];
        line.extend(&stand.path[1..]);
        let probe = self.taxi(&line, 0.0);
        let behind = field.span(back, junction);
        let first = probe.legs.first()?;
        let corner = if first.piece == self.edge(back, junction) {
            behind - first.to
        } else {
            behind
        };
        let start = (behind - corner - (nose * size + 4.0 * k))
            .max(0.5)
            .min((behind - corner).max(0.0));
        let into_stand = self.taxi(&line, start);
        let stop_back = nose * size + NOSE_GAP_M * k;
        let park_at = into_stand.len() - stop_back;
        if park_at < into_stand.turns[1].1 + 1.0 {
            return None;
        }
        let (to_park, _) = cut(&into_stand.legs, park_at);
        let last = *to_park.last()?;
        let park = (last.piece, last.to);
        let push = reversed(&to_park, self.speed(PUSH_MS));

        // Out to the runway and along it.
        let at_b = path.len() - 1;
        let mut nodes = path.clone();
        nodes.extend(&runway.nodes[enters + 1..]);
        let out = self.taxi(&nodes, start);
        let joined = runway.at[enters] + starts_at(&out.legs, out.turns[at_b].1);
        let dep = (joined + 6.0 * k).ceil();
        if !fits(dep) {
            return None;
        }
        let (rotate, _) = pace.take_off(dep);
        let first_on = (2..path.len())
            .find(|&i| field.on_strip[path[i]].is_some_and(|(on, _)| on == strip))
            .unwrap_or(at_b);
        let to_line = out.turns[at_b].1 + (dep - joined);
        let to_hold = (out.turns[first_on].0 - HOLD_SHORT_M * k)
            .max(out.turns[1].1 + 2.0)
            .min(out.turns[first_on].0)
            .min(to_line - 1.0)
            .max(1.0);
        let to_rotate = out.turns[at_b].1 + (rotate - joined);
        let (taxi_out, rest) = cut(&out.legs, to_hold);
        let (line_up, rest) = cut(&rest, to_line - to_hold);
        let (mut roll, _) = cut(&rest, to_rotate - to_line);
        for leg in &mut roll {
            leg.cap = MAX_STEP;
            leg.accel = pace.accel;
            leg.brake = pace.brake;
            leg.holds = Some(strip);
        }

        // In: set down past the touchdown, braked along the runway, off it and to the stand.
        let roll_out = self.speed(ROLL_OUT_MS).min(pace.land);
        let set_down = u.touch + NOSE_DOWN_M * k;
        let slowed = u.touch
            + ((pace.land * pace.land - roll_out * roll_out) / (2.0 * pace.brake))
                .max(NOSE_DOWN_M * k + 1.0);
        if slowed > runway.len() {
            return None;
        }
        let from = runway.node_before(set_down);
        let leaves = runway
            .at
            .partition_point(|&a| a < slowed)
            .min(runway.nodes.len() - 1)
            .max(from + 1);
        let goal = |a: usize, b: usize| -> Option<f64> {
            (b == junction).then(|| turn_cost(turn_deg(field.dir(a, b), field.dir(b, into))) * k)
        };
        // Off at the first way that leads on to the stand: rolling on along the runway
        // would keep it from the next one.
        let way_in = field.way(
            (runway.nodes[leaves - 1], runway.nodes[leaves]),
            &FnvHashSet::default(),
            3.0,
            k,
            goal,
        )?;
        let in_cost: f64 = way_in.windows(2).map(|p| field.span(p[0], p[1])).sum();
        let mut nodes: Vec<usize> = runway.nodes[from..leaves - 1].to_vec();
        let at_junction = nodes.len() + way_in.len() - 1;
        nodes.extend(&way_in);
        nodes.extend(&stand.path[1..]);
        let taxi_in = self.taxi(&nodes, set_down - runway.at[from]);
        let park_at = taxi_in.len() - stop_back;
        if park_at < taxi_in.turns[at_junction].1 + 1.0 {
            return None;
        }
        let (arrive, _) = cut(&taxi_in.legs, park_at);
        let end = arrive.last()?;
        if end.piece != park.0 || (end.to - park.1).abs() > 0.05 {
            return None;
        }
        // Braked hard at first, then rolling out along the runway.
        let (mut braked, mut rest) = cut(&arrive, slowed - set_down);
        for leg in &mut braked {
            leg.cap = pace.land;
            leg.brake = pace.brake;
            leg.holds = Some(strip);
        }
        for leg in &mut rest {
            match self.keys[leg.piece] {
                Key::Edge(a, b) if free.contains(&(a, b)) => leg.cap = roll_out,
                _ => break,
            }
        }
        braked.extend(rest);

        Some(Draft {
            stand: s,
            frame,
            used,
            push,
            out: taxi_out,
            line_up,
            roll,
            dep,
            arrive: braked,
            park,
            taxied: out_cost + in_cost,
        })
    }

    /// The piece of a runway's circuit, wide or tight, made once. None where it does not
    /// clear the ground.
    fn circuit_piece(&mut self, u: &Use, tight: bool) -> Option<usize> {
        let key = Key::Circuit(u.runway.strip, u.runway.back, tight);
        if let Some(&i) = self.by_key.get(&key) {
            return Some(i);
        }
        let shape = if tight { TIGHT_CIRCUIT } else { WIDE_CIRCUIT };
        let fine = self.circuit(
            &u.line,
            shape,
            u.beyond[usize::from(tight)],
            u.touch,
            u.side,
        )?;
        Some(self.piece(key, |_| fine.piece()))
    }

    /// The turn all the way round that an aircraft flies off the middle of a circuit's
    /// downwind leg to land later, away from the runway and level with the leg: its piece,
    /// made once, and how far along the circuit it leaves the leg and rejoins it. None
    /// where it would not clear the ground.
    fn orbit_piece(&mut self, u: &Use, tight: bool, circuit: usize) -> Option<(usize, f64)> {
        let k = self.k;
        let shape = if tight { TIGHT_CIRCUIT } else { WIDE_CIRCUIT };
        let radius = shape.radius * k;
        // The downwind leg as the circuit draws it: from the end of the turn past the far
        // end to abeam the point the final starts at.
        let line = &u.line;
        let far = line.at(line.len() + u.beyond[usize::from(tight)]);
        let to_side = mul(left(line.last()), u.side);
        let from = add(far, mul(to_side, 2.0 * radius));
        let near = line.at(-shape.before * k);
        let to = add(near, mul(left(line.first()), u.side * 2.0 * radius));
        let at = std::f64::consts::PI * radius + norm(sub(to, from)) / 2.0;
        let key = Key::Orbit(u.runway.strip, u.runway.back, tight);
        if let Some(&i) = self.by_key.get(&key) {
            return Some((i, at));
        }
        let start = self.pieces[circuit].at(at);
        let ahead = unit(sub(to, from));
        let centre = add((start.x, start.z), mul(to_side, radius));
        let step = 0.25;
        let n = (2.0 * std::f64::consts::PI * radius / step).ceil() as usize;
        let mut fine = Fine::default();
        for i in 0..=n {
            let t = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            let p = add(
                sub(centre, mul(to_side, radius * t.cos())),
                mul(ahead, radius * t.sin()),
            );
            if i % 8 == 0 && !self.clear((p.0, start.y, p.1), CLEAR_TERRAIN_M * k) {
                return None;
            }
            fine.push((p.0, start.y, p.1), LEVEL_NOSE, false);
        }
        Some((self.piece(key, |_| fine.piece()), at))
    }

    /// The piece of an airframe's climb off a runway from where it lined up, made once.
    fn climb_piece(&mut self, u: &Use, frame: Airframe, dep: f64) -> Option<usize> {
        let key = Key::Climb(u.runway.strip, u.runway.back, frame, dep.round() as i64);
        if let Some(&i) = self.by_key.get(&key) {
            return Some(i);
        }
        let shape = if frame.slow() {
            TIGHT_CIRCUIT
        } else {
            WIDE_CIRCUIT
        };
        let (rotate, lift) = Pace::of(frame, self.k).take_off(dep);
        let fine = self.climb(
            &u.line,
            frame,
            rotate,
            lift,
            u.beyond[usize::from(frame.slow())],
            self.top(shape, u.touch_y),
            u.touch_y,
        )?;
        Some(self.piece(key, |_| fine.piece()))
    }

    /// A draft's whole round as steps, with its climb and circuit laid out.
    fn round(&mut self, draft: &Draft, u: &Use) -> Option<Round> {
        let k = self.k;
        let strip = u.runway.strip;
        let pace = Pace::of(draft.frame, k);
        let climb = self.climb_piece(u, draft.frame, draft.dep)?;
        let circuit = self.circuit_piece(u, draft.frame.slow())?;
        let air = |piece: usize, from: f64, to: f64, cap: f64, holds: bool| Leg {
            piece,
            from,
            to,
            cap,
            accel: pace.accel * 0.6,
            brake: pace.brake,
            holds: holds.then_some(strip),
        };
        let mut fly = draft.roll.clone();
        // The runway is its own until it is past the far end, or well up and away on a
        // long runway, and anyone's after.
        let (rotate, lift) = pace.take_off(draft.dep);
        let climbed = self.pieces[climb].len;
        let past_end =
            ((u.runway.len() + 60.0 * k).min(lift + CLIMB_OUT_M * k) - rotate).clamp(0.0, climbed);
        fly.push(air(climb, 0.0, past_end, pace.air, true));
        fly.push(air(climb, past_end, climbed, pace.air, false));
        // Down to its landing speed well before the wheels touch, the runway its own
        // for the last of the way.
        let len = self.pieces[circuit].len;
        let touch = len - NOSE_DOWN_M * k;
        let slow_from = (touch - 600.0 * k).max(0.0);
        let own_from = (touch - (pace.land * FINAL_S * STEPS_PER_S).max(600.0 * k)).max(0.0);
        // To land later, it may turn all the way round off the downwind leg, once or more.
        let orbit = self
            .orbit_piece(u, draft.frame.slow(), circuit)
            .filter(|&(_, at)| at < own_from)
            .map(|(piece, at)| (piece, at, self.pieces[piece].len));
        let laps = |orbits: usize| -> Vec<Leg> {
            let mut legs = fly.clone();
            match orbit {
                Some((piece, at, round)) if orbits > 0 => {
                    legs.push(air(circuit, 0.0, at, pace.air, false));
                    legs.extend((0..orbits).map(|_| air(piece, 0.0, round, pace.air, false)));
                    legs.push(air(circuit, at, own_from, pace.air, false));
                }
                _ => legs.push(air(circuit, 0.0, own_from, pace.air, false)),
            }
            legs.push(air(circuit, own_from, slow_from, pace.air, true));
            legs.push(air(circuit, slow_from, len, pace.land, true));
            legs.extend(&draft.arrive);
            legs
        };

        let steps = |legs: &[Leg]| -> Vec<Step> {
            run(legs)
                .into_iter()
                .map(|(leg, at)| Step {
                    piece: legs[leg].piece,
                    at,
                    holds: legs[leg].holds,
                })
                .collect()
        };
        let park = Step {
            piece: draft.park.0,
            at: draft.park.1,
            holds: None,
        };
        let pause =
            |from: Step, seconds: f64| std::iter::repeat_n(from, (seconds * STEPS_PER_S) as usize);
        let mut before = steps(&draft.push);
        let pushed = *before.last()?;
        before.extend(pause(pushed, START_UP_S));
        before.extend(steps(&draft.out));
        let mut lined_up = steps(&draft.line_up);
        let lined = *lined_up.last().unwrap_or(before.last()?);
        lined_up.extend(pause(lined, LINE_UP_S));
        let ways = if orbit.is_some() { ORBITS + 1 } else { 1 };
        let afters = (0..ways)
            .map(|orbits| {
                let mut after = lined_up.clone();
                after.extend(steps(&laps(orbits)));
                after
            })
            .collect();
        Some(Round {
            stand: draft.stand,
            frame: draft.frame,
            runway: strip,
            before,
            afters,
            park,
        })
    }

    fn pose(&self, step: Step, frame: Airframe) -> Pose {
        let p = self.pieces[step.piece].at(step.at);
        let yaw = p.yaw.to_radians();
        Pose {
            at: (p.x, p.z),
            y: p.y,
            ahead: (-yaw.sin(), yaw.cos()),
            frame,
        }
    }

    /// The strip an aircraft at `step` is on or has to itself, if any.
    fn on_strip(&self, step: Step, strips: &[usize]) -> Option<usize> {
        if step.holds.is_some() {
            return step.holds;
        }
        if matches!(
            self.keys[step.piece],
            Key::Circuit(..) | Key::Climb(..) | Key::Orbit(..)
        ) {
            return None;
        }
        let p = self.pieces[step.piece].at(step.at);
        let field = self.field;
        strips.iter().copied().find(|&s| {
            let strip = &field.strips[s];
            let (a, b) = (
                field.nodes[strip.nodes[0]].at,
                field.nodes[strip.nodes[strip.nodes.len() - 1]].at,
            );
            point_to_segment((p.x, p.z), a, b) < 0.9 * HOLD_SHORT_M * self.k
        })
    }
}

/// Where an aircraft is each second of part of its round, and when it is on which strip:
/// (strip, from, to) in seconds from the start of the part.
type Half = (Vec<Pose>, Vec<(usize, i64, i64)>);

/// A round's time: its start, its wait before the runway and its turns round.
type Time = (usize, usize, usize);

/// An aircraft's round sampled each second: where it is, and when it is on which strip,
/// for its steps up to the wait before the runway and those after.
struct Sampled {
    before: Vec<Pose>,
    /// (strip, from, to) in seconds from the start of each half.
    busy_before: Vec<(usize, i64, i64)>,
    /// The second half for each number of times it may turn round.
    afters: Vec<Half>,
    park: Pose,
}

fn sample(paths: &Paths, round: &Round, strips: &[usize]) -> Sampled {
    let per = STEPS_PER_S as usize;
    let half = |steps: &[Step]| -> (Vec<Pose>, Vec<(usize, i64, i64)>) {
        let poses = steps
            .iter()
            .step_by(per)
            .map(|&s| paths.pose(s, round.frame))
            .collect();
        let mut busy: Vec<(usize, i64, i64)> = Vec::new();
        for (i, &s) in steps.iter().step_by(per).enumerate() {
            let Some(strip) = paths.on_strip(s, strips) else {
                continue;
            };
            match busy.last_mut() {
                Some(last) if last.0 == strip && last.2 >= i as i64 - 1 => last.2 = i as i64,
                _ => busy.push((strip, i as i64, i as i64)),
            }
        }
        (poses, busy)
    };
    let (before, busy_before) = half(&round.before);
    Sampled {
        before,
        busy_before,
        afters: round.afters.iter().map(|a| half(a)).collect(),
        park: paths.pose(round.park, round.frame),
    }
}

impl Sky {
    fn new(round: usize, strips: usize) -> Self {
        Sky {
            round,
            poses: vec![Vec::new(); round],
            busy: vec![Vec::new(); strips],
        }
    }

    /// The first start and wait at which the round meets nobody, trying starts from
    /// `first` on, and the number of turns round it makes there. The round is then in the
    /// sky for those after it to keep clear of.
    fn place(&mut self, s: &Sampled, first: usize, size: f64, k: f64) -> Option<Time> {
        let round = self.round;
        // The seconds its stand is not its own: someone passes too close to where it stands.
        let mut blocked = vec![0u32; round + 1];
        for t in 0..round {
            let hit = self.poses[t].iter().any(|o| s.park.too_close(o, size, k));
            blocked[t + 1] = blocked[t] + u32::from(hit);
        }
        let stand_free = |from: usize, to: usize| -> bool {
            // Seconds from..to, on the round's clock.
            let (a, b) = (from % round, to % round);
            if to - from >= round {
                blocked[round] == 0
            } else if a <= b {
                blocked[b] - blocked[a] == 0
            } else {
                blocked[round] - blocked[a] + blocked[b] == 0
            }
        };
        let meets = |pose: &Pose, t: usize| {
            self.poses[t % round]
                .iter()
                .any(|o| pose.too_close(o, size, k))
        };
        let runway_free = |strip: usize, from: i64, to: i64| {
            !self.busy[strip]
                .iter()
                .any(|&b| overlap((from - RUNWAY_PAD_S, to + RUNWAY_PAD_S), b, round as i64))
        };
        let nb = s.before.len();
        for start in (0..round)
            .step_by(START_STEP_S)
            .map(|d| (first + d) % round)
        {
            // The runway first: it is the quicker to rule a time out by.
            if !s
                .busy_before
                .iter()
                .all(|&(strip, a, b)| runway_free(strip, start as i64 + a, start as i64 + b))
                || s.before
                    .iter()
                    .enumerate()
                    .any(|(i, p)| meets(p, start + i))
            {
                continue;
            }
            let hold = s.before[nb - 1];
            for wait in (0..=MAX_HOLD_S).step_by(HOLD_STEP_S) {
                if nb + wait + s.afters[0].0.len() + MIN_DWELL_S as usize > round {
                    break;
                }
                // Waiting where it would be in someone's way is no use waiting longer.
                if (0..wait).any(|i| meets(&hold, start + nb + i)) {
                    break;
                }
                let off = start + nb + wait;
                // Of the ways it may come back, the one that leaves the runway the most time
                // either side of its landing, up to a minute, and then the shortest: the
                // runway's take-offs and landings come evenly.
                let mut best: Option<(i64, usize)> = None;
                for (orbits, (after, busy_after)) in s.afters.iter().enumerate() {
                    let total = nb + wait + after.len();
                    if total + MIN_DWELL_S as usize > round {
                        break;
                    }
                    if !busy_after
                        .iter()
                        .all(|&(strip, a, b)| runway_free(strip, off as i64 + a, off as i64 + b))
                        || !stand_free(start + total, start + round)
                        || after.iter().enumerate().any(|(i, p)| meets(p, off + i))
                    {
                        continue;
                    }
                    // Its take-off, the first use of the runway it lands on, and any runway
                    // crossed on the way out are the same whichever way it comes back.
                    let took_off = busy_after
                        .last()
                        .and_then(|l| busy_after.iter().position(|w| w.0 == l.0))
                        .map_or(0, |i| i + 1);
                    let room = busy_after
                        .iter()
                        .skip(took_off)
                        .map(|&(strip, a, b)| {
                            self.room_around(strip, off as i64 + a, off as i64 + b)
                        })
                        .min()
                        .unwrap_or(EVEN_S)
                        .min(EVEN_S);
                    if best.is_none_or(|(r, _)| room > r) {
                        best = Some((room, orbits));
                    }
                }
                if let Some((_, orbits)) = best {
                    let (after, busy_after) = &s.afters[orbits];
                    let total = nb + wait + after.len();
                    // Taken: into the sky with it.
                    for (i, p) in s.before.iter().enumerate() {
                        self.poses[(start + i) % round].push(*p);
                    }
                    for i in 0..wait {
                        self.poses[(start + nb + i) % round].push(hold);
                    }
                    for (i, p) in after.iter().enumerate() {
                        self.poses[(off + i) % round].push(*p);
                    }
                    for t in start + total..start + round {
                        self.poses[t % round].push(s.park);
                    }
                    for &(strip, a, b) in &s.busy_before {
                        self.busy[strip].push((start as i64 + a, start as i64 + b));
                    }
                    for &(strip, a, b) in busy_after {
                        self.busy[strip].push((off as i64 + a, off as i64 + b));
                    }
                    return Some((start, wait, orbits));
                }
            }
        }
        None
    }

    /// The longest any strip in use stands idle between two uses, in seconds.
    fn longest_idle(&self) -> i64 {
        let round = self.round as i64;
        let mut longest = 0;
        for busy in &self.busy {
            let mut uses: Vec<(i64, i64)> = busy
                .iter()
                .map(|&(a, b)| (a.rem_euclid(round), a.rem_euclid(round) + b - a))
                .collect();
            uses.sort_unstable();
            for (i, &(_, end)) in uses.iter().enumerate() {
                let next = uses.get(i + 1).map_or(uses[0].0 + round, |u| u.0);
                longest = longest.max(next - end);
            }
        }
        longest
    }

    /// The time the strip is free either side of a use of it from `from` to `to`, the
    /// shorter, in seconds.
    fn room_around(&self, strip: usize, from: i64, to: i64) -> i64 {
        let round = self.round as i64;
        self.busy[strip]
            .iter()
            .map(|&(a, b)| (a - to).rem_euclid(round).min((from - b).rem_euclid(round)))
            .min()
            .unwrap_or(round)
    }

    /// Whether an aircraft could stand at `pose` all round the clock.
    fn stands_clear(&self, pose: &Pose, size: f64, k: f64) -> bool {
        !self
            .poses
            .iter()
            .any(|at| at.iter().any(|o| pose.too_close(o, size, k)))
    }
}

/// How far each stand's nose is from the next stand's, which is as wide an aircraft as
/// the stands were laid out for. Three lines that end close together are one stand
/// drawn for aircraft of several sizes, a line down the middle for a large one and one
/// either side for two smaller: those have all the room their lines give, and only the
/// aircraft that end up on them keep each other's size down.
fn stands_apart(field: &Field, stands: &[usize], k: f64) -> FnvHashMap<usize, f64> {
    let mut group: Vec<usize> = (0..stands.len()).collect();
    fn root(group: &mut [usize], mut i: usize) -> usize {
        while group[i] != i {
            group[i] = group[group[i]];
            i = group[i];
        }
        i
    }
    let noses: Vec<P> = stands.iter().map(|&s| nose_of(field, s)).collect();
    for a in 0..stands.len() {
        for b in a + 1..stands.len() {
            if norm(sub(noses[a], noses[b])) < ALTERNATE_LINES_M * k {
                let (ra, rb) = (root(&mut group, a), root(&mut group, b));
                group[ra] = rb;
            }
        }
    }
    let roots: Vec<usize> = (0..stands.len()).map(|i| root(&mut group, i)).collect();
    let mut sizes: FnvHashMap<usize, usize> = FnvHashMap::default();
    for &r in &roots {
        *sizes.entry(r).or_default() += 1;
    }
    stands
        .iter()
        .enumerate()
        .map(|(a, &s)| {
            // Two lines so close, or more than three, are small stands side by side.
            let alternates = sizes[&roots[a]] == 3;
            let apart = if alternates {
                f64::MAX
            } else {
                (0..stands.len())
                    .filter(|&b| b != a)
                    .map(|b| norm(sub(noses[b], noses[a])))
                    .fold(f64::MAX, f64::min)
            };
            (s, apart)
        })
        .collect()
}

/// The airframes a stand has room for, the largest first, of those a runway of the
/// airfield takes: its line long enough and the next stand, `apart` from it, far enough
/// off. One that flies leaves the taxiway behind it clear for `passes`, the largest that
/// taxis past, or for its like; one that only stands needs the line for most of its
/// length, since many a stand is drawn with a shorter line than it has room.
fn frames_for(
    field: &Field,
    stand: usize,
    apart: f64,
    uses: &[Use],
    flies: Option<Option<Airframe>>,
    size: f64,
    k: f64,
) -> Vec<Airframe> {
    let path = &field.stands[stand].path;
    let line: f64 = path.windows(2).map(|p| field.span(p[0], p[1])).sum();
    ALL.into_iter()
        .rev()
        .filter(|f| {
            let (len, span) = f.size_m();
            let needs = match flies {
                Some(passes) => {
                    let wing = passes.unwrap_or(*f).size_m().1 / 2.0 * size + (CLEAR_M + 1.0) * k;
                    len * size + NOSE_GAP_M * k + wing.max(TAIL_ROOM_M * k)
                }
                None => (PARKED_LINE_SHARE * len).max(f.reach_m().0 + 2.0) * size + NOSE_GAP_M * k,
            };
            needs <= line
                && span * size + (CLEAR_M + 1.5) * k <= apart
                && uses.iter().any(|u| u.frames.contains(f))
        })
        .collect()
}

/// The stands taken so far: where each one's nose stops, and what stands there.
#[derive(Default)]
struct Taken(Vec<(usize, P, Airframe)>);

impl Taken {
    /// Whether `frame` at `stand` would stand wingtip clear of everything taken. Stands
    /// are often drawn with a line down the middle for a large aircraft and one either
    /// side for two smaller: whichever is taken first, the others then stay empty.
    fn room(&self, field: &Field, stand: usize, frame: Airframe, size: f64, k: f64) -> bool {
        let nose = nose_of(field, stand);
        !self.0.iter().any(|&(other, at, has)| {
            other != stand
                && norm(sub(at, nose))
                    < (frame.size_m().1 + has.size_m().1) / 2.0 * size + (CLEAR_M + 1.5) * k
        })
    }

    fn take(&mut self, field: &Field, stand: usize, frame: Airframe) {
        self.0.push((stand, nose_of(field, stand), frame));
    }
}

fn nose_of(field: &Field, stand: usize) -> P {
    field.nodes[*field.stands[stand].path.last().expect("a stand has nodes")].at
}

/// Which of `count` pieces the flights and the aircraft that only stand are ever on.
fn pieces_used(count: usize, flights: &[Flight], parked: &[Step]) -> Vec<bool> {
    let mut used = vec![false; count];
    for f in flights {
        for s in f.round.before.iter().chain(f.after()) {
            used[s.piece] = true;
        }
        used[f.round.park.piece] = true;
    }
    for s in parked {
        used[s.piece] = true;
    }
    used
}

/// The route points the used pieces come to, with the one that ends the route.
fn points_held(pieces: &[Piece], used: &[bool]) -> usize {
    used.iter()
        .zip(pieces)
        .filter(|(u, _)| **u)
        .map(|(_, p)| p.pts.len())
        .sum::<usize>()
        + 1
}

#[allow(clippy::too_many_arguments)]
fn plan_field(
    field: &Field,
    strips: &[usize],
    stands: &[usize],
    k: f64,
    size: f64,
    ground: &dyn Fn(i32, i32) -> Option<f64>,
    tall: &[(P, f64)],
    room: Room,
) -> Option<(Route, Vec<Service>, usize)> {
    let first = field.nodes[field.strips[strips[0]].nodes[0]].at;
    let datum = ground(first.0.floor() as i32, first.1.floor() as i32).unwrap_or(64.0);
    let mut paths = Paths {
        field,
        k,
        ground,
        datum,
        ceiling: room.ceiling,
        pieces: Vec::new(),
        keys: Vec::new(),
        by_key: FnvHashMap::default(),
        tall,
    };
    let centre = (!stands.is_empty()).then(|| {
        let sum = stands.iter().fold((0.0, 0.0), |acc, &s| {
            add(acc, field.nodes[*field.stands[s].path.last().unwrap()].at)
        });
        mul(sum, 1.0 / stands.len() as f64)
    });
    let mut uses = runway_uses(&paths, strips, centre);
    if uses.is_empty() {
        return None;
    }

    // Each stand's aircraft, by what fits it. The stands that take the largest fly first,
    // so an airport's flights are its airliners and what is small keeps out of their way;
    // among stands alike, in an order of their own, spread over the airfield.
    // What flies leaves room behind it for the largest aircraft any stand here takes.
    let apart = stands_apart(field, stands, k);
    let largest = stands
        .iter()
        .filter_map(|&s| {
            frames_for(field, s, apart[&s], &uses, Some(None), size, k)
                .first()
                .copied()
        })
        .max();
    let mut order: Vec<usize> = stands.to_vec();
    order.sort_by_key(|&s| {
        let largest = frames_for(field, s, apart[&s], &uses, Some(largest), size, k)
            .first()
            .copied();
        (
            std::cmp::Reverse(largest),
            hash(field.stands[s].id, 0x41),
            s,
        )
    });
    let pick = |s: usize, fits: &[Airframe]| -> Vec<Airframe> {
        // Mostly the largest that fits, then the next sizes down.
        let roll = hash(field.stands[s].id, 0x7) % 100;
        let first = match (roll, fits.len()) {
            (0..=54, _) | (_, 1) => 0,
            (55..=84, _) | (_, 2) => 1,
            _ => 2,
        };
        let mut list: Vec<Airframe> = fits[first..].to_vec();
        list.extend(fits[..first].iter().rev());
        list
    };
    let target = MAX_FLYING_PER_FIELD.min(room.flying).min(uses.len() * 7);
    let mut drafts: Vec<Draft> = Vec::new();
    let mut idle: Vec<usize> = Vec::new();
    let mut taken = Taken::default();
    for &s in &order {
        let fits = frames_for(field, s, apart[&s], &uses, Some(largest), size, k);
        let mut made = None;
        if drafts.len() < target && !fits.is_empty() {
            'frames: for frame in pick(s, &fits) {
                if !taken.room(field, s, frame, size, k) {
                    continue;
                }
                // The runway with the shorter way, each aircraft already flying from one
                // counting as a longer way to it, so that the runways share the flights.
                let cost = |d: &Draft| {
                    let flown = drafts.iter().filter(|o| o.used == d.used).count();
                    d.taxied + flown as f64 * RUNWAY_TAKEN_M * k
                };
                let mut best: Option<Draft> = None;
                for (i, u) in uses.iter().enumerate() {
                    if !u.frames.contains(&frame) {
                        continue;
                    }
                    if let Some(d) = paths.draft(s, frame, i, u, size) {
                        if best.as_ref().is_none_or(|b| cost(&d) < cost(b)) {
                            best = Some(d);
                        }
                    }
                }
                if best.is_some() {
                    made = best;
                    break 'frames;
                }
            }
        }
        match made {
            Some(d) => {
                taken.take(field, s, d.frame);
                drafts.push(d);
            }
            None => idle.push(s),
        }
    }

    // The circuits start far enough out for the slowest climb to be up by then.
    for d in &drafts {
        let u = &mut uses[d.used];
        let shape = if d.frame.slow() {
            TIGHT_CIRCUIT
        } else {
            WIDE_CIRCUIT
        };
        let (_, lift) = Pace::of(d.frame, k).take_off(d.dep);
        let at = u.line.at(lift);
        let rise = u.touch_y + paths.top(shape, u.touch_y) - paths.surface(at.0, at.1);
        let need = lift + climb_run(d.frame, rise, k) + 40.0 * k - u.line.len();
        let slot = usize::from(d.frame.slow());
        u.beyond[slot] = u.beyond[slot].max(need);
    }
    let mut rounds: Vec<Round> = Vec::new();
    for d in &drafts {
        match paths.round(d, &uses[d.used]) {
            Some(r) => rounds.push(r),
            None => idle.push(d.stand),
        }
    }
    let seconds = |r: &Round| (r.before.len() + r.afters[0].len()) as f64 / STEPS_PER_S;
    rounds.retain(|r| {
        let fits = seconds(r) + MIN_DWELL_S + 30.0 <= MAX_ROUND_S;
        if !fits {
            idle.push(r.stand);
        }
        fits
    });

    // One clock for the airfield, long enough for every round and for all of them: each
    // runway is in use for well under its share of it. Where that would take too long a
    // clock, the last rounds stay down.
    let mut sampled: Vec<Sampled> = rounds.iter().map(|r| sample(&paths, r, strips)).collect();
    let longest = rounds.iter().map(&seconds).fold(0.0, f64::max);
    let clock_for = |rounds: &[Round], sampled: &[Sampled]| -> f64 {
        let mut in_use = vec![0.0f64; field.strips.len()];
        let mut flown = vec![0usize; field.strips.len()];
        for (r, s) in rounds.iter().zip(sampled) {
            flown[r.runway] += 1;
            for &(strip, a, b) in s.busy_before.iter().chain(&s.afters[0].1) {
                in_use[strip] += (b - a + 2 * RUNWAY_PAD_S) as f64;
            }
        }
        let busiest = in_use.iter().copied().fold(0.0, f64::max);
        let most = flown.iter().copied().max().unwrap_or(0);
        (most as f64 * SLOT_S)
            .max(longest + MIN_DWELL_S + 30.0)
            .max(busiest / RUNWAY_SHARE)
            .max(MIN_ROUND_S)
    };
    while sampled.len() > 1 && clock_for(&rounds, &sampled) > MAX_ROUND_S {
        sampled.pop();
        if let Some(r) = rounds.pop() {
            idle.push(r.stand);
        }
    }

    // The rounds are given times two ways: each from a time of its own, so that the
    // take-offs from each runway come evenly over the clock, and each from where the one
    // before it went, packed close. Of those that find every round a time, the one that
    // leaves no runway idle for as long; where neither does, the clock is let out a little
    // and both are tried again. At the longest clock, those left stay down.
    let mut on_runway = vec![0usize; field.strips.len()];
    let rank: Vec<usize> = rounds
        .iter()
        .map(|r| {
            on_runway[r.runway] += 1;
            on_runway[r.runway] - 1
        })
        .collect();
    let longest_clock = MAX_ROUND_S as usize;
    let mut clock = (clock_for(&rounds, &sampled).min(MAX_ROUND_S).ceil() as usize).max(1);
    let (clock, times, sky) = loop {
        let last = clock >= longest_clock;
        let mut best: Option<(i64, Vec<Option<Time>>, Sky)> = None;
        for packed in [false, true] {
            let mut sky = Sky::new(clock, field.strips.len());
            let mut times = Vec::with_capacity(rounds.len());
            let mut from = 0;
            for ((r, s), &i) in rounds.iter().zip(&sampled).zip(&rank) {
                let first = if packed {
                    from
                } else {
                    let takes_off = i * clock / on_runway[r.runway].max(1);
                    (takes_off + clock - s.before.len() % clock) % clock
                };
                let time = sky.place(s, first, size, k);
                if let Some((start, ..)) = time {
                    from = start;
                } else if !last {
                    break;
                }
                times.push(time);
            }
            let all = times.len() == rounds.len() && times.iter().all(Option::is_some);
            if all || (last && !packed) {
                let idle = if all { sky.longest_idle() } else { i64::MAX };
                if best.as_ref().is_none_or(|(b, ..)| idle < *b) {
                    best = Some((idle, times, sky));
                }
            }
        }
        if let Some((_, times, sky)) = best {
            break (clock, times, sky);
        }
        clock = ((clock as f64 * CLOCK_STEP).ceil() as usize).min(longest_clock);
    };
    let mut flights: Vec<Flight> = Vec::new();
    for (round, time) in rounds.into_iter().zip(times) {
        match time {
            Some((start, wait, orbits)) => flights.push(Flight {
                round,
                start,
                wait,
                orbits,
            }),
            None => idle.push(round.stand),
        }
    }

    // The points the flights take; over what there is room for, the last flights stay down.
    while !flights.is_empty()
        && points_held(
            &paths.pieces,
            &pieces_used(paths.pieces.len(), &flights, &[]),
        ) > room.points
    {
        flights.pop();
    }

    // Aircraft that only stand, at stands nobody flies from and nothing passes too close to.
    idle.sort_by_key(|&s| (hash(field.stands[s].id, 0x41), s));
    idle.dedup();
    let mut parked: Vec<(usize, Airframe, Step)> = Vec::new();
    let mut standing: Vec<Pose> = Vec::new();
    // Only the flights that found a time hold their stands.
    let mut taken = Taken::default();
    for f in &flights {
        taken.take(field, f.round.stand, f.round.frame);
    }
    for &s in &idle {
        if parked.len() >= room.parked {
            break;
        }
        if (hash(field.stands[s].id, 0x13) % 1000) as f64 >= STAND_OCCUPANCY * 1000.0 {
            continue;
        }
        let fits = frames_for(field, s, apart[&s], &uses, None, size, k);
        if fits.is_empty() {
            continue;
        }
        for frame in pick(s, &fits) {
            if !taken.room(field, s, frame, size, k) {
                continue;
            }
            let stand = &field.stands[s];
            let line = paths.taxi(&stand.path, 0.0);
            let (nose, _) = frame.reach_m();
            let park_at = line.len() - (nose * size + NOSE_GAP_M * k);
            let (to_park, _) = cut(&line.legs, park_at);
            let Some(last) = to_park.last() else {
                continue;
            };
            let step = Step {
                piece: last.piece,
                at: last.to,
                holds: None,
            };
            let pose = paths.pose(step, frame);
            if sky.stands_clear(&pose, size, k)
                && !standing.iter().any(|o| pose.too_close(o, size, k))
            {
                standing.push(pose);
                taken.take(field, s, frame);
                parked.push((s, frame, step));
                break;
            }
        }
    }
    let steps_of = |parked: &[(usize, Airframe, Step)]| -> Vec<Step> {
        parked.iter().map(|&(_, _, step)| step).collect()
    };
    let mut used = pieces_used(paths.pieces.len(), &flights, &steps_of(&parked));
    while points_held(&paths.pieces, &used) > room.points && !parked.is_empty() {
        parked.pop();
        used = pieces_used(paths.pieces.len(), &flights, &steps_of(&parked));
    }
    if flights.is_empty() && parked.is_empty() {
        return None;
    }

    // The route: every piece used, one after another, and a point more so that no place
    // on a piece is past the route's last point.
    let mut starts = vec![0usize; paths.pieces.len()];
    let mut points = Vec::new();
    let mut ys = Vec::new();
    let mut air = Air::default();
    for (i, piece) in paths.pieces.iter().enumerate() {
        if !used[i] {
            continue;
        }
        starts[i] = points.len();
        for p in &piece.pts {
            points.push((p.x, p.z));
            ys.push(p.y);
            air.yaw.push(p.yaw as f32);
            air.climb.push(p.climb as f32);
            air.nose.push(p.nose as f32);
            air.gear.push(p.gear);
        }
    }
    let last = points.len() - 1;
    points.push(points[last]);
    ys.push(ys[last]);
    air.yaw.push(air.yaw[last]);
    air.climb.push(0.0);
    air.nose.push(air.nose[last]);
    air.gear.push(air.gear[last]);
    let place = |s: Step| -> i32 { ((starts[s.piece] as f64 + s.at) * 1000.0).round() as i32 };
    let route = Route {
        mode: Mode::Air,
        shape: Shape::Shuttle,
        ends: [End::Buffer; 2],
        ways: vec![0; points.len()],
        caps: Vec::new(),
        stops: Vec::new(),
        points,
        ys: Some(ys),
        lift: 0.0,
        air: Some(air),
    };
    let livery = |s: usize, frame: Airframe| -> Livery {
        let all = frame.liveries();
        all[(hash(field.stands[s].id, 0x29) % all.len() as u64) as usize]
    };
    let service = |frame: Airframe, livery: Livery, timetable: Vec<i32>| Service {
        route: 0,
        coaches: vec![Coach::Plane(frame)],
        offsets: vec![0.0],
        timetable,
        period: 0,
        trains: vec![(0, livery)],
    };
    let per = STEPS_PER_S as usize;
    let mut services = Vec::new();
    let flying = flights.len();
    for f in &flights {
        let len = clock * per;
        let mut timetable = vec![place(f.round.park); len];
        let hold = f.round.before.last().copied().unwrap_or(f.round.park);
        let steps = f
            .round
            .before
            .iter()
            .copied()
            .chain(std::iter::repeat_n(hold, f.wait * per))
            .chain(f.after().iter().copied());
        for (i, s) in steps.enumerate() {
            timetable[(f.start * per + i) % len] = place(s);
        }
        let frame = f.round.frame;
        services.push(service(frame, livery(f.round.stand, frame), timetable));
    }
    for &(s, frame, step) in &parked {
        services.push(service(frame, livery(s, frame), vec![place(step)]));
    }
    Some((route, services, flying))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm_parser::{ProcessedNode, ProcessedWay};
    use std::collections::HashMap;

    /// A way of `kind` through the points, its nodes named by where they are, so that
    /// ways meeting at a point share its node.
    fn way(id: u64, kind: &str, at: &[(i32, i32)]) -> ProcessedElement {
        let nodes = at
            .iter()
            .map(|&(x, z)| ProcessedNode {
                id: (x as u64) << 20 | (z + 5000) as u64,
                tags: HashMap::new(),
                x,
                z,
            })
            .collect();
        let mut tags = HashMap::new();
        tags.insert("aeroway".to_string(), kind.to_string());
        ProcessedElement::Way(ProcessedWay { id, nodes, tags })
    }

    /// The same airfield with every distance `k` times as long, as on a world of that scale.
    fn scaled(elements: Vec<ProcessedElement>, k: f64) -> Vec<ProcessedElement> {
        elements
            .into_iter()
            .map(|e| match e {
                ProcessedElement::Way(mut w) => {
                    for n in &mut w.nodes {
                        n.x = (f64::from(n.x) * k).round() as i32;
                        n.z = (f64::from(n.z) * k).round() as i32;
                    }
                    ProcessedElement::Way(w)
                }
                other => other,
            })
            .collect()
    }

    /// A runway of `length` along x with a taxiway beside it, links between the two, and
    /// a taxilane beyond with `stands` stands off it, each `depth` deep.
    fn airfield(length: i32, stands: usize, depth: i32) -> Vec<ProcessedElement> {
        let links = [0, length / 5, length / 2, 4 * length / 5, length];
        let mut out = vec![
            way(
                1,
                "runway",
                &links.iter().map(|&x| (x, 0)).collect::<Vec<_>>(),
            ),
            way(
                2,
                "taxiway",
                &[
                    (0, 200),
                    (length / 5, 200),
                    (length / 2 - 500, 200),
                    (length / 2, 200),
                    (length / 2 + 500, 200),
                    (4 * length / 5, 200),
                    (length, 200),
                ],
            ),
        ];
        for (i, &x) in links.iter().enumerate() {
            out.push(way(10 + i as u64, "taxiway", &[(x, 0), (x, 200)]));
        }
        // The taxilane, with a node where each stand leaves it.
        let (from, to) = (length / 2 - 500, length / 2 + 500);
        let gap = (to - from) / (stands as i32 + 1);
        let mut lane = vec![(from, 200), (from, 350)];
        lane.extend((1..=stands as i32).map(|i| (from + i * gap, 350)));
        lane.extend([(to, 350), (to, 200)]);
        out.push(way(30, "taxilane", &lane));
        for i in 1..=stands as i32 {
            let x = from + i * gap;
            out.push(way(
                100 + i as u64,
                "parking_position",
                &[(x, 350), (x, 350 + depth)],
            ));
        }
        out
    }

    fn flat(_: i32, _: i32) -> Option<f64> {
        Some(64.0)
    }

    /// Where the pack puts an aircraft whose timetable says `v`.
    fn put(route: &Route, v: i32) -> Pt {
        let air = route.air.as_ref().expect("an airfield's route");
        let i = (v / 1000) as usize;
        let f = f64::from(v % 1000) / 1000.0;
        let (yaw, climb) = (
            f64::from(air.yaw[i]).to_radians(),
            f64::from(air.climb[i]).to_radians(),
        );
        Pt {
            x: route.points[i].0 - yaw.sin() * climb.cos() * f,
            y: route.ys.as_ref().unwrap()[i] + climb.sin() * f,
            z: route.points[i].1 + yaw.cos() * climb.cos() * f,
            yaw: f64::from(air.yaw[i]),
            climb: f64::from(air.climb[i]),
            nose: f64::from(air.nose[i]),
            gear: air.gear[i],
        }
    }

    fn frame_of(service: &Service) -> Airframe {
        match service.coaches[0] {
            Coach::Plane(frame) => frame,
            other => panic!("not an aircraft: {other:?}"),
        }
    }

    fn pose(route: &Route, service: &Service, step: usize) -> Pose {
        let p = put(route, service.timetable[step % service.timetable.len()]);
        let yaw = p.yaw.to_radians();
        Pose {
            at: (p.x, p.z),
            y: p.y,
            ahead: (-yaw.sin(), yaw.cos()),
            frame: frame_of(service),
        }
    }

    fn planned(length: i32, stands: usize, depth: i32) -> Vec<(Route, Vec<Service>)> {
        AirPlan::build(
            &airfield(length, stands, depth),
            1.0,
            usize::MAX,
            f64::INFINITY,
            flat,
        )
        .into_fields()
    }

    #[test]
    fn an_airfield_has_aircraft_that_fly_and_aircraft_that_stand() {
        let plan = AirPlan::build(&airfield(3000, 9, 70), 1.0, usize::MAX, f64::INFINITY, flat);
        let (flying, parked) = plan.counts();
        assert!(flying >= 4, "{flying} flying");
        assert!(parked >= 1, "{parked} parked");
        let fields = plan.into_fields();
        assert_eq!(fields.len(), 1);
        let (route, services) = &fields[0];
        assert_eq!(services.len(), flying + parked);
        let last = (route.points.len() as i32 - 1) * 1000;
        for s in services {
            assert!(s.timetable.iter().all(|&v| (0..last).contains(&v)));
            assert_eq!(s.trains.len(), 1);
        }
        // Those that fly share one clock; those that stand have one place.
        let clocks: Vec<usize> = services[..flying]
            .iter()
            .map(|s| s.timetable.len())
            .collect();
        assert!(
            clocks.iter().all(|&c| c == clocks[0] && c >= 6000),
            "{clocks:?}"
        );
        assert!(services[flying..].iter().all(|s| s.timetable.len() == 1));
    }

    /// A timetable jumps from one piece of path to the next where the two meet, so the
    /// aircraft itself never jumps: no further in a step than it may move, and never
    /// turned much at once.
    #[test]
    fn an_aircraft_moves_smoothly_from_piece_to_piece() {
        for (route, services) in planned(3000, 9, 70) {
            for s in services.iter().filter(|s| s.timetable.len() > 1) {
                let n = s.timetable.len();
                for i in 0..n {
                    let (a, b) = (
                        put(&route, s.timetable[i]),
                        put(&route, s.timetable[(i + 1) % n]),
                    );
                    let moved =
                        ((b.x - a.x).powi(2) + (b.y - a.y).powi(2) + (b.z - a.z).powi(2)).sqrt();
                    assert!(moved <= MAX_STEP + 0.6, "step {i}: moved {moved:.2}");
                    let turned = (b.yaw - a.yaw + 540.0).rem_euclid(360.0) - 180.0;
                    assert!(turned.abs() < 25.0, "step {i}: turned {turned:.1}");
                    assert!(
                        (b.nose - a.nose).abs() < 2.0,
                        "step {i}: nose {}",
                        b.nose - a.nose
                    );
                }
            }
        }
    }

    #[test]
    fn a_turn_round_off_the_downwind_leaves_and_rejoins_it_level_and_away_from_the_runway() {
        let elements = airfield(3000, 9, 70);
        let field = Field::read(&elements, 1.0);
        let strips: Vec<usize> = (0..field.strips.len()).collect();
        let mut paths = Paths {
            field: &field,
            k: 1.0,
            ground: &flat,
            datum: 64.0,
            ceiling: f64::INFINITY,
            pieces: Vec::new(),
            keys: Vec::new(),
            by_key: FnvHashMap::default(),
            tall: &[],
        };
        let uses = runway_uses(&paths, &strips, None);
        let u = &uses[0];
        let off_line = |p: &Pt| point_to_segment((p.x, p.z), u.line.at(-5000.0), u.line.at(9000.0));
        for tight in [false, true] {
            let circuit = paths.circuit_piece(u, tight).expect("a circuit");
            let (orbit, at) = paths.orbit_piece(u, tight, circuit).expect("room to turn");
            let leaves = paths.pieces[circuit].at(at);
            let pts = &paths.pieces[orbit].pts;
            for p in [pts[0], *pts.last().expect("points")] {
                let gap = (p.x - leaves.x).hypot(p.z - leaves.z);
                assert!(gap < 1.0, "{gap:.2} blocks off the leg");
                let turned = (p.yaw - leaves.yaw + 540.0).rem_euclid(360.0) - 180.0;
                assert!(turned.abs() < 3.0, "turned {turned:.1}");
            }
            assert!(pts.iter().all(|p| (p.y - leaves.y).abs() < 1e-6 && !p.gear));
            assert!(pts.iter().all(|p| off_line(p) > off_line(&leaves) - 1.0));
            assert!(at < paths.pieces[circuit].len / 2.0, "on the downwind");
        }
    }

    // Its one taxilane keeps this airfield's aircraft apart on the ground more than its
    // runway does; a real airport's runway sees one every minute or two.
    #[test]
    fn the_runway_sees_a_take_off_or_landing_every_two_minutes_or_so() {
        let fields = AirPlan::build(&airfield(3000, 9, 70), 1.0, usize::MAX, f64::INFINITY, flat)
            .into_fields();
        let (route, services) = &fields[0];
        let gear = &route.air.as_ref().expect("an airfield's route").gear;
        let mut moves = Vec::new();
        let mut clock = 0;
        for s in services.iter().filter(|s| s.timetable.len() > 1) {
            clock = s.timetable.len();
            let down = |i: usize| gear[(s.timetable[i % clock] / 1000) as usize];
            // The wheels go up as it lifts off and come down on the approach.
            moves.extend((0..clock).filter(|&i| down(i) != down(i + 1)));
        }
        moves.sort_unstable();
        let longest = (0..moves.len())
            .map(|i| (moves[(i + 1) % moves.len()] + clock - moves[i]) % clock)
            .max()
            .expect("aircraft fly") as f64
            / STEPS_PER_S;
        let every = clock as f64 / moves.len() as f64 / STEPS_PER_S;
        assert!(every <= 140.0, "one every {every:.0} s");
        assert!(longest <= 480.0, "{longest:.0} s without one");
    }

    #[test]
    fn no_two_aircraft_ever_meet() {
        for (route, services) in planned(3000, 9, 70) {
            let clock = services.iter().map(|s| s.timetable.len()).max().unwrap();
            for step in (0..clock).step_by(5) {
                let poses: Vec<Pose> = services.iter().map(|s| pose(&route, s, step)).collect();
                for a in 0..poses.len() {
                    for b in a + 1..poses.len() {
                        // Sampled finer than they were planned, so with a little less room.
                        let near = Pose::too_close(&poses[a], &poses[b], 1.0, 0.5);
                        assert!(!near, "aircraft {a} and {b} meet at step {step}");
                    }
                }
            }
        }
    }

    /// On a world at half or twice real size the airfield is that much smaller or larger
    /// and so are its aircraft: they fly all the same, keep apart, and never move faster
    /// than the pack can follow.
    #[test]
    fn aircraft_fly_at_other_scales() {
        for k in [0.5, 2.0] {
            let plan = AirPlan::build(
                &scaled(airfield(3000, 9, 70), k),
                k,
                usize::MAX,
                f64::INFINITY,
                flat,
            );
            let (flying, _) = plan.counts();
            assert!(flying >= 3, "{flying} flying at scale {k}");
            for (route, services) in plan.into_fields() {
                let clock = services.iter().map(|s| s.timetable.len()).max().unwrap();
                for s in services.iter().filter(|s| s.timetable.len() > 1) {
                    for i in 0..clock {
                        let (a, b) = (
                            put(&route, s.timetable[i]),
                            put(&route, s.timetable[(i + 1) % clock]),
                        );
                        let moved =
                            ((b.x - a.x).powi(2) + (b.y - a.y).powi(2) + (b.z - a.z).powi(2))
                                .sqrt();
                        assert!(
                            moved <= MAX_STEP + 0.6,
                            "scale {k} step {i}: moved {moved:.2}"
                        );
                    }
                }
                for step in (0..clock).step_by(10) {
                    let poses: Vec<Pose> = services.iter().map(|s| pose(&route, s, step)).collect();
                    for a in 0..poses.len() {
                        for b in a + 1..poses.len() {
                            let near = Pose::too_close(&poses[a], &poses[b], k, 0.5 * k);
                            assert!(!near, "scale {k}: aircraft {a} and {b} meet at step {step}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn an_aircraft_takes_off_flies_high_and_comes_back_to_its_stand() {
        for (route, services) in planned(3000, 9, 70) {
            for s in services.iter().filter(|s| s.timetable.len() > 1) {
                let places: Vec<Pt> = s.timetable.iter().map(|&v| put(&route, v)).collect();
                let top = places.iter().map(|p| p.y).fold(f64::MIN, f64::max);
                let frame = frame_of(s);
                let height = if frame.slow() { 60.0 } else { 130.0 };
                assert!(
                    (top - 65.0 - height).abs() < 2.0,
                    "{frame:?} tops out at {top}"
                );
                // Wheels down on the ground and up at the top, nose up as it climbs.
                assert!(places.iter().filter(|p| p.y < 66.0).all(|p| p.gear));
                assert!(places.iter().filter(|p| p.y > top - 1.0).all(|p| !p.gear));
                assert!(places.iter().any(|p| p.nose > 8.0 && p.climb > 5.0));
                // It stands for a while at the place it comes back to.
                let mut stays: HashMap<i32, usize> = HashMap::new();
                for &v in &s.timetable {
                    *stays.entry(v).or_default() += 1;
                }
                let home = stays.values().copied().max().unwrap();
                assert!(home >= (MIN_DWELL_S * STEPS_PER_S) as usize, "{home}");
                // On the ground it is on the ground: never under it, never floating.
                assert!(places.iter().all(|p| p.y >= 64.99));
            }
        }
    }

    #[test]
    fn a_stand_takes_the_aircraft_it_has_room_for() {
        // Stands 40 deep hold nothing bigger than a business jet.
        for (_, services) in planned(3000, 9, 40) {
            assert!(!services.is_empty());
            for s in &services {
                assert!(frame_of(s) <= Airframe::Bizjet, "{:?}", frame_of(s));
            }
        }
        // A short strip takes light aircraft only.
        for (_, services) in planned(600, 4, 40) {
            assert!(services.iter().all(|s| frame_of(s) == Airframe::Light));
        }
    }

    /// A stand drawn with a line down its middle and one either side, for one large
    /// aircraft or two small, holds whichever comes first and nothing beside its wing.
    /// Lines as close together with a lead-in each are a row of small stands.
    #[test]
    fn lines_side_by_side_hold_aircraft_that_clear_each_other() {
        let mut elements = airfield(3000, 0, 70);
        // The taxilane runs along z = 350; three lines fan out from one point of it.
        for (i, x) in [1482, 1500, 1518].into_iter().enumerate() {
            elements.push(way(
                300 + i as u64,
                "parking_position",
                &[(1500, 350), (x, 450)],
            ));
        }
        // And a row of four, 18 apart, each leaving the taxilane by itself.
        for i in 0..4 {
            let x = 1700 + i * 18;
            elements.push(way(
                310 + i as u64,
                "parking_position",
                &[(x, 350), (x, 450)],
            ));
        }
        let fields = AirPlan::build(&elements, 1.0, usize::MAX, f64::INFINITY, flat).into_fields();
        let (route, services) = &fields[0];
        assert!(!services.is_empty());
        let standing: Vec<(Pt, Airframe)> = services
            .iter()
            .map(|s| {
                // Where it stands longest is its stand.
                let mut stays: HashMap<i32, usize> = HashMap::new();
                for &v in &s.timetable {
                    *stays.entry(v).or_default() += 1;
                }
                let home = stays.into_iter().max_by_key(|&(_, n)| n).unwrap().0;
                (put(route, home), frame_of(s))
            })
            .collect();
        // The fan holds something bigger than its lines are apart, the row only what
        // fits between its lines.
        let (fan, row): (Vec<_>, Vec<_>) = standing.iter().partition(|(at, _)| at.x < 1600.0);
        assert!(fan.iter().any(|(_, f)| f.size_m().1 > 18.0), "{fan:?}");
        assert!(
            !row.is_empty() && row.iter().all(|(_, f)| f.size_m().1 < 18.0),
            "{row:?}"
        );
        for (a, (at_a, frame_a)) in standing.iter().enumerate() {
            for (at_b, frame_b) in &standing[a + 1..] {
                let apart = (at_a.x - at_b.x).hypot(at_a.z - at_b.z);
                let wings = (frame_a.size_m().1 + frame_b.size_m().1) / 2.0;
                assert!(
                    apart > wings,
                    "{frame_a:?} and {frame_b:?} stand {apart:.0} apart"
                );
            }
        }
    }

    /// Bedrock cannot put an entity above its world's top. Under a ceiling the circuit is
    /// flown lower, and where that leaves it no height at all nothing flies or stands, so
    /// the airfield keeps the aeroplanes built of blocks.
    #[test]
    fn a_ceiling_keeps_the_circuit_down_or_the_aircraft_away() {
        let low = AirPlan::build(&airfield(3000, 9, 70), 1.0, usize::MAX, 65.0 + 80.0, flat);
        assert!(low.counts().0 >= 3);
        for (route, services) in low.into_fields() {
            let top = services
                .iter()
                .flat_map(|s| s.timetable.iter().map(|&v| put(&route, v).y))
                .fold(f64::MIN, f64::max);
            assert!((top - 145.0).abs() < 1.0, "tops out at {top}");
        }
        let none = AirPlan::build(&airfield(3000, 9, 70), 1.0, usize::MAX, 65.0 + 20.0, flat);
        assert!(none.is_empty() && none.extents().is_empty());
    }

    /// The pack holds only so many route points, and the plan is made to fit them.
    #[test]
    fn the_plan_fits_the_room_the_pack_has() {
        let all = AirPlan::build(&airfield(3000, 9, 70), 1.0, usize::MAX, f64::INFINITY, flat);
        let points = |plan: AirPlan| -> usize {
            plan.into_fields().iter().map(|(r, _)| r.points.len()).sum()
        };
        let (flying, _) = all.counts();
        let whole = points(all);
        let tight = AirPlan::build(&airfield(3000, 9, 70), 1.0, whole / 2, f64::INFINITY, flat);
        assert!(tight.counts().0 < flying);
        assert!(points(tight) <= whole / 2);
        assert!(AirPlan::build(&airfield(3000, 9, 70), 1.0, 0, f64::INFINITY, flat).is_empty());
    }

    /// Where an airfield has aircraft, its ground is named, for the block-built aeroplanes
    /// to keep off; one with none is not.
    #[test]
    fn the_ground_of_an_airfield_with_aircraft_is_named() {
        let mut elements = airfield(3000, 9, 70);
        // A second runway far off, with nothing but itself: no aircraft there.
        elements.push(way(900, "runway", &[(0, 5000), (2000, 5000)]));
        let plan = AirPlan::build(&elements, 1.0, usize::MAX, f64::INFINITY, flat);
        let [[x0, z0, x1, z1]] = plan.extents()[..] else {
            panic!("one airfield has aircraft: {:?}", plan.extents());
        };
        // The runway, the taxiways and the stands, and a little round them.
        assert!(x0 < 0.0 && x1 > 3000.0 && z0 < 0.0 && z1 > 420.0 && z1 < 500.0);
    }

    #[test]
    fn a_runway_alone_has_no_aircraft() {
        let plan = AirPlan::build(
            &[way(1, "runway", &[(0, 0), (3000, 0)])],
            1.0,
            usize::MAX,
            f64::INFINITY,
            flat,
        );
        assert!(plan.is_empty());
        assert!(AirPlan::build(&[], 1.0, usize::MAX, f64::INFINITY, flat).is_empty());
    }

    #[test]
    fn a_stand_drawn_nose_first_or_as_a_node_still_finds_its_taxiway() {
        let mut elements = airfield(3000, 0, 70);
        // One drawn from the nose back to the taxilane, one that stops short of it, and
        // one that is only a node.
        elements.push(way(200, "parking_position", &[(1400, 420), (1400, 350)]));
        elements.push(way(201, "parking_position", &[(1600, 372), (1600, 420)]));
        let mut tags = HashMap::new();
        tags.insert("aeroway".to_string(), "parking_position".to_string());
        elements.push(ProcessedElement::Node(ProcessedNode {
            id: 77,
            tags,
            x: 1800,
            z: 420,
        }));
        let field = Field::read(&elements, 1.0);
        assert_eq!(field.stands.len(), 3);
        for stand in &field.stands {
            let (from, nose) = (stand.path[0], *stand.path.last().unwrap());
            assert!(field.taxis(from), "stand {} leaves a taxiway", stand.id);
            assert!((field.nodes[from].at.1 - 350.5).abs() < 0.01);
            assert!((field.nodes[nose].at.1 - 420.5).abs() < 0.01);
        }
    }

    #[test]
    fn runway_stretches_that_run_on_are_one_runway() {
        let elements = vec![
            way(5, "runway", &[(0, 0), (900, 0)]),
            way(4, "runway", &[(900, 0), (2000, 10)]),
            // Crossing it at a node of its own, another runway.
            way(6, "runway", &[(900, -800), (900, 0), (900, 900)]),
        ];
        let field = Field::read(&elements, 1.0);
        let mut lengths: Vec<i64> = field
            .strips
            .iter()
            .map(|s| s.len().round() as i64)
            .collect();
        lengths.sort_unstable();
        assert_eq!(lengths, vec![1700, 2000]);
        assert_eq!(field.strips[0].id, 4);
    }

    #[test]
    fn legs_are_covered_within_their_caps_and_end_at_a_stand() {
        let leg = |from: f64, to: f64, cap: f64| Leg {
            piece: 0,
            from,
            to,
            cap,
            accel: 0.01,
            brake: 0.012,
            holds: None,
        };
        let legs = [
            leg(0.0, 100.0, 1.0),
            leg(100.0, 130.0, 0.3),
            leg(130.0, 300.0, 1.0),
        ];
        let steps = run(&legs);
        assert_eq!(steps.last().unwrap().1, 300.0);
        let mut last = 0.0;
        for &(l, at) in &steps {
            let speed = at - last;
            assert!(
                speed > 0.0 && speed <= legs[l].cap + 0.02,
                "{speed} on leg {l}"
            );
            last = at;
        }
        // Backwards along a piece is covered the same.
        let back = run(&reversed(&legs, 0.2));
        assert_eq!(back.last().unwrap().1, 0.0);
        assert!(back.len() > 1400);
    }

    #[test]
    fn the_approach_comes_down_steeply_then_on_the_glide_and_flares() {
        let h = |d: f64| approach_height(d, 130.0, 1.0);
        assert_eq!(h(0.0), 0.0);
        assert!(h(30.0) < 30.0 * GLIDE_DEG.to_radians().tan());
        assert!((h(400.0) - (400.0 - 30.0) * GLIDE_DEG.to_radians().tan()).abs() < 1e-9);
        assert_eq!(h(5000.0), 130.0);
        // Never a step in it.
        for d in 0..3000 {
            let rise = h(f64::from(d + 1)) - h(f64::from(d));
            assert!((0.0..0.12).contains(&rise), "{rise} at {d}");
        }
    }

    /// The network's pack functions and Bedrock data for an airfield of small aircraft.
    fn network() -> crate::element_processing::trains::TrainNetwork {
        use crate::coordinate_system::cartesian::XZBBox;
        use crate::element_processing::trains::TrainNetwork;
        let world = XZBBox::rect_from_min_max(0, 0, 3000, 1000).unwrap();
        let mut net = TrainNetwork::build(&[], 1.0, &world);
        net.add_aircraft(planned(3000, 9, 40));
        net
    }

    #[test]
    fn the_pack_flies_the_aircraft_on_the_trains_functions() {
        let net = network();
        assert_eq!(net.train_count(), 0);
        let files = net.pack_functions(|_, _, _| None);
        let file = |end: &str| -> &String {
            &files
                .iter()
                .find(|(p, _)| p.ends_with(end))
                .unwrap_or_else(|| panic!("no {end}"))
                .1
        };
        let services: Vec<&(String, String)> = files
            .iter()
            .filter(|(p, _)| p.contains("/trains/t/"))
            .collect();
        assert!(services.len() >= 4);
        assert_eq!(
            file("trains/all.mcfunction").lines().count(),
            services.len()
        );
        // The airfield's points are stored once, each with its heading, pitches and wheels,
        // and every aircraft has a timetable of its own.
        let network = file("trains/network.mcfunction");
        assert_eq!(
            network
                .matches("storage arnis:trains r0 set value []")
                .count(),
            1
        );
        assert!(!network.contains("arnis:trains r1 "));
        assert!(network.contains(",q:") && network.contains(",p:") && network.contains(",g:1}"));
        for t in 0..services.len() {
            assert!(network.contains(&format!("storage arnis:trains s{t} set value [I;")));
        }
        // One that only stands is where its one entry says, without asking the clock.
        let standing: Vec<_> = services
            .iter()
            .filter(|(_, text)| !text.contains("function arnis:trains/when"))
            .collect();
        assert!(!standing.is_empty() && standing.len() < services.len());
        assert!(standing
            .iter()
            .all(|(_, text)| text.contains("scoreboard players set #s arnis.train ")));
        for (path, text) in &services {
            assert!(text.contains("function arnis:trains/air\n"), "{path}");
            assert!(!text.contains("function arnis:trains/car"), "{path}");
            assert!(text.contains("{r:0,t:"), "{path}");
            // It names its model for the gear functions, and a body and doors the pack has.
            let model = text
                .split(",m:\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap();
            assert!(model.starts_with("arnis:vehicle/plane_"), "{model}");
            let body = text
                .split(",b:\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap();
            assert!(file(&format!("trains/body/{body}.mcfunction")).contains("view_range:5f"));
            let door = text
                .split(",d:\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap();
            assert!(file(&format!("trains/door/{door}.mcfunction")).contains("arnis.ts$(c)"));
        }
    }

    #[test]
    fn bedrock_gets_the_airfield_once_with_its_pitches_and_wheels() {
        let net = network();
        let (data, bodies) = net.bedrock_data(|_, _, _| None);
        let routes = data["routes"].as_array().unwrap();
        assert_eq!(routes.len(), 1);
        let count = |v: &serde_json::Value| v.as_str().unwrap().split(',').count();
        assert_eq!(count(&routes[0]["air"]) * 4, count(&routes[0]["p"]) * 3);
        let services = data["services"].as_array().unwrap();
        assert!(services.len() >= 4);
        assert!(services.iter().all(|s| s["route"] == 0 && s["period"] == 0));
        assert!(!bodies.is_empty());
        assert!(bodies
            .iter()
            .all(|b| b.fleet == crate::bedrock_addon::Fleet::Plane
                && b.name.starts_with("train_plane_")));
    }

    #[test]
    fn times_on_a_round_clock_overlap_across_its_end() {
        assert!(overlap((10, 20), (15, 30), 100));
        assert!(!overlap((10, 20), (21, 30), 100));
        assert!(overlap((95, 105), (2, 4), 100));
        assert!(overlap((2, 4), (95, 105), 100));
        assert!(!overlap((95, 99), (2, 4), 100));
    }
}
