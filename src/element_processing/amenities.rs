use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZPoint;
use crate::floodfill::flood_fill_area;
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;

pub fn generate_amenities(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
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

    if let Some(amenity_type) = element.tags().get("amenity") {
        let first_node: Option<XZPoint> = element
            .nodes()
            .map(|n: &crate::osm_parser::ProcessedNode| XZPoint::new(n.x, n.z))
            .next();
        match amenity_type.as_str() {
            "waste_disposal" | "waste_basket" => {
                // Place a cauldron for waste disposal or waste basket
                if let Some(pt) = first_node {
                    editor.set_block(CAULDRON, pt.x, 1, pt.z, None, None);
                }
            }
            "vending_machine" | "atm" => {
                if let Some(pt) = first_node {
                    editor.set_block(IRON_BLOCK, pt.x, 1, pt.z, None, None);
                    editor.set_block(IRON_BLOCK, pt.x, 2, pt.z, None, None);
                }
            }
            "bicycle_parking" => {
                let ground_block: Block = OAK_PLANKS;
                let roof_block: Block = STONE_BLOCK_SLAB;

                let polygon_coords: Vec<(i32, i32)> = element
                    .nodes()
                    .map(|n: &crate::osm_parser::ProcessedNode| (n.x, n.z))
                    .collect();

                if polygon_coords.is_empty() {
                    return;
                }

                let floor_area: Vec<(i32, i32)> =
                    flood_fill_area(&polygon_coords, args.timeout.as_ref());

                // Fill the floor area
                for (x, z) in floor_area.iter() {
                    editor.set_block(ground_block, *x, 0, *z, None, None);
                }

                // Place fences and roof slabs at each corner node
                for node in element.nodes() {
                    let x: i32 = node.x;
                    let z: i32 = node.z;

                    // Set ground block and fences
                    editor.set_block(ground_block, x, 0, z, None, None);
                    for y in 1..=4 {
                        editor.set_block(OAK_FENCE, x, y, z, None, None);
                    }
                    editor.set_block(roof_block, x, 5, z, None, None);
                }

                // Flood fill the roof area
                for (x, z) in floor_area.iter() {
                    editor.set_block(roof_block, *x, 5, *z, None, None);
                }
            }
            "bench" => {
                // Place a bench
                if let Some(pt) = first_node {
                    // 50% chance to 90 degrees rotate the bench using if
                    if rand::random::<bool>() {
                        editor.set_block(SMOOTH_STONE, pt.x, 1, pt.z, None, None);
                        editor.set_block(OAK_LOG, pt.x + 1, 1, pt.z, None, None);
                        editor.set_block(OAK_LOG, pt.x - 1, 1, pt.z, None, None);
                    } else {
                        editor.set_block(SMOOTH_STONE, pt.x, 1, pt.z, None, None);
                        editor.set_block(OAK_LOG, pt.x, 1, pt.z + 1, None, None);
                        editor.set_block(OAK_LOG, pt.x, 1, pt.z - 1, None, None);
                    }
                }
            }
            "shelter" => {
                let roof_block: Block = STONE_BRICK_SLAB;

                let polygon_coords: Vec<(i32, i32)> = element
                    .nodes()
                    .map(|n: &crate::osm_parser::ProcessedNode| (n.x, n.z))
                    .collect();
                let roof_area: Vec<(i32, i32)> =
                    flood_fill_area(&polygon_coords, args.timeout.as_ref());

                // Place fences and roof slabs at each corner node directly
                for node in element.nodes() {
                    let x: i32 = node.x;
                    let z: i32 = node.z;

                    for fence_height in 1..=4 {
                        editor.set_block(OAK_FENCE, x, fence_height, z, None, None);
                    }
                    editor.set_block(roof_block, x, 5, z, None, None);
                }

                // Flood fill the roof area
                for (x, z) in roof_area.iter() {
                    editor.set_block(roof_block, *x, 5, *z, None, None);
                }
            }
            "parking" | "fountain" => {
                // Process parking or fountain areas
                let mut previous_node: Option<XZPoint> = None;
                let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
                let mut current_amenity: Vec<(i32, i32)> = vec![];

                let block_type = match amenity_type.as_str() {
                    "fountain" => WATER,
                    "parking" => GRAY_CONCRETE,
                    _ => GRAY_CONCRETE,
                };

                for node in element.nodes() {
                    let pt: XZPoint = node.xz();

                    if let Some(prev) = previous_node {
                        // Create borders for fountain or parking area
                        let bresenham_points: Vec<(i32, i32, i32)> =
                            bresenham_line(prev.x, 0, prev.z, pt.x, 0, pt.z);
                        for (bx, _, bz) in bresenham_points {
                            editor.set_block(block_type, bx, 0, bz, Some(&[BLACK_CONCRETE]), None);

                            // Decorative border around fountains
                            if amenity_type == "fountain" {
                                for dx in [-1, 0, 1].iter() {
                                    for dz in [-1, 0, 1].iter() {
                                        if (*dx, *dz) != (0, 0) {
                                            editor.set_block(
                                                LIGHT_GRAY_CONCRETE,
                                                bx + dx,
                                                0,
                                                bz + dz,
                                                None,
                                                None,
                                            );
                                        }
                                    }
                                }
                            }

                            current_amenity.push((node.x, node.z));
                            corner_addup.0 += node.x;
                            corner_addup.1 += node.z;
                            corner_addup.2 += 1;
                        }
                    }
                    previous_node = Some(pt);
                }

                // Flood-fill the interior area for parking or fountains
                if corner_addup.2 > 0 {
                    let polygon_coords: Vec<(i32, i32)> = current_amenity.to_vec();
                    let flood_area: Vec<(i32, i32)> =
                        flood_fill_area(&polygon_coords, args.timeout.as_ref());

                    for (x, z) in flood_area {
                        editor.set_block(
                            block_type,
                            x,
                            0,
                            z,
                            Some(&[BLACK_CONCRETE, GRAY_CONCRETE]),
                            None,
                        );

                        // Enhanced parking space markings
                        if amenity_type == "parking" {
                            // Create defined parking spaces with realistic layout
                            let space_width = 4; // Width of each parking space
                            let space_length = 6; // Length of each parking space
                            let lane_width = 5; // Width of driving lanes

                            // Calculate which "zone" this coordinate falls into
                            let zone_x = x / space_width;
                            let zone_z = z / (space_length + lane_width);
                            let local_x = x % space_width;
                            let local_z = z % (space_length + lane_width);

                            // Create parking space boundaries (only within parking areas, not in driving lanes)
                            if local_z < space_length {
                                // We're in a parking space area, not in the driving lane
                                if local_x == 0 {
                                    // Vertical parking space lines (only on the left edge)
                                    editor.set_block(
                                        LIGHT_GRAY_CONCRETE,
                                        x,
                                        0,
                                        z,
                                        Some(&[BLACK_CONCRETE, GRAY_CONCRETE]),
                                        None,
                                    );
                                } else if local_z == 0 {
                                    // Horizontal parking space lines (only on the top edge)
                                    editor.set_block(
                                        LIGHT_GRAY_CONCRETE,
                                        x,
                                        0,
                                        z,
                                        Some(&[BLACK_CONCRETE, GRAY_CONCRETE]),
                                        None,
                                    );
                                }
                            } else if local_z == space_length {
                                // Bottom edge of parking spaces (border with driving lane)
                                editor.set_block(
                                    LIGHT_GRAY_CONCRETE,
                                    x,
                                    0,
                                    z,
                                    Some(&[BLACK_CONCRETE, GRAY_CONCRETE]),
                                    None,
                                );
                            } else if local_z > space_length && local_z < space_length + lane_width
                            {
                                // Driving lane - use darker concrete
                                editor.set_block(
                                    BLACK_CONCRETE,
                                    x,
                                    0,
                                    z,
                                    Some(&[GRAY_CONCRETE]),
                                    None,
                                );
                            }

                            // Add light posts at parking space outline corners
                            if local_x == 0 && local_z == 0 && zone_x % 3 == 0 && zone_z % 2 == 0 {
                                // Light posts at regular intervals on parking space corners
                                editor.set_block(COBBLESTONE_WALL, x, 1, z, None, None);
                                for dy in 2..=4 {
                                    editor.set_block(OAK_FENCE, x, dy, z, None, None);
                                }
                                editor.set_block(GLOWSTONE, x, 5, z, None, None);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
