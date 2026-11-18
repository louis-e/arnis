//! The Brandenburger Tor at one block per metre: the gate, its two colonnades and the two
//! gatehouses on Pariser Platz, built square and then sheared onto their real 6.4 degree turn.

use std::collections::BTreeMap;

use super::schematic::PalettizedSchematic;
use crate::block_definitions::*;
use crate::element_processing::connected_blocks::{connected_fence, connected_wall};
use crate::element_processing::structure_kit::{with_prop, with_props};

/// A horizontal direction, north up.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Dir {
    N,
    E,
    S,
    W,
}

impl Dir {
    pub(super) const ALL: [Dir; 4] = [Dir::N, Dir::E, Dir::S, Dir::W];

    pub(super) fn step(self) -> (i32, i32) {
        match self {
            Dir::N => (0, -1),
            Dir::E => (1, 0),
            Dir::S => (0, 1),
            Dir::W => (-1, 0),
        }
    }

    pub(super) fn back(self) -> Dir {
        match self {
            Dir::N => Dir::S,
            Dir::E => Dir::W,
            Dir::S => Dir::N,
            Dir::W => Dir::E,
        }
    }

    fn counter_clockwise(self) -> Dir {
        match self {
            Dir::N => Dir::W,
            Dir::W => Dir::S,
            Dir::S => Dir::E,
            Dir::E => Dir::N,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Dir::N => "north",
            Dir::E => "east",
            Dir::S => "south",
            Dir::W => "west",
        }
    }

    fn stair(self) -> StairFacing {
        match self {
            Dir::N => StairFacing::North,
            Dir::E => StairFacing::East,
            Dir::S => StairFacing::South,
            Dir::W => StairFacing::West,
        }
    }
}

/// One cell of a hand-built model.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Part {
    Block(Block),
    /// Stairs with their tall back toward the direction; `true` hangs them upside down.
    /// The corner shape is worked out from the neighbours, as the game does.
    Stair(Block, Dir, bool),
    /// A slab, `true` in the upper half.
    Slab(Block, bool),
    /// Two slabs, a full block with the slab's texture.
    Double(Block),
    /// A wall or pane, joined to its neighbours when finished.
    Joined(Block),
    /// An open trapdoor standing against the side opposite the direction.
    Trapdoor(Block, Dir),
}

/// A sparse block grid, x east, y up, z south.
#[derive(Default)]
pub(super) struct Grid {
    pub(super) cells: BTreeMap<(i32, i32, i32), Part>,
}

impl Grid {
    pub(super) fn set(&mut self, x: i32, y: i32, z: i32, part: Part) {
        self.cells.insert((x, y, z), part);
    }

    pub(super) fn get(&self, x: i32, y: i32, z: i32) -> Option<Part> {
        self.cells.get(&(x, y, z)).copied()
    }

    pub(super) fn clear(&mut self, x: i32, y: i32, z: i32) {
        self.cells.remove(&(x, y, z));
    }

    /// Sets every cell of the inclusive box.
    pub(super) fn fill(&mut self, x: (i32, i32), y: (i32, i32), z: (i32, i32), part: Part) {
        for xx in x.0..=x.1 {
            for yy in y.0..=y.1 {
                for zz in z.0..=z.1 {
                    self.set(xx, yy, zz, part);
                }
            }
        }
    }

    /// Sets the cells of the box that are still empty.
    pub(super) fn fill_empty(&mut self, x: (i32, i32), y: (i32, i32), z: (i32, i32), part: Part) {
        for xx in x.0..=x.1 {
            for yy in y.0..=y.1 {
                for zz in z.0..=z.1 {
                    self.cells.entry((xx, yy, zz)).or_insert(part);
                }
            }
        }
    }
}

pub(super) fn b(block: Block) -> Part {
    Part::Block(block)
}

pub(super) fn stair(block: Block, dir: Dir) -> Part {
    Part::Stair(block, dir, false)
}

pub(super) fn hung(block: Block, dir: Dir) -> Part {
    Part::Stair(block, dir, true)
}

/// Whether the part fills its cell, so walls and panes join it.
fn is_full(part: Option<Part>) -> bool {
    matches!(part, Some(Part::Block(_) | Part::Double(_)))
}

fn joins_to(part: Option<Part>) -> bool {
    is_full(part) || matches!(part, Some(Part::Joined(_)))
}

/// The corner a stair takes from its neighbours, as `StairBlock::getStairsShape` decides it.
fn stair_shape(grid: &Grid, x: i32, y: i32, z: i32, facing: Dir, top: bool) -> StairShape {
    let stair_at = |d: Dir| {
        let (dx, dz) = d.step();
        match grid.get(x + dx, y, z + dz) {
            Some(Part::Stair(_, f, t)) if t == top => Some(f),
            _ => None,
        }
    };
    // A neighbour on `side` that is the same stair facing the same way blocks the corner.
    let free = |side: Dir| stair_at(side) != Some(facing);
    if let Some(front) = stair_at(facing) {
        if (front == Dir::N || front == Dir::S) != (facing == Dir::N || facing == Dir::S)
            && free(front.back())
        {
            return if front == facing.counter_clockwise() {
                StairShape::OuterLeft
            } else {
                StairShape::OuterRight
            };
        }
    }
    if let Some(behind) = stair_at(facing.back()) {
        if (behind == Dir::N || behind == Dir::S) != (facing == Dir::N || facing == Dir::S)
            && free(behind)
        {
            return if behind == facing.counter_clockwise() {
                StairShape::InnerLeft
            } else {
                StairShape::InnerRight
            };
        }
    }
    StairShape::Straight
}

fn shape_code(shape: StairShape) -> u8 {
    match shape {
        StairShape::Straight => 0,
        StairShape::InnerLeft => 1,
        StairShape::InnerRight => 2,
        StairShape::OuterLeft => 3,
        StairShape::OuterRight => 4,
    }
}

fn shape_of(code: u8) -> StairShape {
    match code {
        1 => StairShape::InnerLeft,
        2 => StairShape::InnerRight,
        3 => StairShape::OuterLeft,
        4 => StairShape::OuterRight,
        _ => StairShape::Straight,
    }
}

/// Neighbour state a part's block state depends on: a stair's corner, a wall's sides.
fn context(grid: &Grid, x: i32, y: i32, z: i32, part: Part) -> u8 {
    match part {
        Part::Stair(_, facing, top) => shape_code(stair_shape(grid, x, y, z, facing, top)),
        Part::Joined(_) => Dir::ALL.iter().enumerate().fold(0, |mask, (i, d)| {
            let (dx, dz) = d.step();
            if joins_to(grid.get(x + dx, y, z + dz)) {
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
            let s = create_stair_with_properties(block, facing.stair(), shape_of(ctx));
            if top {
                top_stair(s)
            } else {
                s
            }
        }
        Part::Slab(block, top) => with_prop(block, "type", if top { "top" } else { "bottom" }),
        Part::Double(block) => with_prop(block, "type", "double"),
        Part::Joined(block) => {
            let [n, e, s, w] = [0, 1, 2, 3].map(|i| ctx & 1 << i != 0);
            if block.name().ends_with("_wall") {
                connected_wall(block, n, s, e, w)
            } else {
                connected_fence(block, n, s, e, w)
            }
        }
        Part::Trapdoor(block, facing) => with_props(
            block,
            &[
                ("facing", facing.name()),
                ("half", "bottom"),
                ("open", "true"),
            ],
        ),
    }
}

/// Lays the grid out as a schematic. Every cell must already sit at x, y, z >= 0.
pub(super) fn finish(grid: &Grid) -> PalettizedSchematic {
    let mut palette: Vec<BlockWithProperties> = Vec::new();
    let mut slots: BTreeMap<(Part, u8), u8> = BTreeMap::new();
    let mut voxels = Vec::with_capacity(grid.cells.len());
    let (mut width, mut length) = (0, 0);
    for (&(x, y, z), &part) in &grid.cells {
        assert!(
            x >= 0 && y >= 0 && z >= 0,
            "cell {x},{y},{z} outside the model"
        );
        let ctx = context(grid, x, y, z, part);
        let slot = *slots.entry((part, ctx)).or_insert_with(|| {
            palette.push(resolve(part, ctx));
            u8::try_from(palette.len() - 1).expect("a landmark uses at most 256 block states")
        });
        voxels.push((x as i16, y as i16, z as i16, slot));
        width = width.max(x + 1);
        length = length.max(z + 1);
    }
    PalettizedSchematic {
        width,
        length,
        palette,
        voxels,
    }
}

// The gate, designed square in its own frame: u runs south along the gate, v east out of
// the Pariser Platz face, both centred on the middle of the column rows.

const STONE: Block = SMOOTH_SANDSTONE;
const DRUMS: Block = SANDSTONE;
const MOULDING: Block = SMOOTH_SANDSTONE_STAIRS;
const LIP: Block = CUT_SANDSTONE_SLAB;
const COPPER: Block = WAXED_OXIDIZED_COPPER;
const COPPER_STAIRS: Block = WAXED_OXIDIZED_CUT_COPPER_STAIRS;

/// Gate-frame u of the cell rows where the frame's x shifts one block east, as a turn of
/// 6.4 degrees does every 9 m; placed in the passages and between columns.
const U_STEPS: [i32; 4] = [5, 14, 23, 33];
/// Gate-frame v where the rows shift one block north, inside the gatehouses.
const V_STEPS: [i32; 2] = [10, 19];
/// Model cell of the gate frame's origin.
pub const GATE_X: i32 = 12;
pub const GATE_Z: i32 = 36;

/// Lower u of each 2 m column pair of the gate, north to south.
const GATE_COLUMNS: [i32; 6] = [-16, -10, -4, 3, 9, 15];
/// Layers of the gate: column tops, architrave, frieze, cornice, attic top.
const CAPITAL: i32 = 12;
const ARCHITRAVE: i32 = 13;
const FRIEZE: i32 = 14;
const CORNICE: i32 = 15;
const ATTIC_TOP: i32 = 19;

fn steps_before(steps: &[i32], n: i32) -> i32 {
    let count = |n: i32| steps.iter().filter(|&&s| n >= s).count() as i32;
    if n >= 0 {
        count(n)
    } else {
        -count(-n)
    }
}

/// Model cell of a gate-frame cell: a shear along v, then one along the model rows.
pub(super) fn to_model(u: i32, v: i32) -> (i32, i32) {
    let z = u - steps_before(&V_STEPS, v);
    (v + steps_before(&U_STEPS, z) + GATE_X, z + GATE_Z)
}

/// Gate-frame builder; `at(v, y, u)` keeps the frame's v as x and u as z.
#[derive(Default)]
struct Frame {
    grid: Grid,
}

impl Frame {
    fn at(&mut self, v: i32, y: i32, u: i32, part: Part) {
        self.grid.set(v, y, u, part);
    }

    fn fill(&mut self, v: (i32, i32), y: (i32, i32), u: (i32, i32), part: Part) {
        self.grid.fill(v, y, u, part);
    }

    fn fill_empty(&mut self, v: (i32, i32), y: (i32, i32), u: (i32, i32), part: Part) {
        self.grid.fill_empty(v, y, u, part);
    }

    /// The ring of cells one out from the box at layer `y`, facing out, as `part(dir)`.
    fn ring(&mut self, v: (i32, i32), u: (i32, i32), y: i32, part: impl Fn(Dir) -> Part) {
        self.ring_open(v, u, y, &[], part);
    }

    /// `ring` without the sides in `open`.
    fn ring_open(
        &mut self,
        v: (i32, i32),
        u: (i32, i32),
        y: i32,
        open: &[Dir],
        part: impl Fn(Dir) -> Part,
    ) {
        let shut = |d: Dir| !open.contains(&d);
        let (v0, v1) = (v.0 - i32::from(shut(Dir::W)), v.1 + i32::from(shut(Dir::E)));
        let (u0, u1) = (u.0 - i32::from(shut(Dir::N)), u.1 + i32::from(shut(Dir::S)));
        for vv in v0..=v1 {
            if shut(Dir::N) {
                self.at(vv, y, u.0 - 1, part(Dir::N));
            }
            if shut(Dir::S) {
                self.at(vv, y, u.1 + 1, part(Dir::S));
            }
        }
        for uu in u0..=u1 {
            if shut(Dir::W) {
                self.at(v.0 - 1, y, uu, part(Dir::W));
            }
            if shut(Dir::E) {
                self.at(v.1 + 1, y, uu, part(Dir::E));
            }
        }
    }
}

pub fn build() -> PalettizedSchematic {
    let mut f = Frame::default();
    gate(&mut f);
    attic(&mut f);
    quadriga(&mut f);
    for side in [-1, 1] {
        colonnade(&mut f, side);
        gatehouse(&mut f, side);
    }
    let mut grid = Grid::default();
    for (&(v, y, u), &part) in &f.grid.cells {
        let (x, z) = to_model(u, v);
        grid.set(x, y, z, part);
    }
    finish(&grid)
}

/// A round shaft on a plinth, `w` blocks square, banded drums under a smooth capital.
fn column(f: &mut Frame, v0: i32, u0: i32, w: i32, top: i32) {
    let (v, u) = ((v0, v0 + w - 1), (u0, u0 + w - 1));
    f.fill(v, (0, 0), u, b(STONE));
    f.fill(v, (1, top - 1), u, b(DRUMS));
    f.fill(v, (top, top), u, b(STONE));
}

fn gate(f: &mut Frame) {
    for &u in &GATE_COLUMNS {
        for v in [-5, 4] {
            column(f, v, u, 2, CAPITAL);
        }
        // The walls between the passages, and the end walls.
        f.fill((-3, 3), (0, CAPITAL), (u, u + 1), b(STONE));
        // Relief panels on the passage sides, a band at the impost.
        for (face, outside) in [(u, u - 1), (u + 1, u + 2)] {
            if outside.abs() > 16 {
                continue;
            }
            f.fill((-2, 2), (4, 6), (face, face), b(DRUMS));
            f.fill((-1, 1), (9, 9), (face, face), b(DRUMS));
        }
        f.fill((-3, 3), (0, 0), (u, u + 1), b(DRUMS));
    }
    // Plinths stepping out round each column foot.
    for &u in &GATE_COLUMNS {
        for v in [-5, 4] {
            for (dv, du) in [
                (-1, 0),
                (-1, 1),
                (2, 0),
                (2, 1),
                (0, -1),
                (1, -1),
                (0, 2),
                (1, 2),
            ] {
                f.fill_empty(
                    (v + dv, v + dv),
                    (0, 0),
                    (u + du, u + du),
                    Part::Slab(LIP, false),
                );
            }
        }
    }
    // Entablature over the whole gate: plain architrave, triglyphs and metopes, cornice.
    f.fill((-5, 5), (ARCHITRAVE, CORNICE), (-16, 16), b(STONE));
    for u in -16..=16 {
        let glyph = if u % 2 == 0 { DRUMS } else { STONE };
        f.at(-5, FRIEZE, u, b(glyph));
        f.at(5, FRIEZE, u, b(glyph));
    }
    for v in -5..=5 {
        let glyph = if v % 2 == 0 { DRUMS } else { STONE };
        f.at(v, FRIEZE, -16, b(glyph));
        f.at(v, FRIEZE, 16, b(glyph));
    }
    f.ring((-5, 5), (-16, 16), CORNICE, |d| hung(MOULDING, d.back()));
    f.ring((-5, 5), (-16, 16), CORNICE + 1, |_| Part::Slab(LIP, false));
}

/// The stepped attic, its raised middle with the relief panels and the quadriga's plinth.
fn attic(f: &mut Frame) {
    let base = CORNICE + 1;
    for side in [-1, 1] {
        let us = if side < 0 { (-16, -5) } else { (5, 16) };
        // Three steps up from the cornice to the attic band.
        for (i, v) in [5, 4, 3].into_iter().enumerate() {
            let y = base + i as i32;
            if y > base {
                f.fill((-v, -v), (base, y - 1), us, b(STONE));
                f.fill((v, v), (base, y - 1), us, b(STONE));
            }
            f.fill((-v, -v), (y, y), us, stair(MOULDING, Dir::E));
            f.fill((v, v), (y, y), us, stair(MOULDING, Dir::W));
        }
        f.fill((-2, 2), (base, ATTIC_TOP - 1), us, b(STONE));
        f.fill((-2, 2), (ATTIC_TOP, ATTIC_TOP), us, Part::Slab(LIP, false));
    }
    // The raised middle over the wide passage.
    f.fill((-5, 5), (base, ATTIC_TOP), (-4, 4), b(STONE));
    for v in [-5, 5] {
        f.fill((v, v), (base + 1, ATTIC_TOP - 1), (-3, 3), b(DRUMS));
    }
    f.ring((-5, 5), (-4, 4), ATTIC_TOP, |d| hung(MOULDING, d.back()));
    f.ring((-5, 5), (-4, 4), ATTIC_TOP + 1, |_| Part::Slab(LIP, false));
    f.fill((-3, 3), (ATTIC_TOP + 1, ATTIC_TOP + 1), (-3, 3), b(STONE));
}

/// Victoria in her chariot behind four horses abreast, driving east toward the city.
fn quadriga(f: &mut Frame) {
    let ground = ATTIC_TOP + 2;
    for u in [-3, -1, 1, 3] {
        // Hind leg set back, front leg reaching out, the body, neck, head and ears.
        f.at(-1, ground, u, hung(COPPER_STAIRS, Dir::W));
        f.at(1, ground, u, hung(COPPER_STAIRS, Dir::E));
        f.fill((-1, 1), (ground + 1, ground + 1), (u, u), b(COPPER));
        f.at(1, ground + 2, u, b(COPPER));
        f.at(2, ground + 2, u, hung(COPPER_STAIRS, Dir::W));
        f.at(1, ground + 3, u, stair(COPPER_STAIRS, Dir::E));
    }
    // The chariot with its breastwork and a wheel either side.
    f.fill((-3, -2), (ground, ground), (-1, 1), b(COPPER));
    f.fill(
        (-2, -2),
        (ground + 1, ground + 1),
        (-1, 1),
        stair(COPPER_STAIRS, Dir::E),
    );
    for u in [-2, 2] {
        f.at(-3, ground + 1, u, stair(COPPER_STAIRS, Dir::E));
        f.at(-2, ground + 1, u, stair(COPPER_STAIRS, Dir::W));
        f.at(-3, ground, u, hung(COPPER_STAIRS, Dir::E));
        f.at(-2, ground, u, hung(COPPER_STAIRS, Dir::W));
    }
    // Victoria with raised wings; in her right hand the standard, wreath and eagle on top.
    f.fill((-3, -3), (ground + 1, ground + 3), (0, 0), b(COPPER));
    f.at(-3, ground + 2, -1, b(COPPER));
    f.at(-3, ground + 2, 1, b(COPPER));
    f.at(-3, ground + 3, -1, stair(COPPER_STAIRS, Dir::N));
    f.at(-3, ground + 3, 1, stair(COPPER_STAIRS, Dir::S));
    f.at(-2, ground + 2, 2, Part::Joined(IRON_BARS));
    f.at(-2, ground + 3, 2, b(COPPER));
    f.at(-2, ground + 4, 2, stair(COPPER_STAIRS, Dir::W));
    f.at(-2, ground + 4, 1, hung(COPPER_STAIRS, Dir::S));
    f.at(-2, ground + 4, 3, hung(COPPER_STAIRS, Dir::N));
}

/// Strack's open hall of Doric columns, from the gate's end wall out to its pediment.
fn colonnade(f: &mut Frame, side: i32) {
    let span = |a: i32, b: i32| (a.min(b), a.max(b));
    let hall = span(17 * side, 32 * side);
    // The wall it shares with the gate.
    f.fill((-4, 4), (0, 9), (17 * side, 17 * side), b(STONE));
    for u in [18, 21, 25, 29, 32] {
        for v in [-5, 5] {
            column(f, v, u * side, 1, 6);
        }
    }
    for v in [-2, 2] {
        for u in [21, 29, 32] {
            column(f, v, u * side, 1, 6);
        }
    }
    let toward_gate = if side < 0 { Dir::S } else { Dir::N };
    entablature(f, (-5, 5), hall, &[toward_gate]);
    // The ceiling under the roof, and the gable along the hall.
    f.fill((-4, 4), (7, 9), hall, b(STONE));
    roof_along_u(f, (-6, 6), hall, 33 * side);
}

/// Architrave, triglyph frieze and cornice round the box, the cornice one block out,
/// left off the `open` sides where the box meets another.
fn entablature(f: &mut Frame, v: (i32, i32), u: (i32, i32), open: &[Dir]) {
    for vv in v.0..=v.1 {
        for uu in u.0..=u.1 {
            let edge = vv == v.0 || vv == v.1 || uu == u.0 || uu == u.1;
            if !edge {
                continue;
            }
            f.at(vv, 7, uu, b(STONE));
            let glyph = if (vv + uu) % 2 == 0 { DRUMS } else { STONE };
            f.at(vv, 8, uu, b(glyph));
            f.at(vv, 9, uu, b(STONE));
        }
    }
    f.ring_open(v, u, 9, open, |d| hung(MOULDING, d.back()));
}

/// A low copper gable over `v` with its ridge along u, the pediment at row `end`.
fn roof_along_u(f: &mut Frame, v: (i32, i32), u: (i32, i32), end: i32) {
    for uu in u.0..=u.1 {
        f.at(v.0, 10, uu, stair(COPPER_STAIRS, Dir::E));
        f.at(v.1, 10, uu, stair(COPPER_STAIRS, Dir::W));
        f.fill((v.0 + 1, v.1 - 1), (10, 10), (uu, uu), b(COPPER));
        f.at(-1, 11, uu, stair(COPPER_STAIRS, Dir::E));
        f.at(1, 11, uu, stair(COPPER_STAIRS, Dir::W));
        f.at(0, 11, uu, b(COPPER));
    }
    pediment_u(f, v, end);
}

/// A sandstone pediment across v at row `u`, raking cornice in stairs.
fn pediment_u(f: &mut Frame, v: (i32, i32), u: i32) {
    f.at(v.0, 10, u, stair(MOULDING, Dir::E));
    f.at(v.1, 10, u, stair(MOULDING, Dir::W));
    f.fill((v.0 + 1, v.1 - 1), (10, 10), (u, u), b(STONE));
    f.at(-1, 11, u, stair(MOULDING, Dir::E));
    f.at(1, 11, u, stair(MOULDING, Dir::W));
    f.at(0, 11, u, b(STONE));
}

/// Langhans' guardhouse on Pariser Platz: a small Doric temple facing east.
fn gatehouse(f: &mut Frame, side: i32) {
    // Its north and south column rows, the cella between them.
    let rows = if side < 0 { (-30, -21) } else { (21, 30) };
    // The cella a step inside its colonnade.
    let cella = (rows.0 + 2, rows.1 - 2);
    f.fill((8, 20), (0, 6), cella, Part::Double(LIP));
    f.fill((8, 20), (0, 0), cella, b(DRUMS));
    for v in [8, 12, 15, 19, 22] {
        column(f, v, rows.0, 1, 6);
        column(f, v, rows.1, 1, 6);
    }
    for u in [rows.0 + 3, rows.0 + 6] {
        column(f, 22, u, 1, 6);
    }
    // A door under a window in the east wall.
    let mid = (rows.0 + rows.1).div_euclid(2);
    for u in [mid, mid + 1] {
        f.fill(
            (20, 20),
            (1, 3),
            (u, u),
            Part::Joined(GRAY_STAINED_GLASS_PANE),
        );
        f.fill(
            (20, 20),
            (5, 5),
            (u, u),
            Part::Joined(GRAY_STAINED_GLASS_PANE),
        );
    }
    f.fill((20, 20), (4, 4), (mid - 1, mid + 2), b(STONE));
    // Entablature on the columns, ceiling, then the gable along v with the pediment east.
    entablature(f, (6, 22), rows, &[Dir::W]);
    f.fill((7, 21), (7, 9), (rows.0 + 1, rows.1 - 1), b(STONE));
    let (n, s) = (rows.0 - 1, rows.1 + 1);
    for v in 6..=22 {
        f.at(v, 10, n, stair(COPPER_STAIRS, Dir::S));
        f.at(v, 10, s, stair(COPPER_STAIRS, Dir::N));
        f.fill((v, v), (10, 10), (n + 1, s - 1), b(COPPER));
        f.at(v, 11, mid, stair(COPPER_STAIRS, Dir::S));
        f.at(v, 11, mid + 1, stair(COPPER_STAIRS, Dir::N));
    }
    f.at(23, 10, n, stair(MOULDING, Dir::S));
    f.at(23, 10, s, stair(MOULDING, Dir::N));
    f.fill((23, 23), (10, 10), (n + 1, s - 1), b(STONE));
    f.at(23, 11, mid, stair(MOULDING, Dir::S));
    f.at(23, 11, mid + 1, stair(MOULDING, Dir::N));
    f.at(23, 10, mid, b(DRUMS));
    f.at(23, 10, mid + 1, b(DRUMS));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_shear_moves_every_cell_to_its_own() {
        let mut seen = HashSet::new();
        for u in -40..=40 {
            for v in -12..=30 {
                assert!(seen.insert(to_model(u, v)), "{u},{v} lands on a taken cell");
            }
        }
        // Within a block of the 6.4 degree turn it stands for.
        let (sin, cos) = 6.4f64.to_radians().sin_cos();
        for (u, v) in [(-33, -6), (33, 6), (-30, 22), (30, 22), (0, 0)] {
            let (x, z) = to_model(u, v);
            let (fu, fv) = (f64::from(u), f64::from(v));
            let (tx, tz) = (fu * sin + fv * cos, fu * cos - fv * sin);
            assert!((f64::from(x - GATE_X) - tx).abs() <= 1.2, "{u},{v} x");
            assert!((f64::from(z - GATE_Z) - tz).abs() <= 1.2, "{u},{v} z");
        }
    }

    #[test]
    fn it_stands_26_metres_on_the_ground() {
        let model = build();
        let top = model.voxels.iter().map(|v| v.1).max();
        assert_eq!(top, Some(25));
        assert!(model.voxels.iter().all(|v| v.1 >= 0));
        assert!(model.palette.iter().all(|p| p.block.id() < 256));
    }

    #[test]
    fn the_five_passages_stay_open_under_the_architrave() {
        let model = build();
        let cells: HashSet<(i16, i16, i16)> =
            model.voxels.iter().map(|v| (v.0, v.1, v.2)).collect();
        for u in [-13, -7, 0, 6, 12] {
            for v in -3..=3 {
                let (x, z) = to_model(u, v);
                for y in 0..ARCHITRAVE {
                    assert!(
                        !cells.contains(&(x as i16, y as i16, z as i16)),
                        "{u},{y},{v}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_quadriga_drives_east() {
        let model = build();
        let xs: Vec<i32> = model
            .voxels
            .iter()
            .filter(|v| i32::from(v.1) > ATTIC_TOP + 1)
            .filter(|v| [COPPER, COPPER_STAIRS].contains(&model.palette[v.3 as usize].block))
            .map(|v| i32::from(v.0))
            .collect();
        // The horses' heads lead on the east, Victoria's chariot trails on the west.
        assert_eq!(xs.iter().max(), Some(&to_model(0, 2).0));
        assert_eq!(xs.iter().min(), Some(&to_model(0, -3).0));
    }
}
