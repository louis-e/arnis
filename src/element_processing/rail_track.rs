//! How a way's track lies: its cells, the track it carries on into at either end, and a
//! height profile its rails can follow. Minecraft rails only climb on straight track, one
//! block at a time, and can't dip for a single block, so the profile keeps to that.

use crate::block_definitions::Block;
#[cfg(test)]
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::railways::{determine_rail_with_slope, smooth_diagonal_rails};
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use fnv::FnvHashMap;

/// Cells a way's end looks back along for its heading, and ahead along the next track.
const HEADING_CELLS: usize = 6;
/// Sharpest bend between two ways still taken as one line.
const MAX_JOIN_DEFLECT_DEG: f64 = 60.0;
/// Cells past a join kept for shaping the rails either side of it.
const BEYOND_CELLS: usize = 2;
/// What a profile pays for a forced step it can't lay as a working rail.
const BROKEN: f64 = 1000.0;

/// Whether a way's track runs on the surface (on the ground or a bridge) or in a tunnel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrackLevel {
    Surface,
    Tunnel,
}

/// What lies past one end of a way.
#[derive(Clone, Debug)]
pub struct TrackEnd {
    /// The cells of the track the line carries on along, nearest first.
    pub beyond: Vec<(i32, i32)>,
    /// That track's way, the index of the end node's cell in it and which way it runs on.
    pub into: Option<(u64, usize, isize)>,
    /// Whether this way lays the rail on its end cell. Where a branch meets a line, the line does.
    pub owns: bool,
    /// Another track ends or passes here, so this end keeps level with it.
    pub shared: bool,
    /// A track on the surface meets this end, as at a tunnel portal.
    pub meets_surface: bool,
}

impl Default for TrackEnd {
    fn default() -> Self {
        Self {
            beyond: Vec::new(),
            into: None,
            owns: true,
            shared: false,
            meets_surface: false,
        }
    }
}

/// The ends of every track way, worked out once for the whole network so every tile
/// joins the same ways the same way.
#[derive(Default)]
pub struct RailJoins {
    ends: FnvHashMap<u64, [TrackEnd; 2]>,
}

struct Track<'a> {
    way: &'a ProcessedWay,
    level: TrackLevel,
    cells: Vec<(i32, i32)>,
    node_cells: Vec<usize>,
}

/// The way an end carries on along: the track, its node index there, the direction and the bend.
type Pick = (usize, usize, isize, f64);

impl RailJoins {
    pub fn build(
        elements: &[ProcessedElement],
        level: impl Fn(&ProcessedWay) -> Option<TrackLevel>,
    ) -> Self {
        let tracks: Vec<Track> = elements
            .iter()
            .filter_map(|e| match e {
                ProcessedElement::Way(way) => level(way).map(|level| {
                    let (cells, node_cells) = track_cells(way);
                    Track {
                        way,
                        level,
                        cells,
                        node_cells,
                    }
                }),
                _ => None,
            })
            .filter(|t| !t.cells.is_empty())
            .collect();
        let mut at_node: FnvHashMap<u64, Vec<(usize, usize)>> = FnvHashMap::default();
        for (t, track) in tracks.iter().enumerate() {
            for (k, node) in track.way.nodes.iter().enumerate() {
                at_node.entry(node.id).or_default().push((t, k));
            }
        }
        let last = |t: usize| tracks[t].way.nodes.len() - 1;

        // The straightest track each end carries on into.
        let mut picks: FnvHashMap<(usize, usize), Option<Pick>> = FnvHashMap::default();
        for (t, track) in tracks.iter().enumerate() {
            for k in [0, last(t)] {
                let others = &at_node[&track.way.nodes[k].id];
                let n = track.cells.len();
                let reach = HEADING_CELLS.min(n - 1);
                let (at, back) = if k == 0 {
                    (track.cells[0], track.cells[reach])
                } else {
                    (track.cells[n - 1], track.cells[n - 1 - reach])
                };
                let heading = ((at.0 - back.0) as f64, (at.1 - back.1) as f64);
                let mut best: Option<Pick> = None;
                for &(o, ko) in others {
                    if (o, ko) == (t, k) {
                        continue;
                    }
                    let other = &tracks[o];
                    let c = other.node_cells[ko];
                    for dir in [1isize, -1] {
                        let room = if dir > 0 {
                            other.cells.len() - 1 - c
                        } else {
                            c
                        };
                        if room == 0 {
                            continue;
                        }
                        let ahead = other.cells
                            [(c as isize + dir * HEADING_CELLS.min(room) as isize) as usize];
                        let out = ((ahead.0 - at.0) as f64, (ahead.1 - at.1) as f64);
                        let bend = deflection(heading, out);
                        let better = match best {
                            None => true,
                            Some((bo, _, _, bb)) => {
                                bend < bb - 1e-9
                                    || (bend < bb + 1e-9 && tracks[o].way.id < tracks[bo].way.id)
                            }
                        };
                        if better {
                            best = Some((o, ko, dir, bend));
                        }
                    }
                }
                picks.insert(
                    (t, k),
                    best.filter(|&(.., bend)| bend <= MAX_JOIN_DEFLECT_DEG),
                );
            }
        }

        let mut ends: FnvHashMap<u64, [TrackEnd; 2]> = FnvHashMap::default();
        for (t, track) in tracks.iter().enumerate() {
            let mut pair: [TrackEnd; 2] = Default::default();
            for (e, k) in [0, last(t)].into_iter().enumerate() {
                let others: Vec<(usize, usize)> = at_node[&track.way.nodes[k].id]
                    .iter()
                    .copied()
                    .filter(|&p| p != (t, k))
                    .collect();
                if others.is_empty() {
                    continue;
                }
                let end = &mut pair[e];
                end.shared = true;
                end.meets_surface = others
                    .iter()
                    .any(|&(o, _)| tracks[o].level == TrackLevel::Surface);
                let pick = picks[&(t, k)];
                if let Some((o, ko, dir, _)) = pick {
                    let other = &tracks[o];
                    let c = other.node_cells[ko];
                    end.beyond = (1..=BEYOND_CELLS as isize)
                        .map(|s| c as isize + dir * s)
                        .take_while(|&i| i >= 0 && (i as usize) < other.cells.len())
                        .map(|i| other.cells[i as usize])
                        .collect();
                    end.into = Some((other.way.id, c, dir));
                }
                end.owns = owns_node(&tracks, &at_node, &picks, (t, k));
            }
            ends.insert(track.way.id, pair);
        }
        Self { ends }
    }

    /// What lies past the start and the end of a way.
    pub fn ends(&self, way: u64) -> [TrackEnd; 2] {
        self.ends.get(&way).cloned().unwrap_or_default()
    }
}

/// Whether the end `(t, k)` lays the rail on its node. A way passing through the node lays
/// it; otherwise two ends that pick each other do; failing both, the lowest way id does.
fn owns_node(
    tracks: &[Track],
    at_node: &FnvHashMap<u64, Vec<(usize, usize)>>,
    picks: &FnvHashMap<(usize, usize), Option<Pick>>,
    (t, k): (usize, usize),
) -> bool {
    let here = &at_node[&tracks[t].way.nodes[k].id];
    let is_end = |(o, ko): (usize, usize)| ko == 0 || ko == tracks[o].way.nodes.len() - 1;
    if here.iter().any(|&p| !is_end(p)) {
        return false;
    }
    let mutual = |p: (usize, usize)| {
        picks[&p].is_some_and(|(o, ko, ..)| {
            is_end((o, ko)) && picks[&(o, ko)].is_some_and(|(b, bk, ..)| (b, bk) == p)
        })
    };
    if mutual((t, k)) {
        return true;
    }
    if here.iter().any(|&p| mutual(p)) {
        return false;
    }
    let lowest = here
        .iter()
        .min_by_key(|&&(o, ko)| (tracks[o].way.id, ko))
        .copied();
    lowest == Some((t, k))
}

/// The angle in degrees between two headings.
fn deflection(a: (f64, f64), b: (f64, f64)) -> f64 {
    let cross = a.0 * b.1 - a.1 * b.0;
    let dot = a.0 * b.0 + a.1 * b.1;
    cross.atan2(dot).abs().to_degrees()
}

/// A way's track cells, each a block from the last, and the cell each node lies on.
pub(crate) fn track_cells(way: &ProcessedWay) -> (Vec<(i32, i32)>, Vec<usize>) {
    let mut cells: Vec<(i32, i32)> = Vec::new();
    let mut node_cells = Vec::with_capacity(way.nodes.len());
    let [first, _, ..] = way.nodes.as_slice() else {
        return (cells, node_cells);
    };
    cells.push((first.x, first.z));
    node_cells.push(0);
    for window in way.nodes.windows(2) {
        let line = bresenham_line(window[0].x, 0, window[0].z, window[1].x, 0, window[1].z);
        for (x, _, z) in smooth_diagonal_rails(&line) {
            if cells.last() != Some(&(x, z)) {
                cells.push((x, z));
            }
        }
        node_cells.push(cells.len() - 1);
    }
    (cells, node_cells)
}

/// For each cell, whether it lies in a straight line with the cells either side, so a
/// rail there can climb. A cell with nothing on one side counts as straight.
pub(crate) fn straight_cells(
    cells: &[(i32, i32)],
    before: Option<(i32, i32)>,
    after: Option<(i32, i32)>,
) -> Vec<bool> {
    let n = cells.len();
    (0..n)
        .map(|i| {
            let prev = if i > 0 { Some(cells[i - 1]) } else { before };
            let next = if i + 1 < n { Some(cells[i + 1]) } else { after };
            match (prev, next) {
                (Some(p), Some(q)) => {
                    let c = cells[i];
                    (p.0 == c.0 && q.0 == c.0) || (p.1 == c.1 && q.1 == c.1)
                }
                _ => true,
            }
        })
        .collect()
}

/// Rail bed heights along a run of cells. Each cell takes a height from `range` at what
/// `cost` charges for it, plus `step` for each change of level; a level changes only
/// where a rail can climb: on a straight cell, onto a cell that isn't already climbing
/// the other way, by one block.
pub(crate) fn rail_profile(
    straight: &[bool],
    range: impl Fn(usize) -> (i32, i32),
    cost: impl Fn(usize, i32) -> f64,
    step: f64,
) -> Vec<i32> {
    let n = straight.len();
    if n == 0 {
        return Vec::new();
    }
    // Per cell, the lowest height and where its states start in `back`.
    let mut lows: Vec<i32> = Vec::with_capacity(n);
    let mut starts = Vec::with_capacity(n);
    // A state is a height and whether the cell climbs back to the cell before it.
    let mut back: Vec<u32> = Vec::new();
    let mut prev: Vec<f64> = Vec::new();
    for i in 0..n {
        let (lo, hi) = range(i);
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        let w = (hi - lo + 1) as usize;
        let mut cur = vec![f64::INFINITY; w * 2];
        starts.push(back.len());
        back.resize(back.len() + w * 2, u32::MAX);
        let base = starts[i];
        for bi in 0..w {
            let b = lo + bi as i32;
            let own = cost(i, b);
            if i == 0 {
                cur[bi * 2] = own;
                continue;
            }
            let plo = lows[i - 1];
            for (s, &was) in prev.iter().enumerate() {
                if !was.is_finite() {
                    continue;
                }
                let a = plo + (s / 2) as i32;
                let climbing_back = s % 2 == 1;
                let d = b - a;
                let (extra, low) = match d {
                    0 => (0.0, false),
                    1 if straight[i - 1] && !climbing_back => (step, false),
                    -1 if straight[i] => (step, true),
                    1 | -1 => (BROKEN, d < 0),
                    _ => (BROKEN * d.abs() as f64, false),
                };
                let total = was + extra + own;
                let slot = bi * 2 + low as usize;
                if total < cur[slot] {
                    cur[slot] = total;
                    back[base + slot] = s as u32;
                }
            }
        }
        lows.push(lo);
        prev = cur;
    }
    let mut s = (0..prev.len())
        .min_by(|&a, &b| prev[a].total_cmp(&prev[b]))
        .unwrap_or(0);
    let mut heights = vec![0; n];
    for i in (0..n).rev() {
        heights[i] = lows[i] + (s / 2) as i32;
        if i > 0 {
            s = back[starts[i] + s] as usize;
        }
    }
    heights
}

/// The rail block for each cell, from the cells it runs between and their heights.
/// `before` and `after` are the cells past either end, where the track goes on.
pub(crate) fn rail_shapes(
    cells: &[(i32, i32)],
    heights: &[i32],
    before: Option<((i32, i32), i32)>,
    after: Option<((i32, i32), i32)>,
) -> Vec<Block> {
    let n = cells.len();
    (0..n)
        .map(|i| {
            let prev = if i > 0 {
                Some((cells[i - 1], heights[i - 1]))
            } else {
                before
            };
            let next = if i + 1 < n {
                Some((cells[i + 1], heights[i + 1]))
            } else {
                after
            };
            let y = heights[i];
            determine_rail_with_slope(
                cells[i],
                prev.map(|p| p.0),
                next.map(|q| q.0),
                prev.map_or(y, |p| p.1),
                y,
                next.map_or(y, |q| q.1),
            )
        })
        .collect()
}

/// Where a rail's two ends lie, in half blocks, so joined rails share an end.
#[cfg(test)]
fn rail_ends(block: Block, (x, z): (i32, i32), y: i32) -> [(i32, i32, i32); 2] {
    let ends = match block {
        b if b == RAIL_NORTH_SOUTH => [(0, 0, -1), (0, 0, 1)],
        b if b == RAIL_EAST_WEST => [(-1, 0, 0), (1, 0, 0)],
        b if b == RAIL_ASCENDING_EAST => [(-1, 0, 0), (1, 2, 0)],
        b if b == RAIL_ASCENDING_WEST => [(1, 0, 0), (-1, 2, 0)],
        b if b == RAIL_ASCENDING_NORTH => [(0, 0, 1), (0, 2, -1)],
        b if b == RAIL_ASCENDING_SOUTH => [(0, 0, -1), (0, 2, 1)],
        b if b == RAIL_SOUTH_EAST => [(0, 0, 1), (1, 0, 0)],
        b if b == RAIL_SOUTH_WEST => [(0, 0, 1), (-1, 0, 0)],
        b if b == RAIL_NORTH_WEST => [(0, 0, -1), (-1, 0, 0)],
        b if b == RAIL_NORTH_EAST => [(0, 0, -1), (1, 0, 0)],
        _ => panic!("not a rail"),
    };
    ends.map(|(dx, dy, dz)| (2 * x + dx, 2 * y + dy, 2 * z + dz))
}

/// Counts rail ends that meet no other rail, over runs laid as (cell, rail y, block).
#[cfg(test)]
pub(crate) fn loose_ends(laid: &[((i32, i32), i32, Block)]) -> usize {
    let mut count: FnvHashMap<(i32, i32, i32), usize> = FnvHashMap::default();
    let mut cells: FnvHashMap<(i32, i32), ()> = FnvHashMap::default();
    for &(c, y, b) in laid {
        if cells.insert(c, ()).is_some() {
            continue;
        }
        for e in rail_ends(b, c, y) {
            *count.entry(e).or_default() += 1;
        }
    }
    count.values().filter(|&&v| v == 1).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_definitions::*;
    use crate::osm_parser::ProcessedNode;

    fn way(id: u64, nodes: &[(u64, i32, i32)]) -> ProcessedWay {
        ProcessedWay {
            id,
            nodes: nodes
                .iter()
                .map(|&(id, x, z)| ProcessedNode {
                    id,
                    tags: Default::default(),
                    x,
                    z,
                })
                .collect(),
            tags: [("railway".to_string(), "rail".to_string())]
                .into_iter()
                .collect(),
        }
    }

    fn joins(ways: &[ProcessedWay]) -> RailJoins {
        let elements: Vec<ProcessedElement> =
            ways.iter().cloned().map(ProcessedElement::Way).collect();
        RailJoins::build(&elements, |_| Some(TrackLevel::Surface))
    }

    fn lay(cells: &[(i32, i32)], ground: &[i32]) -> Vec<((i32, i32), i32, Block)> {
        let straight = straight_cells(cells, None, None);
        let h = rail_profile(
            &straight,
            |i| (ground[i] - 8, ground[i] + 8),
            |i, y| (y - ground[i]).abs() as f64,
            2.5,
        );
        let shapes = rail_shapes(cells, &h, None, None);
        (0..cells.len())
            .map(|i| (cells[i], h[i] + 1, shapes[i]))
            .collect()
    }

    #[test]
    fn a_curve_over_rolling_ground_stays_joined() {
        // East, then a diagonal, then south, over ground that rises and falls.
        let w = way(1, &[(1, 0, 0), (2, 30, 0), (3, 60, 30), (4, 60, 70)]);
        let (cells, _) = track_cells(&w);
        let ground: Vec<i32> = (0..cells.len())
            .map(|i| ((i as f64 / 9.0).sin() * 4.0).round() as i32)
            .collect();
        let laid = lay(&cells, &ground);
        // Only the two real ends of the run are loose.
        assert_eq!(loose_ends(&laid), 2);
        // It climbs, not just lies flat.
        let ys: Vec<i32> = laid.iter().map(|l| l.1).collect();
        assert!(ys.iter().max().unwrap() - ys.iter().min().unwrap() >= 4);
    }

    #[test]
    fn a_one_block_dip_is_filled_rather_than_stepped_into() {
        let cells: Vec<(i32, i32)> = (0..20).map(|x| (x, 0)).collect();
        let mut ground = vec![5; 20];
        ground[10] = 4;
        let laid = lay(&cells, &ground);
        assert!(
            laid.iter().all(|l| l.1 == 6),
            "{:?}",
            laid.iter().map(|l| l.1).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_climb_waits_for_straight_track() {
        // A diagonal is all curves, so a rise along it has to come before or after.
        let w = way(1, &[(1, 0, 0), (2, 20, 0), (3, 40, 20), (4, 60, 20)]);
        let (cells, _) = track_cells(&w);
        let ground: Vec<i32> = cells.iter().map(|&(x, _)| x / 6).collect();
        let laid = lay(&cells, &ground);
        assert_eq!(loose_ends(&laid), 2);
        let straight = straight_cells(&cells, None, None);
        for i in 1..cells.len() {
            let (a, b) = (laid[i - 1].1, laid[i].1);
            if a != b {
                let low = if a < b { i - 1 } else { i };
                assert!(straight[low], "a climb at cell {low}, on a bend");
            }
        }
    }

    #[test]
    fn two_ways_meeting_on_a_bend_lay_one_joined_curve() {
        // The node at (20, 0) bends the line from east to south-east.
        let a = way(1, &[(1, 0, 0), (2, 20, 0)]);
        let b = way(2, &[(2, 20, 0), (3, 40, 20)]);
        let j = joins(&[a.clone(), b.clone()]);
        let mut laid = Vec::new();
        for w in [&a, &b] {
            let (cells, _) = track_cells(w);
            let [start, end] = j.ends(w.id);
            let h = vec![0; cells.len()];
            let before = start.beyond.first().map(|&c| (c, 0));
            let after = end.beyond.first().map(|&c| (c, 0));
            let shapes = rail_shapes(&cells, &h, before, after);
            for (i, &c) in cells.iter().enumerate() {
                let skip = (i == 0 && !start.owns) || (i + 1 == cells.len() && !end.owns);
                if !skip {
                    laid.push((c, 1, shapes[i]));
                }
            }
        }
        assert_eq!(loose_ends(&laid), 2);
        let corner = laid.iter().find(|l| l.0 == (20, 0)).unwrap().2;
        assert_eq!(corner, RAIL_SOUTH_WEST);
    }

    #[test]
    fn a_branch_leaves_the_switch_to_the_line_it_joins() {
        // A line straight through node 2; a siding leaves it there.
        let a = way(1, &[(1, 0, 0), (2, 20, 0)]);
        let b = way(2, &[(2, 20, 0), (3, 40, 0)]);
        let c = way(3, &[(2, 20, 0), (4, 40, 10)]);
        let j = joins(&[a, b, c]);
        assert!(j.ends(1)[1].owns && j.ends(2)[0].owns);
        assert!(!j.ends(3)[0].owns);
        assert_eq!(j.ends(1)[1].beyond.first(), Some(&(21, 0)));
        // A line passing through the node lays it, not the way that ends there.
        let line = way(4, &[(1, 0, 0), (2, 20, 0), (3, 40, 0)]);
        let spur = way(5, &[(2, 20, 0), (5, 40, 8)]);
        let j = joins(&[line, spur]);
        assert!(!j.ends(5)[0].owns);
        assert!(j.ends(5)[0].shared);
    }

    #[test]
    fn a_closed_ring_joins_its_own_ends() {
        // Closed on a node halfway along a side, as rings are drawn.
        let w = way(
            1,
            &[
                (1, 10, 0),
                (2, 20, 0),
                (3, 20, 20),
                (4, 0, 20),
                (5, 0, 0),
                (1, 10, 0),
            ],
        );
        let j = joins(&[w]);
        let [start, end] = j.ends(1);
        assert!(start.owns && end.owns);
        assert_eq!(start.beyond.first(), Some(&(9, 0)));
        assert_eq!(end.beyond.first(), Some(&(11, 0)));
    }
}
