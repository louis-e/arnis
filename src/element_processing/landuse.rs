use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::deterministic_rng::element_rng;
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::element_processing::field_texture::{FarmCrop, FieldCategory, FieldCell, FieldProfile};
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

    // Farmland texturing. Inactive under FieldPreset::Classic, in which case the whole
    // field pass is skipped and the surface stays exactly what it was before.
    let field_profile = FieldProfile::new(args.fields).with_map_scale(args.scale);
    let field_active = landuse_tag == "farmland" && field_profile.is_active();

    for &(x, z) in floor_area.iter() {
        // Resolved once per block and reused by both the surface and the decoration
        // pass, so a plot's crop, growth stage and track flag stay consistent.
        let field_cell = field_active.then(|| field_profile.cell_at(x, z));

        // Apply per-block randomness for certain landuse types
        let actual_block = if let Some(fc) = field_cell {
            fc.surface
        } else if landuse_tag == "industrial" {
            // Industrial: primarily stone, with some stone bricks and smooth stone
            let random_value = rng.random_range(0..100);
            if random_value < 70 {
                STONE
            } else if random_value < 90 {
                STONE_BRICKS
            } else {
                SMOOTH_STONE
            }
        } else if landuse_tag == "military" {
            // Military: primarily gray concrete, with some stone bricks and cobblestone
            let random_value = rng.random_range(0..100);
            if random_value < 89 {
                GRAY_CONCRETE
            } else if random_value < 99 {
                STONE_BRICKS
            } else {
                COBBLESTONE
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
                let random_choice: i32 = rng.random_range(0..100);
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
                    );
                } else {
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
            "farmland"
                if field_cell.is_some() && !editor.check_for_block(x, 0, z, Some(&[WATER])) =>
            {
                // Safe to unwrap: the guard above only matches when the cell exists.
                decorate_field(editor, &field_cell.unwrap(), x, z, &mut rng);
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
                        editor.set_block(OAK_LEAVES, x, 1, z, None, Some(&[SPONGE]));
                    }
                } else {
                    // Set crops only if the block below is farmland
                    if editor.check_for_block(x, 0, z, Some(&[FARMLAND])) {
                        let crop_choice = [WHEAT, CARROTS, POTATOES][rng.random_range(0..3)];
                        editor.set_block(crop_choice, x, 1, z, None, None);
                    }
                }
            }
            "construction" => {
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
                match rng.random_range(0..200) {
                    0 => editor.set_block(OAK_LEAVES, x, 1, z, None, None),
                    1..=8 => editor.set_block(FERN, x, 1, z, None, None),
                    9..=170 => editor.set_block(GRASS, x, 1, z, None, None),
                    _ => {}
                }
            }
            "greenfield" if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) => {
                match rng.random_range(0..200) {
                    0 => editor.set_block(OAK_LEAVES, x, 1, z, None, None),
                    1..=2 => editor.set_block(FERN, x, 1, z, None, None),
                    3..=16 => editor.set_block(GRASS, x, 1, z, None, None),
                    _ => {}
                }
            }
            "meadow" if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) => {
                let random_choice: i32 = rng.random_range(0..1001);
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
                    match rng.random_range(0..100) {
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
                match rng.random_range(0..150) {
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
                    let random_choice: i32 =
                        rng.random_range(0..100 + editor.get_absolute_y(x, 0, z)); // The deeper it is the more resources are there
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

/// Ground cover on grass. Style 0 is plain grass, 1 adds occasional tall grass, 2 is
/// the fuller mix with ferns.
fn place_grass_cover(editor: &mut WorldEditor, x: i32, z: i32, rng: &mut impl Rng, style: u8) {
    match style {
        0 => editor.set_block(GRASS, x, 1, z, None, None),
        1 => {
            if rng.random_range(0..22) == 0 {
                editor.set_block(TALL_GRASS_BOTTOM, x, 1, z, None, None);
                editor.set_block(TALL_GRASS_TOP, x, 2, z, None, None);
            } else {
                editor.set_block(GRASS, x, 1, z, None, None);
            }
        }
        _ => match rng.random_range(0..24) {
            0..=1 => editor.set_block(FERN, x, 1, z, None, None),
            2 => {
                editor.set_block(LARGE_FERN_LOWER, x, 1, z, None, None);
                editor.set_block(LARGE_FERN_UPPER, x, 2, z, None, None);
            }
            3 => {
                editor.set_block(TALL_GRASS_BOTTOM, x, 1, z, None, None);
                editor.set_block(TALL_GRASS_TOP, x, 2, z, None, None);
            }
            _ => editor.set_block(GRASS, x, 1, z, None, None),
        },
    }
}

/// Place a crop at the plot's growth level (0..=7), mapped to the crop's own max age.
/// The `age` NBT compound for each possible crop age, built once.
///
/// A crop block's properties are always `{age: "<0..=7>"}`, so a whole field of wheat
/// shares one of eight compounds. Building a fresh `HashMap`, two `String`s and an `Arc`
/// for every block placed, and then storing that per-block `Arc` in the section property
/// map, is most of the memory cost of field texturing: measured at roughly 3.8 GB of peak
/// RSS on a farmland-dense 3.3 x 4.7 km bbox. Interning makes placement a refcount bump,
/// and the NBT written is byte-identical.
fn crop_age_props(age: u8) -> std::sync::Arc<fastnbt::Value> {
    use std::sync::OnceLock;
    static AGES: OnceLock<Vec<std::sync::Arc<fastnbt::Value>>> = OnceLock::new();
    let table = AGES.get_or_init(|| {
        (0..=MAX_CROP_AGE)
            .map(|a| {
                let mut props: std::collections::HashMap<String, fastnbt::Value> =
                    std::collections::HashMap::new();
                props.insert("age".to_string(), fastnbt::Value::String(a.to_string()));
                std::sync::Arc::new(fastnbt::Value::Compound(props))
            })
            .collect()
    });
    // `age` is growth * max_age / 7 with both inputs <= 7, so it cannot exceed MAX_CROP_AGE.
    // Clamp rather than index blindly, so a future crop with a wider age range degrades to
    // the ripest compound instead of panicking part-way through a render.
    std::sync::Arc::clone(&table[age.min(MAX_CROP_AGE) as usize])
}

/// Highest `age` any vanilla crop uses (wheat/carrots/potatoes; beetroot stops at 3).
const MAX_CROP_AGE: u8 = 7;

fn place_crop(editor: &mut WorldEditor, base: Block, growth: u8, max_age: u8, x: i32, z: i32) {
    let age = (growth as u32 * max_age as u32 / 7) as u8;
    let bwp = BlockWithProperties::from_arc(base, Some(crop_age_props(age)));
    let ay = editor.get_absolute_y(x, 1, z);
    editor.set_block_with_properties_absolute(bwp, x, ay, z, None, None);
}

/// Pick from the parcel's 2-3 flower species. Real meadows carry a few species, not ten.
fn parcel_flower(cell: &FieldCell, rng: &mut impl Rng) -> Block {
    let n = FIELD_FLOWERS.len() as u32;
    let count = 2 + (cell.species_seed % 2); // 2 or 3 species per parcel
                                             // Each species slot draws from a different bit-window of the seed, so the
                                             // subset is genuinely varied rather than collapsing onto one species.
    let slot = rng.random_range(0..count);
    let idx = (cell.species_seed >> (5 * slot + 3)) % n;
    FIELD_FLOWERS[idx as usize]
}

/// Decorate one farmland cell according to its parcel style.
fn decorate_field(editor: &mut WorldEditor, cell: &FieldCell, x: i32, z: i32, rng: &mut impl Rng) {
    // Tracks are the worked ground between plots, so they stay clear.
    if cell.is_track {
        return;
    }
    match cell.cat {
        FieldCategory::Farm => decorate_farm_plot(editor, cell, x, z, rng),
        FieldCategory::Plains => {
            if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                // Vanilla-plains density: most grass cells carry cover.
                let style = ((cell.species_seed >> 16) % 3) as u8;
                match rng.random_range(0..1000) {
                    0..=780 => place_grass_cover(editor, x, z, rng, style),
                    // The odd wildflower or lone sunflower breaking up the grass.
                    781..=795 => {
                        let f = parcel_flower(cell, rng);
                        editor.set_block(f, x, 1, z, None, None);
                    }
                    796..=799 => {
                        editor.set_block(SUNFLOWER_LOWER, x, 1, z, None, None);
                        editor.set_block(SUNFLOWER_UPPER, x, 2, z, None, None);
                    }
                    _ => {}
                }
            } else if editor.check_for_block(x, 0, z, Some(&[COARSE_DIRT]))
                && rng.random_range(0..100) < 30
            {
                // Tufts poking through the bare specks.
                editor.set_block(GRASS, x, 1, z, None, None);
            }
        }
        FieldCategory::Flower => {
            if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                match rng.random_range(0..1000) {
                    // Calm flower cover: the parcel's own 2-3 species, kept sparse,
                    0..=105 => {
                        let f = parcel_flower(cell, rng);
                        editor.set_block(f, x, 1, z, None, None);
                    }
                    // with sunflowers sprinkled in,
                    106..=117 => {
                        editor.set_block(SUNFLOWER_LOWER, x, 1, z, None, None);
                        editor.set_block(SUNFLOWER_UPPER, x, 2, z, None, None);
                    }
                    // on a bed of mostly short grass (style 1 keeps ferns rare).
                    118..=580 => place_grass_cover(editor, x, z, rng, 1),
                    _ => {}
                }
            }
        }
        FieldCategory::Coarse => {
            // Dead bushes and ferns cluster on the coarse and rooted dirt, sparse grass
            // on the grass peeks. Packed mud and dirt-path patches stay bare.
            if editor.check_for_block(x, 0, z, Some(&[COARSE_DIRT, ROOTED_DIRT])) {
                match rng.random_range(0..100) {
                    0..=11 => editor.set_block(DEAD_BUSH, x, 1, z, None, None),
                    12..=17 => editor.set_block(FERN, x, 1, z, None, None),
                    18..=24 => editor.set_block(GRASS, x, 1, z, None, None),
                    _ => {}
                }
            } else if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                match rng.random_range(0..100) {
                    0..=5 => editor.set_block(DEAD_BUSH, x, 1, z, None, None),
                    6..=28 => place_grass_cover(editor, x, z, rng, 2),
                    _ => {}
                }
            }
        }
        FieldCategory::Moss => {
            if editor.check_for_block(x, 0, z, Some(&[MOSS_BLOCK, GRASS_BLOCK])) {
                match rng.random_range(0..100) {
                    0..=3 => editor.set_block(AZALEA, x, 1, z, None, None),
                    4..=30 => editor.set_block(MOSS_CARPET, x, 1, z, None, None),
                    31..=40 => place_grass_cover(editor, x, z, rng, 2),
                    41..=44 => editor.set_block(
                        FIELD_FLOWERS[rng.random_range(0..FIELD_FLOWERS.len())],
                        x,
                        1,
                        z,
                        None,
                        None,
                    ),
                    _ => {}
                }
            } else if editor.check_for_block(x, 0, z, Some(&[COARSE_DIRT, ROOTED_DIRT])) {
                match rng.random_range(0..100) {
                    0..=5 => editor.set_block(DEAD_BUSH, x, 1, z, None, None),
                    6..=12 => editor.set_block(FERN, x, 1, z, None, None),
                    _ => {}
                }
            }
        }
    }
}

/// Grow a farm plot's single crop. Tilled plots keep the enclosure-gated irrigation
/// dots, sunflower plots plant rows on their dirt rows, pumpkin patches fruit sparsely
/// on their grass and coarse mosaic, fallow fields carry stubble.
fn decorate_farm_plot(
    editor: &mut WorldEditor,
    cell: &FieldCell,
    x: i32,
    z: i32,
    rng: &mut impl Rng,
) {
    let Some(crop) = cell.crop else { return };
    match crop {
        FarmCrop::Wheat | FarmCrop::Potato | FarmCrop::Carrot | FarmCrop::Beetroot => {
            if x % 9 == 0 && z % 9 == 0 && editor.water_source_is_enclosed(x, z) {
                editor.set_block(WATER, x, 0, z, Some(&[FARMLAND]), None);
            } else if editor.check_for_block(x, 0, z, Some(&[FARMLAND])) {
                let (mut block, mut max_age) = match crop {
                    FarmCrop::Wheat => (WHEAT, 7),
                    FarmCrop::Potato => (POTATOES, 7),
                    FarmCrop::Carrot => (CARROTS, 7),
                    _ => (BEETROOTS, 3),
                };
                // Stray-seed patches: small pockets where another crop took root, as
                // birds carry seeds. Noise-clustered, so they read as patches rather
                // than as single scattered blocks.
                let stray =
                    (crate::ground_generation::value_noise_01(x + 321, z - 777, 4) * 1000.0) as i32;
                if stray < 22 {
                    let alt = [WHEAT, POTATOES, CARROTS, BEETROOTS]
                        [((cell.species_seed >> 9) % 4) as usize];
                    if alt != block {
                        max_age = if alt == BEETROOTS { 3 } else { 7 };
                        block = alt;
                    }
                }
                // Within-field growth jitter: most of the field sits at the field's own
                // stage, with younger spots mixed in, so it is not a uniform carpet.
                let mut growth = cell.crop_age;
                if rng.random_range(0..5) == 0 {
                    growth = growth.saturating_sub(1 + rng.random_range(0..2));
                }
                place_crop(editor, block, growth, max_age, x, z);
            } else if editor.check_for_block(x, 0, z, Some(&[COARSE_DIRT, ROOTED_DIRT])) {
                // Worn spots inside crop plots: a stray bale on wheat, the odd lone
                // sunflower in between, a little dry growth.
                match rng.random_range(0..100) {
                    0..=2 if crop == FarmCrop::Wheat => {
                        editor.set_block(HAY_BALE, x, 1, z, None, Some(&[SPONGE]));
                    }
                    3..=8 => {
                        editor.set_block(SUNFLOWER_LOWER, x, 1, z, None, None);
                        editor.set_block(SUNFLOWER_UPPER, x, 2, z, None, None);
                    }
                    9..=13 => editor.set_block(DEAD_BUSH, x, 1, z, None, None),
                    14..=20 => editor.set_block(GRASS, x, 1, z, None, None),
                    _ => {}
                }
            }
        }
        FarmCrop::Sunflower => {
            if editor.check_for_block(x, 0, z, Some(&[COARSE_DIRT])) {
                // Planted row: dense sunflowers.
                if rng.random_range(0..100) < 85 {
                    editor.set_block(SUNFLOWER_LOWER, x, 1, z, None, None);
                    editor.set_block(SUNFLOWER_UPPER, x, 2, z, None, None);
                }
            } else if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK]))
                && rng.random_range(0..100) < 25
            {
                editor.set_block(GRASS, x, 1, z, None, None);
            }
        }
        FarmCrop::Pumpkin => {
            if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                match rng.random_range(0..100) {
                    0..=5 => editor.set_block(PUMPKIN, x, 1, z, None, None),
                    6..=25 => editor.set_block(GRASS, x, 1, z, None, None),
                    26 => editor.set_block(DEAD_BUSH, x, 1, z, None, None),
                    _ => {}
                }
            } else if editor.check_for_block(x, 0, z, Some(&[COARSE_DIRT]))
                && rng.random_range(0..100) < 8
            {
                editor.set_block(GRASS, x, 1, z, None, None);
            }
        }
        FarmCrop::Fallow => {
            // Fallow is bare worked ground with no farmland under it, so nothing decays:
            // dry stubble of dead bushes and grasses on the diggable soil.
            if editor.check_for_block(x, 0, z, Some(&[COARSE_DIRT, ROOTED_DIRT])) {
                match rng.random_range(0..100) {
                    0..=5 => editor.set_block(DEAD_BUSH, x, 1, z, None, None),
                    6..=14 => editor.set_block(GRASS, x, 1, z, None, None),
                    15..=16 => editor.set_block(FERN, x, 1, z, None, None),
                    _ => {}
                }
            } else if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK]))
                && rng.random_range(0..100) < 20
            {
                place_grass_cover(editor, x, z, rng, 2);
            }
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
mod crop_props_tests {
    use super::*;

    /// The point of the table: placing a crop is a refcount bump, not a fresh allocation.
    /// Building the compound per block cost roughly 3.8 GB of peak RSS on a farmland-dense
    /// bbox, so a regression here is large and silent.
    #[test]
    fn the_same_age_returns_the_same_allocation() {
        assert!(std::sync::Arc::ptr_eq(
            &crop_age_props(3),
            &crop_age_props(3)
        ));
    }

    #[test]
    fn each_age_writes_its_own_value() {
        for age in 0..=MAX_CROP_AGE {
            let v = crop_age_props(age);
            let fastnbt::Value::Compound(map) = v.as_ref() else {
                panic!("crop properties must be a compound");
            };
            assert_eq!(
                map.get("age"),
                Some(&fastnbt::Value::String(age.to_string())),
                "age {age} wrote the wrong value"
            );
        }
    }

    /// An age beyond the table clamps to the ripest compound rather than panicking
    /// part-way through a render.
    #[test]
    fn an_out_of_range_age_clamps_instead_of_panicking() {
        assert!(std::sync::Arc::ptr_eq(
            &crop_age_props(200),
            &crop_age_props(MAX_CROP_AGE)
        ));
    }
}
