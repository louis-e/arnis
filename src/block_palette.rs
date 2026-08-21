//! Shared color to block palette, matched via Oklab. Colors are curated
//! from in-game texture samples; usage flags give 3D models the full palette
//! while walls and roofs draw from filtered subsets.

use rand::Rng;

use crate::block_definitions::*;
use crate::colors::{oklab_distance, RGBTuple};

pub const USE_MODEL: u8 = 1;
pub const USE_WALL: u8 = 2;
pub const USE_ROOF: u8 = 4;

// Shorthand for the flag column below.
const M: u8 = USE_MODEL;
const MW: u8 = USE_MODEL | USE_WALL;
const MR: u8 = USE_MODEL | USE_ROOF;
const MWR: u8 = USE_MODEL | USE_WALL | USE_ROOF;

#[rustfmt::skip]
static PALETTE: &[(RGBTuple, Block, u8)] = &[
    // Whites / off-whites / quartz
    ((207, 213, 214), WHITE_CONCRETE, MWR),
    ((210, 178, 161), WHITE_TERRACOTTA, MWR),
    ((234, 236, 237), WHITE_WOOL, M),
    ((236, 230, 223), QUARTZ_BLOCK, MWR),
    ((235, 229, 222), QUARTZ_BRICKS, MWR),
    ((249, 254, 254), SNOW_BLOCK, M),
    ((220, 220, 220), IRON_BLOCK, MWR),
    // Light grey
    ((125, 125, 115), LIGHT_GRAY_CONCRETE, MWR),
    ((135, 107, 98),  LIGHT_GRAY_TERRACOTTA, MWR),
    ((189, 188, 189), DIORITE, MW),
    ((193, 193, 195), POLISHED_DIORITE, MW),
    ((132, 135, 134), POLISHED_ANDESITE, MWR),
    ((159, 159, 159), SMOOTH_STONE, MWR),
    ((122, 122, 122), STONE_BRICKS, MWR),
    ((120, 119, 120), CHISELED_STONE_BRICKS, MW),
    ((118, 118, 118), CRACKED_STONE_BRICKS, MW),
    ((126, 126, 126), STONE, MWR),
    ((128, 127, 128), COBBLESTONE, MW),
    ((136, 136, 137), ANDESITE, MWR),
    // Medium grey
    ((55,  58,  62),  GRAY_CONCRETE, MWR),
    ((58,  42,  36),  GRAY_TERRACOTTA, MWR),
    // Dark grey / black
    ((80,  80,  83),  DEEPSLATE, MWR),
    ((71,  71,  71),  DEEPSLATE_BRICKS, MWR),
    ((72,  73,  73),  POLISHED_DEEPSLATE, MWR),
    ((77,  77,  81),  COBBLED_DEEPSLATE, MW),
    ((42,  36,  41),  BLACKSTONE, MWR),
    ((53,  49,  57),  POLISHED_BLACKSTONE, MWR),
    ((48,  43,  50),  POLISHED_BLACKSTONE_BRICKS, MWR),
    ((8,   10,  15),  BLACK_CONCRETE, MWR),
    ((37,  23,  16),  BLACK_TERRACOTTA, MWR),
    ((21,  21,  26),  BLACK_WOOL, M),
    ((67,  61,  64),  NETHERITE_BLOCK, MW),
    // Browns / earth
    ((96,  60,  32),  BROWN_CONCRETE, MWR),
    ((77,  51,  36),  BROWN_TERRACOTTA, MWR),
    ((137, 104, 79),  MUD_BRICKS, MWR),
    ((134, 96,  67),  DIRT, M),
    ((119, 86,  59),  COARSE_DIRT, M),
    ((162, 131, 79),  OAK_PLANKS, MWR),
    ((109, 85,  51),  OAK_LOG, MW),
    ((115, 85,  49),  SPRUCE_PLANKS, MWR),
    ((59,  38,  17),  SPRUCE_LOG, MW),
    ((67,  43,  20),  DARK_OAK_PLANKS, MWR),
    ((60,  47,  26),  DARK_OAK_LOG, MW),
    ((149, 103, 86),  GRANITE, MW),
    ((154, 107, 89),  POLISHED_GRANITE, MW),
    // Sandstone / yellow-tan
    ((216, 203, 156), SANDSTONE, MWR),
    ((224, 214, 170), SMOOTH_SANDSTONE, MWR),
    ((218, 224, 162), END_STONE_BRICKS, MW),
    ((166, 136, 38),  HAY_BALE, MR),
    // Reds
    ((151, 98,  83),  BRICK, MWR),
    ((143, 61,  47),  RED_TERRACOTTA, MWR),
    ((161, 39,  35),  RED_WOOL, M),
    ((142, 33,  33),  RED_CONCRETE, MW),
    ((70,  7,   9),   RED_NETHER_BRICKS, MWR),
    ((44,  22,  26),  NETHER_BRICK, MWR),
    ((152, 94,  68),  TERRACOTTA, MWR),
    // Orange / copper
    ((162, 84,  38),  ORANGE_TERRACOTTA, MWR),
    ((241, 118, 20),  ORANGE_WOOL, M),
    ((224, 97,  1),   ORANGE_CONCRETE, MW),
    ((192, 108, 80),  WAXED_COPPER_BLOCK, MWR),
    ((161, 126, 104), WAXED_EXPOSED_COPPER, MWR),
    // Yellows
    ((241, 175, 21),  YELLOW_CONCRETE, MW),
    ((186, 133, 35),  YELLOW_TERRACOTTA, MWR),
    ((249, 198, 40),  YELLOW_WOOL, M),
    ((246, 208, 62),  GOLD_BLOCK, M),
    // Greens
    ((73,  91,  36),  GREEN_CONCRETE, MWR),
    ((85,  110, 28),  GREEN_WOOL, M),
    ((94,  169, 24),  LIME_CONCRETE, MW),
    ((89,  110, 45),  MOSS_BLOCK, MR),
    ((110, 118, 95),  MOSSY_COBBLESTONE, MW),
    ((82,  163, 133), WAXED_OXIDIZED_COPPER, MWR),
    // Blues
    ((45,  47,  143), BLUE_CONCRETE, MW),
    ((74,  60,  91),  BLUE_TERRACOTTA, MWR),
    ((53,  57,  157), BLUE_WOOL, M),
    ((36,  137, 199), LIGHT_BLUE_CONCRETE, MWR),
    ((113, 109, 138), LIGHT_BLUE_TERRACOTTA, MWR),
    // Purples / magentas
    ((100, 32,  156), PURPLE_CONCRETE, MW),
    ((169, 48,  159), MAGENTA_CONCRETE, MW),
    // Cyans
    ((21,  119, 136), CYAN_CONCRETE, MWR),
    ((87,  91,  91),  CYAN_TERRACOTTA, MWR),
];

/// Palette block whose color is perceptually closest (Oklab) to the input.
pub fn closest_block(color: RGBTuple) -> Block {
    PALETTE
        .iter()
        .min_by(|(a, _, _), (b, _, _)| {
            oklab_distance(&color, a).total_cmp(&oklab_distance(&color, b))
        })
        .map(|(_, block, _)| *block)
        .unwrap_or(STONE_BRICKS)
}

/// Top-K perceptually-closest palette blocks (ascending Oklab distance).
pub fn closest_blocks(color: RGBTuple, k: usize) -> Vec<Block> {
    let mut scored: Vec<(f32, Block)> = PALETTE
        .iter()
        .map(|(c, b, _)| (oklab_distance(&color, c), *b))
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    scored.into_iter().take(k.max(1)).map(|(_, b)| b).collect()
}

/// Squared Oklab distance with lightness downweighted: a colour tag is
/// about hue, and plain Oklab lets pale neutrals outrank the hue match.
fn tag_match_distance(a: &RGBTuple, b: &RGBTuple) -> f32 {
    let a = crate::colors::oklab_components(a);
    let b = crate::colors::oklab_components(b);
    let dl = 0.5 * (a.0 - b.0);
    let da = a.1 - b.1;
    let db = a.2 - b.2;
    dl * dl + da * da + db * db
}

/// Three nearest usage-flagged blocks within 1.5x of the best match, picked
/// with the caller's rng. An exact palette hit is returned alone.
fn pick_for_usage(color: RGBTuple, usage: u8, rng: &mut impl Rng) -> Block {
    let mut scored: Vec<(f32, Block)> = PALETTE
        .iter()
        .filter(|(_, _, flags)| flags & usage != 0)
        .map(|(c, b, _)| (tag_match_distance(&color, c), *b))
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    scored.truncate(3);
    let cutoff = scored[0].0.max(1e-6) * 1.5;
    scored.retain(|&(d, _)| d <= cutoff);
    scored[rng.random_range(0..scored.len())].1
}

/// Wall block for a `building:colour`/`colour` tag value.
pub fn wall_block_for_color(color: RGBTuple, rng: &mut impl Rng) -> Block {
    pick_for_usage(color, USE_WALL, rng)
}

/// Roof block for a `roof:colour` tag value.
pub fn roof_block_for_color(color: RGBTuple, rng: &mut impl Rng) -> Block {
    pick_for_usage(color, USE_ROOF, rng)
}

/// All wall/roof-eligible palette blocks, for export-coverage tests.
#[cfg(test)]
pub(crate) fn all_building_palette_blocks() -> Vec<Block> {
    PALETTE
        .iter()
        .filter(|(_, _, f)| f & (USE_WALL | USE_ROOF) != 0)
        .map(|(_, b, _)| *b)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    #[test]
    fn palette_non_empty() {
        assert!(PALETTE.len() >= 60);
    }

    #[test]
    fn closest_block_brick_red() {
        let block = closest_block((150, 40, 40));
        let acceptable = [
            RED_CONCRETE,
            RED_WOOL,
            RED_TERRACOTTA,
            BRICK,
            RED_NETHER_BRICKS,
            NETHER_BRICK,
        ];
        assert!(
            acceptable.iter().any(|b| b.id() == block.id()),
            "got block id {}",
            block.id()
        );
    }

    #[test]
    fn closest_block_iron_brown() {
        let block = closest_block((139, 90, 60));
        let bad = [WHITE_CONCRETE, QUARTZ_BLOCK, WHITE_WOOL, SNOW_BLOCK];
        assert!(
            !bad.iter().any(|b| b.id() == block.id()),
            "iron-brown should not map to a white block, got {}",
            block.id()
        );
    }

    #[test]
    fn closest_blocks_returns_k_red_variants_for_red_input() {
        let blocks = closest_blocks((150, 40, 40), 4);
        assert_eq!(blocks.len(), 4);
        let acceptable_reds = [
            RED_CONCRETE,
            RED_WOOL,
            RED_TERRACOTTA,
            BRICK,
            RED_NETHER_BRICKS,
            NETHER_BRICK,
            TERRACOTTA,
            ORANGE_TERRACOTTA,
            ORANGE_CONCRETE,
        ];
        for b in &blocks {
            assert!(
                acceptable_reds.iter().any(|r| r.id() == b.id()),
                "got non-red block id {} in palette",
                b.id()
            );
        }
    }

    fn assert_wall_family(rgb: (u8, u8, u8), family: &[Block]) {
        for seed in 0..12u64 {
            let mut r = ChaCha8Rng::seed_from_u64(seed);
            let b = wall_block_for_color(rgb, &mut r);
            assert!(
                family.contains(&b),
                "wall colour {rgb:?} escaped its family: {b:?}"
            );
        }
    }

    #[test]
    fn saturated_wall_colours_stay_in_family() {
        assert_wall_family((0, 128, 0), &[GREEN_CONCRETE, LIME_CONCRETE]);
        // Pure yellow may legitimately land on pale-ochre blocks (how tagged
        // yellow render facades actually look), but never on grey/red/blue.
        assert_wall_family(
            (255, 255, 0),
            &[
                YELLOW_CONCRETE,
                YELLOW_TERRACOTTA,
                END_STONE_BRICKS,
                SMOOTH_SANDSTONE,
                SANDSTONE,
            ],
        );
        assert_wall_family(
            (255, 0, 0),
            // Vivid red paint skews orange-red, so orange concrete is fine.
            &[RED_CONCRETE, RED_TERRACOTTA, ORANGE_CONCRETE],
        );
        assert_wall_family((128, 0, 128), &[PURPLE_CONCRETE, MAGENTA_CONCRETE]);
        assert_wall_family((255, 128, 0), &[ORANGE_CONCRETE, ORANGE_TERRACOTTA]);
        assert_wall_family(
            (24, 116, 205),
            &[LIGHT_BLUE_CONCRETE, BLUE_CONCRETE, CYAN_CONCRETE],
        );
    }

    #[test]
    fn muted_wall_colours_keep_muted_blocks() {
        let saturated = [
            GREEN_CONCRETE,
            LIME_CONCRETE,
            YELLOW_CONCRETE,
            RED_CONCRETE,
            PURPLE_CONCRETE,
            MAGENTA_CONCRETE,
            ORANGE_CONCRETE,
            LIGHT_BLUE_CONCRETE,
            BLUE_CONCRETE,
            CYAN_CONCRETE,
        ];
        // beige, pale rose, brick red
        for rgb in [(187, 173, 142), (209, 177, 161), (176, 74, 58)] {
            for seed in 0..20u64 {
                let mut r = ChaCha8Rng::seed_from_u64(seed);
                let b = wall_block_for_color(rgb, &mut r);
                assert!(
                    !saturated.contains(&b),
                    "colour {rgb:?} got saturated {b:?}"
                );
            }
        }
    }

    #[test]
    fn wall_colours_never_pick_excluded_textures() {
        let excluded = [
            WHITE_WOOL,
            BLACK_WOOL,
            RED_WOOL,
            ORANGE_WOOL,
            YELLOW_WOOL,
            GREEN_WOOL,
            BLUE_WOOL,
            SNOW_BLOCK,
            GOLD_BLOCK,
            HAY_BALE,
            DIRT,
            COARSE_DIRT,
            MOSS_BLOCK,
        ];
        for r in [0u8, 60, 120, 180, 240] {
            for g in [0u8, 60, 120, 180, 240] {
                for b in [0u8, 60, 120, 180, 240] {
                    for seed in 0..3u64 {
                        let mut rng = ChaCha8Rng::seed_from_u64(seed);
                        let block = wall_block_for_color((r, g, b), &mut rng);
                        assert!(
                            !excluded.contains(&block),
                            "wall colour ({r},{g},{b}) picked excluded {block:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn roof_colours_avoid_logs_and_wool() {
        let excluded = [
            OAK_LOG,
            SPRUCE_LOG,
            DARK_OAK_LOG,
            DIRT,
            COARSE_DIRT,
            WHITE_WOOL,
            BLACK_WOOL,
            RED_WOOL,
            ORANGE_WOOL,
            YELLOW_WOOL,
            GREEN_WOOL,
            BLUE_WOOL,
            SNOW_BLOCK,
            GOLD_BLOCK,
        ];
        for rgb in [(10, 10, 10), (150, 40, 40), (100, 70, 40), (250, 250, 250)] {
            for seed in 0..10u64 {
                let mut r = ChaCha8Rng::seed_from_u64(seed);
                let block = roof_block_for_color(rgb, &mut r);
                assert!(
                    !excluded.contains(&block),
                    "roof colour {rgb:?} picked excluded {block:?}"
                );
            }
        }
    }

    #[test]
    fn red_roof_gives_tile_family() {
        let family = [BRICK, RED_TERRACOTTA, TERRACOTTA, RED_NETHER_BRICKS];
        for seed in 0..10u64 {
            let mut r = ChaCha8Rng::seed_from_u64(seed);
            let block = roof_block_for_color((160, 50, 40), &mut r);
            assert!(family.contains(&block), "red roof got {block:?}");
        }
    }

    #[test]
    fn warm_saturated_concrete_never_roofs() {
        let banned = [
            RED_CONCRETE,
            ORANGE_CONCRETE,
            YELLOW_CONCRETE,
            LIME_CONCRETE,
            PURPLE_CONCRETE,
            MAGENTA_CONCRETE,
        ];
        for rgb in [(255, 0, 0), (255, 128, 0), (255, 255, 0), (128, 0, 128)] {
            for seed in 0..12u64 {
                let mut r = ChaCha8Rng::seed_from_u64(seed);
                let b = roof_block_for_color(rgb, &mut r);
                assert!(!banned.contains(&b), "roof colour {rgb:?} got {b:?}");
            }
        }
    }

    // painted blue metal roofs are real, the BMW logo slab stays BMW blue
    #[test]
    fn saturated_blue_roof_keeps_blue_concrete() {
        for seed in 0..12u64 {
            let mut r = ChaCha8Rng::seed_from_u64(seed);
            let b = roof_block_for_color((32, 116, 192), &mut r);
            assert_eq!(b, LIGHT_BLUE_CONCRETE, "BMW blue got {b:?}");
        }
    }

    // violet and navy tags land on muted terracotta
    #[test]
    fn violet_roofs_stay_muted() {
        let banned = [BLUE_CONCRETE, PURPLE_CONCRETE, MAGENTA_CONCRETE];
        for rgb in [(148, 0, 211), (45, 47, 143), (128, 0, 128)] {
            for seed in 0..12u64 {
                let mut r = ChaCha8Rng::seed_from_u64(seed);
                let b = roof_block_for_color(rgb, &mut r);
                assert!(!banned.contains(&b), "colour {rgb:?} got {b:?}");
            }
        }
    }

    #[test]
    fn colour_pick_is_deterministic_per_seed() {
        for seed in 0..20u64 {
            let mut a = ChaCha8Rng::seed_from_u64(seed);
            let mut b = ChaCha8Rng::seed_from_u64(seed);
            assert_eq!(
                wall_block_for_color((151, 98, 83), &mut a),
                wall_block_for_color((151, 98, 83), &mut b)
            );
        }
    }
}
