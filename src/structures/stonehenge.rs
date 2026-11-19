//! Stonehenge as it stands today at one block per metre, set into the turf around it.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use super::schematic::PalettizedSchematic;
use crate::block_definitions::*;

/// Model block whose north-west corner is the centre of the sarsen circle.
pub const CENTRE_X: i32 = 64;
pub const CENTRE_Z: i32 = 107;
/// Model layer just above the turf; the two below are the ground the stones are set in.
pub const GROUND: i32 = 2;
const SIDE_X: i32 = 186;
const SIDE_Z: i32 = 172;

/// Stones mapped as points inside the henge, which would come out as loose boulders. The stones
/// mapped as outlines stay, drawn as bare rock under the model, which turfs over what shows.
pub const MAPPED: &[(&str, u64)] = &[
    ("node", 11_959_515_807),
    ("node", 11_959_515_808),
    ("node", 11_959_515_809),
    ("node", 11_959_515_810),
];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Sarsen,
    /// A sarsen lying where it fell, half sunk in the turf.
    Fallen,
    Blue,
    FallenBlue,
    /// A bluestone broken off at the turf.
    Stump,
    Altar,
    /// The Heel Stone, leaning toward the circle.
    Heel,
}
use Kind::*;

/// One row of `STONES`.
type StoneRow = (&'static str, Kind, i32, i32, i32, i32, i32, i32);

/// Stone number, kind, blocks above the turf, centre in decimetres east and south of the circle's
/// centre, length, width and the bearing of the length in degrees from east toward south. Plan from
/// the published stone plan fitted to the Environment Agency LiDAR, heights from that LiDAR.
#[rustfmt::skip]
const STONES: &[StoneRow] = &[
    ("1", Sarsen, 4, 128, -86, 21, 12, 57),
    ("2", Sarsen, 4, 143, -59, 23, 15, 74),
    ("3", Sarsen, 4, 152, -27, 20, 13, 71),
    ("4", Sarsen, 4, 154, 3, 21, 10, 94),
    ("5", Sarsen, 4, 150, 34, 20, 10, 102),
    ("6", Sarsen, 4, 138, 67, 23, 11, 118),
    ("7", Sarsen, 4, 122, 94, 20, 11, 127),
    ("8", Fallen, 1, 117, 136, 20, 15, 52),
    ("9a", Fallen, 1, 77, 143, 19, 15, 164),
    ("9b", Fallen, 1, 89, 161, 23, 15, 32),
    ("10", Sarsen, 4, 42, 145, 24, 10, 172),
    ("11", Sarsen, 3, 13, 155, 12, 8, 172),
    ("12", Fallen, 1, -20, 168, 50, 18, 81),
    ("14", Fallen, 1, -70, 114, 36, 19, 135),
    ("15", Fallen, 1, -84, 82, 17, 14, 44),
    ("16", Sarsen, 4, -131, 85, 21, 16, 53),
    ("19", Fallen, 1, -184, -7, 26, 21, 19),
    ("21", Sarsen, 4, -139, -67, 15, 12, 98),
    ("22", Sarsen, 4, -123, -95, 18, 10, 134),
    ("23", Sarsen, 4, -99, -115, 21, 7, 142),
    ("25", Fallen, 1, -64, -154, 42, 19, 179),
    ("26", Fallen, 1, -15, -154, 18, 14, 178),
    ("27", Sarsen, 4, 20, -151, 21, 10, 9),
    ("28", Sarsen, 4, 50, -146, 23, 13, 23),
    ("29", Sarsen, 4, 81, -131, 21, 13, 25),
    ("30", Sarsen, 4, 106, -113, 23, 15, 46),
    ("31", Blue, 2, 98, -65, 13, 5, 56),
    ("32", FallenBlue, 1, 111, -17, 16, 10, 6),
    ("32c", Stump, 0, 120, 23, 8, 3, 108),
    ("32d", Stump, 0, 118, 34, 5, 4, 42),
    ("32e", Blue, 2, 115, 42, 6, 5, 99),
    ("33", Blue, 2, 111, 53, 6, 4, 116),
    ("33e", Stump, 0, 77, 97, 11, 3, 34),
    ("33f", Stump, 0, 66, 100, 8, 4, 146),
    ("34", Stump, 0, 53, 106, 7, 6, 74),
    ("35a", FallenBlue, 1, 43, 112, 5, 4, 44),
    ("35b", FallenBlue, 1, 58, 141, 7, 4, 125),
    ("36", FallenBlue, 1, -18, 125, 15, 6, 100),
    ("37", Blue, 1, -31, 107, 11, 4, 21),
    ("38", FallenBlue, 1, -55, 100, 18, 12, 32),
    ("39", FallenBlue, 1, -60, 86, 19, 12, 89),
    ("40", FallenBlue, 1, -69, 80, 24, 10, 94),
    ("40c", Stump, 0, -101, 65, 6, 3, 43),
    ("40g", Stump, 0, -120, 32, 6, 5, 62),
    ("41", FallenBlue, 1, -129, 18, 19, 6, 119),
    ("41d", Stump, 0, -113, -32, 5, 2, 114),
    ("42", FallenBlue, 1, -94, -67, 15, 8, 82),
    ("42c", Stump, 0, -78, -86, 7, 5, 149),
    ("43", FallenBlue, 1, -67, -97, 12, 5, 72),
    ("44", FallenBlue, 1, -50, -71, 12, 5, 132),
    ("45", FallenBlue, 1, -38, -121, 21, 7, 75),
    ("46", Blue, 2, 41, -114, 8, 5, 23),
    ("47", Blue, 2, 57, -108, 9, 6, 24),
    ("48", FallenBlue, 1, 65, -93, 11, 8, 46),
    ("49", Blue, 2, 81, -86, 10, 4, 39),
    ("51", Sarsen, 5, 86, 23, 24, 14, 131),
    ("52", Sarsen, 5, 67, 42, 25, 15, 140),
    ("53", Sarsen, 5, 19, 71, 21, 11, 154),
    ("54", Sarsen, 5, -5, 83, 22, 15, 177),
    ("55a", Fallen, 2, -54, 58, 34, 24, 129),
    ("55b", Fallen, 2, -29, 34, 34, 23, 151),
    ("56", Sarsen, 7, -74, 41, 21, 9, 47),
    ("57", Sarsen, 5, -82, -12, 24, 10, 116),
    ("58", Sarsen, 5, -68, -36, 25, 10, 125),
    ("59a", Fallen, 1, -24, -64, 37, 22, 44),
    ("59b", Fallen, 2, -7, -42, 21, 16, 12),
    ("59c", Fallen, 1, 2, -29, 21, 11, 147),
    ("60", Sarsen, 5, -10, -88, 24, 13, 149),
    ("61", Blue, 2, 76, 8, 7, 5, 133),
    ("61a", Stump, 0, 55, 24, 3, 3, 45),
    ("62", Blue, 2, 44, 41, 6, 5, 128),
    ("63", Blue, 2, 30, 50, 8, 5, 146),
    ("64", Blue, 1, -2, 59, 4, 4, 12),
    ("65", Blue, 1, -18, 55, 9, 4, 24),
    ("67", Blue, 1, -49, 21, 21, 15, 88),
    ("68", Blue, 2, -58, 20, 12, 7, 102),
    ("69", Blue, 2, -62, -11, 7, 6, 111),
    ("70", Blue, 2, -54, -25, 8, 4, 114),
    ("70a", Stump, 0, -47, -41, 5, 3, 6),
    ("70b", Stump, 0, -36, -50, 4, 4, 42),
    ("72", FallenBlue, 1, -10, -74, 13, 8, 142),
    ("80", Altar, 1, -28, 15, 24, 11, 15),
    ("91", Fallen, 1, 403, 202, 27, 17, 28),
    ("93", Sarsen, 2, -381, -191, 13, 11, 60),
    ("95", Fallen, 1, 351, -276, 62, 18, 152),
    ("96", Heel, 4, 609, -495, 30, 24, 40),
    ("120", Fallen, 1, -178, -26, 17, 14, 162),
    ("127", Fallen, 1, 1, -159, 25, 11, 96),
    ("150", FallenBlue, 1, 115, -29, 14, 5, 124),
    ("156", Fallen, 2, -34, -1, 28, 9, 116),
    ("160a", Fallen, 1, 9, -52, 13, 13, 107),
    ("160b", Fallen, 1, 26, -38, 15, 11, 86),
    ("160c", Fallen, 1, 38, -18, 13, 12, 175),
];

/// Lintels still on the outer circle, each over the two uprights it spans.
const CIRCLE_LINTELS: [(&str, &str); 6] = [
    ("29", "30"),
    ("30", "1"),
    ("1", "2"),
    ("4", "5"),
    ("6", "7"),
    ("21", "22"),
];
/// Trilithons that still carry their lintel.
const TRILITHON_LINTELS: [(&str, &str); 3] = [("51", "52"), ("53", "54"), ("57", "58")];
/// Radius of the sarsen circle's centre line.
const CIRCLE_R: f64 = 15.45;

// The henge bank and ditch every 3 degrees from north, reaching from radius `36 + v / 4` m to the
// next value's, measured off the LiDAR terrain; 0 where it finds none or the visitors' path crosses.
#[rustfmt::skip]
const BANK_IN: [u8; 120] = [31, 33, 39, 0, 35, 33, 33, 33, 33, 33, 31, 33, 0, 0, 0, 0, 0, 0, 43, 25, 27, 33, 41, 35, 33, 29, 29, 33, 33, 37, 39, 37, 33, 31, 31, 31, 33, 35, 37, 35, 33, 33, 33, 33, 33, 35, 35, 37, 37, 35, 33, 35, 27, 15, 15, 15, 21, 41, 0, 0, 35, 35, 37, 39, 41, 43, 41, 43, 43, 39, 35, 35, 35, 35, 37, 37, 37, 37, 37, 0, 0, 0, 0, 0, 39, 39, 39, 39, 39, 39, 37, 37, 41, 0, 39, 39, 39, 39, 37, 41, 0, 0, 0, 39, 27, 31, 33, 29, 23, 29, 35, 0, 0, 0, 0, 0, 39, 37, 29, 31];
#[rustfmt::skip]
const BANK_OUT: [u8; 120] = [57, 57, 55, 0, 49, 53, 55, 55, 55, 53, 55, 55, 0, 0, 0, 0, 0, 0, 57, 57, 57, 53, 51, 53, 53, 55, 55, 53, 51, 49, 49, 51, 55, 55, 55, 55, 55, 51, 53, 55, 55, 55, 53, 55, 55, 57, 59, 55, 57, 59, 59, 59, 59, 57, 57, 59, 61, 57, 0, 0, 59, 59, 59, 59, 57, 57, 55, 57, 59, 61, 63, 63, 63, 61, 61, 61, 61, 61, 61, 0, 0, 0, 0, 0, 61, 61, 61, 61, 61, 61, 61, 61, 59, 0, 59, 61, 61, 61, 63, 59, 0, 0, 0, 63, 63, 63, 63, 63, 61, 59, 59, 0, 0, 0, 0, 0, 63, 63, 59, 57];
#[rustfmt::skip]
const DITCH_IN: [u8; 120] = [63, 63, 63, 61, 63, 63, 63, 63, 63, 63, 59, 59, 61, 0, 0, 0, 0, 0, 63, 63, 61, 61, 61, 59, 59, 61, 61, 61, 61, 59, 57, 59, 61, 61, 61, 61, 61, 61, 61, 61, 61, 59, 57, 59, 61, 63, 65, 63, 63, 63, 65, 63, 63, 63, 63, 65, 65, 65, 0, 0, 67, 67, 65, 65, 65, 63, 63, 65, 67, 69, 69, 69, 69, 69, 69, 69, 69, 71, 71, 0, 0, 0, 0, 0, 69, 69, 67, 67, 69, 69, 69, 67, 67, 65, 67, 67, 67, 69, 69, 67, 63, 61, 69, 71, 69, 69, 69, 69, 69, 67, 65, 0, 0, 0, 0, 0, 0, 0, 65, 63];
#[rustfmt::skip]
const DITCH_OUT: [u8; 120] = [81, 79, 75, 73, 73, 75, 75, 73, 73, 71, 73, 73, 75, 0, 0, 0, 0, 0, 79, 77, 75, 77, 75, 75, 75, 75, 77, 77, 77, 77, 77, 77, 77, 77, 81, 79, 75, 75, 75, 79, 79, 77, 75, 75, 75, 79, 79, 79, 77, 79, 79, 79, 81, 79, 77, 77, 77, 77, 0, 0, 79, 79, 79, 79, 77, 75, 75, 75, 79, 83, 81, 83, 83, 83, 85, 83, 79, 79, 77, 0, 0, 0, 0, 0, 83, 81, 79, 79, 79, 79, 79, 77, 77, 77, 79, 79, 81, 81, 83, 83, 89, 89, 85, 85, 85, 85, 83, 81, 79, 81, 81, 0, 0, 0, 0, 0, 0, 0, 83, 85];

/// The Avenue leaves along the solstice axis, in degrees from north.
const AVENUE_BEARING: f64 = 49.5;
/// Its banks past the old road, as (from, to) along the axis and (left, right) across it, in m.
const AVENUE_BANKS: [(f64, f64, f64, f64); 2] =
    [(104.0, 150.0, -10.0, -8.0), (104.0, 150.0, 5.5, 7.5)];
/// Its ditches between the henge and the Heel Stone.
const AVENUE_DITCHES: [(f64, f64, f64, f64); 2] =
    [(62.0, 74.0, -13.0, -11.0), (62.0, 74.0, 10.0, 12.0)];

#[derive(Default)]
struct Henge {
    cells: BTreeMap<(i32, i32, i32), Block>,
}

impl Henge {
    /// `x`, `z` in metres from the circle's centre, `y` in layers above the turf.
    fn set(&mut self, x: i32, y: i32, z: i32, block: Block) {
        self.cells
            .insert((CENTRE_X + x, GROUND + y, CENTRE_Z + z), block);
    }

    fn has(&self, x: i32, y: i32, z: i32) -> bool {
        self.cells
            .contains_key(&(CENTRE_X + x, GROUND + y, CENTRE_Z + z))
    }

    /// A stone ending at layer `y` lies over the earthwork: no tuft of the bank on top of it.
    fn cap(&mut self, x: i32, y: i32, z: i32) {
        for y in y..y + 2 {
            let key = (CENTRE_X + x, GROUND + y, CENTRE_Z + z);
            if matches!(self.cells.get(&key), Some(&GRASS | &AIR)) {
                self.cells.remove(&key);
            }
        }
    }
}

fn noise(x: i32, y: i32, z: i32, salt: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1)
        ^ (y as u32).wrapping_mul(0x85EB_CA77)
        ^ (z as u32).wrapping_mul(0xC2B2_AE3D)
        ^ salt.wrapping_mul(0x27D4_EB2F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

/// Weathered grey sarsen: lichen on the tops, mossier where it lies in the grass.
fn sarsen(x: i32, y: i32, z: i32, top: bool, lying: bool) -> Block {
    let n = noise(x, y, z, 1) % 100;
    let moss = if top {
        30
    } else if lying {
        18
    } else {
        12
    };
    if n < moss {
        MOSSY_COBBLESTONE
    } else if n < moss + 24 {
        TUFF
    } else if n < moss + 40 {
        STONE
    } else {
        ANDESITE
    }
}

/// The darker, bluer dolerites and rhyolites of the bluestones.
fn bluestone(x: i32, y: i32, z: i32) -> Block {
    match noise(x, y, z, 2) % 100 {
        0..=39 => CYAN_TERRACOTTA,
        40..=54 => LIGHT_BLUE_TERRACOTTA,
        55..=74 => COBBLED_DEEPSLATE,
        _ => DEEPSLATE,
    }
}

/// The greenish sandstone of the Altar Stone.
fn altar(x: i32, y: i32, z: i32) -> Block {
    if noise(x, y, z, 3) % 100 < 30 {
        GREEN_STAINED_HARDENED_CLAY
    } else {
        TUFF
    }
}

/// Cells under a stone of `len` by `wid` metres, a rounded rectangle; at least the cell at its centre.
/// Each comes with how far along the length it lies, from -1 to 1.
fn footprint(cx: f64, cz: f64, len: f64, wid: f64, deg: f64) -> Vec<(i32, i32, f64)> {
    let (s, c) = deg.to_radians().sin_cos();
    let reach = (len.max(wid) / 2.0).ceil() as i32 + 1;
    let (bx, bz) = (cx.floor() as i32, cz.floor() as i32);
    let mut cells = Vec::new();
    for z in bz - reach..=bz + reach {
        for x in bx - reach..=bx + reach {
            let (px, pz) = (f64::from(x) + 0.5 - cx, f64::from(z) + 0.5 - cz);
            let u = (px * c + pz * s) / (len / 2.0);
            let v = (pz * c - px * s) / (wid / 2.0);
            if u.powi(4) + v.powi(4) <= 1.0 {
                cells.push((x, z, u));
            }
        }
    }
    if cells.is_empty() {
        cells.push((bx, bz, 0.0));
    }
    cells
}

struct Stone {
    name: &'static str,
    kind: Kind,
    height: i32,
    centre: (f64, f64),
    /// Length, width and bearing.
    plan: (f64, f64, f64),
    cells: Vec<(i32, i32, f64)>,
}

fn stones() -> Vec<Stone> {
    STONES
        .iter()
        .map(|&(name, kind, height, cx, cz, len, wid, deg)| {
            let (cx, cz) = (f64::from(cx) / 10.0, f64::from(cz) / 10.0);
            let plan = (f64::from(len) / 10.0, f64::from(wid) / 10.0, f64::from(deg));
            Stone {
                name,
                kind,
                height,
                centre: (cx, cz),
                plan,
                cells: footprint(cx, cz, plan.0, plan.1, plan.2),
            }
        })
        .collect()
}

pub fn build() -> PalettizedSchematic {
    let mut h = Henge::default();
    earthworks(&mut h);
    let stones = stones();
    let bearers: Vec<&str> = CIRCLE_LINTELS
        .iter()
        .chain(TRILITHON_LINTELS.iter())
        .flat_map(|&(a, b)| [a, b])
        .collect();
    for (i, stone) in stones.iter().enumerate() {
        let flat = bearers.contains(&stone.name);
        place_stone(&mut h, stone, i as u32, flat);
    }
    let find = |name: &str| stones.iter().find(|s| s.name == name).unwrap();
    for &(a, b) in &CIRCLE_LINTELS {
        circle_lintel(&mut h, find(a), find(b));
    }
    for &(a, b) in &TRILITHON_LINTELS {
        trilithon_lintel(&mut h, find(a), find(b));
    }
    turf(&mut h, &stones);
    finish(h)
}

fn place_stone(h: &mut Henge, stone: &Stone, i: u32, flat: bool) {
    for &(x, z, u) in &stone.cells {
        match stone.kind {
            Sarsen | Blue => {
                // Free-standing tops are rough: their ends drop a block here and there.
                let rough = !flat && stone.cells.len() > 2 && u.abs() > 0.5;
                let top = stone.height - i32::from(rough && noise(x, 0, z, i).is_multiple_of(2));
                for y in -2..top {
                    let block = if stone.kind == Blue {
                        bluestone(x, y, z)
                    } else {
                        sarsen(x, y, z, y == top - 1, false)
                    };
                    h.set(x, y, z, block);
                }
                h.cap(x, top, z);
            }
            Fallen | FallenBlue | Altar => {
                // Half sunk; the thick ones show a second course along their middle.
                let top = if stone.height > 1 && u.abs() < 0.6 {
                    2
                } else {
                    1
                };
                for y in -1..top {
                    let block = match stone.kind {
                        FallenBlue => bluestone(x, y, z),
                        Altar => altar(x, y, z),
                        _ => sarsen(x, y, z, y == top - 1, true),
                    };
                    h.set(x, y, z, block);
                }
                h.cap(x, top, z);
            }
            Stump => h.set(x, -1, z, bluestone(x, -1, z)),
            Heel => {}
        }
    }
    if stone.kind == Heel {
        heel_stone(h, stone);
    }
    // The tenon still stands proud on top of the Great Trilithon's upright.
    if stone.name == "56" {
        let (x, z) = (stone.centre.0.floor() as i32, stone.centre.1.floor() as i32);
        h.set(x, stone.height, z, SMOOTH_STONE_SLAB);
    }
}

/// Unworked and leaning toward the circle, so each course sits a little further in.
fn heel_stone(h: &mut Henge, stone: &Stone) {
    let (cx, cz) = stone.centre;
    let lean = (-cx / cx.hypot(cz), -cz / cx.hypot(cz));
    let (len, wid, deg) = stone.plan;
    for y in -2..stone.height {
        let k = f64::from(y.max(0));
        let (ox, oz) = (cx + lean.0 * 0.5 * k, cz + lean.1 * 0.5 * k);
        let shrink = 1.0 - 0.12 * k;
        for (x, z, _) in footprint(ox, oz, len * shrink, wid * shrink, deg) {
            h.set(x, y, z, sarsen(x, y, z, y == stone.height - 1, false));
        }
    }
}

/// Half the width of a lintel's band of cells.
const LINTEL_HALF: f64 = 0.7;

/// An arc of the lintel ring from one upright's middle to the next, on their tops.
fn circle_lintel(h: &mut Henge, a: &Stone, b: &Stone) {
    let y = a.height.max(b.height);
    let ta = a.centre.1.atan2(a.centre.0);
    let turn = |t: f64| (t - ta + PI).rem_euclid(2.0 * PI) - PI;
    let span = turn(b.centre.1.atan2(b.centre.0));
    let ext = 0.45 / CIRCLE_R;
    let (t0, t1) = (span.min(0.0) - ext, span.max(0.0) + ext);
    let reach = CIRCLE_R.ceil() as i32 + 1;
    for z in -reach..=reach {
        for x in -reach..=reach {
            let (px, pz) = (f64::from(x) + 0.5, f64::from(z) + 0.5);
            let t = turn(pz.atan2(px));
            if (px.hypot(pz) - CIRCLE_R).abs() <= LINTEL_HALF && (t0..=t1).contains(&t) {
                h.set(x, y, z, sarsen(x, y, z, true, false));
            }
        }
    }
}

/// A trilithon's lintel, overhanging both uprights by about a metre.
fn trilithon_lintel(h: &mut Henge, a: &Stone, b: &Stone) {
    let y = a.height.max(b.height);
    let (dx, dz) = (b.centre.0 - a.centre.0, b.centre.1 - a.centre.1);
    let len = dx.hypot(dz);
    let (ux, uz) = (dx / len, dz / len);
    let (bx, bz) = (a.centre.0.floor() as i32, a.centre.1.floor() as i32);
    let reach = len.ceil() as i32 + 3;
    for z in bz - reach..=bz + reach {
        for x in bx - reach..=bx + reach {
            let (px, pz) = (
                f64::from(x) + 0.5 - a.centre.0,
                f64::from(z) + 0.5 - a.centre.1,
            );
            let along = px * ux + pz * uz;
            if (-1.0..=len + 1.0).contains(&along) && (px * uz - pz * ux).abs() <= LINTEL_HALF {
                h.set(x, y, z, sarsen(x, y, z, true, false));
            }
        }
    }
}

/// Turf over the ground beside each stone, where the map draws its outline as bare rock.
fn turf(h: &mut Henge, stones: &[Stone]) {
    for stone in stones {
        // The Heel Stone's walkway passes close by, and its outline fits the model.
        let reach: i32 = if stone.kind == Heel { 1 } else { 2 };
        for &(sx, sz, _) in &stone.cells {
            for z in sz - reach..=sz + reach {
                for x in sx - reach..=sx + reach {
                    let far = (x - sx).pow(2) + (z - sz).pow(2) > reach * reach + 1;
                    if far || (-2..=9).any(|y| h.has(x, y, z)) {
                        continue;
                    }
                    h.set(x, -1, z, GRASS_BLOCK);
                    h.set(x, 0, z, tuft(x, z));
                }
            }
        }
    }
}

/// Radius band of one of the ring tables at `bearing` degrees, None where there is no earthwork.
fn ring_band(inner: &[u8; 120], outer: &[u8; 120], bearing: f64) -> Option<(f64, f64)> {
    let pos = bearing / 3.0;
    let i = pos.floor() as usize % 120;
    let j = (i + 1) % 120;
    let f = pos - pos.floor();
    let metres = |v: u8| 36.0 + f64::from(v) / 4.0;
    match (inner[i], inner[j]) {
        (0, 0) => None,
        (0, _) | (_, 0) => {
            let k = if f < 0.5 { i } else { j };
            (inner[k] != 0).then(|| (metres(inner[k]), metres(outer[k])))
        }
        _ => Some((
            metres(inner[i]) * (1.0 - f) + metres(inner[j]) * f,
            metres(outer[i]) * (1.0 - f) + metres(outer[j]) * f,
        )),
    }
}

fn earthworks(h: &mut Henge) {
    let (s, c) = AVENUE_BEARING.to_radians().sin_cos();
    for z in -CENTRE_Z..SIDE_Z - CENTRE_Z {
        for x in -CENTRE_X..SIDE_X - CENTRE_X {
            let (px, pz) = (f64::from(x) + 0.5, f64::from(z) + 0.5);
            let r = px.hypot(pz);
            let bearing = px.atan2(-pz).to_degrees().rem_euclid(360.0);
            let within = |band: Option<(f64, f64)>| band.is_some_and(|(a, b)| (a..=b).contains(&r));
            let (along, across) = (px * s - pz * c, px * c + pz * s);
            let in_strips = |strips: &[(f64, f64, f64, f64)]| {
                strips.iter().any(|&(a0, a1, c0, c1)| {
                    (a0..=a1).contains(&along) && (c0..=c1).contains(&across)
                })
            };
            if within(ring_band(&BANK_IN, &BANK_OUT, bearing)) || in_strips(&AVENUE_BANKS) {
                bank(h, x, z);
            } else if within(ring_band(&DITCH_IN, &DITCH_OUT, bearing))
                || in_strips(&AVENUE_DITCHES)
            {
                ditch(h, x, z);
            }
        }
    }
}

fn tuft(x: i32, z: i32) -> Block {
    if noise(x, 7, z, 4) % 100 < 55 {
        GRASS
    } else {
        AIR
    }
}

/// A block of turf raised on the old surface.
fn bank(h: &mut Henge, x: i32, z: i32) {
    h.set(x, -1, z, DIRT);
    h.set(x, 0, z, GRASS_BLOCK);
    h.set(x, 1, z, tuft(x, z));
}

/// The turf a block lower, with nothing left standing above it.
fn ditch(h: &mut Henge, x: i32, z: i32) {
    h.set(x, -2, z, GRASS_BLOCK);
    h.set(x, -1, z, tuft(x, z));
    h.set(x, 0, z, AIR);
}

fn finish(h: Henge) -> PalettizedSchematic {
    let mut palette: Vec<BlockWithProperties> = Vec::new();
    let mut slots: BTreeMap<Block, u8> = BTreeMap::new();
    let mut voxels = Vec::with_capacity(h.cells.len());
    for (&(x, y, z), &block) in &h.cells {
        debug_assert!((0..SIDE_X).contains(&x) && (0..SIDE_Z).contains(&z) && y >= 0);
        let slot = *slots.entry(block).or_insert_with(|| {
            palette.push(BlockWithProperties::simple(block));
            u8::try_from(palette.len() - 1).expect("Stonehenge uses few blocks")
        });
        voxels.push((x as i16, y as i16, z as i16, slot));
    }
    PalettizedSchematic {
        width: SIDE_X,
        length: SIDE_Z,
        palette,
        voxels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top_at(model: &PalettizedSchematic, x: i32, z: i32) -> Option<i32> {
        model
            .voxels
            .iter()
            .filter(|v| i32::from(v.0) == x && i32::from(v.2) == z)
            .filter(|v| model.palette[v.3 as usize].block != AIR)
            .map(|v| i32::from(v.1) - GROUND + 1)
            .max()
    }

    #[test]
    fn the_great_trilithon_upright_is_the_tallest_stone() {
        let model = build();
        let stone = STONES.iter().find(|s| s.0 == "56").unwrap();
        let (x, z) = (
            CENTRE_X + (f64::from(stone.3) / 10.0).floor() as i32,
            CENTRE_Z + (f64::from(stone.4) / 10.0).floor() as i32,
        );
        // Seven courses and its tenon, over every lintel.
        assert_eq!(top_at(&model, x, z), Some(8));
        let top = model.voxels.iter().map(|v| i32::from(v.1)).max().unwrap();
        assert_eq!(top - GROUND + 1, 8);
    }

    #[test]
    fn the_circle_lintels_rest_on_their_uprights() {
        let model = build();
        for (name, _, height, cx, cz, ..) in STONES.iter().filter(|s| s.1 == Sarsen) {
            let (x, z) = (
                CENTRE_X + (f64::from(*cx) / 10.0).floor() as i32,
                CENTRE_Z + (f64::from(*cz) / 10.0).floor() as i32,
            );
            let carried = CIRCLE_LINTELS
                .iter()
                .chain(TRILITHON_LINTELS.iter())
                .any(|&(a, b)| a == *name || b == *name);
            let top = top_at(&model, x, z).unwrap();
            if carried {
                assert_eq!(top, height + 1, "stone {name} carries a lintel");
            } else if *name != "56" {
                assert!(top <= *height, "stone {name}");
            }
        }
    }

    #[test]
    fn the_bank_stands_inside_the_ditch() {
        let model = build();
        // Due west of the centre, the bank at 47 m and the ditch at 54 m.
        let bank = model
            .voxels
            .iter()
            .any(|v| (v.0, v.1, v.2) == ((CENTRE_X - 48) as i16, GROUND as i16, CENTRE_Z as i16));
        let ditch = model.voxels.iter().any(|v| {
            (v.0, v.1, v.2) == ((CENTRE_X - 55) as i16, (GROUND - 2) as i16, CENTRE_Z as i16)
                && model.palette[v.3 as usize].block == GRASS_BLOCK
        });
        assert!(bank && ditch);
    }
}
