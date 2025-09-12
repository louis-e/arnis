use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZPoint;
use crate::floodfill::flood_fill_area;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::{format_sign_text, WorldEditor};

pub fn generate_highways(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
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

            // Skip if 'layer' or 'level' is negative in the tags
            if let Some(layer) = element.tags().get("layer") {
                if layer.parse::<i32>().unwrap_or(0) < 0 {
                    return;
                }
            }

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
                "motorway" | "primary" => {
                    block_range = 5;
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

            // Iterate over nodes to create the highway
            for node in &way.nodes {
                if let Some(prev) = previous_node {
                    let (x1, z1) = prev;
                    let x2: i32 = node.x;
                    let z2: i32 = node.z;

                    // Generate the line of coordinates between the two nodes
                    // we don't care about the y because it's going to get overwritten
                    // I'm not sure if we'll keep it this way
                    let bresenham_points: Vec<(i32, i32, i32)> =
                        bresenham_line(x1, 0, z1, x2, 0, z2);

                    // Variables to manage dashed line pattern
                    let mut stripe_length: i32 = 0;
                    let dash_length: i32 = (5.0 * scale_factor).ceil() as i32; // Length of the solid part of the stripe
                    let gap_length: i32 = (5.0 * scale_factor).ceil() as i32; // Length of the gap part of the stripe

                    for (x, _, z) in bresenham_points {
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
                                                0,
                                                set_z,
                                                Some(&[BLACK_CONCRETE]),
                                                None,
                                            );
                                        } else {
                                            editor.set_block(
                                                BLACK_CONCRETE,
                                                set_x,
                                                0,
                                                set_z,
                                                None,
                                                None,
                                            );
                                        }
                                    } else if set_z % 2 < 1 {
                                        editor.set_block(
                                            WHITE_CONCRETE,
                                            set_x,
                                            0,
                                            set_z,
                                            Some(&[BLACK_CONCRETE]),
                                            None,
                                        );
                                    } else {
                                        editor.set_block(
                                            BLACK_CONCRETE,
                                            set_x,
                                            0,
                                            set_z,
                                            None,
                                            None,
                                        );
                                    }
                                } else {
                                    editor.set_block(
                                        block_type,
                                        set_x,
                                        0,
                                        set_z,
                                        None,
                                        Some(&[BLACK_CONCRETE, WHITE_CONCRETE]),
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
                                    0,
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
                                    0,
                                    outline_z,
                                    None,
                                    None,
                                );
                            }
                        }

                        // Add a dashed white line in the middle for larger roads
                        if add_stripe {
                            if stripe_length < dash_length {
                                let stripe_x: i32 = x;
                                let stripe_z: i32 = z;
                                editor.set_block(
                                    WHITE_CONCRETE,
                                    stripe_x,
                                    0,
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
                }
                previous_node = Some((node.x, node.z));
            }

            if let Some(name) = element.tags().get("name") {
                eprintln!("Processing highway '{name}'");
                let mut prev_node: Option<&crate::osm_parser::ProcessedNode> = None;
                let sign_interval = (200.0 * args.scale).max(1.0);
                eprintln!("  Sign placement interval: {sign_interval}");
                let mut distance_since_sign = 0.0;
                let mut sign_placed = false;

                for node in &way.nodes {
                    if let Some(start) = prev_node {
                        let dx_seg = node.x - start.x;
                        let dz_seg = node.z - start.z;
                        let mut rotation = (((dz_seg as f64).atan2(dx_seg as f64)
                            / (2.0 * std::f64::consts::PI))
                            * 16.0)
                            .round() as i8;
                        if rotation < 0 {
                            rotation += 16;
                        }

                        let side_dx = -dz_seg.signum();
                        let side_dz = dx_seg.signum();

                        let bres_points = bresenham_line(start.x, 0, start.z, node.x, 0, node.z);
                        let mut prev_point = (start.x, start.z);
                        for (x, _, z) in bres_points.into_iter().skip(1) {
                            let step = (((x - prev_point.0).pow(2) + (z - prev_point.1).pow(2))
                                as f64)
                                .sqrt();
                            distance_since_sign += step;
                            if distance_since_sign >= sign_interval {
                                let sign_x = x + side_dx * (block_range + 1);
                                let sign_z = z + side_dz * (block_range + 1);
                                let (min_x, min_z) = editor.get_min_coords();
                                let (max_x, max_z) = editor.get_max_coords();
                                let sign_y = editor.get_absolute_y(sign_x, 1, sign_z);
                                eprintln!(
                                    "  Attempting sign for '{name}' at ({sign_x}, {sign_y}, {sign_z}) with rotation {rotation}"
                                );
                                if sign_x >= min_x
                                    && sign_x <= max_x
                                    && sign_z >= min_z
                                    && sign_z <= max_z
                                {
                                    eprintln!(
                                        "  Placing sign for '{name}' at ({sign_x}, {sign_y}, {sign_z})"
                                    );
                                    let (l1, l2, l3, l4) = format_sign_text(name);
                                    editor.set_sign(l1, l2, l3, l4, sign_x, 1, sign_z, rotation);
                                    sign_placed = true;
                                } else {
                                    eprintln!(
                                        "  Skipping sign for '{name}' at ({sign_x}, {sign_y}, {sign_z}); out of bounds x:[{min_x},{max_x}] z:[{min_z},{max_z}]"
                                    );
                                }
                                distance_since_sign = 0.0;
                            }
                            prev_point = (x, z);
                        }
                    }
                    prev_node = Some(node);
                }
                if !sign_placed {
                    eprintln!("  No sign placed along highway '{name}', attempting fallback");
                    if let (Some(start), Some(next)) = (way.nodes.first(), way.nodes.get(1)) {
                        let dx_seg = next.x - start.x;
                        let dz_seg = next.z - start.z;
                        let mut rotation = (((dz_seg as f64).atan2(dx_seg as f64)
                            / (2.0 * std::f64::consts::PI))
                            * 16.0)
                            .round() as i8;
                        if rotation < 0 {
                            rotation += 16;
                        }
                        let side_dx = -dz_seg.signum();
                        let side_dz = dx_seg.signum();
                        let sign_x = start.x + side_dx * (block_range + 1);
                        let sign_z = start.z + side_dz * (block_range + 1);
                        let (min_x, min_z) = editor.get_min_coords();
                        let (max_x, max_z) = editor.get_max_coords();
                        let sign_y = editor.get_absolute_y(sign_x, 1, sign_z);
                        if sign_x >= min_x && sign_x <= max_x && sign_z >= min_z && sign_z <= max_z {
                            eprintln!(
                                "  Fallback placing sign for '{name}' at ({sign_x}, {sign_y}, {sign_z})"
                            );
                            let (l1, l2, l3, l4) = format_sign_text(name);
                            editor.set_sign(l1, l2, l3, l4, sign_x, 1, sign_z, rotation);
                        } else {
                            eprintln!(
                                "  Fallback sign for '{name}' out of bounds at ({sign_x}, {sign_y}, {sign_z})"
                            );
                        }
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Args;
    use crate::block_definitions::SIGN;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
    use crate::world_editor::WorldEditor;
    use std::collections::HashMap;
    use tempfile::tempdir;

    #[test]
    fn places_signs_every_200_meters() {
        let bbox = XZBBox::rect_from_xz_lengths(1010.0, 20.0).unwrap();
        let tmp = tempdir().unwrap();
        let region_dir = tmp.path().join("region");
        std::fs::create_dir(&region_dir).unwrap();
        let mut editor = WorldEditor::new(region_dir.to_str().unwrap(), &bbox);

        let args = Args {
            bbox: LLBBox::new(0., 0., 1., 1.).unwrap(),
            file: None,
            save_json_file: None,
            path: tmp.path().to_str().unwrap().to_string(),
            downloader: "requests".to_string(),
            scale: 1.0,
            ground_level: -62,
            terrain: false,
            interior: true,
            roof: true,
            fillground: false,
            debug: false,
            timeout: None,
            spawn_point: None,
        };

        let nodes = vec![
            ProcessedNode {
                id: 1,
                tags: HashMap::new(),
                x: 0,
                z: 0,
            },
            ProcessedNode {
                id: 2,
                tags: HashMap::new(),
                x: 1000,
                z: 0,
            },
        ];
        let mut tags = HashMap::new();
        tags.insert("highway".to_string(), "primary".to_string());
        tags.insert("name".to_string(), "First St.".to_string());
        let way = ProcessedWay { id: 1, nodes, tags };
        let element = ProcessedElement::Way(way);

        generate_highways(&mut editor, &element, &args);

        for x in [200, 400, 600, 800, 1000] {
            assert!(editor.check_for_block(x, 1, 6, Some(&[SIGN])));
        }
        assert!(!editor.check_for_block(100, 1, 6, Some(&[SIGN])));
    }

    #[test]
    fn short_roads_get_a_sign() {
        let bbox = XZBBox::rect_from_xz_lengths(110.0, 20.0).unwrap();
        let tmp = tempdir().unwrap();
        let region_dir = tmp.path().join("region");
        std::fs::create_dir(&region_dir).unwrap();
        let mut editor = WorldEditor::new(region_dir.to_str().unwrap(), &bbox);

        let args = Args {
            bbox: LLBBox::new(0., 0., 1., 1.).unwrap(),
            file: None,
            save_json_file: None,
            path: tmp.path().to_str().unwrap().to_string(),
            downloader: "requests".to_string(),
            scale: 1.0,
            ground_level: -62,
            terrain: false,
            interior: true,
            roof: true,
            fillground: false,
            debug: false,
            timeout: None,
            spawn_point: None,
        };

        let nodes = vec![
            ProcessedNode {
                id: 1,
                tags: HashMap::new(),
                x: 0,
                z: 0,
            },
            ProcessedNode {
                id: 2,
                tags: HashMap::new(),
                x: 100,
                z: 0,
            },
        ];
        let mut tags = HashMap::new();
        tags.insert("highway".to_string(), "primary".to_string());
        tags.insert("name".to_string(), "Short St.".to_string());
        let way = ProcessedWay { id: 1, nodes, tags };
        let element = ProcessedElement::Way(way);

        generate_highways(&mut editor, &element, &args);

        let mut found = false;
        for x in 0..=100 {
            if editor.check_for_block(x, 1, 6, Some(&[SIGN])) {
                found = true;
                break;
            }
        }
        assert!(found);
    }
}
