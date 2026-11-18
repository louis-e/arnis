//! The Elbphilharmonie at one block per metre: Kaispeicher A, the Plaza and the glass hull.

use std::collections::BTreeMap;

use super::schematic::PalettizedSchematic;
use crate::block_definitions::*;
use crate::element_processing::structure_kit::with_props;

/// Layers of quay wall under the ground floor.
const FOOTING: i32 = 3;
/// Model layer at ground level, the quay at 8.3 m above sea level.
pub const GROUND: i32 = FOOTING;
/// Model block of the plan origin at 53.541417 N, 9.984194 E. Plan x runs east, z south, in metres.
pub const ORIGIN_X: i32 = 66;
pub const ORIGIN_Z: i32 = 26;
/// Plan cells the model may cover.
const MIN_X: i32 = -64;
const MAX_X: i32 = 53;
const MIN_Z: i32 = -24;
const MAX_Z: i32 = 61;

/// The Plaza walkway on the warehouse, 37 m above sea level.
const PLAZA: i32 = 29;
/// Underside of the glass superstructure.
const SOFFIT: i32 = 32;
/// Glass panel rows and bays.
const ROW: i32 = 4;
const BAY: f64 = 5.0;
/// Ring of the Plaza's inner glazing, counted in from the facade.
const LOBBY: i32 = 4;

/// Corners of the mapped outline: NW, NE, SE, SW. Facade `i` runs from corner `i` to the next.
const CORNERS: [(f64, f64); 4] = [
    (-62.88, -14.40),
    (44.51, -22.58),
    (51.16, 59.71),
    (-61.50, 6.49),
];
const NORTH: usize = 0;
const EAST: usize = 1;
const SOUTH: usize = 2;
const WEST: usize = 3;

/// The roof is the lowest bowl `a x² + b z² + c xz + d x + e z + f`, fitted to Hamburg's 2022 DOM.
const BOWLS: [[f64; 6]; 7] = [
    [0.021803, 0.028093, -0.002202, 0.10427, 0.80580, 79.698],
    [0.017788, 0.009289, -0.003932, -0.75385, -0.15335, 79.747],
    [0.044401, 0.021328, -0.039176, 1.35342, -1.51366, 99.528],
    [0.017215, 0.008196, 0.000829, -1.50485, -0.82597, 122.866],
    [0.009142, 0.015563, 0.020499, 0.23841, 0.50322, 83.831],
    [0.017611, 0.018803, -0.021670, -1.66509, 1.18448, 110.702],
    [0.028150, 0.045806, 0.042111, 3.29024, 3.39178, 190.406],
];

/// The warehouse front of one facade.
struct Warehouse {
    /// Centres of the recessed stair slots, in metres from the facade's first corner.
    slots: &'static [f64],
    /// Layers of the window rows.
    rows: &'static [i32],
    /// Spacing of the window columns.
    every: f64,
}

const WAREHOUSE: [Warehouse; 4] = [
    Warehouse {
        slots: &[31.7, 60.7, 92.7],
        rows: &[6, 10, 14, 18, 22, 26],
        every: 5.0,
    },
    Warehouse {
        slots: &[],
        rows: &[13, 17, 21],
        every: 5.0,
    },
    Warehouse {
        slots: &[19.6, 50.6, 80.6, 110.6],
        rows: &[6, 10, 14, 18, 22, 26],
        every: 5.0,
    },
    Warehouse {
        slots: &[],
        rows: &[6, 10, 14, 18, 22, 26],
        every: 5.0,
    },
];

/// Vaulted openings over the Plaza: facade, centre and half width along it, height above the soffit.
const ARCHES: [(usize, f64, f64, f64); 2] = [(NORTH, 73.7, 5.5, 15.0), (SOUTH, 62.0, 8.0, 7.0)];

/// Foyer loggias away from the apartments: facade, metres along it, panel row.
const FOYER_LOGGIAS: [(usize, f64, i32); 4] = [
    (NORTH, 73.7, 6),
    (SOUTH, 57.6, 2),
    (SOUTH, 47.6, 3),
    (SOUTH, 45.6, 5),
];

/// An opening cut down into the roof.
struct Well {
    /// First open layer, over the floor.
    floor: i32,
    outline: &'static [(f64, f64)],
    bottom: Block,
}

const WELLS: [Well; 3] = [
    // The apartments' roof terrace.
    Well {
        floor: 73,
        outline: &[
            (-26.5, -4.5),
            (-23.5, -4.5),
            (-20.8, 9.8),
            (-26.0, 9.8),
            (-28.2, -1.5),
        ],
        bottom: SMOOTH_STONE,
    },
    // The hotel's atrium, glazed over at two depths.
    Well {
        floor: 55,
        outline: &[(24.5, 0.5), (31.5, 0.5), (31.5, 17.0), (24.5, 17.0)],
        bottom: GRAY_STAINED_GLASS,
    },
    Well {
        floor: 64,
        outline: &[(25.0, 17.0), (32.0, 17.0), (32.0, 28.0), (25.0, 28.0)],
        bottom: GRAY_STAINED_GLASS,
    },
];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Block(Block),
    /// A glass pane, joined north, east, south and west by bits 1, 2, 4 and 8.
    Pane(u8),
}

fn b(block: Block) -> Part {
    Part::Block(block)
}

/// Height of the roof over a plan point, in metres above the quay.
fn roof_height(x: f64, z: f64) -> f64 {
    BOWLS
        .iter()
        .map(|[a, b, c, d, e, f]| a * x * x + b * z * z + c * x * z + d * x + e * z + f)
        .fold(f64::MAX, f64::min)
}

/// Even-odd point in polygon.
fn inside(poly: &[(f64, f64)], x: f64, z: f64) -> bool {
    let mut odd = false;
    let mut prev = poly[poly.len() - 1];
    for &(x1, z1) in poly {
        let (x2, z2) = prev;
        if (z1 > z) != (z2 > z) && x < x1 + (z - z1) * (x2 - x1) / (z2 - z1) {
            odd = !odd;
        }
        prev = (x1, z1);
    }
    odd
}

fn edge_len(edge: usize) -> f64 {
    let (a, c) = (CORNERS[edge], CORNERS[(edge + 1) % 4]);
    (c.0 - a.0).hypot(c.1 - a.1)
}

/// The facade nearest a plan point and how far along it from its first corner.
fn nearest_face(x: f64, z: f64) -> (usize, f64) {
    let mut best = (f64::MAX, 0, 0.0);
    for edge in 0..4 {
        let (a, c) = (CORNERS[edge], CORNERS[(edge + 1) % 4]);
        let (dx, dz) = (c.0 - a.0, c.1 - a.1);
        let len = dx.hypot(dz);
        let s = (((x - a.0) * dx + (z - a.1) * dz) / len).clamp(0.0, len);
        let off = (x - a.0 - dx * s / len).hypot(z - a.1 - dz * s / len);
        if off < best.0 {
            best = (off, edge, s);
        }
    }
    (best.1, best.2)
}

/// The footprint on the block grid.
struct Plan {
    /// Rings in from the outside, -1 off the footprint.
    depth: Vec<i32>,
    /// Facade and distance along it.
    face: Vec<(usize, f64)>,
    /// Roof height in half metres.
    roof: Vec<i32>,
}

impl Plan {
    fn new() -> Plan {
        let cells = ((MAX_X - MIN_X) * (MAX_Z - MIN_Z)) as usize;
        let mut plan = Plan {
            depth: vec![-1; cells],
            face: vec![(0, 0.0); cells],
            roof: vec![0; cells],
        };
        let mut ring = Vec::new();
        for z in MIN_Z..MAX_Z {
            for x in MIN_X..MAX_X {
                let (cx, cz) = (f64::from(x) + 0.5, f64::from(z) + 0.5);
                if inside(&CORNERS, cx, cz) {
                    let i = plan.slot(x, z).unwrap();
                    plan.depth[i] = i32::MAX;
                    plan.face[i] = nearest_face(cx, cz);
                    plan.roof[i] = (roof_height(cx, cz) * 2.0).round() as i32;
                }
            }
        }
        for z in MIN_Z..MAX_Z {
            for x in MIN_X..MAX_X {
                let edge = plan.depth(x, z).is_some()
                    && around(x, z).any(|(nx, nz)| plan.depth(nx, nz).is_none());
                if let Some(i) = plan.slot(x, z).filter(|_| edge) {
                    plan.depth[i] = 0;
                    ring.push((x, z));
                }
            }
        }
        let mut d = 0;
        while !ring.is_empty() {
            let mut next = Vec::new();
            for (x, z) in ring {
                for (nx, nz) in around(x, z) {
                    if plan.depth(nx, nz) == Some(i32::MAX) {
                        let i = plan.slot(nx, nz).unwrap();
                        plan.depth[i] = d + 1;
                        next.push((nx, nz));
                    }
                }
            }
            ring = next;
            d += 1;
        }
        plan
    }

    fn slot(&self, x: i32, z: i32) -> Option<usize> {
        ((MIN_X..MAX_X).contains(&x) && (MIN_Z..MAX_Z).contains(&z))
            .then(|| ((x - MIN_X) + (z - MIN_Z) * (MAX_X - MIN_X)) as usize)
    }

    fn depth(&self, x: i32, z: i32) -> Option<i32> {
        self.slot(x, z).map(|i| self.depth[i]).filter(|&d| d >= 0)
    }

    fn face(&self, x: i32, z: i32) -> (usize, f64) {
        self.face[self.slot(x, z).unwrap()]
    }

    /// Topmost full roof layer, and whether a slab lies on it.
    fn top(&self, x: i32, z: i32) -> (i32, bool) {
        let half = self.roof[self.slot(x, z).unwrap()];
        (half / 2 - 1, half % 2 == 1)
    }

    /// Lowest layer of the roof cover, low enough to close the step to every lower neighbour.
    fn cover(&self, x: i32, z: i32) -> i32 {
        let top = self.top(x, z).0;
        around(x, z)
            .filter(|&(nx, nz)| self.depth(nx, nz).is_some())
            .map(|(nx, nz)| self.top(nx, nz).0 + 1)
            .fold(top, i32::min)
    }

    /// Whether a cell touches the outside, corners included, so a ring of them closes.
    fn rim(&self, x: i32, z: i32) -> bool {
        (-1..=1).any(|dx| (-1..=1).any(|dz| self.depth(x + dx, z + dz).is_none()))
    }

    /// Footprint cells with their depth.
    fn cells(&self) -> impl Iterator<Item = (i32, i32, i32)> + '_ {
        (MIN_Z..MAX_Z).flat_map(move |z| {
            (MIN_X..MAX_X).filter_map(move |x| self.depth(x, z).map(|d| (x, z, d)))
        })
    }
}

fn around(x: i32, z: i32) -> impl Iterator<Item = (i32, i32)> {
    [(0, -1), (1, 0), (0, 1), (-1, 0)]
        .into_iter()
        .map(move |(dx, dz)| (x + dx, z + dz))
}

#[derive(Default)]
struct Model {
    /// Plan x, layer above the ground, plan z.
    cells: BTreeMap<(i32, i32, i32), Part>,
}

impl Model {
    fn set(&mut self, x: i32, y: i32, z: i32, part: Part) {
        self.cells.insert((x, y, z), part);
    }

    fn clear(&mut self, x: i32, y: i32, z: i32) {
        self.cells.remove(&(x, y, z));
    }
}

pub fn build() -> PalettizedSchematic {
    let plan = Plan::new();
    let mut m = Model::default();
    warehouse(&mut m, &plan);
    plaza(&mut m, &plan);
    superstructure(&mut m, &plan);
    loggias(&mut m, &plan);
    arches(&mut m, &plan);
    wells(&mut m, &plan);
    finish(m)
}

/// Window columns of a facade, centred in the brick between corners and slots.
fn window_columns(edge: usize) -> Vec<f64> {
    let front = &WAREHOUSE[edge];
    let mut bounds = vec![0.0];
    for &c in front.slots {
        bounds.extend([c - 1.5, c + 1.5]);
    }
    bounds.push(edge_len(edge));
    let mut out = Vec::new();
    for pair in bounds.chunks(2) {
        let (a, c) = (pair[0] + 2.0, pair[1] - 2.0);
        if c < a {
            continue;
        }
        let n = ((c - a) / front.every).floor() as i32 + 1;
        let start = (a + c) / 2.0 - f64::from(n - 1) * front.every / 2.0;
        out.extend((0..n).map(|i| start + f64::from(i) * front.every));
    }
    out
}

/// The warehouse wall `d` in from the facade, `s` along it: brick, or the openings in it.
fn brick_wall(edge: usize, s: f64, y: i32, d: i32, windows: &[f64]) -> Option<Part> {
    let front = &WAREHOUSE[edge];
    let outer = d == 0;
    if front.slots.iter().any(|c| (s - c).abs() < 1.5) {
        // Recessed stair slots over flush doors, a ledge across them every other storey.
        return if y < 3 {
            Some(b(if outer {
                BLACK_STAINED_GLASS
            } else {
                GRAY_CONCRETE
            }))
        } else if outer {
            (y % 8 == 0 && y > 0).then(|| b(GRAY_CONCRETE))
        } else if y % 4 == 0 {
            Some(b(GRAY_CONCRETE))
        } else {
            Some(b(BLACK_STAINED_GLASS))
        };
    }
    if edge == EAST && (4.0..50.0).contains(&s) && y <= 4 {
        // The entrance glazed between brick piers under a dark canopy, the lobby behind.
        return match (y, outer) {
            (4, true) => Some(b(GRAY_CONCRETE)),
            (_, true) if s.rem_euclid(BAY) < 1.0 => Some(b(BRICK)),
            (_, true) => Some(b(GRAY_STAINED_GLASS)),
            (4, false) => Some(b(BRICK)),
            _ => None,
        };
    }
    if edge == NORTH && y <= 1 {
        let window = y == 1 && outer && (2.0..3.0).contains(&s.rem_euclid(BAY));
        return Some(b(if window {
            BLACK_STAINED_GLASS
        } else {
            CYAN_TERRACOTTA
        }));
    }
    let big = edge == WEST && (s - 11.9).abs() < 3.5 && (18..=23).contains(&y);
    let strip = edge == EAST
        && (5..=6).contains(&y)
        && (22.0..66.0).contains(&s)
        && (s - 22.0).rem_euclid(2.5) < 1.0;
    // A frieze of slit windows under the Plaza on the east front.
    let frieze = edge == EAST && y == 25 && (2.0..edge_len(EAST) - 2.0).contains(&s);
    let window = (frieze && s.rem_euclid(2.0) < 1.0)
        || (front.rows.contains(&y) && windows.iter().any(|c| (s - c).abs() < 0.5));
    if big || strip || window {
        return if outer {
            None
        } else if big && ((s - 11.9).abs() < 0.5 || y == 21) {
            Some(b(GRAY_CONCRETE))
        } else if strip {
            Some(b(GRAY_STAINED_GLASS))
        } else {
            Some(b(BLACK_STAINED_GLASS))
        };
    }
    Some(b(BRICK))
}

/// Kaispeicher A: a brick shell two blocks thick round its floors, on a footing.
fn warehouse(m: &mut Model, p: &Plan) {
    let windows: Vec<Vec<f64>> = (0..4).map(window_columns).collect();
    for (x, z, d) in p.cells() {
        let (edge, s) = p.face(x, z);
        if d <= 1 {
            for y in -FOOTING..0 {
                m.set(x, y, z, b(POLISHED_ANDESITE));
            }
            for y in 0..PLAZA - 1 {
                if let Some(part) = brick_wall(edge, s, y, d, &windows[edge]) {
                    m.set(x, y, z, part);
                }
            }
        } else {
            for y in [-1, 4, 8, 12, 16, 20, 24] {
                m.set(x, y, z, b(SMOOTH_STONE));
            }
        }
        m.set(x, PLAZA - 1, z, b(BRICK));
    }
}

/// The Plaza: a railing on the brick edge, the walkway, and glazing round the lobby inside.
fn plaza(m: &mut Model, p: &Plan) {
    for (x, z, d) in p.cells() {
        if p.rim(x, z) {
            m.set(x, PLAZA, z, Part::Pane(0));
        }
        if d == LOBBY {
            for y in PLAZA..SOFFIT {
                m.set(x, y, z, b(GRAY_STAINED_GLASS));
            }
        }
    }
}

/// Whether the panes at `s` show the hotel's tall vent slits rather than wide grey ovals.
fn slit_eyes(edge: usize, s: f64) -> bool {
    edge == EAST || (edge == SOUTH && s < 70.0) || (edge == NORTH && s > 62.0)
}

/// What shows through the glass: frames, the curved part of each pane, and the pane.
fn backing(edge: usize, s: f64, y: i32) -> Block {
    let u = s.rem_euclid(BAY);
    let r = (y - SOFFIT).rem_euclid(ROW);
    if r == 0 || u < 1.0 || s > edge_len(edge) - 1.0 {
        return SMOOTH_STONE;
    }
    let eye = if slit_eyes(edge, s) {
        (2.0..3.0).contains(&u) && r <= 2
    } else {
        (2.0..4.0).contains(&u) && r == 2
    };
    if eye {
        GRAY_CONCRETE
    } else {
        WHITE_CONCRETE
    }
}

/// Glass over its backing up to the roof, floors inside, and the white roof with its discs.
fn superstructure(m: &mut Model, p: &Plan) {
    for (x, z, d) in p.cells() {
        let (edge, s) = p.face(x, z);
        let cover = p.cover(x, z);
        for y in SOFFIT..cover {
            let part = match d {
                0 => LIGHT_BLUE_STAINED_GLASS,
                1 => backing(edge, s, y),
                _ if (y - SOFFIT) % ROW == 0 => SMOOTH_STONE,
                _ => continue,
            };
            m.set(x, y, z, b(part));
        }
        let (top, slab) = p.top(x, z);
        for y in cover..top {
            m.set(x, y, z, b(SMOOTH_QUARTZ));
        }
        let disc = (x * 3 + z * 5).rem_euclid(11) == 0 && !slab;
        m.set(x, top, z, b(if disc { IRON_BLOCK } else { SMOOTH_QUARTZ }));
        if slab {
            m.set(x, top + 1, z, b(SMOOTH_QUARTZ_SLAB));
        }
    }
}

/// Horseshoe loggias: a chequer at the west end's apartments, a few at the foyers.
fn is_loggia(edge: usize, s: f64, row: i32) -> bool {
    let bay = (s / BAY).floor() as i32;
    let from_west = match edge {
        NORTH => s,
        SOUTH => edge_len(SOUTH) - s,
        WEST => 0.0,
        _ => f64::MAX,
    };
    let chequer = (bay + row) % 2 == 0;
    let apartments = match from_west {
        _ if edge == WEST => row >= 1 && bay <= 3,
        w if w < 30.0 => row >= 2,
        w if w < 45.0 && edge == SOUTH => row >= 6,
        _ => false,
    };
    let foyer = FOYER_LOGGIAS
        .iter()
        .any(|&(e, at, r)| e == edge && r == row && (at / BAY).floor() as i32 == bay);
    (apartments && chequer) || foyer
}

/// Each loggia a white U round an opening, a glass rail across it, the room glazed behind.
fn loggias(m: &mut Model, p: &Plan) {
    for (x, z, d) in p.cells() {
        if d > 2 {
            continue;
        }
        let (edge, s) = p.face(x, z);
        let u = s.rem_euclid(BAY);
        if u < 1.0 || s > edge_len(edge) - 1.0 {
            continue;
        }
        let middle = (2.0..4.0).contains(&u);
        let cover = p.cover(x, z);
        let rim = p.rim(x, z);
        for y in SOFFIT..cover {
            let (row, r) = ((y - SOFFIT) / ROW, (y - SOFFIT) % ROW);
            let panel_top = SOFFIT + row * ROW + ROW;
            if r == 0 || panel_top > cover || !is_loggia(edge, s, row) {
                continue;
            }
            let opening = middle && r >= 2;
            match d {
                // A glass rail across the opening, round the corner where the facade steps.
                0 | 1 if opening && r == 2 && rim => m.set(x, y, z, Part::Pane(0)),
                0 | 1 if opening => m.clear(x, y, z),
                0 => m.set(x, y, z, b(SMOOTH_QUARTZ)),
                1 if middle => m.set(x, y, z, b(SMOOTH_QUARTZ)),
                2 if opening => m.set(x, y, z, b(BLACK_STAINED_GLASS)),
                _ => {}
            }
        }
    }
}

/// The vaults cut up into the glass over the Plaza, lined in white.
fn arches(m: &mut Model, p: &Plan) {
    for &(edge, centre, half, height) in &ARCHES {
        let vault = |x: i32, y: i32, z: i32| {
            let Some(d) = p.depth(x, z) else {
                return false;
            };
            let (e, s) = p.face(x, z);
            let u = (s - centre) / half;
            e == edge
                && d < LOBBY
                && y >= SOFFIT
                && u.abs() < 1.0
                && f64::from(y - SOFFIT) < height * (1.0 - u * u).sqrt()
        };
        let mut cut = Vec::new();
        let mut lining = Vec::new();
        for (x, z, d) in p.cells() {
            let (e, s) = p.face(x, z);
            if e != edge || d > LOBBY || (s - centre).abs() > half + 2.0 {
                continue;
            }
            for y in SOFFIT..SOFFIT + height as i32 + 2 {
                if vault(x, y, z) {
                    cut.push((x, y, z));
                } else if d > 0
                    && [
                        (1, 0, 0),
                        (-1, 0, 0),
                        (0, 1, 0),
                        (0, -1, 0),
                        (0, 0, 1),
                        (0, 0, -1),
                    ]
                    .iter()
                    .any(|&(dx, dy, dz)| vault(x + dx, y + dy, z + dz))
                {
                    lining.push((x, y, z));
                }
            }
        }
        for (x, y, z) in cut {
            m.clear(x, y, z);
        }
        for (x, y, z) in lining {
            m.set(x, y, z, b(SMOOTH_QUARTZ));
        }
    }
}

/// Openings in the roof, walled in glass over a white backing.
fn wells(m: &mut Model, p: &Plan) {
    let in_well = |w: &Well, x: i32, z: i32| {
        p.depth(x, z).is_some() && inside(w.outline, f64::from(x) + 0.5, f64::from(z) + 0.5)
    };
    let any_well = |x: i32, z: i32| WELLS.iter().any(|w| in_well(w, x, z));
    for well in &WELLS {
        let open = |x: i32, z: i32| in_well(well, x, z);
        let first = |x: i32, z: i32| !any_well(x, z) && around(x, z).any(|(nx, nz)| open(nx, nz));
        for (x, z, _) in p.cells() {
            if open(x, z) {
                let (top, _) = p.top(x, z);
                for y in well.floor..=top + 1 {
                    m.clear(x, y, z);
                }
                m.set(x, well.floor - 1, z, b(well.bottom));
                continue;
            }
            let wall = if first(x, z) {
                LIGHT_BLUE_STAINED_GLASS
            } else if !any_well(x, z) && around(x, z).any(|(nx, nz)| first(nx, nz)) {
                WHITE_CONCRETE
            } else {
                continue;
            };
            for y in well.floor..p.cover(x, z) {
                m.set(x, y, z, b(wall));
            }
        }
    }
}

fn resolve(part: Part) -> BlockWithProperties {
    match part {
        Part::Block(block) => BlockWithProperties::simple(block),
        Part::Pane(joins) => {
            let side = |bit: u8| if joins & bit != 0 { "true" } else { "false" };
            with_props(
                GLASS_PANE,
                &[
                    ("north", side(1)),
                    ("east", side(2)),
                    ("south", side(4)),
                    ("west", side(8)),
                ],
            )
        }
    }
}

fn finish(mut m: Model) -> PalettizedSchematic {
    let panes: Vec<(i32, i32, i32)> = m
        .cells
        .iter()
        .filter(|(_, part)| matches!(part, Part::Pane(_)))
        .map(|(&k, _)| k)
        .collect();
    // Panes join each other and the full blocks beside them.
    for &(x, y, z) in &panes {
        let joins = around(x, z)
            .enumerate()
            .filter(|(_, (nx, nz))| match m.cells.get(&(*nx, y, *nz)) {
                Some(Part::Pane(_)) => true,
                Some(Part::Block(block)) => *block != SMOOTH_QUARTZ_SLAB,
                None => false,
            })
            .fold(0u8, |acc, (i, _)| acc | (1 << i));
        m.set(x, y, z, Part::Pane(joins));
    }
    let mut palette: Vec<BlockWithProperties> = Vec::new();
    let mut slots: BTreeMap<Part, u8> = BTreeMap::new();
    let mut voxels = Vec::with_capacity(m.cells.len());
    for (&(x, y, z), &part) in &m.cells {
        let slot = *slots.entry(part).or_insert_with(|| {
            palette.push(resolve(part));
            u8::try_from(palette.len() - 1).expect("the Elbphilharmonie uses few block states")
        });
        voxels.push((
            (x + ORIGIN_X) as i16,
            (y + GROUND) as i16,
            (z + ORIGIN_Z) as i16,
            slot,
        ));
    }
    // Voxels sit shifted by the origin, so the box reaches one past the farthest of them.
    let span = |axis: fn(&(i16, i16, i16, u8)) -> i16| {
        voxels
            .iter()
            .map(axis)
            .max()
            .map_or(0, |m| i32::from(m) + 1)
    };
    PalettizedSchematic {
        width: span(|v| v.0),
        length: span(|v| v.2),
        palette,
        voxels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_roof_peaks_over_the_south_west_corner() {
        let model = build();
        let top = model.voxels.iter().max_by_key(|v| v.1).unwrap();
        let (x, z) = (i32::from(top.0) - ORIGIN_X, i32::from(top.2) - ORIGIN_Z);
        // Measured 100.5 m over the quay, 108.8 m above sea level.
        assert_eq!(i32::from(top.1) + 1 - GROUND, 100);
        let (sw_x, sw_z) = CORNERS[3];
        assert!(
            (f64::from(x) - sw_x).abs() < 6.0 && (f64::from(z) - sw_z).abs() < 6.0,
            "{x} {z}"
        );
        // The east end stays some 30 m lower.
        let (se_x, se_z) = CORNERS[2];
        assert!(roof_height(se_x - 2.0, se_z - 5.0) < 75.0);
        assert!(model
            .voxels
            .iter()
            .all(|v| v.0 >= 0 && v.1 >= 0 && v.2 >= 0));
    }

    #[test]
    fn every_voxel_lies_inside_the_schematic() {
        let model = build();
        assert!(model.voxels.iter().all(|v| i32::from(v.0) < model.width
            && i32::from(v.2) < model.length
            && v.0 >= 0
            && v.2 >= 0));
        assert!(model
            .voxels
            .iter()
            .any(|v| i32::from(v.0) == model.width - 1));
        assert!(model
            .voxels
            .iter()
            .any(|v| i32::from(v.2) == model.length - 1));
    }

    #[test]
    fn the_brick_ends_at_the_plaza_under_the_glass() {
        let model = build();
        let at = |x: i32, y: i32, z: i32| {
            model
                .voxels
                .iter()
                .find(|v| {
                    (v.0, v.1, v.2)
                        == (
                            (x + ORIGIN_X) as i16,
                            (y + GROUND) as i16,
                            (z + ORIGIN_Z) as i16,
                        )
                })
                .map(|v| model.palette[v.3 as usize].block)
        };
        // A cell on the west facade, clear of its big window.
        let (x, z) = (-63, -10);
        assert_eq!(at(x, 2, z), Some(BRICK));
        assert_eq!(at(x, PLAZA - 1, z), Some(BRICK));
        assert_eq!(at(x, PLAZA, z), Some(GLASS_PANE));
        assert_eq!(at(x, PLAZA + 1, z), None);
        assert_eq!(at(x, SOFFIT + 2, z), Some(LIGHT_BLUE_STAINED_GLASS));
    }

    #[test]
    fn the_railing_panes_join_round_the_plaza() {
        let model = build();
        let panes: Vec<_> = model
            .voxels
            .iter()
            .filter(|v| model.palette[v.3 as usize].block == GLASS_PANE)
            .collect();
        assert!(panes.len() > 300);
        for v in panes {
            let props = model.palette[v.3 as usize].properties.as_deref();
            let Some(fastnbt::Value::Compound(map)) = props else {
                panic!("pane without sides");
            };
            let joined = map
                .values()
                .filter(|s| matches!(s, fastnbt::Value::String(s) if s == "true"))
                .count();
            assert!(joined >= 2, "pane at {v:?} joins {joined}");
        }
    }
}
