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
    Hipped, // All sides slope downwards to walls (including Half-hipped, Gambrel, Mansard variations)
    Skillion, // Single sloping surface
    Pyramidal, // All sides come to a point at the top
    Dome,   // Rounded, hemispherical structure
    Flat,   // Default flat roof
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

// ============================================================================
// Wall Block Palettes for Different Building Types
// ============================================================================

/// Wall blocks suitable for residential buildings (warm, homey materials)
const RESIDENTIAL_WALL_OPTIONS: [Block; 24] = [
    BRICK,
    STONE_BRICKS,
    WHITE_TERRACOTTA,
    BROWN_TERRACOTTA,
    SANDSTONE,
    SMOOTH_SANDSTONE,
    QUARTZ_BRICKS,
    MUD_BRICKS,
    POLISHED_GRANITE,
    END_STONE_BRICKS,
    BROWN_CONCRETE,
    DEEPSLATE_BRICKS,
    GRAY_CONCRETE,
    GRAY_TERRACOTTA,
    LIGHT_BLUE_TERRACOTTA,
    LIGHT_GRAY_CONCRETE,
    LIGHT_GRAY_TERRACOTTA,
    NETHER_BRICK,
    POLISHED_ANDESITE,
    POLISHED_BLACKSTONE,
    POLISHED_BLACKSTONE_BRICKS,
    POLISHED_DEEPSLATE,
    QUARTZ_BLOCK,
    WHITE_CONCRETE,
];

/// Wall blocks suitable for commercial/office buildings (modern, clean look)
const COMMERCIAL_WALL_OPTIONS: [Block; 8] = [
    WHITE_CONCRETE,
    LIGHT_GRAY_CONCRETE,
    GRAY_CONCRETE,
    POLISHED_ANDESITE,
    SMOOTH_STONE,
    QUARTZ_BLOCK,
    QUARTZ_BRICKS,
    STONE_BRICKS,
];

/// Wall blocks suitable for industrial buildings (utilitarian)
const INDUSTRIAL_WALL_OPTIONS: [Block; 7] = [
    GRAY_CONCRETE,
    LIGHT_GRAY_CONCRETE,
    STONE,
    SMOOTH_STONE,
    POLISHED_ANDESITE,
    DEEPSLATE_BRICKS,
    BLACKSTONE,
];

/// Wall blocks suitable for religious buildings (ornate, traditional)
const RELIGIOUS_WALL_OPTIONS: [Block; 8] = [
    STONE_BRICKS,
    CHISELED_STONE_BRICKS,
    QUARTZ_BLOCK,
    WHITE_CONCRETE,
    SANDSTONE,
    SMOOTH_SANDSTONE,
    POLISHED_DIORITE,
    END_STONE_BRICKS,
];

/// Wall blocks suitable for institutional buildings (formal, clean)
const INSTITUTIONAL_WALL_OPTIONS: [Block; 8] = [
    WHITE_CONCRETE,
    LIGHT_GRAY_CONCRETE,
    QUARTZ_BRICKS,
    STONE_BRICKS,
    POLISHED_ANDESITE,
    SMOOTH_STONE,
    SANDSTONE,
    END_STONE_BRICKS,
];

/// Wall blocks suitable for farm/agricultural buildings (rustic)
const FARM_WALL_OPTIONS: [Block; 6] = [
    OAK_PLANKS,
    SPRUCE_PLANKS,
    DARK_OAK_PLANKS,
    COBBLESTONE,
    STONE,
    MUD_BRICKS,
];

/// Wall blocks suitable for historic/castle buildings
const HISTORIC_WALL_OPTIONS: [Block; 10] = [
    STONE_BRICKS,
    CRACKED_STONE_BRICKS,
    CHISELED_STONE_BRICKS,
    COBBLESTONE,
    MOSSY_COBBLESTONE,
    DEEPSLATE_BRICKS,
    POLISHED_ANDESITE,
    ANDESITE,
    SMOOTH_STONE,
    BRICK,
];

/// Wall blocks for garages (sturdy, simple)
const GARAGE_WALL_OPTIONS: [Block; 3] = [BRICK, STONE_BRICKS, POLISHED_ANDESITE];

/// Wall blocks for sheds (wooden)
const SHED_WALL_OPTIONS: [Block; 1] = [OAK_LOG];

/// Wall blocks for greenhouses (glass variants)
const GREENHOUSE_WALL_OPTIONS: [Block; 4] = [
    GLASS,
    GLASS_PANE,
    WHITE_STAINED_GLASS,
    LIGHT_GRAY_STAINED_GLASS,
];

// ============================================================================
// Building Category System
// ============================================================================

/// Building category determines which preset rules to apply.
/// This is derived from OSM tags and can influence style choices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BuildingCategory {
    // Residential types
    Residential, // Generic residential (apartments, etc.)
    House,       // Single-family homes
    Farm,        // Farmhouses and agricultural dwellings

    // Commercial types
    Commercial, // Shops, retail, supermarkets
    Office,     // Office buildings
    Hotel,      // Hotels and accommodation

    // Industrial types
    Industrial, // Factories, manufacturing
    Warehouse,  // Storage and logistics

    // Institutional types
    School,    // Schools, kindergartens, colleges
    Hospital,  // Healthcare buildings
    Religious, // Churches, mosques, temples, etc.

    // Special types
    Skyscraper, // Tall buildings (>7 floors or >28m)
    Historic,   // Castles, ruins, historic buildings
    Garage,     // Garages and carports
    Shed,       // Sheds, huts, simple storage
    Greenhouse, // Greenhouses and glasshouses

    Default, // Unknown or generic buildings
}

impl BuildingCategory {
    /// Determines the building category from OSM tags and calculated properties
    fn from_element(element: &ProcessedWay, is_tall_building: bool, building_height: i32) -> Self {
        // Check for skyscraper first (based on height)
        if is_tall_building || building_height > 28 {
            return BuildingCategory::Skyscraper;
        }

        // Check for historic buildings
        if element.tags.contains_key("historic") {
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
            // Single-family homes
            "house" | "detached" | "semidetached_house" | "terrace" | "bungalow" | "villa"
            | "cabin" | "hut" => BuildingCategory::House,

            // Multi-family residential
            "residential" | "apartments" | "dormitory" => BuildingCategory::Residential,

            // Farm and agricultural
            "farm" | "farm_auxiliary" | "barn" | "stable" | "cowshed" | "sty" | "sheepfold" => {
                BuildingCategory::Farm
            }

            // Commercial/retail
            "commercial" | "retail" | "supermarket" | "kiosk" | "shop" => {
                BuildingCategory::Commercial
            }

            // Office buildings
            "office" => BuildingCategory::Office,

            // Hotels and accommodation
            "hotel" => BuildingCategory::Hotel,

            // Industrial/manufacturing
            "industrial" | "factory" | "manufacture" | "hangar" => BuildingCategory::Industrial,

            // Warehouses and storage
            "warehouse" | "storage_tank" => BuildingCategory::Warehouse,

            // Schools and education
            "school" | "kindergarten" | "college" | "university" => BuildingCategory::School,

            // Healthcare
            "hospital" => BuildingCategory::Hospital,

            // Religious buildings
            "religious" | "church" | "cathedral" | "chapel" | "mosque" | "synagogue" | "temple" => {
                BuildingCategory::Religious
            }

            // Historic structures
            "castle" | "ruins" | "fort" | "bunker" => BuildingCategory::Historic,

            // Garages
            "garage" | "garages" | "carport" => BuildingCategory::Garage,

            // Simple storage structures
            "shed" => BuildingCategory::Shed,

            // Greenhouses
            "greenhouse" | "glasshouse" => BuildingCategory::Greenhouse,

            // Public/civic (map to appropriate institutional)
            "public" | "government" | "civic" => BuildingCategory::School, // Use school style for generic institutional

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
    pub roof_block: Option<Block>, // Material for roof (used in gabled roofs, etc.)

    // Window style
    pub use_vertical_windows: Option<bool>,
    pub has_windows: Option<bool>, // Whether to generate windows at all

    // Accent features
    pub use_accent_roof_line: Option<bool>,
    pub use_accent_lines: Option<bool>,
    pub use_vertical_accent: Option<bool>,

    // Roof
    pub roof_type: Option<RoofType>,
    pub has_chimney: Option<bool>,
    pub generate_roof: Option<bool>,

    // Special features
    pub has_garage_door: Option<bool>, // Generate double door on front face
    pub has_single_door: Option<bool>, // Generate a single door somewhere
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
            use_accent_lines: Some(false), // Residential buildings rarely have accent lines
            ..Default::default()
        }
    }

    /// Preset for skyscrapers and tall buildings
    pub fn skyscraper() -> Self {
        Self {
            use_vertical_windows: Some(true), // Always vertical windows
            roof_type: Some(RoofType::Flat),  // Always flat roof
            has_chimney: Some(false),         // No chimneys on skyscrapers
            use_accent_roof_line: Some(true), // Usually have accent roof line
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

    /// Preset for single-family houses
    pub fn house() -> Self {
        Self {
            use_vertical_windows: Some(false),
            use_accent_lines: Some(false),
            use_accent_roof_line: Some(true),
            has_chimney: Some(true), // Houses often have chimneys
            ..Default::default()
        }
    }

    /// Preset for farm buildings (barns, stables, etc.)
    pub fn farm() -> Self {
        Self {
            use_vertical_windows: Some(false),
            use_accent_lines: Some(false),
            use_vertical_accent: Some(false),
            has_chimney: Some(false),
            ..Default::default()
        }
    }

    /// Preset for office buildings
    pub fn office() -> Self {
        Self {
            use_vertical_windows: Some(true), // Office buildings typically have vertical windows
            use_accent_roof_line: Some(true),
            has_chimney: Some(false),
            ..Default::default()
        }
    }

    /// Preset for hotels
    pub fn hotel() -> Self {
        Self {
            use_vertical_windows: Some(true),
            use_accent_roof_line: Some(true),
            use_accent_lines: Some(true), // Hotels often have floor-separating lines
            has_chimney: Some(false),
            ..Default::default()
        }
    }

    /// Preset for warehouses
    pub fn warehouse() -> Self {
        Self {
            roof_type: Some(RoofType::Flat),
            has_chimney: Some(false),
            use_accent_lines: Some(false),
            use_vertical_accent: Some(false),
            use_accent_roof_line: Some(false),
            use_vertical_windows: Some(false),
            ..Default::default()
        }
    }

    /// Preset for schools and educational buildings
    pub fn school() -> Self {
        Self {
            use_vertical_windows: Some(false), // Schools usually have regular windows
            use_accent_roof_line: Some(true),
            has_chimney: Some(false),
            ..Default::default()
        }
    }

    /// Preset for hospitals
    pub fn hospital() -> Self {
        Self {
            use_vertical_windows: Some(true),
            use_accent_roof_line: Some(true),
            roof_type: Some(RoofType::Flat), // Hospitals typically have flat roofs
            has_chimney: Some(false),
            ..Default::default()
        }
    }

    /// Preset for religious buildings (churches, mosques, etc.)
    pub fn religious() -> Self {
        Self {
            use_vertical_windows: Some(true), // Tall stained glass windows
            use_accent_roof_line: Some(true),
            use_accent_lines: Some(false),
            has_chimney: Some(false),
            ..Default::default()
        }
    }

    /// Preset for garages and carports
    pub fn garage() -> Self {
        Self {
            roof_type: Some(RoofType::Flat),
            roof_block: Some(POLISHED_ANDESITE), // Always polished andesite roof
            has_chimney: Some(false),
            use_accent_lines: Some(false),
            use_vertical_accent: Some(false),
            use_accent_roof_line: Some(false),
            generate_roof: Some(true),
            has_windows: Some(false),    // No windows on garages
            has_garage_door: Some(true), // Generate double door on front
            ..Default::default()
        }
    }

    /// Preset for sheds and small storage structures
    pub fn shed() -> Self {
        Self {
            wall_block: Some(OAK_LOG),    // Oak logs for walls
            roof_block: Some(OAK_PLANKS), // Oak planks for roof
            has_chimney: Some(false),
            use_accent_lines: Some(false),
            use_vertical_accent: Some(false),
            use_accent_roof_line: Some(false),
            has_windows: Some(false),    // No windows on sheds
            has_single_door: Some(true), // One door somewhere
            ..Default::default()
        }
    }

    /// Preset for greenhouses
    pub fn greenhouse() -> Self {
        Self {
            // Wall block is randomly chosen from GREENHOUSE_WALL_OPTIONS
            roof_block: Some(SMOOTH_STONE_SLAB), // Slab roof (will randomize between oak and smooth stone)
            has_chimney: Some(false),
            use_accent_lines: Some(false),
            use_vertical_accent: Some(false),
            use_accent_roof_line: Some(false),
            roof_type: Some(RoofType::Flat),
            generate_roof: Some(true),
            has_windows: Some(false),    // The walls themselves are glass
            has_single_door: Some(true), // One entrance door
            ..Default::default()
        }
    }

    /// Preset for commercial buildings (retail, shops)
    pub fn commercial() -> Self {
        Self {
            use_vertical_windows: Some(false),
            use_accent_roof_line: Some(true),
            ..Default::default()
        }
    }

    /// Gets the appropriate preset for a building category
    pub fn for_category(category: BuildingCategory) -> Self {
        match category {
            BuildingCategory::House => Self::house(),
            BuildingCategory::Residential => Self::residential(),
            BuildingCategory::Farm => Self::farm(),
            BuildingCategory::Commercial => Self::commercial(),
            BuildingCategory::Office => Self::office(),
            BuildingCategory::Hotel => Self::hotel(),
            BuildingCategory::Industrial => Self::industrial(),
            BuildingCategory::Warehouse => Self::warehouse(),
            BuildingCategory::School => Self::school(),
            BuildingCategory::Hospital => Self::hospital(),
            BuildingCategory::Religious => Self::religious(),
            BuildingCategory::Historic => Self::historic(),
            BuildingCategory::Garage => Self::garage(),
            BuildingCategory::Shed => Self::shed(),
            BuildingCategory::Greenhouse => Self::greenhouse(),
            BuildingCategory::Skyscraper => Self::skyscraper(),
            BuildingCategory::Default => Self::empty(),
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
    pub roof_block: Option<Block>, // Optional specific roof material

    // Window style
    pub use_vertical_windows: bool,
    pub has_windows: bool, // Whether to generate windows

    // Accent features
    pub use_accent_roof_line: bool,
    pub use_accent_lines: bool,
    pub use_vertical_accent: bool,

    // Roof
    pub roof_type: RoofType,
    pub has_chimney: bool,
    pub generate_roof: bool,

    // Special features
    pub has_garage_door: bool,
    pub has_single_door: bool,
}

impl BuildingStyle {
    /// Resolves a preset into a fully determined style using deterministic RNG.
    /// Parameters not specified in the preset are randomly chosen.
    ///
    /// # Arguments
    /// * `preset` - The style preset (partial specification)
    /// * `element` - The OSM element (used for tag-based decisions)
    /// * `building_type` - The building type string from tags
    /// * `category` - The resolved building category
    /// * `has_multiple_floors` - Whether building has more than 6 height units
    /// * `footprint_size` - The building's floor area in blocks
    /// * `rng` - Deterministic RNG seeded by element ID
    #[allow(clippy::too_many_arguments)]
    pub fn resolve(
        preset: &BuildingStylePreset,
        element: &ProcessedWay,
        building_type: &str,
        category: BuildingCategory,
        has_multiple_floors: bool,
        footprint_size: usize,
        rng: &mut impl Rng,
    ) -> Self {
        // === Block Palette ===

        // Wall block: from tags, preset, or category palette
        let wall_block = preset
            .wall_block
            .unwrap_or_else(|| determine_wall_block(element, category, rng));

        // Floor block: from preset or random
        let floor_block = preset
            .floor_block
            .unwrap_or_else(|| get_floor_block_with_rng(rng));

        // Window block: from preset or random based on building type
        let window_block = preset
            .window_block
            .unwrap_or_else(|| get_window_block_for_building_type_with_rng(building_type, rng));

        // Accent block: from preset or random
        let accent_block = preset
            .accent_block
            .unwrap_or_else(|| ACCENT_BLOCK_OPTIONS[rng.gen_range(0..ACCENT_BLOCK_OPTIONS.len())]);

        // === Window Style ===

        let use_vertical_windows = preset
            .use_vertical_windows
            .unwrap_or_else(|| rng.gen_bool(0.7));

        // === Accent Features ===

        let use_accent_roof_line = preset
            .use_accent_roof_line
            .unwrap_or_else(|| rng.gen_bool(0.25));

        // Accent lines only for multi-floor buildings
        let use_accent_lines = preset
            .use_accent_lines
            .unwrap_or_else(|| has_multiple_floors && rng.gen_bool(0.2));

        // Vertical accent: only if no accent lines and multi-floor
        let use_vertical_accent = preset
            .use_vertical_accent
            .unwrap_or_else(|| has_multiple_floors && !use_accent_lines && rng.gen_bool(0.1));

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
                "house"
                    | "residential"
                    | "detached"
                    | "semidetached_house"
                    | "terrace"
                    | "farm"
                    | "cabin"
                    | "bungalow"
                    | "villa"
                    | "yes"
            );
            let suitable_roof = matches!(roof_type, RoofType::Gabled | RoofType::Hipped);
            let suitable_size = (30..=400).contains(&footprint_size);

            is_residential && suitable_roof && suitable_size && rng.gen_bool(0.55)
        });

        // Roof block: specific material for roofs
        let roof_block = preset.roof_block;

        // Windows: default to true unless explicitly disabled
        let has_windows = preset.has_windows.unwrap_or(true);

        // Special door features
        let has_garage_door = preset.has_garage_door.unwrap_or(false);
        let has_single_door = preset.has_single_door.unwrap_or(false);

        Self {
            wall_block,
            floor_block,
            window_block,
            accent_block,
            roof_block,
            use_vertical_windows,
            has_windows,
            use_accent_roof_line,
            use_accent_lines,
            use_vertical_accent,
            roof_type,
            has_chimney,
            generate_roof,
            has_garage_door,
            has_single_door,
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
    #[allow(dead_code)] // Reserved for future use in roof generation
    roof_block: Option<Block>,
    use_vertical_windows: bool,
    use_accent_roof_line: bool,
    use_accent_lines: bool,
    use_vertical_accent: bool,
    is_abandoned_building: bool,
    has_windows: bool,
    has_garage_door: bool,
    has_single_door: bool,
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

/// Checks if a building should be skipped (underground structures)
#[inline]
fn should_skip_underground_building(element: &ProcessedWay) -> bool {
    // Check layer tag, negative means underground
    if let Some(layer) = element.tags.get("layer") {
        if layer.parse::<i32>().unwrap_or(0) < 0 {
            return true;
        }
    }

    // Check level tag, negative means underground
    if let Some(level) = element.tags.get("level") {
        if level.parse::<i32>().unwrap_or(0) < 0 {
            return true;
        }
    }

    // Check location tag
    if let Some(location) = element.tags.get("location") {
        if location == "underground" || location == "subway" {
            return true;
        }
    }

    // Check building:levels:underground - if this is the only levels tag, it's underground
    if element.tags.contains_key("building:levels:underground")
        && !element.tags.contains_key("building:levels")
    {
        return true;
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
fn determine_wall_block(
    element: &ProcessedWay,
    category: BuildingCategory,
    rng: &mut impl Rng,
) -> Block {
    // Historic castles have their own special treatment
    if element.tags.get("historic") == Some(&"castle".to_string()) {
        return get_castle_wall_block();
    }

    // Try to get wall block from building:colour tag first
    if let Some(building_colour) = element.tags.get("building:colour") {
        if let Some(rgb) = color_text_to_rgb_tuple(building_colour) {
            return get_building_wall_block_for_color(rgb);
        }
    }

    // Otherwise, select from category-specific palette
    get_wall_block_for_category(category, rng)
}

/// Selects a wall block from the appropriate category palette
fn get_wall_block_for_category(category: BuildingCategory, rng: &mut impl Rng) -> Block {
    match category {
        BuildingCategory::House | BuildingCategory::Residential => {
            RESIDENTIAL_WALL_OPTIONS[rng.gen_range(0..RESIDENTIAL_WALL_OPTIONS.len())]
        }
        BuildingCategory::Commercial | BuildingCategory::Office | BuildingCategory::Hotel => {
            COMMERCIAL_WALL_OPTIONS[rng.gen_range(0..COMMERCIAL_WALL_OPTIONS.len())]
        }
        BuildingCategory::Industrial | BuildingCategory::Warehouse => {
            INDUSTRIAL_WALL_OPTIONS[rng.gen_range(0..INDUSTRIAL_WALL_OPTIONS.len())]
        }
        BuildingCategory::Religious => {
            RELIGIOUS_WALL_OPTIONS[rng.gen_range(0..RELIGIOUS_WALL_OPTIONS.len())]
        }
        BuildingCategory::School | BuildingCategory::Hospital => {
            INSTITUTIONAL_WALL_OPTIONS[rng.gen_range(0..INSTITUTIONAL_WALL_OPTIONS.len())]
        }
        BuildingCategory::Farm => FARM_WALL_OPTIONS[rng.gen_range(0..FARM_WALL_OPTIONS.len())],
        BuildingCategory::Historic => {
            HISTORIC_WALL_OPTIONS[rng.gen_range(0..HISTORIC_WALL_OPTIONS.len())]
        }
        BuildingCategory::Garage => {
            GARAGE_WALL_OPTIONS[rng.gen_range(0..GARAGE_WALL_OPTIONS.len())]
        }
        BuildingCategory::Shed => SHED_WALL_OPTIONS[rng.gen_range(0..SHED_WALL_OPTIONS.len())],
        BuildingCategory::Greenhouse => {
            GREENHOUSE_WALL_OPTIONS[rng.gen_range(0..GREENHOUSE_WALL_OPTIONS.len())]
        }
        BuildingCategory::Skyscraper => {
            // Skyscrapers use commercial palette (glass, concrete, stone)
            COMMERCIAL_WALL_OPTIONS[rng.gen_range(0..COMMERCIAL_WALL_OPTIONS.len())]
        }
        BuildingCategory::Default => get_fallback_building_block(),
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
            let floor_block = if level == 0 {
                SMOOTH_STONE
            } else {
                COBBLESTONE
            };
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
    has_sloped_roof: bool,
) -> (Vec<(i32, i32)>, (i32, i32, i32)) {
    let mut previous_node: Option<(i32, i32)> = None;
    let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
    let mut current_building: Vec<(i32, i32)> = Vec::new();

    for node in &element.nodes {
        let x = node.x;
        let z = node.z;

        if let Some(prev) = previous_node {
            let bresenham_points = bresenham_line(
                prev.0,
                config.start_y_offset,
                prev.1,
                x,
                config.start_y_offset,
                z,
            );

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
                for h in
                    (config.start_y_offset + 1)..=(config.start_y_offset + config.building_height)
                {
                    let block = determine_wall_block_at_position(bx, h, bz, config);
                    editor.set_block_absolute(
                        block,
                        bx,
                        h + config.abs_terrain_offset,
                        bz,
                        None,
                        None,
                    );
                }

                // Add roof line only for flat roofs - sloped roofs will cover this area
                if !has_sloped_roof {
                    let roof_line_block = if config.use_accent_roof_line {
                        config.accent_block
                    } else {
                        config.wall_block
                    };
                    editor.set_block_absolute(
                        roof_line_block,
                        bx,
                        config.start_y_offset
                            + config.building_height
                            + config.abs_terrain_offset
                            + 1,
                        bz,
                        None,
                        None,
                    );
                }

                current_building.push((bx, bz));
                corner_addup = (corner_addup.0 + bx, corner_addup.1 + bz, corner_addup.2 + 1);
            }
        }

        previous_node = Some((x, z));
    }

    (current_building, corner_addup)
}

/// Generates special doors for garages (double door) and sheds (single door)
fn generate_special_doors(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    wall_outline: &[(i32, i32)],
) {
    if wall_outline.is_empty() {
        return;
    }

    // Find the front-facing wall segment (longest or first significant segment)
    // We'll use the first wall segment from the element nodes
    let nodes = &element.nodes;
    if nodes.len() < 2 {
        return;
    }

    let mut rng = element_rng(element.id);
    let door_y = config.start_y_offset + config.abs_terrain_offset + 1;

    if config.has_garage_door {
        // Place double spruce door on front face
        // Find a suitable wall segment (first one with enough length)
        for i in 0..nodes.len().saturating_sub(1) {
            let (x1, z1) = (nodes[i].x, nodes[i].z);
            let (x2, z2) = (nodes[i + 1].x, nodes[i + 1].z);

            let dx = (x2 - x1).abs();
            let dz = (z2 - z1).abs();
            let segment_len = dx.max(dz);

            // Need at least 2 blocks for double door
            if segment_len >= 2 {
                // Place doors in the middle of this segment
                let mid_x = (x1 + x2) / 2;
                let mid_z = (z1 + z2) / 2;

                // Determine door offset based on wall orientation
                let (door1_x, door1_z, door2_x, door2_z) = if dx > dz {
                    // Wall runs along X axis
                    (mid_x, mid_z, mid_x + 1, mid_z)
                } else {
                    // Wall runs along Z axis
                    (mid_x, mid_z, mid_x, mid_z + 1)
                };

                // Place the double door (lower and upper parts)
                editor.set_block_absolute(SPRUCE_DOOR_LOWER, door1_x, door_y, door1_z, None, None);
                editor.set_block_absolute(
                    SPRUCE_DOOR_UPPER,
                    door1_x,
                    door_y + 1,
                    door1_z,
                    None,
                    None,
                );
                editor.set_block_absolute(SPRUCE_DOOR_LOWER, door2_x, door_y, door2_z, None, None);
                editor.set_block_absolute(
                    SPRUCE_DOOR_UPPER,
                    door2_x,
                    door_y + 1,
                    door2_z,
                    None,
                    None,
                );

                break; // Only place one set of garage doors
            }
        }
    } else if config.has_single_door {
        // Place a single oak door somewhere on the wall
        // Pick a random position from the wall outline
        if !wall_outline.is_empty() {
            let door_idx = rng.gen_range(0..wall_outline.len());
            let (door_x, door_z) = wall_outline[door_idx];

            // Place single oak door
            editor.set_block_absolute(OAK_DOOR, door_x, door_y, door_z, None, None);
            editor.set_block_absolute(OAK_DOOR_UPPER, door_x, door_y + 1, door_z, None, None);
        }
    }
}

/// Determines which block to place at a specific wall position (wall, window, or accent)
#[inline]
fn determine_wall_block_at_position(bx: i32, h: i32, bz: i32, config: &BuildingConfig) -> Block {
    // If windows are disabled, always use wall block (with possible accent)
    if !config.has_windows {
        let above_floor = h > config.start_y_offset + 1;
        let use_accent_line = config.use_accent_lines && above_floor && h % 4 == 0;
        if use_accent_line {
            return config.accent_block;
        }
        return config.wall_block;
    }

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
    args: &Args,
    generate_non_flat_roof: bool,
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
                editor.set_block_absolute(block, x, h + config.abs_terrain_offset, z, None, None);
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
        // Use the resolved style flag, not just the OSM tag, since auto-gabled roofs
        // may be generated for residential buildings without a roof:shape tag
        let has_flat_roof = !args.roof || !generate_non_flat_roof;

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
fn calculate_roof_peak_height(
    bounds: &BuildingBounds,
    start_y_offset: i32,
    building_height: i32,
) -> i32 {
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
    // Early return for underground buildings
    if should_skip_underground_building(element) {
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
                let (height, _) =
                    calculate_building_height(element, min_level, scale_factor, relation_levels);
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
        if element
            .tags
            .get("parking")
            .is_some_and(|p| p == "multi-storey")
        {
            let (height, _) =
                calculate_building_height(element, min_level, scale_factor, relation_levels);
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
        category,
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
        roof_block: style.roof_block,
        use_vertical_windows: style.use_vertical_windows,
        use_accent_roof_line: style.use_accent_roof_line,
        use_accent_lines: style.use_accent_lines,
        use_vertical_accent: style.use_vertical_accent,
        is_abandoned_building,
        has_windows: style.has_windows,
        has_garage_door: style.has_garage_door,
        has_single_door: style.has_single_door,
    };

    // Generate walls - pass whether this building will have a sloped roof
    let has_sloped_roof = args.roof && style.generate_roof;
    let (wall_outline, corner_addup) =
        generate_building_walls(editor, element, &config, args, has_sloped_roof);

    // Generate special doors (garage doors, shed doors)
    if config.has_garage_door || config.has_single_door {
        generate_special_doors(editor, element, &config, &wall_outline);
    }

    // Create roof area = floor area + wall outline (so roof covers the walls too)
    let roof_area: Vec<(i32, i32)> = {
        let mut area: HashSet<(i32, i32)> = cached_floor_area.iter().copied().collect();
        area.extend(wall_outline.iter().copied());
        area.into_iter().collect()
    };

    // Generate floors and ceilings
    if corner_addup != (0, 0, 0) {
        generate_floors_and_ceilings(
            editor,
            &cached_floor_area,
            &config,
            args,
            style.generate_roof,
        );

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
        generate_building_roof(editor, element, &config, &style, &bounds, &roof_area);
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
        let roof_peak_height =
            calculate_roof_peak_height(bounds, config.start_y_offset, config.building_height);
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
#[allow(clippy::too_many_arguments)]
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
                0 => *x < center_x && *z < center_z,   // NW
                1 => *x >= center_x && *z < center_z,  // NE
                2 => *x < center_x && *z >= center_z,  // SW
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
    // Height is exactly 4 brick blocks with a slab cap on top
    let chimney_base = roof_peak_height - 2;
    let chimney_height = 4;

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
            Some(replace_any), // Empty blacklist = replace any block
        );
    }

    // Add stone brick slab cap on top
    editor.set_block_absolute(
        STONE_BRICK_SLAB,
        chimney_x,
        chimney_base + chimney_height + abs_terrain_offset,
        chimney_z,
        None,
        Some(replace_any), // Empty blacklist = replace any block
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

        // Roof base_height is always at the roof line level (top of walls + 1)
        // This ensures the roof sits on top of the building consistently
        let base_height = start_y_offset + building_height + 1;

        // 90% wall block, 10% accent block for variety (deterministic based on element ID)
        let mut rng = element_rng(element_id);
        // Advance RNG state to get different value than other style choices
        let _ = rng.gen::<u32>();
        let roof_block = if rng.gen_bool(0.1) {
            accent_block
        } else {
            wall_block
        };

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
fn has_lower_neighbor(
    x: i32,
    z: i32,
    roof_height: i32,
    roof_heights: &HashMap<(i32, i32), i32>,
) -> bool {
    [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)]
        .iter()
        .any(|(nx, nz)| {
            roof_heights
                .get(&(*nx, *nz))
                .is_some_and(|&nh| nh < roof_height)
        })
}

/// Places roof blocks for a given height map
fn place_roof_blocks_with_stairs(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    roof_heights: &HashMap<(i32, i32), i32>,
    config: &RoofConfig,
    stair_direction_fn: impl Fn(i32, i32, i32) -> BlockWithProperties,
) {
    // Use empty blacklist to allow overwriting wall/ceiling blocks
    let replace_any: &[Block] = &[];

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
                        Some(replace_any),
                    );
                } else {
                    editor.set_block_absolute(
                        config.roof_block,
                        x,
                        y + config.abs_terrain_offset,
                        z,
                        None,
                        Some(replace_any),
                    );
                }
            } else {
                editor.set_block_absolute(
                    config.roof_block,
                    x,
                    y + config.abs_terrain_offset,
                    z,
                    None,
                    Some(replace_any),
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
    // Use empty blacklist to allow overwriting wall/ceiling blocks
    let replace_any: &[Block] = &[];
    for &(x, z) in floor_area {
        editor.set_block_absolute(
            floor_block,
            x,
            base_height + abs_terrain_offset,
            z,
            None,
            Some(replace_any),
        );
    }
}

/// Generates a gabled roof
fn generate_gabled_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
    roof_orientation: Option<&str>,
) {
    // Create a HashSet for O(1) footprint lookups - this is the actual building shape
    let footprint: HashSet<(i32, i32)> = floor_area.iter().copied().collect();

    let width_is_longer = config.width() >= config.length();
    let ridge_runs_along_x = match roof_orientation {
        Some(o) if o.eq_ignore_ascii_case("along") => width_is_longer,
        Some(o) if o.eq_ignore_ascii_case("across") => !width_is_longer,
        _ => width_is_longer,
    };

    // Use the full distance from center to edge, accounting for odd sizes
    let max_distance = if ridge_runs_along_x {
        (config.max_z - config.center_z)
            .max(config.center_z - config.min_z)
            .max(1)
    } else {
        (config.max_x - config.center_x)
            .max(config.center_x - config.min_x)
            .max(1)
    };

    // Calculate roof height boost, but limit it to max_distance so the slope
    // is at most 1 block per row (creates a proper diagonal line)
    let raw_roof_height_boost = (3.0 + (config.building_size() as f64 * 0.15).ln().max(1.0)) as i32;
    let roof_height_boost = raw_roof_height_boost.min(max_distance);
    let roof_peak_height = config.base_height + roof_height_boost;

    // Calculate roof heights only for positions in the actual footprint
    let mut roof_heights: HashMap<(i32, i32), i32> = HashMap::new();
    for &(x, z) in floor_area {
        let distance_to_ridge = if ridge_runs_along_x {
            (z - config.center_z).abs()
        } else {
            (x - config.center_x).abs()
        };

        let roof_height = if distance_to_ridge == 0 {
            roof_peak_height
        } else {
            let slope_ratio = (distance_to_ridge as f64 / max_distance as f64).min(1.0);
            (roof_peak_height as f64 - (slope_ratio * roof_height_boost as f64)) as i32
        }
        .max(config.base_height);

        roof_heights.insert((x, z), roof_height);
    }

    let stair_block_material = get_stair_block_for_material(config.roof_block);
    let replace_any: &[Block] = &[];

    // Helper to determine stair facing for outer edges (faces away from building center)
    let get_outer_edge_stair = |x: i32, z: i32| -> BlockWithProperties {
        if ridge_runs_along_x {
            if !footprint.contains(&(x, z - 1)) {
                create_stair_with_properties(
                    stair_block_material,
                    StairFacing::South,
                    StairShape::Straight,
                )
            } else {
                create_stair_with_properties(
                    stair_block_material,
                    StairFacing::North,
                    StairShape::Straight,
                )
            }
        } else if !footprint.contains(&(x - 1, z)) {
            create_stair_with_properties(
                stair_block_material,
                StairFacing::East,
                StairShape::Straight,
            )
        } else {
            create_stair_with_properties(
                stair_block_material,
                StairFacing::West,
                StairShape::Straight,
            )
        }
    };

    // Helper to determine stair facing for slope (faces toward lower side)
    let get_slope_stair = |x: i32, z: i32| -> BlockWithProperties {
        if ridge_runs_along_x {
            if z < config.center_z {
                create_stair_with_properties(
                    stair_block_material,
                    StairFacing::South,
                    StairShape::Straight,
                )
            } else {
                create_stair_with_properties(
                    stair_block_material,
                    StairFacing::North,
                    StairShape::Straight,
                )
            }
        } else if x < config.center_x {
            create_stair_with_properties(
                stair_block_material,
                StairFacing::East,
                StairShape::Straight,
            )
        } else {
            create_stair_with_properties(
                stair_block_material,
                StairFacing::West,
                StairShape::Straight,
            )
        }
    };

    for &(x, z) in floor_area {
        let roof_height = roof_heights[&(x, z)];

        // Check if position is at outer edge (neighbor perpendicular to ridge is missing)
        let is_outer_edge = if ridge_runs_along_x {
            !footprint.contains(&(x, z - 1)) || !footprint.contains(&(x, z + 1))
        } else {
            !footprint.contains(&(x - 1, z)) || !footprint.contains(&(x + 1, z))
        };

        if is_outer_edge {
            // Outer edge: single stair at base_height, overwrites existing blocks
            editor.set_block_with_properties_absolute(
                get_outer_edge_stair(x, z),
                x,
                config.base_height + config.abs_terrain_offset,
                z,
                None,
                Some(replace_any),
            );
        } else {
            // Inner positions: fill from base_height to roof_height
            let has_lower_neighbor =
                [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)]
                    .iter()
                    .any(|&(nx, nz)| {
                        roof_heights
                            .get(&(nx, nz))
                            .is_some_and(|&nh| nh < roof_height)
                    });

            for y in config.base_height..=roof_height {
                if y == roof_height && has_lower_neighbor {
                    editor.set_block_with_properties_absolute(
                        get_slope_stair(x, z),
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
            }
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
        let max_dist_to_edge = if ridge_axis_is_x {
            half_length
        } else {
            half_width
        };

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
            create_stair_with_properties(
                stair_block_material,
                StairFacing::East,
                StairShape::Straight,
            )
        } else if dist_from_max_x == min_dist {
            // Closest to east edge, stair faces west
            create_stair_with_properties(
                stair_block_material,
                StairFacing::West,
                StairShape::Straight,
            )
        } else if dist_from_min_z == min_dist {
            // Closest to north edge, stair faces south
            create_stair_with_properties(
                stair_block_material,
                StairFacing::South,
                StairShape::Straight,
            )
        } else {
            // Closest to south edge, stair faces north
            create_stair_with_properties(
                stair_block_material,
                StairFacing::North,
                StairShape::Straight,
            )
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
            ((config.min_x - config.center_x).pow(2) + (config.min_z - config.center_z).pow(2))
                as f64,
            ((config.min_x - config.center_x).pow(2) + (config.max_z - config.center_z).pow(2))
                as f64,
            ((config.max_x - config.center_x).pow(2) + (config.min_z - config.center_z).pow(2))
                as f64,
            ((config.max_x - config.center_x).pow(2) + (config.max_z - config.center_z).pow(2))
                as f64,
        ];
        corner_distances
            .iter()
            .fold(0.0f64, |a, &b| a.max(b))
            .sqrt()
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
                create_stair_with_properties(
                    stair_block_material,
                    StairFacing::West,
                    StairShape::Straight,
                )
            } else {
                create_stair_with_properties(
                    stair_block_material,
                    StairFacing::East,
                    StairShape::Straight,
                )
            }
        } else if center_dz > 0 {
            create_stair_with_properties(
                stair_block_material,
                StairFacing::North,
                StairShape::Straight,
            )
        } else {
            create_stair_with_properties(
                stair_block_material,
                StairFacing::South,
                StairShape::Straight,
            )
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
        create_stair_with_properties(
            stair_block_material,
            StairFacing::East,
            StairShape::Straight,
        )
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

        let roof_height =
            config.base_height + (height_factor * (peak_height - config.base_height) as f64) as i32;
        roof_heights.insert((x, z), roof_height);
    }

    // Place blocks with complex stair logic for pyramid corners
    // Use empty blacklist to allow overwriting wall/ceiling blocks
    let replace_any: &[Block] = &[];
    let stair_block_material = get_stair_block_for_material(config.roof_block);

    for &(x, z) in floor_area {
        let roof_height = roof_heights[&(x, z)];

        for y in config.base_height..=roof_height {
            if y == roof_height {
                let stair_block = determine_pyramidal_stair_block(
                    x,
                    z,
                    roof_height,
                    &roof_heights,
                    config,
                    stair_block_material,
                );
                editor.set_block_with_properties_absolute(
                    stair_block,
                    x,
                    y + config.abs_terrain_offset,
                    z,
                    None,
                    Some(replace_any),
                );
            } else {
                editor.set_block_absolute(
                    config.roof_block,
                    x,
                    y + config.abs_terrain_offset,
                    z,
                    None,
                    Some(replace_any),
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

    let north_height = roof_heights
        .get(&(x, z - 1))
        .copied()
        .unwrap_or(config.base_height);
    let south_height = roof_heights
        .get(&(x, z + 1))
        .copied()
        .unwrap_or(config.base_height);
    let west_height = roof_heights
        .get(&(x - 1, z))
        .copied()
        .unwrap_or(config.base_height);
    let east_height = roof_heights
        .get(&(x + 1, z))
        .copied()
        .unwrap_or(config.base_height);

    let has_lower_north = north_height < roof_height;
    let has_lower_south = south_height < roof_height;
    let has_lower_west = west_height < roof_height;
    let has_lower_east = east_height < roof_height;

    // Corner situations
    if has_lower_north && has_lower_west {
        create_stair_with_properties(
            stair_block_material,
            StairFacing::East,
            StairShape::OuterRight,
        )
    } else if has_lower_north && has_lower_east {
        create_stair_with_properties(
            stair_block_material,
            StairFacing::South,
            StairShape::OuterRight,
        )
    } else if has_lower_south && has_lower_west {
        create_stair_with_properties(
            stair_block_material,
            StairFacing::East,
            StairShape::OuterLeft,
        )
    } else if has_lower_south && has_lower_east {
        create_stair_with_properties(
            stair_block_material,
            StairFacing::North,
            StairShape::OuterLeft,
        )
    } else if dx.abs() > dz.abs() {
        // Primary slope in X direction
        if dx > 0 && east_height < roof_height {
            create_stair_with_properties(
                stair_block_material,
                StairFacing::West,
                StairShape::Straight,
            )
        } else if dx < 0 && west_height < roof_height {
            create_stair_with_properties(
                stair_block_material,
                StairFacing::East,
                StairShape::Straight,
            )
        } else if dz > 0 && south_height < roof_height {
            create_stair_with_properties(
                stair_block_material,
                StairFacing::North,
                StairShape::Straight,
            )
        } else if dz < 0 && north_height < roof_height {
            create_stair_with_properties(
                stair_block_material,
                StairFacing::South,
                StairShape::Straight,
            )
        } else {
            BlockWithProperties::simple(config.roof_block)
        }
    } else {
        // Primary slope in Z direction
        if dz > 0 && south_height < roof_height {
            create_stair_with_properties(
                stair_block_material,
                StairFacing::North,
                StairShape::Straight,
            )
        } else if dz < 0 && north_height < roof_height {
            create_stair_with_properties(
                stair_block_material,
                StairFacing::South,
                StairShape::Straight,
            )
        } else if dx > 0 && east_height < roof_height {
            create_stair_with_properties(
                stair_block_material,
                StairFacing::West,
                StairShape::Straight,
            )
        } else if dx < 0 && west_height < roof_height {
            create_stair_with_properties(
                stair_block_material,
                StairFacing::East,
                StairShape::Straight,
            )
        } else {
            BlockWithProperties::simple(config.roof_block)
        }
    }
}

/// Generates a dome roof
fn generate_dome_roof(editor: &mut WorldEditor, floor_area: &[(i32, i32)], config: &RoofConfig) {
    let radius = (config.building_size() / 2) as f64;
    // Use empty blacklist to allow overwriting wall/ceiling blocks
    let replace_any: &[Block] = &[];

    for &(x, z) in floor_area {
        let distance_from_center =
            ((x - config.center_x).pow(2) + (z - config.center_z).pow(2)) as f64;
        let normalized_distance = (distance_from_center.sqrt() / radius).min(1.0);

        let height_factor = (1.0 - normalized_distance * normalized_distance).sqrt();
        let surface_height = config.base_height + (height_factor * (radius * 0.8)) as i32;

        for y in config.base_height..=surface_height {
            editor.set_block_absolute(
                config.roof_block,
                x,
                y + config.abs_terrain_offset,
                z,
                None,
                Some(replace_any),
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
            generate_flat_roof(
                editor,
                roof_area,
                floor_block,
                config.base_height,
                abs_terrain_offset,
            );
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
            let roof_peak_height =
                config.base_height + if config.building_size() > 20 { 7 } else { 5 };

            if is_rectangular {
                generate_hipped_roof_rectangular(
                    editor,
                    roof_area,
                    &config,
                    ridge_axis_is_x,
                    roof_peak_height,
                );
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
    // Skip underground buildings/building parts
    // Check layer tag
    if let Some(layer) = relation.tags.get("layer") {
        if layer.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }
    // Check level tag
    if let Some(level) = relation.tags.get("level") {
        if level.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }
    // Check location tag
    if let Some(location) = relation.tags.get("location") {
        if location == "underground" || location == "subway" {
            return;
        }
    }
    // Check building:levels:underground without building:levels
    if relation.tags.contains_key("building:levels:underground")
        && !relation.tags.contains_key("building:levels")
    {
        return;
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
