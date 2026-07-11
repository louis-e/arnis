use crate::clipping::clip_way_to_bbox;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

// Tags Arnis never reads. Filtered at parse time to save memory.
lazy_static::lazy_static! {
    static ref IGNORED_TAGS: HashSet<&'static str> = {
        let mut set = HashSet::new();
        for tag in [
            "created_by", "note", "fixme", "FIXME", "todo", "TODO",
            "wikipedia", "wikimedia_commons", "import_uuid", "import",
            "old_name", "loc_name", "official_name", "alt_name",
            "operator", "phone", "fax", "email", "url", "website",
            "opening_hours", "description", "attribution",
            "start_date", "check_date", "survey:date",
            "ref:bag", "ref:bygningsnr",
        ] {
            set.insert(tag);
        }
        set
    };

    static ref IGNORED_PREFIXES: HashSet<&'static str> = {
        let mut set = HashSet::new();
        for p in [
            "addr:", "source", "name:", "alt_name:", "contact:",
            "is_in:", "operator:", "tiger:", "NHD:", "lacounty:",
            "nysgissam:", "ref:ruian:", "building:ruian:", "osak:",
            "gnis:", "yh:", "check_date:",
        ] {
            set.insert(p);
        }
        set
    };
}

fn filter_tags(mut tags: HashMap<String, String>) -> HashMap<String, String> {
    tags.retain(|k, _| {
        !IGNORED_TAGS.contains(k.as_str()) &&
        !IGNORED_PREFIXES.iter().any(|p| k.starts_with(p))
    });
    tags
}

// Raw data from OSM
#[derive(Debug, Deserialize)]
struct OsmMember {
    r#type: String,
    r#ref: u64,
    r#role: String,
}

#[derive(Debug, Deserialize)]
struct OsmElement {
    pub r#type: String,
    pub id: u64,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub nodes: Option<Vec<u64>>,
    pub tags: Option<HashMap<String, String>>,
    #[serde(default)]
    pub members: Vec<OsmMember>,
}

#[derive(Debug, Deserialize)]
pub struct OsmData {
    elements: Vec<OsmElement>,
    #[serde(default)]
    pub remark: Option<String>,
}

impl OsmData {
    /// Returns true if there are no elements.
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }
}

struct SplitOsmData {
    pub nodes: Vec<OsmElement>,
    pub ways: Vec<OsmElement>,
    pub relations: Vec<OsmElement>,
    #[allow(dead_code)]
    pub others: Vec<OsmElement>,
}

impl SplitOsmData {
    fn total_count(&self) -> usize {
        self.nodes.len() + self.ways.len() + self.relations.len() + self.others.len()
    }

    fn from_raw_osm_data(osm_data: OsmData) -> Self {
        let mut nodes = Vec::new();
        let mut ways = Vec::new();
        let mut relations = Vec::new();
        let mut others = Vec::new();

        for element in osm_data.elements {
            match element.r#type.as_str() {
                "node" => nodes.push(element),
                "way" => ways.push(element),
                "relation" => relations.push(element),
                _ => others.push(element),
            }
        }

        SplitOsmData { nodes, ways, relations, others }
    }
}

// ... (rest of your structs: ProcessedNode, ProcessedWay, etc. remain unchanged) ...

// Keep all your other functions (parse_osm_data, compute_outline_suppression, is_water_element, get_priority, etc.)

// ====================== TESTS ======================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_osm_data_is_empty() {
        let empty = OsmData {
            elements: vec![],
            remark: None,
        };
        assert!(empty.is_empty());

        let populated = OsmData {
            elements: vec![OsmElement {
                r#type: "node".to_string(),
                id: 1,
                lat: Some(0.0),
                lon: Some(0.0),
                nodes: None,
                tags: None,
                members: vec![],
            }],
            remark: Some("dummy".to_string()),
        };
        assert!(!populated.is_empty());
    }

    // ... keep your outline_suppression_tests here too ...
}
