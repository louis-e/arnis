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

/// A cycle lane kept for bicycles on the carriageway, with its line: a two-lane street with
/// one each side, surveyed in Munich, is 10 m kerb to kerb. An advisory lane is marked
/// inside the lanes it runs along and takes nothing more.
const CYCLE_LANE_M: f64 = 1.5;

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
        // A pedestrian street is a path through the town's paving: drawn house front to
        // house front, it was one plain sheet of stone.
        "footway" | "pedestrian" | "path" | "cycleway" | "track" | "escape" | "steps" => Some(1),
        "raceway" => Some(raceways::default_half_width(tags)),
        _ => None,
    }
}

/// Parses a length in metres: the first of several values ("7;8"), with a decimal comma
/// ("3,5") or a trailing "m", or in feet and inches ("10'", "10'6\"", "12 ft").
pub(crate) fn metres(raw: &str) -> Option<f64> {
    let s = raw.split(';').next()?.trim().replace(',', ".");
    let number = |t: &str| t.trim().parse::<f64>().ok();
    let m = if let Some((feet, inches)) = s.split_once('\'') {
        let inches = inches.trim().trim_end_matches('"');
        let inches = if inches.trim().is_empty() {
            0.0
        } else {
            number(inches)?
        };
        number(feet)? * 0.3048 + inches * 0.0254
    } else if let Some(feet) = s.strip_suffix("ft") {
        number(feet)? * 0.3048
    } else {
        number(s.trim_end_matches('m'))?
    };
    Some(m).filter(|v| v.is_finite() && *v > 0.0)
}

/// A count such as `lanes`: the first of several values ("2;3").
pub(crate) fn count(raw: &str) -> Option<i32> {
    raw.split(';').next()?.trim().parse::<i32>().ok()
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

/// Whether any tag's key starts with `prefix`. The parking and cycle lane lookups build
/// their keys one by one; most ways carry none of them and skip that.
fn any_key(tags: &HashMap<String, String>, prefix: &str) -> bool {
    tags.keys().any(|k| k.starts_with(prefix))
}

/// Metres the cars parked on one side take, `side` being "left" or "right", and of that
/// the part in bays set into the sidewalk, which a road's `width` leaves out.
fn parking_side_m(tags: &HashMap<String, String>, side: &str) -> (f64, f64) {
    if !any_key(tags, "parking:") {
        return (0.0, 0.0);
    }
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

/// How the cars parked on one side of the road itself stand, `side` being "left" or
/// "right" of the way: "parallel", "diagonal" or "perpendicular"; None where none park on
/// the road there.
pub(crate) fn parking_orientation(
    tags: &HashMap<String, String>,
    side: &str,
) -> Option<&'static str> {
    if !any_key(tags, "parking:") {
        return None;
    }
    let get = |key: String| tags.get(&key).map(String::as_str);
    let kind = |o: Option<&str>| match o {
        Some("diagonal") => "diagonal",
        Some("perpendicular") => "perpendicular",
        _ => "parallel",
    };
    let street = |suffix: &str| {
        get(format!("parking:{side}{suffix}")).or_else(|| get(format!("parking:both{suffix}")))
    };
    if let Some(position) = street("") {
        return (street_parking_share(position) > 0.0).then(|| kind(street(":orientation")));
    }
    let lane = |suffix: &str| {
        get(format!("parking:lane:{side}{suffix}"))
            .or_else(|| get(format!("parking:lane:both{suffix}")))
    };
    let orientation = match lane("") {
        Some(o @ ("parallel" | "diagonal" | "perpendicular")) => o,
        Some("marked") => "parallel",
        _ => return None,
    };
    let share = lane(&format!(":{orientation}")).map_or(1.0, lane_parking_share);
    (share > 0.0).then(|| kind(Some(orientation)))
}

/// Metres taken by the cars parked on the left and on the right, and of that the part in
/// bays. A deck parks nothing in bays: the bridge is no wider for them.
fn parking_sides_m(tags: &HashMap<String, String>) -> [(f64, f64); 2] {
    let on_bridge = tags.get("bridge").is_some_and(|b| b != "no");
    ["left", "right"].map(|side| {
        let (parked, bays) = parking_side_m(tags, side);
        if on_bridge {
            (parked - bays, 0.0)
        } else {
            (parked, bays)
        }
    })
}

/// Whether cars park on the road itself or in bays along it, which no verge takes back.
pub(crate) fn parks_along(tags: &HashMap<String, String>) -> bool {
    parking_sides_m(tags)
        .iter()
        .any(|&(parked, _)| parked > 0.0)
}

/// Whether a way carries traffic one way only.
fn one_way(tags: &HashMap<String, String>) -> bool {
    let tag = |key: &str| tags.get(key).map(String::as_str);
    matches!(
        tag("oneway"),
        Some("yes" | "true" | "1" | "-1" | "reversible" | "alternating")
    ) || matches!(tag("junction"), Some("roundabout" | "circular"))
}

/// Metres of cycle lanes kept for bicycles on the carriageway, both sides together:
/// `cycleway:<side>=lane` with `cycleway:<side>:lane=exclusive`, and its `:width` if given.
fn cycle_lanes_m(tags: &HashMap<String, String>) -> f64 {
    if !any_key(tags, "cycleway") {
        return 0.0;
    }
    let get = |key: String| tags.get(&key).map(String::as_str);
    ["left", "right"]
        .into_iter()
        .map(|side| {
            let pick = |suffix: &str| {
                get(format!("cycleway:{side}{suffix}"))
                    .or_else(|| get(format!("cycleway:both{suffix}")))
                    .or_else(|| get(format!("cycleway{suffix}")))
            };
            if pick("") == Some("lane") && pick(":lane") == Some("exclusive") {
                pick(":width").and_then(metres).unwrap_or(CYCLE_LANE_M)
            } else {
                0.0
            }
        })
        .sum()
}

/// Whether a street is laid out for two lanes though no lanes are tagged: a residential or
/// unclassified street carrying traffic both ways with a sidewalk beside it, on the way or
/// mapped as its own. Without one, as through a village or an estate, such a street is
/// about 5 m across; with one, 6 to 7 m.
fn two_way_street(highway_type: &str, tags: &HashMap<String, String>) -> bool {
    let tag = |key: &str| tags.get(key).map(String::as_str);
    let sidewalk = [
        "sidewalk",
        "sidewalk:both",
        "sidewalk:left",
        "sidewalk:right",
    ]
    .into_iter()
    .any(|key| tag(key).is_some_and(|v| v != "no" && v != "none"));
    matches!(highway_type, "residential" | "unclassified")
        && tag("lanes").is_none()
        && !one_way(tags)
        && sidewalk
}

/// A road's half-widths, at the scale the world is built at; `kerb` is the wider side's.
///
/// Vehicular roads are their lanes at 3.25 m each (3.5 m on motorways and trunk roads),
/// never narrower than their class's default, plus any cycle lanes kept for bicycles, plus
/// the cars parked along the kerbs. A one-way carriageway with its lanes tagged is as wide
/// as they are, past a motorway's or trunk road's shoulder. A two-way street with a sidewalk and nothing parked along it has two lanes.
/// `width` and `width:carriageway` are kerb to kerb, the cars parked on the road included;
/// bays set into the sidewalk (`street_side`) come on top, and a width too narrow for a lane
/// besides the parked cars is widened to fit one. Down-scaling never collapses a road to
/// nothing; above 1:1 roads keep their 1:1 width.
pub(crate) fn road_halves(
    highway_type: &str,
    tags: &HashMap<String, String>,
    scale: f64,
) -> RoadHalves {
    widths_at(highway_type, tags, scale, true).0
}

/// How far a road's kerb reaches on the left of the way and on its right, in blocks: the
/// lanes' half, and the cars parked on that side only.
pub(crate) fn kerb_sides(
    highway_type: &str,
    tags: &HashMap<String, String>,
    scale: f64,
) -> (i32, i32) {
    widths_at(highway_type, tags, scale, true).1
}

/// The half-width a road keeps where a building stands against it, or a verge: its lanes
/// and cycle lanes, less the second lane a two-way street gets from its sidewalk, which
/// old-town house fronts are often mapped too close to leave room for; and never less than
/// the half of a width tagged on it.
pub(crate) fn wall_half(highway_type: &str, tags: &HashMap<String, String>, scale: f64) -> i32 {
    let lanes = widths_at(highway_type, tags, scale, false).0.lanes;
    let tagged = tags
        .get("width:carriageway")
        .and_then(|w| metres(w))
        .or_else(|| parse_width_tag_m(tags));
    tagged.map_or(lanes, |w| lanes.max(fit(half_of(w), scale)))
}

/// A half-width in blocks at the scale the world is built at: at least one block, at most
/// `MAX_HALF`, and down-scaled below 1:1.
fn fit(half: i32, scale: f64) -> i32 {
    let half = half.clamp(1, MAX_HALF);
    if scale < 1.0 {
        ((f64::from(half) * scale).floor() as i32).max(1)
    } else {
        half
    }
}

/// The road's halves, and its kerb's reach on the left and on the right. With
/// `street_lanes`, its lanes are the ones cars drive in, a two-way street with a sidewalk
/// given its second; without, as `wall_half` wants them, they keep the cycle lanes beside
/// them and leave that second lane out.
fn widths_at(
    highway_type: &str,
    tags: &HashMap<String, String>,
    scale: f64,
    street_lanes: bool,
) -> (RoadHalves, (i32, i32)) {
    // A road under construction is as wide as the finished road.
    let highway_type = if highway_type == "construction" {
        tags.get("construction")
            .map_or(highway_type, String::as_str)
    } else {
        highway_type
    };
    let width = parse_width_tag_m(tags);
    let (lanes, kerb, sides) = if let Some(half) = fixed_half(highway_type, tags) {
        let half = width.map_or(half, half_of);
        (half, half, (half, half))
    } else {
        let (default_m, lane_m) = carriageway_m(highway_type);
        let tagged_lanes = tags.get("lanes").and_then(|l| count(l));
        let lanes = tagged_lanes
            .unwrap_or_else(|| highway_default_lanes(highway_type))
            .clamp(1, 16);
        // A carriageway mapped one way with its lanes tagged spans those lanes, not the
        // class's default for a road carrying both ways; a motorway's or trunk road's keeps
        // the hard shoulder that default takes in.
        let default_m = if tagged_lanes.is_some()
            && one_way(tags)
            && !matches!(highway_type, "motorway" | "trunk")
        {
            0.0
        } else {
            default_m
        };
        let [left, right] = parking_sides_m(tags);
        let (parked_m, bays_m) = (left.0 + right.0, left.1 + right.1);
        // Where cars are parked along it, they take the room of the second lane.
        let street_m = if street_lanes && parked_m == 0.0 && two_way_street(highway_type, tags) {
            2.0 * lane_m
        } else {
            0.0
        };
        let driving_m = default_m.max(f64::from(lanes) * lane_m).max(street_m);
        let cycle_m = cycle_lanes_m(tags);
        let traffic_m = driving_m + cycle_m;
        let tagged = tags
            .get("width:carriageway")
            .and_then(|w| metres(w))
            .or(width);
        let kerb_m = match tagged {
            Some(w) if parked_m > 0.0 => (w + bays_m).max(lane_m + parked_m),
            Some(w) => w,
            None => traffic_m + parked_m,
        };
        let lanes_m = if parked_m > 0.0 {
            (kerb_m - parked_m).max(if tagged.is_some() { lane_m } else { traffic_m })
        } else {
            kerb_m
        };
        // Each kerb is half the lanes out, and the cars parked on its own side: the half
        // of a road with that side's parking on both.
        let sides = (
            half_of(lanes_m + 2.0 * left.0),
            half_of(lanes_m + 2.0 * right.0),
        );
        // Cycle lanes run between the lanes cars drive in and the kerb, as parked cars do.
        let lanes_m = if street_lanes {
            (lanes_m - cycle_m).max(driving_m.min(lanes_m))
        } else {
            lanes_m
        };
        let lanes = half_of(lanes_m.min(kerb_m));
        (lanes, sides.0.max(sides.1), sides)
    };
    let kerb = fit(kerb, scale);
    let lanes = fit(lanes, scale).min(kerb);
    let side = |half: i32| fit(half, scale).clamp(lanes, kerb);
    (RoadHalves { lanes, kerb }, (side(sides.0), side(sides.1)))
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
        // The side's own tag overrides both, and the cars widen their own side only: 5 m
        // of lanes, 2.25 m of cars on the left.
        let one = tags(&[("parking:both", "lane"), ("parking:right", "no")]);
        assert_eq!(road_halves("residential", &one, 1.0).kerb, 4);
        assert_eq!(kerb_sides("residential", &one, 1.0), (4, 2));
        // Rows across the kerb on one side: 5 m of them there, none on the other.
        let across_one = tags(&[
            ("parking:right", "lane"),
            ("parking:right:orientation", "diagonal"),
        ]);
        assert_eq!(kerb_sides("residential", &across_one, 1.0), (2, 7));
    }

    #[test]
    fn the_older_parking_lane_scheme_counts_too() {
        assert_eq!(
            halves("residential", &[("parking:lane:both", "parallel")]).kerb,
            4
        );
        let diagonal = tags(&[
            ("parking:lane:right", "diagonal"),
            ("parking:lane:right:diagonal", "on_street"),
        ]);
        assert_eq!(kerb_sides("residential", &diagonal, 1.0), (2, 7));
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
    fn width_is_kerb_to_kerb_the_cars_parked_on_the_road_included() {
        // 7 m across, not 9.
        assert_eq!(halves("residential", &[("width", "7")]).kerb, 3);
        assert_eq!(halves("residential", &[("width", "8 m")]).kerb, 4);
        // Too narrow for its rows of cars and a lane between them, it is widened to fit
        // them: 7.75 m.
        let narrow = halves("residential", &[("width", "5"), ("parking:both", "lane")]);
        assert_eq!(narrow, RoadHalves { lanes: 1, kerb: 3 });
        // Not to the class's lanes: 10 m of a one-way street with cars along one kerb and
        // at an angle along the other stays 10 m, 2.25 m of cars on the left and 5 m on
        // the right of a 3.25 m lane.
        let ufer = tags(&[
            ("width", "10"),
            ("oneway", "yes"),
            ("parking:both", "lane"),
            ("parking:left:orientation", "parallel"),
            ("parking:right:orientation", "diagonal"),
        ]);
        assert_eq!(kerb_sides("residential", &ufer, 1.0), (3, 6));
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
    fn a_verge_never_takes_a_road_inside_its_tagged_width() {
        let perpendicular = tags(&[
            ("width", "14.9"),
            ("parking:both", "lane"),
            ("parking:both:orientation", "perpendicular"),
        ]);
        // Kerb to kerb 14.9 m: a verge may take the rows back, as the tag is wider than
        // the lanes, but never inside the width tagged.
        assert_eq!(road_halves("residential", &perpendicular, 1.0).kerb, 7);
        assert_eq!(wall_half("residential", &perpendicular, 1.0), 7);
        assert_eq!(wall_half("residential", &tags(&[]), 1.0), 2);
        assert_eq!(
            wall_half("residential", &tags(&[("width:carriageway", "9")]), 1.0),
            4
        );
    }

    #[test]
    fn a_one_way_carriageway_spans_its_tagged_lanes() {
        let one = |extra: &[(&str, &str)]| {
            let pairs: Vec<(&str, &str)> = [("lanes", "1"), ("oneway", "yes")]
                .into_iter()
                .chain(extra.iter().copied())
                .collect();
            halves("primary", &pairs).kerb
        };
        // One 3.25 m lane, not the 8 m of a primary carrying both ways.
        assert_eq!(one(&[]), 1);
        assert_eq!(halves("primary", &[("lanes", "1")]).kerb, 4);
        assert_eq!(halves("primary", &[("oneway", "yes")]).kerb, 4);
        // Two lanes of a dual carriageway: 6.5 m; a motorway's keeps its shoulder.
        assert_eq!(
            halves("primary", &[("lanes", "2"), ("oneway", "yes")]).kerb,
            3
        );
        assert_eq!(
            halves("motorway", &[("lanes", "2"), ("oneway", "yes")]).kerb,
            5
        );
    }

    #[test]
    fn a_bridge_parks_no_cars_in_bays() {
        let street = [
            ("oneway", "yes"),
            ("parking:left", "street_side"),
            ("parking:left:orientation", "diagonal"),
            ("parking:right", "no"),
        ];
        let deck: Vec<(&str, &str)> = street.into_iter().chain([("bridge", "yes")]).collect();
        assert_eq!(halves("residential", &street).kerb, 7);
        assert_eq!(halves("residential", &deck), halves("residential", &[]));
        assert!(!parks_along(&tags(&deck)));
        assert!(parks_along(&tags(&street)));
        // Cars parked on the deck's own road still count.
        let lane = tags(&[("bridge", "yes"), ("parking:both", "lane")]);
        assert_eq!(road_halves("residential", &lane, 1.0).kerb, 4);
    }

    #[test]
    fn paths_and_pedestrian_streets_keep_their_own_widths() {
        assert_eq!(halves("footway", &[("lanes", "4")]).kerb, 1);
        assert_eq!(halves("cycleway", &[]).kerb, 1);
        assert_eq!(halves("pedestrian", &[]).kerb, 1);
        // A tagged width is the width drawn: 3 m three blocks, 4.8 m five.
        assert_eq!(halves("footway", &[("width", "3")]).kerb, 1);
        let sidewalk = halves("footway", &[("footway", "sidewalk"), ("width", "4.8")]);
        assert_eq!(sidewalk, RoadHalves { lanes: 2, kerb: 2 });
    }

    #[test]
    fn a_two_way_street_with_a_sidewalk_has_two_lanes() {
        assert_eq!(halves("residential", &[("sidewalk", "both")]).kerb, 3);
        assert_eq!(
            halves("residential", &[("sidewalk:right", "separate")]).kerb,
            3
        );
        assert_eq!(halves("unclassified", &[("sidewalk", "separate")]).kerb, 3);
        // One way, without a sidewalk, with its lanes tagged, or not a street: as before.
        let as_before: [(&str, &[(&str, &str)]); 6] = [
            ("residential", &[("sidewalk", "both"), ("oneway", "yes")]),
            (
                "residential",
                &[("sidewalk", "both"), ("junction", "roundabout")],
            ),
            ("residential", &[("sidewalk", "no")]),
            ("residential", &[("sidewalk", "both"), ("lanes", "1")]),
            ("service", &[("sidewalk", "both")]),
            ("living_street", &[("sidewalk", "both")]),
        ];
        for (highway, pairs) in as_before {
            assert_eq!(halves(highway, pairs).kerb, 2, "{highway} {pairs:?}");
        }
        // Cars parked along it take the second lane's room.
        let parked = halves(
            "residential",
            &[("sidewalk", "both"), ("parking:both", "lane")],
        );
        assert_eq!(parked, RoadHalves { lanes: 2, kerb: 4 });
    }

    #[test]
    fn cycle_lanes_kept_for_bicycles_widen_the_carriageway() {
        let exclusive = [
            ("lanes", "2"),
            ("cycleway:both", "lane"),
            ("cycleway:both:lane", "exclusive"),
        ];
        // 6.5 m of lanes and two 1.5 m cycle lanes: the cars drive in the 6.5 m, and a
        // building keeps off the cycle lanes too.
        assert_eq!(
            halves("residential", &exclusive),
            RoadHalves { lanes: 3, kerb: 4 }
        );
        assert_eq!(wall_half("residential", &tags(&exclusive), 1.0), 4);
        // Four lanes and their cycle lanes: the outer lane stops at the cycle lane.
        let four: Vec<(&str, &str)> = exclusive
            .into_iter()
            .map(|(k, v)| if k == "lanes" { (k, "4") } else { (k, v) })
            .collect();
        assert_eq!(
            halves("residential", &four),
            RoadHalves { lanes: 6, kerb: 8 }
        );
        // An advisory lane is marked inside the lanes.
        let advisory = [
            ("cycleway:both", "lane"),
            ("cycleway:both:lane", "advisory"),
        ];
        assert_eq!(halves("secondary", &advisory).kerb, 3);
        // One side: 8 m, or at its own 1 m width 7.5 m.
        let one = [
            ("lanes", "2"),
            ("cycleway:right", "lane"),
            ("cycleway:right:lane", "exclusive"),
        ];
        assert_eq!(halves("residential", &one).kerb, 4);
        let narrow: Vec<(&str, &str)> = one
            .into_iter()
            .chain([("cycleway:right:width", "1")])
            .collect();
        assert_eq!(halves("residential", &narrow).kerb, 3);
    }

    #[test]
    fn parking_sides_tell_how_the_cars_stand() {
        let t = tags(&[
            ("parking:left", "lane"),
            ("parking:left:orientation", "perpendicular"),
            ("parking:right", "no"),
        ]);
        assert_eq!(parking_orientation(&t, "left"), Some("perpendicular"));
        assert_eq!(parking_orientation(&t, "right"), None);
        let both = tags(&[("parking:both", "half_on_kerb")]);
        assert_eq!(parking_orientation(&both, "right"), Some("parallel"));
        let old = tags(&[
            ("parking:lane:right", "diagonal"),
            ("parking:lane:right:diagonal", "on_street"),
        ]);
        assert_eq!(parking_orientation(&old, "right"), Some("diagonal"));
        assert_eq!(parking_orientation(&old, "left"), None);
    }

    #[test]
    fn widths_and_lanes_read_as_mappers_write_them() {
        // The first of several values, a decimal comma, feet and inches.
        assert_eq!(metres("7;8"), Some(7.0));
        assert_eq!(metres("3,5"), Some(3.5));
        assert!((metres("10'").unwrap() - 3.048).abs() < 1e-9);
        assert!((metres("10'6\"").unwrap() - 3.2004).abs() < 1e-9);
        assert!((metres("12 ft").unwrap() - 3.6576).abs() < 1e-9);
        assert_eq!(metres("6.5 m"), Some(6.5));
        assert_eq!(metres("wide"), None);
        assert_eq!(metres("0"), None);
        assert_eq!(count("2;3"), Some(2));
        assert_eq!(count("x"), None);
        // As a road reads them: 7 m and 3.5 m kerb to kerb, two lanes.
        assert_eq!(halves("residential", &[("width", "7;8")]).kerb, 3);
        assert_eq!(halves("residential", &[("width", "3,5")]).kerb, 1);
        assert_eq!(halves("residential", &[("width", "10'")]).kerb, 1);
        assert_eq!(
            halves("primary", &[("lanes", "2;3")]),
            halves("primary", &[("lanes", "2")])
        );
    }

    #[test]
    fn scaling_down_never_collapses_a_road() {
        assert_eq!(road_halves("footway", &tags(&[]), 0.4).kerb, 1);
        assert_eq!(road_halves("residential", &tags(&[]), 0.4).kerb, 1);
        let parked = road_halves("residential", &tags(&[("parking:both", "lane")]), 0.5);
        assert_eq!(parked, RoadHalves { lanes: 1, kerb: 2 });
    }
}
