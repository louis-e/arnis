use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::floodfill::flood_fill_area;
use crate::osm_parser::{ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;

pub fn generate_railways(editor: &mut WorldEditor, element: &ProcessedWay) {
    if let Some(railway_type) = element.tags.get("railway") {
        // Check for underground railways (negative level or layer)
        let is_underground = element
            .tags
            .get("level")
            .is_some_and(|level| level.parse::<i32>().is_ok_and(|l| l < 0))
            || element
                .tags
                .get("layer")
                .is_some_and(|layer| layer.parse::<i32>().is_ok_and(|l| l < 0));

        // Also check for tunnel=yes tag
        let is_tunnel = element
            .tags
            .get("tunnel")
            .is_some_and(|tunnel| tunnel == "yes");

        // Skip certain railway types
        if [
            "proposed",
            "abandoned",
            "construction",
            "razed", // "turntable",
        ]
        .contains(&railway_type.as_str())
        {
            return;
        }

        if ["station", "platform"].contains(&railway_type.as_str()) {
            // Stations and platforms
            generate_station(editor, element);
        } else if element.tags.get("subway").is_some_and(|v| v == "yes")
            || is_underground
            || is_tunnel
        {
            // Subways and underground rails
            generate_subway(editor, element);
            return;
        } else {
            // Surface and elevated rails
            for i in 1..element.nodes.len() {
                let prev_node = element.nodes[i - 1].xz();
                let cur_node = element.nodes[i].xz();

                let points = bresenham_line(prev_node.x, 0, prev_node.z, cur_node.x, 0, cur_node.z);
                let smoothed_points = smooth_diagonal_rails(&points);

                for j in 0..smoothed_points.len() {
                    let (bx, _, bz) = smoothed_points[j];
                    let mut ground_level = 0;
                    if let Some(level_str) = element.tags.get("layer") {
                        if let Ok(level) = level_str.parse::<i32>() {
                            ground_level = level * 4;
                        }
                    }

                    if (bx + bz) % 4 == 0 {
                        editor.set_block(
                            OAK_PLANKS,
                            bx,
                            ground_level,
                            bz,
                            Some(&[DIRT, STONE, GRASS_BLOCK]),
                            None,
                        );
                    } else {
                        editor.set_block(
                            GRAVEL,
                            bx,
                            ground_level,
                            bz,
                            Some(&[DIRT, STONE, GRASS_BLOCK]),
                            None,
                        );
                    }

                    let prev = if j > 0 {
                        Some(smoothed_points[j - 1])
                    } else {
                        None
                    };
                    let next = if j < smoothed_points.len() - 1 {
                        Some(smoothed_points[j + 1])
                    } else {
                        None
                    };

                    let rail_block = determine_rail_direction(
                        (bx, bz),
                        prev.map(|(x, _, z)| (x, z)),
                        next.map(|(x, _, z)| (x, z)),
                    );

                    editor.set_block(
                        rail_block,
                        bx,
                        ground_level + 1,
                        bz,
                        Some(&[DIRT, STONE]),
                        None,
                    );
                    if ground_level < 0 {
                        // If underground, add air block above rails.

                        editor.set_block(AIR, bx, ground_level + 2, bz, Some(&[DIRT, STONE]), None);
                    }
                    if ground_level > 0 {
                        // If in the air, add block below gravel
                        editor.set_block(
                            GRAY_CONCRETE,
                            bx,
                            ground_level - 1,
                            bz,
                            Some(&[DIRT, STONE]),
                            None,
                        );
                    }
                }
            }
        }
    }
}
pub fn generate_rail_node(editor: &mut WorldEditor<'_>, node: &ProcessedNode) {
    let node_type = node.tags.get("railway").map_or("none", |v| v);
    let mut ground_level = 0;

    if let Some(level_str) = node.tags.get("layer") {
        if let Ok(level) = level_str.parse::<i32>() {
            ground_level = level * 4;
        }
    }
    if node_type == "halt" || node_type == "tram_stop" {
        editor.set_block(
            SMOOTH_STONE,
            node.x,
            ground_level,
            node.z,
            Some(&[DIRT, STONE, GRASS_BLOCK]),
            None,
        );
    } else if node_type == "subway_entrance" {
        let start_y_level: i32 = 1; // The Y-coordinate for the top-most cleared block
        let num_steps: i32 = 4; // Number of steps in the staircase

        for i in 0..num_steps {
            // Calculate the current x and y for this step
            let current_x = node.x - i + 1; // Each step moves one unit in negative X
            let current_y_top = start_y_level - i; // Each step goes down one Y level

            // Clear the three vertical blocks at the current X and Z
            editor.set_block(
                AIR,
                current_x,
                current_y_top + 1,
                node.z, // Z remains constant for an X-directional staircase
                Some(&[
                    DIRT,
                    STONE,
                    GRASS_BLOCK,
                    LIGHT_GRAY_CONCRETE,
                    GRAY_CONCRETE,
                    SMOOTH_STONE,
                ]),
                None,
            );
            editor.set_block(
                AIR,
                current_x,
                current_y_top,
                node.z, // Z remains constant for an X-directional staircase
                Some(&[
                    DIRT,
                    STONE,
                    GRASS_BLOCK,
                    LIGHT_GRAY_CONCRETE,
                    GRAY_CONCRETE,
                    SMOOTH_STONE,
                ]),
                None,
            );
            editor.set_block(
                AIR,
                current_x,
                current_y_top - 1,
                node.z,
                Some(&[DIRT, STONE, GRASS_BLOCK, LIGHT_GRAY_CONCRETE, GRAY_CONCRETE]),
                None,
            );
        }
        for dx in -4..=3 {
            for dz in -2..=2 {
                editor.set_block(
                    AIR,
                    node.x + dx,
                    1 - num_steps,
                    node.z + dz,
                    Some(&[DIRT, STONE, POLISHED_ANDESITE]),
                    None,
                );
                editor.set_block(
                    AIR,
                    node.x + dx,
                    2 - num_steps,
                    node.z + dz,
                    Some(&[DIRT, STONE, POLISHED_ANDESITE]),
                    None,
                );
                editor.set_block(
                    LIGHT_GRAY_CONCRETE,
                    node.x + dx,
                    0 - num_steps,
                    node.z + dz,
                    Some(&[DIRT, STONE, POLISHED_ANDESITE]),
                    None,
                );
            }
        }
        // Place sign with line name and entrance name. Signs need to be fixed first
        /*  editor.set_sign(
            node.tags.get("name").map_or("    ", |v| v).to_string(),
            "".to_owned(),
            node.tags.get("ref").map_or("    ", |v| v).to_string(),
            "".to_owned(),
            node.x,
            ground_level + 1,
            node.z + 1,
            0,
        ); */
    } else if node_type == "buffer_stop" {
        // Stop on a rail line
        editor.set_block(
            IRON_BLOCK,
            node.x,
            ground_level + 1,
            node.z,
            Some(&[DIRT, STONE, GRASS_BLOCK]),
            None,
        );
    } else if ["crossing", "level_crossing", "tram_level_crossing"].contains(&node_type) {
        // Crossing points
        editor.set_block(
            GRAY_CONCRETE,
            node.x,
            ground_level,
            node.z,
            Some(&[DIRT, STONE, GRASS_BLOCK, GRAVEL]),
            None,
        );
    } else if node_type == "signal" {
        // Railway signals
        for dy in 1..=3 {
            editor.set_block(COBBLESTONE_WALL, node.x + 1, dy, node.z + 1, None, None);
        }

        editor.set_block(YELLOW_WOOL, node.x + 1, 4, node.z + 1, None, None);
        editor.set_block(RED_WOOL, node.x + 1, 5, node.z + 1, None, None);
    }
}
fn generate_station(editor: &mut WorldEditor<'_>, element: &ProcessedWay) {
    let polygon_coords: Vec<(i32, i32)> = element.nodes.iter().map(|n| (n.x, n.z)).collect();
    let floor_area: Vec<(i32, i32)> = flood_fill_area(&polygon_coords, None);
    let mut ground_level = 0;
    if let Some(level_str) = element.tags.get("layer") {
        if let Ok(level) = level_str.parse::<i32>() {
            ground_level = level * 4;
        }
    }
    for (x, z) in floor_area {
        editor.set_block(
            SMOOTH_STONE,
            x,
            ground_level,
            z,
            Some(&[DIRT, STONE, GRASS_BLOCK]),
            None,
        );
        if ground_level < 0 {
            // Clear air in station if below ground
            for dy in 1..=3 {
                editor.set_block(
                    AIR,
                    x,
                    ground_level + dy,
                    z,
                    Some(&[DIRT, STONE, POLISHED_ANDESITE]),
                    None,
                );
            }
        }
    }
}

fn generate_subway(editor: &mut WorldEditor, element: &ProcessedWay) {
    for i in 1..element.nodes.len() {
        let prev_node = element.nodes[i - 1].xz();
        let cur_node = element.nodes[i].xz();
        let mut ground_level = -4;
        if let Some(level_str) = element.tags.get("layer") {
            let level = level_str.parse::<i32>().unwrap_or(-1);
            ground_level = level * 4;
        }

        let points = bresenham_line(prev_node.x, 0, prev_node.z, cur_node.x, 0, cur_node.z);
        let smoothed_points = smooth_diagonal_rails(&points);

        for j in 0..smoothed_points.len() {
            let (bx, _, bz) = smoothed_points[j];

            // Create a 5-block wide floor
            for dx in -2..=2 {
                for dz in -2..=2 {
                    editor.set_block(
                        SMOOTH_STONE,
                        bx + dx,
                        ground_level,
                        bz + dz,
                        Some(&[DIRT, STONE]),
                        None,
                    );
                }
            }

            // Create a 5-block wide ceiling 3 blocks above the floor
            for dx in -2..=2 {
                for dz in -2..=2 {
                    // Add occasional glowstone for lighting
                    if dx == 0 && dz == 0 && j % 4 == 0 {
                        editor.set_block(
                            GLOWSTONE,
                            bx,
                            ground_level + 4,
                            bz,
                            Some(&[DIRT, STONE, SMOOTH_STONE]),
                            None,
                        );
                    } else {
                        editor.set_block(
                            SMOOTH_STONE,
                            bx + dx,
                            ground_level + 4,
                            bz + dz,
                            Some(&[STONE]),
                            Some(&[GLOWSTONE]),
                        );
                        editor.set_block(
                            AIR,
                            bx + dx,
                            ground_level + 1,
                            bz + dz,
                            Some(&[DIRT, STONE,POLISHED_ANDESITE]),
                            None,
                        );
                        editor.set_block(
                            AIR,
                            bx + dx,
                            ground_level + 2,
                            bz + dz,
                            Some(&[DIRT, STONE,POLISHED_ANDESITE]),
                            None,
                        );
                        editor.set_block(
                            AIR,
                            bx + dx,
                            ground_level + 3,
                            bz + dz,
                            Some(&[DIRT, STONE,POLISHED_ANDESITE]),
                            None,
                        );
                    }
                }
            }

            // Get previous and next points for direction
            let prev = if j > 0 {
                Some(smoothed_points[j - 1])
            } else {
                None
            };
            let next = if j < smoothed_points.len() - 1 {
                Some(smoothed_points[j + 1])
            } else {
                None
            };

            let rail_block = determine_rail_direction(
                (bx, bz),
                prev.map(|(x, _, z)| (x, z)),
                next.map(|(x, _, z)| (x, z)),
            );

            // Place the rail on top of the floor
            editor.set_block(
                rail_block,
                bx,
                ground_level + 1,
                bz,
                Some(&[DIRT, STONE,POLISHED_ANDESITE]),
                None,
            );
            editor.set_block(AIR, bx, ground_level + 2, bz, Some(&[DIRT, STONE]), None);
            editor.set_block(AIR, bx, ground_level + 3, bz, Some(&[DIRT, STONE]), None);

            // Helper function to place wall only if block below is not SMOOTH_STONE
            let mut place_wall = |x: i32, y: i32, z: i32| {
                if !editor.check_for_block(x, y - 1, z, Some(&[SMOOTH_STONE])) {
                    editor.set_block(POLISHED_ANDESITE, x, y, z, Some(&[DIRT, STONE]), None);
                    editor.set_block(POLISHED_ANDESITE, x, y + 1, z, Some(&[DIRT, STONE]), None);
                    editor.set_block(POLISHED_ANDESITE, x, y + 2, z, Some(&[DIRT, STONE]), None);
                    editor.set_block(POLISHED_ANDESITE, x, y + 3, z, Some(&[DIRT, STONE]), None);
                    editor.set_block(POLISHED_ANDESITE, x, y + 4, z, None, None);
                }
            };

            // Place wall blocks two blocks away from the rail
            // Determine orientation based on rail block
            match rail_block {
                RAIL_NORTH_SOUTH => {
                    // For north-south rails, place wall three blocks east and west
                    place_wall(bx + 3, ground_level, bz);
                    place_wall(bx - 3, ground_level, bz);
                }
                RAIL_EAST_WEST => {
                    // For east-west rails, place wall three blocks north and south
                    place_wall(bx, ground_level, bz + 3);
                    place_wall(bx, ground_level, bz - 3);
                }
                RAIL_NORTH_EAST => {
                    // For curves, place wall three blocks away at appropriate positions
                    place_wall(bx - 3, ground_level, bz);
                    place_wall(bx - 2, ground_level, bz + 3);
                }
                RAIL_NORTH_WEST => {
                    place_wall(bx + 3, ground_level, bz);
                    place_wall(bx + 2, ground_level, bz + 3);
                }
                RAIL_SOUTH_EAST => {
                    place_wall(bx - 3, ground_level, bz);
                    place_wall(bx - 2, ground_level, bz - 3);
                }
                RAIL_SOUTH_WEST => {
                    place_wall(bx + 3, ground_level, bz);
                    place_wall(bx + 2, ground_level, bz - 3);
                }
                _ => {
                    // Default for any other rail blocks
                    place_wall(bx + 3, ground_level, bz);
                    place_wall(bx - 3, ground_level, bz);
                }
            }
        }
    }
}

fn smooth_diagonal_rails(points: &[(i32, i32, i32)]) -> Vec<(i32, i32, i32)> {
    let mut smoothed = Vec::new();

    for i in 0..points.len() {
        let current = points[i];
        smoothed.push(current);

        if i + 1 >= points.len() {
            continue;
        }

        let next = points[i + 1];
        let (x1, y1, z1) = current;
        let (x2, _, z2) = next;

        // If points are diagonally adjacent
        if (x2 - x1).abs() == 1 && (z2 - z1).abs() == 1 {
            // Look ahead to determine best intermediate point
            let look_ahead = if i + 2 < points.len() {
                Some(points[i + 2])
            } else {
                None
            };

            // Look behind
            let look_behind = if i > 0 { Some(points[i - 1]) } else { None };

            // Choose intermediate point based on the overall curve direction
            let intermediate = if let Some((prev_x, _, _prev_z)) = look_behind {
                if prev_x == x1 {
                    // Coming from vertical, keep x constant
                    (x1, y1, z2)
                } else {
                    // Coming from horizontal, keep z constant
                    (x2, y1, z1)
                }
            } else if let Some((next_x, _, _next_z)) = look_ahead {
                if next_x == x2 {
                    // Going to vertical, keep x constant
                    (x2, y1, z1)
                } else {
                    // Going to horizontal, keep z constant
                    (x1, y1, z2)
                }
            } else {
                // Default to horizontal first if no context
                (x2, y1, z1)
            };

            smoothed.push(intermediate);
        }
    }

    smoothed
}

fn determine_rail_direction(
    current: (i32, i32),
    prev: Option<(i32, i32)>,
    next: Option<(i32, i32)>,
) -> Block {
    let (x, z) = current;

    match (prev, next) {
        (Some((px, pz)), Some((nx, nz))) => {
            if px == nx {
                RAIL_NORTH_SOUTH
            } else if pz == nz {
                RAIL_EAST_WEST
            } else {
                // Calculate relative movements
                let from_prev = (px - x, pz - z);
                let to_next = (nx - x, nz - z);

                match (from_prev, to_next) {
                    // East to North or North to East
                    ((-1, 0), (0, -1)) | ((0, -1), (-1, 0)) => RAIL_NORTH_WEST,
                    // West to North or North to West
                    ((1, 0), (0, -1)) | ((0, -1), (1, 0)) => RAIL_NORTH_EAST,
                    // East to South or South to East
                    ((-1, 0), (0, 1)) | ((0, 1), (-1, 0)) => RAIL_SOUTH_WEST,
                    // West to South or South to West
                    ((1, 0), (0, 1)) | ((0, 1), (1, 0)) => RAIL_SOUTH_EAST,
                    _ => {
                        if (px - x).abs() > (pz - z).abs() {
                            RAIL_EAST_WEST
                        } else {
                            RAIL_NORTH_SOUTH
                        }
                    }
                }
            }
        }
        (Some((px, pz)), None) | (None, Some((px, pz))) => {
            if px == x {
                RAIL_NORTH_SOUTH
            } else if pz == z {
                RAIL_EAST_WEST
            } else {
                RAIL_NORTH_SOUTH
            }
        }
        (None, None) => RAIL_NORTH_SOUTH,
    }
}
