use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

pub fn generate_railways(editor: &mut WorldEditor, element: &ProcessedWay) {
    if let Some(railway_type) = element.tags.get("railway") {
        // Check for underground railways (negative level or layer)
        let is_underground = element
            .tags
            .get("level")
            .map_or(false, |level| level.parse::<i32>().map_or(false, |l| l < 0))
            || element
                .tags
                .get("layer")
                .map_or(false, |layer| layer.parse::<i32>().map_or(false, |l| l < 0));

        // Also check for tunnel=yes tag
        let is_tunnel = element
            .tags
            .get("tunnel")
            .map_or(false, |tunnel| tunnel == "yes");

        // Skip certain railway types
        if [
            "proposed",
            "abandoned",
            "construction",
            "razed",
            "turntable",
        ]
        .contains(&railway_type.as_str())
        {
            return;
        }

        // Process as subway if it's a subway, underground by level/layer tag, or a tunnel
        if element.tags.get("subway").map_or(false, |v| v == "yes") || is_underground || is_tunnel {
            generate_subway(editor, element);
            return;
        }

        // Regular surface railways
        for i in 1..element.nodes.len() {
            let prev_node = element.nodes[i - 1].xz();
            let cur_node = element.nodes[i].xz();

            let points = bresenham_line(prev_node.x, 0, prev_node.z, cur_node.x, 0, cur_node.z);
            let smoothed_points = smooth_diagonal_rails(&points);

            for j in 0..smoothed_points.len() {
                let (bx, _, bz) = smoothed_points[j];

                editor.set_block(GRAVEL, bx, 0, bz, None, None);

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

                editor.set_block(rail_block, bx, 1, bz, None, None);

                if bx % 4 == 0 {
                    editor.set_block(OAK_LOG, bx, 0, bz, None, None);
                }
            }
        }
    }
}

fn generate_subway(editor: &mut WorldEditor, element: &ProcessedWay) {
    for i in 1..element.nodes.len() {
        let prev_node = element.nodes[i - 1].xz();
        let cur_node = element.nodes[i].xz();

        let points = bresenham_line(prev_node.x, 0, prev_node.z, cur_node.x, 0, cur_node.z);
        let smoothed_points = smooth_diagonal_rails(&points);

        for j in 0..smoothed_points.len() {
            let (bx, _, bz) = smoothed_points[j];

            // Create a 4-block wide floor
            for dx in -2..=1 {
                for dz in -2..=1 {
                    editor.set_block(SMOOTH_STONE, bx + dx, -9, bz + dz, None, None);
                }
            }

            // Create a 4-block wide ceiling 3 blocks above the floor
            for dx in -2..=1 {
                for dz in -2..=1 {
                    // Add occasional glowstone for lighting
                    if dx == 0 && dz == 0 && j % 4 == 0 {
                        editor.set_block(GLOWSTONE, bx, -4, bz, None, None);
                    } else {
                        editor.set_block(
                            SMOOTH_STONE,
                            bx + dx,
                            -4,
                            bz + dz,
                            None,
                            Some(&[GLOWSTONE]),
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
            editor.set_block(rail_block, bx, -8, bz, None, None);

            // Helper function to place wall only if block below is not SMOOTH_STONE
            let mut place_wall = |x: i32, y: i32, z: i32| {
                if !editor.check_for_block(x, y - 1, z, Some(&[SMOOTH_STONE])) {
                    editor.set_block(POLISHED_ANDESITE, x, y, z, None, None);
                    editor.set_block(POLISHED_ANDESITE, x, y + 1, z, None, None);
                    editor.set_block(POLISHED_ANDESITE, x, y + 2, z, None, None);
                    editor.set_block(POLISHED_ANDESITE, x, y + 3, z, None, None);
                    editor.set_block(POLISHED_ANDESITE, x, y + 4, z, None, None);
                }
            };

            // Place dirt blocks two blocks away from the rail
            // Determine orientation based on rail block
            match rail_block {
                RAIL_NORTH_SOUTH => {
                    // For north-south rails, place dirt two blocks east and west
                    place_wall(bx + 3, -8, bz);
                    place_wall(bx - 3, -8, bz);
                }
                RAIL_EAST_WEST => {
                    // For east-west rails, place dirt two blocks north and south
                    place_wall(bx, -8, bz + 3);
                    place_wall(bx, -8, bz - 3);
                }
                RAIL_NORTH_EAST => {
                    // For curves, place dirt two blocks away at appropriate positions
                    place_wall(bx - 3, -8, bz);
                    place_wall(bx, -8, bz + 3);
                }
                RAIL_NORTH_WEST => {
                    place_wall(bx + 3, -8, bz);
                    place_wall(bx, -8, bz + 3);
                }
                RAIL_SOUTH_EAST => {
                    place_wall(bx - 3, -8, bz);
                    place_wall(bx, -8, bz - 3);
                }
                RAIL_SOUTH_WEST => {
                    place_wall(bx + 3, -8, bz);
                    place_wall(bx, -8, bz - 3);
                }
                _ => {
                    // Default for any other rail blocks
                    place_wall(bx + 3, -8, bz);
                    place_wall(bx - 3, -8, bz);
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

pub fn generate_roller_coaster(editor: &mut WorldEditor, element: &ProcessedWay) {
    if let Some(roller_coaster) = element.tags.get("roller_coaster") {
        if roller_coaster == "track" {
            // Check if it's indoor (skip if yes)
            if let Some(indoor) = element.tags.get("indoor") {
                if indoor == "yes" {
                    return;
                }
            }

            // Check if layer is negative (skip if yes)
            if let Some(layer) = element.tags.get("layer") {
                if let Ok(layer_value) = layer.parse::<i32>() {
                    if layer_value < 0 {
                        return;
                    }
                }
            }

            let elevation_height = 4; // 4 blocks in the air
            let pillar_interval = 6; // Support pillars every 6 blocks

            for i in 1..element.nodes.len() {
                let prev_node = element.nodes[i - 1].xz();
                let cur_node = element.nodes[i].xz();

                let points = bresenham_line(prev_node.x, 0, prev_node.z, cur_node.x, 0, cur_node.z);
                let smoothed_points = smooth_diagonal_rails(&points);

                for j in 0..smoothed_points.len() {
                    let (bx, _, bz) = smoothed_points[j];

                    // Place track foundation at elevation height
                    editor.set_block(IRON_BLOCK, bx, elevation_height, bz, None, None);

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

                    // Place rail on top of the foundation
                    editor.set_block(rail_block, bx, elevation_height + 1, bz, None, None);

                    // Place support pillars every pillar_interval blocks
                    if bx % pillar_interval == 0 && bz % pillar_interval == 0 {
                        // Create a pillar from ground level up to the track
                        for y in 1..elevation_height {
                            editor.set_block(IRON_BLOCK, bx, y, bz, None, None);
                        }
                    }
                }
            }
        }
    }
}
