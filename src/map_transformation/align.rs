//! "Align to Streets": the world turn that lays most building walls along the block grid.

use rayon::prelude::*;

use crate::args::Args;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode};
use crate::progress::{emit_gui_progress_update, MESSAGE_ONLY};

const BIN_DEG: f64 = 0.5;
const BINS: usize = 180;
/// A wall this close to an axis stands flat in blocks: one block of drift over about 20.
const ON_GRID_DEG: f64 = 3.0;
const ON_GRID_BINS: usize = 6;
/// Window the strongest direction settles in: holds a grid spread by rounding, not a second grid.
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
    /// The clockwise turn in degrees, within ±45, that lines the strongest direction up.
    pub turn: f64,
    /// `turn` where it is worth it, else 0.
    pub rotation: f64,
    /// Share of the measured length on the grid unturned and after `turn`.
    pub before: f64,
    pub after: f64,
    pub source: Source,
}

/// Wall length per direction bin, with its direction at 4x the angle to place the winner between bins.
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

    fn add_line(&mut self, nodes: &[ProcessedNode], area: &XZBBox) {
        for pair in nodes.windows(2) {
            let (dx, dz) = (
                i64::from(pair[1].x - pair[0].x),
                i64::from(pair[1].z - pair[0].z),
            );
            let mid = XZPoint::new((pair[0].x + pair[1].x) / 2, (pair[0].z + pair[1].z) / 2);
            if dx * dx + dz * dz < MIN_PIECE * MIN_PIECE || !area.contains(&mid) {
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

    /// The main direction in [0, 90): the smoothed peak, recentred on its weighted mean until it settles.
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

    /// Share of the length within `tolerance` degrees of an axis once turned by `rotation`.
    fn on_grid(&self, rotation: f64, tolerance: f64) -> f64 {
        if self.total <= 0.0 {
            return 0.0;
        }
        let on: f64 = (0..BINS)
            .filter(|&i| {
                let deg = ((i as f64 + 0.5) * BIN_DEG + rotation).rem_euclid(90.0);
                deg.min(90.0 - deg) <= tolerance
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

/// The turn that puts most building walls inside `area` on the block grid; None with too few.
pub fn street_alignment(elements: &[ProcessedElement], area: &XZBBox) -> Option<Alignment> {
    // Boxed, so the fold moves pointers rather than the histograms.
    let fresh = || (Box::new(Directions::new()), Box::new(Directions::new()));
    let (walls, roads) = elements
        .par_iter()
        .fold(fresh, |(mut walls, mut roads), e| {
            match e {
                ProcessedElement::Way(w) if is_building(&w.tags) => walls.add_line(&w.nodes, area),
                ProcessedElement::Way(w) if is_road(&w.tags) => roads.add_line(&w.nodes, area),
                ProcessedElement::Relation(r) if is_building(&r.tags) => {
                    for m in &r.members {
                        if m.role == ProcessedMemberRole::Outer {
                            walls.add_line(&m.way.nodes, area);
                        }
                    }
                }
                _ => {}
            }
            (walls, roads)
        })
        .reduce(fresh, |a, b| {
            (Box::new(a.0.merge(&b.0)), Box::new(a.1.merge(&b.1)))
        });
    let (dirs, source) = if walls.total >= MIN_LENGTH {
        (*walls, Source::Buildings)
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
    // A small turn is judged on a tighter grid, or walls a couple of degrees off count as on it.
    let tolerance = (turn.abs() / 2.0).clamp(1.0, ON_GRID_DEG);
    let (before, after) = (dirs.on_grid(0.0, tolerance), dirs.on_grid(turn, tolerance));
    let worth = after - before >= MIN_GAIN && turn.abs() >= MIN_TURN_DEG;
    Some(Alignment {
        turn,
        rotation: if worth { turn } else { 0.0 },
        before,
        after,
        source,
    })
}

/// With `--align-to-streets`, sets `args.rotation` from the map in `area` and reports the choice.
pub fn settle(args: &mut Args, elements: &[ProcessedElement], area: &XZBBox) {
    if !args.align_to_streets {
        return;
    }
    if args.skip_osm() {
        return no_map(args);
    }
    let pct = |share: f64| (share * 100.0).round();
    // The per-position climate does not follow a turned ground (the CLI refuses the pair).
    let (full, short) = if args.climate_mode == crate::climate_field::ClimateMode::PerPosition {
        (
            "Align to streets: not with the per-position climate, not turned".to_string(),
            "Not turned: per-position climate".to_string(),
        )
    } else {
        match street_alignment(elements, area) {
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

/// Where no map is loaded (off Earth, or below the object scale) there is nothing to align to.
pub fn no_map(args: &Args) {
    if args.align_to_streets {
        println!("Align to streets: no buildings or streets are loaded at this scale, not turned");
        emit_gui_progress_update(MESSAGE_ONLY, "Not turned: no map at this scale");
    }
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

    fn everywhere() -> XZBBox {
        XZBBox::rect_from_min_max(-5000, -5000, 5000, 5000).unwrap()
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
            let a = street_alignment(&elements, &everywhere()).unwrap();
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
        let turn = street_alignment(&elements, &everywhere()).unwrap().rotation;
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
        let a = street_alignment(&elements, &everywhere()).unwrap();
        assert!((a.rotation + 30.0).abs() < 1.0, "{a:?}");
    }

    #[test]
    fn a_town_already_on_the_grid_is_left_alone() {
        let elements = block_row(1.5, 40, 18.0, 12.0, (500.0, 500.0));
        let a = street_alignment(&elements, &everywhere()).unwrap();
        assert_eq!(a.rotation, 0.0);
    }

    #[test]
    fn a_town_a_few_degrees_off_is_turned() {
        let elements = block_row(2.5, 40, 60.0, 40.0, (500.0, 500.0));
        let a = street_alignment(&elements, &everywhere()).unwrap();
        assert!((a.rotation + 2.5).abs() < 0.6, "{a:?}");
    }

    #[test]
    fn two_equal_grids_half_a_quarter_apart_are_not_turned() {
        let mut elements = block_row(0.0, 40, 18.0, 12.0, (300.0, 300.0));
        elements.extend(block_row(45.0, 40, 18.0, 12.0, (300.0, 1500.0)));
        let a = street_alignment(&elements, &everywhere()).unwrap();
        assert_eq!(a.rotation, 0.0, "{a:?}");
    }

    #[test]
    fn only_walls_inside_the_area_count() {
        let mut elements = block_row(30.0, 20, 18.0, 12.0, (300.0, 300.0));
        elements.extend(block_row(10.0, 60, 18.0, 12.0, (300.0, 2500.0)));
        let area = XZBBox::rect_from_min_max(0, 0, 1200, 1200).unwrap();
        let a = street_alignment(&elements, &area).unwrap();
        assert!((a.rotation + 30.0).abs() < 1.0, "{a:?}");
    }

    #[test]
    fn a_few_sheds_say_nothing() {
        let elements = block_row(30.0, 3, 6.0, 5.0, (500.0, 500.0));
        assert_eq!(street_alignment(&elements, &everywhere()), None);
    }
}
