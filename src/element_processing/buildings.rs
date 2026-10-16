use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::climate::Climate;
use crate::clipping::clip_way_to_bbox;
use crate::colors::color_text_to_rgb_tuple;
use crate::deterministic_rng::{coord_rng, element_rng};
use crate::element_processing::building_facade::{
    compute_facade_plan, BaySlot, BuildingContext, ColumnFacade, FacadeAnchor, FacadeClass,
    FacadePlan, MIN_FACADE_FOOTPRINT,
};
use crate::element_processing::historic;
use crate::element_processing::rooftops::{self, RoofDeck, RoofMembrane, RooftopBrief};
use crate::element_processing::subprocessor::interior::{
    generate_building_interior, plan_interior, Entry, InteriorRequest, PlanInputs,
};
use crate::floodfill_cache::{CoordinateBitmap, FloodFillCache};
use crate::osm_parser::{
    ArchEra, ProcessedMemberRole, ProcessedNode, ProcessedRelation, ProcessedWay,
};
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use fnv::FnvHashSet;
use rand::Rng;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
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
    Gabled, // Two sloping sides meeting at a ridge
    /// Barrel vault: a rounded section across the ridge.
    Round,
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

/// Building tradition of the generated area, from its location. Roof habits
/// differ most: North American cities roof their attached and multi-storey
/// stock flat, European towns keep pitched roofs on tall blocks as well.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BuildingRegion {
    NorthAmerica,
    Europe,
    Other,
}

impl BuildingRegion {
    pub(crate) fn at(lat: f64, lon: f64) -> Self {
        MaterialRegion::at(lat, lon).building_region()
    }
}

/// Building-material tradition of the generated area: what its walls and roof
/// coverings are made of. Finer than `BuildingRegion`, whose split is about
/// roof shapes; Europe and North America each hold several traditions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MaterialRegion {
    /// Red and buff brick, brownstone, limestone; asphalt shingle houses.
    NorthAmerica,
    /// California and the Southwest, Florida: stucco, clay tile.
    NorthAmericaSunbelt,
    /// Canada: more brick and siding, Montreal greystone, steel roofs.
    Canada,
    /// Britain and Ireland: stock and red brick, white stucco, slate.
    BritishIsles,
    /// Low Countries and the North German and Baltic coast: brick, red and
    /// dark glazed tile.
    BrickNorth,
    /// Dressed limestone and render under zinc and slate.
    France,
    /// German-speaking lands, Hungary, Berlin and the north-east: pale render
    /// under red clay tile.
    Central,
    /// Saxony, Thuringia, Bohemia, Silesia: the same render under darker red
    /// Biberschwanz tile.
    CentralEast,
    /// Render, brick and timber under black or red roofs.
    Nordic,
    /// Southern Europe, Turkey and the Balkan coast: render under orange tile.
    Mediterranean,
    /// Russia, Ukraine, Belarus, the Baltic states, Siberia and Mongolia:
    /// ochre render and brick, Soviet panel blocks, painted sheet-iron roofs
    /// in green, red and grey.
    EastEurope,
    /// The Caucasus and Central Asia: tuff, limestone and render, panel
    /// blocks, flat roofs in town and sheet metal on houses.
    CentralAsia,
    /// North Africa, the Middle East, Iran, Afghanistan and the Horn: sand
    /// and cream render and stone under flat roofs.
    MiddleEast,
    /// Africa south of the Sahara: render, laterite, face brick and block
    /// under corrugated sheet and tile.
    SubSaharanAfrica,
    /// The Indian subcontinent: painted plaster and brick under flat RCC
    /// roofs.
    SouthAsia,
    /// Mainland and island Southeast Asia and the Pacific: shophouses in
    /// painted render, houses under tile and corrugated sheet.
    SoutheastAsia,
    /// China, Taiwan, Hong Kong: tile-clad concrete, grey and red brick,
    /// grey tile roofs.
    EastAsia,
    /// Korea: red-brick villas and white apartment slabs, green-painted flat
    /// roofs.
    Korea,
    /// Japan: light siding and mortar houses under dark grey tile and metal,
    /// tile-clad flat-roofed mid-rises.
    Japan,
    /// Mexico, Central America, the Caribbean and tropical South America:
    /// brightly painted render and brick, flat roofs and clay tile.
    LatinAmerica,
    /// Argentina, Chile, Uruguay and the Brazilian south: European-style
    /// render, face brick, French clay tile.
    SouthernCone,
    /// Australia and New Zealand: brick veneer and weatherboard under
    /// terracotta, concrete tile and corrugated steel.
    Australasia,
    Other,
}

/// How untagged roofs are pitched where the European and North American
/// rules do not apply.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RoofHabit {
    /// Chance that a house (or a small, low, free-standing `building=yes`)
    /// gets a pitched roof.
    house: f64,
    /// Chance for a low urban block (walk-up, shophouse, row) up to
    /// `block_max_m`; taller and larger blocks are flat.
    block: f64,
    block_max_m: f64,
    /// Share of free-standing pitched roofs that are hipped.
    hipped: f64,
}

impl MaterialRegion {
    pub(crate) fn at(lat: f64, lon: f64) -> Self {
        if (15.0..=75.0).contains(&lat) && (-170.0..=-50.0).contains(&lon) {
            Self::in_north_america(lat, lon)
        } else if (34.0..=72.0).contains(&lat) && (-25.0..=45.0).contains(&lon) {
            Self::in_europe(lat, lon)
        } else {
            Self::elsewhere(lat, lon)
        }
    }

    /// The roof-shape tradition the material region belongs to.
    pub(crate) fn building_region(self) -> BuildingRegion {
        use MaterialRegion as R;
        match self {
            R::NorthAmerica | R::NorthAmericaSunbelt | R::Canada => BuildingRegion::NorthAmerica,
            R::BritishIsles
            | R::BrickNorth
            | R::France
            | R::Central
            | R::CentralEast
            | R::Nordic
            | R::Mediterranean
            | R::EastEurope => BuildingRegion::Europe,
            _ => BuildingRegion::Other,
        }
    }

    /// Roof habit of the regions outside the European town and North
    /// American city rules; None keeps those rules (and the climate rule of
    /// `Other`).
    fn roof_habit(self) -> Option<RoofHabit> {
        use MaterialRegion as R;
        let habit = |house, block, block_max_m, hipped| RoofHabit {
            house,
            block,
            block_max_m,
            hipped,
        };
        Some(match self {
            // Detached houses are pitched; old wooden apartments too.
            R::Japan => habit(0.93, 0.35, 10.0, 0.4),
            // Houses split between tile or metal roofs and flat slabs; villas
            // and apartments are flat.
            R::Korea => habit(0.45, 0.05, 10.0, 0.35),
            R::EastAsia => habit(0.40, 0.10, 10.0, 0.25),
            // Hipped tile and sheet over houses, some shophouse rows.
            R::SoutheastAsia => habit(0.75, 0.20, 13.0, 0.6),
            // Flat RCC slabs nearly everywhere.
            R::SouthAsia => habit(0.15, 0.03, 10.0, 0.4),
            R::MiddleEast => habit(0.04, 0.0, 0.0, 0.3),
            // Hipped corrugated sheet; flat in town and in the Sahel.
            R::SubSaharanAfrica => habit(0.75, 0.25, 10.0, 0.65),
            // Concrete slab houses beside clay tile.
            R::LatinAmerica => habit(0.40, 0.10, 10.0, 0.35),
            R::SouthernCone => habit(0.55, 0.12, 10.0, 0.4),
            // Pre-revolutionary blocks and many five-storey Soviet blocks
            // carry low hipped sheet roofs; towers are flat.
            R::EastEurope => habit(0.92, 0.55, 23.0, 0.6),
            R::CentralAsia => habit(0.70, 0.15, 13.0, 0.5),
            // Hipped roofs on nearly every house and many walk-up flats.
            R::Australasia => habit(0.97, 0.7, 13.0, 0.7),
            _ => return None,
        })
    }

    fn in_north_america(lat: f64, lon: f64) -> Self {
        // Greenland, east of Baffin Bay.
        if lat > 59.5 && (lon > -60.0 || (lat > 76.0 && lon > -75.0)) {
            return MaterialRegion::Nordic;
        }
        // South of the Rio Grande line, the Caribbean and the Bahamas.
        let border = if lon <= -114.8 {
            32.53
        } else if lon <= -111.0 {
            32.53 - (lon + 114.8) * 0.316
        } else if lon <= -106.5 {
            31.33
        } else if lon <= -97.1 {
            31.76 - 0.645 * (lon + 106.5)
        } else {
            // Below the Florida Keys.
            24.4
        };
        if lat < border || (lon > -79.5 && lat < 27.5) {
            MaterialRegion::LatinAmerica
        } else if Self::canadian(lat, lon) {
            MaterialRegion::Canada
        } else if (lat < 42.0 && lon < -114.0)
            || (lat < 37.5 && lon < -103.0)
            || (lat < 31.0 && lon > -87.7)
        {
            MaterialRegion::NorthAmericaSunbelt
        } else {
            MaterialRegion::NorthAmerica
        }
    }

    /// North of the US border: the 49th parallel, the Great Lakes, the St
    /// Lawrence and the Maine line, to a few kilometres.
    fn canadian(lat: f64, lon: f64) -> bool {
        // Alaska.
        if lon < -141.0 {
            return false;
        }
        // Vancouver Island reaches below the 49th parallel.
        if (-125.0..=-123.2).contains(&lon) && lat >= 48.3 {
            return true;
        }
        // Windsor lies south of the Detroit river, which runs east into Lake
        // St Clair; Sarnia faces Port Huron across the St Clair river.
        if (-83.13..-82.415).contains(&lon) {
            let river = if lon < -82.9 {
                42.325 + (lon + 83.04) * 0.35
            } else {
                42.36 + (lon + 82.9) * 0.55
            };
            return (41.7..river).contains(&lat);
        }
        let border = if lon < -95.15 {
            49.0
        } else if lon < -90.5 {
            // The Rainy river: International Falls and Fort Frances.
            48.605
        } else if lon < -89.5 {
            48.0
        } else if lon < -84.5 {
            // Lake Superior's north shore.
            47.8
        } else if lon < -83.13 {
            46.25
        } else if lon < -78.95 {
            // Lake Erie's north shore, up to the Niagara.
            41.75 + (lon + 82.5) * 0.24
        } else if lon < -76.35 {
            43.65
        } else if lon < -74.73 {
            // The St Lawrence: Kingston, Brockville, Cornwall.
            interpolate(
                &[
                    (-76.35, 44.17),
                    (-75.9, 44.36),
                    (-75.68, 44.585),
                    (-75.5, 44.70),
                    (-75.1, 44.88),
                    (-74.73, 45.0),
                ],
                lon,
            )
        } else if lon < -71.5 {
            45.0
        } else if lon < -69.2 {
            45.0 + (lon + 71.5) * 1.07
        } else if lon < -67.8 {
            47.25
        } else if lon < -67.0 {
            45.0
        } else {
            // New Brunswick, Nova Scotia, Newfoundland.
            40.0
        };
        lat >= border
    }

    fn in_europe(lat: f64, lon: f64) -> Self {
        // The Maghreb coast south of Spain, and the Levant and Mesopotamia
        // south of the Turkish border.
        let maghreb = lon < 11.6 && lat < if lon < -1.5 { 35.95 } else { 37.5 };
        let levant = lon >= 35.7
            && lat
                < if lon >= 42.4 {
                    37.4
                } else if lon >= 40.0 {
                    37.15
                } else {
                    37.0
                };
        if maghreb || levant {
            return MaterialRegion::MiddleEast;
        }
        // Georgia and Armenia; the Russian Black Sea coast stays Russian.
        let caucasus = (lat >= 41.45 && lon >= 40.0 && lat < 43.35)
            || (lon >= 43.45 && (40.3..43.35).contains(&lat))
            || (lon >= 44.4 && (38.85..43.35).contains(&lat));
        if caucasus {
            return MaterialRegion::CentralAsia;
        }
        // Belarus, Ukraine, Moldova, the Baltic states and Russia, east of
        // Poland and Romania and south of the Finnish border.
        let prut = if lat >= 47.2 {
            27.7 - (lat - 47.2) * 1.19
        } else {
            28.1 - (lat - 46.0) * 0.33
        };
        let east = (lat >= 48.0
            && lon >= 23.6
            && (lat < 59.7 || lon >= 28.0)
            && (lat < 60.5 || lon >= 31.0))
            || (lat >= 54.35 && lon >= 19.6 && lat < 59.7)
            || ((45.5..48.5).contains(&lat) && lon >= prut)
            || (lat >= 44.3 && lon >= 29.7)
            || (lat >= 43.35 && lon >= 37.0);
        if east {
            return MaterialRegion::EastEurope;
        }
        // The Channel coast: Cornwall reaches 50°N, Kent faces Boulogne.
        let british = (-11.0..=1.8).contains(&lon)
            && lat <= 61.0
            && lat
                >= if lon < -2.0 {
                    49.85
                } else if lon < 1.25 {
                    50.3
                } else {
                    51.0
                };
        if british {
            return MaterialRegion::BritishIsles;
        }
        if lat >= 62.0 || (lat >= 54.5 && lon >= 4.5 && (lon < 21.0 || lat > 59.5)) {
            return MaterialRegion::Nordic;
        }
        if lat < 44.3 || (lat < 46.0 && (6.6..13.9).contains(&lon)) {
            return MaterialRegion::Mediterranean;
        }
        let low_countries = (50.25..=53.7).contains(&lat)
            && lon >= 2.4
            && lon < if lat >= 52.0 { 7.05 } else { 6.05 };
        let north_coast = lat >= 53.0 && (7.0..24.0).contains(&lon);
        let westphalia = lat >= 51.7 && (6.0..9.0).contains(&lon);
        if low_countries || north_coast || westphalia {
            return MaterialRegion::BrickNorth;
        }
        // West of the Rhine and the Jura.
        let france = lon >= -5.5
            && lat <= 51.2
            && lon
                < if lat >= 49.0 {
                    6.5
                } else if lat >= 47.6 {
                    7.6
                } else {
                    6.0
                };
        if france {
            MaterialRegion::France
        } else if (49.6..52.0).contains(&lat) && lon >= 10.5 {
            MaterialRegion::CentralEast
        } else {
            MaterialRegion::Central
        }
    }

    /// Outside the North American and European boxes. Borders are traced
    /// coarsely, along the lines where building habits change.
    fn elsewhere(lat: f64, lon: f64) -> Self {
        use MaterialRegion as R;
        // Australia and New Zealand.
        if (lat <= -10.5 && (112.0..=155.0).contains(&lon)) || (lat <= -33.5 && lon >= 165.0) {
            return R::Australasia;
        }
        // Japan, without Korea's south-east coast and the Russian shore of
        // the Sea of Japan; the Ryukyu islands.
        let japan = ((30.0..=45.6).contains(&lat)
            && (129.0..=146.0).contains(&lon)
            && !(lon < 129.6 && lat >= 34.8)
            && !(lat > 42.3 && lon < 139.0))
            || ((24.0..30.0).contains(&lat) && (122.9..=131.5).contains(&lon));
        if japan {
            return R::Japan;
        }
        // Korea, south of the Yalu and the Tumen.
        if (124.5..=130.7).contains(&lon)
            && lat >= 33.0
            && lat < (39.9 + (lon - 124.3) * 0.75).min(42.3)
        {
            return R::Korea;
        }
        // Russia beyond the Urals and the Amur, Kazakhstan's north,
        // Mongolia.
        let siberia = (lon >= 131.0 && lat >= 42.4)
            || (lat >= 53.6 && lon >= 87.5)
            || (lat >= 50.0 && (87.5..119.5).contains(&lon))
            || (lat >= 49.6 && (127.6..131.0).contains(&lon));
        let steppe = ((45.0..87.5).contains(&lon) && lat >= 49.2)
            || ((45.0..53.0).contains(&lon) && lat >= 45.0)
            || ((53.0..82.5).contains(&lon) && lat >= 46.0)
            || ((82.5..85.7).contains(&lon) && lat >= 47.2);
        let mongolia = (87.7..116.0).contains(&lon) && lat >= Self::mongolia_south_border(lon);
        if siberia || steppe || mongolia {
            return R::EastEurope;
        }
        // The Caucasus east of 45°E and the Central Asian republics, west of
        // the Chinese border.
        let central_asia = ((45.0..53.0).contains(&lon) && lat >= 38.5)
            || ((53.0..73.5).contains(&lon) && lat >= 37.4)
            || ((73.5..80.2).contains(&lon) && lat >= 39.5)
            || ((80.2..82.5).contains(&lon) && lat >= 45.0);
        if central_asia {
            return R::CentralAsia;
        }
        // Atlantic islands of Spain and Portugal.
        if lon < -13.0 && (27.5..40.0).contains(&lat) {
            return R::Mediterranean;
        }
        // Afghanistan, west of the Durand line.
        let afghan_east = if lat < 31.0 {
            66.5
        } else if lat < 33.0 {
            69.0
        } else if lat < 34.6 {
            70.6
        } else if lat < 36.5 {
            71.3
        } else {
            74.9
        };
        let afghanistan = (29.4..38.5).contains(&lat) && (60.5..afghan_east).contains(&lon);
        // The Sahara and Egypt, Arabia, Iran, the Horn.
        let middle_east = (lat >= 15.5 && (-18.0..61.6).contains(&lon))
            || (lat >= 12.0 && (42.5..61.6).contains(&lon))
            || (lat >= -1.7 && (43.0..61.6).contains(&lon));
        if (afghanistan || middle_east) && lat < 45.0 {
            return R::MiddleEast;
        }
        // The subcontinent, below the Himalaya and west of Myanmar.
        let himalaya = if lon < 77.8 {
            37.1
        } else if lon < 80.0 {
            35.5
        } else if lon < 81.0 {
            30.4
        } else if lon < 88.2 {
            30.4 - (lon - 81.0) * 0.34
        } else {
            28.0
        };
        let burma = if lat < 24.0 {
            92.6
        } else if lat < 27.0 {
            94.6
        } else {
            97.5
        };
        if (60.8..burma).contains(&lon) && lat >= -1.0 && lat < himalaya {
            return R::SouthAsia;
        }
        // Mainland Southeast Asia below Yunnan and Guangxi, the archipelagos
        // and the tropical Pacific.
        let china_south = if lon < 97.5 {
            28.5
        } else if lon < 100.5 {
            24.0
        } else if lon < 106.5 {
            22.9
        } else if lon < 108.5 {
            22.0
        } else if lon < 119.0 {
            17.5
        } else {
            21.3
        };
        let pacific = (lon >= 141.0 || lon <= -130.0) && (-30.0..25.0).contains(&lat);
        if ((92.6..141.0).contains(&lon) && (-11.0..china_south).contains(&lat)) || pacific {
            return R::SoutheastAsia;
        }
        if (73.5..=135.0).contains(&lon) && (17.5..53.6).contains(&lat) {
            return R::EastAsia;
        }
        if (-25.0..=58.0).contains(&lon) && (-35.5..15.5).contains(&lat) {
            return R::SubSaharanAfrica;
        }
        if (-120.0..=-30.0).contains(&lon) && (-60.0..15.0).contains(&lat) {
            return if lat < -30.0 {
                R::SouthernCone
            } else {
                R::LatinAmerica
            };
        }
        // Svalbard.
        if lat >= 66.0 && (-25.0..=45.0).contains(&lon) {
            return R::Nordic;
        }
        R::Other
    }

    /// Latitude of the Mongolian-Chinese border at `lon`, traced through
    /// its corners.
    fn mongolia_south_border(lon: f64) -> f64 {
        interpolate(
            &[
                (87.7, 49.2),
                (91.0, 46.6),
                (97.0, 42.75),
                (105.0, 41.6),
                (112.0, 43.7),
                (116.0, 45.5),
            ],
            lon,
        )
    }
}

/// Piecewise-linear value at `x` through `points` (sorted by x), held flat
/// beyond the ends.
fn interpolate(points: &[(f64, f64)], x: f64) -> f64 {
    let (first, last) = (points[0], points[points.len() - 1]);
    if x <= first.0 {
        return first.1;
    }
    for pair in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        if x <= x1 {
            return y0 + (x - x0) / (x1 - x0) * (y1 - y0);
        }
    }
    last.1
}

/// Side of the square map cells that share a sub-palette, in blocks at 1:1.
const DISTRICT_CELL: f64 = 128.0;

/// Seed of the district cell around (x, z). Buildings in one cell draw their
/// walls and roofs from the same narrowed palette, so a street reads as one
/// fabric while the next quarter can differ.
fn district_seed(x: i32, z: i32, scale: f64) -> u64 {
    let cell = (DISTRICT_CELL * scale).round().max(16.0) as i32;
    coord_rng(
        x.div_euclid(cell),
        z.div_euclid(cell),
        0xD157_C7A1_5EED_0001,
    )
    .random()
}

/// What the style choice needs to know about the building's surroundings.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RoofContext {
    pub region: BuildingRegion,
    /// Shares a wall with a neighbour (terraced row, perimeter block).
    pub attached: bool,
    pub scale: f64,
    /// Regional wall and roof materials.
    pub materials: MaterialRegion,
    /// `district_seed` of the building (of its whole group for parts).
    pub district: u64,
    /// Ringed by other buildings on all sides: a pavilion inside a hall or a
    /// courtyard infill, whose roof is under the neighbour's storeys.
    pub enclosed: bool,
}

impl Default for RoofContext {
    fn default() -> Self {
        Self {
            region: BuildingRegion::Other,
            attached: false,
            scale: 1.0,
            materials: MaterialRegion::Other,
            district: 0,
            enclosed: false,
        }
    }
}

/// Roof of an untagged European building that is not a detached house: old-town
/// fabric up to about seven storeys is pitched, whatever it houses, while
/// postwar and contemporary blocks mostly are not. None leaves the decision to
/// the general rules (houses, farms, sheds, other regions).
fn european_default_roof(
    element: &ProcessedWay,
    building_type: &str,
    era: ArchEra,
    building_height: i32,
    footprint_size: usize,
    style_seed: u64,
    ctx: RoofContext,
) -> Option<(RoofType, bool)> {
    const CIVIC_AND_COMMERCIAL: &[&str] = &[
        "commercial",
        "retail",
        "office",
        "hotel",
        "public",
        "civic",
        "government",
        "school",
        "university",
        "college",
        "kindergarten",
        "townhall",
        "historic",
        "palace",
        "castle",
        "museum",
        "theatre",
    ];
    if ctx.region != BuildingRegion::Europe
        || ctx.materials.roof_habit().is_some()
        || element.tags.contains_key("building:part")
    {
        return None;
    }
    let urban_type = matches!(
        building_type,
        "apartments" | "residential" | "yes" | "terrace"
    ) || CIVIC_AND_COMMERCIAL.contains(&building_type);
    if !urban_type && !qualifies_for_auto_gabled_roof(building_type) {
        return None;
    }
    let scale_sq = ctx.scale * ctx.scale;
    let footprint_m2 = footprint_size as f64 / scale_sq;
    if f64::from(building_height) > 26.0 * ctx.scale || footprint_m2 > 3000.0 {
        return None;
    }
    // Ragged outlines (L-blocks around a car park, arcades) keep their flat lids.
    if !element.nodes.is_empty() {
        let (min_x, max_x, min_z, max_z) = element.nodes.iter().fold(
            (i32::MAX, i32::MIN, i32::MAX, i32::MIN),
            |(a, b, c, d), n| (a.min(n.x), b.max(n.x), c.min(n.z), d.max(n.z)),
        );
        let bbox_cells = (f64::from(max_x - min_x + 1) * f64::from(max_z - min_z + 1)).max(1.0);
        if (footprint_size as f64 / bbox_cells) < 0.5 {
            return None;
        }
    }
    let commercial = CIVIC_AND_COMMERCIAL.contains(&building_type)
        && !matches!(building_type, "historic" | "palace" | "castle" | "townhall");
    let share = match era {
        ArchEra::HistoricOrnate => 0.95,
        ArchEra::TraditionalPreWar => 0.88,
        ArchEra::PostWarPanel if commercial => 0.10,
        ArchEra::PostWarPanel => 0.40,
        ArchEra::Contemporary if commercial => 0.05,
        ArchEra::Contemporary => 0.25,
        // Unknown era: attached old-town fabric is the common case.
        ArchEra::Unknown if ctx.attached => 0.80,
        ArchEra::Unknown if footprint_m2 > 1500.0 => 0.40,
        // Small detached buildings follow the general house rules.
        ArchEra::Unknown if !urban_type => return None,
        ArchEra::Unknown => 0.65,
    };
    let mut rng = element_rng(style_seed ^ 0x5E0F_EA57_0C1D_70B5);
    if !rng.random_bool(share) {
        return Some((RoofType::Flat, false));
    }
    let tall = f64::from(building_height) >= 14.0 * ctx.scale;
    let roof =
        if era == ArchEra::HistoricOrnate && footprint_m2 >= 400.0 && tall && rng.random_bool(0.15)
        {
            RoofType::Mansard
        } else if ctx.attached {
            RoofType::Gabled
        } else {
            RoofType::Hipped
        };
    Some((roof, true))
}

/// Roof of an untagged building where the region has a roof habit of its own
/// (`MaterialRegion::roof_habit`): houses and low blocks are pitched at the
/// habit's odds, everything else is flat. Hot deserts damp the odds further
/// (a Sahel or Rajasthan town is flat), except Australia's sheet-roofed
/// outback. None leaves other building types to the general rules.
#[allow(clippy::too_many_arguments)]
fn regional_default_roof(
    element: &ProcessedWay,
    building_type: &str,
    era: ArchEra,
    climate: Climate,
    building_height: i32,
    footprint_size: usize,
    style_seed: u64,
    ctx: RoofContext,
) -> Option<(RoofType, bool)> {
    let habit = ctx.materials.roof_habit()?;
    if element.tags.contains_key("building:part") || !qualifies_for_auto_gabled_roof(building_type)
    {
        return None;
    }
    let footprint_m2 = footprint_size as f64 / (ctx.scale * ctx.scale);
    let height_m = f64::from(building_height) / ctx.scale;
    // A small, low, free-standing `building=yes` is mostly a house.
    let house = is_house_type(building_type)
        || building_type == "terrace"
        || (matches!(building_type, "yes" | "residential")
            && !ctx.attached
            && footprint_m2 <= 220.0
            && height_m <= 11.0);
    let modern = matches!(era, ArchEra::PostWarPanel | ArchEra::Contemporary);
    let mut chance = if footprint_m2 > 1500.0 {
        0.0
    } else if house {
        habit.house
    } else if height_m <= habit.block_max_m {
        if modern {
            habit.block * 0.5
        } else {
            habit.block
        }
    } else {
        0.0
    };
    if ctx.materials != MaterialRegion::Australasia {
        chance *= match climate {
            Climate::HotDesert => 0.35,
            Climate::HotSteppe => 0.7,
            _ => 1.0,
        };
    }
    let mut rng = element_rng(style_seed ^ 0x2E61_0A1F_0F5E_7001);
    if !rng.random_bool(chance) {
        return Some((RoofType::Flat, false));
    }
    // A hip against a party wall would leave a valley into the neighbour.
    let roof = if ctx.attached || building_type == "terrace" || !rng.random_bool(habit.hipped) {
        RoofType::Gabled
    } else {
        RoofType::Hipped
    };
    Some((roof, true))
}

/// Detached house types: the only stock that keeps a pitched roof in a
/// flat-roofed North American city.
fn is_house_type(building_type: &str) -> bool {
    matches!(
        building_type,
        "house"
            | "detached"
            | "semidetached_house"
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

/// Smallest footprint, in cells, that gets an interior.
const MIN_INTERIOR_CELLS: usize = 24;

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

/// Wall blocks suitable for residential buildings outside the regional
/// palettes. No nether, end or blackstone blocks: those read as fantasy, not
/// as a house.
const RESIDENTIAL_WALL_OPTIONS: [Block; 23] = [
    BRICK,
    STONE_BRICKS,
    WHITE_TERRACOTTA,
    BROWN_TERRACOTTA,
    SANDSTONE,
    SMOOTH_SANDSTONE,
    QUARTZ_BRICKS,
    MUD_BRICKS,
    POLISHED_GRANITE,
    BROWN_CONCRETE,
    GRAY_CONCRETE,
    GRAY_TERRACOTTA,
    LIGHT_GRAY_CONCRETE,
    LIGHT_GRAY_TERRACOTTA,
    POLISHED_ANDESITE,
    QUARTZ_BLOCK,
    WHITE_CONCRETE,
    ORANGE_TERRACOTTA,
    RED_TERRACOTTA,
    TERRACOTTA,
    MUSHROOM_STEM,
    CYAN_TERRACOTTA,
    YELLOW_TERRACOTTA,
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
    BRICK,
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
    BRICK,
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
const HISTORIC_WALL_OPTIONS: [Block; 15] = [
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

// ----------------------------------------------------------------------------
// Regional wall palettes, weighted after the visible facades of the building
// stock of each region.
// Weights are relative; a district keeps a few of the entries (see
// `pick_district_wall`).
// ----------------------------------------------------------------------------

type WallPool = &'static [(Block, u8)];

/// North American row houses, tenements and shops before the war: red and
/// buff brick, brownstone, cream stone.
const NA_TRADITIONAL_WALLS: WallPool = &[
    (BRICK, 30),
    (POLISHED_GRANITE, 6),
    (SMOOTH_SANDSTONE, 13),
    (SANDSTONE, 3),
    (BROWN_TERRACOTTA, 7),
    (MUD_BRICKS, 4),
    (WHITE_TERRACOTTA, 5),
    (MUSHROOM_STEM, 6),
    (CALCITE, 2),
    (CYAN_TERRACOTTA, 6),
    (LIGHT_GRAY_CONCRETE_POWDER, 4),
    (WHITE_CONCRETE, 4),
    (LIGHT_GRAY_CONCRETE, 3),
    (SMOOTH_STONE, 2),
];

/// Early skyscrapers, art deco offices and civic buildings: limestone, buff
/// brick and grey-green stone, red brick less often.
const NA_CIVIC_WALLS: WallPool = &[
    (SMOOTH_SANDSTONE, 18),
    (MUSHROOM_STEM, 12),
    (LIGHT_GRAY_CONCRETE_POWDER, 12),
    (CYAN_TERRACOTTA, 12),
    (WHITE_TERRACOTTA, 5),
    (CALCITE, 5),
    (BRICK, 9),
    (POLISHED_GRANITE, 3),
    (SMOOTH_STONE, 6),
    (GRAY_CONCRETE, 5),
    (TUFF, 3),
    (LIGHT_GRAY_CONCRETE, 5),
    (SANDSTONE, 3),
];

/// Post-war and contemporary North American blocks: precast, concrete and
/// the brick of the housing projects.
const NA_MODERN_WALLS: WallPool = &[
    (LIGHT_GRAY_CONCRETE_POWDER, 14),
    (LIGHT_GRAY_CONCRETE, 12),
    (WHITE_CONCRETE, 12),
    (GRAY_CONCRETE, 10),
    (CYAN_TERRACOTTA, 10),
    (BRICK, 12),
    (SMOOTH_STONE, 8),
    (POLISHED_ANDESITE, 6),
    (MUSHROOM_STEM, 4),
    (SMOOTH_SANDSTONE, 4),
    (BLACK_CONCRETE, 3),
    (POLISHED_DEEPSLATE, 2),
];

/// Detached North American houses: painted siding, brick, some stucco.
const NA_HOUSE_WALLS: WallPool = &[
    (WHITE_CONCRETE, 16),
    (BRICK, 12),
    (SMOOTH_SANDSTONE, 9),
    (WHITE_TERRACOTTA, 9),
    (MUSHROOM_STEM, 8),
    (CALCITE, 5),
    (LIGHT_GRAY_CONCRETE, 6),
    (CYAN_TERRACOTTA, 5),
    (POLISHED_GRANITE, 3),
    (BROWN_TERRACOTTA, 3),
    (OAK_PLANKS, 4),
    (SPRUCE_PLANKS, 4),
    (YELLOW_TERRACOTTA, 2),
];

/// Sunbelt houses: stucco in white, beige and peach.
const SUNBELT_HOUSE_WALLS: WallPool = &[
    (WHITE_CONCRETE, 18),
    (SMOOTH_SANDSTONE, 16),
    (WHITE_TERRACOTTA, 16),
    (MUSHROOM_STEM, 10),
    (CALCITE, 6),
    (LIGHT_GRAY_CONCRETE, 5),
    (CYAN_TERRACOTTA, 4),
    (BRICK, 4),
    (YELLOW_TERRACOTTA, 3),
    (TERRACOTTA, 3),
    (OAK_PLANKS, 3),
    (SPRUCE_PLANKS, 3),
];

/// Sunbelt main streets: stucco with some brick.
const SUNBELT_TRADITIONAL_WALLS: WallPool = &[
    (WHITE_CONCRETE, 16),
    (SMOOTH_SANDSTONE, 14),
    (WHITE_TERRACOTTA, 14),
    (MUSHROOM_STEM, 8),
    (CALCITE, 4),
    (BRICK, 10),
    (POLISHED_GRANITE, 3),
    (CYAN_TERRACOTTA, 4),
    (LIGHT_GRAY_CONCRETE, 5),
    (YELLOW_TERRACOTTA, 3),
    (TERRACOTTA, 3),
    (SANDSTONE, 3),
];

/// Central European old towns: pale render in white, salmon, cream, pale
/// ochre and some pink and oxblood, as in Munich and Dresden. Birch reads as
/// pale ochre render, cherry as pastel pink.
const CENTRAL_TRADITIONAL_WALLS: WallPool = &[
    (WHITE_CONCRETE, 22),
    (CALCITE, 8),
    (WHITE_TERRACOTTA, 14),
    (CHERRY_PLANKS, 4),
    (SMOOTH_SANDSTONE, 12),
    (MUSHROOM_STEM, 8),
    (BIRCH_PLANKS, 8),
    (YELLOW_TERRACOTTA, 4),
    (RED_TERRACOTTA, 4),
    (LIGHT_GRAY_CONCRETE_POWDER, 4),
    (SMOOTH_STONE, 2),
    (BRICK, 4),
];

/// Low Countries and the North German coast: red-brown brick with some
/// white render.
const BRICK_NORTH_TRADITIONAL_WALLS: WallPool = &[
    (BRICK, 32),
    (POLISHED_GRANITE, 8),
    (MUD_BRICKS, 5),
    (BROWN_TERRACOTTA, 4),
    (RED_TERRACOTTA, 3),
    (TERRACOTTA, 3),
    (WHITE_CONCRETE, 12),
    (WHITE_TERRACOTTA, 5),
    (SMOOTH_SANDSTONE, 4),
    (MUSHROOM_STEM, 4),
    (LIGHT_GRAY_CONCRETE, 3),
    (CALCITE, 2),
];

/// British terraces: yellow-brown London stock and red brick, white stucco,
/// Bath and Portland stone.
const BRITISH_TRADITIONAL_WALLS: WallPool = &[
    (MUD_BRICKS, 22),
    (BRICK, 20),
    (POLISHED_GRANITE, 4),
    (WHITE_CONCRETE, 12),
    (MUSHROOM_STEM, 6),
    (SMOOTH_SANDSTONE, 5),
    (CALCITE, 3),
    (WHITE_TERRACOTTA, 3),
    (BROWN_TERRACOTTA, 3),
    (LIGHT_GRAY_CONCRETE, 3),
];

/// French towns: cream dressed limestone and pale render.
const FRANCE_TRADITIONAL_WALLS: WallPool = &[
    (MUSHROOM_STEM, 18),
    (SMOOTH_SANDSTONE, 12),
    (CALCITE, 8),
    (WHITE_CONCRETE, 12),
    (WHITE_TERRACOTTA, 7),
    (LIGHT_GRAY_CONCRETE, 4),
    (LIGHT_GRAY_CONCRETE_POWDER, 3),
    (SANDSTONE, 3),
    (BRICK, 4),
    (YELLOW_TERRACOTTA, 2),
];

/// Southern Europe: render in white, peach, cream and ochre.
const MEDITERRANEAN_TRADITIONAL_WALLS: WallPool = &[
    (WHITE_CONCRETE, 16),
    (WHITE_TERRACOTTA, 14),
    (SMOOTH_SANDSTONE, 11),
    (YELLOW_TERRACOTTA, 7),
    (BIRCH_PLANKS, 4),
    (CHERRY_PLANKS, 3),
    (TERRACOTTA, 5),
    (ORANGE_TERRACOTTA, 3),
    (MUSHROOM_STEM, 6),
    (CALCITE, 4),
    (SANDSTONE, 4),
    (BRICK, 4),
    (RED_TERRACOTTA, 3),
];

/// Nordic towns: white and ochre render, brick, falu red.
const NORDIC_TRADITIONAL_WALLS: WallPool = &[
    (WHITE_CONCRETE, 12),
    (BRICK, 14),
    (POLISHED_GRANITE, 3),
    (YELLOW_TERRACOTTA, 6),
    (BIRCH_PLANKS, 4),
    (WHITE_TERRACOTTA, 9),
    (RED_TERRACOTTA, 6),
    (SMOOTH_SANDSTONE, 6),
    (MUSHROOM_STEM, 5),
    (LIGHT_GRAY_CONCRETE, 4),
    (CALCITE, 3),
    (MUD_BRICKS, 3),
];

/// European representative and civic buildings: sandstone, dressed stone,
/// pale render.
const EU_CIVIC_WALLS: WallPool = &[
    (SMOOTH_SANDSTONE, 16),
    (WHITE_CONCRETE, 12),
    (MUSHROOM_STEM, 10),
    (WHITE_TERRACOTTA, 7),
    (CALCITE, 6),
    (BIRCH_PLANKS, 4),
    (LIGHT_GRAY_CONCRETE_POWDER, 6),
    (YELLOW_TERRACOTTA, 4),
    (SANDSTONE, 4),
    (BRICK, 5),
    (QUARTZ_BLOCK, 3),
];

/// Post-war and contemporary European blocks: render and concrete.
const EU_MODERN_WALLS: WallPool = &[
    (WHITE_CONCRETE, 18),
    (LIGHT_GRAY_CONCRETE_POWDER, 12),
    (SMOOTH_STONE, 8),
    (LIGHT_GRAY_CONCRETE, 6),
    (CYAN_TERRACOTTA, 5),
    (GRAY_CONCRETE, 3),
    (WHITE_TERRACOTTA, 5),
    (CALCITE, 5),
    (MUSHROOM_STEM, 5),
    (BIRCH_PLANKS, 3),
    (BRICK, 4),
    (QUARTZ_BLOCK, 2),
    (YELLOW_TERRACOTTA, 2),
];

/// Post-war blocks where brick stayed the norm (Britain, the Low Countries,
/// Scandinavia).
const EU_BRICK_MODERN_WALLS: WallPool = &[
    (BRICK, 22),
    (POLISHED_GRANITE, 5),
    (MUD_BRICKS, 6),
    (BROWN_TERRACOTTA, 3),
    (LIGHT_GRAY_CONCRETE, 12),
    (WHITE_CONCRETE, 12),
    (GRAY_CONCRETE, 6),
    (SMOOTH_STONE, 5),
    (LIGHT_GRAY_CONCRETE_POWDER, 4),
    (CALCITE, 3),
];

/// Canadian houses: brick and siding in white, grey and slate blue.
const CANADA_HOUSE_WALLS: WallPool = &[
    (BRICK, 16),
    (WHITE_CONCRETE, 14),
    (LIGHT_GRAY_CONCRETE, 8),
    (CYAN_TERRACOTTA, 6),
    (SPRUCE_PLANKS, 6),
    (OAK_PLANKS, 5),
    (MUSHROOM_STEM, 7),
    (SMOOTH_SANDSTONE, 6),
    (WHITE_TERRACOTTA, 6),
    (BROWN_TERRACOTTA, 5),
    (POLISHED_GRANITE, 4),
    (MUD_BRICKS, 4),
    (CALCITE, 4),
    (DARK_OAK_PLANKS, 2),
];

/// Toronto's bay-and-gables and Montreal's plexes: red and buff brick,
/// greystone.
const CANADA_TRADITIONAL_WALLS: WallPool = &[
    (BRICK, 26),
    (POLISHED_GRANITE, 6),
    (MUD_BRICKS, 6),
    (LIGHT_GRAY_CONCRETE_POWDER, 8),
    (SMOOTH_STONE, 6),
    (STONE_BRICKS, 4),
    (SMOOTH_SANDSTONE, 6),
    (WHITE_CONCRETE, 6),
    (BROWN_TERRACOTTA, 5),
    (MUSHROOM_STEM, 4),
    (CYAN_TERRACOTTA, 3),
    (WHITE_TERRACOTTA, 3),
];

/// Russian and Ukrainian houses: log and plank, red and white silicate
/// brick, whitewashed render.
const EAST_HOUSE_WALLS: WallPool = &[
    (SPRUCE_PLANKS, 10),
    (DARK_OAK_PLANKS, 6),
    (OAK_PLANKS, 4),
    (BIRCH_PLANKS, 4),
    (BRICK, 14),
    (WHITE_CONCRETE, 12),
    (CALCITE, 6),
    (LIGHT_GRAY_CONCRETE, 6),
    (SMOOTH_SANDSTONE, 6),
    (YELLOW_TERRACOTTA, 4),
    (MUSHROOM_STEM, 6),
    (WHITE_TERRACOTTA, 4),
];

/// Pre-revolutionary town fabric: ochre, cream, pink and white render, red
/// brick.
const EAST_TRADITIONAL_WALLS: WallPool = &[
    (YELLOW_TERRACOTTA, 10),
    (BIRCH_PLANKS, 8),
    (WHITE_CONCRETE, 14),
    (WHITE_TERRACOTTA, 10),
    (CHERRY_PLANKS, 4),
    (SMOOTH_SANDSTONE, 8),
    (BRICK, 12),
    (RED_TERRACOTTA, 4),
    (CYAN_TERRACOTTA, 3),
    (MUSHROOM_STEM, 6),
    (LIGHT_GRAY_CONCRETE_POWDER, 4),
    (CALCITE, 4),
];

/// Stalinist and imperial institutions: cream and grey stone, render.
const EAST_CIVIC_WALLS: WallPool = &[
    (SMOOTH_SANDSTONE, 14),
    (MUSHROOM_STEM, 12),
    (WHITE_TERRACOTTA, 8),
    (YELLOW_TERRACOTTA, 6),
    (WHITE_CONCRETE, 10),
    (LIGHT_GRAY_CONCRETE_POWDER, 8),
    (BRICK, 8),
    (POLISHED_GRANITE, 4),
    (CALCITE, 6),
    (SANDSTONE, 4),
    (RED_TERRACOTTA, 3),
    (SMOOTH_STONE, 4),
];

/// Soviet and post-Soviet blocks: grey and white panels, silicate and red
/// brick.
const EAST_MODERN_WALLS: WallPool = &[
    (LIGHT_GRAY_CONCRETE, 18),
    (WHITE_CONCRETE, 14),
    (SMOOTH_STONE, 10),
    (MUSHROOM_STEM, 8),
    (CALCITE, 8),
    (BRICK, 10),
    (LIGHT_GRAY_CONCRETE_POWDER, 8),
    (GRAY_CONCRETE, 6),
    (WHITE_TERRACOTTA, 5),
    (YELLOW_TERRACOTTA, 3),
    (CYAN_TERRACOTTA, 3),
];

/// Caucasus and Central Asian houses: render, adobe, brick, tuff.
const CENTRAL_ASIA_HOUSE_WALLS: WallPool = &[
    (WHITE_CONCRETE, 14),
    (SMOOTH_SANDSTONE, 12),
    (MUD_BRICKS, 8),
    (WHITE_TERRACOTTA, 10),
    (BRICK, 10),
    (YELLOW_TERRACOTTA, 5),
    (TERRACOTTA, 5),
    (LIGHT_GRAY_CONCRETE, 8),
    (MUSHROOM_STEM, 6),
    (SANDSTONE, 6),
    (CALCITE, 4),
];

/// Baku limestone, Yerevan's pink and red tuff, Tbilisi brick.
const CENTRAL_ASIA_TRADITIONAL_WALLS: WallPool = &[
    (SMOOTH_SANDSTONE, 14),
    (WHITE_TERRACOTTA, 12),
    (TERRACOTTA, 6),
    (SANDSTONE, 8),
    (WHITE_CONCRETE, 12),
    (BRICK, 10),
    (MUSHROOM_STEM, 8),
    (YELLOW_TERRACOTTA, 5),
    (LIGHT_GRAY_CONCRETE, 6),
    (CALCITE, 4),
    (MUD_BRICKS, 4),
];

/// North Africa and the Middle East: sand and cream render and stone,
/// whitewash, the red brick of unfinished upper floors.
const MIDDLE_EAST_TRADITIONAL_WALLS: WallPool = &[
    (SMOOTH_SANDSTONE, 20),
    (SANDSTONE, 10),
    (WHITE_CONCRETE, 14),
    (MUSHROOM_STEM, 10),
    (WHITE_TERRACOTTA, 10),
    (CALCITE, 8),
    (YELLOW_TERRACOTTA, 5),
    (TERRACOTTA, 5),
    (BRICK, 4),
    (LIGHT_GRAY_CONCRETE, 6),
    (MUD_BRICKS, 4),
];

/// Limestone and sandstone public buildings.
const MIDDLE_EAST_CIVIC_WALLS: WallPool = &[
    (SMOOTH_SANDSTONE, 20),
    (SANDSTONE, 12),
    (CALCITE, 10),
    (WHITE_CONCRETE, 12),
    (MUSHROOM_STEM, 8),
    (QUARTZ_BLOCK, 4),
    (YELLOW_TERRACOTTA, 4),
    (WHITE_TERRACOTTA, 6),
];

/// Gulf and post-war blocks: white and beige render and stone cladding.
const MIDDLE_EAST_MODERN_WALLS: WallPool = &[
    (WHITE_CONCRETE, 18),
    (SMOOTH_SANDSTONE, 16),
    (CALCITE, 10),
    (MUSHROOM_STEM, 10),
    (QUARTZ_BLOCK, 6),
    (LIGHT_GRAY_CONCRETE, 10),
    (SANDSTONE, 6),
    (WHITE_TERRACOTTA, 8),
    (SMOOTH_STONE, 4),
];

/// African houses: render in white, cream and ochre, laterite red, face
/// brick, bare block.
const AFRICA_HOUSE_WALLS: WallPool = &[
    (WHITE_CONCRETE, 16),
    (SMOOTH_SANDSTONE, 12),
    (WHITE_TERRACOTTA, 10),
    (YELLOW_TERRACOTTA, 8),
    (TERRACOTTA, 8),
    (BRICK, 10),
    (MUD_BRICKS, 6),
    (LIGHT_GRAY_CONCRETE, 10),
    (MUSHROOM_STEM, 6),
    (PINK_TERRACOTTA, 3),
    (LIME_TERRACOTTA, 3),
];

/// African town centres: painted render over block and concrete.
const AFRICA_TRADITIONAL_WALLS: WallPool = &[
    (WHITE_CONCRETE, 16),
    (LIGHT_GRAY_CONCRETE, 12),
    (SMOOTH_SANDSTONE, 10),
    (WHITE_TERRACOTTA, 8),
    (YELLOW_TERRACOTTA, 8),
    (MUSHROOM_STEM, 8),
    (TERRACOTTA, 6),
    (BRICK, 6),
    (CYAN_TERRACOTTA, 4),
    (SANDSTONE, 4),
    (PINK_TERRACOTTA, 3),
    (LIME_TERRACOTTA, 3),
];

/// Colonial-era public buildings: grey and cream dressed stone, render.
const AFRICA_CIVIC_WALLS: WallPool = &[
    (SMOOTH_SANDSTONE, 14),
    (WHITE_CONCRETE, 14),
    (SMOOTH_STONE, 10),
    (STONE_BRICKS, 8),
    (MUSHROOM_STEM, 8),
    (SANDSTONE, 6),
    (CALCITE, 6),
    (BRICK, 6),
    (LIGHT_GRAY_CONCRETE, 6),
];

/// Post-war African blocks: white and grey concrete and render.
const AFRICA_MODERN_WALLS: WallPool = &[
    (WHITE_CONCRETE, 20),
    (LIGHT_GRAY_CONCRETE, 14),
    (SMOOTH_SANDSTONE, 10),
    (MUSHROOM_STEM, 10),
    (SMOOTH_STONE, 8),
    (CALCITE, 6),
    (GRAY_CONCRETE, 6),
    (WHITE_TERRACOTTA, 6),
    (YELLOW_TERRACOTTA, 4),
    (BRICK, 4),
    (QUARTZ_BLOCK, 3),
];

/// Indian houses: painted plaster in white, cream, pink, yellow and green,
/// exposed brick, bare concrete.
const SOUTH_ASIA_HOUSE_WALLS: WallPool = &[
    (WHITE_CONCRETE, 16),
    (SMOOTH_SANDSTONE, 10),
    (WHITE_TERRACOTTA, 10),
    (PINK_TERRACOTTA, 6),
    (YELLOW_TERRACOTTA, 6),
    (LIME_TERRACOTTA, 4),
    (BRICK, 10),
    (TERRACOTTA, 4),
    (LIGHT_GRAY_CONCRETE, 10),
    (MUSHROOM_STEM, 6),
    (CALCITE, 4),
];

/// Dense South Asian streets: monsoon-stained render and concrete, paint,
/// brick.
const SOUTH_ASIA_TRADITIONAL_WALLS: WallPool = &[
    (WHITE_CONCRETE, 12),
    (LIGHT_GRAY_CONCRETE, 12),
    (CYAN_TERRACOTTA, 8),
    (SMOOTH_SANDSTONE, 10),
    (WHITE_TERRACOTTA, 8),
    (YELLOW_TERRACOTTA, 6),
    (PINK_TERRACOTTA, 4),
    (LIME_TERRACOTTA, 3),
    (BRICK, 8),
    (MUSHROOM_STEM, 8),
    (GRAY_CONCRETE, 5),
    (SANDSTONE, 4),
];

/// Colonial and Mughal-revival public buildings: buff and red sandstone,
/// grey basalt, white render.
const SOUTH_ASIA_CIVIC_WALLS: WallPool = &[
    (SANDSTONE, 14),
    (SMOOTH_SANDSTONE, 12),
    (TERRACOTTA, 8),
    (WHITE_CONCRETE, 12),
    (MUSHROOM_STEM, 8),
    (TUFF, 6),
    (BRICK, 8),
    (CALCITE, 4),
    (LIGHT_GRAY_CONCRETE, 6),
];

/// Post-war South Asian blocks: white and grey concrete, stained.
const SOUTH_ASIA_MODERN_WALLS: WallPool = &[
    (WHITE_CONCRETE, 20),
    (LIGHT_GRAY_CONCRETE, 14),
    (SMOOTH_SANDSTONE, 10),
    (MUSHROOM_STEM, 10),
    (CYAN_TERRACOTTA, 6),
    (WHITE_TERRACOTTA, 6),
    (SMOOTH_STONE, 6),
    (GRAY_CONCRETE, 5),
    (CALCITE, 5),
    (YELLOW_TERRACOTTA, 3),
    (QUARTZ_BLOCK, 3),
];

/// Southeast Asian houses: painted render and timber.
const SOUTHEAST_ASIA_HOUSE_WALLS: WallPool = &[
    (WHITE_CONCRETE, 18),
    (SMOOTH_SANDSTONE, 12),
    (WHITE_TERRACOTTA, 10),
    (YELLOW_TERRACOTTA, 6),
    (PINK_TERRACOTTA, 5),
    (LIME_TERRACOTTA, 4),
    (LIGHT_GRAY_CONCRETE, 10),
    (MUSHROOM_STEM, 8),
    (BIRCH_PLANKS, 6),
    (OAK_PLANKS, 4),
    (TERRACOTTA, 4),
];

/// Shophouse rows and tube houses: pastel render, mould-stained concrete.
const SOUTHEAST_ASIA_TRADITIONAL_WALLS: WallPool = &[
    (WHITE_CONCRETE, 16),
    (SMOOTH_SANDSTONE, 10),
    (WHITE_TERRACOTTA, 10),
    (YELLOW_TERRACOTTA, 8),
    (PINK_TERRACOTTA, 5),
    (LIME_TERRACOTTA, 4),
    (LIGHT_GRAY_CONCRETE, 10),
    (MUSHROOM_STEM, 8),
    (CYAN_TERRACOTTA, 5),
    (GRAY_CONCRETE, 4),
    (CALCITE, 4),
];

/// Tropical post-war and contemporary blocks: white and pale concrete.
const SOUTHEAST_ASIA_MODERN_WALLS: WallPool = &[
    (WHITE_CONCRETE, 22),
    (LIGHT_GRAY_CONCRETE, 14),
    (MUSHROOM_STEM, 10),
    (SMOOTH_SANDSTONE, 8),
    (CALCITE, 8),
    (SMOOTH_STONE, 8),
    (WHITE_TERRACOTTA, 6),
    (GRAY_CONCRETE, 5),
    (QUARTZ_BLOCK, 4),
    (CYAN_TERRACOTTA, 3),
    (YELLOW_TERRACOTTA, 3),
];

/// Chinese and Taiwanese houses: white tile and render, red and grey brick.
const EAST_ASIA_HOUSE_WALLS: WallPool = &[
    (WHITE_CONCRETE, 18),
    (BRICK, 14),
    (STONE_BRICKS, 10),
    (LIGHT_GRAY_CONCRETE, 10),
    (MUSHROOM_STEM, 8),
    (WHITE_TERRACOTTA, 8),
    (SMOOTH_SANDSTONE, 6),
    (CALCITE, 6),
    (TERRACOTTA, 4),
    (POLISHED_ANDESITE, 4),
];

/// Chinese lanes and walk-ups: tile-clad and rendered concrete, grey and red
/// brick, city grime.
const EAST_ASIA_TRADITIONAL_WALLS: WallPool = &[
    (WHITE_CONCRETE, 16),
    (LIGHT_GRAY_CONCRETE, 14),
    (STONE_BRICKS, 8),
    (BRICK, 8),
    (MUSHROOM_STEM, 10),
    (WHITE_TERRACOTTA, 10),
    (SMOOTH_SANDSTONE, 8),
    (SMOOTH_STONE, 6),
    (GRAY_CONCRETE, 4),
    (CYAN_TERRACOTTA, 4),
    (CALCITE, 4),
];

/// East Asian apartment slabs and towers: white and pale tile and paint.
const EAST_ASIA_MODERN_WALLS: WallPool = &[
    (WHITE_CONCRETE, 24),
    (LIGHT_GRAY_CONCRETE, 14),
    (MUSHROOM_STEM, 12),
    (SMOOTH_STONE, 8),
    (CALCITE, 8),
    (SMOOTH_SANDSTONE, 6),
    (WHITE_TERRACOTTA, 6),
    (QUARTZ_BLOCK, 4),
    (GRAY_CONCRETE, 4),
    (BROWN_TERRACOTTA, 4),
];

/// Korean houses: red and brown brick, white render.
const KOREA_HOUSE_WALLS: WallPool = &[
    (BRICK, 20),
    (MUD_BRICKS, 8),
    (WHITE_CONCRETE, 16),
    (MUSHROOM_STEM, 10),
    (LIGHT_GRAY_CONCRETE, 10),
    (SMOOTH_SANDSTONE, 8),
    (WHITE_TERRACOTTA, 8),
    (BROWN_TERRACOTTA, 6),
    (CALCITE, 4),
];

/// Korean multi-family villas and shops: red-brown brick, granite, render.
const KOREA_TRADITIONAL_WALLS: WallPool = &[
    (BRICK, 18),
    (MUD_BRICKS, 8),
    (POLISHED_GRANITE, 4),
    (WHITE_CONCRETE, 16),
    (LIGHT_GRAY_CONCRETE, 12),
    (MUSHROOM_STEM, 10),
    (SMOOTH_STONE, 6),
    (WHITE_TERRACOTTA, 6),
    (BROWN_TERRACOTTA, 6),
    (GRAY_CONCRETE, 4),
];

/// Japanese houses: siding and mortar in white, beige and grey, dark brown
/// boards, old timber.
const JAPAN_HOUSE_WALLS: WallPool = &[
    (WHITE_CONCRETE, 14),
    (MUSHROOM_STEM, 14),
    (SMOOTH_SANDSTONE, 10),
    (CALCITE, 8),
    (LIGHT_GRAY_CONCRETE, 10),
    (LIGHT_GRAY_CONCRETE_POWDER, 8),
    (WHITE_TERRACOTTA, 8),
    (BIRCH_PLANKS, 5),
    (BROWN_TERRACOTTA, 6),
    (SPRUCE_PLANKS, 4),
    (DARK_OAK_PLANKS, 3),
    (CYAN_TERRACOTTA, 3),
    (SMOOTH_STONE, 4),
];

/// Japanese mid-rises: white, beige, grey and brown tile cladding.
const JAPAN_URBAN_WALLS: WallPool = &[
    (WHITE_CONCRETE, 16),
    (LIGHT_GRAY_CONCRETE, 14),
    (MUSHROOM_STEM, 12),
    (SMOOTH_STONE, 8),
    (WHITE_TERRACOTTA, 8),
    (BROWN_TERRACOTTA, 8),
    (SMOOTH_SANDSTONE, 8),
    (CALCITE, 6),
    (TERRACOTTA, 4),
    (BRICK, 4),
    (GRAY_CONCRETE, 4),
    (QUARTZ_BLOCK, 3),
];

/// Latin American houses: brightly painted render, exposed brick, bare
/// block.
const LATAM_HOUSE_WALLS: WallPool = &[
    (WHITE_CONCRETE, 14),
    (YELLOW_TERRACOTTA, 8),
    (ORANGE_TERRACOTTA, 6),
    (PINK_TERRACOTTA, 7),
    (WHITE_TERRACOTTA, 10),
    (SMOOTH_SANDSTONE, 10),
    (LIME_TERRACOTTA, 4),
    (TERRACOTTA, 6),
    (BRICK, 10),
    (LIGHT_GRAY_CONCRETE, 8),
    (RED_TERRACOTTA, 3),
    (MUSHROOM_STEM, 4),
];

/// Colonial and early twentieth-century streets: painted render, cream
/// cantera stone, red tezontle.
const LATAM_TRADITIONAL_WALLS: WallPool = &[
    (WHITE_CONCRETE, 12),
    (SMOOTH_SANDSTONE, 12),
    (WHITE_TERRACOTTA, 10),
    (YELLOW_TERRACOTTA, 8),
    (PINK_TERRACOTTA, 6),
    (ORANGE_TERRACOTTA, 4),
    (TERRACOTTA, 6),
    (MUSHROOM_STEM, 8),
    (LIGHT_GRAY_CONCRETE, 8),
    (BRICK, 6),
    (RED_TERRACOTTA, 4),
    (LIME_TERRACOTTA, 3),
    (CALCITE, 4),
];

/// Churches, palaces and public buildings: cantera, tezontle, render.
const LATAM_CIVIC_WALLS: WallPool = &[
    (SMOOTH_SANDSTONE, 16),
    (SANDSTONE, 6),
    (WHITE_CONCRETE, 12),
    (MUSHROOM_STEM, 10),
    (CALCITE, 6),
    (RED_TERRACOTTA, 6),
    (YELLOW_TERRACOTTA, 6),
    (WHITE_TERRACOTTA, 8),
    (LIGHT_GRAY_CONCRETE, 6),
    (TUFF, 4),
];

/// Post-war Latin American blocks: concrete and render, Bogotá brick.
const LATAM_MODERN_WALLS: WallPool = &[
    (WHITE_CONCRETE, 18),
    (LIGHT_GRAY_CONCRETE, 14),
    (SMOOTH_STONE, 8),
    (MUSHROOM_STEM, 10),
    (SMOOTH_SANDSTONE, 8),
    (WHITE_TERRACOTTA, 6),
    (BRICK, 8),
    (GRAY_CONCRETE, 6),
    (CALCITE, 5),
    (YELLOW_TERRACOTTA, 3),
    (TERRACOTTA, 4),
    (QUARTZ_BLOCK, 3),
];

/// Argentine, Chilean and Uruguayan houses: render and face brick.
const SOUTHERN_CONE_HOUSE_WALLS: WallPool = &[
    (WHITE_CONCRETE, 16),
    (BRICK, 16),
    (MUSHROOM_STEM, 10),
    (SMOOTH_SANDSTONE, 10),
    (WHITE_TERRACOTTA, 10),
    (LIGHT_GRAY_CONCRETE, 8),
    (YELLOW_TERRACOTTA, 5),
    (TERRACOTTA, 5),
    (CALCITE, 5),
    (MUD_BRICKS, 4),
    (PINK_TERRACOTTA, 3),
];

/// Buenos Aires and Montevideo: Italianate and French render in stone
/// tones, brick.
const SOUTHERN_CONE_TRADITIONAL_WALLS: WallPool = &[
    (MUSHROOM_STEM, 16),
    (SMOOTH_SANDSTONE, 12),
    (WHITE_CONCRETE, 12),
    (LIGHT_GRAY_CONCRETE_POWDER, 8),
    (WHITE_TERRACOTTA, 10),
    (CALCITE, 6),
    (BRICK, 8),
    (YELLOW_TERRACOTTA, 4),
    (LIGHT_GRAY_CONCRETE, 6),
    (PINK_TERRACOTTA, 3),
    (SMOOTH_STONE, 4),
];

/// Australian and New Zealand houses: red, orange, cream and brown brick
/// veneer, painted weatherboard, render.
const AUSTRALASIA_HOUSE_WALLS: WallPool = &[
    (BRICK, 18),
    (TERRACOTTA, 8),
    (MUD_BRICKS, 6),
    (SMOOTH_SANDSTONE, 8),
    (WHITE_CONCRETE, 14),
    (MUSHROOM_STEM, 8),
    (BIRCH_PLANKS, 6),
    (CALCITE, 5),
    (LIGHT_GRAY_CONCRETE, 6),
    (WHITE_TERRACOTTA, 5),
    (OAK_PLANKS, 4),
    (BROWN_TERRACOTTA, 3),
    (CYAN_TERRACOTTA, 2),
];

/// Victorian terraces and inner suburbs: brick, sandstone, stucco.
const AUSTRALASIA_TRADITIONAL_WALLS: WallPool = &[
    (BRICK, 16),
    (SANDSTONE, 8),
    (SMOOTH_SANDSTONE, 10),
    (WHITE_CONCRETE, 12),
    (MUSHROOM_STEM, 8),
    (WHITE_TERRACOTTA, 8),
    (MUD_BRICKS, 6),
    (TERRACOTTA, 5),
    (CALCITE, 4),
    (YELLOW_TERRACOTTA, 3),
    (LIGHT_GRAY_CONCRETE, 5),
];

/// Sydney sandstone, Melbourne bluestone, brick civic halls.
const AUSTRALASIA_CIVIC_WALLS: WallPool = &[
    (SANDSTONE, 14),
    (SMOOTH_SANDSTONE, 14),
    (BRICK, 10),
    (WHITE_CONCRETE, 8),
    (MUSHROOM_STEM, 8),
    (CALCITE, 4),
    (TUFF, 4),
    (LIGHT_GRAY_CONCRETE, 6),
];

/// Post-war Australasian flats and offices: brick walk-ups, concrete,
/// render.
const AUSTRALASIA_MODERN_WALLS: WallPool = &[
    (WHITE_CONCRETE, 16),
    (LIGHT_GRAY_CONCRETE, 14),
    (BRICK, 12),
    (SMOOTH_STONE, 8),
    (MUSHROOM_STEM, 8),
    (CALCITE, 6),
    (GRAY_CONCRETE, 6),
    (SMOOTH_SANDSTONE, 6),
    (MUD_BRICKS, 4),
    (TERRACOTTA, 4),
    (QUARTZ_BLOCK, 3),
];

/// Every regional pool, for the export-coverage test.
#[cfg(test)]
const REGIONAL_WALL_POOLS: [WallPool; 54] = [
    NA_TRADITIONAL_WALLS,
    NA_CIVIC_WALLS,
    NA_MODERN_WALLS,
    NA_HOUSE_WALLS,
    SUNBELT_HOUSE_WALLS,
    SUNBELT_TRADITIONAL_WALLS,
    CENTRAL_TRADITIONAL_WALLS,
    BRICK_NORTH_TRADITIONAL_WALLS,
    BRITISH_TRADITIONAL_WALLS,
    FRANCE_TRADITIONAL_WALLS,
    MEDITERRANEAN_TRADITIONAL_WALLS,
    NORDIC_TRADITIONAL_WALLS,
    EU_CIVIC_WALLS,
    EU_MODERN_WALLS,
    EU_BRICK_MODERN_WALLS,
    CANADA_HOUSE_WALLS,
    CANADA_TRADITIONAL_WALLS,
    EAST_HOUSE_WALLS,
    EAST_TRADITIONAL_WALLS,
    EAST_CIVIC_WALLS,
    EAST_MODERN_WALLS,
    CENTRAL_ASIA_HOUSE_WALLS,
    CENTRAL_ASIA_TRADITIONAL_WALLS,
    MIDDLE_EAST_TRADITIONAL_WALLS,
    MIDDLE_EAST_CIVIC_WALLS,
    MIDDLE_EAST_MODERN_WALLS,
    AFRICA_HOUSE_WALLS,
    AFRICA_TRADITIONAL_WALLS,
    AFRICA_CIVIC_WALLS,
    AFRICA_MODERN_WALLS,
    SOUTH_ASIA_HOUSE_WALLS,
    SOUTH_ASIA_TRADITIONAL_WALLS,
    SOUTH_ASIA_CIVIC_WALLS,
    SOUTH_ASIA_MODERN_WALLS,
    SOUTHEAST_ASIA_HOUSE_WALLS,
    SOUTHEAST_ASIA_TRADITIONAL_WALLS,
    SOUTHEAST_ASIA_MODERN_WALLS,
    EAST_ASIA_HOUSE_WALLS,
    EAST_ASIA_TRADITIONAL_WALLS,
    EAST_ASIA_MODERN_WALLS,
    KOREA_HOUSE_WALLS,
    KOREA_TRADITIONAL_WALLS,
    JAPAN_HOUSE_WALLS,
    JAPAN_URBAN_WALLS,
    LATAM_HOUSE_WALLS,
    LATAM_TRADITIONAL_WALLS,
    LATAM_CIVIC_WALLS,
    LATAM_MODERN_WALLS,
    SOUTHERN_CONE_HOUSE_WALLS,
    SOUTHERN_CONE_TRADITIONAL_WALLS,
    AUSTRALASIA_HOUSE_WALLS,
    AUSTRALASIA_TRADITIONAL_WALLS,
    AUSTRALASIA_CIVIC_WALLS,
    AUSTRALASIA_MODERN_WALLS,
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
            use_accent_lines: Some(false), // Residential buildings rarely have accent lines
            wall_depth_style: Some(WallDepthStyle::SubtlePilasters),
            ..Default::default()
        }
    }

    /// Preset for tall buildings (>7 floors, not true skyscrapers)
    pub fn tall_building() -> Self {
        Self {
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
            has_windows: Some(true),
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
            has_windows: Some(true),
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

    /// Preset for historic buildings (castles, etc.). The roof follows the
    /// building type: castles and forts stay flat, a historic house is pitched.
    pub fn historic() -> Self {
        Self {
            roof_type: None,
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
            wall_depth_style: Some(WallDepthStyle::IndustrialBeams),
            ..Default::default()
        }
    }

    /// Preset for schools and educational buildings
    pub fn school() -> Self {
        Self {
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
        roof_ctx: RoofContext,
        rng: &mut impl Rng,
    ) -> Self {
        // === Block Palette ===

        // Priority: OSM tag > preset > category palette. A photographed colour
        // enters later, in `resolve`, because it belongs to the building rather
        // than to its category and the facade store is what knows it.
        let wall_block = determine_wall_block_from_tags(element, category, rng)
            .or(preset.wall_block)
            .unwrap_or_else(|| {
                determine_wall_block(
                    element,
                    category,
                    era,
                    climate,
                    building_height,
                    &roof_ctx,
                    rng,
                )
            });

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
                accent_for_wall(wall_block, rng)
            }
        });

        // Dark walls and dark accent bands take dark glass. Church glass keeps
        // its colour, and a preset's glass is part of its design.
        let window_block =
            if preset.window_block.is_none() && category != BuildingCategory::Religious {
                coordinated_window_block(wall_block, accent_block, window_block)
            } else {
                window_block
            };

        // === Window Style ===

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
        } else if roof_ctx.enclosed {
            (RoofType::Flat, false)
        } else if let Some(choice) = european_default_roof(
            element,
            building_type,
            era,
            building_height,
            footprint_size,
            style_seed,
            roof_ctx,
        ) {
            choice
        } else if let Some(choice) = regional_default_roof(
            element,
            building_type,
            era,
            climate,
            building_height,
            footprint_size,
            style_seed,
            roof_ctx,
        ) {
            choice
        } else if qualifies_for_auto_gabled_roof(building_type) {
            const MAX_FOOTPRINT_FOR_GABLED: usize = 800;
            // Own stream: the draw count here depends on footprint and would
            // otherwise desync sibling parts.
            let mut roof_rng = element_rng(style_seed ^ 0x0F1E_2D3C_4B5A_6907);
            let big_block = footprint_size > MAX_FOOTPRINT_FOR_GABLED || building_height >= 15;
            // Above about four storeys, in a row, or beyond house size, North
            // American buildings are flat-roofed; detached houses keep a pitch.
            let scale_sq = roof_ctx.scale * roof_ctx.scale;
            let american_flat = roof_ctx.region == BuildingRegion::NorthAmerica
                && (roof_ctx.attached
                    || f64::from(building_height) >= 13.0 * roof_ctx.scale
                    || (!is_house_type(building_type) && footprint_size as f64 > 300.0 * scale_sq));
            if american_flat {
                (RoofType::Flat, false)
            } else if building_type == "apartments" && big_block {
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
                    // A hip against a party wall leaves a valley into the
                    // neighbour; a row of blocks shares one gabled roof line.
                    if roof_ctx.attached {
                        (RoofType::Gabled, true)
                    } else {
                        (RoofType::Hipped, true)
                    }
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
        } else if matches!(building_type, "church" | "cathedral" | "chapel")
            && (footprint_size as f64) <= 6000.0 * roof_ctx.scale * roof_ctx.scale
        {
            // A church nave is pitched; generate_building_roof makes it steep.
            (RoofType::Gabled, true)
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

        // Every flat roof but a part's runs the roof pass, which lays its
        // membrane, parapet and plant. A part's top is often the floor of the
        // part above, so a part keeps its plain lid unless a preset asks.
        let generate_roof = generate_roof
            || (roof_type == RoofType::Flat
                && !element.tags.contains_key("building:part")
                && !roof_ctx.enclosed);

        // Chimney: only for residential with gabled/hipped roofs
        // Houses in warm and East Asian regions are not heated by open fires
        // and carry no chimney stack, whatever the house preset says.
        let heated = !matches!(
            roof_ctx.materials,
            MaterialRegion::Japan
                | MaterialRegion::Korea
                | MaterialRegion::EastAsia
                | MaterialRegion::SoutheastAsia
                | MaterialRegion::SouthAsia
                | MaterialRegion::MiddleEast
                | MaterialRegion::SubSaharanAfrica
                | MaterialRegion::LatinAmerica
        );
        let has_chimney = heated
            && preset.has_chimney.unwrap_or_else(|| {
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

        // Roof covering: the preset's, else a regional family for pitched
        // roofs. Mapped roof:material / roof:colour still win in generate_roof.
        let roof_block = preset.roof_block.or_else(|| {
            (generate_roof && roof_type != RoofType::Flat).then(|| {
                pitched_roof_block(category, era, roof_type, wall_block, roof_ctx, style_seed)
            })
        });

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

        let style = Self {
            wall_block,
            floor_block,
            window_block,
            accent_block,
            roof_block,
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
        };

        // A building with facade textures takes the lab's whole-building
        // colour as its wall block, at every scale, and keeps everything else
        // it resolved. The flattening a texture needs, so that it is not
        // fighting procedural windows and pilasters, is applied per column
        // instead (`BuildingConfig::is_photo_column`), because a building
        // gets a facade as soon as one of its walls was photographed: on the
        // Munich test box 105 of the 310 ring segments of the 42 buildings
        // with a facade carry a photograph, so flattening the whole shell
        // turned the other 205 into blank single colour walls where a normal
        // Arnis building has windows and doors.
        if crate::mapillary::facades::has_building(element.id) {
            let base = crate::mapillary::facades::building_colour(element.id)
                .map(|rgb| crate::block_palette::wall_block_for_color(rgb, rng))
                .unwrap_or(style.wall_block);
            return style.with_wall_block(base);
        }
        style
    }

    /// The same style with only its wall block swapped: windows, doors,
    /// accents, wall depth and roof stay as resolved, so a building with
    /// facade data still reads as the procedural one, in the lab's colour,
    /// wherever no photograph covers it.
    fn with_wall_block(mut self, wall_block: Block) -> Self {
        self.wall_block = wall_block;
        self
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
    condition: BuildingCondition,
    element_id: u64,
    // shared across all parts of one building for coherent roof style
    style_seed: u64,
    /// Per-building offset of the lattice rhythm, used where no wall run was
    /// laid out (setback tiers), so those don't align citywide.
    window_phase: i32,
    /// Per-building window type.
    window_archetype: WindowArchetype,
    /// Window and pier widths the wall runs are laid out with.
    bays: BayRhythm,
    /// Balcony layout on street facades.
    balcony_band: BalconyBand,
    /// Banded stone ground floor on masonry-era urban buildings.
    rustication: bool,
    /// Darker plinth block for the bottom wall rows, None to skip.
    base_course_block: Option<Block>,
    /// What the street-facing ground floor is.
    ground: GroundFloor,
    /// Light stone for lintels, sills, string courses and the cornice.
    trim_block: Block,
    /// Flush trim-stone frames around the windows of historic European
    /// stucco (Faschen).
    surrounds: bool,
    /// Dark frame of a shopfront or lobby: its piers and sign band.
    shop_frame: Block,
    /// Glass sits a block behind the wall face. Off when interiors are
    /// furnished, since the furniture plans against the wall plane.
    recess_windows: bool,
    /// Building tradition of the area, for habits like shutters.
    region: BuildingRegion,
    /// Per-building window dressing style, derived from hand-built reference frames.
    window_frame: Option<WindowFrameStyle>,
    /// The wall columns a facade photograph covers, built as a plain shell
    /// for the texture to override (see `photo_shell_columns`). Empty on a
    /// building with no facade, and below two blocks per metre without photo
    /// panels, where the export supplies colours only and the building keeps
    /// every procedural feature.
    facade_shell: Arc<FnvHashSet<(i32, i32)>>,
    /// The wall columns a preset facade photograph covers, flattened for the
    /// same reason a Mapillary-photographed column is
    /// (`building_facades::collect`). Per column and not per run: the
    /// presets hang nothing on a courtyard ring inside a building, on a ring
    /// too short or too low to carry a panel, or on a wall the Mapillary store
    /// already owns, and a column flattened for a photograph that never comes
    /// loses its window, its plinth and its accent line for nothing.
    preset_shell: Arc<FnvHashSet<(i32, i32)>>,
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
    /// Trapdoor shutters beside each window, on piers wide enough to hold them.
    fn shutter_block(self) -> Option<Block> {
        match self {
            Self::SpruceCottage => Some(SPRUCE_TRAPDOOR),
            Self::TerracottaCopper => Some(JUNGLE_TRAPDOOR),
            Self::DarkTimber => Some(DARK_OAK_TRAPDOOR),
            _ => None,
        }
    }

    /// Cottage styles keep the odd flower pot on their sills.
    fn has_planters(self) -> bool {
        matches!(
            self,
            Self::SpruceCottage | Self::TerracottaCopper | Self::RusticMossy
        )
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
    /// Whether the wall column at `(bx, bz)` is one a facade photograph
    /// covers, and so is built as a plain shell in `wall_block` for the
    /// texture to override.
    ///
    /// Only the photographed walls are flattened, because only they have a
    /// photograph: a procedural window, accent line, pilaster or quoin here
    /// would show through the texture wherever it has no data, and a
    /// protrusion in front of it would hide part of it, but on the walls the
    /// export never covered, the same features are the whole building.
    #[inline]
    fn is_photo_column(&self, bx: i32, bz: i32) -> bool {
        self.facade_shell.contains(&(bx, bz))
    }

    /// Whether the wall column at `(bx, bz)` has a photograph of some kind
    /// coming over it, from Mapillary or from the preset set, and so is built
    /// as a plain shell.
    ///
    /// A panel hangs `displays::PUSH_OUT` in front of the outermost wall
    /// cell, so anything the building sticks out past its own wall stands
    /// between the viewer and the picture: a window frame, a quoin, a cornice
    /// or a downpipe reads as a lump of concrete floating on the photograph.
    /// The Mapillary path has always flattened for this reason. Doors are the
    /// deliberate exception and keep using [`Self::is_photo_column`]: a door
    /// sits in the wall plane rather than in front of it, so the panel hides
    /// it anyway, and suppressing it would seal every building in the world.
    #[inline]
    fn is_flat_column(&self, bx: i32, bz: i32) -> bool {
        self.is_photo_column(bx, bz) || self.preset_shell.contains(&(bx, bz))
    }

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

    /// Rows an entrance column keeps clear of dressing: the door itself and
    /// its canopy row. Bands, headers and sills above carry on across it.
    #[inline]
    fn entrance_row(&self, h: i32) -> bool {
        h <= self.start_y_offset + 3
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

    /// Slot of along-wall ordinate `u` on a world-aligned lattice of the
    /// rhythm, for walls no run layout covers. `u` is the along-wall ordinate,
    /// not a world-space sum; `bx + bz` would make the period depend on the
    /// wall angle.
    fn lattice_slot(&self, u: i32, rhythm: BayRhythm) -> BaySlot {
        let period = rhythm.window + rhythm.pier;
        let at = u + self.window_phase;
        let k = at.rem_euclid(period);
        if k < rhythm.window {
            BaySlot::Window {
                k,
                width: rhythm.window,
                bay: at.div_euclid(period),
            }
        } else {
            BaySlot::Pier
        }
    }

    /// Upper-floor slot of a wall column.
    #[inline]
    fn slot_of(&self, col: ColumnFacade) -> BaySlot {
        match col.slot {
            BaySlot::Lattice => self.lattice_slot(col.wall_u, self.bays),
            slot => slot,
        }
    }

    /// Shopfront slot of a wall column.
    #[inline]
    fn shop_slot_of(&self, col: ColumnFacade) -> BaySlot {
        match col.shop {
            BaySlot::Lattice => self.lattice_slot(col.wall_u, SHOP_RHYTHM),
            slot => slot,
        }
    }

    /// Whether the facade is punched windows laid out in bays, rather than
    /// glass bands, a curtain grid or tower slits.
    #[inline]
    fn punched_windows(&self) -> bool {
        self.has_windows
            && !self.use_horizontal_windows
            && !matches!(
                self.category,
                BuildingCategory::Tower | BuildingCategory::GridSkyscraper
            )
    }

    /// Whether row `h` belongs to the piano nobile, the first storey over
    /// the ground floor.
    #[inline]
    fn nobile_storey(&self, h: i32) -> bool {
        let gft = self.ground_floor_top();
        self.piano_nobile && h > gft && h <= gft + self.floor_cycle
    }

    /// Glazed rows a punched window of an upper storey is capped at, if any.
    /// European fabric keeps its windows two rows tall in a taller storey,
    /// three on the piano nobile, so a solid head row stays over each.
    fn window_row_cap(&self, h: i32) -> Option<i32> {
        let capped = self.region == BuildingRegion::Europe
            && !matches!(
                self.window_archetype,
                WindowArchetype::Ribbon
                    | WindowArchetype::Wide
                    | WindowArchetype::Piers
                    | WindowArchetype::Strips
            );
        capped.then(|| if self.nobile_storey(h) { 3 } else { 2 })
    }

    /// Whether the block `determine_wall_block_at_position` chose for row `h`
    /// of a column sits a block behind the wall face: glass, and the spandrels
    /// of a tower shaft, which recede with it so the piers run unbroken.
    fn set_back(&self, h: i32, col: ColumnFacade, block: Block, glazed: bool) -> bool {
        if block == self.wall_block {
            return false;
        }
        glazed
            || (self.window_archetype.recessed_spandrels()
                && self.punched_windows()
                && block == self.accent_block
                && h > self.ground_floor_top()
                && h < self.start_y_offset + self.building_height
                && self.slot_of(col).is_window())
    }

    /// Whether this column carries the street-level shopfront or lobby.
    #[inline]
    fn shopfront_column(&self, col: ColumnFacade) -> bool {
        self.ground != GroundFloor::Plain && col.street && !col.party && self.has_windows
    }

    /// Bays of a straight wall run `len` columns long, upper floors and ground.
    fn run_layouts(&self, len: i32) -> (BayLayout, BayLayout) {
        let upper = BayLayout::plan(len, self.bays);
        // The shopfront ends on the same corner piers as the floors above,
        // so the piers run straight down to the pavement.
        let shop = BayLayout::plan(
            len,
            BayRhythm {
                corner: upper.lead.clamp(1, 2),
                ..SHOP_RHYTHM
            },
        );
        (upper, shop)
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

/// What the street-facing ground floor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroundFloor {
    /// Windows like the floors above, from the second row up.
    Plain,
    /// Display glass from the pavement up between dark piers, under a sign band.
    Shopfront,
    /// A tower's glazed entrance hall, laid out like a shopfront.
    Lobby,
}

/// What the street-facing ground floor of a building is. A shop comes from
/// the building's own tags or category, or, for unlabelled multi-storey
/// fabric in a built-up row or on a street corner, from how often city blocks
/// of the region keep shops at street level. Towers get a glazed lobby.
fn pick_ground_floor(
    element: &ProcessedWay,
    category: BuildingCategory,
    floors: i32,
    region: BuildingRegion,
    attached: bool,
    facade: &FacadePlan,
    group_seed: u64,
) -> GroundFloor {
    if matches!(
        category,
        BuildingCategory::TallBuilding
            | BuildingCategory::ModernSkyscraper
            | BuildingCategory::ContemporarySkyscraper
            | BuildingCategory::MasonrySkyscraper
            | BuildingCategory::GridSkyscraper
    ) {
        return GroundFloor::Lobby;
    }
    if !facade.has_any_street {
        return GroundFloor::Plain;
    }
    let tags = &element.tags;
    let tagged_shop = tags.get("shop").is_some_and(|v| v != "no")
        || tags.get("amenity").is_some_and(|v| {
            matches!(
                v.as_str(),
                "restaurant" | "cafe" | "bar" | "pub" | "fast_food" | "bank" | "pharmacy"
            )
        });
    if tagged_shop {
        return GroundFloor::Shopfront;
    }
    let corner = facade.corner.is_some();
    let chance = match category {
        BuildingCategory::Commercial | BuildingCategory::Hotel => {
            if corner {
                0.85
            } else {
                0.65
            }
        }
        BuildingCategory::Office => 0.40,
        BuildingCategory::Residential | BuildingCategory::Default
            if floors >= 3 && (attached || corner) =>
        {
            let base = match region {
                BuildingRegion::NorthAmerica => 0.60,
                BuildingRegion::Europe => 0.45,
                BuildingRegion::Other => 0.30,
            };
            if corner {
                base + 0.15
            } else {
                base
            }
        }
        _ => 0.0,
    };
    if chance > 0.0 && element_rng(group_seed ^ 0x5709_EF90_0000_0002).random_bool(chance) {
        GroundFloor::Shopfront
    } else {
        GroundFloor::Plain
    }
}

/// Light stone for lintels, sills and cornices, set off against the wall the
/// way limestone and brownstone trim is against brick and stucco.
fn trim_block_for_wall(wall: Block, seed: u64) -> Block {
    match wall {
        BRICK | TERRACOTTA | RED_TERRACOTTA | ORANGE_TERRACOTTA | GRANITE | POLISHED_GRANITE
        | NETHER_BRICK | RED_NETHER_BRICKS | MUD_BRICKS | BROWN_TERRACOTTA | BROWN_CONCRETE => {
            const LIGHT_STONE: [Block; 3] = [SMOOTH_STONE, POLISHED_DIORITE, SMOOTH_SANDSTONE];
            LIGHT_STONE[element_rng(seed ^ 0x7121_A57E_0000_0001).random_range(0..3)]
        }
        SANDSTONE => SMOOTH_SANDSTONE,
        SMOOTH_SANDSTONE | END_STONE_BRICKS | YELLOW_TERRACOTTA => SANDSTONE,
        // Grey stone on white render, the way Munich's stucco is trimmed.
        QUARTZ_BLOCK | QUARTZ_BRICKS | SMOOTH_QUARTZ | WHITE_CONCRETE => STONE_BRICKS,
        // White surrounds on pink and ochre render.
        WHITE_TERRACOTTA => QUARTZ_BLOCK,
        SMOOTH_STONE | POLISHED_DIORITE | DIORITE => QUARTZ_BLOCK,
        OAK_PLANKS | OAK_LOG | SPRUCE_PLANKS | SPRUCE_LOG => DARK_OAK_PLANKS,
        DARK_OAK_PLANKS => SPRUCE_PLANKS,
        _ => POLISHED_DIORITE,
    }
}

/// Slab cut of a trim block, for sills, ledges and cornice caps.
fn trim_slab(trim: Block) -> Block {
    match trim {
        SMOOTH_STONE => SMOOTH_STONE_SLAB,
        POLISHED_DIORITE => POLISHED_DIORITE_SLAB,
        SMOOTH_SANDSTONE | SANDSTONE => CUT_SANDSTONE_SLAB,
        QUARTZ_BLOCK => QUARTZ_SLAB_TOP,
        DARK_OAK_PLANKS => DARK_OAK_SLAB,
        SPRUCE_PLANKS => SPRUCE_SLAB,
        POLISHED_ANDESITE => POLISHED_ANDESITE_SLAB,
        POLISHED_BLACKSTONE => POLISHED_BLACKSTONE_SLAB,
        POLISHED_DEEPSLATE => POLISHED_DEEPSLATE_SLAB,
        BLACK_CONCRETE => BLACKSTONE_SLAB,
        DARK_PRISMARINE => DARK_PRISMARINE_SLAB,
        other => get_slab_block_for_material(other),
    }
}

/// Stair cut of a trim block, for cornices and window hoods.
fn trim_stair(trim: Block) -> Block {
    match trim {
        SMOOTH_STONE => STONE_STAIRS,
        QUARTZ_BLOCK => QUARTZ_STAIRS,
        SMOOTH_SANDSTONE | SANDSTONE => SMOOTH_SANDSTONE_STAIRS,
        DARK_OAK_PLANKS => DARK_OAK_STAIRS,
        SPRUCE_PLANKS => SPRUCE_STAIRS,
        other => get_stair_block_for_material(other),
    }
}

/// Dark frame materials of hand-built shopfronts: painted metal, granite, slate.
const SHOP_FRAME_OPTIONS: [Block; 4] = [
    BLACK_CONCRETE,
    POLISHED_DEEPSLATE,
    POLISHED_BLACKSTONE,
    DARK_PRISMARINE,
];

/// European shopfronts are framed in grey and black, never painted teal.
const EU_SHOP_FRAME_OPTIONS: [Block; 4] = [
    BLACK_CONCRETE,
    POLISHED_DEEPSLATE,
    GRAY_CONCRETE,
    POLISHED_BLACKSTONE,
];

fn shop_frame_options(region: BuildingRegion) -> &'static [Block] {
    match region {
        BuildingRegion::Europe => &EU_SHOP_FRAME_OPTIONS,
        _ => &SHOP_FRAME_OPTIONS,
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
        // Seeded from the terrain itself, never from args.ground_level: with the extended
        // floor the scaler sinks the terrain base far below it, and a seeded max would
        // strand every low-lying building up at the requested ground level.
        let mut max_ground_level: Option<i32> = None;
        for node in &element.nodes {
            if let Some(level) = editor.terrain_level(node.x, node.z) {
                max_ground_level = Some(max_ground_level.map_or(level, |m: i32| m.max(level)));
            }
        }
        max_ground_level.unwrap_or(args.ground_level) + min_level_offset
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
        | BuildingCategory::Commercial
        | BuildingCategory::TallBuilding => &INSTITUTIONAL_WINDOW_OPTIONS,
        BuildingCategory::ContemporarySkyscraper
        | BuildingCategory::GridSkyscraper
        | BuildingCategory::ModernSkyscraper
        | BuildingCategory::GlassySkyscraper
        | BuildingCategory::GlassCornerSkyscraper => &TOWER_WINDOW_OPTIONS,
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
    era: ArchEra,
    region: BuildingRegion,
    building_height: i32,
    floor_cycle: i32,
    has_street: bool,
    group_seed: u64,
) -> BalconyBand {
    // Pre-war European street fronts carry the odd balcony, never a stack
    // of them on every bay.
    let old_europe = region == BuildingRegion::Europe
        && !matches!(era, ArchEra::PostWarPanel | ArchEra::Contemporary);
    if !matches!(
        category,
        BuildingCategory::Residential | BuildingCategory::House
    ) || building_height < 3 * floor_cycle
        || !has_street
        || old_europe
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

/// Per-building window type: how wide the windows and piers are, and how the
/// rows between storeys read. Laid out per wall run by [`BayLayout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowArchetype {
    /// 1-wide portrait sashes a pier apart: the walk-up tenement rhythm.
    Sash,
    /// 2-wide portrait windows between 2-wide piers.
    Portrait,
    /// 1-wide windows between 2-wide piers: stucco fabric, churches, farms.
    Spaced,
    /// 3-wide windows: shops, schools, workshops.
    Wide,
    /// Near-continuous glass bands over a spandrel band, slab ledge every floor.
    Ribbon,
    /// Portrait windows under stair hoods. Ornate historic fabric only.
    Hooded,
    /// Tower shaft of paired windows between continuous 1-wide piers, the
    /// spandrels between storeys set back with the glass.
    Piers,
    /// Tower shaft of 1-wide glass strips between 1-wide piers, spandrels set back.
    Strips,
}

impl WindowArchetype {
    /// Window and pier widths in columns.
    fn rhythm(self) -> BayRhythm {
        let (window, pier, corner) = match self {
            Self::Sash => (1, 1, 1),
            Self::Portrait | Self::Hooded => (2, 2, 2),
            Self::Spaced => (1, 2, 2),
            Self::Wide => (3, 2, 2),
            Self::Ribbon => (4, 1, 1),
            Self::Piers => (2, 1, 2),
            Self::Strips => (1, 1, 1),
        };
        BayRhythm {
            window,
            pier,
            corner,
        }
    }

    /// Whether the rows between storeys are set back with the glass, so the
    /// piers run unbroken up the shaft.
    fn recessed_spandrels(self) -> bool {
        matches!(self, Self::Piers | Self::Strips)
    }
}

/// Window, pier and corner pier widths of a facade, in columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BayRhythm {
    window: i32,
    pier: i32,
    /// Narrowest pier a wall run may end on, so no glass reaches a corner.
    corner: i32,
}

/// Bays of a shopfront or tower lobby: wide display windows between 1-wide piers.
const SHOP_RHYTHM: BayRhythm = BayRhythm {
    window: 3,
    pier: 1,
    corner: 1,
};

/// Windows laid out symmetrically along one straight wall run: as many as fit
/// the rhythm, centred between corner piers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BayLayout {
    len: i32,
    /// Width of the pier at either end.
    lead: i32,
    window: i32,
    pier: i32,
    count: i32,
    /// The middle window is a column wider: an odd count over an odd remainder.
    wide_window: bool,
    /// The middle pier is a column wider: an even count over an odd remainder.
    wide_pier: bool,
}

impl BayLayout {
    fn plan(len: i32, rhythm: BayRhythm) -> Self {
        let (window, pier) = (rhythm.window.max(1), rhythm.pier.max(1));
        // A short wall cannot afford wide corner piers on top of its windows.
        let corner = if len < 3 * (window + pier) {
            1
        } else {
            rhythm.corner.max(1)
        };
        let mut count = if len - 2 * corner >= window {
            (len - 2 * corner + pier) / (window + pier)
        } else {
            0
        };
        loop {
            let mut layout = Self {
                len,
                lead: len,
                window,
                pier,
                count,
                wide_window: false,
                wide_pier: false,
            };
            if count == 0 {
                return layout;
            }
            // What the windows leave over goes to the two end piers; an odd
            // column widens the middle of the run instead of one end.
            let spare = len - count * window - (count - 1) * pier;
            if spare % 2 == 1 {
                if count % 2 == 0 {
                    layout.wide_pier = true;
                } else if window == 1 && count >= 5 {
                    // A double sash amid a long row of single ones reads as a
                    // mistake; one window fewer gives a wider middle pier.
                    count -= 1;
                    continue;
                } else {
                    layout.wide_window = true;
                }
            }
            layout.lead = spare / 2;
            return layout;
        }
    }

    fn slot(&self, i: i32) -> BaySlot {
        if self.count == 0 || i < self.lead || i >= self.len - self.lead {
            return BaySlot::End;
        }
        let mid = self.count / 2;
        let mut x = i - self.lead;
        for bay in 0..self.count {
            let width = self.window + i32::from(self.wide_window && bay == mid);
            if x < width {
                return BaySlot::Window { k: x, width, bay };
            }
            x -= width;
            let pier = self.pier + i32::from(self.wide_pier && bay + 1 == mid);
            if x < pier {
                return BaySlot::Pier;
            }
            x -= pier;
        }
        BaySlot::End
    }
}

/// Whether `floor_row` of an upper storey is glass under the given archetype.
/// Row 0 is the band between storeys; rows 1..cycle-1 are open rows.
fn archetype_allows_window(archetype: WindowArchetype, floor_row: i32, floor_cycle: i32) -> bool {
    match archetype {
        // A ribbon keeps a two-row spandrel where the storey is tall enough.
        WindowArchetype::Ribbon if floor_cycle >= 4 => floor_row >= 2,
        // Single sashes stay two rows tall, portrait rather than slit.
        WindowArchetype::Spaced | WindowArchetype::Sash => (1..=2).contains(&floor_row),
        _ => floor_row >= 1,
    }
}

/// Whether a wall block reads as load-bearing masonry, which carries piers
/// and spandrels rather than glass bands.
fn is_masonry_wall(block: Block) -> bool {
    matches!(
        block,
        BRICK
            | STONE_BRICKS
            | SANDSTONE
            | SMOOTH_SANDSTONE
            | GRANITE
            | POLISHED_GRANITE
            | TERRACOTTA
            | RED_TERRACOTTA
            | ORANGE_TERRACOTTA
            | BROWN_TERRACOTTA
            | WHITE_TERRACOTTA
            | END_STONE_BRICKS
            | QUARTZ_BRICKS
            | MUD_BRICKS
            | NETHER_BRICK
            | RED_NETHER_BRICKS
            | POLISHED_DIORITE
            | COBBLESTONE
    )
}

/// Weighted per-category archetype choice, seeded on the shared group seed.
fn pick_window_archetype(
    category: BuildingCategory,
    era: ArchEra,
    region: BuildingRegion,
    wall_block: Block,
    group_seed: u64,
) -> WindowArchetype {
    use WindowArchetype::*;
    let europe = region == BuildingRegion::Europe;
    let america = region == BuildingRegion::NorthAmerica;
    // (archetype, weight), summing to 100 per row
    let table: &[(WindowArchetype, u32)] = match category {
        BuildingCategory::House => &[(Portrait, 45), (Spaced, 30), (Sash, 10), (Wide, 15)],
        // Walk-up tenements in North America, stucco blocks in Europe.
        BuildingCategory::Residential if america => &[
            (Sash, 40),
            (Portrait, 30),
            (Spaced, 10),
            (Ribbon, 10),
            (Wide, 10),
        ],
        // Central European stucco: 1-wide windows on a period of three.
        BuildingCategory::Residential if europe => &[
            (Spaced, 60),
            (Portrait, 32),
            (Sash, 4),
            (Ribbon, 2),
            (Wide, 2),
        ],
        BuildingCategory::Residential => &[
            (Portrait, 40),
            (Spaced, 20),
            (Sash, 15),
            (Ribbon, 15),
            (Wide, 10),
        ],
        BuildingCategory::Default if america => {
            &[(Sash, 30), (Portrait, 40), (Ribbon, 15), (Wide, 15)]
        }
        BuildingCategory::Default if europe => {
            &[(Spaced, 55), (Portrait, 33), (Ribbon, 6), (Wide, 6)]
        }
        BuildingCategory::Default => &[(Portrait, 45), (Spaced, 20), (Ribbon, 20), (Wide, 15)],
        BuildingCategory::Commercial | BuildingCategory::Office => {
            &[(Portrait, 35), (Ribbon, 35), (Wide, 20), (Spaced, 10)]
        }
        BuildingCategory::Hotel => &[(Portrait, 45), (Spaced, 25), (Ribbon, 20), (Wide, 10)],
        BuildingCategory::School | BuildingCategory::Hospital => {
            &[(Portrait, 30), (Wide, 35), (Ribbon, 35)]
        }
        BuildingCategory::Industrial | BuildingCategory::Warehouse => {
            &[(Wide, 40), (Ribbon, 50), (Portrait, 10)]
        }
        BuildingCategory::Historic => &[(Portrait, 50), (Spaced, 30), (Hooded, 20)],
        // Masonry towers stack paired windows between continuous piers;
        // anything else reads as a postwar or modern slab.
        BuildingCategory::TallBuilding
            if is_masonry_wall(wall_block)
                || matches!(era, ArchEra::TraditionalPreWar | ArchEra::HistoricOrnate) =>
        {
            &[(Piers, 65), (Strips, 35)]
        }
        BuildingCategory::TallBuilding => &[(Ribbon, 50), (Piers, 25), (Strips, 25)],
        BuildingCategory::MasonrySkyscraper => return Piers,
        BuildingCategory::ContemporarySkyscraper => return Wide,
        BuildingCategory::Religious | BuildingCategory::Farm => return Spaced,
        _ => return Portrait,
    };
    let mut rng = element_rng(group_seed ^ 0x57A2_C0DE_A5C1_0007);
    let total: u32 = table.iter().map(|&(_, w)| w).sum();
    let mut roll = rng.random_range(0..total);
    let mut picked = Portrait;
    for &(archetype, weight) in table {
        if roll < weight {
            picked = archetype;
            break;
        }
        roll -= weight;
    }
    if category == BuildingCategory::TallBuilding {
        return picked;
    }
    // Era adjustment on top of the category weights: masonry eras punch
    // windows, panel and glass eras band them.
    let mut era_rng = element_rng(group_seed ^ 0x57A2_C0DE_A5C1_0008);
    match (era, picked) {
        (ArchEra::HistoricOrnate, Portrait) if era_rng.random_bool(0.6) => Hooded,
        (ArchEra::HistoricOrnate | ArchEra::TraditionalPreWar, Ribbon | Wide) => Portrait,
        (ArchEra::PostWarPanel | ArchEra::Contemporary, Sash | Spaced | Hooded) => {
            if era_rng.random_bool(0.5) {
                Ribbon
            } else {
                Wide
            }
        }
        _ => picked,
    }
}

/// Darkens the glass against a dark wall or dark accent bands: grey glass, or
/// black where grey was picked, so windows do not glow out of a dark facade.
/// Other walls keep the pick.
fn coordinated_window_block(wall_block: Block, accent_block: Block, picked: Block) -> Block {
    const DARK: &[Block] = &[
        BLACK_CONCRETE,
        GRAY_CONCRETE,
        BLACKSTONE,
        POLISHED_BLACKSTONE,
        POLISHED_BLACKSTONE_BRICKS,
        DEEPSLATE_BRICKS,
        POLISHED_DEEPSLATE,
        COBBLED_DEEPSLATE,
        NETHER_BRICK,
        RED_NETHER_BRICKS,
        BLACK_TERRACOTTA,
        GRAY_TERRACOTTA,
        BROWN_TERRACOTTA,
        DARK_OAK_PLANKS,
    ];
    if !(DARK.contains(&wall_block) || DARK.contains(&accent_block)) {
        picked
    } else if matches!(picked, GRAY_STAINED_GLASS | BLACK_STAINED_GLASS) {
        BLACK_STAINED_GLASS
    } else {
        GRAY_STAINED_GLASS
    }
}

/// Trim stone that suits the wall: grey stone or
/// quartz on white render, white quartz surrounds on pink and ochre render,
/// buff and light stone on brick, a sibling sandstone on sandstone. Other
/// walls draw from the generic trims.
fn accent_for_wall(wall: Block, rng: &mut impl Rng) -> Block {
    let options: &[Block] = match wall {
        WHITE_CONCRETE | CALCITE | QUARTZ_BLOCK | MUSHROOM_STEM | LIGHT_GRAY_CONCRETE_POWDER => {
            &[STONE_BRICKS, STONE_BRICKS, SMOOTH_STONE, QUARTZ_BLOCK]
        }
        WHITE_TERRACOTTA | CHERRY_PLANKS | BIRCH_PLANKS | YELLOW_TERRACOTTA | RED_TERRACOTTA
        | TERRACOTTA | ORANGE_TERRACOTTA => {
            &[QUARTZ_BLOCK, QUARTZ_BLOCK, SMOOTH_QUARTZ, STONE_BRICKS]
        }
        SMOOTH_SANDSTONE | END_STONE_BRICKS => &[SANDSTONE, SANDSTONE, SMOOTH_STONE],
        SANDSTONE => &[SMOOTH_SANDSTONE, SMOOTH_SANDSTONE, SMOOTH_STONE],
        BRICK | POLISHED_GRANITE | GRANITE | MUD_BRICKS | BROWN_TERRACOTTA => {
            &[SMOOTH_SANDSTONE, MUSHROOM_STEM, STONE_BRICKS, SMOOTH_STONE]
        }
        _ => &ACCENT_BLOCK_OPTIONS,
    };
    options[rng.random_range(0..options.len())]
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
            return Some(wall_block_for_colour_tag(rgb, rng));
        }
    }
    None
}

/// Render and stone tones with their mean texture colour. Pale and earthy
/// `building:colour` values describe a rendered or stone facade, and the
/// general palette sends them to grey stone: a beige tag came out smooth
/// stone. End stone stays for a tagged pale yellow only.
const RENDER_TONES: [(Block, (u8, u8, u8)); 13] = [
    (WHITE_CONCRETE, (207, 213, 214)),
    (WHITE_TERRACOTTA, (210, 178, 161)),
    (CHERRY_PLANKS, (226, 178, 172)),
    (SMOOTH_SANDSTONE, (224, 214, 170)),
    (MUSHROOM_STEM, (203, 196, 185)),
    (BIRCH_PLANKS, (192, 175, 121)),
    (END_STONE_BRICKS, (218, 224, 162)),
    (YELLOW_TERRACOTTA, (186, 133, 35)),
    (LIGHT_GRAY_CONCRETE_POWDER, (154, 154, 148)),
    (SMOOTH_STONE, (159, 159, 159)),
    (RED_TERRACOTTA, (143, 61, 47)),
    (BRICK, (151, 98, 83)),
    (TERRACOTTA, (152, 94, 68)),
];

/// Wall block for a `building:colour` tag: the nearest render or stone tone
/// for pale, unsaturated colours, the general palette for the rest.
fn wall_block_for_colour_tag(rgb: (u8, u8, u8), rng: &mut impl Rng) -> Block {
    let target = crate::colors::oklab_components(&rgb);
    if target.0 < 0.55 || target.1.hypot(target.2) >= 0.14 {
        return crate::block_palette::wall_block_for_color(rgb, rng);
    }
    RENDER_TONES
        .iter()
        .map(|&(b, c)| {
            let lab = crate::colors::oklab_components(&c);
            let (dl, da, db) = (0.5 * (target.0 - lab.0), target.1 - lab.1, target.2 - lab.2);
            (b, dl * dl + da * da + db * db)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(WHITE_CONCRETE, |(b, _)| b)
}

/// Wall block for a building without material or colour tags (those are
/// `determine_wall_block_from_tags`): the regional palette where the region
/// has one, else the category palette.
fn determine_wall_block(
    element: &ProcessedWay,
    category: BuildingCategory,
    era: ArchEra,
    climate: Climate,
    building_height: i32,
    ctx: &RoofContext,
    rng: &mut impl Rng,
) -> Block {
    // Historic castles have their own special treatment
    if element.tags.get("historic").map(String::as_str) == Some("castle") {
        return get_castle_wall_block(rng);
    }
    let typology = if category == BuildingCategory::Historic {
        // A listed townhouse or palace is built like its neighbours; only
        // fortifications keep the castle stones.
        (!is_fortification(element)).then_some(WallTypology::Civic)
    } else {
        wall_typology(category, era, building_height, ctx)
    };
    if let Some(pool) = typology.and_then(|t| regional_wall_pool(ctx.materials, t)) {
        return pick_district_wall(pool, category, climate, ctx.district, rng);
    }
    get_wall_block_for_category(category, era, climate, rng)
}

/// Walls, towers and ruins that keep castle masonry under `historic=*`.
fn is_fortification(element: &ProcessedWay) -> bool {
    let tag = |k: &str| element.tags.get(k).map(String::as_str).unwrap_or("");
    matches!(tag("building"), "castle" | "ruins" | "fort" | "bunker")
        || matches!(
            tag("historic"),
            "castle"
                | "fort"
                | "fortress"
                | "fortification"
                | "citywalls"
                | "city_wall"
                | "city_gate"
                | "tower"
                | "ruins"
                | "bunker"
                | "archaeological_site"
        )
}

/// Which kind of fabric a building belongs to, for the regional palettes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WallTypology {
    /// Detached houses.
    House,
    /// Pre-war or undated urban fabric: rows, tenements, shops.
    Traditional,
    /// Pre-war offices, early high-rises, institutions, listed buildings.
    Civic,
    /// Post-war and contemporary blocks.
    Modern,
}

/// Typology from category, era and height; None for categories that keep
/// their own palette (industry, worship, farms, sheds, skyscraper families).
fn wall_typology(
    category: BuildingCategory,
    era: ArchEra,
    building_height: i32,
    ctx: &RoofContext,
) -> Option<WallTypology> {
    let modern_era = matches!(era, ArchEra::PostWarPanel | ArchEra::Contemporary);
    let old_era = matches!(era, ArchEra::HistoricOrnate | ArchEra::TraditionalPreWar);
    // About ten storeys: above that an undated block is rarely a tenement.
    let high_rise = f64::from(building_height) >= 30.0 * ctx.scale;
    let american = matches!(
        ctx.materials,
        MaterialRegion::NorthAmerica | MaterialRegion::NorthAmericaSunbelt | MaterialRegion::Canada
    );
    // Where untagged houses follow a regional habit, a small free-standing
    // `building=yes` is a house too, and takes the house walls.
    let small_house = category == BuildingCategory::Default
        && ctx.materials.roof_habit().is_some()
        && !ctx.attached
        && f64::from(building_height) <= 11.0 * ctx.scale;
    Some(match category {
        BuildingCategory::House if modern_era && !american => WallTypology::Modern,
        BuildingCategory::House => WallTypology::House,
        _ if small_house && !modern_era => WallTypology::House,
        BuildingCategory::Residential
        | BuildingCategory::Default
        | BuildingCategory::Commercial
        | BuildingCategory::Hotel => {
            if modern_era {
                WallTypology::Modern
            } else if old_era || !high_rise {
                WallTypology::Traditional
            } else if american {
                // New York's undated mid-rises are as often pre-war stone as
                // post-war concrete; the civic pool holds both.
                WallTypology::Civic
            } else {
                WallTypology::Modern
            }
        }
        BuildingCategory::Office | BuildingCategory::TallBuilding => {
            if modern_era || (!old_era && !american && high_rise) {
                WallTypology::Modern
            } else {
                WallTypology::Civic
            }
        }
        BuildingCategory::School | BuildingCategory::Hospital => {
            if modern_era {
                WallTypology::Modern
            } else {
                WallTypology::Civic
            }
        }
        _ => return None,
    })
}

/// Weighted wall pool of a region and typology; None outside the regions
/// with a palette of their own.
fn regional_wall_pool(region: MaterialRegion, typology: WallTypology) -> Option<WallPool> {
    use MaterialRegion as R;
    use WallTypology as T;
    Some(match (region, typology) {
        (R::Other, _) => return None,
        (R::Canada, T::House) => CANADA_HOUSE_WALLS,
        (R::Canada, T::Traditional) => CANADA_TRADITIONAL_WALLS,
        (R::Canada, T::Civic) => NA_CIVIC_WALLS,
        (R::Canada, T::Modern) => NA_MODERN_WALLS,
        (R::EastEurope, T::House) => EAST_HOUSE_WALLS,
        (R::EastEurope, T::Traditional) => EAST_TRADITIONAL_WALLS,
        (R::EastEurope, T::Civic) => EAST_CIVIC_WALLS,
        (R::EastEurope | R::CentralAsia, T::Modern) => EAST_MODERN_WALLS,
        (R::CentralAsia, T::House) => CENTRAL_ASIA_HOUSE_WALLS,
        (R::CentralAsia, _) => CENTRAL_ASIA_TRADITIONAL_WALLS,
        (R::MiddleEast, T::House | T::Traditional) => MIDDLE_EAST_TRADITIONAL_WALLS,
        (R::MiddleEast, T::Civic) => MIDDLE_EAST_CIVIC_WALLS,
        (R::MiddleEast, T::Modern) => MIDDLE_EAST_MODERN_WALLS,
        (R::SubSaharanAfrica, T::House) => AFRICA_HOUSE_WALLS,
        (R::SubSaharanAfrica, T::Traditional) => AFRICA_TRADITIONAL_WALLS,
        (R::SubSaharanAfrica, T::Civic) => AFRICA_CIVIC_WALLS,
        (R::SubSaharanAfrica, T::Modern) => AFRICA_MODERN_WALLS,
        (R::SouthAsia, T::House) => SOUTH_ASIA_HOUSE_WALLS,
        (R::SouthAsia, T::Traditional) => SOUTH_ASIA_TRADITIONAL_WALLS,
        (R::SouthAsia, T::Civic) => SOUTH_ASIA_CIVIC_WALLS,
        (R::SouthAsia, T::Modern) => SOUTH_ASIA_MODERN_WALLS,
        (R::SoutheastAsia, T::House) => SOUTHEAST_ASIA_HOUSE_WALLS,
        (R::SoutheastAsia, T::Traditional) => SOUTHEAST_ASIA_TRADITIONAL_WALLS,
        (R::SoutheastAsia, T::Civic | T::Modern) => SOUTHEAST_ASIA_MODERN_WALLS,
        (R::EastAsia, T::House) => EAST_ASIA_HOUSE_WALLS,
        (R::EastAsia, T::Traditional) => EAST_ASIA_TRADITIONAL_WALLS,
        (R::EastAsia | R::Korea, T::Civic | T::Modern) => EAST_ASIA_MODERN_WALLS,
        (R::Korea, T::House) => KOREA_HOUSE_WALLS,
        (R::Korea, T::Traditional) => KOREA_TRADITIONAL_WALLS,
        (R::Japan, T::House) => JAPAN_HOUSE_WALLS,
        (R::Japan, _) => JAPAN_URBAN_WALLS,
        (R::LatinAmerica | R::SouthernCone, T::Modern) => LATAM_MODERN_WALLS,
        (R::LatinAmerica, T::House) => LATAM_HOUSE_WALLS,
        (R::LatinAmerica, T::Traditional) => LATAM_TRADITIONAL_WALLS,
        (R::LatinAmerica, T::Civic) => LATAM_CIVIC_WALLS,
        (R::SouthernCone, T::House) => SOUTHERN_CONE_HOUSE_WALLS,
        (R::SouthernCone, T::Traditional) => SOUTHERN_CONE_TRADITIONAL_WALLS,
        // Buenos Aires built its institutions in European stone.
        (R::SouthernCone, T::Civic) => EU_CIVIC_WALLS,
        (R::Australasia, T::House) => AUSTRALASIA_HOUSE_WALLS,
        (R::Australasia, T::Traditional) => AUSTRALASIA_TRADITIONAL_WALLS,
        (R::Australasia, T::Civic) => AUSTRALASIA_CIVIC_WALLS,
        (R::Australasia, T::Modern) => AUSTRALASIA_MODERN_WALLS,
        (R::NorthAmerica, T::House) => NA_HOUSE_WALLS,
        (R::NorthAmerica, T::Traditional) => NA_TRADITIONAL_WALLS,
        (R::NorthAmericaSunbelt, T::House) => SUNBELT_HOUSE_WALLS,
        (R::NorthAmericaSunbelt, T::Traditional) => SUNBELT_TRADITIONAL_WALLS,
        (R::NorthAmerica | R::NorthAmericaSunbelt, T::Civic) => NA_CIVIC_WALLS,
        (R::NorthAmerica | R::NorthAmericaSunbelt, T::Modern) => NA_MODERN_WALLS,
        (R::BritishIsles | R::BrickNorth | R::Nordic, T::Modern) => EU_BRICK_MODERN_WALLS,
        (_, T::Modern) => EU_MODERN_WALLS,
        // Brick civic halls and limestone facades are the local norm there.
        (R::BrickNorth, T::Civic) => BRICK_NORTH_TRADITIONAL_WALLS,
        (R::France, T::Civic) => FRANCE_TRADITIONAL_WALLS,
        (_, T::Civic) => EU_CIVIC_WALLS,
        (R::Central | R::CentralEast, _) => CENTRAL_TRADITIONAL_WALLS,
        (R::BrickNorth, _) => BRICK_NORTH_TRADITIONAL_WALLS,
        (R::BritishIsles, _) => BRITISH_TRADITIONAL_WALLS,
        (R::France, _) => FRANCE_TRADITIONAL_WALLS,
        (R::Nordic, _) => NORDIC_TRADITIONAL_WALLS,
        (R::Mediterranean, _) => MEDITERRANEAN_TRADITIONAL_WALLS,
    })
}

/// Index into a weighted list by one roll.
fn weighted_index(weights: impl Iterator<Item = u32> + Clone, rng: &mut impl Rng) -> usize {
    let total: u32 = weights.clone().sum();
    let mut roll = rng.random_range(0..total.max(1));
    for (i, w) in weights.enumerate() {
        if roll < w {
            return i;
        }
        roll -= w;
    }
    0
}

/// Share of buildings that step outside their district's sub-palette.
const DISTRICT_STRAY_CHANCE: f64 = 0.12;

/// Draws a wall from a regional pool. The district keeps three to five of the
/// pool's blocks, drawn by weight, and its buildings draw from those, so a
/// street shares a few materials; a few buildings stray to the whole pool.
/// Away from temperate climates the pool is reweighted like the category
/// palettes, and the boreal north adds timber.
fn pick_district_wall(
    pool: WallPool,
    category: BuildingCategory,
    climate: Climate,
    district: u64,
    rng: &mut impl Rng,
) -> Block {
    let reweight = climate != Climate::Temperate && climate_applies(climate, category);
    let timber: &[Block] =
        if reweight && matches!(climate, Climate::Boreal | Climate::Tundra | Climate::IceCap) {
            &NORDIC_WOOD_ADDITIONS
        } else {
            &[]
        };
    let mut weighted: Vec<(Block, u32)> = pool
        .iter()
        .map(|&(b, w)| (b, u32::from(w)))
        .chain(timber.iter().map(|&b| (b, 4)))
        .map(|(b, w)| {
            let factor = if reweight {
                u32::from(climate_wall_weight(climate, category, b))
            } else {
                1
            };
            (b, w * factor)
        })
        .filter(|&(_, w)| w > 0)
        .collect();
    if weighted.len() < 2 {
        weighted = pool.iter().map(|&(b, w)| (b, u32::from(w))).collect();
    }

    let mut district_rng = element_rng(district ^ 0x5B9A_1E77_D157_0002);
    let keep = district_rng.random_range(3..=5usize).min(weighted.len());
    let mut rest = weighted.clone();
    let mut sub: Vec<(Block, u32)> = Vec::with_capacity(keep);
    for _ in 0..keep {
        let i = weighted_index(rest.iter().map(|&(_, w)| w), &mut district_rng);
        sub.push(rest.swap_remove(i));
    }

    let source = if rng.random_bool(DISTRICT_STRAY_CHANCE) {
        &weighted
    } else {
        &sub
    };
    source[weighted_index(source.iter().map(|&(_, w)| w), rng)].0
}

/// Blocks a protruding feature must run down from the wall base to reach the
/// ground under both the wall column and its own cell.
fn feature_descent(
    editor: &WorldEditor,
    config: &BuildingConfig,
    wall_x: i32,
    wall_z: i32,
    out_x: i32,
    out_z: i32,
) -> i32 {
    let ground = [(wall_x, wall_z), (out_x, out_z)]
        .iter()
        .filter_map(|&(x, z)| editor.terrain_level(x, z))
        .min();
    ground.map_or(0, |g| (config.start_y_offset - g).max(0))
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
            QUARTZ_BRICKS,
            ORANGE_TERRACOTTA,
            RED_TERRACOTTA,
            MUSHROOM_STEM,
            YELLOW_TERRACOTTA,
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
            CYAN_TERRACOTTA,
            LIGHT_GRAY_TERRACOTTA,
        ],
        ArchEra::HistoricOrnate => &[
            SANDSTONE,
            SMOOTH_SANDSTONE,
            STONE_BRICKS,
            CHISELED_STONE_BRICKS,
            MUSHROOM_STEM,
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
    score += building_height.saturating_mul(2).min(25);
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
            const MODERN_SKYSCRAPER_WALL_OPTIONS: [Block; 7] = [
                GRAY_CONCRETE,
                LIGHT_GRAY_CONCRETE,
                WHITE_CONCRETE,
                POLISHED_ANDESITE,
                SMOOTH_STONE,
                QUARTZ_BLOCK,
                SMOOTH_SANDSTONE,
            ];
            MODERN_SKYSCRAPER_WALL_OPTIONS
                [rng.random_range(0..MODERN_SKYSCRAPER_WALL_OPTIONS.len())]
        }
        BuildingCategory::ContemporarySkyscraper | BuildingCategory::GridSkyscraper => {
            // Light modern concrete/stone frame. Sandstone belongs here for the
            // same reason it belongs in the masonry and historic palettes: a
            // buff stone frame around glass is as common as a pale concrete
            // one, and without it every untagged tower comes out grey.
            const CONTEMPORARY: [Block; 7] = [
                LIGHT_GRAY_CONCRETE,
                WHITE_CONCRETE,
                GRAY_CONCRETE,
                QUARTZ_BLOCK,
                SMOOTH_STONE,
                SMOOTH_SANDSTONE,
                SANDSTONE,
            ];
            CONTEMPORARY[rng.random_range(0..CONTEMPORARY.len())]
        }
        BuildingCategory::MasonrySkyscraper => {
            // One warm (buff sandstone) or grey (limestone/granite) palette per building.
            if rng.random_bool(0.5) {
                const WARM: [Block; 3] = [SMOOTH_SANDSTONE, SANDSTONE, MUSHROOM_STEM];
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
            // Curtain walls read grey to black; cyan only on some modern towers.
            const GLASSY_WALL_OPTIONS: [Block; 5] = [
                GRAY_STAINED_GLASS,
                GRAY_STAINED_GLASS,
                BLACK_STAINED_GLASS,
                LIGHT_GRAY_STAINED_GLASS,
                CYAN_STAINED_GLASS,
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
    // Both mapped: the storeys are as tall as the building says. Under the
    // usual 3 a 25 m five-storey office would show seven rows of windows.
    let levels = tags
        .get("building:levels")
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|l| *l >= 2.0);
    if let (Some(height), Some(levels)) = (parse_building_height_tag(tags), levels) {
        let cycle = ((height - GROUND_FLOOR_BONUS as f64) / levels).floor() as i32;
        return cycle.clamp(3, 5);
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

/// Tallest `height` tag taken at face value. The tallest building is about
/// 830 m; values beyond this are usually an elevation mapped into `height`.
const MAX_PLAUSIBLE_HEIGHT_M: f64 = 1000.0;

/// Parses a `height` tag in meters, dropping values no building can have.
fn parse_building_height_tag(tags: &HashMap<String, String>) -> Option<f64> {
    let height = tags
        .get("height")?
        .trim_end_matches('m')
        .trim()
        .parse::<f64>()
        .ok()?;
    (height.is_finite() && height <= MAX_PLAUSIBLE_HEIGHT_M).then_some(height)
}

/// Rise of the pitched roof the generator will put on a building whose mapped
/// `height` already includes it, or None when the walls keep the full height:
/// flat roofs, a mapped roof:height or roof:angle (roof_peak_cap reads those),
/// or no height tag at all (levels count the walls only).
fn mapped_height_roof_rise(
    element: &ProcessedWay,
    style: &BuildingStyle,
    condition: BuildingCondition,
    wall_height: i32,
    footprint_cells: usize,
    steep: bool,
    scale_factor: f64,
) -> Option<i32> {
    let pitched = matches!(
        style.roof_type,
        RoofType::Gabled
            | RoofType::Hipped
            | RoofType::HalfHipped
            | RoofType::Gambrel
            | RoofType::Pyramidal
            | RoofType::Mansard
    );
    if !style.generate_roof
        || !pitched
        || !matches!(
            condition,
            BuildingCondition::Normal | BuildingCondition::Disused | BuildingCondition::Abandoned
        )
        || parse_building_height_tag(&element.tags).is_none()
        || element.tags.contains_key("roof:height")
        || element.tags.contains_key("roof:angle")
    {
        return None;
    }
    // Shorter half-span of a near-rectangle: area over its longest extent.
    let mut diameter: f64 = 1.0;
    for (i, a) in element.nodes.iter().enumerate() {
        for b in &element.nodes[i + 1..] {
            let d = f64::from(a.x - b.x).hypot(f64::from(a.z - b.z));
            diameter = diameter.max(d);
        }
    }
    let short_half = footprint_cells as f64 / diameter / 2.0;
    let slope_rise = if style.roof_type == RoofType::Mansard {
        4.0 * scale_factor + ((short_half - 2.0) / 2.0).max(0.0)
    } else {
        short_half * 0.85
    };
    // The roof generator caps a rise at a share of the walls left under it.
    let wall_share = if steep { 0.47 } else { 0.375 };
    let rise = slope_rise
        .min(f64::from(max_inferred_rise(steep, scale_factor)))
        .min(f64::from(wall_height) * wall_share)
        .round() as i32;
    (rise >= 1).then_some(rise)
}

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
    if let Some(height) = parse_building_height_tag(&element.tags) {
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
        // Parts can be thin slabs (a plinth, a cornice), elevated or not:
        // they skip the 3-block interior minimum, under a roof as well
        let is_part = element.tags.contains_key("building:part");
        let wall_floor: f64 = if is_part { 1.0 } else { 3.0 };
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
                (effective - rh.max(0.0)).max(wall_floor.min(effective))
            }
            _ => effective,
        };
        building_height = (effective * scale_factor) as i32;
        building_height = building_height.max(if is_elevated_part || is_part { 1 } else { 3 });
        if height > 28.0 {
            is_tall_building = true;
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
    let roof_thickness: i32 = if let Some(h) = parse_building_height_tag(&element.tags) {
        let total = (h * scale_factor) as i32;
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
        | RoofType::Onion
        | RoofType::Round => {
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
                    max_rise: None,
                    trim: RoofTrim::default(),
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
                let nodes = &element.nodes;
                match roof_type {
                    RoofType::Cone => generate_cone_roof(editor, cached_floor_area, &config, nodes),
                    RoofType::Onion => generate_onion_roof(editor, cached_floor_area, &config),
                    RoofType::Hipped => {
                        generate_hipped_roof(editor, cached_floor_area, &config);
                    }
                    RoofType::Pyramidal => {
                        generate_pyramidal_roof(editor, cached_floor_area, &config, nodes)
                    }
                    // A canopy mapped round is a barrel vault like any other.
                    RoofType::Round => {
                        let ridge = ridge_frame(
                            nodes,
                            config.width(),
                            config.length(),
                            element.tags.get("roof:orientation").map(|s| s.as_str()),
                            element.tags.get("roof:direction").map(|s| s.as_str()),
                            None,
                        );
                        generate_gabled_roof(
                            editor,
                            cached_floor_area,
                            &config,
                            GableProfile::Round,
                            &ridge,
                            None,
                        );
                    }
                    _ => generate_dome_roof(editor, cached_floor_area, &config, nodes),
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

/// Footprints this small are pillars: no windows, no doors.
const THIN_RING_MAX_CELLS: usize = 4;

/// Cells on a closed ring's edges, for footprints the flood fill leaves
/// empty because no lattice cell lies strictly inside them.
fn thin_ring_cells(nodes: &[ProcessedNode]) -> Vec<(i32, i32)> {
    let closed = nodes.len() >= 4
        && (nodes[0].x, nodes[0].z) == (nodes[nodes.len() - 1].x, nodes[nodes.len() - 1].z);
    if !closed {
        return Vec::new();
    }
    let mut cells: Vec<(i32, i32)> = nodes
        .windows(2)
        .flat_map(|pair| {
            bresenham_line(pair[0].x, 0, pair[0].z, pair[1].x, 0, pair[1].z)
                .into_iter()
                .map(|(x, _, z)| (x, z))
        })
        .collect();
    cells.sort_unstable();
    cells.dedup();
    cells
}

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
    /// Metal doors of a tower lobby.
    Iron,
}

impl DoorStyle {
    fn base_block(self) -> Block {
        match self {
            DoorStyle::Oak => OAK_DOOR,
            DoorStyle::Spruce => SPRUCE_DOOR_LOWER,
            DoorStyle::DarkOak => DARK_OAK_DOOR_LOWER,
            DoorStyle::Birch => BIRCH_DOOR,
            DoorStyle::Iron => IRON_DOOR,
        }
    }

    /// Matching trapdoor species for the entrance canopy.
    fn trapdoor_block(self) -> Block {
        match self {
            DoorStyle::Oak => OAK_TRAPDOOR,
            DoorStyle::Spruce => SPRUCE_TRAPDOOR,
            DoorStyle::DarkOak => DARK_OAK_TRAPDOOR,
            DoorStyle::Birch => BIRCH_TRAPDOOR,
            DoorStyle::Iron => IRON_TRAPDOOR,
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
    let wants_double = (config.ground == GroundFloor::Lobby
        || matches!(
            config.category,
            BuildingCategory::Commercial
                | BuildingCategory::Office
                | BuildingCategory::Hotel
                | BuildingCategory::School
                | BuildingCategory::Hospital
        ))
        && seg.len >= 8
        && (seg.tangent.0 == 0 || seg.tangent.1 == 0);

    // The door takes a window bay of the ground floor, the shop's or the
    // storey's above, so it lines up with the facade; nearest the target
    // first, dodging passages and party columns and keeping 2 columns clear
    // of the ends.
    let columns: Vec<WallColumn> = facade_columns(&element.nodes, config)
        .into_iter()
        .filter(|c| c.seg == seg_index)
        .collect();
    let usable = |idx: usize| {
        let (bx, bz) = points[idx];
        (2..points.len() - 2).contains(&idx)
            && !building_passages.contains(bx, bz)
            && !facade.is_party(bx, bz)
    };
    let bay_door = |idx: usize, leaves: i32| -> bool {
        let Some(wc) = columns.get(idx) else {
            return false;
        };
        let slot = if config.shopfront_column(wc.facade(facade)) {
            wc.shop_slot()
        } else {
            wc.slot()
        };
        matches!(slot, BaySlot::Window { k, width, .. } if k + leaves <= width)
    };
    let reach = points.len() as isize / 2;
    let nearest = |accept: &dyn Fn(usize) -> bool| -> Option<usize> {
        (0..=reach)
            .flat_map(|d| [d, -d])
            .map(|d| target as isize + d)
            .filter(|&i| i >= 0 && (i as usize) < points.len())
            .map(|i| i as usize)
            .find(|&i| usable(i) && accept(i))
    };
    let (idx, double) = if let Some(i) = nearest(&|i| wants_double && bay_door(i, 2)) {
        (i, true)
    } else if let Some(i) = nearest(&|i| bay_door(i, 1)) {
        (i, false)
    } else {
        (nearest(&|_| true)?, false)
    };
    let (x, z) = points[idx];

    let style = if config.ground == GroundFloor::Lobby {
        DoorStyle::Iron
    } else {
        door_style_for(config.category, config.wall_block, group_seed)
    };
    let mut dress_rng = element_rng(group_seed ^ 0xD00E_57E9_0000_0013);
    // A shopfront has its awnings instead.
    let canopy = (double
        || matches!(
            config.category,
            BuildingCategory::Commercial | BuildingCategory::Hotel
        ))
        && config.ground != GroundFloor::Shopfront
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

/// A street door for each unit of a shared ground floor that has none, on the wall
/// in front of the unit nearest its anchor.
fn plan_unit_entrances(
    element: &ProcessedWay,
    config: &BuildingConfig,
    facade: &FacadePlan,
    building_passages: &CoordinateBitmap,
    group_seed: u64,
    anchors: &[(i32, i32)],
    existing: &[EntrancePlan],
) -> Vec<EntrancePlan> {
    if !config.is_ground_level
        || config.has_garage_door
        || config.has_single_door
        || matches!(
            config.condition,
            BuildingCondition::Construction | BuildingCondition::Ruined
        )
        || facade.segments.is_empty()
    {
        return Vec::new();
    }
    let unit_of = |x: i32, z: i32| -> usize {
        anchors
            .iter()
            .enumerate()
            .min_by_key(|(_, a)| {
                let (dx, dz) = ((x - a.0) as i64, (z - a.1) as i64);
                dx * dx + dz * dz
            })
            .map(|(k, _)| k)
            .unwrap_or(0)
    };
    let mut doors: Vec<(i32, i32)> = existing.iter().map(|p| (p.x, p.z)).collect();
    let mut served = vec![false; anchors.len()];
    for p in existing {
        served[unit_of(p.x - p.normal.0, p.z - p.normal.1)] = true;
    }
    let style = door_style_for(config.category, config.wall_block, group_seed);
    let mut out = Vec::new();
    for (unit, anchor) in anchors.iter().enumerate() {
        if served[unit] {
            continue;
        }
        // Street walls first, then the nearest column to the anchor.
        // (wall rank, distance squared), door column, segment.
        type Pick = ((u8, i64), (i32, i32), usize);
        let mut best: Option<Pick> = None;
        for (i, seg) in facade.segments.iter().enumerate() {
            let Some(seg) = seg else {
                continue;
            };
            let rank = match seg.class {
                FacadeClass::Street => 0,
                FacadeClass::Open => 1,
                FacadeClass::Rear => 2,
                FacadeClass::Party => continue,
            };
            let (Some(a), Some(b)) = (element.nodes.get(i), element.nodes.get(i + 1)) else {
                continue;
            };
            let points: Vec<(i32, i32)> = bresenham_line(a.x, 0, a.z, b.x, 0, b.z)
                .into_iter()
                .map(|(x, _, z)| (x, z))
                .collect();
            if seg.len < 4 || points.len() < 5 {
                continue;
            }
            for &(x, z) in &points[2..points.len() - 2] {
                if building_passages.contains(x, z) || facade.is_party(x, z) {
                    continue;
                }
                if unit_of(x - seg.normal.0, z - seg.normal.1) != unit {
                    continue;
                }
                if doors
                    .iter()
                    .any(|&(dx, dz)| (dx - x).abs() + (dz - z).abs() < 3)
                {
                    continue;
                }
                let d = ((x - anchor.0) as i64).pow(2) + ((z - anchor.1) as i64).pow(2);
                if best.as_ref().is_none_or(|(k, _, _)| (rank, d) < *k) {
                    best = Some(((rank, d), (x, z), i));
                }
            }
        }
        if let Some((_, (x, z), i)) = best {
            let Some(seg) = facade.segments[i].as_ref() else {
                continue;
            };
            doors.push((x, z));
            out.push(EntrancePlan {
                x,
                z,
                normal: seg.normal,
                tangent: seg.tangent,
                double: false,
                style,
                canopy: false,
                lantern: false,
            });
        }
    }
    out
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
            (Some(a), Some(b)) => Some(if a.len >= b.len {
                (a, true)
            } else {
                (b, false)
            }),
            (Some(a), None) => Some((a, true)),
            (None, Some(b)) => Some((b, false)),
            (None, None) => None,
        };
        let Some((seg, ends_here)) = seg else {
            continue;
        };
        // The second leaf steps along the wall, so on a segment that ends at
        // this node it steps back into it, not past the corner.
        let tangent = if ends_here {
            (-seg.tangent.0, -seg.tangent.1)
        } else {
            seg.tangent
        };
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
            tangent,
            double,
            style,
            canopy: false,
            lantern: false,
        });
    }
    plans
}

/// The longest doorstep run outside a door: three steps, one block of descent
/// each. A deeper drop gets no steps at all rather than a longer flight, which
/// in front of every downhill door would read as a ramp.
const DOORSTEP_MAX_STEPS: i32 = 3;

/// The cells and heights of the doorstep run outside a door, or `None` when the
/// run cannot land on the ground.
///
/// A building's floor is `calculate_start_y_offset`, the *highest* terrain level
/// under its outline, so on a slope the door on the downhill side sits well
/// above the ground in front of it. The run descends one block per cell and so
/// bridges at most `DOORSTEP_MAX_STEPS` of that; anything deeper used to get a
/// truncated flight whose last step stopped two or three blocks up in the air.
///
/// The terrain under each step decides, not one probe in front of the door:
/// what the run has to reach is the ground beneath the steps themselves, and on
/// a slope that is not the ground at the threshold.
fn plan_doorstep_steps(
    editor: &WorldEditor,
    door: (i32, i32),
    normal: (i32, i32),
    floor_y: i32,
) -> Option<Vec<((i32, i32), i32)>> {
    let (nx, nz) = normal;
    // No elevation data means no drop to bridge.
    let out_ground = editor.terrain_level(door.0 + nx, door.1 + nz)?;
    let drop = floor_y - out_ground;
    // Nothing to bridge below one, and past the reach the run cannot land.
    if !(1..=DOORSTEP_MAX_STEPS).contains(&drop) {
        return None;
    }
    let mut steps = Vec::with_capacity(drop as usize);
    for step in 1..=drop {
        let cell = (door.0 + nx * step, door.1 + nz * step);
        let step_y = floor_y - step + 1;
        // The first cell already carries the threshold block, so its step is
        // never written and only the terrain past it can leave one hanging.
        if step > 1 && editor.terrain_level(cell.0, cell.1)? < step_y - 1 {
            return None;
        }
        steps.push((cell, step_y));
    }
    Some(steps)
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

        // Doorstep stairs bridging a terrain drop in front of the door, but
        // only where the run lands on the ground: see `plan_doorstep_steps`.
        if args.terrain() && config.is_ground_level {
            if let Some(steps) =
                plan_doorstep_steps(editor, (*dx, *dz), (nx, nz), config.start_y_offset)
            {
                let stair_base = get_stair_block_for_material(threshold);
                // Ascend toward the door: high side of each step faces inward.
                let stair_facing = match facing_for_normal(-nx, -nz) {
                    "north" => StairFacing::North,
                    "south" => StairFacing::South,
                    "east" => StairFacing::East,
                    _ => StairFacing::West,
                };
                for ((sx, sz), step_y) in steps {
                    let stair = create_stair_with_properties(
                        stair_base,
                        stair_facing,
                        StairShape::Straight,
                    );
                    editor.set_block_with_properties_absolute(
                        stair,
                        sx,
                        step_y + config.abs_terrain_offset,
                        sz,
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
    // Lantern: hung from a small bracket over the door, never standing on air.
    if plan.lantern {
        let (lx, lz) = (plan.x + nx, plan.z + nz);
        editor.set_block_with_properties_absolute(
            make_closed_trapdoor(plan.style.trapdoor_block(), facing, "top"),
            lx,
            config.start_y_offset + 4 + config.abs_terrain_offset,
            lz,
            Some(&[AIR]),
            None,
        );
        editor.set_block_with_properties_absolute(
            make_prop_block(LANTERN, &[("hanging", "true")]),
            lx,
            config.start_y_offset + 3 + config.abs_terrain_offset,
            lz,
            Some(&[AIR]),
            None,
        );
    }
}

/// One column of a wall ring as the facade passes walk it. A vertex column is
/// visited once for each segment that ends or starts on it, like the wall is.
#[derive(Clone, Copy)]
struct WallColumn {
    x: i32,
    z: i32,
    /// Ring segment walked: `nodes[seg]` to `nodes[seg + 1]`.
    seg: usize,
    /// Axis-snapped outward normal of that segment, (0, 0) when degenerate.
    normal: (i32, i32),
    /// Along-wall ordinate, for the patterns that keep a world lattice.
    u: i32,
    /// Straight wall run the column belongs to, numbered along the ring.
    run: usize,
    /// Position along that run.
    pos: i32,
    /// Window layout of that run: upper floors, then shopfront.
    upper: BayLayout,
    shop: BayLayout,
}

impl WallColumn {
    #[inline]
    fn slot(&self) -> BaySlot {
        self.upper.slot(self.pos)
    }

    #[inline]
    fn shop_slot(&self) -> BaySlot {
        self.shop.slot(self.pos)
    }

    /// Upper-floor slot `d` columns further along the run; past either end
    /// of the run is its end pier.
    fn slot_at(&self, d: i32) -> BaySlot {
        let p = self.pos + d;
        if p < 0 || p >= self.upper.len {
            BaySlot::End
        } else {
            self.upper.slot(p)
        }
    }

    /// Within one block of where this run meets the next. A run too short to
    /// have a middle has no corners at all, or it would have no windows.
    fn corner(&self) -> bool {
        self.upper.len >= 5 && (self.pos == 0 || self.pos == self.upper.len - 1)
    }

    fn facade(&self, facade: &FacadePlan) -> ColumnFacade {
        ColumnFacade {
            wall_u: self.u,
            party: facade.is_party(self.x, self.z),
            street: !facade.has_any_street || facade.is_street(self.x, self.z),
            corner: self.corner(),
            slot: self.slot(),
            shop: self.shop_slot(),
        }
    }
}

/// Headings closer than this cosine (about 14 degrees) continue one wall run.
const RUN_HEADING_COS: f64 = 0.97;

/// Every column of a ring, segment by segment, placed on its straight wall
/// run. Consecutive segments that keep their heading form one run, so a node
/// part way along a facade, where a neighbour's party wall or a mapped
/// entrance meets it, does not break the bays.
fn facade_columns(nodes: &[ProcessedNode], config: &BuildingConfig) -> Vec<WallColumn> {
    let n_seg = nodes.len().saturating_sub(1);
    if n_seg == 0 {
        return Vec::new();
    }
    let outward = outward_side(nodes);
    let points: Vec<Vec<(i32, i32)>> = (0..n_seg)
        .map(|i| {
            let (a, b) = (&nodes[i], &nodes[i + 1]);
            bresenham_line(a.x, 0, a.z, b.x, 0, b.z)
                .into_iter()
                .map(|(x, _, z)| (x, z))
                .collect()
        })
        .collect();
    let heading = |i: usize| -> Option<(f64, f64)> {
        let (dx, dz) = (
            (nodes[i + 1].x - nodes[i].x) as f64,
            (nodes[i + 1].z - nodes[i].z) as f64,
        );
        let len = dx.hypot(dz);
        (len > 0.0).then(|| (dx / len, dz / len))
    };
    let parallel = |a: Option<(f64, f64)>, b: Option<(f64, f64)>| match (a, b) {
        (Some(a), Some(b)) => a.0 * b.0 + a.1 * b.1 > RUN_HEADING_COS,
        // A zero-length edge has no heading and joins whatever it touches.
        _ => true,
    };

    // Runs as lists of consecutive segments.
    let mut runs: Vec<Vec<usize>> = Vec::new();
    let mut last: Option<(f64, f64)> = None;
    for i in 0..n_seg {
        let here = heading(i);
        if runs.is_empty() || !parallel(last, here) {
            runs.push(Vec::new());
        }
        runs.last_mut().unwrap().push(i);
        if here.is_some() {
            last = here;
        }
    }
    // A closed ring whose seam falls mid-wall continues its last run into
    // its first.
    let closed = nodes.len() >= 3 && nodes[0].x == nodes[n_seg].x && nodes[0].z == nodes[n_seg].z;
    if closed && runs.len() > 1 {
        let first_heading = (0..n_seg).find_map(heading);
        if parallel(last, first_heading) {
            let mut tail = runs.pop().unwrap();
            tail.extend(runs[0].iter().copied());
            runs[0] = tail;
        }
    }

    // Position of every point along its run; a vertex shared by two segments
    // of one run takes one position.
    let mut placed: Vec<Vec<(usize, i32)>> = points.iter().map(|p| vec![(0, 0); p.len()]).collect();
    let mut run_len: Vec<i32> = Vec::with_capacity(runs.len());
    for (r, segs) in runs.iter().enumerate() {
        let mut pos = 0;
        for (j, &seg) in segs.iter().enumerate() {
            for (idx, slot) in placed[seg].iter_mut().enumerate() {
                if j > 0 && idx == 0 {
                    *slot = (r, pos - 1);
                    continue;
                }
                *slot = (r, pos);
                pos += 1;
            }
        }
        run_len.push(pos);
    }
    let layouts: Vec<(BayLayout, BayLayout)> =
        run_len.iter().map(|&len| config.run_layouts(len)).collect();

    let mut out = Vec::with_capacity(points.iter().map(Vec::len).sum());
    for seg in 0..n_seg {
        let (a, b) = (&nodes[seg], &nodes[seg + 1]);
        let normal = compute_outward_normal(a.x, a.z, b.x, b.z, outward);
        let seg_axis_x = segment_axis_x(a.x, a.z, b.x, b.z);
        let start_axis_x = segment_start_axis_x(nodes, seg + 1, seg_axis_x);
        for (idx, &(x, z)) in points[seg].iter().enumerate() {
            let (run, pos) = placed[seg][idx];
            out.push(WallColumn {
                x,
                z,
                seg,
                normal,
                u: wall_ordinate(idx, start_axis_x, seg_axis_x, x, z),
                run,
                pos: pos.max(0),
                upper: layouts[run].0,
                shop: layouts[run].1,
            });
        }
    }
    out
}

/// Builds a wall ring (outer shell or inner courtyard) for a set of nodes.
///
/// With `floor_cells`, the building's own floor, glass is set a block behind
/// the wall face wherever the cell behind the column is floor, so a window
/// reads as an opening in a solid wall rather than a pane painted on it.
#[allow(clippy::too_many_arguments)]
fn build_wall_ring(
    editor: &mut WorldEditor,
    nodes: &[ProcessedNode],
    config: &BuildingConfig,
    args: &Args,
    has_sloped_roof: bool,
    building_passages: &CoordinateBitmap,
    facade: &FacadePlan,
    floor_cells: Option<&FnvHashSet<(i32, i32)>>,
) -> (Vec<(i32, i32)>, i32, Vec<WallSegmentKind>) {
    // Count of generated wall coordinates; the caller only needs to know the ring is non-empty.
    let mut corner_count: i32 = 0;
    let mut current_building: Vec<(i32, i32)> = Vec::new();
    let mut segments: Vec<WallSegmentKind> =
        vec![WallSegmentKind::default(); nodes.len().saturating_sub(1)];

    let passage_height = BUILDING_PASSAGE_HEIGHT.min(config.building_height);
    let columns = facade_columns(nodes, config);
    let ring_cells: FnvHashSet<(i32, i32)> = columns.iter().map(|c| (c.x, c.z)).collect();

    for wc in &columns {
        let (bx, bz) = (wc.x, wc.z);
        let kind = &mut segments[wc.seg];
        // Passages only apply to ground-level buildings; elevated
        // building:part elements (min_level > 0) receive an empty bitmap
        // via effective_passages, so this is always false for them.
        let is_passage = building_passages.contains(bx, bz);

        // Foundation pillars below terrain. Skipped in passage zones.
        if args.terrain() && config.is_ground_level && !is_passage {
            let local_ground_level = editor.terrain_level(bx, bz).unwrap_or(args.ground_level);

            for y in local_ground_level..config.start_y_offset + 1 {
                let block = apply_block_variety(config.wall_block, bx, y, bz, config);
                editor.set_block_absolute(block, bx, y + config.abs_terrain_offset, bz, None, None);
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
            let local =
                config.start_y_offset + ((config.building_height as f64) * factor).round() as i32;
            local
                .max(config.start_y_offset + 1)
                .min(config.start_y_offset + config.building_height - 1)
        } else {
            config.start_y_offset + config.building_height
        };

        let col = wc.facade(facade);
        // Resolved once per column rather than per block: it is a
        // lookup in the facade store, and the whole column shares it.
        let photo = config.is_photo_column(bx, bz);
        // The wall follows whichever source is going to cover it;
        // `photographed` stays Mapillary's own, because the facade
        // diagnostic below counts photographed segments.
        let flat = config.is_flat_column(bx, bz);
        kind.photographed |= photo;
        // The cell behind the column takes the glass when it is open floor
        // and no wall of its own; never through a passage, party wall or a
        // photograph's shell.
        let inner = (bx - wc.normal.0, bz - wc.normal.1);
        let recess = config.recess_windows
            && wc.normal != (0, 0)
            && !flat
            && !col.party
            && !is_passage
            && floor_cells.is_some_and(|f| f.contains(&inner) && !f.contains(&(bx, bz)))
            && !ring_cells.contains(&inner);
        // A door stands in the wall plane, so its column keeps the ground
        // floor there too.
        let door_top = if facade.is_door(bx, bz) {
            config.ground_floor_top()
        } else {
            i32::MIN
        };
        for h in wall_start..=column_top {
            let block = determine_wall_block_at_position(bx, h, bz, config, col, flat);
            let glazed = !flat && (block == config.window_block || block == GLASS);
            kind.windowed |= glazed;
            let (x, z) = if recess && h > door_top && config.set_back(h, col, block, glazed) {
                inner
            } else {
                (bx, bz)
            };
            editor.set_block_absolute(block, x, h + config.abs_terrain_offset, z, None, None);
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
            // An accent line over a photographed wall belongs to a
            // building the photograph does not show, so the shell
            // keeps its own colour up here too, and so does a wall
            // the presets flatten for a picture of their own.
            let roof_line_block = if config.use_accent_roof_line && !photo && !flat {
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
        }

        current_building.push((bx, bz));
        corner_count += 1;
    }

    // Photo panels are recorded once the wall exists, since how tall it was
    // built decides the panel. They are placed at the end of the run, once
    // every later block is in and the finished world can be checked.
    if crate::mapillary::facades::displays_enabled() {
        crate::mapillary::displays::collect(
            editor,
            config.element_id,
            config.start_y_offset,
            config.abs_terrain_offset,
            config.building_height,
        );
    }
    // The preset facades are not recorded here but in `generate_buildings`,
    // before this runs: their panels decide which columns are built as a plain
    // shell, and this function has already built them by now.

    (current_building, corner_count, segments)
}

/// The wall columns of `element`'s ring that a facade photograph covers, and
/// so are built as a plain shell for the texture to override. Empty unless
/// the export gives this building a block grid or a photo panel: below two
/// blocks per metre without panels it supplies floor band colours only, and
/// those already apply per column through `band_block_at`.
///
/// A column carrying a facade cell, plus its two neighbours along the same
/// ring segment. The dilation is one block of rounding, not a coverage claim:
/// the export numbers its columns in metres along its own extent, so the
/// block that straddles the end of a texture falls just past the last column
/// and drops out. Without it a photographed wall could end in a single
/// procedural window standing alone against the corner. Taking the columns as
/// a set also settles the corner block two ring segments share: it is decided
/// the same way whichever segment is being drawn, so a photographed wall
/// meeting a normal one cannot leave a gap or a doubled block there.
fn photo_shell_columns(element: &ProcessedWay) -> Arc<FnvHashSet<(i32, i32)>> {
    let mut shell = FnvHashSet::default();
    if !crate::mapillary::facades::has_building(element.id)
        || (crate::mapillary::facades::colour_only()
            && !crate::mapillary::facades::displays_enabled())
    {
        return Arc::new(shell);
    }
    let mut previous: Option<(i32, i32)> = None;
    for node in &element.nodes {
        if let Some((px, pz)) = previous {
            let points = bresenham_line(px, 0, pz, node.x, 0, node.z);
            for (i, &(bx, _, bz)) in points.iter().enumerate() {
                if !crate::mapillary::facades::photo_column(bx, bz, element.id) {
                    continue;
                }
                for near in &points[i.saturating_sub(1)..(i + 2).min(points.len())] {
                    shell.insert((near.0, near.2));
                }
            }
        }
        previous = Some((node.x, node.z));
    }
    Arc::new(shell)
}

/// What one ring segment of a facade building turned out to be, for the
/// `ARNIS_FACADE_WALL_STATS` line: whether a photograph covers any of it, and
/// whether the procedural pass put a window on it. A segment can be neither
/// (a blank wall, a party wall, a windowless style) and, where a photographed
/// wall ends part way along a ring segment, both.
#[derive(Clone, Copy, Default)]
struct WallSegmentKind {
    photographed: bool,
    windowed: bool,
}

/// Prints how one facade building's ring split between the walls a photograph
/// covers and the walls a normal Arnis building was generated on, and audits
/// every corner where the two meet, so both can be counted over a whole area
/// without reading the world back. Off unless `ARNIS_FACADE_WALL_STATS` is
/// set, since it is one line per building.
///
/// Segments are what the export calls a wall, but a corner block belongs to
/// two of them, so `cols`, `shell_cols` and `window_cols` count the ring's
/// world columns instead, each exactly once, which is the honest denominator.
///
/// `seams` counts the ring vertices a photographed segment shares with one
/// that is not, which are the corners the texture stops short of: a corner it
/// reaches is shell, and a shell corner belongs to both its segments, so both
/// count as photographed and it is not a seam at all. `seam_gaps` counts the
/// seam columns the wall left a hole in and has to read zero. `seam_orphan`
/// counts the ones that touch a shell column without being one, which is the
/// first block past the shell's single block of rounding allowance rather
/// than a defect, so a few are expected on an area of any size.
fn report_wall_segments(
    editor: &WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    passages: &CoordinateBitmap,
    segments: &[WallSegmentKind],
) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("ARNIS_FACADE_WALL_STATS").is_some()) {
        return;
    }
    if !crate::mapillary::facades::has_building(config.element_id) || segments.len() < 2 {
        return;
    }
    let photo_segs = segments.iter().filter(|s| s.photographed).count();
    let window_segs = segments.iter().filter(|s| s.windowed).count();

    // Does any block of this wall column read as a window? On a shell column
    // that is the photograph's own window cell, everywhere else the
    // procedural one.
    let has_window = |x: i32, z: i32| {
        ((config.start_y_offset + 1)..=(config.start_y_offset + config.building_height)).any(|h| {
            editor
                .get_block_absolute(x, h + config.abs_terrain_offset, z)
                .is_some_and(|b| b.id() == config.window_block.id() || b.id() == GLASS.id())
        })
    };

    let mut ring: FnvHashSet<(i32, i32)> = FnvHashSet::default();
    let mut previous: Option<(i32, i32)> = None;
    for node in &element.nodes {
        if let Some((px, pz)) = previous {
            for (bx, _, bz) in bresenham_line(px, 0, pz, node.x, 0, node.z) {
                ring.insert((bx, bz));
            }
        }
        previous = Some((node.x, node.z));
    }
    let shell_cols = ring
        .iter()
        .filter(|c| config.facade_shell.contains(c))
        .count();
    let window_cols = ring
        .iter()
        .filter(|c| !config.facade_shell.contains(*c) && has_window(c.0, c.1))
        .count();

    let (mut seams, mut seam_gaps, mut seam_orphan) = (0, 0, 0);
    // Closed rings only, so that the last segment really does meet the first.
    let ends = |n: Option<&ProcessedNode>| n.map(|n| (n.x, n.z));
    let closed = ends(element.nodes.first()) == ends(element.nodes.last());
    for (i, kind) in segments.iter().enumerate() {
        let next = (i + 1) % segments.len();
        if (next == 0 && !closed) || kind.photographed == segments[next].photographed {
            continue;
        }
        // Segment i runs nodes[i] to nodes[i + 1], so the two share nodes[i + 1],
        // which for the closing pair is the repeated first node.
        let node = &element.nodes[i + 1];
        if passages.contains(node.x, node.z) {
            continue;
        }
        seams += 1;
        if !config.facade_shell.contains(&(node.x, node.z))
            && [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .any(|(dx, dz)| config.facade_shell.contains(&(node.x + dx, node.z + dz)))
        {
            seam_orphan += 1;
        }
        let gap = ((config.start_y_offset + 1)..=(config.start_y_offset + config.building_height))
            .any(|h| {
                editor
                    .get_block_absolute(node.x, h + config.abs_terrain_offset, node.z)
                    .is_none()
            });
        seam_gaps += i32::from(gap);
    }
    println!(
        "facade-walls id={} segments={} photo_segs={photo_segs} window_segs={window_segs} \
         cols={} shell_cols={shell_cols} window_cols={window_cols} \
         seams={seams} seam_gaps={seam_gaps} seam_orphan={seam_orphan}",
        config.element_id,
        segments.len(),
        ring.len(),
    );
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

                // Skip placing doors inside a building passage, and on a
                // photographed wall, which carries the door the photograph
                // shows and not a procedural one.
                if building_passages.contains(door1_x, door1_z)
                    || building_passages.contains(door2_x, door2_z)
                    || config.is_photo_column(door1_x, door1_z)
                    || config.is_photo_column(door2_x, door2_z)
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

            // Skip placing a door inside a building passage or on a
            // photographed wall, which carries its own door.
            if !building_passages.contains(door_x, door_z)
                && !config.is_photo_column(door_x, door_z)
            {
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
    flat: bool,
) -> Block {
    // A column with a photograph coming over it is a plain shell: the facade
    // cell or panel above it decides what shows, and a procedural window or
    // accent line underneath would come through wherever the picture has no
    // data. `flat` is `config.is_flat_column(bx, bz)`, hoisted out of the
    // height loop.
    let chosen = if flat {
        config.wall_block
    } else {
        determine_wall_block_at_position_pristine(h, config, col)
    };
    let chosen = apply_block_variety(chosen, bx, h, bz, config);
    apply_condition_variation(chosen, bx, h, bz, config)
}

/// Substitutes the wall block with a same-family alternative for variety.
fn apply_block_variety(chosen: Block, bx: i32, h: i32, bz: i32, config: &BuildingConfig) -> Block {
    // From two blocks per metre up a facade texture cell wins over every
    // procedural choice for this position.
    if let Some(block) = crate::mapillary::facades::block_at(
        bx,
        h,
        bz,
        config.start_y_offset,
        config.element_id,
        config.window_block,
    ) {
        return block;
    }
    // Only the plain wall material is varied or recoloured below. Windows,
    // accent lines, plinth courses and storefront glass come back from
    // `determine_wall_block_at_position_pristine` as other blocks and are
    // kept as they are; pilasters and the other wall depth features never
    // pass through here.
    if chosen != config.wall_block {
        return chosen;
    }
    // Below two blocks per metre the lab supplies colours only: a plain wall
    // block takes the colour of the lab's floor band at this height, flat,
    // and keeps its procedural block above the textured rows. Scaffolding on
    // a construction site is not a wall colour.
    if config.condition != BuildingCondition::Construction {
        if let Some(block) = crate::mapillary::facades::band_block_at(
            bx,
            h,
            bz,
            config.start_y_offset,
            config.element_id,
        ) {
            return block;
        }
    }
    let pool = substitute_pool_only(chosen);
    if pool.is_empty() {
        return chosen;
    }

    // A fifth of the buildings keep one flat field.
    let mut variety_mode_rng = element_rng(config.element_id ^ 0xBABE_F1A1_2222_8888);
    if variety_mode_rng.random_bool(0.20) {
        return chosen;
    }

    // Weathering comes in patches of a same-tone sibling, three wide and two
    // high, rather than as speckle on single blocks. Each band of patches is
    // offset like a brick course so they do not line up in columns.
    let band = h.div_euclid(2);
    let shift = band.rem_euclid(3);
    let mut patch_rng = coord_rng(
        (bx + shift).div_euclid(3),
        (bz + shift).div_euclid(3),
        config.element_id ^ 0x5050_3030_AAFF_BBCC ^ ((band as u64) << 20),
    );
    if patch_rng.random_range(0u32..100) < 12 {
        pool[patch_rng.random_range(0..pool.len())]
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

// ----------------------------------------------------------------------------
// Pitched-roof coverings. A family is a full block (hidden fill, ridge, mansard
// face) whose stair and slab (`get_stair_block_for_material`,
// `get_slab_block_for_material`) carry the slope:
//   tile red     red terracotta, brick stairs: bright clay tile in courses
//   tile brick   bricks: uniform salmon-red tile
//   tile dark    mangrove planks: dark red Biberschwanz
//   tile brown   nether bricks: old dark red-brown tile
//   tile orange  terracotta, granite stairs: orange-brown tile
//   slate        cobbled deepslate: mid-dark grey slate, asphalt shingle
//   slate dark   deepslate tiles
//   glazed black polished blackstone bricks
//   zinc         polished andesite: light blue-grey sheet
//   copper       oxidized copper
//   metal green  dark prismarine: green-painted sheet iron
//   metal red    red nether bricks: maroon-painted sheet iron
// ----------------------------------------------------------------------------

const TILE_RED: Block = RED_TERRACOTTA;
const TILE_BRICK: Block = BRICK;
const TILE_DARK: Block = MANGROVE_PLANKS;
const TILE_BROWN: Block = NETHER_BRICK;
const TILE_ORANGE: Block = TERRACOTTA;
const SLATE: Block = COBBLED_DEEPSLATE;
const SLATE_DARK: Block = DEEPSLATE_TILES;
const GLAZED_BLACK: Block = POLISHED_BLACKSTONE_BRICKS;
const ZINC: Block = POLISHED_ANDESITE;
const COPPER_ROOF: Block = WAXED_OXIDIZED_COPPER;
const METAL_GREEN: Block = DARK_PRISMARINE;
const METAL_RED: Block = RED_NETHER_BRICKS;

/// The clay tile families, which `roof:material=tile` and warm `roof:colour`
/// values choose among.
const TILE_FAMILIES: [Block; 5] = [TILE_RED, TILE_BRICK, TILE_DARK, TILE_BROWN, TILE_ORANGE];

type RoofPool = &'static [(Block, u8)];

/// Bavaria, Austria, Switzerland, Berlin: bright clay tile, some slate.
const CENTRAL_ROOFS: RoofPool = &[
    (TILE_RED, 55),
    (TILE_BRICK, 10),
    (TILE_BROWN, 12),
    (TILE_ORANGE, 5),
    (SLATE, 13),
];

/// Saxony, Thuringia, Bohemia, Silesia: darker red Biberschwanz tile.
const CENTRAL_EAST_ROOFS: RoofPool = &[
    (TILE_DARK, 30),
    (TILE_RED, 25),
    (TILE_BRICK, 15),
    (TILE_ORANGE, 10),
    (SLATE, 15),
];

/// Low Countries and the north coast: red-orange tile, dark glazed tile.
const BRICK_NORTH_ROOFS: RoofPool = &[
    (TILE_BRICK, 35),
    (TILE_RED, 15),
    (TILE_ORANGE, 15),
    (SLATE_DARK, 25),
    (GLAZED_BLACK, 10),
];

/// Britain and Ireland: Welsh slate first, red tile second.
const BRITISH_ROOFS: RoofPool = &[(SLATE_DARK, 65), (TILE_BRICK, 25), (TILE_ORANGE, 10)];

/// France: zinc and slate in the north and the cities, tile elsewhere. Paris
/// zinc is a mid blue-grey, so the pale zinc leads and dark slate stays rare.
const FRANCE_ROOFS: RoofPool = &[
    (ZINC, 50),
    (SLATE, 20),
    (SLATE_DARK, 15),
    (TILE_RED, 10),
    (TILE_ORANGE, 5),
];

/// Southern Europe and Turkey: orange canal tile.
const MEDITERRANEAN_ROOFS: RoofPool = &[
    (TILE_ORANGE, 45),
    (POLISHED_GRANITE, 15),
    (TILE_BRICK, 25),
    (TILE_RED, 10),
    (MUD_BRICKS, 5),
];

/// Scandinavia: black and dark grey, or red.
const NORDIC_ROOFS: RoofPool = &[
    (TILE_BRICK, 30),
    (TILE_RED, 10),
    (GLAZED_BLACK, 25),
    (SLATE_DARK, 25),
    (TILE_BROWN, 10),
];

/// North American houses: asphalt shingle in grey, black and brown, cedar,
/// now and then a clay tile or green metal roof.
const NA_HOUSE_ROOFS: RoofPool = &[
    (SLATE, 28),
    (SLATE_DARK, 16),
    (GLAZED_BLACK, 12),
    (DARK_OAK_PLANKS, 14),
    (SPRUCE_PLANKS, 8),
    (MUD_BRICKS, 6),
    (TILE_BRICK, 5),
    (TILE_ORANGE, 4),
    (METAL_GREEN, 3),
];

/// Sunbelt houses: Spanish clay tile beside asphalt shingle.
const SUNBELT_ROOFS: RoofPool = &[
    (TILE_ORANGE, 22),
    (TILE_RED, 8),
    (TILE_BRICK, 10),
    (SLATE, 24),
    (SLATE_DARK, 12),
    (MUD_BRICKS, 10),
    (DARK_OAK_PLANKS, 7),
    (SPRUCE_PLANKS, 7),
];

/// Pitched roofs on North American row houses, churches and civic buildings.
const NA_URBAN_ROOFS: RoofPool = &[
    (SLATE_DARK, 45),
    (SLATE, 20),
    (GLAZED_BLACK, 20),
    (TILE_BRICK, 15),
];

/// Everywhere else: tile, slate and shingle.
const OTHER_ROOFS: RoofPool = &[
    (TILE_BRICK, 30),
    (TILE_ORANGE, 15),
    (TILE_RED, 5),
    (SLATE, 20),
    (SLATE_DARK, 15),
    (ZINC, 10),
    (DARK_OAK_PLANKS, 5),
];

/// Canadian houses: asphalt shingle, Quebec's bright steel roofs.
const CANADA_HOUSE_ROOFS: RoofPool = &[
    (SLATE, 28),
    (SLATE_DARK, 18),
    (GLAZED_BLACK, 14),
    (DARK_OAK_PLANKS, 10),
    (SPRUCE_PLANKS, 6),
    (ZINC, 10),
    (MUD_BRICKS, 6),
    (METAL_GREEN, 4),
    (TILE_BRICK, 4),
];

/// Russia and its neighbours: painted sheet iron in green, red and grey,
/// grey asbestos-cement sheet, newer metal tile.
const EAST_ROOFS: RoofPool = &[
    (ZINC, 22),
    (METAL_GREEN, 18),
    (METAL_RED, 12),
    (TILE_DARK, 8),
    (SLATE, 14),
    (TILE_RED, 12),
    (TILE_BRICK, 8),
    (TILE_BROWN, 6),
];

/// The odd pitched roof of North Africa and the Middle East: clay tile and
/// sheet.
const MIDDLE_EAST_ROOFS: RoofPool = &[
    (TILE_ORANGE, 35),
    (TILE_BRICK, 25),
    (ZINC, 20),
    (TILE_RED, 10),
    (SLATE, 10),
];

/// Corrugated sheet, bare and painted red, green and maroon, beside clay
/// and concrete tile.
const AFRICA_ROOFS: RoofPool = &[
    (ZINC, 30),
    (TILE_BRICK, 15),
    (TILE_RED, 10),
    (METAL_GREEN, 10),
    (SLATE, 12),
    (TILE_ORANGE, 10),
    (METAL_RED, 8),
    (MUD_BRICKS, 5),
];

/// Mangalore clay tile and galvanised or cement sheet.
const SOUTH_ASIA_ROOFS: RoofPool = &[
    (TILE_ORANGE, 30),
    (TILE_BRICK, 20),
    (ZINC, 25),
    (SLATE, 10),
    (TILE_RED, 10),
    (METAL_GREEN, 5),
];

/// Clay and concrete tile, corrugated sheet bare or painted.
const SOUTHEAST_ASIA_ROOFS: RoofPool = &[
    (TILE_ORANGE, 25),
    (TILE_BRICK, 15),
    (ZINC, 20),
    (TILE_RED, 10),
    (SLATE, 10),
    (METAL_GREEN, 8),
    (METAL_RED, 6),
    (MUD_BRICKS, 6),
];

/// Grey clay tile, red tile in the villages, colour-painted sheet.
const EAST_ASIA_ROOFS: RoofPool = &[
    (SLATE, 30),
    (SLATE_DARK, 20),
    (GLAZED_BLACK, 10),
    (TILE_BRICK, 12),
    (TILE_ORANGE, 8),
    (ZINC, 10),
    (METAL_GREEN, 6),
    (TILE_RED, 4),
];

/// Korean roofs: dark giwa tile, green, red and orange painted sheet.
const KOREA_ROOFS: RoofPool = &[
    (SLATE_DARK, 26),
    (SLATE, 16),
    (METAL_GREEN, 18),
    (TILE_ORANGE, 12),
    (TILE_RED, 10),
    (METAL_RED, 8),
    (ZINC, 10),
];

/// Japanese houses: dark grey and black kawara, galvalume sheet, brown
/// cement shingle, the odd green sheet roof.
const JAPAN_ROOFS: RoofPool = &[
    (SLATE_DARK, 38),
    (SLATE, 34),
    (GLAZED_BLACK, 10),
    (ZINC, 10),
    (TILE_BROWN, 5),
    (METAL_GREEN, 3),
];

/// Latin America: orange clay tile, sheet over the self-built houses.
const LATAM_ROOFS: RoofPool = &[
    (TILE_ORANGE, 35),
    (TILE_BRICK, 25),
    (TILE_RED, 10),
    (ZINC, 15),
    (SLATE, 8),
    (MUD_BRICKS, 7),
];

/// The Southern Cone: French clay tile, slate and sheet.
const SOUTHERN_CONE_ROOFS: RoofPool = &[
    (TILE_BRICK, 30),
    (TILE_ORANGE, 25),
    (TILE_RED, 12),
    (SLATE, 15),
    (ZINC, 10),
    (SLATE_DARK, 8),
];

/// Australia and New Zealand: terracotta and concrete tile, corrugated steel
/// in charcoal, grey, zincalume, red and green.
const AUSTRALASIA_ROOFS: RoofPool = &[
    (TILE_ORANGE, 22),
    (TILE_BRICK, 14),
    (SLATE, 22),
    (ZINC, 16),
    (TILE_BROWN, 8),
    (METAL_RED, 6),
    (GLAZED_BLACK, 6),
    (METAL_GREEN, 4),
];

/// Spires, domes and onions: copper or slate.
const SPIRE_ROOFS: RoofPool = &[(COPPER_ROOF, 45), (SLATE_DARK, 40), (GLAZED_BLACK, 15)];

/// Mansards are zinc or slate wherever they stand; Paris leans to zinc.
const MANSARD_ROOFS: RoofPool = &[(SLATE_DARK, 50), (ZINC, 30), (SLATE, 20)];
const FRENCH_MANSARD_ROOFS: RoofPool = &[(ZINC, 50), (SLATE_DARK, 50)];

/// Factory and warehouse halls: metal sheet, fibre cement, old tile.
const INDUSTRIAL_ROOFS: RoofPool = &[(ZINC, 40), (TILE_BRICK, 20), (SLATE, 25), (SLATE_DARK, 15)];

/// Every roof pool, for the export-coverage and stair tests.
#[cfg(test)]
const ROOF_POOLS: [RoofPool; 27] = [
    CENTRAL_ROOFS,
    CENTRAL_EAST_ROOFS,
    BRICK_NORTH_ROOFS,
    BRITISH_ROOFS,
    FRANCE_ROOFS,
    MEDITERRANEAN_ROOFS,
    NORDIC_ROOFS,
    NA_HOUSE_ROOFS,
    SUNBELT_ROOFS,
    NA_URBAN_ROOFS,
    OTHER_ROOFS,
    SPIRE_ROOFS,
    MANSARD_ROOFS,
    FRENCH_MANSARD_ROOFS,
    INDUSTRIAL_ROOFS,
    CANADA_HOUSE_ROOFS,
    EAST_ROOFS,
    MIDDLE_EAST_ROOFS,
    AFRICA_ROOFS,
    SOUTH_ASIA_ROOFS,
    SOUTHEAST_ASIA_ROOFS,
    EAST_ASIA_ROOFS,
    KOREA_ROOFS,
    JAPAN_ROOFS,
    LATAM_ROOFS,
    SOUTHERN_CONE_ROOFS,
    AUSTRALASIA_ROOFS,
];

/// Share of a district's pitched roofs in its dominant covering.
const DISTRICT_ROOF_SHARE: f64 = 0.70;
/// The same outside Europe: a suburb's shingles come in several colours.
const DISTRICT_ROOF_SHARE_ELSEWHERE: f64 = 0.40;

/// The roof covering of a pitched roof, from the region's families. Each
/// district has a dominant covering that about 70% of its roofs take (40%
/// outside Europe), so an old town reads as one roofscape. Copper stays on churches, listed
/// buildings and spires.
fn pitched_roof_block(
    category: BuildingCategory,
    era: ArchEra,
    roof_type: RoofType,
    wall_block: Block,
    ctx: RoofContext,
    style_seed: u64,
) -> Block {
    use MaterialRegion as R;
    let civic = matches!(
        category,
        BuildingCategory::Religious | BuildingCategory::Historic
    );
    let spire = matches!(roof_type, RoofType::Dome | RoofType::Onion | RoofType::Cone)
        || (roof_type == RoofType::Pyramidal && category == BuildingCategory::Religious);
    // A free-standing `building=yes` under a pitched roof is a house in the
    // American suburbs (multi-storey and attached blocks are flat there).
    let house = matches!(
        category,
        BuildingCategory::House | BuildingCategory::Farm | BuildingCategory::Garage
    ) || (!ctx.attached
        && matches!(
            category,
            BuildingCategory::Default | BuildingCategory::Residential
        ));
    let pool: RoofPool = if spire {
        SPIRE_ROOFS
    } else if roof_type == RoofType::Mansard {
        if ctx.materials == R::France {
            FRENCH_MANSARD_ROOFS
        } else {
            MANSARD_ROOFS
        }
    } else if matches!(
        category,
        BuildingCategory::Industrial | BuildingCategory::Warehouse
    ) {
        INDUSTRIAL_ROOFS
    } else {
        match ctx.materials {
            R::Central => CENTRAL_ROOFS,
            R::CentralEast => CENTRAL_EAST_ROOFS,
            R::BrickNorth => BRICK_NORTH_ROOFS,
            R::BritishIsles => BRITISH_ROOFS,
            R::France => FRANCE_ROOFS,
            R::Mediterranean => MEDITERRANEAN_ROOFS,
            R::Nordic => NORDIC_ROOFS,
            R::NorthAmerica if house => NA_HOUSE_ROOFS,
            R::NorthAmericaSunbelt if house => SUNBELT_ROOFS,
            R::Canada if house => CANADA_HOUSE_ROOFS,
            R::NorthAmerica | R::NorthAmericaSunbelt | R::Canada => NA_URBAN_ROOFS,
            R::EastEurope | R::CentralAsia => EAST_ROOFS,
            R::MiddleEast => MIDDLE_EAST_ROOFS,
            R::SubSaharanAfrica => AFRICA_ROOFS,
            R::SouthAsia => SOUTH_ASIA_ROOFS,
            R::SoutheastAsia => SOUTHEAST_ASIA_ROOFS,
            R::EastAsia => EAST_ASIA_ROOFS,
            R::Korea => KOREA_ROOFS,
            R::Japan => JAPAN_ROOFS,
            R::LatinAmerica => LATAM_ROOFS,
            R::SouthernCone => SOUTHERN_CONE_ROOFS,
            R::Australasia => AUSTRALASIA_ROOFS,
            R::Other => OTHER_ROOFS,
        }
    };
    // The old European roofscapes, one covering to a quarter.
    let european =
        ctx.materials.building_region() == BuildingRegion::Europe && ctx.materials != R::EastEurope;
    let modern = matches!(era, ArchEra::PostWarPanel | ArchEra::Contemporary);
    let weights = |pool: RoofPool| -> Vec<(Block, u32)> {
        pool.iter().map(|&(b, w)| (b, u32::from(w))).collect()
    };
    let mut rng = element_rng(style_seed ^ 0x2F0F_C0DE_0000_0004);
    let roof = if spire {
        let all = weights(pool);
        all[weighted_index(all.iter().map(|&(_, w)| w), &mut rng)].0
    } else {
        let mut district_rng = element_rng(ctx.district ^ 0x2F0F_C0DE_D157_0003);
        let regional = weights(pool);
        // Old dark tile is a single roof here and there, never a quarter's
        // roofscape (in Munich about 4% of the roof surface).
        // Paris and the French north are zinc and slate; tile there is the
        // odd roof, never the quarter's.
        let french_tile = |b: Block| ctx.materials == R::France && TILE_FAMILIES.contains(&b);
        // Painted sheet is a house here and there; only Russian towns roof
        // whole quarters in green and red iron.
        let painted =
            |b: Block| ctx.materials != R::EastEurope && matches!(b, METAL_GREEN | METAL_RED);
        let dominant_weights = regional.iter().map(|&(b, w)| {
            if b == TILE_BROWN || french_tile(b) || painted(b) {
                0
            } else {
                w
            }
        });
        let dominant = regional[weighted_index(dominant_weights, &mut district_rng)].0;
        let mut own = regional;
        if civic {
            // Churches and listed buildings: more slate, some copper.
            own.push((SLATE_DARK, 20));
            own.push((COPPER_ROOF, 10));
        } else if european && modern {
            // Post-war concrete tile is mostly dark.
            own.push((SLATE, 15));
            own.push((TILE_BROWN, 10));
        }
        let share = if european {
            DISTRICT_ROOF_SHARE
        } else {
            DISTRICT_ROOF_SHARE_ELSEWHERE
        };
        let independent = civic || modern || rng.random_bool(1.0 - share);
        if independent {
            own[weighted_index(own.iter().map(|&(_, w)| w), &mut rng)].0
        } else {
            dominant
        }
    };
    roof_distinct_from_wall(roof, wall_block, ctx.materials)
}

/// Roof covering from `roof:material` and `roof:colour`, or None without
/// either. On a pitched roof the material picks the family (clay tile, slate,
/// copper) and the colour only the shade within it; a bare tile tag keeps the
/// tile the style chose, so tagged and untagged roofs of a street match, and
/// a bare colour is matched among the roof families before anything else.
/// Flat roofs keep the plain material and colour maps.
fn tagged_roof_block(
    tags: &HashMap<String, String>,
    pitched: bool,
    chosen: Option<Block>,
    rng: &mut impl Rng,
) -> Option<Block> {
    let material = tags.get("roof:material");
    let colour = tags
        .get("roof:colour")
        .and_then(|c| color_text_to_rgb_tuple(c));
    if !pitched {
        return material
            .and_then(|m| get_roof_block_for_material(m, rng))
            .or_else(|| colour.map(|rgb| crate::block_palette::roof_block_for_color(rgb, rng)));
    }
    if let Some(family) = material.and_then(|m| roof_material_family(m)) {
        return Some(match colour {
            Some(rgb) => nearest_roof_shade(family, rgb).0,
            None => chosen
                .filter(|b| family.contains(b))
                .unwrap_or_else(|| family[rng.random_range(0..family.len())]),
        });
    }
    // Painted or patinated sheet: the colour tells zinc from copper or a red
    // metal roof.
    if let (Some(rgb), true) = (colour, material.is_some_and(|m| is_sheet_metal(m))) {
        return Some(roof_family_for_colour(rgb, rng));
    }
    if let Some(block) = material.and_then(|m| get_roof_block_for_material(m, rng)) {
        return Some(block);
    }
    colour.map(|rgb| roof_family_for_colour(rgb, rng))
}

/// Whether a `roof:material` value is sheet metal, whose colour is paint or
/// patina rather than a material of its own.
fn is_sheet_metal(material: &str) -> bool {
    let normalized: String = material
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect();
    matches!(
        normalized.as_str(),
        "metal"
            | "steel"
            | "aluminium"
            | "aluminum"
            | "iron"
            | "tin"
            | "zinc"
            | "zink"
            | "lead"
            | "sheetmetal"
            | "metalsheet"
            | "metalplates"
            | "corrugated"
            | "corrugatediron"
            | "corrugatedsteel"
            | "corrugatedmetal"
    )
}

/// Roof families named by a `roof:material` value; None for the materials
/// that map to one block of their own (glass, thatch, grass, metal...).
fn roof_material_family(material: &str) -> Option<&'static [Block]> {
    let normalized: String = material
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect();
    Some(match normalized.as_str() {
        "tile" | "tiles" | "rooftile" | "rooftiles" | "claytile" | "claytiles" | "ceramic"
        | "ceramictiles" | "terracotta" | "pantiles" => &TILE_FAMILIES,
        // Concrete tile is mostly dark.
        "concretetile" | "concretetiles" | "cementtile" | "cementtiles" => &[TILE_BROWN, SLATE],
        "slate" | "slates" => &[SLATE, SLATE_DARK],
        // Old copper roofs are patina green.
        "copper" => &[COPPER_ROOF],
        _ => return None,
    })
}

/// Mean texture colour of the roof family blocks.
const ROOF_SHADES: [(Block, (u8, u8, u8)); 10] = [
    (TILE_RED, (143, 61, 46)),
    (TILE_BRICK, (150, 97, 83)),
    (TILE_DARK, (117, 54, 48)),
    (TILE_BROWN, (44, 21, 26)),
    (TILE_ORANGE, (152, 94, 67)),
    (SLATE, (77, 77, 80)),
    (SLATE_DARK, (54, 54, 55)),
    (GLAZED_BLACK, (48, 43, 50)),
    (ZINC, (132, 135, 134)),
    (COPPER_ROOF, (82, 163, 133)),
];

/// The block of `family` nearest to `rgb` (Oklab, lightness half weight, as a
/// colour tag is about hue), with that distance.
fn nearest_roof_shade(family: &[Block], rgb: (u8, u8, u8)) -> (Block, f32) {
    let target = crate::colors::oklab_components(&rgb);
    ROOF_SHADES
        .iter()
        .filter(|(b, _)| family.contains(b))
        .map(|&(b, c)| {
            let lab = crate::colors::oklab_components(&c);
            let (dl, da, db) = (0.5 * (target.0 - lab.0), target.1 - lab.1, target.2 - lab.2);
            (b, (dl * dl + da * da + db * db).sqrt())
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((family[0], 0.0))
}

/// Pitched roof for a bare `roof:colour`: greys by lightness among slate and
/// zinc, reds and browns among the clay tiles, green as copper. Only colours
/// no roof family comes near (blue, purple) go to the general palette.
fn roof_family_for_colour(rgb: (u8, u8, u8), rng: &mut impl Rng) -> Block {
    let (l, a, b) = crate::colors::oklab_components(&rgb);
    if a.hypot(b) < 0.035 {
        return if l >= 0.55 {
            ZINC
        } else if l >= 0.38 {
            SLATE
        } else if l >= 0.25 {
            SLATE_DARK
        } else {
            GLAZED_BLACK
        };
    }
    // Reds, oranges and browns are clay tile however bright the tag: a vivid
    // orange #E96B39 is still a tile roof, not orange concrete.
    let (r, g, bl) = (f32::from(rgb.0), f32::from(rgb.1), f32::from(rgb.2));
    if r > g && r > bl {
        let hue = 60.0 * (g - bl) / (r - g.min(bl));
        if (-20.0..=40.0).contains(&hue) {
            return nearest_roof_shade(&TILE_FAMILIES, rgb).0;
        }
    }
    let families: Vec<Block> = ROOF_SHADES.iter().map(|&(b, _)| b).collect();
    match nearest_roof_shade(&families, rgb) {
        (block, d) if d <= 0.17 => block,
        _ => crate::block_palette::roof_block_for_color(rgb, rng),
    }
}

/// A roof in the wall's own tone merges with it into one lump: brick tile
/// over a brick wall moves to the red clay shade (brick under red tile is the
/// norm, the same block is not), zinc over a grey wall becomes slate.
fn roof_distinct_from_wall(roof: Block, wall: Block, region: MaterialRegion) -> Block {
    const BRICK_TONE: &[Block] = &[BRICK, POLISHED_GRANITE, GRANITE];
    const RED_TONE: &[Block] = &[RED_TERRACOTTA, MANGROVE_PLANKS];
    const ORANGE_TONE: &[Block] = &[TERRACOTTA, ORANGE_TERRACOTTA];
    const MID_GREY: &[Block] = &[
        POLISHED_ANDESITE,
        ANDESITE,
        STONE,
        STONE_BRICKS,
        SMOOTH_STONE,
        LIGHT_GRAY_CONCRETE,
        LIGHT_GRAY_CONCRETE_POWDER,
        CYAN_TERRACOTTA,
        TUFF,
    ];
    let warm_tone = [BRICK_TONE, RED_TONE, ORANGE_TONE]
        .into_iter()
        .find(|tone| tone.contains(&roof));
    let clash = roof == wall
        || warm_tone.is_some_and(|tone| tone.contains(&wall))
        || (MID_GREY.contains(&roof) && MID_GREY.contains(&wall));
    if !clash {
        roof
    } else if warm_tone.is_some() && region != MaterialRegion::BritishIsles {
        // Still clay tile, in a neighbouring shade.
        if warm_tone == Some(RED_TONE) {
            TILE_BRICK
        } else {
            TILE_RED
        }
    } else if roof == SLATE_DARK || region == MaterialRegion::BrickNorth {
        GLAZED_BLACK
    } else {
        SLATE_DARK
    }
}

/// Categories whose roof aesthetic (pitched / domed / glass) doesn't fit a parapet.
#[inline]
fn flat_parapet_excluded(category: BuildingCategory) -> bool {
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

/// Parapet on a flat roof the preset leaves open. Most flat roofs end in a
/// low wall, nearly all of them in North America; houses, garages and sheds
/// stop at a plain edge, and so do more single-storey buildings.
fn flat_parapet_for(
    group_seed: u64,
    roof_type: RoofType,
    building_height: i32,
    category: BuildingCategory,
    condition: BuildingCondition,
    region: BuildingRegion,
) -> bool {
    if roof_type != RoofType::Flat
        || flat_parapet_excluded(category)
        || matches!(
            condition,
            BuildingCondition::Construction | BuildingCondition::Ruined
        )
    {
        return false;
    }
    let share = match region {
        BuildingRegion::NorthAmerica => 0.92,
        BuildingRegion::Europe => 0.70,
        BuildingRegion::Other => 0.80,
    };
    let share = if building_height <= 5 {
        share * 0.7
    } else {
        share
    };
    element_rng(group_seed ^ 0x534F_F154_4AAE_E0F0).random_bool(share)
}

/// Same-tone substitutes for the wall variety patches; empty slice = no
/// variety. Only siblings close enough in colour to read as weathering or
/// repair, never a second material.
fn substitute_pool_only(block: Block) -> &'static [Block] {
    match block {
        // Mid-grey stone
        STONE_BRICKS => &[CRACKED_STONE_BRICKS, STONE],
        COBBLESTONE => &[ANDESITE, STONE],
        STONE => &[ANDESITE, COBBLESTONE],
        ANDESITE => &[POLISHED_ANDESITE, STONE],
        POLISHED_ANDESITE => &[ANDESITE],
        CHISELED_STONE_BRICKS | CRACKED_STONE_BRICKS => &[STONE_BRICKS],
        MOSSY_STONE_BRICKS => &[MOSSY_COBBLESTONE, STONE_BRICKS],
        MOSSY_COBBLESTONE => &[MOSSY_STONE_BRICKS, COBBLESTONE],

        // Warm reds: repointed and weathered brick
        BRICK => &[POLISHED_GRANITE, POLISHED_GRANITE, GRANITE],
        POLISHED_GRANITE => &[BRICK, GRANITE],
        GRANITE => &[POLISHED_GRANITE],
        TERRACOTTA => &[POLISHED_GRANITE],
        ORANGE_TERRACOTTA => &[TERRACOTTA],

        // Deepslate
        DEEPSLATE_BRICKS => &[POLISHED_DEEPSLATE, COBBLED_DEEPSLATE],
        POLISHED_DEEPSLATE => &[DEEPSLATE_BRICKS, COBBLED_DEEPSLATE],
        COBBLED_DEEPSLATE => &[DEEPSLATE_BRICKS, POLISHED_DEEPSLATE, DEEPSLATE],
        DEEPSLATE => &[COBBLED_DEEPSLATE, DEEPSLATE_BRICKS],

        // Blackstone
        POLISHED_BLACKSTONE_BRICKS => &[POLISHED_BLACKSTONE, BLACKSTONE],
        POLISHED_BLACKSTONE => &[POLISHED_BLACKSTONE_BRICKS, BLACKSTONE],
        BLACKSTONE => &[POLISHED_BLACKSTONE, POLISHED_BLACKSTONE_BRICKS],

        // Brown family
        BROWN_TERRACOTTA => &[BROWN_CONCRETE],
        BROWN_CONCRETE => &[BROWN_TERRACOTTA],

        // Off-white: quartz, and render touched up in a brighter white
        QUARTZ_BLOCK => &[SMOOTH_QUARTZ],
        QUARTZ_BRICKS | SMOOTH_QUARTZ => &[QUARTZ_BLOCK],
        WHITE_CONCRETE => &[CALCITE],

        // Sandstone: rough beside smooth
        SANDSTONE => &[SMOOTH_SANDSTONE],
        SMOOTH_SANDSTONE => &[SANDSTONE],

        POLISHED_DIORITE => &[DIORITE],
        DIORITE => &[POLISHED_DIORITE],

        // Concrete beside its powder
        GRAY_CONCRETE => &[GRAY_CONCRETE_POWDER],
        LIGHT_GRAY_CONCRETE => &[LIGHT_GRAY_CONCRETE_POWDER],
        LIGHT_GRAY_CONCRETE_POWDER => &[LIGHT_GRAY_CONCRETE],

        // Weathered copper
        WAXED_EXPOSED_COPPER => &[WAXED_EXPOSED_CHISELED_COPPER, WAXED_EXPOSED_CUT_COPPER],
        WAXED_EXPOSED_CHISELED_COPPER => &[WAXED_EXPOSED_COPPER, WAXED_EXPOSED_CUT_COPPER],
        WAXED_EXPOSED_CUT_COPPER => &[WAXED_EXPOSED_COPPER, WAXED_EXPOSED_CHISELED_COPPER],

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

/// Glass a tower can be walled in, so a corner can tell it apart from stone.
fn is_glass_wall(block: Block) -> bool {
    const GLASS_WALLS: [Block; 9] = [
        GLASS,
        WHITE_STAINED_GLASS,
        GRAY_STAINED_GLASS,
        LIGHT_GRAY_STAINED_GLASS,
        BLACK_STAINED_GLASS,
        BROWN_STAINED_GLASS,
        CYAN_STAINED_GLASS,
        BLUE_STAINED_GLASS,
        LIGHT_BLUE_STAINED_GLASS,
    ];
    GLASS_WALLS.contains(&block)
}

/// The pier at a glass tower's corner. Half the glassy towers draw a glass
/// accent, which would leave no corner at all.
fn corner_pier(config: &BuildingConfig) -> Block {
    if is_glass_wall(config.accent_block) {
        LIGHT_GRAY_CONCRETE
    } else {
        config.accent_block
    }
}

/// The street-level shopfront or lobby: display glass from the pavement up
/// between dark piers, under a sign band in the same frame. The end piers carry
/// the wall down to the ground.
fn shopfront_block(h: i32, config: &BuildingConfig, col: ColumnFacade) -> Block {
    match config.shop_slot_of(col) {
        BaySlot::End | BaySlot::Lattice => match config.base_course_block {
            Some(base) if h <= config.start_y_offset + config.base_course_rows() => base,
            _ => config.wall_block,
        },
        _ if h == config.ground_floor_top() => config.shop_frame,
        BaySlot::Window { .. } => GLASS,
        BaySlot::Pier => config.shop_frame,
    }
}

/// The trim between two storeys at a window: a light-stone lintel under the
/// window above, or the spandrel panel of a ribbon or tower shaft.
fn band_block(config: &BuildingConfig, slot: BaySlot) -> Block {
    let window = slot.is_window();
    if config.window_archetype == WindowArchetype::Ribbon {
        // The spandrel runs unbroken past the mullions.
        return if slot == BaySlot::End {
            config.wall_block
        } else {
            config.accent_block
        };
    }
    if !window {
        config.wall_block
    } else if config.window_archetype.recessed_spandrels() || config.use_vertical_accent {
        config.accent_block
    } else {
        config.trim_block
    }
}

fn determine_wall_block_at_position_pristine(
    h: i32,
    config: &BuildingConfig,
    col: ColumnFacade,
) -> Block {
    // Shops and lobbies reach the pavement, ahead of the plinth.
    if h <= config.ground_floor_top() && config.shopfront_column(col) {
        return shopfront_block(h, config, col);
    }

    // Darker plinth rows ground the building visually. A ground-floor window
    // cuts through the upper course, as its sill sits on the lower one.
    if let Some(base) = config.base_course_block {
        let window_cut = h > config.start_y_offset + 1
            && !col.party
            && config.punched_windows()
            && config.slot_of(col).is_window();
        if h <= config.start_y_offset + config.base_course_rows() && !window_cut {
            return base;
        }
    }

    let floor_row = config.floor_row(h);

    // Party walls render like windowless walls: no glazing into the
    // attached neighbor of a terraced row.
    if !config.has_windows || col.party {
        let above_floor = h > config.start_y_offset + 1;
        // Glass carried around a corner reads as a box of panes. Only the glass
        // towers: a greenhouse is glass walled too and wants no piers. A party
        // wall is not seen, so it keeps its plain treatment.
        let glass_tower = matches!(
            config.category,
            BuildingCategory::GlassySkyscraper | BuildingCategory::GlassCornerSkyscraper
        );
        if col.corner && !col.party && glass_tower {
            return corner_pier(config);
        }
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
        if col.corner {
            // Ahead of the bands, so the pier is not cut by one every storey.
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
            && (col.wall_u + config.window_phase).rem_euclid(4) == 1;

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
        // The corner turns on a pier: glass carried around a corner reads as a
        // box of panes rather than a building, and a real curtain wall almost
        // always meets the return on solid material.
        let mullion = !above_floor
            || floor_row == 0
            || col.corner
            || (col.wall_u + config.window_phase).rem_euclid(5) == 0;
        if mullion {
            config.wall_block
        } else {
            config.window_block
        }
    } else {
        // Punched windows, laid out per wall run so every run is symmetric
        // and turns its corners on solid piers.
        let slot = config.slot_of(col);
        let role = config.floor_role(h);

        if role == FloorRole::Ground {
            // A sill row at the pavement, then glass up to the first slab: the
            // ground floor's windows are a row taller than the ones above.
            if above_floor && slot.is_window() {
                return config.window_block;
            }
            if config.rustication
                && h > config.start_y_offset + config.base_course_rows()
                && (h - config.start_y_offset) % 2 == 0
            {
                // Banded stone courses over the plinth.
                return config.base_course_block.unwrap_or(config.accent_block);
            }
            return config.wall_block;
        }

        if floor_row == 0 {
            // Band cornice separating the body from a treated top floor.
            let top_band = config.top_treatment
                && h == config.start_y_offset + config.building_height - config.floor_cycle;
            return if top_band || config.use_accent_lines {
                config.accent_block
            } else {
                band_block(config, slot)
            };
        }

        // Attic band under a pitched roof and a treated top floor both keep
        // their window columns but glaze only part of the storey.
        let rows = if config.attic_style && role == FloorRole::Top {
            floor_row == config.floor_cycle - 1
        } else if config.top_treatment && role == FloorRole::Top && config.floor_cycle >= 3 {
            floor_row < config.floor_cycle - 1
        } else {
            archetype_allows_window(config.window_archetype, floor_row, config.floor_cycle)
                && config.window_row_cap(h).is_none_or(|cap| floor_row <= cap)
        };
        if slot.is_window() && rows {
            config.window_block
        } else if config.window_archetype == WindowArchetype::Ribbon && !rows {
            // The spandrel band under a ribbon is two rows deep.
            band_block(config, slot)
        } else {
            config.wall_block
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

/// The rotated frame of a near-rectangular footprint turned off the grid:
/// `u` runs along its dominant axis (within 45 degrees of world X), `v`
/// across it. A two-slope roof laid in this frame has planar faces that
/// meet the walls, where a scan along the world axes warps them.
#[derive(Clone, Copy, Debug)]
struct TentFrame {
    /// Rotation of the dominant axis off world X, radians, in (-45, 45] deg.
    angle: f64,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
}

impl TentFrame {
    fn uv(&self, x: i32, z: i32) -> (f64, f64) {
        let (c, sn) = (self.angle.cos(), self.angle.sin());
        let (x, z) = (x as f64, z as f64);
        (x * c + z * sn, -x * sn + z * c)
    }

    fn u_axis(&self) -> (f64, f64) {
        (self.angle.cos(), self.angle.sin())
    }

    fn v_axis(&self) -> (f64, f64) {
        (-self.angle.sin(), self.angle.cos())
    }
}

/// Near-rectangular footprints rotated 1 to 45 degrees get a planar tent in
/// their own frame; aligned ones, and wings, keep the polygon scan.
fn tent_frame(nodes: &[ProcessedNode]) -> Option<TentFrame> {
    if nodes.len() < 3 {
        return None;
    }
    let ang = dominant_axis_angle(nodes);
    let dev = ang.to_degrees().abs();
    if !(1.0..=45.0).contains(&dev) {
        return None;
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
    (rect_area > 0.0 && polygon_area / rect_area >= 0.78).then_some(TentFrame {
        angle: ang,
        u_min,
        u_max,
        v_min,
        v_max,
    })
}

/// Which way the ridge of a two-slope roof runs, and the frame it runs in.
#[derive(Clone, Copy, Debug)]
struct RidgeFrame {
    /// Ridge runs along world X (else Z); in a tent, along the axis nearest X.
    along_x: bool,
    /// Rotated frame of a turned footprint; None keeps the world grid.
    tent: Option<TentFrame>,
}

impl RidgeFrame {
    #[cfg(test)]
    fn grid(along_x: bool) -> Self {
        Self {
            along_x,
            tent: None,
        }
    }

    /// Cells the slope crosses from eave to eave, for a bounding box of
    /// `width` by `length` (extents, one less than the cell counts).
    fn span_cells(&self, width: i32, length: i32) -> f64 {
        match self.tent {
            Some(t) if self.along_x => t.v_max - t.v_min + 1.0,
            Some(t) => t.u_max - t.u_min + 1.0,
            None if self.along_x => length as f64 + 1.0,
            None => width as f64 + 1.0,
        }
    }
}

/// Ridge of a two-slope roof: across a mapped `roof:direction` (the way the
/// slope faces), else per `roof:orientation`, else along the longer side.
fn ridge_frame(
    nodes: &[ProcessedNode],
    width: i32,
    length: i32,
    roof_orientation: Option<&str>,
    roof_direction: Option<&str>,
    preferred_ridge_along_x: Option<bool>,
) -> RidgeFrame {
    let tent = tent_frame(nodes);
    let (u_len, v_len) = match tent {
        Some(t) => (t.u_max - t.u_min, t.v_max - t.v_min),
        None => (width as f64, length as f64),
    };
    let u_is_longer = u_len >= v_len;
    let (ux, uz) = tent.map_or((1.0, 0.0), |t| t.u_axis());
    let (vx, vz) = tent.map_or((0.0, 1.0), |t| t.v_axis());
    let along_x = if let Some(deg) = roof_direction.and_then(parse_roof_direction_degrees) {
        let (dx, dz) = bearing_unit_vector(deg);
        (dx * vx + dz * vz).abs() >= (dx * ux + dz * uz).abs()
    } else {
        match roof_orientation {
            Some(o) if o.eq_ignore_ascii_case("along") => u_is_longer,
            Some(o) if o.eq_ignore_ascii_case("across") => !u_is_longer,
            _ => preferred_ridge_along_x.unwrap_or(u_is_longer),
        }
    };
    RidgeFrame { along_x, tent }
}

/// Whether a wall segment steps along X, matching bresenham_line's driving axis.
/// Not derivable from compute_outward_normal, which snaps the opposite way.
#[inline]
fn segment_axis_x(x1: i32, z1: i32, x2: i32, z2: i32) -> bool {
    (x2 - x1).abs() >= (z2 - z1).abs()
}

/// Axis for a segment's first point, a vertex shared with the previous segment.
/// Both visits use that previous segment's axis so every pass agrees on the column.
#[inline]
fn segment_start_axis_x(nodes: &[ProcessedNode], i: usize, own_axis_x: bool) -> bool {
    let n = nodes.len();
    let closed = n >= 3 && nodes[0].x == nodes[n - 1].x && nodes[0].z == nodes[n - 1].z;
    let here = &nodes[i - 1];
    let mut j = i - 1;
    // A zero-length edge has no direction, so walk back to the last one that moved.
    for _ in 0..n {
        let prev_idx = if j >= 1 {
            j - 1
        } else if closed {
            n - 2
        } else {
            // Open polyline start: nothing else touches this vertex.
            return own_axis_x;
        };
        let prev = &nodes[prev_idx];
        if prev.x != here.x || prev.z != here.z {
            return segment_axis_x(prev.x, prev.z, here.x, here.z);
        }
        j = prev_idx;
    }
    own_axis_x
}

/// Along-wall ordinate of a segment's idx-th point. Only the first uses start_axis_x.
#[inline]
fn wall_ordinate(idx: usize, start_axis_x: bool, seg_axis_x: bool, bx: i32, bz: i32) -> i32 {
    let axis_x = if idx == 0 { start_axis_x } else { seg_axis_x };
    if axis_x {
        bx
    } else {
        bz
    }
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
    outward: i32,
) -> (i32, i32) {
    let seg_dx = x2 - x1;
    let seg_dz = z2 - z1;

    // Left-hand normal of the segment, turned outward by the ring's winding.
    let (raw_nx, raw_nz) = (-seg_dz * outward, seg_dx * outward);

    // Snap to the dominant axis so the normal is always one of
    // (±1, 0) or (0, ±1).
    if raw_nx.abs() >= raw_nz.abs() {
        (raw_nx.signum(), 0)
    } else {
        (0, raw_nz.signum())
    }
}

/// Which side of its edges a ring's outside lies on: the factor that turns
/// the left-hand normal (-dz, dx) of every edge outward. Taken from the
/// signed area, which every edge of the ring shares, so the re-entrant walls
/// of an L or U get the same answer as the rest; a test against the centroid
/// flips them, and every balcony, door and pilaster on them ends up indoors.
pub(crate) fn outward_side(nodes: &[ProcessedNode]) -> i32 {
    let mut area2 = 0i64;
    for i in 0..nodes.len() {
        let j = (i + 1) % nodes.len();
        area2 += nodes[i].x as i64 * nodes[j].z as i64 - nodes[j].x as i64 * nodes[i].z as i64;
    }
    if area2 > 0 {
        -1
    } else {
        1
    }
}

/// Whether vertex `c` between `p` and `q` is a convex corner of a ring whose
/// outside is on `outward` (see `outward_side`). Collinear and re-entrant
/// vertices are not corners.
fn corner_is_convex(p: (i32, i32), c: (i32, i32), q: (i32, i32), outward: i32) -> bool {
    let cross = (c.0 - p.0) as i64 * (q.1 - c.1) as i64 - (c.1 - p.1) as i64 * (q.0 - c.0) as i64;
    cross != 0 && cross.signum() as i32 == -outward
}

/// Distinct vertices of a closed ring in order, the closing node dropped.
fn ring_vertices(nodes: &[ProcessedNode]) -> Vec<(i32, i32)> {
    let mut corners: Vec<(i32, i32)> = Vec::new();
    for node in nodes {
        let pos = (node.x, node.z);
        if corners.last() != Some(&pos) {
            corners.push(pos);
        }
    }
    if corners.len() > 1 && corners.first() == corners.last() {
        corners.pop();
    }
    corners
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

/// Whether a wall column is out of bounds for anything standing in front of
/// it: a passage, a party wall, a photograph's shell, or for a building part
/// the cell of another building.
fn front_blocked(
    wc: &WallColumn,
    config: &BuildingConfig,
    building_passages: &CoordinateBitmap,
    facade: &FacadePlan,
    part_probe: Option<(&CoordinateBitmap, &FnvHashSet<(i32, i32)>)>,
) -> bool {
    let (bx, bz) = (wc.x, wc.z);
    let (nx, nz) = wc.normal;
    if (nx, nz) == (0, 0)
        || building_passages.contains(bx, bz)
        || facade.is_party(bx, bz)
        || config.is_flat_column(bx, bz)
    {
        return true;
    }
    part_probe.is_some_and(|(footprints, own)| {
        let blocked = |cx: i32, cz: i32| footprints.contains(cx, cz) && !own.contains(&(cx, cz));
        blocked(bx + nx, bz + nz) || blocked(bx + 2 * nx, bz + 2 * nz)
    })
}

/// Whether the wall picks glass at row `h` of a column.
fn glazed_at(h: i32, config: &BuildingConfig, col: ColumnFacade) -> bool {
    let block = determine_wall_block_at_position_pristine(h, config, col);
    block != config.wall_block && (block == config.window_block || block == GLASS)
}

/// Balconies on residential facades: centred on the windows of a regular
/// band up the street front, or scattered.
fn generate_balconies(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    building_passages: &CoordinateBitmap,
    facade: &FacadePlan,
) {
    if config.is_tall_building
        || !matches!(
            config.category,
            BuildingCategory::Residential | BuildingCategory::House
        )
        || !config.punched_windows()
        || config.condition != BuildingCondition::Normal
    {
        return;
    }
    let mut rng = element_rng(element.id);
    let trapdoor_base =
        SHUTTER_TRAPDOOR_OPTIONS[rng.random_range(0..SHUTTER_TRAPDOOR_OPTIONS.len())];
    let balcony_floor = make_top_slab(SMOOTH_STONE_SLAB);
    let top_h = config.start_y_offset + config.building_height;

    let columns = facade_columns(&element.nodes, config);
    let mut seg_cells: Vec<FnvHashSet<(i32, i32)>> =
        vec![FnvHashSet::default(); element.nodes.len().saturating_sub(1)];
    for wc in &columns {
        seg_cells[wc.seg].insert((wc.x, wc.z));
    }

    for wc in &columns {
        let BaySlot::Window { k, width, bay } = wc.slot() else {
            continue;
        };
        // One balcony per window, hung from its middle column.
        if k != (width - 1) / 2 || front_blocked(wc, config, building_passages, facade, None) {
            continue;
        }
        let (bx, bz) = (wc.x, wc.z);
        let (out_nx, out_nz) = wc.normal;
        let (tan_x, tan_z) = (-out_nz, out_nx);
        // A balcony spans five columns of this wall: all of them must be
        // on it, so it never wraps a corner, stands before a party wall or
        // hangs over a passage.
        let fits = (-2i32..=2).all(|t| {
            let c = (bx + tan_x * t, bz + tan_z * t);
            seg_cells[wc.seg].contains(&c)
                && !facade.is_party(c.0, c.1)
                && !building_passages.contains(c.0, c.1)
        });
        if !fits {
            continue;
        }
        let is_rear = facade
            .segments
            .get(wc.seg)
            .and_then(|s| s.as_ref())
            .is_some_and(|s| s.class == FacadeClass::Rear);
        // Banded balconies skip windows until five columns are clear.
        let period = width + config.bays.pier;
        let step = (5 + period - 1) / period;
        let band_step = match config.balcony_band {
            BalconyBand::Scattered => None,
            BalconyBand::EveryBay => Some(step),
            BalconyBand::Alternating => Some(2 * step),
        };
        let col = wc.facade(facade);
        // Never on the ground floor; elevated parts keep their base row.
        for h in (config.start_y_offset + config.grammar_anchor() + config.floor_cycle)..top_h {
            if config.floor_row(h) != 0 || !glazed_at(h + 1, config, col) {
                continue;
            }
            let wants = match band_step {
                Some(step) => facade.is_street(bx, bz) && bay % step == 0,
                None => {
                    let floor_idx = h / config.floor_cycle;
                    let roll = coord_rng(
                        bx.wrapping_add(floor_idx * 3),
                        bz.wrapping_add(floor_idx * 5),
                        element.id ^ 0xBA1C_0417_0000_0011,
                    )
                    .random_range(0u32..100);
                    roll < if is_rear { 3 } else { 8 }
                }
            };
            if !wants {
                continue;
            }
            let abs_y = h + config.abs_terrain_offset;
            let floor_idx = h / config.floor_cycle;
            // A 3-wide by 2-deep platform with open-trapdoor railing around
            // the outer edge and occasional furniture.
            //
            // Top-down layout (outward = up):
            //  depth 3:  [Tf] [Tf] [Tf]  front fence
            //  depth 2:  [ f] [ f] [ f]  floor
            //  depth 1:  [ f] [ f] [ f]  floor
            //            wall wall wall
            // Side fences at t=±2, depths 1-2.
            let front_facing = facing_for_normal(out_nx, out_nz);
            let left_facing = facing_for_normal(-tan_x, -tan_z);
            let right_facing = facing_for_normal(tan_x, tan_z);
            let front_fence = make_open_trapdoor(trapdoor_base, front_facing);
            let left_fence = make_open_trapdoor(trapdoor_base, left_facing);
            let right_fence = make_open_trapdoor(trapdoor_base, right_facing);

            for t in -1i32..=1 {
                for depth in 1i32..=2 {
                    editor.set_block_with_properties_absolute(
                        balcony_floor.clone(),
                        bx + tan_x * t + out_nx * depth,
                        abs_y,
                        bz + tan_z * t + out_nz * depth,
                        Some(&[AIR]),
                        None,
                    );
                }
                editor.set_block_with_properties_absolute(
                    front_fence.clone(),
                    bx + tan_x * t + out_nx * 3,
                    abs_y + 1,
                    bz + tan_z * t + out_nz * 3,
                    Some(&[AIR]),
                    None,
                );
            }
            for depth in 1i32..=2 {
                editor.set_block_with_properties_absolute(
                    left_fence.clone(),
                    bx - tan_x * 2 + out_nx * depth,
                    abs_y + 1,
                    bz - tan_z * 2 + out_nz * depth,
                    Some(&[AIR]),
                    None,
                );
                editor.set_block_with_properties_absolute(
                    right_fence.clone(),
                    bx + tan_x * 2 + out_nx * depth,
                    abs_y + 1,
                    bz + tan_z * 2 + out_nz * depth,
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
            let side = if furn_rng.random_bool(0.5) { -1i32 } else { 1 };
            let (fx, fz) = (bx + tan_x * side + out_nx, bz + tan_z * side + out_nz);
            if furniture_roll < 30 {
                // Cauldron "planter" with a leaf block on top
                editor.set_block_absolute(CAULDRON, fx, abs_y + 1, fz, Some(&[AIR]), None);
                editor.set_block_absolute(OAK_LEAVES, fx, abs_y + 2, fz, Some(&[AIR]), None);
            } else if furniture_roll < 55 {
                // Stair "chair" facing outward
                let stair_facing = match facing_for_normal(-out_nx, -out_nz) {
                    "north" => StairFacing::North,
                    "south" => StairFacing::South,
                    "east" => StairFacing::East,
                    _ => StairFacing::West,
                };
                let chair =
                    create_stair_with_properties(OAK_STAIRS, stair_facing, StairShape::Straight);
                editor.set_block_with_properties_absolute(
                    chair,
                    fx,
                    abs_y + 1,
                    fz,
                    Some(&[AIR]),
                    None,
                );
            }
        }
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
    upside_down_stair(get_stair_block_for_material(material), facing)
}

/// [`make_upside_down_stair`] for a stair block picked directly.
fn upside_down_stair(stair_block: Block, facing: &str) -> BlockWithProperties {
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
    // Quoins are an occasional accent, not a rule.
    let quoin_chance = if matches!(
        config.era,
        ArchEra::TraditionalPreWar | ArchEra::HistoricOrnate
    ) {
        0.5
    } else {
        0.25
    };
    let roll_passed = element_rng(element.id.wrapping_add(3571)).random_bool(quoin_chance);
    if !roll_passed && guaranteed_corner.is_none() {
        return;
    }

    // Corners are where the wall turns: a mid-wall node shared with a
    // neighbour or carrying an entrance gets no column.
    let vertices = ring_vertices(&element.nodes);
    let n = vertices.len();
    let corners: Vec<(i32, i32)> = (0..n)
        .filter(|&i| {
            let p = vertices[(i + n - 1) % n];
            let c = vertices[i];
            let q = vertices[(i + 1) % n];
            let cross =
                (c.0 - p.0) as i64 * (q.1 - c.1) as i64 - (c.1 - p.1) as i64 * (q.0 - c.0) as i64;
            n < 3 || cross != 0
        })
        .map(|i| vertices[i])
        .collect();

    let quoin_block = config.accent_block;
    let top_h = config.start_y_offset + config.building_height;
    let passage_h = config.start_y_offset + BUILDING_PASSAGE_HEIGHT.min(config.building_height);

    // Whitelist the whole substitute family, otherwise variety-substituted wall blocks break the columns.
    let mut wall_family: Vec<Block> = vec![config.wall_block];
    wall_family.extend_from_slice(substitute_pool_only(config.wall_block));

    for &(cx, cz) in &corners {
        // Party-wall vertices sit against the neighbor and get no framing.
        // A corner block a photograph reaches belongs to the photograph: the
        // quoin would overwrite the one column two ring segments share, which
        // is exactly where a photographed wall meets a normal one.
        if facade.is_party(cx, cz) || config.is_flat_column(cx, cz) {
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

/// Adds the wall depth features that suit their building: corner beams on
/// industrial halls, buttresses on churches, corner piers on glass curtain
/// walls and a slab ledge on every band of a banded tower. Facades of punched
/// windows get their relief from the recessed glass, sills and cornices
/// instead, never from blocks stood in front of the wall.
///
/// All outward placements use an AIR whitelist to avoid overwriting
/// neighbouring buildings or existing decorations.
fn generate_wall_depth_features(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    has_sloped_roof: bool,
    building_passages: &CoordinateBitmap,
    facade: &FacadePlan,
    part_probe: Option<(&CoordinateBitmap, &FnvHashSet<(i32, i32)>)>,
) {
    let style = config.wall_depth_style;
    let banded_tower = style == WallDepthStyle::SkyscraperFins && config.use_horizontal_windows;
    if !matches!(
        style,
        WallDepthStyle::IndustrialBeams
            | WallDepthStyle::ReligiousButtress
            | WallDepthStyle::GlassCurtain
    ) && !banded_tower
    {
        return;
    }

    // Skip buildings that are too small for depth features
    let bounds = BuildingBounds::from_nodes(&element.nodes);
    if bounds.width() < 4 || bounds.length() < 4 {
        return;
    }
    // Low buildings keep only their buttresses.
    if config.building_height < config.floor_cycle + 2 && style != WallDepthStyle::ReligiousButtress
    {
        return;
    }
    // GlassCurtain: 40% of eligible buildings. Seeded on the shared style
    // seed (== element id for standalone buildings) so identically-tagged
    // parts of one building agree.
    let depth_roll: u32 = element_rng(config.style_seed.wrapping_add(7919)).random_range(0..100);
    if style == WallDepthStyle::GlassCurtain && depth_roll >= 40 {
        return;
    }

    let ledge = make_top_slab(get_slab_block_for_material(config.accent_block));
    // For sloped roofs with overhangs, stop depth features 2 blocks short
    // so protruding features don't visually break the clean eave/overhang line.
    // 2 blocks: one for the eave-edge stair row at base_height, one for the
    // overhang stair placed 1 block outward at base_height - 1.
    let height_reduction = if has_sloped_roof { 2 } else { 0 };

    for wc in facade_columns(&element.nodes, config) {
        let (bx, bz) = (wc.x, wc.z);
        if front_blocked(&wc, config, building_passages, facade, part_probe)
            || facade.is_door(bx, bz)
        {
            continue;
        }
        let (out_nx, out_nz) = wc.normal;
        // The wall carries a foundation down to the local terrain, so on
        // sloping ground the vertical details have to follow it down: to
        // the ground under the protrusion itself, which one cell out
        // from the wall can be a block lower still.
        let descent = if config.is_ground_level {
            feature_descent(editor, config, bx, bz, bx + out_nx, bz + out_nz)
        } else {
            0
        };
        let run_end = wc.pos == 0 || wc.pos == wc.upper.len - 1;

        match style {
            WallDepthStyle::IndustrialBeams if wc.pos < 2 || wc.pos >= wc.upper.len - 2 => {
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
            WallDepthStyle::ReligiousButtress => {
                place_religious_buttress(
                    editor,
                    config,
                    &wc,
                    facing_for_normal(out_nx, out_nz),
                    height_reduction,
                    descent,
                );
            }
            WallDepthStyle::GlassCurtain if run_end => {
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
            WallDepthStyle::SkyscraperFins if !wc.corner() => {
                // A slab ledge along every band.
                let top_h = config.start_y_offset + config.building_height - height_reduction;
                for h in (config.ground_floor_top() + 1)..=top_h {
                    if config.floor_row(h) == 0 {
                        editor.set_block_with_properties_absolute(
                            ledge.clone(),
                            bx + out_nx,
                            h + config.abs_terrain_offset,
                            bz + out_nz,
                            Some(&[AIR]),
                            None,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

/// Window dressing on punched facades, all of it sub-block: a light-stone
/// sill under each window, a stair hood over it on ornate fabric, and the
/// shutters or odd flower pot of a cottage frame style. Ribbon windows get a
/// slab ledge along every storey instead. Tower shafts take their relief from
/// their recessed spandrels and get none of it.
fn generate_window_frames(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    building_passages: &CoordinateBitmap,
    facade: &FacadePlan,
    part_probe: Option<(&CoordinateBitmap, &FnvHashSet<(i32, i32)>)>,
) {
    if !config.punched_windows()
        || config.condition != BuildingCondition::Normal
        || config.detail == DetailTier::Minimal
    {
        return;
    }
    let ribbon = config.window_archetype == WindowArchetype::Ribbon;
    if config.is_tall_building && !ribbon {
        return;
    }
    let top_h = config.start_y_offset + config.building_height;
    let sill = make_top_slab(trim_slab(config.trim_block));
    let hood_stair = trim_stair(config.trim_block);
    // Houses carry shutters; city walk-ups and stucco blocks rarely do.
    let shutter = config
        .window_frame
        .and_then(WindowFrameStyle::shutter_block)
        .filter(|_| config.category == BuildingCategory::House);
    let planters = config
        .window_frame
        .is_some_and(WindowFrameStyle::has_planters);
    let window_boxes = has_window_boxes(config, &element.tags);

    for wc in facade_columns(&element.nodes, config) {
        if front_blocked(&wc, config, building_passages, facade, part_probe) {
            continue;
        }
        let (bx, bz) = (wc.x, wc.z);
        let (lx, lz) = (bx + wc.normal.0, bz + wc.normal.1);
        let facing = facing_for_normal(wc.normal.0, wc.normal.1);
        let col = wc.facade(facade);
        let slot = wc.slot();
        let door_column = facade.is_door(bx, bz);
        let place = |editor: &mut WorldEditor, block: &BlockWithProperties, h: i32| {
            editor.set_block_with_properties_absolute(
                block.clone(),
                lx,
                h + config.abs_terrain_offset,
                lz,
                Some(&[AIR]),
                None,
            );
        };

        // Surrounds: the jambs on the piers either side of a window, flush
        // in the trim stone, and the head row over it; the band row under
        // it is trim already.
        if config.surrounds {
            let head =
                |c: ColumnFacade, h: i32| glazed_at(h - 1, config, c) && !glazed_at(h, config, c);
            let neighbours: Vec<ColumnFacade> = if slot.is_window() {
                vec![col]
            } else {
                [-1, 1]
                    .into_iter()
                    .map(|s| wc.slot_at(s))
                    .filter(|s| s.is_window())
                    .map(|s| ColumnFacade { slot: s, ..col })
                    .collect()
            };
            for h in (config.ground_floor_top() + 2)..top_h {
                let framed = neighbours.iter().any(|&c| {
                    head(c, h)
                        || (!slot.is_window()
                            && (glazed_at(h, config, c)
                                || (config.floor_row(h) == 0 && glazed_at(h + 1, config, c))))
                });
                if framed {
                    editor.set_block_absolute(
                        config.trim_block,
                        bx,
                        h + config.abs_terrain_offset,
                        bz,
                        None,
                        Some(&[config.window_block, GLASS]),
                    );
                }
            }
        }

        // Stair hoods over the window heads of ornate fabric: every storey
        // in North America, the piano nobile alone in Europe.
        if slot.is_window() {
            for h in (config.ground_floor_top() + 2)..top_h {
                let hooded = (config.window_archetype == WindowArchetype::Hooded
                    && (config.region != BuildingRegion::Europe
                        || h <= config.ground_floor_top() + 1 + config.floor_cycle))
                    || config.nobile_storey(h - 1);
                if hooded && glazed_at(h - 1, config, col) && !glazed_at(h, config, col) {
                    place(editor, &upside_down_stair(hood_stair, facing), h);
                }
            }
        }

        // The bands between storeys.
        for h in (config.ground_floor_top() + 1)..top_h {
            if config.floor_row(h) != 0 {
                continue;
            }
            if ribbon {
                if slot != BaySlot::End {
                    place(editor, &sill, h);
                }
                continue;
            }
            if !slot.is_window() {
                continue;
            }
            if glazed_at(h + 1, config, col) {
                place(editor, &sill, h);
                if planters
                    && matches!(slot, BaySlot::Window { k: 0, .. })
                    && !(window_boxes && window_box_at(config, &wc, h))
                {
                    let roll = coord_rng(bx, bz.wrapping_add(h * 7), config.element_id)
                        .random_range(0..100u32);
                    if roll < 20 {
                        let pot = POTTED_PLANT_OPTIONS[roll as usize % POTTED_PLANT_OPTIONS.len()];
                        editor.set_block_absolute(
                            pot,
                            lx,
                            h + 1 + config.abs_terrain_offset,
                            lz,
                            Some(&[AIR]),
                            None,
                        );
                    }
                }
            }
        }
        // Ground-floor windows sit on the plinth row.
        if !ribbon
            && slot.is_window()
            && !door_column
            && !config.shopfront_column(col)
            && glazed_at(config.start_y_offset + 2, config, col)
        {
            place(editor, &sill, config.start_y_offset + 1);
        }

        // Shutters hang on a pier beside a window, where the pier is wide
        // enough for each window to keep its own.
        let Some(shutter_base) = shutter else {
            continue;
        };
        if slot.is_window() {
            continue;
        }
        let shutter_block = make_open_trapdoor(shutter_base, facing);
        for side in [-1, 1] {
            let window = wc.slot_at(side);
            if !window.is_window() || wc.slot_at(-side).is_window() {
                continue;
            }
            let beside = ColumnFacade {
                slot: window,
                ..col
            };
            for h in (config.start_y_offset + 2)..=top_h {
                if glazed_at(h, config, beside) && !(door_column && config.entrance_row(h)) {
                    place(editor, &shutter_block, h);
                }
            }
        }
    }
}

// ============================================================================
// Window Life (Curtains, Air Conditioners, Window Boxes)
// ============================================================================

/// Storey a wall row belongs to, the ground floor being 0. The band row
/// under a storey's windows counts with that storey, so every row of one
/// window, and the sill under it, share an index.
#[inline]
fn storey_of(config: &BuildingConfig, h: i32) -> i32 {
    (h - config.start_y_offset - config.grammar_anchor()).div_euclid(config.floor_cycle.max(1))
}

/// Rng shared by every column and row of one window, the bay of its wall run
/// on its storey, so a window is dressed as a whole. None off a window.
fn window_rng(
    config: &BuildingConfig,
    wc: &WallColumn,
    storey: i32,
    salt: u64,
) -> Option<impl Rng> {
    let BaySlot::Window { bay, .. } = wc.slot() else {
        return None;
    };
    Some(coord_rng(
        (wc.run as i32).wrapping_mul(4096).wrapping_add(bay),
        storey,
        config.element_id ^ salt,
    ))
}

/// Whether the building is lived in: flats, houses and hotels, or a generic
/// building whose use is mapped as residential.
fn is_dwelling(config: &BuildingConfig, tags: &HashMap<String, String>) -> bool {
    match config.category {
        BuildingCategory::Residential | BuildingCategory::House | BuildingCategory::Hotel => true,
        BuildingCategory::Default => matches!(
            tags.get("building:use").map(String::as_str),
            Some("residential" | "apartments")
        ),
        _ => false,
    }
}

/// How a window's curtain hangs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CurtainDraw {
    /// Across the whole window.
    Closed,
    /// Pulled back to the jambs: the end columns of a window three or more
    /// wide, one side of a pair.
    Open,
    /// A blind let down over the top row.
    Blind,
}

/// Share of dwelling windows, in percent, with a curtain behind the glass.
const CURTAIN_PERCENT: u32 = 32;
/// Share of street-facing walk-up windows, in percent, with an air conditioner.
const AIR_CONDITIONER_PERCENT: u32 = 10;
/// Share of upper-floor windows, in percent, with a window box.
const WINDOW_BOX_PERCENT: u32 = 10;

/// Curtain colour and hang of one window, if it has one.
fn window_curtain(
    config: &BuildingConfig,
    wc: &WallColumn,
    storey: i32,
) -> Option<(Block, CurtainDraw)> {
    let mut rng = window_rng(config, wc, storey, 0xC0A7_A1A5_0000_0001)?;
    if rng.random_range(0..100u32) >= CURTAIN_PERCENT {
        return None;
    }
    let colour = if rng.random_bool(0.65) {
        WHITE_WOOL
    } else {
        LIGHT_GRAY_WOOL
    };
    let draw = match rng.random_range(0..3u32) {
        0 => CurtainDraw::Closed,
        1 => CurtainDraw::Open,
        _ => CurtainDraw::Blind,
    };
    Some((colour, draw))
}

/// Whether the curtain covers column `k` of a `width`-wide window at a row,
/// `top_row` being the window's highest glazed row.
fn curtain_covers(draw: CurtainDraw, k: i32, width: i32, top_row: bool) -> bool {
    match draw {
        CurtainDraw::Closed => true,
        CurtainDraw::Open if width >= 3 => k == 0 || k == width - 1,
        CurtainDraw::Open if width == 2 => k == 0,
        // A single sash has no side to pull a curtain to.
        CurtainDraw::Open | CurtainDraw::Blind => top_row,
    }
}

/// Whether the building carries window air conditioners: a pre-war North
/// American walk-up of four to eight storeys with a street front.
fn has_air_conditioners(
    config: &BuildingConfig,
    tags: &HashMap<String, String>,
    facade: &FacadePlan,
) -> bool {
    let storeys = (config.building_height - config.grammar_anchor()) / config.floor_cycle.max(1);
    config.region == BuildingRegion::NorthAmerica
        && config.recess_windows
        && config.detail != DetailTier::Minimal
        && facade.has_any_street
        && matches!(config.era, ArchEra::Unknown | ArchEra::TraditionalPreWar)
        && (4..=8).contains(&storeys)
        && matches!(
            config.category,
            BuildingCategory::Residential | BuildingCategory::Default
        )
        && is_dwelling(config, tags)
}

/// Whether the building carries window boxes: pre-war and historic European
/// houses and flats.
fn has_window_boxes(config: &BuildingConfig, tags: &HashMap<String, String>) -> bool {
    config.region == BuildingRegion::Europe
        && config.detail != DetailTier::Minimal
        && matches!(
            config.era,
            ArchEra::TraditionalPreWar | ArchEra::HistoricOrnate
        )
        && config.category != BuildingCategory::Hotel
        && is_dwelling(config, tags)
}

/// Whether the window over the sill row `h` gets a window box.
fn window_box_at(config: &BuildingConfig, wc: &WallColumn, h: i32) -> bool {
    h > config.ground_floor_top()
        && window_rng(config, wc, storey_of(config, h), 0xF10B_0C5E_0000_0002)
            .is_some_and(|mut rng| rng.random_range(0..100u32) < WINDOW_BOX_PERCENT)
}

/// Signs of life in the windows of a dwelling, all within its own windows:
/// a curtain or blind hung just inside the recessed glass, a window air
/// conditioner in the lower sash of a North American walk-up, and a box of
/// greenery on the sill of an upper European window.
///
/// Curtains need the glass set back a block (`recess_windows`), so they never
/// meet a furnished interior, which hangs its own, and `floor_cells`, the
/// rooms the building alone fills. Runs after the balconies, so a window box
/// never lands on a balcony.
#[allow(clippy::too_many_arguments)]
fn generate_window_life(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    building_passages: &CoordinateBitmap,
    facade: &FacadePlan,
    part_probe: Option<(&CoordinateBitmap, &FnvHashSet<(i32, i32)>)>,
    floor_cells: Option<&FnvHashSet<(i32, i32)>>,
    footprints: Option<&CoordinateBitmap>,
) {
    if config.condition != BuildingCondition::Normal
        || !config.punched_windows()
        || config.is_tall_building
        || !is_dwelling(config, &element.tags)
    {
        return;
    }
    // Only in rooms the building has to itself: another part sharing the
    // footprint may build a wall through them after this one.
    let curtains = config.recess_windows && floor_cells.is_some();
    let no_floor = FnvHashSet::default();
    let floor_cells = floor_cells.unwrap_or(&no_floor);
    let air_con = has_air_conditioners(config, &element.tags, facade);
    let boxes = has_window_boxes(config, &element.tags);
    if !(curtains || air_con || boxes) {
        return;
    }
    let columns = facade_columns(&element.nodes, config);
    let ring: FnvHashSet<(i32, i32)> = columns.iter().map(|c| (c.x, c.z)).collect();
    let abs = config.abs_terrain_offset;
    let top_h = config.start_y_offset + config.building_height;
    let gft = config.ground_floor_top();
    let sill = trim_slab(config.trim_block);
    let empty = |editor: &WorldEditor, x: i32, h: i32, z: i32| {
        editor
            .get_block_absolute(x, h + abs, z)
            .is_none_or(|b| b == AIR)
    };

    for wc in &columns {
        let BaySlot::Window { k, width, .. } = wc.slot() else {
            continue;
        };
        let (bx, bz) = (wc.x, wc.z);
        if facade.is_door(bx, bz)
            || front_blocked(wc, config, building_passages, facade, part_probe)
        {
            continue;
        }
        let (nx, nz) = wc.normal;
        let col = wc.facade(facade);
        let inner = (bx - nx, bz - nz);
        let behind = (bx - 2 * nx, bz - 2 * nz);
        let street = facade.has_any_street && facade.is_street(bx, bz);
        let (lx, lz) = (bx + nx, bz + nz);
        let (fx, fz) = (bx + 2 * nx, bz + 2 * nz);
        // Behind the glass is the building's own open floor, not a wall
        // and not the far side of a thin building.
        // Another building or part close by may build its own wall through
        // these cells after this one, where a curtain would take its glass.
        let room_behind = floor_cells.contains(&behind)
            && !ring.contains(&behind)
            && footprints.is_none_or(|f| {
                (-2..=2).all(|dx| {
                    (-2..=2).all(|dz| {
                        let c = (behind.0 + dx, behind.1 + dz);
                        !f.contains(c.0, c.1) || floor_cells.contains(&c) || ring.contains(&c)
                    })
                })
            });

        for h in (config.start_y_offset + 1)..=top_h {
            if h <= gft && config.shopfront_column(col) {
                continue;
            }
            if !glazed_at(h, config, col) {
                continue;
            }
            // The glass stands a block behind an open wall face.
            let recessed = empty(editor, bx, h, bz)
                && editor
                    .get_block_absolute(inner.0, h + abs, inner.1)
                    .is_some_and(|b| b == config.window_block || b == GLASS);
            if !recessed {
                continue;
            }
            let storey = storey_of(config, h);
            let lowest = !glazed_at(h - 1, config, col);
            let top_row = !glazed_at(h + 1, config, col);

            // Never behind a balcony: the cells before the window are open.
            if air_con
                && street
                && lowest
                && h > gft
                && empty(editor, lx, h, lz)
                && empty(editor, fx, h - 1, fz)
            {
                let unit =
                    window_rng(config, wc, storey, 0xA1C0_0D17_0000_0003).and_then(|mut rng| {
                        (rng.random_range(0..100u32) < AIR_CONDITIONER_PERCENT)
                            .then(|| rng.random_range(0..width))
                    });
                if unit == Some(k) {
                    editor.set_block_absolute(SMOOTH_STONE, bx, h + abs, bz, Some(&[AIR]), None);
                }
            }

            if curtains && room_behind {
                if let Some((colour, draw)) = window_curtain(config, wc, storey) {
                    if curtain_covers(draw, k, width, top_row) {
                        editor.set_block_absolute(
                            colour,
                            behind.0,
                            h + abs,
                            behind.1,
                            Some(&[AIR]),
                            None,
                        );
                    }
                }
            }
        }

        // Window boxes stand on the sill, under an upper window, wherever
        // nothing else stands before it.
        if !boxes {
            continue;
        }
        for h in (gft + 1)..top_h {
            if config.floor_row(h) != 0
                || !glazed_at(h + 1, config, col)
                || !window_box_at(config, wc, h)
                || editor.get_block_absolute(lx, h + abs, lz) != Some(sill)
                || !empty(editor, lx, h + 1, lz)
                || !empty(editor, fx, h, fz)
                || !empty(editor, fx, h + 1, fz)
            {
                continue;
            }
            let flowering =
                coord_rng(bx, bz.wrapping_add(h * 13), config.element_id ^ 0xF10B).random_bool(0.6);
            let plant = if flowering {
                FLOWERING_AZALEA_LEAVES
            } else {
                AZALEA_LEAVES
            };
            editor.set_block_absolute(plant, lx, h + 1 + abs, lz, Some(&[AIR]), None);
        }
    }
}

/// Cornice and string course of a masonry facade: an upside-down stair row
/// where the wall ends, under a slab cap on a flat roof or under the eave of a
/// pitched one, a dentil row below it on ornate fabric, and a slab ledge over
/// the base, which over a shopfront doubles as its cornice. All of it stands a
/// block out from the wall, clear of the parapet on the wall itself.
fn generate_facade_cornices(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    has_sloped_roof: bool,
    building_passages: &CoordinateBitmap,
    facade: &FacadePlan,
    part_probe: Option<(&CoordinateBitmap, &FnvHashSet<(i32, i32)>)>,
) {
    if config.condition != BuildingCondition::Normal
        || !config.punched_windows()
        || config.building_height < 2 * config.floor_cycle
        || config.window_archetype == WindowArchetype::Ribbon
        || matches!(
            config.category,
            BuildingCategory::Industrial
                | BuildingCategory::Warehouse
                | BuildingCategory::Farm
                | BuildingCategory::Religious
                | BuildingCategory::ContemporarySkyscraper
        )
    {
        return;
    }
    // Pre-war masonry, and the rendered fabric of European towns, which is
    // corniced and coursed whatever its wall.
    let europe = config.region == BuildingRegion::Europe;
    let masonry = matches!(
        config.era,
        ArchEra::TraditionalPreWar | ArchEra::HistoricOrnate
    ) || (config.era == ArchEra::Unknown
        && (europe || is_masonry_wall(config.wall_block)));
    let ornate =
        config.era == ArchEra::HistoricOrnate || config.category == BuildingCategory::Historic;
    let floors = config.building_height / config.floor_cycle;
    let string_course = config.is_ground_level
        && ((masonry && floors >= if europe { 2 } else { 3 })
            || config.ground == GroundFloor::Shopfront);
    if !masonry && !string_course {
        return;
    }
    let top_h = config.start_y_offset + config.building_height;
    let course_h = config.ground_floor_top() + 1;
    let stair = trim_stair(config.trim_block);
    let slab = trim_slab(config.trim_block);
    let ledge = make_top_slab(slab);
    let cap = cached_prop_block(slab, &[("type", "bottom")]);

    for wc in facade_columns(&element.nodes, config) {
        if front_blocked(&wc, config, building_passages, facade, part_probe) {
            continue;
        }
        let (lx, lz) = (wc.x + wc.normal.0, wc.z + wc.normal.1);
        let facing = facing_for_normal(wc.normal.0, wc.normal.1);
        // Dentils need solid wall behind them, not a window head.
        let dentil = ornate && wc.pos % 2 == 0 && !glazed_at(top_h - 1, config, wc.facade(facade));
        let mut place = |block: BlockWithProperties, h: i32| {
            editor.set_block_with_properties_absolute(
                block,
                lx,
                h + config.abs_terrain_offset,
                lz,
                Some(&[AIR]),
                None,
            );
        };
        if masonry {
            // A pitched roof's eave overhang sits on the row above.
            place(upside_down_stair(stair, facing), top_h);
            if !has_sloped_roof {
                place(cap.clone(), top_h + 1);
            }
            if dentil {
                place(ledge.clone(), top_h - 1);
            }
        }
        if string_course && course_h < top_h - 1 {
            place(ledge.clone(), course_h);
        }
    }
}

/// Awnings over the display windows of a shopfront: a trapdoor canopy along
/// the top of the glass, under the sign band, one colour per building.
fn generate_storefront_awnings(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    building_passages: &CoordinateBitmap,
    facade: &FacadePlan,
) {
    if config.ground != GroundFloor::Shopfront || config.condition != BuildingCondition::Normal {
        return;
    }
    let mut rng = element_rng(config.style_seed ^ 0x0A3B_11B6_0000_000B);
    if !rng.random_bool(0.55) {
        return;
    }
    const AWNING_OPTIONS: [Block; 5] = [
        WARPED_TRAPDOOR,
        SPRUCE_TRAPDOOR,
        DARK_OAK_TRAPDOOR,
        JUNGLE_TRAPDOOR,
        ACACIA_TRAPDOOR,
    ];
    let awning = AWNING_OPTIONS[rng.random_range(0..AWNING_OPTIONS.len())];
    let awning_y = config.ground_floor_top() - 1 + config.abs_terrain_offset;

    for wc in facade_columns(&element.nodes, config) {
        if front_blocked(&wc, config, building_passages, facade, None)
            || !config.shopfront_column(wc.facade(facade))
            || !wc.shop_slot().is_window()
        {
            continue;
        }
        editor.set_block_with_properties_absolute(
            make_closed_trapdoor(awning, facing_for_normal(wc.normal.0, wc.normal.1), "top"),
            wc.x + wc.normal.0,
            awning_y,
            wc.z + wc.normal.1,
            Some(&[AIR]),
            None,
        );
    }
}

/// Vertical drainpipe runs hugging two facade corners, a staple of hand-built city blocks.
fn generate_corner_downpipes(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    building_passages: &CoordinateBitmap,
    facade: &FacadePlan,
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
    let outward = outward_side(&element.nodes);

    // Convex corners of two perpendicular walls, with the pipe cell on the
    // diagonal both walls' outward normals point to. Re-entrant corners
    // would put it indoors and mid-wall nodes are not corners at all.
    let vertices = ring_vertices(&element.nodes);
    let n = vertices.len();
    if n < 3 {
        return;
    }
    let corners: Vec<((i32, i32), (i32, i32))> = (0..n)
        .filter_map(|i| {
            let p = vertices[(i + n - 1) % n];
            let c = vertices[i];
            let q = vertices[(i + 1) % n];
            if !corner_is_convex(p, c, q, outward) {
                return None;
            }
            let (ax, az) = compute_outward_normal(p.0, p.1, c.0, c.1, outward);
            let (bx, bz) = compute_outward_normal(c.0, c.1, q.0, q.1, outward);
            let (dx, dz) = (ax + bx, az + bz);
            (dx != 0 && dz != 0).then_some((c, (dx, dz)))
        })
        .collect();
    if corners.is_empty() {
        return;
    }

    let pipe = get_wall_piece_for_material(config.wall_block);
    let start_idx = rng.random_range(0..corners.len());
    for k in 0..2usize {
        let ((px, pz), (dx, dz)) = corners[(start_idx + k * corners.len() / 2) % corners.len()];
        // A pipe off a photographed corner stands in front of the photograph;
        // one off a party-wall corner stands in front of the neighbour.
        if building_passages.contains(px, pz)
            || config.is_flat_column(px, pz)
            || facade.is_party(px, pz)
        {
            continue;
        }
        let (ox, oz) = (px + dx, pz + dz);
        // Follow the wall foundation down where the ground drops away.
        let descent = if config.is_ground_level {
            feature_descent(editor, config, px, pz, ox, oz)
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

/// ReligiousButtress: stepped buttresses on the pier after every other
/// window, plus a cornice at the roof line. Buttresses extend 2 blocks outward
/// at the lower portion and 1 block outward for the full height.
fn place_religious_buttress(
    editor: &mut WorldEditor,
    config: &BuildingConfig,
    wc: &WallColumn,
    facing: &str,
    height_reduction: i32,
    descent: i32,
) {
    let (out_nx, out_nz) = wc.normal;
    let lx = wc.x + out_nx;
    let lz = wc.z + out_nz;
    let top_h = config.start_y_offset + config.building_height - height_reduction;

    let buttress = !wc.slot().is_window()
        && matches!(wc.slot_at(-1), BaySlot::Window { bay, .. } if bay % 2 == 1);
    if buttress {
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
        for h in (config.start_y_offset + 1 - descent)..=buttress_cutoff {
            editor.set_block_absolute(
                config.wall_block,
                lx + out_nx,
                h + config.abs_terrain_offset,
                lz + out_nz,
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
    open_hall: bool,
    furnished: bool,
    lid: &RoofMembrane,
) -> HashSet<(i32, i32)> {
    let mut processed_points: HashSet<(i32, i32)> = HashSet::new();
    let ceiling_light_block = if config.is_abandoned_building {
        COBWEB
    } else {
        GLOWSTONE
    };
    // An empty shell is lit by invisible light hung under each slab, so no
    // lamp grid shows through the windows; furnished interiors keep their
    // glowstone, which the furnishing plans around.
    let hidden_lights = !furnished && !config.is_abandoned_building;

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

        // Set intermediate ceilings with light fixtures; a furnished hall is one room
        let top = config.start_y_offset + config.building_height;
        let mut top_floor = config.start_y_offset;
        if config.building_height > config.floor_cycle && !open_hall {
            for h in (config.start_y_offset + config.grammar_anchor() + config.floor_cycle..top)
                .step_by(config.floor_cycle as usize)
            {
                top_floor = h;
                // Skip intermediate ceilings below passage opening
                if is_passage && h <= config.start_y_offset + passage_height {
                    continue;
                }

                let lamp = x % 3 == 0 && z % 3 == 0;
                let block = if lamp && !hidden_lights {
                    ceiling_light_block
                } else {
                    config.floor_block
                };
                editor.set_block_absolute(block, x, h + config.abs_terrain_offset, z, None, None);
                if lamp && hidden_lights {
                    editor.set_block_absolute(
                        LIGHT,
                        x,
                        h - 1 + config.abs_terrain_offset,
                        z,
                        None,
                        None,
                    );
                }
            }
        }
        // The top storey has no slab above for lights, so they hang in its top row.
        let top_headroom = top - top_floor;
        let under_passage = is_passage && top <= config.start_y_offset + passage_height;
        if x % 3 == 0
            && z % 3 == 0
            && !under_passage
            && (top_headroom >= 3 || top_floor == config.start_y_offset)
        {
            editor.set_block_absolute(
                if hidden_lights {
                    LIGHT
                } else {
                    ceiling_light_block
                },
                x,
                top + config.abs_terrain_offset,
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

        // The lid of a roof the roof pass skips (a part's top), in the roof
        // membrane rather than the floor block: oak planks or mud bricks make
        // no roof. Construction sites and ruins stay open at the top.
        let has_flat_roof = !generate_non_flat_roof;
        let skip_top = matches!(
            config.condition,
            BuildingCondition::Construction | BuildingCondition::Ruined
        );

        if has_flat_roof && !skip_top {
            editor.set_block_absolute(
                lid.block_at(x, z),
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
        "hipped" | "hip" | "side_hipped" => RoofType::Hipped,
        "round" | "barrel" | "vault" | "vaulted" => RoofType::Round,
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

    // A ring too thin to enclose a lattice cell, such as a column or a beam
    // mapped as a building:part, stands on its outline cells instead of
    // vanishing.
    // An oversized ring is empty because the fill refused it, not because it is thin.
    if cached_floor_area.is_empty()
        && hole_polygons.is_none_or(|h| h.is_empty())
        && !crate::floodfill_cache::is_oversized_ring(element)
    {
        cached_floor_area = thin_ring_cells(&element.nodes);
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
                crate::element_processing::parking_garage::generate_parking_garage(
                    editor,
                    &cached_floor_area,
                    height,
                    start_y_offset + abs_terrain_offset,
                    group_seed,
                );
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
            crate::element_processing::parking_garage::generate_parking_garage(
                editor,
                &cached_floor_area,
                height,
                start_y_offset + abs_terrain_offset,
                group_seed,
            );
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
    // The building's own footprint, which is what the preset facades measure a
    // neighbour against: a wall of one `building:part` that stands against
    // another part of the same building faces that part all the same, and
    // its panel is hung above that part's roof or not at all. The facade plan
    // keeps counting the group as its own, so the procedural walls between
    // two parts stay as they are.
    let own_fill: FnvHashSet<(i32, i32)> = cached_floor_area.iter().copied().collect();
    let mut facade = if min_level_offset == 0 && cached_footprint_size >= MIN_FACADE_FOOTPRINT {
        let mut own_cells = own_fill.clone();
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
    let roof_ctx = {
        let (lat, lon) = editor.geo_anchor();
        let (mut party, mut total) = (0, 0);
        for seg in facade.segments.iter().flatten() {
            total += seg.len;
            if seg.class == FacadeClass::Party {
                party += seg.len;
            }
        }
        // Parts of one building take the district at the centre of the whole
        // group, so a cell seam never splits one building's materials.
        let (mut min_x, mut max_x, mut min_z, mut max_z) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
        let group_cells = group_other_cells.iter().copied();
        for (x, z) in element.nodes.iter().map(|n| (n.x, n.z)).chain(group_cells) {
            (min_x, max_x) = (min_x.min(x), max_x.max(x));
            (min_z, max_z) = (min_z.min(z), max_z.max(z));
        }
        let own: FnvHashSet<(i32, i32)> = cached_floor_area.iter().copied().collect();
        let (mut ring, mut walled) = (0usize, 0usize);
        for &(x, z) in cached_floor_area.iter() {
            for c in [(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)] {
                if !own.contains(&c) {
                    ring += 1;
                    if ctx.building_footprints.contains(c.0, c.1) {
                        walled += 1;
                    }
                }
            }
        }
        RoofContext {
            enclosed: ring > 0 && walled * 10 >= ring * 9,
            region: BuildingRegion::at(lat, lon),
            attached: party >= 4 && party * 100 >= total * 15,
            scale: scale_factor,
            materials: MaterialRegion::at(lat, lon),
            district: district_seed(
                min_x.saturating_add(max_x) / 2,
                min_z.saturating_add(max_z) / 2,
                scale_factor,
            ),
        }
    };
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
        roof_ctx,
        &mut rng,
    );
    // What a flat roof is covered in: a mapped colour or material, a preset's
    // block, else a membrane for the region (shared by the parts of a group).
    let roof_cover = mapped_roof_block(&element.tags, group_seed)
        .or(style.roof_block)
        .map(RoofMembrane::uniform)
        .unwrap_or_else(|| {
            rooftops::pick_membrane(
                roof_ctx.region,
                roof_ctx.materials,
                category,
                era,
                building_height,
                group_seed,
            )
        });
    // Massing the data does not show (a podium and tower, a setback crown)
    // stays with the skyscraper presets and mapped roof shapes, which ran the
    // roof pass before every flat roof did.
    let invented_massing =
        preset.generate_roof == Some(true) || element.tags.contains_key("roof:shape");

    let condition = BuildingCondition::from_tags(&element.tags);
    let is_abandoned_building = matches!(
        condition,
        BuildingCondition::Abandoned | BuildingCondition::Ruined
    );

    let mut wall_block = style.wall_block;
    // A column has no room for a window or a door.
    let is_thin_ring = cached_footprint_size <= THIN_RING_MAX_CELLS;
    let mut has_windows = style.has_windows && !is_thin_ring;
    let mut has_garage_door = style.has_garage_door && !is_thin_ring;
    let mut has_single_door = style.has_single_door && !is_thin_ring;
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

    // OSM `height` is the top of the roof. A pitched roof the generator adds
    // takes its rise off the walls, so the building ends up as tall as mapped
    // instead of a roof taller.
    let roof_rise_hint = mapped_height_roof_rise(
        element,
        &style,
        condition,
        effective_building_height,
        cached_footprint_size,
        category == BuildingCategory::Religious,
        scale_factor,
    );
    if let Some(rise) = roof_rise_hint {
        effective_building_height = (effective_building_height - rise).max(3);
    }

    // Whether this building will get a sloped roof (drives the attic band).
    let has_sloped_roof = style.generate_roof
        && style.roof_type != RoofType::Flat
        && !matches!(
            condition,
            BuildingCondition::Construction | BuildingCondition::Ruined
        );

    // Window layout is picked before the config literal so the frame gate
    // below can see it (frames dress punched windows only).
    let window_archetype =
        pick_window_archetype(category, era, roof_ctx.region, wall_block, group_seed);

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
    // Window surrounds on historic European stucco and sandstone, which
    // space their windows a column further apart so a strip of plain wall
    // stays between two frames. Brick keeps its plain openings.
    let surrounds = roof_ctx.region == BuildingRegion::Europe
        && has_windows
        && !matches!(
            wall_block,
            BRICK
                | TERRACOTTA
                | RED_TERRACOTTA
                | ORANGE_TERRACOTTA
                | BROWN_TERRACOTTA
                | NETHER_BRICK
                | RED_NETHER_BRICKS
                | MUD_BRICKS
                | GRANITE
                | POLISHED_GRANITE
                | BROWN_CONCRETE
        )
        && condition == BuildingCondition::Normal
        && detail >= DetailTier::Standard
        && !is_tall_building
        && matches!(
            window_archetype,
            WindowArchetype::Spaced | WindowArchetype::Portrait | WindowArchetype::Hooded
        )
        && {
            let chance = match (era, category) {
                (ArchEra::HistoricOrnate, _) | (_, BuildingCategory::Historic) => 1.0,
                (ArchEra::TraditionalPreWar, _) => 0.6,
                (
                    ArchEra::Unknown,
                    BuildingCategory::Residential
                    | BuildingCategory::Default
                    | BuildingCategory::Commercial
                    | BuildingCategory::Hotel
                    | BuildingCategory::Office,
                ) => 0.35,
                _ => 0.0,
            };
            chance > 0.0 && element_rng(group_seed ^ 0xFA5C_4E40_0000_0001).random_bool(chance)
        };
    let piano_nobile = has_windows
        && effective_building_height >= 3 * floor_cycle
        && match category {
            BuildingCategory::Historic => {
                element_rng(group_seed ^ 0xF10A_401E_0000_0003).random_bool(0.50)
            }
            BuildingCategory::Hotel => {
                element_rng(group_seed ^ 0xF10A_401E_0000_0003).random_bool(0.20)
            }
            // Baroque and historicist town houses dress their first floor.
            BuildingCategory::Residential
            | BuildingCategory::Default
            | BuildingCategory::Commercial
                if roof_ctx.region == BuildingRegion::Europe && era == ArchEra::HistoricOrnate =>
            {
                element_rng(group_seed ^ 0xF10A_401E_0000_0003).random_bool(0.60)
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
            || flat_parapet_for(
                group_seed,
                style.roof_type,
                effective_building_height,
                category,
                condition,
                roof_ctx.region,
            ),
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
        bays: {
            let mut bays = window_archetype.rhythm();
            bays.pier += i32::from(surrounds);
            bays
        },
        balcony_band: pick_balcony_band(
            category,
            era,
            roof_ctx.region,
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
            // Every pre-war European house stands on a plinth.
            let old_europe = roof_ctx.region == BuildingRegion::Europe
                && !matches!(era, ArchEra::PostWarPanel | ArchEra::Contemporary);
            (eligible
                && base != wall_block
                && (old_europe
                    || element_rng(group_seed ^ 0xBA5E_C0A2_5E11_0001).random_bool(0.70)))
            .then_some(base)
        },
        ground: if has_windows && condition == BuildingCondition::Normal && min_level_offset == 0 {
            pick_ground_floor(
                element,
                category,
                effective_building_height / floor_cycle,
                roof_ctx.region,
                roof_ctx.attached,
                &facade,
                group_seed,
            )
        } else {
            GroundFloor::Plain
        },
        trim_block: trim_block_for_wall(wall_block, group_seed),
        surrounds,
        shop_frame: {
            let options = shop_frame_options(roof_ctx.region);
            options[element_rng(group_seed ^ 0x5409_F2A3_0000_0003).random_range(0..options.len())]
        },
        recess_windows: has_windows && !args.interior,
        region: roof_ctx.region,
        window_frame: (has_windows
            && condition == BuildingCondition::Normal
            && !is_tall_building
            && !matches!(
                window_archetype,
                WindowArchetype::Ribbon | WindowArchetype::Piers | WindowArchetype::Strips
            ))
        .then(|| pick_window_frame(category, era, detail, wall_block, group_seed))
        .flatten(),
        facade_shell: photo_shell_columns(element),
        // Filled in below, once the podium massing has settled the wall height
        // `building_facades::collect` will be given.
        preset_shell: Arc::default(),
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
        style.generate_roof && invented_massing,
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
    // The preset facades are recorded before the wall is built, not after it:
    // the columns they return are the columns built as a plain shell, so the
    // shell is exactly what the photographs cover and a wall they hang nothing
    // on is built exactly as it is with the feature off. Nothing is written
    // into the world here; the panels are hung once it is finished.
    let config = BuildingConfig {
        preset_shell: crate::building_facades::collect_facing(
            editor,
            &element.nodes,
            config.element_id,
            // The shared style seed, so the parts of one building hang one
            // photograph the way they already share one wall block.
            config.style_seed,
            config.category,
            config.start_y_offset,
            config.abs_terrain_offset,
            config.building_height,
            // A wall column faces into another building when the cell one or
            // two out from it belongs to a footprint that is not this
            // element's own: the party test of `compute_facade_plan`, asked
            // per wall direction so a corner is party along one wall and
            // free along the other, and against the element alone so a part
            // standing against another part of its building counts too. An
            // elevated part stands on the building below it and faces nothing
            // at ground level, so it is not asked.
            &|bx, bz, (ox, oz)| {
                config.is_ground_level
                    && (1..=2).any(|d| {
                        let c = (bx + ox * d, bz + oz * d);
                        ctx.building_footprints.contains(c.0, c.1) && !own_fill.contains(&c)
                    })
            },
        ),
        ..config
    };
    // Interiors must stop at the podium roof or rooms would float.
    let effective_building_height = match &podium_tower {
        Some(plan) => plan.podium_height,
        None => effective_building_height,
    };

    // Interior uses per storey, planned before doors and floors: shared shop floors
    // get a door per shop, open halls lose their storey ceilings.
    let storey_levels = calculate_floor_levels(
        start_y_offset,
        effective_building_height,
        config.floor_cycle,
        config.grammar_anchor(),
    );
    let interior_plan = (args.interior
        && cached_floor_area.len() >= MIN_INTERIOR_CELLS
        && !matches!(
            building_type,
            "garage"
                | "garages"
                | "carport"
                | "shed"
                | "parking"
                | "roof"
                | "bridge"
                | "greenhouse"
                | "glasshouse"
                | "silo"
                | "storage_tank"
                | "transformer_tower"
                | "water_tower"
                | "service"
                | "toilets"
                | "bunker"
                | "container"
                | "outbuilding"
                | "allotment_house"
                | "boathouse"
                | "pavilion"
        )
        && !matches!(
            config.condition,
            BuildingCondition::Construction | BuildingCondition::Ruined
        ))
    .then(|| {
        plan_interior(&PlanInputs {
            tags: &element.tags,
            building_type,
            floors: storey_levels.len(),
            min_level,
            elevated: !config.is_ground_level,
            footprint: cached_floor_area.len(),
            center: (
                (bounds.min_x + bounds.max_x) / 2,
                (bounds.min_z + bounds.max_z) / 2,
            ),
            tenants: ctx.interior_uses.tenants(element.id),
            area: ctx.interior_uses.area(element.id),
        })
    });
    let open_hall = interior_plan.as_ref().is_some_and(|p| p.open_hall);

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
    // Shops sharing the ground floor each get a street door of their own.
    if let Some(units) = interior_plan
        .as_ref()
        .and_then(|p| p.floors.first())
        .filter(|units| units.len() >= 2)
    {
        let anchors: Vec<(i32, i32)> = units.iter().map(|u| u.anchor).collect();
        let extra = plan_unit_entrances(
            element,
            &config,
            &facade,
            effective_passages,
            group_seed,
            &anchors,
            &entrance_plans,
        );
        entrance_plans.extend(extra);
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

    let (wall_outline, corner_count, wall_segments) = build_wall_ring(
        editor,
        &element.nodes,
        &config,
        args,
        has_sloped_roof,
        effective_passages,
        &facade,
        Some(&own_fill),
    );

    // A courtyard ring's normals point into the building, so its glass
    // stays in the wall plane.
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
                    None,
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

    // Facade relief: window dressing, balconies, wall depth features and
    // cornices. building:part sub-sections get them too, with a footprint
    // probe so protrusions stay out of other buildings' cells (sibling parts
    // count as this building's own).
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
    generate_window_frames(
        editor,
        element,
        &config,
        effective_passages,
        &facade,
        part_probe,
    );
    if !is_part {
        generate_balconies(editor, element, &config, effective_passages, &facade);
    }
    generate_window_life(
        editor,
        element,
        &config,
        effective_passages,
        &facade,
        part_probe,
        (!is_part && group_other_cells.is_empty()).then_some(&own_fill),
        Some(ctx.building_footprints),
    );
    generate_wall_depth_features(
        editor,
        element,
        &config,
        has_sloped_roof,
        effective_passages,
        &facade,
        part_probe,
    );

    generate_facade_cornices(
        editor,
        element,
        &config,
        has_sloped_roof,
        effective_passages,
        &facade,
        part_probe,
    );

    // Add corner quoins (accent-block columns at building corners)
    if !is_part {
        generate_corner_quoins(editor, element, &config, effective_passages, &facade);
        generate_corner_downpipes(editor, element, &config, effective_passages, &facade);
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
            open_hall,
            interior_plan.is_some(),
            &roof_cover,
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
        if let Some(plan) = &interior_plan {
            let floor_levels = if open_hall {
                vec![start_y_offset]
            } else {
                storey_levels
            };
            // The cell inside each leaf of every ground-level door.
            let entrances: Vec<Entry> = if config.is_ground_level {
                entrance_plans
                    .iter()
                    .flat_map(|p| {
                        let leaves = if p.double { 2 } else { 1 };
                        (0..leaves).map(move |t| Entry {
                            cell: (
                                p.x + p.tangent.0 * t - p.normal.0,
                                p.z + p.tangent.1 * t - p.normal.1,
                            ),
                            inward: (-p.normal.0, -p.normal.1),
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            };
            generate_building_interior(
                editor,
                &InteriorRequest {
                    footprint: &cached_floor_area,
                    floor_levels: &floor_levels,
                    start_y_offset,
                    building_height: effective_building_height,
                    abs_terrain_offset,
                    wall_block: style.wall_block,
                    plan,
                    abandoned: is_abandoned_building,
                    passages: effective_passages,
                    entrances: &entrances,
                    bounds: ((bounds.min_x, bounds.min_z), (bounds.max_x, bounds.max_z)),
                    claims: ctx.interior_uses.claims(element.id),
                    scale: args.scale,
                    floor_block: config.floor_block,
                    seed: group_seed,
                },
            );
        }
    }

    // Construction/Ruined buildings stay roofless.
    let skip_roof = matches!(
        config.condition,
        BuildingCondition::Construction | BuildingCondition::Ruined
    );
    if style.generate_roof && !skip_roof {
        // Terraced fabric prefers its ridge parallel to the fronting street.
        let preferred_ridge_along_x =
            if matches!(building_type, "terrace" | "semidetached_house") || roof_ctx.attached {
                facade
                    .front_segment
                    .and_then(|i| facade.segments[i].as_ref())
                    .map(|seg| seg.normal.0 == 0)
            } else {
                None
            };
        let covered_by_sibling_part = roof_ctx.enclosed
            || (is_part
                && cached_floor_area
                    .iter()
                    .any(|c| group_other_cells.contains(c)));
        let kit = FlatRoofKit {
            region: roof_ctx.region,
            cover: roof_cover,
            facade: &facade,
            residential: matches!(
                category,
                BuildingCategory::House | BuildingCategory::Residential
            ) || is_residential_type(building_type),
            explicit_pass: invented_massing,
        };
        // Cells within an overhang's reach that belong to another building
        // or part: a pitched roof's eaves stop short of them.
        let eave_blocked: FnvHashSet<(i32, i32)> = if style.roof_type == RoofType::Flat {
            FnvHashSet::default()
        } else {
            let own: HashSet<(i32, i32)> = roof_area.iter().copied().collect();
            let mut blocked = FnvHashSet::default();
            for &(x, z) in &roof_area {
                if ROOF_SIDES
                    .iter()
                    .all(|&(dx, dz)| own.contains(&(x + dx, z + dz)))
                {
                    continue;
                }
                for dx in -2..=2 {
                    for dz in -2..=2 {
                        let c = (x + dx, z + dz);
                        if !own.contains(&c)
                            && !own_fill.contains(&c)
                            && (ctx.building_footprints.contains(c.0, c.1)
                                || group_other_cells.contains(&c))
                        {
                            blocked.insert(c);
                        }
                    }
                }
            }
            blocked
        };
        let hole_rings: Vec<&[ProcessedNode]> = hole_polygons
            .unwrap_or_default()
            .iter()
            .map(|h| h.way.nodes.as_slice())
            .collect();
        generate_building_roof(
            editor,
            element,
            &config,
            &style,
            &roof_area,
            category,
            preferred_ridge_along_x,
            podium_tower.is_some(),
            covered_by_sibling_part,
            scale_factor,
            roof_rise_hint,
            &kit,
            roof_ctx.region,
            &hole_rings,
            eave_blocked,
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
                &roof_cover,
            );
            // The tower's own roof gets the parapet and plant a tower carries.
            let tower_cells: Vec<(i32, i32)> = roof_area
                .iter()
                .copied()
                .filter(|&(x, z)| dist.get(x, z) >= plan.inset)
                .collect();
            fit_out_flat_roof(
                editor,
                element,
                &tower_config,
                category,
                &tower_cells,
                &empty_facade,
                &kit,
                scale_factor,
            );
        }
    }

    // After every wall and decoration pass, so the seam audit sees the world
    // the player would.
    report_wall_segments(editor, element, &config, effective_passages, &wall_segments);

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
    // A shopfront's own sign band, over its glazing and awning; otherwise the
    // first row above the ground floor.
    let fascia_y = if config.ground == GroundFloor::Shopfront {
        config.ground_floor_top() + abs
    } else {
        config.ground_floor_top() + 1 + abs
    };
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

/// What the flat-roof pass needs beyond the building config.
struct FlatRoofKit<'a> {
    region: BuildingRegion,
    /// The roof covering, see `RoofMembrane`.
    cover: RoofMembrane,
    /// Party walls, where a neighbour may already stand as high as the roof.
    facade: &'a FacadePlan,
    /// Homes: a terrace or podium roof people use gets a railing.
    residential: bool,
    /// A preset or a mapped roof:shape asked for the roof pass, which ran only
    /// for those before every flat roof got one. A setback crown, which the
    /// data does not show, and the hospital's wall crosses stay with them.
    explicit_pass: bool,
}

/// Building types that are homes.
fn is_residential_type(building_type: &str) -> bool {
    matches!(
        building_type,
        "apartments"
            | "residential"
            | "dormitory"
            | "house"
            | "detached"
            | "semidetached_house"
            | "terrace"
            | "bungalow"
            | "villa"
    )
}

/// What tops a parapet course.
#[derive(Clone, Copy)]
enum ParapetCap {
    /// A coping slab.
    Coping(Block),
    /// A steel railing, where people use the roof.
    Railing,
}

/// The neighbour on the party wall at (x, z) already stands at least as high
/// as this roof surface, so a parapet there would only double its wall.
fn neighbour_reaches_roof(
    editor: &WorldEditor,
    dist: &RoofDistanceGrid,
    x: i32,
    z: i32,
    surface_y: i32,
) -> bool {
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .filter(|&(dx, dz)| dist.get(x + dx, z + dz) < 0)
        .any(|(dx, dz)| {
            (1..=2).any(|k| {
                editor
                    .highest_block_between(x + dx * k, z + dz * k, surface_y, surface_y + 1)
                    .is_some()
            })
        })
}

/// Parapet round a flat roof: one course of the wall block on the roof's outer
/// ring, capped with a coping slab or a railing. On a party wall the course is
/// left out where the neighbour already stands as high as this roof, so two
/// buildings of one height share one parapet line. It carries no slab there
/// either: a taller neighbour raised later over the same cells would keep a
/// half-block notch in its wall.
fn generate_parapet(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    cells: &[(i32, i32)],
    facade: &FacadePlan,
    cap: ParapetCap,
) {
    if !config.has_parapet || cells.is_empty() {
        return;
    }
    let dist = roof_edge_distances(cells);
    let surface_y = config.start_y_offset + config.building_height + config.abs_terrain_offset + 1;
    let course_y = surface_y + 1;
    // Modern towers cap the parapet in their accent band's stone.
    let modern = config.category == BuildingCategory::ModernSkyscraper;
    let cap = match cap {
        ParapetCap::Coping(_) if modern => {
            ParapetCap::Coping(get_slab_block_for_material(config.accent_block))
        }
        cap => cap,
    };
    for &(x, z) in cells {
        if dist.get(x, z) != 0 {
            continue;
        }
        let party = facade.is_party(x, z);
        if party && neighbour_reaches_roof(editor, &dist, x, z, surface_y) {
            continue;
        }
        editor.set_block_absolute(config.wall_block, x, course_y, z, Some(&[AIR]), None);
        if party {
            continue;
        }
        match cap {
            ParapetCap::Coping(slab) => editor.set_block_with_properties_absolute(
                rooftops::bottom_slab(slab),
                x,
                course_y + 1,
                z,
                Some(&[AIR]),
                None,
            ),
            ParapetCap::Railing => crate::element_processing::connected_blocks::place_connected(
                editor,
                IRON_BARS,
                x,
                course_y + 1,
                z,
            ),
        }
    }

    // Corner posts in the accent block on modern towers.
    if modern {
        for node in &element.nodes {
            if dist.get(node.x, node.z) == 0 && !facade.is_party(node.x, node.z) {
                editor.set_block_absolute(
                    config.accent_block,
                    node.x,
                    course_y + 1,
                    node.z,
                    None,
                    Some(&[]),
                );
            }
        }
    }
}

/// A flat roof without a parapet still ends in a crisp edge: on most of them
/// a curb of coping slabs round the outer ring, like metal flashing.
fn generate_flat_roof_edge_variation(
    editor: &mut WorldEditor,
    config: &BuildingConfig,
    cells: &[(i32, i32)],
    facade: &FacadePlan,
    coping: Block,
) {
    if cells.is_empty()
        || matches!(
            config.category,
            BuildingCategory::Shed | BuildingCategory::Greenhouse
        )
        || !element_rng(config.style_seed ^ 0xED6E_C0B1_0000_0001).random_bool(0.6)
    {
        return;
    }
    let dist = roof_edge_distances(cells);
    let curb_y = config.start_y_offset + config.building_height + config.abs_terrain_offset + 2;
    let curb = rooftops::bottom_slab(coping);
    for &(x, z) in cells {
        if dist.get(x, z) == 0 && !facade.is_party(x, z) {
            editor.set_block_with_properties_absolute(
                curb.clone(),
                x,
                curb_y,
                z,
                Some(&[AIR]),
                None,
            );
        }
    }
}

/// What the rooftop fit-out needs to know about this building.
fn rooftop_brief(
    element: &ProcessedWay,
    config: &BuildingConfig,
    category: BuildingCategory,
    kit: &FlatRoofKit,
    scale_factor: f64,
) -> RooftopBrief {
    RooftopBrief {
        region: kit.region,
        category,
        era: config.era,
        height: config.building_height,
        floor_cycle: config.floor_cycle,
        wall: config.wall_block,
        scale: scale_factor,
        seed: element.id,
        residential: kit.residential,
        whole_building: !element.tags.contains_key("building:part"),
        sparse: config.detail == DetailTier::Minimal
            || config.condition != BuildingCondition::Normal,
        green: kit.cover.is_green(),
    }
}

/// Plant on the flat roof over `cells`: a fit-out for the building's kind and
/// region, or a planted terrace on a residential tower part.
#[allow(clippy::too_many_arguments)]
fn plant_flat_roof(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    category: BuildingCategory,
    cells: &[(i32, i32)],
    kit: &FlatRoofKit,
    scale_factor: f64,
    terrace: bool,
) {
    let surface_y = config.start_y_offset + config.building_height + config.abs_terrain_offset + 1;
    let Some(mut deck) = RoofDeck::new(cells, surface_y) else {
        return;
    };
    let brief = rooftop_brief(element, config, category, kit, scale_factor);
    if terrace {
        rooftops::fit_out_terrace(editor, &mut deck, &brief);
        return;
    }
    if category == BuildingCategory::Hospital {
        // Clear of the helipad laid on the middle of the roof afterwards.
        let bounds = BuildingBounds::from_nodes(&element.nodes);
        let (cx, cz) = (
            (bounds.min_x + bounds.max_x) / 2,
            (bounds.min_z + bounds.max_z) / 2,
        );
        deck.reserve(cx - 3, cz - 3, 7, 7);
    }
    rooftops::fit_out_roof(editor, &mut deck, &brief);
}

/// Parapet and plant on a tower's roof above its podium.
#[allow(clippy::too_many_arguments)]
fn fit_out_flat_roof(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    category: BuildingCategory,
    cells: &[(i32, i32)],
    facade: &FacadePlan,
    kit: &FlatRoofKit,
    scale_factor: f64,
) {
    let coping = rooftops::coping_for(config.wall_block, config.accent_block, config.style_seed);
    generate_parapet(
        editor,
        element,
        config,
        cells,
        facade,
        ParapetCap::Coping(coping),
    );
    if should_generate_rooftop_equipment(config, RoofType::Flat, category) {
        plant_flat_roof(
            editor,
            element,
            config,
            category,
            cells,
            kit,
            scale_factor,
            false,
        );
    }
}

/// Handles roof generation including chimney placement and rooftop equipment
#[allow(clippy::too_many_arguments)]
fn generate_building_roof(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    config: &BuildingConfig,
    style: &BuildingStyle,
    roof_area: &[(i32, i32)],
    category: BuildingCategory,
    preferred_ridge_along_x: Option<bool>,
    suppress_flat_extras: bool,
    covered_by_sibling_part: bool,
    scale_factor: f64,
    rise_hint: Option<i32>,
    kit: &FlatRoofKit,
    region: BuildingRegion,
    holes: &[&[ProcessedNode]],
    eave_blocked: FnvHashSet<(i32, i32)>,
) {
    // Dormers by region and kind: most pre-war European town roofs and
    // nearly every mansard carry a row, American roofs far fewer (Cape Cods).
    let storeys = config.building_height / config.floor_cycle.max(1);
    let town_block = category != BuildingCategory::House && storeys >= 3;
    let pre_war = matches!(
        config.era,
        ArchEra::Unknown | ArchEra::HistoricOrnate | ArchEra::TraditionalPreWar
    );
    let dormer_chance = match (style.roof_type, region) {
        (RoofType::Mansard, BuildingRegion::NorthAmerica) => 0.5,
        (RoofType::Mansard, _) => 0.95,
        (_, BuildingRegion::Europe) if !pre_war => 0.15,
        (_, BuildingRegion::Europe) if town_block => 0.65,
        (_, BuildingRegion::Europe) if category == BuildingCategory::House => 0.3,
        (_, BuildingRegion::Europe) => 0.25,
        (_, BuildingRegion::NorthAmerica) if category == BuildingCategory::House => 0.12,
        (_, BuildingRegion::NorthAmerica) => 0.06,
        _ => 0.15,
    };
    let add_dormers = config.condition == BuildingCondition::Normal
        && matches!(
            category,
            BuildingCategory::House
                | BuildingCategory::Residential
                | BuildingCategory::Commercial
                | BuildingCategory::Office
                | BuildingCategory::Hotel
                | BuildingCategory::Historic
        )
        && matches!(
            style.roof_type,
            RoofType::Gabled | RoofType::Hipped | RoofType::HalfHipped | RoofType::Mansard
        )
        && element_rng(config.style_seed ^ 0xD0E4_0000_5EED_0001).random_bool(dormer_chance);

    // Churches carry visibly steeper gables than houses.
    let steep_gable = category == BuildingCategory::Religious;

    // roof:colour/material on a part means the mapper modeled this surface, keep it clean
    let modeled_part_roof = element.tags.contains_key("building:part")
        && (element.tags.contains_key("roof:colour") || element.tags.contains_key("roof:material"));

    // A glass curtain wall is three window rows to one wall row, so a glass
    // roof continuing it takes the window glass, not the band's.
    let curtain_glass = (is_glass_wall(config.wall_block) && is_glass_wall(config.window_block))
        .then_some(config.window_block);

    let trim = RoofTrim {
        gable_block: (!is_glass_wall(config.wall_block)).then_some(config.wall_block),
        window_block: Some(config.window_block),
        window_phase: Some(config.window_phase),
        // Munich and its like show no fascia; detailed buildings elsewhere do.
        soffit: config.condition == BuildingCondition::Normal
            && (config.detail >= DetailTier::Enhanced
                || (config.detail >= DetailTier::Standard && region != BuildingRegion::Europe)),
        copper_dormers: config.era == ArchEra::HistoricOrnate
            && element_rng(config.style_seed ^ 0xC0FF_E400_0000_0001).random_bool(0.6),
        // A deep town block's capped top is a membrane; a house's stays tile.
        sheet_top: town_block,
        steep_cap: region == BuildingRegion::Europe,
        short_eave: region == BuildingRegion::Europe && town_block,
        blocked: eave_blocked,
        tone_patches: config.condition == BuildingCondition::Normal
            && !element.tags.contains_key("roof:colour")
            && !element.tags.contains_key("roof:material"),
    };

    // Generate the roof using the pre-determined roof type from style
    let surface = generate_roof(
        editor,
        element,
        config.start_y_offset,
        config.building_height,
        &kit.cover,
        config.wall_block,
        curtain_glass,
        config.roof_block,
        style.roof_type,
        roof_area,
        config.abs_terrain_offset,
        add_dormers,
        config.style_seed,
        steep_gable,
        preferred_ridge_along_x,
        scale_factor,
        rise_hint,
        trim,
        holes,
    );

    // Parapet, crown and plant on a flat roof. A roof another part stands on
    // is a floor and gets none of them, and a column has no roof to speak of.
    let is_part = element.tags.contains_key("building:part");
    let open_flat_roof = style.roof_type == RoofType::Flat
        && !covered_by_sibling_part
        && roof_area.len() > THIN_RING_MAX_CELLS;
    // Residential tower parts are fitted out as terraces.
    let terrace = open_flat_roof
        && !modeled_part_roof
        && kit.residential
        && should_generate_roof_terrace(element, config, style.roof_type);
    if open_flat_roof {
        let coping =
            rooftops::coping_for(config.wall_block, config.accent_block, config.style_seed);
        // People walk out onto a terrace, and onto a residential podium.
        let cap = if terrace || (suppress_flat_extras && kit.residential) {
            ParapetCap::Railing
        } else {
            ParapetCap::Coping(coping)
        };
        if config.has_parapet {
            generate_parapet(editor, element, config, roof_area, kit.facade, cap);
        } else if !modeled_part_roof {
            generate_flat_roof_edge_variation(editor, config, roof_area, kit.facade, coping);
        }

        // A stepped setback crown, unless a planned tower rises from this roof.
        let has_crown = suppress_flat_extras
            || (!is_part
                && kit.explicit_pass
                && generate_setback_crown(editor, config, roof_area, &kit.cover));
        if !has_crown
            && !modeled_part_roof
            && should_generate_rooftop_equipment(config, style.roof_type, category)
        {
            plant_flat_roof(
                editor,
                element,
                config,
                category,
                roof_area,
                kit,
                scale_factor,
                terrace,
            );
        }
    }

    // A pre-war European town block has its row of flues whatever the style
    // drew for houses; commercial-only and post-war blocks have none.
    let town_flues = region == BuildingRegion::Europe
        && town_block
        && pre_war
        && !covered_by_sibling_part
        && matches!(
            category,
            BuildingCategory::Residential | BuildingCategory::Historic | BuildingCategory::Hotel
        )
        && element_rng(config.style_seed ^ 0xC41E_0000_5EED_0004).random_bool(0.7);
    // chimneys only on pitched roofs, presets cannot force one onto flat
    if (style.has_chimney || town_flues)
        && matches!(
            style.roof_type,
            RoofType::Gabled
                | RoofType::Hipped
                | RoofType::Mansard
                | RoofType::Gambrel
                | RoofType::HalfHipped
        )
    {
        // A European town block has a flue per flat, so a row of stacks,
        // stuccoed like the walls or in brick; a house has one or two.
        let row = (region == BuildingRegion::Europe && town_block)
            .then(|| multiply_scale(9, scale_factor).max(4));
        let stucco = row.is_some()
            && !is_glass_wall(config.wall_block)
            && element_rng(config.style_seed ^ 0xC41E_0000_5EED_0002).random_bool(0.5);
        generate_chimney(
            editor,
            &surface,
            config.start_y_offset + config.building_height + 1,
            config.abs_terrain_offset,
            element.id,
            row,
            if stucco { config.wall_block } else { BRICK },
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
    if category == BuildingCategory::Hospital && kit.explicit_pass {
        generate_hospital_green_cross(editor, element, config);
    }
}

/// Compact bbox-indexed distance grid, 4 bytes per cell instead of a hash map.
pub(crate) struct RoofDistanceGrid {
    pub(crate) min_x: i32,
    pub(crate) min_z: i32,
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) dist: Vec<i32>,
}

impl RoofDistanceGrid {
    /// Distance to the nearest roof edge, -1 outside the roof.
    pub(crate) fn get(&self, x: i32, z: i32) -> i32 {
        let (lx, lz) = (x - self.min_x, z - self.min_z);
        if lx < 0 || lz < 0 || lx >= self.width || lz >= self.height {
            return -1;
        }
        self.dist[(lz * self.width + lx) as usize]
    }
}

/// Per-cell BFS distance to the nearest roof edge.
pub(crate) fn roof_edge_distances(roof_area: &[(i32, i32)]) -> RoofDistanceGrid {
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
    cover: &RoofMembrane,
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
            // An open X neighbour means the wall runs along Z, so its ordinate is z.
            // Corners have both open; either pick keeps one run's rhythm and breaks the other.
            let open_x = dist.get(x + 1, z) < tier.inset || dist.get(x - 1, z) < tier.inset;
            let open_z = dist.get(x, z + 1) < tier.inset || dist.get(x, z - 1) < tier.inset;
            if open_x || open_z {
                let col = ColumnFacade {
                    wall_u: if open_x { z } else { x },
                    // Both open is the corner cell, and the outline below
                    // turns its corners on the wall block; a crown that kept
                    // glass there would break the pier at the setback.
                    corner: open_x && open_z,
                    ..ColumnFacade::default()
                };
                // Same wall and window logic as the facade, so bands continue upward.
                let photo = config.is_photo_column(x, z);
                for h in 0..tier.height {
                    let block = determine_wall_block_at_position(
                        x,
                        current_base + h,
                        z,
                        config,
                        col,
                        photo,
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
            // Each tier's top is a roof, covered like the main one.
            editor.set_block_absolute(
                cover.block_at(x, z),
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
    cover: &RoofMembrane,
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
    let Some(top) = generate_inset_tiers(editor, config, roof_area, &dist, roof_rel, &tiers, cover)
    else {
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

/// Chimney stacks in a row on the ridge of a pitched roof: one, sometimes
/// two, on a house; one per `row_spacing` blocks of ridge (at most six) on a
/// town block, where every flat had its flue. Each stands one or two blocks
/// clear of the ridge under a slab or a pot. Stacks keep two cells off every
/// wall, so none is buried at a gable end or sunk at an eave. Heights come
/// from the roof's own `surface`, not the world, so every tile agrees.
fn generate_chimney(
    editor: &mut WorldEditor,
    surface: &RoofSurface,
    roof_base: i32,
    abs_terrain_offset: i32,
    element_id: u64,
    row_spacing: Option<i32>,
    shaft: Block,
) {
    if surface.len() < 9 {
        return;
    }
    // A dormer's cells are missing from the surface, so the margin keeps
    // the stacks off them too.
    let mut tops: Vec<((i32, i32), i32)> = surface
        .iter()
        .map(|(&c, &y)| (c, y))
        .filter(|&((x, z), _)| {
            (-2..=2).all(|d| surface.contains_key(&(x + d, z)) && surface.contains_key(&(x, z + d)))
        })
        .collect();
    tops.sort_unstable();
    let Some(ridge) = tops.iter().map(|t| t.1).max() else {
        return;
    };
    let band: Vec<(i32, i32)> = tops
        .iter()
        .filter(|t| t.1 >= ridge - 1)
        .map(|t| t.0)
        .collect();
    // The ridge line: the cross-ridge ordinate most crown cells share.
    let crown: Vec<(i32, i32)> = band
        .iter()
        .copied()
        .filter(|c| surface.get(c) == Some(&ridge))
        .collect();
    let crown = if crown.is_empty() { &band } else { &crown };
    let spread = |f: fn(&(i32, i32)) -> i32| {
        let (lo, hi) = crown
            .iter()
            .map(f)
            .fold((i32::MAX, i32::MIN), |(a, b), v| (a.min(v), b.max(v)));
        hi - lo
    };
    let along_x = spread(|c| c.0) >= spread(|c| c.1);
    let across = |&(x, z): &(i32, i32)| if along_x { z } else { x };
    let mut lines: Vec<i32> = crown.iter().map(across).collect();
    lines.sort_unstable();
    let line = *lines
        .iter()
        .max_by_key(|&&a| (lines.iter().filter(|&&b| b == a).count(), -a))
        .unwrap();
    let mut row: Vec<(i32, i32)> = band.into_iter().filter(|c| across(c) == line).collect();
    row.sort_unstable_by_key(|&(x, z)| if along_x { x } else { z });
    let n = row.len();

    let mut rng = element_rng(element_id ^ 0xC41E_0000_0000_0003);
    let picks: Vec<usize> = match row_spacing {
        // Evenly spaced along the ridge.
        Some(spacing) => {
            let count = (n as i32 / spacing.max(1)).clamp(1, 6) as usize;
            (0..count).map(|k| (2 * k + 1) * n / (2 * count)).collect()
        }
        None if n >= 12 && rng.random_bool(0.35) => vec![n / 5, n - 1 - n / 5],
        None => vec![rng.random_range(n / 5..=(n - 1 - n / 5).max(n / 5))],
    };

    // Blocks that the chimney is allowed to replace: empty blacklist = any.
    let replace_any: &[Block] = &[];
    let base = roof_base + abs_terrain_offset;
    for i in picks {
        let (x, z) = row[i.min(n - 1)];
        let top = ridge + abs_terrain_offset + rng.random_range(1..=2);
        for y in base + 1..=top {
            editor.set_block_absolute(shaft, x, y, z, None, Some(replace_any));
        }
        // Cap: some get an empty flower pot as a chimney pot, the rest a slab.
        let cap = if rng.random_bool(0.35) {
            EMPTY_FLOWER_POT
        } else {
            STONE_BRICK_SLAB
        };
        editor.set_block_absolute(cap, x, top + 1, z, None, Some(replace_any));
    }
}

// ============================================================================
// Roof Terrace Generation
// ============================================================================

/// Determines whether a building should get a roof terrace (the caller
/// keeps it to homes).
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

    // The rod stands on the ridge as built, whatever rise the roof got.
    let wall_top = config.start_y_offset + config.building_height + config.abs_terrain_offset;
    let Some(ridge_y) = editor.highest_block_between(best_x, best_z, wall_top, wall_top + 96)
    else {
        return;
    };
    editor.set_block_absolute(LIGHTNING_ROD, best_x, ridge_y + 1, best_z, None, None);
}

/// Whether a flat roof is fitted out with plant (see `rooftops::fit_out_roof`,
/// which scales it to the roof and the building). Every flat roof is, but a
/// barn's, a garage's, a shed's, a greenhouse's or a church's, or one too low
/// to be a storey.
fn should_generate_rooftop_equipment(
    config: &BuildingConfig,
    roof_type: RoofType,
    category: BuildingCategory,
) -> bool {
    roof_type == RoofType::Flat
        && config.building_height >= 4
        && !matches!(
            config.condition,
            BuildingCondition::Construction | BuildingCondition::Ruined
        )
        && !matches!(
            category,
            BuildingCategory::Farm
                | BuildingCategory::Garage
                | BuildingCategory::Shed
                | BuildingCategory::Greenhouse
                | BuildingCategory::Religious
        )
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
    /// Ceiling on an inferred rise in blocks. Without it the rise follows the
    /// wall height, and a tall block grows a roof the size of a house.
    max_rise: Option<i32>,
    /// What the roof borrows from the building under it.
    trim: RoofTrim,
}

/// What a pitched roof takes from the building beneath it and its
/// neighbours; the default leaves the roof to its own block.
#[derive(Clone, Debug, Default)]
struct RoofTrim {
    /// Wall block for gable triangles and dormer cheeks, None keeps the roof block.
    gable_block: Option<Block>,
    /// The building's window glass, for the dormers.
    window_block: Option<Block>,
    /// Window rhythm phase of the walls, so dormers stand on the window axes.
    window_phase: Option<i32>,
    /// Upside-down stairs under the eave overhang.
    soffit: bool,
    /// Copper hoods on the dormers of a historic roof.
    copper_dormers: bool,
    /// A flat top left by the rise cap is sheet metal, not tile.
    sheet_top: bool,
    /// Where the rise cap stops an inferred 1:1 slope, keep the pitch and
    /// flatten the top, as on a European town block, instead of dropping to
    /// a 1:2 slope.
    steep_cap: bool,
    /// One-cell eaves, as on a European town block, instead of two.
    short_eave: bool,
    /// Cells of neighbouring buildings no eave or trim may reach.
    blocked: FnvHashSet<(i32, i32)>,
    /// Small patches of a same-tone sibling covering on the slopes, as
    /// replaced tiles weather apart. Off where the roof is mapped.
    tone_patches: bool,
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

        // Only a fallback: a pitched roof comes with the covering its style
        // picked (`pitched_roof_block`), which generate_roof puts in here.
        let roof_block = roof_friendly_block(wall_block);

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
            max_rise: None,
            trim: RoofTrim::default(),
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

/// Drops of more than this many blocks to a lower neighbour are a face, not a
/// slope: a stair on top of such a column reads as a notch, a full block does
/// not. Gambrel bands drop two per cell and keep their stairs.
const MAX_STAIR_DROP: i32 = 2;

/// Whether the top block of a roof column faces a drop too steep for a stair.
#[inline]
fn steep_drop(x: i32, z: i32, roof_height: i32, roof_heights: &HashMap<(i32, i32), i32>) -> bool {
    [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)]
        .iter()
        .any(|(nx, nz)| {
            roof_heights
                .get(&(*nx, *nz))
                .is_some_and(|&nh| roof_height - nh > MAX_STAIR_DROP)
        })
}

/// Dormers along a pitched roof slope, each over a window axis of the wall
/// below: a glass face between two cheeks in the wall stone, jutting one
/// cell out over the eave, under a small gable of roof stairs (a copper hood
/// on a historic building). A mansard gets one on every axis of both
/// slopes; other roofs one on every other axis of one slope, which is how
/// the attics of real town houses read. Returns the cells the dormers took.
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
) -> Vec<(i32, i32)> {
    let mut built: Vec<(i32, i32)> = Vec::new();
    if !config.add_dormers {
        return built;
    }

    // second row at base+2/base+3, the mansard band comes from the caller
    let target_h = config.base_height + 2;
    let alt_target_h = config.base_height + 3;
    let (px, pz) = parallel_to_ridge;

    // (x, z, h, side): side is the eave the slope faces, -1 or +1 across the ridge.
    let mut candidates: Vec<(i32, i32, i32, i32)> = Vec::new();
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

        candidates.push((x, z, h, -inward));
    }

    if candidates.is_empty() {
        return built;
    }
    candidates.sort_unstable();

    let mut rng = element_rng(config.element_id_for_decor ^ 0x000D_04E4_DEC0);
    let every_axis = mansard_band.is_some();
    // The along-ridge ordinate is the one the wall below lays its bays on, so
    // a dormer centred on window column 1 stands over the window strip.
    let along = |x: i32, z: i32| if px != 0 { x } else { z };
    let on_axis = |u: i32| {
        config
            .trim
            .window_phase
            .is_none_or(|p| (u + p).rem_euclid(6) == 1)
    };
    let on_side = |side: i32| -> Vec<(i32, i32, i32, i32)> {
        candidates
            .iter()
            .copied()
            .filter(|c| c.3 == side && on_axis(along(c.0, c.1)))
            .collect()
    };
    let (minus, plus) = (on_side(-1), on_side(1));
    let sides: Vec<Vec<(i32, i32, i32, i32)>> = if every_axis {
        vec![minus, plus]
    } else if minus.len() != plus.len() {
        vec![if minus.len() > plus.len() {
            minus
        } else {
            plus
        }]
    } else if rng.random_bool(0.5) {
        vec![minus]
    } else {
        vec![plus]
    };

    let mut chosen: Vec<(i32, i32, i32)> = Vec::new();
    for cells in sides {
        if cells.is_empty() {
            continue;
        }
        // One row per slope: the band most axes reach.
        let mut rows: Vec<i32> = cells.iter().map(|c| c.2).collect();
        rows.sort_unstable();
        let row = *rows
            .iter()
            .max_by_key(|&&h| (rows.iter().filter(|&&r| r == h).count(), -h))
            .unwrap();
        let mut row_cells: Vec<(i32, i32, i32)> = cells
            .iter()
            .filter(|c| c.2 == row)
            .map(|c| (c.0, c.1, c.2))
            .collect();
        row_cells.sort_by_key(|c| along(c.0, c.1));
        let picked: Vec<(i32, i32, i32)> = if every_axis {
            row_cells
        } else {
            // Every other axis, on whichever parity seats more dormers.
            let parity = |c: &(i32, i32, i32)| {
                let u = along(c.0, c.1) + config.trim.window_phase.unwrap_or(0);
                u.div_euclid(6).rem_euclid(2)
            };
            let even = row_cells.iter().filter(|c| parity(c) == 0).count();
            let odd = row_cells.len() - even;
            let keep = if even != odd {
                i32::from(odd > even)
            } else {
                rng.random_range(0..2)
            };
            row_cells
                .into_iter()
                .filter(|c| parity(c) == keep)
                .collect()
        };
        // Bays 6 apart already space them; this guards a phase-less roof.
        for c in picked {
            let clear = chosen
                .iter()
                .all(|&(cx, cz, _)| ((cx - c.0) * px + (cz - c.1) * pz).abs() >= 4);
            if clear {
                chosen.push(c);
            }
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
    // A copper hood on a historic roof, else a small gable of the roof's stairs.
    let (stair_material, cap_centre_block) = if config.trim.copper_dormers {
        (WAXED_OXIDIZED_CUT_COPPER_STAIRS, WAXED_OXIDIZED_COPPER)
    } else {
        (
            get_stair_block_for_material(config.roof_block),
            config.roof_block,
        )
    };
    let glass = config.trim.window_block.unwrap_or(LIGHT_GRAY_STAINED_GLASS);
    // Cheeks in the wall stone: a dormer in the roof block reads as a box.
    let flank = config.trim.gable_block.unwrap_or(config.roof_block);

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

        let mut rows: Vec<(i32, i32)> = vec![(x, z)];
        if ext_in {
            rows.push((ext_x, ext_z));
        }
        for &(rx, rz) in &rows {
            built.extend([-1, 0, 1].map(|side| (rx + px * side, rz + pz * side)));
            // Lower base row: solid at the eave so the dormer reads as flush.
            for side in [-1, 0, 1] {
                editor.set_block_absolute(
                    flank,
                    rx + px * side,
                    lower_y,
                    rz + pz * side,
                    None,
                    Some(overwrite_anything),
                );
            }
            // Face row: cheek, glass, cheek, over the slope stair.
            editor.set_block_absolute(
                flank,
                rx - px,
                face_y,
                rz - pz,
                None,
                Some(overwrite_anything),
            );
            editor.set_block_absolute(glass, rx, face_y, rz, None, Some(overwrite_anything));
            editor.set_block_absolute(
                flank,
                rx + px,
                face_y,
                rz + pz,
                None,
                Some(overwrite_anything),
            );
            // Cap row: stair, full block, stair.
            editor.set_block_with_properties_absolute(
                create_stair_with_properties(stair_material, left_facing, StairShape::Straight),
                rx - px,
                cap_y,
                rz - pz,
                None,
                Some(overwrite_anything),
            );
            editor.set_block_absolute(
                cap_centre_block,
                rx,
                cap_y,
                rz,
                None,
                Some(overwrite_anything),
            );
            editor.set_block_with_properties_absolute(
                create_stair_with_properties(stair_material, right_facing, StairShape::Straight),
                rx + px,
                cap_y,
                rz + pz,
                None,
                Some(overwrite_anything),
            );
        }
    }
    built
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
                if has_lower && !steep_drop(x, z, roof_height, roof_heights) {
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

/// Generates a flat roof: the cover over the floor, while the outer ring
/// keeps the wall's own top course (its accent roof line) under the parapet.
fn generate_flat_roof(
    editor: &mut WorldEditor,
    roof_area: &[(i32, i32)],
    cover: &RoofMembrane,
    base_height: i32,
    abs_terrain_offset: i32,
) {
    let dist = roof_edge_distances(roof_area);
    let replace_any: &[Block] = &[];
    for &(x, z) in roof_area {
        let edge = dist.get(x, z);
        editor.set_block_absolute(
            cover.block_at_edge(x, z, edge),
            x,
            base_height + abs_terrain_offset,
            z,
            None,
            (edge > 0).then_some(replace_any),
        );
    }
}

/// Roof block a mapped roof:material, else roof:colour, asks for.
fn mapped_roof_block(tags: &HashMap<String, String>, style_seed: u64) -> Option<Block> {
    let mut roof_rng = element_rng(style_seed ^ 0xF00F_C010_BA5E_F00D);
    tagged_roof_block(tags, false, None, &mut roof_rng)
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
    /// Barrel vault: semi-elliptical section across the ridge.
    Round,
}

/// Feather sentinel for cells whose slope needs no scan clamp.
const NO_FEATHER: i32 = 10_000;

/// The four sides of a roof cell: west, east, north, south.
const ROOF_SIDES: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];

/// The four quarters of a roof cell: NW, NE, SW, SE (north is -Z, west -X).
const ROOF_QUARTERS: [(i32, i32); 4] = [(-1, -1), (1, -1), (-1, 1), (1, 1)];

/// Slab of a roof stair's own family, for 1:2 slopes and single-cell
/// ridges; None where no slab matches its texture closely enough.
fn roof_slab_for_stair(stair: Block) -> Option<Block> {
    Some(match stair {
        STONE_BRICK_STAIRS => STONE_BRICK_SLAB,
        COBBLESTONE_STAIRS => COBBLESTONE_SLAB,
        MOSSY_COBBLESTONE_STAIRS => MOSSY_COBBLESTONE_SLAB,
        MOSSY_STONE_BRICK_STAIRS => MOSSY_STONE_BRICK_SLAB,
        STONE_STAIRS => STONE_BLOCK_SLAB,
        ANDESITE_STAIRS => ANDESITE_SLAB,
        POLISHED_ANDESITE_STAIRS => POLISHED_ANDESITE_SLAB,
        POLISHED_DIORITE_STAIRS | DIORITE_STAIRS => POLISHED_DIORITE_SLAB,
        DEEPSLATE_BRICK_STAIRS => DEEPSLATE_TILE_SLAB,
        POLISHED_DEEPSLATE_STAIRS => POLISHED_DEEPSLATE_SLAB,
        COBBLED_DEEPSLATE_STAIRS => COBBLED_DEEPSLATE_SLAB,
        POLISHED_BLACKSTONE_BRICK_STAIRS => POLISHED_BLACKSTONE_SLAB,
        BLACKSTONE_STAIRS => BLACKSTONE_SLAB,
        BRICK_STAIRS => BRICK_SLAB,
        MUD_BRICK_STAIRS => MUD_BRICK_SLAB,
        END_STONE_BRICK_STAIRS => END_STONE_BRICK_SLAB,
        SMOOTH_SANDSTONE_STAIRS => CUT_SANDSTONE_SLAB,
        QUARTZ_STAIRS | SMOOTH_QUARTZ_STAIRS => SMOOTH_QUARTZ_SLAB,
        RED_NETHER_BRICK_STAIRS => RED_NETHER_BRICK_SLAB,
        WAXED_EXPOSED_CUT_COPPER_STAIRS => WAXED_EXPOSED_CUT_COPPER_SLAB,
        OAK_STAIRS => OAK_SLAB,
        SPRUCE_STAIRS => SPRUCE_SLAB,
        DARK_OAK_STAIRS => DARK_OAK_SLAB,
        JUNGLE_STAIRS => JUNGLE_SLAB,
        CHERRY_STAIRS => CHERRY_SLAB,
        CRIMSON_STAIRS => CRIMSON_SLAB,
        WARPED_STAIRS => WARPED_SLAB,
        BAMBOO_STAIRS => BAMBOO_SLAB,
        PURPUR_STAIRS => PURPUR_SLAB,
        DARK_PRISMARINE_STAIRS => DARK_PRISMARINE_SLAB,
        _ => return None,
    })
}

/// Stair facing and shape whose raised half covers exactly the given
/// quarters of a cell (see `ROOF_QUARTERS`): two adjacent ones are a
/// straight stair, one an outer corner (a hip), three an inner corner (a
/// valley). None for no quarter, all four, or two diagonal ones.
fn stair_for_high_quarters(high: [bool; 4]) -> Option<(StairFacing, StairShape)> {
    use StairFacing::{East, North, South, West};
    use StairShape::{InnerLeft, InnerRight, OuterLeft, OuterRight, Straight};
    match high {
        [true, true, false, false] => Some((North, Straight)),
        [false, false, true, true] => Some((South, Straight)),
        [true, false, true, false] => Some((West, Straight)),
        [false, true, false, true] => Some((East, Straight)),
        [true, false, false, false] => Some((North, OuterLeft)),
        [false, true, false, false] => Some((North, OuterRight)),
        [false, false, true, false] => Some((South, OuterRight)),
        [false, false, false, true] => Some((South, OuterLeft)),
        [true, true, true, false] => Some((North, InnerLeft)),
        [true, true, false, true] => Some((North, InnerRight)),
        [true, false, true, true] => Some((South, InnerRight)),
        [false, true, true, true] => Some((South, InnerLeft)),
        _ => None,
    }
}

/// Quarters an eave stair raises toward the roof it hangs off, for the
/// sides (bits of `ROOF_SIDES`) it hangs off: both halves in a valley.
fn eave_quarters(sides: u8) -> [bool; 4] {
    ROOF_QUARTERS.map(|(sx, sz)| {
        ROOF_SIDES.iter().enumerate().any(|(i, &(dx, dz))| {
            sides & (1 << i) != 0 && ((dx != 0 && sx == -dx) || (dz != 0 && sz == -dz))
        })
    })
}

/// Top block height of a pitched roof per cell, relative to the terrain
/// offset, as the generators report it for the chimneys. Dormer cells are
/// left out.
type RoofSurface = HashMap<(i32, i32), i32>;

/// Surface levels of a pitched roof per cell, in half blocks: `2y + 1` for
/// a stair or block topping out at `y`, `2y` for a bottom slab at `y`.
/// Equal levels are flush, one apart a slab step, two apart a stair step.
type RoofLevels = HashMap<(i32, i32), i32>;

/// How a slope is built.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum PitchMode {
    /// Stairs, 1:1 or steeper.
    Stairs,
    /// Bottom and top slabs alternating, 1:2: a low pitch without the
    /// terraced look of stairs between level treads.
    Slabs,
}

/// What tops a roof column, so the rake trim can repeat it past a gable.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum RoofCap {
    Full,
    Stair(StairFacing, StairShape),
    Slab { top: bool },
}

/// Cells inside the flat tops the rise cap leaves on a roof: every
/// neighbour at the same level, in a patch big enough to read as a deck.
/// A slope never has a 3 x 3 flat, so only the capped plateaus qualify; a
/// patch of a few cells would read as a skylight and stays tile.
fn capped_flat_tops(levels: &RoofLevels) -> HashSet<(i32, i32)> {
    const MIN_DECK: usize = 30;
    let flat: HashSet<(i32, i32)> = levels
        .iter()
        .filter(|&(&(x, z), &l)| {
            ROOF_QUARTERS
                .iter()
                .chain(ROOF_SIDES.iter())
                .all(|&(dx, dz)| levels.get(&(x + dx, z + dz)) == Some(&l))
        })
        .map(|(&c, _)| c)
        .collect();
    let mut seen: HashSet<(i32, i32)> = HashSet::with_capacity(flat.len());
    let mut decks: HashSet<(i32, i32)> = HashSet::new();
    let mut starts: Vec<(i32, i32)> = flat.iter().copied().collect();
    starts.sort_unstable();
    for start in starts {
        if !seen.insert(start) {
            continue;
        }
        let mut patch = vec![start];
        let mut i = 0;
        while i < patch.len() {
            let (x, z) = patch[i];
            i += 1;
            for &(dx, dz) in &ROOF_SIDES {
                let n = (x + dx, z + dz);
                if flat.contains(&n) && seen.insert(n) {
                    patch.push(n);
                }
            }
        }
        if patch.len() >= MIN_DECK {
            decks.extend(patch);
        }
    }
    decks
}

/// A covering of nearly the same tone as a roof's, in the same shapes: its
/// full block, stair and slab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RoofTone {
    full: Block,
    stairs: Block,
    slab: Option<Block>,
}

/// The same-tone sibling of a pitched covering, for the odd patch of
/// replaced tiles. Only coverings whose sibling has a matching stair, and a
/// slab wherever the covering has one; sheet metal is laid in whole runs.
fn roof_tone_sibling(roof: Block) -> Option<RoofTone> {
    let tone = match roof {
        // Brick-stair clay tile next to the smooth pink of polished granite.
        RED_TERRACOTTA | BRICK => RoofTone {
            full: roof,
            stairs: POLISHED_GRANITE_STAIRS,
            slab: Some(POLISHED_GRANITE_SLAB),
        },
        TERRACOTTA => RoofTone {
            full: TERRACOTTA,
            stairs: POLISHED_GRANITE_STAIRS,
            slab: Some(POLISHED_GRANITE_SLAB),
        },
        COBBLED_DEEPSLATE => RoofTone {
            full: POLISHED_DEEPSLATE,
            stairs: POLISHED_DEEPSLATE_STAIRS,
            slab: Some(POLISHED_DEEPSLATE_SLAB),
        },
        DEEPSLATE_TILES => RoofTone {
            full: DEEPSLATE_BRICKS,
            stairs: DEEPSLATE_BRICK_STAIRS,
            slab: Some(DEEPSLATE_TILE_SLAB),
        },
        POLISHED_BLACKSTONE_BRICKS => RoofTone {
            full: BLACKSTONE,
            stairs: BLACKSTONE_STAIRS,
            slab: Some(BLACKSTONE_SLAB),
        },
        _ => return None,
    };
    let own_slab = roof_slab_for_stair(get_stair_block_for_material(roof));
    (own_slab.is_none() || tone.slab.is_some()).then_some(tone)
}

/// Share of roof patches, in percent, laid in the sibling covering.
const ROOF_TONE_PERCENT: u32 = 8;

/// Whether roof cell `(x, z)` lies in a patch of the sibling covering:
/// two by two cells, every other row of patches offset by a cell so they do
/// not line up.
fn roof_tone_patch(x: i32, z: i32, seed: u64) -> bool {
    let row = z.div_euclid(2);
    let shift = row.rem_euclid(2);
    coord_rng((x + shift).div_euclid(2), row, seed ^ 0x7111_E5A1_0000_0005).random_range(0..100u32)
        < ROOF_TONE_PERCENT
}

/// Builds a pitched roof from its levels: each column is filled from the
/// wall top up and capped by what its neighbours call for. A slope takes a
/// straight stair, a hip an outer corner, a valley an inner corner, a ridge
/// one cell wide a slab, a 1:2 slope alternating slabs. `outside` gives the
/// level a cell off the footprint continues at (past a gable end), None
/// where the roof drops to an eave. Gable-end columns are filled with the
/// wall block, so the triangle under the rake reads as wall. A flat top left
/// by the rise cap is sheet metal, as on a real deep block, not tile.
/// Returns each column's top block height and cap.
fn place_roof_surface(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    levels: &RoofLevels,
    config: &RoofConfig,
    mode: PitchMode,
    outside: &dyn Fn(i32, i32) -> Option<i32>,
    overwrite_inner: bool,
) -> HashMap<(i32, i32), (i32, RoofCap)> {
    let stair_mat = get_stair_block_for_material(config.roof_block);
    let slab = roof_slab_for_stair(stair_mat);
    let level_at = |x: i32, z: i32| levels.get(&(x, z)).copied().or_else(|| outside(x, z));
    let sheet_cells = if config.trim.sheet_top {
        capped_flat_tops(levels)
    } else {
        HashSet::new()
    };
    let abs = config.abs_terrain_offset;
    let replace_any: &[Block] = &[];
    let gable_fill = config.trim.gable_block.unwrap_or(config.roof_block);
    let sibling = config
        .trim
        .tone_patches
        .then(|| roof_tone_sibling(config.roof_block))
        .flatten();
    let mut tops = HashMap::with_capacity(floor_area.len());
    for &(x, z) in floor_area {
        let l = levels[&(x, z)];
        let y = l.div_euclid(2);
        let mut sheet = false;
        let cap = if l.rem_euclid(2) == 0 {
            RoofCap::Slab { top: false }
        } else {
            // A 1:2 slope steps half a block per cell; only a whole-block
            // drop there calls for a stair.
            let floor = if mode == PitchMode::Slabs { l - 1 } else { l };
            let not_lower = |dx: i32, dz: i32| level_at(x + dx, z + dz).is_some_and(|n| n >= floor);
            let high = ROOF_QUARTERS
                .map(|(dx, dz)| not_lower(dx, 0) && not_lower(0, dz) && not_lower(dx, dz));
            let steep = ROOF_SIDES.iter().any(|&(dx, dz)| {
                levels
                    .get(&(x + dx, z + dz))
                    .is_some_and(|&n| l - n > 2 * MAX_STAIR_DROP)
            });
            if steep {
                RoofCap::Full
            } else if high.iter().all(|&h| h) {
                let stepped = ROOF_SIDES
                    .iter()
                    .any(|&(dx, dz)| level_at(x + dx, z + dz).is_some_and(|n| n < l));
                sheet = sheet_cells.contains(&(x, z));
                if mode == PitchMode::Slabs && stepped {
                    RoofCap::Slab { top: true }
                } else {
                    RoofCap::Full
                }
            } else if let Some((facing, shape)) = stair_for_high_quarters(high) {
                RoofCap::Stair(facing, shape)
            } else if high.iter().all(|&h| !h) {
                RoofCap::Slab { top: false }
            } else {
                RoofCap::Full
            }
        };
        let cap = match (cap, slab) {
            (RoofCap::Slab { .. }, None) => RoofCap::Full,
            _ => cap,
        };
        // A slab cap only stands where the covering has a slab, and the
        // sibling has one wherever it does.
        let cover = sibling
            .filter(|_| roof_tone_patch(x, z, config.element_id_for_decor))
            .unwrap_or(RoofTone {
                full: config.roof_block,
                stairs: stair_mat,
                slab,
            });
        let block = match cap {
            RoofCap::Full if sheet => {
                // Low-frequency patches, as a patched membrane weathers.
                let patch =
                    coord_rng(x.div_euclid(3), z.div_euclid(3), 0x5EE7_0F7A).random_bool(0.3);
                BlockWithProperties::simple(if patch {
                    LIGHT_GRAY_CONCRETE
                } else {
                    SMOOTH_STONE
                })
            }
            RoofCap::Full => BlockWithProperties::simple(cover.full),
            RoofCap::Stair(facing, shape) => {
                create_stair_with_properties(cover.stairs, facing, shape)
            }
            RoofCap::Slab { top } => {
                let s = cover.slab.unwrap_or(cover.full);
                if top {
                    make_top_slab(s)
                } else {
                    BlockWithProperties::simple(s)
                }
            }
        };
        let off = |dx: i32, dz: i32| !levels.contains_key(&(x + dx, z + dz));
        let edge = ROOF_SIDES.iter().any(|&(dx, dz)| off(dx, dz));
        let gable = ROOF_SIDES
            .iter()
            .any(|&(dx, dz)| off(dx, dz) && outside(x + dx, z + dz).is_some());
        let fill = if gable { gable_fill } else { config.roof_block };
        let rule = (overwrite_inner || edge).then_some(replace_any);
        for yy in config.base_height..y {
            editor.set_block_absolute(fill, x, yy + abs, z, None, rule);
        }
        editor.set_block_with_properties_absolute(block, x, y + abs, z, None, rule);
        tops.insert((x, z), (y, cap));
    }
    tops
}

/// Rake trim: the top course carried one cell past every gable end, so the
/// eave and the verge meet at the corners instead of leaving a notch. Cells
/// of a neighbouring building get none. Returns each trim cell with the
/// direction it continues the roof in.
fn place_rake_trim(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    tops: &HashMap<(i32, i32), (i32, RoofCap)>,
    config: &RoofConfig,
    outside: &dyn Fn(i32, i32) -> Option<i32>,
) -> Vec<((i32, i32), (i32, i32))> {
    let stair_mat = get_stair_block_for_material(config.roof_block);
    let slab = roof_slab_for_stair(stair_mat);
    let abs = config.abs_terrain_offset;
    let mut trims: Vec<((i32, i32), (i32, i32))> = Vec::new();
    let mut seen: HashSet<(i32, i32)> = HashSet::new();
    for &(x, z) in floor_area {
        let (y, cap) = tops[&(x, z)];
        let block = match (cap, slab) {
            (RoofCap::Stair(facing, _), _) => {
                create_stair_with_properties(stair_mat, facing, StairShape::Straight)
            }
            (RoofCap::Slab { top: true }, Some(s)) => make_top_slab(s),
            (RoofCap::Slab { top: false }, Some(s)) => BlockWithProperties::simple(s),
            _ => continue,
        };
        for &(dx, dz) in &ROOF_SIDES {
            let o = (x + dx, z + dz);
            if tops.contains_key(&o)
                || config.trim.blocked.contains(&o)
                || outside(o.0, o.1).is_none()
                || !seen.insert(o)
            {
                continue;
            }
            editor.set_block_with_properties_absolute(
                block.clone(),
                o.0,
                y + abs,
                o.1,
                Some(&[AIR]),
                None,
            );
            trims.push((o, (dx, dz)));
        }
    }
    trims
}

/// Eave overhang two cells deep (one with `short_eave`), stepping down with
/// the slope, off every side of a roof cell that drops to an eave (`seeds`:
/// the cell and a bit per `ROOF_SIDES` entry), with outer corners where two
/// such sides meet and inner corners where two eaves meet in a valley. Cells in `covered` (the
/// roof and its trim) or of a neighbouring building get none, so a party
/// wall stays flush. A soffit tucks an upside-down stair under the inner
/// row, against the wall.
fn place_roof_eaves(
    editor: &mut WorldEditor,
    seeds: &[((i32, i32), u8)],
    covered: &HashSet<(i32, i32)>,
    config: &RoofConfig,
    mode: PitchMode,
) {
    let stair_mat = get_stair_block_for_material(config.roof_block);
    let slab = roof_slab_for_stair(stair_mat);
    let mode = if slab.is_none() {
        PitchMode::Stairs
    } else {
        mode
    };
    let abs = config.abs_terrain_offset;
    let open = |c: &(i32, i32)| !covered.contains(c) && !config.trim.blocked.contains(c);

    // Ring cells keep the sides they hang off: two in a valley.
    let mut inner: HashMap<(i32, i32), u8> = HashMap::new();
    let mut outer: HashMap<(i32, i32), u8> = HashMap::new();
    let mut corner_inner: HashMap<(i32, i32), usize> = HashMap::new();
    let mut corner_outer: HashMap<(i32, i32), usize> = HashMap::new();
    let mut arms: HashMap<(i32, i32), u8> = HashMap::new();
    for &((x, z), sides) in seeds {
        for (i, &(dx, dz)) in ROOF_SIDES.iter().enumerate() {
            if sides & (1 << i) == 0 {
                continue;
            }
            let n1 = (x + dx, z + dz);
            if !open(&n1) {
                continue;
            }
            *inner.entry(n1).or_default() |= 1 << i;
            let n2 = (x + 2 * dx, z + 2 * dz);
            if open(&n2) && !config.trim.short_eave {
                *outer.entry(n2).or_default() |= 1 << i;
            }
        }
        // Convex corners where two dropping sides meet; q indexes ROOF_QUARTERS.
        for (q, (ix, iz)) in [(0usize, 2usize), (1, 2), (0, 3), (1, 3)]
            .into_iter()
            .enumerate()
        {
            if sides & (1 << ix) == 0 || sides & (1 << iz) == 0 {
                continue;
            }
            let (dx, dz) = ROOF_QUARTERS[q];
            if !open(&(x + dx, z)) || !open(&(x, z + dz)) || !open(&(x + dx, z + dz)) {
                continue;
            }
            corner_inner.entry((x + dx, z + dz)).or_insert(q);
            if config.trim.short_eave {
                continue;
            }
            if open(&(x + 2 * dx, z + 2 * dz)) {
                corner_outer.entry((x + 2 * dx, z + 2 * dz)).or_insert(q);
            }
            for (arm, i) in [((x + 2 * dx, z + dz), ix), ((x + dx, z + 2 * dz), iz)] {
                if open(&arm) {
                    *arms.entry(arm).or_default() |= 1 << i;
                }
            }
        }
    }

    let side_stair = |sides: u8| -> BlockWithProperties {
        let (facing, shape) = stair_for_high_quarters(eave_quarters(sides)).unwrap_or_else(|| {
            let first = (0..4).find(|i| sides & (1 << i) != 0).unwrap_or(0);
            let one = eave_quarters(1 << first);
            stair_for_high_quarters(one).unwrap_or((StairFacing::North, StairShape::Straight))
        });
        create_stair_with_properties(stair_mat, facing, shape)
    };
    // An eave corner raises the one quarter facing back to the roof.
    let corner_stair = |q: usize| -> BlockWithProperties {
        let mut high = [false; 4];
        high[3 - q] = true;
        let (facing, shape) =
            stair_for_high_quarters(high).unwrap_or((StairFacing::North, StairShape::Straight));
        create_stair_with_properties(stair_mat, facing, shape)
    };
    let (y_inner, y_outer) = match mode {
        PitchMode::Stairs => (config.base_height + abs, config.base_height - 1 + abs),
        PitchMode::Slabs => (config.base_height + abs, config.base_height + abs),
    };
    let ring_block = |stair: BlockWithProperties, upper: bool| -> BlockWithProperties {
        match (mode, slab) {
            (PitchMode::Slabs, Some(s)) if upper => make_top_slab(s),
            (PitchMode::Slabs, Some(s)) => BlockWithProperties::simple(s),
            _ => stair,
        }
    };

    for (cell, &sides) in &inner {
        let stair = side_stair(sides);
        if config.trim.soffit && mode == PitchMode::Stairs {
            editor.set_block_with_properties_absolute(
                top_stair(stair.clone()),
                cell.0,
                y_inner - 1,
                cell.1,
                None,
                None,
            );
        }
        editor.set_block_with_properties_absolute(
            ring_block(stair, true),
            cell.0,
            y_inner,
            cell.1,
            None,
            None,
        );
    }
    for (cell, &q) in &corner_inner {
        if inner.contains_key(cell) {
            continue;
        }
        editor.set_block_with_properties_absolute(
            ring_block(corner_stair(q), true),
            cell.0,
            y_inner,
            cell.1,
            None,
            None,
        );
    }
    // The outer ring skips cells already claimed by the inner or corner ring.
    for (cell, &sides) in &outer {
        if inner.contains_key(cell) || corner_inner.contains_key(cell) {
            continue;
        }
        editor.set_block_with_properties_absolute(
            ring_block(side_stair(sides), false),
            cell.0,
            y_outer,
            cell.1,
            None,
            None,
        );
    }
    for (cell, &q) in &corner_outer {
        if inner.contains_key(cell) || outer.contains_key(cell) || corner_inner.contains_key(cell) {
            continue;
        }
        editor.set_block_with_properties_absolute(
            ring_block(corner_stair(q), false),
            cell.0,
            y_outer,
            cell.1,
            None,
            None,
        );
    }
    for (cell, &sides) in &arms {
        if inner.contains_key(cell)
            || outer.contains_key(cell)
            || corner_inner.contains_key(cell)
            || corner_outer.contains_key(cell)
        {
            continue;
        }
        editor.set_block_with_properties_absolute(
            ring_block(side_stair(sides), false),
            cell.0,
            y_outer,
            cell.1,
            None,
            None,
        );
    }
}

/// Eave seeds for a roof that drops on every open side (hipped family).
fn all_side_eave_seeds(
    floor_area: &[(i32, i32)],
    footprint: &HashSet<(i32, i32)>,
) -> Vec<((i32, i32), u8)> {
    floor_area
        .iter()
        .filter_map(|&(x, z)| {
            let sides = ROOF_SIDES
                .iter()
                .enumerate()
                .filter(|(_, &(dx, dz))| !footprint.contains(&(x + dx, z + dz)))
                .fold(0u8, |m, (i, _)| m | (1 << i));
            (sides != 0).then_some(((x, z), sides))
        })
        .collect()
}

/// Where a cell sits on a two-slope roof, in whole cells.
#[derive(Clone, Copy, Debug)]
struct SlopeSample {
    /// Distance to the nearer eave, across the ridge.
    dist_to_edge: i32,
    /// The narrowest local half-span, which caps a narrow wing.
    local_half: i32,
    /// Farthest a cell gets from the eaves across the ridge here: the
    /// half-span, one less on an even span whose middle falls between two
    /// cells.
    perp_reach: i32,
    /// Distances to the two ends of the ridge.
    dm_along: i32,
    dp_along: i32,
    /// Shortest grid scan across the ridge, feathering a snapped tent.
    scan_perp_min: i32,
}

/// A cell's slope sample on each wing covering it, by wing index.
type WingSamples = Vec<(usize, SlopeSample)>;

/// One rectangular wing of a footprint, in its `WingFrame` (`u` along the
/// frame's axis, `v` across it), roofed along its own ridge.
#[derive(Clone, Copy, Debug)]
struct RoofWing {
    u: (f64, f64),
    v: (f64, f64),
    /// The ridge runs along `u`, else along `v`.
    along_u: bool,
    /// Ridge ends (low, high) cut back where the wing runs into a larger
    /// wing's roof: they meet its slope in valleys instead of a gable.
    clipped: (bool, bool),
}

impl RoofWing {
    fn along(&self) -> (f64, f64) {
        if self.along_u {
            self.u
        } else {
            self.v
        }
    }

    fn across(&self) -> (f64, f64) {
        if self.along_u {
            self.v
        } else {
            self.u
        }
    }

    fn length(&self) -> f64 {
        self.along().1 - self.along().0
    }

    fn area(&self) -> f64 {
        (self.u.1 - self.u.0) * (self.v.1 - self.v.0)
    }

    /// Half-span in whole cells, the farthest a cell gets from its eaves.
    fn reach(&self) -> i32 {
        let (b0, b1) = self.across();
        ((b1 - b0) / 2.0 + 1e-6).floor() as i32
    }

    /// How far a frame point lies off the wing, zero inside it.
    fn gap(&self, u: f64, v: f64) -> f64 {
        let du = (self.u.0 - u).max(u - self.u.1).max(0.0);
        let dv = (self.v.0 - v).max(v - self.v.1).max(0.0);
        du.max(dv)
    }

    /// Slope sample at frame point `(u, v)`, or None off the wing. A cell of
    /// the footprint gets a cell of slack for the rasterized outline; a cell
    /// off it counts only past an open (gable) end, where the roof carries
    /// on into the rake, and never across an eave.
    fn slope(&self, u: f64, v: f64, on_footprint: bool) -> Option<SlopeSample> {
        const EPS: f64 = 1e-6;
        let (a, b) = if self.along_u { (u, v) } else { (v, u) };
        let (a0, a1) = self.along();
        let (b0, b1) = self.across();
        let slack = if on_footprint { 1.0 } else { 1.5 };
        let lo = if self.clipped.0 { a0 - EPS } else { a0 - slack };
        let hi = if self.clipped.1 { a1 + EPS } else { a1 + slack };
        let e = (b - b0).min(b1 - b);
        if a < lo || a > hi || e < if on_footprint { -1.0 } else { -EPS } {
            return None;
        }
        let whole = |d: f64| (d + EPS).floor().max(0.0) as i32;
        let reach = self.reach();
        Some(SlopeSample {
            dist_to_edge: whole(e),
            local_half: reach,
            perp_reach: reach,
            dm_along: if self.clipped.0 {
                NO_FEATHER
            } else {
                whole(a - a0)
            },
            dp_along: if self.clipped.1 {
                NO_FEATHER
            } else {
                whole(a1 - a)
            },
            scan_perp_min: NO_FEATHER,
        })
    }
}

/// A footprint split into wings, in a frame turned `angle` off the grid.
#[derive(Clone, Debug)]
struct WingFrame {
    angle: f64,
    wings: Vec<RoofWing>,
}

impl WingFrame {
    fn uv(&self, x: i32, z: i32) -> (f64, f64) {
        let (c, sn) = (self.angle.cos(), self.angle.sin());
        let (x, z) = (x as f64, z as f64);
        (x * c + z * sn, -x * sn + z * c)
    }

    /// The grid step nearest a wing's ridge; the frame turns at most 45°.
    fn ridge_step(wing: &RoofWing) -> (i32, i32) {
        if wing.along_u {
            (1, 0)
        } else {
            (0, 1)
        }
    }

    /// Index of the largest wing, the main body of the building.
    fn main_wing(&self) -> usize {
        (0..self.wings.len())
            .max_by(|&a, &b| self.wings[a].area().total_cmp(&self.wings[b].area()))
            .unwrap_or(0)
    }
}

/// Wall lines along one axis merged into the cuts of a coarse grid: lines
/// within two blocks of each other are one wall with a jog. The outermost
/// cuts keep the extreme line, so the grid spans the whole outline. None
/// for fewer than two cuts or more than a dozen.
fn merge_wall_lines(mut lines: Vec<(f64, f64)>) -> Option<Vec<f64>> {
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut groups: Vec<Vec<(f64, f64)>> = Vec::new();
    for line in lines {
        match groups.last_mut() {
            Some(g) if line.0 - g[g.len() - 1].0 <= 2.0 && line.0 - g[0].0 <= 3.0 => g.push(line),
            _ => groups.push(vec![line]),
        }
    }
    let n = groups.len();
    if !(2..=12).contains(&n) {
        return None;
    }
    Some(
        groups
            .iter()
            .enumerate()
            .map(|(i, g)| {
                if i == 0 {
                    g.iter().map(|l| l.0).fold(f64::MAX, f64::min)
                } else if i == n - 1 {
                    g.iter().map(|l| l.0).fold(f64::MIN, f64::max)
                } else {
                    let w: f64 = g.iter().map(|l| l.1).sum();
                    g.iter().map(|l| l.0 * l.1).sum::<f64>() / w.max(1e-9)
                }
            })
            .collect(),
    )
}

/// Splits an L, T, U, courtyard or other right-angled footprint into
/// overlapping rectangular wings, each roofed along its own length, so a
/// side wing gets its own ridge instead of a flat-topped stretch of the main
/// one. A wing that runs into a longer one is cut back at that wing's ridge
/// and meets its slope in valleys. `holes` are the courtyards. None for a
/// plain rectangle, a footprint with long oblique walls, or one too
/// intricate to split; the caller keeps its single-ridge scan then.
fn roof_wings(
    nodes: &[ProcessedNode],
    holes: &[&[ProcessedNode]],
    cells: usize,
    main_along_u: Option<bool>,
) -> Option<WingFrame> {
    let ring = |ns: &[ProcessedNode]| -> Vec<(f64, f64)> {
        let mut pts: Vec<(f64, f64)> = ns.iter().map(|n| (n.x as f64, n.z as f64)).collect();
        if pts.len() > 1 && pts.first() == pts.last() {
            pts.pop();
        }
        pts
    };
    let outer = ring(nodes);
    if outer.len() < 4 || (outer.len() < 6 && holes.is_empty()) {
        return None;
    }
    let mut angle = dominant_axis_angle(nodes);
    if angle.to_degrees().abs() < 1.0 {
        angle = 0.0;
    }
    let (c, s) = (angle.cos(), angle.sin());
    let to_uv = |pts: Vec<(f64, f64)>| -> Vec<(f64, f64)> {
        pts.into_iter()
            .map(|(x, z)| (x * c + z * s, -x * s + z * c))
            .collect()
    };
    let rings: Vec<Vec<(f64, f64)>> = std::iter::once(to_uv(outer))
        .chain(holes.iter().map(|h| to_uv(ring(h))))
        .filter(|r| r.len() >= 3)
        .collect();
    let shoelace = |r: &[(f64, f64)]| -> f64 {
        (0..r.len())
            .map(|i| {
                let (a, b) = (r[i], r[(i + 1) % r.len()]);
                a.0 * b.1 - b.0 * a.1
            })
            .sum::<f64>()
            .abs()
            / 2.0
    };
    let area = shoelace(&rings[0]) - rings[1..].iter().map(|r| shoelace(r)).sum::<f64>();
    // Cells well short of the outline's area mean a courtyard nobody mapped.
    if area <= 0.0 || (cells as f64) < 0.9 * area {
        return None;
    }

    let skew = 15f64.to_radians().tan();
    let (mut u_lines, mut v_lines) = (Vec::new(), Vec::new());
    let (mut perimeter, mut oblique) = (0.0, 0.0);
    for r in &rings {
        for i in 0..r.len() {
            let (a, b) = (r[i], r[(i + 1) % r.len()]);
            let (du, dv) = (b.0 - a.0, b.1 - a.1);
            let len = du.hypot(dv);
            if len < 0.5 {
                continue;
            }
            perimeter += len;
            if dv.abs() <= du.abs() * skew {
                v_lines.push(((a.1 + b.1) / 2.0, len));
            } else if du.abs() <= dv.abs() * skew {
                u_lines.push(((a.0 + b.0) / 2.0, len));
            } else {
                oblique += len;
            }
        }
    }
    if perimeter <= 0.0 || oblique > 0.15 * perimeter {
        return None;
    }
    let us = merge_wall_lines(u_lines)?;
    let vs = merge_wall_lines(v_lines)?;
    let (nu, nv) = (us.len() - 1, vs.len() - 1);
    let occupied: Vec<bool> = (0..nv)
        .flat_map(|j| (0..nu).map(move |i| (i, j)))
        .map(|(i, j)| {
            let (mu, mv) = ((us[i] + us[i + 1]) / 2.0, (vs[j] + vs[j + 1]) / 2.0);
            polygon_contains(&rings[0], mu, mv)
                && !rings[1..].iter().any(|h| polygon_contains(h, mu, mv))
        })
        .collect();
    let occ = |i: usize, j: usize| occupied[j * nu + i];
    // Prefix sums make "is this block of coarse cells all roof" O(1).
    let mut pre = vec![0i32; (nu + 1) * (nv + 1)];
    for j in 0..nv {
        for i in 0..nu {
            pre[(j + 1) * (nu + 1) + i + 1] =
                i32::from(occ(i, j)) + pre[j * (nu + 1) + i + 1] + pre[(j + 1) * (nu + 1) + i]
                    - pre[j * (nu + 1) + i];
        }
    }
    let full = |i0: usize, i1: usize, j0: usize, j1: usize| -> bool {
        let sum = pre[(j1 + 1) * (nu + 1) + i1 + 1]
            - pre[j0 * (nu + 1) + i1 + 1]
            - pre[(j1 + 1) * (nu + 1) + i0]
            + pre[j0 * (nu + 1) + i0];
        sum == ((i1 - i0 + 1) * (j1 - j0 + 1)) as i32
    };
    if full(0, nu - 1, 0, nv - 1) {
        return None;
    }

    // Maximal rectangles of the coarse grid.
    let mut rects: Vec<(usize, usize, usize, usize)> = Vec::new();
    for i0 in 0..nu {
        for i1 in i0..nu {
            for j0 in 0..nv {
                for j1 in j0..nv {
                    if !full(i0, i1, j0, j1) {
                        break;
                    }
                    let grows = (i0 > 0 && full(i0 - 1, i1, j0, j1))
                        || (i1 + 1 < nu && full(i0, i1 + 1, j0, j1))
                        || (j0 > 0 && full(i0, i1, j0 - 1, j1))
                        || (j1 + 1 < nv && full(i0, i1, j0, j1 + 1));
                    if !grows {
                        rects.push((i0, i1, j0, j1));
                    }
                }
            }
        }
    }
    // Greedy cover, the rectangle adding the most uncovered roof first.
    let cell_area = |i: usize, j: usize| (us[i + 1] - us[i]) * (vs[j + 1] - vs[j]);
    let mut covered = vec![false; nu * nv];
    let mut chosen: Vec<(usize, usize, usize, usize)> = Vec::new();
    loop {
        let gain = |r: &(usize, usize, usize, usize)| -> f64 {
            let mut g = 0.0;
            for j in r.2..=r.3 {
                for i in r.0..=r.1 {
                    if !covered[j * nu + i] {
                        g += cell_area(i, j);
                    }
                }
            }
            g
        };
        let Some(best) = rects
            .iter()
            .filter(|r| !chosen.contains(r))
            .max_by(|a, b| gain(a).total_cmp(&gain(b)))
            .copied()
        else {
            break;
        };
        if gain(&best) <= 0.0 {
            break;
        }
        chosen.push(best);
        if chosen.len() > 8 {
            return None;
        }
        for j in best.2..=best.3 {
            for i in best.0..=best.1 {
                covered[j * nu + i] = true;
            }
        }
    }
    if (0..nu * nv).any(|k| occupied[k] && !covered[k]) {
        return None;
    }
    // Drop a rectangle the others already cover.
    let mut k = chosen.len();
    while k > 0 {
        k -= 1;
        let r = chosen[k];
        let redundant = (r.2..=r.3).all(|j| {
            (r.0..=r.1).all(|i| {
                chosen
                    .iter()
                    .enumerate()
                    .any(|(m, o)| m != k && (o.0..=o.1).contains(&i) && (o.2..=o.3).contains(&j))
            })
        });
        if redundant {
            chosen.remove(k);
        }
    }
    if chosen.len() < 2 {
        return None;
    }

    let mut wings: Vec<RoofWing> = chosen
        .iter()
        .map(|r| {
            let (u, v) = ((us[r.0], us[r.1 + 1]), (vs[r.2], vs[r.3 + 1]));
            RoofWing {
                u,
                v,
                along_u: u.1 - u.0 >= v.1 - v.0,
                clipped: (false, false),
            }
        })
        .collect();
    let frame_main = WingFrame {
        angle,
        wings: wings.clone(),
    }
    .main_wing();
    if let Some(along_u) = main_along_u {
        wings[frame_main].along_u = along_u;
    }
    // Wings that all run one way are a bar with jogs in its walls, which
    // reads best under one continuous ridge.
    if wings.iter().all(|w| w.along_u == wings[0].along_u) {
        return None;
    }

    // A wing whose end runs into a perpendicular wing at least as long, and
    // past that wing's ridge, is cut back to the ridge: an arm dies into the
    // main roof in two valleys instead of pushing a gable out of its far side.
    let snapshot = wings.clone();
    let dominates = |s: usize, r: usize| -> bool {
        let (a, b) = (&snapshot[s], &snapshot[r]);
        if (a.length() - b.length()).abs() > 0.5 {
            a.length() > b.length()
        } else if (a.area() - b.area()).abs() > 0.5 {
            a.area() > b.area()
        } else {
            s < r
        }
    };
    for r in 0..snapshot.len() {
        let (ra0, ra1) = snapshot[r].along();
        let (rb0, rb1) = snapshot[r].across();
        let (mut lo, mut hi) = (None::<f64>, None::<f64>);
        for s in 0..snapshot.len() {
            if s == r || snapshot[s].along_u == snapshot[r].along_u || !dominates(s, r) {
                continue;
            }
            // The other wing must span this one's whole width.
            let (sa0, sa1) = snapshot[s].along();
            if sa0 > rb0 + 0.5 || sa1 < rb1 - 0.5 {
                continue;
            }
            let (sc0, sc1) = snapshot[s].across();
            let ridge = (sc0 + sc1) / 2.0;
            if ra1 <= sc1 + 0.5 && ra1 > ridge && ra0 < ridge {
                hi = Some(hi.map_or(ridge, |h| h.max(ridge)));
            }
            if ra0 >= sc0 - 0.5 && ra0 < ridge && ra1 > ridge {
                lo = Some(lo.map_or(ridge, |l| l.min(ridge)));
            }
        }
        let (a0, a1) = (lo.unwrap_or(ra0), hi.unwrap_or(ra1));
        if a1 - a0 < 1.0 {
            continue;
        }
        let w = &mut wings[r];
        if w.along_u {
            w.u = (a0, a1);
        } else {
            w.v = (a0, a1);
        }
        w.clipped = (lo.is_some(), hi.is_some());
    }
    Some(WingFrame { angle, wings })
}

fn generate_gabled_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
    profile: GableProfile,
    ridge: &RidgeFrame,
    wings: Option<&WingFrame>,
) -> RoofSurface {
    // Create a HashSet for O(1) footprint lookups, this is the actual building shape
    let footprint: HashSet<(i32, i32)> = floor_area.iter().copied().collect();

    let ridge_runs_along_x = ridge.along_x;
    let tent = ridge.tent;
    // A turned rectangle is one wing in its own frame, which tells the
    // cells past its gable ends from those past its eaves.
    let tent_frame = tent.map(|t| WingFrame {
        angle: t.angle,
        wings: vec![RoofWing {
            u: (t.u_min, t.u_max),
            v: (t.v_min, t.v_max),
            along_u: ridge_runs_along_x,
            clipped: (false, false),
        }],
    });

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

    // Hard cap: the roof peak should not exceed a fraction of the wall height.
    let cap_factor = if profile == GableProfile::Steep {
        0.9
    } else {
        0.6
    };
    let wall_cap = config.peak_cap.unwrap_or_else(|| {
        (((config.building_height as f64) * cap_factor)
            .round()
            .max(1.0) as i32)
            .min(config.max_rise.unwrap_or(i32::MAX))
    });

    // Slope samples per cell, one for each wing covering it (the highest
    // roof wins, which forms the valleys where wings meet), and the
    // distances to the eaves across the ridge for the dormers.
    let mut samples: Vec<((i32, i32), WingSamples)> = Vec::with_capacity(floor_area.len());
    let mut edge_scans: HashMap<(i32, i32), (i32, i32)> = HashMap::new();

    if let Some(frame) = wings {
        for &(x, z) in floor_area {
            let (u, v) = frame.uv(x, z);
            let mut cands: Vec<(usize, SlopeSample)> = frame
                .wings
                .iter()
                .enumerate()
                .filter_map(|(i, w)| w.slope(u, v, true).map(|s| (i, s)))
                .collect();
            if cands.is_empty() {
                // A sliver the wings miss carries on the nearest one.
                let nearest = (0..frame.wings.len()).min_by(|&a, &b| {
                    frame.wings[a]
                        .gap(u, v)
                        .total_cmp(&frame.wings[b].gap(u, v))
                });
                if let Some(i) = nearest {
                    let w = &frame.wings[i];
                    let (cu, cv) = (u.clamp(w.u.0, w.u.1), v.clamp(w.v.0, w.v.1));
                    cands.extend(w.slope(cu, cv, true).map(|s| (i, s)));
                }
            }
            samples.push(((x, z), cands));
        }
    } else {
        // For each footprint position, scan all 4 cardinal directions to
        // find the distance to the nearest polygon edge. The cross-axis span
        // caps the roof by the narrowest local wing width in any direction.
        for &(x, z) in floor_area {
            let sm_z = scan_dir(x, z, 0, -1);
            let sp_z = scan_dir(x, z, 0, 1);
            let sm_x = scan_dir(x, z, -1, 0);
            let sp_x = scan_dir(x, z, 1, 0);
            // Tent frame: distances to the turned rectangle's sides, v standing
            // in for z and u for x; the scans still bound the hip ends and trim.
            let (dm_z, dp_z, dm_x, dp_x) = match tent {
                Some(t) => {
                    // Whole cells in from each side: the outline cells sit up to
                    // half a cell inside the ideal edge and must stay the eave.
                    let (u, v) = t.uv(x, z);
                    (
                        (v - t.v_min).floor().max(0.0) as i32,
                        (t.v_max - v).floor().max(0.0) as i32,
                        (u - t.u_min).floor().max(0.0) as i32,
                        (t.u_max - u).floor().max(0.0) as i32,
                    )
                }
                None => (sm_z, sp_z, sm_x, sp_x),
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
            // The planar tent meets its rectangle's sides on its own.
            let scan_perp_min = if tent.is_some() {
                NO_FEATHER
            } else if ridge_runs_along_x {
                sm_z.min(sp_z)
            } else {
                sm_x.min(sp_x)
            };
            let half_z = (dm_z + dp_z + 1) / 2;
            let half_x = (dm_x + dp_x + 1) / 2;
            samples.push((
                (x, z),
                vec![(
                    0,
                    SlopeSample {
                        dist_to_edge: dm_perp.min(dp_perp),
                        local_half: half_z.min(half_x),
                        perp_reach: (dm_perp + dp_perp) / 2,
                        dm_along,
                        dp_along,
                        scan_perp_min,
                    },
                )],
            ));
        }
    }

    // The widest half-span decides the pitch for the whole roof.
    let max_reach = samples
        .iter()
        .flat_map(|(_, c)| c.iter())
        .map(|(_, s)| s.perp_reach.min(s.local_half))
        .max()
        .unwrap_or(0);
    // A stated rise sets the pitch: the ridge reaches it at the middle of the
    // span, whether that is a shallow pediment or a steep 2:1 gable, instead
    // of a 1:1 slope cut off into a mesa or capped short of the mapped peak.
    let stated_pitch = config.peak_cap.filter(|_| profile != GableProfile::Gambrel);
    // Wings share the widest one's pitch, so a narrower wing tops out lower.
    let wing_reach = wings.map(|f| f.wings.iter().map(RoofWing::reach).max().unwrap_or(1));
    let stair_mat = get_stair_block_for_material(config.roof_block);
    let slab_fits = profile == GableProfile::Standard && roof_slab_for_stair(stair_mat).is_some();
    // Too wide for 1:1 under the cap: 1:2 in slabs rather than a mesa.
    let shallow = match stated_pitch {
        Some(cap) => (cap as f64) < 0.75 * max_reach.max(1) as f64,
        None => max_reach > wall_cap + 1 && !config.trim.steep_cap,
    };
    let mode = if shallow && slab_fits {
        PitchMode::Slabs
    } else {
        PitchMode::Stairs
    };
    // Without a matching slab a wide span keeps the old stair-and-block treads.
    let tread_pitch = mode == PitchMode::Stairs
        && stated_pitch.is_none()
        && !matches!(profile, GableProfile::Gambrel | GableProfile::Round)
        && !config.trim.steep_cap
        && max_reach - wall_cap >= 4;

    let eave = config.base_height + 1;
    let level_of = |s: &SlopeSample| -> i32 {
        let e = s.dist_to_edge.min(s.scan_perp_min);
        if profile == GableProfile::Round {
            // Barrel vault: a semi-elliptical section, vertical at the eaves and
            // level at the crown, rising to the mapped roof:height or the cap.
            let cap = config.peak_cap.unwrap_or_else(|| {
                wall_cap.min(((s.local_half as f64) * 0.85).round().max(1.0) as i32)
            });
            let reach = s.perp_reach.max(1) as f64;
            let t = (1.0 - s.dist_to_edge as f64 / reach).clamp(0.0, 1.0);
            let boost = (cap as f64 * (1.0 - t * t).sqrt()).round() as i32;
            return 2 * (eave + boost.min(cap.max(1))) + 1;
        }
        // Rise above the eave, in half blocks.
        let mut rise = if let Some(cap) = stated_pitch {
            let reach = wing_reach.unwrap_or(s.perp_reach).max(1);
            let pitch = cap as f64 / reach as f64;
            // Narrow wings climb at the same pitch and top out lower.
            let local = ((s.local_half as f64 * pitch).round() as i32).clamp(1, cap.max(1));
            if mode == PitchMode::Slabs {
                ((2.0 * e as f64 * pitch).round() as i32).min(2 * local)
            } else if pitch < 1.0 {
                // Rounding a pitch short of 1:1 leaves level treads mid-slope;
                // 1:1 topped off at the stated rise reads as a roof.
                2 * e.min(local)
            } else {
                2 * ((e as f64 * pitch).round() as i32).min(local)
            }
        } else if mode == PitchMode::Slabs {
            e.min(s.local_half.max(1)).min(2 * wall_cap)
        } else {
            let slope = if tread_pitch { (e + 1) / 2 } else { e };
            let boost = if profile == GableProfile::Gambrel {
                // 2 blocks per cell over the steep band, then 1 per cell.
                2 * slope.min(2) + (slope - 2).max(0)
            } else {
                slope
            };
            2 * boost.min(s.local_half.max(1)).min(wall_cap)
        };
        if profile == GableProfile::HalfHipped {
            // The hip only bites above half the peak near the gable ends.
            let hip_start = (wall_cap / 2).max(2);
            rise = rise.min(2 * (hip_start + s.dm_along.min(s.dp_along)));
        }
        match mode {
            PitchMode::Stairs => 2 * (eave + rise / 2) + 1,
            PitchMode::Slabs => 2 * eave + rise,
        }
    };

    let mut levels: RoofLevels = HashMap::with_capacity(floor_area.len());
    let mut owner: HashMap<(i32, i32), usize> = HashMap::with_capacity(floor_area.len());
    for (cell, cands) in &samples {
        let best = cands
            .iter()
            .map(|(i, s)| (level_of(s), *i))
            .max_by_key(|&(l, i)| (l, std::cmp::Reverse(i)));
        let (l, i) = best.unwrap_or((2 * eave + i32::from(mode == PitchMode::Stairs), 0));
        levels.insert(*cell, l);
        owner.insert(*cell, i);
    }

    // median along the ridge evens rasterization wobble, identity when aligned.
    // A tent turned well off the grid has no wobble, only the true staircase
    // of its plane, which a median along a cardinal would erode. Wings sit on
    // exact wall lines and need none.
    if wings.is_none() {
        let smooth_along: (i32, i32) = if ridge_runs_along_x { (1, 0) } else { (0, 1) };
        let smooth = tent.is_none_or(|t| t.angle.to_degrees().abs() <= 12.0);
        levels = levels
            .iter()
            .map(|(&(x, z), &h)| {
                let l = levels.get(&(x - smooth_along.0, z - smooth_along.1));
                let r = levels.get(&(x + smooth_along.0, z + smooth_along.1));
                let h = match (l, r) {
                    (Some(&a), Some(&b)) if smooth => {
                        let mut t = [a, h, b];
                        t.sort_unstable();
                        t[1]
                    }
                    _ => h,
                };
                ((x, z), h)
            })
            .collect();
    }

    // Past a gable end the roof carries on at the level of the cell it
    // continues; past an eave it drops away.
    let frame = wings.or(tent_frame.as_ref());
    let along_step: (i32, i32) = if ridge_runs_along_x { (1, 0) } else { (0, 1) };
    let outside = |x: i32, z: i32| -> Option<i32> {
        match frame {
            Some(f) => {
                let (u, v) = f.uv(x, z);
                let (w, s) = f
                    .wings
                    .iter()
                    .filter_map(|w| w.slope(u, v, false).map(|s| (w, s)))
                    .max_by_key(|(_, s)| level_of(s))?;
                let (sx, sz) = WingFrame::ridge_step(w);
                levels
                    .get(&(x - sx, z - sz))
                    .or_else(|| levels.get(&(x + sx, z + sz)))
                    .copied()
                    .or(Some(level_of(&s)))
            }
            None => [
                (x - along_step.0, z - along_step.1),
                (x + along_step.0, z + along_step.1),
            ]
            .iter()
            .find_map(|c| levels.get(c).copied()),
        }
    };

    let tops = place_roof_surface(editor, floor_area, &levels, config, mode, &outside, false);

    // Rake trim past the gable ends; a half-hipped end has none.
    let trims = if profile == GableProfile::HalfHipped {
        Vec::new()
    } else {
        place_rake_trim(editor, floor_area, &tops, config, &outside)
    };

    // Dormers step along the grid; past a slight turn they no longer sit
    // on the slope, a vault has no slope to sit on, and a 1:2 slope is too
    // low for one.
    let dormers_fit = mode == PitchMode::Stairs
        && profile != GableProfile::Round
        && tent.is_none_or(|t| t.angle.to_degrees().abs() <= 12.0)
        && wings.is_none_or(|f| f.angle == 0.0);
    let mut heights: RoofSurface = tops.iter().map(|(&c, &(y, _))| (c, y)).collect();
    if dormers_fit {
        let dormers = match wings {
            Some(f) => {
                // On the main wing's slopes only.
                let main = f.main_wing();
                let w = &f.wings[main];
                let cells: Vec<(i32, i32)> = floor_area
                    .iter()
                    .copied()
                    .filter(|c| owner.get(c) == Some(&main))
                    .collect();
                let (b0, b1) = w.across();
                let scans: HashMap<(i32, i32), (i32, i32)> = cells
                    .iter()
                    .map(|&(x, z)| {
                        let (u, v) = f.uv(x, z);
                        let b = if w.along_u { v } else { u };
                        let whole = |d: f64| (d + 1e-6).floor().max(0.0) as i32;
                        ((x, z), (whole(b - b0), whole(b1 - b)))
                    })
                    .collect();
                place_dormer_windows(
                    editor,
                    &cells,
                    &heights,
                    &scans,
                    config,
                    WingFrame::ridge_step(w),
                    &footprint,
                    None,
                )
            }
            None => place_dormer_windows(
                editor,
                floor_area,
                &heights,
                &edge_scans,
                config,
                along_step,
                &footprint,
                None,
            ),
        };
        for c in dormers {
            heights.remove(&c);
        }
    }

    // Two-block eave overhang on every side the roof drops on, running on
    // under the rake trim so the corners close.
    let mut covered = footprint.clone();
    covered.extend(trims.iter().map(|t| t.0));
    let drops_toward = |x: i32, z: i32, skip: Option<(i32, i32)>| -> u8 {
        ROOF_SIDES
            .iter()
            .enumerate()
            .filter(|(_, &(dx, dz))| {
                skip.is_none_or(|(sx, sz)| (dx, dz) != (sx, sz) && (dx, dz) != (-sx, -sz))
                    && !covered.contains(&(x + dx, z + dz))
                    && outside(x + dx, z + dz).is_none()
            })
            .fold(0u8, |m, (i, _)| m | (1 << i))
    };
    let seeds: Vec<((i32, i32), u8)> = floor_area
        .iter()
        .map(|&(x, z)| ((x, z), drops_toward(x, z, None)))
        .chain(
            trims
                .iter()
                .map(|&((x, z), dir)| ((x, z), drops_toward(x, z, Some(dir)))),
        )
        .filter(|&(_, sides)| sides != 0)
        .collect();
    place_roof_eaves(editor, &seeds, &covered, config, mode);
    heights
}

/// Hipped roof via polygon-edge scanning, capped at 60% of wall height.
fn generate_hipped_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
) -> RoofSurface {
    generate_hipped_roof_inner(editor, floor_area, config, true, None)
}

/// Mansard roof: the hipped scan with a piecewise profile.
fn generate_mansard_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
    steep_h: i32,
) -> RoofSurface {
    generate_hipped_roof_inner(editor, floor_area, config, true, Some(steep_h))
}

/// Height boost of a mansard profile at `dist_to_edge`: a steep 2-cell band
/// rising to `steep_h`, then a shallow 1:2 slope, capped at `cap`.
fn mansard_boost(dist_to_edge: i32, steep_h: i32, cap: i32) -> i32 {
    const STEEP_RUN: i32 = 2;
    let steep = (dist_to_edge.min(STEEP_RUN) * steep_h + STEEP_RUN - 1) / STEEP_RUN;
    let shallow = (dist_to_edge - STEEP_RUN).max(0) / 2;
    (steep + shallow).min(cap)
}

/// Chebyshev distance from each cell to the outline: the run of a hip roof
/// up from its eave. Outline cells are zero, and so is the corner cell of a
/// re-entrant turn, so an L-shaped roof falls into a valley there instead of
/// bulging over the inside corner as scans along the axes made it.
fn hip_edge_distances(
    floor_area: &[(i32, i32)],
    footprint: &HashSet<(i32, i32)>,
) -> HashMap<(i32, i32), i32> {
    let inside = |x: i32, z: i32| footprint.contains(&(x, z));
    let mut dist: HashMap<(i32, i32), i32> = HashMap::with_capacity(floor_area.len());
    let mut queue: VecDeque<(i32, i32)> = VecDeque::new();
    for &(x, z) in floor_area {
        let edge = ROOF_SIDES.iter().any(|&(dx, dz)| !inside(x + dx, z + dz))
            || ROOF_QUARTERS.iter().any(|&(dx, dz)| {
                // Both walls run on straight past the corner: a real turn,
                // not one step of a rasterized diagonal.
                !inside(x + dx, z + dz)
                    && !inside(x + 2 * dx, z + dz)
                    && !inside(x + dx, z + 2 * dz)
                    && inside(x + 2 * dx, z)
                    && inside(x, z + 2 * dz)
            });
        if edge {
            dist.insert((x, z), 0);
            queue.push_back((x, z));
        }
    }
    while let Some((x, z)) = queue.pop_front() {
        let d = dist[&(x, z)];
        for (dx, dz) in ROOF_SIDES.iter().chain(ROOF_QUARTERS.iter()) {
            let n = (x + dx, z + dz);
            if inside(n.0, n.1) && !dist.contains_key(&n) {
                dist.insert(n, d + 1);
                queue.push_back(n);
            }
        }
    }
    dist
}

fn generate_hipped_roof_inner(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
    with_overhang: bool,
    mansard_steep_h: Option<i32>,
) -> RoofSurface {
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

    let wall_cap = config.peak_cap.unwrap_or_else(|| {
        (((config.building_height as f64) * 0.6).round().max(1.0) as i32)
            .min(config.max_rise.unwrap_or(i32::MAX))
    });

    let dist = hip_edge_distances(floor_area, &footprint);
    // Perpendicular-to-ridge scans for dormer placement (ridge = longer axis),
    // and the narrowest local half-span, which caps a mansard's narrow wings.
    let ridge_runs_along_x = config.width() >= config.length();
    let mut edge_scans: HashMap<(i32, i32), (i32, i32)> = HashMap::new();
    let mut local_half: HashMap<(i32, i32), i32> = HashMap::new();
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
        local_half.insert((x, z), ((dm_x + dp_x + 1) / 2).min((dm_z + dp_z + 1) / 2));
    }

    // The hips climb the short half-span, the farthest any cell gets from
    // the eaves: that, not the long one, decides whether 1:1 fits the cap.
    let max_dist = dist.values().copied().max().unwrap_or(0);
    let stair_mat = get_stair_block_for_material(config.roof_block);
    let slab_fits = mansard_steep_h.is_none() && roof_slab_for_stair(stair_mat).is_some();
    let shallow = match config.peak_cap {
        Some(cap) => (cap as f64) < 0.75 * max_dist.max(1) as f64,
        None => max_dist > wall_cap + 1 && !config.trim.steep_cap,
    };
    let mode = if shallow && slab_fits {
        PitchMode::Slabs
    } else {
        PitchMode::Stairs
    };
    // Without a matching slab a wide span keeps the old stair-and-block treads.
    let tread_pitch = mode == PitchMode::Stairs
        && mansard_steep_h.is_none()
        && config.peak_cap.is_none()
        && !config.trim.steep_cap
        && max_dist - wall_cap >= 4;

    let lift = if with_overhang { 1 } else { 0 };
    let eave = config.base_height + lift;
    let mut levels: RoofLevels = HashMap::with_capacity(floor_area.len());
    for &(x, z) in floor_area {
        let e = dist.get(&(x, z)).copied().unwrap_or(0);
        // Rise above the eave, in half blocks.
        let rise = if let Some(steep_h) = mansard_steep_h {
            // Mansard: piecewise profile with its own cap; the local wing
            // width still caps narrow wings so they don't overshoot.
            let cap = wall_cap.max(steep_h + 2).min(config.building_height.max(3));
            let local = local_half.get(&(x, z)).copied().unwrap_or(1);
            let local_cap = ((local as f64) * 0.9).round().max(1.0) as i32 + steep_h;
            2 * mansard_boost(e, steep_h, cap.min(local_cap))
        } else if let Some(cap) = config.peak_cap {
            // A stated rise sets the pitch across the short half-span, so the
            // ridge reaches it at the middle instead of a 1:1 slope cut into a
            // mesa or capped short of the mapped peak.
            let pitch = cap as f64 / max_dist.max(1) as f64;
            if mode == PitchMode::Slabs {
                ((2.0 * e as f64 * pitch).round() as i32).clamp(0, 2 * cap.max(1))
            } else if pitch < 1.0 {
                2 * e.min(cap.max(1))
            } else {
                2 * ((e as f64 * pitch).round() as i32).clamp(0, cap.max(1))
            }
        } else if mode == PitchMode::Slabs {
            e.min(2 * wall_cap)
        } else {
            let slope = if tread_pitch { (e + 1) / 2 } else { e };
            2 * slope.min(wall_cap)
        };
        let level = match mode {
            PitchMode::Stairs => 2 * (eave + rise / 2) + 1,
            PitchMode::Slabs => 2 * eave + rise,
        };
        levels.insert((x, z), level);
    }

    // Every side is an eave: off the footprint the roof always drops.
    let tops = place_roof_surface(
        editor,
        floor_area,
        &levels,
        config,
        mode,
        &|_, _| None,
        true,
    );

    if with_overhang {
        let seeds = all_side_eave_seeds(floor_area, &footprint);
        place_roof_eaves(editor, &seeds, &footprint, config, mode);
    }

    let mut heights: RoofSurface = tops.iter().map(|(&c, &(y, _))| (c, y)).collect();
    if mode == PitchMode::Stairs {
        let parallel_to_ridge = if ridge_runs_along_x { (1, 0) } else { (0, 1) };
        let mansard_band = mansard_steep_h.map(|sh| config.base_height + (sh + 1) / 2 + lift);
        let dormers = place_dormer_windows(
            editor,
            floor_area,
            &heights,
            &edge_scans,
            config,
            parallel_to_ridge,
            &footprint,
            mansard_band,
        );
        for c in dormers {
            heights.remove(&c);
        }
    }
    heights
}

/// Parses `roof:direction` (compass point or degrees) to the nearest cardinal.
/// `roof:direction` as a compass bearing in degrees (0 = north, clockwise),
/// from a compass point or a number.
fn parse_roof_direction_degrees(value: &str) -> Option<f64> {
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
    deg.is_finite().then(|| deg.rem_euclid(360.0))
}

/// The cardinal a bearing rounds to; stairs can only face those four.
fn cardinal_for_bearing(deg: f64) -> StairFacing {
    match ((deg / 90.0).round() as i64).rem_euclid(4) {
        0 => StairFacing::North,
        1 => StairFacing::East,
        2 => StairFacing::South,
        _ => StairFacing::West,
    }
}

/// Unit vector along a compass bearing in world coordinates: north is -Z,
/// east is +X.
fn bearing_unit_vector(deg: f64) -> (f64, f64) {
    let rad = deg.to_radians();
    (rad.sin(), -rad.cos())
}

/// Extent of the roof bounding box along a bearing, in cells: what a
/// mono-pitch roof descending that way climbs over.
fn bbox_extent_along_bearing(width: i32, length: i32, deg: f64) -> f64 {
    let (dx, dz) = bearing_unit_vector(deg);
    (width as f64 * dx).abs() + (length as f64 * dz).abs()
}

/// Tallest inferred roof in blocks: about a 9 m attic over a deep block, more
/// under a church nave. Real roofs stop growing with the walls beneath them.
fn max_inferred_rise(steep: bool, scale_factor: f64) -> i32 {
    ((if steep { 14.0 } else { 9.0 }) * scale_factor)
        .round()
        .max(2.0) as i32
}

/// Roof rise in blocks, from roof:height if mapped, else from the roof:angle pitch.
/// Only roof:height is rescaled; the angle applies to a span already in blocks.
fn roof_peak_cap(
    tags: &HashMap<String, String>,
    roof_type: RoofType,
    config: &RoofConfig,
    ridge: &RidgeFrame,
    scale_factor: f64,
) -> Option<i32> {
    if let Some(m) = tags
        .get("roof:height")
        .and_then(|v| v.trim_end_matches('m').trim().parse::<f64>().ok())
        .filter(|m| *m > 0.0)
    {
        return Some(multiply_scale(m.round() as i32, scale_factor).max(1));
    }

    // These shapes never read peak_cap as a pitch.
    if matches!(
        roof_type,
        RoofType::Flat | RoofType::Dome | RoofType::Cone | RoofType::Onion
    ) {
        return None;
    }

    let deg = tags
        .get("roof:angle")
        .and_then(|v| v.trim_end_matches('\u{b0}').trim().parse::<f64>().ok())
        .filter(|d| *d > 0.0 && *d < 90.0)?;

    // width()/length() are bbox extents, one less than the span in cells.
    let w = config.width() + 1;
    let l = config.length() + 1;

    let run = match roof_type {
        // A skillion climbs its whole run along the mapped bearing, as the
        // generator does; untagged it crosses the shorter axis.
        RoofType::Skillion => match tags
            .get("roof:direction")
            .map(|s| s.as_str())
            .and_then(parse_roof_direction_degrees)
        {
            Some(deg) => bbox_extent_along_bearing(w, l, deg).max(1.0),
            None => w.min(l) as f64,
        },
        // These slope over the shorter half-span in every direction.
        RoofType::Hipped | RoofType::Mansard | RoofType::Pyramidal => w.min(l) as f64 / 2.0,
        // Slopes across the ridge, in the frame generate_gabled_roof uses.
        _ => ridge.span_cells(config.width(), config.length()) / 2.0,
    };

    let rise = (deg.to_radians().tan() * run).round().max(1.0) as i32;
    // An inferred rise still stays under the walls, as the mansard path bounds it.
    Some(rise.min(config.building_height.max(3)))
}

/// Skillion (mono-pitch) roof descending toward `roof:direction`, or across the shorter axis.
///
/// The bearing is used as mapped, not snapped to a cardinal: the roof plane
/// is laid along it, so a wing rotated off the grid, or a chamfer whose low
/// corner points diagonally, still meets its neighbours flush. A mapped
/// `roof:height` is the full rise; only the inferred rise is kept shallow.
fn generate_skillion_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
    roof_direction: Option<&str>,
) {
    let downhill_deg = roof_direction
        .and_then(parse_roof_direction_degrees)
        .unwrap_or_else(|| {
            let mut rng = element_rng(config.element_id_for_decor ^ 0x5C11_1104_D129_0000);
            let flip = rng.random_bool(0.5);
            if config.width() <= config.length() {
                if flip {
                    270.0
                } else {
                    90.0
                }
            } else if flip {
                0.0
            } else {
                180.0
            }
        });
    let (dx, dz) = bearing_unit_vector(downhill_deg);

    // Cells projected onto the downhill axis: the eave sits at the far end,
    // the ridge at the near end, whatever the footprint's shape.
    let (mut p_min, mut p_max) = (f64::MAX, f64::MIN);
    for &(x, z) in floor_area {
        let p = x as f64 * dx + z as f64 * dz;
        p_min = p_min.min(p);
        p_max = p_max.max(p);
    }
    let run = (p_max - p_min).max(1.0);

    // An inferred rise follows the run and stays below the wall height.
    let max_roof_height = config.peak_cap.map_or_else(
        || {
            (run as i32 / 3)
                .clamp(2, 10)
                .min(((config.building_height as f64) * 0.9).round().max(1.0) as i32)
        },
        |p| p.max(1),
    );

    // Stairs face uphill, opposite the downhill direction.
    let stair_facing = cardinal_for_bearing(downhill_deg + 180.0);

    let mut roof_heights = HashMap::new();
    for &(x, z) in floor_area {
        let p = x as f64 * dx + z as f64 * dz;
        let slope_progress = (p_max - p) / run;
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

/// How far a ray may run outside the footprint and still count as having
/// grazed it rather than crossed open ground. Rasterised curves and clipped
/// vertices stay far under one cell; a real notch is several.
const MAX_GRAZE_GAP: f64 = 1.0;

/// Even-odd point-in-polygon test on the polygon's own coordinates.
fn polygon_contains(pts: &[(f64, f64)], x: f64, z: f64) -> bool {
    let mut inside = false;
    let n = pts.len();
    for i in 0..n {
        let (x0, z0) = pts[i];
        let (x1, z1) = pts[(i + 1) % n];
        if (z0 > z) != (z1 > z) {
            let cross = x0 + (z - z0) * (x1 - x0) / (z1 - z0);
            if cross > x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Radial fraction of every roof cell within the footprint polygon: 0 at
/// the area centroid, 1 on the outline, measured along the ray from the
/// centroid through the cell. Its level sets are scaled copies of the
/// outline, so a roof built on it is a pyramid over a rotated square, a cone
/// over a circle, or an ellipsoid over a rotated ellipse, each meeting its
/// eave on every wall. The bounding box the fallbacks use only manages that
/// for axis-aligned rectangles.
///
/// None unless the footprint is star-shaped about that centroid, i.e. every
/// cell is reached by a ray that stays inside it. A courtyard block whose
/// centroid sits in its thick base still hides the inner faces of its wings
/// behind open ground, and a ray that crosses that ground measures the cell
/// against the far side of the wing, so the roof there rides too high and
/// never comes down to the wing's own wall. Those footprints keep the
/// bounding-box profile instead.
fn footprint_radial_fractions(
    nodes: &[ProcessedNode],
    cells: &[(i32, i32)],
) -> Option<HashMap<(i32, i32), f64>> {
    let mut pts: Vec<(f64, f64)> = nodes.iter().map(|n| (n.x as f64, n.z as f64)).collect();
    if pts.len() >= 2 && pts.first() == pts.last() {
        pts.pop();
    }
    pts.dedup();
    if pts.len() < 3 {
        return None;
    }
    let n = pts.len();

    // Area centroid: a run of dense nodes on one wall must not pull it over.
    let (mut area2, mut cx, mut cz) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..n {
        let (x0, z0) = pts[i];
        let (x1, z1) = pts[(i + 1) % n];
        let cross = x0 * z1 - x1 * z0;
        area2 += cross;
        cx += (x0 + x1) * cross;
        cz += (z0 + z1) * cross;
    }
    if area2.abs() < 1e-9 {
        return None;
    }
    let (cx, cz) = (cx / (3.0 * area2), cz / (3.0 * area2));
    if !polygon_contains(&pts, cx, cz) {
        return None;
    }

    // Edges bucketed by the bearings they cover from the centroid, so a cell
    // tests the few edges its ray can cross rather than the whole outline.
    const BUCKETS: usize = 512;
    let bucket_of = |ang: f64| -> usize {
        (((ang + std::f64::consts::PI) / std::f64::consts::TAU) * BUCKETS as f64) as usize
    };
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); BUCKETS];
    for i in 0..n {
        let (ax, az) = pts[i];
        let (bx, bz) = pts[(i + 1) % n];
        let a0 = (az - cz).atan2(ax - cx);
        let a1 = (bz - cz).atan2(bx - cx);
        let (lo, hi) = (a0.min(a1), a0.max(a1));
        // One bucket of slack each side covers rays on a bucket boundary.
        let wraps = hi - lo > std::f64::consts::PI;
        let ranges: Vec<(usize, usize)> = if wraps {
            // The edge crosses the +-180 seam: cover both ends of the circle.
            vec![
                (0, bucket_of(lo) + 1),
                (bucket_of(hi).saturating_sub(1), BUCKETS - 1),
            ]
        } else {
            vec![(bucket_of(lo).saturating_sub(1), bucket_of(hi) + 1)]
        };
        for (from, to) in ranges {
            for bucket in buckets.iter_mut().take(to.min(BUCKETS - 1) + 1).skip(from) {
                bucket.push(i);
            }
        }
    }

    let mut out: HashMap<(i32, i32), f64> = HashMap::with_capacity(cells.len());
    // Scratch for the crossings short of the current cell, reused per cell.
    let mut before: Vec<f64> = Vec::new();
    for &(x, z) in cells {
        let (px, pz) = (x as f64 - cx, z as f64 - cz);
        let dist = (px * px + pz * pz).sqrt();
        if dist < 1e-9 {
            out.insert((x, z), 0.0);
            continue;
        }
        let (ux, uz) = (px / dist, pz / dist);
        // Ray C + t*u against each edge A + s*(B - A). The outline is the first
        // crossing at or beyond the cell; wall cells rasterised a little past
        // it lean on the half-cell slack and land on 1.
        let mut t_hit: Option<f64> = None;
        let mut t_far = 0.0f64;
        before.clear();
        for &i in &buckets[bucket_of(pz.atan2(px)).min(BUCKETS - 1)] {
            let (ax, az) = pts[i];
            let (bx, bz) = pts[(i + 1) % n];
            let (ex, ez) = (bx - ax, bz - az);
            let denom = ux * ez - uz * ex;
            if denom.abs() < 1e-12 {
                continue;
            }
            let (wx, wz) = (ax - cx, az - cz);
            let t = (wx * ez - wz * ex) / denom;
            let sp = (wx * uz - wz * ux) / denom;
            if t <= 0.0 || !(-1e-9..=1.0 + 1e-9).contains(&sp) {
                continue;
            }
            t_far = t_far.max(t);
            if t + 0.5 >= dist {
                t_hit = Some(t_hit.map_or(t, |h: f64| h.min(t)));
            } else {
                before.push(t);
            }
        }
        // Crossings short of the cell come in pairs: the ray leaves the
        // footprint and comes back. A pair that close together is the ray
        // clipping a vertex or a rasterised bump, which costs nothing; a pair
        // a whole cell apart is real open ground, and this footprint is not
        // one the centroid can measure.
        before.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        if before
            .as_chunks::<2>()
            .0
            .iter()
            .any(|pair| pair[1] - pair[0] > MAX_GRAZE_GAP)
        {
            return None;
        }
        let t = t_hit.unwrap_or(t_far);
        let r = if t > 0.0 { (dist / t).min(1.0) } else { 1.0 };
        out.insert((x, z), r);
    }
    Some(out)
}

/// Pyramidal roof: tapers to a single apex above the footprint's centroid.
fn generate_pyramidal_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
    nodes: &[ProcessedNode],
) {
    let footprint: HashSet<(i32, i32)> = floor_area.iter().copied().collect();
    // A mapped roof:height is the apex rise, a spire included; an inferred
    // one stays squat against both the footprint and the walls.
    let peak_boost = config.peak_cap.unwrap_or_else(|| {
        let shorter_half = config.width().min(config.length()) / 2;
        let uncapped_boost = ((shorter_half as f64) * 0.85).round().max(1.0) as i32;
        let wall_cap = (((config.building_height as f64) * 0.6).round().max(1.0) as i32)
            .min(config.max_rise.unwrap_or(i32::MAX));
        uncapped_boost.min(wall_cap)
    });
    // Per-axis halves for the bounding-box fallback, so elongated footprints reach the eave.
    let half_w = (config.width() / 2).max(1) as f64;
    let half_l = (config.length() / 2).max(1) as f64;
    let radial = footprint_radial_fractions(nodes, floor_area);

    let mut roof_heights: HashMap<(i32, i32), i32> = HashMap::new();
    for &(x, z) in floor_area {
        let fraction = radial.as_ref().map_or_else(
            || {
                let dx = (x - config.center_x).abs() as f64;
                let dz = (z - config.center_z).abs() as f64;
                (dx / half_w).max(dz / half_l).min(1.0)
            },
            |m| m[&(x, z)],
        );
        let height_factor = 1.0 - fraction;
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
fn generate_dome_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
    nodes: &[ProcessedNode],
) {
    // elliptical normalization so the shell meets the eave on every wall
    let half_w = (config.width() as f64 / 2.0).max(1.0);
    let half_l = (config.length() as f64 / 2.0).max(1.0);
    // A mapped roof:height is the crown rise; inferred domes stay squat.
    let rise = config
        .peak_cap
        .map_or_else(|| (half_w.min(half_l) * 0.8).max(1.0), |p| p.max(1) as f64);
    let radial = footprint_radial_fractions(nodes, floor_area);
    // Use empty blacklist to allow overwriting wall/ceiling blocks
    let replace_any: &[Block] = &[];
    let mut surface: HashMap<(i32, i32), i32> = HashMap::with_capacity(floor_area.len());

    for &(x, z) in floor_area {
        let normalized_distance = radial.as_ref().map_or_else(
            || {
                let nx = (x - config.center_x) as f64 / half_w;
                let nz = (z - config.center_z) as f64 / half_l;
                (nx * nx + nz * nz).sqrt().min(1.0)
            },
            |m| m[&(x, z)],
        );

        let height_factor = (1.0 - normalized_distance * normalized_distance).sqrt();
        surface.insert((x, z), config.base_height + (height_factor * rise) as i32);
    }
    fill_roof_shell(editor, floor_area, &surface, config, replace_any);
}

/// Places a rounded roof as a shell over its surface heights: every column
/// runs from one below its lowest neighbour's surface up to its own, so the
/// shell is watertight and two blocks thick at the crown, and the space
/// beneath a dome or cone stays a hall instead of a solid mass.
fn fill_roof_shell(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    surface: &HashMap<(i32, i32), i32>,
    config: &RoofConfig,
    replace_any: &[Block],
) {
    for &(x, z) in floor_area {
        let top = surface[&(x, z)];
        let lowest_neighbour = [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)]
            .iter()
            .map(|n| surface.get(n).copied().unwrap_or(config.base_height))
            .min()
            .unwrap_or(config.base_height);
        let bottom = (lowest_neighbour - 1).max(config.base_height);
        for y in bottom..=top {
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
fn generate_cone_roof(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    config: &RoofConfig,
    nodes: &[ProcessedNode],
) {
    let half_w = (config.width() as f64 / 2.0).max(1.0);
    let half_l = (config.length() as f64 / 2.0).max(1.0);
    let replace_any: &[Block] = &[];

    // A mapped roof:height is the apex rise, a spire included.
    let peak_height = config.peak_cap.map_or_else(
        || {
            ((half_w.min(half_l) * 1.2) as i32)
                .max(2)
                .min(config.building_height * 2)
        },
        |p| p.max(1),
    );
    let radial = footprint_radial_fractions(nodes, floor_area);
    let mut surface: HashMap<(i32, i32), i32> = HashMap::with_capacity(floor_area.len());

    for &(x, z) in floor_area {
        let normalized = radial.as_ref().map_or_else(
            || {
                let dx = (x - config.center_x) as f64;
                let dz = (z - config.center_z) as f64;
                ((dx / half_w).powi(2) + (dz / half_l).powi(2))
                    .sqrt()
                    .min(1.0)
            },
            |m| m[&(x, z)],
        );

        surface.insert(
            (x, z),
            config.base_height + ((1.0 - normalized) * peak_height as f64) as i32,
        );
    }
    fill_roof_shell(editor, floor_area, &surface, config, replace_any);
}

/// Onion roof: bulbous Russian-Orthodox / Bavarian profile.
fn generate_onion_roof(editor: &mut WorldEditor, floor_area: &[(i32, i32)], config: &RoofConfig) {
    // shorter axis, the bulb bulges a little instead of half the long axis
    let base_radius = (config.width().min(config.length()) / 2) as f64;
    let replace_any: &[Block] = &[];

    // A mapped roof:height is the tip; the bulb profile scales with it.
    let total_height = config.peak_cap.map_or_else(
        || {
            ((base_radius * 1.8) as i32)
                .max(6)
                .min(config.building_height * 2)
        },
        |p| p.max(1),
    );

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
    flat_cover: &RoofMembrane,
    wall_block: Block,
    curtain_glass: Option<Block>,
    roof_block_override: Option<Block>,
    roof_type: RoofType,
    roof_area: &[(i32, i32)],
    abs_terrain_offset: i32,
    add_dormers: bool,
    style_seed: u64,
    steep_gable: bool,
    preferred_ridge_along_x: Option<bool>,
    scale_factor: f64,
    rise_hint: Option<i32>,
    trim: RoofTrim,
    holes: &[&[ProcessedNode]],
) -> RoofSurface {
    if roof_area.is_empty() {
        return RoofSurface::new();
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
    let osm_roof_block = tagged_roof_block(
        &element.tags,
        roof_type != RoofType::Flat,
        roof_block_override,
        &mut roof_rng,
    );

    if let Some(block) = osm_roof_block {
        // A glass roof over a glass wall carries the curtain wall on: a
        // sloped face mapped as a roof must not change tint at the seam.
        config.roof_block = if !is_glass_wall(block) {
            block
        } else if let Some(glass) = curtain_glass {
            glass
        } else if is_glass_wall(wall_block) {
            wall_block
        } else {
            block
        };
    } else if let Some(override_block) = roof_block_override {
        config.roof_block = override_block;
    }

    config.add_dormers = add_dormers;
    config.trim = trim;
    // A mapped roof surface keeps its mapped material on the flat top too.
    if osm_roof_block.is_some() {
        config.trim.sheet_top = false;
        config.trim.tone_patches = false;
    }

    let roof_orientation = element.tags.get("roof:orientation").map(|s| s.as_str());
    let roof_direction = element.tags.get("roof:direction").map(|s| s.as_str());
    let ridge = ridge_frame(
        &element.nodes,
        config.width(),
        config.length(),
        roof_orientation,
        roof_direction,
        preferred_ridge_along_x,
    );
    // A mapped roof:height, else a roof:angle pitch, overrides the heuristic caps.
    let peak_cap = roof_peak_cap(&element.tags, roof_type, &config, &ridge, scale_factor);
    config.peak_cap = peak_cap.or(rise_hint);
    config.max_rise = Some(max_inferred_rise(steep_gable, scale_factor));
    // An L, T, U or courtyard footprint gets a ridge per wing. A mapped or
    // preferred ridge direction applies to the main wing.
    let forced_ridge = (roof_orientation.is_some()
        || roof_direction.is_some()
        || preferred_ridge_along_x.is_some())
    .then_some(ridge.along_x);
    let wings = if ridge.tent.is_none()
        && matches!(
            roof_type,
            RoofType::Gabled | RoofType::Gambrel | RoofType::HalfHipped
        ) {
        roof_wings(&element.nodes, holes, roof_area.len(), forced_ridge)
    } else {
        None
    };

    let mut surface = RoofSurface::new();
    match roof_type {
        // The cover already ranks OSM tags over the preset over the membrane.
        RoofType::Flat => {
            generate_flat_roof(
                editor,
                roof_area,
                flat_cover,
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
            surface =
                generate_gabled_roof(editor, roof_area, &config, profile, &ridge, wings.as_ref());
        }

        RoofType::Gambrel => {
            surface = generate_gabled_roof(
                editor,
                roof_area,
                &config,
                GableProfile::Gambrel,
                &ridge,
                wings.as_ref(),
            );
        }

        RoofType::HalfHipped => {
            surface = generate_gabled_roof(
                editor,
                roof_area,
                &config,
                GableProfile::HalfHipped,
                &ridge,
                wings.as_ref(),
            );
        }

        RoofType::Round => {
            generate_gabled_roof(
                editor,
                roof_area,
                &config,
                GableProfile::Round,
                &ridge,
                None,
            );
        }

        RoofType::Hipped => {
            surface = generate_hipped_roof(editor, roof_area, &config);
        }

        RoofType::Mansard => {
            // roof:levels hints how tall the steep band is (3 blocks per level).
            let steep_h = element
                .tags
                .get("roof:levels")
                .and_then(|v| v.trim().parse::<f64>().ok())
                .map(|l| ((l * 3.0).round() as i32).clamp(3, 6))
                .unwrap_or(4);
            surface = generate_mansard_roof(editor, roof_area, &config, steep_h);
        }

        RoofType::Skillion => {
            generate_skillion_roof(editor, roof_area, &config, roof_direction);
        }

        RoofType::Pyramidal => {
            generate_pyramidal_roof(editor, roof_area, &config, &element.nodes);
        }

        RoofType::Dome => {
            generate_dome_roof(editor, roof_area, &config, &element.nodes);
        }

        RoofType::Cone => {
            generate_cone_roof(editor, roof_area, &config, &element.nodes);
        }

        RoofType::Onion => {
            generate_onion_roof(editor, roof_area, &config);
        }
    }
    surface
}

/// Synthetic way id a relation's `ring_idx`-th outer ring is built under.
/// Bit 63 keeps it out of the real way id range, so it cannot collide in the
/// flood fill cache or the deterministic RNG seeded by element id; the hole
/// rings use the same scheme with bit 15 of the ring slot set.
pub(crate) fn relation_ring_id(relation_id: u64, ring_idx: usize) -> u64 {
    (1u64 << 63) | (relation_id << 16) | (ring_idx as u64 & 0xFFFF)
}

/// Merges member way segments into rings, clips them to the world and keeps
/// the closed ones.
///
/// Multipolygon relations commonly split the outline across many short way
/// segments that share endpoints. Without merging, each segment would be
/// processed individually, producing degenerate polygons and empty flood
/// fills (only wall outlines, no filled floors, ceilings or roofs). Member
/// ways were kept unclipped during parsing to allow that assembly, so the
/// merged rings may extend beyond the requested area and are clipped here;
/// clipping prevents oversized flood fills and unnecessary block placement.
/// Rings whose ends are within one block are closed, the rest are dropped.
fn assemble_relation_rings(
    mut rings: Vec<Vec<ProcessedNode>>,
    xzbbox: &crate::coordinate_system::cartesian::XZBBox,
) -> Vec<Vec<ProcessedNode>> {
    super::merge_way_segments(&mut rings);

    let mut rings: Vec<Vec<ProcessedNode>> = rings
        .into_iter()
        .map(|ring| clip_way_to_bbox(&ring, xzbbox))
        .filter(|ring| ring.len() >= 4)
        .collect();

    // Close rings that are nearly closed (endpoints within 1 block)
    for ring in &mut rings {
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
    rings.retain(|ring| {
        if ring.len() < 4 {
            return false;
        }
        let first = &ring[0];
        let last = ring.last().unwrap();
        first.id == last.id || ((first.x - last.x).abs() <= 1 && (first.z - last.z).abs() <= 1)
    });
    rings
}

/// The closed outer rings a building relation is generated from, each with
/// the synthetic way id `generate_buildings` runs under for it, in generation
/// order. Empty when the relation builds nothing of its own: it is
/// underground, it is a type=building relation whose parts render standalone,
/// or its outer members are closed building:part rings that render on their
/// own. The facade textures use this to key a relation's walls the way the
/// wall builder will ask for them.
pub(crate) fn relation_outer_rings(
    relation: &ProcessedRelation,
    xzbbox: &crate::coordinate_system::cartesian::XZBBox,
) -> Vec<(u64, Vec<ProcessedNode>)> {
    if is_underground_building(&relation.tags) {
        return Vec::new();
    }

    // Check if this is a type=building relation with part members.
    // Only type=building relations use Part roles; type=multipolygon relations
    // should always render their Outer members normally.
    let is_building_type = relation.tags.get("type").map(|t| t.as_str()) == Some("building");
    let has_parts = is_building_type
        && relation
            .members
            .iter()
            .any(|m| m.role == ProcessedMemberRole::Part);
    if has_parts {
        // Parts are rendered as standalone ways from the elements list. The
        // outline way is suppressed in data_processing to avoid overlaying the parts.
        return Vec::new();
    }

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
        return Vec::new();
    }

    let outer_rings: Vec<Vec<ProcessedNode>> = relation
        .members
        .iter()
        .filter(|m| m.role == ProcessedMemberRole::Outer && !SKIP_WAY_IDS.contains(&m.way.id))
        .map(|m| m.way.nodes.clone())
        .collect();
    assemble_relation_rings(outer_rings, xzbbox)
        .into_iter()
        .enumerate()
        .map(|(ring_idx, ring)| (relation_ring_id(relation.id, ring_idx), ring))
        .collect()
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

    // The closed outer rings this relation is built from, with the ids the
    // buildings are generated under. Nothing when the relation renders no
    // building of its own (see `relation_outer_rings`).
    let outer_rings = relation_outer_rings(relation, xzbbox);
    if outer_rings.is_empty() {
        return;
    }

    // Collect and assemble inner rings for courtyards/holes.
    let inner_rings = assemble_relation_rings(
        relation
            .members
            .iter()
            .filter(|m| m.role == ProcessedMemberRole::Inner)
            .map(|m| m.way.nodes.clone())
            .collect(),
        xzbbox,
    );

    let hole_polygons: Option<Vec<HolePolygon>> = if inner_rings.is_empty() {
        None
    } else {
        Some(
            inner_rings
                .into_iter()
                .enumerate()
                .map(|(ring_idx, ring)| {
                    // Use a different index range from outer rings to avoid cache collisions.
                    let ring_slot = 0x8000 | (ring_idx & 0x7FFF);
                    HolePolygon {
                        way: ProcessedWay {
                            id: relation_ring_id(relation.id, ring_slot),
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
    for (synthetic_id, ring) in outer_rings {
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
    fn tent_frame_catches_any_rotation_of_a_rectangle() {
        for deg in [4.0, 8.0, 20.0, 44.0] {
            let frame = tent_frame(&rotated_rect(12.0, 30.0, deg))
                .unwrap_or_else(|| panic!("{deg} deg rectangle should get a tent"));
            assert!(
                (frame.angle.to_degrees() - deg).abs() < 3.0,
                "{deg} deg: frame angle {}",
                frame.angle.to_degrees()
            );
            // The frame's long axis is the rectangle's 30 side.
            assert!((frame.u_max - frame.u_min - 30.0).abs() < 2.5);
            assert!((frame.v_max - frame.v_min - 12.0).abs() < 2.5);
        }
    }

    #[test]
    fn tent_frame_leaves_aligned_rectangles_alone() {
        assert!(tent_frame(&rotated_rect(12.0, 30.0, 0.0)).is_none());
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
        assert!(tent_frame(&ring(&pts)).is_none());
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

    fn european_roof(
        tags: &[(&str, &str)],
        era: ArchEra,
        attached: bool,
        seed: u64,
    ) -> Option<(RoofType, bool)> {
        let way = ProcessedWay {
            id: 1,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            nodes: ring(&[(0, 0), (19, 0), (19, 14), (0, 14), (0, 0)]),
        };
        let ctx = RoofContext {
            region: BuildingRegion::Europe,
            attached,
            scale: 1.0,
            ..RoofContext::default()
        };
        european_default_roof(&way, "apartments", era, 18, 300, seed, ctx)
    }

    // Attached old-town blocks of unknown age are mostly pitched in Europe.
    #[test]
    fn european_old_town_fabric_is_mostly_pitched() {
        let pitched = (0..200u64)
            .filter(|&seed| {
                matches!(
                    european_roof(&[("building", "apartments")], ArchEra::Unknown, true, seed),
                    Some((RoofType::Gabled, true))
                )
            })
            .count();
        assert!(pitched > 130, "{pitched}");
        assert_eq!(
            european_roof(&[("building", "apartments")], ArchEra::Unknown, true, 7),
            european_roof(&[("building", "apartments")], ArchEra::Unknown, true, 7)
        );
    }

    // Contemporary blocks stay mostly flat, and other regions are left alone.
    #[test]
    fn european_rule_spares_modern_blocks_and_other_regions() {
        let pitched = (0..200u64)
            .filter(|&seed| {
                european_roof(
                    &[("building", "apartments")],
                    ArchEra::Contemporary,
                    false,
                    seed,
                )
                .is_some_and(|(_, generate)| generate)
            })
            .count();
        assert!(pitched < 80, "{pitched}");
        let way = way_with_tags(&[("building", "apartments")]);
        let ctx = RoofContext {
            region: BuildingRegion::NorthAmerica,
            attached: true,
            scale: 1.0,
            ..RoofContext::default()
        };
        assert!(
            european_default_roof(&way, "apartments", ArchEra::Unknown, 18, 300, 1, ctx).is_none()
        );
    }

    fn regional_roof(
        building: &str,
        materials: MaterialRegion,
        climate: Climate,
        footprint: usize,
        height: i32,
        seed: u64,
    ) -> Option<(RoofType, bool)> {
        let way = way_with_tags(&[("building", building)]);
        let ctx = RoofContext {
            region: materials.building_region(),
            materials,
            scale: 1.0,
            ..RoofContext::default()
        };
        regional_default_roof(
            &way,
            building,
            ArchEra::Unknown,
            climate,
            height,
            footprint,
            seed,
            ctx,
        )
    }

    fn pitched_share(
        building: &str,
        materials: MaterialRegion,
        climate: Climate,
        footprint: usize,
        height: i32,
    ) -> usize {
        (0..400u64)
            .filter(|&seed| {
                regional_roof(building, materials, climate, footprint, height, seed)
                    .expect("a habit region decides")
                    .1
            })
            .count()
    }

    // Japanese and Australian houses are pitched, Cairo's and Mumbai's flat;
    // multi-storey blocks are flat everywhere a habit holds.
    #[test]
    fn regional_habits_pitch_houses_and_flatten_blocks() {
        use MaterialRegion as R;
        let t = Climate::Temperate;
        assert!(pitched_share("house", R::Japan, t, 90, 9) > 340);
        assert!(pitched_share("yes", R::Japan, t, 90, 9) > 340);
        assert!(pitched_share("yes", R::Australasia, Climate::HotDesert, 180, 5) > 360);
        assert!(pitched_share("yes", R::MiddleEast, Climate::HotDesert, 120, 9) < 20);
        assert!(pitched_share("house", R::SouthAsia, t, 120, 9) < 100);
        assert!(pitched_share("apartments", R::Japan, t, 600, 30) == 0);
        assert!(pitched_share("apartments", R::SouthAsia, t, 600, 30) == 0);
        // Pre-revolutionary walk-ups keep their sheet roofs, towers do not.
        assert!(pitched_share("apartments", R::EastEurope, t, 700, 18) > 150);
        assert!(pitched_share("apartments", R::EastEurope, t, 700, 40) == 0);
        // The Sahel is flatter than the East African highlands.
        assert!(
            pitched_share("yes", R::SubSaharanAfrica, Climate::HotDesert, 120, 6)
                < pitched_share("yes", R::SubSaharanAfrica, t, 120, 6)
        );
        // Hipped roofs lead in Australia.
        let hipped = (0..400u64)
            .filter(|&seed| {
                regional_roof("house", R::Australasia, t, 180, 5, seed)
                    == Some((RoofType::Hipped, true))
            })
            .count();
        assert!(hipped > 220, "{hipped}");
        assert_eq!(
            regional_roof("house", R::Korea, t, 100, 9, 5),
            regional_roof("house", R::Korea, t, 100, 9, 5)
        );
    }

    // Europe, North America and the unmapped rest keep their own rules, and
    // shops and industry are left to the general ones.
    #[test]
    fn regional_habits_leave_other_rules_alone() {
        use MaterialRegion as R;
        for materials in [
            R::Central,
            R::Mediterranean,
            R::NorthAmerica,
            R::Canada,
            R::Other,
        ] {
            assert!(regional_roof("house", materials, Climate::Temperate, 90, 9, 1).is_none());
        }
        assert!(regional_roof("retail", R::Japan, Climate::Temperate, 90, 9, 1).is_none());
        assert!(regional_roof("industrial", R::Japan, Climate::Temperate, 900, 9, 1).is_none());
        // Moscow is in Europe but follows its own habit.
        let way = way_with_tags(&[("building", "apartments")]);
        let ctx = RoofContext {
            region: BuildingRegion::Europe,
            materials: R::EastEurope,
            attached: true,
            scale: 1.0,
            ..RoofContext::default()
        };
        assert!(
            european_default_roof(&way, "apartments", ArchEra::Unknown, 18, 300, 1, ctx).is_none()
        );
    }

    #[test]
    fn every_region_has_walls_for_every_typology() {
        use MaterialRegion as R;
        use WallTypology as T;
        let regions = [
            R::Canada,
            R::EastEurope,
            R::CentralAsia,
            R::MiddleEast,
            R::SubSaharanAfrica,
            R::SouthAsia,
            R::SoutheastAsia,
            R::EastAsia,
            R::Korea,
            R::Japan,
            R::LatinAmerica,
            R::SouthernCone,
            R::Australasia,
        ];
        for region in regions {
            for typology in [T::House, T::Traditional, T::Civic, T::Modern] {
                let pool = regional_wall_pool(region, typology)
                    .unwrap_or_else(|| panic!("{region:?} {typology:?}"));
                assert!(pool.len() >= 6, "{region:?} {typology:?}");
                assert!(
                    REGIONAL_WALL_POOLS.contains(&pool),
                    "{region:?} {typology:?}"
                );
            }
        }
        // Desert towns are pale; Soviet blocks grey and white.
        let pale = |pool: WallPool| {
            let total: u32 = pool.iter().map(|&(_, w)| u32::from(w)).sum();
            let light: u32 = pool
                .iter()
                .filter(|(b, _)| {
                    matches!(
                        *b,
                        SMOOTH_SANDSTONE
                            | SANDSTONE
                            | WHITE_CONCRETE
                            | CALCITE
                            | MUSHROOM_STEM
                            | WHITE_TERRACOTTA
                            | QUARTZ_BLOCK
                            | LIGHT_GRAY_CONCRETE
                            | SMOOTH_STONE
                    )
                })
                .map(|&(_, w)| u32::from(w))
                .sum();
            light * 100 / total
        };
        assert!(pale(MIDDLE_EAST_TRADITIONAL_WALLS) >= 75);
        assert!(pale(MIDDLE_EAST_MODERN_WALLS) >= 90);
        assert!(pale(EAST_MODERN_WALLS) >= 60);
        assert!(pale(JAPAN_HOUSE_WALLS) >= 60);
    }

    #[test]
    fn regional_pitched_roofs_draw_from_the_region() {
        use MaterialRegion as R;
        for (materials, pool) in [
            (R::Japan, JAPAN_ROOFS),
            (R::EastEurope, EAST_ROOFS),
            (R::Australasia, AUSTRALASIA_ROOFS),
            (R::SubSaharanAfrica, AFRICA_ROOFS),
            (R::LatinAmerica, LATAM_ROOFS),
        ] {
            let ctx = RoofContext {
                region: materials.building_region(),
                materials,
                district: 3,
                ..RoofContext::default()
            };
            for seed in 0..80u64 {
                let roof = pitched_roof_block(
                    BuildingCategory::House,
                    ArchEra::Unknown,
                    RoofType::Hipped,
                    WHITE_CONCRETE,
                    ctx,
                    seed,
                );
                assert!(
                    pool.iter().any(|&(b, _)| b == roof),
                    "{materials:?}: {}",
                    roof.name()
                );
            }
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

    // An elevation mapped as height (e.g. an alpine hut with height=1640)
    // is ignored, levels still apply
    #[test]
    fn implausible_height_is_ignored() {
        let way = way_with_tags(&[("height", "1640"), ("building:levels", "2")]);
        let (h, tall) = calculate_building_height(&way, "yes", 0, 1.0, None, 4, 100, 1);
        assert_eq!(h, 10);
        assert!(!tall);

        let way = way_with_tags(&[("height", "828")]);
        let (h, _) = calculate_building_height(&way, "yes", 0, 1.0, None, 4, 100, 1);
        assert_eq!(h, 828);
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
        // Row 0 is the band between storeys, never glass.
        for archetype in [Sash, Portrait, Spaced, Wide, Ribbon, Hooded, Piers, Strips] {
            assert!(!archetype_allows_window(archetype, 0, 4));
            assert!(archetype_allows_window(archetype, 2, 4));
        }
        // Portrait windows take every open row.
        assert!(archetype_allows_window(Portrait, 1, 4));
        assert!(archetype_allows_window(Portrait, 3, 4));
        // Single sashes stay two rows tall.
        assert!(archetype_allows_window(Spaced, 1, 4));
        assert!(!archetype_allows_window(Spaced, 3, 4));
        // A ribbon keeps a two-row spandrel on a tall storey only.
        assert!(!archetype_allows_window(Ribbon, 1, 4));
        assert!(archetype_allows_window(Ribbon, 1, 3));
    }

    #[test]
    fn archetype_choice_is_deterministic_and_defaults_hold() {
        for seed in 0..30u64 {
            let pick = |seed| {
                pick_window_archetype(
                    BuildingCategory::House,
                    ArchEra::Unknown,
                    BuildingRegion::Europe,
                    BRICK,
                    seed,
                )
            };
            assert_eq!(pick(seed), pick(seed));
        }
        // Unhandled categories keep plain portrait windows; churches slender ones.
        assert_eq!(
            pick_window_archetype(
                BuildingCategory::Religious,
                ArchEra::Unknown,
                BuildingRegion::Other,
                STONE_BRICKS,
                7
            ),
            WindowArchetype::Spaced
        );
        assert_eq!(
            pick_window_archetype(
                BuildingCategory::GlassySkyscraper,
                ArchEra::Unknown,
                BuildingRegion::Other,
                GLASS,
                7
            ),
            WindowArchetype::Portrait
        );
    }

    #[test]
    fn houses_vary_archetypes_across_seeds() {
        let mut seen = std::collections::HashSet::new();
        for seed in 0..60u64 {
            seen.insert(format!(
                "{:?}",
                pick_window_archetype(
                    BuildingCategory::House,
                    ArchEra::Unknown,
                    BuildingRegion::NorthAmerica,
                    BRICK,
                    seed
                )
            ));
        }
        assert!(seen.len() >= 3, "expected variety, got {seen:?}");
    }

    #[test]
    fn era_and_region_shift_archetypes() {
        use WindowArchetype::*;
        let pick = |category, era, region, wall, seed| {
            pick_window_archetype(category, era, region, wall, seed)
        };
        let (mut hooded, mut sashes, mut spaced) = (0, 0, 0);
        for seed in 0..60u64 {
            let ornate = pick(
                BuildingCategory::House,
                ArchEra::HistoricOrnate,
                BuildingRegion::Europe,
                BRICK,
                seed,
            );
            // Masonry eras punch their windows.
            assert!(
                !matches!(ornate, Ribbon | Wide),
                "ornate era picked {ornate:?}"
            );
            hooded += i32::from(ornate == Hooded);
            // Walk-up tenements in New York, period-three stucco in Munich.
            sashes += i32::from(
                pick(
                    BuildingCategory::Residential,
                    ArchEra::Unknown,
                    BuildingRegion::NorthAmerica,
                    BRICK,
                    seed,
                ) == Sash,
            );
            spaced += i32::from(
                pick(
                    BuildingCategory::Residential,
                    ArchEra::Unknown,
                    BuildingRegion::Europe,
                    WHITE_CONCRETE,
                    seed,
                ) == Spaced,
            );
            // A brick tower stacks windows between piers, never bands them.
            let tower = pick(
                BuildingCategory::TallBuilding,
                ArchEra::Unknown,
                BuildingRegion::NorthAmerica,
                BRICK,
                seed,
            );
            assert!(
                matches!(tower, Piers | Strips),
                "brick tower picked {tower:?}"
            );
            // Panel-era blocks band theirs.
            let panel = pick(
                BuildingCategory::Residential,
                ArchEra::PostWarPanel,
                BuildingRegion::Europe,
                GRAY_CONCRETE,
                seed,
            );
            assert!(
                !matches!(panel, Sash | Spaced | Hooded),
                "panel picked {panel:?}"
            );
        }
        assert!(
            hooded >= 8,
            "ornate era should hood windows, got {hooded}/60"
        );
        assert!(
            sashes >= 12,
            "tenements should favour sashes, got {sashes}/60"
        );
        assert!(
            spaced >= 20,
            "stucco should favour spaced sashes, got {spaced}/60"
        );
    }

    #[test]
    fn panel_era_walls_stay_in_the_allow_list() {
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

    /// Every run is laid out symmetrically, ends on solid piers, and keeps
    /// whole windows of the rhythm's width.
    #[test]
    fn bay_layouts_are_symmetric_and_never_touch_a_corner() {
        use WindowArchetype::*;
        let rhythms = [Sash, Portrait, Spaced, Wide, Ribbon, Piers, Strips]
            .map(WindowArchetype::rhythm)
            .into_iter()
            .chain([SHOP_RHYTHM]);
        for rhythm in rhythms {
            for len in 1..90 {
                let layout = BayLayout::plan(len, rhythm);
                let slots: Vec<BaySlot> = (0..len).map(|i| layout.slot(i)).collect();
                let glass: Vec<bool> = slots.iter().map(|s| s.is_window()).collect();
                let mirrored: Vec<bool> = glass.iter().rev().copied().collect();
                assert_eq!(glass, mirrored, "{rhythm:?} over {len} is lopsided");
                if len >= 2 {
                    assert_eq!(
                        slots[0],
                        BaySlot::End,
                        "{rhythm:?} over {len} starts on glass"
                    );
                    assert_eq!(slots[len as usize - 1], BaySlot::End);
                }
                // Windows are never narrower than the rhythm, at most one column wider.
                let mut run = 0;
                for &g in glass.iter().chain([false].iter()) {
                    if g {
                        run += 1;
                    } else if run > 0 {
                        assert!(
                            (rhythm.window..=rhythm.window + 1).contains(&run),
                            "{rhythm:?} over {len} has a {run}-wide window"
                        );
                        run = 0;
                    }
                }
                // A run with room for a window keeps at least one.
                if len >= rhythm.window + 2 * rhythm.corner {
                    assert!(layout.count >= 1, "{rhythm:?} over {len} has no window");
                }
            }
        }
    }

    #[test]
    fn a_straight_wall_split_by_a_node_is_one_run() {
        // The north wall carries a mid node, as where a neighbour's party
        // wall meets it; the bays must run straight through it.
        let ring: Vec<ProcessedNode> = [(0, 0), (9, 0), (20, 0), (20, 10), (0, 10), (0, 0)]
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: i as u64,
                tags: HashMap::new(),
                x,
                z,
            })
            .collect();
        let config = test_config(14, false, false);
        let columns = facade_columns(&ring, &config);
        let north: Vec<&WallColumn> = columns.iter().filter(|c| c.seg <= 1).collect();
        assert!(north.iter().all(|c| c.upper.len == 21));
        // The shared node takes one position, as a window or pier of the run.
        let at_node: Vec<i32> = north.iter().filter(|c| c.x == 9).map(|c| c.pos).collect();
        assert_eq!(at_node, vec![9, 9]);
        assert!(north
            .iter()
            .filter(|c| c.x == 9)
            .all(|c| c.slot() != BaySlot::End));
        // The real corners do end their runs.
        let corner = columns.iter().find(|c| (c.x, c.z) == (20, 0)).unwrap();
        assert_eq!(corner.slot(), BaySlot::End);
        assert!(corner.corner());
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
            condition: BuildingCondition::Normal,
            element_id: 1,
            style_seed: 1,
            window_phase: 0,
            window_archetype: WindowArchetype::Portrait,
            bays: WindowArchetype::Portrait.rhythm(),
            balcony_band: BalconyBand::Scattered,
            rustication: false,
            base_course_block: None,
            ground: GroundFloor::Plain,
            trim_block: SMOOTH_STONE,
            surrounds: false,
            shop_frame: BLACK_CONCRETE,
            recess_windows: true,
            region: BuildingRegion::NorthAmerica,
            window_frame: None,
            facade_shell: Arc::default(),
            preset_shell: Arc::default(),
        }
    }

    /// Every glass tower turns its corner on something solid, whether the glass
    /// is the wall itself or a band running round it.
    #[test]
    fn a_glass_tower_turns_its_corner_on_a_pier() {
        use crate::element_processing::building_facade::ColumnFacade;
        let corner = ColumnFacade {
            wall_u: 0,
            corner: true,
            ..Default::default()
        };
        let middle = ColumnFacade {
            wall_u: 3,
            ..Default::default()
        };

        // Walled in glass, with a glass accent: the pier falls back to concrete.
        let mut glassy = test_config(30, false, false);
        glassy.category = BuildingCategory::GlassySkyscraper;
        glassy.has_windows = false;
        glassy.wall_block = BLUE_STAINED_GLASS;
        glassy.accent_block = WHITE_STAINED_GLASS;
        assert_eq!(
            determine_wall_block_at_position_pristine(12, &glassy, corner),
            LIGHT_GRAY_CONCRETE
        );
        assert_eq!(
            determine_wall_block_at_position_pristine(12, &glassy, middle),
            BLUE_STAINED_GLASS
        );

        // A solid accent is the pier.
        glassy.accent_block = BLACKSTONE;
        assert_eq!(
            determine_wall_block_at_position_pristine(12, &glassy, corner),
            BLACKSTONE
        );

        // The party wall of a glass tower keeps its plain treatment.
        let party = ColumnFacade {
            wall_u: 0,
            corner: true,
            party: true,
            ..Default::default()
        };
        assert_eq!(
            determine_wall_block_at_position_pristine(12, &glassy, party),
            BLUE_STAINED_GLASS
        );

        // Horizontal bands: the pier runs unbroken past every storey line.
        let mut banded = test_config(30, false, false);
        banded.category = BuildingCategory::ModernSkyscraper;
        banded.use_horizontal_windows = true;
        banded.wall_block = GRAY_CONCRETE;
        banded.window_block = GLASS;
        banded.accent_block = SMOOTH_STONE;
        for h in [9, 12, 13] {
            assert_eq!(
                determine_wall_block_at_position_pristine(h, &banded, corner),
                GRAY_CONCRETE,
                "row {h} of the corner pier"
            );
        }
        assert_eq!(
            determine_wall_block_at_position_pristine(13, &banded, middle),
            GLASS,
            "the band itself is still glass"
        );

        // A windowless brick shed is not a glass tower and keeps its corners.
        let mut shed = test_config(6, false, false);
        shed.has_windows = false;
        assert_eq!(
            determine_wall_block_at_position_pristine(3, &shed, corner),
            BRICK
        );

        // Nor is a greenhouse, which is glass walled for its own reasons.
        let mut greenhouse = test_config(6, false, false);
        greenhouse.category = BuildingCategory::Greenhouse;
        greenhouse.has_windows = false;
        greenhouse.wall_block = GLASS;
        assert_eq!(
            determine_wall_block_at_position_pristine(3, &greenhouse, corner),
            GLASS
        );
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
        let at = |h: i32, u: i32| {
            determine_wall_block_at_position_pristine(
                h,
                &config,
                ColumnFacade {
                    wall_u: u,
                    ..Default::default()
                },
            )
        };
        // The attic glazes only the top row of each window column.
        assert_eq!(at(13, 1), GLASS, "attic light on floor_row 3");
        assert_eq!(at(12, 1), BRICK, "attic band is otherwise solid");
        assert_eq!(at(13, 2), BRICK, "piers stay solid");
        // Body floors keep their full windows.
        assert_eq!(at(7, 0), GLASS);
        assert_eq!(at(8, 0), GLASS);
    }

    #[test]
    fn top_treatment_shortens_windows_and_adds_band() {
        use crate::element_processing::building_facade::ColumnFacade;
        let config = test_config(14, false, true);
        let at = |h: i32, u: i32| {
            determine_wall_block_at_position_pristine(
                h,
                &config,
                ColumnFacade {
                    wall_u: u,
                    ..Default::default()
                },
            )
        };
        // Band below the treated top floor (h = height - cycle = 10, row 0).
        assert_eq!(at(10, 2), SMOOTH_STONE);
        // The top floor loses its top window row but keeps the rest.
        assert_eq!(at(13, 0), BRICK);
        assert_eq!(at(12, 0), GLASS);
        assert_eq!(at(11, 1), GLASS);
    }

    /// European windows stay two rows tall under a solid head row in a
    /// four-row storey, three on the piano nobile; elsewhere they fill it.
    #[test]
    fn european_windows_keep_a_head_row() {
        use crate::element_processing::building_facade::ColumnFacade;
        let mut config = test_config(14, false, false);
        let at = |config: &BuildingConfig, h: i32| {
            determine_wall_block_at_position_pristine(h, config, ColumnFacade::default())
        };
        // North America: rows 1-3 of the storey at h 10..13.
        assert_eq!(at(&config, 13), GLASS);
        config.region = BuildingRegion::Europe;
        assert_eq!(at(&config, 11), GLASS);
        assert_eq!(at(&config, 12), GLASS);
        assert_eq!(at(&config, 13), BRICK, "head row over a European window");
        assert_eq!(at(&config, 9), BRICK);
        config.piano_nobile = true;
        assert_eq!(at(&config, 9), GLASS, "the piano nobile is a row taller");
        assert_eq!(at(&config, 13), BRICK);
        // Bands of glass are not capped.
        config.window_archetype = WindowArchetype::Ribbon;
        config.bays = WindowArchetype::Ribbon.rhythm();
        assert_eq!(at(&config, 13), GLASS);
    }

    /// Lintels in light stone over punched windows, wall between them.
    #[test]
    fn band_rows_carry_lintels_over_windows_only() {
        use crate::element_processing::building_facade::ColumnFacade;
        let config = test_config(14, false, false);
        let band = |u: i32| {
            determine_wall_block_at_position_pristine(
                10,
                &config,
                ColumnFacade {
                    wall_u: u,
                    ..Default::default()
                },
            )
        };
        assert_eq!(band(0), config.trim_block);
        assert_eq!(band(1), config.trim_block);
        assert_eq!(band(2), BRICK);
    }

    /// A commercial street front glazes from the pavement up to eye level
    /// between dark piers, under a sign band, while its end piers and its
    /// rear wall keep the ordinary ground floor.
    #[test]
    fn a_shopfront_glazes_rows_one_to_three() {
        use crate::element_processing::building_facade::ColumnFacade;
        for cycle in [3, 4] {
            let mut config = test_config(2 + 3 * cycle, false, false);
            config.floor_cycle = cycle;
            config.ground = GroundFloor::Shopfront;
            config.base_course_block = Some(STONE_BRICKS);
            let layout = BayLayout::plan(20, SHOP_RHYTHM);
            let col = |i: i32, street: bool| ColumnFacade {
                wall_u: i,
                street,
                slot: BayLayout::plan(20, config.bays).slot(i),
                shop: layout.slot(i),
                ..Default::default()
            };
            let window = (0..20).find(|&i| layout.slot(i).is_window()).unwrap();
            let pier = (0..20).find(|&i| layout.slot(i) == BaySlot::Pier).unwrap();
            let gft = config.ground_floor_top();
            for h in 1..=3 {
                assert_eq!(
                    determine_wall_block_at_position_pristine(h, &config, col(window, true)),
                    GLASS,
                    "cycle {cycle}: shop glass at row {h}"
                );
                assert_eq!(
                    determine_wall_block_at_position_pristine(h, &config, col(pier, true)),
                    BLACK_CONCRETE
                );
            }
            assert_eq!(
                determine_wall_block_at_position_pristine(gft, &config, col(window, true)),
                BLACK_CONCRETE,
                "sign band on the top ground row"
            );
            // The end pier carries the plinth down to the pavement.
            assert_eq!(
                determine_wall_block_at_position_pristine(1, &config, col(0, true)),
                STONE_BRICKS
            );
            // Off the street the ground floor keeps its sill row.
            assert_ne!(
                determine_wall_block_at_position_pristine(1, &config, col(window, false)),
                GLASS
            );
        }
    }

    // The lattice rhythm must hold at any angle. bx + bz was constant along a
    // 135-degree wall, turning the facade into one band.
    #[test]
    fn lattice_rhythm_holds_at_any_wall_angle() {
        use crate::bresenham::bresenham_line;
        let config = test_config(14, false, false);
        // 0, 90, 45 and 135 degrees, plus shallow angles both ways.
        for &(x2, z2) in &[
            (40, 0),
            (0, 40),
            (40, 40),
            (-40, 40),
            (40, 13),
            (13, 40),
            (40, -13),
        ] {
            let axis_x = segment_axis_x(0, 0, x2, z2);
            let mut prev_u: Option<i32> = None;
            let (mut glass_run, mut max_glass_run) = (0, 0);
            for (bx, _, bz) in bresenham_line(0, 0, 0, x2, 0, z2) {
                let u = if axis_x { bx } else { bz };
                if let Some(p) = prev_u {
                    assert_eq!(
                        (u - p).abs(),
                        1,
                        "ordinate must step by one per column at {:?}",
                        (x2, z2)
                    );
                }
                prev_u = Some(u);
                if config.lattice_slot(u, config.bays).is_window() {
                    glass_run += 1;
                    max_glass_run = max_glass_run.max(glass_run);
                } else {
                    glass_run = 0;
                }
            }
            assert_eq!(
                max_glass_run,
                config.bays.window,
                "window must keep its width at {:?}",
                (x2, z2)
            );
        }
    }

    fn peak_cap_config(w: i32, l: i32) -> RoofConfig {
        RoofConfig {
            min_x: 0,
            max_x: w - 1,
            min_z: 0,
            max_z: l - 1,
            center_x: (w - 1) / 2,
            center_z: (l - 1) / 2,
            base_height: 10,
            building_height: 40,
            abs_terrain_offset: 0,
            roof_block: BRICK,
            add_dormers: false,
            element_id_for_decor: 1,
            peak_cap: None,
            max_rise: None,
            trim: RoofTrim::default(),
        }
    }

    fn cap(
        pairs: &[(&str, &str)],
        rt: RoofType,
        w: i32,
        l: i32,
        orient: Option<&str>,
    ) -> Option<i32> {
        let tags: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let nodes: Vec<ProcessedNode> = [(0, 0), (w - 1, 0), (w - 1, l - 1), (0, l - 1), (0, 0)]
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: i as u64,
                tags: HashMap::new(),
                x,
                z,
            })
            .collect();
        let ridge = ridge_frame(&nodes, w - 1, l - 1, orient, None, None);
        roof_peak_cap(&tags, rt, &peak_cap_config(w, l), &ridge, 1.0)
    }

    // roof:angle is a stated pitch, previously read by nothing at all.
    #[test]
    fn roof_angle_drives_the_ridge_rise() {
        // 45 degrees on a 20x18 gable: the ridge runs along 20, so the slope crosses 9.
        assert_eq!(
            cap(&[("roof:angle", "45")], RoofType::Gabled, 20, 18, None),
            Some(9)
        );
        // Shallower pitch, same span.
        assert_eq!(
            cap(&[("roof:angle", "30")], RoofType::Gabled, 20, 18, None),
            Some(5)
        );
        // A mapped roof:height still wins outright.
        assert_eq!(
            cap(
                &[("roof:angle", "45"), ("roof:height", "3")],
                RoofType::Gabled,
                20,
                18,
                None
            ),
            Some(3)
        );
    }

    #[test]
    fn roof_angle_uses_the_span_the_roof_actually_crosses() {
        // 20 wide x 8 long, 45 degrees. Default ridge runs along the LONG axis,
        // so the slope crosses the 8 axis: rise 4.
        assert_eq!(
            cap(&[("roof:angle", "45")], RoofType::Gabled, 20, 8, None),
            Some(4)
        );
        // `roof:orientation=across` flips the ridge onto the short axis, so the
        // slope now crosses the 20 axis: rise 10.
        assert_eq!(
            cap(
                &[("roof:angle", "45")],
                RoofType::Gabled,
                20,
                8,
                Some("across")
            ),
            Some(10)
        );
        // A skillion climbs its whole run, not a half-span, and picks the axis
        // from roof:direction. East -> the 20 axis.
        assert_eq!(
            cap(
                &[("roof:angle", "45"), ("roof:direction", "90")],
                RoofType::Skillion,
                20,
                8,
                None
            ),
            Some(20)
        );
        // Untagged direction falls back to the shorter axis, like the generator.
        assert_eq!(
            cap(&[("roof:angle", "45")], RoofType::Skillion, 20, 8, None),
            Some(8)
        );
    }

    #[test]
    fn implausible_or_irrelevant_roof_angles_are_ignored() {
        for bad in ["0", "90", "-10", "flat", ""] {
            assert_eq!(
                cap(&[("roof:angle", bad)], RoofType::Gabled, 20, 18, None),
                None,
                "roof:angle={bad:?} must not produce a rise"
            );
        }
        // Shapes that never read peak_cap as a pitch stay on their own paths.
        for rt in [
            RoofType::Flat,
            RoofType::Dome,
            RoofType::Cone,
            RoofType::Onion,
        ] {
            assert_eq!(cap(&[("roof:angle", "45")], rt, 20, 18, None), None);
        }
        // A steep pitch on a wide footprint is still bounded by the walls.
        assert_eq!(
            cap(&[("roof:angle", "80")], RoofType::Gabled, 200, 200, None),
            Some(40),
            "rise is clamped to building_height"
        );
    }

    // Each ring vertex is walked twice and the axis flips at most corners, so every
    // pass must agree on its ordinate or decorations land in front of solid wall.
    #[test]
    fn shared_ring_vertices_resolve_to_one_ordinate() {
        let rings: [&[(i32, i32)]; 6] = [
            // Axis-aligned rectangle: the axis flips at all four corners.
            &[(0, 0), (20, 0), (20, 10), (0, 10), (0, 0)],
            // Shallow/steep mix: the axis flips mid-polyline, not just at
            // right angles.
            &[(0, 0), (25, 7), (30, 30), (5, 26), (0, 0)],
            // Reflex corner.
            &[(0, 0), (18, 0), (18, 9), (9, 9), (9, 18), (0, 18), (0, 0)],
            // 45-degree ties on every edge.
            &[(0, 0), (16, 16), (32, 0), (16, -16), (0, 0)],
            // Duplicate vertex: low --scale collapses these and rotation re-rounds nodes.
            &[(0, 0), (0, 10), (0, 10), (10, 10), (0, 0)],
            // Duplicate on the seam vertex, where the walk-back has to wrap.
            &[(0, 0), (0, 0), (14, 0), (14, 8), (0, 0)],
        ];

        for ring in rings {
            let nodes: Vec<ProcessedNode> = ring
                .iter()
                .enumerate()
                .map(|(i, &(x, z))| ProcessedNode {
                    id: i as u64,
                    tags: HashMap::new(),
                    x,
                    z,
                })
                .collect();

            let mut seen: HashMap<(i32, i32), i32> = HashMap::new();
            for i in 1..nodes.len() {
                let (x1, z1) = (nodes[i - 1].x, nodes[i - 1].z);
                let (x2, z2) = (nodes[i].x, nodes[i].z);
                let seg_axis_x = segment_axis_x(x1, z1, x2, z2);
                let start_axis_x = segment_start_axis_x(&nodes, i, seg_axis_x);
                for (idx, (bx, _, bz)) in bresenham_line(x1, 0, z1, x2, 0, z2).iter().enumerate() {
                    let u = wall_ordinate(idx, start_axis_x, seg_axis_x, *bx, *bz);
                    if let Some(prev) = seen.insert((*bx, *bz), u) {
                        assert_eq!(
                            prev,
                            u,
                            "column {:?} got two ordinates on ring {ring:?}",
                            (*bx, *bz)
                        );
                    }
                }
            }
        }
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
        assert_eq!(parse_roof_type("round"), RoofType::Round);
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
        blocks.extend_from_slice(&INSTITUTIONAL_WINDOW_OPTIONS);
        blocks.extend_from_slice(&HOSPITALITY_WINDOW_OPTIONS);
        blocks.extend_from_slice(&INDUSTRIAL_WINDOW_OPTIONS);
        blocks.extend_from_slice(&TOWER_WINDOW_OPTIONS);
        blocks.extend_from_slice(&FLOOR_BLOCK_OPTIONS);
        blocks.extend(RoofMembrane::palette_blocks());
        blocks.extend_from_slice(rooftops::PROP_BLOCKS);
        for pool in REGIONAL_WALL_POOLS.iter().chain(ROOF_POOLS.iter()) {
            for &(b, _) in pool.iter() {
                blocks.extend([
                    b,
                    get_stair_block_for_material(b),
                    get_slab_block_for_material(b),
                    accent_for_wall(b, &mut element_rng(1)),
                ]);
            }
        }
        blocks.extend(RENDER_TONES.iter().map(|&(b, _)| b));
        blocks.extend(ROOF_SHADES.iter().map(|&(b, _)| b));
        // Window life: curtains, air conditioners, window boxes.
        blocks.extend_from_slice(&[
            WHITE_WOOL,
            LIGHT_GRAY_WOOL,
            SMOOTH_STONE,
            AZALEA_LEAVES,
            FLOWERING_AZALEA_LEAVES,
        ]);
        for pool in ROOF_POOLS.iter() {
            for &(b, _) in pool.iter() {
                if let Some(tone) = roof_tone_sibling(b) {
                    blocks.extend([tone.full, tone.stairs]);
                    blocks.extend(tone.slab);
                }
            }
        }
        for style in [
            DoorStyle::Oak,
            DoorStyle::Spruce,
            DoorStyle::DarkOak,
            DoorStyle::Birch,
            DoorStyle::Iron,
        ] {
            blocks.push(style.base_block());
            blocks.push(style.trapdoor_block());
        }
        // Facade trim, cut into slabs and stairs, and shopfront frames.
        blocks.extend_from_slice(&SHOP_FRAME_OPTIONS);
        blocks.extend_from_slice(&EU_SHOP_FRAME_OPTIONS);
        let walls: Vec<Block> = blocks.clone();
        for wall in walls {
            let trim = trim_block_for_wall(wall, 3);
            blocks.extend_from_slice(&[trim, trim_slab(trim), trim_stair(trim)]);
        }
        for frame in SHOP_FRAME_OPTIONS {
            blocks.push(trim_slab(frame));
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
                    RoofContext::default(),
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
        let region = BuildingRegion::Europe;
        assert_eq!(
            pick_window_archetype(cat_a, ArchEra::Unknown, region, a.wall_block, group_seed),
            pick_window_archetype(cat_b, ArchEra::Unknown, region, b.wall_block, group_seed)
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
                RoofContext::default(),
                &mut rng,
            );
            assert_eq!(style.roof_type, RoofType::Flat, "seed {seed}");
            assert!(!style.generate_roof, "seed {seed}");
        }
    }

    // Untagged flat roofs run the roof pass that lays their membrane, parapet
    // and plant; before, only presets and mapped roof shapes did.
    #[test]
    fn untagged_flat_roofs_run_the_roof_pass() {
        use crate::element_processing::building_test_support::rect_way;
        let ctx = RoofContext {
            region: BuildingRegion::NorthAmerica,
            attached: true,
            scale: 1.0,
            ..RoofContext::default()
        };
        for (seed, btype) in [
            (3u64, "yes"),
            (4, "commercial"),
            (5, "apartments"),
            (6, "office"),
        ] {
            let tags: &[(&str, &str)] = &[("building", btype), ("building:levels", "8")];
            let way = rect_way(seed * 11, 20, 20, 40, 34, tags);
            let (h, tall) = calculate_building_height(&way, btype, 0, 1.0, None, 3, 300, seed);
            let category = BuildingCategory::from_element(&way, tall, h, seed, 1.0);
            let preset = BuildingStylePreset::for_category(category);
            let mut rng = element_rng(seed);
            let style = BuildingStyle::resolve(
                &preset,
                &way,
                btype,
                category,
                ArchEra::Unknown,
                Climate::Temperate,
                DetailTier::Standard,
                h,
                true,
                300,
                seed,
                ctx,
                &mut rng,
            );
            assert_eq!(style.roof_type, RoofType::Flat, "{btype}");
            assert!(style.generate_roof, "{btype} skips the roof pass");
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
    fn detail_tier_height_score_does_not_overflow() {
        let way = ProcessedWay {
            id: 1,
            nodes: Vec::new(),
            tags: std::collections::HashMap::new(),
        };
        assert_eq!(
            compute_detail_tier(&way, BuildingCategory::House, 100, i32::MAX, false),
            DetailTier::Standard
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
    fn material_regions_split_europe_and_north_america() {
        use MaterialRegion as R;
        for (lat, lon, want) in [
            (48.872, 2.293, R::France),
            (52.538, 13.416, R::Central),
            (48.137, 11.576, R::Central),
            (51.050, 13.740, R::CentralEast),
            (51.540, -0.110, R::BritishIsles),
            (55.950, -3.190, R::BritishIsles),
            (52.373, 4.885, R::BrickNorth),
            (53.550, 10.000, R::BrickNorth),
            (59.330, 18.070, R::Nordic),
            (41.900, 12.500, R::Mediterranean),
            (45.464, 9.190, R::Mediterranean),
            (40.751, -73.985, R::NorthAmerica),
            (42.350, -71.083, R::NorthAmerica),
            (37.445, -122.148, R::NorthAmericaSunbelt),
            (19.430, -99.130, R::LatinAmerica),
            (32.720, -117.160, R::NorthAmericaSunbelt),
            (35.680, 139.690, R::Japan),
        ] {
            assert_eq!(MaterialRegion::at(lat, lon), want, "{lat},{lon}");
        }
    }

    #[test]
    fn material_regions_cover_the_world() {
        use MaterialRegion as R;
        for (lat, lon, want) in [
            // The Americas.
            (43.653, -79.383, R::Canada),
            (45.502, -73.567, R::Canada),
            (49.283, -123.121, R::Canada),
            (48.428, -123.366, R::Canada),
            (44.649, -63.575, R::Canada),
            (42.315, -83.036, R::Canada),
            (42.331, -83.046, R::NorthAmerica),
            (42.886, -78.878, R::NorthAmerica),
            (47.606, -122.332, R::NorthAmerica),
            (44.694, -75.486, R::NorthAmerica),
            (25.762, -80.192, R::NorthAmericaSunbelt),
            (24.555, -81.780, R::NorthAmericaSunbelt),
            (23.113, -82.366, R::LatinAmerica),
            (25.048, -77.355, R::LatinAmerica),
            (9.928, -84.091, R::LatinAmerica),
            (4.711, -74.072, R::LatinAmerica),
            (-23.551, -46.633, R::LatinAmerica),
            (-12.046, -77.043, R::LatinAmerica),
            (-34.603, -58.381, R::SouthernCone),
            (-33.449, -70.669, R::SouthernCone),
            (-34.901, -56.164, R::SouthernCone),
            (64.181, -51.694, R::Nordic),
            // Europe's edges.
            (41.008, 28.978, R::Mediterranean),
            (39.920, 32.854, R::Mediterranean),
            (37.983, 23.727, R::Mediterranean),
            (40.416, -3.703, R::Mediterranean),
            (44.787, 20.457, R::Central),
            (52.230, 21.011, R::Central),
            (44.426, 26.102, R::Central),
            (47.159, 27.587, R::Central),
            (60.169, 24.938, R::Nordic),
            (28.124, -15.430, R::Mediterranean),
            (55.756, 37.617, R::EastEurope),
            (59.934, 30.335, R::EastEurope),
            (50.450, 30.523, R::EastEurope),
            (53.902, 27.561, R::EastEurope),
            (56.949, 24.105, R::EastEurope),
            (59.437, 24.754, R::EastEurope),
            (47.010, 28.863, R::EastEurope),
            (43.585, 39.723, R::EastEurope),
            (55.008, 82.935, R::EastEurope),
            (43.115, 131.885, R::EastEurope),
            (51.169, 71.449, R::EastEurope),
            (47.886, 106.906, R::EastEurope),
            (41.716, 44.783, R::CentralAsia),
            (40.179, 44.499, R::CentralAsia),
            (40.409, 49.867, R::CentralAsia),
            (41.299, 69.240, R::CentralAsia),
            (43.238, 76.946, R::CentralAsia),
            // The Middle East and Africa.
            (36.806, 10.181, R::MiddleEast),
            (36.754, 3.059, R::MiddleEast),
            (33.573, -7.589, R::MiddleEast),
            (36.202, 37.134, R::MiddleEast),
            (33.894, 35.502, R::MiddleEast),
            (30.044, 31.236, R::MiddleEast),
            (25.270, 55.304, R::MiddleEast),
            (24.713, 46.675, R::MiddleEast),
            (35.689, 51.389, R::MiddleEast),
            (34.555, 69.207, R::MiddleEast),
            (2.047, 45.318, R::MiddleEast),
            (15.588, 32.534, R::MiddleEast),
            (37.066, 37.383, R::Mediterranean),
            (-1.286, 36.817, R::SubSaharanAfrica),
            (6.524, 3.379, R::SubSaharanAfrica),
            (9.030, 38.740, R::SubSaharanAfrica),
            (12.639, -8.003, R::SubSaharanAfrica),
            (-26.204, 28.047, R::SubSaharanAfrica),
            (-18.879, 47.508, R::SubSaharanAfrica),
            // Asia and the Pacific.
            (18.922, 72.833, R::SouthAsia),
            (28.614, 77.209, R::SouthAsia),
            (24.861, 67.010, R::SouthAsia),
            (34.008, 71.578, R::SouthAsia),
            (23.810, 90.413, R::SouthAsia),
            (27.717, 85.324, R::SouthAsia),
            (6.927, 79.861, R::SouthAsia),
            (13.756, 100.502, R::SoutheastAsia),
            (21.028, 105.854, R::SoutheastAsia),
            (1.352, 103.820, R::SoutheastAsia),
            (-6.208, 106.846, R::SoutheastAsia),
            (14.599, 120.984, R::SoutheastAsia),
            (16.840, 96.173, R::SoutheastAsia),
            (-18.142, 178.441, R::SoutheastAsia),
            (39.904, 116.407, R::EastAsia),
            (31.230, 121.474, R::EastAsia),
            (22.319, 114.169, R::EastAsia),
            (25.033, 121.565, R::EastAsia),
            (29.653, 91.172, R::EastAsia),
            (43.825, 87.617, R::EastAsia),
            (39.470, 75.990, R::EastAsia),
            (45.803, 126.535, R::EastAsia),
            (40.124, 124.383, R::EastAsia),
            (43.651, 111.977, R::EastAsia),
            (37.567, 126.978, R::Korea),
            (35.180, 129.075, R::Korea),
            (39.039, 125.763, R::Korea),
            (35.676, 139.650, R::Japan),
            (43.062, 141.354, R::Japan),
            (26.212, 127.681, R::Japan),
            (33.590, 130.402, R::Japan),
            (-33.868, 151.209, R::Australasia),
            (-37.814, 144.963, R::Australasia),
            (-31.950, 115.860, R::Australasia),
            (-36.848, 174.763, R::Australasia),
            (-23.698, 133.880, R::Australasia),
            (-77.846, 166.676, R::Australasia),
            (-80.0, 0.0, R::Other),
        ] {
            assert_eq!(MaterialRegion::at(lat, lon), want, "{lat},{lon}");
        }
    }

    #[test]
    fn building_regions_follow_the_material_regions() {
        for (lat, lon, want) in [
            (52.538, 13.416, BuildingRegion::Europe),
            (55.756, 37.617, BuildingRegion::Europe),
            (55.008, 82.935, BuildingRegion::Europe),
            (40.751, -73.985, BuildingRegion::NorthAmerica),
            (43.653, -79.383, BuildingRegion::NorthAmerica),
            (19.430, -99.130, BuildingRegion::Other),
            (36.806, 10.181, BuildingRegion::Other),
            (41.716, 44.783, BuildingRegion::Other),
            (35.676, 139.650, BuildingRegion::Other),
        ] {
            assert_eq!(BuildingRegion::at(lat, lon), want, "{lat},{lon}");
        }
    }

    #[test]
    fn district_seed_is_shared_within_a_cell() {
        assert_eq!(district_seed(5, 5, 1.0), district_seed(120, 90, 1.0));
        let distinct: std::collections::HashSet<u64> =
            (0..8).map(|i| district_seed(i * 128 + 3, 7, 1.0)).collect();
        assert!(distinct.len() >= 7, "neighbouring cells should differ");
        // The cell scales with the world.
        assert_eq!(district_seed(10, 10, 2.0), district_seed(250, 250, 2.0));
    }

    #[test]
    fn a_district_keeps_a_few_materials_and_is_deterministic() {
        for district in [1u64, 77, 4242, 0xDEAD_BEEF] {
            let draw = |seed: u64| {
                pick_district_wall(
                    CENTRAL_TRADITIONAL_WALLS,
                    BuildingCategory::Residential,
                    Climate::Temperate,
                    district,
                    &mut element_rng(seed),
                )
            };
            let mut counts: HashMap<Block, usize> = HashMap::new();
            for seed in 0..400u64 {
                assert_eq!(draw(seed), draw(seed));
                *counts.entry(draw(seed)).or_default() += 1;
            }
            let mut by_count: Vec<usize> = counts.values().copied().collect();
            by_count.sort_unstable_by(|a, b| b.cmp(a));
            let top5: usize = by_count.iter().take(5).sum();
            assert!(top5 >= 340, "district {district}: {counts:?}");
            // Strays still reach blocks outside the sub-palette.
            assert!(counts.len() > 5, "district {district}: no strays");
        }
    }

    #[test]
    fn regional_palettes_leave_out_fantasy_blocks() {
        const ODD: [Block; 10] = [
            NETHER_BRICK,
            RED_NETHER_BRICKS,
            END_STONE_BRICKS,
            BLACKSTONE,
            POLISHED_BLACKSTONE,
            POLISHED_BLACKSTONE_BRICKS,
            LIGHT_BLUE_TERRACOTTA,
            PURPUR_BLOCK,
            MUD,
            DEEPSLATE_BRICKS,
        ];
        for pool in REGIONAL_WALL_POOLS {
            for (b, _) in pool {
                assert!(!ODD.contains(b), "{} in a regional pool", b.name());
            }
        }
        for b in RESIDENTIAL_WALL_OPTIONS {
            assert!(!ODD.contains(&b), "{} in the residential pool", b.name());
        }
        for seed in 0..200u64 {
            let b = get_fallback_building_block(&mut element_rng(seed));
            assert!(!ODD.contains(&b), "{} as a fallback wall", b.name());
        }
    }

    #[test]
    fn untagged_walls_follow_the_region() {
        use crate::element_processing::building_test_support::rect_way;
        let way = rect_way(9, 0, 0, 12, 12, &[("building", "yes")]);
        let ctx = |materials| RoofContext {
            materials,
            district: 99,
            ..RoofContext::default()
        };
        let pick = |materials, seed| {
            determine_wall_block(
                &way,
                BuildingCategory::Default,
                ArchEra::Unknown,
                Climate::Temperate,
                12,
                &ctx(materials),
                &mut element_rng(seed),
            )
        };
        let in_pool = |pool: WallPool, b: Block| pool.iter().any(|&(p, _)| p == b);
        for seed in 0..60u64 {
            let b = pick(MaterialRegion::Central, seed);
            assert!(in_pool(CENTRAL_TRADITIONAL_WALLS, b), "{}", b.name());
            let b = pick(MaterialRegion::NorthAmerica, seed);
            assert!(in_pool(NA_TRADITIONAL_WALLS, b), "{}", b.name());
        }
        // A tall undated New York block is drawn from the civic mix.
        assert_eq!(
            wall_typology(
                BuildingCategory::Default,
                ArchEra::Unknown,
                40,
                &ctx(MaterialRegion::NorthAmerica)
            ),
            Some(WallTypology::Civic)
        );
        assert_eq!(
            wall_typology(
                BuildingCategory::Residential,
                ArchEra::PostWarPanel,
                12,
                &ctx(MaterialRegion::Central)
            ),
            Some(WallTypology::Modern)
        );
        // Industry keeps its own palette.
        assert_eq!(
            wall_typology(
                BuildingCategory::Industrial,
                ArchEra::Unknown,
                12,
                &ctx(MaterialRegion::Central)
            ),
            None
        );
    }

    #[test]
    fn a_listed_townhouse_is_not_built_of_castle_stone() {
        use crate::element_processing::building_test_support::rect_way;
        let ctx = RoofContext {
            materials: MaterialRegion::Central,
            district: 5,
            ..RoofContext::default()
        };
        let house = rect_way(
            3,
            0,
            0,
            10,
            10,
            &[("building", "yes"), ("historic", "building")],
        );
        let gate = rect_way(
            4,
            0,
            0,
            10,
            10,
            &[("building", "yes"), ("historic", "city_gate")],
        );
        for seed in 0..40u64 {
            let wall = |way: &ProcessedWay| {
                determine_wall_block(
                    way,
                    BuildingCategory::Historic,
                    ArchEra::Unknown,
                    Climate::Temperate,
                    12,
                    &ctx,
                    &mut element_rng(seed),
                )
            };
            assert!(EU_CIVIC_WALLS.iter().any(|&(b, _)| b == wall(&house)));
            assert!(HISTORIC_WALL_OPTIONS.contains(&wall(&gate)));
        }
    }

    #[test]
    fn default_glass_is_grey_and_cyan_stays_on_towers() {
        const TINTED: [Block; 4] = [
            LIGHT_BLUE_STAINED_GLASS,
            BLUE_STAINED_GLASS,
            BROWN_STAINED_GLASS,
            TINTED_GLASS,
        ];
        for pool in [
            &WINDOW_VARIATIONS[..],
            &RESIDENTIAL_WINDOW_OPTIONS[..],
            &INSTITUTIONAL_WINDOW_OPTIONS[..],
            &HOSPITALITY_WINDOW_OPTIONS[..],
            &INDUSTRIAL_WINDOW_OPTIONS[..],
            &HISTORIC_WINDOW_OPTIONS[..],
            &TOWER_WINDOW_OPTIONS[..],
        ] {
            for b in pool {
                assert!(!TINTED.contains(b), "{} in a window pool", b.name());
                if *b == CYAN_STAINED_GLASS {
                    assert!(std::ptr::eq(pool, &TOWER_WINDOW_OPTIONS[..]));
                }
            }
        }
        assert_eq!(
            window_pool_for_category(BuildingCategory::ContemporarySkyscraper),
            &TOWER_WINDOW_OPTIONS[..]
        );
        // Grey glass on a dark wall goes black.
        assert_eq!(
            coordinated_window_block(BROWN_TERRACOTTA, SMOOTH_STONE, GRAY_STAINED_GLASS),
            BLACK_STAINED_GLASS
        );
    }

    #[test]
    fn every_roof_family_has_a_matching_stair_and_slab() {
        for pool in ROOF_POOLS {
            for &(b, _) in pool {
                assert_ne!(
                    get_stair_block_for_material(b),
                    STONE_BRICK_STAIRS,
                    "{} has no stair of its own",
                    b.name()
                );
                assert_ne!(
                    get_slab_block_for_material(b),
                    STONE_BRICK_SLAB,
                    "{} has no slab of its own",
                    b.name()
                );
            }
        }
    }

    #[test]
    fn european_pitched_roofs_are_tile_or_slate_and_follow_the_district() {
        let ctx = |district| RoofContext {
            materials: MaterialRegion::Central,
            district,
            ..RoofContext::default()
        };
        let central: Vec<Block> = CENTRAL_ROOFS.iter().map(|&(b, _)| b).collect();
        for district in [3u64, 31, 313] {
            let mut counts: HashMap<Block, usize> = HashMap::new();
            for seed in 0..200u64 {
                let roof = pitched_roof_block(
                    BuildingCategory::Residential,
                    ArchEra::Unknown,
                    RoofType::Gabled,
                    WHITE_CONCRETE,
                    ctx(district),
                    seed,
                );
                assert!(central.contains(&roof), "{}", roof.name());
                *counts.entry(roof).or_default() += 1;
            }
            let (&top_roof, &top) = counts.iter().max_by_key(|(_, &n)| n).unwrap();
            assert_ne!(
                top_roof, TILE_BROWN,
                "dark tile dominates district {district}"
            );
            assert!(
                top >= 140,
                "district {district} has no dominant roof: {counts:?}"
            );
        }
        // Clay tile never lands on a wall of its own tone.
        for seed in 0..100u64 {
            let roof = pitched_roof_block(
                BuildingCategory::Residential,
                ArchEra::Unknown,
                RoofType::Gabled,
                RED_TERRACOTTA,
                ctx(seed),
                seed,
            );
            assert!(
                !matches!(roof, RED_TERRACOTTA | MANGROVE_PLANKS),
                "{}",
                roof.name()
            );
        }
        // Brick under red tile is the norm; brick under brick tile is a lump.
        for seed in 0..100u64 {
            let roof = pitched_roof_block(
                BuildingCategory::Residential,
                ArchEra::Unknown,
                RoofType::Gabled,
                BRICK,
                ctx(seed),
                seed,
            );
            assert_ne!(roof, BRICK);
        }
        // Paris quarters are zinc and slate, with the odd tile roof.
        for district in 0..40u64 {
            let paris = RoofContext {
                materials: MaterialRegion::France,
                district,
                ..RoofContext::default()
            };
            let tiles = (0..50u64)
                .filter(|&seed| {
                    let roof = pitched_roof_block(
                        BuildingCategory::Residential,
                        ArchEra::Unknown,
                        RoofType::Gabled,
                        WHITE_CONCRETE,
                        paris,
                        seed,
                    );
                    TILE_FAMILIES.contains(&roof)
                })
                .count();
            assert!(tiles <= 15, "district {district}: {tiles} of 50 tile roofs");
        }
        // A detached American `building=yes` takes house shingle, not the
        // row-house slate.
        let suburb = RoofContext {
            materials: MaterialRegion::NorthAmerica,
            district: 9,
            ..RoofContext::default()
        };
        for seed in 0..50u64 {
            let roof = pitched_roof_block(
                BuildingCategory::Default,
                ArchEra::Unknown,
                RoofType::Gabled,
                WHITE_CONCRETE,
                suburb,
                seed,
            );
            assert!(
                NA_HOUSE_ROOFS.iter().any(|&(b, _)| b == roof),
                "{}",
                roof.name()
            );
        }
        // Domes and onions are copper or slate.
        for seed in 0..50u64 {
            let roof = pitched_roof_block(
                BuildingCategory::Religious,
                ArchEra::Unknown,
                RoofType::Onion,
                WHITE_CONCRETE,
                ctx(seed),
                seed,
            );
            assert!(SPIRE_ROOFS.iter().any(|&(b, _)| b == roof));
        }
    }

    #[test]
    fn roof_tags_pick_the_family_and_the_colour_its_shade() {
        let tags = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let roof = |pairs: &[(&str, &str)], chosen: Option<Block>, seed: u64| {
            tagged_roof_block(&tags(pairs), true, chosen, &mut element_rng(seed))
        };
        for seed in 0..50u64 {
            let b = roof(&[("roof:material", "roof_tiles")], None, seed).unwrap();
            assert!(TILE_FAMILIES.contains(&b), "{}", b.name());
            // A bare tile tag keeps the tile the district chose.
            assert_eq!(
                roof(&[("roof:material", "roof_tiles")], Some(TILE_DARK), seed),
                Some(TILE_DARK)
            );
            let b = roof(&[("roof:material", "copper")], None, seed).unwrap();
            assert_eq!(b, COPPER_ROOF);
        }
        // Dresden's tile colours are tile, not copper or mud.
        for colour in ["#C97F62", "#9A6E53"] {
            let b = roof(&[("roof:colour", colour)], None, 1).unwrap();
            assert!(
                matches!(b, TILE_ORANGE | TILE_BRICK),
                "{colour}: {}",
                b.name()
            );
        }
        assert_eq!(
            roof(
                &[("roof:material", "tiles"), ("roof:colour", "#a3331e")],
                None,
                1
            ),
            Some(TILE_RED)
        );
        assert_eq!(
            roof(&[("roof:colour", "black")], None, 1),
            Some(GLAZED_BLACK)
        );
        assert_eq!(
            roof(&[("roof:colour", "green")], None, 1),
            Some(COPPER_ROOF)
        );
        assert_eq!(roof(&[], Some(TILE_RED), 1), None);
        // A vivid orange tag is still clay tile; painted metal follows its colour.
        let b = roof(&[("roof:colour", "#E96B39")], None, 1).unwrap();
        assert!(TILE_FAMILIES.contains(&b), "{}", b.name());
        assert_eq!(
            roof(
                &[("roof:material", "metal"), ("roof:colour", "#43b3ae")],
                None,
                1
            ),
            Some(COPPER_ROOF)
        );
        assert!(matches!(
            roof(&[("roof:material", "metal")], None, 1),
            Some(POLISHED_ANDESITE | SMOOTH_STONE | LIGHT_GRAY_CONCRETE | IRON_BLOCK)
        ));
    }

    #[test]
    fn pale_colour_tags_become_render_not_grey_stone() {
        for (tag, want) in [
            (
                (187, 171, 148),
                [WHITE_TERRACOTTA, BIRCH_PLANKS, SMOOTH_SANDSTONE],
            ),
            (
                (255, 255, 255),
                [WHITE_CONCRETE, WHITE_CONCRETE, WHITE_CONCRETE],
            ),
            (
                (245, 245, 220),
                [SMOOTH_SANDSTONE, WHITE_CONCRETE, MUSHROOM_STEM],
            ),
        ] {
            let b = wall_block_for_colour_tag(tag, &mut element_rng(1));
            assert!(want.contains(&b), "{tag:?}: {}", b.name());
        }
    }

    #[test]
    fn wall_variety_stays_within_the_tone() {
        assert_eq!(substitute_pool_only(GRAY_CONCRETE), &[GRAY_CONCRETE_POWDER]);
        for b in substitute_pool_only(BRICK) {
            assert!(matches!(*b, POLISHED_GRANITE | GRANITE));
        }
        for b in [
            WHITE_TERRACOTTA,
            MUSHROOM_STEM,
            OAK_PLANKS,
            LIGHT_GRAY_TERRACOTTA,
        ] {
            assert!(substitute_pool_only(b).is_empty(), "{}", b.name());
        }
    }

    #[test]
    fn window_life_gates_by_building_kind() {
        let tags = HashMap::new();
        let mut street = FacadePlan::empty();
        street.has_any_street = true;
        // A pre-war walk-up of five storeys in New York.
        let mut walk_up = test_config(2 + 5 * 4, false, false);
        walk_up.category = BuildingCategory::Residential;
        assert!(has_air_conditioners(&walk_up, &tags, &street));
        assert!(!has_air_conditioners(&walk_up, &tags, &FacadePlan::empty()));
        for region in [BuildingRegion::Europe, BuildingRegion::Other] {
            let c = BuildingConfig {
                region,
                ..walk_up.clone()
            };
            assert!(!has_air_conditioners(&c, &tags, &street));
        }
        for category in [
            BuildingCategory::Office,
            BuildingCategory::Commercial,
            BuildingCategory::House,
            BuildingCategory::Hotel,
            BuildingCategory::Industrial,
            BuildingCategory::GlassySkyscraper,
            BuildingCategory::Default,
        ] {
            let c = BuildingConfig {
                category,
                ..walk_up.clone()
            };
            assert!(
                !has_air_conditioners(&c, &tags, &street),
                "air conditioners on {category:?}"
            );
        }
        // A generic building mapped as homes counts.
        let homes: HashMap<String, String> =
            std::iter::once(("building:use".to_string(), "residential".to_string())).collect();
        let generic = BuildingConfig {
            category: BuildingCategory::Default,
            ..walk_up.clone()
        };
        assert!(has_air_conditioners(&generic, &homes, &street));
        for (height, ok) in [(2 + 3 * 4, false), (2 + 4 * 4, true), (2 + 9 * 4, false)] {
            let c = BuildingConfig {
                building_height: height,
                ..walk_up.clone()
            };
            assert_eq!(has_air_conditioners(&c, &tags, &street), ok, "{height}");
        }
        for (era, ok) in [
            (ArchEra::TraditionalPreWar, true),
            (ArchEra::PostWarPanel, false),
            (ArchEra::Contemporary, false),
        ] {
            let c = BuildingConfig {
                era,
                ..walk_up.clone()
            };
            assert_eq!(has_air_conditioners(&c, &tags, &street), ok, "{era:?}");
        }
        let furnished = BuildingConfig {
            recess_windows: false,
            ..walk_up.clone()
        };
        assert!(!has_air_conditioners(&furnished, &tags, &street));

        // Window boxes: pre-war European dwellings only.
        let town_house = BuildingConfig {
            region: BuildingRegion::Europe,
            era: ArchEra::TraditionalPreWar,
            ..walk_up.clone()
        };
        assert!(has_window_boxes(&town_house, &tags));
        for c in [
            BuildingConfig {
                region: BuildingRegion::NorthAmerica,
                ..town_house.clone()
            },
            BuildingConfig {
                era: ArchEra::Unknown,
                ..town_house.clone()
            },
            BuildingConfig {
                era: ArchEra::Contemporary,
                ..town_house.clone()
            },
            BuildingConfig {
                category: BuildingCategory::Office,
                ..town_house.clone()
            },
            BuildingConfig {
                category: BuildingCategory::Hotel,
                ..town_house.clone()
            },
        ] {
            assert!(!has_window_boxes(&c, &tags));
        }
    }

    #[test]
    fn curtains_hang_across_whole_windows() {
        // Closed covers every cell, a blind only the top row.
        for k in 0..3 {
            assert!(curtain_covers(CurtainDraw::Closed, k, 3, false));
            assert!(!curtain_covers(CurtainDraw::Blind, k, 3, false));
            assert!(curtain_covers(CurtainDraw::Blind, k, 3, true));
        }
        // Drawn back to the jambs of a wide window, to one side of a pair.
        assert!(curtain_covers(CurtainDraw::Open, 0, 3, false));
        assert!(!curtain_covers(CurtainDraw::Open, 1, 3, false));
        assert!(curtain_covers(CurtainDraw::Open, 2, 3, false));
        assert!(curtain_covers(CurtainDraw::Open, 0, 2, false));
        assert!(!curtain_covers(CurtainDraw::Open, 1, 2, false));
        assert!(!curtain_covers(CurtainDraw::Open, 0, 1, false));

        // Every column and row of one window shares one curtain.
        let config = BuildingConfig {
            category: BuildingCategory::Residential,
            ..test_config(26, false, false)
        };
        let way = crate::element_processing::building_test_support::rect_way(7, 0, 0, 30, 12, &[]);
        let columns = facade_columns(&way.nodes, &config);
        let mut seen: HashMap<(usize, i32, i32), Option<(Block, CurtainDraw)>> = HashMap::new();
        let mut hung = 0;
        for wc in &columns {
            let BaySlot::Window { bay, .. } = wc.slot() else {
                continue;
            };
            for storey in 0..6 {
                let curtain = window_curtain(&config, wc, storey);
                hung += usize::from(curtain.is_some());
                let first = *seen.entry((wc.run, bay, storey)).or_insert(curtain);
                assert_eq!(first, curtain, "one window, two curtains");
            }
        }
        assert!(hung > 0);
    }

    #[test]
    fn roof_tone_siblings_keep_tone_and_shape() {
        for pool in ROOF_POOLS {
            for &(roof, _) in pool.iter() {
                let Some(tone) = roof_tone_sibling(roof) else {
                    continue;
                };
                let stairs = get_stair_block_for_material(roof);
                assert_ne!(
                    tone.stairs,
                    stairs,
                    "{} has no visible sibling",
                    roof.name()
                );
                if roof_slab_for_stair(stairs).is_some() {
                    assert!(tone.slab.is_some(), "{} sibling lacks a slab", roof.name());
                }
            }
        }
        // Sheet metal and the tiles with no close sibling stay uniform.
        for roof in [ZINC, COPPER_ROOF, TILE_DARK, TILE_BROWN] {
            assert!(roof_tone_sibling(roof).is_none(), "{}", roof.name());
        }
        // A few small patches, not a speckle.
        let cells = 200 * 200;
        let patched = (0..200)
            .flat_map(|x| (0..200).map(move |z| (x, z)))
            .filter(|&(x, z)| roof_tone_patch(x, z, 99))
            .count();
        assert!(
            (cells * 4 / 100..=cells * 12 / 100).contains(&patched),
            "{patched} of {cells} cells patched"
        );
        let paired = (0..200)
            .flat_map(|x| (0..200).map(move |z| (x, z)))
            .filter(|&(x, z)| roof_tone_patch(x, z, 99) && roof_tone_patch(x ^ 1, z, 99))
            .count();
        assert!(paired * 2 >= patched, "patches are single cells");
    }

    /// A rectangular dwelling's wall ring, window dressing and window life,
    /// built alone, and the blocks within its bounding box.
    fn window_life_world(config: &BuildingConfig) -> Vec<((i32, i32, i32), Block)> {
        use crate::coordinate_system::cartesian::XZBBox;
        use crate::element_processing::building_test_support::{rect_way, test_editor};
        use clap::Parser as _;
        let args = Args::parse_from([
            "arnis",
            "--bbox",
            "1,2,3,4",
            "--mode",
            "geo-only",
            "--ground-level",
            "0",
        ]);
        let xz = XZBBox::rect_from_xz_lengths(50.0, 50.0).unwrap();
        let mut editor = test_editor(&xz);
        let way = rect_way(config.element_id, 5, 5, 36, 20, &[]);
        let floor: FnvHashSet<(i32, i32)> =
            (6..36).flat_map(|x| (6..20).map(move |z| (x, z))).collect();
        let passages = CoordinateBitmap::new_empty();
        let facade = FacadePlan::empty();
        build_wall_ring(
            &mut editor,
            &way.nodes,
            config,
            &args,
            false,
            &passages,
            &facade,
            Some(&floor),
        );
        generate_window_frames(&mut editor, &way, config, &passages, &facade, None);
        generate_window_life(
            &mut editor,
            &way,
            config,
            &passages,
            &facade,
            None,
            Some(&floor),
            None,
        );
        let mut out = Vec::new();
        for x in 0..42 {
            for z in 0..26 {
                for y in -2..40 {
                    if let Some(b) = editor.get_block_absolute(x, y, z) {
                        out.push(((x, y, z), b));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn curtains_only_hang_inside_behind_recessed_glass() {
        let is_curtain = |b: Block| b == WHITE_WOOL || b == LIGHT_GRAY_WOOL;
        let mut hung = 0;
        for id in 1..6u64 {
            let config = BuildingConfig {
                category: BuildingCategory::Residential,
                element_id: id,
                ..test_config(2 + 4 * 4, false, false)
            };
            let world = window_life_world(&config);
            let at: HashMap<(i32, i32, i32), Block> = world.iter().copied().collect();
            for &((x, y, z), b) in &world {
                if !is_curtain(b) {
                    continue;
                }
                hung += 1;
                // Two blocks in from a wall face: glass between, the
                // recess open in front.
                assert!(
                    (7..=34).contains(&x) && (7..=18).contains(&z),
                    "curtain outside the room at {x},{y},{z}"
                );
                let behind_glass = ROOF_SIDES.iter().any(|&(dx, dz)| {
                    at.get(&(x + dx, y, z + dz)) == Some(&config.window_block)
                        && !at.contains_key(&(x + 2 * dx, y, z + 2 * dz))
                });
                assert!(behind_glass, "curtain not behind glass at {x},{y},{z}");
            }
        }
        assert!(hung > 0, "no curtain on five dwellings");

        // Not with a furnished interior, nor on offices, glass towers or works.
        let base = BuildingConfig {
            category: BuildingCategory::Residential,
            ..test_config(2 + 4 * 4, false, false)
        };
        for config in [
            BuildingConfig {
                recess_windows: false,
                ..base.clone()
            },
            BuildingConfig {
                category: BuildingCategory::Office,
                ..base.clone()
            },
            BuildingConfig {
                category: BuildingCategory::GlassySkyscraper,
                ..base.clone()
            },
            BuildingConfig {
                category: BuildingCategory::Industrial,
                ..base.clone()
            },
            BuildingConfig {
                category: BuildingCategory::Commercial,
                ..base.clone()
            },
        ] {
            for id in 1..6u64 {
                let config = BuildingConfig {
                    element_id: id,
                    ..config.clone()
                };
                assert!(
                    window_life_world(&config)
                        .iter()
                        .all(|&(_, b)| !is_curtain(b) && b != FLOWERING_AZALEA_LEAVES),
                    "window life on {:?}",
                    config.category
                );
            }
        }
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

    /// `flat_args` at a chosen world scale. The facade export applies as a
    /// block grid from two blocks per metre up, which is where the shell is.
    fn scaled_args(scale: &str) -> Args {
        Args::parse_from([
            "arnis",
            "--bbox",
            "1,2,3,4",
            "--mode",
            "geo-only",
            "--ground-level",
            "0",
            "--scale",
            scale,
        ])
    }

    fn run_building(
        editor: &mut WorldEditor,
        way: &ProcessedWay,
        road: &CoordinateBitmap,
        footprints: &CoordinateBitmap,
    ) {
        run_building_with(editor, way, road, footprints, &flat_args());
    }

    fn run_building_with(
        editor: &mut WorldEditor,
        way: &ProcessedWay,
        road: &CoordinateBitmap,
        footprints: &CoordinateBitmap,
        args: &Args,
    ) {
        let cache = FloodFillCache::new();
        let passages = CoordinateBitmap::new_empty();
        let groups: FnvHashMap<u64, Vec<u64>> = FnvHashMap::default();
        let ctx = BuildingContext {
            flood_fill_cache: &cache,
            building_passages: &passages,
            road_mask: road,
            building_footprints: footprints,
            group_members: &groups,
            interior_uses:
                crate::element_processing::subprocessor::interior::InteriorUseIndex::empty(),
        };
        generate_buildings(editor, way, args, None, None, &ctx, way.id);
    }

    /// Every column's highest block over an 80 x 80 area.
    fn column_tops(editor: &WorldEditor) -> Vec<Option<i32>> {
        (0..80)
            .flat_map(|x| (0..80).map(move |z| (x, z)))
            .map(|(x, z)| editor.highest_block_between(x, z, -64, 300))
            .collect()
    }

    // A New York mid-rise without roof tags gets a membrane, a parapet and
    // plant: the plant stands on its own roof, clear of the parapet, and is
    // the same on every run.
    #[test]
    fn untagged_flat_roof_is_fitted_out_inside_its_footprint() {
        use crate::coordinate_system::geographic::LLBBox;
        use crate::element_processing::building_test_support::test_editor_at;
        let xz = XZBBox::rect_from_xz_lengths(80.0, 80.0).unwrap();
        let road = CoordinateBitmap::new(&xz);
        let footprints = CoordinateBitmap::new(&xz);
        let nyc = LLBBox::new(40.70, -74.02, 40.71, -74.01).unwrap();
        let (x0, z0, x1, z1) = (20, 20, 51, 43);
        let run = || {
            let way = rect_way(
                4711,
                x0,
                z0,
                x1,
                z1,
                &[("building", "apartments"), ("building:levels", "8")],
            );
            let mut editor = test_editor_at(&xz, nyc);
            run_building(&mut editor, &way, &road, &footprints);
            editor
        };
        let editor = run();
        assert_eq!(
            column_tops(&editor),
            column_tops(&run()),
            "the fit-out is deterministic"
        );

        // The bare roof is the commonest top over the footprint.
        let mut counts: HashMap<i32, usize> = HashMap::new();
        for x in x0..=x1 {
            for z in z0..=z1 {
                if let Some(y) = editor.highest_block_between(x, z, -64, 300) {
                    *counts.entry(y).or_default() += 1;
                }
            }
        }
        let surface = counts.into_iter().max_by_key(|&(_, n)| n).unwrap().0;
        let inside = |x: i32, z: i32| (x0..=x1).contains(&x) && (z0..=z1).contains(&z);
        let ring_in = |x: i32, z: i32, k: i32| {
            inside(x, z)
                && (x == x0 + k || x == x1 - k || z == z0 + k || z == z1 - k)
                && (x0 + k..=x1 - k).contains(&x)
                && (z0 + k..=z1 - k).contains(&z)
        };
        let (mut parapet, mut ring, mut plant) = (0, 0, 0);
        for x in 0..80 {
            for z in 0..80 {
                let above = editor.highest_block_between(x, z, surface + 1, 300);
                if !inside(x, z) {
                    assert!(
                        above.is_none(),
                        "a block above the roof off the footprint at {x},{z}"
                    );
                } else if ring_in(x, z, 0) {
                    ring += 1;
                    parapet += usize::from(above.is_some());
                } else if ring_in(x, z, 1) {
                    assert!(above.is_none(), "plant against the parapet at {x},{z}");
                } else {
                    plant += usize::from(above.is_some());
                    let roof = editor.get_block_absolute(x, surface, z).unwrap();
                    assert!(
                        !matches!(roof, OAK_PLANKS | MUD_BRICKS),
                        "a floor block as roof at {x},{z}"
                    );
                }
            }
        }
        assert!(
            parapet * 10 >= ring * 9,
            "parapet on {parapet} of {ring} edge cells"
        );
        assert!(plant >= 12, "only {plant} plant columns on the roof");
    }

    // A roof-only canopy mapped `roof:shape=round` is a barrel vault: level
    // along the ridge, curved across it. A dome falls away in both directions.
    #[test]
    fn a_round_canopy_vaults_instead_of_doming() {
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let road = CoordinateBitmap::new(&xz);
        let footprints = CoordinateBitmap::new(&xz);
        let way = rect_way(
            77,
            10,
            20,
            39,
            31,
            &[("building", "roof"), ("roof:shape", "round")],
        );
        let mut editor = test_editor(&xz);
        run_building(&mut editor, &way, &road, &footprints);

        let top = |e: &WorldEditor, x: i32, z: i32| e.highest_block_between(x, z, 0, 200);
        let crown_mid = top(&editor, 25, 25).expect("a vault over the middle");
        // Level along the ridge (the 30-cell axis).
        for x in [15, 20, 30, 35] {
            assert_eq!(
                top(&editor, x, 25),
                Some(crown_mid),
                "the ridge is level at x={x}"
            );
        }
        // Curved across it: the flank sits below the crown.
        let flank = top(&editor, 25, 21).expect("a vault over the flank");
        assert!(
            flank < crown_mid,
            "flank {flank} should sit below crown {crown_mid}"
        );
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
        // The two facade tests install a process-global picture source while
        // they run, and a dressed ring has no windows at all, so this one has
        // to take the same lock rather than race them.
        let _guard = crate::mapillary::facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
        // Glass may sit in the wall plane or a block behind it.
        let east_has_glass =
            (21..32).any(|z| (2..=9).any(|y| glass(&editor, 40, z, y) || glass(&editor, 39, z, y)));
        let west_has_glass =
            (21..32).any(|z| (2..=9).any(|y| glass(&editor, 20, z, y) || glass(&editor, 21, z, y)));
        assert!(
            !east_has_glass,
            "party wall must not glaze into the neighbor"
        );
        assert!(west_has_glass, "free wall keeps its windows");
    }

    #[test]
    fn mansard_and_gambrel_roofs_build_above_the_walls() {
        const SLATE: &[Block] = &[COBBLED_DEEPSLATE, DEEPSLATE_TILES];
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

    /// Every block the residential window pass can leave on a wall: the
    /// category's own pool plus the dark-wall substitution.
    const WINDOW_BLOCKS: &[Block] = &[
        GLASS,
        LIGHT_GRAY_STAINED_GLASS,
        GRAY_STAINED_GLASS,
        BLACK_STAINED_GLASS,
    ];

    /// Whether the wall column at `(x, z)` has a window, in the wall plane or
    /// set back a block behind an opening in it. Glass behind solid wall
    /// belongs to the window of the wall round the corner.
    fn column_has_window(editor: &WorldEditor, x: i32, z: i32) -> bool {
        let glass = |x: i32, y: i32, z: i32| {
            editor
                .get_block_absolute(x, y, z)
                .is_some_and(|b| WINDOW_BLOCKS.iter().any(|w| w.id() == b.id()))
        };
        (1..80).any(|y| {
            glass(x, y, z)
                || (editor.get_block_absolute(x, y, z).is_none()
                    && [(1, 0), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .any(|(dx, dz)| glass(x + dx, y, z + dz)))
        })
    }

    /// Only the walls a photograph covers are built as a plain shell. The rest
    /// of the ring stays a normal Arnis building, and the corner column where
    /// the two meet carries neither a gap nor a window of its own.
    #[test]
    fn only_the_photographed_wall_loses_its_windows() {
        let _guard = crate::mapillary::facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        const WAY: u64 = 4711;
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        // The street runs south of the building, so the synthetic entrance
        // lands on the wall without a photograph and cannot be mistaken for a
        // hole in the shell.
        let road = bitmap_with_rect(&xz, 0, 38, 59, 40);
        let footprints = CoordinateBitmap::new(&xz);
        let way = rect_way(
            WAY,
            20,
            20,
            40,
            32,
            &[
                ("building", "apartments"),
                ("building:levels", "4"),
                ("roof:shape", "flat"),
            ],
        );
        let args = scaled_args("2");

        // The same building twice: once with one photographed wall, the north
        // edge from node (20, 20) to node (40, 20), and once with no export at
        // all, which is what the other three walls have to end up looking like.
        let elements = vec![crate::osm_parser::ProcessedElement::Way(way.clone())];
        crate::mapillary::facades::install_wall_blocks_for_test(
            // 11 metre columns over the edge's 21 blocks at two blocks per
            // metre, and 20 metres of rows, taller than the building, so every
            // wall row of it has a cell.
            &[(WAY, WAY * 100, WAY * 100 + 1, 11, 20)],
            &elements,
            &xz,
            2.0,
        );
        // The export numbers its columns in metres, so the block at the far
        // end of the edge falls exactly on column 11 of 11 and carries no
        // cell. That is the corner the seam runs through.
        assert!(crate::mapillary::facades::photo_column(20, 20, WAY));
        assert!(!crate::mapillary::facades::photo_column(40, 20, WAY));
        let mut facaded = test_editor(&xz);
        run_building_with(&mut facaded, &way, &road, &footprints, &args);
        crate::mapillary::facades::clear();
        let mut plain = test_editor(&xz);
        run_building_with(&mut plain, &way, &road, &footprints, &args);

        // Without the export this building glazes both walls, so the north
        // wall losing its windows is the shell and not the style.
        assert!(
            (21..40).any(|x| column_has_window(&plain, x, 20)),
            "the test building has to glaze its north wall without an export"
        );
        assert!(
            (21..40).any(|x| column_has_window(&plain, x, 32)),
            "and its south wall"
        );

        for x in 20..=40 {
            assert!(
                !column_has_window(&facaded, x, 20),
                "column {x} of the photographed wall kept a procedural window"
            );
        }
        assert!(
            (21..40).any(|x| column_has_window(&facaded, x, 32)),
            "the wall with no photograph must still be a normal building"
        );

        // The photograph reaches the wall: its cells are one flat colour, so
        // the block they resolve to has to be on that wall and nowhere else.
        let shell =
            crate::block_palette::facade_block_for_color(crate::mapillary::facades::TEST_WALL_RGB);
        let carries_shell = |e: &WorldEditor, x: i32, z: i32| {
            (1..80).any(|y| {
                e.get_block_absolute(x, y, z)
                    .is_some_and(|b| b.id() == shell.id())
            })
        };
        assert!(carries_shell(&facaded, 30, 20), "no texture on the wall");
        assert!(
            !carries_shell(&facaded, 30, 32),
            "the texture must not reach the wall it was not taken of"
        );

        // The seam. The corner the projection drops is still built as shell,
        // so a photographed wall cannot end in a lone procedural window, and
        // the column two ring segments both write is solid from the ground to
        // the roof line whichever of them wrote it last.
        for &(x, z) in &[(20, 20), (40, 20)] {
            let top = (1..80)
                .rev()
                .find(|&y| facaded.get_block_absolute(x, y, z).is_some())
                .expect("corner column is empty");
            assert!(
                (1..=top).all(|y| facaded.get_block_absolute(x, y, z).is_some()),
                "gap in the corner column at ({x}, {z})"
            );
        }
    }

    /// A preset facade covers a building's whole ring, so the whole ring is
    /// built as a plain shell, which is the same flattening a photographed
    /// wall gets and for the same reason: the panel hangs a fraction of a
    /// block in front of the wall, so a sill, a quoin or a cornice stands
    /// between the viewer and the picture. Measured on Leipzig, this is the
    /// difference between a fifth of the average panel being hidden by the
    /// building's own blocks and a tenth.
    #[test]
    fn a_preset_facade_flattens_the_whole_ring() {
        let _guard = crate::mapillary::facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let road = bitmap_with_rect(&xz, 0, 38, 59, 40);
        let footprints = CoordinateBitmap::new(&xz);
        let way = rect_way(
            4712,
            20,
            20,
            40,
            32,
            &[
                ("building", "apartments"),
                ("building:levels", "4"),
                ("roof:shape", "flat"),
            ],
        );
        let args = scaled_args("1");

        // The same building twice: once with no preset set, which is what the
        // dressed one has to stop looking like, and once with one installed.
        crate::building_facades::reset(false, None, 16, 1.0);
        let mut plain = test_editor(&xz);
        run_building_with(&mut plain, &way, &road, &footprints, &args);

        let dir = tempfile::tempdir().unwrap();
        // An empty file is enough: the loader only checks that the picture the
        // manifest names is there, and the panels are never hung here because
        // the test editor writes no hanging entities.
        std::fs::write(dir.path().join("wall.png"), b"").unwrap();
        std::fs::write(
            dir.path()
                .join(crate::building_facades::manifest::MANIFEST_NAME),
            br#"{"version": 1, "textures": [{"file": "wall.png",
                 "categories": ["Default"], "metres_wide": 20.0,
                 "metres_tall": 12.0, "storeys": 4,
                 "tiles_horizontally": true, "has_ground_floor": true}]}"#,
        )
        .unwrap();
        crate::building_facades::reset(true, Some(dir.path()), 16, 1.0);
        assert!(crate::building_facades::enabled());
        let mut dressed = test_editor(&xz);
        // The shell is only as wide as the panels are, and a world that hangs
        // no display entities gets no panels, so the flattening has to be asked
        // for on an editor that would hang them.
        dressed.set_map_decals(true);
        run_building_with(&mut dressed, &way, &road, &footprints, &args);
        crate::building_facades::reset(false, None, 16, 1.0);

        // Windows first, so a shell that flattened nothing cannot pass.
        assert!(
            (21..40).any(|x| column_has_window(&plain, x, 20)),
            "the test building has to glaze its walls without a preset set"
        );
        for x in 20..=40 {
            for z in [20, 32] {
                assert!(
                    !column_has_window(&dressed, x, z),
                    "column ({x}, {z}) of a dressed ring kept a procedural window"
                );
            }
        }
        for z in 20..=32 {
            for x in [20, 40] {
                assert!(
                    !column_has_window(&dressed, x, z),
                    "column ({x}, {z}) of a dressed ring kept a procedural window"
                );
            }
        }

        // And nothing of the building's own stands in the cells the panels
        // hang in, all the way round, below the roof line.
        let outside = |e: &WorldEditor| {
            let mut n = 0;
            for y in 1..=10 {
                for x in 19..=41 {
                    n += usize::from(e.get_block_absolute(x, y, 19).is_some());
                    n += usize::from(e.get_block_absolute(x, y, 33).is_some());
                }
                for z in 19..=33 {
                    n += usize::from(e.get_block_absolute(19, y, z).is_some());
                    n += usize::from(e.get_block_absolute(41, y, z).is_some());
                }
            }
            n
        };
        assert!(
            outside(&plain) > 0,
            "the test building has to stick something out without a preset set"
        );
        assert_eq!(
            outside(&dressed),
            0,
            "a dressed ring left {} blocks standing in front of its panels",
            outside(&dressed)
        );
    }

    /// Writes a one-texture preset set into `dir` and switches it on.
    fn install_preset_set(dir: &std::path::Path) {
        // An empty file is enough for the loader, which only checks that the
        // picture the manifest names is there.
        std::fs::write(dir.join("wall.png"), b"").unwrap();
        std::fs::write(
            dir.join(crate::building_facades::manifest::MANIFEST_NAME),
            br#"{"version": 1, "textures": [{"file": "wall.png",
                 "categories": ["Default"], "metres_wide": 20.0,
                 "metres_tall": 12.0, "storeys": 4,
                 "tiles_horizontally": true, "has_ground_floor": true}]}"#,
        )
        .unwrap();
        crate::building_facades::reset(true, Some(dir), 16, 1.0);
        assert!(crate::building_facades::enabled());
    }

    /// A wall the presets hang nothing on is built exactly as it is with the
    /// feature off.
    ///
    /// The courtyard ring of a building with a hole is the case that shows it:
    /// `building_facades::collect` claims a building once, from its outer ring,
    /// so the inner ring never records a candidate and no panel is ever hung
    /// there. Flattening it anyway costs the courtyard every window, plinth and
    /// accent line it would have had and gives it nothing back, which is how a
    /// building ends up a featureless box of its own wall material.
    #[test]
    fn a_courtyard_ring_the_presets_never_reach_keeps_its_windows() {
        let _guard = crate::mapillary::facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let xz = XZBBox::rect_from_xz_lengths(70.0, 70.0).unwrap();
        let road = bitmap_with_rect(&xz, 0, 52, 69, 54);
        let footprints = CoordinateBitmap::new(&xz);
        let way = rect_way(
            4713,
            16,
            16,
            48,
            48,
            &[
                ("building", "apartments"),
                ("building:levels", "4"),
                ("roof:shape", "flat"),
            ],
        );
        let hole = [HolePolygon {
            way: rect_way(4714, 26, 26, 38, 38, &[]),
            add_walls: true,
        }];
        let args = scaled_args("1");

        let build = |editor: &mut WorldEditor| {
            let cache = FloodFillCache::new();
            let passages = CoordinateBitmap::new_empty();
            let groups: FnvHashMap<u64, Vec<u64>> = FnvHashMap::default();
            let ctx = BuildingContext {
                flood_fill_cache: &cache,
                building_passages: &passages,
                road_mask: &road,
                building_footprints: &footprints,
                group_members: &groups,
                interior_uses:
                    crate::element_processing::subprocessor::interior::InteriorUseIndex::empty(),
            };
            generate_buildings(editor, &way, &args, None, Some(&hole), &ctx, way.id);
        };

        crate::building_facades::reset(false, None, 16, 1.0);
        let mut plain = test_editor(&xz);
        plain.set_map_decals(true);
        build(&mut plain);

        let dir = tempfile::tempdir().unwrap();
        install_preset_set(dir.path());
        let mut dressed = test_editor(&xz);
        dressed.set_map_decals(true);
        build(&mut dressed);
        crate::building_facades::reset(false, None, 16, 1.0);

        // The courtyard walls, whose four corners are the hole's.
        let courtyard: Vec<(i32, i32)> = (26..=38)
            .flat_map(|i| [(i, 26), (i, 38), (26, i), (38, i)])
            .collect();
        let glazed = |e: &WorldEditor| {
            courtyard
                .iter()
                .filter(|&&(x, z)| column_has_window(e, x, z))
                .count()
        };
        assert!(
            glazed(&plain) > 0,
            "the test courtyard has to glaze its walls without a preset set"
        );
        assert_eq!(
            glazed(&dressed),
            glazed(&plain),
            "the courtyard ring lost windows to a photograph that never comes"
        );

        // And the outer ring, which does get panels, is still flattened.
        assert!(
            (17..48).any(|x| column_has_window(&plain, x, 16)),
            "the test building has to glaze its outer wall without a preset set"
        );
        assert!(
            !(16..=48).any(|x| column_has_window(&dressed, x, 16)),
            "the photographed outer wall kept a procedural window"
        );
    }

    #[test]
    fn street_aware_generation_is_deterministic() {
        // The two facade tests install a process-global picture source while
        // they run, and a dressed ring has no windows at all, so this one has
        // to take the same lock rather than race them.
        let _guard = crate::mapillary::facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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

    /// Blocks around a rectangular `x0..=x1` by `z0..=z1` building after a
    /// full run, with a street along its north side.
    fn dwelling_world(
        llbbox: crate::coordinate_system::geographic::LLBBox,
        id: u64,
        tags: &[(&str, &str)],
    ) -> HashMap<(i32, i32, i32), Block> {
        use crate::element_processing::building_test_support::test_editor_at;
        let xz = XZBBox::rect_from_xz_lengths(70.0, 60.0).unwrap();
        let (x0, z0, x1, z1) = (10, 20, 45, 36);
        let road = bitmap_with_rect(&xz, 0, z0 - 5, 69, z0 - 2);
        let footprints = bitmap_with_rect(&xz, x0, z0, x1, z1);
        let way = rect_way(id, x0, z0, x1, z1, tags);
        let mut editor = test_editor_at(&xz, llbbox);
        run_building(&mut editor, &way, &road, &footprints);
        let mut out = HashMap::new();
        for x in 0..70 {
            for z in 0..60 {
                for y in -2..60 {
                    if let Some(b) = editor.get_block_absolute(x, y, z) {
                        out.insert((x, y, z), b);
                    }
                }
            }
        }
        out
    }

    fn nyc() -> crate::coordinate_system::geographic::LLBBox {
        crate::coordinate_system::geographic::LLBBox::new(40.70, -74.02, 40.71, -74.01).unwrap()
    }

    fn berlin() -> crate::coordinate_system::geographic::LLBBox {
        crate::coordinate_system::geographic::LLBBox::new(52.53, 13.41, 52.54, 13.42).unwrap()
    }

    fn is_glass(b: Block) -> bool {
        b.name().ends_with("glass")
    }

    /// Air conditioners: smooth stone in a wall face with glass behind it.
    fn air_conditioners(world: &HashMap<(i32, i32, i32), Block>) -> Vec<(i32, i32, i32)> {
        let ring = |x: i32, z: i32| {
            ((10..=45).contains(&x) && (z == 20 || z == 36))
                || ((20..=36).contains(&z) && (x == 10 || x == 45))
        };
        world
            .iter()
            .filter(|&(&(x, _, z), &b)| b == SMOOTH_STONE && ring(x, z))
            .filter(|&(&(x, y, z), _)| {
                ROOF_SIDES.iter().any(|&(dx, dz)| {
                    world
                        .get(&(x + dx, y, z + dz))
                        .is_some_and(|&b| is_glass(b))
                })
            })
            .map(|(&c, _)| c)
            .collect()
    }

    /// Window boxes: azalea leaves a block out from the wall.
    fn window_boxes(world: &HashMap<(i32, i32, i32), Block>) -> Vec<(i32, i32, i32)> {
        world
            .iter()
            .filter(|&(_, &b)| b == AZALEA_LEAVES || b == FLOWERING_AZALEA_LEAVES)
            .map(|(&c, _)| c)
            .collect()
    }

    #[test]
    fn air_conditioners_only_in_north_american_walk_ups() {
        let walk_up: &[(&str, &str)] = &[
            ("building", "apartments"),
            ("building:levels", "6"),
            ("start_date", "1910"),
        ];
        let mut units = 0;
        for id in 300..306u64 {
            let world = dwelling_world(nyc(), id, walk_up);
            for (x, y, z) in air_conditioners(&world) {
                units += 1;
                // Upper floors of the street front, flush in the wall.
                assert_eq!(z, 20, "air conditioner off the street front at {x},{y},{z}");
                assert!(y > 6, "air conditioner on the ground floor at {x},{y},{z}");
                assert!(
                    !world.contains_key(&(x, y, z - 1)),
                    "air conditioner sticks out at {x},{y},{z}"
                );
            }
            assert!(window_boxes(&world).is_empty(), "window boxes in New York");
        }
        assert!(units > 0, "no air conditioner on six walk-ups");

        // Not in Europe, not on offices or modern blocks.
        let office: &[(&str, &str)] = &[("building", "office"), ("building:levels", "6")];
        let modern: &[(&str, &str)] = &[
            ("building", "apartments"),
            ("building:levels", "6"),
            ("start_date", "1995"),
        ];
        for id in 300..306u64 {
            assert!(air_conditioners(&dwelling_world(berlin(), id, walk_up)).is_empty());
            assert!(air_conditioners(&dwelling_world(nyc(), id, office)).is_empty());
            assert!(air_conditioners(&dwelling_world(nyc(), id, modern)).is_empty());
        }
    }

    #[test]
    fn window_boxes_only_on_upper_floors_of_old_european_dwellings() {
        let old: &[(&str, &str)] = &[
            ("building", "apartments"),
            ("building:levels", "5"),
            ("start_date", "1895"),
        ];
        let mut boxes = 0;
        for id in 400..408u64 {
            let world = dwelling_world(berlin(), id, old);
            for (x, y, z) in window_boxes(&world) {
                boxes += 1;
                // A block out from the wall, on a sill, above the ground floor.
                let outside = (x == 9 || x == 46) && (20..=36).contains(&z)
                    || (z == 19 || z == 37) && (10..=45).contains(&x);
                assert!(outside, "window box off the wall face at {x},{y},{z}");
                // Over the first storey band, not on a ground-floor sill.
                assert!(y >= 6, "window box on the ground floor at {x},{y},{z}");
                assert!(
                    world
                        .get(&(x, y - 1, z))
                        .is_some_and(|b| b.name().ends_with("_slab")),
                    "window box without a sill at {x},{y},{z}"
                );
            }
        }
        assert!(boxes > 0, "no window box on eight old blocks");

        // Not on new blocks, offices, or in North America.
        let new: &[(&str, &str)] = &[
            ("building", "apartments"),
            ("building:levels", "5"),
            ("start_date", "1990"),
        ];
        let office: &[(&str, &str)] = &[
            ("building", "office"),
            ("building:levels", "5"),
            ("start_date", "1895"),
        ];
        for id in 400..408u64 {
            assert!(window_boxes(&dwelling_world(berlin(), id, new)).is_empty());
            assert!(window_boxes(&dwelling_world(berlin(), id, office)).is_empty());
            assert!(window_boxes(&dwelling_world(nyc(), id, old)).is_empty());
        }
    }

    // A mapped roof colour keeps one covering; an unmapped one shows the odd
    // patch of its sibling.
    #[test]
    fn roof_tone_patches_skip_mapped_roofs() {
        let count = |tags: &[(&str, &str)]| {
            let mut sibling = 0;
            for id in 500..504u64 {
                let world = dwelling_world(berlin(), id, tags);
                sibling += world
                    .values()
                    .filter(|&&b| {
                        matches!(
                            b,
                            POLISHED_GRANITE_STAIRS
                                | POLISHED_DEEPSLATE_STAIRS
                                | DEEPSLATE_BRICK_STAIRS
                                | BLACKSTONE_STAIRS
                        )
                    })
                    .count();
            }
            sibling
        };
        let house: &[(&str, &str)] = &[
            ("building", "house"),
            ("roof:shape", "gabled"),
            ("building:levels", "2"),
        ];
        assert!(count(house) > 0, "no tone patch on four tiled roofs");
        let tagged: &[(&str, &str)] = &[
            ("building", "house"),
            ("roof:shape", "gabled"),
            ("building:levels", "2"),
            ("roof:colour", "darkred"),
        ];
        assert_eq!(count(tagged), 0, "a mapped roof colour got patches");
    }
}

#[cfg(test)]
mod facade_dump {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{bitmap_with_rect, rect_way};
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
        } else if matches!(
            b,
            SMOOTH_STONE | POLISHED_DIORITE | QUARTZ_BLOCK | STONE_BRICKS | SMOOTH_SANDSTONE
        ) {
            '='
        } else if SHOP_FRAME_OPTIONS.contains(&b) || EU_SHOP_FRAME_OPTIONS.contains(&b) {
            'X'
        } else if name == "air" {
            ' '
        } else {
            '#'
        }
    }

    // Renders the street facade of a synthetic building as ASCII for manual
    // dial tuning: cargo test facade_dump -- --ignored --nocapture
    // Upper case: a block in front of the wall; ':' glass set back behind it,
    // ';' anything else set back; '=' trim stone; 'X' shopfront frame.
    #[test]
    #[ignore]
    fn dump_street_facades() {
        use crate::coordinate_system::geographic::LLBBox;
        use crate::element_processing::building_test_support::test_editor_at;
        // Same reason as the tests above: this dump reads wall blocks, and a
        // facade test running beside it would flatten them.
        let _guard = crate::mapillary::facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let nyc = LLBBox::new(40.70, -74.02, 40.72, -74.00).unwrap();
        let munich = LLBBox::new(48.13, 11.56, 48.15, 11.58).unwrap();
        for (label, place, tags, attached) in [
            (
                "NYC apartments 5 levels, attached",
                nyc,
                vec![("building", "apartments"), ("building:levels", "5")],
                true,
            ),
            (
                "NYC commercial 4 levels",
                nyc,
                vec![("building", "commercial"), ("building:levels", "4")],
                false,
            ),
            (
                "NYC yes 6 levels, attached",
                nyc,
                vec![("building", "yes"), ("building:levels", "6")],
                true,
            ),
            (
                "NYC tall yes, 20 levels",
                nyc,
                vec![("building", "yes"), ("building:levels", "20")],
                false,
            ),
            (
                "Munich apartments 4 levels, pre-war",
                munich,
                vec![
                    ("building", "apartments"),
                    ("building:levels", "4"),
                    ("start_date", "1905"),
                ],
                true,
            ),
            (
                "plain house, no tags",
                munich,
                vec![("building", "house")],
                false,
            ),
        ] {
            for seed_shift in 0..2u64 {
                let xz = XZBBox::rect_from_xz_lengths(70.0, 60.0).unwrap();
                let road = bitmap_with_rect(&xz, 0, 12, 69, 14);
                let footprints = if attached {
                    let mut f = bitmap_with_rect(&xz, 10, 20, 21, 34);
                    for x in 53..=64 {
                        for z in 20..=34 {
                            f.set(x, z);
                        }
                    }
                    f
                } else {
                    CoordinateBitmap::new(&xz)
                };
                let way = rect_way(4242 + seed_shift * 7, 22, 20, 52, 34, &tags);
                let mut editor = test_editor_at(&xz, place);
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
                    interior_uses:
                        crate::element_processing::subprocessor::interior::InteriorUseIndex::empty(),
                };
                generate_buildings(&mut editor, &way, &args, None, None, &ctx, way.id);

                println!("\n=== {label} (way {}) ===", way.id);
                let top = (0..90)
                    .rev()
                    .find(|&y| (20..=54).any(|x| editor.get_block_absolute(x, y, 20).is_some()))
                    .unwrap_or(0);
                for y in (0..=top.min(40) + 2).rev() {
                    let mut line = String::new();
                    for x in 20..=54 {
                        let outward = editor.get_block_absolute(x, y, 19);
                        let wall = editor.get_block_absolute(x, y, 20);
                        let inner = editor.get_block_absolute(x, y, 21);
                        let c = if outward.is_some() {
                            glyph(outward).to_ascii_uppercase()
                        } else if wall.is_some() {
                            glyph(wall)
                        } else if inner.is_some() {
                            if glyph(inner) == 'o' {
                                ':'
                            } else {
                                ';'
                            }
                        } else {
                            ' '
                        };
                        line.push(c);
                    }
                    println!("y{y:>2} |{line}|");
                }
            }
        }
    }
}

#[cfg(test)]
mod start_y_tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{rect_way, test_editor};
    use clap::Parser as _;

    fn offset_for(terrain_y: i32) -> i32 {
        let xzbbox = XZBBox::rect_from_xz_lengths(64.0, 64.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        editor.set_ground(std::sync::Arc::new(crate::ground::Ground::new_flat(
            terrain_y,
        )));
        let way = rect_way(1, 10, 10, 20, 20, &[("building", "yes")]);
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        calculate_start_y_offset(&editor, &way, &args, 0)
    }

    /// With the extended floor the scaler sinks the terrain base far below the default
    /// -62, and buildings must follow it down instead of floating at the old level.
    #[test]
    fn sunk_terrain_base_does_not_strand_buildings() {
        assert_eq!(offset_for(-1500), -1500);
    }

    #[test]
    fn vanilla_terrain_is_unchanged() {
        assert_eq!(offset_for(-62), -62);
        assert_eq!(offset_for(80), 80);
    }
}

#[cfg(test)]
mod doorstep_tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{
        bitmap_with_rect, rect_way, test_editor,
    };
    use clap::Parser as _;
    use fnv::FnvHashMap;
    use std::sync::Arc;

    /// World bounds are inclusive, and the elevation grid is one cell per
    /// block, so that `terrain_level` returns the fixture's own numbers
    /// instead of a bilinear blend of them.
    const MAX_X: i32 = 70;
    const MAX_Z: i32 = 60;
    const WORLD_W: usize = MAX_X as usize + 1;
    const WORLD_L: usize = MAX_Z as usize + 1;
    /// The house's north wall. The street in these fixtures runs north of it,
    /// so this is the side `plan_synthetic_entrance` puts the door on.
    const WALL_Z: i32 = 20;
    /// How far out `render_entrance` can reach: the threshold cell plus the
    /// steps behind it. Spelled out rather than taken from the constant, so
    /// this reads the same against the code before and after the fix.
    const REACH: i32 = 3;

    /// One cell of the doorstep run, read back out of the generated world.
    #[derive(Debug, PartialEq, Eq)]
    struct RunCell {
        /// Distance out from the door, 1 being the threshold cell.
        step: i32,
        /// The block found there, if any.
        block: Option<String>,
        y: i32,
        /// Terrain top under it. A block rests on the ground when this is
        /// exactly one below it; anything lower and it hangs in the air.
        ground: i32,
    }

    impl RunCell {
        fn is_stair(&self) -> bool {
            self.block
                .as_deref()
                .is_some_and(|n| n.ends_with("_stairs"))
        }

        fn hangs(&self) -> bool {
            self.ground < self.y - 1
        }
    }

    struct Fixture {
        /// The door column found in the north wall, if the building got one.
        door: Option<(i32, i32)>,
        floor: i32,
        run: Vec<RunCell>,
        /// Glass anywhere in the north wall, which a flattened facade removes.
        wall_has_glass: bool,
    }

    /// Generates one house over terrain that steps from `outside_y` in front of
    /// the north wall up to `inside_y` under the building, then reads the door
    /// and the cells of its doorstep run back out of the world.
    ///
    /// The floor follows the *highest* terrain under the outline, so
    /// `inside_y - outside_y` is exactly the drop the run has to bridge.
    fn build(inside_y: i32, outside_y: i32, presets: bool) -> Fixture {
        let xz = XZBBox::rect_from_min_max(0, 0, MAX_X, MAX_Z).unwrap();
        let road = bitmap_with_rect(&xz, 0, 12, MAX_X, 14);
        let footprints = CoordinateBitmap::new(&xz);
        let way = rect_way(4242, 22, WALL_Z, 52, 34, &[("building", "house")]);
        let mut editor = test_editor(&xz);
        editor.set_map_decals(presets);
        let heights: Vec<Vec<f32>> = (0..WORLD_L)
            .map(|z| {
                let y = if (z as i32) < WALL_Z {
                    outside_y
                } else {
                    inside_y
                };
                vec![y as f32; WORLD_W]
            })
            .collect();
        editor.set_ground(Arc::new(crate::ground::Ground::new_elevation_test(
            heights, WORLD_W, WORLD_L,
        )));
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4", "--ground-level", "0"]);
        let cache = FloodFillCache::new();
        let passages = CoordinateBitmap::new_empty();
        let groups: FnvHashMap<u64, Vec<u64>> = FnvHashMap::default();
        let ctx = BuildingContext {
            flood_fill_cache: &cache,
            building_passages: &passages,
            road_mask: &road,
            building_footprints: &footprints,
            group_members: &groups,
            interior_uses:
                crate::element_processing::subprocessor::interior::InteriorUseIndex::empty(),
        };
        generate_buildings(&mut editor, &way, &args, None, None, &ctx, way.id);

        // Every outline node stands on the inside terrace, and the floor is the
        // highest terrain under the outline, so the floor is `inside_y` even
        // when the ground outside is higher.
        let floor = inside_y;
        let door = (22..=52).find(|&x| {
            editor
                .get_block_absolute(x, floor + 1, WALL_Z)
                .is_some_and(|b| b.name().ends_with("_door"))
        });
        let run = door
            .map(|dx| {
                (1..=REACH)
                    .map(|step| {
                        let z = WALL_Z - step;
                        let y = floor - step + 1;
                        RunCell {
                            step,
                            block: editor
                                .get_block_absolute(dx, y, z)
                                .map(|b| b.name().to_string()),
                            y,
                            ground: editor.terrain_level(dx, z).unwrap(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let wall_has_glass = (22..=52).any(|x| {
            (floor..=floor + 8).any(|y| {
                editor
                    .get_block_absolute(x, y, WALL_Z)
                    .is_some_and(|b| b.name().contains("glass"))
            })
        });
        Fixture {
            door: door.map(|x| (x, WALL_Z)),
            floor,
            run,
            wall_has_glass,
        }
    }

    fn plain(inside_y: i32, outside_y: i32) -> Fixture {
        let _guard = crate::mapillary::facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        build(inside_y, outside_y, false)
    }

    /// The owner's report: a building whose floor is six blocks above the
    /// ground in front of its door used to get a run of steps that stopped in
    /// mid air, because the run was clamped to three steps and never asked
    /// whether the third one had landed.
    #[test]
    fn a_drop_the_run_cannot_reach_gets_no_steps() {
        let f = plain(6, 0);
        assert!(f.door.is_some(), "the fixture has to produce a door");
        let hanging: Vec<&RunCell> = f.run.iter().filter(|c| c.is_stair() && c.hangs()).collect();
        assert!(
            hanging.is_empty(),
            "steps left hanging over a 6 block drop: {hanging:?}"
        );
        assert!(
            f.run.iter().all(|c| !c.is_stair()),
            "a drop this deep must get no steps at all: {:?}",
            f.run
        );
    }

    /// The same building one block deeper than the run's three step reach.
    #[test]
    fn a_drop_one_deeper_than_the_reach_gets_no_steps() {
        let f = plain(4, 0);
        assert!(
            f.run.iter().all(|c| !c.is_stair()),
            "a 4 block drop is past the run's reach: {:?}",
            f.run
        );
    }

    /// The case the steps exist for: the run lands, so it is still built.
    #[test]
    fn a_drop_the_run_reaches_keeps_its_steps() {
        let f = plain(2, 0);
        let steps: Vec<&RunCell> = f.run.iter().filter(|c| c.is_stair()).collect();
        assert_eq!(steps.len(), 1, "expected one step, got {:?}", f.run);
        assert!(!steps[0].hangs(), "the step has to rest on the ground");
        assert_eq!(steps[0].step, 2, "the step sits behind the threshold");
    }

    /// Flat ground: no drop, so nothing but the threshold, before and after.
    #[test]
    fn flat_ground_gets_a_threshold_and_no_steps() {
        let f = plain(0, 0);
        assert!(f.door.is_some());
        assert!(
            f.run.iter().all(|c| !c.is_stair()),
            "flat ground must not get steps: {:?}",
            f.run
        );
        let threshold = &f.run[0];
        assert_eq!(threshold.y, f.floor);
        assert!(
            threshold.block.is_some() && !threshold.hangs(),
            "the threshold sits on the ground in front of the door: {threshold:?}"
        );
    }

    /// Ground higher than the floor: there is nothing to step down to.
    #[test]
    fn ground_above_the_floor_gets_no_steps() {
        let f = plain(0, 3);
        assert!(f.door.is_some());
        assert!(
            f.run.iter().all(|c| !c.is_stair()),
            "an uphill door must not get steps: {:?}",
            f.run
        );
    }

    /// A preset facade flattens the wall it hangs on but leaves the door, so
    /// the doorstep rule has to hold there too.
    #[test]
    fn a_preset_flattened_wall_follows_the_same_rule() {
        let _guard = crate::mapillary::facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("r00.png"), b"").unwrap();
        std::fs::write(
            dir.path()
                .join(crate::building_facades::manifest::MANIFEST_NAME),
            r#"{"version": 1, "textures": [{"file": "r00.png", "categories": ["Default"],
                "metres_wide": 12.0, "metres_tall": 12.4, "storeys": 4,
                "tiles_horizontally": true, "has_ground_floor": true}]}"#,
        )
        .unwrap();
        crate::building_facades::reset(true, Some(dir.path()), 16, 1.0);
        let deep = build(6, 0, true);
        let shallow = build(2, 0, true);
        crate::building_facades::reset(false, None, 16, 1.0);
        let unflattened = build(6, 0, false);

        assert!(
            unflattened.wall_has_glass && !deep.wall_has_glass,
            "the fixture has to actually flatten the wall"
        );
        assert!(deep.door.is_some(), "a flattened wall still gets its door");
        assert!(
            deep.run.iter().all(|c| !c.is_stair()),
            "a flattened wall must not get hanging steps: {:?}",
            deep.run
        );
        let steps: Vec<&RunCell> = shallow.run.iter().filter(|c| c.is_stair()).collect();
        assert_eq!(steps.len(), 1, "and keeps the ones that land");
        assert!(!steps[0].hangs());
    }
}

#[cfg(test)]
mod roof_geometry_tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{tag_map, test_editor};

    const BASE: i32 = 10;

    fn roof_config(cells: &[(i32, i32)]) -> RoofConfig {
        let mut config = RoofConfig::from_roof_area(cells, 7, BASE - 1, 0, BRICK, 0);
        config.roof_block = BRICK;
        config.building_height = 40;
        config
    }

    fn ring(pts: &[(i32, i32)]) -> Vec<ProcessedNode> {
        pts.iter()
            .chain(pts.first())
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: i as u64,
                tags: HashMap::new(),
                x,
                z,
            })
            .collect()
    }

    /// Lattice cells inside a polygon (even-odd), plus its outline cells.
    fn fill(nodes: &[ProcessedNode]) -> Vec<(i32, i32)> {
        let pts: Vec<(f64, f64)> = nodes.iter().map(|n| (n.x as f64, n.z as f64)).collect();
        let (min_x, max_x) = (
            nodes.iter().map(|n| n.x).min().unwrap(),
            nodes.iter().map(|n| n.x).max().unwrap(),
        );
        let (min_z, max_z) = (
            nodes.iter().map(|n| n.z).min().unwrap(),
            nodes.iter().map(|n| n.z).max().unwrap(),
        );
        let mut cells: Vec<(i32, i32)> = (min_z..=max_z)
            .flat_map(|z| (min_x..=max_x).map(move |x| (x, z)))
            .filter(|&(x, z)| polygon_contains(&pts, x as f64, z as f64))
            .collect();
        cells.extend(thin_ring_cells(nodes));
        cells.sort_unstable();
        cells.dedup();
        cells
    }

    fn rect_cells(w: i32, l: i32) -> Vec<(i32, i32)> {
        (0..l).flat_map(|z| (0..w).map(move |x| (x, z))).collect()
    }

    fn top(editor: &WorldEditor, x: i32, z: i32) -> i32 {
        editor
            .highest_block_between(x, z, 0, 400)
            .unwrap_or_else(|| panic!("no roof at {x},{z}"))
    }

    #[test]
    fn roof_direction_parses_points_and_bearings() {
        assert_eq!(parse_roof_direction_degrees("ne"), Some(45.0));
        assert_eq!(parse_roof_direction_degrees("164"), Some(164.0));
        assert_eq!(parse_roof_direction_degrees("-90"), Some(270.0));
        assert_eq!(parse_roof_direction_degrees("uphill"), None);
        assert_eq!(cardinal_for_bearing(164.0), StairFacing::South);
        assert_eq!(cardinal_for_bearing(344.0), StairFacing::North);
        let (dx, dz) = bearing_unit_vector(90.0);
        assert!((dx - 1.0).abs() < 1e-9 && dz.abs() < 1e-9, "east is +X");
        let (dx, dz) = bearing_unit_vector(0.0);
        assert!(dx.abs() < 1e-9 && (dz + 1.0).abs() < 1e-9, "north is -Z");
    }

    // A chamfer or a rotated wing slopes along its mapped bearing, not the
    // nearest cardinal: cells on one line across the bearing share a height.
    #[test]
    fn skillion_slopes_along_a_diagonal_bearing() {
        let xz = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xz);
        let cells = rect_cells(20, 20);
        let mut config = roof_config(&cells);
        config.peak_cap = Some(19);
        // Downhill to the north-east: the south-west corner is the ridge.
        generate_skillion_roof(&mut editor, &cells, &config, Some("45"));

        let sw = top(&editor, 0, 19);
        let ne = top(&editor, 19, 0);
        assert_eq!(sw - ne, 19, "full mapped rise from ridge to eave");
        // Lines of constant x - z run across the bearing.
        assert_eq!(top(&editor, 2, 2), top(&editor, 10, 10));
        assert_eq!(top(&editor, 10, 10), top(&editor, 17, 17));
        assert_eq!(top(&editor, 0, 10), top(&editor, 9, 19));
        // Along the bearing it descends monotonically.
        let along: Vec<i32> = (0..20).map(|i| top(&editor, i, 19 - i)).collect();
        assert!(along.windows(2).all(|w| w[0] >= w[1]), "{along:?}");
    }

    #[test]
    fn skillion_keeps_a_mapped_rise_above_twelve_blocks() {
        let xz = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xz);
        let cells = rect_cells(10, 10);
        let mut config = roof_config(&cells);
        config.peak_cap = Some(30);
        generate_skillion_roof(&mut editor, &cells, &config, Some("south"));
        assert_eq!(top(&editor, 5, 0) - top(&editor, 5, 9), 30);
        // A face that steep is built of full blocks, not stairs.
        let y = top(&editor, 5, 4);
        assert_eq!(editor.get_block_absolute(5, y, 4), Some(BRICK));
    }

    #[test]
    fn skillion_cardinal_bearing_matches_the_bbox_slope() {
        let xz = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xz);
        let cells = rect_cells(12, 6);
        let config = roof_config(&cells);
        generate_skillion_roof(&mut editor, &cells, &config, Some("west"));
        // Descends toward -X, one height per column.
        for z in 0..6 {
            assert_eq!(top(&editor, 0, z), top(&editor, 0, 0));
        }
        assert!(top(&editor, 11, 3) > top(&editor, 0, 3));
    }

    #[test]
    fn roof_angle_run_follows_a_diagonal_skillion_bearing() {
        let tags = tag_map(&[("roof:angle", "45"), ("roof:direction", "45")]);
        let cells = rect_cells(20, 20);
        let config = roof_config(&cells);
        // The bbox spans 20 cells each way, so the diagonal run is 20 * sqrt 2.
        let cap = roof_peak_cap(
            &tags,
            RoofType::Skillion,
            &config,
            &RidgeFrame::grid(true),
            1.0,
        );
        assert_eq!(cap, Some(28));
    }

    // The apex sits over the centroid and the faces follow the outline, so a
    // 45-degree square gets a pyramid with ridges on its own diagonals.
    #[test]
    fn pyramid_follows_a_rotated_square() {
        let nodes = ring(&[(10, 0), (20, 10), (10, 20), (0, 10)]);
        let cells = fill(&nodes);
        let xz = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xz);
        let mut config = roof_config(&cells);
        config.peak_cap = Some(10);
        generate_pyramidal_roof(&mut editor, &cells, &config, &nodes);

        assert_eq!(
            top(&editor, 10, 10),
            BASE + 10,
            "apex carries the mapped rise"
        );
        // Equal L1 distance from the centre means equal height on the diamond.
        assert_eq!(top(&editor, 13, 7), top(&editor, 10, 4));
        assert_eq!(top(&editor, 13, 7), top(&editor, 16, 10));
        // Halfway out along a face the rise is half; the bbox rule gave 7 here.
        assert_eq!(top(&editor, 13, 7), BASE + 4);
        // The outline is the eave.
        assert_eq!(top(&editor, 15, 5), BASE);
    }

    #[test]
    fn pyramid_on_an_axis_aligned_rectangle_is_unchanged() {
        let nodes = ring(&[(0, 0), (20, 0), (20, 10), (0, 10)]);
        let cells = fill(&nodes);
        let xz = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xz);
        let mut config = roof_config(&cells);
        config.peak_cap = Some(5);
        generate_pyramidal_roof(&mut editor, &cells, &config, &nodes);
        assert_eq!(top(&editor, 10, 5), BASE + 5);
        // Single apex: the long axis climbs at half the pitch of the short one.
        assert_eq!(top(&editor, 5, 5), BASE + 2);
        assert_eq!(top(&editor, 10, 2), BASE + 2);
    }

    #[test]
    fn dome_meets_the_eave_all_round_a_rotated_ellipse() {
        // Semi-axes 18 and 7, rotated 30 degrees.
        let (a, b, rot) = (18.0f64, 7.0f64, 30.0f64.to_radians());
        let pts: Vec<(i32, i32)> = (0..48)
            .map(|i| {
                let t = i as f64 / 48.0 * std::f64::consts::TAU;
                let (ex, ez) = (a * t.cos(), b * t.sin());
                let x = 30.0 + ex * rot.cos() - ez * rot.sin();
                let z = 30.0 + ex * rot.sin() + ez * rot.cos();
                (x.round() as i32, z.round() as i32)
            })
            .collect();
        let mut pts_dedup = pts.clone();
        pts_dedup.dedup();
        let nodes = ring(&pts_dedup);
        let cells = fill(&nodes);
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut editor = test_editor(&xz);
        let mut config = roof_config(&cells);
        config.peak_cap = Some(12);
        generate_dome_roof(&mut editor, &cells, &config, &nodes);

        assert_eq!(
            top(&editor, 30, 30),
            BASE + 12,
            "crown carries the mapped rise"
        );
        let footprint: HashSet<(i32, i32)> = cells.iter().copied().collect();
        let mut worst = 0;
        for &(x, z) in &cells {
            let on_edge = [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)]
                .iter()
                .any(|n| !footprint.contains(n));
            if on_edge {
                worst = worst.max(top(&editor, x, z) - BASE);
            }
        }
        // A hemisphere is steep at its rim, so the half cell an outline cell
        // sits inside the polygon is worth a few blocks. The bbox rule left a
        // wall of ten along the flanks.
        assert!(
            worst <= 4,
            "eave cells rise {worst} blocks above the wall top"
        );
    }

    #[test]
    fn radial_fractions_bail_out_on_a_footprint_the_centroid_cannot_see() {
        // A courtyard block: a thick base with two wings. The area centroid
        // lands in the base, inside the footprint, but the inner face of each
        // wing sits behind the open courtyard, so no single ray fan measures
        // the wings against their own walls.
        let nodes = ring(&[
            (0, 0),
            (30, 0),
            (30, 30),
            (18, 30),
            (18, 20),
            (12, 20),
            (12, 30),
            (0, 30),
        ]);
        let cells = fill(&nodes);
        let pts: Vec<(f64, f64)> = nodes.iter().map(|n| (n.x as f64, n.z as f64)).collect();
        assert!(polygon_contains(&pts, 15.0, 14.0), "the centroid is inside");
        assert!(footprint_radial_fractions(&nodes, &cells).is_none());
    }

    // A plus is concave but every cell is still visible from its centre, so it
    // keeps the radial profile rather than falling back to the bounding box.
    #[test]
    fn radial_fractions_keep_a_concave_but_visible_footprint() {
        let nodes = ring(&[
            (10, 0),
            (20, 0),
            (20, 10),
            (30, 10),
            (30, 20),
            (20, 20),
            (20, 30),
            (10, 30),
            (10, 20),
            (0, 20),
            (0, 10),
            (10, 10),
        ]);
        let cells = fill(&nodes);
        let radial =
            footprint_radial_fractions(&nodes, &cells).expect("a plus is star-shaped from centre");
        assert!(radial[&(15, 15)] < 0.2, "the middle reads as the apex");
        assert!(radial[&(15, 29)] > 0.9, "an arm tip reads as the eave");
    }

    #[test]
    fn radial_fractions_survive_a_convex_footprint() {
        // A rotated octagon: concave nowhere, so every cell stays visible.
        let nodes = ring(&[
            (10, 0),
            (20, 4),
            (24, 14),
            (20, 24),
            (10, 28),
            (0, 24),
            (-4, 14),
            (0, 4),
        ]);
        let cells = fill(&nodes);
        let radial = footprint_radial_fractions(&nodes, &cells).expect("convex ring keeps its fan");
        assert_eq!(radial.len(), cells.len());
        // The rim reads as the eave, the middle as the apex.
        assert!(radial[&(10, 0)] > 0.9);
        assert!(radial[&(10, 14)] < 0.2);
    }

    #[test]
    fn radial_fractions_bail_out_when_the_centroid_is_outside() {
        // A thin L: its centroid lies in the empty quadrant.
        let nodes = ring(&[(0, 0), (30, 0), (30, 4), (4, 4), (4, 30), (0, 30)]);
        let cells = fill(&nodes);
        assert!(footprint_radial_fractions(&nodes, &cells).is_none());
    }

    // A mapped roof:height fixes the pitch: a 3 m rise across a 12-wide span
    // is a shallow pediment, not a 1:1 slope cut off into a mesa.
    #[test]
    fn gable_pitch_follows_a_stated_rise() {
        let cells = rect_cells(30, 12);
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut editor = test_editor(&xz);
        let mut config = roof_config(&cells);
        config.peak_cap = Some(3);
        generate_gabled_roof(
            &mut editor,
            &cells,
            &config,
            GableProfile::Standard,
            &RidgeFrame::grid(true),
            None,
        );
        let eave = top(&editor, 15, 0);
        let ridge = top(&editor, 15, 5);
        assert_eq!(ridge - eave, 3, "ridge reaches the mapped rise");
        let mid = top(&editor, 15, 3);
        assert!(
            mid > eave && mid < ridge,
            "eave {eave} mid {mid} ridge {ridge}"
        );
    }

    #[test]
    fn gable_reaches_a_stated_rise_steeper_than_one_to_one() {
        let cells = rect_cells(30, 12);
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut editor = test_editor(&xz);
        let mut config = roof_config(&cells);
        config.peak_cap = Some(10);
        generate_gabled_roof(
            &mut editor,
            &cells,
            &config,
            GableProfile::Standard,
            &RidgeFrame::grid(true),
            None,
        );
        // Previously capped at 85% of the half-span (5).
        assert_eq!(top(&editor, 15, 5) - top(&editor, 15, 0), 10);
    }

    #[test]
    fn hip_pitch_follows_a_stated_rise() {
        let cells = rect_cells(30, 12);
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut editor = test_editor(&xz);
        let mut config = roof_config(&cells);
        config.peak_cap = Some(3);
        generate_hipped_roof(&mut editor, &cells, &config);
        let eave = top(&editor, 15, 0);
        let ridge = top(&editor, 15, 5);
        assert_eq!(ridge - eave, 3);
        let mid = top(&editor, 15, 3);
        assert!(
            mid > eave && mid < ridge,
            "eave {eave} mid {mid} ridge {ridge}"
        );
    }

    #[test]
    fn roof_direction_turns_the_ridge_across_the_slope() {
        // 30 x 12: the ridge would run along X. A slope facing east means the
        // ridge runs along Z instead, so heights vary with X and not with Z.
        let nodes = ring(&[(0, 0), (29, 0), (29, 11), (0, 11)]);
        let ridge = ridge_frame(&nodes, 29, 11, None, Some("90"), None);
        assert!(!ridge.along_x);
        let ridge = ridge_frame(&nodes, 29, 11, None, Some("north"), None);
        assert!(ridge.along_x);
        // roof:direction wins over the longer side, roof:orientation still
        // applies when it is absent.
        let ridge = ridge_frame(&nodes, 29, 11, Some("across"), None, None);
        assert!(!ridge.along_x);
    }

    #[test]
    fn turned_rectangle_gets_a_planar_tent_on_its_own_axis() {
        // A 30 x 12 rectangle turned 30 degrees.
        let (c, sn) = (30.0f64.to_radians().cos(), 30.0f64.to_radians().sin());
        let pts: Vec<(i32, i32)> = [(0.0, 0.0), (30.0, 0.0), (30.0, 12.0), (0.0, 12.0)]
            .iter()
            .map(|&(u, v): &(f64, f64)| {
                (
                    (20.0 + u * c - v * sn).round() as i32,
                    (5.0 + u * sn + v * c).round() as i32,
                )
            })
            .collect();
        let nodes = ring(&pts);
        let cells = fill(&nodes);
        let xz = XZBBox::rect_from_xz_lengths(70.0, 70.0).unwrap();
        let mut editor = test_editor(&xz);
        let mut config = roof_config(&cells);
        config.peak_cap = Some(5);
        let ridge = ridge_frame(&nodes, config.width(), config.length(), None, None, None);
        let tent = ridge.tent.expect("turned rectangle gets a tent");
        assert!(ridge.along_x, "ridge follows the 30 side, nearest X");
        generate_gabled_roof(
            &mut editor,
            &cells,
            &config,
            GableProfile::Standard,
            &ridge,
            None,
        );
        // Two cells on the ridge line (v at mid-span), a third of the way in
        // and two thirds of the way in, share the crown height; a cell near
        // the long side sits at the eave.
        let at = |u: f64, v: f64| -> (i32, i32) {
            (
                (20.0 + u * c - v * sn).round() as i32,
                (5.0 + u * sn + v * c).round() as i32,
            )
        };
        let (rx, rz) = at(10.0, 6.0);
        let (sx, sz) = at(20.0, 6.0);
        let (ex, ez) = at(15.0, 0.0);
        assert!(tent.angle.to_degrees() > 25.0 && tent.angle.to_degrees() < 35.0);

        assert_eq!(top(&editor, rx, rz), top(&editor, sx, sz));
        assert_eq!(top(&editor, rx, rz) - top(&editor, ex, ez), 5);
        // A cell on the gable end is not treated as an eave.
        let (gx, gz) = at(0.5, 6.0);
        assert!(top(&editor, gx, gz) >= top(&editor, ex, ez) + 3);
    }

    fn shape_at(editor: &WorldEditor, x: i32, y: i32, z: i32) -> String {
        match editor.block_properties_absolute(x, y, z) {
            Some(Value::Compound(map)) => match map.get("shape") {
                Some(Value::String(s)) => s.clone(),
                _ => "straight".to_string(),
            },
            _ => "none".to_string(),
        }
    }

    fn l_shape() -> Vec<ProcessedNode> {
        // A 30 x 10 bar along X and a 10 x 30 arm down Z, meeting at the origin.
        ring(&[(0, 0), (30, 0), (30, 10), (10, 10), (10, 30), (0, 30)])
    }

    #[test]
    fn wings_split_l_t_and_courtyard_footprints() {
        let l = l_shape();
        let frame = roof_wings(&l, &[], fill(&l).len(), None).expect("an L splits");
        assert_eq!(frame.wings.len(), 2);
        assert_ne!(frame.wings[0].along_u, frame.wings[1].along_u);
        // One wing runs into the other and stops at its ridge.
        let clipped: Vec<&RoofWing> = frame
            .wings
            .iter()
            .filter(|w| w.clipped.0 || w.clipped.1)
            .collect();
        assert_eq!(clipped.len(), 1);
        let (a0, a1) = clipped[0].along();
        assert!((a0 - 5.0).abs() < 1e-9 || (a1 - 5.0).abs() < 1e-9);

        // A T: the stem dies into the bar instead of gabling out its far side.
        let t = ring(&[
            (0, 0),
            (40, 0),
            (40, 10),
            (25, 10),
            (25, 24),
            (15, 24),
            (15, 10),
            (0, 10),
        ]);
        let frame = roof_wings(&t, &[], fill(&t).len(), None).expect("a T splits");
        let stem = frame
            .wings
            .iter()
            .find(|w| !w.along_u)
            .expect("a stem wing");
        assert_eq!(stem.v.0, 5.0, "the stem starts at the bar's ridge");
        assert!(stem.clipped.0 && !stem.clipped.1);

        // A plain rectangle stays with the single-ridge scan, and so does a
        // bar whose walls jog: its parts all run one way.
        let rect = ring(&[(0, 0), (20, 0), (20, 10), (0, 10)]);
        assert!(roof_wings(&rect, &[], fill(&rect).len(), None).is_none());
        let jog = ring(&[(0, 0), (40, 0), (40, 14), (10, 14), (10, 10), (0, 10)]);
        assert!(roof_wings(&jog, &[], fill(&jog).len(), None).is_none());

        // A courtyard ring is four wings around the hole.
        let outer = ring(&[(0, 0), (40, 0), (40, 30), (0, 30)]);
        let hole = ring(&[(12, 12), (28, 12), (28, 18), (12, 18)]);
        let cells: usize = fill(&outer).len() - 15 * 5;
        let frame = roof_wings(&outer, &[hole.as_slice()], cells, None).expect("a ring splits");
        assert_eq!(frame.wings.len(), 4);
    }

    // A side wing gets its own ridge and eaves, not a flat-topped stretch of
    // the main ridge, and every outline cell is roofed in stairs or slabs.
    #[test]
    fn l_shaped_gable_roofs_both_wings_and_forms_a_valley() {
        let nodes = l_shape();
        let cells = fill(&nodes);
        let xz = XZBBox::rect_from_xz_lengths(50.0, 50.0).unwrap();
        let mut editor = test_editor(&xz);
        let config = roof_config(&cells);
        let wings = roof_wings(&nodes, &[], cells.len(), None);
        assert!(wings.is_some());
        let ridge = ridge_frame(&nodes, config.width(), config.length(), None, None, None);
        generate_gabled_roof(
            &mut editor,
            &cells,
            &config,
            GableProfile::Standard,
            &ridge,
            wings.as_ref(),
        );
        // Bar: ridge along X at z=5, eaves at z=0 and z=10.
        assert_eq!(top(&editor, 20, 5) - top(&editor, 20, 0), 5);
        assert_eq!(top(&editor, 15, 5), top(&editor, 25, 5));
        // Arm: ridge along Z at x=5.
        assert_eq!(top(&editor, 5, 20) - top(&editor, 0, 20), 5);
        assert_eq!(top(&editor, 5, 15), top(&editor, 5, 25));
        let footprint: HashSet<(i32, i32)> = cells.iter().copied().collect();
        for &(x, z) in &cells {
            let edge = ROOF_SIDES
                .iter()
                .any(|&(dx, dz)| !footprint.contains(&(x + dx, z + dz)));
            if !edge {
                continue;
            }
            let y = top(&editor, x, z);
            let block = editor.get_block_absolute(x, y, z);
            assert!(
                matches!(block, Some(BRICK_STAIRS) | Some(BRICK_SLAB)),
                "outline cell {x},{z} is topped by {block:?}"
            );
        }
        // The inside corner is a valley: inner stair corners down its line.
        for d in 1..4 {
            let (x, z) = (10 - d, 10 - d);
            let y = top(&editor, x, z);
            assert!(
                shape_at(&editor, x, y, z).starts_with("inner"),
                "valley cell {x},{z} is {}",
                shape_at(&editor, x, y, z)
            );
        }
    }

    // The 1:1 test takes the short half-span: a 10 x 20 house climbs at 45
    // degrees to its ridge, not at a half pitch set by its length, and its
    // hips are outer stair corners.
    #[test]
    fn hipped_pitch_follows_the_short_span() {
        let cells = rect_cells(20, 10);
        let xz = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xz);
        let mut config = roof_config(&cells);
        config.building_height = 8;
        generate_hipped_roof(&mut editor, &cells, &config);
        assert_eq!(top(&editor, 10, 4) - top(&editor, 10, 0), 4);
        let y = top(&editor, 10, 2);
        assert_eq!(editor.get_block_absolute(10, y, 2), Some(BRICK_STAIRS));
        let y = top(&editor, 2, 2);
        assert!(shape_at(&editor, 2, y, 2).starts_with("outer"));
    }

    // Too wide for 1:1 under the cap, a gable drops to 1:2 in alternating
    // slabs instead of stair and block treads.
    #[test]
    fn wide_gable_goes_shallow_in_slabs() {
        let cells = rect_cells(30, 21);
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut editor = test_editor(&xz);
        let mut config = roof_config(&cells);
        config.building_height = 8;
        generate_gabled_roof(
            &mut editor,
            &cells,
            &config,
            GableProfile::Standard,
            &RidgeFrame::grid(true),
            None,
        );
        assert_eq!(top(&editor, 15, 10) - top(&editor, 15, 0), 5);
        for z in 0..10 {
            let y = top(&editor, 15, z);
            assert_eq!(
                editor.get_block_absolute(15, y, z),
                Some(BRICK_SLAB),
                "z {z}"
            );
        }
        let y = top(&editor, 15, 1);
        let props = editor.block_properties_absolute(15, y, 1);
        assert!(format!("{props:?}").contains("top"), "{props:?}");
    }

    // A European deep block keeps its 1:1 slopes under the rise cap and
    // decks the flat top over in sheet, not tile; a 1:2 slope it is not.
    #[test]
    fn capped_town_block_keeps_its_pitch_under_a_sheet_top() {
        let cells = rect_cells(30, 30);
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut editor = test_editor(&xz);
        let mut config = roof_config(&cells);
        config.building_height = 8;
        config.trim.steep_cap = true;
        config.trim.sheet_top = true;
        generate_hipped_roof(&mut editor, &cells, &config);
        let rise = top(&editor, 15, 15) - top(&editor, 15, 0);
        assert_eq!(rise, 5, "1:1 up to the cap");
        assert_eq!(top(&editor, 15, 3) - top(&editor, 15, 0), 3);
        let y = top(&editor, 15, 15);
        assert!(matches!(
            editor.get_block_absolute(15, y, 15),
            Some(SMOOTH_STONE) | Some(LIGHT_GRAY_CONCRETE)
        ));
        let y = top(&editor, 15, 2);
        assert_eq!(editor.get_block_absolute(15, y, 2), Some(BRICK_STAIRS));
    }

    // The eave runs on under the rake past the gable, so the corners meet,
    // and it stops short of a neighbour's cells.
    #[test]
    fn eaves_close_the_gable_corners_and_spare_neighbours() {
        // Shifted off the world's edge so the overhang has room.
        let cells: Vec<(i32, i32)> = rect_cells(12, 9)
            .into_iter()
            .map(|(x, z)| (x + 5, z + 5))
            .collect();
        let xz = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xz);
        let mut config = roof_config(&cells);
        // A neighbour hard against the south wall.
        config.trim.blocked = (5..17).flat_map(|x| [(x, 14), (x, 15)]).collect();
        generate_gabled_roof(
            &mut editor,
            &cells,
            &config,
            GableProfile::Standard,
            &RidgeFrame::grid(true),
            None,
        );
        // Rake trim at the eave corner, the overhang running on below it.
        assert_eq!(
            editor.get_block_absolute(4, BASE + 1, 5),
            Some(BRICK_STAIRS)
        );
        assert_eq!(editor.get_block_absolute(4, BASE, 4), Some(BRICK_STAIRS));
        assert_eq!(
            editor.get_block_absolute(4, BASE - 1, 3),
            Some(BRICK_STAIRS)
        );
        // Free north side overhangs, the party side does not.
        assert_eq!(editor.get_block_absolute(10, BASE, 4), Some(BRICK_STAIRS));
        for x in 5..17 {
            for y in BASE - 1..=BASE + 1 {
                assert_eq!(editor.get_block_absolute(x, y, 14), None, "{x},{y}");
            }
        }
    }

    // A stack rises through the ridge and stands clear of it, away from the
    // gable ends; one cell in from an eave it was buried by the slope.
    #[test]
    fn chimney_stands_clear_of_the_ridge() {
        let cells = rect_cells(14, 8);
        let xz = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xz);
        let config = roof_config(&cells);
        let surface = generate_gabled_roof(
            &mut editor,
            &cells,
            &config,
            GableProfile::Standard,
            &RidgeFrame::grid(true),
            None,
        );
        let ridge = top(&editor, 7, 3);
        assert_eq!(surface[&(7, 3)], ridge);
        generate_chimney(&mut editor, &surface, BASE, 0, 42, None, BRICK);
        let stack: Vec<(i32, i32)> = cells
            .iter()
            .copied()
            .filter(|&(x, z)| editor.get_block_absolute(x, ridge + 1, z) == Some(BRICK))
            .collect();
        assert_eq!(stack.len(), 1, "one stack on a short house");
        let (x, z) = stack[0];
        assert!((2..=11).contains(&x) && (3..=4).contains(&z), "{x},{z}");
        assert!(top(&editor, x, z) >= ridge + 2);
    }

    // Dormers stand on the window axes of the wall below, every other one.
    #[test]
    fn dormers_follow_the_window_axes() {
        let cells = rect_cells(40, 12);
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut editor = test_editor(&xz);
        let mut config = roof_config(&cells);
        config.add_dormers = true;
        config.trim.window_phase = Some(2);
        config.trim.window_block = Some(GLASS);
        generate_gabled_roof(
            &mut editor,
            &cells,
            &config,
            GableProfile::Standard,
            &RidgeFrame::grid(true),
            None,
        );
        let mut glass_x: Vec<i32> = Vec::new();
        for &(x, z) in &cells {
            for y in BASE..BASE + 8 {
                if editor.get_block_absolute(x, y, z) == Some(GLASS) && !glass_x.contains(&x) {
                    glass_x.push(x);
                }
            }
        }
        assert!(!glass_x.is_empty(), "a long roof gets dormers");
        glass_x.sort_unstable();
        for w in glass_x.windows(2) {
            assert_eq!(w[1] - w[0], 12, "every other axis: {glass_x:?}");
        }
        assert!(glass_x.iter().all(|x| (x + 2).rem_euclid(6) == 1));
    }

    #[test]
    fn round_roof_is_a_barrel_vault() {
        assert_eq!(parse_roof_type("round"), RoofType::Round);
        let cells = rect_cells(30, 12);
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut editor = test_editor(&xz);
        let mut config = roof_config(&cells);
        config.peak_cap = Some(5);
        generate_gabled_roof(
            &mut editor,
            &cells,
            &config,
            GableProfile::Round,
            &RidgeFrame::grid(true),
            None,
        );
        let eave = top(&editor, 15, 0);
        let crown = top(&editor, 15, 5);
        assert_eq!(crown - eave, 5);
        // Convex section: a quarter of the way in it is already above the
        // straight line from eave to crown (which would be 2 there, 3 at half).
        assert!(top(&editor, 15, 2) - eave >= 3);
        assert_eq!(top(&editor, 15, 3), top(&editor, 15, 4));
    }

    #[test]
    fn domes_are_shells_over_a_hall() {
        let nodes = ring(&[(0, 0), (30, 0), (30, 30), (0, 30)]);
        let cells = fill(&nodes);
        let xz = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xz);
        let mut config = roof_config(&cells);
        config.peak_cap = Some(12);
        generate_dome_roof(&mut editor, &cells, &config, &nodes);
        let crown = top(&editor, 15, 15);
        assert_eq!(crown, BASE + 12);
        assert_eq!(editor.get_block_absolute(15, crown - 1, 15), Some(BRICK));
        // The hall under the crown is open.
        assert_eq!(editor.get_block_absolute(15, BASE + 4, 15), None);
        // The rim runs down to the wall top.
        assert_eq!(editor.get_block_absolute(15, BASE, 0), Some(BRICK));
    }

    #[test]
    fn parts_keep_thin_walls_under_a_pitched_roof() {
        let part = ProcessedWay {
            id: 1,
            nodes: vec![],
            tags: tag_map(&[
                ("building:part", "yes"),
                ("height", "4"),
                ("roof:height", "3"),
                ("roof:shape", "gabled"),
            ]),
        };
        let (h, _) = calculate_building_height(&part, "yes", 0, 1.0, None, 4, 100, 1);
        assert_eq!(h, 1);
    }

    #[test]
    fn outward_normals_point_out_of_every_wall_of_an_l_shape() {
        // Bar 0..20 x 0..4 plus a leg 0..4 x 0..20. The centroid of the
        // vertices lies in the notch, which used to flip the two inner walls.
        let nodes = ring(&[(0, 0), (20, 0), (20, 4), (4, 4), (4, 20), (0, 20)]);
        let pts: Vec<(f64, f64)> = nodes.iter().map(|n| (n.x as f64, n.z as f64)).collect();
        let outward = outward_side(&nodes);
        for pair in nodes.windows(2) {
            let (x1, z1, x2, z2) = (pair[0].x, pair[0].z, pair[1].x, pair[1].z);
            let (nx, nz) = compute_outward_normal(x1, z1, x2, z2, outward);
            let (mx, mz) = ((x1 + x2) as f64 / 2.0, (z1 + z2) as f64 / 2.0);
            assert!(
                !polygon_contains(&pts, mx + nx as f64 * 0.5, mz + nz as f64 * 0.5),
                "wall ({x1},{z1})-({x2},{z2}) normal ({nx},{nz}) points inside"
            );
        }
        // The other winding gives the same answer.
        let reversed: Vec<ProcessedNode> = nodes.iter().rev().cloned().collect();
        let outward = outward_side(&reversed);
        let (nx, nz) = compute_outward_normal(4, 4, 20, 4, outward);
        assert_eq!((nx, nz), (0, 1));
    }

    #[test]
    fn corners_are_convex_turns_only() {
        let nodes = ring(&[(0, 0), (10, 0), (20, 0), (20, 4), (4, 4), (4, 20), (0, 20)]);
        let outward = outward_side(&nodes);
        assert!(corner_is_convex((0, 0), (20, 0), (20, 4), outward));
        // Mid-wall node.
        assert!(!corner_is_convex((0, 0), (10, 0), (20, 0), outward));
        // Re-entrant corner of the L.
        assert!(!corner_is_convex((20, 4), (4, 4), (4, 20), outward));
        assert_eq!(ring_vertices(&nodes).len(), 7);
    }

    #[test]
    fn thin_rings_stand_on_their_outline() {
        // A one-metre column: every node lands on one or two cells.
        let column = ring(&[(5, 5), (6, 5), (6, 6), (5, 6)]);
        let cells = thin_ring_cells(&column);
        assert!(!cells.is_empty());
        assert!(cells.len() <= THIN_RING_MAX_CELLS);
        // A degenerate ring on a single cell still stands.
        let dot = ring(&[(5, 5), (5, 5), (5, 5)]);
        assert_eq!(thin_ring_cells(&dot), vec![(5, 5)]);
        // Open ways are not footprints.
        let open: Vec<ProcessedNode> = column[..3].to_vec();
        assert!(thin_ring_cells(&open).is_empty());
    }

    #[test]
    fn ground_level_parts_keep_a_thin_mapped_height() {
        let plinth = ProcessedWay {
            id: 1,
            nodes: vec![],
            tags: tag_map(&[("building:part", "yes"), ("height", "1")]),
        };
        let (h, _) = calculate_building_height(&plinth, "yes", 0, 1.0, None, 4, 100, 1);
        assert_eq!(h, 1);
        // A whole building still gets the interior minimum.
        let hut = ProcessedWay {
            id: 2,
            nodes: vec![],
            tags: tag_map(&[("building", "yes"), ("height", "1")]),
        };
        let (h, _) = calculate_building_height(&hut, "yes", 0, 1.0, None, 3, 100, 1);
        assert_eq!(h, 3);
    }
}
