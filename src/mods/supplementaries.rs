//! Supplementaries: flower boxes, flags, wind vanes, globes, clocks and cannons.
//!
//! Most of these draw through their block entity, so each is written with one.

use super::{dir_name, state};
use crate::block_definitions::*;
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use std::collections::HashMap;

fn place_with_entity(
    editor: &mut WorldEditor,
    block: BlockWithProperties,
    x: i32,
    y: i32,
    z: i32,
    entity: &str,
    extra: HashMap<String, Value>,
) {
    let mut be = extra;
    be.insert(
        "id".to_string(),
        Value::String(format!("supplementaries:{entity}")),
    );
    be.insert("x".to_string(), Value::Int(x));
    be.insert("y".to_string(), Value::Int(y));
    be.insert("z".to_string(), Value::Int(z));
    editor.set_block_entity_absolute(block, x, y, z, be);
}

/// A box of three flowers on a window sill, its back to the wall the window is in.
pub fn flower_box(editor: &mut WorldEditor, x: i32, y: i32, z: i32, out: (i32, i32), h: u64) {
    const FLOWERS: [&str; 8] = [
        "minecraft:poppy",
        "minecraft:red_tulip",
        "minecraft:pink_tulip",
        "minecraft:white_tulip",
        "minecraft:orange_tulip",
        "minecraft:cornflower",
        "minecraft:azure_bluet",
        "minecraft:oxeye_daisy",
    ];
    // One kind per box, the odd one out in the middle now and then.
    let main = FLOWERS[(h % FLOWERS.len() as u64) as usize];
    let middle = if (h >> 8).is_multiple_of(3) {
        FLOWERS[((h >> 12) % FLOWERS.len() as u64) as usize]
    } else {
        main
    };
    let items: Vec<Value> = [main, middle, main]
        .iter()
        .enumerate()
        .map(|(slot, id)| {
            Value::Compound(HashMap::from([
                ("Slot".to_string(), Value::Byte(slot as i8)),
                ("id".to_string(), Value::String(id.to_string())),
                ("count".to_string(), Value::Int(1)),
            ]))
        })
        .collect();
    let block = state(
        "supplementaries:flower_box",
        FLOWERING_AZALEA_LEAVES,
        &[("face", "floor"), ("facing", dir_name(out))],
    );
    place_with_entity(
        editor,
        block,
        x,
        y,
        z,
        "flower_box",
        HashMap::from([("Items".to_string(), Value::List(items))]),
    );
}

/// A spinning globe on a desk or a shelf.
pub fn globe(editor: &mut WorldEditor, x: i32, y: i32, z: i32, front: (i32, i32)) {
    let block = state(
        "supplementaries:globe",
        LANTERN,
        &[("facing", dir_name(front))],
    );
    place_with_entity(editor, block, x, y, z, "globe", HashMap::new());
}

/// A clock face on a wall, keeping the world's time.
pub fn clock(editor: &mut WorldEditor, x: i32, y: i32, z: i32, front: (i32, i32)) {
    let block = state(
        "supplementaries:clock_block",
        BOOKSHELF,
        &[("facing", dir_name(front)), ("two_faced", "false")],
    );
    place_with_entity(editor, block, x, y, z, "clock_block", HashMap::new());
}

/// A clock with a face front and back, for a post in the street.
pub fn street_clock(editor: &mut WorldEditor, x: i32, y: i32, z: i32, front: (i32, i32)) {
    let block = state(
        "supplementaries:clock_block",
        BOOKSHELF,
        &[("facing", dir_name(front)), ("two_faced", "true")],
    );
    place_with_entity(editor, block, x, y, z, "clock_block", HashMap::new());
}

/// A wind vane on a spire's tip.
pub fn wind_vane(editor: &mut WorldEditor, x: i32, y: i32, z: i32) {
    let block = state("supplementaries:wind_vane", LIGHTNING_ROD, &[]);
    place_with_entity(editor, block, x, y, z, "wind_vane", HashMap::new());
}

/// A cloth flag flying from the side of a pole along `out`.
pub fn flag(editor: &mut WorldEditor, x: i32, y: i32, z: i32, out: (i32, i32), colour: &str) {
    let block = state(
        &format!("supplementaries:flag_{colour}"),
        WHITE_WOOL,
        &[("facing", dir_name(out))],
    );
    place_with_entity(editor, block, x, y, z, "flag", HashMap::new());
}

/// An old cannon, for a mapped `historic=cannon`.
pub fn cannon(editor: &mut WorldEditor, x: i32, y: i32, z: i32, front: (i32, i32)) {
    let block = state(
        "supplementaries:cannon",
        ANVIL,
        &[("facing", dir_name(front))],
    );
    place_with_entity(editor, block, x, y, z, "cannon", HashMap::new());
}
