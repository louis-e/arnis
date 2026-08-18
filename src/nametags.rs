//! Floats a name/category label above every named or categorized OSM element,
//! independent of any other feature — enabled with `--nametags`. Java only.

use std::collections::HashMap;

use fastnbt::Value;

use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;

/// Checked in priority order against each element's tags to pick a category label;
/// the first one present wins.
const CATEGORY_TAGS: &[&str] = &[
    "amenity", "shop", "leisure", "tourism", "railway", "highway", "building", "office", "craft",
    "landuse", "natural", "man_made",
];

/// `fast_food` -> "Fast Food".
fn title_case_tag_value(value: &str) -> String {
    value
        .replace('_', " ")
        .split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The first `CATEGORY_TAGS` value present on `tags`, title-cased
/// (`amenity=fast_food` -> "Fast Food"), or `None` if none of them are present.
fn find_category_tag(tags: &HashMap<String, String>) -> Option<String> {
    // OSM's boolean-flag convention (`building=yes`, `building=no`) is by far the
    // most common `building` value and carries no information — skip to the next
    // candidate tag instead of surfacing a meaningless "Yes" label.
    const BOOLEAN_PLACEHOLDERS: &[&str] = &["yes", "no", "true", "false"];
    CATEGORY_TAGS
        .iter()
        .filter_map(|key| tags.get(*key))
        .find(|value| !BOOLEAN_PLACEHOLDERS.contains(&value.as_str()))
        .map(|value| title_case_tag_value(value))
}

/// The `name` tag if present, else `"<housenumber> <street>"` synthesized from
/// `addr:housenumber`/`addr:street` (common on buildings that carry an address but
/// no proper name — "123 Main Street" is still a meaningful label). Elements with
/// neither are skipped entirely; a bare category label like "Restaurant" alone is
/// handled separately by `find_category_tag`.
fn element_display_name(tags: &HashMap<String, String>) -> Option<String> {
    if let Some(name) = tags.get("name") {
        return Some(name.clone());
    }
    let house_number = tags.get("addr:housenumber")?;
    let street = tags.get("addr:street")?;
    Some(format!("{house_number} {street}"))
}

/// Average of an element's node coordinates, or `None` for a relation (which carries
/// no nodes of its own — see `ProcessedElement::nodes`).
fn element_centroid(element: &ProcessedElement) -> Option<(i32, i32)> {
    let mut sum_x: i64 = 0;
    let mut sum_z: i64 = 0;
    let mut count: i64 = 0;
    for node in element.nodes() {
        sum_x += node.x as i64;
        sum_z += node.z as i64;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    Some(((sum_x / count) as i32, (sum_z / count) as i32))
}

/// Safety ceiling on how many *elements* get labeled (each can produce up to two
/// physical labels) — nametags aren't gated behind any name/category allowlist, so a
/// dense city could otherwise want a label on nearly every element.
const NAMETAG_MAX_ELEMENTS: usize = 3000;

/// How far above ground each element's floating nametag hovers — high enough to
/// clear most rooftops without needing per-building height data.
const NAMETAG_HEIGHT: i32 = 5;
/// The category tag floats a couple blocks above the name/address tag, so an element
/// with both shows two stacked labels instead of one overwriting the other.
const CATEGORY_NAMETAG_HEIGHT: i32 = NAMETAG_HEIGHT + 2;

/// A floating label pair for one real-world location: its name/address (if any) and
/// its OSM category tag (if any). An element needs only one of the two to qualify.
pub struct NametagLabel {
    pub name: Option<String>,
    pub category: Option<String>,
    pub x: i32,
    pub z: i32,
}

/// Every element with a name (or address fallback) or a `CATEGORY_TAGS` tag. Elements
/// with a name are kept ahead of category-only elements when `NAMETAG_MAX_ELEMENTS`
/// caps the list, since a name is strictly more useful than a bare "Restaurant".
pub fn collect_nametag_labels(elements: &[ProcessedElement]) -> Vec<NametagLabel> {
    let mut named = Vec::new();
    let mut category_only = Vec::new();

    for element in elements {
        let tags = element.tags();
        let name = element_display_name(tags);
        let category = find_category_tag(tags);
        if name.is_none() && category.is_none() {
            continue;
        }
        let Some((x, z)) = element_centroid(element) else {
            continue;
        };
        let is_named = name.is_some();
        let label = NametagLabel {
            name,
            category,
            x,
            z,
        };
        if is_named {
            named.push(label);
        } else {
            category_only.push(label);
        }
    }

    named.extend(category_only);
    named.truncate(NAMETAG_MAX_ELEMENTS);
    named
}

/// Floats `minecraft:text_display` label(s) over each location's real in-world
/// coordinates — the name/address at `NAMETAG_HEIGHT`, its category stacked above at
/// `CATEGORY_NAMETAG_HEIGHT` when both are present. `billboard: "center"` always
/// faces the viewer; `see_through: 1` keeps it legible through light terrain/foliage.
/// Must run before `editor.save()` — plain entity API, no post-save NBT surgery needed.
pub fn place_nametags(editor: &mut WorldEditor, labels: &[NametagLabel]) {
    for label in labels {
        if let Some(name) = &label.name {
            place_text_display(editor, name, label.x, NAMETAG_HEIGHT, label.z);
        }
        if let Some(category) = &label.category {
            place_text_display(editor, category, label.x, CATEGORY_NAMETAG_HEIGHT, label.z);
        }
    }
}

fn place_text_display(editor: &mut WorldEditor, text: &str, x: i32, y: i32, z: i32) {
    let mut extra = HashMap::new();
    extra.insert("text".to_string(), Value::String(format!("\"{text}\"")));
    extra.insert("billboard".to_string(), Value::String("center".to_string()));
    extra.insert("see_through".to_string(), Value::Byte(1));
    editor.add_entity("minecraft:text_display", x, y, z, Some(extra));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::osm_parser::{ProcessedNode, ProcessedWay};

    fn node_with_tags(tags: Vec<(&str, &str)>, x: i32, z: i32) -> ProcessedElement {
        let tags = tags
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        ProcessedElement::Node(ProcessedNode { id: 1, tags, x, z })
    }

    #[test]
    fn collects_named_and_category_only_elements() {
        let elements = vec![
            node_with_tags(vec![("amenity", "restaurant")], 0, 0), // no name -> category only
            node_with_tags(
                vec![("name", "Joe's Diner"), ("amenity", "restaurant")],
                1,
                0,
            ), // both
            node_with_tags(vec![("landuse", "residential")], 2, 0), // no name, category only
        ];
        let labels = collect_nametag_labels(&elements);
        assert_eq!(labels.len(), 3);
        // Named elements are ordered ahead of category-only ones.
        assert_eq!(labels[0].name.as_deref(), Some("Joe's Diner"));
        assert_eq!(labels[0].category.as_deref(), Some("Restaurant"));
        assert!(labels[1].name.is_none());
        assert!(labels[2].name.is_none());
        let categories: Vec<&str> = labels[1..]
            .iter()
            .map(|l| l.category.as_deref().unwrap())
            .collect();
        assert!(categories.contains(&"Restaurant"));
        assert!(categories.contains(&"Residential"));
    }

    #[test]
    fn falls_back_to_address_when_name_is_missing() {
        let elements = vec![
            // No name, but has both address tags -> synthesized "123 Main Street".
            node_with_tags(
                vec![
                    ("amenity", "cafe"),
                    ("addr:housenumber", "123"),
                    ("addr:street", "Main Street"),
                ],
                0,
                0,
            ),
        ];
        let labels = collect_nametag_labels(&elements);
        assert_eq!(labels[0].name.as_deref(), Some("123 Main Street"));
    }

    #[test]
    fn skips_boolean_flag_tags_like_building_yes() {
        // building=yes carries no information; landuse=residential (further down the
        // priority list) should be surfaced instead of a meaningless "Yes" label.
        let elements = vec![node_with_tags(
            vec![("building", "yes"), ("landuse", "residential")],
            0,
            0,
        )];
        let labels = collect_nametag_labels(&elements);
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0].category.as_deref(), Some("Residential"));

        // No usable tag at all once the boolean flag is skipped -> no label.
        let elements = vec![node_with_tags(vec![("building", "yes")], 0, 0)];
        assert!(collect_nametag_labels(&elements).is_empty());
    }

    #[test]
    fn way_uses_node_centroid() {
        let mut tags = HashMap::new();
        tags.insert("name".to_string(), "Terminal Building".to_string());
        let way = ProcessedElement::Way(ProcessedWay {
            id: 1,
            tags,
            nodes: vec![
                ProcessedNode {
                    id: 1,
                    tags: HashMap::new(),
                    x: 0,
                    z: 0,
                },
                ProcessedNode {
                    id: 2,
                    tags: HashMap::new(),
                    x: 10,
                    z: 20,
                },
            ],
        });
        let labels = collect_nametag_labels(&[way]);
        assert_eq!(labels.len(), 1);
        assert_eq!((labels[0].x, labels[0].z), (5, 10));
    }

    #[test]
    fn place_nametags_does_not_panic_on_name_and_category_labels() {
        let xzbbox = XZBBox::rect_from_min_max(-50, -50, 50, 50).unwrap();
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let mut editor = WorldEditor::new(std::env::temp_dir(), &xzbbox, llbbox);

        let labels = vec![
            NametagLabel {
                name: Some("Central Station".to_string()),
                category: Some("Station".to_string()),
                x: 20,
                z: 20,
            },
            NametagLabel {
                name: None,
                category: Some("Restaurant".to_string()),
                x: -30,
                z: 15,
            },
        ];
        place_nametags(&mut editor, &labels);
    }
}
