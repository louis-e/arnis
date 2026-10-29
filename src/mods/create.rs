//! Create: seats you can sit on, factory machinery and girders, workshop toolboxes, and
//! windmills that turn once assembled.

use super::{dir_name, state};
use crate::block_definitions::*;
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use std::collections::HashMap;

const DYES: [&str; 8] = [
    "red",
    "blue",
    "green",
    "gray",
    "black",
    "brown",
    "orange",
    "light_gray",
];

/// A seat players and mobs can sit on.
pub fn seat(h: u64) -> BlockWithProperties {
    state(
        &format!("create:{}_seat", DYES[(h % DYES.len() as u64) as usize]),
        OAK_SLAB,
        &[],
    )
}

/// One block of factory machinery: a casing, an iron block or a depot.
pub fn machine(h: u64) -> BlockWithProperties {
    let (name, fallback) = match h % 6 {
        0 => ("andesite_casing", POLISHED_ANDESITE),
        1 => ("brass_casing", OAK_PLANKS),
        2 => ("copper_casing", WAXED_COPPER_BLOCK),
        3 => ("industrial_iron_block", IRON_BLOCK),
        4 => ("railway_casing", POLISHED_BLACKSTONE),
        _ => ("andesite_alloy_block", POLISHED_ANDESITE),
    };
    state(&format!("create:{name}"), fallback, &[])
}

/// A girder running along x or z under a ceiling.
pub fn girder(along_x: bool) -> BlockWithProperties {
    state(
        "create:metal_girder",
        IRON_BARS,
        &[
            ("x", if along_x { "true" } else { "false" }),
            ("z", if along_x { "false" } else { "true" }),
            ("top", "false"),
            ("bottom", "false"),
        ],
    )
}

/// A toolbox on a workbench, its lid towards `front`.
pub fn toolbox(editor: &mut WorldEditor, x: i32, y: i32, z: i32, front: (i32, i32), h: u64) {
    let colour = DYES[(h % DYES.len() as u64) as usize];
    let block = state(
        &format!("create:{colour}_toolbox"),
        CHEST,
        &[("facing", dir_name(front)), ("waterlogged", "false")],
    );
    with_entity(editor, block, x, y, z, "toolbox");
}

/// A depot, the flat stand Create's machines set items on.
pub fn depot(editor: &mut WorldEditor, x: i32, y: i32, z: i32) {
    with_entity(
        editor,
        state("create:depot", SMOOTH_STONE_SLAB, &[]),
        x,
        y,
        z,
        "depot",
    );
}

fn with_entity(
    editor: &mut WorldEditor,
    block: BlockWithProperties,
    x: i32,
    y: i32,
    z: i32,
    id: &str,
) {
    let be = HashMap::from([
        ("id".to_string(), Value::String(format!("create:{id}"))),
        ("x".to_string(), Value::Int(x)),
        ("y".to_string(), Value::Int(y)),
        ("z".to_string(), Value::Int(z)),
    ]);
    editor.set_block_entity_absolute(block, x, y, z, be);
}

/// Windmill sails on a bearing at (`x`, `y`, `z`) facing `out`: four arms of `arm` sails.
/// The player assembles the mill by using the bearing, and the sails turn from then on.
pub fn windmill(editor: &mut WorldEditor, x: i32, y: i32, z: i32, out: (i32, i32), arm: i32) {
    let face = dir_name(out);
    with_entity(
        editor,
        state("create:windmill_bearing", OAK_LOG, &[("facing", face)]),
        x,
        y,
        z,
        "windmill_bearing",
    );
    // The sails stand one block out, in the plane across `out`.
    let (cx, cz) = (x + out.0, z + out.1);
    let across = (out.1.abs(), out.0.abs());
    let hub = state("create:sail_frame", OAK_PLANKS, &[("facing", face)]);
    editor.set_block_with_properties_absolute(hub, cx, y, cz, None, Some(&[]));
    for i in 1..=arm {
        // A cloth arm up and down, a frame arm left and right, as mills often had.
        for (dy, d, cloth) in [(i, 0, true), (-i, 0, true), (0, i, false), (0, -i, false)] {
            let sail = if cloth {
                state("create:white_sail", WHITE_WOOL, &[("facing", face)])
            } else {
                state("create:sail_frame", OAK_PLANKS, &[("facing", face)])
            };
            editor.set_block_with_properties_absolute(
                sail,
                cx + across.0 * d,
                y + dy,
                cz + across.1 * d,
                None,
                Some(&[]),
            );
        }
    }
}
