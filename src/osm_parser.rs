use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

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

#[derive(Deserialize)]
struct OsmData {
    pub elements: Vec<OsmElement>,
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
        SplitOsmData {
            nodes,
            ways,
            relations,
            others,
        }
    }
}

fn parse_raw_osm_data(json_data: Value) -> Result<SplitOsmData, serde_json::Error> {
    let osm_data: OsmData = serde_json::from_value(json_data)?;
    Ok(SplitOsmData::from_raw_osm_data(osm_data))
}

// End raw data

// Normalized data that we can use

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedNode {
    pub id: u64,
    pub tags: HashMap<String, String>,

    // Minecraft coordinates
    pub x: i32,
    pub z: i32,
}

impl ProcessedNode {
    pub fn xz(&self) -> XZPoint {
        XZPoint {
            x: self.x,
            z: self.z,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedWay {
    pub id: u64,
    pub nodes: Vec<ProcessedNode>,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, PartialEq, Clone)]
pub enum ProcessedMemberRole {
    Outer,
    Inner,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedMember {
    pub role: ProcessedMemberRole,
    pub way: ProcessedWay,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedRelation {
    pub id: u64,
    pub tags: HashMap<String, String>,
    pub members: Vec<ProcessedMember>,
}

#[derive(Debug, Clone)]
pub enum ProcessedElement {
    Node(ProcessedNode),
    Way(ProcessedWay),
    Relation(ProcessedRelation),
}

impl ProcessedElement {
    pub fn tags(&self) -> &HashMap<String, String> {
        match self {
            ProcessedElement::Node(n) => &n.tags,
            ProcessedElement::Way(w) => &w.tags,
            ProcessedElement::Relation(r) => &r.tags,
        }
    }

    pub fn id(&self) -> u64 {
        match self {
            ProcessedElement::Node(n) => n.id,
            ProcessedElement::Way(w) => w.id,
            ProcessedElement::Relation(r) => r.id,
        }
    }

    pub fn kind(&self) -> &str {
        match self {
            ProcessedElement::Node(_) => "node",
            ProcessedElement::Way(_) => "way",
            ProcessedElement::Relation(_) => "relation",
        }
    }

    pub fn nodes<'a>(&'a self) -> Box<dyn Iterator<Item = &'a ProcessedNode> + 'a> {
        match self {
            ProcessedElement::Node(node) => Box::new([node].into_iter()),
            ProcessedElement::Way(way) => Box::new(way.nodes.iter()),
            ProcessedElement::Relation(_) => Box::new([].into_iter()),
        }
    }
}

pub fn parse_osm_data(
    json_data: Value,
    bbox: LLBBox,
    scale: f64,
    debug: bool,
) -> (Vec<ProcessedElement>, XZBBox) {
    println!("{} Parsing data...", "[2/6]".bold());
    emit_gui_progress_update(10.0, "Parsing data...");

    // Deserialize the JSON data into the OSMData structure
    let data = parse_raw_osm_data(json_data).expect("Failed to parse OSM data");

    let (coord_transformer, xzbbox) = CoordTransformer::llbbox_to_xzbbox(&bbox, scale)
        .unwrap_or_else(|e| {
            eprintln!("Error in defining coordinate transformation:\n{e}");
            panic!();
        });

    if debug {
        println!("Total elements: {}", data.total_count());
        println!("Scale factor X: {}", coord_transformer.scale_factor_x());
        println!("Scale factor Z: {}", coord_transformer.scale_factor_z());
    }

    let mut nodes_map: HashMap<u64, ProcessedNode> = HashMap::new();
    let mut ways_map: HashMap<u64, ProcessedWay> = HashMap::new();

    let mut processed_elements: Vec<ProcessedElement> = Vec::new();

    // First pass: store all nodes with Minecraft coordinates and process nodes with tags
    for element in data.nodes {
        if let (Some(lat), Some(lon)) = (element.lat, element.lon) {
            let llpoint = LLPoint::new(lat, lon).unwrap_or_else(|e| {
                eprintln!("Encountered invalid node element:\n{e}");
                panic!();
            });

            let xzpoint = coord_transformer.transform_point(llpoint);

            let processed: ProcessedNode = ProcessedNode {
                id: element.id,
                tags: element.tags.clone().unwrap_or_default(),
                x: xzpoint.x,
                z: xzpoint.z,
            };

            nodes_map.insert(element.id, processed.clone());

            // Process nodes with tags
            if let Some(tags) = &element.tags {
                if !tags.is_empty() {
                    processed_elements.push(ProcessedElement::Node(processed));
                }
            }
        }
    }

    // Second pass: process ways
    for element in data.ways {
        let mut nodes: Vec<ProcessedNode> = vec![];
        if let Some(node_ids) = &element.nodes {
            for &node_id in node_ids {
                if let Some(node) = nodes_map.get(&node_id) {
                    nodes.push(node.clone());
                }
            }
        }

        let processed: ProcessedWay = ProcessedWay {
            id: element.id,
            tags: element.tags.clone().unwrap_or_default(),
            nodes,
        };

        ways_map.insert(element.id, processed.clone());

        if !processed.nodes.is_empty() {
            processed_elements.push(ProcessedElement::Way(processed));
        }
    }

    // Third pass: process relations
    for element in data.relations {
        let Some(tags) = &element.tags else {
            continue;
        };

        // Only process multipolygons for now
        if tags.get("type").map(|x: &String| x.as_str()) != Some("multipolygon") {
            continue;
        };

        let members: Vec<ProcessedMember> = element
            .members
            .iter()
            .filter_map(|mem: &OsmMember| {
                if mem.r#type != "way" {
                    eprintln!("WARN: Unknown relation member type \"{}\"", mem.r#type);
                    return None;
                }

                let role = match mem.role.as_str() {
                    "outer" => ProcessedMemberRole::Outer,
                    "inner" => ProcessedMemberRole::Inner,
                    _ => return None,
                };

                let way: ProcessedWay = ways_map
                    .get(&mem.r#ref)
                    .expect("Missing a way referenced by a rel")
                    .clone();

                Some(ProcessedMember { role, way })
            })
            .collect();

        processed_elements.push(ProcessedElement::Relation(ProcessedRelation {
            id: element.id,
            members,
            tags: tags.clone(),
        }));
    }

    emit_gui_progress_update(20.0, "");

    (processed_elements, xzbbox)
}

const PRIORITY_ORDER: [&str; 6] = [
    "entrance", "building", "highway", "waterway", "water", "barrier",
];

// Function to determine the priority of each element
pub fn get_priority(element: &ProcessedElement) -> usize {
    // Check each tag against the priority order
    for (i, &tag) in PRIORITY_ORDER.iter().enumerate() {
        if element.tags().contains_key(tag) {
            return i;
        }
    }
    // Return a default priority if none of the tags match
    PRIORITY_ORDER.len()
}
