//! Bushes: small leaf clumps in a few shapes, with species that suit the region and
//! drift across an area, in place of a single cube of oak leaves.
//!
//! Shape, species and the extra cells are keyed on world coordinates, so a bush comes
//! out the same whichever tile draws it.

use crate::block_definitions::*;
use crate::climate::Climate;
use crate::ecoregion::EcoBiome;
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

/// Where a bush grows, which sets the shapes it takes and how much azalea it carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BushKind {
    /// Scrub, woodland and wild ground: every shape.
    Wild,
    /// Parks, lawns and cemeteries: rounded shapes, more flowering azalea.
    Garden,
    /// Fields, heath and dry ground: one or two blocks.
    Low,
}

/// Every leaf block a bush is made of. All are persistent, so a bush with no log keeps.
pub(crate) const BUSH_LEAVES: &[Block] = &[
    OAK_LEAVES,
    BIRCH_LEAVES,
    SPRUCE_LEAVES,
    DARK_OAK_LEAVES,
    JUNGLE_LEAVES,
    ACACIA_LEAVES,
    AZALEA_LEAVES,
    FLOWERING_AZALEA_LEAVES,
];

/// Ground a bush may spread onto beside its root. Empty ground counts as well,
/// since the ground pass fills it from land cover after the elements.
const SOIL: &[Block] = &[
    GRASS_BLOCK,
    DIRT,
    COARSE_DIRT,
    PODZOL,
    MOSS_BLOCK,
    SNOWY_GRASS_BLOCK,
    SNOWY_PODZOL,
    MUD,
];

/// Leaf offsets (dx, dy, dz) from the root, dy 0 being the block above the ground.
/// A raised block only grows over a cell whose lower block was placed.
type Shape = &'static [(i32, i32, i32)];

const SINGLE: Shape = &[(0, 0, 0)];
const TALL: Shape = &[(0, 0, 0), (0, 1, 0)];
const PAIR: Shape = &[(0, 0, 0), (1, 0, 0)];
const CLUMP: Shape = &[(0, 0, 0), (1, 0, 0), (0, 0, 1), (0, 1, 0)];
const MOUND: Shape = &[
    (0, 0, 0),
    (1, 0, 0),
    (-1, 0, 0),
    (0, 0, 1),
    (0, 0, -1),
    (0, 1, 0),
];
const WIDE: Shape = &[
    (0, 0, 0),
    (1, 0, 0),
    (0, 0, 1),
    (1, 0, 1),
    (0, 1, 0),
    (1, 1, 1),
];
const SPRAWL: Shape = &[(0, 0, 0), (1, 0, 0), (1, 0, 1), (2, 0, 1), (-1, 0, 0)];

const WILD_SHAPES: &[(Shape, u32)] = &[
    (SINGLE, 25),
    (TALL, 15),
    (PAIR, 15),
    (CLUMP, 18),
    (MOUND, 12),
    (WIDE, 7),
    (SPRAWL, 8),
];
const GARDEN_SHAPES: &[(Shape, u32)] = &[
    (SINGLE, 10),
    (TALL, 15),
    (CLUMP, 25),
    (MOUND, 30),
    (WIDE, 20),
];
const LOW_SHAPES: &[(Shape, u32)] = &[(SINGLE, 55), (PAIR, 30), (TALL, 15)];

/// Regional shrub flora.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Flora {
    Broadleaf,
    Conifer,
    Tropical,
    Dry,
    Mediterranean,
    Grassland,
}

fn flora_for_ecoregion(biome: EcoBiome) -> Flora {
    use EcoBiome::*;
    match biome {
        MoistTropical | TropicalConifer | Flooded | Mangroves => Flora::Tropical,
        DryTropical | TropicalGrassland | Desert => Flora::Dry,
        TemperateBroadleaf => Flora::Broadleaf,
        TemperateConifer | Boreal | Tundra | MontaneGrassland => Flora::Conifer,
        Mediterranean => Flora::Mediterranean,
        TemperateGrassland => Flora::Grassland,
    }
}

fn flora_for_climate(climate: Climate) -> Flora {
    match climate {
        Climate::Temperate => Flora::Broadleaf,
        Climate::TropicalSavanna | Climate::HotDesert | Climate::HotSteppe => Flora::Dry,
        Climate::ColdDesert | Climate::ColdSteppe | Climate::DryContinental => Flora::Grassland,
        Climate::Boreal | Climate::Tundra | Climate::IceCap => Flora::Conifer,
    }
}

/// Leaf species and their weights. Azalea stands for both azalea leaves.
fn species_weights(flora: Flora) -> &'static [(Block, u32)] {
    match flora {
        Flora::Broadleaf => &[
            (OAK_LEAVES, 45),
            (DARK_OAK_LEAVES, 15),
            (BIRCH_LEAVES, 15),
            (AZALEA_LEAVES, 25),
        ],
        Flora::Conifer => &[
            (SPRUCE_LEAVES, 50),
            (BIRCH_LEAVES, 25),
            (OAK_LEAVES, 15),
            (AZALEA_LEAVES, 10),
        ],
        Flora::Tropical => &[
            (JUNGLE_LEAVES, 45),
            (OAK_LEAVES, 20),
            (AZALEA_LEAVES, 20),
            (DARK_OAK_LEAVES, 15),
        ],
        Flora::Dry => &[(ACACIA_LEAVES, 50), (OAK_LEAVES, 35), (DARK_OAK_LEAVES, 15)],
        Flora::Mediterranean => &[
            (OAK_LEAVES, 35),
            (DARK_OAK_LEAVES, 30),
            (AZALEA_LEAVES, 25),
            (ACACIA_LEAVES, 10),
        ],
        Flora::Grassland => &[
            (OAK_LEAVES, 50),
            (BIRCH_LEAVES, 20),
            (AZALEA_LEAVES, 15),
            (DARK_OAK_LEAVES, 15),
        ],
    }
}

/// Blocks across one drift of bushes sharing a species.
const DRIFT_CELL: i32 = 18;
const SALT_SHAPE: i32 = 0x5B05_11C3;
const SALT_DRIFT: i32 = 0x0D21_F7A9;
const SALT_FLOWER: i32 = 0x7F10_3A2D;

fn hash(x: i32, z: i32, salt: i32) -> u64 {
    coord_hash(x ^ salt, z.wrapping_add(salt))
}

fn pick<T: Copy>(options: &[(T, u32)], roll: u64) -> T {
    let total: u32 = options.iter().map(|&(_, w)| w).sum();
    let mut r = (roll % total as u64) as u32;
    for &(item, w) in options {
        if r < w {
            return item;
        }
        r -= w;
    }
    options[0].0
}

fn rotate(dx: i32, dz: i32, quarter_turns: u64) -> (i32, i32) {
    match quarter_turns & 3 {
        0 => (dx, dz),
        1 => (-dz, dx),
        2 => (-dx, -dz),
        _ => (dz, -dx),
    }
}

/// Species of the bush rooted at (x, z): mostly its drift's, sometimes its own.
fn species_at(editor: &WorldEditor, x: i32, z: i32, kind: BushKind) -> Block {
    let flora = editor
        .ecoregion(x, z)
        .map(|eco| flora_for_ecoregion(eco.biome))
        .unwrap_or_else(|| flora_for_climate(editor.climate()));
    let weights = species_weights(flora);
    let own = hash(x, z, SALT_DRIFT);
    let roll = if own.is_multiple_of(4) {
        own >> 8
    } else {
        hash(
            x.div_euclid(DRIFT_CELL),
            z.div_euclid(DRIFT_CELL),
            SALT_DRIFT,
        )
    };
    let species = pick(weights, roll);
    // Gardens favour azalea for its flowers.
    if kind == BushKind::Garden && species != AZALEA_LEAVES && (own >> 40).is_multiple_of(5) {
        AZALEA_LEAVES
    } else {
        species
    }
}

/// True if a bush may spread a leaf onto the cell beside its root.
fn can_spread(editor: &WorldEditor, x: i32, z: i32) -> bool {
    if editor.surface_is_sealed(x, z) || editor.is_lc_water(x, z) {
        return false;
    }
    let ground_y = editor.get_absolute_y(x, 0, z);
    let soil_ok = editor
        .get_block_absolute(x, ground_y, z)
        .is_none_or(|b| SOIL.contains(&b));
    soil_ok && !editor.block_exists_absolute(x, ground_y + 1, z)
}

/// Grows a bush rooted on the ground at (x, z). The caller has checked the root cell;
/// leaves only spread onto open soil beside it and never replace a block.
pub fn place_bush(editor: &mut WorldEditor, x: i32, z: i32, kind: BushKind) {
    if editor.block_exists_absolute(x, editor.get_absolute_y(x, 1, z), z) {
        return;
    }
    let h = hash(x, z, SALT_SHAPE);
    let shapes = match kind {
        BushKind::Wild => WILD_SHAPES,
        BushKind::Garden => GARDEN_SHAPES,
        BushKind::Low => LOW_SHAPES,
    };
    let shape = pick(shapes, h);
    let turns = h >> 32;
    let species = species_at(editor, x, z, kind);
    let flower_share = if kind == BushKind::Garden { 45 } else { 25 };

    let mut rooted: Vec<(i32, i32)> = Vec::with_capacity(shape.len());
    for &(dx, dy, dz) in shape {
        let (rx, rz) = rotate(dx, dz, turns);
        let (bx, bz) = (x + rx, z + rz);
        if dy == 0 {
            if (rx, rz) != (0, 0) && !can_spread(editor, bx, bz) {
                continue;
            }
            rooted.push((bx, bz));
        } else if !rooted.contains(&(bx, bz)) {
            continue;
        }
        let leaf = if species == AZALEA_LEAVES
            && hash(bx, bz.wrapping_add(dy), SALT_FLOWER) % 100 < flower_share
        {
            FLOWERING_AZALEA_LEAVES
        } else {
            species
        };
        editor.set_block(leaf, bx, 1 + dy, bz, None, None);
    }
}

/// Species for a bed of shrubbery: one per area, from its drift.
pub fn shrubbery_species(editor: &WorldEditor, x: i32, z: i32) -> Block {
    species_at(editor, x, z, BushKind::Garden)
}

/// Leaf for one block of a shrubbery bed of `species`, flowering azalea mixed in.
pub fn shrubbery_leaf(species: Block, x: i32, y: i32, z: i32) -> Block {
    if species == AZALEA_LEAVES && hash(x, z.wrapping_add(y), SALT_FLOWER) % 100 < 45 {
        FLOWERING_AZALEA_LEAVES
    } else {
        species
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::test_editor;
    use crate::floodfill_cache::SealedSurfaceBitmap;
    use std::sync::Arc;

    fn leaves_in(editor: &WorldEditor, x0: i32, z0: i32, x1: i32, z1: i32) -> Vec<(i32, i32, i32)> {
        let mut out = Vec::new();
        for x in x0..=x1 {
            for z in z0..=z1 {
                for y in 1..=3 {
                    let ay = editor.get_absolute_y(x, y, z);
                    if editor
                        .get_block_absolute(x, ay, z)
                        .is_some_and(|b| BUSH_LEAVES.contains(&b))
                    {
                        out.push((x, y, z));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn bushes_take_several_shapes_and_species() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let mut sizes = std::collections::HashSet::new();
        let mut species = std::collections::HashSet::new();
        for i in 0..40 {
            let (x, z) = (5 + (i % 8) * 24, 5 + (i / 8) * 37);
            place_bush(&mut editor, x, z, BushKind::Wild);
            let cells = leaves_in(&editor, x - 2, z - 2, x + 2, z + 2);
            assert!(!cells.is_empty(), "a bush grew at ({x}, {z})");
            sizes.insert(cells.len());
            for (bx, by, bz) in cells {
                let ay = editor.get_absolute_y(bx, by, bz);
                species.insert(editor.get_block_absolute(bx, ay, bz).unwrap());
            }
        }
        assert!(sizes.len() >= 3, "shapes vary: {sizes:?}");
        assert!(species.len() >= 2, "species vary");
    }

    #[test]
    fn a_bush_stays_off_sealed_ground_and_never_floats() {
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        // A path on every odd row: only even rows are open soil.
        let mut mask = SealedSurfaceBitmap::new(&xzbbox);
        for x in 0..120 {
            for z in (1..120).step_by(2) {
                mask.set(x, z);
            }
        }
        editor.set_sealed_surface(Arc::new(mask));
        for x in (4..116).step_by(6) {
            for z in (4..116).step_by(6) {
                place_bush(&mut editor, x, z, BushKind::Garden);
            }
        }
        for (x, y, z) in leaves_in(&editor, 0, 0, 119, 119) {
            assert_eq!(z % 2, 0, "leaf on the sealed row at ({x}, {z})");
            if y > 1 {
                let below = editor.get_absolute_y(x, y - 1, z);
                assert!(
                    editor.block_exists_absolute(x, below, z),
                    "raised leaf at ({x}, {y}, {z}) rests on another"
                );
            }
        }
    }

    #[test]
    fn a_bush_is_keyed_on_its_position() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut a = test_editor(&xzbbox);
        let mut b = test_editor(&xzbbox);
        place_bush(&mut a, 20, 20, BushKind::Wild);
        place_bush(&mut b, 7, 7, BushKind::Wild);
        place_bush(&mut b, 20, 20, BushKind::Wild);
        let near = |e: &WorldEditor| leaves_in(e, 17, 17, 23, 23);
        assert_eq!(near(&a), near(&b));
    }
}
