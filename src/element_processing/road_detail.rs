//! `--road-detail`: how much paint and how many minor ways a road network gets. Every
//! decision is a pure function of tags and position, so One World seams never change it.

use crate::osm_parser::ProcessedElement;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum RoadDetail {
    /// Every marking and way, as mapped
    #[default]
    Max,
    /// Removes lane dividers except on one-way roads wider than 5 blocks, and every line on
    /// service and pedestrian ways, parking aisles and roads under 4 blocks wide. Paints zebra
    /// and give-way bars 2 blocks apart
    Clean,
    /// Clean with dashes and gaps of at least 4 blocks and no crossing paint. Also leaves out
    /// footways and sidewalks, paths, cycleways, steps, tracks, crossings, parking aisles,
    /// driveways, alleys and drive-throughs
    Compact,
}

/// Ways for people rather than traffic; no lane dividers outside `max`.
fn is_pedestrian_grade(highway: &str) -> bool {
    matches!(
        highway,
        "footway"
            | "path"
            | "cycleway"
            | "steps"
            | "corridor"
            | "pedestrian"
            | "platform"
            | "bus_stop"
            | "track"
    )
}

/// Service roads `compact` leaves out; an untagged one is usually a real access road.
fn is_minor_service(tags: &HashMap<String, String>) -> bool {
    matches!(
        tags.get("service").map(String::as_str),
        Some("parking_aisle" | "driveway" | "alley" | "drive-through" | "drive_through")
    )
}

impl RoadDetail {
    /// Whether `compact` leaves this highway out. Signals stay; only their crossing paint goes.
    pub fn skips_highway(self, tags: &HashMap<String, String>) -> bool {
        let Some(highway) = tags.get("highway") else {
            return false;
        };
        self == RoadDetail::Compact
            && match highway.as_str() {
                "footway" | "path" | "cycleway" | "steps" | "track" | "crossing" => true,
                "service" => is_minor_service(tags),
                _ => false,
            }
    }

    /// Whether to drop the element before generation, so the road mask, junctions and
    /// signage match the roads drawn. One another handler builds first is kept.
    pub fn drops_element(self, element: &ProcessedElement) -> bool {
        let tags = element.tags();
        !matches!(element, ProcessedElement::Relation(_))
            && self.skips_highway(tags)
            && tags.get("man_made").map(String::as_str) != Some("pier")
            && ![
                "building",
                "building:part",
                "aerialway",
                "aeroway",
                "amenity",
                "attraction",
                "barrier",
                "door",
                "entrance",
                "natural",
            ]
            .iter()
            .any(|key| tags.contains_key(*key))
    }

    /// Whether a lane line upstream paints stays: outside `max`, centre lines and, on one-way
    /// roads wider than 5 blocks, lane dividers; none where a stripe leaves no asphalt beside it.
    pub fn keeps_lane_line(
        self,
        highway: &str,
        tags: &HashMap<String, String>,
        road_width: i32,
        centre: bool,
        oneway: bool,
    ) -> bool {
        if self == RoadDetail::Max {
            return true;
        }
        let unmarked = road_width < 4
            || highway == "service"
            || is_pedestrian_grade(highway)
            || tags.get("service").map(String::as_str) == Some("parking_aisle");
        (centre || (oneway && road_width > 5)) && !unmarked
    }

    /// Upstream's `(dash, period)` in path cells; `compact` keeps dashes and gaps at least 4
    /// long so short dashes do not merge into a checker.
    pub fn dash_pattern(self, (dash, period): (u32, u32)) -> (u32, u32) {
        if self != RoadDetail::Compact {
            return (dash, period);
        }
        let on = dash.max(4);
        (on, on + period.saturating_sub(dash).max(4))
    }

    /// Whether bar `i` of a zebra, give-way line or broken crossing edge is white: every
    /// other cell at `max`, as upstream; 2-on/2-off otherwise, which stays apart from above.
    pub fn bar(self, i: i32) -> bool {
        if self == RoadDetail::Max {
            i.rem_euclid(2) == 0
        } else {
            i.rem_euclid(4) < 2
        }
    }

    /// Whether crossings get paint, lane-line gaps and stop lines.
    pub fn paints_crossings(self) -> bool {
        self != RoadDetail::Compact
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element_processing::building_test_support::tag_map as tags;
    use crate::osm_parser::{ProcessedNode, ProcessedWay};

    #[test]
    fn only_compact_skips_highways() {
        let footway = tags(&[("highway", "footway")]);
        assert!(!RoadDetail::Max.skips_highway(&footway));
        assert!(!RoadDetail::Clean.skips_highway(&footway));
        assert!(RoadDetail::Compact.skips_highway(&footway));
        for skipped in [
            &[("highway", "service"), ("service", "parking_aisle")][..],
            &[("highway", "service"), ("service", "driveway")],
            &[("highway", "service"), ("service", "alley")],
            &[("highway", "service"), ("service", "drive-through")],
            &[("highway", "track")],
            &[("highway", "crossing")],
            &[("highway", "footway"), ("footway", "sidewalk")],
        ] {
            assert!(
                RoadDetail::Compact.skips_highway(&tags(skipped)),
                "{skipped:?}"
            );
        }
        for kept in [
            &[("highway", "residential")][..],
            &[("highway", "primary"), ("footway", "no")],
            &[("highway", "street_lamp")],
            &[("building", "yes")],
            &[("highway", "service")],
            &[("highway", "service"), ("service", "emergency_access")],
            &[("highway", "pedestrian")],
            &[("highway", "pedestrian"), ("area", "yes")],
            &[("highway", "platform")],
            &[("highway", "bus_stop")],
            &[("highway", "corridor")],
            &[
                ("highway", "traffic_signals"),
                ("crossing", "traffic_signals"),
            ],
            &[("highway", "residential"), ("footway", "crossing")],
        ] {
            assert!(!RoadDetail::Compact.skips_highway(&tags(kept)), "{kept:?}");
        }
    }

    #[test]
    fn compact_keeps_elements_another_handler_builds() {
        let node = |pairs: &[(&str, &str)]| {
            ProcessedElement::Node(ProcessedNode {
                id: 1,
                tags: tags(pairs),
                x: 0,
                z: 0,
            })
        };
        let way = |pairs: &[(&str, &str)]| {
            ProcessedElement::Way(ProcessedWay {
                id: 1,
                nodes: Vec::new(),
                tags: tags(pairs),
            })
        };
        assert!(RoadDetail::Compact.drops_element(&way(&[("highway", "footway")])));
        assert!(RoadDetail::Compact.drops_element(&node(&[("highway", "crossing")])));
        assert!(!RoadDetail::Clean.drops_element(&way(&[("highway", "footway")])));
        assert!(!RoadDetail::Compact
            .drops_element(&way(&[("highway", "pedestrian"), ("building", "yes")])));
        assert!(!RoadDetail::Compact
            .drops_element(&node(&[("highway", "bus_stop"), ("amenity", "shelter")])));
        assert!(!RoadDetail::Compact
            .drops_element(&node(&[("highway", "crossing"), ("barrier", "kerb")])));
        assert!(!RoadDetail::Compact
            .drops_element(&way(&[("highway", "footway"), ("man_made", "pier")])));
        assert!(!RoadDetail::Compact.drops_element(&node(&[
            ("highway", "traffic_signals"),
            ("crossing", "traffic_signals")
        ])));
    }

    #[test]
    fn max_is_v340() {
        let none = HashMap::new();
        for width in 1..=12 {
            for centre in [false, true] {
                for oneway in [false, true] {
                    assert!(
                        RoadDetail::Max.keeps_lane_line("service", &none, width, centre, oneway)
                    );
                }
            }
        }
        for pattern in [(3, 9), (6, 18), (1, 3), (2, 5)] {
            assert_eq!(RoadDetail::Max.dash_pattern(pattern), pattern);
            assert_eq!(RoadDetail::Clean.dash_pattern(pattern), pattern);
        }
        for x in -8..=8 {
            assert_eq!(RoadDetail::Max.bar(x), x.rem_euclid(2) == 0);
        }
        assert!(RoadDetail::Max.paints_crossings() && RoadDetail::Clean.paints_crossings());
        assert!(!RoadDetail::Compact.paints_crossings());
    }

    #[test]
    fn low_drops_markings_keeps_roads() {
        let none = HashMap::new();
        for mode in [RoadDetail::Clean, RoadDetail::Compact] {
            assert!(mode.keeps_lane_line("primary", &none, 9, true, false));
            assert!(!mode.keeps_lane_line("primary", &none, 9, false, false));
            // One-way multi-lane roads keep their lane dividers when wide enough.
            assert!(mode.keeps_lane_line("primary", &none, 9, false, true));
            assert!(!mode.keeps_lane_line("primary", &none, 5, false, true));
            assert!(!mode.keeps_lane_line("service", &none, 9, false, true));
            // Too narrow, service, pedestrian ways and parking aisles get no line.
            assert!(!mode.keeps_lane_line("primary", &none, 3, true, false));
            assert!(!mode.keeps_lane_line("service", &none, 5, true, false));
            assert!(!mode.keeps_lane_line("footway", &none, 5, true, false));
            let aisle = tags(&[("service", "parking_aisle")]);
            assert!(!mode.keeps_lane_line("unclassified", &aisle, 5, true, false));
        }
        // Compact stretches short dashes and gaps to 4 blocks.
        assert_eq!(RoadDetail::Compact.dash_pattern((2, 5)), (4, 8));
        assert_eq!(RoadDetail::Compact.dash_pattern((3, 9)), (4, 10));
        assert_eq!(RoadDetail::Compact.dash_pattern((6, 18)), (6, 18));
    }

    #[test]
    fn zebra_bars_alternate_across_zero() {
        let bars: Vec<bool> = (-8..8).map(|x| RoadDetail::Clean.bar(x)).collect();
        for pair in bars.chunks(2) {
            assert_eq!(pair[0], pair[1]);
        }
        for quad in bars.chunks(4) {
            assert_ne!(quad[0], quad[2]);
        }
        // `max` alternates single cells, across zero too.
        assert!(RoadDetail::Max.bar(-2) && !RoadDetail::Max.bar(-1) && RoadDetail::Max.bar(0));
    }
}
