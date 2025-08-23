use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::colors::color_text_to_rgb_tuple;
use crate::coordinate_system::cartesian::XZPoint;
use crate::element_processing::subprocessor::buildings_interior::generate_building_interior;
use crate::floodfill::flood_fill_area;
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::Rng;
use std::collections::HashSet;
use std::time::Duration;

/// Enum representing different roof types
#[derive(Debug, Clone, Copy, PartialEq)]
enum RoofType {
    Gabled,    // Two sloping sides meeting at a ridge
    Hipped, // All sides slope downwards to walls (including Half-hipped, Gambrel, Mansard variations)
    Skillion, // Single sloping surface
    Pyramidal, // All sides come to a point at the top
    Dome,   // Rounded, hemispherical structure
    Flat,   // Default flat roof
}

#[inline]
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

    // Calculate y-offset for non-terrain mode for absolute positioning
    let abs_terrain_offset = if !args.terrain { args.ground_level } else { 0 };

    // Calculate starting y-offset from min_level
    let scale_factor = args.scale;
    let min_level_offset = multiply_scale(min_level * 4, scale_factor);

    // Cache floodfill result: compute once and reuse throughout
    let polygon_coords: Vec<(i32, i32)> = element.nodes.iter().map(|n| (n.x, n.z)).collect();
    let cached_floor_area: Vec<(i32, i32)> =
        flood_fill_area(&polygon_coords, args.timeout.as_ref());
    let cached_footprint_size = cached_floor_area.len();

    // Use fixed starting Y coordinate based on maximum ground level when terrain is enabled
    let start_y_offset = if args.terrain {
        // Get nodes' XZ points to find maximum elevation
        let building_points: Vec<XZPoint> = element
            .nodes
            .iter()
            .map(|n| {
                XZPoint::new(
                    n.x - editor.get_min_coords().0,
                    n.z - editor.get_min_coords().1,
                )
            })
            .collect();

        // Calculate maximum and minimum ground level across all nodes
        let mut max_ground_level = args.ground_level;

        for point in &building_points {
            if let Some(ground) = editor.get_ground() {
                let level = ground.level(*point);
                max_ground_level = max_ground_level.max(level);
            }
        }

        // Use the maximum level + min_level offset as the fixed base for the entire building
        max_ground_level + min_level_offset
    } else {
        // When terrain is disabled, just use min_level_offset
        min_level_offset
    };

    // Calculate building bounds and floor area before processing interior
    let min_x = element.nodes.iter().map(|n| n.x).min().unwrap_or(0);
    let max_x = element.nodes.iter().map(|n| n.x).max().unwrap_or(0);
    let min_z = element.nodes.iter().map(|n| n.z).min().unwrap_or(0);
    let max_z = element.nodes.iter().map(|n| n.z).max().unwrap_or(0);

    let mut previous_node: Option<(i32, i32)> = None;
    let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
    let mut current_building: Vec<(i32, i32)> = vec![];

    // Get building type for type-specific block selection
    let building_type = element
        .tags
        .get("building")
        .or_else(|| element.tags.get("building:part"))
        .map(|s| s.as_str())
        .unwrap_or("yes");

    let wall_block: Block = if element.tags.get("historic") == Some(&"castle".to_string()) {
        // Historic forts and castles should use stone/brick materials
        get_castle_wall_block()
    } else {
        element
            .tags
            .get("building:colour")
            .and_then(|building_colour: &String| {
                color_text_to_rgb_tuple(building_colour)
                    .map(|rgb: (u8, u8, u8)| get_building_wall_block_for_color(rgb))
            })
            .unwrap_or_else(get_fallback_building_block)
    };

    let floor_block: Block = get_random_floor_block();

    // Select window type based on building type
    let window_block: Block = get_window_block_for_building_type(building_type);

    // Set to store processed flood fill points
    let mut processed_points: HashSet<(i32, i32)> = HashSet::new();
    let mut building_height: i32 = ((6.0 * scale_factor) as i32).max(3); // Default building height with scale and minimum
    let mut is_tall_building = false;
    let mut rng = rand::thread_rng();
    let use_vertical_windows = rng.gen_bool(0.7);
    let use_accent_roof_line = rng.gen_bool(0.25);

    // Random accent block selection for this building
    let accent_blocks = [
        POLISHED_ANDESITE,
        SMOOTH_STONE,
        STONE_BRICKS,
        MUD_BRICKS,
        ANDESITE,
        CHISELED_STONE_BRICKS,
    ];
    let accent_block = accent_blocks[rng.gen_range(0..accent_blocks.len())];

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
                building_height = multiply_scale(levels * 4 + 2, scale_factor);
                building_height = building_height.max(3);

                // Mark as tall building if more than 7 stories
                if levels > 7 {
                    is_tall_building = true;
                }
            }
        }
    }

    if let Some(height_str) = element.tags.get("height") {
        if let Ok(height) = height_str.trim_end_matches("m").trim().parse::<f64>() {
            building_height = (height * scale_factor) as i32;
            building_height = building_height.max(3);

            // Mark as tall building if height suggests more than 7 stories
            if height > 28.0 {
                is_tall_building = true;
            }
        }
    }

    if let Some(levels) = relation_levels {
        building_height = multiply_scale(levels * 4 + 2, scale_factor);
        building_height = building_height.max(3);

        // Mark as tall building if more than 7 stories
        if levels > 7 {
            is_tall_building = true;
        }
    }

    // Determine accent line usage based on whether building has multiple floors
    let has_multiple_floors = building_height > 6;
    let use_accent_lines = has_multiple_floors && rng.gen_bool(0.2);
    let use_vertical_accent = has_multiple_floors && !use_accent_lines && rng.gen_bool(0.1);

    if let Some(amenity_type) = element.tags.get("amenity") {
        if amenity_type == "shelter" {
            let roof_block: Block = STONE_BRICK_SLAB;

            // Use cached floor area instead of recalculating
            let roof_area: &Vec<(i32, i32)> = &cached_floor_area;

            // Place fences and roof slabs at each corner node directly
            for node in &element.nodes {
                let x: i32 = node.x;
                let z: i32 = node.z;

                for shelter_y in 1..=multiply_scale(4, scale_factor) {
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

                // Use cached floor area instead of recalculating
                let floor_area: &Vec<(i32, i32)> = &cached_floor_area;

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

            // Use cached floor area instead of recalculating
            let floor_area: &Vec<(i32, i32)> = &cached_floor_area;

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
                for (x, z) in floor_area {
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

            // Use cached floor area
            let roof_area: &Vec<(i32, i32)> = &cached_floor_area;

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
                // Create foundation pillars from ground up to building base if needed
                // Only create foundations for buildings without min_level (elevated buildings shouldn't have foundations)
                if args.terrain && min_level == 0 {
                    // Calculate actual ground level at this position
                    let local_ground_level = if let Some(ground) = editor.get_ground() {
                        ground.level(XZPoint::new(
                            bx - editor.get_min_coords().0,
                            bz - editor.get_min_coords().1,
                        ))
                    } else {
                        args.ground_level
                    };

                    // Add foundation blocks from ground to building base
                    for y in local_ground_level..start_y_offset + 1 {
                        editor.set_block_absolute(
                            wall_block,
                            bx,
                            y + abs_terrain_offset,
                            bz,
                            None,
                            None,
                        );
                    }
                }

                for h in (start_y_offset + 1)..=(start_y_offset + building_height) {
                    // Add windows to the walls at intervals
                    // Use different window patterns for tall buildings
                    if is_tall_building && use_vertical_windows {
                        // Tall building pattern - narrower windows with continuous vertical strips
                        if h > start_y_offset + 1 && (bx + bz) % 3 == 0 {
                            editor.set_block_absolute(
                                window_block,
                                bx,
                                h + abs_terrain_offset,
                                bz,
                                None,
                                None,
                            );
                        } else {
                            editor.set_block_absolute(
                                wall_block,
                                bx,
                                h + abs_terrain_offset,
                                bz,
                                None,
                                None,
                            );
                        }
                    } else {
                        // Original pattern for regular buildings (non-vertical windows)
                        if h > start_y_offset + 1 && h % 4 != 0 && (bx + bz) % 6 < 3 {
                            editor.set_block_absolute(
                                window_block,
                                bx,
                                h + abs_terrain_offset,
                                bz,
                                None,
                                None,
                            );
                        } else {
                            // Use accent block line between windows if enabled for this building
                            let use_accent_line =
                                use_accent_lines && h > start_y_offset + 1 && h % 4 == 0;
                            // Use vertical accent block pattern (where windows would be, but on non-window Y levels) if enabled
                            let use_vertical_accent_here = use_vertical_accent
                                && h > start_y_offset + 1
                                && h % 4 == 0
                                && (bx + bz) % 6 < 3;

                            if use_accent_line || use_vertical_accent_here {
                                editor.set_block_absolute(
                                    accent_block,
                                    bx,
                                    h + abs_terrain_offset,
                                    bz,
                                    None,
                                    None,
                                );
                            } else {
                                editor.set_block_absolute(
                                    wall_block,
                                    bx,
                                    h + abs_terrain_offset,
                                    bz,
                                    None,
                                    None,
                                );
                            }
                        }
                    }
                }

                let roof_line_block = if use_accent_roof_line {
                    accent_block
                } else {
                    wall_block
                };
                editor.set_block_absolute(
                    roof_line_block,
                    bx,
                    start_y_offset + building_height + abs_terrain_offset + 1,
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
        // Use cached floor area
        let floor_area: &Vec<(i32, i32)> = &cached_floor_area;

        // Calculate floor heights for each level based on building height
        let mut floor_levels = Vec::new();

        // Always add the ground floor
        floor_levels.push(start_y_offset);

        // Calculate additional floors if building has sufficient height
        if building_height > 6 {
            // Determine number of floors (approximately 1 floor per 4 blocks of height)
            let num_upper_floors = (building_height / 4).max(1);

            // Add Y coordinates for each upper floor - match the intermediate floor placement
            // Main building code places intermediate floors at start_y_offset + 2 + 4, start_y_offset + 2 + 8, etc.
            for floor in 1..num_upper_floors {
                floor_levels.push(start_y_offset + 2 + (floor * 4));
            }
        }

        for (x, z) in floor_area.iter().cloned() {
            if processed_points.insert((x, z)) {
                // Create foundation columns for the floor area when using terrain
                if args.terrain {
                    // Calculate actual ground level at this position
                    if let Some(ground) = editor.get_ground() {
                        ground.level(XZPoint::new(
                            x - editor.get_min_coords().0,
                            z - editor.get_min_coords().1,
                        ))
                    } else {
                        args.ground_level
                    };
                }

                // Set floor at start_y_offset
                editor.set_block_absolute(
                    floor_block,
                    x,
                    start_y_offset + abs_terrain_offset,
                    z,
                    None,
                    None,
                );

                // Set level ceilings if height > 4
                if building_height > 4 {
                    for h in (start_y_offset + 2 + 4..start_y_offset + building_height).step_by(4) {
                        if x % 5 == 0 && z % 5 == 0 {
                            // Light fixtures
                            editor.set_block_absolute(
                                GLOWSTONE,
                                x,
                                h + abs_terrain_offset,
                                z,
                                None,
                                None,
                            );
                        } else {
                            editor.set_block_absolute(
                                floor_block,
                                x,
                                h + abs_terrain_offset,
                                z,
                                None,
                                None,
                            );
                        }
                    }
                } else if x % 5 == 0 && z % 5 == 0 {
                    editor.set_block_absolute(
                        GLOWSTONE,
                        x,
                        start_y_offset + building_height + abs_terrain_offset,
                        z,
                        None,
                        None,
                    );
                }

                // Only set ceiling at proper height if we don't use a specific roof shape or roof generation is disabled
                if !args.roof
                    || !element.tags.contains_key("roof:shape")
                    || element.tags.get("roof:shape").unwrap() == "flat"
                {
                    editor.set_block_absolute(
                        floor_block,
                        x,
                        start_y_offset + building_height + abs_terrain_offset + 1,
                        z,
                        None,
                        None,
                    );
                }
            }
        }

        // Generate interior features
        if args.interior {
            // Only generate interiors for buildings that aren't special types
            let building_type = element
                .tags
                .get("building")
                .map(|s| s.as_str())
                .unwrap_or("yes");
            let skip_interior = matches!(
                building_type,
                "garage" | "shed" | "parking" | "roof" | "bridge"
            );

            if !skip_interior && floor_area.len() > 100 {
                // Only for buildings with sufficient floor area
                generate_building_interior(
                    editor,
                    floor_area,
                    min_x,
                    min_z,
                    max_x,
                    max_z,
                    start_y_offset,
                    building_height,
                    wall_block,
                    &floor_levels,
                    args,
                    element,
                    abs_terrain_offset,
                );
            }
        }
    }

    // Process roof shapes if specified and roof generation is enabled
    if args.roof {
        if let Some(roof_shape) = element.tags.get("roof:shape") {
            let roof_type = match roof_shape.as_str() {
                "gabled" => RoofType::Gabled,
                "hipped" | "half-hipped" | "gambrel" | "mansard" | "round" => RoofType::Hipped,
                "skillion" => RoofType::Skillion,
                "pyramidal" => RoofType::Pyramidal,
                "dome" | "onion" | "cone" => RoofType::Dome,
                _ => RoofType::Flat,
            };

            generate_roof(
                editor,
                element,
                start_y_offset,
                building_height,
                floor_block,
                wall_block,
                accent_block,
                roof_type,
                &cached_floor_area,
                abs_terrain_offset,
            );
        } else {
            // Handle buildings without explicit roof:shape tag
            let building_type = element
                .tags
                .get("building")
                .map(|s| s.as_str())
                .unwrap_or("yes");

            // For apartments, give 80% chance to generate a gabled roof only if building footprint is not too large
            if building_type == "apartments"
                || building_type == "residential"
                || building_type == "house"
                || building_type == "yes"
            {
                // Use cached footprint area and size instead of recalculating
                let footprint_size = cached_footprint_size;

                // Maximum footprint size threshold for gabled roofs
                let max_footprint_for_gabled = 800;

                let mut rng = rand::thread_rng();
                if footprint_size <= max_footprint_for_gabled && rng.gen_bool(0.9) {
                    generate_roof(
                        editor,
                        element,
                        start_y_offset,
                        building_height,
                        floor_block,
                        wall_block,
                        accent_block,
                        RoofType::Gabled,
                        &cached_floor_area,
                        abs_terrain_offset,
                    );
                }
                // If footprint too large or not selected for gabled roof, building gets default flat roof (no action needed)
            }
            // Other building types without roof:shape get default flat roof (no action needed)
        }
    } else {
        // Default flat roof - already handled by the building generation code
    }
}

fn multiply_scale(value: i32, scale_factor: f64) -> i32 {
    // Use bit operations for faster multiplication when possible
    if scale_factor == 1.0 {
        value
    } else if scale_factor == 2.0 {
        value << 1
    } else if scale_factor == 4.0 {
        value << 2
    } else {
        let result = (value as f64) * scale_factor;
        result.floor() as i32
    }
}

/// Unified function to generate various roof types
#[allow(clippy::too_many_arguments)]
#[inline]
fn generate_roof(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    start_y_offset: i32,
    building_height: i32,
    floor_block: Block,
    wall_block: Block,
    accent_block: Block,
    roof_type: RoofType,
    cached_floor_area: &[(i32, i32)],
    abs_terrain_offset: i32,
) {
    // Use the provided cached floor area instead of recalculating
    let floor_area = cached_floor_area;

    // Pre-calculate building bounds once
    let (min_x, max_x, min_z, max_z) = element.nodes.iter().fold(
        (i32::MAX, i32::MIN, i32::MAX, i32::MIN),
        |(min_x, max_x, min_z, max_z), n| {
            (
                min_x.min(n.x),
                max_x.max(n.x),
                min_z.min(n.z),
                max_z.max(n.z),
            )
        },
    );

    let center_x = (min_x + max_x) >> 1; // Bit shift is faster than division
    let center_z = (min_z + max_z) >> 1;

    // Set base height for roof to be at least one block above building top
    let base_height = start_y_offset + building_height + 1;

    match roof_type {
        RoofType::Flat => {
            // Simple flat roof
            for &(x, z) in floor_area {
                editor.set_block_absolute(
                    floor_block,
                    x,
                    base_height + abs_terrain_offset,
                    z,
                    None,
                    None,
                );
            }
        }

        RoofType::Gabled => {
            // Pre-calculate building dimensions once
            let width = max_x - min_x;
            let length = max_z - min_z;
            let building_size = width.max(length);

            // Enhanced logarithmic scaling with increased base values for taller roofs
            let roof_height_boost = (3.0 + (building_size as f64 * 0.15).ln().max(1.0)) as i32;
            let roof_peak_height = base_height + roof_height_boost;

            // Pre-determine orientation and material
            let is_wider_than_long = width > length;
            let max_distance = if is_wider_than_long {
                length >> 1
            } else {
                width >> 1
            };

            // 50% accent block, otherwise wall block for roof
            let mut rng = rand::thread_rng();
            let roof_block = if rng.gen_bool(0.5) {
                accent_block
            } else {
                wall_block
            };

            // Pre-allocate with capacity hint for better performance
            let mut roof_heights = Vec::with_capacity(floor_area.len());
            let mut blocks_to_place = Vec::with_capacity(floor_area.len() * 4);

            // First pass: calculate all roof heights using vectorized operations
            for &(x, z) in floor_area {
                let distance_to_ridge = if is_wider_than_long {
                    (z - center_z).abs()
                } else {
                    (x - center_x).abs()
                };

                let roof_height = if distance_to_ridge == 0
                    && ((is_wider_than_long && z == center_z)
                        || (!is_wider_than_long && x == center_x))
                {
                    roof_peak_height
                } else {
                    let slope_ratio = distance_to_ridge as f64 / max_distance.max(1) as f64;
                    (roof_peak_height as f64 - (slope_ratio * roof_height_boost as f64)) as i32
                }
                .max(base_height);

                roof_heights.push(((x, z), roof_height));
            }

            // Second pass: batch process blocks with pre-computed stair materials
            let stair_block_material = get_stair_block_for_material(roof_block);

            for &((x, z), roof_height) in &roof_heights {
                // Check neighboring heights efficiently using iterator
                let has_lower_neighbor = roof_heights
                    .iter()
                    .filter_map(|&((nx, nz), nh)| {
                        if (nx - x).abs() + (nz - z).abs() == 1 {
                            Some(nh)
                        } else {
                            None
                        }
                    })
                    .any(|nh| nh < roof_height);

                // Fill from base height to calculated roof height
                for y in base_height..=roof_height {
                    if y == roof_height && has_lower_neighbor {
                        // Pre-compute stair direction
                        let stair_block_with_props = if is_wider_than_long {
                            if z < center_z {
                                create_stair_with_properties(
                                    stair_block_material,
                                    StairFacing::South,
                                    StairShape::Straight,
                                )
                            } else {
                                create_stair_with_properties(
                                    stair_block_material,
                                    StairFacing::North,
                                    StairShape::Straight,
                                )
                            }
                        } else if x < center_x {
                            create_stair_with_properties(
                                stair_block_material,
                                StairFacing::East,
                                StairShape::Straight,
                            )
                        } else {
                            create_stair_with_properties(
                                stair_block_material,
                                StairFacing::West,
                                StairShape::Straight,
                            )
                        };

                        blocks_to_place.push((x, y, z, roof_block, Some(stair_block_with_props)));
                    } else {
                        blocks_to_place.push((x, y, z, roof_block, None));
                    }
                }
            }

            // Batch place all blocks to reduce function call overhead
            for (x, y, z, block, stair_props) in blocks_to_place {
                if let Some(stair_block) = stair_props {
                    editor.set_block_with_properties_absolute(
                        stair_block,
                        x,
                        y + abs_terrain_offset,
                        z,
                        None,
                        None,
                    );
                } else {
                    editor.set_block_absolute(block, x, y + abs_terrain_offset, z, None, None);
                }
            }
        }

        RoofType::Hipped => {
            // Calculate building dimensions and determine the long axis
            let width = max_x - min_x;
            let length = max_z - min_z;

            // Determine if building is significantly rectangular or more square-shaped
            let is_rectangular =
                (width as f64 / length as f64 > 1.3) || (length as f64 / width as f64 > 1.3);
            let long_axis_is_x = width > length;

            // Make roof taller and more pointy
            let roof_peak_height = base_height + if width.max(length) > 20 { 7 } else { 5 };

            // 50% accent block, otherwise wall block for roof
            let mut rng = rand::thread_rng();
            let roof_block = if rng.gen_bool(0.5) {
                accent_block
            } else {
                wall_block
            };

            // Find the building's approximate center line along the long axis
            if is_rectangular {
                // First pass: calculate all roof heights
                let mut roof_heights = std::collections::HashMap::new();

                for &(x, z) in floor_area {
                    // Calculate distance to the ridge line
                    let distance_to_ridge = if long_axis_is_x {
                        // Distance in Z direction for X-axis ridge
                        (z - center_z).abs()
                    } else {
                        // Distance in X direction for Z-axis ridge
                        (x - center_x).abs()
                    };

                    // Calculate maximum distance from ridge to edge
                    let max_distance_from_ridge = if long_axis_is_x {
                        (max_z - min_z) / 2
                    } else {
                        (max_x - min_x) / 2
                    };

                    // Create proper slope from ridge (high) to edges (low)
                    let slope_factor = if max_distance_from_ridge > 0 {
                        distance_to_ridge as f64 / max_distance_from_ridge as f64
                    } else {
                        0.0
                    };

                    // Ridge gets peak height, edges get base height
                    let roof_height = roof_peak_height
                        - (slope_factor * (roof_peak_height - base_height) as f64) as i32;
                    let roof_y = roof_height.max(base_height);
                    roof_heights.insert((x, z), roof_y);
                }

                // Second pass: place blocks with stairs at height transitions
                for &(x, z) in floor_area {
                    let roof_height = roof_heights[&(x, z)];

                    // Fill from base to calculated height
                    for y in base_height..=roof_height {
                        if y == roof_height {
                            // Check if this is a height transition point by looking at neighboring blocks
                            let has_lower_neighbor =
                                [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)].iter().any(
                                    |(nx, nz)| {
                                        roof_heights
                                            .get(&(*nx, *nz))
                                            .is_some_and(|&nh| nh < roof_height)
                                    },
                                );

                            if has_lower_neighbor {
                                // Determine stair direction based on ridge orientation and position
                                let stair_block_material = get_stair_block_for_material(roof_block);
                                let stair_block_with_props = if long_axis_is_x {
                                    // Ridge runs along X, slopes in Z direction
                                    if z < center_z {
                                        create_stair_with_properties(
                                            stair_block_material,
                                            StairFacing::South,
                                            StairShape::Straight,
                                        ) // Facing toward center (south)
                                    } else {
                                        create_stair_with_properties(
                                            stair_block_material,
                                            StairFacing::North,
                                            StairShape::Straight,
                                        ) // Facing toward center (north)
                                    }
                                } else {
                                    // Ridge runs along Z, slopes in X direction
                                    if x < center_x {
                                        create_stair_with_properties(
                                            stair_block_material,
                                            StairFacing::East,
                                            StairShape::Straight,
                                        ) // Facing toward center (east)
                                    } else {
                                        create_stair_with_properties(
                                            stair_block_material,
                                            StairFacing::West,
                                            StairShape::Straight,
                                        ) // Facing toward center (west)
                                    }
                                };
                                editor.set_block_with_properties_absolute(
                                    stair_block_with_props,
                                    x,
                                    y + abs_terrain_offset,
                                    z,
                                    None,
                                    None,
                                );
                            } else {
                                // Use regular roof block where height doesn't change (ridge area)
                                editor.set_block_absolute(
                                    roof_block,
                                    x,
                                    y + abs_terrain_offset,
                                    z,
                                    None,
                                    None,
                                );
                            }
                        } else {
                            // Fill interior with solid blocks
                            editor.set_block_absolute(
                                roof_block,
                                x,
                                y + abs_terrain_offset,
                                z,
                                None,
                                None,
                            );
                        }
                    }
                }
            } else {
                // For more complex or square buildings, use distance from center approach

                // First pass: calculate all roof heights based on distance from center
                let mut roof_heights = std::collections::HashMap::new();

                for &(x, z) in floor_area {
                    // Calculate distance from center point
                    let dx = (x - center_x) as f64;
                    let dz = (z - center_z) as f64;
                    let distance_from_center = (dx * dx + dz * dz).sqrt();

                    // Calculate maximum possible distance from center to any corner
                    let max_distance = {
                        let corner_distances = [
                            ((min_x - center_x).pow(2) + (min_z - center_z).pow(2)) as f64,
                            ((min_x - center_x).pow(2) + (max_z - center_z).pow(2)) as f64,
                            ((max_x - center_x).pow(2) + (min_z - center_z).pow(2)) as f64,
                            ((max_x - center_x).pow(2) + (max_z - center_z).pow(2)) as f64,
                        ];
                        corner_distances
                            .iter()
                            .fold(0.0f64, |a, &b| a.max(b))
                            .sqrt()
                    };

                    // Create slope from center (high) to edges (low)
                    let distance_factor = if max_distance > 0.0 {
                        (distance_from_center / max_distance).min(1.0)
                    } else {
                        0.0
                    };

                    // Center gets peak height, edges get base height
                    let roof_height = roof_peak_height
                        - (distance_factor * (roof_peak_height - base_height) as f64) as i32;
                    let roof_y = roof_height.max(base_height);
                    roof_heights.insert((x, z), roof_y);
                }

                // Second pass: place blocks with stairs at height transitions
                for &(x, z) in floor_area {
                    let roof_height = roof_heights[&(x, z)];

                    // Fill from base height to calculated roof height
                    for y in base_height..=roof_height {
                        if y == roof_height {
                            // Check if this is a height transition point by looking at neighboring blocks
                            let has_lower_neighbor =
                                [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)].iter().any(
                                    |(nx, nz)| {
                                        roof_heights
                                            .get(&(*nx, *nz))
                                            .is_some_and(|&nh| nh < roof_height)
                                    },
                                );

                            if has_lower_neighbor {
                                // For complex buildings, determine stair direction based on slope toward center
                                let center_dx = x - center_x;
                                let center_dz = z - center_z;

                                let stair_block_material = get_stair_block_for_material(roof_block);
                                let stair_block = if center_dx.abs() > center_dz.abs() {
                                    // Primary slope is in X direction
                                    if center_dx > 0 {
                                        create_stair_with_properties(
                                            stair_block_material,
                                            StairFacing::West,
                                            StairShape::Straight,
                                        ) // Facing toward center
                                    } else {
                                        create_stair_with_properties(
                                            stair_block_material,
                                            StairFacing::East,
                                            StairShape::Straight,
                                        ) // Facing toward center
                                    }
                                } else {
                                    // Primary slope is in Z direction
                                    if center_dz > 0 {
                                        create_stair_with_properties(
                                            stair_block_material,
                                            StairFacing::North,
                                            StairShape::Straight,
                                        ) // Facing toward center
                                    } else {
                                        create_stair_with_properties(
                                            stair_block_material,
                                            StairFacing::South,
                                            StairShape::Straight,
                                        ) // Facing toward center
                                    }
                                };

                                editor.set_block_with_properties_absolute(
                                    stair_block,
                                    x,
                                    y + abs_terrain_offset,
                                    z,
                                    None,
                                    None,
                                );
                            } else {
                                // Use regular roof block where height doesn't change
                                editor.set_block_absolute(
                                    roof_block,
                                    x,
                                    y + abs_terrain_offset,
                                    z,
                                    None,
                                    None,
                                );
                            }
                        } else {
                            // Fill interior with solid blocks
                            editor.set_block_absolute(
                                roof_block,
                                x,
                                y + abs_terrain_offset,
                                z,
                                None,
                                None,
                            );
                        }
                    }
                }
            }
        }

        RoofType::Skillion => {
            // Skillion roof - single sloping surface
            let width = (max_x - min_x).max(1);
            let building_size = (max_x - min_x).max(max_z - min_z);

            // Scale roof height based on building size (4-10 blocks)
            let max_roof_height = (building_size / 3).clamp(4, 10);

            // 50% accent block, otherwise wall block for roof
            let mut rng = rand::thread_rng();
            let roof_block = if rng.gen_bool(0.5) {
                accent_block
            } else {
                wall_block
            };

            // First pass: calculate all roof heights
            let mut roof_heights = std::collections::HashMap::new();
            for &(x, z) in floor_area {
                let slope_progress = (x - min_x) as f64 / width as f64;
                let roof_height = base_height + (slope_progress * max_roof_height as f64) as i32;
                roof_heights.insert((x, z), roof_height);
            }

            // Second pass: place blocks with stairs only where height increases
            for &(x, z) in floor_area {
                let roof_height = roof_heights[&(x, z)];

                // Fill from base height to calculated roof height to create solid roof
                for y in base_height..=roof_height {
                    if y == roof_height {
                        // Check if this is a height transition point by looking at neighboring blocks
                        let has_lower_neighbor = [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)]
                            .iter()
                            .any(|(nx, nz)| {
                                roof_heights
                                    .get(&(*nx, *nz))
                                    .is_some_and(|&nh| nh < roof_height)
                            });

                        if has_lower_neighbor {
                            // Place stairs at height transitions for a stepped appearance
                            let stair_block_material = get_stair_block_for_material(roof_block);
                            let stair_block_with_props = create_stair_with_properties(
                                stair_block_material,
                                StairFacing::East,
                                StairShape::Straight,
                            );
                            editor.set_block_with_properties_absolute(
                                stair_block_with_props,
                                x,
                                y + abs_terrain_offset,
                                z,
                                None,
                                None,
                            );
                        } else {
                            // Use regular roof material where height doesn't change
                            editor.set_block_absolute(
                                roof_block,
                                x,
                                y + abs_terrain_offset,
                                z,
                                None,
                                None,
                            );
                        }
                    } else {
                        // Fill interior with solid blocks
                        editor.set_block_absolute(
                            roof_block,
                            x,
                            y + abs_terrain_offset,
                            z,
                            None,
                            None,
                        );
                    }
                }
            }
        }

        RoofType::Pyramidal => {
            // Pyramidal roof - all sides slope to a single central peak point
            let building_size = (max_x - min_x).max(max_z - min_z);

            // Calculate peak height based on building size (taller peak for larger buildings)
            let peak_height = base_height + (building_size / 3).clamp(3, 8);

            // 50% accent block, otherwise wall block for roof
            let mut rng = rand::thread_rng();
            let roof_block = if rng.gen_bool(0.5) {
                accent_block
            } else {
                wall_block
            };

            // First pass: calculate all roof heights
            let mut roof_heights = std::collections::HashMap::new();
            for &(x, z) in floor_area {
                // Calculate distance from this point to the center
                let dx = (x - center_x).abs() as f64;
                let dz = (z - center_z).abs() as f64;

                // Use the maximum distance to either edge to determine slope
                // This creates the pyramid effect where all sides slope equally
                let distance_to_edge = dx.max(dz);

                // Calculate maximum distance from center to any edge
                let max_distance = ((max_x - min_x) / 2).max((max_z - min_z) / 2) as f64;

                // Calculate height based on distance from center
                // Points closer to center are higher, creating the pyramid slope
                let height_factor = if max_distance > 0.0 {
                    (1.0 - (distance_to_edge / max_distance)).max(0.0f64)
                } else {
                    1.0
                };

                let roof_height =
                    base_height + (height_factor * (peak_height - base_height) as f64) as i32;
                roof_heights.insert((x, z), roof_height);
            }

            // Second pass: place blocks with stairs at the surface
            for &(x, z) in floor_area {
                let roof_height = roof_heights[&(x, z)];

                // Fill from base height to calculated roof height to create solid pyramid
                for y in base_height..=roof_height {
                    if y == roof_height {
                        // Place stairs at the surface with correct facing direction
                        // Determine which direction the stairs should face based on the slope
                        let dx = x - center_x;
                        let dz = z - center_z;

                        // Check if there are higher neighbors to determine stair orientation
                        let north_height = roof_heights
                            .get(&(x, z - 1))
                            .copied()
                            .unwrap_or(base_height);
                        let south_height = roof_heights
                            .get(&(x, z + 1))
                            .copied()
                            .unwrap_or(base_height);
                        let west_height = roof_heights
                            .get(&(x - 1, z))
                            .copied()
                            .unwrap_or(base_height);
                        let east_height = roof_heights
                            .get(&(x + 1, z))
                            .copied()
                            .unwrap_or(base_height);

                        // Check for corner situations where two directions have lower neighbors
                        let has_lower_north = north_height < roof_height;
                        let has_lower_south = south_height < roof_height;
                        let has_lower_west = west_height < roof_height;
                        let has_lower_east = east_height < roof_height;

                        // Check for corner situations (two adjacent directions are lower)
                        let stair_block_material = get_stair_block_for_material(roof_block);
                        let stair_block = if has_lower_north && has_lower_west {
                            create_stair_with_properties(
                                stair_block_material,
                                StairFacing::East,
                                StairShape::OuterRight,
                            )
                        } else if has_lower_north && has_lower_east {
                            create_stair_with_properties(
                                stair_block_material,
                                StairFacing::South,
                                StairShape::OuterRight,
                            )
                        } else if has_lower_south && has_lower_west {
                            create_stair_with_properties(
                                stair_block_material,
                                StairFacing::East,
                                StairShape::OuterLeft,
                            )
                        } else if has_lower_south && has_lower_east {
                            create_stair_with_properties(
                                stair_block_material,
                                StairFacing::North,
                                StairShape::OuterLeft,
                            )
                        } else {
                            // Single direction
                            if dx.abs() > dz.abs() {
                                // Primary slope is in X direction
                                if dx > 0 && east_height < roof_height {
                                    create_stair_with_properties(
                                        stair_block_material,
                                        StairFacing::West,
                                        StairShape::Straight,
                                    ) // Facing west (stairs face toward center)
                                } else if dx < 0 && west_height < roof_height {
                                    create_stair_with_properties(
                                        stair_block_material,
                                        StairFacing::East,
                                        StairShape::Straight,
                                    ) // Facing east (stairs face toward center)
                                } else if dz > 0 && south_height < roof_height {
                                    create_stair_with_properties(
                                        stair_block_material,
                                        StairFacing::North,
                                        StairShape::Straight,
                                    ) // Facing north (stairs face toward center)
                                } else if dz < 0 && north_height < roof_height {
                                    create_stair_with_properties(
                                        stair_block_material,
                                        StairFacing::South,
                                        StairShape::Straight,
                                    ) // Facing south (stairs face toward center)
                                } else {
                                    BlockWithProperties::simple(roof_block) // Use regular block if no clear slope direction
                                }
                            } else {
                                // Primary slope is in Z direction
                                if dz > 0 && south_height < roof_height {
                                    create_stair_with_properties(
                                        stair_block_material,
                                        StairFacing::North,
                                        StairShape::Straight,
                                    ) // Facing north (stairs face toward center)
                                } else if dz < 0 && north_height < roof_height {
                                    create_stair_with_properties(
                                        stair_block_material,
                                        StairFacing::South,
                                        StairShape::Straight,
                                    ) // Facing south (stairs face toward center)
                                } else if dx > 0 && east_height < roof_height {
                                    create_stair_with_properties(
                                        stair_block_material,
                                        StairFacing::West,
                                        StairShape::Straight,
                                    ) // Facing west (stairs face toward center)
                                } else if dx < 0 && west_height < roof_height {
                                    create_stair_with_properties(
                                        stair_block_material,
                                        StairFacing::East,
                                        StairShape::Straight,
                                    ) // Facing east (stairs face toward center)
                                } else {
                                    BlockWithProperties::simple(roof_block) // Use regular block if no clear slope direction
                                }
                            }
                        };

                        editor.set_block_with_properties_absolute(
                            stair_block,
                            x,
                            y + abs_terrain_offset,
                            z,
                            None,
                            None,
                        );
                    } else {
                        // Fill interior with solid blocks
                        editor.set_block_absolute(
                            roof_block,
                            x,
                            y + abs_terrain_offset,
                            z,
                            None,
                            None,
                        );
                    }
                }
            }
        }

        RoofType::Dome => {
            // Dome roof - rounded hemispherical structure
            let radius = ((max_x - min_x).max(max_z - min_z) / 2) as f64;

            // 50% accent block, otherwise wall block for roof
            let mut rng = rand::thread_rng();
            let roof_block = if rng.gen_bool(0.5) {
                accent_block
            } else {
                wall_block
            };

            for &(x, z) in floor_area {
                let distance_from_center = ((x - center_x).pow(2) + (z - center_z).pow(2)) as f64;
                let normalized_distance = (distance_from_center.sqrt() / radius).min(1.0);

                // Use hemisphere equation to determine the height
                let height_factor = (1.0 - normalized_distance * normalized_distance).sqrt();
                let surface_height = base_height + (height_factor * (radius * 0.8)) as i32;

                // Fill from the base to the surface
                for y in base_height..=surface_height {
                    editor.set_block_absolute(roof_block, x, y + abs_terrain_offset, z, None, None);
                }
            }
        }
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
        .and_then(|l: &String| l.parse::<i32>().ok())
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
