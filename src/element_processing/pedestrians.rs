//! People walking the mapped footways, sidewalks, pedestrian streets and paths, run by the
//! extras data pack. A lane each way along every way, more on a wide pedestrian street, the
//! ways on at each junction, where walkers wait at a kerb or a light and how busy each way
//! is are worked out here and written into the pack as storage. Each walker is a mannequin
//! (a villager before 1.21.9) the pack carries from point to point; the storage layout and
//! the scores are the contract with `assets/minecraft/datapack_extras/data/arnis/function/walk`.

use crate::clipping::is_invented_node_id;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::datapack::macro_coord;
use crate::element_processing::cars::{drivable, lane_line};
use crate::element_processing::highways::highway_block_range;
use crate::element_processing::traffic_lights::group_of;
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::water_probe::WalkProbe;
use fnv::FnvHashMap;
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
const WALK_MPS: f64 = 1.4;
const PACES: [f64; 3] = [0.85, 1.0, 1.2];
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
    let closed = |k: &str| matches!(get(k), Some("no" | "private" | "use_sidepath"));
    let bridge = matches!(get("bridge"), Some(v) if v != "no");
    // Underground and indoor ways are not where the ground says they are; a pier a layer
    // up or a way under a roof is found on what was built, like any other way.
    let off_grade = matches!(get("tunnel"), Some(v) if v != "no")
        || get("layer")
            .and_then(|l| l.parse::<i32>().ok())
            .is_some_and(|l| l < 0 && !bridge)
        || get("level").is_some_and(|l| l != "0");
    if off_grade
        || get("area") == Some("yes")
        || get("indoor") == Some("yes")
        || tags.contains_key("conveying")
        || closed("access") && !yes("foot")
        || closed("foot")
    {
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
}

/// The walkable ways of an area.
pub struct WalkNetwork {
    lanes: Vec<Lane>,
    /// Where walkers can join, as (lane, point) by map cell.
    spots: BTreeMap<(i32, i32), Vec<(usize, usize)>>,
    meta: Vec<Meta>,
    size: f64,
    scale: f64,
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
        let ways: Vec<(&ProcessedWay, i32, f64)> = elements
            .iter()
            .filter_map(|e| match e {
                ProcessedElement::Way(w) if w.nodes.len() >= 2 => {
                    let density = walk_density(&w.tags)?;
                    let half = highway_block_range(&w.tags["highway"], &w.tags, scale);
                    Some((w, half, density))
                }
                _ => None,
            })
            .collect();
        // How wide the road is at every node a road runs through, where a walker crosses:
        // the widest, the first road there and whether any other meets it.
        let mut roads: FnvHashMap<u64, (i32, u64, bool)> = FnvHashMap::default();
        let mut draws: Vec<(f64, f64)> = Vec::new();
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

        let mut uses: FnvHashMap<u64, usize> = FnvHashMap::default();
        for (w, _, _) in &ways {
            for n in &w.nodes {
                *uses.entry(n.id).or_default() += 1;
            }
        }
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
                    for k in 1..path.points.len().saturating_sub(1) {
                        marks.push((path.at[k], None, false));
                    }
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
                    points += pts.len();
                    let mid = path.point_at(path.length() / 2.0);
                    lanes.push(Lane {
                        points: pts,
                        alt,
                        next: Vec::new(),
                        back: None,
                        cut: (false, false),
                        beside: Vec::new(),
                        out: at_edge(nodes[nodes.len() - 1]),
                        density: piece.density * footfall.at(mid) * 2.0 / (count as f64 + 1.0),
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
        let spots = spots(&lanes, &meta, scale);
        Self {
            lanes,
            spots,
            meta,
            size,
            scale,
        }
    }

    /// The columns every block along each lane, and along the middle of its way, whose
    /// ground is read once the world is built.
    pub fn probe(&self) -> WalkProbe {
        WalkProbe::new(self.lanes.iter().flat_map(|l| {
            steps(&l.points)
                .into_iter()
                .chain(steps(&l.alt))
                .map(|s| s.column())
        }))
    }

    /// Puts every lane on the ground as it was built, past a post or a tree but never
    /// through a wall or a house: a lane that would walks the middle of its way instead,
    /// or stops short of it, where its walkers turn round.
    pub fn finish(&mut self, probe: &WalkProbe) {
        for lane in &mut self.lanes {
            if lane.points.len() >= 2 {
                let side = settle(&lane.points, probe);
                let best = match side {
                    Some(s) if s.cut == (false, false) => Some(s),
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
                });
                lane.points = best.points;
                lane.cut = best.cut;
                lane.out &= !best.cut.1;
            }
            lane.alt = Vec::new();
        }
        if self.lanes.iter().map(|l| l.points.len()).sum::<usize>() > MAX_POINTS {
            prune(&mut self.lanes);
        }
        link(&mut self.lanes, &self.meta, self.scale);
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
    fn paces(&self) -> [i32; 3] {
        PACES.map(|p| (WALK_MPS * p * 0.1 * self.scale * 1000.0).round().max(20.0) as i32)
    }

    /// The pack functions: the lanes in storage, the moves at each pace and what a walker
    /// looks out for, all at the world's scale.
    pub fn pack_functions(&self) -> Vec<(String, String)> {
        let k = self.size;
        let d = |m: f64| format!("{:.2}", m * k);
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
        for p in paces {
            let _ = writeln!(
                moves,
                "execute if score @s arnis.wv matches {p} run return run tp @s ^ ^ ^{:.3}",
                f64::from(p) / 1000.0
            );
        }
        let ahead = format!(
            "# Someone right in front: a player, or after a while of waiting only a player.\n\
             execute positioned ^ ^ ^{near} if entity @a[gamemode=!spectator,tag=!arnis.riding,distance=..{round}] run return 1\n\
             execute if score @s arnis.wb matches ..29 positioned ^ ^ ^{near} if entity @e[tag=arnis.walker,distance=..{walker}] run return 1\n\
             return 0\n",
            near = d(0.9),
            round = d(0.7),
            walker = d(0.45),
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
        assert!(west.points.iter().all(|p| p.z < 0.0), "{:?}", west.points);
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
        assert_eq!(text("walk/move.mcfunction").lines().count(), 3);
        for (_, body) in &functions {
            for line in body.lines().filter(|l| !l.starts_with('#')) {
                assert!(!line.contains("$("), "{line}");
            }
        }
    }
}
