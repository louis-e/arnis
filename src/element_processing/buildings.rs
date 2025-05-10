use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::colors::{color_text_to_rgb_tuple, rgb_distance, RGBTuple};
use crate::floodfill::flood_fill_area;
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::Rng;
use std::collections::HashSet;
use std::time::Duration;

pub fn generate_buildings(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    relation_levels: Option<i32>,
) {
    // Get min_level first so we can use it both for start_level and building height calculations
    let min_level = if let Some(min_level_str) = element.tags.get("building:min_level") {
        min_level_str.parse::<i32>().unwrap_or(0)
    } else {
        0
    };

    // Calculate starting y-offset from min_level
    let scale_factor = args.scale;
    let start_y_offset = modname::multiply_scale(min_level * 4, scale_factor);

    let mut previous_node: Option<(i32, i32)> = None;
    let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
    let mut current_building: Vec<(i32, i32)> = vec![];

    // Randomly select block variations for corners, walls, and floors
    let mut rng: rand::prelude::ThreadRng = rand::thread_rng();
    let variation_index_corner: usize = rng.gen_range(0..BUILDING_CORNER_VARIATIONS.len());
    let variation_index_wall: usize = rng.gen_range(0..building_wall_variations().len());
    let variation_index_floor: usize = rng.gen_range(0..building_floor_variations().len());

    let corner_block: Block = BUILDING_CORNER_VARIATIONS[variation_index_corner];
    let wall_block: Block = element
        .tags
        .get("building:colour")
        .and_then(|building_colour: &String| {
            color_text_to_rgb_tuple(building_colour).map(|rgb: (u8, u8, u8)| {
                find_nearest_block_in_color_map(&rgb, &BUILDING_WALL_COLOR_MAP)
            })
        })
        .flatten()
        .unwrap_or_else(|| building_wall_variations()[variation_index_wall]);
    let floor_block: Block = element
        .tags
        .get("roof:colour")
        .and_then(|roof_colour: &String| {
            color_text_to_rgb_tuple(roof_colour).map(|rgb: (u8, u8, u8)| {
                find_nearest_block_in_color_map(&rgb, &BUILDING_FLOOR_COLOR_MAP)
            })
        })
        .flatten()
        .unwrap_or_else(|| {
            if let Some(building_type) = element
                .tags
                .get("building")
                .or_else(|| element.tags.get("building:part"))
            {
                //Random roof color only for single houses
                match building_type.as_str() {
                    "yes" | "house" | "detached" | "static_caravan" | "semidetached_house"
                    | "bungalow" | "manor" | "villa" => {
                        return building_floor_variations()[variation_index_floor];
                    }
                    _ => return LIGHT_GRAY_CONCRETE,
                }
            }
            LIGHT_GRAY_CONCRETE
        });
    let window_block: Block = WHITE_STAINED_GLASS;

    // Set to store processed flood fill points
    let mut processed_points: HashSet<(i32, i32)> = HashSet::new();
    let mut building_height: i32 = ((6.0 * scale_factor) as i32).max(3); // Default building height with scale and minimum

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
            let lev = levels - min_level;

            if lev >= 1 {
                building_height = modname::multiply_scale(levels * 4 + 2, scale_factor);
                building_height = building_height.max(3);
            }
        }
    }

    if let Some(height_str) = element.tags.get("height") {
        if let Ok(height) = height_str.trim_end_matches("m").trim().parse::<f64>() {
            building_height = (height * scale_factor) as i32;
            building_height = building_height.max(3);
        }
    }

    if let Some(levels) = relation_levels {
        building_height = modname::multiply_scale(levels * 4 + 2, scale_factor);
        building_height = building_height.max(3);
    }

    if let Some(amenity_type) = element.tags.get("amenity") {
        if amenity_type == "shelter" {
            let roof_block: Block = STONE_BRICK_SLAB;

            let polygon_coords: Vec<(i32, i32)> = element
                .nodes
                .iter()
                .map(|n: &crate::osm_parser::ProcessedNode| (n.x, n.z))
                .collect();
            let roof_area: Vec<(i32, i32)> =
                flood_fill_area(&polygon_coords, args.timeout.as_ref());

            // Place fences and roof slabs at each corner node directly
            for node in &element.nodes {
                let x: i32 = node.x;
                let z: i32 = node.z;

                for shelter_y in 1..=modname::multiply_scale(4, scale_factor) {
                    editor.set_block(OAK_FENCE, x, shelter_y, z, None, None);
                }
                editor.set_block(roof_block, x, 5, z, None, None);
            }

            // Flood fill the roof area
            for (x, z) in roof_area.iter() {
                editor.set_block(roof_block, *x, 5, *z, None, None);
            }

            return;
        }
    }

    if let Some(building_type) = element.tags.get("building") {
        if building_type == "garage" {
            building_height = ((2.0 * scale_factor) as i32).max(3);
        } else if building_type == "shed" {
            building_height = ((2.0 * scale_factor) as i32).max(3);

            if element.tags.contains_key("bicycle_parking") {
                let ground_block: Block = OAK_PLANKS;
                let roof_block: Block = STONE_BLOCK_SLAB;

                let polygon_coords: Vec<(i32, i32)> = element
                    .nodes
                    .iter()
                    .map(|n: &crate::osm_parser::ProcessedNode| (n.x, n.z))
                    .collect();
                let floor_area: Vec<(i32, i32)> =
                    flood_fill_area(&polygon_coords, args.timeout.as_ref());

                // Fill the floor area
                for (x, z) in floor_area.iter() {
                    editor.set_block(ground_block, *x, 0, *z, None, None);
                }

                // Place fences and roof slabs at each corner node directly
                for node in &element.nodes {
                    let x: i32 = node.x;
                    let z: i32 = node.z;

                    for dy in 1..=4 {
                        editor.set_block(OAK_FENCE, x, dy, z, None, None);
                    }
                    editor.set_block(roof_block, x, 5, z, None, None);
                }

                // Flood fill the roof area
                for (x, z) in floor_area.iter() {
                    editor.set_block(roof_block, *x, 5, *z, None, None);
                }

                return;
            }
        } else if building_type == "parking"
            || element
                .tags
                .get("parking")
                .is_some_and(|p| p == "multi-storey")
        {
            // Parking building structure

            // Ensure minimum height
            building_height = building_height.max(16);

            let polygon_coords: Vec<(i32, i32)> = element
                .nodes
                .iter()
                .map(|n: &crate::osm_parser::ProcessedNode| (n.x, n.z))
                .collect();
            let floor_area: Vec<(i32, i32)> =
                flood_fill_area(&polygon_coords, args.timeout.as_ref());

            for level in 0..=(building_height / 4) {
                let current_level_y = level * 4;

                // Build walls
                for node in &element.nodes {
                    let x: i32 = node.x;
                    let z: i32 = node.z;

                    // Build walls up to the current level
                    for y in (current_level_y + 1)..=(current_level_y + 4) {
                        editor.set_block(STONE_BRICKS, x, y, z, None, None);
                    }
                }

                // Fill the floor area for each level
                for (x, z) in &floor_area {
                    if level == 0 {
                        editor.set_block(SMOOTH_STONE, *x, current_level_y, *z, None, None);
                    } else {
                        editor.set_block(COBBLESTONE, *x, current_level_y, *z, None, None);
                    }
                }
            }

            // Outline for each level
            for level in 0..=(building_height / 4) {
                let current_level_y = level * 4;

                // Use the nodes to create the outline
                let mut prev_outline = None;
                for node in &element.nodes {
                    let x = node.x;
                    let z = node.z;

                    if let Some((prev_x, prev_z)) = prev_outline {
                        let outline_points =
                            bresenham_line(prev_x, current_level_y, prev_z, x, current_level_y, z);
                        for (bx, _, bz) in outline_points {
                            editor.set_block(
                                SMOOTH_STONE,
                                bx,
                                current_level_y,
                                bz,
                                Some(&[COBBLESTONE, COBBLESTONE_WALL]),
                                None,
                            );
                            editor.set_block(
                                STONE_BRICK_SLAB,
                                bx,
                                current_level_y + 2,
                                bz,
                                None,
                                None,
                            );
                            if bx % 2 == 0 {
                                editor.set_block(
                                    COBBLESTONE_WALL,
                                    bx,
                                    current_level_y + 1,
                                    bz,
                                    None,
                                    None,
                                );
                            }
                        }
                    }
                    prev_outline = Some((x, z));
                }
            }

            return;
        } else if building_type == "roof" {
            let roof_height: i32 = 5;

            // Iterate through the nodes to create the roof edges using Bresenham's line algorithm
            for node in &element.nodes {
                let x: i32 = node.x;
                let z: i32 = node.z;

                if let Some(prev) = previous_node {
                    let bresenham_points: Vec<(i32, i32, i32)> =
                        bresenham_line(prev.0, roof_height, prev.1, x, roof_height, z);
                    for (bx, _, bz) in bresenham_points {
                        editor.set_block(STONE_BRICK_SLAB, bx, roof_height, bz, None, None);
                        // Set roof block at edge
                    }
                }

                for y in 1..=(roof_height - 1) {
                    editor.set_block(COBBLESTONE_WALL, x, y, z, None, None);
                }

                previous_node = Some((x, z));
            }

            // Use flood-fill to fill the interior of the roof
            let polygon_coords: Vec<(i32, i32)> = element
                .nodes
                .iter()
                .map(|node: &crate::osm_parser::ProcessedNode| (node.x, node.z))
                .collect();
            let roof_area: Vec<(i32, i32)> =
                flood_fill_area(&polygon_coords, args.timeout.as_ref()); // Use flood-fill to determine the area

            // Fill the interior of the roof with STONE_BRICK_SLAB
            for (x, z) in roof_area.iter() {
                editor.set_block(STONE_BRICK_SLAB, *x, roof_height, *z, None, None);
                // Set roof block
            }

            return;
        } else if building_type == "apartments" {
            // If building has no height attribute, assign a defined height
            if building_height == ((6.0 * scale_factor) as i32).max(3) {
                building_height = ((15.0 * scale_factor) as i32).max(3);
            }
        } else if building_type == "hospital" {
            // If building has no height attribute, assign a defined height
            if building_height == ((6.0 * scale_factor) as i32).max(3) {
                building_height = ((23.0 * scale_factor) as i32).max(3);
            }
        } else if building_type == "bridge" {
            generate_bridge(editor, element, args.timeout.as_ref());
            return;
        }
    }

    // Process nodes to create walls and corners
    for node in &element.nodes {
        let x: i32 = node.x;
        let z: i32 = node.z;

        if let Some(prev) = previous_node {
            // Calculate walls and corners using Bresenham line
            let bresenham_points =
                bresenham_line(prev.0, start_y_offset, prev.1, x, start_y_offset, z);
            for (bx, _, bz) in bresenham_points {
                for h in (start_y_offset + 1)..=(start_y_offset + building_height) {
                    if element.nodes[0].x == bx && element.nodes[0].x == bz {
                        // Corner Block
                        editor.set_block(corner_block, bx, h, bz, None, None);
                    } else {
                        // Add windows to the walls at intervals
                        if h > start_y_offset + 1 && h % 4 != 0 && (bx + bz) % 6 < 3 {
                            editor.set_block(window_block, bx, h, bz, None, None);
                        } else {
                            editor.set_block(wall_block, bx, h, bz, None, None);
                        }
                    }
                }

                editor.set_block(
                    COBBLESTONE,
                    bx,
                    start_y_offset + building_height + 1,
                    bz,
                    None,
                    None,
                );

                current_building.push((bx, bz));
                corner_addup = (corner_addup.0 + bx, corner_addup.1 + bz, corner_addup.2 + 1);
            }
        }

        previous_node = Some((x, z));
    }

    // Flood-fill interior with floor variation
    if corner_addup != (0, 0, 0) {
        let polygon_coords: Vec<(i32, i32)> = element
            .nodes
            .iter()
            .map(|n: &crate::osm_parser::ProcessedNode| (n.x, n.z))
            .collect();
        let floor_area: Vec<(i32, i32)> = flood_fill_area(&polygon_coords, args.timeout.as_ref());

        for (x, z) in floor_area {
            if processed_points.insert((x, z)) {
                // Set floor at start_y_offset
                editor.set_block(floor_block, x, start_y_offset, z, None, None);

                // Set level ceilings if height > 4
                if building_height > 4 {
                    for h in (start_y_offset + 2 + 4..start_y_offset + building_height).step_by(4) {
                        if x % 6 == 0 && z % 6 == 0 {
                            // Light fixtures
                            editor.set_block(GLOWSTONE, x, h, z, None, None);
                        } else {
                            editor.set_block(floor_block, x, h, z, None, None);
                        }
                    }
                } else if x % 6 == 0 && z % 6 == 0 {
                    editor.set_block(
                        GLOWSTONE,
                        x,
                        start_y_offset + building_height,
                        z,
                        None,
                        None,
                    );
                }

                // Set ceiling at proper height
                editor.set_block(
                    floor_block,
                    x,
                    start_y_offset + building_height + 1,
                    z,
                    None,
                    None,
                );
            }
        }
    }
}

mod modname {
    pub(crate) fn multiply_scale(value: i32, scale_factor: f64) -> i32 {
        let result = (value as f64) * (scale_factor);
        result.floor() as i32
    }
}

pub fn generate_building_from_relation(
    editor: &mut WorldEditor,
    relation: &ProcessedRelation,
    args: &Args,
) {
    // Extract levels from relation tags
    let relation_levels = relation
        .tags
        .get("building:levels")
        .and_then(|l| l.parse::<i32>().ok())
        .unwrap_or(2); // Default to 2 levels

    // Process the outer way to create the building walls
    for member in &relation.members {
        if member.role == ProcessedMemberRole::Outer {
            generate_buildings(editor, &member.way, args, Some(relation_levels));
        }
    }

    // Handle inner ways (holes, courtyards, etc.)
    /*for member in &relation.members {
        if member.role == ProcessedMemberRole::Inner {
            let polygon_coords: Vec<(i32, i32)> =
                member.way.nodes.iter().map(|n| (n.x, n.z)).collect();
            let hole_area: Vec<(i32, i32)> =
                flood_fill_area(&polygon_coords, args.timeout.as_ref());

            for (x, z) in hole_area {
                // Remove blocks in the inner area to create a hole
                editor.set_block(AIR, x, ground_level, z, None, Some(&[SPONGE]));
            }
        }
    }*/
}

fn find_nearest_block_in_color_map(
    rgb: &RGBTuple,
    color_map: &[(RGBTuple, Block)],
) -> Option<Block> {
    color_map
        .iter()
        .min_by_key(|(entry_rgb, _)| rgb_distance(entry_rgb, rgb))
        .map(|(_, block)| block)
        .copied()
}

/// Generates a bridge structure, paying attention to the "level" tag.
fn generate_bridge(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    floodfill_timeout: Option<&Duration>,
) {
    let floor_block: Block = STONE;
    let railing_block: Block = STONE_BRICKS;

    // Process the nodes to create bridge pathways and railings
    let mut previous_node: Option<(i32, i32)> = None;
    for node in &element.nodes {
        let x: i32 = node.x;
        let z: i32 = node.z;

        // Calculate bridge level based on the "level" tag
        let bridge_y_offset = if let Some(level_str) = element.tags.get("level") {
            if let Ok(level) = level_str.parse::<i32>() {
                (level * 3) + 1
            } else {
                1 // Default elevation
            }
        } else {
            1 // Default elevation
        };

        // Create bridge path using Bresenham's line
        if let Some(prev) = previous_node {
            let bridge_points: Vec<(i32, i32, i32)> =
                bresenham_line(prev.0, bridge_y_offset, prev.1, x, bridge_y_offset, z);

            for (bx, by, bz) in bridge_points {
                // Place railing blocks
                editor.set_block(railing_block, bx, by + 1, bz, None, None);
                editor.set_block(railing_block, bx, by, bz, None, None);
            }
        }

        previous_node = Some((x, z));
    }

    // Flood fill the area between the bridge path nodes
    let polygon_coords: Vec<(i32, i32)> = element.nodes.iter().map(|n| (n.x, n.z)).collect();

    let bridge_area: Vec<(i32, i32)> = flood_fill_area(&polygon_coords, floodfill_timeout);

    // Calculate bridge level based on the "level" tag
    let bridge_y_offset = if let Some(level_str) = element.tags.get("level") {
        if let Ok(level) = level_str.parse::<i32>() {
            (level * 3) + 1
        } else {
            1 // Default elevation
        }
    } else {
        1 // Default elevation
    };

    // Place floor blocks
    for (x, z) in bridge_area {
        editor.set_block(floor_block, x, bridge_y_offset, z, None, None);
    }
}
