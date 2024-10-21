use crate::block_definitions::*;
use crate::world_editor::WorldEditor;
use once_cell::sync::Lazy;

/// Helper function to set blocks in a circular pattern around a central point.
fn round1(editor: &mut WorldEditor, material: &'static Lazy<Block>, x: i32, y: i32, z: i32) {
    editor.set_block(material, x - 2, y, z, None, None);
    editor.set_block(material, x + 2, y, z, None, None);
    editor.set_block(material, x, y, z - 2, None, None);
    editor.set_block(material, x, y, z + 2, None, None);
    editor.set_block(material, x - 1, y, z - 1, None, None);
    editor.set_block(material, x + 1, y, z + 1, None, None);
    editor.set_block(material, x + 1, y, z - 1, None, None);
    editor.set_block(material, x - 1, y, z + 1, None, None);
}

/// Helper function to set blocks in a wider circular pattern.
fn round2(editor: &mut WorldEditor, material: &'static Lazy<Block>, x: i32, y: i32, z: i32) {
    editor.set_block(material, x + 3, y, z, None, None);
    editor.set_block(material, x + 2, y, z - 1, None, None);
    editor.set_block(material, x + 2, y, z + 1, None, None);
    editor.set_block(material, x + 1, y, z - 2, None, None);
    editor.set_block(material, x + 1, y, z + 2, None, None);
    editor.set_block(material, x - 3, y, z, None, None);
    editor.set_block(material, x - 2, y, z - 1, None, None);
    editor.set_block(material, x - 2, y, z + 1, None, None);
    editor.set_block(material, x - 1, y, z + 2, None, None);
    editor.set_block(material, x - 1, y, z - 2, None, None);
    editor.set_block(material, x, y, z - 3, None, None);
    editor.set_block(material, x, y, z + 3, None, None);
}

/// Helper function to set blocks in a more scattered circular pattern.
fn round3(editor: &mut WorldEditor, material: &'static Lazy<Block>, x: i32, y: i32, z: i32) {
    editor.set_block(material, x + 3, y, z - 1, None, None);
    editor.set_block(material, x + 3, y, z + 1, None, None);
    editor.set_block(material, x + 2, y, z - 2, None, None);
    editor.set_block(material, x + 2, y, z + 2, None, None);
    editor.set_block(material, x + 1, y, z - 3, None, None);
    editor.set_block(material, x + 1, y, z + 3, None, None);
    editor.set_block(material, x - 3, y, z - 1, None, None);
    editor.set_block(material, x - 3, y, z + 1, None, None);
    editor.set_block(material, x - 2, y, z - 2, None, None);
    editor.set_block(material, x - 2, y, z + 2, None, None);
    editor.set_block(material, x - 1, y, z + 3, None, None);
    editor.set_block(material, x - 1, y, z - 3, None, None);
}

/// Function to create different types of trees.
pub fn create_tree(editor: &mut WorldEditor, x: i32, y: i32, z: i32, typetree: u8) {
    let mut blacklist: Vec<&'static Lazy<Block>> = Vec::new();
    blacklist.extend(building_corner_variations());
    blacklist.extend(building_wall_variations());
    blacklist.extend(building_floor_variations());
    blacklist.push(&WATER);

    if editor.check_for_block(x, y - 1, z, None, Some(&blacklist)) {
        return;
    }

    match typetree {
        1 => {
            // Oak tree
            editor.fill_blocks(&OAK_LOG, x, y, z, x, y + 8, z, None, None);
            editor.fill_blocks(&OAK_LEAVES, x - 1, y + 3, z, x - 1, y + 9, z, None, None);
            editor.fill_blocks(&OAK_LEAVES, x + 1, y + 3, z, x + 1, y + 9, z, None, None);
            editor.fill_blocks(&OAK_LEAVES, x, y + 3, z - 1, x, y + 9, z - 1, None, None);
            editor.fill_blocks(&OAK_LEAVES, x, y + 3, z + 1, x, y + 9, z + 1, None, None);
            editor.fill_blocks(&OAK_LEAVES, x, y + 9, z, x, y + 10, z, None, None);
            round1(editor, &OAK_LEAVES, x, y + 8, z);
            round1(editor, &OAK_LEAVES, x, y + 7, z);
            round1(editor, &OAK_LEAVES, x, y + 6, z);
            round1(editor, &OAK_LEAVES, x, y + 5, z);
            round1(editor, &OAK_LEAVES, x, y + 4, z);
            round1(editor, &OAK_LEAVES, x, y + 3, z);
            round2(editor, &OAK_LEAVES, x, y + 7, z);
            round2(editor, &OAK_LEAVES, x, y + 6, z);
            round2(editor, &OAK_LEAVES, x, y + 5, z);
            round2(editor, &OAK_LEAVES, x, y + 4, z);
            round3(editor, &OAK_LEAVES, x, y + 6, z);
            round3(editor, &OAK_LEAVES, x, y + 5, z);
        }
        2 => {
            // Spruce tree
            editor.fill_blocks(&SPRUCE_LOG, x, y, z, x, y + 9, z, None, None);
            editor.fill_blocks(&BIRCH_LEAVES, x - 1, y + 3, z, x - 1, y + 10, z, None, None);
            editor.fill_blocks(&BIRCH_LEAVES, x + 1, y + 3, z, x + 1, y + 10, z, None, None);
            editor.fill_blocks(&BIRCH_LEAVES, x, y + 3, z - 1, x, y + 10, z - 1, None, None);
            editor.fill_blocks(&BIRCH_LEAVES, x, y + 3, z + 1, x, y + 10, z + 1, None, None);
            editor.set_block(&BIRCH_LEAVES, x, y + 10, z, None, None);
            round1(editor, &BIRCH_LEAVES, x, y + 9, z);
            round1(editor, &BIRCH_LEAVES, x, y + 7, z);
            round1(editor, &BIRCH_LEAVES, x, y + 6, z);
            round1(editor, &BIRCH_LEAVES, x, y + 4, z);
            round1(editor, &BIRCH_LEAVES, x, y + 3, z);
            round2(editor, &BIRCH_LEAVES, x, y + 6, z);
            round2(editor, &BIRCH_LEAVES, x, y + 3, z);
        }
        3 => {
            // Birch tree
            editor.fill_blocks(&BIRCH_LOG, x, y, z, x, y + 6, z, None, None);
            editor.fill_blocks(&BIRCH_LEAVES, x - 1, y + 2, z, x - 1, y + 7, z, None, None);
            editor.fill_blocks(&BIRCH_LEAVES, x + 1, y + 2, z, x + 1, y + 7, z, None, None);
            editor.fill_blocks(&BIRCH_LEAVES, x, y + 2, z - 1, x, y + 7, z - 1, None, None);
            editor.fill_blocks(&BIRCH_LEAVES, x, y + 2, z + 1, x, y + 7, z + 1, None, None);
            editor.fill_blocks(&BIRCH_LEAVES, x, y + 7, z, x, y + 8, z, None, None);
            round1(editor, &BIRCH_LEAVES, x, y + 6, z);
            round1(editor, &BIRCH_LEAVES, x, y + 5, z);
            round1(editor, &BIRCH_LEAVES, x, y + 4, z);
            round1(editor, &BIRCH_LEAVES, x, y + 3, z);
            round1(editor, &BIRCH_LEAVES, x, y + 2, z);
            round2(editor, &BIRCH_LEAVES, x, y + 2, z);
            round2(editor, &BIRCH_LEAVES, x, y + 3, z);
            round2(editor, &BIRCH_LEAVES, x, y + 4, z);
        }
        _ => {} // Do nothing if typetree is not recognized
    }
}
