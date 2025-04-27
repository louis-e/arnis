use crate::block_definitions::*;
use crate::osm_parser::ProcessedNode;
use crate::world_editor::WorldEditor;

pub fn generate_tourisms(editor: &mut WorldEditor, element: &ProcessedNode) {
    // Skip if 'layer' or 'level' is negative in the tags
    if let Some(layer) = element.tags.get("layer") {
        if layer.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(level) = element.tags.get("level") {
        if level.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(tourism_type) = element.tags.get("tourism") {
        let x: i32 = element.x;
        let z: i32 = element.z;

        if tourism_type == "information" {
            if let Some(info_type) = element.tags.get("information").map(|x: &String| x.as_str()) {
                if info_type != "office" && info_type != "visitor_centre" {
                    // Draw an information board
                    // TODO draw a sign with text if provided
                    editor.set_block(COBBLESTONE_WALL, x, 1, z, None, None);
                    editor.set_block(OAK_PLANKS, x, 2, z, None, None);
                }
            }
        }
    }
}
