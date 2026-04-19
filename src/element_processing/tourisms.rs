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
                    editor.set_block(COBBLESTONE_WALL, x, 1, z, None, None);
                    editor.set_block(OAK_PLANKS, x, 2, z, None, None);

                    // White banner with blue masking to form a lowercase "i" shape.
                    // Layers: start white, paint blue on left/right/top/middle/border,
                    // leaving a dot (between top and middle) and a stem below.
                    let abs_y = editor.get_absolute_y(x, 2, z);
                    const INFO_PATTERNS: &[(&str, &str)] = &[
                        ("blue", "minecraft:stripe_left"),
                        ("blue", "minecraft:stripe_right"),
                        ("blue", "minecraft:stripe_top"),
                        ("blue", "minecraft:stripe_middle"),
                        ("blue", "minecraft:border"),
                    ];
                    // Place info banners on all four sides
                    let banner_faces: [(i32, i32, &str); 4] = [
                        (0, 1, "south"),
                        (0, -1, "north"),
                        (1, 0, "east"),
                        (-1, 0, "west"),
                    ];
                    for (dx, dz, facing) in &banner_faces {
                        editor.place_wall_banner(
                            WHITE_WALL_BANNER,
                            x + dx,
                            abs_y,
                            z + dz,
                            facing,
                            "white",
                            INFO_PATTERNS,
                        );
                    }
                }
            }
        }
    }
}
