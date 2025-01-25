use crate::block_definitions::*;
use crate::world_editor::WorldEditor;

/// A circular pattern around a central point.
const ROUND1_COORDS: [(i32, i32, i32); 8] = [
    (-2, 0, 0),
    (2, 0, 0),
    (0, 0, -2),
    (0, 0, 2),
    (-1, 0, -1),
    (1, 0, 1),
    (1, 0, -1),
    (-1, 0, 1),
];

/// A wider circular pattern.
const ROUND2_COORDS: [(i32, i32, i32); 12] = [
    (3, 0, 0),
    (2, 0, -1),
    (2, 0, 1),
    (1, 0, -2),
    (1, 0, 2),
    (-3, 0, 0),
    (-2, 0, -1),
    (-2, 0, 1),
    (-1, 0, 2),
    (-1, 0, -2),
    (0, 0, -3),
    (0, 0, 3),
];

/// A more scattered circular pattern.
const ROUND3_COORDS: [(i32, i32, i32); 12] = [
    (3, 0, -1),
    (3, 0, 1),
    (2, 0, -2),
    (2, 0, 2),
    (1, 0, -3),
    (1, 0, 3),
    (-3, 0, -1),
    (-3, 0, 1),
    (-2, 0, -2),
    (-2, 0, 2),
    (-1, 0, 3),
    (-1, 0, -3),
];

/// Helper function to set blocks in various patterns.
fn round(editor: &mut WorldEditor, material: Block, x: i32, y: i32, z: i32, block_pattern: &[(i32, i32, i32)]) {
    for (i, j, k) in block_pattern {
        editor.set_block(material, x + i, y + j, z + k, None, None);
    }
}

/// Function to create different types of trees.
pub fn create_tree(editor: &mut WorldEditor, x: i32, y: i32, z: i32, typetree: u8, snow: bool) {
    let mut blacklist: Vec<Block> = Vec::new();
    blacklist.extend(building_corner_variations());
    blacklist.extend(building_wall_variations());
    blacklist.extend(building_floor_variations());
    blacklist.push(WATER);

    if editor.check_for_block(x, y - 1, z, None, Some(&blacklist)) {
        return;
    }

    match typetree {
        1 => {
            // Oak tree
            editor.fill_blocks(OAK_LOG, x, y, z, x, y + 8, z, None, None);
            let leaves_fill_coords = [
                ((-1, 3, 0), (-1, 9, 0)),
                ((1, 3, 0), (1, 9, 0)),
                ((0, 3, -1), (0, 9, -1)),
                ((0, 3, 1), (0, 9, 1)),
                ((0, 9, 0), (0, 10, 0)),
            ];
            for ((x1, y1, z1), (x2, y2, z2)) in leaves_fill_coords {
                editor.fill_blocks(OAK_LEAVES, x1, y1, z1, x2, y2, z2, None, None);
            }

            for i in (3..=8).rev() {
                round(editor, OAK_LEAVES, x, y + i, z, &ROUND1_COORDS);
            }

            for i in (4..=7).rev() {
                round(editor, OAK_LEAVES, x, y + i, z, &ROUND2_COORDS);
            }

            for i in (5..=6).rev() {
                round(editor, OAK_LEAVES, x, y + i, z, &ROUND3_COORDS);
            }

            if snow {
                let snow_coords = [
                    (0, 11, 0), 
                    (1, 10, 0),
                    (-1, 10, 0),
                    (0, 10, -1),
                    (0, 10, 1),
                ];
                for (x, y, z) in snow_coords {
                    editor.set_block(SNOW_LAYER, x, y, z, None, None);
                }

                for i in (6..=9).rev() {
                    round(editor, SNOW_LAYER, x, y + i, z, &ROUND1_COORDS);
                }

                for i in (5..=8).rev() {
                    round(editor, SNOW_LAYER, x, y + i, z, &ROUND2_COORDS);
                }

                for i in (6..=7).rev() {
                    round(editor, SNOW_LAYER, x, y + i, z, &ROUND3_COORDS);
                }
            }
        }
        2 => {
            // Spruce tree
            editor.fill_blocks(SPRUCE_LOG, x, y, z, x, y + 9, z, None, None);
            editor.fill_blocks(BIRCH_LEAVES, x - 1, y + 3, z, x - 1, y + 10, z, None, None);
            editor.fill_blocks(BIRCH_LEAVES, x + 1, y + 3, z, x + 1, y + 10, z, None, None);
            editor.fill_blocks(BIRCH_LEAVES, x, y + 3, z - 1, x, y + 10, z - 1, None, None);
            editor.fill_blocks(BIRCH_LEAVES, x, y + 3, z + 1, x, y + 10, z + 1, None, None);
            editor.set_block(BIRCH_LEAVES, x, y + 10, z, None, None);

            for i in [9, 7, 6, 4, 3] {
                round(editor, BIRCH_LEAVES, x, y + i, z, &ROUND1_COORDS);
            }

            for i in [6, 3] {
                round(editor, BIRCH_LEAVES, x, y + i, z, &ROUND2_COORDS);
            }

            if snow {
                editor.set_block(SNOW_LAYER, x, y + 11, z, None, None);
                editor.set_block(SNOW_LAYER, x + 1, y + 11, z, None, None);
                editor.set_block(SNOW_LAYER, x - 1, y + 11, z, None, None);
                editor.set_block(SNOW_LAYER, x, y + 11, z - 1, None, None);
                editor.set_block(SNOW_LAYER, x, y + 11, z + 1, None, None);

                for i in [10, 8, 7, 5, 4] {
                    round(editor, SNOW_LAYER, x, y + i, z, &ROUND1_COORDS);
                }

                for i in [7, 4] {
                    round(editor, SNOW_LAYER, x, y + i, z, &ROUND2_COORDS);
                }
            }
        }
        3 => {
            // Birch tree
            editor.fill_blocks(BIRCH_LOG, x, y, z, x, y + 6, z, None, None);
            editor.fill_blocks(BIRCH_LEAVES, x - 1, y + 2, z, x - 1, y + 7, z, None, None);
            editor.fill_blocks(BIRCH_LEAVES, x + 1, y + 2, z, x + 1, y + 7, z, None, None);
            editor.fill_blocks(BIRCH_LEAVES, x, y + 2, z - 1, x, y + 7, z - 1, None, None);
            editor.fill_blocks(BIRCH_LEAVES, x, y + 2, z + 1, x, y + 7, z + 1, None, None);
            editor.fill_blocks(BIRCH_LEAVES, x, y + 7, z, x, y + 8, z, None, None);

            for i in (2..=6).rev() {
                round(editor, BIRCH_LEAVES, x, y + i, z, &ROUND1_COORDS);
            }

            for i in 2..=4 {
                round(editor, BIRCH_LEAVES, x, y + i, z, &ROUND2_COORDS);
            }

            if snow {
                editor.set_block(SNOW_LAYER, x, y + 9, z, None, None);
                editor.set_block(SNOW_LAYER, x + 1, y + 8, z, None, None);
                editor.set_block(SNOW_LAYER, x - 1, y + 8, z, None, None);
                editor.set_block(SNOW_LAYER, x, y + 8, z - 1, None, None);
                editor.set_block(SNOW_LAYER, x, y + 8, z + 1, None, None);

                for i in (3..=7).rev() {
                    round(editor, SNOW_LAYER, x, y + i, z, &ROUND1_COORDS);
                }

                for i in 3..=5 {
                    round(editor, SNOW_LAYER, x, y + i, z, &ROUND2_COORDS);
                }
            }
        }
        _ => {} // Do nothing if typetree is not recognized
    }
}
