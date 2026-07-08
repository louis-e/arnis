//! Minecraft Java 1.21 filled-map color palette and nearest-color quantizer.

use once_cell::sync::Lazy;

/// Map color id for fully transparent pixels (base 0, any shade).
pub const TRANSPARENT: u8 = 0;

/// Shade multipliers for shade 0..3; channel = floor(base * m / 255).
const SHADE_MULTIPLIERS: [u32; 4] = [180, 220, 255, 135];

/// Base colors for Java 1.21 map ids 0..=61, per https://minecraft.wiki/w/Map_item_format.
const BASE_COLORS: [(u8, u8, u8); 62] = [
    (0, 0, 0),       // 0 NONE (transparent)
    (127, 178, 56),  // 1 GRASS
    (247, 233, 163), // 2 SAND
    (199, 199, 199), // 3 WOOL
    (255, 0, 0),     // 4 FIRE
    (160, 160, 255), // 5 ICE
    (167, 167, 167), // 6 METAL
    (0, 124, 0),     // 7 PLANT
    (255, 255, 255), // 8 SNOW
    (164, 168, 184), // 9 CLAY
    (151, 109, 77),  // 10 DIRT
    (112, 112, 112), // 11 STONE
    (64, 64, 255),   // 12 WATER
    (143, 119, 72),  // 13 WOOD
    (255, 252, 245), // 14 QUARTZ
    (216, 127, 51),  // 15 COLOR_ORANGE
    (178, 76, 216),  // 16 COLOR_MAGENTA
    (102, 153, 216), // 17 COLOR_LIGHT_BLUE
    (229, 229, 51),  // 18 COLOR_YELLOW
    (127, 204, 25),  // 19 COLOR_LIGHT_GREEN
    (242, 127, 165), // 20 COLOR_PINK
    (76, 76, 76),    // 21 COLOR_GRAY
    (153, 153, 153), // 22 COLOR_LIGHT_GRAY
    (76, 127, 153),  // 23 COLOR_CYAN
    (127, 63, 178),  // 24 COLOR_PURPLE
    (51, 76, 178),   // 25 COLOR_BLUE
    (102, 76, 51),   // 26 COLOR_BROWN
    (102, 127, 51),  // 27 COLOR_GREEN
    (153, 51, 51),   // 28 COLOR_RED
    (25, 25, 25),    // 29 COLOR_BLACK
    (250, 238, 77),  // 30 GOLD
    (92, 219, 213),  // 31 DIAMOND
    (74, 128, 255),  // 32 LAPIS
    (0, 217, 58),    // 33 EMERALD
    (129, 86, 49),   // 34 PODZOL
    (112, 2, 0),     // 35 NETHER
    (209, 177, 161), // 36 TERRACOTTA_WHITE
    (159, 82, 36),   // 37 TERRACOTTA_ORANGE
    (149, 87, 108),  // 38 TERRACOTTA_MAGENTA
    (112, 108, 138), // 39 TERRACOTTA_LIGHT_BLUE
    (186, 133, 36),  // 40 TERRACOTTA_YELLOW
    (103, 117, 53),  // 41 TERRACOTTA_LIGHT_GREEN
    (160, 77, 78),   // 42 TERRACOTTA_PINK
    (57, 41, 35),    // 43 TERRACOTTA_GRAY
    (135, 107, 98),  // 44 TERRACOTTA_LIGHT_GRAY
    (87, 92, 92),    // 45 TERRACOTTA_CYAN
    (122, 73, 88),   // 46 TERRACOTTA_PURPLE
    (76, 62, 92),    // 47 TERRACOTTA_BLUE
    (76, 50, 35),    // 48 TERRACOTTA_BROWN
    (76, 82, 42),    // 49 TERRACOTTA_GREEN
    (142, 60, 46),   // 50 TERRACOTTA_RED
    (37, 22, 16),    // 51 TERRACOTTA_BLACK
    (189, 48, 49),   // 52 CRIMSON_NYLIUM
    (148, 63, 97),   // 53 CRIMSON_STEM
    (92, 25, 29),    // 54 CRIMSON_HYPHAE
    (22, 126, 134),  // 55 WARPED_NYLIUM
    (58, 142, 140),  // 56 WARPED_STEM
    (86, 44, 62),    // 57 WARPED_HYPHAE
    (20, 180, 133),  // 58 WARPED_WART_BLOCK
    (100, 100, 100), // 59 DEEPSLATE
    (216, 175, 147), // 60 RAW_IRON
    (127, 167, 150), // 61 GLOW_LICHEN
];

/// Applies a shade multiplier to one channel.
fn shade_channel(base: u8, multiplier: u32) -> u8 {
    ((base as u32 * multiplier) / 255) as u8
}

/// Shaded RGB for a given color id (base * 4 + shade).
fn shaded_color(color_id: u8) -> (u8, u8, u8) {
    let (r, g, b) = BASE_COLORS[(color_id / 4) as usize];
    let m = SHADE_MULTIPLIERS[(color_id % 4) as usize];
    (
        shade_channel(r, m),
        shade_channel(g, m),
        shade_channel(b, m),
    )
}

/// All opaque shaded variants (base ids 1..=61, four shades each) as (color_id, r, g, b).
static SHADED_PALETTE: Lazy<Vec<(u8, u8, u8, u8)>> = Lazy::new(|| {
    (4..(BASE_COLORS.len() as u16 * 4))
        .map(|id| {
            let (r, g, b) = shaded_color(id as u8);
            (id as u8, r, g, b)
        })
        .collect()
});

/// Returns the opaque map color id (base * 4 + shade) closest to the given RGB.
pub fn nearest_map_color(r: u8, g: u8, b: u8) -> u8 {
    let mut best_id = 4u8;
    let mut best_dist = u32::MAX;
    for &(id, pr, pg, pb) in SHADED_PALETTE.iter() {
        let dr = r as i32 - pr as i32;
        let dg = g as i32 - pg as i32;
        let db = b as i32 - pb as i32;
        let dist = (dr * dr + dg * dg + db * db) as u32;
        if dist < best_dist {
            best_dist = dist;
            best_id = id;
        }
    }
    best_id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_table_has_62_entries() {
        assert_eq!(BASE_COLORS.len(), 62);
        assert_eq!(SHADED_PALETTE.len(), 61 * 4);
    }

    #[test]
    fn exact_grass_matches_shade_2() {
        // GRASS base (127,178,56) at shade 2 (m=255) is unchanged -> id 1*4+2.
        assert_eq!(nearest_map_color(127, 178, 56), 4 + 2);
    }

    #[test]
    fn exact_water_matches_shade_2() {
        // WATER is base id 12 -> shade 2 id is 12*4+2 = 50.
        assert_eq!(nearest_map_color(64, 64, 255), 12 * 4 + 2);
    }

    #[test]
    fn white_maps_to_snow() {
        // Pure white should hit SNOW (base 8) at shade 2, over QUARTZ (255,252,245).
        assert_eq!(nearest_map_color(255, 255, 255), 8 * 4 + 2);
    }

    #[test]
    fn black_maps_to_dark_color() {
        // Near-black must resolve to a dark shaded variant, never the transparent base.
        let id = nearest_map_color(5, 5, 5);
        assert!(id >= 4);
        let (r, g, b) = shaded_color(id);
        assert!(r < 40 && g < 40 && b < 40);
    }

    #[test]
    fn never_returns_transparent_ids() {
        for &(r, g, b) in &[(0u8, 0u8, 0u8), (255, 255, 255), (1, 2, 3)] {
            assert!(nearest_map_color(r, g, b) >= 4);
        }
    }

    #[test]
    fn shading_formula_matches_spec() {
        // GRASS shade 0: floor(127*180/255)=89, floor(178*180/255)=125, floor(56*180/255)=39.
        assert_eq!(shaded_color(4), (89, 125, 39));
        // GRASS shade 3: floor(127*135/255)=67, floor(178*135/255)=94, floor(56*135/255)=29.
        assert_eq!(shaded_color(4 + 3), (67, 94, 29));
    }
}
