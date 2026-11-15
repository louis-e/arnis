//! The aircraft: their sizes, speeds, seats and colours, and the model from
//! `assets/vehicles` each is drawn with. A model stands on its wheels at y 0 with its
//! nose ahead and its main wheels at the origin, so it turns and lifts its nose about
//! them. `aircraft.rs` decides where each one taxis and flies.

use crate::element_processing::train_bodies::Livery;

const WHITE: &str = "minecraft:white_concrete";
const LIGHT: &str = "minecraft:light_gray_concrete";
const GREY: &str = "minecraft:gray_concrete";
const BLACK: &str = "minecraft:black_concrete";
const RED: &str = "minecraft:red_concrete";
const ORANGE: &str = "minecraft:orange_concrete";
const YELLOW: &str = "minecraft:yellow_concrete";
const GREEN: &str = "minecraft:green_concrete";
const CYAN: &str = "minecraft:cyan_concrete";
const SKY: &str = "minecraft:light_blue_concrete";
const BLUE: &str = "minecraft:blue_concrete";
const PURPLE: &str = "minecraft:purple_concrete";

/// A kind of aircraft.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Airframe {
    /// A four-seat piston single with a high wing and fixed wheels.
    Light,
    /// A light business jet, its engines at the tail.
    Bizjet,
    /// A twin turboprop airliner with a high wing.
    Regional,
    /// A single-aisle twinjet.
    Narrow,
    /// A twin-aisle twinjet.
    Wide,
}

/// Every airframe, smallest first.
pub const ALL: [Airframe; 5] = [
    Airframe::Light,
    Airframe::Bizjet,
    Airframe::Regional,
    Airframe::Narrow,
    Airframe::Wide,
];

impl Airframe {
    pub fn name(self) -> &'static str {
        match self {
            Airframe::Light => "light",
            Airframe::Bizjet => "bizjet",
            Airframe::Regional => "regional",
            Airframe::Narrow => "narrow",
            Airframe::Wide => "wide",
        }
    }

    /// The vehicle model it is drawn with (`assets/vehicles`).
    pub fn model(self) -> &'static str {
        match self {
            Airframe::Light => "plane_light",
            Airframe::Bizjet => "plane_bizjet",
            Airframe::Regional => "plane_regional",
            Airframe::Narrow => "plane_narrow",
            Airframe::Wide => "plane_wide",
        }
    }

    /// Length and wingspan in metres.
    pub fn size_m(self) -> (f64, f64) {
        match self {
            Airframe::Light => (8.3, 11.0),
            Airframe::Bizjet => (15.6, 16.3),
            Airframe::Regional => (27.2, 27.05),
            Airframe::Narrow => (37.6, 35.8),
            Airframe::Wide => (65.0, 63.0),
        }
    }

    /// How far ahead of its main wheels the nose reaches and how far behind them the
    /// tail, in metres.
    pub fn reach_m(self) -> (f64, f64) {
        match self {
            Airframe::Light => (2.9, 5.4),
            Airframe::Bizjet => (8.2, 7.4),
            Airframe::Regional => (13.2, 14.0),
            Airframe::Narrow => (17.7, 19.9),
            Airframe::Wide => (33.5, 31.5),
        }
    }

    /// Whether its wheels fold away in flight.
    #[cfg(test)]
    pub fn retracts(self) -> bool {
        self != Airframe::Light
    }

    /// The shortest runway it uses, in metres, and whether that has to be paved.
    pub fn runway_m(self) -> (f64, bool) {
        match self {
            Airframe::Light => (320.0, false),
            Airframe::Bizjet => (900.0, true),
            Airframe::Regional => (1100.0, true),
            Airframe::Narrow => (1700.0, true),
            Airframe::Wide => (2400.0, true),
        }
    }

    /// Speed it lifts off at and crosses the threshold at, m/s.
    pub fn speeds(self) -> (f64, f64) {
        match self {
            Airframe::Light => (28.0, 28.0),
            Airframe::Bizjet => (56.0, 54.0),
            Airframe::Regional => (58.0, 56.0),
            Airframe::Narrow => (72.0, 68.0),
            Airframe::Wide => (79.0, 72.0),
        }
    }

    /// Speeding up along the runway and braking on it, m/s².
    pub fn accel(self) -> (f64, f64) {
        match self {
            Airframe::Light => (1.8, 1.6),
            Airframe::Bizjet => (2.8, 2.2),
            Airframe::Regional => (2.3, 2.0),
            Airframe::Narrow => (2.2, 1.9),
            Airframe::Wide => (1.9, 1.7),
        }
    }

    /// The angle it climbs at and the nose-up attitude it climbs and touches down with,
    /// in degrees.
    pub fn angles(self) -> (f64, f64, f64) {
        match self {
            Airframe::Light => (6.0, 9.0, 5.0),
            Airframe::Bizjet => (12.0, 15.0, 5.0),
            Airframe::Regional => (9.0, 12.0, 4.0),
            Airframe::Narrow => (10.0, 14.0, 5.0),
            Airframe::Wide => (8.0, 12.0, 5.0),
        }
    }

    /// Whether it flies the light aircraft's tight, low circuit.
    pub fn slow(self) -> bool {
        self == Airframe::Light
    }

    /// Where passengers sit, along the cabin's middle in metres ahead of the main wheels
    /// (behind them is negative, the other way round from the model's z), and how high
    /// their eyes are over the ground.
    pub fn seats_m(self) -> (&'static [f32], f64) {
        match self {
            Airframe::Light => (&[0.44], 1.72),
            Airframe::Bizjet => (&[2.5, 0.9], 1.75),
            Airframe::Regional => (&[8.0, 4.25, -0.25, -4.0], 2.72),
            Airframe::Narrow => (&[12.0, 7.0, 1.0, -5.0, -11.0], 4.5),
            Airframe::Wide => (&[24.0, 16.0, 8.0, 0.0, -8.0, -16.0], 5.81),
        }
    }

    /// Width of the cabin, for the box that takes a click to get on, in metres.
    pub fn cabin_m(self) -> f64 {
        match self {
            Airframe::Light => 1.6,
            Airframe::Bizjet => 2.0,
            Airframe::Regional => 3.0,
            Airframe::Narrow => 4.0,
            Airframe::Wide => 6.0,
        }
    }

    /// The colours an aircraft of this kind comes in.
    pub fn liveries(self) -> &'static [Livery] {
        match self {
            Airframe::Light => &LIVERIES[11..15],
            Airframe::Bizjet => &LIVERIES[8..11],
            Airframe::Regional => &LIVERIES[4..8],
            Airframe::Narrow => &LIVERIES[..6],
            Airframe::Wide => &LIVERIES[..3],
        }
    }
}

/// Fuselage, belly, cheatline, fin and engines. No airline is meant by any of them.
const LIVERIES: [Livery; 15] = [
    Livery::new("wb", WHITE, LIGHT, WHITE, BLUE, WHITE),
    Livery::new("wr", WHITE, WHITE, WHITE, RED, RED),
    Livery::new("wy", WHITE, BLUE, YELLOW, YELLOW, BLUE),
    Livery::new("wo", WHITE, ORANGE, WHITE, ORANGE, ORANGE),
    Livery::new("wg", WHITE, LIGHT, GREEN, GREEN, WHITE),
    Livery::new("ws", WHITE, SKY, WHITE, SKY, WHITE),
    Livery::new("wp", WHITE, PURPLE, WHITE, PURPLE, PURPLE),
    Livery::new("wc", WHITE, LIGHT, CYAN, CYAN, CYAN),
    Livery::new("jw", WHITE, WHITE, GREY, WHITE, WHITE),
    Livery::new("jk", WHITE, BLACK, RED, BLACK, BLACK),
    Livery::new("jb", WHITE, BLUE, LIGHT, BLUE, WHITE),
    Livery::new("lr", WHITE, WHITE, RED, RED, WHITE),
    Livery::new("lb", WHITE, WHITE, BLUE, BLUE, WHITE),
    Livery::new("ly", YELLOW, YELLOW, BLACK, YELLOW, YELLOW),
    Livery::new("lg", WHITE, GREEN, GREEN, GREEN, WHITE),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Each livery of an aircraft is a texture of its own in the block atlas the facade
    /// photos share. Every aircraft in every livery takes under a sixth of an 8192 atlas.
    #[test]
    fn every_aircraft_in_every_livery_leaves_the_atlas_to_the_facades() {
        use crate::element_processing::train_bodies::{look, Coach};
        let mut looks = Vec::new();
        for frame in ALL {
            for &livery in frame.liveries() {
                looks.push(look(Coach::Plane(frame), livery));
            }
        }
        let px = crate::vehicle_models::atlas_px(&looks);
        assert!(px <= 8192 * 8192 / 6, "{px} px");
    }

    #[test]
    fn every_aircraft_is_the_size_it_says_and_stands_on_its_main_wheels() {
        for frame in ALL {
            let (len, span) = frame.size_m();
            let [lo, hi] = crate::vehicle_models::model(frame.model())
                .meta()
                .bounds
                .map(|c| c.map(|u| u / 16.0));
            assert!(
                (hi[2] - lo[2] - len).abs() < len * 0.06,
                "{frame:?} {lo:?} {hi:?}"
            );
            assert!(
                (hi[0] - lo[0] - span).abs() < span * 0.06,
                "{frame:?} {lo:?} {hi:?}"
            );
            // Wheels on the ground, and the main ones where the taxi routes take them to be.
            assert!(lo[1].abs() < 0.05, "{frame:?} {lo:?}");
            let (nose, tail) = frame.reach_m();
            assert!(
                (-lo[2] - nose).abs() < 1.0 && (hi[2] - tail).abs() < 1.0,
                "{frame:?} {lo:?} {hi:?}"
            );
        }
    }

    #[test]
    fn only_aircraft_with_folding_wheels_have_a_wheels_up_model() {
        for frame in ALL {
            let model = crate::vehicle_models::model(frame.model());
            assert_eq!(model.has_gear_up(), frame.retracts(), "{frame:?}");
        }
    }

    #[test]
    fn passengers_sit_in_the_cabin() {
        for frame in ALL {
            let (spots, eye) = frame.seats_m();
            let (nose, tail) = frame.reach_m();
            let top = crate::vehicle_models::model(frame.model()).top_m();
            assert!(!spots.is_empty());
            assert!(eye > 1.0 && eye < top, "{frame:?}");
            // Ahead of the wheels as far as the nose, behind them as far as the tail.
            assert!(
                spots
                    .iter()
                    .all(|&z| f64::from(z) < nose - 1.0 && f64::from(z) > -tail + 1.0),
                "{frame:?}"
            );
            // The seat the model's rider view was checked from is one of them: the
            // light single's is 0.44 m ahead of its wheels, in its front seats.
            if frame == Airframe::Light {
                assert_eq!(spots, &[0.44]);
            }
        }
    }

    #[test]
    fn a_bigger_aircraft_needs_a_longer_runway() {
        for pair in ALL.windows(2) {
            assert!(pair[0].runway_m().0 < pair[1].runway_m().0);
            let (lift, land) = pair[0].speeds();
            assert!(lift > 0.0 && land > 0.0);
        }
        // Its roll to lift-off and its landing roll both fit the runway it asks for.
        for frame in ALL {
            let (lift, land) = frame.speeds();
            let (up, brake) = frame.accel();
            let (runway, _) = frame.runway_m();
            assert!(lift * lift / (2.0 * up) < runway * 0.75, "{frame:?}");
            assert!(land * land / (2.0 * brake) < runway * 0.8, "{frame:?}");
        }
    }
}
