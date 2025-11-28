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

            // Only add tagged nodes to processed_elements if they're within or near the bbox
            // This significantly improves performance by filtering out distant nodes
            if !element.tags.as_ref().map(|t| t.is_empty()).unwrap_or(true) {
                // Node has tags, check if it's in the bbox (with some margin)
                if xzbbox.contains(&xzpoint) {
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

        // Clip the way to bbox to reduce node count dramatically
        let tags = element.tags.clone().unwrap_or_default();

        // Store unclipped way for relation assembly (clipping happens after ring merging)
        ways_map.insert(
            element.id,
            ProcessedWay {
                id: element.id,
                tags: tags.clone(),
                nodes: nodes.clone(),
            },
        );

        // Clip way nodes for standalone way processing (not relations)
        let clipped_nodes = clip_way_to_bbox(&nodes, &xzbbox);

        // Skip ways that are completely outside the bbox (empty after clipping)
        if clipped_nodes.is_empty() {
            continue;
        }

        let processed: ProcessedWay = ProcessedWay {
            id: element.id,
            tags: tags.clone(),
            nodes: clipped_nodes.clone(),
        };

        processed_elements.push(ProcessedElement::Way(processed));
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

        // Water relations require unclipped ways for ring merging in water_areas.rs
        let is_water_relation = is_water_element(tags);

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

                // Water relations: keep unclipped for ring merging
                // Non-water relations: clip member ways now
                let final_way = if is_water_relation {
                    way
                } else {
                    let clipped_nodes = clip_way_to_bbox(&way.nodes, &xzbbox);
                    if clipped_nodes.is_empty() {
                        return None;
                    }
                    ProcessedWay {
                        id: way.id,
                        tags: way.tags,
                        nodes: clipped_nodes,
                    }
                };

                Some(ProcessedMember { role, way: final_way })
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

/// Returns true if tags indicate a water element handled by water_areas.rs.
fn is_water_element(tags: &HashMap<String, String>) -> bool {
    // Check for explicit water tag
    if tags.contains_key("water") {
        return true;
    }
    
    // Check for natural=water or natural=bay
    if let Some(natural_val) = tags.get("natural") {
        if natural_val == "water" || natural_val == "bay" {
            return true;
        }
    }
    
    // Check for waterway=dock (also handled as water area)
    if let Some(waterway_val) = tags.get("waterway") {
        if waterway_val == "dock" {
            return true;
        }
    }
    
    false
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

/// Check if a clipped coordinate matches an original endpoint (within tolerance)
fn matches_endpoint(coord: (f64, f64), endpoint: &ProcessedNode, tolerance: f64) -> bool {
    let dx = (coord.0 - endpoint.x as f64).abs();
    let dz = (coord.1 - endpoint.z as f64).abs();
    let distance_sq = dx * dx + dz * dz;
    distance_sq < tolerance * tolerance
}

/// Assigns node IDs to clipped coordinates, preserving original endpoint IDs where possible.
/// Endpoint ID preservation is required for `merge_loopy_loops` to correctly connect
/// way segments in multipolygon relations. Middle nodes receive synthetic IDs.
fn assign_node_ids_preserving_endpoints(
    original_nodes: &[ProcessedNode],
    clipped_coords: Vec<(f64, f64)>,
    way_id: u64,
) -> Vec<ProcessedNode> {
    if clipped_coords.is_empty() {
        return Vec::new();
    }

    let original_first = original_nodes.first();
    let original_last = original_nodes.last();

    // Large tolerance to account for endpoint displacement from bbox clipping
    let tolerance = 50.0;
    let last_index = clipped_coords.len() - 1;

    clipped_coords
        .into_iter()
        .enumerate()
        .map(|(i, coord)| {
            let is_first = i == 0;
            let is_last = i == last_index;

            // Try to preserve endpoint IDs (but use CLIPPED coordinates to stay in bbox)
            if is_first || is_last {
                // Check if this matches original first endpoint
                if let Some(first) = original_first {
                    if matches_endpoint(coord, first, tolerance) {
                        return ProcessedNode {
                            id: first.id,              // Preserve ID for merge_loopy_loops matching
                            x: coord.0.round() as i32, // Use CLIPPED coord (stays in bbox)
                            z: coord.1.round() as i32, // Use CLIPPED coord (stays in bbox)
                            tags: HashMap::new(),
                        };
                    }
                }
                // Check if this matches original last endpoint
                if let Some(last) = original_last {
                    if matches_endpoint(coord, last, tolerance) {
                        return ProcessedNode {
                            id: last.id,               // Preserve ID for merge_loopy_loops matching
                            x: coord.0.round() as i32, // Use CLIPPED coord (stays in bbox)
                            z: coord.1.round() as i32, // Use CLIPPED coord (stays in bbox)
                            tags: HashMap::new(),
                        };
                    }
                }
                // Endpoint doesn't match original - use synthetic ID and clipped coords
                return ProcessedNode {
                    id: way_id.wrapping_mul(10000000).wrapping_add(i as u64),
                    x: coord.0.round() as i32,
                    z: coord.1.round() as i32,
                    tags: HashMap::new(),
                };
            }

            // Middle node - always use unique synthetic ID and clipped coords
            ProcessedNode {
                id: way_id.wrapping_mul(10000000).wrapping_add(i as u64),
                x: coord.0.round() as i32,
                z: coord.1.round() as i32,
                tags: HashMap::new(),
            }
        })
        .collect()
}

/// Removes consecutive duplicate points from a polygon (within epsilon tolerance).
fn remove_consecutive_duplicates(polygon: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    if polygon.is_empty() {
        return polygon;
    }
    
    let eps = 0.1; // Tolerance for considering points as duplicates
    let mut result: Vec<(f64, f64)> = Vec::with_capacity(polygon.len());
    
    for p in &polygon {
        if let Some(last) = result.last() {
            // Skip if this point is essentially the same as the previous one
            if (p.0 - last.0).abs() < eps && (p.1 - last.1).abs() < eps {
                continue;
            }
        }
        result.push(*p);
    }
    
    // Also check if first and last are duplicates (for closed polygons)
    if result.len() > 1 {
        let first = result.first().unwrap();
        let last = result.last().unwrap();
        if (first.0 - last.0).abs() < eps && (first.1 - last.1).abs() < eps {
            result.pop();
        }
    }
    
    result
}

/// Returns which bbox edge a point lies on: 0=bottom, 1=right, 2=top, 3=left, -1=interior.
fn get_bbox_edge(point: (f64, f64), min_x: f64, min_z: f64, max_x: f64, max_z: f64) -> i32 {
    let eps = 0.5; // Tolerance for floating point comparison
    
    let on_left = (point.0 - min_x).abs() < eps;
    let on_right = (point.0 - max_x).abs() < eps;
    let on_bottom = (point.1 - min_z).abs() < eps;
    let on_top = (point.1 - max_z).abs() < eps;
    
    // Handle corners - assign to the edge we're "leaving" in counter-clockwise order
    if on_bottom && on_left {
        return 3; // Bottom-left corner, assign to left edge
    }
    if on_bottom && on_right {
        return 0; // Bottom-right corner, assign to bottom edge
    }
    if on_top && on_right {
        return 1; // Top-right corner, assign to right edge
    }
    if on_top && on_left {
        return 2; // Top-left corner, assign to top edge
    }
    
    // Single edge
    if on_bottom { return 0; }
    if on_right { return 1; }
    if on_top { return 2; }
    if on_left { return 3; }
    
    -1 // Not on any edge (interior point)
}

/// Returns corners to insert when traversing from edge1 to edge2 via the shorter path.
/// Returns empty for opposite edges (polygon passes through bbox).
fn get_corners_between_edges(
    edge1: i32, 
    edge2: i32, 
    min_x: f64, 
    min_z: f64, 
    max_x: f64, 
    max_z: f64
) -> Vec<(f64, f64)> {
    if edge1 == edge2 || edge1 < 0 || edge2 < 0 {
        return Vec::new();
    }
    
    // Corners indexed by edge: corner[i] is the corner between edge i and edge (i+1)%4
    // Edges: 0=bottom, 1=right, 2=top, 3=left
    // Corners: 0=bottom-right, 1=top-right, 2=top-left, 3=bottom-left
    let corners = [
        (max_x, min_z), // 0: bottom-right (between bottom and right)
        (max_x, max_z), // 1: top-right (between right and top)
        (min_x, max_z), // 2: top-left (between top and left)
        (min_x, min_z), // 3: bottom-left (between left and bottom)
    ];
    
    // Calculate distance in both directions
    let ccw_dist = ((edge2 - edge1 + 4) % 4) as usize; // Counter-clockwise distance
    let cw_dist = ((edge1 - edge2 + 4) % 4) as usize;  // Clockwise distance
    
    // If edges are opposite (distance 2 in both directions), don't insert corners.
    // The polygon is passing through the bbox, not wrapping around it.
    if ccw_dist == 2 && cw_dist == 2 {
        return Vec::new();
    }
    
    let mut result = Vec::new();
    
    if ccw_dist <= cw_dist {
        // Go counter-clockwise (shorter or equal)
        let mut current = edge1;
        for _ in 0..ccw_dist {
            result.push(corners[current as usize]);
            current = (current + 1) % 4;
        }
    } else {
        // Go clockwise (shorter)
        let mut current = edge1;
        for _ in 0..cw_dist {
            current = (current + 4 - 1) % 4; // Move clockwise (decrement with wrap)
            result.push(corners[current as usize]);
        }
    }
    
    result
}

/// Inserts bbox corners where polygon transitions between different bbox edges.
fn insert_bbox_corners(
    polygon: Vec<(f64, f64)>, 
    min_x: f64, 
    min_z: f64, 
    max_x: f64, 
    max_z: f64
) -> Vec<(f64, f64)> {
    if polygon.len() < 3 {
        return polygon;
    }
    
    let mut result = Vec::with_capacity(polygon.len() + 4); // May add up to 4 corners
    
    for i in 0..polygon.len() {
        let current = polygon[i];
        let next = polygon[(i + 1) % polygon.len()];
        
        result.push(current);
        
        // Check if current and next are on different bbox edges
        let edge1 = get_bbox_edge(current, min_x, min_z, max_x, max_z);
        let edge2 = get_bbox_edge(next, min_x, min_z, max_x, max_z);
        
        if edge1 >= 0 && edge2 >= 0 && edge1 != edge2 {
            // Both points are on bbox edges but different edges
            // We need to insert corner(s) between them
            let corners = get_corners_between_edges(edge1, edge2, min_x, min_z, max_x, max_z);
            for corner in corners {
                result.push(corner);
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
                    // Create synthetic node with unique ID for intersection
                    let synthetic_id = nodes[0]
                        .id
                        .wrapping_mul(10000000)
                        .wrapping_add(result.len() as u64);
                    result.push(ProcessedNode {
                        id: synthetic_id,
                        x: intersection.0.round() as i32,
                        z: intersection.1.round() as i32,
                        tags: HashMap::new(),
                    });
                }
            }
        }
    }

    // Now preserve endpoint IDs if they match original endpoints
    if !result.is_empty() && result.len() >= 2 {
        let tolerance = 50.0; // Large tolerance for bbox edge intersections
        if let Some(first_orig) = nodes.first() {
            if matches_endpoint(
                (result[0].x as f64, result[0].z as f64),
                first_orig,
                tolerance,
            ) {
                result[0].id = first_orig.id;
            }
        }
        if let Some(last_orig) = nodes.last() {
            let last_idx = result.len() - 1;
            if matches_endpoint(
                (result[last_idx].x as f64, result[last_idx].z as f64),
                last_orig,
                tolerance,
            ) {
                result[last_idx].id = last_orig.id;
            }
        }
    }

    result
}

/// Clips a way to the bounding box using Sutherland-Hodgman for polygons or
/// simple line clipping for polylines. Preserves endpoint IDs for ring assembly.
fn clip_way_to_bbox(
    nodes: &[ProcessedNode],
    xzbbox: &XZBBox,
) -> Vec<ProcessedNode> {
    if nodes.is_empty() {
        return Vec::new();
    }

    // Closed polygons use Sutherland-Hodgman; open paths use polyline clipping
    let is_closed_polygon = if nodes.len() >= 3 {
        let first = nodes.first().unwrap();
        let last = nodes.last().unwrap();
        // Check if closed by ID or by coordinates
        first.id == last.id || (first.x == last.x && first.z == last.z)
    } else {
        false
    };

    // Only clip if the way extends outside the bbox
    let has_nodes_outside = nodes
        .iter()
        .any(|node| !xzbbox.contains(&XZPoint::new(node.x, node.z)));

    if !is_closed_polygon {
        return clip_polyline_to_bbox(nodes, xzbbox);
    }

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

    // Sutherland-Hodgman: clip against each bbox edge (counter-clockwise traversal)
    let bbox_edges = [
        (min_x, min_z, max_x, min_z, 0), // Bottom edge: horizontal, clamp z only
        (max_x, min_z, max_x, max_z, 1), // Right edge: vertical, clamp x only
        (max_x, max_z, min_x, max_z, 2), // Top edge: horizontal, clamp z only
        (min_x, max_z, min_x, min_z, 3), // Left edge: vertical, clamp x only
    ];

    for (edge_x1, edge_z1, edge_x2, edge_z2, edge_idx) in bbox_edges {
        if polygon.is_empty() {
            break;
        }

        let mut clipped_polygon: Vec<(f64, f64)> = Vec::new();

        // Recompute whether polygon is explicitly closed for EACH pass
        // (the clipping can change this property)
        let is_closed = !polygon.is_empty() && polygon.first() == polygon.last();
        
        // If explicitly closed, process n-1 edges; else process n edges with wrap
        let edge_count = if is_closed {
            polygon.len().saturating_sub(1)
        } else {
            polygon.len()
        };

        for i in 0..edge_count {
            let current = polygon[i];
            let next = if i + 1 < polygon.len() {
                polygon[i + 1]
            } else {
                polygon[0]
            };

            let current_inside = point_inside_edge(current, edge_x1, edge_z1, edge_x2, edge_z2);
            let next_inside = point_inside_edge(next, edge_x1, edge_z1, edge_x2, edge_z2);

            if next_inside {
                if !current_inside {
                    // Entering: add intersection point
                    if let Some(mut intersection) = line_edge_intersection(
                        current.0, current.1, next.0, next.1, edge_x1, edge_z1, edge_x2, edge_z2,
                    ) {
                        // Clamp to current edge only
                        match edge_idx {
                            0 => intersection.1 = min_z,
                            1 => intersection.0 = max_x,
                            2 => intersection.1 = max_z,
                            3 => intersection.0 = min_x,
                            _ => {}
                        }
                        clipped_polygon.push(intersection);
                    }
                }
                // Add the next point since it's inside
                clipped_polygon.push(next);
            } else if current_inside {
                // Exiting: add intersection point
                if let Some(mut intersection) = line_edge_intersection(
                    current.0, current.1, next.0, next.1, edge_x1, edge_z1, edge_x2, edge_z2,
                ) {
                    // Clamp to current edge only
                    match edge_idx {
                        0 => intersection.1 = min_z,
                        1 => intersection.0 = max_x,
                        2 => intersection.1 = max_z,
                        3 => intersection.0 = min_x,
                        _ => {}
                    }
                    clipped_polygon.push(intersection);
                }
            }
            // If both outside, don't add anything
        }

        polygon = clipped_polygon;
    }

    // Validate and clamp the resulting polygon
    if polygon.len() < 3 {
        return Vec::new();
    }

    // Final clamping for floating-point errors
    for p in &mut polygon {
        p.0 = p.0.clamp(min_x, max_x);
        p.1 = p.1.clamp(min_z, max_z);
    }
    
    // Remove duplicates from corner clipping
    let polygon = remove_consecutive_duplicates(polygon);
    
    if polygon.len() < 3 {
        return Vec::new();
    }

    // Insert corners where polygon follows bbox edges
    let polygon = insert_bbox_corners(polygon, min_x, min_z, max_x, max_z);
    
    // Remove duplicates from corner insertion
    let polygon = remove_consecutive_duplicates(polygon);

    if polygon.len() < 3 {
        return Vec::new();
    }

    // Convert back to ProcessedNode format (preserve endpoint IDs)
    let way_id = nodes.first().map(|n| n.id).unwrap_or(0);
    assign_node_ids_preserving_endpoints(nodes, polygon, way_id)
}
