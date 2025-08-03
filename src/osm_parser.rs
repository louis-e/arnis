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
    println!("Bounding box: {:?}", bbox);
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

            // Process nodes with tags
            if let Some(tags) = &element.tags {
                if !tags.is_empty() {
                    processed_elements.push(ProcessedElement::Node(processed));
                }
            }
        }
    }

    // Second pass: process ways and clip them to bbox
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
            // Clip the way to the bounding box
            let tags = element.tags.clone().unwrap_or_default();
            let clipped_nodes = clip_way_to_bbox(&nodes, &xzbbox, &tags);

            if !clipped_nodes.is_empty() {
                let processed: ProcessedWay = ProcessedWay {
                    id: element.id,
                    tags: element.tags.clone().unwrap_or_default(),
                    nodes: clipped_nodes,
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

/// Clips a way to the bounding box boundaries using Sutherland-Hodgman algorithm for polygons
/// or simple line clipping for polylines
fn clip_way_to_bbox(
    nodes: &[ProcessedNode],
    xzbbox: &XZBBox,
    tags: &HashMap<String, String>,
) -> Vec<ProcessedNode> {
    if nodes.is_empty() {
        return Vec::new();
    }

    // For certain tags, use simple line clipping instead of polygon clipping
    if ["waterway", "highway", "barrier", "railway", "service"]
        .iter()
        .any(|key| tags.contains_key(*key))
    {
        return clip_polyline_to_bbox(nodes, xzbbox);
    }

    // For now, let's be conservative and only clip if the way actually extends outside the bbox
    // Check if any nodes are outside the bbox
    let has_nodes_outside = nodes
        .iter()
        .any(|node| !xzbbox.contains(&XZPoint::new(node.x, node.z)));

    // If all nodes are inside the bbox, return the original nodes unchanged
    if !has_nodes_outside {
        return nodes.to_vec();
    }

    let min_x = xzbbox.min_x() as f64;
    let min_z = xzbbox.min_z() as f64;
    let max_x = xzbbox.max_x() as f64;
    let max_z = xzbbox.max_z() as f64;

    // Convert nodes to a simple coordinate list for easier processing
    let mut polygon: Vec<(f64, f64)> = nodes.iter().map(|n| (n.x as f64, n.z as f64)).collect();

    // Only close polygon if it's already nearly closed (last point close to first)
    let should_close = if polygon.len() > 2 {
        let first = polygon[0];
        let last = polygon[polygon.len() - 1];
        let distance = ((first.0 - last.0).powi(2) + (first.1 - last.1).powi(2)).sqrt();
        distance < 10.0 // Close if within 10 units
    } else {
        false
    };

    if should_close && polygon.first() != polygon.last() {
        polygon.push(polygon[0]);
    }

    // Clip against each edge of the bounding box using Sutherland-Hodgman algorithm
    let bbox_edges = [
        (min_x, min_z, max_x, min_z), // Bottom edge
        (max_x, min_z, max_x, max_z), // Right edge
        (max_x, max_z, min_x, max_z), // Top edge
        (min_x, max_z, min_x, min_z), // Left edge
    ];

    for (edge_x1, edge_z1, edge_x2, edge_z2) in bbox_edges {
        let mut clipped_polygon = Vec::new();

        if polygon.is_empty() {
            break;
        }

        for i in 0..polygon.len() {
            let current = polygon[i];
            let next = polygon[(i + 1) % polygon.len()];

            let current_inside = point_inside_edge(current, edge_x1, edge_z1, edge_x2, edge_z2);
            let next_inside = point_inside_edge(next, edge_x1, edge_z1, edge_x2, edge_z2);

            if next_inside {
                if !current_inside {
                    // Entering: add intersection point
                    if let Some(intersection) = line_edge_intersection(
                        current.0, current.1, next.0, next.1, edge_x1, edge_z1, edge_x2, edge_z2,
                    ) {
                        clipped_polygon.push(intersection);
                    }
                }
                // Add the next point since it's inside
                clipped_polygon.push(next);
            } else if current_inside {
                // Exiting: add intersection point
                if let Some(intersection) = line_edge_intersection(
                    current.0, current.1, next.0, next.1, edge_x1, edge_z1, edge_x2, edge_z2,
                ) {
                    clipped_polygon.push(intersection);
                }
            }
            // If both outside, don't add anything
        }

        polygon = clipped_polygon;
    }

    // Convert back to ProcessedNode format
    let mut result: Vec<ProcessedNode> = polygon
        .into_iter()
        .enumerate()
        .map(|(i, (x, z))| ProcessedNode {
            id: i as u64, // Use index as synthetic ID
            x: x.round() as i32,
            z: z.round() as i32,
            tags: HashMap::new(),
        })
        .collect();

    // For closed polygons, ensure the first and last nodes have the same ID to maintain closure
    // Check if the original was nearly closed and if so, ensure the clipped result is also closed
    if nodes.len() > 2 {
        let original_first = &nodes[0];
        let original_last = nodes.last().unwrap();
        let was_closed = original_first.id == original_last.id;

        if was_closed && !result.is_empty() && result.len() > 2 {
            // Make sure the clipped polygon is also closed by giving the last node the same ID as the first
            let first_id = result[0].id;
            if let Some(last_node) = result.last_mut() {
                last_node.id = first_id;
            }
        }
    }

    result
}

/// Check if a point is on the "inside" side of an edge (using cross product)
fn point_inside_edge(
    point: (f64, f64),
    edge_x1: f64,
    edge_z1: f64,
    edge_x2: f64,
    edge_z2: f64,
) -> bool {
    // Calculate cross product to determine which side of the edge the point is on
    let edge_dx = edge_x2 - edge_x1;
    let edge_dz = edge_z2 - edge_z1;
    let point_dx = point.0 - edge_x1;
    let point_dz = point.1 - edge_z1;

    // Cross product: positive means point is on the "left" side (inside for clockwise bbox)
    let cross_product = edge_dx * point_dz - edge_dz * point_dx;
    cross_product >= 0.0
}

/// Find intersection between a line segment and an edge
#[allow(clippy::too_many_arguments)]
fn line_edge_intersection(
    line_x1: f64,
    line_z1: f64,
    line_x2: f64,
    line_z2: f64,
    edge_x1: f64,
    edge_z1: f64,
    edge_x2: f64,
    edge_z2: f64,
) -> Option<(f64, f64)> {
    let line_dx = line_x2 - line_x1;
    let line_dz = line_z2 - line_z1;
    let edge_dx = edge_x2 - edge_x1;
    let edge_dz = edge_z2 - edge_z1;

    let denom = line_dx * edge_dz - line_dz * edge_dx;

    if denom.abs() < 1e-10 {
        return None; // Lines are parallel
    }

    let dx = edge_x1 - line_x1;
    let dz = edge_z1 - line_z1;

    let t = (dx * edge_dz - dz * edge_dx) / denom;

    // Only return intersection if it's on the line segment
    if (0.0..=1.0).contains(&t) {
        let x = line_x1 + t * line_dx;
        let z = line_z1 + t * line_dz;
        Some((x, z))
    } else {
        None
    }
}

/// Clips a polyline (open line) to the bounding box boundaries
/// This prevents artificial connections that can occur with polygon clipping algorithms
fn clip_polyline_to_bbox(nodes: &[ProcessedNode], xzbbox: &XZBBox) -> Vec<ProcessedNode> {
    if nodes.is_empty() {
        return Vec::new();
    }

    let min_x = xzbbox.min_x() as f64;
    let min_z = xzbbox.min_z() as f64;
    let max_x = xzbbox.max_x() as f64;
    let max_z = xzbbox.max_z() as f64;

    let mut result = Vec::new();

    for i in 0..nodes.len() {
        let current = &nodes[i];
        let current_point = (current.x as f64, current.z as f64);

        // Check if current point is inside bbox
        let current_inside = current_point.0 >= min_x
            && current_point.0 <= max_x
            && current_point.1 >= min_z
            && current_point.1 <= max_z;

        if current_inside {
            result.push(current.clone());
        }

        // If there's a next point, check for intersections with bbox edges
        if i + 1 < nodes.len() {
            let next = &nodes[i + 1];
            let next_point = (next.x as f64, next.z as f64);
            let next_inside = next_point.0 >= min_x
                && next_point.0 <= max_x
                && next_point.1 >= min_z
                && next_point.1 <= max_z;

            // If line segment crosses bbox boundary, add intersection points
            if current_inside != next_inside {
                let intersections =
                    find_bbox_intersections(current_point, next_point, min_x, min_z, max_x, max_z);

                for intersection in intersections {
                    result.push(ProcessedNode {
                        id: 0, // Synthetic ID for intersection points
                        x: intersection.0.round() as i32,
                        z: intersection.1.round() as i32,
                        tags: HashMap::new(),
                    });
                }
            }
        }
    }

    result
}

/// Find intersections between a line segment and bounding box edges
fn find_bbox_intersections(
    start: (f64, f64),
    end: (f64, f64),
    min_x: f64,
    min_z: f64,
    max_x: f64,
    max_z: f64,
) -> Vec<(f64, f64)> {
    let mut intersections = Vec::new();

    // Check intersection with each bbox edge
    let bbox_edges = [
        (min_x, min_z, max_x, min_z), // Bottom edge
        (max_x, min_z, max_x, max_z), // Right edge
        (max_x, max_z, min_x, max_z), // Top edge
        (min_x, max_z, min_x, min_z), // Left edge
    ];

    for (edge_x1, edge_z1, edge_x2, edge_z2) in bbox_edges {
        if let Some(intersection) = line_edge_intersection(
            start.0, start.1, end.0, end.1, edge_x1, edge_z1, edge_x2, edge_z2,
        ) {
            // Check if intersection is actually on the bbox edge
            let on_edge = (intersection.0 >= min_x
                && intersection.0 <= max_x
                && intersection.1 >= min_z
                && intersection.1 <= max_z)
                && ((intersection.0 == min_x || intersection.0 == max_x)
                    || (intersection.1 == min_z || intersection.1 == max_z));

            if on_edge {
                intersections.push(intersection);
            }
        }
    }

    intersections
}
