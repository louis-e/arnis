//! What each generation mode keeps of the parsed OpenStreetMap data.
use std::collections::HashMap;

use crate::args::GenerationMode;
use crate::osm_parser::{OutlineSuppression, ProcessedElement};

/// Keys that make a feature man-made, whatever else it is tagged with.
const MAN_MADE_KEYS: [&str; 14] = [
    "building",
    "building:part",
    "highway",
    "railway",
    "amenity",
    "leisure",
    "man_made",
    "barrier",
    "power",
    "aeroway",
    "tourism",
    "historic",
    "bridge",
    "place",
];

/// Drops what the mode leaves out. Runs after the land cover overrides, which still read
/// roads and buildings as evidence of where the land is.
pub fn retain_for_mode(elements: &mut Vec<ProcessedElement>, mode: GenerationMode) {
    match mode {
        GenerationMode::TerrainOnly => elements.retain(is_terrain_feature),
        GenerationMode::Blueprint => elements.retain(is_blueprint_feature),
        GenerationMode::GeoTerrain | GenerationMode::GeoOnly => {}
    }
}

/// Outlines whose parts would render in their place. A blueprint drops the parts and
/// draws the outline itself, so nothing is suppressed there.
pub fn outline_suppression_for_mode(
    suppression: OutlineSuppression,
    mode: GenerationMode,
) -> OutlineSuppression {
    if mode == GenerationMode::Blueprint {
        OutlineSuppression::new()
    } else {
        suppression
    }
}

/// Water and natural ground. Mapped trees stay out: in a town they stand along streets
/// that are gone, and would trace them.
fn is_terrain_feature(element: &ProcessedElement) -> bool {
    let tags = element.tags();
    if matches!(element, ProcessedElement::Node(_))
        || MAN_MADE_KEYS.iter().any(|k| tags.contains_key(*k))
    {
        return false;
    }
    if let Some(natural) = tags.get("natural") {
        return !matches!(natural.as_str(), "tree" | "tree_row");
    }
    if let Some(waterway) = tags.get("waterway") {
        return !is_waterway_structure(waterway);
    }
    tags.contains_key("water")
        || matches!(tag(tags, "landuse"), Some("reservoir" | "basin" | "forest"))
}

/// Roads, rails, water and nature, plus the buildings, which only get an outline. What
/// furnishes a street or a lot stays out.
fn is_blueprint_feature(element: &ProcessedElement) -> bool {
    let tags = element.tags();
    if let ProcessedElement::Node(_) = element {
        return tag(tags, "natural") == Some("tree");
    }
    // A part only repeats a piece of its building's outline.
    if tags
        .get("building:part")
        .is_some_and(|v| !v.eq_ignore_ascii_case("no"))
    {
        return false;
    }
    if tags.contains_key("building") || tag(tags, "type") == Some("building") {
        return true;
    }
    if let Some(waterway) = tags.get("waterway") {
        return !is_waterway_structure(waterway);
    }
    if let Some(leisure) = tags.get("leisure") {
        return leisure != "playground";
    }
    if let Some(man_made) = tags.get("man_made") {
        return matches!(man_made.as_str(), "pier" | "breakwater");
    }
    if let Some(amenity) = tags.get("amenity") {
        return amenity == "parking";
    }
    [
        "highway", "railway", "natural", "landuse", "water", "aeroway", "place",
    ]
    .iter()
    .any(|k| tags.contains_key(*k))
}

fn is_waterway_structure(waterway: &str) -> bool {
    matches!(waterway, "dam" | "weir" | "lock_gate" | "boatyard" | "fuel")
}

fn tag<'a>(tags: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    tags.get(key).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm_parser::{ProcessedNode, ProcessedWay};

    fn way(tags: &[(&str, &str)]) -> ProcessedElement {
        ProcessedElement::Way(ProcessedWay {
            id: 1,
            nodes: Vec::new(),
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    fn node(tags: &[(&str, &str)]) -> ProcessedElement {
        ProcessedElement::Node(ProcessedNode {
            id: 2,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            x: 0,
            z: 0,
        })
    }

    #[test]
    fn terrain_only_keeps_water_and_nature() {
        assert!(is_terrain_feature(&way(&[("natural", "water")])));
        assert!(is_terrain_feature(&way(&[("waterway", "river")])));
        assert!(is_terrain_feature(&way(&[("natural", "wood")])));
        assert!(is_terrain_feature(&way(&[("landuse", "reservoir")])));
        assert!(is_terrain_feature(&way(&[("water", "lake")])));
    }

    #[test]
    fn terrain_only_drops_what_people_built() {
        assert!(!is_terrain_feature(&way(&[("building", "yes")])));
        assert!(!is_terrain_feature(&way(&[("highway", "residential")])));
        assert!(!is_terrain_feature(&way(&[("waterway", "dam")])));
        assert!(!is_terrain_feature(&way(&[("landuse", "farmland")])));
        assert!(!is_terrain_feature(&way(&[("natural", "tree_row")])));
        assert!(!is_terrain_feature(&node(&[("natural", "tree")])));
        assert!(!is_terrain_feature(&way(&[
            ("natural", "water"),
            ("amenity", "fountain")
        ])));
        assert!(!is_terrain_feature(&way(&[
            ("natural", "water"),
            ("leisure", "swimming_pool")
        ])));
    }

    #[test]
    fn blueprint_keeps_the_plan_and_drops_the_furniture() {
        assert!(is_blueprint_feature(&way(&[("building", "house")])));
        assert!(is_blueprint_feature(&way(&[("highway", "primary")])));
        assert!(is_blueprint_feature(&way(&[("railway", "rail")])));
        assert!(is_blueprint_feature(&way(&[("leisure", "park")])));
        assert!(is_blueprint_feature(&way(&[("amenity", "parking")])));
        assert!(is_blueprint_feature(&node(&[("natural", "tree")])));
        assert!(!is_blueprint_feature(&way(&[("building:part", "yes")])));
        assert!(!is_blueprint_feature(&way(&[
            ("building", "yes"),
            ("building:part", "yes")
        ])));
        assert!(is_blueprint_feature(&way(&[
            ("building", "yes"),
            ("building:part", "no")
        ])));
        assert!(!is_blueprint_feature(&way(&[("amenity", "school")])));
        assert!(!is_blueprint_feature(&way(&[("leisure", "playground")])));
        assert!(!is_blueprint_feature(&way(&[("power", "line")])));
        assert!(!is_blueprint_feature(&node(&[("highway", "street_lamp")])));
        assert!(!is_blueprint_feature(&node(&[("amenity", "bench")])));
    }
}
