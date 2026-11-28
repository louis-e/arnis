//! "Align to the streets": the world turn that lays most building walls along the block grid.
//!
//! Every wall piece votes for its direction, folded onto a quarter turn and weighted by its
//! length; the strongest direction wins, so a district with two street grids follows the one
//! with more wall. One pass over the outlines into a fixed histogram, nothing stored per wall.

use rayon::prelude::*;

use crate::args::Args;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode};
use crate::progress::{emit_gui_progress_update, MESSAGE_ONLY};

const BIN_DEG: f64 = 0.5;
const BINS: usize = 180;
/// A wall this close to an axis stands flat in blocks: one block of drift over about 20.
const ON_GRID_DEG: f64 = 3.0;
const ON_GRID_BINS: usize = 6;
/// Half-width of the window the strongest direction settles in: wide enough to hold a grid
/// that rounding spread over several degrees, narrow enough to leave a second grid out.
const SETTLE_DEG: f64 = 8.0;
/// Below this a wall of 30 blocks drifts less than one: turning would not show.
const MIN_TURN_DEG: f64 = 2.0;
/// Shorter pieces are curves or rounding: their direction is too coarse to count.
const MIN_PIECE: i64 = 4;
/// Less wall than this (in blocks) says too little; the roads are taken with it.
const MIN_LENGTH: f64 = 400.0;
/// A turn has to put this much more of the length on the grid to be worth north not being up.
const MIN_GAIN: f64 = 0.15;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    Buildings,
    BuildingsAndRoads,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Alignment {
    /// The clockwise turn, in degrees in (-45, 45], that lines the strongest direction up.
    pub turn: f64,
    /// `turn` where it is worth it, else 0.
    pub rotation: f64,
    /// Share of the measured length on the grid unturned and after `turn`.
    pub before: f64,
    pub after: f64,
    pub source: Source,
}

/// Wall length per direction bin, with each bin's weighted direction (as 4x the angle, where a
/// quarter turn is a full circle) to place the winner between bin edges.
#[derive(Clone)]
struct Directions {
    length: [f64; BINS],
    cos4: [f64; BINS],
    sin4: [f64; BINS],
    total: f64,
}

impl Directions {
    fn new() -> Self {
        Directions {
            length: [0.0; BINS],
            cos4: [0.0; BINS],
            sin4: [0.0; BINS],
            total: 0.0,
        }
    }

    fn add_line(&mut self, nodes: &[ProcessedNode]) {
        for pair in nodes.windows(2) {
            let (dx, dz) = (
                i64::from(pair[1].x - pair[0].x),
                i64::from(pair[1].z - pair[0].z),
            );
            if dx * dx + dz * dz < MIN_PIECE * MIN_PIECE {
                continue;
            }
            let len = ((dx * dx + dz * dz) as f64).sqrt();
            let deg = (dz as f64).atan2(dx as f64).to_degrees().rem_euclid(90.0);
            let bin = ((deg / BIN_DEG) as usize).min(BINS - 1);
            let (s, c) = (4.0 * deg).to_radians().sin_cos();
            self.length[bin] += len;
            self.cos4[bin] += len * c;
            self.sin4[bin] += len * s;
            self.total += len;
        }
    }

    fn merge(mut self, other: &Directions) -> Self {
        for i in 0..BINS {
            self.length[i] += other.length[i];
            self.cos4[i] += other.cos4[i];
            self.sin4[i] += other.sin4[i];
        }
        self.total += other.total;
        self
    }

    /// The direction most wall runs in, in [0, 90): the strongest bin of the smoothed
    /// histogram, then the window recentred on its own weighted mean until it settles, so
    /// rounding that spreads a grid over several degrees does not pull it off centre.
    fn strongest(&self) -> f64 {
        let ring = |i: isize| i.rem_euclid(BINS as isize) as usize;
        let k = ON_GRID_BINS as isize;
        let peak = (0..BINS)
            .map(|i| {
                let smooth: f64 = (-k..=k)
                    .map(|d| (k + 1 - d.abs()) as f64 * self.length[ring(i as isize + d)])
                    .sum();
                (smooth, i)
            })
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map_or(0, |(_, i)| i);
        let mut centre = (peak as f64 + 0.5) * BIN_DEG;
        for _ in 0..20 {
            let (mut c, mut s) = (0.0, 0.0);
            for (i, (&c4, &s4)) in self.cos4.iter().zip(&self.sin4).enumerate() {
                let off = ((i as f64 + 0.5) * BIN_DEG - centre + 45.0).rem_euclid(90.0) - 45.0;
                let weight = (1.0 - off.abs() / SETTLE_DEG).max(0.0);
                c += weight * c4;
                s += weight * s4;
            }
            let next = (s.atan2(c).to_degrees() / 4.0).rem_euclid(90.0);
            let moved = ((next - centre + 45.0).rem_euclid(90.0) - 45.0).abs();
            centre = next;
            if moved < 0.01 {
                break;
            }
        }
        centre
    }

    /// Share of the length within ON_GRID_DEG of an axis once turned by `rotation`.
    fn on_grid(&self, rotation: f64) -> f64 {
        if self.total <= 0.0 {
            return 0.0;
        }
        let on: f64 = (0..BINS)
            .filter(|&i| {
                let deg = ((i as f64 + 0.5) * BIN_DEG + rotation).rem_euclid(90.0);
                deg.min(90.0 - deg) <= ON_GRID_DEG
            })
            .map(|i| self.length[i])
            .sum();
        on / self.total
    }
}

fn is_building(tags: &std::collections::HashMap<String, String>) -> bool {
    tags.get("building").is_some_and(|v| v != "no")
}

fn is_road(tags: &std::collections::HashMap<String, String>) -> bool {
    tags.get("highway").is_some_and(|v| {
        matches!(
            v.as_str(),
            "motorway"
                | "trunk"
                | "primary"
                | "secondary"
                | "tertiary"
                | "unclassified"
                | "residential"
                | "living_street"
                | "pedestrian"
        )
    })
}

/// The turn that puts most building walls on the block grid, or None where the map holds too
/// few walls and streets to tell.
pub fn street_alignment(elements: &[ProcessedElement]) -> Option<Alignment> {
    let (walls, roads) = elements
        .par_iter()
        .fold(
            || (Directions::new(), Directions::new()),
            |(mut walls, mut roads), e| {
                match e {
                    ProcessedElement::Way(w) if is_building(&w.tags) => walls.add_line(&w.nodes),
                    ProcessedElement::Way(w) if is_road(&w.tags) => roads.add_line(&w.nodes),
                    ProcessedElement::Relation(r) if is_building(&r.tags) => {
                        for m in &r.members {
                            if m.role == ProcessedMemberRole::Outer {
                                walls.add_line(&m.way.nodes);
                            }
                        }
                    }
                    _ => {}
                }
                (walls, roads)
            },
        )
        .reduce(
            || (Directions::new(), Directions::new()),
            |a, b| (a.0.merge(&b.0), a.1.merge(&b.1)),
        );
    let (dirs, source) = if walls.total >= MIN_LENGTH {
        (walls, Source::Buildings)
    } else {
        (walls.merge(&roads), Source::BuildingsAndRoads)
    };
    if dirs.total < MIN_LENGTH {
        return None;
    }
    let mut turn = -dirs.strongest();
    if turn <= -45.0 {
        turn += 90.0;
    }
    let turn = (turn * 10.0).round() / 10.0;
    let (before, after) = (dirs.on_grid(0.0), dirs.on_grid(turn));
    let worth = after - before >= MIN_GAIN && turn.abs() >= MIN_TURN_DEG;
    Some(Alignment {
        turn,
        rotation: if worth { turn } else { 0.0 },
        before,
        after,
        source,
    })
}

/// With `--align-to-streets`, sets `args.rotation` from the map and says what was chosen: in
/// full on the console, in short on the GUI's status line.
pub fn settle(args: &mut Args, elements: &[ProcessedElement]) {
    if !args.align_to_streets {
        return;
    }
    let pct = |share: f64| (share * 100.0).round();
    // The per-position climate does not follow a turned ground (the CLI refuses the pair).
    let (full, short) = if args.climate_mode == crate::climate_field::ClimateMode::PerPosition {
        (
            "Align to streets: not with the per-position climate, not turned".to_string(),
            "Not turned: per-position climate".to_string(),
        )
    } else {
        match street_alignment(elements) {
            None => (
                "Align to streets: too few buildings or streets to measure, not turned".to_string(),
                "Not turned: too few buildings".to_string(),
            ),
            Some(a) if a.turn.abs() < MIN_TURN_DEG => (
                "Align to streets: the buildings' main direction already follows the block grid, not turned"
                    .to_string(),
                "Not turned: already on the grid".to_string(),
            ),
            Some(a) if a.rotation == 0.0 => (
                format!(
                    "Align to streets: turning {:.1}° would put only {}% of walls on the block grid instead of {}%, not turned",
                    a.turn,
                    pct(a.after),
                    pct(a.before)
                ),
                "Not turned: no clear direction".to_string(),
            ),
            Some(a) => {
                args.rotation = a.rotation;
                let what = match a.source {
                    Source::Buildings => "walls",
                    Source::BuildingsAndRoads => "walls and streets",
                };
                (
                    format!(
                        "Align to streets: turned {:.1}°, {}% of {what} on the block grid (was {}%)",
                        a.rotation,
                        pct(a.after),
                        pct(a.before)
                    ),
                    format!("Aligned to streets: {:.1}°", a.rotation),
                )
            }
        }
    };
    println!("{full}");
    emit_gui_progress_update(MESSAGE_ONLY, &short);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::map_transformation::rotate::rotate_xz_point;
    use std::collections::HashMap;

    fn node(x: f64, z: f64) -> ProcessedNode {
        ProcessedNode {
            id: 0,
            tags: HashMap::new(),
            x: x.round() as i32,
            z: z.round() as i32,
        }
    }

    /// A row of `n` buildings `w` x `d` blocks, their walls at `deg` clockwise from east.
    fn block_row(deg: f64, n: usize, w: f64, d: f64, origin: (f64, f64)) -> Vec<ProcessedElement> {
        let (s, c) = deg.to_radians().sin_cos();
        let at = |u: f64, v: f64| node(origin.0 + u * c - v * s, origin.1 + u * s + v * c);
        (0..n)
            .map(|i| {
                let u = i as f64 * (w + 3.0);
                let mut tags = HashMap::new();
                tags.insert("building".to_string(), "yes".to_string());
                ProcessedElement::Way(crate::osm_parser::ProcessedWay {
                    id: i as u64,
                    nodes: vec![
                        at(u, 0.0),
                        at(u + w, 0.0),
                        at(u + w, d),
                        at(u, d),
                        at(u, 0.0),
                    ],
                    tags,
                })
            })
            .collect()
    }

    #[test]
    fn a_grid_at_an_angle_is_turned_onto_the_blocks() {
        for deg in [33.0, 57.0, 12.0, -21.0, 44.0] {
            let elements = block_row(deg, 40, 18.0, 12.0, (500.0, 500.0));
            let a = street_alignment(&elements).unwrap();
            let folded = (deg + a.rotation).rem_euclid(90.0);
            assert!(
                folded.min(90.0 - folded) < 0.4,
                "{deg}: turned {}",
                a.rotation
            );
            assert!(a.rotation.abs() <= 45.0);
            // Corners rounded to whole blocks leave some short walls a few degrees off.
            assert!(a.after > a.before + 0.4, "{a:?}");
        }
    }

    #[test]
    fn the_turn_matches_the_world_rotation() {
        let elements = block_row(33.0, 40, 18.0, 12.0, (500.0, 500.0));
        let turn = street_alignment(&elements).unwrap().rotation;
        let bbox = XZBBox::rect_from_xz_lengths(1000.0, 1000.0).unwrap();
        let ProcessedElement::Way(w) = &elements[0] else {
            unreachable!()
        };
        let (a, b) = (&w.nodes[0], &w.nodes[1]);
        let p = rotate_xz_point(a.x, a.z, turn, &bbox);
        let q = rotate_xz_point(b.x, b.z, turn, &bbox);
        assert!((p.1 - q.1).abs() <= 1, "{p:?} {q:?}");
    }

    #[test]
    fn the_longer_of_two_grids_wins() {
        let mut elements = block_row(30.0, 60, 18.0, 12.0, (300.0, 300.0));
        elements.extend(block_row(10.0, 20, 18.0, 12.0, (300.0, 900.0)));
        let a = street_alignment(&elements).unwrap();
        assert!((a.rotation + 30.0).abs() < 1.0, "{a:?}");
    }

    #[test]
    fn a_town_already_on_the_grid_is_left_alone() {
        let elements = block_row(1.5, 40, 18.0, 12.0, (500.0, 500.0));
        let a = street_alignment(&elements).unwrap();
        assert_eq!(a.rotation, 0.0);
    }

    #[test]
    fn a_few_sheds_say_nothing() {
        let elements = block_row(30.0, 3, 6.0, 5.0, (500.0, 500.0));
        assert_eq!(street_alignment(&elements), None);
    }
}
