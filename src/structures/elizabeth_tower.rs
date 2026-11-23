//! The Elizabeth Tower (Big Ben) at one block per metre. Stage heights are read off a level photo
//! from Methodist Central Hall, scaled between the 55 m dial centre and the 96 m cross; the plan is
//! the mapped outline, which turns 6.4 degrees off north while the model stands square.

use std::collections::BTreeMap;

use super::schematic::PalettizedSchematic;
use crate::block_definitions::*;
use crate::element_processing::connected_blocks::{connected_fence, connected_wall};
use crate::element_processing::structure_kit::with_props;

/// Model grid side; the tower is centred on block `C`.
const SIDE: i32 = 17;
const C: i32 = 8;

// Anston limestone, cast iron and gilding.
const ASHLAR: Block = SMOOTH_SANDSTONE;
const ASHLAR_STAIRS: Block = SMOOTH_SANDSTONE_STAIRS;
const PANEL: Block = CUT_SANDSTONE;
const CARVED: Block = CHISELED_SANDSTONE;
const PINNACLE: Block = SANDSTONE_WALL;
/// Painted iron plates of the roofs, mid grey in daylight since the 2017-2022 restoration.
const PLATE: Block = STONE_BRICKS;
const PLATE_STAIRS: Block = STONE_BRICK_STAIRS;
const PLATE_WALL: Block = STONE_BRICK_WALL;
const PLATE_SLAB: Block = STONE_BRICK_SLAB;
/// Dark ironwork: poles, shafts, the lantern's core.
const IRON: Block = POLISHED_DEEPSLATE;
const IRON_WALL: Block = POLISHED_DEEPSLATE_WALL;
const GILT: Block = GOLD_BLOCK;
/// Gilding too fine for solid gold.
const LEAF: Block = YELLOW_TERRACOTTA;
/// The hands' Prussian blue, near black from the street.
const HANDS: Block = BLACK_CONCRETE;
/// Dark louvres of the roof's dormers.
const LOUVRE: Block = DEEPSLATE_TILES;
/// A window slit a fifth of a block wide.
const SLIT: Block = CHAIN_Y;

/// Panelled tiers of the shaft, bottom and top layer.
const TIERS: [(i32, i32); 5] = [(2, 7), (9, 16), (19, 26), (29, 36), (39, 46)];
/// Carved bands between the tiers; a two-layer band has a string course on top.
const BANDS: [(i32, i32); 4] = [(8, 8), (17, 18), (27, 28), (37, 38)];
/// Corbel table under the clock stage.
const CORNICE: i32 = 47;
const INSCRIPTION: i32 = 50;
const DIAL_Y: i32 = 55;
const SHIELDS: i32 = 60;
/// Lowest layer of the belfry's openings; their heads are three up.
const BELFRY: i32 = 62;
/// Heraldic band under the roof's eave.
const CREST: i32 = 66;
/// Layer, half-width and whether it steps in above, from the eave to the lantern's gallery.
const ROOF: [(i32, i32, bool); 6] = [
    (67, 5, false),
    (68, 5, true),
    (69, 4, false),
    (70, 4, true),
    (71, 3, false),
    (72, 3, true),
];
const GALLERY: i32 = 73;
/// Gilded head of the Ayrton Light's arcade.
const LANTERN_HEAD: i32 = 78;
/// Layer, half-width and whether the corners are cut, for the spire's courses from the lantern's
/// top to the needle; cut corners between the steps keep the hips a near-straight line.
const SPIRE: [(i32, i32, bool); 10] = [
    (80, 2, false),
    (81, 2, false),
    (82, 2, true),
    (83, 2, true),
    (84, 1, false),
    (85, 1, false),
    (86, 1, true),
    (87, 1, true),
    (88, 0, false),
    (89, 0, false),
];
/// The gilded orb on the needle; the cross tops out at 96 m.
const ORB: i32 = 92;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Dir {
    N,
    E,
    S,
    W,
}

impl Dir {
    const ALL: [Dir; 4] = [Dir::N, Dir::E, Dir::S, Dir::W];

    fn step(self) -> (i32, i32) {
        match self {
            Dir::N => (0, -1),
            Dir::E => (1, 0),
            Dir::S => (0, 1),
            Dir::W => (-1, 0),
        }
    }

    fn back(self) -> Dir {
        Dir::ALL[(self as usize + 2) % 4]
    }

    fn counter_clockwise(self) -> Dir {
        Dir::ALL[(self as usize + 3) % 4]
    }

    fn name(self) -> &'static str {
        ["north", "east", "south", "west"][self as usize]
    }

    fn stair(self) -> StairFacing {
        [
            StairFacing::North,
            StairFacing::East,
            StairFacing::South,
            StairFacing::West,
        ][self as usize]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Part {
    Block(Block),
    /// Stairs with their tall back toward the direction, hung upside down if `true`; the corner
    /// shape comes from the neighbours, as in the game.
    Stair(Block, Dir, bool),
    /// A wall, pane or bars joined to its neighbours.
    Joined(Block),
    /// A button on the block behind it, facing out.
    Button(Block, Dir),
    /// An invisible light block of this level, for the floodlighting.
    Light(u8),
    /// A slab in the upper half of its cell.
    SlabTop(Block),
}

/// Viewer-right step along the north, east, south and west faces, seen from outside.
const RIGHT: [(i32, i32); 4] = [(-1, 0), (0, -1), (1, 0), (0, 1)];

#[derive(Default)]
struct Tower {
    cells: BTreeMap<(i32, i32, i32), Part>,
}

impl Tower {
    fn set(&mut self, x: i32, y: i32, z: i32, part: Part) {
        self.cells.insert((x, y, z), part);
    }

    fn get(&self, x: i32, y: i32, z: i32) -> Option<Part> {
        self.cells.get(&(x, y, z)).copied()
    }

    fn cell(f: usize, u: i32, d: i32) -> (i32, i32) {
        let ((rx, rz), (nx, nz)) = (RIGHT[f], Dir::ALL[f].step());
        (C + rx * u + nx * d, C + rz * u + nz * d)
    }

    /// Face `f` at `u` blocks right of centre and `d` out from the axis.
    fn at(&mut self, f: usize, u: i32, y: i32, d: i32, part: Part) {
        let (x, z) = Self::cell(f, u, d);
        self.set(x, y, z, part);
    }

    fn clear(&mut self, f: usize, u: i32, y: i32, d: i32) {
        let (x, z) = Self::cell(f, u, d);
        self.cells.remove(&(x, y, z));
    }

    /// Cells `d` out with |u| up to `w` on every face, through `pick(face, u)`.
    fn ring(&mut self, y: i32, d: i32, w: i32, mut pick: impl FnMut(usize, i32) -> Option<Part>) {
        for f in 0..4 {
            // A corner is shared with the next face; the face it ends on the right of takes it.
            let from = if w >= d { 1 - d } else { -w };
            for u in from..=w {
                if let Some(part) = pick(f, u) {
                    self.at(f, u, y, d, part);
                }
            }
        }
    }

    fn fill(&mut self, y: i32, h: i32, part: Part) {
        for x in C - h..=C + h {
            for z in C - h..=C + h {
                self.set(x, y, z, part);
            }
        }
    }

    /// A pyramid layer of half-width `h`, its edge stairs if the next layer steps in, so each
    /// slope stays one plane without ledges.
    fn pyramid(&mut self, y: i32, h: i32, step: bool) {
        self.fill(y, h, b(PLATE));
        if step {
            self.ring(y, h, h, |f, _| Some(up(PLATE_STAIRS, f)));
        }
    }
}

fn b(block: Block) -> Part {
    Part::Block(block)
}

fn inward(f: usize) -> Dir {
    Dir::ALL[f].back()
}

/// Stairs rising toward the axis from face `f`.
fn up(block: Block, f: usize) -> Part {
    Part::Stair(block, inward(f), false)
}

/// Upside-down stairs on face `f`, open underneath on the outside.
fn hood(block: Block, f: usize) -> Part {
    Part::Stair(block, inward(f), true)
}

pub fn build() -> PalettizedSchematic {
    let mut t = Tower::default();
    plinth(&mut t);
    shaft(&mut t);
    clock_stage(&mut t);
    belfry(&mut t);
    roof(&mut t);
    lantern(&mut t);
    spire(&mut t);
    floodlights(&mut t);
    finish(&t)
}

fn plinth(t: &mut Tower) {
    t.fill(0, 7, b(ASHLAR));
    t.ring(1, 7, 7, |f, _| Some(up(ASHLAR_STAIRS, f)));
    t.ring(1, 6, 6, |_, _| Some(b(ASHLAR)));
}

/// The shaft's face at `u` across: the cell in the face and the one behind it.
fn shaft_cell(f: usize, u: i32, y: i32) -> (Option<Part>, Option<Part>) {
    let solid = |block| (Some(b(block)), None);
    let a = u.abs();
    if let Some(&(lo, hi)) = BANDS.iter().find(|r| (r.0..=r.1).contains(&y)) {
        // A string course over a carved frieze.
        return if y == hi && hi > lo {
            (Some(hood(ASHLAR_STAIRS, f)), Some(b(ASHLAR)))
        } else if a < 6 {
            solid(CARVED)
        } else {
            solid(ASHLAR)
        };
    }
    let Some(&(lo, hi)) = TIERS.iter().find(|r| (r.0..=r.1).contains(&y)) else {
        return solid(ASHLAR);
    };
    // Corner piers, then the field's blind panels between the glazed ones.
    if a == 6 {
        return solid(ASHLAR);
    }
    if a == 5 || a % 2 == 0 {
        return solid(PANEL);
    }
    // Sunk panels under cusped heads, each with a slit of leaded glazing down its middle.
    match y {
        _ if y == hi => (Some(hood(ASHLAR_STAIRS, f)), Some(b(ASHLAR))),
        _ if y == lo => (Some(up(ASHLAR_STAIRS, f)), Some(b(ASHLAR))),
        _ => (Some(b(SLIT)), Some(b(ASHLAR))),
    }
}

fn shaft(t: &mut Tower) {
    for y in 2..CORNICE {
        for f in 0..4 {
            for u in -5..=6 {
                let (front, back) = shaft_cell(f, u, y);
                if let Some(front) = front {
                    t.at(f, u, y, 6, front);
                }
                // A corner's backing would land in the next face's front.
                if let Some(back) = back.filter(|_| u < 6) {
                    t.at(f, u, y, 5, back);
                }
            }
        }
    }
    // Clasping buttresses round the lower corners, sloped off under the second band.
    for y in 2..=16 {
        t.ring(y, 7, 7, |f, u| {
            (u.abs() >= 6).then(|| {
                if y < 16 {
                    b(ASHLAR)
                } else {
                    up(ASHLAR_STAIRS, f)
                }
            })
        });
    }
}

/// The dial and its square frame at `u` across and `dy` above the centre: the cell in the face and
/// the one behind it.
fn dial(u: i32, dy: i32) -> (Part, Block) {
    let in_disc = |u: i32, dy: i32| u * u + dy * dy <= 12;
    if dy.abs() == 4 {
        return (b(PANEL), ASHLAR);
    }
    if !in_disc(u, dy) {
        // Gilded spandrels round a rosette in each corner.
        let rosette = u.abs() == 3 && dy.abs() == 3;
        return (b(if rosette { GILT } else { LEAF }), ASHLAR);
    }
    // Hands at a quarter past twelve, as mapped: the minute hand out to the III, the shorter hour
    // hand toward the XII.
    if matches!((u, dy), (0, 0) | (1, 0) | (2, 0) | (3, 0) | (0, 1) | (0, 2)) {
        return (b(HANDS), SEA_LANTERN);
    }
    // Opal glass lit from behind.
    (b(WHITE_STAINED_GLASS), SEA_LANTERN)
}

/// The chapter ring: the disc's edge, where the numerals sit.
fn on_ring(u: i32, dy: i32) -> bool {
    let in_disc = |u: i32, dy: i32| u * u + dy * dy <= 12;
    in_disc(u, dy)
        && !(in_disc(u + 1, dy) && in_disc(u - 1, dy) && in_disc(u, dy + 1) && in_disc(u, dy - 1))
}

fn clock_stage(t: &mut Tower) {
    // Corbel table carrying the clock stage.
    t.ring(CORNICE, 7, 7, |f, _| Some(hood(ASHLAR_STAIRS, f)));
    t.ring(CORNICE, 6, 6, |_, _| Some(b(ASHLAR)));
    for y in CORNICE + 1..=SHIELDS {
        let dy = y - DIAL_Y;
        for f in 0..4 {
            for u in -6..=6i32 {
                let a = u.abs();
                let (front, back) = match y {
                    // Arcade of the clock room, glazed in blue under the dial.
                    _ if y < INSCRIPTION && a <= 4 && a % 2 == 0 => {
                        let head = (y == INSCRIPTION - 1).then(|| hood(ASHLAR_STAIRS, f));
                        (head, BLUE_STAINED_GLASS)
                    }
                    _ if y < INSCRIPTION => (Some(b(if a == 5 { CARVED } else { PANEL })), ASHLAR),
                    // Gilded pilasters either side of the dial.
                    _ if a == 4 => (Some(b(if y % 2 == 0 { LEAF } else { PANEL })), ASHLAR),
                    // The Latin inscription's blue band.
                    INSCRIPTION if a < 4 => (Some(b(LAPIS_BLOCK)), ASHLAR),
                    // St George shields over the dial.
                    SHIELDS if a < 4 => {
                        let shield = if a % 2 == 1 { WHITE_CONCRETE } else { PANEL };
                        (Some(b(shield)), ASHLAR)
                    }
                    _ if a < 4 => {
                        let (front, back) = dial(u, dy);
                        (Some(front), back)
                    }
                    // Panels with carved lozenges on the corner piers.
                    _ if a == 5 && (y == 53 || y == 57) => (Some(b(CARVED)), ASHLAR),
                    _ if a == 5 => (Some(b(PANEL)), ASHLAR),
                    _ => (Some(b(ASHLAR)), ASHLAR),
                };
                match front {
                    Some(front) => t.at(f, u, y, 6, front),
                    None => t.clear(f, u, y, 6),
                }
                if a < 6 {
                    t.at(f, u, y, 5, b(back));
                }
                // Dark behind the arcade's glass, not the far side.
                if back == BLUE_STAINED_GLASS {
                    t.at(f, u, y, 4, b(BLACK_CONCRETE));
                }
                if a < 4 && (y - DIAL_Y).abs() < 4 && on_ring(u, dy) {
                    t.at(
                        f,
                        u,
                        y,
                        7,
                        Part::Button(POLISHED_BLACKSTONE_BUTTON, Dir::ALL[f]),
                    );
                }
            }
            // Carved beasts at the corners, at the shields and under the inscription.
            if y == SHIELDS || y == INSCRIPTION {
                t.at(f, 6, y, 7, hood(ASHLAR_STAIRS, f));
            }
        }
    }
}

fn belfry(t: &mut Tower) {
    // Pierced parapet with gilded lozenges in front of a walk round the belfry.
    t.ring(SHIELDS + 1, 6, 6, |_, u| {
        Some(match u.abs() {
            4 => b(PANEL),
            6 => b(CARVED),
            a if a % 2 == 0 => b(LEAF),
            _ => Part::Joined(PINNACLE),
        })
    });
    // Pinnacles along it, gilded on the pilasters.
    t.ring(SHIELDS + 2, 6, 6, |_, u| match u.abs() {
        4 => Some(b(LEAF)),
        a if a % 2 == 1 || a == 6 => Some(Part::Joined(PINNACLE)),
        _ => None,
    });
    t.fill(SHIELDS + 1, 4, b(DARK_OAK_PLANKS));
    t.ring(SHIELDS + 1, 5, 5, |_, _| Some(b(ASHLAR)));
    for y in BELFRY..=BELFRY + 3 {
        // Pointed openings parted by slender shafts.
        t.ring(y, 5, 5, |f, u| {
            let open = u.abs() <= 4 && u % 2 == 0;
            match (y - BELFRY, u.abs()) {
                (3, _) if open => Some(hood(ASHLAR_STAIRS, f)),
                _ if open => None,
                (0..=2, 1 | 3) => Some(Part::Joined(PINNACLE)),
                (_, 5) => Some(b(CARVED)),
                _ => Some(b(ASHLAR)),
            }
        });
        // Louvres behind the openings.
        if y < BELFRY + 3 {
            t.fill(y, 3, b(DEEPSLATE_TILES));
        }
    }
    // Corner turrets with crocketed pinnacles.
    for f in 0..4 {
        for y in SHIELDS + 3..CREST {
            t.at(f, 6, y, 6, Part::Joined(PINNACLE));
        }
    }
    // Heraldic band under the eave: shields between gilding.
    t.fill(CREST, 5, b(PLATE));
    t.ring(CREST, 6, 6, |_, u| {
        Some(b(match u.abs() {
            1 | 5 => DARK_PRISMARINE,
            3 => RED_CONCRETE,
            _ => LEAF,
        }))
    });
}

fn roof(t: &mut Tower) {
    for &(y, h, step) in &ROOF {
        t.pyramid(y, h, step);
    }
    // Two staggered rows of small dormers: a dark louvre under a little pitched hood.
    for (y, h, us) in [(68, 5, &[-3, -1, 1, 3][..]), (70, 4, &[-2, 0, 2][..])] {
        for f in 0..4 {
            for &u in us {
                t.at(f, u, y, h, b(LOUVRE));
                t.at(f, u, y + 1, h, up(PLATE_STAIRS, f));
            }
        }
    }
    // Iron poles with gilded vanes over the corners.
    for f in 0..4 {
        for y in CREST + 1..=CREST + 4 {
            t.at(f, 6, y, 6, Part::Joined(IRON_WALL));
        }
        t.at(f, 6, CREST + 5, 6, b(LEAF));
    }
}

fn lantern(t: &mut Tower) {
    // A thin railed gallery round the lantern's foot.
    t.fill(GALLERY, 3, b(PLATE));
    t.ring(GALLERY, 4, 4, |_, _| Some(Part::SlabTop(PLATE_SLAB)));
    t.ring(GALLERY + 1, 4, 4, |_, _| Some(Part::Joined(IRON_BARS)));
    // The Ayrton Light: an open arcade, gilded corner piers and slender shafts, round a glazed
    // lamp room whose lamp is lit while Parliament sits.
    for y in GALLERY + 1..LANTERN_HEAD {
        t.ring(y, 3, 3, |_, u| match u.abs() {
            3 => Some(b(LEAF)),
            1 => Some(Part::Joined(PINNACLE)),
            _ => None,
        });
        t.ring(y, 2, 2, |_, _| Some(b(BLACK_STAINED_GLASS)));
        let lamp = y > GALLERY + 1 && y < LANTERN_HEAD - 1;
        t.set(C, y, C, b(if lamp { GLOWSTONE } else { IRON }));
    }
    // A gilded cornice, then gilded finials at its corners and middles round the spire's foot.
    t.ring(LANTERN_HEAD, 3, 3, |_, _| Some(b(LEAF)));
    t.fill(LANTERN_HEAD, 2, b(PLATE));
    t.ring(LANTERN_HEAD + 1, 3, 3, |_, u| {
        matches!(u.abs(), 0 | 3).then_some(b(LEAF))
    });
    // Corner pinnacles beside the spire's foot.
    for f in 0..4 {
        for y in GALLERY + 1..SPIRE[0].0 {
            t.at(f, 4, y, 4, Part::Joined(IRON_WALL));
        }
        t.at(f, 4, SPIRE[0].0, 4, b(LEAF));
    }
}

fn spire(t: &mut Tower) {
    for (i, &(y, h, cut)) in SPIRE.iter().enumerate() {
        let next = SPIRE.get(i + 1).map_or(0, |s| s.1);
        t.pyramid(y, h, next < h);
        if cut {
            for f in 0..4 {
                t.clear(f, h, y, h);
            }
        }
    }
    // Gilded studs up the faces.
    for f in 0..4 {
        for u in [-1, 1] {
            t.at(f, u, SPIRE[1].0, 2, b(LEAF));
        }
        t.at(f, 0, SPIRE[5].0, 1, b(LEAF));
    }
    // The needle, then the gilded orb and cross on a slender shaft.
    let top = SPIRE[SPIRE.len() - 1].0;
    for y in (top + 1..ORB).chain(ORB + 1..ORB + 3) {
        t.set(C, y, C, Part::Joined(PLATE_WALL));
    }
    t.set(C, ORB, C, b(GILT));
    t.set(C, ORB + 3, C, b(GILT));
}

/// The night floodlighting: hidden light blocks in the air just off each face, all inside the
/// model's own columns so nothing of the Palace beside it is cleared for them.
fn floodlights(t: &mut Tower) {
    let mut light = |f: usize, u: i32, y: i32, d: i32| {
        let (x, z) = Tower::cell(f, u, d);
        debug_assert!(
            t.get(x, y, z).is_none(),
            "light over a block at {x},{y},{z}"
        );
        t.set(x, y, z, Part::Light(15));
    };
    for f in 0..4 {
        for u in [-3, 3] {
            for y in (8..CORNICE).step_by(6) {
                light(f, u, y, 7);
            }
            light(f, u, BELFRY + 2, 6);
        }
        for y in [52, 58] {
            for u in [-5, 5] {
                light(f, u, y, 7);
            }
        }
        light(f, 0, ROOF[2].0, 5);
        light(f, 0, SPIRE[2].0, 3);
        light(f, 0, SPIRE[7].0, 2);
    }
}

/// Whether the part fills its cell, so walls and panes join it.
fn is_full(part: Option<Part>) -> bool {
    matches!(part, Some(Part::Block(_)))
}

/// The corner a stair takes from its neighbours, as `StairBlock::getStairsShape` decides it.
fn stair_shape(t: &Tower, (x, y, z): (i32, i32, i32), facing: Dir, top: bool) -> StairShape {
    let stair_at = |d: Dir| {
        let (dx, dz) = d.step();
        match t.get(x + dx, y, z + dz) {
            Some(Part::Stair(_, f, t)) if t == top => Some(f),
            _ => None,
        }
    };
    let across = |d: Dir| (d == Dir::N || d == Dir::S) != (facing == Dir::N || facing == Dir::S);
    let free = |side: Dir| stair_at(side) != Some(facing);
    if let Some(front) = stair_at(facing) {
        if across(front) && free(front.back()) {
            return if front == facing.counter_clockwise() {
                StairShape::OuterLeft
            } else {
                StairShape::OuterRight
            };
        }
    }
    if let Some(behind) = stair_at(facing.back()) {
        if across(behind) && free(behind) {
            return if behind == facing.counter_clockwise() {
                StairShape::InnerLeft
            } else {
                StairShape::InnerRight
            };
        }
    }
    StairShape::Straight
}

const SHAPES: [StairShape; 5] = [
    StairShape::Straight,
    StairShape::InnerLeft,
    StairShape::InnerRight,
    StairShape::OuterLeft,
    StairShape::OuterRight,
];

/// Neighbour state a part's block state depends on: a stair's corner, a wall's sides.
fn context(t: &Tower, (x, y, z): (i32, i32, i32), part: Part) -> u8 {
    match part {
        Part::Stair(_, facing, top) => {
            let shape = stair_shape(t, (x, y, z), facing, top);
            SHAPES.iter().position(|&s| s == shape).unwrap_or(0) as u8
        }
        Part::Joined(_) => Dir::ALL.iter().enumerate().fold(0, |mask, (i, d)| {
            let (dx, dz) = d.step();
            let next = t.get(x + dx, y, z + dz);
            if is_full(next) || matches!(next, Some(Part::Joined(_))) {
                mask | 1 << i
            } else {
                mask
            }
        }),
        _ => 0,
    }
}

fn resolve(part: Part, ctx: u8) -> BlockWithProperties {
    match part {
        Part::Block(block) => BlockWithProperties::simple(block),
        Part::Stair(block, facing, top) => {
            let s = create_stair_with_properties(block, facing.stair(), SHAPES[ctx as usize]);
            if top {
                top_stair(s)
            } else {
                s
            }
        }
        Part::Joined(block) => {
            let [n, e, s, w] = [0, 1, 2, 3].map(|i| ctx & 1 << i != 0);
            if block.name().ends_with("_wall") {
                connected_wall(block, n, s, e, w)
            } else {
                connected_fence(block, n, s, e, w)
            }
        }
        Part::Button(block, facing) => with_props(
            block,
            &[
                ("face", "wall"),
                ("facing", facing.name()),
                ("powered", "false"),
            ],
        ),
        Part::Light(level) => with_props(
            LIGHT,
            &[("level", &level.to_string()), ("waterlogged", "false")],
        ),
        Part::SlabTop(block) => with_props(block, &[("type", "top"), ("waterlogged", "false")]),
    }
}

fn finish(t: &Tower) -> PalettizedSchematic {
    let mut palette: Vec<BlockWithProperties> = Vec::new();
    let mut slots: BTreeMap<(Part, u8), u8> = BTreeMap::new();
    let mut voxels = Vec::with_capacity(t.cells.len());
    for (&at, &part) in &t.cells {
        let (x, y, z) = at;
        debug_assert!((0..SIDE).contains(&x) && (0..SIDE).contains(&z) && y >= 0);
        let ctx = context(t, at, part);
        let slot = *slots.entry((part, ctx)).or_insert_with(|| {
            palette.push(resolve(part, ctx));
            u8::try_from(palette.len() - 1).expect("the tower uses at most 256 block states")
        });
        voxels.push((x as i16, y as i16, z as i16, slot));
    }
    PalettizedSchematic {
        width: SIDE,
        length: SIDE,
        palette,
        voxels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn blocks(model: &PalettizedSchematic) -> HashMap<(i16, i16, i16), Block> {
        model
            .voxels
            .iter()
            .map(|&(x, y, z, slot)| ((x, y, z), model.palette[slot as usize].block))
            .collect()
    }

    #[test]
    fn every_face_is_the_same_turned_a_quarter() {
        let cells = blocks(&build());
        let c = C as i16;
        for (&(x, y, z), &block) in &cells {
            let turned = (c - (z - c), y, c + (x - c));
            assert_eq!(cells.get(&turned), Some(&block), "{x},{y},{z}");
        }
    }

    #[test]
    fn it_stands_96_metres_on_its_grid() {
        let model = build();
        let top = model.voxels.iter().map(|v| v.1).max();
        assert_eq!(top, Some(95));
        assert!(model.voxels.iter().all(|v| v.1 >= 0
            && (0..SIDE as i16).contains(&v.0)
            && (0..SIDE as i16).contains(&v.2)));
        let cells = blocks(&model);
        let south = C as i16 + 6;
        let dial = |u: i16, dy: i16, d: i16| cells.get(&(C as i16 + u, DIAL_Y as i16 + dy, d));
        assert_eq!(dial(0, 0, south), Some(&HANDS), "hub");
        assert_eq!(dial(3, 0, south), Some(&HANDS), "minute hand at the III");
        assert_eq!(
            dial(0, 3, south),
            Some(&WHITE_STAINED_GLASS),
            "hour hand short of the XII"
        );
        assert_eq!(dial(-1, 0, south), Some(&WHITE_STAINED_GLASS));
        assert_eq!(dial(-1, 0, south - 1), Some(&SEA_LANTERN));
        assert_eq!(
            dial(0, -3, south + 1),
            Some(&POLISHED_BLACKSTONE_BUTTON),
            "VI"
        );
        assert!(model.palette.len() <= 256);
        assert!(cells.values().any(|&b| b == LIGHT), "floodlit");
        assert!(cells.values().any(|&b| b == GLOWSTONE), "the Ayrton Light");
    }
}
