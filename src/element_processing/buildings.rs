use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::climate::Climate;
use crate::clipping::clip_way_to_bbox;
use crate::colors::color_text_to_rgb_tuple;
use crate::deterministic_rng::{coord_rng, element_rng};
use crate::element_processing::building_facade::{
    compute_facade_plan, BuildingContext, ColumnFacade, FacadeAnchor, FacadeClass, FacadePlan,
    MIN_FACADE_FOOTPRINT,
};
use crate::element_processing::historic;
use crate::element_processing::subprocessor::buildings_interior::generate_building_interior;
use crate::floodfill_cache::{CoordinateBitmap, FloodFillCache};
use crate::osm_parser::{
    ArchEra, ProcessedMemberRole, ProcessedNode, ProcessedRelation, ProcessedWay,
};
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use fnv::FnvHashSet;
use rand::Rng;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

/// Lifecycle / damage state derived from OSM tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildingCondition {
    Normal,
    Construction,
    Disused,
    Abandoned,
    Ruined,
}

impl BuildingCondition {
    /// Returns the strongest applicable state (Ruined > Abandoned > Disused > Construction > Normal).
    fn from_tags(tags: &HashMap<String, String>) -> Self {
        let building_value = tags.get("building").map(String::as_str);
        if matches!(building_value, Some("ruins" | "collapsed"))
            || tags.get("historic").map(String::as_str) == Some("ruins")
            || tags.get("ruins").map(String::as_str) == Some("yes")
            || tags.contains_key("ruins:building")
        {
            return Self::Ruined;
        }

        if tags.get("abandoned").map(String::as_str) == Some("yes")
            || tags.contains_key("abandoned:building")
        {
            return Self::Abandoned;
        }

        // Bare `disused=yes` is ambiguous, only the namespaced form counts.
        if tags.contains_key("disused:building") {
            return Self::Disused;
        }

        if building_value == Some("construction") || tags.contains_key("construction:building") {
            return Self::Construction;
        }

        Self::Normal
    }
}

/// Enum representing different roof types
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RoofType {
    /// Steep lower slope, shallow upper slope, flat cap.
    Mansard,
    /// Barn roof: steep lower gable pitch breaking to a shallow upper pitch.
    Gambrel,
    /// Gable whose ends are hipped only above half height.
    HalfHipped,
    Gabled,    // Two sloping sides meeting at a ridge
    Hipped, // All sides slope downwards to walls (including Half-hipped, Gambrel, Mansard variations)
    Skillion, // Single sloping surface
    Pyramidal, // All sides come to a point at the top
    Dome,   // Rounded, hemispherical structure
    Cone,   // Conical roof, circular base tapering to a point
    Onion,  // Bulbous onion roof
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

/// Wall blocks suitable for residential buildings.
const RESIDENTIAL_WALL_OPTIONS: [Block; 29] = [
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
    ORANGE_TERRACOTTA,
    RED_TERRACOTTA,
    RED_NETHER_BRICKS,
    GRANITE,
    TERRACOTTA,
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
const RELIGIOUS_WALL_OPTIONS: [Block; 9] = [
    STONE_BRICKS,
    CHISELED_STONE_BRICKS,
    QUARTZ_BLOCK,
    WHITE_CONCRETE,
    SANDSTONE,
    SMOOTH_SANDSTONE,
    POLISHED_DIORITE,
    END_STONE_BRICKS,
    WAXED_OXIDIZED_COPPER,
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
const FARM_WALL_OPTIONS: [Block; 8] = [
    OAK_PLANKS,
    SPRUCE_PLANKS,
    DARK_OAK_PLANKS,
    COBBLESTONE,
    STONE,
    MUD_BRICKS,
    MOSSY_COBBLESTONE,
    BROWN_TERRACOTTA,
];

/// Wall blocks suitable for historic/castle buildings
const HISTORIC_WALL_OPTIONS: [Block; 16] = [
    STONE_BRICKS,
    CRACKED_STONE_BRICKS,
    CHISELED_STONE_BRICKS,
    COBBLESTONE,
    SANDSTONE,
    SMOOTH_SANDSTONE,
    POLISHED_BLACKSTONE_BRICKS,
    DEEPSLATE_BRICKS,
    POLISHED_ANDESITE,
    ANDESITE,
    SMOOTH_STONE,
    BRICK,
    RED_NETHER_BRICKS,
    MOSSY_STONE_BRICKS,
    MOSSY_COBBLESTONE,
    COBBLED_DEEPSLATE,
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
    TallBuilding,           // Tall buildings (>7 floors or >28m)
    GlassySkyscraper,       // Glass-facade skyscrapers
    GlassCornerSkyscraper,  // Glass facade with concrete corner pillars
    GridSkyscraper,         // Large glass panes in a concrete grid
    ContemporarySkyscraper, // Concrete/light-stone frame with wide glass windows
    ModernSkyscraper,       // Horizontal-window skyscrapers with stone bands
    MasonrySkyscraper,      // Stone/art-deco towers, from historic/material OSM tags
    Historic,               // Castles, ruins, historic buildings
    Tower,                  // man_made=tower or building=tower (stone towers)
    Garage,                 // Garages and carports
    Shed,                   // Sheds, huts, simple storage
    Greenhouse,             // Greenhouses and glasshouses

    Default, // Unknown or generic buildings
}

impl BuildingCategory {
    /// Determines the building category from OSM tags and calculated properties
    fn from_element(
        element: &ProcessedWay,
        is_tall_building: bool,
        building_height: i32,
        group_seed: u64,
        scale_factor: f64,
    ) -> Self {
        // Check for man_made=tower before anything else
        if element.tags.get("man_made").map(|s| s.as_str()) == Some("tower") {
            return BuildingCategory::Tower;
        }

        if is_tall_building {
            // OSM tags can pin the facade style. The seed carries the building's decision to all
            // its S3DB parts; the element's own tags cover standalone buildings.
            use crate::osm_parser::StyleHint;
            let mut hint = crate::osm_parser::style_hint_from_seed(group_seed);
            if hint == StyleHint::None {
                hint = crate::osm_parser::building_style_hint(&element.tags);
            }
            let clean_seed = crate::osm_parser::seed_without_hint(group_seed);
            match hint {
                // Tagged glass towers stay glass, but vary the treatment for a livelier skyline.
                StyleHint::Glass => return Self::glass_family_variant(clean_seed),
                StyleHint::Masonry => return BuildingCategory::MasonrySkyscraper,
                StyleHint::Contemporary => return BuildingCategory::ContemporarySkyscraper,
                StyleHint::None => {}
            }

            // Check if this qualifies as a true skyscraper:
            // Must be significantly tall AND have skyscraper proportions
            // (taller than twice its longest side dimension)
            let is_true_skyscraper = building_height >= multiply_scale(120, scale_factor)
                && Self::has_skyscraper_proportions(element, building_height);

            if is_true_skyscraper {
                // shared seed so parts of one tower pick the same variant
                let hash = clean_seed.wrapping_mul(2654435761); // Knuth multiplicative hash
                return match hash % 100 {
                    0..=17 => BuildingCategory::GlassySkyscraper,
                    18..=29 => BuildingCategory::GlassCornerSkyscraper,
                    30..=44 => BuildingCategory::GridSkyscraper,
                    45..=69 => BuildingCategory::ContemporarySkyscraper,
                    70..=84 => BuildingCategory::ModernSkyscraper,
                    _ => BuildingCategory::TallBuilding,
                };
            }

            return BuildingCategory::TallBuilding;
        }

        // Religious buildings keep their style even when also tagged historic.
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
    /// Picks a glass-family treatment (pure curtain, concrete corners, or grid) from the shared seed.
    fn glass_family_variant(seed: u64) -> BuildingCategory {
        // Even split so a formerly all-glass tower is usually a grid or corner variant.
        match (seed ^ 0x6C07_A55E).wrapping_mul(2654435761) % 3 {
            0 => BuildingCategory::GlassySkyscraper,
            1 => BuildingCategory::GridSkyscraper,
            _ => BuildingCategory::GlassCornerSkyscraper,
        }
    }

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
            ..Default::default()
        }
    }

    /// Preset for glass-facade skyscrapers
    pub fn glassy_skyscraper() -> Self {
        Self {
            has_windows: Some(false),
            use_vertical_accent: Some(false),
            use_accent_roof_line: Some(true),
            roof_type: Some(RoofType::Flat),
            generate_roof: Some(true),
            has_chimney: Some(false),
            wall_depth_style: Some(WallDepthStyle::GlassCurtain),
            has_parapet: Some(true),
            ..Default::default()
        }
    }

    /// Glass tower with concrete corner pillars (GlassCurtain places accent_block at the corners).
    pub fn glass_corner_skyscraper() -> Self {
        Self {
            accent_block: Some(LIGHT_GRAY_CONCRETE),
            has_windows: Some(false),
            use_vertical_accent: Some(false),
            use_accent_roof_line: Some(true),
            roof_type: Some(RoofType::Flat),
            generate_roof: Some(true),
            has_chimney: Some(false),
            wall_depth_style: Some(WallDepthStyle::GlassCurtain),
            has_parapet: Some(true),
            ..Default::default()
        }
    }

    /// Large glass panes in a concrete grid (mullions at floor lines and every few columns).
    pub fn grid_skyscraper() -> Self {
        Self {
            window_block: Some(LIGHT_BLUE_STAINED_GLASS),
            has_windows: Some(true),
            use_vertical_windows: Some(false),
            use_horizontal_windows: Some(false),
            use_accent_roof_line: Some(true),
            roof_type: Some(RoofType::Flat),
            generate_roof: Some(true),
            has_chimney: Some(false),
            has_garage_door: Some(false),
            has_single_door: Some(false),
            wall_depth_style: Some(WallDepthStyle::None),
            has_parapet: Some(true),
            ..Default::default()
        }
    }

    /// Preset for contemporary towers: concrete/light-stone piers with wide glass windows.
    pub fn contemporary_skyscraper() -> Self {
        Self {
            window_block: Some(LIGHT_BLUE_STAINED_GLASS),
            has_windows: Some(true),
            use_vertical_windows: Some(false),
            use_horizontal_windows: Some(false),
            use_accent_roof_line: Some(true),
            roof_type: Some(RoofType::Flat),
            generate_roof: Some(true),
            has_chimney: Some(false),
            has_garage_door: Some(false),
            has_single_door: Some(false),
            wall_depth_style: Some(WallDepthStyle::ModernPillars),
            has_parapet: Some(true),
            ..Default::default()
        }
    }

    /// Preset for stone/art-deco towers (historic + masonry-tagged tall buildings).
    pub fn masonry_skyscraper() -> Self {
        Self {
            // wall_block None so building:material / building:colour still win; palette from category.
            floor_block: Some(SMOOTH_STONE), // stops the setback crown capping tiers in oak planks
            window_block: Some(LIGHT_GRAY_STAINED_GLASS),
            accent_block: Some(SMOOTH_STONE),
            has_windows: Some(true),
            use_vertical_windows: Some(false),
            use_horizontal_windows: Some(false),
            use_accent_lines: Some(true),
            use_accent_roof_line: Some(true),
            roof_type: Some(RoofType::Flat),
            generate_roof: Some(true),
            has_chimney: Some(false),
            has_garage_door: Some(false),
            has_single_door: Some(false),
            wall_depth_style: Some(WallDepthStyle::HistoricOrnate),
            has_parapet: Some(true),
            ..Default::default()
        }
    }

    /// Preset for industrial buildings (warehouses, factories)
    pub fn industrial() -> Self {
        Self {
            roof_type: None,
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
            roof_type: None,
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

    /// Preset for man_made=tower buildings.
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
            BuildingCategory::GlassCornerSkyscraper => Self::glass_corner_skyscraper(),
            BuildingCategory::GridSkyscraper => Self::grid_skyscraper(),
            BuildingCategory::ContemporarySkyscraper => Self::contemporary_skyscraper(),
            BuildingCategory::ModernSkyscraper => Self::modern_skyscraper(),
            BuildingCategory::MasonrySkyscraper => Self::masonry_skyscraper(),
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
        era: ArchEra,
        climate: Climate,
        detail: DetailTier,
        building_height: i32,
        has_multiple_floors: bool,
        footprint_size: usize,
        style_seed: u64,
        rng: &mut impl Rng,
    ) -> Self {
        // === Block Palette ===

        // Priority: OSM tag > preset > category palette.
        let wall_block = determine_wall_block_from_tags(element, category, rng)
            .or(preset.wall_block)
            .unwrap_or_else(|| determine_wall_block(element, category, era, climate, rng));

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

        // Window block: from preset or random based on building category (tint coordinated below).
        let window_block = preset.window_block.unwrap_or_else(|| {
            let pool = window_pool_for_category(category);
            pool[rng.random_range(0..pool.len())]
        });

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

        // Concrete/modern towers tint their glass dark when the wall or the accent bands are
        // dark, so a blackstone-banded tower reads with dark glass instead of bright blue.
        let window_block = if matches!(
            category,
            BuildingCategory::ContemporarySkyscraper
                | BuildingCategory::GridSkyscraper
                | BuildingCategory::ModernSkyscraper
        ) {
            coordinated_window_block(wall_block, accent_block, window_block)
        } else {
            window_block
        };

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

        // Priority: roof:shape tag, preset default, auto-gable, then flat.
        let (roof_type, generate_roof) = if let Some(roof_shape) = element.tags.get("roof:shape") {
            (parse_roof_type(roof_shape), true)
        } else if let Some(rt) = preset.roof_type {
            let should_generate = preset.generate_roof.unwrap_or(rt != RoofType::Flat);
            (rt, should_generate)
        } else if element.tags.contains_key("building:part")
            && (element.tags.contains_key("height") || element.tags.contains_key("building:levels"))
        {
            // Parts with an explicit top are modeled volumes, flat by default
            (RoofType::Flat, false)
        } else if qualifies_for_auto_gabled_roof(building_type) {
            const MAX_FOOTPRINT_FOR_GABLED: usize = 800;
            // Own stream: the draw count here depends on footprint and would
            // otherwise desync sibling parts.
            let mut roof_rng = element_rng(style_seed ^ 0x0F1E_2D3C_4B5A_6907);
            let big_block = footprint_size > MAX_FOOTPRINT_FOR_GABLED || building_height >= 15;
            if building_type == "apartments" && big_block {
                // Urban apartment blocks: flat, hipped, or (pre-war) mansard.
                let (flat_w, hip_w) =
                    if matches!(era, ArchEra::HistoricOrnate | ArchEra::TraditionalPreWar) {
                        (45, 20)
                    } else {
                        (45, 35)
                    };
                let roll = roof_rng.random_range(0u32..100);
                if roll < flat_w {
                    (RoofType::Flat, false)
                } else if roll < flat_w + hip_w {
                    (RoofType::Hipped, true)
                } else {
                    (RoofType::Mansard, true)
                }
            } else if matches!(era, ArchEra::HistoricOrnate | ArchEra::TraditionalPreWar)
                && (100..=MAX_FOOTPRINT_FOR_GABLED).contains(&footprint_size)
                && roof_rng.random_bool(0.30)
            {
                // Mid-size pre-war fabric occasionally carries a mansard.
                (RoofType::Mansard, true)
            } else if footprint_size <= MAX_FOOTPRINT_FOR_GABLED
                && roof_rng.random_bool(climate_gable_probability(climate))
            {
                (RoofType::Gabled, true)
            } else {
                (RoofType::Flat, false)
            }
        } else if matches!(building_type, "industrial" | "warehouse" | "hangar")
            && footprint_size > 800
        {
            // Big industrial halls: shallow mono-pitch or flat.
            let mut roof_rng = element_rng(style_seed ^ 0x0F1E_2D3C_4B5A_6907);
            if roof_rng.random_bool(0.55) {
                (RoofType::Skillion, true)
            } else {
                (RoofType::Flat, false)
            }
        } else {
            (RoofType::Flat, false)
        };

        // downgrade only truly rotated shapes, not concave axis-aligned ones
        let has_explicit_roof_shape = element.tags.contains_key("roof:shape");
        const DIAGONAL_THRESHOLD: f64 = 0.35;
        let diagonality = compute_building_diagonality(&element.nodes);
        let roof_type = if !has_explicit_roof_shape
            && matches!(
                roof_type,
                RoofType::Gabled | RoofType::Hipped | RoofType::Mansard | RoofType::Gambrel
            )
            && diagonality < DIAGONAL_THRESHOLD
            && dominant_axis_angle(&element.nodes).to_degrees().abs() > 10.0
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
            let suitable_roof = matches!(
                roof_type,
                RoofType::Gabled
                    | RoofType::Hipped
                    | RoofType::Mansard
                    | RoofType::Gambrel
                    | RoofType::HalfHipped
            );
            let suitable_size = (30..=400).contains(&footprint_size);

            is_residential && suitable_roof && suitable_size && rng.random_bool(0.40)
        });

        // Roof block: specific material for roofs
        let roof_block = preset.roof_block;

        // Windows: default to true unless explicitly disabled
        let has_windows = preset.has_windows.unwrap_or(true);

        // Suppress preset doors when an entrance/door node is mapped on the outline.
        let has_mapped_entrance = outline_has_mapped_entrance(element);
        let has_garage_door = if has_mapped_entrance {
            false
        } else {
            preset.has_garage_door.unwrap_or(false)
        };
        let has_single_door = if has_mapped_entrance {
            false
        } else {
            preset.has_single_door.unwrap_or(false)
        };

        // Wall depth style: default based on category and era (preset may override)
        let wall_depth_style = preset.wall_depth_style.unwrap_or_else(|| {
            if footprint_size < 20 || detail == DetailTier::Minimal {
                WallDepthStyle::None
            } else {
                // Era overrides for the general urban fabric: heritage gets
                // ornate relief, panel-era facades stay flat.
                match (era, category) {
                    (
                        ArchEra::HistoricOrnate,
                        BuildingCategory::House
                        | BuildingCategory::Residential
                        | BuildingCategory::Commercial,
                    ) => return WallDepthStyle::HistoricOrnate,
                    (
                        ArchEra::PostWarPanel,
                        BuildingCategory::House | BuildingCategory::Residential,
                    ) => return WallDepthStyle::None,
                    (ArchEra::Contemporary, BuildingCategory::Residential)
                        if element_rng(style_seed ^ 0xE5A0_11DE_57A1_0003).random_bool(0.50) =>
                    {
                        return WallDepthStyle::ModernPillars;
                    }
                    _ => {}
                }
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
#[derive(Clone)]
struct BuildingConfig {
    /// True when the building starts at ground level (no min_height / min_level offset).
    /// When false, foundation pillars should not be generated.
    is_ground_level: bool,
    building_height: i32,
    /// Gross block height of one upper floor (see `floor_cycle_for`).
    floor_cycle: i32,
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
    /// Architectural era from tags (weathering, and style decisions upstream).
    era: ArchEra,
    /// Decorative budget from prominence (shutters, rooftop bits, two-tone).
    detail: DetailTier,
    /// Narrower top-floor windows with an accent band below them.
    top_treatment: bool,
    /// Solid attic band with small lights under a pitched roof.
    attic_style: bool,
    /// Taller, wider windows on the first floor above ground (piano nobile).
    piano_nobile: bool,
    wall_depth_style: WallDepthStyle,
    has_parapet: bool,
    has_lobby_base: bool,
    condition: BuildingCondition,
    element_id: u64,
    // shared across all parts of one building for coherent roof style
    style_seed: u64,
    /// Per-building offset of the window rhythm so facades don't align citywide.
    window_phase: i32,
    /// Per-building window layout on the shared lattice.
    window_archetype: WindowArchetype,
    /// Balcony layout on street facades.
    balcony_band: BalconyBand,
    /// Banded stone ground floor on masonry-era urban buildings.
    rustication: bool,
    /// Darker plinth block for the bottom wall rows, None to skip.
    base_course_block: Option<Block>,
    /// Wider, taller glass on the ground floor of commercial buildings.
    has_storefront: bool,
    /// Per-building window dressing style, derived from hand-built reference frames.
    window_frame: Option<WindowFrameStyle>,
}

/// Window frame styles distilled from the reference schematics, one per building.
#[derive(Copy, Clone, PartialEq, Eq)]
enum WindowFrameStyle {
    SpruceCottage,
    DarkTimber,
    StoneOrnate,
    Blackstone,
    RusticMossy,
    TerracottaCopper,
    QuartzModern,
}

impl WindowFrameStyle {
    /// Post block at flanking columns, None when the style uses shutters there.
    fn post_block(self) -> Option<Block> {
        match self {
            Self::SpruceCottage | Self::TerracottaCopper => None,
            Self::DarkTimber => Some(DARK_OAK_PLANKS),
            Self::StoneOrnate => Some(POLISHED_DIORITE),
            Self::Blackstone => Some(POLISHED_BLACKSTONE),
            Self::RusticMossy => Some(MOSSY_COBBLESTONE),
            Self::QuartzModern => Some(QUARTZ_BLOCK),
        }
    }

    /// Trapdoor shutters at flanking columns.
    fn shutter_block(self) -> Option<Block> {
        match self {
            Self::SpruceCottage => Some(SPRUCE_TRAPDOOR),
            Self::TerracottaCopper => Some(JUNGLE_TRAPDOOR),
            _ => None,
        }
    }

    /// Material for the upside-down stair band over each window.
    fn band_material(self) -> Block {
        match self {
            Self::SpruceCottage => SPRUCE_PLANKS,
            Self::DarkTimber => DARK_OAK_PLANKS,
            Self::StoneOrnate => STONE_BRICKS,
            Self::Blackstone => BLACKSTONE,
            Self::RusticMossy => COBBLESTONE,
            Self::TerracottaCopper => WAXED_COPPER_BLOCK,
            Self::QuartzModern => QUARTZ_BLOCK,
        }
    }

    fn has_lanterns(self) -> bool {
        matches!(self, Self::SpruceCottage | Self::TerracottaCopper)
    }

    /// Trapdoor material for shelves, canopies and aprons.
    fn detail_trapdoor(self) -> Block {
        match self {
            Self::DarkTimber => DARK_OAK_TRAPDOOR,
            Self::Blackstone => WARPED_TRAPDOOR,
            Self::QuartzModern => IRON_TRAPDOOR,
            Self::TerracottaCopper => JUNGLE_TRAPDOOR,
            _ => SPRUCE_TRAPDOOR,
        }
    }

    /// Lantern hung under the band beside a window top.
    fn hanging_lantern(self) -> Option<Block> {
        match self {
            Self::SpruceCottage | Self::TerracottaCopper | Self::StoneOrnate => Some(LANTERN),
            Self::Blackstone => Some(SOUL_LANTERN),
            _ => None,
        }
    }

    /// Button studs on the band front.
    fn stud_button(self) -> Option<Block> {
        match self {
            Self::RusticMossy | Self::StoneOrnate => Some(STONE_BUTTON),
            Self::Blackstone => Some(POLISHED_BLACKSTONE_BUTTON),
            _ => None,
        }
    }
}

/// Whether a frame style's trim harmonizes with the wall it sits on.
fn frame_fits_wall(style: WindowFrameStyle, wall: Block) -> bool {
    use WindowFrameStyle::*;
    match wall {
        WHITE_CONCRETE | LIGHT_GRAY_CONCRETE | GRAY_CONCRETE | QUARTZ_BLOCK | QUARTZ_BRICKS
        | SMOOTH_STONE | POLISHED_ANDESITE | POLISHED_DIORITE => {
            matches!(style, QuartzModern | Blackstone | StoneOrnate)
        }
        OAK_PLANKS | SPRUCE_PLANKS | DARK_OAK_PLANKS | OAK_LOG | SPRUCE_LOG => {
            matches!(style, SpruceCottage | DarkTimber | RusticMossy)
        }
        BRICK | RED_TERRACOTTA | ORANGE_TERRACOTTA | TERRACOTTA | BROWN_TERRACOTTA
        | RED_NETHER_BRICKS | NETHER_BRICK | GRANITE | POLISHED_GRANITE => {
            matches!(style, StoneOrnate | TerracottaCopper | DarkTimber)
        }
        SANDSTONE | SMOOTH_SANDSTONE | END_STONE_BRICKS | WHITE_TERRACOTTA => {
            matches!(style, StoneOrnate | TerracottaCopper | QuartzModern)
        }
        _ => true,
    }
}

/// Picks a per-building frame style suited to category, era and wall.
fn pick_window_frame(
    category: BuildingCategory,
    era: ArchEra,
    detail: DetailTier,
    wall_block: Block,
    element_id: u64,
) -> Option<WindowFrameStyle> {
    if detail == DetailTier::Minimal {
        return None;
    }
    use WindowFrameStyle::*;
    // Category gate first: only these get dressed frames at all.
    if !matches!(
        category,
        BuildingCategory::House
            | BuildingCategory::Residential
            | BuildingCategory::Commercial
            | BuildingCategory::Hotel
            | BuildingCategory::Historic
    ) {
        return None;
    }
    // A known era narrows the pool and shifts how often frames appear:
    // heritage facades are almost always dressed, panel-era ones rarely.
    let (pool, chance): (&[WindowFrameStyle], f64) = match era {
        ArchEra::HistoricOrnate => (&[StoneOrnate, RusticMossy], 0.95),
        ArchEra::TraditionalPreWar => (
            &[
                SpruceCottage,
                DarkTimber,
                StoneOrnate,
                RusticMossy,
                TerracottaCopper,
            ],
            0.90,
        ),
        ArchEra::PostWarPanel => (&[QuartzModern], 0.15),
        ArchEra::Contemporary => (&[QuartzModern, Blackstone], 0.70),
        ArchEra::Unknown => (
            match category {
                BuildingCategory::House | BuildingCategory::Residential => &[
                    SpruceCottage,
                    DarkTimber,
                    StoneOrnate,
                    RusticMossy,
                    TerracottaCopper,
                ],
                BuildingCategory::Commercial | BuildingCategory::Hotel => {
                    &[QuartzModern, Blackstone, StoneOrnate]
                }
                _ => &[RusticMossy, StoneOrnate, Blackstone],
            },
            0.90,
        ),
    };
    let chance = match detail {
        DetailTier::Enhanced => (chance + 0.15).min(0.95),
        DetailTier::Landmark => (chance + 0.30).min(0.95),
        _ => chance,
    };
    let fitting: Vec<WindowFrameStyle> = pool
        .iter()
        .copied()
        .filter(|&f| frame_fits_wall(f, wall_block))
        .collect();
    if fitting.is_empty() {
        return None;
    }
    let mut rng = element_rng(element_id ^ 0xF7A3_E001_57BD_2210);
    rng.random_bool(chance)
        .then(|| fitting[rng.random_range(0..fitting.len())])
}

impl BuildingConfig {
    /// Grammar anchor: +2 at ground level; elevated parts already carry the
    /// bonus in their min_level offset, keeping stacked bands in phase.
    #[inline]
    fn grammar_anchor(&self) -> i32 {
        if self.is_ground_level {
            GROUND_FLOOR_BONUS
        } else {
            0
        }
    }

    /// Returns the position within the floor cycle (0 = floor row, 1..cycle-1 = open rows).
    /// This aligns with `generate_floors_and_ceilings` which places intermediate ceilings
    /// at `start_y_offset + anchor + cycle, + 2*cycle, …`.
    #[inline]
    fn floor_row(&self, h: i32) -> i32 {
        ((h - self.start_y_offset - self.grammar_anchor()) % self.floor_cycle + self.floor_cycle)
            % self.floor_cycle
    }

    /// Highest wall row that still belongs to the ground floor (the ground floor
    /// is one row taller than upper floors thanks to the +2 grammar offset).
    #[inline]
    fn ground_floor_top(&self) -> i32 {
        self.start_y_offset + self.grammar_anchor() - 1 + self.floor_cycle
    }

    /// Positional role of a wall row: ground floor, body, or topmost cycle.
    #[inline]
    fn floor_role(&self, h: i32) -> FloorRole {
        if h <= self.ground_floor_top() {
            FloorRole::Ground
        } else if h > self.start_y_offset + self.building_height - self.floor_cycle {
            FloorRole::Top
        } else {
            FloorRole::Body
        }
    }

    /// Position within the 6-block window cycle (0-2 = window strip, 3-5 = wall pier).
    #[inline]
    fn window_col(&self, bx: i32, bz: i32) -> i32 {
        (bx + bz + self.window_phase).rem_euclid(6)
    }

    /// Number of darker plinth rows at the wall base (2 from roughly three floors up).
    #[inline]
    fn base_course_rows(&self) -> i32 {
        if self.building_height >= 3 * self.floor_cycle {
            2
        } else {
            1
        }
    }
}

// Never render as buildings: Eiffel Tower, London Eye, Utah Capitol, Starbase Pad 2 trench + mount ring.
const SKIP_WAY_IDS: &[u64] = &[5013364, 204068874, 32920861, 1352374225, 1486731987];

/// Darker stone family plinth matching the wall material tone.
fn base_course_for_wall(wall: Block) -> Block {
    match wall {
        OAK_PLANKS | SPRUCE_PLANKS | OAK_LOG | SPRUCE_LOG => COBBLESTONE,
        ANDESITE | GRAY_CONCRETE | LIGHT_GRAY_CONCRETE => POLISHED_ANDESITE,
        _ => STONE_BRICKS,
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
pub(crate) fn is_underground_building(tags: &HashMap<String, String>) -> bool {
    // An explicit surface location wins over layer=-1, which is only stacking order
    match tags.get("location").map(String::as_str) {
        Some("underground") | Some("subway") => return true,
        Some("surface") | Some("overground") | Some("roof") => return false,
        _ => {}
    }

    // Check layer tag, negative means underground
    if let Some(layer) = tags.get("layer") {
        if layer.parse::<i32>().unwrap_or(0) < 0 {
            return true;
        }
    }

    // Check level tag, negative means underground
    if let Some(level) = tags.get("level") {
        if level.parse::<i32>().unwrap_or(0) < 0 {
            return true;
        }
    }

    // Check building:levels:underground, if this is the only levels tag, it's underground
    if tags.contains_key("building:levels:underground") && !tags.contains_key("building:levels") {
        return true;
    }

    false
}

/// Calculates the starting Y offset based on terrain and min_level
pub(crate) fn calculate_start_y_offset(
    editor: &WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    min_level_offset: i32,
) -> i32 {
    if args.terrain() {
        let mut max_ground_level = args.ground_level;
        for node in &element.nodes {
            if let Some(level) = editor.terrain_level(node.x, node.z) {
                max_ground_level = max_ground_level.max(level);
            }
        }
        max_ground_level + min_level_offset
    } else {
        min_level_offset
    }
}

/// Window glass pool per building category.
fn window_pool_for_category(category: BuildingCategory) -> &'static [Block] {
    match category {
        BuildingCategory::Residential | BuildingCategory::House => &RESIDENTIAL_WINDOW_OPTIONS,
        BuildingCategory::School
        | BuildingCategory::Hospital
        | BuildingCategory::Office
        | BuildingCategory::Commercial => &INSTITUTIONAL_WINDOW_OPTIONS,
        BuildingCategory::Hotel => &HOSPITALITY_WINDOW_OPTIONS,
        BuildingCategory::Industrial | BuildingCategory::Warehouse => &INDUSTRIAL_WINDOW_OPTIONS,
        BuildingCategory::Religious => &RELIGIOUS_WINDOW_OPTIONS,
        BuildingCategory::Farm => &FARM_WINDOW_OPTIONS,
        BuildingCategory::Historic => &HISTORIC_WINDOW_OPTIONS,
        _ => &WINDOW_VARIATIONS,
    }
}

/// Balcony layout for multi-storey residential street facades. A repeated
/// pattern reads as architecture where scattered singles read as noise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BalconyBand {
    /// Legacy per-window random balconies.
    Scattered,
    /// A balcony on every street-facing bay of every upper floor.
    EveryBay,
    /// Balconies on every other bay, stacked vertically.
    Alternating,
}

fn pick_balcony_band(
    category: BuildingCategory,
    building_height: i32,
    floor_cycle: i32,
    has_street: bool,
    group_seed: u64,
) -> BalconyBand {
    if !matches!(
        category,
        BuildingCategory::Residential | BuildingCategory::House
    ) || building_height < 3 * floor_cycle
        || !has_street
    {
        return BalconyBand::Scattered;
    }
    let roll = element_rng(group_seed ^ 0xBA1C_0417_0000_0009).random_range(0u32..100);
    if roll < 30 {
        BalconyBand::Scattered
    } else if roll < 60 {
        BalconyBand::EveryBay
    } else {
        BalconyBand::Alternating
    }
}

/// Per-building window layout on the shared 6-column / floor-cycle lattice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowArchetype {
    /// Classic 3-wide bays over the full window rows.
    Standard3,
    /// Two 1-wide sashes (cols 0 and 2) with a lintel row above.
    PairedNarrow,
    /// Single centered 1-wide strip over the full window rows.
    VerticalStrip,
    /// 4-wide band windows with a sill row below.
    WideHorizontal,
    /// Like Standard3 plus protruding stair headers over the window tops.
    ArchedTraditional,
}

/// Whether (window_col, floor_row) is glass under the given archetype.
/// `floor_row` 0 is always the solid band; rows 1..cycle-1 are open rows.
fn archetype_allows_window(
    archetype: WindowArchetype,
    window_col: i32,
    floor_row: i32,
    floor_cycle: i32,
) -> bool {
    match archetype {
        WindowArchetype::Standard3 | WindowArchetype::ArchedTraditional => window_col < 3,
        // Drop the top open row: it reads as a lintel over the two sashes.
        WindowArchetype::PairedNarrow => {
            (window_col == 0 || window_col == 2) && floor_row < floor_cycle - 1
        }
        WindowArchetype::VerticalStrip => window_col == 1,
        // Drop the bottom open row: it reads as a sill under the band window.
        WindowArchetype::WideHorizontal => window_col < 4 && floor_row > 1,
    }
}

/// Weighted per-category archetype choice, seeded on the shared group seed.
fn pick_window_archetype(
    category: BuildingCategory,
    era: ArchEra,
    group_seed: u64,
) -> WindowArchetype {
    use WindowArchetype::*;
    // (archetype, weight), summing to 100 per row
    let table: &[(WindowArchetype, u32)] = match category {
        BuildingCategory::House => &[
            (Standard3, 40),
            (PairedNarrow, 35),
            (VerticalStrip, 5),
            (WideHorizontal, 10),
            (ArchedTraditional, 10),
        ],
        BuildingCategory::Residential => &[
            (Standard3, 35),
            (PairedNarrow, 30),
            (VerticalStrip, 10),
            (WideHorizontal, 15),
            (ArchedTraditional, 10),
        ],
        BuildingCategory::Commercial | BuildingCategory::Office => &[
            (Standard3, 25),
            (PairedNarrow, 5),
            (VerticalStrip, 20),
            (WideHorizontal, 45),
            (ArchedTraditional, 5),
        ],
        BuildingCategory::Hotel => &[
            (Standard3, 30),
            (PairedNarrow, 10),
            (VerticalStrip, 20),
            (WideHorizontal, 35),
            (ArchedTraditional, 5),
        ],
        BuildingCategory::School | BuildingCategory::Hospital => &[
            (Standard3, 30),
            (PairedNarrow, 10),
            (VerticalStrip, 10),
            (WideHorizontal, 50),
        ],
        BuildingCategory::Industrial | BuildingCategory::Warehouse => {
            &[(Standard3, 20), (VerticalStrip, 10), (WideHorizontal, 70)]
        }
        _ => return Standard3,
    };
    let mut rng = element_rng(group_seed ^ 0x57A2_C0DE_A5C1_0007);
    let total: u32 = table.iter().map(|&(_, w)| w).sum();
    let mut roll = rng.random_range(0..total);
    let mut picked = Standard3;
    for &(archetype, weight) in table {
        if roll < weight {
            picked = archetype;
            break;
        }
        roll -= weight;
    }
    // Era adjustment on top of the category weights.
    let mut era_rng = element_rng(group_seed ^ 0x57A2_C0DE_A5C1_0008);
    match (era, picked) {
        (ArchEra::HistoricOrnate, Standard3) if era_rng.random_bool(0.75) => ArchedTraditional,
        (ArchEra::TraditionalPreWar, Standard3) if era_rng.random_bool(0.25) => ArchedTraditional,
        (ArchEra::PostWarPanel, PairedNarrow) => WideHorizontal,
        (ArchEra::Contemporary, Standard3) if era_rng.random_bool(0.25) => WideHorizontal,
        _ => picked,
    }
}

/// Tints a tower's glass to match a dark wall or dark accent band; else keeps the light default.
fn coordinated_window_block(wall_block: Block, accent_block: Block, light_default: Block) -> Block {
    const DARK: &[Block] = &[
        BLACK_CONCRETE,
        GRAY_CONCRETE,
        BLACKSTONE,
        POLISHED_BLACKSTONE,
        DEEPSLATE_BRICKS,
        NETHER_BRICK,
        BLACK_TERRACOTTA,
        GRAY_TERRACOTTA,
    ];
    if DARK.contains(&wall_block) || DARK.contains(&accent_block) {
        GRAY_STAINED_GLASS
    } else {
        light_default
    }
}

/// Wall block from an OSM material/colour tag, or None if no tag is set.
fn determine_wall_block_from_tags(
    element: &ProcessedWay,
    category: BuildingCategory,
    rng: &mut impl Rng,
) -> Option<Block> {
    if category == BuildingCategory::GlassySkyscraper {
        // GlassySkyscraper walls must stay glass.
        return None;
    }
    if element.tags.get("historic").map(String::as_str) == Some("castle") {
        return None;
    }
    if let Some(material) = element
        .tags
        .get("building:material")
        .or_else(|| element.tags.get("building:facade:material"))
        .or_else(|| element.tags.get("facade:material"))
    {
        if let Some(block) = get_wall_block_for_material(material, rng) {
            return Some(block);
        }
    }
    let colour = element
        .tags
        .get("building:colour")
        .or_else(|| element.tags.get("colour"));
    if let Some(building_colour) = colour {
        if let Some(rgb) = color_text_to_rgb_tuple(building_colour) {
            return Some(crate::block_palette::wall_block_for_color(rgb, rng));
        }
    }
    None
}

/// Wall block from explicit material/colour tags, falling back to category palette.
fn determine_wall_block(
    element: &ProcessedWay,
    category: BuildingCategory,
    era: ArchEra,
    climate: Climate,
    rng: &mut impl Rng,
) -> Block {
    // Historic castles have their own special treatment
    if element.tags.get("historic").map(String::as_str) == Some("castle") {
        return get_castle_wall_block(rng);
    }

    // GlassySkyscraper walls must stay glass.
    if category != BuildingCategory::GlassySkyscraper {
        if let Some(material) = element
            .tags
            .get("building:material")
            .or_else(|| element.tags.get("building:facade:material"))
            .or_else(|| element.tags.get("facade:material"))
        {
            if let Some(block) = get_wall_block_for_material(material, rng) {
                return block;
            }
        }

        let colour = element
            .tags
            .get("building:colour")
            .or_else(|| element.tags.get("colour"));
        if let Some(building_colour) = colour {
            if let Some(rgb) = color_text_to_rgb_tuple(building_colour) {
                return crate::block_palette::wall_block_for_color(rgb, rng);
            }
        }
    }

    // Otherwise, select from category-specific palette
    get_wall_block_for_category(category, era, climate, rng)
}

/// Wall blocks that fit a building era; None for Unknown.
fn era_allow_list(era: ArchEra) -> Option<&'static [Block]> {
    let allowed: &'static [Block] = match era {
        ArchEra::Unknown => return None,
        ArchEra::TraditionalPreWar => &[
            BRICK,
            STONE_BRICKS,
            WHITE_TERRACOTTA,
            BROWN_TERRACOTTA,
            SANDSTONE,
            SMOOTH_SANDSTONE,
            MUD_BRICKS,
            GRANITE,
            POLISHED_GRANITE,
            TERRACOTTA,
            END_STONE_BRICKS,
            QUARTZ_BRICKS,
            ORANGE_TERRACOTTA,
            RED_TERRACOTTA,
            RED_NETHER_BRICKS,
            NETHER_BRICK,
            COBBLESTONE,
            OAK_PLANKS,
            SPRUCE_PLANKS,
        ],
        ArchEra::PostWarPanel => &[
            GRAY_CONCRETE,
            LIGHT_GRAY_CONCRETE,
            WHITE_CONCRETE,
            BROWN_CONCRETE,
            GRAY_TERRACOTTA,
            LIGHT_GRAY_TERRACOTTA,
            POLISHED_ANDESITE,
            SMOOTH_STONE,
            BRICK,
            WHITE_TERRACOTTA,
        ],
        ArchEra::Contemporary => &[
            WHITE_CONCRETE,
            LIGHT_GRAY_CONCRETE,
            GRAY_CONCRETE,
            QUARTZ_BLOCK,
            QUARTZ_BRICKS,
            POLISHED_ANDESITE,
            SMOOTH_STONE,
            POLISHED_DEEPSLATE,
            DEEPSLATE_BRICKS,
            POLISHED_BLACKSTONE,
            LIGHT_GRAY_TERRACOTTA,
        ],
        ArchEra::HistoricOrnate => &[
            SANDSTONE,
            SMOOTH_SANDSTONE,
            STONE_BRICKS,
            CHISELED_STONE_BRICKS,
            END_STONE_BRICKS,
            QUARTZ_BRICKS,
            WHITE_TERRACOTTA,
            BRICK,
            POLISHED_DIORITE,
            GRANITE,
            POLISHED_GRANITE,
        ],
    };
    Some(allowed)
}

/// Categories whose palettes get filtered by the building era. Specialised
/// palettes (farm wood, religious stone, industrial, glass towers) already
/// carry their identity and stay untouched.
fn era_filters_category(category: BuildingCategory) -> bool {
    matches!(
        category,
        BuildingCategory::House
            | BuildingCategory::Residential
            | BuildingCategory::Commercial
            | BuildingCategory::Office
            | BuildingCategory::Hotel
            | BuildingCategory::Default
    )
}

/// Vertical position of a wall row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloorRole {
    Ground,
    Body,
    /// The topmost floor cycle (attic/top-floor treatments hook in here).
    Top,
}

/// Decorative budget from footprint, height, category, tags and visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DetailTier {
    Minimal,
    Standard,
    Enhanced,
    Landmark,
}

fn compute_detail_tier(
    element: &ProcessedWay,
    category: BuildingCategory,
    footprint: usize,
    building_height: i32,
    street_facing: bool,
) -> DetailTier {
    let mut score: i32 = 0;
    score += (footprint as i32 / 40).min(25);
    score += (building_height * 2).min(25);
    score += match category {
        BuildingCategory::Historic | BuildingCategory::Religious => 20,
        BuildingCategory::Commercial | BuildingCategory::Hotel | BuildingCategory::Office => 10,
        BuildingCategory::Garage | BuildingCategory::Shed | BuildingCategory::Greenhouse => -20,
        _ => 0,
    };
    let tags = &element.tags;
    if tags.contains_key("wikidata") {
        score += 15;
    }
    if tags.get("tourism").is_some_and(|t| {
        matches!(
            t.as_str(),
            "attraction" | "museum" | "gallery" | "viewpoint"
        )
    }) {
        score += 15;
    }
    if tags.get("heritage").is_some_and(|v| v != "no") {
        score += 20;
    }
    if tags.get("historic").is_some_and(|v| v != "no") {
        score += 15;
    }
    if tags.contains_key("building:architecture") || tags.contains_key("architecture") {
        score += 10;
    }
    if street_facing {
        score += 10;
    }
    if score <= 24 {
        DetailTier::Minimal
    } else if score <= 55 {
        DetailTier::Standard
    } else if score <= 80 {
        DetailTier::Enhanced
    } else {
        DetailTier::Landmark
    }
}

/// Wood species a nordic climate adds to the residential palette.
const NORDIC_WOOD_ADDITIONS: [Block; 3] = [SPRUCE_PLANKS, OAK_PLANKS, DARK_OAK_PLANKS];

/// Whether climate reweighting applies to this category in this climate.
/// Residential fabric adapts everywhere; commercial only in hot climates
/// (light high-albedo bias). Temperate is handled upstream (no change at all).
fn climate_applies(climate: Climate, category: BuildingCategory) -> bool {
    match category {
        BuildingCategory::House | BuildingCategory::Residential => true,
        BuildingCategory::Commercial | BuildingCategory::Office | BuildingCategory::Hotel => {
            matches!(
                climate,
                Climate::HotDesert | Climate::HotSteppe | Climate::TropicalSavanna
            )
        }
        _ => false,
    }
}

/// Per-block weight for climate-adapted palettes (1 neutral, 0 excluded),
/// following real regional construction. Tags override upstream.
fn climate_wall_weight(climate: Climate, category: BuildingCategory, block: Block) -> u8 {
    let residential_scale = |w: u8| {
        // Multi-family blocks are less often wood than single homes.
        if category == BuildingCategory::Residential {
            (w / 2).max(1)
        } else {
            w
        }
    };
    match climate {
        Climate::Temperate => 1,
        Climate::HotDesert | Climate::HotSteppe => match block {
            SANDSTONE | SMOOTH_SANDSTONE | MUD_BRICKS | WHITE_TERRACOTTA => 4,
            WHITE_CONCRETE | END_STONE_BRICKS | TERRACOTTA => 3,
            LIGHT_GRAY_CONCRETE | QUARTZ_BRICKS | QUARTZ_BLOCK | ORANGE_TERRACOTTA
            | BROWN_TERRACOTTA => 2,
            DEEPSLATE_BRICKS
            | POLISHED_DEEPSLATE
            | POLISHED_BLACKSTONE
            | POLISHED_BLACKSTONE_BRICKS
            | NETHER_BRICK
            | RED_NETHER_BRICKS
            | RED_TERRACOTTA
            | GRAY_TERRACOTTA
            | BROWN_CONCRETE
            | SPRUCE_PLANKS
            | OAK_PLANKS
            | DARK_OAK_PLANKS => 0,
            _ => 1,
        },
        Climate::TropicalSavanna => match block {
            WHITE_CONCRETE => 4,
            WHITE_TERRACOTTA | LIGHT_GRAY_CONCRETE | BRICK => 3,
            GRAY_CONCRETE
            | TERRACOTTA
            | ORANGE_TERRACOTTA
            | LIGHT_BLUE_TERRACOTTA
            | QUARTZ_BRICKS
            | SMOOTH_SANDSTONE
            | MUD_BRICKS => 2,
            DEEPSLATE_BRICKS
            | POLISHED_DEEPSLATE
            | POLISHED_BLACKSTONE
            | POLISHED_BLACKSTONE_BRICKS
            | NETHER_BRICK
            | RED_NETHER_BRICKS
            | SPRUCE_PLANKS
            | OAK_PLANKS
            | DARK_OAK_PLANKS => 0,
            _ => 1,
        },
        Climate::Boreal => match block {
            SPRUCE_PLANKS => residential_scale(5),
            OAK_PLANKS => residential_scale(3),
            DARK_OAK_PLANKS => residential_scale(2),
            RED_TERRACOTTA => 3, // falu red, rare-filter exempt
            WHITE_CONCRETE | WHITE_TERRACOTTA | BRICK => 2,
            SANDSTONE | SMOOTH_SANDSTONE | MUD_BRICKS => 0,
            _ => 1,
        },
        Climate::Tundra | Climate::IceCap => match block {
            SPRUCE_PLANKS => residential_scale(3),
            OAK_PLANKS => residential_scale(2),
            LIGHT_BLUE_TERRACOTTA
            | RED_TERRACOTTA
            | WHITE_CONCRETE
            | GRAY_CONCRETE
            | LIGHT_GRAY_CONCRETE => 2,
            SANDSTONE | SMOOTH_SANDSTONE | MUD_BRICKS | DARK_OAK_PLANKS => 0,
            _ => 1,
        },
        Climate::ColdSteppe | Climate::ColdDesert | Climate::DryContinental => match block {
            BRICK => 3,
            WHITE_TERRACOTTA | WHITE_CONCRETE | LIGHT_GRAY_CONCRETE | GRAY_CONCRETE
            | STONE_BRICKS | MUD_BRICKS => 2,
            SPRUCE_PLANKS | OAK_PLANKS | DARK_OAK_PLANKS => 0,
            _ => 1,
        },
    }
}

/// Weighted wall pick with the same rare-block damping as
/// `pick_with_rare_filter`; `rare_exempt` lets a climate keep one rare block
/// at full frequency (boreal falu red).
fn pick_weighted_wall(
    pool: &[(Block, u8)],
    rare_exempt: Option<Block>,
    rng: &mut impl Rng,
) -> Block {
    let total: u32 = pool.iter().map(|&(_, w)| w as u32).sum();
    for _ in 0..8 {
        let mut roll = rng.random_range(0..total);
        let mut picked = pool[0].0;
        for &(b, w) in pool {
            if roll < w as u32 {
                picked = b;
                break;
            }
            roll -= w as u32;
        }
        if !is_rare_wall_block(picked) || Some(picked) == rare_exempt || rng.random_bool(0.20) {
            return picked;
        }
    }
    pool[0].0
}

/// Probability that an auto-gabled candidate actually gets the pitched roof;
/// flat roofs dominate arid construction, pitched roofs the snowy north.
fn climate_gable_probability(climate: Climate) -> f64 {
    match climate {
        Climate::HotDesert | Climate::HotSteppe => 0.35,
        Climate::TropicalSavanna => 0.60,
        Climate::Boreal | Climate::Tundra | Climate::IceCap => 0.95,
        _ => 0.90,
    }
}

/// Walls accepted only ~20% of the time when picked, to keep them rare.
#[inline]
fn is_rare_wall_block(block: Block) -> bool {
    matches!(block, NETHER_BRICK | RED_NETHER_BRICKS | RED_TERRACOTTA)
}

/// Picks from `palette`, biasing against `is_rare_wall_block` entries.
fn pick_with_rare_filter<R: Rng>(palette: &[Block], rng: &mut R) -> Block {
    debug_assert!(!palette.is_empty());
    let mut last = palette[rng.random_range(0..palette.len())];
    for _ in 0..8 {
        if !is_rare_wall_block(last) || rng.random_bool(0.20) {
            return last;
        }
        last = palette[rng.random_range(0..palette.len())];
    }
    last
}

/// Selects a wall block from the appropriate category palette
fn get_wall_block_for_category(
    category: BuildingCategory,
    era: ArchEra,
    climate: Climate,
    rng: &mut impl Rng,
) -> Block {
    // Climate path (never Temperate, which keeps the legacy draw sequence
    // exactly): reweighted palette intersected with the era allow-list.
    if climate != Climate::Temperate && climate_applies(climate, category) {
        let palette: &[Block] = match category {
            BuildingCategory::House | BuildingCategory::Residential => &RESIDENTIAL_WALL_OPTIONS,
            _ => &COMMERCIAL_WALL_OPTIONS,
        };
        let nordic = matches!(climate, Climate::Boreal | Climate::Tundra | Climate::IceCap)
            && matches!(
                category,
                BuildingCategory::House | BuildingCategory::Residential
            );
        let allow = era_allow_list(era);
        let extra: &[Block] = if nordic { &NORDIC_WOOD_ADDITIONS } else { &[] };
        let pool: Vec<(Block, u8)> = palette
            .iter()
            .chain(extra.iter())
            .copied()
            .filter(|b| allow.is_none_or(|a| a.contains(b)))
            .map(|b| (b, climate_wall_weight(climate, category, b)))
            .filter(|&(_, w)| w > 0)
            .collect();
        if pool.len() >= 2 {
            let rare_exempt =
                matches!(climate, Climate::Boreal | Climate::Tundra | Climate::IceCap)
                    .then_some(RED_TERRACOTTA);
            return pick_weighted_wall(&pool, rare_exempt, rng);
        }
    }

    // Era path: intersect the category palette with the era allow-list; thin
    // intersections draw from the era list directly.
    if era_filters_category(category) {
        if let Some(allow) = era_allow_list(era) {
            let palette: &[Block] = match category {
                BuildingCategory::House | BuildingCategory::Residential => {
                    &RESIDENTIAL_WALL_OPTIONS
                }
                BuildingCategory::Commercial
                | BuildingCategory::Office
                | BuildingCategory::Hotel => &COMMERCIAL_WALL_OPTIONS,
                _ => &[],
            };
            let filtered: Vec<Block> = palette
                .iter()
                .copied()
                .filter(|b| allow.contains(b))
                .collect();
            return if filtered.len() >= 4 {
                pick_with_rare_filter(&filtered, rng)
            } else {
                pick_with_rare_filter(allow, rng)
            };
        }
    }
    match category {
        BuildingCategory::House | BuildingCategory::Residential => {
            pick_with_rare_filter(&RESIDENTIAL_WALL_OPTIONS, rng)
        }
        BuildingCategory::Commercial | BuildingCategory::Office | BuildingCategory::Hotel => {
            pick_with_rare_filter(&COMMERCIAL_WALL_OPTIONS, rng)
        }
        BuildingCategory::Industrial | BuildingCategory::Warehouse => {
            pick_with_rare_filter(&INDUSTRIAL_WALL_OPTIONS, rng)
        }
        BuildingCategory::Religious => pick_with_rare_filter(&RELIGIOUS_WALL_OPTIONS, rng),
        BuildingCategory::School | BuildingCategory::Hospital => {
            pick_with_rare_filter(&INSTITUTIONAL_WALL_OPTIONS, rng)
        }
        BuildingCategory::Farm => pick_with_rare_filter(&FARM_WALL_OPTIONS, rng),
        BuildingCategory::Historic => pick_with_rare_filter(&HISTORIC_WALL_OPTIONS, rng),
        BuildingCategory::Garage => pick_with_rare_filter(&GARAGE_WALL_OPTIONS, rng),
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
        BuildingCategory::ContemporarySkyscraper | BuildingCategory::GridSkyscraper => {
            // Light modern concrete/stone frame.
            const CONTEMPORARY: [Block; 5] = [
                LIGHT_GRAY_CONCRETE,
                WHITE_CONCRETE,
                GRAY_CONCRETE,
                QUARTZ_BLOCK,
                SMOOTH_STONE,
            ];
            CONTEMPORARY[rng.random_range(0..CONTEMPORARY.len())]
        }
        BuildingCategory::MasonrySkyscraper => {
            // One warm (buff sandstone) or grey (limestone/granite) palette per building.
            if rng.random_bool(0.5) {
                const WARM: [Block; 3] = [SMOOTH_SANDSTONE, SANDSTONE, END_STONE_BRICKS];
                WARM[rng.random_range(0..WARM.len())]
            } else {
                const GREY: [Block; 4] = [
                    STONE_BRICKS,
                    SMOOTH_STONE,
                    POLISHED_ANDESITE,
                    POLISHED_DIORITE,
                ];
                GREY[rng.random_range(0..GREY.len())]
            }
        }
        BuildingCategory::GlassySkyscraper | BuildingCategory::GlassCornerSkyscraper => {
            // Glass-facade skyscrapers use stained glass as wall material
            const GLASSY_WALL_OPTIONS: [Block; 4] = [
                GRAY_STAINED_GLASS,
                CYAN_STAINED_GLASS,
                BLUE_STAINED_GLASS,
                LIGHT_BLUE_STAINED_GLASS,
            ];
            GLASSY_WALL_OPTIONS[rng.random_range(0..GLASSY_WALL_OPTIONS.len())]
        }
        BuildingCategory::Default => get_fallback_building_block(rng),
    }
}

/// Gross block height of one upper floor: 3 for residential-scale types
/// (~3 m storeys), 4 for commercial/institutional. Tags only, because the
/// category is resolved after the height and cannot be used here.
fn floor_cycle_for(building_type: &str, tags: &HashMap<String, String>) -> i32 {
    const RESIDENTIAL_SCALE: &[&str] = &[
        "house",
        "detached",
        "semidetached_house",
        "terrace",
        "bungalow",
        "villa",
        "cabin",
        "residential",
        "apartments",
        "dormitory",
        "farm",
        "hut",
        "shed",
        "static_caravan",
    ];
    if tags.contains_key("building:part") {
        // One cycle for all parts keeps stacked volumes flush at the seams.
        return 4;
    }
    if RESIDENTIAL_SCALE.contains(&building_type) || building_type == "yes" {
        3
    } else {
        4
    }
}

/// Inferred storey count or explicit hall height for buildings without any
/// height data in OSM. Halls are single tall volumes (industrial, churches,
/// garages) whose height is not a storey multiple.
enum InferredHeight {
    Levels(f64),
    HallBlocks(i32),
}

/// Deterministic weighted pick from (value, weight) pairs.
fn pick_weighted_value(rng: &mut impl Rng, options: &[(i32, u32)]) -> i32 {
    let total: u32 = options.iter().map(|&(_, w)| w).sum();
    let mut roll = rng.random_range(0..total);
    for &(value, weight) in options {
        if roll < weight {
            return value;
        }
        roll -= weight;
    }
    options[options.len() - 1].0
}

/// Storey inference for buildings without any OSM height data, following
/// real-world typology per type and footprint. Seeded on the group seed so
/// parts of one building agree (#1197, #935, #1220).
fn infer_building_height(
    building_type: &str,
    tags: &HashMap<String, String>,
    footprint_area: usize,
    scale_factor: f64,
    group_seed: u64,
) -> InferredHeight {
    let mut rng = element_rng(group_seed ^ 0x48E1_6F00_1EA5_0001);

    // Thresholds are m2. Parts are pinned to the middle band so siblings of
    // one group draw from the same table regardless of their own footprint.
    let area_m2 = if tags.contains_key("building:part") {
        400
    } else if scale_factor > 0.0 {
        (footprint_area as f64 / (scale_factor * scale_factor)) as usize
    } else {
        footprint_area
    };

    if tags.get("man_made").map(String::as_str) == Some("tower") {
        let blocks = pick_weighted_value(&mut rng, &[(12, 40), (16, 35), (20, 25)]);
        return InferredHeight::HallBlocks(blocks);
    }

    let (is_hall, table): (bool, &[(i32, u32)]) = match building_type {
        "bungalow" | "static_caravan" => (false, &[(1, 100)]),
        "house" | "detached" | "villa" | "farm" => (false, &[(1, 25), (2, 55), (3, 20)]),
        "semidetached_house" => (false, &[(2, 70), (3, 30)]),
        "terrace" => (false, &[(2, 50), (3, 40), (4, 10)]),
        "apartments" | "residential" | "dormitory" => {
            (false, &[(3, 30), (4, 30), (5, 25), (6, 15)])
        }
        "hotel" => (false, &[(3, 25), (4, 30), (5, 25), (6, 10), (8, 10)]),
        "office" | "commercial" => match area_m2 {
            0..=299 => (false, &[(2, 40), (3, 60)]),
            300..=999 => (false, &[(3, 40), (4, 35), (5, 25)]),
            _ => (false, &[(4, 40), (5, 30), (6, 20), (8, 10)]),
        },
        "retail" | "shop" | "supermarket" => (false, &[(1, 60), (2, 40)]),
        "kiosk" => (false, &[(1, 100)]),
        "parking" => (false, &[(2, 30), (3, 40), (4, 20), (5, 10)]),
        "school" | "kindergarten" | "college" | "university" => {
            (false, &[(2, 50), (3, 40), (4, 10)])
        }
        "hospital" => (false, &[(4, 40), (5, 30), (6, 20), (7, 10)]),
        "cathedral" => (true, &[(18, 50), (21, 30), (24, 20)]),
        "church" | "chapel" | "mosque" | "synagogue" | "temple" | "religious" => {
            (true, &[(10, 50), (12, 30), (14, 20)])
        }
        "industrial" | "warehouse" | "hangar" | "barn" | "stable" => {
            (true, &[(5, 20), (7, 50), (9, 30)])
        }
        "garage" | "garages" | "carport" | "shed" | "hut" => (true, &[(3, 100)]),
        "cabin" => (false, &[(1, 80), (2, 20)]),
        // building=yes and unrecognised types: infer from footprint size.
        _ => match area_m2 {
            // Tiny: a utility box or a small one-storey house.
            0..=39 => {
                if rng.random_bool(0.5) {
                    (true, &[(3, 100)])
                } else {
                    (false, &[(1, 100)])
                }
            }
            40..=149 => (false, &[(1, 15), (2, 60), (3, 25)]),
            150..=599 => (false, &[(2, 40), (3, 40), (4, 20)]),
            // Large generic footprint: often a hall, else a mid-rise block.
            _ => {
                let roll = rng.random_range(0u32..100);
                if roll < 40 {
                    (true, &[(7, 100)])
                } else if roll < 75 {
                    (false, &[(3, 100)])
                } else {
                    (false, &[(4, 100)])
                }
            }
        },
    };

    let value = pick_weighted_value(&mut rng, table);
    if is_hall {
        InferredHeight::HallBlocks(value)
    } else {
        InferredHeight::Levels(value as f64)
    }
}

/// Extra rows on top of `levels * cycle`: one taller ground floor row plus the
/// floor slab row (the "+2" in the wall grammar).
const GROUND_FLOOR_BONUS: i32 = 2;

/// Determines building height from OSM tags, falling back to per-type
/// inference when the element carries no height data at all.
#[allow(clippy::too_many_arguments)]
fn calculate_building_height(
    element: &ProcessedWay,
    building_type: &str,
    min_level: i32,
    scale_factor: f64,
    relation_levels: Option<i32>,
    floor_cycle: i32,
    footprint_area: usize,
    group_seed: u64,
) -> (i32, bool) {
    let mut building_height = ((10.0 * scale_factor) as i32).max(3);
    let mut is_tall_building = false;
    // Whether any explicit height source (tag or relation) applied.
    let mut has_source = false;

    // From building:levels tag (may be fractional, e.g. "2.5")
    if let Some(levels_str) = element.tags.get("building:levels") {
        if let Ok(levels) = levels_str.trim().parse::<f64>() {
            let lev = levels - min_level as f64;
            if lev >= 1.0 {
                // Elevated elements get the +2 in their min_level offset instead,
                // keeping the total top at levels * cycle + 2
                let bonus = if min_level > 0 {
                    0.0
                } else {
                    GROUND_FLOOR_BONUS as f64
                };
                building_height =
                    (((lev * floor_cycle as f64 + bonus) * scale_factor) as i32).max(3);
                has_source = true;
                if levels > 7.0 {
                    is_tall_building = true;
                }
            }
        }
    }

    // From height tag (overrides levels).
    // When min_height is also present, the wall height is height − min_height
    // (OSM `height` is absolute from ground, not relative to min_height).
    let mut has_explicit_height = false;
    if let Some(height_str) = element.tags.get("height") {
        if let Ok(height) = height_str.trim_end_matches("m").trim().parse::<f64>() {
            has_explicit_height = true;
            has_source = true;
            let mut is_elevated_part = false;
            let effective = if let Some(mh_str) = element.tags.get("min_height") {
                let mh = mh_str
                    .trim_end_matches('m')
                    .trim()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                is_elevated_part = mh > 0.0;
                (height - mh).max(1.0)
            } else if min_level > 0 {
                // `height` is absolute from ground; without a min_height tag
                // the level-based offset must still come off the wall span,
                // matching the min_level_offset the part is lifted by.
                is_elevated_part = true;
                let offset = (min_level * floor_cycle + GROUND_FLOOR_BONUS) as f64;
                (height - offset).max(1.0)
            } else {
                height
            };
            let effective = match (
                element.tags.get("roof:height"),
                element.tags.get("roof:shape"),
            ) {
                (Some(rh), Some(shape)) if shape != "flat" => {
                    // height includes the roof, a tagged roof:height comes off the walls
                    let rh = rh
                        .trim_end_matches('m')
                        .trim()
                        .parse::<f64>()
                        .unwrap_or(0.0);
                    (effective - rh.max(0.0)).max(3.0f64.min(effective))
                }
                _ => effective,
            };
            building_height = (effective * scale_factor) as i32;
            // Elevated parts can be thin slabs, skip the 3-block interior minimum
            building_height = building_height.max(if is_elevated_part { 1 } else { 3 });
            if height > 28.0 {
                is_tall_building = true;
            }
        }
    }

    // Relation levels only estimate the height, an explicit height tag wins
    if !has_explicit_height {
        if let Some(levels) = relation_levels {
            let bonus = if min_level > 0 { 0 } else { GROUND_FLOOR_BONUS };
            building_height = multiply_scale(
                (levels - min_level).max(1) * floor_cycle + bonus,
                scale_factor,
            )
            .max(3);
            has_source = true;
            if levels > 7 {
                is_tall_building = true;
            }
        }
    }

    // No height data anywhere: infer a plausible height from the building type
    // and footprint instead of a flat citywide default.
    if !has_source {
        match infer_building_height(
            building_type,
            &element.tags,
            footprint_area,
            scale_factor,
            group_seed,
        ) {
            InferredHeight::Levels(levels) => {
                // subtract min_level like the tagged branches, parts top out flush
                let lev = (levels - min_level as f64).max(1.0);
                let bonus = if min_level > 0 {
                    0.0
                } else {
                    GROUND_FLOOR_BONUS as f64
                };
                building_height =
                    (((lev * floor_cycle as f64 + bonus) * scale_factor) as i32).max(3);
                if levels > 7.0 {
                    is_tall_building = true;
                }
            }
            InferredHeight::HallBlocks(blocks) => {
                building_height = multiply_scale(blocks, scale_factor).max(3);
            }
        }
    }

    (building_height, is_tall_building)
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
    group_seed: u64,
) {
    let scale_factor = args.scale;
    let abs_terrain_offset = if !args.terrain() {
        args.ground_level
    } else {
        0
    };
    let floor_cycle = floor_cycle_for(
        element
            .tags
            .get("building")
            .or_else(|| element.tags.get("building:part"))
            .map(|s| s.as_str())
            .unwrap_or("roof"),
        &element.tags,
    );

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
            .map(|l| {
                if l > 0 {
                    multiply_scale(l * floor_cycle + GROUND_FLOOR_BONUS, scale_factor)
                } else {
                    0
                }
            })
            .unwrap_or(0)
    } else if let Some(layer) = element.tags.get("layer") {
        // For building:part=roof elements without explicit height tags, interpret
        // the layer tag as a coarse vertical-placement hint.  Each layer maps to
        // one floor cycle, producing reasonable stacking for multi-shell roof structures.
        layer
            .parse::<i32>()
            .ok()
            .filter(|&l| l > 0)
            .map(|l| multiply_scale(l * floor_cycle, scale_factor))
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
            // Elevated roof parts can be thin plates, keep them thin
            (total - min_level_offset).max(if min_level_offset > 0 { 1 } else { 3 })
        } else {
            total.max(3)
        }
    } else if let Some(levels) = element.tags.get("building:levels") {
        levels
            .parse::<i32>()
            .ok()
            .map(|l| multiply_scale(l * floor_cycle + GROUND_FLOOR_BONUS, scale_factor).max(3))
            .unwrap_or(5)
    } else {
        5 // Default thickness for thin roof / canopy structures
    };

    // Pick a block for the roof surface.
    // Priority: roof:material > roof:colour > building/colour > default.
    let mut rng = element_rng(group_seed);
    let roof_block = element
        .tags
        .get("roof:material")
        .or_else(|| element.tags.get("material"))
        .and_then(|m| get_roof_block_for_material(m, &mut rng))
        .or_else(|| {
            element
                .tags
                .get("roof:colour")
                .or_else(|| element.tags.get("building:colour"))
                .or_else(|| element.tags.get("colour"))
                .and_then(|c| color_text_to_rgb_tuple(c))
                .map(|rgb| crate::block_palette::roof_block_for_color(rgb, &mut rng))
        })
        .unwrap_or(STONE_BRICK_SLAB);

    // Determine the roof shape from tags.
    let roof_type = element
        .tags
        .get("roof:shape")
        .map(|s| parse_roof_type(s))
        .unwrap_or(RoofType::Flat);

    match roof_type {
        RoofType::Dome
        | RoofType::Hipped
        | RoofType::Pyramidal
        | RoofType::Cone
        | RoofType::Onion => {
            if !cached_floor_area.is_empty() {
                let (min_x, max_x, min_z, max_z) = cached_floor_area.iter().fold(
                    (i32::MAX, i32::MIN, i32::MAX, i32::MIN),
                    |(min_x, max_x, min_z, max_z), &(x, z)| {
                        (min_x.min(x), max_x.max(x), min_z.min(z), max_z.max(z))
                    },
                );
                // shaped canopies spring from the support height like the flat arm
                let springing = start_y_offset + roof_thickness;
                let config = RoofConfig {
                    min_x,
                    max_x,
                    min_z,
                    max_z,
                    center_x: (min_x + max_x) >> 1,
                    center_z: (min_z + max_z) >> 1,
                    base_height: springing,
                    building_height: roof_thickness.max(3),
                    abs_terrain_offset,
                    roof_block,
                    add_dormers: false,
                    element_id_for_decor: element.id,
                    peak_cap: None,
                };
                for node in &element.nodes {
                    let pillar_base = if args.terrain() {
                        editor.get_ground_level(node.x, node.z)
                    } else {
                        0
                    };
                    for y in (pillar_base + 1)..springing {
                        editor.set_block_absolute(
                            COBBLESTONE_WALL,
                            node.x,
                            y + abs_terrain_offset,
                            node.z,
                            None,
                            None,
                        );
                    }
                }
                match roof_type {
                    RoofType::Cone => generate_cone_roof(editor, cached_floor_area, &config),
                    RoofType::Onion => generate_onion_roof(editor, cached_floor_area, &config),
                    RoofType::Hipped => generate_hipped_roof(editor, cached_floor_area, &config),
                    RoofType::Pyramidal => {
                        generate_pyramidal_roof(editor, cached_floor_area, &config)
                    }
                    _ => generate_dome_roof(editor, cached_floor_area, &config),
                }
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
                let pillar_base = if args.terrain() {
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
/// True when the outline carries a mapped entrance/door node.
pub(crate) fn outline_has_mapped_entrance(element: &ProcessedWay) -> bool {
    element
        .nodes
        .iter()
        .any(|n| n.tags.contains_key("entrance") || n.tags.contains_key("door"))
}

/// Wood species for an entrance door, matched to category and wall material.
#[derive(Copy, Clone, PartialEq, Eq)]
enum DoorStyle {
    Oak,
    Spruce,
    DarkOak,
    Birch,
}

impl DoorStyle {
    fn base_block(self) -> Block {
        match self {
            DoorStyle::Oak => OAK_DOOR,
            DoorStyle::Spruce => SPRUCE_DOOR_LOWER,
            DoorStyle::DarkOak => DARK_OAK_DOOR_LOWER,
            DoorStyle::Birch => BIRCH_DOOR,
        }
    }

    /// Matching trapdoor species for the entrance canopy.
    fn trapdoor_block(self) -> Block {
        match self {
            DoorStyle::Oak => OAK_TRAPDOOR,
            DoorStyle::Spruce => SPRUCE_TRAPDOOR,
            DoorStyle::DarkOak => DARK_OAK_TRAPDOOR,
            DoorStyle::Birch => BIRCH_TRAPDOOR,
        }
    }
}

/// A planned entrance: one or two door leaves plus their dressing.
struct EntrancePlan {
    x: i32,
    z: i32,
    normal: (i32, i32),
    tangent: (i32, i32),
    double: bool,
    style: DoorStyle,
    canopy: bool,
    lantern: bool,
}

fn door_style_for(category: BuildingCategory, wall_block: Block, group_seed: u64) -> DoorStyle {
    match category {
        BuildingCategory::Industrial
        | BuildingCategory::Warehouse
        | BuildingCategory::Commercial
        | BuildingCategory::Office
        | BuildingCategory::Hotel
        | BuildingCategory::Historic
        | BuildingCategory::Religious => DoorStyle::DarkOak,
        _ => match wall_block {
            OAK_PLANKS | OAK_LOG => DoorStyle::Spruce,
            SPRUCE_PLANKS | SPRUCE_LOG | DARK_OAK_PLANKS => DoorStyle::Oak,
            QUARTZ_BLOCK | QUARTZ_BRICKS | WHITE_CONCRETE | WHITE_TERRACOTTA => DoorStyle::Birch,
            _ => {
                if element_rng(group_seed ^ 0xD00E_57E9_0000_0012).random_bool(0.50) {
                    DoorStyle::Oak
                } else {
                    DoorStyle::DarkOak
                }
            }
        },
    }
}

/// Plans a synthetic street-facing entrance for buildings whose outline has no
/// mapped entrance node. Every ordinary ground-level building gets one door.
fn plan_synthetic_entrance(
    element: &ProcessedWay,
    config: &BuildingConfig,
    facade: &FacadePlan,
    building_passages: &CoordinateBitmap,
    group_seed: u64,
) -> Option<EntrancePlan> {
    if !config.is_ground_level
        || config.has_garage_door
        || config.has_single_door
        || matches!(
            config.condition,
            BuildingCondition::Construction | BuildingCondition::Ruined
        )
        || matches!(
            config.category,
            BuildingCategory::Greenhouse | BuildingCategory::Shed | BuildingCategory::Garage
        )
        || facade.segments.is_empty()
    {
        return None;
    }

    // Corner buildings of public categories put the door right at the corner.
    let corner_pick = facade.corner.as_ref().filter(|_| {
        matches!(
            config.category,
            BuildingCategory::Commercial | BuildingCategory::Hotel | BuildingCategory::Office
        )
    });

    let seg_index = if let Some(corner) = corner_pick {
        // The longer of the two corner legs hosts the door.
        let len = |i: usize| facade.segments[i].as_ref().map(|s| s.len).unwrap_or(0);
        if len(corner.seg_a) >= len(corner.seg_b) {
            corner.seg_a
        } else {
            corner.seg_b
        }
    } else if let Some(front) = facade.front_segment {
        front
    } else {
        // Rural fallback: the longest non-party segment still gets a door.
        facade
            .segments
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|s| (i, s)))
            .filter(|(_, s)| s.class != FacadeClass::Party)
            .max_by_key(|(_, s)| s.len)
            .map(|(i, _)| i)?
    };

    let seg = facade.segments.get(seg_index)?.as_ref()?;
    if seg.len < 4 {
        return None;
    }
    let (x1, z1) = {
        let n = element.nodes.get(seg_index)?;
        (n.x, n.z)
    };
    let (x2, z2) = {
        let n = element.nodes.get(seg_index + 1)?;
        (n.x, n.z)
    };
    let points: Vec<(i32, i32)> = bresenham_line(x1, 0, z1, x2, 0, z2)
        .into_iter()
        .map(|(x, _, z)| (x, z))
        .collect();
    if points.len() < 5 {
        return None;
    }

    let mut pos_rng = element_rng(group_seed ^ 0xD00E_57E9_0000_0011);
    let target = if let Some(corner) = corner_pick {
        // 2-3 columns in from the corner vertex end of the segment.
        let from_corner = 2 + pos_rng.random_range(0..2u32) as usize;
        if corner.seg_a == seg_index {
            points.len().saturating_sub(1 + from_corner)
        } else {
            from_corner
        }
    } else {
        let jitter: f64 = pos_rng.random_range(-0.15..0.15);
        ((points.len() as f64) * (0.5 + jitter)) as usize
    };
    // Dodge passages and party columns, keep 2 columns clear of the ends.
    let clamp = |i: isize| (i.clamp(2, points.len() as isize - 3)) as usize;
    let mut chosen: Option<(i32, i32)> = None;
    for attempt in [0isize, 1, -1, 2, -2] {
        let idx = clamp(target as isize + attempt);
        let (bx, bz) = points[idx];
        if building_passages.contains(bx, bz) || facade.is_party(bx, bz) {
            continue;
        }
        chosen = Some((bx, bz));
        break;
    }
    let (x, z) = chosen?;

    let double = matches!(
        config.category,
        BuildingCategory::Commercial
            | BuildingCategory::Office
            | BuildingCategory::Hotel
            | BuildingCategory::School
            | BuildingCategory::Hospital
    ) && seg.len >= 8
        && (seg.tangent.0 == 0 || seg.tangent.1 == 0);

    let style = door_style_for(config.category, config.wall_block, group_seed);
    let mut dress_rng = element_rng(group_seed ^ 0xD00E_57E9_0000_0013);
    let canopy = (double
        || matches!(
            config.category,
            BuildingCategory::Commercial | BuildingCategory::Hotel
        ))
        && dress_rng.random_bool(0.60);
    let lantern = !canopy
        && matches!(
            config.category,
            BuildingCategory::House | BuildingCategory::Residential | BuildingCategory::Historic
        )
        && dress_rng.random_bool(0.35);

    Some(EntrancePlan {
        x,
        z,
        normal: seg.normal,
        tangent: seg.tangent,
        double,
        style,
        canopy,
        lantern,
    })
}

/// Oriented, styled doors at mapped entrance/door nodes on the outline,
/// replacing the unoriented placeholder from the node-level doors pass.
fn plan_mapped_entrances(
    element: &ProcessedWay,
    config: &BuildingConfig,
    facade: &FacadePlan,
    group_seed: u64,
) -> Vec<EntrancePlan> {
    let mut plans = Vec::new();
    if !config.is_ground_level
        || matches!(
            config.condition,
            BuildingCondition::Construction | BuildingCondition::Ruined
        )
    {
        return plans;
    }
    let style = door_style_for(config.category, config.wall_block, group_seed);
    let n_nodes = element.nodes.len();
    let mut seen: FnvHashSet<(i32, i32)> = FnvHashSet::default();
    for (i, node) in element.nodes.iter().enumerate() {
        let entrance = node.tags.get("entrance");
        let door = node.tags.get("door");
        if entrance.is_none() && door.is_none() {
            continue;
        }
        // A closed way repeats its first node at the end; render one door.
        if !seen.insert((node.x, node.z)) {
            continue;
        }
        if entrance.map(String::as_str) == Some("no") || door.map(String::as_str) == Some("no") {
            continue;
        }
        // Only ground-level entrances; upper-level ones have no wall opening here.
        if let Some(level) = node.tags.get("level") {
            if level
                .trim()
                .parse::<f64>()
                .map(|l| l != 0.0)
                .unwrap_or(false)
            {
                continue;
            }
        }
        // Segment ending here (i-1) or starting here (i); pick the longer.
        let before = i
            .checked_sub(1)
            .and_then(|j| facade.segments.get(j))
            .and_then(|s| s.as_ref());
        let after = if i + 1 < n_nodes {
            facade.segments.get(i).and_then(|s| s.as_ref())
        } else {
            None
        };
        let seg = match (before, after) {
            (Some(a), Some(b)) => Some(if a.len >= b.len { a } else { b }),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let Some(seg) = seg else { continue };
        let double = entrance.map(String::as_str) == Some("main")
            && matches!(
                config.category,
                BuildingCategory::Commercial | BuildingCategory::Office | BuildingCategory::Hotel
            )
            && seg.len >= 8
            && (seg.tangent.0 == 0 || seg.tangent.1 == 0);
        plans.push(EntrancePlan {
            x: node.x,
            z: node.z,
            normal: seg.normal,
            tangent: seg.tangent,
            double,
            style,
            canopy: false,
            lantern: false,
        });
    }
    plans
}

/// Places a planned entrance: door leaves (overwriting the wall), threshold,
/// doorstep stairs down to terrain, and optional canopy/lantern dressing.
fn render_entrance(
    editor: &mut WorldEditor,
    plan: &EntrancePlan,
    config: &BuildingConfig,
    args: &Args,
) {
    let (nx, nz) = plan.normal;
    let facing = facing_for_normal(nx, nz);
    let base = plan.style.base_block();
    let door_positions: Vec<((i32, i32), &str)> = if plan.double {
        vec![
            ((plan.x, plan.z), "left"),
            ((plan.x + plan.tangent.0, plan.z + plan.tangent.1), "right"),
        ]
    } else {
        vec![((plan.x, plan.z), "left")]
    };

    for ((dx, dz), hinge) in &door_positions {
        let lower = cached_prop_block(
            base,
            &[("half", "lower"), ("facing", facing), ("hinge", hinge)],
        );
        let upper = cached_prop_block(
            base,
            &[("half", "upper"), ("facing", facing), ("hinge", hinge)],
        );
        editor.set_block_with_properties_absolute(
            lower,
            *dx,
            config.start_y_offset + 1 + config.abs_terrain_offset,
            *dz,
            None,
            Some(&[]),
        );
        editor.set_block_with_properties_absolute(
            upper,
            *dx,
            config.start_y_offset + 2 + config.abs_terrain_offset,
            *dz,
            None,
            Some(&[]),
        );

        // Threshold block just outside the door (first writer wins the cell,
        // and buildings run before roads).
        let threshold = config.base_course_block.unwrap_or(STONE_BRICKS);
        editor.set_block_absolute(
            threshold,
            dx + nx,
            config.start_y_offset + config.abs_terrain_offset,
            dz + nz,
            None,
            None,
        );

        // Doorstep stairs bridging a terrain drop in front of the door.
        if args.terrain() && config.is_ground_level {
            let out_ground = editor
                .terrain_level(dx + nx, dz + nz)
                .unwrap_or(config.start_y_offset);
            let drop = config.start_y_offset - out_ground;
            if drop >= 1 {
                let stair_base = get_stair_block_for_material(threshold);
                // Ascend toward the door: high side of each step faces inward.
                let stair_facing = match facing_for_normal(-nx, -nz) {
                    "north" => StairFacing::North,
                    "south" => StairFacing::South,
                    "east" => StairFacing::East,
                    _ => StairFacing::West,
                };
                for step in 1..=drop.min(3) {
                    let stair = create_stair_with_properties(
                        stair_base,
                        stair_facing,
                        StairShape::Straight,
                    );
                    editor.set_block_with_properties_absolute(
                        stair,
                        dx + nx * step,
                        config.start_y_offset - step + 1 + config.abs_terrain_offset,
                        dz + nz * step,
                        Some(&[AIR]),
                        None,
                    );
                }
            }
        }
    }

    // Canopy: closed top-half trapdoors one block out, spanning the doorway.
    if plan.canopy {
        let span: &[i32] = if plan.double {
            &[-1, 0, 1, 2]
        } else {
            &[-1, 0, 1]
        };
        for t in span {
            let cx = plan.x + nx + plan.tangent.0 * t;
            let cz = plan.z + nz + plan.tangent.1 * t;
            editor.set_block_with_properties_absolute(
                make_closed_trapdoor(plan.style.trapdoor_block(), facing, "top"),
                cx,
                config.start_y_offset + 3 + config.abs_terrain_offset,
                cz,
                Some(&[AIR]),
                None,
            );
        }
    }
    if plan.lantern {
        editor.set_block_absolute(
            LANTERN,
            plan.x + nx,
            config.start_y_offset + 3 + config.abs_terrain_offset,
            plan.z + nz,
            Some(&[AIR]),
            None,
        );
    }
}

fn build_wall_ring(
    editor: &mut WorldEditor,
    nodes: &[ProcessedNode],
    config: &BuildingConfig,
    args: &Args,
    has_sloped_roof: bool,
    building_passages: &CoordinateBitmap,
    facade: &FacadePlan,
) -> (Vec<(i32, i32)>, i32) {
    let mut previous_node: Option<(i32, i32)> = None;
    // Count of generated wall coordinates; the caller only needs to know the ring is non-empty.
    let mut corner_count: i32 = 0;
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

                // Foundation pillars below terrain. Skipped in passage zones.
                if args.terrain() && config.is_ground_level && !is_passage {
                    let local_ground_level =
                        editor.terrain_level(bx, bz).unwrap_or(args.ground_level);

                    for y in local_ground_level..config.start_y_offset + 1 {
                        let block = apply_block_variety(config.wall_block, bx, y, bz, config);
                        editor.set_block_absolute(
                            block,
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

                // Construction: per-column variable wall height for a half-built look.
                let column_top = if config.condition == BuildingCondition::Construction {
                    let mut col_rng = coord_rng(bx, bz, config.element_id);
                    let factor: f64 = 0.30 + col_rng.random::<f64>() * 0.55;
                    let local = config.start_y_offset
                        + ((config.building_height as f64) * factor).round() as i32;
                    local
                        .max(config.start_y_offset + 1)
                        .min(config.start_y_offset + config.building_height - 1)
                } else {
                    config.start_y_offset + config.building_height
                };

                let col = ColumnFacade {
                    party: facade.is_party(bx, bz),
                    street: !facade.has_any_street || facade.is_street(bx, bz),
                };
                for h in wall_start..=column_top {
                    let block = determine_wall_block_at_position(bx, h, bz, config, col);
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
                corner_count += 1;
            }
        }

        previous_node = Some((x, z));
    }

    (current_building, corner_count)
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
fn determine_wall_block_at_position(
    bx: i32,
    h: i32,
    bz: i32,
    config: &BuildingConfig,
    col: ColumnFacade,
) -> Block {
    let chosen = determine_wall_block_at_position_pristine(bx, h, bz, config, col);
    let chosen = apply_block_variety(chosen, bx, h, bz, config);
    apply_condition_variation(chosen, bx, h, bz, config)
}

/// Substitutes the wall block with a same-family alternative for variety.
fn apply_block_variety(chosen: Block, bx: i32, h: i32, bz: i32, config: &BuildingConfig) -> Block {
    if chosen != config.wall_block {
        return chosen;
    }
    let pool = substitute_pool_only(chosen);
    if pool.is_empty() {
        return chosen;
    }

    // Two-tone facade: 20% chance on >=3-floor buildings; background fabric
    // (minimal detail) keeps a single flat field.
    let floors_estimate = config.building_height / config.floor_cycle;
    if floors_estimate >= 3 && config.detail != DetailTier::Minimal {
        let mut tt_enable_rng = element_rng(config.element_id ^ 0x9F4A_5DB2_C0DE_C0DE);
        if tt_enable_rng.random_bool(0.20) {
            let ground_floor_top = config.ground_floor_top();
            if h > ground_floor_top {
                let mut tt_mode_rng = element_rng(config.element_id ^ 0xC1A2_5544_99B7_3F02);
                let secondary_mix = tt_mode_rng.random_bool(0.50);
                if secondary_mix {
                    let mut pos_rng = coord_rng(
                        bx,
                        bz,
                        config.element_id ^ 0x7E11_AABB_5DEF_3211 ^ ((h as u64) << 12),
                    );
                    return pool[pos_rng.random_range(0..pool.len())];
                } else {
                    let mut sec_rng = element_rng(config.element_id ^ 0x4A8C_99B0_CAFE_F00D);
                    return pool[sec_rng.random_range(0..pool.len())];
                }
            }
        }
    }

    // 10% stays single; the rest keeps a clean primary field with ~20% same-family texture.
    let mut variety_mode_rng = element_rng(config.element_id ^ 0xBABE_F1A1_2222_8888);
    if !variety_mode_rng.random_bool(0.90) {
        return chosen;
    }

    let mut pos_rng = coord_rng(
        bx,
        bz,
        config.element_id ^ 0x5050_3030_AAFF_BBCC ^ ((h as u64) << 12),
    );
    if pos_rng.random_range(0u32..100) < 20 {
        pool[pos_rng.random_range(0..pool.len())]
    } else {
        chosen
    }
}

/// Swaps wall blocks that genuinely don't work as a roof.
fn roof_friendly_block(block: Block) -> Block {
    match block {
        OAK_LOG => OAK_PLANKS,
        SPRUCE_LOG => SPRUCE_PLANKS,
        RED_CONCRETE => RED_TERRACOTTA,
        ORANGE_CONCRETE => ORANGE_TERRACOTTA,
        YELLOW_CONCRETE => YELLOW_TERRACOTTA,
        LIME_CONCRETE => GREEN_CONCRETE,
        BLUE_CONCRETE => BLUE_TERRACOTTA,
        _ => block,
    }
}

/// Categories whose roof aesthetic (pitched / domed / glass) doesn't fit a parapet.
#[inline]
fn short_flat_parapet_excluded(category: BuildingCategory) -> bool {
    matches!(
        category,
        BuildingCategory::House
            | BuildingCategory::Farm
            | BuildingCategory::Garage
            | BuildingCategory::Shed
            | BuildingCategory::Greenhouse
            | BuildingCategory::Religious
    )
}

/// 75% chance of a parapet on short flat-roof buildings.
fn short_flat_parapet_for(
    element_id: u64,
    roof_type: RoofType,
    building_height: i32,
    category: BuildingCategory,
    condition: BuildingCondition,
) -> bool {
    if roof_type != RoofType::Flat {
        return false;
    }
    if building_height > 8 {
        return false;
    }
    if short_flat_parapet_excluded(category) {
        return false;
    }
    if condition != BuildingCondition::Normal {
        return false;
    }
    let mut rng = element_rng(element_id ^ 0x534F_F154_4AAE_E0F0);
    rng.random_bool(0.75)
}

/// 10% chance of rooftop equipment on short flat-roof buildings.
fn short_flat_rooftop_bits_for(
    element_id: u64,
    roof_type: RoofType,
    building_height: i32,
    category: BuildingCategory,
    condition: BuildingCondition,
    detail: DetailTier,
) -> bool {
    if detail == DetailTier::Minimal {
        return false;
    }
    if roof_type != RoofType::Flat {
        return false;
    }
    if building_height > 8 {
        return false;
    }
    if short_flat_parapet_excluded(category) {
        return false;
    }
    if condition != BuildingCondition::Normal {
        return false;
    }
    let mut rng = element_rng(element_id ^ 0xBCB1_5EED_5704_77F0);
    rng.random_bool(0.10)
}

/// Same-family substitutes for the wall variety system; empty slice = no variety.
fn substitute_pool_only(block: Block) -> &'static [Block] {
    match block {
        // Mid-grey stone
        STONE_BRICKS => &[
            COBBLESTONE,
            CRACKED_STONE_BRICKS,
            ANDESITE,
            CHISELED_STONE_BRICKS,
        ],
        COBBLESTONE => &[STONE_BRICKS, ANDESITE, STONE],
        STONE => &[COBBLESTONE, STONE_BRICKS],
        ANDESITE => &[POLISHED_ANDESITE, COBBLESTONE, STONE_BRICKS],
        POLISHED_ANDESITE => &[ANDESITE, STONE_BRICKS],
        CHISELED_STONE_BRICKS => &[STONE_BRICKS, CRACKED_STONE_BRICKS],
        CRACKED_STONE_BRICKS => &[STONE_BRICKS, MOSSY_STONE_BRICKS, CHISELED_STONE_BRICKS],
        TUFF => &[STONE_BRICKS, MOSSY_STONE_BRICKS],
        MOSSY_STONE_BRICKS => &[MOSSY_COBBLESTONE, TUFF],
        MOSSY_COBBLESTONE => &[MOSSY_STONE_BRICKS, TUFF],

        // Warm reds
        BRICK => &[POLISHED_GRANITE, GRANITE, TERRACOTTA],
        POLISHED_GRANITE => &[BRICK, GRANITE],
        GRANITE => &[POLISHED_GRANITE, BRICK],
        TERRACOTTA => &[BRICK, POLISHED_GRANITE],

        // Deepslate
        DEEPSLATE_BRICKS => &[POLISHED_DEEPSLATE, COBBLED_DEEPSLATE],
        POLISHED_DEEPSLATE => &[DEEPSLATE_BRICKS, COBBLED_DEEPSLATE],
        COBBLED_DEEPSLATE => &[DEEPSLATE_BRICKS, POLISHED_DEEPSLATE, DEEPSLATE],
        DEEPSLATE => &[COBBLED_DEEPSLATE, DEEPSLATE_BRICKS],

        // Blackstone
        POLISHED_BLACKSTONE_BRICKS => &[POLISHED_BLACKSTONE, BLACKSTONE],
        POLISHED_BLACKSTONE => &[POLISHED_BLACKSTONE_BRICKS, BLACKSTONE],
        BLACKSTONE => &[POLISHED_BLACKSTONE, POLISHED_BLACKSTONE_BRICKS],

        // Nether
        NETHER_BRICK => &[BLACK_TERRACOTTA],
        BLACK_TERRACOTTA => &[NETHER_BRICK],
        RED_NETHER_BRICKS => &[NETHER_BRICK],

        // Brown family
        BROWN_TERRACOTTA => &[BROWN_CONCRETE_POWDER, BROWN_CONCRETE],
        BROWN_CONCRETE => &[BROWN_CONCRETE_POWDER, BROWN_TERRACOTTA],

        // Off-white / quartz
        QUARTZ_BLOCK => &[QUARTZ_BRICKS, SMOOTH_QUARTZ],
        QUARTZ_BRICKS => &[QUARTZ_BLOCK, WHITE_CONCRETE, SMOOTH_QUARTZ],
        SMOOTH_QUARTZ => &[QUARTZ_BLOCK, QUARTZ_BRICKS],
        WHITE_CONCRETE => &[QUARTZ_BRICKS],

        // Sandstone
        SANDSTONE => &[SMOOTH_SANDSTONE],
        SMOOTH_SANDSTONE => &[SANDSTONE],

        SMOOTH_STONE => &[STONE_BRICKS, STONE],

        POLISHED_DIORITE => &[DIORITE],
        DIORITE => &[POLISHED_DIORITE],

        // Light-grey terracotta + muted copper
        LIGHT_GRAY_TERRACOTTA => &[
            WAXED_EXPOSED_COPPER,
            WAXED_EXPOSED_CHISELED_COPPER,
            WAXED_EXPOSED_CUT_COPPER,
        ],
        WAXED_EXPOSED_COPPER => &[
            LIGHT_GRAY_TERRACOTTA,
            WAXED_EXPOSED_CHISELED_COPPER,
            WAXED_EXPOSED_CUT_COPPER,
        ],
        WAXED_EXPOSED_CHISELED_COPPER => &[
            LIGHT_GRAY_TERRACOTTA,
            WAXED_EXPOSED_COPPER,
            WAXED_EXPOSED_CUT_COPPER,
        ],
        WAXED_EXPOSED_CUT_COPPER => &[
            LIGHT_GRAY_TERRACOTTA,
            WAXED_EXPOSED_COPPER,
            WAXED_EXPOSED_CHISELED_COPPER,
        ],

        // Orange terracotta + copper
        ORANGE_TERRACOTTA => &[WAXED_COPPER_BLOCK, TERRACOTTA],
        WAXED_COPPER_BLOCK => &[ORANGE_TERRACOTTA, TERRACOTTA],

        GRAY_CONCRETE => &[MUD],
        MUD => &[GRAY_CONCRETE],

        // Wood
        OAK_PLANKS => &[SPRUCE_PLANKS],
        SPRUCE_PLANKS => &[OAK_PLANKS, DARK_OAK_PLANKS],
        DARK_OAK_PLANKS => &[SPRUCE_PLANKS, SPRUCE_LOG],
        OAK_LOG => &[SPRUCE_LOG],

        _ => &[],
    }
}

/// Weathers walls and boards up windows for damaged condition states.
fn apply_condition_variation(
    chosen: Block,
    bx: i32,
    h: i32,
    bz: i32,
    config: &BuildingConfig,
) -> Block {
    if config.condition == BuildingCondition::Construction {
        return chosen;
    }

    // Subtle weathering on age-appropriate Normal-condition categories/eras.
    let normal_weather_rate: f64 = if config.condition == BuildingCondition::Normal {
        let category_rate: f64 = match config.category {
            BuildingCategory::Historic | BuildingCategory::Religious => 0.06,
            BuildingCategory::Farm => 0.03,
            _ => 0.0,
        };
        let era_rate = match config.era {
            ArchEra::HistoricOrnate => 0.05,
            ArchEra::TraditionalPreWar => 0.02,
            _ => 0.0,
        };
        category_rate.max(era_rate)
    } else {
        0.0
    };

    if config.condition == BuildingCondition::Normal && normal_weather_rate == 0.0 {
        return chosen;
    }

    let mut rng = coord_rng(bx, bz, config.element_id ^ ((h as u64) << 16));
    let is_window = chosen == config.window_block && config.has_windows;

    // Window-boarding rate for Disused/Abandoned (Ruined drops windows entirely).
    let board_rate: f64 = match config.condition {
        BuildingCondition::Disused => 0.30,
        BuildingCondition::Abandoned => 0.50,
        _ => 0.0,
    };
    if is_window && board_rate > 0.0 && rng.random_bool(board_rate) {
        return config.wall_block;
    }

    // Stone weathering rate per condition.
    let weather_rate: f64 = match config.condition {
        BuildingCondition::Abandoned => 0.05,
        BuildingCondition::Ruined => 0.50,
        BuildingCondition::Normal => normal_weather_rate,
        _ => 0.0,
    };
    if !is_window && weather_rate > 0.0 && rng.random_bool(weather_rate) {
        return weathered_variant(chosen, &mut rng);
    }

    chosen
}

/// Aged counterpart of `block` for damaged-condition states.
fn weathered_variant(block: Block, rng: &mut impl Rng) -> Block {
    match block {
        STONE_BRICKS => {
            const OPTIONS: [Block; 3] = [MOSSY_STONE_BRICKS, CRACKED_STONE_BRICKS, STONE_BRICKS];
            OPTIONS[rng.random_range(0..OPTIONS.len())]
        }
        COBBLESTONE => MOSSY_COBBLESTONE,
        STONE => {
            const OPTIONS: [Block; 2] = [ANDESITE, COBBLESTONE];
            OPTIONS[rng.random_range(0..OPTIONS.len())]
        }
        SMOOTH_STONE => STONE,
        POLISHED_ANDESITE => ANDESITE,
        OAK_PLANKS => {
            const OPTIONS: [Block; 2] = [SPRUCE_PLANKS, DARK_OAK_PLANKS];
            OPTIONS[rng.random_range(0..OPTIONS.len())]
        }
        OAK_LOG => SPRUCE_LOG,
        _ => block,
    }
}

fn determine_wall_block_at_position_pristine(
    bx: i32,
    h: i32,
    bz: i32,
    config: &BuildingConfig,
    col: ColumnFacade,
) -> Block {
    // Darker plinth rows ground the building visually.
    if let Some(base) = config.base_course_block {
        if h <= config.start_y_offset + config.base_course_rows() {
            return base;
        }
    }

    let floor_row = config.floor_row(h);

    // Party walls render like windowless walls: no glazing into the
    // attached neighbor of a terraced row.
    if !config.has_windows || col.party {
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
        // with stone separation bands at floor levels (every floor cycle)
        if above_floor && config.has_lobby_base && h <= config.ground_floor_top() {
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
        let is_slit = above_floor
            && (floor_row == 1 || floor_row == 2)
            && (bx + bz + config.window_phase).rem_euclid(4) == 1;

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
    } else if config.category == BuildingCategory::GridSkyscraper {
        // Big glass panes separated by concrete mullions at floor lines and every 5th column.
        let mullion =
            !above_floor || floor_row == 0 || (bx + bz + config.window_phase).rem_euclid(5) == 0;
        if mullion {
            config.wall_block
        } else {
            config.window_block
        }
    } else if config.is_tall_building && config.use_vertical_windows {
        // Tall building pattern, vertical window strips alternating with wall columns
        if above_floor && (bx + bz + config.window_phase).rem_euclid(2) == 0 {
            config.window_block
        } else {
            config.wall_block
        }
    } else {
        // Regular building pattern
        let window_col = config.window_col(bx, bz);

        // Storefront glazing: wider full-glass bays across the whole ground floor.
        if config.has_storefront
            && col.street
            && above_floor
            && h <= config.ground_floor_top()
            && floor_row != 0
            && window_col < 4
        {
            return GLASS;
        }

        let role = config.floor_role(h);

        // Attic band: solid wall under the pitched roof with small single
        // lights instead of full windows.
        if config.attic_style && role == FloorRole::Top {
            return if above_floor && window_col == 1 && floor_row == 2 {
                config.window_block
            } else {
                config.wall_block
            };
        }

        // Band cornice separating the body from a treated top floor.
        if config.top_treatment
            && floor_row == 0
            && h == config.start_y_offset + config.building_height - config.floor_cycle
        {
            return config.accent_block;
        }

        // Window layout across the 6-block cycle: the two skyscraper families
        // keep their fixed widths (masonry narrow, contemporary wide); everything
        // else follows the per-building archetype.
        let mut is_window_position = above_floor
            && floor_row != 0
            && match config.category {
                BuildingCategory::MasonrySkyscraper => window_col < 2,
                BuildingCategory::ContemporarySkyscraper => window_col < 4,
                _ => archetype_allows_window(
                    config.window_archetype,
                    window_col,
                    floor_row,
                    config.floor_cycle,
                ),
            };

        // Treated top floors narrow their windows by one column.
        if config.top_treatment && role == FloorRole::Top {
            let narrowed = match config.window_archetype {
                WindowArchetype::Standard3 | WindowArchetype::ArchedTraditional => window_col != 2,
                WindowArchetype::WideHorizontal => window_col != 3,
                _ => true,
            };
            is_window_position = is_window_position && narrowed;
        }

        // Piano nobile: the first floor above ground gets grander glazing.
        if config.piano_nobile
            && role == FloorRole::Body
            && h <= config.ground_floor_top() + config.floor_cycle
            && above_floor
            && floor_row != 0
            && window_col < 3
        {
            is_window_position = true;
        }

        if is_window_position {
            config.window_block
        } else {
            let use_accent_line = config.use_accent_lines && above_floor && floor_row == 0;
            let use_vertical_accent_here =
                config.use_vertical_accent && above_floor && floor_row == 0 && window_col < 3;

            if use_accent_line || use_vertical_accent_here {
                config.accent_block
            } else if config.rustication
                && role == FloorRole::Ground
                && h > config.start_y_offset + config.base_course_rows()
                && (h - config.start_y_offset) % 2 == 0
            {
                // Banded stone courses over the plinth.
                config.base_course_block.unwrap_or(config.accent_block)
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

// Share one Arc per distinct facade property compound instead of allocating per placement.
type FacadePropsCache = std::sync::Mutex<fnv::FnvHashMap<(u16, String), std::sync::Arc<Value>>>;
static FACADE_PROPS: once_cell::sync::Lazy<FacadePropsCache> =
    once_cell::sync::Lazy::new(Default::default);

pub(crate) fn cached_prop_block(base: Block, props: &[(&str, &str)]) -> BlockWithProperties {
    let key: String = props.iter().flat_map(|(k, v)| [*k, "=", *v, ";"]).collect();
    let mut cache = FACADE_PROPS.lock().unwrap();
    let arc = cache
        .entry((base.id(), key))
        .or_insert_with(|| {
            let mut map: HashMap<String, Value> = HashMap::new();
            for (k, v) in props {
                map.insert((*k).to_string(), Value::String((*v).to_string()));
            }
            std::sync::Arc::new(Value::Compound(map))
        })
        .clone();
    BlockWithProperties::from_arc(base, Some(arc))
}

/// Creates a `BlockWithProperties` for an open trapdoor with the given
/// base block and facing direction string.
fn make_open_trapdoor(base: Block, facing: &str) -> BlockWithProperties {
    cached_prop_block(
        base,
        &[("facing", facing), ("open", "true"), ("half", "top")],
    )
}

/// Creates a `BlockWithProperties` for a top-half slab.
fn make_top_slab(base: Block) -> BlockWithProperties {
    cached_prop_block(base, &[("type", "top")])
}

/// Closed trapdoor pinned flat against the wall face, top or bottom half.
fn make_closed_trapdoor(base: Block, facing: &str, half: &str) -> BlockWithProperties {
    cached_prop_block(
        base,
        &[("facing", facing), ("open", "false"), ("half", half)],
    )
}

/// Block with arbitrary string properties, for repeated decorated placements.
fn make_prop_block(base: Block, props: &[(&str, &str)]) -> BlockWithProperties {
    cached_prop_block(base, props)
}

/// Computes the centroid (average position) of the building outline nodes.
/// Returns `None` if the node list is empty.
pub(crate) fn compute_building_centroid(nodes: &[ProcessedNode]) -> Option<(i32, i32)> {
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

/// Length-weighted dominant edge angle in radians, folded to (-45, 45] deg.
fn dominant_axis_angle(nodes: &[ProcessedNode]) -> f64 {
    let mut sum_c = 0.0f64;
    let mut sum_s = 0.0f64;
    for i in 0..nodes.len() {
        let j = (i + 1) % nodes.len();
        let dx = (nodes[j].x - nodes[i].x) as f64;
        let dz = (nodes[j].z - nodes[i].z) as f64;
        let len = (dx * dx + dz * dz).sqrt();
        if len < 3.0 {
            continue;
        }
        let ang = dz.atan2(dx);
        sum_c += len * (4.0 * ang).cos();
        sum_s += len * (4.0 * ang).sin();
    }
    if sum_c == 0.0 && sum_s == 0.0 {
        return 0.0;
    }
    sum_s.atan2(sum_c) / 4.0
}

/// Near-rectangular footprints rotated 1 to 12 deg get an axis-aligned tent.
fn gable_axis_snap(nodes: &[ProcessedNode]) -> bool {
    if nodes.len() < 3 {
        return false;
    }
    let ang = dominant_axis_angle(nodes);
    let dev = ang.to_degrees().abs();
    if !(1.0..=12.0).contains(&dev) {
        return false;
    }
    let mut area = 0i64;
    for i in 0..nodes.len() {
        let j = (i + 1) % nodes.len();
        area += (nodes[i].x as i64) * (nodes[j].z as i64);
        area -= (nodes[j].x as i64) * (nodes[i].z as i64);
    }
    let polygon_area = (area.abs() as f64) / 2.0;
    let (c, sn) = (ang.cos(), ang.sin());
    let (mut u_min, mut u_max) = (f64::MAX, f64::MIN);
    let (mut v_min, mut v_max) = (f64::MAX, f64::MIN);
    for n in nodes {
        let u = n.x as f64 * c + n.z as f64 * sn;
        let v = -(n.x as f64) * sn + n.z as f64 * c;
        u_min = u_min.min(u);
        u_max = u_max.max(u);
        v_min = v_min.min(v);
        v_max = v_max.max(v);
    }
    let rect_area = (u_max - u_min) * (v_max - v_min);
    rect_area > 0.0 && polygon_area / rect_area >= 0.78
}

/// Computes the axis-aligned outward normal for a wall segment defined by
/// `(x1,z1)→(x2,z2)`, given the building centroid `(cx,cz)`.
///
/// Returns one of `(±1, 0)` or `(0, ±1)`, or `(0, 0)` for degenerate
/// (zero-length) segments.
pub(crate) fn compute_outward_normal(
    x1: i32,
    z1: i32,
    x2: i32,
    z2: i32,
    cx: i32,
    cz: i32,
) -> (i32, i32) {
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
pub(crate) fn facing_for_normal(nx: i32, nz: i32) -> &'static str {
    match (nx, nz) {
        (1, _) => "east",
        (-1, _) => "west",
        (_, 1) => "south",
        _ => "north",
    }
}

/// Shutters and window sills on non-tall residential/house buildings.
fn generate_residential_window_decorations(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    building_passages: &CoordinateBitmap,
    facade: &FacadePlan,
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
    let mut seg_idx = 0usize;

    for node in &element.nodes {
        let (x2, z2) = (node.x, node.z);
        if let Some((x1, z1)) = previous_node {
            // Rear facades get thinner dressing than street-facing ones.
            let is_rear = facade
                .segments
                .get(seg_idx)
                .and_then(|s| s.as_ref())
                .is_some_and(|s| s.class == FacadeClass::Rear);
            seg_idx += 1;
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
                if building_passages.contains(bx, bz)
                    || facade.is_party(bx, bz)
                    || facade.is_door(bx, bz)
                {
                    continue;
                }

                let mod6 = config.window_col(bx, bz); // always 0..5

                // --- Shutters ---
                // mod6 == 3 or 5 are the wall blocks flanking a window strip.
                // Both sides share the same roll (seeded on window centre).
                // Frame styles bring their own flank treatment, so skip these.
                // Only archetypes whose cols 3/5 are actually solid flanks
                // next to glazing get shutters (a strip archetype has no
                // window there; a wide band glazes col 3 itself).
                if (mod6 == 3 || mod6 == 5)
                    && config.window_frame.is_none()
                    && matches!(
                        config.window_archetype,
                        WindowArchetype::Standard3
                            | WindowArchetype::PairedNarrow
                            | WindowArchetype::ArchedTraditional
                    )
                {
                    let centre_sum = if mod6 == 3 { bx + bz - 2 } else { bx + bz + 2 };
                    let shutter_roll =
                        coord_rng(centre_sum, centre_sum, element.id).random_range(0u32..100);
                    let shutter_max = match config.detail {
                        DetailTier::Minimal => 0,
                        DetailTier::Standard => 25,
                        DetailTier::Enhanced | DetailTier::Landmark => 40,
                    };
                    if shutter_roll < shutter_max {
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
                // At each floor's floor_row==0 row we decide once per window
                // whether this floor gets a sill OR a balcony (mutually
                // exclusive).  The decision is shared across the window
                // columns via a seed derived from the window centre. Only
                // columns the archetype actually glazes get sills.
                if archetype_allows_window(config.window_archetype, mod6, 1, config.floor_cycle) {
                    // Stop a full window height before the top so every sill
                    // has a full window above it, avoids placing sills at the
                    // roof line.
                    let attic_gap = if config.attic_style {
                        config.floor_cycle
                    } else {
                        0
                    };
                    let sill_max = config.start_y_offset + config.building_height
                        - (config.floor_cycle - 1)
                        - attic_gap;
                    for h in (config.start_y_offset + config.grammar_anchor())..=sill_max {
                        if config.floor_row(h) == 0 {
                            let floor_idx = h / config.floor_cycle;

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
                            let (sill_roll_max, balcony_roll_max) =
                                if is_rear { (8, 18) } else { (15, 23) };
                            let above_ground = !config.is_ground_level
                                || h >= config.start_y_offset + 2 + config.floor_cycle;
                            let wants_balcony = mod6 == 1
                                && above_ground
                                && match config.balcony_band {
                                    BalconyBand::Scattered => {
                                        (15..balcony_roll_max).contains(&decoration_roll)
                                    }
                                    BalconyBand::EveryBay => facade.is_street(bx, bz),
                                    BalconyBand::Alternating => {
                                        facade.is_street(bx, bz)
                                            && (bx + bz + config.window_phase).div_euclid(6) % 2
                                                == 0
                                    }
                                };

                            if !wants_balcony
                                && decoration_roll < sill_roll_max
                                && config.window_frame.is_none()
                            {
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
                                let (pot_centre, pot_side) =
                                    if is_rear { (35, 12) } else { (70, 25) };
                                let pot_here = if mod6 == 1 {
                                    pot_rng.random_range(0u32..100) < pot_centre
                                } else {
                                    pot_rng.random_range(0u32..100) < pot_side
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
                            } else if wants_balcony {
                                // ── Balcony (placed once from centre col) ──
                                // Never on the ground floor; elevated parts keep their base row.
                                // A small 3-wide × 2-deep platform with
                                // open-trapdoor railing around the outer
                                // edge and occasional furniture.
                                //
                                // Top-down layout (outward = up):
                                //  depth 3:  [Tf] [Tf] [Tf]  front fence
                                //  depth 2:  [ f] [ f] [ f]  floor
                                //  depth 1:  [ f] [ f] [ f]  floor
                                //            wall wall wall
                                // Side fences at t=±2, depths 1-2.

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

                                // Side fences: trapdoors at t=±2, depths 1-2
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
    facade: &FacadePlan,
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

    // A detected street corner on a public building always gets its framing
    // column; otherwise the deterministic 60% roll decides.
    let guaranteed_corner = facade.corner.as_ref().map(|c| c.vertex).filter(|_| {
        matches!(
            config.category,
            BuildingCategory::Commercial
                | BuildingCategory::Hotel
                | BuildingCategory::Office
                | BuildingCategory::Historic
        )
    });
    let quoin_chance = if matches!(
        config.era,
        ArchEra::TraditionalPreWar | ArchEra::HistoricOrnate
    ) {
        0.9
    } else {
        0.6
    };
    let roll_passed = element_rng(element.id.wrapping_add(3571)).random_bool(quoin_chance);
    if !roll_passed && guaranteed_corner.is_none() {
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

    // Whitelist the whole substitute family, otherwise variety-substituted wall blocks break the columns.
    let mut wall_family: Vec<Block> = vec![config.wall_block];
    wall_family.extend_from_slice(substitute_pool_only(config.wall_block));

    for &(cx, cz) in &corners {
        // Party-wall vertices sit against the neighbor and get no framing.
        if facade.is_party(cx, cz) {
            continue;
        }
        // When only the street corner qualified (roll failed), frame it alone.
        if !roll_passed && guaranteed_corner != Some((cx, cz)) {
            continue;
        }
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
                Some(&wall_family),
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
    facade: &FacadePlan,
    part_probe: Option<(&CoordinateBitmap, &FnvHashSet<(i32, i32)>)>,
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
    if config.building_height < config.floor_cycle + 2
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

    // Per-building deterministic roll for probability-gated styles. Seeded on
    // the shared style seed (== element id for standalone buildings) so
    // identically-tagged parts of one building agree.
    let mut bldg_rng = element_rng(config.style_seed.wrapping_add(7919));
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

                // Skip decorative features at passage openings - the road
                // passes through here so no pilasters/buttresses/etc.
                if building_passages.contains(bx, bz)
                    || facade.is_party(bx, bz)
                    || facade.is_door(bx, bz)
                {
                    continue;
                }

                // Part protrusions stay out of any other building's cells.
                if let Some((footprints, own)) = part_probe {
                    let blocked =
                        |cx: i32, cz: i32| footprints.contains(cx, cz) && !own.contains(&(cx, cz));
                    if blocked(bx + out_nx, bz + out_nz)
                        || blocked(bx + 2 * out_nx, bz + 2 * out_nz)
                    {
                        continue;
                    }
                }

                // The wall carries a foundation down to the local terrain, so on
                // sloping ground the vertical details have to follow it down.
                let descent = if config.is_ground_level {
                    editor
                        .terrain_level(bx, bz)
                        .map_or(0, |g| (config.start_y_offset - g).max(0))
                } else {
                    0
                };

                let mod6 = config.window_col(bx, bz);

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
                            descent,
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
                            descent,
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
                            descent,
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
                                descent,
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
                            descent,
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
                            descent,
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
                            descent,
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
                                descent,
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

/// Per-building window frames: flank posts or shutters, band stairs, occasional dressing.
fn generate_window_frames(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    building_passages: &CoordinateBitmap,
    facade: &FacadePlan,
) {
    let Some(style) = config.window_frame else {
        return;
    };
    if config.use_horizontal_windows || config.category == BuildingCategory::Tower {
        return;
    }
    let (cx, cz) = match compute_building_centroid(&element.nodes) {
        Some(c) => c,
        None => return,
    };

    let top_h = config.start_y_offset + config.building_height;
    let post_block = style.post_block();
    let shutter_block = style.shutter_block();

    let mut previous_node: Option<(i32, i32)> = None;
    let mut seg_idx = 0usize;
    for node in &element.nodes {
        let (x2, z2) = (node.x, node.z);
        if let Some((x1, z1)) = previous_node {
            // Rear facades keep the band but get half the dressing.
            let is_rear = facade
                .segments
                .get(seg_idx)
                .and_then(|s| s.as_ref())
                .is_some_and(|s| s.class == FacadeClass::Rear);
            seg_idx += 1;
            let (out_nx, out_nz) = compute_outward_normal(x1, z1, x2, z2, cx, cz);
            if out_nx == 0 && out_nz == 0 {
                previous_node = Some((x2, z2));
                continue;
            }
            let facing = facing_for_normal(out_nx, out_nz);
            let band_stair = make_upside_down_stair(style.band_material(), facing);
            let shutter = shutter_block.map(|b| make_open_trapdoor(b, facing));

            let points =
                bresenham_line(x1, config.start_y_offset, z1, x2, config.start_y_offset, z2);
            for (bx, _, bz) in &points {
                let (bx, bz) = (*bx, *bz);
                if building_passages.contains(bx, bz)
                    || facade.is_party(bx, bz)
                    || facade.is_door(bx, bz)
                {
                    continue;
                }
                let col = config.window_col(bx, bz);
                let lx = bx + out_nx;
                let lz = bz + out_nz;

                if col < 3 {
                    // Band stair over each window at every floor line, plus occasional dressing.
                    for h in config.ground_floor_top()..=(top_h - 1) {
                        if config.floor_row(h) != 0 {
                            continue;
                        }
                        editor.set_block_with_properties_absolute(
                            band_stair.clone(),
                            lx,
                            h + config.abs_terrain_offset,
                            lz,
                            Some(&[AIR]),
                            None,
                        );

                        if col == 1 {
                            // One partitioned roll decides the centre dressing on the band.
                            let roll = coord_rng(
                                bx,
                                bz.wrapping_add(h),
                                config.element_id ^ 0x00F7_A3E0_D411_0001,
                            )
                            .random_range(0u32..100);
                            let above = h + 1 + config.abs_terrain_offset;
                            let (t_lantern, t_pot, t_trapdoor) =
                                if is_rear { (7, 16, 20) } else { (15, 32, 40) };
                            if h + 1 < top_h {
                                if roll < t_lantern {
                                    if style.has_lanterns() {
                                        editor.set_block_absolute(
                                            LANTERN,
                                            lx,
                                            above,
                                            lz,
                                            Some(&[AIR]),
                                            None,
                                        );
                                    }
                                } else if roll < t_pot {
                                    let mut pot_rng =
                                        coord_rng(bx, bz.wrapping_add(h * 7), config.element_id);
                                    let pot = POTTED_PLANT_OPTIONS
                                        [pot_rng.random_range(0..POTTED_PLANT_OPTIONS.len())];
                                    editor.set_block_absolute(
                                        pot,
                                        lx,
                                        above,
                                        lz,
                                        Some(&[AIR]),
                                        None,
                                    );
                                } else if roll < t_trapdoor {
                                    editor.set_block_with_properties_absolute(
                                        make_closed_trapdoor(
                                            style.detail_trapdoor(),
                                            facing,
                                            "bottom",
                                        ),
                                        lx,
                                        above,
                                        lz,
                                        Some(&[AIR]),
                                        None,
                                    );
                                }
                            }

                            // Under-band corbel shelf under the sill of the window above.
                            if h - 1 > config.start_y_offset + 2 {
                                let shelf_roll = coord_rng(
                                    bx,
                                    bz.wrapping_add(h * 3),
                                    config.element_id ^ 0x0000_5E1F_0000_0002,
                                )
                                .random_range(0u32..100);
                                if shelf_roll < 15 {
                                    editor.set_block_with_properties_absolute(
                                        make_closed_trapdoor(
                                            style.detail_trapdoor(),
                                            facing,
                                            "top",
                                        ),
                                        lx,
                                        h - 1 + config.abs_terrain_offset,
                                        lz,
                                        Some(&[AIR]),
                                        None,
                                    );
                                }
                            }
                        } else if let Some(button) = style.stud_button() {
                            // Button studs on the band front at the window edges.
                            let centre = bx + bz + if col == 0 { 1 } else { -1 };
                            let stud_roll = coord_rng(
                                centre,
                                centre.wrapping_add(h),
                                config.element_id ^ 0x0000_B417_0000_0004,
                            )
                            .random_range(0u32..100);
                            if stud_roll < 8 {
                                editor.set_block_with_properties_absolute(
                                    make_prop_block(
                                        button,
                                        &[("face", "wall"), ("facing", facing)],
                                    ),
                                    bx + 2 * out_nx,
                                    h + config.abs_terrain_offset,
                                    bz + 2 * out_nz,
                                    Some(&[AIR]),
                                    None,
                                );
                            }
                        }
                    }

                    // Hanging lantern under a band, beside the window top; one side per window.
                    if col != 1 {
                        if let Some(lantern) = style.hanging_lantern() {
                            for h in (config.start_y_offset + 3)..=(top_h - 2) {
                                if config.floor_row(h) != 3 {
                                    continue;
                                }
                                let centre = bx + bz + if col == 0 { 1 } else { -1 };
                                let roll = coord_rng(
                                    centre,
                                    centre.wrapping_add(h),
                                    config.element_id ^ 0x0000_7A96_0000_0005,
                                )
                                .random_range(0u32..100);
                                if roll < 12 && (roll % 2 == 0) == (col == 0) {
                                    editor.set_block_with_properties_absolute(
                                        make_prop_block(lantern, &[("hanging", "true")]),
                                        lx,
                                        h + config.abs_terrain_offset,
                                        lz,
                                        Some(&[AIR]),
                                        None,
                                    );
                                }
                            }
                        }
                    }
                } else if col == 3 || col == 5 {
                    // Flank treatment on window rows: posts or shutters.
                    for h in (config.start_y_offset + 2)..=(top_h - 1) {
                        if config.floor_row(h) == 0 {
                            continue;
                        }
                        let abs_y = h + config.abs_terrain_offset;
                        if let Some(post) = post_block {
                            editor.set_block_absolute(post, lx, abs_y, lz, Some(&[AIR]), None);
                        } else if let Some(ref trapdoor) = shutter {
                            editor.set_block_with_properties_absolute(
                                trapdoor.clone(),
                                lx,
                                abs_y,
                                lz,
                                Some(&[AIR]),
                                None,
                            );
                        }
                    }
                }
            }
        }
        previous_node = Some((x2, z2));
    }
}

/// String courses, crown cornice or window header trim, for styles without their own banding.
fn generate_facade_cornices(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    has_sloped_roof: bool,
    building_passages: &CoordinateBitmap,
    facade: &FacadePlan,
) {
    if !matches!(
        config.wall_depth_style,
        WallDepthStyle::SubtlePilasters | WallDepthStyle::None
    ) {
        return;
    }
    // Buildings with a window frame style already get banding from it.
    if config.window_frame.is_some() {
        return;
    }
    if config.condition != BuildingCondition::Normal
        || !config.has_windows
        || config.building_height < 2 * config.floor_cycle
    {
        return;
    }
    let bounds = BuildingBounds::from_nodes(&element.nodes);
    if bounds.width() < 4 || bounds.length() < 4 {
        return;
    }
    let (cx, cz) = match compute_building_centroid(&element.nodes) {
        Some(c) => c,
        None => return,
    };

    // 55% string courses, 40% window header trim, 5% plain.
    let roll: u32 = element_rng(config.element_id ^ 0xC0A2_11CE_0000_77AB).random_range(0..100);
    let string_courses = roll < 55;
    let window_trim = (55..95).contains(&roll);
    if !string_courses && !window_trim {
        return;
    }

    let top_h = config.start_y_offset + config.building_height;

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
            let cornice_stair = make_upside_down_stair(config.accent_block, facing);

            let points =
                bresenham_line(x1, config.start_y_offset, z1, x2, config.start_y_offset, z2);
            for (bx, _, bz) in &points {
                let (bx, bz) = (*bx, *bz);
                if building_passages.contains(bx, bz)
                    || facade.is_party(bx, bz)
                    || facade.is_door(bx, bz)
                {
                    continue;
                }
                if window_trim && config.window_col(bx, bz) >= 3 {
                    continue;
                }
                let lx = bx + out_nx;
                let lz = bz + out_nz;
                for h in config.ground_floor_top()..=top_h {
                    // Band rows double as window headers and sills of the floor above.
                    let is_band = config.floor_row(h) == 0 && h < top_h - 1;
                    let is_crown = string_courses && !has_sloped_roof && h == top_h;
                    if is_band || is_crown {
                        editor.set_block_with_properties_absolute(
                            cornice_stair.clone(),
                            lx,
                            h + config.abs_terrain_offset,
                            lz,
                            Some(&[AIR]),
                            None,
                        );
                    }
                }
            }
        }
        previous_node = Some((x2, z2));
    }
}

/// Stair headers over each window top for the ArchedTraditional archetype.
/// HistoricOrnate depth styling already places its own headers.
fn generate_archetype_window_headers(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    building_passages: &CoordinateBitmap,
    facade: &FacadePlan,
) {
    if config.window_archetype != WindowArchetype::ArchedTraditional
        || config.wall_depth_style == WallDepthStyle::HistoricOrnate
        || !config.has_windows
        || config.condition != BuildingCondition::Normal
    {
        return;
    }
    let (cx, cz) = match compute_building_centroid(&element.nodes) {
        Some(c) => c,
        None => return,
    };
    let top_h = config.start_y_offset + config.building_height;

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
            let header_stair = make_upside_down_stair(config.wall_block, facing);

            let points =
                bresenham_line(x1, config.start_y_offset, z1, x2, config.start_y_offset, z2);
            for (bx, _, bz) in &points {
                let (bx, bz) = (*bx, *bz);
                if building_passages.contains(bx, bz)
                    || facade.is_party(bx, bz)
                    || facade.is_door(bx, bz)
                {
                    continue;
                }
                let mod6 = config.window_col(bx, bz);
                // Arch shoulders sit over the window edge columns.
                if mod6 != 0 && mod6 != 2 {
                    continue;
                }
                let lx = bx + out_nx;
                let lz = bz + out_nz;
                for h in (config.start_y_offset + 2)..=top_h {
                    if config.floor_row(h) == config.floor_cycle - 1 {
                        editor.set_block_with_properties_absolute(
                            header_stair.clone(),
                            lx,
                            h + config.abs_terrain_offset,
                            lz,
                            Some(&[AIR]),
                            None,
                        );
                    }
                }
            }
        }
        previous_node = Some((x2, z2));
    }
}

/// Awning trapdoors over street-facing storefront glass.
fn generate_storefront_awnings(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    building_passages: &CoordinateBitmap,
    facade: &FacadePlan,
) {
    if !config.has_storefront || config.condition != BuildingCondition::Normal {
        return;
    }
    let (cx, cz) = match compute_building_centroid(&element.nodes) {
        Some(c) => c,
        None => return,
    };
    const AWNING_OPTIONS: [Block; 5] = [
        WARPED_TRAPDOOR,
        SPRUCE_TRAPDOOR,
        DARK_OAK_TRAPDOOR,
        JUNGLE_TRAPDOOR,
        ACACIA_TRAPDOOR,
    ];
    let awning = AWNING_OPTIONS[element_rng(config.style_seed ^ 0x0A3B_11B6_0000_000B)
        .random_range(0..AWNING_OPTIONS.len())];
    let awning_y = config.ground_floor_top() + config.abs_terrain_offset;

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
            for (bx, _, bz) in &points {
                let (bx, bz) = (*bx, *bz);
                if building_passages.contains(bx, bz)
                    || facade.is_party(bx, bz)
                    || facade.is_door(bx, bz)
                    || !facade.is_street(bx, bz)
                    || config.window_col(bx, bz) >= 4
                {
                    continue;
                }
                editor.set_block_with_properties_absolute(
                    make_closed_trapdoor(awning, facing, "top"),
                    bx + out_nx,
                    awning_y,
                    bz + out_nz,
                    Some(&[AIR]),
                    None,
                );
            }
        }
        previous_node = Some((x2, z2));
    }
}

/// Vertical drainpipe runs hugging two facade corners, a staple of hand-built city blocks.
fn generate_corner_downpipes(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    building_passages: &CoordinateBitmap,
) {
    if config.building_height < 10
        || config.condition != BuildingCondition::Normal
        || !config.has_windows
    {
        return;
    }
    if !matches!(
        config.category,
        BuildingCategory::Residential
            | BuildingCategory::House
            | BuildingCategory::Commercial
            | BuildingCategory::Hotel
            | BuildingCategory::Historic
    ) {
        return;
    }
    let mut rng = element_rng(config.element_id ^ 0xD0DA_1290_D0FF_AA01);
    if !rng.random_bool(0.35) {
        return;
    }
    let (cx, cz) = match compute_building_centroid(&element.nodes) {
        Some(c) => c,
        None => return,
    };

    let mut corners: Vec<(i32, i32)> = Vec::new();
    for node in &element.nodes {
        let pos = (node.x, node.z);
        if corners.last() != Some(&pos) && corners.first() != Some(&pos) {
            corners.push(pos);
        }
    }
    if corners.len() < 2 {
        return;
    }

    let pipe = get_wall_piece_for_material(config.wall_block);
    let start_idx = rng.random_range(0..corners.len());
    for k in 0..2usize {
        let (px, pz) = corners[(start_idx + k * corners.len() / 2) % corners.len()];
        // Diagonal outward offset so the pipe hugs the corner edge.
        let dx = (px - cx).signum();
        let dz = (pz - cz).signum();
        if (dx == 0 && dz == 0) || building_passages.contains(px, pz) {
            continue;
        }
        let (ox, oz) = (px + dx, pz + dz);
        // Follow the wall foundation down where the ground drops away.
        let descent = if config.is_ground_level {
            editor
                .terrain_level(px, pz)
                .map_or(0, |g| (config.start_y_offset - g).max(0))
        } else {
            0
        };
        for h in
            (config.start_y_offset + 1 - descent)..=(config.start_y_offset + config.building_height)
        {
            editor.set_block_absolute(
                pipe,
                ox,
                h + config.abs_terrain_offset,
                oz,
                Some(&[AIR]),
                None,
            );
        }
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
    descent: i32,
) {
    if mod6 != 3 {
        return;
    }

    let lx = bx + out_nx;
    let lz = bz + out_nz;
    let top_h = config.start_y_offset + config.building_height - height_reduction;

    for h in (config.start_y_offset + 1 - descent)..=top_h {
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
    descent: i32,
) {
    let lx = bx + out_nx;
    let lz = bz + out_nz;
    let top_h = config.start_y_offset + config.building_height - height_reduction;

    // Pillar columns at edges of window bays
    if mod6 == 3 || mod6 == 5 {
        for h in (config.start_y_offset + 1 - descent)..=top_h {
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
    descent: i32,
) {
    let lx = bx + out_nx;
    let lz = bz + out_nz;
    let top_h = config.start_y_offset + config.building_height - height_reduction;

    // Pillar columns
    if mod6 == 3 {
        for h in (config.start_y_offset + 1 - descent)..=top_h {
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
    descent: i32,
) {
    let lx = bx + out_nx;
    let lz = bz + out_nz;
    let top_h = config.start_y_offset + config.building_height - height_reduction;

    for h in (config.start_y_offset + 1 - descent)..=top_h {
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
    descent: i32,
) {
    let lx = bx + out_nx;
    let lz = bz + out_nz;

    let top_h = config.start_y_offset + config.building_height - height_reduction;

    // Full-height pillar columns between window groups
    if mod6 == 3 {
        for h in (config.start_y_offset + 1 - descent)..=top_h {
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

    // Arched window headers at window-top rows for window-edge positions
    if mod6 == 0 || mod6 == 2 {
        for h in (config.start_y_offset + 2)..=top_h {
            if config.floor_row(h) == config.floor_cycle - 1 {
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
    descent: i32,
) {
    let lx = bx + out_nx;
    let lz = bz + out_nz;
    let top_h = config.start_y_offset + config.building_height - height_reduction;

    // Buttress at every other window group center (mod6==0)
    let window_group = ((bx + bz) / 6).rem_euclid(2);
    if mod6 == 0 && window_group == 0 {
        let buttress_cutoff = config.start_y_offset + (config.building_height * 3 / 5);

        // Inner layer (outward+1): full height
        for h in (config.start_y_offset + 1 - descent)..=top_h {
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
        for h in (config.start_y_offset + 1 - descent)..=buttress_cutoff {
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
    descent: i32,
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
        for h in (config.start_y_offset + 1 - descent)..=top_h {
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
    descent: i32,
) {
    let lx = bx + out_nx;
    let lz = bz + out_nz;
    let top_h = config.start_y_offset + config.building_height - height_reduction;

    for h in (config.start_y_offset + 1 - descent)..=top_h {
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

/// Hospital green-cross wall banners on segments >= 5 blocks long.
fn generate_hospital_green_cross(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
) {
    if element.nodes.len() < 3 {
        return;
    }

    // Green cross on white background - universal pharmacy/hospital symbol.
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

        // Set ground floor - skip in passage zones (the road surface is placed
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
        if config.building_height > config.floor_cycle {
            for h in (config.start_y_offset + config.grammar_anchor() + config.floor_cycle
                ..config.start_y_offset + config.building_height)
                .step_by(config.floor_cycle as usize)
            {
                // Skip intermediate ceilings below passage opening
                if is_passage && h <= config.start_y_offset + passage_height {
                    continue;
                }

                let block = if x % 3 == 0 && z % 3 == 0 {
                    ceiling_light_block
                } else {
                    config.floor_block
                };
                editor.set_block_absolute(block, x, h + config.abs_terrain_offset, z, None, None);
            }
        } else if x % 3 == 0 && z % 3 == 0 && !is_passage {
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
        // may be generated for residential buildings without a roof:shape tag.
        //
        // Construction sites and ruins stay open at the top.
        let has_flat_roof = !generate_non_flat_roof;
        let skip_top = matches!(
            config.condition,
            BuildingCondition::Construction | BuildingCondition::Ruined
        );

        if has_flat_roof && !skip_top {
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
fn calculate_floor_levels(
    start_y_offset: i32,
    building_height: i32,
    floor_cycle: i32,
    grammar_anchor: i32,
) -> Vec<i32> {
    let mut floor_levels = vec![start_y_offset];

    if building_height > floor_cycle + 2 {
        let num_upper_floors = (building_height / floor_cycle).max(1);
        for floor in 1..num_upper_floors {
            floor_levels.push(start_y_offset + grammar_anchor + (floor * floor_cycle));
        }
    }

    floor_levels
}

/// Calculates roof peak height for chimney placement
/// Parses roof:shape tag into RoofType enum.
///
/// Tag frequencies from OSM taginfo are used to decide which synonyms
/// deserve a mapping: anything above ~0.1% is handled here so those
/// buildings get a pitched roof instead of falling through to Flat.
fn parse_roof_type(roof_shape: &str) -> RoofType {
    match roof_shape {
        "gabled" | "gable" | "pitched" | "saltbox" | "double_saltbox" | "quadruple_saltbox"
        | "gabled_row" => RoofType::Gabled,
        "hipped" | "hip" | "round" | "side_hipped" => RoofType::Hipped,
        "mansard" => RoofType::Mansard,
        "gambrel" => RoofType::Gambrel,
        "half-hipped" | "half_hipped" | "side_half-hipped" => RoofType::HalfHipped,
        "skillion" | "shed" | "lean_to" | "monopitch" => RoofType::Skillion,
        "pyramidal" | "pyramid" => RoofType::Pyramidal,
        "dome" | "spherical" => RoofType::Dome,
        "cone" | "conical" | "circular" | "spire" => RoofType::Cone,
        "onion" => RoofType::Onion,
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
#[allow(clippy::too_many_arguments)]
/// Renders one building. None when it rendered as something else (shelter, roof, car park,
/// bridge, tank, pyramid, underground) or has no street-facing wall.
pub fn generate_buildings(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    relation_levels: Option<i32>,
    hole_polygons: Option<&[HolePolygon]>,
    ctx: &BuildingContext<'_>,
    group_seed: u64,
) -> Option<FacadeAnchor> {
    let flood_fill_cache = ctx.flood_fill_cache;
    let building_passages = ctx.building_passages;
    // Early return for underground buildings
    if is_underground_building(&element.tags) {
        return None;
    }

    if SKIP_WAY_IDS.contains(&element.id) {
        return None;
    }

    // Tank-style structures route to their own cylindrical renderer.
    if crate::element_processing::man_made::is_tank_structure(element) {
        let processed_element = crate::osm_parser::ProcessedElement::Way(element.clone());
        crate::element_processing::man_made::generate_tank_structure(
            editor,
            &processed_element,
            args,
        );
        return None;
    }

    // Intercept tomb=pyramid: generate a sandstone pyramid instead of a building
    if element.tags.get("tomb").map(|v| v.as_str()) == Some("pyramid") {
        historic::generate_pyramid(editor, element, args, flood_fill_cache);
        return None;
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
    let abs_terrain_offset = if !args.terrain() {
        args.ground_level
    } else {
        0
    };

    // Get building type (tags only; also drives the floor cycle used below)
    let building_type = element
        .tags
        .get("building")
        .or_else(|| element.tags.get("building:part"))
        .map(|s| s.as_str())
        .unwrap_or("yes");
    let floor_cycle = floor_cycle_for(building_type, &element.tags);

    // Architectural era, consumed by palettes, frames, depth styles and
    // weathering. Untagged parts inherit the hint packed into the group seed.
    let mut era = crate::osm_parser::building_arch_era(&element.tags);
    if era == ArchEra::Unknown && element.tags.contains_key("building:part") {
        era = crate::osm_parser::arch_era_from_hint(crate::osm_parser::style_hint_from_seed(
            group_seed,
        ));
    }

    let min_level_offset = if let Some(mh) = element.tags.get("min_height") {
        mh.trim_end_matches('m')
            .trim()
            .parse::<f64>()
            .ok()
            .map(|h| (h * scale_factor) as i32)
            .unwrap_or(0)
    } else if min_level > 0 {
        // Matches the levels height formula (cycle per level + 2) so a skybridge
        // floor lines up with the level tops of the buildings it connects
        multiply_scale(min_level * floor_cycle + GROUND_FLOOR_BONUS, scale_factor)
    } else {
        0
    };

    // Get cached floor area. Hole carving below needs `retain`, which requires
    // ownership, so we materialize a Vec here. Buildings typically have small
    // footprints (tens to hundreds of cells), so the deep copy is cheap - the
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
        return None;
    }

    // Calculate start Y offset
    let start_y_offset = calculate_start_y_offset(editor, element, args, min_level_offset);

    // Calculate building bounds
    let bounds = BuildingBounds::from_nodes(&element.nodes);

    // Handle shelter amenity
    if element.tags.get("amenity").map(String::as_str) == Some("shelter") {
        generate_shelter(editor, element, &cached_floor_area, scale_factor);
        return None;
    }

    // Route building:part="roof" to the roof-only structure generator.
    // This must be checked before the "building" tag match below, since elements
    // with building:part="roof" (but no "building" tag) would otherwise fall
    // through to the full building pipeline and render as small boxy buildings.
    if element.tags.get("building:part").map(|v| v.as_str()) == Some("roof") {
        generate_roof_only_structure(editor, element, &cached_floor_area, args, group_seed);
        return None;
    }

    // Handle special building types with early returns
    if let Some(btype) = element.tags.get("building") {
        match btype.as_str() {
            "shed" if element.tags.contains_key("bicycle_parking") => {
                generate_bicycle_parking_shed(editor, element, &cached_floor_area);
                return None;
            }
            "parking" => {
                let (height, _) = calculate_building_height(
                    element,
                    building_type,
                    min_level,
                    scale_factor,
                    relation_levels,
                    floor_cycle,
                    cached_footprint_size,
                    group_seed,
                );
                generate_parking_building(editor, element, &cached_floor_area, height);
                return None;
            }
            "roof" => {
                generate_roof_only_structure(editor, element, &cached_floor_area, args, group_seed);
                return None;
            }
            // Skybridges with elevation data render as normal elevated buildings,
            // the flat deck below is only the fallback for untagged ones
            "bridge"
                if !element.tags.contains_key("min_height")
                    && !element.tags.contains_key("building:min_level") =>
            {
                generate_bridge(editor, element, flood_fill_cache, args.timeout.as_ref());
                return None;
            }
            _ => {}
        }

        // Also check for multi-storey parking
        if element
            .tags
            .get("parking")
            .is_some_and(|p| p == "multi-storey")
        {
            let (height, _) = calculate_building_height(
                element,
                building_type,
                min_level,
                scale_factor,
                relation_levels,
                floor_cycle,
                cached_footprint_size,
                group_seed,
            );
            generate_parking_building(editor, element, &cached_floor_area, height);
            return None;
        }
    }

    // Calculate building height (tags first, per-type inference as fallback)
    let (building_height, is_tall_building) = calculate_building_height(
        element,
        building_type,
        min_level,
        scale_factor,
        relation_levels,
        floor_cycle,
        cached_footprint_size,
        group_seed,
    );
    // Untagged towers read better on the taller commercial rhythm.
    let (floor_cycle, building_height, is_tall_building, min_level_offset) =
        if is_tall_building && building_type == "yes" && floor_cycle == 3 {
            let (h, tall) = calculate_building_height(
                element,
                building_type,
                min_level,
                scale_factor,
                relation_levels,
                4,
                cached_footprint_size,
                group_seed,
            );
            let lift = if element.tags.contains_key("min_height") {
                min_level_offset
            } else if min_level > 0 {
                multiply_scale(min_level * 4 + GROUND_FLOOR_BONUS, scale_factor)
            } else {
                min_level_offset
            };
            (4, h, tall, lift)
        } else {
            (
                floor_cycle,
                building_height,
                is_tall_building,
                min_level_offset,
            )
        };

    // Determine building category and get appropriate style preset
    let category = BuildingCategory::from_element(
        element,
        is_tall_building,
        building_height,
        group_seed,
        scale_factor,
    );
    let preset = BuildingStylePreset::for_category(category);

    // Street/neighbor classification: party walls, fronting streets, corner.
    // Sibling part cells: exempt from party-wall detection and kept clear of
    // part facade depth. Membership check guards against id collisions.
    let mut group_other_cells: FnvHashSet<(i32, i32)> = FnvHashSet::default();
    if let Some(members) = ctx
        .group_members
        .get(&crate::osm_parser::seed_without_hint(group_seed))
    {
        if members.binary_search(&element.id).is_ok() {
            for id in members {
                if *id == element.id {
                    continue;
                }
                if let Some(fill) = ctx.flood_fill_cache.get_cached(*id) {
                    group_other_cells.extend(fill.iter().copied());
                }
            }
        }
    }
    let mut facade = if min_level_offset == 0 && cached_footprint_size >= MIN_FACADE_FOOTPRINT {
        let mut own_cells: FnvHashSet<(i32, i32)> = cached_floor_area.iter().copied().collect();
        own_cells.extend(group_other_cells.iter().copied());
        compute_facade_plan(element, ctx, args.scale, &own_cells)
    } else {
        FacadePlan::empty()
    };

    // Detail budget from prominence and notable tags.
    let detail = compute_detail_tier(
        element,
        category,
        cached_footprint_size,
        building_height,
        facade.has_any_street,
    );

    // Resolve style with deterministic RNG
    let mut rng = element_rng(group_seed);
    let has_multiple_floors = building_height > floor_cycle + 2;
    let climate = editor.climate();
    let style = BuildingStyle::resolve(
        &preset,
        element,
        building_type,
        category,
        era,
        climate,
        detail,
        building_height,
        has_multiple_floors,
        cached_footprint_size,
        group_seed,
        &mut rng,
    );

    let condition = BuildingCondition::from_tags(&element.tags);
    let is_abandoned_building = matches!(
        condition,
        BuildingCondition::Abandoned | BuildingCondition::Ruined
    );

    let mut wall_block = style.wall_block;
    let mut has_windows = style.has_windows;
    let mut has_garage_door = style.has_garage_door;
    let mut has_single_door = style.has_single_door;
    let mut effective_building_height = building_height;
    match condition {
        BuildingCondition::Construction => {
            effective_building_height = (building_height / 2).max(3);
            wall_block = SCAFFOLDING;
            has_windows = false;
            has_garage_door = false;
            has_single_door = false;
        }
        BuildingCondition::Ruined => {
            effective_building_height = ((building_height as f64 * 0.6) as i32).max(3);
            has_windows = false;
            has_garage_door = false;
            has_single_door = false;
        }
        BuildingCondition::Abandoned => {
            has_garage_door = false;
            has_single_door = false;
        }
        BuildingCondition::Disused | BuildingCondition::Normal => {}
    }

    // Whether this building will get a sloped roof (drives the attic band).
    let has_sloped_roof = style.generate_roof
        && style.roof_type != RoofType::Flat
        && !matches!(
            condition,
            BuildingCondition::Construction | BuildingCondition::Ruined
        );

    // Window layout is picked before the config literal so the frame gate
    // below can see it (frames are designed around 3-wide bays).
    let window_archetype = pick_window_archetype(category, era, group_seed);

    // Vertical composition: top-floor and first-floor treatments, seeded on
    // the group seed so building:part groups agree.
    let top_treatment = has_windows
        && effective_building_height >= 4 * floor_cycle
        && !style.use_horizontal_windows
        && !matches!(
            category,
            BuildingCategory::GlassySkyscraper
                | BuildingCategory::GlassCornerSkyscraper
                | BuildingCategory::GridSkyscraper
                | BuildingCategory::Tower
        )
        && element_rng(group_seed ^ 0xF10A_401E_0000_0001).random_bool(0.45);
    let attic_style = has_windows
        && has_sloped_roof
        && effective_building_height >= 3 * floor_cycle
        && matches!(
            category,
            BuildingCategory::House | BuildingCategory::Residential | BuildingCategory::Historic
        )
        && element_rng(group_seed ^ 0xF10A_401E_0000_0002).random_bool(0.55);
    let piano_nobile = has_windows
        && effective_building_height >= 3 * floor_cycle
        && match category {
            BuildingCategory::Historic => {
                element_rng(group_seed ^ 0xF10A_401E_0000_0003).random_bool(0.50)
            }
            BuildingCategory::Hotel => {
                element_rng(group_seed ^ 0xF10A_401E_0000_0003).random_bool(0.20)
            }
            _ => false,
        };

    // Create config struct for cleaner function calls
    let config = BuildingConfig {
        is_ground_level: min_level_offset == 0,
        building_height: effective_building_height,
        floor_cycle,
        is_tall_building,
        start_y_offset,
        abs_terrain_offset,
        wall_block,
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
        has_windows,
        has_garage_door,
        has_single_door,
        category,
        era,
        detail,
        top_treatment,
        attic_style,
        piano_nobile,
        wall_depth_style: style.wall_depth_style,
        has_parapet: style.has_parapet
            || short_flat_parapet_for(
                group_seed,
                style.roof_type,
                effective_building_height,
                category,
                condition,
            ),
        has_lobby_base: if category == BuildingCategory::ModernSkyscraper {
            element_rng(group_seed.wrapping_add(6143)).random_bool(0.70)
        } else {
            false
        },
        condition,
        element_id: element.id,
        style_seed: group_seed,
        // Parts of one building must share a phase, so they stay world-coord aligned.
        window_phase: if element.tags.contains_key("building:part") {
            0
        } else {
            element_rng(element.id ^ 0x77D0_A3E1_9B1C_5544).random_range(0..6)
        },
        window_archetype,
        balcony_band: pick_balcony_band(
            category,
            effective_building_height,
            floor_cycle,
            facade.has_any_street,
            group_seed,
        ),
        rustication: matches!(
            category,
            BuildingCategory::House
                | BuildingCategory::Residential
                | BuildingCategory::Commercial
                | BuildingCategory::Hotel
                | BuildingCategory::Historic
        ) && min_level_offset == 0
            && condition == BuildingCondition::Normal
            && detail >= DetailTier::Standard
            && match era {
                ArchEra::HistoricOrnate => true,
                ArchEra::TraditionalPreWar => {
                    element_rng(group_seed ^ 0x0BA5_E5A0_0000_000A).random_bool(0.70)
                }
                _ => false,
            },
        base_course_block: {
            let eligible = min_level_offset == 0
                && has_windows
                && condition == BuildingCondition::Normal
                && !matches!(
                    category,
                    BuildingCategory::Greenhouse
                        | BuildingCategory::Shed
                        | BuildingCategory::Garage
                        | BuildingCategory::GlassySkyscraper
                );
            let base = base_course_for_wall(wall_block);
            (eligible
                && base != wall_block
                && element_rng(group_seed ^ 0xBA5E_C0A2_5E11_0001).random_bool(0.70))
            .then_some(base)
        },
        has_storefront: matches!(
            category,
            BuildingCategory::Commercial | BuildingCategory::Hotel
        ) && has_windows
            && condition == BuildingCondition::Normal
            && min_level_offset == 0
            && facade.has_any_street
            && element_rng(group_seed ^ 0x5709_EF90_0000_0002)
                .random_bool(if facade.corner.is_some() { 0.85 } else { 0.60 }),
        window_frame: (has_windows
            && condition == BuildingCondition::Normal
            && !is_tall_building
            && matches!(
                window_archetype,
                WindowArchetype::Standard3
                    | WindowArchetype::PairedNarrow
                    | WindowArchetype::ArchedTraditional
            ))
        .then(|| pick_window_frame(category, era, detail, wall_block, group_seed))
        .flatten(),
    };

    // Passages only apply to ground-level buildings. Elevated building:part
    // elements (min_level > 0) sit above the passage and must keep their
    // walls, floors and decorations intact.
    let empty_passages = CoordinateBitmap::new_empty();
    let empty_facade = FacadePlan::empty();
    let effective_passages: &CoordinateBitmap = if config.is_ground_level {
        building_passages
    } else {
        &empty_passages
    };

    // Podium + tower massing: shell at podium height, tower after the roof pass.
    let podium_tower = plan_podium_tower(
        element,
        &config,
        style.roof_type,
        style.generate_roof,
        cached_footprint_size,
        &cached_floor_area,
        group_seed,
    );
    let config = match &podium_tower {
        Some(plan) => BuildingConfig {
            building_height: plan.podium_height,
            ..config.clone()
        },
        None => config,
    };
    // Interiors must stop at the podium roof or rooms would float.
    let effective_building_height = match &podium_tower {
        Some(plan) => plan.podium_height,
        None => effective_building_height,
    };

    // Entrances are planned before the decoration passes so their columns
    // stay clear of shutters, sills and pilasters.
    let mut entrance_plans = plan_mapped_entrances(element, &config, &facade, group_seed);
    if entrance_plans.is_empty() {
        if let Some(plan) =
            plan_synthetic_entrance(element, &config, &facade, effective_passages, group_seed)
        {
            entrance_plans.push(plan);
        }
    }
    for plan in &entrance_plans {
        let leaves = if plan.double { 2 } else { 1 };
        for t in 0..leaves {
            let dx = plan.x + plan.tangent.0 * t;
            let dz = plan.z + plan.tangent.1 * t;
            facade.mark_door_column(dx, dz);
            facade.mark_door_column(dx + plan.normal.0, dz + plan.normal.1);
        }
    }

    let (wall_outline, corner_count) = build_wall_ring(
        editor,
        &element.nodes,
        &config,
        args,
        has_sloped_roof,
        effective_passages,
        &facade,
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
                    &empty_facade,
                );
            }
        }
    }

    // Generate special doors (garage doors, shed doors)
    if config.has_garage_door || config.has_single_door {
        generate_special_doors(editor, element, &config, &wall_outline, effective_passages);
    }

    // Entrance doors overwrite the freshly built wall columns.
    for plan in &entrance_plans {
        render_entrance(editor, plan, &config, args);
    }

    // Per-building window frame dressing, then shutters/window boxes for the rest
    if !element.tags.contains_key("building:part") {
        generate_window_frames(editor, element, &config, effective_passages, &facade);
    }
    generate_residential_window_decorations(editor, element, &config, effective_passages, &facade);

    // Add wall depth features (pilasters, columns, ledges, cornices, buttresses).
    // building:part sub-sections get them too, with a sibling-cell probe so
    // protrusions stay clear of adjoining parts.
    let is_part = element.tags.contains_key("building:part");
    let part_own_cells: FnvHashSet<(i32, i32)> = if is_part {
        cached_floor_area
            .iter()
            .copied()
            .chain(group_other_cells.iter().copied())
            .collect()
    } else {
        FnvHashSet::default()
    };
    let part_probe = is_part.then_some((ctx.building_footprints, &part_own_cells));
    generate_wall_depth_features(
        editor,
        element,
        &config,
        has_sloped_roof,
        effective_passages,
        &facade,
        part_probe,
    );

    // Add corner quoins (accent-block columns at building corners)
    if !element.tags.contains_key("building:part") {
        generate_corner_quoins(editor, element, &config, effective_passages, &facade);
        generate_facade_cornices(
            editor,
            element,
            &config,
            has_sloped_roof,
            effective_passages,
            &facade,
        );
        generate_corner_downpipes(editor, element, &config, effective_passages);
        generate_archetype_window_headers(editor, element, &config, effective_passages, &facade);
        generate_storefront_awnings(editor, element, &config, effective_passages, &facade);
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
    if corner_count > 0 {
        generate_floors_and_ceilings(
            editor,
            &cached_floor_area,
            &config,
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
            ) || matches!(
                config.condition,
                BuildingCondition::Construction | BuildingCondition::Ruined
            );

            if !skip_interior && cached_floor_area.len() > 100 {
                let floor_levels = calculate_floor_levels(
                    start_y_offset,
                    effective_building_height,
                    config.floor_cycle,
                    config.grammar_anchor(),
                );
                generate_building_interior(
                    editor,
                    &cached_floor_area,
                    bounds.min_x,
                    bounds.min_z,
                    bounds.max_x,
                    bounds.max_z,
                    start_y_offset,
                    effective_building_height,
                    style.wall_block,
                    &floor_levels,
                    abs_terrain_offset,
                    is_abandoned_building,
                    effective_passages,
                    has_sloped_roof,
                );
            }
        }
    }

    // Construction/Ruined buildings stay roofless.
    let skip_roof = matches!(
        config.condition,
        BuildingCondition::Construction | BuildingCondition::Ruined
    );
    if style.generate_roof && !skip_roof {
        // Terraced fabric prefers its ridge parallel to the fronting street.
        let preferred_ridge_along_x = if matches!(building_type, "terrace" | "semidetached_house") {
            facade
                .front_segment
                .and_then(|i| facade.segments[i].as_ref())
                .map(|seg| seg.normal.0 == 0)
        } else {
            None
        };
        let covered_by_sibling_part = is_part
            && cached_floor_area
                .iter()
                .any(|c| group_other_cells.contains(c));
        generate_building_roof(
            editor,
            element,
            &config,
            &style,
            &bounds,
            &roof_area,
            category,
            preferred_ridge_along_x,
            podium_tower.is_some(),
            covered_by_sibling_part,
            scale_factor,
        );

        // Tower ring above the podium roof; full height so top-floor
        // treatment stays at the actual top.
        if let Some(plan) = &podium_tower {
            let dist = roof_edge_distances(&roof_area);
            let tower = [InsetTier {
                inset: plan.inset,
                height: plan.full_height - plan.podium_height,
                with_ceilings: true,
            }];
            let tower_config = BuildingConfig {
                building_height: plan.full_height,
                ..config.clone()
            };
            generate_inset_tiers(
                editor,
                &tower_config,
                &roof_area,
                &dist,
                config.start_y_offset + config.building_height + 1,
                &tower,
            );
        }
    }

    facade_anchor(element, &facade, &config, &entrance_plans)
}

/// Sign anchor: the entrance column, else the middle of the front wall.
fn facade_anchor(
    element: &ProcessedWay,
    facade: &FacadePlan,
    config: &BuildingConfig,
    entrances: &[EntrancePlan],
) -> Option<FacadeAnchor> {
    let abs = config.abs_terrain_offset;
    // First row above the ground floor, clearing the storefront glazing and its awning.
    let fascia_y = config.ground_floor_top() + 1 + abs;
    let door_y = config.start_y_offset + abs + 1;

    if let Some(plan) = entrances.first() {
        return Some(FacadeAnchor {
            x: plan.x,
            z: plan.z,
            normal: plan.normal,
            fascia_y: fascia_y.max(door_y + 3),
            number_y: door_y + 1,
            door: Some((plan.x, plan.z)),
        });
    }

    let front = facade.front_segment?;
    let seg = facade.segments[front].as_ref()?;
    // Segment i spans nodes[i]..nodes[i+1]; hang the plate on the middle of that wall.
    let (a, b) = (element.nodes.get(front)?, element.nodes.get(front + 1)?);
    let cells = bresenham_line(a.x, 0, a.z, b.x, 0, b.z);
    let (x, _, z) = *cells.get(cells.len() / 2)?;
    Some(FacadeAnchor {
        x,
        z,
        normal: seg.normal,
        fascia_y,
        number_y: door_y + 1,
        door: None,
    })
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
#[allow(clippy::too_many_arguments)]
fn generate_building_roof(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    style: &BuildingStyle,
    bounds: &BuildingBounds,
    roof_area: &[(i32, i32)],
    category: BuildingCategory,
    preferred_ridge_along_x: Option<bool>,
    suppress_flat_extras: bool,
    covered_by_sibling_part: bool,
    scale_factor: f64,
) {
    // Dormers: house/residential pitched roofs, plus historic mansards.
    let add_dormers = config.condition == BuildingCondition::Normal
        && ((matches!(
            category,
            BuildingCategory::House | BuildingCategory::Residential
        ) && matches!(
            style.roof_type,
            RoofType::Gabled | RoofType::Hipped | RoofType::Mansard
        )) || (category == BuildingCategory::Historic && style.roof_type == RoofType::Mansard));

    // Churches carry visibly steeper gables than houses.
    let steep_gable = category == BuildingCategory::Religious;

    // roof:colour/material on a part means the mapper modeled this surface, keep it clean
    let modeled_part_roof = element.tags.contains_key("building:part")
        && (element.tags.contains_key("roof:colour") || element.tags.contains_key("roof:material"));

    // Generate the roof using the pre-determined roof type from style
    generate_roof(
        editor,
        element,
        config.start_y_offset,
        config.building_height,
        config.floor_block,
        config.wall_block,
        config.roof_block,
        style.roof_type,
        roof_area,
        config.abs_terrain_offset,
        add_dormers,
        config.style_seed,
        steep_gable,
        preferred_ridge_along_x,
        scale_factor,
    );

    // Add parapet on flat-roofed buildings
    if config.has_parapet && style.roof_type == RoofType::Flat {
        generate_parapet(editor, element, config);
    }

    // Add decorative roofline variation on flat-roofed residential/generic buildings
    // (those that don't already have a parapet or non-flat roof)
    if !config.has_parapet && style.roof_type == RoofType::Flat && !modeled_part_roof {
        generate_flat_roof_edge_variation(editor, element, config);
    }

    // Stepped setback crowns and wooden water towers on flat roofs.
    // A planned tower rises from this roof, so it gets neither.
    let mut has_crown = suppress_flat_extras;
    let mut water_tower_at: Option<(i32, i32)> = None;
    if !suppress_flat_extras
        && style.roof_type == RoofType::Flat
        && !element.tags.contains_key("building:part")
    {
        has_crown = generate_setback_crown(editor, config, roof_area);
        if !has_crown {
            water_tower_at = generate_water_tower(editor, config, roof_area, category);
        }
    }

    // chimneys only on pitched roofs, presets cannot force one onto flat
    if style.has_chimney
        && matches!(
            style.roof_type,
            RoofType::Gabled
                | RoofType::Hipped
                | RoofType::Mansard
                | RoofType::Gambrel
                | RoofType::HalfHipped
        )
    {
        generate_chimney(
            editor,
            roof_area,
            bounds.min_x,
            bounds.max_x,
            bounds.min_z,
            bounds.max_z,
            config.start_y_offset + config.building_height + 1,
            config.abs_terrain_offset,
            element.id,
        );
    }

    // no rooftop extras when a sibling part is stacked on top
    let has_terrace = !modeled_part_roof
        && !covered_by_sibling_part
        && should_generate_roof_terrace(element, config, style.roof_type);
    if has_terrace {
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

    if !has_crown
        && !has_terrace
        && !modeled_part_roof
        && !covered_by_sibling_part
        && (should_generate_rooftop_equipment(config, style.roof_type, category)
            || short_flat_rooftop_bits_for(
                element.id,
                style.roof_type,
                config.building_height,
                category,
                config.condition,
                config.detail,
            ))
    {
        let roof_y = config.start_y_offset + config.building_height;
        generate_rooftop_equipment(
            editor,
            element,
            roof_area,
            roof_y,
            config.abs_terrain_offset,
            water_tower_at,
        );
    }

    // Lightning rod on peaked residential houses, 5% chance.
    if matches!(category, BuildingCategory::House)
        && config.condition == BuildingCondition::Normal
        && matches!(
            style.roof_type,
            RoofType::Gabled | RoofType::Hipped | RoofType::Pyramidal
        )
    {
        generate_residential_antenna(editor, element, roof_area, config);
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

/// Compact bbox-indexed distance grid, 4 bytes per cell instead of a hash map.
struct RoofDistanceGrid {
    min_x: i32,
    min_z: i32,
    width: i32,
    height: i32,
    dist: Vec<i32>,
}

impl RoofDistanceGrid {
    /// Distance to the nearest roof edge, -1 outside the roof.
    fn get(&self, x: i32, z: i32) -> i32 {
        let (lx, lz) = (x - self.min_x, z - self.min_z);
        if lx < 0 || lz < 0 || lx >= self.width || lz >= self.height {
            return -1;
        }
        self.dist[(lz * self.width + lx) as usize]
    }
}

/// Per-cell BFS distance to the nearest roof edge.
fn roof_edge_distances(roof_area: &[(i32, i32)]) -> RoofDistanceGrid {
    let (mut min_x, mut max_x, mut min_z, mut max_z) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for &(x, z) in roof_area {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_z = min_z.min(z);
        max_z = max_z.max(z);
    }
    let width = max_x - min_x + 1;
    let height = max_z - min_z + 1;
    let mut dist = vec![-1i32; (width * height) as usize];
    let idx = |x: i32, z: i32| ((z - min_z) * width + (x - min_x)) as usize;

    for &(x, z) in roof_area {
        dist[idx(x, z)] = i32::MAX;
    }
    let mut queue: VecDeque<(i32, i32)> = VecDeque::new();
    for &(x, z) in roof_area {
        let on_edge = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dz)| {
            let (nx, nz) = (x + dx, z + dz);
            nx < min_x || nx > max_x || nz < min_z || nz > max_z || dist[idx(nx, nz)] == -1
        });
        if on_edge {
            dist[idx(x, z)] = 0;
            queue.push_back((x, z));
        }
    }
    while let Some((x, z)) = queue.pop_front() {
        let d = dist[idx(x, z)];
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let (nx, nz) = (x + dx, z + dz);
            if nx < min_x || nx > max_x || nz < min_z || nz > max_z {
                continue;
            }
            let i = idx(nx, nz);
            if dist[i] == i32::MAX {
                dist[i] = d + 1;
                queue.push_back((nx, nz));
            }
        }
    }
    RoofDistanceGrid {
        min_x,
        min_z,
        width,
        height,
        dist,
    }
}

/// Stepped setback crown on 35% of tall flat-roofed towers, the classic art deco silhouette.
/// Podium + tower massing for a large tall building mapped as one outline.
struct PodiumTowerPlan {
    podium_height: i32,
    inset: i32,
    full_height: i32,
}

/// Big flat-roofed towers on large footprints read better as a 2-3 storey
/// podium with an inset tower than as one sheer extrusion. Mapped
/// `building:part` massing always wins (parts never take this path).
fn plan_podium_tower(
    element: &ProcessedWay,
    config: &BuildingConfig,
    roof_type: RoofType,
    generate_roof_flag: bool,
    footprint_size: usize,
    floor_area: &[(i32, i32)],
    group_seed: u64,
) -> Option<PodiumTowerPlan> {
    if !config.is_tall_building
        || config.building_height < 30
        || footprint_size < 600
        || roof_type != RoofType::Flat
        || !generate_roof_flag
        || config.condition != BuildingCondition::Normal
        || element.tags.contains_key("building:part")
    {
        return None;
    }
    let mut rng = element_rng(group_seed ^ 0x90D1_0A70_0000_0001);
    if !rng.random_bool(0.40) {
        return None;
    }
    let inset = if footprint_size < 1200 {
        3
    } else {
        4 + rng.random_range(0..2)
    };
    // Validate the tower footprint before committing to the massing.
    let dist = roof_edge_distances(floor_area);
    let tower_cells = floor_area
        .iter()
        .filter(|&&(x, z)| dist.get(x, z) >= inset)
        .count();
    if tower_cells < 150 || tower_cells * 4 < footprint_size {
        return None;
    }
    let podium_floors = 2 + rng.random_range(0..2);
    let podium_height = podium_floors * config.floor_cycle + GROUND_FLOOR_BONUS;
    if config.building_height - podium_height < 2 * config.floor_cycle {
        return None;
    }
    Some(PodiumTowerPlan {
        podium_height,
        inset,
        full_height: config.building_height,
    })
}

/// One inset ring of a tiered top (setback crown tier, or a whole tower).
struct InsetTier {
    inset: i32,
    height: i32,
    /// Lay interior ceiling plates with the usual light grid inside the ring.
    with_ceilings: bool,
}

/// Places stacked inset wall rings above `base_rel` using the building's own
/// facade grammar (window bands continue upward), capping each tier with a
/// floor plate. Returns the relative Y of the top cap, or None when even the
/// first tier is too small.
fn generate_inset_tiers(
    editor: &mut WorldEditor,
    config: &BuildingConfig,
    roof_area: &[(i32, i32)],
    dist: &RoofDistanceGrid,
    base_rel: i32,
    tiers: &[InsetTier],
) -> Option<i32> {
    let mut current_base = base_rel;
    let mut placed = false;
    for tier in tiers {
        let tier_cells: Vec<(i32, i32)> = roof_area
            .iter()
            .copied()
            .filter(|&(x, z)| dist.get(x, z) >= tier.inset)
            .collect();
        if tier_cells.len() < 30 {
            break;
        }
        placed = true;

        for &(x, z) in &tier_cells {
            let is_wall = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .any(|&(dx, dz)| dist.get(x + dx, z + dz) < tier.inset);
            if is_wall {
                // Same wall and window logic as the facade, so bands continue upward.
                for h in 0..tier.height {
                    let block = determine_wall_block_at_position(
                        x,
                        current_base + h,
                        z,
                        config,
                        ColumnFacade::default(),
                    );
                    editor.set_block_absolute(
                        block,
                        x,
                        current_base + h + config.abs_terrain_offset,
                        z,
                        None,
                        Some(&[]),
                    );
                }
            } else if tier.with_ceilings {
                for h in 0..tier.height {
                    let gy = current_base + h;
                    if config.floor_row(gy) == 0 {
                        let block = if x % 3 == 0 && z % 3 == 0 {
                            GLOWSTONE
                        } else {
                            config.floor_block
                        };
                        editor.set_block_absolute(
                            block,
                            x,
                            gy + config.abs_terrain_offset,
                            z,
                            None,
                            Some(&[]),
                        );
                    }
                }
            }
            editor.set_block_absolute(
                config.floor_block,
                x,
                current_base + tier.height + config.abs_terrain_offset,
                z,
                None,
                Some(&[]),
            );
        }
        current_base += tier.height;
    }
    placed.then_some(current_base)
}

fn generate_setback_crown(
    editor: &mut WorldEditor,
    config: &BuildingConfig,
    roof_area: &[(i32, i32)],
) -> bool {
    if !config.is_tall_building
        || config.condition != BuildingCondition::Normal
        || roof_area.len() < 200
    {
        return false;
    }
    let mut rng = element_rng(config.element_id ^ 0x5E7B_AC4C_0000_0001);
    if !rng.random_bool(0.45) {
        return false;
    }

    let dist = roof_edge_distances(roof_area);
    let roof_rel = config.start_y_offset + config.building_height + 1;
    // Masonry towers get the full art-deco wedding cake.
    let tier_count = if config.category == BuildingCategory::MasonrySkyscraper {
        3
    } else {
        2
    };
    let tiers: Vec<InsetTier> = (0..tier_count)
        .map(|t| InsetTier {
            inset: 3 + t * 3,
            height: config.floor_cycle,
            with_ceilings: false,
        })
        .collect();
    let Some(top) = generate_inset_tiers(editor, config, roof_area, &dist, roof_rel, &tiers) else {
        return false;
    };

    // Small mast on the crown centre, on the deepest cell.
    if let Some(&(mx, mz)) = roof_area.iter().max_by_key(|&&(x, z)| dist.get(x, z)) {
        if dist.get(mx, mz) >= 6 {
            let top_abs = top + 1 + config.abs_terrain_offset;
            for h in 0..3 {
                editor.set_block_absolute(IRON_BARS, mx, top_abs + h, mz, None, Some(&[]));
            }
            editor.set_block_absolute(LIGHTNING_ROD, mx, top_abs + 3, mz, None, Some(&[]));
        }
    }

    true
}

/// Wooden rooftop water tank on legs, a staple of brick mid-rises. 18% of eligible flat roofs.
fn generate_water_tower(
    editor: &mut WorldEditor,
    config: &BuildingConfig,
    roof_area: &[(i32, i32)],
    category: BuildingCategory,
) -> Option<(i32, i32)> {
    // Only mid-rises with a real base: 4+ levels and a decent roof area.
    if config.building_height < 16
        || roof_area.len() < 300
        || config.condition != BuildingCondition::Normal
    {
        return None;
    }
    if matches!(
        category,
        BuildingCategory::GlassySkyscraper
            | BuildingCategory::ModernSkyscraper
            | BuildingCategory::Religious
            | BuildingCategory::Hospital
    ) {
        return None;
    }
    let mut rng = element_rng(config.element_id ^ 0x3A7E_12F0_0000_0002);
    if !rng.random_bool(0.18) {
        return None;
    }

    // roof_area is sorted at construction, so membership is a binary search.
    let on_roof = |x: i32, z: i32| roof_area.binary_search(&(x, z)).is_ok();
    let spots: Vec<(i32, i32)> = roof_area
        .iter()
        .copied()
        .filter(|&(x, z)| (-2..=2).all(|dx| (-2..=2).all(|dz| on_roof(x + dx, z + dz))))
        .collect();
    if spots.is_empty() {
        return None;
    }
    let (cx, cz) = spots[rng.random_range(0..spots.len())];
    let base = config.start_y_offset + config.building_height + 2 + config.abs_terrain_offset;
    let replace_any: &[Block] = &[];

    for (lx, lz) in [
        (cx - 1, cz - 1),
        (cx + 1, cz - 1),
        (cx - 1, cz + 1),
        (cx + 1, cz + 1),
    ] {
        for h in 0..2 {
            editor.set_block_absolute(SPRUCE_FENCE, lx, base + h, lz, None, Some(replace_any));
        }
    }
    for dx in -1..=1 {
        for dz in -1..=1 {
            for h in 2..5 {
                editor.set_block_absolute(
                    SPRUCE_PLANKS,
                    cx + dx,
                    base + h,
                    cz + dz,
                    None,
                    Some(replace_any),
                );
            }
            let cap = if dx == 0 && dz == 0 {
                SPRUCE_PLANKS
            } else {
                SPRUCE_SLAB
            };
            editor.set_block_absolute(cap, cx + dx, base + 5, cz + dz, None, Some(replace_any));
        }
    }
    editor.set_block_absolute(SPRUCE_SLAB, cx, base + 6, cz, None, Some(replace_any));
    Some((cx, cz))
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
    roof_base: i32,
    abs_terrain_offset: i32,
    element_id: u64,
) {
    if floor_area.is_empty() {
        return;
    }
    let footprint: HashSet<(i32, i32)> = floor_area.iter().copied().collect();
    // one step inside the edge the surface is low in every roof profile
    let near_eave = |x: i32, z: i32| {
        [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)]
            .iter()
            .all(|n| footprint.contains(n))
            && [(x - 2, z), (x + 2, z), (x, z - 2), (x, z + 2)]
                .iter()
                .any(|n| !footprint.contains(n))
    };

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
            in_quadrant && near_eave(*x, *z)
        })
        .copied()
        .collect();

    // If no good candidates in the quadrant, try any eave-adjacent point
    let final_candidates = if candidate_points.is_empty() {
        floor_area
            .iter()
            .filter(|(x, z)| near_eave(*x, *z))
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

    // short shaft above the roof base, embedded below and clear on top
    let chimney_base = roof_base + 1;
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

    // Cap: 40% get an empty flower pot as a chimney pot, the rest a stone brick slab.
    let cap_y = chimney_base + chimney_height + abs_terrain_offset;
    if rng.random_bool(0.4) {
        editor.set_block_absolute(
            EMPTY_FLOWER_POT,
            chimney_x,
            cap_y,
            chimney_z,
            None,
            Some(replace_any),
        );
    } else {
        editor.set_block_absolute(
            STONE_BRICK_SLAB,
            chimney_x,
            cap_y,
            chimney_z,
            None,
            Some(replace_any), // Empty blacklist = replace any block
        );
    }
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
/// Lightning rod on top of peaked-roof houses, 5% chance.
fn generate_residential_antenna(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    roof_area: &[(i32, i32)],
    config: &BuildingConfig,
) {
    if roof_area.len() < 30 {
        return;
    }

    let mut rng = element_rng(element.id ^ 0x04E7_E44A);
    if !rng.random_bool(0.05) {
        return;
    }

    let footprint: HashSet<(i32, i32)> = roof_area.iter().copied().collect();
    let cx = roof_area.iter().map(|p| p.0).sum::<i32>() / roof_area.len() as i32;
    let cz = roof_area.iter().map(|p| p.1).sum::<i32>() / roof_area.len() as i32;

    let interior: Vec<(i32, i32)> = roof_area
        .iter()
        .filter(|&&(x, z)| {
            footprint.contains(&(x - 1, z))
                && footprint.contains(&(x + 1, z))
                && footprint.contains(&(x, z - 1))
                && footprint.contains(&(x, z + 1))
        })
        .copied()
        .collect();
    if interior.is_empty() {
        return;
    }
    let (best_x, best_z) = *interior
        .iter()
        .min_by_key(|&&(x, z)| (x - cx).pow(2) + (z - cz).pow(2))
        .unwrap();

    // Match the slope formula used by gabled/hipped/pyramidal plus the +1 lift.
    let min_x = roof_area.iter().map(|p| p.0).min().unwrap();
    let max_x = roof_area.iter().map(|p| p.0).max().unwrap();
    let min_z = roof_area.iter().map(|p| p.1).min().unwrap();
    let max_z = roof_area.iter().map(|p| p.1).max().unwrap();
    let narrow_half = (max_x - min_x).min(max_z - min_z) / 2;
    let local_boost = ((narrow_half as f64) * 0.85).round().max(1.0) as i32;
    let wall_cap = ((config.building_height as f64) * 0.6).round().max(1.0) as i32;
    let estimated_rise = local_boost.min(wall_cap) + 1;
    let anchor_y = config.start_y_offset
        + config.building_height
        + estimated_rise
        + 2
        + config.abs_terrain_offset;
    editor.set_block_absolute(LIGHTNING_ROD, best_x, anchor_y, best_z, None, None);
}

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
    reserved: Option<(i32, i32)>,
) {
    if floor_area.is_empty() {
        return;
    }

    let replace_any: &[Block] = &[];
    let equip_y = roof_y + abs_terrain_offset + 2; // On top of the flat roof surface

    // keep equipment off the outline ring where the parapet sits
    let raw_set: HashSet<(i32, i32)> = floor_area.iter().copied().collect();
    let floor_set: HashSet<(i32, i32)> = raw_set
        .iter()
        .filter(|&&(x, z)| {
            [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)]
                .iter()
                .all(|n| raw_set.contains(n))
        })
        .copied()
        .collect();

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
    if let Some((wx, wz)) = reserved {
        // Keep the water tower footprint and a margin clear.
        for dx in -3..=3 {
            for dz in -3..=3 {
                used.insert((wx + dx, wz + dz));
            }
        }
    }

    for &(x, z) in &interior {
        if used.contains(&(x, z)) {
            continue;
        }

        let mut rng = coord_rng(x, z, element.id ^ 0xE90B_375E_ED00_1001);
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
    add_dormers: bool,
    element_id_for_decor: u64,
    /// Roof rise in blocks from a roof:height tag, overriding the heuristics.
    peak_cap: Option<i32>,
}

impl RoofConfig {
    /// Creates RoofConfig from roof area (includes wall outline for proper coverage)
    fn from_roof_area(
        roof_area: &[(i32, i32)],
        element_id: u64,
        start_y_offset: i32,
        building_height: i32,
        wall_block: Block,
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

        let mut rng = element_rng(element_id ^ 0xA11E_D700_F1A7_5EED);

        // 15% stone bricks override regardless of wall.
        let mut stone_brick_rng = element_rng(element_id ^ 0x57_4F_E4_8B_1C_42_E0_91);
        let force_stone_bricks = stone_brick_rng.random_bool(0.15);

        // 10% same-family variety from the wall's substitute pool.
        let raw_roof = if force_stone_bricks {
            STONE_BRICKS
        } else if rng.random_bool(0.1) {
            let pool = substitute_pool_only(wall_block);
            if pool.is_empty() {
                wall_block
            } else {
                pool[rng.random_range(0..pool.len())]
            }
        } else {
            wall_block
        };
        let roof_block = roof_friendly_block(raw_roof);

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
            add_dormers: false,
            element_id_for_decor: element_id,
            peak_cap: None,
        }
    }

    fn width(&self) -> i32 {
        self.max_x - self.min_x
    }

    fn length(&self) -> i32 {
        self.max_z - self.min_z
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

/// Places 3-block-wide dormer protrusions along a pitched roof slope.
#[allow(clippy::too_many_arguments)]
fn place_dormer_windows(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    roof_heights: &HashMap<(i32, i32), i32>,
    edge_scans: &HashMap<(i32, i32), (i32, i32)>,
    config: &RoofConfig,
    parallel_to_ridge: (i32, i32),
    footprint: &HashSet<(i32, i32)>,
    mansard_band: Option<i32>,
) {
    if !config.add_dormers {
        return;
    }

    // second row at base+2/base+3, the mansard band comes from the caller
    let target_h = config.base_height + 2;
    let alt_target_h = config.base_height + 3;
    let (px, pz) = parallel_to_ridge;

    let mut candidates: Vec<(i32, i32, i32)> = Vec::new();
    for &(x, z) in floor_area {
        let h = match roof_heights.get(&(x, z)) {
            Some(&h) if h == target_h || h == alt_target_h || mansard_band == Some(h) => h,
            _ => continue,
        };

        let (dm_perp, dp_perp) = match edge_scans.get(&(x, z)) {
            Some(&pair) => pair,
            None => continue,
        };
        let max_perp = dm_perp.max(dp_perp);
        if max_perp < 4 {
            continue;
        }

        let left = (x - px, z - pz);
        let right = (x + px, z + pz);
        let same = roof_heights.get(&left) == Some(&h) && roof_heights.get(&right) == Some(&h);
        if !same {
            continue;
        }

        let further_left = (x - 2 * px, z - 2 * pz);
        let further_right = (x + 2 * px, z + 2 * pz);
        if !footprint.contains(&further_left) || !footprint.contains(&further_right) {
            continue;
        }

        // roof must keep rising inward, else capped flat tops sprout dormers
        let inward = if dm_perp <= dp_perp { 1 } else { -1 };
        let above = (x + pz * inward, z + px * inward);
        if roof_heights.get(&above).is_none_or(|&ah| ah <= h) {
            continue;
        }

        candidates.push((x, z, h));
    }

    if candidates.is_empty() {
        return;
    }

    candidates.sort_unstable();
    let target_count = (candidates.len() / 12).clamp(1, 3);

    let mut rng = element_rng(config.element_id_for_decor ^ 0x000D_04E4_DEC0);
    let mut chosen: Vec<(i32, i32, i32)> = Vec::new();
    let mut attempts = 0;
    while chosen.len() < target_count && attempts < candidates.len() * 4 {
        attempts += 1;
        let idx = rng.random_range(0..candidates.len());
        let pos = candidates[idx];
        // Ridge-axis spacing of at least 4 blocks between dormers.
        let too_close = chosen.iter().any(|&(cx, cz, _)| {
            let d = ((cx - pos.0) * px + (cz - pos.1) * pz).abs();
            d < 4
        });
        if !too_close {
            chosen.push(pos);
        }
    }

    // Cap ridge runs 90° to the main ridge.
    let (left_facing, right_facing) = if px != 0 {
        (StairFacing::East, StairFacing::West)
    } else {
        (StairFacing::South, StairFacing::North)
    };

    let perp_unit: (i32, i32) = if px != 0 { (0, 1) } else { (1, 0) };

    let abs = config.abs_terrain_offset;
    let stair_material = get_stair_block_for_material(config.roof_block);
    let glass = LIGHT_GRAY_STAINED_GLASS;
    let flank = config.roof_block;
    let cap_centre_block = config.roof_block;

    let overwrite_anything: &[Block] = &[];

    for (x, z, h) in chosen {
        let lower_y = h - 1 + abs;
        let face_y = h + abs;
        let cap_y = h + 1 + abs;

        // Outward perpendicular toward the closer eave so the extension hangs over it.
        let (dm_perp, dp_perp) = edge_scans.get(&(x, z)).copied().unwrap_or((1, 1));
        let outward_sign: i32 = if dm_perp <= dp_perp { -1 } else { 1 };
        let ox = perp_unit.0 * outward_sign;
        let oz = perp_unit.1 * outward_sign;

        let ext_x = x + ox;
        let ext_z = z + oz;
        // skip the extension when the outward cell leaves the footprint
        let ext_in = footprint.contains(&(ext_x, ext_z));

        // Lower base row: solid wall at the eave so the dormer reads as flush.
        editor.set_block_absolute(
            flank,
            x - px,
            lower_y,
            z - pz,
            None,
            Some(overwrite_anything),
        );
        editor.set_block_absolute(flank, x, lower_y, z, None, Some(overwrite_anything));
        editor.set_block_absolute(
            flank,
            x + px,
            lower_y,
            z + pz,
            None,
            Some(overwrite_anything),
        );
        if ext_in {
            editor.set_block_absolute(
                flank,
                ext_x - px,
                lower_y,
                ext_z - pz,
                None,
                Some(overwrite_anything),
            );
            editor.set_block_absolute(flank, ext_x, lower_y, ext_z, None, Some(overwrite_anything));
            editor.set_block_absolute(
                flank,
                ext_x + px,
                lower_y,
                ext_z + pz,
                None,
                Some(overwrite_anything),
            );
        }

        // Face row 1: wall+glass+wall, overwrites the slope stair.
        editor.set_block_absolute(
            flank,
            x - px,
            face_y,
            z - pz,
            None,
            Some(overwrite_anything),
        );
        editor.set_block_absolute(glass, x, face_y, z, None, Some(overwrite_anything));
        editor.set_block_absolute(
            flank,
            x + px,
            face_y,
            z + pz,
            None,
            Some(overwrite_anything),
        );

        // Face row 2: extends the dormer body one block outward over the eave.
        if ext_in {
            editor.set_block_absolute(
                flank,
                ext_x - px,
                face_y,
                ext_z - pz,
                None,
                Some(overwrite_anything),
            );
            editor.set_block_absolute(glass, ext_x, face_y, ext_z, None, Some(overwrite_anything));
            editor.set_block_absolute(
                flank,
                ext_x + px,
                face_y,
                ext_z + pz,
                None,
                Some(overwrite_anything),
            );
        }

        // Cap row 1: stair, full block, stair.
        editor.set_block_with_properties_absolute(
            create_stair_with_properties(stair_material, left_facing, StairShape::Straight),
            x - px,
            cap_y,
            z - pz,
            None,
            Some(overwrite_anything),
        );
        editor.set_block_absolute(
            cap_centre_block,
            x,
            cap_y,
            z,
            None,
            Some(overwrite_anything),
        );
        editor.set_block_with_properties_absolute(
            create_stair_with_properties(stair_material, right_facing, StairShape::Straight),
            x + px,
            cap_y,
            z + pz,
            None,
            Some(overwrite_anything),
        );

        // Cap row 2: mirrors row 1 over the extended face.
        if ext_in {
            editor.set_block_with_properties_absolute(
                create_stair_with_properties(stair_material, left_facing, StairShape::Straight),
                ext_x - px,
                cap_y,
                ext_z - pz,
                None,
                Some(overwrite_anything),
            );
            editor.set_block_absolute(
                cap_centre_block,
                ext_x,
                cap_y,
                ext_z,
                None,
                Some(overwrite_anything),
            );
            editor.set_block_with_properties_absolute(
                create_stair_with_properties(stair_material, right_facing, StairShape::Straight),
                ext_x + px,
                cap_y,
                ext_z + pz,
                None,
                Some(overwrite_anything),
            );
        }
    }
}

/// Places roof blocks for a given height map
fn place_roof_blocks_with_stairs(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    roof_heights: &HashMap<(i32, i32), i32>,
    config: &RoofConfig,
    stair_direction_fn: impl Fn(i32, i32, i32) -> BlockWithProperties,
    footprint: Option<&HashSet<(i32, i32)>>,
) {
    // Use empty blacklist to allow overwriting wall/ceiling blocks
    let replace_any: &[Block] = &[];

    for &(x, z) in floor_area {
        let roof_height = roof_heights[&(x, z)];

        for y in config.base_height..=roof_height {
            if y == roof_height {
                // Polygon-edge cells get stairs to continue the eave slope outward.
                let on_polygon_edge = footprint.is_some_and(|fp| {
                    [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)]
                        .iter()
                        .any(|n| !fp.contains(n))
                });
                let has_lower =
                    has_lower_neighbor(x, z, roof_height, roof_heights) || on_polygon_edge;
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
/// Variants of the two-slope roof family sharing the gabled scan.
#[derive(Copy, Clone, PartialEq)]
enum GableProfile {
    /// Classic gable, peak capped at 60% of the wall height.
    Standard,
    /// Steeper church-style gable (90% cap).
    Steep,
    /// Barn roof: 2:1 rise for the first two cells, then 1:1.
    Gambrel,
    /// Gable with hipped ends above half height.
    HalfHipped,
}

fn generate_gabled_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
    roof_orientation: Option<&str>,
    profile: GableProfile,
    preferred_ridge_along_x: Option<bool>,
    axis_snap: bool,
) {
    // Create a HashSet for O(1) footprint lookups, this is the actual building shape
    let footprint: HashSet<(i32, i32)> = floor_area.iter().copied().collect();

    let width_is_longer = config.width() >= config.length();
    let ridge_runs_along_x = match roof_orientation {
        Some(o) if o.eq_ignore_ascii_case("along") => width_is_longer,
        Some(o) if o.eq_ignore_ascii_case("across") => !width_is_longer,
        _ => preferred_ridge_along_x.unwrap_or(width_is_longer),
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

    // Hard cap: the roof peak should not exceed a fraction of the wall height.
    let cap_factor = if profile == GableProfile::Steep {
        0.9
    } else {
        0.6
    };
    let wall_cap = config.peak_cap.unwrap_or_else(|| {
        ((config.building_height as f64) * cap_factor)
            .round()
            .max(1.0) as i32
    });

    struct PosData {
        dist_to_edge: i32,
        local_half: i32,
        dm_along: i32,
        dp_along: i32,
        scan_perp_min: i32,
    }
    let mut pos_data: HashMap<(i32, i32), PosData> = HashMap::new();
    let mut max_perp_half: i32 = 0;

    for &(x, z) in floor_area {
        let sm_z = scan_dir(x, z, 0, -1);
        let sp_z = scan_dir(x, z, 0, 1);
        let sm_x = scan_dir(x, z, -1, 0);
        let sp_x = scan_dir(x, z, 1, 0);
        // snap mode: bbox distances drive the slope, scans keep the rim honest
        let (dm_z, dp_z, dm_x, dp_x) = if axis_snap {
            (
                z - config.min_z,
                config.max_z - z,
                x - config.min_x,
                config.max_x - x,
            )
        } else {
            (sm_z, sp_z, sm_x, sp_x)
        };

        let (dm_perp, dp_perp) = if ridge_runs_along_x {
            (dm_z, dp_z)
        } else {
            (dm_x, dp_x)
        };
        edge_scans.insert((x, z), (dm_perp, dp_perp));
        let (dm_along, dp_along) = if ridge_runs_along_x {
            (sm_x, sp_x)
        } else {
            (sm_z, sp_z)
        };
        let scan_perp_min = if ridge_runs_along_x {
            sm_z.min(sp_z)
        } else {
            sm_x.min(sp_x)
        };

        let dist_to_edge = dm_perp.min(dp_perp);

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
                dm_along,
                dp_along,
                scan_perp_min,
            },
        );
    }

    // Half-pitch when the capped flat ridge would be >= 4 blocks wide.
    let use_half_pitch = profile != GableProfile::Gambrel && max_perp_half - wall_cap >= 4;

    for &(x, z) in floor_area {
        let pd = &pos_data[&(x, z)];
        let slope_dist = if use_half_pitch {
            (pd.dist_to_edge + 1) / 2
        } else {
            pd.dist_to_edge
        };
        let boost = if profile == GableProfile::Gambrel {
            // 2 blocks per cell over the steep band, then 1 per cell.
            2 * slope_dist.min(2) + (slope_dist - 2).max(0)
        } else {
            slope_dist
        };
        let local_boost = ((pd.local_half as f64) * 0.85).round().max(1.0) as i32;
        let capped_boost = local_boost.min(wall_cap);
        let mut roof_height =
            (config.base_height + boost).min(config.base_height + capped_boost) + 1;
        // no-op for the polygon scan, feathers the snap tent at rotated corners
        roof_height = roof_height.min(config.base_height + pd.scan_perp_min + 1);
        if profile == GableProfile::HalfHipped {
            // The hip only bites above half the peak near the gable ends.
            let hip_start = (wall_cap / 2).max(2);
            let hip_limit = config.base_height + hip_start + pd.dm_along.min(pd.dp_along) + 1;
            roof_height = roof_height.min(hip_limit);
        }
        roof_heights.insert((x, z), roof_height);
    }

    // median along the ridge evens rasterization wobble, identity when aligned
    let smooth_along: (i32, i32) = if ridge_runs_along_x { (1, 0) } else { (0, 1) };
    let roof_heights: HashMap<(i32, i32), i32> = roof_heights
        .iter()
        .map(|(&(x, z), &h)| {
            let l = roof_heights.get(&(x - smooth_along.0, z - smooth_along.1));
            let r = roof_heights.get(&(x + smooth_along.0, z + smooth_along.1));
            let h = match (l, r) {
                (Some(&a), Some(&b)) => {
                    let mut t = [a, h, b];
                    t.sort_unstable();
                    t[1]
                }
                _ => h,
            };
            ((x, z), h)
        })
        .collect();

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

    // hip ends descend along the ridge, perp-only stairs would face sideways
    let along_descent_stair = |x: i32, z: i32, h: i32| -> Option<BlockWithProperties> {
        if profile != GableProfile::HalfHipped {
            return None;
        }
        let lower = |nx: i32, nz: i32| roof_heights.get(&(nx, nz)).is_some_and(|&nh| nh < h);
        let (am, ap, pm, pp) = if ridge_runs_along_x {
            (
                lower(x - 1, z),
                lower(x + 1, z),
                lower(x, z - 1),
                lower(x, z + 1),
            )
        } else {
            (
                lower(x, z - 1),
                lower(x, z + 1),
                lower(x - 1, z),
                lower(x + 1, z),
            )
        };
        if pm || pp || am == ap {
            return None;
        }
        let facing = match (ridge_runs_along_x, am) {
            (true, true) => StairFacing::East,
            (true, false) => StairFacing::West,
            (false, true) => StairFacing::South,
            (false, false) => StairFacing::North,
        };
        Some(create_stair_with_properties(
            stair_block_material,
            facing,
            StairShape::Straight,
        ))
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
            // Roof_block at base, stair at base+1.
            editor.set_block_absolute(
                config.roof_block,
                x,
                config.base_height + config.abs_terrain_offset,
                z,
                None,
                Some(replace_any),
            );
            editor.set_block_with_properties_absolute(
                get_outer_edge_stair(x, z),
                x,
                config.base_height + 1 + config.abs_terrain_offset,
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
                    let stair = along_descent_stair(x, z, roof_height)
                        .unwrap_or_else(|| get_slope_stair(x, z));
                    editor.set_block_with_properties_absolute(
                        stair,
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

    // Gable-end trim: the roof line continues one block past the gable face.
    if profile != GableProfile::HalfHipped {
        let along: (i32, i32) = if ridge_runs_along_x { (1, 0) } else { (0, 1) };
        for &(x, z) in floor_area {
            let pd = &pos_data[&(x, z)];
            let is_perp_edge = if ridge_runs_along_x {
                !footprint.contains(&(x, z - 1)) || !footprint.contains(&(x, z + 1))
            } else {
                !footprint.contains(&(x - 1, z)) || !footprint.contains(&(x + 1, z))
            };
            if is_perp_edge {
                continue;
            }
            let h = roof_heights[&(x, z)];
            // skip flat cap cells only, half-pitch treads pair equal heights
            let (pdx, pdz) = if ridge_runs_along_x { (0, 1) } else { (1, 0) };
            let flat_here = [(x - pdx, z - pdz), (x + pdx, z + pdz)]
                .iter()
                .all(|n| roof_heights.get(n).is_none_or(|&nh| nh == h));
            if flat_here {
                continue;
            }
            for (end_zero, sign) in [(pd.dm_along == 0, -1), (pd.dp_along == 0, 1)] {
                if !end_zero {
                    continue;
                }
                let tx = x + along.0 * sign;
                let tz = z + along.1 * sign;
                if footprint.contains(&(tx, tz)) {
                    continue;
                }
                editor.set_block_with_properties_absolute(
                    get_slope_stair(x, z),
                    tx,
                    h + config.abs_terrain_offset,
                    tz,
                    Some(&[AIR]),
                    None,
                );
            }
        }
    }

    let parallel_to_ridge = if ridge_runs_along_x { (1, 0) } else { (0, 1) };
    place_dormer_windows(
        editor,
        floor_area,
        &roof_heights,
        &edge_scans,
        config,
        parallel_to_ridge,
        &footprint,
        None,
    );

    // 2-block eave overhang on the slope sides (perpendicular to the ridge).
    place_eave_overhang(
        editor,
        floor_area,
        &footprint,
        config,
        stair_block_material,
        ridge_runs_along_x,
    );
}

// 2-block-deep eave overhang on slope sides only (gabled).
fn place_eave_overhang(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    footprint: &HashSet<(i32, i32)>,
    config: &RoofConfig,
    stair_block_material: Block,
    ridge_runs_along_x: bool,
) {
    place_eave_overhang_inner(
        editor,
        floor_area,
        footprint,
        config,
        stair_block_material,
        Some(ridge_runs_along_x),
    );
}

fn place_eave_overhang_all_sides(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    footprint: &HashSet<(i32, i32)>,
    config: &RoofConfig,
    stair_block_material: Block,
) {
    place_eave_overhang_inner(
        editor,
        floor_area,
        footprint,
        config,
        stair_block_material,
        None,
    );
}

fn place_eave_overhang_inner(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    footprint: &HashSet<(i32, i32)>,
    config: &RoofConfig,
    stair_block_material: Block,
    ridge_runs_along_x: Option<bool>,
) {
    let abs = config.abs_terrain_offset;
    let y_inner = config.base_height + abs;
    let y_outer = config.base_height - 1 + abs;

    let dirs: &[(i32, i32, StairFacing)] = match ridge_runs_along_x {
        Some(true) => &[(0, -1, StairFacing::South), (0, 1, StairFacing::North)],
        Some(false) => &[(-1, 0, StairFacing::East), (1, 0, StairFacing::West)],
        None => &[
            (-1, 0, StairFacing::East),
            (1, 0, StairFacing::West),
            (0, -1, StairFacing::South),
            (0, 1, StairFacing::North),
        ],
    };

    let mut inner_cells: HashMap<(i32, i32), StairFacing> = HashMap::new();
    let mut outer_cells: HashMap<(i32, i32), StairFacing> = HashMap::new();

    for &(x, z) in floor_area {
        for &(dx, dz, facing) in dirs {
            let n1 = (x + dx, z + dz);
            let n2 = (x + 2 * dx, z + 2 * dz);
            if footprint.contains(&n1) {
                continue;
            }
            inner_cells.entry(n1).or_insert(facing);
            if !footprint.contains(&n2) {
                outer_cells.entry(n2).or_insert(facing);
            }
        }
    }

    // Fill the four diagonal eave cells at each polygon corner (hipped only).
    let mut diag_inner: HashMap<(i32, i32), (StairFacing, StairShape)> = HashMap::new();
    let mut diag_outer: HashMap<(i32, i32), (StairFacing, StairShape)> = HashMap::new();
    let mut diag_l_arm: HashMap<(i32, i32), StairFacing> = HashMap::new();

    if ridge_runs_along_x.is_none() {
        // dx, dz, corner facing+shape, x-arm facing, z-arm facing.
        let corner_dirs: &[(i32, i32, StairFacing, StairShape, StairFacing, StairFacing)] = &[
            (
                -1,
                -1,
                StairFacing::East,
                StairShape::OuterRight,
                StairFacing::East,
                StairFacing::South,
            ),
            (
                1,
                -1,
                StairFacing::South,
                StairShape::OuterRight,
                StairFacing::West,
                StairFacing::South,
            ),
            (
                -1,
                1,
                StairFacing::East,
                StairShape::OuterLeft,
                StairFacing::East,
                StairFacing::North,
            ),
            (
                1,
                1,
                StairFacing::North,
                StairShape::OuterLeft,
                StairFacing::West,
                StairFacing::North,
            ),
        ];

        for &(x, z) in floor_area {
            for &(dx, dz, corner_facing, corner_shape, x_arm_facing, z_arm_facing) in corner_dirs {
                if footprint.contains(&(x + dx, z)) || footprint.contains(&(x, z + dz)) {
                    continue;
                }
                let inner_corner = (x + dx, z + dz);
                let outer_corner = (x + 2 * dx, z + 2 * dz);
                let l_arm_x = (x + 2 * dx, z + dz);
                let l_arm_z = (x + dx, z + 2 * dz);

                diag_inner
                    .entry(inner_corner)
                    .or_insert((corner_facing, corner_shape));
                diag_outer
                    .entry(outer_corner)
                    .or_insert((corner_facing, corner_shape));
                diag_l_arm.entry(l_arm_x).or_insert(x_arm_facing);
                diag_l_arm.entry(l_arm_z).or_insert(z_arm_facing);
            }
        }
    }

    for (cell, facing) in &inner_cells {
        let stair =
            create_stair_with_properties(stair_block_material, *facing, StairShape::Straight);
        editor.set_block_with_properties_absolute(stair, cell.0, y_inner, cell.1, None, None);
    }
    for (cell, (facing, shape)) in &diag_inner {
        if inner_cells.contains_key(cell) {
            continue;
        }
        let stair = create_stair_with_properties(stair_block_material, *facing, *shape);
        editor.set_block_with_properties_absolute(stair, cell.0, y_inner, cell.1, None, None);
    }
    // Outer ring skips cells already claimed by the inner or corner ring.
    for (cell, facing) in &outer_cells {
        if inner_cells.contains_key(cell) || diag_inner.contains_key(cell) {
            continue;
        }
        let stair =
            create_stair_with_properties(stair_block_material, *facing, StairShape::Straight);
        editor.set_block_with_properties_absolute(stair, cell.0, y_outer, cell.1, None, None);
    }
    for (cell, (facing, shape)) in &diag_outer {
        if inner_cells.contains_key(cell)
            || outer_cells.contains_key(cell)
            || diag_inner.contains_key(cell)
        {
            continue;
        }
        let stair = create_stair_with_properties(stair_block_material, *facing, *shape);
        editor.set_block_with_properties_absolute(stair, cell.0, y_outer, cell.1, None, None);
    }
    for (cell, facing) in &diag_l_arm {
        if inner_cells.contains_key(cell)
            || outer_cells.contains_key(cell)
            || diag_inner.contains_key(cell)
            || diag_outer.contains_key(cell)
        {
            continue;
        }
        let stair =
            create_stair_with_properties(stair_block_material, *facing, StairShape::Straight);
        editor.set_block_with_properties_absolute(stair, cell.0, y_outer, cell.1, None, None);
    }
}

/// Hipped roof via polygon-edge scanning, capped at 60% of wall height.
fn generate_hipped_roof(editor: &mut WorldEditor, floor_area: &[(i32, i32)], config: &RoofConfig) {
    generate_hipped_roof_inner(editor, floor_area, config, true, None);
}

/// Mansard roof: the hipped scan with a piecewise profile.
fn generate_mansard_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
    steep_h: i32,
) {
    generate_hipped_roof_inner(editor, floor_area, config, true, Some(steep_h));
}

/// Height boost of a mansard profile at `dist_to_edge`: a steep 2-cell band
/// rising to `steep_h`, then a shallow 1:2 slope, capped at `cap`.
fn mansard_boost(dist_to_edge: i32, steep_h: i32, cap: i32) -> i32 {
    const STEEP_RUN: i32 = 2;
    let steep = (dist_to_edge.min(STEEP_RUN) * steep_h + STEEP_RUN - 1) / STEEP_RUN;
    let shallow = (dist_to_edge - STEEP_RUN).max(0) / 2;
    (steep + shallow).min(cap)
}

fn generate_hipped_roof_inner(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
    with_overhang: bool,
    mansard_steep_h: Option<i32>,
) {
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

    let wall_cap = config
        .peak_cap
        .unwrap_or_else(|| ((config.building_height as f64) * 0.6).round().max(1.0) as i32);

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
    let mut max_full_span: i32 = 0;
    // Perpendicular-to-ridge scans for dormer placement (ridge = longer axis).
    let ridge_runs_along_x = config.width() >= config.length();
    let mut edge_scans: HashMap<(i32, i32), (i32, i32)> = HashMap::new();

    for &(x, z) in floor_area {
        let dm_x = scan_dir(x, z, -1, 0);
        let dp_x = scan_dir(x, z, 1, 0);
        let dm_z = scan_dir(x, z, 0, -1);
        let dp_z = scan_dir(x, z, 0, 1);
        edge_scans.insert(
            (x, z),
            if ridge_runs_along_x {
                (dm_z, dp_z)
            } else {
                (dm_x, dp_x)
            },
        );

        let dists = [dm_x, dp_x, dm_z, dp_z];
        let dist_to_edge = *dists.iter().min().unwrap();

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
        if full_span > max_full_span {
            max_full_span = full_span;
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

    // Half-pitch when the capped flat peak would be >= 4 blocks wide.
    let use_half_pitch = max_full_span - wall_cap >= 4;

    let mut roof_heights: HashMap<(i32, i32), i32> = HashMap::new();

    let lift = if with_overhang { 1 } else { 0 };

    for &(x, z) in floor_area {
        let pd = &pos_data[&(x, z)];
        let roof_height = if let Some(steep_h) = mansard_steep_h {
            // Mansard: piecewise profile with its own cap; the local wing
            // width still caps narrow wings so they don't overshoot.
            let cap = wall_cap.max(steep_h + 2).min(config.building_height.max(3));
            let local_cap = ((pd.local_half as f64) * 0.9).round().max(1.0) as i32 + steep_h;
            config.base_height + mansard_boost(pd.dist_to_edge, steep_h, cap.min(local_cap)) + lift
        } else {
            let slope_dist = if use_half_pitch {
                (pd.dist_to_edge + 1) / 2
            } else {
                pd.dist_to_edge
            };
            let local_boost = ((pd.local_half as f64) * 0.85).round().max(1.0) as i32;
            let capped_boost = local_boost.min(wall_cap);
            (config.base_height + slope_dist).min(config.base_height + capped_boost) + lift
        };
        roof_heights.insert((x, z), roof_height);
    }

    // --- Place blocks with stair facing toward nearest polygon edge ---
    let stair_block_material = get_stair_block_for_material(config.roof_block);

    place_roof_blocks_with_stairs(
        editor,
        floor_area,
        &roof_heights,
        config,
        |x, z, h| {
            let north_h = roof_heights
                .get(&(x, z - 1))
                .copied()
                .unwrap_or(config.base_height);
            let south_h = roof_heights
                .get(&(x, z + 1))
                .copied()
                .unwrap_or(config.base_height);
            let west_h = roof_heights
                .get(&(x - 1, z))
                .copied()
                .unwrap_or(config.base_height);
            let east_h = roof_heights
                .get(&(x + 1, z))
                .copied()
                .unwrap_or(config.base_height);
            let lower_n = north_h < h;
            let lower_s = south_h < h;
            let lower_w = west_h < h;
            let lower_e = east_h < h;
            let lower_count =
                (lower_n as i32) + (lower_s as i32) + (lower_w as i32) + (lower_e as i32);

            // Outer corner - exactly 2 perpendicular sides lower.
            if lower_count == 2 {
                if lower_n && lower_w {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::East,
                        StairShape::OuterRight,
                    );
                }
                if lower_n && lower_e {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::South,
                        StairShape::OuterRight,
                    );
                }
                if lower_s && lower_w {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::East,
                        StairShape::OuterLeft,
                    );
                }
                if lower_s && lower_e {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::North,
                        StairShape::OuterLeft,
                    );
                }
                // 2-opposite (N+S or W+E): on a "ridge" - fall through.
            }

            // Single-sided slope (exactly one lower).
            if lower_count == 1 {
                if lower_n {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::South,
                        StairShape::Straight,
                    );
                }
                if lower_s {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::North,
                        StairShape::Straight,
                    );
                }
                if lower_w {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::East,
                        StairShape::Straight,
                    );
                }
                if lower_e {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::West,
                        StairShape::Straight,
                    );
                }
            }

            // Diamond tip: face the only higher direction.
            if lower_count == 3 {
                if !lower_n {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::North,
                        StairShape::Straight,
                    );
                }
                if !lower_s {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::South,
                        StairShape::Straight,
                    );
                }
                if !lower_w {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::West,
                        StairShape::Straight,
                    );
                }
                if !lower_e {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::East,
                        StairShape::Straight,
                    );
                }
            }

            // Plateau or apex: fall back to the closest-edge heuristic.
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
        },
        Some(&footprint),
    );

    if with_overhang {
        place_eave_overhang_all_sides(editor, floor_area, &footprint, config, stair_block_material);
    }

    let parallel_to_ridge = if ridge_runs_along_x { (1, 0) } else { (0, 1) };
    let mansard_band = mansard_steep_h.map(|sh| config.base_height + (sh + 1) / 2 + lift);
    place_dormer_windows(
        editor,
        floor_area,
        &roof_heights,
        &edge_scans,
        config,
        parallel_to_ridge,
        &footprint,
        mansard_band,
    );
}

/// Parses `roof:direction` (compass point or degrees) to the nearest cardinal.
fn parse_roof_direction(value: &str) -> Option<StairFacing> {
    let deg = match value.trim().to_ascii_lowercase().as_str() {
        "n" | "north" => 0.0,
        "ne" => 45.0,
        "e" | "east" => 90.0,
        "se" => 135.0,
        "s" | "south" => 180.0,
        "sw" => 225.0,
        "w" | "west" => 270.0,
        "nw" => 315.0,
        other => other.parse::<f64>().ok()?,
    };
    if !deg.is_finite() {
        return None;
    }
    Some(match ((deg / 90.0).round() as i64).rem_euclid(4) {
        0 => StairFacing::North,
        1 => StairFacing::East,
        2 => StairFacing::South,
        _ => StairFacing::West,
    })
}

/// Skillion (mono-pitch) roof descending toward `roof:direction`, or across the shorter axis.
fn generate_skillion_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
    roof_direction: Option<&str>,
) {
    let downhill = roof_direction
        .and_then(parse_roof_direction)
        .unwrap_or_else(|| {
            let mut rng = element_rng(config.element_id_for_decor ^ 0x5C11_1104_D129_0000);
            let flip = rng.random_bool(0.5);
            if config.width() <= config.length() {
                if flip {
                    StairFacing::West
                } else {
                    StairFacing::East
                }
            } else if flip {
                StairFacing::North
            } else {
                StairFacing::South
            }
        });

    let width = config.width().max(1);
    let length = config.length().max(1);

    // rise follows the slope run and stays below the wall height
    let run = match downhill {
        StairFacing::West | StairFacing::East => width,
        StairFacing::North | StairFacing::South => length,
    };
    let max_roof_height = config.peak_cap.map_or_else(
        || {
            (run / 3)
                .clamp(2, 10)
                .min(((config.building_height as f64) * 0.9).round().max(1.0) as i32)
        },
        |p| p.clamp(1, 12),
    );

    // Stairs face uphill, opposite the downhill direction.
    let stair_facing = match downhill {
        StairFacing::West => StairFacing::East,
        StairFacing::East => StairFacing::West,
        StairFacing::North => StairFacing::South,
        StairFacing::South => StairFacing::North,
    };

    let mut roof_heights = HashMap::new();
    for &(x, z) in floor_area {
        let slope_progress = match downhill {
            StairFacing::West => (x - config.min_x) as f64 / width as f64,
            StairFacing::East => (config.max_x - x) as f64 / width as f64,
            StairFacing::North => (z - config.min_z) as f64 / length as f64,
            StairFacing::South => (config.max_z - z) as f64 / length as f64,
        };
        let roof_height = config.base_height + (slope_progress * max_roof_height as f64) as i32;
        roof_heights.insert((x, z), roof_height);
    }

    let stair_block_material = get_stair_block_for_material(config.roof_block);

    place_roof_blocks_with_stairs(
        editor,
        floor_area,
        &roof_heights,
        config,
        |_, _, _| {
            create_stair_with_properties(stair_block_material, stair_facing, StairShape::Straight)
        },
        None,
    );
}

/// Pyramidal roof: tapers to a single apex via Chebyshev distance from centre.
fn generate_pyramidal_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
) {
    let footprint: HashSet<(i32, i32)> = floor_area.iter().copied().collect();
    let shorter_half = config.width().min(config.length()) / 2;
    let uncapped_boost = ((shorter_half as f64) * 0.85).round().max(1.0) as i32;
    let wall_cap = config
        .peak_cap
        .unwrap_or_else(|| ((config.building_height as f64) * 0.6).round().max(1.0) as i32);
    let peak_boost = uncapped_boost.min(wall_cap);
    let max_distance = (config.width() / 2).max(config.length() / 2).max(1) as f64;

    let mut roof_heights: HashMap<(i32, i32), i32> = HashMap::new();
    for &(x, z) in floor_area {
        let dx = (x - config.center_x).abs() as f64;
        let dz = (z - config.center_z).abs() as f64;
        let distance_to_apex = dx.max(dz);
        let height_factor = (1.0 - distance_to_apex / max_distance).max(0.0);
        let roof_height = config.base_height + (height_factor * peak_boost as f64) as i32;
        roof_heights.insert((x, z), roof_height);
    }

    let stair_block_material = get_stair_block_for_material(config.roof_block);

    place_roof_blocks_with_stairs(
        editor,
        floor_area,
        &roof_heights,
        config,
        |x, z, h| {
            let north_h = roof_heights
                .get(&(x, z - 1))
                .copied()
                .unwrap_or(config.base_height);
            let south_h = roof_heights
                .get(&(x, z + 1))
                .copied()
                .unwrap_or(config.base_height);
            let west_h = roof_heights
                .get(&(x - 1, z))
                .copied()
                .unwrap_or(config.base_height);
            let east_h = roof_heights
                .get(&(x + 1, z))
                .copied()
                .unwrap_or(config.base_height);
            let lower_n = north_h < h;
            let lower_s = south_h < h;
            let lower_w = west_h < h;
            let lower_e = east_h < h;
            let lower_count =
                (lower_n as i32) + (lower_s as i32) + (lower_w as i32) + (lower_e as i32);

            if lower_count == 2 {
                if lower_n && lower_w {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::East,
                        StairShape::OuterRight,
                    );
                }
                if lower_n && lower_e {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::South,
                        StairShape::OuterRight,
                    );
                }
                if lower_s && lower_w {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::East,
                        StairShape::OuterLeft,
                    );
                }
                if lower_s && lower_e {
                    return create_stair_with_properties(
                        stair_block_material,
                        StairFacing::North,
                        StairShape::OuterLeft,
                    );
                }
            }

            if lower_count == 0 {
                // Plateau ring (common on truncated slopes): face the apex.
                let dx = x - config.center_x;
                let dz = z - config.center_z;
                let facing = if dx.abs() >= dz.abs() {
                    if dx > 0 {
                        StairFacing::West
                    } else {
                        StairFacing::East
                    }
                } else if dz > 0 {
                    StairFacing::North
                } else {
                    StairFacing::South
                };
                return create_stair_with_properties(
                    stair_block_material,
                    facing,
                    StairShape::Straight,
                );
            }

            if lower_n {
                create_stair_with_properties(
                    stair_block_material,
                    StairFacing::South,
                    StairShape::Straight,
                )
            } else if lower_s {
                create_stair_with_properties(
                    stair_block_material,
                    StairFacing::North,
                    StairShape::Straight,
                )
            } else if lower_w {
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
        },
        Some(&footprint),
    );
}

/// Generates a dome roof
fn generate_dome_roof(editor: &mut WorldEditor, floor_area: &[(i32, i32)], config: &RoofConfig) {
    // elliptical normalization so the shell meets the eave on every wall
    let half_w = (config.width() as f64 / 2.0).max(1.0);
    let half_l = (config.length() as f64 / 2.0).max(1.0);
    let rise = (half_w.min(half_l) * 0.8).max(1.0);
    // Use empty blacklist to allow overwriting wall/ceiling blocks
    let replace_any: &[Block] = &[];

    for &(x, z) in floor_area {
        let nx = (x - config.center_x) as f64 / half_w;
        let nz = (z - config.center_z) as f64 / half_l;
        let normalized_distance = (nx * nx + nz * nz).sqrt().min(1.0);

        let height_factor = (1.0 - normalized_distance * normalized_distance).sqrt();
        let surface_height = config.base_height + (height_factor * rise) as i32;

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

/// Conical roof: circular base tapering linearly to a point.
fn generate_cone_roof(editor: &mut WorldEditor, floor_area: &[(i32, i32)], config: &RoofConfig) {
    let half_w = (config.width() as f64 / 2.0).max(1.0);
    let half_l = (config.length() as f64 / 2.0).max(1.0);
    let replace_any: &[Block] = &[];

    let peak_height = ((half_w.min(half_l) * 1.2) as i32)
        .max(2)
        .min(config.building_height * 2);

    for &(x, z) in floor_area {
        let dx = (x - config.center_x) as f64;
        let dz = (z - config.center_z) as f64;
        let normalized = ((dx / half_w).powi(2) + (dz / half_l).powi(2))
            .sqrt()
            .min(1.0);

        let surface_height = config.base_height + ((1.0 - normalized) * peak_height as f64) as i32;

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

/// Onion roof: bulbous Russian-Orthodox / Bavarian profile.
fn generate_onion_roof(editor: &mut WorldEditor, floor_area: &[(i32, i32)], config: &RoofConfig) {
    // shorter axis, the bulb bulges a little instead of half the long axis
    let base_radius = (config.width().min(config.length()) / 2) as f64;
    let replace_any: &[Block] = &[];

    let total_height = ((base_radius * 1.8) as i32)
        .max(6)
        .min(config.building_height * 2);

    let footprint: HashSet<(i32, i32)> = floor_area.iter().copied().collect();

    // Bulb extends up to 1.25x the base radius.
    let max_search = (base_radius * 1.25).ceil() as i32 + 1;

    for layer in 0..=total_height {
        let t = layer as f64 / total_height as f64;
        let radius_factor = onion_profile_radius(t);
        let layer_radius = base_radius * radius_factor;
        let abs_y = config.base_height + layer + config.abs_terrain_offset;

        // Apex: collapse to a single block to give the spire a clean point.
        if layer_radius < 0.6 && t > 0.85 {
            editor.set_block_absolute(
                config.roof_block,
                config.center_x,
                abs_y,
                config.center_z,
                None,
                Some(replace_any),
            );
            continue;
        }

        // Base plate: full footprint regardless of radius.
        if t < 0.05 {
            for &(x, z) in floor_area {
                editor.set_block_absolute(config.roof_block, x, abs_y, z, None, Some(replace_any));
            }
            continue;
        }

        let r2 = layer_radius * layer_radius;
        for dz in -max_search..=max_search {
            for dx in -max_search..=max_search {
                if (dx as f64).powi(2) + (dz as f64).powi(2) > r2 {
                    continue;
                }
                let x = config.center_x + dx;
                let z = config.center_z + dz;
                // Drum: clip to footprint. Above the drum the bulb may bulge out.
                if t < 0.15 && !footprint.contains(&(x, z)) {
                    continue;
                }
                editor.set_block_absolute(config.roof_block, x, abs_y, z, None, Some(replace_any));
            }
        }
    }
}

/// Onion-roof radius factor at relative height `t ∈ [0, 1]`.
#[inline]
fn onion_profile_radius(t: f64) -> f64 {
    if t < 0.05 {
        // Base plate - full coverage.
        1.0
    } else if t < 0.15 {
        // Drum: taper 1.00 → 0.55.
        let local = (t - 0.05) / 0.10;
        1.0 - local * 0.45
    } else if t < 0.55 {
        // Bulb: smooth sin-curve swell 0.55 → 1.20 → 0.55, peak at t≈0.35.
        let local = (t - 0.15) / 0.40;
        let peak = (local * std::f64::consts::PI).sin();
        0.55 + peak * 0.65
    } else if t < 0.78 {
        // Neck: pinch from 0.55 to 0.18 - creates the onion waist.
        let local = (t - 0.55) / 0.23;
        0.55 - local * 0.37
    } else {
        // Spire: 0.18 down to 0 across the top fifth of the height.
        let local = (t - 0.78) / 0.22;
        (0.18 * (1.0 - local)).max(0.0)
    }
}

/// Unified function to generate various roof types
#[inline]
#[allow(clippy::too_many_arguments)]
fn generate_roof(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    start_y_offset: i32,
    building_height: i32,
    floor_block: Block,
    wall_block: Block,
    roof_block_override: Option<Block>,
    roof_type: RoofType,
    roof_area: &[(i32, i32)],
    abs_terrain_offset: i32,
    add_dormers: bool,
    style_seed: u64,
    steep_gable: bool,
    preferred_ridge_along_x: Option<bool>,
    scale_factor: f64,
) {
    if roof_area.is_empty() {
        return;
    }

    let mut config = RoofConfig::from_roof_area(
        roof_area,
        style_seed,
        start_y_offset,
        building_height,
        wall_block,
        abs_terrain_offset,
    );

    // OSM roof:material / roof:colour override the preset.
    let mut roof_rng = element_rng(style_seed ^ 0xF00F_C010_BA5E_F00D);
    let osm_roof_block = element
        .tags
        .get("roof:material")
        .and_then(|m| get_roof_block_for_material(m, &mut roof_rng))
        .or_else(|| {
            element
                .tags
                .get("roof:colour")
                .and_then(|c| color_text_to_rgb_tuple(c))
                .map(|rgb| crate::block_palette::roof_block_for_color(rgb, &mut roof_rng))
        });

    if let Some(block) = osm_roof_block {
        config.roof_block = block;
    } else if let Some(override_block) = roof_block_override {
        config.roof_block = override_block;
    }

    config.add_dormers = add_dormers;
    // A mapped roof:height overrides the heuristic rise caps.
    config.peak_cap = element
        .tags
        .get("roof:height")
        .and_then(|v| v.trim_end_matches('m').trim().parse::<f64>().ok())
        .filter(|m| *m > 0.0)
        .map(|m| multiply_scale(m.round() as i32, scale_factor).max(1));

    let roof_orientation = element.tags.get("roof:orientation").map(|s| s.as_str());
    let axis_snap = matches!(
        roof_type,
        RoofType::Gabled | RoofType::Gambrel | RoofType::HalfHipped
    ) && gable_axis_snap(&element.nodes);

    // For flat roofs: OSM tags override > preset override > floor block default.
    let flat_roof_block = osm_roof_block
        .or(roof_block_override)
        .unwrap_or(floor_block);

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
            let profile = if steep_gable {
                GableProfile::Steep
            } else {
                GableProfile::Standard
            };
            generate_gabled_roof(
                editor,
                roof_area,
                &config,
                roof_orientation,
                profile,
                preferred_ridge_along_x,
                axis_snap,
            );
        }

        RoofType::Gambrel => {
            generate_gabled_roof(
                editor,
                roof_area,
                &config,
                roof_orientation,
                GableProfile::Gambrel,
                preferred_ridge_along_x,
                axis_snap,
            );
        }

        RoofType::HalfHipped => {
            generate_gabled_roof(
                editor,
                roof_area,
                &config,
                roof_orientation,
                GableProfile::HalfHipped,
                preferred_ridge_along_x,
                axis_snap,
            );
        }

        RoofType::Hipped => {
            generate_hipped_roof(editor, roof_area, &config);
        }

        RoofType::Mansard => {
            // roof:levels hints how tall the steep band is (3 blocks per level).
            let steep_h = element
                .tags
                .get("roof:levels")
                .and_then(|v| v.trim().parse::<f64>().ok())
                .map(|l| ((l * 3.0).round() as i32).clamp(3, 6))
                .unwrap_or(4);
            generate_mansard_roof(editor, roof_area, &config, steep_h);
        }

        RoofType::Skillion => {
            let roof_direction = element.tags.get("roof:direction").map(|s| s.as_str());
            generate_skillion_roof(editor, roof_area, &config, roof_direction);
        }

        RoofType::Pyramidal => {
            generate_pyramidal_roof(editor, roof_area, &config);
        }

        RoofType::Dome => {
            generate_dome_roof(editor, roof_area, &config);
        }

        RoofType::Cone => {
            generate_cone_roof(editor, roof_area, &config);
        }

        RoofType::Onion => {
            generate_onion_roof(editor, roof_area, &config);
        }
    }
}

pub fn generate_building_from_relation(
    editor: &mut WorldEditor,
    relation: &ProcessedRelation,
    args: &Args,
    ctx: &BuildingContext<'_>,
    xzbbox: &crate::coordinate_system::cartesian::XZBBox,
) {
    // Skip underground buildings/building parts
    if is_underground_building(&relation.tags) {
        return;
    }

    // Landmark: a Starship parked on the Starbase Pad 2 launch mount.
    for member in &relation.members {
        if member.way.id == crate::structures::starship::STARBASE_PAD2_INNER_RING_WAY
            && member.role == ProcessedMemberRole::Inner
        {
            crate::structures::starship::place_on_launch_mount(editor, &member.way);
        }
    }

    // Extract levels from relation tags. Untagged relations get no fixed
    // default; the synthetic outline ways carry the relation tags, so the
    // per-type height inference applies to them like to any way.
    let relation_levels = relation
        .tags
        .get("building:levels")
        .and_then(|l: &String| l.trim().parse::<f64>().ok())
        .map(|l| l.round() as i32);

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
        // Closed building:part outer rings render standalone with their own
        // height tags; rendering the relation too would stack a box on top
        let mut outer_iter = relation
            .members
            .iter()
            .filter(|m| m.role == ProcessedMemberRole::Outer)
            .peekable();
        if outer_iter.peek().is_some()
            && outer_iter.all(|m| {
                m.way
                    .tags
                    .get("building:part")
                    .is_some_and(|v| !v.eq_ignore_ascii_case("no"))
                    && m.way.nodes.len() >= 4
                    && m.way.nodes.first().map(|n| n.id) == m.way.nodes.last().map(|n| n.id)
            })
        {
            return;
        }

        // Collect outer member node lists and merge open segments into closed rings.
        // Multipolygon relations commonly split the outline across many short way
        // segments that share endpoints. Without merging, each segment is processed
        // individually, producing degenerate polygons and empty flood fills (only
        // wall outlines, no filled floors/ceilings/roofs).
        let mut outer_rings: Vec<Vec<ProcessedNode>> = relation
            .members
            .iter()
            .filter(|m| m.role == ProcessedMemberRole::Outer && !SKIP_WAY_IDS.contains(&m.way.id))
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
                relation_levels,
                hole_polygons.as_deref(),
                ctx,
                merged_way.id,
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

    // Calculate bridge level offset based on the "level" tag, layer as fallback
    let bridge_y_offset = if let Some(level) = element
        .tags
        .get("level")
        .and_then(|s| s.parse::<i32>().ok())
    {
        (level * 3) + 1
    } else if let Some(layer) = element
        .tags
        .get("layer")
        .and_then(|s| s.parse::<i32>().ok())
        .filter(|l| *l > 0)
    {
        layer * 4 + 2
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

#[cfg(test)]
mod height_tests {
    use super::*;

    fn ring(points: &[(i32, i32)]) -> Vec<ProcessedNode> {
        let mut nodes: Vec<ProcessedNode> = points
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: i as u64 + 1,
                tags: HashMap::new(),
                x,
                z,
            })
            .collect();
        nodes.push(nodes[0].clone());
        nodes
    }

    fn rotated_rect(w: f64, l: f64, deg: f64) -> Vec<ProcessedNode> {
        let (c, sn) = (deg.to_radians().cos(), deg.to_radians().sin());
        let pts: Vec<(i32, i32)> = [(0.0, 0.0), (l, 0.0), (l, w), (0.0, w)]
            .iter()
            .map(|&(u, v)| {
                (
                    (u * c - v * sn).round() as i32,
                    (u * sn + v * c).round() as i32,
                )
            })
            .collect();
        ring(&pts)
    }

    #[test]
    fn axis_snap_catches_slight_rotation() {
        assert!(gable_axis_snap(&rotated_rect(12.0, 30.0, 8.0)));
        assert!(gable_axis_snap(&rotated_rect(10.0, 24.0, 4.0)));
    }

    #[test]
    fn axis_snap_leaves_aligned_and_steep_alone() {
        assert!(!gable_axis_snap(&rotated_rect(12.0, 30.0, 0.0)));
        assert!(!gable_axis_snap(&rotated_rect(12.0, 30.0, 20.0)));
        assert!(!gable_axis_snap(&rotated_rect(12.0, 30.0, 44.0)));
    }

    #[test]
    fn axis_snap_skips_l_shapes() {
        // L-shape rotated ~8 deg: rectangle-likeness gate must reject it.
        let (c, sn) = (8.0f64.to_radians().cos(), 8.0f64.to_radians().sin());
        let pts: Vec<(i32, i32)> = [
            (0.0, 0.0),
            (30.0, 0.0),
            (30.0, 10.0),
            (12.0, 10.0),
            (12.0, 26.0),
            (0.0, 26.0),
        ]
        .iter()
        .map(|&(u, v): &(f64, f64)| {
            (
                (u * c - v * sn).round() as i32,
                (u * sn + v * c).round() as i32,
            )
        })
        .collect();
        assert!(!gable_axis_snap(&ring(&pts)));
    }

    fn way_with_tags(tags: &[(&str, &str)]) -> ProcessedWay {
        ProcessedWay {
            id: 1,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            nodes: Vec::new(),
        }
    }

    // height=89 min_height=60 levels=19: the 29m height span wins over the levels estimate
    #[test]
    fn explicit_height_beats_relation_levels() {
        let way = way_with_tags(&[
            ("height", "89"),
            ("min_height", "60"),
            ("building:levels", "19"),
        ]);
        let (h, _) = calculate_building_height(&way, "yes", 0, 1.0, Some(19), 4, 100, 1);
        assert_eq!(h, 29);
    }

    #[test]
    fn relation_levels_apply_without_height_tag() {
        let way = way_with_tags(&[]);
        let (h, _) = calculate_building_height(&way, "yes", 0, 1.0, Some(5), 3, 100, 1);
        assert_eq!(h, 17); // 5 levels * 3 + 2
    }

    // height spans walls plus roof; roof:height comes off the wall span
    #[test]
    fn roof_height_reduces_wall_span() {
        let way = way_with_tags(&[
            ("height", "12"),
            ("roof:height", "4"),
            ("roof:shape", "gabled"),
        ]);
        let (h, _) = calculate_building_height(&way, "house", 0, 1.0, None, 3, 100, 1);
        assert_eq!(h, 8);
        // flat roofs keep the full span
        let way = way_with_tags(&[
            ("height", "12"),
            ("roof:height", "4"),
            ("roof:shape", "flat"),
        ]);
        let (h, _) = calculate_building_height(&way, "house", 0, 1.0, None, 3, 100, 1);
        assert_eq!(h, 12);
    }

    // A 5cm roof plate stays a thin slab instead of a 3-block band
    #[test]
    fn elevated_thin_part_is_not_fattened() {
        let way = way_with_tags(&[("height", "19.05"), ("min_height", "19")]);
        let (h, _) = calculate_building_height(&way, "yes", 0, 1.0, None, 4, 100, 1);
        assert_eq!(h, 1);
    }

    // height=96 min_height=94 spans exactly 2 blocks
    #[test]
    fn elevated_part_keeps_exact_span() {
        let way = way_with_tags(&[("height", "96"), ("min_height", "94")]);
        let (h, _) = calculate_building_height(&way, "yes", 0, 1.0, None, 4, 100, 1);
        assert_eq!(h, 2);
    }

    // Ground-level buildings keep the 3-block interior minimum
    #[test]
    fn ground_level_building_keeps_minimum() {
        let way = way_with_tags(&[("height", "2")]);
        let (h, _) = calculate_building_height(&way, "yes", 0, 1.0, None, 4, 100, 1);
        assert_eq!(h, 3);
    }

    // min_height=0 is not an elevated part, the minimum still applies
    #[test]
    fn zero_min_height_is_ground_level() {
        let way = way_with_tags(&[("height", "2"), ("min_height", "0")]);
        let (h, _) = calculate_building_height(&way, "yes", 0, 1.0, None, 4, 100, 1);
        assert_eq!(h, 3);
    }

    #[test]
    fn fractional_levels_parse() {
        let way = way_with_tags(&[("building:levels", "2.5")]);
        let (h, _) = calculate_building_height(&way, "yes", 0, 1.0, None, 3, 100, 1);
        assert_eq!(h, 9); // 2.5 levels * 3 + 2
    }

    // The +2 moved into the min_level offset, wall span is exactly 4 per level
    #[test]
    fn min_level_walls_span_remaining_levels() {
        let way = way_with_tags(&[("building:levels", "4"), ("building:min_level", "2")]);
        let (h, _) = calculate_building_height(&way, "yes", 2, 1.0, None, 3, 100, 1);
        assert_eq!(h, 6);
    }

    // layer=-1 is treated as underground unless an explicit surface/overground/roof location is set
    #[test]
    fn surface_location_overrides_negative_layer() {
        let way = way_with_tags(&[("building", "office"), ("layer", "-1")]);
        assert!(is_underground_building(&way.tags));
        let way = way_with_tags(&[
            ("building", "office"),
            ("layer", "-1"),
            ("location", "surface"),
        ]);
        assert!(!is_underground_building(&way.tags));
    }

    // height with building:min_level but no min_height: the level-based lift
    // comes off the wall span, matching the offset the part is raised by
    #[test]
    fn height_minus_min_level_offset() {
        let way = way_with_tags(&[
            ("height", "30"),
            ("building:levels", "9"),
            ("building:min_level", "5"),
        ]);
        let (h, _) = calculate_building_height(&way, "yes", 5, 1.0, None, 3, 100, 1);
        // offset = 5*3+2 = 17; wall span = 30-17 = 13
        assert_eq!(h, 13);
    }

    // Sibling parts must infer identical heights regardless of footprint
    #[test]
    fn part_inference_ignores_footprint() {
        let way = way_with_tags(&[("building:part", "yes")]);
        for seed in 0..20u64 {
            let (small, _) = calculate_building_height(&way, "yes", 0, 1.0, None, 4, 30, seed);
            let (large, _) = calculate_building_height(&way, "yes", 0, 1.0, None, 4, 900, seed);
            assert_eq!(small, large, "seed {seed}");
        }
    }

    // Inference: same seed → same height, and values stay inside the type table
    #[test]
    fn inferred_house_heights_are_deterministic_and_in_range() {
        for seed in 0..40u64 {
            let way = way_with_tags(&[("building", "house")]);
            let (h1, tall1) = calculate_building_height(&way, "house", 0, 1.0, None, 3, 100, seed);
            let (h2, _) = calculate_building_height(&way, "house", 0, 1.0, None, 3, 100, seed);
            assert_eq!(h1, h2);
            assert!(!tall1);
            // 1-3 storeys → 5/8/11 blocks at the 3-block cycle
            assert!(matches!(h1, 5 | 8 | 11), "unexpected house height {h1}");
        }
    }

    #[test]
    fn inferred_garage_is_a_low_hall() {
        let way = way_with_tags(&[("building", "garage")]);
        let (h, tall) = calculate_building_height(&way, "garage", 0, 1.0, None, 4, 40, 7);
        assert_eq!(h, 3);
        assert!(!tall);
    }

    // A garage with explicit levels honours the mapper, not the type default
    #[test]
    fn tags_override_inference() {
        let way = way_with_tags(&[("building", "garage"), ("building:levels", "3")]);
        let (h, _) = calculate_building_height(&way, "garage", 0, 1.0, None, 3, 40, 7);
        assert_eq!(h, 11);
    }

    #[test]
    fn generic_yes_scales_with_footprint() {
        let way = way_with_tags(&[("building", "yes")]);
        for seed in 0..40u64 {
            let (small, _) = calculate_building_height(&way, "yes", 0, 1.0, None, 3, 30, seed);
            assert!(matches!(small, 3 | 5), "tiny yes-building got {small}");
            let (large, _) = calculate_building_height(&way, "yes", 0, 1.0, None, 3, 900, seed);
            assert!(
                matches!(large, 7 | 11 | 14),
                "large yes-building got {large}"
            );
        }
    }

    #[test]
    fn inferred_apartments_reach_midrise() {
        let way = way_with_tags(&[("building", "apartments")]);
        for seed in 0..40u64 {
            let (h, _) = calculate_building_height(&way, "apartments", 0, 1.0, None, 3, 400, seed);
            assert!(matches!(h, 11 | 14 | 17 | 20), "apartments got {h}");
        }
    }
}

#[cfg(test)]
mod style_tests {
    use super::*;

    #[test]
    fn archetype_truth_table() {
        use WindowArchetype::*;
        // Standard3: cols 0-2 on all open rows
        assert!(archetype_allows_window(Standard3, 0, 1, 4));
        assert!(archetype_allows_window(Standard3, 2, 3, 4));
        assert!(!archetype_allows_window(Standard3, 3, 2, 4));
        // PairedNarrow: cols 0 and 2 only, top open row is a lintel
        assert!(archetype_allows_window(PairedNarrow, 0, 1, 4));
        assert!(!archetype_allows_window(PairedNarrow, 1, 1, 4));
        assert!(archetype_allows_window(PairedNarrow, 2, 2, 4));
        assert!(!archetype_allows_window(PairedNarrow, 0, 3, 4));
        // VerticalStrip: col 1 only
        assert!(archetype_allows_window(VerticalStrip, 1, 3, 4));
        assert!(!archetype_allows_window(VerticalStrip, 0, 2, 4));
        // WideHorizontal: cols 0-3, bottom open row is a sill
        assert!(archetype_allows_window(WideHorizontal, 3, 2, 4));
        assert!(!archetype_allows_window(WideHorizontal, 3, 1, 4));
        assert!(!archetype_allows_window(WideHorizontal, 4, 2, 4));
    }

    #[test]
    fn archetype_choice_is_deterministic_and_defaults_hold() {
        for seed in 0..30u64 {
            let a = pick_window_archetype(BuildingCategory::House, ArchEra::Unknown, seed);
            let b = pick_window_archetype(BuildingCategory::House, ArchEra::Unknown, seed);
            assert_eq!(a, b);
        }
        // Unhandled categories stay on the classic layout
        assert_eq!(
            pick_window_archetype(BuildingCategory::Religious, ArchEra::Unknown, 7),
            WindowArchetype::Standard3
        );
        assert_eq!(
            pick_window_archetype(BuildingCategory::GlassySkyscraper, ArchEra::Unknown, 7),
            WindowArchetype::Standard3
        );
    }

    #[test]
    fn houses_vary_archetypes_across_seeds() {
        let mut seen = std::collections::HashSet::new();
        for seed in 0..60u64 {
            seen.insert(format!(
                "{:?}",
                pick_window_archetype(BuildingCategory::House, ArchEra::Unknown, seed)
            ));
        }
        assert!(seen.len() >= 3, "expected variety, got {seen:?}");
    }

    #[test]
    fn era_shifts_archetypes_and_walls() {
        // Ornate era pushes houses toward arched bays
        let mut arched = 0;
        for seed in 0..60u64 {
            if pick_window_archetype(BuildingCategory::House, ArchEra::HistoricOrnate, seed)
                == WindowArchetype::ArchedTraditional
            {
                arched += 1;
            }
        }
        assert!(
            arched >= 20,
            "ornate era should favor arches, got {arched}/60"
        );
        // Panel-era residential walls stay inside the era allow-list
        for seed in 0..40u64 {
            let mut rng = element_rng(seed);
            let b = get_wall_block_for_category(
                BuildingCategory::Residential,
                ArchEra::PostWarPanel,
                Climate::Temperate,
                &mut rng,
            );
            assert!(
                era_allow_list(ArchEra::PostWarPanel).unwrap().contains(&b),
                "panel era picked {b:?}"
            );
        }
        // Unknown era keeps the full palette reachable (no filtering)
        assert!(era_allow_list(ArchEra::Unknown).is_none());
    }

    #[test]
    fn climate_reweights_walls() {
        // Desert houses stay inside the desert-weighted pool and lean sandy
        let mut sandy = 0;
        for seed in 0..60u64 {
            let mut rng = element_rng(seed);
            let b = get_wall_block_for_category(
                BuildingCategory::House,
                ArchEra::Unknown,
                Climate::HotDesert,
                &mut rng,
            );
            assert!(
                climate_wall_weight(Climate::HotDesert, BuildingCategory::House, b) > 0,
                "desert picked excluded {b:?}"
            );
            if matches!(
                b,
                SANDSTONE | SMOOTH_SANDSTONE | MUD_BRICKS | WHITE_TERRACOTTA
            ) {
                sandy += 1;
            }
        }
        assert!(sandy >= 15, "desert should lean sandy, got {sandy}/60");
        // Boreal houses can be wood, which the base palette never yields
        let mut wood = 0;
        for seed in 0..60u64 {
            let mut rng = element_rng(seed);
            let b = get_wall_block_for_category(
                BuildingCategory::House,
                ArchEra::Unknown,
                Climate::Boreal,
                &mut rng,
            );
            if NORDIC_WOOD_ADDITIONS.contains(&b) {
                wood += 1;
            }
        }
        assert!(wood >= 10, "boreal should often be wood, got {wood}/60");
    }

    #[test]
    fn climate_gable_probabilities_ordered() {
        assert!(
            climate_gable_probability(Climate::HotDesert)
                < climate_gable_probability(Climate::Temperate)
        );
        assert!(
            climate_gable_probability(Climate::Boreal)
                > climate_gable_probability(Climate::Temperate)
        );
    }

    #[test]
    fn sahara_bbox_editor_reports_hot_desert() {
        use crate::coordinate_system::cartesian::XZBBox;
        use crate::coordinate_system::geographic::LLBBox;
        use crate::element_processing::building_test_support::test_editor_at;
        let xz = XZBBox::rect_from_xz_lengths(50.0, 50.0).unwrap();
        let editor = test_editor_at(&xz, LLBBox::new(22.9, 12.9, 23.1, 13.1).unwrap());
        assert_eq!(editor.climate(), Climate::HotDesert);
    }

    fn test_config(height: i32, attic: bool, top: bool) -> BuildingConfig {
        BuildingConfig {
            is_ground_level: true,
            building_height: height,
            floor_cycle: 4,
            is_tall_building: false,
            start_y_offset: 0,
            abs_terrain_offset: 0,
            wall_block: BRICK,
            floor_block: OAK_PLANKS,
            window_block: GLASS,
            accent_block: SMOOTH_STONE,
            roof_block: None,
            use_vertical_windows: false,
            use_horizontal_windows: false,
            use_accent_roof_line: false,
            use_accent_lines: false,
            use_vertical_accent: false,
            is_abandoned_building: false,
            has_windows: true,
            has_garage_door: false,
            has_single_door: false,
            category: BuildingCategory::House,
            era: ArchEra::Unknown,
            detail: DetailTier::Standard,
            top_treatment: top,
            attic_style: attic,
            piano_nobile: false,
            wall_depth_style: WallDepthStyle::None,
            has_parapet: false,
            has_lobby_base: false,
            condition: BuildingCondition::Normal,
            element_id: 1,
            style_seed: 1,
            window_phase: 0,
            window_archetype: WindowArchetype::Standard3,
            balcony_band: BalconyBand::Scattered,
            rustication: false,
            base_course_block: None,
            has_storefront: false,
            window_frame: None,
        }
    }

    #[test]
    fn floor_role_boundaries() {
        // 3 storeys at cycle 4: height = 3*4 + 2 = 14.
        let config = test_config(14, false, false);
        assert_eq!(config.floor_role(1), FloorRole::Ground);
        assert_eq!(config.floor_role(5), FloorRole::Ground);
        assert_eq!(config.floor_role(6), FloorRole::Body);
        assert_eq!(config.floor_role(10), FloorRole::Body);
        assert_eq!(config.floor_role(11), FloorRole::Top);
        assert_eq!(config.floor_role(14), FloorRole::Top);
    }

    #[test]
    fn attic_band_is_solid_with_small_lights() {
        use crate::element_processing::building_facade::ColumnFacade;
        let config = test_config(14, true, false);
        let col = ColumnFacade::default();
        // Middle of the attic band: window only at the single light position.
        let light = determine_wall_block_at_position_pristine(1, 12, 0, &config, col);
        assert_eq!(light, GLASS, "attic light at window_col 1, floor_row 2");
        let wall = determine_wall_block_at_position_pristine(0, 12, 0, &config, col);
        assert_eq!(wall, BRICK, "attic band is otherwise solid");
        // Body floors keep full 3-wide windows.
        let body = determine_wall_block_at_position_pristine(0, 7, 0, &config, col);
        assert_eq!(body, GLASS);
    }

    #[test]
    fn top_treatment_narrows_windows_and_adds_band() {
        use crate::element_processing::building_facade::ColumnFacade;
        let config = test_config(14, false, true);
        let col = ColumnFacade::default();
        // Band below the treated top floor (h = height - cycle = 10, row 0).
        let band = determine_wall_block_at_position_pristine(0, 10, 0, &config, col);
        assert_eq!(band, SMOOTH_STONE);
        // Top floor loses its third window column…
        let narrowed = determine_wall_block_at_position_pristine(2, 12, 0, &config, col);
        assert_eq!(narrowed, BRICK);
        // …but keeps the first two.
        let kept = determine_wall_block_at_position_pristine(1, 12, 0, &config, col);
        assert_eq!(kept, GLASS);
    }

    #[test]
    fn mansard_profile_shape() {
        // Steep 2-cell band to steep_h, then shallow 1:2, capped.
        assert_eq!(mansard_boost(0, 4, 10), 0);
        assert_eq!(mansard_boost(1, 4, 10), 2);
        assert_eq!(mansard_boost(2, 4, 10), 4);
        assert_eq!(mansard_boost(3, 4, 10), 4);
        assert_eq!(mansard_boost(4, 4, 10), 5);
        assert_eq!(mansard_boost(8, 4, 6), 6); // capped
        for d in 0..20 {
            assert!(mansard_boost(d + 1, 4, 12) >= mansard_boost(d, 4, 12));
        }
    }

    #[test]
    fn new_roof_shapes_parse_distinctly() {
        assert_eq!(parse_roof_type("mansard"), RoofType::Mansard);
        assert_eq!(parse_roof_type("gambrel"), RoofType::Gambrel);
        assert_eq!(parse_roof_type("half-hipped"), RoofType::HalfHipped);
        assert_eq!(parse_roof_type("hipped"), RoofType::Hipped);
        assert_eq!(parse_roof_type("round"), RoofType::Hipped);
        assert_eq!(parse_roof_type("gabled"), RoofType::Gabled);
    }

    #[test]
    fn podium_tower_planning_is_bounded_and_deterministic() {
        let mut config = test_config(50, false, false);
        config.is_tall_building = true;
        let way = ProcessedWay {
            id: 9,
            nodes: Vec::new(),
            tags: std::iter::once(("building".to_string(), "office".to_string())).collect(),
        };
        let mut floor_area: Vec<(i32, i32)> = Vec::new();
        for x in 20..=50 {
            for z in 20..=44 {
                floor_area.push((x, z));
            }
        }
        let footprint = floor_area.len();

        let mut any_plan = false;
        for seed in 0..30u64 {
            let a = plan_podium_tower(
                &way,
                &config,
                RoofType::Flat,
                true,
                footprint,
                &floor_area,
                seed,
            );
            let b = plan_podium_tower(
                &way,
                &config,
                RoofType::Flat,
                true,
                footprint,
                &floor_area,
                seed,
            );
            assert_eq!(a.is_some(), b.is_some());
            if let (Some(a), Some(b)) = (a, b) {
                any_plan = true;
                assert_eq!(a.podium_height, b.podium_height);
                assert!(a.podium_height == 10 || a.podium_height == 14);
                assert_eq!(a.full_height, 50);
                assert!(a.inset >= 3);
                assert!(config.building_height - a.podium_height >= 2 * config.floor_cycle);
            }
        }
        assert!(any_plan, "the 40% roll should hit within 30 seeds");

        // Small footprints never get a podium split.
        let small: Vec<(i32, i32)> = floor_area.iter().copied().take(300).collect();
        for seed in 0..30u64 {
            assert!(
                plan_podium_tower(&way, &config, RoofType::Flat, true, 300, &small, seed).is_none()
            );
        }
        // Pitched roofs never do either.
        for seed in 0..30u64 {
            assert!(plan_podium_tower(
                &way,
                &config,
                RoofType::Gabled,
                true,
                footprint,
                &floor_area,
                seed
            )
            .is_none());
        }
    }

    // The Luanti stone fallback would silently break the export.
    #[test]
    fn every_reachable_building_block_maps_to_luanti() {
        use crate::luanti_block_map::{to_luanti_node, LuantiGame};
        let mut blocks: Vec<Block> = Vec::new();
        blocks.extend(crate::block_palette::all_building_palette_blocks());
        blocks.extend_from_slice(&RESIDENTIAL_WALL_OPTIONS);
        blocks.extend_from_slice(&COMMERCIAL_WALL_OPTIONS);
        blocks.extend_from_slice(&INDUSTRIAL_WALL_OPTIONS);
        blocks.extend_from_slice(&RELIGIOUS_WALL_OPTIONS);
        blocks.extend_from_slice(&INSTITUTIONAL_WALL_OPTIONS);
        blocks.extend_from_slice(&FARM_WALL_OPTIONS);
        blocks.extend_from_slice(&HISTORIC_WALL_OPTIONS);
        blocks.extend_from_slice(&GARAGE_WALL_OPTIONS);
        blocks.extend_from_slice(&NORDIC_WOOD_ADDITIONS);
        blocks.extend_from_slice(&ACCENT_BLOCK_OPTIONS);
        blocks.extend_from_slice(&WINDOW_VARIATIONS);
        blocks.extend_from_slice(&RESIDENTIAL_WINDOW_OPTIONS);
        blocks.extend_from_slice(&FARM_WINDOW_OPTIONS);
        blocks.extend_from_slice(&HISTORIC_WINDOW_OPTIONS);
        blocks.extend_from_slice(&FLOOR_BLOCK_OPTIONS);
        for style in [
            DoorStyle::Oak,
            DoorStyle::Spruce,
            DoorStyle::DarkOak,
            DoorStyle::Birch,
        ] {
            blocks.push(style.base_block());
        }
        let base: Vec<Block> = blocks.clone();
        for b in base {
            blocks.extend_from_slice(substitute_pool_only(b));
        }
        for b in blocks {
            if b == STONE {
                continue;
            }
            let node = to_luanti_node(b, LuantiGame::Mineclonia, None);
            assert_ne!(
                node.name,
                "mcl_core:stone",
                "{} falls back to stone on Luanti",
                b.name()
            );
        }
    }

    // Identically-tagged parts of one group must resolve the same style.
    #[test]
    fn identically_tagged_parts_resolve_one_shared_style() {
        use crate::element_processing::building_test_support::rect_way;
        let tags: &[(&str, &str)] = &[("building:part", "yes"), ("height", "40")];
        let way_a = rect_way(500, 20, 20, 30, 30, tags);
        let way_b = rect_way(501, 31, 20, 41, 30, tags);
        let group_seed = 4242u64;

        let style_for = |way: &ProcessedWay| {
            let (h, tall) = calculate_building_height(way, "yes", 0, 1.0, None, 4, 100, group_seed);
            let category = BuildingCategory::from_element(way, tall, h, group_seed, 1.0);
            let preset = BuildingStylePreset::for_category(category);
            let era = crate::osm_parser::building_arch_era(&way.tags);
            let detail = compute_detail_tier(way, category, 100, h, false);
            let mut rng = element_rng(group_seed);
            (
                category,
                BuildingStyle::resolve(
                    &preset,
                    way,
                    "yes",
                    category,
                    era,
                    Climate::Temperate,
                    detail,
                    h,
                    h > 6,
                    100,
                    group_seed,
                    &mut rng,
                ),
            )
        };
        let (cat_a, a) = style_for(&way_a);
        let (cat_b, b) = style_for(&way_b);
        assert_eq!(cat_a, cat_b);
        assert_eq!(a.wall_block, b.wall_block);
        assert_eq!(a.window_block, b.window_block);
        assert_eq!(a.accent_block, b.accent_block);
        assert_eq!(a.roof_block, b.roof_block);
        assert_eq!(a.wall_depth_style, b.wall_depth_style);
        assert_eq!(a.roof_type, b.roof_type);
        assert_eq!(
            pick_window_archetype(cat_a, ArchEra::Unknown, group_seed),
            pick_window_archetype(cat_b, ArchEra::Unknown, group_seed)
        );
    }

    // An inferred elevated part ends flush with its ground-level sibling
    #[test]
    fn inferred_elevated_part_tops_flush() {
        let tagged = |pairs: &[(&str, &str)]| ProcessedWay {
            id: 1,
            nodes: Vec::new(),
            tags: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let ground = tagged(&[("building:part", "yes")]);
        let lifted = tagged(&[("building:part", "yes"), ("building:min_level", "1")]);
        for seed in 1..20u64 {
            let (hg, _) = calculate_building_height(&ground, "yes", 0, 1.0, None, 4, 400, seed);
            let (hl, _) = calculate_building_height(&lifted, "yes", 1, 1.0, None, 4, 400, seed);
            let lift = 4 + GROUND_FLOOR_BONUS;
            assert_eq!(hl + lift, hg, "seed {seed}");
        }
    }

    // Parts without roof:shape stay flat instead of rolling the gable lottery.
    #[test]
    fn untagged_parts_never_infer_pitched_roofs() {
        use crate::element_processing::building_test_support::rect_way;
        for seed in 1..30u64 {
            let tags: &[(&str, &str)] = &[("building:part", "yes"), ("height", "12")];
            let way = rect_way(seed * 7 + 1, 20, 20, 32, 32, tags);
            let (h, tall) = calculate_building_height(&way, "yes", 0, 1.0, None, 4, 144, seed);
            let category = BuildingCategory::from_element(&way, tall, h, seed, 1.0);
            let preset = BuildingStylePreset::for_category(category);
            let era = crate::osm_parser::building_arch_era(&way.tags);
            let detail = compute_detail_tier(&way, category, 144, h, false);
            let mut rng = element_rng(seed);
            let style = BuildingStyle::resolve(
                &preset,
                &way,
                "yes",
                category,
                era,
                Climate::Temperate,
                detail,
                h,
                h > 6,
                144,
                seed,
                &mut rng,
            );
            assert_eq!(style.roof_type, RoofType::Flat, "seed {seed}");
            assert!(!style.generate_roof, "seed {seed}");
        }
    }

    #[test]
    fn detail_tier_boundaries() {
        let tagged = |pairs: &[(&str, &str)]| ProcessedWay {
            id: 1,
            nodes: Vec::new(),
            tags: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let plain = tagged(&[]);
        assert_eq!(
            compute_detail_tier(&plain, BuildingCategory::Garage, 30, 3, false),
            DetailTier::Minimal
        );
        assert_eq!(
            compute_detail_tier(&plain, BuildingCategory::House, 100, 10, true),
            DetailTier::Standard
        );
        assert_eq!(
            compute_detail_tier(&plain, BuildingCategory::Commercial, 800, 20, true),
            DetailTier::Enhanced
        );
        let notable = tagged(&[("historic", "yes"), ("heritage", "2"), ("wikidata", "Q42")]);
        assert_eq!(
            compute_detail_tier(&notable, BuildingCategory::Historic, 600, 20, true),
            DetailTier::Landmark
        );
    }

    #[test]
    fn window_pools_cover_previously_generic_categories() {
        assert_eq!(
            window_pool_for_category(BuildingCategory::Farm),
            &FARM_WINDOW_OPTIONS[..]
        );
        assert_eq!(
            window_pool_for_category(BuildingCategory::Office),
            &INSTITUTIONAL_WINDOW_OPTIONS[..]
        );
        assert_eq!(
            window_pool_for_category(BuildingCategory::Historic),
            &HISTORIC_WINDOW_OPTIONS[..]
        );
    }

    #[test]
    fn dark_wall_or_dark_accent_gets_dark_glass() {
        // dark wall
        assert_eq!(
            coordinated_window_block(BLACK_CONCRETE, SMOOTH_STONE, LIGHT_BLUE_STAINED_GLASS),
            GRAY_STAINED_GLASS
        );
        // light wall but dark accent band (the blackstone-line modern tower)
        assert_eq!(
            coordinated_window_block(WHITE_CONCRETE, BLACKSTONE, LIGHT_BLUE_STAINED_GLASS),
            GRAY_STAINED_GLASS
        );
        assert_eq!(
            coordinated_window_block(GRAY_CONCRETE, SMOOTH_STONE, LIGHT_BLUE_STAINED_GLASS),
            GRAY_STAINED_GLASS
        );
    }

    #[test]
    fn light_wall_and_light_accent_keep_the_light_window() {
        assert_eq!(
            coordinated_window_block(WHITE_CONCRETE, SMOOTH_STONE, LIGHT_BLUE_STAINED_GLASS),
            LIGHT_BLUE_STAINED_GLASS
        );
        assert_eq!(
            coordinated_window_block(QUARTZ_BLOCK, POLISHED_ANDESITE, LIGHT_BLUE_STAINED_GLASS),
            LIGHT_BLUE_STAINED_GLASS
        );
    }

    #[test]
    fn glass_family_variant_is_always_glass_family() {
        for seed in 0u64..300 {
            assert!(matches!(
                BuildingCategory::glass_family_variant(seed),
                BuildingCategory::GlassySkyscraper
                    | BuildingCategory::GridSkyscraper
                    | BuildingCategory::GlassCornerSkyscraper
            ));
        }
    }
}

#[cfg(test)]
mod facade_integration_tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{
        bitmap_with_rect, rect_way, test_editor,
    };
    use clap::Parser as _;
    use fnv::FnvHashMap;

    const DOOR_BLOCKS: &[Block] = &[OAK_DOOR, SPRUCE_DOOR_LOWER, DARK_OAK_DOOR_LOWER, BIRCH_DOOR];

    fn flat_args() -> Args {
        Args::parse_from([
            "arnis",
            "--bbox",
            "1,2,3,4",
            "--mode",
            "geo-only",
            "--ground-level",
            "0",
        ])
    }

    fn run_building(
        editor: &mut WorldEditor,
        way: &ProcessedWay,
        road: &CoordinateBitmap,
        footprints: &CoordinateBitmap,
    ) {
        let args = flat_args();
        let cache = FloodFillCache::new();
        let passages = CoordinateBitmap::new_empty();
        let groups: FnvHashMap<u64, Vec<u64>> = FnvHashMap::default();
        let ctx = BuildingContext {
            flood_fill_cache: &cache,
            building_passages: &passages,
            road_mask: road,
            building_footprints: footprints,
            group_members: &groups,
        };
        generate_buildings(editor, way, &args, None, None, &ctx, way.id);
    }

    #[test]
    fn synthetic_door_lands_on_the_street_wall() {
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let road = bitmap_with_rect(&xz, 0, 12, 59, 14);
        let footprints = CoordinateBitmap::new(&xz);
        let way = rect_way(
            42,
            20,
            20,
            40,
            32,
            &[("building", "house"), ("building:levels", "2")],
        );
        let mut editor = test_editor(&xz);
        run_building(&mut editor, &way, &road, &footprints);

        let door_on =
            |e: &WorldEditor, x: i32, z: i32| e.check_for_block(x, 1, z, Some(DOOR_BLOCKS));
        let street_wall = (20..=40).any(|x| door_on(&editor, x, 20));
        let other_walls = (20..=40).any(|x| door_on(&editor, x, 32))
            || (20..=32).any(|z| door_on(&editor, 20, z) || door_on(&editor, 40, z));
        assert!(street_wall, "door should be on the street-facing wall");
        assert!(!other_walls, "no doors on the other walls");
    }

    #[test]
    fn mapped_entrance_suppresses_the_synthetic_door() {
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let road = bitmap_with_rect(&xz, 0, 12, 59, 14);
        let footprints = CoordinateBitmap::new(&xz);
        let mut way = rect_way(
            44,
            20,
            20,
            40,
            32,
            &[("building", "house"), ("building:levels", "2")],
        );
        // Mapped entrance in the middle of the rear wall.
        let mut tags = HashMap::new();
        tags.insert("entrance".to_string(), "yes".to_string());
        way.nodes.insert(
            3,
            ProcessedNode {
                id: 4444,
                tags,
                x: 30,
                z: 32,
            },
        );
        let mut editor = test_editor(&xz);
        run_building(&mut editor, &way, &road, &footprints);

        assert!(
            editor.check_for_block(30, 1, 32, Some(DOOR_BLOCKS)),
            "mapped entrance gets an oriented door"
        );
        let street_doors = (20..=40).any(|x| editor.check_for_block(x, 1, 20, Some(DOOR_BLOCKS)));
        assert!(
            !street_doors,
            "no synthetic door once an entrance is mapped"
        );
    }

    #[test]
    fn party_wall_has_no_windows_but_free_wall_does() {
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let road = bitmap_with_rect(&xz, 0, 12, 59, 14);
        // Attached neighbor east of the house.
        let footprints = bitmap_with_rect(&xz, 41, 20, 52, 32);
        let way = rect_way(
            43,
            20,
            20,
            40,
            32,
            &[("building", "house"), ("building:levels", "2")],
        );
        let mut editor = test_editor(&xz);
        run_building(&mut editor, &way, &road, &footprints);

        let glass = |e: &WorldEditor, x: i32, z: i32, y: i32| {
            e.check_for_block(x, y, z, Some(&RESIDENTIAL_WINDOW_OPTIONS))
        };
        let east_has_glass = (21..32).any(|z| (2..=9).any(|y| glass(&editor, 40, z, y)));
        let west_has_glass = (21..32).any(|z| (2..=9).any(|y| glass(&editor, 20, z, y)));
        assert!(
            !east_has_glass,
            "party wall must not glaze into the neighbor"
        );
        assert!(west_has_glass, "free wall keeps its windows");
    }

    #[test]
    fn mansard_and_gambrel_roofs_build_above_the_walls() {
        const SLATE: &[Block] = &[POLISHED_BLACKSTONE, DEEPSLATE_BRICKS, BLACKSTONE];
        for shape in ["mansard", "gambrel"] {
            let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
            let road = bitmap_with_rect(&xz, 0, 12, 59, 14);
            let footprints = CoordinateBitmap::new(&xz);
            let way = rect_way(
                46,
                20,
                20,
                40,
                32,
                &[
                    ("building", "house"),
                    ("building:levels", "2"),
                    ("roof:shape", shape),
                    ("roof:material", "slate"),
                ],
            );
            let mut editor = test_editor(&xz);
            run_building(&mut editor, &way, &road, &footprints);

            // Slate roof mass above the 10-block walls near the footprint centre.
            let roofed =
                (11..=18).any(|y| (24..=28).any(|z| editor.check_for_block(30, y, z, Some(SLATE))));
            assert!(roofed, "{shape} roof should rise above the walls");
        }
    }

    #[test]
    fn street_aware_generation_is_deterministic() {
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let road = bitmap_with_rect(&xz, 0, 12, 59, 14);
        let footprints = bitmap_with_rect(&xz, 41, 20, 52, 32);
        let way = rect_way(
            45,
            20,
            20,
            40,
            32,
            &[("building", "apartments"), ("building:levels", "3")],
        );
        let mut a = test_editor(&xz);
        run_building(&mut a, &way, &road, &footprints);
        let mut b = test_editor(&xz);
        run_building(&mut b, &way, &road, &footprints);
        assert_eq!(a.content_hash(), b.content_hash());
    }
}

#[cfg(test)]
mod facade_dump {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{
        bitmap_with_rect, rect_way, test_editor,
    };
    use clap::Parser as _;
    use fnv::FnvHashMap;

    fn glyph(block: Option<Block>) -> char {
        let Some(b) = block else { return ' ' };
        let name = b.name();
        if name.contains("glass") {
            'o'
        } else if name.contains("trapdoor") {
            '^'
        } else if name.contains("door") {
            'D'
        } else if name.contains("stair") {
            '/'
        } else if name.contains("slab") {
            '_'
        } else if name.contains("fence") || name.contains("bars") {
            '+'
        } else if name.contains("lantern") || name.contains("potted") || name.contains("pot") {
            '*'
        } else if name == "air" {
            ' '
        } else {
            '#'
        }
    }

    // Renders the street facade of a synthetic building as ASCII for manual
    // dial tuning: cargo test facade_dump -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_street_facades() {
        for (label, tags) in [
            (
                "apartments 4 levels, pre-war",
                vec![
                    ("building", "apartments"),
                    ("building:levels", "4"),
                    ("start_date", "1905"),
                ],
            ),
            (
                "commercial 3 levels (storefront)",
                vec![("building", "commercial"), ("building:levels", "3")],
            ),
            ("plain house, no tags", vec![("building", "house")]),
        ] {
            let xz = XZBBox::rect_from_xz_lengths(70.0, 60.0).unwrap();
            let road = bitmap_with_rect(&xz, 0, 12, 69, 14);
            let footprints = CoordinateBitmap::new(&xz);
            let way = rect_way(4242, 22, 20, 52, 34, &tags);
            let mut editor = test_editor(&xz);
            let args = Args::parse_from([
                "arnis",
                "--bbox",
                "1,2,3,4",
                "--mode",
                "geo-only",
                "--ground-level",
                "0",
            ]);
            let cache = FloodFillCache::new();
            let passages = CoordinateBitmap::new_empty();
            let groups: FnvHashMap<u64, Vec<u64>> = FnvHashMap::default();
            let ctx = BuildingContext {
                flood_fill_cache: &cache,
                building_passages: &passages,
                road_mask: &road,
                building_footprints: &footprints,
                group_members: &groups,
            };
            generate_buildings(&mut editor, &way, &args, None, None, &ctx, way.id);

            println!("\n=== {label} ===");
            println!("street wall plane (z=20) with outward layer (z=19) overlaid:");
            for y in (0..=22).rev() {
                let mut line = String::new();
                for x in 20..=54 {
                    let outward = editor.get_block_absolute(x, y, 19);
                    let wall = editor.get_block_absolute(x, y, 20);
                    let c = if outward.is_some() {
                        glyph(outward).to_ascii_uppercase()
                    } else {
                        glyph(wall)
                    };
                    line.push(c);
                }
                println!("y{y:>2} |{line}|");
            }
        }
    }
}
