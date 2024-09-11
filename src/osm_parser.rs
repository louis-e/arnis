use serde_json::Value;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct OSMElement {
    pub r#type: String,
    pub id: u64,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub nodes: Option<Vec<u64>>,
    pub members: Option<Vec<Member>>, // For relations
    pub tags: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
pub struct Member {
    pub r#type: String,
    #[serde(rename = "ref")]
    pub ref_id: u64,
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct OSMData {
    pub elements: Vec<OSMElement>,
}

pub struct ProcessedElement {
    pub id: u64,
    pub r#type: String,
    pub tags: HashMap<String, String>,
    pub nodes: Vec<(i32, i32)>, // Minecraft coordinates (x, z)
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
    let rel_x: f64 = (lon - min_lon) / (max_lon - min_lon);
    let rel_z: f64 = (lat - min_lat) / (max_lat - min_lat);

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
) -> (Vec<ProcessedElement>, f64, f64) {
    println!("Parsing data...");
    
    // Deserialize the JSON data into the OSMData structure
    let data: OSMData = serde_json::from_value(json_data.clone()).expect("Failed to parse OSM data");

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
    let (scale_factor_x, scale_factor_z) = if (bbox_scaled.2 - bbox_scaled.0) > (bbox_scaled.3 - bbox_scaled.1) {
        // Longitude difference is greater than latitude difference
        (
            ((bbox_scaled.2 - bbox_scaled.0) * 10 / 100) as f64, // Scale for width (x) is based on longitude difference
            ((bbox_scaled.3 - bbox_scaled.1) * 14 / 100) as f64, // Scale for length (z) is based on latitude difference
        )
    } else {
        // Latitude difference is greater than or equal to longitude difference
        (
            ((bbox_scaled.3 - bbox_scaled.1) * 10 / 100) as f64, // Scale for width (x) is based on latitude difference
            ((bbox_scaled.2 - bbox_scaled.0) * 14 / 100) as f64, // Scale for length (z) is based on longitude difference
        )
    };

    println!("Scale factor X: {}", scale_factor_x); // Only if debug
    println!("Scale factor Z: {}", scale_factor_z); // Only if debug

    let mut nodes_map: HashMap<u64, (i32, i32)> = HashMap::new();
    let mut processed_elements: Vec<ProcessedElement> = Vec::new();

    // First pass: store all nodes with Minecraft coordinates and process nodes with tags
    for element in &data.elements {
        if element.r#type == "node" {
            if let (Some(lat), Some(lon)) = (element.lat, element.lon) {
                let mc_coords: (i32, i32) = lat_lon_to_minecraft_coords(lat, lon, bbox, scale_factor_x, scale_factor_z);
                nodes_map.insert(element.id, mc_coords);

                // Process nodes with tags
                if let Some(tags) = &element.tags {
                    if !tags.is_empty() {
                        processed_elements.push(ProcessedElement {
                            id: element.id,
                            r#type: element.r#type.clone(),
                            tags: tags.clone(),
                            nodes: vec![mc_coords], // Nodes for nodes is just the single coordinate
                        });
                    }
                }
            }
        }
    }

    // Second pass: process ways and relations
    for element in data.elements {
        match element.r#type.as_str() {
            "way" | "relation" => {
                let mut nodes: Vec<(i32, i32)> = Vec::new();
                if let Some(node_ids) = element.nodes {
                    for &node_id in &node_ids {
                        if let Some(&coords) = nodes_map.get(&node_id) {
                            nodes.push(coords);
                        }
                    }
                }

                if !nodes.is_empty() {
                    processed_elements.push(ProcessedElement {
                        id: element.id,
                        r#type: element.r#type.clone(),
                        tags: element.tags.unwrap_or_default(),
                        nodes,
                    });
                }
            }
            _ => {}
        }
    }

    (processed_elements, scale_factor_z, scale_factor_x)
}

const PRIORITY_ORDER: [&str; 5] = ["entrance", "building", "highway", "waterway", "barrier"];

// Function to determine the priority of each element
pub fn get_priority(element: &ProcessedElement) -> usize {
    // Check each tag against the priority order
    for (i, &tag) in PRIORITY_ORDER.iter().enumerate() {
        if element.tags.contains_key(tag) {
            return i;
        }
    }
    // Return a default priority if none of the tags match
    PRIORITY_ORDER.len()
}
