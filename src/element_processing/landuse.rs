use crate::args::Args;
use crate::block_definitions::*;
use crate::deterministic_rng::{coord_rng, element_rng};
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::element_processing::bush::{place_bush, BushKind};
use crate::element_processing::construction_site;
use crate::element_processing::field_texture::{place_cover, FieldLayout, Plot};
use crate::element_processing::tree::{Tree, TreeType};
use crate::floodfill_cache::{
    in_hole, without_holes, BuildingFootprintBitmap, FloodFillCache, Holes, InnerRings,
    RoadMaskBitmap,
};
use crate::ground_generation::FOREST_LITTER;
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
    landuse_area(
        editor,
        element,
        args,
        flood_fill_cache,
        building_footprints,
        road_mask,
        bridge_surface,
        None,
    );
}

/// A landuse area less the cells of `holes`.
#[allow(clippy::too_many_arguments)]
fn landuse_area(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    road_mask: &RoadMaskBitmap,
    bridge_surface: &BridgeSurfaceMap,
    holes: Option<&Holes>,
) {
    if !super::landuse_paints_ground(&element.tags) {
        return;
    }

    // Determine block type based on landuse tag
    let binding: String = "".to_string();
    let landuse_tag: &String = element.tags.get("landuse").unwrap_or(&binding);

    if matches!(
        landuse_tag.as_str(),
        "vineyard" | "orchard" | "allotments" | "greenhouse_horticulture"
    ) {
        let fill = flood_fill_cache.get_or_compute(element, args.timeout.as_ref());
        if landuse_tag == "greenhouse_horticulture" {
            super::greenhouses::horticulture_area(
                editor,
                element,
                &fill,
                road_mask,
                building_footprints,
                holes,
            );
        } else if landuse_tag == "allotments" {
            super::allotments::allotments_area(
                editor,
                element,
                &fill,
                road_mask,
                building_footprints,
                holes,
            );
        } else if landuse_tag == "vineyard" {
            super::plantations::vineyard_area(
                editor,
                element,
                &fill,
                road_mask,
                building_footprints,
                holes,
            );
        } else {
            super::plantations::orchard_area(
                editor,
                element,
                &fill,
                road_mask,
                building_footprints,
                holes,
            );
        }
        return;
    }

    if landuse_tag == "cemetery" {
        super::cemetery::cemetery_area(
            editor,
            element,
            args,
            flood_fill_cache,
            building_footprints,
            road_mask,
            bridge_surface,
            holes,
        );
        return;
    }

    // Use deterministic RNG seeded by element ID for consistent results across region boundaries
    let mut rng = element_rng(element.id);

    let block_type = match landuse_tag.as_str() {
        "greenfield" | "meadow" | "grass" | "forest" => GRASS_BLOCK,
        "farmland" => FARMLAND,
        "construction" => COARSE_DIRT,
        "traffic_island" => STONE_BLOCK_SLAB,
        "industrial" => STONE,     // Randomized per-block below
        "military" => GRASS_BLOCK, // Chosen per block by military_ground below
        "railway" => GRAVEL,
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

    // Get the area of the landuse element using cache; hole cells are skipped where read,
    // so the shared fill is not copied.
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

    let is_military = landuse_tag == "military";
    // Training grounds and ranges are churned up far more than a barracks lawn.
    let military_rough = is_military
        && matches!(
            element.tags.get("military").map(String::as_str),
            Some("training_area" | "range" | "danger_area" | "trench")
        );
    let area_climate = editor.climate();

    let field_layout = (landuse_tag == "farmland")
        .then(|| FieldLayout::new(args.fields, args.scale, element.id, &element.nodes))
        .flatten();

    let draws_per_cell = matches!(landuse_tag.as_str(), "industrial" | "quarry");
    let plants_here = crate::element_processing::natural::piece_plants_at(args);
    let woods_by_cell = crate::element_processing::natural::woods_by_cell(args);
    // Commercial land and the like are marked from the element list (`garden_areas`).
    let wild = matches!(landuse_tag.as_str(), "forest" | "meadow");
    let mown = mows(landuse_tag, floor_area.len(), args.scale);
    for &(x, z) in floor_area.iter() {
        if in_hole(holes, x, z) {
            continue;
        }
        // A tile reads the whole area, but nothing lands outside its editor; only the
        // per-cell draw has to happen there, so the cells it owns see the same sequence.
        // The main editor owns One World's clip margin, and every draw there still counts.
        if !editor.in_bounds(x, z) && !editor.owns(x, z) {
            if draws_per_cell {
                rng.random_range(0..100);
            }
            continue;
        }
        if wild {
            editor.mark_ground(crate::world_editor::GroundMark::Wild, x, z);
        }
        let plot = field_layout.as_ref().map(|l| l.plot_at(x, z));
        let climate = editor.local_climate(x, z).unwrap_or(area_climate);
        let forest_floor = (landuse_tag == "forest")
            .then(|| editor.forest_floor_at(x, z))
            .flatten();

        // Apply per-block randomness for certain landuse types
        let actual_block = if let Some(p) = &plot {
            p.surface(x, z)
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
        } else if let Some(floor) = forest_floor {
            floor
        } else {
            block_type
        };

        let is_protected = ground_is_protected(editor, x, z);

        if landuse_tag == "traffic_island" {
            editor.set_block(actual_block, x, 1, z, None, None);
        } else if landuse_tag == "construction" {
            // Roads, paved yards and water in the site keep their surface.
            if !is_protected {
                let ground =
                    construction_site::ground_block(x, z, construction_site::is_arid(climate));
                editor.set_block(ground, x, 0, z, None, Some(&[SPONGE]));
            }
        } else if landuse_tag == "railway" {
            // Ballast over whatever ground is there, but not over roads or water.
            if !is_protected {
                editor.set_block(actual_block, x, 0, z, None, Some(&[SPONGE]));
            }
        } else if !is_protected {
            editor.set_block(actual_block, x, 0, z, None, None);
        }
        if mown
            && !editor.surface_is_sealed(x, z)
            && editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK]))
            && crate::land_cover::mown_cover(editor.land_cover_class(x, z))
        {
            editor.mark_ground(crate::world_editor::GroundMark::Lawn, x, z);
        }

        // Nothing is scattered on land-cover water: the depth carve turns these
        // cells into lake after this runs, leaving plants floating on top. And
        // only this tile's own cells get plants and trees: a neighbouring tile's
        // halo copy draws its own random sequence, so its plants and trees would
        // land on this tile's water and under its trunks.
        if editor.is_lc_water(x, z) || !editor.owns(x, z) {
            continue;
        }
        // A piece's halo cells take their draws but root no trees or bushes.
        let roots = plants_here(x, z);

        // Add specific features for different landuse types
        match landuse_tag.as_str() {
            "forest"
                if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK]))
                    || editor.check_for_block(x, 0, z, Some(FOREST_LITTER)) =>
            {
                // Litter carries the odd fern, a sward under crowns thins out.
                let litter = editor.check_for_block(x, 0, z, Some(FOREST_LITTER));
                let floor = forest_floor.is_some();
                // Density-modulated spawn: thickets in some patches, clearings in others.
                let density = crate::ground_generation::value_noise_01(x, z, 32);
                let tree_threshold = ((60.0 - density * 45.0) as i32).max(5);
                let mut cell = woods_by_cell.then(|| coord_rng(x, z, element.id));
                if cell
                    .as_mut()
                    .unwrap_or(&mut rng)
                    .random_range(0..tree_threshold)
                    == 0
                {
                    let tree_type = *trees_ok_to_generate
                        .choose(cell.as_mut().unwrap_or(&mut rng))
                        .unwrap_or(&TreeType::Oak);
                    if roots || woods_by_cell {
                        Tree::create_of_type(
                            editor,
                            (x, 1, z),
                            tree_type,
                            Some(building_footprints),
                            Some(bridge_surface),
                            false,
                            leaf_type_tagged,
                        );
                    }
                } else {
                    let random_choice: i32 = rng.random_range(0..30);
                    if random_choice == 2 {
                        match rng.random_range(1..=6) {
                            1 if !roots => {}
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
                        let fern = rng.random_range(0..100) < 12;
                        if litter {
                            if random_choice <= 3 {
                                editor.set_block(FERN, x, 1, z, None, None);
                            }
                        } else if fern {
                            editor.set_block(FERN, x, 1, z, None, None);
                        } else if !floor || random_choice <= 5 {
                            editor.set_block(GRASS, x, 1, z, None, None);
                        }
                    }
                }
            }
            "farmland" if field_layout.is_some() => {
                if let Some(p) = plot.filter(|_| !is_protected) {
                    plant_plot(editor, &p, actual_block, x, z);
                }
            }
            "farmland" if !editor.check_for_block(x, 0, z, Some(&[WATER])) => {
                if rng.random_range(0..76) == 0 {
                    let special_choice: i32 = rng.random_range(1..=10);
                    if special_choice <= 4 {
                        editor.set_block(HAY_BALE, x, 1, z, None, Some(&[SPONGE]));
                    } else if roots {
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
                    0 if !roots => {}
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
                    0 if !roots => {}
                    0 => place_bush(editor, x, z, BushKind::Wild),
                    1..=2 => editor.set_block(FERN, x, 1, z, None, None),
                    3..=16 => editor.set_block(GRASS, x, 1, z, None, None),
                    _ => {}
                }
            }
            "meadow" if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) => {
                let random_choice: i32 = rng.random_range(0..1001);
                if random_choice < 5 && editor.land_cover_backs_trees(x, z) {
                    if roots {
                        Tree::create(
                            editor,
                            (x, 1, z),
                            Some(building_footprints),
                            Some(bridge_surface),
                        );
                    }
                } else if random_choice < 6 {
                    crate::ground_decoration::place_scattered_flower(
                        editor,
                        x,
                        z,
                        crate::ground_decoration::FlowerSetting::Meadow,
                    );
                } else if random_choice < 9 {
                    if roots {
                        place_bush(editor, x, z, BushKind::Wild);
                    }
                } else if random_choice < 40 {
                    editor.set_block(FERN, x, 1, z, None, None);
                } else if random_choice < 65 {
                    editor.set_block(LARGE_FERN_LOWER, x, 1, z, None, None);
                    editor.set_block(LARGE_FERN_UPPER, x, 2, z, None, None);
                } else if random_choice < 825 {
                    editor.set_block(GRASS, x, 1, z, None, None);
                }
            }
            "brownfield" | "landfill" if editor.check_for_block(x, 0, z, Some(&[COARSE_DIRT])) => {
                // Sparse regrowth on abandoned land; spoil heaps are gravel and skipped.
                match rng.random_range(0..150) {
                    0..=3 if !roots => {}
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
    }

    // Large construction sites get a centre crane plus scattered excavators, and
    // every site its fence and props, which keep clear of both.
    if landuse_tag == "construction" {
        let floor_area = without_holes(&floor_area, holes);
        crate::structures::crane::maybe_place_crane(
            editor,
            floor_area.as_slice(),
            building_footprints,
        );
        crate::structures::excavator::scatter_excavators(editor, floor_area.as_slice());
        construction_site::furnish(editor, element, floor_area.as_slice(), building_footprints);
    }

    // Farmland fields rarely get a tractor.
    if landuse_tag == "farmland" {
        let floor_area = without_holes(&floor_area, holes);
        crate::structures::tractor::maybe_place_tractor(editor, floor_area.as_slice());
    }
}

/// Grows a field plot's cell, which holds `surface` if nothing else claimed it first.
fn plant_plot(editor: &mut WorldEditor, plot: &Plot, surface: Block, x: i32, z: i32) {
    if !editor.check_for_block(x, 0, z, Some(&[surface])) {
        return;
    }
    if let Some(cover) = plot.cover(surface, x, z) {
        let ground_y = editor.get_absolute_y(x, 0, z);
        place_cover(editor, cover, x, ground_y, z);
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
            let red =
                crate::terrain_surface::Rock::at(climate, x, z, |sx, sz| editor.ecoregion(sx, sz))
                    == crate::terrain_surface::Rock::Red;
            if let Some((surface, _)) = climate.surface_palette(cover, x, z, red) {
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

/// Roads, paved areas and water, which landuse ground must not overwrite. The mask catches
/// the surfaces the block list cannot, such as a gravel or dirt track.
pub(crate) fn ground_is_protected(editor: &WorldEditor, x: i32, z: i32) -> bool {
    editor.surface_is_sealed(x, z)
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
        )
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
    if rel.tags.contains_key("landuse") && super::landuse_paints_ground(&rel.tags) {
        // Process each outer member way individually using cached flood fill.
        // We intentionally do not combine all outer nodes into one mega-way,
        // because that creates a nonsensical polygon spanning the whole relation
        // extent, misses the flood fill cache, and can cause multi-GB allocations.
        let inners = InnerRings::of(rel);
        for member in &rel.members {
            if member.role == ProcessedMemberRole::Outer {
                let holes = flood_fill_cache.holes_in(&member.way, &inners, args.timeout.as_ref());
                // Use relation tags so the member inherits the relation's landuse=* type
                let way_with_rel_tags = ProcessedWay {
                    id: member.way.id,
                    nodes: member.way.nodes.clone(),
                    tags: rel.tags.clone(),
                };
                landuse_area(
                    editor,
                    &way_with_rel_tags,
                    args,
                    flood_fill_cache,
                    building_footprints,
                    road_mask,
                    bridge_surface,
                    holes.as_ref(),
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

/// Largest recreation ground mown as a lawn, in m²: a sports field or a park lawn. A larger
/// one, an airfield's meadow or a common, keeps its tall grass.
const MOWN_RECREATION_M2: f64 = 50_000.0;

/// Whether a landuse area of `cells` blocks is mown: a village green, or a recreation
/// ground up to `MOWN_RECREATION_M2`. Not `landuse=grass`, which is also mapped on rural
/// meadows.
fn mows(landuse: &str, cells: usize, scale: f64) -> bool {
    match landuse {
        "village_green" => true,
        "recreation_ground" => (cells as f64) <= MOWN_RECREATION_M2 * scale * scale,
        _ => false,
    }
}

/// Calls `span(x0, x1, z)` for the filled row spans inside `bbox` of every closed way and
/// multipolygon whose tags `pick` takes, holes left out. `pick` is told whether it is
/// looking at a relation.
fn area_spans(
    elements: &[crate::osm_parser::ProcessedElement],
    bbox: &crate::coordinate_system::cartesian::XZBBox,
    pick: impl Fn(&std::collections::HashMap<String, String>, bool) -> bool,
    mut span: impl FnMut(i32, i32, i32),
) {
    use crate::coordinate_system::cartesian::XZPoint;
    use crate::osm_parser::{ProcessedElement, ProcessedNode};
    let ring = |nodes: &[ProcessedNode]| -> Option<Vec<XZPoint>> {
        let (first, last) = (nodes.first()?, nodes.last()?);
        (nodes.len() >= 4 && first.x == last.x && first.z == last.z)
            .then(|| nodes.iter().map(ProcessedNode::xz).collect())
    };
    let mut fill = |outers: Vec<Vec<XZPoint>>, inners: Vec<Vec<XZPoint>>| {
        let Some((z0, z1)) = outers
            .iter()
            .flatten()
            .fold(None, |b: Option<(i32, i32)>, p| {
                Some(b.map_or((p.z, p.z), |(lo, hi)| (lo.min(p.z), hi.max(p.z))))
            })
        else {
            return;
        };
        let edges = crate::element_processing::water_areas::PolygonEdges::new(&outers, &inners);
        for z in z0.max(bbox.min_z())..=z1.min(bbox.max_z()) {
            for (x0, x1) in edges.row_spans(z, bbox.min_x(), bbox.max_x()) {
                span(x0, x1, z);
            }
        }
    };
    for element in elements {
        match element {
            ProcessedElement::Way(way) if pick(&way.tags, false) => {
                if let Some(r) = ring(&way.nodes) {
                    fill(vec![r], Vec::new());
                }
            }
            ProcessedElement::Relation(rel) if pick(&rel.tags, true) => {
                let (mut outers, mut inners) = (Vec::new(), Vec::new());
                for m in &rel.members {
                    match m.role {
                        ProcessedMemberRole::Outer => outers.push(m.way.nodes.clone()),
                        ProcessedMemberRole::Inner => inners.push(m.way.nodes.clone()),
                        ProcessedMemberRole::Part => {}
                    }
                }
                super::merge_way_segments(&mut outers);
                super::merge_way_segments(&mut inners);
                let outers: Vec<_> = outers.iter().filter_map(|r| ring(r)).collect();
                let inners = inners.iter().filter_map(|r| ring(r)).collect();
                if !outers.is_empty() {
                    fill(outers, inners);
                }
            }
            _ => {}
        }
    }
}

/// Land whose open ground stays paved whatever stands near it: commercial and retail land,
/// and the grounds of schools, hospitals and town halls. Not church land, where the
/// churches are no homes but a rectory or a seminary's houses stand in green, nor a
/// kindergarten's, which is a play garden.
fn keeps_no_gardens(tags: &std::collections::HashMap<String, String>) -> bool {
    matches!(
        tags.get("landuse").map(String::as_str),
        Some("commercial" | "retail" | "education")
    ) || matches!(
        tags.get("amenity").map(String::as_str),
        Some("school" | "college" | "university" | "hospital" | "clinic" | "townhall")
    )
}

/// What the gardens read of the land, from the element list on their coarse grid: the
/// grounds that stay paved (`keeps_no_gardens`), and residential land. Ways and
/// multipolygons, holes left out. From the element list, since most of this land leaves its
/// ground to the land cover and is never filled while elements are placed.
pub fn garden_areas(
    elements: &[crate::osm_parser::ProcessedElement],
    xzbbox: &crate::coordinate_system::cartesian::XZBBox,
) -> (
    crate::floodfill_cache::CoarseMask,
    crate::floodfill_cache::CoarseMask,
) {
    let mut grounds = crate::floodfill_cache::CoarseMask::new(xzbbox);
    area_spans(
        elements,
        xzbbox,
        |tags, _| keeps_no_gardens(tags),
        |x0, x1, z| grounds.set_span(x0, x1, z),
    );
    let mut residential = crate::floodfill_cache::CoarseMask::new(xzbbox);
    area_spans(
        elements,
        xzbbox,
        |tags, _| tags.get("landuse").map(String::as_str) == Some("residential"),
        |x0, x1, z| residential.set_span(x0, x1, z),
    );
    (grounds, residential)
}

/// Marks the paved areas a garden keeps a street's margin from: squares, pedestrian areas,
/// `area:highway` surfaces, marketplaces, and car parks mapped as multipolygons (those
/// mapped as ways come in with their fills, `highways::collect_garden_streets`). Many of
/// these paint no ground of their own, so their land would read as open.
pub fn mark_paved_areas(
    elements: &[crate::osm_parser::ProcessedElement],
    bbox: &crate::coordinate_system::cartesian::XZBBox,
    streets: &mut crate::floodfill_cache::CoordinateBitmap,
) {
    let paved = |tags: &std::collections::HashMap<String, String>, relation: bool| {
        tags.contains_key("area:highway")
            || tags.get("place").map(String::as_str) == Some("square")
            || tags.get("amenity").map(String::as_str) == Some("marketplace")
            || (relation && super::highways::is_surface_car_park(tags))
            || (tags.contains_key("highway")
                && (relation || tags.get("area").map(String::as_str) == Some("yes")))
    };
    area_spans(elements, bbox, paved, |x0, x1, z| {
        for x in x0..=x1 {
            streets.set(x, z);
        }
    });
}

#[cfg(test)]
mod garden_areas_tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::osm_parser::{ProcessedElement, ProcessedMember, ProcessedNode};
    use std::collections::HashMap;

    fn square(id: u64, (x0, z0): (i32, i32), (x1, z1): (i32, i32)) -> ProcessedWay {
        let node = |i: u64, x: i32, z: i32| ProcessedNode {
            id: id * 10 + i,
            tags: HashMap::new(),
            x,
            z,
        };
        ProcessedWay {
            id,
            nodes: vec![
                node(0, x0, z0),
                node(1, x1, z0),
                node(2, x1, z1),
                node(3, x0, z1),
                node(0, x0, z0),
            ],
            tags: HashMap::new(),
        }
    }

    fn tagged(mut way: ProcessedWay, k: &str, v: &str) -> ProcessedWay {
        way.tags.insert(k.to_string(), v.to_string());
        way
    }

    /// Commercial land paints no ground and is never filled while elements are placed,
    /// so it is marked from the element list: ways and multipolygons, holes left out,
    /// retail as well. Residential land comes in its own mask.
    #[test]
    fn commercial_and_retail_land_is_marked_from_the_element_list() {
        let bbox = XZBBox::rect_from_min_max(0, 0, 199, 199).unwrap();
        let elements = vec![
            ProcessedElement::Way(tagged(
                square(1, (10, 10), (40, 40)),
                "landuse",
                "commercial",
            )),
            ProcessedElement::Way(tagged(square(2, (60, 10), (80, 30)), "landuse", "retail")),
            ProcessedElement::Way(tagged(
                square(3, (100, 10), (130, 40)),
                "landuse",
                "residential",
            )),
            ProcessedElement::Relation(crate::osm_parser::ProcessedRelation {
                id: 9,
                tags: HashMap::from([
                    ("type".to_string(), "multipolygon".to_string()),
                    ("landuse".to_string(), "commercial".to_string()),
                ]),
                members: vec![
                    ProcessedMember {
                        role: ProcessedMemberRole::Outer,
                        way: std::sync::Arc::new(square(4, (10, 100), (90, 180))),
                    },
                    ProcessedMember {
                        role: ProcessedMemberRole::Inner,
                        way: std::sync::Arc::new(square(5, (30, 120), (70, 160))),
                    },
                ],
            }),
        ];
        // The landuse pass itself leaves commercial land alone.
        assert!(!crate::element_processing::landuse_paints_ground(
            elements[0].tags()
        ));
        let (mask, residential) = garden_areas(&elements, &bbox);
        assert!(mask.contains(25, 25) && mask.contains(70, 20));
        assert!(!mask.contains(115, 25), "residential");
        assert!(residential.contains(115, 25) && !residential.contains(25, 25));
        assert!(mask.contains(20, 110) && mask.contains(85, 175));
        assert!(!mask.contains(50, 140), "the hole");
        assert!(!mask.contains(150, 150));
    }

    /// School and hospital grounds stay paved like commercial land, mapped as landuse or as
    /// the amenity's own area; church land, kindergartens and parks do not.
    #[test]
    fn school_and_hospital_grounds_keep_no_gardens() {
        let bbox = XZBBox::rect_from_min_max(0, 0, 199, 99).unwrap();
        let elements = vec![
            ProcessedElement::Way(tagged(square(1, (0, 0), (20, 20)), "landuse", "education")),
            ProcessedElement::Way(tagged(square(2, (30, 0), (50, 20)), "landuse", "retail")),
            ProcessedElement::Way(tagged(square(3, (60, 0), (80, 20)), "amenity", "school")),
            ProcessedElement::Way(tagged(square(4, (90, 0), (110, 20)), "amenity", "hospital")),
            ProcessedElement::Way(tagged(square(5, (120, 0), (140, 20)), "amenity", "clinic")),
            ProcessedElement::Way(tagged(
                square(6, (150, 0), (170, 20)),
                "amenity",
                "townhall",
            )),
            ProcessedElement::Way(tagged(square(7, (0, 50), (20, 70)), "amenity", "cafe")),
            ProcessedElement::Way(tagged(square(8, (30, 50), (50, 70)), "leisure", "park")),
            ProcessedElement::Way(tagged(
                square(9, (60, 50), (80, 70)),
                "landuse",
                "religious",
            )),
            ProcessedElement::Way(tagged(
                square(10, (90, 50), (110, 70)),
                "amenity",
                "kindergarten",
            )),
        ];
        let (grounds, _) = garden_areas(&elements, &bbox);
        for x in [10, 40, 70, 100, 130, 160] {
            assert!(grounds.contains(x, 10), "x={x}");
        }
        assert!(!grounds.contains(10, 60) && !grounds.contains(40, 60));
        assert!(!grounds.contains(70, 60), "church land");
        assert!(!grounds.contains(100, 60), "a kindergarten");
    }

    /// Squares, pedestrian areas and car parks mapped as multipolygons paint no ground of
    /// their own, so they are marked as paving from the element list, with `area:highway`
    /// surfaces and pedestrian areas mapped as ways. A footway or a lawn is not.
    #[test]
    fn squares_and_pedestrian_areas_are_paved_for_the_gardens() {
        let bbox = XZBBox::rect_from_min_max(0, 0, 199, 99).unwrap();
        let relation = |id: u64, pairs: &[(&str, &str)], outer: ProcessedWay| {
            let mut tags: HashMap<String, String> = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            tags.insert("type".to_string(), "multipolygon".to_string());
            ProcessedElement::Relation(crate::osm_parser::ProcessedRelation {
                id,
                tags,
                members: vec![ProcessedMember {
                    role: ProcessedMemberRole::Outer,
                    way: std::sync::Arc::new(outer),
                }],
            })
        };
        let elements = vec![
            relation(
                10,
                &[("place", "square"), ("highway", "pedestrian")],
                square(1, (0, 0), (20, 20)),
            ),
            relation(11, &[("amenity", "parking")], square(2, (30, 0), (50, 20))),
            relation(
                12,
                &[("amenity", "parking"), ("parking", "underground")],
                square(3, (60, 0), (80, 20)),
            ),
            ProcessedElement::Way(tagged(
                square(4, (90, 0), (110, 20)),
                "area:highway",
                "pedestrian",
            )),
            ProcessedElement::Way(tagged(
                tagged(square(5, (120, 0), (140, 20)), "highway", "pedestrian"),
                "area",
                "yes",
            )),
            ProcessedElement::Way(tagged(square(6, (150, 0), (170, 20)), "highway", "footway")),
            ProcessedElement::Way(tagged(square(7, (0, 50), (20, 70)), "landuse", "grass")),
            ProcessedElement::Way(tagged(
                square(8, (30, 50), (50, 70)),
                "amenity",
                "marketplace",
            )),
        ];
        let mut streets = crate::floodfill_cache::CoordinateBitmap::new(&bbox);
        mark_paved_areas(&elements, &bbox, &mut streets);
        for (x, z) in [(10, 10), (40, 10), (100, 10), (130, 10), (40, 60)] {
            assert!(streets.contains(x, z), "({x}, {z})");
        }
        assert!(!streets.contains(70, 10), "an underground car park");
        assert!(!streets.contains(160, 10), "a footway's ring is a line");
        assert!(!streets.contains(10, 60), "a lawn");
    }

    /// Greens and small recreation grounds are mown, a recreation ground of more than 5 ha
    /// keeps its tall grass, at any scale, and plain grass is never a lawn.
    #[test]
    fn only_greens_and_small_recreation_grounds_are_mown() {
        assert!(mows("village_green", 400_000, 1.0));
        assert!(mows("recreation_ground", 30_000, 1.0));
        assert!(!mows("recreation_ground", 60_000, 1.0));
        assert!(mows("recreation_ground", 12_000, 0.5));
        assert!(!mows("recreation_ground", 15_000, 0.5));
        assert!(mows("recreation_ground", 150_000, 2.0));
        assert!(!mows("recreation_ground", 250_000, 2.0));
        assert!(!mows("grass", 100, 1.0));
    }
}

#[cfg(test)]
mod sealed_surface_tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::bridge_styles::BridgeOutlineIndex;
    use crate::element_processing::bridges::{BridgeStructureMap, BridgeSurfaceMap};
    use crate::element_processing::building_test_support::{rect_way, test_editor};
    use crate::element_processing::highways::HighwayConnectivityMap;
    use crate::floodfill_cache::SealedSurfaceBitmap;
    use clap::Parser as _;
    use std::sync::Arc;

    #[test]
    fn a_landuse_relation_leaves_its_holes_alone() {
        use crate::osm_parser::{ProcessedElement, ProcessedMember};
        let xzbbox = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let outlines = BridgeOutlineIndex::build(&[]);
        let structures =
            BridgeStructureMap::build(&[], &editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
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
        // A railway corridor round a park, like the one round Disneyland Paris, with an
        // island of railway land in the middle of the park.
        let member = |role, way| ProcessedMember {
            role,
            way: Arc::new(way),
        };
        let rel = ProcessedRelation {
            id: 7,
            tags: [("landuse".to_string(), "railway".to_string())].into(),
            members: vec![
                member(ProcessedMemberRole::Outer, rect_way(1, 5, 5, 54, 54, &[])),
                member(ProcessedMemberRole::Inner, rect_way(2, 15, 15, 44, 44, &[])),
                member(ProcessedMemberRole::Outer, rect_way(3, 27, 27, 33, 33, &[])),
            ],
        };
        let cache = FloodFillCache::precompute(&[ProcessedElement::Relation(rel.clone())], None);
        editor.set_block(WATER, 8, 0, 8, None, None);
        generate_landuse_from_relation(
            &mut editor,
            &rel,
            &args,
            &cache,
            &BuildingFootprintBitmap::new_empty(),
            &RoadMaskBitmap::new_empty(),
            &surface,
        );
        assert!(
            editor.check_for_block(10, 0, 10, Some(&[GRAVEL])),
            "the corridor"
        );
        assert!(
            !editor.check_for_block(20, 0, 20, Some(&[GRAVEL])),
            "the park"
        );
        assert!(
            editor.check_for_block(30, 0, 30, Some(&[GRAVEL])),
            "the island"
        );
        assert!(
            editor.check_for_block(8, 0, 8, Some(&[WATER])),
            "a stream under the line"
        );
    }

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
        let structures =
            BridgeStructureMap::build(&[], &editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
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
    fn church_and_school_grounds_are_left_to_the_land_cover() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let outlines = BridgeOutlineIndex::build(&[]);
        let args = Args::parse_from([
            "arnis",
            "--bbox",
            "1,2,3,4",
            "--mode",
            "geo-only",
            "--ground-level",
            "0",
        ]);
        for landuse in ["religious", "education"] {
            let mut editor = test_editor(&xzbbox);
            let structures = BridgeStructureMap::build(
                &[],
                &editor,
                &HighwayConnectivityMap::new(),
                &outlines,
                1.0,
            );
            let surface = BridgeSurfaceMap::build(&[], &structures, 1.0);
            let way = rect_way(1, 5, 5, 34, 34, &[("landuse", landuse)]);
            generate_landuse(
                &mut editor,
                &way,
                &args,
                &FloodFillCache::new(),
                &BuildingFootprintBitmap::new_empty(),
                &RoadMaskBitmap::new_empty(),
                &surface,
            );
            assert!(
                !editor.check_for_block(20, 0, 20, Some(&[POLISHED_ANDESITE])),
                "landuse={landuse} paves its lawns"
            );
        }
    }

    #[test]
    fn a_flowerbed_is_planted_soil() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let outlines = BridgeOutlineIndex::build(&[]);
        let structures =
            BridgeStructureMap::build(&[], &editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
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
        let structures =
            BridgeStructureMap::build(&[], editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
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
            water_blend_cache: once_cell::sync::OnceCell::new(),
            beach_tiles: once_cell::sync::OnceCell::new(),
            inland_water: Default::default(),
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
        let structures =
            BridgeStructureMap::build(&[], &editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
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
