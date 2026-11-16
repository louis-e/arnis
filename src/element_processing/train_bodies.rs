//! What trains, metros and trams are made of and look like: each carriage a model from
//! `assets/vehicles` in its operator's colours, drawn by one item display. The pack
//! summons a carriage from its body function when it comes into view; `trains.rs`
//! decides which carriages make a train.

use crate::element_processing::plane_bodies::Airframe;
use crate::element_processing::ship_bodies::Vessel;
use crate::vehicle_models::{self, Look};
use std::fmt::Write;

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
    /// An aircraft.
    Plane(Airframe),
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

/// Boxes along a ship's hull for a spectator to click, every so many metres, kept this far
/// from a seat's box and from the ends, in metres.
const HULL_BOX_SPACING: f32 = 12.0;
const HULL_BOX_CLEAR: f32 = 6.0;

/// Where along a carriage people get on, in metres from its middle, how high their eyes
/// are over the rail and the box that takes a click there. A click nearly anywhere on a
/// carriage's side finds one. Freight wagons carry nobody, but have boxes all the same, as
/// a ship's hull has between its seats: a spectator clicks one to follow the vehicle.
pub struct Doors {
    pub spots: Vec<f32>,
    /// Boxes with no seat, only to be clicked by a spectator.
    pub view: Vec<f32>,
    eye: f64,
    width: f32,
    height: f32,
    /// The seat is over the box rather than beside it, as on a boat's deck.
    deck: bool,
}

impl Doors {
    /// No doors and no boxes.
    pub fn none() -> Self {
        Doors {
            spots: Vec::new(),
            view: Vec::new(),
            eye: RIDER_EYE,
            width: DOOR_BOX_WIDTH,
            height: DOOR_BOX_HEIGHT,
            deck: false,
        }
    }
}

/// Spots every `spacing` metres or less along `len` metres, the first and last half a
/// spacing in from the ends.
fn spread(len: f32, spacing: f32) -> Vec<f32> {
    let n = (len / spacing).ceil().max(1.0) as usize;
    let step = len / n as f32;
    (0..n)
        .map(|i| -len / 2.0 + step * (i as f32 + 0.5))
        .collect()
}

pub fn doors(coach: Coach) -> Doors {
    if let Coach::Ship { vessel, .. } = coach {
        let (spots, eye) = vessel.seats_m();
        let (len, beam) = vessel.size_m();
        let width = (beam as f32).max(DOOR_BOX_WIDTH);
        let hull = len as f32 - 2.0 * HULL_BOX_CLEAR;
        // A box as wide as the beam reaches as far along the hull, and one over a seat's
        // would take the click that boards.
        let clear = HULL_BOX_CLEAR.max(width);
        let view = if hull > 0.0 {
            spread(hull, HULL_BOX_SPACING)
                .into_iter()
                .filter(|v| spots.iter().all(|s| (s - v).abs() >= clear))
                .collect()
        } else {
            Vec::new()
        };
        return Doors {
            spots: spots.to_vec(),
            view,
            eye,
            width,
            height: eye as f32 + 0.5,
            deck: true,
        };
    }
    // Clicked from the apron under it or beside it, the seat up in the cabin.
    if let Coach::Plane(frame) = coach {
        let (spots, eye) = frame.seats_m();
        return Doors {
            spots: spots.to_vec(),
            view: Vec::new(),
            eye,
            width: frame.cabin_m() as f32 + 1.0,
            height: eye as f32 + 0.8,
            deck: true,
        };
    }
    let along = spread(length_m(coach) as f32, DOOR_SPACING);
    let (spots, view) = if matches!(
        coach,
        Coach::Container(_) | Coach::Tank(_) | Coach::Boxcar(_)
    ) {
        (Vec::new(), along)
    } else {
        (along, Vec::new())
    };
    Doors {
        spots,
        view,
        eye: RIDER_EYE,
        width: DOOR_BOX_WIDTH,
        height: DOOR_BOX_HEIGHT,
        deck: false,
    }
}

/// The name of the functions that keep a carriage's doors with it, shared by carriages alike.
pub fn door_key(coach: Coach) -> String {
    let doors = doors(coach);
    match coach {
        Coach::Ship { vessel, .. } => format!("w_{}", vessel.name()),
        Coach::Plane(frame) => format!("p_{}", frame.name()),
        _ if doors.spots.is_empty() && doors.view.is_empty() => NO_DOORS.to_string(),
        _ if doors.spots.is_empty() => format!("f{}", (length_m(coach) * 10.0).round() as u32),
        _ => format!("m{}", (length_m(coach) * 10.0).round() as u32),
    }
}

/// The function that keeps a carriage's doors with it, run at the carriage's middle facing
/// along it, and the one that makes them. Each door is a box to click and a seat to ride;
/// the boxes with no seat are only for a spectator to click.
pub fn door_functions(key: &str, doors: &Doors, size: f64) -> (String, String) {
    let spots = &doors.spots;
    if spots.is_empty() && doors.view.is_empty() {
        return (String::new(), String::new());
    }
    let k = size as f32;
    let reach = spots
        .iter()
        .chain(&doors.view)
        .fold(0f32, |a, d| a.max(d.abs()))
        * k
        + 12.0;
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
    for (j, v) in doors.view.iter().enumerate() {
        let at = v * k;
        let _ = writeln!(
            keep,
            "$tp @e[type=interaction,tag=arnis.ts$(c),tag=arnis.tv{j},distance=..{reach:.1}] ^ ^ ^{at:.2}"
        );
        let _ = writeln!(
            make,
            "$summon interaction ^ ^ ^{at:.2} {{Tags:[\"arnis.tp\",\"arnis.ts$(c)\",\"arnis.tv{j}\",\"arnis.tview\"],width:{:.2}f,height:{:.2}f}}",
            doors.width * k,
            doors.height * k,
        );
    }
    // Each box knows its carriage, for a spectator who clicks it to be put on that model.
    let _ = writeln!(
        make,
        "$scoreboard players set @e[type=interaction,tag=arnis.ts$(c),distance=..{reach:.1}] arnis.tw $(c)"
    );
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
        Coach::Plane(frame) => frame.size_m().0,
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
        // Wagon colours come from the variant and the train, so both name the body, as
        // its look does.
        Coach::Container(_) => {
            let l = look(coach, livery);
            format!("{}_{}", l.model.trim_start_matches("freight_"), l.livery)
        }
        Coach::Tank(v) => format!("tank_{}", wagon(v, livery.seed) % TANKS.len()),
        Coach::Boxcar(v) => format!("boxcar_{}", wagon(v, livery.seed) % BOXCARS.len()),
        Coach::Ship { vessel, low } => {
            format!("ship_{}{}", vessel.name(), if low { "_low" } else { "" })
        }
        Coach::Plane(frame) => format!("plane_{}", frame.name()),
    };
    match coach {
        Coach::Container(_) | Coach::Tank(_) | Coach::Boxcar(_) => c,
        _ => format!("{c}_{}", livery.name),
    }
}

fn wagon(v: u8, seed: u8) -> usize {
    (roll(v as u64, seed as u64) % 1000) as usize
}

/// The model a carriage is drawn with, in its train's colours, and which way round.
/// Cabs are modelled facing ahead; one at the back of its train sits backward.
pub fn look(coach: Coach, l: Livery) -> Look {
    let wagon_look =
        |model: &'static str, livery: String, first: &'static str, second: &'static str| Look {
            model,
            livery,
            colours: [first, second, first, first, first],
            backward: false,
        };
    let (model, backward) = match coach {
        Coach::TramCab { front } => ("train_tram_cab", !front),
        Coach::TramMid => ("train_tram_mid", false),
        Coach::TramShort => ("train_tram_short", false),
        Coach::MetroCab { front } => ("train_metro_cab", !front),
        Coach::MetroMid => ("train_metro_mid", false),
        // Berlin's runs on a third rail: no pantograph.
        Coach::SBahnCab { front } if l.name == "sb" => ("train_sbahn_cab_third_rail", !front),
        Coach::SBahnCab { front } => ("train_sbahn_cab", !front),
        Coach::SBahnMid => ("train_sbahn_mid", false),
        Coach::RegioCab { front } => ("train_regio_cab", !front),
        Coach::RegioMid => ("train_regio_mid", false),
        Coach::DeckCab { front } => ("train_deck_cab", !front),
        Coach::DeckMid => ("train_deck_mid", false),
        Coach::IceCab { front } => ("train_ice_cab", !front),
        Coach::IceMid => ("train_ice_mid", false),
        Coach::Loco => ("train_loco", false),
        // A wagon's livery is named by the colours its model shows, so wagons alike in
        // colour share a texture: the 40 foot box shows one, the two 20 foot boxes two,
        // the empty flat none.
        Coach::Container(v) => {
            let w = wagon(v, l.seed);
            let (a, b) = (w % CONTAINERS.len(), (w / 2) % CONTAINERS.len());
            let (first, second) = (CONTAINERS[a], CONTAINERS[b]);
            return match w % 10 {
                0..=3 => wagon_look("freight_container_40", a.to_string(), first, first),
                4 => wagon_look("freight_flat", "plain".into(), CONTAINERS[0], CONTAINERS[0]),
                _ => wagon_look("freight_container_2x20", format!("{a}_{b}"), first, second),
            };
        }
        Coach::Tank(v) => {
            let c = wagon(v, l.seed) % TANKS.len();
            return wagon_look("freight_tank", c.to_string(), TANKS[c], TANKS[c]);
        }
        Coach::Boxcar(v) => {
            let c = wagon(v, l.seed) % BOXCARS.len();
            return wagon_look("freight_boxcar", c.to_string(), BOXCARS[c], BOXCARS[c]);
        }
        Coach::Ship { vessel, low } => (vessel.model(low), false),
        Coach::Plane(frame) => (frame.model(), false),
    };
    Look {
        model,
        livery: l.name.to_string(),
        colours: [l.body, l.lower, l.stripe, l.door, l.roof],
        backward,
    }
}

/// Whether a carriage is a cab, and if so whether it leads (true) or trails its train.
#[cfg(test)]
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

/// The body function of a carriage: a macro taking the carriage id `c` and its heading
/// `h`, run `LIFT` over where the carriage centre goes. One item display draws it, the model
/// let down as far. An aircraft is seen from further off, since it is as far overhead as it
/// is away.
pub fn summon_function(look: &Look, size: f64) -> String {
    let view = if vehicle_models::flies(look.model) {
        5
    } else {
        3
    };
    format!(
        "$summon item_display ~ ~ ~ {{Tags:[\"arnis.tp\",\"arnis.tc$(c)\"],Rotation:[$(h)f,0f],teleport_duration:2,view_range:{view}f,{}}}\n",
        vehicle_models::display_snbt(
            look.model,
            &vehicle_models::item_model(look),
            size,
            look.backward,
            vehicle_models::LIFT,
            ""
        ),
    )
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
    fn passengers_can_get_on_along_a_whole_carriage_but_not_on_a_wagon() {
        assert!(doors(Coach::Container(0)).spots.is_empty());
        assert_eq!(door_key(Coach::Tank(3)), "f150");
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
        let (keep, make) = door_functions(NO_DOORS, &Doors::none(), 1.0);
        assert!(keep.is_empty() && make.is_empty());
    }

    /// A spectator follows any vehicle by clicking it: every one has boxes along its body
    /// that move with it, those of wagons and between a ship's seats with nothing to board.
    #[test]
    fn every_vehicle_has_boxes_along_it_for_a_spectator_to_click() {
        use crate::element_processing::plane_bodies::Airframe;
        use crate::element_processing::ship_bodies::Vessel;
        let mut coaches = vec![
            Coach::Container(0),
            Coach::Tank(0),
            Coach::Boxcar(0),
            Coach::TramShort,
            Coach::MetroMid,
            Coach::Loco,
            Coach::IceMid,
        ];
        coaches.extend(
            [
                Airframe::Light,
                Airframe::Bizjet,
                Airframe::Regional,
                Airframe::Narrow,
                Airframe::Wide,
            ]
            .map(Coach::Plane),
        );
        coaches.extend(
            [
                Vessel::RiverFerry,
                Vessel::WaterBus,
                Vessel::CarFerry,
                Vessel::SeaFerry,
                Vessel::Barge,
                Vessel::RiverCruise,
                Vessel::Peniche,
                Vessel::TourBoat,
                Vessel::Sailboat,
                Vessel::Motorboat,
            ]
            .map(|vessel| Coach::Ship { vessel, low: false }),
        );
        for coach in coaches {
            let doors = doors(coach);
            let mut boxes: Vec<f32> = doors.spots.iter().chain(&doors.view).copied().collect();
            assert!(!boxes.is_empty(), "{coach:?}");
            boxes.sort_by(f32::total_cmp);
            // No long stretch of a carriage or a hull out of any box's reach, outside an
            // aircraft's wings and tail.
            if !matches!(coach, Coach::Plane(_)) {
                let half = length_m(coach) as f32 / 2.0;
                let mut edges = vec![-half];
                edges.extend(&boxes);
                edges.push(half);
                for pair in edges.windows(2) {
                    assert!(
                        pair[1] - pair[0] - doors.width / 2.0 <= 2.0 * HULL_BOX_SPACING,
                        "{coach:?}: {boxes:?}"
                    );
                }
            }
            // A box with no seat never covers one that boards.
            for v in &doors.view {
                assert!(
                    doors.spots.iter().all(|s| (s - v).abs() >= doors.width),
                    "{coach:?}"
                );
            }
            let (keep, make) = door_functions(&door_key(coach), &doors, 1.0);
            assert_eq!(make.matches("summon interaction").count(), boxes.len());
            assert_eq!(make.matches("arnis.tview").count(), doors.view.len());
            assert_eq!(make.matches("arnis.seat").count(), doors.spots.len());
            assert_eq!(keep.matches("$tp @e[type=interaction").count(), boxes.len());
            for line in keep.lines().chain(make.lines()) {
                assert!(line.starts_with('$') && line.contains("$(c)"), "{line}");
            }
        }
        // Clicking a box with no seat boards nothing.
        for path in ["cars/board", "trains/board"] {
            let text = match path {
                "cars/board" => include_str!(
                    "../../assets/minecraft/datapack_extras/data/arnis/function/cars/board.mcfunction"
                ),
                _ => include_str!(
                    "../../assets/minecraft/datapack_extras/data/arnis/function/trains/board.mcfunction"
                ),
            };
            for line in text.lines().filter(|l| l.contains("tag=arnis.tp")) {
                assert!(line.contains("tag=!arnis.tview"), "{path}: {line}");
            }
        }
    }

    #[test]
    fn every_carriage_stays_within_its_length_and_the_loading_gauge() {
        for kind in ALL {
            for seed in 0..20u64 {
                let (coaches, livery) = compose(kind, seed, false);
                for coach in coaches {
                    let look = look(coach, livery(0));
                    let [lo, hi] = look.meta().bounds.map(|c| c.map(|v| v / 16.0));
                    let half = length_m(coach) / 2.0 + 0.25;
                    assert!(lo[2] >= -half && hi[2] <= half, "{coach:?}: {lo:?} {hi:?}");
                    assert!(lo[0] >= -1.6 && hi[0] <= 1.6, "{coach:?}: {lo:?} {hi:?}");
                    assert!(lo[1] >= -0.01 && hi[1] <= 5.8, "{coach:?}: {lo:?} {hi:?}");
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
            let (coaches, livery) = compose(kind, 3, false);
            let (first, last) = (coaches[0], *coaches.last().unwrap());
            assert_eq!(front_of(first), Some(true), "{kind:?}");
            assert_eq!(front_of(last), Some(false), "{kind:?}");
            assert!(!look(first, livery(0)).backward);
            assert!(look(last, livery(0)).backward);
        }
    }

    #[test]
    fn a_body_function_summons_one_display_tagged_for_its_carriage() {
        let (coaches, livery) = compose(Kind::Intercity, 1, false);
        let look = look(coaches[0], livery(0));
        let text = summon_function(&look, 1.0);
        assert_eq!(text.lines().count(), 1);
        let line = text.lines().next().unwrap();
        assert!(
            line.starts_with("$summon item_display ~ ~ ~ {Tags:[\"arnis.tp\",\"arnis.tc$(c)\"]")
        );
        assert!(line.contains("Rotation:[$(h)f,0f]"));
        assert!(line.contains(&format!(
            "\"minecraft:item_model\":\"arnis:vehicle/{}\"",
            look.name()
        )));
        assert_eq!(
            line.matches('{').count(),
            line.matches('}').count(),
            "{line}"
        );
    }

    /// The pack draws each (model, livery) once, and the data pack summons each body key
    /// once: wagons that share either must look alike.
    #[test]
    fn wagons_named_alike_look_alike() {
        let mut by_name = std::collections::HashMap::new();
        let mut by_key = std::collections::HashMap::new();
        for seed in 0..=255u8 {
            let livery = Livery { seed, ..LOCOS[0] };
            for v in 0..=255u8 {
                for coach in [Coach::Container(v), Coach::Tank(v), Coach::Boxcar(v)] {
                    let l = look(coach, livery);
                    let seen = by_name
                        .entry((l.model, l.livery.clone()))
                        .or_insert(l.colours);
                    assert_eq!(*seen, l.colours, "{} {}", l.model, l.livery);
                    let seen = by_key.entry(key(coach, livery)).or_insert(l.name());
                    assert_eq!(*seen, l.name(), "{}", key(coach, livery));
                }
            }
        }
    }

    #[test]
    fn every_carriage_of_every_train_has_a_model() {
        for kind in ALL {
            for seed in 0..40u64 {
                let (coaches, livery) = compose(kind, seed, seed % 2 == 0);
                for (k, coach) in coaches.into_iter().enumerate() {
                    let look = look(coach, livery(k % 3));
                    // Panics on a model that is not built.
                    let _ = crate::vehicle_models::model(look.model);
                }
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
}
