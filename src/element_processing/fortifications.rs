//! Town walls and gates: solid masonry with merlons and arched passages.

use crate::block_definitions::*;
use crate::osm_parser::{building_tag, ProcessedElement, ProcessedMemberRole, ProcessedNode};

/// The keys a facade material is mapped under.
const MATERIAL_KEYS: [&str; 4] = [
    "building:material",
    "building:facade:material",
    "facade:material",
    "building:cladding",
];
use crate::world_editor::WorldEditor;
use fnv::{FnvHashMap, FnvHashSet};
use std::collections::{HashMap, VecDeque};

/// `historic` and `wall` values of town and castle defences.
const DEFENCES: &[&str] = &["citywalls", "city_wall", "castle_wall"];

/// Set on a plain building or tower in the grounds of a castle: `fort` where it is built of
/// castle stone, `palace` in the grounds of a palace or manor.
pub(crate) const CASTLE_GROUNDS_TAG: &str = "arnis:castle_grounds";

/// `historic` values of the grounds a castle's buildings stand in.
const CASTLE_GROUNDS: &[&str] = &["castle", "fort", "fortress"];

/// `castle_type` values of a residence rather than a stronghold.
const PALACES: &[&str] = &[
    "palace",
    "stately",
    "manor",
    "schloss",
    "chateau",
    "residence",
];

/// Grounds larger than this hold a town, whose plain buildings are its own, not the castle's.
const MAX_GROUNDS_M2: f64 = 100_000.0;

/// A plain building smaller than this is a kiosk or a shed in the grounds.
const MIN_BUILDING_M2: f64 = 50.0;

/// `tower:type` values of a castle's towers.
const CASTLE_TOWERS: &[&str] = &[
    "defensive",
    "watchtower",
    "observation",
    "lookout",
    "keep",
    "bastion",
    "fortification",
];

/// Facade materials no castle stone stands in for.
const NOT_MASONRY: &[&str] = &[
    "wood",
    "timber",
    "timber_framing",
    "metal",
    "steel",
    "aluminium",
    "aluminum",
    "glass",
    "mirror",
    "plastic",
];

/// `historic` values a building in a fortress town is the fortress's by.
const FORTIFIED: &[&str] = &[
    "castle",
    "fort",
    "fortress",
    "tower",
    "city_gate",
    "citywalls",
    "castle_wall",
];

/// `tourism` values a castle's own buildings carry.
const CASTLE_TOURISM: &[&str] = &["museum", "attraction", "gallery", "viewpoint"];

/// Marks the buildings of a castle mapped as plain buildings in its grounds: its palace, keep,
/// gatehouse and towers. In grounds no larger than a castle's, one listed or marked historic is
/// its own, and so is an unmarked one that is no kiosk, shop or cafe. In a larger castle park
/// only one marked historic is, and in a fortress town only a gate, tower or wall marked as one.
/// None mapped as timber, steel or glass is, nor a tower of another kind such as a belfry.
pub fn mark_castle_grounds(elements: &mut [ProcessedElement], scale: f64) {
    use crate::element_processing::area_layout::{interior_point, point_in_rings};
    let is_grounds = |tags: &HashMap<String, String>| {
        tag(tags, "historic").is_some_and(|h| CASTLE_GROUNDS.contains(&h))
            && !tags.contains_key("building")
            && !tags.contains_key("building:part")
    };
    struct Grounds {
        bounds: (i32, i32, i32, i32),
        rings: Vec<Vec<ProcessedNode>>,
        palace: bool,
        /// A fort or fortress, which may hold a town, rather than a castle.
        fortress: bool,
        /// Square blocks inside; None where the area cuts a ring or the rings make no sense,
        /// and the size is not known.
        area: Option<f64>,
        /// Square blocks of its bounds, which tell nested grounds of unknown size apart.
        extent: f64,
    }
    let m2 = |blocks: f64| blocks / (scale * scale);
    let mut grounds: Vec<Grounds> = Vec::new();
    for e in elements.iter() {
        // Outer and inner rings, their pieces joined.
        let (outer, inner): (Vec<Vec<ProcessedNode>>, Vec<Vec<ProcessedNode>>) = match e {
            ProcessedElement::Way(w) if is_grounds(&w.tags) && closed(&w.nodes) => {
                (vec![w.nodes.clone()], Vec::new())
            }
            ProcessedElement::Relation(r) if is_grounds(&r.tags) => {
                let rings = |role: ProcessedMemberRole| {
                    let mut rings: Vec<Vec<ProcessedNode>> = r
                        .members
                        .iter()
                        .filter(|m| m.role == role)
                        .map(|m| m.way.nodes.clone())
                        .collect();
                    crate::element_processing::merge_way_segments(&mut rings);
                    rings
                };
                (
                    rings(ProcessedMemberRole::Outer),
                    rings(ProcessedMemberRole::Inner),
                )
            }
            _ => continue,
        };
        let mut nodes = outer.iter().flatten();
        let Some(first) = nodes.next() else { continue };
        let bounds = nodes.fold((first.x, first.z, first.x, first.z), |b, n| {
            (b.0.min(n.x), b.1.min(n.z), b.2.max(n.x), b.3.max(n.z))
        });
        // A ring the area cuts carries the clipper's nodes, or stays open.
        let whole = outer.iter().chain(&inner).all(|r| {
            nearly_closed(r) && !r.iter().any(|n| crate::clipping::is_invented_node_id(n.id))
        });
        let area = whole
            .then(|| {
                outer.iter().map(|r| ring_area(r)).sum::<f64>()
                    - inner.iter().map(|r| ring_area(r)).sum::<f64>()
            })
            .filter(|a| *a > 0.0);
        let tags = e.tags();
        grounds.push(Grounds {
            bounds,
            rings: outer.into_iter().chain(inner).collect(),
            palace: tag(tags, "castle_type").is_some_and(|t| PALACES.contains(&t)),
            fortress: tag(tags, "historic") != Some("castle"),
            area,
            extent: f64::from(bounds.2 - bounds.0) * f64::from(bounds.3 - bounds.1),
        });
    }
    if grounds.is_empty() {
        return;
    }
    // The smallest grounds around a point: a castle within a fortress is the castle's.
    let around = |p: (f64, f64)| {
        grounds
            .iter()
            .filter(|g| {
                let b = g.bounds;
                p.0 >= b.0 as f64
                    && p.1 >= b.1 as f64
                    && p.0 <= b.2 as f64
                    && p.1 <= b.3 as f64
                    && point_in_rings(&g.rings.iter().map(Vec::as_slice).collect::<Vec<_>>(), p)
            })
            .min_by(|a, b| {
                let key = |g: &Grounds| (g.area.unwrap_or(f64::MAX), g.extent);
                let (ka, kb) = (key(a), key(b));
                ka.0.total_cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
            })
    };
    for e in elements.iter_mut() {
        let ProcessedElement::Way(w) = e else {
            continue;
        };
        let tags = &w.tags;
        let kind = tag(tags, "building").or_else(|| tag(tags, "building:part"));
        // A keep, a wall tower or a lookout; not a mast, a belfry or a minaret.
        let values = |v: &str| {
            v.split(';')
                .map(|v| v.trim().to_ascii_lowercase())
                .collect::<Vec<_>>()
        };
        let castle_tower = tag(tags, "tower:type").is_none_or(|t| {
            values(t)
                .iter()
                .any(|v| CASTLE_TOWERS.contains(&v.as_str()))
        });
        let tower = tag(tags, "man_made") == Some("tower") || kind == Some("tower");
        // One mapped as timber, steel or glass is built so.
        let other_material = MATERIAL_KEYS
            .iter()
            .filter_map(|k| tag(tags, k))
            .any(|m| values(m).iter().any(|v| NOT_MASONRY.contains(&v.as_str())));
        if !(tower || kind == Some("yes")) || !castle_tower || other_material || !closed(&w.nodes) {
            continue;
        }
        let Some(g) = interior_point(&w.nodes).and_then(around) else {
            continue;
        };
        // A part answers for its building. A castle's palace is often its museum.
        let marked = ["historic", "heritage"]
            .iter()
            .any(|k| building_tag(tags, k).is_some_and(|v| v != "no"));
        let other_use = ["amenity", "shop", "office", "craft"]
            .iter()
            .any(|k| building_tag(tags, k).is_some())
            || building_tag(tags, "tourism").is_some_and(|t| !CASTLE_TOURISM.contains(&t));
        let fortified = building_tag(tags, "historic").is_some_and(|h| FORTIFIED.contains(&h))
            || tag(tags, "tower:type") == Some("defensive");
        // Grounds of a castle's size lend themselves to all its plain buildings, a castle's
        // park to those marked historic, a fortress town to its gates, towers and walls.
        let own = if g.area.is_some_and(|a| m2(a) <= MAX_GROUNDS_M2) {
            marked || (!other_use && (tower || m2(ring_area(&w.nodes)) >= MIN_BUILDING_M2))
        } else if g.fortress {
            fortified
        } else {
            marked || fortified
        };
        if own {
            let value = if g.palace { "palace" } else { "fort" };
            w.tags
                .insert(CASTLE_GROUNDS_TAG.to_string(), value.to_string());
        }
    }
}

fn closed(nodes: &[ProcessedNode]) -> bool {
    nodes.len() >= 4 && nodes.first().map(|n| (n.x, n.z)) == nodes.last().map(|n| (n.x, n.z))
}

/// Closed, or with ends a block apart, as joined pieces of a ring may be.
fn nearly_closed(nodes: &[ProcessedNode]) -> bool {
    match (nodes.first(), nodes.last()) {
        (Some(a), Some(b)) => nodes.len() >= 4 && (a.x - b.x).abs() <= 1 && (a.z - b.z).abs() <= 1,
        _ => false,
    }
}

/// Shoelace area of a ring, an almost closed one closed by the step between its ends.
fn ring_area(nodes: &[ProcessedNode]) -> f64 {
    let twice: f64 = nodes
        .iter()
        .zip(nodes.iter().cycle().skip(1))
        .map(|(a, b)| a.x as f64 * b.z as f64 - b.x as f64 * a.z as f64)
        .sum();
    twice.abs() / 2.0
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

    // The palace, keep and towers in a castle's grounds are the castle's; a typed house, a
    // kiosk, a cafe or a building beyond the grounds is not. A palace's grounds lend no stone,
    // and grounds the size of a town lend nothing to its plain buildings.
    #[test]
    fn plain_buildings_in_castle_grounds_are_marked() {
        use crate::element_processing::building_test_support::rect_way;
        let marks = |grounds: &[(&str, &str)], side: i32| -> Vec<(u64, String)> {
            let mut elements: Vec<ProcessedElement> = [
                rect_way(1, 0, 0, side, side, grounds),
                rect_way(
                    2,
                    10,
                    10,
                    30,
                    30,
                    &[("building", "yes"), ("historic", "yes")],
                ),
                rect_way(3, 40, 40, 50, 50, &[("building:part", "yes")]),
                rect_way(4, 50, 10, 56, 16, &[("building", "tower")]),
                rect_way(5, 60, 60, 70, 70, &[("building", "house")]),
                rect_way(6, 72, 72, 76, 76, &[("building", "yes")]),
                rect_way(
                    7,
                    80,
                    10,
                    92,
                    22,
                    &[("building", "yes"), ("amenity", "cafe")],
                ),
                rect_way(8, side + 50, 10, side + 70, 30, &[("building", "yes")]),
                rect_way(
                    9,
                    80,
                    30,
                    92,
                    42,
                    &[("building:part", "yes"), ("arnis:outline:amenity", "cafe")],
                ),
                rect_way(
                    10,
                    80,
                    50,
                    84,
                    54,
                    &[("man_made", "tower"), ("tower:type", "communication")],
                ),
                rect_way(
                    11,
                    80,
                    60,
                    84,
                    64,
                    &[("man_made", "tower"), ("tower:type", "observation")],
                ),
                rect_way(
                    12,
                    20,
                    60,
                    40,
                    80,
                    &[
                        ("building:part", "yes"),
                        ("arnis:outline:tourism", "museum"),
                    ],
                ),
                rect_way(
                    13,
                    88,
                    60,
                    92,
                    64,
                    &[("man_made", "tower"), ("building:material", "wood")],
                ),
                rect_way(
                    14,
                    88,
                    70,
                    92,
                    74,
                    &[("man_made", "tower"), ("tower:type", "bell_tower")],
                ),
                rect_way(
                    15,
                    40,
                    80,
                    50,
                    90,
                    &[
                        ("building", "yes"),
                        ("man_made", "tower"),
                        ("tower:type", "bell_tower"),
                        ("historic", "yes"),
                    ],
                ),
                rect_way(
                    16,
                    55,
                    80,
                    70,
                    95,
                    &[("building", "yes"), ("building:cladding", "stone;timber")],
                ),
            ]
            .into_iter()
            .map(ProcessedElement::Way)
            .collect();
            mark_castle_grounds(&mut elements, 1.0);
            elements
                .iter()
                .filter_map(|e| Some((e.id(), e.tags().get(CASTLE_GROUNDS_TAG)?.clone())))
                .collect()
        };
        let fort = |ids: &[u64], value: &str| -> Vec<(u64, String)> {
            ids.iter().map(|&id| (id, value.to_string())).collect()
        };
        assert_eq!(
            marks(&[("historic", "castle")], 100),
            fort(&[2, 3, 4, 11, 12], "fort")
        );
        assert_eq!(
            marks(&[("historic", "castle"), ("castle_type", "stately")], 100),
            fort(&[2, 3, 4, 11, 12], "palace")
        );
        // In a castle's park only what is marked historic is the castle's, in a fortress town
        // only a tower or gate marked as one.
        assert_eq!(marks(&[("historic", "castle")], 400), fort(&[2], "fort"));
        assert_eq!(marks(&[("historic", "fortress")], 400), fort(&[], "fort"));
    }

    // A castle's grounds mapped in pieces are measured whole, and a castle within a fortress
    // town is judged by its own grounds.
    #[test]
    fn castle_grounds_are_measured_whole_and_the_smallest_wins() {
        use crate::element_processing::building_test_support::rect_way;
        use crate::osm_parser::{ProcessedMember, ProcessedRelation, ProcessedWay};
        use std::sync::Arc;
        let piece = |id: u64, points: &[(i32, i32)]| {
            let nodes = points
                .iter()
                .enumerate()
                .map(|(i, &(x, z))| ProcessedNode {
                    id: id * 10 + i as u64,
                    tags: HashMap::new(),
                    x,
                    z,
                })
                .collect();
            ProcessedMember {
                role: ProcessedMemberRole::Outer,
                way: Arc::new(ProcessedWay {
                    id,
                    tags: HashMap::new(),
                    nodes,
                }),
            }
        };
        // A 500 m fortress town, its outer ring in two pieces, round a castle of 100 m.
        let fortress = ProcessedRelation {
            id: 1,
            tags: tags(&[("type", "multipolygon"), ("historic", "fortress")]),
            members: vec![
                piece(11, &[(0, 0), (500, 0), (500, 500)]),
                piece(12, &[(500, 500), (0, 500), (0, 0)]),
            ],
        };
        let mut elements = vec![
            ProcessedElement::Relation(fortress),
            ProcessedElement::Way(rect_way(2, 100, 100, 200, 200, &[("historic", "castle")])),
            ProcessedElement::Way(rect_way(3, 120, 120, 140, 140, &[("building", "yes")])),
            ProcessedElement::Way(rect_way(4, 300, 300, 320, 320, &[("building", "yes")])),
        ];
        mark_castle_grounds(&mut elements, 1.0);
        let marked: Vec<u64> = elements
            .iter()
            .filter(|e| e.tags().contains_key(CASTLE_GROUNDS_TAG))
            .map(ProcessedElement::id)
            .collect();
        assert_eq!(marked, [3]);

        // Grounds the area cuts open are of no known size, so a plain building in them is
        // not taken for the castle's.
        let cut = ProcessedRelation {
            id: 5,
            tags: tags(&[("type", "multipolygon"), ("historic", "castle")]),
            members: vec![piece(51, &[(0, 0), (60, 0), (60, 60), (0, 60)])],
        };
        let mut elements = vec![
            ProcessedElement::Relation(cut),
            ProcessedElement::Way(rect_way(6, 10, 10, 30, 30, &[("building", "yes")])),
        ];
        mark_castle_grounds(&mut elements, 1.0);
        assert!(!elements[1].tags().contains_key(CASTLE_GROUNDS_TAG));

        // So are grounds the area clips down to a closed ring of the clipper's nodes.
        let mut clipped = rect_way(7, 0, 0, 60, 60, &[("historic", "fortress")]);
        clipped.nodes[1].id = (1 << 62) + 1;
        let mut elements = vec![
            ProcessedElement::Way(clipped),
            ProcessedElement::Way(rect_way(8, 10, 10, 30, 30, &[("building", "yes")])),
        ];
        mark_castle_grounds(&mut elements, 1.0);
        assert!(!elements[1].tags().contains_key(CASTLE_GROUNDS_TAG));
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
