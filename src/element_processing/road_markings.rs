//! Junction-aware road paint, resolved once over every way before rendering: where lane
//! lines stop short of a crossing carriageway, stop and give-way lines, and pedestrian
//! crossings painted across the road.

use crate::block_definitions::Block;
use crate::bresenham::bresenham_line;
use crate::clipping::is_invented_node_id;
use crate::decals::region::SignRegion;
use crate::element_processing::highways::{
    highway_block_range, lane_marking_count, surface_palette,
};
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use fnv::{FnvHashMap, FnvHashSet};
use std::collections::HashMap;

/// Free cells between a crossing carriageway's edge and the first paint of an approach.
const MARGIN: i32 = 1;
/// How far along an approach a signal, stop or give-way node still belongs to the junction.
const SIGN_REACH_M: f64 = 30.0;
/// Shortest stretch of lane line left between the two nodes of a split junction.
const MIN_RUN_M: f64 = 8.0;
/// More arm directions than this at one node are left unpainted rather than tracked.
const MAX_ARMS: usize = 16;

/// Paint laid across a carriageway at one point of a way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossMark {
    /// Solid line where traffic stops.
    Stop,
    /// Broken line where traffic yields.
    GiveWay,
    /// One row of zebra bars.
    Zebra,
    /// One edge line of a crossing; `true` is broken.
    CrossingLines(bool),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransverseMark {
    /// Path index along the way, counted like the renderer's `tds`.
    pub t: u32,
    pub kind: CrossMark,
    /// Half of the road it spans: +1 right of the way's direction, -1 left, 0 both.
    pub side: i8,
}

impl TransverseMark {
    /// Whether the row at path index `tds` belongs to this mark.
    pub fn covers(&self, tds: u32) -> bool {
        tds == self.t
    }
}

/// Paint decisions for one way.
#[derive(Default, Debug)]
pub struct WayMarks {
    /// Inclusive path-index ranges without lane lines.
    pub gaps: Vec<(u32, u32)>,
    pub marks: Vec<TransverseMark>,
    /// Dash phase at path index 0 and its step per index, carried over from the way this
    /// one continues; a zero step means a phase of `t`.
    phase: (i64, i8),
}

impl WayMarks {
    pub fn in_gap(&self, t: u32) -> bool {
        self.gaps.iter().any(|&(a, b)| a <= t && t <= b)
    }

    /// Dash phase at path index `t`.
    pub fn dash_phase(&self, t: u32) -> i64 {
        match self.phase {
            (_, 0) => t as i64,
            (offset, step) => offset + step as i64 * t as i64,
        }
    }
}

/// What holds traffic back on a junction approach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Signal,
    Stop,
    GiveWay,
}

/// How a pedestrian crossing is painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossingPaint {
    Zebra,
    /// Edge lines only, as at signalised crossings in Germany or the UK.
    Lines {
        broken: bool,
    },
}

/// A carriageway a crossing way runs over, as the road's centreline through a shared node.
#[derive(Clone, Debug)]
pub struct CarriagewayClip {
    /// Road centreline cells near the crossing; the road paves `half` around each.
    centre: Vec<(i32, i32)>,
    half: i32,
    palette: &'static [Block],
}

impl CarriagewayClip {
    /// Surface blocks of the crossed road, which crossing paint may replace.
    pub fn palette(&self) -> &'static [Block] {
        self.palette
    }

    pub fn contains(&self, x: i32, z: i32) -> bool {
        self.centre
            .iter()
            .any(|&(cx, cz)| (x - cx).abs() <= self.half && (z - cz).abs() <= self.half)
    }
}

#[derive(Default)]
pub struct RoadMarkingIndex {
    ways: FnvHashMap<u64, WayMarks>,
    crossings: FnvHashMap<u64, Vec<CarriagewayClip>>,
    /// Traffic keeps left, so approaching lanes sit left of the centreline.
    pub drives_on_left: bool,
    /// The line between opposing traffic is yellow, as in North America.
    pub yellow_centre: bool,
    /// Signalised crossings get broken edge lines instead of a zebra.
    signal_crossing_lines: bool,
}

/// Vehicular carriageway attributes the junction rules need.
struct Road<'a> {
    way: &'a ProcessedWay,
    rank: u8,
    link: bool,
    ring: bool,
    /// +1 one-way along the way, -1 against it, 0 two-way.
    oneway: i8,
    half: i32,
    marked: bool,
    palette: &'static [Block],
    /// Path index of each node.
    node_t: Vec<u32>,
}

/// A centreline cell of an arm, with the way and path index that paint it.
#[derive(Clone, Copy)]
struct ArmCell {
    x: i32,
    z: i32,
    road: usize,
    t: u32,
    /// +1 when leaving the junction runs along this way, -1 against it.
    dir: i8,
}

/// One direction a road leaves a junction node in.
struct Arm {
    road: usize,
    node: usize,
    dir: i8,
    cells: Vec<ArmCell>,
    unit: (f32, f32),
}

/// Importance class and link flag of a vehicular highway; None for anything else.
fn road_class(highway: &str) -> Option<(u8, bool)> {
    Some(match highway {
        "motorway" | "trunk" => (5, false),
        "motorway_link" | "trunk_link" => (4, true),
        "primary" => (4, false),
        "primary_link" => (3, true),
        "secondary" => (3, false),
        "secondary_link" => (2, true),
        "tertiary" => (2, false),
        "tertiary_link" => (1, true),
        "unclassified" | "residential" | "living_street" | "road" | "busway" => (1, false),
        "service" | "track" => (0, false),
        _ => return None,
    })
}

pub(crate) fn is_roundabout(tags: &HashMap<String, String>) -> bool {
    matches!(
        tags.get("junction").map(String::as_str),
        Some("roundabout" | "circular")
    )
}

/// +1 when traffic only runs along the way, -1 when only against it, else 0.
pub(crate) fn oneway_sign(highway: &str, tags: &HashMap<String, String>) -> i8 {
    match tags.get("oneway").map(String::as_str) {
        Some("yes" | "true" | "1") => 1,
        Some("-1" | "reverse") => -1,
        Some("no" | "false" | "0" | "reversible" | "alternating") => 0,
        _ if is_roundabout(tags) => 1,
        _ if matches!(highway, "motorway" | "motorway_link") => 1,
        _ => 0,
    }
}

/// Ways the renderer skips, so they neither paint nor count as junction arms.
fn renders_on_surface(tags: &HashMap<String, String>) -> bool {
    tags.get("area").map(String::as_str) != Some("yes")
        && tags.get("indoor").map(String::as_str) != Some("yes")
        && tags
            .get("level")
            .and_then(|l| l.parse::<i32>().ok())
            .is_none_or(|l| l >= 0)
}

/// Path index of every node, matching the renderer's cumulative bresenham count.
fn node_path_index(way: &ProcessedWay) -> Vec<u32> {
    let mut out = Vec::with_capacity(way.nodes.len());
    let mut t = 0u32;
    out.push(0);
    for pair in way.nodes.windows(2) {
        t += (pair[1].x - pair[0].x)
            .unsigned_abs()
            .max((pair[1].z - pair[0].z).unsigned_abs());
        out.push(t);
    }
    out
}

/// Centreline cells leaving node `i` of `road` in direction `dir`, the node first, carried
/// on where the way just continues as another. Each segment is rasterised forwards, as the
/// renderer does, so the cells are the ones it paints.
fn arm_cells(
    roads: &[Road],
    joins: &FnvHashMap<u64, Vec<(usize, usize)>>,
    road: usize,
    i: usize,
    dir: i8,
    max_len: usize,
) -> Vec<ArmCell> {
    let (mut r, mut k, mut d) = (road, i, dir);
    let first = &roads[r].way.nodes[k];
    let mut cells = vec![ArmCell {
        x: first.x,
        z: first.z,
        road: r,
        t: roads[r].node_t[k],
        dir: d,
    }];
    let mut hops = 0;
    while cells.len() < max_len {
        let n = &roads[r].way.nodes;
        let next = if d > 0 {
            (k + 1 < n.len()).then(|| (k, k + 1))
        } else {
            (k > 0).then(|| (k - 1, k))
        };
        let Some((a, b)) = next else {
            // Carry on only through a plain joint of two way ends.
            let Some(refs) = joins.get(&n[k].id) else {
                break;
            };
            let dirs = arm_directions(roads, refs);
            let onward = dirs.iter().find(|x| !(x.0 == r && x.1 == k));
            match onward {
                Some(&(r2, k2, d2)) if dirs.len() == 2 && hops < 8 => {
                    (r, k, d) = (r2, k2, d2);
                    hops += 1;
                    continue;
                }
                _ => break,
            }
        };
        let t0 = roads[r].node_t[k];
        let pts = bresenham_line(n[a].x, 0, n[a].z, n[b].x, 0, n[b].z);
        let cell = |j: usize, &(x, _, z): &(i32, i32, i32)| ArmCell {
            x,
            z,
            road: r,
            t: if d > 0 { t0 + j as u32 } else { t0 - j as u32 },
            dir: d,
        };
        if d > 0 {
            cells.extend(pts.iter().enumerate().skip(1).map(|(j, p)| cell(j, p)));
        } else {
            cells.extend(
                pts.iter()
                    .rev()
                    .enumerate()
                    .skip(1)
                    .map(|(j, p)| cell(j, p)),
            );
        }
        k = if d > 0 { k + 1 } else { k - 1 };
    }
    cells.truncate(max_len);
    cells
}

/// Nodes along an arm up to `limit` cells out, with their distance, the junction excluded.
fn arm_nodes<'a, 'r>(
    roads: &'r [Road<'a>],
    arm: &'r Arm,
    limit: usize,
) -> impl Iterator<Item = (&'a ProcessedNode, usize)> + 'r {
    arm.cells
        .iter()
        .enumerate()
        .skip(1)
        .take(limit)
        .filter_map(|(s, c)| node_at(roads, c).map(|n| (n, s)))
}

/// The node a cell sits on, if it is one.
fn node_at<'a>(roads: &[Road<'a>], cell: &ArmCell) -> Option<&'a ProcessedNode> {
    let road = &roads[cell.road];
    let i = road.node_t.binary_search(&cell.t).ok()?;
    road.way.nodes.get(i)
}

fn unit(dx: f32, dz: f32) -> Option<(f32, f32)> {
    let len = (dx * dx + dz * dz).sqrt();
    (len > 1e-3).then(|| (dx / len, dz / len))
}

/// Direction of `cells` around index `s`, looking a couple of cells either way.
fn local_dir(cells: &[ArmCell], s: usize, fallback: (f32, f32)) -> (f32, f32) {
    let a = cells[s.saturating_sub(2)];
    let b = cells[(s + 2).min(cells.len() - 1)];
    unit((b.x - a.x) as f32, (b.z - a.z) as f32).unwrap_or(fallback)
}

/// Every centreline cell of a way, rasterised as the renderer does.
fn way_cells(way: &ProcessedWay) -> Vec<(i32, i32)> {
    let mut cells: Vec<(i32, i32)> = Vec::new();
    for pair in way.nodes.windows(2) {
        let pts = bresenham_line(pair[0].x, 0, pair[0].z, pair[1].x, 0, pair[1].z);
        let skip = usize::from(!cells.is_empty());
        cells.extend(pts.iter().skip(skip).map(|&(x, _, z)| (x, z)));
    }
    cells
}

/// Every direction the roads in `refs` leave their shared node in: (road, node, dir).
fn arm_directions(roads: &[Road], refs: &[(usize, usize)]) -> Vec<(usize, usize, i8)> {
    let mut dirs = Vec::new();
    for &(ri, ni) in refs {
        let t = roads[ri].node_t[ni];
        if t < *roads[ri].node_t.last().unwrap_or(&0) {
            dirs.push((ri, ni, 1));
        }
        if t > 0 {
            dirs.push((ri, ni, -1));
        }
    }
    dirs
}

/// Whether the signalised crossings tagged with their paint here mostly have edge lines
/// rather than zebras; None without any to go by.
fn signal_crossings_have_lines(elements: &[ProcessedElement]) -> Option<bool> {
    let (mut lines, mut zebras) = (0u32, 0u32);
    for element in elements {
        let tags = element.tags();
        let crossing = tags.get("highway").map(String::as_str) == Some("crossing")
            || tags.get("footway").map(String::as_str) == Some("crossing");
        if !crossing || !is_signalled(tags) {
            continue;
        }
        match tags.get("crossing:markings").map(String::as_str) {
            Some("lines" | "lines:paired" | "dashes" | "dashes:paired" | "dots") => lines += 1,
            Some(m) if m.starts_with("zebra") || m.starts_with("ladder") => zebras += 1,
            _ => {}
        }
    }
    (lines + zebras > 0).then_some(lines > zebras)
}

/// A crossing controlled by signals, in either tagging.
fn is_signalled(tags: &HashMap<String, String>) -> bool {
    tags.get("crossing").map(String::as_str) == Some("traffic_signals")
        || tags.get("crossing:signals").map(String::as_str) == Some("yes")
}

fn is_signal(node: &ProcessedNode) -> bool {
    node.tags.get("highway").map(String::as_str) == Some("traffic_signals")
}

/// Small square grid of arm footprints around one junction, one bit per arm.
struct LocalGrid {
    x0: i32,
    z0: i32,
    size: i32,
    bits: Vec<u16>,
}

impl LocalGrid {
    fn new(cx: i32, cz: i32, radius: i32) -> Self {
        let size = 2 * radius + 1;
        Self {
            x0: cx - radius,
            z0: cz - radius,
            size,
            bits: vec![0; (size * size) as usize],
        }
    }

    fn stamp(&mut self, x: i32, z: i32, r: i32, bit: u16) {
        let lx0 = (x - r - self.x0).max(0);
        let lx1 = (x + r - self.x0).min(self.size - 1);
        let lz0 = (z - r - self.z0).max(0);
        let lz1 = (z + r - self.z0).min(self.size - 1);
        for lz in lz0..=lz1 {
            for lx in lx0..=lx1 {
                self.bits[(lz * self.size + lx) as usize] |= bit;
            }
        }
    }

    fn get(&self, x: i32, z: i32) -> u16 {
        let lx = x - self.x0;
        let lz = z - self.z0;
        if lx < 0 || lz < 0 || lx >= self.size || lz >= self.size {
            return 0;
        }
        self.bits[(lz * self.size + lx) as usize]
    }
}

impl RoadMarkingIndex {
    pub fn way(&self, id: u64) -> Option<&WayMarks> {
        self.ways.get(&id)
    }

    /// Carriageways a crossing way runs over; None when it shares no node with a road.
    pub fn crossing_clips(&self, id: u64) -> Option<&[CarriagewayClip]> {
        self.crossings.get(&id).map(Vec::as_slice)
    }

    /// Paint of a crossing from its tags; `untagged` is used when nothing says either way.
    fn crossing_paint(
        &self,
        tags: &HashMap<String, String>,
        untagged: Option<CrossingPaint>,
    ) -> Option<CrossingPaint> {
        let crossing = tags.get("crossing").map(String::as_str);
        if matches!(
            crossing,
            Some("no" | "unmarked" | "informal" | "impossible")
        ) {
            return None;
        }
        match tags.get("crossing:markings").map(String::as_str) {
            Some("no" | "surface") => return None,
            Some("lines" | "lines:paired") => return Some(CrossingPaint::Lines { broken: false }),
            Some("dashes" | "dots" | "dashes:paired") => {
                return Some(CrossingPaint::Lines { broken: true })
            }
            Some(_) => return Some(CrossingPaint::Zebra),
            None => {}
        }
        if tags.get("crossing_ref").map(String::as_str) == Some("zebra") {
            return Some(CrossingPaint::Zebra);
        }
        match crossing {
            Some("zebra") => Some(CrossingPaint::Zebra),
            _ if is_signalled(tags) => Some(if self.signal_crossing_lines {
                CrossingPaint::Lines { broken: true }
            } else {
                CrossingPaint::Zebra
            }),
            Some(_) => Some(CrossingPaint::Zebra),
            None => untagged,
        }
    }

    /// Paint of a `footway=crossing` way, which renders as a zebra unless tagged otherwise.
    pub fn crossing_way_paint(&self, tags: &HashMap<String, String>) -> Option<CrossingPaint> {
        if tags.get("highway").map(String::as_str) != Some("footway")
            || tags.get("footway").map(String::as_str) != Some("crossing")
        {
            return None;
        }
        self.crossing_paint(tags, Some(CrossingPaint::Zebra))
    }

    /// Paint of a crossing mapped only as a node on the road.
    fn crossing_node_paint(&self, node: &ProcessedNode) -> Option<CrossingPaint> {
        let highway = node.tags.get("highway").map(String::as_str);
        let on_road = highway == Some("crossing")
            || (highway == Some("traffic_signals") && node.tags.contains_key("crossing"));
        if !on_road {
            return None;
        }
        self.crossing_paint(&node.tags, None)
    }

    pub fn build(elements: &[ProcessedElement], scale: f64, region: SignRegion) -> Self {
        let mut index = Self {
            drives_on_left: region.drives_on_left(),
            yellow_centre: matches!(region, SignRegion::NorthAmerica | SignRegion::Canada),
            signal_crossing_lines: signal_crossings_have_lines(elements).unwrap_or(matches!(
                region,
                SignRegion::Germanic | SignRegion::UkIreland
            )),
            ..Self::default()
        };

        let mut roads: Vec<Road> = Vec::new();
        let mut crossing_ways: Vec<&ProcessedWay> = Vec::new();
        for element in elements {
            let ProcessedElement::Way(way) = element else {
                continue;
            };
            let Some(highway) = way.tags.get("highway") else {
                continue;
            };
            if way.nodes.len() < 2 || !renders_on_surface(&way.tags) {
                continue;
            }
            if let Some((rank, link)) = road_class(highway) {
                roads.push(Road {
                    way,
                    rank,
                    link,
                    ring: is_roundabout(&way.tags),
                    oneway: oneway_sign(highway, &way.tags),
                    half: highway_block_range(highway, &way.tags, scale),
                    marked: lane_marking_count(highway, &way.tags) >= 2,
                    palette: surface_palette(highway, &way.tags),
                    node_t: node_path_index(way),
                });
            } else if index.crossing_way_paint(&way.tags).is_some() {
                crossing_ways.push(way);
            }
        }

        // Most nodes belong to one way only; index just the shared ones.
        let mut uses: FnvHashMap<u64, u32> = FnvHashMap::default();
        let mut count_way = |way: &ProcessedWay| {
            let closed =
                way.nodes.len() > 2 && way.nodes[0].id == way.nodes[way.nodes.len() - 1].id;
            let nodes = if closed {
                &way.nodes[1..]
            } else {
                &way.nodes[..]
            };
            for node in nodes {
                if !is_invented_node_id(node.id) {
                    *uses.entry(node.id).or_insert(0) += 1;
                }
            }
        };
        roads.iter().for_each(|r| count_way(r.way));
        crossing_ways.iter().for_each(|w| count_way(w));
        uses.retain(|_, n| *n >= 2);

        let mut road_arms: FnvHashMap<u64, Vec<(usize, usize)>> = FnvHashMap::default();
        for (ri, road) in roads.iter().enumerate() {
            for (ni, node) in road.way.nodes.iter().enumerate() {
                if uses.contains_key(&node.id) {
                    road_arms.entry(node.id).or_default().push((ri, ni));
                }
            }
        }
        let mut crossing_arms: FnvHashMap<u64, Vec<(usize, usize)>> = FnvHashMap::default();
        for (ci, way) in crossing_ways.iter().enumerate() {
            for (ni, node) in way.nodes.iter().enumerate() {
                if road_arms.contains_key(&node.id) {
                    crossing_arms.entry(node.id).or_default().push((ci, ni));
                }
            }
        }

        // Painted crossing nodes and how far their paint reaches along the road.
        let mut crossing_reach: FnvHashMap<u64, u32> = FnvHashMap::default();

        // Crossing ways: clip their paint to the carriageways they cross, keep lane lines off them.
        // Zebra bars fill a crossing's rendered width; edge lines sit a row outside it.
        let paint_half: Vec<i32> = crossing_ways
            .iter()
            .map(|w| {
                let half = highway_block_range("footway", &w.tags, scale);
                match index.crossing_way_paint(&w.tags) {
                    Some(CrossingPaint::Lines { .. }) => half + 1,
                    _ => half,
                }
            })
            .collect();
        for (node_id, crossers) in &crossing_arms {
            for &(ri, ni) in &road_arms[node_id] {
                let road = &roads[ri];
                for &(ci, _) in crossers {
                    let crossing = way_cells(crossing_ways[ci]);
                    let paint = paint_half[ci];
                    // Chebyshev distance to the crossing centreline: square stamps, as rendered.
                    let to_crossing = |x: i32, z: i32| {
                        crossing
                            .iter()
                            .map(|&(cx, cz)| (x - cx).abs().max((z - cz).abs()))
                            .min()
                            .unwrap_or(i32::MAX)
                    };
                    // The road either side of the node, as far as the crossing could run along it.
                    let limit = crossing.len() + (road.half + paint) as usize + 2;
                    let sides = [
                        arm_cells(&roads, &road_arms, ri, ni, 1, limit),
                        arm_cells(&roads, &road_arms, ri, ni, -1, limit),
                    ];
                    let centre: Vec<(i32, i32)> = sides
                        .iter()
                        .flat_map(|cells| cells.iter())
                        .map(|c| (c.x, c.z))
                        .filter(|&(x, z)| to_crossing(x, z) <= road.half + paint)
                        .collect();
                    let clip = CarriagewayClip {
                        centre,
                        half: road.half,
                        palette: road.palette,
                    };
                    // Cells the crossing paints: its width, on this carriageway only.
                    let painted: FnvHashSet<(i32, i32)> = crossing
                        .iter()
                        .flat_map(|&(cx, cz)| {
                            (-paint..=paint).flat_map(move |dx| {
                                (-paint..=paint).map(move |dz| (cx + dx, cz + dz))
                            })
                        })
                        .filter(|&(x, z)| clip.contains(x, z))
                        .collect();
                    let near_paint = |x: i32, z: i32| {
                        (-1..=1).any(|dx| (-1..=1).any(|dz| painted.contains(&(x + dx, z + dz))))
                    };
                    index
                        .crossings
                        .entry(crossing_ways[ci].id)
                        .or_default()
                        .push(clip);
                    // Lane lines keep a clear row off every cell the crossing paints.
                    let mut reach = 0;
                    for cells in &sides {
                        let last = (0..cells.len()).rev().find(|&k| {
                            let (ux, uz) = local_dir(cells, k, (1.0, 0.0));
                            let (px, pz) = (-uz, ux);
                            let w = (road.half as f32 * (px.abs() + pz.abs())).round() as i32;
                            (-w..=w).any(|p| {
                                let x = cells[k].x + (px * p as f32).round() as i32;
                                let z = cells[k].z + (pz * p as f32).round() as i32;
                                near_paint(x, z)
                            })
                        });
                        if let Some(k) = last {
                            index.clear_cells(&roads, &cells[..=k]);
                            reach = reach.max(k as u32);
                        }
                    }
                    let r = crossing_reach.entry(*node_id).or_insert(reach);
                    *r = (*r).max(reach);
                }
            }
        }

        // Signal-controlled crossings, one road position each: (road, node, reach, id).
        let mut signal_crossings: Vec<(usize, usize, u32, u64)> = Vec::new();
        for (node_id, crossers) in &crossing_arms {
            let &(ri, ni) = &road_arms[node_id][0];
            let signalled = is_signalled(&roads[ri].way.nodes[ni].tags)
                || crossers
                    .iter()
                    .any(|&(ci, _)| is_signalled(&crossing_ways[ci].tags));
            if let (true, Some(&reach)) = (signalled, crossing_reach.get(node_id)) {
                signal_crossings.push((ri, ni, reach, *node_id));
            }
        }

        // Crossings mapped only as a node on the road, painted row by row so a crossing at
        // a way split lands on both ways.
        let near_crossing = (8.0 * scale).max(2.0) as usize;
        let is_crossing = |n: &ProcessedNode| {
            crossing_arms.contains_key(&n.id)
                || n.tags.get("highway").map(String::as_str) == Some("crossing")
        };
        for (ri, road) in roads.iter().enumerate() {
            for (ni, node) in road.way.nodes.iter().enumerate() {
                if node.tags.is_empty()
                    || crossing_arms.contains_key(&node.id)
                    || crossing_reach.contains_key(&node.id)
                {
                    continue;
                }
                let Some(paint) = index.crossing_node_paint(node) else {
                    continue;
                };
                // Signal heads tagged for a crossing mapped beside them are not the crossing.
                let beside_crossing = [1i8, -1].iter().any(|&dir| {
                    arm_cells(&roads, &road_arms, ri, ni, dir, near_crossing + 1)
                        .iter()
                        .skip(1)
                        .any(|c| node_at(&roads, c).is_some_and(is_crossing))
                });
                if is_signal(node) && beside_crossing {
                    continue;
                }
                // A crossing on a junction node has no single road to run across.
                if road_arms
                    .get(&node.id)
                    .is_some_and(|refs| arm_directions(&roads, refs).len() > 2)
                {
                    continue;
                }
                let (kind, rows, reach): (CrossMark, &[i32], u32) = match paint {
                    CrossingPaint::Zebra => (CrossMark::Zebra, &[-1, 0, 1], 2),
                    CrossingPaint::Lines { broken } => {
                        (CrossMark::CrossingLines(broken), &[-2, 2], 3)
                    }
                };
                index.clear_around(&roads, &road_arms, ri, ni, reach as usize);
                let ahead = arm_cells(&roads, &road_arms, ri, ni, 1, 3);
                let behind = arm_cells(&roads, &road_arms, ri, ni, -1, 3);
                for &row in rows {
                    let cell = if row >= 0 {
                        ahead.get(row as usize)
                    } else {
                        behind.get(row.unsigned_abs() as usize)
                    };
                    if let Some(c) = cell {
                        index
                            .ways
                            .entry(roads[c.road].way.id)
                            .or_default()
                            .marks
                            .push(TransverseMark {
                                t: c.t,
                                kind,
                                side: 0,
                            });
                    }
                }
                crossing_reach.insert(node.id, reach);
                if is_signalled(&node.tags) {
                    signal_crossings.push((ri, ni, reach, node.id));
                }
            }
        }

        // Nodes where streets meet; driveways and tracks joining do not make one.
        let junctions: FnvHashSet<u64> = road_arms
            .iter()
            .filter(|(_, refs)| {
                let streets: Vec<(usize, usize)> = refs
                    .iter()
                    .copied()
                    .filter(|&(ri, _)| roads[ri].rank > 0)
                    .collect();
                arm_directions(&roads, &streets).len() > 2
            })
            .map(|(id, _)| *id)
            .collect();

        // Pairs of way ends that carry one painted line on: (road, t, road, t).
        let mut links: Vec<(usize, u32, usize, u32)> = Vec::new();
        let sign_reach = (SIGN_REACH_M * scale).max(4.0) as u32;
        let min_run = (MIN_RUN_M * scale).max(3.0) as usize;
        let mut signalised: FnvHashSet<u64> = FnvHashSet::default();
        for (node_id, refs) in &road_arms {
            if let [a, b] = arm_directions(&roads, refs)[..] {
                if a.0 != b.0 {
                    links.push((a.0, roads[a.0].node_t[a.1], b.0, roads[b.0].node_t[b.1]));
                }
                continue;
            }
            let signals = index.resolve_junction(
                &roads,
                &road_arms,
                &junctions,
                refs,
                (sign_reach, min_run),
                &crossing_reach,
                &mut links,
            );
            if signals {
                signalised.insert(*node_id);
            }
        }
        index.chain_dash_phases(&roads, &links);

        for (ri, ni, reach, _) in signal_crossings {
            index.signal_crossing_stops(
                &roads,
                &road_arms,
                (&junctions, &signalised),
                &crossing_reach,
                ri,
                ni,
                reach,
            );
        }

        for marks in index.ways.values_mut() {
            marks.gaps.sort_unstable();
            marks.marks.sort_by_key(|m| m.t);
            // A junction and a crossing next to it can ask for the same line.
            let mut kept: Vec<TransverseMark> = Vec::with_capacity(marks.marks.len());
            marks.marks.retain(|m| {
                let new = !kept.contains(m);
                kept.push(*m);
                new
            });
        }
        index
    }

    /// Stop lines either side of a signalised crossing, on the approaches a nearby junction
    /// does not already control.
    #[allow(clippy::too_many_arguments)]
    fn signal_crossing_stops(
        &mut self,
        roads: &[Road],
        joins: &FnvHashMap<u64, Vec<(usize, usize)>>,
        (junctions, signalised): (&FnvHashSet<u64>, &FnvHashSet<u64>),
        crossing_reach: &FnvHashMap<u64, u32>,
        ri: usize,
        ni: usize,
        reach: u32,
    ) {
        let half = roads[ri].half as usize;
        let clear = reach as usize + 2 * half + 14;
        // The line needs the free row plus a crossing road's width before any junction.
        let room = reach as usize + 2 * half + 3;
        let mut stops = Vec::new();
        for dir in [1i8, -1] {
            let cells = arm_cells(roads, joins, ri, ni, dir, clear + 16);
            // Traffic leaving signals is held there; a line squeezed into a junction is no line.
            let held = cells.iter().enumerate().skip(1).take(clear).any(|(s, c)| {
                node_at(roads, c).is_some_and(|n| {
                    signalised.contains(&n.id) || (s <= room && junctions.contains(&n.id))
                })
            });
            if held {
                continue;
            }
            // Behind any crossing right next to this one too, never on its paint.
            let mut at = reach as usize + 1;
            for (k, c) in cells.iter().enumerate().skip(1) {
                let Some(&r) = node_at(roads, c).and_then(|n| crossing_reach.get(&n.id)) else {
                    continue;
                };
                if k.saturating_sub(r as usize) > at {
                    break;
                }
                at = at.max(k + r as usize + 1);
            }
            let Some(c) = cells.get(at) else {
                continue;
            };
            // Only traffic heading for the crossing stops on this side of it.
            let road = &roads[c.road];
            if road.oneway != 0 && road.oneway != -c.dir {
                continue;
            }
            let side = if road.oneway != 0 {
                0
            } else if self.drives_on_left {
                c.dir
            } else {
                -c.dir
            };
            stops.push((road.way.id, c.t, side));
        }
        for (id, t, side) in stops {
            self.ways.entry(id).or_default().marks.push(TransverseMark {
                t,
                kind: CrossMark::Stop,
                side,
            });
        }
    }

    /// Keeps lane lines off `reach` cells either side of node `ni`, across plain joints.
    fn clear_around(
        &mut self,
        roads: &[Road],
        joins: &FnvHashMap<u64, Vec<(usize, usize)>>,
        ri: usize,
        ni: usize,
        reach: usize,
    ) {
        for dir in [1i8, -1] {
            let cells = arm_cells(roads, joins, ri, ni, dir, reach + 1);
            self.clear_cells(roads, &cells);
        }
    }

    /// Keeps lane lines off a run of arm cells, one gap per way it crosses.
    fn clear_cells(&mut self, roads: &[Road], cells: &[ArmCell]) {
        let mut run_start = 0;
        for s in 1..=cells.len() {
            if s < cells.len() && cells[s].road == cells[run_start].road {
                continue;
            }
            let first = &cells[run_start];
            if roads[first.road].marked {
                let last = &cells[s - 1];
                // The joint cell is this way's end node too.
                let start = if run_start > 0 {
                    (first.t as i64 - first.dir as i64).max(0) as u32
                } else {
                    first.t
                };
                self.ways
                    .entry(roads[first.road].way.id)
                    .or_default()
                    .gaps
                    .push((start.min(last.t), start.max(last.t)));
            }
            run_start = s;
        }
    }

    /// Carries the dash rhythm from way to way along each painted line, so a split way
    /// does not restart its dashes at the joint.
    fn chain_dash_phases(&mut self, roads: &[Road], links: &[(usize, u32, usize, u32)]) {
        let mut adjacent: Vec<Vec<(u32, usize, u32)>> = vec![Vec::new(); roads.len()];
        for &(a, ta, b, tb) in links {
            if roads[a].marked && roads[b].marked {
                adjacent[a].push((ta, b, tb));
                adjacent[b].push((tb, a, ta));
            }
        }
        let mut phase: Vec<Option<(i64, i8)>> = vec![None; roads.len()];
        let mut queue = Vec::new();
        for start in 0..roads.len() {
            if adjacent[start].is_empty() || phase[start].is_some() {
                continue;
            }
            phase[start] = Some((0, 1));
            queue.push(start);
            while let Some(a) = queue.pop() {
                let (oa, sa) = phase[a].unwrap_or((0, 1));
                for &(ta, b, tb) in &adjacent[a] {
                    if phase[b].is_some() {
                        continue;
                    }
                    // Phase step per cell walking along a into the joint, kept walking out along b.
                    let step = if ta == 0 { -sa } else { sa };
                    let sb = if tb == 0 { step } else { -step };
                    let at_joint = oa + sa as i64 * ta as i64;
                    phase[b] = Some((at_joint - sb as i64 * tb as i64, sb));
                    queue.push(b);
                }
            }
        }
        for (road, p) in roads.iter().zip(phase) {
            if let Some(p) = p {
                self.ways.entry(road.way.id).or_default().phase = p;
            }
        }
    }

    /// Cuts lane lines back from the other carriageways at one node and adds stop or
    /// give-way lines where an approach has to wait.
    #[allow(clippy::too_many_arguments)]
    fn resolve_junction(
        &mut self,
        roads: &[Road],
        joins: &FnvHashMap<u64, Vec<(usize, usize)>>,
        junctions: &FnvHashSet<u64>,
        refs: &[(usize, usize)],
        (sign_reach, min_run): (u32, usize),
        crossing_reach: &FnvHashMap<u64, u32>,
        links: &mut Vec<(usize, u32, usize, u32)>,
    ) -> bool {
        let dirs = arm_directions(roads, refs);
        if dirs.len() < 3 || dirs.len() > MAX_ARMS {
            return false;
        }
        // Most junctions join unmarked streets with nothing to paint.
        let has_sign = |n: &ProcessedNode| {
            matches!(
                n.tags.get("highway").map(String::as_str),
                Some("traffic_signals" | "stop" | "give_way")
            )
        };
        let max_half = dirs.iter().map(|d| roads[d.0].half).max().unwrap_or(1);
        // Signs and signals stand back from wide junctions by more.
        let near = sign_reach as usize + 2 * max_half as usize;
        let anything = dirs.iter().any(|&(ri, ni, dir)| {
            arm_cells(roads, joins, ri, ni, dir, near + 1)
                .iter()
                .any(|c| {
                    let road = &roads[c.road];
                    road.marked || road.ring || node_at(roads, c).is_some_and(has_sign)
                })
        });
        if !anything {
            return false;
        }

        let cap = (2 * (2 * max_half + MARGIN) + 8) as usize;
        let reach =
            (cap + 3 * max_half as usize + 4).max(sign_reach as usize + 2 * max_half as usize + 1);
        let arms: Vec<Arm> = dirs
            .iter()
            .filter_map(|&(road, node, dir)| {
                let cells = arm_cells(roads, joins, road, node, dir, reach);
                let far = cells[cells.len().min(7) - 1];
                let unit = unit((far.x - cells[0].x) as f32, (far.z - cells[0].z) as f32)?;
                Some(Arm {
                    road,
                    node,
                    dir,
                    cells,
                    unit,
                })
            })
            .collect();
        if arms.len() < 3 {
            return false;
        }

        // The nearest signal, stop or give-way sign on each arm controls its traffic.
        let controls: Vec<Option<Control>> = arms
            .iter()
            .map(|a| {
                arm_nodes(roads, a, near).find_map(|(n, _)| {
                    match n.tags.get("highway").map(String::as_str) {
                        Some("traffic_signals") => Some(Control::Signal),
                        Some("stop") => Some(Control::Stop),
                        Some("give_way") => Some(Control::GiveWay),
                        _ => None,
                    }
                })
            })
            .collect();
        let signal_arms: Vec<usize> = (0..arms.len())
            .filter(|&i| controls[i] == Some(Control::Signal))
            .collect();
        let signed = controls
            .iter()
            .any(|c| matches!(c, Some(Control::Stop | Control::GiveWay)));
        let junction = &roads[arms[0].road].way.nodes[arms[0].node];

        // The arm each arm carries straight on into; it never cuts the line.
        let name = |a: &Arm| roads[a.road].way.tags.get("name");
        let continuation: Vec<Option<usize>> = (0..arms.len())
            .map(|i| {
                let a = &arms[i];
                if roads[a.road].ring {
                    // A ring carries on into the ring, however tight it turns.
                    return (0..arms.len())
                        .filter(|&j| j != i && roads[arms[j].road].ring)
                        .min_by(|&j, &k| {
                            let dj = a.unit.0 * arms[j].unit.0 + a.unit.1 * arms[j].unit.1;
                            let dk = a.unit.0 * arms[k].unit.0 + a.unit.1 * arms[k].unit.1;
                            dj.total_cmp(&dk)
                        });
                }
                // A way running through the node carries on into its own other arm.
                (0..arms.len())
                    .filter(|&j| j != i)
                    .filter_map(|j| {
                        let dot = a.unit.0 * arms[j].unit.0 + a.unit.1 * arms[j].unit.1;
                        if dot > -0.5 {
                            return None;
                        }
                        let same_name = name(a).is_some() && name(a) == name(&arms[j]);
                        Some((j, dot - if same_name { 0.5 } else { 0.0 }))
                    })
                    .min_by(|x, y| x.1.total_cmp(&y.1))
                    .map(|(j, _)| j)
            })
            .collect();

        for (i, c) in continuation.iter().enumerate() {
            if let Some(j) = *c {
                let (a, b) = (&arms[i], &arms[j]);
                if i < j && continuation[j] == Some(i) && a.road != b.road {
                    links.push((
                        a.road,
                        roads[a.road].node_t[a.node],
                        b.road,
                        roads[b.road].node_t[b.node],
                    ));
                }
            }
        }

        // Signals on one arm, or on both arms of one road running through, belong to
        // pedestrian crossings; a driveway joining does not make a junction either.
        let crossing_signals = match signal_arms[..] {
            [_] => true,
            [i, j] => continuation[i] == Some(j) && continuation[j] == Some(i),
            _ => false,
        };
        let signalised = junctions.contains(&junction.id)
            && (is_signal(junction) || (!signal_arms.is_empty() && !crossing_signals && !signed));

        // A road running straight through has priority over one of its class ending here.
        let through = |i: usize| continuation[i].is_some_and(|j| continuation[j] == Some(i));
        let outranks = |j: usize, i: usize| {
            let (a, b) = (&roads[arms[i].road], &roads[arms[j].road]);
            b.rank > a.rank || (b.rank == a.rank && through(j) && !through(i))
        };

        let signed_at = |k: usize| matches!(controls[k], Some(Control::Stop | Control::GiveWay));

        // Arms whose carriageway this arm's paint has to keep off.
        let suppressors: Vec<u16> = (0..arms.len())
            .map(|i| {
                let a = &roads[arms[i].road];
                let mut mask = 0u16;
                for (j, b_arm) in arms.iter().enumerate() {
                    if b_arm.road == arms[i].road || continuation[i] == Some(j) {
                        continue;
                    }
                    let b = &roads[b_arm.road];
                    let cuts = if a.ring {
                        b.ring
                    } else if b.ring {
                        true
                    } else if (b.link && !a.link) || (b.rank == 0 && a.rank > 0) {
                        // Slip roads and driveways never break a road's line.
                        false
                    } else if signed_at(i) {
                        // A mapped sign makes its approach yield to every street.
                        true
                    } else if signed_at(j) && through(i) {
                        // The road running through keeps its line past a signed approach.
                        false
                    } else {
                        signalised
                            || b.rank > a.rank
                            || (b.rank == a.rank && (through(j) || !through(i)))
                    };
                    if cuts {
                        mask |= 1 << j;
                    }
                }
                mask
            })
            .collect();

        let line_for = |i: usize| -> Option<CrossMark> {
            let arm = &arms[i];
            let a = &roads[arm.road];
            let incoming = a.oneway == 0 || a.oneway == -arm.dir;
            if !incoming || (a.rank == 0 && !signalised) {
                return None;
            }
            match controls[i] {
                Some(Control::Signal | Control::Stop) => return Some(CrossMark::Stop),
                Some(Control::GiveWay) => return Some(CrossMark::GiveWay),
                None => {}
            }
            if signalised {
                return Some(CrossMark::Stop);
            }
            let mask = suppressors[i];
            let yields_to = |pred: &dyn Fn(usize) -> bool| {
                (0..arms.len()).any(|j| mask & (1 << j) != 0 && pred(j))
            };
            if !a.ring && yields_to(&|j| roads[arms[j].road].ring) {
                return Some(CrossMark::GiveWay);
            }
            if a.marked && yields_to(&|j| outranks(j, i)) {
                return Some(if self.yellow_centre {
                    CrossMark::Stop
                } else {
                    CrossMark::GiveWay
                });
            }
            None
        };

        let needs: Vec<Option<CrossMark>> = (0..arms.len()).map(line_for).collect();
        let work: Vec<usize> = (0..arms.len())
            .filter(|&i| {
                suppressors[i] != 0
                    && (arms[i].cells.iter().any(|c| roads[c.road].marked) || needs[i].is_some())
            })
            .collect();
        if work.is_empty() {
            return signalised;
        }

        let grid_mask = work.iter().fold(0u16, |m, &i| m | suppressors[i]);
        let center = &arms[0].cells[0];
        let mut grid = LocalGrid::new(center.x, center.z, reach as i32 + max_half + MARGIN);
        for (j, b) in arms.iter().enumerate() {
            if grid_mask & (1 << j) == 0 {
                continue;
            }
            for c in &b.cells {
                grid.stamp(c.x, c.z, roads[c.road].half + MARGIN, 1 << j);
            }
        }

        for i in work {
            let arm = &arms[i];
            let mask = suppressors[i];
            let mut cut = None;
            for s in 0..arm.cells.len().min(cap + 1) {
                let (ux, uz) = local_dir(&arm.cells, s, arm.unit);
                let (px, pz) = (-uz, ux);
                let c = &arm.cells[s];
                let w = (roads[c.road].half as f32 * (px.abs() + pz.abs())).round() as i32;
                let blocked = (-w..=w).any(|p| {
                    let x = (c.x as f32 + px * p as f32).round() as i32;
                    let z = (c.z as f32 + pz * p as f32).round() as i32;
                    grid.get(x, z) & mask != 0
                });
                if !blocked {
                    cut = Some(s);
                    break;
                }
            }

            let mut mark_at = match cut {
                Some(0) => continue,
                other => other,
            };
            // Traffic stops short of the crossings at the junction, not on the far side, and no
            // line is painted over a crossing.
            if let (Some(kind), Some(s)) = (needs[i], mark_at) {
                let setback = if kind == CrossMark::Stop {
                    s + 4 * roads[arm.road].half.max(2) as usize
                } else {
                    s
                };
                let mut behind: Option<usize> = None;
                for (n, d) in arm_nodes(roads, arm, setback + 16) {
                    let Some(&reach) = crossing_reach.get(&n.id) else {
                        continue;
                    };
                    let reach = reach as usize;
                    match behind {
                        None if d + reach >= s && d <= setback + reach => {
                            behind = Some(d + reach + 1)
                        }
                        // A crossing right behind the first one goes before the line too.
                        Some(b) if d <= b + reach + 1 => behind = Some(b.max(d + reach + 1)),
                        Some(_) => break,
                        None => {}
                    }
                }
                if let Some(b) = behind.filter(|&b| b < arm.cells.len()) {
                    mark_at = Some(b);
                }
            }

            // Between two nodes of one junction the stretch left over is too short to paint:
            // it belongs to the junction, lines and all.
            let mut end = mark_at.unwrap_or(arm.cells.len().min(cap + 1));
            if let Some(s) = mark_at {
                let next = arm
                    .cells
                    .iter()
                    .enumerate()
                    .take(2 * s + min_run + 1)
                    .skip(s)
                    .find(|(_, c)| node_at(roads, c).is_some_and(|n| junctions.contains(&n.id)));
                if let Some((k, _)) = next {
                    mark_at = None;
                    end = k + 1;
                }
            }

            // No lane lines from the junction up to the mark, per way the arm runs over.
            let end = end.min(arm.cells.len());
            self.clear_cells(roads, &arm.cells[..end]);

            if let (Some(kind), Some(s)) = (needs[i], mark_at) {
                let c = &arm.cells[s];
                let road = &roads[c.road];
                // Approaching traffic keeps to its own side of the centreline.
                let side = if road.oneway != 0 {
                    0
                } else if self.drives_on_left {
                    c.dir
                } else {
                    -c.dir
                };
                self.ways
                    .entry(road.way.id)
                    .or_default()
                    .marks
                    .push(TransverseMark { t: c.t, kind, side });
            }
        }
        signalised
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
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    fn plain(id: u64, x: i32, z: i32) -> ProcessedNode {
        node(id, x, z, &[])
    }

    fn no_gaps(index: &RoadMarkingIndex, way: u64) -> bool {
        index
            .way(way)
            .is_none_or(|m| m.gaps.is_empty() && m.marks.is_empty())
    }

    fn build(elements: &[ProcessedElement]) -> RoadMarkingIndex {
        RoadMarkingIndex::build(elements, 1.0, SignRegion::Europe)
    }

    /// Two equal roads crossing, each split at the junction node like OSM maps them.
    fn crossroads(a: &str, b: &str) -> Vec<ProcessedElement> {
        vec![
            way(
                1,
                vec![plain(10, 0, 100), plain(1, 100, 100)],
                &[("highway", a)],
            ),
            way(
                2,
                vec![plain(1, 100, 100), plain(11, 200, 100)],
                &[("highway", a)],
            ),
            way(
                3,
                vec![plain(12, 100, 0), plain(1, 100, 100)],
                &[("highway", b)],
            ),
            way(
                4,
                vec![plain(1, 100, 100), plain(13, 100, 200)],
                &[("highway", b)],
            ),
        ]
    }

    #[test]
    fn equal_roads_keep_their_lines_out_of_each_other() {
        let index = build(&crossroads("secondary", "secondary"));
        // Way 2 starts at the junction; its line stays clear of the crossing road's width.
        let half = highway_block_range("secondary", &HashMap::new(), 1.0) as u32;
        let marks = index.way(2).expect("way 2 is cut");
        assert!(marks.in_gap(0) && marks.in_gap(half + MARGIN as u32));
        assert!(!marks.in_gap(half + MARGIN as u32 + 2));
        // Way 1 ends there, so its gap sits at the far end of its path.
        let marks = index.way(1).expect("way 1 is cut");
        assert!(marks.in_gap(100) && !marks.in_gap(100 - half - 3));
        // Neither has priority and nothing is signalised: no transverse line.
        assert!(marks.marks.is_empty());
    }

    #[test]
    fn a_major_road_runs_on_past_a_minor_one() {
        let index = build(&crossroads("primary", "tertiary"));
        assert!(no_gaps(&index, 1) && no_gaps(&index, 2));
        let marks = index.way(4).expect("the tertiary is cut");
        assert!(marks.in_gap(1));
        // The tertiary leaving southwards has its approach on the west half (travel north,
        // right-hand traffic keeps to the east, which is -perp of a southbound way).
        let line = marks.marks.first().expect("the tertiary gives way");
        assert_eq!(line.kind, CrossMark::GiveWay);
        assert_eq!(line.side, -1);
        assert!(!marks.in_gap(line.t + 1));
    }

    #[test]
    fn a_through_road_keeps_its_line_past_an_equal_side_road() {
        let elements = vec![
            way(
                1,
                vec![plain(10, 0, 100), plain(1, 100, 100)],
                &[("highway", "secondary")],
            ),
            way(
                2,
                vec![plain(1, 100, 100), plain(11, 200, 100)],
                &[("highway", "secondary")],
            ),
            way(
                3,
                vec![plain(12, 100, 0), plain(1, 100, 100)],
                &[("highway", "secondary")],
            ),
        ];
        let index = build(&elements);
        assert!(no_gaps(&index, 1) && no_gaps(&index, 2));
        let side = index.way(3).expect("the side road is cut");
        assert!(side.in_gap(100));
        assert_eq!(side.marks[0].kind, CrossMark::GiveWay);
    }

    #[test]
    fn a_cut_carries_on_past_a_way_split_near_the_junction() {
        // The tertiary is split two cells short of the primary, inside the primary's width.
        let elements = vec![
            way(
                1,
                vec![plain(10, 0, 100), plain(1, 100, 100)],
                &[("highway", "primary")],
            ),
            way(
                2,
                vec![plain(1, 100, 100), plain(11, 200, 100)],
                &[("highway", "primary")],
            ),
            way(
                3,
                vec![plain(12, 100, 0), plain(13, 100, 98)],
                &[("highway", "tertiary")],
            ),
            way(
                4,
                vec![plain(13, 100, 98), plain(1, 100, 100)],
                &[("highway", "tertiary")],
            ),
        ];
        let index = build(&elements);
        let half = highway_block_range("primary", &HashMap::new(), 1.0) as u32;
        let far = index.way(3).expect("the upstream way is cut too");
        assert!(far.in_gap(98) && far.in_gap(100 - half - 1));
        assert!(far.marks.iter().any(|m| m.kind == CrossMark::GiveWay));
        assert!(index.way(4).expect("short way cut").in_gap(0));
    }

    #[test]
    fn signals_clear_the_box_for_every_road() {
        let mut elements = crossroads("primary", "tertiary");
        if let ProcessedElement::Way(w) = &mut elements[0] {
            w.nodes[1]
                .tags
                .insert("highway".into(), "traffic_signals".into());
        }
        let index = build(&elements);
        let marks = index.way(2).expect("the primary is cut too");
        assert!(marks.in_gap(0));
        assert_eq!(marks.marks[0].kind, CrossMark::Stop);
    }

    #[test]
    fn a_road_split_at_a_driveway_keeps_its_line() {
        let elements = vec![
            way(
                1,
                vec![plain(10, 0, 100), plain(1, 100, 100)],
                &[("highway", "primary")],
            ),
            way(
                2,
                vec![plain(1, 100, 100), plain(11, 200, 100)],
                &[("highway", "primary")],
            ),
            way(
                3,
                vec![plain(12, 100, 50), plain(1, 100, 100)],
                &[("highway", "residential")],
            ),
        ];
        let index = build(&elements);
        assert!(no_gaps(&index, 1) && no_gaps(&index, 2));
    }

    #[test]
    fn roundabout_approach_yields_and_the_ring_runs_on() {
        let ring: Vec<ProcessedNode> = (0..=12)
            .map(|i| {
                let a = std::f32::consts::TAU * (i % 12) as f32 / 12.0;
                plain(
                    100 + (i % 12) as u64,
                    50 + (30.0 * a.cos()).round() as i32,
                    50 + (30.0 * a.sin()).round() as i32,
                )
            })
            .collect();
        let elements = vec![
            way(
                1,
                ring,
                &[
                    ("highway", "primary"),
                    ("junction", "roundabout"),
                    ("lanes", "2"),
                ],
            ),
            way(
                2,
                vec![plain(100, 80, 50), plain(9, 150, 50)],
                &[("highway", "secondary")],
            ),
        ];
        let index = build(&elements);
        assert!(index.way(1).is_none_or(|m| m.gaps.is_empty()));
        let marks = index.way(2).expect("the approach is cut");
        assert!(marks.in_gap(0));
        let line = marks.marks.first().expect("entry line");
        assert_eq!(line.kind, CrossMark::GiveWay);
    }

    #[test]
    fn roundabouts_default_to_one_unmarked_lane() {
        let tags: HashMap<String, String> = [("junction", "roundabout")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(lane_marking_count("primary", &tags), 1);
        assert_eq!(lane_marking_count("primary", &HashMap::new()), 2);
    }

    #[test]
    fn a_crossing_node_paints_across_the_road() {
        let elements = vec![way(
            1,
            vec![
                plain(10, 0, 50),
                node(2, 40, 50, &[("highway", "crossing"), ("crossing", "zebra")]),
                plain(11, 100, 50),
            ],
            &[("highway", "secondary")],
        )];
        let index = build(&elements);
        let marks = index.way(1).expect("crossing painted");
        let rows: Vec<(u32, CrossMark)> = marks.marks.iter().map(|m| (m.t, m.kind)).collect();
        assert_eq!(
            rows,
            vec![
                (39, CrossMark::Zebra),
                (40, CrossMark::Zebra),
                (41, CrossMark::Zebra)
            ]
        );
        assert!(marks.in_gap(38) && marks.in_gap(42) && !marks.in_gap(43));
    }

    #[test]
    fn a_crossing_at_a_way_split_paints_both_ways() {
        // The crossing is one cell before the first way ends; its far edge line is on the next.
        let lines = [
            ("highway", "crossing"),
            ("crossing", "marked"),
            ("crossing:markings", "lines"),
        ];
        let elements = vec![
            way(
                1,
                vec![plain(10, 0, 50), node(2, 39, 50, &lines), plain(3, 40, 50)],
                &[("highway", "secondary")],
            ),
            way(
                2,
                vec![plain(3, 40, 50), plain(11, 100, 50)],
                &[("highway", "secondary")],
            ),
        ];
        let index = build(&elements);
        let near = index.way(1).expect("first way");
        assert_eq!(near.marks.len(), 1);
        assert_eq!(near.marks[0].t, 37);
        let far = index.way(2).expect("second way");
        assert_eq!(far.marks.len(), 1);
        assert_eq!(far.marks[0].t, 1);
        assert!(far.in_gap(2) && !far.in_gap(3));
    }

    #[test]
    fn crossing_signals_alone_mark_a_signalised_crossing() {
        let elements = vec![way(
            1,
            vec![
                plain(10, 0, 50),
                node(
                    2,
                    60,
                    50,
                    &[("highway", "crossing"), ("crossing:signals", "yes")],
                ),
                plain(11, 140, 50),
            ],
            &[("highway", "secondary")],
        )];
        let index = build(&elements);
        let marks = &index.way(1).expect("painted").marks;
        assert!(marks.iter().any(|m| m.kind == CrossMark::Zebra));
        assert_eq!(
            marks.iter().filter(|m| m.kind == CrossMark::Stop).count(),
            2
        );
    }

    #[test]
    fn a_wide_crossing_keeps_the_lane_lines_further_off() {
        let elements = vec![
            way(
                1,
                vec![plain(10, 0, 50), plain(2, 40, 50), plain(11, 100, 50)],
                &[("highway", "secondary")],
            ),
            way(
                5,
                vec![plain(20, 40, 35), plain(2, 40, 50), plain(21, 40, 65)],
                &[
                    ("highway", "footway"),
                    ("footway", "crossing"),
                    ("crossing", "zebra"),
                    ("width", "8"),
                ],
            ),
        ];
        let index = build(&elements);
        // Half-width 4 plus a clear row either side of the node.
        let marks = index.way(1).expect("gap");
        assert!(marks.in_gap(35) && marks.in_gap(45) && !marks.in_gap(46));
    }

    #[test]
    fn an_oblique_crossing_is_clipped_at_the_kerb_not_short_of_it() {
        // About 20 degrees off the road: the crossing meets the kerb far along it.
        let elements = vec![
            way(
                1,
                vec![plain(10, 0, 50), plain(2, 60, 50), plain(11, 120, 50)],
                &[("highway", "secondary")],
            ),
            way(
                5,
                vec![plain(20, 33, 40), plain(2, 60, 50), plain(21, 87, 60)],
                &[
                    ("highway", "footway"),
                    ("footway", "crossing"),
                    ("crossing", "zebra"),
                ],
            ),
        ];
        let index = build(&elements);
        let clips = index.crossing_clips(5).expect("clipped");
        let half = highway_block_range("secondary", &HashMap::new(), 1.0);
        // The crossing centreline where it reaches the kerb.
        let x = 60 + (half as f32 * 2.7).round() as i32;
        assert!(clips.iter().any(|c| c.contains(x, 50 + half)));
        assert!(!clips.iter().any(|c| c.contains(x, 50 + half + 1)));
    }

    #[test]
    fn an_unsplit_road_runs_through_like_a_split_one() {
        let elements = vec![
            way(
                1,
                vec![plain(10, 0, 100), plain(1, 100, 100), plain(11, 200, 100)],
                &[("highway", "secondary")],
            ),
            way(
                3,
                vec![plain(12, 100, 0), plain(1, 100, 100)],
                &[("highway", "secondary")],
            ),
        ];
        let index = build(&elements);
        assert!(no_gaps(&index, 1));
        let side = index.way(3).expect("the side road is cut");
        assert_eq!(side.marks[0].kind, CrossMark::GiveWay);
    }

    #[test]
    fn a_mapped_stop_sign_overrides_road_class() {
        // A primary ends in a stop sign at a tertiary running through.
        let elements = vec![
            way(
                1,
                vec![
                    plain(10, 100, 0),
                    node(5, 100, 90, &[("highway", "stop")]),
                    plain(1, 100, 100),
                ],
                &[("highway", "primary")],
            ),
            way(
                2,
                vec![plain(11, 0, 100), plain(1, 100, 100)],
                &[("highway", "tertiary")],
            ),
            way(
                3,
                vec![plain(1, 100, 100), plain(12, 200, 100)],
                &[("highway", "tertiary")],
            ),
        ];
        let index = build(&elements);
        let primary = index.way(1).expect("the primary stops");
        assert_eq!(primary.marks[0].kind, CrossMark::Stop);
        assert!(primary.in_gap(100));
        assert!(no_gaps(&index, 2) && no_gaps(&index, 3));
    }

    #[test]
    fn a_signalised_crossing_gets_a_stop_line_on_each_approach() {
        let elements = vec![way(
            1,
            vec![
                plain(10, 0, 50),
                node(
                    2,
                    60,
                    50,
                    &[("highway", "crossing"), ("crossing", "traffic_signals")],
                ),
                plain(11, 140, 50),
            ],
            &[("highway", "secondary")],
        )];
        let index = build(&elements);
        let stops: Vec<(u32, i8)> = index
            .way(1)
            .expect("painted")
            .marks
            .iter()
            .filter(|m| m.kind == CrossMark::Stop)
            .map(|m| (m.t, m.side))
            .collect();
        // Zebra reach 2, a free row, then the line on the half traffic arrives in.
        assert_eq!(stops, vec![(57, 1), (63, -1)]);
    }

    #[test]
    fn a_pelican_beside_a_side_street_still_gets_its_stop_lines() {
        let elements = vec![
            way(
                1,
                vec![
                    plain(10, 0, 50),
                    plain(3, 40, 50),
                    node(
                        2,
                        60,
                        50,
                        &[("highway", "crossing"), ("crossing", "traffic_signals")],
                    ),
                    plain(11, 140, 50),
                ],
                &[("highway", "primary")],
            ),
            way(
                2,
                vec![plain(12, 40, 0), plain(3, 40, 50)],
                &[("highway", "residential")],
            ),
        ];
        let index = build(&elements);
        let stops = index
            .way(1)
            .expect("painted")
            .marks
            .iter()
            .filter(|m| m.kind == CrossMark::Stop)
            .count();
        assert_eq!(stops, 2);
    }

    #[test]
    fn traffic_leaving_signals_gets_no_stop_line_before_the_next_crossing() {
        let signal = [("highway", "traffic_signals")];
        let elements = vec![
            way(
                1,
                vec![plain(10, 0, 50), node(1, 50, 50, &signal)],
                &[("highway", "secondary")],
            ),
            way(
                2,
                vec![
                    node(1, 50, 50, &signal),
                    node(
                        2,
                        65,
                        50,
                        &[("highway", "crossing"), ("crossing", "traffic_signals")],
                    ),
                    plain(11, 150, 50),
                ],
                &[("highway", "secondary")],
            ),
            way(
                3,
                vec![plain(12, 50, 0), node(1, 50, 50, &signal)],
                &[("highway", "secondary")],
            ),
        ];
        let index = build(&elements);
        let stops: Vec<i8> = index
            .way(2)
            .expect("painted")
            .marks
            .iter()
            .filter(|m| m.kind == CrossMark::Stop && m.t > 15)
            .map(|m| m.side)
            .collect();
        // Only the line for traffic heading back towards the junction.
        assert_eq!(stops, vec![-1]);
    }

    #[test]
    fn a_give_way_line_moves_off_a_zebra_at_the_entry() {
        let ring: Vec<ProcessedNode> = (0..=12)
            .map(|i| {
                let a = std::f32::consts::TAU * (i % 12) as f32 / 12.0;
                plain(
                    100 + (i % 12) as u64,
                    50 + (12.0 * a.cos()).round() as i32,
                    50 + (12.0 * a.sin()).round() as i32,
                )
            })
            .collect();
        let zebra = [("highway", "crossing"), ("crossing", "zebra")];
        let elements = vec![
            way(
                1,
                ring,
                &[("highway", "tertiary"), ("junction", "roundabout")],
            ),
            way(
                2,
                vec![
                    plain(100, 62, 50),
                    node(5, 68, 50, &zebra),
                    plain(9, 150, 50),
                ],
                &[("highway", "tertiary")],
            ),
        ];
        let index = build(&elements);
        let marks = &index.way(2).expect("painted").marks;
        let zebra_t = marks
            .iter()
            .find(|m| m.kind == CrossMark::Zebra)
            .expect("zebra")
            .t;
        let line = marks
            .iter()
            .find(|m| m.kind == CrossMark::GiveWay)
            .expect("line");
        assert!(
            line.t > zebra_t + 2,
            "give-way at {} on the zebra at {zebra_t}",
            line.t
        );
    }

    #[test]
    fn local_tagging_decides_how_signal_crossings_are_painted() {
        let crossing = |id: u64, markings: &str| {
            ProcessedElement::Node(node(
                id,
                0,
                0,
                &[
                    ("highway", "crossing"),
                    ("crossing", "traffic_signals"),
                    ("crossing:markings", markings),
                ],
            ))
        };
        let untagged: HashMap<String, String> = [
            ("highway", "footway"),
            ("footway", "crossing"),
            ("crossing", "traffic_signals"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let vienna = [
            crossing(1, "zebra"),
            crossing(2, "zebra"),
            crossing(3, "dashes"),
        ];
        let index = RoadMarkingIndex::build(&vienna, 1.0, SignRegion::Germanic);
        assert_eq!(
            index.crossing_way_paint(&untagged),
            Some(CrossingPaint::Zebra)
        );
        let berlin = [crossing(1, "dashes"), crossing(2, "lines")];
        let index = RoadMarkingIndex::build(&berlin, 1.0, SignRegion::Europe);
        assert_eq!(
            index.crossing_way_paint(&untagged),
            Some(CrossingPaint::Lines { broken: true })
        );
    }

    #[test]
    fn the_stretch_between_two_carriageways_belongs_to_the_junction() {
        // A secondary crosses both halves of a dual carriageway 20 cells apart.
        let oneway = [("highway", "primary"), ("oneway", "yes")];
        let elements = vec![
            way(1, vec![plain(10, 200, 100), plain(1, 100, 100)], &oneway),
            way(2, vec![plain(1, 100, 100), plain(11, 0, 100)], &oneway),
            way(3, vec![plain(12, 0, 120), plain(2, 100, 120)], &oneway),
            way(4, vec![plain(2, 100, 120), plain(13, 200, 120)], &oneway),
            way(
                5,
                vec![plain(14, 100, 0), plain(1, 100, 100)],
                &[("highway", "secondary")],
            ),
            way(
                6,
                vec![plain(1, 100, 100), plain(2, 100, 120)],
                &[("highway", "secondary")],
            ),
            way(
                7,
                vec![plain(2, 100, 120), plain(15, 100, 220)],
                &[("highway", "secondary")],
            ),
        ];
        let index = build(&elements);
        let middle = index.way(6).expect("middle is cut");
        assert!((0..=20).all(|t| middle.in_gap(t)), "{:?}", middle.gaps);
        assert!(middle.marks.is_empty());
        // The outer approaches still give way before the carriageways.
        assert!(index.way(5).expect("approach").marks[0].kind == CrossMark::GiveWay);
    }

    #[test]
    fn a_driveway_between_two_pelicans_is_not_a_signalised_junction() {
        let pelican = [("highway", "crossing"), ("crossing", "traffic_signals")];
        let elements = vec![
            way(
                1,
                vec![
                    plain(10, 0, 50),
                    node(2, 40, 50, &pelican),
                    plain(3, 55, 50),
                    node(4, 70, 50, &pelican),
                    plain(11, 140, 50),
                ],
                &[("highway", "primary")],
            ),
            way(
                2,
                vec![plain(12, 55, 0), plain(3, 55, 50)],
                &[("highway", "service")],
            ),
        ];
        let index = build(&elements);
        let marks = &index.way(1).expect("painted").marks;
        assert_eq!(
            marks.iter().filter(|m| m.kind == CrossMark::Stop).count(),
            4
        );
    }

    /// The cells a renderer stamp of `half` paints around a way's centreline.
    fn footprint(nodes: &[(i32, i32)], half: i32) -> FnvHashSet<(i32, i32)> {
        let way = ProcessedWay {
            id: 0,
            nodes: nodes.iter().map(|&(x, z)| plain(0, x, z)).collect(),
            tags: HashMap::new(),
        };
        way_cells(&way)
            .iter()
            .flat_map(|&(x, z)| {
                (-half..=half).flat_map(move |dx| (-half..=half).map(move |dz| (x + dx, z + dz)))
            })
            .collect()
    }

    #[test]
    fn crossing_clips_follow_the_rendered_road_on_diagonals() {
        // A diagonal secondary with a perpendicular zebra four wide.
        let zebra = [
            ("highway", "footway"),
            ("footway", "crossing"),
            ("crossing", "zebra"),
            ("width", "4"),
        ];
        let road_nodes = [(0, 11), (50, 51), (100, 91)];
        let crossing_nodes = [(38, 66), (50, 51), (62, 36)];
        let elements = vec![
            way(
                1,
                vec![plain(10, 0, 11), plain(2, 50, 51), plain(11, 100, 91)],
                &[("highway", "secondary")],
            ),
            way(
                5,
                vec![plain(20, 38, 66), plain(2, 50, 51), plain(21, 62, 36)],
                &zebra,
            ),
        ];
        let index = build(&elements);
        let clips = index.crossing_clips(5).expect("clipped");
        let half = highway_block_range("secondary", &HashMap::new(), 1.0);
        let road = footprint(&road_nodes, half);
        for cell in footprint(&crossing_nodes, 2) {
            let on_road = road.contains(&cell);
            assert_eq!(
                clips.iter().any(|c| c.contains(cell.0, cell.1)),
                on_road,
                "{cell:?}"
            );
        }
    }

    #[test]
    fn a_wide_oblique_crossing_is_clipped_where_both_footprints_meet() {
        let elements = vec![
            way(
                1,
                vec![plain(10, -60, 0), plain(2, 0, 0), plain(11, 60, 0)],
                &[("highway", "secondary")],
            ),
            way(
                5,
                vec![plain(20, -20, -20), plain(2, 0, 0), plain(21, 20, 20)],
                &[
                    ("highway", "footway"),
                    ("footway", "crossing"),
                    ("crossing", "zebra"),
                    ("width", "8"),
                ],
            ),
        ];
        let index = build(&elements);
        let clips = index.crossing_clips(5).expect("clipped");
        assert!(clips.iter().any(|c| c.contains(12, 4)));
        assert!(!clips.iter().any(|c| c.contains(12, 5)));
        // The lane line keeps a clear row past the last painted cell.
        assert!(index.way(1).expect("gap").in_gap(13 + 60));
    }

    #[test]
    fn a_signal_head_beside_a_crossing_past_a_way_split_is_not_a_crossing() {
        let head = [
            ("highway", "traffic_signals"),
            ("crossing", "traffic_signals"),
        ];
        let zebra = [("highway", "crossing"), ("crossing", "zebra")];
        let elements = vec![
            way(
                1,
                vec![plain(10, 0, 50), node(2, 47, 50, &head), plain(3, 50, 50)],
                &[("highway", "secondary")],
            ),
            way(
                2,
                vec![
                    plain(3, 50, 50),
                    node(4, 53, 50, &zebra),
                    plain(11, 100, 50),
                ],
                &[("highway", "secondary")],
            ),
        ];
        let index = build(&elements);
        let zebra_rows = |id| {
            index.way(id).map_or(0, |m| {
                m.marks
                    .iter()
                    .filter(|m| m.kind == CrossMark::Zebra)
                    .count()
            })
        };
        // Only the mapped crossing: three rows straddling the split at most.
        assert_eq!(zebra_rows(1) + zebra_rows(2), 3);
    }

    #[test]
    fn stop_lines_stay_off_a_neighbouring_crossing() {
        let pelican = [("highway", "crossing"), ("crossing", "traffic_signals")];
        let elements = vec![way(
            1,
            vec![
                plain(10, 0, 50),
                node(2, 40, 50, &pelican),
                node(3, 44, 50, &pelican),
                plain(11, 100, 50),
            ],
            &[("highway", "secondary")],
        )];
        let index = build(&elements);
        let marks = &index.way(1).expect("painted").marks;
        let mut stops: Vec<u32> = marks
            .iter()
            .filter(|m| m.kind == CrossMark::Stop)
            .map(|m| m.t)
            .collect();
        stops.dedup();
        // One line before the pair on each side, none between them.
        assert_eq!(stops, vec![37, 47]);
    }

    #[test]
    fn a_stop_sign_past_a_way_split_still_paints_its_line() {
        // Unmarked residential T; the approach is split two cells short of the junction.
        let elements = vec![
            way(
                1,
                vec![plain(10, 0, 100), plain(1, 100, 100), plain(11, 200, 100)],
                &[("highway", "residential")],
            ),
            way(
                2,
                vec![
                    plain(12, 100, 0),
                    node(5, 100, 90, &[("highway", "stop")]),
                    plain(13, 100, 98),
                ],
                &[("highway", "residential")],
            ),
            way(
                3,
                vec![plain(13, 100, 98), plain(1, 100, 100)],
                &[("highway", "residential")],
            ),
        ];
        let index = build(&elements);
        let stops = [2, 3]
            .iter()
            .filter_map(|&id| index.way(id))
            .flat_map(|m| m.marks.iter())
            .filter(|m| m.kind == CrossMark::Stop)
            .count();
        assert_eq!(stops, 1);
    }

    #[test]
    fn unmarked_crossing_nodes_paint_nothing() {
        let elements = vec![way(
            1,
            vec![
                plain(10, 0, 50),
                node(
                    2,
                    40,
                    50,
                    &[("highway", "crossing"), ("crossing", "unmarked")],
                ),
                plain(11, 100, 50),
            ],
            &[("highway", "secondary")],
        )];
        assert!(build(&elements).way(1).is_none());
    }

    #[test]
    fn a_crossing_way_is_clipped_to_the_carriageway() {
        let elements = vec![
            way(
                1,
                vec![plain(10, 0, 50), plain(2, 40, 50), plain(11, 100, 50)],
                &[("highway", "secondary")],
            ),
            way(
                5,
                vec![plain(20, 40, 35), plain(2, 40, 50), plain(21, 40, 65)],
                &[
                    ("highway", "footway"),
                    ("footway", "crossing"),
                    ("crossing", "zebra"),
                ],
            ),
        ];
        let index = build(&elements);
        let clips = index.crossing_clips(5).expect("clipped");
        let half = highway_block_range("secondary", &HashMap::new(), 1.0);
        assert!(clips.iter().any(|c| c.contains(40, 50 + half)));
        assert!(!clips.iter().any(|c| c.contains(40, 50 + half + 1)));
        // The road's lane line steps around the zebra.
        assert!(index.way(1).expect("gap").in_gap(40));
    }

    #[test]
    fn signalised_crossings_paint_lines_in_germany_and_zebras_elsewhere() {
        let tags: HashMap<String, String> = [
            ("highway", "footway"),
            ("footway", "crossing"),
            ("crossing", "traffic_signals"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let de = RoadMarkingIndex::build(&[], 1.0, SignRegion::Germanic);
        let fr = RoadMarkingIndex::build(&[], 1.0, SignRegion::Europe);
        assert_eq!(
            de.crossing_way_paint(&tags),
            Some(CrossingPaint::Lines { broken: true })
        );
        assert_eq!(fr.crossing_way_paint(&tags), Some(CrossingPaint::Zebra));
    }
}
