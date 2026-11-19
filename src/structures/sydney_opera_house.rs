//! The Sydney Opera House at one block per metre, every shell cut from one sphere of 75.2 m.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::schematic::PalettizedSchematic;
use crate::block_definitions::*;

const RADIUS: f64 = 75.2;
/// Model block of the plan origin at 33.8568 S, 151.2153 E; plan x runs east, z south.
const ORIGIN_X: i32 = 100;
const ORIGIN_Z: i32 = 80;
/// Foot of the seawall under the broadwalk.
const SEAWALL: i32 = -3;
/// Model layer of the broadwalk.
pub const GROUND: i32 = -SEAWALL;
/// Podium deck above the broadwalk.
const DECK: i32 = 10;
/// Model grid in plan metres.
const MIN_X: i32 = -100;
const MAX_X: i32 = 64;
const MIN_Z: i32 = -80;
const MAX_Z: i32 = 146;

/// The land of Bennelong Point the model covers, along the harbour's mapped edge.
const LAND: &[(f64, f64)] = &[
    (-84.8, 123.1),
    (-40.3, 133.6),
    (-1.0, 143.0),
    (3.1, 125.8),
    (8.8, 103.2),
    (20.0, 98.1),
    (21.0, 94.4),
    (24.4, 94.4),
    (37.1, 37.5),
    (40.0, 24.2),
    (53.1, 6.9),
    (60.4, -24.0),
    (56.4, -24.8),
    (51.8, -44.3),
    (49.2, -50.6),
    (45.3, -57.0),
    (40.9, -61.8),
    (36.0, -65.9),
    (31.0, -68.9),
    (23.8, -71.7),
    (13.2, -74.6),
    (4.1, -76.3),
    (-4.2, -76.9),
    (-10.2, -76.7),
    (-18.1, -75.7),
    (-24.0, -74.0),
    (-36.3, -70.1),
    (-44.6, -67.4),
    (-46.4, -66.4),
    (-51.3, -67.7),
    (-56.2, -69.1),
    (-58.9, -70.0),
    (-60.8, -70.3),
    (-62.8, -70.4),
    (-63.8, -69.5),
    (-64.9, -68.3),
    (-65.9, -64.3),
    (-68.6, -52.8),
    (-83.2, 8.3),
    (-97.2, 70.4),
    (-91.5, 121.5),
];

/// The podium, the mapped outline of the building.
const PODIUM: &[(f64, f64)] = &[
    (-74.6, 80.8),
    (-71.7, 68.8),
    (-69.8, 60.9),
    (-68.6, 55.8),
    (-63.2, 32.9),
    (-59.4, 16.5),
    (-58.5, 12.8),
    (-56.2, 3.0),
    (-56.4, 1.6),
    (-56.9, -0.8),
    (-57.4, -3.1),
    (-59.2, -12.8),
    (-56.1, -24.6),
    (-54.4, -31.2),
    (-59.6, -32.3),
    (-57.7, -40.2),
    (-57.5, -41.4),
    (-57.2, -42.4),
    (-55.3, -50.9),
    (-51.1, -52.4),
    (-44.3, -53.7),
    (-38.7, -54.6),
    (-32.1, -55.1),
    (-25.4, -54.9),
    (-19.1, -54.2),
    (-8.9, -51.8),
    (-0.7, -48.9),
    (-0.1, -45.1),
    (1.1, -38.3),
    (1.8, -29.8),
    (-1.7, -29.3),
    (-0.7, -22.2),
    (0.2, -15.3),
    (4.7, -14.0),
    (19.6, -33.7),
    (22.9, -33.3),
    (27.4, -33.1),
    (33.8, -31.7),
    (38.6, -29.8),
    (43.5, -27.5),
    (47.7, -24.9),
    (51.1, -22.5),
    (54.5, -18.7),
    (47.7, 10.0),
    (38.3, 23.2),
    (24.1, 82.8),
    (21.0, 94.4),
    (20.0, 98.1),
    (18.2, 105.4),
    (8.8, 103.2),
    (5.3, 118.1),
    (3.1, 125.8),
    (-36.5, 116.4),
    (-45.4, 114.3),
    (-76.9, 106.9),
    (-80.7, 106.0),
    (-79.0, 99.4),
];

/// The Monumental Steps, from just inside the podium's south edge down to the forecourt.
const STEPS: [(f64, f64); 4] = [(-80.5, 105.0), (3.3, 124.8), (-1.0, 143.0), (-84.8, 123.1)];

#[derive(Clone, Copy, PartialEq)]
enum Mouth {
    /// Glazed from the mouth's edge down to the deck, leaning out at the foot.
    Glazed,
    /// Glass set back under the shell, so its ribs show.
    Recessed,
    /// Under the next shell of a nested run.
    Covered,
}

/// One shell: tip and ridge end on its axis, then its two pedestals.
struct Shell {
    tip: [f64; 3],
    back: [f64; 3],
    feet: [(f64, f64); 2],
    mouth: Mouth,
    /// Concert Hall, Joan Sutherland Theatre, Bennelong restaurant.
    hall: usize,
}

const fn shell(
    tip: (f64, f64, f64),
    back: (f64, f64, f64),
    feet: [(f64, f64); 2],
    mouth: Mouth,
    hall: usize,
) -> Shell {
    Shell {
        tip: [tip.0, tip.1, tip.2],
        back: [back.0, back.1, back.2],
        feet,
        mouth,
        hall,
    }
}

/// Halls, numbered as in `Shell::hall`.
const CONCERT_HALL: usize = 0;
const THEATRE: usize = 1;
const RESTAURANT: usize = 2;

const A4: usize = 0;
const A3: usize = 1;
const A2: usize = 2;
const A1: usize = 3;
const B4: usize = 4;
const B3: usize = 5;
const B2: usize = 6;
const B1: usize = 7;
const C2: usize = 8;
const C1: usize = 9;

/// Plan corners from the mapped shell parts, heights from photographs.
const SHELLS: [Shell; 10] = [
    shell(
        (-27.3, 38.0, -51.9),
        (-28.35, 33.5, -25.0),
        [(-51.9, -34.7), (-4.1, -33.0)],
        Mouth::Glazed,
        CONCERT_HALL,
    ),
    shell(
        (-28.2, 50.0, -27.8),
        (-29.05, 45.0, -4.3),
        [(-55.6, -13.3), (-1.5, -12.5)],
        Mouth::Covered,
        CONCERT_HALL,
    ),
    shell(
        (-28.85, 64.0, -9.9),
        (-30.35, 31.0, 33.35),
        [(-54.3, 8.0), (-4.8, 9.7)],
        Mouth::Covered,
        CONCERT_HALL,
    ),
    shell(
        (-31.8, 40.0, 72.55),
        (-30.55, 30.0, 37.9),
        [(-46.4, 57.4), (-16.1, 58.5)],
        Mouth::Recessed,
        CONCERT_HALL,
    ),
    shell(
        (39.15, 36.0, -27.75),
        (30.95, 31.0, -9.75),
        [(16.2, -22.5), (50.3, -7.1)],
        Mouth::Glazed,
        THEATRE,
    ),
    shell(
        (31.35, 44.0, -10.6),
        (24.25, 44.0, 5.0),
        [(7.0, -7.6), (46.6, 8.0)],
        Mouth::Covered,
        THEATRE,
    ),
    shell(
        (25.5, 55.0, 2.35),
        (11.2, 29.0, 33.85),
        [(2.4, 7.1), (37.1, 22.9)],
        Mouth::Covered,
        THEATRE,
    ),
    shell(
        (-1.25, 34.0, 61.25),
        (9.6, 29.0, 37.4),
        [(-6.8, 46.6), (13.4, 55.7)],
        Mouth::Recessed,
        THEATRE,
    ),
    shell(
        (-51.85, 23.0, 60.1),
        (-55.45, 19.0, 75.35),
        [(-64.55, 68.3), (-44.0, 72.5)],
        Mouth::Glazed,
        RESTAURANT,
    ),
    shell(
        (-61.7, 25.0, 101.65),
        (-55.45, 19.0, 75.35),
        [(-71.0, 80.5), (-44.2, 86.0)],
        Mouth::Recessed,
        RESTAURANT,
    ),
];

/// Each shell's back edge rises to the mouth of the shell behind it, closed by louvre shells.
const NESTED: [(usize, usize); 4] = [(A4, A3), (A3, A2), (B4, B3), (B3, B2)];
/// Shells set back to back, joined by side shells along their back edges.
const BACK_TO_BACK: [(usize, usize); 3] = [(A2, A1), (B2, B1), (C2, C1)];

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Part {
    Block(Block),
    /// A step climbing north.
    Step(Block),
}

/// What wins where two parts claim a cell.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Layer {
    HallWall,
    Ground,
    Infill,
    Mullion,
    Underside,
    Tiles,
    Pedestal,
}

#[derive(Clone, Copy)]
struct V(f64, f64, f64);

impl V {
    fn of(a: [f64; 3]) -> V {
        V(a[0], a[1], a[2])
    }
    fn plus(self, o: V) -> V {
        V(self.0 + o.0, self.1 + o.1, self.2 + o.2)
    }
    fn minus(self, o: V) -> V {
        V(self.0 - o.0, self.1 - o.1, self.2 - o.2)
    }
    fn scale(self, k: f64) -> V {
        V(self.0 * k, self.1 * k, self.2 * k)
    }
    fn dot(self, o: V) -> f64 {
        self.0 * o.0 + self.1 * o.1 + self.2 * o.2
    }
    fn cross(self, o: V) -> V {
        V(
            self.1 * o.2 - self.2 * o.1,
            self.2 * o.0 - self.0 * o.2,
            self.0 * o.1 - self.1 * o.0,
        )
    }
    fn norm(self) -> f64 {
        self.dot(self).sqrt()
    }
    fn unit(self) -> V {
        self.scale(1.0 / self.norm())
    }
}

/// One half of a shell on its sphere.
struct Half {
    foot: V,
    tip: V,
    back: V,
    centre: V,
    /// Unit direction from the centre to the foot.
    p: V,
    /// Planes through the centre bounding the half: the mouth rib and the back rib.
    mouth_n: V,
    back_n: V,
    /// Horizontal normal of the ridge plane, toward the foot.
    ridge_n: V,
    /// Tangents at the foot toward the tip and the back, and the angle between them.
    to_tip: V,
    fan: f64,
    middle: V,
}

impl Half {
    fn new(foot: V, tip: V, back: V) -> Half {
        let centre = sphere_centre(foot, tip, back);
        let dir = |q: V| q.minus(centre).unit();
        let (p, t, b) = (dir(foot), dir(tip), dir(back));
        let toward = |n: V, q: V| if n.dot(q) < 0.0 { n.scale(-1.0) } else { n };
        let axis = V(back.0 - tip.0, 0.0, back.2 - tip.2);
        let ridge_n = toward(V(-axis.2, 0.0, axis.0).unit(), foot.minus(tip));
        let tangent = |q: V| q.minus(p.scale(p.dot(q))).unit();
        let (to_tip, to_back) = (tangent(t), tangent(b));
        Half {
            foot,
            tip,
            back,
            centre,
            p,
            mouth_n: toward(p.cross(t), b),
            back_n: toward(p.cross(b), t),
            ridge_n,
            to_tip,
            fan: to_tip.dot(to_back).clamp(-1.0, 1.0).acos(),
            middle: p.plus(t).plus(b).unit(),
        }
    }

    /// Great-circle arc from `a` to `b` on this half's sphere.
    fn arc(&self, a: V, b: V, s: f64) -> V {
        let (u, w) = (a.minus(self.centre), b.minus(self.centre));
        let angle = (u.dot(w) / (u.norm() * w.norm())).clamp(-1.0, 1.0).acos();
        if angle < 1e-9 {
            return a;
        }
        let k = 1.0 / angle.sin();
        self.centre
            .plus(u.scale(((1.0 - s) * angle).sin() * k))
            .plus(w.scale((s * angle).sin() * k))
    }

    fn mouth(&self, s: f64) -> V {
        self.arc(self.foot, self.tip, s)
    }

    fn back_edge(&self, s: f64) -> V {
        self.arc(self.foot, self.back, s)
    }

    /// A point on the sphere, moved in to the middle of the shell's thickness.
    fn inset(&self, q: V) -> V {
        self.centre
            .plus(q.minus(self.centre).scale((RADIUS - 0.9) / RADIUS))
    }

    /// Unit vector along the plan axis, out of the mouth.
    fn forward(&self) -> V {
        V(self.tip.0 - self.back.0, 0.0, self.tip.2 - self.back.2).unit()
    }

    fn bounds(&self) -> ([i32; 3], [i32; 3]) {
        let mut lo = [f64::MAX; 3];
        let mut hi = [f64::MIN; 3];
        let mut grow = |q: V| {
            for (i, c) in [q.0, q.1, q.2].into_iter().enumerate() {
                lo[i] = lo[i].min(c);
                hi[i] = hi[i].max(c);
            }
        };
        for i in 0..=12 {
            for j in 0..=12 - i {
                let (u, w) = (i as f64 / 12.0, j as f64 / 12.0);
                let q = self
                    .foot
                    .plus(self.tip.minus(self.foot).scale(u))
                    .plus(self.back.minus(self.foot).scale(w))
                    .minus(self.centre);
                grow(self.centre.plus(q.unit().scale(RADIUS)));
            }
        }
        for i in 0..=40 {
            let s = i as f64 / 40.0;
            grow(self.mouth(s));
            grow(self.back_edge(s));
        }
        (
            lo.map(|c| c.floor() as i32 - 2),
            hi.map(|c| c.ceil() as i32 + 2),
        )
    }
}

/// Centre of the radius-75.2 sphere through three points, the one below them.
fn sphere_centre(a: V, b: V, c: V) -> V {
    let (u, w) = (b.minus(a), c.minus(a));
    let n = u.cross(w);
    let nn = n.dot(n);
    let circ = a.plus(
        n.cross(u)
            .scale(w.dot(w))
            .plus(w.cross(n).scale(u.dot(u)))
            .scale(0.5 / nn),
    );
    let h = (RADIUS * RADIUS - circ.minus(a).dot(circ.minus(a)))
        .max(0.0)
        .sqrt();
    let n = n.unit();
    let (c1, c2) = (circ.plus(n.scale(h)), circ.minus(n.scale(h)));
    if c1.1 < c2.1 {
        c1
    } else {
        c2
    }
}

fn split(s: &Shell) -> [Half; 2] {
    let (tip, back) = (V::of(s.tip), V::of(s.back));
    let foot = |(x, z): (f64, f64)| V(x, f64::from(DECK + 1), z);
    [
        Half::new(foot(s.feet[0]), tip, back),
        Half::new(foot(s.feet[1]), tip, back),
    ]
}

#[derive(Default)]
struct Model {
    cells: BTreeMap<(i32, i32, i32), (Layer, Part)>,
    /// Lowest roof cell over each column, per hall, for the walls under its edge.
    roofs: [HashMap<(i32, i32), i32>; 3],
}

impl Model {
    fn put(&mut self, x: i32, y: i32, z: i32, layer: Layer, part: Part) {
        let cell = self.cells.entry((x, y, z)).or_insert((layer, part));
        if layer >= cell.0 {
            *cell = (layer, part);
        }
    }

    fn roof(&mut self, hall: usize, x: i32, y: i32, z: i32) {
        let low = self.roofs[hall].entry((x, z)).or_insert(y);
        *low = (*low).min(y);
    }
}

pub fn build() -> PalettizedSchematic {
    finish(model())
}

fn model() -> Model {
    let mut m = Model::default();
    ground(&mut m);
    let halves: Vec<[Half; 2]> = SHELLS.iter().map(split).collect();
    for (shell, pair) in SHELLS.iter().zip(&halves) {
        // Keep the walls under a mouth behind its glass.
        let clear = match shell.mouth {
            Mouth::Recessed => 5.0,
            Mouth::Glazed => 2.0,
            Mouth::Covered => 0.0,
        };
        for half in pair {
            tiles(&mut m, half, shell.hall, clear);
        }
    }
    for &(front, back) in &NESTED {
        for (a, b) in halves[front].iter().zip(&halves[back]) {
            infill(
                &mut m,
                SHELLS[front].hall,
                |s| a.inset(a.back_edge(1.0 - s)),
                |s| b.inset(b.mouth(1.0 - s)),
            );
        }
    }
    for &(a, b) in &BACK_TO_BACK {
        for (ha, hb) in halves[a].iter().zip(&halves[b]) {
            infill(
                &mut m,
                SHELLS[a].hall,
                |s| ha.inset(ha.back_edge(s)),
                |s| hb.inset(hb.back_edge(s)),
            );
        }
    }
    for (shell, pair) in SHELLS.iter().zip(&halves) {
        for half in pair {
            glazing(&mut m, half, shell.mouth);
        }
    }
    hall_walls(&mut m);
    m
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

/// Broadwalk, seawall, podium and the Monumental Steps.
fn ground(m: &mut Model) {
    let [head, _, b, a] = STEPS;
    let along = V(b.0 - a.0, 0.0, b.1 - a.1).unit();
    // Up the steps, toward the podium.
    let up = V(along.2, 0.0, -along.0);
    let rise = |x: f64, z: f64| (x - a.0) * up.0 + (z - a.1) * up.2;
    let run = rise(head.0, head.1);

    let level = |x: i32, z: i32| -> Option<i32> {
        let (cx, cz) = (f64::from(x) + 0.5, f64::from(z) + 0.5);
        if !inside(LAND, cx, cz) {
            return None;
        }
        if inside(&STEPS, cx, cz) {
            let t = rise(cx, cz) / run;
            return Some(((t * f64::from(DECK)).round() as i32).clamp(0, DECK));
        }
        Some(if inside(PODIUM, cx, cz) { DECK } else { 0 })
    };
    let mut levels: HashMap<(i32, i32), i32> = HashMap::new();
    for x in MIN_X..MAX_X {
        for z in MIN_Z..MAX_Z {
            if let Some(y) = level(x, z) {
                levels.insert((x, z), y);
            }
        }
    }
    for (&(x, z), &y) in &levels {
        let around =
            [(1, 0), (-1, 0), (0, 1), (0, -1)].map(|(dx, dz)| levels.get(&(x + dx, z + dz)));
        let shore = around.iter().any(Option::is_none);
        let low = if shore {
            SEAWALL
        } else {
            around.iter().flatten().map(|&&l| l).min().unwrap_or(y)
        };
        // Granite steps, a stair where the next step down lies to the south.
        let top = if y > 0 && levels.get(&(x, z + 1)) == Some(&(y - 1)) {
            Part::Step(POLISHED_GRANITE_STAIRS)
        } else if y == 0 || y == DECK {
            Part::Block(WHITE_TERRACOTTA)
        } else {
            Part::Block(POLISHED_GRANITE)
        };
        m.put(x, y, z, Layer::Ground, top);
        let from = if shore { SEAWALL } else { low + 1 };
        let facade = !shore && low == 0 && y == DECK;
        for wy in from..y {
            // Granite panels banded every four courses; the seawall below the broadwalk.
            let block = if facade && podium_glass(x, z, wy) {
                BROWN_STAINED_GLASS
            } else if wy < 0 || (wy - from) % 4 == 3 {
                POLISHED_GRANITE
            } else {
                GRANITE
            };
            m.put(x, wy, z, Layer::Ground, Part::Block(block));
        }
        if y == DECK && low < DECK - 2 {
            m.put(x, DECK + 1, z, Layer::Ground, Part::Block(POLISHED_GRANITE));
        }
    }
}

/// Foyer bands round the north end, bars along the south-west, window slots along the east.
fn podium_glass(x: i32, z: i32, y: i32) -> bool {
    if z < -45 && x < -10 {
        y % 3 != 0
    } else if x < -50 && z > 0 {
        (1..=3).contains(&y) && (x + z).rem_euclid(4) != 0
    } else {
        x > 20 && (5..=6).contains(&y) && z.rem_euclid(18) < 11
    }
}

/// A half shell of tiles, one spherical triangle between its pedestal, tip and ridge end.
fn tiles(m: &mut Model, h: &Half, hall: usize, clear: f64) {
    const THICK: f64 = 1.8;
    const RIB_EVERY: f64 = 10.0;
    let (lo, hi) = h.bounds();
    let mut shell: Vec<((i32, i32, i32), V)> = Vec::new();
    for x in lo[0]..=hi[0] {
        for y in lo[1].max(DECK + 1)..=hi[1] {
            for z in lo[2]..=hi[2] {
                let q = V(f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5);
                let d = q.minus(h.centre);
                let r = d.norm();
                if r > RADIUS || r < RADIUS - THICK {
                    continue;
                }
                let u = d.scale(1.0 / r);
                if u.dot(h.mouth_n) < 0.0
                    || u.dot(h.back_n) < 0.0
                    || u.dot(h.middle) < 0.5
                    || q.minus(h.tip).dot(h.ridge_n) < -0.6
                {
                    continue;
                }
                shell.push(((x, y, z), q));
            }
        }
    }
    let cells: HashSet<(i32, i32, i32)> = shell.iter().map(|c| c.0).collect();
    let reach = shell
        .iter()
        .map(|(_, q)| {
            RADIUS
                * q.minus(h.centre)
                    .unit()
                    .dot(h.p)
                    .clamp(-1.0, 1.0)
                    .acos()
                    .sin()
        })
        .fold(0.0, f64::max);
    let ribs = ((h.fan * reach / RIB_EVERY).round() as i32).max(3);
    let step = h.fan / f64::from(ribs);
    for &((x, y, z), q) in &shell {
        let r = q.minus(h.centre).norm();
        let u = q.minus(h.centre).unit();
        // Angle round the pedestal from the mouth rib, and arc length up from the pedestal.
        let along = u.minus(h.p.scale(h.p.dot(u))).unit();
        let phi = along.dot(h.to_tip).clamp(-1.0, 1.0).acos();
        let theta = u.dot(h.p).clamp(-1.0, 1.0).acos();
        let width = RADIUS * theta.sin();
        let arc = RADIUS * theta;
        let nearest = (phi / step).round();
        let off_rib = (phi - nearest * step).abs() * width;
        let to_mouth = phi * width;
        let to_back = (h.fan - phi) * width;
        // Faces the outside if a free neighbour lies farther from the centre.
        let mut outward = false;
        let mut inward = false;
        for (dx, dy, dz) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            let n = (x + dx, y + dy, z + dz);
            if cells.contains(&n) {
                continue;
            }
            let nq = V(
                f64::from(n.0) + 0.5,
                f64::from(n.1) + 0.5,
                f64::from(n.2) + 0.5,
            );
            if nq.minus(h.centre).norm() > r {
                outward = true;
            } else {
                inward = true;
            }
        }
        let (layer, block) = if inward && !outward {
            let rib = off_rib < 0.9 || to_mouth < 1.2 || to_back < 1.2;
            (
                Layer::Underside,
                if rib { SMOOTH_SANDSTONE } else { SANDSTONE },
            )
        } else if q.minus(h.tip).dot(h.ridge_n) < 0.9 || to_mouth < 0.7 || to_back < 0.7 {
            (Layer::Tiles, SMOOTH_SANDSTONE)
        } else if arc < 3.5 {
            (Layer::Tiles, SMOOTH_STONE)
        } else if off_rib < 0.45 {
            (Layer::Tiles, CALCITE)
        } else {
            // Chevrons of matte cream across each lid of glossy white tiles.
            let across = (phi - nearest * step).abs() * width;
            let chevron = (arc + 1.4 * across).rem_euclid(3.0) < 0.8;
            (
                Layer::Tiles,
                if chevron {
                    SMOOTH_QUARTZ
                } else {
                    WHITE_CONCRETE
                },
            )
        };
        m.put(x, y, z, layer, Part::Block(block));
        if to_mouth >= clear {
            m.roof(hall, x, y, z);
        }
    }
    // The pedestal the ribs spring from.
    let (fx, fz) = (h.foot.0.floor() as i32, h.foot.2.floor() as i32);
    for (dx, dz) in [(-1, -1), (-1, 0), (0, -1), (0, 0)] {
        m.put(
            fx + dx,
            DECK + 1,
            fz + dz,
            Layer::Pedestal,
            Part::Block(SMOOTH_STONE),
        );
    }
}

/// Fill between two edges: tiled where it lies shallow, louvre glass where it stands steep.
fn infill(m: &mut Model, hall: usize, a: impl Fn(f64) -> V, b: impl Fn(f64) -> V) {
    const N: usize = 80;
    for i in 0..=N {
        let s = i as f64 / N as f64;
        let (p, q) = (a(s), b(s));
        let along =
            a(((i + 1).min(N)) as f64 / N as f64).minus(a(i.saturating_sub(1) as f64 / N as f64));
        let n = along.cross(q.minus(p));
        let steep = n.norm() > 1e-9 && n.1.abs() / n.norm() < 0.5;
        let block = if steep {
            BROWN_STAINED_GLASS
        } else {
            WHITE_CONCRETE
        };
        for (x, y, z) in line(p, q) {
            m.put(x, y, z, Layer::Infill, Part::Block(block));
            m.roof(hall, x, y, z);
        }
    }
}

/// Bronze glass in the mouths: hung to the deck in front, set back under the south shells.
fn glazing(m: &mut Model, h: &Half, mouth: Mouth) {
    const N: usize = 120;
    if mouth == Mouth::Covered {
        return;
    }
    let fwd = h.forward();
    let foot_y = f64::from(DECK + 1) + 0.5;
    let pane = |s: f64| {
        let top = h.inset(h.mouth(s));
        let foot = V(top.0, foot_y, top.2);
        if mouth == Mouth::Glazed {
            // Leaning out at the foot, but not past the podium's edge.
            let mut lean = 0.12 * (top.1 - foot_y);
            while lean > 0.0 && !inside(PODIUM, foot.0 + fwd.0 * lean, foot.2 + fwd.2 * lean) {
                lean -= 0.25;
            }
            (top, foot.plus(fwd.scale(lean.max(0.0))))
        } else {
            let back = fwd.scale(-4.0);
            (top.plus(back).minus(V(0.0, 1.0, 0.0)), foot.plus(back))
        }
    };
    // Where a shell's edge overhangs the podium, the glass stops at the podium's edge.
    let over_deck =
        |&(x, _, z): &(i32, i32, i32)| inside(PODIUM, f64::from(x) + 0.5, f64::from(z) + 0.5);
    for i in 0..=N {
        let (from, to) = pane(i as f64 / N as f64);
        for (x, y, z) in line(from, to).filter(over_deck) {
            m.put(x, y, z, Layer::Infill, Part::Block(BROWN_STAINED_GLASS));
        }
    }
    // Bronze mullions every 4 m across the hung walls.
    if mouth == Mouth::Glazed {
        let span = h.foot.minus(h.tip).norm();
        for k in 1..(span / 4.0).ceil() as i32 {
            let (from, to) = pane(f64::from(k) * 4.0 / span);
            for (x, y, z) in line(from, to).filter(over_deck) {
                m.put(x, y, z, Layer::Mullion, Part::Block(BROWN_TERRACOTTA));
            }
        }
    }
}

/// Cells along a segment, above the deck.
fn line(from: V, to: V) -> impl Iterator<Item = (i32, i32, i32)> {
    let steps = ((to.minus(from).norm() * 2.5) as usize).max(2);
    (0..=steps).filter_map(move |j| {
        let q = from.plus(to.minus(from).scale(j as f64 / steps as f64));
        let (x, y, z) = (q.0.floor() as i32, q.1.floor() as i32, q.2.floor() as i32);
        (y > DECK).then_some((x, y, z))
    })
}

/// Glass under the edge of each hall's roof, and a timber auditorium well inside it.
fn hall_walls(m: &mut Model) {
    const FOYER: usize = 9;
    for (hall, roof) in std::mem::take(&mut m.roofs).into_iter().enumerate() {
        let edge = |cells: &HashMap<(i32, i32), i32>, x: i32, z: i32| {
            [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .any(|(dx, dz)| !cells.contains_key(&(x + dx, z + dz)))
        };
        for (&(x, z), &low) in &roof {
            if edge(&roof, x, z) {
                for y in DECK + 1..low.min(DECK + 46) {
                    m.put(x, y, z, Layer::HallWall, Part::Block(BROWN_STAINED_GLASS));
                }
            }
        }
        if hall == RESTAURANT {
            continue;
        }
        let mut core = roof;
        for _ in 0..FOYER {
            let rim: Vec<(i32, i32)> = core
                .keys()
                .filter(|&&(x, z)| edge(&core, x, z))
                .copied()
                .collect();
            for cell in rim {
                core.remove(&cell);
            }
        }
        for (&(x, z), &low) in &core {
            let top = (low - 4).min(DECK + 22);
            let from = if edge(&core, x, z) { DECK + 1 } else { top };
            for y in from..=top {
                m.put(x, y, z, Layer::HallWall, Part::Block(SPRUCE_PLANKS));
            }
        }
    }
}

fn resolve(part: Part) -> BlockWithProperties {
    match part {
        Part::Block(block) => BlockWithProperties::simple(block),
        Part::Step(block) => {
            create_stair_with_properties(block, StairFacing::North, StairShape::Straight)
        }
    }
}

fn finish(m: Model) -> PalettizedSchematic {
    let mut palette: Vec<BlockWithProperties> = Vec::new();
    let mut slots: HashMap<Part, u8> = HashMap::new();
    let mut voxels = Vec::with_capacity(m.cells.len());
    let cell = |v: i32| i16::try_from(v).expect("the opera house fits its grid");
    for ((x, y, z), (_, part)) in m.cells {
        let slot = *slots.entry(part).or_insert_with(|| {
            palette.push(resolve(part));
            u8::try_from(palette.len() - 1).expect("the opera house uses few block states")
        });
        voxels.push((
            cell(x + ORIGIN_X),
            cell(y + GROUND),
            cell(z + ORIGIN_Z),
            slot,
        ));
    }
    PalettizedSchematic {
        width: MAX_X - MIN_X,
        length: MAX_Z - MIN_Z,
        palette,
        voxels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shell_fits_one_sphere() {
        for shell in &SHELLS {
            for half in split(shell) {
                for corner in [half.foot, half.tip, half.back] {
                    let r = corner.minus(half.centre).norm();
                    assert!((r - RADIUS).abs() < 1e-6, "corner {r} m from the centre");
                }
            }
        }
    }

    #[test]
    fn the_concert_hall_peaks_at_67_metres_above_the_sea() {
        let model = build();
        let top = model.voxels.iter().max_by_key(|v| v.1).unwrap();
        // The broadwalk stands about 3 m above the harbour.
        assert_eq!(i32::from(top.1) + 1 - GROUND + 3, 67);
        let x = f64::from(i32::from(top.0) - ORIGIN_X);
        let z = f64::from(i32::from(top.2) - ORIGIN_Z);
        assert!((x - SHELLS[A2].tip[0]).abs() < 3.0 && (z - SHELLS[A2].tip[2]).abs() < 3.0);
        assert!(model
            .voxels
            .iter()
            .all(|v| v.0 >= 0 && v.1 >= 0 && v.2 >= 0));
    }

    #[test]
    fn glass_and_infill_stand_on_the_deck() {
        let m = model();
        let stray: Vec<_> = m
            .cells
            .iter()
            .filter(|&(_, &(layer, _))| matches!(layer, Layer::Infill | Layer::Mullion))
            .filter(|&(&(x, y, z), _)| {
                y <= DECK || !inside(PODIUM, f64::from(x) + 0.5, f64::from(z) + 0.5)
            })
            .map(|(k, _)| *k)
            .collect();
        assert!(
            stray.is_empty(),
            "{} cells off the deck: {:?}",
            stray.len(),
            &stray[..stray.len().min(12)]
        );
    }
}
