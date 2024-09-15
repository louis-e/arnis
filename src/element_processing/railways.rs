use crate::world_editor::WorldEditor;
use crate::osm_parser::ProcessedElement;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;

pub fn generate_railways(editor: &mut WorldEditor, element: &ProcessedElement, ground_level: i32) {
    if let Some(railway_type) = element.tags.get("railway") {
        if ["proposed", "abandoned", "subway", "construction"].contains(&railway_type.as_str()) {
            return;
        }

        if let Some(subway) = element.tags.get("subway") {
            if subway == "yes" {
                return;
            }
        }

        if let Some(tunnel) = element.tags.get("tunnel") {
            if tunnel == "yes" {
                return;
            }
        }

        for i in 1..element.nodes.len() {
            let (x1, z1) = element.nodes[i - 1];
            let (x2, z2) = element.nodes[i];

            // Generate the line of coordinates between the two nodes
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(x1, ground_level, z1, x2, ground_level, z2);

            for (bx, _, bz) in bresenham_points {
                // TODO: Set direction of rail
                editor.set_block(&IRON_BLOCK, bx, ground_level, bz, None, None);
                editor.set_block(&RAIL, bx, ground_level + 1, bz, None, None);

                if bx % 4 == 0 {
                    editor.set_block(&OAK_LOG, bx, ground_level, bz, None, None);
                }
            }
        }
    }
}
