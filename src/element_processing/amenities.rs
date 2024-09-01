use crate::world_editor::WorldEditor;
use crate::osm_parser::ProcessedElement;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::floodfill::flood_fill_area;

pub fn generate_amenities(editor: &mut WorldEditor, element: &ProcessedElement, ground_level: i32) {
    if let Some(amenity_type) = element.tags.get("amenity") {
        let first_node: Option<&(i32, i32)> = element.nodes.first();
        match amenity_type.as_str() {
            "waste_disposal" | "waste_basket" => {
                // Place a cauldron for waste disposal or waste basket
                if let Some(&(x, z)) = first_node {
                    editor.set_block(&CAULDRON, x, ground_level + 1, z);
                }
                return;
            }
            "bicycle_parking" => {
                let ground_block: &once_cell::sync::Lazy<Block> = &OAK_PLANKS;
                let roof_block: &once_cell::sync::Lazy<Block> = &STONE_BLOCK_SLAB;
        
                let polygon_coords: Vec<(i32, i32)> = element.nodes.iter().copied().collect();
                let floor_area: Vec<(i32, i32)> = flood_fill_area(&polygon_coords, 2);
        
                // Fill the floor area
                for (x, z) in floor_area.iter() {
                    editor.set_block(ground_block, *x, ground_level, *z);
                }
        
                // Place fences and roof slabs at each corner node directly
                for &(x, z) in &element.nodes {
                    for y in 1..=4 {
                        editor.set_block(ground_block, x, ground_level, z);
                        editor.set_block(&OAK_FENCE, x, ground_level + y, z);
                    }
                    editor.set_block(roof_block, x, ground_level + 5, z);
                }
        
                // Flood fill the roof area
                let roof_height: i32 = ground_level + 5;
                for (x, z) in floor_area.iter() {
                    editor.set_block(roof_block, *x, roof_height, *z);
                }
            }
            "bench" => {
                // Place a bench
                if let Some(&(x, z)) = first_node {
                    editor.set_block(&SMOOTH_STONE, x, ground_level + 1, z);
                    editor.set_block(&OAK_LOG, x + 1, ground_level + 1, z);
                    editor.set_block(&OAK_LOG, x - 1, ground_level + 1, z);
                }
            }
            "vending" => {
                // Place vending machine blocks
                if let Some(&(x, z)) = first_node {
                    editor.set_block(&IRON_BLOCK, x, ground_level + 1, z);
                    editor.set_block(&IRON_BLOCK, x, ground_level + 2, z);
                }
            }
            "parking" | "fountain" => {
                // Process parking or fountain areas
                let mut previous_node: Option<(i32, i32)> = None;  // Explicitly annotated type
                let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
                let mut current_amenity: Vec<(i32, i32)> = vec![];

                let block_type: &once_cell::sync::Lazy<Block> = match amenity_type.as_str() {
                    "fountain" => &WATER,
                    "parking" => &GRAY_CONCRETE,
                    _ => &GRAY_CONCRETE, // Default type if needed
                };

                for &node in &element.nodes {
                    if let Some(prev) = previous_node {
                        // Create borders for fountain or parking area
                        let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(prev.0, ground_level, prev.1, node.0, ground_level, node.1);
                        for (bx, _, bz) in bresenham_points {
                            editor.set_block(block_type, bx, ground_level, bz);

                            // Decorative border around fountains
                            if amenity_type == "fountain" {
                                for dx in [-1, 0, 1].iter() {
                                    for dz in [-1, 0, 1].iter() {
                                        if (*dx, *dz) != (0, 0) {
                                            editor.set_block(&LIGHT_GRAY_CONCRETE, bx + dx, ground_level, bz + dz);
                                        }
                                    }
                                }
                            }

                            current_amenity.push((node.0, node.1));
                            corner_addup.0 += node.0;
                            corner_addup.1 += node.1;
                            corner_addup.2 += 1;
                        }
                    }
                    previous_node = Some(node);
                }

                // Flood-fill the interior area for parking or fountains
                if corner_addup.2 > 0 {
                    let polygon_coords: Vec<(i32, i32)> = current_amenity.iter().copied().collect();
                    let flood_area: Vec<(i32, i32)> = flood_fill_area(&polygon_coords, 2);

                    for (x, z) in flood_area {
                        editor.set_block(block_type, x, ground_level, z);

                        // Add parking spot markings
                        if amenity_type == "parking" && (x + z) % 8 == 0 && (x * z) % 32 != 0 {
                            editor.set_block(&LIGHT_GRAY_CONCRETE, x, ground_level, z);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
