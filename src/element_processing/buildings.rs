use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::clipping::clip_way_to_bbox;
use crate::colors::color_text_to_rgb_tuple;
use crate::coordinate_system::cartesian::XZPoint;
use crate::deterministic_rng::{coord_rng, element_rng};
use crate::element_processing::historic;
use crate::element_processing::subprocessor::buildings_interior::generate_building_interior;
use crate::floodfill_cache::{CoordinateBitmap, FloodFillCache};
use crate::osm_parser::{ProcessedMemberRole, ProcessedNode, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;
use fastnbt::Value;
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

/// Enum representing different wall depth styles for building facades.
/// Each style creates visual depth by placing blocks outward from the wall
/// plane, making windows appear recessed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WallDepthStyle {
    None,               // No depth features (sheds, greenhouses, tiny buildings)
    SubtlePilasters,    // Thin columns between windows (residential, houses)
    ModernPillars,      // Clean paired columns + horizontal bands (commercial, office, hotel)
    InstitutionalBands, // Columns + stair ledges at floor lines (school, hospital)
    IndustrialBeams,    // Corner pillars only (industrial, warehouse)
    HistoricOrnate,     // Stone columns + arched window tops + cornice (historic)
    ReligiousButtress,  // Stepped buttresses + cornice (religious)
    SkyscraperFins,     // Full-height vertical fins (tall building, modern skyscraper)
    GlassCurtain,       // Minimal corner definition only (glassy skyscraper)
}

#[derive(Clone)]
pub(crate) struct HolePolygon {
    way: ProcessedWay,
    add_walls: bool,
}

// ============================================================================
// Building Style System
// ============================================================================

/// Height (in blocks above ground floor) of a building-passage archway.
/// Walls and floors below this height are removed at tunnel=building_passage
/// highway coordinates, creating a ground-level opening through the building.
pub(crate) const BUILDING_PASSAGE_HEIGHT: i32 = 4;

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
    POLISHED_BLACKSTONE_BRICKS,
    DEEPSLATE_BRICKS,
    POLISHED_ANDESITE,
    ANDESITE,
    SMOOTH_STONE,
    BRICK,
];

/// Wall blocks for garages (sturdy, simple, varied)
const GARAGE_WALL_OPTIONS: [Block; 6] = [
    BRICK,
    STONE_BRICKS,
    POLISHED_ANDESITE,
    COBBLESTONE,
    SMOOTH_STONE,
    LIGHT_GRAY_CONCRETE,
];

/// Wall blocks for sheds (wooden)
const SHED_WALL_OPTIONS: [Block; 1] = [OAK_LOG];

/// Wall blocks for greenhouses (glass variants)
const GREENHOUSE_WALL_OPTIONS: [Block; 4] = [
    GLASS,
    CYAN_STAINED_GLASS,
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
    TallBuilding,     // Tall buildings (>7 floors or >28m)
    GlassySkyscraper, // Glass-facade skyscrapers (50% of true skyscrapers)
    ModernSkyscraper, // Horizontal-window skyscrapers with stone bands (35%)
    Historic,         // Castles, ruins, historic buildings
    Tower,            // man_made=tower or building=tower (stone towers)
    Garage,           // Garages and carports
    Shed,             // Sheds, huts, simple storage
    Greenhouse,       // Greenhouses and glasshouses

    Default, // Unknown or generic buildings
}

impl BuildingCategory {
    /// Determines the building category from OSM tags and calculated properties
    fn from_element(element: &ProcessedWay, is_tall_building: bool, building_height: i32) -> Self {
        // Check for man_made=tower before anything else
        if element.tags.get("man_made").map(|s| s.as_str()) == Some("tower") {
            return BuildingCategory::Tower;
        }

        if is_tall_building {
            // Check if this qualifies as a true skyscraper:
            // Must be significantly tall AND have skyscraper proportions
            // (taller than twice its longest side dimension)
            let is_true_skyscraper = building_height >= 160
                && Self::has_skyscraper_proportions(element, building_height);

            if is_true_skyscraper {
                // Deterministic variant selection based on element ID
                let hash = element.id.wrapping_mul(2654435761); // Knuth multiplicative hash
                let roll = hash % 100;
                return if roll < 50 {
                    BuildingCategory::GlassySkyscraper
                } else if roll < 85 {
                    BuildingCategory::ModernSkyscraper
                } else {
                    // 15% use the standard TallBuilding preset
                    BuildingCategory::TallBuilding
                };
            }

            return BuildingCategory::TallBuilding;
        }

        // Check for religious buildings BEFORE the generic historic check.
        // A church/mosque/temple that also carries a heritage tag should still
        // be styled as Religious — its function defines its architecture.
        let building_type = element
            .tags
            .get("building")
            .or_else(|| element.tags.get("building:part"))
            .map(|s| s.as_str())
            .unwrap_or("yes");

        let is_religious_building = matches!(
            building_type,
            "religious" | "church" | "cathedral" | "chapel" | "mosque" | "synagogue" | "temple"
        );
        let is_religious_amenity =
            element.tags.get("amenity").map(|s| s.as_str()) == Some("place_of_worship");

        if is_religious_building || is_religious_amenity {
            return BuildingCategory::Religious;
        }

        // Check for historic buildings (only after ruling out religious ones)
        if element.tags.contains_key("historic") {
            return BuildingCategory::Historic;
        }

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

            // Towers
            "tower" | "clock_tower" | "transformer_tower" => BuildingCategory::Tower,

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

    /// Checks if a tall building has skyscraper proportions:
    /// building height >= 40 blocks AND height >= 2× the longest side of its bounding box.
    fn has_skyscraper_proportions(element: &ProcessedWay, building_height: i32) -> bool {
        if building_height < 40 {
            return false;
        }

        if element.nodes.len() < 3 {
            return false;
        }

        let min_x = element.nodes.iter().map(|n| n.x).min().unwrap_or(0);
        let max_x = element.nodes.iter().map(|n| n.x).max().unwrap_or(0);
        let min_z = element.nodes.iter().map(|n| n.z).min().unwrap_or(0);
        let max_z = element.nodes.iter().map(|n| n.z).max().unwrap_or(0);

        let longest_side = (max_x - min_x).max(max_z - min_z).max(1);
        building_height as f64 / longest_side as f64 >= 2.0
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
    pub use_horizontal_windows: Option<bool>, // Full-width horizontal window bands (modern skyscrapers)
    pub has_windows: Option<bool>,            // Whether to generate windows at all

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

    // Wall depth
    pub wall_depth_style: Option<WallDepthStyle>,
    pub has_parapet: Option<bool>, // Whether flat-roofed buildings get a parapet wall
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
            wall_depth_style: Some(WallDepthStyle::SubtlePilasters),
            ..Default::default()
        }
    }

    /// Preset for tall buildings (>7 floors, not true skyscrapers)
    pub fn tall_building() -> Self {
        Self {
            use_vertical_windows: Some(true), // Always vertical windows
            roof_type: Some(RoofType::Flat),  // Always flat roof
            has_chimney: Some(false),         // No chimneys on tall buildings
            use_accent_roof_line: Some(true), // Usually have accent roof line
            wall_depth_style: Some(WallDepthStyle::SkyscraperFins),
            has_parapet: Some(true),
            ..Default::default()
        }
    }

    /// Preset for modern skyscrapers with horizontal window bands
    pub fn modern_skyscraper() -> Self {
        Self {
            roof_type: Some(RoofType::Flat),
            generate_roof: Some(true),
            has_chimney: Some(false),
            use_accent_roof_line: Some(true),
            use_vertical_accent: Some(false),
            wall_depth_style: Some(WallDepthStyle::SkyscraperFins),
            has_parapet: Some(true),
            // has_windows, use_accent_lines, and use_horizontal_windows
            // are resolved in BuildingStyle::resolve() with category-specific logic
            ..Default::default()
        }
    }

    /// Preset for glass-facade skyscrapers
    pub fn glassy_skyscraper() -> Self {
        Self {
            has_windows: Some(false), // The wall IS glass, no extra windows
            use_vertical_accent: Some(false),
            use_accent_roof_line: Some(true),
            roof_type: Some(RoofType::Flat), // Always flat roof
            generate_roof: Some(true),       // Generate the flat cap
            has_chimney: Some(false),
            wall_depth_style: Some(WallDepthStyle::GlassCurtain),
            has_parapet: Some(true),
            // accent_lines, accent_block and floor_block are resolved randomly in resolve()
            // with GlassySkyscraper-specific palettes
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
            wall_depth_style: Some(WallDepthStyle::IndustrialBeams),
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
            wall_depth_style: Some(WallDepthStyle::HistoricOrnate),
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
            wall_depth_style: Some(WallDepthStyle::SubtlePilasters),
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
            wall_depth_style: Some(WallDepthStyle::None),
            ..Default::default()
        }
    }

    /// Preset for office buildings
    pub fn office() -> Self {
        Self {
            use_vertical_windows: Some(true), // Office buildings typically have vertical windows
            use_accent_roof_line: Some(true),
            has_chimney: Some(false),
            wall_depth_style: Some(WallDepthStyle::ModernPillars),
            has_parapet: Some(true),
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
            wall_depth_style: Some(WallDepthStyle::ModernPillars),
            has_parapet: Some(true),
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
            wall_depth_style: Some(WallDepthStyle::IndustrialBeams),
            ..Default::default()
        }
    }

    /// Preset for schools and educational buildings
    pub fn school() -> Self {
        Self {
            use_vertical_windows: Some(false), // Schools usually have regular windows
            use_accent_roof_line: Some(true),
            has_chimney: Some(false),
            wall_depth_style: Some(WallDepthStyle::InstitutionalBands),
            has_parapet: Some(true),
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
            wall_depth_style: Some(WallDepthStyle::InstitutionalBands),
            has_parapet: Some(true),
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
            wall_depth_style: Some(WallDepthStyle::ReligiousButtress),
            ..Default::default()
        }
    }

    /// Preset for towers (man_made=tower) — stone walls with accent banding
    /// and glass windows for a clean historic look.
    pub fn tower() -> Self {
        Self {
            has_windows: Some(true),
            window_block: Some(GLASS),
            use_accent_lines: Some(true),
            use_vertical_accent: Some(false),
            use_accent_roof_line: Some(true),
            roof_type: Some(RoofType::Flat),
            generate_roof: Some(true),
            has_chimney: Some(false),
            wall_depth_style: Some(WallDepthStyle::None),
            ..Default::default()
        }
    }

    /// Preset for garages and carports
    pub fn garage() -> Self {
        Self {
            roof_type: Some(RoofType::Flat),
            has_chimney: Some(false),
            use_accent_lines: Some(false),
            use_vertical_accent: Some(false),
            use_accent_roof_line: Some(true), // Accent band at roofline for visual interest
            generate_roof: Some(true),
            has_windows: Some(false),    // No windows on garages
            has_garage_door: Some(true), // Generate double door on front
            wall_depth_style: Some(WallDepthStyle::None),
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
            wall_depth_style: Some(WallDepthStyle::None),
            ..Default::default()
        }
    }

    /// Preset for greenhouses
    pub fn greenhouse() -> Self {
        Self {
            // Wall block is randomly chosen from GREENHOUSE_WALL_OPTIONS
            roof_block: Some(SMOOTH_STONE_SLAB), // Smooth stone slab roof
            has_chimney: Some(false),
            use_accent_lines: Some(false),
            use_vertical_accent: Some(false),
            use_accent_roof_line: Some(false),
            roof_type: Some(RoofType::Flat),
            generate_roof: Some(true),
            has_windows: Some(false),    // The walls themselves are glass
            has_single_door: Some(true), // One entrance door
            wall_depth_style: Some(WallDepthStyle::None),
            ..Default::default()
        }
    }

    /// Preset for commercial buildings (retail, shops)
    pub fn commercial() -> Self {
        Self {
            use_vertical_windows: Some(false),
            use_accent_roof_line: Some(true),
            wall_depth_style: Some(WallDepthStyle::ModernPillars),
            has_parapet: Some(true),
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
            BuildingCategory::Tower => Self::tower(),
            BuildingCategory::Garage => Self::garage(),
            BuildingCategory::Shed => Self::shed(),
            BuildingCategory::Greenhouse => Self::greenhouse(),
            BuildingCategory::TallBuilding => Self::tall_building(),
            BuildingCategory::GlassySkyscraper => Self::glassy_skyscraper(),
            BuildingCategory::ModernSkyscraper => Self::modern_skyscraper(),
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
    pub use_horizontal_windows: bool, // Full-width horizontal window bands
    pub has_windows: bool,            // Whether to generate windows

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

    // Wall depth
    pub wall_depth_style: WallDepthStyle,
    pub has_parapet: bool,
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
    #[allow(clippy::too_many_arguments, clippy::unnecessary_lazy_evaluations)]
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
        // For glassy/modern skyscrapers, use dark cap materials for the flat roof
        let floor_block = preset.floor_block.unwrap_or_else(|| {
            if matches!(
                category,
                BuildingCategory::GlassySkyscraper | BuildingCategory::ModernSkyscraper
            ) {
                const SKYSCRAPER_ROOF_CAP_OPTIONS: [Block; 3] =
                    [POLISHED_ANDESITE, BLACKSTONE, NETHER_BRICK];
                SKYSCRAPER_ROOF_CAP_OPTIONS[rng.random_range(0..SKYSCRAPER_ROOF_CAP_OPTIONS.len())]
            } else {
                get_floor_block_with_rng(rng)
            }
        });

        // Window block: from preset or random based on building type
        let window_block = preset
            .window_block
            .unwrap_or_else(|| get_window_block_for_building_type_with_rng(building_type, rng));

        // Accent block: from preset or random
        // For glassy skyscrapers, use white stained glass or blackstone
        // For modern skyscrapers, use stone separation band materials
        let accent_block = preset.accent_block.unwrap_or_else(|| {
            if category == BuildingCategory::GlassySkyscraper {
                const GLASSY_ACCENT_OPTIONS: [Block; 2] = [WHITE_STAINED_GLASS, BLACKSTONE];
                GLASSY_ACCENT_OPTIONS[rng.random_range(0..GLASSY_ACCENT_OPTIONS.len())]
            } else if category == BuildingCategory::ModernSkyscraper {
                const MODERN_ACCENT_OPTIONS: [Block; 5] = [
                    POLISHED_ANDESITE,
                    SMOOTH_STONE,
                    BLACKSTONE,
                    NETHER_BRICK,
                    STONE_BRICKS,
                ];
                MODERN_ACCENT_OPTIONS[rng.random_range(0..MODERN_ACCENT_OPTIONS.len())]
            } else {
                ACCENT_BLOCK_OPTIONS[rng.random_range(0..ACCENT_BLOCK_OPTIONS.len())]
            }
        });

        // === Window Style ===

        let use_vertical_windows = preset
            .use_vertical_windows
            .unwrap_or_else(|| rng.random_bool(0.7));

        // Horizontal windows: full-width bands, used by modern skyscrapers
        let use_horizontal_windows = preset
            .use_horizontal_windows
            .unwrap_or_else(|| category == BuildingCategory::ModernSkyscraper);

        // === Accent Features ===

        let use_accent_roof_line = preset
            .use_accent_roof_line
            .unwrap_or_else(|| rng.random_bool(0.25));

        // Accent lines only for multi-floor buildings
        // Glassy skyscrapers get 60% chance, Modern skyscrapers always have them
        let use_accent_lines = preset.use_accent_lines.unwrap_or_else(|| {
            if category == BuildingCategory::ModernSkyscraper {
                true // Stone bands always present on modern skyscrapers
            } else if category == BuildingCategory::GlassySkyscraper {
                rng.random_bool(0.6)
            } else {
                has_multiple_floors && rng.random_bool(0.2)
            }
        });

        // Vertical accent: only if no accent lines and multi-floor
        let use_vertical_accent = preset
            .use_vertical_accent
            .unwrap_or_else(|| has_multiple_floors && !use_accent_lines && rng.random_bool(0.1));

        // === Roof ===

        // Determine roof type from preset, tags, or auto-generation.
        // An explicit roof:shape OSM tag ALWAYS takes priority over preset defaults,
        // since the mapper knows the actual shape of the building.
        let (roof_type, generate_roof) = if let Some(roof_shape) = element.tags.get("roof:shape") {
            // OSM tag always wins — the mapper explicitly specified the roof shape
            (parse_roof_type(roof_shape), true)
        } else if let Some(rt) = preset.roof_type {
            // Preset default (used when no OSM tag is present)
            let should_generate = preset.generate_roof.unwrap_or(rt != RoofType::Flat);
            (rt, should_generate)
        } else if qualifies_for_auto_gabled_roof(building_type) {
            // Auto-generate gabled roof for residential buildings
            const MAX_FOOTPRINT_FOR_GABLED: usize = 800;
            if footprint_size <= MAX_FOOTPRINT_FOR_GABLED && rng.random_bool(0.9) {
                (RoofType::Gabled, true)
            } else {
                (RoofType::Flat, false)
            }
        } else {
            (RoofType::Flat, false)
        };

        // For diagonal buildings without an explicit roof:shape tag,
        // switch from gabled/hipped to pyramidal.  With polygon-edge
        // scanning, gabled roofs now work on moderately rotated buildings,
        // so only very diagonal shapes (ratio < 0.35) are downgraded.
        // If the mapper explicitly tagged a roof shape, always respect it.
        let has_explicit_roof_shape = element.tags.contains_key("roof:shape");
        const DIAGONAL_THRESHOLD: f64 = 0.35;
        let diagonality = compute_building_diagonality(&element.nodes);
        let roof_type = if !has_explicit_roof_shape
            && matches!(roof_type, RoofType::Gabled | RoofType::Hipped)
            && diagonality < DIAGONAL_THRESHOLD
        {
            RoofType::Pyramidal
        } else {
            roof_type
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

            is_residential && suitable_roof && suitable_size && rng.random_bool(0.55)
        });

        // Roof block: specific material for roofs
        let roof_block = preset.roof_block;

        // Windows: default to true unless explicitly disabled
        let has_windows = preset.has_windows.unwrap_or(true);

        // Special door features
        let has_garage_door = preset.has_garage_door.unwrap_or(false);
        let has_single_door = preset.has_single_door.unwrap_or(false);

        // Wall depth style: default based on category (preset may override)
        let wall_depth_style = preset.wall_depth_style.unwrap_or_else(|| {
            if footprint_size < 20 {
                WallDepthStyle::None
            } else {
                match category {
                    BuildingCategory::House | BuildingCategory::Residential => {
                        WallDepthStyle::SubtlePilasters
                    }
                    BuildingCategory::Commercial
                    | BuildingCategory::Office
                    | BuildingCategory::Hotel => WallDepthStyle::ModernPillars,
                    BuildingCategory::School | BuildingCategory::Hospital => {
                        WallDepthStyle::InstitutionalBands
                    }
                    BuildingCategory::Industrial | BuildingCategory::Warehouse => {
                        WallDepthStyle::IndustrialBeams
                    }
                    BuildingCategory::Historic => WallDepthStyle::HistoricOrnate,
                    BuildingCategory::Religious => WallDepthStyle::ReligiousButtress,
                    BuildingCategory::TallBuilding | BuildingCategory::ModernSkyscraper => {
                        WallDepthStyle::SkyscraperFins
                    }
                    BuildingCategory::GlassySkyscraper => WallDepthStyle::GlassCurtain,
                    _ => WallDepthStyle::None,
                }
            }
        });

        // Parapet: flat-roofed multi-floor non-residential buildings
        let has_parapet = preset.has_parapet.unwrap_or_else(|| {
            let is_flat = roof_type == RoofType::Flat;
            let suitable = matches!(
                category,
                BuildingCategory::Commercial
                    | BuildingCategory::Office
                    | BuildingCategory::Hotel
                    | BuildingCategory::School
                    | BuildingCategory::Hospital
                    | BuildingCategory::TallBuilding
                    | BuildingCategory::GlassySkyscraper
                    | BuildingCategory::ModernSkyscraper
            );
            is_flat && has_multiple_floors && suitable
        });

        Self {
            wall_block,
            floor_block,
            window_block,
            accent_block,
            roof_block,
            use_vertical_windows,
            use_horizontal_windows,
            has_windows,
            use_accent_roof_line,
            use_accent_lines,
            use_vertical_accent,
            roof_type,
            has_chimney,
            generate_roof,
            has_garage_door,
            has_single_door,
            wall_depth_style,
            has_parapet,
        }
    }
}

/// Building configuration derived from OSM tags and args
struct BuildingConfig {
    /// True when the building starts at ground level (no min_height / min_level offset).
    /// When false, foundation pillars should not be generated.
    is_ground_level: bool,
    building_height: i32,
    is_tall_building: bool,
    start_y_offset: i32,
    abs_terrain_offset: i32,
    wall_block: Block,
    floor_block: Block,
    window_block: Block,
    accent_block: Block,
    roof_block: Option<Block>,
    use_vertical_windows: bool,
    use_horizontal_windows: bool,
    use_accent_roof_line: bool,
    use_accent_lines: bool,
    use_vertical_accent: bool,
    is_abandoned_building: bool,
    has_windows: bool,
    has_garage_door: bool,
    has_single_door: bool,
    category: BuildingCategory,
    wall_depth_style: WallDepthStyle,
    has_parapet: bool,
    has_lobby_base: bool,
}

impl BuildingConfig {
    /// Returns the position within a 4-block floor cycle (0 = floor row, 1-3 = open rows).
    /// This aligns with `generate_floors_and_ceilings` which places intermediate ceilings
    /// at `start_y_offset + 6, +10, +14, …` (i.e. every 4 blocks offset by +2).
    #[inline]
    fn floor_row(&self, h: i32) -> i32 {
        ((h - self.start_y_offset - 2) % 4 + 4) % 4
    }
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

    // Check building:levels:underground, if this is the only levels tag, it's underground
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

    // Try to get wall block from building:colour tag first.
    // Skip for GlassySkyscraper: its wall MUST be glass (has_windows=false relies on this).
    if category != BuildingCategory::GlassySkyscraper {
        if let Some(building_colour) = element.tags.get("building:colour") {
            if let Some(rgb) = color_text_to_rgb_tuple(building_colour) {
                return get_building_wall_block_for_color(rgb);
            }
        }
    }

    // Otherwise, select from category-specific palette
    get_wall_block_for_category(category, rng)
}

/// Selects a wall block from the appropriate category palette
fn get_wall_block_for_category(category: BuildingCategory, rng: &mut impl Rng) -> Block {
    match category {
        BuildingCategory::House | BuildingCategory::Residential => {
            RESIDENTIAL_WALL_OPTIONS[rng.random_range(0..RESIDENTIAL_WALL_OPTIONS.len())]
        }
        BuildingCategory::Commercial | BuildingCategory::Office | BuildingCategory::Hotel => {
            COMMERCIAL_WALL_OPTIONS[rng.random_range(0..COMMERCIAL_WALL_OPTIONS.len())]
        }
        BuildingCategory::Industrial | BuildingCategory::Warehouse => {
            INDUSTRIAL_WALL_OPTIONS[rng.random_range(0..INDUSTRIAL_WALL_OPTIONS.len())]
        }
        BuildingCategory::Religious => {
            RELIGIOUS_WALL_OPTIONS[rng.random_range(0..RELIGIOUS_WALL_OPTIONS.len())]
        }
        BuildingCategory::School | BuildingCategory::Hospital => {
            INSTITUTIONAL_WALL_OPTIONS[rng.random_range(0..INSTITUTIONAL_WALL_OPTIONS.len())]
        }
        BuildingCategory::Farm => FARM_WALL_OPTIONS[rng.random_range(0..FARM_WALL_OPTIONS.len())],
        BuildingCategory::Historic => {
            HISTORIC_WALL_OPTIONS[rng.random_range(0..HISTORIC_WALL_OPTIONS.len())]
        }
        BuildingCategory::Garage => {
            GARAGE_WALL_OPTIONS[rng.random_range(0..GARAGE_WALL_OPTIONS.len())]
        }
        BuildingCategory::Shed => SHED_WALL_OPTIONS[rng.random_range(0..SHED_WALL_OPTIONS.len())],
        BuildingCategory::Tower => {
            const TOWER_WALL_OPTIONS: [Block; 8] = [
                STONE_BRICKS,
                COBBLESTONE,
                CRACKED_STONE_BRICKS,
                BRICK,
                POLISHED_ANDESITE,
                ANDESITE,
                DEEPSLATE_BRICKS,
                SMOOTH_STONE,
            ];
            TOWER_WALL_OPTIONS[rng.random_range(0..TOWER_WALL_OPTIONS.len())]
        }
        BuildingCategory::Greenhouse => {
            GREENHOUSE_WALL_OPTIONS[rng.random_range(0..GREENHOUSE_WALL_OPTIONS.len())]
        }
        BuildingCategory::TallBuilding => {
            // Tall buildings use commercial palette (glass, concrete, stone)
            COMMERCIAL_WALL_OPTIONS[rng.random_range(0..COMMERCIAL_WALL_OPTIONS.len())]
        }
        BuildingCategory::ModernSkyscraper => {
            // Modern skyscrapers use clean concrete/stone wall materials
            const MODERN_SKYSCRAPER_WALL_OPTIONS: [Block; 6] = [
                GRAY_CONCRETE,
                LIGHT_GRAY_CONCRETE,
                WHITE_CONCRETE,
                POLISHED_ANDESITE,
                SMOOTH_STONE,
                QUARTZ_BLOCK,
            ];
            MODERN_SKYSCRAPER_WALL_OPTIONS
                [rng.random_range(0..MODERN_SKYSCRAPER_WALL_OPTIONS.len())]
        }
        BuildingCategory::GlassySkyscraper => {
            // Glass-facade skyscrapers use stained glass as wall material
            const GLASSY_WALL_OPTIONS: [Block; 4] = [
                GRAY_STAINED_GLASS,
                CYAN_STAINED_GLASS,
                BLUE_STAINED_GLASS,
                LIGHT_BLUE_STAINED_GLASS,
            ];
            GLASSY_WALL_OPTIONS[rng.random_range(0..GLASSY_WALL_OPTIONS.len())]
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
    // Default: 2 floors (ground + 1 upper) = 2*4+2 = 10 blocks
    let default_height = ((10.0 * scale_factor) as i32).max(3);
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

    // From height tag (overrides levels).
    // When min_height is also present, the wall height is height − min_height
    // (OSM `height` is absolute from ground, not relative to min_height).
    if let Some(height_str) = element.tags.get("height") {
        if let Ok(height) = height_str.trim_end_matches("m").trim().parse::<f64>() {
            let effective = if let Some(mh_str) = element.tags.get("min_height") {
                let mh = mh_str
                    .trim_end_matches('m')
                    .trim()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                (height - mh).max(1.0)
            } else {
                height
            };
            building_height = (effective * scale_factor) as i32;
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
    let default_height = ((10.0 * scale_factor) as i32).max(3);
    match building_type {
        "garage" | "garages" | "carport" | "shed" => ((2.0 * scale_factor) as i32).max(3),
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
    args: &Args,
) {
    let scale_factor = args.scale;
    let abs_terrain_offset = if !args.terrain { args.ground_level } else { 0 };

    // Determine where the roof structure starts vertically.
    // Priority: min_height → building:min_level → layer hint → default.
    let min_level_offset = if let Some(mh) = element.tags.get("min_height") {
        // min_height is in meters; convert via scale factor.
        mh.trim_end_matches('m')
            .trim()
            .parse::<f64>()
            .ok()
            .map(|h| (h * scale_factor) as i32)
            .unwrap_or(0)
    } else if let Some(ml) = element.tags.get("building:min_level") {
        ml.parse::<i32>()
            .ok()
            .map(|l| multiply_scale(l * 4, scale_factor))
            .unwrap_or(0)
    } else if let Some(layer) = element.tags.get("layer") {
        // For building:part=roof elements without explicit height tags, interpret
        // the layer tag as a coarse vertical-placement hint.  Each layer maps to
        // 4 blocks, producing reasonable stacking for multi-shell roof structures.
        layer
            .parse::<i32>()
            .ok()
            .filter(|&l| l > 0)
            .map(|l| multiply_scale(l * 4, scale_factor))
            .unwrap_or(0)
    } else {
        0
    };

    let start_y_offset = calculate_start_y_offset(editor, element, args, min_level_offset);

    // Determine roof thickness / height.
    let roof_thickness: i32 = if let Some(h) = element.tags.get("height") {
        let total = h
            .trim_end_matches('m')
            .trim()
            .parse::<f64>()
            .ok()
            .map(|v| (v * scale_factor) as i32)
            .unwrap_or(5);
        // If we already applied a min_height offset, the thickness is just
        // the difference.  Otherwise keep the parsed value.
        if element.tags.contains_key("min_height") {
            (total - min_level_offset).max(3)
        } else {
            total.max(3)
        }
    } else if let Some(levels) = element.tags.get("building:levels") {
        levels
            .parse::<i32>()
            .ok()
            .map(|l| multiply_scale(l * 4 + 2, scale_factor).max(3))
            .unwrap_or(5)
    } else {
        5 // Default thickness for thin roof / canopy structures
    };

    // Pick a block for the roof surface.
    let roof_block = if element
        .tags
        .get("material")
        .or_else(|| element.tags.get("roof:material"))
        .map(|s| s.as_str())
        == Some("glass")
    {
        GLASS
    } else if element.tags.get("colour").map(|s| s.as_str()) == Some("white")
        || element.tags.get("building:colour").map(|s| s.as_str()) == Some("white")
    {
        SMOOTH_QUARTZ
    } else {
        STONE_BRICK_SLAB
    };

    // Determine the roof shape from tags.
    let roof_type = element
        .tags
        .get("roof:shape")
        .map(|s| parse_roof_type(s))
        .unwrap_or(RoofType::Flat);

    match roof_type {
        RoofType::Dome | RoofType::Hipped | RoofType::Pyramidal => {
            // Standalone roof parts with curved or sloped shapes are rendered
            // as domes.  Without supporting walls, the dome approximation
            // produces the best visual result for shell-like roof structures.
            if !cached_floor_area.is_empty() {
                let (min_x, max_x, min_z, max_z) = cached_floor_area.iter().fold(
                    (i32::MAX, i32::MIN, i32::MAX, i32::MIN),
                    |(min_x, max_x, min_z, max_z), &(x, z)| {
                        (min_x.min(x), max_x.max(x), min_z.min(z), max_z.max(z))
                    },
                );
                // For roof-only structures, base_height is the elevation where
                // the dome starts (not on top of walls as in from_roof_area),
                // since there is no building body underneath.
                let config = RoofConfig {
                    min_x,
                    max_x,
                    min_z,
                    max_z,
                    center_x: (min_x + max_x) >> 1,
                    center_z: (min_z + max_z) >> 1,
                    base_height: start_y_offset,
                    building_height: 4, // roof-only structure, no real walls
                    abs_terrain_offset,
                    roof_block,
                };
                generate_dome_roof(editor, cached_floor_area, &config);
            }
        }
        _ => {
            // Flat / unsupported shape: pillars at outline nodes + slab fill.
            let slab_y = start_y_offset + roof_thickness;

            // Outline pillars and edge slabs.
            let mut previous_node: Option<(i32, i32)> = None;
            for node in &element.nodes {
                let x = node.x;
                let z = node.z;

                if let Some(prev) = previous_node {
                    let pts = bresenham_line(prev.0, slab_y, prev.1, x, slab_y, z);
                    for (bx, _, bz) in pts {
                        editor.set_block_absolute(
                            roof_block,
                            bx,
                            slab_y + abs_terrain_offset,
                            bz,
                            None,
                            None,
                        );
                    }
                }

                // Determine the pillar base in the same coordinate system as
                // slab_y.  When terrain is enabled, both values are absolute
                // world coordinates.  When terrain is disabled, both are
                // relative to ground (abs_terrain_offset is added separately).
                let pillar_base = if args.terrain {
                    editor.get_ground_level(x, z)
                } else {
                    0
                };
                for y in (pillar_base + 1)..slab_y {
                    editor.set_block_absolute(
                        COBBLESTONE_WALL,
                        x,
                        y + abs_terrain_offset,
                        z,
                        None,
                        None,
                    );
                }

                previous_node = Some((x, z));
            }

            // Slab fill across the floor area.
            for &(x, z) in cached_floor_area {
                editor.set_block_absolute(
                    roof_block,
                    x,
                    slab_y + abs_terrain_offset,
                    z,
                    None,
                    None,
                );
            }
        }
    }
}

// ============================================================================
// Building Component Generators
// ============================================================================

/// Builds a wall ring (outer shell or inner courtyard) for a set of nodes.
#[allow(clippy::too_many_arguments)]
fn build_wall_ring(
    editor: &mut WorldEditor,
    nodes: &[ProcessedNode],
    config: &BuildingConfig,
    args: &Args,
    has_sloped_roof: bool,
    building_passages: &CoordinateBitmap,
) -> (Vec<(i32, i32)>, (i32, i32, i32)) {
    let mut previous_node: Option<(i32, i32)> = None;
    let mut corner_addup: (i32, i32, i32) = (0, 0, 0);
    let mut current_building: Vec<(i32, i32)> = Vec::new();

    let passage_height = BUILDING_PASSAGE_HEIGHT.min(config.building_height);

    for node in nodes {
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
                // Passages only apply to ground-level buildings; elevated
                // building:part elements (min_level > 0) receive an empty bitmap
                // via effective_passages, so this is always false for them.
                let is_passage = building_passages.contains(bx, bz);

                // Create foundation pillars when using terrain
                // Skip in passage zones so the road can pass through.
                if args.terrain && config.is_ground_level && !is_passage {
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

                // Generate wall blocks with windows.
                // In passage zones, skip below passage ceiling so the road
                // can pass through; place a floor-block lintel at the top of
                // the opening and continue the wall above.
                let wall_start = if is_passage {
                    config.start_y_offset + passage_height + 1
                } else {
                    config.start_y_offset + 1
                };

                for h in wall_start..=(config.start_y_offset + config.building_height) {
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

                // Place passage ceiling lintel
                if is_passage && passage_height < config.building_height {
                    editor.set_block_absolute(
                        config.floor_block,
                        bx,
                        config.start_y_offset + passage_height + config.abs_terrain_offset,
                        bz,
                        None,
                        None,
                    );
                }

                // Add roof line only for flat roofs, sloped roofs will cover this area
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
    building_passages: &CoordinateBitmap,
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

                // Skip placing doors inside a building passage
                if building_passages.contains(door1_x, door1_z)
                    || building_passages.contains(door2_x, door2_z)
                {
                    continue;
                }

                // Place the double door (lower and upper parts)
                // Use empty blacklist to overwrite existing wall blocks
                editor.set_block_absolute(
                    SPRUCE_DOOR_LOWER,
                    door1_x,
                    door_y,
                    door1_z,
                    None,
                    Some(&[]),
                );
                editor.set_block_absolute(
                    SPRUCE_DOOR_UPPER,
                    door1_x,
                    door_y + 1,
                    door1_z,
                    None,
                    Some(&[]),
                );
                editor.set_block_absolute(
                    SPRUCE_DOOR_LOWER,
                    door2_x,
                    door_y,
                    door2_z,
                    None,
                    Some(&[]),
                );
                editor.set_block_absolute(
                    SPRUCE_DOOR_UPPER,
                    door2_x,
                    door_y + 1,
                    door2_z,
                    None,
                    Some(&[]),
                );

                break; // Only place one set of garage doors
            }
        }
    } else if config.has_single_door {
        // Place a single oak door somewhere on the wall
        // Pick a random position from the wall outline
        if !wall_outline.is_empty() {
            let door_idx = rng.random_range(0..wall_outline.len());
            let (door_x, door_z) = wall_outline[door_idx];

            // Skip placing a door inside a building passage
            if !building_passages.contains(door_x, door_z) {
                // Place single oak door (empty blacklist to overwrite wall blocks)
                editor.set_block_absolute(OAK_DOOR, door_x, door_y, door_z, None, Some(&[]));
                editor.set_block_absolute(
                    OAK_DOOR_UPPER,
                    door_x,
                    door_y + 1,
                    door_z,
                    None,
                    Some(&[]),
                );
            }
        }
    }
}

/// Determines which block to place at a specific wall position (wall, window, or accent)
#[inline]
fn determine_wall_block_at_position(bx: i32, h: i32, bz: i32, config: &BuildingConfig) -> Block {
    let floor_row = config.floor_row(h);

    // If windows are disabled, always use wall block (with possible accent)
    if !config.has_windows {
        let above_floor = h > config.start_y_offset + 1;
        let use_accent_line = config.use_accent_lines && above_floor && floor_row == 0;
        if use_accent_line {
            return config.accent_block;
        }
        return config.wall_block;
    }

    let above_floor = h > config.start_y_offset + 1;

    if config.use_horizontal_windows {
        // Modern skyscraper pattern: continuous horizontal window bands
        // with stone separation bands at floor levels (every 4th block)
        if above_floor && config.has_lobby_base && h <= config.start_y_offset + 5 {
            // Solid lobby base: first floor cycle uses wall block
            config.wall_block
        } else if above_floor && floor_row == 0 {
            // Floor-level separation band (stone/accent material)
            config.accent_block
        } else if above_floor {
            // Full-width window band
            config.window_block
        } else {
            config.wall_block
        }
    } else if config.category == BuildingCategory::Tower {
        // Tower pattern: glass windows every 4 blocks along the wall,
        // only in the middle two rows of each 4-row floor
        let is_slit =
            above_floor && (floor_row == 1 || floor_row == 2) && ((bx + bz) % 4 + 4) % 4 == 1;

        if is_slit {
            config.window_block
        } else {
            let use_accent_line = config.use_accent_lines && above_floor && floor_row == 0;
            if use_accent_line {
                config.accent_block
            } else {
                config.wall_block
            }
        }
    } else if config.is_tall_building && config.use_vertical_windows {
        // Tall building pattern, vertical window strips alternating with wall columns
        if above_floor && (bx + bz) % 2 == 0 {
            config.window_block
        } else {
            config.wall_block
        }
    } else {
        // Regular building pattern
        let is_window_position = above_floor && floor_row != 0 && (bx + bz).rem_euclid(6) < 3;

        if is_window_position {
            config.window_block
        } else {
            let use_accent_line = config.use_accent_lines && above_floor && floor_row == 0;
            let use_vertical_accent_here = config.use_vertical_accent
                && above_floor
                && floor_row == 0
                && (bx + bz).rem_euclid(6) < 3;

            if use_accent_line || use_vertical_accent_here {
                config.accent_block
            } else {
                config.wall_block
            }
        }
    }
}

// ============================================================================
// Residential Window Decorations (Shutters & Window Boxes)
// ============================================================================

/// Trapdoor base blocks available for shutters (chosen once per building).
const SHUTTER_TRAPDOOR_OPTIONS: [Block; 4] = [
    OAK_TRAPDOOR_OPEN_NORTH, // re-used just for its name "oak_trapdoor"
    DARK_OAK_TRAPDOOR,
    SPRUCE_TRAPDOOR,
    BIRCH_TRAPDOOR,
];

/// Slab base blocks available for window sills (chosen once per building).
const SILL_SLAB_OPTIONS: [Block; 5] = [
    QUARTZ_SLAB_TOP,  // quartz_slab
    STONE_BRICK_SLAB, // stone_brick_slab
    MUD_BRICK_SLAB,   // mud_brick_slab
    OAK_SLAB,         // oak_slab
    BRICK_SLAB,       // brick_slab
];

/// Potted plant options for window boxes (chosen randomly per pot).
const POTTED_PLANT_OPTIONS: [Block; 4] = [
    FLOWER_POT, // potted_poppy
    POTTED_RED_TULIP,
    POTTED_DANDELION,
    POTTED_BLUE_ORCHID,
];

/// Creates a `BlockWithProperties` for an open trapdoor with the given
/// base block and facing direction string.
fn make_open_trapdoor(base: Block, facing: &str) -> BlockWithProperties {
    let mut map: HashMap<String, Value> = HashMap::new();
    map.insert("facing".to_string(), Value::String(facing.to_string()));
    map.insert("open".to_string(), Value::String("true".to_string()));
    map.insert("half".to_string(), Value::String("top".to_string()));
    BlockWithProperties::new(base, Some(Value::Compound(map)))
}

/// Creates a `BlockWithProperties` for a top-half slab.
fn make_top_slab(base: Block) -> BlockWithProperties {
    let mut map: HashMap<String, Value> = HashMap::new();
    map.insert("type".to_string(), Value::String("top".to_string()));
    BlockWithProperties::new(base, Some(Value::Compound(map)))
}

/// Computes the centroid (average position) of the building outline nodes.
/// Returns `None` if the node list is empty.
fn compute_building_centroid(nodes: &[ProcessedNode]) -> Option<(i32, i32)> {
    if nodes.is_empty() {
        return None;
    }
    let n = nodes.len() as i64;
    let sx: i64 = nodes.iter().map(|nd| nd.x as i64).sum();
    let sz: i64 = nodes.iter().map(|nd| nd.z as i64).sum();
    Some(((sx / n) as i32, (sz / n) as i32))
}

/// Computes how axis-aligned a building polygon is.
/// Returns ratio of polygon area to bounding box area.
/// - 1.0 = perfectly axis-aligned rectangle
/// - ~0.5 = 45° rotated square (bounding box is 2x larger)
/// - Lower values = more diagonal/rotated
///
/// Used to detect diagonal buildings that need rotation-invariant roofs.
fn compute_building_diagonality(nodes: &[ProcessedNode]) -> f64 {
    if nodes.len() < 3 {
        return 1.0;
    }

    // Calculate polygon area using shoelace formula
    let mut area = 0i64;
    for i in 0..nodes.len() {
        let j = (i + 1) % nodes.len();
        area += (nodes[i].x as i64) * (nodes[j].z as i64);
        area -= (nodes[j].x as i64) * (nodes[i].z as i64);
    }
    let polygon_area = (area.abs() as f64) / 2.0;

    // Calculate bounding box area
    let min_x = nodes.iter().map(|n| n.x).min().unwrap_or(0);
    let max_x = nodes.iter().map(|n| n.x).max().unwrap_or(0);
    let min_z = nodes.iter().map(|n| n.z).min().unwrap_or(0);
    let max_z = nodes.iter().map(|n| n.z).max().unwrap_or(0);
    let bbox_area = ((max_x - min_x + 1) * (max_z - min_z + 1)) as f64;

    if bbox_area <= 0.0 {
        return 1.0;
    }

    (polygon_area / bbox_area).min(1.0)
}

/// Computes the axis-aligned outward normal for a wall segment defined by
/// `(x1,z1)→(x2,z2)`, given the building centroid `(cx,cz)`.
///
/// Returns one of `(±1, 0)` or `(0, ±1)`, or `(0, 0)` for degenerate
/// (zero-length) segments.
fn compute_outward_normal(x1: i32, z1: i32, x2: i32, z2: i32, cx: i32, cz: i32) -> (i32, i32) {
    let seg_dx = x2 - x1;
    let seg_dz = z2 - z1;

    // Candidate outward normal (perpendicular to segment direction)
    let (na_x, na_z) = (-seg_dz, seg_dx);

    // Mid-point of the segment
    let mid_x = (x1 + x2) / 2;
    let mid_z = (z1 + z2) / 2;

    // Pick the normal that points AWAY from the centroid.
    let dot = (mid_x - cx) as i64 * na_x as i64 + (mid_z - cz) as i64 * na_z as i64;
    let (raw_nx, raw_nz) = if dot >= 0 {
        (na_x, na_z)
    } else {
        (-na_x, -na_z)
    };

    // Snap to the dominant axis so the normal is always one of
    // (±1, 0) or (0, ±1).
    if raw_nx.abs() >= raw_nz.abs() {
        (raw_nx.signum(), 0)
    } else {
        (0, raw_nz.signum())
    }
}

/// Returns the facing string for the wall's outward normal.
fn facing_for_normal(nx: i32, nz: i32) -> &'static str {
    match (nx, nz) {
        (1, _) => "east",
        (-1, _) => "west",
        (_, 1) => "south",
        _ => "north",
    }
}

/// Adds shutters and window sills (with occasional flower pots) to
/// **non-tall residential / house** buildings.
///
/// *Shutters* – open trapdoors placed one block outward from the wall
/// beside windows.  Both sides always appear together.  The trapdoor
/// material is chosen randomly per building.
///
/// *Window sills* – slabs spanning the full window width, one block outward
/// at the floor row below each window band.  A flower pot sits on one or
/// two of the three slab positions.  Slab material is random per building.
fn generate_residential_window_decorations(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    building_passages: &CoordinateBitmap,
) {
    // Only non-tall residential / house buildings get decorations.
    if config.is_tall_building {
        return;
    }
    if !matches!(
        config.category,
        BuildingCategory::Residential | BuildingCategory::House
    ) {
        return;
    }
    if !config.has_windows {
        return;
    }

    // --- Per-building random material choices ---
    let mut rng = element_rng(element.id);
    let trapdoor_base =
        SHUTTER_TRAPDOOR_OPTIONS[rng.random_range(0..SHUTTER_TRAPDOOR_OPTIONS.len())];
    let sill_base = SILL_SLAB_OPTIONS[rng.random_range(0..SILL_SLAB_OPTIONS.len())];
    let sill_block = make_top_slab(sill_base);

    // We need the building centroid so we can figure out which side of
    // each wall segment is "outside".
    let (cx, cz) = match compute_building_centroid(&element.nodes) {
        Some(c) => c,
        None => return,
    };

    let mut previous_node: Option<(i32, i32)> = None;

    for node in &element.nodes {
        let (x2, z2) = (node.x, node.z);
        if let Some((x1, z1)) = previous_node {
            let (out_nx, out_nz) = compute_outward_normal(x1, z1, x2, z2, cx, cz);

            // Skip degenerate normals (zero-length segment)
            if out_nx == 0 && out_nz == 0 {
                previous_node = Some((x2, z2));
                continue;
            }

            let facing = facing_for_normal(out_nx, out_nz);
            let trapdoor_bwp = make_open_trapdoor(trapdoor_base, facing);

            // Wall tangent (axis-aligned): perpendicular to the outward
            // normal inside the XZ plane.
            let (tan_x, tan_z) = (-out_nz, out_nx);

            // Walk the bresenham points of this wall segment
            let points =
                bresenham_line(x1, config.start_y_offset, z1, x2, config.start_y_offset, z2);

            for (bx, _, bz) in &points {
                let bx = *bx;
                let bz = *bz;

                // Skip decorations at passage openings
                if building_passages.contains(bx, bz) {
                    continue;
                }

                let mod6 = ((bx + bz) % 6 + 6) % 6; // always 0..5

                // --- Shutters ---
                // mod6 == 3 or 5 are the wall blocks flanking a window strip.
                // Both sides share the same roll (seeded on window centre).
                if mod6 == 3 || mod6 == 5 {
                    let centre_sum = if mod6 == 3 { bx + bz - 2 } else { bx + bz + 2 };
                    let shutter_roll =
                        coord_rng(centre_sum, centre_sum, element.id).random_range(0u32..100);
                    if shutter_roll < 25 {
                        for h in (config.start_y_offset + 1)
                            ..=(config.start_y_offset + config.building_height)
                        {
                            let above_floor = h > config.start_y_offset + 1;
                            if above_floor && config.floor_row(h) != 0 {
                                editor.set_block_with_properties_absolute(
                                    trapdoor_bwp.clone(),
                                    bx + out_nx,
                                    h + config.abs_terrain_offset,
                                    bz + out_nz,
                                    Some(&[AIR]),
                                    None,
                                );
                            }
                        }
                    }
                }

                // --- Window Sills / Balconies ---
                // Window columns are mod6 ∈ {0, 1, 2}.
                // At each floor's floor_row==0 row we decide once per window
                // whether this floor gets a sill OR a balcony (mutually
                // exclusive).  The decision is shared across all three
                // columns via a seed derived from the window centre.
                if mod6 < 3 {
                    // Stop 3 rows before the top so every sill has a
                    // full window (h+1..h+3) above it, avoids placing
                    // sills at the roof line.
                    let sill_max = config.start_y_offset + config.building_height - 3;
                    for h in (config.start_y_offset + 2)..=sill_max {
                        if config.floor_row(h) == 0 {
                            let floor_idx = h / 4;

                            // Shared roll seeded from the window centre.
                            let centre_sum = match mod6 {
                                0 => bx + bz + 1,
                                1 => bx + bz,
                                _ => bx + bz - 1,
                            };
                            let decoration_roll = coord_rng(
                                centre_sum.wrapping_add(floor_idx * 3),
                                centre_sum.wrapping_add(floor_idx * 5),
                                element.id,
                            )
                            .random_range(0u32..100);

                            let abs_y = h + config.abs_terrain_offset;

                            if decoration_roll < 15 {
                                // ── Window sill ──
                                let lx = bx + out_nx;
                                let lz = bz + out_nz;

                                editor.set_block_with_properties_absolute(
                                    sill_block.clone(),
                                    lx,
                                    abs_y,
                                    lz,
                                    Some(&[AIR]),
                                    None,
                                );

                                let mut pot_rng =
                                    coord_rng(bx, bz.wrapping_add(floor_idx), element.id);
                                let pot_here = if mod6 == 1 {
                                    pot_rng.random_range(0u32..100) < 70
                                } else {
                                    pot_rng.random_range(0u32..100) < 25
                                };
                                if pot_here {
                                    let plant = POTTED_PLANT_OPTIONS
                                        [pot_rng.random_range(0..POTTED_PLANT_OPTIONS.len())];
                                    editor.set_block_absolute(
                                        plant,
                                        lx,
                                        abs_y + 1,
                                        lz,
                                        Some(&[AIR]),
                                        None,
                                    );
                                }
                            } else if decoration_roll < 23 && mod6 == 1 {
                                // ── Balcony (placed once from centre col) ──
                                // A small 3-wide × 2-deep platform with
                                // open-trapdoor railing around the outer
                                // edge and occasional furniture.
                                //
                                // Top-down layout (outward = up):
                                //  depth 3:  [Tf] [Tf] [Tf]  front fence
                                //  depth 2:  [ f] [ f] [ f]  floor
                                //  depth 1:  [ f] [ f] [ f]  floor
                                //            wall wall wall
                                // Side fences at t=±2, depths 1–2.

                                let balcony_floor = make_top_slab(SMOOTH_STONE_SLAB);

                                // Facing strings for fences:
                                // Front fence faces back toward building
                                let front_facing = facing_for_normal(out_nx, out_nz);
                                // Side fences face inward along tangent
                                let left_facing = facing_for_normal(-tan_x, -tan_z);
                                let right_facing = facing_for_normal(tan_x, tan_z);

                                let front_fence = make_open_trapdoor(trapdoor_base, front_facing);
                                let left_fence = make_open_trapdoor(trapdoor_base, left_facing);
                                let right_fence = make_open_trapdoor(trapdoor_base, right_facing);

                                // Place floor slabs (3 wide × 2 deep)
                                for t in -1i32..=1 {
                                    let fx = bx + tan_x * t;
                                    let fz = bz + tan_z * t;

                                    for depth in 1i32..=2 {
                                        let px = fx + out_nx * depth;
                                        let pz = fz + out_nz * depth;

                                        editor.set_block_with_properties_absolute(
                                            balcony_floor.clone(),
                                            px,
                                            abs_y,
                                            pz,
                                            Some(&[AIR]),
                                            None,
                                        );
                                    }
                                }

                                // Front fence: trapdoors at depth 3
                                for t in -1i32..=1 {
                                    let fx = bx + tan_x * t + out_nx * 3;
                                    let fz = bz + tan_z * t + out_nz * 3;
                                    editor.set_block_with_properties_absolute(
                                        front_fence.clone(),
                                        fx,
                                        abs_y + 1,
                                        fz,
                                        Some(&[AIR]),
                                        None,
                                    );
                                }

                                // Side fences: trapdoors at t=±2, depths 1–2
                                for depth in 1i32..=2 {
                                    // Left side (t = -2)
                                    let lx = bx + tan_x * -2 + out_nx * depth;
                                    let lz = bz + tan_z * -2 + out_nz * depth;
                                    editor.set_block_with_properties_absolute(
                                        left_fence.clone(),
                                        lx,
                                        abs_y + 1,
                                        lz,
                                        Some(&[AIR]),
                                        None,
                                    );

                                    // Right side (t = +2)
                                    let rx = bx + tan_x * 2 + out_nx * depth;
                                    let rz = bz + tan_z * 2 + out_nz * depth;
                                    editor.set_block_with_properties_absolute(
                                        right_fence.clone(),
                                        rx,
                                        abs_y + 1,
                                        rz,
                                        Some(&[AIR]),
                                        None,
                                    );
                                }

                                // Occasional furniture on the balcony floor
                                let mut furn_rng = coord_rng(
                                    bx.wrapping_add(floor_idx * 11),
                                    bz.wrapping_add(floor_idx * 17),
                                    element.id,
                                );
                                let furniture_roll = furn_rng.random_range(0u32..100);

                                if furniture_roll < 30 {
                                    // Cauldron "planter" with a leaf block
                                    // on top, placed at depth 1 on one side
                                    let side = if furn_rng.random_bool(0.5) { -1i32 } else { 1 };
                                    let cx = bx + tan_x * side + out_nx;
                                    let cz = bz + tan_z * side + out_nz;
                                    editor.set_block_absolute(
                                        CAULDRON,
                                        cx,
                                        abs_y + 1,
                                        cz,
                                        Some(&[AIR]),
                                        None,
                                    );
                                    editor.set_block_absolute(
                                        OAK_LEAVES,
                                        cx,
                                        abs_y + 2,
                                        cz,
                                        Some(&[AIR]),
                                        None,
                                    );
                                } else if furniture_roll < 55 {
                                    // Stair "chair" facing outward
                                    let side = if furn_rng.random_bool(0.5) { -1i32 } else { 1 };
                                    let sx = bx + tan_x * side + out_nx;
                                    let sz = bz + tan_z * side + out_nz;
                                    let stair_facing = match facing_for_normal(-out_nx, -out_nz) {
                                        "north" => StairFacing::North,
                                        "south" => StairFacing::South,
                                        "east" => StairFacing::East,
                                        _ => StairFacing::West,
                                    };
                                    let chair = create_stair_with_properties(
                                        OAK_STAIRS,
                                        stair_facing,
                                        StairShape::Straight,
                                    );
                                    editor.set_block_with_properties_absolute(
                                        chair,
                                        sx,
                                        abs_y + 1,
                                        sz,
                                        Some(&[AIR]),
                                        None,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        previous_node = Some((x2, z2));
    }
}

// ============================================================================
// Wall Depth Features (Facade Protrusions)
// ============================================================================

/// Creates a `BlockWithProperties` for an upside-down stair used for
/// cornices and arched window headers. The `facing` parameter is the
/// **outward** wall direction; the stair is flipped to face **inward**
/// so that its ledge extends outward (matching real-world cornice behaviour).
fn make_upside_down_stair(material: Block, facing: &str) -> BlockWithProperties {
    let stair_block = get_stair_block_for_material(material);
    // Flip: stair faces inward so the "seat" ledge projects outward
    let stair_facing = match facing {
        "north" => StairFacing::South,
        "south" => StairFacing::North,
        "east" => StairFacing::West,
        _ => StairFacing::East,
    };
    top_stair(create_stair_with_properties(
        stair_block,
        stair_facing,
        StairShape::Straight,
    ))
}

/// Places accent-block columns at building polygon vertices (corner quoins).
/// This frames the building visually, a very common architectural detail.
/// Uses deterministic RNG for consistency across region boundaries.
fn generate_corner_quoins(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    building_passages: &CoordinateBitmap,
) {
    // Skip if wall and accent are the same block (nothing visible)
    if config.wall_block == config.accent_block {
        return;
    }

    // Too-small buildings look odd with quoins
    let bounds = BuildingBounds::from_nodes(&element.nodes);
    if bounds.width() < 4 || bounds.length() < 4 {
        return;
    }

    // Deterministic 60% chance
    let mut rng = element_rng(element.id.wrapping_add(3571));
    if !rng.random_bool(0.6) {
        return;
    }

    // Collect unique corner positions from polygon vertices
    // (skip duplicate closing node if first == last)
    let mut corners: Vec<(i32, i32)> = Vec::new();
    for node in &element.nodes {
        let pos = (node.x, node.z);
        if corners.last() != Some(&pos) {
            corners.push(pos);
        }
    }

    let quoin_block = config.accent_block;
    let top_h = config.start_y_offset + config.building_height;
    let passage_h = config.start_y_offset + BUILDING_PASSAGE_HEIGHT.min(config.building_height);

    for &(cx, cz) in &corners {
        let is_passage = building_passages.contains(cx, cz);
        let start_h = if is_passage {
            passage_h + 1
        } else {
            config.start_y_offset + 1
        };
        for h in start_h..=top_h {
            editor.set_block_absolute(
                quoin_block,
                cx,
                h + config.abs_terrain_offset,
                cz,
                Some(&[config.wall_block]),
                None,
            );
        }
    }
}

/// Adds wall depth features (pilasters, columns, ledges, cornices, buttresses)
/// to building facades. Blocks are placed 1+ block(s) outward from the wall
/// plane, making windows appear recessed by contrast.
///
/// Each `WallDepthStyle` produces a distinct visual effect appropriate for
/// the building's category. All outward placements use an AIR whitelist to
/// avoid overwriting neighboring buildings or existing decorations.
fn generate_wall_depth_features(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    has_sloped_roof: bool,
    building_passages: &CoordinateBitmap,
) {
    if config.wall_depth_style == WallDepthStyle::None {
        return;
    }

    // Skip buildings that are too small for depth features
    let bounds = BuildingBounds::from_nodes(&element.nodes);
    if bounds.width() < 4 || bounds.length() < 4 {
        return;
    }

    // Skip buildings with fewer than 2 floors for most styles
    if config.building_height < 6
        && !matches!(
            config.wall_depth_style,
            WallDepthStyle::HistoricOrnate | WallDepthStyle::ReligiousButtress
        )
    {
        return;
    }

    let (cx, cz) = match compute_building_centroid(&element.nodes) {
        Some(c) => c,
        None => return,
    };

    // Per-building deterministic roll for probability-gated styles
    let mut bldg_rng = element_rng(element.id.wrapping_add(7919));
    let depth_roll: u32 = bldg_rng.random_range(0..100);

    // SubtlePilasters: 60% of eligible buildings
    if config.wall_depth_style == WallDepthStyle::SubtlePilasters && depth_roll >= 60 {
        return;
    }
    // GlassCurtain: 40% of eligible buildings
    if config.wall_depth_style == WallDepthStyle::GlassCurtain && depth_roll >= 40 {
        return;
    }

    // Resolve material blocks for depth features
    let slab_block = get_slab_block_for_material(config.wall_block);
    let sill_block = make_top_slab(slab_block);

    // For sloped roofs with overhangs, stop depth features 2 blocks short
    // so protruding pilasters don't visually break the clean eave/overhang line.
    // 2 blocks: one for the eave-edge stair row at base_height, one for the
    // overhang stair placed 1 block outward at base_height - 1.
    let height_reduction = if has_sloped_roof { 2 } else { 0 };

    let mut previous_node: Option<(i32, i32)> = None;

    for node in &element.nodes {
        let (x2, z2) = (node.x, node.z);
        if let Some((x1, z1)) = previous_node {
            let (out_nx, out_nz) = compute_outward_normal(x1, z1, x2, z2, cx, cz);

            if out_nx == 0 && out_nz == 0 {
                previous_node = Some((x2, z2));
                continue;
            }

            let facing = facing_for_normal(out_nx, out_nz);

            let points =
                bresenham_line(x1, config.start_y_offset, z1, x2, config.start_y_offset, z2);

            let num_points = points.len();

            for (idx, (bx, _, bz)) in points.iter().enumerate() {
                let bx = *bx;
                let bz = *bz;

                // Skip decorative features at passage openings — the road
                // passes through here so no pilasters/buttresses/etc.
                if building_passages.contains(bx, bz) {
                    continue;
                }

                let mod6 = ((bx + bz) % 6 + 6) % 6;

                match config.wall_depth_style {
                    WallDepthStyle::SubtlePilasters => {
                        place_subtle_pilasters(
                            editor,
                            config,
                            bx,
                            bz,
                            mod6,
                            out_nx,
                            out_nz,
                            height_reduction,
                        );
                    }
                    WallDepthStyle::ModernPillars => {
                        place_modern_pillars(
                            editor,
                            config,
                            bx,
                            bz,
                            mod6,
                            out_nx,
                            out_nz,
                            &sill_block,
                            height_reduction,
                        );
                    }
                    WallDepthStyle::InstitutionalBands => {
                        place_institutional_bands(
                            editor,
                            config,
                            bx,
                            bz,
                            mod6,
                            out_nx,
                            out_nz,
                            facing,
                            height_reduction,
                        );
                    }
                    WallDepthStyle::IndustrialBeams => {
                        // Only at segment endpoints (first 2 and last 2 points)
                        if idx < 2 || idx >= num_points.saturating_sub(2) {
                            place_industrial_beams(
                                editor,
                                config,
                                bx,
                                bz,
                                out_nx,
                                out_nz,
                                height_reduction,
                            );
                        }
                    }
                    WallDepthStyle::HistoricOrnate => {
                        place_historic_ornate(
                            editor,
                            config,
                            bx,
                            bz,
                            mod6,
                            out_nx,
                            out_nz,
                            facing,
                            height_reduction,
                        );
                    }
                    WallDepthStyle::ReligiousButtress => {
                        place_religious_buttress(
                            editor,
                            config,
                            bx,
                            bz,
                            mod6,
                            out_nx,
                            out_nz,
                            facing,
                            height_reduction,
                        );
                    }
                    WallDepthStyle::SkyscraperFins => {
                        place_skyscraper_fins(
                            editor,
                            config,
                            bx,
                            bz,
                            mod6,
                            out_nx,
                            out_nz,
                            &sill_block,
                            height_reduction,
                        );
                    }
                    WallDepthStyle::GlassCurtain => {
                        // Only at segment endpoints
                        if idx == 0 || idx == num_points.saturating_sub(1) {
                            place_glass_curtain_corners(
                                editor,
                                config,
                                bx,
                                bz,
                                out_nx,
                                out_nz,
                                height_reduction,
                            );
                        }
                    }
                    WallDepthStyle::None => {}
                }
            }
        }

        previous_node = Some((x2, z2));
    }
}

/// SubtlePilasters: thin wall_block columns at mod6==3 positions (between window groups)
/// with an accent_block foundation course at ground level.
#[allow(clippy::too_many_arguments)]
fn place_subtle_pilasters(
    editor: &mut WorldEditor,
    config: &BuildingConfig,
    bx: i32,
    bz: i32,
    mod6: i32,
    out_nx: i32,
    out_nz: i32,
    height_reduction: i32,
) {
    if mod6 != 3 {
        return;
    }

    let lx = bx + out_nx;
    let lz = bz + out_nz;
    let top_h = config.start_y_offset + config.building_height - height_reduction;

    for h in (config.start_y_offset + 1)..=top_h {
        let block = if h == config.start_y_offset + 1 {
            config.accent_block // Foundation course
        } else {
            config.wall_block
        };
        editor.set_block_absolute(
            block,
            lx,
            h + config.abs_terrain_offset,
            lz,
            Some(&[AIR]),
            None,
        );
    }
}

/// ModernPillars: paired accent_block columns at mod6==3 and mod6==5,
/// plus horizontal slab bands at floor-separation rows.
#[allow(clippy::too_many_arguments)]
fn place_modern_pillars(
    editor: &mut WorldEditor,
    config: &BuildingConfig,
    bx: i32,
    bz: i32,
    mod6: i32,
    out_nx: i32,
    out_nz: i32,
    sill_block: &BlockWithProperties,
    height_reduction: i32,
) {
    let lx = bx + out_nx;
    let lz = bz + out_nz;
    let top_h = config.start_y_offset + config.building_height - height_reduction;

    // Pillar columns at edges of window bays
    if mod6 == 3 || mod6 == 5 {
        for h in (config.start_y_offset + 1)..=top_h {
            editor.set_block_absolute(
                config.accent_block,
                lx,
                h + config.abs_terrain_offset,
                lz,
                Some(&[AIR]),
                None,
            );
        }
        return;
    }

    // Horizontal slab bands at floor-level rows, for non-window positions
    if mod6 >= 3 {
        // Already handled by pillar columns above
        return;
    }

    // Foundation course at ground level
    editor.set_block_absolute(
        config.accent_block,
        lx,
        config.start_y_offset + 1 + config.abs_terrain_offset,
        lz,
        Some(&[AIR]),
        None,
    );

    // Floor-level slab bands (skip the window center at mod6==1 for cleaner look)
    for h in (config.start_y_offset + 2)..=top_h {
        if config.floor_row(h) == 0 {
            editor.set_block_with_properties_absolute(
                sill_block.clone(),
                lx,
                h + config.abs_terrain_offset,
                lz,
                Some(&[AIR]),
                None,
            );
        }
    }
}

/// InstitutionalBands: accent_block columns at mod6==3 + upside-down stair
/// ledges at floor-separation rows for non-window positions.
#[allow(clippy::too_many_arguments)]
fn place_institutional_bands(
    editor: &mut WorldEditor,
    config: &BuildingConfig,
    bx: i32,
    bz: i32,
    mod6: i32,
    out_nx: i32,
    out_nz: i32,
    facing: &str,
    height_reduction: i32,
) {
    let lx = bx + out_nx;
    let lz = bz + out_nz;
    let top_h = config.start_y_offset + config.building_height - height_reduction;

    // Pillar columns
    if mod6 == 3 {
        for h in (config.start_y_offset + 1)..=top_h {
            editor.set_block_absolute(
                config.accent_block,
                lx,
                h + config.abs_terrain_offset,
                lz,
                Some(&[AIR]),
                None,
            );
        }
        return;
    }

    // Foundation course
    editor.set_block_absolute(
        config.accent_block,
        lx,
        config.start_y_offset + 1 + config.abs_terrain_offset,
        lz,
        Some(&[AIR]),
        None,
    );

    // Stair ledges at floor-separation rows (non-window positions only)
    if mod6 >= 3 {
        return;
    }
    for h in (config.start_y_offset + 2)..=top_h {
        if config.floor_row(h) == 0 {
            let stair_bwp = make_upside_down_stair(config.wall_block, facing);
            editor.set_block_with_properties_absolute(
                stair_bwp,
                lx,
                h + config.abs_terrain_offset,
                lz,
                Some(&[AIR]),
                None,
            );
        }
    }
}

/// IndustrialBeams: heavy wall_block columns placed only at wall segment
/// endpoints (corners), running full building height.
#[allow(clippy::too_many_arguments)]
fn place_industrial_beams(
    editor: &mut WorldEditor,
    config: &BuildingConfig,
    bx: i32,
    bz: i32,
    out_nx: i32,
    out_nz: i32,
    height_reduction: i32,
) {
    let lx = bx + out_nx;
    let lz = bz + out_nz;
    let top_h = config.start_y_offset + config.building_height - height_reduction;

    for h in (config.start_y_offset + 1)..=top_h {
        editor.set_block_absolute(
            config.wall_block,
            lx,
            h + config.abs_terrain_offset,
            lz,
            Some(&[AIR]),
            None,
        );
    }
}

/// HistoricOrnate: wall_block columns at mod6==3, arched window headers
/// (upside-down stairs at window-top rows), cornice at roof line, and
/// foundation course.
#[allow(clippy::too_many_arguments)]
fn place_historic_ornate(
    editor: &mut WorldEditor,
    config: &BuildingConfig,
    bx: i32,
    bz: i32,
    mod6: i32,
    out_nx: i32,
    out_nz: i32,
    facing: &str,
    height_reduction: i32,
) {
    let lx = bx + out_nx;
    let lz = bz + out_nz;

    let top_h = config.start_y_offset + config.building_height - height_reduction;

    // Full-height pillar columns between window groups
    if mod6 == 3 {
        for h in (config.start_y_offset + 1)..=top_h {
            editor.set_block_absolute(
                config.wall_block,
                lx,
                h + config.abs_terrain_offset,
                lz,
                Some(&[AIR]),
                None,
            );
        }
        // Cornice at top (skip for sloped roofs - would conflict with roof)
        if height_reduction == 0 {
            let stair_bwp = make_upside_down_stair(config.wall_block, facing);
            editor.set_block_with_properties_absolute(
                stair_bwp,
                lx,
                top_h + config.abs_terrain_offset + 1,
                lz,
                Some(&[AIR]),
                None,
            );
        }
        return;
    }

    // Foundation course for all positions
    editor.set_block_absolute(
        config.accent_block,
        lx,
        config.start_y_offset + 1 + config.abs_terrain_offset,
        lz,
        Some(&[AIR]),
        None,
    );

    // Arched window headers at window-top rows (floor_row == 3) for window-edge positions
    if mod6 == 0 || mod6 == 2 {
        for h in (config.start_y_offset + 2)..=top_h {
            if config.floor_row(h) == 3 {
                let stair_bwp = make_upside_down_stair(config.wall_block, facing);
                editor.set_block_with_properties_absolute(
                    stair_bwp,
                    lx,
                    h + config.abs_terrain_offset,
                    lz,
                    Some(&[AIR]),
                    None,
                );
            }
        }
    }

    // Cornice along the full roofline (skip for sloped roofs)
    if height_reduction == 0 {
        let stair_bwp = make_upside_down_stair(config.wall_block, facing);
        editor.set_block_with_properties_absolute(
            stair_bwp,
            lx,
            top_h + config.abs_terrain_offset + 1,
            lz,
            Some(&[AIR]),
            None,
        );
    }
}

/// ReligiousButtress: stepped buttresses at every other window group,
/// plus cornice at roof line. Buttresses extend 2 blocks outward at the
/// lower portion and 1 block outward for the full height.
#[allow(clippy::too_many_arguments)]
fn place_religious_buttress(
    editor: &mut WorldEditor,
    config: &BuildingConfig,
    bx: i32,
    bz: i32,
    mod6: i32,
    out_nx: i32,
    out_nz: i32,
    facing: &str,
    height_reduction: i32,
) {
    let lx = bx + out_nx;
    let lz = bz + out_nz;
    let top_h = config.start_y_offset + config.building_height - height_reduction;

    // Buttress at every other window group center (mod6==0)
    let window_group = ((bx + bz) / 6).rem_euclid(2);
    if mod6 == 0 && window_group == 0 {
        let buttress_cutoff = config.start_y_offset + (config.building_height * 3 / 5);

        // Inner layer (outward+1): full height
        for h in (config.start_y_offset + 1)..=top_h {
            editor.set_block_absolute(
                config.wall_block,
                lx,
                h + config.abs_terrain_offset,
                lz,
                Some(&[AIR]),
                None,
            );
        }

        // Outer layer (outward+2): lower 60% of height
        let lx2 = bx + out_nx * 2;
        let lz2 = bz + out_nz * 2;
        for h in (config.start_y_offset + 1)..=buttress_cutoff {
            editor.set_block_absolute(
                config.wall_block,
                lx2,
                h + config.abs_terrain_offset,
                lz2,
                Some(&[AIR]),
                None,
            );
        }
        return;
    }

    // Cornice along the full roofline (skip for sloped roofs)
    if height_reduction == 0 {
        let stair_bwp = make_upside_down_stair(config.wall_block, facing);
        editor.set_block_with_properties_absolute(
            stair_bwp,
            lx,
            top_h + config.abs_terrain_offset + 1,
            lz,
            Some(&[AIR]),
            None,
        );
    }
}

/// SkyscraperFins: continuous accent_block vertical fins at mod6==3,
/// horizontal slab ledge bands at floor-separation rows for other positions,
/// and a foundation course at ground level.
#[allow(clippy::too_many_arguments)]
fn place_skyscraper_fins(
    editor: &mut WorldEditor,
    config: &BuildingConfig,
    bx: i32,
    bz: i32,
    mod6: i32,
    out_nx: i32,
    out_nz: i32,
    sill_block: &BlockWithProperties,
    height_reduction: i32,
) {
    let lx = bx + out_nx;
    let lz = bz + out_nz;
    let top_h = config.start_y_offset + config.building_height - height_reduction;

    // Foundation course at ground level (all positions)
    editor.set_block_absolute(
        config.accent_block,
        lx,
        config.start_y_offset + 1 + config.abs_terrain_offset,
        lz,
        Some(&[AIR]),
        None,
    );

    if mod6 == 3 {
        // Vertical fin column (existing behavior)
        for h in (config.start_y_offset + 1)..=top_h {
            editor.set_block_absolute(
                config.accent_block,
                lx,
                h + config.abs_terrain_offset,
                lz,
                Some(&[AIR]),
                None,
            );
        }
        return;
    }

    // Floor-level ledge bands at non-fin positions
    for h in (config.start_y_offset + 2)..=top_h {
        if config.floor_row(h) == 0 {
            editor.set_block_with_properties_absolute(
                sill_block.clone(),
                lx,
                h + config.abs_terrain_offset,
                lz,
                Some(&[AIR]),
                None,
            );
        }
    }
}

/// GlassCurtain: minimal accent_block columns only at wall segment
/// endpoints (corners) for subtle edge definition.
#[allow(clippy::too_many_arguments)]
fn place_glass_curtain_corners(
    editor: &mut WorldEditor,
    config: &BuildingConfig,
    bx: i32,
    bz: i32,
    out_nx: i32,
    out_nz: i32,
    height_reduction: i32,
) {
    let lx = bx + out_nx;
    let lz = bz + out_nz;
    let top_h = config.start_y_offset + config.building_height - height_reduction;

    for h in (config.start_y_offset + 1)..=top_h {
        editor.set_block_absolute(
            config.accent_block,
            lx,
            h + config.abs_terrain_offset,
            lz,
            Some(&[AIR]),
            None,
        );
    }
}

// ============================================================================
// Hospital Decorations
// ============================================================================

/// Places green-cross banners on the exterior walls of a hospital building.
///
/// For each wall segment (polygon edge), a wall banner with a green cross pattern
/// on a white background is placed at the midpoint of the segment, facing
/// outward.  The banner sits roughly at 2/3 of the building height so it is
/// clearly visible from the ground.
///
/// Only segments that are at least 5 blocks long receive a banner — this avoids
/// cluttering narrow corners and ensures the cross is readable.
fn generate_hospital_green_cross(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
) {
    if element.nodes.len() < 3 {
        return;
    }

    // Green cross on white background — universal pharmacy/hospital symbol.
    // Layer the full cross, then paint over the top/bottom edges with white
    // so the vertical arm doesn't stretch the full banner height.
    const GREEN_CROSS_PATTERNS: &[(&str, &str)] = &[
        ("green", "minecraft:straight_cross"),
        ("white", "minecraft:stripe_top"),
        ("white", "minecraft:stripe_bottom"),
        ("white", "minecraft:border"),
    ];

    let banner_y =
        config.start_y_offset + (config.building_height * 2 / 3).max(2) + config.abs_terrain_offset;

    let bounds = BuildingBounds::from_nodes(&element.nodes);
    let center_x = (bounds.min_x + bounds.max_x) / 2;
    let center_z = (bounds.min_z + bounds.max_z) / 2;

    let mut previous_node: Option<(i32, i32)> = None;
    for node in &element.nodes {
        let (x2, z2) = (node.x, node.z);
        if let Some((x1, z1)) = previous_node {
            let seg_len = ((x2 - x1).abs()).max((z2 - z1).abs());
            if seg_len < 5 {
                previous_node = Some((x2, z2));
                continue;
            }

            let mid_x = (x1 + x2) / 2;
            let mid_z = (z1 + z2) / 2;

            // Determine outward facing direction.
            // The wall runs from (x1,z1) to (x2,z2).  We pick the cardinal
            // direction that points away from the building centre.
            let dx = x2 - x1;
            let dz = z2 - z1;

            // Normal vector components (perpendicular to the wall segment).
            // Two candidates: (dz, -dx) and (-dz, dx).  Pick the one that
            // points away from the building centre.
            let (nx, nz) = {
                let (n1x, n1z) = (dz, -dx);
                let dot = (mid_x - center_x) * n1x + (mid_z - center_z) * n1z;
                if dot >= 0 {
                    (n1x, n1z)
                } else {
                    (-dz, dx)
                }
            };

            // Convert normal to cardinal facing and banner offset
            let (facing, bx, bz) = if nx.abs() >= nz.abs() {
                if nx > 0 {
                    ("east", mid_x + 1, mid_z) // banner faces east, placed east of wall
                } else {
                    ("west", mid_x - 1, mid_z) // banner faces west, placed west of wall
                }
            } else if nz > 0 {
                ("south", mid_x, mid_z + 1) // banner faces south, placed south of wall
            } else {
                ("north", mid_x, mid_z - 1) // banner faces north, placed north of wall
            };

            editor.place_wall_banner(
                WHITE_WALL_BANNER,
                bx,
                banner_y,
                bz,
                facing,
                "white",
                GREEN_CROSS_PATTERNS,
            );
        }
        previous_node = Some((x2, z2));
    }
}

/// Generates a helipad marking on the flat roof of a hospital.
///
/// Layout (7×7 yellow concrete pad with a 5×5 "H" pattern):
/// The pad is placed near the centre of the roof surface.
fn generate_hospital_helipad(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    floor_area: &[(i32, i32)],
    config: &BuildingConfig,
) {
    if floor_area.is_empty() {
        return;
    }

    let floor_set: HashSet<(i32, i32)> = floor_area.iter().copied().collect();

    // Roof surface Y (on top of the flat roof)
    let roof_y = config.start_y_offset + config.building_height + config.abs_terrain_offset + 1;

    // Find centre of the building footprint
    let bounds = BuildingBounds::from_nodes(&element.nodes);
    let center_x = (bounds.min_x + bounds.max_x) / 2;
    let center_z = (bounds.min_z + bounds.max_z) / 2;

    let pad_half = 3; // 7×7 pad → half-size = 3

    // Verify the 7×7 area fits within the roof
    let pad_fits = (-pad_half..=pad_half).all(|dx| {
        (-pad_half..=pad_half).all(|dz| floor_set.contains(&(center_x + dx, center_z + dz)))
    });

    if !pad_fits {
        return;
    }

    let replace_any: &[Block] = &[];

    // The "H" character in a 5×5 grid (centred inside the 7×7 pad)
    // Rows/cols indexed -2..=2
    let is_h = |col: i32, row: i32| -> bool {
        let ac = col.abs();
        let ar = row.abs();
        // Two vertical bars at col ±2, plus horizontal bar at row 0
        ac == 2 || (ar == 0 && ac <= 2)
    };

    for dx in -pad_half..=pad_half {
        for dz in -pad_half..=pad_half {
            let bx = center_x + dx;
            let bz = center_z + dz;

            // Outer ring is always yellow
            let is_border = dx.abs() == pad_half || dz.abs() == pad_half;

            let block = if is_border {
                YELLOW_CONCRETE
            } else if is_h(dx, dz) {
                WHITE_CONCRETE
            } else {
                YELLOW_CONCRETE
            };

            editor.set_block_absolute(block, bx, roof_y, bz, None, Some(replace_any));
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
    building_passages: &CoordinateBitmap,
) -> HashSet<(i32, i32)> {
    let mut processed_points: HashSet<(i32, i32)> = HashSet::new();
    let ceiling_light_block = if config.is_abandoned_building {
        COBWEB
    } else {
        GLOWSTONE
    };

    let passage_height = BUILDING_PASSAGE_HEIGHT.min(config.building_height);

    for &(x, z) in cached_floor_area {
        if !processed_points.insert((x, z)) {
            continue;
        }

        let is_passage = building_passages.contains(x, z);

        // Set ground floor — skip in passage zones (the road surface is placed
        // by the highway processor instead).
        if !is_passage {
            editor.set_block_absolute(
                config.floor_block,
                x,
                config.start_y_offset + config.abs_terrain_offset,
                z,
                None,
                None,
            );
        }

        // Set intermediate ceilings with light fixtures
        if config.building_height > 4 {
            for h in (config.start_y_offset + 2 + 4..config.start_y_offset + config.building_height)
                .step_by(4)
            {
                // Skip intermediate ceilings below passage opening
                if is_passage && h <= config.start_y_offset + passage_height {
                    continue;
                }

                let block = if x % 5 == 0 && z % 5 == 0 {
                    ceiling_light_block
                } else {
                    config.floor_block
                };
                editor.set_block_absolute(block, x, h + config.abs_terrain_offset, z, None, None);
            }
        } else if x % 5 == 0 && z % 5 == 0 && !is_passage {
            // Single floor building with ceiling light (skip in passage)
            editor.set_block_absolute(
                ceiling_light_block,
                x,
                config.start_y_offset + config.building_height + config.abs_terrain_offset,
                z,
                None,
                None,
            );
        }

        // Place passage ceiling lintel at the top of the archway
        if is_passage && passage_height < config.building_height {
            editor.set_block_absolute(
                config.floor_block,
                x,
                config.start_y_offset + passage_height + config.abs_terrain_offset,
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

/// Parses roof:shape tag into RoofType enum.
///
/// Tag frequencies from OSM taginfo are used to decide which synonyms
/// deserve a mapping: anything above ~0.1% is handled here so those
/// buildings get a pitched roof instead of falling through to Flat.
fn parse_roof_type(roof_shape: &str) -> RoofType {
    match roof_shape {
        // Gabled variants: "pitched" is a common synonym; saltbox/gabled_row
        // are asymmetric/repeated gables that still read as gabled at block
        // resolution.
        "gabled" | "pitched" | "saltbox" | "double_saltbox" | "quadruple_saltbox"
        | "gabled_row" => RoofType::Gabled,
        "hipped" | "half-hipped" | "gambrel" | "mansard" | "round" | "side_hipped"
        | "side_half-hipped" => RoofType::Hipped,
        "skillion" | "lean_to" => RoofType::Skillion,
        "pyramidal" => RoofType::Pyramidal,
        "dome" | "onion" | "cone" | "circular" | "spherical" => RoofType::Dome,
        _ => RoofType::Flat,
    }
}

/// Checks if building type qualifies for automatic gabled roof.
///
/// Single-family/low-rise residential and agricultural buildings should
/// default to a pitched roof in the absence of an explicit roof:shape tag,
/// since real-world buildings of these types almost never have flat roofs.
fn qualifies_for_auto_gabled_roof(building_type: &str) -> bool {
    matches!(
        building_type,
        "apartments"
            | "residential"
            | "house"
            | "yes"
            | "detached"
            | "semidetached_house"
            | "terrace"
            | "bungalow"
            | "villa"
            | "cabin"
            | "hut"
            | "farm"
            | "farm_auxiliary"
            | "barn"
            | "stable"
            | "cowshed"
            | "sty"
            | "sheepfold"
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
    hole_polygons: Option<&[HolePolygon]>,
    flood_fill_cache: &FloodFillCache,
    building_passages: &CoordinateBitmap,
) {
    // Early return for underground buildings
    if should_skip_underground_building(element) {
        return;
    }

    // Skip structures that cannot be represented as conventional buildings.
    // building:part elements at that location add the correct details
    // Eiffel Tower, London Eye, Utah State Capitol
    const SKIP_WAY_IDS: &[u64] = &[5013364, 204068874, 32920861];
    if SKIP_WAY_IDS.contains(&element.id) {
        return;
    }

    // Intercept tomb=pyramid: generate a sandstone pyramid instead of a building
    if element.tags.get("tomb").map(|v| v.as_str()) == Some("pyramid") {
        historic::generate_pyramid(editor, element, args, flood_fill_cache);
        return;
    }

    // Parse vertical offset: min_height (meters) takes priority, then
    // building:min_level (floor count).  This lifts the structure off the
    // ground for elevated building:parts such as observation-wheel capsules.
    let min_level = element
        .tags
        .get("building:min_level")
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);

    let scale_factor = args.scale;
    let abs_terrain_offset = if !args.terrain { args.ground_level } else { 0 };

    let min_level_offset = if let Some(mh) = element.tags.get("min_height") {
        mh.trim_end_matches('m')
            .trim()
            .parse::<f64>()
            .ok()
            .map(|h| (h * scale_factor) as i32)
            .unwrap_or(0)
    } else {
        multiply_scale(min_level * 4, scale_factor)
    };

    // Get cached floor area. Hole carving below needs `retain`, which requires
    // ownership, so we materialize a Vec here. Buildings typically have small
    // footprints (tens to hundreds of cells), so the deep copy is cheap — the
    // big Arc wins come from landuse/natural/leisure handlers.
    let mut cached_floor_area: Vec<(i32, i32)> = flood_fill_cache
        .get_or_compute(element, args.timeout.as_ref())
        .as_ref()
        .clone();

    if let Some(holes) = hole_polygons {
        if !holes.is_empty() {
            let outer_area: HashSet<(i32, i32)> = cached_floor_area.iter().copied().collect();
            let mut hole_points: HashSet<(i32, i32)> = HashSet::new();

            for hole in holes {
                if hole.way.nodes.len() < 3 {
                    continue;
                }

                let hole_area = flood_fill_cache.get_or_compute(&hole.way, args.timeout.as_ref());
                if hole_area.is_empty() {
                    continue;
                }

                if !hole_area.iter().any(|pt| outer_area.contains(pt)) {
                    continue;
                }

                for &point in hole_area.iter() {
                    hole_points.insert(point);
                }
            }

            if !hole_points.is_empty() {
                cached_floor_area.retain(|point| !hole_points.contains(point));
            }
        }
    }

    let cached_footprint_size = cached_floor_area.len();
    if cached_footprint_size == 0 {
        return;
    }

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

    // Route building:part="roof" to the roof-only structure generator.
    // This must be checked before the "building" tag match below, since elements
    // with building:part="roof" (but no "building" tag) would otherwise fall
    // through to the full building pipeline and render as small boxy buildings.
    if element.tags.get("building:part").map(|v| v.as_str()) == Some("roof") {
        generate_roof_only_structure(editor, element, &cached_floor_area, args);
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
                generate_roof_only_structure(editor, element, &cached_floor_area, args);
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
        is_ground_level: min_level_offset == 0,
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
        use_horizontal_windows: style.use_horizontal_windows,
        use_accent_roof_line: style.use_accent_roof_line,
        use_accent_lines: style.use_accent_lines,
        use_vertical_accent: style.use_vertical_accent,
        is_abandoned_building,
        has_windows: style.has_windows,
        has_garage_door: style.has_garage_door,
        has_single_door: style.has_single_door,
        category,
        wall_depth_style: style.wall_depth_style,
        has_parapet: style.has_parapet,
        has_lobby_base: if category == BuildingCategory::ModernSkyscraper {
            element_rng(element.id.wrapping_add(6143)).random_bool(0.70)
        } else {
            false
        },
    };

    // Passages only apply to ground-level buildings. Elevated building:part
    // elements (min_level > 0) sit above the passage and must keep their
    // walls, floors and decorations intact.
    let empty_passages = CoordinateBitmap::new_empty();
    let effective_passages: &CoordinateBitmap = if config.is_ground_level {
        building_passages
    } else {
        &empty_passages
    };

    // Generate walls, pass whether this building will have a sloped roof
    let has_sloped_roof = args.roof && style.generate_roof && style.roof_type != RoofType::Flat;
    let (wall_outline, corner_addup) = build_wall_ring(
        editor,
        &element.nodes,
        &config,
        args,
        has_sloped_roof,
        effective_passages,
    );

    if let Some(holes) = hole_polygons {
        for hole in holes {
            if hole.add_walls {
                let _ = build_wall_ring(
                    editor,
                    &hole.way.nodes,
                    &config,
                    args,
                    has_sloped_roof,
                    effective_passages,
                );
            }
        }
    }

    // Generate special doors (garage doors, shed doors)
    if config.has_garage_door || config.has_single_door {
        generate_special_doors(editor, element, &config, &wall_outline, effective_passages);
    }

    // Add shutters and window boxes to small residential buildings
    generate_residential_window_decorations(editor, element, &config, effective_passages);

    // Add wall depth features (pilasters, columns, ledges, cornices, buttresses)
    // Only for standalone buildings, not building:part sub-sections (parts adjoin
    // other parts and outward protrusions would collide with neighbours).
    if !element.tags.contains_key("building:part") {
        generate_wall_depth_features(
            editor,
            element,
            &config,
            has_sloped_roof,
            effective_passages,
        );
    }

    // Add corner quoins (accent-block columns at building corners)
    if !element.tags.contains_key("building:part") {
        generate_corner_quoins(editor, element, &config, effective_passages);
    }

    // Create roof area = floor area + wall outline (so roof covers the walls too)
    let roof_area: Vec<(i32, i32)> = {
        let mut area: HashSet<(i32, i32)> = cached_floor_area.iter().copied().collect();
        area.extend(wall_outline.iter().copied());
        // Sort to ensure deterministic iteration order across runs/platforms
        let mut v: Vec<(i32, i32)> = area.into_iter().collect();
        v.sort_unstable();
        v
    };

    // Generate floors and ceilings
    if corner_addup != (0, 0, 0) {
        generate_floors_and_ceilings(
            editor,
            &cached_floor_area,
            &config,
            args,
            style.generate_roof,
            effective_passages,
        );

        // Build tunnel side walls: for each interior coordinate that borders a
        // passage coordinate, place a wall column from ground to passage ceiling.
        // This creates the left/right corridor walls inside the archway.
        // Only applies to ground-level buildings (elevated building:parts are
        // above the passage and should not get corridor walls).
        if !effective_passages.is_empty() {
            let passage_height = BUILDING_PASSAGE_HEIGHT.min(config.building_height);
            let abs = config.abs_terrain_offset;
            for &(x, z) in &cached_floor_area {
                if effective_passages.contains(x, z) {
                    continue; // this is road, not a wall
                }
                // Check 4-connected neighbours for passage adjacency
                let adjacent_to_passage = effective_passages.contains(x - 1, z)
                    || effective_passages.contains(x + 1, z)
                    || effective_passages.contains(x, z - 1)
                    || effective_passages.contains(x, z + 1);
                if adjacent_to_passage {
                    for y in (config.start_y_offset + 1)..=(config.start_y_offset + passage_height)
                    {
                        editor.set_block_absolute(config.wall_block, x, y + abs, z, None, None);
                    }
                }
            }
        }

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
                    effective_passages,
                );
            }
        }
    }

    // Process roof generation using style decisions
    if args.roof && style.generate_roof {
        generate_building_roof(
            editor, element, &config, &style, &bounds, &roof_area, category,
        );
    }
}

/// Generates a parapet (low wall) around the edge of flat-roofed buildings.
///
/// For shorter buildings (< 16 blocks), uses a thin wall piece.
/// For taller buildings, uses a full wall block for a more substantial parapet.
fn generate_parapet(editor: &mut WorldEditor, element: &ProcessedWay, config: &BuildingConfig) {
    if !config.has_parapet {
        return;
    }

    if element.nodes.is_empty() {
        return;
    }

    let wall_piece = get_wall_piece_for_material(config.wall_block);
    // Parapet sits on top of the flat roof surface (roof_y + 1 + abs_terrain_offset)
    let parapet_y = config.start_y_offset + config.building_height + config.abs_terrain_offset + 2;

    let mut previous_node: Option<(i32, i32)> = None;

    for node in &element.nodes {
        let (x2, z2) = (node.x, node.z);
        if let Some((x1, z1)) = previous_node {
            let points =
                bresenham_line(x1, config.start_y_offset, z1, x2, config.start_y_offset, z2);

            for (bx, _, bz) in &points {
                let block = if config.building_height >= 16 {
                    config.wall_block
                } else {
                    wall_piece
                };
                editor.set_block_absolute(block, *bx, parapet_y, *bz, Some(&[AIR]), None);
            }
        }
        previous_node = Some((x2, z2));
    }

    // Enhanced parapet for modern skyscrapers: accent slab cap + corner posts
    if config.category == BuildingCategory::ModernSkyscraper {
        let cap_slab = make_top_slab(get_slab_block_for_material(config.accent_block));
        let cap_y = parapet_y + 1;

        // Cap slabs along wall perimeter
        let mut prev: Option<(i32, i32)> = None;
        for node in &element.nodes {
            let (x2, z2) = (node.x, node.z);
            if let Some((x1, z1)) = prev {
                let points =
                    bresenham_line(x1, config.start_y_offset, z1, x2, config.start_y_offset, z2);
                for (bx, _, bz) in &points {
                    editor.set_block_with_properties_absolute(
                        cap_slab.clone(),
                        *bx,
                        cap_y,
                        *bz,
                        Some(&[AIR]),
                        None,
                    );
                }
            }
            prev = Some((x2, z2));
        }

        // Corner posts: full accent block at polygon vertices
        let mut corners: Vec<(i32, i32)> = Vec::new();
        for node in &element.nodes {
            let pos = (node.x, node.z);
            if corners.last() != Some(&pos) {
                corners.push(pos);
            }
        }
        for &(cx, cz) in &corners {
            editor.set_block_absolute(config.accent_block, cx, cap_y, cz, None, Some(&[]));
        }
    }
}

/// Adds a decorative top edge to flat-roofed residential/generic buildings.
/// Randomly picks one of: raised wall row, slab cap, accent block row, or nothing.
/// Uses deterministic RNG so the result is consistent across region boundaries.
fn generate_flat_roof_edge_variation(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
) {
    if element.nodes.is_empty() {
        return;
    }

    let mut rng = element_rng(element.id);
    // 55% chance to add edge variation
    if !rng.random_bool(0.55) {
        return;
    }

    // Pick variation type: 0 = wall cap (1 block higher), 1 = slab cap, 2 = accent block row
    let variation = rng.random_range(0u32..3);
    let roof_top_y = config.start_y_offset + config.building_height + config.abs_terrain_offset + 2;

    let mut previous_node: Option<(i32, i32)> = None;
    for node in &element.nodes {
        let (x2, z2) = (node.x, node.z);
        if let Some((x1, z1)) = previous_node {
            let points =
                bresenham_line(x1, config.start_y_offset, z1, x2, config.start_y_offset, z2);
            for (bx, _, bz) in &points {
                let block = match variation {
                    0 => config.wall_block,
                    1 => get_slab_block_for_material(config.wall_block),
                    _ => config.accent_block,
                };
                editor.set_block_absolute(block, *bx, roof_top_y, *bz, Some(&[AIR]), None);
            }
        }
        previous_node = Some((x2, z2));
    }
}

/// Handles roof generation including chimney placement and rooftop equipment
fn generate_building_roof(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    style: &BuildingStyle,
    bounds: &BuildingBounds,
    roof_area: &[(i32, i32)],
    category: BuildingCategory,
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
        config.roof_block,
        style.roof_type,
        roof_area,
        config.abs_terrain_offset,
    );

    // Add parapet on flat-roofed buildings
    if config.has_parapet && style.roof_type == RoofType::Flat {
        generate_parapet(editor, element, config);
    }

    // Add decorative roofline variation on flat-roofed residential/generic buildings
    // (those that don't already have a parapet or non-flat roof)
    if !config.has_parapet && style.roof_type == RoofType::Flat {
        generate_flat_roof_edge_variation(editor, element, config);
    }

    // Add chimney if style says so
    if style.has_chimney {
        let roof_peak_height =
            calculate_roof_peak_height(bounds, config.start_y_offset, config.building_height);
        generate_chimney(
            editor,
            roof_area,
            bounds.min_x,
            bounds.max_x,
            bounds.min_z,
            bounds.max_z,
            roof_peak_height,
            config.abs_terrain_offset,
            element.id,
        );
    }

    // Add roof terrace on flat-roofed tall building:part elements
    if should_generate_roof_terrace(element, config, style.roof_type) {
        let roof_y = config.start_y_offset + config.building_height;
        generate_roof_terrace(
            editor,
            element,
            roof_area,
            bounds,
            roof_y,
            config.abs_terrain_offset,
        );
    }

    // Add sparse rooftop equipment on flat-roofed commercial/institutional buildings
    if should_generate_rooftop_equipment(config, style.roof_type, category) {
        let roof_y = config.start_y_offset + config.building_height;
        generate_rooftop_equipment(
            editor,
            element,
            roof_area,
            roof_y,
            config.abs_terrain_offset,
        );
    }

    // Hospital helipad on the flat roof
    if category == BuildingCategory::Hospital && style.roof_type == RoofType::Flat {
        generate_hospital_helipad(editor, element, roof_area, config);
    }

    // Hospital green cross banners on exterior walls
    if category == BuildingCategory::Hospital {
        generate_hospital_green_cross(editor, element, config);
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
    let quadrant = rng.random_range(0..4);

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
    let (chimney_x, chimney_z) = final_candidates[rng.random_range(0..final_candidates.len())];

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
// Roof Terrace Generation
// ============================================================================

/// Generates a roof terrace on top of flat-roofed tall buildings (building:part).
///
/// Includes:
/// - Stone brick railing around the perimeter
/// - Scattered rooftop furniture/equipment (tables, ventilation units, planters, seating, antenna)
#[allow(clippy::too_many_arguments)]
fn generate_roof_terrace(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    floor_area: &[(i32, i32)],
    bounds: &BuildingBounds,
    roof_y: i32,
    abs_terrain_offset: i32,
) {
    if floor_area.is_empty() {
        return;
    }

    let replace_any: &[Block] = &[];
    // Flat roof is placed at (start_y_offset + building_height + 1 + abs_terrain_offset)
    // roof_y = start_y_offset + building_height, so terrace must be at roof_y + 2 to sit ON TOP of the roof
    let terrace_y = roof_y + abs_terrain_offset + 2;

    // Build a set for O(1) lookup of floor positions
    let floor_set: HashSet<(i32, i32)> = floor_area.iter().copied().collect();

    // --- Step 1: Railing around the perimeter ---
    // A perimeter block is one that has at least one cardinal neighbor NOT in the floor set
    for &(x, z) in floor_area {
        let neighbors = [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)];
        let is_edge = neighbors.iter().any(|n| !floor_set.contains(n));

        if is_edge {
            editor.set_block_absolute(STONE_BRICKS, x, terrace_y, z, None, Some(replace_any));
        }
    }

    // --- Step 2: Collect interior positions (non-edge blocks at least 1 from edge) ---
    let interior: Vec<(i32, i32)> = floor_area
        .iter()
        .filter(|&&(x, z)| {
            let neighbors = [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)];
            neighbors.iter().all(|n| floor_set.contains(n))
        })
        .copied()
        .collect();

    if interior.is_empty() {
        return;
    }

    // --- Step 3: Place rooftop furniture deterministically ---
    // Use coord_rng so each position is independently and deterministically decorated.
    // The low placement probability (15%) naturally creates spacing between items.

    // We iterate over interior positions and use coord_rng to decide what goes where.
    // This avoids RNG ordering issues and is fully deterministic per-position.
    for &(x, z) in &interior {
        // Deterministic per-position decision using coord_rng
        let mut rng = coord_rng(x, z, element.id);
        let roll: u32 = rng.random_range(0..100);

        // ~85% of interior tiles are empty (open terrace space)
        if roll >= 15 {
            continue;
        }

        // Among the 15% that get furniture, distribute types
        match roll {
            0..=2 => {
                // Ventilation unit: iron block with a slab on top
                editor.set_block_absolute(IRON_BLOCK, x, terrace_y, z, None, Some(replace_any));
                editor.set_block_absolute(
                    SMOOTH_STONE_SLAB,
                    x,
                    terrace_y + 1,
                    z,
                    None,
                    Some(replace_any),
                );
            }
            3..=5 => {
                // Planter: leaf block on top of cauldron
                editor.set_block_absolute(CAULDRON, x, terrace_y, z, None, Some(replace_any));
                // Vary the leaf type
                let leaf = match rng.random_range(0..3) {
                    0 => OAK_LEAVES,
                    1 => BIRCH_LEAVES,
                    _ => SPRUCE_LEAVES,
                };
                editor.set_block_absolute(leaf, x, terrace_y + 1, z, None, Some(replace_any));
            }
            6..=8 => {
                // Table: oak slab on top of an oak fence
                editor.set_block_absolute(OAK_FENCE, x, terrace_y, z, None, Some(replace_any));
                editor.set_block_absolute(OAK_SLAB, x, terrace_y + 1, z, None, Some(replace_any));
            }
            9..=10 => {
                // Seating: stairs block (looks like a bench/chair)
                editor.set_block_absolute(OAK_STAIRS, x, terrace_y, z, None, Some(replace_any));
            }
            11..=12 => {
                // Antenna / lightning rod
                editor.set_block_absolute(LIGHTNING_ROD, x, terrace_y, z, None, Some(replace_any));
            }
            13 => {
                // Cauldron (rain collector / decorative)
                editor.set_block_absolute(CAULDRON, x, terrace_y, z, None, Some(replace_any));
            }
            _ => {
                // Sea lantern (subtle rooftop light)
                editor.set_block_absolute(SEA_LANTERN, x, terrace_y, z, None, Some(replace_any));
            }
        }
    }

    // --- Step 4: Always place a lightning rod or antenna near the center (if space) ---
    let center_x = (bounds.min_x + bounds.max_x) / 2;
    let center_z = (bounds.min_z + bounds.max_z) / 2;

    // Find the interior point closest to center
    if let Some(&(cx, cz)) = interior
        .iter()
        .min_by_key(|&&(x, z)| (x - center_x).pow(2) + (z - center_z).pow(2))
    {
        // Tall antenna: 6 iron bars + lightning rod on top
        for dy in 0..6 {
            editor.set_block_absolute(IRON_BARS, cx, terrace_y + dy, cz, None, Some(replace_any));
        }
        editor.set_block_absolute(
            LIGHTNING_ROD,
            cx,
            terrace_y + 6,
            cz,
            None,
            Some(replace_any),
        );
    }
}

/// Determines whether a building should get a roof terrace.
///
/// Conditions:
/// - The element is a `building:part` (composite building component)
/// - Has a flat roof
/// - Is tall enough (skyscraper-class or very tall: height >= 28 blocks)
fn should_generate_roof_terrace(
    element: &ProcessedWay,
    config: &BuildingConfig,
    roof_type: RoofType,
) -> bool {
    let is_building_part = element.tags.contains_key("building:part");
    let is_flat = roof_type == RoofType::Flat;
    let is_very_tall = config.building_height >= 28;

    is_building_part && is_flat && is_very_tall
}

/// Determines whether a building should get sparse rooftop equipment (HVAC, solar panels).
///
/// Applies to flat-roofed commercial, office, industrial, warehouse, hospital, and hotel
/// buildings that are at least a few floors tall.
fn should_generate_rooftop_equipment(
    config: &BuildingConfig,
    roof_type: RoofType,
    category: BuildingCategory,
) -> bool {
    let is_flat = roof_type == RoofType::Flat;
    let is_multi_floor = config.building_height >= 8;
    // Place rooftop equipment on any flat-roofed multi-floor building
    // except small residential houses, religious, and special types.
    let dominated_by_roof_elements = matches!(
        category,
        BuildingCategory::House
            | BuildingCategory::Farm
            | BuildingCategory::Garage
            | BuildingCategory::Shed
            | BuildingCategory::Greenhouse
            | BuildingCategory::Religious
    );

    is_flat && is_multi_floor && !dominated_by_roof_elements
}

/// Generates sparse rooftop equipment on flat-roofed commercial/institutional buildings.
///
/// Much sparser than the skyscraper roof terrace (~1% of interior tiles).
/// Equipment types:
/// - HVAC / ventilation units (iron block + slab)
/// - Solar panel clusters (daylight detectors in 5×4 fields)
/// - Antenna masts (iron bars + lightning rod)
/// - Water tanks (barrel + cauldron)
/// - Vent stacks (cobblestone wall columns)
/// - Roof access structures (2×2 stone brick box with slab cap)
fn generate_rooftop_equipment(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    floor_area: &[(i32, i32)],
    roof_y: i32,
    abs_terrain_offset: i32,
) {
    if floor_area.is_empty() {
        return;
    }

    let replace_any: &[Block] = &[];
    let equip_y = roof_y + abs_terrain_offset + 2; // On top of the flat roof surface

    // Build set for edge detection
    let floor_set: HashSet<(i32, i32)> = floor_area.iter().copied().collect();

    // Collect interior positions (skip edge tiles to avoid overhanging equipment)
    let interior: Vec<(i32, i32)> = floor_area
        .iter()
        .filter(|&&(x, z)| {
            let neighbors = [
                (x - 1, z),
                (x + 1, z),
                (x, z - 1),
                (x, z + 1),
                (x - 1, z - 1),
                (x + 1, z + 1),
                (x - 1, z + 1),
                (x + 1, z - 1),
            ];
            neighbors.iter().all(|n| floor_set.contains(n))
        })
        .copied()
        .collect();

    if interior.is_empty() {
        return;
    }

    // Track which positions are already used (for solar panel clusters)
    let mut used: HashSet<(i32, i32)> = HashSet::new();

    for &(x, z) in &interior {
        if used.contains(&(x, z)) {
            continue;
        }

        let mut rng = coord_rng(x, z, element.id);
        let roll: u32 = rng.random_range(0..1200);

        // ~99% of tiles are empty, very sparse
        if roll >= 12 {
            continue;
        }

        match roll {
            0..=2 => {
                // HVAC / ventilation unit: iron block + smooth stone slab
                editor.set_block_absolute(IRON_BLOCK, x, equip_y, z, None, Some(replace_any));
                editor.set_block_absolute(
                    SMOOTH_STONE_SLAB,
                    x,
                    equip_y + 1,
                    z,
                    None,
                    Some(replace_any),
                );
                used.insert((x, z));
            }
            3..=5 => {
                // Solar panel cluster: four 5×4 fields in a 2×2 grid with 1-block gaps
                // Layout (top view, 11 wide × 9 deep):
                //   SSSSS . SSSSS
                //   SSSSS . SSSSS
                //   SSSSS . SSSSS
                //   SSSSS . SSSSS
                //   ..... . .....
                //   SSSSS . SSSSS
                //   SSSSS . SSSSS
                //   SSSSS . SSSSS
                //   SSSSS . SSSSS
                let quad_offsets: [(i32, i32); 4] = [(0, 0), (6, 0), (0, 5), (6, 5)];
                let quad_panels: Vec<(i32, i32)> = quad_offsets
                    .iter()
                    .flat_map(|&(ox, oz)| {
                        (0..5).flat_map(move |dx| (0..4).map(move |dz| (x + ox + dx, z + oz + dz)))
                    })
                    .collect();
                // Check that the entire 11×9 bounding box fits on the roof
                let bbox: Vec<(i32, i32)> = (0..11)
                    .flat_map(|dx| (0..9).map(move |dz| (x + dx, z + dz)))
                    .collect();
                let quad_ok = bbox
                    .iter()
                    .all(|pos| floor_set.contains(pos) && !used.contains(pos));

                if quad_ok {
                    for &(cx, cz) in &quad_panels {
                        editor.set_block_absolute(
                            DAYLIGHT_DETECTOR,
                            cx,
                            equip_y,
                            cz,
                            None,
                            Some(replace_any),
                        );
                    }
                    // Reserve the whole bounding box so nothing overlaps
                    for &(cx, cz) in &bbox {
                        used.insert((cx, cz));
                    }
                } else {
                    // Fall back to a single 5×4 field
                    let single_field: Vec<(i32, i32)> = (0..5)
                        .flat_map(|dx| (0..4).map(move |dz| (x + dx, z + dz)))
                        .collect();
                    let single_ok = single_field
                        .iter()
                        .all(|pos| floor_set.contains(pos) && !used.contains(pos));

                    if single_ok {
                        for &(cx, cz) in &single_field {
                            editor.set_block_absolute(
                                DAYLIGHT_DETECTOR,
                                cx,
                                equip_y,
                                cz,
                                None,
                                Some(replace_any),
                            );
                            used.insert((cx, cz));
                        }
                    } else {
                        // Not enough room, place a single daylight detector
                        editor.set_block_absolute(
                            DAYLIGHT_DETECTOR,
                            x,
                            equip_y,
                            z,
                            None,
                            Some(replace_any),
                        );
                        used.insert((x, z));
                    }
                }
            }
            6 => {
                // Small antenna mast: 2 iron bars + lightning rod
                editor.set_block_absolute(IRON_BARS, x, equip_y, z, None, Some(replace_any));
                editor.set_block_absolute(IRON_BARS, x, equip_y + 1, z, None, Some(replace_any));
                editor.set_block_absolute(
                    LIGHTNING_ROD,
                    x,
                    equip_y + 2,
                    z,
                    None,
                    Some(replace_any),
                );
                used.insert((x, z));
            }
            7..=8 => {
                // Water tank: barrel with cauldron on top
                editor.set_block_absolute(BARREL, x, equip_y, z, None, Some(replace_any));
                editor.set_block_absolute(CAULDRON, x, equip_y + 1, z, None, Some(replace_any));
                used.insert((x, z));
            }
            9..=10 => {
                // Vent stack: 2-3 cobblestone wall blocks tall
                let stack_h = rng.random_range(2i32..=3);
                for dy in 0..stack_h {
                    editor.set_block_absolute(
                        COBBLESTONE_WALL,
                        x,
                        equip_y + dy,
                        z,
                        None,
                        Some(replace_any),
                    );
                }
                used.insert((x, z));
            }
            _ => {
                // Roof access box: 2×2 stone brick structure (stairwell exit)
                let positions = [(x, z), (x + 1, z), (x, z + 1), (x + 1, z + 1)];
                let all_fit = positions
                    .iter()
                    .all(|pos| floor_set.contains(pos) && !used.contains(pos));
                if all_fit {
                    for &(bx, bz) in &positions {
                        editor.set_block_absolute(
                            STONE_BRICKS,
                            bx,
                            equip_y,
                            bz,
                            None,
                            Some(replace_any),
                        );
                        editor.set_block_absolute(
                            STONE_BRICKS,
                            bx,
                            equip_y + 1,
                            bz,
                            None,
                            Some(replace_any),
                        );
                        editor.set_block_absolute(
                            STONE_BRICK_SLAB,
                            bx,
                            equip_y + 2,
                            bz,
                            None,
                            Some(replace_any),
                        );
                        used.insert((bx, bz));
                    }
                }
            }
        }
    }
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
    building_height: i32,
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
        let _ = rng.random::<u32>();
        let roof_block = if rng.random_bool(0.1) {
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
            building_height,
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
    // Create a HashSet for O(1) footprint lookups, this is the actual building shape
    let footprint: HashSet<(i32, i32)> = floor_area.iter().copied().collect();

    let width_is_longer = config.width() >= config.length();
    let ridge_runs_along_x = match roof_orientation {
        Some(o) if o.eq_ignore_ascii_case("along") => width_is_longer,
        Some(o) if o.eq_ignore_ascii_case("across") => !width_is_longer,
        _ => width_is_longer,
    };

    // For each footprint position, scan all 4 cardinal directions to
    // find the distance to the nearest polygon edge.  This replaces an
    // older single-axis scan that only measured perpendicular to the
    // ridge, which failed on complex buildings (perimeter buildings,
    // L/U shapes with courtyards) where wings run in both directions.
    //
    // We store the perpendicular-to-ridge (dm_perp, dp_perp) per position
    // for stair facing direction, and also compute the cross-axis span
    // so we can cap roof height by the narrowest local wing width in
    // ANY direction.
    let mut edge_scans: HashMap<(i32, i32), (i32, i32)> = HashMap::new();

    // Helper: scan from (x,z) in a direction until leaving the footprint
    let scan_dir = |mut cx: i32, mut cz: i32, dx: i32, dz: i32| -> i32 {
        let mut dist = 0;
        loop {
            cx += dx;
            cz += dz;
            if !footprint.contains(&(cx, cz)) {
                break;
            }
            dist += 1;
        }
        dist
    };

    let mut roof_heights: HashMap<(i32, i32), i32> = HashMap::new();

    // Hard cap: the roof peak should never exceed the wall height.
    // Real gabled roofs typically add at most ~60% of the wall height.
    let wall_cap = ((config.building_height as f64) * 0.6).round().max(1.0) as i32;

    // First pass: compute roof heights with 1:1 slope, gather stats to
    // detect whether the flat ridge area is unacceptably wide.
    struct PosData {
        dist_to_edge: i32,
        local_half: i32,
    }
    let mut pos_data: HashMap<(i32, i32), PosData> = HashMap::new();
    let mut max_perp_half: i32 = 0; // widest perpendicular half-span

    for &(x, z) in floor_area {
        // Scan all 4 cardinal directions
        let dm_z = scan_dir(x, z, 0, -1); // -Z
        let dp_z = scan_dir(x, z, 0, 1); // +Z
        let dm_x = scan_dir(x, z, -1, 0); // -X
        let dp_x = scan_dir(x, z, 1, 0); // +X

        // Perpendicular-to-ridge distances (for slope direction / dist_to_edge)
        let (dm_perp, dp_perp) = if ridge_runs_along_x {
            (dm_z, dp_z)
        } else {
            (dm_x, dp_x)
        };
        edge_scans.insert((x, z), (dm_perp, dp_perp));

        let dist_to_edge = dm_perp.min(dp_perp);

        // Local wing width in both axes
        let half_z = (dm_z + dp_z + 1) / 2;
        let half_x = (dm_x + dp_x + 1) / 2;
        let local_half = half_z.min(half_x);

        let perp_half = (dm_perp + dp_perp + 1) / 2;
        if perp_half > max_perp_half {
            max_perp_half = perp_half;
        }

        pos_data.insert(
            (x, z),
            PosData {
                dist_to_edge,
                local_half,
            },
        );
    }

    // If the widest perpendicular half-span exceeds `wall_cap`, the 1:1
    // slope would create a flat ridge larger than `max_perp_half - wall_cap`
    // blocks wide.  When that flat band is ≥ 4 blocks, switch to half-pitch
    // (1 block rise per 2 blocks inward) so the slope is gentler and the
    // flat area at the peak is reduced.
    let flat_band = max_perp_half - wall_cap;
    let use_half_pitch = flat_band >= 4;

    for &(x, z) in floor_area {
        let pd = &pos_data[&(x, z)];
        let slope_dist = if use_half_pitch {
            pd.dist_to_edge / 2
        } else {
            pd.dist_to_edge
        };
        let local_boost = ((pd.local_half as f64) * 0.85).round().max(1.0) as i32;
        let capped_boost = local_boost.min(wall_cap);
        let roof_height = (config.base_height + slope_dist).min(config.base_height + capped_boost);
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
    // Uses the polygon-edge scanning to pick the correct slope direction
    // even for diagonal buildings where the center coordinate is misleading.
    let get_slope_stair = |x: i32, z: i32| -> BlockWithProperties {
        let closer_to_minus = edge_scans.get(&(x, z)).is_some_and(|&(dm, dp)| dm <= dp);
        if ridge_runs_along_x {
            if closer_to_minus {
                // Closer to north (-Z) edge → on north slope → faces south
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
        } else if closer_to_minus {
            // Closer to west (-X) edge → on west slope → faces east
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

    // ── Overhang: extend eave 1 block outward with stairs ──────────
    // For each position on the eave (outer edge perpendicular to the ridge),
    // place a stair block 1 block outward at base_height, facing away from
    // the building. This only extends sideways (perpendicular to the ridge),
    // not along the gable ends, matching real roof construction.
    let mut overhang_positions: Vec<(i32, i32, BlockWithProperties)> = Vec::new();

    for &(x, z) in floor_area {
        if ridge_runs_along_x {
            // Eave runs along X; overhang extends in +Z / -Z direction
            if !footprint.contains(&(x, z - 1)) {
                // North eave — place overhang 1 block further north
                let oz = z - 1;
                let stair = create_stair_with_properties(
                    stair_block_material,
                    StairFacing::South,
                    StairShape::Straight,
                );
                overhang_positions.push((x, oz, stair));
            }
            if !footprint.contains(&(x, z + 1)) {
                // South eave — place overhang 1 block further south
                let oz = z + 1;
                let stair = create_stair_with_properties(
                    stair_block_material,
                    StairFacing::North,
                    StairShape::Straight,
                );
                overhang_positions.push((x, oz, stair));
            }
        } else {
            // Eave runs along Z; overhang extends in +X / -X direction
            if !footprint.contains(&(x - 1, z)) {
                let ox = x - 1;
                let stair = create_stair_with_properties(
                    stair_block_material,
                    StairFacing::East,
                    StairShape::Straight,
                );
                overhang_positions.push((ox, z, stair));
            }
            if !footprint.contains(&(x + 1, z)) {
                let ox = x + 1;
                let stair = create_stair_with_properties(
                    stair_block_material,
                    StairFacing::West,
                    StairShape::Straight,
                );
                overhang_positions.push((ox, z, stair));
            }
        }
    }

    for (ox, oz, stair) in overhang_positions {
        // No whitelist — overhang stairs must overwrite wall depth pillars
        // that extend to this Y level
        editor.set_block_with_properties_absolute(
            stair,
            ox,
            config.base_height - 1 + config.abs_terrain_offset,
            oz,
            None,
            None,
        );
    }
}

/// Generates a hipped roof using polygon-edge scanning.
///
/// A hipped roof slopes on ALL four sides.  For complex / multipolygon
/// buildings the old bounding-box approach produced a single pyramid peak
/// at the bounding-box center.  This version scans the actual polygon
/// footprint in all 4 cardinal directions — the same technique used for
/// gabled roofs — so it adapts to L/U/courtyard shapes automatically.
///
/// Height at each position = min(dist to nearest polygon edge in any
/// cardinal direction), capped by 60 % of the building wall height,
/// with half-pitch when the flat peak area would be too wide.
fn generate_hipped_roof(editor: &mut WorldEditor, floor_area: &[(i32, i32)], config: &RoofConfig) {
    let footprint: HashSet<(i32, i32)> = floor_area.iter().copied().collect();

    // Scan from (x,z) in one cardinal direction until leaving the footprint.
    let scan_dir = |mut cx: i32, mut cz: i32, dx: i32, dz: i32| -> i32 {
        let mut dist = 0;
        loop {
            cx += dx;
            cz += dz;
            if !footprint.contains(&(cx, cz)) {
                break;
            }
            dist += 1;
        }
        dist
    };

    let wall_cap = ((config.building_height as f64) * 0.6).round().max(1.0) as i32;

    // --- First pass: gather per-position edge distances ---
    struct PosData {
        /// Minimum distance to polygon edge in any of the 4 cardinal dirs
        dist_to_edge: i32,
        /// The narrowest local half-span (min of the two cross-axis halves)
        local_half: i32,
        /// Which cardinal direction had the shortest distance (for stair facing).
        /// 0 = -X, 1 = +X, 2 = -Z, 3 = +Z
        closest_dir: u8,
    }
    let mut pos_data: HashMap<(i32, i32), PosData> = HashMap::new();
    let mut max_half: i32 = 0; // widest half-span across all positions

    for &(x, z) in floor_area {
        let dm_x = scan_dir(x, z, -1, 0);
        let dp_x = scan_dir(x, z, 1, 0);
        let dm_z = scan_dir(x, z, 0, -1);
        let dp_z = scan_dir(x, z, 0, 1);

        let dists = [dm_x, dp_x, dm_z, dp_z];
        let dist_to_edge = *dists.iter().min().unwrap();

        // Determine which edge is closest (for stair facing)
        let closest_dir = if dist_to_edge == dm_x {
            0u8
        } else if dist_to_edge == dp_x {
            1
        } else if dist_to_edge == dm_z {
            2
        } else {
            3
        };

        let half_x = (dm_x + dp_x + 1) / 2;
        let half_z = (dm_z + dp_z + 1) / 2;
        let local_half = half_x.min(half_z);

        let full_span = half_x.max(half_z);
        if full_span > max_half {
            max_half = full_span;
        }

        pos_data.insert(
            (x, z),
            PosData {
                dist_to_edge,
                local_half,
                closest_dir,
            },
        );
    }

    // Half-pitch when the flat peak area would be ≥ 4 blocks wide
    let flat_band = max_half - wall_cap;
    let use_half_pitch = flat_band >= 4;

    // --- Second pass: compute roof heights ---
    let mut roof_heights: HashMap<(i32, i32), i32> = HashMap::new();

    for &(x, z) in floor_area {
        let pd = &pos_data[&(x, z)];
        let slope_dist = if use_half_pitch {
            pd.dist_to_edge / 2
        } else {
            pd.dist_to_edge
        };
        let local_boost = ((pd.local_half as f64) * 0.85).round().max(1.0) as i32;
        let capped_boost = local_boost.min(wall_cap);
        let roof_height = (config.base_height + slope_dist).min(config.base_height + capped_boost);
        roof_heights.insert((x, z), roof_height);
    }

    // --- Place blocks with stair facing toward nearest polygon edge ---
    let stair_block_material = get_stair_block_for_material(config.roof_block);

    place_roof_blocks_with_stairs(editor, floor_area, &roof_heights, config, |x, z, _| {
        let dir = pos_data.get(&(x, z)).map(|pd| pd.closest_dir).unwrap_or(0);
        match dir {
            0 => {
                // Closest edge is -X, stair faces east (toward centre)
                create_stair_with_properties(
                    stair_block_material,
                    StairFacing::East,
                    StairShape::Straight,
                )
            }
            1 => {
                // Closest edge is +X, stair faces west
                create_stair_with_properties(
                    stair_block_material,
                    StairFacing::West,
                    StairShape::Straight,
                )
            }
            2 => {
                // Closest edge is -Z, stair faces south
                create_stair_with_properties(
                    stair_block_material,
                    StairFacing::South,
                    StairShape::Straight,
                )
            }
            _ => {
                // Closest edge is +Z, stair faces north
                create_stair_with_properties(
                    stair_block_material,
                    StairFacing::North,
                    StairShape::Straight,
                )
            }
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
    let shorter_half = config.width().min(config.length()) / 2;
    let uncapped_boost = ((shorter_half as f64) * 0.75).round().max(3.0) as i32;
    let wall_cap = ((config.building_height as f64) * 0.6).round().max(3.0) as i32;
    let peak_height = config.base_height + uncapped_boost.min(wall_cap);
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
    roof_block_override: Option<Block>,
    roof_type: RoofType,
    roof_area: &[(i32, i32)],
    abs_terrain_offset: i32,
) {
    if roof_area.is_empty() {
        return;
    }

    let mut config = RoofConfig::from_roof_area(
        roof_area,
        element.id,
        start_y_offset,
        building_height,
        wall_block,
        accent_block,
        abs_terrain_offset,
    );

    // If a preset specifies a dedicated roof block, use it instead of
    // the randomly-derived wall/accent block.
    if let Some(override_block) = roof_block_override {
        config.roof_block = override_block;
    }

    let roof_orientation = element.tags.get("roof:orientation").map(|s| s.as_str());

    // For flat roofs, also honour the override so preset flat-roof
    // materials (e.g. greenhouse smooth-stone slab) are respected.
    let flat_roof_block = roof_block_override.unwrap_or(floor_block);

    match roof_type {
        RoofType::Flat => {
            generate_flat_roof(
                editor,
                roof_area,
                flat_roof_block,
                config.base_height,
                abs_terrain_offset,
            );
        }

        RoofType::Gabled => {
            generate_gabled_roof(editor, roof_area, &config, roof_orientation);
        }

        RoofType::Hipped => {
            generate_hipped_roof(editor, roof_area, &config);
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
    xzbbox: &crate::coordinate_system::cartesian::XZBBox,
    building_passages: &CoordinateBitmap,
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

    // Check if this is a type=building relation with part members.
    // Only type=building relations use Part roles; type=multipolygon relations
    // should always render their Outer members normally.
    let is_building_type = relation.tags.get("type").map(|t| t.as_str()) == Some("building");
    let has_parts = is_building_type
        && relation
            .members
            .iter()
            .any(|m| m.role == ProcessedMemberRole::Part);

    if !has_parts {
        // Collect outer member node lists and merge open segments into closed rings.
        // Multipolygon relations commonly split the outline across many short way
        // segments that share endpoints. Without merging, each segment is processed
        // individually, producing degenerate polygons and empty flood fills (only
        // wall outlines, no filled floors/ceilings/roofs).
        let mut outer_rings: Vec<Vec<ProcessedNode>> = relation
            .members
            .iter()
            .filter(|m| m.role == ProcessedMemberRole::Outer)
            .map(|m| m.way.nodes.clone())
            .collect();

        super::merge_way_segments(&mut outer_rings);

        // Clip assembled rings to the world bounding box.  Because member ways
        // were kept unclipped during parsing (to allow ring assembly), the
        // merged rings may extend beyond the requested area.  Clipping prevents
        // oversized flood fills and unnecessary block placement.
        outer_rings = outer_rings
            .into_iter()
            .map(|ring| clip_way_to_bbox(&ring, xzbbox))
            .filter(|ring| ring.len() >= 4)
            .collect();

        // Close rings that are nearly closed (endpoints within 1 block)
        for ring in &mut outer_rings {
            if ring.len() >= 3 {
                let first = &ring[0];
                let last = ring.last().unwrap();
                if first.id != last.id {
                    let dx = (first.x - last.x).abs();
                    let dz = (first.z - last.z).abs();
                    if dx <= 1 && dz <= 1 {
                        let close_node = ring[0].clone();
                        ring.push(close_node);
                    }
                }
            }
        }

        // Discard rings that are still open or too small
        outer_rings.retain(|ring| {
            if ring.len() < 4 {
                return false;
            }
            let first = &ring[0];
            let last = ring.last().unwrap();
            first.id == last.id || ((first.x - last.x).abs() <= 1 && (first.z - last.z).abs() <= 1)
        });

        // Collect and assemble inner rings for courtyards/holes.
        let mut inner_rings: Vec<Vec<ProcessedNode>> = relation
            .members
            .iter()
            .filter(|m| m.role == ProcessedMemberRole::Inner)
            .map(|m| m.way.nodes.clone())
            .collect();

        super::merge_way_segments(&mut inner_rings);

        inner_rings = inner_rings
            .into_iter()
            .map(|ring| clip_way_to_bbox(&ring, xzbbox))
            .filter(|ring| ring.len() >= 4)
            .collect();

        // Close rings that are nearly closed (endpoints within 1 block)
        for ring in &mut inner_rings {
            if ring.len() >= 3 {
                let first = &ring[0];
                let last = ring.last().unwrap();
                if first.id != last.id {
                    let dx = (first.x - last.x).abs();
                    let dz = (first.z - last.z).abs();
                    if dx <= 1 && dz <= 1 {
                        let close_node = ring[0].clone();
                        ring.push(close_node);
                    }
                }
            }
        }

        // Discard rings that are still open or too small
        inner_rings.retain(|ring| {
            if ring.len() < 4 {
                return false;
            }
            let first = &ring[0];
            let last = ring.last().unwrap();
            first.id == last.id || ((first.x - last.x).abs() <= 1 && (first.z - last.z).abs() <= 1)
        });

        let hole_polygons: Option<Vec<HolePolygon>> = if inner_rings.is_empty() {
            None
        } else {
            Some(
                inner_rings
                    .into_iter()
                    .enumerate()
                    .map(|(ring_idx, ring)| {
                        // Use a different index range from outer rings to avoid cache collisions.
                        let ring_slot = 0x8000u64 | (ring_idx as u64 & 0x7FFF);
                        let synthetic_id = (1u64 << 63) | (relation.id << 16) | ring_slot;
                        HolePolygon {
                            way: ProcessedWay {
                                id: synthetic_id,
                                tags: HashMap::new(),
                                nodes: ring,
                            },
                            add_walls: true,
                        }
                    })
                    .collect(),
            )
        };

        // Build a synthetic ProcessedWay for each assembled ring and render it.
        // The relation tags are applied so that building type, levels, and roof
        // shape from the relation are honoured.
        //
        // Synthetic IDs use bit 63 as a flag combined with the relation ID and a
        // ring index.  This prevents collisions with real way IDs in the flood
        // fill cache and the deterministic RNG seeded by element ID.
        for (ring_idx, ring) in outer_rings.into_iter().enumerate() {
            let synthetic_id = (1u64 << 63) | (relation.id << 16) | (ring_idx as u64 & 0xFFFF);
            let merged_way = ProcessedWay {
                id: synthetic_id,
                tags: relation.tags.clone(),
                nodes: ring,
            };
            generate_buildings(
                editor,
                &merged_way,
                args,
                Some(relation_levels),
                hole_polygons.as_deref(),
                flood_fill_cache,
                building_passages,
            );
        }
    }
    // When has_parts: parts are rendered as standalone ways from the elements list.
    // The outline way is suppressed in data_processing to avoid overlaying the parts.
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
    let bridge_area = flood_fill_cache.get_or_compute(element, floodfill_timeout);

    // Use the same level bridge deck height for filled areas
    let floor_y = bridge_deck_ground_y + bridge_y_offset;

    // Place floor blocks
    for &(x, z) in bridge_area.iter() {
        editor.set_block_absolute(floor_block, x, floor_y, z, None, None);
    }
}
