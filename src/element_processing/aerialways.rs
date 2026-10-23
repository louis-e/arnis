//! Ski lifts, gondolas and cable cars along `aerialway` ways: pylons, terminals, the rope
//! and the chairs, cabins or hangers on it.
//!
//! Inner vertices of a lift way are its pylons (the tile archive drops the `aerialway=pylon`
//! node tags but keeps the vertices); a way with none gets pylons where the rope needs them.

use crate::block_definitions::*;
use crate::clipping::is_invented_node_id;
use crate::coordinate_system::cartesian::XZBBox;
use crate::element_processing::connected_blocks::{four_connected_line, place_connected};
use crate::element_processing::structure_kit::{
    axis_of, beam, concrete_for_colour, facing_name, mix, place, stair_backed, top_slab, with_prop,
    with_props, Frame,
};
use crate::floodfill_cache::{CoordinateBitmap, RoadMaskBitmap};
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    CableCar,
    Gondola,
    Mixed,
    Chair,
    TBar,
    JBar,
    Platter,
    RopeTow,
    MagicCarpet,
    ZipLine,
    Goods,
}

impl Kind {
    fn from_tag(value: &str) -> Option<Kind> {
        Some(match value {
            "cable_car" => Kind::CableCar,
            "gondola" => Kind::Gondola,
            "mixed_lift" => Kind::Mixed,
            "chair_lift" => Kind::Chair,
            "drag_lift" | "t-bar" => Kind::TBar,
            "j-bar" => Kind::JBar,
            "platter" => Kind::Platter,
            "rope_tow" => Kind::RopeTow,
            "magic_carpet" => Kind::MagicCarpet,
            "zip_line" => Kind::ZipLine,
            "goods" => Kind::Goods,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Carrier {
    /// Seats `2 * half + 1` wide.
    Chair {
        half: i32,
        bubble: bool,
    },
    Cabin {
        half_w: i32,
        half_l: i32,
    },
    Tram {
        half_w: i32,
        half_l: i32,
    },
    Bar {
        half: i32,
    },
    JBar,
    Platter,
    Bucket,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Terminal {
    /// Detachable lifts: a hooded terminal round the bullwheel.
    Housing,
    /// Fixed-grip and surface lifts: a bullwheel on a post.
    Open,
    /// Aerial tramways: a hall the cabins drive into.
    Hall,
    /// Rope tows: a post.
    Post,
    /// Zip lines: a launch tower and a landing post.
    Platform,
}

#[derive(Clone, Debug)]
struct Spec {
    kind: Kind,
    /// Distance between the up and down rope.
    gauge: f64,
    /// Rope above the terminal floor.
    station_rope: f64,
    /// Rope above the ground at a pylon, before the solver raises it.
    pylon_rope: f64,
    min_rope: f64,
    max_rope: f64,
    /// How far a carrier reaches below the rope.
    hang: f64,
    /// Air left under a carrier away from the terminals.
    clearance: f64,
    /// Mid-span sag per block of span.
    sag: f64,
    /// Pylon spacing for a way with no inner vertices; 0 places none up front.
    spacing: f64,
    /// Rope distance between carriers.
    gap: f64,
    carrier: Option<Carrier>,
    /// Mixed lifts alternate cabins and chairs.
    alternate: Option<Carrier>,
    terminal: Terminal,
    /// Terminal length along the line.
    terminal_len: f64,
}

fn tag<'a>(tags: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    tags.get(key).map(String::as_str)
}

impl Spec {
    fn new(kind: Kind, tags: &HashMap<String, String>) -> Spec {
        let occupancy = tag(tags, "aerialway:occupancy")
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|n| (1..=250).contains(n));
        let bubble = tag(tags, "aerialway:bubble") == Some("yes");
        let base = Spec {
            kind,
            gauge: 6.0,
            station_rope: 5.5,
            pylon_rope: 9.0,
            min_rope: 6.0,
            max_rope: 22.0,
            hang: 4.5,
            clearance: 2.0,
            sag: 0.015,
            spacing: 70.0,
            gap: 12.0,
            carrier: None,
            alternate: None,
            terminal: Terminal::Open,
            terminal_len: 0.0,
        };
        match kind {
            Kind::Chair => {
                let seats = occupancy.unwrap_or(4);
                let half = match seats {
                    1 => 0,
                    2..=4 => 1,
                    _ => 2,
                };
                let detachable = match tag(tags, "aerialway:detachable") {
                    Some("yes") => true,
                    Some("no") => false,
                    _ => seats >= 4 || bubble,
                };
                Spec {
                    gauge: (2 * half + 1) as f64 + 3.0,
                    gap: if detachable { 36.0 } else { 18.0 },
                    carrier: Some(Carrier::Chair { half, bubble }),
                    terminal: if detachable {
                        Terminal::Housing
                    } else {
                        Terminal::Open
                    },
                    terminal_len: 16.0,
                    ..base
                }
            }
            Kind::Gondola | Kind::Mixed => {
                let seats = occupancy.unwrap_or(8);
                let (half_w, half_l) = match seats {
                    0..=10 => (1, 1),
                    11..=20 => (1, 2),
                    _ => (2, 2),
                };
                let mixed = kind == Kind::Mixed;
                let width = if mixed { 5.0 } else { (2 * half_w + 1) as f64 };
                Spec {
                    gauge: width + 3.0,
                    station_rope: 6.5,
                    pylon_rope: 13.0,
                    min_rope: 9.0,
                    max_rope: 45.0,
                    hang: 6.5,
                    clearance: 3.0,
                    spacing: 110.0,
                    gap: if mixed {
                        45.0
                    } else if half_w > 1 {
                        80.0
                    } else {
                        54.0
                    },
                    carrier: Some(Carrier::Cabin { half_w, half_l }),
                    alternate: mixed.then_some(Carrier::Chair {
                        half: 2,
                        bubble: true,
                    }),
                    terminal: Terminal::Housing,
                    terminal_len: 18.0,
                    ..base
                }
            }
            Kind::CableCar => {
                let (half_w, half_l) = match occupancy.unwrap_or(80) {
                    0..=40 => (2, 2),
                    41..=110 => (2, 3),
                    _ => (3, 4),
                };
                Spec {
                    gauge: (2 * half_w + 1) as f64 + 5.0,
                    station_rope: 8.5,
                    pylon_rope: 30.0,
                    min_rope: 14.0,
                    max_rope: 90.0,
                    hang: 8.5,
                    clearance: 6.0,
                    sag: 0.02,
                    spacing: 0.0,
                    gap: 0.0,
                    carrier: Some(Carrier::Tram { half_w, half_l }),
                    terminal: Terminal::Hall,
                    terminal_len: 16.0,
                    ..base
                }
            }
            Kind::TBar | Kind::JBar | Kind::Platter => Spec {
                gauge: 3.0,
                station_rope: 6.5,
                pylon_rope: 6.5,
                min_rope: 5.0,
                max_rope: 11.0,
                hang: 4.5,
                clearance: 1.0,
                sag: 0.01,
                spacing: 55.0,
                gap: if kind == Kind::Platter { 10.0 } else { 12.0 },
                carrier: Some(match kind {
                    Kind::TBar => Carrier::Bar { half: 1 },
                    Kind::JBar => Carrier::JBar,
                    _ => Carrier::Platter,
                }),
                ..base
            },
            Kind::RopeTow => Spec {
                gauge: 2.0,
                station_rope: 1.3,
                pylon_rope: 1.3,
                min_rope: 1.1,
                max_rope: 2.0,
                hang: 0.0,
                clearance: 0.2,
                sag: 0.0,
                spacing: 20.0,
                terminal: Terminal::Post,
                ..base
            },
            Kind::Goods => Spec {
                gauge: 3.0,
                station_rope: 4.5,
                pylon_rope: 10.0,
                min_rope: 5.0,
                max_rope: 30.0,
                hang: 2.5,
                clearance: 2.0,
                spacing: 120.0,
                gap: 30.0,
                carrier: Some(Carrier::Bucket),
                ..base
            },
            Kind::ZipLine | Kind::MagicCarpet => Spec {
                gauge: 0.0,
                station_rope: 10.5,
                pylon_rope: 10.5,
                min_rope: 2.0,
                max_rope: 10.5,
                hang: 0.0,
                clearance: 0.0,
                sag: 0.02,
                spacing: 0.0,
                terminal: Terminal::Platform,
                ..base
            },
        }
    }

    /// Half the terminal length, where the ropes leave it.
    fn mouth(&self) -> f64 {
        match self.terminal {
            Terminal::Housing | Terminal::Hall => self.terminal_len / 2.0,
            Terminal::Open => self.gauge / 2.0,
            _ => 0.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Station,
    Pylon,
    /// Where the way leaves the world: the rope runs on, nothing stands there.
    Edge,
}

#[derive(Clone, Debug)]
struct Support {
    x: f64,
    z: f64,
    ground: f64,
    /// Absolute rope height.
    rope_y: f64,
    role: Role,
    /// Added by the planner rather than mapped.
    inferred: bool,
}

/// Closest supports may stand to one another.
const MIN_GAP: f64 = 8.0;
/// Clearance under the carriers grows to its full value over this run past a terminal.
const CLEARANCE_RAMP: f64 = 20.0;
const ROPE_THICKNESS: f64 = 0.09;
const ROPE_BLOCK: &str = "minecraft:gray_concrete";
/// Longest rope chord drawn as one display; displays draw only near their centre.
const MAX_CHORD: f64 = 12.0;

fn dist(ax: f64, az: f64, bx: f64, bz: f64) -> f64 {
    ((ax - bx).powi(2) + (az - bz).powi(2)).sqrt()
}

/// What a way vertex is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hint {
    Station,
    Pylon,
    Edge,
    Skip,
}

/// Supports along the line with rope heights that keep the carriers off the ground.
fn plan_supports(
    points: &[(f64, f64, Hint)],
    spec: &Spec,
    ground: &dyn Fn(f64, f64) -> f64,
) -> Vec<Support> {
    let mut sup: Vec<Support> = Vec::new();
    let n = points.len();
    for (i, &(x, z, hint)) in points.iter().enumerate() {
        let role = if i == 0 || i == n - 1 {
            if hint == Hint::Edge {
                Role::Edge
            } else {
                Role::Station
            }
        } else if hint == Hint::Skip {
            continue;
        } else {
            Role::Pylon
        };
        sup.push(Support {
            x,
            z,
            ground: ground(x, z),
            rope_y: 0.0,
            role,
            inferred: false,
        });
    }
    // Drop inner pylons crowding a neighbour, or standing inside a terminal.
    let mouth = spec.mouth() + 4.0;
    let mut kept: Vec<Support> = Vec::new();
    for (i, s) in sup.iter().enumerate() {
        if s.role == Role::Pylon {
            let prev = kept.last().unwrap();
            let next = &sup[sup.len() - 1];
            let near_prev = dist(s.x, s.z, prev.x, prev.z)
                < if prev.role == Role::Station {
                    mouth
                } else {
                    MIN_GAP
                };
            let near_end = i + 1 < sup.len()
                && next.role == Role::Station
                && dist(s.x, s.z, next.x, next.z) < mouth;
            if near_prev || near_end {
                continue;
            }
        }
        kept.push(s.clone());
    }
    let mut sup = kept;
    if sup.len() < 2 {
        return sup;
    }

    if sup.len() == 2 && spec.spacing > 0.0 {
        let (a, b) = (sup[0].clone(), sup[1].clone());
        let len = dist(a.x, a.z, b.x, b.z);
        let count = (len / spec.spacing).round() as usize;
        let mut extra = Vec::new();
        for k in 1..count {
            let t = k as f64 / count as f64;
            let (x, z) = (a.x + (b.x - a.x) * t, a.z + (b.z - a.z) * t);
            extra.push(Support {
                x,
                z,
                ground: ground(x, z),
                rope_y: 0.0,
                role: Role::Pylon,
                inferred: true,
            });
        }
        sup.splice(1..1, extra);
    }

    for s in sup.iter_mut() {
        s.rope_y = s.ground
            + match s.role {
                Role::Station => spec.station_rope,
                _ => spec.pylon_rope,
            };
    }
    solve_heights(&mut sup, spec, ground);
    // A planned pylon in a hollow the rope spans anyway only stands in the way.
    let mut i = 1;
    while i + 1 < sup.len() {
        let s = &sup[i];
        if s.inferred && s.role == Role::Pylon {
            let (a, b) = (&sup[i - 1], &sup[i + 1]);
            let t = dist(a.x, a.z, s.x, s.z) / dist(a.x, a.z, b.x, b.z).max(1e-6);
            let chord = a.rope_y + (b.rope_y - a.rope_y) * t;
            if chord - s.ground > spec.max_rope {
                let mut without = sup.clone();
                without.remove(i);
                if worst_shortfall(&without[i - 1], &without[i], spec, ground, &without).is_none() {
                    sup = without;
                    continue;
                }
            }
        }
        i += 1;
    }
    // A pylon's rope sits just on top of its sheave block.
    for s in sup.iter_mut() {
        if s.role == Role::Pylon {
            s.rope_y = s.rope_y.ceil() + 0.1;
        }
    }
    sup
}

/// Rope height `t` of the way along a span, with its sag.
fn rope_at(a: &Support, b: &Support, t: f64, sag: f64) -> f64 {
    let len = dist(a.x, a.z, b.x, b.z);
    a.rope_y + (b.rope_y - a.rope_y) * t - 4.0 * sag * len * t * (1.0 - t)
}

/// Worst shortfall of rope height along a span, with where it falls.
fn worst_shortfall(
    a: &Support,
    b: &Support,
    spec: &Spec,
    ground: &dyn Fn(f64, f64) -> f64,
    all: &[Support],
) -> Option<(f64, f64)> {
    let len = dist(a.x, a.z, b.x, b.z);
    if len < 1.0 {
        return None;
    }
    let (ux, uz) = ((b.x - a.x) / len, (b.z - a.z) / len);
    let (rx, rz) = (-uz, ux);
    let half = spec.gauge / 2.0;
    let stations: Vec<(f64, f64)> = all
        .iter()
        .filter(|s| s.role == Role::Station)
        .map(|s| (s.x, s.z))
        .collect();
    let steps = len.ceil() as usize;
    let mut worst: Option<(f64, f64)> = None;
    for k in 1..steps {
        let t = k as f64 / steps as f64;
        let (x, z) = (a.x + (b.x - a.x) * t, a.z + (b.z - a.z) * t);
        let mut g = ground(x, z);
        if half > 0.0 {
            g = g
                .max(ground(x + rx * half, z + rz * half))
                .max(ground(x - rx * half, z - rz * half));
        }
        let from_station = stations
            .iter()
            .map(|&(sx, sz)| dist(x, z, sx, sz))
            .fold(f64::MAX, f64::min);
        let ramp = ((from_station - spec.mouth()) / CLEARANCE_RAMP).clamp(0.0, 1.0);
        let need = g + spec.hang + spec.clearance * ramp;
        let short = need - rope_at(a, b, t, spec.sag);
        if short > 0.15 && worst.is_none_or(|(_, w)| short > w) {
            worst = Some((t, short));
        }
    }
    worst
}

fn solve_heights(sup: &mut Vec<Support>, spec: &Spec, ground: &dyn Fn(f64, f64) -> f64) {
    let can_insert = spec.kind != Kind::ZipLine;
    let total_len: f64 = sup
        .windows(2)
        .map(|w| dist(w[0].x, w[0].z, w[1].x, w[1].z))
        .sum();
    let max_supports = 2 + (total_len / 12.0) as usize;
    for _ in 0..400 {
        let mut changed = false;
        let mut i = 0;
        while i + 1 < sup.len() {
            let Some((t, short)) = worst_shortfall(&sup[i], &sup[i + 1], spec, ground, sup) else {
                i += 1;
                continue;
            };
            let room = |s: &Support| {
                if s.role == Role::Station {
                    0.0
                } else {
                    (s.ground + spec.max_rope - s.rope_y).max(0.0)
                }
            };
            let (ra, rb) = (room(&sup[i]), room(&sup[i + 1]));
            let (wa, wb) = (1.0 - t, t);
            let (da, db) = match (ra > 0.01, rb > 0.01) {
                (true, true) => (short.min(ra), short.min(rb)),
                (true, false) => ((short / wa.max(0.25)).min(ra), 0.0),
                (false, true) => (0.0, (short / wb.max(0.25)).min(rb)),
                (false, false) => (0.0, 0.0),
            };
            sup[i].rope_y += da;
            sup[i + 1].rope_y += db;
            let gained = da * wa + db * wb;
            if da > 0.01 || db > 0.01 {
                changed = true;
            }
            if gained + 0.1 < short {
                let len = dist(sup[i].x, sup[i].z, sup[i + 1].x, sup[i + 1].z);
                if can_insert
                    && sup.len() < max_supports
                    && t * len >= MIN_GAP
                    && (1.0 - t) * len >= MIN_GAP
                {
                    let (a, b) = (&sup[i], &sup[i + 1]);
                    let (x, z) = (a.x + (b.x - a.x) * t, a.z + (b.z - a.z) * t);
                    let g = ground(x, z);
                    let rope = (rope_at(a, b, t, spec.sag) + short)
                        .clamp(g + spec.min_rope, g + spec.max_rope);
                    sup.insert(
                        i + 1,
                        Support {
                            x,
                            z,
                            ground: g,
                            rope_y: rope,
                            role: Role::Pylon,
                            inferred: true,
                        },
                    );
                    changed = true;
                    continue;
                }
            }
            i += 1;
        }
        if !changed {
            break;
        }
    }
}

/// Ground under the line, from the terrain only so every tile agrees.
fn ground_at(editor: &WorldEditor, x: f64, z: f64) -> f64 {
    let (bx, bz) = (x.floor() as i32, z.floor() as i32);
    editor
        .terrain_level(bx, bz)
        .unwrap_or_else(|| editor.get_ground_level(bx, bz)) as f64
}

/// Body colour of the cabins, chairs' bubbles and terminal trim.
fn lift_colour(way: &ProcessedWay) -> Block {
    if let Some(b) = tag(&way.tags, "colour").and_then(concrete_for_colour) {
        return b;
    }
    // Red twice: the classic cabin colour.
    const COLOURS: [Block; 9] = [
        RED_CONCRETE,
        RED_CONCRETE,
        WHITE_CONCRETE,
        YELLOW_CONCRETE,
        BLACK_CONCRETE,
        BLUE_CONCRETE,
        LIGHT_GRAY_CONCRETE,
        ORANGE_CONCRETE,
        LIGHT_BLUE_CONCRETE,
    ];
    COLOURS[(mix(way.id, 0xA1) % COLOURS.len() as u64) as usize]
}

fn bubble_glass(body: Block) -> Block {
    match body {
        ORANGE_CONCRETE | RED_CONCRETE => ORANGE_STAINED_GLASS,
        YELLOW_CONCRETE => YELLOW_STAINED_GLASS,
        GREEN_CONCRETE | LIME_CONCRETE => LIME_STAINED_GLASS,
        _ => LIGHT_BLUE_STAINED_GLASS,
    }
}

/// Lift ways in drawing order (stations are buildings, handled elsewhere).
pub fn is_lift(tags: &HashMap<String, String>) -> bool {
    tag(tags, "aerialway").and_then(Kind::from_tag).is_some()
}

struct Line<'a> {
    way: &'a ProcessedWay,
    /// Seeds the rope displays; each section of a lift has its own.
    id: u64,
    spec: Spec,
    sup: Vec<Support>,
    /// Travel direction at each support.
    dirs: Vec<(f64, f64)>,
    colour: Block,
    tower: Block,
}

impl Line<'_> {
    fn right(&self, i: usize) -> (f64, f64) {
        let (tx, tz) = self.dirs[i];
        (-tz, tx)
    }

    /// Unit vector from support `i` into the line, for a terminal.
    fn inward(&self, i: usize) -> (f64, f64) {
        let (tx, tz) = self.dirs[i];
        if i == 0 {
            (tx, tz)
        } else {
            (-tx, -tz)
        }
    }

    /// Rope point at support `i` for rope `side` (+1 up, -1 down).
    fn anchor(&self, i: usize, side: f64) -> [f64; 3] {
        let s = &self.sup[i];
        let (rx, rz) = self.right(i);
        let half = self.spec.gauge / 2.0 * side;
        match s.role {
            Role::Pylon => {
                let (x, z) = (s.x + rx * half, s.z + rz * half);
                [x.floor() + 0.5, s.rope_y, z.floor() + 0.5]
            }
            Role::Edge => [s.x + rx * half, s.rope_y, s.z + rz * half],
            Role::Station => {
                let (ix, iz) = self.inward(i);
                let m = self.spec.mouth();
                let along = if self.spec.terminal == Terminal::Open {
                    0.0
                } else {
                    m
                };
                [
                    s.x + ix * along + rx * half,
                    s.rope_y,
                    s.z + iz * along + rz * half,
                ]
            }
        }
    }
}

/// Draws one lift way.
pub fn generate_aerialway(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    building_footprints: &CoordinateBitmap,
    road_mask: &RoadMaskBitmap,
) {
    let Some(kind) = tag(&way.tags, "aerialway").and_then(Kind::from_tag) else {
        return;
    };
    if way.nodes.len() < 2 {
        return;
    }
    if kind == Kind::MagicCarpet {
        generate_magic_carpet(editor, way);
        return;
    }
    let spec = Spec::new(kind, &way.tags);
    let mut points: Vec<(f64, f64, Hint)> = Vec::new();
    let last = way.nodes.len() - 1;
    for (i, node) in way.nodes.iter().enumerate() {
        let (x, z) = (node.x as f64 + 0.5, node.z as f64 + 0.5);
        if points
            .last()
            .is_some_and(|p| dist(p.0, p.1, x, z) < 1.0 && i != last)
        {
            continue;
        }
        let hint = if (i == 0 || i == last) && is_invented_node_id(node.id) {
            Hint::Edge
        } else {
            match tag(&node.tags, "aerialway") {
                Some("pylon") | None => Hint::Pylon,
                Some("station") => Hint::Station,
                Some(_) => Hint::Skip,
            }
        };
        points.push((x, z, hint));
    }
    if points.len() < 2 {
        return;
    }
    let length: f64 = points
        .windows(2)
        .map(|w| dist(w[0].0, w[0].1, w[1].0, w[1].1))
        .sum();
    if length < 12.0 {
        return;
    }
    // Zip lines run downhill from their launch tower.
    if kind == Kind::ZipLine {
        let (a, b) = (points[0], points[points.len() - 1]);
        if ground_at(editor, a.0, a.1) < ground_at(editor, b.0, b.1) {
            points.reverse();
        }
    }
    for (section, range) in sections(&points).into_iter().enumerate() {
        let id = if section == 0 {
            way.id
        } else {
            mix(way.id, section as u64)
        };
        build_section(
            editor,
            way,
            id,
            spec.clone(),
            &points[range],
            building_footprints,
            road_mask,
        );
    }
}

/// Shortest run of rope between a mid-station and the next station.
const MIN_SECTION: f64 = 40.0;

/// Point ranges between the ends and any mapped mid-stations, sharing the station points.
fn sections(points: &[(f64, f64, Hint)]) -> Vec<std::ops::RangeInclusive<usize>> {
    let last = points.len() - 1;
    let mut along = vec![0.0];
    for w in points.windows(2) {
        along.push(along.last().unwrap() + dist(w[0].0, w[0].1, w[1].0, w[1].1));
    }
    let mut out = Vec::new();
    let mut start = 0;
    for (i, p) in points.iter().enumerate().skip(1) {
        let mid = p.2 == Hint::Station
            && along[i] - along[start] >= MIN_SECTION
            && along[last] - along[i] >= MIN_SECTION;
        if i == last || mid {
            out.push(start..=i);
            start = i;
        }
    }
    out
}

fn build_section(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    id: u64,
    spec: Spec,
    points: &[(f64, f64, Hint)],
    building_footprints: &CoordinateBitmap,
    road_mask: &RoadMaskBitmap,
) {
    let kind = spec.kind;
    let ground = |x: f64, z: f64| ground_at(editor, x, z);
    let mut sup = plan_supports(points, &spec, &ground);
    if sup.len() < 2 {
        return;
    }
    if kind == Kind::ZipLine {
        let end = sup.len() - 1;
        sup[end].rope_y = sup[end].ground + 2.5;
    }
    let dirs = (0..sup.len())
        .map(|i| {
            let (p, n) = (i.saturating_sub(1), (i + 1).min(sup.len() - 1));
            let (dx, dz) = (sup[n].x - sup[p].x, sup[n].z - sup[p].z);
            let len = (dx * dx + dz * dz).sqrt().max(1e-6);
            (dx / len, dz / len)
        })
        .collect();
    let line = Line {
        way,
        id,
        spec,
        sup,
        dirs,
        colour: lift_colour(way),
        // Galvanised lattice for tramways, painted tubes for the rest.
        tower: if kind == Kind::CableCar {
            IRON_BLOCK
        } else {
            LIGHT_GRAY_CONCRETE
        },
    };

    for i in 0..line.sup.len() {
        match line.sup[i].role {
            Role::Pylon => draw_pylon(editor, &line, i),
            Role::Station => {
                let (x, z) = (line.sup[i].x.floor() as i32, line.sup[i].z.floor() as i32);
                if !building_footprints.contains(x, z) {
                    draw_terminal(editor, &line, i, road_mask, building_footprints);
                }
            }
            Role::Edge => {}
        }
    }
    draw_ropes(editor, &line, building_footprints);
    place_carriers(editor, &line);
}

fn draw_ropes(editor: &mut WorldEditor, line: &Line, footprints: &CoordinateBitmap) {
    let sides: &[f64] = if line.spec.gauge > 0.0 {
        &[1.0, -1.0]
    } else {
        &[0.0]
    };
    let mut seed = (line.id as i64).wrapping_mul(1_000_003);
    for &side in sides {
        for i in 0..line.sup.len() - 1 {
            let (a, b) = (line.anchor(i, side), line.anchor(i + 1, side));
            let len = dist(a[0], a[2], b[0], b[2]);
            let chords = (len / MAX_CHORD).ceil().max(1.0) as usize;
            let sag = line.spec.sag * len;
            let point = |t: f64| {
                [
                    a[0] + (b[0] - a[0]) * t,
                    a[1] + (b[1] - a[1]) * t - 4.0 * sag * t * (1.0 - t),
                    a[2] + (b[2] - a[2]) * t,
                ]
            };
            for c in 0..chords {
                let (p, q) = (
                    point(c as f64 / chords as f64),
                    point((c + 1) as f64 / chords as f64),
                );
                // A mapped station building takes the rope in through its wall.
                let inside =
                    |r: [f64; 3]| footprints.contains(r[0].floor() as i32, r[2].floor() as i32);
                if inside(p) && inside(q) {
                    continue;
                }
                seed += 1;
                rope(editor, p, q, seed);
            }
        }
    }
    // The rope round the bullwheel inside a housing.
    for i in [0, line.sup.len() - 1] {
        if line.sup[i].role != Role::Station || line.spec.terminal != Terminal::Housing {
            continue;
        }
        let s = &line.sup[i];
        let (sx, sz) = (s.x.floor() as i32, s.z.floor() as i32);
        if footprints.contains(sx, sz) {
            continue;
        }
        let f = terminal_frame(line, i);
        let r = line.spec.gauge / 2.0;
        let wheel = bullwheel_a(&line.spec);
        let y = s.rope_y;
        let mut path: Vec<(f64, f64)> = vec![(line.spec.mouth(), r), (wheel, r)];
        for k in 1..12 {
            let ang = std::f64::consts::PI * k as f64 / 12.0;
            path.push((wheel - r * ang.sin(), r * ang.cos()));
        }
        path.push((wheel, -r));
        path.push((line.spec.mouth(), -r));
        for w in path.windows(2) {
            let (p, q) = (f.world(w[0].0, w[0].1), f.world(w[1].0, w[1].1));
            seed += 1;
            rope(editor, [p.0, y, p.1], [q.0, y, q.1], seed);
        }
    }
}

fn rope(editor: &mut WorldEditor, p: [f64; 3], q: [f64; 3], seed: i64) {
    beam(
        editor,
        p,
        q,
        ROPE_THICKNESS,
        ROPE_BLOCK,
        seed,
        CHAIN_X,
        CHAIN_Y,
    );
}

/// Where along the terminal frame the bullwheel centre sits.
fn bullwheel_a(spec: &Spec) -> f64 {
    match spec.terminal {
        Terminal::Housing => -(spec.terminal_len / 2.0 - spec.gauge / 2.0 - 1.5),
        _ => 0.0,
    }
}

/// Terminal frame: `a` into the line, `b` to the right of travel at either end.
fn terminal_frame(line: &Line, i: usize) -> Frame {
    let s = &line.sup[i];
    let (ix, iz) = line.inward(i);
    let f = Frame::new(s.x, s.z, ix, iz);
    let (rx, rz) = line.right(i);
    let (vx, vz) = f.v();
    if vx * rx + vz * rz < 0.0 {
        f.flipped()
    } else {
        f
    }
}

fn draw_pylon(editor: &mut WorldEditor, line: &Line, i: usize) {
    let s = &line.sup[i];
    let (cx, cz) = (s.x.floor() as i32, s.z.floor() as i32);
    let base = s.ground as i32;
    let sheave_y = s.rope_y.floor() as i32 - 1;
    let (tx, tz) = line.dirs[i];
    let along = axis_of(tx, tz);
    if line.spec.kind == Kind::RopeTow {
        for y in base + 1..=base + 2 {
            place_connected(editor, SPRUCE_FENCE, cx, y, cz);
        }
        return;
    }
    let height = sheave_y - base;
    if height < 2 {
        return;
    }
    if line.spec.kind == Kind::CableCar && height >= 14 {
        lattice_tower(editor, line, i, cx, cz, base, sheave_y);
    } else {
        // Footing, then a tube that is stouter low down on tall towers.
        for dx in -1..=1 {
            for dz in -1..=1 {
                let g = ground_at(editor, (cx + dx) as f64, (cz + dz) as f64) as i32;
                editor.set_block_absolute(POLISHED_ANDESITE, cx + dx, g, cz + dz, None, None);
                if dx == 0 && dz == 0 {
                    continue;
                }
                for y in g + 1..=base {
                    editor.set_block_absolute(POLISHED_ANDESITE, cx + dx, y, cz + dz, None, None);
                }
            }
        }
        let stout = if height >= 16 {
            base + height * 2 / 5
        } else {
            base
        };
        for y in base + 1..sheave_y {
            editor.set_block_absolute(line.tower, cx, y, cz, None, None);
            if y <= stout {
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    editor.set_block_absolute(line.tower, cx + dx, y, cz + dz, None, None);
                }
            }
        }
        // Maintenance ladder on the downhill face.
        let back = (-along.0, -along.1);
        let ladder = with_prop(LADDER, "facing", facing_name(back));
        let from = if height >= 16 { stout + 1 } else { base + 2 };
        for y in from..sheave_y {
            place(editor, ladder.clone(), cx + back.0, y, cz + back.1);
        }
    }
    // Crossarm with a sheave train under each rope, laid along the rope.
    let left = line.anchor(i, -1.0);
    let right = line.anchor(i, 1.0);
    let sheave = top_slab(POLISHED_BLACKSTONE_SLAB);
    for anchor in [left, right] {
        let train = Frame::new(anchor[0], anchor[2], tx, tz);
        let cells: Vec<(i32, i32)> = [-1.2, -0.6, 0.0, 0.6, 1.2]
            .iter()
            .map(|&a| train.cell(a, 0.0))
            .collect();
        for (x, z) in four_connected_line(&cells) {
            place(editor, sheave.clone(), x, sheave_y, z);
        }
    }
    let (lx, lz) = (left[0].floor() as i32, left[2].floor() as i32);
    let (rx, rz) = (right[0].floor() as i32, right[2].floor() as i32);
    let arm: Vec<(i32, i32)> = crate::bresenham::bresenham_line(lx, 0, lz, rx, 0, rz)
        .into_iter()
        .map(|(x, _, z)| (x, z))
        .collect();
    for (x, z) in four_connected_line(&arm) {
        editor.set_block_absolute(line.tower, x, sheave_y, z, None, None);
    }
    editor.set_block_absolute(line.tower, cx, sheave_y, cz, None, None);
}

/// A tapering four-legged steel tower for an aerial tramway.
fn lattice_tower(
    editor: &mut WorldEditor,
    line: &Line,
    i: usize,
    cx: i32,
    cz: i32,
    base: i32,
    top: i32,
) {
    let (tx, tz) = line.dirs[i];
    let f = Frame::new(cx as f64 + 0.5, cz as f64 + 0.5, tx, tz);
    let height = (top - base) as f64;
    let foot = (height / 7.0).clamp(3.0, 7.0);
    let head = 1.5;
    for y in base + 1..top {
        let k = (y - base) as f64 / height;
        let half = foot + (head - foot) * k;
        for (sa, sb) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
            let (x, z) = f.cell(sa * half, sb * half);
            editor.set_block_absolute(line.tower, x, y, z, None, None);
        }
        // Bracing rings every few blocks.
        if (y - base) % 6 == 0 {
            for (a0, b0, a1, b1) in [
                (-half, -half, half, -half),
                (-half, half, half, half),
                (-half, -half, -half, half),
                (half, -half, half, half),
            ] {
                let (x0, z0) = f.cell(a0, b0);
                let (x1, z1) = f.cell(a1, b1);
                for (x, _, z) in crate::bresenham::bresenham_line(x0, 0, z0, x1, 0, z1) {
                    editor.set_block_absolute(IRON_BARS, x, y, z, None, None);
                }
            }
        }
    }
    for (sa, sb) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        let (x, z) = f.cell(sa * foot, sb * foot);
        let g = ground_at(editor, x as f64, z as f64) as i32;
        for y in g..=base {
            editor.set_block_absolute(POLISHED_ANDESITE, x, y, z, None, None);
        }
    }
    // A warning lamp on the head.
    editor.set_block_absolute(REDSTONE_LAMP, cx, top + 1, cz, None, None);
}

fn draw_terminal(
    editor: &mut WorldEditor,
    line: &Line,
    i: usize,
    road_mask: &RoadMaskBitmap,
    footprints: &CoordinateBitmap,
) {
    let s = &line.sup[i];
    let floor = s.ground as i32;
    let rope_cell = s.rope_y.floor() as i32;
    let bottom = i == 0;
    match line.spec.terminal {
        Terminal::Housing => {
            let f = terminal_frame(line, i);
            housing(
                editor, line, &f, floor, rope_cell, bottom, road_mask, footprints,
            );
        }
        Terminal::Hall => {
            let f = terminal_frame(line, i);
            hall(editor, line, &f, floor, rope_cell, road_mask, footprints);
        }
        Terminal::Open => {
            let f = terminal_frame(line, i);
            open_terminal(
                editor, line, &f, floor, rope_cell, bottom, road_mask, footprints,
            );
        }
        Terminal::Post => {
            let (x, z) = (s.x.floor() as i32, s.z.floor() as i32);
            for y in floor + 1..=floor + 2 {
                editor.set_block_absolute(SPRUCE_LOG, x, y, z, None, None);
            }
            editor.set_block_absolute(GRINDSTONE, x, floor + 3, z, None, None);
        }
        Terminal::Platform => {
            if bottom {
                launch_tower(editor, line, i);
            } else {
                let (x, z) = (s.x.floor() as i32, s.z.floor() as i32);
                for y in floor + 1..=rope_cell {
                    editor.set_block_absolute(SPRUCE_LOG, x, y, z, None, None);
                }
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, -1)] {
                    let (ix, iz) = line.inward(i);
                    let (ax, az) = axis_of(ix, iz);
                    editor.set_block_absolute(
                        HAY_BALE,
                        x + dx + ax * 2,
                        floor + 1,
                        z + dz + az * 2,
                        None,
                        None,
                    );
                }
            }
        }
    }
}

/// Flattens the terminal's ground to `floor` and paves it.
#[allow(clippy::too_many_arguments)]
fn platform(
    editor: &mut WorldEditor,
    f: &Frame,
    half_a: f64,
    half_b: f64,
    floor: i32,
    paving: Block,
    road_mask: &RoadMaskBitmap,
    footprints: &CoordinateBitmap,
) {
    for (x, z, _, _) in f.cells(-half_a, half_a, -half_b, half_b) {
        if road_mask.contains(x, z) || footprints.contains(x, z) {
            continue;
        }
        editor.register_road_surface_y(x, z, floor);
        editor.set_block_absolute(paving, x, floor, z, None, None);
    }
}

#[allow(clippy::too_many_arguments)]
fn housing(
    editor: &mut WorldEditor,
    line: &Line,
    f: &Frame,
    floor: i32,
    rope: i32,
    bottom: bool,
    road_mask: &RoadMaskBitmap,
    footprints: &CoordinateBitmap,
) {
    let spec = &line.spec;
    let half_a = spec.terminal_len / 2.0;
    let r = spec.gauge / 2.0;
    let half_b = r + 2.5;
    platform(
        editor,
        f,
        half_a + 1.5,
        half_b + 1.5,
        floor,
        POLISHED_ANDESITE,
        road_mask,
        footprints,
    );
    // Hood: a shallow vault over the whole terminal, trimmed in the lift colour.
    for (x, z, a, b) in f.cells(-half_a, half_a, -half_b, half_b) {
        let edge_b = 1.0 - (b / half_b).powi(2);
        let end = ((a.abs() - (half_a - 3.0)).max(0.0) / 3.0).powi(2);
        let rise = (2.4 * (edge_b - end).max(0.0)).round() as i32;
        let top = rope + 2 + rise;
        let rim = b.abs() > half_b - 1.0 || a.abs() > half_a - 1.0;
        for y in rope + 2..=top {
            editor.set_block_absolute(WHITE_CONCRETE, x, y, z, None, None);
        }
        if rim {
            editor.set_block_absolute(line.colour, x, rope + 1, z, None, None);
            // Side and back skirts cover the wheel and rails.
            if a < half_a - 1.0 {
                editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, rope, z, None, None);
                editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, rope - 1, z, None, None);
            }
        }
    }
    // Columns down the sides and across the back.
    for a in [-half_a + 0.5, -half_a / 3.0, half_a / 3.0, half_a - 0.5] {
        for b in [-half_b + 0.5, half_b - 0.5] {
            let (x, z) = f.cell(a, b);
            for y in floor + 1..rope - 1 {
                editor.set_block_absolute(GRAY_CONCRETE, x, y, z, None, None);
            }
        }
    }
    // Glazed back wall.
    for b in -(half_b as i32)..=(half_b as i32) {
        let (x, z) = f.cell(-half_a + 0.5, b as f64);
        for y in floor + 1..rope - 1 {
            editor.set_block_absolute(LIGHT_GRAY_STAINED_GLASS, x, y, z, None, None);
        }
    }
    // Bullwheel, spokes and drive.
    let wheel = bullwheel_a(spec);
    for k in 0..48 {
        let ang = std::f64::consts::TAU * k as f64 / 48.0;
        let (x, z) = f.cell(wheel + r * ang.cos(), r * ang.sin());
        editor.set_block_absolute(IRON_BLOCK, x, rope, z, None, None);
    }
    for d in 1..(r as i32) {
        for (sa, sb) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            let (x, z) = f.cell(wheel + sa * d as f64, sb * d as f64);
            place(editor, top_slab(SMOOTH_STONE_SLAB), x, rope, z);
        }
    }
    let (hx, hz) = f.cell(wheel, 0.0);
    editor.set_block_absolute(POLISHED_DEEPSLATE, hx, rope, hz, None, None);
    editor.set_block_absolute(POLISHED_DEEPSLATE, hx, rope + 1, hz, None, None);
    // Rails the carriers run on through the terminal.
    let mut a = wheel;
    while a <= half_a - 0.5 {
        for b in [r, -r] {
            let (x, z) = f.cell(a, b);
            editor.set_block_absolute(SMOOTH_STONE_SLAB, x, rope + 1, z, None, None);
        }
        a += 0.5;
    }
    // Ceiling lights between the rails.
    let mut a = -half_a + 2.5;
    while a < half_a - 1.0 {
        let (x, z) = f.cell(a, 0.0);
        editor.set_block_absolute(SEA_LANTERN, x, rope + 1, z, None, None);
        a += 4.0;
    }
    // Loading edge railings.
    for b in [-half_b - 1.0, half_b + 1.0] {
        let mut a = -half_a;
        while a <= half_a - 3.0 {
            let (x, z) = f.cell(a, b);
            if !road_mask.contains(x, z) {
                place_connected(editor, IRON_BARS, x, floor + 1, z);
            }
            a += 1.0;
        }
    }
    // Operator booth beside the loading area.
    let booth_b = -(half_b + 3.5);
    let (booth_a0, booth_a1) = (half_a - 4.0, half_a - 1.0);
    let booth = Frame::new(
        f.world((booth_a0 + booth_a1) / 2.0, booth_b).0,
        f.world((booth_a0 + booth_a1) / 2.0, booth_b).1,
        f.ux,
        f.uz,
    );
    platform(
        editor,
        &booth,
        1.5,
        1.5,
        floor,
        POLISHED_ANDESITE,
        road_mask,
        footprints,
    );
    for (x, z, a, b) in f.cells(booth_a0, booth_a1, booth_b - 1.5, booth_b + 1.5) {
        if road_mask.contains(x, z) || footprints.contains(x, z) {
            continue;
        }
        let wall = (a - (half_a - 4.0)).abs() < 0.8
            || (a - (half_a - 1.0)).abs() < 0.8
            || (b - booth_b).abs() > 0.8;
        editor.set_block_absolute(POLISHED_ANDESITE, x, floor, z, None, None);
        if wall {
            editor.set_block_absolute(WHITE_CONCRETE, x, floor + 1, z, None, None);
            editor.set_block_absolute(GLASS, x, floor + 2, z, None, None);
        }
        editor.set_block_absolute(line.colour, x, floor + 3, z, None, None);
    }
    // Parked carriers, one per side.
    if let Some(carrier) = spec.carrier {
        for side in [1.0, -1.0] {
            let (x, z) = f.world(half_a - 3.0, side * r);
            let (fx, fz) = {
                let (ax, az) = f.world(1.0, 0.0);
                let (ox, oz) = f.world(0.0, 0.0);
                (ax - ox, az - oz)
            };
            // The up rope leaves the bottom station and comes into the top one.
            let sign = if bottom { side } else { -side };
            let travel = (fx * sign, fz * sign);
            draw_carrier(
                editor,
                carrier,
                line.colour,
                (x.floor() as i32, rope, z.floor() as i32),
                travel,
            );
        }
    }
    name_sign(editor, line, f, half_a - 0.5, -half_b + 0.5, floor + 3);
}

fn hall(
    editor: &mut WorldEditor,
    line: &Line,
    f: &Frame,
    floor: i32,
    rope: i32,
    road_mask: &RoadMaskBitmap,
    footprints: &CoordinateBitmap,
) {
    let spec = &line.spec;
    let half_a = spec.terminal_len / 2.0;
    let half_b = spec.gauge / 2.0 + 4.0;
    let roof = rope + 3;
    platform(
        editor,
        f,
        half_a + 1.0,
        half_b + 1.0,
        floor,
        POLISHED_ANDESITE,
        road_mask,
        footprints,
    );
    let opening = spec.gauge / 2.0 + 3.0;
    for (x, z, a, b) in f.cells(-half_a, half_a, -half_b, half_b) {
        let wall = a < -half_a + 1.0 || b.abs() > half_b - 1.0 || a > half_a - 1.0;
        if wall {
            let front = a > half_a - 1.0 && b.abs() < opening;
            for y in floor + 1..=roof {
                if front && y <= rope + 1 {
                    continue;
                }
                let block = if y == floor + 1 {
                    LIGHT_GRAY_CONCRETE
                } else if (y - floor) % 5 == 3 || y == rope {
                    GLASS
                } else {
                    WHITE_CONCRETE
                };
                editor.set_block_absolute(block, x, y, z, None, None);
            }
            editor.set_block_absolute(WHITE_CONCRETE, x, roof + 1, z, None, None);
        } else {
            editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, roof, z, None, None);
            if (x + z) % 5 == 0 {
                editor.set_block_absolute(SEA_LANTERN, x, roof - 1, z, None, None);
            }
        }
    }
    // Rope anchors at the back of the hall.
    for side in [1.0, -1.0] {
        let (x, z) = f.cell(-half_a + 1.5, side * spec.gauge / 2.0);
        for y in floor + 1..=rope {
            editor.set_block_absolute(POLISHED_DEEPSLATE, x, y, z, None, None);
        }
        let p = f.world(half_a, side * spec.gauge / 2.0);
        let q = f.world(-half_a + 1.5, side * spec.gauge / 2.0);
        let y = rope as f64 + 0.5;
        rope_beam(editor, [p.0, y, p.1], [q.0, y, q.1], line.id, side);
    }
    name_sign(editor, line, f, half_a - 0.5, half_b - 0.5, rope + 2);
}

fn rope_beam(editor: &mut WorldEditor, p: [f64; 3], q: [f64; 3], id: u64, side: f64) {
    let seed = (id as i64).wrapping_mul(7_919) + if side > 0.0 { 1 } else { 2 };
    rope(editor, p, q, seed);
}

#[allow(clippy::too_many_arguments)]
fn open_terminal(
    editor: &mut WorldEditor,
    line: &Line,
    f: &Frame,
    floor: i32,
    rope: i32,
    bottom: bool,
    road_mask: &RoadMaskBitmap,
    footprints: &CoordinateBitmap,
) {
    let spec = &line.spec;
    let r = spec.gauge / 2.0;
    let (cx, cz) = f.cell(0.0, 0.0);
    for y in floor + 1..rope {
        editor.set_block_absolute(GRAY_CONCRETE, cx, y, cz, None, None);
    }
    editor.set_block_absolute(POLISHED_DEEPSLATE, cx, rope, cz, None, None);
    editor.set_block_absolute(POLISHED_DEEPSLATE, cx, rope + 1, cz, None, None);
    place(editor, top_slab(SMOOTH_STONE_SLAB), cx, rope + 2, cz);
    let steps = (r * 10.0).ceil().max(16.0) as usize;
    for k in 0..steps {
        let ang = std::f64::consts::TAU * k as f64 / steps as f64;
        let (x, z) = f.cell(r * ang.cos(), r * ang.sin());
        editor.set_block_absolute(IRON_BLOCK, x, rope, z, None, None);
    }
    for d in 1..(r.ceil() as i32) {
        for (sa, sb) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            let (x, z) = f.cell(sa * d as f64, sb * d as f64);
            place(editor, top_slab(SMOOTH_STONE_SLAB), x, rope, z);
        }
    }
    if !bottom {
        return;
    }
    // Drive hut behind the wheel and a boarding deck under the up rope.
    let hut_a = -(r + 4.0);
    let hut = Frame::new(f.world(hut_a, 0.0).0, f.world(hut_a, 0.0).1, f.ux, f.uz);
    platform(
        editor,
        &hut,
        1.5,
        2.5,
        floor,
        POLISHED_ANDESITE,
        road_mask,
        footprints,
    );
    for (x, z, a, b) in f.cells(hut_a - 1.5, hut_a + 1.5, -2.5, 2.5) {
        let wall = (a - hut_a).abs() > 0.8 || b.abs() > 1.8;
        if wall {
            editor.set_block_absolute(WHITE_CONCRETE, x, floor + 1, z, None, None);
            let window = if b.abs() < 0.8 || (a - hut_a).abs() < 0.8 {
                GLASS
            } else {
                WHITE_CONCRETE
            };
            editor.set_block_absolute(window, x, floor + 2, z, None, None);
        }
        editor.set_block_absolute(line.colour, x, floor + 3, z, None, None);
    }
    if matches!(spec.carrier, Some(Carrier::Chair { .. })) {
        let (dx, dz) = f.world((r + 5.0) / 2.0, r);
        let deck = Frame::new(dx, dz, f.ux, f.uz);
        platform(
            editor,
            &deck,
            (r + 5.0) / 2.0,
            1.5,
            floor,
            SPRUCE_PLANKS,
            road_mask,
            footprints,
        );
    }
    name_sign(editor, line, f, hut_a + 1.5, 0.0, floor + 2);
}

fn launch_tower(editor: &mut WorldEditor, line: &Line, i: usize) {
    let s = &line.sup[i];
    let (cx, cz) = (s.x.floor() as i32, s.z.floor() as i32);
    let deck = s.rope_y.floor() as i32 - 2;
    for (dx, dz) in [(-1, -1), (-1, 1), (1, -1), (1, 1)] {
        let g = ground_at(editor, (cx + dx) as f64, (cz + dz) as f64) as i32;
        for y in g + 1..deck {
            editor.set_block_absolute(SPRUCE_LOG, cx + dx, y, cz + dz, None, None);
        }
    }
    for dx in -1..=1 {
        for dz in -1..=1 {
            editor.set_block_absolute(SPRUCE_PLANKS, cx + dx, deck, cz + dz, None, None);
            if dx.abs() == 1 || dz.abs() == 1 {
                place_connected(editor, SPRUCE_FENCE, cx + dx, deck + 1, cz + dz);
            }
        }
    }
    let (tx, tz) = line.dirs[i];
    let (ax, az) = axis_of(tx, tz);
    // Gaps in the railing where riders launch and climb up, and a ladder up a post at the back.
    editor.set_block_absolute(AIR, cx + ax, deck + 1, cz + az, Some(&[SPRUCE_FENCE]), None);
    let (px, pz) = (cx - ax, cz - az);
    editor.set_block_absolute(AIR, px, deck + 1, pz, Some(&[SPRUCE_FENCE]), None);
    let post_ground = ground_at(editor, px as f64, pz as f64) as i32;
    for y in post_ground + 1..deck {
        editor.set_block_absolute(SPRUCE_LOG, px, y, pz, None, None);
    }
    let ladder = with_prop(LADDER, "facing", facing_name((-ax, -az)));
    let (lx, lz) = (cx - ax * 2, cz - az * 2);
    let ladder_ground = ground_at(editor, lx as f64, lz as f64) as i32;
    for y in ladder_ground + 1..=deck {
        place(editor, ladder.clone(), lx, y, lz);
    }
    for y in deck + 1..=deck + 2 {
        editor.set_block_absolute(SPRUCE_LOG, cx, y, cz, None, None);
    }
}

/// Wall sign with the lift's name, Java only.
fn name_sign(editor: &mut WorldEditor, line: &Line, f: &Frame, a: f64, b: f64, y: i32) {
    let Some(name) = tag(&line.way.tags, "name") else {
        return;
    };
    let (x, z) = f.cell(a, b);
    let (ox, oz) = f.world(0.0, 0.0);
    let (px, pz) = f.world(a + 1.0, b);
    let (fx, fz) = (px - ox, pz - oz);
    let facing = match axis_of(fx, fz) {
        (0, -1) => 2,
        (0, 1) => 3,
        (-1, 0) => 4,
        _ => 5,
    };
    let lines = split_name(name);
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    editor.place_wall_sign(x, y, z, facing, &refs);
}

fn split_name(name: &str) -> Vec<String> {
    let mut lines: Vec<String> = vec![String::new()];
    for word in name.split_whitespace() {
        let cur = lines.last_mut().unwrap();
        if !cur.is_empty() && cur.chars().count() + word.chars().count() + 1 > 15 {
            if lines.len() == 4 {
                break;
            }
            lines.push(word.to_string());
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
    }
    lines
}

/// Carriers along both ropes, facing the way they travel.
fn place_carriers(editor: &mut WorldEditor, line: &Line) {
    let Some(carrier) = line.spec.carrier else {
        return;
    };
    let spans: Vec<(usize, f64)> = (0..line.sup.len() - 1)
        .map(|i| {
            let (a, b) = (line.anchor(i, 1.0), line.anchor(i + 1, 1.0));
            (i, dist(a[0], a[2], b[0], b[2]))
        })
        .collect();
    let total: f64 = spans.iter().map(|s| s.1).sum();
    if let Carrier::Tram { .. } = carrier {
        for (side, at) in [(1.0, 0.42), (-1.0, 0.58)] {
            place_on_rope(editor, line, carrier, side, total * at, &spans);
        }
        return;
    }
    let gap = line.spec.gap;
    for side in [1.0, -1.0] {
        let mut s = if side > 0.0 { gap * 0.5 } else { gap };
        let mut k = 0usize;
        while s < total {
            let kind = match line.spec.alternate {
                Some(alt) if k % 3 == 2 => alt,
                _ => carrier,
            };
            place_on_rope(editor, line, kind, side, s, &spans);
            s += gap;
            k += 1;
        }
    }
}

fn place_on_rope(
    editor: &mut WorldEditor,
    line: &Line,
    carrier: Carrier,
    side: f64,
    s: f64,
    spans: &[(usize, f64)],
) {
    let mut left = s;
    for &(i, len) in spans {
        if left > len {
            left -= len;
            continue;
        }
        let t = left / len.max(1e-6);
        let (a, b) = (line.anchor(i, side), line.anchor(i + 1, side));
        let (ra, rb) = (&line.sup[i], &line.sup[i + 1]);
        // Keep clear of terminals and pylon heads.
        let keep = |r: &Support| match r.role {
            Role::Station => line.spec.mouth() + 3.0,
            Role::Pylon => 3.0,
            Role::Edge => 0.0,
        };
        if left < keep(ra) || len - left < keep(rb) {
            return;
        }
        let sag = line.spec.sag * len;
        let p = [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t - 4.0 * sag * t * (1.0 - t),
            a[2] + (b[2] - a[2]) * t,
        ];
        let (dx, dz) = (b[0] - a[0], b[2] - a[2]);
        let travel = if side >= 0.0 { (dx, dz) } else { (-dx, -dz) };
        let cell = (
            p[0].floor() as i32,
            p[1].floor() as i32,
            p[2].floor() as i32,
        );
        let bottom = cell.1 - (line.spec.hang - 0.5).round() as i32;
        if (bottom as f64) <= ground_at(editor, p[0], p[2]) {
            return;
        }
        draw_carrier(editor, carrier, line.colour, cell, travel);
        return;
    }
}

/// One carrier hanging from the rope cell `h`.
fn draw_carrier(
    editor: &mut WorldEditor,
    carrier: Carrier,
    colour: Block,
    h: (i32, i32, i32),
    travel: (f64, f64),
) {
    let (hx, hy, hz) = h;
    let fwd = axis_of(travel.0, travel.1);
    let right = (-fwd.1, fwd.0);
    let at = |f: i32, r: i32| (hx + fwd.0 * f + right.0 * r, hz + fwd.1 * f + right.1 * r);
    match carrier {
        Carrier::Chair { half, bubble } => {
            for y in hy - 2..=hy {
                editor.set_block_absolute(CHAIN_Y, hx, y, hz, None, None);
            }
            let back = (-fwd.0, -fwd.1);
            for r in -half..=half {
                // Tall padded backrest in the lift colour, seat in front of it.
                let (x, z) = at(0, r);
                editor.set_block_absolute(colour, x, hy - 3, z, None, None);
                editor.set_block_absolute(colour, x, hy - 4, z, None, None);
                let (sx, sz) = at(1, r);
                place(
                    editor,
                    stair_backed(POLISHED_DEEPSLATE_STAIRS, back),
                    sx,
                    hy - 4,
                    sz,
                );
            }
            if bubble {
                let glass = bubble_glass(colour);
                for r in -half..=half {
                    let (x, z) = at(1, r);
                    editor.set_block_absolute(glass, x, hy - 2, z, None, None);
                    let (x, z) = at(2, r);
                    editor.set_block_absolute(glass, x, hy - 3, z, None, None);
                }
            } else {
                // Armrests, safety bar and footrest.
                for r in [-half, half] {
                    let (x, z) = at(1, r);
                    place_connected(editor, IRON_BARS, x, hy - 3, z);
                }
                for r in -half..=half {
                    let (x, z) = at(2, r);
                    place_connected(editor, IRON_BARS, x, hy - 3, z);
                    place(editor, top_slab(POLISHED_BLACKSTONE_SLAB), x, hy - 5, z);
                }
                let (x, z) = at(2, 0);
                editor.set_block_absolute(IRON_BARS, x, hy - 4, z, None, None);
            }
        }
        Carrier::Cabin { half_w, half_l } => {
            editor.set_block_absolute(CHAIN_Y, hx, hy, hz, None, None);
            editor.set_block_absolute(CHAIN_Y, hx, hy - 1, hz, None, None);
            cabin_body(editor, colour, &at, hy - 2, half_w, half_l, 2);
        }
        Carrier::Tram { half_w, half_l } => {
            for f in -2..=2 {
                let (x, z) = at(f, 0);
                editor.set_block_absolute(POLISHED_DEEPSLATE, x, hy, z, None, None);
            }
            for f in [-1, 1] {
                let (x, z) = at(f, 0);
                for y in hy - 3..hy {
                    editor.set_block_absolute(POLISHED_DEEPSLATE_WALL, x, y, z, None, None);
                }
            }
            cabin_body(editor, colour, &at, hy - 4, half_w, half_l, 2);
        }
        Carrier::Bar { half } => {
            for y in hy - 3..=hy {
                editor.set_block_absolute(CHAIN_Y, hx, y, hz, None, None);
            }
            for r in -half..=half {
                let (x, z) = at(0, r);
                place_connected(editor, IRON_BARS, x, hy - 4, z);
            }
        }
        Carrier::JBar => {
            for y in hy - 3..=hy {
                editor.set_block_absolute(CHAIN_Y, hx, y, hz, None, None);
            }
            for r in 0..=1 {
                let (x, z) = at(0, r);
                place_connected(editor, IRON_BARS, x, hy - 4, z);
            }
        }
        Carrier::Platter => {
            for y in hy - 3..=hy {
                editor.set_block_absolute(CHAIN_Y, hx, y, hz, None, None);
            }
            place(
                editor,
                with_props(IRON_TRAPDOOR, &[("half", "top"), ("open", "false")]),
                hx,
                hy - 4,
                hz,
            );
        }
        Carrier::Bucket => {
            editor.set_block_absolute(CHAIN_Y, hx, hy, hz, None, None);
            editor.set_block_absolute(CHAIN_Y, hx, hy - 1, hz, None, None);
            editor.set_block_absolute(CAULDRON, hx, hy - 2, hz, None, None);
        }
    }
}

/// A cabin box: roof at `roof`, `windows` rows of glass, floor below them.
fn cabin_body(
    editor: &mut WorldEditor,
    colour: Block,
    at: &dyn Fn(i32, i32) -> (i32, i32),
    roof: i32,
    half_w: i32,
    half_l: i32,
    windows: i32,
) {
    let floor = roof - windows - 1;
    for f in -half_l..=half_l {
        for r in -half_w..=half_w {
            let (x, z) = at(f, r);
            let corner = f.abs() == half_l && r.abs() == half_w;
            let rim = f.abs() == half_l || r.abs() == half_w;
            // Rounded corners top and bottom.
            if !corner {
                editor.set_block_absolute(colour, x, roof, z, None, None);
                editor.set_block_absolute(colour, x, floor, z, None, None);
            }
            if !rim {
                continue;
            }
            // Window posts at the corners and every second cell of a long side.
            let post = corner || (f.abs() == half_l && r % 2 == 0 && half_w > 1 && r != 0);
            for y in floor + 1..roof {
                let block = if post { colour } else { GRAY_STAINED_GLASS };
                editor.set_block_absolute(block, x, y, z, None, None);
            }
        }
    }
}

/// Belt conveyor for beginners, under a white gallery unless `covered=no`.
fn generate_magic_carpet(editor: &mut WorldEditor, way: &ProcessedWay) {
    let covered = tag(&way.tags, "covered") != Some("no");
    let mut belt: Vec<(i32, i32)> = Vec::new();
    for w in way.nodes.windows(2) {
        for (x, _, z) in crate::bresenham::bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z) {
            if belt.last() != Some(&(x, z)) {
                belt.push((x, z));
            }
        }
    }
    let cells: std::collections::HashSet<(i32, i32)> = belt.iter().copied().collect();
    let (dx, dz) = {
        let (a, b) = (&way.nodes[0], &way.nodes[way.nodes.len() - 1]);
        ((b.x - a.x) as f64, (b.z - a.z) as f64)
    };
    let side = {
        let (ax, az) = axis_of(dx, dz);
        (-az, ax)
    };
    for &(x, z) in &belt {
        let g = ground_at(editor, x as f64, z as f64) as i32;
        editor.set_block_absolute(BLACK_CONCRETE, x, g, z, None, None);
        for s in [-1, 1] {
            let (ex, ez) = (x + side.0 * s, z + side.1 * s);
            if cells.contains(&(ex, ez)) {
                continue;
            }
            let eg = ground_at(editor, ex as f64, ez as f64) as i32;
            editor.set_block_absolute(SMOOTH_STONE, ex, eg, ez, None, None);
            if covered {
                for y in g + 1..=g + 2 {
                    editor.set_block_absolute(WHITE_STAINED_GLASS, ex, y, ez, None, None);
                }
                editor.set_block_absolute(WHITE_STAINED_GLASS, ex, g + 3, ez, None, None);
            } else {
                place(
                    editor,
                    with_prop(SMOOTH_STONE_SLAB, "type", "bottom"),
                    ex,
                    eg + 1,
                    ez,
                );
            }
        }
        if covered {
            editor.set_block_absolute(WHITE_STAINED_GLASS, x, g + 3, z, None, None);
        }
    }
}

/// Cells kept free of trees along lift lines and round their terminals.
pub fn clear_corridors(elements: &[ProcessedElement], xzbbox: &XZBBox) -> CoordinateBitmap {
    let mut map = CoordinateBitmap::new(xzbbox);
    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };
        let Some(kind) = tag(&way.tags, "aerialway").and_then(Kind::from_tag) else {
            continue;
        };
        if matches!(kind, Kind::MagicCarpet | Kind::RopeTow) {
            continue;
        }
        let spec = Spec::new(kind, &way.tags);
        // Room for the carriers plus a tree crown either side.
        let half = spec.gauge / 2.0 + 6.0;
        for w in way.nodes.windows(2) {
            mark_band(&mut map, w[0].x, w[0].z, w[1].x, w[1].z, half);
        }
        let reach = spec.mouth().max(4.0) + 8.0;
        for node in [&way.nodes[0], &way.nodes[way.nodes.len() - 1]] {
            mark_band(&mut map, node.x, node.z, node.x, node.z, reach);
        }
    }
    map
}

fn mark_band(map: &mut CoordinateBitmap, ax: i32, az: i32, bx: i32, bz: i32, half: f64) {
    let (ax, az, bx, bz) = (ax as f64, az as f64, bx as f64, bz as f64);
    let (dx, dz) = (bx - ax, bz - az);
    let len2 = (dx * dx + dz * dz).max(1e-9);
    let r = half.ceil() as i32;
    let x0 = ax.min(bx) as i32 - r;
    let x1 = ax.max(bx) as i32 + r;
    let z0 = az.min(bz) as i32 - r;
    let z1 = az.max(bz) as i32 + r;
    // Long diagonal lines would scan a huge box; walk the segment instead.
    let len = len2.sqrt();
    let steps = len.ceil().max(1.0) as i32;
    if len > 2.0 && (x1 - x0) as f64 * (z1 - z0) as f64 > 4.0 * len * (2.0 * half + 2.0) {
        for k in 0..=steps {
            let t = k as f64 / steps as f64;
            let (cx, cz) = (ax + dx * t, az + dz * t);
            mark_band(map, cx as i32, cz as i32, cx as i32, cz as i32, half);
        }
        return;
    }
    for x in x0..=x1 {
        for z in z0..=z1 {
            let (px, pz) = (x as f64, z as f64);
            let t = (((px - ax) * dx + (pz - az) * dz) / len2).clamp(0.0, 1.0);
            let d = ((px - ax - t * dx).powi(2) + (pz - az - t * dz).powi(2)).sqrt();
            if d <= half {
                map.set(x, z);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: Kind, tags: &[(&str, &str)]) -> Spec {
        let tags = tags
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Spec::new(kind, &tags)
    }

    fn line(len: f64) -> Vec<(f64, f64, Hint)> {
        vec![(0.5, 0.5, Hint::Pylon), (len + 0.5, 0.5, Hint::Pylon)]
    }

    /// Every carrier keeps its clearance once past the terminal mouths.
    fn assert_clear(sup: &[Support], spec: &Spec, ground: &dyn Fn(f64, f64) -> f64) {
        for w in sup.windows(2) {
            if let Some((t, short)) = worst_shortfall(&w[0], &w[1], spec, ground, sup) {
                panic!("rope short by {short:.1} at t={t:.2} between {w:?}");
            }
        }
    }

    #[test]
    fn a_plain_chairlift_gets_evenly_spaced_pylons() {
        let s = spec(Kind::Chair, &[]);
        let slope = |x: f64, _z: f64| x * 0.4;
        let sup = plan_supports(&line(420.0), &s, &slope);
        let pylons = sup.iter().filter(|p| p.role == Role::Pylon).count();
        assert!((5..=8).contains(&pylons), "{pylons} pylons");
        assert_eq!(sup.first().unwrap().role, Role::Station);
        assert_eq!(sup.last().unwrap().role, Role::Station);
        assert_clear(&sup, &s, &slope);
    }

    #[test]
    fn a_crest_gets_a_taller_pylon_or_one_of_its_own() {
        let s = spec(Kind::Gondola, &[]);
        let hill = |x: f64, _z: f64| 40.0 - (x - 300.0).abs() * 0.15;
        let sup = plan_supports(&line(600.0), &s, &hill);
        assert_clear(&sup, &s, &hill);
        for p in &sup {
            assert!(p.rope_y - p.ground <= s.max_rope + 1.2, "{p:?}");
        }
    }

    #[test]
    fn a_tramway_spans_a_valley_without_a_pylon_in_it() {
        let s = spec(Kind::CableCar, &[]);
        let valley = |x: f64, _z: f64| {
            if (100.0..700.0).contains(&x) {
                -150.0
            } else {
                0.0
            }
        };
        let sup = plan_supports(&line(800.0), &s, &valley);
        assert!(sup.iter().all(|p| p.ground > -100.0), "{sup:?}");
        assert_clear(&sup, &s, &valley);
    }

    #[test]
    fn mapped_pylons_are_kept_and_crowded_ones_dropped() {
        let s = spec(Kind::Chair, &[]);
        let flat = |_x: f64, _z: f64| 0.0;
        let pts = vec![
            (0.5, 0.5, Hint::Station),
            (60.5, 0.5, Hint::Pylon),
            (63.5, 0.5, Hint::Pylon),
            (130.5, 0.5, Hint::Pylon),
            (190.5, 0.5, Hint::Station),
        ];
        let sup = plan_supports(&pts, &s, &flat);
        let xs: Vec<i32> = sup.iter().map(|p| p.x as i32).collect();
        assert!(
            xs.contains(&60) && xs.contains(&130) && !xs.contains(&63),
            "{xs:?}"
        );
    }

    #[test]
    fn a_clipped_end_is_an_edge_not_a_station() {
        let s = spec(Kind::TBar, &[]);
        let flat = |_x: f64, _z: f64| 0.0;
        let pts = vec![(0.5, 0.5, Hint::Station), (200.5, 0.5, Hint::Edge)];
        let sup = plan_supports(&pts, &s, &flat);
        assert_eq!(sup.last().unwrap().role, Role::Edge);
    }

    #[test]
    fn a_mid_station_splits_the_line() {
        let pts = vec![
            (0.5, 0.5, Hint::Station),
            (100.5, 0.5, Hint::Pylon),
            (200.5, 0.5, Hint::Station),
            (300.5, 0.5, Hint::Pylon),
            (400.5, 0.5, Hint::Station),
        ];
        assert_eq!(sections(&pts), vec![0..=2, 2..=4]);
        assert_eq!(sections(&pts[..2]), vec![0..=1]);
        // A station vertex beside the end is part of the end station.
        let pts = vec![
            (0.5, 0.5, Hint::Station),
            (6.5, 0.5, Hint::Station),
            (200.5, 0.5, Hint::Station),
        ];
        assert_eq!(sections(&pts), vec![0..=2]);
    }

    #[test]
    fn chair_width_follows_occupancy() {
        assert_eq!(
            spec(Kind::Chair, &[("aerialway:occupancy", "6")]).carrier,
            Some(Carrier::Chair {
                half: 2,
                bubble: false
            })
        );
        assert_eq!(
            spec(Kind::Chair, &[("aerialway:occupancy", "2")]).terminal,
            Terminal::Open
        );
    }

    #[test]
    fn corridors_cover_the_line_and_terminals() {
        let bbox = XZBBox::rect_from_min_max(-50, -50, 300, 50).unwrap();
        let node = |id, x, z| crate::osm_parser::ProcessedNode {
            id,
            tags: HashMap::new(),
            x,
            z,
        };
        let way = ProcessedWay {
            id: 1,
            nodes: vec![node(1, 0, 0), node(2, 250, 0)],
            tags: [("aerialway".to_string(), "chair_lift".to_string())]
                .into_iter()
                .collect(),
        };
        let map = clear_corridors(&[ProcessedElement::Way(way)], &bbox);
        assert!(map.contains(125, 8) && !map.contains(125, 20));
        assert!(map.contains(-12, 0));
    }
}
