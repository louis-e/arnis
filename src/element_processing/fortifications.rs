//! Town walls and gates: solid masonry with merlons and arched passages.

use crate::block_definitions::*;
use crate::world_editor::WorldEditor;
use fnv::{FnvHashMap, FnvHashSet};
use std::collections::{HashMap, VecDeque};

/// `historic` and `wall` values of town and castle defences.
const DEFENCES: &[&str] = &["citywalls", "city_wall", "castle_wall"];

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

/// Whether a building is a wall or a town gate; a gate tower mapped with a roof stays a building.
pub(crate) fn kind(tags: &HashMap<String, String>) -> Option<Kind> {
    let gate = tag(tags, "historic") == Some("city_gate")
        || (tag(tags, "building") == Some("gatehouse") && defence(tags));
    if gate {
        return tag(tags, "roof:shape")
            .is_none_or(|s| s == "flat")
            .then_some(Kind::Gate);
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
    pub scale: f64,
}

/// Builds a wall or gate as solid masonry: no windows, doors, rooms or roof.
pub(crate) fn generate(editor: &mut WorldEditor, mass: &Mass) {
    let own: FnvHashSet<(i32, i32)> = mass.cells.iter().copied().collect();
    let arch = passage_clearances(&own, &mass.passages, mass.height, mass.scale);
    let merlon = mass.crenellated && mass.height >= 3;
    for &(x, z) in mass.cells {
        let ground = editor.get_absolute_y(x, 0, z);
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
        let edge = [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .iter()
            .any(|&(dx, dz)| !own.contains(&(x + dx, z + dz)));
        if merlon && edge && (x + z).rem_euclid(2) == 0 {
            editor.set_block_absolute(mass.block, x, top + 1, z, None, None);
        }
    }
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
