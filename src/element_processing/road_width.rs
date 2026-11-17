//! How wide a road is drawn: its traffic lanes and the cars parked along its kerbs.
//!
//! Widths are worked out in metres, then turned into a half-width in blocks either side of
//! the centreline cell, so a road `2 * half + 1` blocks across.

use crate::element_processing::highways::{highway_default_lanes, parse_width_tag_m};
use crate::element_processing::raceways;
use std::collections::HashMap;

/// Widest road half-width in blocks.
pub(crate) const MAX_HALF: i32 = 8;

/// A traffic lane, in metres. Motorways and trunk roads use the wider lane and a shoulder.
const LANE_M: f64 = 3.25;
const FAST_LANE_M: f64 = 3.5;

/// Cars parked along the kerb take this much of the carriageway; parked across it or at an
/// angle, a stall's depth.
const PARALLEL_PARKING_M: f64 = 2.25;
const ANGLED_PARKING_M: f64 = 5.0;

/// A road's half-widths in blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RoadHalves {
    /// The traffic lanes, which lane lines divide and cars drive in.
    pub lanes: i32,
    /// Kerb to kerb: the lanes and the cars parked along them.
    pub kerb: i32,
}

/// Width in metres to a half-width in blocks, the centre cell counted once.
fn half_of(metres: f64) -> i32 {
    ((metres - 1.0) / 2.0).round() as i32
}

/// Width of a carriageway with no `lanes` or `width` tag, and the width of one lane, in
/// metres. Rural main roads are narrower than their class suggests: a Bavarian state road
/// is 6.5 to 7.5 m across.
fn carriageway_m(highway_type: &str) -> (f64, f64) {
    match highway_type {
        "motorway" | "trunk" => (11.0, FAST_LANE_M),
        "motorway_link" | "trunk_link" => (5.0, FAST_LANE_M),
        "primary" => (8.0, LANE_M),
        "secondary" => (7.0, LANE_M),
        "tertiary" => (6.5, LANE_M),
        "secondary_link" | "tertiary_link" => (3.0, LANE_M),
        _ => (5.0, LANE_M),
    }
}

/// Half-width of the ways that never widen with lanes.
fn fixed_half(highway_type: &str, tags: &HashMap<String, String>) -> Option<i32> {
    match highway_type {
        // Bicycle paths are 2-4 m across; at the road default they swallowed the verge
        // and the kerb of the road they run beside.
        "footway" | "path" | "cycleway" | "track" | "escape" | "steps" => Some(1),
        // A pedestrian street is the whole street, house front to house front.
        "pedestrian" => Some(3),
        "raceway" => Some(raceways::default_half_width(tags)),
        _ => None,
    }
}

/// Parses a length in metres, tolerating a trailing "m".
pub(crate) fn metres(raw: &str) -> Option<f64> {
    let s = raw.trim().trim_end_matches('m').trim();
    s.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

/// How much of a parking row stands on the carriageway, by `parking:<side>=*`.
fn street_parking_share(position: &str) -> f64 {
    match position {
        "lane" | "street_side" | "shoulder" | "yes" => 1.0,
        "half_on_kerb" => 0.5,
        _ => 0.0,
    }
}

/// The same, by the older `parking:lane:<side>:<orientation>=*`.
fn lane_parking_share(position: &str) -> f64 {
    match position {
        "on_street" | "painted_area_only" | "street_side" | "lay_by" | "shoulder" => 1.0,
        "half_on_kerb" => 0.5,
        _ => 0.0,
    }
}

fn parking_depth_m(orientation: Option<&str>) -> f64 {
    match orientation {
        Some("diagonal" | "perpendicular") => ANGLED_PARKING_M,
        _ => PARALLEL_PARKING_M,
    }
}

/// Metres the cars parked on one side take, `side` being "left" or "right", and of that
/// the part in bays set into the sidewalk, which a road's `width` leaves out.
fn parking_side_m(tags: &HashMap<String, String>, side: &str) -> (f64, f64) {
    let get = |key: String| tags.get(&key).map(String::as_str);
    // parking:<side>=<position>, parking:<side>:orientation=*, parking:<side>:width=*.
    let street = |suffix: &str| {
        get(format!("parking:{side}{suffix}")).or_else(|| get(format!("parking:both{suffix}")))
    };
    if let Some(position) = street("") {
        let share = street_parking_share(position);
        if share == 0.0 {
            return (0.0, 0.0);
        }
        let depth = street(":width")
            .and_then(metres)
            .unwrap_or_else(|| parking_depth_m(street(":orientation")));
        let bays = if position == "street_side" {
            depth
        } else {
            0.0
        };
        return (share * depth, bays);
    }
    // parking:lane:<side>=<orientation>, parking:lane:<side>:<orientation>=<position>.
    let lane = |suffix: &str| {
        get(format!("parking:lane:{side}{suffix}"))
            .or_else(|| get(format!("parking:lane:both{suffix}")))
    };
    let orientation = match lane("") {
        Some(o @ ("parallel" | "diagonal" | "perpendicular")) => o,
        Some("marked") => "parallel",
        _ => return (0.0, 0.0),
    };
    let position = lane(&format!(":{orientation}"));
    let depth = position.map_or(1.0, lane_parking_share) * parking_depth_m(Some(orientation));
    let bays = matches!(position, Some("street_side" | "lay_by"));
    (depth, if bays { depth } else { 0.0 })
}

/// Metres taken by parked cars, both sides together, and of that the part in bays.
fn parking_m(tags: &HashMap<String, String>) -> (f64, f64) {
    let (l, r) = (parking_side_m(tags, "left"), parking_side_m(tags, "right"));
    (l.0 + r.0, l.1 + r.1)
}

/// A road's half-widths, at the scale the world is built at.
///
/// Vehicular roads are their lanes at 3.25 m each (3.5 m on motorways and trunk roads),
/// never narrower than their class's default, plus the cars parked along the kerbs.
/// `width` and `width:carriageway` are kerb to kerb where nothing is parked. Bays set into
/// the sidewalk (`street_side`) come on top of them; parking on the road itself is in some
/// mappers' `width` and not in others', so there the width is the least the road spans.
/// Down-scaling never collapses a road to nothing; above 1:1 roads keep their
/// 1:1 width.
pub(crate) fn road_halves(
    highway_type: &str,
    tags: &HashMap<String, String>,
    scale: f64,
) -> RoadHalves {
    // A road under construction is as wide as the finished road.
    let highway_type = if highway_type == "construction" {
        tags.get("construction")
            .map_or(highway_type, String::as_str)
    } else {
        highway_type
    };
    let width = parse_width_tag_m(tags);
    let (lanes, kerb) = if let Some(half) = fixed_half(highway_type, tags) {
        let half = width.map_or(half, half_of);
        (half, half)
    } else {
        let (default_m, lane_m) = carriageway_m(highway_type);
        let lanes = tags
            .get("lanes")
            .and_then(|l| l.parse::<i32>().ok())
            .unwrap_or_else(|| highway_default_lanes(highway_type))
            .clamp(1, 16);
        let traffic_m = default_m.max(f64::from(lanes) * lane_m);
        let (parked_m, bays_m) = parking_m(tags);
        let tagged = tags
            .get("width:carriageway")
            .and_then(|w| metres(w))
            .or(width);
        let kerb_m = match tagged {
            Some(w) if parked_m > 0.0 => (w + bays_m).max(traffic_m + parked_m),
            Some(w) => w,
            None => traffic_m + parked_m,
        };
        let lanes_m = if parked_m > 0.0 {
            (kerb_m - parked_m).max(traffic_m)
        } else {
            kerb_m
        };
        (half_of(lanes_m.min(kerb_m)), half_of(kerb_m))
    };
    let fit = |half: i32| {
        let half = half.clamp(1, MAX_HALF);
        if scale < 1.0 {
            ((f64::from(half) * scale).floor() as i32).max(1)
        } else {
            half
        }
    };
    let kerb = fit(kerb);
    RoadHalves {
        lanes: fit(lanes).min(kerb),
        kerb,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn halves(highway: &str, pairs: &[(&str, &str)]) -> RoadHalves {
        road_halves(highway, &tags(pairs), 1.0)
    }

    #[test]
    fn rural_main_roads_are_as_narrow_as_they_are_built() {
        assert_eq!(halves("secondary", &[]).kerb, 3);
        assert_eq!(halves("primary", &[]).kerb, 4);
        assert_eq!(halves("tertiary", &[]).kerb, 3);
        assert_eq!(halves("residential", &[]).kerb, 2);
        assert_eq!(halves("motorway", &[]).kerb, 5);
        // Two tagged lanes are no wider than the class already is.
        assert_eq!(halves("secondary", &[("lanes", "2")]).kerb, 3);
    }

    #[test]
    fn lanes_widen_at_about_three_metres_each() {
        // 13 m for four lanes, not the 15 m of 3.5 m lanes.
        assert_eq!(halves("residential", &[("lanes", "4")]).kerb, 6);
        assert_eq!(halves("primary", &[("lanes", "4")]).kerb, 6);
        assert_eq!(halves("motorway", &[("lanes", "6")]).kerb, MAX_HALF);
    }

    #[test]
    fn parked_cars_widen_the_kerb_but_not_the_lanes() {
        let both = halves("residential", &[("parking:both", "lane")]);
        // 5 m of lanes and 2 x 2.25 m of cars along the kerbs.
        assert_eq!(both, RoadHalves { lanes: 2, kerb: 4 });
        let across = halves(
            "residential",
            &[
                ("parking:both", "lane"),
                ("parking:both:orientation", "perpendicular"),
            ],
        );
        assert_eq!(across.kerb, 7);
        // Half on the kerb is half on the road; on the kerb or nothing is none.
        let half = halves(
            "residential",
            &[("parking:left", "half_on_kerb"), ("parking:right", "no")],
        );
        assert_eq!(half.kerb, 3);
        assert_eq!(
            halves("residential", &[("parking:both", "on_kerb")]).kerb,
            2
        );
        // The side's own tag overrides both.
        let one = halves(
            "residential",
            &[("parking:both", "lane"), ("parking:right", "no")],
        );
        assert_eq!(one.kerb, 3);
    }

    #[test]
    fn the_older_parking_lane_scheme_counts_too() {
        assert_eq!(
            halves("residential", &[("parking:lane:both", "parallel")]).kerb,
            4
        );
        let diagonal = halves(
            "residential",
            &[
                ("parking:lane:right", "diagonal"),
                ("parking:lane:right:diagonal", "on_street"),
            ],
        );
        assert_eq!(diagonal.kerb, 5);
        assert_eq!(
            halves(
                "residential",
                &[
                    ("parking:lane:both", "parallel"),
                    ("parking:lane:both:parallel", "on_kerb"),
                ]
            )
            .kerb,
            2
        );
        assert_eq!(
            halves("residential", &[("parking:lane:both", "no_stopping")]).kerb,
            2
        );
    }

    #[test]
    fn width_is_kerb_to_kerb_and_at_least_the_span_with_parking() {
        // 7 m across, not 9.
        assert_eq!(halves("residential", &[("width", "7")]).kerb, 3);
        assert_eq!(halves("residential", &[("width", "8 m")]).kerb, 4);
        // A width that leaves out the parking rows still gets them.
        let narrow = halves("residential", &[("width", "5"), ("parking:both", "lane")]);
        assert_eq!(narrow.kerb, 4);
        // One that takes them in stays as it is, and the lanes are what is left.
        let wide = halves("residential", &[("width", "11"), ("parking:both", "lane")]);
        assert_eq!(wide, RoadHalves { lanes: 3, kerb: 5 });
        assert_eq!(
            halves(
                "residential",
                &[("width:carriageway", "9"), ("parking:both", "lane")]
            )
            .kerb,
            4
        );
        // Bays set into the sidewalk are outside the tagged width: 4.6 m and two rows of
        // stalls across the kerb.
        let bays = halves(
            "residential",
            &[
                ("width", "4.6"),
                ("parking:both", "street_side"),
                ("parking:both:orientation", "perpendicular"),
            ],
        );
        assert_eq!(bays, RoadHalves { lanes: 2, kerb: 7 });
        let parallel_bays = halves(
            "residential",
            &[("width", "8.4"), ("parking:both", "street_side")],
        );
        assert_eq!(parallel_bays.kerb, 6);
    }

    #[test]
    fn paths_and_pedestrian_streets_keep_their_own_widths() {
        assert_eq!(halves("footway", &[("lanes", "4")]).kerb, 1);
        assert_eq!(halves("cycleway", &[]).kerb, 1);
        assert_eq!(halves("pedestrian", &[]).kerb, 3);
        assert_eq!(halves("footway", &[("width", "3")]).kerb, 1);
    }

    #[test]
    fn scaling_down_never_collapses_a_road() {
        assert_eq!(road_halves("footway", &tags(&[]), 0.4).kerb, 1);
        assert_eq!(road_halves("residential", &tags(&[]), 0.4).kerb, 1);
        let parked = road_halves("residential", &tags(&[("parking:both", "lane")]), 0.5);
        assert_eq!(parked, RoadHalves { lanes: 1, kerb: 2 });
    }
}
