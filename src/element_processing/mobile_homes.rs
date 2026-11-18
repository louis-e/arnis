//! Mobile homes: single and double-wides on skirting under a low roof, with steps or a deck.

use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::deterministic_rng::element_rng;
use crate::element_processing::area_layout::{bounds, centroid, inside, point_in_rings, Frame};
use crate::element_processing::buildings::{cached_prop_block, facing_for_normal};
use crate::floodfill_cache::{BuildingFootprintBitmap, RoadMaskBitmap};
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use fnv::{FnvHashMap, FnvHashSet};
use rand::Rng;
use std::collections::{BTreeMap, HashMap};

/// Set by `prepare` on a plain building in a trailer park shaped like a mobile home.
const IN_PARK_TAG: &str = "arnis:mobile_home";

const MAPPED_TYPES: [&str; 4] = ["static_caravan", "mobile_home", "trailer", "caravan"];

const SIDES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// Mapped as a mobile home, or marked as one by `prepare`, and no more than a storey.
pub fn claims(tags: &HashMap<String, String>) -> bool {
    let mapped = tags
        .get("building")
        .is_some_and(|b| MAPPED_TYPES.contains(&b.as_str()));
    (mapped || tags.contains_key(IN_PARK_TAG)) && one_storey(tags)
}

/// Mapped with no more than a storey; Overture's guessed height and storeys do not count.
fn one_storey(tags: &HashMap<String, String>) -> bool {
    overture_guess(tags)
        || tags
            .get("building:levels")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .is_none_or(|l| l < 2.0)
}

/// A building or a height that only Overture's estimates give.
fn overture_guess(tags: &HashMap<String, String>) -> bool {
    tags.get("source").map(String::as_str) == Some("overture_maps")
        || tags.get("arnis:height_source").map(String::as_str) == Some("overture_maps")
}

/// A trailer park: tagged as one, or residential land named like one.
pub fn is_trailer_park(tags: &HashMap<String, String>) -> bool {
    if matches!(
        tags.get("residential").map(String::as_str),
        Some("trailer_park" | "mobile_home_park" | "mobile_home" | "mobile")
    ) {
        return true;
    }
    let area = tags.get("landuse").map(String::as_str) == Some("residential")
        || tags.get("place").map(String::as_str) == Some("neighbourhood");
    area && tags.get("name").is_some_and(|n| named_as_park(n))
}

fn named_as_park(name: &str) -> bool {
    const PHRASES: [&str; 11] = [
        "trailer",
        "mobile home",
        "mobile park",
        "mobile estate",
        "mobile village",
        "mobile court",
        "mobile manor",
        "mobile ranch",
        "mobile villa",
        "mobile community",
        "manufactured home",
    ];
    let name = name.to_lowercase();
    PHRASES.iter().any(|p| name.contains(p))
        || name
            .split(|c: char| !c.is_alphanumeric())
            .any(|w| w == "mhp")
}

/// A plain one-storey building with nothing else mapped on it.
fn could_be_one(tags: &HashMap<String, String>) -> bool {
    let plain = match tags.get("building").map(String::as_str) {
        Some("yes" | "house" | "residential" | "detached" | "bungalow" | "cabin") => true,
        // Overture's classes are coarse; a unit in a park can come as flats.
        Some("apartments") => tags.get("source").map(String::as_str) == Some("overture_maps"),
        _ => false,
    };
    let other_use = [
        "amenity",
        "shop",
        "office",
        "craft",
        "leisure",
        "tourism",
        "man_made",
        "building:part",
    ]
    .iter()
    .any(|k| tags.contains_key(*k));
    let low = overture_guess(tags)
        || tags
            .get("height")
            .and_then(|v| v.trim_end_matches('m').trim().parse::<f64>().ok())
            .is_none_or(|h| h <= 6.0);
    plain && !other_use && low && one_storey(tags)
}

fn ring_area(nodes: &[ProcessedNode]) -> f64 {
    let twice: f64 = nodes
        .windows(2)
        .map(|w| w[0].x as f64 * w[1].z as f64 - w[1].x as f64 * w[0].z as f64)
        .sum();
    twice.abs() / 2.0
}

/// A single-wide is about 4 by 20 m, a double-wide 8 by 18 m, wider with its carport drawn in.
fn shaped_like_one(nodes: &[ProcessedNode], scale: f64) -> bool {
    let Some(frame) = Frame::of(nodes) else {
        return false;
    };
    let (l, w) = (frame.length() / scale, frame.width() / scale);
    let area = ring_area(nodes) / (scale * scale);
    if w < 2.5 || l > 30.0 || area < 0.75 * l * w {
        return false;
    }
    if w <= 6.0 {
        l >= 8.0 && l >= 2.4 * w
    } else {
        w <= 12.5 && l <= 26.0 && l >= 1.3 * w && area <= 260.0
    }
}

/// Marks the plain buildings in trailer parks that are shaped like mobile homes.
pub fn prepare(elements: &mut [ProcessedElement], scale: f64) {
    const CELL: i32 = 128;
    type Bounds = (f64, f64, f64, f64);
    let mut parks: Vec<(Bounds, Vec<Vec<ProcessedNode>>)> = Vec::new();
    for e in elements.iter() {
        if !is_trailer_park(e.tags()) {
            continue;
        }
        // Inner rings too: the even-odd test leaves their holes out.
        let rings: Vec<Vec<ProcessedNode>> = match e {
            ProcessedElement::Way(w) => vec![w.nodes.clone()],
            ProcessedElement::Relation(r) => r
                .members
                .iter()
                .filter(|m| m.role != ProcessedMemberRole::Part)
                .map(|m| m.way.nodes.clone())
                .collect(),
            ProcessedElement::Node(_) => continue,
        };
        let all: Vec<ProcessedNode> = rings.iter().flatten().cloned().collect();
        if let Some(b) = bounds(&all) {
            parks.push((b, rings));
        }
    }
    if parks.is_empty() {
        return;
    }
    let slices: Vec<Vec<&[ProcessedNode]>> = parks
        .iter()
        .map(|(_, rings)| rings.iter().map(Vec::as_slice).collect())
        .collect();
    let mut grid: FnvHashMap<(i32, i32), Vec<usize>> = FnvHashMap::default();
    for (i, &(b, _)) in parks.iter().enumerate() {
        let (x0, z0) = ((b.0 as i32).div_euclid(CELL), (b.1 as i32).div_euclid(CELL));
        let (x1, z1) = ((b.2 as i32).div_euclid(CELL), (b.3 as i32).div_euclid(CELL));
        for gx in x0..=x1 {
            for gz in z0..=z1 {
                grid.entry((gx, gz)).or_default().push(i);
            }
        }
    }
    for e in elements.iter_mut() {
        let ProcessedElement::Way(w) = e else {
            continue;
        };
        if w.nodes.len() < 4 || claims(&w.tags) || !could_be_one(&w.tags) {
            continue;
        }
        let c = centroid(&w.nodes[..w.nodes.len() - 1]);
        let key = ((c.0 as i32).div_euclid(CELL), (c.1 as i32).div_euclid(CELL));
        let in_park = grid.get(&key).is_some_and(|ids| {
            ids.iter()
                .any(|&i| inside(parks[i].0, c) && point_in_rings(&slices[i], c))
        });
        if in_park && shaped_like_one(&w.nodes, scale) {
            w.tags.insert(IN_PARK_TAG.to_string(), "yes".to_string());
        }
    }
}

/// What one unit is built of.
struct Look {
    siding: Block,
    /// A darker lower course on two-tone units.
    band: Option<Block>,
    skirting: Block,
    roof: (Block, Block),
    door: Block,
    glass: Block,
    shutters: Option<Block>,
    planks: Block,
    stairs: Block,
    peaked: bool,
}

fn pick<T: Copy>(rng: &mut impl Rng, options: &[(T, u32)]) -> T {
    let total: u32 = options.iter().map(|o| o.1).sum();
    let mut roll = rng.random_range(0..total);
    for &(v, w) in options {
        if roll < w {
            return v;
        }
        roll -= w;
    }
    options[0].0
}

impl Look {
    fn pick(rng: &mut impl Rng) -> Self {
        let siding = pick(
            rng,
            &[
                (WHITE_CONCRETE, 20),
                (SMOOTH_QUARTZ, 8),
                (BIRCH_PLANKS, 14),
                (SMOOTH_SANDSTONE, 12),
                (WHITE_TERRACOTTA, 8),
                (END_STONE_BRICKS, 6),
                (LIGHT_BLUE_TERRACOTTA, 9),
                (LIGHT_GRAY_CONCRETE, 10),
                (CALCITE, 5),
            ],
        );
        let band = rng
            .random_bool(0.15)
            .then(|| {
                pick(
                    rng,
                    &[
                        (LIGHT_BLUE_TERRACOTTA, 3),
                        (GRAY_TERRACOTTA, 2),
                        (BROWN_TERRACOTTA, 2),
                        (LIGHT_GRAY_CONCRETE, 2),
                    ],
                )
            })
            .filter(|&b| b != siding);
        let skirting = loop {
            let s = pick(
                rng,
                &[
                    (STONE_BRICKS, 5),
                    (LIGHT_GRAY_TERRACOTTA, 4),
                    (GRAY_TERRACOTTA, 3),
                    (POLISHED_ANDESITE, 3),
                    (WHITE_TERRACOTTA, 2),
                    (BRICK, 2),
                    (COBBLESTONE, 2),
                ],
            );
            if s != siding {
                break s;
            }
        };
        let roof = pick(
            rng,
            &[
                ((SMOOTH_STONE, SMOOTH_STONE_SLAB), 35),
                ((SMOOTH_QUARTZ, SMOOTH_QUARTZ_SLAB), 20),
                ((POLISHED_ANDESITE, POLISHED_ANDESITE_SLAB), 25),
                ((COBBLED_DEEPSLATE, COBBLED_DEEPSLATE_SLAB), 20),
            ],
        );
        let door = pick(rng, &[(BIRCH_DOOR, 5), (DARK_OAK_DOOR_LOWER, 4)]);
        let glass = pick(rng, &[(GLASS, 4), (LIGHT_GRAY_STAINED_GLASS, 1)]);
        let shutters = rng.random_bool(0.35).then(|| {
            pick(
                rng,
                &[
                    (DARK_OAK_TRAPDOOR, 3),
                    (SPRUCE_TRAPDOOR, 2),
                    (IRON_TRAPDOOR, 1),
                ],
            )
        });
        let (planks, stairs) = pick(
            rng,
            &[
                ((OAK_PLANKS, OAK_STAIRS), 3),
                ((SPRUCE_PLANKS, SPRUCE_STAIRS), 2),
            ],
        );
        Self {
            siding,
            band,
            skirting,
            roof,
            door,
            glass,
            shutters,
            planks,
            stairs,
            peaked: rng.random_bool(0.35),
        }
    }
}

/// The unit's cells and axes: `along` its length, `front` out of the side its door is on.
struct Shell {
    body: FnvHashSet<(i32, i32)>,
    frame: Frame,
    along: (i32, i32),
    front: (i32, i32),
    length: i32,
    width: i32,
}

impl Shell {
    fn of(element: &ProcessedWay, fill: &[(i32, i32)]) -> Option<Self> {
        let frame = Frame::of(&element.nodes)?;
        let mut body: FnvHashSet<(i32, i32)> = fill.iter().copied().collect();
        for w in element.nodes.windows(2) {
            for (x, _, z) in bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z) {
                body.insert((x, z));
            }
        }
        let along = snap(frame.along);
        // Square to the snapped long axis, so a unit turned 45 degrees keeps two axes.
        let across = if along.0 != 0 {
            (0, if frame.across.1 >= 0.0 { 1 } else { -1 })
        } else {
            (if frame.across.0 >= 0.0 { 1 } else { -1 }, 0)
        };
        Some(Self {
            body,
            frame,
            along,
            front: across,
            length: frame.length().round() as i32,
            width: frame.width().round() as i32,
        })
    }

    fn back(&self) -> (i32, i32) {
        (-self.front.0, -self.front.1)
    }

    fn has(&self, x: i32, z: i32) -> bool {
        self.body.contains(&(x, z))
    }

    fn is_wall(&self, (x, z): (i32, i32)) -> bool {
        SIDES.iter().any(|d| !self.has(x + d.0, z + d.1))
    }

    fn faces(&self, (x, z): (i32, i32), d: (i32, i32)) -> bool {
        !self.has(x + d.0, z + d.1)
    }

    fn at(&self, (x, z): (i32, i32)) -> i32 {
        self.frame.local(x, z).0.round() as i32
    }

    fn across_at(&self, (x, z): (i32, i32)) -> i32 {
        self.frame.local(x, z).1.round() as i32
    }

    fn sorted(&self) -> Vec<(i32, i32)> {
        let mut cells: Vec<(i32, i32)> = self.body.iter().copied().collect();
        cells.sort_unstable();
        cells
    }

    /// Blocks to the nearer long side, measured square to the unit.
    fn depth(&self, (x, z): (i32, i32)) -> f64 {
        let across = self.frame.local(x, z).1;
        across.min(self.frame.width() - across).max(0.0)
    }

    /// Whether a wall cell is where the wall turns or steps along the unit's length.
    fn corner(&self, c: (i32, i32)) -> bool {
        self.faces(c, self.along) || self.faces(c, (-self.along.0, -self.along.1))
    }

    /// The wall cells facing `d` by their place along the unit, one per place, corners last.
    fn side(&self, d: (i32, i32)) -> BTreeMap<i32, (i32, i32)> {
        let mut cells: BTreeMap<i32, ((i32, i32), bool)> = BTreeMap::new();
        for c in self.sorted().into_iter().filter(|&c| self.faces(c, d)) {
            let corner = self.corner(c);
            let entry = cells.entry(self.at(c)).or_insert((c, corner));
            if entry.1 && !corner {
                *entry = (c, corner);
            }
        }
        cells.into_iter().map(|(a, (c, _))| (a, c)).collect()
    }
}

fn snap((dx, dz): (f64, f64)) -> (i32, i32) {
    if dx.abs() >= dz.abs() {
        (if dx >= 0.0 { 1 } else { -1 }, 0)
    } else {
        (0, if dz >= 0.0 { 1 } else { -1 })
    }
}

fn stair_facing(d: (i32, i32)) -> StairFacing {
    match d {
        (1, _) => StairFacing::East,
        (-1, _) => StairFacing::West,
        (_, 1) => StairFacing::South,
        _ => StairFacing::North,
    }
}

fn step((x, z): (i32, i32), d: (i32, i32), k: i32) -> (i32, i32) {
    (x + d.0 * k, z + d.1 * k)
}

/// The wall cell placed nearest `at` along a side.
fn nearest(side: &BTreeMap<i32, (i32, i32)>, at: i32) -> Option<(i32, i32)> {
    side.iter()
        .min_by_key(|(&a, _)| ((a - at).abs(), a))
        .map(|(_, &c)| c)
}

/// What a unit keeps clear of.
pub struct Surroundings<'a> {
    pub road_mask: &'a RoadMaskBitmap,
    pub footprints: &'a BuildingFootprintBitmap,
}

/// Window places along the two long sides, kept clear of the doors.
#[derive(Clone, Copy)]
struct Windows {
    period: i32,
    pane: i32,
    phases: [i32; 2],
    door_at: i32,
    rear_at: Option<i32>,
    length: i32,
}

impl Windows {
    fn at(&self, front: bool, at: i32) -> bool {
        let phase = self.phases[usize::from(!front)];
        let door = if front {
            Some(self.door_at)
        } else {
            self.rear_at
        };
        at > 1
            && at < self.length - 1
            && (at - phase).rem_euclid(self.period) < self.pane
            && door.is_none_or(|d| (at - d).abs() > 1)
    }
}

/// Absolute heights of one unit.
#[derive(Clone, Copy)]
struct Rows {
    ground: i32,
    /// The floor, level with the top of the skirting.
    floor: i32,
    /// The top row of the walls; the roof starts above it.
    top: i32,
    window: (i32, i32),
}

struct Site<'a, 'b> {
    editor: &'a mut WorldEditor<'b>,
    shell: Shell,
    rows: Rows,
    look: Look,
    windows: Windows,
    around: Surroundings<'a>,
    terrain: bool,
    scale: f64,
    glazed: FnvHashSet<(i32, i32)>,
    doors: Vec<(i32, i32)>,
    /// Ground beside the unit already holding its steps, deck or yard things.
    taken: FnvHashSet<(i32, i32)>,
}

/// Draws a mobile home over `ground`, the height under its highest corner; false if too small.
pub fn generate(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    fill: &[(i32, i32)],
    ground: i32,
    args: &Args,
    around: Surroundings,
) -> bool {
    let Some(mut shell) = Shell::of(element, fill) else {
        return false;
    };
    if shell.width < 3 || shell.length < 5 || shell.body.len() < 15 {
        return false;
    }
    let s = args.scale;
    let mut rng = element_rng(element.id ^ 0x7A11_E400_0000_0001);
    let look = Look::pick(&mut rng);

    // The door goes on the long side nearer a road, else on either.
    let reach = shell.width + (12.0 * s).round() as i32;
    let road = |shell: &Shell, d: (i32, i32)| -> i32 {
        let mut best = i32::MAX;
        for (_, c) in shell.side(d) {
            if let Some(k) =
                (1..=reach).find(|&k| around.road_mask.contains(c.0 + d.0 * k, c.1 + d.1 * k))
            {
                best = best.min(k);
            }
        }
        best
    };
    let (ahead, behind) = (road(&shell, shell.front), road(&shell, shell.back()));
    if behind < ahead || (behind == ahead && rng.random_bool(0.5)) {
        shell.front = shell.back();
    }

    let skirt = ((0.7 * s).round() as i32).max(1);
    let floor = ground + skirt;
    let top = floor + ((2.4 * s).round() as i32).max(2);
    let sill = floor + ((1.0 * s).round() as i32).max(1) + 1;
    let rows = Rows {
        ground,
        floor,
        top,
        window: (sill.min(top), top - (0.4 * s).round() as i32),
    };
    let door_share = [0.3, 0.35, 0.65, 0.7][rng.random_range(0..4)];
    let door_at =
        ((shell.length as f64 * door_share).round() as i32).clamp(2, (shell.length - 3).max(2));
    let period = ((3.5 * s).round() as i32).max(3);
    let windows = Windows {
        period,
        pane: if rng.random_bool(0.5) { 1 } else { 2 },
        phases: [rng.random_range(0..period), rng.random_range(0..period)],
        door_at,
        rear_at: rng.random_bool(0.4).then_some(shell.length - door_at),
        length: shell.length,
    };

    let mut site = Site {
        editor,
        shell,
        rows,
        look,
        windows,
        around,
        terrain: args.terrain(),
        scale: s,
        glazed: FnvHashSet::default(),
        doors: Vec::new(),
        taken: FnvHashSet::default(),
    };
    site.build_shell();
    site.roof();
    site.entrances(&mut rng);
    site.yard(&mut rng);
    if args.interior {
        site.furnish();
    }
    true
}

impl Site<'_, '_> {
    fn put(&mut self, block: Block, x: i32, y: i32, z: i32) {
        self.editor
            .set_block_absolute(block, x, y, z, None, Some(&[]));
    }

    fn put_props(&mut self, block: BlockWithProperties, x: i32, y: i32, z: i32) {
        self.editor
            .set_block_with_properties_absolute(block, x, y, z, None, Some(&[]));
    }

    fn ground_at(&self, x: i32, z: i32) -> i32 {
        if self.terrain {
            self.editor.terrain_level(x, z).unwrap_or(self.rows.ground)
        } else {
            self.rows.ground
        }
    }

    /// Open ground beside the unit, below its floor: no building, road, water or own yard thing.
    fn free(&self, x: i32, z: i32) -> bool {
        !self.shell.has(x, z)
            && !self.taken.contains(&(x, z))
            && !self.around.footprints.contains(x, z)
            && !self.around.road_mask.contains(x, z)
            && !self.editor.is_mapped_water(x, z)
            && self.ground_at(x, z) < self.rows.floor
    }

    fn eave(&self) -> i32 {
        self.rows.top + 1
    }

    fn build_shell(&mut self) {
        let Look {
            siding,
            band,
            skirting,
            glass,
            planks,
            ..
        } = self.look;
        let rows = self.rows;
        let (front, back) = (self.shell.front, self.shell.back());
        let ends = [self.shell.along, (-self.shell.along.0, -self.shell.along.1)];
        let width = self.shell.width;
        let end_windows = if width as f64 / self.scale > 6.5 {
            vec![width / 3, width - width / 3]
        } else {
            vec![width / 2]
        };
        for (x, z) in self.shell.sorted() {
            if !self.shell.is_wall((x, z)) {
                self.put(planks, x, rows.floor, z);
                continue;
            }
            for y in self.ground_at(x, z).min(rows.floor)..=rows.floor {
                self.put(skirting, x, y, z);
            }
            // On a turned unit only the straight stretches between the steps get windows.
            let at = self.shell.at((x, z));
            let straight = !self.shell.corner((x, z));
            let window = if self.shell.faces((x, z), front) {
                straight && self.windows.at(true, at)
            } else if self.shell.faces((x, z), back) {
                straight && self.windows.at(false, at)
            } else {
                ends.iter().any(|&d| self.shell.faces((x, z), d))
                    && end_windows.contains(&self.shell.across_at((x, z)))
            };
            if window {
                self.glazed.insert((x, z));
            }
            for y in rows.floor + 1..=rows.top {
                let block = if window && (rows.window.0..=rows.window.1).contains(&y) {
                    glass
                } else if y == rows.floor + 1 {
                    band.unwrap_or(siding)
                } else {
                    siding
                };
                self.put(block, x, y, z);
            }
        }
    }

    /// Slabs along the eaves rising to a low ridge, a little higher on a double-wide.
    fn roof(&mut self) {
        let s = self.scale;
        let double = self.shell.width as f64 / s > 6.5;
        let cells = self.shell.sorted();
        let middle = cells
            .iter()
            .map(|&c| self.shell.depth(c))
            .fold(0.0, f64::max);
        // Half-block steps counted down from the middle, so a turned unit keeps an unbroken ridge.
        let (steps, run) = if double {
            (
                ((middle - 0.5) / 2.0)
                    .floor()
                    .clamp(0.0, (2.0 * s).round().max(2.0)),
                2.0,
            )
        } else if self.look.peaked && middle >= 1.75 {
            (1.0, 0.5)
        } else {
            (0.0, 1.0)
        };
        let (block, slab) = self.look.roof;
        let eave = self.eave();
        for (x, z) in cells {
            let d = self.shell.depth((x, z));
            let halves = if d < 0.5 {
                1
            } else {
                2 + (steps - ((middle - d) / run).floor()).max(0.0) as i32
            };
            for i in 0..halves / 2 {
                self.put(block, x, eave + i, z);
            }
            if halves % 2 == 1 {
                self.put(slab, x, eave + halves / 2, z);
            }
        }
    }

    /// The front door with steps or a deck, maybe a back door, and shutters on the front.
    fn entrances(&mut self, rng: &mut impl Rng) {
        let (front, back) = (self.shell.front, self.shell.back());
        let door_at = self.windows.door_at;
        let Some(door) = nearest(&self.shell.side(front), door_at) else {
            return;
        };
        self.door(door, front);
        let porch = rng.random_bool(0.35);
        if !(rng.random_bool(0.45) && self.deck(door, front, porch)) {
            self.steps(door, front);
        }
        self.taken.insert(step(door, front, 1));
        if let Some(rear_at) = self.windows.rear_at {
            if let Some(rear) = nearest(&self.shell.side(back), rear_at) {
                self.door(rear, back);
                self.steps(rear, back);
                self.taken.insert(step(rear, back, 1));
            }
        }
        if let Some(shutter) = self.look.shutters {
            self.shutters(shutter);
        }
    }

    fn door(&mut self, (x, z): (i32, i32), out: (i32, i32)) {
        self.doors.push((x, z));
        self.glazed.remove(&(x, z));
        let facing = facing_for_normal(out.0, out.1);
        for (dy, half) in [(1, "lower"), (2, "upper")] {
            let leaf = cached_prop_block(
                self.look.door,
                &[("half", half), ("facing", facing), ("hinge", "left")],
            );
            self.put_props(leaf, x, self.rows.floor + dy, z);
        }
        // A turned unit can move its door under a window, whose glass would hang over it.
        for y in self.rows.floor + 3..=self.rows.top {
            self.put(self.look.siding, x, y, z);
        }
    }

    /// The stairs from a door or a deck down to the ground, empty on level ground;
    /// None where four steps on open ground do not reach it.
    fn stair_run(&self, from: (i32, i32), out: (i32, i32)) -> Option<Vec<((i32, i32), i32)>> {
        let first = step(from, out, 1);
        let drop = self.rows.floor - self.ground_at(first.0, first.1);
        if drop > 4 {
            return None;
        }
        let run: Vec<((i32, i32), i32)> = (1..=drop)
            .map(|i| (step(from, out, i), self.rows.floor + 1 - i))
            .collect();
        run.iter()
            .all(|&((x, z), y)| self.free(x, z) && self.ground_at(x, z) < y)
            .then_some(run)
    }

    fn steps(&mut self, from: (i32, i32), out: (i32, i32)) {
        let Some(run) = self.stair_run(from, out) else {
            return;
        };
        let stair = create_stair_with_properties(
            self.look.stairs,
            stair_facing((-out.0, -out.1)),
            StairShape::Straight,
        );
        for ((x, z), y) in run {
            self.put_props(stair.clone(), x, y, z);
            self.taken.insert((x, z));
        }
    }

    /// A railed wooden deck before the door at floor height, sometimes under a roof.
    fn deck(&mut self, door: (i32, i32), out: (i32, i32), porch: bool) -> bool {
        let s = self.scale;
        let width = ((4.0 * s).round() as i32).max(3);
        let depth = ((2.0 * s).round() as i32).max(2);
        let along = self.shell.along;
        let (u0, u1) = (-1, width - 2);
        let mut cells = Vec::new();
        for t in 1..=depth {
            for u in u0..=u1 {
                cells.push((t, u, step(step(door, out, t), along, u)));
            }
        }
        let floor = self.rows.floor;
        if cells
            .iter()
            .any(|&(_, _, (x, z))| !self.free(x, z) || self.ground_at(x, z) < floor - 4)
        {
            return false;
        }
        // No deck whose gate has no way down.
        let gate = step(door, out, depth);
        if self.stair_run(gate, out).is_none() {
            return false;
        }
        let (planks, rail, eave) = (self.look.planks, OAK_FENCE, self.eave());
        for &(t, u, (x, z)) in &cells {
            self.taken.insert((x, z));
            self.put(planks, x, floor, z);
            let corner = t == depth && (u == u0 || u == u1);
            if corner {
                for y in self.ground_at(x, z) + 1..floor {
                    self.put(rail, x, y, z);
                }
            }
            let edge = t == depth || u == u0 || u == u1;
            if edge && !(t == depth && u == 0) {
                self.put(rail, x, floor + 1, z);
            }
            if porch {
                if corner {
                    for y in floor + 2..eave {
                        self.put(rail, x, y, z);
                    }
                }
                self.put(self.look.roof.1, x, eave, z);
            }
        }
        self.steps(gate, out);
        true
    }

    /// Open shutters either side of each front window.
    fn shutters(&mut self, shutter: Block) {
        let front = self.shell.front;
        let block = cached_prop_block(
            shutter,
            &[
                ("facing", facing_for_normal(front.0, front.1)),
                ("open", "true"),
                ("half", "top"),
            ],
        );
        let along = self.shell.along;
        let mut beside: Vec<(i32, i32)> = Vec::new();
        for &w in &self.glazed {
            if !self.shell.faces(w, front) {
                continue;
            }
            for d in [along, (-along.0, -along.1)] {
                let c = step(w, d, 1);
                let wall = self.shell.has(c.0, c.1) && self.shell.faces(c, front);
                if wall && !self.glazed.contains(&c) && !self.doors.contains(&c) {
                    beside.push(step(c, front, 1));
                }
            }
        }
        beside.sort_unstable();
        beside.dedup();
        for (x, z) in beside {
            if !self.free(x, z) {
                continue;
            }
            for y in self.rows.window.0..=self.rows.window.1 {
                self.editor
                    .set_block_with_properties_absolute(block.clone(), x, y, z, None, None);
            }
        }
    }

    /// A carport on the front past the door, an AC unit at the back, a propane tank at an end.
    fn yard(&mut self, rng: &mut impl Rng) {
        let s = self.scale;
        let (front, back) = (self.shell.front, self.shell.back());
        let length = self.shell.length;
        let door_at = self.windows.door_at;
        if rng.random_bool(0.2) {
            let len = ((6.0 * s).round() as i32).max(4);
            let span = if door_at < length / 2 {
                (length - 1 - len, length - 1)
            } else {
                (1, 1 + len)
            };
            let car = rng.random::<u64>();
            self.carport(span, front, car);
        }
        if rng.random_bool(0.5) {
            let at = if rng.random_bool(0.5) {
                length / 3
            } else {
                length - length / 3
            };
            if let Some(c) = nearest(&self.shell.side(back), at) {
                let (x, z) = step(c, back, 1);
                if self.free(x, z) {
                    let y = self.ground_at(x, z) + 1;
                    self.put(SMOOTH_STONE, x, y, z);
                    self.taken.insert((x, z));
                }
            }
        }
        if rng.random_bool(0.3) {
            let end = if rng.random_bool(0.5) {
                self.shell.along
            } else {
                (-self.shell.along.0, -self.shell.along.1)
            };
            // Two blocks lying across the end, a block off the wall.
            let mid = self.shell.width / 2;
            let mut tank: Vec<(i32, (i32, i32))> = self
                .shell
                .sorted()
                .into_iter()
                .filter(|&c| self.shell.faces(c, end))
                .map(|c| (self.shell.across_at(c), step(c, end, 2)))
                .filter(|&(c, _)| c == mid || c == mid + 1)
                .collect();
            tank.dedup_by_key(|e| e.0);
            if tank.len() == 2 && tank.iter().all(|&(_, (x, z))| self.free(x, z)) {
                for (_, (x, z)) in tank {
                    let y = self.ground_at(x, z) + 1;
                    self.put(SMOOTH_QUARTZ, x, y, z);
                    self.taken.insert((x, z));
                }
            }
        }
    }

    fn carport(&mut self, (a0, a1): (i32, i32), out: (i32, i32), seed: u64) {
        let depth = ((3.5 * self.scale).round() as i32).max(3);
        let side = self.shell.side(out);
        let mut cells = Vec::new();
        for a in a0..=a1 {
            let Some(&wall) = side.get(&a) else {
                return;
            };
            for t in 1..=depth {
                cells.push((a, t, step(wall, out, t)));
            }
        }
        let floor = self.rows.floor;
        if cells
            .iter()
            .any(|&(_, _, (x, z))| !self.free(x, z) || self.ground_at(x, z) < floor - 4)
        {
            return;
        }
        let roof = if seed & 1 == 0 {
            SMOOTH_STONE_SLAB
        } else {
            SMOOTH_QUARTZ_SLAB
        };
        let eave = self.eave();
        let mid = (a0 + a1) / 2;
        for &(a, t, (x, z)) in &cells {
            self.taken.insert((x, z));
            self.put(roof, x, eave, z);
            if t == depth && (a == a0 || a == a1 || a == mid) {
                for y in self.ground_at(x, z) + 1..eave {
                    self.put(IRON_BARS, x, y, z);
                }
            }
        }
        if (seed >> 1) % 5 < 3 {
            if let Some(&wall) = side.get(&mid) {
                let (cx, cz) = step(wall, out, depth / 2 + 1);
                let rot = if self.shell.along.0 != 0 { 1 } else { 0 };
                crate::structures::car::place_vehicle(self.editor, cx, cz, rot, false, seed >> 8);
            }
        }
    }

    /// A bed at the far end, a kitchen along the back wall, a couch near the door.
    fn furnish(&mut self) {
        let (front, back) = (self.shell.front, self.shell.back());
        let door_at = self.windows.door_at;
        let doors: Vec<i32> = self.doors.iter().map(|&d| self.shell.at(d)).collect();
        let y = self.rows.floor + 1;
        // Room cells against the back wall, clear of the doors.
        let by_back: Vec<(i32, i32)> = self
            .shell
            .sorted()
            .into_iter()
            .filter(|&c| !self.shell.is_wall(c) && self.shell.is_wall(step(c, back, 1)))
            .filter(|&c| {
                let a = self.shell.at(c);
                (a - door_at).abs() > 1 && doors.iter().all(|&d| (a - d).abs() > 1)
            })
            .map(|c| (self.shell.at(c), c))
            .collect::<BTreeMap<i32, (i32, i32)>>()
            .into_values()
            .collect();
        if by_back.len() < 8 {
            return;
        }
        let room = |c: (i32, i32)| self.shell.has(c.0, c.1) && !self.shell.is_wall(c);
        let toward_far = door_at < self.shell.length / 2;
        let ordered: Vec<(i32, i32)> = if toward_far {
            by_back.iter().rev().copied().collect()
        } else {
            by_back.clone()
        };
        let head = ordered[1];
        let foot = step(head, front, 1);
        if room(foot) {
            let f = facing_for_normal(back.0, back.1);
            for (cell, part) in [(foot, "foot"), (head, "head")] {
                let b = cached_prop_block(
                    RED_BED_NORTH_HEAD,
                    &[("facing", f), ("part", part), ("occupied", "false")],
                );
                self.editor
                    .set_block_with_properties_absolute(b, cell.0, y, cell.1, None, None);
                self.editor.set_bed_block_entity_absolute(cell.0, y, cell.1);
            }
        }
        let toward = facing_for_normal(front.0, front.1);
        let kitchen_at = by_back.len() / 2 - 2..by_back.len() / 2 + 2;
        let kitchen = [BARREL, CAULDRON, FURNACE, BARREL];
        for (&block, &(x, z)) in kitchen.iter().zip(&by_back[kitchen_at.clone()]) {
            if block == CAULDRON {
                self.editor.set_block_absolute(block, x, y, z, None, None);
            } else {
                let block = cached_prop_block(block, &[("facing", toward)]);
                self.editor
                    .set_block_with_properties_absolute(block, x, y, z, None, None);
            }
        }
        let seat =
            create_stair_with_properties(OAK_STAIRS, stair_facing(back), StairShape::Straight);
        // Up to three seats at the door's end, short of the kitchen on a short unit.
        let n = by_back.len();
        let couch: Vec<usize> = if toward_far {
            (0..3).collect()
        } else {
            (n - 3..n).rev().collect()
        };
        for i in couch.into_iter().filter(|i| !kitchen_at.contains(i)) {
            let (x, z) = by_back[i];
            self.editor
                .set_block_with_properties_absolute(seat.clone(), x, y, z, None, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{
        flat_ground_args, rect_way, tag_map, test_editor,
    };
    use crate::floodfill_cache::{CoordinateBitmap, FloodFillCache};

    #[test]
    fn parks_are_tagged_or_named_as_such() {
        let park = |pairs: &[(&str, &str)]| is_trailer_park(&tag_map(pairs));
        assert!(park(&[
            ("landuse", "residential"),
            ("residential", "trailer_park")
        ]));
        assert!(park(&[
            ("landuse", "residential"),
            ("name", "Swift Mobile Home Park")
        ]));
        assert!(park(&[
            ("landuse", "residential"),
            ("name", "Kentinia Trailer Park")
        ]));
        assert!(park(&[
            ("place", "neighbourhood"),
            ("name", "Minari Mobile Home Court")
        ]));
        assert!(park(&[
            ("landuse", "residential"),
            ("name", "Shady Acres MHP")
        ]));
        assert!(!park(&[
            ("landuse", "residential"),
            ("name", "Mobile Terrace")
        ]));
        assert!(!park(&[("landuse", "residential")]));
        assert!(!park(&[
            ("tourism", "caravan_site"),
            ("name", "Pine Trailer Park")
        ]));
    }

    #[test]
    fn mapped_mobile_homes_are_claimed_unless_two_storeys() {
        let claimed = |pairs: &[(&str, &str)]| claims(&tag_map(pairs));
        assert!(claimed(&[("building", "static_caravan")]));
        assert!(claimed(&[
            ("building", "mobile_home"),
            ("building:levels", "1")
        ]));
        assert!(claimed(&[("building", "yes"), (IN_PARK_TAG, "yes")]));
        assert!(!claimed(&[
            ("building", "static_caravan"),
            ("building:levels", "2")
        ]));
        assert!(claimed(&[
            ("building", "static_caravan"),
            ("building:levels", "2"),
            ("arnis:height_source", "overture_maps"),
        ]));
        assert!(!claimed(&[("building", "yes")]));
        assert!(!claimed(&[("building", "house")]));
    }

    #[test]
    fn single_and_double_wides_have_their_own_shape() {
        let shaped = |w: i32, l: i32, scale: f64| {
            shaped_like_one(&rect_way(1, 0, 0, l, w, &[]).nodes, scale)
        };
        assert!(shaped(4, 20, 1.0));
        assert!(shaped(8, 17, 1.0));
        assert!(shaped(8, 40, 2.0));
        assert!(shaped(11, 17, 1.0));
        assert!(!shaped(10, 12, 1.0));
        assert!(!shaped(14, 19, 1.0));
        assert!(!shaped(4, 6, 1.0));
        assert!(!shaped(12, 40, 1.0));
        assert!(!shaped(2, 20, 1.0));
    }

    #[test]
    fn only_plain_long_buildings_in_a_park_are_marked() {
        let ways = [
            rect_way(
                100,
                0,
                0,
                100,
                100,
                &[("landuse", "residential"), ("residential", "trailer_park")],
            ),
            rect_way(1, 10, 10, 30, 14, &[("building", "yes")]),
            rect_way(2, 150, 10, 170, 14, &[("building", "yes")]),
            rect_way(3, 40, 40, 52, 50, &[("building", "yes")]),
            rect_way(
                4,
                10,
                30,
                30,
                34,
                &[("building", "yes"), ("shop", "convenience")],
            ),
            rect_way(5, 10, 50, 30, 54, &[("building", "static_caravan")]),
            rect_way(6, 10, 70, 30, 74, &[("building", "garage")]),
            rect_way(
                7,
                10,
                80,
                30,
                84,
                &[
                    ("building", "apartments"),
                    ("height", "12"),
                    ("source", "overture_maps"),
                ],
            ),
            rect_way(8, 40, 80, 60, 84, &[("building", "yes"), ("height", "12")]),
        ];
        let mut elements: Vec<ProcessedElement> =
            ways.into_iter().map(ProcessedElement::Way).collect();
        prepare(&mut elements, 1.0);
        let marked: Vec<u64> = elements
            .iter()
            .filter(|e| e.tags().contains_key(IN_PARK_TAG))
            .map(|e| e.id())
            .collect();
        assert_eq!(marked, vec![1, 7]);
        let claimed: Vec<u64> = elements
            .iter()
            .filter(|e| claims(e.tags()))
            .map(|e| e.id())
            .collect();
        assert_eq!(claimed, vec![1, 5, 7]);
    }

    #[test]
    fn a_single_wide_stands_low_with_a_door_and_windows() {
        let xz = XZBBox::rect_from_xz_lengths(80.0, 80.0).unwrap();
        let args = flat_ground_args();
        let empty = CoordinateBitmap::new(&xz);
        for id in 0..12u64 {
            let mut editor = test_editor(&xz);
            let way = rect_way(id, 20, 20, 40, 24, &[("building", "static_caravan")]);
            let fill = FloodFillCache::new().get_or_compute(&way, None);
            let around = Surroundings {
                road_mask: &empty,
                footprints: &empty,
            };
            assert!(generate(&mut editor, &way, &fill, 0, &args, around));
            let block = |x: i32, y: i32, z: i32| editor.get_block_absolute(x, y, z);
            let (mut doors, mut glass) = (0, 0);
            for x in 20..=40 {
                for z in 20..=24 {
                    assert!(block(x, 4, z).is_some(), "roof missing at {x},{z}");
                    assert!(block(x, 6, z).is_none(), "{x},{z} rises past a low roof");
                    if x == 20 || x == 40 || z == 20 || z == 24 {
                        assert!(block(x, 1, z).is_some(), "no skirting at {x},{z}");
                    }
                    let door = block(x, 2, z).is_some_and(|b| b.name().ends_with("_door"));
                    doors += usize::from(door);
                    let pane = block(x, 3, z).is_some_and(|b| b.name().contains("glass"));
                    glass += usize::from(pane);
                }
            }
            assert!(doors >= 1, "unit {id} has no door");
            assert!(glass >= 4, "unit {id} has {glass} windows");
        }
    }

    #[test]
    fn every_door_is_wooden_with_no_glass_over_it_and_a_way_down() {
        let xz = XZBBox::rect_from_xz_lengths(160.0, 160.0).unwrap();
        let empty = CoordinateBitmap::new(&xz);
        for s in [1, 2] {
            let mut args = flat_ground_args();
            args.scale = f64::from(s);
            let (x1, z1) = (20 + 20 * s, 20 + 4 * s);
            let top = 1 + ((2.4 * args.scale).round() as i32).max(2);
            for id in 0..40u64 {
                let mut editor = test_editor(&xz);
                let way = rect_way(id, 20, 20, x1, z1, &[("building", "static_caravan")]);
                let fill = FloodFillCache::new().get_or_compute(&way, None);
                let around = Surroundings {
                    road_mask: &empty,
                    footprints: &empty,
                };
                assert!(generate(&mut editor, &way, &fill, 0, &args, around));
                let block = |x: i32, y: i32, z: i32| editor.get_block_absolute(x, y, z);
                for x in 20..=x1 {
                    for (z, out) in [(20, -1), (z1, 1)] {
                        let Some(door) = block(x, 2, z).filter(|b| b.name().ends_with("_door"))
                        else {
                            continue;
                        };
                        assert!(!door.name().contains("iron"), "unit {id} has an iron door");
                        for y in 4..=top {
                            let glass = block(x, y, z).is_some_and(|b| b.name().contains("glass"));
                            assert!(!glass, "unit {id} at scale {s} has glass over its door");
                        }
                        let first = block(x, 1, z + out).map(|b| b.name().to_string());
                        assert!(
                            first
                                .as_deref()
                                .is_some_and(|n| n.ends_with("_stairs") || n.ends_with("_planks")),
                            "unit {id} at scale {s}: door at {x},{z} steps onto {first:?}"
                        );
                    }
                }
            }
        }
    }
}
