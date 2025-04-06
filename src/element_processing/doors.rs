use crate::block_definitions::BLOCKS;
use crate::cartesian::XZPoint;
use crate::ground::Ground;
use crate::osm_parser::ProcessedNode;
use crate::world_editor::WorldEditor;

pub fn generate_doors(editor: &mut WorldEditor, element: &ProcessedNode, ground: &Ground) {
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

        // Calculate the dynamic ground level
        let ground_level = ground.level(XZPoint::new(x, z));

        // Set the ground block and the door blocks
        editor.set_block(
            &*BLOCKS.by_name("gray_concrete").unwrap(),
            x,
            ground_level,
            z,
            None,
            None,
        );
        editor.set_block(
            &*BLOCKS.by_name("dark_oak_door_lower").unwrap(),
            x,
            ground_level + 1,
            z,
            None,
            None,
        );
        editor.set_block(
            &*BLOCKS.by_name("dark_oak_door_upper").unwrap(),
            x,
            ground_level + 2,
            z,
            None,
            None,
        );
    }
}
