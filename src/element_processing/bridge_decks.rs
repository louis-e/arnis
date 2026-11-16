//! Railings, gap fills, beams, lamps and piers, laid out per structure at any grid angle.

use crate::block_definitions::*;
use crate::element_processing::bridge_styles::BridgeStyle;
use crate::element_processing::bridges::{
    deck_columns, grade_obstacle, is_non_vehicular_bridge_highway, way_cells, BridgeMemberInfo,
    BridgeStructureMap, BridgeSurfaceMap,
};
use crate::element_processing::connected_blocks::{is_connectable, place_connected};
use crate::element_processing::highways::highway_block_range;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use std::collections::{HashMap, HashSet};

/// Widest gap between decks of one structure that is decked over.
const GAP_FILL_MAX: i32 = 4;
/// Deck height above the ground below which no pier stands.
const PIER_MIN_HEIGHT: i32 = 3;
/// Deck height above water below which no pier stands in it.
const LOW_WATER_PIER: i32 = 6;
/// Cells between lamps along a lit bridge.
const LAMP_SPACING: usize = 16;
/// Least distance between two lamps of one structure.
const LAMP_CLEARANCE: i32 = 8;
/// Bucket side of the index over at-grade ways.
const ROAD_GRID: i32 = 32;
/// Longest way of a structure that may still be varied in look.
const VARIED_MAX_LENGTH: f32 = 60.0;

/// One of 8 looks for a structure, fixed by its lowest way id.
fn variety(seed: u64) -> u64 {
    seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 61
}

/// How a bridge structure is built: its deck edge, railing, beams and piers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeckDesign {
    /// Motorways and trunk roads: concrete edge beams with a concrete parapet.
    Motorway,
    /// Other roads: concrete deck with a steel railing.
    Road,
    /// Footbridges: slim concrete deck with a steel railing.
    Foot,
    /// Footbridges with a glass balustrade.
    Glass,
    /// Masonry curb and balustrade, as on old arch bridges.
    Stone,
    /// Brick curb and balustrade.
    Brick,
    /// Timber deck with a fence.
    Timber,
    /// A boardwalk on its own posts, without a railing.
    Boardwalk,
    /// Walled in by its roof structure.
    Covered,
}

impl DeckDesign {
    /// The design for a structure of `style` made of `ways`.
    pub fn pick<'a>(style: BridgeStyle, ways: impl Iterator<Item = &'a ProcessedWay>) -> Self {
        let ways: Vec<&ProcessedWay> = ways.collect();
        match style {
            BridgeStyle::Boardwalk => return DeckDesign::Boardwalk,
            BridgeStyle::Covered => return DeckDesign::Covered,
            _ => {}
        }
        let tag = |key: &str| {
            ways.iter()
                .find_map(|w| w.tags.get(key).map(String::as_str))
        };
        let vehicular = ways.iter().any(|w| {
            !w.tags
                .get("highway")
                .is_some_and(|h| is_non_vehicular_bridge_highway(h))
        });
        // A steel structure carries a steel railing, whatever its road, age or material.
        if matches!(
            style,
            BridgeStyle::Suspension | BridgeStyle::CableStayed | BridgeStyle::Truss
        ) {
            return if vehicular {
                DeckDesign::Road
            } else {
                DeckDesign::Foot
            };
        }
        let road = || {
            let major = ways.iter().any(|w| {
                matches!(
                    w.tags.get("highway").map(String::as_str),
                    Some("motorway" | "motorway_link" | "trunk" | "trunk_link")
                ) || w.tags.get("motorroad").map(String::as_str) == Some("yes")
            });
            if major {
                DeckDesign::Motorway
            } else if vehicular {
                DeckDesign::Road
            } else {
                DeckDesign::Foot
            }
        };
        match tag("bridge:material").or_else(|| tag("material")) {
            Some("wood" | "timber") => return DeckDesign::Timber,
            Some("stone" | "masonry" | "sandstone" | "granite" | "limestone") => {
                return DeckDesign::Stone
            }
            Some("brick" | "bricks") => return DeckDesign::Brick,
            Some("concrete" | "reinforced_concrete" | "steel" | "metal" | "iron") => return road(),
            _ => {}
        }
        if style == BridgeStyle::Arch {
            return DeckDesign::Stone;
        }
        if !vehicular {
            let wooden = tag("surface") == Some("wood")
                || ways.iter().all(|w| {
                    matches!(
                        w.tags.get("highway").map(String::as_str),
                        Some("path" | "bridleway")
                    ) && w.tags.get("surface").is_none_or(|s| {
                        matches!(
                            s.as_str(),
                            "ground"
                                | "dirt"
                                | "unpaved"
                                | "gravel"
                                | "fine_gravel"
                                | "compacted"
                                | "wood"
                        )
                    })
                });
            if wooden {
                return DeckDesign::Timber;
            }
        }
        // Old bridges, and stepped or setted ones as in old towns, are masonry.
        let old = tag("historic").is_some()
            || tag("heritage").is_some()
            || tag("start_date")
                .and_then(|d| d.get(..4)?.parse::<i32>().ok())
                .is_some_and(|year| year < 1900);
        let masonry = ways.iter().any(|w| {
            w.tags.get("highway").map(String::as_str) == Some("steps")
                || matches!(
                    w.tags.get("surface").map(String::as_str),
                    Some("sett" | "cobblestone" | "unhewn_cobblestone" | "stone")
                )
        });
        if old || masonry {
            return DeckDesign::Stone;
        }
        // Where no tag decides, some short minor bridges are masonry, some footbridges glass.
        let minor = ways.iter().all(|w| {
            matches!(
                w.tags.get("highway").map(String::as_str),
                Some(
                    "residential"
                        | "unclassified"
                        | "service"
                        | "living_street"
                        | "track"
                        | "pedestrian"
                        | "footway"
                        | "path"
                        | "cycleway"
                        | "bridleway"
                )
            )
        });
        let short = ways.iter().all(|w| {
            let length: f32 = w
                .nodes
                .windows(2)
                .map(|p| ((p[1].x - p[0].x) as f32).hypot((p[1].z - p[0].z) as f32))
                .sum();
            length <= VARIED_MAX_LENGTH
        });
        let roll = variety(ways.iter().map(|w| w.id).min().unwrap_or(0));
        match road() {
            DeckDesign::Road if minor && short && roll < 3 => DeckDesign::Stone,
            DeckDesign::Foot if roll < 2 => DeckDesign::Glass,
            design => design,
        }
    }

    /// The slab under the deck surface.
    pub fn slab(self) -> Block {
        match self {
            DeckDesign::Stone => STONE_BRICKS,
            DeckDesign::Brick => BRICK,
            DeckDesign::Timber => SPRUCE_PLANKS,
            DeckDesign::Boardwalk => OAK_PLANKS,
            DeckDesign::Covered => DARK_OAK_PLANKS,
            _ => SMOOTH_STONE,
        }
    }

    /// The edge the railing stands on, level with the deck.
    pub fn curb(self) -> Block {
        self.slab()
    }

    /// The railing on the curb; None where something else walls the deck.
    pub fn parapet(self) -> Option<Block> {
        match self {
            DeckDesign::Motorway => Some(ANDESITE_WALL),
            DeckDesign::Road | DeckDesign::Foot => Some(IRON_BARS),
            DeckDesign::Glass => Some(GLASS_PANE),
            DeckDesign::Stone => Some(STONE_BRICK_WALL),
            DeckDesign::Brick => Some(BRICK_WALL),
            DeckDesign::Timber => Some(SPRUCE_FENCE),
            DeckDesign::Boardwalk | DeckDesign::Covered => None,
        }
    }

    fn pier(self) -> Block {
        match self {
            DeckDesign::Stone => STONE_BRICKS,
            DeckDesign::Brick => BRICK,
            DeckDesign::Timber | DeckDesign::Boardwalk | DeckDesign::Covered => SPRUCE_LOG,
            _ => LIGHT_GRAY_CONCRETE,
        }
    }

    /// Edge beams and pier caps.
    fn beam(self) -> Block {
        self.slab()
    }

    /// A road deck high over the ground stands on a girder deeper than its slab.
    fn girders(self) -> bool {
        matches!(
            self,
            DeckDesign::Motorway | DeckDesign::Road | DeckDesign::Stone | DeckDesign::Brick
        )
    }

    fn lamps(self) -> bool {
        matches!(
            self,
            DeckDesign::Motorway
                | DeckDesign::Road
                | DeckDesign::Foot
                | DeckDesign::Glass
                | DeckDesign::Stone
                | DeckDesign::Brick
        )
    }

    /// A lamp post in the railing's own material, slim among bars and glass.
    fn lamp_pole(self) -> Block {
        match self.parapet() {
            Some(IRON_BARS | GLASS_PANE) => IRON_BARS,
            Some(railing) => railing,
            None => STONE_BRICK_WALL,
        }
    }

    /// Footbridges stand on slimmer piers, never walls.
    fn slender(self) -> bool {
        matches!(
            self,
            DeckDesign::Foot | DeckDesign::Glass | DeckDesign::Timber
        )
    }

    /// Centerline cells between pier stations for a deck `height` above the ground.
    fn pier_spacing(self, style: BridgeStyle, height: i32) -> f32 {
        match style {
            BridgeStyle::Truss => 32.0,
            BridgeStyle::Suspension | BridgeStyle::CableStayed => 24.0,
            _ => match self {
                DeckDesign::Foot | DeckDesign::Glass => {
                    (8.0 + height as f32 * 0.5).clamp(8.0, 20.0)
                }
                DeckDesign::Timber => 6.0,
                _ => (12.0 + height as f32 * 0.6).clamp(12.0, 40.0),
            },
        }
    }
}

/// The parts of a structure one of its member ways draws.
#[derive(Default)]
pub struct DeckParts {
    /// Cells between decks, decked over level with them: (x, z, y).
    pub fills: Vec<(i32, i32, i32)>,
    /// Curb cells along the open deck edges: (x, z, deck y).
    pub rails: Vec<(i32, i32, i32)>,
    /// A second railing block where the curb next to it is a block higher: (x, z, y).
    pub steps: Vec<(i32, i32, i32)>,
    /// Lamp posts on the curb: (x, z, deck y).
    pub lamps: Vec<(i32, i32, i32)>,
    /// Pier columns standing up to (x, z, top y) from the ground.
    pub piers: Vec<(i32, i32, i32)>,
    /// Pier columns that get a footing.
    pub footed: Vec<(i32, i32)>,
    /// Pier caps under the deck: (x, z, y).
    pub caps: Vec<(i32, i32, i32)>,
    /// Girder cells under a high deck: (x, z, y).
    pub girders: Vec<(i32, i32, i32)>,
}

/// A deck cell of one structure.
#[derive(Clone, Copy)]
struct DeckCell {
    y: i32,
    way: u64,
}

/// Where a deck leaves its structure, heading `out`, kept clear of railings for `reach`.
struct OpenEnd {
    at: (f32, f32),
    out: (f32, f32),
    reach: f32,
}

/// At-grade ways as segments in buckets, to keep railings off the roads they meet.
struct GradeRoads {
    segments: Vec<(f32, f32, f32, f32, f32)>,
    buckets: HashMap<(i32, i32), Vec<u32>>,
}

impl GradeRoads {
    fn build(elements: &[ProcessedElement], structures: &BridgeStructureMap, scale: f64) -> Self {
        let mut out = Self {
            segments: Vec::new(),
            buckets: HashMap::new(),
        };
        for elem in elements {
            let ProcessedElement::Way(way) = elem else {
                continue;
            };
            if structures.lookup_ramp(way.id).is_some() || !way.tags.contains_key("highway") {
                continue;
            }
            let Some((half, _, _)) = grade_obstacle(way, scale) else {
                continue;
            };
            let reach = half as f32 + 0.5;
            for pair in way.nodes.windows(2) {
                let seg = out.segments.len() as u32;
                out.segments.push((
                    pair[0].x as f32,
                    pair[0].z as f32,
                    pair[1].x as f32,
                    pair[1].z as f32,
                    reach,
                ));
                let r = half + 1;
                let (x0, x1) = (pair[0].x.min(pair[1].x) - r, pair[0].x.max(pair[1].x) + r);
                let (z0, z1) = (pair[0].z.min(pair[1].z) - r, pair[0].z.max(pair[1].z) + r);
                for bx in x0.div_euclid(ROAD_GRID)..=x1.div_euclid(ROAD_GRID) {
                    for bz in z0.div_euclid(ROAD_GRID)..=z1.div_euclid(ROAD_GRID) {
                        out.buckets.entry((bx, bz)).or_default().push(seg);
                    }
                }
            }
        }
        out
    }

    /// Whether (x, z) lies on an at-grade way's surface, as its square stamps lay it.
    fn covers(&self, x: i32, z: i32) -> bool {
        let Some(list) = self
            .buckets
            .get(&(x.div_euclid(ROAD_GRID), z.div_euclid(ROAD_GRID)))
        else {
            return false;
        };
        let (px, pz) = (x as f32, z as f32);
        list.iter().any(|&i| {
            let (ax, az, bx, bz, reach) = self.segments[i as usize];
            let (dx, dz) = (bx - ax, bz - az);
            let len_sq = dx * dx + dz * dz;
            let t = if len_sq > 0.0 {
                (((px - ax) * dx + (pz - az) * dz) / len_sq).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (cx, cz) = (ax + t * dx, az + t * dz);
            (px - cx).abs().max((pz - cz).abs()) <= reach
        })
    }
}

/// One member's centerline, as the renderer walks it.
struct MemberPath<'a> {
    way: &'a ProcessedWay,
    info: &'a BridgeMemberInfo,
    cells: Vec<(i32, i32)>,
    half: i32,
}

impl MemberPath<'_> {
    fn y(&self, i: usize) -> i32 {
        self.info.y_at(i)
    }

    /// Unit direction of travel around cell `i`.
    fn heading(&self, i: usize) -> (f32, f32) {
        let n = self.cells.len();
        let a = self.cells[i.saturating_sub(3)];
        let b = self.cells[(i + 3).min(n - 1)];
        let (dx, dz) = ((b.0 - a.0) as f32, (b.1 - a.1) as f32);
        let len = dx.hypot(dz);
        if len > 0.0 {
            (dx / len, dz / len)
        } else {
            (1.0, 0.0)
        }
    }
}

/// The parts of every bridge structure, by the member way that draws them.
pub(crate) fn plan(
    elements: &[ProcessedElement],
    structures: &BridgeStructureMap,
    surface: &BridgeSurfaceMap,
    editor: &WorldEditor,
    scale: f64,
) -> HashMap<u64, DeckParts> {
    let mut by_structure: HashMap<usize, Vec<MemberPath>> = HashMap::new();
    for elem in elements {
        let ProcessedElement::Way(way) = elem else {
            continue;
        };
        let Some(info) = structures.lookup_member(way.id) else {
            continue;
        };
        let cells = way_cells(way);
        if cells.is_empty() {
            continue;
        }
        let half = highway_block_range(
            way.tags.get("highway").map(String::as_str).unwrap_or(""),
            &way.tags,
            scale,
        );
        by_structure
            .entry(info.structure)
            .or_default()
            .push(MemberPath {
                way,
                info,
                cells,
                half,
            });
    }
    let mut out: HashMap<u64, DeckParts> = HashMap::new();
    if by_structure.is_empty() {
        return out;
    }
    let roads = GradeRoads::build(elements, structures, scale);
    // Street lamps mapped on the bridges, which the generated ones keep clear of.
    let mapped_lamps: Vec<(i32, i32)> = elements
        .iter()
        .filter_map(|e| match e {
            ProcessedElement::Node(n)
                if n.tags.get("highway").map(String::as_str) == Some("street_lamp") =>
            {
                Some((n.x, n.z))
            }
            _ => None,
        })
        .collect();
    let mut order: Vec<usize> = by_structure.keys().copied().collect();
    order.sort_unstable();
    for s in order {
        let mut paths = by_structure.remove(&s).unwrap_or_default();
        paths.sort_by_key(|p| p.way.id);
        plan_structure(
            &paths,
            structures,
            surface,
            &roads,
            &mapped_lamps,
            editor,
            &mut out,
        );
    }
    out
}

fn plan_structure(
    paths: &[MemberPath],
    structures: &BridgeStructureMap,
    surface: &BridgeSurfaceMap,
    roads: &GradeRoads,
    mapped_lamps: &[(i32, i32)],
    editor: &WorldEditor,
    out: &mut HashMap<u64, DeckParts>,
) {
    let Some(first) = paths.first() else {
        return;
    };
    let info = structures.structure_of(first.info);
    let design = info.design;
    let style = first.info.style;

    // The decks as the renderer stamps them, the higher level where two meet.
    let mut deck: HashMap<(i32, i32), DeckCell> = HashMap::new();
    for p in paths {
        for (column, i) in deck_columns(p.way, p.half) {
            let y = p.y(i);
            deck.entry(column)
                .and_modify(|c| {
                    if y > c.y {
                        *c = DeckCell { y, way: p.way.id };
                    }
                })
                .or_insert(DeckCell { y, way: p.way.id });
        }
    }

    // Narrow gaps between decks running side by side are decked over.
    let other_deck = |x: i32, z: i32, y: i32| {
        surface.deck_near(x, z, y, 2) || surface.rail_deck_near(x, z, y, 2)
    };
    let mut fills: HashMap<(i32, i32), DeckCell> = HashMap::new();
    for along_x in [true, false] {
        let mut lines: HashMap<i32, Vec<(i32, DeckCell)>> = HashMap::new();
        for (&(x, z), &c) in &deck {
            let (line, pos) = if along_x { (z, x) } else { (x, z) };
            lines.entry(line).or_default().push((pos, c));
        }
        for (line, mut cells) in lines {
            cells.sort_unstable_by_key(|c| c.0);
            for pair in cells.windows(2) {
                let ((a, ca), (b, cb)) = (pair[0], pair[1]);
                let gap = b - a - 1;
                if !(1..=GAP_FILL_MAX).contains(&gap) || (ca.y - cb.y).abs() > 1 {
                    continue;
                }
                let y = ca.y.max(cb.y);
                for pos in a + 1..b {
                    let (x, z) = if along_x { (pos, line) } else { (line, pos) };
                    if other_deck(x, z, y) {
                        continue;
                    }
                    let way = if pos - a <= b - pos { ca.way } else { cb.way };
                    fills.entry((x, z)).or_insert(DeckCell { y, way });
                }
            }
        }
    }
    for (&(x, z), &c) in &fills {
        out.entry(c.way).or_default().fills.push((x, z, c.y));
    }
    let mut all = deck.clone();
    all.extend(fills.iter().map(|(&k, &v)| (k, v)));

    // Ends the deck leaves the structure at: no railing in front of them.
    let ends: Vec<OpenEnd> = paths
        .iter()
        .flat_map(|p| {
            let last = p.cells.len() - 1;
            let mut v = Vec::new();
            for (open, i, j) in [
                (p.info.start_is_group_boundary, 0, last.min(3)),
                (p.info.end_is_group_boundary, last, last.saturating_sub(3)),
            ] {
                if !open || i == j {
                    continue;
                }
                let (ex, ez) = (p.cells[i].0 as f32, p.cells[i].1 as f32);
                let (dx, dz) = (ex - p.cells[j].0 as f32, ez - p.cells[j].1 as f32);
                let len = dx.hypot(dz);
                if len > 0.0 {
                    v.push(OpenEnd {
                        at: (ex, ez),
                        out: (dx / len, dz / len),
                        reach: p.half as f32 + 2.0,
                    });
                }
            }
            v
        })
        .collect();
    let beyond_end = |x: i32, z: i32| {
        ends.iter().any(|e| {
            let (rx, rz) = (x as f32 - e.at.0, z as f32 - e.at.1);
            let along = rx * e.out.0 + rz * e.out.1;
            let across = (rx * e.out.1 - rz * e.out.0).abs();
            along > 0.5 && along <= e.reach && across <= e.reach
        })
    };

    // The ring of cells round the decks, each at the level of the deck beside it.
    let mut ring: HashMap<(i32, i32), DeckCell> = HashMap::new();
    for (&(x, z), &c) in &all {
        for dx in -1..=1 {
            for dz in -1..=1 {
                let n = (x + dx, z + dz);
                if all.contains_key(&n) {
                    continue;
                }
                ring.entry(n)
                    .and_modify(|r| {
                        if c.y > r.y {
                            *r = c;
                        }
                    })
                    .or_insert(c);
            }
        }
    }
    let mut rails: HashMap<(i32, i32), DeckCell> = HashMap::new();
    // A boardwalk keeps to its planks.
    let ring = if design == DeckDesign::Boardwalk {
        HashMap::new()
    } else {
        ring
    };
    for (&(x, z), &c) in &ring {
        let ground = editor.get_ground_level(x, z);
        // None on banks, beside other decks, past open ends, or on roads meeting the deck.
        if ground >= c.y
            || other_deck(x, z, c.y)
            || beyond_end(x, z)
            || (c.y - ground <= 2 && roads.covers(x, z))
        {
            continue;
        }
        rails.insert((x, z), c);
    }

    // Lamps on the edges of a lit deck at even steps, each clear of the rest, so a narrow
    // deck takes them on alternate sides and two ways of one bridge never pair them up.
    let mut lamps: Vec<(i32, i32)> = Vec::new();
    if info.lit && design.lamps() {
        let (lo, hi) = all.keys().fold(
            ((i32::MAX, i32::MAX), (i32::MIN, i32::MIN)),
            |(lo, hi), &(x, z)| ((lo.0.min(x), lo.1.min(z)), (hi.0.max(x), hi.1.max(z))),
        );
        let r = LAMP_CLEARANCE;
        let mapped: Vec<(i32, i32)> = mapped_lamps
            .iter()
            .filter(|&&(x, z)| x >= lo.0 - r && x <= hi.0 + r && z >= lo.1 - r && z <= hi.1 + r)
            .copied()
            .collect();
        for p in paths {
            for (k, i) in (LAMP_SPACING / 2..p.cells.len())
                .step_by(LAMP_SPACING)
                .enumerate()
            {
                let (hx, hz) = p.heading(i);
                let (px, pz) = (-hz, hx);
                let reach = p.half as f32 * (px.abs() + pz.abs()) + 1.0;
                let (cx, cz) = p.cells[i];
                let first = if k % 2 == 0 { 1.0f32 } else { -1.0 };
                for sign in [first, -first] {
                    let cell = (
                        (cx as f32 + px * reach * sign).round() as i32,
                        (cz as f32 + pz * reach * sign).round() as i32,
                    );
                    let clear = lamps.iter().chain(&mapped).all(|&(x, z)| {
                        (x - cell.0).pow(2) + (z - cell.1).pow(2) >= LAMP_CLEARANCE.pow(2)
                    });
                    if rails.contains_key(&cell) && clear {
                        lamps.push(cell);
                    }
                }
            }
        }
    }
    let lamps: HashSet<(i32, i32)> = lamps.into_iter().collect();

    for (&(x, z), &c) in &rails {
        let parts = out.entry(c.way).or_default();
        if lamps.contains(&(x, z)) {
            parts.lamps.push((x, z, c.y));
            continue;
        }
        parts.rails.push((x, z, c.y));
        let higher = [(0, -1), (0, 1), (1, 0), (-1, 0)]
            .iter()
            .filter_map(|&(dx, dz)| rails.get(&(x + dx, z + dz)))
            .any(|n| n.y == c.y + 1);
        if higher {
            parts.steps.push((x, z, c.y + 2));
        }
    }

    // The higher a deck stands, the deeper the girder under it.
    if design.girders() {
        for (&(x, z), &c) in all.iter().chain(rails.iter()) {
            let height = c.y - editor.get_ground_level(x, z);
            let depth = match height {
                ..=9 => 0,
                10..=19 => 1,
                _ => 2,
            };
            let parts = out.entry(c.way).or_default();
            for d in 0..depth {
                parts.girders.push((x, z, c.y - 2 - d));
            }
        }
    }

    plan_piers(paths, info, design, style, &all, surface, editor, out);
}

#[allow(clippy::too_many_arguments)]
fn plan_piers(
    paths: &[MemberPath],
    info: &crate::element_processing::bridges::StructureInfo,
    design: DeckDesign,
    style: BridgeStyle,
    all: &HashMap<(i32, i32), DeckCell>,
    surface: &BridgeSurfaceMap,
    editor: &WorldEditor,
    out: &mut HashMap<u64, DeckParts>,
) {
    // Arches stand on their springers, boardwalks on their own posts.
    if matches!(style, BridgeStyle::Arch | BridgeStyle::Boardwalk) {
        return;
    }
    let hangs = |p: &MemberPath, x: i32, z: i32| match &p.info.cable_span {
        Some(span) => span.suspends(x, z),
        None => p.info.cable_pylons.is_some(),
    };
    // Position along the structure, and the stations piers stand at.
    let along = |p: &MemberPath, i: usize| -> f32 {
        match info.axis {
            Some(((ax, az), (dx, dz))) => {
                let (x, z) = p.cells[i];
                (x as f32 - ax) * dx + (z as f32 - az) * dz
            }
            None => i as f32,
        }
    };
    let mut placed: HashSet<(i32, i32)> = HashSet::new();
    let groups: Vec<Vec<&MemberPath>> = if info.axis.is_some() {
        vec![paths.iter().collect()]
    } else {
        paths.iter().map(|p| vec![p]).collect()
    };
    for group in groups {
        let (lo, hi) = group
            .iter()
            .flat_map(|p| (0..p.cells.len()).map(move |i| along(p, i)))
            .fold((f32::MAX, f32::MIN), |(lo, hi), u| (lo.min(u), hi.max(u)));
        let span = hi - lo;
        if span < 4.0 {
            continue;
        }
        // Typical height decides the spacing: the tallest centerline cell.
        let height = group
            .iter()
            .flat_map(|p| {
                p.cells
                    .iter()
                    .enumerate()
                    .map(move |(i, &(x, z))| p.y(i) - editor.get_ground_level(x, z))
            })
            .max()
            .unwrap_or(0);
        let spacing = design.pier_spacing(style, height);
        // A span under twice the spacing is crossed in one.
        let spans = if span < 2.0 * spacing {
            1
        } else {
            (span / spacing).round() as usize
        };
        let step = span / spans as f32;
        for k in 1..spans {
            let target = lo + step * k as f32;
            for p in &group {
                // The member cell nearest the station, then the nearest unblocked one near it.
                let Some(i0) = (0..p.cells.len())
                    .min_by(|&a, &b| {
                        (along(p, a) - target)
                            .abs()
                            .total_cmp(&(along(p, b) - target).abs())
                    })
                    .filter(|&i| (along(p, i) - target).abs() <= 1.0)
                else {
                    continue;
                };
                if placed.contains(&p.cells[i0]) {
                    continue;
                }
                let shift = (step / 3.0) as usize;
                let candidates = (0..=shift).flat_map(|d| {
                    let mut v = vec![i0 + d];
                    if d > 0 && i0 >= d {
                        v.push(i0 - d);
                    }
                    v
                });
                for i in candidates.filter(|&i| i < p.cells.len()) {
                    let (cx, cz) = p.cells[i];
                    let y = p.y(i);
                    let ground = editor.get_ground_level(cx, cz);
                    if y - ground < PIER_MIN_HEIGHT || hangs(p, cx, cz) {
                        break;
                    }
                    // Low over water a pier would wall it off; look along for the bank.
                    let water = editor.is_mapped_water(cx, cz);
                    if water && y - editor.get_water_level(cx, cz) < LOW_WATER_PIER {
                        continue;
                    }
                    let line = station_line(p, i, all);
                    let free = line
                        .iter()
                        .filter(|&&(x, z)| !surface.support_blocked(x, z, y))
                        .count();
                    if free * 2 < line.len() {
                        continue;
                    }
                    lay_pier(p, i, &line, y - ground, water, design, surface, out);
                    placed.extend(line.iter().copied());
                    break;
                }
            }
        }
    }
}

/// The deck cells across the structure at member `p`'s cell `i`, in order.
fn station_line(p: &MemberPath, i: usize, all: &HashMap<(i32, i32), DeckCell>) -> Vec<(i32, i32)> {
    let (hx, hz) = p.heading(i);
    let (px, pz) = (-hz, hx);
    let (cx, cz) = p.cells[i];
    let at = |o: i32| {
        (
            (cx as f32 + px * o as f32).round() as i32,
            (cz as f32 + pz * o as f32).round() as i32,
        )
    };
    let mut left: Vec<(i32, i32)> = Vec::new();
    for o in 1..64 {
        let c = at(o);
        if !all.contains_key(&c) {
            break;
        }
        left.push(c);
    }
    let mut right: Vec<(i32, i32)> = Vec::new();
    for o in 1..64 {
        let c = at(-o);
        if !all.contains_key(&c) {
            break;
        }
        right.push(c);
    }
    let mut line: Vec<(i32, i32)> = right.into_iter().rev().collect();
    line.push((cx, cz));
    line.extend(left);
    crate::element_processing::connected_blocks::four_connected_line(&line)
}

#[allow(clippy::too_many_arguments)]
fn lay_pier(
    p: &MemberPath,
    i: usize,
    line: &[(i32, i32)],
    height: i32,
    water: bool,
    design: DeckDesign,
    surface: &BridgeSurfaceMap,
    out: &mut HashMap<u64, DeckParts>,
) {
    let y = p.y(i);
    let width = line.len();
    let thick: i32 = match height {
        h if design.slender() && h < 16 => 1,
        _ if design.slender() => 2,
        h if h < 10 => 1,
        h if h < 25 => 2,
        h if h < 50 => 3,
        _ => 4,
    };
    // Columns across the line, or a wall pier in water and under tall decks.
    let wall = !design.slender() && width >= 5 && (water || height >= 30);
    let picks: Vec<usize> = if wall {
        (1..width - 1).collect()
    } else if design == DeckDesign::Timber && width >= 3 {
        vec![0, width - 1]
    } else if width <= 4 {
        vec![width / 2]
    } else {
        let (a, b) = (1, width - 2);
        let gaps = ((b - a) as f32 / 8.0).ceil().max(1.0) as usize;
        (0..=gaps).map(|k| a + (b - a) * k / gaps).collect()
    };
    let (hx, hz) = p.heading(i);
    let parts = out.entry(p.way.id).or_default();
    let mut seen: HashSet<(i32, i32)> = HashSet::new();
    // Square columns grow across the line as well as along the deck; a wall only along.
    let widths = if wall { 1 } else { thick };
    for &k in &picks {
        let (lx, lz) = line[k];
        for t in 0..thick {
            let a = (t - (thick - 1) / 2) as f32;
            for w in 0..widths {
                let b = (w - (widths - 1) / 2) as f32;
                let cell = (
                    (lx as f32 + hx * a - hz * b).round() as i32,
                    (lz as f32 + hz * a + hx * b).round() as i32,
                );
                if seen.insert(cell) && !surface.support_blocked(cell.0, cell.1, y) {
                    parts.piers.push((cell.0, cell.1, y - 2));
                    if !wall && height >= 5 {
                        parts.footed.push(cell);
                    }
                }
            }
        }
    }
    // A cap beam ties separate columns under the deck.
    if !wall && picks.len() > 1 && height >= 5 {
        for &(x, z) in line {
            parts.caps.push((x, z, y - 2));
        }
    }
}

fn lit_lamp() -> BlockWithProperties {
    BlockWithProperties::new(
        REDSTONE_LAMP,
        Some(Value::Compound(HashMap::from([(
            "lit".to_string(),
            Value::String("true".to_string()),
        )]))),
    )
}

/// Draws the parts member `way_id` owns.
pub fn render(editor: &mut WorldEditor, design: DeckDesign, parts: &DeckParts) {
    let slab = design.slab();
    for &(x, z, y) in &parts.fills {
        editor.set_block_absolute(design.curb(), x, y, z, None, None);
        editor.set_block_absolute(slab, x, y - 1, z, None, None);
    }
    // Over water a beam keeps a block of air above it.
    let room = |editor: &WorldEditor, x: i32, z: i32, y: i32| {
        y > editor.get_ground_level(x, z) + i32::from(editor.is_mapped_water(x, z))
    };
    for &(x, z, y) in parts.rails.iter().chain(&parts.lamps) {
        editor.set_block_absolute(design.curb(), x, y, z, None, Some(&[]));
        editor.set_block_absolute(slab, x, y - 1, z, None, None);
        // An edge beam makes the deck read thicker than its slab.
        if room(editor, x, z, y - 2) {
            editor.set_block_absolute(design.beam(), x, y - 2, z, None, None);
        }
    }
    if let Some(parapet) = design.parapet() {
        let joined = is_connectable(parapet);
        for (x, z, y) in parts
            .rails
            .iter()
            .map(|r| (r.0, r.1, r.2 + 1))
            .chain(parts.steps.iter().copied())
        {
            if joined {
                place_connected(editor, parapet, x, y, z);
            } else {
                editor.set_block_absolute(parapet, x, y, z, None, None);
            }
        }
    }
    for &(x, z, y) in &parts.lamps {
        for dy in 1..=4 {
            place_connected(editor, design.lamp_pole(), x, y + dy, z);
        }
        editor.set_block_with_properties_absolute(lit_lamp(), x, y + 5, z, None, None);
        editor.set_block_absolute(IRON_TRAPDOOR, x, y + 6, z, None, None);
    }
    for &(x, z, y) in parts.caps.iter().chain(&parts.girders) {
        if room(editor, x, z, y) {
            editor.set_block_absolute(design.beam(), x, y, z, None, None);
        }
    }
    let footed: HashSet<(i32, i32)> = parts.footed.iter().copied().collect();
    let pier = design.pier();
    for &(x, z, top) in &parts.piers {
        let ground = editor.get_ground_level(x, z);
        if top <= ground {
            continue;
        }
        for y in ground + 1..=top {
            editor.set_block_absolute(pier, x, y, z, None, None);
        }
        editor.register_support_column(x, z, pier);
        if footed.contains(&(x, z)) {
            for dx in -1..=1 {
                for dz in -1..=1 {
                    editor.set_block_absolute(pier, x + dx, ground, z + dz, None, None);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::element_processing::bridge_styles::BridgeOutlineIndex;
    use crate::element_processing::highways::build_highway_connectivity_map;
    use crate::osm_parser::ProcessedNode;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn way(id: u64, tags: &[(&str, &str)], points: &[(i32, i32)]) -> ProcessedWay {
        ProcessedWay {
            id,
            nodes: points
                .iter()
                .enumerate()
                .map(|(i, &(x, z))| ProcessedNode {
                    id: id * 100 + i as u64,
                    tags: HashMap::new(),
                    x,
                    z,
                })
                .collect(),
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn old_stepped_and_setted_bridges_are_masonry() {
        let pick = |tags: &[(&str, &str)]| {
            DeckDesign::pick(BridgeStyle::Beam, [way(7, tags, &[(0, 0), (20, 0)])].iter())
        };
        for tags in [
            &[("highway", "footway"), ("historic", "bridge")][..],
            &[("highway", "residential"), ("start_date", "1885")],
            &[("highway", "steps")],
            &[("highway", "footway"), ("surface", "sett")],
        ] {
            assert_eq!(pick(tags), DeckDesign::Stone, "{tags:?}");
        }
    }

    #[test]
    fn untagged_bridges_vary_but_main_roads_and_long_spans_keep_their_look() {
        let looks = |highway: &str, length: i32| -> HashSet<DeckDesign> {
            (1..64)
                .map(|id| {
                    let w = way(id, &[("highway", highway)], &[(0, 0), (length, 0)]);
                    DeckDesign::pick(BridgeStyle::Beam, [w].iter())
                })
                .collect()
        };
        use DeckDesign::*;
        assert_eq!(looks("residential", 30), HashSet::from([Road, Stone]));
        assert_eq!(looks("footway", 30), HashSet::from([Foot, Glass]));
        assert_eq!(looks("primary", 30), HashSet::from([Road]));
        assert_eq!(looks("residential", 90), HashSet::from([Road]));
    }

    #[test]
    fn steel_structures_keep_a_steel_railing_whatever_their_age() {
        let old = [("start_date", "1937"), ("surface", "sett")];
        let pick = |style, highway| {
            let tags = [("highway", highway), old[0], old[1]];
            DeckDesign::pick(style, [way(7, &tags, &[(0, 0), (20, 0)])].iter())
        };
        for style in [
            BridgeStyle::Suspension,
            BridgeStyle::CableStayed,
            BridgeStyle::Truss,
        ] {
            assert_eq!(pick(style, "motorway"), DeckDesign::Road);
            assert_eq!(pick(style, "cycleway"), DeckDesign::Foot);
        }
        assert_eq!(pick(BridgeStyle::Beam, "residential"), DeckDesign::Stone);
    }

    /// Every pair of `lamps` stands at least `LAMP_CLEARANCE` apart.
    fn assert_lamps_clear(lamps: &[(i32, i32, i32)]) {
        for (i, a) in lamps.iter().enumerate() {
            for b in &lamps[i + 1..] {
                let d = (a.0 - b.0).pow(2) + (a.1 - b.1).pow(2);
                assert!(d >= LAMP_CLEARANCE.pow(2), "{a:?} by {b:?}: {lamps:?}");
            }
        }
    }

    #[test]
    fn lamps_on_a_narrow_lit_deck_take_alternate_sides() {
        let tags = &[("highway", "footway"), ("bridge", "yes"), ("lit", "yes")];
        let (_, surface) = build(&[way(1, tags, &[(10, 50), (90, 50)])], VALLEY, &[]);
        let lamps = &surface.parts_of(1).expect("parts").lamps;
        assert!(lamps.len() >= 3, "{lamps:?}");
        assert_lamps_clear(lamps);
        assert!(lamps.iter().any(|l| l.1 > 50) && lamps.iter().any(|l| l.1 < 50));
    }

    #[test]
    fn a_road_and_its_sidewalk_never_pair_their_lamps() {
        let lit = |highway| [("highway", highway), ("bridge", "yes"), ("lit", "yes")];
        let road = way(1, &lit("residential"), &[(10, 50), (90, 50)]);
        let walk = way(2, &lit("footway"), &[(10, 56), (90, 56)]);
        let (_, surface) = build(&[road, walk], VALLEY, &[]);
        let lamps: Vec<(i32, i32, i32)> = [1, 2]
            .iter()
            .filter_map(|&id| surface.parts_of(id))
            .flat_map(|p| p.lamps.iter().copied())
            .collect();
        assert!(lamps.len() >= 3, "{lamps:?}");
        assert_lamps_clear(&lamps);
    }

    #[test]
    fn a_lamp_mapped_beside_a_deck_finds_its_railing() {
        let tags = &[("highway", "footway"), ("bridge", "yes")];
        let (_, surface) = build(&[way(1, tags, &[(10, 50), (90, 50)])], VALLEY, &[]);
        let ((x, z), y) = surface
            .nearest_railing(50, 55, 4)
            .expect("a railing in reach");
        assert_eq!((x, z), (50, 52));
        let rail = &surface.parts_of(1).expect("parts").rails;
        assert!(rail.contains(&(x, z, y)), "{rail:?}");
        assert_eq!(surface.nearest_railing(50, 58, 4), None);
    }

    #[test]
    fn lamp_posts_are_made_like_the_railing() {
        assert_eq!(DeckDesign::Road.lamp_pole(), IRON_BARS);
        assert_eq!(DeckDesign::Glass.lamp_pole(), IRON_BARS);
        assert_eq!(DeckDesign::Stone.lamp_pole(), STONE_BRICK_WALL);
        assert_eq!(DeckDesign::Motorway.lamp_pole(), ANDESITE_WALL);
    }

    /// Plans and draws `ways` over a 100x100 world whose ground at column x is `height(x)`.
    fn build(
        ways: &[ProcessedWay],
        height: impl Fn(usize) -> f32,
        draw: &[u64],
    ) -> (WorldEditor<'static>, BridgeSurfaceMap) {
        let xzbbox: &'static XZBBox = Box::leak(Box::new(
            XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap(),
        ));
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let mut editor = WorldEditor::new(PathBuf::from("/dev/null/unused"), xzbbox, llbbox);
        let row: Vec<f32> = (0..100).map(height).collect();
        let ground = crate::ground::Ground::new_elevation_test(vec![row; 100], 100, 100);
        editor.set_ground(Arc::new(ground));
        let elements: Vec<ProcessedElement> =
            ways.iter().cloned().map(ProcessedElement::Way).collect();
        let outlines = BridgeOutlineIndex::build(&elements);
        let structures = BridgeStructureMap::build(
            &elements,
            &editor,
            &build_highway_connectivity_map(&elements),
            &outlines,
            1.0,
        );
        let surface = BridgeSurfaceMap::build(&elements, &structures, 1.0).with_parts(
            &elements,
            &structures,
            &editor,
            1.0,
        );
        for &id in draw {
            let info = structures.lookup_member(id).expect("member");
            if let Some(parts) = surface.parts_of(id) {
                render(&mut editor, structures.structure_of(info).design, parts);
            }
        }
        (editor, surface)
    }

    const VALLEY: fn(usize) -> f32 = |x| if (25..75).contains(&x) { 0.0 } else { 12.0 };

    #[test]
    fn a_skewed_deck_has_joined_railings_along_both_sides_and_none_past_its_ends() {
        let bridge = way(
            1,
            &[("highway", "residential"), ("bridge", "yes")],
            &[(15, 30), (85, 62)],
        );
        let west = way(2, &[("highway", "residential")], &[(0, 23), (15, 30)]);
        let east = way(3, &[("highway", "residential")], &[(85, 62), (99, 68)]);
        let (editor, surface) = build(&[bridge, west, east], VALLEY, &[1]);
        let parts = surface.parts_of(1).expect("the deck has parts");
        let (dx, dz) = (70.0f32, 32.0f32);
        let len = dx.hypot(dz);
        let side = |x: i32, z: i32| ((x - 15) as f32 * dz - (z - 30) as f32 * dx).signum();
        let along = |x: i32, z: i32| ((x - 15) as f32 * dx + (z - 30) as f32 * dz) / len;
        assert!(parts.rails.iter().any(|r| side(r.0, r.1) > 0.0));
        assert!(parts.rails.iter().any(|r| side(r.0, r.1) < 0.0));
        for r in &parts.rails {
            let a = along(r.0, r.1);
            assert!(a > -1.0 && a < len + 1.0, "railing past an end at {r:?}");
        }
        // Every railing block joins each railing block beside it, and nothing else.
        let rails: HashSet<(i32, i32, i32)> =
            parts.rails.iter().map(|r| (r.0, r.2 + 1, r.1)).collect();
        for &(x, y, z) in &rails {
            let Some(Value::Compound(props)) = editor.block_properties_absolute(x, y, z) else {
                panic!("no railing at ({x}, {y}, {z})");
            };
            for (key, (ox, oz)) in [
                ("north", (0, -1)),
                ("south", (0, 1)),
                ("east", (1, 0)),
                ("west", (-1, 0)),
            ] {
                let joined = props.get(key) == Some(&Value::String("true".to_string()));
                let next = editor.get_block_absolute(x + ox, y, z + oz);
                assert_eq!(
                    joined,
                    next.is_some_and(is_connectable),
                    "({x}, {y}, {z}) {key}"
                );
            }
        }
    }

    #[test]
    fn decks_side_by_side_are_decked_over_between_them() {
        let tags = &[("highway", "footway"), ("bridge", "yes"), ("name", "Steg")];
        let a = way(1, tags, &[(10, 40), (90, 40)]);
        let b = way(2, tags, &[(10, 44), (90, 44)]);
        let (_, surface) = build(&[a, b], VALLEY, &[]);
        let fills: Vec<(i32, i32, i32)> = [1, 2]
            .iter()
            .filter_map(|&id| surface.parts_of(id))
            .flat_map(|p| p.fills.iter().copied())
            .collect();
        assert!(fills.iter().any(|f| f.1 == 42 && f.0 == 50), "{fills:?}");
        let rails_between = [1, 2]
            .iter()
            .filter_map(|&id| surface.parts_of(id))
            .flat_map(|p| p.rails.iter())
            .filter(|r| (41..=43).contains(&r.1) && (20..80).contains(&r.0))
            .count();
        assert_eq!(rails_between, 0);
    }

    #[test]
    fn piers_stand_under_a_high_deck_but_never_on_the_road_below() {
        let bridge = way(
            1,
            &[("highway", "secondary"), ("bridge", "yes")],
            &[(5, 50), (95, 50)],
        );
        let under = way(2, &[("highway", "residential")], &[(50, 0), (50, 99)]);
        let (editor, surface) = build(&[bridge, under], VALLEY, &[1]);
        let parts = surface.parts_of(1).expect("the deck has parts");
        let mut stations: Vec<i32> = parts.piers.iter().map(|p| p.0).collect();
        stations.sort_unstable();
        stations.dedup();
        assert!(
            !stations.is_empty(),
            "a deck 12 over the valley stands on piers"
        );
        for p in &parts.piers {
            assert!((p.0 - 50).abs() > 3, "a pier on the road at {p:?}");
            assert!(editor.get_block_absolute(p.0, p.2, p.1).is_some());
        }
    }
}
