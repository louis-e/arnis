//! The Colosseum at one block per metre as the ruin stands, outer wall on the north side only.

use std::collections::BTreeMap;
use std::f64::consts::TAU;

use super::schematic::PalettizedSchematic;
use crate::block_definitions::*;
use crate::element_processing::structure_kit::top_slab;

/// Model grid, with the centre of the ellipse at (`CX`, `CZ`).
const WIDTH: i32 = 200;
const LENGTH: i32 = 176;
pub const CX: f64 = 100.0;
pub const CZ: f64 = 88.0;
/// Semi-axes of the outer face, and the major axis turned this far from east toward south.
const A: f64 = 94.0;
const B: f64 = 78.0;
const TILT_DEG: f64 = 20.0;
const BAYS: f64 = 80.0;
/// Model layer of the arena and the ground around; the hypogeum floor is the layer at 0.
pub const GROUND: i32 = 7;
const TOP: i32 = 48;

/// Depths in from the outer face where each ring ends, metres.
const RING1: f64 = 2.7;
const AMB1: f64 = 8.2;
const RING2: f64 = 10.2;
const AMB2: f64 = 14.4;
const RING3: f64 = 16.4;
const UPPER: f64 = 28.4;
const RING4: f64 = 30.2;
const PASSAGE: f64 = 33.5;
const LOWER: f64 = 48.4;
const PODIUM: f64 = 51.0;
/// Pier width of the arcades, so the arches are what is left of a bay.
const PIER: f64 = 2.2;
/// Arches of the standing outer wall: XXIII to LIV and the north entrance between them.
const NORTH: (f64, f64) = (43.5, 76.5);
/// Valadier's stepped spur to the west and Stern's brick wedge to the east.
const VALADIER: f64 = 41.0;
const STERN: f64 = 79.5;
/// Arena-axis distance from the centre where the rebuilt floor over the east end begins.
const DECK: f64 = 20.0;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Block(Block),
    /// An upside-down stair with its full side toward north, east, south or west.
    Haunch(Block, u8),
    SlabTop(Block),
    SlabBottom(Block),
}

fn b(block: Block) -> Part {
    Part::Block(block)
}

/// Where a column sits on the oval.
#[derive(Clone, Copy)]
struct Spot {
    /// Metres in from the outer face, negative outside it.
    d: f64,
    /// Bays clockwise from the east end of the major axis; arches centre on whole numbers.
    b: f64,
    /// Unit tangent toward rising `b`, in model x and z.
    tangent: (f64, f64),
    /// Bay width at this depth, metres.
    bay: f64,
    /// Position in the oval's own axes, `u` along the major axis toward the east end.
    u: f64,
    v: f64,
}

impl Spot {
    /// Signed metres along the ring from the nearest arch centre.
    fn off(&self) -> f64 {
        (self.b - self.b.round()) * self.bay
    }

    /// The nearest arch, 0 to 79.
    fn arch(&self) -> i32 {
        (self.b.round() as i32).rem_euclid(BAYS as i32)
    }

    /// Metres from the nearest pier centre.
    fn off_pier(&self) -> f64 {
        self.bay / 2.0 - self.off().abs()
    }

    /// Half the clear width of an arch here.
    fn half_arch(&self) -> f64 {
        (self.bay - PIER) / 2.0
    }

    fn standing(&self) -> bool {
        (NORTH.0..=NORTH.1).contains(&self.b)
    }

    fn valadier(&self) -> bool {
        (VALADIER..NORTH.0).contains(&self.b)
    }

    fn stern(&self) -> bool {
        self.b > NORTH.1 && self.b < STERN
    }
}

/// The outer face as an ellipse, with its arc length tabulated so bays fall evenly along it.
struct Oval {
    arc: Vec<f64>,
    perimeter: f64,
    cos: f64,
    sin: f64,
}

const STEPS: usize = 4096;

impl Oval {
    fn new() -> Oval {
        let speed = |t: f64| (A * A * t.sin().powi(2) + B * B * t.cos().powi(2)).sqrt();
        let h = TAU / STEPS as f64;
        let mut arc = vec![0.0];
        let mut s = 0.0;
        for i in 0..STEPS {
            let t = i as f64 * h;
            s += h / 6.0 * (speed(t) + 4.0 * speed(t + h / 2.0) + speed(t + h));
            arc.push(s);
        }
        let (sin, cos) = TILT_DEG.to_radians().sin_cos();
        Oval {
            arc,
            perimeter: s,
            cos,
            sin,
        }
    }

    fn bay_of(&self, t: f64) -> f64 {
        let f = t.rem_euclid(TAU) / TAU * STEPS as f64;
        let i = (f as usize).min(STEPS - 1);
        let s = self.arc[i] + (self.arc[i + 1] - self.arc[i]) * (f - i as f64);
        s / self.perimeter * BAYS
    }

    fn t_of(&self, bay: f64) -> f64 {
        let s = bay.rem_euclid(BAYS) / BAYS * self.perimeter;
        let i = self.arc.partition_point(|&a| a <= s).clamp(1, STEPS) - 1;
        let f = (s - self.arc[i]) / (self.arc[i + 1] - self.arc[i]);
        (i as f64 + f) / STEPS as f64 * TAU
    }

    /// Model x and z of a point in the oval's axes.
    fn model(&self, u: f64, v: f64) -> (f64, f64) {
        (
            CX + u * self.cos - v * self.sin,
            CZ + u * self.sin + v * self.cos,
        )
    }

    /// The point `d` metres in from the face at `bay`.
    fn point(&self, bay: f64, d: f64) -> (f64, f64) {
        let t = self.t_of(bay);
        let (nu, nv) = (B * t.cos(), A * t.sin());
        let n = nu.hypot(nv);
        self.model(A * t.cos() - nu / n * d, B * t.sin() - nv / n * d)
    }

    fn locate(&self, x: f64, z: f64) -> Spot {
        let (px, pz) = (x - CX, z - CZ);
        let u = px * self.cos + pz * self.sin;
        let v = -px * self.sin + pz * self.cos;
        let dist2 = |t: f64| (A * t.cos() - u).powi(2) + (B * t.sin() - v).powi(2);
        let mut t = (0..256)
            .map(|i| i as f64 / 256.0 * TAU)
            .min_by(|&p, &q| dist2(p).total_cmp(&dist2(q)))
            .unwrap_or(0.0);
        // Newton on the foot of the normal.
        for _ in 0..6 {
            let (s, c) = t.sin_cos();
            let g = (A * c - u) * (-A * s) + (B * s - v) * (B * c);
            let dg =
                A * A * s * s + B * B * c * c + (A * c - u) * (-A * c) + (B * s - v) * (-B * s);
            if dg.abs() < 1e-9 {
                break;
            }
            t -= g / dg;
        }
        let (s, c) = t.sin_cos();
        let inside = (u / A).powi(2) + (v / B).powi(2) < 1.0;
        let dist = dist2(t).sqrt();
        let speed = (A * A * s * s + B * B * c * c).sqrt();
        let kappa = A * B / speed.powi(3);
        let d = if inside { dist } else { -dist };
        let (tu, tv) = (-A * s / speed, B * c / speed);
        Spot {
            d,
            b: self.bay_of(t),
            tangent: (tu * self.cos - tv * self.sin, tu * self.sin + tv * self.cos),
            bay: self.perimeter / BAYS * (1.0 - d * kappa).max(0.05),
            u,
            v,
        }
    }
}

/// Cheap integer hash for the ruin's irregular tops.
fn hash(a: i32, salt: u32) -> u32 {
    let mut h = (a as u32).wrapping_mul(0x9E37_79B1) ^ salt.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^ (h >> 13)
}

/// Height of the outer section at `b`: Stern's wedge slopes, Valadier's spur steps by storeys.
fn envelope(b: f64) -> i32 {
    if (NORTH.0..=NORTH.1).contains(&b) {
        TOP
    } else if (NORTH.1..STERN).contains(&b) {
        (f64::from(TOP) * (STERN - b) / (STERN - NORTH.1)).ceil() as i32
    } else if (VALADIER..NORTH.0).contains(&b) {
        match b {
            b if b >= 43.0 => 35 + ((b - 43.0) / 0.5 * 13.0) as i32,
            b if b >= 42.2 => 34,
            b if b >= 41.6 => 22,
            _ => 10,
        }
    } else {
        0
    }
}

/// A ring's top: `north` within the outer section, `south` per bay elsewhere.
fn ring_top(b: f64, north: i32, south: i32) -> i32 {
    south.max(north.min(envelope(b) - (TOP - north)))
}

/// Top of the second ring per bay where it is the ruin's outer face.
fn ring2_south(arch: i32) -> i32 {
    31 - match hash(arch, 2) % 10 {
        0 => 3,
        1 | 2 => 1,
        _ => 0,
    } - i32::from((22..34).contains(&arch)) * 2
}

fn ring3_south(arch: i32) -> i32 {
    23 + (hash(arch, 3) % 4) as i32
}

struct Model {
    oval: Oval,
    cells: Vec<u16>,
    parts: Vec<Part>,
    slots: BTreeMap<Part, u16>,
}

const HEIGHT: i32 = GROUND + TOP + 2;

impl Model {
    fn new() -> Model {
        Model {
            oval: Oval::new(),
            cells: vec![0; (WIDTH * LENGTH * HEIGHT) as usize],
            parts: Vec::new(),
            slots: BTreeMap::new(),
        }
    }

    fn index(x: i32, layer: i32, z: i32) -> Option<usize> {
        let y = layer + GROUND;
        ((0..WIDTH).contains(&x) && (0..LENGTH).contains(&z) && (0..HEIGHT).contains(&y))
            .then(|| ((y * LENGTH + z) * WIDTH + x) as usize)
    }

    fn set(&mut self, x: i32, layer: i32, z: i32, part: Part) {
        let Some(i) = Self::index(x, layer, z) else {
            return;
        };
        let next = self.parts.len();
        let slot = *self.slots.entry(part).or_insert_with(|| next as u16 + 1);
        if slot as usize > self.parts.len() {
            self.parts.push(part);
        }
        self.cells[i] = slot;
    }

    fn fill(&mut self, x: i32, z: i32, from: i32, to: i32, part: Part) {
        for y in from..=to {
            self.set(x, y, z, part);
        }
    }

    /// The cell holding the point `d` in from the face at `bay`.
    fn cell(&self, bay: f64, d: f64) -> (i32, i32) {
        let (x, z) = self.oval.point(bay, d);
        (x.floor() as i32, z.floor() as i32)
    }
}

pub fn build() -> PalettizedSchematic {
    let mut m = Model::new();
    for x in 0..WIDTH {
        for z in 0..LENGTH {
            let s = m.oval.locate(f64::from(x) + 0.5, f64::from(z) + 0.5);
            column(&mut m, x, z, &s);
        }
    }
    half_columns(&mut m);
    cross(&mut m);
    finish(m)
}

fn column(m: &mut Model, x: i32, z: i32, s: &Spot) {
    let d = s.d;
    if d < -5.0 {
        return;
    }
    let outer = s.standing() || s.valadier();
    if d < 0.0 || (d < AMB1 && !outer && !s.stern()) {
        pavement(m, x, z, s);
        if (-1.0..0.0).contains(&d) && outer {
            bands(m, x, z, envelope(s.b));
        }
        return;
    }
    // A tuff course marks the ground dug out, from the third ring in; outside it the terrain stays.
    if (AMB2..PODIUM).contains(&d) {
        m.set(x, -1, z, b(TUFF));
    }
    // The buttresses are brick through the outer ring; the second ring behind keeps its own height.
    if (s.stern() && d < RING2) || (s.valadier() && (RING1..RING2).contains(&d)) {
        let top = if d < AMB1 {
            envelope(s.b)
        } else {
            envelope(s.b).max(ring2_south(s.arch()))
        };
        m.fill(x, z, 0, top, b(BRICK));
        return;
    }
    if d < RING1 {
        m.set(x, 0, z, b(SMOOTH_SANDSTONE));
        outer_wall(m, x, z, s);
    } else if d < AMB1 {
        m.set(x, 0, z, b(SMOOTH_STONE));
        for y in [10, 22, 34] {
            m.set(x, y, z, b(MUD_BRICKS));
        }
        m.set(x, 46, z, b(SMOOTH_STONE));
    } else if d < RING2 {
        m.set(x, 0, z, b(SMOOTH_SANDSTONE));
        ring2(m, x, z, s);
    } else if d < AMB2 {
        m.set(x, 0, z, b(SMOOTH_STONE));
        m.set(x, 10, z, b(SMOOTH_STONE));
        // Upper floors only where both rings still stand to carry them.
        let outer = ring_top(s.b, 46, 0);
        let inner = ring_top(s.b, 36, ring3_south(s.arch()));
        for y in [22, 34] {
            if outer > y + 1 && inner >= y {
                m.set(x, y, z, b(MUD_BRICKS));
            }
        }
    } else if d < RING3 {
        m.set(x, 0, z, b(SMOOTH_SANDSTONE));
        ring3(m, x, z, s);
    } else if d < UPPER {
        m.set(x, 0, z, b(COARSE_DIRT));
        upper_cavea(m, x, z, s);
    } else if d < RING4 {
        m.set(x, 0, z, b(MUD_BRICKS));
        ring4(m, x, z, s);
    } else if d < PASSAGE {
        m.set(x, 0, z, b(SMOOTH_STONE));
    } else if d < LOWER {
        m.set(x, 0, z, b(COARSE_DIRT));
        lower_cavea(m, x, z, s);
    } else if d < PODIUM {
        // The podium wall, its walk railed on both sides.
        m.fill(x, z, 0, 4, b(SMOOTH_SANDSTONE));
        if d < LOWER + 0.8 || d >= PODIUM - 0.8 {
            m.set(x, 5, z, b(SMOOTH_SANDSTONE));
        }
    } else {
        arena(m, x, z, s);
    }
}

/// The paving round the building, stepping down outward over a masonry footing.
fn pavement(m: &mut Model, x: i32, z: i32, s: &Spot) {
    let top = if s.d >= -2.0 {
        0
    } else {
        (s.d + 2.0).floor() as i32
    };
    m.fill(x, z, -4, top - 1, b(STONE_BRICKS));
    m.set(x, top, z, b(SMOOTH_STONE));
}

/// Entablatures and the crowning cornice, standing proud of the outer face up to `top`.
fn bands(m: &mut Model, x: i32, z: i32, top: i32) {
    for y in [9, 10, 21, 22, 33, 34, 47, 48] {
        if y <= top {
            m.set(x, y, z, b(SMOOTH_SANDSTONE));
        }
    }
}

/// Tuscan, Ionic and Corinthian arcades and the attic; Valadier repeats them in blind brick.
fn outer_wall(m: &mut Model, x: i32, z: i32, s: &Spot) {
    let a = s.off().abs();
    let hw = s.half_arch();
    let face = s.d < 1.0;
    let arch = s.arch();
    let brick = s.valadier();
    let top = envelope(s.b);
    for y in 1..=top {
        let (open, narrow) = match y {
            1..=6 | 12..=16 | 24..=28 => (a < hw, None),
            11 | 23 => (!face && a < hw, None),
            7 => (a < 1.42, Some(hw)),
            17 => (a < 1.3, Some(hw)),
            29 => (a < 1.0, Some(hw)),
            36 => (arch % 2 == 0 && a < 0.55, None),
            39 | 40 => (arch % 2 == 1 && a < 1.0, None),
            _ => (false, None),
        };
        let band = matches!(y, 9 | 10 | 21 | 22 | 33 | 34) || y >= 47;
        // Coursed travertine for the wall, smooth for the entablatures.
        let fill = match (brick, band) {
            (_, true) => SMOOTH_SANDSTONE,
            (true, false) => BRICK,
            (false, false) => SANDSTONE,
        };
        if open {
            if brick && !face {
                m.set(x, y, z, b(BRICK));
            }
            continue;
        }
        let part = match narrow {
            Some(hw) if face && a < hw && !brick => haunch(s, SMOOTH_SANDSTONE_STAIRS, SANDSTONE),
            _ => b(fill),
        };
        m.set(x, y, z, part);
    }
    // Flat pilasters in the attic, flush with the wall.
    if top == TOP && face && s.off_pier() < 0.5 {
        for y in 37..=46 {
            m.set(x, y, z, b(SMOOTH_SANDSTONE));
        }
    }
}

/// An arch's springing: an upside-down stair turned to the pier, where the face runs near an axis.
fn haunch(s: &Spot, stair: Block, plain: Block) -> Part {
    let sign = s.off().signum();
    let (tx, tz) = (s.tangent.0 * sign, s.tangent.1 * sign);
    let (big, small) = (tx.abs().max(tz.abs()), tx.abs().min(tz.abs()));
    if small > big * 0.58 {
        return b(plain);
    }
    let dir = if tx.abs() >= tz.abs() {
        if tx > 0.0 {
            1
        } else {
            3
        }
    } else if tz > 0.0 {
        2
    } else {
        0
    };
    Part::Haunch(stair, dir)
}

/// The second ring: two travertine arcades, and a brick storey with windows where exposed.
fn ring2(m: &mut Model, x: i32, z: i32, s: &Spot) {
    let top = ring_top(s.b, 47, ring2_south(s.arch()));
    let a = s.off().abs();
    let hw = s.half_arch();
    let exposed = !s.standing() && !s.valadier() && !s.stern();
    let face = exposed && s.d < AMB1 + 1.0;
    for y in 1..=top {
        let (open, narrow) = match y {
            1..=6 | 12..=16 => (a < hw, None),
            7 | 17 => (a < 1.4, Some(hw)),
            11 => (!face && a < hw, None),
            23..=28 if top > 34 => (a < hw, None),
            29 if top > 34 => (a < 1.0, None),
            25 | 26 => (a < 1.0, None),
            35..=40 => (a < 1.0, None),
            _ => (false, None),
        };
        if open {
            continue;
        }
        let part = match narrow {
            Some(hw) if face && a < hw => haunch(s, SMOOTH_SANDSTONE_STAIRS, SMOOTH_SANDSTONE),
            _ if exposed && y > 18 => b(BRICK),
            _ => b(SMOOTH_SANDSTONE),
        };
        m.set(x, y, z, part);
    }
}

/// The third ring: travertine piers, brick between them above the ground arcade.
fn ring3(m: &mut Model, x: i32, z: i32, s: &Spot) {
    let top = ring_top(s.b, 36, ring3_south(s.arch()));
    let a = s.off().abs();
    let hw = s.half_arch();
    for y in 1..=top {
        let open = match y {
            1..=6 | 12..=16 | 24..=28 => a < hw,
            7 | 17 | 29 => a < 1.3,
            _ => false,
        };
        if open {
            continue;
        }
        let block = if y >= 11 && a < hw - 0.4 {
            MUD_BRICKS
        } else {
            SMOOTH_SANDSTONE
        };
        m.set(x, y, z, b(block));
    }
}

/// How much lower than the rest a bay's radial walls have come down.
fn decay(arch: i32, salt: u32) -> i32 {
    match hash(arch, salt) % 8 {
        0 => 4,
        1 | 2 => 2,
        3 => 1,
        _ => 0,
    }
}

/// Height at `s` of a tier falling from `high` at depth `from` to `low` at depth `to`.
fn tier(s: &Spot, (from, to): (f64, f64), (high, low): (i32, i32)) -> f64 {
    let f = ((s.d - from) / (to - from)).clamp(0.0, 1.0);
    f64::from(high) - f * f64::from(high - low)
}

/// Radial walls that carried the seats; `vault` bridges each bay with its vault's outer end.
fn wedge(
    m: &mut Model,
    x: i32,
    z: i32,
    s: &Spot,
    span: (f64, f64),
    heights: (i32, i32),
    vault: bool,
) {
    let top = tier(s, span, heights).round() as i32;
    if s.off_pier() < 0.21 * s.bay {
        m.fill(x, z, 1, top, b(MUD_BRICKS));
    } else if vault && s.d < span.0 + 1.6 && top > 6 {
        m.fill(x, z, top - 3, top - 1, b(MUD_BRICKS));
    }
}

/// Stretches of seating rebuilt as smooth slopes: the upper tier, and two of the lower one.
const UPPER_SLOPE: (f64, f64) = (50.0, 66.0);
const LOWER_SLOPES: [(f64, f64); 2] = [(53.0, 59.0), (61.0, 67.0)];

/// A rebuilt slope between bays `bays`, half steps in slabs, railed where its top and sides drop.
fn slope(
    m: &mut Model,
    x: i32,
    z: i32,
    s: &Spot,
    bays: (f64, f64),
    span: (f64, f64),
    heights: (i32, i32),
) {
    let h = tier(s, span, heights);
    let top = h.floor() as i32;
    m.fill(x, z, 1, top, b(SMOOTH_STONE));
    let side = (s.b - bays.0).min(bays.1 - s.b) * s.bay;
    if s.d < span.0 + 0.8 || side < 0.8 {
        m.set(x, top + 1, z, b(SMOOTH_STONE));
    } else if h - h.floor() >= 0.5 {
        m.set(x, top + 1, z, Part::SlabBottom(SMOOTH_STONE_SLAB));
    }
}

fn upper_cavea(m: &mut Model, x: i32, z: i32, s: &Spot) {
    if (UPPER_SLOPE.0..UPPER_SLOPE.1).contains(&s.b) {
        slope(m, x, z, s, UPPER_SLOPE, (RING3, UPPER), (28, 14));
        return;
    }
    let high = ring_top(s.b, 30, 21) - decay(s.arch(), 7);
    wedge(m, x, z, s, (RING3, UPPER), (high, 13), true);
}

/// The ring between the two tiers of seating: a doorway under each half bay, windows above.
fn ring4(m: &mut Model, x: i32, z: i32, s: &Spot) {
    let half = s.b * 2.0;
    let door = ((half - half.floor() - 0.5) * s.bay / 2.0).abs() < 0.7;
    let window = s.off().abs() < 0.7;
    // A step higher where the upper slope comes down to it, as a parapet at its foot.
    let top = if (UPPER_SLOPE.0..UPPER_SLOPE.1).contains(&s.b) {
        15
    } else {
        13
    };
    for y in 1..=top {
        let open = match y {
            1..=5 => door,
            8..=10 => window,
            _ => false,
        };
        if !open {
            let block = if y == top {
                SMOOTH_SANDSTONE
            } else {
                MUD_BRICKS
            };
            m.set(x, y, z, b(block));
        }
    }
}

fn lower_cavea(m: &mut Model, x: i32, z: i32, s: &Spot) {
    if let Some(&bays) = LOWER_SLOPES.iter().find(|r| (r.0..r.1).contains(&s.b)) {
        slope(m, x, z, s, bays, (PASSAGE, LOWER), (11, 5));
    } else {
        let high = 11 - decay(s.arch(), 9).min(2);
        wedge(m, x, z, s, (PASSAGE, LOWER), (high, 4), false);
    }
}

/// The hypogeum's galleries and banks of lift cells, with the rebuilt floor over its east end.
fn arena(m: &mut Model, x: i32, z: i32, s: &Spot) {
    m.set(x, -7, z, b(COARSE_DIRT));
    let deck = s.u >= DECK;
    if deck {
        m.set(x, 0, z, b(BIRCH_PLANKS));
        if s.u < DECK + 1.0 {
            m.set(x, 1, z, b(IRON_BARS));
        }
    }
    let v = s.v.abs();
    let edge = s.d < PODIUM + 4.8;
    let solid = if s.d < PODIUM + 1.0 {
        true
    } else if s.d < PODIUM + 3.6 {
        false
    } else if edge {
        // The curved wall, open where the galleries run into it.
        v >= 1.7 && (v - 1.7).rem_euclid(4.8) < 2.6
    } else if v < 1.7 {
        false
    } else {
        let band = (v - 1.7) / 4.8;
        let i = band.floor();
        let w = (band - i) * 4.8;
        i < 4.0 && w < 2.6 && (w < 0.9 || w >= 1.7 || s.u.rem_euclid(3.0) < 0.9)
    };
    if solid {
        let broken = !deck && hash((s.u / 6.0).floor() as i32, (v / 4.8) as u32).is_multiple_of(5);
        let top = if broken { -3 } else { -1 };
        m.fill(x, z, -6, top, b(if edge { TUFF } else { BRICK }));
    }
}

/// Engaged columns on the piers, with their pedestals and capitals; brick ones on Valadier's spur.
fn half_columns(m: &mut Model) {
    let first = VALADIER.ceil() as i32;
    let last = NORTH.1.floor() as i32;
    for pier in first..=last {
        let p = f64::from(pier) + 0.5;
        let top = envelope(p);
        let (shaft_block, capital_block) = if p < NORTH.0 {
            (BRICK_WALL, BRICK)
        } else {
            (SANDSTONE_WALL, SMOOTH_SANDSTONE)
        };
        let (x, z) = m.cell(p, -0.5);
        // The ground order stands on no pedestal.
        #[allow(clippy::reversed_empty_ranges)]
        let orders = [
            (1..=7, 8, 1..=0),
            (13..=19, 20, 11..=12),
            (25..=31, 32, 23..=24),
        ];
        for (shaft, capital, pedestal) in orders {
            if capital > top {
                break;
            }
            for y in pedestal {
                m.set(x, y, z, b(SMOOTH_SANDSTONE));
            }
            for y in shaft {
                m.set(x, y, z, b(shaft_block));
            }
            m.set(x, capital, z, b(capital_block));
        }
    }
    // Three corbels a bay under the cornice, which held the masts of the awning.
    for arch in NORTH.0.ceil() as i32..=last {
        for k in [-0.25, 0.0, 0.25] {
            let (x, z) = m.cell(f64::from(arch) + k, -0.5);
            m.set(x, 44, z, Part::SlabTop(CUT_SANDSTONE_SLAB));
        }
    }
}

/// The wooden cross on the arena's north edge.
fn cross(m: &mut Model) {
    let (x, z) = m.cell(60.0, PODIUM - 1.5);
    m.fill(x, z, 5, 10, b(DARK_OAK_LOG));
    let bay = m.oval.locate(f64::from(x) + 0.5, f64::from(z) + 0.5).bay;
    for k in [-1.0, 1.0] {
        let (cx, cz) = m.cell(60.0 + k / bay, PODIUM - 1.5);
        m.set(cx, 9, cz, b(DARK_OAK_LOG));
    }
}

fn resolve(part: Part) -> BlockWithProperties {
    const FACING: [StairFacing; 4] = [
        StairFacing::North,
        StairFacing::East,
        StairFacing::South,
        StairFacing::West,
    ];
    match part {
        Part::Block(block) => BlockWithProperties::simple(block),
        Part::Haunch(block, dir) => top_stair(create_stair_with_properties(
            block,
            FACING[dir as usize],
            StairShape::Straight,
        )),
        Part::SlabTop(block) => top_slab(block),
        Part::SlabBottom(block) => BlockWithProperties::simple(block),
    }
}

fn finish(m: Model) -> PalettizedSchematic {
    let palette: Vec<BlockWithProperties> = m.parts.iter().map(|&p| resolve(p)).collect();
    assert!(
        palette.len() <= 256,
        "the Colosseum uses at most 256 block states"
    );
    let mut voxels = Vec::new();
    for x in 0..WIDTH {
        for z in 0..LENGTH {
            for y in 0..HEIGHT {
                let slot = m.cells[((y * LENGTH + z) * WIDTH + x) as usize];
                if slot != 0 {
                    voxels.push((x as i16, y as i16, z as i16, (slot - 1) as u8));
                }
            }
        }
    }
    PalettizedSchematic {
        width: WIDTH,
        length: LENGTH,
        palette,
        voxels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn blocks(model: &PalettizedSchematic) -> HashMap<(i32, i32, i32), Block> {
        model
            .voxels
            .iter()
            .map(|&(x, y, z, slot)| {
                let at = (i32::from(x), i32::from(y) - GROUND, i32::from(z));
                (at, model.palette[slot as usize].block)
            })
            .collect()
    }

    /// Ground-floor arches met walking round the ring `d` in from the face, between two bays.
    fn arches(cells: &HashMap<(i32, i32, i32), Block>, d: f64, from: f64, to: f64) -> usize {
        let oval = Oval::new();
        let mut open = Vec::new();
        let mut bay = from;
        while bay < to {
            let (x, z) = oval.point(bay, d);
            open.push(!cells.contains_key(&(x.floor() as i32, 3, z.floor() as i32)));
            bay += 0.01;
        }
        open.windows(2).filter(|w| w[1] && !w[0]).count()
    }

    #[test]
    fn eighty_bays_round_and_thirty_three_arches_standing() {
        let cells = blocks(&build());
        assert_eq!(arches(&cells, 15.4, 0.0, 80.0), 80);
        assert_eq!(arches(&cells, 1.3, NORTH.0, NORTH.1), 33);
    }

    #[test]
    fn it_stands_48_metres_and_digs_out_only_the_cavea() {
        let model = build();
        let top = model.voxels.iter().map(|v| i32::from(v.1)).max().unwrap();
        assert_eq!(top - GROUND, TOP);
        let oval = Oval::new();
        let mut marked = 0;
        for (&(x, y, z), &block) in &blocks(&model) {
            if block == TUFF && y == -1 {
                let d = oval.locate(f64::from(x) + 0.5, f64::from(z) + 0.5).d;
                assert!(d >= AMB2, "marker in the outer rings at ({x}, {z})");
                marked += 1;
            }
        }
        assert!(marked > 10_000);
    }
}
