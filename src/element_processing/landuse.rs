use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::deterministic_rng::coord_rng;
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::element_processing::tree::{Tree, TreeType};
use crate::floodfill_cache::{BuildingFootprintBitmap, FloodFillCache, RoadMaskBitmap};
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::prelude::IndexedRandom;
use rand::Rng;

// Salts folded into the element id so that independent decisions taken at the
// same block never share one draw sequence. Every scatter below is seeded from
// the absolute (x, z) rather than streamed from a single per-element RNG, so a
// block decorates identically no matter which tile generated it. The values are
// arbitrary; only their distinctness matters.

/// Per-block ground variation (industrial/military/quarry surfaces).
const SALT_SURFACE: u64 = 0x9e37_79b9_7f4a_7c15;
/// Decoration scattered on top of the surface (plants, debris, crops).
const SALT_SCATTER: u64 = 0xc2b2_ae3d_27d4_eb4f;
/// Whether a tree spawns on this block.
const SALT_TREE: u64 = 0x1656_67b1_9e37_79f9;
/// Which species the tree on this block is.
const SALT_TREE_TYPE: u64 = 0xff51_afd7_ed55_8ccd;

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
        "industrial" => STONE,       // Randomized per-block below
        "military" => GRAY_CONCRETE, // Randomized per-block below
        "railway" => GRAVEL,
        "vineyard" => COARSE_DIRT,
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

    for &(x, z) in floor_area.iter() {
        // Apply per-block randomness for certain landuse types
        let actual_block = if landuse_tag == "industrial" {
            // Industrial: primarily stone, with some stone bricks and smooth stone
            let random_value = coord_rng(x, z, element.id ^ SALT_SURFACE).random_range(0..100);
            if random_value < 70 {
                STONE
            } else if random_value < 90 {
                STONE_BRICKS
            } else {
                SMOOTH_STONE
            }
        } else if landuse_tag == "military" {
            // Military: primarily gray concrete, with some stone bricks and cobblestone
            let random_value = coord_rng(x, z, element.id ^ SALT_SURFACE).random_range(0..100);
            if random_value < 89 {
                GRAY_CONCRETE
            } else if random_value < 99 {
                STONE_BRICKS
            } else {
                COBBLESTONE
            }
        } else if landuse_tag == "quarry" {
            // Quarry: mix of stone, gravel, cobblestone, andesite
            let random_value = coord_rng(x, z, element.id ^ SALT_SURFACE).random_range(0..100);
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

        // Don't overwrite roads or water with landuse ground blocks
        let is_protected = editor.check_for_block(
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
        } else if landuse_tag == "construction" || landuse_tag == "railway" {
            editor.set_block(actual_block, x, 0, z, None, Some(&[SPONGE]));
        } else if !is_protected {
            editor.set_block(actual_block, x, 0, z, None, None);
        }

        // Nothing is scattered on land-cover water: the depth carve turns these
        // cells into lake after this runs, leaving plants floating on top.
        if editor.is_lc_water(x, z) {
            continue;
        }

        // Add specific features for different landuse types
        match landuse_tag.as_str() {
            "cemetery" if (x % 3 == 0) && (z % 3 == 0) => {
                // Flowers and ground cover only; tombstones are stamped below in this loop.
                // 0..15 left empty to keep the original flower rates.
                let random_choice: i32 =
                    coord_rng(x, z, element.id ^ SALT_SCATTER).random_range(0..100);
                if (15..30).contains(&random_choice) {
                    if editor.check_for_block(x, 0, z, Some(&[PODZOL])) {
                        editor.set_block(RED_FLOWER, x, 1, z, None, None);
                    }
                } else if (30..33).contains(&random_choice) {
                    Tree::create(
                        editor,
                        (x, 1, z),
                        Some(building_footprints),
                        Some(bridge_surface),
                    );
                } else if !is_protected && (33..35).contains(&random_choice) {
                    editor.set_block(OAK_LEAVES, x, 1, z, None, None);
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
                if coord_rng(x, z, element.id ^ SALT_TREE).random_range(0..tree_threshold) == 0 {
                    let tree_type = *trees_ok_to_generate
                        .choose(&mut coord_rng(x, z, element.id ^ SALT_TREE_TYPE))
                        .unwrap_or(&TreeType::Oak);
                    Tree::create_of_type(
                        editor,
                        (x, 1, z),
                        tree_type,
                        Some(building_footprints),
                        Some(bridge_surface),
                        false,
                    );
                } else {
                    let mut rng = coord_rng(x, z, element.id ^ SALT_SCATTER);
                    let random_choice: i32 = rng.random_range(0..30);
                    if random_choice == 2 {
                        let flower_block: Block = match rng.random_range(1..=6) {
                            1 => OAK_LEAVES,
                            2 => RED_FLOWER,
                            3 => BLUE_FLOWER,
                            4 => YELLOW_FLOWER,
                            5 => FERN,
                            _ => WHITE_FLOWER,
                        };
                        editor.set_block(flower_block, x, 1, z, None, None);
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
                } else {
                    let mut rng = coord_rng(x, z, element.id ^ SALT_SCATTER);
                    if rng.random_range(0..76) == 0 {
                        let special_choice: i32 = rng.random_range(1..=10);
                        if special_choice <= 4 {
                            editor.set_block(HAY_BALE, x, 1, z, None, Some(&[SPONGE]));
                        } else {
                            editor.set_block(OAK_LEAVES, x, 1, z, None, Some(&[SPONGE]));
                        }
                    } else if editor.check_for_block(x, 0, z, Some(&[FARMLAND])) {
                        // Set crops only if the block below is farmland
                        let crop_choice = [WHEAT, CARROTS, POTATOES][rng.random_range(0..3)];
                        editor.set_block(crop_choice, x, 1, z, None, None);
                    }
                }
            }
            "construction" => {
                let mut rng = coord_rng(x, z, element.id ^ SALT_SCATTER);
                let random_choice: i32 = rng.random_range(0..1501);
                if random_choice < 15 {
                    editor.set_block(SCAFFOLDING, x, 1, z, None, None);
                    if random_choice < 2 {
                        editor.set_block(SCAFFOLDING, x, 2, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 3, z, None, None);
                    } else if random_choice < 4 {
                        editor.set_block(SCAFFOLDING, x, 2, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 3, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 4, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 1, z + 1, None, None);
                    } else {
                        editor.set_block(SCAFFOLDING, x, 2, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 3, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 4, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 5, z, None, None);
                        editor.set_block(SCAFFOLDING, x - 1, 1, z, None, None);
                        editor.set_block(SCAFFOLDING, x + 1, 1, z - 1, None, None);
                    }
                } else if random_choice < 55 {
                    let construction_items: [Block; 13] = [
                        OAK_LOG,
                        COBBLESTONE,
                        GRAVEL,
                        GLOWSTONE,
                        STONE,
                        COBBLESTONE_WALL,
                        BLACK_CONCRETE,
                        SAND,
                        OAK_PLANKS,
                        DIRT,
                        BRICK,
                        CRAFTING_TABLE,
                        FURNACE,
                    ];
                    editor.set_block(
                        construction_items[rng.random_range(0..construction_items.len())],
                        x,
                        1,
                        z,
                        None,
                        None,
                    );
                } else if random_choice < 65 {
                    if random_choice < 60 {
                        editor.set_block(DIRT, x, 1, z, None, None);
                        editor.set_block(DIRT, x, 2, z, None, None);
                        editor.set_block(DIRT, x + 1, 1, z, None, None);
                        editor.set_block(DIRT, x, 1, z + 1, None, None);
                    } else {
                        editor.set_block(DIRT, x, 1, z, None, None);
                        editor.set_block(DIRT, x, 2, z, None, None);
                        editor.set_block(DIRT, x - 1, 1, z, None, None);
                        editor.set_block(DIRT, x, 1, z - 1, None, None);
                    }
                } else if random_choice < 100 {
                    editor.set_block(GRAVEL, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 115 {
                    editor.set_block(SAND, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 125 {
                    editor.set_block(DIORITE, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 145 {
                    editor.set_block(BRICK, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 155 {
                    editor.set_block(GRANITE, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 180 {
                    editor.set_block(ANDESITE, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 565 {
                    editor.set_block(COBBLESTONE, x, 0, z, None, Some(&[SPONGE]));
                }
            }
            "grass" if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) => {
                match coord_rng(x, z, element.id ^ SALT_SCATTER).random_range(0..200) {
                    0 => editor.set_block(OAK_LEAVES, x, 1, z, None, None),
                    1..=8 => editor.set_block(FERN, x, 1, z, None, None),
                    9..=170 => editor.set_block(GRASS, x, 1, z, None, None),
                    _ => {}
                }
            }
            "greenfield" if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) => {
                match coord_rng(x, z, element.id ^ SALT_SCATTER).random_range(0..200) {
                    0 => editor.set_block(OAK_LEAVES, x, 1, z, None, None),
                    1..=2 => editor.set_block(FERN, x, 1, z, None, None),
                    3..=16 => editor.set_block(GRASS, x, 1, z, None, None),
                    _ => {}
                }
            }
            "meadow" if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) => {
                let random_choice: i32 =
                    coord_rng(x, z, element.id ^ SALT_SCATTER).random_range(0..1001);
                if random_choice < 5 {
                    Tree::create(
                        editor,
                        (x, 1, z),
                        Some(building_footprints),
                        Some(bridge_surface),
                    );
                } else if random_choice < 6 {
                    editor.set_block(RED_FLOWER, x, 1, z, None, None);
                } else if random_choice < 9 {
                    editor.set_block(OAK_LEAVES, x, 1, z, None, None);
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
                    match coord_rng(x, z, element.id ^ SALT_SCATTER).random_range(0..100) {
                        0 => editor.set_block(OAK_LEAVES, x, 1, z, None, None),
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
                match coord_rng(x, z, element.id ^ SALT_SCATTER).random_range(0..150) {
                    0..=3 => editor.set_block(OAK_LEAVES, x, 1, z, None, None),
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
                    // The deeper it is the more resources are there
                    let random_choice: i32 = coord_rng(x, z, element.id ^ SALT_SCATTER)
                        .random_range(0..100 + editor.get_absolute_y(x, 0, z));
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

    // Large construction sites get a centre crane plus scattered excavators.
    if landuse_tag == "construction" {
        crate::structures::crane::maybe_place_crane(editor, floor_area.as_slice());
        crate::structures::excavator::scatter_excavators(editor, floor_area.as_slice());
    }

    // Farmland fields rarely get a tractor.
    if landuse_tag == "farmland" {
        crate::structures::tractor::maybe_place_tractor(editor, floor_area.as_slice());
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
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{rect_way, test_editor};
    use clap::Parser as _;

    // A tile is generated with a margin and only its inner part is kept, so the
    // seam test below renders one 64x16 area three ways: once whole, and once
    // per 32-wide half with a 16-block margin on the seam side. A decoration
    // anchored just outside a half still lands inside it thanks to the margin,
    // so the two inner halves must reassemble into exactly the whole.
    const AREA_MAX_X: i32 = 63;
    const AREA_MAX_Z: i32 = 15;
    const HALF: i32 = 32;
    const MARGIN: i32 = 16;

    /// Every block the editor holds in `min_x..=max_x` over the test area, in a
    /// fixed (x, z, y) walk order. Test editors have no elevation data, so
    /// ground level is 0 and offset Y equals absolute Y.
    fn snapshot(editor: &WorldEditor, min_x: i32, max_x: i32) -> Vec<(i32, i32, i32, u16)> {
        let mut out: Vec<(i32, i32, i32, u16)> = Vec::new();
        for x in min_x..=max_x {
            for z in 0..=AREA_MAX_Z {
                // Widest vertical span anything here writes: a quarry digs to
                // -2, the tallest procedural tree tops out around +31.
                for y in -8..=40 {
                    if let Some(block) = editor.get_block_absolute(x, y, z) {
                        out.push((x, y, z, block.id()));
                    }
                }
            }
        }
        out
    }

    /// An empty bridge deck map, the shape these processors expect when no
    /// bridge is anywhere near.
    fn empty_bridge_surface(xzbbox: &XZBBox) -> BridgeSurfaceMap {
        let editor = test_editor(xzbbox);
        let outlines = crate::element_processing::bridge_styles::BridgeOutlineIndex::build(&[]);
        let structures =
            crate::element_processing::bridges::BridgeStructureMap::build(&[], &editor, &outlines);
        BridgeSurfaceMap::build(&[], &structures, 1.0)
    }

    fn seam_test_args() -> Args {
        Args::parse_from(["arnis", "--bbox", "1,2,3,4"])
    }

    /// Asserts the two inner halves reassemble into the whole, and that the
    /// render is varied enough for a misalignment to have shown up.
    fn assert_seam_free(
        label: &str,
        whole: Vec<(i32, i32, i32, u16)>,
        halves: Vec<(i32, i32, i32, u16)>,
    ) {
        assert!(!whole.is_empty(), "{label} rendered nothing");
        let distinct: std::collections::BTreeSet<u16> = whole.iter().map(|b| b.3).collect();
        assert!(
            distinct.len() >= 2,
            "{label} rendered a single block type, so the seam check is vacuous"
        );
        assert_eq!(
            whole, halves,
            "{label} renders differently either side of a tile seam"
        );
    }

    fn render_landuse(
        xzbbox: &XZBBox,
        way: &ProcessedWay,
        min_x: i32,
        max_x: i32,
    ) -> Vec<(i32, i32, i32, u16)> {
        let bridges = empty_bridge_surface(xzbbox);
        let footprints = BuildingFootprintBitmap::new_empty();
        let roads = RoadMaskBitmap::new_empty();
        let mut editor = test_editor(xzbbox);
        generate_landuse(
            &mut editor,
            way,
            &seam_test_args(),
            &FloodFillCache::new(),
            &footprints,
            &roads,
            &bridges,
        );
        snapshot(&editor, min_x, max_x)
    }

    /// Scatter is addressed by absolute position, so splitting an area at a
    /// tile seam must not shift a single block.
    #[test]
    fn scatter_is_identical_across_a_tile_seam() {
        let whole_bbox = XZBBox::rect_from_min_max(0, 0, AREA_MAX_X, AREA_MAX_Z).unwrap();
        let left_bbox = XZBBox::rect_from_min_max(0, 0, HALF - 1 + MARGIN, AREA_MAX_Z).unwrap();
        let right_bbox =
            XZBBox::rect_from_min_max(HALF - MARGIN, 0, AREA_MAX_X, AREA_MAX_Z).unwrap();

        // Types whose decoration is stamped by a structure placer (cemetery,
        // construction, farmland) are left out: those placers pick one spot for
        // a whole field and are not what this test is about.
        for landuse_tag in [
            "grass",
            "greenfield",
            "meadow",
            "orchard",
            "forest",
            "industrial",
            "military",
            "quarry",
            "vineyard",
            "brownfield",
            "landfill",
        ] {
            let mut tags: Vec<(&str, &str)> = vec![("landuse", landuse_tag)];
            if landuse_tag == "quarry" {
                tags.push(("resource", "iron_ore"));
            }
            let way = rect_way(7001, 0, 0, AREA_MAX_X, AREA_MAX_Z, &tags);

            let whole = render_landuse(&whole_bbox, &way, 0, AREA_MAX_X);
            let mut halves = render_landuse(&left_bbox, &way, 0, HALF - 1);
            halves.extend(render_landuse(&right_bbox, &way, HALF, AREA_MAX_X));

            assert_seam_free(&format!("landuse={landuse_tag}"), whole, halves);
        }
    }
}
