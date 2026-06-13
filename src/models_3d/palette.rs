//! Color → Block palette, matched via Oklab. Block colors curated from in-game texture samples.

use crate::block_definitions::*;
use crate::colors::{oklab_distance, RGBTuple};

#[rustfmt::skip]
static PALETTE: &[(RGBTuple, Block)] = &[
    // Whites / off-whites / quartz
    ((207, 213, 214), WHITE_CONCRETE),
    ((210, 178, 161), WHITE_TERRACOTTA),
    ((234, 236, 237), WHITE_WOOL),
    ((236, 230, 223), QUARTZ_BLOCK),
    ((235, 229, 222), QUARTZ_BRICKS),
    ((249, 254, 254), SNOW_BLOCK),
    ((220, 220, 220), IRON_BLOCK),
    // Light grey
    ((125, 125, 115), LIGHT_GRAY_CONCRETE),
    ((135, 107, 98),  LIGHT_GRAY_TERRACOTTA),
    ((189, 188, 189), DIORITE),
    ((193, 193, 195), POLISHED_DIORITE),
    ((132, 135, 134), POLISHED_ANDESITE),
    ((159, 159, 159), SMOOTH_STONE),
    ((122, 122, 122), STONE_BRICKS),
    ((120, 119, 120), CHISELED_STONE_BRICKS),
    ((118, 118, 118), CRACKED_STONE_BRICKS),
    ((126, 126, 126), STONE),
    ((128, 127, 128), COBBLESTONE),
    ((136, 136, 137), ANDESITE),
    // Medium grey
    ((55,  58,  62),  GRAY_CONCRETE),
    ((58,  42,  36),  GRAY_TERRACOTTA),
    // Dark grey / black
    ((80,  80,  83),  DEEPSLATE),
    ((71,  71,  71),  DEEPSLATE_BRICKS),
    ((72,  73,  73),  POLISHED_DEEPSLATE),
    ((77,  77,  81),  COBBLED_DEEPSLATE),
    ((42,  36,  41),  BLACKSTONE),
    ((53,  49,  57),  POLISHED_BLACKSTONE),
    ((48,  43,  50),  POLISHED_BLACKSTONE_BRICKS),
    ((8,   10,  15),  BLACK_CONCRETE),
    ((37,  23,  16),  BLACK_TERRACOTTA),
    ((21,  21,  26),  BLACK_WOOL),
    ((67,  61,  64),  NETHERITE_BLOCK),
    // Browns / earth
    ((96,  60,  32),  BROWN_CONCRETE),
    ((77,  51,  36),  BROWN_TERRACOTTA),
    ((137, 104, 79),  MUD_BRICKS),
    ((134, 96,  67),  DIRT),
    ((119, 86,  59),  COARSE_DIRT),
    ((162, 131, 79),  OAK_PLANKS),
    ((109, 85,  51),  OAK_LOG),
    ((115, 85,  49),  SPRUCE_PLANKS),
    ((59,  38,  17),  SPRUCE_LOG),
    ((67,  43,  20),  DARK_OAK_PLANKS),
    ((60,  47,  26),  DARK_OAK_LOG),
    ((149, 103, 86),  GRANITE),
    ((154, 107, 89),  POLISHED_GRANITE),
    // Sandstone / yellow-tan
    ((216, 203, 156), SANDSTONE),
    ((224, 214, 170), SMOOTH_SANDSTONE),
    ((218, 224, 162), END_STONE_BRICKS),
    ((166, 136, 38),  HAY_BALE),
    // Reds
    ((151, 98,  83),  BRICK),
    ((143, 61,  47),  RED_TERRACOTTA),
    ((161, 39,  35),  RED_WOOL),
    ((142, 33,  33),  RED_CONCRETE),
    ((70,  7,   9),   RED_NETHER_BRICKS),
    ((44,  22,  26),  NETHER_BRICK),
    ((152, 94,  68),  TERRACOTTA),
    // Orange / copper
    ((162, 84,  38),  ORANGE_TERRACOTTA),
    ((241, 118, 20),  ORANGE_WOOL),
    ((192, 108, 80),  WAXED_COPPER_BLOCK),
    ((161, 126, 104), WAXED_EXPOSED_COPPER),
    // Yellows
    ((241, 175, 21),  YELLOW_CONCRETE),
    ((186, 133, 35),  YELLOW_TERRACOTTA),
    ((249, 198, 40),  YELLOW_WOOL),
    ((246, 208, 62),  GOLD_BLOCK),
    // Greens
    ((73,  91,  36),  GREEN_CONCRETE),
    ((85,  110, 28),  GREEN_WOOL),
    ((94,  169, 24),  LIME_CONCRETE),
    ((89,  110, 45),  MOSS_BLOCK),
    ((110, 118, 95),  MOSSY_COBBLESTONE),
    ((82,  163, 133), WAXED_OXIDIZED_COPPER),
    // Blues
    ((45,  47,  143), BLUE_CONCRETE),
    ((74,  60,  91),  BLUE_TERRACOTTA),
    ((53,  57,  157), BLUE_WOOL),
    ((36,  137, 199), LIGHT_BLUE_CONCRETE),
    ((113, 109, 138), LIGHT_BLUE_TERRACOTTA),
    // Purples / magentas
    ((100, 32,  156), PURPLE_CONCRETE),
    ((169, 48,  159), MAGENTA_CONCRETE),
    // Cyans
    ((21,  119, 136), CYAN_CONCRETE),
    ((87,  91,  91),  CYAN_TERRACOTTA),
];

/// Palette block whose color is perceptually closest (Oklab) to the input.
pub fn closest_block(color: RGBTuple) -> Block {
    PALETTE
        .iter()
        .min_by(|(a, _), (b, _)| oklab_distance(&color, a).total_cmp(&oklab_distance(&color, b)))
        .map(|(_, block)| *block)
        .unwrap_or(STONE_BRICKS)
}

/// Top-K perceptually-closest palette blocks (ascending Oklab distance).
pub fn closest_blocks(color: RGBTuple, k: usize) -> Vec<Block> {
    let mut scored: Vec<(f32, Block)> = PALETTE
        .iter()
        .map(|(c, b)| (oklab_distance(&color, c), *b))
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    scored.into_iter().take(k.max(1)).map(|(_, b)| b).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "iron-brown should not map to a white block — got {}",
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
        ];
        for b in &blocks {
            assert!(
                acceptable_reds.iter().any(|r| r.id() == b.id()),
                "got non-red block id {} in palette",
                b.id()
            );
        }
    }
}
