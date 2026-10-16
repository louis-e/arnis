//! Named places worth walking to, picked from the OSM data, and the advancement tab that
//! records which ones a player has found.

use crate::coordinate_system::cartesian::XZBBox;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode};
use serde_json::{json, Value};
use std::collections::HashMap;

/// Most places one tab shows; the tree is one column per category, so more is a long scroll.
const MAX_PLACES: usize = 72;
const MAX_PER_CATEGORY: usize = 16;
/// How close a mapped point has to be walked to, in blocks.
const POINT_RADIUS: i32 = 8;
/// Slack around a mapped outline, so walking up to a building counts.
const OUTLINE_PAD: i32 = 3;
/// Folder of the tab's advancements.
pub const FOLDER: &str = "main";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Landmark,
    Culture,
    Viewpoint,
    Nature,
    Faith,
    Transport,
    Bridge,
    Sport,
    Civic,
}

impl Category {
    fn key(self) -> &'static str {
        match self {
            Category::Landmark => "landmarks",
            Category::Culture => "culture",
            Category::Viewpoint => "viewpoints",
            Category::Nature => "nature",
            Category::Faith => "faith",
            Category::Transport => "transport",
            Category::Bridge => "bridges",
            Category::Sport => "sport",
            Category::Civic => "civic",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Category::Landmark => "Landmarks",
            Category::Culture => "Culture",
            Category::Viewpoint => "Views and Peaks",
            Category::Nature => "Parks and Nature",
            Category::Faith => "Places of Worship",
            Category::Transport => "Stations",
            Category::Bridge => "Bridges",
            Category::Sport => "Sport",
            Category::Civic => "Civic",
        }
    }

    // Item ids present from 1.21.4 to 26.3: a renamed one fails the whole tab.
    fn icon(self) -> &'static str {
        match self {
            Category::Landmark => "minecraft:chiseled_stone_bricks",
            Category::Culture => "minecraft:painting",
            Category::Viewpoint => "minecraft:spyglass",
            Category::Nature => "minecraft:oak_sapling",
            Category::Faith => "minecraft:bell",
            Category::Transport => "minecraft:minecart",
            Category::Bridge => "minecraft:stone_bricks",
            Category::Sport => "minecraft:target",
            Category::Civic => "minecraft:book",
        }
    }

    fn weight(self) -> i32 {
        match self {
            Category::Landmark => 50,
            Category::Culture => 45,
            Category::Viewpoint => 35,
            Category::Nature => 30,
            Category::Transport => 30,
            Category::Faith => 25,
            Category::Bridge => 25,
            Category::Sport => 25,
            Category::Civic => 20,
        }
    }
}

/// A place to discover: walking into `min..=max` finds it.
#[derive(Clone, Debug)]
pub struct Place {
    pub name: String,
    pub category: Category,
    /// What kind of place, shown under the name.
    pub label: String,
    pub min: (i32, i32),
    pub max: (i32, i32),
    /// Where travel drops the player.
    pub anchor: (i32, i32),
    /// Ground height at `anchor`, once settled; travel searches down from a little above it.
    pub ground_y: Option<i32>,
    pub notable: bool,
    score: i32,
}

/// The places worth discovering, best first, and the most important `place=*` name as a
/// fallback for the area's name.
pub fn collect(elements: &[ProcessedElement], world: &XZBBox) -> (Vec<Place>, Option<String>) {
    let mut found: Vec<Place> = Vec::new();
    let mut settlement: Option<(u8, String)> = None;
    for element in elements {
        let tags = element.tags();
        if let (Some(rank), Some(name)) = (settlement_rank(tags), tags.get("name")) {
            if settlement.as_ref().is_none_or(|(r, _)| rank > *r) {
                settlement = Some((rank, name.clone()));
            }
        }
        let Some(name) = tags.get("name").filter(|n| !n.trim().is_empty()) else {
            continue;
        };
        let Some((category, label)) = classify(element) else {
            continue;
        };
        let Some(((min, max), point)) = outline(element) else {
            continue;
        };
        let (pad, area) = if point {
            (POINT_RADIUS, 0)
        } else {
            (
                OUTLINE_PAD,
                (max.0 - min.0 + 1) as i64 * (max.1 - min.1 + 1) as i64,
            )
        };
        let min = (
            (min.0 - pad).max(world.min_x()),
            (min.1 - pad).max(world.min_z()),
        );
        let max = (
            (max.0 + pad).min(world.max_x()),
            (max.1 + pad).min(world.max_z()),
        );
        if min.0 > max.0 || min.1 > max.1 {
            continue;
        }
        let notable = tags.contains_key("wikidata") || tags.contains_key("wikipedia");
        let mut score = category.weight();
        if tags.contains_key("wikidata") {
            score += 100;
        }
        if tags.contains_key("wikipedia") {
            score += 60;
        }
        if tags.contains_key("heritage") {
            score += 30;
        }
        if tags.get("tourism").map(String::as_str) == Some("attraction") {
            score += 20;
        }
        if area > 0 {
            score += ((area as f64).log2() * 3.0).min(30.0) as i32;
        }
        let anchor = if point {
            let ProcessedElement::Node(n) = element else {
                unreachable!()
            };
            (n.x, n.z)
        } else {
            ((min.0 + max.0) / 2, (min.1 + max.1) / 2)
        };
        found.push(Place {
            name: name.trim().to_string(),
            category,
            label: label.to_string(),
            min,
            max,
            anchor,
            ground_y: None,
            notable,
            score,
        });
    }
    (select(found), settlement.map(|(_, n)| n))
}

/// How far from its centre a place's landing spot may be, in blocks.
const SETTLE_RADIUS: i32 = 48;
/// How much further than the nearest open cell a street may be and still be preferred.
const STREET_SLACK: i32 = 6;

impl Place {
    /// Moves the travel anchor off buildings and water to the nearest open cell, or to a
    /// street a few blocks further out, and records the ground height there.
    pub fn settle(
        &mut self,
        open: impl Fn(i32, i32) -> bool,
        street: impl Fn(i32, i32) -> bool,
        ground_y: impl Fn(i32, i32) -> i32,
    ) {
        let (cx, cz) = self.anchor;
        let ring = |r: i32| {
            (-r..=r)
                .flat_map(move |d| {
                    [
                        (cx + d, cz - r),
                        (cx + d, cz + r),
                        (cx - r, cz + d),
                        (cx + r, cz + d),
                    ]
                })
                .filter(|&(x, z)| open(x, z))
        };
        if let Some((r0, first)) =
            (0..=SETTLE_RADIUS).find_map(|r| ring(r).next().map(|cell| (r, cell)))
        {
            self.anchor = (r0..=(r0 + STREET_SLACK).min(SETTLE_RADIUS))
                .find_map(|r| ring(r).find(|&(x, z)| street(x, z)))
                .unwrap_or(first);
        }
        self.ground_y = Some(ground_y(self.anchor.0, self.anchor.1));
    }
}

/// Merges the pieces one place is mapped as, then keeps the best of each category.
fn select(mut found: Vec<Place>) -> Vec<Place> {
    found.sort_by_key(|p| std::cmp::Reverse(p.score));
    let mut merged: Vec<Place> = Vec::new();
    for place in found {
        // A long bridge or a park is often several ways with one name.
        let near = |m: &Place| {
            m.category == place.category
                && m.name.eq_ignore_ascii_case(&place.name)
                && place.min.0 <= m.max.0 + 64
                && place.max.0 >= m.min.0 - 64
                && place.min.1 <= m.max.1 + 64
                && place.max.1 >= m.min.1 - 64
        };
        if let Some(m) = merged.iter_mut().find(|m| near(m)) {
            m.min = (m.min.0.min(place.min.0), m.min.1.min(place.min.1));
            m.max = (m.max.0.max(place.max.0), m.max.1.max(place.max.1));
            m.notable |= place.notable;
            continue;
        }
        // Chains and same-named stops elsewhere are not one place, and not worth two.
        if merged
            .iter()
            .any(|m| m.category == place.category && m.name.eq_ignore_ascii_case(&place.name))
        {
            continue;
        }
        merged.push(place);
    }
    let mut per_category: HashMap<Category, usize> = HashMap::new();
    merged.retain(|p| {
        let n = per_category.entry(p.category).or_default();
        *n += 1;
        *n <= MAX_PER_CATEGORY
    });
    merged.truncate(MAX_PLACES);
    merged
}

fn settlement_rank(tags: &HashMap<String, String>) -> Option<u8> {
    Some(match tags.get("place")?.as_str() {
        "city" => 6,
        "town" => 5,
        "borough" | "suburb" => 4,
        "village" => 3,
        "quarter" | "neighbourhood" => 2,
        "hamlet" => 1,
        _ => return None,
    })
}

fn classify(element: &ProcessedElement) -> Option<(Category, &'static str)> {
    let tags = element.tags();
    let get = |k: &str| tags.get(k).map(String::as_str);
    let is_node = matches!(element, ProcessedElement::Node(_));
    let notable = tags.contains_key("wikidata") || tags.contains_key("wikipedia");

    if let Some(c) = match get("tourism") {
        Some("museum") => Some((Category::Culture, "Museum")),
        Some("gallery") => Some((Category::Culture, "Gallery")),
        Some("zoo") => Some((Category::Culture, "Zoo")),
        Some("aquarium") => Some((Category::Culture, "Aquarium")),
        Some("theme_park") => Some((Category::Culture, "Theme park")),
        Some("viewpoint") => Some((Category::Viewpoint, "Viewpoint")),
        _ => None,
    } {
        return Some(c);
    }
    if let Some(c) = match get("historic") {
        Some("castle") => Some((Category::Landmark, "Castle")),
        Some("palace") => Some((Category::Landmark, "Palace")),
        Some("monument") => Some((Category::Landmark, "Monument")),
        Some("city_gate") => Some((Category::Landmark, "City gate")),
        Some("fort") => Some((Category::Landmark, "Fort")),
        Some("ruins") => Some((Category::Landmark, "Ruins")),
        Some("tower") => Some((Category::Landmark, "Historic tower")),
        Some("archaeological_site") => Some((Category::Landmark, "Archaeological site")),
        Some("memorial") if notable => Some((Category::Landmark, "Memorial")),
        _ => None,
    } {
        return Some(c);
    }
    match get("amenity") {
        Some("theatre") => return Some((Category::Culture, "Theatre")),
        Some("arts_centre") => return Some((Category::Culture, "Arts centre")),
        Some("library") => return Some((Category::Culture, "Library")),
        Some("concert_hall") => return Some((Category::Culture, "Concert hall")),
        Some("place_of_worship") if !is_node || notable => {
            let label = match get("religion") {
                Some("christian") => "Church",
                Some("muslim") => "Mosque",
                Some("jewish") => "Synagogue",
                Some("buddhist" | "hindu" | "shinto" | "sikh" | "jain") => "Temple",
                _ => "Place of worship",
            };
            return Some((Category::Faith, label));
        }
        Some("townhall") => return Some((Category::Civic, "Town hall")),
        Some("university") => return Some((Category::Civic, "University")),
        Some("courthouse") => return Some((Category::Civic, "Courthouse")),
        Some("ferry_terminal") => return Some((Category::Transport, "Ferry terminal")),
        Some("bus_station") => return Some((Category::Transport, "Bus station")),
        _ => {}
    }
    if get("tourism") == Some("attraction") {
        return Some((Category::Landmark, "Attraction"));
    }
    match get("man_made") {
        Some("lighthouse") => return Some((Category::Landmark, "Lighthouse")),
        Some("windmill") => return Some((Category::Landmark, "Windmill")),
        Some("obelisk") => return Some((Category::Landmark, "Obelisk")),
        Some("tower") if notable || get("tower:type") == Some("observation") => {
            return Some((Category::Landmark, "Tower"))
        }
        Some("bridge") => return Some((Category::Bridge, "Bridge")),
        _ => {}
    }
    if get("natural") == Some("peak") {
        return Some((Category::Viewpoint, "Peak"));
    }
    if !is_node {
        let bridged = matches!(get("bridge"), Some(v) if v != "no");
        if bridged && (tags.contains_key("highway") || tags.contains_key("railway")) {
            return Some((Category::Bridge, "Bridge"));
        }
        match (get("leisure"), get("natural"), get("landuse")) {
            (Some("park"), _, _) => return Some((Category::Nature, "Park")),
            (Some("garden"), _, _) => return Some((Category::Nature, "Garden")),
            (Some("nature_reserve"), _, _) => return Some((Category::Nature, "Nature reserve")),
            (Some("stadium"), _, _) => return Some((Category::Sport, "Stadium")),
            (Some("sports_centre"), _, _) => return Some((Category::Sport, "Sports centre")),
            (_, Some("beach"), _) => return Some((Category::Nature, "Beach")),
            (_, Some("water"), _)
                if !matches!(get("water"), Some("river" | "canal" | "stream")) =>
            {
                return Some((Category::Nature, "Lake"))
            }
            (_, Some("wood"), _) | (_, _, Some("forest")) => {
                return Some((Category::Nature, "Forest"))
            }
            _ => {}
        }
        if get("place") == Some("island") {
            return Some((Category::Nature, "Island"));
        }
    }
    let station = get("railway") == Some("station")
        || (get("public_transport") == Some("station")
            && (tags.contains_key("railway")
                || matches!(get("train"), Some("yes"))
                || matches!(get("subway"), Some("yes"))));
    if station {
        return Some((Category::Transport, "Station"));
    }
    if get("aeroway") == Some("aerodrome") {
        return Some((Category::Transport, "Airport"));
    }
    None
}

/// Lowest and highest corner, in blocks.
type Extent = ((i32, i32), (i32, i32));

/// The element's extent, and whether it is a single point.
fn outline(element: &ProcessedElement) -> Option<(Extent, bool)> {
    fn extent<'n>(nodes: impl Iterator<Item = &'n ProcessedNode>) -> Option<Extent> {
        nodes.fold(None, |acc, n| {
            Some(match acc {
                None => ((n.x, n.z), (n.x, n.z)),
                Some((lo, hi)) => (
                    (lo.0.min(n.x), lo.1.min(n.z)),
                    (hi.0.max(n.x), hi.1.max(n.z)),
                ),
            })
        })
    }
    match element {
        ProcessedElement::Node(n) => Some((((n.x, n.z), (n.x, n.z)), true)),
        ProcessedElement::Way(w) => extent(w.nodes.iter()).map(|e| (e, false)),
        ProcessedElement::Relation(r) => extent(
            r.members
                .iter()
                .filter(|m| m.role != ProcessedMemberRole::Inner)
                .flat_map(|m| m.way.nodes.iter()),
        )
        .map(|e| (e, false)),
    }
}

/// How a criterion's player check is spelled, by pack format.
#[derive(Clone, Copy)]
pub enum CriteriaSchema {
    /// Up to 26.1: `condition`, plain `location`.
    Legacy,
    /// 26.2: entity predicate keys are namespaced.
    Namespaced,
    /// 26.3: conditions are typed by `type`, and one condition stands alone.
    Typed,
}

fn location_criterion(place: &Place, schema: CriteriaSchema) -> Value {
    // The same coordinates in the Nether or the End are not the place.
    let location = json!({
        "dimension": "minecraft:overworld",
        "position": {
            "x": { "min": place.min.0 as f64, "max": (place.max.0 + 1) as f64 },
            "z": { "min": place.min.1 as f64, "max": (place.max.1 + 1) as f64 },
        },
    });
    let player = match schema {
        CriteriaSchema::Legacy => json!([{
            "condition": "minecraft:entity_properties",
            "entity": "this",
            "predicate": { "location": location },
        }]),
        CriteriaSchema::Namespaced => json!([{
            "condition": "minecraft:entity_properties",
            "entity": "this",
            "predicate": { "minecraft:location": location },
        }]),
        CriteriaSchema::Typed => json!({
            "type": "minecraft:entity_properties",
            "entity": "this",
            "predicate": { "minecraft:location": location },
        }),
    };
    json!({ "trigger": "minecraft:location", "conditions": { "player": player } })
}

/// The root advancement, which differs only in how its background is named.
pub fn root(area: &str, places: usize, modern_background: bool) -> Value {
    let background = if modern_background {
        "minecraft:block/stone_bricks"
    } else {
        "minecraft:textures/block/stone_bricks.png"
    };
    json!({
        "display": {
            "icon": { "id": "minecraft:filled_map" },
            "title": area,
            "description": format!("{places} places to discover, from OpenStreetMap"),
            "background": background,
            "show_toast": false,
            "announce_to_chat": false,
        },
        "criteria": { "joined": { "trigger": "minecraft:tick" } },
    })
}

/// Every advancement but the root, as (path below `FOLDER`, JSON).
pub fn tree(area: &str, places: &[Place], schema: CriteriaSchema) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let root_id = format!("arnis:discoveries/{FOLDER}/root");
    let mut categories: Vec<Category> = places.iter().map(|p| p.category).collect();
    categories.sort();
    categories.dedup();

    for category in categories {
        let members: Vec<(usize, &Place)> = places
            .iter()
            .enumerate()
            .filter(|(_, p)| p.category == category)
            .collect();
        let criteria: serde_json::Map<String, Value> = members
            .iter()
            .map(|(i, p)| (format!("p{i}"), location_criterion(p, schema)))
            .collect();
        // Any one place of a category opens its heading.
        let any: Vec<String> = criteria.keys().cloned().collect();
        out.push((
            category.key().to_string(),
            json!({
                "parent": root_id,
                "display": {
                    "icon": { "id": category.icon() },
                    "title": category.title(),
                    "description": format!("{} to find", members.len()),
                    "show_toast": false,
                    "announce_to_chat": false,
                },
                "criteria": criteria,
                "requirements": [any],
            }),
        ));
        for (i, place) in members {
            out.push((
                format!("{}/{i}", category.key()),
                json!({
                    "parent": format!("arnis:discoveries/{FOLDER}/{}", category.key()),
                    "display": {
                        "icon": { "id": category.icon() },
                        "title": place.name,
                        "description": place.label,
                        "frame": if place.notable { "goal" } else { "task" },
                    },
                    "criteria": { "visit": location_criterion(place, schema) },
                    "rewards": { "experience": if place.notable { 25 } else { 10 } },
                }),
            ));
        }
    }

    let criteria: serde_json::Map<String, Value> = places
        .iter()
        .enumerate()
        .map(|(i, p)| (format!("p{i}"), location_criterion(p, schema)))
        .collect();
    // One requirement per place, so the tooltip counts them off.
    let each: Vec<Vec<String>> = criteria.keys().map(|k| vec![k.clone()]).collect();
    out.push((
        "explorer".to_string(),
        json!({
            "parent": root_id,
            "display": {
                "icon": { "id": "minecraft:compass" },
                "title": format!("Know {area} by Heart"),
                "description": format!("Discover all {} places", places.len()),
                "frame": "challenge",
            },
            "criteria": criteria,
            "requirements": each,
            "rewards": { "experience": 250 },
        }),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm_parser::{ProcessedNode, ProcessedWay};

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn square(id: u64, x: i32, z: i32, size: i32, t: &[(&str, &str)]) -> ProcessedElement {
        let corner = |i: u64, x, z| ProcessedNode {
            id: i,
            tags: HashMap::new(),
            x,
            z,
        };
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes: vec![
                corner(id * 10, x, z),
                corner(id * 10 + 1, x + size, z),
                corner(id * 10 + 2, x + size, z + size),
                corner(id * 10 + 3, x, z + size),
                corner(id * 10, x, z),
            ],
            tags: tags(t),
        })
    }

    #[test]
    fn a_museum_and_a_park_are_found_and_unnamed_places_are_not() {
        let world = XZBBox::rect_from_min_max(0, 0, 500, 500).unwrap();
        let elements = vec![
            square(
                1,
                10,
                10,
                20,
                &[
                    ("tourism", "museum"),
                    ("name", "Pergamon"),
                    ("wikidata", "Q1"),
                ],
            ),
            square(
                2,
                100,
                100,
                80,
                &[("leisure", "park"), ("name", "Tiergarten")],
            ),
            square(3, 300, 300, 30, &[("leisure", "park")]),
            ProcessedElement::Node(ProcessedNode {
                id: 4,
                tags: tags(&[("place", "city"), ("name", "Berlin")]),
                x: 250,
                z: 250,
            }),
        ];
        let (places, settlement) = collect(&elements, &world);
        assert_eq!(settlement.as_deref(), Some("Berlin"));
        assert_eq!(places.len(), 2);
        assert_eq!(places[0].name, "Pergamon");
        assert!(places[0].notable);
        assert_eq!(places[0].min, (7, 7));
    }

    #[test]
    fn pieces_of_one_bridge_merge_into_one_place() {
        let world = XZBBox::rect_from_min_max(0, 0, 500, 500).unwrap();
        let piece = |id, x| {
            square(
                id,
                x,
                50,
                10,
                &[
                    ("highway", "primary"),
                    ("bridge", "yes"),
                    ("name", "Oberbaumbrücke"),
                ],
            )
        };
        let (places, _) = collect(&[piece(1, 10), piece(2, 20)], &world);
        assert_eq!(places.len(), 1);
        assert_eq!((places[0].min.0, places[0].max.0), (7, 33));
    }

    #[test]
    fn travel_lands_on_the_nearest_street_outside_the_building() {
        let world = XZBBox::rect_from_min_max(0, 0, 500, 500).unwrap();
        let (mut places, _) = collect(
            &[square(
                1,
                100,
                100,
                20,
                &[("tourism", "museum"), ("name", "Museum")],
            )],
            &world,
        );
        let place = &mut places[0];
        let inside = |x: i32, z: i32| (100..=120).contains(&x) && (100..=120).contains(&z);
        // A street runs along the museum's east side.
        place.settle(|x, z| !inside(x, z), |x, _| x == 124, |_, _| 64);
        assert!(!inside(place.anchor.0, place.anchor.1));
        assert_eq!(place.anchor.0, 124);
        assert_eq!(place.ground_y, Some(64));
    }

    #[test]
    fn each_schema_spells_the_player_check_its_own_way() {
        let place = Place {
            name: "X".into(),
            category: Category::Culture,
            label: "Museum".into(),
            min: (0, 0),
            max: (9, 9),
            anchor: (5, 5),
            ground_y: None,
            notable: false,
            score: 0,
        };
        let legacy = location_criterion(&place, CriteriaSchema::Legacy);
        assert_eq!(
            legacy["conditions"]["player"][0]["predicate"]["location"]["dimension"],
            "minecraft:overworld"
        );
        let namespaced = location_criterion(&place, CriteriaSchema::Namespaced);
        assert!(
            namespaced["conditions"]["player"][0]["predicate"]["minecraft:location"].is_object()
        );
        let typed = location_criterion(&place, CriteriaSchema::Typed);
        assert_eq!(
            typed["conditions"]["player"]["type"],
            "minecraft:entity_properties"
        );
        assert_eq!(
            typed["conditions"]["player"]["predicate"]["minecraft:location"]["position"]["x"]
                ["max"],
            10.0
        );
    }
}
