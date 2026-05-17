//! Color → Block palette built at first use from `sorted_colors.txt`, matched via Oklab.

use crate::block_definitions::*;
use crate::colors::{oklab_distance, RGBTuple};
use once_cell::sync::Lazy;

const SORTED_COLORS: &str = include_str!("../../sorted_colors.txt");

/// Texture-name → Block. Names match `sorted_colors.txt` exactly.
static NAME_TO_BLOCK: &[(&str, Block)] = &[
    // Whites / off-whites
    ("white_concrete", WHITE_CONCRETE),
    ("white_terracotta", WHITE_TERRACOTTA),
    ("white_wool", WHITE_WOOL),
    ("quartz_block_side", QUARTZ_BLOCK),
    ("quartz_bricks", QUARTZ_BRICKS),
    ("snow", SNOW_BLOCK),
    ("iron_block", IRON_BLOCK),
    // Light grey
    ("light_gray_concrete", LIGHT_GRAY_CONCRETE),
    ("light_gray_terracotta", LIGHT_GRAY_TERRACOTTA),
    ("diorite", DIORITE),
    ("polished_diorite", POLISHED_DIORITE),
    ("polished_andesite", POLISHED_ANDESITE),
    ("smooth_stone", SMOOTH_STONE),
    ("stone_bricks", STONE_BRICKS),
    ("chiseled_stone_bricks", CHISELED_STONE_BRICKS),
    ("cracked_stone_bricks", CRACKED_STONE_BRICKS),
    ("stone", STONE),
    ("cobblestone", COBBLESTONE),
    ("andesite", ANDESITE),
    // Medium grey
    ("gray_concrete", GRAY_CONCRETE),
    ("gray_terracotta", GRAY_TERRACOTTA),
    // Dark grey / black
    ("deepslate", DEEPSLATE),
    ("deepslate_bricks", DEEPSLATE_BRICKS),
    ("polished_deepslate", POLISHED_DEEPSLATE),
    ("cobbled_deepslate", COBBLED_DEEPSLATE),
    ("blackstone", BLACKSTONE),
    ("polished_blackstone", POLISHED_BLACKSTONE),
    ("polished_blackstone_bricks", POLISHED_BLACKSTONE_BRICKS),
    ("black_concrete", BLACK_CONCRETE),
    ("black_terracotta", BLACK_TERRACOTTA),
    ("black_wool", BLACK_WOOL),
    ("netherite_block", NETHERITE_BLOCK),
    // Browns / earth
    ("brown_concrete", BROWN_CONCRETE),
    ("brown_terracotta", BROWN_TERRACOTTA),
    ("mud_bricks", MUD_BRICKS),
    ("dirt", DIRT),
    ("coarse_dirt", COARSE_DIRT),
    ("oak_planks", OAK_PLANKS),
    ("oak_log", OAK_LOG),
    ("spruce_planks", SPRUCE_PLANKS),
    ("spruce_log", SPRUCE_LOG),
    ("dark_oak_planks", DARK_OAK_PLANKS),
    ("dark_oak_log", DARK_OAK_LOG),
    ("granite", GRANITE),
    ("polished_granite", POLISHED_GRANITE),
    // Sandstone / yellow-tan
    ("sandstone", SANDSTONE),
    ("smooth_sandstone_top", SMOOTH_SANDSTONE),
    ("end_stone_bricks", END_STONE_BRICKS),
    ("hay_block_side", HAY_BALE),
    // Reds
    ("bricks", BRICK),
    ("red_terracotta", RED_TERRACOTTA),
    ("red_wool", RED_WOOL),
    ("red_concrete", RED_CONCRETE),
    ("red_nether_bricks", RED_NETHER_BRICKS),
    ("nether_bricks", NETHER_BRICK),
    ("terracotta", TERRACOTTA),
    // Orange / amber (covers the iron-orange Eiffel Tower hue too)
    ("orange_terracotta", ORANGE_TERRACOTTA),
    ("orange_wool", ORANGE_WOOL),
    ("copper_block", WAXED_COPPER_BLOCK),
    ("exposed_copper", WAXED_EXPOSED_COPPER),
    // Yellows
    ("yellow_concrete", YELLOW_CONCRETE),
    ("yellow_terracotta", YELLOW_TERRACOTTA),
    ("yellow_wool", YELLOW_WOOL),
    ("gold_block", GOLD_BLOCK),
    // Greens
    ("green_concrete", GREEN_CONCRETE),
    ("green_wool", GREEN_WOOL),
    ("lime_concrete", LIME_CONCRETE),
    ("moss_block", MOSS_BLOCK),
    ("mossy_cobblestone", MOSSY_COBBLESTONE),
    ("oxidized_copper", WAXED_OXIDIZED_COPPER),
    // Blues
    ("blue_concrete", BLUE_CONCRETE),
    ("blue_terracotta", BLUE_TERRACOTTA),
    ("blue_wool", BLUE_WOOL),
    ("light_blue_concrete", LIGHT_BLUE_CONCRETE),
    ("light_blue_terracotta", LIGHT_BLUE_TERRACOTTA),
    // Purples / magentas
    ("purple_concrete", PURPLE_CONCRETE),
    ("magenta_concrete", MAGENTA_CONCRETE),
    // Cyans
    ("cyan_concrete", CYAN_CONCRETE),
    ("cyan_terracotta", CYAN_TERRACOTTA),
];

static PALETTE: Lazy<Vec<(RGBTuple, Block)>> = Lazy::new(build_palette);

fn build_palette() -> Vec<(RGBTuple, Block)> {
    let mut name_map = std::collections::HashMap::with_capacity(NAME_TO_BLOCK.len());
    for (name, block) in NAME_TO_BLOCK {
        name_map.insert(*name, *block);
    }

    let mut out = Vec::with_capacity(NAME_TO_BLOCK.len());
    for line in SORTED_COLORS.lines() {
        let Some((name, rgb)) = parse_line(line) else {
            continue;
        };
        if let Some(&block) = name_map.get(name) {
            out.push((rgb, block));
        }
    }
    out
}

fn parse_line(line: &str) -> Option<(&str, RGBTuple)> {
    let (name, rest) = line.split_once(": rgb(")?;
    let inner = rest.split_once(')')?.0;
    let mut parts = inner.split(',').map(|s| s.trim().parse::<u16>().ok());
    let r = parts.next()??;
    let g = parts.next()??;
    let b = parts.next()??;
    Some((name, (r.min(255) as u8, g.min(255) as u8, b.min(255) as u8)))
}

/// Palette block whose color is perceptually closest (Oklab) to the input.
pub fn closest_block(color: RGBTuple) -> Block {
    PALETTE
        .iter()
        .min_by(|(a, _), (b, _)| oklab_distance(&color, a).total_cmp(&oklab_distance(&color, b)))
        .map(|(_, block)| *block)
        .unwrap_or(STONE_BRICKS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_built_from_sorted_colors() {
        // Palette should include at least most of the curated entries.
        let n = PALETTE.len();
        assert!(
            n >= NAME_TO_BLOCK.len() - 5,
            "expected palette ~{} entries, got {n} — sorted_colors.txt missing keys?",
            NAME_TO_BLOCK.len()
        );
    }

    #[test]
    fn closest_block_brick_red() {
        // A typical brick/red building color sits firmly in the red palette.
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
        // Approximate Eiffel-Tower iron color.
        let block = closest_block((139, 90, 60));
        // Should land on a brown/orange-ish block, never on white/quartz.
        let bad = [WHITE_CONCRETE, QUARTZ_BLOCK, WHITE_WOOL, SNOW_BLOCK];
        assert!(
            !bad.iter().any(|b| b.id() == block.id()),
            "iron-brown should not map to a white block — got {}",
            block.id()
        );
    }

    #[test]
    fn parse_line_basic() {
        let r = parse_line("oak_planks: rgb(162, 131, 79) #A2834F");
        assert_eq!(r, Some(("oak_planks", (162u8, 131u8, 79u8))));
    }

    #[test]
    fn parse_line_garbage() {
        assert!(parse_line("# a comment").is_none());
        assert!(parse_line("").is_none());
    }
}
