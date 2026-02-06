use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::colors::color_text_to_rgb_tuple;
use crate::coordinate_system::cartesian::XZPoint;
use crate::deterministic_rng::element_rng;
use crate::element_processing::subprocessor::buildings_interior::generate_building_interior;
use crate::floodfill_cache::FloodFillCache;
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::Rng;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Enum representing different roof types
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RoofType {
    Gabled,    // Two sloping sides meeting at a ridge
    Hipped,    // All sides slope downwards to walls (including Half-hipped, Gambrel, Mansard variations)
    Skillion,  // Single sloping surface
    Pyramidal, // All sides come to a point at the top
    Dome,      // Rounded, hemispherical structure
    Flat,      // Default flat roof
}

// ============================================================================
// Building Style System
// ============================================================================

/// Accent block options for building decoration
const ACCENT_BLOCK_OPTIONS: [Block; 6] = [
    POLISHED_ANDESITE,
    SMOOTH_STONE,
    STONE_BRICKS,
    MUD_BRICKS,
    ANDESITE,
    CHISELED_STONE_BRICKS,
];

/// Building category determines which preset rules to apply.
/// This is derived from OSM tags and can influence style choices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BuildingCategory {
    Residential,    // Houses, apartments, detached homes
    Commercial,     // Shops, offices, retail
    Industrial,     // Warehouses, factories
    Institutional,  // Schools, hospitals, government
    Skyscraper,     // Tall buildings (>7 floors or >28m)
    Historic,       // Castles, historic buildings
    Default,        // Unknown or generic buildings
}

impl BuildingCategory {
    /// Determines the building category from OSM tags and calculated properties
    fn from_element(element: &ProcessedWay, is_tall_building: bool, building_height: i32) -> Self {
        // Check for skyscraper first (based on height)
        if is_tall_building || building_height > 28 {
            return BuildingCategory::Skyscraper;
        }

        // Check for historic buildings
        if element.tags.get("historic").is_some() {
            return BuildingCategory::Historic;
        }

        // Get building type tag
        let building_type = element
            .tags
            .get("building")
            .or_else(|| element.tags.get("building:part"))
            .map(|s| s.as_str())
            .unwrap_or("yes");

        match building_type {
            // Residential types
            "residential" | "house" | "apartments" | "detached" | "semidetached_house"
            | "terrace" | "farm" | "cabin" | "bungalow" | "villa" | "dormitory" => {
                BuildingCategory::Residential
            }
            // Commercial types
            "commercial" | "retail" | "office" | "supermarket" | "kiosk" | "shop" => {
                BuildingCategory::Commercial
            }
            // Industrial types
            "industrial" | "warehouse" | "factory" | "manufacture" | "storage_tank" => {
                BuildingCategory::Industrial
            }
            // Institutional types
            "hospital" | "school" | "university" | "college" | "public" | "government"
            | "civic" | "church" | "cathedral" | "chapel" | "mosque" | "synagogue" | "temple" => {
                BuildingCategory::Institutional
            }
            // Historic (also check building type)
            "castle" | "ruins" | "fort" | "bunker" => BuildingCategory::Historic,
            // Default for unknown types
            _ => BuildingCategory::Default,
        }
    }
}

/// A partial style specification where `None` means "pick randomly".
/// Use this to create building presets that enforce certain properties
/// while allowing variation in others.
#[derive(Debug, Clone, Default)]
pub struct BuildingStylePreset {
    // Block palette (None = randomly chosen)
    pub wall_block: Option<Block>,
    pub floor_block: Option<Block>,
    pub window_block: Option<Block>,
    pub accent_block: Option<Block>,

    // Window style
    pub use_vertical_windows: Option<bool>,

    // Accent features
    pub use_accent_roof_line: Option<bool>,
    pub use_accent_lines: Option<bool>,
    pub use_vertical_accent: Option<bool>,

    // Roof
    pub roof_type: Option<RoofType>,
    pub has_chimney: Option<bool>,
    pub generate_roof: Option<bool>,
}

impl BuildingStylePreset {
    /// Creates an empty preset (all random)
    pub fn empty() -> Self {
        Self::default()
    }

    /// Preset for residential buildings (houses, apartments)
    pub fn residential() -> Self {
        Self {
            use_vertical_windows: Some(false),
            use_accent_lines: Some(false),  // Residential buildings rarely have accent lines
            ..Default::default()
        }
    }

    /// Preset for skyscrapers and tall buildings
    pub fn skyscraper() -> Self {
        Self {
            use_vertical_windows: Some(true),  // Always vertical windows
            roof_type: Some(RoofType::Flat),   // Always flat roof
            has_chimney: Some(false),          // No chimneys on skyscrapers
            use_accent_roof_line: Some(true),  // Usually have accent roof line
            ..Default::default()
        }
    }

    /// Preset for industrial buildings (warehouses, factories)
    pub fn industrial() -> Self {
        Self {
            roof_type: Some(RoofType::Flat),
            has_chimney: Some(false),
            use_accent_lines: Some(false),
            use_vertical_accent: Some(false),
            ..Default::default()
        }
    }

    /// Preset for institutional buildings (hospitals, schools)
    pub fn institutional() -> Self {
        Self {
            use_vertical_windows: Some(false),
            use_accent_roof_line: Some(true),
            ..Default::default()
        }
    }

    /// Preset for historic buildings (castles, etc.)
    pub fn historic() -> Self {
        Self {
            roof_type: Some(RoofType::Flat),
            has_chimney: Some(false),
            use_accent_lines: Some(false),
            use_vertical_accent: Some(false),
            use_accent_roof_line: Some(false),
            ..Default::default()
        }
    }

    /// Gets the appropriate preset for a building category
    pub fn for_category(category: BuildingCategory) -> Self {
        match category {
            BuildingCategory::Residential => Self::residential(),
            BuildingCategory::Skyscraper => Self::skyscraper(),
            BuildingCategory::Industrial => Self::industrial(),
            BuildingCategory::Institutional => Self::institutional(),
            BuildingCategory::Historic => Self::historic(),
            BuildingCategory::Commercial | BuildingCategory::Default => Self::empty(),
        }
    }
}

/// Fully resolved building style with all parameters determined.
/// Created by resolving a `BuildingStylePreset` with deterministic RNG.
#[derive(Debug, Clone)]
pub struct BuildingStyle {
    // Block palette
    pub wall_block: Block,
    pub floor_block: Block,
    pub window_block: Block,
    pub accent_block: Block,

    // Window style
    pub use_vertical_windows: bool,

    // Accent features
    pub use_accent_roof_line: bool,
    pub use_accent_lines: bool,
    pub use_vertical_accent: bool,

    // Roof
    pub roof_type: RoofType,
    pub has_chimney: bool,
    pub generate_roof: bool,
}

impl BuildingStyle {
    /// Resolves a preset into a fully determined style using deterministic RNG.
    /// Parameters not specified in the preset are randomly chosen.
    ///
    /// # Arguments
    /// * `preset` - The style preset (partial specification)
    /// * `element` - The OSM element (used for tag-based decisions)
    /// * `building_type` - The building type string from tags
    /// * `has_multiple_floors` - Whether building has more than 6 height units
    /// * `footprint_size` - The building's floor area in blocks
    /// * `rng` - Deterministic RNG seeded by element ID
    #[allow(clippy::too_many_arguments)]
    pub fn resolve(
        preset: &BuildingStylePreset,
        element: &ProcessedWay,
        building_type: &str,
        has_multiple_floors: bool,
        footprint_size: usize,
        rng: &mut impl Rng,
    ) -> Self {
        // === Block Palette ===

        // Wall block: from tags or preset
        let wall_block = preset
            .wall_block
            .unwrap_or_else(|| determine_wall_block(element));

        // Floor block: from preset or random
        let floor_block = preset
            .floor_block
            .unwrap_or_else(|| get_floor_block_with_rng(rng));

        // Window block: from preset or random based on building type
        let window_block = preset
            .window_block
            .unwrap_or_else(|| get_window_block_for_building_type_with_rng(building_type, rng));

        // Accent block: from preset or random
        let accent_block = preset.accent_block.unwrap_or_else(|| {
            ACCENT_BLOCK_OPTIONS[rng.gen_range(0..ACCENT_BLOCK_OPTIONS.len())]
        });

        // === Window Style ===

        let use_vertical_windows = preset.use_vertical_windows.unwrap_or_else(|| rng.gen_bool(0.7));

        // === Accent Features ===

        let use_accent_roof_line = preset
            .use_accent_roof_line
            .unwrap_or_else(|| rng.gen_bool(0.25));

        // Accent lines only for multi-floor buildings
        let use_accent_lines = preset.use_accent_lines.unwrap_or_else(|| {
            has_multiple_floors && rng.gen_bool(0.2)
        });

        // Vertical accent: only if no accent lines and multi-floor
        let use_vertical_accent = preset.use_vertical_accent.unwrap_or_else(|| {
            has_multiple_floors && !use_accent_lines && rng.gen_bool(0.1)
        });

        // === Roof ===

        // Determine roof type from preset, tags, or auto-generation
        let (roof_type, generate_roof) = if let Some(rt) = preset.roof_type {
            // Preset forces a specific roof type
            let should_generate = preset.generate_roof.unwrap_or(rt != RoofType::Flat);
            (rt, should_generate)
        } else if let Some(roof_shape) = element.tags.get("roof:shape") {
            // Use OSM tag
            (parse_roof_type(roof_shape), true)
        } else if qualifies_for_auto_gabled_roof(building_type) {
            // Auto-generate gabled roof for residential buildings
            const MAX_FOOTPRINT_FOR_GABLED: usize = 800;
            if footprint_size <= MAX_FOOTPRINT_FOR_GABLED && rng.gen_bool(0.9) {
                (RoofType::Gabled, true)
            } else {
                (RoofType::Flat, false)
            }
        } else {
            (RoofType::Flat, false)
        };

        // Chimney: only for residential with gabled/hipped roofs
        let has_chimney = preset.has_chimney.unwrap_or_else(|| {
            let is_residential = matches!(
                building_type,
                "house" | "residential" | "detached" | "semidetached_house"
                    | "terrace" | "farm" | "cabin" | "bungalow" | "villa" | "yes"
            );
            let suitable_roof = matches!(roof_type, RoofType::Gabled | RoofType::Hipped);
            let suitable_size = footprint_size >= 30 && footprint_size <= 400;

            is_residential && suitable_roof && suitable_size && rng.gen_bool(0.55)
        });

        Self {
            wall_block,
            floor_block,
            window_block,
            accent_block,
            use_vertical_windows,
            use_accent_roof_line,
            use_accent_lines,
            use_vertical_accent,
            roof_type,
            has_chimney,
            generate_roof,
        }
    }
}

/// Building configuration derived from OSM tags and args
struct BuildingConfig {
    min_level: i32,
    building_height: i32,
    is_tall_building: bool,
    start_y_offset: i32,
    abs_terrain_offset: i32,
    wall_block: Block,
    floor_block: Block,
    window_block: Block,
    accent_block: Block,
    use_vertical_windows: bool,
    use_accent_roof_line: bool,
    use_accent_lines: bool,
    use_vertical_accent: bool,
    is_abandoned_building: bool,
}

/// Building bounds calculated from nodes
struct BuildingBounds {
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
}

impl BuildingBounds {
    fn from_nodes(nodes: &[crate::osm_parser::ProcessedNode]) -> Self {
        Self {
            min_x: nodes.iter().map(|n| n.x).min().unwrap_or(0),
            max_x: nodes.iter().map(|n| n.x).max().unwrap_or(0),
            min_z: nodes.iter().map(|n| n.z).min().unwrap_or(0),
            max_z: nodes.iter().map(|n| n.z).max().unwrap_or(0),
        }
    }

    fn width(&self) -> i32 {
        self.max_x - self.min_x
    }

    fn length(&self) -> i32 {
        self.max_z - self.min_z
    }
}

// ============================================================================
// Helper Functions for Building Configuration
// ============================================================================

/// Checks if a building part should be skipped (underground parts)
#[inline]
fn should_skip_underground_building_part(element: &ProcessedWay) -> bool {
    if !element.tags.contains_key("building:part") {
        return false;
    }
    if let Some(layer) = element.tags.get("layer") {
        if layer.parse::<i32>().unwrap_or(0) < 0 {
            return true;
        }
    }
    if let Some(level) = element.tags.get("level") {
        if level.parse::<i32>().unwrap_or(0) < 0 {
            return true;
        }
    }
    false
}

/// Calculates the starting Y offset based on terrain and min_level
fn calculate_start_y_offset(
    editor: &WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    min_level_offset: i32,
) -> i32 {
    if args.terrain {
        let building_points: Vec<XZPoint> = element
            .nodes
            .iter()
            .map(|n| {
                XZPoint::new(
                    n.x - editor.get_min_coords().0,
                    n.z - editor.get_min_coords().1,
                )
            })
            .collect();

        let mut max_ground_level = args.ground_level;
        for point in &building_points {
            if let Some(ground) = editor.get_ground() {
                let level = ground.level(*point);
                max_ground_level = max_ground_level.max(level);
            }
        }
        max_ground_level + min_level_offset
    } else {
        min_level_offset
    }
}

/// Determines the wall block based on building tags
fn determine_wall_block(element: &ProcessedWay) -> Block {
    if element.tags.get("historic") == Some(&"castle".to_string()) {
        get_castle_wall_block()
    } else {
        element
            .tags
            .get("building:colour")
            .and_then(|building_colour: &String| {
                color_text_to_rgb_tuple(building_colour)
                    .map(|rgb: (u8, u8, u8)| get_building_wall_block_for_color(rgb))
            })
            .unwrap_or_else(get_fallback_building_block)
    }
}

/// Determines building height from OSM tags
fn calculate_building_height(
    element: &ProcessedWay,
    min_level: i32,
    scale_factor: f64,
    relation_levels: Option<i32>,
) -> (i32, bool) {
    let default_height = ((6.0 * scale_factor) as i32).max(3);
    let mut building_height = default_height;
    let mut is_tall_building = false;

    // From building:levels tag
    if let Some(levels_str) = element.tags.get("building:levels") {
        if let Ok(levels) = levels_str.parse::<i32>() {
            let lev = levels - min_level;
            if lev >= 1 {
                building_height = multiply_scale(lev * 4 + 2, scale_factor).max(3);
                if levels > 7 {
                    is_tall_building = true;
                }
            }
        }
    }

    // From height tag (overrides levels)
    if let Some(height_str) = element.tags.get("height") {
        if let Ok(height) = height_str.trim_end_matches("m").trim().parse::<f64>() {
            building_height = (height * scale_factor) as i32;
            building_height = building_height.max(3);
            if height > 28.0 {
                is_tall_building = true;
            }
        }
    }

    // From relation levels (highest priority)
    if let Some(levels) = relation_levels {
        building_height = multiply_scale(levels * 4 + 2, scale_factor).max(3);
        if levels > 7 {
            is_tall_building = true;
        }
    }

    (building_height, is_tall_building)
}

/// Adjusts building height for specific building types
fn adjust_height_for_building_type(
    building_type: &str,
    building_height: i32,
    scale_factor: f64,
) -> i32 {
    let default_height = ((6.0 * scale_factor) as i32).max(3);
    match building_type {
        "garage" | "shed" => ((2.0 * scale_factor) as i32).max(3),
        "apartments" if building_height == default_height => ((15.0 * scale_factor) as i32).max(3),
        "hospital" if building_height == default_height => ((23.0 * scale_factor) as i32).max(3),
        _ => building_height,
    }
}

// ============================================================================
// Special Building Type Generators
// ============================================================================

/// Generates a shelter structure with fence posts and roof
fn generate_shelter(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    cached_floor_area: &[(i32, i32)],
    scale_factor: f64,
) {
    let roof_block = STONE_BRICK_SLAB;

    for node in &element.nodes {
        let x = node.x;
        let z = node.z;
        for shelter_y in 1..=multiply_scale(4, scale_factor) {
            editor.set_block(OAK_FENCE, x, shelter_y, z, None, None);
        }
        editor.set_block(roof_block, x, 5, z, None, None);
    }

    for &(x, z) in cached_floor_area {
        editor.set_block(roof_block, x, 5, z, None, None);
    }
}

/// Generates a bicycle parking shed structure
fn generate_bicycle_parking_shed(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    cached_floor_area: &[(i32, i32)],
) {
    let ground_block = OAK_PLANKS;
    let roof_block = STONE_BLOCK_SLAB;

    // Fill the floor area
    for &(x, z) in cached_floor_area {
        editor.set_block(ground_block, x, 0, z, None, None);
    }

    // Place fences and roof slabs at each corner node
    for node in &element.nodes {
        let x = node.x;
        let z = node.z;
        for dy in 1..=4 {
            editor.set_block(OAK_FENCE, x, dy, z, None, None);
        }
        editor.set_block(roof_block, x, 5, z, None, None);
    }

    // Flood fill the roof area
    for &(x, z) in cached_floor_area {
        editor.set_block(roof_block, x, 5, z, None, None);
    }
}

/// Generates a multi-storey parking building structure
fn generate_parking_building(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    cached_floor_area: &[(i32, i32)],
    building_height: i32,
) {
    let building_height = building_height.max(16);

    for level in 0..=(building_height / 4) {
        let current_level_y = level * 4;

        // Build walls
        for node in &element.nodes {
            let x = node.x;
            let z = node.z;
            for y in (current_level_y + 1)..=(current_level_y + 4) {
                editor.set_block(STONE_BRICKS, x, y, z, None, None);
            }
        }

        // Fill the floor area for each level
        for &(x, z) in cached_floor_area {
            let floor_block = if level == 0 { SMOOTH_STONE } else { COBBLESTONE };
            editor.set_block(floor_block, x, current_level_y, z, None, None);
        }
    }

    // Outline for each level
    for level in 0..=(building_height / 4) {
        let current_level_y = level * 4;
        let mut prev_outline = None;

        for node in &element.nodes {
            let x = node.x;
            let z = node.z;

            if let Some((prev_x, prev_z)) = prev_outline {
                let outline_points =
                    bresenham_line(prev_x, current_level_y, prev_z, x, current_level_y, z);

                for (bx, _, bz) in outline_points {
                    editor.set_block(
                        SMOOTH_STONE,
                        bx,
                        current_level_y,
                        bz,
                        Some(&[COBBLESTONE, COBBLESTONE_WALL]),
                        None,
                    );
                    editor.set_block(STONE_BRICK_SLAB, bx, current_level_y + 2, bz, None, None);
                    if bx % 2 == 0 {
                        editor.set_block(COBBLESTONE_WALL, bx, current_level_y + 1, bz, None, None);
                    }
                }
            }
            prev_outline = Some((x, z));
        }
    }
}

/// Generates a roof-only structure (covered walkway, etc.)
fn generate_roof_only_structure(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    cached_floor_area: &[(i32, i32)],
) {
    let roof_height: i32 = 5;
    let mut previous_node: Option<(i32, i32)> = None;

    for node in &element.nodes {
        let x = node.x;
        let z = node.z;

        if let Some(prev) = previous_node {
            let bresenham_points = bresenham_line(prev.0, roof_height, prev.1, x, roof_height, z);
            for (bx, _, bz) in bresenham_points {
                editor.set_block(STONE_BRICK_SLAB, bx, roof_height, bz, None, None);
            }
        }

        for y in 1..=(roof_height - 1) {
            editor.set_block(COBBLESTONE_WALL, x, y, z, None, None);
        }

        previous_node = Some((x, z));
    }

    for &(x, z) in cached_floor_area {
        editor.set_block(STONE_BRICK_SLAB, x, roof_height, z, None, None);
    }
}

// ============================================================================
// Building Component Generators
// ============================================================================

/// Generates the walls of a building including foundations, windows, and accent blocks
#[allow(clippy::too_many_arguments)]
fn generate_building_walls(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    args: &Args,
) -> (Vec<(i32, i32)>, (i32, i32, i32)) {
    let mut previous_node: Option<(i32, i32)> = None;
    let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
    let mut current_building: Vec<(i32, i32)> = Vec::new();

    for node in &element.nodes {
        let x = node.x;
        let z = node.z;

        if let Some(prev) = previous_node {
            let bresenham_points =
                bresenham_line(prev.0, config.start_y_offset, prev.1, x, config.start_y_offset, z);

            for (bx, _, bz) in bresenham_points {
                // Create foundation pillars when using terrain
                if args.terrain && config.min_level == 0 {
                    let local_ground_level = if let Some(ground) = editor.get_ground() {
                        ground.level(XZPoint::new(
                            bx - editor.get_min_coords().0,
                            bz - editor.get_min_coords().1,
                        ))
                    } else {
                        args.ground_level
                    };

                    for y in local_ground_level..config.start_y_offset + 1 {
                        editor.set_block_absolute(
                            config.wall_block,
                            bx,
                            y + config.abs_terrain_offset,
                            bz,
                            None,
                            None,
                        );
                    }
                }

                // Generate wall blocks with windows
                for h in (config.start_y_offset + 1)..=(config.start_y_offset + config.building_height)
                {
                    let block = determine_wall_block_at_position(
                        bx,
                        h,
                        bz,
                        config,
                    );
                    editor.set_block_absolute(
                        block,
                        bx,
                        h + config.abs_terrain_offset,
                        bz,
                        None,
                        None,
                    );
                }

                // Add roof line
                let roof_line_block = if config.use_accent_roof_line {
                    config.accent_block
                } else {
                    config.wall_block
                };
                editor.set_block_absolute(
                    roof_line_block,
                    bx,
                    config.start_y_offset + config.building_height + config.abs_terrain_offset + 1,
                    bz,
                    None,
                    None,
                );

                current_building.push((bx, bz));
                corner_addup = (corner_addup.0 + bx, corner_addup.1 + bz, corner_addup.2 + 1);
            }
        }

        previous_node = Some((x, z));
    }

    (current_building, corner_addup)
}

/// Determines which block to place at a specific wall position (wall, window, or accent)
#[inline]
fn determine_wall_block_at_position(
    bx: i32,
    h: i32,
    bz: i32,
    config: &BuildingConfig,
) -> Block {
    let above_floor = h > config.start_y_offset + 1;

    if config.is_tall_building && config.use_vertical_windows {
        // Tall building pattern - narrower windows with continuous vertical strips
        if above_floor && (bx + bz) % 3 == 0 {
            config.window_block
        } else {
            config.wall_block
        }
    } else {
        // Regular building pattern
        let is_window_position = above_floor && h % 4 != 0 && (bx + bz) % 6 < 3;

        if is_window_position {
            config.window_block
        } else {
            let use_accent_line = config.use_accent_lines && above_floor && h % 4 == 0;
            let use_vertical_accent_here =
                config.use_vertical_accent && above_floor && h % 4 == 0 && (bx + bz) % 6 < 3;

            if use_accent_line || use_vertical_accent_here {
                config.accent_block
            } else {
                config.wall_block
            }
        }
    }
}

/// Generates floors and ceilings for the building interior
#[allow(clippy::too_many_arguments)]
fn generate_floors_and_ceilings(
    editor: &mut WorldEditor,
    cached_floor_area: &[(i32, i32)],
    config: &BuildingConfig,
    element: &ProcessedWay,
    args: &Args,
) -> HashSet<(i32, i32)> {
    let mut processed_points: HashSet<(i32, i32)> = HashSet::new();
    let ceiling_light_block = if config.is_abandoned_building {
        COBWEB
    } else {
        GLOWSTONE
    };

    for &(x, z) in cached_floor_area {
        if !processed_points.insert((x, z)) {
            continue;
        }

        // Set ground floor
        editor.set_block_absolute(
            config.floor_block,
            x,
            config.start_y_offset + config.abs_terrain_offset,
            z,
            None,
            None,
        );

        // Set intermediate ceilings with light fixtures
        if config.building_height > 4 {
            for h in (config.start_y_offset + 2 + 4..config.start_y_offset + config.building_height)
                .step_by(4)
            {
                let block = if x % 5 == 0 && z % 5 == 0 {
                    ceiling_light_block
                } else {
                    config.floor_block
                };
                editor.set_block_absolute(
                    block,
                    x,
                    h + config.abs_terrain_offset,
                    z,
                    None,
                    None,
                );
            }
        } else if x % 5 == 0 && z % 5 == 0 {
            // Single floor building with ceiling light
            editor.set_block_absolute(
                ceiling_light_block,
                x,
                config.start_y_offset + config.building_height + config.abs_terrain_offset,
                z,
                None,
                None,
            );
        }

        // Set top ceiling (only if flat roof or no roof generation)
        let has_flat_roof = !args.roof
            || !element.tags.contains_key("roof:shape")
            || element.tags.get("roof:shape").unwrap() == "flat";

        if has_flat_roof {
            editor.set_block_absolute(
                config.floor_block,
                x,
                config.start_y_offset + config.building_height + config.abs_terrain_offset + 1,
                z,
                None,
                None,
            );
        }
    }

    processed_points
}

/// Calculates floor levels for multi-story buildings
fn calculate_floor_levels(start_y_offset: i32, building_height: i32) -> Vec<i32> {
    let mut floor_levels = vec![start_y_offset];

    if building_height > 6 {
        let num_upper_floors = (building_height / 4).max(1);
        for floor in 1..num_upper_floors {
            floor_levels.push(start_y_offset + 2 + (floor * 4));
        }
    }

    floor_levels
}

/// Calculates roof peak height for chimney placement
fn calculate_roof_peak_height(bounds: &BuildingBounds, start_y_offset: i32, building_height: i32) -> i32 {
    let building_size = bounds.width().max(bounds.length());
    let base_height = start_y_offset + building_height;
    let roof_height_boost = (3.0 + (building_size as f64 * 0.15).ln().max(1.0)) as i32;
    base_height + roof_height_boost
}

/// Parses roof:shape tag into RoofType enum
fn parse_roof_type(roof_shape: &str) -> RoofType {
    match roof_shape {
        "gabled" => RoofType::Gabled,
        "hipped" | "half-hipped" | "gambrel" | "mansard" | "round" => RoofType::Hipped,
        "skillion" => RoofType::Skillion,
        "pyramidal" => RoofType::Pyramidal,
        "dome" | "onion" | "cone" => RoofType::Dome,
        _ => RoofType::Flat,
    }
}

/// Checks if building type qualifies for automatic gabled roof
fn qualifies_for_auto_gabled_roof(building_type: &str) -> bool {
    matches!(
        building_type,
        "apartments" | "residential" | "house" | "yes"
    )
}

// ============================================================================
// Main Building Generation Function
// ============================================================================

#[inline]
pub fn generate_buildings(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    relation_levels: Option<i32>,
    flood_fill_cache: &FloodFillCache,
) {
    // Early return for underground building parts
    if should_skip_underground_building_part(element) {
        return;
    }

    // Parse min_level from tags
    let min_level = element
        .tags
        .get("building:min_level")
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);

    let scale_factor = args.scale;
    let abs_terrain_offset = if !args.terrain { args.ground_level } else { 0 };
    let min_level_offset = multiply_scale(min_level * 4, scale_factor);

    // Get cached floor area
    let cached_floor_area: Vec<(i32, i32)> =
        flood_fill_cache.get_or_compute(element, args.timeout.as_ref());
    let cached_footprint_size = cached_floor_area.len();

    // Calculate start Y offset
    let start_y_offset = calculate_start_y_offset(editor, element, args, min_level_offset);

    // Calculate building bounds
    let bounds = BuildingBounds::from_nodes(&element.nodes);

    // Get building type
    let building_type = element
        .tags
        .get("building")
        .or_else(|| element.tags.get("building:part"))
        .map(|s| s.as_str())
        .unwrap_or("yes");

    // Handle shelter amenity
    if element.tags.get("amenity") == Some(&"shelter".to_string()) {
        generate_shelter(editor, element, &cached_floor_area, scale_factor);
        return;
    }

    // Handle special building types with early returns
    if let Some(btype) = element.tags.get("building") {
        match btype.as_str() {
            "shed" if element.tags.contains_key("bicycle_parking") => {
                generate_bicycle_parking_shed(editor, element, &cached_floor_area);
                return;
            }
            "parking" => {
                let (height, _) = calculate_building_height(element, min_level, scale_factor, relation_levels);
                generate_parking_building(editor, element, &cached_floor_area, height);
                return;
            }
            "roof" => {
                generate_roof_only_structure(editor, element, &cached_floor_area);
                return;
            }
            "bridge" => {
                generate_bridge(editor, element, flood_fill_cache, args.timeout.as_ref());
                return;
            }
            _ => {}
        }

        // Also check for multi-storey parking
        if element.tags.get("parking").is_some_and(|p| p == "multi-storey") {
            let (height, _) = calculate_building_height(element, min_level, scale_factor, relation_levels);
            generate_parking_building(editor, element, &cached_floor_area, height);
            return;
        }
    }

    // Calculate building height with type-specific adjustments
    let (mut building_height, is_tall_building) =
        calculate_building_height(element, min_level, scale_factor, relation_levels);
    building_height = adjust_height_for_building_type(building_type, building_height, scale_factor);

    // Determine building category and get appropriate style preset
    let category = BuildingCategory::from_element(element, is_tall_building, building_height);
    let preset = BuildingStylePreset::for_category(category);

    // Resolve style with deterministic RNG
    let mut rng = element_rng(element.id);
    let has_multiple_floors = building_height > 6;
    let style = BuildingStyle::resolve(
        &preset,
        element,
        building_type,
        has_multiple_floors,
        cached_footprint_size,
        &mut rng,
    );

    // Detect abandoned buildings
    let is_abandoned_building = element
        .tags
        .get("abandoned")
        .is_some_and(|value| value == "yes")
        || element.tags.contains_key("abandoned:building");

    // Create config struct for cleaner function calls
    let config = BuildingConfig {
        min_level,
        building_height,
        is_tall_building,
        start_y_offset,
        abs_terrain_offset,
        wall_block: style.wall_block,
        floor_block: style.floor_block,
        window_block: style.window_block,
        accent_block: style.accent_block,
        use_vertical_windows: style.use_vertical_windows,
        use_accent_roof_line: style.use_accent_roof_line,
        use_accent_lines: style.use_accent_lines,
        use_vertical_accent: style.use_vertical_accent,
        is_abandoned_building,
    };

    // Generate walls
    let (wall_outline, corner_addup) = generate_building_walls(editor, element, &config, args);

    // Create roof area = floor area + wall outline (so roof covers the walls too)
    let roof_area: Vec<(i32, i32)> = {
        let mut area: HashSet<(i32, i32)> = cached_floor_area.iter().copied().collect();
        area.extend(wall_outline.iter().copied());
        area.into_iter().collect()
    };

    // Generate floors and ceilings
    if corner_addup != (0, 0, 0) {
        generate_floors_and_ceilings(editor, &cached_floor_area, &config, element, args);

        // Generate interior features
        if args.interior {
            let skip_interior = matches!(
                building_type,
                "garage" | "shed" | "parking" | "roof" | "bridge"
            );

            if !skip_interior && cached_floor_area.len() > 100 {
                let floor_levels = calculate_floor_levels(start_y_offset, building_height);
                generate_building_interior(
                    editor,
                    &cached_floor_area,
                    bounds.min_x,
                    bounds.min_z,
                    bounds.max_x,
                    bounds.max_z,
                    start_y_offset,
                    building_height,
                    style.wall_block,
                    &floor_levels,
                    args,
                    element,
                    abs_terrain_offset,
                    is_abandoned_building,
                );
            }
        }
    }

    // Process roof generation using style decisions
    if args.roof && style.generate_roof {
        generate_building_roof(
            editor,
            element,
            &config,
            &style,
            &bounds,
            &roof_area,
        );
    }
}

/// Handles roof generation including chimney placement
fn generate_building_roof(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    style: &BuildingStyle,
    bounds: &BuildingBounds,
    cached_floor_area: &[(i32, i32)],
) {
    // Generate the roof using the pre-determined roof type from style
    generate_roof(
        editor,
        element,
        config.start_y_offset,
        config.building_height,
        config.floor_block,
        config.wall_block,
        config.accent_block,
        style.roof_type,
        cached_floor_area,
        config.abs_terrain_offset,
    );

    // Add chimney if style says so
    if style.has_chimney {
        let roof_peak_height = calculate_roof_peak_height(bounds, config.start_y_offset, config.building_height);
        generate_chimney(
            editor,
            cached_floor_area,
            bounds.min_x,
            bounds.max_x,
            bounds.min_z,
            bounds.max_z,
            roof_peak_height,
            config.abs_terrain_offset,
            element.id,
        );
    }
}

fn multiply_scale(value: i32, scale_factor: f64) -> i32 {
    // Use bit operations for faster multiplication when possible
    if scale_factor == 1.0 {
        value
    } else if scale_factor == 2.0 {
        value << 1
    } else if scale_factor == 4.0 {
        value << 2
    } else {
        let result = (value as f64) * scale_factor;
        result.floor() as i32
    }
}

/// Generate a chimney on a building roof
///
/// Creates a small brick chimney (1x1) typically found on residential buildings.
/// Chimneys are placed within the actual building footprint near a corner.
fn generate_chimney(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    roof_peak_height: i32,
    abs_terrain_offset: i32,
    element_id: u64,
) {
    if floor_area.is_empty() {
        return;
    }

    // Use deterministic RNG based on element ID for consistent placement
    let mut rng = element_rng(element_id);

    // Find a position within the actual floor area near a corner
    // Calculate center point
    let center_x = (min_x + max_x) / 2;
    let center_z = (min_z + max_z) / 2;

    // Choose which quadrant to place the chimney (deterministically)
    let quadrant = rng.gen_range(0..4);

    // Filter floor area points to the chosen quadrant and find one that's
    // offset from the edge (so it's actually on the roof, not at the wall)
    let candidate_points: Vec<(i32, i32)> = floor_area
        .iter()
        .filter(|(x, z)| {
            let in_quadrant = match quadrant {
                0 => *x < center_x && *z < center_z, // NW
                1 => *x >= center_x && *z < center_z, // NE
                2 => *x < center_x && *z >= center_z, // SW
                _ => *x >= center_x && *z >= center_z, // SE
            };
            // Must be at least 1 block from building edge
            let away_from_edge = *x > min_x && *x < max_x && *z > min_z && *z < max_z;
            in_quadrant && away_from_edge
        })
        .copied()
        .collect();

    // If no good candidates in the quadrant, try any interior point
    let final_candidates = if candidate_points.is_empty() {
        floor_area
            .iter()
            .filter(|(x, z)| *x > min_x + 1 && *x < max_x - 1 && *z > min_z + 1 && *z < max_z - 1)
            .copied()
            .collect::<Vec<_>>()
    } else {
        candidate_points
    };

    if final_candidates.is_empty() {
        return;
    }

    // Pick a point from candidates
    let (chimney_x, chimney_z) = final_candidates[rng.gen_range(0..final_candidates.len())];

    // Chimney starts 2 blocks below roof peak to replace roof blocks properly
    // Height is exactly 3 brick blocks with a slab cap on top
    let chimney_base = roof_peak_height - 2;
    let chimney_height = 3;

    // Blocks that the chimney is allowed to replace (roof materials and stairs)
    // We pass None for whitelist and use a blacklist that excludes nothing,
    // which means we ALWAYS overwrite. But set_block_absolute with None, None
    // won't overwrite existing blocks. So we need to specify that ANY existing
    // block should be replaced.
    // Since set_block_absolute only overwrites when whitelist matches or blacklist doesn't,
    // we use an empty blacklist to mean "blacklist nothing" = overwrite everything.
    let replace_any: &[Block] = &[];

    // Build the chimney shaft (1x1 brick column, exactly 3 blocks tall)
    for y in chimney_base..(chimney_base + chimney_height) {
        editor.set_block_absolute(
            BRICK,
            chimney_x,
            y + abs_terrain_offset,
            chimney_z,
            None,
            Some(replace_any),  // Empty blacklist = replace any block
        );
    }

    // Add stone brick slab cap on top
    editor.set_block_absolute(
        STONE_BRICK_SLAB,
        chimney_x,
        chimney_base + chimney_height + abs_terrain_offset,
        chimney_z,
        None,
        Some(replace_any),  // Empty blacklist = replace any block
    );
}

// ============================================================================
// Roof Generation
// ============================================================================

/// Configuration for roof generation
struct RoofConfig {
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    center_x: i32,
    center_z: i32,
    base_height: i32,
    abs_terrain_offset: i32,
    roof_block: Block,
}

impl RoofConfig {
    /// Creates RoofConfig from roof area (includes wall outline for proper coverage)
    fn from_roof_area(
        roof_area: &[(i32, i32)],
        element_id: u64,
        start_y_offset: i32,
        building_height: i32,
        wall_block: Block,
        accent_block: Block,
        abs_terrain_offset: i32,
    ) -> Self {
        // Calculate bounds from the actual roof area (floor + walls)
        let (min_x, max_x, min_z, max_z) = roof_area.iter().fold(
            (i32::MAX, i32::MIN, i32::MAX, i32::MIN),
            |(min_x, max_x, min_z, max_z), &(x, z)| {
                (min_x.min(x), max_x.max(x), min_z.min(z), max_z.max(z))
            },
        );

        let center_x = (min_x + max_x) >> 1;
        let center_z = (min_z + max_z) >> 1;
        // Roof starts at the roof line level (above the top wall block)
        // Wall goes up to start_y_offset + building_height
        // Roof line is at start_y_offset + building_height + 1
        let base_height = start_y_offset + building_height + 1;

        // 90% wall block, 10% accent block for variety (deterministic based on element ID)
        let mut rng = element_rng(element_id);
        // Advance RNG state to get different value than other style choices
        let _ = rng.gen::<u32>();
        let roof_block = if rng.gen_bool(0.1) { accent_block } else { wall_block };

        Self {
            min_x,
            max_x,
            min_z,
            max_z,
            center_x,
            center_z,
            base_height,
            abs_terrain_offset,
            roof_block,
        }
    }

    fn width(&self) -> i32 {
        self.max_x - self.min_x
    }

    fn length(&self) -> i32 {
        self.max_z - self.min_z
    }

    fn building_size(&self) -> i32 {
        self.width().max(self.length())
    }
}

/// Checks if a point has any neighbor with lower height
#[inline]
fn has_lower_neighbor(x: i32, z: i32, roof_height: i32, roof_heights: &HashMap<(i32, i32), i32>) -> bool {
    [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)]
        .iter()
        .any(|(nx, nz)| roof_heights.get(&(*nx, *nz)).is_some_and(|&nh| nh < roof_height))
}

/// Places roof blocks for a given height map
fn place_roof_blocks_with_stairs(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    roof_heights: &HashMap<(i32, i32), i32>,
    config: &RoofConfig,
    stair_direction_fn: impl Fn(i32, i32, i32) -> BlockWithProperties,
) {
    for &(x, z) in floor_area {
        let roof_height = roof_heights[&(x, z)];

        for y in config.base_height..=roof_height {
            if y == roof_height {
                let has_lower = has_lower_neighbor(x, z, roof_height, roof_heights);
                if has_lower {
                    let stair_block = stair_direction_fn(x, z, roof_height);
                    editor.set_block_with_properties_absolute(
                        stair_block,
                        x,
                        y + config.abs_terrain_offset,
                        z,
                        None,
                        None,
                    );
                } else {
                    editor.set_block_absolute(
                        config.roof_block,
                        x,
                        y + config.abs_terrain_offset,
                        z,
                        None,
                        None,
                    );
                }
            } else {
                editor.set_block_absolute(
                    config.roof_block,
                    x,
                    y + config.abs_terrain_offset,
                    z,
                    None,
                    None,
                );
            }
        }
    }
}

/// Generates a flat roof
fn generate_flat_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    floor_block: Block,
    base_height: i32,
    abs_terrain_offset: i32,
) {
    for &(x, z) in floor_area {
        editor.set_block_absolute(floor_block, x, base_height + abs_terrain_offset, z, None, None);
    }
}

/// Generates a gabled roof
fn generate_gabled_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
    roof_orientation: Option<&str>,
) {
    let width_is_longer = config.width() >= config.length();
    let ridge_runs_along_x = match roof_orientation {
        Some(o) if o.eq_ignore_ascii_case("along") => width_is_longer,
        Some(o) if o.eq_ignore_ascii_case("across") => !width_is_longer,
        _ => width_is_longer,
    };

    let max_distance = if ridge_runs_along_x {
        config.length() >> 1
    } else {
        config.width() >> 1
    };

    let roof_height_boost = (3.0 + (config.building_size() as f64 * 0.15).ln().max(1.0)) as i32;
    let roof_peak_height = config.base_height + roof_height_boost;

    // Calculate roof heights
    let mut roof_heights = Vec::with_capacity(floor_area.len());
    for &(x, z) in floor_area {
        let distance_to_ridge = if ridge_runs_along_x {
            (z - config.center_z).abs()
        } else {
            (x - config.center_x).abs()
        };

        let roof_height = if distance_to_ridge == 0
            && ((ridge_runs_along_x && z == config.center_z)
                || (!ridge_runs_along_x && x == config.center_x))
        {
            roof_peak_height
        } else {
            let slope_ratio = distance_to_ridge as f64 / max_distance.max(1) as f64;
            (roof_peak_height as f64 - (slope_ratio * roof_height_boost as f64)) as i32
        }
        .max(config.base_height);

        roof_heights.push(((x, z), roof_height));
    }

    // Place blocks
    let stair_block_material = get_stair_block_for_material(config.roof_block);
    let mut blocks_to_place = Vec::with_capacity(floor_area.len() * 4);

    for &((x, z), roof_height) in &roof_heights {
        let has_lower_neighbor = roof_heights
            .iter()
            .filter_map(|&((nx, nz), nh)| {
                if (nx - x).abs() + (nz - z).abs() == 1 { Some(nh) } else { None }
            })
            .any(|nh| nh < roof_height);

        for y in config.base_height..=roof_height {
            if y == roof_height && has_lower_neighbor {
                let stair_block_with_props = if ridge_runs_along_x {
                    if z < config.center_z {
                        create_stair_with_properties(stair_block_material, StairFacing::South, StairShape::Straight)
                    } else {
                        create_stair_with_properties(stair_block_material, StairFacing::North, StairShape::Straight)
                    }
                } else if x < config.center_x {
                    create_stair_with_properties(stair_block_material, StairFacing::East, StairShape::Straight)
                } else {
                    create_stair_with_properties(stair_block_material, StairFacing::West, StairShape::Straight)
                };
                blocks_to_place.push((x, y, z, Some(stair_block_with_props)));
            } else {
                blocks_to_place.push((x, y, z, None));
            }
        }
    }

    // Batch place all blocks
    for (x, y, z, stair_props) in blocks_to_place {
        if let Some(stair_block) = stair_props {
            editor.set_block_with_properties_absolute(
                stair_block, x, y + config.abs_terrain_offset, z, None, None,
            );
        } else {
            editor.set_block_absolute(
                config.roof_block, x, y + config.abs_terrain_offset, z, None, None,
            );
        }
    }
}

/// Generates a hipped roof for rectangular buildings
/// A hipped roof slopes on ALL four sides, unlike a gabled roof which only slopes on two.
/// For rectangular buildings, it has a ridge along the longer axis, and the shorter
/// ends also slope upward to meet the ridge.
fn generate_hipped_roof_rectangular(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
    ridge_axis_is_x: bool,
    roof_peak_height: i32,
) {
    let mut roof_heights = HashMap::new();

    // For a hipped roof, height is determined by the MINIMUM distance to any edge
    // The ridge runs along one axis, but the ends also slope
    let half_width = config.width() / 2;
    let half_length = config.length() / 2;

    for &(x, z) in floor_area {
        // Distance from each edge
        let dist_from_min_x = x - config.min_x;
        let dist_from_max_x = config.max_x - x;
        let dist_from_min_z = z - config.min_z;
        let dist_from_max_z = config.max_z - z;

        // Minimum distance to any edge determines height (closer to edge = lower)
        let min_dist_to_edge = dist_from_min_x
            .min(dist_from_max_x)
            .min(dist_from_min_z)
            .min(dist_from_max_z);

        // Max possible distance to edge (from center to edge along shorter axis)
        let max_dist_to_edge = if ridge_axis_is_x { half_length } else { half_width };

        // Calculate slope factor (0 at edge, 1 at ridge/center)
        let slope_factor = if max_dist_to_edge > 0 {
            (min_dist_to_edge as f64 / max_dist_to_edge as f64).min(1.0)
        } else {
            1.0
        };

        let roof_height = config.base_height
            + (slope_factor * (roof_peak_height - config.base_height) as f64) as i32;
        roof_heights.insert((x, z), roof_height.max(config.base_height));
    }

    let stair_block_material = get_stair_block_for_material(config.roof_block);
    let min_x = config.min_x;
    let max_x = config.max_x;
    let min_z = config.min_z;
    let max_z = config.max_z;

    // For stair direction, determine which edge the point is closest to
    place_roof_blocks_with_stairs(editor, floor_area, &roof_heights, config, |x, z, _| {
        let dist_from_min_x = x - min_x;
        let dist_from_max_x = max_x - x;
        let dist_from_min_z = z - min_z;
        let dist_from_max_z = max_z - z;

        // Find which edge is closest
        let min_dist = dist_from_min_x
            .min(dist_from_max_x)
            .min(dist_from_min_z)
            .min(dist_from_max_z);

        if dist_from_min_x == min_dist {
            // Closest to west edge, stair faces east (toward center)
            create_stair_with_properties(stair_block_material, StairFacing::East, StairShape::Straight)
        } else if dist_from_max_x == min_dist {
            // Closest to east edge, stair faces west
            create_stair_with_properties(stair_block_material, StairFacing::West, StairShape::Straight)
        } else if dist_from_min_z == min_dist {
            // Closest to north edge, stair faces south
            create_stair_with_properties(stair_block_material, StairFacing::South, StairShape::Straight)
        } else {
            // Closest to south edge, stair faces north
            create_stair_with_properties(stair_block_material, StairFacing::North, StairShape::Straight)
        }
    });
}

/// Generates a hipped roof for square/complex buildings using distance from center
fn generate_hipped_roof_square(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
    roof_peak_height: i32,
) {
    let mut roof_heights = HashMap::new();

    // Calculate max distance from center to any corner
    let max_distance = {
        let corner_distances = [
            ((config.min_x - config.center_x).pow(2) + (config.min_z - config.center_z).pow(2)) as f64,
            ((config.min_x - config.center_x).pow(2) + (config.max_z - config.center_z).pow(2)) as f64,
            ((config.max_x - config.center_x).pow(2) + (config.min_z - config.center_z).pow(2)) as f64,
            ((config.max_x - config.center_x).pow(2) + (config.max_z - config.center_z).pow(2)) as f64,
        ];
        corner_distances.iter().fold(0.0f64, |a, &b| a.max(b)).sqrt()
    };

    for &(x, z) in floor_area {
        let dx = (x - config.center_x) as f64;
        let dz = (z - config.center_z) as f64;
        let distance_from_center = (dx * dx + dz * dz).sqrt();

        let distance_factor = if max_distance > 0.0 {
            (distance_from_center / max_distance).min(1.0)
        } else {
            0.0
        };

        let roof_height = roof_peak_height
            - (distance_factor * (roof_peak_height - config.base_height) as f64) as i32;
        roof_heights.insert((x, z), roof_height.max(config.base_height));
    }

    let stair_block_material = get_stair_block_for_material(config.roof_block);
    let center_x = config.center_x;
    let center_z = config.center_z;

    place_roof_blocks_with_stairs(editor, floor_area, &roof_heights, config, |x, z, _| {
        let center_dx = x - center_x;
        let center_dz = z - center_z;

        if center_dx.abs() > center_dz.abs() {
            if center_dx > 0 {
                create_stair_with_properties(stair_block_material, StairFacing::West, StairShape::Straight)
            } else {
                create_stair_with_properties(stair_block_material, StairFacing::East, StairShape::Straight)
            }
        } else if center_dz > 0 {
            create_stair_with_properties(stair_block_material, StairFacing::North, StairShape::Straight)
        } else {
            create_stair_with_properties(stair_block_material, StairFacing::South, StairShape::Straight)
        }
    });
}

/// Generates a skillion (mono-pitch) roof
fn generate_skillion_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
) {
    let width = config.width().max(1);
    let max_roof_height = (config.building_size() / 3).clamp(4, 10);

    let mut roof_heights = HashMap::new();
    for &(x, z) in floor_area {
        let slope_progress = (x - config.min_x) as f64 / width as f64;
        let roof_height = config.base_height + (slope_progress * max_roof_height as f64) as i32;
        roof_heights.insert((x, z), roof_height);
    }

    let stair_block_material = get_stair_block_for_material(config.roof_block);

    place_roof_blocks_with_stairs(editor, floor_area, &roof_heights, config, |_, _, _| {
        create_stair_with_properties(stair_block_material, StairFacing::East, StairShape::Straight)
    });
}

/// Generates a pyramidal roof
fn generate_pyramidal_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
) {
    let peak_height = config.base_height + (config.building_size() / 3).clamp(3, 8);
    let max_distance = (config.width() / 2).max(config.length() / 2) as f64;

    let mut roof_heights = HashMap::new();
    for &(x, z) in floor_area {
        let dx = (x - config.center_x).abs() as f64;
        let dz = (z - config.center_z).abs() as f64;
        let distance_to_edge = dx.max(dz);

        let height_factor = if max_distance > 0.0 {
            (1.0 - (distance_to_edge / max_distance)).max(0.0)
        } else {
            1.0
        };

        let roof_height = config.base_height + (height_factor * (peak_height - config.base_height) as f64) as i32;
        roof_heights.insert((x, z), roof_height);
    }

    // Place blocks with complex stair logic for pyramid corners
    let stair_block_material = get_stair_block_for_material(config.roof_block);

    for &(x, z) in floor_area {
        let roof_height = roof_heights[&(x, z)];

        for y in config.base_height..=roof_height {
            if y == roof_height {
                let stair_block = determine_pyramidal_stair_block(
                    x, z, roof_height, &roof_heights, config, stair_block_material,
                );
                editor.set_block_with_properties_absolute(
                    stair_block, x, y + config.abs_terrain_offset, z, None, None,
                );
            } else {
                editor.set_block_absolute(
                    config.roof_block, x, y + config.abs_terrain_offset, z, None, None,
                );
            }
        }
    }
}

/// Determines the appropriate stair block for pyramidal roof corners and edges
fn determine_pyramidal_stair_block(
    x: i32,
    z: i32,
    roof_height: i32,
    roof_heights: &HashMap<(i32, i32), i32>,
    config: &RoofConfig,
    stair_block_material: Block,
) -> BlockWithProperties {
    let dx = x - config.center_x;
    let dz = z - config.center_z;

    let north_height = roof_heights.get(&(x, z - 1)).copied().unwrap_or(config.base_height);
    let south_height = roof_heights.get(&(x, z + 1)).copied().unwrap_or(config.base_height);
    let west_height = roof_heights.get(&(x - 1, z)).copied().unwrap_or(config.base_height);
    let east_height = roof_heights.get(&(x + 1, z)).copied().unwrap_or(config.base_height);

    let has_lower_north = north_height < roof_height;
    let has_lower_south = south_height < roof_height;
    let has_lower_west = west_height < roof_height;
    let has_lower_east = east_height < roof_height;

    // Corner situations
    if has_lower_north && has_lower_west {
        create_stair_with_properties(stair_block_material, StairFacing::East, StairShape::OuterRight)
    } else if has_lower_north && has_lower_east {
        create_stair_with_properties(stair_block_material, StairFacing::South, StairShape::OuterRight)
    } else if has_lower_south && has_lower_west {
        create_stair_with_properties(stair_block_material, StairFacing::East, StairShape::OuterLeft)
    } else if has_lower_south && has_lower_east {
        create_stair_with_properties(stair_block_material, StairFacing::North, StairShape::OuterLeft)
    } else if dx.abs() > dz.abs() {
        // Primary slope in X direction
        if dx > 0 && east_height < roof_height {
            create_stair_with_properties(stair_block_material, StairFacing::West, StairShape::Straight)
        } else if dx < 0 && west_height < roof_height {
            create_stair_with_properties(stair_block_material, StairFacing::East, StairShape::Straight)
        } else if dz > 0 && south_height < roof_height {
            create_stair_with_properties(stair_block_material, StairFacing::North, StairShape::Straight)
        } else if dz < 0 && north_height < roof_height {
            create_stair_with_properties(stair_block_material, StairFacing::South, StairShape::Straight)
        } else {
            BlockWithProperties::simple(config.roof_block)
        }
    } else {
        // Primary slope in Z direction
        if dz > 0 && south_height < roof_height {
            create_stair_with_properties(stair_block_material, StairFacing::North, StairShape::Straight)
        } else if dz < 0 && north_height < roof_height {
            create_stair_with_properties(stair_block_material, StairFacing::South, StairShape::Straight)
        } else if dx > 0 && east_height < roof_height {
            create_stair_with_properties(stair_block_material, StairFacing::West, StairShape::Straight)
        } else if dx < 0 && west_height < roof_height {
            create_stair_with_properties(stair_block_material, StairFacing::East, StairShape::Straight)
        } else {
            BlockWithProperties::simple(config.roof_block)
        }
    }
}

/// Generates a dome roof
fn generate_dome_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
) {
    let radius = (config.building_size() / 2) as f64;

    for &(x, z) in floor_area {
        let distance_from_center = ((x - config.center_x).pow(2) + (z - config.center_z).pow(2)) as f64;
        let normalized_distance = (distance_from_center.sqrt() / radius).min(1.0);

        let height_factor = (1.0 - normalized_distance * normalized_distance).sqrt();
        let surface_height = config.base_height + (height_factor * (radius * 0.8)) as i32;

        for y in config.base_height..=surface_height {
            editor.set_block_absolute(
                config.roof_block, x, y + config.abs_terrain_offset, z, None, None,
            );
        }
    }
}

/// Unified function to generate various roof types
#[allow(clippy::too_many_arguments)]
#[inline]
fn generate_roof(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    start_y_offset: i32,
    building_height: i32,
    floor_block: Block,
    wall_block: Block,
    accent_block: Block,
    roof_type: RoofType,
    roof_area: &[(i32, i32)],
    abs_terrain_offset: i32,
) {
    let config = RoofConfig::from_roof_area(
        roof_area,
        element.id,
        start_y_offset,
        building_height,
        wall_block,
        accent_block,
        abs_terrain_offset,
    );

    let roof_orientation = element.tags.get("roof:orientation").map(|s| s.as_str());

    match roof_type {
        RoofType::Flat => {
            generate_flat_roof(editor, roof_area, floor_block, config.base_height, abs_terrain_offset);
        }

        RoofType::Gabled => {
            generate_gabled_roof(editor, roof_area, &config, roof_orientation);
        }

        RoofType::Hipped => {
            let is_rectangular = (config.width() as f64 / config.length() as f64 > 1.3)
                || (config.length() as f64 / config.width() as f64 > 1.3);
            let width_is_longer = config.width() >= config.length();
            let ridge_axis_is_x = match roof_orientation {
                Some(o) if o.eq_ignore_ascii_case("along") => width_is_longer,
                Some(o) if o.eq_ignore_ascii_case("across") => !width_is_longer,
                _ => width_is_longer,
            };
            let roof_peak_height = config.base_height + if config.building_size() > 20 { 7 } else { 5 };

            if is_rectangular {
                generate_hipped_roof_rectangular(editor, roof_area, &config, ridge_axis_is_x, roof_peak_height);
            } else {
                generate_hipped_roof_square(editor, roof_area, &config, roof_peak_height);
            }
        }

        RoofType::Skillion => {
            generate_skillion_roof(editor, roof_area, &config);
        }

        RoofType::Pyramidal => {
            generate_pyramidal_roof(editor, roof_area, &config);
        }

        RoofType::Dome => {
            generate_dome_roof(editor, roof_area, &config);
        }
    }
}

pub fn generate_building_from_relation(
    editor: &mut WorldEditor,
    relation: &ProcessedRelation,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
) {
    // Skip building:part relations if layer or level is negative (underground parts)
    if relation.tags.contains_key("building:part") {
        if let Some(layer) = relation.tags.get("layer") {
            if layer.parse::<i32>().unwrap_or(0) < 0 {
                return;
            }
        }
        if let Some(level) = relation.tags.get("level") {
            if level.parse::<i32>().unwrap_or(0) < 0 {
                return;
            }
        }
    }

    // Extract levels from relation tags
    let relation_levels = relation
        .tags
        .get("building:levels")
        .and_then(|l: &String| l.parse::<i32>().ok())
        .unwrap_or(2); // Default to 2 levels

    // Process the outer way to create the building walls
    for member in &relation.members {
        if member.role == ProcessedMemberRole::Outer {
            generate_buildings(
                editor,
                &member.way,
                args,
                Some(relation_levels),
                flood_fill_cache,
            );
        }
    }

    // Handle inner ways (holes, courtyards, etc.)
    /*for member in &relation.members {
        if member.role == ProcessedMemberRole::Inner {
            let polygon_coords: Vec<(i32, i32)> =
                member.way.nodes.iter().map(|n| (n.x, n.z)).collect();
            let hole_area: Vec<(i32, i32)> =
                flood_fill_area(&polygon_coords, args.timeout.as_ref());

            for (x, z) in hole_area {
                // Remove blocks in the inner area to create a hole
                editor.set_block(AIR, x, ground_level, z, None, Some(&[SPONGE]));
            }
        }
    }*/
}

/// Generates a bridge structure, paying attention to the "level" tag.
/// Bridge deck is interpolated between start and end point elevations to avoid
/// being dragged down by valleys underneath.
fn generate_bridge(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    flood_fill_cache: &FloodFillCache,
    floodfill_timeout: Option<&Duration>,
) {
    let floor_block: Block = STONE;
    let railing_block: Block = STONE_BRICKS;

    // Calculate bridge level offset based on the "level" tag
    let bridge_y_offset = if let Some(level_str) = element.tags.get("level") {
        if let Ok(level) = level_str.parse::<i32>() {
            (level * 3) + 1
        } else {
            1 // Default elevation
        }
    } else {
        1 // Default elevation
    };

    // Need at least 2 nodes to form a bridge
    if element.nodes.len() < 2 {
        return;
    }

    // Get start and end node elevations and use MAX for level bridge deck
    // Using MAX ensures bridges don't dip when multiple bridge ways meet in a valley
    let start_node = &element.nodes[0];
    let end_node = &element.nodes[element.nodes.len() - 1];
    let start_y = editor.get_ground_level(start_node.x, start_node.z);
    let end_y = editor.get_ground_level(end_node.x, end_node.z);
    let bridge_deck_ground_y = start_y.max(end_y);

    // Process the nodes to create bridge pathways and railings
    let mut previous_node: Option<(i32, i32)> = None;

    for node in &element.nodes {
        let x: i32 = node.x;
        let z: i32 = node.z;

        // Create bridge path using Bresenham's line
        if let Some(prev) = previous_node {
            let bridge_points: Vec<(i32, i32, i32)> = bresenham_line(prev.0, 0, prev.1, x, 0, z);

            for (bx, _, bz) in bridge_points.iter() {
                // Use fixed bridge deck height (max of endpoints)
                let bridge_y = bridge_deck_ground_y + bridge_y_offset;

                // Place railing blocks
                editor.set_block_absolute(railing_block, *bx, bridge_y + 1, *bz, None, None);
                editor.set_block_absolute(railing_block, *bx, bridge_y, *bz, None, None);
            }
        }

        previous_node = Some((x, z));
    }

    // Flood fill the area between the bridge path nodes (uses cache)
    let bridge_area: Vec<(i32, i32)> = flood_fill_cache.get_or_compute(element, floodfill_timeout);

    // Use the same level bridge deck height for filled areas
    let floor_y = bridge_deck_ground_y + bridge_y_offset;

    // Place floor blocks
    for (x, z) in bridge_area {
        editor.set_block_absolute(floor_block, x, floor_y, z, None, None);
    }
}
