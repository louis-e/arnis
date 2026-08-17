use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;
use std::collections::HashMap;

pub fn generate_waterways(editor: &mut WorldEditor, element: &ProcessedWay) {
    if let Some(waterway_type) = element.tags.get("waterway") {
        // waterway=* structures are not channels; outlining a dam draws canals down it.
        if !is_channel_waterway(waterway_type) {
            return;
        }
        let waterway_width = waterway_width(waterway_type, &element.tags);

        // Culverts and pipes are not open water; they would cut channels through banks.
        if is_underground_waterway(&element.tags) {
            return;
        }

        // Process consecutive node pairs to create waterways
        // Use windows(2) to avoid connecting last node back to first
        for nodes_pair in element.nodes.windows(2) {
            let prev_node = nodes_pair[0].xz();
            let current_node = nodes_pair[1].xz();

            // Compute flat water level for this segment (min of both endpoints)
            let seg_water_y = editor
                .get_water_level(prev_node.x, prev_node.z)
                .min(editor.get_water_level(current_node.x, current_node.z));

            // Draw a line between the current and previous node
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(
                prev_node.x,
                0,
                prev_node.z,
                current_node.x,
                0,
                current_node.z,
            );

            for (bx, _, bz) in bresenham_points {
                create_water_channel(editor, bx, bz, waterway_width, seg_water_y);
            }
        }
    }
}

/// False for `waterway=*` values that are structures or points, not a channel.
pub fn is_channel_waterway(waterway_type: &str) -> bool {
    !matches!(
        waterway_type,
        "dam"
            | "weir"
            | "lock_gate"
            | "waterfall"
            | "rapids"
            | "boatyard"
            | "fuel"
            | "dock"
            | "riverbank"
            | "water_point"
            | "turning_point"
            | "sluice_gate"
            | "fish_pass"
            | "security_lock"
            | "milestone"
            | "check_dam"
            | "floating_barrier"
    )
}

/// True for waterways underground: any `tunnel=*` other than `no`, or a negative layer.
pub fn is_underground_waterway(tags: &std::collections::HashMap<String, String>) -> bool {
    if tags
        .get("tunnel")
        .is_some_and(|v| !matches!(v.as_str(), "no" | "0" | "false"))
    {
        return true;
    }
    tags.get("layer")
        .and_then(|l| l.trim().parse::<i32>().ok())
        .is_some_and(|l| l < 0)
}

/// Determines channel width based on waterway type.
pub fn get_waterway_width(waterway_type: &str) -> i32 {
    match waterway_type {
        "river" => 8,
        "canal" => 6,
        "stream" => 3,
        "fairway" => 12,
        "flowline" => 2,
        "brook" => 2,
        "ditch" => 2,
        "drain" => 1,
        _ => 4,
    }
}

/// Widest channel a `width=*` tag may ask for. Every renderer of a waterway walks its
/// width squared per centreline point, so an unbounded tag value hangs generation.
pub const MAX_WATERWAY_WIDTH: i32 = 128;

/// Channel width in blocks, from `width=*` when it parses and the type default otherwise.
pub fn waterway_width(waterway_type: &str, tags: &HashMap<String, String>) -> i32 {
    let tagged = tags
        .get("width")
        .and_then(|s| s.trim().split(' ').next())
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|w| w.is_finite())
        .map(|w| w.round() as i64);
    match tagged {
        Some(w) if w >= 1 => w.min(i64::from(MAX_WATERWAY_WIDTH)) as i32,
        _ => get_waterway_width(waterway_type),
    }
}

/// Creates a water channel at a target water level with the given width.
/// Skips blocks where terrain is above the water surface, with a small tolerance
/// to avoid gaps on gentle slopes (can create stepped banks).
fn create_water_channel(
    editor: &mut WorldEditor,
    center_x: i32,
    center_z: i32,
    width: i32,
    flat_water_y: i32,
) {
    const BANK_TOLERANCE: i32 = 2;
    let half_width = width / 2;

    for x in (center_x - half_width - 1)..=(center_x + half_width + 1) {
        for z in (center_z - half_width - 1)..=(center_z + half_width + 1) {
            let dx = (x - center_x).abs();
            let dz = (z - center_z).abs();
            let distance_from_center = dx.max(dz);

            if distance_from_center <= half_width + 1 {
                let ground_y = editor.get_ground_level(x, z);
                // Only place water where terrain is at or below the water surface,
                // but allow small elevation steps to avoid gaps on gentle slopes.
                let water_y = if ground_y <= flat_water_y {
                    Some(flat_water_y)
                } else if ground_y <= flat_water_y + BANK_TOLERANCE
                    && !editor.block_exists_absolute(x, ground_y, z)
                {
                    Some(ground_y)
                } else {
                    None
                };

                if let Some(water_y) = water_y {
                    editor.set_block_absolute(WATER, x, water_y, z, None, None);

                    // Clear vegetation above the water
                    editor.set_block_absolute(
                        AIR,
                        x,
                        water_y + 1,
                        z,
                        Some(&[GRASS, WHEAT, CARROTS, POTATOES]),
                        None,
                    );
                }
            }
        }
    }
}
