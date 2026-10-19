//! Signal heads built from block displays, one per approach, so the extras data pack can
//! cycle their lamps. The tags written here are the contract with
//! `assets/minecraft/datapack_extras/data/arnis/function/traffic`.

use crate::block_definitions::{COBBLESTONE_WALL, IRON_BARS};
use crate::bresenham::bresenham_line;
use crate::element_processing::connected_blocks::{connected_iron_bars, four_connected_line};
use crate::element_processing::highways::highway_block_range;
use crate::element_processing::road_markings::oneway_sign;
use crate::floodfill_cache::RoadMaskBitmap;
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use fnv::FnvHashMap;
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

const HOUSING: &str = "minecraft:black_concrete";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lamp {
    Red,
    Amber,
    Green,
}

impl Lamp {
    fn block(self) -> &'static str {
        match self {
            Lamp::Red => "minecraft:redstone_block",
            Lamp::Amber => "minecraft:yellow_concrete",
            Lamp::Green => "minecraft:emerald_block",
        }
    }

    fn tag(self) -> char {
        match self {
            Lamp::Red => 'r',
            Lamp::Amber => 'y',
            Lamp::Green => 'g',
        }
    }
}

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
    let mut nodes: FnvHashMap<u64, SignalNode> = FnvHashMap::default();
    for way in &ways {
        if let Some(highway) = way.tags.get("highway").filter(|h| is_vehicular(h)) {
            let half = highway_block_range(highway, &way.tags, scale);
            let oneway = oneway_sign(highway, &way.tags);
            add_arms(&mut nodes, way, half, oneway, Kind::Car, scale, |_| true);
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
        add_arms(&mut off_road, way, half, oneway, Kind::Bike, scale, |n| {
            !nodes.contains_key(&n.id) && if is_crossing(&n.tags) { crossed } else { bike }
        });
    }
    for node in off_road.values_mut().filter(|n| n.crossing) {
        // Walkers get the heads at a crossing; bikes have their own signals mapped.
        node.arms.iter_mut().for_each(|a| a.inbound = false);
    }
    nodes.extend(off_road);
    nodes
}

/// Adds the arms of this way at each of its signal nodes `wanted` accepts.
fn add_arms(
    nodes: &mut FnvHashMap<u64, SignalNode>,
    way: &ProcessedWay,
    half: i32,
    oneway: i8,
    kind: Kind,
    scale: f64,
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
        // just come through another signal is not held again.
        let candidates = [
            (
                arm_dir(&way.nodes, i, -1),
                forward_ok && !signal_upstream(&way.nodes, i, -1, upstream),
            ),
            (
                arm_dir(&way.nodes, i, 1),
                backward_ok && !signal_upstream(&way.nodes, i, 1, upstream),
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
}

/// Two heads facing one way this close together are one head mapped twice, as a junction
/// mapped with a signal on each approach and one in the middle is.
const COVER: i32 = 6;

struct Plan<'r> {
    poles: Vec<Pole>,
    by_node: FnvHashMap<u64, Vec<usize>>,
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
            let kerb = kerb_cell(origin, side, arm.half, self.road_mask).or_else(|| {
                // A bike path often runs along the road on that side; the head then stands
                // on the other.
                let other = (-side.0, -side.1);
                (node.kind == Kind::Bike)
                    .then(|| kerb_cell(origin, other, arm.half, self.road_mask))
                    .flatten()
            });
            let Some((px, pz, off)) = kerb else {
                continue;
            };
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
                    let kerb = [0.0, 2.0, -2.0].iter().find_map(|&along| {
                        let from = (origin.0 + road.dir.0 * along, origin.1 + road.dir.1 * along);
                        kerb_cell(from, side, road.half, self.road_mask)
                    });
                    let Some((px, pz, _)) = kerb else {
                        continue;
                    };
                    self.pole(id, px, pz)
                }
            };
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
    ) -> Self {
        let nodes = signal_nodes(elements, scale);
        let mut ids: Vec<u64> = nodes.keys().copied().collect();
        // Planning order decides which of two overlapping heads stays, so keep it fixed.
        ids.sort_unstable();
        let mut plan = Plan {
            poles: Vec::new(),
            by_node: FnvHashMap::default(),
            drives_on_left,
            road_mask,
        };
        for id in &ids {
            plan.node(*id, &nodes[id]);
        }
        Self {
            poles: plan.poles,
            by_node: plan.by_node,
        }
    }

    /// Builds the poles and heads this node owns.
    pub fn place(
        &self,
        editor: &mut WorldEditor,
        node: &ProcessedNode,
        base_y: &dyn Fn(&WorldEditor, i32, i32, i32) -> i32,
    ) {
        for &i in self.by_node.get(&node.id).into_iter().flatten() {
            let pole = &self.poles[i];
            let base = base_y(editor, pole.x, pole.z, 1);
            let mut builder = Builder {
                editor,
                seed_base: i as i64 * 1_000,
                parts: 0,
            };
            builder.post(pole.x, pole.z, base, pole.top);
            for head in &pole.heads {
                let (y, scale) = match head.kind {
                    Kind::Car => (CAR_HEAD_Y, 1.0),
                    Kind::Bike => (BIKE_HEAD_Y, BIKE_SCALE),
                    Kind::Pedestrian => (PED_HEAD_Y, 1.0),
                };
                let centre = pole_front(pole.x, pole.z, head.facing, base as f64 + y, scale);
                builder.head(centre, *head);
            }
            if let Some((end, head)) = pole.mast {
                builder.mast_arm((pole.x, pole.z), end, base + MAST_Y);
                let hang = (
                    end.0 as f64 + 0.5,
                    (base + MAST_Y) as f64 - 0.62,
                    end.1 as f64 + 0.5,
                );
                builder.head(hang, head);
            }
        }
    }
}

fn kerb_side(travel: (f64, f64), drives_on_left: bool) -> (f64, f64) {
    if drives_on_left {
        (travel.1, -travel.0)
    } else {
        (-travel.1, travel.0)
    }
}

/// The first cell off the road surface beside `origin`, and how far out it is.
fn kerb_cell(
    origin: (f64, f64),
    side: (f64, f64),
    half: i32,
    road_mask: &RoadMaskBitmap,
) -> Option<(i32, i32, i32)> {
    (half + 1..=half + 4).find_map(|off| {
        let x = (origin.0 + side.0 * off as f64).floor() as i32;
        let z = (origin.1 + side.1 * off as f64).floor() as i32;
        (!road_mask.contains(x, z)).then_some((x, z, off))
    })
}

/// Where a head of this scale hanging on the pole's `facing` side has its centre.
fn pole_front(px: i32, pz: i32, facing: (f64, f64), y: f64, scale: f64) -> (f64, f64, f64) {
    // Half the iron bar post plus half the housing depth.
    let out = 0.0625 + 0.15 * scale;
    (
        px as f64 + 0.5 + facing.0 * out,
        y,
        pz as f64 + 0.5 + facing.1 * out,
    )
}

struct Builder<'e, 'a> {
    editor: &'e mut WorldEditor<'a>,
    seed_base: i64,
    parts: i64,
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

    fn head(&mut self, centre: (f64, f64, f64), head: Head) {
        match head.kind {
            Kind::Car => self.car_head(centre, head.facing, head.group, 1.0),
            Kind::Bike => self.car_head(centre, head.facing, head.group, BIKE_SCALE),
            Kind::Pedestrian => self.ped_head(centre, head.facing, head.group),
        }
    }

    fn mast_arm(&mut self, from: (i32, i32), to: (i32, i32), y: i32) {
        // Bars join on their faces only, so a diagonal arm needs a corner at every step.
        let cells = four_connected_line(
            &bresenham_line(from.0, 0, from.1, to.0, 0, to.1)
                .into_iter()
                .map(|(x, _, z)| (x, z))
                .collect::<Vec<_>>(),
        );
        for &(x, z) in &cells {
            let joins = |dx: i32, dz: i32| cells.contains(&(x + dx, z + dz));
            self.editor.set_block_with_properties_absolute(
                connected_iron_bars(joins(0, -1), joins(0, 1), joins(1, 0), joins(-1, 0)),
                x,
                y,
                z,
                None,
                None,
            );
        }
    }

    /// A three-lamp head; bike heads are the same at a smaller `scale`.
    fn car_head(&mut self, centre: (f64, f64, f64), facing: (f64, f64), group: char, scale: f64) {
        let k = scale as f32;
        self.part(
            centre,
            facing,
            0.0,
            0.0,
            (0.42 * k, 1.10 * k, 0.30 * k),
            HOUSING,
            Vec::new(),
            None,
        );
        for (lamp, y) in [(Lamp::Red, 0.34), (Lamp::Amber, 0.0), (Lamp::Green, -0.34)] {
            let lit = match group {
                'a' => lamp == Lamp::Green,
                _ => lamp == Lamp::Red,
            };
            self.lamp(
                centre,
                facing,
                y * scale,
                0.17 * scale,
                0.26 * k,
                lamp,
                format!("arnis.tl_{group}_{}", lamp.tag()),
                lit,
            );
            self.part(
                centre,
                facing,
                (y + 0.15) * scale,
                0.225 * scale,
                (0.32 * k, 0.035 * k, 0.15 * k),
                HOUSING,
                Vec::new(),
                None,
            );
        }
    }

    fn ped_head(&mut self, centre: (f64, f64, f64), facing: (f64, f64), group: char) {
        self.part(
            centre,
            facing,
            0.0,
            0.0,
            (0.34, 0.72, 0.26),
            HOUSING,
            Vec::new(),
            None,
        );
        for (lamp, y) in [(Lamp::Red, 0.17), (Lamp::Green, -0.17)] {
            // Walkers cross road a while group b has its green.
            let lit = match group {
                'a' => lamp == Lamp::Red,
                _ => lamp == Lamp::Green,
            };
            self.lamp(
                centre,
                facing,
                y,
                0.15,
                0.24,
                lamp,
                format!("arnis.tl_p{group}_{}", lamp.tag()),
                lit,
            );
            self.part(
                centre,
                facing,
                y + 0.135,
                0.19,
                (0.28, 0.03, 0.12),
                HOUSING,
                Vec::new(),
                None,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lamp(
        &mut self,
        centre: (f64, f64, f64),
        facing: (f64, f64),
        y: f64,
        forward: f64,
        size: f32,
        lamp: Lamp,
        tag: String,
        lit: bool,
    ) {
        let brightness = if lit { (15, 15) } else { (3, 0) };
        self.part(
            centre,
            facing,
            y,
            forward,
            (size, size, 0.04),
            lamp.block(),
            vec!["arnis.tl".to_string(), tag],
            Some(brightness),
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
        // Yaw turns the model's +z towards `facing`.
        let yaw = (-facing.0).atan2(facing.1).to_degrees() as f32;
        extra.insert(
            "Rotation".to_string(),
            Value::List(vec![Value::Float(yaw), Value::Float(0.0)]),
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
        let index = SignalIndex::build(&elements, 1.0, false, &mask);
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
            SignalIndex::build(els, 1.0, false, &mask)
                .poles
                .iter()
                .map(|p| p.heads.len())
                .sum::<usize>()
        };
        assert_eq!(count(&mapped_twice), count(&alone));
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
        let index = SignalIndex::build(&[crossing], 1.0, false, &mask);
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
        let index = SignalIndex::build(&[path], 1.0, false, &mask);
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
        let index = SignalIndex::build(&elements, 1.0, false, &mask);
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
        let index = SignalIndex::build(&elements, 1.0, false, &road_mask(std::iter::empty()));
        assert_eq!(heads_of(&index, Kind::Pedestrian).len(), 2);
    }

    #[test]
    fn kerb_side_follows_the_driving_side() {
        // Driving north, the right-hand kerb is to the east.
        assert_eq!(kerb_side((0.0, -1.0), false), (1.0, 0.0));
        assert_eq!(kerb_side((0.0, -1.0), true), (-1.0, 0.0));
    }
}
