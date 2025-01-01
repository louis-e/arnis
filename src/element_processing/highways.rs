use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::floodfill::flood_fill_area;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;

pub fn generate_highways(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    ground_level: i32,
    args: &Args,
) {
    if let Some(highway_type) = element.tags().get("highway") {
        if highway_type == "street_lamp" {
            // Handle street lamps
            if let ProcessedElement::Node(first_node) = element {
                let x: i32 = first_node.x;
                let z: i32 = first_node.z;
                for y in 1..=4 {
                    editor.set_block(OAK_FENCE, x, ground_level + y, z, None, None);
                }
                editor.set_block(GLOWSTONE, x, ground_level + 5, z, None, None);
            }
        } else if highway_type == "crossing" {
            // Handle traffic signals for crossings
            if let Some(crossing_type) = element.tags().get("crossing") {
                if crossing_type == "traffic_signals" {
                    if let ProcessedElement::Node(node) = element {
                        let x: i32 = node.x;
                        let z: i32 = node.z;
                        for y in 1..=3 {
                            editor.set_block(COBBLESTONE_WALL, x, ground_level + y, z, None, None);
                        }

                        editor.set_block(GREEN_WOOL, x, ground_level + 4, z, None, None);
                        editor.set_block(YELLOW_WOOL, x, ground_level + 5, z, None, None);
                        editor.set_block(RED_WOOL, x, ground_level + 6, z, None, None);

                        if args.winter {
                            editor.set_block(SNOW_LAYER, x, ground_level + 7, z, None, None);
                        }
                    }
                }
            }
        } else if highway_type == "bus_stop" {
            // Handle bus stops
            if let ProcessedElement::Node(node) = element {
                let x: i32 = node.x;
                let z: i32 = node.z;
                for y in 1..=3 {
                    editor.set_block(COBBLESTONE_WALL, x, ground_level + y, z, None, None);
                }

                editor.set_block(WHITE_WOOL, x, ground_level + 4, z, None, None);
                editor.set_block(WHITE_WOOL, x + 1, ground_level + 4, z, None, None);
            }
        } else if element
            .tags()
            .get("area")
            .map_or(false, |v: &String| v == "yes")
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
                    "grass" => {
                        if args.winter {
                            SNOW_BLOCK
                        } else {
                            GRASS_BLOCK
                        }
                    }
                    "dirt" => DIRT,
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
                editor.set_block(surface_block, x, ground_level, z, None, None);
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
                    block_type = LIGHT_GRAY_CONCRETE;
                    block_range = 1;
                }
                "motorway" | "primary" => {
                    block_range = 5;
                    add_stripe = true; // Add stripes for motorways and primary roads
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
                    let bresenham_points: Vec<(i32, i32, i32)> =
                        bresenham_line(x1, ground_level, z1, x2, ground_level, z2);

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
                                                ground_level,
                                                set_z,
                                                Some(&[BLACK_CONCRETE]),
                                                None,
                                            );
                                        } else {
                                            editor.set_block(
                                                BLACK_CONCRETE,
                                                set_x,
                                                ground_level,
                                                set_z,
                                                None,
                                                None,
                                            );
                                        }
                                    } else if set_z % 2 < 1 {
                                        editor.set_block(
                                            WHITE_CONCRETE,
                                            set_x,
                                            ground_level,
                                            set_z,
                                            Some(&[BLACK_CONCRETE]),
                                            None,
                                        );
                                    } else {
                                        editor.set_block(
                                            BLACK_CONCRETE,
                                            set_x,
                                            ground_level,
                                            set_z,
                                            None,
                                            None,
                                        );
                                    }
                                } else {
                                    editor.set_block(
                                        block_type,
                                        set_x,
                                        ground_level,
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
                                    ground_level,
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

            // Generate block letters for street names
            if let Some(name) = element.tags().get("name") {
                generate_block_letters(editor, name, way, ground_level);
            }
        }
    }
}

/// Generates a siding using stone brick slabs
pub fn generate_siding(editor: &mut WorldEditor, element: &ProcessedWay, ground_level: i32) {
    let mut previous_node: Option<(i32, i32)> = None;
    let siding_block: Block = STONE_BRICK_SLAB;

    for node in &element.nodes {
        let x: i32 = node.x;
        let z: i32 = node.z;

        // Draw the siding using Bresenham's line algorithm between nodes
        if let Some(prev) = previous_node {
            let bresenham_points: Vec<(i32, i32, i32)> =
                bresenham_line(prev.0, ground_level + 1, prev.1, x, ground_level + 1, z);
            for (bx, by, bz) in bresenham_points {
                if !editor.check_for_block(
                    bx,
                    by - 1,
                    bz,
                    None,
                    Some(&[BLACK_CONCRETE, WHITE_CONCRETE]),
                ) {
                    editor.set_block(siding_block, bx, by, bz, None, None);
                }
            }
        }

        previous_node = Some((x, z));
    }
}

/// Generates block letters for street names
fn generate_block_letters(
    editor: &mut WorldEditor,
    name: &str,
    way: &ProcessedWay,
    ground_level: i32,
) {
    let block_type = WHITE_CONCRETE;
    let letter_spacing = 2;
    let mut current_x = way.nodes[0].x;
    let mut current_z = way.nodes[0].z;

    for letter in name.chars() {
        match letter {
            'A' => {
                // Draw letter A
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                    editor.set_block(block_type, current_x + 2, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level + 2, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 4, current_z, None, None);
            }
            'B' => {
                // Draw letter B
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level + 4, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 2, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 3, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 1, current_z, None, None);
            }
            'C' => {
                // Draw letter C
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 4, current_z, None, None);
            }
            'D' => {
                // Draw letter D
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 4, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 1, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 3, current_z, None, None);
            }
            'E' => {
                // Draw letter E
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 2, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 4, current_z, None, None);
            }
            'F' => {
                // Draw letter F
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level + 2, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 4, current_z, None, None);
            }
            'G' => {
                // Draw letter G
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 4, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 1, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 2, current_z, None, None);
            }
            'H' => {
                // Draw letter H
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                    editor.set_block(block_type, current_x + 2, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level + 2, current_z, None, None);
            }
            'I' => {
                // Draw letter I
                for i in 0..5 {
                    editor.set_block(block_type, current_x + 1, ground_level + i, current_z, None, None);
                }
            }
            'J' => {
                // Draw letter J
                for i in 0..5 {
                    editor.set_block(block_type, current_x + 1, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x, ground_level, current_z, None, None);
            }
            'K' => {
                // Draw letter K
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level + 2, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 1, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 3, current_z, None, None);
            }
            'L' => {
                // Draw letter L
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level, current_z, None, None);
            }
            'M' => {
                // Draw letter M
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                    editor.set_block(block_type, current_x + 2, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level + 3, current_z, None, None);
            }
            'N' => {
                // Draw letter N
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                    editor.set_block(block_type, current_x + 2, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level + 1, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 3, current_z, None, None);
            }
            'O' => {
                // Draw letter O
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                    editor.set_block(block_type, current_x + 2, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 4, current_z, None, None);
            }
            'P' => {
                // Draw letter P
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level + 2, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 3, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 1, current_z, None, None);
            }
            'Q' => {
                // Draw letter Q
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                    editor.set_block(block_type, current_x + 2, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 4, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 1, current_z + 1, None, None);
            }
            'R' => {
                // Draw letter R
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level + 2, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 3, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 1, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 4, current_z, None, None);
            }
            'S' => {
                // Draw letter S
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 2, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 4, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 1, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 3, current_z, None, None);
            }
            'T' => {
                // Draw letter T
                for i in 0..5 {
                    editor.set_block(block_type, current_x + 1, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x, ground_level + 4, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 4, current_z, None, None);
            }
            'U' => {
                // Draw letter U
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                    editor.set_block(block_type, current_x + 2, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level, current_z, None, None);
            }
            'V' => {
                // Draw letter V
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                    editor.set_block(block_type, current_x + 2, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level, current_z, None, None);
            }
            'W' => {
                // Draw letter W
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                    editor.set_block(block_type, current_x + 2, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level + 1, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 3, current_z, None, None);
            }
            'X' => {
                // Draw letter X
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                    editor.set_block(block_type, current_x + 2, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level + 1, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 3, current_z, None, None);
            }
            'Y' => {
                // Draw letter Y
                for i in 0..5 {
                    editor.set_block(block_type, current_x + 1, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x, ground_level + 3, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 3, current_z, None, None);
            }
            'Z' => {
                // Draw letter Z
                for i in 0..5 {
                    editor.set_block(block_type, current_x, ground_level + i, current_z, None, None);
                }
                editor.set_block(block_type, current_x + 1, ground_level, current_z, None, None);
                editor.set_block(block_type, current_x + 1, ground_level + 4, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 1, current_z, None, None);
                editor.set_block(block_type, current_x + 2, ground_level + 3, current_z, None, None);
            }
            _ => {}
        }

        // Move to the next position for the next letter
        current_x += letter_spacing + 3;
    }
}
