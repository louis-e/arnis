use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZPoint;
use crate::deterministic_rng::element_rng;
use crate::floodfill_cache::{FloodFillCache, RoadMaskBitmap};
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use rand::{
    prelude::{IndexedRandom, SliceRandom},
    Rng,
};
use std::collections::{HashMap, HashSet};

/// Looks outward from (x, z) in each of the four cardinal directions,
/// up to max_radius blocks away, and returns the (x, z) position of
/// the nearest road node found.
///
/// Returns None if no road node exists within range.
/// Callers can use the returned position to derive a facing direction,
/// compute a distance, or do anything else they need.
fn get_nearest_road_block(
    x: i32,
    z: i32,
    max_radius: i32,
    road_mask: &RoadMaskBitmap,
) -> Option<(i32, i32)> {
    // Begins at 2 and skips to 4, 6, 8, etc.
    for dist in (2..=max_radius).step_by(2) {
        // Cross pattern: North, South, West, East
        let candidates = [(x, z - dist), (x, z + dist), (x - dist, z), (x + dist, z)];

        for (cx, cz) in candidates {
            if road_mask.contains(cx, cz) {
                return Some((cx, cz));
            }
        }
    }

    None
}

pub fn generate_amenities(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    road_mask: &RoadMaskBitmap,
) {
    // Skip if 'layer' or 'level' is negative in the tags
    if let Some(layer) = element.tags().get("layer") {
        if layer.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(level) = element.tags().get("level") {
        if level.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(amenity_type) = element.tags().get("amenity") {
        let first_node: Option<XZPoint> = element
            .nodes()
            .map(|n: &crate::osm_parser::ProcessedNode| XZPoint::new(n.x, n.z))
            .next();
        match amenity_type.as_str() {
            "recycling" => {
                let is_container = element
                    .tags()
                    .get("recycling_type")
                    .is_some_and(|value| value == "container");

                if !is_container {
                    return;
                }

                if let Some(pt) = first_node {
                    let mut rng = rand::rng();
                    let loot_pool = build_recycling_loot_pool(element.tags());
                    let items = build_recycling_items(&loot_pool, &mut rng);

                    let properties = Value::Compound(recycling_barrel_properties());
                    let barrel_block = BlockWithProperties::new(BARREL, Some(properties));
                    let absolute_y = editor.get_absolute_y(pt.x, 1, pt.z);

                    editor.set_block_entity_with_items(
                        barrel_block,
                        pt.x,
                        1,
                        pt.z,
                        "minecraft:barrel",
                        items,
                    );

                    if let Some(category) = single_loot_category(&loot_pool) {
                        if let Some(display_item) =
                            build_display_item_for_category(category, &mut rng)
                        {
                            place_item_frame_on_random_side(
                                editor,
                                pt.x,
                                absolute_y,
                                pt.z,
                                display_item,
                            );
                        }
                    }
                }
            }
            "waste_disposal" | "waste_basket" => {
                // Place a cauldron for waste disposal or waste basket
                if let Some(pt) = first_node {
                    editor.set_block(CAULDRON, pt.x, 1, pt.z, None, None);
                }
            }
            "vending_machine" | "atm" => {
                if let Some(pt) = first_node {
                    editor.set_block(IRON_BLOCK, pt.x, 1, pt.z, None, None);
                    editor.set_block(IRON_BLOCK, pt.x, 2, pt.z, None, None);
                }
            }
            "bicycle_parking" => {
                let ground_block: Block = OAK_PLANKS;
                let roof_block: Block = STONE_BLOCK_SLAB;

                // Use pre-computed flood fill from cache
                let floor_area: Vec<(i32, i32)> =
                    flood_fill_cache.get_or_compute_element(element, args.timeout.as_ref());

                if floor_area.is_empty() {
                    return;
                }

                // Fill the floor area
                for (x, z) in floor_area.iter() {
                    editor.set_block(ground_block, *x, 0, *z, None, None);
                }

                // Place fences and roof slabs at each corner node
                for node in element.nodes() {
                    let x: i32 = node.x;
                    let z: i32 = node.z;

                    // Set ground block and fences
                    editor.set_block(ground_block, x, 0, z, None, None);
                    for y in 1..=4 {
                        editor.set_block(OAK_FENCE, x, y, z, None, None);
                    }
                    editor.set_block(roof_block, x, 5, z, None, None);
                }

                // Flood fill the roof area
                for (x, z) in floor_area.iter() {
                    editor.set_block(roof_block, *x, 5, *z, None, None);
                }
            }
            "bench" => {
                // Place a bench
                if let Some(pt) = first_node {
                    let mut rng = element_rng(element.id());
                    let road_pos = get_nearest_road_block(pt.x, pt.z, 4, road_mask);

                    let use_east_west = if let Some((rx, rz)) = road_pos {
                        let dx = (rx - pt.x).abs();
                        let dz = (rz - pt.z).abs();
                        dz >= dx
                    } else {
                        rng.random_bool(0.5)
                    };

                    // facing_a and facing_b must face AWAY from the center (pt.x, pt.z)
                    let (facing_a, facing_b, dx, dz) = if use_east_west {
                        // Bench stretches along X axis.
                        // Stair A is at -1 (West), so it faces West.
                        // Stair B is at +1 (East), so it faces East.
                        (StairFacing::West, StairFacing::East, 1, 0)
                    } else {
                        // Bench stretches along Z axis.
                        // Stair A is at -1 (North), so it faces North.
                        // Stair B is at +1 (South), so it faces South.
                        (StairFacing::North, StairFacing::South, 0, 1)
                    };

                    let abs_y = editor.get_absolute_y(pt.x, 1, pt.z);
                    let bench_blacklist = [OAK_LOG, SPRUCE_LOG];
                    //place bench
                    let stair_a = top_stair(create_stair_with_properties(
                        OAK_STAIRS,
                        facing_a,
                        StairShape::Straight,
                    ));
                    editor.set_block_with_properties_absolute(
                        stair_a,
                        pt.x - dx,
                        abs_y,
                        pt.z - dz,
                        None,
                        Some(&bench_blacklist),
                    );

                    editor.set_block(OAK_SLAB_TOP, pt.x, 1, pt.z, None, Some(&bench_blacklist));

                    let stair_b = top_stair(create_stair_with_properties(
                        OAK_STAIRS,
                        facing_b,
                        StairShape::Straight,
                    ));
                    editor.set_block_with_properties_absolute(
                        stair_b,
                        pt.x + dx,
                        abs_y,
                        pt.z + dz,
                        None,
                        Some(&bench_blacklist),
                    );
                }
            }
            "shelter" => {
                let roof_block: Block = STONE_BRICK_SLAB;

                // Use pre-computed flood fill from cache
                let roof_area: Vec<(i32, i32)> =
                    flood_fill_cache.get_or_compute_element(element, args.timeout.as_ref());

                // Place fences and roof slabs at each corner node directly
                for node in element.nodes() {
                    let x: i32 = node.x;
                    let z: i32 = node.z;

                    for fence_height in 1..=4 {
                        editor.set_block(OAK_FENCE, x, fence_height, z, None, None);
                    }
                    editor.set_block(roof_block, x, 5, z, None, None);
                }

                // Flood fill the roof area
                for (x, z) in roof_area.iter() {
                    editor.set_block(roof_block, *x, 5, *z, None, None);
                }
            }
            "fountain" => {
                generate_fountain(editor, element, args, flood_fill_cache);
            }
            "parking" => {
                // Process parking areas
                let mut previous_node: Option<XZPoint> = None;

                let block_type = GRAY_CONCRETE;

                for node in element.nodes() {
                    let pt: XZPoint = node.xz();

                    if let Some(prev) = previous_node {
                        // Create borders for parking area
                        let bresenham_points: Vec<(i32, i32, i32)> =
                            bresenham_line(prev.x, 0, prev.z, pt.x, 0, pt.z);
                        for (bx, _, bz) in bresenham_points {
                            editor.set_block(
                                block_type,
                                bx,
                                0,
                                bz,
                                Some(&[BLACK_CONCRETE, GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA]),
                                None,
                            );
                        }
                    }
                    previous_node = Some(pt);
                }

                // Flood-fill the interior area for parking
                let flood_area: Vec<(i32, i32)> =
                    flood_fill_cache.get_or_compute_element(element, args.timeout.as_ref());

                for (x, z) in flood_area {
                    editor.set_block(
                        block_type,
                        x,
                        0,
                        z,
                        Some(&[
                            BLACK_CONCRETE,
                            GRAY_CONCRETE_POWDER,
                            CYAN_TERRACOTTA,
                            GRAY_CONCRETE,
                        ]),
                        None,
                    );

                    // Enhanced parking space markings
                    if amenity_type == "parking" {
                        // Create defined parking spaces with realistic layout
                        let space_width = 4; // Width of each parking space
                        let space_length = 6; // Length of each parking space
                        let lane_width = 5; // Width of driving lanes

                        // Calculate which "zone" this coordinate falls into
                        let zone_x = x / space_width;
                        let zone_z = z / (space_length + lane_width);
                        let local_x = x % space_width;
                        let local_z = z % (space_length + lane_width);

                        // Create parking space boundaries (only within parking areas, not in driving lanes)
                        if local_z < space_length {
                            // We're in a parking space area, not in the driving lane
                            if local_x == 0 {
                                // Vertical parking space lines (only on the left edge)
                                editor.set_block(
                                    LIGHT_GRAY_CONCRETE,
                                    x,
                                    0,
                                    z,
                                    Some(&[
                                        BLACK_CONCRETE,
                                        GRAY_CONCRETE_POWDER,
                                        CYAN_TERRACOTTA,
                                        GRAY_CONCRETE,
                                    ]),
                                    None,
                                );
                            } else if local_z == 0 {
                                // Horizontal parking space lines (only on the top edge)
                                editor.set_block(
                                    LIGHT_GRAY_CONCRETE,
                                    x,
                                    0,
                                    z,
                                    Some(&[
                                        BLACK_CONCRETE,
                                        GRAY_CONCRETE_POWDER,
                                        CYAN_TERRACOTTA,
                                        GRAY_CONCRETE,
                                    ]),
                                    None,
                                );
                            }
                        } else if local_z == space_length {
                            // Bottom edge of parking spaces (border with driving lane)
                            editor.set_block(
                                LIGHT_GRAY_CONCRETE,
                                x,
                                0,
                                z,
                                Some(&[
                                    BLACK_CONCRETE,
                                    GRAY_CONCRETE_POWDER,
                                    CYAN_TERRACOTTA,
                                    GRAY_CONCRETE,
                                ]),
                                None,
                            );
                        } else if local_z > space_length && local_z < space_length + lane_width {
                            // Driving lane - use darker concrete
                            editor.set_block(BLACK_CONCRETE, x, 0, z, Some(&[GRAY_CONCRETE]), None);
                        }

                        // Add light posts at parking space outline corners
                        if local_x == 0 && local_z == 0 && zone_x % 3 == 0 && zone_z % 2 == 0 {
                            // Light posts at regular intervals on parking space corners
                            editor.set_block(COBBLESTONE_WALL, x, 1, z, None, None);
                            for dy in 2..=4 {
                                editor.set_block(OAK_FENCE, x, dy, z, None, None);
                            }
                            editor.set_block(GLOWSTONE, x, 5, z, None, None);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Generates a 3D fountain that adapts to the polygon shape of the element.
///
/// Layout (inside→out):
///   - Smooth stone interior floor with stone brick wall rim at polygon edge (2 blocks tall)
///   - Water fills the interior at y=1
///   - Central pillar of chiseled stone bricks + sea lantern at base
///   - Water basin (Wasserbecken) on top of the pillar
///
/// For node-based fountains (single point) a compact 3×3 basin is built.
fn generate_fountain(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
) {
    // ── Node fountain (single point) ───────────────────────────────
    let nodes: Vec<_> = element.nodes().collect();
    if nodes.len() < 3 {
        if let Some(node) = nodes.first() {
            let cx = node.x;
            let cz = node.z;
            // 3×3 basin with rim + central pillar + raised basin
            for dx in -1i32..=1 {
                for dz in -1i32..=1 {
                    let is_rim = dx.abs() == 1 || dz.abs() == 1;
                    if is_rim {
                        editor.set_block(STONE_BRICK_WALL, cx + dx, 1, cz + dz, None, None);
                    }
                }
            }
            // Central pillar with small basin on top
            editor.set_block(SEA_LANTERN, cx, 1, cz, None, None);
            editor.set_block(CHISELED_STONE_BRICKS, cx, 2, cz, None, None);
            // Basin at y=3: cardinal walls + water center
            editor.set_block(WATER, cx, 3, cz, None, None);
            editor.set_block(STONE_BRICK_WALL, cx - 1, 3, cz, None, None);
            editor.set_block(STONE_BRICK_WALL, cx + 1, 3, cz, None, None);
            editor.set_block(STONE_BRICK_WALL, cx, 3, cz - 1, None, None);
            editor.set_block(STONE_BRICK_WALL, cx, 3, cz + 1, None, None);
        }
        return;
    }

    // ── Way fountain (polygon) ─────────────────────────────────────
    let floor_area: Vec<(i32, i32)> =
        flood_fill_cache.get_or_compute_element(element, args.timeout.as_ref());

    if floor_area.is_empty() {
        return;
    }

    // Compute centroid
    let (sum_x, sum_z) = floor_area.iter().fold((0i64, 0i64), |(sx, sz), &(x, z)| {
        (sx + x as i64, sz + z as i64)
    });
    let count = floor_area.len() as i64;
    let cx = (sum_x / count) as i32;
    let cz = (sum_z / count) as i32;

    // Compute approximate radius (average distance from centroid)
    let avg_dist: f64 = floor_area
        .iter()
        .map(|&(x, z)| {
            let dx = (x - cx) as f64;
            let dz = (z - cz) as f64;
            (dx * dx + dz * dz).sqrt()
        })
        .sum::<f64>()
        / floor_area.len() as f64;

    // Pillar height scales with fountain size (min 2, max 5)
    let pillar_height = (avg_dist as i32).clamp(2, 5);

    // Collect edge outline via Bresenham
    let mut edge_set: HashSet<(i32, i32)> = HashSet::new();
    let mut prev: Option<(i32, i32)> = None;
    for node in element.nodes() {
        if let Some((px, pz)) = prev {
            for (bx, _, bz) in bresenham_line(px, 0, pz, node.x, 0, node.z) {
                edge_set.insert((bx, bz));
            }
        }
        prev = Some((node.x, node.z));
    }

    // Place rim (stone brick wall, 2 blocks high) along the edge
    for &(ex, ez) in &edge_set {
        editor.set_block(STONE_BRICKS, ex, 0, ez, None, None);
        editor.set_block(STONE_BRICK_WALL, ex, 1, ez, None, None);
    }

    // Fill interior with water at y=1 (and a stone floor at y=0)
    for &(x, z) in &floor_area {
        if !edge_set.contains(&(x, z)) {
            editor.set_block(SMOOTH_STONE, x, 0, z, None, None);
            editor.set_block(WATER, x, 1, z, None, None);
        }
    }

    // Central pillar — find closest interior point to centroid
    let pillar_pos = floor_area
        .iter()
        .filter(|&&(x, z)| !edge_set.contains(&(x, z)))
        .min_by_key(|&&(x, z)| {
            let dx = (x - cx) as i64;
            let dz = (z - cz) as i64;
            dx * dx + dz * dz
        })
        .copied()
        .unwrap_or((cx, cz));

    let (px, pz) = pillar_pos;

    // Build pillar: sea lantern at base, chiseled stone bricks upward
    editor.set_block(SEA_LANTERN, px, 1, pz, None, None);
    for h in 2..=pillar_height {
        editor.set_block(CHISELED_STONE_BRICKS, px, h, pz, None, None);
    }

    // Basin (Wasserbecken) on top: stone brick wall ring with water inside
    let basin_y = pillar_height + 1;
    for dx in -1i32..=1 {
        for dz in -1i32..=1 {
            if dx == 0 && dz == 0 {
                // Centre of basin: water
                editor.set_block(WATER, px, basin_y, pz, None, None);
            } else if dx.abs() + dz.abs() <= 1 {
                // Cardinal neighbours: stone brick wall rim
                editor.set_block(STONE_BRICK_WALL, px + dx, basin_y, pz + dz, None, None);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum RecyclingLootKind {
    GlassBottle,
    Paper,
    GlassBlock,
    GlassPane,
    LeatherArmor,
    EmptyBucket,
    LeatherBoots,
    ScrapMetal,
    GreenWaste,
}

#[derive(Clone, Copy)]
enum LeatherPiece {
    Helmet,
    Chestplate,
    Leggings,
    Boots,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum LootCategory {
    GlassBottle,
    Paper,
    Glass,
    Leather,
    EmptyBucket,
    ScrapMetal,
    GreenWaste,
}

fn recycling_barrel_properties() -> HashMap<String, Value> {
    let mut props = HashMap::new();
    props.insert("facing".to_string(), Value::String("up".to_string()));
    props
}

fn build_recycling_loot_pool(tags: &HashMap<String, String>) -> Vec<RecyclingLootKind> {
    let mut loot_pool: Vec<RecyclingLootKind> = Vec::new();

    if tag_enabled(tags, "recycling:glass_bottles") {
        loot_pool.push(RecyclingLootKind::GlassBottle);
    }
    if tag_enabled(tags, "recycling:paper") {
        loot_pool.push(RecyclingLootKind::Paper);
    }
    if tag_enabled(tags, "recycling:glass") {
        loot_pool.push(RecyclingLootKind::GlassBlock);
        loot_pool.push(RecyclingLootKind::GlassPane);
    }
    if tag_enabled(tags, "recycling:clothes") {
        loot_pool.push(RecyclingLootKind::LeatherArmor);
    }
    if tag_enabled(tags, "recycling:cans") {
        loot_pool.push(RecyclingLootKind::EmptyBucket);
    }
    if tag_enabled(tags, "recycling:shoes") {
        loot_pool.push(RecyclingLootKind::LeatherBoots);
    }
    if tag_enabled(tags, "recycling:scrap_metal") {
        loot_pool.push(RecyclingLootKind::ScrapMetal);
    }
    if tag_enabled(tags, "recycling:green_waste") {
        loot_pool.push(RecyclingLootKind::GreenWaste);
    }

    loot_pool
}

fn build_recycling_items(
    loot_pool: &[RecyclingLootKind],
    rng: &mut impl Rng,
) -> Vec<HashMap<String, Value>> {
    if loot_pool.is_empty() {
        return Vec::new();
    }

    let mut items = Vec::new();
    for slot in 0..27 {
        if rng.random_bool(0.2) {
            let kind = loot_pool[rng.random_range(0..loot_pool.len())];
            if let Some(item) = build_item_for_kind(kind, slot as i8, rng) {
                items.push(item);
            }
        }
    }

    items
}

fn kind_to_category(kind: RecyclingLootKind) -> LootCategory {
    match kind {
        RecyclingLootKind::GlassBottle => LootCategory::GlassBottle,
        RecyclingLootKind::Paper => LootCategory::Paper,
        RecyclingLootKind::GlassBlock | RecyclingLootKind::GlassPane => LootCategory::Glass,
        RecyclingLootKind::LeatherArmor | RecyclingLootKind::LeatherBoots => LootCategory::Leather,
        RecyclingLootKind::EmptyBucket => LootCategory::EmptyBucket,
        RecyclingLootKind::ScrapMetal => LootCategory::ScrapMetal,
        RecyclingLootKind::GreenWaste => LootCategory::GreenWaste,
    }
}

fn single_loot_category(loot_pool: &[RecyclingLootKind]) -> Option<LootCategory> {
    let mut categories: HashSet<LootCategory> = HashSet::new();
    for kind in loot_pool {
        categories.insert(kind_to_category(*kind));
        if categories.len() > 1 {
            return None;
        }
    }
    categories.iter().next().copied()
}

fn build_display_item_for_category(
    category: LootCategory,
    rng: &mut impl Rng,
) -> Option<HashMap<String, Value>> {
    match category {
        LootCategory::GlassBottle => Some(make_display_item("minecraft:glass_bottle", 1)),
        LootCategory::Paper => Some(make_display_item(
            "minecraft:paper",
            rng.random_range(1..=4),
        )),
        LootCategory::Glass => Some(make_display_item("minecraft:glass", 1)),
        LootCategory::Leather => Some(build_leather_display_item(rng)),
        LootCategory::EmptyBucket => Some(make_display_item("minecraft:bucket", 1)),
        LootCategory::ScrapMetal => {
            let metals = [
                "minecraft:copper_ingot",
                "minecraft:iron_ingot",
                "minecraft:gold_ingot",
            ];
            let metal = metals.choose(rng)?;
            Some(make_display_item(metal, rng.random_range(1..=2)))
        }
        LootCategory::GreenWaste => {
            let options = [
                "minecraft:oak_sapling",
                "minecraft:birch_sapling",
                "minecraft:tall_grass",
                "minecraft:sweet_berries",
                "minecraft:wheat_seeds",
            ];
            let choice = options.choose(rng)?;
            Some(make_display_item(choice, rng.random_range(1..=3)))
        }
    }
}

fn place_item_frame_on_random_side(
    editor: &mut WorldEditor,
    x: i32,
    barrel_absolute_y: i32,
    z: i32,
    item: HashMap<String, Value>,
) {
    let mut rng = rand::rng();
    let mut directions = [
        ((0, 0, -1), 2), // North
        ((0, 0, 1), 3),  // South
        ((-1, 0, 0), 4), // West
        ((1, 0, 0), 5),  // East
    ];
    directions.shuffle(&mut rng);

    let (min_x, min_z) = editor.get_min_coords();
    let (max_x, max_z) = editor.get_max_coords();

    let ((dx, _dy, dz), facing) = directions
        .into_iter()
        .find(|((dx, _dy, dz), _)| {
            let target_x = x + dx;
            let target_z = z + dz;
            target_x >= min_x && target_x <= max_x && target_z >= min_z && target_z <= max_z
        })
        .unwrap_or(((0, 0, 1), 3)); // Fallback south if all directions are out of bounds

    let target_x = x + dx;
    let target_y = barrel_absolute_y;
    let target_z = z + dz;

    let ground_y = editor.get_absolute_y(target_x, 0, target_z);

    let mut extra = HashMap::new();
    extra.insert("Facing".to_string(), Value::Byte(facing)); // 2=north, 3=south, 4=west, 5=east
    extra.insert("ItemRotation".to_string(), Value::Byte(0));
    extra.insert("Item".to_string(), Value::Compound(item));
    extra.insert("ItemDropChance".to_string(), Value::Float(1.0));
    extra.insert(
        "block_pos".to_string(),
        Value::List(vec![
            Value::Int(target_x),
            Value::Int(target_y),
            Value::Int(target_z),
        ]),
    );
    extra.insert("TileX".to_string(), Value::Int(target_x));
    extra.insert("TileY".to_string(), Value::Int(target_y));
    extra.insert("TileZ".to_string(), Value::Int(target_z));
    extra.insert("Fixed".to_string(), Value::Byte(1));

    let relative_y = target_y - ground_y;
    editor.add_entity(
        "minecraft:item_frame",
        target_x,
        relative_y,
        target_z,
        Some(extra),
    );
}

fn make_display_item(id: &str, count: i8) -> HashMap<String, Value> {
    let mut item = HashMap::new();
    item.insert("id".to_string(), Value::String(id.to_string()));
    item.insert("Count".to_string(), Value::Byte(count));
    item
}

fn build_leather_display_item(rng: &mut impl Rng) -> HashMap<String, Value> {
    let mut item = make_display_item("minecraft:leather_chestplate", 1);
    let damage = biased_damage(80, rng);

    let mut tag = HashMap::new();
    tag.insert("Damage".to_string(), Value::Int(damage));

    if let Some(color) = maybe_leather_color(rng) {
        let mut display = HashMap::new();
        display.insert("color".to_string(), Value::Int(color));
        tag.insert("display".to_string(), Value::Compound(display));
    }

    item.insert("tag".to_string(), Value::Compound(tag));

    let mut components = HashMap::new();
    components.insert("minecraft:damage".to_string(), Value::Int(damage));
    item.insert("components".to_string(), Value::Compound(components));

    item
}

fn build_item_for_kind(
    kind: RecyclingLootKind,
    slot: i8,
    rng: &mut impl Rng,
) -> Option<HashMap<String, Value>> {
    match kind {
        RecyclingLootKind::GlassBottle => Some(make_basic_item(
            "minecraft:glass_bottle",
            slot,
            rng.random_range(1..=4),
        )),
        RecyclingLootKind::Paper => Some(make_basic_item(
            "minecraft:paper",
            slot,
            rng.random_range(1..=10),
        )),
        RecyclingLootKind::GlassBlock => Some(build_glass_item(false, slot, rng)),
        RecyclingLootKind::GlassPane => Some(build_glass_item(true, slot, rng)),
        RecyclingLootKind::LeatherArmor => {
            Some(build_leather_item(random_leather_piece(rng), slot, rng))
        }
        RecyclingLootKind::EmptyBucket => Some(make_basic_item("minecraft:bucket", slot, 1)),
        RecyclingLootKind::LeatherBoots => Some(build_leather_item(LeatherPiece::Boots, slot, rng)),
        RecyclingLootKind::ScrapMetal => Some(build_scrap_metal_item(slot, rng)),
        RecyclingLootKind::GreenWaste => Some(build_green_waste_item(slot, rng)),
    }
}

fn build_scrap_metal_item(slot: i8, rng: &mut impl Rng) -> HashMap<String, Value> {
    let metals = ["copper_ingot", "iron_ingot", "gold_ingot"];
    let metal = metals.choose(rng).expect("scrap metal list is non-empty");
    let count = rng.random_range(1..=3);
    make_basic_item(&format!("minecraft:{metal}"), slot, count)
}

fn build_green_waste_item(slot: i8, rng: &mut impl Rng) -> HashMap<String, Value> {
    #[allow(clippy::match_same_arms)]
    let (id, count) = match rng.random_range(0..8) {
        0 => ("minecraft:tall_grass", rng.random_range(1..=4)),
        1 => ("minecraft:sweet_berries", rng.random_range(2..=6)),
        2 => ("minecraft:oak_sapling", rng.random_range(1..=2)),
        3 => ("minecraft:birch_sapling", rng.random_range(1..=2)),
        4 => ("minecraft:spruce_sapling", rng.random_range(1..=2)),
        5 => ("minecraft:jungle_sapling", rng.random_range(1..=2)),
        6 => ("minecraft:acacia_sapling", rng.random_range(1..=2)),
        _ => ("minecraft:dark_oak_sapling", rng.random_range(1..=2)),
    };

    // 25% chance to replace with seeds instead
    let id = if rng.random_bool(0.25) {
        match rng.random_range(0..4) {
            0 => "minecraft:wheat_seeds",
            1 => "minecraft:pumpkin_seeds",
            2 => "minecraft:melon_seeds",
            _ => "minecraft:beetroot_seeds",
        }
    } else {
        id
    };

    make_basic_item(id, slot, count)
}

fn build_glass_item(is_pane: bool, slot: i8, rng: &mut impl Rng) -> HashMap<String, Value> {
    const GLASS_COLORS: &[&str] = &[
        "white",
        "orange",
        "magenta",
        "light_blue",
        "yellow",
        "lime",
        "pink",
        "gray",
        "light_gray",
        "cyan",
        "purple",
        "blue",
        "brown",
        "green",
        "red",
        "black",
    ];

    let use_colorless = rng.random_bool(0.7);

    let id = if use_colorless {
        if is_pane {
            "minecraft:glass_pane".to_string()
        } else {
            "minecraft:glass".to_string()
        }
    } else {
        let color = GLASS_COLORS
            .choose(rng)
            .expect("glass color array is non-empty");
        if is_pane {
            format!("minecraft:{color}_stained_glass_pane")
        } else {
            format!("minecraft:{color}_stained_glass")
        }
    };

    let count = if is_pane {
        rng.random_range(4..=16)
    } else {
        rng.random_range(1..=6)
    };

    make_basic_item(&id, slot, count)
}

fn build_leather_item(piece: LeatherPiece, slot: i8, rng: &mut impl Rng) -> HashMap<String, Value> {
    let (id, max_damage) = match piece {
        LeatherPiece::Helmet => ("minecraft:leather_helmet", 55),
        LeatherPiece::Chestplate => ("minecraft:leather_chestplate", 80),
        LeatherPiece::Leggings => ("minecraft:leather_leggings", 75),
        LeatherPiece::Boots => ("minecraft:leather_boots", 65),
    };

    let mut item = make_basic_item(id, slot, 1);
    let damage = biased_damage(max_damage, rng);

    let mut tag = HashMap::new();
    tag.insert("Damage".to_string(), Value::Int(damage));

    if let Some(color) = maybe_leather_color(rng) {
        let mut display = HashMap::new();
        display.insert("color".to_string(), Value::Int(color));
        tag.insert("display".to_string(), Value::Compound(display));
    }

    item.insert("tag".to_string(), Value::Compound(tag));

    let mut components = HashMap::new();
    components.insert("minecraft:damage".to_string(), Value::Int(damage));
    item.insert("components".to_string(), Value::Compound(components));

    item
}

fn biased_damage(max_damage: i32, rng: &mut impl Rng) -> i32 {
    let safe_max = max_damage.max(1);
    let upper = safe_max.saturating_sub(1);
    let lower = (safe_max / 2).min(upper);

    let heavy_wear = rng.random_range(lower..=upper);
    let random_wear = rng.random_range(0..=upper);
    heavy_wear.max(random_wear)
}

fn maybe_leather_color(rng: &mut impl Rng) -> Option<i32> {
    if rng.random_bool(0.3) {
        Some(rng.random_range(0..=0x00FF_FFFF))
    } else {
        None
    }
}

fn random_leather_piece(rng: &mut impl Rng) -> LeatherPiece {
    match rng.random_range(0..4) {
        0 => LeatherPiece::Helmet,
        1 => LeatherPiece::Chestplate,
        2 => LeatherPiece::Leggings,
        _ => LeatherPiece::Boots,
    }
}

fn make_basic_item(id: &str, slot: i8, count: i8) -> HashMap<String, Value> {
    let mut item = HashMap::new();
    item.insert("id".to_string(), Value::String(id.to_string()));
    item.insert("Slot".to_string(), Value::Byte(slot));
    item.insert("Count".to_string(), Value::Byte(count));
    item
}

fn tag_enabled(tags: &HashMap<String, String>, key: &str) -> bool {
    tags.get(key).is_some_and(|value| value == "yes")
}
