use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::cartesian::XZPoint;
use crate::ground::Ground;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

pub fn generate_railways(editor: &mut WorldEditor, element: &ProcessedWay, ground: &Ground) {
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
            let prev_node = element.nodes[i - 1].xz();
            let cur_node = element.nodes[i].xz();

            // Generate the line of coordinates between the two nodes
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(prev_node.x, 0, prev_node.z, cur_node.x, 0, cur_node.z);

            for (bx, _, bz) in bresenham_points {
                let ground_level = ground.level(XZPoint::new(bx, bz));

                // TODO: Set direction of rail
                editor.set_block(IRON_BLOCK, bx, ground_level, bz, None, None);
                editor.set_block(RAIL, bx, ground_level + 1, bz, None, None);

                if bx % 4 == 0 {
                    editor.set_block(OAK_LOG, bx, ground_level, bz, None, None);
                }
            }
        }
    }
}
