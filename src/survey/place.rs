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

/// Surface and under-block for a surveyed column, or `None` to keep what the
/// land cover would put there (a pool's own element does its ground).
pub fn ground_blocks(cell: Cell) -> Option<(Block, Block)> {
    match cell.kind {
        Kind::Paved | Kind::Shed => {
            // The photo is brighter than any block texture: squeeze its range
            // (dark asphalt ~60 .. pale flags ~190) onto the textures' (~80 .. ~165).
            let (r, g, b) = cell.color;
            let squeeze = |v: u8| (40.0 + 0.7 * f32::from(v)).round() as u8;
            let color = (squeeze(r), squeeze(g), squeeze(b));
            Some((closest_of(PAVING, color), STONE))
        }
        Kind::Pool => None,
        // Roof the laser saw past a mapped outline: an eave over the yard, or a
        // building OSM lacks. Either way the ground under it is not lawn.
        Kind::Building => Some((POLISHED_ANDESITE, STONE)),
        _ => Some((GRASS_BLOCK, DIRT)),
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
        Kind::Lawn | Kind::Paved | Kind::Tree | Kind::Shed | Kind::Pool | Kind::Building => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paving_follows_brightness_and_keeps_red_only_when_red() {
        let cell = |r, g, b| Cell {
            kind: Kind::Paved,
            height: 0.0,
            color: (r, g, b),
        };
        assert_eq!(ground_blocks(cell(175, 175, 175)).unwrap().0, SMOOTH_STONE);
        assert_eq!(
            ground_blocks(cell(145, 145, 145)).unwrap().0,
            POLISHED_ANDESITE
        );
        assert_eq!(ground_blocks(cell(115, 115, 115)).unwrap().0, STONE_BRICKS);
        assert_eq!(ground_blocks(cell(30, 30, 30)).unwrap().0, GRAY_CONCRETE);
        assert_eq!(
            ground_blocks(cell(170, 95, 70)).unwrap().0,
            POLISHED_GRANITE
        );
    }

    #[test]
    fn hedges_are_two_blocks_at_least() {
        assert_eq!(blocks_tall(1.1, 2, 4), 2);
        assert_eq!(blocks_tall(3.2, 2, 4), 3);
        assert_eq!(blocks_tall(9.0, 2, 4), 4);
    }
}
