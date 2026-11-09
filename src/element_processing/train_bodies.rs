//! What trains, metros and trams look like: each carriage a handful of stretched blocks
//! drawn by block displays, in its operator's colours. The pack summons a carriage from
//! its body function when it comes into view; `trains.rs` decides which carriages make a
//! train.

use crate::element_processing::ship_bodies::Vessel;
use std::fmt::Write;

const GLASS: &str = "minecraft:tinted_glass";
const BOGIE: &str = "minecraft:black_concrete";
const UNDER: &str = "minecraft:gray_concrete";
const BLACK: &str = "minecraft:black_concrete";
const HEAD: &str = "minecraft:white_concrete";
const TAIL: &str = "minecraft:red_concrete";
const SIGN: &str = "minecraft:orange_concrete";
const STEEL: &str = "minecraft:gray_concrete";

/// What a route's trains are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Tram,
    Metro,
    SBahn,
    Regional,
    DoubleDeck,
    Intercity,
    Freight,
}

/// One carriage. Cabs face +z when `front`, -z otherwise.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Coach {
    TramCab {
        front: bool,
    },
    TramMid,
    TramShort,
    MetroCab {
        front: bool,
    },
    MetroMid,
    SBahnCab {
        front: bool,
    },
    SBahnMid,
    RegioCab {
        front: bool,
    },
    RegioMid,
    DeckCab {
        front: bool,
    },
    DeckMid,
    Loco,
    IceCab {
        front: bool,
    },
    IceMid,
    Container(u8),
    Tank(u8),
    Boxcar(u8),
    /// A boat, `low` with its wheelhouse let down for low bridges.
    Ship {
        vessel: Vessel,
        low: bool,
    },
}

/// A train's colours.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Livery {
    pub(crate) body: &'static str,
    pub(crate) lower: &'static str,
    pub(crate) stripe: &'static str,
    pub(crate) door: &'static str,
    pub(crate) roof: &'static str,
    pub(crate) name: &'static str,
    /// Picks the colours of a freight train's wagons.
    seed: u8,
}

impl Livery {
    pub(crate) const fn new(
        name: &'static str,
        body: &'static str,
        lower: &'static str,
        stripe: &'static str,
        door: &'static str,
        roof: &'static str,
    ) -> Self {
        livery(name, body, lower, stripe, door, roof)
    }
}

const fn livery(
    name: &'static str,
    body: &'static str,
    lower: &'static str,
    stripe: &'static str,
    door: &'static str,
    roof: &'static str,
) -> Livery {
    Livery {
        body,
        lower,
        stripe,
        door,
        roof,
        name,
        seed: 0,
    }
}

const LIGHT_GREY: &str = "minecraft:light_gray_concrete";
const WHITE: &str = "minecraft:white_concrete";

const TRAMS: [Livery; 5] = [
    livery(
        "ty",
        "minecraft:yellow_concrete",
        LIGHT_GREY,
        "minecraft:yellow_concrete",
        "minecraft:yellow_concrete",
        LIGHT_GREY,
    ),
    livery(
        "twr",
        WHITE,
        "minecraft:red_concrete",
        "minecraft:red_concrete",
        "minecraft:red_concrete",
        LIGHT_GREY,
    ),
    livery(
        "trw",
        "minecraft:red_concrete",
        WHITE,
        WHITE,
        WHITE,
        LIGHT_GREY,
    ),
    livery(
        "tbw",
        WHITE,
        "minecraft:blue_concrete",
        "minecraft:blue_concrete",
        "minecraft:blue_concrete",
        LIGHT_GREY,
    ),
    livery(
        "tgw",
        WHITE,
        "minecraft:green_concrete",
        "minecraft:green_concrete",
        "minecraft:green_concrete",
        LIGHT_GREY,
    ),
];
const METROS: [Livery; 4] = [
    livery(
        "my",
        "minecraft:yellow_concrete",
        "minecraft:gray_concrete",
        "minecraft:yellow_concrete",
        "minecraft:yellow_concrete",
        LIGHT_GREY,
    ),
    livery(
        "mb",
        LIGHT_GREY,
        LIGHT_GREY,
        "minecraft:blue_concrete",
        "minecraft:blue_concrete",
        "minecraft:gray_concrete",
    ),
    livery(
        "mr",
        WHITE,
        WHITE,
        "minecraft:red_concrete",
        "minecraft:red_concrete",
        LIGHT_GREY,
    ),
    livery(
        "mo",
        LIGHT_GREY,
        LIGHT_GREY,
        "minecraft:orange_concrete",
        "minecraft:orange_concrete",
        "minecraft:gray_concrete",
    ),
];
const SBAHNS: [Livery; 3] = [
    livery(
        "sb",
        "minecraft:yellow_terracotta",
        "minecraft:red_concrete",
        "minecraft:yellow_terracotta",
        "minecraft:red_concrete",
        LIGHT_GREY,
    ),
    livery(
        "sd",
        WHITE,
        LIGHT_GREY,
        "minecraft:red_concrete",
        "minecraft:red_concrete",
        LIGHT_GREY,
    ),
    livery(
        "sg",
        WHITE,
        "minecraft:green_concrete",
        "minecraft:green_concrete",
        "minecraft:green_concrete",
        LIGHT_GREY,
    ),
];
const REGIONALS: [Livery; 4] = [
    livery(
        "rr",
        "minecraft:red_concrete",
        LIGHT_GREY,
        WHITE,
        LIGHT_GREY,
        LIGHT_GREY,
    ),
    livery(
        "rb",
        WHITE,
        "minecraft:blue_concrete",
        "minecraft:blue_concrete",
        "minecraft:blue_concrete",
        LIGHT_GREY,
    ),
    livery(
        "rg",
        WHITE,
        "minecraft:lime_concrete",
        "minecraft:green_concrete",
        "minecraft:lime_concrete",
        LIGHT_GREY,
    ),
    livery(
        "rk",
        "minecraft:yellow_concrete",
        "minecraft:gray_concrete",
        "minecraft:gray_concrete",
        "minecraft:gray_concrete",
        LIGHT_GREY,
    ),
];
const DECKS: [Livery; 2] = [
    livery(
        "dr",
        "minecraft:red_concrete",
        LIGHT_GREY,
        WHITE,
        LIGHT_GREY,
        LIGHT_GREY,
    ),
    livery(
        "db",
        WHITE,
        "minecraft:blue_concrete",
        "minecraft:blue_concrete",
        "minecraft:blue_concrete",
        LIGHT_GREY,
    ),
];
const INTERCITY: [Livery; 2] = [
    livery(
        "ice",
        WHITE,
        WHITE,
        "minecraft:red_concrete",
        LIGHT_GREY,
        LIGHT_GREY,
    ),
    livery(
        "icb",
        WHITE,
        WHITE,
        "minecraft:blue_concrete",
        LIGHT_GREY,
        LIGHT_GREY,
    ),
];
const LOCOS: [Livery; 5] = [
    livery(
        "fr",
        "minecraft:red_concrete",
        "minecraft:gray_concrete",
        WHITE,
        "minecraft:red_concrete",
        LIGHT_GREY,
    ),
    livery(
        "fb",
        "minecraft:blue_concrete",
        "minecraft:gray_concrete",
        WHITE,
        "minecraft:blue_concrete",
        LIGHT_GREY,
    ),
    livery(
        "fs",
        LIGHT_GREY,
        "minecraft:gray_concrete",
        "minecraft:red_concrete",
        LIGHT_GREY,
        "minecraft:gray_concrete",
    ),
    livery(
        "fg",
        "minecraft:green_concrete",
        "minecraft:gray_concrete",
        "minecraft:yellow_concrete",
        "minecraft:green_concrete",
        LIGHT_GREY,
    ),
    livery(
        "fk",
        "minecraft:black_concrete",
        "minecraft:gray_concrete",
        "minecraft:yellow_concrete",
        "minecraft:black_concrete",
        "minecraft:gray_concrete",
    ),
];
const CONTAINERS: [&str; 10] = [
    "minecraft:blue_concrete",
    "minecraft:red_terracotta",
    "minecraft:green_terracotta",
    "minecraft:orange_concrete",
    "minecraft:white_concrete",
    "minecraft:gray_concrete",
    "minecraft:brown_concrete",
    "minecraft:light_blue_terracotta",
    "minecraft:cyan_terracotta",
    "minecraft:yellow_terracotta",
];
const TANKS: [&str; 4] = [
    "minecraft:black_concrete",
    "minecraft:white_concrete",
    "minecraft:light_gray_concrete",
    "minecraft:gray_concrete",
];
const BOXCARS: [&str; 4] = [
    "minecraft:brown_terracotta",
    "minecraft:red_terracotta",
    "minecraft:gray_terracotta",
    "minecraft:brown_concrete",
];

/// The door functions of carriages nobody rides in.
pub const NO_DOORS: &str = "none";
/// A place to get on every so many metres of carriage.
const DOOR_SPACING: f32 = 5.5;
/// The box that takes a click on the carriage at each door, in metres.
const DOOR_BOX_WIDTH: f32 = 4.4;
const DOOR_BOX_HEIGHT: f32 = 3.8;
/// Height of a seated player's eyes over the track, in metres: in the window band.
const RIDER_EYE: f64 = 2.12;
/// How far over the block a player rides on their eyes are, in blocks at any scale.
const RIDER_EYE_OVER_SEAT: f64 = 1.02;

/// Where along a carriage people get on, in metres from its middle, how high their eyes
/// are over the rail and the box that takes a click there. A click nearly anywhere on a
/// carriage's side finds one. Freight wagons carry nobody.
pub struct Doors {
    pub spots: Vec<f32>,
    eye: f64,
    width: f32,
    height: f32,
    /// The seat is over the box rather than beside it, as on a boat's deck.
    deck: bool,
}

pub fn doors(coach: Coach) -> Doors {
    if let Coach::Ship { vessel, .. } = coach {
        let (spots, eye) = vessel.seats_m();
        let (_, beam) = vessel.size_m();
        return Doors {
            spots: spots.to_vec(),
            eye,
            width: (beam as f32).max(DOOR_BOX_WIDTH),
            height: eye as f32 + 0.5,
            deck: true,
        };
    }
    let spots = if matches!(
        coach,
        Coach::Container(_) | Coach::Tank(_) | Coach::Boxcar(_)
    ) {
        Vec::new()
    } else {
        let len = length_m(coach) as f32;
        let n = (len / DOOR_SPACING).ceil().max(1.0) as usize;
        let spacing = len / n as f32;
        (0..n)
            .map(|i| -len / 2.0 + spacing * (i as f32 + 0.5))
            .collect()
    };
    Doors {
        spots,
        eye: RIDER_EYE,
        width: DOOR_BOX_WIDTH,
        height: DOOR_BOX_HEIGHT,
        deck: false,
    }
}

/// The name of the functions that keep a carriage's doors with it, shared by carriages alike.
pub fn door_key(coach: Coach) -> String {
    match coach {
        Coach::Ship { vessel, .. } => format!("w_{}", vessel.name()),
        _ if doors(coach).spots.is_empty() => NO_DOORS.to_string(),
        _ => format!("m{}", (length_m(coach) * 10.0).round() as u32),
    }
}

/// The function that keeps a carriage's doors with it, run at the carriage's middle facing
/// along it, and the one that makes them. Each door is a box to click and a seat to ride.
pub fn door_functions(key: &str, doors: &Doors, size: f64) -> (String, String) {
    let spots = &doors.spots;
    if spots.is_empty() {
        return (String::new(), String::new());
    }
    let k = size as f32;
    let reach = spots.iter().fold(0f32, |a, d| a.max(d.abs())) * k + 12.0;
    let seat = (doors.eye * size - RIDER_EYE_OVER_SEAT).max(0.0) as f32;
    let deck = if doors.deck { ",\"arnis.tdeck\"" } else { "" };
    let mut keep = format!(
        "$execute unless entity @e[type=interaction,tag=arnis.ts$(c),distance=..{reach:.1}] run function arnis:trains/door_new/{key} {{c:$(c)}}\n\
         $scoreboard players operation @e[tag=arnis.ts$(c),distance=..{reach:.1}] arnis.te = #now arnis.train\n"
    );
    let mut make = String::new();
    for (i, d) in spots.iter().enumerate() {
        let at = d * k;
        let _ = writeln!(
            keep,
            "$tp @e[type=interaction,tag=arnis.ts$(c),tag=arnis.td{i},distance=..{reach:.1}] ^ ^ ^{at:.2}\n\
             $tp @e[type=block_display,tag=arnis.ts$(c),tag=arnis.td{i},distance=..{reach:.1}] ^ ^{seat:.2} ^{at:.2}"
        );
        let _ = writeln!(
            make,
            "$summon interaction ^ ^ ^{at:.2} {{Tags:[\"arnis.tp\",\"arnis.ts$(c)\",\"arnis.td{i}\"{deck}],width:{:.2}f,height:{:.2}f}}\n\
             $summon block_display ^ ^{seat:.2} ^{at:.2} {{Tags:[\"arnis.tp\",\"arnis.ts$(c)\",\"arnis.td{i}\",\"arnis.seat\"{deck}],teleport_duration:2}}",
            doors.width * k,
            doors.height * k,
        );
    }
    (keep, make)
}

/// Carriage length over its body, in metres.
pub fn length_m(coach: Coach) -> f64 {
    match coach {
        Coach::TramCab { .. } => 7.4,
        Coach::TramMid => 7.0,
        Coach::TramShort => 4.0,
        Coach::MetroCab { .. } | Coach::MetroMid => 16.0,
        Coach::SBahnCab { .. } | Coach::SBahnMid => 18.4,
        Coach::RegioCab { .. } | Coach::RegioMid => 20.0,
        Coach::DeckCab { .. } | Coach::DeckMid => 26.8,
        Coach::Loco => 19.5,
        Coach::IceCab { .. } => 25.8,
        Coach::IceMid => 24.8,
        Coach::Container(_) => 18.4,
        Coach::Tank(_) => 15.0,
        Coach::Boxcar(_) => 16.5,
        Coach::Ship { vessel, .. } => vessel.size_m().0,
    }
}

fn roll(seed: u64, k: u64) -> u64 {
    let mut h = seed ^ k.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^ (h >> 29)
}

/// The carriages of a route's trains, front to back, and the livery of its `k`th train.
pub fn compose(kind: Kind, seed: u64, short: bool) -> (Vec<Coach>, impl Fn(usize) -> Livery) {
    let pick = |list: &[Livery]| list[(roll(seed, 1) % list.len() as u64) as usize];
    let (coaches, base): (Vec<Coach>, Livery) = match kind {
        Kind::Tram => {
            let mut c = vec![Coach::TramCab { front: true }, Coach::TramShort];
            if !short {
                c.extend([Coach::TramMid, Coach::TramShort]);
            }
            c.push(Coach::TramCab { front: false });
            (c, pick(&TRAMS))
        }
        Kind::Metro | Kind::SBahn | Kind::Regional | Kind::Intercity => {
            let (front, back, mid, mids, list): (Coach, Coach, Coach, usize, &[Livery]) = match kind
            {
                Kind::Metro => (
                    Coach::MetroCab { front: true },
                    Coach::MetroCab { front: false },
                    Coach::MetroMid,
                    if short { 2 } else { 4 },
                    &METROS,
                ),
                Kind::SBahn => (
                    Coach::SBahnCab { front: true },
                    Coach::SBahnCab { front: false },
                    Coach::SBahnMid,
                    if short { 2 } else { 4 },
                    &SBAHNS,
                ),
                Kind::Regional => (
                    Coach::RegioCab { front: true },
                    Coach::RegioCab { front: false },
                    Coach::RegioMid,
                    if short { 1 } else { 2 },
                    &REGIONALS,
                ),
                _ => (
                    Coach::IceCab { front: true },
                    Coach::IceCab { front: false },
                    Coach::IceMid,
                    if short { 2 } else { 5 },
                    &INTERCITY,
                ),
            };
            let mut c = vec![front];
            c.extend(std::iter::repeat_n(mid, mids));
            c.push(back);
            (c, pick(list))
        }
        Kind::DoubleDeck => {
            let mut c = vec![Coach::Loco];
            c.extend(std::iter::repeat_n(
                Coach::DeckMid,
                if short { 1 } else { 3 },
            ));
            c.push(Coach::DeckCab { front: false });
            (c, pick(&DECKS))
        }
        Kind::Freight => {
            let mut c = vec![Coach::Loco];
            for k in 0..if short { 4 } else { 10 } {
                let r = roll(seed, 100 + k);
                let v = (r >> 8) as u8;
                c.push(match r % 100 {
                    0..=44 => Coach::Container(v),
                    45..=74 => Coach::Tank(v),
                    _ => Coach::Boxcar(v),
                });
            }
            (c, pick(&LOCOS))
        }
    };
    let freight = kind == Kind::Freight;
    let livery = move |k: usize| {
        if freight {
            let mut l = LOCOS[(roll(seed, 50 + k as u64) % LOCOS.len() as u64) as usize];
            l.seed = (roll(seed, 60 + k as u64) & 0xff) as u8;
            l
        } else {
            base
        }
    };
    (coaches, livery)
}

/// The name of a carriage's body function.
pub fn key(coach: Coach, livery: Livery) -> String {
    let c = match coach {
        Coach::TramCab { front } => format!("tram_cab_{}", if front { "f" } else { "b" }),
        Coach::TramMid => "tram_mid".to_string(),
        Coach::TramShort => "tram_short".to_string(),
        Coach::MetroCab { front } => format!("metro_cab_{}", if front { "f" } else { "b" }),
        Coach::MetroMid => "metro_mid".to_string(),
        Coach::SBahnCab { front } => format!("sbahn_cab_{}", if front { "f" } else { "b" }),
        Coach::SBahnMid => "sbahn_mid".to_string(),
        Coach::RegioCab { front } => format!("regio_cab_{}", if front { "f" } else { "b" }),
        Coach::RegioMid => "regio_mid".to_string(),
        Coach::DeckCab { front } => format!("deck_cab_{}", if front { "f" } else { "b" }),
        Coach::DeckMid => "deck_mid".to_string(),
        Coach::Loco => "loco".to_string(),
        Coach::IceCab { front } => format!("ice_cab_{}", if front { "f" } else { "b" }),
        Coach::IceMid => "ice_mid".to_string(),
        // Wagon colours come from the variant and the train, so both name the body.
        Coach::Container(v) => format!("container_{}", wagon(v, livery.seed) % 100),
        Coach::Tank(v) => format!("tank_{}", wagon(v, livery.seed) % TANKS.len()),
        Coach::Boxcar(v) => format!("boxcar_{}", wagon(v, livery.seed) % BOXCARS.len()),
        Coach::Ship { vessel, low } => {
            format!("ship_{}{}", vessel.name(), if low { "_low" } else { "" })
        }
    };
    match coach {
        Coach::Container(_) | Coach::Tank(_) | Coach::Boxcar(_) => c,
        _ => format!("{c}_{}", livery.name),
    }
}

fn wagon(v: u8, seed: u8) -> usize {
    (roll(v as u64, seed as u64) % 1000) as usize
}

/// One block of a carriage: its block, size and lowest corner in the carriage's frame (+z
/// ahead, the rail at 0, centred on the track), a tilt about x in radians, and whether it
/// glows.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub block: &'static str,
    pub size: [f32; 3],
    pub corner: [f32; 3],
    pub tilt: f32,
    pub glow: bool,
}

pub(crate) fn bx(block: &'static str, x: (f32, f32), y: (f32, f32), z: (f32, f32)) -> Part {
    let (z0, z1) = (z.0.min(z.1), z.0.max(z.1));
    Part {
        block,
        size: [x.1 - x.0, y.1 - y.0, z1 - z0],
        corner: [x.0, y.0, z0],
        tilt: 0.0,
        glow: false,
    }
}

fn lit(mut p: Part) -> Part {
    p.glow = true;
    p
}

/// A thin plate across the carriage from (y, z) `from` to `to`, as a raked windscreen.
fn plate(block: &'static str, x: (f32, f32), from: (f32, f32), to: (f32, f32), thick: f32) -> Part {
    let (dy, dz) = (to.0 - from.0, to.1 - from.1);
    Part {
        block,
        size: [x.1 - x.0, thick, dy.hypot(dz)],
        corner: [x.0, from.0, from.1],
        tilt: (-dy).atan2(dz),
        glow: false,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Nose {
    Flat,
    Tram,
    Streamlined,
}

struct Spec<'a> {
    len: f32,
    width: f32,
    floor: f32,
    top: f32,
    windows: &'a [(f32, f32)],
    doors: &'a [f32],
    door_w: f32,
    bogies: &'a [f32],
    bogie: (f32, f32),
    /// Height of the lower band of colour above the floor.
    band: Option<f32>,
    stripe: Option<(f32, f32)>,
    panto: Option<f32>,
    /// Which end the cab is at: 1 at +z, -1 at -z.
    cab: Option<f32>,
    nose: Nose,
}

/// A passenger carriage or a tram section.
fn coach(s: &Spec, l: Livery) -> Vec<Part> {
    let mut out = Vec::new();
    let (hw, half) = (s.width / 2.0, s.len / 2.0);
    let nose = if s.nose == Nose::Streamlined {
        3.6
    } else {
        0.0
    };
    let (z0, z1) = match s.cab {
        Some(d) if d > 0.0 => (-half + 0.15, half - nose),
        Some(_) => (-half + nose, half - 0.15),
        None => (-half + 0.15, half - 0.15),
    };
    for &bz in s.bogies {
        out.push(bx(
            BOGIE,
            (-hw + 0.35, hw - 0.35),
            (0.04, s.bogie.1),
            (bz - s.bogie.0 / 2.0, bz + s.bogie.0 / 2.0),
        ));
    }
    if s.floor > 0.7 {
        out.push(bx(
            UNDER,
            (-hw + 0.15, hw - 0.15),
            (0.55, s.floor),
            (z0 + 0.4, z1 - 0.4),
        ));
    }
    out.push(bx(l.body, (-hw, hw), (s.floor, s.top), (z0, z1)));
    if let Some(h) = s.band {
        out.push(bx(
            l.lower,
            (-hw - 0.01, hw + 0.01),
            (s.floor, s.floor + h),
            (z0, z1),
        ));
    }
    if let Some((a, b)) = s.stripe {
        out.push(bx(l.stripe, (-hw - 0.02, hw + 0.02), (a, b), (z0, z1)));
    }
    let (wz0, wz1) = (z0 + 0.7, z1 - 0.7);
    for &(a, b) in s.windows {
        out.push(bx(GLASS, (-hw - 0.03, hw + 0.03), (a, b), (wz0, wz1)));
    }
    let door_top = s.windows.first().map_or(s.top - 0.3, |w| w.1 + 0.12);
    for &dz in s.doors {
        out.push(bx(
            l.door,
            (-hw - 0.045, hw + 0.045),
            (s.floor + 0.05, door_top),
            (dz - s.door_w / 2.0, dz + s.door_w / 2.0),
        ));
    }
    out.push(bx(
        l.roof,
        (-hw + 0.12, hw - 0.12),
        (s.top, s.top + 0.14),
        (z0 + 0.25, z1 - 0.25),
    ));
    out.push(bx(
        l.roof,
        (-hw + 0.6, hw - 0.6),
        (s.top + 0.14, s.top + 0.26),
        (z0 + 0.5, z1 - 0.5),
    ));
    // Bellows to the next carriage at every end without a cab.
    for end in [-1.0f32, 1.0] {
        if s.cab == Some(end) {
            continue;
        }
        let face = end * half;
        out.push(bx(
            BLACK,
            (-hw + 0.5, hw - 0.5),
            (s.floor + 0.15, s.top - 0.2),
            (face - end * 0.3, face + end * 0.22),
        ));
    }
    if let Some(d) = s.cab {
        out.extend(cab(s, l, d, if d > 0.0 { z1 } else { z0 }));
    }
    if let Some(pz) = s.panto {
        out.extend(pantograph(s.top + 0.26, pz));
    }
    // An air-conditioning unit on the roof, clear of the pantograph.
    if s.len > 12.0 {
        let az = match s.panto {
            Some(pz) if pz > 0.0 => -half / 2.0,
            _ => half / 2.0 - 0.5,
        };
        out.push(bx(
            UNDER,
            (-0.75, 0.75),
            (s.top + 0.26, s.top + 0.55),
            (az - 1.3, az + 1.3),
        ));
    }
    out
}

/// The driving end at `face`, `d` pointing out of it.
fn cab(s: &Spec, l: Livery, d: f32, face: f32) -> Vec<Part> {
    let hw = s.width / 2.0;
    let half = s.len / 2.0;
    let mut out = Vec::new();
    let at = |a: f32, b: f32| (face + d * a, face + d * b);
    let lamp_y = |base: f32| (base, base + 0.2);
    match s.nose {
        Nose::Flat | Nose::Tram => {
            let low = if s.nose == Nose::Tram {
                s.floor + 0.95
            } else {
                s.windows.first().map_or(s.floor + 1.2, |w| w.0 - 0.1)
            };
            out.push(bx(
                GLASS,
                (-hw + 0.22, hw - 0.22),
                (low, s.top - 0.36),
                at(-0.02, 0.04),
            ));
            out.push(lit(bx(
                SIGN,
                (-hw + 0.55, hw - 0.55),
                (s.top - 0.31, s.top - 0.1),
                at(-0.02, 0.04),
            )));
            let lamps = if s.nose == Nose::Tram {
                // The bumper the lamps sit in.
                out.push(bx(
                    l.lower,
                    (-hw + 0.08, hw - 0.08),
                    (s.floor.max(0.3), s.floor + 0.8),
                    at(0.0, 0.14),
                ));
                lamp_y(s.floor + 0.45)
            } else {
                out.push(bx(
                    BLACK,
                    (-hw + 0.3, hw - 0.3),
                    (0.55, s.floor + 0.15),
                    at(0.0, 0.22),
                ));
                lamp_y(s.floor + 0.35)
            };
            let z = if s.nose == Nose::Tram {
                at(0.1, 0.17)
            } else {
                at(0.0, 0.05)
            };
            out.push(lit(bx(HEAD, (hw - 0.62, hw - 0.28), lamps, z)));
            out.push(lit(bx(HEAD, (-hw + 0.28, -hw + 0.62), lamps, z)));
            out.push(lit(bx(TAIL, (hw - 0.9, hw - 0.7), lamps, z)));
            out.push(lit(bx(TAIL, (-hw + 0.7, -hw + 0.9), lamps, z)));
        }
        Nose::Streamlined => {
            let tip = d * (half - 0.15);
            let span = |reach: f32| (face, d * (half - reach));
            out.push(bx(
                l.body,
                (-hw, hw),
                (s.floor, s.floor + 1.05),
                (face, tip),
            ));
            out.push(bx(
                l.body,
                (-hw + 0.08, hw - 0.08),
                (s.floor + 1.05, s.floor + 1.75),
                span(1.3),
            ));
            out.push(bx(
                l.body,
                (-hw + 0.25, hw - 0.25),
                (s.floor + 1.75, s.top - 0.12),
                span(2.7),
            ));
            out.push(plate(
                GLASS,
                (-hw + 0.3, hw - 0.3),
                (s.floor + 1.0, d * (half - 0.95)),
                (s.top - 0.08, d * (half - 2.85)),
                0.06,
            ));
            if let Some((a, b)) = s.stripe {
                out.push(bx(
                    l.stripe,
                    (-hw - 0.02, hw + 0.02),
                    (a, b),
                    (face, d * (half - 0.7)),
                ));
            }
            out.push(bx(
                UNDER,
                (-hw + 0.2, hw - 0.2),
                (0.45, s.floor),
                (face, d * (half - 0.4)),
            ));
            let lamps = lamp_y(s.floor + 0.42);
            let z = (tip - d * 0.02, tip + d * 0.03);
            out.push(lit(bx(HEAD, (hw - 0.75, hw - 0.4), lamps, z)));
            out.push(lit(bx(HEAD, (-hw + 0.4, -hw + 0.75), lamps, z)));
            out.push(lit(bx(TAIL, (hw - 1.0, hw - 0.82), lamps, z)));
            out.push(lit(bx(TAIL, (-hw + 0.82, -hw + 1.0), lamps, z)));
        }
    }
    out
}

/// A single-arm pantograph standing on the roof at `roof`, centred on `z`.
fn pantograph(roof: f32, z: f32) -> Vec<Part> {
    vec![
        bx(STEEL, (-0.5, 0.5), (roof, roof + 0.16), (z - 0.7, z + 0.7)),
        plate(
            STEEL,
            (-0.05, 0.05),
            (roof + 0.16, z - 0.55),
            (roof + 0.95, z + 0.3),
            0.08,
        ),
        bx(
            BLACK,
            (-0.85, 0.85),
            (roof + 0.93, roof + 1.0),
            (z + 0.22, z + 0.42),
        ),
    ]
}

/// An electric locomotive, a cab at either end and grilles between.
fn loco(l: Livery) -> Vec<Part> {
    let (len, width, floor, top) = (19.5f32, 3.0f32, 1.15f32, 4.1f32);
    let (hw, half) = (width / 2.0, len / 2.0);
    let spec = Spec {
        len,
        width,
        floor,
        top,
        windows: &[],
        doors: &[],
        door_w: 0.0,
        bogies: &[-half + 3.6, half - 3.6],
        bogie: (3.6, 1.0),
        band: Some(0.35),
        stripe: Some((floor + 0.55, floor + 0.75)),
        panto: None,
        cab: None,
        nose: Nose::Flat,
    };
    let mut out = vec![
        bx(
            BOGIE,
            (-hw + 0.35, hw - 0.35),
            (0.04, 1.0),
            (-half + 1.8, -half + 5.4),
        ),
        bx(
            BOGIE,
            (-hw + 0.35, hw - 0.35),
            (0.04, 1.0),
            (half - 5.4, half - 1.8),
        ),
        bx(
            UNDER,
            (-hw + 0.15, hw - 0.15),
            (0.55, floor),
            (-half + 0.5, half - 0.5),
        ),
        bx(l.body, (-hw, hw), (floor, top), (-half + 0.15, half - 0.15)),
        bx(
            l.lower,
            (-hw - 0.01, hw + 0.01),
            (floor, floor + 0.35),
            (-half + 0.15, half - 0.15),
        ),
        bx(
            l.stripe,
            (-hw - 0.02, hw + 0.02),
            (floor + 0.55, floor + 0.75),
            (-half + 0.15, half - 0.15),
        ),
        // Grilles along the machine room, a side window to each cab.
        bx(
            UNDER,
            (-hw - 0.02, hw + 0.02),
            (floor + 1.2, floor + 2.3),
            (-half + 3.4, half - 3.4),
        ),
        bx(
            GLASS,
            (-hw - 0.03, hw + 0.03),
            (floor + 1.35, floor + 2.25),
            (half - 2.6, half - 1.2),
        ),
        bx(
            GLASS,
            (-hw - 0.03, hw + 0.03),
            (floor + 1.35, floor + 2.25),
            (-half + 1.2, -half + 2.6),
        ),
        bx(
            l.roof,
            (-hw + 0.12, hw - 0.12),
            (top, top + 0.14),
            (-half + 0.4, half - 0.4),
        ),
    ];
    for d in [1.0f32, -1.0] {
        let windowed = Spec {
            windows: &[(floor + 1.35, top - 0.4)],
            ..spec
        };
        out.extend(cab(&windowed, l, d, d * (half - 0.15)));
        out.extend(pantograph(top + 0.14, d * (half - 4.6)));
    }
    out
}

/// A freight wagon's frame, bogies and buffers, `len` long.
fn wagon_frame(len: f32) -> Vec<Part> {
    let half = len / 2.0;
    vec![
        bx(
            BOGIE,
            (-1.15, 1.15),
            (0.04, 0.85),
            (-half + 1.6, -half + 4.2),
        ),
        bx(BOGIE, (-1.15, 1.15), (0.04, 0.85), (half - 4.2, half - 1.6)),
        bx(UNDER, (-1.4, 1.4), (0.85, 1.25), (-half + 0.3, half - 0.3)),
        bx(BLACK, (-1.1, 1.1), (0.82, 1.15), (half - 0.3, half + 0.18)),
        bx(
            BLACK,
            (-1.1, 1.1),
            (0.82, 1.15),
            (-half - 0.18, -half + 0.3),
        ),
    ]
}

/// A carriage's parts at 1:1.
pub fn parts(coach: Coach, l: Livery) -> Vec<Part> {
    let mut parts = built(coach, l);
    crate::element_processing::ship_bodies::apart(&mut parts);
    parts
}

fn built(coach: Coach, l: Livery) -> Vec<Part> {
    let len = length_m(coach) as f32;
    let half = len / 2.0;
    let cab_of = |front: bool| Some(if front { 1.0 } else { -1.0 });
    match coach {
        Coach::TramCab { front } => {
            let d = if front { 1.0 } else { -1.0 };
            self::coach(
                &Spec {
                    len,
                    width: 2.4,
                    floor: 0.35,
                    top: 3.3,
                    windows: &[(1.25, 2.75)],
                    doors: &[-d * 1.6],
                    door_w: 1.3,
                    bogies: &[-d * 0.6],
                    bogie: (2.0, 0.55),
                    band: Some(0.45),
                    stripe: None,
                    panto: None,
                    cab: cab_of(front),
                    nose: Nose::Tram,
                },
                l,
            )
        }
        Coach::TramMid | Coach::TramShort => {
            let short = coach == Coach::TramShort;
            self::coach(
                &Spec {
                    len,
                    width: 2.4,
                    floor: 0.35,
                    top: 3.3,
                    windows: &[(1.25, 2.75)],
                    doors: if short { &[] } else { &[0.0] },
                    door_w: 1.3,
                    bogies: if short { &[] } else { &[0.0] },
                    bogie: (2.0, 0.55),
                    band: Some(0.45),
                    stripe: None,
                    panto: (!short).then_some(-1.8),
                    cab: None,
                    nose: Nose::Tram,
                },
                l,
            )
        }
        Coach::MetroCab { .. } | Coach::MetroMid => {
            let cab = front_of(coach).and_then(cab_of_opt);
            self::coach(
                &Spec {
                    len,
                    width: 2.65,
                    floor: 0.95,
                    top: 3.5,
                    windows: &[(1.85, 2.85)],
                    doors: &[-5.0, 0.0, 5.0],
                    door_w: 1.3,
                    bogies: &[-half + 2.4, half - 2.4],
                    bogie: (2.4, 0.85),
                    band: Some(0.3),
                    stripe: Some((1.55, 1.7)),
                    panto: None,
                    cab,
                    nose: Nose::Flat,
                },
                l,
            )
        }
        Coach::SBahnCab { .. } | Coach::SBahnMid => self::coach(
            &Spec {
                len,
                width: 3.0,
                floor: 1.0,
                top: 3.95,
                windows: &[(2.0, 3.05)],
                doors: &[-5.6, 0.0, 5.6],
                door_w: 1.4,
                bogies: &[-half + 2.5, half - 2.5],
                bogie: (2.6, 0.9),
                band: Some(1.0),
                stripe: Some((1.82, 1.96)),
                panto: front_of(coach).filter(|_| l.name != "sb").map(|f| {
                    if f {
                        half - 4.0
                    } else {
                        -half + 4.0
                    }
                }),
                cab: front_of(coach).and_then(cab_of_opt),
                nose: Nose::Flat,
            },
            l,
        ),
        Coach::RegioCab { .. } | Coach::RegioMid => self::coach(
            &Spec {
                len,
                width: 2.9,
                floor: 0.75,
                top: 3.9,
                windows: &[(1.75, 2.85)],
                doors: &[-half + 3.2, half - 3.2],
                door_w: 1.3,
                bogies: &[-half + 2.4, half - 2.4],
                bogie: (2.5, 0.8),
                band: Some(0.35),
                stripe: Some((1.5, 1.62)),
                panto: front_of(coach).map(|f| if f { half - 5.0 } else { -half + 5.0 }),
                cab: front_of(coach).and_then(cab_of_opt),
                nose: Nose::Flat,
            },
            l,
        ),
        Coach::DeckCab { .. } | Coach::DeckMid => self::coach(
            &Spec {
                len,
                width: 2.8,
                floor: 0.6,
                top: 4.6,
                windows: &[(1.35, 2.2), (2.95, 3.85)],
                doors: &[-half + 2.6, half - 2.6],
                door_w: 1.4,
                bogies: &[-half + 2.6, half - 2.6],
                bogie: (2.6, 0.75),
                band: Some(0.45),
                stripe: Some((2.45, 2.62)),
                panto: None,
                cab: front_of(coach).and_then(cab_of_opt),
                nose: Nose::Flat,
            },
            l,
        ),
        Coach::IceCab { .. } | Coach::IceMid => {
            let ends = [-half + 1.6, half - 1.6];
            self::coach(
                &Spec {
                    len,
                    width: 2.95,
                    floor: 1.15,
                    top: 3.85,
                    windows: &[(2.15, 3.15)],
                    doors: if coach == Coach::IceMid {
                        &ends
                    } else {
                        &[0.0]
                    },
                    door_w: 1.0,
                    bogies: &[-half + 3.0, half - 3.0],
                    bogie: (3.0, 1.0),
                    band: None,
                    stripe: Some((1.9, 2.05)),
                    panto: front_of(coach).map(|f| if f { -half + 6.0 } else { half - 6.0 }),
                    cab: front_of(coach).and_then(cab_of_opt),
                    nose: Nose::Streamlined,
                },
                l,
            )
        }
        Coach::Loco => loco(l),
        Coach::Container(v) => {
            let mut out = wagon_frame(len);
            let colour = |k: usize| CONTAINERS[(wagon(v, l.seed) / (k + 1)) % CONTAINERS.len()];
            let (hx, y) = (1.22, (1.25, 3.85));
            match wagon(v, l.seed) % 10 {
                0..=3 => out.push(bx(colour(0), (-hx, hx), y, (-6.1, 6.1))),
                4 => {}
                _ => {
                    out.push(bx(colour(0), (-hx, hx), y, (-6.35, -0.25)));
                    out.push(bx(colour(1), (-hx, hx), y, (0.25, 6.35)));
                }
            }
            out
        }
        Coach::Tank(v) => {
            let mut out = wagon_frame(len);
            let c = TANKS[wagon(v, l.seed) % TANKS.len()];
            out.push(bx(c, (-1.0, 1.0), (1.35, 3.65), (-half + 0.7, half - 0.7)));
            out.push(bx(
                c,
                (-1.35, 1.35),
                (1.7, 3.3),
                (-half + 0.95, half - 0.95),
            ));
            out.push(bx(c, (-0.4, 0.4), (3.65, 3.9), (-0.5, 0.5)));
            out
        }
        Coach::Boxcar(v) => {
            let mut out = wagon_frame(len);
            let c = BOXCARS[wagon(v, l.seed) % BOXCARS.len()];
            out.push(bx(
                c,
                (-1.4, 1.4),
                (1.25, 3.95),
                (-half + 0.35, half - 0.35),
            ));
            out.push(bx(
                c,
                (-1.25, 1.25),
                (3.95, 4.12),
                (-half + 0.45, half - 0.45),
            ));
            out.push(bx(
                "minecraft:gray_terracotta",
                (-1.43, 1.43),
                (1.4, 3.7),
                (-1.6, 1.6),
            ));
            out
        }
        Coach::Ship { vessel, low } => vessel.parts(l, low),
    }
}

fn front_of(coach: Coach) -> Option<bool> {
    match coach {
        Coach::TramCab { front }
        | Coach::MetroCab { front }
        | Coach::SBahnCab { front }
        | Coach::RegioCab { front }
        | Coach::DeckCab { front }
        | Coach::IceCab { front } => Some(front),
        _ => None,
    }
}

fn cab_of_opt(front: bool) -> Option<f32> {
    Some(if front { 1.0 } else { -1.0 })
}

/// The quaternion of a turn by `angle` about x, as the pack writes it.
fn about_x(angle: f32) -> [f32; 4] {
    let h = angle / 2.0;
    [h.sin(), 0.0, 0.0, h.cos()]
}

/// The body function of a carriage: a macro taking the carriage id `c` and its heading
/// `h`, run where the carriage centre goes. 26.3 names a block bare (`bare`).
pub fn summon_function(parts: &[Part], size: f64, bare: bool) -> String {
    let k = size as f32;
    let v3 = |v: [f32; 3]| format!("[{:.3}f,{:.3}f,{:.3}f]", v[0] * k, v[1] * k, v[2] * k);
    let mut text = String::new();
    for p in parts {
        let block = if bare {
            format!("\"{}\"", p.block)
        } else {
            format!("{{Name:\"{}\"}}", p.block)
        };
        let q = about_x(p.tilt);
        let glow = if p.glow {
            ",brightness:{sky:15,block:15}"
        } else {
            ""
        };
        let _ = writeln!(
            text,
            "$summon block_display ~ ~ ~ {{Tags:[\"arnis.tp\",\"arnis.tc$(c)\"],Rotation:[$(h)f,0f],teleport_duration:2,view_range:3f,block_state:{block},transformation:{{left_rotation:[{:.4}f,0f,0f,{:.4}f],right_rotation:[0f,0f,0f,1f],translation:{},scale:{}}}{glow}}}",
            q[0],
            q[3],
            v3(p.corner),
            v3(p.size),
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Kind; 7] = [
        Kind::Tram,
        Kind::Metro,
        Kind::SBahn,
        Kind::Regional,
        Kind::DoubleDeck,
        Kind::Intercity,
        Kind::Freight,
    ];

    #[test]
    fn no_two_parts_of_a_carriage_flicker_against_each_other_or_hide_one_another() {
        use crate::element_processing::ship_bodies::{buried, shared_faces};
        let sets: [(&[Coach], &[Livery]); 8] = [
            (
                &[
                    Coach::TramCab { front: true },
                    Coach::TramMid,
                    Coach::TramShort,
                ],
                &TRAMS,
            ),
            (&[Coach::MetroCab { front: true }, Coach::MetroMid], &METROS),
            (&[Coach::SBahnCab { front: true }, Coach::SBahnMid], &SBAHNS),
            (
                &[Coach::RegioCab { front: true }, Coach::RegioMid],
                &REGIONALS,
            ),
            (&[Coach::DeckCab { front: true }, Coach::DeckMid], &DECKS),
            (&[Coach::IceCab { front: true }, Coach::IceMid], &INTERCITY),
            (&[Coach::Loco], &LOCOS),
            (
                &[Coach::Container(0), Coach::Tank(0), Coach::Boxcar(0)],
                &LOCOS,
            ),
        ];
        let mut found = Vec::new();
        for (coaches, liveries) in sets {
            for &coach in coaches {
                for &l in liveries {
                    let parts = parts(coach, l);
                    for (i, a) in parts.iter().enumerate() {
                        for b in &parts[i + 1..] {
                            if !shared_faces(a, b).is_empty() {
                                found.push(format!("{coach:?} {} / {}", a.block, b.block));
                            }
                        }
                    }
                    for b in buried(&parts) {
                        found.push(format!("{coach:?} {b}"));
                    }
                }
            }
        }
        found.sort();
        found.dedup();
        assert!(
            found.is_empty(),
            "{} flickering:\n{}",
            found.len(),
            found.join("\n")
        );
    }

    #[test]
    fn passengers_can_get_on_along_a_whole_carriage_but_not_on_a_wagon() {
        assert!(doors(Coach::Container(0)).spots.is_empty());
        assert_eq!(door_key(Coach::Tank(3)), NO_DOORS);
        assert_eq!(doors(Coach::TramShort).spots.len(), 1);
        for coach in [
            Coach::MetroMid,
            Coach::SBahnMid,
            Coach::RegioMid,
            Coach::DeckMid,
            Coach::IceMid,
            Coach::Loco,
        ] {
            let spots = doors(coach).spots;
            let half = length_m(coach) as f32 / 2.0;
            assert!(spots.iter().all(|d| d.abs() < half));
            // Boxes side by side leave little gap to click into.
            for pair in spots.windows(2) {
                assert!(pair[1] - pair[0] <= DOOR_BOX_WIDTH + 1.2);
            }
        }
    }

    #[test]
    fn a_seat_is_in_reach_of_its_door_at_every_scale() {
        let board = include_str!(
            "../../assets/minecraft/datapack_extras/data/arnis/function/trains/board.mcfunction"
        );
        let reach: f64 = board
            .split("tag=arnis.seat,distance=..")
            .nth(1)
            .and_then(|r| r.split(',').next())
            .and_then(|n| n.parse().ok())
            .expect("board looks for a seat");
        // Trains are drawn at up to three times their size.
        for size in [0.4, 1.0, 2.0, 3.0] {
            let seat = RIDER_EYE * size - RIDER_EYE_OVER_SEAT;
            assert!(seat < reach, "a seat {seat:.2} up at size {size}");
            // The next door's seat is further off than this one's.
            let spacing = f64::from(DOOR_SPACING) * size * 0.5;
            assert!(seat < spacing.hypot(seat), "size {size}");
        }
    }

    #[test]
    fn door_functions_give_each_door_a_box_and_a_seat_that_move_with_the_carriage() {
        let doors = doors(Coach::MetroMid);
        let spots = &doors.spots;
        let (keep, make) = door_functions("m160", &doors, 1.0);
        assert!(!make.contains("arnis.tdeck"));
        assert_eq!(make.matches("summon interaction").count(), spots.len());
        assert_eq!(make.matches("arnis.seat").count(), spots.len());
        assert_eq!(keep.matches("$tp @e[type=interaction").count(), spots.len());
        assert_eq!(
            keep.matches("$tp @e[type=block_display").count(),
            spots.len()
        );
        // A macro line must use a macro, or the function will not load.
        for line in keep.lines().chain(make.lines()) {
            assert!(line.starts_with('$') && line.contains("$(c)"), "{line}");
        }
        let (keep, make) = door_functions(NO_DOORS, &self::doors(Coach::Tank(0)), 1.0);
        assert!(keep.is_empty() && make.is_empty());
    }

    #[test]
    fn every_carriage_stays_within_its_length_and_the_loading_gauge() {
        for kind in ALL {
            for seed in 0..20u64 {
                let (coaches, livery) = compose(kind, seed, false);
                for coach in coaches {
                    let half = (length_m(coach) / 2.0) as f32 + 0.25;
                    for p in parts(coach, livery(0)) {
                        // A tilted part's corners, turned about x as the display turns them.
                        let (sin, cos) = p.tilt.sin_cos();
                        let (ys, zs): (Vec<f32>, Vec<f32>) = [
                            (0.0, 0.0),
                            (p.size[1], 0.0),
                            (0.0, p.size[2]),
                            (p.size[1], p.size[2]),
                        ]
                        .iter()
                        .map(|&(y, z)| {
                            (
                                p.corner[1] + y * cos - z * sin,
                                p.corner[2] + y * sin + z * cos,
                            )
                        })
                        .unzip();
                        let lo = |v: &[f32]| v.iter().copied().fold(f32::MAX, f32::min);
                        let hi = |v: &[f32]| v.iter().copied().fold(f32::MIN, f32::max);
                        assert!(lo(&zs) >= -half && hi(&zs) <= half, "{coach:?} {p:?}");
                        assert!(
                            p.corner[0] >= -1.6 && p.corner[0] + p.size[0] <= 1.6,
                            "{coach:?} {p:?}"
                        );
                        assert!(lo(&ys) >= -0.01 && hi(&ys) <= 5.8, "{coach:?} {p:?}");
                        assert!(p.size.iter().all(|&s| s > 0.0), "{coach:?} {p:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_train_has_a_cab_facing_out_at_either_end() {
        for kind in [
            Kind::Tram,
            Kind::Metro,
            Kind::SBahn,
            Kind::Regional,
            Kind::Intercity,
        ] {
            let (coaches, _) = compose(kind, 3, false);
            assert_eq!(front_of(coaches[0]), Some(true), "{kind:?}");
            assert_eq!(front_of(*coaches.last().unwrap()), Some(false), "{kind:?}");
        }
    }

    #[test]
    fn body_functions_summon_every_part_tagged_for_its_carriage() {
        let (coaches, livery) = compose(Kind::Intercity, 1, false);
        let parts = parts(coaches[0], livery(0));
        for bare in [false, true] {
            let text = summon_function(&parts, 1.0, bare);
            assert_eq!(text.lines().count(), parts.len());
            for line in text.lines() {
                assert!(line.starts_with(
                    "$summon block_display ~ ~ ~ {Tags:[\"arnis.tp\",\"arnis.tc$(c)\"]"
                ));
                assert!(line.contains("Rotation:[$(h)f,0f]"));
                assert_eq!(
                    line.matches('{').count(),
                    line.matches('}').count(),
                    "{line}"
                );
            }
        }
    }

    #[test]
    fn freight_trains_on_a_route_differ_but_each_is_the_same_every_time() {
        let (coaches, livery) = compose(Kind::Freight, 9, false);
        let keys =
            |k: usize| -> Vec<String> { coaches.iter().map(|&c| key(c, livery(k))).collect() };
        assert_eq!(keys(0), keys(0));
        assert_ne!(keys(0), keys(1));
        assert_eq!(coaches[0], Coach::Loco);
    }

    // Writes every kind's carriages as JSON for an offline render: ARNIS_TRAIN_DUMP=path.
    #[test]
    #[ignore]
    fn dump_trains_for_a_render() {
        let Ok(path) = std::env::var("ARNIS_TRAIN_DUMP") else {
            return;
        };
        let mut out = String::from("[");
        for (t, kind) in ALL.iter().enumerate() {
            let (coaches, livery) = compose(*kind, 5, false);
            let mut z = 0.0f64;
            for (k, coach) in coaches.iter().enumerate() {
                if k > 0 {
                    z -= (length_m(coaches[k - 1]) + length_m(*coach)) / 2.0 + 0.4;
                }
                for p in parts(*coach, livery(0)) {
                    out.push_str(&format!(
                        "{{\"train\":{t},\"z\":{z},\"block\":\"{}\",\"size\":[{},{},{}],\"corner\":[{},{},{}],\"tilt\":{},\"glow\":{}}},",
                        p.block, p.size[0], p.size[1], p.size[2], p.corner[0], p.corner[1], p.corner[2], p.tilt, p.glow
                    ));
                }
            }
        }
        out.pop();
        out.push(']');
        std::fs::write(path, out).unwrap();
    }
}
