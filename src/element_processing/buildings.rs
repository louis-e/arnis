use crate::world_editor::WorldEditor;
use crate::osm_parser::ProcessedElement;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::floodfill::flood_fill_area;
use rand::Rng;
use std::collections::HashSet;

pub fn generate_buildings(editor: &mut WorldEditor, element: &ProcessedElement, ground_level: i32) {
    let mut previous_node: Option<(i32, i32)> = None;
    let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
    let mut current_building: Vec<(i32, i32)> = vec![];

    // Randomly select block variations for corners, walls, and floors
    let mut rng: rand::prelude::ThreadRng = rand::thread_rng();
    //let variation_index = 0;
    let variation_index: usize = rng.gen_range(0..building_corner_variations().len());

    let corner_block: &&once_cell::sync::Lazy<Block> = &building_corner_variations()[variation_index];
    let wall_block: &&once_cell::sync::Lazy<Block> = &building_wall_variations()[variation_index];
    let floor_block: &&once_cell::sync::Lazy<Block> = &building_floor_variations()[variation_index];

    // Set to store processed flood fill points
    let mut processed_points: HashSet<(i32, i32)> = HashSet::new();
    let mut building_height: i32 = 4; // Default building height

    // Determine building height from tags
    if let Some(height_str) = element.tags.get("height") {
        if let Ok(height) = height_str.parse::<i32>() {
            building_height = height;
        }
    }

    if let Some(levels_str) = element.tags.get("building:levels") {
        if let Ok(levels) = levels_str.parse::<i32>() {
            if levels >= 1 && (levels * 3) > building_height {
                building_height = levels * 3;
            }
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
        
                return;
            }
        } else if building_type == "roof" {
            let roof_height = ground_level + 5;
        
            // Iterate through the nodes to create the roof edges using Bresenham's line algorithm
            for &node in &element.nodes {
                let (x, z) = node;
        
                if let Some(prev) = previous_node {
                    let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(prev.0, roof_height, prev.1, x, roof_height, z);
                    for (bx, _, bz) in bresenham_points {
                        editor.set_block(&STONE_BRICK_SLAB, bx, roof_height, bz);  // Set roof block at edge
                    }
                }
        
                previous_node = Some(node);
            }
        
            // Use flood-fill to fill the interior of the roof
            let polygon_coords: Vec<(i32, i32)> = element.nodes.iter().copied().collect();
            let roof_area: Vec<(i32, i32)> = flood_fill_area(&polygon_coords, 2);  // Use flood-fill to determine the area
        
            // Fill the interior of the roof with STONE_BRICK_SLAB
            for (x, z) in roof_area.iter() {
                editor.set_block(&STONE_BRICK_SLAB, *x, roof_height, *z);  // Set roof block
            }
        
            return;
        }
    }

    // Process nodes to create walls and corners
    for &node in &element.nodes {
        let (x, z) = node;

        if let Some(prev) = previous_node {
            // Calculate walls and corners using Bresenham line
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(prev.0, ground_level, prev.1, x, ground_level, z);
            for (bx, _, bz) in bresenham_points {
                for h in (ground_level + 1)..=(ground_level + building_height) {
                    if (bx, bz) == element.nodes[0] {
                        editor.set_block(corner_block, bx, h, bz); // Corner block
                    } else {
                        editor.set_block(corner_block, bx, h, bz); // Wall block
                    }
                }
                editor.set_block(corner_block, bx, ground_level + building_height + 1, bz); // Ceiling cobblestone
                current_building.push((bx, bz));
                corner_addup = (corner_addup.0 + bx, corner_addup.1 + bz, corner_addup.2 + 1);
            }
        }

        previous_node = Some(node);
    }

    // Flood-fill interior with floor variation
    if corner_addup != (0, 0, 0) {
        let polygon_coords: Vec<(i32, i32)> = element.nodes.iter().copied().collect();
        let floor_area: Vec<(i32, i32)> = flood_fill_area(&polygon_coords, 2);
        if element.id == 905796139 {
            println!("CHECKPOINT START");
        }

        for (x, z) in floor_area {
            if processed_points.insert((x, z)) {
                editor.set_block(corner_block, x, ground_level, z); // Set floor
                //editor.set_block(floor_block, x, ground_level + building_height + 2, z); // Set floor
                //editor.set_block(&SPONGE, x, ground_level, z); // Set floor
                if (element.id == 905796139) {
                    //println!("G{} B{} /tp {} {} {}", ground_level, building_height, x, h, z);
                }

                // Set level ceilings if height > 4
                if building_height > 4 {
                    for h in (ground_level + 4..ground_level + building_height).step_by(4) {                        
                        if x % 6 == 0 && z % 6 == 0 {
                            if (element.id == 905796139) {
                                //println!("/setblock {} {} {} glowstone", x, h, z);
                            }
                            editor.set_block(corner_block, x, h, z); // Light fixtures
                        } else {
                            editor.set_block(corner_block, x, h, z);
                        }
                    }
                } else if x % 6 == 0 && z % 6 == 0 {
                    editor.set_block(corner_block, x, ground_level + building_height, z); // Light fixtures
                }

                // Set the house ceiling
                editor.set_block(corner_block, x, ground_level + building_height + 1, z);
                //editor.set_block(&SPONGE, x, ground_level + building_height + 1, z);
            }
        }
    }

    if element.id == 905796139 {
        println!("CHECKPOINT REACHED");
        //std::process::exit(0);
    }
}
