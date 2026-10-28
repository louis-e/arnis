//! Trains, metros and trams that run the mapped tracks, driven by the extras data pack.
//! The tracks are joined here into routes, each with its stops, and every train's round
//! trip is worked out in advance as a timetable: where its front carriage is at each step.
//! The pack reads the timetable and sets each carriage down on the route behind the
//! front, so a train keeps no state of its own, and summons the carriages that come into
//! view. The storage layout, the tags and the body functions are the contract with
//! `assets/minecraft/datapack_extras/data/arnis/function/trains`.
//!
//! Distances are in blocks here and in thousandths of a block in the pack; a step is the
//! two ticks between moves.

use crate::bedrock_addon::{self, Fleet, Lamp};
use crate::clipping::is_invented_node_id;
use crate::coordinate_system::cartesian::XZBBox;
use crate::datapack::macro_coord;
use crate::element_processing::train_bodies::{self, Coach, Livery};
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use fnv::FnvHashMap;
use serde_json::json;
use std::collections::HashMap;
use std::fmt::Write;

/// Moves a second: the pack moves trains every second tick.
const STEPS_PER_S: f64 = 10.0;
/// A switch leaves the line at a few degrees; turning further, a track is another line.
const MAX_DEFLECT_DEG: f64 = 40.0;
/// Headings at a node are taken this far along each track, in blocks.
const HEADING_REACH: f64 = 3.0;
/// A track ending this close to the edge of the area runs on outside it.
const EDGE_REACH: f64 = 3.0;
/// How long a train stays outside the area before it comes back, in seconds.
const LAYOVER_S: f64 = 25.0;
/// Most trains and carriages a world gets, whatever its rail network.
const MAX_TRAINS: usize = 160;
const MAX_CARRIAGES: usize = 1400;
/// Most route points the pack holds, about 45 bytes each in its storage.
const MAX_POINTS: usize = 250_000;
/// Longest timetable, so a train that cannot get round is dropped rather than run away.
const MAX_STEPS: usize = 72_000;
/// Gap between two carriages, in metres.
const COUPLING_M: f64 = 0.4;
/// How far a mapped stop reaches to a track, in metres: a platform edge, a stop position
/// on the track or beside it, a station's node somewhere near its tracks.
const PLATFORM_REACH_M: f64 = 9.0;
const STOP_REACH_M: f64 = 6.0;
const STATION_REACH_M: f64 = 90.0;
/// Stops closer than this along a track are one stop, in metres.
const STOP_MERGE_M: f64 = 80.0;
/// A terminus stop this close to the end of the track is where trains turn back.
const TERMINUS_STOP_M: f64 = 160.0;
/// Room a train leaves before a buffer stop or the junction it turns back at, in metres.
const END_MARGIN_M: f64 = 6.0;
/// Shortest track a train turns back on at a junction, in metres: anything less is a
/// curve between two lines, which no train shuttles along.
const JUNCTION_RUN_M: f64 = 400.0;
/// Slowest a train creeps up to where it stops, in blocks a step.
const CREEP: f64 = 0.03;
/// Points of a route list written per line of the pack.
const POINTS_PER_LINE: usize = 400;

/// What runs on a track.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
enum Mode {
    Tram,
    Metro,
    SBahn,
    Rail,
}

impl Mode {
    fn of(way: &ProcessedWay) -> Option<Self> {
        let tags = &way.tags;
        let get = |k: &str| tags.get(k).map(String::as_str);
        // Rendered as something else first, so no rails are laid there.
        if [
            "building", "highway", "landuse", "natural", "amenity", "leisure", "barrier",
            "waterway",
        ]
        .iter()
        .any(|k| tags.contains_key(*k))
            || get("area") == Some("yes")
            || get("disused") == Some("yes")
            || matches!(
                get("service"),
                Some("yard" | "siding" | "spur" | "crossover")
            )
            || matches!(get("usage"), Some("industrial" | "military" | "test"))
        {
            return None;
        }
        match get("railway")? {
            "tram" => Some(Self::Tram),
            "subway" => Some(Self::Metro),
            "light_rail" => Some(Self::SBahn),
            "rail" | "narrow_gauge" | "preserved" => Some(Self::Rail),
            _ => None,
        }
    }

    /// Top speed, km/h, where the track sets none.
    fn cruise_kmh(self) -> f64 {
        match self {
            Self::Tram => 45.0,
            Self::Metro => 65.0,
            Self::SBahn => 80.0,
            Self::Rail => 110.0,
        }
    }

    /// Starting, braking and the sideways pull it takes in a curve, m/s².
    fn accel(self) -> (f64, f64, f64) {
        match self {
            Self::Tram => (1.1, 1.2, 1.0),
            Self::Metro => (1.0, 1.0, 0.8),
            Self::SBahn => (0.9, 0.9, 0.7),
            Self::Rail => (0.6, 0.7, 0.6),
        }
    }

    /// Seconds standing at a stop and at the end of the line.
    fn dwell_s(self) -> (f64, f64) {
        match self {
            Self::Tram => (14.0, 30.0),
            Self::Metro => (18.0, 30.0),
            Self::SBahn => (22.0, 40.0),
            Self::Rail => (35.0, 60.0),
        }
    }

    /// Seconds between two trains on a line that goes round or through.
    fn headway_s(self) -> f64 {
        match self {
            Self::Tram => 150.0,
            Self::Metro => 150.0,
            Self::SBahn => 200.0,
            Self::Rail => 330.0,
        }
    }

    /// Whether a mapped stop serves this kind of track.
    fn served_by(self, stop: StopKind) -> bool {
        match stop {
            StopKind::Tram => self == Self::Tram,
            StopKind::Metro => self == Self::Metro,
            StopKind::Train => matches!(self, Self::SBahn | Self::Rail),
            StopKind::Any => self != Self::Tram,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StopKind {
    Tram,
    Metro,
    Train,
    Any,
}

/// How firmly a mapped feature says where a train stops.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum StopSource {
    Station,
    OnTrack,
    Platform,
}

struct StopFeature {
    kind: StopKind,
    source: StopSource,
    /// Points that reach a track: a node, or a platform's outline.
    at: Vec<(f64, f64)>,
    /// Where on a track it reaches the train stops level with this.
    centre: (f64, f64),
}

/// How a track ends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum End {
    /// At the edge of the area: the line goes on outside it.
    Edge,
    /// At a buffer stop.
    Buffer,
    /// Where it meets a track another route already runs on.
    Junction,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shape {
    /// Closed on itself, as a ring line or a tram loop.
    Loop,
    /// In at one edge of the area and out at another.
    Through,
    /// Back and forth, turning at its ends.
    Shuttle,
}

/// A piece of track between two junctions.
struct Piece {
    way: u64,
    mode: Mode,
    maxspeed: Option<f64>,
    nodes: Vec<(u64, f64, f64)>,
}

/// A route, a block between points.
struct Route {
    mode: Mode,
    shape: Shape,
    ends: [End; 2],
    points: Vec<(f64, f64)>,
    /// The way each point lies on.
    ways: Vec<u64>,
    /// Speed limit at each point, blocks a step.
    caps: Vec<f64>,
    /// Where the middle of a train stands at each stop, as an index into `points`.
    stops: Vec<f64>,
}

impl Route {
    fn len(&self) -> f64 {
        match self.shape {
            Shape::Loop => self.points.len() as f64,
            _ => self.points.len().saturating_sub(1) as f64,
        }
    }
}

/// What a route runs, and how.
struct Service {
    route: usize,
    /// Carriage bodies front to back, and how far behind the front carriage each centre is.
    coaches: Vec<Coach>,
    offsets: Vec<f64>,
    /// Front carriage centre at each step, in thousandths of a block along the route.
    timetable: Vec<i32>,
    /// Length of one lap in thousandths of a block, through the stretch outside the area
    /// on a through route; 0 for a shuttle.
    period: i64,
    /// The steps into the timetable each train starts at, and its livery.
    trains: Vec<(usize, Livery)>,
}

impl Service {
    /// How far along its route a train is still in the area, and the lap it wraps by, both
    /// in thousandths of a block.
    fn reach(&self, route: &Route) -> (i64, i64) {
        let len_mm = (route.len() * 1000.0).round() as i64;
        match route.shape {
            Shape::Loop => (len_mm - 1, len_mm),
            Shape::Through => (len_mm, self.period),
            Shape::Shuttle => (len_mm, 0),
        }
    }

    /// How far behind the front carriage centre the middle of the train is, in blocks.
    fn middle(&self) -> f64 {
        self.offsets.last().copied().unwrap_or(0.0) / 2.0
    }
}

pub struct TrainNetwork {
    size: f64,
    routes: Vec<Route>,
    services: Vec<Service>,
}

fn hash(a: u64, b: u64) -> u64 {
    let mut h = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^ (h >> 32)
}

fn unit(dx: f64, dz: f64) -> Option<(f64, f64)> {
    let len = dx.hypot(dz);
    (len > 1e-9).then(|| (dx / len, dz / len))
}

fn angle_deg(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 * b.0 + a.1 * b.1).clamp(-1.0, 1.0).acos().to_degrees()
}

/// Most blocks a train moves in a step: under the 8 past its end the pack still clears a
/// carriage at, and the 12 it looks for one within.
const MAX_STEP: f64 = 7.9;

/// Blocks a step at `kmh` on a world of `scale` blocks a metre.
fn step_speed(kmh: f64, scale: f64) -> f64 {
    (kmh / 3.6 / STEPS_PER_S * scale).min(MAX_STEP)
}

/// `maxspeed` in km/h, from a number or a number in mph.
fn parse_kmh(raw: &str) -> Option<f64> {
    let v = raw.trim().to_ascii_lowercase();
    if let Some(mph) = v.strip_suffix("mph") {
        return mph.trim().parse::<f64>().ok().map(|m| m * 1.609);
    }
    v.parse::<f64>().ok().filter(|&k| k > 0.0)
}

impl TrainNetwork {
    pub fn build(elements: &[ProcessedElement], scale: f64, world: &XZBBox) -> Self {
        let size = scale.clamp(0.4, 3.0);
        let pieces = pieces(elements);
        let mut routes = routes(&pieces, world, scale);
        let features = stop_features(elements);
        for route in &mut routes {
            route.stops = stops_on(route, &features, scale);
        }
        keep_right(&mut routes, scale);

        let mut services = Vec::new();
        let (mut trains, mut carriages, mut points) = (0usize, 0usize, 0usize);
        for (r, route) in routes.iter().enumerate() {
            if trains >= MAX_TRAINS || carriages >= MAX_CARRIAGES {
                break;
            }
            if points + route.points.len() > MAX_POINTS {
                continue;
            }
            let Some(mut service) = service_for(r, route, scale, size) else {
                continue;
            };
            let room =
                (MAX_TRAINS - trains).min((MAX_CARRIAGES - carriages) / service.coaches.len());
            service.trains.truncate(room);
            if service.trains.is_empty() {
                continue;
            }
            trains += service.trains.len();
            carriages += service.trains.len() * service.coaches.len();
            points += route.points.len();
            services.push(service);
        }
        Self {
            size,
            routes,
            services,
        }
    }

    pub fn train_count(&self) -> usize {
        self.services.iter().map(|s| s.trains.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.services.is_empty()
    }

    /// The pack functions: the routes and timetables in storage, one function per train
    /// and the one that runs them all, and the carriage bodies. `rail_y` gives the block a
    /// rail lies in on a way's track, where one was laid.
    pub fn pack_functions(
        &self,
        rail_y: impl Fn(u64, i32, i32) -> Option<i32>,
    ) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut network = String::from("data modify storage arnis:trains tmp set value []\n");
        let mut all = String::new();
        let mut bodies: FnvHashMap<String, Vec<train_bodies::Part>> = FnvHashMap::default();
        let mut carriage = 0usize;
        for (t, service) in self.services.iter().enumerate() {
            let route = &self.routes[service.route];
            let Some(ys) = heights(route, &rail_y) else {
                continue;
            };
            let r = t;
            write_points(&mut network, r, route, &ys);
            let _ = writeln!(
                network,
                "data modify storage arnis:trains s{r} set value [I;{}]",
                service
                    .timetable
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let (visible, period) = service.reach(route);
            let middle = service.middle();
            for (k, &(phase, livery)) in service.trains.iter().enumerate() {
                let name = format!("{r}_{k}");
                let mut text = format!(
                    "# Train {k} on route {r}: where its timetable says it is now.\n\
                     scoreboard players operation #i arnis.train = #now arnis.train\n\
                     scoreboard players add #i arnis.train {phase}\n\
                     scoreboard players set #T arnis.train {}\n\
                     scoreboard players operation #i arnis.train %= #T arnis.train\n\
                     data modify storage arnis:trains a set value {{r:{r}}}\n\
                     execute store result storage arnis:trains a.i int 1 run scoreboard players get #i arnis.train\n\
                     function arnis:trains/when with storage arnis:trains a\n\
                     scoreboard players set #L arnis.train {visible}\n\
                     scoreboard players set #P arnis.train {period}\n\
                     # Nothing to draw while the middle of the train is out of loaded chunks.\n\
                     scoreboard players operation #sk arnis.train = #s arnis.train\n\
                     scoreboard players remove #sk arnis.train {}\n\
                     function arnis:trains/near\n\
                     execute unless score #go arnis.train matches 1 run return 0\n",
                    service.timetable.len(),
                    (middle * 1000.0).round() as i64,
                );
                for (c, (&coach, &offset)) in
                    service.coaches.iter().zip(&service.offsets).enumerate()
                {
                    let parts = train_bodies::parts(coach, livery);
                    let key = train_bodies::key(coach, livery);
                    bodies.entry(key.clone()).or_insert(parts);
                    let _ = write!(
                        text,
                        "scoreboard players operation #sk arnis.train = #s arnis.train\n\
                         scoreboard players remove #sk arnis.train {}\n\
                         data modify storage arnis:trains c set value {{r:{r},c:{},b:\"{key}\"}}\n\
                         function arnis:trains/car\n",
                        (offset * 1000.0).round() as i64,
                        carriage + c,
                    );
                }
                carriage += service.coaches.len();
                let _ = writeln!(all, "function arnis:trains/t/{name}");
                out.push((
                    format!("data/arnis/function/trains/t/{name}.mcfunction"),
                    text,
                ));
            }
        }
        network.push_str("data remove storage arnis:trains tmp\n");
        out.push((
            "data/arnis/function/trains/network.mcfunction".to_string(),
            network,
        ));
        out.push(("data/arnis/function/trains/all.mcfunction".to_string(), all));
        let mut keys: Vec<&String> = bodies.keys().collect();
        keys.sort();
        for key in keys {
            for (prefix, bare) in [("", false), ("ov121/", true)] {
                out.push((
                    format!("{prefix}data/arnis/function/trains/body/{key}.mcfunction"),
                    train_bodies::summon_function(&bodies[key], self.size, bare),
                ));
            }
        }
        out
    }

    /// What the Bedrock script runs the trains by, the same routes and timetables the pack
    /// gets: each route's points in hundredths of a block (heights in thousandths, headings
    /// in tenths of a degree) each from the last, and per service its timetable as run
    /// lengths, both flat lists, its reach and its trains, each carriage as (id, body,
    /// offset). Also the carriage bodies, which the carriages name by their place in the list.
    pub fn bedrock_data(
        &self,
        rail_y: impl Fn(u64, i32, i32) -> Option<i32>,
    ) -> (serde_json::Value, Vec<bedrock_addon::Body>) {
        let mut routes = Vec::new();
        let mut services = Vec::new();
        let mut bodies: Vec<bedrock_addon::Body> = Vec::new();
        let mut body_of: FnvHashMap<String, usize> = FnvHashMap::default();
        let mut carriage = 0usize;
        let size = self.size as f32;
        for service in &self.services {
            let route = &self.routes[service.route];
            let Some(ys) = heights(route, &rail_y) else {
                continue;
            };
            let mut p = Vec::with_capacity(route.points.len() * 4);
            let mut last = [0i64; 4];
            for (i, &(x, z)) in route.points.iter().enumerate() {
                // On the rail, which is a sixteenth of a block deep.
                let at = [
                    (x * 100.0).round() as i64,
                    ((ys[i] + 0.0625) * 1000.0).round() as i64,
                    (z * 100.0).round() as i64,
                    (heading(route, i) * 10.0).round() as i64,
                ];
                p.extend((0..4).map(|k| at[k] - last[k]));
                last = at;
            }
            let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
            for &(x, z) in &route.points {
                lo = (lo.0.min(x), lo.1.min(z));
                hi = (hi.0.max(x), hi.1.max(z));
            }
            routes.push(json!({
                "p": bedrock_addon::list(p),
                "loop": route.shape == Shape::Loop,
                "box": [lo.0.floor(), lo.1.floor(), hi.0.ceil(), hi.1.ceil()],
            }));
            let (visible, period) = service.reach(route);
            let trains: Vec<serde_json::Value> = service
                .trains
                .iter()
                .map(|&(phase, livery)| {
                    let cars: Vec<[i64; 3]> = service
                        .coaches
                        .iter()
                        .zip(&service.offsets)
                        .enumerate()
                        .map(|(c, (&coach, &offset))| {
                            let name = format!("train_{}", train_bodies::key(coach, livery));
                            let body = *body_of.entry(name.clone()).or_insert_with(|| {
                                bodies.push(bedrock_addon::Body {
                                    name,
                                    fleet: Fleet::Train,
                                    shapes: train_bodies::parts(coach, livery)
                                        .into_iter()
                                        .map(|p| bedrock_addon::Shape {
                                            block: p.block,
                                            size: p.size.map(|v| v * size),
                                            corner: p.corner.map(|v| v * size),
                                            tilt: p.tilt,
                                            lamp: if p.glow { Lamp::On } else { Lamp::Off },
                                        })
                                        .collect(),
                                });
                                bodies.len() - 1
                            });
                            [
                                (carriage + c) as i64,
                                body as i64,
                                (offset * 1000.0).round() as i64,
                            ]
                        })
                        .collect();
                    carriage += service.coaches.len();
                    json!({ "phase": phase, "cars": cars })
                })
                .collect();
            services.push(json!({
                "route": routes.len() - 1,
                "steps": bedrock_addon::list(run_lengths(&service.timetable)),
                "count": service.timetable.len(),
                "visible": visible,
                "period": period,
                "middle": (service.middle() * 1000.0).round() as i64,
                "trains": trains,
            }));
        }
        let names: Vec<&str> = bodies.iter().map(|b| b.name.as_str()).collect();
        let data = json!({ "routes": routes, "services": services, "bodies": names });
        (data, bodies)
    }
}

/// The track heights along a route: each point on the rail its way laid nearest it,
/// smoothed so a train rides a slope rather than its steps. None where most of the route
/// has no rail to stand on.
fn heights(route: &Route, rail_y: &impl Fn(u64, i32, i32) -> Option<i32>) -> Option<Vec<f64>> {
    let raw: Vec<Option<f64>> = route
        .points
        .iter()
        .zip(&route.ways)
        .map(|(&(x, z), &way)| {
            let (bx, bz) = (x.floor() as i32, z.floor() as i32);
            let mut best: Option<(i32, i32)> = None;
            for dx in -1..=1 {
                for dz in -1..=1 {
                    if let Some(y) = rail_y(way, bx + dx, bz + dz) {
                        let d = dx * dx + dz * dz;
                        if best.is_none_or(|(b, _)| d < b) {
                            best = Some((d, y));
                        }
                    }
                }
            }
            best.map(|(_, y)| y as f64)
        })
        .collect();
    let known = raw.iter().filter(|y| y.is_some()).count();
    if known * 2 < raw.len() {
        return None;
    }
    // Fill the gaps from the nearest known height on either side.
    let mut filled = raw.clone();
    let mut last: Option<f64> = None;
    for y in filled.iter_mut() {
        match y {
            Some(v) => last = Some(*v),
            None => *y = last,
        }
    }
    let mut next: Option<f64> = None;
    for (y, r) in filled.iter_mut().zip(&raw).rev() {
        match r {
            Some(v) => next = Some(*v),
            None if y.is_none() => *y = next,
            None => {}
        }
    }
    let ys: Vec<f64> = filled.into_iter().map(|y| y.unwrap_or(0.0)).collect();
    // A rail climbs a block in one, so a moving average over a few points is the slope.
    const REACH: usize = 3;
    let n = ys.len();
    let looped = route.shape == Shape::Loop;
    Some(
        (0..n)
            .map(|i| {
                let mut sum = 0.0;
                let mut count = 0.0;
                for d in -(REACH as isize)..=REACH as isize {
                    let j = i as isize + d;
                    let j = if looped {
                        j.rem_euclid(n as isize) as usize
                    } else if j < 0 || j >= n as isize {
                        continue;
                    } else {
                        j as usize
                    };
                    sum += ys[j];
                    count += 1.0;
                }
                sum / count
            })
            .collect(),
    )
}

/// The yaw the route runs at from point `i`.
fn heading(route: &Route, i: usize) -> f64 {
    let n = route.points.len();
    let (a, b) = if route.shape == Shape::Loop {
        (route.points[i], route.points[(i + 1) % n])
    } else if i + 1 < n {
        (route.points[i], route.points[i + 1])
    } else {
        (route.points[i.saturating_sub(1)], route.points[i])
    };
    (-(b.0 - a.0)).atan2(b.1 - a.1).to_degrees()
}

/// A timetable as its first value and then (step, repeats) pairs: a train covers the same
/// distance every step for long stretches, cruising or standing.
fn run_lengths(values: &[i32]) -> Vec<i64> {
    let Some(&first) = values.first() else {
        return Vec::new();
    };
    let mut out = vec![first as i64];
    let mut k = 1;
    while k < values.len() {
        let step = values[k] as i64 - values[k - 1] as i64;
        let mut n = 1;
        while k + n < values.len() && values[k + n] as i64 - values[k + n - 1] as i64 == step {
            n += 1;
        }
        out.extend([step, n as i64]);
        k += n;
    }
    out
}

/// Writes a route's points into storage as `r<r>`, a list of `{x,y,z,h}` with the heading
/// a yaw, in lines of a few hundred.
fn write_points(out: &mut String, r: usize, route: &Route, ys: &[f64]) {
    let n = route.points.len();
    let point = |i: usize| {
        let (x, z) = route.points[i];
        // On the rail, which is a sixteenth of a block deep.
        format!(
            "{{x:{:.2}d,y:{:.3}d,z:{:.2}d,h:{:.1}f}}",
            macro_coord(x),
            ys[i] + 0.0625,
            macro_coord(z),
            heading(route, i)
        )
    };
    let _ = writeln!(out, "data modify storage arnis:trains r{r} set value []");
    for chunk in (0..n).collect::<Vec<_>>().chunks(POINTS_PER_LINE) {
        let list: Vec<String> = chunk.iter().map(|&i| point(i)).collect();
        let _ = writeln!(
            out,
            "data modify storage arnis:trains tmp set value [{}]\n\
             data modify storage arnis:trains r{r} append from storage arnis:trains tmp[]",
            list.join(",")
        );
    }
}

/// The tracks trains run on, split where they meet.
fn pieces(elements: &[ProcessedElement]) -> Vec<Piece> {
    let ways: Vec<(&ProcessedWay, Mode)> = elements
        .iter()
        .filter_map(|e| match e {
            ProcessedElement::Way(w) if w.nodes.len() >= 2 => Mode::of(w).map(|m| (w, m)),
            _ => None,
        })
        .collect();
    let mut uses: FnvHashMap<u64, usize> = FnvHashMap::default();
    for (way, _) in &ways {
        for n in &way.nodes {
            *uses.entry(n.id).or_default() += 1;
        }
    }
    let mut out = Vec::new();
    for (way, mode) in ways {
        let maxspeed = way.tags.get("maxspeed").and_then(|m| parse_kmh(m));
        let mut nodes: Vec<(u64, f64, f64)> = Vec::new();
        for (k, n) in way.nodes.iter().enumerate() {
            if nodes.last().is_some_and(|l| l.0 == n.id) {
                continue;
            }
            nodes.push((n.id, n.x as f64 + 0.5, n.z as f64 + 0.5));
            let inner = k > 0 && k + 1 < way.nodes.len();
            if inner && uses[&n.id] > 1 && nodes.len() >= 2 {
                out.push(Piece {
                    way: way.id,
                    mode,
                    maxspeed,
                    nodes: std::mem::take(&mut nodes),
                });
                nodes.push((n.id, n.x as f64 + 0.5, n.z as f64 + 0.5));
            }
        }
        if nodes.len() >= 2 {
            out.push(Piece {
                way: way.id,
                mode,
                maxspeed,
                nodes,
            });
        }
    }
    out
}

/// Heading out of a piece's start (or, `from_end`, out of its end), taken a few blocks in.
fn heading_out(piece: &Piece, from_end: bool) -> Option<(f64, f64)> {
    let nodes: Vec<&(u64, f64, f64)> = if from_end {
        piece.nodes.iter().rev().collect()
    } else {
        piece.nodes.iter().collect()
    };
    let a = nodes[0];
    let far = nodes
        .iter()
        .skip(1)
        .find(|n| (n.1 - a.1).hypot(n.2 - a.2) >= HEADING_REACH)
        .or(nodes.last())?;
    unit(far.1 - a.1, far.2 - a.2)
}

type Step = (usize, bool);

/// Where a run of pieces ends, and the heading it arrives there with.
fn arrival(pieces: &[Piece], step: Step) -> (u64, (f64, f64)) {
    let (p, forward) = step;
    let piece = &pieces[p];
    let node = if forward {
        piece.nodes.last().unwrap().0
    } else {
        piece.nodes[0].0
    };
    let out = heading_out(piece, forward).unwrap_or((1.0, 0.0));
    (node, (-out.0, -out.1))
}

/// Carries a run on from `node`, arriving with heading `inc`, along the straightest
/// unused piece of the same kind until none is straight enough.
fn extend(
    pieces: &[Piece],
    at: &FnvHashMap<u64, Vec<Step>>,
    used: &mut [bool],
    mut node: u64,
    mut inc: (f64, f64),
    mode: Mode,
) -> Vec<Step> {
    let mut out = Vec::new();
    loop {
        let best = at
            .get(&node)
            .into_iter()
            .flatten()
            .filter(|&&(q, _)| !used[q] && pieces[q].mode == mode)
            .filter_map(|&(q, starts_here)| {
                let out = heading_out(&pieces[q], !starts_here)?;
                let turn = angle_deg(inc, out);
                (turn <= MAX_DEFLECT_DEG).then_some((q, starts_here, turn))
            })
            .min_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)));
        let Some((q, forward, _)) = best else {
            break;
        };
        used[q] = true;
        out.push((q, forward));
        (node, inc) = arrival(pieces, (q, forward));
    }
    out
}

/// Joins the pieces into routes through every junction the straight way, then lays each
/// out a block between points.
fn routes(pieces: &[Piece], world: &XZBBox, scale: f64) -> Vec<Route> {
    let mut at: FnvHashMap<u64, Vec<Step>> = FnvHashMap::default();
    for (p, piece) in pieces.iter().enumerate() {
        at.entry(piece.nodes[0].0).or_default().push((p, true));
        at.entry(piece.nodes.last().unwrap().0)
            .or_default()
            .push((p, false));
    }
    let position: FnvHashMap<u64, (f64, f64)> = pieces
        .iter()
        .flat_map(|p| p.nodes.iter().map(|&(id, x, z)| (id, (x, z))))
        .collect();
    let at_edge = |node: u64| {
        let (x, z) = position[&node];
        let near = |v: f64, lo: i32, hi: i32| {
            v < lo as f64 + EDGE_REACH || v > (hi + 1) as f64 - EDGE_REACH
        };
        is_invented_node_id(node)
            || near(x, world.min_x(), world.max_x())
            || near(z, world.min_z(), world.max_z())
    };
    let end_of = |node: u64| {
        if at_edge(node) {
            End::Edge
        } else if at.get(&node).map_or(0, Vec::len) <= 1 {
            End::Buffer
        } else {
            End::Junction
        }
    };

    let mut used = vec![false; pieces.len()];
    let mut runs: Vec<Vec<Step>> = Vec::new();
    // Lines are followed in from where they leave the area, then from their buffers.
    let mut starts: Vec<(bool, u64)> = at
        .iter()
        .filter(|(_, steps)| steps.len() == 1)
        .map(|(&node, _)| (!at_edge(node), node))
        .collect();
    starts.sort_unstable();
    for (_, node) in starts {
        let &(p, forward) = &at[&node][0];
        if used[p] {
            continue;
        }
        used[p] = true;
        let mut run = vec![(p, forward)];
        let (end, inc) = arrival(pieces, (p, forward));
        run.extend(extend(pieces, &at, &mut used, end, inc, pieces[p].mode));
        runs.push(run);
    }
    // What is left closes on itself or runs between junctions: follow it both ways.
    let mut rest: Vec<usize> = (0..pieces.len()).filter(|&p| !used[p]).collect();
    rest.sort_by_key(|&p| (pieces[p].way, p));
    for p in rest {
        if used[p] {
            continue;
        }
        used[p] = true;
        let mode = pieces[p].mode;
        let (end, inc) = arrival(pieces, (p, true));
        let ahead = extend(pieces, &at, &mut used, end, inc, mode);
        let (start, back) = arrival(pieces, (p, false));
        let behind = extend(pieces, &at, &mut used, start, back, mode);
        let mut run: Vec<Step> = behind.into_iter().rev().map(|(q, f)| (q, !f)).collect();
        run.push((p, true));
        run.extend(ahead);
        runs.push(run);
    }

    runs.into_iter()
        .filter_map(|run| lay_out(pieces, &run, &end_of, scale))
        .collect()
}

/// A run's nodes in order, a point a block apart along them, and how it ends.
fn lay_out(
    pieces: &[Piece],
    run: &[Step],
    end_of: &impl Fn(u64) -> End,
    scale: f64,
) -> Option<Route> {
    let mode = pieces[run[0].0].mode;
    let mut line: Vec<(f64, f64, u64)> = Vec::new();
    let mut limits: Vec<Option<f64>> = Vec::new();
    for &(p, forward) in run {
        let piece = &pieces[p];
        let nodes: Vec<&(u64, f64, f64)> = if forward {
            piece.nodes.iter().collect()
        } else {
            piece.nodes.iter().rev().collect()
        };
        for (k, n) in nodes.iter().enumerate() {
            // The stretch from a joint on is this piece's, for its heights and its limit.
            if k == 0 && !line.is_empty() {
                if let (Some(joint), Some(limit)) = (line.last_mut(), limits.last_mut()) {
                    joint.2 = piece.way;
                    *limit = piece.maxspeed;
                }
                continue;
            }
            line.push((n.1, n.2, piece.way));
            limits.push(piece.maxspeed);
        }
    }
    let first = if run[0].1 {
        pieces[run[0].0].nodes[0].0
    } else {
        pieces[run[0].0].nodes.last().unwrap().0
    };
    let (last, arrive) = arrival(pieces, *run.last().unwrap());
    let closed = first == last
        && line.len() > 3
        && heading_out(&pieces[run[0].0], !run[0].1)
            .is_some_and(|out| angle_deg(arrive, out) <= MAX_DEFLECT_DEG);
    let shape;
    let ends;
    if closed {
        line.pop();
        limits.pop();
        shape = Shape::Loop;
        ends = [End::Junction, End::Junction];
    } else {
        ends = [end_of(first), end_of(last)];
        shape = if ends == [End::Edge, End::Edge] {
            Shape::Through
        } else {
            Shape::Shuttle
        };
    }
    if line.len() < 2 {
        return None;
    }

    // Lengths of the stretches, closing the loop back to its start.
    let segs = if closed { line.len() } else { line.len() - 1 };
    let seg = |i: usize| (line[i], line[(i + 1) % line.len()]);
    let total: f64 = (0..segs)
        .map(|i| {
            let (a, b) = seg(i);
            (b.0 - a.0).hypot(b.1 - a.1)
        })
        .sum();
    let count = if closed {
        total.round() as usize
    } else {
        total.floor() as usize + 1
    };
    if count < 8 {
        return None;
    }
    let spacing = if closed { total / count as f64 } else { 1.0 };
    let (mut points, mut ways, mut limit) = (Vec::new(), Vec::new(), Vec::new());
    let (mut i, mut start) = (0usize, 0.0f64);
    for k in 0..count {
        let s = k as f64 * spacing;
        loop {
            let (a, b) = seg(i);
            let len = (b.0 - a.0).hypot(b.1 - a.1);
            if s <= start + len || i + 1 >= segs {
                let t = if len > 0.0 {
                    ((s - start) / len).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                points.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
                ways.push(a.2);
                limit.push(limits[i]);
                break;
            }
            start += len;
            i += 1;
        }
    }

    // Rounds off each node a little, so carriages turn through it rather than at it.
    let n = points.len();
    if n > 4 {
        let raw = points.clone();
        for (i, point) in points.iter_mut().enumerate() {
            if !closed && (i < 2 || i + 2 >= n) {
                continue;
            }
            let (mut x, mut z) = (0.0, 0.0);
            for d in -2..=2isize {
                let q = raw[(i as isize + d).rem_euclid(n as isize) as usize];
                x += q.0;
                z += q.1;
            }
            *point = (x / 5.0, z / 5.0);
        }
    }

    // A curve holds a train to what it takes sideways. Mapped track turns at its nodes, so
    // the radius is the turning summed over a stretch longer than the gap between them.
    let (_, _, lateral) = mode.accel();
    let window_m = if mode == Mode::Tram { 10.0 } else { 20.0 };
    let reach = ((window_m * scale).round() as isize).clamp(3, 40);
    let heading = |i: usize| -> Option<(f64, f64)> {
        let j = if closed { (i + 1) % n } else { i + 1 };
        if j < n {
            unit(points[j].0 - points[i].0, points[j].1 - points[i].1)
        } else {
            None
        }
    };
    let turns: Vec<f64> = (0..n)
        .map(|i| {
            let prev = match i {
                0 if closed => heading(n - 1),
                0 => None,
                _ => heading(i - 1),
            };
            match (prev, heading(i)) {
                (Some(u), Some(v)) => (u.0 * v.1 - u.1 * v.0).atan2(u.0 * v.0 + u.1 * v.1),
                _ => 0.0,
            }
        })
        .collect();
    let caps = (0..n)
        .map(|i| {
            let mut kmh = mode.cruise_kmh().min(limit[i].unwrap_or(f64::MAX));
            let mut turn = 0.0f64;
            for d in -reach..=reach {
                let j = i as isize + d;
                if closed {
                    turn += turns[j.rem_euclid(n as isize) as usize];
                } else if j >= 0 && (j as usize) < n {
                    turn += turns[j as usize];
                }
            }
            if turn.abs() > 1e-3 {
                let radius_m = (2 * reach + 1) as f64 / turn.abs() / scale;
                kmh = kmh.min(((lateral * radius_m).sqrt() * 3.6).max(12.0));
            }
            step_speed(kmh, scale)
        })
        .collect();

    Some(Route {
        mode,
        shape,
        ends,
        points,
        ways,
        caps,
        stops: Vec::new(),
    })
}

/// The mapped stops, stations and platforms.
fn stop_features(elements: &[ProcessedElement]) -> Vec<StopFeature> {
    let mut out = Vec::new();
    let yes =
        |tags: &HashMap<String, String>, k: &str| tags.get(k).map(String::as_str) == Some("yes");
    let kind_of = |tags: &HashMap<String, String>| {
        if yes(tags, "tram") || tags.get("railway").map(String::as_str) == Some("tram_stop") {
            StopKind::Tram
        } else if yes(tags, "subway") || tags.get("station").map(String::as_str) == Some("subway") {
            StopKind::Metro
        } else if yes(tags, "train") || yes(tags, "light_rail") {
            StopKind::Train
        } else {
            StopKind::Any
        }
    };
    for element in elements {
        match element {
            ProcessedElement::Node(node) => {
                let tags = &node.tags;
                let railway = tags.get("railway").map(String::as_str);
                let pt = tags.get("public_transport").map(String::as_str);
                let source = match (railway, pt) {
                    (Some("stop" | "tram_stop"), _) | (_, Some("stop_position")) => {
                        StopSource::OnTrack
                    }
                    (Some("station" | "halt"), _) => StopSource::Station,
                    (_, Some("station")) if tags.contains_key("railway") || yes(tags, "train") => {
                        StopSource::Station
                    }
                    _ => continue,
                };
                if pt == Some("stop_position")
                    && kind_of(tags) == StopKind::Any
                    && railway.is_none()
                {
                    // A bus stop position.
                    continue;
                }
                let at = (node.x as f64 + 0.5, node.z as f64 + 0.5);
                out.push(StopFeature {
                    kind: kind_of(tags),
                    source,
                    at: vec![at],
                    centre: at,
                });
            }
            ProcessedElement::Way(way) => {
                let tags = &way.tags;
                let platform = tags.get("railway").map(String::as_str) == Some("platform")
                    || (tags.get("public_transport").map(String::as_str) == Some("platform")
                        && (yes(tags, "train")
                            || yes(tags, "subway")
                            || yes(tags, "light_rail")
                            || yes(tags, "tram")));
                if !platform || way.nodes.is_empty() {
                    continue;
                }
                let at: Vec<(f64, f64)> = way
                    .nodes
                    .iter()
                    .map(|n: &ProcessedNode| (n.x as f64 + 0.5, n.z as f64 + 0.5))
                    .collect();
                let centre = (
                    at.iter().map(|p| p.0).sum::<f64>() / at.len() as f64,
                    at.iter().map(|p| p.1).sum::<f64>() / at.len() as f64,
                );
                out.push(StopFeature {
                    kind: kind_of(tags),
                    source: StopSource::Platform,
                    at,
                    centre,
                });
            }
            ProcessedElement::Relation(_) => {}
        }
    }
    out
}

/// Where trains on a route stop, as points along it: each stop serving its kind of track
/// that reaches it, one per stretch however many features mark it.
fn stops_on(route: &Route, features: &[StopFeature], scale: f64) -> Vec<f64> {
    // A coarse grid of the route's points, so each feature looks at its neighbourhood.
    const CELL: f64 = 32.0;
    let key = |x: f64, z: f64| ((x / CELL).floor() as i32, (z / CELL).floor() as i32);
    let mut grid: FnvHashMap<(i32, i32), Vec<usize>> = FnvHashMap::default();
    for (i, &(x, z)) in route.points.iter().enumerate() {
        grid.entry(key(x, z)).or_default().push(i);
    }
    let nearest = |p: (f64, f64), reach: f64| -> Option<(usize, f64)> {
        let span = (reach / CELL).ceil() as i32;
        let (cx, cz) = key(p.0, p.1);
        let mut best: Option<(usize, f64)> = None;
        for dx in -span..=span {
            for dz in -span..=span {
                for &i in grid.get(&(cx + dx, cz + dz)).into_iter().flatten() {
                    let q = route.points[i];
                    let d = (q.0 - p.0).hypot(q.1 - p.1);
                    if d <= reach && best.is_none_or(|(_, b)| d < b) {
                        best = Some((i, d));
                    }
                }
            }
        }
        best
    };
    let mut found: Vec<(f64, StopSource)> = Vec::new();
    for f in features {
        if !route.mode.served_by(f.kind) {
            continue;
        }
        let reach = match f.source {
            StopSource::Platform => PLATFORM_REACH_M,
            StopSource::OnTrack => STOP_REACH_M,
            StopSource::Station => STATION_REACH_M,
        } * scale;
        // A platform runs beside the track; its middle says where the train stands.
        let touches = f.at.iter().any(|&p| nearest(p, reach).is_some());
        if !touches {
            continue;
        }
        if let Some((i, _)) = nearest(f.centre, reach.max(PLATFORM_REACH_M * scale * 4.0)) {
            found.push((i as f64, f.source));
        }
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    let merge = STOP_MERGE_M * scale;
    let mut stops: Vec<(f64, StopSource)> = Vec::new();
    for (s, source) in found {
        match stops.last_mut() {
            Some(last) if s - last.0 < merge => {
                if source > last.1 {
                    *last = (s, source);
                }
            }
            _ => stops.push((s, source)),
        }
    }
    // A loop's first and last stop may be the same one, seen from either side of its start.
    if route.shape == Shape::Loop && stops.len() > 1 {
        let n = route.points.len() as f64;
        let (first, last) = (stops[0], stops[stops.len() - 1]);
        if first.0 + n - last.0 < merge {
            stops.pop();
        }
    }
    stops.into_iter().map(|(s, _)| s).collect()
}

/// Turns each line that runs one way round so its trains keep to the right of the track
/// beside it going the other way.
fn keep_right(routes: &mut [Route], scale: f64) {
    let (near, far) = (2.0 * scale, 8.0 * scale);
    // Every point of a route that runs one way round, in cells as wide as the reach.
    let cell = far.max(1.0);
    let key = |x: f64, z: f64| ((x / cell).floor() as i32, (z / cell).floor() as i32);
    let mut grid: FnvHashMap<(i32, i32), Vec<(usize, usize)>> = FnvHashMap::default();
    for (r, route) in routes.iter().enumerate() {
        if route.shape == Shape::Shuttle {
            continue;
        }
        for (i, &(x, z)) in route.points.iter().enumerate() {
            grid.entry(key(x, z)).or_default().push((r, i));
        }
    }
    let flips: Vec<bool> = (0..routes.len())
        .map(|a| {
            let route = &routes[a];
            if route.shape == Shape::Shuttle {
                return false;
            }
            let (mut left, mut right) = (0usize, 0usize);
            let n = route.points.len();
            for i in (0..n.saturating_sub(1)).step_by((n / 32).max(1)) {
                let p = route.points[i];
                let Some(d) = unit(route.points[i + 1].0 - p.0, route.points[i + 1].1 - p.1) else {
                    continue;
                };
                // The nearest point of each other route of the kind within reach.
                let mut nearest: FnvHashMap<usize, (f64, (f64, f64))> = FnvHashMap::default();
                let (cx, cz) = key(p.0, p.1);
                for dx in -1..=1 {
                    for dz in -1..=1 {
                        for &(b, j) in grid.get(&(cx + dx, cz + dz)).into_iter().flatten() {
                            if b == a || routes[b].mode != route.mode {
                                continue;
                            }
                            let q = routes[b].points[j];
                            let dist = (q.0 - p.0).hypot(q.1 - p.1);
                            if nearest.get(&b).is_none_or(|&(best, _)| dist < best) {
                                nearest.insert(b, (dist, q));
                            }
                        }
                    }
                }
                for (dist, q) in nearest.into_values() {
                    if dist < near || dist > far {
                        continue;
                    }
                    // East is +x and south +z, so a positive cross product lies to the right.
                    let cross = d.0 * (q.1 - p.1) - d.1 * (q.0 - p.0);
                    if cross > 0.0 {
                        right += 1;
                    } else {
                        left += 1;
                    }
                }
            }
            right > left
        })
        .collect();
    for (route, flip) in routes.iter_mut().zip(flips) {
        if !flip {
            continue;
        }
        let n = route.points.len();
        route.points.reverse();
        route.ways.reverse();
        route.caps.reverse();
        route.ends.reverse();
        let top = (n - 1) as f64;
        route.stops = route.stops.iter().rev().map(|s| top - s).collect();
        if route.shape == Shape::Loop {
            // Point 0 stays the start, so every index i becomes n - i.
            route.points.rotate_right(1);
            route.ways.rotate_right(1);
            route.caps.rotate_right(1);
            route.stops = route
                .stops
                .iter()
                .map(|s| (s + 1.0).rem_euclid(n as f64))
                .collect();
            route.stops.sort_by(|a, b| a.total_cmp(b));
        }
    }
}

/// What runs on a route and its timetable, or None where no train fits.
fn service_for(r: usize, route: &Route, scale: f64, size: f64) -> Option<Service> {
    let seed = route.ways[0] ^ (r as u64).wrapping_mul(0x9E37);
    let len_m = route.len() / scale;
    let has_stops = !route.stops.is_empty();
    for short in [false, true] {
        let (coaches, liveries) = train_bodies::compose(
            route_kind(route, has_stops, len_m, seed),
            seed,
            short || len_m < 400.0,
        );
        let lengths: Vec<f64> = coaches
            .iter()
            .map(|c| train_bodies::length_m(*c) * size)
            .collect();
        let mut offsets = vec![0.0];
        for k in 1..lengths.len() {
            offsets.push(offsets[k - 1] + (lengths[k - 1] + lengths[k]) / 2.0 + COUPLING_M * size);
        }
        let body = Body {
            front: lengths[0] / 2.0,
            back: lengths[lengths.len() - 1] / 2.0,
            span: *offsets.last().unwrap(),
        };
        let train_m = (body.span + body.front + body.back) / scale;
        if route.ends.contains(&End::Junction)
            && route.shape == Shape::Shuttle
            && len_m < JUNCTION_RUN_M.max(3.0 * train_m)
        {
            continue;
        }
        let Some((timetable, period)) = timetable(route, &body, scale) else {
            continue;
        };
        let t = timetable.len();
        let trains = match route.shape {
            Shape::Shuttle => vec![(0usize, liveries(0))],
            _ => {
                let headway = (route.mode.headway_s() * STEPS_PER_S) as usize;
                let room = body.span + body.front + body.back + 30.0 * scale;
                // A ring gets a second train when it has the room, so one is always in sight.
                let least = if route.shape == Shape::Loop { 2 } else { 1 };
                let mut n = ((t + headway / 2) / headway).max(least);
                while n > 1 && closest(&timetable, period, n) < room * 1000.0 {
                    n -= 1;
                }
                if n == 1 && period > 0 && (period as f64) < room * 1000.0 * 2.0 {
                    continue;
                }
                let start = (hash(seed, 7) % t as u64) as usize;
                (0..n)
                    .map(|k| ((start + k * t / n) % t, liveries(k)))
                    .collect()
            }
        };
        return Some(Service {
            route: r,
            coaches,
            offsets,
            timetable,
            period,
            trains,
        });
    }
    None
}

/// The closest any two of `n` trains spaced evenly through a looping `timetable` come,
/// front to front, in thousandths of a block.
fn closest(timetable: &[i32], period: i64, n: usize) -> f64 {
    let t = timetable.len();
    let gap = t / n;
    (0..t)
        .map(|i| {
            let ahead = timetable[(i + gap) % t] as i64;
            let behind = timetable[i] as i64;
            // The one behind is `gap` steps later along the same timetable, so it is at
            // where the one ahead was `gap` steps ago.
            (ahead - behind).rem_euclid(period.max(1)) as f64
        })
        .fold(f64::MAX, f64::min)
}

/// What a route's trains look like.
fn route_kind(route: &Route, has_stops: bool, len_m: f64, seed: u64) -> train_bodies::Kind {
    use train_bodies::Kind;
    match route.mode {
        Mode::Tram => Kind::Tram,
        Mode::Metro => Kind::Metro,
        Mode::SBahn => Kind::SBahn,
        Mode::Rail => {
            let roll = hash(seed, 3) % 100;
            if has_stops {
                match roll {
                    0..=24 if len_m > 1500.0 => Kind::Intercity,
                    0..=59 => Kind::Regional,
                    _ => Kind::DoubleDeck,
                }
            } else {
                match roll {
                    0..=54 => Kind::Freight,
                    55..=79 => Kind::Intercity,
                    _ => Kind::Regional,
                }
            }
        }
    }
}

/// A train's reach from its front carriage centre: ahead to its nose, back to the last
/// carriage centre and on to its tail.
struct Body {
    front: f64,
    span: f64,
    back: f64,
}

/// A stretch of a trip: where to get to, and what happens there.
#[derive(Clone, Copy)]
struct Target {
    at: f64,
    /// Brake to a stand there and wait this many steps.
    dwell: Option<usize>,
}

/// One trip of a route's trains, step by step: the front carriage centre in thousandths
/// of a block, and the lap length the pack wraps it by (0 on a shuttle).
fn timetable(route: &Route, body: &Body, scale: f64) -> Option<(Vec<i32>, i64)> {
    let mode = route.mode;
    let (accel, brake, _) = mode.accel();
    let per_step2 = |a: f64| a / (STEPS_PER_S * STEPS_PER_S) * scale;
    let (a, b) = (per_step2(accel), per_step2(brake));
    let cruise = step_speed(mode.cruise_kmh(), scale);
    let (dwell_stop, dwell_end) = mode.dwell_s();
    let (dwell_stop, dwell_end) = (
        (dwell_stop * STEPS_PER_S) as usize,
        (dwell_end * STEPS_PER_S) as usize,
    );
    let len = route.len();
    let margin = END_MARGIN_M * scale;
    let layover = cruise * LAYOVER_S * STEPS_PER_S;
    let n = route.points.len();
    // Where the front carriage centre stands with the train's middle at a stop.
    let stop_at = |s: f64| s + body.span / 2.0;

    // Off the route a train runs at its cruising speed.
    let cap_at = |s: i64| -> f64 {
        if route.shape == Shape::Loop {
            route.caps[s.rem_euclid(n as i64) as usize]
        } else if s < 0 || s as f64 > len {
            cruise
        } else {
            route.caps[(s as usize).min(n - 1)]
        }
    };

    match route.shape {
        Shape::Shuttle => {
            let first_stop = route.stops.first().map(|&s| stop_at(s));
            let last_stop = route.stops.last().map(|&s| stop_at(s));
            let low = match route.ends[0] {
                End::Edge => -(body.front + layover),
                _ => {
                    let end = margin + body.back + body.span;
                    first_stop
                        .filter(|&s| s >= end && s - end < TERMINUS_STOP_M * scale)
                        .unwrap_or(end)
                }
            };
            let high = match route.ends[1] {
                End::Edge => len + body.span + body.back + layover,
                _ => {
                    let end = len - margin - body.front;
                    last_stop
                        .filter(|&s| s <= end && end - s < TERMINUS_STOP_M * scale)
                        .unwrap_or(end)
                }
            };
            if high - low < 10.0 {
                return None;
            }
            let profile = Profile::build(
                low.floor() as i64 - 1,
                high.ceil() as i64 + 1,
                None,
                body,
                b,
                &cap_at,
            );
            let end_dwell = |end: End| match end {
                End::Edge => None,
                End::Buffer => Some(dwell_end),
                End::Junction => Some(dwell_stop / 2),
            };
            let inner: Vec<f64> = route
                .stops
                .iter()
                .map(|&s| stop_at(s))
                .filter(|&s| s > low + 1.0 && s < high - 1.0)
                .collect();
            let leg = |stops: &mut dyn Iterator<Item = f64>, end: f64, at_end: End| {
                let mut out: Vec<Target> = stops
                    .map(|at| Target {
                        at,
                        dwell: Some(dwell_stop),
                    })
                    .collect();
                out.push(Target {
                    at: end,
                    dwell: end_dwell(at_end),
                });
                out
            };
            let out = leg(&mut inner.iter().copied(), high, route.ends[1]);
            let back = leg(&mut inner.iter().rev().copied(), low, route.ends[0]);
            let mut trip = Trip::new(low, a, b, &profile);
            match end_dwell(route.ends[0]) {
                Some(d) => trip.wait(d),
                None => trip.v = cruise,
            }
            for target in out {
                trip.run(1.0, target)?;
            }
            for target in back {
                trip.run(-1.0, target)?;
            }
            // The trip ends standing where it began, and the next begins with that wait.
            if let Some(d) = end_dwell(route.ends[0]) {
                trip.steps.truncate(trip.steps.len().saturating_sub(d));
            }
            let steps = trip.finish()?;
            Some((
                steps.iter().map(|&u| (u * 1000.0).round() as i32).collect(),
                0,
            ))
        }
        Shape::Loop | Shape::Through => {
            let lap = match route.shape {
                Shape::Loop => len,
                _ => len + body.span + body.back + body.front + layover,
            };
            let mut stops: Vec<f64> = route
                .stops
                .iter()
                .map(|&s| stop_at(s))
                .filter(|&s| route.shape == Shape::Loop || s <= len)
                .map(|s| s.rem_euclid(lap))
                .collect();
            stops.sort_by(|x, y| x.total_cmp(y));
            let cells = lap.ceil() as i64;
            let profile = Profile::build(0, cells - 1, Some(lap), body, b, &|s: i64| {
                if route.shape == Shape::Loop {
                    cap_at(s)
                } else {
                    cap_at(s.rem_euclid(cells))
                }
            });
            let start = stops.first().copied().unwrap_or(match route.shape {
                Shape::Loop => 0.0,
                _ => len + body.span + body.back + layover / 2.0,
            });
            let mut trip = Trip::new(start, a, b, &profile);
            if stops.is_empty() {
                trip.v = profile.at(start, 1.0).min(cruise);
                trip.run(
                    1.0,
                    Target {
                        at: start + lap,
                        dwell: None,
                    },
                )?;
            } else {
                trip.wait(dwell_stop);
                let mut targets: Vec<f64> = stops.iter().skip(1).copied().collect();
                targets.push(stops[0] + lap);
                for at in targets {
                    trip.run(
                        1.0,
                        Target {
                            at,
                            dwell: Some(dwell_stop),
                        },
                    )?;
                }
                trip.steps
                    .truncate(trip.steps.len().saturating_sub(dwell_stop));
            }
            let steps = trip.finish()?;
            let lap_mm = (lap * 1000.0).round() as i64;
            Some((
                steps
                    .iter()
                    .map(|&u| ((u * 1000.0).round() as i64).rem_euclid(lap_mm) as i32)
                    .collect(),
                lap_mm,
            ))
        }
    }
}

/// The fastest a train may run with its front carriage centre at each whole block, either
/// way, and still slow in time for every limit ahead: the slowest limit under the whole
/// train, eased back over braking distance.
struct Profile {
    base: i64,
    wrap: Option<f64>,
    ahead: Vec<f64>,
    back: Vec<f64>,
}

impl Profile {
    fn build(
        lo: i64,
        hi: i64,
        wrap: Option<f64>,
        body: &Body,
        b: f64,
        cap_at: &dyn Fn(i64) -> f64,
    ) -> Self {
        let (behind, ahead) = (
            (body.span + body.back).ceil() as i64,
            body.front.ceil() as i64,
        );
        let n = (hi - lo + 1).max(1) as usize;
        // The slowest limit under the train, by a running minimum over the caps.
        let caps: Vec<f64> = (lo - behind..=hi + ahead).map(cap_at).collect();
        let width = (behind + ahead + 1) as usize;
        let mut under = Vec::with_capacity(n);
        let mut window: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        for (i, &c) in caps.iter().enumerate() {
            while window.back().is_some_and(|&j| caps[j] >= c) {
                window.pop_back();
            }
            window.push_back(i);
            if window.front().is_some_and(|&j| j + width <= i) {
                window.pop_front();
            }
            if i + 1 >= width {
                under.push(caps[window[0]]);
            }
        }
        under.truncate(n);
        let ease = |v: f64| (v * v + 2.0 * b).sqrt();
        let mut fwd = under.clone();
        let mut bwd = under.clone();
        let rounds = if wrap.is_some() { 2 } else { 1 };
        for _ in 0..rounds {
            for i in (0..n).rev() {
                let next = if i + 1 < n {
                    fwd[i + 1]
                } else if wrap.is_some() {
                    fwd[0]
                } else {
                    continue;
                };
                fwd[i] = fwd[i].min(ease(next));
            }
            for i in 0..n {
                let prev = if i > 0 {
                    bwd[i - 1]
                } else if wrap.is_some() {
                    bwd[n - 1]
                } else {
                    continue;
                };
                bwd[i] = bwd[i].min(ease(prev));
            }
        }
        Self {
            base: lo,
            wrap,
            ahead: fwd,
            back: bwd,
        }
    }

    /// The speed allowed at `u` running in `dir`.
    fn at(&self, u: f64, dir: f64) -> f64 {
        let u = match self.wrap {
            Some(lap) => u.rem_euclid(lap),
            None => u,
        };
        let list = if dir > 0.0 { &self.ahead } else { &self.back };
        let n = list.len() as i64;
        let cell = |i: i64| {
            let i = i - self.base;
            let i = if self.wrap.is_some() {
                i.rem_euclid(n)
            } else {
                i.clamp(0, n - 1)
            };
            list[i as usize]
        };
        cell(u.floor() as i64).min(cell(u.ceil() as i64))
    }
}

/// A train run along its route a step at a time, keeping to its profile and braking in
/// time for where it has to stand.
struct Trip<'a> {
    u: f64,
    v: f64,
    a: f64,
    b: f64,
    profile: &'a Profile,
    steps: Vec<f64>,
}

impl<'a> Trip<'a> {
    fn new(u: f64, a: f64, b: f64, profile: &'a Profile) -> Self {
        Self {
            u,
            v: 0.0,
            a,
            b,
            profile,
            steps: Vec::new(),
        }
    }

    fn wait(&mut self, steps: usize) {
        self.v = 0.0;
        self.steps.extend(std::iter::repeat_n(self.u, steps));
    }

    fn run(&mut self, dir: f64, target: Target) -> Option<()> {
        loop {
            if self.steps.len() > MAX_STEPS {
                return None;
            }
            let dist = (target.at - self.u) * dir;
            if dist <= 1e-6 {
                break;
            }
            let mut top = (self.v + self.a).min(self.profile.at(self.u, dir));
            if target.dwell.is_some() {
                top = top.min((2.0 * self.b * dist).sqrt());
            }
            self.v = top.max(CREEP.min(dist));
            if self.v >= dist {
                self.u = target.at;
                self.steps.push(self.u);
                break;
            }
            self.u += dir * self.v;
            self.steps.push(self.u);
        }
        if let Some(steps) = target.dwell {
            self.wait(steps);
        }
        Some(())
    }

    fn finish(self) -> Option<Vec<f64>> {
        (!self.steps.is_empty() && self.steps.len() <= MAX_STEPS).then_some(self.steps)
    }
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

    fn track(id: u64, nodes: Vec<ProcessedNode>, railway: &str) -> ProcessedElement {
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes,
            tags: [("railway".to_string(), railway.to_string())]
                .into_iter()
                .collect(),
        })
    }

    fn world() -> XZBBox {
        XZBBox::rect_from_min_max(0, 0, 2000, 2000).unwrap()
    }

    fn line(id0: u64, from: (i32, i32), to: (i32, i32), n: usize) -> Vec<ProcessedNode> {
        (0..=n)
            .map(|k| {
                let t = k as f64 / n as f64;
                node(
                    id0 + k as u64,
                    (from.0 as f64 + (to.0 - from.0) as f64 * t).round() as i32,
                    (from.1 as f64 + (to.1 - from.1) as f64 * t).round() as i32,
                    &[],
                )
            })
            .collect()
    }

    /// A ring of `n` nodes round (cx, cz), closed on its first node.
    fn ring(id0: u64, (cx, cz): (i32, i32), r: f64, n: usize) -> Vec<ProcessedNode> {
        let mut out: Vec<ProcessedNode> = (0..n)
            .map(|k| {
                let a = k as f64 / n as f64 * std::f64::consts::TAU;
                node(
                    id0 + k as u64,
                    cx + (r * a.cos()).round() as i32,
                    cz + (r * a.sin()).round() as i32,
                    &[],
                )
            })
            .collect();
        out.push(out[0].clone());
        out
    }

    fn net(elements: &[ProcessedElement]) -> TrainNetwork {
        TrainNetwork::build(elements, 1.0, &world())
    }

    #[test]
    fn a_double_track_through_the_area_runs_each_way_on_its_right() {
        let elements = vec![
            track(1, line(100, (0, 1000), (2000, 1000), 20), "rail"),
            track(2, line(200, (0, 1004), (2000, 1004), 20), "rail"),
        ];
        let net = net(&elements);
        assert_eq!(net.routes.len(), 2);
        for route in &net.routes {
            assert_eq!(route.shape, Shape::Through);
            let (a, b) = (route.points[0], route.points[route.points.len() - 1]);
            // Driving east the right is the south, +z: the track at 1004.
            if a.1 > 1002.0 {
                assert!(b.0 > a.0, "the southern track runs east");
            } else {
                assert!(b.0 < a.0, "the northern track runs west");
            }
        }
        assert!(net.train_count() >= 2);
    }

    #[test]
    fn a_train_on_the_largest_scale_stays_within_what_the_pack_follows() {
        let world = XZBBox::rect_from_min_max(0, 0, 4000, 2000).unwrap();
        let elements = vec![track(1, line(100, (0, 1000), (4000, 1000), 40), "rail")];
        let net = TrainNetwork::build(&elements, crate::args::MAX_SCALE, &world);
        assert!(!net.services.is_empty());
        for service in &net.services {
            // A through route's timetable wraps by its lap.
            let lap = service.period;
            let step = |d: i64| match lap {
                0 => d.abs(),
                _ => d.rem_euclid(lap).min(lap - d.rem_euclid(lap)),
            };
            let t = &service.timetable;
            let most = t.windows(2).map(|w| step(i64::from(w[1] - w[0]))).max();
            assert!(most.is_some_and(|m| m > 7000 && m < 8000), "{most:?}");
        }
    }

    #[test]
    fn a_ring_line_is_a_loop_with_trains_kept_apart() {
        let elements = vec![track(1, ring(100, (1000, 1000), 400.0, 64), "subway")];
        let net = net(&elements);
        assert_eq!(net.routes.len(), 1);
        assert_eq!(net.routes[0].shape, Shape::Loop);
        let service = &net.services[0];
        assert!(service.trains.len() >= 2, "{}", service.trains.len());
        let room = (service.offsets.last().unwrap() + 20.0) * 1000.0;
        let gap = closest(&service.timetable, service.period, service.trains.len());
        assert!(gap > room, "trains come within {gap} of each other");
    }

    #[test]
    fn a_small_ring_holds_only_the_trains_that_fit_on_it() {
        let mut served = 0;
        for r in [25.0, 35.0, 50.0, 70.0, 100.0] {
            let elements = vec![track(1, ring(100, (1000, 1000), r, 48), "subway")];
            let net = net(&elements);
            for service in &net.services {
                served += 1;
                let train = (service.offsets.last().unwrap() + 20.0) * 1000.0;
                let gap = closest(&service.timetable, service.period, service.trains.len());
                // Front to front, a train's length and some clear track between them.
                assert!(
                    gap > train + 20_000.0,
                    "radius {r}: trains come within {gap} of each other"
                );
            }
        }
        assert!(served >= 2, "only {served} rings got trains");
    }

    #[test]
    fn a_line_between_buffers_shuttles_and_stays_on_its_track() {
        let elements = vec![track(1, line(100, (500, 500), (1300, 500), 16), "rail")];
        let net = net(&elements);
        let route = &net.routes[0];
        assert_eq!(route.shape, Shape::Shuttle);
        assert_eq!(route.ends, [End::Buffer, End::Buffer]);
        let service = &net.services[0];
        let span = *service.offsets.last().unwrap();
        let len = route.len() * 1000.0;
        let lo = service.timetable.iter().min().copied().unwrap() as f64;
        let hi = service.timetable.iter().max().copied().unwrap() as f64;
        assert!(lo - span * 1000.0 >= 0.0, "the tail runs off the low end");
        assert!(hi <= len, "the front runs off the high end");
        assert!(hi - lo > len * 0.5, "it hardly moves");
    }

    #[test]
    fn a_line_into_a_terminus_leaves_the_area_and_comes_back() {
        let elements = vec![track(1, line(100, (0, 500), (900, 500), 18), "subway")];
        let net = net(&elements);
        let route = &net.routes[0];
        assert_eq!(route.ends, [End::Edge, End::Buffer]);
        let tt = &net.services[0].timetable;
        let lo = *tt.iter().min().unwrap();
        assert!(lo < 0, "it never leaves the area");
        // It stands at the buffer end for a while.
        let hi = *tt.iter().max().unwrap();
        let standing = tt.iter().filter(|&&s| s == hi).count();
        assert!(standing >= 200, "{standing} steps at the end");
    }

    #[test]
    fn a_train_stands_with_its_middle_at_the_platform() {
        let mut elements = vec![track(1, line(100, (500, 500), (1500, 500), 20), "rail")];
        elements.push(ProcessedElement::Way(ProcessedWay {
            id: 9,
            nodes: vec![node(900, 900, 504, &[]), node(901, 1100, 504, &[])],
            tags: [("railway".to_string(), "platform".to_string())]
                .into_iter()
                .collect(),
        }));
        let net = net(&elements);
        let route = &net.routes[0];
        assert_eq!(route.stops.len(), 1);
        let service = &net.services[0];
        let middle = service.offsets.last().unwrap() / 2.0;
        let stop_mm = ((route.stops[0] + middle) * 1000.0).round() as i32;
        let standing = service.timetable.iter().filter(|&&s| s == stop_mm).count();
        let (dwell, _) = Mode::Rail.dwell_s();
        // Both ways past it, it waits.
        assert!(
            standing >= 2 * (dwell * STEPS_PER_S) as usize - 2,
            "{standing}"
        );
        let at = route.points[route.stops[0].round() as usize];
        assert!((at.0 - 1000.5).abs() < 2.0, "the stop is at x={}", at.0);
    }

    #[test]
    fn a_train_never_runs_faster_than_the_track_under_it() {
        // A tight curve between two straights.
        let mut nodes = line(100, (0, 500), (800, 500), 8);
        nodes.extend((1..12).map(|k| {
            let a = k as f64 / 12.0 * std::f64::consts::FRAC_PI_2;
            node(
                200 + k as u64,
                800 + (60.0 * a.sin()).round() as i32,
                560 - (60.0 * a.cos()).round() as i32,
                &[],
            )
        }));
        nodes.extend(line(300, (860, 560), (860, 2000), 14));
        let net = net(&[track(1, nodes, "tram")]);
        let route = &net.routes[0];
        let service = &net.services[0];
        let tt = &service.timetable;
        let span = *service.offsets.last().unwrap();
        for w in tt.windows(2) {
            let (u0, u1) = (w[0] as f64 / 1000.0, w[1] as f64 / 1000.0);
            let v = (u1 - u0).abs();
            for s in [u0, u0 - span] {
                if s >= 0.0 && s <= route.len() {
                    let cap = route.caps[(s.round() as usize).min(route.caps.len() - 1)];
                    assert!(v <= cap * 1.02 + 0.06, "{v} over {cap} at {s}");
                }
            }
        }
    }

    #[test]
    fn a_line_goes_straight_through_a_switch_and_a_crossing() {
        let main = line(100, (0, 1000), (2000, 1000), 20);
        // A branch off node 110 at (1000, 1000), and a line crossing at node 105.
        let mut branch = vec![node(110, 1000, 1000, &[])];
        branch.extend(line(400, (1100, 1020), (2000, 1400), 8));
        let mut cross = line(500, (500, 0), (500, 990), 10);
        cross.push(node(105, 500, 1000, &[]));
        cross.extend(line(600, (500, 1010), (500, 2000), 10));
        let elements = vec![
            track(1, main, "tram"),
            track(2, branch, "tram"),
            track(3, cross, "tram"),
        ];
        let net = net(&elements);
        let east_west = net
            .routes
            .iter()
            .find(|r| {
                let (a, b) = (r.points[0], r.points[r.points.len() - 1]);
                (a.1 - 1000.5).abs() < 1.0 && (b.1 - 1000.5).abs() < 1.0
            })
            .expect("the main line is one route");
        assert_eq!(east_west.shape, Shape::Through);
        assert!(net
            .routes
            .iter()
            .any(|r| r.points[0].0 > 499.0 && r.points[0].0 < 502.0 && r.points.len() > 1900));
    }

    #[test]
    fn a_curve_joining_two_lines_gets_no_train_of_its_own() {
        let mut curve = vec![node(110, 1000, 1000, &[])];
        curve.extend(line(400, (1010, 1010), (1040, 1100), 6));
        curve.push(node(211, 1040, 1100, &[]));
        let elements = vec![
            track(1, line(100, (0, 1000), (2000, 1000), 20), "tram"),
            track(2, line(200, (1040, 0), (1040, 2000), 20), "tram"),
            track(3, curve, "tram"),
        ];
        let net = net(&elements);
        let connector = net
            .routes
            .iter()
            .position(|r| r.points.len() < 200)
            .expect("the curve is a route of its own");
        assert_eq!(net.routes[connector].ends, [End::Junction, End::Junction]);
        assert!(net.services.iter().all(|s| s.route != connector));
        assert_eq!(net.services.len(), 2, "both lines still run");
    }

    #[test]
    fn spacing_counts_a_train_catching_up_on_one_that_stands() {
        // A block a step all round a 100-block lap: ten trains stay ten blocks apart.
        let steady: Vec<i32> = (0..100).map(|i| i * 1000).collect();
        assert_eq!(closest(&steady, 100_000, 10), 10_000.0);
        // Standing half the lap's time, then running: the one behind reaches it.
        let stop_go: Vec<i32> = (0..100).map(|i| (i.max(50) - 50) * 2000).collect();
        assert_eq!(closest(&stop_go, 100_000, 2), 0.0);
    }

    #[test]
    fn points_past_a_joint_belong_to_the_way_they_lie_on() {
        // One long stretch each side of the joint at x = 1000.
        let elements = vec![
            track(1, line(100, (0, 1000), (1000, 1000), 2), "rail"),
            track(2, line(102, (1000, 1000), (2000, 1100), 1), "rail"),
        ];
        let net = net(&elements);
        let route = &net.routes[0];
        assert_eq!(route.points.len(), route.ways.len());
        for (p, &w) in route.points.iter().zip(&route.ways) {
            if p.0 < 990.0 {
                assert_eq!(w, 1, "at {:?}", p);
            } else if p.0 > 1010.0 {
                assert_eq!(w, 2, "at {:?}", p);
            }
        }
    }

    #[test]
    fn a_siding_too_short_for_a_train_gets_none() {
        let elements = vec![track(1, line(100, (500, 500), (520, 500), 2), "rail")];
        let net = net(&elements);
        assert_eq!(net.train_count(), 0);
    }

    #[test]
    fn run_lengths_give_back_the_timetable() {
        let tt = [5, 5, 5, 8, 11, 14, 14, -2, -2, -2, 0];
        let runs = run_lengths(&tt);
        let mut back = vec![runs[0]];
        for pair in runs[1..].chunks(2) {
            for _ in 0..pair[1] {
                back.push(back.last().unwrap() + pair[0]);
            }
        }
        assert_eq!(back, tt.iter().map(|&v| v as i64).collect::<Vec<_>>());
        assert!(run_lengths(&[]).is_empty());
        assert_eq!(run_lengths(&[7]), vec![7]);
    }

    #[test]
    fn bedrock_runs_the_trains_the_pack_runs() {
        let elements = vec![
            track(1, line(100, (0, 1000), (2000, 1000), 20), "light_rail"),
            track(2, ring(300, (1000, 400), 200.0, 40), "tram"),
        ];
        let net = net(&elements);
        let (data, bodies) = net.bedrock_data(|_, _, _| Some(-61));
        let services = data["services"].as_array().unwrap();
        let trains: usize = services
            .iter()
            .map(|s| s["trains"].as_array().unwrap().len())
            .sum();
        assert_eq!(trains, net.train_count());
        let numbers = |v: &serde_json::Value| -> Vec<i64> {
            v.as_str()
                .unwrap()
                .split(',')
                .map(|n| n.parse().unwrap())
                .collect()
        };
        let mut ids = Vec::new();
        for (service, ours) in services.iter().zip(&net.services) {
            assert_eq!(numbers(&service["steps"]), run_lengths(&ours.timetable));
            assert_eq!(service["count"], ours.timetable.len());
            let route = &data["routes"][service["route"].as_u64().unwrap() as usize];
            let p = numbers(&route["p"]);
            // Rails at -61 put trains at -60.9375.
            assert_eq!(p[1], -60938);
            assert_eq!(p.len(), net.routes[ours.route].points.len() * 4);
            // The script skips a route no one is near by this box, so all of it is inside.
            let b: Vec<f64> = route["box"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect();
            for &(x, z) in &net.routes[ours.route].points {
                assert!(x >= b[0] && z >= b[1] && x <= b[2] && z <= b[3]);
            }
            for train in service["trains"].as_array().unwrap() {
                for car in train["cars"].as_array().unwrap() {
                    ids.push(car[0].as_u64().unwrap());
                    assert!((car[1].as_u64().unwrap() as usize) < bodies.len());
                }
            }
        }
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "every carriage has an id of its own");
        let names = data["bodies"].as_array().unwrap();
        assert_eq!(names.len(), bodies.len());
        for (name, body) in names.iter().zip(&bodies) {
            assert_eq!(name.as_str().unwrap(), body.name);
            assert!(body.name.starts_with("train_"));
            assert!(body
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
            for s in &body.shapes {
                assert!(crate::bedrock_addon::palette_has(s.block), "{}", s.block);
            }
        }
        // No rails laid, nothing to run.
        let (none, _) = net.bedrock_data(|_, _, _| None);
        assert!(none["services"].as_array().unwrap().is_empty());
    }

    #[test]
    fn the_pack_writes_a_function_per_train_and_heights_from_the_rails() {
        let elements = vec![
            track(1, line(100, (0, 1000), (2000, 1000), 20), "light_rail"),
            track(2, ring(300, (1000, 400), 200.0, 40), "tram"),
        ];
        let net = net(&elements);
        let files = net.pack_functions(|_, _, _| Some(-61));
        let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.contains(&"data/arnis/function/trains/network.mcfunction"));
        let all = &files
            .iter()
            .find(|(p, _)| p.ends_with("trains/all.mcfunction"))
            .unwrap()
            .1;
        assert_eq!(all.lines().count(), net.train_count());
        let network = &files
            .iter()
            .find(|(p, _)| p.ends_with("network.mcfunction"))
            .unwrap()
            .1;
        assert!(
            network.contains("y:-60.938d"),
            "rails at -61 put trains at -60.9375"
        );
        // A straight line along z = 1000 would sit on whole blocks; the pack would shift it.
        for key in ["{x:", ",z:"] {
            for part in network.split(key).skip(1) {
                let v: f64 = part.split('d').next().unwrap().parse().unwrap();
                assert!(v.fract() != 0.0, "{key}{v} is whole");
            }
        }
        for (path, text) in &files {
            if path.contains("/trains/t/") {
                assert!(text.contains("function arnis:trains/car"));
            }
        }
        // No rails laid, no route.
        assert!(net
            .pack_functions(|_, _, _| None)
            .iter()
            .all(|(p, _)| !p.contains("/trains/t/")));
    }
}
