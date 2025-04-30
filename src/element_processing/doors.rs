use crate::block_definitions::*;
use crate::osm_parser::ProcessedNode;
use crate::world_editor::WorldEditor;

pub fn generate_doors(editor: &mut WorldEditor, element: &ProcessedNode) {
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

        let x: i32 = element.x;
        let z: i32 = element.z;

        // Set the ground block and the door blocks
        editor.set_block(GRAY_CONCRETE, x, 0, z, None, None);
        editor.set_block(DARK_OAK_DOOR_LOWER, x, 1, z, None, None);
        editor.set_block(DARK_OAK_DOOR_UPPER, x, 2, z, None, None);
    }
}
