//! The ferries, barges and boats: their sizes, speeds, seats and colours, and the model
//! from `assets/vehicles` each is drawn with, waterline at y 0 and bow ahead. Ferries
//! that shuttle are built the same at both ends, as double-ended ferries are, since they
//! sail back stern first. `ships.rs` decides where each one sails.

use crate::element_processing::train_bodies::Livery;

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

    /// Highest point over the waterline at 1:1, in metres, `low` with the wheelhouse
    /// lowered and the top tier left off, as for low bridges: the top of its model.
    pub fn air_draft_m(self, low: bool) -> f64 {
        crate::vehicle_models::model(self.model(low)).top_m()
    }

    /// The vehicle model a boat is drawn with (`assets/vehicles`).
    pub fn model(self, low: bool) -> &'static str {
        let low = low && self.lowers();
        match (self, low) {
            (Vessel::RiverFerry, _) => "ship_river_ferry",
            (Vessel::WaterBus, _) => "ship_water_bus",
            (Vessel::CarFerry, _) => "ship_car_ferry",
            (Vessel::SeaFerry, _) => "ship_sea_ferry",
            (Vessel::Barge, false) => "ship_barge",
            (Vessel::Barge, true) => "ship_barge_low",
            (Vessel::ContainerBarge, false) => "ship_container_barge",
            (Vessel::ContainerBarge, true) => "ship_container_barge_low",
            (Vessel::TankBarge, false) => "ship_tank_barge",
            (Vessel::TankBarge, true) => "ship_tank_barge_low",
            (Vessel::RiverCruise, false) => "ship_river_cruise",
            (Vessel::RiverCruise, true) => "ship_river_cruise_low",
            (Vessel::Peniche, false) => "ship_peniche",
            (Vessel::Peniche, true) => "ship_peniche_low",
            (Vessel::TourBoat, false) => "ship_tour_boat",
            (Vessel::TourBoat, true) => "ship_tour_boat_low",
            (Vessel::Sailboat, _) => "ship_sailboat",
            (Vessel::Motorboat, _) => "ship_motorboat",
        }
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

    /// Each livery of a boat is a texture of its own in the block atlas the facade photos
    /// share. Every boat in every livery, lowered or not, takes under a third of an 8192
    /// atlas, so a world on a big river with ferries still leaves the photos room.
    #[test]
    fn every_boat_in_every_livery_leaves_the_atlas_to_the_facades() {
        use crate::element_processing::train_bodies::{look, Coach};
        let mut looks = Vec::new();
        for vessel in ALL {
            for low in [false, true] {
                if low && !vessel.lowers() {
                    continue;
                }
                for &livery in vessel.liveries() {
                    looks.push(look(Coach::Ship { vessel, low }, livery));
                }
            }
        }
        let px = crate::vehicle_models::atlas_px(&looks);
        assert!(px <= 8192 * 8192 / 3, "{px} px");
    }

    #[test]
    fn every_boat_stays_within_its_length_and_beam() {
        for v in ALL {
            let (len, beam) = v.size_m();
            for low in [false, true] {
                let [lo, hi] = crate::vehicle_models::model(v.model(low))
                    .meta()
                    .bounds
                    .map(|c| c.map(|u| u / 16.0));
                assert!(
                    lo[0] >= -beam / 2.0 - 0.1 && hi[0] <= beam / 2.0 + 0.1,
                    "{v:?} {lo:?} {hi:?}"
                );
                // The bow is at -z; a rudder or outboard may stand off the stern.
                assert!(
                    lo[2] >= -len / 2.0 - 0.1 && hi[2] <= len / 2.0 + 0.6,
                    "{v:?} {lo:?} {hi:?}"
                );
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
                let [lo, hi] = crate::vehicle_models::model(v.model(false)).meta().bounds;
                assert!((hi[2] + lo[2]).abs() / 16.0 < 0.3, "{v:?} {lo:?} {hi:?}");
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
