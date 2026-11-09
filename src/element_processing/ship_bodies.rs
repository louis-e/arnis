//! What ferries, barges and boats look like: a hull and its decks, cabins and masts, each
//! a stretched block drawn by a block display in the boat's colours, with the waterline at
//! y 0 and the bow towards +z. Ferries that shuttle are built the same at both ends, as
//! double-ended ferries are, since they sail back stern first. `ships.rs` decides where
//! each one sails.

use crate::element_processing::train_bodies::{bx, Livery, Part};

const GLASS: &str = "minecraft:tinted_glass";
const WHITE: &str = "minecraft:white_concrete";
const BLACK: &str = "minecraft:black_concrete";
const GREY: &str = "minecraft:gray_concrete";
const LIGHT: &str = "minecraft:light_gray_concrete";
const RED: &str = "minecraft:red_concrete";
const BLUE: &str = "minecraft:blue_concrete";
const GREEN: &str = "minecraft:green_concrete";
const YELLOW: &str = "minecraft:yellow_concrete";
const ORANGE: &str = "minecraft:orange_concrete";
const CYAN: &str = "minecraft:cyan_concrete";
const BROWN: &str = "minecraft:brown_terracotta";
const TEAK: &str = "minecraft:light_gray_terracotta";
const RUST: &str = "minecraft:red_terracotta";

/// A kind of boat.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Vessel {
    /// A small car and foot ferry across a river, a pontoon with a wheelhouse at its side.
    RiverFerry,
    /// A foot ferry that calls at piers, as a harbour or lake water bus.
    WaterBus,
    /// A double-ended car ferry across a lake, a strait or a wide river.
    CarFerry,
    /// A large double-ended ro-ro ferry over the sea.
    SeaFerry,
    /// A motor cargo ship, its hold covered.
    Barge,
    /// The same hull with containers on it.
    ContainerBarge,
    /// The same hull as a tanker.
    TankBarge,
    /// A river cruise ship, three decks of cabins.
    RiverCruise,
    /// A small canal barge of the old gauge.
    Peniche,
    /// A sightseeing boat with a glazed cabin.
    TourBoat,
    Sailboat,
    Motorboat,
}

impl Vessel {
    pub fn name(self) -> &'static str {
        match self {
            Vessel::RiverFerry => "river_ferry",
            Vessel::WaterBus => "water_bus",
            Vessel::CarFerry => "car_ferry",
            Vessel::SeaFerry => "sea_ferry",
            Vessel::Barge => "barge",
            Vessel::ContainerBarge => "container_barge",
            Vessel::TankBarge => "tank_barge",
            Vessel::RiverCruise => "river_cruise",
            Vessel::Peniche => "peniche",
            Vessel::TourBoat => "tour_boat",
            Vessel::Sailboat => "sailboat",
            Vessel::Motorboat => "motorboat",
        }
    }

    /// Length and beam in metres.
    pub fn size_m(self) -> (f64, f64) {
        match self {
            Vessel::RiverFerry => (24.0, 9.0),
            Vessel::WaterBus => (28.0, 7.0),
            Vessel::CarFerry => (72.0, 16.0),
            Vessel::SeaFerry => (135.0, 24.0),
            Vessel::Barge | Vessel::ContainerBarge | Vessel::TankBarge => (105.0, 11.4),
            Vessel::RiverCruise => (135.0, 11.4),
            Vessel::Peniche => (38.5, 5.05),
            Vessel::TourBoat => (40.0, 8.0),
            Vessel::Sailboat => (10.0, 3.4),
            Vessel::Motorboat => (7.0, 2.6),
        }
    }

    /// Highest point over the waterline, in metres, `low` with the wheelhouse lowered
    /// and the top tier left off, as for low bridges.
    pub fn air_draft_m(self, low: bool) -> f64 {
        self.parts_at(LIVERIES[0], low)
            .iter()
            .map(|p| f64::from(p.corner[1] + p.size[1]))
            .fold(0.0, f64::max)
    }

    /// Whether it has a lower build to pass under bridges.
    pub fn lowers(self) -> bool {
        matches!(
            self,
            Vessel::Barge
                | Vessel::ContainerBarge
                | Vessel::TankBarge
                | Vessel::RiverCruise
                | Vessel::Peniche
                | Vessel::TourBoat
        )
    }

    /// Top speed in km/h.
    pub fn cruise_kmh(self) -> f64 {
        match self {
            Vessel::RiverFerry => 8.0,
            Vessel::WaterBus => 20.0,
            Vessel::CarFerry => 22.0,
            Vessel::SeaFerry => 30.0,
            Vessel::Barge | Vessel::ContainerBarge | Vessel::TankBarge => 13.0,
            Vessel::RiverCruise => 15.0,
            Vessel::Peniche => 9.0,
            Vessel::TourBoat => 12.0,
            Vessel::Sailboat => 9.0,
            Vessel::Motorboat => 18.0,
        }
    }

    /// Starting, stopping and the sideways pull it turns with, m/s².
    pub fn accel(self) -> (f64, f64, f64) {
        match self {
            Vessel::Sailboat | Vessel::Motorboat => (0.25, 0.3, 0.4),
            Vessel::RiverFerry | Vessel::WaterBus | Vessel::TourBoat => (0.15, 0.2, 0.25),
            _ => (0.06, 0.08, 0.12),
        }
    }

    /// Seconds at a pier on the way and at the end of the crossing.
    pub fn dwell_s(self) -> (f64, f64) {
        match self {
            Vessel::RiverFerry => (20.0, 40.0),
            Vessel::WaterBus => (25.0, 45.0),
            Vessel::TourBoat => (40.0, 60.0),
            Vessel::CarFerry => (60.0, 150.0),
            Vessel::SeaFerry => (60.0, 240.0),
            _ => (40.0, 60.0),
        }
    }

    /// Whether it is built alike at both ends, so it can sail back stern first.
    #[cfg(test)]
    pub fn double_ended(self) -> bool {
        matches!(
            self,
            Vessel::RiverFerry | Vessel::WaterBus | Vessel::CarFerry | Vessel::SeaFerry
        )
    }

    /// Where passengers sit, along the boat's middle in metres from its centre, and how high
    /// their eyes are over the waterline: on deck, in the cabin or on the roof.
    pub fn seats_m(self) -> (&'static [f32], f64) {
        match self {
            Vessel::RiverFerry => (&[-6.0, 0.0, 6.0], 2.3),
            Vessel::WaterBus => (&[-6.0, 0.0, 6.0], 2.7),
            Vessel::CarFerry => (&[-24.0, -12.0, 0.0, 12.0, 24.0], 3.6),
            Vessel::SeaFerry => (&[-30.0, -15.0, 0.0, 15.0, 30.0], 18.9),
            // On the roof aft, by the wheelhouse.
            Vessel::Barge | Vessel::ContainerBarge | Vessel::TankBarge => (&[-49.5], 5.8),
            Vessel::RiverCruise => (&[-45.0, -20.0, 5.0, 30.0], 9.35),
            Vessel::Peniche => (&[0.0], 3.6),
            Vessel::TourBoat => (&[-10.0, 0.0, 8.0], 2.6),
            Vessel::Sailboat => (&[-2.5], 2.2),
            Vessel::Motorboat => (&[-1.9], 1.7),
        }
    }

    pub fn parts(self, l: Livery, low: bool) -> Vec<Part> {
        let mut parts = self.parts_at(l, low);
        apart(&mut parts);
        parts
    }

    fn parts_at(self, l: Livery, low: bool) -> Vec<Part> {
        let (len, beam) = self.size_m();
        let (len, beam) = (len as f32, beam as f32);
        match self {
            Vessel::RiverFerry => river_ferry(len, beam, l),
            Vessel::WaterBus => water_bus(len, beam, l),
            Vessel::CarFerry => car_ferry(len, beam, l, 2.0, 8.5),
            Vessel::SeaFerry => sea_ferry(len, beam, l),
            Vessel::Barge | Vessel::ContainerBarge | Vessel::TankBarge => {
                barge(self, len, beam, l, low)
            }
            Vessel::RiverCruise => river_cruise(len, beam, l, low),
            Vessel::Peniche => peniche(len, beam, l, low),
            Vessel::TourBoat => tour_boat(len, beam, l, low),
            Vessel::Sailboat => sailboat(len, beam, l),
            Vessel::Motorboat => motorboat(len, beam, l),
        }
    }

    /// The colours a boat of this kind comes in.
    pub fn liveries(self) -> &'static [Livery] {
        match self {
            Vessel::RiverFerry | Vessel::WaterBus | Vessel::CarFerry | Vessel::SeaFerry => {
                &LIVERIES[..5]
            }
            Vessel::Barge | Vessel::ContainerBarge | Vessel::TankBarge | Vessel::Peniche => {
                &LIVERIES[5..10]
            }
            Vessel::RiverCruise | Vessel::TourBoat => &LIVERIES[10..13],
            Vessel::Sailboat | Vessel::Motorboat => &LIVERIES[13..],
        }
    }
}

/// Body colour, hull, stripe, deck and roof or funnel.
const LIVERIES: [Livery; 17] = [
    Livery::new("fwb", WHITE, BLUE, BLUE, GREY, BLUE),
    Livery::new("fwr", WHITE, RED, RED, GREY, RED),
    Livery::new("fwg", WHITE, GREEN, GREEN, GREY, YELLOW),
    Livery::new("fwk", WHITE, BLACK, YELLOW, GREY, BLACK),
    Livery::new("fyb", YELLOW, BLUE, WHITE, GREY, BLUE),
    Livery::new("bk", WHITE, BLACK, RED, GREY, GREY),
    Livery::new("bb", WHITE, BLUE, WHITE, RUST, BLUE),
    Livery::new("br", WHITE, RED, BLACK, GREY, RED),
    Livery::new("bg", WHITE, GREEN, WHITE, BROWN, GREEN),
    Livery::new("bn", LIGHT, BLACK, ORANGE, GREEN, BLACK),
    Livery::new("cw", WHITE, BLUE, CYAN, LIGHT, BLUE),
    Livery::new("ck", WHITE, BLACK, WHITE, LIGHT, YELLOW),
    Livery::new("cr", WHITE, RED, WHITE, LIGHT, WHITE),
    Livery::new("sw", WHITE, WHITE, BLUE, TEAK, WHITE),
    Livery::new("sb", WHITE, BLUE, WHITE, TEAK, WHITE),
    Livery::new("sr", WHITE, RED, WHITE, TEAK, WHITE),
    Livery::new("sk", WHITE, BLACK, YELLOW, TEAK, WHITE),
];

/// A hull from `-len/2` to `len/2`, narrowing in steps at the bow, and at the stern too
/// where `both`. Its top is `top` over the waterline and it reaches `down` under it.
fn hull(len: f32, beam: f32, down: f32, top: f32, block: &'static str, both: bool) -> Vec<Part> {
    let (half, hw) = (len / 2.0, beam / 2.0);
    let taper = (beam * 0.9).min(len * 0.2);
    let mut out = Vec::new();
    let aft = if both { -half + taper } else { -half };
    out.push(bx(block, (-hw, hw), (-down, top), (aft, half - taper)));
    let steps = [(0.4, 0.82), (0.75, 0.55), (1.0, 0.28)];
    let mut from = 0.0;
    for (to, width) in steps {
        let (z0, z1) = (half - taper * (1.0 - from), half - taper * (1.0 - to));
        let w = hw * width;
        let (y0, y1) = (-down * (1.0 - to * 0.6), top + 0.25 * to);
        out.push(bx(block, (-w, w), (y0, y1), (z0, z1)));
        if both {
            out.push(bx(block, (-w, w), (y0, y1), (-z1, -z0)));
        }
        from = to;
    }
    out
}

/// A thin band round the hull's side at `y`.
fn band(len: f32, beam: f32, y: (f32, f32), block: &'static str, both: bool) -> Part {
    let taper = (beam * 0.9).min(len * 0.2);
    let aft = if both { -len / 2.0 + taper } else { -len / 2.0 };
    bx(
        block,
        (-beam / 2.0 - 0.04, beam / 2.0 + 0.04),
        y,
        (aft, len / 2.0 - taper),
    )
}

fn river_ferry(len: f32, beam: f32, l: Livery) -> Vec<Part> {
    let (half, hw) = (len / 2.0, beam / 2.0);
    let mut out = vec![
        bx(l.lower, (-hw, hw), (-0.8, 0.6), (-half + 1.2, half - 1.2)),
        bx(
            l.stripe,
            (-hw - 0.04, hw + 0.04),
            (0.3, 0.6),
            (-half + 1.2, half - 1.2),
        ),
        bx(
            GREY,
            (-hw + 0.3, hw - 0.3),
            (0.6, 0.7),
            (-half + 1.2, half - 1.2),
        ),
    ];
    // Ramps at both ends.
    for end in [-1.0f32, 1.0] {
        let (a, b) = (end * (half - 1.2), end * half);
        out.push(bx(
            GREY,
            (-hw + 0.8, hw - 0.8),
            (0.3, 0.55),
            (a.min(b), a.max(b)),
        ));
    }
    // Railings along both sides, a wheelhouse on one of them.
    for side in [-1.0f32, 1.0] {
        let x = side * (hw - 0.1);
        out.push(bx(
            WHITE,
            (x - 0.06, x + 0.06),
            (0.7, 1.7),
            (-half + 2.0, half - 2.0),
        ));
    }
    out.extend([
        bx(l.body, (hw - 2.2, hw - 0.2), (0.7, 3.0), (-1.4, 1.4)),
        bx(GLASS, (hw - 2.25, hw - 0.15), (1.9, 2.7), (-1.3, 1.3)),
        bx(l.roof, (hw - 2.4, hw), (3.0, 3.2), (-1.6, 1.6)),
    ]);
    out.extend(parked_car(-hw + 2.2, -4.5, BLUE));
    out.extend(parked_car(-hw + 2.2, 2.0, WHITE));
    out
}

/// A car standing on a deck at `y` 0.6, as ferries carry them.
fn parked_car(x: f32, z: f32, block: &'static str) -> Vec<Part> {
    vec![
        bx(block, (x - 0.9, x + 0.9), (0.85, 1.55), (z - 2.2, z + 2.2)),
        bx(GLASS, (x - 0.8, x + 0.8), (1.55, 2.05), (z - 1.2, z + 1.0)),
        bx(BLACK, (x - 0.95, x + 0.95), (0.7, 1.1), (z - 1.5, z - 0.9)),
        bx(BLACK, (x - 0.95, x + 0.95), (0.7, 1.1), (z + 0.9, z + 1.5)),
    ]
}

fn water_bus(len: f32, beam: f32, l: Livery) -> Vec<Part> {
    let (half, hw) = (len / 2.0, beam / 2.0);
    let mut out = hull(len, beam, 1.2, 1.0, l.lower, true);
    out.push(band(len, beam, (0.65, 1.0), l.stripe, true));
    let cabin = half * 0.62;
    out.extend([
        bx(
            l.door,
            (-hw + 0.25, hw - 0.25),
            (1.0, 1.1),
            (-half + 2.0, half - 2.0),
        ),
        bx(l.body, (-hw + 0.45, hw - 0.45), (1.1, 3.2), (-cabin, cabin)),
        bx(
            GLASS,
            (-hw + 0.4, hw - 0.4),
            (1.9, 2.8),
            (-cabin + 0.4, cabin - 0.4),
        ),
        bx(
            l.roof,
            (-hw + 0.35, hw - 0.35),
            (3.2, 3.4),
            (-cabin - 0.3, cabin + 0.3),
        ),
        // The wheelhouse on top, glazed all round.
        bx(l.body, (-1.3, 1.3), (3.4, 4.9), (-1.3, 1.3)),
        bx(GLASS, (-1.35, 1.35), (4.0, 4.7), (-1.2, 1.2)),
        bx(l.roof, (-1.5, 1.5), (4.9, 5.1), (-1.5, 1.5)),
    ]);
    for side in [-1.0f32, 1.0] {
        let x = side * (hw - 0.3);
        out.push(bx(WHITE, (x - 0.05, x + 0.05), (3.4, 4.3), (-cabin, cabin)));
    }
    out
}

fn car_ferry(len: f32, beam: f32, l: Livery, deck: f32, upper: f32) -> Vec<Part> {
    let (half, hw) = (len / 2.0, beam / 2.0);
    let mut out = hull(len, beam, 2.5, deck, l.lower, true);
    out.push(band(len, beam, (deck - 0.45, deck), l.stripe, true));
    out.push(bx(
        GREY,
        (-hw + 1.4, hw - 1.4),
        (deck, deck + 0.1),
        (-half + 2.0, half - 2.0),
    ));
    // Superstructure along both sides over the open car deck, bridged amidships.
    let side = half * 0.62;
    for s in [-1.0f32, 1.0] {
        let (a, b) = (s * (hw - 1.6), s * hw);
        let x = (a.min(b), a.max(b));
        out.push(bx(l.body, x, (deck, upper), (-side, side)));
        let gx = (x.0 - 0.04, x.1 + 0.04);
        out.push(bx(
            GLASS,
            gx,
            (deck + 1.4, deck + 2.4),
            (-side + 1.0, side - 1.0),
        ));
        out.push(bx(
            GLASS,
            gx,
            (deck + 4.0, deck + 5.0),
            (-side + 1.0, side - 1.0),
        ));
    }
    out.extend([
        bx(
            l.body,
            (-hw, hw),
            (upper, upper + 0.6),
            (-side * 0.7, side * 0.7),
        ),
        bx(
            l.body,
            (-hw * 0.6, hw * 0.6),
            (upper + 0.6, upper + 3.2),
            (-4.0, 4.0),
        ),
        bx(
            GLASS,
            (-hw * 0.6 - 0.05, hw * 0.6 + 0.05),
            (upper + 1.6, upper + 2.8),
            (-3.8, 3.8),
        ),
        bx(
            l.roof,
            (-hw * 0.6 - 0.2, hw * 0.6 + 0.2),
            (upper + 3.2, upper + 3.5),
            (-4.2, 4.2),
        ),
    ]);
    for s in [-1.0f32, 1.0] {
        let x = s * (hw - 0.9);
        out.push(bx(
            l.roof,
            (x - 0.7, x + 0.7),
            (upper + 0.6, upper + 5.0),
            (-1.2, 1.2),
        ));
        out.push(bx(
            BLACK,
            (x - 0.72, x + 0.72),
            (upper + 4.4, upper + 5.0),
            (-1.22, 1.22),
        ));
    }
    // Two lanes of cars on the deck.
    let colours = [RED, WHITE, BLACK, BLUE, LIGHT, GREY];
    let mut k = 0;
    for x in [-2.6f32, 2.6] {
        let mut z = -half + 9.0;
        while z < half - 9.0 {
            let block = colours[k % colours.len()];
            out.extend(parked_car(x, z, block).into_iter().map(|mut p| {
                p.corner[1] += deck - 0.6;
                p
            }));
            k += 1;
            z += 11.0;
        }
    }
    out
}

fn sea_ferry(len: f32, beam: f32, l: Livery) -> Vec<Part> {
    let (half, hw) = (len / 2.0, beam / 2.0);
    let mut out = hull(len, beam, 5.5, 4.0, l.lower, true);
    out.push(band(len, beam, (3.4, 4.0), l.stripe, true));
    let body = half * 0.78;
    out.extend([
        bx(l.body, (-hw, hw), (4.0, 14.0), (-body, body)),
        bx(
            l.body,
            (-hw + 1.5, hw - 1.5),
            (14.0, 17.0),
            (-body * 0.6, body * 0.6),
        ),
        bx(
            l.roof,
            (-hw + 1.2, hw - 1.2),
            (17.0, 17.3),
            (-body * 0.62, body * 0.62),
        ),
        bx(l.body, (-hw * 0.7, hw * 0.7), (17.3, 20.5), (-5.0, 5.0)),
        bx(
            GLASS,
            (-hw * 0.7 - 0.05, hw * 0.7 + 0.05),
            (18.4, 19.8),
            (-4.8, 4.8),
        ),
    ]);
    for (a, b) in [(5.5, 6.8), (8.0, 9.3), (10.5, 11.8), (14.8, 16.0)] {
        let w = if a > 14.0 { hw - 1.5 } else { hw };
        let reach = if a > 14.0 { body * 0.6 } else { body };
        out.push(bx(
            GLASS,
            (-w - 0.05, w + 0.05),
            (a, b),
            (-reach + 1.5, reach - 1.5),
        ));
    }
    // Bow doors' outline at both ends.
    for end in [-1.0f32, 1.0] {
        let z = end * body;
        out.push(bx(
            l.stripe,
            (-hw * 0.7, hw * 0.7),
            (4.0, 4.6),
            (z - 0.6, z + 0.6),
        ));
    }
    for s in [-1.0f32, 1.0] {
        let x = s * (hw - 3.0);
        out.push(bx(l.roof, (x - 1.4, x + 1.4), (17.3, 23.0), (-2.0, 2.0)));
        out.push(bx(BLACK, (x - 1.42, x + 1.42), (22.2, 23.0), (-2.02, 2.02)));
    }
    out
}

fn barge(kind: Vessel, len: f32, beam: f32, l: Livery, low: bool) -> Vec<Part> {
    let (half, hw) = (len / 2.0, beam / 2.0);
    let mut out = hull(len, beam, 2.8, 1.2, l.lower, false);
    out.push(band(len, beam, (0.9, 1.2), l.stripe, false));
    let (hold_from, hold_to) = (-half + 13.0, half - 9.0);
    match kind {
        Vessel::ContainerBarge => {
            out.push(bx(
                GREY,
                (-hw + 0.5, hw - 0.5),
                (1.2, 1.8),
                (hold_from, hold_to),
            ));
            let boxes = [RED, BLUE, ORANGE, GREEN, LIGHT, BROWN, CYAN, WHITE];
            let tiers = if low { 1 } else { 2 };
            let bay = (hold_to - hold_from) / 7.0;
            for tier in 0..tiers {
                let y = 1.8 + tier as f32 * 2.6;
                for (row, x) in [(-hw + 0.7, -0.1), (0.1, hw - 0.7)].into_iter().enumerate() {
                    for b in 0..7 {
                        if tier == 1 && (b + row) % 3 == 0 {
                            continue;
                        }
                        let z = hold_from + bay * b as f32;
                        let block = boxes[(b * 3 + row * 5 + tier * 2) % boxes.len()];
                        out.push(bx(block, x, (y, y + 2.55), (z + 0.15, z + bay - 0.15)));
                    }
                }
            }
        }
        Vessel::TankBarge => {
            out.extend([
                bx(
                    RUST,
                    (-hw + 0.3, hw - 0.3),
                    (1.2, 1.35),
                    (hold_from, hold_to),
                ),
                bx(
                    LIGHT,
                    (-0.3, 0.3),
                    (1.35, 1.85),
                    (hold_from + 2.0, hold_to - 2.0),
                ),
                bx(
                    LIGHT,
                    (1.2, 1.6),
                    (1.35, 1.75),
                    (hold_from + 2.0, hold_to - 2.0),
                ),
                bx(
                    LIGHT,
                    (-1.6, -1.2),
                    (1.35, 1.75),
                    (hold_from + 2.0, hold_to - 2.0),
                ),
            ]);
            let mut z = hold_from + 6.0;
            while z < hold_to - 4.0 {
                out.push(bx(GREY, (-0.9, 0.9), (1.35, 2.3), (z - 0.9, z + 0.9)));
                z += 14.0;
            }
        }
        _ => {
            // Hatch covers over the hold, in the deck colour.
            out.push(bx(
                l.lower,
                (-hw + 0.5, hw - 0.5),
                (1.2, 2.3),
                (hold_from, hold_to),
            ));
            let n = 8;
            let lid = (hold_to - hold_from) / n as f32;
            for k in 0..n {
                let z = hold_from + lid * k as f32;
                out.push(bx(
                    l.door,
                    (-hw + 0.6, hw - 0.6),
                    (2.3, 2.6),
                    (z + 0.08, z + lid - 0.08),
                ));
            }
        }
    }
    // Accommodation and wheelhouse aft, the wheelhouse let down for low bridges.
    out.extend([
        bx(
            l.body,
            (-hw + 0.4, hw - 0.4),
            (1.2, 4.0),
            (-half + 1.0, -half + 11.0),
        ),
        bx(
            GLASS,
            (-hw + 0.35, hw - 0.35),
            (2.2, 3.2),
            (-half + 1.6, -half + 10.4),
        ),
        bx(
            l.roof,
            (-hw + 0.3, hw - 0.3),
            (4.0, 4.2),
            (-half + 0.8, -half + 11.2),
        ),
    ]);
    let wheel = if low { 4.2 } else { 5.6 };
    out.extend([
        bx(
            l.body,
            (-2.4, 2.4),
            (wheel, wheel + 1.8),
            (-half + 7.0, -half + 10.6),
        ),
        bx(
            GLASS,
            (-2.45, 2.45),
            (wheel + 0.6, wheel + 1.6),
            (-half + 7.2, -half + 10.65),
        ),
        bx(
            l.roof,
            (-2.6, 2.6),
            (wheel + 1.8, wheel + 2.0),
            (-half + 6.8, -half + 10.8),
        ),
    ]);
    if !low {
        out.push(bx(
            GREY,
            (-0.25, 0.25),
            (4.2, wheel),
            (-half + 8.5, -half + 9.0),
        ));
    }
    out.push(bx(
        l.roof,
        (-0.15, 0.15),
        (1.2, 3.2),
        (half - 3.5, half - 3.2),
    ));
    out
}

fn river_cruise(len: f32, beam: f32, l: Livery, low: bool) -> Vec<Part> {
    let (half, hw) = (len / 2.0, beam / 2.0);
    let mut out = hull(len, beam, 1.6, 1.0, l.lower, false);
    out.push(band(len, beam, (0.7, 1.0), l.stripe, false));
    let (from, to) = (-half + 3.0, half - 12.0);
    out.push(bx(l.body, (-hw + 0.2, hw - 0.2), (1.0, 7.6), (from, to)));
    for (a, b) in [(1.7, 2.7), (3.9, 5.0), (6.1, 7.2)] {
        out.push(bx(
            GLASS,
            (-hw + 0.15, hw - 0.15),
            (a, b),
            (from + 1.0, to - 1.0),
        ));
    }
    out.extend([
        bx(l.door, (-hw + 0.3, hw - 0.3), (7.6, 7.75), (from, to)),
        bx(WHITE, (-hw + 0.25, -hw + 0.35), (7.75, 8.6), (from, to)),
        bx(WHITE, (hw - 0.35, hw - 0.25), (7.75, 8.6), (from, to)),
        bx(l.roof, (-2.0, 2.0), (7.75, 8.6), (from + 30.0, from + 34.0)),
    ]);
    let wheel = if low { 7.75 } else { 8.6 };
    out.extend([
        bx(
            l.body,
            (-hw + 1.0, hw - 1.0),
            (wheel, wheel + 1.8),
            (to - 7.0, to - 2.0),
        ),
        bx(
            GLASS,
            (-hw + 0.95, hw - 0.95),
            (wheel + 0.7, wheel + 1.6),
            (to - 6.8, to - 1.95),
        ),
    ]);
    out
}

fn peniche(len: f32, beam: f32, l: Livery, low: bool) -> Vec<Part> {
    let (half, hw) = (len / 2.0, beam / 2.0);
    let mut out = hull(len, beam, 1.8, 1.0, l.lower, false);
    out.push(band(len, beam, (0.75, 1.0), l.stripe, false));
    out.extend([
        bx(
            l.door,
            (-hw + 0.35, hw - 0.35),
            (1.0, 2.0),
            (-half + 8.0, half - 4.0),
        ),
        bx(
            l.body,
            (-hw + 0.4, hw - 0.4),
            (1.0, 3.0),
            (-half + 0.6, -half + 6.5),
        ),
        bx(
            GLASS,
            (-hw + 0.35, hw - 0.35),
            (1.9, 2.6),
            (-half + 1.0, -half + 6.1),
        ),
        bx(
            l.roof,
            (-hw + 0.3, hw - 0.3),
            (3.0, 3.15),
            (-half + 0.4, -half + 6.7),
        ),
    ]);
    if !low {
        out.extend([
            bx(l.body, (-1.3, 1.3), (3.15, 4.6), (-half + 2.0, -half + 4.5)),
            bx(
                GLASS,
                (-1.35, 1.35),
                (3.6, 4.4),
                (-half + 2.1, -half + 4.55),
            ),
        ]);
    }
    out
}

fn tour_boat(len: f32, beam: f32, l: Livery, low: bool) -> Vec<Part> {
    let (half, hw) = (len / 2.0, beam / 2.0);
    let mut out = hull(len, beam, 1.4, 1.0, l.lower, false);
    out.push(band(len, beam, (0.65, 1.0), l.stripe, false));
    let (from, to) = (-half + 3.0, half - 7.0);
    out.extend([
        bx(l.body, (-hw + 0.3, hw - 0.3), (1.0, 1.6), (from, to)),
        bx(
            GLASS,
            (-hw + 0.3, hw - 0.3),
            (1.6, 3.2),
            (from + 0.3, to - 0.3),
        ),
        bx(l.roof, (-hw + 0.25, hw - 0.25), (3.2, 3.4), (from, to)),
        bx(l.door, (-hw + 0.8, hw - 0.8), (1.0, 1.1), (to, half - 2.0)),
    ]);
    if !low {
        out.extend([
            bx(WHITE, (-hw + 0.3, -hw + 0.4), (3.4, 4.3), (from, to)),
            bx(WHITE, (hw - 0.4, hw - 0.3), (3.4, 4.3), (from, to)),
            bx(l.body, (-1.4, 1.4), (3.4, 5.0), (to - 4.0, to - 1.0)),
            bx(GLASS, (-1.45, 1.45), (4.0, 4.8), (to - 3.8, to - 0.95)),
        ]);
    }
    out
}

fn sailboat(len: f32, beam: f32, l: Livery) -> Vec<Part> {
    let half = len / 2.0;
    let mut out = hull(len, beam, 0.5, 0.8, l.lower, false);
    out.push(band(len, beam, (0.55, 0.8), l.stripe, false));
    out.extend([
        bx(
            l.door,
            (-beam / 2.0 + 0.2, beam / 2.0 - 0.2),
            (0.8, 0.85),
            (-half, half - 2.0),
        ),
        bx(l.body, (-0.9, 0.9), (0.85, 1.45), (-1.6, 1.4)),
        bx(GLASS, (-0.95, 0.95), (1.05, 1.3), (-1.4, 1.2)),
        // Mast and boom, the mainsail stepped to a point, and the jib.
        bx(WHITE, (-0.07, 0.07), (0.85, 12.5), (1.3, 1.45)),
        bx(GREY, (-0.06, 0.06), (1.6, 1.72), (-3.6, 1.35)),
    ]);
    let sail: [(f32, f32, f32); 4] = [
        (1.8, 4.4, 4.8),
        (4.4, 7.2, 3.6),
        (7.2, 9.8, 2.3),
        (9.8, 12.2, 1.0),
    ];
    for (a, b, reach) in sail {
        out.push(bx(WHITE, (-0.03, 0.03), (a, b), (1.25 - reach, 1.25)));
    }
    for (a, b, reach) in [(1.2, 4.0, 3.0), (4.0, 7.0, 2.0), (7.0, 10.5, 0.9)] {
        out.push(bx(WHITE, (-0.03, 0.03), (a, b), (1.5, 1.5 + reach)));
    }
    out
}

fn motorboat(len: f32, beam: f32, l: Livery) -> Vec<Part> {
    let half = len / 2.0;
    let mut out = hull(len, beam, 0.45, 0.75, l.lower, false);
    out.push(band(len, beam, (0.45, 0.75), l.stripe, false));
    out.extend([
        bx(
            l.door,
            (-beam / 2.0 + 0.15, beam / 2.0 - 0.15),
            (0.75, 0.8),
            (-half, half - 1.5),
        ),
        bx(
            l.body,
            (-beam / 2.0 + 0.2, beam / 2.0 - 0.2),
            (0.8, 1.15),
            (-0.2, 1.6),
        ),
        bx(
            GLASS,
            (-beam / 2.0 + 0.25, beam / 2.0 - 0.25),
            (1.15, 1.6),
            (0.9, 1.15),
        ),
        bx(GREY, (-0.9, -0.2), (0.8, 1.2), (-2.4, -1.4)),
        bx(GREY, (0.2, 0.9), (0.8, 1.2), (-2.4, -1.4)),
        bx(BLACK, (-0.35, 0.35), (0.3, 1.1), (-half - 0.5, -half)),
    ]);
    out
}

/// How far a face is pushed out of one it would lie in, so the two never flicker.
const APART: f32 = 0.03;

/// The faces two differently coloured parts share, by axis and whether it is the far side
/// of both, where they overlap facing the same way.
pub(crate) fn shared_faces(a: &Part, b: &Part) -> Vec<(usize, bool)> {
    if a.tilt != 0.0 || b.tilt != 0.0 || a.block == b.block {
        return Vec::new();
    }
    (0..3)
        .flat_map(|axis| [(axis, false), (axis, true)])
        .filter(|&(axis, far)| {
            let face = |p: &Part| p.corner[axis] + if far { p.size[axis] } else { 0.0 };
            (face(a) - face(b)).abs() <= 0.02
                && (0..3).filter(|&k| k != axis).all(|k| {
                    let lo = a.corner[k].max(b.corner[k]);
                    let hi = (a.corner[k] + a.size[k]).min(b.corner[k] + b.size[k]);
                    hi - lo > 1e-3
                })
        })
        .collect()
}

/// Pushes the smaller of two parts out of a face they share, so the detail, a stripe on
/// a hull, a window in a side, a lamp on a bumper, shows rather than both in turn.
pub(crate) fn apart(parts: &mut [Part]) {
    let volume = |p: &Part| p.size[0] * p.size[1] * p.size[2];
    for _ in 0..4 {
        let mut moved = false;
        for j in 1..parts.len() {
            for i in 0..j {
                for (axis, far) in shared_faces(&parts[i], &parts[j]) {
                    let (fixed, k) = if volume(&parts[i]) < volume(&parts[j]) {
                        (j, i)
                    } else {
                        (i, j)
                    };
                    let f = &parts[fixed];
                    let to = f.corner[axis] + if far { f.size[axis] } else { 0.0 };
                    let p = &mut parts[k];
                    if far {
                        p.size[axis] = to + APART - p.corner[axis];
                    } else {
                        let end = p.corner[axis] + p.size[axis];
                        p.corner[axis] = to - APART;
                        p.size[axis] = end - p.corner[axis];
                    }
                    moved = true;
                }
            }
        }
        if !moved {
            break;
        }
    }
}

/// Parts wholly inside a part of another colour, which never show.
#[cfg(test)]
pub(crate) fn buried(parts: &[Part]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, a) in parts.iter().enumerate() {
        for (j, b) in parts.iter().enumerate() {
            if i == j || a.block == b.block || a.tilt != 0.0 || b.tilt != 0.0 {
                continue;
            }
            let inside = (0..3).all(|k| {
                a.corner[k] >= b.corner[k] - 1e-4
                    && a.corner[k] + a.size[k] <= b.corner[k] + b.size[k] + 1e-4
            });
            if inside {
                out.push(format!(
                    "{} {:?}+{:?} in {}",
                    a.block, a.corner, a.size, b.block
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Vessel; 12] = [
        Vessel::RiverFerry,
        Vessel::WaterBus,
        Vessel::CarFerry,
        Vessel::SeaFerry,
        Vessel::Barge,
        Vessel::ContainerBarge,
        Vessel::TankBarge,
        Vessel::RiverCruise,
        Vessel::Peniche,
        Vessel::TourBoat,
        Vessel::Sailboat,
        Vessel::Motorboat,
    ];

    fn flickers(parts: &[Part]) -> Vec<String> {
        let mut out = Vec::new();
        for (i, a) in parts.iter().enumerate() {
            for b in &parts[i + 1..] {
                for (axis, far) in shared_faces(a, b) {
                    out.push(format!(
                        "{} {:?}+{:?} / {} {:?}+{:?} axis {axis} far {far}",
                        a.block, a.corner, a.size, b.block, b.corner, b.size
                    ));
                }
            }
        }
        out
    }

    #[test]
    fn no_two_parts_of_a_boat_flicker_against_each_other_or_hide_one_another() {
        let mut found = Vec::new();
        for v in ALL {
            for &l in v.liveries() {
                for low in [false, true] {
                    if low && !v.lowers() {
                        continue;
                    }
                    let parts = v.parts(l, low);
                    for f in flickers(&parts).into_iter().chain(buried(&parts)) {
                        found.push(format!("{v:?} {} low {low}: {f}", l.name));
                    }
                }
            }
        }
        found.sort();
        found.dedup();
        assert!(
            found.is_empty(),
            "{} flickering faces:\n{}",
            found.len(),
            found.join("\n")
        );
    }

    #[test]
    fn every_boat_stays_within_its_length_and_beam_and_uses_known_colours() {
        for v in ALL {
            let (len, beam) = v.size_m();
            for &l in v.liveries() {
                for low in [false, true] {
                    for p in v.parts(l, low) {
                        let (x0, x1) = (p.corner[0], p.corner[0] + p.size[0]);
                        let (z0, z1) = (p.corner[2], p.corner[2] + p.size[2]);
                        assert!(p.size.iter().all(|&s| s > 0.0), "{v:?} {p:?}");
                        assert!(
                            x0 >= -(beam as f32) / 2.0 - 0.1 && x1 <= beam as f32 / 2.0 + 0.1,
                            "{v:?} {p:?}"
                        );
                        assert!(
                            z0 >= -(len as f32) / 2.0 - 0.6 && z1 <= len as f32 / 2.0 + 0.1,
                            "{v:?} {p:?}"
                        );
                        assert!(crate::bedrock_addon::palette_has(p.block), "{}", p.block);
                    }
                }
            }
        }
    }

    #[test]
    fn a_lowered_boat_passes_under_less_and_a_ferry_is_alike_at_both_ends() {
        for v in ALL {
            if v.lowers() {
                assert!(v.air_draft_m(true) < v.air_draft_m(false), "{v:?}");
            } else {
                assert_eq!(v.air_draft_m(true), v.air_draft_m(false), "{v:?}");
            }
            if v.double_ended() {
                let parts = v.parts(v.liveries()[0], false);
                let reach = |sign: f32| {
                    parts
                        .iter()
                        .map(|p| {
                            sign * if sign > 0.0 {
                                p.corner[2] + p.size[2]
                            } else {
                                p.corner[2]
                            }
                        })
                        .fold(f32::MIN, f32::max)
                };
                assert!((reach(1.0) - reach(-1.0)).abs() < 0.3, "{v:?}");
            }
        }
    }

    #[test]
    fn passengers_sit_on_board() {
        for v in ALL {
            let (len, _) = v.size_m();
            let (spots, eye) = v.seats_m();
            assert!(eye > 1.0 && eye < v.air_draft_m(false) + 2.0, "{v:?}");
            assert!(!spots.is_empty());
            assert!(spots.iter().all(|z| z.abs() < len as f32 / 2.0), "{v:?}");
        }
    }
}
