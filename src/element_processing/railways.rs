use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

/// Number of blocks per OSM layer level (matches highway elevation step).
const LAYER_HEIGHT_STEP: i32 = 6;

/// Vertical offset in blocks from the terrain surface to the tunnel ceiling.
const SUBWAY_DEPTH: i32 = 3;

/// Half-width of the outer stone shell (total footprint = 2 * WALL_RADIUS + 1 = 5).
const WALL_RADIUS: i32 = 2;

/// Half-width of the interior air space (total air width = 2 * AIR_RADIUS + 1 = 3).
const AIR_RADIUS: i32 = 1;

/// Number of interior Y-levels (rail + 3 air = 4 blocks for minecart clearance).
const INTERIOR_HEIGHT: i32 = 4;

/// Interval in centerline points between ceiling lights.
const LIGHT_INTERVAL: usize = 8;

/// Deterministic spatial hash for tunnel wall/ceiling block variety.
/// Returns CRACKED_STONE_BRICKS (~15%), MOSSY_STONE_BRICKS (~3%),
/// or STONE_BRICKS (~82%).
fn subway_shell_block(x: i32, y: i32, z: i32) -> Block {
    let h = (x as u32)
        .wrapping_mul(73856093)
        .wrapping_add((y as u32).wrapping_mul(19349663))
        .wrapping_add((z as u32).wrapping_mul(83492791));
    let v = h % 100;
    if v < 15 {
        CRACKED_STONE_BRICKS
    } else if v < 18 {
        MOSSY_STONE_BRICKS
    } else {
        STONE_BRICKS
    }
}

pub fn generate_railways(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    subway_points: &mut Vec<(i32, i32)>,
) {
    if let Some(railway_type) = element.tags.get("railway") {
        // Subway lines get their own two-phase generation pipeline.
        let is_subway = railway_type == "subway"
            || element
                .tags
                .get("subway")
                .map(|v| v == "yes")
                .unwrap_or(false);
        if is_subway {
            generate_subway_shell(editor, element, subway_points);
            return;
        }

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

        if let Some(tunnel) = element.tags.get("tunnel") {
            if tunnel == "yes" {
                return;
            }
        }

        // Respect the OSM `layer` tag so elevated railways (bridges, overpasses)
        // are placed above ground level instead of clipping through it.
        let layer_value: i32 = element
            .tags
            .get("layer")
            .and_then(|l| l.parse::<i32>().ok())
            .unwrap_or(0)
            .max(0); // underground (<0) is handled by the tunnel check above

        let layer_offset = layer_value * LAYER_HEIGHT_STEP;

        for i in 1..element.nodes.len() {
            let prev_node = element.nodes[i - 1].xz();
            let cur_node = element.nodes[i].xz();

            let points = bresenham_line(prev_node.x, 0, prev_node.z, cur_node.x, 0, cur_node.z);
            let smoothed_points = smooth_diagonal_rails(&points);

            for j in 0..smoothed_points.len() {
                let (bx, _, bz) = smoothed_points[j];

                // Base offsets from ground level.
                // When layer_offset > 0 the rail floats above terrain (bridge/overpass).
                let gravel_y = layer_offset;
                let rail_y = layer_offset + 1;

                // --- Terrain-slope detection (only for at-grade railways) ---
                // When terrain is enabled and layer == 0 we try to place ascending
                // rail variants so consecutive blocks stay visually connected even
                // when the ground rises or falls by one block per step.
                let prev_ground = if j > 0 {
                    let (px, _, pz) = smoothed_points[j - 1];
                    editor.get_ground_level(px, pz)
                } else {
                    editor.get_ground_level(bx, bz)
                };

                let next_ground = if j + 1 < smoothed_points.len() {
                    let (nx, _, nz) = smoothed_points[j + 1];
                    editor.get_ground_level(nx, nz)
                } else {
                    editor.get_ground_level(bx, bz)
                };

                let current_ground = editor.get_ground_level(bx, bz);

                // Fill the vertical gap under the rail when terrain rises steeply
                // so there is always a solid gravel block supporting the track.
                if layer_offset == 0 && prev_ground < current_ground {
                    for fill_y in prev_ground..current_ground {
                        editor.set_block_absolute(GRAVEL, bx, fill_y, bz, None, None);
                    }
                }

                editor.set_block(GRAVEL, bx, gravel_y, bz, None, None);

                let prev_xz = if j > 0 {
                    let (px, _, pz) = smoothed_points[j - 1];
                    Some((px, pz))
                } else {
                    None
                };
                let next_xz = if j + 1 < smoothed_points.len() {
                    let (nx, _, nz) = smoothed_points[j + 1];
                    Some((nx, nz))
                } else {
                    None
                };

                let rail_block = if layer_offset == 0 {
                    determine_rail_with_slope(
                        (bx, bz),
                        prev_xz,
                        next_xz,
                        prev_ground,
                        current_ground,
                        next_ground,
                    )
                } else {
                    determine_rail_direction((bx, bz), prev_xz, next_xz)
                };

                editor.set_block(rail_block, bx, rail_y, bz, None, None);

                if bx % 4 == 0 {
                    editor.set_block(OAK_LOG, bx, gravel_y, bz, None, None);
                }
            }
        }
    }
}

/// Choose between a flat or ascending rail block based on the ground-level
/// difference between the previous, current, and next track points.
fn determine_rail_with_slope(
    current: (i32, i32),
    prev: Option<(i32, i32)>,
    next: Option<(i32, i32)>,
    prev_ground: i32,
    current_ground: i32,
    next_ground: i32,
) -> Block {
    // Ascending toward the *higher* neighbour.
    if next_ground > current_ground {
        if let Some((nx, nz)) = next {
            return ascending_toward(current, (nx, nz));
        }
    }
    if prev_ground > current_ground {
        if let Some((px, pz)) = prev {
            return ascending_toward(current, (px, pz));
        }
    }
    // Flat section – fall back to standard direction logic.
    determine_rail_direction(current, prev, next)
}

/// Return the ascending rail variant that climbs from `from` toward `to`.
fn ascending_toward(from: (i32, i32), to: (i32, i32)) -> Block {
    let (fx, fz) = from;
    let (tx, tz) = to;
    let dx = tx - fx;
    let dz = tz - fz;
    if dx.abs() >= dz.abs() {
        if dx > 0 {
            RAIL_ASCENDING_EAST
        } else {
            RAIL_ASCENDING_WEST
        }
    } else if dz < 0 {
        RAIL_ASCENDING_NORTH
    } else {
        RAIL_ASCENDING_SOUTH
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

/// Phase 1 of subway generation: place the structural tunnel shell and rail
/// track.  Called during element processing (step 4) so that all non-AIR
/// blocks survive the underground stone fill in step 6.
///
/// Centerline points are collected into `subway_points` for phase 2.
fn generate_subway_shell(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    subway_points: &mut Vec<(i32, i32)>,
) {
    for i in 1..element.nodes.len() {
        let prev_node = element.nodes[i - 1].xz();
        let cur_node = element.nodes[i].xz();

        let points = bresenham_line(prev_node.x, 0, prev_node.z, cur_node.x, 0, cur_node.z);
        let smoothed = smooth_diagonal_rails(&points);

        for j in 0..smoothed.len() {
            let (bx, _, bz) = smoothed[j];

            // Record centerline point for phase 2 air-carving, skipping
            // duplicate shared endpoints between adjacent segments.
            if subway_points.last().copied() != Some((bx, bz)) {
                subway_points.push((bx, bz));
            }

            let ground_y = editor.get_ground_level(bx, bz);
            let ceil_y = ground_y - SUBWAY_DEPTH;
            let floor_y = ceil_y - INTERIOR_HEIGHT - 1;

            // Safety: skip if the tunnel would go below world minimum.
            if floor_y <= crate::world_editor::MIN_Y {
                continue;
            }

            // Ground levels at adjacent points, used for slope-aware rail
            // placement. Because the tunnel depth is fixed, surface-level
            // differences map 1:1 to rail-level differences.
            let prev_ground = if j > 0 {
                let (px, _, pz) = smoothed[j - 1];
                editor.get_ground_level(px, pz)
            } else {
                ground_y
            };
            let next_ground = if j + 1 < smoothed.len() {
                let (nx, _, nz) = smoothed[j + 1];
                editor.get_ground_level(nx, nz)
            } else {
                ground_y
            };

            // Place tunnel shell (5x5 footprint, full height).
            // Interior positions deliberately get non-AIR blocks too so that
            // the ground fill (skip_existing: true) leaves them alone.
            // Wall/ceiling blocks get random cracked/mossy variants for variety.
            for dx in -WALL_RADIUS..=WALL_RADIUS {
                for dz in -WALL_RADIUS..=WALL_RADIUS {
                    for y in floor_y..=ceil_y {
                        let is_wall_or_ceiling =
                            dx.abs() == WALL_RADIUS || dz.abs() == WALL_RADIUS || y == ceil_y;

                        let block = if y == floor_y {
                            // Entire floor row: polished deepslate
                            POLISHED_DEEPSLATE
                        } else if is_wall_or_ceiling {
                            // Visible wall/ceiling: mix in cracked and mossy
                            subway_shell_block(bx + dx, y, bz + dz)
                        } else {
                            // Interior placeholder (carved in phase 2)
                            STONE_BRICKS
                        };
                        editor.set_block_absolute(block, bx + dx, y, bz + dz, None, None);
                    }
                }
            }

            // Place rail on the structural floor (one above floor_y).
            let prev_xz = if j > 0 {
                let (px, _, pz) = smoothed[j - 1];
                Some((px, pz))
            } else {
                None
            };
            let next_xz = if j + 1 < smoothed.len() {
                let (nx, _, nz) = smoothed[j + 1];
                Some((nx, nz))
            } else {
                None
            };

            let rail_block = determine_rail_with_slope(
                (bx, bz),
                prev_xz,
                next_xz,
                prev_ground,
                ground_y,
                next_ground,
            );
            // Whitelist: allow overwriting the STONE_BRICKS placeholder.
            editor.set_block_absolute(
                rail_block,
                bx,
                floor_y + 1,
                bz,
                Some(&[STONE_BRICKS, CRACKED_STONE_BRICKS, MOSSY_STONE_BRICKS]),
                None,
            );
        }
    }
}

/// Phase 2 of subway generation: carve the 3x3 air interior and place
/// ceiling lights.  Called AFTER ground generation so that the carved
/// air blocks are not overwritten by the underground stone fill.
pub fn carve_subway_interior(editor: &mut WorldEditor, subway_points: &[(i32, i32)]) {
    for (idx, &(bx, bz)) in subway_points.iter().enumerate() {
        let ground_y = editor.get_ground_level(bx, bz);
        let ceil_y = ground_y - SUBWAY_DEPTH;
        let floor_y = ceil_y - INTERIOR_HEIGHT - 1;

        if floor_y <= crate::world_editor::MIN_Y {
            continue;
        }

        // Whitelist: allow overwriting shell blocks and ground-fill STONE
        // so the tunnel is actually hollow.
        let carve_whitelist: &[Block] = &[
            STONE_BRICKS,
            CRACKED_STONE_BRICKS,
            MOSSY_STONE_BRICKS,
            STONE,
        ];
        for dx in -AIR_RADIUS..=AIR_RADIUS {
            for dz in -AIR_RADIUS..=AIR_RADIUS {
                for y in (floor_y + 1)..ceil_y {
                    // Skip the center rail block.
                    if dx == 0 && dz == 0 && y == floor_y + 1 {
                        continue;
                    }
                    editor.set_block_absolute(
                        AIR,
                        bx + dx,
                        y,
                        bz + dz,
                        Some(carve_whitelist),
                        None,
                    );
                }
            }
        }

        // Periodic ceiling lighting.
        if idx % LIGHT_INTERVAL == 0 {
            editor.set_block_absolute(SEA_LANTERN, bx, ceil_y - 1, bz, None, None);
        }
    }
}
