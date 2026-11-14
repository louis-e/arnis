//! Signal heads built from block displays, one per approach, so the extras data pack can
//! cycle their lamps. The tags written here are the contract with
//! `assets/minecraft/datapack_extras/data/arnis/function/traffic`.

use crate::block_definitions::{COBBLESTONE_WALL, IRON_BARS};
use crate::element_processing::cars::drivable;
use crate::element_processing::highways::highway_block_range;
use crate::element_processing::road_markings::oneway_sign;
use crate::floodfill_cache::RoadMaskBitmap;
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use fnv::{FnvHashMap, FnvHashSet};
use std::collections::HashMap;

/// One direction a road leaves a signal node in.
#[derive(Clone, Copy, Debug)]
struct Arm {
    /// Unit vector from the node out along the road.
    dir: (f64, f64),
    half: i32,
    /// Traffic drives towards the node along this arm.
    inbound: bool,
}

/// How far along the way the arm direction is measured, so a kink at the node does not
/// turn the head.
const DIR_REACH: f64 = 6.0;
const CAR_HEAD_Y: f64 = 3.9;
const BIKE_HEAD_Y: f64 = 3.0;
const PED_HEAD_Y: f64 = 2.3;
/// Bike heads are car heads at this size.
const BIKE_SCALE: f64 = 0.62;
const MAST_Y: i32 = 7;
/// Roads this wide get a mast arm with a second head over the lanes.
const MAST_HALF: i32 = 3;

/// East-west roads are group a, north-south ones group b; the two take turns.
pub(crate) fn group_of(dir: (f64, f64)) -> char {
    if dir.0.abs() >= dir.1.abs() {
        'a'
    } else {
        'b'
    }
}

pub(crate) fn is_signal(tags: &HashMap<String, String>) -> bool {
    tags.get("highway").map(String::as_str) == Some("traffic_signals") || is_crossing(tags)
}

fn is_crossing(tags: &HashMap<String, String>) -> bool {
    tags.get("crossing").map(String::as_str) == Some("traffic_signals")
}

fn is_bike_path(highway: &str) -> bool {
    matches!(highway, "cycleway" | "path" | "bridleway")
}

/// A way a signalised crossing off the roads leads over: a bike path or a track, not the
/// crossing itself.
fn crossable(way: &ProcessedWay) -> bool {
    let tags = &way.tags;
    if ["footway", "cycleway", "path"]
        .iter()
        .any(|k| tags.get(*k).map(String::as_str) == Some("crossing"))
    {
        return false;
    }
    match (tags.get("highway"), tags.get("railway")) {
        (Some(highway), _) => is_bike_path(highway),
        (None, Some(railway)) => {
            matches!(
                railway.as_str(),
                "tram" | "light_rail" | "rail" | "narrow_gauge"
            )
        }
        _ => false,
    }
}

pub(crate) fn is_vehicular(highway: &str) -> bool {
    matches!(
        highway,
        "motorway"
            | "motorway_link"
            | "trunk"
            | "trunk_link"
            | "primary"
            | "primary_link"
            | "secondary"
            | "secondary_link"
            | "tertiary"
            | "tertiary_link"
            | "unclassified"
            | "residential"
            | "living_street"
            | "road"
            | "busway"
            | "service"
    )
}

fn arm_dir(nodes: &[ProcessedNode], i: usize, step: isize) -> Option<(f64, f64)> {
    let (x0, z0) = (nodes[i].x as f64, nodes[i].z as f64);
    let mut j = i as isize + step;
    let mut dir = None;
    while j >= 0 && (j as usize) < nodes.len() {
        let n = &nodes[j as usize];
        let (dx, dz) = (n.x as f64 - x0, n.z as f64 - z0);
        let len = dx.hypot(dz);
        if len > 0.0 {
            dir = Some((dx / len, dz / len));
            if len >= DIR_REACH {
                break;
            }
        }
        j += step;
    }
    dir
}

/// Signals this close along the road are one junction mapped twice: one on each
/// approach, or the approaches and the middle.
pub(crate) const UPSTREAM_M: f64 = 12.0;

/// Whether the road along this arm reaches another signal within `reach` blocks.
pub(crate) fn signal_upstream(nodes: &[ProcessedNode], i: usize, step: isize, reach: f64) -> bool {
    let (x0, z0) = (nodes[i].x as f64, nodes[i].z as f64);
    let mut j = i as isize + step;
    while j >= 0 && (j as usize) < nodes.len() {
        let n = &nodes[j as usize];
        if (n.x as f64 - x0).hypot(n.z as f64 - z0) > reach {
            return false;
        }
        if n.id != nodes[i].id && is_signal(&n.tags) {
            return true;
        }
        j += step;
    }
    false
}

/// The junctions of the drivable roads, and the signals at or next to each.
pub(crate) struct Junctions {
    /// How many ways lead out of each node: three or more make a junction, two are just a
    /// road that goes on, one is a dead end.
    arms: FnvHashMap<u64, u32>,
    /// Half width of the widest road through each node.
    widest: FnvHashMap<u64, i32>,
    signals: FnvHashMap<u64, Vec<u64>>,
    upstream: f64,
}

impl Junctions {
    /// From each drivable way and its half width.
    pub(crate) fn build<'a>(
        roads: impl Iterator<Item = (&'a ProcessedWay, i32)>,
        scale: f64,
    ) -> Self {
        let roads: Vec<(&ProcessedWay, i32)> = roads.collect();
        let mut junctions = Self {
            arms: FnvHashMap::default(),
            widest: FnvHashMap::default(),
            signals: FnvHashMap::default(),
            upstream: (UPSTREAM_M * scale).max(6.0),
        };
        for (way, half) in &roads {
            let closed =
                way.nodes.len() > 2 && way.nodes[0].id == way.nodes[way.nodes.len() - 1].id;
            let nodes = if closed {
                &way.nodes[1..]
            } else {
                &way.nodes[..]
            };
            for (k, n) in nodes.iter().enumerate() {
                let end = !closed && (k == 0 || k == nodes.len() - 1);
                *junctions.arms.entry(n.id).or_default() += if end { 1 } else { 2 };
                let w = junctions.widest.entry(n.id).or_default();
                *w = (*w).max(*half);
            }
        }
        let mut signals: FnvHashMap<u64, Vec<u64>> = FnvHashMap::default();
        for (way, _) in &roads {
            for (i, n) in way.nodes.iter().enumerate() {
                if !is_signal(&n.tags) {
                    continue;
                }
                if junctions.is_junction(n.id) {
                    signals.entry(n.id).or_default().push(n.id);
                    continue;
                }
                for step in [-1, 1] {
                    if let Some((j, d)) = junctions.nearest(&way.nodes, i, step) {
                        if d <= junctions.reach(j) {
                            signals.entry(j).or_default().push(n.id);
                        }
                    }
                }
            }
        }
        junctions.signals = signals;
        junctions
    }

    pub(crate) fn arms(&self, id: u64) -> u32 {
        self.arms.get(&id).copied().unwrap_or(0)
    }

    pub(crate) fn is_junction(&self, id: u64) -> bool {
        self.arms(id) >= 3
    }

    pub(crate) fn widest(&self, id: u64) -> i32 {
        self.widest.get(&id).copied().unwrap_or(1)
    }

    /// How far from a junction's middle its approach signals are looked for.
    fn reach(&self, junction: u64) -> f64 {
        self.upstream + self.widest(junction) as f64 + 1.0
    }

    /// The first junction along the nodes from `i`, and how far along the road it is.
    fn nearest(&self, nodes: &[ProcessedNode], i: usize, step: isize) -> Option<(u64, f64)> {
        let mut at = &nodes[i];
        let mut far = 0.0;
        let mut j = i as isize + step;
        while j >= 0 && (j as usize) < nodes.len() {
            let n = &nodes[j as usize];
            far += (n.x as f64 - at.x as f64).hypot(n.z as f64 - at.z as f64);
            if self.is_junction(n.id) {
                return Some((n.id, far));
            }
            at = n;
            j += step;
        }
        None
    }

    /// Whether signal `i` on a two-way road is one that traffic arriving from `step` has
    /// already left behind. A junction mapped with a signal on each approach has one on this
    /// road just past the junction, for those coming the other way: a car crossing the
    /// junction or turning in passes it. A signal with a tagged direction says for itself,
    /// and one with no other signal at the junction is a crossing of its own.
    pub(crate) fn passed(&self, nodes: &[ProcessedNode], i: usize, step: isize) -> bool {
        let node = &nodes[i];
        if self.is_junction(node.id) || node.tags.contains_key("traffic_signals:direction") {
            return false;
        }
        let Some((junction, behind)) = self.nearest(nodes, i, step) else {
            return false;
        };
        // A crosswalk just past the junction is the junction's own. Further along, people
        // cross on their own green, whichever way cars came.
        let reach = if is_crossing(&node.tags) {
            self.widest(junction) as f64 + self.upstream / 2.0
        } else {
            self.reach(junction)
        };
        if behind > reach {
            return false;
        }
        if self
            .nearest(nodes, i, -step)
            .is_some_and(|(_, ahead)| ahead <= behind)
        {
            return false;
        }
        self.signals
            .get(&junction)
            .is_some_and(|all| all.iter().any(|&s| s != node.id))
    }
}

/// A signalised node and the road arms that meet at it.
struct SignalNode {
    x: i32,
    z: i32,
    crossing: bool,
    /// What the heads for traffic on the arms are built for.
    kind: Kind,
    arms: Vec<Arm>,
}

/// The arms at every signalised node on a road, a bike path or a track.
fn signal_nodes(elements: &[ProcessedElement], scale: f64) -> FnvHashMap<u64, SignalNode> {
    let ways: Vec<&ProcessedWay> = elements
        .iter()
        .filter_map(|e| match e {
            ProcessedElement::Way(way) if way.nodes.iter().any(|n| is_signal(&n.tags)) => Some(way),
            _ => None,
        })
        .collect();
    let junctions = Junctions::build(
        elements.iter().filter_map(|e| match e {
            ProcessedElement::Way(w) if w.nodes.len() >= 2 && drivable(&w.tags) => {
                Some((w, highway_block_range(&w.tags["highway"], &w.tags, scale)))
            }
            _ => None,
        }),
        scale,
    );
    let mut nodes: FnvHashMap<u64, SignalNode> = FnvHashMap::default();
    for way in &ways {
        if let Some(highway) = way.tags.get("highway").filter(|h| is_vehicular(h)) {
            let half = highway_block_range(highway, &way.tags, scale);
            let oneway = oneway_sign(highway, &way.tags);
            add_arms(
                &mut nodes,
                way,
                half,
                oneway,
                Kind::Car,
                scale,
                Some(&junctions),
                |_| true,
            );
        }
    }
    // The rest stand on bike paths, or are crossings over a bike path or a tram track.
    let mut off_road: FnvHashMap<u64, SignalNode> = FnvHashMap::default();
    for way in &ways {
        let highway = way.tags.get("highway").map(String::as_str);
        let bike = highway.is_some_and(is_bike_path);
        let crossed = crossable(way);
        if !bike && !crossed {
            continue;
        }
        let half = match highway {
            Some(highway) => highway_block_range(highway, &way.tags, scale),
            None => 1,
        };
        let oneway = highway.map_or(0, |h| oneway_sign(h, &way.tags));
        add_arms(
            &mut off_road,
            way,
            half,
            oneway,
            Kind::Bike,
            scale,
            None,
            |n| !nodes.contains_key(&n.id) && if is_crossing(&n.tags) { crossed } else { bike },
        );
    }
    for node in off_road.values_mut().filter(|n| n.crossing) {
        // Walkers get the heads at a crossing; bikes have their own signals mapped.
        node.arms.iter_mut().for_each(|a| a.inbound = false);
    }
    nodes.extend(off_road);
    nodes
}

/// Adds the arms of this way at each of its signal nodes `wanted` accepts.
#[allow(clippy::too_many_arguments)]
fn add_arms(
    nodes: &mut FnvHashMap<u64, SignalNode>,
    way: &ProcessedWay,
    half: i32,
    oneway: i8,
    kind: Kind,
    scale: f64,
    junctions: Option<&Junctions>,
    wanted: impl Fn(&ProcessedNode) -> bool,
) {
    let upstream = (UPSTREAM_M * scale).max(6.0);
    for (i, node) in way.nodes.iter().enumerate() {
        if !is_signal(&node.tags) || !wanted(node) {
            continue;
        }
        // A head controls traffic going one way along the way, or both.
        let along = node
            .tags
            .get("traffic_signals:direction")
            .map(String::as_str);
        let forward_ok = oneway >= 0 && along != Some("backward");
        let backward_ok = oneway <= 0 && along != Some("forward");
        // Traffic going forward arrives from the node before this one. Traffic that has
        // just come through another signal or across the junction is not held again.
        let passed =
            |step: isize| oneway == 0 && junctions.is_some_and(|j| j.passed(&way.nodes, i, step));
        let candidates = [
            (
                arm_dir(&way.nodes, i, -1),
                forward_ok && !signal_upstream(&way.nodes, i, -1, upstream) && !passed(-1),
            ),
            (
                arm_dir(&way.nodes, i, 1),
                backward_ok && !signal_upstream(&way.nodes, i, 1, upstream) && !passed(1),
            ),
        ];
        let entry = nodes.entry(node.id).or_insert_with(|| SignalNode {
            x: node.x,
            z: node.z,
            crossing: is_crossing(&node.tags),
            kind,
            arms: Vec::new(),
        });
        for (dir, inbound) in candidates {
            let Some(dir) = dir else { continue };
            // A way split at the node adds the same arm twice.
            if let Some(same) = entry
                .arms
                .iter_mut()
                .find(|a| a.dir.0 * dir.0 + a.dir.1 * dir.1 > 0.9)
            {
                same.inbound |= inbound;
                same.half = same.half.max(half);
            } else {
                entry.arms.push(Arm { dir, half, inbound });
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Car,
    Bike,
    Pedestrian,
}

#[derive(Clone, Copy, Debug)]
struct Head {
    facing: (f64, f64),
    group: char,
    kind: Kind,
}

/// A pole and what hangs on it, built by the node that planned it first.
#[derive(Clone, Debug)]
struct Pole {
    x: i32,
    z: i32,
    /// Height of the post above the ground.
    top: i32,
    heads: Vec<Head>,
    /// The arm's last cell and the head hanging under it.
    mast: Option<((i32, i32), Head)>,
}

/// Every pole and head, planned once for the whole area.
pub struct SignalIndex {
    poles: Vec<Pole>,
    /// Poles each signal node builds.
    by_node: FnvHashMap<u64, Vec<usize>>,
    /// Signal nodes the plan found a kerb for, including those whose heads another node's
    /// pole carries or that a nearby head already covers.
    handled: FnvHashSet<u64>,
    /// The extras' data pack cycles the heads. Without it they are built from vanilla
    /// blocks, frozen in their first state.
    cycling: bool,
}

/// Two heads facing one way this close together are one head mapped twice, as a junction
/// mapped with a signal on each approach and one in the middle is.
const COVER: i32 = 6;

struct Plan<'r> {
    poles: Vec<Pole>,
    by_node: FnvHashMap<u64, Vec<usize>>,
    handled: FnvHashSet<u64>,
    drives_on_left: bool,
    road_mask: &'r RoadMaskBitmap,
}

impl Plan<'_> {
    fn node(&mut self, id: u64, node: &SignalNode) {
        let junction = node.arms.len() >= 3;
        for (i, arm) in node.arms.iter().enumerate() {
            if !arm.inbound {
                continue;
            }
            let side = kerb_side((-arm.dir.0, -arm.dir.1), self.drives_on_left);
            // At a junction the head stands back behind the crossing road.
            let back = if junction {
                node.arms
                    .iter()
                    .enumerate()
                    .filter(|&(j, _)| j != i)
                    .map(|(_, o)| o.half)
                    .max()
                    .unwrap_or(1)
                    + 2
            } else {
                1
            };
            let origin = (
                node.x as f64 + 0.5 + arm.dir.0 * back as f64,
                node.z as f64 + 0.5 + arm.dir.1 * back as f64,
            );
            let kerb = kerb_cell(origin, side, arm.half, self.road_mask)
                .or_else(|| {
                    // A bike path, tram tracks or the other carriageway often run along
                    // the road on that side; the head then stands on the other.
                    kerb_cell(origin, (-side.0, -side.1), arm.half, self.road_mask)
                })
                .or_else(|| island_cell(origin, self.road_mask));
            let Some((px, pz, off)) = kerb else {
                continue;
            };
            self.handled.insert(id);
            let head = Head {
                facing: arm.dir,
                group: group_of(arm.dir),
                kind: node.kind,
            };
            if self.covered(px, pz, head) {
                continue;
            }
            let pole = self.pole(id, px, pz);
            self.poles[pole].heads.push(head);
            if node.kind == Kind::Car && arm.half >= MAST_HALF && self.poles[pole].mast.is_none() {
                // Two-way traffic keeps to its half of the road; one-way takes all of it.
                let both_ways = node.arms.iter().enumerate().any(|(j, o)| {
                    j != i && o.inbound && o.dir.0 * arm.dir.0 + o.dir.1 * arm.dir.1 < -0.9
                });
                let lane = if both_ways { arm.half / 2 } else { 0 };
                let reach = (off - lane).max(1) as f64;
                let end = (
                    (px as f64 + 0.5 - side.0 * reach).floor() as i32,
                    (pz as f64 + 0.5 - side.1 * reach).floor() as i32,
                );
                self.poles[pole].mast = Some((end, head));
                self.poles[pole].top = MAST_Y;
            }
        }
        if node.crossing && !junction {
            self.pedestrian_heads(id, node);
        }
    }

    /// Pedestrian heads either side of a signalised crossing, each facing the kerb opposite.
    fn pedestrian_heads(&mut self, id: u64, node: &SignalNode) {
        let road = node.arms[0];
        let across = (-road.dir.1, road.dir.0);
        let origin = (node.x as f64 + 0.5, node.z as f64 + 0.5);
        for sign in [1.0, -1.0] {
            let side = (across.0 * sign, across.1 * sign);
            // A car head's pole on this kerb carries it when there is one.
            let existing = self.poles.iter().position(|p| {
                let (dx, dz) = (p.x as f64 + 0.5 - origin.0, p.z as f64 + 0.5 - origin.1);
                dx * side.0 + dz * side.1 > 0.0
                    && (dx * road.dir.0 + dz * road.dir.1).abs() < 3.0
                    && (dx * side.0 + dz * side.1) <= (road.half + 5) as f64
            });
            let pole = match existing {
                Some(p) => p,
                None => {
                    // The crossing path is paved as well, so the pole may have to stand
                    // beside it.
                    let kerb = [0.0, 2.0, -2.0]
                        .iter()
                        .find_map(|&along| {
                            let from =
                                (origin.0 + road.dir.0 * along, origin.1 + road.dir.1 * along);
                            kerb_cell(from, side, road.half, self.road_mask)
                        })
                        .or_else(|| {
                            let ahead = (
                                origin.0 + side.0 * f64::from(road.half),
                                origin.1 + side.1 * f64::from(road.half),
                            );
                            island_cell(ahead, self.road_mask)
                        });
                    let Some((px, pz, _)) = kerb else {
                        continue;
                    };
                    self.pole(id, px, pz)
                }
            };
            self.handled.insert(id);
            let head = Head {
                facing: (-side.0, -side.1),
                group: group_of(road.dir),
                kind: Kind::Pedestrian,
            };
            let (x, z) = (self.poles[pole].x, self.poles[pole].z);
            if !self.covered(x, z, head) {
                self.poles[pole].heads.push(head);
            }
        }
    }

    /// Whether a head of this kind already faces this way nearby.
    fn covered(&self, x: i32, z: i32, head: Head) -> bool {
        self.poles.iter().any(|p| {
            (p.x - x).abs() <= COVER
                && (p.z - z).abs() <= COVER
                && p.heads.iter().any(|h| {
                    h.kind == head.kind
                        && h.facing.0 * head.facing.0 + h.facing.1 * head.facing.1 > 0.9
                })
        })
    }

    /// The pole standing at or right next to this cell, or a new one there.
    fn pole(&mut self, owner: u64, x: i32, z: i32) -> usize {
        if let Some(i) = self
            .poles
            .iter()
            .position(|p| (p.x - x).abs() <= 1 && (p.z - z).abs() <= 1)
        {
            return i;
        }
        self.poles.push(Pole {
            x,
            z,
            top: 4,
            heads: Vec::new(),
            mast: None,
        });
        self.by_node
            .entry(owner)
            .or_default()
            .push(self.poles.len() - 1);
        self.poles.len() - 1
    }
}

impl SignalIndex {
    pub fn build(
        elements: &[ProcessedElement],
        scale: f64,
        drives_on_left: bool,
        road_mask: &RoadMaskBitmap,
        cycling: bool,
    ) -> Self {
        let nodes = signal_nodes(elements, scale);
        let mut ids: Vec<u64> = nodes.keys().copied().collect();
        // Planning order decides which of two overlapping heads stays, so keep it fixed.
        ids.sort_unstable();
        let mut plan = Plan {
            poles: Vec::new(),
            by_node: FnvHashMap::default(),
            handled: FnvHashSet::default(),
            drives_on_left,
            road_mask,
        };
        for id in &ids {
            plan.node(*id, &nodes[id]);
        }
        Self {
            poles: plan.poles,
            by_node: plan.by_node,
            handled: plan.handled,
            cycling,
        }
    }

    /// Whether the data pack cycles these heads.
    pub fn cycling(&self) -> bool {
        self.cycling
    }

    /// Whether a car head near `at`, within reach of a junction's widest heads, faces
    /// `facing`: towards traffic that has to stop there.
    pub(crate) fn shows(&self, at: (f64, f64), facing: (f64, f64), scale: f64) -> bool {
        let reach = (HEAD_REACH_M * scale).max(12.0);
        self.poles.iter().any(|pole| {
            let d = (f64::from(pole.x) + 0.5 - at.0).hypot(f64::from(pole.z) + 0.5 - at.1);
            d <= reach
                && pole
                    .heads
                    .iter()
                    .chain(pole.mast.as_ref().map(|(_, h)| h))
                    .any(|h| {
                        h.kind == Kind::Car && h.facing.0 * facing.0 + h.facing.1 * facing.1 > 0.7
                    })
        })
    }

    /// Builds the poles `node` owns. False when the plan found no kerb for it at all, so
    /// that it has no head here or on another node's pole.
    pub fn place(
        &self,
        editor: &mut WorldEditor,
        node: &ProcessedNode,
        base_y: &dyn Fn(&WorldEditor, i32, i32, i32) -> i32,
    ) -> bool {
        let poles = self.by_node.get(&node.id).map_or(&[][..], Vec::as_slice);
        for &i in poles {
            let pole = &self.poles[i];
            let base = base_y(editor, pole.x, pole.z, 1);
            let mut builder = Builder {
                editor,
                seed_base: i as i64 * 1_000,
                parts: 0,
                cycling: self.cycling,
            };
            builder.post(pole.x, pole.z, base, pole.top);
            let pole_centre = (f64::from(pole.x) + 0.5, f64::from(pole.z) + 0.5);
            for head in &pole.heads {
                let y = match head.kind {
                    Kind::Car => CAR_HEAD_Y,
                    Kind::Bike => BIKE_HEAD_Y,
                    Kind::Pedestrian => PED_HEAD_Y,
                };
                // On the post's face, the brackets against it.
                let back = (
                    pole_centre.0 + head.facing.0 * POST_HALF,
                    pole_centre.1 + head.facing.1 * POST_HALF,
                );
                builder.head(back, f64::from(base) + y, *head, true);
            }
            if let Some((end, head)) = pole.mast {
                let arm_y = f64::from(base + MAST_Y) + 0.5;
                let end_centre = (f64::from(end.0) + 0.5, f64::from(end.1) + 0.5);
                builder.mast_arm(pole_centre, end_centre, arm_y);
                // Hung under the arm's end, its housing centred on the arm.
                let depth = HEAD_DEPTH_M * head.kind.size();
                let back = (
                    end_centre.0 - head.facing.0 * depth / 2.0,
                    end_centre.1 - head.facing.1 * depth / 2.0,
                );
                let centre_y = arm_y - ARM_SIDE / 2.0 - head.kind.half_height();
                builder.head(back, centre_y, head, false);
            }
        }
        self.handled.contains(&node.id)
    }
}

fn kerb_side(travel: (f64, f64), drives_on_left: bool) -> (f64, f64) {
    if drives_on_left {
        (travel.1, -travel.0)
    } else {
        (-travel.1, travel.0)
    }
}

/// How far past the road's edge a pole may stand: across a bike path or a sidewalk.
const KERB_REACH: i32 = 6;

/// The first cell off the road surface beside `origin`, and how far out it is.
fn kerb_cell(
    origin: (f64, f64),
    side: (f64, f64),
    half: i32,
    road_mask: &RoadMaskBitmap,
) -> Option<(i32, i32, i32)> {
    (half + 1..=half + KERB_REACH).find_map(|off| {
        let x = (origin.0 + side.0 * off as f64).floor() as i32;
        let z = (origin.1 + side.1 * off as f64).floor() as i32;
        (!road_mask.contains(x, z)).then_some((x, z, off))
    })
}

/// In a junction paved wall to wall a head stands on the nearest traffic island, this
/// far out at most.
const ISLAND_REACH: i32 = 8;

/// The nearest cell off the road within ISLAND_REACH of `origin`, as a kerb one block out,
/// so that a mast from it stays short.
fn island_cell(origin: (f64, f64), road_mask: &RoadMaskBitmap) -> Option<(i32, i32, i32)> {
    let (ox, oz) = (origin.0.floor() as i32, origin.1.floor() as i32);
    (1..=ISLAND_REACH).find_map(|r| {
        (-r..=r)
            .flat_map(|dx| (-r..=r).map(move |dz| (dx, dz)))
            .filter(|&(dx, dz)| dx.abs().max(dz.abs()) == r)
            .filter(|&(dx, dz)| !road_mask.contains(ox + dx, oz + dz))
            .min_by_key(|&(dx, dz)| (dx * dx + dz * dz, dx, dz))
            .map(|(dx, dz)| (ox + dx, oz + dz, 1))
    })
}

/// How far from its signal node a car head can stand: back behind the widest crossing
/// road, and out to the kerb.
const HEAD_REACH_M: f64 = 20.0;

/// Half the width of the iron bar post, where a head's brackets meet it.
const POST_HALF: f64 = 0.0625;
/// The heads' models are drawn at 32 units to the metre (see `assets/vehicles/models/
/// signals.py`), so at half the size of a vehicle's: a car head is 0.34 m across.
const HEAD_SIZE: f64 = 0.5;
/// From the back of a head's housing, where its origin is, to the back of its brackets,
/// and to the front of its housing, at 1:1.
const BRACKET_M: f64 = 1.5 / 32.0;
const HEAD_DEPTH_M: f64 = 8.0 / 32.0;
/// The mast arm: a square bar, the colour of the iron bar post.
const ARM_SIDE: f64 = 0.12;
const ARM_BLOCK: &str = "minecraft:polished_andesite";

impl Kind {
    /// How big this kind of head is drawn, against its model.
    fn size(self) -> f64 {
        match self {
            Kind::Bike => BIKE_SCALE,
            Kind::Car | Kind::Pedestrian => 1.0,
        }
    }

    /// From a head's middle down to the foot of its housing, the model's origin.
    fn half_height(self) -> f64 {
        let units = match self {
            Kind::Car | Kind::Bike => 17.5,
            Kind::Pedestrian => 12.5,
        };
        units / 32.0 * self.size()
    }

    /// The model it shows at the start, and the tag the data pack switches it by: walkers
    /// cross road a while group b has its green.
    fn start(self, group: char) -> (&'static str, String) {
        match (self, group) {
            (Kind::Pedestrian, 'a') => ("signal_walk_red", format!("arnis.tl_p{group}")),
            (Kind::Pedestrian, _) => ("signal_walk_green", format!("arnis.tl_p{group}")),
            (_, 'a') => ("signal_car_green", format!("arnis.tl_{group}")),
            _ => ("signal_car_red", format!("arnis.tl_{group}")),
        }
    }
}

/// The look a signal head shows in each state: the data pack names the same ids.
pub fn signal_looks() -> Vec<crate::vehicle_models::Look> {
    [
        "signal_car_red",
        "signal_car_amber",
        "signal_car_green",
        "signal_car_red_amber",
        "signal_walk_red",
        "signal_walk_green",
    ]
    .into_iter()
    .map(signal_look)
    .collect()
}

fn signal_look(model: &'static str) -> crate::vehicle_models::Look {
    crate::vehicle_models::Look::plain(model, "std", "minecraft:black_concrete")
}

struct Builder<'e, 'a> {
    editor: &'e mut WorldEditor<'a>,
    seed_base: i64,
    parts: i64,
    cycling: bool,
}

/// A head built from blocks: its housing, as deep as the model's is.
const HOUSING: &str = "minecraft:black_concrete";
const HOUSING_DEPTH: f64 = 0.3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lamp {
    Red,
    Amber,
    Green,
}

impl Lamp {
    fn block(self) -> &'static str {
        match self {
            Lamp::Red => "minecraft:red_concrete",
            Lamp::Amber => "minecraft:yellow_concrete",
            Lamp::Green => "minecraft:lime_concrete",
        }
    }
}

impl Builder<'_, '_> {
    fn post(&mut self, x: i32, z: i32, base: i32, top: i32) {
        self.editor
            .set_block_absolute(COBBLESTONE_WALL, x, base + 1, z, None, None);
        for y in base + 2..=base + top {
            self.editor
                .set_block_absolute(IRON_BARS, x, y, z, None, None);
        }
    }

    /// One head, its back at `back` (the face of the post its brackets meet, if
    /// `bracketed`), its middle `centre_y` up, facing its traffic.
    fn head(&mut self, back: (f64, f64), centre_y: f64, head: Head, bracketed: bool) {
        if self.cycling {
            self.model_head(back, centre_y, head, bracketed);
        } else {
            self.block_head(back, centre_y, head, bracketed);
        }
    }

    /// A head of scaled vanilla blocks, for a world without the data pack and the
    /// resource pack its model comes in. It shows the state the data pack would start it
    /// in: roads of group a on green, the others on red, walkers the other way round.
    fn block_head(&mut self, back: (f64, f64), centre_y: f64, head: Head, bracketed: bool) {
        let k = head.kind.size();
        let out = if bracketed { BRACKET_M * k } else { 0.0 };
        let forward = out + HOUSING_DEPTH * k / 2.0;
        let centre = (
            back.0 + head.facing.0 * forward,
            centre_y,
            back.1 + head.facing.1 * forward,
        );
        let walk = head.kind == Kind::Pedestrian;
        let (housing, lamps): ((f64, f64), &[(Lamp, f64)]) = if walk {
            ((0.34, 0.72), &[(Lamp::Red, 0.17), (Lamp::Green, -0.17)])
        } else {
            (
                (0.42, 1.1),
                &[(Lamp::Red, 0.34), (Lamp::Amber, 0.0), (Lamp::Green, -0.34)],
            )
        };
        let lit = match (walk, head.group) {
            (false, 'a') | (true, 'b') => Lamp::Green,
            _ => Lamp::Red,
        };
        let depth = HOUSING_DEPTH * k;
        self.part(
            centre,
            head.facing,
            0.0,
            0.0,
            ((housing.0 * k) as f32, (housing.1 * k) as f32, depth as f32),
            HOUSING,
            Vec::new(),
            None,
        );
        let lens = if walk { 0.24 } else { 0.26 } * k;
        for &(lamp, y) in lamps {
            // A lit lens shows at full brightness whatever the light around it.
            // An unlit one is darker, but its colour still shows by day, so a head reads as
            // a traffic light with all its lamps; by night it goes dark.
            let brightness = if lamp == lit { (15, 15) } else { (11, 4) };
            self.part(
                centre,
                head.facing,
                y * k,
                depth / 2.0 + 0.02 * k,
                (lens as f32, lens as f32, (0.04 * k) as f32),
                lamp.block(),
                Vec::new(),
                Some(brightness),
            );
        }
    }

    /// A display of the head's model, which the data pack switches between states.
    fn model_head(&mut self, back: (f64, f64), centre_y: f64, head: Head, bracketed: bool) {
        use crate::vehicle_models::{display_nbt, item_model};
        let size = head.kind.size();
        let (model, tag) = head.kind.start(head.group);
        let out = if bracketed { BRACKET_M * size } else { 0.0 };
        let origin = (back.0 + head.facing.0 * out, back.1 + head.facing.1 * out);
        let mut nbt = display_nbt(
            model,
            &item_model(&signal_look(model)),
            HEAD_SIZE * size,
            false,
        );
        nbt.insert(
            "Rotation".to_string(),
            Value::List(vec![Value::Float(yaw_of(head.facing)), Value::Float(0.0)]),
        );
        nbt.insert(
            "Tags".to_string(),
            Value::List(vec![
                Value::String("arnis.tl".to_string()),
                Value::String(tag),
            ]),
        );
        let seed = self.seed_base + self.parts;
        self.parts += 1;
        self.editor.add_entity_exact(
            "minecraft:item_display",
            origin.0,
            centre_y - head.kind.half_height(),
            origin.1,
            seed,
            nbt,
        );
    }

    /// A straight bar from the post out to the head over the lanes, at any angle.
    fn mast_arm(&mut self, from: (f64, f64), to: (f64, f64), y: f64) {
        let (dx, dz) = (to.0 - from.0, to.1 - from.1);
        let len = dx.hypot(dz);
        if len < 0.5 {
            return;
        }
        let dir = (dx / len, dz / len);
        // On past the head's middle by half the bar, so the head hangs under its end.
        let reach = len + ARM_SIDE / 2.0;
        let side = ARM_SIDE as f32;
        self.part(
            (from.0, y, from.1),
            dir,
            0.0,
            reach / 2.0,
            (side, side, reach as f32),
            ARM_BLOCK,
            Vec::new(),
            None,
        );
    }

    /// One scaled block, centred `y` above and `forward` in front of `centre`.
    #[allow(clippy::too_many_arguments)]
    fn part(
        &mut self,
        centre: (f64, f64, f64),
        facing: (f64, f64),
        y: f64,
        forward: f64,
        size: (f32, f32, f32),
        block: &str,
        tags: Vec<String>,
        brightness: Option<(i32, i32)>,
    ) {
        let floats = |v: &[f32]| Value::List(v.iter().map(|f| Value::Float(*f)).collect());
        let mut transformation = HashMap::new();
        transformation.insert("left_rotation".to_string(), floats(&[0.0, 0.0, 0.0, 1.0]));
        transformation.insert("right_rotation".to_string(), floats(&[0.0, 0.0, 0.0, 1.0]));
        // The block model spans 0..1, so the translation centres it on the entity.
        transformation.insert(
            "translation".to_string(),
            floats(&[-size.0 / 2.0, -size.1 / 2.0, -size.2 / 2.0]),
        );
        transformation.insert("scale".to_string(), floats(&[size.0, size.1, size.2]));

        let mut extra = HashMap::new();
        extra.insert(
            "block_state".to_string(),
            Value::Compound(HashMap::from([(
                "Name".to_string(),
                Value::String(block.to_string()),
            )])),
        );
        extra.insert(
            "transformation".to_string(),
            Value::Compound(transformation),
        );
        extra.insert(
            "Rotation".to_string(),
            Value::List(vec![Value::Float(yaw_of(facing)), Value::Float(0.0)]),
        );
        if !tags.is_empty() {
            extra.insert(
                "Tags".to_string(),
                Value::List(tags.into_iter().map(Value::String).collect()),
            );
        }
        if let Some((sky, block)) = brightness {
            extra.insert(
                "brightness".to_string(),
                Value::Compound(HashMap::from([
                    ("sky".to_string(), Value::Int(sky)),
                    ("block".to_string(), Value::Int(block)),
                ])),
            );
        }

        let seed = self.seed_base + self.parts;
        self.parts += 1;
        self.editor.add_block_display(
            centre.0 + facing.0 * forward,
            centre.1 + y,
            centre.2 + facing.1 * forward,
            seed,
            extra,
        );
    }
}

/// The yaw that turns an entity's +z, and a vehicle model's front, towards `facing`.
fn yaw_of(facing: (f64, f64)) -> f32 {
    (-facing.0).atan2(facing.1).to_degrees() as f32
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
        ProcessedElement::Way(crate::osm_parser::ProcessedWay {
            id,
            nodes,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    fn road_mask(cells: impl Iterator<Item = (i32, i32)>) -> RoadMaskBitmap {
        let bbox = crate::coordinate_system::cartesian::XZBBox::rect_from_min_max(-64, -64, 64, 64)
            .unwrap();
        let mut mask = RoadMaskBitmap::new(&bbox);
        for (x, z) in cells {
            mask.set(x, z);
        }
        mask
    }

    /// Two residential streets crossing at the origin, paved two blocks either side.
    fn crossroads(signal_nodes: &[(u64, i32, i32)]) -> (Vec<ProcessedElement>, RoadMaskBitmap) {
        let signal = [("highway", "traffic_signals")];
        let approach = |west: bool| {
            signal_nodes
                .iter()
                .filter(move |&&(_, x, _)| (x < 0) == west)
                .map(|&(id, x, z)| node(id, x, z, &signal))
        };
        let mut ew = vec![node(1, -40, 0, &[])];
        ew.extend(approach(true));
        ew.push(node(9, 0, 0, &signal));
        ew.extend(approach(false));
        ew.push(node(2, 40, 0, &[]));
        let ns = vec![
            node(3, 0, -40, &[]),
            node(9, 0, 0, &signal),
            node(4, 0, 40, &[]),
        ];
        let elements = vec![
            way(1, ew, &[("highway", "residential")]),
            way(2, ns, &[("highway", "residential")]),
        ];
        let paved = (-40..=40).flat_map(|t| (-2..=2).flat_map(move |w| [(t, w), (w, t)]));
        (elements, road_mask(paved))
    }

    #[test]
    fn crossing_roads_meet_as_four_inbound_arms_in_two_groups() {
        let (elements, _) = crossroads(&[]);
        let nodes = signal_nodes(&elements, 1.0);
        let arms = &nodes[&9].arms;
        assert_eq!(arms.len(), 4);
        assert!(arms.iter().all(|a| a.inbound));
        assert_eq!(arms.iter().filter(|a| group_of(a.dir) == 'a').count(), 2);
    }

    #[test]
    fn a_junction_gets_one_head_per_approach_facing_its_traffic_off_the_road() {
        let (elements, mask) = crossroads(&[]);
        let index = SignalIndex::build(&elements, 1.0, false, &mask, true);
        let heads: Vec<(&Pole, &Head)> = index
            .poles
            .iter()
            .flat_map(|p| p.heads.iter().map(move |h| (p, h)))
            .collect();
        assert_eq!(heads.len(), 4);
        for (pole, head) in heads {
            assert!(!mask.contains(pole.x, pole.z), "pole on the road");
            // The head faces back along its own arm, away from the junction.
            let out = (pole.x as f64 * head.facing.0 + pole.z as f64 * head.facing.1) > 0.0;
            assert!(out, "head at {},{} faces into the junction", pole.x, pole.z);
        }
    }

    #[test]
    fn approach_signals_mapped_beside_a_signalised_junction_add_no_second_head() {
        let (alone, mask) = crossroads(&[]);
        let (mapped_twice, _) = crossroads(&[(20, -5, 0), (21, 5, 0)]);
        let count = |els: &[ProcessedElement]| {
            SignalIndex::build(els, 1.0, false, &mask, true)
                .poles
                .iter()
                .map(|p| p.heads.len())
                .sum::<usize>()
        };
        assert_eq!(count(&mapped_twice), count(&alone));
    }

    #[test]
    fn signals_on_each_approach_get_heads_only_for_traffic_heading_into_the_junction() {
        let signal = [("highway", "traffic_signals")];
        let road = [("highway", "residential")];
        let elements = vec![
            way(
                1,
                vec![
                    node(1, -40, 0, &[]),
                    node(11, -8, 0, &signal),
                    node(9, 0, 0, &[]),
                    node(12, 8, 0, &signal),
                    node(2, 40, 0, &[]),
                ],
                &road,
            ),
            way(
                2,
                vec![
                    node(3, 0, -40, &[]),
                    node(13, 0, -8, &signal),
                    node(9, 0, 0, &[]),
                    node(14, 0, 8, &signal),
                    node(4, 0, 40, &[]),
                ],
                &road,
            ),
        ];
        let paved = (-40..=40).flat_map(|t| (-2..=2).flat_map(move |w| [(t, w), (w, t)]));
        let index = SignalIndex::build(&elements, 1.0, false, &road_mask(paved), true);
        let heads: Vec<(&Pole, &Head)> = index
            .poles
            .iter()
            .flat_map(|p| p.heads.iter().map(move |h| (p, h)))
            .collect();
        assert_eq!(heads.len(), 4);
        for (pole, head) in heads {
            let out = pole.x as f64 * head.facing.0 + pole.z as f64 * head.facing.1 > 0.0;
            assert!(
                out,
                "head at {},{} faces out of the junction",
                pole.x, pole.z
            );
        }
    }

    #[test]
    fn one_way_roads_only_hold_traffic_arriving_along_them() {
        let ow = way(
            1,
            vec![
                node(1, -20, 0, &[]),
                node(9, 0, 0, &[("highway", "traffic_signals")]),
                node(2, 20, 0, &[]),
            ],
            &[("highway", "primary"), ("oneway", "yes")],
        );
        let nodes = signal_nodes(&[ow], 1.0);
        let inbound: Vec<_> = nodes[&9].arms.iter().filter(|a| a.inbound).collect();
        assert_eq!(inbound.len(), 1);
        // Traffic heads east, so it arrives along the arm pointing west.
        assert!(inbound[0].dir.0 < -0.9);
    }

    #[test]
    fn a_signalised_crossing_gets_a_pedestrian_head_on_each_kerb() {
        let crossing = way(
            1,
            vec![
                node(1, -30, 0, &[]),
                node(
                    9,
                    0,
                    0,
                    &[("highway", "crossing"), ("crossing", "traffic_signals")],
                ),
                node(2, 30, 0, &[]),
            ],
            &[("highway", "residential")],
        );
        let mask = road_mask((-30..=30).flat_map(|x| (-2..=2).map(move |z| (x, z))));
        let index = SignalIndex::build(&[crossing], 1.0, false, &mask, true);
        let peds: Vec<&Head> = index
            .poles
            .iter()
            .flat_map(|p| &p.heads)
            .filter(|h| h.kind == Kind::Pedestrian)
            .collect();
        assert_eq!(peds.len(), 2);
        // They face across the road, so north and south.
        assert!(peds.iter().all(|h| h.facing.1.abs() > 0.9));
    }

    fn heads_of(index: &SignalIndex, kind: Kind) -> Vec<(&Pole, &Head)> {
        index
            .poles
            .iter()
            .flat_map(|p| p.heads.iter().map(move |h| (p, h)))
            .filter(|(_, h)| h.kind == kind)
            .collect()
    }

    #[test]
    fn a_bike_signal_gets_a_bike_head_beside_the_path() {
        let path = way(
            1,
            vec![
                node(1, -30, 0, &[]),
                node(9, 0, 0, &[("highway", "traffic_signals")]),
                node(2, 30, 0, &[]),
            ],
            &[("highway", "cycleway"), ("oneway", "yes")],
        );
        let mask = road_mask((-30..=30).flat_map(|x| (-1..=1).map(move |z| (x, z))));
        let index = SignalIndex::build(&[path], 1.0, false, &mask, true);
        let bikes = heads_of(&index, Kind::Bike);
        assert_eq!(bikes.len(), 1);
        assert!(heads_of(&index, Kind::Car).is_empty());
        let (pole, head) = bikes[0];
        assert!(!mask.contains(pole.x, pole.z), "pole on the path");
        // Bikes ride east, so the head faces west towards them.
        assert!(head.facing.0 < -0.9);
    }

    #[test]
    fn a_signalised_crossing_over_a_bike_path_gets_walker_heads_beside_the_footway() {
        let crossing = [("highway", "crossing"), ("crossing", "traffic_signals")];
        let elements = vec![
            way(
                1,
                vec![
                    node(1, -30, 0, &[]),
                    node(9, 0, 0, &crossing),
                    node(2, 30, 0, &[]),
                ],
                &[("highway", "cycleway")],
            ),
            way(
                2,
                vec![
                    node(3, 0, -10, &[]),
                    node(9, 0, 0, &crossing),
                    node(4, 0, 10, &[]),
                ],
                &[("highway", "footway"), ("footway", "crossing")],
            ),
        ];
        let paved = (-30..=30).flat_map(|t| (-1..=1).flat_map(move |w| [(t, w), (w, t / 3)]));
        let mask = road_mask(paved);
        let index = SignalIndex::build(&elements, 1.0, false, &mask, true);
        let walkers = heads_of(&index, Kind::Pedestrian);
        assert_eq!(walkers.len(), 2);
        for (pole, head) in walkers {
            assert!(!mask.contains(pole.x, pole.z), "pole on the paving");
            assert!(head.facing.1.abs() > 0.9);
        }
        assert!(heads_of(&index, Kind::Bike).is_empty());
    }

    #[test]
    fn a_tram_crossing_gets_walker_heads() {
        let crossing = [
            ("railway", "tram_crossing"),
            ("crossing", "traffic_signals"),
        ];
        let elements = vec![
            way(
                1,
                vec![
                    node(1, -30, 0, &[]),
                    node(9, 0, 0, &crossing),
                    node(2, 30, 0, &[]),
                ],
                &[("railway", "tram")],
            ),
            way(
                2,
                vec![
                    node(3, 0, -10, &[]),
                    node(9, 0, 0, &crossing),
                    node(4, 0, 10, &[]),
                ],
                &[("highway", "footway"), ("footway", "crossing")],
            ),
        ];
        let index = SignalIndex::build(&elements, 1.0, false, &road_mask(std::iter::empty()), true);
        assert_eq!(heads_of(&index, Kind::Pedestrian).len(), 2);
    }

    #[test]
    fn kerb_side_follows_the_driving_side() {
        // Driving north, the right-hand kerb is to the east.
        assert_eq!(kerb_side((0.0, -1.0), false), (1.0, 0.0));
        assert_eq!(kerb_side((0.0, -1.0), true), (-1.0, 0.0));
    }

    /// Without the data pack a head is built of vanilla blocks and stands still: one lamp
    /// of each head is lit, green for the group whose turn it is and red for the other.
    #[test]
    fn without_the_pack_heads_are_still_blocks_with_one_lamp_lit() {
        use crate::coordinate_system::cartesian::XZBBox;
        use crate::coordinate_system::geographic::LLBBox;

        let (elements, mask) = crossroads(&[]);
        let index = SignalIndex::build(&elements, 1.0, false, &mask, false);
        assert!(!index.cycling());
        let bbox = XZBBox::rect_from_min_max(-64, -64, 64, 64).unwrap();
        let llbbox = LLBBox::new(47.0, 8.0, 47.001, 8.001).unwrap();
        let mut editor = WorldEditor::new(std::env::temp_dir(), &bbox, llbbox);
        let junction = node(9, 0, 0, &[("highway", "traffic_signals")]);
        assert!(index.place(&mut editor, &junction, &|e, x, z, y| e
            .get_absolute_y(x, y, z)));
        // No models: nothing here needs the resource pack.
        assert!(editor.item_displays().is_empty());
        let parts = editor.entities_of("minecraft:block_display");
        let block = |p: &HashMap<String, Value>| match p.get("block_state") {
            Some(Value::Compound(state)) => match state.get("Name") {
                Some(Value::String(name)) => name.clone(),
                _ => String::new(),
            },
            _ => String::new(),
        };
        let lit = |p: &HashMap<String, Value>| {
            matches!(p.get("brightness"), Some(Value::Compound(b))
                if b.get("block") == Some(&Value::Int(15)))
        };
        let lit_lamps: Vec<String> = parts.iter().filter(|p| lit(p)).map(block).collect();
        // Four heads, one lamp lit on each, two of them green.
        assert_eq!(lit_lamps.len(), 4, "{lit_lamps:?}");
        let green = lit_lamps.iter().filter(|b| *b == "minecraft:lime_concrete");
        assert_eq!(green.count(), 2);
        assert!(parts.iter().any(|p| block(p) == "minecraft:black_concrete"));
    }

    /// One carriageway of a dual road often has a bike path, tram tracks or the other
    /// carriageway along its kerb; its head then stands on the median side.
    #[test]
    fn a_one_way_carriageway_blocked_on_its_kerb_side_gets_a_head_on_the_other() {
        for (from, to) in [(-40, 40), (40, -40)] {
            let signal = [("highway", "traffic_signals")];
            let elements = vec![way(
                1,
                vec![
                    node(1, from, 0, &[]),
                    node(9, 0, 0, &signal),
                    node(2, to, 0, &[]),
                ],
                &[("highway", "secondary"), ("oneway", "yes")],
            )];
            // Paved two blocks either side, and ten more on the south.
            let paved = (-40..=40).flat_map(|x| (-2..=12).map(move |z| (x, z)));
            let index = SignalIndex::build(&elements, 1.0, false, &road_mask(paved), false);
            let heads: Vec<&Pole> = index.poles.iter().filter(|p| !p.heads.is_empty()).collect();
            assert_eq!(heads.len(), 1, "travelling {from} to {to}");
            assert!(
                heads[0].z < 0,
                "head on the open north side, at z {}",
                heads[0].z
            );
        }
    }

    /// In a junction paved far out on both sides the head stands on the nearest island.
    #[test]
    fn a_head_in_a_wide_paved_junction_stands_on_an_island() {
        let signal = [("highway", "traffic_signals")];
        let elements = vec![way(
            1,
            vec![
                node(1, -40, 0, &[]),
                node(9, 0, 0, &signal),
                node(2, 40, 0, &[]),
            ],
            &[("highway", "secondary"), ("oneway", "yes")],
        )];
        // Paved twenty blocks out on both sides, but for a small island.
        let paved = (-40..=40)
            .flat_map(|x| (-20..=20).map(move |z| (x, z)))
            .filter(|&(x, z)| !((-6..=-4).contains(&x) && (5..=6).contains(&z)));
        let index = SignalIndex::build(&elements, 1.0, false, &road_mask(paved), false);
        let poles: Vec<&Pole> = index.poles.iter().filter(|p| !p.heads.is_empty()).collect();
        assert_eq!(poles.len(), 1);
        assert!((-6..=-4).contains(&poles[0].x) && (5..=6).contains(&poles[0].z));
    }

    /// A signal whose head another signal's pole carries is handled: without the data pack
    /// it must not fall back to a banner signal of its own beside it.
    #[test]
    fn a_signal_covered_by_another_counts_as_placed() {
        use crate::coordinate_system::cartesian::XZBBox;
        use crate::coordinate_system::geographic::LLBBox;

        let (elements, mask) = crossroads(&[(21, -6, 0), (22, 6, 0)]);
        let index = SignalIndex::build(&elements, 1.0, false, &mask, false);
        let bbox = XZBBox::rect_from_min_max(-64, -64, 64, 64).unwrap();
        let llbbox = LLBBox::new(47.0, 8.0, 47.001, 8.001).unwrap();
        let mut editor = WorldEditor::new(std::env::temp_dir(), &bbox, llbbox);
        let base = |e: &WorldEditor, x: i32, z: i32, y: i32| e.get_absolute_y(x, y, z);
        let signal = [("highway", "traffic_signals")];
        for (id, x) in [(21, -6), (22, 6), (9, 0)] {
            let placed = index.place(&mut editor, &node(id, x, 0, &signal), &base);
            assert!(placed, "signal {id} would get a banner signal too");
        }
        // A signal far from any road is not.
        assert!(!index.place(&mut editor, &node(99, 60, 60, &signal), &base));
    }

    /// The data pack switches a head between models by id: each must be one the resource
    /// pack draws, and so must the one every head starts with.
    #[test]
    fn heads_switch_between_models_the_pack_draws() {
        use crate::vehicle_models::item_model;
        let drawn: Vec<String> = signal_looks().iter().map(item_model).collect();
        let switches = [
            include_str!("../../assets/minecraft/datapack_extras/data/arnis/function/traffic/car/red.mcfunction"),
            include_str!("../../assets/minecraft/datapack_extras/data/arnis/function/traffic/car/amber.mcfunction"),
            include_str!("../../assets/minecraft/datapack_extras/data/arnis/function/traffic/car/green.mcfunction"),
            include_str!("../../assets/minecraft/datapack_extras/data/arnis/function/traffic/car/red_amber.mcfunction"),
            include_str!("../../assets/minecraft/datapack_extras/data/arnis/function/traffic/walk/red.mcfunction"),
            include_str!("../../assets/minecraft/datapack_extras/data/arnis/function/traffic/walk/green.mcfunction"),
        ];
        for text in switches {
            let id = text.trim().rsplit(' ').next().unwrap().trim_matches('"');
            assert!(drawn.iter().any(|d| d == id), "{id}");
        }
        for kind in [Kind::Car, Kind::Bike, Kind::Pedestrian] {
            for group in ['a', 'b'] {
                let (model, tag) = kind.start(group);
                assert!(drawn.contains(&item_model(&signal_look(model))), "{model}");
                assert!(tag.starts_with("arnis.tl_"), "{tag}");
                // Every model exists, and stands on its foot with its lenses forward.
                let meta = crate::vehicle_models::model(model).meta();
                let (foot, front) = (meta.bounds[0][1], meta.bounds[0][2]);
                assert!((-2.0..=0.0).contains(&foot) && front < -8.0, "{model}");
            }
        }
    }
}
