//! The Reichstagsgebäude at one block per metre: Wallot's building with its corner towers,
//! the west portico and Foster's glass dome with its ramps and mirror cone.

use super::brandenburger_tor::{b, finish, hung, stair, Dir, Grid, Part};
use super::schematic::PalettizedSchematic;
use crate::block_definitions::*;

// Model x runs east, z south. The plan is mirrored north to south about z = 69.

const RUSTIC: Block = SANDSTONE;
const ASHLAR: Part = Part::Double(CUT_SANDSTONE_SLAB);
const TRIM: Block = SMOOTH_SANDSTONE;
const MOULDING: Block = SMOOTH_SANDSTONE_STAIRS;
const LIP: Block = CUT_SANDSTONE_SLAB;
const BALUSTER: Block = SANDSTONE_WALL;
const WINDOW: Part = Part::Joined(GRAY_STAINED_GLASS_PANE);
const DECK: Block = SMOOTH_STONE;
const SKYLIGHT: Block = LIGHT_GRAY_STAINED_GLASS;
const DOME_GLASS: Block = GLASS;
/// The dome's ribs and rings, a whiter glass so the steel shows faintly through the skin.
const DOME_FRAME: Block = WHITE_STAINED_GLASS;
const MIRROR: Block = IRON_BLOCK;
const RAMP: Block = SMOOTH_QUARTZ_SLAB;
/// Bronze letters, thin plates on the frieze.
const LETTER: Part = Part::Trapdoor(DARK_OAK_TRAPDOOR, Dir::W);

/// Layers: socle top, main order, entablature, cornice, roof deck.
const SOCLE: i32 = 5;
const ARCHITRAVE: i32 = 24;
const CORNICE: i32 = 26;
const DECK_Y: i32 = 29;
/// The cell column the building is symmetric about east to west; x mirrors to 2 * AXIS - x.
const AXIS: i32 = 60;
/// Dome: centre, base layer, radius and height.
const DOME_X: f64 = AXIS as f64 + 0.5;
const DOME_Z: f64 = 69.0;
const DOME_BASE: i32 = 30;
const DOME_R: f64 = 19.0;
const DOME_H: f64 = 23.5;
/// The plan's mirror axis lies between cell rows 68 and 69.
const MIRROR_SUM: i32 = 137;
/// Rows the plan is moved south so its north front starts at the grid's edge.
const SHIFT_Z: i32 = 1;

/// Model point pinned to the mapped middle of the dome. The map has the dome half a metre west
/// of the outline's axis, so the point sits half a block west of the model's axis and the walls
/// stay on the outline.
pub const ANCHOR: (f64, f64) = (DOME_X - 0.5, DOME_Z + SHIFT_Z as f64);

/// One straight run of outer wall: its outward side, outline row and the cells along it.
#[derive(Clone, Copy)]
struct Run {
    out: Dir,
    row: i32,
    from: i32,
    to: i32,
}

impl Run {
    /// Cell `along` the run, `d` blocks in from the outline (negative is outside).
    fn cell(&self, along: i32, d: i32) -> (i32, i32) {
        match self.out {
            Dir::N => (along, self.row + d),
            Dir::S => (along, self.row - d),
            Dir::W => (self.row + d, along),
            Dir::E => (self.row - d, along),
        }
    }
}

struct Model {
    g: Grid,
}

impl Model {
    fn put(&mut self, r: &Run, along: i32, d: i32, y: i32, part: Part) {
        let (x, z) = r.cell(along, d);
        self.g.set(x, y, z, part);
    }

    fn clear(&mut self, r: &Run, along: i32, d: i32, y: i32) {
        let (x, z) = r.cell(along, d);
        self.g.clear(x, y, z);
    }
}

/// How the windows of a facade's bays are stacked.
#[derive(Clone, Copy, PartialEq)]
enum Storeys {
    /// Round-headed windows on the main floor, pedimented ones above (west and east).
    Arched,
    /// Three rows of square-headed windows (north and south).
    Three,
    /// Plain wall, as on the narrow returns.
    Blank,
}

pub fn build() -> PalettizedSchematic {
    let mut m = Model { g: Grid::default() };
    north_half(&mut m);
    m.g.cells.retain(|&(_, _, z), _| 2 * z < MIRROR_SUM);
    let half: Vec<_> = m.g.cells.iter().map(|(&k, &p)| (k, p)).collect();
    for ((x, y, z), part) in half {
        m.g.set(x, y, MIRROR_SUM - z, mirrored(part));
    }
    inscription(&mut m);
    dome(&mut m);
    // Clear of the grid's edge for the north front's columns and cornice.
    let mut grid = Grid::default();
    for (&(x, y, z), &part) in &m.g.cells {
        grid.set(x, y, z + SHIFT_Z, part);
    }
    finish(&grid)
}

fn flip(d: Dir) -> Dir {
    match d {
        Dir::N => Dir::S,
        Dir::S => Dir::N,
        other => other,
    }
}

fn mirrored(part: Part) -> Part {
    match part {
        Part::Stair(block, d, top) => Part::Stair(block, flip(d), top),
        Part::Trapdoor(block, d) => Part::Trapdoor(block, flip(d)),
        other => other,
    }
}

/// Footprint rectangles of the north half, (x0, x1, z0, z1) inclusive.
const PLAN: [(i32, i32, i32, i32); 9] = [
    // The main block and its north front.
    (17, 103, 4, 68),
    (51, 69, 3, 3),
    // Corner pavilions.
    (19, 31, 0, 2),
    (17, 33, 3, 3),
    (14, 16, 6, 17),
    (89, 101, 0, 2),
    (87, 103, 3, 3),
    (104, 106, 6, 17),
    // East portal.
    (104, 111, 55, 68),
];
/// The north courtyard.
const COURT: (i32, i32, i32, i32) = (46, 74, 28, 43);

fn in_plan(x: i32, z: i32) -> bool {
    let inside = |r: &(i32, i32, i32, i32)| r.0 <= x && x <= r.1 && r.2 <= z && z <= r.3;
    PLAN.iter().any(inside) && !inside(&COURT)
}

fn north_half(m: &mut Model) {
    // The roof deck first, so the facades overwrite its edge.
    for x in 0..=115 {
        for z in 0..=68 {
            if in_plan(x, z) {
                m.g.set(x, DECK_Y, z, b(DECK));
            }
        }
    }
    skylights(m);
    // West front: pavilion, wing and the portico's flank.
    wing(
        m,
        Run {
            out: Dir::W,
            row: 17,
            from: 21,
            to: 52,
        },
        &[21, 27, 33, 39, 45, 52],
        Storeys::Arched,
    );
    wing(
        m,
        Run {
            out: Dir::W,
            row: 17,
            from: 18,
            to: 20,
        },
        &[18, 20],
        Storeys::Blank,
    );
    // East front.
    wing(
        m,
        Run {
            out: Dir::E,
            row: 103,
            from: 21,
            to: 54,
        },
        &[21, 27, 33, 39, 45, 51, 54],
        Storeys::Arched,
    );
    wing(
        m,
        Run {
            out: Dir::E,
            row: 103,
            from: 18,
            to: 20,
        },
        &[18, 20],
        Storeys::Blank,
    );
    // North front: the wings either side of the middle projection.
    wing(
        m,
        Run {
            out: Dir::N,
            row: 4,
            from: 34,
            to: 50,
        },
        &[34, 39, 45, 50],
        Storeys::Three,
    );
    wing(
        m,
        Run {
            out: Dir::N,
            row: 4,
            from: 70,
            to: 86,
        },
        &[70, 75, 81, 86],
        Storeys::Three,
    );
    north_portal(m);
    for east in [false, true] {
        pavilion(m, east);
    }
    courtyard(m);
    west_portico(m);
    east_portal(m);
    restaurant(m);
}

/// One run of facade: rusticated socle, giant pilasters with windows between, entablature,
/// cornice and balustrade, and the wall the windows sit in one block back.
fn wing(m: &mut Model, r: Run, pilasters: &[i32], storeys: Storeys) {
    for along in r.from..=r.to {
        let pil = pilasters.contains(&along);
        // Rusticated socle with its band, the wall behind it for the windows' reveals.
        for y in 0..SOCLE {
            let part = if y % 2 == 0 { b(RUSTIC) } else { ASHLAR };
            m.put(&r, along, 0, y, part);
            m.put(&r, along, 1, y, ASHLAR);
        }
        m.put(&r, along, 1, SOCLE, ASHLAR);
        m.put(&r, along, 0, SOCLE, b(TRIM));
        m.put(&r, along, -1, SOCLE, stair(MOULDING, r.out.back()));
        for y in SOCLE + 1..ARCHITRAVE {
            if pil {
                m.put(&r, along, 0, y, b(TRIM));
            }
            m.put(&r, along, 1, y, ASHLAR);
        }
        if pil {
            m.put(&r, along, -1, ARCHITRAVE - 1, hung(MOULDING, r.out.back()));
        }
        for d in 0..=1 {
            m.put(&r, along, d, ARCHITRAVE, b(TRIM));
            m.put(
                &r,
                along,
                d,
                ARCHITRAVE + 1,
                b(if d == 0 { RUSTIC } else { TRIM }),
            );
            m.put(&r, along, d, CORNICE, b(TRIM));
            m.put(&r, along, d, CORNICE + 1, b(TRIM));
        }
        m.put(&r, along, -1, CORNICE, hung(MOULDING, r.out.back()));
        m.put(&r, along, -1, CORNICE + 1, Part::Slab(LIP, false));
        // Balustrade on the cornice, pedestals with finials over the pilasters.
        m.put(&r, along, 1, CORNICE + 2, b(TRIM));
        for y in CORNICE + 2..=CORNICE + 4 {
            let part = if pil {
                b(TRIM)
            } else if y == CORNICE + 4 {
                Part::Slab(LIP, false)
            } else {
                Part::Joined(BALUSTER)
            };
            m.put(&r, along, 0, y, part);
        }
        if pil {
            m.put(&r, along, 0, CORNICE + 5, Part::Joined(BALUSTER));
        }
    }
    // Windows, centred in each bay.
    for pair in pilasters.windows(2) {
        let (a, z) = (pair[0] + 1, pair[1] - 1);
        let width = z - a + 1;
        if width < 1 {
            continue;
        }
        let w = if width >= 5 {
            3
        } else if width >= 3 {
            width - 2
        } else {
            width
        };
        let lo = a + (width - w) / 2;
        let hi = lo + w - 1;
        // Socle windows, set in a reveal.
        if storeys != Storeys::Blank {
            for along in lo..=hi {
                for y in 1..=3 {
                    m.clear(&r, along, 0, y);
                    m.put(&r, along, 1, y, WINDOW);
                }
            }
        }
        match storeys {
            Storeys::Arched => {
                arched_window(m, &r, lo, hi, 7, 13);
                square_window(m, &r, lo, hi, 17, 20, true);
            }
            Storeys::Three => {
                for (y0, y1) in [(7, 9), (12, 15), (18, 20)] {
                    square_window(m, &r, lo, hi, y0, y1, y0 == 18);
                }
            }
            Storeys::Blank => {}
        }
    }
}

/// A round-headed window in the wall plane between `y0` and `y1`, its corners closed.
fn arched_window(m: &mut Model, r: &Run, lo: i32, hi: i32, y0: i32, y1: i32) {
    for along in lo..=hi {
        for y in y0..=y1 {
            let corner = y == y1 && hi > lo && (along == lo || along == hi);
            if !corner {
                m.put(r, along, 1, y, WINDOW);
            }
        }
        m.put(r, along, 0, y0 - 1, Part::Slab(LIP, true));
    }
    // The keystone over the arch.
    if hi - lo == 2 {
        m.put(r, lo + 1, 0, y1 + 1, b(RUSTIC));
    }
}

/// A square-headed window with a sill and a hood, pedimented if `pediment`.
fn square_window(m: &mut Model, r: &Run, lo: i32, hi: i32, y0: i32, y1: i32, pediment: bool) {
    for along in lo..=hi {
        for y in y0..=y1 {
            m.put(r, along, 1, y, WINDOW);
        }
        m.put(r, along, 0, y0 - 1, Part::Slab(LIP, true));
    }
    let (left, right) = along_dirs(r);
    if pediment && hi - lo == 2 {
        m.put(r, lo, 0, y1 + 1, stair(MOULDING, right));
        m.put(r, lo + 1, 0, y1 + 1, b(TRIM));
        m.put(r, hi, 0, y1 + 1, stair(MOULDING, left));
    } else {
        for along in lo..=hi {
            m.put(r, along, 0, y1 + 1, Part::Slab(LIP, false));
        }
    }
}

/// Directions of decreasing and increasing `along` on a run.
fn along_dirs(r: &Run) -> (Dir, Dir) {
    match r.out {
        Dir::N | Dir::S => (Dir::W, Dir::E),
        Dir::W | Dir::E => (Dir::N, Dir::S),
    }
}

/// The north front's middle: giant columns either side of a tall arched door, a pediment over.
fn north_portal(m: &mut Model) {
    let r = Run {
        out: Dir::N,
        row: 3,
        from: 51,
        to: 69,
    };
    let pilasters = [51, 52, 55, 56, 64, 65, 68, 69];
    wing(m, r, &pilasters, Storeys::Three);
    // The tall arched opening in the middle bay.
    for along in 58..=62 {
        for y in 1..=15 {
            m.put(&r, along, 1, y, WINDOW);
        }
        m.clear(&r, along, 0, 1);
        m.clear(&r, along, 0, 2);
        m.clear(&r, along, 0, 3);
    }
    for along in [58, 62] {
        m.put(&r, along, 1, 15, ASHLAR);
    }
    // Engaged columns stand a block proud of the pilasters.
    for along in pilasters {
        engaged(m, &r, along);
    }
    pediment(m, &r, 50, 70, CORNICE + 1, -1, 2);
}

/// A triangular pediment along `r` from `lo` to `hi`, rising half a block a cell, from depth
/// `d` back to `back`: the gable and the roof behind it in one.
fn pediment(m: &mut Model, r: &Run, lo: i32, hi: i32, y0: i32, d: i32, back: i32) {
    let (left, right) = along_dirs(r);
    let mut k = 0;
    loop {
        let (a, z) = (lo + 2 * k, hi - 2 * k);
        if a > z {
            break;
        }
        let y = y0 + k;
        for dd in d..=back {
            for along in a..=z {
                let part = if along == a || along == z {
                    Part::Slab(LIP, false)
                } else if along == a + 1 && a + 1 < z {
                    stair(MOULDING, right)
                } else if along == z - 1 && z - 1 > a {
                    stair(MOULDING, left)
                } else {
                    b(TRIM)
                };
                m.put(r, along, dd, y, part);
            }
        }
        k += 1;
    }
}

/// A corner pavilion with its tower, stepped twice at the corner; the east ones mirror the
/// west about the axis.
fn pavilion(m: &mut Model, east: bool) {
    let x = |x: i32| if east { 2 * AXIS - x } else { x };
    let span = |a: i32, b: i32| (x(a).min(x(b)), x(a).max(x(b)));
    let (out, back) = if east {
        (Dir::E, Dir::W)
    } else {
        (Dir::W, Dir::E)
    };
    let blank = |m: &mut Model, r: Run| wing(m, r, &[r.from, r.to], Storeys::Blank);
    // The face to the side street and the one to the north, each a big window between
    // paired columns.
    let side = Run {
        out,
        row: x(14),
        from: 6,
        to: 17,
    };
    wing(m, side, &[6, 7, 16, 17], Storeys::Blank);
    big_window(m, &side, 9, 14);
    let (n0, n1) = span(19, 31);
    let front = Run {
        out: Dir::N,
        row: 0,
        from: n0,
        to: n1,
    };
    let pil = [n0, n0 + 1, n1 - 1, n1];
    wing(m, front, &pil, Storeys::Blank);
    big_window(m, &front, n0 + 4, n1 - 4);
    for along in [6, 7, 16, 17] {
        engaged(m, &side, along);
    }
    for along in pil {
        engaged(m, &front, along);
    }
    // The steps of the corner and the returns to the wings.
    let (a, b) = span(17, 18);
    blank(
        m,
        Run {
            out: Dir::N,
            row: 3,
            from: a,
            to: b,
        },
    );
    blank(
        m,
        Run {
            out,
            row: x(17),
            from: 3,
            to: 5,
        },
    );
    blank(
        m,
        Run {
            out,
            row: x(19),
            from: 0,
            to: 2,
        },
    );
    blank(
        m,
        Run {
            out: Dir::N,
            row: 6,
            from: span(14, 16).0,
            to: span(14, 16).1,
        },
    );
    blank(
        m,
        Run {
            out: Dir::S,
            row: 17,
            from: span(14, 16).0,
            to: span(14, 16).1,
        },
    );
    blank(
        m,
        Run {
            out: back,
            row: x(31),
            from: 0,
            to: 2,
        },
    );
    let (a, b) = span(32, 33);
    blank(
        m,
        Run {
            out: Dir::N,
            row: 3,
            from: a,
            to: b,
        },
    );
    tower(m, span(18, 32), (4, 18));
}

/// A giant column a block proud of the face, on its own pedestal, the entablature breaking
/// forward over it.
fn engaged(m: &mut Model, r: &Run, along: i32) {
    for y in 0..SOCLE {
        m.put(r, along, -1, y, b(RUSTIC));
    }
    m.put(r, along, -1, SOCLE, b(TRIM));
    for y in SOCLE + 1..ARCHITRAVE - 1 {
        m.put(r, along, -1, y, b(RUSTIC));
    }
    for y in ARCHITRAVE - 1..CORNICE {
        m.put(r, along, -1, y, b(TRIM));
    }
}

/// The pavilions' tall round-headed window over an aedicule.
fn big_window(m: &mut Model, r: &Run, lo: i32, hi: i32) {
    for along in lo..=hi {
        for y in SOCLE + 1..ARCHITRAVE - 1 {
            m.put(r, along, 1, y, ASHLAR);
        }
    }
    let mid = (lo + hi) / 2;
    arched_window(m, r, mid - 1, mid + 1, 15, 21);
    square_window(m, r, mid - 1, mid + 1, 7, 11, true);
}

/// A corner tower: three round-headed windows a side, cornice, balustrade and a glass lantern.
fn tower(m: &mut Model, xs: (i32, i32), zs: (i32, i32)) {
    let top = 37;
    for x in xs.0..=xs.1 {
        for z in zs.0..=zs.1 {
            let edge = x == xs.0 || x == xs.1 || z == zs.0 || z == zs.1;
            if edge {
                for y in CORNICE + 2..=top {
                    m.g.set(x, y, z, ASHLAR);
                }
            } else {
                m.g.set(x, top, z, b(DECK));
            }
        }
    }
    let runs = [
        Run {
            out: Dir::N,
            row: zs.0,
            from: xs.0,
            to: xs.1,
        },
        Run {
            out: Dir::S,
            row: zs.1,
            from: xs.0,
            to: xs.1,
        },
        Run {
            out: Dir::W,
            row: xs.0,
            from: zs.0,
            to: zs.1,
        },
        Run {
            out: Dir::E,
            row: xs.1,
            from: zs.0,
            to: zs.1,
        },
    ];
    for r in &runs {
        let n = r.to - r.from;
        for along in r.from..=r.to {
            let i = along - r.from;
            // Corner pilasters, the base course and the cornice.
            if i == 0 || i == n {
                for y in CORNICE + 2..=top {
                    m.put(r, along, 0, y, b(TRIM));
                }
            }
            m.put(r, along, 0, CORNICE + 2, b(RUSTIC));
            m.put(r, along, 0, top - 1, b(TRIM));
            m.put(r, along, -1, top, hung(MOULDING, r.out.back()));
            m.put(r, along, -1, top + 1, Part::Slab(LIP, false));
            m.put(r, along, 0, top + 1, b(TRIM));
            let post = i == 0 || i == n || i == n / 2;
            m.put(
                r,
                along,
                0,
                top + 2,
                if post {
                    b(TRIM)
                } else {
                    Part::Joined(BALUSTER)
                },
            );
            m.put(
                r,
                along,
                0,
                top + 3,
                if post {
                    Part::Joined(BALUSTER)
                } else {
                    Part::Slab(LIP, false)
                },
            );
            // Three round-headed windows a side.
            let w = i % 4;
            if (2..n - 1).contains(&i) && w != 1 {
                for y in 31..=34 {
                    m.put(r, along, 0, y, WINDOW);
                }
                if w == 3 {
                    m.put(r, along, 0, 35, WINDOW);
                }
                m.put(r, along, -1, 30, Part::Slab(LIP, true));
            }
        }
    }
    // The round glass roof light inside the parapet.
    let (cx, cz) = (
        f64::from(xs.0 + xs.1 + 1) / 2.0,
        f64::from(zs.0 + zs.1 + 1) / 2.0,
    );
    for x in xs.0 + 1..xs.1 {
        for z in zs.0 + 1..zs.1 {
            let d = (f64::from(x) + 0.5 - cx).hypot(f64::from(z) + 0.5 - cz);
            if d < 5.0 {
                m.g.set(x, top + 1, z, b(SKYLIGHT));
            }
            if d < 2.5 {
                m.g.set(x, top + 2, z, b(SKYLIGHT));
            }
        }
    }
    // Statues on the pavilion's corners at the foot of the tower.
    for (x, z) in [(xs.0 - 1, zs.0 - 1), (xs.1 + 1, zs.0 - 1)] {
        statue(m, x, z);
    }
}

/// A figure on a pedestal: base, body and head.
fn statue(m: &mut Model, x: i32, z: i32) {
    m.g.set(x, CORNICE + 2, z, b(TRIM));
    m.g.set(x, CORNICE + 3, z, Part::Joined(BALUSTER));
    m.g.set(x, CORNICE + 4, z, Part::Joined(BALUSTER));
    m.g.set(x, CORNICE + 5, z, b(RUSTIC));
}

/// The north courtyard's four walls, windowed in rows.
fn courtyard(m: &mut Model) {
    let (x0, x1, z0, z1) = COURT;
    let runs = [
        Run {
            out: Dir::S,
            row: z0 - 1,
            from: x0,
            to: x1,
        },
        Run {
            out: Dir::N,
            row: z1 + 1,
            from: x0,
            to: x1,
        },
        Run {
            out: Dir::E,
            row: x0 - 1,
            from: z0,
            to: z1,
        },
        Run {
            out: Dir::W,
            row: x1 + 1,
            from: z0,
            to: z1,
        },
    ];
    for r in &runs {
        for along in r.from..=r.to {
            let bay = (along - r.from) % 4;
            for y in 0..DECK_Y {
                let window = bay != 0 && y % 4 != 0 && y > 0 && y < 24;
                let part = if y == SOCLE || y == CORNICE {
                    b(TRIM)
                } else if window {
                    WINDOW
                } else {
                    ASHLAR
                };
                m.put(r, along, 0, y, part);
            }
            m.put(r, along, 0, DECK_Y, b(TRIM));
            m.put(r, along, 0, DECK_Y + 1, Part::Joined(BALUSTER));
        }
    }
}

/// Glazed roof lights over the wings, framed by the deck.
fn skylights(m: &mut Model) {
    let lights: [(i32, i32, i32, i32); 4] = [
        (35, 56, 7, 16),
        (64, 85, 7, 16),
        (20, 32, 22, 48),
        (88, 100, 22, 48),
    ];
    // Raised a storey over the deck, so they show above the balustrade as they do.
    for (x0, x1, z0, z1) in lights {
        for x in x0..=x1 {
            for z in z0..=z1 {
                // Ribs every four from both ends, so each light is symmetric in itself.
                let rib = (x - x0).min(x1 - x) % 4 == 0 || (z - z0).min(z1 - z) % 4 == 0;
                m.g.set(x, DECK_Y + 1, z, b(if rib { DECK } else { SKYLIGHT }));
                if x0 < x && x < x1 && z0 < z && z < z1 {
                    m.g.clear(x, DECK_Y, z);
                }
            }
        }
    }
}

/// Six Corinthian columns on a podium up the steps, the pediment over them and the west hall
/// behind rising above the roof between its two corner turrets.
fn west_portico(m: &mut Model) {
    // Podium and the flight of steps down to the forecourt, between cheek walls.
    for x in 8..=16 {
        for z in 53..=68 {
            for y in 0..=SOCLE {
                m.g.set(x, y, z, b(if y % 2 == 0 { RUSTIC } else { TRIM }));
            }
        }
    }
    for (i, x) in (2..=7).enumerate() {
        let y = i as i32;
        for z in 55..=68 {
            for yy in 0..y {
                m.g.set(x, yy, z, b(TRIM));
            }
            m.g.set(x, y, z, stair(MOULDING, Dir::E));
        }
    }
    for x in 4..=7 {
        for z in 53..=54 {
            for y in 0..=SOCLE {
                m.g.set(x, y, z, b(RUSTIC));
            }
            m.g.set(x, SOCLE + 1, z, Part::Slab(LIP, false));
        }
    }
    // Free columns in front, engaged ones against the back wall.
    for z in [53, 59, 65] {
        for x in [8, 15] {
            for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (cx, cz) = (x + dx, z + dz);
                m.g.set(cx, SOCLE + 1, cz, b(TRIM));
                for y in SOCLE + 2..ARCHITRAVE - 2 {
                    m.g.set(cx, y, cz, b(RUSTIC));
                }
                m.g.set(cx, ARCHITRAVE - 2, cz, b(TRIM));
                m.g.set(cx, ARCHITRAVE - 1, cz, b(TRIM));
            }
            // Acanthus capitals flaring out.
            for (cx, cz, d) in [
                (x - 1, z, Dir::E),
                (x - 1, z + 1, Dir::E),
                (x + 2, z, Dir::W),
                (x + 2, z + 1, Dir::W),
                (x, z - 1, Dir::S),
                (x + 1, z - 1, Dir::S),
                (x, z + 2, Dir::N),
                (x + 1, z + 2, Dir::N),
            ] {
                if cx < 17 && m.g.get(cx, ARCHITRAVE - 1, cz).is_none() {
                    m.g.set(cx, ARCHITRAVE - 1, cz, hung(MOULDING, d));
                }
            }
        }
    }
    // Back wall: the glazed entrance hall between the inner columns, carved coats of arms
    // either side.
    for z in 53..=68 {
        for y in SOCLE + 1..ARCHITRAVE {
            let part = if z >= 61 && y < ARCHITRAVE - 2 {
                WINDOW
            } else if (56..=58).contains(&z) && (8..=19).contains(&y) {
                b(RUSTIC)
            } else {
                ASHLAR
            };
            m.g.set(17, y, z, part);
        }
    }
    // The portico's ceiling.
    for x in 9..=16 {
        for z in 53..=68 {
            m.g.set(x, ARCHITRAVE - 1, z, b(TRIM));
        }
    }
    // Entablature, then the pediment and the roof behind it.
    for x in 8..=17 {
        for z in 53..=68 {
            m.g.set(x, ARCHITRAVE, z, b(TRIM));
            m.g.set(x, ARCHITRAVE + 1, z, b(TRIM));
            m.g.set(x, CORNICE, z, b(TRIM));
        }
        m.g.set(x, CORNICE, 52, hung(MOULDING, Dir::S));
        m.g.set(x, CORNICE + 1, 52, Part::Slab(LIP, false));
    }
    for z in 52..=68 {
        m.g.set(7, CORNICE, z, hung(MOULDING, Dir::E));
        m.g.set(7, CORNICE + 1, z, Part::Slab(LIP, false));
    }
    let front = Run {
        out: Dir::W,
        row: 8,
        from: 52,
        to: 85,
    };
    pediment(m, &front, 52, 85, CORNICE + 1, -1, 8);
    // The coat of arms carved in the tympanum.
    for z in 64..=68 {
        for y in CORNICE + 2..=CORNICE + 4 {
            m.g.set(7, y, z, b(RUSTIC));
        }
    }
    // The raised west hall with its glass roof.
    for x in 17..=35 {
        for z in 50..=68 {
            let edge = x == 17 || x == 35 || z == 50;
            if edge {
                for y in CORNICE + 1..=34 {
                    m.g.set(x, y, z, ASHLAR);
                }
                m.g.set(x, 33, z, b(TRIM));
                m.g.set(x, 35, z, b(TRIM));
            } else {
                m.g.set(x, 35, z, b(if (x + z) % 4 == 0 { DECK } else { SKYLIGHT }));
            }
        }
    }
    let hall = [
        Run {
            out: Dir::W,
            row: 17,
            from: 50,
            to: 68,
        },
        Run {
            out: Dir::N,
            row: 50,
            from: 17,
            to: 35,
        },
        Run {
            out: Dir::E,
            row: 35,
            from: 50,
            to: 68,
        },
    ];
    for r in &hall {
        for along in r.from..=r.to {
            m.put(r, along, -1, 34, hung(MOULDING, r.out.back()));
        }
    }
    turret(m, 16, 49);
}

/// A corner turret: a square shaft with an open lantern and a finial.
fn turret(m: &mut Model, x0: i32, z0: i32) {
    for x in x0..=x0 + 2 {
        for z in z0..=z0 + 2 {
            for y in CORNICE + 1..=36 {
                let corner = (x == x0 || x == x0 + 2) && (z == z0 || z == z0 + 2);
                let part = if corner || y < 30 || y == 36 {
                    b(TRIM)
                } else {
                    ASHLAR
                };
                m.g.set(x, y, z, part);
            }
            m.g.set(x, 37, z, Part::Slab(LIP, false));
        }
    }
    for (x, z, d) in [
        (x0 - 1, z0 + 1, Dir::E),
        (x0 + 3, z0 + 1, Dir::W),
        (x0 + 1, z0 - 1, Dir::S),
        (x0 + 1, z0 + 3, Dir::N),
    ] {
        m.g.set(x, 36, z, hung(MOULDING, d));
    }
    let (cx, cz) = (x0 + 1, z0 + 1);
    m.g.set(cx, 37, cz, b(TRIM));
    m.g.set(cx, 38, cz, Part::Joined(BALUSTER));
    m.g.set(cx, 39, cz, b(RUSTIC));
    m.g.set(cx, 40, cz, Part::Joined(BALUSTER));
}

/// The east entrance: giant columns round a high arched portal, an attic over, turrets behind.
fn east_portal(m: &mut Model) {
    let r = Run {
        out: Dir::E,
        row: 111,
        from: 55,
        to: 68,
    };
    wing(m, r, &[55, 56, 60, 61], Storeys::Arched);
    for along in [55, 56, 60, 61] {
        engaged(m, &r, along);
    }
    for along in 66..=68 {
        for y in 0..=16 {
            m.put(&r, along, 1, y, WINDOW);
            m.clear(&r, along, 0, y);
        }
        m.put(&r, along, 0, 17, b(TRIM));
    }
    for along in 55..=68 {
        for y in CORNICE + 2..=30 {
            m.put(&r, along, 0, y, if y == 30 { b(TRIM) } else { ASHLAR });
        }
        m.put(&r, along, -1, 30, hung(MOULDING, Dir::W));
    }
    let side = Run {
        out: Dir::N,
        row: 55,
        from: 104,
        to: 111,
    };
    wing(m, side, &[104, 111], Storeys::Blank);
    turret(m, 97, 51);
}

/// The roof restaurant: a glass pavilion east of the dome.
fn restaurant(m: &mut Model) {
    for x in 81..=101 {
        for z in 60..=68 {
            let edge = x == 81 || x == 101 || z == 60;
            for y in DECK_Y + 1..=34 {
                if edge {
                    let post = (x - 81) % 4 == 0 && z == 60 || (z - 60) % 4 == 0;
                    m.g.set(x, y, z, b(if post { DECK } else { DOME_GLASS }));
                }
            }
            m.g.set(x, 35, z, Part::Slab(SMOOTH_STONE_SLAB, false));
        }
    }
}

/// "DEM DEUTSCHEN VOLKE" in bronze letters on the portico frieze.
fn inscription(m: &mut Model) {
    let text = "DEM DEUTSCHEN VOLKE";
    let start = 69 - text.len() as i32 / 2;
    for (i, c) in text.chars().enumerate() {
        if c != ' ' {
            m.g.set(7, ARCHITRAVE + 1, start + i as i32, LETTER);
        }
    }
}

/// Height of the dome's skin at radius `d`, if it covers that ring.
fn dome_height(d: f64) -> Option<f64> {
    (d <= DOME_R).then(|| f64::from(DOME_BASE) + DOME_H * (1.0 - (d / DOME_R).powi(2)).sqrt())
}

/// Foster's dome: a glass skin with its 24 ribs and its rings in whiter glass, the open
/// crown, two helical ramps inside, the mirror cone down the middle and the platform on top.
fn dome(m: &mut Model) {
    let inside = |x: f64, y: f64, z: f64| -> bool {
        let d = (x - DOME_X).hypot(z - DOME_Z);
        dome_height(d).is_some_and(|h| y < h)
    };
    let (x0, x1) = ((DOME_X - DOME_R) as i32 - 1, (DOME_X + DOME_R) as i32 + 1);
    let (z0, z1) = ((DOME_Z - DOME_R) as i32 - 1, (DOME_Z + DOME_R) as i32 + 1);
    let top = DOME_BASE + DOME_H as i32;
    for x in x0..=x1 {
        for z in z0..=z1 {
            let (cx, cz) = (f64::from(x) + 0.5, f64::from(z) + 0.5);
            let d = (cx - DOME_X).hypot(cz - DOME_Z);
            // Floor of the dome: stone round the glass over the plenary hall.
            if d < DOME_R {
                let floor = if d < 8.0 { b(SKYLIGHT) } else { b(DECK) };
                m.g.set(x, DECK_Y, z, floor);
            }
            for y in DOME_BASE..=top {
                let cy = f64::from(y) + 0.5;
                if !inside(cx, cy, cz) || d < 4.0 {
                    continue;
                }
                // Skin where a cell beside it, or beside the cell above, is outside, so the
                // flat crown has no gaps between its layers.
                let around = [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0), (0.0, 0.0)];
                let skin = around.iter().any(|&(dx, dz)| {
                    !inside(cx + dx, cy, cz + dz) || !inside(cx + dx, cy + 1.0, cz + dz)
                });
                if !skin {
                    continue;
                }
                let angle = (cz - DOME_Z).atan2(cx - DOME_X);
                let step = std::f64::consts::TAU / 24.0;
                let off = (angle / step - (angle / step).round()).abs() * step * d;
                let rib = off < 0.5;
                let ring = (y - DOME_BASE) % 4 == 0 || d < 5.0;
                m.g.set(
                    x,
                    y,
                    z,
                    b(if rib || ring { DOME_FRAME } else { DOME_GLASS }),
                );
            }
        }
    }
    ramps(m);
    cone(m);
}

/// The two ramps winding up inside the skin from the floor to the platform, half a turn apart.
fn ramps(m: &mut Model) {
    let (bottom, platform) = (f64::from(DOME_BASE) + 0.5, 46.0);
    let turns = 2.2;
    for k in 0..2 {
        let steps = 4000;
        for i in 0..=steps {
            let t = f64::from(i) / f64::from(steps);
            let h = bottom + t * (platform - bottom);
            let angle = f64::from(k) * std::f64::consts::PI + t * turns * std::f64::consts::TAU;
            let skin = dome_height_inverse(h);
            for w in [1.2, 2.0, 2.8] {
                let r = skin - w;
                let (x, z) = (DOME_X + r * angle.cos(), DOME_Z + r * angle.sin());
                let y = h.floor() as i32;
                let upper = h - h.floor() >= 0.5;
                m.g.set(
                    x.floor() as i32,
                    y,
                    z.floor() as i32,
                    Part::Slab(RAMP, upper),
                );
            }
        }
    }
    // The viewing platform under the open crown.
    let y = platform as i32;
    for x in 40..=80 {
        for z in 49..=89 {
            let d = (f64::from(x) + 0.5 - DOME_X).hypot(f64::from(z) + 0.5 - DOME_Z);
            if (2.5..dome_height_inverse(platform) - 1.0).contains(&d) {
                m.g.set(x, y, z, Part::Slab(RAMP, true));
            }
        }
    }
}

/// Radius of the dome's skin at height `h`.
fn dome_height_inverse(h: f64) -> f64 {
    let t = ((h - f64::from(DOME_BASE)) / DOME_H).clamp(0.0, 1.0);
    DOME_R * (1.0 - t * t).sqrt()
}

/// The mirror cone, narrow at the plenary hall's glass ceiling and widening up to the platform.
fn cone(m: &mut Model) {
    for y in DOME_BASE..=47 {
        let t = f64::from(y - DOME_BASE) / 16.0;
        let r = if y >= 46 { 2.0 } else { 1.2 + t * 4.8 };
        for x in 50..=70 {
            for z in 59..=79 {
                let d = (f64::from(x) + 0.5 - DOME_X).hypot(f64::from(z) + 0.5 - DOME_Z);
                if d <= r && d > r - 1.2 {
                    m.g.set(x, y, z, b(MIRROR));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dome_crowns_it() {
        let model = build();
        let top = model.voxels.iter().map(|v| v.1).max();
        assert_eq!(top, Some(52));
        let (x, z) = (ANCHOR.0 as i16, ANCHOR.1 as i16);
        assert!(model
            .voxels
            .iter()
            .filter(|v| v.1 == 52)
            .all(|v| (v.0 - x).abs() < 8 && (v.2 - z).abs() < 8));
        assert!(model.voxels.iter().all(|v| v.1 >= 0));
        assert!(model.palette.iter().all(|p| p.block.id() < 256));
    }

    #[test]
    fn north_front_and_dome_mirror_about_the_axis() {
        let model = build();
        let blocks: std::collections::HashMap<(i16, i16, i16), Block> = model
            .voxels
            .iter()
            .map(|v| ((v.0, v.1, v.2), model.palette[v.3 as usize].block))
            .collect();
        // Everything up to the courtyard, and the dome's glass above the ramps, which wind one way.
        for (&(x, y, z), &block) in &blocks {
            let front = z <= 21 + SHIFT_Z as i16;
            let dome = y > 46 && (block == DOME_GLASS || block == DOME_FRAME);
            if front || dome {
                let mirrored = blocks.get(&(2 * AXIS as i16 - x, y, z));
                assert_eq!(mirrored, Some(&block), "{x},{y},{z}");
            }
        }
    }

    #[test]
    fn four_towers_rise_to_41_metres() {
        let model = build();
        let top = |x0: i16, z0: i16| {
            model
                .voxels
                .iter()
                .filter(|v| (x0..x0 + 15).contains(&v.0) && (z0..z0 + 15).contains(&v.2))
                .map(|v| v.1)
                .max()
        };
        let z = 4 + SHIFT_Z as i16;
        let south = (MIRROR_SUM - 18) as i16 + SHIFT_Z as i16;
        for (x, z) in [(18, z), (88, z), (18, south), (88, south)] {
            assert_eq!(top(x, z), Some(40), "tower at {x},{z}");
        }
    }

    #[test]
    fn the_portico_frieze_reads_dem_deutschen_volke() {
        let model = build();
        let letters = model
            .voxels
            .iter()
            .filter(|v| v.0 == 7 && i32::from(v.1) == ARCHITRAVE + 1)
            .filter(|v| model.palette[v.3 as usize].block == DARK_OAK_TRAPDOOR)
            .count();
        assert_eq!(letters, "DEMDEUTSCHENVOLKE".len());
    }
}
