use crate::block_definitions::{Block, BLOCKS};
use crate::world_editor::WorldEditor;
use rand::Rng;

type Coord = (i32, i32, i32);

// TODO all this data would probably be better suited in a TOML file or something.

/// A circular pattern around a central point.
#[rustfmt::skip]
const ROUND1_PATTERN: [Coord; 8] = [
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
const ROUND2_PATTERN: [Coord; 12] = [
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
const ROUND3_PATTERN: [Coord; 12] = [
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

/// Used for iterating over each of the round patterns
const ROUND_PATTERNS: [&[Coord]; 3] = [&ROUND1_PATTERN, &ROUND2_PATTERN, &ROUND3_PATTERN];

//////////////////////////////////////////////////

const OAK_LEAVES_FILL: [(Coord, Coord); 5] = [
    ((-1, 3, 0), (-1, 9, 0)),
    ((1, 3, 0), (1, 9, 0)),
    ((0, 3, -1), (0, 9, -1)),
    ((0, 3, 1), (0, 9, 1)),
    ((0, 9, 0), (0, 10, 0)),
];

const SPRUCE_LEAVES_FILL: [(Coord, Coord); 5] = [
    ((-1, 3, 0), (-1, 10, 0)),
    ((0, 3, -1), (0, 10, -1)),
    ((1, 3, 0), (1, 10, 0)),
    ((0, 3, -1), (0, 10, -1)),
    ((0, 3, 1), (0, 10, 1)),
];

const BIRCH_LEAVES_FILL: [(Coord, Coord); 5] = [
    ((-1, 2, 0), (-1, 7, 0)),
    ((1, 2, 0), (1, 7, 0)),
    ((0, 2, -1), (0, 7, -1)),
    ((0, 2, 1), (0, 7, 1)),
    ((0, 7, 0), (0, 8, 0)),
];

//////////////////////////////////////////////////

#[rustfmt::skip]
const OAK_SNOW_LAYERS: [Coord; 5] = [
    (0, 11, 0), 
    (1, 10, 0),
    (-1, 10, 0),
    (0, 10, -1),
    (0, 10, 1),
];

#[rustfmt::skip]
const SPRUCE_SNOW_LAYERS: [Coord; 5] = [
    (0, 11, 0),
    (1, 11, 0),
    (-1, 11, 0),
    (0, 11, -1),
    (0, 11, 1),
];

#[rustfmt::skip]
const BIRCH_SNOW_LAYERS: [Coord; 5] = [
    (0, 9, 0),
    (1, 8, 0),
    (-1, 8, 0),
    (0, 8, -1),
    (0, 8, 1),
];

//////////////////////////////////////////////////

/// Helper function to set blocks in various patterns.
fn round(editor: &mut WorldEditor, material: Block, (x, y, z): Coord, block_pattern: &[Coord]) {
    for (i, j, k) in block_pattern {
        editor.set_block(material, x + i, y + j, z + k, None, None);
    }
}

pub enum TreeType {
    Oak,
    Spruce,
    Birch,
}

// TODO what should be moved in, and what should be referenced?
pub struct Tree<'a> {
    // kind: TreeType, // NOTE: Not actually necessary to store!
    log_block: Block,
    log_height: i32,
    leaves_block: Block,
    leaves_fill: &'a [(Coord, Coord)],
    round_ranges: [Vec<i32>; 3],
    snow_layer: [Coord; 5],
    snow_ranges: [Vec<i32>; 3],
}

impl Tree<'_> {
    pub fn create(editor: &mut WorldEditor, (x, y, z): Coord, snow: bool) {
        let mut blacklist: Vec<Block> = Vec::new();
        blacklist.extend(building_corner_variations());
        blacklist.extend(building_wall_variations());
        blacklist.extend(building_floor_variations());
        blacklist.push(WATER);

        let mut rng = rand::thread_rng();

        let tree = Self::get_tree(match rng.gen_range(1..=3) {
            1 => TreeType::Oak,
            2 => TreeType::Spruce,
            3 => TreeType::Birch,
            _ => unreachable!(),
        });

        // Build the logs
        editor.fill_blocks(
            tree.log_block,
            x,
            y,
            z,
            x,
            y + tree.log_height,
            z,
            None,
            Some(&blacklist),
        );

        // Fill in the leaves
        for ((i1, j1, k1), (i2, j2, k2)) in tree.leaves_fill {
            editor.fill_blocks(
                tree.leaves_block,
                x + i1,
                y + j1,
                z + k1,
                x + i2,
                y + j2,
                z + k2,
                None,
                None,
            );
        }

        // Do the three rounds
        for (round_range, round_pattern) in tree.round_ranges.iter().zip(ROUND_PATTERNS) {
            for offset in round_range {
                round(editor, tree.leaves_block, (x, y + offset, z), round_pattern);
            }
        }

        if !snow {
            return;
        }

        // Do the snow layers now
        for (i, j, k) in tree.snow_layer {
            editor.set_block(SNOW_LAYER, x + i, y + j, z + k, None, None);
        }

        // Snow rounds
        for (round_range, round_pattern) in tree.snow_ranges.iter().zip(ROUND_PATTERNS) {
            for offset in round_range {
                round(editor, SNOW_LAYER, (x, y + offset, z), round_pattern);
            }
        }
    }

    fn get_tree(kind: TreeType) -> Self {
        match kind {
            TreeType::Oak => Self {
                // kind,
                log_block: &*BLOCKS.by_name("oak_log").unwrap(),
                log_height: 8,
                leaves_block: &*BLOCKS.by_name("oak_leaves").unwrap(),
                leaves_fill: &OAK_LEAVES_FILL,
                round_ranges: [
                    (3..=8).rev().collect(),
                    (4..=7).rev().collect(),
                    (5..=6).rev().collect(),
                ],
                snow_layer: OAK_SNOW_LAYERS,
                snow_ranges: [
                    (6..=9).rev().collect(),
                    (5..=8).rev().collect(),
                    (6..=7).rev().collect(),
                ],
            },

            TreeType::Spruce => Self {
                // kind,
                log_block: &*BLOCKS.by_name("spruce_log").unwrap(),
                log_height: 9,
                leaves_block: &*BLOCKS.by_name("birch_leaves").unwrap(), // TODO Is this correct?
                leaves_fill: &SPRUCE_LEAVES_FILL,
                // TODO can I omit the third empty vec? May cause issues with iter zip
                round_ranges: [vec![9, 7, 6, 4, 3], vec![6, 3], vec![]],
                snow_layer: SPRUCE_SNOW_LAYERS,
                snow_ranges: [vec![10, 8, 7, 5, 4], vec![7, 4], vec![]],
            },

            TreeType::Birch => Self {
                // kind,
                log_block: &*BLOCKS.by_name("birch_log").unwrap(),
                log_height: 6,
                leaves_block: &*BLOCKS.by_name("birch_leaves").unwrap(),
                leaves_fill: &BIRCH_LEAVES_FILL,
                round_ranges: [(2..=6).rev().collect(), (2..=4).collect(), vec![]],
                snow_layer: BIRCH_SNOW_LAYERS,
                snow_ranges: [(3..=7).rev().collect(), (3..=5).collect(), vec![]],
            },
        } // match
    } // fn get_tree
} // impl Tree
