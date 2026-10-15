use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::deterministic_rng::element_rng;
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::element_processing::bush::{place_bush, BushKind};
use crate::element_processing::construction_site;
use crate::element_processing::tree::{Tree, TreeType};
use crate::floodfill_cache::{BuildingFootprintBitmap, FloodFillCache, RoadMaskBitmap};
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::prelude::IndexedRandom;
use rand::Rng;

pub fn generate_landuse(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    road_mask: &RoadMaskBitmap,
    bridge_surface: &BridgeSurfaceMap,
) {
    // Determine block type based on landuse tag
    let binding: String = "".to_string();
    let landuse_tag: &String = element.tags.get("landuse").unwrap_or(&binding);

    // Use deterministic RNG seeded by element ID for consistent results across region boundaries
    let mut rng = element_rng(element.id);

    let block_type = match landuse_tag.as_str() {
        "greenfield" | "meadow" | "grass" | "orchard" | "forest" => GRASS_BLOCK,
        "farmland" => FARMLAND,
        "cemetery" => PODZOL,
        "construction" => COARSE_DIRT,
        "traffic_island" => STONE_BLOCK_SLAB,
        // residential and commercial are too broad, they cover entire zones including
        // gardens, parks, and green spaces. ESA WorldCover handles built-up classification
        // at 10m satellite resolution, which is far more precise.
        "residential" | "commercial" => return,
        "education" => POLISHED_ANDESITE,
        "religious" => POLISHED_ANDESITE,
        "industrial" => STONE,     // Randomized per-block below
        "military" => GRASS_BLOCK, // Chosen per block by military_ground below
        "railway" => GRAVEL,
        "vineyard" => COARSE_DIRT,
        "flowerbed" => DIRT,
        "brownfield" => COARSE_DIRT,
        "farmyard" => COARSE_DIRT,
        "landfill" => {
            // Gravel if man_made = spoil_heap or heap, coarse dirt else
            let manmade_tag = element.tags.get("man_made").unwrap_or(&binding);
            if manmade_tag == "spoil_heap" || manmade_tag == "heap" {
                GRAVEL
            } else {
                COARSE_DIRT
            }
        }
        "quarry" => STONE, // Randomized per-block below
        _ => GRASS_BLOCK,
    };

    // Get the area of the landuse element using cache
    let floor_area = flood_fill_cache.get_or_compute(element, args.timeout.as_ref());

    let leaf_type_tagged = matches!(
        element.tags.get("leaf_type").map(String::as_str),
        Some("broadleaved" | "needleleaved")
    );
    // Cherry/FloweringOak only via the random Tree::create pool (rare).
    let trees_ok_to_generate: Vec<TreeType> = {
        let mut trees: Vec<TreeType> = vec![];
        if let Some(leaf_type) = element.tags.get("leaf_type") {
            match leaf_type.as_str() {
                "broadleaved" => {
                    trees.push(TreeType::Oak);
                    trees.push(TreeType::Birch);
                    trees.push(TreeType::TallOak);
                    trees.push(TreeType::Bush);
                    trees.push(TreeType::AzaleaBush);
                }
                "needleleaved" => {
                    trees.push(TreeType::Spruce);
                    trees.push(TreeType::Pine);
                }
                _ => {
                    trees.push(TreeType::Oak);
                    trees.push(TreeType::Spruce);
                    trees.push(TreeType::Birch);
                    trees.push(TreeType::TallOak);
                    trees.push(TreeType::Pine);
                    trees.push(TreeType::Bush);
                    trees.push(TreeType::AzaleaBush);
                    trees.push(TreeType::Willow);
                }
            }
        } else {
            trees.push(TreeType::Oak);
            trees.push(TreeType::Spruce);
            trees.push(TreeType::Birch);
            trees.push(TreeType::TallOak);
            trees.push(TreeType::Pine);
            trees.push(TreeType::Bush);
            trees.push(TreeType::AzaleaBush);
        }
        trees
    };

    let is_cemetery = landuse_tag == "cemetery";
    let is_military = landuse_tag == "military";
    // Training grounds and ranges are churned up far more than a barracks lawn.
    let military_rough = is_military
        && matches!(
            element.tags.get("military").map(String::as_str),
            Some("training_area" | "range" | "danger_area" | "trench")
        );
    let climate = editor.climate();
    let site_arid = construction_site::is_arid(climate);

    for &(x, z) in floor_area.iter() {
        // Apply per-block randomness for certain landuse types
        let actual_block = if landuse_tag == "industrial" {
            // Industrial: primarily stone, with some stone bricks and smooth stone
            let random_value = rng.random_range(0..100);
            if random_value < 70 {
                STONE
            } else if random_value < 90 {
                STONE_BRICKS
            } else {
                SMOOTH_STONE
            }
        } else if is_military {
            match military_ground(editor, climate, x, z, military_rough) {
                Some(block) => block,
                None => continue,
            }
        } else if landuse_tag == "quarry" {
            // Quarry: mix of stone, gravel, cobblestone, andesite
            let random_value = rng.random_range(0..100);
            if random_value < 40 {
                STONE
            } else if random_value < 60 {
                GRAVEL
            } else if random_value < 80 {
                COBBLESTONE
            } else {
                ANDESITE
            }
        } else {
            block_type
        };

        // Don't overwrite roads, paved areas or water with landuse ground blocks.
        // The mask catches the surfaces the block list cannot, such as a gravel
        // or dirt track that would otherwise be repainted as grass and then
        // planted on.
        let is_protected = editor.surface_is_sealed(x, z)
            || editor.check_for_block(
                x,
                0,
                z,
                Some(&[
                    BLACK_CONCRETE,
                    GRAY_CONCRETE_POWDER,
                    CYAN_TERRACOTTA,
                    GRAY_CONCRETE,
                    LIGHT_GRAY_CONCRETE,
                    WHITE_CONCRETE,
                    DIRT_PATH,
                    SMOOTH_STONE,
                    WATER,
                ]),
            );

        if landuse_tag == "traffic_island" {
            editor.set_block(actual_block, x, 1, z, None, None);
        } else if landuse_tag == "construction" {
            // Roads, paved yards and water in the site keep their surface.
            if !is_protected {
                let ground = construction_site::ground_block(x, z, site_arid);
                editor.set_block(ground, x, 0, z, None, Some(&[SPONGE]));
            }
        } else if landuse_tag == "railway" {
            editor.set_block(actual_block, x, 0, z, None, Some(&[SPONGE]));
        } else if !is_protected {
            editor.set_block(actual_block, x, 0, z, None, None);
        }

        // Nothing is scattered on land-cover water: the depth carve turns these
        // cells into lake after this runs, leaving plants floating on top. And
        // only this tile's own cells get plants and trees: a neighbouring tile's
        // halo copy draws its own random sequence, so its plants and trees would
        // land on this tile's water and under its trunks.
        if editor.is_lc_water(x, z) || !editor.owns(x, z) {
            continue;
        }

        // Add specific features for different landuse types
        match landuse_tag.as_str() {
            "cemetery" if (x % 3 == 0) && (z % 3 == 0) => {
                // Flowers and ground cover only; tombstones are stamped below in this loop.
                // 0..15 left empty to keep the original flower rates.
                let random_choice: i32 = rng.random_range(0..100);
                if (15..30).contains(&random_choice) {
                    if editor.check_for_block(x, 0, z, Some(&[PODZOL])) {
                        editor.set_block(RED_FLOWER, x, 1, z, None, None);
                    }
                } else if (30..33).contains(&random_choice) && editor.land_cover_backs_trees(x, z) {
                    Tree::create(
                        editor,
                        (x, 1, z),
                        Some(building_footprints),
                        Some(bridge_surface),
                    );
                } else if !is_protected && (33..35).contains(&random_choice) {
                    place_bush(editor, x, z, BushKind::Garden);
                } else if !is_protected && (35..37).contains(&random_choice) {
                    editor.set_block(FERN, x, 1, z, None, None);
                } else if !is_protected && (37..41).contains(&random_choice) {
                    editor.set_block(LARGE_FERN_LOWER, x, 1, z, None, None);
                    editor.set_block(LARGE_FERN_UPPER, x, 2, z, None, None);
                }
            }
            "forest" if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) => {
                // Density-modulated spawn: thickets in some patches, clearings in others.
                let density = crate::ground_generation::value_noise_01(x, z, 32);
                let tree_threshold = ((60.0 - density * 45.0) as i32).max(5);
                if rng.random_range(0..tree_threshold) == 0 {
                    let tree_type = *trees_ok_to_generate
                        .choose(&mut rng)
                        .unwrap_or(&TreeType::Oak);
                    Tree::create_of_type(
                        editor,
                        (x, 1, z),
                        tree_type,
                        Some(building_footprints),
                        Some(bridge_surface),
                        false,
                        leaf_type_tagged,
                    );
                } else {
                    let random_choice: i32 = rng.random_range(0..30);
                    if random_choice == 2 {
                        match rng.random_range(1..=6) {
                            1 => place_bush(editor, x, z, BushKind::Wild),
                            5 => editor.set_block(FERN, x, 1, z, None, None),
                            _ => crate::ground_decoration::place_scattered_flower(
                                editor,
                                x,
                                z,
                                crate::ground_decoration::FlowerSetting::Forest,
                            ),
                        }
                    } else if random_choice <= 12 {
                        if rng.random_range(0..100) < 12 {
                            editor.set_block(FERN, x, 1, z, None, None);
                        } else {
                            editor.set_block(GRASS, x, 1, z, None, None);
                        }
                    }
                }
            }
            "farmland" if !editor.check_for_block(x, 0, z, Some(&[WATER])) => {
                // Irrigation dots, but only where boxed in so they can't flow downhill and wash out crops.
                if x % 9 == 0 && z % 9 == 0 && editor.water_source_is_enclosed(x, z) {
                    editor.set_block(WATER, x, 0, z, Some(&[FARMLAND]), None);
                } else if rng.random_range(0..76) == 0 {
                    let special_choice: i32 = rng.random_range(1..=10);
                    if special_choice <= 4 {
                        editor.set_block(HAY_BALE, x, 1, z, None, Some(&[SPONGE]));
                    } else {
                        place_bush(editor, x, z, BushKind::Low);
                    }
                } else {
                    // Set crops only if the block below is farmland
                    if editor.check_for_block(x, 0, z, Some(&[FARMLAND])) {
                        let crop_choice = [WHEAT, CARROTS, POTATOES][rng.random_range(0..3)];
                        editor.set_block(crop_choice, x, 1, z, None, None);
                    }
                }
            }
            "grass" if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) => {
                match rng.random_range(0..200) {
                    0 => place_bush(editor, x, z, BushKind::Garden),
                    1..=8 => editor.set_block(FERN, x, 1, z, None, None),
                    9..=170 => editor.set_block(GRASS, x, 1, z, None, None),
                    _ => {}
                }
            }
            "flowerbed" if editor.check_for_block(x, 0, z, Some(&[DIRT])) => {
                crate::ground_decoration::place_bed_flower(editor, x, z);
            }
            "greenfield" if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) => {
                match rng.random_range(0..200) {
                    0 => place_bush(editor, x, z, BushKind::Wild),
                    1..=2 => editor.set_block(FERN, x, 1, z, None, None),
                    3..=16 => editor.set_block(GRASS, x, 1, z, None, None),
                    _ => {}
                }
            }
            "meadow" if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) => {
                let random_choice: i32 = rng.random_range(0..1001);
                if random_choice < 5 && editor.land_cover_backs_trees(x, z) {
                    Tree::create(
                        editor,
                        (x, 1, z),
                        Some(building_footprints),
                        Some(bridge_surface),
                    );
                } else if random_choice < 6 {
                    crate::ground_decoration::place_scattered_flower(
                        editor,
                        x,
                        z,
                        crate::ground_decoration::FlowerSetting::Meadow,
                    );
                } else if random_choice < 9 {
                    place_bush(editor, x, z, BushKind::Wild);
                } else if random_choice < 40 {
                    editor.set_block(FERN, x, 1, z, None, None);
                } else if random_choice < 65 {
                    editor.set_block(LARGE_FERN_LOWER, x, 1, z, None, None);
                    editor.set_block(LARGE_FERN_UPPER, x, 2, z, None, None);
                } else if random_choice < 825 {
                    editor.set_block(GRASS, x, 1, z, None, None);
                }
            }
            "orchard" => {
                if x % 18 == 0 && z % 10 == 0 {
                    Tree::create(
                        editor,
                        (x, 1, z),
                        Some(building_footprints),
                        Some(bridge_surface),
                    );
                } else if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                    match rng.random_range(0..100) {
                        0 => place_bush(editor, x, z, BushKind::Wild),
                        1..=2 => editor.set_block(FERN, x, 1, z, None, None),
                        3..=20 => editor.set_block(GRASS, x, 1, z, None, None),
                        _ => {}
                    }
                }
            }
            "vineyard" | "brownfield" | "landfill"
                if editor.check_for_block(x, 0, z, Some(&[COARSE_DIRT])) =>
            {
                // Sparse weeds/regrowth on coarse-dirt surfaces: vineyard rows
                // grow some grass between vines, and brownfield/landfill are
                // abandoned land that nature is slowly reclaiming. Kept rare so
                // the ground still reads as dry/disturbed rather than meadow.
                // (Skipped for landfill spoil heaps — those are GRAVEL, not
                // COARSE_DIRT, and the guard above filters them out.)
                match rng.random_range(0..150) {
                    0..=3 => place_bush(editor, x, z, BushKind::Low),
                    4 => editor.set_block(DEAD_BUSH, x, 1, z, None, None),
                    5..=15 => editor.set_block(GRASS, x, 1, z, None, None),
                    _ => {}
                }
            }
            "quarry" => {
                // Add stone layer under it
                editor.set_block(STONE, x, -1, z, Some(&[STONE]), None);
                editor.set_block(STONE, x, -2, z, Some(&[STONE]), None);
                // Generate ore blocks
                if let Some(resource) = element.tags.get("resource") {
                    let ore_block = match resource.as_str() {
                        "iron_ore" => IRON_ORE,
                        "coal" => COAL_ORE,
                        "copper" => COPPER_ORE,
                        "gold" => GOLD_ORE,
                        "clay" | "kaolinite" => CLAY,
                        _ => STONE,
                    };
                    // The deeper it is the more resources there are. Clamp the span
                    // to keep this valid even when the terrain floor goes below -100.
                    let ore_roll_span = 100_i32
                        .saturating_add(editor.get_absolute_y(x, 0, z))
                        .max(1);
                    let random_choice: i32 = rng.random_range(0..ore_roll_span);
                    if random_choice < 5 {
                        editor.set_block(ore_block, x, 0, z, Some(&[STONE]), None);
                    }
                }
            }
            _ => {}
        }

        if is_cemetery {
            crate::structures::tombstone::maybe_place(editor, x, z, road_mask);
        }
    }

    // Generate a stone brick wall fence around cemeteries
    if landuse_tag == "cemetery" {
        generate_cemetery_fence(editor, element);
    }

    // Large construction sites get a centre crane plus scattered excavators, and
    // every site its fence and props, which keep clear of both.
    if landuse_tag == "construction" {
        crate::structures::crane::maybe_place_crane(editor, floor_area.as_slice());
        crate::structures::excavator::scatter_excavators(editor, floor_area.as_slice());
        construction_site::furnish(editor, element, floor_area.as_slice(), building_footprints);
    }

    // Farmland fields rarely get a tractor.
    if landuse_tag == "farmland" {
        crate::structures::tractor::maybe_place_tractor(editor, floor_area.as_slice());
    }
}

/// Ground for `landuse=military`. A base is mown grass and worn training ground around a
/// paved core, and the land cover says which of those a cell is, so a base that is really a
/// forest, a heath or a desert keeps looking like one instead of turning into a concrete slab.
/// Water, wetland, beach and ice give `None` and stay with the land cover. Positional only, so the
/// tiles agree at their seams.
fn military_ground(
    editor: &WorldEditor,
    climate: crate::climate::Climate,
    x: i32,
    z: i32,
    rough: bool,
) -> Option<Block> {
    use crate::ground_generation::value_noise_01;
    use crate::land_cover::{
        coord_hash, LC_BARE, LC_BEACH, LC_BUILT_UP, LC_MANGROVES, LC_SNOW_ICE, LC_WATER, LC_WETLAND,
    };

    let cover = editor.cover_class(x, z);
    let h = coord_hash(x, z);
    match cover {
        LC_WATER | LC_WETLAND | LC_MANGROVES | LC_SNOW_ICE | LC_BEACH => None,
        LC_BUILT_UP => {
            // Concrete yards with gravel hardstands for the vehicles and strips of lawn.
            let n = value_noise_01(x + 211, z + 17, 7);
            Some(if n < 0.25 {
                GRASS_BLOCK
            } else if n > 0.8 {
                GRAVEL
            } else {
                match h % 10 {
                    0..=6 => POLISHED_ANDESITE,
                    7..=8 => ANDESITE,
                    _ => STONE,
                }
            })
        }
        _ => {
            // Arid and polar bases sit on the region's own ground.
            if let Some((surface, _)) = climate.surface_palette(cover, x, z) {
                return Some(surface);
            }
            // Vehicle tracks and training ground as organic patches in the grass: about a
            // seventh of a lawn, a third of a training area, most of bare land.
            let worn_share = if cover == LC_BARE {
                0.7
            } else if rough {
                0.4
            } else {
                0.25
            };
            if value_noise_01(x + 97, z + 31, 9) >= worn_share {
                return Some(GRASS_BLOCK);
            }
            Some(match h % 10 {
                0..=5 => COARSE_DIRT,
                6..=7 => DIRT,
                _ => GRAVEL,
            })
        }
    }
}

/// Draws a stone-brick wall fence (with slab cap) along the outline of a
/// cemetery way.
fn generate_cemetery_fence(editor: &mut WorldEditor, element: &ProcessedWay) {
    for i in 1..element.nodes.len() {
        let prev = &element.nodes[i - 1];
        let cur = &element.nodes[i];

        let points = bresenham_line(prev.x, 0, prev.z, cur.x, 0, cur.z);
        for (bx, _, bz) in points {
            editor.set_block(STONE_BRICK_WALL, bx, 1, bz, None, None);
            editor.set_block(STONE_BRICK_SLAB, bx, 2, bz, None, None);
        }
    }
}

pub fn generate_landuse_from_relation(
    editor: &mut WorldEditor,
    rel: &ProcessedRelation,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    road_mask: &RoadMaskBitmap,
    bridge_surface: &BridgeSurfaceMap,
) {
    if rel.tags.contains_key("landuse") {
        // Process each outer member way individually using cached flood fill.
        // We intentionally do not combine all outer nodes into one mega-way,
        // because that creates a nonsensical polygon spanning the whole relation
        // extent, misses the flood fill cache, and can cause multi-GB allocations.
        for member in &rel.members {
            if member.role == ProcessedMemberRole::Outer {
                // Use relation tags so the member inherits the relation's landuse=* type
                let way_with_rel_tags = ProcessedWay {
                    id: member.way.id,
                    nodes: member.way.nodes.clone(),
                    tags: rel.tags.clone(),
                };
                generate_landuse(
                    editor,
                    &way_with_rel_tags,
                    args,
                    flood_fill_cache,
                    building_footprints,
                    road_mask,
                    bridge_surface,
                );
            }
        }
    }
}

/// Generates ground blocks for place=* areas (squares, neighbourhoods, etc.)
pub fn generate_place(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
) {
    let binding = String::new();
    let place_tag = element.tags.get("place").unwrap_or(&binding);

    // Determine block type based on place tag
    let block_type = match place_tag.as_str() {
        "square" => STONE_BRICKS,
        // neighbourhood/city_block/quarter/suburb are too broad, ESA WorldCover
        // land cover data handles built-up classification at 10m resolution instead
        "neighbourhood" | "city_block" | "quarter" | "suburb" => return,
        _ => return,
    };

    // Get the area using flood fill cache
    let floor_area = flood_fill_cache.get_or_compute(element, args.timeout.as_ref());

    // Place ground blocks
    for &(x, z) in floor_area.iter() {
        editor.set_block(block_type, x, 0, z, None, None);
    }
}

#[cfg(test)]
mod sealed_surface_tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::bridge_styles::BridgeOutlineIndex;
    use crate::element_processing::bridges::{BridgeStructureMap, BridgeSurfaceMap};
    use crate::element_processing::building_test_support::{rect_way, test_editor};
    use crate::floodfill_cache::SealedSurfaceBitmap;
    use clap::Parser as _;
    use std::sync::Arc;

    #[test]
    fn a_forest_leaves_a_dirt_track_alone() {
        let xzbbox = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut editor = test_editor(&xzbbox);

        // Stand-in for a highway=track surface=dirt already rendered by the road pass.
        let mut mask = SealedSurfaceBitmap::new(&xzbbox);
        for z in 0..60 {
            mask.set(30, z);
            editor.set_block(DIRT, 30, 0, z, None, None);
        }
        editor.set_sealed_surface(Arc::new(mask));

        let outlines = BridgeOutlineIndex::build(&[]);
        let structures = BridgeStructureMap::build(&[], &editor, &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&[], &structures, 1.0);

        let args = Args::parse_from([
            "arnis",
            "--bbox",
            "1,2,3,4",
            "--mode",
            "geo-only",
            "--ground-level",
            "0",
        ]);
        let way = rect_way(1, 5, 5, 54, 54, &[("landuse", "forest")]);
        let cache = FloodFillCache::new();
        let footprints = BuildingFootprintBitmap::new_empty();
        let roads = RoadMaskBitmap::new_empty();
        generate_landuse(
            &mut editor,
            &way,
            &args,
            &cache,
            &footprints,
            &roads,
            &surface,
        );

        for z in 10..50 {
            assert!(
                editor.check_for_block(30, 0, z, Some(&[DIRT])),
                "the track keeps its dirt surface at z={z}"
            );
        }
        assert!(
            editor.check_for_block(20, 0, 20, Some(&[GRASS_BLOCK])),
            "the forest still paints the ground beside it"
        );
    }

    #[test]
    fn a_flowerbed_is_planted_soil() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let outlines = BridgeOutlineIndex::build(&[]);
        let structures = BridgeStructureMap::build(&[], &editor, &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&[], &structures, 1.0);
        let args = Args::parse_from([
            "arnis",
            "--bbox",
            "1,2,3,4",
            "--mode",
            "geo-only",
            "--ground-level",
            "0",
        ]);
        let way = rect_way(3, 5, 5, 34, 34, &[("landuse", "flowerbed")]);
        generate_landuse(
            &mut editor,
            &way,
            &args,
            &FloodFillCache::new(),
            &BuildingFootprintBitmap::new_empty(),
            &RoadMaskBitmap::new_empty(),
            &surface,
        );

        let (mut soil, mut flowers, mut cells) = (0, 0, 0);
        for x in 10..30 {
            for z in 10..30 {
                cells += 1;
                soil += editor.check_for_block(x, 0, z, Some(&[DIRT])) as i32;
                flowers +=
                    editor.block_exists_absolute(x, editor.get_absolute_y(x, 1, z), z) as i32;
            }
        }
        assert_eq!(soil, cells, "the bed is soil, not lawn");
        assert!(
            flowers * 10 > cells * 8,
            "nearly every cell is planted ({flowers}/{cells})"
        );
    }

    /// Paints one `landuse=military` square over x/z 5..=114 and counts the ground blocks
    /// of the inner 10..110 square as (grass, worn, paved, anything else).
    fn paint_military(
        editor: &mut WorldEditor,
        tags: &[(&str, &str)],
    ) -> (usize, usize, usize, usize) {
        let outlines = BridgeOutlineIndex::build(&[]);
        let structures = BridgeStructureMap::build(&[], editor, &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&[], &structures, 1.0);
        let args = Args::parse_from([
            "arnis",
            "--bbox",
            "1,2,3,4",
            "--mode",
            "geo-only",
            "--ground-level",
            "0",
        ]);
        let way = rect_way(7, 5, 5, 114, 114, tags);
        generate_landuse(
            editor,
            &way,
            &args,
            &FloodFillCache::new(),
            &BuildingFootprintBitmap::new_empty(),
            &RoadMaskBitmap::new_empty(),
            &surface,
        );
        let (mut grass, mut worn, mut paved, mut other) = (0, 0, 0, 0);
        for x in 10..110 {
            for z in 10..110 {
                if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                    grass += 1;
                } else if editor.check_for_block(x, 0, z, Some(&[COARSE_DIRT, DIRT, GRAVEL])) {
                    worn += 1;
                } else if editor.check_for_block(
                    x,
                    0,
                    z,
                    Some(&[POLISHED_ANDESITE, ANDESITE, STONE]),
                ) {
                    paved += 1;
                } else {
                    other += 1;
                }
            }
        }
        (grass, worn, paved, other)
    }

    #[test]
    fn military_ground_is_grass_with_worn_patches_not_concrete() {
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let (grass, worn, paved, other) = paint_military(&mut editor, &[("landuse", "military")]);
        let total = (grass + worn + paved + other) as f64;
        assert_eq!(
            other, 0,
            "every cell is painted, and none of it gray concrete"
        );
        assert_eq!(paved, 0, "without land cover nothing says built-up");
        let worn_share = worn as f64 / total;
        assert!(
            (0.05..0.3).contains(&worn_share),
            "worn patches stay a minority of a base's lawns ({worn_share:.2})"
        );

        let mut training = test_editor(&xzbbox);
        let (_, training_worn, _, _) = paint_military(
            &mut training,
            &[("landuse", "military"), ("military", "training_area")],
        );
        assert!(
            training_worn > worn,
            "a training area is more churned up than a plain base ({training_worn} vs {worn})"
        );
    }

    #[test]
    fn military_ground_follows_the_land_cover() {
        use crate::land_cover::{LandCoverData, LC_BUILT_UP, LC_WATER};
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        // West half built up, east half water.
        let lc = LandCoverData {
            grid: vec![vec![LC_BUILT_UP, LC_WATER]; 2],
            water_distance: vec![vec![0, 1]; 2],
            water_blend_cache: once_cell::sync::OnceCell::with_value(vec![vec![0.0, 1.0]; 2]),
            width: 2,
            height: 2,
            cells_per_meter: 1.0,
        };
        editor.set_ground(Arc::new(crate::ground::Ground::new_flat_land_cover_test(
            lc, 120, 120,
        )));
        paint_military(&mut editor, &[("landuse", "military")]);

        let (mut paved, mut west) = (0, 0);
        for x in 10..55 {
            for z in 10..110 {
                west += 1;
                if editor.check_for_block(x, 0, z, Some(&[POLISHED_ANDESITE, ANDESITE, STONE])) {
                    paved += 1;
                }
            }
        }
        assert!(
            paved * 2 > west,
            "the built-up core is mostly paved ({paved} of {west})"
        );
        for x in 65..110 {
            for z in 10..110 {
                assert!(
                    !editor.block_exists_absolute(x, 0, z),
                    "water at ({x}, {z}) is left to the land cover"
                );
            }
        }
    }

    #[test]
    fn quarry_ore_roll_handles_deep_terrain_without_panicking() {
        let xzbbox = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        editor.set_ground(Arc::new(crate::ground::Ground::new_flat(-250)));

        let outlines = BridgeOutlineIndex::build(&[]);
        let structures = BridgeStructureMap::build(&[], &editor, &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&[], &structures, 1.0);

        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4", "--mode", "geo-only"]);
        let way = rect_way(
            2,
            5,
            5,
            54,
            54,
            &[("landuse", "quarry"), ("resource", "coal")],
        );
        let cache = FloodFillCache::new();
        let footprints = BuildingFootprintBitmap::new_empty();
        let roads = RoadMaskBitmap::new_empty();
        generate_landuse(
            &mut editor,
            &way,
            &args,
            &cache,
            &footprints,
            &roads,
            &surface,
        );

        let mut ore_cells = 0usize;
        for x in 10..50 {
            for z in 10..50 {
                if editor.check_for_block(x, 0, z, Some(&[COAL_ORE])) {
                    ore_cells += 1;
                }
            }
        }
        assert!(
            ore_cells > 0,
            "deep quarries should still generate ore cells (found {ore_cells})"
        );
    }
}
