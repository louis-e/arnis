use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

// TODO FIX
#[allow(dead_code)]
pub fn generate_bridges(editor: &mut WorldEditor, element: &ProcessedWay) {
    if let Some(_bridge_type) = element.tags.get("bridge") {
        let bridge_height = 3; // Fixed height

        for i in 1..element.nodes.len() {
            let prev = &element.nodes[i - 1];
            let cur = &element.nodes[i];
            let points = bresenham_line(prev.x, 0, prev.z, cur.x, 0, cur.z);

            let total_length = points.len();
            let ramp_length = 6; // Length of ramp at each end

            for (idx, (x, _, z)) in points.iter().enumerate() {
                let height = if idx < ramp_length {
                    // Start ramp (rising)
                    (idx * bridge_height) / ramp_length
                } else if idx >= total_length - ramp_length {
                    // End ramp (descending)
                    ((total_length - idx) * bridge_height) / ramp_length
                } else {
                    // Middle section (constant height)
                    bridge_height
                };

                // Place bridge blocks
                for dx in -2..=2 {
                    editor.set_block(LIGHT_GRAY_CONCRETE, *x + dx, height as i32, *z, None, None);
                }
            }
        }
    }
}
