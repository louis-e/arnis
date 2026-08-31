//! What a ground block means, in one place.
//!
//! Several passes read the surface block back to decide whether a plant grows,
//! a tree roots, or a tunnel bore cuts through. Each used to carry its own
//! literal list, so a new surface material silently changed behaviour
//! elsewhere. The coverage test below walks every palette and checks them.

use crate::block_definitions::*;

/// Excludes SAND on purpose: it is the whole hot-desert surface, so admitting
/// it would scatter grass across the Sahara.
pub const fn supports_vegetation(b: Block) -> bool {
    matches!(
        b,
        GRASS_BLOCK | DIRT | COARSE_DIRT | PODZOL | MOSS_BLOCK | MUD | FARMLAND
    )
}

/// Wider than vegetation, since street trees stand in paved ground.
pub const fn supports_trees(b: Block) -> bool {
    supports_vegetation(b)
        || matches!(
            b,
            SMOOTH_STONE | STONE_BRICKS | CRACKED_STONE_BRICKS | STONE | COBBLESTONE
        )
}

/// `dead_bush_may_place_on` covers terracotta and sand but not sandstone,
/// where the bush would pop off on the first block update.
pub const fn supports_dead_bush(b: Block) -> bool {
    matches!(
        b,
        TERRACOTTA
            | ORANGE_TERRACOTTA
            | RED_TERRACOTTA
            | BROWN_TERRACOTTA
            | GRAY_TERRACOTTA
            | LIGHT_GRAY_TERRACOTTA
            | WHITE_TERRACOTTA
            | SAND
            | RED_SAND
    ) || supports_vegetation(b)
}

/// Need soil under them, or they drop on the first chunk update.
pub const SOIL_PLANTS: &[Block] = &[
    GRASS,
    FERN,
    TALL_GRASS_BOTTOM,
    TALL_GRASS_TOP,
    LARGE_FERN_LOWER,
    LARGE_FERN_UPPER,
    RED_FLOWER,
    BLUE_FLOWER,
    YELLOW_FLOWER,
    WHITE_FLOWER,
];

/// Upper halves of the two-block plants above. Clearing a stranded plant by its
/// lower half alone leaves one of these floating a block off the ground.
pub const SOIL_PLANT_TOPS: &[Block] = &[TALL_GRASS_TOP, LARGE_FERN_UPPER];

/// Every block the ground pass may leave as a column's surface. Only the
/// coverage test reads this; it is the contract each palette has to satisfy.
#[cfg(test)]
pub const fn is_natural_ground(b: Block) -> bool {
    supports_trees(b)
        || matches!(
            b,
            TERRACOTTA
                | ORANGE_TERRACOTTA
                | RED_TERRACOTTA
                | BROWN_TERRACOTTA
                | GRAY_TERRACOTTA
                | LIGHT_GRAY_TERRACOTTA
                | WHITE_TERRACOTTA
                | SANDSTONE
                | SMOOTH_SANDSTONE
        )
        || matches!(
            b,
            ANDESITE
                | TUFF
                | GRAVEL
                | SAND
                | RED_SAND
                | DEEPSLATE
                | COBBLED_DEEPSLATE
                | GRANITE
                | BLACKSTONE
                | CLAY
                | SNOW_BLOCK
                | PACKED_ICE
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::climate::Climate;
    use crate::land_cover::*;

    const CLIMATES: [Climate; 10] = [
        Climate::Temperate,
        Climate::TropicalSavanna,
        Climate::HotDesert,
        Climate::HotSteppe,
        Climate::ColdDesert,
        Climate::ColdSteppe,
        Climate::DryContinental,
        Climate::Boreal,
        Climate::Tundra,
        Climate::IceCap,
    ];

    const COVERS: [u8; 11] = [
        LC_TREE_COVER,
        LC_SHRUBLAND,
        LC_GRASSLAND,
        LC_CROPLAND,
        LC_BUILT_UP,
        LC_BARE,
        LC_SNOW_ICE,
        LC_WATER,
        LC_WETLAND,
        LC_MANGROVES,
        LC_MOSS,
    ];

    /// A spread of coordinates wide enough to hit every branch of the patch and
    /// per-block hashes.
    fn spread() -> impl Iterator<Item = (i32, i32)> {
        (-3..13).flat_map(|i| (-3..13).map(move |j| (i * 37, j * 53)))
    }

    fn check(b: Block, what: &str) {
        assert!(
            is_natural_ground(b),
            "{what} emits {} which no pass recognises as natural ground",
            b.name()
        );
        assert!(
            b.id() < BYTE_ID_LIMIT,
            "{what} emits {} (id {}), which widens every section it lands in",
            b.name(),
            b.id()
        );
    }

    #[test]
    fn every_climate_palette_emits_recognised_ground() {
        for c in CLIMATES {
            for cover in COVERS {
                for (x, z) in spread() {
                    if let Some((surf, under)) = c.surface_palette(cover, x, z) {
                        check(surf, "Climate::surface_palette");
                        check(under, "Climate::surface_palette under");
                    }
                }
            }
        }
    }

    #[test]
    fn desert_floor_emits_recognised_ground() {
        for wooded in [false, true] {
            for (x, z) in spread() {
                let (surf, under) = crate::strata::desert_floor(x, z, wooded);
                check(surf, "desert_floor");
                check(under, "desert_floor under");
            }
        }
    }

    #[test]
    fn strata_bands_emit_recognised_ground() {
        let s = crate::strata::Strata::new(0, 300);
        for (x, z) in spread() {
            let col = s.column(x, z);
            for y in 0..300 {
                check(s.block(&col, y), "Strata::block");
            }
        }
    }

    #[test]
    fn wooded_desert_floor_can_root_a_tree() {
        // The canyon rim carries pinyon-juniper, so its floor has to pass the
        // tree gate or the rim comes out bald.
        let rootable = spread()
            .filter(|&(x, z)| supports_trees(crate::strata::desert_floor(x, z, true).0))
            .count();
        assert!(
            rootable * 2 > spread().count(),
            "most wooded rock-desert columns should be rootable"
        );
    }

    #[test]
    fn dead_bush_only_where_minecraft_allows_it() {
        // sandstone is not in dead_bush_may_place_on, so a bush there would drop.
        assert!(!supports_dead_bush(SANDSTONE));
        assert!(!supports_dead_bush(SMOOTH_SANDSTONE));
        assert!(supports_dead_bush(TERRACOTTA));
        assert!(supports_dead_bush(ORANGE_TERRACOTTA));
        assert!(supports_dead_bush(SAND));
    }

    #[test]
    fn every_two_block_plant_has_its_top_listed() {
        // Clearing a stranded plant reads SOIL_PLANTS at ground+1 and
        // SOIL_PLANT_TOPS at ground+2. A two-block plant whose upper half is
        // missing from the second list gets decapitated and left floating.
        for top in SOIL_PLANT_TOPS {
            assert!(
                SOIL_PLANTS.contains(top),
                "{} is cleared at ground+2 but never recognised at ground+1",
                top.name()
            );
        }
        for (lower, upper) in [
            (TALL_GRASS_BOTTOM, TALL_GRASS_TOP),
            (LARGE_FERN_LOWER, LARGE_FERN_UPPER),
        ] {
            assert!(
                SOIL_PLANTS.contains(&lower),
                "{} is placed on soil but never cleared",
                lower.name()
            );
            assert!(
                SOIL_PLANT_TOPS.contains(&upper),
                "{} would be left floating when {} is cleared",
                upper.name(),
                lower.name()
            );
        }
    }

    #[test]
    fn sand_never_grows_plants() {
        assert!(!supports_vegetation(SAND));
        assert!(!supports_vegetation(GRAVEL));
        assert!(is_natural_ground(SAND));
    }
}
