use crate::args::Args;
use crate::block_definitions::BLOCKS;
use crate::bresenham::bresenham_line;
use crate::cartesian::XZPoint;
use crate::floodfill::flood_fill_area;
use crate::ground::Ground;
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;

pub fn generate_amenities(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    ground: &Ground,
    args: &Args,
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
        let first_node: Option<XZPoint> = element
            .nodes()
            .map(|n: &crate::osm_parser::ProcessedNode| XZPoint::new(n.x, n.z))
            .next();
        match amenity_type.as_str() {
            "waste_disposal" | "waste_basket" => {
                // Place a cauldron for waste disposal or waste basket
                if let Some(pt) = first_node {
                    editor.set_block(&*BLOCKS.by_name("cauldron").unwrap(), pt.x, ground.level(pt) + 1, pt.z, None, None);
                }
            }
            "vending_machine" | "atm" => {
                if let Some(pt) = first_node {
                    let y = ground.level(pt);

                    editor.set_block(&*BLOCKS.by_name("iron_block").unwrap(), pt.x, y + 1, pt.z, None, None);
                    editor.set_block(&*BLOCKS.by_name("iron_block").unwrap(), pt.x, y + 2, pt.z, None, None);
                }
            }
            "bicycle_parking" => {
                let ground_block = &*BLOCKS.by_name("oak_planks").unwrap();
                let roof_block = &*BLOCKS.by_name("stone_block_slab").unwrap();

                let polygon_coords: Vec<(i32, i32)> = element
                    .nodes()
                    .map(|n: &crate::osm_parser::ProcessedNode| (n.x, n.z))
                    .collect();
                let floor_area: Vec<(i32, i32)> =
                    flood_fill_area(&polygon_coords, args.timeout.as_ref());

                let pts: Vec<_> = floor_area.iter().map(|c| XZPoint::new(c.0, c.1)).collect();

                if pts.is_empty() {
                    return;
                }

                let y_min = ground.min_level(pts.iter().cloned()).unwrap();
                let roof_y = ground.max_level(pts.iter().cloned()).unwrap() + 5;

                // Fill the floor area
                for (x, z) in floor_area.iter() {
                    editor.set_block(ground_block, *x, y_min, *z, None, None);
                }

                // Place fences and roof slabs at each corner node directly
                for node in element.nodes() {
                    let x: i32 = node.x;
                    let z: i32 = node.z;

                    let pt = XZPoint::new(x, z);

                    let y = ground.level(pt);
                    editor.set_block(ground_block, x, y, z, None, None);

                    for cur_y in (y_min + 1)..roof_y {
                        editor.set_block(&*BLOCKS.by_name("oak_fence").unwrap(), x, cur_y, z, None, None);
                    }
                    editor.set_block(roof_block, x, roof_y, z, None, None);
                }

                // Flood fill the roof area
                for (x, z) in floor_area.iter() {
                    editor.set_block(roof_block, *x, roof_y, *z, None, None);
                }
            }
            "bench" => {
                // Place a bench
                if let Some(pt) = first_node {
                    let y = ground.level(pt) + 1;

                    editor.set_block(&*BLOCKS.by_name("smooth_stone").unwrap(), pt.x, y, pt.z, None, None);
                    editor.set_block(&*BLOCKS.by_name("oak_log").unwrap(), pt.x + 1, y, pt.z, None, None);
                    editor.set_block(&*BLOCKS.by_name("oak_log").unwrap(), pt.x - 1, y, pt.z, None, None);
                }
            }
            "vending" => {
                // Place vending machine blocks
                if let Some(pt) = first_node {
                    let y = ground.level(pt);

                    editor.set_block(&*BLOCKS.by_name("iron_block").unwrap(), pt.x, y + 1, pt.z, None, None);
                    editor.set_block(&*BLOCKS.by_name("iron_block").unwrap(), pt.x, y + 2, pt.z, None, None);
                }
            }
            "shelter" => {
                let roof_block = &*BLOCKS.by_name("stone_brick_slab").unwrap();

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

                    let Some(y) = ground.min_level(element.nodes().map(|n| n.xz())) else {
                        return;
                    };

                    for fence_height in 1..=4 {
                        editor.set_block(&*BLOCKS.by_name("oak_fence").unwrap(), x, y + fence_height, z, None, None);
                    }
                    editor.set_block(roof_block, x, y + 5, z, None, None);
                }

                // Flood fill the roof area
                let Some(y) = ground.min_level(element.nodes().map(|n| n.xz())) else {
                    return;
                };

                let roof_height: i32 = y + 5;
                for (x, z) in roof_area.iter() {
                    editor.set_block(roof_block, *x, roof_height, *z, None, None);
                }
            }
            "parking" | "fountain" => {
                // Process parking or fountain areas
                let mut previous_node: Option<XZPoint> = None;
                let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
                let mut current_amenity: Vec<(i32, i32)> = vec![];

                let block_type = match amenity_type.as_str() {
                    "fountain" => &*BLOCKS.by_name("water").unwrap(),
                    "parking" => &*BLOCKS.by_name("gray_concrete").unwrap(),
                    _ => &*BLOCKS.by_name("gray_concrete").unwrap(),
                };

                for node in element.nodes() {
                    let pt: XZPoint = node.xz();
                    let y = ground.level(pt);

                    if let Some(prev) = previous_node {
                        // Create borders for fountain or parking area
                        let prev_y = ground.level(prev);
                        let bresenham_points: Vec<(i32, i32, i32)> =
                            bresenham_line(prev.x, prev_y, prev.z, pt.x, y, pt.z);
                        for (bx, by, bz) in bresenham_points {
                            editor.set_block(block_type, bx, by, bz, Some(&[&*BLOCKS.by_name("black_concrete").unwrap()]), None);

                            // Decorative border around fountains
                            if amenity_type == "fountain" {
                                for dx in [-1, 0, 1].iter() {
                                    for dz in [-1, 0, 1].iter() {
                                        if (*dx, *dz) != (0, 0) {
                                            editor.set_block(
                                                &*BLOCKS.by_name("light_gray_concrete").unwrap(),
                                                bx + dx,
                                                by,
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
                        let pt = XZPoint::new(x, z);
                        let y = ground.level(pt);
                        editor.set_block(
                            block_type,
                            x,
                            y,
                            z,
                            Some(&[&*BLOCKS.by_name("black_concrete").unwrap(), &*BLOCKS.by_name("gray_concrete").unwrap()]),
                            None,
                        );

                        // Add parking spot markings
                        if amenity_type == "parking" && (x + z) % 8 == 0 && (x * z) % 32 != 0 {
                            editor.set_block(
                                &*BLOCKS.by_name("light_gray_concrete").unwrap(),
                                x,
                                y,
                                z,
                                Some(&[&*BLOCKS.by_name("black_concrete").unwrap(), &*BLOCKS.by_name("gray_concrete").unwrap()]),
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
