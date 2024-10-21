use std::time::Duration;

use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::floodfill::flood_fill_area;
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;

pub fn generate_amenities(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    ground_level: i32,
    floodfill_timeout: Option<&Duration>,
) {
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
        let first_node = element.nodes().map(|n| (n.x, n.z)).next();
        match amenity_type.as_str() {
            "waste_disposal" | "waste_basket" => {
                // Place a cauldron for waste disposal or waste basket
                if let Some((x, z)) = first_node {
                    editor.set_block(&CAULDRON, x, ground_level + 1, z, None, None);
                }
            }
            "vending_machine" | "atm" => {
                if let Some((x, z)) = first_node {
                    editor.set_block(&IRON_BLOCK, x, ground_level + 1, z, None, None);
                    editor.set_block(&IRON_BLOCK, x, ground_level + 2, z, None, None);
                }
            }
            "bicycle_parking" => {
                let ground_block: &once_cell::sync::Lazy<Block> = &OAK_PLANKS;
                let roof_block: &once_cell::sync::Lazy<Block> = &STONE_BLOCK_SLAB;

                let polygon_coords: Vec<(i32, i32)> = element.nodes().map(|n| (n.x, n.z)).collect();
                let floor_area: Vec<(i32, i32)> =
                    flood_fill_area(&polygon_coords, floodfill_timeout);

                // Fill the floor area
                for (x, z) in floor_area.iter() {
                    editor.set_block(ground_block, *x, ground_level, *z, None, None);
                }

                // Place fences and roof slabs at each corner node directly
                for node in element.nodes() {
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
            }
            "bench" => {
                // Place a bench
                if let Some((x, z)) = first_node {
                    editor.set_block(&SMOOTH_STONE, x, ground_level + 1, z, None, None);
                    editor.set_block(&OAK_LOG, x + 1, ground_level + 1, z, None, None);
                    editor.set_block(&OAK_LOG, x - 1, ground_level + 1, z, None, None);
                }
            }
            "vending" => {
                // Place vending machine blocks
                if let Some((x, z)) = first_node {
                    editor.set_block(&IRON_BLOCK, x, ground_level + 1, z, None, None);
                    editor.set_block(&IRON_BLOCK, x, ground_level + 2, z, None, None);
                }
            }
            "parking" | "fountain" => {
                // Process parking or fountain areas
                let mut previous_node: Option<(i32, i32)> = None;
                let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
                let mut current_amenity: Vec<(i32, i32)> = vec![];

                let block_type: &once_cell::sync::Lazy<Block> = match amenity_type.as_str() {
                    "fountain" => &WATER,
                    "parking" => &GRAY_CONCRETE,
                    _ => &GRAY_CONCRETE,
                };
                for node in element.nodes() {
                    let x = node.x;
                    let z = node.z;

                    if let Some(prev) = previous_node {
                        // Create borders for fountain or parking area
                        let bresenham_points: Vec<(i32, i32, i32)> =
                            bresenham_line(prev.0, ground_level, prev.1, x, ground_level, z);
                        for (bx, _, bz) in bresenham_points {
                            editor.set_block(
                                block_type,
                                bx,
                                ground_level,
                                bz,
                                Some(&[&BLACK_CONCRETE]),
                                None,
                            );

                            // Decorative border around fountains
                            if amenity_type == "fountain" {
                                for dx in [-1, 0, 1].iter() {
                                    for dz in [-1, 0, 1].iter() {
                                        if (*dx, *dz) != (0, 0) {
                                            editor.set_block(
                                                &LIGHT_GRAY_CONCRETE,
                                                bx + dx,
                                                ground_level,
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
                    previous_node = Some((x, z));
                }

                // Flood-fill the interior area for parking or fountains
                if corner_addup.2 > 0 {
                    let polygon_coords: Vec<(i32, i32)> = current_amenity.to_vec();
                    let flood_area: Vec<(i32, i32)> =
                        flood_fill_area(&polygon_coords, floodfill_timeout);

                    for (x, z) in flood_area {
                        editor.set_block(
                            block_type,
                            x,
                            ground_level,
                            z,
                            Some(&[&BLACK_CONCRETE, &GRAY_CONCRETE]),
                            None,
                        );

                        // Add parking spot markings
                        if amenity_type == "parking" && (x + z) % 8 == 0 && (x * z) % 32 != 0 {
                            editor.set_block(
                                &LIGHT_GRAY_CONCRETE,
                                x,
                                ground_level,
                                z,
                                Some(&[&BLACK_CONCRETE, &GRAY_CONCRETE]),
                                None,
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
