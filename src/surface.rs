//! What Minecraft holds up (supports_*) and where Arnis scatters (takes_wild_*).

use crate::block_definitions::*;

/// Excludes SAND: it is the hot-desert surface.
pub const fn supports_vegetation(b: Block) -> bool {
    matches!(
        b,
        GRASS_BLOCK | DIRT | COARSE_DIRT | PODZOL | MOSS_BLOCK | MUD | FARMLAND
    )
}

/// Vanilla dead_bush_may_place_on: terracotta and sand, but not sandstone.
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

/// Where the land-cover pass may scatter. Narrower than [`supports_vegetation`]:
/// podzol is a cemetery, moss a wetland ring, both planted by their own pass.
pub const fn takes_wild_plants(b: Block) -> bool {
    matches!(b, GRASS_BLOCK | DIRT | COARSE_DIRT | MUD | FARMLAND)
}

/// Street trees stand in paved ground. Stone and cobblestone stay out, being
/// the quarry and bare-rock surfaces.
pub const fn takes_wild_trees(b: Block) -> bool {
    takes_wild_plants(b) || matches!(b, SMOOTH_STONE | STONE_BRICKS | CRACKED_STONE_BRICKS)
}

/// The wild-plant soils plus the rock-desert floor, but not the hot-desert SAND.
pub const fn takes_wild_dead_bush(b: Block) -> bool {
    takes_wild_plants(b)
        || matches!(
            b,
            TERRACOTTA
                | ORANGE_TERRACOTTA
                | RED_TERRACOTTA
                | BROWN_TERRACOTTA
                | GRAY_TERRACOTTA
                | LIGHT_GRAY_TERRACOTTA
                | WHITE_TERRACOTTA
                | RED_SAND
        )
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

/// Farmland only. A crop pops off plain dirt just as it does off stone.
pub const fn supports_crops(b: Block) -> bool {
    matches!(b, FARMLAND)
}

/// Placed on farmland at ground+1, before the ground pass can replace it.
pub const CROPS: &[Block] = &[WHEAT, CARROTS, POTATOES];

/// Sand and soil, but not farmland and not the rock-desert floor.
pub const fn supports_sugar_cane(b: Block) -> bool {
    matches!(
        b,
        GRASS_BLOCK | DIRT | COARSE_DIRT | PODZOL | MOSS_BLOCK | MUD | SAND | RED_SAND
    )
}

/// Cane stacks on itself, so a stranded stalk comes out whole.
pub const SUGAR_CANE_MAX_HEIGHT: i32 = 3;

/// Everything the sweep may clear at ground+1, each with its own footing test.
pub const CLEARABLE_PLANTS: &[Block] = &[
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
    DEAD_BUSH,
    WHEAT,
    CARROTS,
    POTATOES,
    SUGAR_CANE,
];

/// Upper halves, or clearing a lower half leaves one of these floating.
pub const SOIL_PLANT_TOPS: &[Block] = &[TALL_GRASS_TOP, LARGE_FERN_UPPER];

/// Every block the ground pass may leave as a surface. No smooth stone or
/// stone bricks, which are building materials a tunnel bore must not eat.
pub const fn is_natural_ground(b: Block) -> bool {
    supports_vegetation(b)
        || matches!(b, STONE | COBBLESTONE)
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

    /// Wide enough to hit every branch of the patch and per-block hashes.
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
        use crate::strata::FloorCover;
        for cover in [FloorCover::Bare, FloorCover::Sparse, FloorCover::Wooded] {
            for (x, z) in spread() {
                let (surf, under) = crate::strata::desert_floor(x, z, cover);
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
    fn vegetated_desert_floor_keeps_ground_its_plants_can_use() {
        use crate::strata::FloorCover;
        // Or classifying a bbox as rock desert deletes what the data promised.
        let total = spread().count();
        let rootable = spread()
            .filter(|&(x, z)| {
                takes_wild_trees(crate::strata::desert_floor(x, z, FloorCover::Wooded).0)
            })
            .count();
        assert!(
            rootable * 2 > total,
            "most wooded rock-desert columns should root a tree, got {rootable}/{total}"
        );
        let growable = spread()
            .filter(|&(x, z)| {
                takes_wild_plants(crate::strata::desert_floor(x, z, FloorCover::Sparse).0)
            })
            .count();
        assert!(
            growable * 2 > total,
            "most shrub/grass rock-desert columns should grow a plant, got {growable}/{total}"
        );
        // Measured-bare stays the rock desert proper: mostly not plantable.
        let bare_growable = spread()
            .filter(|&(x, z)| {
                supports_vegetation(crate::strata::desert_floor(x, z, FloorCover::Bare).0)
            })
            .count();
        assert!(
            bare_growable * 2 < total,
            "bare rock desert should stay mostly bare, got {bare_growable}/{total}"
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
        // A two-block plant missing from SOIL_PLANT_TOPS is left decapitated.
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
    fn the_sweep_can_clear_everything_it_can_strand() {
        // Anything the predicate finds but the whitelist omits is left standing.
        for p in SOIL_PLANTS {
            assert!(
                CLEARABLE_PLANTS.contains(p),
                "{} can be found stranded but never cleared",
                p.name()
            );
        }
        assert!(
            CLEARABLE_PLANTS.contains(&DEAD_BUSH),
            "dead bush is checked against supports_dead_bush but never cleared"
        );
        for c in CROPS {
            assert!(
                CLEARABLE_PLANTS.contains(c),
                "{} is checked against supports_crops but never cleared",
                c.name()
            );
            assert!(
                !supports_crops(GRASS_BLOCK) && supports_crops(FARMLAND),
                "crops must require farmland, not any soil"
            );
        }
        assert!(
            CLEARABLE_PLANTS.contains(&SUGAR_CANE),
            "sugar cane is checked against supports_sugar_cane but never cleared"
        );
    }

    // Sand holds cane, the banded rock floor does not.
    #[test]
    fn sugar_cane_footing_matches_the_blocks_it_can_stand_on() {
        for b in [
            GRASS_BLOCK,
            DIRT,
            COARSE_DIRT,
            PODZOL,
            MOSS_BLOCK,
            MUD,
            SAND,
            RED_SAND,
        ] {
            assert!(supports_sugar_cane(b), "{} holds cane in game", b.name());
        }
        for b in [
            FARMLAND,
            TERRACOTTA,
            ORANGE_TERRACOTTA,
            SANDSTONE,
            SMOOTH_SANDSTONE,
            STONE,
            GRAVEL,
        ] {
            assert!(!supports_sugar_cane(b), "{} drops cane in game", b.name());
        }
    }

    // Anything the scatter plants must be something the sweep will not clear.
    #[test]
    fn every_surface_the_scatter_plants_on_also_holds_the_plant() {
        let held = |b: Block, what: &str| {
            if takes_wild_plants(b) {
                assert!(
                    supports_vegetation(b),
                    "{what} scatters plants on {}, which the sweep then clears",
                    b.name()
                );
            }
            if takes_wild_dead_bush(b) {
                assert!(
                    supports_dead_bush(b),
                    "{what} scatters a dead bush on {}, which the sweep then clears",
                    b.name()
                );
            }
        };
        for c in CLIMATES {
            for cover in COVERS {
                for (x, z) in spread() {
                    if let Some((surf, under)) = c.surface_palette(cover, x, z) {
                        held(surf, "Climate::surface_palette");
                        held(under, "Climate::surface_palette under");
                    }
                }
            }
        }
        use crate::strata::FloorCover;
        for cover in [FloorCover::Bare, FloorCover::Sparse, FloorCover::Wooded] {
            for (x, z) in spread() {
                let (surf, under) = crate::strata::desert_floor(x, z, cover);
                held(surf, "desert_floor");
                held(under, "desert_floor under");
            }
        }
    }

    // A surface an OSM pass wrote on purpose is not open countryside.
    #[test]
    fn authored_osm_surfaces_keep_their_own_planting() {
        assert!(
            !takes_wild_plants(PODZOL) && !takes_wild_trees(PODZOL),
            "landuse=cemetery writes podzol"
        );
        assert!(
            !takes_wild_plants(MOSS_BLOCK),
            "wetland puddle rings are moss"
        );
        assert!(
            !takes_wild_trees(STONE),
            "landuse=quarry, landuse=industrial and natural=bare_rock write stone"
        );
        assert!(
            !takes_wild_trees(COBBLESTONE),
            "natural=blockfield and natural=mountain_range write cobblestone"
        );
        assert!(
            !takes_wild_dead_bush(SAND),
            "sand is the hot-desert surface, not a bare patch to weed"
        );
        // Minecraft still holds a plant on those, which the sweep relies on.
        assert!(supports_vegetation(PODZOL) && supports_vegetation(MOSS_BLOCK));
    }

    #[test]
    fn sand_never_grows_plants() {
        assert!(!supports_vegetation(SAND));
        assert!(!supports_vegetation(GRAVEL));
        assert!(is_natural_ground(SAND));
    }
}
