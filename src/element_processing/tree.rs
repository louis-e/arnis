use crate::block_definitions::*;
use crate::world_editor::WorldEditor;

/// A circular pattern around a central point.
const ROUND1_PATTERN: [(i32, i32, i32); 8] = [
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
const ROUND2_PATTERN: [(i32, i32, i32); 12] = [
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
const ROUND3_PATTERN: [(i32, i32, i32); 12] = [
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
pub fn create_tree(editor: &mut WorldEditor, (x, y, z): (i32, i32, i32), tree_type: u8, snow: bool) {
    // TODO this gets created every time fn is called.
    let mut blacklist: Vec<Block> = Vec::new();
    blacklist.extend(building_corner_variations());
    blacklist.extend(building_wall_variations());
    blacklist.extend(building_floor_variations());
    blacklist.push(WATER);

    if editor.check_for_block(x, y - 1, z, None, Some(&blacklist)) {
        return;
    }

    match tree_type {
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
            for ((i1, j1, k1), (i2, j2, k2)) in leaves_fill_coords {
                editor.fill_blocks(OAK_LEAVES, x + i1, y + j1, z + k1, x + i2, y + j2, z + k2, None, None);
            }

            for j in (3..=8).rev() {
                round(editor, OAK_LEAVES, x, y + j, z, &ROUND1_PATTERN);
            }

            for j in (4..=7).rev() {
                round(editor, OAK_LEAVES, x, y + j, z, &ROUND2_PATTERN);
            }

            for j in (5..=6).rev() {
                round(editor, OAK_LEAVES, x, y + j, z, &ROUND3_PATTERN);
            }

            if snow {
                let snow_coords = [
                    (0, 11, 0), 
                    (1, 10, 0),
                    (-1, 10, 0),
                    (0, 10, -1),
                    (0, 10, 1),
                ];
                for (i, j, k) in snow_coords {
                    editor.set_block(SNOW_LAYER, x + i, y + j, z + k, None, None);
                }

                for j in (6..=9).rev() {
                    round(editor, SNOW_LAYER, x, y + j, z, &ROUND1_PATTERN);
                }

                for j in (5..=8).rev() {
                    round(editor, SNOW_LAYER, x, y + j, z, &ROUND2_PATTERN);
                }

                for j in (6..=7).rev() {
                    round(editor, SNOW_LAYER, x, y + j, z, &ROUND3_PATTERN);
                }
            }
        }
        2 => {
            // Spruce tree
            editor.fill_blocks(SPRUCE_LOG, x, y, z, x, y + 9, z, None, None);
            let birch_leaves_fill = [
                ((-1, 3, 0), (-1, 10, 0)),
                ((0, 3, -1), (0, 10, -1)),
                ((1, 3, 0), (1, 10, 0)),
                ((0, 3, -1), (0, 10, -1)),
                ((0, 3, 1), (0, 10, 1)),
            ];
            for ((i1, j1, k1), (i2, j2, k2)) in birch_leaves_fill {
                editor.fill_blocks(BIRCH_LEAVES, x + i1, y + j1, z + k1, x + i2, y + j2, z + k2, None, None);
            }
            editor.set_block(BIRCH_LEAVES, x, y + 10, z, None, None);

            for j in [9, 7, 6, 4, 3] {
                round(editor, BIRCH_LEAVES, x, y + j, z, &ROUND1_PATTERN);
            }

            for j in [6, 3] {
                round(editor, BIRCH_LEAVES, x, y + j, z, &ROUND2_PATTERN);
            }

            if snow {
                let snow_fill = [
                    (0, 11, 0),
                    (1, 11, 0),
                    (-1, 11, 0),
                    (0, 11, -1),
                    (0, 11, 1),
                ];
                for (i, j, k) in snow_fill {
                    editor.set_block(SNOW_LAYER, x + i, y + j, z + k, None, None);
                }

                for j in [10, 8, 7, 5, 4] {
                    round(editor, SNOW_LAYER, x, y + j, z, &ROUND1_PATTERN);
                }

                for j in [7, 4] {
                    round(editor, SNOW_LAYER, x, y + j, z, &ROUND2_PATTERN);
                }
            }
        }
        3 => {
            // Birch tree
            editor.fill_blocks(BIRCH_LOG, x, y, z, x, y + 6, z, None, None);
            let leaves_fills = [
                ((-1, 2, 0), (-1, 7, 0)),
                ((1, 2, 0), (1, 7, 0)),
                ((0, 2, -1), (0, 7, -1)),
                ((0, 2, 1), (0, 7, 1)),
                ((0, 7, 0), (0, 8, 0)),
            ];
            for ((i1, j1, k1), (i2, j2, k2)) in leaves_fills {
                editor.fill_blocks(BIRCH_LEAVES, x + i1, y + j1, z + k1, x + i2, y + j2, z + k2, None, None);
            }

            for j in (2..=6).rev() {
                round(editor, BIRCH_LEAVES, x, y + j, z, &ROUND1_PATTERN);
            }

            for j in 2..=4 {
                round(editor, BIRCH_LEAVES, x, y + j, z, &ROUND2_PATTERN);
            }

            if snow {
                let snow_coords = [
                    (0, 9, 0),
                    (1, 8, 0),
                    (-1, 8, 0),
                    (0, 8, -1),
                    (0, 8, 1),
                ];
                for (i, j, k) in snow_coords {
                    editor.set_block(SNOW_LAYER, x + i, y + j, z + k, None, None);
                }

                for j in (3..=7).rev() {
                    round(editor, SNOW_LAYER, x, y + j, z, &ROUND1_PATTERN);
                }

                for j in 3..=5 {
                    round(editor, SNOW_LAYER, x, y + j, z, &ROUND2_PATTERN);
                }
            }
        }
        _ => {} // Do nothing if typetree is not recognized
    }
}
