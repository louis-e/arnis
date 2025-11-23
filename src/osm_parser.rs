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
    println!("{} Parsing data...", "[2/7]".bold());
    println!("Bounding box: {bbox:?}");
    emit_gui_progress_update(5.0, "Parsing data...");

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

            processed_elements.push(ProcessedElement::Node(processed));
        }
    }

    // Second pass: process ways and filter nodes to bbox
    for element in data.ways {
        let mut nodes: Vec<ProcessedNode> = vec![];
        if let Some(node_ids) = &element.nodes {
            for &node_id in node_ids {
                if let Some(node) = nodes_map.get(&node_id) {
                    nodes.push(node.clone());
                }
            }
        }

        if !nodes.is_empty() {
            // Filter nodes to bounding box
            let tags = element.tags.clone().unwrap_or_default();
            let filtered_nodes = filter_nodes_to_bbox(&nodes, &xzbbox, &tags);

            if !filtered_nodes.is_empty() {
                let processed: ProcessedWay = ProcessedWay {
                    id: element.id,
                    tags,
                    nodes: filtered_nodes,
                };

                ways_map.insert(element.id, processed.clone());
                processed_elements.push(ProcessedElement::Way(processed));
            }
        }
    }

    // Third pass: process relations and clip member ways
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

                // Check if the way exists in ways_map
                let way: ProcessedWay = match ways_map.get(&mem.r#ref) {
                    Some(w) => w.clone(),
                    None => {
                        // Way was likely filtered out because it was completely outside the bbox
                        return None;
                    }
                };

                Some(ProcessedMember { role, way })
            })
            .collect();

        if !members.is_empty() {
            processed_elements.push(ProcessedElement::Relation(ProcessedRelation {
                id: element.id,
                members,
                tags: tags.clone(),
            }));
        }
    }

    emit_gui_progress_update(15.0, "");

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

/// Clips nodes to bounding box using Sutherland-Hodgman algorithm.
/// For clipped nodes, preserves original node IDs by finding the closest original node.
/// This ensures multipolygon relations can still merge ways correctly via node ID matching.
fn filter_nodes_to_bbox(
    nodes: &[ProcessedNode],
    xzbbox: &XZBBox,
    tags: &HashMap<String, String>,
) -> Vec<ProcessedNode> {
    if nodes.is_empty() {
        return Vec::new();
    }

    // Don't clip/filter ways that are typically part of multipolygon relations
    // These need exact node ID preservation for merge_loopy_loops to work
    if tags.get("natural") == Some(&"coastline".to_string())
        || tags.is_empty()
        || (tags.get("natural").is_some()
            && !tags.contains_key("building")
            && !tags.contains_key("highway"))
    {
        return nodes.to_vec();
    }

    // Check if any nodes are outside the bbox
    let min_x = xzbbox.min_x();
    let max_x = xzbbox.max_x();
    let min_z = xzbbox.min_z();
    let max_z = xzbbox.max_z();

    let has_nodes_outside = nodes
        .iter()
        .any(|node| node.x < min_x || node.x > max_x || node.z < min_z || node.z > max_z);

    // If all nodes are inside the bbox, return original to preserve IDs and structure
    if !has_nodes_outside {
        return nodes.to_vec();
    }

    // Determine if this is a polyline (highway, railway, waterway, barrier, service)
    // or a polygon (building, area, etc.)
    let is_polyline = tags.contains_key("highway")
        || tags.contains_key("railway")
        || tags.contains_key("waterway")
        || tags.contains_key("barrier")
        || tags.get("service").is_some();

    if is_polyline {
        // For polylines, simply filter out nodes outside bbox
        let filtered: Vec<ProcessedNode> = nodes
            .iter()
            .filter(|node| {
                node.x >= min_x && node.x <= max_x && node.z >= min_z && node.z <= max_z
            })
            .cloned()
            .collect();
        return filtered;
    }

    // For polygons, use Sutherland-Hodgman clipping with ID preservation
    clip_polygon_to_bbox(nodes, xzbbox)
}

/// Clips a polygon to a bounding box using the Sutherland-Hodgman algorithm.
/// Preserves original node IDs by finding the closest original node for clipped points.
fn clip_polygon_to_bbox(nodes: &[ProcessedNode], xzbbox: &XZBBox) -> Vec<ProcessedNode> {
    if nodes.is_empty() {
        return Vec::new();
    }

    let min_x = xzbbox.min_x();
    let max_x = xzbbox.max_x();
    let min_z = xzbbox.min_z();
    let max_z = xzbbox.max_z();

    // Sutherland-Hodgman algorithm clips against each edge sequentially
    let mut output = nodes.to_vec();

    // Clip against left edge (x = min_x)
    output = clip_against_edge(&output, min_x, max_x, min_z, max_z, EdgeType::Left, nodes);
    if output.is_empty() {
        return Vec::new();
    }

    // Clip against right edge (x = max_x)
    output = clip_against_edge(&output, min_x, max_x, min_z, max_z, EdgeType::Right, nodes);
    if output.is_empty() {
        return Vec::new();
    }

    // Clip against bottom edge (z = min_z)
    output = clip_against_edge(&output, min_x, max_x, min_z, max_z, EdgeType::Bottom, nodes);
    if output.is_empty() {
        return Vec::new();
    }

    // Clip against top edge (z = max_z)
    output = clip_against_edge(&output, min_x, max_x, min_z, max_z, EdgeType::Top, nodes);

    output
}

#[derive(Clone, Copy)]
enum EdgeType {
    Left,
    Right,
    Bottom,
    Top,
}

/// Helper function to clip polygon against a single edge
fn clip_against_edge(
    polygon: &[ProcessedNode],
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    edge: EdgeType,
    original_nodes: &[ProcessedNode],
) -> Vec<ProcessedNode> {
    if polygon.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();

    for i in 0..polygon.len() {
        let current = &polygon[i];
        let next = &polygon[(i + 1) % polygon.len()];

        let current_inside = is_inside(current, min_x, max_x, min_z, max_z, edge);
        let next_inside = is_inside(next, min_x, max_x, min_z, max_z, edge);

        if current_inside && next_inside {
            // Both inside: add next vertex
            result.push(next.clone());
        } else if current_inside && !next_inside {
            // Leaving: add intersection point
            let intersection = compute_intersection(current, next, min_x, max_x, min_z, max_z, edge);
            let intersection_with_id = assign_closest_id(&intersection, original_nodes);
            result.push(intersection_with_id);
        } else if !current_inside && next_inside {
            // Entering: add intersection point and next vertex
            let intersection = compute_intersection(current, next, min_x, max_x, min_z, max_z, edge);
            let intersection_with_id = assign_closest_id(&intersection, original_nodes);
            result.push(intersection_with_id);
            result.push(next.clone());
        }
        // Both outside: add nothing
    }

    result
}

/// Check if a point is inside relative to the given edge
fn is_inside(node: &ProcessedNode, min_x: i32, max_x: i32, min_z: i32, max_z: i32, edge: EdgeType) -> bool {
    match edge {
        EdgeType::Left => node.x >= min_x,
        EdgeType::Right => node.x <= max_x,
        EdgeType::Bottom => node.z >= min_z,
        EdgeType::Top => node.z <= max_z,
    }
}

/// Compute intersection point between line segment and edge
fn compute_intersection(
    p1: &ProcessedNode,
    p2: &ProcessedNode,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    edge: EdgeType,
) -> ProcessedNode {
    let x1 = p1.x as f64;
    let z1 = p1.z as f64;
    let x2 = p2.x as f64;
    let z2 = p2.z as f64;

    let (x, z) = match edge {
        EdgeType::Left => {
            let edge_x = min_x as f64;
            if (x2 - x1).abs() < 1e-10 {
                (edge_x, z1)
            } else {
                let t = (edge_x - x1) / (x2 - x1);
                let z = z1 + t * (z2 - z1);
                (edge_x, z)
            }
        }
        EdgeType::Right => {
            let edge_x = max_x as f64;
            if (x2 - x1).abs() < 1e-10 {
                (edge_x, z1)
            } else {
                let t = (edge_x - x1) / (x2 - x1);
                let z = z1 + t * (z2 - z1);
                (edge_x, z)
            }
        }
        EdgeType::Bottom => {
            let edge_z = min_z as f64;
            if (z2 - z1).abs() < 1e-10 {
                (x1, edge_z)
            } else {
                let t = (edge_z - z1) / (z2 - z1);
                let x = x1 + t * (x2 - x1);
                (x, edge_z)
            }
        }
        EdgeType::Top => {
            let edge_z = max_z as f64;
            if (z2 - z1).abs() < 1e-10 {
                (x1, edge_z)
            } else {
                let t = (edge_z - z1) / (z2 - z1);
                let x = x1 + t * (x2 - x1);
                (x, edge_z)
            }
        }
    };

    ProcessedNode {
        id: 0, // Temporary ID, will be replaced by assign_closest_id
        tags: HashMap::new(),
        x: x.round() as i32,
        z: z.round() as i32,
    }
}

/// Assigns the ID of the closest original node to a clipped intersection point.
/// This preserves node IDs for multipolygon merging.
fn assign_closest_id(node: &ProcessedNode, original_nodes: &[ProcessedNode]) -> ProcessedNode {
    if original_nodes.is_empty() {
        return node.clone();
    }

    // Find the closest original node
    let mut min_distance = i64::MAX;
    let mut closest_id = original_nodes[0].id;

    for original in original_nodes {
        let dx = (node.x - original.x) as i64;
        let dz = (node.z - original.z) as i64;
        let distance = dx * dx + dz * dz;

        if distance < min_distance {
            min_distance = distance;
            closest_id = original.id;
        }
    }

    ProcessedNode {
        id: closest_id,
        tags: node.tags.clone(),
        x: node.x,
        z: node.z,
    }
}
