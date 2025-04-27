use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::cartesian::XZPoint;
use crate::floodfill::flood_fill_area;
use crate::ground::Ground;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;

pub fn generate_highways(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    ground: &Ground,
    args: &Args,
) {
    if let Some(highway_type) = element.tags().get("highway") {
        if highway_type == "street_lamp" {
            // Handle street lamps
            if let ProcessedElement::Node(first_node) = element {
                let x: i32 = first_node.x;
                let y: i32 = ground.level(first_node.xz());
                let z: i32 = first_node.z;
                for dy in 1..=4 {
                    editor.set_block(OAK_FENCE, x, y + dy, z, None, None);
                }
                editor.set_block(GLOWSTONE, x, y + 5, z, None, None);
            }
        } else if highway_type == "crossing" {
            // Handle traffic signals for crossings
            if let Some(crossing_type) = element.tags().get("crossing") {
                if crossing_type == "traffic_signals" {
                    if let ProcessedElement::Node(node) = element {
                        let x: i32 = node.x;
                        let y: i32 = ground.level(node.xz());
                        let z: i32 = node.z;

                        for dy in 1..=3 {
                            editor.set_block(COBBLESTONE_WALL, x, y + dy, z, None, None);
                        }

                        editor.set_block(GREEN_WOOL, x, y + 4, z, None, None);
                        editor.set_block(YELLOW_WOOL, x, y + 5, z, None, None);
                        editor.set_block(RED_WOOL, x, y + 6, z, None, None);
                    }
                }
            }
        } else if highway_type == "bus_stop" {
            // Handle bus stops
            if let ProcessedElement::Node(node) = element {
                let x = node.x;
                let y = ground.level(node.xz());
                let z = node.z;
                for dy in 1..=3 {
                    editor.set_block(COBBLESTONE_WALL, x, y + dy, z, None, None);
                }

                editor.set_block(WHITE_WOOL, x, y + 4, z, None, None);
                editor.set_block(WHITE_WOOL, x + 1, y + 4, z, None, None);
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
                editor.set_block(
                    surface_block,
                    x,
                    ground.level(XZPoint::new(x, z)),
                    z,
                    None,
                    None,
                );
            }
        } else {
            let mut previous_node: Option<(i32, i32)> = None;
            let mut block_type = BLACK_CONCRETE;
            let mut block_range: i32 = 2;
            let mut add_stripe = false;

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
                _ => {
                    if let Some(lanes) = element.tags().get("lanes") {
                        if lanes == "2" {
                            block_range = 3;
                            add_stripe = true;
                        } else if lanes != "1" {
                            block_range = 4;
                            add_stripe = true;
                        }
                    }
                }
            }

            let ProcessedElement::Way(way) = element else {
                return;
            };

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
                    let dash_length: i32 = 5; // Length of the solid part of the stripe
                    let gap_length: i32 = 5; // Length of the gap part of the stripe

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
                                                ground.level(XZPoint::new(set_x, set_z)),
                                                set_z,
                                                Some(&[BLACK_CONCRETE]),
                                                None,
                                            );
                                        } else {
                                            editor.set_block(
                                                BLACK_CONCRETE,
                                                set_x,
                                                ground.level(XZPoint::new(set_x, set_z)),
                                                set_z,
                                                None,
                                                None,
                                            );
                                        }
                                    } else if set_z % 2 < 1 {
                                        editor.set_block(
                                            WHITE_CONCRETE,
                                            set_x,
                                            ground.level(XZPoint::new(set_x, set_z)),
                                            set_z,
                                            Some(&[BLACK_CONCRETE]),
                                            None,
                                        );
                                    } else {
                                        editor.set_block(
                                            BLACK_CONCRETE,
                                            set_x,
                                            ground.level(XZPoint::new(set_x, set_z)),
                                            set_z,
                                            None,
                                            None,
                                        );
                                    }
                                } else {
                                    editor.set_block(
                                        block_type,
                                        set_x,
                                        ground.level(XZPoint::new(set_x, set_z)),
                                        set_z,
                                        None,
                                        Some(&[BLACK_CONCRETE, WHITE_CONCRETE]),
                                    );
                                }
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
                                    ground.level(XZPoint::new(stripe_x, stripe_z)),
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
        }
    }
}

/// Generates a siding using stone brick slabs
pub fn generate_siding(editor: &mut WorldEditor, element: &ProcessedWay, ground: &Ground) {
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
                let ground_level = ground.level(XZPoint::new(bx, bz)) + 1;

                if !editor.check_for_block(
                    bx,
                    ground_level - 1,
                    bz,
                    Some(&[BLACK_CONCRETE, WHITE_CONCRETE]),
                ) {
                    editor.set_block(siding_block, bx, ground_level, bz, None, None);
                }
            }
        }

        previous_node = Some(current_node);
    }
}

/// Generates an aeroway
pub fn generate_aeroway(editor: &mut WorldEditor, way: &ProcessedWay, ground: &Ground) {
    let mut previous_node: Option<(i32, i32)> = None;
    let surface_block = LIGHT_GRAY_CONCRETE;

    for node in &way.nodes {
        if let Some(prev) = previous_node {
            let (x1, z1) = prev;
            let x2 = node.x;
            let z2 = node.z;
            let points = bresenham_line(x1, 0, z1, x2, 0, z2);

            for (x, _, z) in points {
                for dx in -12..=12 {
                    for dz in -12..=12 {
                        let set_x = x + dx;
                        let set_z = z + dz;
                        let y = ground.level(XZPoint::new(set_x, set_z));
                        editor.set_block(surface_block, set_x, y, set_z, None, None);
                    }
                }
            }
        }
        previous_node = Some((node.x, node.z));
    }
}
