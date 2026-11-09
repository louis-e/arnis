//! Ferries, barges and boats on the mapped water. Ferry lines, navigable rivers and canals
//! and loops on open lakes and the sea are planned before the world is built; once it is,
//! the water each route crosses is read back (`water_probe`), ferry lines are trimmed to
//! where the water starts, and each route gets the largest boat that fits its width and
//! passes under its bridges. They then sail on the trains' timetables and pack functions.

use crate::clipping::is_invented_node_id;
use crate::coordinate_system::cartesian::XZBBox;
use crate::element_processing::ship_bodies::Vessel;
use crate::element_processing::train_bodies::Coach;
use crate::element_processing::trains::{
    self, Body, End, Mode, Route, Service, Shape, StopFeature, StopKind, StopSource,
};
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::water_probe::{Sample, WaterProbe};
use fnv::FnvHashMap;
use std::collections::HashMap;

/// Most routes a world gets of each use.
const MAX_FERRIES: usize = 40;
const MAX_WATERWAYS: usize = 40;
const MAX_LEISURE: usize = 40;
/// A route ending this close to the edge of the area sails on outside it.
const EDGE_REACH: f64 = 3.0;
/// The surface of a water block over its bottom, where a boat floats.
const WATER_LIFT: f64 = 0.875;
/// Leisure boats per square kilometre of open water.
const BOATS_PER_KM2: f64 = 2.0;
/// Side of the cells open water is looked at in, in blocks.
const CELL: i32 = 8;
/// Radius of a leisure boat's loop, and how far it keeps off the shore, in metres.
const LOOP_RADIUS_M: (f64, f64) = (25.0, 90.0);
const SHORE_M: f64 = 15.0;
/// How far leisure boats keep from a river and from the routes of ferries and ships.
const KEEP_OFF_RIVER_M: f64 = 120.0;
const KEEP_OFF_ROUTE_M: f64 = 60.0;
/// How far either way of a waterway's middle its width is looked for, in metres.
const CHANNEL_M: f64 = 48.0;
/// Farthest a ship keeps off a waterway's middle, in metres.
const LANE_MAX_M: f64 = 30.0;
/// Every so many points of a waterway its width is looked across.
const CHANNEL_STRIDE: usize = 3;
/// Ferry lines this close for most of their way share a crossing, in metres.
const SHARED_M: f64 = 15.0;
/// Room left between a boat's top and what it passes under, in blocks.
const HEADROOM: f64 = 0.5;
/// Longest stretch of a ferry line, in blocks at 1:1, the water may break off for.
const DRY_GAP: usize = 3;

/// What a route is for.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Use {
    /// A ferry line, whether it carries cars and how long a crossing is mapped to take, in
    /// minutes.
    Ferry { cars: bool, minutes: Option<u32> },
    /// A navigable river or canal, by the CEMT class of ship it takes (0 for boats only).
    Waterway { class: u8 },
    /// A loop for a leisure boat on a lake or the sea.
    Leisure,
}

struct Line {
    role: Use,
    /// A block apart.
    points: Vec<(f64, f64)>,
    ways: Vec<u64>,
    looped: bool,
    ends: [End; 2],
    seed: u64,
}

/// The water routes of an area, waiting for the built world to be read back.
pub struct ShipPlan {
    lines: Vec<Line>,
    piers: Vec<StopFeature>,
    scale: f64,
    size: f64,
}

/// A way's nodes as (id, x, z), centred on their blocks.
type Nodes = Vec<(u64, f64, f64)>;

impl ShipPlan {
    /// `is_water` says where the land cover has open water, before anything is built.
    pub fn build(
        elements: &[ProcessedElement],
        scale: f64,
        world: &XZBBox,
        is_water: impl Fn(i32, i32) -> bool,
    ) -> Self {
        let size = scale.clamp(0.4, 3.0);
        let at_edge = |id: u64, x: f64, z: f64| {
            let near = |v: f64, lo: i32, hi: i32| {
                v < lo as f64 + EDGE_REACH || v > (hi + 1) as f64 - EDGE_REACH
            };
            is_invented_node_id(id)
                || near(x, world.min_x(), world.max_x())
                || near(z, world.min_z(), world.max_z())
        };
        let mut ferries: Vec<(Nodes, u64, bool)> = Vec::new();
        let mut durations: FnvHashMap<u64, u32> = FnvHashMap::default();
        let mut waterways: Vec<(Nodes, u64, u8)> = Vec::new();
        let mut rivers: Vec<Nodes> = Vec::new();
        let mut piers = Vec::new();
        for element in elements {
            match element {
                ProcessedElement::Way(way) if way.nodes.len() >= 2 => {
                    let river = matches!(
                        way.tags.get("waterway").map(String::as_str),
                        Some("river" | "canal")
                    );
                    let ferry = ferry_cars(&way.tags);
                    let class = waterway_class(&way.tags);
                    // Most ways are none of these, and are passed by without a copy.
                    if !river && ferry.is_none() && class.is_none() && !is_terminal(&way.tags) {
                        continue;
                    }
                    let nodes: Nodes = way
                        .nodes
                        .iter()
                        .map(|n| (n.id, n.x as f64 + 0.5, n.z as f64 + 0.5))
                        .collect();
                    if river {
                        rivers.push(nodes.clone());
                    }
                    if let Some(cars) = ferry {
                        ferries.push((nodes, way.id, cars));
                        if let Some(m) = way.tags.get("duration").and_then(|d| minutes(d)) {
                            durations.insert(way.id, m);
                        }
                    } else if let Some(class) = class {
                        waterways.extend(
                            split_at_barriers(way)
                                .into_iter()
                                .map(|nodes| (nodes, way.id, class)),
                        );
                    } else if is_terminal(&way.tags) {
                        piers.push(StopFeature {
                            kind: StopKind::Ferry,
                            source: StopSource::Platform,
                            centre: centroid(&nodes),
                            at: nodes.iter().map(|n| (n.1, n.2)).collect(),
                        });
                    }
                }
                ProcessedElement::Node(node) if is_terminal(&node.tags) => {
                    let at = (node.x as f64 + 0.5, node.z as f64 + 0.5);
                    piers.push(StopFeature {
                        kind: StopKind::Ferry,
                        source: StopSource::Platform,
                        at: vec![at],
                        centre: at,
                    });
                }
                _ => {}
            }
        }

        let mut lines = Vec::new();
        let ends_of = |chain: &Chain| {
            let end = |n: &(u64, f64, f64)| {
                if at_edge(n.0, n.1, n.2) {
                    End::Edge
                } else {
                    End::Buffer
                }
            };
            [
                end(&chain.nodes[0]),
                end(&chain.nodes[chain.nodes.len() - 1]),
            ]
        };
        for chain in chains(
            ferries
                .iter()
                .map(|(n, id, cars)| (n.clone(), *id, u64::from(*cars))),
            120.0,
        ) {
            if lines.len() >= MAX_FERRIES {
                break;
            }
            let points = smooth(resample(&chain.points(), false), false);
            if points.len() < 16 {
                continue;
            }
            lines.push(Line {
                role: Use::Ferry {
                    cars: chain.tag == 1,
                    minutes: chain
                        .ways
                        .iter()
                        .filter_map(|w| durations.get(w))
                        .max()
                        .copied(),
                },
                ways: vec![chain.way; points.len()],
                ends: ends_of(&chain),
                points,
                looped: false,
                seed: chain.way,
            });
        }
        let mut sailed = 0;
        for chain in chains(
            waterways
                .iter()
                .map(|(n, id, class)| (n.clone(), *id, u64::from(*class))),
            40.0,
        ) {
            if sailed >= MAX_WATERWAYS {
                break;
            }
            // Only through the area: a ship that cannot turn has nowhere to go otherwise.
            if ends_of(&chain) != [End::Edge, End::Edge] {
                continue;
            }
            let points = smooth(resample(&chain.points(), false), false);
            if (points.len() as f64) < 200.0 * scale {
                continue;
            }
            sailed += 1;
            lines.push(Line {
                role: Use::Waterway {
                    class: chain.tag as u8,
                },
                ways: vec![chain.way; points.len()],
                ends: [End::Edge, End::Edge],
                points,
                looped: false,
                seed: chain.way,
            });
        }
        let routes: Vec<&[(f64, f64)]> = lines.iter().map(|l| l.points.as_slice()).collect();
        let loops = leisure_loops(world, scale, &is_water, &rivers, &routes);
        for (k, points) in loops.into_iter().enumerate() {
            lines.push(Line {
                role: Use::Leisure,
                ways: vec![0; points.len()],
                ends: [End::Junction, End::Junction],
                points,
                looped: true,
                seed: 0x5EA ^ (k as u64).wrapping_mul(0x9E37_79B9),
            });
        }
        Self {
            lines,
            piers,
            scale,
            size,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// How far across a route of `role` the water is read, in blocks either way of its
    /// middle, and every how many points.
    fn across(&self, role: Use) -> (Vec<f64>, usize) {
        let mut offsets: Vec<f64> = match role {
            Use::Ferry { .. } => [
                Vessel::RiverFerry,
                Vessel::WaterBus,
                Vessel::CarFerry,
                Vessel::SeaFerry,
            ]
            .iter()
            .map(|v| v.size_m().1 / 2.0)
            .chain([2.0, 6.0])
            .map(|m| m * self.size)
            .collect(),
            Use::Waterway { .. } => return (channel_offsets(self.size), CHANNEL_STRIDE),
            Use::Leisure => [Vessel::Sailboat, Vessel::Motorboat]
                .iter()
                .map(|v| v.size_m().1 / 2.0 * self.size)
                .collect(),
        };
        offsets.sort_by(|a, b| a.total_cmp(b));
        offsets.dedup();
        (offsets, 3)
    }

    /// The columns to read once they are built: every point of each route, and enough
    /// either side to tell how wide the water is and what spans it.
    pub fn probe(&self) -> WaterProbe {
        let mut columns = Vec::new();
        for line in &self.lines {
            let (offsets, stride) = self.across(line.role);
            for (i, &(x, z)) in line.points.iter().enumerate() {
                columns.push(block(x, z));
                if i % stride != 0 {
                    continue;
                }
                let (nx, nz) = normal(&line.points, i, line.looped);
                for &d in &offsets {
                    columns.push(block(x + nx * d, z + nz * d));
                    columns.push(block(x - nx * d, z - nz * d));
                }
            }
        }
        // The world knows where its water is; no level of its own to look round.
        WaterProbe::new(columns.into_iter().map(|(x, z)| (x, z, 0)))
    }

    /// The routes that turned out to have the water for a boat, each with what sails it.
    pub(crate) fn finish(&self, probe: &WaterProbe) -> Vec<(Route, Service)> {
        let water = |x: f64, z: f64| {
            let (x, z) = block(x, z);
            probe.get(x, z, 0)
        };
        let mut out = Vec::new();
        let mut ferries = Vec::new();
        for line in &self.lines {
            match line.role {
                Use::Ferry { cars, minutes } => {
                    for part in self.sections(line, &water) {
                        ferries.extend(self.ferry(&part, cars, minutes, &water));
                    }
                }
                Use::Waterway { class } => out.extend(self.waterway(line, class, &water)),
                Use::Leisure => out.extend(self.leisure(line, &water)),
            }
        }
        // Lines that share a crossing get one ferry there, the longest line's, rather than
        // several that sail through each other.
        ferries.sort_by_key(|(route, _): &(Route, Service)| std::cmp::Reverse(route.points.len()));
        let cell = (SHARED_M * self.size).max(4.0);
        let key = |&(x, z): &(f64, f64)| ((x / cell).floor() as i32, (z / cell).floor() as i32);
        let mut sailed: FnvHashMap<(i32, i32), ()> = FnvHashMap::default();
        let mut kept = Vec::new();
        for (route, service) in ferries {
            let shared = route
                .points
                .iter()
                .filter(|p| {
                    let (cx, cz) = key(p);
                    (-1..=1).any(|dx| (-1..=1).any(|dz| sailed.contains_key(&(cx + dx, cz + dz))))
                })
                .count();
            if shared * 2 > route.points.len() {
                continue;
            }
            sailed.extend(route.points.iter().map(|p| (key(p), ())));
            kept.push((route, service));
        }
        // The mapped ferry lines first, should the pack run short of room.
        kept.extend(out);
        kept
    }

    /// A ferry line cut where it runs over more land than a quay or a jetty, as a water bus
    /// line over the landing stages it calls at: each stretch of water between them sails
    /// on its own.
    fn sections(&self, line: &Line, water: &impl Fn(f64, f64) -> Option<Sample>) -> Vec<Line> {
        let n = line.points.len();
        let wet: Vec<bool> = line
            .points
            .iter()
            .map(|&(x, z)| floats(water(x, z)))
            .collect();
        let gap = (DRY_GAP as f64 * self.scale).ceil() as usize;
        let mut cuts: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        while i < n {
            if wet[i] {
                i += 1;
                continue;
            }
            let start = i;
            while i < n && !wet[i] {
                i += 1;
            }
            if start > 0 && i < n && i - start > gap {
                // Land with no landing stage on it is no way through at all.
                let reach = 25.0 * self.scale;
                let stage = self
                    .piers
                    .iter()
                    .flat_map(|p| p.at.iter())
                    .any(|&(px, pz)| {
                        line.points[start..i]
                            .iter()
                            .any(|&(x, z)| (x - px).hypot(z - pz) <= reach)
                    });
                if !stage {
                    return Vec::new();
                }
                cuts.push((start, i));
            }
        }
        // Each stretch starts on a point whose columns across were read.
        let stride = self.across(line.role).1;
        let mut bounds = vec![0];
        for &(a, b) in &cuts {
            bounds.extend([a, b - b % stride]);
        }
        bounds.push(n);
        bounds
            .chunks(2)
            .enumerate()
            .filter(|(_, c)| c[1] > c[0] + 1)
            .map(|(k, c)| Line {
                role: line.role,
                points: line.points[c[0]..c[1]].to_vec(),
                ways: line.ways[c[0]..c[1]].to_vec(),
                looped: false,
                ends: [
                    if c[0] == 0 { line.ends[0] } else { End::Buffer },
                    if c[1] == n { line.ends[1] } else { End::Buffer },
                ],
                seed: if k == 0 {
                    line.seed
                } else {
                    trains::hash(line.seed, 100 + k as u64)
                },
            })
            .collect()
    }

    fn ferry(
        &self,
        line: &Line,
        cars: bool,
        minutes: Option<u32>,
        water: &impl Fn(f64, f64) -> Option<Sample>,
    ) -> Option<(Route, Service)> {
        let pts = &line.points;
        let n = pts.len();
        let wet: Vec<bool> = pts.iter().map(|&(x, z)| floats(water(x, z))).collect();
        // From where the water starts at either end, over the slipway or the quay.
        let k = 12.min(n / 4).max(1);
        let firm = |range: std::ops::Range<usize>| {
            range.clone().filter(|&i| wet[i]).count() * 10 >= range.len() * 9
        };
        let start = (0..n.saturating_sub(k)).find(|&i| wet[i] && firm(i..i + k))?;
        let end = (k..n).rev().find(|&i| wet[i] && firm(i + 1 - k..i + 1))?;
        if end < start + 20 {
            return None;
        }
        let gap = (DRY_GAP as f64 * self.scale).ceil() as usize;
        let mut dry = 0;
        for &w in &wet[start..=end] {
            dry = if w { 0 } else { dry + 1 };
            if dry > gap {
                return None;
            }
        }
        let ends = [
            if start <= 2 && line.ends[0] == End::Edge {
                End::Edge
            } else {
                End::Buffer
            },
            if end + 3 >= n && line.ends[1] == End::Edge {
                End::Edge
            } else {
                End::Buffer
            },
        ];
        let span = end + 1 - start;
        let len_m = span as f64 / self.scale;
        let edge = ends.contains(&End::Edge);
        // A crossing mapped to take most of an hour is out to sea; a shorter one, however
        // wide the water, is a lake or fjord ferry.
        let sea = match minutes {
            Some(m) => m >= 45,
            None => len_m > 8000.0 || (edge && len_m > 3000.0),
        };
        let choices: &[Vessel] = match (cars, len_m) {
            (true, _) if sea => &[Vessel::SeaFerry, Vessel::CarFerry, Vessel::RiverFerry],
            (true, l) if l > 450.0 => &[Vessel::CarFerry, Vessel::RiverFerry],
            (true, _) => &[Vessel::RiverFerry, Vessel::WaterBus],
            (false, _) => &[Vessel::WaterBus],
        };
        let margin = (6.0 * self.scale).ceil() as usize;
        let (vessel, low) = choices.iter().find_map(|&v| {
            let length = v.size_m().0 * self.size;
            // Room to stand clear of both ends and still cross.
            if (span as f64) < 2.5 * length + 2.0 * margin as f64 {
                return None;
            }
            // The hull reaches to within the margin of either end, never further.
            self.fits(line, start + margin..end + 1 - margin, v, water)
        })?;
        let points = pts[start..=end].to_vec();
        let ys = levels(&points, false, water)?;
        let mut route = Route {
            mode: Mode::Water(vessel),
            shape: Shape::Shuttle,
            ends,
            caps: caps(&points, false, vessel, self.scale),
            ways: line.ways[start..=end].to_vec(),
            points,
            stops: Vec::new(),
            ys: Some(ys),
            lift: WATER_LIFT,
        };
        // Piers on the way; the ends are where it turns anyway.
        let clear = vessel.size_m().0 * self.size;
        let last = route.points.len() as f64 - 1.0;
        route.stops = trains::stops_on(&route, &self.piers, self.scale)
            .into_iter()
            .filter(|&s| s > clear && s < last - clear)
            .collect();
        let service = self.service(&route, vessel, low, line.seed, None)?;
        Some((route, service))
    }

    fn waterway(
        &self,
        line: &Line,
        class: u8,
        water: &impl Fn(f64, f64) -> Option<Sample>,
    ) -> Vec<(Route, Service)> {
        let pts = &line.points;
        let n = pts.len();
        let (offsets, stride) = self.across(line.role);
        // Across the water at every sample: the room over it at each offset either side,
        // 0 where there is bank or a bridge pier, -1 where something sits right on the water.
        let mut rows: Vec<[Vec<i32>; 2]> = Vec::new();
        let mut middle: Vec<bool> = Vec::new();
        for i in (0..n).step_by(stride) {
            let (x, z) = pts[i];
            let (nx, nz) = normal(pts, i, false);
            let side = |sign: f64| -> Vec<i32> {
                offsets
                    .iter()
                    .map(|&d| water(x + sign * nx * d, z + sign * nz * d))
                    .map(|s| {
                        s.and_then(|s| s.clearance())
                            .map_or(0, |c| if c >= 1 { c } else { -1 })
                    })
                    .collect()
            };
            middle.push(floats(water(x, z)));
            rows.push([side(1.0), side(-1.0)]);
        }
        let dry = middle.iter().filter(|&&m| !m).count();
        if rows.is_empty() || dry * 20 > rows.len() {
            return Vec::new();
        }
        let ys_middle = levels(pts, false, water);
        let mut out = Vec::new();
        for (dir, side) in [(1.0, 0usize), (-1.0, 1usize)] {
            let spans: Vec<Option<(f64, f64)>> = rows
                .iter()
                .zip(&middle)
                .map(|(r, &m)| widest(&r[side], &offsets, m))
                .collect();
            // Each stretch keeps to the water open all along a ship's length round it, so a
            // ship passes between the piers of a bridge; with the room left either side.
            let lane_for = |length: f64, half: f64| -> Vec<(f64, f64)> {
                let window = (length / 2.0 / stride as f64).ceil() as usize;
                let bands: Vec<Option<(f64, f64)>> = (0..spans.len())
                    .map(|k| {
                        let (a, b) = (k.saturating_sub(window), (k + window + 1).min(spans.len()));
                        let (inner, outer) =
                            spans[a..b].iter().fold((0.0, f64::MAX), |(i, o), s| {
                                s.map_or((f64::MAX, f64::MIN), |(si, so)| {
                                    (f64::max(i, si), f64::min(o, so))
                                })
                            });
                        (inner < outer).then_some((inner, outer))
                    })
                    .collect();
                let mut lane: Vec<f64> = bands
                    .iter()
                    .zip(&spans)
                    .map(|(band, span)| match (band, span) {
                        (Some((i, o)), _) => ((i + o) / 2.0).min(LANE_MAX_M * self.size).max(*i),
                        (None, Some((i, o))) => (i + o) / 2.0,
                        (None, None) => f64::NAN,
                    })
                    .collect();
                // Where no water is open the lane carries on from either side of it.
                let mut last = lane.iter().copied().find(|o| !o.is_nan()).unwrap_or(0.0);
                for o in &mut lane {
                    if o.is_nan() {
                        *o = last;
                    }
                    last = *o;
                }
                // A ship moves over gently, a block for every eight it goes on, and keeps
                // its hull clear of the piers while it does.
                let step = stride as f64 / 8.0;
                let keep = |k: usize, o: f64| match bands[k] {
                    Some((i, out)) if i + half <= out - half => o.clamp(i + half, out - half),
                    _ => o,
                };
                for _ in 0..3 {
                    for k in 1..lane.len() {
                        lane[k] = keep(k, lane[k].clamp(lane[k - 1] - step, lane[k - 1] + step));
                    }
                    for k in (0..lane.len().saturating_sub(1)).rev() {
                        lane[k] = keep(k, lane[k].clamp(lane[k + 1] - step, lane[k + 1] + step));
                    }
                }
                lane.iter()
                    .zip(&bands)
                    .map(|(&o, band)| (o, band.map_or(-1.0, |(i, out)| (o - i).min(out - o))))
                    .collect()
            };
            let seed = trains::hash(line.seed, side as u64);
            // The first vessel that fits and gets through the area in time to run.
            let sail = |v: Vessel| -> Option<(Route, Service)> {
                let half = v.size_m().1 * self.size / 2.0;
                let lane = lane_for(v.size_m().0 * self.size, half);
                // Wide enough everywhere but a stray sample or two, as at a bridge pier; a
                // ship's length at either end, where it comes in off the edge, may brush the bank.
                let ends = (v.size_m().0 * self.size / stride as f64).ceil() as usize;
                let inner = lane
                    .get(ends..lane.len().saturating_sub(ends))
                    .unwrap_or(&[]);
                let mut room: Vec<f64> = inner.iter().map(|&(_, r)| r - half).collect();
                room.sort_by(|a, b| a.total_cmp(b));
                if room.get(room.len() / 50).is_none_or(|&r| r < 1.0) {
                    return None;
                }
                let builds: &[bool] = if v.lowers() { &[false, true] } else { &[false] };
                let low = builds.iter().copied().find(|&low| {
                    let top = v.air_draft_m(low) * self.size + HEADROOM;
                    rows.iter().zip(&lane).all(|(row, &(o, _))| {
                        offsets
                            .iter()
                            .zip(&row[side])
                            .filter(|(&d, &c)| c != 0 && (d - o).abs() <= half + 0.5)
                            .all(|(_, &c)| f64::from(c) >= top)
                    })
                })?;
                let lane_at = |i: usize| {
                    let k = i / stride;
                    let t = (i % stride) as f64 / stride as f64;
                    let a = lane[k.min(lane.len() - 1)].0;
                    let b = lane[(k + 1).min(lane.len() - 1)].0;
                    a + (b - a) * t
                };
                let mut shifted: Vec<(f64, f64)> = (0..n)
                    .map(|i| {
                        let (nx, nz) = normal(pts, i, false);
                        let o = dir * lane_at(i);
                        (pts[i].0 + nx * o, pts[i].1 + nz * o)
                    })
                    .collect();
                let mut heights = ys_middle.clone().unwrap_or_default();
                if dir < 0.0 {
                    shifted.reverse();
                    heights.reverse();
                }
                let points = smooth(resample(&shifted, false), false);
                if heights.is_empty() || points.len() < 2 {
                    return None;
                }
                // Each point floats as high as the middle of the river beside it.
                let ys: Vec<f64> = (0..points.len())
                    .map(|j| heights[(j * (heights.len() - 1)) / (points.len() - 1).max(1)])
                    .collect();
                let route = Route {
                    mode: Mode::Water(v),
                    shape: Shape::Through,
                    ends: [End::Edge, End::Edge],
                    caps: caps(&points, false, v, self.scale),
                    ways: vec![line.seed; points.len()],
                    points,
                    stops: Vec::new(),
                    ys: Some(ys),
                    lift: WATER_LIFT,
                };
                let service = self.service(&route, v, low, seed, Some(headway_s(class)))?;
                Some((route, service))
            };
            out.extend(waterway_vessels(class, seed).into_iter().find_map(sail));
        }
        out
    }

    fn leisure(
        &self,
        line: &Line,
        water: &impl Fn(f64, f64) -> Option<Sample>,
    ) -> Option<(Route, Service)> {
        let choices: &[Vessel] = if trains::hash(line.seed, 1) % 100 < 60 {
            &[Vessel::Sailboat, Vessel::Motorboat]
        } else {
            &[Vessel::Motorboat]
        };
        let (vessel, low) = choices
            .iter()
            .find_map(|&v| self.fits(line, 0..line.points.len(), v, water))?;
        let ys = levels(&line.points, true, water)?;
        let route = Route {
            mode: Mode::Water(vessel),
            shape: Shape::Loop,
            ends: line.ends,
            caps: caps(&line.points, true, vessel, self.scale),
            ways: line.ways.clone(),
            points: line.points.clone(),
            stops: Vec::new(),
            ys: Some(ys),
            lift: WATER_LIFT,
        };
        let service = self.service(&route, vessel, low, line.seed, None)?;
        Some((route, service))
    }

    /// The first build of `v`, as it is or lowered, that floats along `range` of the line's
    /// points with water under the whole of its beam and room under everything over it, by
    /// the columns read across the line.
    fn fits(
        &self,
        line: &Line,
        range: std::ops::Range<usize>,
        v: Vessel,
        water: &impl Fn(f64, f64) -> Option<Sample>,
    ) -> Option<(Vessel, bool)> {
        let half = v.size_m().1 * self.size / 2.0;
        let (offsets, stride) = self.across(line.role);
        let across: Vec<f64> = std::iter::once(0.0)
            .chain(offsets.into_iter().filter(|&d| d <= half + 1e-6))
            .collect();
        let builds: &[bool] = if v.lowers() { &[false, true] } else { &[false] };
        'build: for &low in builds {
            let top = v.air_draft_m(low) * self.size + HEADROOM;
            let (mut checked, mut missed) = (0usize, 0usize);
            for i in range.clone() {
                let (x, z) = line.points[i];
                let (nx, nz) = normal(&line.points, i, line.looped);
                let reach = if i % stride == 0 {
                    &across[..]
                } else {
                    &across[..1]
                };
                for &d in reach {
                    for sign in [1.0, -1.0] {
                        if d == 0.0 && sign < 0.0 {
                            continue;
                        }
                        checked += 1;
                        match water(x + sign * nx * d, z + sign * nz * d)
                            .and_then(|s| s.clearance())
                        {
                            // A bridge too low anywhere is no way through.
                            Some(c) if f64::from(c) < top => continue 'build,
                            Some(_) => {}
                            // The few dry columns `ferry` lets by, or a quay under the edge.
                            None => missed += 1,
                        }
                    }
                }
            }
            // The odd column of a quay or a jetty under the hull's edge.
            if missed * 30 <= checked {
                return Some((v, low));
            }
        }
        None
    }

    /// What sails a route: `headway` apart in seconds, or one boat.
    fn service(
        &self,
        route: &Route,
        vessel: Vessel,
        low: bool,
        seed: u64,
        headway: Option<f64>,
    ) -> Option<Service> {
        let length = vessel.size_m().0 * self.size;
        let body = Body {
            front: length / 2.0,
            back: length / 2.0,
            span: 0.0,
        };
        let (timetable, period) = trains::timetable(route, &body, self.scale)?;
        let t = timetable.len();
        let n = match headway {
            Some(h) => {
                let h = (h * trains::STEPS_PER_S) as usize;
                let room = (length + 40.0 * self.scale) * 1000.0;
                let mut n = ((t + h / 2) / h).max(1);
                while n > 1 && trains::closest(&timetable, period, n) < room {
                    n -= 1;
                }
                n
            }
            None => 1,
        };
        let liveries = vessel.liveries();
        let start = (trains::hash(seed, 7) % t as u64) as usize;
        let boats = (0..n)
            .map(|k| {
                let roll = trains::hash(seed, 11 + k as u64) as usize;
                ((start + k * t / n) % t, liveries[roll % liveries.len()])
            })
            .collect();
        Some(Service {
            route: 0,
            coaches: vec![Coach::Ship { vessel, low }],
            offsets: vec![0.0],
            timetable,
            period,
            trains: boats,
        })
    }
}

fn block(x: f64, z: f64) -> (i32, i32) {
    (x.floor() as i32, z.floor() as i32)
}

/// Whether a column holds water a boat can float on.
/// The widest stretch of open water on one side of a waterway, as its nearest and farthest
/// offsets; from the middle itself where that is open and the stretch starts beside it.
fn widest(room: &[i32], offsets: &[f64], middle: bool) -> Option<(f64, f64)> {
    let mut best: Option<(f64, f64)> = None;
    let mut start = None;
    for k in 0..=room.len() {
        match (room.get(k).is_some_and(|&c| c > 0), start) {
            (true, None) => start = Some(k),
            (false, Some(a)) => {
                let inner = if a == 0 && middle { 0.0 } else { offsets[a] };
                let span = (inner, offsets[k - 1]);
                if best.is_none_or(|b| span.1 - span.0 > b.1 - b.0) {
                    best = Some(span);
                }
                start = None;
            }
            _ => {}
        }
    }
    best
}

fn floats(s: Option<Sample>) -> bool {
    s.and_then(|s| s.clearance()).is_some_and(|c| c >= 1)
}

/// How far across a waterway its width is looked for, every two metres out to the reach.
fn channel_offsets(size: f64) -> Vec<f64> {
    let step = (2.0 * size).max(1.0);
    let n = (CHANNEL_M * size / step).floor() as usize;
    (1..=n).map(|k| k as f64 * step).collect()
}

/// A mapped duration, as `HH:MM`, `HH:MM:SS` or minutes, in minutes.
fn minutes(value: &str) -> Option<u32> {
    let parts: Vec<u32> = value
        .split(':')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    match parts[..] {
        [m] => Some(m),
        [h, m] | [h, m, _] => h.checked_mul(60)?.checked_add(m),
        _ => None,
    }
}

fn ferry_cars(tags: &HashMap<String, String>) -> Option<bool> {
    let get = |k: &str| tags.get(k).map(String::as_str);
    if get("route") != Some("ferry")
        || get("disused") == Some("yes")
        || get("abandoned") == Some("yes")
        || tags.contains_key("highway")
        || tags.contains_key("railway")
    {
        return None;
    }
    let yes = |k: &str| matches!(get(k), Some("yes" | "designated"));
    Some(yes("motor_vehicle") || yes("motorcar") || yes("hgv") || yes("vehicle"))
}

fn is_terminal(tags: &HashMap<String, String>) -> bool {
    let get = |k: &str| tags.get(k).map(String::as_str);
    get("amenity") == Some("ferry_terminal")
        || get("ferry") == Some("yes")
            && matches!(
                get("public_transport"),
                Some("platform" | "stop_position" | "station")
            )
}

/// The CEMT class of ship a waterway takes, 0 where only smaller boats go, or None.
fn waterway_class(tags: &HashMap<String, String>) -> Option<u8> {
    let get = |k: &str| tags.get(k).map(String::as_str);
    if !matches!(get("waterway")?, "river" | "canal" | "fairway")
        || matches!(get("tunnel"), Some(v) if v != "no")
        || get("lock") == Some("yes")
        || get("intermittent") == Some("yes")
    {
        return None;
    }
    let no = |k: &str| matches!(get(k), Some("no" | "private"));
    let yes = |k: &str| matches!(get(k), Some("yes" | "designated" | "permissive"));
    if no("ship") && no("boat") || no("motorboat") && !yes("ship") {
        return None;
    }
    if let Some(c) = get("CEMT") {
        let c = c.trim().to_ascii_uppercase();
        let class = if c.starts_with("VII") {
            7
        } else if c.starts_with("VI") {
            6
        } else if c.starts_with('V') {
            5
        } else if c.starts_with("IV") {
            4
        } else if c.starts_with("III") {
            3
        } else if c.starts_with("II") {
            2
        } else if c.starts_with('I') {
            1
        } else {
            0
        };
        return Some(class);
    }
    if get("waterway") == Some("fairway") {
        return Some(4);
    }
    if yes("ship") {
        return Some(3);
    }
    (yes("boat") || yes("motorboat")).then_some(0)
}

/// What sails a waterway of `class`, largest first, then smaller where it is too narrow.
fn waterway_vessels(class: u8, seed: u64) -> Vec<Vessel> {
    let roll = seed % 100;
    let mut out = match class {
        4.. => vec![match roll {
            0..=39 => Vessel::Barge,
            40..=69 => Vessel::ContainerBarge,
            70..=87 => Vessel::TankBarge,
            _ => Vessel::RiverCruise,
        }],
        1..=3 => vec![if roll < 70 {
            Vessel::Peniche
        } else {
            Vessel::TourBoat
        }],
        0 => vec![if roll < 75 {
            Vessel::TourBoat
        } else {
            Vessel::Motorboat
        }],
    };
    for v in [Vessel::Peniche, Vessel::TourBoat, Vessel::Motorboat] {
        if !out.contains(&v) && (class > 0 || v != Vessel::Peniche) {
            out.push(v);
        }
    }
    out
}

fn headway_s(class: u8) -> f64 {
    match class {
        6.. => 300.0,
        5 => 420.0,
        4 => 600.0,
        1..=3 => 1200.0,
        0 => 900.0,
    }
}

/// A way's nodes, cut where a weir, a dam, a waterfall or a lock blocks it.
fn split_at_barriers(way: &ProcessedWay) -> Vec<Nodes> {
    let mut out = Vec::new();
    let mut run: Nodes = Vec::new();
    for n in &way.nodes {
        let barrier = matches!(
            n.tags.get("waterway").map(String::as_str),
            Some("weir" | "dam" | "waterfall" | "lock_gate" | "rapids")
        ) || n.tags.get("lock").map(String::as_str) == Some("yes");
        run.push((n.id, n.x as f64 + 0.5, n.z as f64 + 0.5));
        if barrier {
            if run.len() > 1 {
                out.push(std::mem::take(&mut run));
            }
            run.clear();
        }
    }
    if run.len() > 1 {
        out.push(run);
    }
    out
}

fn centroid(nodes: &Nodes) -> (f64, f64) {
    let n = nodes.len().max(1) as f64;
    (
        nodes.iter().map(|p| p.1).sum::<f64>() / n,
        nodes.iter().map(|p| p.2).sum::<f64>() / n,
    )
}

/// Ways joined end to end into one line.
struct Chain {
    nodes: Nodes,
    way: u64,
    /// Every way joined into it.
    ways: Vec<u64>,
    tag: u64,
}

impl Chain {
    fn points(&self) -> Vec<(f64, f64)> {
        self.nodes.iter().map(|n| (n.1, n.2)).collect()
    }
}

/// Joins ways of the same tag that meet end to end, through every node the straightest
/// way on within `max_turn` degrees, into lines.
fn chains(ways: impl Iterator<Item = (Nodes, u64, u64)>, max_turn: f64) -> Vec<Chain> {
    let ways: Vec<(Nodes, u64, u64)> = ways.filter(|w| w.0.len() >= 2).collect();
    let mut at: FnvHashMap<u64, Vec<(usize, bool)>> = FnvHashMap::default();
    for (i, (nodes, _, _)) in ways.iter().enumerate() {
        at.entry(nodes[0].0).or_default().push((i, true));
        at.entry(nodes[nodes.len() - 1].0)
            .or_default()
            .push((i, false));
    }
    let heading = |nodes: &Nodes, from_start: bool| {
        let (a, b) = if from_start {
            (nodes[0], nodes[1.min(nodes.len() - 1)])
        } else {
            (nodes[nodes.len() - 1], nodes[nodes.len() - 2])
        };
        trains::unit(b.1 - a.1, b.2 - a.2).unwrap_or((1.0, 0.0))
    };
    let mut used = vec![false; ways.len()];
    let mut out = Vec::new();
    // Lines are followed in from their loose ends first, so each starts at one.
    let mut order: Vec<usize> = (0..ways.len()).collect();
    order.sort_by_key(|&i| {
        let nodes = &ways[i].0;
        let loose = at[&nodes[0].0].len() == 1 || at[&nodes[nodes.len() - 1].0].len() == 1;
        (!loose, ways[i].1)
    });
    for start in order {
        if used[start] {
            continue;
        }
        used[start] = true;
        let (first, way, tag) = &ways[start];
        let mut nodes = first.clone();
        let mut joined = vec![*way];
        if at[&nodes[nodes.len() - 1].0].len() == 1 && at[&nodes[0].0].len() > 1 {
            nodes.reverse();
        }
        // Forwards from the end, then backwards from the start.
        for _ in 0..2 {
            loop {
                let end = nodes[nodes.len() - 1];
                let inc = heading(&nodes, false);
                let inc = (-inc.0, -inc.1);
                let next = at
                    .get(&end.0)
                    .into_iter()
                    .flatten()
                    .filter(|&&(q, _)| !used[q] && ways[q].2 == *tag)
                    .map(|&(q, starts)| {
                        let out = heading(&ways[q].0, starts);
                        (q, starts, trains::angle_deg(inc, out))
                    })
                    .filter(|&(_, _, turn)| turn <= max_turn)
                    .min_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)));
                let Some((q, starts, _)) = next else {
                    break;
                };
                used[q] = true;
                joined.push(ways[q].1);
                let mut more = ways[q].0.clone();
                if !starts {
                    more.reverse();
                }
                nodes.extend(more.into_iter().skip(1));
            }
            nodes.reverse();
        }
        out.push(Chain {
            nodes,
            way: *way,
            ways: joined,
            tag: *tag,
        });
    }
    out
}

/// Points a block apart along a line, or round it when `looped`.
fn resample(line: &[(f64, f64)], looped: bool) -> Vec<(f64, f64)> {
    let mut line: Vec<(f64, f64)> = line.to_vec();
    line.dedup_by(|a, b| (a.0 - b.0).hypot(a.1 - b.1) < 1e-6);
    if looped {
        if let Some(&first) = line.first() {
            line.push(first);
        }
    }
    if line.len() < 2 {
        return line;
    }
    let total: f64 = line
        .windows(2)
        .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
        .sum();
    let count = if looped {
        total.round().max(1.0) as usize
    } else {
        total.floor() as usize + 1
    };
    let spacing = if looped { total / count as f64 } else { 1.0 };
    let mut out = Vec::with_capacity(count);
    let (mut seg, mut start) = (0usize, 0.0f64);
    for k in 0..count {
        let s = k as f64 * spacing;
        loop {
            let (a, b) = (line[seg], line[seg + 1]);
            let len = (b.0 - a.0).hypot(b.1 - a.1);
            if s <= start + len || seg + 2 >= line.len() {
                let t = if len > 0.0 {
                    ((s - start) / len).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
                break;
            }
            start += len;
            seg += 1;
        }
    }
    out
}

/// Rounds a line's corners, which boats sail round rather than turn at.
fn smooth(points: Vec<(f64, f64)>, looped: bool) -> Vec<(f64, f64)> {
    const REACH: isize = 4;
    let n = points.len() as isize;
    if n < 2 * REACH + 2 {
        return points;
    }
    (0..n)
        .map(|i| {
            if !looped && (i < REACH || i >= n - REACH) {
                return points[i as usize];
            }
            let (mut x, mut z) = (0.0, 0.0);
            for d in -REACH..=REACH {
                let q = points[(i + d).rem_euclid(n) as usize];
                x += q.0;
                z += q.1;
            }
            let k = (2 * REACH + 1) as f64;
            (x / k, z / k)
        })
        .collect()
}

/// The unit vector to the right of a line at point `i`, from its heading a few points
/// either way.
fn normal(points: &[(f64, f64)], i: usize, looped: bool) -> (f64, f64) {
    let n = points.len();
    let at = |d: isize| {
        let j = i as isize + d;
        if looped {
            points[j.rem_euclid(n as isize) as usize]
        } else {
            points[j.clamp(0, n as isize - 1) as usize]
        }
    };
    let (a, b) = (at(-3), at(3));
    let (dx, dz) = trains::unit(b.0 - a.0, b.1 - a.1).unwrap_or((1.0, 0.0));
    // East is +x and south +z, so the right of a heading (dx, dz) is (-dz, dx).
    (-dz, dx)
}

/// The water block each point floats on, gaps filled from either side and eased over a
/// few points so a boat rides a river's steps smoothly. None with no water anywhere.
fn levels(
    points: &[(f64, f64)],
    looped: bool,
    water: &impl Fn(f64, f64) -> Option<Sample>,
) -> Option<Vec<f64>> {
    let raw: Vec<Option<f64>> = points
        .iter()
        .map(|&(x, z)| water(x, z).and_then(|s| s.water()).map(f64::from))
        .collect();
    let first = raw.iter().flatten().next().copied()?;
    let mut filled = Vec::with_capacity(raw.len());
    let mut last = first;
    for y in &raw {
        last = y.unwrap_or(last);
        filled.push(last);
    }
    const REACH: isize = 5;
    let n = filled.len() as isize;
    Some(
        (0..n)
            .map(|i| {
                let (mut sum, mut count) = (0.0, 0.0);
                for d in -REACH..=REACH {
                    let j = i + d;
                    let j = if looped {
                        j.rem_euclid(n)
                    } else if j < 0 || j >= n {
                        continue;
                    } else {
                        j
                    };
                    sum += filled[j as usize];
                    count += 1.0;
                }
                sum / count
            })
            .collect(),
    )
}

/// The speed each point allows, in blocks a step: the boat's own, or less round a bend
/// tighter than it turns at that speed.
fn caps(points: &[(f64, f64)], looped: bool, vessel: Vessel, scale: f64) -> Vec<f64> {
    let n = points.len();
    let (_, _, lateral) = vessel.accel();
    let reach = ((30.0 * scale).round() as isize).clamp(3, 60);
    let heading = |i: usize| -> Option<(f64, f64)> {
        let j = if looped { (i + 1) % n } else { i + 1 };
        (j < n)
            .then(|| trains::unit(points[j].0 - points[i].0, points[j].1 - points[i].1))
            .flatten()
    };
    let turns: Vec<f64> = (0..n)
        .map(|i| {
            let prev = match i {
                0 if looped => heading(n - 1),
                0 => None,
                _ => heading(i - 1),
            };
            match (prev, heading(i)) {
                (Some(u), Some(v)) => (u.0 * v.1 - u.1 * v.0).atan2(u.0 * v.0 + u.1 * v.1),
                _ => 0.0,
            }
        })
        .collect();
    (0..n)
        .map(|i| {
            let mut kmh = vessel.cruise_kmh();
            let mut turn = 0.0f64;
            for d in -reach..=reach {
                let j = i as isize + d;
                if looped {
                    turn += turns[j.rem_euclid(n as isize) as usize];
                } else if j >= 0 && (j as usize) < n {
                    turn += turns[j as usize];
                }
            }
            if turn.abs() > 1e-3 {
                let radius_m = (2 * reach + 1) as f64 / turn.abs() / scale;
                kmh = kmh.min(((lateral * radius_m).sqrt() * 3.6).max(3.0));
            }
            trains::step_speed(kmh, scale)
        })
        .collect()
}

/// Loops for leisure boats on open water: on lakes and along the coast, clear of the
/// shore, of rivers and of the routes ferries and ships take, each as a closed line.
fn leisure_loops(
    world: &XZBBox,
    scale: f64,
    is_water: &impl Fn(i32, i32) -> bool,
    rivers: &[Nodes],
    routes: &[&[(f64, f64)]],
) -> Vec<Vec<(f64, f64)>> {
    let (x0, z0) = (world.min_x(), world.min_z());
    let w = ((world.max_x() - x0) / CELL + 1).max(1) as usize;
    let h = ((world.max_z() - z0) / CELL + 1).max(1) as usize;
    if w.saturating_mul(h) > 40_000_000 {
        return Vec::new();
    }
    let centre = |cx: usize, cz: usize| {
        (
            x0 + cx as i32 * CELL + CELL / 2,
            z0 + cz as i32 * CELL + CELL / 2,
        )
    };
    let mut open: Vec<bool> = (0..w * h)
        .map(|c| {
            let (x, z) = centre(c % w, c / w);
            x <= world.max_x() && z <= world.max_z() && is_water(x, z)
        })
        .collect();
    if !open.iter().any(|&o| o) {
        return Vec::new();
    }
    // Rivers flow and ferries and ships come through; boats stay out of their way.
    let mut keep_off = |line: &[(f64, f64)], reach: f64| {
        let r = (reach / CELL as f64).ceil() as i32;
        let step = (reach / 2.0).max(CELL as f64);
        for pair in line.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let len = (b.0 - a.0).hypot(b.1 - a.1);
            let steps = (len / step).ceil().max(1.0) as usize;
            for s in 0..=steps {
                let t = s as f64 / steps as f64;
                let (x, z) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                let (cx, cz) = (
                    ((x - x0 as f64) / CELL as f64).floor() as i32,
                    ((z - z0 as f64) / CELL as f64).floor() as i32,
                );
                for dz in -r..=r {
                    for dx in -r..=r {
                        let (px, pz) = (cx + dx, cz + dz);
                        if dx * dx + dz * dz > r * r
                            || px < 0
                            || pz < 0
                            || px >= w as i32
                            || pz >= h as i32
                        {
                            continue;
                        }
                        open[pz as usize * w + px as usize] = false;
                    }
                }
            }
        }
    };
    for river in rivers {
        let line: Vec<(f64, f64)> = river.iter().map(|n| (n.1, n.2)).collect();
        keep_off(&line, KEEP_OFF_RIVER_M * scale);
    }
    for route in routes {
        keep_off(route, KEEP_OFF_ROUTE_M * scale);
    }
    // Distance to land in cells, the edge of the area counting as land.
    let mut dist = vec![u16::MAX / 2; w * h];
    for (c, d) in dist.iter_mut().enumerate() {
        let (cx, cz) = (c % w, c / w);
        if !open[c] || cx == 0 || cz == 0 || cx + 1 == w || cz + 1 == h {
            *d = 0;
        }
    }
    // Two passes of a 3-4 chamfer: 3 a step along, 4 a step across.
    for pass in 0..2 {
        let cells: Box<dyn Iterator<Item = usize>> = if pass == 0 {
            Box::new(0..w * h)
        } else {
            Box::new((0..w * h).rev())
        };
        for c in cells {
            let (cx, cz) = ((c % w) as isize, (c / w) as isize);
            let s: isize = if pass == 0 { -1 } else { 1 };
            let mut best = dist[c];
            for (dx, dz, cost) in [(s, 0, 3), (0, s, 3), (s, s, 4), (-s, s, 4)] {
                let (px, pz) = (cx + dx, cz + dz);
                if px < 0 || pz < 0 || px >= w as isize || pz >= h as isize {
                    continue;
                }
                best = best.min(dist[pz as usize * w + px as usize].saturating_add(cost));
            }
            dist[c] = best;
        }
    }
    let metres = |c: usize| dist[c] as f64 / 3.0 * CELL as f64 / scale;
    let (r_min, r_max) = LOOP_RADIUS_M;
    let fits = |c: usize| open[c] && metres(c) >= r_min + SHORE_M;
    let count = (0..w * h).filter(|&c| fits(c)).count();
    let area_km2 = count as f64 * (CELL as f64 / scale).powi(2) / 1e6;
    let wanted = ((area_km2 * BOATS_PER_KM2).round() as usize).min(MAX_LEISURE);
    // A sample of a sea is plenty to place a few dozen boats from.
    let every = (count / (wanted.max(1) * 64)).max(1) as u64;
    let mut candidates: Vec<usize> = (0..w * h)
        .filter(|&c| fits(c) && trains::hash(c as u64, 0x5A3).is_multiple_of(every))
        .collect();
    candidates.sort_by_key(|&c| trains::hash(c as u64, 0xB0A7));
    let mut placed: Vec<((f64, f64), f64)> = Vec::new();
    let mut out = Vec::new();
    for c in candidates {
        if placed.len() >= wanted {
            break;
        }
        let (x, z) = centre(c % w, c / w);
        let at = (x as f64, z as f64);
        let r = (metres(c) - SHORE_M).min(r_max) * scale;
        let clear = placed
            .iter()
            .all(|&(p, q)| (p.0 - at.0).hypot(p.1 - at.1) >= r + q + 20.0 * scale);
        if !clear {
            continue;
        }
        placed.push((at, r));
        let roll = trains::hash(c as u64, 0x100F);
        let phase = |k: u64| ((roll >> (8 * k)) & 0xff) as f64 / 255.0 * std::f64::consts::TAU;
        let stretch = 0.6 + 0.4 * ((roll >> 40) & 0xff) as f64 / 255.0;
        let turn = phase(3);
        let (sin, cos) = turn.sin_cos();
        let mut ring: Vec<(f64, f64)> = (0..360)
            .map(|k| {
                let t = (k as f64).to_radians();
                let rho = r
                    * (0.78
                        + 0.14 * (2.0 * t + phase(1)).sin()
                        + 0.08 * (3.0 * t + phase(2)).sin());
                let (u, v) = (rho * t.cos(), rho * stretch * t.sin());
                (at.0 + u * cos - v * sin, at.1 + u * sin + v * cos)
            })
            .collect();
        if roll & 1 == 0 {
            ring.reverse();
        }
        let points = resample(&ring, true);
        if points.len() >= 40 {
            out.push(points);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm_parser::ProcessedNode;

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
        XZBBox::rect_from_min_max(0, 0, 1999, 1999).unwrap()
    }

    /// Water at y 62 inside `wet`, with a deck at `deck` over the columns `bridge` says.
    fn probe_of(
        plan: &ShipPlan,
        wet: impl Fn(i32, i32) -> bool,
        bridge: impl Fn(i32, i32) -> Option<i32>,
    ) -> WaterProbe {
        let probe = plan.probe();
        let regions: Vec<(i32, i32)> = probe.regions().collect();
        for (rx, rz) in regions {
            probe.read_region(rx, rz, |x, z, _| {
                let spot = |y: i32| {
                    if bridge(x, z) == Some(y) {
                        crate::water_probe::Spot::Solid
                    } else if wet(x, z) && y <= 62 {
                        crate::water_probe::Spot::Water
                    } else if !wet(x, z) && y <= 63 {
                        crate::water_probe::Spot::Solid
                    } else {
                        crate::water_probe::Spot::Open
                    }
                };
                crate::water_probe::water_column(spot, 62, 200)
            });
        }
        probe
    }

    fn ferry_across_a_lake(cars: &str) -> Vec<ProcessedElement> {
        vec![
            way(
                1,
                vec![node(10, 200, 1000, &[]), node(11, 1800, 1000, &[])],
                &[("route", "ferry"), ("motor_vehicle", cars)],
            ),
            ProcessedElement::Node(node(20, 990, 1003, &[("amenity", "ferry_terminal")])),
        ]
    }

    #[test]
    fn a_line_over_a_landing_stage_sails_each_stretch_of_water() {
        let elements = vec![way(
            1,
            vec![node(10, 200, 1000, &[]), node(11, 1800, 1000, &[])],
            &[("route", "ferry")],
        )];
        let mut elements = elements;
        elements.push(ProcessedElement::Node(node(
            20,
            1000,
            1000,
            &[("amenity", "ferry_terminal")],
        )));
        let plan = ShipPlan::build(&elements, 1.0, &world(), |_, _| false);
        // A pontoon twenty blocks long right where the line runs.
        let lake = |x: i32, z: i32| {
            (300..1700).contains(&x) && (800..1200).contains(&z) && !(990..1011).contains(&x)
        };
        let ships = plan.finish(&probe_of(&plan, lake, |_, _| None));
        assert_eq!(ships.len(), 2);
        for (route, _) in &ships {
            let xs = route.points.iter().map(|p| p.0);
            let (lo, hi) = (
                xs.clone().fold(f64::MAX, f64::min),
                xs.fold(f64::MIN, f64::max),
            );
            assert!(hi < 990.0 || lo > 1011.0, "{lo} {hi}");
        }
    }

    #[test]
    fn a_mapped_crossing_time_tells_a_lake_ferry_from_a_sea_ferry() {
        assert_eq!(minutes("00:15"), Some(15));
        assert_eq!(minutes("1:40"), Some(100));
        assert_eq!(minutes("04:55:00"), Some(295));
        assert_eq!(minutes("15"), Some(15));
        assert_eq!(minutes("soon"), None);
        // Five kilometres of open lake, crossed in a quarter of an hour or in two hours.
        let lake = |x: i32, z: i32| (0..6000).contains(&x) && (700..1300).contains(&z);
        let world = XZBBox::rect_from_min_max(-2000, -2000, 8000, 3000).unwrap();
        for (duration, sea) in [(Some("00:15"), false), (None, false), (Some("02:00"), true)] {
            let mut tags = vec![("route", "ferry"), ("motor_vehicle", "yes")];
            tags.extend(duration.map(|d| ("duration", d)));
            let elements = vec![way(
                1,
                vec![node(10, -100, 1000, &[]), node(11, 5100, 1000, &[])],
                &tags,
            )];
            let plan = ShipPlan::build(&elements, 1.0, &world, |_, _| false);
            let ships = plan.finish(&probe_of(&plan, lake, |_, _| None));
            let vessel = match ships[0].0.mode {
                Mode::Water(v) => v,
                _ => panic!(),
            };
            assert_eq!(vessel == Vessel::SeaFerry, sea, "{duration:?} {vessel:?}");
        }
    }

    #[test]
    fn a_ferry_sails_from_where_the_water_starts_and_calls_at_the_pier_on_the_way() {
        let elements = ferry_across_a_lake("yes");
        let plan = ShipPlan::build(&elements, 1.0, &world(), |_, _| false);
        let lake = |x: i32, z: i32| (300..1700).contains(&x) && (800..1200).contains(&z);
        let ships = plan.finish(&probe_of(&plan, lake, |_, _| None));
        assert_eq!(ships.len(), 1);
        let (route, service) = &ships[0];
        assert!(
            matches!(route.mode, Mode::Water(Vessel::CarFerry)),
            "{:?}",
            route.mode
        );
        assert_eq!(route.ends, [End::Buffer, End::Buffer]);
        let (first, last) = (route.points[0], route.points[route.points.len() - 1]);
        assert!((299.0..310.0).contains(&first.0), "{first:?}");
        assert!((1690.0..1701.0).contains(&last.0), "{last:?}");
        assert_eq!(route.stops.len(), 1);
        assert!((route.points[route.stops[0] as usize].0 - 990.5).abs() < 3.0);
        assert!(route.ys.as_ref().unwrap().iter().all(|&y| y == 62.0));
        assert_eq!(service.trains.len(), 1);
        assert!(matches!(
            service.coaches[0],
            Coach::Ship {
                vessel: Vessel::CarFerry,
                ..
            }
        ));
    }

    #[test]
    fn two_lines_on_one_crossing_get_one_ferry() {
        let mut elements = ferry_across_a_lake("no");
        elements.push(way(
            2,
            vec![node(30, 200, 1006, &[]), node(31, 1800, 1006, &[])],
            &[("route", "ferry")],
        ));
        let plan = ShipPlan::build(&elements, 1.0, &world(), |_, _| false);
        let lake = |x: i32, z: i32| (300..1700).contains(&x) && (800..1200).contains(&z);
        assert_eq!(plan.finish(&probe_of(&plan, lake, |_, _| None)).len(), 1);
    }

    #[test]
    fn a_ferry_without_water_under_its_line_or_with_land_in_the_way_does_not_sail() {
        let elements = ferry_across_a_lake("no");
        let plan = ShipPlan::build(&elements, 1.0, &world(), |_, _| false);
        assert!(plan
            .finish(&probe_of(&plan, |_, _| false, |_, _| None))
            .is_empty());
        // An isthmus across it, away from the pier.
        let split = |x: i32, z: i32| {
            (300..1700).contains(&x) && !(1290..1310).contains(&x) && (800..1200).contains(&z)
        };
        assert!(plan.finish(&probe_of(&plan, split, |_, _| None)).is_empty());
    }

    #[test]
    fn a_waterway_runs_each_way_on_its_own_side_and_lowers_for_a_low_bridge() {
        let elements = vec![way(
            1,
            vec![node(10, 0, 1000, &[]), node(11, 1999, 1000, &[])],
            &[("waterway", "river"), ("CEMT", "Vb")],
        )];
        let plan = ShipPlan::build(&elements, 1.0, &world(), |_, _| false);
        let river = |_: i32, z: i32| (960..1041).contains(&z);
        // A bridge deck 6 blocks over the water at x 1000.
        let low = |x: i32, _: i32| (995..1005).contains(&x).then_some(69);
        let ships = plan.finish(&probe_of(&plan, river, low));
        assert_eq!(ships.len(), 2, "both ways");
        for (route, service) in &ships {
            assert_eq!(route.shape, Shape::Through);
            let Coach::Ship { vessel, low } = service.coaches[0] else {
                panic!()
            };
            assert!(vessel.air_draft_m(low) + HEADROOM < 6.0, "{vessel:?} {low}");
            let mid = route.points[route.points.len() / 2];
            let (a, b) = (route.points[0], route.points[route.points.len() - 1]);
            // Eastbound keeps south of the middle, westbound north.
            if b.0 > a.0 {
                assert!(mid.1 > 1005.0, "{mid:?}");
            } else {
                assert!(mid.1 < 996.0, "{mid:?}");
            }
        }
        // Too low for anything.
        let lower = |x: i32, _: i32| (995..1005).contains(&x).then_some(63);
        assert!(plan.finish(&probe_of(&plan, river, lower)).is_empty());
        // A pier right where the river is mapped is passed on either side.
        let piered =
            |x: i32, z: i32| river(x, z) && !((990..1010).contains(&x) && (994..1007).contains(&z));
        let high = |x: i32, _: i32| (990..1010).contains(&x).then_some(75);
        let ships = plan.finish(&probe_of(&plan, piered, high));
        assert_eq!(ships.len(), 2, "both ways past the pier");
        for (route, _) in &ships {
            for p in route
                .points
                .iter()
                .filter(|p| (985.0..1015.0).contains(&p.0))
            {
                assert!(!(990.0..1011.0).contains(&p.1), "{p:?}");
            }
        }
    }

    #[test]
    fn a_waterway_that_ends_at_a_weir_inside_the_area_carries_no_ships() {
        let elements = vec![way(
            1,
            vec![
                node(10, 0, 1000, &[]),
                node(12, 900, 1000, &[("waterway", "weir")]),
                node(11, 1999, 1000, &[]),
            ],
            &[("waterway", "canal"), ("CEMT", "IV")],
        )];
        let plan = ShipPlan::build(&elements, 1.0, &world(), |_, _| false);
        assert!(plan.is_empty());
    }

    #[test]
    fn leisure_boats_loop_on_a_lake_clear_of_its_shore() {
        let lake = |x: i32, z: i32| (x - 1000).pow(2) + (z - 1000).pow(2) < 700 * 700;
        let plan = ShipPlan::build(&[], 1.0, &world(), lake);
        assert!(!plan.is_empty());
        let ships = plan.finish(&probe_of(&plan, lake, |_, _| None));
        assert!(ships.len() >= 3, "{}", ships.len());
        for (route, _) in &ships {
            assert_eq!(route.shape, Shape::Loop);
            for &(x, z) in &route.points {
                let r = (x - 1000.0).hypot(z - 1000.0);
                assert!(r < 700.0 - SHORE_M, "{x} {z}");
            }
        }
    }

    #[test]
    fn navigability_comes_from_the_tags() {
        let tags = |kv: &[(&str, &str)]| -> HashMap<String, String> {
            kv.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert_eq!(
            waterway_class(&tags(&[("waterway", "river"), ("CEMT", "VIb")])),
            Some(6)
        );
        assert_eq!(
            waterway_class(&tags(&[("waterway", "canal"), ("CEMT", "IV")])),
            Some(4)
        );
        assert_eq!(
            waterway_class(&tags(&[("waterway", "river"), ("boat", "yes")])),
            Some(0)
        );
        assert_eq!(waterway_class(&tags(&[("waterway", "river")])), None);
        assert_eq!(
            waterway_class(&tags(&[("waterway", "stream"), ("boat", "yes")])),
            None
        );
        assert_eq!(
            waterway_class(&tags(&[
                ("waterway", "river"),
                ("ship", "no"),
                ("boat", "no")
            ])),
            None
        );
        assert_eq!(
            ferry_cars(&tags(&[("route", "ferry"), ("motorcar", "yes")])),
            Some(true)
        );
        assert_eq!(ferry_cars(&tags(&[("route", "ferry")])), Some(false));
        assert_eq!(ferry_cars(&tags(&[("route", "bus")])), None);
    }

    #[test]
    fn lines_join_end_to_end_and_resample_a_block_apart() {
        let a: Nodes = vec![(1, 0.5, 0.5), (2, 10.5, 0.5)];
        let b: Nodes = vec![(3, 20.5, 0.5), (2, 10.5, 0.5)];
        let joined = chains([(a, 1, 0), (b, 2, 0)].into_iter(), 40.0);
        assert_eq!(joined.len(), 1);
        let points = resample(&joined[0].points(), false);
        assert_eq!(points.len(), 21);
        for w in points.windows(2) {
            assert!(((w[1].0 - w[0].0).hypot(w[1].1 - w[0].1) - 1.0).abs() < 1e-9);
        }
    }
}
