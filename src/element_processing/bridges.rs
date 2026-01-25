use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

// TODO FIX - This handles ways with bridge=yes tag (e.g., highway bridges)
#[allow(dead_code)]
pub fn generate_bridges(editor: &mut WorldEditor, element: &ProcessedWay) {
    if let Some(_bridge_type) = element.tags.get("bridge") {
        let bridge_height = 3; // Height above the ground level

        // Get start and end node elevations and use MAX for level bridge deck
        // Using MAX ensures bridges don't dip when multiple bridge ways meet in a valley
        let bridge_deck_ground_y = if element.nodes.len() >= 2 {
            let start_node = &element.nodes[0];
            let end_node = &element.nodes[element.nodes.len() - 1];
            let start_y = editor.get_ground_level(start_node.x, start_node.z);
            let end_y = editor.get_ground_level(end_node.x, end_node.z);
            start_y.max(end_y)
        } else {
            return; // Need at least 2 nodes for a bridge
        };

        // Calculate total bridge length for ramp positioning
        let total_length: f64 = element
            .nodes
            .windows(2)
            .map(|pair| {
                let dx = (pair[1].x - pair[0].x) as f64;
                let dz = (pair[1].z - pair[0].z) as f64;
                (dx * dx + dz * dz).sqrt()
            })
            .sum();

        if total_length == 0.0 {
            return;
        }

        let mut accumulated_length: f64 = 0.0;

        for i in 1..element.nodes.len() {
            let prev = &element.nodes[i - 1];
            let cur = &element.nodes[i];

            let segment_dx = (cur.x - prev.x) as f64;
            let segment_dz = (cur.z - prev.z) as f64;
            let segment_length = (segment_dx * segment_dx + segment_dz * segment_dz).sqrt();

            let points = bresenham_line(prev.x, 0, prev.z, cur.x, 0, cur.z);

            let ramp_length = (total_length * 0.15).clamp(6.0, 20.0) as usize; // 15% of bridge, min 6, max 20 blocks

            for (idx, (x, _, z)) in points.iter().enumerate() {
                // Calculate progress along this segment
                let segment_progress = if points.len() > 1 {
                    idx as f64 / (points.len() - 1) as f64
                } else {
                    0.0
                };

                // Calculate overall progress along the entire bridge
                let point_distance = accumulated_length + segment_progress * segment_length;
                let overall_progress = (point_distance / total_length).clamp(0.0, 1.0);
                let total_len_usize = total_length as usize;
                let overall_idx = (overall_progress * total_len_usize as f64) as usize;

                // Calculate ramp height offset
                let ramp_offset = if overall_idx < ramp_length {
                    // Start ramp (rising)
                    (overall_idx as f64 * bridge_height as f64 / ramp_length as f64) as i32
                } else if overall_idx >= total_len_usize.saturating_sub(ramp_length) {
                    // End ramp (descending)
                    let dist_from_end = total_len_usize - overall_idx;
                    (dist_from_end as f64 * bridge_height as f64 / ramp_length as f64) as i32
                } else {
                    // Middle section (constant height)
                    bridge_height
                };

                // Use fixed bridge deck height (max of endpoints) plus ramp offset
                let bridge_y = bridge_deck_ground_y + ramp_offset;

                // Place bridge blocks
                for dx in -2..=2 {
                    editor.set_block_absolute(
                        LIGHT_GRAY_CONCRETE,
                        *x + dx,
                        bridge_y,
                        *z,
                        None,
                        None,
                    );
                }
            }

            accumulated_length += segment_length;
        }
    }
}
