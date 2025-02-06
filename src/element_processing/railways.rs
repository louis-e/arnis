use crate::block_definitions::{BLOCKS, Block};
use crate::bresenham::bresenham_line;
use crate::cartesian::XZPoint;
use crate::ground::Ground;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

pub fn generate_railways(editor: &mut WorldEditor, element: &ProcessedWay, ground: &Ground) {
    if let Some(railway_type) = element.tags.get("railway") {
        if [
            "proposed",
            "abandoned",
            "subway",
            "construction",
            "razed",
            "turntable",
        ]
        .contains(&railway_type.as_str())
        {
            return;
        }

        if let Some(subway) = element.tags.get("subway") {
            if subway == "yes" {
                return;
            }
        }

        if let Some(tunnel) = element.tags.get("tunnel") {
            if tunnel == "yes" {
                return;
            }
        }

        for i in 1..element.nodes.len() {
            let prev_node = element.nodes[i - 1].xz();
            let cur_node = element.nodes[i].xz();

            let points = bresenham_line(prev_node.x, 0, prev_node.z, cur_node.x, 0, cur_node.z);
            let smoothed_points = smooth_diagonal_rails(&points);

            for j in 0..smoothed_points.len() {
                let (bx, _, bz) = smoothed_points[j];
                let ground_level = ground.level(XZPoint::new(bx, bz));

                editor.set_block(&*BLOCKS.by_name("iron_block").unwrap(), bx, ground_level, bz, None, None);

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

                editor.set_block(rail_block, bx, ground_level + 1, bz, None, None);

                if bx % 4 == 0 {
                    editor.set_block(&*BLOCKS.by_name("oak_log").unwrap(), bx, ground_level, bz, None, None);
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
) -> &'static Block {
    let (x, z) = current;

    match (prev, next) {
        (Some((px, pz)), Some((nx, nz))) => {
            if px == nx {
                &*BLOCKS.by_name("rail_north_south").unwrap()
            } else if pz == nz {
                &*BLOCKS.by_name("rail_east_west").unwrap()
            } else {
                // Calculate relative movements
                let from_prev = (px - x, pz - z);
                let to_next = (nx - x, nz - z);

                match (from_prev, to_next) {
                    // East to North or North to East
                    ((-1, 0), (0, -1)) | ((0, -1), (-1, 0)) => &*BLOCKS.by_name("rail_north_west").unwrap(),
                    // West to North or North to West
                    ((1, 0), (0, -1)) | ((0, -1), (1, 0)) => &*BLOCKS.by_name("rail_north_east").unwrap(),
                    // East to South or South to East
                    ((-1, 0), (0, 1)) | ((0, 1), (-1, 0)) => &*BLOCKS.by_name("rail_south_west").unwrap(),
                    // West to South or South to West
                    ((1, 0), (0, 1)) | ((0, 1), (1, 0)) => &*BLOCKS.by_name("rail_south_east").unwrap(),
                    _ => {
                        if (px - x).abs() > (pz - z).abs() {
                            &*BLOCKS.by_name("rail_east_west").unwrap()
                        } else {
                            &*BLOCKS.by_name("rail_north_south").unwrap()
                        }
                    }
                }
            }
        }
        (Some((px, pz)), None) | (None, Some((px, pz))) => {
            if px == x {
                &*BLOCKS.by_name("rail_north_south").unwrap()
            } else if pz == z {
                &*BLOCKS.by_name("rail_east_west").unwrap()
            } else {
                &*BLOCKS.by_name("rail_north_south").unwrap()
            }
        }
        (None, None) => &*BLOCKS.by_name("rail_north_south").unwrap(),
    }
}
