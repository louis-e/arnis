use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZPoint;
use crate::floodfill::flood_fill_area;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::HashMap;

/// Generates highways with elevation support based on layer tags and connectivity analysis
pub fn generate_highways(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    all_elements: &[ProcessedElement],
) {
    let highway_connectivity = build_highway_connectivity_map(all_elements);
    generate_highways_internal(editor, element, args, &highway_connectivity);
}

/// Build a connectivity map for highway endpoints to determine where slopes are needed
fn build_highway_connectivity_map(elements: &[ProcessedElement]) -> HashMap<(i32, i32), Vec<i32>> {
    let mut connectivity_map: HashMap<(i32, i32), Vec<i32>> = HashMap::new();

    for element in elements {
        if let ProcessedElement::Way(way) = element {
            if way.tags.contains_key("highway") {
                let layer_value = way
                    .tags
                    .get("layer")
                    .and_then(|layer| layer.parse::<i32>().ok())
                    .unwrap_or(0);

                // Treat negative layers as ground level (0) for connectivity
                let layer_value = if layer_value < 0 { 0 } else { layer_value };

                // Add connectivity for start and end nodes
                if !way.nodes.is_empty() {
                    let start_node = &way.nodes[0];
                    let end_node = &way.nodes[way.nodes.len() - 1];

                    let start_coord = (start_node.x, start_node.z);
                    let end_coord = (end_node.x, end_node.z);

                    connectivity_map
                        .entry(start_coord)
                        .or_default()
                        .push(layer_value);
                    connectivity_map
                        .entry(end_coord)
                        .or_default()
                        .push(layer_value);
                }
            }
        }
    }

    connectivity_map
}

/// Internal function that generates highways with connectivity context for elevation handling
fn generate_highways_internal(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    highway_connectivity: &HashMap<(i32, i32), Vec<i32>>, // Maps node coordinates to list of layers that connect to this node
) {
    if let Some(highway_type) = element.tags().get("highway") {
        if highway_type == "street_lamp" {
            // Handle street lamps
            if let ProcessedElement::Node(first_node) = element {
                let x: i32 = first_node.x;
                let z: i32 = first_node.z;
                editor.set_block(COBBLESTONE_WALL, x, 1, z, None, None);
                for dy in 2..=4 {
                    editor.set_block(OAK_FENCE, x, dy, z, None, None);
                }
                editor.set_block(GLOWSTONE, x, 5, z, None, None);
            }
        } else if highway_type == "crossing" {
            // Handle traffic signals for crossings
            if let Some(crossing_type) = element.tags().get("crossing") {
                if crossing_type == "traffic_signals" {
                    if let ProcessedElement::Node(node) = element {
                        let x: i32 = node.x;
                        let z: i32 = node.z;

                        for dy in 1..=3 {
                            editor.set_block(COBBLESTONE_WALL, x, dy, z, None, None);
                        }

                        editor.set_block(GREEN_WOOL, x, 4, z, None, None);
                        editor.set_block(YELLOW_WOOL, x, 5, z, None, None);
                        editor.set_block(RED_WOOL, x, 6, z, None, None);
                    }
                }
            }
        } else if highway_type == "bus_stop" {
            // Handle bus stops
            if let ProcessedElement::Node(node) = element {
                let x = node.x;
                let z = node.z;
                for dy in 1..=3 {
                    editor.set_block(COBBLESTONE_WALL, x, dy, z, None, None);
                }

                editor.set_block(WHITE_WOOL, x, 4, z, None, None);
                editor.set_block(WHITE_WOOL, x + 1, 4, z, None, None);
            }
        } else if element
            .tags()
            .get("area")
            .is_some_and(|v: &String| v == "yes")
        {
            let ProcessedElement::Way(way) = element else {
                return;
            };

            // Handle areas like pedestrian plazas
            let mut surface_block: Block = STONE; // Default block

            // Determine the block type based on the 'surface' tag
            if let Some(surface) = element.tags().get("surface") {
                surface_block = match surface.as_str() {
                    "paving_stones" | "sett" => STONE_BRICKS,
                    "bricks" => BRICK,
                    "wood" => OAK_PLANKS,
                    "asphalt" => BLACK_CONCRETE,
                    "gravel" | "fine_gravel" => GRAVEL,
                    "grass" => GRASS_BLOCK,
                    "dirt" | "ground" | "earth" => DIRT,
                    "sand" => SAND,
                    "concrete" => LIGHT_GRAY_CONCRETE,
                    _ => STONE, // Default to stone for unknown surfaces
                };
            }

            // Fill the area using flood fill or by iterating through the nodes
            let polygon_coords: Vec<(i32, i32)> = way
                .nodes
                .iter()
                .map(|n: &crate::osm_parser::ProcessedNode| (n.x, n.z))
                .collect();
            let filled_area: Vec<(i32, i32)> =
                flood_fill_area(&polygon_coords, args.timeout.as_ref());

            for (x, z) in filled_area {
                editor.set_block(surface_block, x, 0, z, None, None);
            }
        } else {
            let mut previous_node: Option<(i32, i32)> = None;
            let mut block_type = BLACK_CONCRETE;
            let mut block_range: i32 = 2;
            let mut add_stripe = false;
            let mut add_outline = false;
            let scale_factor = args.scale;

            // Parse the layer value for elevation calculation
            let layer_value = element
                .tags()
                .get("layer")
                .and_then(|layer| layer.parse::<i32>().ok())
                .unwrap_or(0);

            // Treat negative layers as ground level (0)
            let layer_value = if layer_value < 0 { 0 } else { layer_value };

            // Skip if 'level' is negative in the tags (indoor mapping)
            if let Some(level) = element.tags().get("level") {
                if level.parse::<i32>().unwrap_or(0) < 0 {
                    return;
                }
            }

            // Determine block type and range based on highway type
            match highway_type.as_str() {
                "footway" | "pedestrian" => {
                    block_type = GRAY_CONCRETE;
                    block_range = 1;
                }
                "path" => {
                    block_type = DIRT_PATH;
                    block_range = 1;
                }
                "motorway" | "primary" | "trunk" => {
                    block_range = 5;
                    add_stripe = true;
                }
                "secondary" => {
                    block_range = 4;
                    add_stripe = true;
                }
                "tertiary" => {
                    add_stripe = true;
                }
                "track" => {
                    block_range = 1;
                }
                "service" => {
                    block_type = GRAY_CONCRETE;
                    block_range = 2;
                }
                "secondary_link" | "tertiary_link" => {
                    //Exit ramps, sliproads
                    block_type = BLACK_CONCRETE;
                    block_range = 1;
                }
                "escape" => {
                    // Sand trap for vehicles on mountainous roads
                    block_type = SAND;
                    block_range = 1;
                }
                "steps" => {
                    //TODO: Add correct stairs respecting height, step_count, etc.
                    block_type = GRAY_CONCRETE;
                    block_range = 1;
                }

                _ => {
                    if let Some(lanes) = element.tags().get("lanes") {
                        if lanes == "2" {
                            block_range = 3;
                            add_stripe = true;
                            add_outline = true;
                        } else if lanes != "1" {
                            block_range = 4;
                            add_stripe = true;
                            add_outline = true;
                        }
                    }
                }
            }

            let ProcessedElement::Way(way) = element else {
                return;
            };

            if scale_factor < 1.0 {
                block_range = ((block_range as f64) * scale_factor).floor() as i32;
            }

            // Calculate elevation based on layer
            const LAYER_HEIGHT_STEP: i32 = 6; // Each layer is 6 blocks higher/lower
            let base_elevation = layer_value * LAYER_HEIGHT_STEP;

            // Check if we need slopes at start and end
            let needs_start_slope =
                should_add_slope_at_node(&way.nodes[0], layer_value, highway_connectivity);
            let needs_end_slope = should_add_slope_at_node(
                &way.nodes[way.nodes.len() - 1],
                layer_value,
                highway_connectivity,
            );

            // Calculate total way length for slope distribution
            let total_way_length = calculate_way_length(way);

            // Check if this is a short isolated elevated segment - if so, treat as ground level
            let is_short_isolated_elevated =
                needs_start_slope && needs_end_slope && layer_value > 0 && total_way_length <= 35;

            // Override elevation and slopes for short isolated segments
            let (effective_elevation, effective_start_slope, effective_end_slope) =
                if is_short_isolated_elevated {
                    (0, false, false) // Treat as ground level
                } else {
                    (base_elevation, needs_start_slope, needs_end_slope)
                };

            let slope_length = (total_way_length as f32 * 0.35).clamp(15.0, 50.0) as usize; // 35% of way length, max 50 blocks, min 15 blocks

            // Iterate over nodes to create the highway
            let mut segment_index = 0;
            let total_segments = way.nodes.len() - 1;

            for node in &way.nodes {
                if let Some(prev) = previous_node {
                    let (x1, z1) = prev;
                    let x2: i32 = node.x;
                    let z2: i32 = node.z;

                    // Generate the line of coordinates between the two nodes
                    let bresenham_points: Vec<(i32, i32, i32)> =
                        bresenham_line(x1, 0, z1, x2, 0, z2);

                    // Calculate elevation for this segment
                    let segment_length = bresenham_points.len();

                    // Variables to manage dashed line pattern
                    let mut stripe_length: i32 = 0;
                    let dash_length: i32 = (5.0 * scale_factor).ceil() as i32;
                    let gap_length: i32 = (5.0 * scale_factor).ceil() as i32;

                    for (point_index, (x, _, z)) in bresenham_points.iter().enumerate() {
                        // Calculate Y elevation for this point based on slopes and layer
                        let current_y = calculate_point_elevation(
                            segment_index,
                            point_index,
                            segment_length,
                            total_segments,
                            effective_elevation,
                            effective_start_slope,
                            effective_end_slope,
                            slope_length,
                        );

                        // Draw the road surface for the entire width
                        for dx in -block_range..=block_range {
                            for dz in -block_range..=block_range {
                                let set_x: i32 = x + dx;
                                let set_z: i32 = z + dz;

                                // Zebra crossing logic
                                if highway_type == "footway"
                                    && element.tags().get("footway")
                                        == Some(&"crossing".to_string())
                                {
                                    let is_horizontal: bool = (x2 - x1).abs() >= (z2 - z1).abs();
                                    if is_horizontal {
                                        if set_x % 2 < 1 {
                                            editor.set_block(
                                                WHITE_CONCRETE,
                                                set_x,
                                                current_y,
                                                set_z,
                                                Some(&[BLACK_CONCRETE]),
                                                None,
                                            );
                                        } else {
                                            editor.set_block(
                                                BLACK_CONCRETE,
                                                set_x,
                                                current_y,
                                                set_z,
                                                None,
                                                None,
                                            );
                                        }
                                    } else if set_z % 2 < 1 {
                                        editor.set_block(
                                            WHITE_CONCRETE,
                                            set_x,
                                            current_y,
                                            set_z,
                                            Some(&[BLACK_CONCRETE]),
                                            None,
                                        );
                                    } else {
                                        editor.set_block(
                                            BLACK_CONCRETE,
                                            set_x,
                                            current_y,
                                            set_z,
                                            None,
                                            None,
                                        );
                                    }
                                } else {
                                    editor.set_block(
                                        block_type,
                                        set_x,
                                        current_y,
                                        set_z,
                                        None,
                                        Some(&[BLACK_CONCRETE, WHITE_CONCRETE]),
                                    );
                                }

                                // Add stone brick foundation underneath elevated highways for thickness
                                if effective_elevation > 0 && current_y > 0 {
                                    // Add 1 layer of stone bricks underneath the highway surface
                                    editor.set_block(
                                        STONE_BRICKS,
                                        set_x,
                                        current_y - 1,
                                        set_z,
                                        None,
                                        None,
                                    );
                                }

                                // Add support pillars for elevated highways
                                if effective_elevation != 0 && current_y > 0 {
                                    add_highway_support_pillar(
                                        editor,
                                        set_x,
                                        current_y,
                                        set_z,
                                        dx,
                                        dz,
                                        block_range,
                                    );
                                }
                            }
                        }

                        // Add light gray concrete outline for multi-lane roads
                        if add_outline {
                            // Left outline
                            for dz in -block_range..=block_range {
                                let outline_x = x - block_range - 1;
                                let outline_z = z + dz;
                                editor.set_block(
                                    LIGHT_GRAY_CONCRETE,
                                    outline_x,
                                    current_y,
                                    outline_z,
                                    None,
                                    None,
                                );
                            }
                            // Right outline
                            for dz in -block_range..=block_range {
                                let outline_x = x + block_range + 1;
                                let outline_z = z + dz;
                                editor.set_block(
                                    LIGHT_GRAY_CONCRETE,
                                    outline_x,
                                    current_y,
                                    outline_z,
                                    None,
                                    None,
                                );
                            }
                        }

                        // Add a dashed white line in the middle for larger roads
                        if add_stripe {
                            if stripe_length < dash_length {
                                let stripe_x: i32 = *x;
                                let stripe_z: i32 = *z;
                                editor.set_block(
                                    WHITE_CONCRETE,
                                    stripe_x,
                                    current_y,
                                    stripe_z,
                                    Some(&[BLACK_CONCRETE]),
                                    None,
                                );
                            }

                            // Increment stripe_length and reset after completing a dash and gap
                            stripe_length += 1;
                            if stripe_length >= dash_length + gap_length {
                                stripe_length = 0;
                            }
                        }
                    }

                    segment_index += 1;
                }
                previous_node = Some((node.x, node.z));
            }
        }
    }
}

/// Helper function to determine if a slope should be added at a specific node
fn should_add_slope_at_node(
    node: &crate::osm_parser::ProcessedNode,
    current_layer: i32,
    highway_connectivity: &HashMap<(i32, i32), Vec<i32>>,
) -> bool {
    let node_coord = (node.x, node.z);

    // If we don't have connectivity information, always add slopes for non-zero layers
    if highway_connectivity.is_empty() {
        return current_layer != 0;
    }

    // Check if there are other highways at different layers connected to this node
    if let Some(connected_layers) = highway_connectivity.get(&node_coord) {
        // Count how many ways are at the same layer as current way
        let same_layer_count = connected_layers
            .iter()
            .filter(|&&layer| layer == current_layer)
            .count();

        // If this is the only way at this layer connecting to this node, we need a slope
        // (unless we're at ground level and connecting to ground level ways)
        if same_layer_count <= 1 {
            return current_layer != 0;
        }

        // If there are multiple ways at the same layer, don't add slope
        false
    } else {
        // No other highways connected, add slope if not at ground level
        current_layer != 0
    }
}

/// Helper function to calculate the total length of a way in blocks
fn calculate_way_length(way: &ProcessedWay) -> usize {
    let mut total_length = 0;
    let mut previous_node: Option<&crate::osm_parser::ProcessedNode> = None;

    for node in &way.nodes {
        if let Some(prev) = previous_node {
            let dx = (node.x - prev.x).abs();
            let dz = (node.z - prev.z).abs();
            total_length += ((dx * dx + dz * dz) as f32).sqrt() as usize;
        }
        previous_node = Some(node);
    }

    total_length
}

/// Calculate the Y elevation for a specific point along the highway
#[allow(clippy::too_many_arguments)]
fn calculate_point_elevation(
    segment_index: usize,
    point_index: usize,
    segment_length: usize,
    total_segments: usize,
    base_elevation: i32,
    needs_start_slope: bool,
    needs_end_slope: bool,
    slope_length: usize,
) -> i32 {
    // If no slopes needed, return base elevation
    if !needs_start_slope && !needs_end_slope {
        return base_elevation;
    }

    // Calculate total distance from start
    let total_distance_from_start = segment_index * segment_length + point_index;
    let total_way_length = total_segments * segment_length;

    // Ensure we have reasonable values
    if total_way_length == 0 || slope_length == 0 {
        return base_elevation;
    }

    // Start slope calculation - gradual rise from ground level
    if needs_start_slope && total_distance_from_start <= slope_length {
        let slope_progress = total_distance_from_start as f32 / slope_length as f32;
        let elevation_offset = (base_elevation as f32 * slope_progress) as i32;
        return elevation_offset;
    }

    // End slope calculation - gradual descent to ground level
    if needs_end_slope
        && total_distance_from_start >= (total_way_length.saturating_sub(slope_length))
    {
        let distance_from_end = total_way_length - total_distance_from_start;
        let slope_progress = distance_from_end as f32 / slope_length as f32;
        let elevation_offset = (base_elevation as f32 * slope_progress) as i32;
        return elevation_offset;
    }

    // Middle section at full elevation
    base_elevation
}

/// Add support pillars for elevated highways
fn add_highway_support_pillar(
    editor: &mut WorldEditor,
    x: i32,
    highway_y: i32,
    z: i32,
    dx: i32,
    dz: i32,
    _block_range: i32, // Keep for future use
) {
    // Only add pillars at specific intervals and positions
    if dx == 0 && dz == 0 && (x + z) % 8 == 0 {
        // Add pillar from ground to highway level
        for y in 1..highway_y {
            editor.set_block(STONE_BRICKS, x, y, z, None, None);
        }

        // Add pillar base
        for base_dx in -1..=1 {
            for base_dz in -1..=1 {
                editor.set_block(STONE_BRICKS, x + base_dx, 0, z + base_dz, None, None);
            }
        }
    }
}

/// Generates a siding using stone brick slabs
pub fn generate_siding(editor: &mut WorldEditor, element: &ProcessedWay) {
    let mut previous_node: Option<XZPoint> = None;
    let siding_block: Block = STONE_BRICK_SLAB;

    for node in &element.nodes {
        let current_node = node.xz();

        // Draw the siding using Bresenham's line algorithm between nodes
        if let Some(prev_node) = previous_node {
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(
                prev_node.x,
                0,
                prev_node.z,
                current_node.x,
                0,
                current_node.z,
            );

            for (bx, _, bz) in bresenham_points {
                if !editor.check_for_block(bx, 0, bz, Some(&[BLACK_CONCRETE, WHITE_CONCRETE])) {
                    editor.set_block(siding_block, bx, 1, bz, None, None);
                }
            }
        }

        previous_node = Some(current_node);
    }
}

/// Generates an aeroway
pub fn generate_aeroway(editor: &mut WorldEditor, way: &ProcessedWay, args: &Args) {
    let mut previous_node: Option<(i32, i32)> = None;
    let surface_block = LIGHT_GRAY_CONCRETE;

    for node in &way.nodes {
        if let Some(prev) = previous_node {
            let (x1, z1) = prev;
            let x2 = node.x;
            let z2 = node.z;
            let points = bresenham_line(x1, 0, z1, x2, 0, z2);
            let way_width: i32 = (12.0 * args.scale).ceil() as i32;

            for (x, _, z) in points {
                for dx in -way_width..=way_width {
                    for dz in -way_width..=way_width {
                        let set_x = x + dx;
                        let set_z = z + dz;
                        editor.set_block(surface_block, set_x, 0, set_z, None, None);
                    }
                }
            }
        }
        previous_node = Some((node.x, node.z));
    }
}
