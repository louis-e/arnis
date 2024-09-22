use std::collections::HashMap;

use fastnbt::Value;

use crate::block_definitions::*;
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;

pub fn generate_tourisms(editor: &mut WorldEditor, element: &ProcessedElement, ground_level: i32) {
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
        let first_node: Option<&(i32, i32)> = element.nodes.first();
        match tourism_type.as_str() {
            "information" => {
                if let Some(information_type) = element.tags.get("information") {
                    match information_type.as_str() {
                        "board" => {
                            if let Some(&(x, z)) = first_node {
                                // TODO draw a sign
                                editor.set_block(&OAK_PLANKS, x, ground_level + 1, z, None, None);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}
