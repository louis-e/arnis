//! Blocks for a surveyed column: what the ground pass puts where OSM left a gap.

use super::classify;
use crate::block_definitions::*;
use crate::block_palette::closest_of;
use crate::colors::RGBTuple;
use crate::world_editor::WorldEditor;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Lawn,
    Paved,
    Hedge,
    Bush,
    Tree,
    /// Built as an element (see `elements`); the ground pass only paves under it.
    Shed,
    /// Built as an element; the ground pass leaves it alone.
    Pool,
    /// Knee-high planting: flowers, perennials, vegetables.
    Bed,
    /// A wooden fence or garden wall the laser saw as a thin line.
    Fence,
    /// Roof seen by the laser; `height` is the roof's.
    Building,
    /// Open ground, neither lawn nor paving: the land cover's.
    Bare,
}

#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub kind: Kind,
    /// Measured height of the object on this block, in metres (0 for ground cover).
    pub height: f32,
    pub color: RGBTuple,
}

impl Cell {
    pub(super) fn decode(c: [u8; 5]) -> Option<Self> {
        let kind = match c[0] {
            classify::LAWN => Kind::Lawn,
            classify::PAVED => Kind::Paved,
            classify::HEDGE => Kind::Hedge,
            classify::BUSH => Kind::Bush,
            classify::TREE => Kind::Tree,
            classify::SHED => Kind::Shed,
            classify::POOL => Kind::Pool,
            classify::BED => Kind::Bed,
            classify::FENCE => Kind::Fence,
            classify::BUILDING => Kind::Building,
            classify::BARE => Kind::Bare,
            _ => return None,
        };
        Some(Cell {
            kind,
            height: f32::from(c[1]) / 5.0,
            color: (c[2], c[3], c[4]),
        })
    }
}

/// Paving materials and their texture's average colour.
const PAVING: &[(RGBTuple, Block)] = &[
    ((159, 159, 159), SMOOTH_STONE),
    ((132, 135, 134), POLISHED_ANDESITE),
    ((122, 121, 122), STONE_BRICKS),
    ((55, 58, 62), GRAY_CONCRETE),
    ((154, 107, 89), POLISHED_GRANITE),
    ((150, 97, 83), BRICK),
    ((137, 104, 79), MUD_BRICKS),
];

/// Surface and under-block for a surveyed column at (x, z) of land cover
/// `cover`, or `None` to keep what the land cover would put there (a pool's
/// own element does its ground).
pub fn ground_blocks(cell: Cell, x: i32, z: i32, cover: u8) -> Option<(Block, Block)> {
    use crate::land_cover::{LC_BUILT_UP, LC_TREE_COVER};
    match cell.kind {
        Kind::Paved | Kind::Shed => {
            // The photo is brighter than any block texture: squeeze its range
            // (dark asphalt ~60 .. pale flags ~190) onto the textures' (~80 .. ~165).
            let (r, g, b) = cell.color;
            let squeeze = |v: u8| (40.0 + 0.7 * f32::from(v)).round() as u8;
            let color = (squeeze(r), squeeze(g), squeeze(b));
            Some((paving_block(closest_of(PAVING, color), x, z), STONE))
        }
        Kind::Pool => None,
        // In town sunlit concrete reads as warm as earth: the town's paving
        // mix, never a guessed garden's lawn.
        Kind::Bare if cover == LC_BUILT_UP => Some((paving_block(STONE_BRICKS, x, z), STONE)),
        Kind::Bare => Some(open_ground(x, z)),
        // Roof the laser saw past a mapped outline: an eave over the yard, or a
        // building OSM lacks. Either way the ground under it is not lawn.
        Kind::Building => Some((POLISHED_ANDESITE, STONE)),
        // Under a wood's crowns and shrubs the forest floor stays.
        Kind::Tree | Kind::Bush | Kind::Hedge | Kind::Bed | Kind::Fence
            if cover == LC_TREE_COVER =>
        {
            None
        }
        _ => Some((GRASS_BLOCK, DIRT)),
    }
}

/// Whether the laser's ground takes over a column of this land cover: towns,
/// gardens, parks and woods. Fields, beaches, bare rock, wetland and snow keep
/// the land cover's own ground, which the laser cannot tell from paving.
pub fn claims_cover(cover: u8) -> bool {
    use crate::land_cover::{LC_BUILT_UP, LC_GRASSLAND, LC_SHRUBLAND, LC_TREE_COVER};
    matches!(
        cover,
        LC_BUILT_UP | LC_GRASSLAND | LC_SHRUBLAND | LC_TREE_COVER
    )
}

/// Grey paving as the photo's brightness reads it, each a mix laid block by
/// block as main lays unmapped town ground, so a yard is neither one flat block
/// nor a patchwork: pale paving keeps some smooth slabs among its pavers, mid
/// grey some flags, and the rest is main's own mix. Weights in hundredths.
const PALE_PAVING: &[(u64, Block)] = &[
    (50, STONE_BRICKS),
    (22, SMOOTH_STONE),
    (12, POLISHED_ANDESITE),
    (8, CRACKED_STONE_BRICKS),
    (4, STONE),
    (4, COBBLESTONE),
];
const MID_PAVING: &[(u64, Block)] = &[
    (55, STONE_BRICKS),
    (15, POLISHED_ANDESITE),
    (13, CRACKED_STONE_BRICKS),
    (5, ANDESITE),
    (5, STONE),
    (7, COBBLESTONE),
];
/// Main's mix for unmapped town ground (`LC_BUILT_UP`), block for block.
const TOWN_PAVING: &[(u64, Block)] = &[
    (72, STONE_BRICKS),
    (15, CRACKED_STONE_BRICKS),
    (5, STONE),
    (8, COBBLESTONE),
];

/// The block of paving at (x, z) whose colour reads nearest `nearest`: grey as
/// a mix, asphalt and brick as themselves.
fn paving_block(nearest: Block, x: i32, z: i32) -> Block {
    let mix = match nearest {
        SMOOTH_STONE => PALE_PAVING,
        POLISHED_ANDESITE => MID_PAVING,
        STONE_BRICKS => TOWN_PAVING,
        other => return other,
    };
    let mut v = crate::land_cover::coord_hash(x, z) % 100;
    for &(w, block) in mix {
        if v < w {
            return block;
        }
        v -= w;
    }
    mix[0].1
}

/// Open ground: earth, worn in patches to dirt, with gravel here and there.
fn open_ground(x: i32, z: i32) -> (Block, Block) {
    let n = crate::ground_generation::ground_noise(x, z, 6, 0x0BA2_E000);
    if n < 0.55 {
        (COARSE_DIRT, DIRT)
    } else if n < 0.85 {
        (DIRT, DIRT)
    } else {
        (GRAVEL, STONE)
    }
}

/// Hedge species, one per stretch: thuja (spruce), hornbeam or privet (oak), yew
/// (dark oak). Keyed on twelve-block cells so a hedge does not flicker.
fn hedge_leaf(x: i32, z: i32) -> Block {
    let h = crate::land_cover::coord_hash(x.div_euclid(12), z.div_euclid(12)) % 100;
    if h < 45 {
        SPRUCE_LEAVES
    } else if h < 85 {
        OAK_LEAVES
    } else {
        DARK_OAK_LEAVES
    }
}

/// The plant on a block of a bed: drifts of one kind a few blocks across, mostly
/// flowers, with grass between them.
fn bed_plant(x: i32, z: i32) -> Block {
    const DRIFTS: &[Block] = &[
        RED_FLOWER,
        YELLOW_FLOWER,
        WHITE_FLOWER,
        BLUE_FLOWER,
        PINK_TULIP,
        RED_TULIP,
        OXEYE_DAISY,
        CORNFLOWER,
        ALLIUM,
        FERN,
    ];
    let own = crate::land_cover::coord_hash(x, z) % 100;
    if own < 20 {
        return GRASS;
    }
    let drift = crate::land_cover::coord_hash(x.div_euclid(3) ^ 0x5B, z.div_euclid(3));
    DRIFTS[(drift % DRIFTS.len() as u64) as usize]
}

/// Whole blocks of a measured height, between `min` and `max`.
fn blocks_tall(height: f32, min: i32, max: i32) -> i32 {
    (height.round() as i32).clamp(min, max)
}

/// Whether a lawn block gets a tuft of grass: a mown lawn, a tuft here and there.
pub fn lawn_tuft(x: i32, z: i32) -> bool {
    crate::land_cover::coord_hash(x ^ 0x6A, z ^ 0x2D) % 100 < 6
}

/// Hedges, bushes, beds and fences on a column the survey owns. Never replaces a
/// block: anything already above the ground (a mapped bench, a lamp, a tree
/// trunk) stays, and the column is left as it is from there up.
pub fn place_objects(editor: &mut WorldEditor, cell: Cell, x: i32, ground_y: i32, z: i32) {
    let column = |editor: &mut WorldEditor, tall: i32, block: &dyn Fn(i32) -> Block| {
        for dy in 1..=tall {
            if editor.block_exists_absolute(x, ground_y + dy, z) {
                break;
            }
            editor.set_block_absolute(block(ground_y + dy), x, ground_y + dy, z, None, None);
        }
    };
    match cell.kind {
        // Two blocks at least: that is what reads as a hedge in the game, and what
        // a mapped barrier=hedge gets.
        Kind::Hedge => {
            let leaf = hedge_leaf(x, z);
            column(editor, blocks_tall(cell.height, 2, 4), &|_| leaf);
        }
        Kind::Bush => {
            let species = crate::element_processing::bush::shrubbery_species(editor, x, z);
            column(editor, blocks_tall(cell.height, 1, 3), &|y| {
                crate::element_processing::bush::shrubbery_leaf(species, x, y, z)
            });
        }
        Kind::Bed => {
            let plant = bed_plant(x, z);
            column(editor, 1, &|_| plant);
        }
        Kind::Fence => column(editor, blocks_tall(cell.height, 1, 2), &|_| SPRUCE_FENCE),
        Kind::Lawn
        | Kind::Paved
        | Kind::Tree
        | Kind::Shed
        | Kind::Pool
        | Kind::Building
        | Kind::Bare => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::land_cover::{LC_BUILT_UP, LC_GRASSLAND, LC_TREE_COVER};
    use std::collections::HashMap;

    fn cell(kind: Kind, (r, g, b): RGBTuple) -> Cell {
        Cell {
            kind,
            height: 0.0,
            color: (r, g, b),
        }
    }

    /// How many of a 60 x 60 area's surfaces a paved patch of this colour gets
    /// of each block, in hundredths.
    fn shares(color: RGBTuple) -> HashMap<Block, usize> {
        let mut n: HashMap<Block, usize> = HashMap::new();
        for x in 0..60 {
            for z in 0..60 {
                let (top, _) = ground_blocks(cell(Kind::Paved, color), x, z, LC_BUILT_UP).unwrap();
                *n.entry(top).or_default() += 1;
            }
        }
        n.values_mut().for_each(|v| *v = *v * 100 / 3600);
        n
    }

    #[test]
    fn paving_follows_brightness_and_keeps_red_only_when_red() {
        // Asphalt and brick stay what they are.
        assert_eq!(shares((30, 30, 30)), HashMap::from([(GRAY_CONCRETE, 100)]));
        assert_eq!(
            shares((170, 95, 70)),
            HashMap::from([(POLISHED_GRANITE, 100)])
        );
        // Grey is a mix, never one flat block: pale paving keeps some smooth
        // slabs, the darker grey is main's own town ground.
        let pale = shares((175, 175, 175));
        let mid = shares((145, 145, 145));
        let dark = shares((115, 115, 115));
        for mix in [&pale, &mid, &dark] {
            assert!(mix.values().all(|&v| v < 80), "{mix:?}");
            assert!(mix[&STONE_BRICKS] > 40, "{mix:?}");
        }
        assert!((15..30).contains(&pale[&SMOOTH_STONE]), "{pale:?}");
        assert!(mid[&POLISHED_ANDESITE] > 8, "{mid:?}");
        assert!(!dark.contains_key(&SMOOTH_STONE) && dark.contains_key(&COBBLESTONE));
    }

    #[test]
    fn open_ground_is_earth_except_in_town() {
        let bare = cell(Kind::Bare, (190, 175, 155));
        for (x, z) in [(3, 4), (17, 9), (40, 2)] {
            let (top, _) = ground_blocks(bare, x, z, LC_BUILT_UP).unwrap();
            assert!(TOWN_PAVING.iter().any(|&(_, b)| b == top), "{top:?}");
        }
        let earth: Vec<Block> = (0..40)
            .flat_map(|x| (0..40).map(move |z| (x, z)))
            .map(|(x, z)| ground_blocks(bare, x, z, LC_GRASSLAND).unwrap().0)
            .collect();
        assert!(earth
            .iter()
            .all(|b| [COARSE_DIRT, DIRT, GRAVEL].contains(b)));
        assert!(earth.contains(&COARSE_DIRT) && earth.contains(&GRAVEL));
    }

    #[test]
    fn a_woods_floor_stays_under_its_trees() {
        let tree = cell(Kind::Tree, (60, 90, 50));
        assert_eq!(ground_blocks(tree, 0, 0, LC_TREE_COVER), None);
        assert_eq!(
            ground_blocks(tree, 0, 0, LC_BUILT_UP),
            Some((GRASS_BLOCK, DIRT))
        );
        // A lawn in a wood's cover is a clearing.
        assert_eq!(
            ground_blocks(cell(Kind::Lawn, (60, 120, 50)), 0, 0, LC_TREE_COVER),
            Some((GRASS_BLOCK, DIRT))
        );
    }

    #[test]
    fn the_laser_takes_town_and_green_ground_only() {
        use crate::land_cover::{LC_BARE, LC_BEACH, LC_CROPLAND, LC_SHRUBLAND, LC_WETLAND};
        for cover in [LC_BUILT_UP, LC_GRASSLAND, LC_SHRUBLAND, LC_TREE_COVER] {
            assert!(claims_cover(cover), "{cover}");
        }
        for cover in [LC_CROPLAND, LC_BEACH, LC_BARE, LC_WETLAND] {
            assert!(!claims_cover(cover), "{cover}");
        }
    }

    #[test]
    fn hedges_are_two_blocks_at_least() {
        assert_eq!(blocks_tall(1.1, 2, 4), 2);
        assert_eq!(blocks_tall(3.2, 2, 4), 3);
        assert_eq!(blocks_tall(9.0, 2, 4), 4);
    }
}
