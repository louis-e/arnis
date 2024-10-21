use crate::args::Args;
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

#[derive(Debug, Deserialize)]
struct OsmData {
    pub elements: Vec<OsmElement>,
}

// End raw data

// Normalized data that we can use

#[derive(Debug, Clone)]
pub struct ProcessedNode {
    pub id: u64,
    pub tags: HashMap<String, String>,

    // minecraft coords
    pub x: i32,
    pub z: i32,
}

#[derive(Debug, Clone)]
pub struct ProcessedWay {
    pub id: u64,
    pub nodes: Vec<ProcessedNode>,
    pub tags: HashMap<String, String>,
}

#[derive(Debug)]
pub enum ProcessedMemberRole {
    Outer,
    Inner,
}

#[derive(Debug)]
pub struct ProcessedMember {
    pub role: ProcessedMemberRole,
    pub way: ProcessedWay,
}

#[derive(Debug)]
pub struct ProcessedRelation {
    id: u64,
    pub members: Vec<ProcessedMember>,
    pub tags: HashMap<String, String>,
}

#[derive(Debug)]
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

// Function to convert latitude and longitude to Minecraft coordinates.
fn lat_lon_to_minecraft_coords(
    lat: f64,
    lon: f64,
    bbox: (f64, f64, f64, f64), // (min_lon, min_lat, max_lon, max_lat)
    scale_factor_x: f64,
    scale_factor_z: f64,
) -> (i32, i32) {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;

    // Calculate the relative position within the bounding box
    let rel_x: f64 = 1.0 - (lat - min_lat) / (max_lat - min_lat);
    let rel_z: f64 = (lon - min_lon) / (max_lon - min_lon);

    // Apply scaling factors for each dimension and convert to Minecraft coordinates
    let x: i32 = (rel_x * scale_factor_x) as i32;
    let z: i32 = (rel_z * scale_factor_z) as i32;

    (z, x) // Swap x and z coords to avoid a mirrored projection on the Minecraft map
}

/// Function to determine the number of decimal places in a float as a string
fn count_decimal_places(value: f64) -> usize {
    let s: String = value.to_string();
    if let Some(pos) = s.find('.') {
        s.len() - pos - 1 // Number of digits after the decimal point
    } else {
        0
    }
}

/// Function to convert f64 to an integer based on the number of decimal places
fn convert_to_scaled_int(value: f64, max_decimal_places: usize) -> i64 {
    let multiplier: i64 = 10_i64.pow(max_decimal_places as u32); // Compute multiplier
    (value * multiplier as f64).round() as i64 // Scale and convert to integer
}

pub fn parse_osm_data(
    json_data: &Value,
    bbox: (f64, f64, f64, f64),
    args: &Args,
) -> (Vec<ProcessedElement>, f64, f64) {
    println!("{} Parsing data...", "[2/5]".bold());

    // Deserialize the JSON data into the OSMData structure
    let data: OsmData =
        serde_json::from_value(json_data.clone()).expect("Failed to parse OSM data");

    // Calculate the maximum number of decimal places in bbox elements
    let max_decimal_places: usize = [
        count_decimal_places(bbox.0),
        count_decimal_places(bbox.1),
        count_decimal_places(bbox.2),
        count_decimal_places(bbox.3),
    ]
    .into_iter()
    .max()
    .unwrap();

    // Convert each element to a scaled integer
    let bbox_scaled: (i64, i64, i64, i64) = (
        convert_to_scaled_int(bbox.0, max_decimal_places),
        convert_to_scaled_int(bbox.1, max_decimal_places),
        convert_to_scaled_int(bbox.2, max_decimal_places),
        convert_to_scaled_int(bbox.3, max_decimal_places),
    );

    // Determine which dimension is larger and assign scale factors accordingly
    let (scale_factor_x, scale_factor_z) =
        if (bbox_scaled.2 - bbox_scaled.0) > (bbox_scaled.3 - bbox_scaled.1) {
            // Longitude difference is greater than latitude difference
            (
                ((bbox_scaled.3 - bbox_scaled.1) * 14 / 100) as f64, // Scale for width (x) is based on latitude difference
                ((bbox_scaled.2 - bbox_scaled.0) * 10 / 100) as f64, // Scale for length (z) is based on longitude difference
            )
        } else {
            // Latitude difference is greater than or equal to longitude difference
            (
                ((bbox_scaled.2 - bbox_scaled.0) * 14 / 100) as f64, // Scale for width (x) is based on longitude difference
                ((bbox_scaled.3 - bbox_scaled.1) * 10 / 100) as f64, // Scale for length (z) is based on latitude difference
            )
        };

    if args.debug {
        println!("Scale factor X: {}", scale_factor_x);
        println!("Scale factor Z: {}", scale_factor_z);
    }

    let mut nodes_map = HashMap::new();
    let mut ways_map = HashMap::new();

    let mut processed_elements: Vec<ProcessedElement> = Vec::new();

    // First pass: store all nodes with Minecraft coordinates and process nodes with tags
    for element in &data.elements {
        if element.r#type == "node" {
            if let (Some(lat), Some(lon)) = (element.lat, element.lon) {
                let (x, z) =
                    lat_lon_to_minecraft_coords(lat, lon, bbox, scale_factor_x, scale_factor_z);

                let processed = ProcessedNode {
                    id: element.id,
                    tags: element.tags.clone().unwrap_or_default(),
                    x,
                    z,
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
    }

    // Second pass: process ways
    for element in &data.elements {
        if element.r#type != "way" {
            continue;
        }

        let mut nodes = vec![];
        if let Some(node_ids) = &element.nodes {
            for &node_id in node_ids {
                if let Some(node) = nodes_map.get(&node_id) {
                    nodes.push(node.clone());
                }
            }
        }

        let processed = ProcessedWay {
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
    for element in &data.elements {
        if element.r#type != "relation" {
            continue;
        }

        let Some(tags) = &element.tags else {
            continue;
        };

        // Only process multipolygons for now
        if tags.get("type").map(|x| x.as_str()) != Some("multipolygon") {
            continue;
        };

        let members = element
            .members
            .iter()
            .filter_map(|mem| {
                if mem.r#type != "way" {
                    eprintln!("WARN: Unknown relation type {}", mem.r#type);
                    return None;
                }

                let role = match mem.role.as_str() {
                    "outer" => ProcessedMemberRole::Outer,
                    "inner" => ProcessedMemberRole::Inner,
                    _ => {
                        // We only care about outer/inner because
                        // we just want multipolygons at the current time

                        return None;
                    }
                };

                let way = ways_map
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

    (processed_elements, scale_factor_z, scale_factor_x)
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
