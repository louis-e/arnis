use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::colors::{color_text_to_rgb_tuple, rgb_distance, RGBTuple};
use crate::floodfill::flood_fill_area;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;
use once_cell::sync::Lazy;
use rand::Rng;
use std::collections::HashSet;
use std::time::Duration;

pub fn generate_buildings(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    ground_level: i32,
    floodfill_timeout: Option<&Duration>,
) {
    let mut previous_node: Option<(i32, i32)> = None;
    let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
    let mut current_building: Vec<(i32, i32)> = vec![];

    // Randomly select block variations for corners, walls, and floors
    let mut rng: rand::prelude::ThreadRng = rand::thread_rng();
    let variation_index_corner: usize = rng.gen_range(0..building_corner_variations().len());
    let variation_index_wall: usize = rng.gen_range(0..building_wall_variations().len());
    let variation_index_floor: usize = rng.gen_range(0..building_floor_variations().len());

    let corner_block: &&once_cell::sync::Lazy<Block> =
        &building_corner_variations()[variation_index_corner];
    let wall_block: &once_cell::sync::Lazy<Block> = element
        .tags
        .get("building:colour")
        .and_then(|building_colour| {
            color_text_to_rgb_tuple(building_colour)
                .map(|rgb| find_nearest_block_in_color_map(&rgb, building_wall_color_map()))
        })
        .flatten()
        .unwrap_or_else(|| building_wall_variations()[variation_index_wall]);
    let floor_block: &once_cell::sync::Lazy<Block> = element
        .tags
        .get("roof:colour")
        .and_then(|roof_colour| {
            color_text_to_rgb_tuple(roof_colour)
                .map(|rgb| find_nearest_block_in_color_map(&rgb, building_floor_color_map()))
        })
        .flatten()
        .unwrap_or_else(|| building_floor_variations()[variation_index_floor]);
    let window_block: &once_cell::sync::Lazy<Block> = &WHITE_STAINED_GLASS;

    // Set to store processed flood fill points
    let mut processed_points: HashSet<(i32, i32)> = HashSet::new();
    let mut building_height: i32 = 6; // Default building height

    // Skip if 'layer' or 'level' is negative in the tags
    if let Some(layer) = element.tags.get("layer") {
        if layer.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(level) = element.tags.get("level") {
        if level.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    // Determine building height from tags
    if let Some(levels_str) = element.tags.get("building:levels") {
        if let Ok(levels) = levels_str.parse::<i32>() {
            if levels >= 1 && (levels * 4 + 2) > building_height {
                building_height = levels * 4 + 2;
            }
        }
    }

    if let Some(height_str) = element.tags.get("height") {
        if let Ok(height) = height_str.trim_end_matches("m").trim().parse::<f64>() {
            building_height = height.round() as i32;
        }
    }

    if let Some(building_type) = element.tags.get("building") {
        if building_type == "garage" {
            building_height = 2;
        } else if building_type == "shed" {
            building_height = 2;

            if element.tags.contains_key("bicycle_parking") {
                let ground_block: &once_cell::sync::Lazy<Block> = &OAK_PLANKS;
                let roof_block: &once_cell::sync::Lazy<Block> = &STONE_BLOCK_SLAB;

                let polygon_coords: Vec<(i32, i32)> =
                    element.nodes.iter().map(|n| (n.x, n.z)).collect();
                let floor_area: Vec<(i32, i32)> =
                    flood_fill_area(&polygon_coords, floodfill_timeout);

                // Fill the floor area
                for (x, z) in floor_area.iter() {
                    editor.set_block(ground_block, *x, ground_level, *z, None, None);
                }

                // Place fences and roof slabs at each corner node directly
                for node in &element.nodes {
                    let x = node.x;
                    let z = node.z;

                    for y in 1..=4 {
                        editor.set_block(ground_block, x, ground_level, z, None, None);
                        editor.set_block(&OAK_FENCE, x, ground_level + y, z, None, None);
                    }
                    editor.set_block(roof_block, x, ground_level + 5, z, None, None);
                }

                // Flood fill the roof area
                let roof_height: i32 = ground_level + 5;
                for (x, z) in floor_area.iter() {
                    editor.set_block(roof_block, *x, roof_height, *z, None, None);
                }

                return;
            }
        } else if building_type == "roof" {
            let roof_height = ground_level + 5;

            // Iterate through the nodes to create the roof edges using Bresenham's line algorithm
            for node in &element.nodes {
                let x = node.x;
                let z = node.z;

                if let Some(prev) = previous_node {
                    let bresenham_points: Vec<(i32, i32, i32)> =
                        bresenham_line(prev.0, roof_height, prev.1, x, roof_height, z);
                    for (bx, _, bz) in bresenham_points {
                        editor.set_block(&STONE_BRICK_SLAB, bx, roof_height, bz, None, None);
                        // Set roof block at edge
                    }
                }

                for y in (ground_level + 1)..=(roof_height - 1) {
                    editor.set_block(&COBBLESTONE_WALL, x, y, z, None, None);
                }

                previous_node = Some((x, z));
            }

            // Use flood-fill to fill the interior of the roof
            let polygon_coords: Vec<(i32, i32)> =
                element.nodes.iter().map(|node| (node.x, node.z)).collect();
            let roof_area: Vec<(i32, i32)> = flood_fill_area(&polygon_coords, floodfill_timeout); // Use flood-fill to determine the area

            // Fill the interior of the roof with STONE_BRICK_SLAB
            for (x, z) in roof_area.iter() {
                editor.set_block(&STONE_BRICK_SLAB, *x, roof_height, *z, None, None);
                // Set roof block
            }

            return;
        } else if building_type == "apartments" {
            // If building has no height attribute, assign a defined height
            if building_height == 6 {
                building_height = 15
            }
        } else if building_type == "hospital" {
            // If building has no height attribute, assign a defined height
            if building_height == 6 {
                building_height = 23
            }
        } else if building_type == "bridge" {
            generate_bridge(editor, element, ground_level, floodfill_timeout);
            return;
        }
    }

    // Process nodes to create walls and corners
    for node in &element.nodes {
        let x = node.x;
        let z = node.z;

        if let Some(prev) = previous_node {
            // Calculate walls and corners using Bresenham line
            let bresenham_points: Vec<(i32, i32, i32)> =
                bresenham_line(prev.0, ground_level, prev.1, x, ground_level, z);
            for (bx, _, bz) in bresenham_points {
                for h in (ground_level + 1)..=(ground_level + building_height) {
                    if element.nodes[0].x == bx && element.nodes[0].x == bz {
                        editor.set_block(corner_block, bx, h, bz, None, None); // Corner block
                    } else {
                        // Add windows to the walls at intervals
                        if h > ground_level + 1 && h % 4 != 0 && (bx + bz) % 6 < 3 {
                            editor.set_block(window_block, bx, h, bz, None, None);
                        // Window block
                        } else {
                            editor.set_block(wall_block, bx, h, bz, None, None);
                            // Wall block
                        }
                    }
                }
                editor.set_block(
                    &COBBLESTONE,
                    bx,
                    ground_level + building_height + 1,
                    bz,
                    None,
                    None,
                ); // Ceiling cobblestone
                current_building.push((bx, bz));
                corner_addup = (corner_addup.0 + bx, corner_addup.1 + bz, corner_addup.2 + 1);
            }
        }

        previous_node = Some((x, z));
    }

    // Flood-fill interior with floor variation
    if corner_addup != (0, 0, 0) {
        let polygon_coords: Vec<(i32, i32)> = element.nodes.iter().map(|n| (n.x, n.z)).collect();
        let floor_area: Vec<(i32, i32)> = flood_fill_area(&polygon_coords, floodfill_timeout);

        for (x, z) in floor_area {
            if processed_points.insert((x, z)) {
                editor.set_block(floor_block, x, ground_level, z, None, None); // Set floor

                // Set level ceilings if height > 4
                if building_height > 4 {
                    for h in (ground_level + 2 + 4..ground_level + building_height).step_by(4) {
                        if x % 6 == 0 && z % 6 == 0 {
                            editor.set_block(&GLOWSTONE, x, h, z, None, None); // Light fixtures
                        } else {
                            editor.set_block(floor_block, x, h, z, None, None);
                        }
                    }
                } else if x % 6 == 0 && z % 6 == 0 {
                    editor.set_block(&GLOWSTONE, x, ground_level + building_height, z, None, None);
                    // Light fixtures
                }

                // Set the house ceiling
                editor.set_block(
                    floor_block,
                    x,
                    ground_level + building_height + 1,
                    z,
                    None,
                    None,
                );
            }
        }
    }
}

fn find_nearest_block_in_color_map(
    rgb: &RGBTuple,
    color_map: Vec<(RGBTuple, &'static Lazy<Block>)>,
) -> Option<&'static once_cell::sync::Lazy<Block>> {
    color_map
        .into_iter()
        .min_by_key(|(entry_rgb, _)| rgb_distance(entry_rgb, rgb))
        .map(|(_, block)| block)
}

/// Generates a bridge structure, paying attention to the "level" tag.
fn generate_bridge(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    base_level: i32,
    floodfill_timeout: Option<&Duration>,
) {
    // Calculate the bridge level
    let mut bridge_level = base_level;
    if let Some(level_str) = element.tags.get("level") {
        if let Ok(level) = level_str.parse::<i32>() {
            bridge_level += (level * 3) + 1; // Adjust height by levels
        }
    }

    let floor_block: &once_cell::sync::Lazy<Block> = &STONE;
    let railing_block: &once_cell::sync::Lazy<Block> = &STONE_BRICKS;

    // Process the nodes to create bridge pathways and railings
    let mut previous_node: Option<(i32, i32)> = None;
    for node in &element.nodes {
        let x = node.x;
        let z = node.z;

        // Create bridge path using Bresenham's line
        if let Some(prev) = previous_node {
            let bridge_points: Vec<(i32, i32, i32)> =
                bresenham_line(prev.0, bridge_level, prev.1, x, bridge_level, z);
            for (bx, by, bz) in bridge_points {
                editor.set_block(railing_block, bx, by + 1, bz, None, None);
                editor.set_block(railing_block, bx, by, bz, None, None);
            }
        }
        previous_node = Some((x, z));
    }

    // Flood fill the area between the bridge path nodes
    let polygon_coords: Vec<(i32, i32)> = element.nodes.iter().map(|n| (n.x, n.z)).collect();
    let bridge_area: Vec<(i32, i32)> = flood_fill_area(&polygon_coords, floodfill_timeout);
    for (x, z) in bridge_area {
        editor.set_block(floor_block, x, bridge_level, z, None, None);
    }
}
