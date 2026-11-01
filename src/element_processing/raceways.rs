//! Race tracks: widths by the sport raced, and the paint that marks out a paved circuit:
//! white edge lines, red and white kerbs through the corners and a chequered start line.

use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::highways::highway_block_range;
use crate::element_processing::stations::SparseCells;
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use fnv::FnvHashMap;
use std::collections::HashMap;

/// Curvature is read over this much track either side of a cell, in metres.
const CURVE_WINDOW_M: f32 = 16.0;
/// Corners tighter than this get a kerb on the inside.
const INNER_KERB_RADIUS_M: f32 = 90.0;
/// Corners tighter than this get one on the outside as well.
const OUTER_KERB_RADIUS_M: f32 = 35.0;
/// Shortest kerb worth laying, in metres.
const MIN_KERB_M: f32 = 6.0;
/// Length of one red or white kerb stripe, in blocks.
const STRIPE: f32 = 2.0;
/// Kerbs are two blocks deep on tracks at least this half-width.
const WIDE_KERB_HALF: i32 = 5;
const SURFACED_BLOCKS: &[Block] = &[COARSE_DIRT, DIRT];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Discipline {
    Circuit,
    Kart,
    Dirt,
    Model,
    Unknown,
}

fn discipline(tags: &HashMap<String, String>) -> Discipline {
    let Some(sport) = tags.get("sport") else {
        return Discipline::Unknown;
    };
    for s in sport.split(';').map(str::trim) {
        match s {
            "motor" | "motorsport" | "motor_racing" | "motorcycle_racing" | "auto_racing"
            | "drag_racing" => return Discipline::Circuit,
            "karting" | "kart" | "go_kart" | "go-kart" => return Discipline::Kart,
            "motocross" | "enduro" | "supercross" | "speedway" | "bmx" | "dirt_bike" => {
                return Discipline::Dirt
            }
            "rc_car" | "model" | "model_car" | "modelrace" | "rc" => return Discipline::Model,
            _ => {}
        }
    }
    Discipline::Unknown
}

/// Half-width in blocks of a raceway with no `width`: a circuit is about 12 m across, a kart
/// track 8 m and a motocross track 6 m.
pub(crate) fn default_half_width(tags: &HashMap<String, String>) -> i32 {
    let metres: f64 = match discipline(tags) {
        Discipline::Circuit => 12.0,
        Discipline::Kart => 8.0,
        Discipline::Dirt => 6.0,
        Discipline::Model => 2.0,
        Discipline::Unknown => 10.0,
    };
    (metres / 2.0).round() as i32
}

/// Loose ground for an untagged dirt track; None leaves the road default.
pub(crate) fn unsurfaced_palette(tags: &HashMap<String, String>) -> Option<&'static [Block]> {
    (discipline(tags) == Discipline::Dirt).then_some(SURFACED_BLOCKS)
}

/// Ground of a `leisure=track` area raced by motor sport, without a `surface` tag. A closed
/// way without `area=yes` is the track's line, so paving it would pave its infield.
pub(crate) fn track_area_block(tags: &HashMap<String, String>) -> Option<Block> {
    let area = tags.get("area").map(String::as_str) == Some("yes")
        || tags.get("type").map(String::as_str) == Some("multipolygon");
    if !area {
        return None;
    }
    match discipline(tags) {
        Discipline::Circuit | Discipline::Kart | Discipline::Model => Some(GRAY_CONCRETE_POWDER),
        Discipline::Dirt => Some(COARSE_DIRT),
        Discipline::Unknown => None,
    }
}

fn is_paved(tags: &HashMap<String, String>) -> bool {
    match tags.get("surface").map(String::as_str) {
        None => discipline(tags) != Discipline::Dirt,
        Some(s) => matches!(
            s,
            "asphalt"
                | "paved"
                | "concrete"
                | "concrete:plates"
                | "concrete:lanes"
                | "cement"
                | "chipseal"
                | "bitmac"
        ),
    }
}

fn is_pit_lane(tags: &HashMap<String, String>) -> bool {
    matches!(
        tags.get("raceway").map(String::as_str),
        Some("pitlane" | "pit_lane" | "pit" | "pits")
    )
}

fn is_start_line(tags: &HashMap<String, String>) -> bool {
    matches!(
        tags.get("raceway").map(String::as_str),
        Some("start-finish" | "start" | "finish" | "startline" | "start/finish" | "finish_line")
    )
}

/// Deck a raceway is drawn on: 0 at grade, else its layer, at least 1 on a bridge.
pub(crate) fn deck(tags: &HashMap<String, String>) -> i32 {
    let layer = tags
        .get("layer")
        .and_then(|l| l.parse::<i32>().ok())
        .unwrap_or(0)
        .max(0);
    if matches!(tags.get("bridge").map(String::as_str), Some(v) if v != "no") {
        layer.max(1)
    } else {
        layer
    }
}

/// Raceways the highway renderer lays as a line on the surface.
fn at_surface(way: &ProcessedWay) -> bool {
    let tags = &way.tags;
    tags.get("highway").map(String::as_str) == Some("raceway")
        && way.nodes.len() >= 2
        && tags.get("area").map(String::as_str) != Some("yes")
        && tags.get("indoor").map(String::as_str) != Some("yes")
        && !matches!(tags.get("tunnel").map(String::as_str), Some(v) if v != "no")
        && tags
            .get("level")
            .and_then(|l| l.parse::<i32>().ok())
            .is_none_or(|l| l >= 0)
}

struct Track<'a> {
    way: &'a ProcessedWay,
    half: i32,
    deck: i32,
    kerbs: bool,
}

/// A centreline cell of a chain of joined ways.
struct Cell {
    x: i32,
    z: i32,
    /// Distance along the chain, in blocks.
    s: f32,
    /// Unit direction of travel.
    t: (f32, f32),
    /// Side of the inner kerb (sign of the turn), 0 for none.
    kerb: i8,
    /// A tight corner, kerbed on both sides.
    both: bool,
    half: i32,
    deck: i32,
}

struct Chain {
    cells: Vec<Cell>,
    length: f32,
    closed: bool,
}

impl Chain {
    /// Distance from `a` to `b` along the chain, the short way round on a loop.
    fn gap(&self, a: f32, b: f32) -> f32 {
        let d = (a - b).abs();
        if self.closed {
            d.min(self.length - d)
        } else {
            d
        }
    }
}

/// Polyline sampled by distance along it.
struct Line {
    pts: Vec<(f32, f32)>,
    cum: Vec<f32>,
    closed: bool,
}

impl Line {
    fn new(nodes: &[&ProcessedNode], closed: bool) -> Self {
        let pts: Vec<(f32, f32)> = nodes.iter().map(|n| (n.x as f32, n.z as f32)).collect();
        let mut cum = vec![0.0];
        for w in pts.windows(2) {
            let last = *cum.last().unwrap();
            cum.push(last + (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1));
        }
        Self { pts, cum, closed }
    }

    fn length(&self) -> f32 {
        *self.cum.last().unwrap()
    }

    fn at(&self, s: f32) -> (f32, f32) {
        let len = self.length();
        let s = if self.closed && len > 0.0 {
            s.rem_euclid(len)
        } else {
            s.clamp(0.0, len)
        };
        let i = self
            .cum
            .partition_point(|&c| c <= s)
            .clamp(1, self.pts.len() - 1);
        let (a, b) = (self.pts[i - 1], self.pts[i]);
        let seg = self.cum[i] - self.cum[i - 1];
        let f = if seg > 0.0 {
            (s - self.cum[i - 1]) / seg
        } else {
            0.0
        };
        (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f)
    }
}

fn unit(v: (f32, f32)) -> Option<(f32, f32)> {
    let len = v.0.hypot(v.1);
    (len > 1e-6).then(|| (v.0 / len, v.1 / len))
}

fn cross(a: (f32, f32), b: (f32, f32)) -> f32 {
    a.0 * b.1 - a.1 * b.0
}

/// Joins ways end to end where exactly two meet, as (track, reversed) runs.
fn join_tracks(tracks: &[Track]) -> Vec<Vec<(usize, bool)>> {
    let first = |i: usize| tracks[i].way.nodes[0].id;
    let last = |i: usize| tracks[i].way.nodes.last().unwrap().id;
    let mut ends: FnvHashMap<u64, Vec<usize>> = FnvHashMap::default();
    for i in 0..tracks.len() {
        if first(i) != last(i) {
            ends.entry(first(i)).or_default().push(i);
            ends.entry(last(i)).or_default().push(i);
        }
    }
    let next_at = |node: u64, visited: &[bool]| -> Option<usize> {
        let list = ends.get(&node)?;
        (list.len() == 2)
            .then(|| list.iter().copied().find(|&j| !visited[j]))
            .flatten()
    };
    let mut visited = vec![false; tracks.len()];
    let mut chains = Vec::new();
    for i in 0..tracks.len() {
        if visited[i] {
            continue;
        }
        visited[i] = true;
        let mut run = std::collections::VecDeque::from([(i, false)]);
        if first(i) != last(i) {
            let mut tail = last(i);
            while let Some(j) = next_at(tail, &visited) {
                visited[j] = true;
                let rev = last(j) == tail;
                tail = if rev { first(j) } else { last(j) };
                run.push_back((j, rev));
            }
            let mut head = first(i);
            while let Some(j) = next_at(head, &visited) {
                visited[j] = true;
                let rev = first(j) == head;
                head = if rev { last(j) } else { first(j) };
                run.push_front((j, rev));
            }
        }
        chains.push(run.into_iter().collect());
    }
    chains
}

fn build_chain(tracks: &[Track], run: &[(usize, bool)], scale: f32) -> Chain {
    let mut nodes: Vec<&ProcessedNode> = Vec::new();
    let mut owner: Vec<usize> = Vec::new();
    for &(i, rev) in run {
        let way_nodes: Vec<&ProcessedNode> = if rev {
            tracks[i].way.nodes.iter().rev().collect()
        } else {
            tracks[i].way.nodes.iter().collect()
        };
        let skip = usize::from(!nodes.is_empty());
        for n in way_nodes.into_iter().skip(skip) {
            nodes.push(n);
            owner.push(i);
        }
    }
    let closed = nodes.len() > 3 && nodes[0].id == nodes.last().unwrap().id;
    let line = Line::new(&nodes, closed);

    // Same cells as the renderer's bresenham walk, each at its distance along the line.
    let mut cells = Vec::new();
    let mut kerbed = Vec::new();
    for (k, w) in nodes.windows(2).enumerate() {
        let pts = bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z);
        let n = (pts.len() - 1).max(1) as f32;
        let seg = line.cum[k + 1] - line.cum[k];
        let track = &tracks[owner[k + 1]];
        for (j, &(x, _, z)) in pts.iter().enumerate().skip(usize::from(k > 0)) {
            cells.push(Cell {
                x,
                z,
                s: line.cum[k] + seg * j as f32 / n,
                t: (1.0, 0.0),
                kerb: 0,
                both: false,
                half: track.half,
                deck: track.deck,
            });
            kerbed.push(track.kerbs);
        }
    }

    let window = (CURVE_WINDOW_M * scale).max(4.0);
    let inner = 1.0 / (INNER_KERB_RADIUS_M * scale);
    let outer = 1.0 / (OUTER_KERB_RADIUS_M * scale);
    let mut side = vec![0i8; cells.len()];
    let mut tight = vec![false; cells.len()];
    for (i, c) in cells.iter_mut().enumerate() {
        let p = line.at(c.s);
        let ahead = line.at(c.s + 2.0);
        let behind = line.at(c.s - 2.0);
        c.t = unit((ahead.0 - behind.0, ahead.1 - behind.1)).unwrap_or(c.t);
        if !kerbed[i] {
            continue;
        }
        // A short reach near an open end turns rounding to whole blocks into a bend.
        if !closed && (c.s < window * 0.75 || line.length() - c.s < window * 0.75) {
            continue;
        }
        let a = line.at(c.s - window);
        let b = line.at(c.s + window);
        let (d1, d2) = ((p.0 - a.0, p.1 - a.1), (b.0 - p.0, b.1 - p.1));
        let (l1, l2) = (d1.0.hypot(d1.1), d2.0.hypot(d2.1));
        if l1 < 1.0 || l2 < 1.0 {
            continue;
        }
        let turn = cross(d1, d2).atan2(d1.0 * d2.0 + d1.1 * d2.1);
        let curvature = turn.abs() / ((l1 + l2) / 2.0);
        if curvature > inner {
            side[i] = if turn > 0.0 { 1 } else { -1 };
            tight[i] = curvature > outer;
        }
    }
    let min_run = (MIN_KERB_M * scale).max(3.0) as usize;
    drop_short_runs(&mut side, min_run, closed);
    let mut tight_sign: Vec<i8> = tight.iter().map(|&t| i8::from(t)).collect();
    drop_short_runs(&mut tight_sign, min_run, closed);
    for (i, c) in cells.iter_mut().enumerate() {
        c.kerb = side[i];
        c.both = side[i] != 0 && tight_sign[i] != 0;
    }
    Chain {
        cells,
        length: line.length(),
        closed,
    }
}

/// Clears runs of one non-zero value shorter than `min`; a loop's runs may wrap round.
fn drop_short_runs(flags: &mut [i8], min: usize, closed: bool) {
    let n = flags.len();
    let start = if closed {
        match flags.iter().position(|&f| f == 0) {
            Some(i) => i,
            None => return,
        }
    } else {
        0
    };
    let mut k = 0;
    while k < n {
        let v = flags[(start + k) % n];
        let mut len = 1;
        while k + len < n && flags[(start + k + len) % n] == v {
            len += 1;
        }
        if v != 0 && len < min {
            for j in k..k + len {
                flags[(start + j) % n] = 0;
            }
        }
        k += len;
    }
}

/// Paint for every cell of a paved raceway that is not plain asphalt, by deck, resolved over
/// the whole area at once so every tile paints a track alike.
#[derive(Default)]
pub struct RacewayPaint {
    cells: FnvHashMap<(i32, i32, i32), Block>,
}

/// The renderer's centreline cells of a way.
fn way_cells(way: &ProcessedWay) -> Vec<(i32, i32)> {
    let mut cells: Vec<(i32, i32)> = Vec::new();
    for w in way.nodes.windows(2) {
        let pts = bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z);
        let skip = usize::from(!cells.is_empty());
        cells.extend(pts.into_iter().skip(skip).map(|(x, _, z)| (x, z)));
    }
    cells
}

impl RacewayPaint {
    pub fn at(&self, deck: i32, x: i32, z: i32) -> Option<Block> {
        if self.cells.is_empty() {
            return None;
        }
        self.cells.get(&(deck, x, z)).copied()
    }

    pub fn build(elements: &[ProcessedElement], scale: f64) -> Self {
        let mut paint = Self::default();
        let ways: Vec<(&ProcessedWay, i32, i32)> = elements
            .iter()
            .filter_map(|e| match e {
                ProcessedElement::Way(w) if at_surface(w) => Some((
                    w,
                    highway_block_range("raceway", &w.tags, scale),
                    deck(&w.tags),
                )),
                _ => None,
            })
            .collect();
        if !ways
            .iter()
            .any(|(w, half, _)| *half >= 2 && is_paved(&w.tags))
        {
            return paint;
        }
        let mut end_decks: FnvHashMap<u64, Vec<i32>> = FnvHashMap::default();
        for (w, _, d) in &ways {
            for id in [w.nodes[0].id, w.nodes.last().unwrap().id] {
                end_decks.entry(id).or_default().push(*d);
            }
        }

        let mut surface: FnvHashMap<i32, SparseCells> = FnvHashMap::default();
        let mut paved: FnvHashMap<i32, SparseCells> = FnvHashMap::default();
        let mut tracks: Vec<Track> = Vec::new();
        for &(way, half, d) in &ways {
            let painted = half >= 2 && is_paved(&way.tags);
            let cells = way_cells(way);
            for &(x, z) in &cells {
                surface.entry(d).or_default().set_square(x, z, half);
                if painted {
                    paved.entry(d).or_default().set_square(x, z, half);
                }
            }
            // Where a way runs onto another deck, its end counts there too, so the joint stays open.
            let reach = ((half + 2) as usize).min(cells.len());
            let ends = [
                (way.nodes[0].id, &cells[..reach]),
                (way.nodes.last().unwrap().id, &cells[cells.len() - reach..]),
            ];
            for (id, end) in ends {
                for &other in end_decks.get(&id).map_or(&[][..], Vec::as_slice) {
                    if other == d {
                        continue;
                    }
                    let layer = surface.entry(other).or_default();
                    for &(x, z) in end {
                        layer.set_square(x, z, half);
                    }
                }
            }
            if painted {
                tracks.push(Track {
                    way,
                    half,
                    deck: d,
                    kerbs: !is_pit_lane(&way.tags),
                });
            }
        }

        let chains: Vec<Chain> = join_tracks(&tracks)
            .iter()
            .map(|run| build_chain(&tracks, run, scale as f32))
            .collect();
        let max_half = tracks.iter().map(|t| t.half).max().unwrap_or(1);
        let mut grid: FnvHashMap<(i32, i32), Vec<(u32, u32)>> = FnvHashMap::default();
        for (ci, chain) in chains.iter().enumerate() {
            for (i, c) in chain.cells.iter().enumerate() {
                grid.entry((c.x >> 3, c.z >> 3))
                    .or_default()
                    .push((ci as u32, i as u32));
            }
        }
        let reach = max_half * 3 / 2 + 2;
        // Nearest centreline cell on `deck`, preferring those whose stamp covers (x, z): a closer
        // track that does not reach the cell must not take over its paint.
        let nearest = |x: i32, z: i32, deck: Option<i32>| -> Option<(usize, usize)> {
            let mut best: Option<(bool, i64, u32, u32)> = None;
            for gx in (x - reach) >> 3..=(x + reach) >> 3 {
                for gz in (z - reach) >> 3..=(z + reach) >> 3 {
                    for &(ci, i) in grid.get(&(gx, gz)).map_or(&[][..], Vec::as_slice) {
                        let c = &chains[ci as usize].cells[i as usize];
                        if deck.is_some_and(|d| d != c.deck) {
                            continue;
                        }
                        let outside = (c.x - x).abs().max((c.z - z).abs()) > c.half;
                        let d = ((c.x - x) as i64).pow(2) + ((c.z - z) as i64).pow(2);
                        if best.is_none_or(|b| (outside, d, ci, i) < b) {
                            best = Some((outside, d, ci, i));
                        }
                    }
                }
            }
            best.map(|(_, _, ci, i)| (ci as usize, i as usize))
        };

        for (&d, cells) in &paved {
            let Some(surface) = surface.get(&d) else {
                continue;
            };
            let off = |x: i32, z: i32| !surface.contains(x, z);
            let rim = |x: i32, z: i32| {
                surface.contains(x, z)
                    && (off(x + 1, z) || off(x - 1, z) || off(x, z + 1) || off(x, z - 1))
            };
            for (x, z) in cells.iter() {
                let depth = if rim(x, z) {
                    0
                } else if rim(x + 1, z) || rim(x - 1, z) || rim(x, z + 1) || rim(x, z - 1) {
                    1
                } else {
                    continue;
                };
                let Some((ci, i)) = nearest(x, z, Some(d)) else {
                    continue;
                };
                let c = &chains[ci].cells[i];
                let rel = ((x - c.x) as f32, (z - c.z) as f32);
                let side = cross(c.t, rel);
                let side = if side > 0.5 {
                    1
                } else if side < -0.5 {
                    -1
                } else {
                    0
                };
                let kerb = c.kerb != 0
                    && side != 0
                    && (side == c.kerb || c.both)
                    && (depth == 0 || c.half >= WIDE_KERB_HALF);
                if kerb {
                    let red = (c.s / STRIPE).floor() as i64 % 2 == 0;
                    let block = if red { RED_CONCRETE } else { WHITE_CONCRETE };
                    paint.cells.insert((d, x, z), block);
                } else if depth == 0 {
                    paint.cells.insert((d, x, z), WHITE_CONCRETE);
                }
            }
        }

        for element in elements {
            let ProcessedElement::Node(node) = element else {
                continue;
            };
            if is_start_line(&node.tags) {
                paint.start_line(node, &chains, &paved, &nearest);
            }
        }
        paint
    }

    /// Two chequered rows across the track at a start or finish node.
    fn start_line(
        &mut self,
        node: &ProcessedNode,
        chains: &[Chain],
        paved: &FnvHashMap<i32, SparseCells>,
        nearest: &dyn Fn(i32, i32, Option<i32>) -> Option<(usize, usize)>,
    ) {
        let Some((ci, i)) = nearest(node.x, node.z, None) else {
            return;
        };
        let chain = &chains[ci];
        let p = &chain.cells[i];
        let Some(paved) = paved.get(&p.deck) else {
            return;
        };
        let r = p.half * 3 / 2 + 1;
        if (p.x - node.x).abs().max((p.z - node.z).abs()) > p.half + 2 {
            return;
        }
        for x in p.x - r..=p.x + r {
            for z in p.z - r..=p.z + r {
                if !paved.contains(x, z) {
                    continue;
                }
                let rel = ((x - p.x) as f32, (z - p.z) as f32);
                let along = rel.0 * p.t.0 + rel.1 * p.t.1;
                if !(-1.0..1.0).contains(&along) {
                    continue;
                }
                // Not where a parallel stretch or pit lane runs closer.
                match nearest(x, z, Some(p.deck)) {
                    Some((cj, j)) if cj == ci && chain.gap(chain.cells[j].s, p.s) <= 3.0 => {}
                    _ => continue,
                }
                let col = cross(p.t, rel).floor() as i64;
                let row = along.floor() as i64;
                let block = if (row + col).rem_euclid(2) == 0 {
                    WHITE_CONCRETE
                } else {
                    BLACK_CONCRETE
                };
                self.cells.insert((p.deck, x, z), block);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn node(id: u64, x: i32, z: i32) -> ProcessedNode {
        ProcessedNode {
            id,
            tags: HashMap::new(),
            x,
            z,
        }
    }

    fn raceway(id: u64, pts: &[(u64, i32, i32)], extra: &[(&str, &str)]) -> ProcessedElement {
        let mut t = tags(&[("highway", "raceway")]);
        t.extend(tags(extra));
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes: pts.iter().map(|&(n, x, z)| node(n, x, z)).collect(),
            tags: t,
        })
    }

    fn circuit(id: u64, pts: &[(u64, i32, i32)]) -> ProcessedElement {
        raceway(id, pts, &[("sport", "motor"), ("surface", "asphalt")])
    }

    #[test]
    fn widths_follow_the_sport_unless_tagged() {
        let half = |t: &[(&str, &str)]| highway_block_range("raceway", &tags(t), 1.0);
        assert_eq!(half(&[("sport", "motor")]), 6);
        assert_eq!(half(&[("sport", "karting")]), 4);
        assert_eq!(half(&[("sport", "motocross")]), 3);
        assert_eq!(half(&[("sport", "rc_car")]), 1);
        assert_eq!(half(&[]), 5);
        assert_eq!(half(&[("sport", "motor"), ("width", "8")]), 4);
        assert_eq!(half(&[("sport", "motor"), ("lanes", "4")]), 6);
        assert_eq!(
            highway_block_range("raceway", &tags(&[("sport", "motor")]), 0.5),
            3
        );
    }

    #[test]
    fn motor_track_areas_are_paved_but_not_loops_or_running_tracks() {
        let block = |t: &[(&str, &str)]| {
            let mut t = tags(t);
            t.insert("area".into(), "yes".into());
            track_area_block(&t)
        };
        assert_eq!(block(&[("sport", "motor")]), Some(GRAY_CONCRETE_POWDER));
        assert_eq!(block(&[("sport", "karting")]), Some(GRAY_CONCRETE_POWDER));
        assert_eq!(block(&[("sport", "motocross")]), Some(COARSE_DIRT));
        assert_eq!(block(&[("sport", "running")]), None);
        assert_eq!(block(&[]), None);
        let relation = tags(&[("sport", "motor"), ("type", "multipolygon")]);
        assert_eq!(track_area_block(&relation), Some(GRAY_CONCRETE_POWDER));
        // A closed loop is a line: its infield stays as it was.
        assert_eq!(track_area_block(&tags(&[("sport", "motor")])), None);
        use crate::element_processing::RelationArea;
        let rel = |t: &[(&str, &str)]| RelationArea::of(&tags(t));
        let motor = [
            ("leisure", "track"),
            ("sport", "motor"),
            ("type", "multipolygon"),
        ];
        assert_eq!(rel(&motor), Some(RelationArea::Leisure));
        let running = [
            ("leisure", "track"),
            ("sport", "running"),
            ("type", "multipolygon"),
        ];
        assert_eq!(rel(&running), None);
    }

    #[test]
    fn a_straight_gets_edge_lines_and_no_kerbs() {
        let paint = RacewayPaint::build(&[circuit(1, &[(1, 0, 0), (2, 200, 0)])], 1.0);
        for x in 10..190 {
            assert_eq!(paint.at(0, x, -6), Some(WHITE_CONCRETE), "x={x}");
            assert_eq!(paint.at(0, x, 6), Some(WHITE_CONCRETE), "x={x}");
            for z in -5..=5 {
                assert_eq!(paint.at(0, x, z), None, "({x},{z})");
            }
        }
    }

    #[test]
    fn a_corner_is_kerbed_on_the_inside() {
        // Along +x, then a right-angle turn to +z: the inside is the +x/+z corner.
        let paint =
            RacewayPaint::build(&[circuit(1, &[(1, 0, 0), (2, 100, 0), (3, 100, 100)])], 1.0);
        let kerb = |x: i32, z: i32| matches!(paint.at(0, x, z), Some(RED_CONCRETE));
        let inside: usize = (85..95).filter(|&x| kerb(x, 6)).count();
        assert!(inside > 0, "red kerb on the inner edge before the apex");
        assert!((85..95).any(|x| paint.at(0, x, 5) == Some(RED_CONCRETE)
            || paint.at(0, x, 5) == Some(WHITE_CONCRETE)));
        // Far from the corner the edges are plain lines.
        assert_eq!(paint.at(0, 20, 6), Some(WHITE_CONCRETE));
        assert_eq!(paint.at(0, 20, -6), Some(WHITE_CONCRETE));
        assert!(!(20..60).any(|x| kerb(x, 6) || kerb(x, -6)));
        // A right angle is tight, so the outside is kerbed as well.
        assert!((90..110).any(|x| kerb(x, -6)));
    }

    #[test]
    fn a_gentle_bend_is_kerbed_only_on_the_inside() {
        // A 150 m radius arc in 7.5 m chords, turning towards +z.
        let pts: Vec<(u64, i32, i32)> = (0..=24)
            .map(|k| {
                let a = k as f32 * 0.05;
                (
                    k as u64 + 1,
                    (150.0 * a.sin()).round() as i32,
                    (150.0 - 150.0 * a.cos()).round() as i32,
                )
            })
            .collect();
        let way = circuit(1, &pts);
        let paint = RacewayPaint::build(std::slice::from_ref(&way), 1.0);
        assert!(!paint.cells.values().any(|&b| b == RED_CONCRETE));

        let tight: Vec<(u64, i32, i32)> = (0..=24)
            .map(|k| {
                let a = k as f32 * 0.12;
                (
                    k as u64 + 1,
                    (60.0 * a.sin()).round() as i32,
                    (60.0 - 60.0 * a.cos()).round() as i32,
                )
            })
            .collect();
        let paint = RacewayPaint::build(&[circuit(1, &tight)], 1.0);
        let centre = (0.0f32, 60.0f32);
        let mut inner = 0;
        let mut outer = 0;
        for (&(_, x, z), &b) in &paint.cells {
            if b != RED_CONCRETE {
                continue;
            }
            let r = (x as f32 - centre.0).hypot(z as f32 - centre.1);
            if r < 60.0 {
                inner += 1;
            } else {
                outer += 1;
            }
        }
        assert!(inner > 10, "60 m bend has an inner kerb ({inner})");
        assert_eq!(outer, 0, "but no outer kerb");
    }

    #[test]
    fn kerbs_stripe_red_and_white() {
        let paint =
            RacewayPaint::build(&[circuit(1, &[(1, 0, 0), (2, 100, 0), (3, 100, 100)])], 1.0);
        let row: Vec<Block> = (80..100).filter_map(|x| paint.at(0, x, 6)).collect();
        assert!(row.contains(&RED_CONCRETE) && row.contains(&WHITE_CONCRETE));
    }

    #[test]
    fn joined_ways_read_one_corner_across_the_split() {
        let split = RacewayPaint::build(
            &[
                circuit(1, &[(1, 0, 0), (2, 100, 0)]),
                circuit(2, &[(3, 100, 100), (2, 100, 0)]),
            ],
            1.0,
        );
        let whole =
            RacewayPaint::build(&[circuit(1, &[(1, 0, 0), (2, 100, 0), (3, 100, 100)])], 1.0);
        assert_eq!(split.cells, whole.cells);
    }

    #[test]
    fn dirt_tracks_and_narrow_tracks_are_not_painted() {
        let dirt = raceway(1, &[(1, 0, 0), (2, 200, 0)], &[("sport", "motocross")]);
        assert!(RacewayPaint::build(&[dirt], 1.0).cells.is_empty());
        let rally = raceway(
            1,
            &[(1, 0, 0), (2, 200, 0)],
            &[("sport", "motor"), ("surface", "dirt")],
        );
        assert!(RacewayPaint::build(&[rally], 1.0).cells.is_empty());
        let rc = raceway(1, &[(1, 0, 0), (2, 200, 0)], &[("sport", "rc_car")]);
        assert!(RacewayPaint::build(&[rc], 1.0).cells.is_empty());
    }

    #[test]
    fn a_pit_lane_gets_lines_but_no_kerbs() {
        let pit = raceway(
            1,
            &[(1, 0, 0), (2, 100, 0), (3, 100, 100)],
            &[("sport", "motor"), ("raceway", "pitlane")],
        );
        let paint = RacewayPaint::build(&[pit], 1.0);
        assert!(!paint.cells.values().any(|&b| b == RED_CONCRETE));
        assert_eq!(paint.at(0, 20, 6), Some(WHITE_CONCRETE));
    }

    #[test]
    fn no_line_across_where_tracks_join() {
        // A branch leaving the straight at x=100: its mouth stays open.
        let paint = RacewayPaint::build(
            &[
                circuit(1, &[(1, 0, 0), (2, 100, 0), (3, 200, 0)]),
                circuit(2, &[(2, 100, 0), (4, 100, 100)]),
            ],
            1.0,
        );
        for x in 96..=104 {
            assert_ne!(paint.at(0, x, 6), Some(WHITE_CONCRETE), "x={x}");
        }
    }

    #[test]
    fn a_start_node_paints_a_chequered_band_across() {
        let mut start = node(9, 100, 0);
        start.tags = tags(&[("raceway", "start-finish")]);
        let paint = RacewayPaint::build(
            &[
                circuit(1, &[(1, 0, 0), (2, 200, 0)]),
                ProcessedElement::Node(start),
            ],
            1.0,
        );
        for z in -6..=6 {
            let pair = (paint.at(0, 99, z), paint.at(0, 100, z));
            assert!(
                matches!(
                    pair,
                    (Some(WHITE_CONCRETE), Some(BLACK_CONCRETE))
                        | (Some(BLACK_CONCRETE), Some(WHITE_CONCRETE))
                ),
                "z={z}: {pair:?}"
            );
        }
        assert_eq!(paint.at(0, 98, 0), None);
        assert_eq!(paint.at(0, 101, 0), None);
    }

    #[test]
    fn a_bridge_keeps_both_tracks_edges_where_they_cross() {
        let bridge = raceway(
            2,
            &[(3, 100, -80), (4, 100, 80)],
            &[
                ("sport", "motor"),
                ("surface", "asphalt"),
                ("bridge", "yes"),
                ("layer", "1"),
            ],
        );
        let paint = RacewayPaint::build(&[circuit(1, &[(1, 0, 0), (2, 200, 0)]), bridge], 1.0);
        for x in 94..=106 {
            assert_eq!(paint.at(0, x, 6), Some(WHITE_CONCRETE), "ground edge x={x}");
        }
        for z in -6..=6 {
            assert_eq!(paint.at(1, 106, z), Some(WHITE_CONCRETE), "deck edge z={z}");
            assert_eq!(paint.at(1, 100, z), None, "deck middle z={z}");
        }
    }

    #[test]
    fn no_line_across_where_a_track_runs_onto_a_bridge() {
        let deck = [
            ("sport", "motor"),
            ("surface", "asphalt"),
            ("bridge", "yes"),
        ];
        let paint = RacewayPaint::build(
            &[
                circuit(1, &[(1, 0, 0), (2, 100, 0)]),
                raceway(2, &[(2, 100, 0), (3, 140, 0)], &deck),
                circuit(3, &[(3, 140, 0), (4, 240, 0)]),
            ],
            1.0,
        );
        for (d, x) in [(0, 100), (1, 100), (1, 140), (0, 140)] {
            for z in -5..=5 {
                assert_eq!(paint.at(d, x, z), None, "deck {d} ({x},{z})");
            }
        }
        assert_eq!(paint.at(1, 120, 6), Some(WHITE_CONCRETE));
    }

    #[test]
    fn a_pit_lane_beside_a_corner_leaves_the_circuit_its_kerbs() {
        // The pit lane's centreline is nearer the circuit's inner rim than the circuit's own.
        let pit = raceway(
            2,
            &[(5, 40, 11), (6, 94, 11)],
            &[("sport", "motor"), ("raceway", "pitlane"), ("width", "4")],
        );
        let paint = RacewayPaint::build(
            &[circuit(1, &[(1, 0, 0), (2, 100, 0), (3, 100, 100)]), pit],
            1.0,
        );
        assert!((85..92).any(|x| paint.at(0, x, 6) == Some(RED_CONCRETE)));
    }

    #[test]
    fn a_closed_kart_loop_is_kerbed_all_round() {
        let ring = raceway(
            1,
            &[(1, 0, 0), (2, 60, 0), (3, 60, 40), (4, 0, 40), (1, 0, 0)],
            &[("sport", "karting")],
        );
        let paint = RacewayPaint::build(&[ring], 1.0);
        // Every corner of the rectangle has red on its inner side.
        for (cx, cz) in [(0, 0), (60, 0), (60, 40), (0, 40)] {
            let dx = if cx == 0 { 1 } else { -1 };
            let dz = if cz == 0 { 1 } else { -1 };
            let near = (0..12).any(|k| {
                (0..12).any(|j| {
                    paint.at(0, cx + dx * (4 + k), cz + dz * (4 + j)) == Some(RED_CONCRETE)
                })
            });
            assert!(near, "corner ({cx},{cz})");
        }
    }
}
