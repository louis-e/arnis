//! Bundled fountains at OSM amenity=fountain, the largest that fits the room the fountain has.
//!
//! An area gets the largest model that fills its outline, or a basin laid along the outline
//! with a model's centrepiece in it. A point gets its `diameter` or a small default, shrunk
//! until it clears the buildings, roads, paths and water around it. Besides the four
//! hand-built originals, the models are built by `assets/structures/build_fountains.py`.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, structure_base, StructureSchematic};
use crate::block_definitions::*;
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Drinking,
    Basin,
}

/// The stone of a basin laid along an outline, to match the centrepiece in it.
#[derive(Clone, Copy)]
struct Trim {
    rim: Block,
    coping: Block,
    floor: Block,
}

const BRICK_TRIM: Trim = Trim {
    rim: STONE_BRICKS,
    coping: STONE_BRICK_SLAB,
    floor: SMOOTH_STONE,
};
const SANDSTONE_TRIM: Trim = Trim {
    rim: SMOOTH_SANDSTONE,
    coping: CUT_SANDSTONE_SLAB,
    floor: SMOOTH_STONE,
};
const ANDESITE_TRIM: Trim = Trim {
    rim: POLISHED_ANDESITE,
    coping: POLISHED_ANDESITE_SLAB,
    floor: POLISHED_DEEPSLATE,
};
const PLAIN_TRIM: Trim = Trim {
    rim: STONE,
    coping: STONE_BLOCK_SLAB,
    floor: SMOOTH_STONE,
};
const QUARTZ_TRIM: Trim = Trim {
    rim: SMOOTH_QUARTZ,
    coping: SMOOTH_QUARTZ_SLAB,
    floor: SMOOTH_STONE,
};
const DIORITE_TRIM: Trim = Trim {
    rim: POLISHED_DIORITE,
    coping: POLISHED_DIORITE_SLAB,
    floor: SMOOTH_STONE,
};

/// A bundled model; `sink` of its layers lie below the ground it stands on.
struct Spec {
    bytes: &'static [u8],
    sink: i32,
    kind: Kind,
    trough: bool,
    trim: Trim,
}

macro_rules! spec {
    ($file:literal, $sink:expr, $kind:ident, $trough:expr, $trim:expr) => {
        Spec {
            bytes: include_bytes!(concat!("../../assets/structures/", $file, ".schem")),
            sink: $sink,
            kind: Kind::$kind,
            trough: $trough,
            trim: $trim,
        }
    };
}

/// The four hand-built originals come first.
static SPECS: &[Spec] = &[
    spec!("fountain1", 0, Basin, false, BRICK_TRIM),
    spec!("fountain2", 0, Basin, false, BRICK_TRIM),
    spec!("fountain3", 0, Basin, false, ANDESITE_TRIM),
    spec!("fountain4", 0, Basin, false, ANDESITE_TRIM),
    spec!("fountain_drinking_post", 0, Drinking, false, BRICK_TRIM),
    spec!("fountain_drinking_stone", 0, Drinking, false, BRICK_TRIM),
    spec!("fountain_bowl3", 1, Basin, false, BRICK_TRIM),
    spec!("fountain_pedestal3", 1, Basin, false, BRICK_TRIM),
    spec!("fountain_trough5x3", 1, Basin, true, ANDESITE_TRIM),
    spec!("fountain_octagon5", 2, Basin, false, SANDSTONE_TRIM),
    spec!("fountain_tiered7", 2, Basin, false, BRICK_TRIM),
    spec!("fountain_column7", 2, Basin, true, PLAIN_TRIM),
    spec!("fountain_square9", 2, Basin, false, DIORITE_TRIM),
    spec!("fountain_round9", 2, Basin, false, QUARTZ_TRIM),
    spec!("fountain_obelisk11", 2, Basin, false, SANDSTONE_TRIM),
    spec!("fountain_jets13", 2, Basin, false, ANDESITE_TRIM),
    spec!("fountain_tiered13", 2, Basin, false, PLAIN_TRIM),
    spec!("fountain_grand17", 2, Basin, false, BRICK_TRIM),
];

struct Model {
    schem: StructureSchematic,
    sink: i32,
    kind: Kind,
    trough: bool,
    trim: Trim,
    side: i32,
    /// Columns around the anchor, sorted, per quarter turn.
    footprint: [Vec<(i32, i32)>; 4],
    /// The same without a sunk basin's rim: what stands in a basin laid along an outline.
    core: [Vec<(i32, i32)>; 4],
}

fn models() -> &'static [Model] {
    static CELL: OnceLock<Vec<Model>> = OnceLock::new();
    CELL.get_or_init(|| {
        SPECS
            .iter()
            .enumerate()
            .filter_map(|(i, spec)| {
                let schem = match load_structure(spec.bytes) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("fountain model {i} load failed: {e}");
                        return None;
                    }
                };
                // A drinking fountain stands at its post, the tallest column.
                let schem = match spec.kind {
                    Kind::Basin => schem.centered(),
                    Kind::Drinking => schem,
                };
                let footprint: [Vec<(i32, i32)>; 4] =
                    std::array::from_fn(|rot| structure_base(&schem, 0, 0, rot as u8, i32::MAX));
                let core = std::array::from_fn(|rot| {
                    let cols = &footprint[rot];
                    if spec.sink < 2 {
                        return cols.clone();
                    }
                    cols.iter()
                        .copied()
                        .filter(|&(x, z)| {
                            SIDES
                                .iter()
                                .all(|(dx, dz)| cols.binary_search(&(x + dx, z + dz)).is_ok())
                        })
                        .collect()
                });
                Some(Model {
                    side: schem.width.max(schem.length),
                    schem,
                    sink: spec.sink,
                    kind: spec.kind,
                    trough: spec.trough,
                    trim: spec.trim,
                    footprint,
                    core,
                })
            })
            .collect()
    })
}

const SIDES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// What lies at a column around a fountain mapped as a point.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Room {
    Open,
    /// A path or sidewalk, which a basin stays off where it can.
    Path,
    /// A building or a carriageway, which a basin keeps a block clear of.
    Taken,
    Water,
}

/// The room at a column, from the masks every tile shares.
pub fn room_at<'a>(
    editor: &'a WorldEditor,
    roads: &'a crate::floodfill_cache::CoordinateBitmap,
) -> impl Fn(i32, i32) -> Room + 'a {
    let walls = editor.wall_footprints();
    move |x, z| {
        if walls.as_ref().is_some_and(|w| w.contains(x, z))
            || editor.carriageway().is_some_and(|c| c.contains(x, z))
        {
            Room::Taken
        } else if editor.water_at(x, z) {
            Room::Water
        } else if roads.contains(x, z) {
            Room::Path
        } else {
            Room::Open
        }
    }
}

/// A model at a spot and quarter turn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pick {
    model: usize,
    x: i32,
    z: i32,
    rot: u8,
}

impl Pick {
    fn model(&self) -> &'static Model {
        &models()[self.model]
    }

    /// The columns it builds on.
    pub fn columns(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.cells(&self.model().footprint)
    }

    fn core(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.cells(&self.model().core)
    }

    fn cells<'a>(
        &'a self,
        cells: &'a [Vec<(i32, i32)>; 4],
    ) -> impl Iterator<Item = (i32, i32)> + 'a {
        cells[self.rot as usize]
            .iter()
            .map(move |&(dx, dz)| (self.x + dx, self.z + dz))
    }
}

fn fountain_values(tags: &HashMap<String, String>) -> impl Iterator<Item = &str> {
    tags.get("fountain")
        .map_or("", String::as_str)
        .split(';')
        .map(str::trim)
}

/// A fountain to drink from: a post with a bowl, not a basin.
fn is_drinking(tags: &HashMap<String, String>) -> bool {
    fountain_values(tags).any(|v| {
        matches!(
            v,
            "bubbler"
                | "bottle_refill"
                | "drinking_fountain"
                | "water_tap"
                | "tap"
                | "water_dispenser"
                | "nasone"
                | "toret"
                | "roman_wolf"
                | "viktorija_zdenac"
                | "wallace"
                | "basilisk"
                | "borne-fontaine_genevoise"
                | "Muhr-Brunnen"
        )
    })
}

/// A small street fountain to drink from, which in the Alpine towns that tag most of them is
/// a stone trough with a post or a column.
fn is_street_trough(tags: &HashMap<String, String>) -> bool {
    fountain_values(tags).any(|v| {
        matches!(
            v,
            "drinking" | "drinking_water" | "trough" | "stone_block" | "spout"
        )
    })
}

fn prefers_trough(tags: &HashMap<String, String>) -> bool {
    is_street_trough(tags) || tags.get("drinking_water").is_some_and(|v| v == "yes")
}

/// Models of `kind` no wider than `cap`, largest first. The hash turns the order among those
/// of one size, so neighbouring fountains of that size differ.
fn candidates(kind: Kind, cap: i32, h: u64, trough_first: bool) -> Vec<usize> {
    let models = models();
    let mut out = Vec::new();
    for side in (1..=cap.min(64)).rev() {
        let mut same: Vec<usize> = (0..models.len())
            .filter(|&i| models[i].kind == kind && models[i].side == side)
            .collect();
        if same.is_empty() {
            continue;
        }
        let turn = (h % same.len() as u64) as usize;
        same.rotate_left(turn);
        if trough_first {
            same.sort_by_key(|&i| !models[i].trough);
        }
        out.extend(same);
    }
    out
}

/// Side in blocks a point fountain may take: its `diameter` or `width`, else 5 or 7 m, 5 m
/// for a street trough.
fn node_cap(tags: &HashMap<String, String>, scale: f64, h: u64) -> i32 {
    let tagged = ["diameter", "width"]
        .iter()
        .find_map(|k| {
            tags.get(*k)
                .and_then(|v| crate::element_processing::road_width::metres(v))
        })
        .filter(|m| *m < 200.0);
    let metres = tagged.unwrap_or(if h & 1 == 0 || is_street_trough(tags) {
        5.0
    } else {
        7.0
    });
    ((metres * scale).round() as i32).max(1)
}

/// The fountain for a point: the largest model up to its size that stays off buildings,
/// carriageways, paths and water, turned to fit if need be. Nothing in water, or deeper than
/// NUDGE in a building or road.
pub fn pick_node(
    tags: &HashMap<String, String>,
    x: i32,
    z: i32,
    scale: f64,
    room: &dyn Fn(i32, i32) -> Room,
) -> Option<Pick> {
    if is_splash_pad(tags) {
        return None;
    }
    let (x, z) = match room(x, z) {
        Room::Water => return None,
        // Mapped a block or two into the wall it stands against.
        Room::Taken => beside_wall(x, z, room)?,
        Room::Open | Room::Path => (x, z),
    };
    let h = coord_hash(x, z);
    let rot0 = ((h >> 8) & 3) as u8;
    let drinking = is_drinking(tags);
    let (kind, cap) = if drinking {
        (Kind::Drinking, i32::MAX)
    } else {
        (Kind::Basin, node_cap(tags, scale, h))
    };
    let fits = |pick: &Pick, on_paths: bool, walls: Walls| {
        let columns: Vec<(i32, i32)> = pick.columns().collect();
        let free = columns.iter().all(|&(cx, cz)| match room(cx, cz) {
            Room::Open => true,
            Room::Path => on_paths,
            Room::Taken | Room::Water => false,
        });
        if !free || walls == Walls::Any {
            return free;
        }
        let taken: Vec<(i32, i32)> = columns
            .iter()
            .flat_map(|&(cx, cz)| {
                (-1..=1).flat_map(move |dx| (-1..=1).map(move |dz| (cx + dx, cz + dz)))
            })
            .filter(|&(cx, cz)| room(cx, cz) == Room::Taken)
            .collect();
        if walls == Walls::Clear {
            return taken.is_empty();
        }
        // Against one wall, not wedged between two.
        let (x0, x1, z0, z1) = columns.iter().fold(
            (i32::MAX, i32::MIN, i32::MAX, i32::MIN),
            |(a, b, c, d), &(cx, cz)| (a.min(cx), b.max(cx), c.min(cz), d.max(cz)),
        );
        taken.iter().all(|t| t.0 > x1)
            || taken.iter().all(|t| t.0 < x0)
            || taken.iter().all(|t| t.1 > z1)
            || taken.iter().all(|t| t.1 < z0)
    };
    let first_fit = |order: &[usize], on_paths: bool, walls: Walls| {
        order.iter().find_map(|&model| {
            (0..4)
                .map(|turn| Pick {
                    model,
                    x,
                    z,
                    rot: (rot0 + turn) & 3,
                })
                .find(|p| fits(p, on_paths, walls))
        })
    };
    let order = candidates(kind, cap, h >> 4, prefers_trough(tags));
    if drinking {
        return first_fit(&order, true, Walls::Any);
    }
    let least = order.iter().map(|&m| models()[m].side).min();
    let smallest: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&m| Some(models()[m].side) == least)
        .collect();
    let spouts = candidates(Kind::Drinking, i32::MAX, h >> 4, false);
    // A block clear of houses and roads first, then against a wall as a wall fountain stands.
    // Paths drawn right through it are routes across its square. Squeezed in, the smallest
    // basin may take a little path; failing that, a spout and bowl against the wall.
    let tries = if room(x, z) == Room::Path { 4 } else { 2 };
    [
        (false, Walls::Clear),
        (false, Walls::One),
        (true, Walls::Clear),
        (true, Walls::One),
    ][..tries]
        .iter()
        .find_map(|&(on_paths, walls)| first_fit(&order, on_paths, walls))
        .or_else(|| first_fit(&smallest, true, Walls::One))
        .or_else(|| first_fit(&spouts, true, Walls::Any))
}

/// How near the buildings and carriageways around it a point fountain may stand.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Walls {
    Clear,
    One,
    Any,
}

/// How far a point mapped inside a building or road is moved out to the open.
const NUDGE: i32 = 2;

/// The open column nearest (x, z), at most NUDGE away, one with room for a small basin
/// round it where there is one.
fn beside_wall(x: i32, z: i32, room: &dyn Fn(i32, i32) -> Room) -> Option<(i32, i32)> {
    let mut near: Vec<(i32, i32)> = (-NUDGE..=NUDGE)
        .flat_map(|dz| (-NUDGE..=NUDGE).map(move |dx| (x + dx, z + dz)))
        .filter(|&(cx, cz)| matches!(room(cx, cz), Room::Open | Room::Path))
        .collect();
    near.sort_by_key(|&(cx, cz)| ((cx - x).pow(2) + (cz - z).pow(2), cz, cx));
    let roomy = |&(cx, cz): &(i32, i32)| {
        (-1..=1).all(|dx| (-1..=1).all(|dz| room(cx + dx, cz + dz) != Room::Taken))
    };
    near.iter()
        .find(|c| roomy(c))
        .or_else(|| near.first())
        .copied()
}

/// A splash pad is jets in the paving, no basin.
fn is_splash_pad(tags: &HashMap<String, String>) -> bool {
    fountain_values(tags).any(|v| v == "splash_pad")
}

/// Where a splash pad mapped as a point has its jets: a few around it, off buildings and water.
fn splash_jets(x: i32, z: i32, room: &dyn Fn(i32, i32) -> Room) -> Vec<(i32, i32)> {
    if matches!(room(x, z), Room::Taken | Room::Water) {
        return Vec::new();
    }
    [(0, 0), (2, 0), (-2, 0), (0, 2), (0, -2)]
        .iter()
        .map(|&(dx, dz)| (x + dx, z + dz))
        .filter(|&(cx, cz)| matches!(room(cx, cz), Room::Open | Room::Path))
        .collect()
}

/// The columns a fountain mapped as a point builds on.
pub fn node_columns(
    tags: &HashMap<String, String>,
    x: i32,
    z: i32,
    scale: f64,
    room: &dyn Fn(i32, i32) -> Room,
) -> Vec<(i32, i32)> {
    if is_splash_pad(tags) {
        return splash_jets(x, z, room);
    }
    pick_node(tags, x, z, scale, room).map_or_else(Vec::new, |p| p.columns().collect())
}

/// How far into the tiles beside its own a fountain mapped as a point reaches, so each
/// of them builds it and its sunk basin is not covered by their ground.
pub fn node_spread(tags: &HashMap<String, String>, scale: f64) -> Option<i32> {
    (tags.get("amenity").map(String::as_str) == Some("fountain"))
        .then(|| node_reach_bound(tags, scale) + 1)
}

/// How far from where it stands a point fountain with these tags can reach at most.
pub fn node_reach_bound(tags: &HashMap<String, String>, scale: f64) -> i32 {
    if is_splash_pad(tags) {
        return 2;
    }
    let kind = if is_drinking(tags) {
        Kind::Drinking
    } else {
        Kind::Basin
    };
    let cap = node_cap(tags, scale, 0).max(node_cap(tags, scale, 1));
    models()
        .iter()
        .filter(|m| m.kind == Kind::Drinking || (kind == Kind::Basin && m.side <= cap))
        .map(|m| m.schem.max_extent)
        .max()
        .map_or(0, |r| r + NUDGE)
}

/// A fountain's area on a grid of its own.
struct Outline {
    x0: i32,
    z0: i32,
    w: i32,
    l: i32,
    inside: Vec<bool>,
}

impl Outline {
    fn new(cells: &[(i32, i32)]) -> Option<Self> {
        let (x0, z0, x1, z1) = cells.iter().fold(
            (i32::MAX, i32::MAX, i32::MIN, i32::MIN),
            |(a, b, c, d), &(x, z)| (a.min(x), b.min(z), c.max(x), d.max(z)),
        );
        if x0 > x1 {
            return None;
        }
        let (w, l) = (x1 - x0 + 1, z1 - z0 + 1);
        let mut inside = vec![false; (w * l) as usize];
        for &(x, z) in cells {
            inside[((z - z0) * w + (x - x0)) as usize] = true;
        }
        Some(Outline {
            x0,
            z0,
            w,
            l,
            inside,
        })
    }

    fn has(&self, x: i32, z: i32) -> bool {
        let (gx, gz) = (x - self.x0, z - self.z0);
        gx >= 0 && gz >= 0 && gx < self.w && gz < self.l && self.inside[(gz * self.w + gx) as usize]
    }

    fn cells(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        (0..self.l).flat_map(move |gz| {
            (0..self.w)
                .filter(move |&gx| self.inside[(gz * self.w + gx) as usize])
                .map(move |gx| (self.x0 + gx, self.z0 + gz))
        })
    }

    /// Whether a cell has a side open to the outside.
    fn is_rim(&self, x: i32, z: i32) -> bool {
        SIDES.iter().any(|(dx, dz)| !self.has(x + dx, z + dz))
    }

    /// The cell with the largest square around it that stays inside; of those the one
    /// nearest the middle, so a concave outline still gets it inside.
    fn deepest(&self) -> (i32, i32) {
        let (w, l) = (self.w as usize, self.l as usize);
        let mut depth: Vec<i32> = self.inside.iter().map(|&i| i32::from(i)).collect();
        let at = |d: &[i32], x: isize, z: isize| {
            if x < 0 || z < 0 || x >= w as isize || z >= l as isize {
                0
            } else {
                d[z as usize * w + x as usize]
            }
        };
        // Chessboard distance to the outside, in a pass each way.
        for z in 0..l as isize {
            for x in 0..w as isize {
                let i = z as usize * w + x as usize;
                if depth[i] > 0 {
                    depth[i] = [(-1, -1), (0, -1), (1, -1), (-1, 0)]
                        .iter()
                        .map(|(dx, dz)| at(&depth, x + dx, z + dz))
                        .min()
                        .unwrap_or(0)
                        + 1;
                }
            }
        }
        for z in (0..l as isize).rev() {
            for x in (0..w as isize).rev() {
                let i = z as usize * w + x as usize;
                if depth[i] > 0 {
                    let after = [(1, 1), (0, 1), (-1, 1), (1, 0)]
                        .iter()
                        .map(|(dx, dz)| at(&depth, x + dx, z + dz))
                        .min()
                        .unwrap_or(0);
                    depth[i] = depth[i].min(after + 1);
                }
            }
        }
        let n = self.inside.iter().filter(|&&i| i).count().max(1) as i64;
        let (sx, sz) = self.cells().fold((0i64, 0i64), |(a, b), (x, z)| {
            (a + i64::from(x), b + i64::from(z))
        });
        let (cx, cz) = (sx / n, sz / n);
        self.cells()
            .max_by_key(|&(x, z)| {
                let d = depth[((z - self.z0) * self.w + (x - self.x0)) as usize];
                let (dx, dz) = (i64::from(x) - cx, i64::from(z) - cz);
                (
                    d,
                    std::cmp::Reverse(dx * dx + dz * dz),
                    std::cmp::Reverse((x, z)),
                )
            })
            .unwrap_or((self.x0, self.z0))
    }

    /// Basins that fit inside at the deepest cell, largest first, each at its first turn
    /// that does; with `core`, only what of each stands inside a basin has to.
    fn fits(&self, core: bool, sinks: std::ops::RangeInclusive<i32>) -> Vec<Pick> {
        let (x, z) = self.deepest();
        let h = coord_hash(x, z);
        let rot0 = ((h >> 8) & 3) as u8;
        candidates(Kind::Basin, self.w.max(self.l), h >> 4, false)
            .into_iter()
            .filter(|&m| sinks.contains(&models()[m].sink))
            .filter_map(|model| {
                (0..4)
                    .map(|turn| Pick {
                        model,
                        x,
                        z,
                        rot: (rot0 + turn) & 3,
                    })
                    .find(|p| {
                        if core {
                            p.core().all(|(cx, cz)| self.has(cx, cz))
                        } else {
                            p.columns().all(|(cx, cz)| self.has(cx, cz))
                        }
                    })
            })
            .collect()
    }
}

/// How a fountain mapped as an area is built.
#[derive(Debug, PartialEq)]
pub enum AreaPlan {
    /// A model inside the outline.
    Model(Pick),
    /// A basin along the outline, with a model's centrepiece where one fits.
    Basin(Option<Pick>),
    /// Too narrow for a basin: a stone bowl on each cell.
    Bowls,
    /// A splash pad: jets standing in the paving.
    Jets(Vec<(i32, i32)>),
    Nothing,
}

/// A model stands for an outline it fills this much of.
const MODEL_FILLS: f64 = 0.7;

pub fn plan_area(tags: &HashMap<String, String>, cells: &[(i32, i32)]) -> AreaPlan {
    let Some(outline) = Outline::new(cells) else {
        return AreaPlan::Nothing;
    };
    if is_splash_pad(tags) {
        // On a grid of the world, so tiles agree, a block in from the edge.
        return AreaPlan::Jets(
            outline
                .cells()
                .filter(|&(x, z)| x.rem_euclid(3) == 0 && z.rem_euclid(3) == 0)
                .filter(|&(x, z)| !outline.is_rim(x, z))
                .collect(),
        );
    }
    let whole = outline.fits(false, 0..=2);
    let filling = whole
        .iter()
        .find(|p| p.columns().count() as f64 >= cells.len() as f64 * MODEL_FILLS);
    let interior: Vec<(i32, i32)> = outline
        .cells()
        .filter(|&(x, z)| !outline.is_rim(x, z))
        .collect();
    if let Some(&pick) = filling {
        return AreaPlan::Model(pick);
    }
    if !interior.is_empty() {
        // What stands in the water needs a floor of its own or its pedestal on the bottom.
        let centre = Outline::new(&interior).and_then(|o| o.fits(true, 1..=2).first().copied());
        return AreaPlan::Basin(centre);
    }
    match whole.first() {
        Some(&pick) => AreaPlan::Model(pick),
        None if cells.len() <= 9 => AreaPlan::Bowls,
        None => AreaPlan::Nothing,
    }
}

/// An area's filled cells and its closed ring's own, which the fill leaves out, sorted.
pub fn with_ring(fill: &[(i32, i32)], ring: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut cells = fill.to_vec();
    if ring.len() >= 4 && ring.first() == ring.last() {
        for pair in ring.windows(2) {
            let ((ax, az), (bx, bz)) = (pair[0], pair[1]);
            cells.extend(
                crate::bresenham::bresenham_line(ax, 0, az, bx, 0, bz)
                    .into_iter()
                    .map(|(x, _, z)| (x, z)),
            );
        }
    }
    cells.sort_unstable();
    cells.dedup();
    cells
}

/// Builds the fountain mapped as a point at (x, z).
pub fn place_node(
    editor: &mut WorldEditor,
    tags: &HashMap<String, String>,
    x: i32,
    z: i32,
    scale: f64,
    roads: &crate::floodfill_cache::CoordinateBitmap,
) {
    if !editor.place_prop(super::Prop::Fountain) {
        return;
    }
    if is_splash_pad(tags) {
        let jets = splash_jets(x, z, &room_at(editor, roads));
        jets_in_paving(editor, &jets);
        return;
    }
    let pick = {
        let room = room_at(editor, roads);
        pick_node(tags, x, z, scale, &room)
    };
    if let Some(pick) = pick {
        let columns: Vec<(i32, i32)> = pick.columns().collect();
        if let Some(base) = level(editor, &columns) {
            stamp(editor, &pick, base, false);
        }
    }
}

/// Builds the fountain mapped as an area over `cells`.
pub fn place_area(editor: &mut WorldEditor, tags: &HashMap<String, String>, cells: &[(i32, i32)]) {
    if !editor.place_prop(super::Prop::Fountain) {
        return;
    }
    match plan_area(tags, cells) {
        AreaPlan::Model(pick) => {
            let columns: Vec<(i32, i32)> = pick.columns().collect();
            if let Some(base) = level(editor, &columns) {
                stamp(editor, &pick, base, false);
            }
        }
        AreaPlan::Basin(pick) => {
            let (Some(outline), Some(base)) = (Outline::new(cells), level(editor, cells)) else {
                return;
            };
            let trim = pick.map_or(BRICK_TRIM, |p| p.model().trim);
            for &(x, z) in cells {
                if outline.is_rim(x, z) {
                    editor.set_block_absolute(trim.rim, x, base, z, None, Some(&[]));
                    editor.set_block_absolute(trim.coping, x, base + 1, z, None, Some(&[]));
                } else {
                    editor.set_block_absolute(trim.floor, x, base - 1, z, None, Some(&[]));
                    editor.set_block_absolute(WATER, x, base, z, None, Some(&[]));
                }
            }
            if let Some(pick) = pick {
                stamp(editor, &pick, base, true);
            }
        }
        AreaPlan::Bowls => {
            for &(x, z) in cells {
                let y = editor.get_absolute_y(x, 1, z);
                editor.set_block_absolute(WATER_CAULDRON, x, y, z, None, Some(&[]));
            }
        }
        AreaPlan::Jets(jets) => jets_in_paving(editor, &jets),
        AreaPlan::Nothing => {}
    }
}

/// Jets a block or two high standing in the paving, falling water with no source that
/// at most dries up.
fn jets_in_paving(editor: &mut WorldEditor, jets: &[(i32, i32)]) {
    // Luanti and Hytale turn falling water into sources, which would flood the paving.
    if matches!(
        editor.format(),
        crate::world_editor::WorldFormat::LuantiWorld
            | crate::world_editor::WorldFormat::HytaleWorld
    ) {
        return;
    }
    let jet = BlockWithProperties::new(
        WATER,
        Some(fastnbt::Value::Compound(HashMap::from([(
            "level".to_string(),
            fastnbt::Value::String("8".to_string()),
        )]))),
    );
    for &(x, z) in jets {
        let ground = editor.get_absolute_y(x, 0, z);
        let height = 1 + (coord_hash(x, z) >> 3 & 1) as i32;
        for y in ground + 1..=ground + height {
            editor.set_block_with_properties_absolute(jet.clone(), x, y, z, None, None);
        }
    }
}

/// Levels the ground under `columns`: a basin sits on the lowest ground under it, cut at
/// most MAX_CUT into a hill and on a footing where the hill falls away further.
fn level(editor: &mut WorldEditor, columns: &[(i32, i32)]) -> Option<i32> {
    let levels = columns
        .iter()
        .map(|&(cx, cz)| editor.get_ground_level(cx, cz));
    let (low, high) = (levels.clone().min()?, levels.max()?);
    let base = low.max(high - MAX_CUT);
    for &(cx, cz) in columns {
        // A road or another pad already set this column's height; it keeps it.
        if editor.surface_override(cx, cz).is_some() {
            continue;
        }
        let ground = editor.get_ground_level(cx, cz);
        for y in base + 1..=ground {
            editor.set_block_absolute(AIR, cx, y, cz, None, Some(&[]));
        }
        for y in ground + 1..=base {
            editor.set_block_absolute(FOOTING, cx, y, cz, None, Some(&[]));
        }
        editor.register_road_surface_y(cx, cz, base);
    }
    Some(base)
}

/// Stamps the model on ground levelled to `base`. As the centrepiece of a basin laid along an
/// outline, a sunk model leaves out its own floor and rim.
fn stamp(editor: &mut WorldEditor, pick: &Pick, base: i32, centrepiece: bool) {
    let model = pick.model();
    // place_structure would refuse it, after the ground below had been cut.
    if model.schem.max_extent > crate::tile::TILE_EDITOR_HALO {
        return;
    }
    let y = base + 1 - model.sink;
    if !centrepiece || model.sink < 2 {
        place_structure(editor, &model.schem, pick.x, pick.z, y, pick.rot, None);
        return;
    }
    let core = &model.core[pick.rot as usize];
    let schem = StructureSchematic {
        width: model.schem.width,
        length: model.schem.length,
        anchor_x: model.schem.anchor_x,
        anchor_z: model.schem.anchor_z,
        max_extent: model.schem.max_extent,
        voxels: model
            .schem
            .voxels
            .iter()
            .filter(|v| v.1 > 0)
            .filter(|v| {
                let at = turned(&model.schem, v.0, v.2, pick.rot);
                core.binary_search(&at).is_ok()
            })
            .cloned()
            .collect(),
    };
    place_structure(editor, &schem, pick.x, pick.z, y, pick.rot, None);
}

/// Where a voxel column lands around the anchor at quarter turn `rot`.
fn turned(schem: &StructureSchematic, vx: i32, vz: i32, rot: u8) -> (i32, i32) {
    use crate::trees::schematic::rotate_xz;
    let (w, l) = (schem.width, schem.length);
    let (ax, az) = rotate_xz(schem.anchor_x, schem.anchor_z, w, l, rot);
    let (rx, rz) = rotate_xz(vx, vz, w, l, rot);
    (rx - ax, rz - az)
}

/// How far a fountain is cut into a hill at most.
const MAX_CUT: i32 = 3;
/// What a fountain stands on where the hill falls away further than that.
const FOOTING: Block = STONE_BRICKS;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::ground::Ground;
    use std::sync::Arc;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn rect(x0: i32, z0: i32, w: i32, l: i32) -> Vec<(i32, i32)> {
        (z0..z0 + l)
            .flat_map(|z| (x0..x0 + w).map(move |x| (x, z)))
            .collect()
    }

    fn disc(cx: i32, cz: i32, r: f64) -> Vec<(i32, i32)> {
        let n = r.ceil() as i32;
        rect(cx - n, cz - n, 2 * n + 1, 2 * n + 1)
            .into_iter()
            .filter(|&(x, z)| f64::from(x - cx).hypot(f64::from(z - cz)) <= r)
            .collect()
    }

    fn side(pick: &Pick) -> i32 {
        pick.model().side
    }

    fn open(_: i32, _: i32) -> Room {
        Room::Open
    }

    fn prop<'a>(b: &'a BlockWithProperties, key: &str) -> Option<&'a str> {
        match b.properties.as_deref()? {
            fastnbt::Value::Compound(c) => match c.get(key)? {
                fastnbt::Value::String(s) => Some(s),
                _ => None,
            },
            _ => None,
        }
    }

    /// Sides of a block that water held in it cannot pass, as (dx, dy, dz).
    fn full_faces(b: &BlockWithProperties) -> Vec<(i32, i32, i32)> {
        let name = b.block.name();
        let facing = |d: &str| match d {
            "north" => (0, 0, -1),
            "south" => (0, 0, 1),
            "west" => (-1, 0, 0),
            _ => (1, 0, 0),
        };
        let left = |d: &str| match d {
            "north" => "west",
            "west" => "south",
            "south" => "east",
            _ => "north",
        };
        if name.ends_with("_slab") {
            return match prop(b, "type") {
                Some("bottom") => vec![(0, -1, 0)],
                Some("top") => Vec::new(),
                _ => vec![(1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1), (0, -1, 0)],
            };
        }
        if name.ends_with("_stairs") {
            let f = prop(b, "facing").unwrap_or("north");
            let mut faces = if prop(b, "half") == Some("top") {
                Vec::new()
            } else {
                vec![(0, -1, 0)]
            };
            match prop(b, "shape") {
                Some("straight") | None => faces.push(facing(f)),
                Some("inner_left") => faces.extend([facing(f), facing(left(f))]),
                Some("inner_right") => {
                    faces.extend([facing(f), facing(left(left(left(f))))]);
                }
                _ => {}
            }
            return faces;
        }
        Vec::new()
    }

    /// Water let loose by a block changing beside it, run as Minecraft runs it and kept to the
    /// worst case, stays over the model: no source has an open side or air below that leads
    /// anywhere else. In the models built by script, what flows already lands in still water.
    #[test]
    fn every_fountain_loads_and_holds_its_water() {
        assert_eq!(models().len(), SPECS.len(), "a model failed to load");
        let flat = [(1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)];
        for (i, m) in models().iter().enumerate() {
            let voxels: HashMap<(i32, i32, i32), &BlockWithProperties> = m
                .schem
                .voxels
                .iter()
                .map(|(x, y, z, b)| ((*x, *y, *z), b))
                .collect();
            let footprint: std::collections::HashSet<(i32, i32)> =
                voxels.keys().map(|&(x, _, z)| (x, z)).collect();
            assert!(voxels.keys().any(|k| k.1 == 0), "model {i} floats");
            // The hand-built originals keep flowing water that only drains, never runs off.
            let settled = i >= 4;
            let ground = |x: i32, y: i32, z: i32| {
                y < if footprint.contains(&(x, z)) {
                    0
                } else {
                    m.sink
                }
            };
            let block = |c: (i32, i32, i32)| voxels.get(&c).copied();
            let water = |c| block(c).is_some_and(|b| b.block == WATER);
            let source = |c| {
                block(c).is_some_and(|b| {
                    (b.block == WATER && prop(b, "level").unwrap_or("0") == "0")
                        || prop(b, "waterlogged") == Some("true")
                })
            };
            let opens = |c: (i32, i32, i32)| {
                !ground(c.0, c.1, c.2) && (block(c).is_none() || (water(c) && !source(c)))
            };
            let mut todo: Vec<((i32, i32, i32), i32)> = Vec::new();
            for (&(x, y, z), b) in &voxels {
                let name = b.block.name();
                let waterloggable = ["_slab", "_stairs", "_wall", "lightning_rod"]
                    .iter()
                    .any(|s| name.contains(s));
                // Two sources running into it and something solid under it waterlog it.
                let fed = flat
                    .iter()
                    .filter(|d| {
                        let n = (x + d.0, y, z + d.2);
                        source(n)
                            && block(n).is_some_and(|nb| !full_faces(nb).contains(&(-d.0, 0, -d.2)))
                    })
                    .count();
                let below = (x, y - 1, z);
                let soaks = waterloggable
                    && fed >= 2
                    && (ground(x, y - 1, z) || (block(below).is_some() && !water(below)));
                if !source((x, y, z)) && !soaks {
                    continue;
                }
                let shut = if b.block == WATER {
                    Vec::new()
                } else {
                    full_faces(b)
                };
                for d in flat.iter().copied().chain([(0, -1, 0)]) {
                    let n = (x + d.0, y + d.1, z + d.2);
                    if !shut.contains(&d) && opens(n) {
                        assert!(!settled, "model {i}: still water opens at {n:?}");
                        todo.push((n, i32::from(d.1 == 0)));
                    }
                }
            }
            let mut reached: HashMap<(i32, i32, i32), i32> = HashMap::new();
            while let Some((c, dist)) = todo.pop() {
                if reached.get(&c).is_some_and(|&d| d <= dist) {
                    continue;
                }
                reached.insert(c, dist);
                assert!(
                    footprint.contains(&(c.0, c.2)),
                    "model {i}: water runs out at {c:?}"
                );
                let below = (c.0, c.1 - 1, c.2);
                if opens(below) {
                    todo.push((below, 0));
                } else if !source(below) && dist < 7 {
                    for d in flat {
                        let n = (c.0 + d.0, c.1, c.2 + d.2);
                        if opens(n) {
                            todo.push((n, dist + 1));
                        }
                    }
                }
            }
            if !settled {
                continue;
            }
            for &(x, y, z) in voxels.keys() {
                if water((x, y, z)) && !source((x, y, z)) {
                    let mut y = y - 1;
                    while water((x, y, z)) && !source((x, y, z)) {
                        y -= 1;
                    }
                    assert!(source((x, y, z)), "model {i}: water at {x} {y} {z} hangs");
                }
            }
        }
    }

    #[test]
    fn an_area_gets_the_largest_model_that_fills_it() {
        let model = |cells: &[(i32, i32)]| match plan_area(&HashMap::new(), cells) {
            AreaPlan::Model(p) => Some(side(&p)),
            _ => None,
        };
        assert_eq!(
            plan_area(&HashMap::new(), &rect(0, 0, 2, 2)),
            AreaPlan::Bowls
        );
        assert_eq!(model(&rect(0, 0, 3, 3)), Some(3));
        assert_eq!(model(&rect(0, 0, 5, 5)), Some(5));
        assert_eq!(model(&rect(0, 0, 7, 7)), Some(7));
        assert_eq!(model(&rect(0, 0, 9, 9)), Some(9));
        assert_eq!(model(&disc(0, 0, 6.4)), Some(13));
        assert_eq!(model(&disc(0, 0, 8.6)), Some(17));
        assert_eq!(
            model(&disc(0, 0, 11.2)),
            Some(21),
            "a plaza gets the largest"
        );
        assert_eq!(plan_area(&HashMap::new(), &[(0, 0)]), AreaPlan::Bowls);
        assert_eq!(
            plan_area(&HashMap::new(), &rect(0, 0, 1, 12)),
            AreaPlan::Nothing
        );
        // Far more than any model fills: a basin along the outline, the largest centrepiece in it.
        match plan_area(&HashMap::new(), &rect(0, 0, 40, 25)) {
            AreaPlan::Basin(Some(p)) => assert_eq!(side(&p), 17),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            plan_area(&HashMap::new(), &rect(0, 0, 12, 3)),
            AreaPlan::Basin(None)
        ));
    }

    /// Whatever an area gets stays inside its outline, however small or odd the outline.
    #[test]
    fn an_area_fountain_never_reaches_past_its_outline() {
        let mut shapes: Vec<Vec<(i32, i32)>> = Vec::new();
        for w in 1..=24 {
            for l in 1..=24 {
                shapes.push(rect(100, -50, w, l));
            }
        }
        for r in 1..=20 {
            shapes.push(disc(7, 3, f64::from(r) * 0.6));
        }
        // An L, a ring and a diagonal band.
        let mut l_shape = rect(0, 0, 20, 6);
        l_shape.extend(rect(0, 6, 6, 14));
        shapes.push(l_shape);
        let ring: Vec<(i32, i32)> = disc(0, 0, 12.0)
            .into_iter()
            .filter(|&(x, z)| f64::from(x).hypot(f64::from(z)) > 7.0)
            .collect();
        shapes.push(ring);
        shapes.push(
            rect(0, 0, 40, 40)
                .into_iter()
                .filter(|&(x, z)| (x - z).abs() <= 4)
                .collect(),
        );
        for cells in shapes {
            let inside: std::collections::HashSet<(i32, i32)> = cells.iter().copied().collect();
            let built: Vec<(i32, i32)> = match plan_area(&HashMap::new(), &cells) {
                AreaPlan::Model(p) => p.columns().collect(),
                AreaPlan::Basin(p) => {
                    let interior: Vec<(i32, i32)> = cells
                        .iter()
                        .copied()
                        .filter(|&(x, z)| {
                            SIDES
                                .iter()
                                .all(|(dx, dz)| inside.contains(&(x + dx, z + dz)))
                        })
                        .collect();
                    if let Some(p) = p {
                        assert!(
                            p.core().all(|c| interior.contains(&c)),
                            "a centrepiece on the rim"
                        );
                    }
                    cells.clone()
                }
                AreaPlan::Jets(jets) => jets,
                AreaPlan::Bowls | AreaPlan::Nothing => Vec::new(),
            };
            assert!(
                built.iter().all(|c| inside.contains(c)),
                "{} cells: built past the outline",
                cells.len()
            );
        }
    }

    /// A ring too small to fill still has its own cells, so a small basin gets something.
    #[test]
    fn an_area_counts_its_ring() {
        let ring = [(0, 0), (2, 0), (2, 2), (0, 2), (0, 0)];
        let mut square = rect(0, 0, 3, 3);
        square.sort_unstable();
        assert_eq!(with_ring(&[(1, 1)], &ring), square);
        assert!(
            with_ring(&[], &[(0, 0), (5, 0), (5, 5)]).is_empty(),
            "an open way"
        );
    }

    #[test]
    fn a_point_takes_its_diameter_or_a_small_default() {
        let none = tags(&[("amenity", "fountain")]);
        for (x, z) in [(0, 0), (31, 7), (-12, 40), (5, 5), (100, -3)] {
            let p = pick_node(&none, x, z, 1.0, &open).unwrap();
            assert!((3..=7).contains(&side(&p)), "default at {x} {z}");
        }
        let sized = |d: &str, scale: f64| {
            let t = tags(&[("amenity", "fountain"), ("diameter", d)]);
            side(&pick_node(&t, 0, 0, scale, &open).unwrap())
        };
        assert_eq!(sized("3", 1.0), 3);
        assert_eq!(sized("9 m", 1.0), 9);
        assert!((10..=13).contains(&sized("12", 1.0)));
        assert_eq!(sized("20", 1.0), 17);
        assert_eq!(sized("22", 1.0), 21);
        assert!(sized("14", 0.5) <= 7);
        // A bubbler is a post; a fountain tagged for drinking a small trough.
        let bubbler = tags(&[("amenity", "fountain"), ("fountain", "bubbler")]);
        let p = pick_node(&bubbler, 0, 0, 1.0, &open).unwrap();
        assert_eq!(p.model().kind, Kind::Drinking);
        let trough = tags(&[("amenity", "fountain"), ("fountain", "drinking")]);
        let p = pick_node(&trough, 0, 0, 1.0, &open).unwrap();
        assert!(p.model().trough && side(&p) == 5, "{p:?}");
        // A splash pad is jets in the paving: a few around a point, a grid over an area.
        let pad = tags(&[("amenity", "fountain"), ("fountain", "splash_pad")]);
        assert!(pick_node(&pad, 0, 0, 1.0, &open).is_none());
        assert_eq!(node_columns(&pad, 0, 0, 1.0, &open).len(), 5);
        match plan_area(&pad, &rect(0, 0, 12, 12)) {
            AreaPlan::Jets(jets) => assert_eq!(jets.len(), 9),
            other => panic!("{other:?}"),
        }
        // The same point always gets the same fountain.
        assert_eq!(
            pick_node(&none, 31, 7, 1.0, &open),
            pick_node(&none, 31, 7, 1.0, &open)
        );
    }

    /// Beside a house and a footpath a point shrinks or turns until it clears both, the
    /// house by a block.
    #[test]
    fn a_point_keeps_off_buildings_paths_and_water() {
        let big = tags(&[("amenity", "fountain"), ("diameter", "15")]);
        // A house wall four blocks east, a path five blocks south.
        let room = |x: i32, z: i32| {
            if x >= 4 {
                Room::Taken
            } else if z == 5 {
                Room::Path
            } else {
                Room::Open
            }
        };
        let p = pick_node(&big, 0, 0, 1.0, &room).unwrap();
        for (x, z) in p.columns() {
            assert!(x <= 2 && z != 5, "{x} {z}");
        }
        assert!(side(&p) >= 3);
        // On a path with no room either side: the smallest basin, on the path.
        let lane = |x: i32, _: i32| match x.abs() {
            0 => Room::Path,
            1 => Room::Open,
            _ => Room::Taken,
        };
        let p = pick_node(&big, 0, 0, 1.0, &lane);
        assert!(p.is_some_and(|p| p.model().kind == Kind::Drinking), "{p:?}");
        let pond = |_: i32, _: i32| Room::Water;
        assert!(pick_node(&big, 0, 0, 1.0, &pond).is_none());
        let house = |_: i32, _: i32| Room::Taken;
        assert!(pick_node(&big, 0, 0, 1.0, &house).is_none());
        // In a pedestrian street against a house: as large as fits, touching the wall.
        let street = |x: i32, _: i32| if x >= 2 { Room::Taken } else { Room::Path };
        let p = pick_node(
            &tags(&[("amenity", "fountain"), ("diameter", "5")]),
            0,
            0,
            1.0,
            &street,
        );
        let p = p.unwrap();
        assert!(side(&p) >= 3 && p.columns().all(|(x, _)| x <= 1), "{p:?}");
        // Mapped a block into the wall it stands against: built in front of the wall.
        let wall = |x: i32, _: i32| if x >= 0 { Room::Taken } else { Room::Open };
        let p = pick_node(&big, 0, 0, 1.0, &wall).unwrap();
        assert!(p.columns().all(|(x, _)| x < 0), "{p:?}");
        assert!(p.model().kind == Kind::Basin, "{p:?}");
    }

    #[test]
    fn the_reach_bound_covers_every_point_pick() {
        for t in [
            tags(&[("amenity", "fountain")]),
            tags(&[("amenity", "fountain"), ("diameter", "11")]),
            tags(&[("amenity", "fountain"), ("fountain", "drinking")]),
        ] {
            let bound = node_reach_bound(&t, 1.0);
            for (x, z) in (0..40).map(|i| (i * 7, i * 13)) {
                if let Some(p) = pick_node(&t, x, z, 1.0, &open) {
                    assert!(p
                        .columns()
                        .all(|(cx, cz)| (cx - x).abs().max((cz - z).abs()) <= bound));
                }
            }
        }
    }

    fn editor_on(heights: Vec<Vec<f32>>) -> WorldEditor<'static> {
        let bbox = Box::leak(Box::new(XZBBox::rect_from_min_max(0, 0, 63, 63).unwrap()));
        let llbbox = LLBBox::new(47.0, 8.0, 47.001, 8.001).unwrap();
        let mut editor = WorldEditor::new(std::env::temp_dir(), bbox, llbbox);
        editor.set_ground(Arc::new(Ground::new_elevation_test(heights, 64, 64)));
        editor.set_ground_origin(0, 0);
        editor
    }

    fn placed_columns(x: i32, z: i32) -> (Vec<(i32, i32)>, i32) {
        let p = pick_node(&tags(&[("amenity", "fountain")]), x, z, 1.0, &open).unwrap();
        (p.columns().collect(), p.model().sink)
    }

    /// On a hillside the fountain sits on the lowest ground under it, and the rest of its
    /// footprint is cut down to that level instead of burying the uphill side.
    #[test]
    fn a_fountain_on_a_slope_sits_on_its_lowest_ground() {
        // Ground rising one block every two to the east.
        let heights = (0..64)
            .map(|_| (0..64).map(|x| x as f32 / 2.0).collect())
            .collect();
        let mut editor = editor_on(heights);
        let (x, z) = (32, 32);
        let before: Vec<i32> = (0..64).map(|cx| editor.get_ground_level(cx, z)).collect();
        let roads = crate::floodfill_cache::CoordinateBitmap::new_empty();
        place_node(
            &mut editor,
            &tags(&[("amenity", "fountain")]),
            x,
            z,
            1.0,
            &roads,
        );

        let (columns, sink) = placed_columns(x, z);
        let low = columns
            .iter()
            .map(|&(cx, _)| before[cx as usize])
            .min()
            .unwrap();
        assert!(
            columns.iter().any(|&(cx, _)| before[cx as usize] > low + 1),
            "the footprint spans the slope"
        );
        for &(cx, cz) in &columns {
            assert_eq!(
                editor.get_ground_level(cx, cz),
                low,
                "levelled at {cx} {cz}"
            );
        }
        let lowest_block = columns
            .iter()
            .filter_map(|&(cx, cz)| {
                (low - 3..low + 20).find(|&y| editor.block_exists_absolute(cx, y, cz))
            })
            .min();
        assert_eq!(
            lowest_block,
            Some(low + 1 - sink),
            "the basin rests on the lowest ground"
        );
    }

    /// On a steep hillside the basin is cut no more than MAX_CUT into the hill, and the
    /// downhill side stands on a footing instead of a deep pit being dug.
    #[test]
    fn a_fountain_on_a_steep_slope_is_cut_in_a_little_and_footed() {
        // Ground rising two blocks every block to the east.
        let heights = (0..64)
            .map(|_| (0..64).map(|x| x as f32 * 2.0).collect())
            .collect();
        let mut editor = editor_on(heights);
        let (x, z) = (32, 32);
        let before: Vec<i32> = (0..64).map(|cx| editor.get_ground_level(cx, z)).collect();
        let roads = crate::floodfill_cache::CoordinateBitmap::new_empty();
        place_node(
            &mut editor,
            &tags(&[("amenity", "fountain")]),
            x,
            z,
            1.0,
            &roads,
        );

        let (columns, sink) = placed_columns(x, z);
        let ground = |cx: i32| before[cx as usize];
        let high = columns.iter().map(|&(cx, _)| ground(cx)).max().unwrap();
        let low = columns.iter().map(|&(cx, _)| ground(cx)).min().unwrap();
        assert!(high - low > MAX_CUT, "the footprint spans a steep slope");
        let base = high - MAX_CUT;
        for &(cx, cz) in &columns {
            assert_eq!(editor.get_ground_level(cx, cz), base, "level at {cx} {cz}");
            if ground(cx) < base - sink {
                assert_eq!(
                    editor.get_block_absolute(cx, base - sink, cz),
                    Some(FOOTING)
                );
            }
        }
    }

    /// A basin laid along an outline holds still water inside a rim, the centrepiece in it.
    #[test]
    fn a_large_outline_is_laid_as_a_basin() {
        let mut editor = editor_on(vec![vec![10.0; 64]; 64]);
        let cells = rect(10, 10, 40, 25);
        place_area(&mut editor, &HashMap::new(), &cells);
        let base = editor.get_ground_level(10, 10);
        assert_eq!(editor.get_block_absolute(10, base, 20), Some(STONE_BRICKS));
        assert_eq!(editor.get_block_absolute(12, base, 12), Some(WATER));
        assert_eq!(
            editor.get_block_absolute(12, base - 1, 12),
            Some(SMOOTH_STONE)
        );
        // Nothing outside it.
        assert!(!editor.block_exists_absolute(9, base, 20));
        assert!(!editor.block_exists_absolute(50, base + 1, 20));
    }
}
