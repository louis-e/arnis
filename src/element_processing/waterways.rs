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

            let y0 = editor.get_water_level(prev_node.x, prev_node.z);
            let y1 = editor.get_water_level(current_node.x, current_node.z);

            // Draw a line between the current and previous node
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(
                prev_node.x,
                0,
                prev_node.z,
                current_node.x,
                0,
                current_node.z,
            );

            // A flat Y per segment leaves puddles near the lower node, so ramp between
            // the endpoints and clamp to the local surface so the sheet never floats.
            let last = bresenham_points.len().saturating_sub(1) as i64;
            let dy = i64::from(y1 - y0);
            for (i, (bx, _, bz)) in bresenham_points.into_iter().enumerate() {
                let ramped = if last > 0 {
                    let num = dy * i as i64;
                    y0 + ((2 * num + last * dy.signum()) / (2 * last)) as i32
                } else {
                    y0.min(y1)
                };
                let seg_water_y = ramped.min(editor.get_water_level(bx, bz));
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
    target_water_y: i32,
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
                let water_y = if ground_y <= target_water_y {
                    Some(target_water_y)
                } else if ground_y <= target_water_y + BANK_TOLERANCE
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

#[cfg(test)]
mod waterway_profile_tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::ground::Ground;
    use crate::osm_parser::ProcessedNode;
    use std::path::PathBuf;
    use std::sync::Arc;

    const SIDE: usize = 64;
    const CENTER_X: i32 = 32;
    const FIRST_Z: i32 = 4;
    const LAST_Z: i32 = 52;

    fn world_bbox() -> XZBBox {
        XZBBox::rect_from_min_max(0, 0, SIDE as i32 - 1, SIDE as i32 - 1).unwrap()
    }

    /// Terrain dropping `per_block` blocks per step along Z, gentle enough that
    /// `water_level` never snaps, so a column's expected surface is its own level.
    fn editor_over_slope(bbox: &XZBBox, top: f64, per_block: f64) -> WorldEditor<'_> {
        let heights: Vec<Vec<f32>> = (0..SIDE)
            .map(|z| vec![(top - per_block * z as f64) as f32; SIDE])
            .collect();
        let ground = Ground::new_elevation_test(heights, SIDE, SIDE);
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let mut editor = WorldEditor::new(PathBuf::from("/dev/null/unused"), bbox, llbbox);
        editor.set_ground(Arc::new(ground));
        editor
    }

    fn stream(from_z: i32, to_z: i32) -> ProcessedWay {
        let node = |id: u64, z: i32| ProcessedNode {
            id,
            tags: HashMap::new(),
            x: CENTER_X,
            z,
        };
        let mut tags = HashMap::new();
        tags.insert("waterway".to_string(), "stream".to_string());
        ProcessedWay {
            id: 1,
            nodes: vec![node(1, from_z), node(2, to_z)],
            tags,
        }
    }

    fn water_ys(editor: &WorldEditor, x: i32, z: i32) -> Vec<i32> {
        (-8..40)
            .filter(|&y| editor.check_for_block_absolute(x, y, z, Some(&[WATER]), None))
            .collect()
    }

    #[test]
    fn a_dropping_channel_follows_the_terrain_instead_of_pooling_at_the_lower_node() {
        let bbox = world_bbox();
        let mut editor = editor_over_slope(&bbox, 16.0, 0.25);
        generate_waterways(&mut editor, &stream(FIRST_Z, LAST_Z));

        let upper = editor.get_water_level(CENTER_X, FIRST_Z);
        let lower = editor.get_water_level(CENTER_X, LAST_Z);
        assert_eq!((upper, lower), (15, 3), "test fixture no longer drops 12");

        for z in FIRST_Z..=LAST_Z {
            let local = editor.get_water_level(CENTER_X, z);
            let ys = water_ys(&editor, CENTER_X, z);
            assert!(!ys.is_empty(), "dry gap at z={z}, local level {local}");
            let top = *ys.iter().max().unwrap();
            assert!(
                (top - local).abs() <= 1,
                "z={z}: surface {top} does not track the local level {local}"
            );
        }
    }

    #[test]
    fn a_flat_channel_sits_at_the_single_shared_level() {
        let bbox = world_bbox();
        let mut editor = editor_over_slope(&bbox, 8.0, 0.0);
        generate_waterways(&mut editor, &stream(FIRST_Z, LAST_Z));

        for z in FIRST_Z..=LAST_Z {
            assert_eq!(
                water_ys(&editor, CENTER_X, z),
                vec![8],
                "flat terrain must place one sheet at the shared level, z={z}"
            );
        }
    }

    #[test]
    fn a_one_block_drop_moves_the_upper_half_by_at_most_one_block() {
        let bbox = world_bbox();
        let mut editor = editor_over_slope(&bbox, 8.0, 0.02);
        generate_waterways(&mut editor, &stream(FIRST_Z, LAST_Z));

        let upper = editor.get_water_level(CENTER_X, FIRST_Z);
        let lower = editor.get_water_level(CENTER_X, LAST_Z);
        assert_eq!((upper, lower), (8, 7), "test fixture no longer drops 1");

        let top_at =
            |editor: &WorldEditor, z: i32| *water_ys(editor, CENTER_X, z).iter().max().unwrap();
        assert_eq!(top_at(&editor, FIRST_Z), upper, "upstream end left behind");
        assert_eq!(top_at(&editor, LAST_Z), lower, "downstream end lifted");
        for z in FIRST_Z..=LAST_Z {
            let top = top_at(&editor, z);
            assert!(
                top == lower || top == lower + 1,
                "z={z}: {top} is more than one block off the pre-ramp level {lower}"
            );
        }
    }
}
