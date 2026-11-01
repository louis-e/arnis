//! Railway stations: surface platforms, underground halls and stairs from subway entrances.
//! Laid out once for the whole area so tiles draw stations alike and tracks level to them.

use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZBBox;
use crate::element_processing::bridges::{BridgeStructureMap, RailDeck};
use crate::element_processing::rail_track::{straight_cells, track_cells, RailJoins, TrackLevel};
use crate::element_processing::railways::{
    renders_as_rail_bridge, track_level, RAIL_TUNNEL_FLOOR_DEPTH,
};
use crate::element_processing::structure_kit::{concrete_for_colour, mix, with_props};
use crate::floodfill_cache::CoordinateBitmap;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode};
use crate::world_editor::WorldEditor;
use fnv::{FnvHashMap, FnvHashSet};
use rayon::prelude::*;
use std::collections::HashMap;
use std::time::Duration;

/// Furthest a track's centre lies from a platform it serves, in cells.
const TRACK_REACH: i32 = 3;
/// The same under ground, where platforms are mapped more loosely.
const TUNNEL_TRACK_REACH: i32 = 4;
/// Street to a hall's floor: the hall, the walk to the stairs over it and a roof.
pub(crate) const HALL_DEPTH: i32 = 11;
/// Floor to roof of a hall: the platform, four blocks of headroom and the roof.
const HALL_HEIGHT: i32 = 6;
/// Least depth a hall is built at at all: its own height and two blocks over it.
const HALL_MIN_DEPTH: i32 = HALL_HEIGHT + 2;
/// Floor to floor of halls stacked where two lines cross.
pub(crate) const HALL_STACK: i32 = 8;
/// Floor to roof of the walks between a hall and the street.
const WALK_HEIGHT: i32 = 4;
/// Largest platform taken, in cells; anything bigger is a mapping error.
const MAX_PLATFORM_CELLS: usize = 40_000;
/// Cells either side of a surface track trees keep their trunks off (one beside a tram).
const TRACK_TREE_REACH: i32 = 2;
/// Furthest a subway entrance lies from the platform it leads down to, in blocks.
const ENTRANCE_REACH: i32 = 160;
/// Platform cells tried for the foot of the flight into a hall, nearest the entrance first.
const LANDING_TRIES: usize = 48;
/// Steps of the flight from a platform up to the walk over the hall, one per block it climbs.
const HALL_FLIGHT: i32 = HALL_HEIGHT - 1;
/// How close halls of one line lie to share a floor.
const SIBLING_REACH: i32 = 24;
/// Cells either way along a track its heading is taken over.
const HEADING_CELLS: usize = 4;
/// How far from a hall its tracks may run through another before it is stacked under it.
const STACK_REACH: i32 = 48;
/// Two halls whose tracks run within this many degrees are one station, not a crossing.
const PARALLEL_DEG: f64 = 25.0;

/// Blocks nothing in a station overwrites: rails laid by the tracks.
const RAILS: &[Block] = &[
    RAIL,
    RAIL_NORTH_SOUTH,
    RAIL_EAST_WEST,
    RAIL_ASCENDING_EAST,
    RAIL_ASCENDING_WEST,
    RAIL_ASCENDING_NORTH,
    RAIL_ASCENDING_SOUTH,
    RAIL_NORTH_EAST,
    RAIL_NORTH_WEST,
    RAIL_SOUTH_EAST,
    RAIL_SOUTH_WEST,
];

/// Placeholder for open station space until the ground fill is done; walkable if never carved.
const VOID: Block = LIGHT;

/// A column of a station's open space, `lo..=hi`, emptied once the ground is filled.
pub type StationVoid = (i32, i32, i32, i32);

type Xz = (i32, i32);

/// Whether a way or relation is a rail platform rather than a bus stop's.
pub(crate) fn is_rail_platform(tags: &HashMap<String, String>) -> bool {
    let get = |k: &str| tags.get(k).map(String::as_str);
    if get("railway") == Some("platform") {
        return true;
    }
    get("public_transport") == Some("platform")
        && ["train", "subway", "light_rail", "tram", "monorail"]
            .iter()
            .any(|k| get(k) == Some("yes"))
}

/// A number from the start of a tag value: `-2;-3` gives -3, the lowest listed.
fn tag_number(value: &str) -> Option<f64> {
    value
        .split(';')
        .filter_map(|v| {
            let v = v.trim();
            let end = v
                .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
                .unwrap_or(v.len());
            v[..end].parse::<f64>().ok()
        })
        .reduce(f64::min)
}

/// How deep the mapper put a feature: its layer, or else its level.
pub(crate) fn depth_key(tags: &HashMap<String, String>) -> Option<i32> {
    ["layer", "level"]
        .iter()
        .find_map(|k| tags.get(*k).and_then(|v| tag_number(v)))
        .map(|v| v.floor().clamp(-16.0, 16.0) as i32)
}

/// Whether the tags keep a platform out of a tunnel.
pub(crate) fn tagged_surface(tags: &HashMap<String, String>) -> bool {
    tags.get("tunnel").map(String::as_str) == Some("no")
        || matches!(
            tags.get("location").map(String::as_str),
            Some("overground" | "surface")
        )
        || depth_key(tags).is_some_and(|k| k >= 0)
}

/// Whether the tags put a platform under ground.
pub(crate) fn tagged_underground(tags: &HashMap<String, String>) -> bool {
    tags.get("tunnel").is_some_and(|v| v != "no")
        || tags.get("location").map(String::as_str) == Some("underground")
        || depth_key(tags).is_some_and(|k| k < 0)
}

/// A low kerb platform, as trams use: under half a metre, or a tram's with no height given.
pub(crate) fn is_low(tags: &HashMap<String, String>) -> bool {
    let height = tags.get("height").and_then(|h| {
        let n = tag_number(h)?;
        Some(if h.contains("cm") { n / 100.0 } else { n })
    });
    match height {
        Some(h) => h < 0.45,
        None => {
            tags.get("tram").map(String::as_str) == Some("yes")
                && tags.get("train").map(String::as_str) != Some("yes")
        }
    }
}

fn yes(tags: &HashMap<String, String>, key: &str) -> bool {
    tags.get(key).map(String::as_str) == Some("yes")
}

/// A mapped platform and the cells it covers.
struct Platform {
    id: u64,
    cells: Vec<(i32, i32)>,
    low: bool,
    covered: bool,
    shelter: bool,
    name: Option<String>,
    colour: Option<Block>,
    depth: Option<i32>,
    underground: bool,
    surface: bool,
}

/// The cells a closed ring covers, its outline included.
fn ring_cells(ring: &[(i32, i32)], timeout: Option<&Duration>) -> Vec<(i32, i32)> {
    // The fill refuses a ring this big, and its outline alone is no platform.
    let (x0, z0, x1, z1) = bbox_of(ring.iter().copied());
    if i64::from(x1 - x0 + 1) * i64::from(z1 - z0 + 1) > crate::floodfill::MAX_FLOOD_FILL_AREA {
        return Vec::new();
    }
    let mut cells = crate::floodfill::flood_fill_area(ring, timeout);
    for w in ring.windows(2) {
        for (x, _, z) in bresenham_line(w[0].0, 0, w[0].1, w[1].0, 0, w[1].1) {
            cells.push((x, z));
        }
    }
    cells
}

/// The cells a platform covers: its area, or a band along a platform mapped as a line.
fn platform_cells(element: &ProcessedElement, timeout: Option<&Duration>) -> Vec<(i32, i32)> {
    let mut cells: Vec<(i32, i32)> = match element {
        ProcessedElement::Way(way) => {
            let pts: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
            let closed = pts.len() >= 4 && pts.first() == pts.last();
            if closed {
                ring_cells(&pts, timeout)
            } else {
                let half = way
                    .tags
                    .get("width")
                    .and_then(|w| tag_number(w))
                    .map_or(2, |w| ((w / 2.0).round() as i32).clamp(1, 4));
                let mut band = FnvHashSet::default();
                for w in pts.windows(2) {
                    for (x, _, z) in bresenham_line(w[0].0, 0, w[0].1, w[1].0, 0, w[1].1) {
                        for dx in -half..=half {
                            for dz in -half..=half {
                                if dx * dx + dz * dz <= half * half + half {
                                    band.insert((x + dx, z + dz));
                                }
                            }
                        }
                    }
                }
                band.into_iter().collect()
            }
        }
        ProcessedElement::Relation(rel) => {
            let rings = |role: ProcessedMemberRole| {
                let mut rings: Vec<Vec<ProcessedNode>> = rel
                    .members
                    .iter()
                    .filter(|m| m.role == role)
                    .map(|m| m.way.nodes.clone())
                    .collect();
                crate::element_processing::merge_way_segments(&mut rings);
                rings
            };
            let fill = |rings: Vec<Vec<ProcessedNode>>| -> FnvHashSet<(i32, i32)> {
                rings
                    .iter()
                    .filter(|r| r.len() >= 4)
                    .flat_map(|r| {
                        let pts: Vec<(i32, i32)> = r.iter().map(|n| (n.x, n.z)).collect();
                        ring_cells(&pts, timeout)
                    })
                    .collect()
            };
            let outer = fill(rings(ProcessedMemberRole::Outer));
            let inner = fill(rings(ProcessedMemberRole::Inner));
            outer.difference(&inner).copied().collect()
        }
        ProcessedElement::Node(_) => Vec::new(),
    };
    cells.sort_unstable();
    cells.dedup();
    if cells.len() > MAX_PLATFORM_CELLS {
        cells.clear();
    }
    cells
}

fn collect_platforms(elements: &[ProcessedElement], timeout: Option<&Duration>) -> Vec<Platform> {
    let mut out = Vec::new();
    for element in elements {
        let tags = element.tags();
        if matches!(element, ProcessedElement::Node(_)) || !is_rail_platform(tags) {
            continue;
        }
        if ["disused", "abandoned", "construction", "proposed", "razed"]
            .iter()
            .any(|v| tags.get("railway").map(String::as_str) == Some(*v))
        {
            continue;
        }
        let cells = platform_cells(element, timeout);
        if cells.is_empty() {
            continue;
        }
        out.push(Platform {
            id: element.id(),
            cells,
            low: is_low(tags),
            covered: yes(tags, "covered"),
            shelter: yes(tags, "shelter"),
            name: tags.get("name").cloned(),
            colour: tags.get("colour").and_then(|c| concrete_for_colour(c)),
            depth: depth_key(tags),
            underground: tagged_underground(tags),
            surface: tagged_surface(tags),
        });
    }
    out
}

/// One cell of a track near a platform.
#[derive(Clone, Copy, Debug)]
struct TrackCell {
    way: u64,
    x: i32,
    z: i32,
    tunnel: bool,
    /// The deck a bridge's track lies on here.
    deck: Option<i32>,
    /// Which way the track runs here, as a unit vector.
    dir: (f32, f32),
}

/// Cells a set of bits is kept for, by 16-cell block, for the sparse masks of the area.
#[derive(Default)]
pub struct SparseCells {
    blocks: FnvHashMap<(i32, i32), [u64; 4]>,
}

impl SparseCells {
    pub fn set(&mut self, x: i32, z: i32) {
        let bit = (((z & 15) << 4) | (x & 15)) as usize;
        self.blocks.entry((x >> 4, z >> 4)).or_insert([0; 4])[bit >> 6] |= 1 << (bit & 63);
    }

    /// Sets every cell of the square of half-side `r` round (x, z), a row at a time.
    fn set_square(&mut self, x: i32, z: i32, r: i32) {
        for zz in z - r..=z + r {
            let mut x0 = x - r;
            while x0 <= x + r {
                // The run of the row inside one 16-cell block.
                let x1 = (x + r).min(x0 | 15);
                let row = ((zz & 15) << 4) as usize;
                let (lo, hi) = (row + (x0 & 15) as usize, row + (x1 & 15) as usize);
                let block = self.blocks.entry((x0 >> 4, zz >> 4)).or_insert([0; 4]);
                for bit in lo..=hi {
                    block[bit >> 6] |= 1 << (bit & 63);
                }
                x0 = x1 + 1;
            }
        }
    }

    pub fn contains(&self, x: i32, z: i32) -> bool {
        let bit = (((z & 15) << 4) | (x & 15)) as usize;
        self.blocks
            .get(&(x >> 4, z >> 4))
            .is_some_and(|b| b[bit >> 6] & (1 << (bit & 63)) != 0)
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// Where a tunnel track lies in a hall: its floor, and whether the hall builds the bore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TunnelPin {
    pub floor: i32,
    pub in_hall: bool,
}

/// What a surface platform's cell is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Deck {
    /// The walking surface.
    Inner,
    /// The kerb of any side.
    Edge,
    /// The safety line inside a kerb facing a track.
    Line,
    /// A ramp at either end.
    End,
}

/// Furniture standing on a platform cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Prop {
    None,
    Lamp,
    /// A post holding up the canopy.
    Post,
    /// A lantern hung from the canopy.
    Hanging,
    /// A bench whose back faces this way.
    Bench(i8, i8),
    Bin,
    /// A name board's post, or the board itself at the middle.
    BoardPost,
    Board(i8, i8),
    /// A pillar of a hall.
    Pillar,
}

/// One cell of a surface platform.
#[derive(Clone, Copy, Debug)]
struct DeckCell {
    x: i32,
    z: i32,
    /// The block the platform's surface is laid in, or the terrain under a low one.
    top: i32,
    deck: Deck,
    prop: Prop,
    roof: bool,
    /// Under a pillar of an elevated platform.
    pier: bool,
    /// A side away from the tracks, railed off when high up.
    back: bool,
}

/// A surface station: its platforms' cells and the ballast laid round its tracks.
struct SurfaceStation {
    bbox: (i32, i32, i32, i32),
    cells: Vec<DeckCell>,
    /// Ballast cells beside the tracks and the bed they lie at.
    beds: Vec<(i32, i32, i32)>,
    low: bool,
    elevated: bool,
    name: Option<String>,
}

/// What a cell of a hall is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HallPart {
    /// Floor for the tracks, open to the roof.
    Trackbed,
    Platform(Deck),
    Wall,
    /// A wall cell a track runs out through.
    Portal,
}

#[derive(Clone, Copy, Debug)]
struct HallCell {
    x: i32,
    z: i32,
    part: HallPart,
    prop: Prop,
    light: bool,
}

/// The blocks a hall is finished in.
#[derive(Clone, Copy, Debug)]
struct HallStyle {
    wall: Block,
    ceiling: Block,
    floor: Block,
    pillar: Block,
    skirting: Block,
}

const HALL_STYLES: [HallStyle; 3] = [
    HallStyle {
        wall: SMOOTH_QUARTZ,
        ceiling: WHITE_CONCRETE,
        floor: SMOOTH_STONE,
        pillar: QUARTZ_BLOCK,
        skirting: GRAY_CONCRETE,
    },
    HallStyle {
        wall: CALCITE,
        ceiling: SMOOTH_STONE,
        floor: POLISHED_ANDESITE,
        pillar: QUARTZ_BRICKS,
        skirting: POLISHED_DEEPSLATE,
    },
    HallStyle {
        wall: WHITE_TERRACOTTA,
        ceiling: LIGHT_GRAY_CONCRETE,
        floor: SMOOTH_STONE,
        pillar: POLISHED_ANDESITE,
        skirting: GRAY_CONCRETE,
    },
];

const HALL_ACCENTS: [Block; 8] = [
    BLUE_CONCRETE,
    ORANGE_CONCRETE,
    CYAN_CONCRETE,
    GREEN_CONCRETE,
    RED_CONCRETE,
    YELLOW_CONCRETE,
    PURPLE_CONCRETE,
    LIGHT_BLUE_CONCRETE,
];

/// A station hall round the platforms of tunnel tracks.
struct Hall {
    bbox: (i32, i32, i32, i32),
    cells: Vec<HallCell>,
    floor: i32,
    style: HallStyle,
    accent: Block,
    /// Its tracks' heading, to tell a crossing line from the same one.
    heading: (f64, f64),
    /// Footprint cells, for the halls stacked under or over it and the walks.
    footprint: FnvHashSet<(i32, i32)>,
    /// Platform cells, for the stairs down to it.
    platform: FnvHashSet<(i32, i32)>,
    /// Edge cells of the platforms, which the stairs keep off.
    platform_edge: FnvHashSet<(i32, i32)>,
}

/// Street flight, walk over the hall and flight down through its roof onto a platform.
struct Entrance {
    bbox: (i32, i32, i32, i32),
    style: HallStyle,
    accent: Block,
    cells: Vec<EntranceCell>,
}

#[derive(Clone, Copy, Debug)]
struct EntranceCell {
    x: i32,
    z: i32,
    part: EntrancePart,
}

#[derive(Clone, Copy, Debug)]
enum EntrancePart {
    /// A street flight step, filled down to `base`; roofed at `roof` or open to the street.
    Stair {
        y: i32,
        facing: i8,
        base: i32,
        roof: Option<i32>,
    },
    /// A side wall from `base` up to `top`, or up to the street with a railing on it.
    Wall { base: i32, top: Option<i32> },
    /// A cell of the walk: its floor, and whether a light is let into its roof.
    Walk { floor: i32, light: bool },
    /// A step of the flight down into the hall, open up to `top`.
    HallStair {
        y: i32,
        facing: i8,
        base: i32,
        top: i32,
    },
    /// Glass beside the flight into the hall, from `lo` to `hi`.
    Glass { lo: i32, hi: i32 },
}

/// The stations of an area, laid out before anything is placed.
#[derive(Default)]
pub struct StationIndex {
    surface: Vec<SurfaceStation>,
    halls: Vec<Hall>,
    entrances: Vec<Entrance>,
    /// Rail beds of surface tracks along a platform, by way and cell.
    beds: FnvHashMap<(u64, i32, i32), i32>,
    /// Floors of tunnel tracks through a hall, by way and cell.
    floors: FnvHashMap<(u64, i32, i32), TunnelPin>,
    /// Floors of the joints next to a deep hall, shared by the tunnel ways meeting there.
    joints: FnvHashMap<(i32, i32), i32>,
    /// Platform surfaces, registered in each editor before anything is placed.
    levels: Vec<(i32, i32, i32)>,
    /// Cells trees keep their trunks off: beside surface tracks and on platforms.
    tree_free: SparseCells,
    /// Surface platform cells, kept clear of catenary masts.
    on_platform: SparseCells,
    /// Columns a hall or an entrance is dug into, which cave geodes keep out of.
    dug: SparseCells,
}

/// Union-find over indices.
struct Groups(Vec<usize>);

impl Groups {
    fn new(n: usize) -> Self {
        Self((0..n).collect())
    }
    fn find(&mut self, i: usize) -> usize {
        let mut r = i;
        while self.0[r] != r {
            r = self.0[r];
        }
        let mut c = i;
        while self.0[c] != r {
            let next = self.0[c];
            self.0[c] = r;
            c = next;
        }
        r
    }
    fn join(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb)] = ra.min(rb);
        }
    }
    fn sets(&mut self) -> Vec<Vec<usize>> {
        let mut by_root: FnvHashMap<usize, Vec<usize>> = FnvHashMap::default();
        for i in 0..self.0.len() {
            let r = self.find(i);
            by_root.entry(r).or_default().push(i);
        }
        let mut sets: Vec<Vec<usize>> = by_root.into_values().collect();
        sets.sort_unstable();
        sets
    }
}

/// A local frame along the main direction of a set of cells.
#[derive(Clone, Copy, Debug)]
struct Frame {
    cx: f64,
    cz: f64,
    ux: f64,
    uz: f64,
}

impl Frame {
    fn of(cells: impl Iterator<Item = (i32, i32)> + Clone) -> Self {
        let n = cells.clone().count().max(1) as f64;
        let (sx, sz) = cells
            .clone()
            .fold((0.0, 0.0), |(a, b), (x, z)| (a + x as f64, b + z as f64));
        let (cx, cz) = (sx / n, sz / n);
        let (mut xx, mut zz, mut xz) = (0.0, 0.0, 0.0);
        for (x, z) in cells {
            let (dx, dz) = (x as f64 - cx, z as f64 - cz);
            xx += dx * dx;
            zz += dz * dz;
            xz += dx * dz;
        }
        let angle = 0.5 * (2.0 * xz).atan2(xx - zz);
        Self {
            cx,
            cz,
            ux: angle.cos(),
            uz: angle.sin(),
        }
    }

    fn along(&self, x: i32, z: i32) -> f64 {
        (x as f64 - self.cx) * self.ux + (z as f64 - self.cz) * self.uz
    }

    fn across(&self, x: i32, z: i32) -> f64 {
        -(x as f64 - self.cx) * self.uz + (z as f64 - self.cz) * self.ux
    }

    /// The grid step nearest the frame's direction.
    fn grid_along(&self) -> (i32, i32) {
        if self.ux.abs() >= self.uz.abs() {
            (self.ux.signum() as i32, 0)
        } else {
            (0, self.uz.signum() as i32)
        }
    }
}

const NEIGHBOURS4: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

fn dilate(cells: &FnvHashSet<(i32, i32)>, r: i32) -> FnvHashSet<(i32, i32)> {
    let mut out = FnvHashSet::with_capacity_and_hasher(cells.len() * 2, Default::default());
    for &(x, z) in cells {
        for dx in -r..=r {
            for dz in -r..=r {
                out.insert((x + dx, z + dz));
            }
        }
    }
    out
}

/// Whether two boxes come within `r` of each other.
fn boxes_within(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32), r: i32) -> bool {
    a.0 - r <= b.2 && b.0 - r <= a.2 && a.1 - r <= b.3 && b.1 - r <= a.3
}

fn bbox_of(cells: impl Iterator<Item = (i32, i32)>) -> (i32, i32, i32, i32) {
    cells.fold(
        (i32::MAX, i32::MAX, i32::MIN, i32::MIN),
        |(a, b, c, d), (x, z)| (a.min(x), b.min(z), c.max(x), d.max(z)),
    )
}

/// Item frame facing for a grid step: 2 north, 3 south, 4 west, 5 east.
fn facing_of((dx, dz): (i32, i32)) -> i8 {
    match (dx, dz) {
        (0, -1) => 2,
        (0, _) => 3,
        (-1, _) => 4,
        _ => 5,
    }
}

fn step_of(facing: i8) -> (i32, i32) {
    match facing {
        2 => (0, -1),
        3 => (0, 1),
        4 => (-1, 0),
        _ => (1, 0),
    }
}

fn stair_facing(facing: i8) -> &'static str {
    match facing {
        2 => "north",
        3 => "south",
        4 => "west",
        _ => "east",
    }
}

/// Inputs the layout reads from the rest of the prescan.
pub struct StationInputs<'a> {
    pub editor: &'a WorldEditor<'a>,
    pub xzbbox: &'a XZBBox,
    pub bridges: &'a BridgeStructureMap,
    pub tunnel_footprint: &'a CoordinateBitmap,
    pub building_footprints: &'a CoordinateBitmap,
    pub road_mask: &'a CoordinateBitmap,
    pub rail_joins: &'a RailJoins,
    pub scale: f64,
    pub timeout: Option<&'a Duration>,
    /// Halls are built at all: the mode builds objects and the scale leaves room for them.
    pub underground: bool,
}

impl StationIndex {
    /// Lays out every station of the area, and the corridors trees keep off along the tracks.
    pub fn build(elements: &[ProcessedElement], inputs: &StationInputs) -> Self {
        let mut index = StationIndex::default();
        let platforms = collect_platforms(elements, inputs.timeout);

        // Only cells near a platform are indexed; elsewhere tracks just mark the tree corridor.
        let near: FnvHashSet<(i32, i32)> = platforms
            .iter()
            .flat_map(|p| p.cells.iter())
            .flat_map(|&(x, z)| {
                let (bx, bz) = (x >> 4, z >> 4);
                (-1..=1).flat_map(move |dx| (-1..=1).map(move |dz| (bx + dx, bz + dz)))
            })
            .collect();
        let mut tracks: FnvHashMap<(i32, i32), Vec<TrackCell>> = FnvHashMap::default();
        let mut tunnel_ways: FnvHashMap<u64, Vec<(i32, i32)>> = FnvHashMap::default();
        let mut tunnel_nodes: FnvHashMap<u64, Vec<u64>> = FnvHashMap::default();
        for element in elements {
            let ProcessedElement::Way(way) = element else {
                continue;
            };
            let Some(level) = track_level(way) else {
                continue;
            };
            let cells = track_cells(way).0;
            if level == TrackLevel::Surface {
                // A tram runs down the street, its trees a step off the rail.
                let reach = if way.tags.get("railway").map(String::as_str) == Some("tram") {
                    1
                } else {
                    TRACK_TREE_REACH
                };
                for &(x, z) in &cells {
                    index.tree_free.set_square(x, z, reach);
                }
            }
            if near.is_empty() {
                continue;
            }
            let decks: Option<Vec<i32>> = renders_as_rail_bridge(way)
                .then(|| inputs.bridges.rail_deck(way.id))
                .flatten()
                .map(|deck| match deck {
                    RailDeck::Own(d) => d.ys.clone(),
                    RailDeck::Carried(ys) => ys.clone(),
                });
            if level == TrackLevel::Tunnel
                && cells.iter().any(|&(x, z)| near.contains(&(x >> 4, z >> 4)))
            {
                tunnel_ways.insert(way.id, cells.clone());
                tunnel_nodes.insert(way.id, way.nodes.iter().map(|n| n.id).collect());
            }
            for (i, &(x, z)) in cells.iter().enumerate() {
                if !near.contains(&(x >> 4, z >> 4)) {
                    continue;
                }
                let deck = decks
                    .as_ref()
                    .and_then(|ys| ys.get(i).or(ys.last()).copied());
                let (a, b) = (
                    cells[i.saturating_sub(HEADING_CELLS)],
                    cells[(i + HEADING_CELLS).min(cells.len() - 1)],
                );
                let (dx, dz) = ((b.0 - a.0) as f32, (b.1 - a.1) as f32);
                let len = dx.hypot(dz).max(1e-3);
                tracks.entry((x, z)).or_default().push(TrackCell {
                    way: way.id,
                    x,
                    z,
                    tunnel: level == TrackLevel::Tunnel,
                    deck,
                    dir: (dx / len, dz / len),
                });
            }
        }
        if platforms.is_empty() {
            return index;
        }

        // The tracks each platform serves: those running along it, not one crossing it.
        let adjacent: Vec<Vec<TrackCell>> = platforms
            .par_iter()
            .map(|p| {
                let frame = Frame::of(p.cells.iter().copied());
                // A short or square platform has no heading of its own to go by.
                let (lo, hi) = p
                    .cells
                    .iter()
                    .fold((f64::MAX, f64::MIN), |(a, b), &(x, z)| {
                        let t = frame.along(x, z);
                        (a.min(t), b.max(t))
                    });
                let (slo, shi) = p
                    .cells
                    .iter()
                    .fold((f64::MAX, f64::MIN), |(a, b), &(x, z)| {
                        let s = frame.across(x, z);
                        (a.min(s), b.max(s))
                    });
                let headed = hi - lo > 2.0 * (shi - slo).max(1.0);
                let along = |t: &TrackCell| {
                    !headed
                        || ((t.dir.0 as f64 * frame.ux + t.dir.1 as f64 * frame.uz).abs()
                            >= PARALLEL_DEG.to_radians().cos())
                };
                let mut seen = FnvHashSet::default();
                let mut out = Vec::new();
                let cell_set: FnvHashSet<(i32, i32)> = p.cells.iter().copied().collect();
                for &(x, z) in &p.cells {
                    // Reach is taken from the edge; inside only a track through the platform counts.
                    let edge = NEIGHBOURS4
                        .iter()
                        .any(|&(dx, dz)| !cell_set.contains(&(x + dx, z + dz)));
                    let r = if edge { TUNNEL_TRACK_REACH } else { 0 };
                    for dx in -r..=r {
                        for dz in -r..=r {
                            for t in tracks.get(&(x + dx, z + dz)).into_iter().flatten() {
                                let reach = if t.tunnel {
                                    TUNNEL_TRACK_REACH
                                } else {
                                    TRACK_REACH
                                };
                                if dx.abs().max(dz.abs()) <= reach
                                    && along(t)
                                    && seen.insert((t.way, t.x, t.z))
                                {
                                    out.push(*t);
                                }
                            }
                        }
                    }
                }
                out
            })
            .collect();
        // Tags decide first, so a tram stop over a subway tunnel stays at the surface.
        let underground: Vec<bool> = platforms
            .iter()
            .zip(&adjacent)
            .map(|(p, adj)| {
                if p.underground {
                    true
                } else if p.low || p.surface {
                    false
                } else {
                    adj.iter().filter(|t| t.tunnel).count() * 2 > adj.len()
                }
            })
            .collect();
        let adjacent: Vec<Vec<TrackCell>> = adjacent
            .into_iter()
            .zip(&underground)
            .map(|(adj, &under)| adj.into_iter().filter(|t| t.tunnel == under).collect())
            .collect();

        // Platforms beside the same stretch of track are one station.
        let mut groups = Groups::new(platforms.len());
        let mut owner: FnvHashMap<(u64, i32, i32), usize> = FnvHashMap::default();
        for (i, adj) in adjacent.iter().enumerate() {
            for t in adj {
                if let Some(&j) = owner.get(&(t.way, t.x, t.z)) {
                    if underground[i] == underground[j] {
                        groups.join(i, j);
                    }
                } else {
                    owner.insert((t.way, t.x, t.z), i);
                }
            }
        }
        let sets = groups.sets();
        let mut hall_sets = Vec::new();
        for set in sets {
            if underground[set[0]] {
                // A platform tagged under ground with no tunnel beside it has nothing to serve.
                if inputs.underground && set.iter().any(|&i| !adjacent[i].is_empty()) {
                    hall_sets.push(set);
                }
            } else {
                index.lay_surface(&platforms, &adjacent, &set, inputs);
            }
        }
        index.lay_halls(
            &platforms,
            &adjacent,
            hall_sets,
            &tracks,
            &tunnel_nodes,
            inputs,
        );
        index.lay_joints(&tunnel_ways, inputs);
        index.lay_entrances(elements, inputs);
        index
    }

    /// Lowers joints next to deep halls the way's own straight cells cannot climb out of.
    fn lay_joints(
        &mut self,
        tunnel_ways: &FnvHashMap<u64, Vec<(i32, i32)>>,
        inputs: &StationInputs,
    ) {
        if self.floors.is_empty() {
            return;
        }
        let editor = inputs.editor;
        let lowest = crate::world_editor::min_y() + 1;
        let mut ways: Vec<u64> = self.floors.keys().map(|k| k.0).collect();
        ways.sort_unstable();
        ways.dedup();
        for way in ways {
            let Some(cells) = tunnel_ways.get(&way) else {
                continue;
            };
            let n = cells.len();
            if n < 2 {
                continue;
            }
            let ends = inputs.rail_joins.ends(way);
            let straight = straight_cells(
                cells,
                ends[0].beyond.first().copied(),
                ends[1].beyond.first().copied(),
            );
            for (e, end) in ends.iter().enumerate() {
                if !end.shared || end.meets_surface {
                    continue;
                }
                let order: Vec<usize> = if e == 0 {
                    (0..n).collect()
                } else {
                    (0..n).rev().collect()
                };
                let at = order[0];
                let Some((k, floor)) = order.iter().enumerate().find_map(|(k, &i)| {
                    self.floors
                        .get(&(way, cells[i].0, cells[i].1))
                        .map(|p| (k, p.floor))
                }) else {
                    continue;
                };
                let climbs = order[1..k.max(1)].iter().filter(|&&i| straight[i]).count() as i32;
                let (x, z) = cells[at];
                let ground = editor
                    .terrain_level(x, z)
                    .unwrap_or(editor.get_ground_level(x, z));
                let surface = editor
                    .lowest_carved_bed(x, z, 2)
                    .map_or(ground, |b| b.min(ground));
                let deep = (surface - RAIL_TUNNEL_FLOOR_DEPTH).max(lowest);
                let reach = floor + climbs / 2;
                if reach < deep {
                    let j = self.joints.entry((x, z)).or_insert(reach);
                    *j = (*j).min(reach);
                }
            }
        }
    }

    /// A surface station: its platforms levelled with the tracks beside them.
    fn lay_surface(
        &mut self,
        platforms: &[Platform],
        adjacent: &[Vec<TrackCell>],
        set: &[usize],
        inputs: &StationInputs,
    ) {
        let editor = inputs.editor;
        let ground = |x: i32, z: i32| {
            editor
                .terrain_level(x, z)
                .unwrap_or(editor.get_ground_level(x, z))
        };
        let tracks: Vec<TrackCell> = set
            .iter()
            .flat_map(|&i| adjacent[i].iter().copied())
            .filter(|t| !t.tunnel)
            .collect();
        let track_at: FnvHashSet<(i32, i32)> = tracks.iter().map(|t| (t.x, t.z)).collect();
        // The ballast either side of a track is no platform.
        let bed_zone = dilate(&track_at, 1);
        let mut cells: Vec<(i32, i32)> = set
            .iter()
            .flat_map(|&i| platforms[i].cells.iter().copied())
            .filter(|c| !bed_zone.contains(c))
            .collect();
        cells.sort_unstable();
        cells.dedup();
        if cells.is_empty() {
            return;
        }
        let first = &platforms[set[0]];
        let low = set.iter().all(|&i| platforms[i].low);
        let frame = Frame::of(cells.iter().copied());
        let decks: Vec<(i32, i32, i32)> = tracks
            .iter()
            .filter_map(|t| t.deck.map(|d| (t.x, t.z, d + 1)))
            .collect();
        let elevated = !low && decks.len() * 2 > tracks.len().max(1);

        // The deck's level, or a linear fit to the ground that stays flat on gentle ground.
        let samples: Vec<(f64, f64)> = if elevated {
            decks
                .iter()
                .map(|&(x, z, y)| (frame.along(x, z), y as f64))
                .collect()
        } else {
            cells
                .iter()
                .map(|&(x, z)| (frame.along(x, z), (ground(x, z) + 1) as f64))
                .collect()
        };
        let n = samples.len().max(1) as f64;
        let mean_t = samples.iter().map(|s| s.0).sum::<f64>() / n;
        let mean_y = samples.iter().map(|s| s.1).sum::<f64>() / n;
        let (lo, hi) = samples
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), s| (a.min(s.1), b.max(s.1)));
        let slope = if hi - lo <= 2.0 {
            0.0
        } else {
            let var: f64 = samples.iter().map(|s| (s.0 - mean_t).powi(2)).sum();
            let cov: f64 = samples
                .iter()
                .map(|s| (s.0 - mean_t) * (s.1 - mean_y))
                .sum();
            (cov / var.max(1.0)).clamp(-0.12, 0.12)
        };
        let level_at = |x: i32, z: i32| -> i32 {
            (mean_y + slope * (frame.along(x, z) - mean_t)).round() as i32
        };

        // The tracks lie level with the platform, their ballast under the rail.
        let mut beds: Vec<(i32, i32, i32)> = Vec::new();
        if !low && !elevated {
            for t in &tracks {
                if t.deck.is_some() {
                    continue;
                }
                let bed = level_at(t.x, t.z) - 1;
                self.beds.insert((t.way, t.x, t.z), bed);
            }
            let cell_set: FnvHashSet<(i32, i32)> = cells.iter().copied().collect();
            for &(x, z) in &bed_zone {
                if track_at.contains(&(x, z)) || inputs.road_mask.contains(x, z) {
                    continue;
                }
                // Only the ballast between a track and its platform.
                let by_platform = NEIGHBOURS4
                    .iter()
                    .any(|&(dx, dz)| cell_set.contains(&(x + dx, z + dz)));
                if by_platform {
                    beds.push((x, z, level_at(x, z) - 1));
                }
            }
            beds.sort_unstable();
        }

        let set_cells: FnvHashSet<(i32, i32)> = cells.iter().copied().collect();
        let (t_min, t_max) = cells.iter().fold((f64::MAX, f64::MIN), |(a, b), &(x, z)| {
            let t = frame.along(x, z);
            (a.min(t), b.max(t))
        });
        let roofed_by_building = cells
            .iter()
            .filter(|&&(x, z)| inputs.building_footprints.contains(x, z))
            .count()
            * 3
            > cells.len();
        let covered = set.iter().any(|&i| platforms[i].covered) && !roofed_by_building;
        let sheltered = set.iter().any(|&i| platforms[i].shelter) && !roofed_by_building;
        let t_mid = (t_min + t_max) / 2.0;
        let under_roof = |t: f64| -> bool {
            if covered {
                t - t_min > 3.0 && t_max - t > 3.0
            } else if sheltered {
                (t - t_mid).abs() <= if low { 2.5 } else { 10.0 }
            } else {
                false
            }
        };

        let decks_of: Vec<Deck> = cells
            .iter()
            .map(|&(x, z)| {
                let outside = |dx: i32, dz: i32| !set_cells.contains(&(x + dx, z + dz));
                let boundary = NEIGHBOURS4.iter().any(|&(dx, dz)| outside(dx, dz));
                let faces_track = |x: i32, z: i32| {
                    NEIGHBOURS4
                        .iter()
                        .any(|&(dx, dz)| bed_zone.contains(&(x + dx, z + dz)))
                };
                let t = frame.along(x, z);
                if boundary && !faces_track(x, z) && (t - t_min < 1.0 || t_max - t < 1.0) {
                    Deck::End
                } else if boundary {
                    Deck::Edge
                } else if NEIGHBOURS4.iter().any(|&(dx, dz)| {
                    let (nx, nz) = (x + dx, z + dz);
                    let nb = NEIGHBOURS4
                        .iter()
                        .any(|&(ex, ez)| !set_cells.contains(&(nx + ex, nz + ez)));
                    nb && faces_track(nx, nz)
                }) {
                    Deck::Line
                } else {
                    Deck::Inner
                }
            })
            .collect();

        // Slices across the platform, each with its middle cell, carry the furniture.
        let mut slices: FnvHashMap<i32, Vec<usize>> = FnvHashMap::default();
        for (k, &(x, z)) in cells.iter().enumerate() {
            slices
                .entry((frame.along(x, z) - t_min).round() as i32)
                .or_default()
                .push(k);
        }
        let mut props = vec![Prop::None; cells.len()];
        let mut roofs = vec![false; cells.len()];
        let mut piers = vec![false; cells.len()];
        let (ax, az) = frame.grid_along();
        let side = (-az, ax);
        let mut keys: Vec<i32> = slices.keys().copied().collect();
        keys.sort_unstable();
        let board_slice = ((t_mid - t_min).round()) as i32;
        let has_name = first.name.is_some() && !low;
        for &t in &keys {
            let slice = &slices[&t];
            let width = slice.len() as i32;
            let mid = |k: &usize| frame.across(cells[*k].0, cells[*k].1);
            let (s_lo, s_hi) = slice
                .iter()
                .map(mid)
                .fold((f64::MAX, f64::MIN), |(a, b), s| (a.min(s), b.max(s)));
            let s_mid = (s_lo + s_hi) / 2.0;
            let Some(&centre) = slice
                .iter()
                .filter(|&&k| decks_of[k] == Deck::Inner)
                .min_by(|&&a, &&b| {
                    (mid(&a) - s_mid)
                        .abs()
                        .total_cmp(&(mid(&b) - s_mid).abs())
                        .then(cells[a].cmp(&cells[b]))
                })
            else {
                continue;
            };
            let t_abs = t_min + t as f64;
            let roofed = under_roof(t_abs);
            for &k in slice {
                if roofed && decks_of[k] != Deck::Edge && decks_of[k] != Deck::End {
                    roofs[k] = true;
                }
            }
            if elevated && t.rem_euclid(6) == 3 {
                piers[centre] = true;
            }
            let near_board = has_name && (t - board_slice).abs() <= 1;
            props[centre] = if near_board {
                if t == board_slice {
                    Prop::Board(facing_of(side), facing_of((-side.0, -side.1)))
                } else if roofed {
                    Prop::None
                } else {
                    Prop::BoardPost
                }
            } else if roofed && t.rem_euclid(8) == 0 {
                Prop::Post
            } else if roofed && !low && t.rem_euclid(8) == 4 {
                Prop::Hanging
            } else if !roofed && !low && t.rem_euclid(14) == 7 {
                Prop::Lamp
            } else if width >= 5 && t.rem_euclid(16) == 3 {
                Prop::Bench(facing_of(side), facing_of((-side.0, -side.1)))
            } else if width >= 4 && t.rem_euclid(16) == 5 {
                Prop::Bin
            } else if low && roofed && t.rem_euclid(3) == 1 {
                Prop::Bench(facing_of(side), facing_of(side))
            } else {
                Prop::None
            };
        }

        let mut out = Vec::with_capacity(cells.len());
        for (k, &(x, z)) in cells.iter().enumerate() {
            let back = !elevated
                || !NEIGHBOURS4.iter().any(|&(dx, dz)| {
                    bed_zone.contains(&(x + dx, z + dz)) || track_at.contains(&(x + dx, z + dz))
                });
            out.push(DeckCell {
                x,
                z,
                top: if low { 0 } else { level_at(x, z) },
                deck: decks_of[k],
                prop: props[k],
                roof: roofs[k],
                pier: piers[k],
                back: back && decks_of[k] == Deck::Edge,
            });
            if !low && !elevated {
                self.levels.push((x, z, level_at(x, z)));
            }
            self.tree_free.set(x, z);
            self.on_platform.set(x, z);
        }
        let bbox = bbox_of(
            out.iter()
                .map(|c| (c.x, c.z))
                .chain(beds.iter().map(|b| (b.0, b.1))),
        );
        self.surface.push(SurfaceStation {
            bbox,
            cells: out,
            beds,
            low,
            elevated,
            name: first.name.clone(),
        });
    }

    /// Station halls round the platforms of tunnel tracks, stacked where lines cross.
    fn lay_halls(
        &mut self,
        platforms: &[Platform],
        adjacent: &[Vec<TrackCell>],
        sets: Vec<Vec<usize>>,
        tracks: &FnvHashMap<(i32, i32), Vec<TrackCell>>,
        tunnel_nodes: &FnvHashMap<u64, Vec<u64>>,
        inputs: &StationInputs,
    ) {
        let editor = inputs.editor;
        struct Draft {
            platforms: Vec<usize>,
            core: FnvHashSet<(i32, i32)>,
            members: FnvHashSet<u64>,
            depth: i32,
            heading: (f64, f64),
        }
        let heading_of = |cells: &[TrackCell]| -> (f64, f64) {
            let f = Frame::of(cells.iter().map(|t| (t.x, t.z)));
            (f.ux, f.uz)
        };
        let drafts: Vec<Draft> = sets
            .into_iter()
            .map(|set| {
                let tunnel: Vec<TrackCell> = set
                    .iter()
                    .flat_map(|&i| adjacent[i].iter().copied())
                    .filter(|t| t.tunnel)
                    .collect();
                let mut core: FnvHashSet<(i32, i32)> = set
                    .iter()
                    .flat_map(|&i| platforms[i].cells.iter().copied())
                    .collect();
                core.extend(tunnel.iter().map(|t| (t.x, t.z)));
                Draft {
                    depth: set
                        .iter()
                        .filter_map(|&i| platforms[i].depth)
                        .min()
                        .unwrap_or(-1),
                    members: tunnel.iter().map(|t| t.way).collect(),
                    heading: heading_of(&tunnel),
                    platforms: set,
                    core,
                }
            })
            .collect();

        // Halls of the same line run on into each other; crossing lines stay apart.
        let parallel = |a: (f64, f64), b: (f64, f64)| {
            (a.0 * b.0 + a.1 * b.1).abs() >= PARALLEL_DEG.to_radians().cos()
        };
        let draft_boxes: Vec<_> = drafts
            .iter()
            .map(|d| bbox_of(d.core.iter().copied()))
            .collect();
        let mut joined = Groups::new(drafts.len());
        for i in 0..drafts.len() {
            let mut grown = None;
            for j in i + 1..drafts.len() {
                let (a, b) = (&drafts[i], &drafts[j]);
                if !parallel(a.heading, b.heading)
                    || (a.depth - b.depth).abs() > 1
                    || !boxes_within(draft_boxes[i], draft_boxes[j], 4)
                {
                    continue;
                }
                let grown = grown.get_or_insert_with(|| dilate(&a.core, 4));
                if b.core.iter().any(|c| grown.contains(c)) {
                    joined.join(i, j);
                }
            }
        }
        let mut slots: Vec<Option<Draft>> = drafts.into_iter().map(Some).collect();
        let drafts: Vec<Draft> = joined
            .sets()
            .into_iter()
            .filter_map(|set| {
                let mut a = slots[set[0]].take()?;
                for b in set[1..].iter().filter_map(|&j| slots[j].take()) {
                    a.platforms.extend(b.platforms);
                    a.core.extend(b.core);
                    a.members.extend(b.members);
                    a.depth = a.depth.min(b.depth);
                }
                Some(a)
            })
            .collect();

        let lowest = crate::world_editor::min_y() + 1;
        let surface_at = |x: i32, z: i32| {
            let g = editor
                .terrain_level(x, z)
                .unwrap_or(editor.get_ground_level(x, z));
            editor.lowest_carved_bed(x, z, 0).map_or(g, |b| b.min(g))
        };
        let mut halls: Vec<(Draft, FnvHashSet<Xz>, i32)> = drafts
            .into_iter()
            .map(|d| {
                let footprint = dilate(&d.core, 2);
                let surface = footprint
                    .iter()
                    .map(|&(x, z)| surface_at(x, z))
                    .min()
                    .unwrap_or(0);
                (d, footprint, surface)
            })
            .collect();
        // Shallowest first, so a deeper line goes under the halls already placed.
        halls.sort_by(|a, b| {
            b.0.depth.cmp(&a.0.depth).then_with(|| {
                platforms[a.0.platforms[0]]
                    .id
                    .cmp(&platforms[b.0.platforms[0]].id)
            })
        });
        // The tunnel tracks through each footprint, where they cross it.
        let through: Vec<Vec<TrackCell>> = halls
            .iter()
            .map(|(_, footprint, _)| {
                let mut cells: Vec<TrackCell> = footprint
                    .iter()
                    .flat_map(|c| tracks.get(c).into_iter().flatten())
                    .filter(|t| t.tunnel)
                    .copied()
                    .collect();
                cells.sort_unstable_by_key(|t| (t.way, t.x, t.z));
                cells
            })
            .collect();
        let boxes: Vec<(i32, i32, i32, i32)> = halls
            .iter()
            .map(|(_, fp, _)| bbox_of(fp.iter().copied()))
            .collect();
        // Each hall's line: its own tracks and the ways joined to them inside its footprint.
        let line: Vec<FnvHashSet<u64>> = halls
            .iter()
            .zip(&through)
            .map(|((draft, _, _), cells)| {
                let mut ways: Vec<u64> = cells.iter().map(|t| t.way).collect();
                ways.dedup();
                let mut line = draft.members.clone();
                let mut nodes: FnvHashSet<u64> = line
                    .iter()
                    .filter_map(|w| tunnel_nodes.get(w))
                    .flatten()
                    .copied()
                    .collect();
                loop {
                    let joined: Vec<u64> = ways
                        .iter()
                        .copied()
                        .filter(|w| {
                            !line.contains(w)
                                && tunnel_nodes
                                    .get(w)
                                    .is_some_and(|ns| ns.iter().any(|n| nodes.contains(n)))
                        })
                        .collect();
                    if joined.is_empty() {
                        break;
                    }
                    for w in joined {
                        nodes.extend(tunnel_nodes[&w].iter().copied());
                        line.insert(w);
                    }
                }
                line
            })
            .collect();
        // Halls of one line side by side, as single-track tubes are, share the deeper floor.
        let mut siblings = Groups::new(halls.len());
        for a in 0..halls.len() {
            for b in a + 1..halls.len() {
                let (da, db) = (&halls[a].0, &halls[b].0);
                if boxes_within(boxes[a], boxes[b], SIBLING_REACH)
                    && parallel(da.heading, db.heading)
                    && (da.depth - db.depth).abs() <= 1
                {
                    siblings.join(a, b);
                }
            }
        }
        let sibling_sets = siblings.sets();
        let sibling: Vec<usize> = (0..halls.len()).map(|k| siblings.find(k)).collect();
        let along = |t: &TrackCell, k: usize| {
            let h = halls[k].0.heading;
            (t.dir.0 as f64 * h.0 + t.dir.1 as f64 * h.1).abs() >= PARALLEL_DEG.to_radians().cos()
        };
        let near = |t: &TrackCell, bx: (i32, i32, i32, i32)| {
            t.x >= bx.0 - STACK_REACH
                && t.x <= bx.2 + STACK_REACH
                && t.z >= bx.1 - STACK_REACH
                && t.z <= bx.3 + STACK_REACH
        };
        // A track of hall `b` crossing hall `a` too close to `b` to dive under `a`.
        let crosses = |a: usize, b: usize| {
            through[a].iter().any(|t| {
                halls[b].0.members.contains(&t.way)
                    && !halls[a].0.members.contains(&t.way)
                    && !along(t, a)
                    && near(t, boxes[b])
            })
        };
        // Stacked if footprints meet or a line crosses; one line's halls never stack.
        let stacked = |a: usize, b: usize| -> bool {
            if sibling[a] == sibling[b] || !boxes_within(boxes[a], boxes[b], STACK_REACH) {
                return false;
            }
            let (da, db) = (&halls[a].0, &halls[b].0);
            let joined = db.members.iter().any(|w| line[a].contains(w))
                || da.members.iter().any(|w| line[b].contains(w));
            if joined && parallel(da.heading, db.heading) {
                return false;
            }
            let (fa, fb) = (&halls[a].1, &halls[b].1);
            let (small, big) = if fa.len() <= fb.len() {
                (fa, fb)
            } else {
                (fb, fa)
            };
            small.iter().any(|c| big.contains(c)) || crosses(a, b) || crosses(b, a)
        };
        let mut placed: Vec<(usize, i32)> = Vec::new();
        let mut floors: Vec<Option<i32>> = Vec::new();
        for (k, (_, _, surface)) in halls.iter().enumerate() {
            let mut floor = (surface - HALL_DEPTH).max(lowest);
            let mut ok = surface - floor >= HALL_MIN_DEPTH;
            for &(j, other) in &placed {
                if stacked(j, k) {
                    floor = floor.min(other - HALL_STACK);
                    ok &= floor >= lowest;
                }
            }
            floors.push(ok.then_some(floor));
            if ok {
                placed.push((k, floor));
            }
        }

        for _ in 0..4 {
            let before = floors.clone();
            for set in &sibling_sets {
                if let Some(low) = set.iter().filter_map(|&k| floors[k]).min() {
                    for &k in set {
                        if floors[k].is_some() {
                            floors[k] = Some(low);
                        }
                    }
                }
            }
            for k in 0..halls.len() {
                for j in 0..k {
                    if let (Some(other), Some(floor)) = (floors[j], floors[k]) {
                        if stacked(j, k) && floor > other - HALL_STACK {
                            let lower = other - HALL_STACK;
                            floors[k] = (lower >= lowest).then_some(lower);
                        }
                    }
                }
            }
            if floors == before {
                break;
            }
        }

        // Every hall's own tracks keep its floor through it.
        let mut level_tracks: Vec<FnvHashSet<(i32, i32)>> =
            vec![FnvHashSet::default(); halls.len()];
        let mut halls_of: FnvHashMap<u64, Vec<usize>> = FnvHashMap::default();
        for (k, (draft, _, _)) in halls.iter().enumerate() {
            let Some(floor) = floors[k] else {
                continue;
            };
            for t in through[k].iter().filter(|t| draft.members.contains(&t.way)) {
                self.floors.insert(
                    (t.way, t.x, t.z),
                    TunnelPin {
                        floor,
                        in_hall: true,
                    },
                );
                level_tracks[k].insert((t.x, t.z));
            }
            for &way in &draft.members {
                halls_of.entry(way).or_default().push(k);
            }
        }
        // Other ways: level through a hall if they carry its line on or run along it at its
        // floor, over it if their own hall is stacked above, else under it (or through, with
        // no room below).
        for (k, (draft, _, _)) in halls.iter().enumerate() {
            let Some(floor) = floors[k] else {
                continue;
            };
            let foreign: Vec<TrackCell> = through[k]
                .iter()
                .filter(|t| !draft.members.contains(&t.way))
                .copied()
                .collect();
            for cells in foreign.chunk_by(|a, b| a.way == b.way) {
                let way = cells[0].way;
                let own = halls_of.get(&way).map_or(&[][..], Vec::as_slice);
                let level = line[k].contains(&way)
                    || own.iter().any(|&j| floors[j] == Some(floor))
                    || (own.is_empty()
                        && cells.iter().filter(|t| along(t, k)).count() * 2 > cells.len());
                if !level
                    && own
                        .iter()
                        .any(|&j| floors[j].is_some_and(|f| f > floor) && stacked(j, k))
                {
                    continue;
                }
                for t in cells {
                    let key = (t.way, t.x, t.z);
                    if self.floors.get(&key).is_some_and(|p| p.in_hall) {
                        continue;
                    }
                    let under = floor - HALL_STACK;
                    let pin = if !level && under >= lowest {
                        TunnelPin {
                            floor: self.floors.get(&key).map_or(under, |p| p.floor.min(under)),
                            in_hall: false,
                        }
                    } else {
                        level_tracks[k].insert((t.x, t.z));
                        TunnelPin {
                            floor,
                            in_hall: true,
                        }
                    };
                    self.floors.insert(key, pin);
                }
            }
        }

        for (k, (draft, footprint, _)) in halls.into_iter().enumerate() {
            let Some(floor) = floors[k] else {
                continue;
            };
            let interior = dilate(&draft.core, 1);
            let level_tracks = std::mem::take(&mut level_tracks[k]);
            let bed_zone = dilate(&level_tracks, 1);
            let platform_cells: FnvHashSet<(i32, i32)> = interior
                .iter()
                .copied()
                .filter(|c| !bed_zone.contains(c))
                .collect();
            let frame = Frame::of(draft.core.iter().copied());

            let part_of = |x: i32, z: i32| -> HallPart {
                if !interior.contains(&(x, z)) {
                    return if bed_zone.contains(&(x, z)) {
                        HallPart::Portal
                    } else {
                        HallPart::Wall
                    };
                }
                if bed_zone.contains(&(x, z)) {
                    return HallPart::Trackbed;
                }
                let faces_track = |x: i32, z: i32| {
                    NEIGHBOURS4
                        .iter()
                        .any(|&(dx, dz)| bed_zone.contains(&(x + dx, z + dz)))
                };
                HallPart::Platform(if faces_track(x, z) {
                    Deck::Edge
                } else if NEIGHBOURS4.iter().any(|&(dx, dz)| {
                    faces_track(x + dx, z + dz) && platform_cells.contains(&(x + dx, z + dz))
                }) {
                    Deck::Line
                } else {
                    Deck::Inner
                })
            };

            let mut cells: Vec<HallCell> = footprint
                .iter()
                .map(|&(x, z)| HallCell {
                    x,
                    z,
                    part: part_of(x, z),
                    prop: Prop::None,
                    light: false,
                })
                .collect();
            cells.sort_unstable_by_key(|c| (c.x, c.z));

            // Across each slice, runs of platform carry the pillars, benches and lights.
            let t0 = cells
                .iter()
                .map(|c| frame.along(c.x, c.z))
                .fold(f64::MAX, f64::min);
            let mut slices: FnvHashMap<i32, Vec<usize>> = FnvHashMap::default();
            for (i, c) in cells.iter().enumerate() {
                if matches!(c.part, HallPart::Platform(_)) {
                    slices
                        .entry((frame.along(c.x, c.z) - t0).round() as i32)
                        .or_default()
                        .push(i);
                }
            }
            let (ax, az) = frame.grid_along();
            let side = (-az, ax);
            for (&t, slice) in &slices {
                let mut slice = slice.clone();
                slice.sort_by(|&a, &b| {
                    frame
                        .across(cells[a].x, cells[a].z)
                        .total_cmp(&frame.across(cells[b].x, cells[b].z))
                });
                let across: Vec<f64> = slice
                    .iter()
                    .map(|&i| frame.across(cells[i].x, cells[i].z))
                    .collect();
                let mut runs: Vec<Vec<usize>> = Vec::new();
                for (k, &i) in slice.iter().enumerate() {
                    match runs.last_mut() {
                        Some(run) if across[k] - across[k - 1] < 1.6 => run.push(i),
                        _ => runs.push(vec![i]),
                    }
                }
                for run in &runs {
                    let width = run.len();
                    let centre = run[width / 2];
                    let lit_row =
                        |i: usize| width >= 7 && (i == width / 2 - 2 || i == width / 2 + 2);
                    if t.rem_euclid(3) == 1 {
                        for (i, &c) in run.iter().enumerate() {
                            if (width < 7 && i == width / 2) || lit_row(i) {
                                cells[c].light = true;
                            }
                        }
                    }
                    if width >= 6 && t.rem_euclid(8) == 4 {
                        cells[centre].prop = Prop::Pillar;
                    } else if width >= 5 && t.rem_euclid(8) == 0 && width / 2 >= 1 {
                        cells[centre].prop =
                            Prop::Bench(facing_of(side), facing_of((-side.0, -side.1)));
                    } else if width >= 4 && t.rem_euclid(16) == 2 {
                        cells[centre].prop = Prop::Bin;
                    }
                }
            }
            // Lights over the tracks too, sparser.
            for c in cells.iter_mut() {
                if c.part == HallPart::Trackbed && level_tracks.contains(&(c.x, c.z)) {
                    let t = (frame.along(c.x, c.z) - t0).round() as i32;
                    c.light = t.rem_euclid(6) == 1;
                }
            }

            let seed = draft
                .platforms
                .iter()
                .map(|&i| platforms[i].id)
                .min()
                .unwrap_or(0);
            let name_seed = draft
                .platforms
                .iter()
                .find_map(|&i| platforms[i].name.as_deref())
                .map_or(seed, |n| {
                    n.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
                        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
                    })
                });
            let style = HALL_STYLES[(mix(name_seed, 1) % HALL_STYLES.len() as u64) as usize];
            let accent = draft
                .platforms
                .iter()
                .find_map(|&i| platforms[i].colour)
                .unwrap_or(HALL_ACCENTS[(mix(name_seed, 2) % HALL_ACCENTS.len() as u64) as usize]);
            for c in &cells {
                self.dug.set(c.x, c.z);
            }
            let bbox = bbox_of(cells.iter().map(|c| (c.x, c.z)));
            let platform_edge = cells
                .iter()
                .filter(|c| matches!(c.part, HallPart::Platform(Deck::Edge)))
                .map(|c| (c.x, c.z))
                .collect();
            self.halls.push(Hall {
                bbox,
                cells,
                floor,
                style,
                accent,
                heading: draft.heading,
                platform_edge,
                footprint,
                platform: platform_cells,
            });
        }
    }

    /// Stairs from each subway entrance down to the nearest hall it can reach.
    fn lay_entrances(&mut self, elements: &[ProcessedElement], inputs: &StationInputs) {
        if self.halls.is_empty() {
            return;
        }
        let editor = inputs.editor;
        let nodes: Vec<&ProcessedNode> = elements
            .iter()
            .filter_map(|e| match e {
                ProcessedElement::Node(n)
                    if matches!(
                        n.tags.get("railway").map(String::as_str),
                        Some("subway_entrance" | "train_station_entrance")
                    ) =>
                {
                    Some(n)
                }
                _ => None,
            })
            .collect();
        if nodes.is_empty() {
            return;
        }
        let (carriageway, fenced) = carriageway_near(elements, &nodes, inputs.scale);
        let surface_at = |x: i32, z: i32| {
            let g = editor
                .terrain_level(x, z)
                .unwrap_or(editor.get_ground_level(x, z));
            editor.lowest_carved_bed(x, z, 0).map_or(g, |b| b.min(g))
        };
        // The halls over each footprint cell, for the walks of the halls under them.
        let mut cover: FnvHashMap<(i32, i32), Vec<usize>> = FnvHashMap::default();
        for (g, hall) in self.halls.iter().enumerate() {
            for &c in &hall.footprint {
                cover.entry(c).or_default().push(g);
            }
        }
        let ctx = EntranceContext {
            inputs,
            carriageway: &carriageway,
            fenced: &fenced,
            cover: &cover,
        };
        // Routed in parallel, then accepted in order; a clash is re-routed round the taken cells.
        let routes: Vec<Option<Route>> = nodes
            .par_iter()
            .map(|n| self.route_entrance(n, &ctx, &FnvHashSet::default()))
            .collect();
        let mut taken: FnvHashSet<(i32, i32)> = FnvHashSet::default();
        for (node, route) in nodes.iter().zip(routes) {
            let route = match route {
                Some(r) if !r.cells_used().any(|c| taken.contains(&c)) => Some(r),
                Some(_) => self.route_entrance(node, &ctx, &taken),
                None => None,
            };
            let Some(route) = route else {
                continue;
            };
            taken.extend(route.cells_used());
            self.build_entrance(route, &surface_at);
        }
    }

    /// The way down from one entrance, if any fits clear of `taken`.
    fn route_entrance(
        &self,
        node: &ProcessedNode,
        ctx: &EntranceContext,
        taken: &FnvHashSet<(i32, i32)>,
    ) -> Option<Route> {
        let inputs = ctx.inputs;
        let editor = inputs.editor;
        let (carriageway, fenced, cover) = (ctx.carriageway, ctx.fenced, ctx.cover);
        let surface_at = |x: i32, z: i32| {
            let g = editor
                .terrain_level(x, z)
                .unwrap_or(editor.get_ground_level(x, z));
            editor.lowest_carved_bed(x, z, 0).map_or(g, |b| b.min(g))
        };
        let in_world = |x: i32, z: i32| {
            inputs
                .xzbbox
                .contains(&crate::coordinate_system::cartesian::XZPoint::new(x, z))
        };
        let (ex, ez) = (node.x, node.z);
        if !in_world(ex, ez) || inputs.building_footprints.contains(ex, ez) {
            return None;
        }
        // The nearest platform any hall has.
        let (h, _) = self
            .halls
            .iter()
            .enumerate()
            .filter(|(_, hall)| {
                let b = hall.bbox;
                ex >= b.0 - ENTRANCE_REACH
                    && ex <= b.2 + ENTRANCE_REACH
                    && ez >= b.1 - ENTRANCE_REACH
                    && ez <= b.3 + ENTRANCE_REACH
            })
            .filter_map(|(h, hall)| {
                hall.platform
                    .iter()
                    .map(|&(x, z)| {
                        let (dx, dz) = ((x - ex) as i64, (z - ez) as i64);
                        dx * dx + dz * dz
                    })
                    .min()
                    .map(|d| (h, d))
            })
            .filter(|&(_, d)| d <= (ENTRANCE_REACH as i64).pow(2))
            .min_by_key(|&(h, d)| (d, h))?;
        let hall = &self.halls[h];
        let floor = hall.floor;
        let walk = floor + HALL_HEIGHT;
        let roof = walk + WALK_HEIGHT;
        let street = surface_at(ex, ez);
        let steps = street - walk - 1;
        if steps < 1 {
            return None;
        }
        // Memoised per entrance.
        let mut surface_memo: FnvHashMap<(i32, i32), i32> = FnvHashMap::default();
        let mut surface = |c: (i32, i32)| {
            *surface_memo
                .entry(c)
                .or_insert_with(|| surface_at(c.0, c.1))
        };
        let mut free_memo: FnvHashMap<(i32, i32), bool> = FnvHashMap::default();
        let mut free = |c: (i32, i32)| {
            *free_memo.entry(c).or_insert_with(|| {
                let (x, z) = c;
                in_world(x, z)
                    && (!inputs.tunnel_footprint.contains(x, z) || hall.footprint.contains(&c))
                    && !cover.get(&c).is_some_and(|gs| {
                        gs.iter().any(|&g| g != h && self.halls[g].floor >= floor)
                    })
                    && !taken.contains(&c)
            })
        };

        // Feet of the flight into the hall: cells of a platform with room either side.
        let frame_dir = Frame {
            cx: 0.0,
            cz: 0.0,
            ux: hall.heading.0,
            uz: hall.heading.1,
        }
        .grid_along();
        let mut landings: Vec<(i64, (i32, i32))> = hall
            .platform
            .iter()
            .map(|&(x, z)| {
                let (dx, dz) = ((x - ex) as i64, (z - ez) as i64);
                (dx * dx + dz * dz, (x, z))
            })
            .collect();
        landings.sort_unstable();
        let mut flights: Vec<Flight> = Vec::new();
        for &(_, p) in &landings {
            if flights.len() >= LANDING_TRIES {
                break;
            }
            for d in [frame_dir, (-frame_dir.0, -frame_dir.1)] {
                // Clear of upper halls and the stairs of entrances taken before.
                let Some(f) = Flight::fit(hall, p, d) else {
                    continue;
                };
                if f.cells
                    .iter()
                    .copied()
                    .chain(f.sides.iter().map(|s| s.0))
                    .all(&mut free)
                {
                    flights.push(f);
                    break;
                }
            }
        }
        if flights.is_empty() {
            return None;
        }

        // The flights from the street that fit, one per heading and side of the post.
        let mut street_flights: Vec<(Xz, Xz, Xz)> = Vec::new();
        for e in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            for side in [1, -1] {
                let q = (-e.1 * side, e.0 * side);
                let c0 = (ex + 2 * q.0, ez + 2 * q.1);
                let at = |j: i32, l: i32| (c0.0 + e.0 * j + q.0 * l, c0.1 + e.1 * j + q.1 * l);
                // Open while a roof would not fit under the street.
                let fits = (0..=steps + 1).all(|j| {
                    (-2..=2).all(|l| {
                        let c = at(j, l);
                        if !in_world(c.0, c.1)
                            || inputs.building_footprints.contains(c.0, c.1)
                            || carriageway.contains(&c)
                            || editor.is_lc_water(c.0, c.1)
                            || !free(c)
                        {
                            return false;
                        }
                        let s = surface(c);
                        let step = j.clamp(1, steps);
                        let open = street - step + 4 > s - 1;
                        // An open stairwell would undercut a track or platform beside it.
                        if open && self.tree_free.contains(c.0, c.1) {
                            return false;
                        }
                        if open {
                            j > steps || (s - street).abs() <= 1
                        } else {
                            j < 1 || s >= street - step + 5
                        }
                    })
                });
                // The way out at the top: level with the street or under it, and open.
                let entry = fits
                    && (-1..=1).all(|l| {
                        let c = at(-1, l);
                        in_world(c.0, c.1)
                            && !inputs.building_footprints.contains(c.0, c.1)
                            && !fenced.contains(&c)
                            && !self.on_platform.contains(c.0, c.1)
                            && !editor.is_lc_water(c.0, c.1)
                            && surface(c) <= street
                    });
                if entry {
                    street_flights.push((e, q, c0));
                }
            }
        }
        if street_flights.is_empty() {
            return None;
        }

        // Shortest first; the first whose walk fits wins.
        let mut routes: Vec<(i32, usize, usize, bool)> = Vec::new();
        for (fi, flight) in flights.iter().enumerate() {
            for (si, &(e, _, c0)) in street_flights.iter().enumerate() {
                let b = (c0.0 + e.0 * (steps + 1), c0.1 + e.1 * (steps + 1));
                let t = flight.top;
                let est = (b.0 - t.0).abs() + (b.1 - t.1).abs();
                for x_first in [true, false] {
                    routes.push((est, fi, si, x_first));
                }
            }
        }
        routes.sort_unstable();
        let mut chosen = None;
        for &(_, fi, si, x_first) in &routes {
            let flight = &flights[fi];
            let (e, q, c0) = street_flights[si];
            let at = |j: i32, l: i32| (c0.0 + e.0 * j + q.0 * l, c0.1 + e.1 * j + q.1 * l);
            let flight_cells: FnvHashSet<(i32, i32)> = (1..=steps)
                .flat_map(|j| (-1..=1).map(move |l| (j, l)))
                .map(|(j, l)| at(j, l))
                .collect();
            // The street flight comes down beside the flight into the hall, never over it.
            let hall_flight: FnvHashSet<(i32, i32)> = flight
                .cells
                .iter()
                .copied()
                .chain(flight.sides.iter().map(|s| s.0))
                .chain([flight.top])
                .collect();
            if (0..=steps + 1).any(|j| (-2..=2).any(|l| hall_flight.contains(&at(j, l)))) {
                continue;
            }
            // The walk may pass the stairwells, not run down either of them.
            let path = l_path(at(steps + 1, 0), flight.top, x_first);
            if path.iter().skip(1).any(|c| flight_cells.contains(c))
                || path[..path.len() - 1]
                    .iter()
                    .any(|c| flight.cells.contains(c))
            {
                continue;
            }
            let walk_set: FnvHashSet<(i32, i32)> = path.iter().copied().collect();
            if dilate(&walk_set, 2)
                .into_iter()
                .any(|c| !free(c) || surface(c) < roof + 1)
            {
                continue;
            }
            let opening: Vec<(i32, i32)> = (0..=steps + 1)
                .flat_map(|j| (-2..=2).map(move |l| (j, l)))
                .chain((-1..=1).map(|l| (-1, l)))
                .map(|(j, l)| at(j, l))
                .collect();
            chosen = Some((fi, e, q, c0, path, opening, flight_cells));
            break;
        }
        let (fi, e, q, c0, path, opening, flight_cells) = chosen?;
        Some(Route {
            hall: h,
            street,
            steps,
            e,
            q,
            c0,
            flight: flights[fi].clone(),
            path,
            opening,
            flight_cells,
        })
    }

    /// Lays out the cells of a routed entrance and keeps the hall's furniture off its stairs.
    fn build_entrance(&mut self, route: Route, surface_at: &impl Fn(i32, i32) -> i32) {
        let Route {
            hall: h,
            street,
            steps,
            e,
            q,
            c0,
            flight,
            path,
            opening,
            flight_cells,
        } = route;
        let floor = self.halls[h].floor;
        let walk = floor + HALL_HEIGHT;
        let roof = walk + WALK_HEIGHT;
        let at = |j: i32, l: i32| (c0.0 + e.0 * j + q.0 * l, c0.1 + e.1 * j + q.1 * l);
        let mut cells: Vec<EntranceCell> = Vec::new();
        let climb = facing_of((-e.0, -e.1));
        let roof_at = |j: i32, c: (i32, i32)| {
            let y = street - j;
            (y + 4 < surface_at(c.0, c.1)).then_some(y + 4)
        };
        // The top step lies in the pavement, so the last rise onto it is half a block too.
        for l in -1..=1 {
            let c = at(0, l);
            cells.push(EntranceCell {
                x: c.0,
                z: c.1,
                part: EntrancePart::Stair {
                    y: street,
                    facing: climb,
                    base: street,
                    roof: None,
                },
            });
        }
        for j in 1..=steps {
            for l in -1..=1 {
                let c = at(j, l);
                cells.push(EntranceCell {
                    x: c.0,
                    z: c.1,
                    part: EntrancePart::Stair {
                        y: street - j,
                        facing: climb,
                        base: walk,
                        roof: roof_at(j, c),
                    },
                });
            }
            for l in [-2, 2] {
                let c = at(j, l);
                cells.push(EntranceCell {
                    x: c.0,
                    z: c.1,
                    part: EntrancePart::Wall {
                        base: walk,
                        top: roof_at(j, c),
                    },
                });
            }
            // Where the roof starts, a wall from it up to the street closes the opening.
            let last_open = roof_at(j, at(j, 0)).is_none()
                && (j == steps || roof_at(j + 1, at(j + 1, 0)).is_some());
            if last_open {
                let next = j + 1;
                let base = if next > steps {
                    roof
                } else {
                    street - next + 4
                };
                for l in -2..=2 {
                    let c = at(next, l);
                    cells.push(EntranceCell {
                        x: c.0,
                        z: c.1,
                        part: EntrancePart::Wall { base, top: None },
                    });
                }
            }
        }
        // The walk, three wide, from the foot of that flight over to the hall.
        let walk_set: FnvHashSet<(i32, i32)> = path.iter().copied().collect();
        let inside: FnvHashSet<(i32, i32)> = dilate(&walk_set, 1)
            .into_iter()
            .filter(|c| !flight_cells.contains(c))
            .collect();
        let lit: FnvHashSet<(i32, i32)> = path.iter().copied().step_by(4).collect();
        let mut inside_sorted: Vec<(i32, i32)> = inside.iter().copied().collect();
        inside_sorted.sort_unstable();
        for &(x, z) in &inside_sorted {
            cells.push(EntranceCell {
                x,
                z,
                part: EntrancePart::Walk {
                    floor: walk,
                    light: lit.contains(&(x, z)),
                },
            });
        }
        let hall_flight: FnvHashSet<(i32, i32)> = flight.cells.iter().copied().collect();
        let mut walls: Vec<(i32, i32)> = dilate(&walk_set, 2)
            .into_iter()
            .chain(flight.sides.iter().map(|s| s.0))
            .filter(|c| {
                !inside.contains(c) && !flight_cells.contains(c) && !hall_flight.contains(c)
            })
            .collect();
        walls.sort_unstable();
        walls.dedup();
        for (x, z) in walls {
            // The walls of the flight from the street stand there already.
            let beside_flight = (1..=steps).any(|j| [-2, 2].iter().any(|&l| at(j, l) == (x, z)));
            if beside_flight {
                continue;
            }
            cells.push(EntranceCell {
                x,
                z,
                part: EntrancePart::Wall {
                    base: walk,
                    top: Some(roof),
                },
            });
        }
        // The flight down into the hall, glass along its sides on the platform.
        for &(c, k) in &flight.steps {
            cells.push(EntranceCell {
                x: c.0,
                z: c.1,
                part: EntrancePart::HallStair {
                    y: floor + 1 + k,
                    facing: facing_of(flight.dir),
                    base: floor + 1,
                    top: roof - 1,
                },
            });
        }
        for &(c, k) in &flight.sides {
            if k >= 2 {
                cells.push(EntranceCell {
                    x: c.0,
                    z: c.1,
                    part: EntrancePart::Glass {
                        lo: floor + 2,
                        hi: floor + 1 + k,
                    },
                });
            }
        }

        // The hall keeps its benches and pillars off the stairs.
        let clear: FnvHashSet<(i32, i32)> = flight
            .cells
            .iter()
            .copied()
            .chain(flight.sides.iter().map(|s| s.0))
            .chain(
                (0..=1)
                    .flat_map(|k| {
                        flight
                            .offsets
                            .iter()
                            .map(move |&l| (k, l))
                            .collect::<Vec<_>>()
                    })
                    .map(|(k, l)| flight.cell(-k, l)),
            )
            .collect();
        let hall = &mut self.halls[h];
        for c in hall.cells.iter_mut() {
            if clear.contains(&(c.x, c.z)) {
                c.prop = Prop::None;
            }
        }
        for &c in &opening {
            self.tree_free.set(c.0, c.1);
        }
        for c in &cells {
            self.dug.set(c.x, c.z);
        }
        let bbox = bbox_of(cells.iter().map(|c| (c.x, c.z)));
        let (style, accent) = (self.halls[h].style, self.halls[h].accent);
        self.entrances.push(Entrance {
            bbox,
            style,
            accent,
            cells,
        });
    }

    /// Whether trees keep their trunks off (x, z): beside a surface track or on a platform.
    pub fn keeps_trees_off(&self, x: i32, z: i32) -> bool {
        !self.tree_free.is_empty() && self.tree_free.contains(x, z)
    }

    /// Whether a hall or an entrance is dug into the column (x, z).
    pub fn digs(&self, x: i32, z: i32) -> bool {
        !self.dug.is_empty() && self.dug.contains(x, z)
    }

    /// Whether a surface platform covers (x, z).
    pub fn is_platform(&self, x: i32, z: i32) -> bool {
        !self.on_platform.is_empty() && self.on_platform.contains(x, z)
    }

    /// The bed a surface track lies at on a cell beside a platform.
    pub fn track_bed(&self, way: u64, x: i32, z: i32) -> Option<i32> {
        if self.beds.is_empty() {
            return None;
        }
        self.beds.get(&(way, x, z)).copied()
    }

    /// The floor a tunnel joint next to a deep hall is lowered to.
    pub fn joint_floor(&self, x: i32, z: i32) -> Option<i32> {
        if self.joints.is_empty() {
            return None;
        }
        self.joints.get(&(x, z)).copied()
    }

    /// Where a tunnel track lies on a cell in or under a hall.
    pub fn tunnel_pin(&self, way: u64, x: i32, z: i32) -> Option<TunnelPin> {
        if self.floors.is_empty() {
            return None;
        }
        self.floors.get(&(way, x, z)).copied()
    }

    /// Sets the platforms' surfaces as ground in an editor, before anything stands on them.
    pub fn register_levels(&self, editor: &mut WorldEditor) {
        for &(x, z, y) in &self.levels {
            if editor.in_bounds(x, z) {
                editor.register_road_surface_y(x, z, y);
            }
        }
    }

    /// Draws the stations within the editor; open space goes to `voids` for `carve_voids`.
    pub fn render(&self, editor: &mut WorldEditor, voids: &mut Vec<StationVoid>) {
        let (x0, z0, x1, z1) = editor.bounds();
        let overlaps = |b: &(i32, i32, i32, i32)| b.0 <= x1 && b.2 >= x0 && b.1 <= z1 && b.3 >= z0;
        for station in self.surface.iter().filter(|s| overlaps(&s.bbox)) {
            render_surface(editor, station);
        }
        for hall in self.halls.iter().filter(|h| overlaps(&h.bbox)) {
            render_hall(editor, hall, voids);
        }
        let entrances: Vec<&Entrance> = self
            .entrances
            .iter()
            .filter(|e| overlaps(&e.bbox))
            .collect();
        // Walls first, then the open space, so two walks that meet open into each other.
        for entrance in &entrances {
            render_entrance(editor, entrance, voids, false);
        }
        for entrance in &entrances {
            render_entrance(editor, entrance, voids, true);
        }
    }
}

/// Writes over anything but the rails.
fn put(editor: &mut WorldEditor, block: Block, x: i32, y: i32, z: i32) {
    editor.set_block_absolute(block, x, y, z, None, Some(RAILS));
}

fn put_with(
    editor: &mut WorldEditor,
    block: crate::block_definitions::BlockWithProperties,
    x: i32,
    y: i32,
    z: i32,
) {
    editor.set_block_with_properties_absolute(block, x, y, z, None, Some(RAILS));
}

fn stairs(block: Block, facing: i8, top: bool) -> crate::block_definitions::BlockWithProperties {
    with_props(
        block,
        &[
            ("facing", stair_facing(facing)),
            ("half", if top { "top" } else { "bottom" }),
        ],
    )
}

fn slab(block: Block, kind: &str) -> crate::block_definitions::BlockWithProperties {
    with_props(block, &[("type", kind)])
}

/// Open space from `lo` to `hi` in a column: filled for now, emptied after the ground.
fn void(editor: &mut WorldEditor, voids: &mut Vec<StationVoid>, x: i32, z: i32, lo: i32, hi: i32) {
    if lo > hi || !editor.in_bounds(x, z) {
        return;
    }
    for y in lo..=hi {
        put(editor, VOID, x, y, z);
    }
    voids.push((x, z, lo, hi));
}

/// Walls off the open space where nothing else bounds it, so without the ground fill no gap
/// shows the void; only under the crust the ground pass lays anyway.
fn seal(editor: &mut WorldEditor, voids: &[StationVoid], block: Block) {
    for &(x, z, lo, hi) in voids {
        for (nx, nz) in NEIGHBOURS4.map(|(dx, dz)| (x + dx, z + dz)) {
            if !editor.in_bounds(nx, nz) {
                continue;
            }
            let top = hi.min(editor.get_ground_level(nx, nz) - CRUST);
            for y in lo..=top {
                editor.set_block_absolute(block, nx, y, nz, None, None);
            }
        }
        let top = editor.get_ground_level(x, z) - CRUST;
        for y in [lo - 1, hi + 1] {
            if y <= top {
                editor.set_block_absolute(block, x, y, z, None, None);
            }
        }
    }
}

/// Blocks under the surface the ground pass lays without `--fillground`.
const CRUST: i32 = 3;

/// Empties the open space of every station once the ground fill is done.
pub fn carve_voids(editor: &mut WorldEditor, voids: &[StationVoid]) {
    for &(x, z, lo, hi) in voids {
        for y in lo..=hi {
            editor.set_block_absolute(AIR, x, y, z, Some(&[VOID]), None);
        }
    }
}

fn render_surface(editor: &mut WorldEditor, station: &SurfaceStation) {
    for &(x, z, bed) in &station.beds {
        if !editor.in_bounds(x, z) {
            continue;
        }
        editor.register_road_surface_y(x, z, bed);
        put(editor, GRAVEL, x, bed, z);
        for y in bed + 1..=bed + 3 {
            editor.set_block_absolute(AIR, x, y, z, Some(LOOSE), None);
        }
    }
    for c in &station.cells {
        if !editor.in_bounds(c.x, c.z) {
            continue;
        }
        let (x, z) = (c.x, c.z);
        if station.low {
            let y = editor.get_ground_level(x, z) + 1;
            let block = match c.deck {
                Deck::Edge | Deck::End => POLISHED_ANDESITE_SLAB,
                Deck::Line => SMOOTH_QUARTZ_SLAB,
                Deck::Inner => SMOOTH_STONE_SLAB,
            };
            // Furniture stands in the kerb's place, its seat flush with the slabs.
            if c.prop == Prop::None {
                put_with(editor, slab(block, "bottom"), x, y, z);
            }
            continue;
        }
        let y = c.top;
        if !station.elevated {
            editor.register_road_surface_y(x, z, y);
        }
        match c.deck {
            Deck::End => put_with(editor, slab(SMOOTH_STONE_SLAB, "bottom"), x, y, z),
            Deck::Edge => put(editor, POLISHED_ANDESITE, x, y, z),
            Deck::Line => put(editor, WHITE_CONCRETE, x, y, z),
            Deck::Inner => put(editor, SMOOTH_STONE, x, y, z),
        }
        // Whatever stood on the ground here is under the platform now.
        for dy in 1..=3 {
            editor.set_block_absolute(AIR, x, y + dy, z, Some(LOOSE), None);
        }
        if station.elevated {
            put_with(editor, slab(STONE_BRICK_SLAB, "top"), x, y - 1, z);
            if c.pier {
                let ground = editor.get_ground_level(x, z);
                for py in ground + 1..y - 1 {
                    put(editor, STONE_BRICKS, x, py, z);
                }
            }
            if c.back {
                editor.set_block_absolute(IRON_BARS, x, y + 1, z, None, None);
            }
        }
        // A bottom slab, so the posts reach right up to it.
        if c.roof {
            put_with(editor, slab(SMOOTH_STONE_SLAB, "bottom"), x, y + 5, z);
        }
    }
    for c in &station.cells {
        if station.low && editor.in_bounds(c.x, c.z) {
            let y = editor.get_ground_level(c.x, c.z) + 1;
            render_props(editor, c, y, true, station.name.as_deref());
        }
        if !station.low && editor.in_bounds(c.x, c.z) {
            render_props(editor, c, c.top + 1, false, station.name.as_deref());
        }
    }
}

/// Undergrowth and the like a platform's surface buries.
const LOOSE: &[Block] = &[
    GRASS,
    TALL_GRASS_BOTTOM,
    TALL_GRASS_TOP,
    FERN,
    RED_FLOWER,
    YELLOW_FLOWER,
    BLUE_FLOWER,
    WHITE_FLOWER,
    DEAD_BUSH,
    SNOW_LAYER,
];

/// Furniture on a surface platform cell, standing on `y`.
fn render_props(editor: &mut WorldEditor, c: &DeckCell, y: i32, low: bool, name: Option<&str>) {
    let (x, z) = (c.x, c.z);
    match c.prop {
        Prop::Lamp => {
            for dy in 0..3 {
                editor.set_block_absolute(ANDESITE_WALL, x, y + dy, z, None, None);
            }
            editor.set_block_absolute(LANTERN, x, y + 3, z, None, None);
        }
        Prop::Post => {
            let top = if low { y + 2 } else { y + 3 };
            for py in y..=top {
                editor.set_block_absolute(ANDESITE_WALL, x, py, z, None, None);
            }
        }
        Prop::Bench(a, _) if low => {
            editor.set_block_with_properties_absolute(
                stairs(OAK_STAIRS, a, false),
                x,
                y,
                z,
                None,
                None,
            );
        }
        Prop::Bench(a, b) => {
            // Back to back across the platform: the other seat on the next cell over.
            let (dx, dz) = step_of(a);
            editor.set_block_with_properties_absolute(
                stairs(OAK_STAIRS, b, false),
                x,
                y,
                z,
                None,
                None,
            );
            editor.set_block_with_properties_absolute(
                stairs(OAK_STAIRS, a, false),
                x + dx,
                y,
                z + dz,
                None,
                None,
            );
        }
        Prop::Hanging => editor.set_block_with_properties_absolute(
            with_props(LANTERN, &[("hanging", "true")]),
            x,
            y + 3,
            z,
            None,
            None,
        ),
        Prop::Bin => editor.set_block_absolute(CAULDRON, x, y, z, None, None),
        Prop::BoardPost => {
            for dy in 0..3 {
                editor.set_block_absolute(STONE_BRICK_WALL, x, y + dy, z, None, None);
            }
        }
        Prop::Board(a, b) => {
            let board = y + 3;
            for dy in 0..3 {
                editor.set_block_absolute(STONE_BRICK_WALL, x, y + dy, z, None, None);
            }
            editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, board, z, None, None);
            if let Some(name) = name {
                crate::element_processing::signage::place_platform_name(
                    editor, x, board, z, a, name,
                );
                crate::element_processing::signage::place_platform_name(
                    editor, x, board, z, b, name,
                );
            }
        }
        Prop::Pillar | Prop::None => {}
    }
    if c.roof && low {
        put_with(editor, slab(SMOOTH_STONE_SLAB, "bottom"), x, y + 3, z);
    }
}

fn render_hall(editor: &mut WorldEditor, hall: &Hall, voids: &mut Vec<StationVoid>) {
    let start = voids.len();
    let f = hall.floor;
    let roof = f + HALL_HEIGHT;
    let s = hall.style;
    for c in &hall.cells {
        let (x, z) = (c.x, c.z);
        if !editor.in_bounds(x, z) {
            continue;
        }
        match c.part {
            HallPart::Wall => {
                put(editor, POLISHED_DEEPSLATE, x, f, z);
                put(editor, s.skirting, x, f + 1, z);
                for y in f + 2..roof {
                    put(
                        editor,
                        if y == f + 3 { hall.accent } else { s.wall },
                        x,
                        y,
                        z,
                    );
                }
                put(editor, s.ceiling, x, roof, z);
            }
            HallPart::Portal => {
                put(editor, POLISHED_DEEPSLATE, x, f, z);
                void(editor, voids, x, z, f + 1, f + 4);
                put(editor, s.wall, x, f + 5, z);
                put(editor, s.ceiling, x, roof, z);
            }
            HallPart::Trackbed => {
                put(editor, POLISHED_DEEPSLATE, x, f, z);
                void(editor, voids, x, z, f + 1, roof - 1);
                put(
                    editor,
                    if c.light { SEA_LANTERN } else { s.ceiling },
                    x,
                    roof,
                    z,
                );
            }
            HallPart::Platform(deck) => {
                put(editor, POLISHED_DEEPSLATE, x, f, z);
                let top = match deck {
                    Deck::Edge => POLISHED_ANDESITE,
                    Deck::Line => WHITE_CONCRETE,
                    _ => s.floor,
                };
                put(editor, top, x, f + 1, z);
                void(editor, voids, x, z, f + 2, roof - 1);
                put(
                    editor,
                    if c.light { SEA_LANTERN } else { s.ceiling },
                    x,
                    roof,
                    z,
                );
            }
        }
    }
    for c in &hall.cells {
        let (x, z) = (c.x, c.z);
        if editor.in_bounds(x, z) && matches!(c.part, HallPart::Platform(_)) {
            match c.prop {
                Prop::Pillar => {
                    for y in f + 2..roof {
                        put(editor, s.pillar, x, y, z);
                    }
                }
                Prop::Bench(a, b) => {
                    let (dx, dz) = step_of(a);
                    put_with(editor, stairs(SPRUCE_STAIRS, b, false), x, f + 2, z);
                    if hall.platform.contains(&(x + dx, z + dz)) {
                        put_with(
                            editor,
                            stairs(SPRUCE_STAIRS, a, false),
                            x + dx,
                            f + 2,
                            z + dz,
                        );
                    }
                }
                Prop::Bin => put(editor, CAULDRON, x, f + 2, z),
                _ => {}
            }
        }
    }
    seal(editor, &voids[start..], s.wall);
}

/// The flight from a platform up through a hall's roof.
#[derive(Clone, Debug)]
struct Flight {
    foot: (i32, i32),
    dir: (i32, i32),
    offsets: Vec<i32>,
    /// Stair cells with their step, 1 at the foot.
    steps: Vec<((i32, i32), i32)>,
    cells: Vec<(i32, i32)>,
    /// Cells either side of the stairs, with the step they flank.
    sides: Vec<((i32, i32), i32)>,
    /// Where the walk takes over, past the top step.
    top: (i32, i32),
}

impl Flight {
    fn cell(&self, k: i32, l: i32) -> (i32, i32) {
        let side = (-self.dir.1, self.dir.0);
        (
            self.foot.0 + self.dir.0 * k + side.0 * l,
            self.foot.1 + self.dir.1 * k + side.1 * l,
        )
    }

    /// The widest flight along `dir` from `foot` that fits on the platform, glass included.
    fn fit(hall: &Hall, foot: (i32, i32), dir: (i32, i32)) -> Option<Flight> {
        let on = |c: (i32, i32)| hall.platform.contains(&c);
        let inner = |c: (i32, i32)| on(c) && !hall.platform_edge.contains(&c);
        for offsets in [vec![-1, 0, 1], vec![0, 1], vec![-1, 0], vec![0]] {
            let mut f = Flight {
                foot,
                dir,
                offsets: offsets.clone(),
                steps: Vec::new(),
                cells: Vec::new(),
                sides: Vec::new(),
                top: (0, 0),
            };
            let (lo, hi) = (offsets[0], offsets[offsets.len() - 1]);
            let fits = (0..=HALL_FLIGHT).all(|k| offsets.iter().all(|&l| inner(f.cell(k, l))))
                && (1..=HALL_FLIGHT).all(|k| on(f.cell(k, lo - 1)) && on(f.cell(k, hi + 1)));
            if !fits {
                continue;
            }
            for k in 1..=HALL_FLIGHT {
                for &l in &offsets {
                    f.steps.push((f.cell(k, l), k));
                    f.cells.push(f.cell(k, l));
                }
                f.sides.push((f.cell(k, lo - 1), k));
                f.sides.push((f.cell(k, hi + 1), k));
            }
            f.top = f.cell(HALL_FLIGHT + 1, 0);
            return Some(f);
        }
        None
    }
}

/// Cells from `a` to `b` along the grid, one leg along x and one along z.
fn l_path(a: (i32, i32), b: (i32, i32), x_first: bool) -> Vec<(i32, i32)> {
    let corner = if x_first { (b.0, a.1) } else { (a.0, b.1) };
    let mut out = vec![a];
    for (from, to) in [(a, corner), (corner, b)] {
        let (dx, dz) = ((to.0 - from.0).signum(), (to.1 - from.1).signum());
        let mut c = from;
        while c != to {
            c = (c.0 + dx, c.1 + dz);
            out.push(c);
        }
    }
    out
}

/// The carriageway cells round the entrances, which their stairs keep off, and the barriers
/// there, which none may open onto.
fn carriageway_near(
    elements: &[ProcessedElement],
    nodes: &[&ProcessedNode],
    scale: f64,
) -> (FnvHashSet<Xz>, FnvHashSet<Xz>) {
    const NEAR: i32 = 32;
    let buckets: FnvHashSet<(i32, i32)> = nodes
        .iter()
        .flat_map(|n| {
            let (bx, bz) = (n.x >> 5, n.z >> 5);
            (-1..=1).flat_map(move |dx| (-1..=1).map(move |dz| (bx + dx, bz + dz)))
        })
        .collect();
    let near_nodes = |way: &crate::osm_parser::ProcessedWay| {
        let (x0, z0, x1, z1) = bbox_of(way.nodes.iter().map(|n| (n.x, n.z)));
        (((x0 - NEAR) >> 5)..=((x1 + NEAR) >> 5)).any(|bx| {
            (((z0 - NEAR) >> 5)..=((z1 + NEAR) >> 5)).any(|bz| buckets.contains(&(bx, bz)))
        })
    };
    let mut out = FnvHashSet::default();
    let mut fenced = FnvHashSet::default();
    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };
        if way
            .tags
            .get("barrier")
            .is_some_and(|b| b != "kerb" && b != "ditch")
            && near_nodes(way)
        {
            for w in way.nodes.windows(2) {
                for (x, _, z) in bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z) {
                    fenced.insert((x, z));
                }
            }
            continue;
        }
        let Some(kind) = crate::element_processing::highways::surface_highway_type(way) else {
            continue;
        };
        if !crate::element_processing::highways::is_carriageway(kind)
            || ["tunnel", "bridge"]
                .iter()
                .any(|k| way.tags.get(*k).is_some_and(|v| v != "no"))
        {
            continue;
        }
        if !near_nodes(way) {
            continue;
        }
        let r = crate::element_processing::highways::highway_block_range(kind, &way.tags, scale);
        for w in way.nodes.windows(2) {
            for (x, _, z) in bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z) {
                if !buckets.contains(&(x >> 5, z >> 5)) {
                    continue;
                }
                for dx in -r..=r {
                    for dz in -r..=r {
                        out.insert((x + dx, z + dz));
                    }
                }
            }
        }
    }
    (out, fenced)
}

fn render_entrance(
    editor: &mut WorldEditor,
    entrance: &Entrance,
    voids: &mut Vec<StationVoid>,
    open: bool,
) {
    let start = voids.len();
    let s = entrance.style;
    for c in &entrance.cells {
        let (x, z) = (c.x, c.z);
        if !editor.in_bounds(x, z) {
            continue;
        }
        match (c.part, open) {
            (
                EntrancePart::Stair {
                    y,
                    facing,
                    base,
                    roof,
                },
                false,
            ) => {
                for fy in base..y {
                    put(editor, s.wall, x, fy, z);
                }
                put_with(
                    editor,
                    stairs(POLISHED_ANDESITE_STAIRS, facing, false),
                    x,
                    y,
                    z,
                );
                if let Some(r) = roof {
                    put(editor, s.ceiling, x, r, z);
                }
            }
            (EntrancePart::Stair { y, roof, .. }, true) => match roof {
                Some(r) => void(editor, voids, x, z, y + 1, r - 1),
                None => {
                    let street = editor.get_ground_level(x, z);
                    void(editor, voids, x, z, y + 1, street);
                    for dy in 1..=4 {
                        editor.set_block_absolute(AIR, x, street + dy, z, None, Some(RAILS));
                    }
                }
            },
            (EntrancePart::Wall { base, top }, false) => {
                let t = top.unwrap_or_else(|| editor.get_ground_level(x, z));
                for y in base..=t {
                    let block = if top.is_none() && y == t {
                        SMOOTH_STONE
                    } else if y == base + 2 {
                        entrance.accent
                    } else {
                        s.wall
                    };
                    put(editor, block, x, y, z);
                }
            }
            (EntrancePart::Wall { top: None, .. }, true) => {
                let t = editor.get_ground_level(x, z);
                editor.set_block_absolute(AIR, x, t + 1, z, Some(LOOSE), None);
                crate::element_processing::connected_blocks::place_connected(
                    editor,
                    IRON_BARS,
                    x,
                    t + 1,
                    z,
                );
            }
            (EntrancePart::Walk { floor, light }, false) => {
                put(editor, s.floor, x, floor, z);
                put(
                    editor,
                    if light { SEA_LANTERN } else { s.ceiling },
                    x,
                    floor + WALK_HEIGHT,
                    z,
                );
            }
            (EntrancePart::Walk { floor, .. }, true) => {
                void(editor, voids, x, z, floor + 1, floor + WALK_HEIGHT - 1);
            }
            (
                EntrancePart::HallStair {
                    y,
                    facing,
                    base,
                    top,
                },
                false,
            ) => {
                for fy in base..y {
                    put(editor, s.floor, x, fy, z);
                }
                put_with(
                    editor,
                    stairs(POLISHED_ANDESITE_STAIRS, facing, false),
                    x,
                    y,
                    z,
                );
                put(editor, s.ceiling, x, top + 1, z);
            }
            (EntrancePart::HallStair { y, top, .. }, true) => {
                void(editor, voids, x, z, y + 1, top);
            }
            (EntrancePart::Glass { lo, hi }, true) => {
                for y in lo..=hi {
                    editor.set_block_absolute(AIR, x, y, z, Some(&[VOID]), None);
                    crate::element_processing::connected_blocks::place_connected(
                        editor, GLASS_PANE, x, y, z,
                    );
                }
            }
            _ => {}
        }
    }
    if open {
        seal(editor, &voids[start..], s.wall);
    }
}

/// What every entrance's route reads.
struct EntranceContext<'a> {
    inputs: &'a StationInputs<'a>,
    carriageway: &'a FnvHashSet<(i32, i32)>,
    fenced: &'a FnvHashSet<(i32, i32)>,
    cover: &'a FnvHashMap<(i32, i32), Vec<usize>>,
}

/// A routed entrance: street flight along `e`, walk `path` and flight into the hall.
struct Route {
    hall: usize,
    street: i32,
    steps: i32,
    e: (i32, i32),
    q: (i32, i32),
    c0: (i32, i32),
    flight: Flight,
    path: Vec<(i32, i32)>,
    opening: Vec<(i32, i32)>,
    flight_cells: FnvHashSet<(i32, i32)>,
}

impl Route {
    /// Every column the route digs, for keeping the next ones clear of it.
    fn cells_used(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        let walk: FnvHashSet<(i32, i32)> = self.path.iter().copied().collect();
        let walls: Vec<(i32, i32)> = dilate(&walk, 2).into_iter().collect();
        walls
            .into_iter()
            .chain(self.opening.iter().copied())
            .chain(self.flight.cells.iter().copied())
            .chain(self.flight.sides.iter().map(|s| s.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::element_processing::bridge_styles::BridgeOutlineIndex;
    use crate::element_processing::highways::HighwayConnectivityMap;
    use crate::osm_parser::ProcessedWay;
    use std::path::PathBuf;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn way(id: u64, pts: &[(i32, i32)], t: &[(&str, &str)]) -> ProcessedElement {
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes: pts
                .iter()
                .enumerate()
                .map(|(i, &(x, z))| ProcessedNode {
                    id: id * 100 + i as u64,
                    tags: HashMap::new(),
                    x,
                    z,
                })
                .collect(),
            tags: tags(t),
        })
    }

    fn rect(x0: i32, z0: i32, x1: i32, z1: i32) -> Vec<(i32, i32)> {
        vec![(x0, z0), (x1, z0), (x1, z1), (x0, z1), (x0, z0)]
    }

    const SUBWAY: &[(&str, &str)] = &[("railway", "subway"), ("tunnel", "yes")];

    fn test_editor(xzbbox: &XZBBox) -> WorldEditor<'_> {
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        WorldEditor::new(PathBuf::from("/dev/null/unused"), xzbbox, llbbox)
    }

    // Flat ground at 0, nothing else in the area.
    fn lay_out(elements: &[ProcessedElement], xzbbox: &XZBBox) -> StationIndex {
        let editor = test_editor(xzbbox);
        let outlines = BridgeOutlineIndex::build(&[]);
        let bridges =
            BridgeStructureMap::build(&[], &editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
        let none = CoordinateBitmap::new(xzbbox);
        StationIndex::build(
            elements,
            &StationInputs {
                editor: &editor,
                xzbbox,
                bridges: &bridges,
                tunnel_footprint: &none,
                building_footprints: &none,
                road_mask: &none,
                rail_joins: &RailJoins::default(),
                scale: 1.0,
                timeout: None,
                underground: true,
            },
        )
    }

    fn open(editor: &WorldEditor, x: i32, y: i32, z: i32) -> bool {
        editor.get_block_absolute(x, y, z).is_none_or(|b| b == AIR)
    }

    #[test]
    fn platform_tags_are_read_as_mapped() {
        assert!(is_rail_platform(&tags(&[("railway", "platform")])));
        assert!(is_rail_platform(&tags(&[
            ("public_transport", "platform"),
            ("subway", "yes")
        ])));
        assert!(!is_rail_platform(&tags(&[
            ("public_transport", "platform"),
            ("bus", "yes")
        ])));
        assert_eq!(tag_number("-2;-3"), Some(-3.0));
        assert_eq!(tag_number("0.76 m"), Some(0.76));
        assert_eq!(tag_number("yes"), None);
        assert_eq!(
            depth_key(&tags(&[("level", "-1"), ("layer", "-2")])),
            Some(-2)
        );
        assert_eq!(depth_key(&tags(&[("level", "-1.5")])), Some(-2));
        assert!(tagged_underground(&tags(&[("location", "underground")])));
        assert!(tagged_underground(&tags(&[("level", "-1")])));
        assert!(!tagged_underground(&tags(&[
            ("tunnel", "no"),
            ("level", "0")
        ])));
        assert!(is_low(&tags(&[("height", "30 cm")])));
        assert!(!is_low(&tags(&[("height", "0.76")])));
        assert!(is_low(&tags(&[("tram", "yes")])));
        assert!(!is_low(&tags(&[("tram", "yes"), ("train", "yes")])));
        assert!(!is_low(&tags(&[("railway", "platform")])));
    }

    #[test]
    fn a_square_of_cells_is_set_row_by_row_across_blocks() {
        let (mut fast, mut slow) = (SparseCells::default(), SparseCells::default());
        assert!(fast.is_empty());
        for (x, z, r) in [(0, 0, 2), (-17, 15, 3), (31, -1, 1), (-1, -1, 0)] {
            fast.set_square(x, z, r);
            for dx in -r..=r {
                for dz in -r..=r {
                    slow.set(x + dx, z + dz);
                }
            }
        }
        for x in -40..40 {
            for z in -40..40 {
                assert_eq!(fast.contains(x, z), slow.contains(x, z), "({x}, {z})");
            }
        }
    }

    #[test]
    fn an_l_path_steps_one_cell_at_a_time_round_its_corner() {
        for x_first in [true, false] {
            let path = l_path((2, 3), (-1, 7), x_first);
            assert_eq!((path[0], path[path.len() - 1]), ((2, 3), (-1, 7)));
            assert_eq!(path.len(), 8);
            assert!(path
                .windows(2)
                .all(|w| (w[0].0 - w[1].0).abs() + (w[0].1 - w[1].1).abs() == 1));
            assert!(path.contains(&if x_first { (-1, 3) } else { (2, 7) }));
        }
        assert_eq!(l_path((5, 5), (5, 5), true), vec![(5, 5)]);
    }

    #[test]
    fn a_surface_platform_levels_the_track_beside_it() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 200, 100).unwrap();
        let index = lay_out(
            &[
                way(1, &[(10, 50), (190, 50)], &[("railway", "rail")]),
                way(2, &rect(60, 52, 120, 57), &[("railway", "platform")]),
            ],
            &xzbbox,
        );
        assert_eq!((index.surface.len(), index.halls.len()), (1, 0));
        // The platform stands a block over the ground, the rail level with it.
        assert_eq!(index.track_bed(1, 90, 50), Some(0));
        assert_eq!(index.track_bed(1, 20, 50), None);
        assert!(index.is_platform(90, 55) && !index.is_platform(90, 50));
        // Trees keep off the track's corridor and the platform.
        assert!(index.keeps_trees_off(20, 52) && !index.keeps_trees_off(20, 53));
        assert!(index.keeps_trees_off(90, 57));

        // A tram stop is a kerb on the ground; the track keeps its own level.
        let index = lay_out(
            &[
                way(1, &[(10, 50), (190, 50)], &[("railway", "tram")]),
                way(
                    2,
                    &rect(60, 52, 80, 54),
                    &[("railway", "platform"), ("tram", "yes")],
                ),
            ],
            &xzbbox,
        );
        assert!(index.surface.len() == 1 && index.surface[0].low);
        assert_eq!(index.track_bed(1, 70, 50), None);
    }

    #[test]
    fn an_underground_platform_gets_a_hall_with_stairs_up_to_its_entrance() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 200, 120).unwrap();
        let entrance = ProcessedElement::Node(ProcessedNode {
            id: 3,
            tags: tags(&[("railway", "subway_entrance")]),
            x: 90,
            z: 75,
        });
        let index = lay_out(
            &[
                way(1, &[(10, 50), (190, 50)], SUBWAY),
                way(
                    2,
                    &rect(60, 52, 120, 57),
                    &[("railway", "platform"), ("location", "underground")],
                ),
                entrance,
            ],
            &xzbbox,
        );
        assert!(index.surface.is_empty());
        assert_eq!(index.halls.len(), 1);
        let floor = -HALL_DEPTH;
        assert_eq!(index.halls[0].floor, floor);
        // The tunnel runs level through its hall and is left alone past it.
        let pin = Some(TunnelPin {
            floor,
            in_hall: true,
        });
        assert_eq!(index.tunnel_pin(1, 90, 50), pin);
        assert_eq!(index.tunnel_pin(1, 20, 50), None);
        assert_eq!(index.entrances.len(), 1);

        let mut editor = test_editor(&xzbbox);
        let mut voids = Vec::new();
        index.render(&mut editor, &mut voids);
        carve_voids(&mut editor, &voids);
        for &(x, z, lo, hi) in &voids {
            assert!((lo..=hi).all(|y| editor.get_block_absolute(x, y, z) != Some(VOID)));
        }
        // Without the ground fill nothing under the crust may open onto the void.
        let open_space: FnvHashSet<(i32, i32, i32)> = voids
            .iter()
            .flat_map(|&(x, z, lo, hi)| (lo..=hi).map(move |y| (x, y, z)))
            .collect();
        for &(x, z, lo, hi) in &voids {
            for y in lo..=hi.min(-CRUST - 1) {
                for (nx, ny, nz) in [
                    (x + 1, y, z),
                    (x - 1, y, z),
                    (x, y, z + 1),
                    (x, y, z - 1),
                    (x, y - 1, z),
                    (x, y + 1, z),
                ] {
                    if ny <= -CRUST
                        && editor.in_bounds(nx, nz)
                        && !open_space.contains(&(nx, ny, nz))
                    {
                        assert!(
                            editor.block_exists_absolute(nx, ny, nz),
                            "gap at {:?}",
                            (nx, ny, nz)
                        );
                    }
                }
            }
        }
        // Clear headroom over every platform cell free of the stairs and furniture.
        let entrance = &index.entrances[0];
        let hall = &index.halls[0];
        let mut taken: FnvHashSet<(i32, i32)> = entrance.cells.iter().map(|c| (c.x, c.z)).collect();
        for c in &hall.cells {
            if let Prop::Bench(a, _) = c.prop {
                let (dx, dz) = step_of(a);
                taken.insert((c.x + dx, c.z + dz));
            }
        }
        for c in hall.cells.iter().filter(|c| {
            matches!(c.part, HallPart::Platform(_))
                && c.prop == Prop::None
                && !taken.contains(&(c.x, c.z))
        }) {
            assert!(editor.block_exists_absolute(c.x, floor + 1, c.z));
            assert!((floor + 2..floor + HALL_HEIGHT).all(|y| open(&editor, c.x, y, c.z)));
        }
        // A step in the pavement, a walk over the hall and a flight down through its roof, each
        // flight a stair for every block it climbs.
        let mut parts = (false, false, false);
        for c in &entrance.cells {
            match c.part {
                EntrancePart::Stair { y: 0, .. } => {
                    parts.0 = true;
                    assert!(open(&editor, c.x, 1, c.z) && open(&editor, c.x, 2, c.z));
                }
                EntrancePart::Walk { floor: f, .. } => {
                    parts.1 = true;
                    assert_eq!(f, floor + HALL_HEIGHT);
                    assert!((f + 1..f + WALK_HEIGHT).all(|y| open(&editor, c.x, y, c.z)));
                }
                EntrancePart::HallStair { y, .. } => {
                    parts.2 |= y == floor + HALL_HEIGHT;
                    assert_eq!(
                        editor.get_block_absolute(c.x, y, c.z),
                        Some(POLISHED_ANDESITE_STAIRS)
                    );
                }
                _ => {}
            }
        }
        assert_eq!(parts, (true, true, true));
    }

    #[test]
    fn a_line_crossing_another_station_gets_its_hall_a_storey_lower() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 200, 200).unwrap();
        let platform = |level: &'static str| {
            [
                ("railway", "platform"),
                ("location", "underground"),
                ("level", level),
            ]
        };
        let index = lay_out(
            &[
                way(1, &[(10, 100), (190, 100)], SUBWAY),
                way(2, &rect(70, 102, 130, 107), &platform("-1")),
                way(3, &[(100, 10), (100, 190)], SUBWAY),
                way(4, &rect(102, 70, 107, 130), &platform("-2")),
            ],
            &xzbbox,
        );
        let floors: Vec<i32> = index.halls.iter().map(|h| h.floor).collect();
        assert_eq!(floors, vec![-HALL_DEPTH, -HALL_DEPTH - HALL_STACK]);
        // Each line keeps its own hall's floor right through the crossing.
        for (way, x, z, floor) in [(1, 100, 100, floors[0]), (3, 100, 100, floors[1])] {
            assert_eq!(
                index.tunnel_pin(way, x, z),
                Some(TunnelPin {
                    floor,
                    in_hall: true
                })
            );
        }
    }

    const UNDER: &[(&str, &str)] = &[("railway", "platform"), ("location", "underground")];

    #[test]
    fn a_way_split_past_the_platform_keeps_the_hall_floor() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 200, 100).unwrap();
        for split in 121..=128 {
            let node = |id, x| ProcessedNode {
                id,
                tags: HashMap::new(),
                x,
                z: 50,
            };
            let part = |id, a, b| {
                ProcessedElement::Way(ProcessedWay {
                    id,
                    nodes: vec![a, b],
                    tags: tags(SUBWAY),
                })
            };
            let index = lay_out(
                &[
                    part(1, node(10, 10), node(11, split)),
                    part(2, node(11, split), node(12, 190)),
                    way(3, &rect(60, 52, 120, 57), UNDER),
                ],
                &xzbbox,
            );
            for x in 100..140 {
                for way in [1, 2] {
                    if let Some(pin) = index.tunnel_pin(way, x, 50) {
                        assert_eq!(pin.floor, -HALL_DEPTH, "split {split}, way {way} at {x}");
                    }
                }
            }
        }
    }

    #[test]
    fn halls_along_one_line_never_stack() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 400, 100).unwrap();
        for gap in [10, 20, 30, 40, 60] {
            let b = 120 + gap;
            let index = lay_out(
                &[
                    way(1, &[(10, 50), (390, 50)], SUBWAY),
                    way(3, &rect(60, 52, 120, 57), UNDER),
                    way(4, &rect(b, 52, b + 60, 57), UNDER),
                ],
                &xzbbox,
            );
            assert!(
                index.halls.iter().all(|h| h.floor == -HALL_DEPTH),
                "gap {gap}"
            );
        }
    }

    #[test]
    fn a_line_from_a_far_station_goes_under_a_hall_it_crosses() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 400, 400).unwrap();
        let elements = [
            way(1, &[(10, 100), (390, 100)], SUBWAY),
            way(2, &rect(70, 102, 130, 107), UNDER),
            way(3, &[(100, 10), (100, 390)], SUBWAY),
            way(4, &rect(102, 250, 107, 310), UNDER),
        ];
        // The far station stands on higher ground, so its hall lies shallower.
        let mut editor = test_editor(&xzbbox);
        for x in 80..130 {
            for z in 230..330 {
                editor.register_road_surface_y(x, z, 3);
            }
        }
        let outlines = BridgeOutlineIndex::build(&[]);
        let bridges =
            BridgeStructureMap::build(&[], &editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
        let none = CoordinateBitmap::new(&xzbbox);
        let index = StationIndex::build(
            &elements,
            &StationInputs {
                editor: &editor,
                xzbbox: &xzbbox,
                bridges: &bridges,
                tunnel_footprint: &none,
                building_footprints: &none,
                road_mask: &none,
                rail_joins: &RailJoins::default(),
                scale: 1.0,
                timeout: None,
                underground: true,
            },
        );
        let floors: Vec<i32> = index.halls.iter().map(|h| h.floor).collect();
        assert_eq!(floors, vec![-HALL_DEPTH, 3 - HALL_DEPTH]);
        assert_eq!(
            index.tunnel_pin(3, 100, 104),
            Some(TunnelPin {
                floor: -HALL_DEPTH - HALL_STACK,
                in_hall: false
            })
        );
    }

    #[test]
    fn a_wild_layer_tag_is_kept_in_range() {
        assert_eq!(depth_key(&tags(&[("layer", "99999999999")])), Some(16));
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 200, 200).unwrap();
        let index = lay_out(
            &[
                way(1, &[(10, 50), (190, 50)], SUBWAY),
                way(
                    3,
                    &rect(60, 52, 120, 57),
                    &[
                        ("railway", "platform"),
                        ("tunnel", "yes"),
                        ("layer", "-99999999999"),
                    ],
                ),
                way(2, &[(10, 150), (190, 150)], SUBWAY),
                way(4, &rect(60, 152, 120, 157), UNDER),
            ],
            &xzbbox,
        );
        assert_eq!(index.halls.len(), 2);
    }

    #[test]
    fn without_room_under_ground_no_hall_is_dug() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 200, 100).unwrap();
        let elements = [
            way(1, &[(10, 50), (190, 50)], SUBWAY),
            way(
                2,
                &rect(60, 52, 120, 57),
                &[("railway", "platform"), ("location", "underground")],
            ),
        ];
        let editor = test_editor(&xzbbox);
        let outlines = BridgeOutlineIndex::build(&[]);
        let bridges =
            BridgeStructureMap::build(&[], &editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
        let none = CoordinateBitmap::new(&xzbbox);
        let index = StationIndex::build(
            &elements,
            &StationInputs {
                editor: &editor,
                xzbbox: &xzbbox,
                bridges: &bridges,
                tunnel_footprint: &none,
                building_footprints: &none,
                road_mask: &none,
                rail_joins: &RailJoins::default(),
                scale: 1.0,
                timeout: None,
                underground: false,
            },
        );
        assert!(index.halls.is_empty() && index.surface.is_empty());
        assert_eq!(index.tunnel_pin(1, 90, 50), None);
    }
}
