use crate::{
    args::Args, bbox::BBox, coordinate_system::cartesian::XZPoint, geo_coord::GeoCoord,
    progress::emit_gui_progress_update,
};
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

#[derive(Debug, Clone)]
pub struct ProcessedWay {
    pub id: u64,
    pub nodes: Vec<ProcessedNode>,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, PartialEq)]
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
    pub id: u64,
    pub tags: HashMap<String, String>,
    pub members: Vec<ProcessedMember>,
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
    bbox: BBox, // (min_lon, min_lat, max_lon, max_lat)
    scale_factor_z: f64,
    scale_factor_x: f64,
) -> (i32, i32) {
    // Calculate the relative position within the bounding box
    let rel_x: f64 = (lon - bbox.min().lng()) / (bbox.max().lng() - bbox.min().lng());
    let rel_z: f64 = 1.0 - (lat - bbox.min().lat()) / (bbox.max().lat() - bbox.min().lat());

    // Apply scaling factors for each dimension and convert to Minecraft coordinates
    let x: i32 = (rel_x * scale_factor_x) as i32;
    let z: i32 = (rel_z * scale_factor_z) as i32;

    (x, z)
}

pub fn parse_osm_data(
    json_data: &Value,
    bbox: BBox,
    args: &Args,
) -> (Vec<ProcessedElement>, f64, f64) {
    println!("{} Parsing data...", "[2/6]".bold());
    emit_gui_progress_update(5.0, "Parsing data...");

    // Deserialize the JSON data into the OSMData structure
    let data: OsmData =
        serde_json::from_value(json_data.clone()).expect("Failed to parse OSM data");

    // Determine which dimension is larger and assign scale factors accordingly
    let (scale_factor_z, scale_factor_x) = geo_distance(bbox.min(), bbox.max());
    let scale_factor_z: f64 = scale_factor_z.floor() * args.scale;
    let scale_factor_x: f64 = scale_factor_x.floor() * args.scale;

    if args.debug {
        println!("Scale factor X: {}", scale_factor_x);
        println!("Scale factor Z: {}", scale_factor_z);
    }

    let mut nodes_map: HashMap<u64, ProcessedNode> = HashMap::new();
    let mut ways_map: HashMap<u64, ProcessedWay> = HashMap::new();

    let mut processed_elements: Vec<ProcessedElement> = Vec::new();

    // First pass: store all nodes with Minecraft coordinates and process nodes with tags
    for element in &data.elements {
        if element.r#type == "node" {
            if let (Some(lat), Some(lon)) = (element.lat, element.lon) {
                let (x, z) =
                    lat_lon_to_minecraft_coords(lat, lon, bbox, scale_factor_z, scale_factor_x);

                let processed: ProcessedNode = ProcessedNode {
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
    for element in &data.elements {
        if element.r#type != "relation" {
            continue;
        }

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
                    eprintln!("WARN: Unknown relation type {}", mem.r#type);
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

    emit_gui_progress_update(10.0, "");

    (processed_elements, scale_factor_x, scale_factor_z)
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

// (lat meters, lon meters)
#[inline]
pub fn geo_distance(a: GeoCoord, b: GeoCoord) -> (f64, f64) {
    let z: f64 = lat_distance(a.lat(), b.lat());

    // distance between two lons depends on their latitude. In this case we'll just average them
    let x: f64 = lon_distance((a.lat() + b.lat()) / 2.0, a.lng(), b.lng());

    (z, x)
}

// Haversine but optimized for a latitude delta of 0
// returns meters
fn lon_distance(lat: f64, lon1: f64, lon2: f64) -> f64 {
    const R: f64 = 6_371_000.0;
    let d_lon: f64 = (lon2 - lon1).to_radians();
    let a: f64 =
        lat.to_radians().cos() * lat.to_radians().cos() * (d_lon / 2.0).sin() * (d_lon / 2.0).sin();
    let c: f64 = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());

    R * c
}

// Haversine but optimized for a longitude delta of 0
// returns meters
fn lat_distance(lat1: f64, lat2: f64) -> f64 {
    const R: f64 = 6_371_000.0;
    let d_lat: f64 = (lat2 - lat1).to_radians();
    let a: f64 = (d_lat / 2.0).sin() * (d_lat / 2.0).sin();
    let c: f64 = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());

    R * c
}
