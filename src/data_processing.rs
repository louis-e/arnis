use crate::args::Args;
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;
use crate::element_processing::{*};

pub fn generate_world(elements: Vec<ProcessedElement>, args: &Args, scale_factor_x: f64, scale_factor_z: f64) {
    println!("Generating blocks...");
    let region_template_path: &str = "empty.mca"; // "empty.mca"; //"region.template";
    let region_dir: String = format!("{}/region", args.path);
    let ground_level: i32 = -61;

    let mut editor: WorldEditor = WorldEditor::new(region_template_path, &region_dir, scale_factor_x, scale_factor_z);

    for element in &elements {
        //println!("Processing element ID: {} of type: {}", element.id, element.r#type);
        
        match element.r#type.as_str() {
            "way" => {
                if element.tags.contains_key("building") || element.tags.contains_key("area:highway") {
                    buildings::generate_buildings(&mut editor, element, ground_level);
                } else if element.tags.contains_key("highway") {
                    highways::generate_highways(&mut editor, element, ground_level);
                } else if element.tags.contains_key("landuse") {
                    landuse::generate_landuse(&mut editor, element, ground_level);
                } else if element.tags.contains_key("natural") {
                    natural::generate_natural(&mut editor, element, ground_level);
                } else if element.tags.contains_key("amenity") {
                    amenities::generate_amenities(&mut editor, element, ground_level);
                } else if element.tags.contains_key("leisure") {
                    leisure::generate_leisure(&mut editor, element, ground_level);
                } else if element.tags.contains_key("barrier") {
                    barriers::generate_barriers(&mut editor, element, ground_level);
                } else if element.tags.contains_key("waterway") {
                    waterways::generate_waterways(&mut editor, element, ground_level);
                } else if element.tags.contains_key("bridge") {
                    bridges::generate_bridges(&mut editor, element, ground_level);
                } else if element.tags.contains_key("railway") {
                    railways::generate_railways(&mut editor, element, ground_level);
                }
            }
            "node" => {
                if element.tags.contains_key("door") || element.tags.contains_key("entrance") {
                    doors::generate_doors(&mut editor, element, ground_level);
                } else if element.tags.contains_key("natural") && element.tags.get("natural") == Some(&"tree".to_string()) {
                    natural::generate_natural(&mut editor, element, ground_level);
                } else if element.tags.contains_key("amenity") {
                    amenities::generate_amenities(&mut editor, element, ground_level);
                } else if element.tags.contains_key("barrier") {
                    barriers::generate_barriers(&mut editor, element, ground_level);
                } else if element.tags.contains_key("highway") {
                    highways::generate_highways(&mut editor, element, ground_level);
                }
            }
            _ => {}
        }
    }

    println!("Generating ground layer...");
    for x in 0..=(scale_factor_x as i32) {
        for z in 0..=(scale_factor_z as i32) {
            editor.set_block(&crate::block_definitions::GRASS_BLOCK, x, ground_level, z, None, None);
            editor.set_block(&crate::block_definitions::DIRT, x, ground_level - 1, z, None, None);
        }
    }

    // Save world
    editor.save();
}
