use crate::block_definitions::*;
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

const SPRUCE_LEAVES_FILL: [(Coord, Coord); 6] = [
    ((-1, 3, 0), (-1, 10, 0)),
    ((0, 3, -1), (0, 10, -1)),
    ((1, 3, 0), (1, 10, 0)),
    ((0, 3, -1), (0, 10, -1)),
    ((0, 3, 1), (0, 10, 1)),
    ((0, 11, 0), (0, 11, 0)),
];

const BIRCH_LEAVES_FILL: [(Coord, Coord); 5] = [
    ((-1, 2, 0), (-1, 7, 0)),
    ((1, 2, 0), (1, 7, 0)),
    ((0, 2, -1), (0, 7, -1)),
    ((0, 2, 1), (0, 7, 1)),
    ((0, 7, 0), (0, 8, 0)),
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
}

impl Tree<'_> {
    pub fn create(editor: &mut WorldEditor, (x, y, z): Coord) {
        let mut blacklist: Vec<Block> = Vec::new();
        blacklist.extend(Self::get_building_wall_blocks());
        blacklist.extend(Self::get_building_floor_blocks());
        blacklist.extend(Self::get_structural_blocks());
        blacklist.extend(Self::get_functional_blocks());
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
    }

    fn get_tree(kind: TreeType) -> Self {
        match kind {
            TreeType::Oak => Self {
                // kind,
                log_block: OAK_LOG,
                log_height: 8,
                leaves_block: OAK_LEAVES,
                leaves_fill: &OAK_LEAVES_FILL,
                round_ranges: [
                    (3..=8).rev().collect(),
                    (4..=7).rev().collect(),
                    (5..=6).rev().collect(),
                ],
            },

            TreeType::Spruce => Self {
                // kind,
                log_block: SPRUCE_LOG,
                log_height: 9,
                leaves_block: BIRCH_LEAVES, // TODO Is this correct?
                leaves_fill: &SPRUCE_LEAVES_FILL,
                // TODO can I omit the third empty vec? May cause issues with iter zip
                round_ranges: [vec![9, 7, 6, 4, 3], vec![6, 3], vec![]],
            },

            TreeType::Birch => Self {
                // kind,
                log_block: BIRCH_LOG,
                log_height: 6,
                leaves_block: BIRCH_LEAVES,
                leaves_fill: &BIRCH_LEAVES_FILL,
                round_ranges: [(2..=6).rev().collect(), (2..=4).collect(), vec![]],
            },
        } // match
    } // fn get_tree

    /// Get all possible building wall blocks
    fn get_building_wall_blocks() -> Vec<Block> {
        vec![
            BLACKSTONE,
            BLACK_TERRACOTTA,
            BRICK,
            BROWN_CONCRETE,
            BROWN_TERRACOTTA,
            DEEPSLATE_BRICKS,
            END_STONE_BRICKS,
            GRAY_CONCRETE,
            GRAY_TERRACOTTA,
            LIGHT_BLUE_TERRACOTTA,
            LIGHT_GRAY_CONCRETE,
            MUD_BRICKS,
            NETHER_BRICK,
            NETHERITE_BLOCK,
            POLISHED_ANDESITE,
            POLISHED_BLACKSTONE,
            POLISHED_BLACKSTONE_BRICKS,
            POLISHED_DEEPSLATE,
            POLISHED_GRANITE,
            QUARTZ_BLOCK,
            QUARTZ_BRICKS,
            SANDSTONE,
            SMOOTH_SANDSTONE,
            SMOOTH_STONE,
            STONE_BRICKS,
            WHITE_CONCRETE,
            WHITE_TERRACOTTA,
            ORANGE_TERRACOTTA,
            GREEN_STAINED_HARDENED_CLAY,
            BLUE_TERRACOTTA,
            YELLOW_TERRACOTTA,
            BLACK_CONCRETE,
            WHITE_CONCRETE,
            GRAY_CONCRETE,
            LIGHT_GRAY_CONCRETE,
            BROWN_CONCRETE,
            RED_CONCRETE,
            ORANGE_TERRACOTTA,
            YELLOW_CONCRETE,
            LIME_CONCRETE,
            GREEN_STAINED_HARDENED_CLAY,
            CYAN_CONCRETE,
            LIGHT_BLUE_CONCRETE,
            BLUE_CONCRETE,
            PURPLE_CONCRETE,
            MAGENTA_CONCRETE,
            RED_TERRACOTTA,
        ]
    }

    /// Get all possible building floor blocks
    fn get_building_floor_blocks() -> Vec<Block> {
        vec![
            GRAY_CONCRETE,
            LIGHT_GRAY_CONCRETE,
            WHITE_CONCRETE,
            SMOOTH_STONE,
            POLISHED_ANDESITE,
            STONE_BRICKS,
        ]
    }

    /// Get structural blocks (fences, walls, stairs, slabs, rails, etc.)
    fn get_structural_blocks() -> Vec<Block> {
        vec![
            // Fences
            OAK_FENCE,
            // Walls
            COBBLESTONE_WALL,
            ANDESITE_WALL,
            STONE_BRICK_WALL,
            // Stairs
            OAK_STAIRS,
            // Slabs
            OAK_SLAB,
            STONE_BLOCK_SLAB,
            STONE_BRICK_SLAB,
            // Rails
            RAIL,
            RAIL_NORTH_SOUTH,
            RAIL_EAST_WEST,
            RAIL_ASCENDING_EAST,
            RAIL_ASCENDING_WEST,
            RAIL_ASCENDING_NORTH,
            RAIL_ASCENDING_SOUTH,
            RAIL_NORTH_EAST,
            RAIL_NORTH_WEST,
            RAIL_SOUTH_EAST,
            RAIL_SOUTH_WEST,
            // Doors and trapdoors
            OAK_DOOR,
            DARK_OAK_DOOR_LOWER,
            DARK_OAK_DOOR_UPPER,
            OAK_TRAPDOOR,
            // Ladders
            LADDER,
        ]
    }

    /// Get functional blocks (furniture, decorative items, etc.)
    fn get_functional_blocks() -> Vec<Block> {
        vec![
            // Furniture and functional blocks
            CHEST,
            CRAFTING_TABLE,
            FURNACE,
            ANVIL,
            BREWING_STAND,
            NOTE_BLOCK,
            BOOKSHELF,
            CAULDRON,
            // Beds
            RED_BED_NORTH_HEAD,
            RED_BED_NORTH_FOOT,
            RED_BED_EAST_HEAD,
            RED_BED_EAST_FOOT,
            RED_BED_SOUTH_HEAD,
            RED_BED_SOUTH_FOOT,
            RED_BED_WEST_HEAD,
            RED_BED_WEST_FOOT,
            // Pressure plates and signs
            OAK_PRESSURE_PLATE,
            SIGN,
            // Glass blocks (windows)
            GLASS,
            WHITE_STAINED_GLASS,
            GRAY_STAINED_GLASS,
            LIGHT_GRAY_STAINED_GLASS,
            BROWN_STAINED_GLASS,
            TINTED_GLASS,
            // Carpets
            WHITE_CARPET,
            RED_CARPET,
            // Other structural/building blocks
            IRON_BARS,
            IRON_BLOCK,
            SCAFFOLDING,
            BEDROCK,
        ]
    }
} // impl Tree
