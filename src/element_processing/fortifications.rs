//! Town walls and gates: solid masonry with merlons and arched passages.

use crate::block_definitions::*;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode};
use crate::world_editor::WorldEditor;
use fnv::{FnvHashMap, FnvHashSet};
use std::collections::{HashMap, VecDeque};

/// `historic` and `wall` values of town and castle defences.
const DEFENCES: &[&str] = &["citywalls", "city_wall", "castle_wall"];

/// Set on a plain building or tower that stands in the grounds of a castle or fort.
pub(crate) const CASTLE_GROUNDS_TAG: &str = "arnis:castle_grounds";

/// `historic` values of the grounds a castle's buildings stand in.
const CASTLE_GROUNDS: &[&str] = &["castle", "fort", "fortress"];

/// Marks the buildings mapped with no type of their own, and the towers, that stand in the
/// grounds of a castle or fort: they are its palace, keep and gatehouse.
pub fn mark_castle_grounds(elements: &mut [ProcessedElement]) {
    use crate::element_processing::area_layout::{interior_point, point_in_rings};
    let is_grounds = |tags: &HashMap<String, String>| {
        tag(tags, "historic").is_some_and(|h| CASTLE_GROUNDS.contains(&h))
            && !tags.contains_key("building")
            && !tags.contains_key("building:part")
    };
    type Grounds = ((i32, i32, i32, i32), Vec<Vec<ProcessedNode>>);
    let mut grounds: Vec<Grounds> = Vec::new();
    for e in elements.iter() {
        let rings: Vec<Vec<ProcessedNode>> = match e {
            ProcessedElement::Way(w) if is_grounds(&w.tags) && closed(&w.nodes) => {
                vec![w.nodes.clone()]
            }
            ProcessedElement::Relation(r) if is_grounds(&r.tags) => r
                .members
                .iter()
                .filter(|m| m.role != ProcessedMemberRole::Part)
                .map(|m| m.way.nodes.clone())
                .collect(),
            _ => continue,
        };
        let mut nodes = rings.iter().flatten();
        let Some(first) = nodes.next() else { continue };
        let bounds = nodes.fold((first.x, first.z, first.x, first.z), |b, n| {
            (b.0.min(n.x), b.1.min(n.z), b.2.max(n.x), b.3.max(n.z))
        });
        grounds.push((bounds, rings));
    }
    if grounds.is_empty() {
        return;
    }
    let inside = |p: (f64, f64)| {
        grounds.iter().any(|(b, rings)| {
            p.0 >= b.0 as f64
                && p.1 >= b.1 as f64
                && p.0 <= b.2 as f64
                && p.1 <= b.3 as f64
                && point_in_rings(&rings.iter().map(Vec::as_slice).collect::<Vec<_>>(), p)
        })
    };
    for e in elements.iter_mut() {
        let ProcessedElement::Way(w) = e else {
            continue;
        };
        let kind = tag(&w.tags, "building").or_else(|| tag(&w.tags, "building:part"));
        let plain = kind == Some("yes");
        let tower = tag(&w.tags, "man_made") == Some("tower") || kind == Some("tower");
        if (plain || tower) && closed(&w.nodes) && interior_point(&w.nodes).is_some_and(inside) {
            w.tags
                .insert(CASTLE_GROUNDS_TAG.to_string(), "yes".to_string());
        }
    }
}

fn closed(nodes: &[ProcessedNode]) -> bool {
    nodes.len() >= 4 && nodes.first().map(|n| (n.x, n.z)) == nodes.last().map(|n| (n.x, n.z))
}

/// Clear height of a passage at its sides, in metres; it rises by an arch towards the middle.
const PASSAGE_M: f64 = 4.0;

/// Blocks a passage's arch rises above its sides.
const ARCH_RISE: i32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Wall,
    Gate,
}

fn tag<'a>(tags: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    tags.get(key).map(String::as_str)
}

fn defence(tags: &HashMap<String, String>) -> bool {
    tag(tags, "historic").is_some_and(|h| DEFENCES.contains(&h))
        || tag(tags, "wall").is_some_and(|w| DEFENCES.contains(&w))
        || tag(tags, "barrier") == Some("city_wall")
}

/// Whether a barrier line is a town or castle wall.
pub(crate) fn line_wall(tags: &HashMap<String, String>) -> bool {
    matches!(tag(tags, "barrier"), Some("city_wall" | "wall")) && defence(tags)
}

/// Whether a building is a wall or a town gate; a gate, tower or covered wall walk mapped with
/// a roof stays a building under it.
pub(crate) fn kind(tags: &HashMap<String, String>) -> Option<Kind> {
    if tag(tags, "roof:shape").is_some_and(|s| s != "flat") {
        return None;
    }
    let gate = tag(tags, "historic") == Some("city_gate")
        || (tag(tags, "building") == Some("gatehouse") && defence(tags));
    if gate {
        return Some(Kind::Gate);
    }
    (tag(tags, "building") == Some("wall") || defence(tags)).then_some(Kind::Wall)
}

/// Whether merlons crown it: town walls and gates, unless mapped in 3D with parts of their own.
pub(crate) fn crenellated(tags: &HashMap<String, String>, kind: Kind) -> bool {
    (kind == Kind::Gate || defence(tags)) && !tags.contains_key("building:part")
}

/// A wall or gate to build: its cells, how high it stands and where ways pass through it.
pub(crate) struct Mass<'a> {
    pub cells: &'a [(i32, i32)],
    pub block: Block,
    /// Blocks from its foot to the top of the wall walk.
    pub height: i32,
    /// Y a gate or raised part stands on throughout; None follows the ground.
    pub base: Option<i32>,
    /// Raised off the ground, with nothing under it.
    pub raised: bool,
    pub crenellated: bool,
    /// Cells a way passes through.
    pub passages: FnvHashSet<(i32, i32)>,
    /// Cells standing on something other than the ground, at the Y of its top.
    pub footings: FnvHashMap<(i32, i32), i32>,
    /// Cells of other masonry it meets there, which make no face of its own.
    pub abutting: &'a dyn Fn(i32, i32) -> bool,
    pub scale: f64,
}

/// Builds a wall or gate as solid masonry: no windows, doors, rooms or roof.
pub(crate) fn generate(editor: &mut WorldEditor, mass: &Mass) {
    let own: FnvHashSet<(i32, i32)> = mass.cells.iter().copied().collect();
    // A raised mass already spans what passes under it.
    let arch = if mass.raised {
        FnvHashMap::default()
    } else {
        passage_clearances(&own, &mass.passages, mass.height, mass.scale)
    };
    let open = |c: &(i32, i32)| arch.get(c).is_some_and(|&clear| clear >= mass.height);
    let faces: Vec<(i32, i32)> = mass
        .cells
        .iter()
        .copied()
        .filter(|&(x, z)| {
            [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dz)| {
                let n = (x + dx, z + dz);
                !own.contains(&n) && !(mass.abutting)(n.0, n.1)
            })
        })
        .filter(|c| !open(c))
        .collect();
    let merlons = if mass.crenellated && mass.height >= 3 {
        every_other(&faces)
    } else {
        FnvHashSet::default()
    };
    for &(x, z) in mass.cells {
        let ground = mass
            .footings
            .get(&(x, z))
            .copied()
            .unwrap_or_else(|| editor.get_absolute_y(x, 0, z));
        let base = mass.base.unwrap_or(ground);
        let top = base + mass.height;
        let from = match arch.get(&(x, z)) {
            Some(&clear) => base.max(ground) + clear + 1,
            None if mass.raised => base + 1,
            None => ground.min(base) + 1,
        };
        for y in from..=top {
            editor.set_block_absolute(mass.block, x, y, z, None, None);
        }
        if merlons.contains(&(x, z)) {
            editor.set_block_absolute(mass.block, x, top + 1, z, None, None);
        }
    }
}

/// One cell in two along each run of face cells, whichever way the face runs.
fn every_other(faces: &[(i32, i32)]) -> FnvHashSet<(i32, i32)> {
    let set: FnvHashSet<(i32, i32)> = faces.iter().copied().collect();
    let mut starts = faces.to_vec();
    starts.sort_unstable();
    let mut steps: FnvHashMap<(i32, i32), u32> = FnvHashMap::default();
    let mut queue: VecDeque<(i32, i32)> = VecDeque::new();
    for start in starts {
        if steps.contains_key(&start) {
            continue;
        }
        steps.insert(start, 0);
        queue.push_back(start);
        while let Some(c) = queue.pop_front() {
            let d = steps[&c];
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let n = (c.0 + dx, c.1 + dz);
                    if set.contains(&n) && !steps.contains_key(&n) {
                        steps.insert(n, d + 1);
                        queue.push_back(n);
                    }
                }
            }
        }
    }
    steps
        .into_iter()
        .filter(|&(_, d)| d % 2 == 0)
        .map(|(c, _)| c)
        .collect()
}

/// Clear height over each passage cell, arched from the jambs up and under a two-block lintel.
fn passage_clearances(
    own: &FnvHashSet<(i32, i32)>,
    passages: &FnvHashSet<(i32, i32)>,
    height: i32,
    scale: f64,
) -> FnvHashMap<(i32, i32), i32> {
    let mut steps: FnvHashMap<(i32, i32), i32> = FnvHashMap::default();
    let mut queue: VecDeque<(i32, i32)> = VecDeque::new();
    for &c in passages.iter().filter(|c| own.contains(c)) {
        let jamb = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dz)| {
            let n = (c.0 + dx, c.1 + dz);
            own.contains(&n) && !passages.contains(&n)
        });
        if jamb {
            steps.insert(c, 0);
            queue.push_back(c);
        }
    }
    while let Some(c) = queue.pop_front() {
        let d = steps[&c];
        if d >= ARCH_RISE {
            continue;
        }
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let n = (c.0 + dx, c.1 + dz);
            if own.contains(&n) && passages.contains(&n) && !steps.contains_key(&n) {
                steps.insert(n, d + 1);
                queue.push_back(n);
            }
        }
    }
    let sides = ((PASSAGE_M * scale).round() as i32).max(2);
    passages
        .iter()
        .filter(|c| own.contains(c))
        .map(|&c| {
            let rise = steps.get(&c).copied().unwrap_or(ARCH_RISE);
            let clear = sides + rise;
            // Too low a wall for a lintel is open to the sky there.
            let clear = if height - clear >= 2 {
                clear
            } else if height - sides >= 2 {
                height - 2
            } else {
                height
            };
            (c, clear)
        })
        .collect()
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

    // The palace, keep and towers in a castle's grounds are the castle's; a typed house or
    // a building beyond the grounds is not.
    #[test]
    fn plain_buildings_in_castle_grounds_are_marked() {
        use crate::element_processing::building_test_support::rect_way;
        let mut elements: Vec<ProcessedElement> = [
            rect_way(1, 0, 0, 100, 100, &[("historic", "castle")]),
            rect_way(
                2,
                10,
                10,
                30,
                30,
                &[("building", "yes"), ("historic", "yes")],
            ),
            rect_way(3, 40, 40, 46, 46, &[("building:part", "yes")]),
            rect_way(4, 50, 10, 56, 16, &[("building", "tower")]),
            rect_way(5, 60, 60, 70, 70, &[("building", "house")]),
            rect_way(6, 150, 10, 170, 30, &[("building", "yes")]),
        ]
        .into_iter()
        .map(ProcessedElement::Way)
        .collect();
        mark_castle_grounds(&mut elements);
        let marked: Vec<u64> = elements
            .iter()
            .filter(|e| e.tags().contains_key(CASTLE_GROUNDS_TAG))
            .map(ProcessedElement::id)
            .collect();
        assert_eq!(marked, [2, 3, 4]);
    }

    #[test]
    fn walls_and_gates_are_told_from_houses() {
        // The Temple Mount's walls, Lions' Gate and the Kremlin wall's base in Moscow.
        assert_eq!(kind(&tags(&[("building", "wall")])), Some(Kind::Wall));
        assert_eq!(
            kind(&tags(&[("building", "yes"), ("historic", "city_gate")])),
            Some(Kind::Gate)
        );
        assert_eq!(
            kind(&tags(&[("building", "yes"), ("historic", "citywalls")])),
            Some(Kind::Wall)
        );
        assert_eq!(kind(&tags(&[("building", "yes")])), None);
        assert_eq!(
            kind(&tags(&[("building", "yes"), ("historic", "building")])),
            None
        );
        // A gatehouse at a factory gate, and a gate tower with its spire.
        assert_eq!(kind(&tags(&[("building", "gatehouse")])), None);
        assert_eq!(
            kind(&tags(&[
                ("building", "tower"),
                ("historic", "city_gate"),
                ("roof:shape", "pyramidal")
            ])),
            None
        );
        // A wall tower under its cone stays a building too, and so does Rothenburg's covered
        // wall walk.
        assert_eq!(
            kind(&tags(&[
                ("building", "tower"),
                ("historic", "citywalls"),
                ("roof:shape", "conical")
            ])),
            None
        );
        assert_eq!(
            kind(&tags(&[
                ("building", "wall"),
                ("historic", "citywalls"),
                ("roof:shape", "gabled")
            ])),
            None
        );
        let base = tags(&[("building", "wall"), ("building:part", "base")]);
        assert!(!crenellated(&base, Kind::Wall));
        assert!(!crenellated(&tags(&[("building", "wall")]), Kind::Wall));
        assert!(crenellated(
            &tags(&[("building", "yes"), ("historic", "citywalls")]),
            Kind::Wall
        ));
        // Lines: Jerusalem's walls, and a garden wall of stone.
        assert!(line_wall(&tags(&[("barrier", "city_wall")])));
        assert!(line_wall(&tags(&[
            ("barrier", "wall"),
            ("historic", "citywalls")
        ])));
        assert!(!line_wall(&tags(&[("barrier", "wall"), ("wall", "stone")])));
        assert!(!line_wall(&tags(&[
            ("barrier", "fence"),
            ("historic", "citywalls")
        ])));
    }

    #[test]
    fn merlons_alternate_along_straight_and_diagonal_faces() {
        let row: Vec<(i32, i32)> = (0..10).map(|x| (x, 0)).collect();
        let diagonal: Vec<(i32, i32)> = (0..10).map(|k| (k, 9 - k)).collect();
        for face in [row, diagonal] {
            let merlons = every_other(&face);
            assert_eq!(merlons.len(), 5);
            assert!(face
                .windows(2)
                .all(|w| merlons.contains(&w[0]) != merlons.contains(&w[1])));
        }
    }

    #[test]
    fn a_passage_is_arched_under_a_lintel() {
        // A wall three cells thick along z with a way five cells wide through it.
        let own: FnvHashSet<(i32, i32)> =
            (0..9).flat_map(|x| (0..3).map(move |z| (x, z))).collect();
        let passages: FnvHashSet<(i32, i32)> =
            (2..7).flat_map(|x| (0..3).map(move |z| (x, z))).collect();
        let clear = passage_clearances(&own, &passages, 12, 1.0);
        assert_eq!(clear[&(2, 1)], 4);
        assert_eq!(clear[&(3, 1)], 5);
        assert_eq!(clear[&(4, 1)], 6);
        assert_eq!(clear[&(6, 1)], 4);
        // A low wall leaves two blocks over the way, or none.
        assert_eq!(passage_clearances(&own, &passages, 7, 1.0)[&(4, 1)], 5);
        assert_eq!(passage_clearances(&own, &passages, 5, 1.0)[&(4, 1)], 5);
    }
}
