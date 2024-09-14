use crate::world_editor::WorldEditor;
use crate::osm_parser::ProcessedElement;
use crate::block_definitions::*;

pub fn generate_doors(editor: &mut WorldEditor, element: &ProcessedElement, ground_level: i32) {
    // Check if the element is a door or entrance
    if element.tags.contains_key("door") || element.tags.contains_key("entrance") {
        // Check for the "level" tag and skip doors that are not at ground level
        if let Some(level_str) = element.tags.get("level") {
            if let Ok(level) = level_str.parse::<i32>() {
                if level != 0 {
                    return; // Skip doors not on ground level
                }
            }
        }

        // Process the first node of the door/entrance element
        if let Some(&(x, z)) = element.nodes.first() {
            // TODO println!("DOOR {} {} {}", x, ground_level, z);
            // Set the ground block and the door blocks
            editor.set_block(&GRAY_CONCRETE, x, ground_level, z, None, None);
            editor.set_block(&DARK_OAK_DOOR_LOWER, x, ground_level + 1, z, None, None);
            editor.set_block(&DARK_OAK_DOOR_UPPER, x, ground_level + 2, z, None, None);
        }
    }
}