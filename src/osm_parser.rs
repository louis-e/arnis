use crate::clipping::{clip_way_to_bbox, clip_way_to_bbox_owned, mark_cut_ends};
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use fnv::{FnvHashMap, FnvHashSet};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

// Tags Arnis never reads. Filtered at parse time to save memory.
const IGNORED_TAGS: &[&str] = &[
    "created_by",
    "note",
    "fixme",
    "FIXME",
    "todo",
    "TODO",
    "wikipedia",
    "wikimedia_commons",
    "import_uuid",
    "import",
    "old_name",
    "loc_name",
    "official_name",
    "alt_name",
    "operator",
    "phone",
    "fax",
    "email",
    "url",
    "website",
    "opening_hours",
    "description",
    "attribution",
    "start_date",
    "check_date",
    "survey:date",
    "ref:bag",
    "ref:bygningsnr",
];

// Tag-key prefixes Arnis never reads (localized names, addresses, regional import refs).
const IGNORED_PREFIXES: &[&str] = &[
    "addr:",
    "source",
    "name:",
    "alt_name:",
    "contact:",
    "is_in:",
    "operator:",
    "tiger:",
    "NHD:",
    "lacounty:",
    "nysgissam:",
    "ref:ruian:",
    "building:ruian:",
    "osak:",
    "gnis:",
    "yh:",
    "check_date:",
];

fn filter_tags(mut tags: HashMap<String, String>) -> HashMap<String, String> {
    // start_date is otherwise filtered, but on buildings the construction year picks the facade style.
    let keep_start_date = tags.contains_key("building") || tags.contains_key("building:part");
    tags.retain(|k, _| {
        if k == "start_date" {
            return keep_start_date;
        }
        // The house number feeds the door plate; the rest of addr:* stays out.
        if k == "addr:housenumber" {
            return true;
        }
        !IGNORED_TAGS.contains(&k.as_str()) && !IGNORED_PREFIXES.iter().any(|p| k.starts_with(p))
    });
    // Proposed and razed roads don't exist, and neither do their bridges or tunnels.
    if matches!(
        tags.get("highway").map(String::as_str),
        Some("proposed" | "razed")
    ) {
        tags.remove("highway");
        if !tags.contains_key("railway") {
            for key in ["bridge", "tunnel", "layer"] {
                tags.remove(key);
            }
        }
    }
    if tags
        .get("natural")
        .is_some_and(|natural| is_landform(natural))
    {
        tags.remove("natural");
    }
    tags
}

/// Natural values naming a region, like a peninsula, rather than what covers the ground.
fn is_landform(natural: &str) -> bool {
    matches!(
        natural,
        "peninsula"
            | "isthmus"
            | "cape"
            | "valley"
            | "plateau"
            | "massif"
            | "mountain_range"
            | "gorge"
            | "landform"
            | "plain"
            | "arete"
            | "couloir"
            | "coastline"
    )
}

// Raw data from OSM

#[derive(Debug, Deserialize, Serialize)]
pub struct OsmMember {
    pub r#type: String,
    pub r#ref: u64,
    pub r#role: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct OsmElement {
    pub r#type: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Vec<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<OsmMember>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct OsmData {
    elements: Vec<OsmElement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remark: Option<String>,
    #[serde(default, rename = "bounds", skip_serializing_if = "Option::is_none")]
    saved_bounds: Option<SavedBounds>,
}

/// The `bounds` layout of the OSM API's JSON.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
struct SavedBounds {
    minlat: f64,
    minlon: f64,
    maxlat: f64,
    maxlon: f64,
}

impl OsmData {
    /// Returns true if there are no elements in the OSM data
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    /// Dataset assembled outside the Overpass path (the tile archive), bypassing serde.
    pub fn from_elements(elements: Vec<OsmElement>) -> Self {
        OsmData {
            elements,
            remark: None,
            saved_bounds: None,
        }
    }

    pub fn elements(&self) -> &[OsmElement] {
        &self.elements
    }

    /// Object-free dataset, used by terrain-only runs that never query Overpass.
    pub fn empty() -> Self {
        OsmData {
            elements: Vec::new(),
            remark: None,
            saved_bounds: None,
        }
    }

    /// The requested area, which a saved copy regenerates instead of the wider node extent.
    pub fn set_saved_bounds(&mut self, bbox: &LLBBox) {
        self.saved_bounds = Some(SavedBounds {
            minlat: bbox.min().lat(),
            minlon: bbox.min().lng(),
            maxlat: bbox.max().lat(),
            maxlon: bbox.max().lng(),
        });
    }

    pub fn saved_bounds(&self) -> Option<LLBBox> {
        let b = self.saved_bounds?;
        LLBBox::new(b.minlat, b.minlon, b.maxlat, b.maxlon).ok()
    }

    /// Bounding box spanning every node with coordinates, or None when the data has no
    /// locatable nodes (or they are collinear/degenerate). Used as the fallback bbox for
    /// local `.osm` files that omit the `<bounds>` element.
    pub fn bounds(&self) -> Option<LLBBox> {
        let mut min_lat = f64::INFINITY;
        let mut min_lng = f64::INFINITY;
        let mut max_lat = f64::NEG_INFINITY;
        let mut max_lng = f64::NEG_INFINITY;
        let mut found = false;
        for element in &self.elements {
            if let (Some(lat), Some(lon)) = (element.lat, element.lon) {
                min_lat = min_lat.min(lat);
                max_lat = max_lat.max(lat);
                min_lng = min_lng.min(lon);
                max_lng = max_lng.max(lon);
                found = true;
            }
        }
        if !found {
            return None;
        }
        // LLBBox::new rejects a zero-area (min == max) box; treat that as "no usable bounds".
        LLBBox::new(min_lat, min_lng, max_lat, max_lng).ok()
    }
}

/// Parses a raw OSM XML (`.osm`) document into the same [`OsmData`] shape produced by the
/// Overpass `[out:json]` path, so the entire downstream pipeline is reused unchanged.
///
/// Returns the parsed data plus the `<bounds>` element as an [`LLBBox`] when the document
/// carries one. The caller decides bbox precedence via [`resolve_bbox`]; surfacing the
/// explicit `<bounds>` here keeps it from being silently replaced by the node-extent fallback.
pub fn parse_osm_xml(
    reader: impl std::io::BufRead,
) -> Result<(OsmData, Option<LLBBox>), Box<dyn std::error::Error>> {
    use quick_xml::events::{BytesStart, Event};
    use quick_xml::reader::Reader;

    // Single-attribute lookup, decoding XML entities in the value. Element attribute
    // counts are tiny, re-scan is cheap.
    fn attr_string(
        e: &BytesStart,
        key: &[u8],
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        for attr in e.attributes() {
            let attr = attr?;
            if attr.key.into_inner() == key {
                // normalized_value resolves predefined entities (&amp; -> & etc.); Implicit1_0
                // applies XML 1.0 attribute-value normalization for OSM files.
                return Ok(Some(
                    attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)?
                        .into_owned(),
                ));
            }
        }
        Ok(None)
    }
    fn attr_u64(e: &BytesStart, key: &[u8]) -> Result<Option<u64>, Box<dyn std::error::Error>> {
        match attr_string(e, key)? {
            Some(s) => Ok(Some(s.trim().parse()?)),
            None => Ok(None),
        }
    }
    fn attr_f64(e: &BytesStart, key: &[u8]) -> Result<Option<f64>, Box<dyn std::error::Error>> {
        match attr_string(e, key)? {
            Some(s) => Ok(Some(s.trim().parse()?)),
            None => Ok(None),
        }
    }

    // The node/way/relation currently being assembled from its child events.
    enum Building {
        Node {
            id: u64,
            lat: Option<f64>,
            lon: Option<f64>,
            tags: HashMap<String, String>,
        },
        Way {
            id: u64,
            nodes: Vec<u64>,
            tags: HashMap<String, String>,
        },
        Relation {
            id: u64,
            members: Vec<OsmMember>,
            tags: HashMap<String, String>,
        },
    }

    // Mirror Overpass JSON: an element with no tags deserializes to `None`, not an empty map.
    fn finish(b: Building) -> OsmElement {
        let opt = |t: HashMap<String, String>| if t.is_empty() { None } else { Some(t) };
        match b {
            Building::Node { id, lat, lon, tags } => OsmElement {
                r#type: "node".to_string(),
                id,
                lat,
                lon,
                nodes: None,
                tags: opt(tags),
                members: Vec::new(),
            },
            Building::Way { id, nodes, tags } => OsmElement {
                r#type: "way".to_string(),
                id,
                lat: None,
                lon: None,
                nodes: Some(nodes),
                tags: opt(tags),
                members: Vec::new(),
            },
            Building::Relation { id, members, tags } => OsmElement {
                r#type: "relation".to_string(),
                id,
                lat: None,
                lon: None,
                nodes: None,
                tags: opt(tags),
                members,
            },
        }
    }

    // Handles a start/empty tag. `is_empty` is true for self-closing tags (no child events,
    // so node/way/relation are finalized immediately rather than on a matching end tag).
    fn on_open(
        e: &BytesStart,
        is_empty: bool,
        elements: &mut Vec<OsmElement>,
        bounds: &mut Option<LLBBox>,
        building: &mut Option<Building>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match e.name().into_inner() {
            b"bounds" => {
                if let (Some(minlat), Some(minlon), Some(maxlat), Some(maxlon)) = (
                    attr_f64(e, b"minlat")?,
                    attr_f64(e, b"minlon")?,
                    attr_f64(e, b"maxlat")?,
                    attr_f64(e, b"maxlon")?,
                ) {
                    // Propagate LLBBox::new's validation error
                    *bounds = Some(LLBBox::new(minlat, minlon, maxlat, maxlon)?);
                }
            }
            b"node" => {
                let id = attr_u64(e, b"id")?.ok_or("OSM node missing required id attribute")?;
                let node = Building::Node {
                    id,
                    lat: attr_f64(e, b"lat")?,
                    lon: attr_f64(e, b"lon")?,
                    tags: HashMap::new(),
                };
                if is_empty {
                    elements.push(finish(node));
                } else {
                    *building = Some(node);
                }
            }
            b"way" => {
                let id = attr_u64(e, b"id")?.ok_or("OSM way missing required id attribute")?;
                let way = Building::Way {
                    id,
                    nodes: Vec::new(),
                    tags: HashMap::new(),
                };
                if is_empty {
                    elements.push(finish(way));
                } else {
                    *building = Some(way);
                }
            }
            b"relation" => {
                let id = attr_u64(e, b"id")?.ok_or("OSM relation missing required id attribute")?;
                let relation = Building::Relation {
                    id,
                    members: Vec::new(),
                    tags: HashMap::new(),
                };
                if is_empty {
                    elements.push(finish(relation));
                } else {
                    *building = Some(relation);
                }
            }
            b"tag" => {
                if let Some(b) = building.as_mut() {
                    if let (Some(k), Some(v)) = (attr_string(e, b"k")?, attr_string(e, b"v")?) {
                        match b {
                            Building::Node { tags, .. }
                            | Building::Way { tags, .. }
                            | Building::Relation { tags, .. } => {
                                tags.insert(k, v);
                            }
                        }
                    }
                }
            }
            b"nd" => {
                if let Some(Building::Way { nodes, .. }) = building.as_mut() {
                    if let Some(node_ref) = attr_u64(e, b"ref")? {
                        nodes.push(node_ref);
                    }
                }
            }
            b"member" => {
                if let Some(Building::Relation { members, .. }) = building.as_mut() {
                    members.push(OsmMember {
                        r#type: attr_string(e, b"type")?.unwrap_or_default(),
                        r#ref: attr_u64(e, b"ref")?.unwrap_or_default(),
                        r#role: attr_string(e, b"role")?.unwrap_or_default(),
                    });
                }
            }
            _ => {}
        }
        Ok(())
    }

    let mut xml = Reader::from_reader(reader);
    let mut buf: Vec<u8> = Vec::new();
    let mut elements: Vec<OsmElement> = Vec::new();
    let mut bounds: Option<LLBBox> = None;
    let mut building: Option<Building> = None;

    loop {
        match xml.read_event_into(&mut buf)? {
            Event::Eof => break,
            Event::Start(e) => on_open(&e, false, &mut elements, &mut bounds, &mut building)?,
            Event::Empty(e) => on_open(&e, true, &mut elements, &mut bounds, &mut building)?,
            Event::End(e) => {
                if matches!(e.name().into_inner(), b"node" | b"way" | b"relation") {
                    if let Some(b) = building.take() {
                        elements.push(finish(b));
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }

    Ok((
        OsmData {
            elements,
            remark: None,
            saved_bounds: None,
        },
        bounds,
    ))
}

/// Resolves the effective bounding box with precedence: explicit `--bbox` > file `<bounds>`
/// element > node-coordinate extent. Returns None only when no source yields a usable box.
pub fn resolve_bbox(
    explicit: Option<LLBBox>,
    file_bounds: Option<LLBBox>,
    data: &OsmData,
) -> Option<LLBBox> {
    explicit.or(file_bounds).or_else(|| data.bounds())
}

struct SplitOsmData {
    pub nodes: Vec<OsmElement>,
    pub ways: Vec<OsmElement>,
    pub relations: Vec<OsmElement>,
    #[allow(dead_code)]
    pub others: Vec<OsmElement>,
}

impl SplitOsmData {
    fn total_count(&self) -> usize {
        self.nodes.len() + self.ways.len() + self.relations.len() + self.others.len()
    }
    fn from_raw_osm_data(osm_data: OsmData) -> Self {
        let mut nodes = Vec::new();
        let mut ways = Vec::new();
        let mut relations = Vec::new();
        let mut others = Vec::new();
        for element in osm_data.elements {
            match element.r#type.as_str() {
                "node" => nodes.push(element),
                "way" => ways.push(element),
                "relation" => relations.push(element),
                _ => others.push(element),
            }
        }
        SplitOsmData {
            nodes,
            ways,
            relations,
            others,
        }
    }
}

// End raw data

// Normalized data that we can use

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedNode {
    pub id: u64,
    pub tags: HashMap<String, String>,

    // Minecraft coordinates
    pub x: i32,
    pub z: i32,
}

impl ProcessedNode {
    pub fn xz(&self) -> XZPoint {
        XZPoint {
            x: self.x,
            z: self.z,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedWay {
    pub id: u64,
    pub nodes: Vec<ProcessedNode>,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, PartialEq, Clone)]
pub enum ProcessedMemberRole {
    Outer,
    Inner,
    Part,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedMember {
    pub role: ProcessedMemberRole,
    pub way: Arc<ProcessedWay>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedRelation {
    pub id: u64,
    pub tags: HashMap<String, String>,
    pub members: Vec<ProcessedMember>,
}

#[derive(Debug, Clone)]
pub enum ProcessedElement {
    Node(ProcessedNode),
    Way(ProcessedWay),
    Relation(ProcessedRelation),
}

impl ProcessedElement {
    pub fn tags(&self) -> &HashMap<String, String> {
        match self {
            ProcessedElement::Node(n) => &n.tags,
            ProcessedElement::Way(w) => &w.tags,
            ProcessedElement::Relation(r) => &r.tags,
        }
    }

    pub fn id(&self) -> u64 {
        match self {
            ProcessedElement::Node(n) => n.id,
            ProcessedElement::Way(w) => w.id,
            ProcessedElement::Relation(r) => r.id,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            ProcessedElement::Node(_) => "node",
            ProcessedElement::Way(_) => "way",
            ProcessedElement::Relation(_) => "relation",
        }
    }

    pub fn nodes<'a>(&'a self) -> Box<dyn Iterator<Item = &'a ProcessedNode> + 'a> {
        match self {
            ProcessedElement::Node(node) => Box::new([node].into_iter()),
            ProcessedElement::Way(way) => Box::new(way.nodes.iter()),
            ProcessedElement::Relation(_) => Box::new([].into_iter()),
        }
    }
}

pub type OutlineSuppression = HashSet<(&'static str, u64)>;

// building:part way id -> shared style seed (containing outline id, or salted relation id)
pub type PartGroups = HashMap<u64, u64>;

// keeps relation-derived seeds out of the way-id namespace
const RELATION_SEED_BIT: u64 = 1 << 63;

// 2-bit facade-style hint packed into a part's shared seed (bits 61-62)
const STYLE_HINT_SHIFT: u64 = 61;
const STYLE_HINT_MASK: u64 = 0b11 << STYLE_HINT_SHIFT;

/// Facade-style hint derived from a building's OSM tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleHint {
    None = 0,
    Masonry = 1,      // historic / stone / brick
    Contemporary = 2, // concrete frame, modern
    Glass = 3,        // self-declared glass curtain
}

/// Reads the packed style hint back out of a shared seed.
pub fn style_hint_from_seed(seed: u64) -> StyleHint {
    match (seed & STYLE_HINT_MASK) >> STYLE_HINT_SHIFT {
        1 => StyleHint::Masonry,
        2 => StyleHint::Contemporary,
        3 => StyleHint::Glass,
        _ => StyleHint::None,
    }
}

/// The seed with its style-hint bits cleared (used for the random variant roll).
pub fn seed_without_hint(seed: u64) -> u64 {
    seed & !(STYLE_HINT_MASK | RESIDENTIAL_GROUP_BIT | YEAR_MASK)
}

// Set on the shared seed of parts whose building is housing (bit 60)
const RESIDENTIAL_GROUP_BIT: u64 = 1 << 60;

/// Whether a part's shared seed marks its building as housing, whose
/// levels-only parts take the residential storey height.
pub fn residential_group_from_seed(seed: u64) -> bool {
    seed & RESIDENTIAL_GROUP_BIT != 0
}

/// The seed without the housing mark, so every roll a part makes stays the
/// one it made before the mark existed.
pub fn seed_without_residential_mark(seed: u64) -> u64 {
    seed & !RESIDENTIAL_GROUP_BIT
}

fn seed_with_residential_mark(seed: u64, residential: bool) -> u64 {
    if residential {
        seed | RESIDENTIAL_GROUP_BIT
    } else {
        seed
    }
}

/// Building types built on the residential storey height: flats, houses and
/// their outbuildings.
fn is_housing_type(tags: &HashMap<String, String>) -> bool {
    matches!(
        tags.get("building").map(String::as_str),
        Some(
            "residential"
                | "apartments"
                | "house"
                | "detached"
                | "semidetached_house"
                | "terrace"
                | "dormitory"
                | "bungalow"
                | "villa"
        )
    )
}

/// The key the parts of one building are grouped under: the seed without its
/// packed style hint, year and housing mark, which two parts of one building
/// may not share.
pub fn group_key(seed: u64) -> u64 {
    seed & !(STYLE_HINT_MASK | YEAR_MASK | RESIDENTIAL_GROUP_BIT)
}

fn seed_with_hint(seed: u64, hint: StyleHint) -> u64 {
    (seed & !STYLE_HINT_MASK) | ((hint as u64) << STYLE_HINT_SHIFT)
}

// Construction year of the building a part belongs to, packed into the part's
// shared seed (bits 52-59) as years after 1800, so a start_date mapped only on
// the outline still dates its parts. 0 is no year.
const YEAR_SHIFT: u64 = 52;
const YEAR_MASK: u64 = 0xFF << YEAR_SHIFT;
const YEAR_BASE: i32 = 1800;

/// Reads the packed construction year of the part's building back out of a shared seed.
pub fn year_from_seed(seed: u64) -> Option<i32> {
    let packed = ((seed & YEAR_MASK) >> YEAR_SHIFT) as i32;
    (packed > 0).then_some(YEAR_BASE + packed)
}

/// The seed with its year bits cleared, which is the seed the building's own
/// rolls are made with: dating a building must not reroll its style.
pub fn seed_without_year(seed: u64) -> u64 {
    seed & !YEAR_MASK
}

fn seed_with_year(seed: u64, year: Option<i32>) -> u64 {
    let packed = year.map_or(0, |y| (y - YEAR_BASE).clamp(1, 255) as u64);
    (seed & !YEAR_MASK) | (packed << YEAR_SHIFT)
}

// lowercase, strip whitespace/_/- so art_deco, neo-gothic, "concrete masonry unit" all collapse
fn norm_tag(v: &str) -> String {
    v.chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// The mapped architectural style, normalised; the first of a `;` list.
fn architecture(tags: &HashMap<String, String>) -> Option<String> {
    tags.get("building:architecture")
        .or_else(|| tags.get("architecture"))
        .map(|a| norm_tag(a.split(';').next().unwrap_or(a)))
}

/// Whether the building is mapped as art deco.
pub fn is_art_deco(tags: &HashMap<String, String>) -> bool {
    architecture(tags).as_deref() == Some("artdeco")
}

/// Year the building went up, from the first of start_date, construction_date
/// and year_of_construction that holds one.
pub fn building_start_year(tags: &HashMap<String, String>) -> Option<i32> {
    ["start_date", "construction_date", "year_of_construction"]
        .iter()
        .find_map(|key| tags.get(*key).and_then(|v| first_year(v)))
}

// First 4-digit year in a date-ish value, e.g. "1911-1913" -> 1911, "1955-12-31" -> 1955.
fn first_year(v: &str) -> Option<i32> {
    let bytes = v.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..i + 4].iter().all(|b| b.is_ascii_digit()) {
            return v[i..i + 4].parse().ok();
        }
        i += 1;
    }
    None
}

// Brutalist styles are masonry for StyleHint but post-war for ArchEra.
const ORNATE_STYLES: &[&str] = &[
    "artdeco",
    "artnouveau",
    "gothic",
    "neogothic",
    "gothicrevival",
    "neoclassicism",
    "neoclassical",
    "classicism",
    "classicalrevival",
    "greekrevival",
    "baroque",
    "neobaroque",
    "rococo",
    "barocco",
    "historicism",
    "eclectic",
    "renaissance",
    "neorenaissance",
    "romanesque",
    "neoromanesque",
    "romanesquerevival",
    "victorian",
    "georgian",
    "federal",
    "italianate",
    "beauxarts",
    "wilhelminianstyle",
    "queenanne",
];
const BRUTALIST_STYLES: &[&str] = &["brutalist", "constructivism", "stalinistneoclassicism"];
const MODERN_STYLES: &[&str] = &[
    "modern",
    "contemporary",
    "modernism",
    "functionalism",
    "newobjectivity",
    "postmodern",
    "bauhaus",
];
const MASONRY_MATERIALS: &[&str] = &[
    "brick",
    "bricks",
    "redbrick",
    "silicatebrick",
    "stone",
    "naturalstone",
    "sandstone",
    "limestone",
    "masonry",
    "granite",
    "marble",
    "terracotta",
    "adobe",
    "stucco",
    "pebbledash",
];
const MASONRY_CLADDING: &[&str] = &[
    "brick",
    "brickmonolith",
    "plaster",
    "rendered",
    "rendering",
    "stone",
    "tiling",
];
const CONCRETE_MATERIALS: &[&str] = &[
    "concrete",
    "reinforcedconcrete",
    "concretereinforced",
    "concretemasonryunit",
];
const PANEL_MATERIALS: &[&str] = &["panel", "panels", "prefab", "prefabricated", "panelhouse"];

/// Picks a facade style for a building from its OSM tags, or None to leave it to the random roll.
pub fn building_style_hint(tags: &HashMap<String, String>) -> StyleHint {
    let material = tags
        .get("building:material")
        .or_else(|| tags.get("building:facade:material"))
        .or_else(|| tags.get("facade:material"))
        .map(|m| norm_tag(m));

    // Glass override wins over everything, so heritage-listed glass towers stay glass.
    if material.as_deref() == Some("glass") || material.as_deref() == Some("mirror") {
        return StyleHint::Glass;
    }
    if tags.get("roof:material").map(|r| norm_tag(r)).as_deref() == Some("glass") {
        return StyleHint::Glass;
    }

    // Masonry / historic, unless the listing is for a building of the modern
    // movement or one built after the war.
    let arch = architecture(tags);
    let year = building_start_year(tags);
    if is_listed(tags) && !listed_for_modern_design(tags, arch.as_deref(), year) {
        return StyleHint::Masonry;
    }
    if material
        .as_deref()
        .is_some_and(|m| MASONRY_MATERIALS.contains(&m))
    {
        return StyleHint::Masonry;
    }
    if let Some(c) = tags.get("building:cladding") {
        if MASONRY_CLADDING.contains(&norm_tag(c).as_str()) {
            return StyleHint::Masonry;
        }
    }
    if let Some(a) = arch.as_deref() {
        if ORNATE_STYLES.contains(&a) || BRUTALIST_STYLES.contains(&a) {
            return StyleHint::Masonry;
        }
        if MODERN_STYLES.contains(&a) {
            return StyleHint::Contemporary;
        }
    }
    // Pre-curtain-wall era: load-bearing masonry. start_date is the best-populated source.
    if year.is_some_and(|y| y < 1945) {
        return StyleHint::Masonry;
    }

    // Concrete frame reads as a solid facade with windows: the contemporary middle style.
    if material
        .as_deref()
        .is_some_and(|m| CONCRETE_MATERIALS.contains(&m))
    {
        return StyleHint::Contemporary;
    }
    StyleHint::None
}

/// Architectural era, consumed by low-rise styling. Recomputed from tags
/// per building, unlike the seed-packed `StyleHint`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchEra {
    Unknown,
    /// Explicit heritage or ornate-architecture signals.
    HistoricOrnate,
    /// Pre-1945 masonry-era fabric without ornate signals.
    TraditionalPreWar,
    /// 1945–1979: panel, prefab, brutalist.
    PostWarPanel,
    /// 1980+: concrete/glass contemporary.
    Contemporary,
}

/// Whether the building is listed or marked historic. `no` on these keys is
/// an explicit negation, not a signal.
fn is_listed(tags: &HashMap<String, String>) -> bool {
    let present_and_not_no =
        |key: &str| tags.get(key).is_some_and(|v| !v.eq_ignore_ascii_case("no"));
    present_and_not_no("historic")
        || present_and_not_no("heritage")
        || tags.contains_key("ref:nrhp")
        || present_and_not_no("listed_status")
}

/// Whether a listed building is listed for a modern design: mapped in a
/// modern style, or dated after the war. A listed church dated after the
/// war was far more often rebuilt in its old form, so its date does not
/// count.
fn listed_for_modern_design(
    tags: &HashMap<String, String>,
    arch: Option<&str>,
    year: Option<i32>,
) -> bool {
    arch.is_some_and(|a| MODERN_STYLES.contains(&a))
        || (year.is_some_and(|y| y >= 1945) && !is_place_of_worship(tags))
}

fn is_place_of_worship(tags: &HashMap<String, String>) -> bool {
    tags.get("amenity").is_some_and(|a| a == "place_of_worship")
        || tags.get("building").is_some_and(|b| {
            matches!(
                b.as_str(),
                "church"
                    | "cathedral"
                    | "chapel"
                    | "basilica"
                    | "mosque"
                    | "synagogue"
                    | "temple"
                    | "shrine"
                    | "monastery"
            )
        })
}

/// The year a building passes on to its parts: its own date, unless the
/// date is later than its style, as on a church rebuilt in its old form.
fn year_for_parts(tags: &HashMap<String, String>) -> Option<i32> {
    let year = building_start_year(tags)?;
    (year < 1945
        || matches!(
            building_arch_era_dated(tags, Some(year)),
            ArchEra::PostWarPanel | ArchEra::Contemporary
        ))
    .then_some(year)
}

/// Era of a building put up in `year`.
pub fn era_for_year(year: i32) -> ArchEra {
    if year < 1945 {
        ArchEra::TraditionalPreWar
    } else if year < 1980 {
        ArchEra::PostWarPanel
    } else {
        ArchEra::Contemporary
    }
}

/// Classifies a building's era from its tags; `Unknown` when nothing signals.
#[cfg(test)]
pub fn building_arch_era(tags: &HashMap<String, String>) -> ArchEra {
    building_arch_era_dated(tags, building_start_year(tags))
}

/// Era of a building from its tags, with `year` standing in for the date
/// tags: a part takes the year of the building it belongs to.
pub fn building_arch_era_dated(tags: &HashMap<String, String>, year: Option<i32>) -> ArchEra {
    // A named style says the most. A modern one keeps to its date: interwar
    // modernism is still a punched masonry wall, postwar modernism a panel.
    let arch = architecture(tags);
    if let Some(a) = arch.as_deref() {
        if ORNATE_STYLES.contains(&a) {
            return ArchEra::HistoricOrnate;
        }
        if BRUTALIST_STYLES.contains(&a) {
            return ArchEra::PostWarPanel;
        }
        if MODERN_STYLES.contains(&a) {
            return year.map_or(ArchEra::Contemporary, era_for_year);
        }
    }

    // A listing makes the building ornate unless it dates from after the war.
    if is_listed(tags) {
        return match year {
            Some(y) if listed_for_modern_design(tags, None, Some(y)) => era_for_year(y),
            _ => ArchEra::HistoricOrnate,
        };
    }

    if let Some(y) = year {
        return era_for_year(y);
    }

    let material = tags
        .get("building:material")
        .or_else(|| tags.get("building:facade:material"))
        .or_else(|| tags.get("facade:material"))
        .map(|m| norm_tag(m));
    if let Some(m) = material.as_deref() {
        if PANEL_MATERIALS.contains(&m) {
            return ArchEra::PostWarPanel;
        }
        if MASONRY_MATERIALS.contains(&m) {
            return ArchEra::TraditionalPreWar;
        }
        if CONCRETE_MATERIALS.contains(&m) || m == "glass" || m == "mirror" {
            return ArchEra::Contemporary;
        }
    }
    if let Some(c) = tags.get("building:cladding") {
        if MASONRY_CLADDING.contains(&norm_tag(c).as_str()) {
            return ArchEra::TraditionalPreWar;
        }
    }
    ArchEra::Unknown
}

/// Fallback era for untagged `building:part`s from the packed group-seed hint.
pub fn arch_era_from_hint(hint: StyleHint) -> ArchEra {
    match hint {
        StyleHint::Masonry => ArchEra::TraditionalPreWar,
        StyleHint::Contemporary | StyleHint::Glass => ArchEra::Contemporary,
        StyleHint::None => ArchEra::Unknown,
    }
}

fn way_touches_rect(nodes: &[ProcessedNode], rect: &XZBBox) -> bool {
    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_z = i32::MAX;
    let mut max_z = i32::MIN;
    for n in nodes {
        min_x = min_x.min(n.x);
        max_x = max_x.max(n.x);
        min_z = min_z.min(n.z);
        max_z = max_z.max(n.z);
    }
    !nodes.is_empty()
        && max_x >= rect.min_x()
        && min_x <= rect.max_x()
        && max_z >= rect.min_z()
        && min_z <= rect.max_z()
}

pub fn parse_osm_data(
    osm_data: OsmData,
    bbox: LLBBox,
    debug: bool,
    projection: &crate::projection::ProjectionSpec,
) -> (
    Vec<ProcessedElement>,
    XZBBox,
    OutlineSuppression,
    PartGroups,
) {
    println!("{} Parsing data...", "[2/7]".bold());
    println!("Bounding box: {bbox:?}");

    // Deserialize the JSON data into the OSMData structure
    let mut data = SplitOsmData::from_raw_osm_data(osm_data);

    let (coord_transformer, xzbbox) = projection
        .transformer(&bbox)
        // Panics rather than exits: the GUI calls this from a Tauri blocking task, where an
        // exit would take the whole app down. Bad scales are rejected up front by validate_scale.
        .unwrap_or_else(|e| {
            panic!("Error in defining coordinate transformation:\n{e}");
        });
    // Ways lying only in the clip margin cannot put a block into the world.
    let clip_bbox = projection.clip_bbox(&xzbbox);
    let touches_world = |nodes: &[ProcessedNode]| -> bool {
        projection.clip_pad <= 0 || way_touches_rect(nodes, &xzbbox)
    };

    if debug {
        println!("Total elements: {}", data.total_count());
        println!("Scale factor X: {}", coord_transformer.scale_factor_x());
        println!("Scale factor Z: {}", coord_transformer.scale_factor_z());
    }

    let mut part_groups = PartGroups::new();
    let mut outline_suppression =
        compute_outline_suppression(&data.relations, &data.ways, &data.nodes, &mut part_groups);
    // Ways owned by a type=building relation are handled above; the spatial pass must
    // not re-judge them against unrelated parts that merely fall inside their footprint.
    let relation_ways: HashSet<u64> = data
        .relations
        .iter()
        .filter(|r| {
            r.tags
                .as_ref()
                .and_then(|t| t.get("type"))
                .map(|t| t == "building")
                .unwrap_or(false)
        })
        .flat_map(|r| {
            r.members
                .iter()
                .filter(|m| m.r#type == "way")
                .map(|m| m.r#ref)
        })
        .collect();
    // also catch S3DB outlines mapped without a relation
    outline_suppression.extend(compute_spatial_part_suppression(
        &data.ways,
        &data.nodes,
        &relation_ways,
        &mut part_groups,
    ));
    // S3DB outlines expressed as multipolygon RELATIONS (e.g. an amphitheatre whose tiers are
    // separate building:part relations) are invisible to both passes above: the membership
    // pass only links parts that are relation MEMBERS, and the way pass only scans closed ways.
    // Judge them spatially so a relation outline covered by part relations is dropped.
    outline_suppression.extend(spatial_relation_part_suppression(
        &data.relations,
        &data.ways,
        &data.nodes,
        &mut part_groups,
    ));
    // A ride drawn whole replaces any 3D parts mapped inside it, and is never replaced by them.
    suppress_ride_parts(&data.ways, &data.nodes, &mut outline_suppression);
    inherit_masonry_facade_tags(&mut data.ways, &data.relations, &part_groups);

    let mut nodes_map: FnvHashMap<u64, ProcessedNode> =
        FnvHashMap::with_capacity_and_hasher(data.nodes.len(), Default::default());
    // Only relation members are looked up again, so only they keep an unclipped copy.
    let relation_way_ids: FnvHashSet<u64> = data
        .relations
        .iter()
        .flat_map(|r| r.members.iter())
        .filter(|m| m.r#type == "way")
        .map(|m| m.r#ref)
        .collect();
    let mut ways_map: HashMap<u64, Arc<ProcessedWay>> = HashMap::new();
    let mut seen_ways: FnvHashSet<u64> = FnvHashSet::default();
    // Outer ways tagged like the relation that draws them less its holes.
    let mut repeated_outers: FnvHashSet<u64> = FnvHashSet::default();

    let mut processed_elements: Vec<ProcessedElement> = Vec::new();

    // First pass: store all nodes with Minecraft coordinates and process nodes with tags
    for element in data.nodes {
        // Overpass emits elements again per matching relation; keep the first copy only.
        if nodes_map.contains_key(&element.id) {
            continue;
        }
        if let (Some(lat), Some(lon)) = (element.lat, element.lon) {
            let llpoint = LLPoint::new(lat, lon).unwrap_or_else(|e| {
                eprintln!("Encountered invalid node element:\n{e}");
                panic!();
            });

            let xzpoint = coord_transformer.transform_point(llpoint);

            let processed: ProcessedNode = ProcessedNode {
                id: element.id,
                tags: filter_tags(element.tags.unwrap_or_default()),
                x: xzpoint.x,
                z: xzpoint.z,
            };

            nodes_map.insert(element.id, processed.clone());

            // Only add tagged nodes to processed_elements if they're within or near the bbox
            // This significantly improves performance by filtering out distant nodes
            if !processed.tags.is_empty() && xzbbox.contains(&xzpoint) {
                processed_elements.push(ProcessedElement::Node(processed));
            }
        }
    }

    // A way whose node list is not fully resolvable is silently shortened below.
    // For a road that only bends it; for an area it breaks the ring, and an
    // unclosed ring flood-fills to nothing, so the whole polygon disappears
    // without a word. Counted here so an incomplete source is reported once
    // rather than discovered by looking at the finished world.
    let mut unresolved_node_refs: u64 = 0;
    let mut total_node_refs: u64 = 0;
    let mut ways_missing_nodes: u64 = 0;

    // Second pass: process ways and clip them to bbox
    for element in data.ways {
        if !seen_ways.insert(element.id) {
            continue;
        }
        let mut nodes: Vec<ProcessedNode> = vec![];
        if let Some(node_ids) = &element.nodes {
            let before = unresolved_node_refs;
            for &node_id in node_ids {
                total_node_refs += 1;
                match nodes_map.get(&node_id) {
                    Some(node) => nodes.push(node.clone()),
                    None => unresolved_node_refs += 1,
                }
            }
            if unresolved_node_refs > before {
                ways_missing_nodes += 1;
            }
        }

        // Clip the way to bbox to reduce node count dramatically
        let tags = filter_tags(element.tags.unwrap_or_default());

        // A lift the area cuts must not end in the station beyond the cut.
        let lift_ends = match (nodes.first(), nodes.last()) {
            (Some(a), Some(b)) if a.id != b.id && tags.contains_key("aerialway") => {
                Some([(a.x, a.z), (b.x, b.z)])
            }
            _ => None,
        };

        // Store unclipped way for relation assembly (clipping happens after ring merging)
        let (tags, mut clipped_nodes) = if relation_way_ids.contains(&element.id) {
            let way = Arc::new(ProcessedWay {
                id: element.id,
                tags,
                nodes,
            });
            ways_map.insert(element.id, Arc::clone(&way));
            (way.tags.clone(), clip_way_to_bbox(&way.nodes, &clip_bbox))
        } else {
            (tags, clip_way_to_bbox_owned(nodes, &clip_bbox))
        };
        if let Some(ends) = lift_ends {
            mark_cut_ends(&mut clipped_nodes, ends, element.id);
        }

        // Skip ways that are completely outside the bbox (empty after clipping)
        if clipped_nodes.is_empty() || !touches_world(&clipped_nodes) {
            continue;
        }

        let processed: ProcessedWay = ProcessedWay {
            id: element.id,
            tags,
            nodes: clipped_nodes,
        };

        processed_elements.push(ProcessedElement::Way(processed));
    }

    if unresolved_node_refs > 0 {
        let percent = unresolved_node_refs as f64 / total_node_refs.max(1) as f64 * 100.0;
        eprintln!(
            "{}",
            format!(
                "Warning: {unresolved_node_refs} of {total_node_refs} way node references \
                 ({percent:.1}%) could not be resolved, affecting {ways_missing_nodes} ways. \
                 Buildings and other areas that lost a corner cannot be filled and will not \
                 appear in the world. Expected near the edges of a clipped local file; from \
                 the API it means the response was incomplete."
            )
            .yellow()
            .bold()
        );
    }

    // Third pass: process relations and clip member ways
    let mut seen_relations: HashSet<u64> = HashSet::new();
    for element in data.relations {
        if !seen_relations.insert(element.id) {
            continue;
        }
        let Some(raw_tags) = &element.tags else {
            continue;
        };
        let tags = &filter_tags(raw_tags.clone());

        // Process multipolygons and building relations
        let relation_type = tags.get("type").map(|x: &String| x.as_str());
        if relation_type != Some("multipolygon") && relation_type != Some("building") {
            continue;
        };

        let is_building_relation = relation_type == Some("building")
            || tags.contains_key("building")
            || tags.contains_key("building:part");

        // Water relations require unclipped ways for ring merging in water_areas.rs
        // Building multipolygon relations also need unclipped ways so that
        // open outer-way segments can be merged into closed rings before clipping
        let is_water_relation = is_water_element(tags);
        let is_building_multipolygon = (tags.contains_key("building")
            || tags.contains_key("building:part"))
            && relation_type == Some("multipolygon");
        let keep_unclipped = is_water_relation || is_building_multipolygon;

        let roles = member_roles(
            &element.members,
            &ways_map,
            relation_type == Some("building"),
            is_building_relation,
        );

        // Natural, landuse and leisure areas are filled one outer member at a time. A ring
        // split over several ways closes only once they are joined, and clipping each way
        // on its own cuts them apart at the bbox edge, so such a relation arrived as open
        // pieces that filled to nothing and left just an outline: the big beach and dune
        // relations are mapped this way. Relations whose members are all closed skip this.
        if !keep_unclipped
            && relation_type == Some("multipolygon")
            && is_filled_area_relation(tags)
            && element.members.iter().zip(&roles).any(|(m, role)| {
                role.is_some() && ways_map.get(&m.r#ref).is_some_and(|w| !is_closed_way(w))
            })
        {
            let members = assemble_area_rings(
                element.id,
                &element.members,
                &roles,
                &ways_map,
                &clip_bbox,
                touches_world,
            );
            if !members.is_empty() {
                repeated_outers.extend(repeating_outers(
                    tags,
                    &element.members,
                    &roles,
                    &members,
                    &ways_map,
                ));
                processed_elements.push(ProcessedElement::Relation(ProcessedRelation {
                    id: element.id,
                    members,
                    tags: tags.clone(),
                }));
            }
            continue;
        }

        let members: Vec<ProcessedMember> = element
            .members
            .iter()
            .zip(&roles)
            .filter_map(|(mem, role)| {
                if mem.r#type != "way" {
                    if mem.r#type != "relation" && mem.r#type != "node" {
                        eprintln!("WARN: Unknown relation member type \"{}\"", mem.r#type);
                    }
                    return None;
                }
                let role = role.clone()?;

                // Check if the way exists in ways_map
                let way = match ways_map.get(&mem.r#ref) {
                    Some(w) => Arc::clone(w),
                    None => {
                        // Way was likely filtered out because it was completely outside the bbox
                        return None;
                    }
                };

                // If keep_unclipped is true (e.g., certain water or building multipolygon
                // relations), keep member ways unclipped for ring merging; otherwise clip now.
                let final_way = if keep_unclipped {
                    way
                } else {
                    let clipped_nodes = clip_way_to_bbox(&way.nodes, &clip_bbox);
                    if clipped_nodes.is_empty() {
                        return None;
                    }
                    Arc::new(ProcessedWay {
                        id: way.id,
                        tags: way.tags.clone(),
                        nodes: clipped_nodes,
                    })
                };

                Some(ProcessedMember {
                    role,
                    way: final_way,
                })
            })
            .collect();

        if !members.is_empty() {
            repeated_outers.extend(repeating_outers(
                tags,
                &element.members,
                &roles,
                &members,
                &ways_map,
            ));
            processed_elements.push(ProcessedElement::Relation(ProcessedRelation {
                id: element.id,
                members,
                tags: tags.clone(),
            }));
        }
    }
    if !repeated_outers.is_empty() {
        processed_elements.retain(
            |e| !matches!(e, ProcessedElement::Way(way) if repeated_outers.contains(&way.id)),
        );
    }

    emit_gui_progress_update(18.5, "");

    drop(nodes_map);
    drop(ways_map);

    (processed_elements, xzbbox, outline_suppression, part_groups)
}

// Parts replace the outline only when they cover at least this much of it.
const MIN_PART_COVERAGE: f64 = 0.5;

// Grid cell size (degrees) for the spatial index that buckets nearby outline bboxes.
const SUPPRESSION_GRID_CELL_DEG: f64 = 0.0005;

// A part covers the outline's ground footprint only if it starts at ground level.
// Elevated parts (min_height / building:min_level > 0) model raised roof/dome volumes
// that float above the ground (e.g. S3DB churches), so they can't stand in for the outline.
pub(crate) fn part_covers_ground(tags: &HashMap<String, String>) -> bool {
    // Leading number only: sign at position 0, one decimal point. So "54-60" → 54, not a parse fail.
    let leading_f64 = |s: &str| {
        let t = s.trim();
        let mut end = 0;
        let mut seen_dot = false;
        for (i, c) in t.char_indices() {
            let ok =
                c.is_ascii_digit() || (c == '.' && !seen_dot) || ((c == '-' || c == '+') && i == 0);
            if !ok {
                break;
            }
            seen_dot |= c == '.';
            end = i + c.len_utf8();
        }
        t[..end].parse::<f64>().ok()
    };
    let min_h = tags.get("min_height").and_then(|s| leading_f64(s));
    let min_lvl = tags.get("building:min_level").and_then(|s| leading_f64(s));
    min_h.unwrap_or(0.0) <= 0.0 && min_lvl.unwrap_or(0.0) <= 0.0
}

/// Facade tags a part that names no material or colour of its own takes from
/// its building: the mapper described the brick, stone or render of the whole
/// building once, on its outline or relation, and the parts are that
/// building. Glass, metal and the like stay with the outline, where the
/// group's style hint already carries them.
fn inherit_masonry_facade_tags(
    ways: &mut [OsmElement],
    relations: &[OsmElement],
    part_groups: &PartGroups,
) {
    const FACADE_KEYS: [&str; 5] = [
        "building:material",
        "building:facade:material",
        "facade:material",
        "building:colour",
        "colour",
    ];
    let masonry = |tags: &HashMap<String, String>| {
        tags.get("building:material")
            .or_else(|| tags.get("building:facade:material"))
            .is_some_and(|m| {
                matches!(
                    norm_tag(m).as_str(),
                    "brick"
                        | "bricks"
                        | "stone"
                        | "naturalstone"
                        | "limestone"
                        | "sandstone"
                        | "marble"
                        | "granite"
                        | "plaster"
                        | "stucco"
                        | "render"
                        | "concrete"
                )
            })
    };
    let facade_of = |tags: &HashMap<String, String>| -> Vec<(String, String)> {
        FACADE_KEYS
            .iter()
            .filter_map(|&k| tags.get(k).map(|v| (k.to_string(), v.clone())))
            .collect()
    };
    if part_groups.is_empty() {
        return;
    }
    let groups: HashSet<u64> = part_groups
        .values()
        .map(|&s| seed_without_hint(s))
        .collect();
    // Facade of each group's building, keyed like the groups: an outline way
    // by its id, a building relation by its salted id.
    let mut facade: HashMap<u64, Vec<(String, String)>> = HashMap::new();
    let mut relation_outlines: HashMap<u64, u64> = HashMap::new();
    for rel in relations {
        let Some(tags) = &rel.tags else { continue };
        let key = RELATION_SEED_BIT | rel.id;
        if !groups.contains(&key) {
            continue;
        }
        if masonry(tags) {
            facade.insert(key, facade_of(tags));
            continue;
        }
        for m in &rel.members {
            let role = m.role.trim();
            if m.r#type == "way"
                && (role.eq_ignore_ascii_case("outline") || role.eq_ignore_ascii_case("outer"))
            {
                relation_outlines.insert(m.r#ref, key);
            }
        }
    }
    for w in ways.iter() {
        let Some(tags) = w.tags.as_ref().filter(|t| masonry(t)) else {
            continue;
        };
        if groups.contains(&w.id) {
            facade.insert(w.id, facade_of(tags));
        } else if let Some(&key) = relation_outlines.get(&w.id) {
            facade.entry(key).or_insert_with(|| facade_of(tags));
        }
    }
    if facade.is_empty() {
        return;
    }
    for w in ways.iter_mut() {
        let Some(inherited) = part_groups
            .get(&w.id)
            .and_then(|&seed| facade.get(&seed_without_hint(seed)))
        else {
            continue;
        };
        let tags = w.tags.get_or_insert_with(HashMap::new);
        if FACADE_KEYS.iter().any(|k| tags.contains_key(*k)) {
            continue;
        }
        tags.extend(inherited.iter().cloned());
    }
}

fn compute_outline_suppression(
    relations: &[OsmElement],
    ways: &[OsmElement],
    nodes: &[OsmElement],
    part_group: &mut PartGroups,
) -> OutlineSuppression {
    let is_outline = |r: &str| r.eq_ignore_ascii_case("outline") || r.eq_ignore_ascii_case("outer");

    let mut needed_ways: HashSet<u64> = HashSet::new();
    for rel in relations {
        let Some(tags) = &rel.tags else { continue };
        if tags.get("type").map(|t| t.as_str()) != Some("building") {
            continue;
        }
        for m in &rel.members {
            let r = m.role.trim();
            if m.r#type == "way" && (r.eq_ignore_ascii_case("part") || is_outline(r)) {
                needed_ways.insert(m.r#ref);
            }
        }
    }
    if needed_ways.is_empty() {
        return HashSet::new();
    }

    // Single pass over member ways: geometry (way_nodes) plus style hint (way_hint,
    // so the group seed carries the building's decision).
    let mut way_nodes: HashMap<u64, &Vec<u64>> = HashMap::new();
    let mut way_hint: HashMap<u64, StyleHint> = HashMap::new();
    let mut way_year: HashMap<u64, i32> = HashMap::new();
    let mut way_ground: HashMap<u64, bool> = HashMap::new();
    let mut housing_ways: HashSet<u64> = HashSet::new();
    for w in ways.iter().filter(|w| needed_ways.contains(&w.id)) {
        if let Some(ns) = w.nodes.as_ref() {
            way_nodes.insert(w.id, ns);
        }
        if let Some(t) = w.tags.as_ref() {
            way_hint.insert(w.id, building_style_hint(t));
            if let Some(y) = year_for_parts(t) {
                way_year.insert(w.id, y);
            }
            way_ground.insert(w.id, part_covers_ground(t));
            if is_housing_type(t) {
                housing_ways.insert(w.id);
            }
        }
    }
    let mut needed_nodes: HashSet<u64> = HashSet::new();
    for ns in way_nodes.values() {
        needed_nodes.extend(ns.iter().copied());
    }
    let node_ll: HashMap<u64, (f64, f64)> = nodes
        .iter()
        .filter(|n| needed_nodes.contains(&n.id))
        .filter_map(|n| Some((n.id, (n.lat?, n.lon?))))
        .collect();

    // Shoelace area of a closed ring; lon scaled by cos(lat) so only the ratio matters.
    let way_area = |way_ref: u64| -> Option<f64> {
        let ids = way_nodes.get(&way_ref)?;
        let pts: Vec<(f64, f64)> = ids
            .iter()
            .filter_map(|id| node_ll.get(id).copied())
            .collect();
        if pts.len() < 3 {
            return None;
        }
        let lon_scale = pts[0].0.to_radians().cos();
        let mut area = 0.0;
        for i in 0..pts.len() {
            let (lat_a, lon_a) = pts[i];
            let (lat_b, lon_b) = pts[(i + 1) % pts.len()];
            area += (lon_a * lat_b - lon_b * lat_a) * lon_scale;
        }
        Some((area / 2.0).abs())
    };

    let mut suppressed: OutlineSuppression = HashSet::new();
    for rel in relations {
        let Some(tags) = &rel.tags else { continue };
        if tags.get("type").map(|t| t.as_str()) != Some("building") {
            continue;
        }

        // Style decision lives on the relation or any of its member ways.
        let mut rel_hint = building_style_hint(tags);
        if rel_hint == StyleHint::None {
            rel_hint = rel
                .members
                .iter()
                .filter_map(|m| way_hint.get(&m.r#ref).copied())
                .find(|h| *h != StyleHint::None)
                .unwrap_or(StyleHint::None);
        }
        // So does its date, which the outline usually carries alone.
        let rel_year = year_for_parts(tags).or_else(|| {
            rel.members
                .iter()
                .filter(|m| is_outline(m.role.trim()))
                .find_map(|m| way_year.get(&m.r#ref).copied())
        });

        // Housing when the relation or its outline says so.
        let housing = is_housing_type(tags)
            || rel.members.iter().any(|m| {
                is_outline(m.role.trim()) && m.r#type == "way" && housing_ways.contains(&m.r#ref)
            });

        // Sub-relation parts carry no way geometry here, so they skip the coverage gate.
        let mut has_part = false;
        let mut has_relation_part = false;
        let mut part_area = 0.0;
        for m in &rel.members {
            if !m.role.trim().eq_ignore_ascii_case("part") {
                continue;
            }
            has_part = true;
            match m.r#type.as_str() {
                "relation" => has_relation_part = true,
                "way" => {
                    if way_ground.get(&m.r#ref).copied().unwrap_or(true) {
                        part_area += way_area(m.r#ref).unwrap_or(0.0);
                    }
                    part_group.insert(
                        m.r#ref,
                        seed_with_residential_mark(
                            seed_with_year(
                                seed_with_hint(RELATION_SEED_BIT | rel.id, rel_hint),
                                rel_year,
                            ),
                            housing,
                        ),
                    );
                }
                _ => {}
            }
        }
        if !has_part {
            continue;
        }

        for m in &rel.members {
            let r = m.role.trim();
            if !is_outline(r) {
                continue;
            }
            let kind: &'static str = match m.r#type.as_str() {
                "way" => "way",
                "relation" => "relation",
                _ => continue,
            };

            // Keep the outline when the parts are too sparse to stand in for it.
            if kind == "way" && !has_relation_part {
                if let Some(outline_area) = way_area(m.r#ref) {
                    if outline_area > 0.0 && part_area / outline_area < MIN_PART_COVERAGE {
                        continue;
                    }
                }
            }

            suppressed.insert((kind, m.r#ref));
        }
    }
    suppressed
}

// Shoelace area of a closed lat/lon ring; lon scaled by cos(lat) so only the ratio matters.
fn ring_area(r: &[(f64, f64)]) -> f64 {
    if r.len() < 3 {
        return 0.0;
    }
    let lon_scale = r[0].0.to_radians().cos();
    let mut a = 0.0;
    for i in 0..r.len() {
        let (lat_a, lon_a) = r[i];
        let (lat_b, lon_b) = r[(i + 1) % r.len()];
        a += (lon_a * lat_b - lon_b * lat_a) * lon_scale;
    }
    (a / 2.0).abs()
}

/// Suppresses building:part ways whose centre lies inside a ride `attractions` draws
/// whole, and keeps the ride's own outline.
fn suppress_ride_parts(
    ways: &[OsmElement],
    nodes: &[OsmElement],
    suppressed: &mut OutlineSuppression,
) {
    use crate::element_processing::attractions;
    let rides: Vec<&OsmElement> = ways
        .iter()
        .filter(|w| w.tags.as_ref().is_some_and(attractions::replaces_parts))
        .collect();
    if rides.is_empty() {
        return;
    }
    for w in &rides {
        suppressed.remove(&("way", w.id));
    }
    let is_part = |tags: &HashMap<String, String>| {
        tags.get("building:part")
            .is_some_and(|v| !v.eq_ignore_ascii_case("no"))
            && !attractions::claims(tags)
    };
    let parts: Vec<&OsmElement> = ways
        .iter()
        .filter(|w| w.tags.as_ref().is_some_and(is_part))
        .collect();
    if parts.is_empty() {
        return;
    }
    // Only the rides' and parts' own nodes, not every node in the area.
    let wanted: FnvHashSet<u64> = rides
        .iter()
        .chain(&parts)
        .flat_map(|w| w.nodes.iter().flatten().copied())
        .collect();
    let coords: FnvHashMap<u64, (f64, f64)> = nodes
        .iter()
        .filter(|n| wanted.contains(&n.id))
        .filter_map(|n| Some((n.id, (n.lat?, n.lon?))))
        .collect();
    let ring = |w: &OsmElement| -> Vec<(f64, f64)> {
        w.nodes
            .iter()
            .flatten()
            .filter_map(|id| coords.get(id).copied())
            .collect()
    };
    let outlines: Vec<Vec<(f64, f64)>> = rides
        .iter()
        .map(|w| ring(w))
        .filter(|r| r.len() >= 4)
        .collect();
    for w in parts {
        let pts = ring(w);
        if pts.is_empty() {
            continue;
        }
        let n = pts.len() as f64;
        let lat = pts.iter().map(|p| p.0).sum::<f64>() / n;
        let lon = pts.iter().map(|p| p.1).sum::<f64>() / n;
        if outlines.iter().any(|o| point_in_ring(lat, lon, o)) {
            suppressed.insert(("way", w.id));
        }
    }
}

// Ray-cast point-in-polygon test on a lat/lon ring.
pub(crate) fn point_in_ring(lat: f64, lon: f64, r: &[(f64, f64)]) -> bool {
    let n = r.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (yi, xi) = r[i];
        let (yj, xj) = r[j];
        if (yi > lat) != (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Stitches way segments (each a node-id sequence) into closed rings of (lat,lon) points by
/// matching shared endpoint node ids. A multipolygon outer boundary is frequently split across
/// several member ways (e.g. an ellipse mapped as 4 arcs), so segments must be joined before
/// their area/containment can be measured. Segments that never close into a ring, or whose
/// nodes lack coordinates, are dropped (degenerate geometry is skipped, never panics).
pub(crate) fn stitch_rings(
    segments: Vec<Vec<u64>>,
    node_ll: &HashMap<u64, (f64, f64)>,
) -> Vec<Vec<(f64, f64)>> {
    let mut open: Vec<Vec<u64>> = segments.into_iter().filter(|s| s.len() >= 2).collect();
    let mut rings: Vec<Vec<(f64, f64)>> = Vec::new();

    while let Some(mut ring) = open.pop() {
        // Grow this ring by attaching segments at either end until it closes or stalls.
        // `ring` is never empty here, so the pattern always binds; we exit via the breaks below.
        while let (Some(&head), Some(&tail)) = (ring.first(), ring.last()) {
            if ring.len() >= 4 && head == tail {
                break; // closed ring
            }
            // Find a remaining segment that shares an endpoint node with either ring end.
            let Some(i) = open.iter().position(|seg| match (seg.first(), seg.last()) {
                (Some(&sf), Some(&sl)) => sf == tail || sl == tail || sf == head || sl == head,
                _ => false,
            }) else {
                break; // nothing connects: leave this ring open (it will be discarded)
            };
            let seg = open.swap_remove(i);
            let (sf, sl) = (seg[0], seg[seg.len() - 1]);
            if sf == tail {
                ring.extend(seg.into_iter().skip(1)); // drop the shared node
            } else if sl == tail {
                let mut s = seg;
                s.reverse();
                ring.extend(s.into_iter().skip(1));
            } else if sl == head {
                let mut s = seg;
                s.pop(); // drop the shared node
                s.extend(ring);
                ring = s;
            } else {
                // sf == head
                let mut s = seg;
                s.reverse();
                s.pop();
                s.extend(ring);
                ring = s;
            }
        }

        if ring.len() >= 4 && ring.first() == ring.last() {
            let pts: Vec<(f64, f64)> = ring
                .iter()
                .filter_map(|id| node_ll.get(id).copied())
                .collect();
            // A closed node ring may still lose points if some node lacked coordinates.
            if pts.len() >= 4 {
                rings.push(pts);
            }
        }
    }
    rings
}

/// Suppresses relation-less S3DB outlines: a building polygon that spatially contains building:part polygons.
/// Ways in `relation_ways` belong to a type=building relation and are judged by the relation pass instead.
fn compute_spatial_part_suppression(
    ways: &[OsmElement],
    nodes: &[OsmElement],
    relation_ways: &HashSet<u64>,
    part_group: &mut PartGroups,
) -> OutlineSuppression {
    let is_part = |tags: &HashMap<String, String>| {
        tags.get("building:part")
            .is_some_and(|v| !v.eq_ignore_ascii_case("no"))
    };

    // split building ways into candidate outlines and parts
    let mut outline_ids: Vec<u64> = Vec::new();
    let mut part_ids: Vec<u64> = Vec::new();
    let mut way_nodes: HashMap<u64, &Vec<u64>> = HashMap::new();
    let mut way_hint: HashMap<u64, StyleHint> = HashMap::new();
    let mut way_year: HashMap<u64, i32> = HashMap::new();
    let mut way_ground: HashMap<u64, bool> = HashMap::new();
    let mut housing_outlines: HashSet<u64> = HashSet::new();
    let mut needed_nodes: HashSet<u64> = HashSet::new();
    for w in ways {
        if relation_ways.contains(&w.id) {
            continue;
        }
        let (Some(tags), Some(ns)) = (&w.tags, &w.nodes) else {
            continue;
        };
        // need a closed ring (first node repeated as last)
        if ns.len() < 4 || ns.first() != ns.last() {
            continue;
        }
        if is_part(tags) {
            part_ids.push(w.id);
        } else if tags.contains_key("building") {
            outline_ids.push(w.id);
            if is_housing_type(tags) {
                housing_outlines.insert(w.id);
            }
        } else {
            continue;
        }
        way_nodes.insert(w.id, ns);
        way_hint.insert(w.id, building_style_hint(tags));
        if let Some(y) = year_for_parts(tags) {
            way_year.insert(w.id, y);
        }
        way_ground.insert(w.id, part_covers_ground(tags));
        needed_nodes.extend(ns.iter().copied());
    }
    if outline_ids.is_empty() || part_ids.is_empty() {
        return HashSet::new();
    }

    let node_ll: HashMap<u64, (f64, f64)> = nodes
        .iter()
        .filter(|n| needed_nodes.contains(&n.id))
        .filter_map(|n| Some((n.id, (n.lat?, n.lon?))))
        .collect();

    let ring = |id: u64| -> Vec<(f64, f64)> {
        way_nodes
            .get(&id)
            .map(|ids| ids.iter().filter_map(|i| node_ll.get(i).copied()).collect())
            .unwrap_or_default()
    };

    // area (shoelace) and point_in_ring are shared module-level helpers (see above).

    struct OutlineGeom {
        id: u64,
        ring: Vec<(f64, f64)>,
        area: f64,
    }

    // grid of outline bboxes so each part only tests nearby outlines
    let cell = |lat: f64, lon: f64| {
        (
            (lat / SUPPRESSION_GRID_CELL_DEG).floor() as i64,
            (lon / SUPPRESSION_GRID_CELL_DEG).floor() as i64,
        )
    };

    let mut geoms: Vec<OutlineGeom> = Vec::new();
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for id in outline_ids {
        let r = ring(id);
        let a = ring_area(&r);
        if a <= 0.0 {
            continue;
        }
        let (mut min_la, mut min_lo, mut max_la, mut max_lo) =
            (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(la, lo) in &r {
            min_la = min_la.min(la);
            max_la = max_la.max(la);
            min_lo = min_lo.min(lo);
            max_lo = max_lo.max(lo);
        }
        let gi = geoms.len();
        let (c0a, c0o) = cell(min_la, min_lo);
        let (c1a, c1o) = cell(max_la, max_lo);
        for ca in c0a..=c1a {
            for co in c0o..=c1o {
                grid.entry((ca, co)).or_default().push(gi);
            }
        }
        geoms.push(OutlineGeom {
            id,
            ring: r,
            area: a,
        });
    }

    // add each part area to every outline containing its centroid
    let mut covered: HashMap<usize, f64> = HashMap::new();
    for pid in part_ids {
        let r = ring(pid);
        let pa = ring_area(&r);
        if pa <= 0.0 {
            continue;
        }
        let (mut sla, mut slo) = (0.0, 0.0);
        for &(la, lo) in &r {
            sla += la;
            slo += lo;
        }
        let (cla, clo) = (sla / r.len() as f64, slo / r.len() as f64);
        let Some(cands) = grid.get(&cell(cla, clo)) else {
            continue;
        };
        let ground = way_ground.get(&pid).copied().unwrap_or(true);
        // smallest containing outline (tie-break min id) is the part's building
        let mut best: Option<usize> = None;
        for &gi in cands {
            if point_in_ring(cla, clo, &geoms[gi].ring) {
                if ground {
                    *covered.entry(gi).or_insert(0.0) += pa;
                }
                best = Some(match best {
                    Some(b) if (geoms[b].area, geoms[b].id) <= (geoms[gi].area, geoms[gi].id) => b,
                    _ => gi,
                });
            }
        }
        if let Some(gi) = best {
            // prefer the outline's style, fall back to the part's own tags
            let mut hint = way_hint
                .get(&geoms[gi].id)
                .copied()
                .unwrap_or(StyleHint::None);
            if hint == StyleHint::None {
                hint = way_hint.get(&pid).copied().unwrap_or(StyleHint::None);
            }
            let year = way_year.get(&geoms[gi].id).copied();
            part_group.insert(
                pid,
                seed_with_residential_mark(
                    seed_with_year(seed_with_hint(geoms[gi].id, hint), year),
                    housing_outlines.contains(&geoms[gi].id),
                ),
            );
        }
    }

    let mut suppressed: OutlineSuppression = HashSet::new();
    for (gi, cov) in covered {
        let g = &geoms[gi];
        if cov / g.area >= MIN_PART_COVERAGE {
            suppressed.insert(("way", g.id));
        }
    }
    suppressed
}

/// Suppresses S3DB outlines expressed as MULTIPOLYGON RELATIONS: a building relation whose
/// ground footprint is spatially covered (>= MIN_PART_COVERAGE) by independent building:part
/// RELATIONS, each of which still renders on its own via the relation path. Mirrors
/// `compute_spatial_part_suppression` but sources footprints from relations rather than
/// closed ways.
///
/// Two relation specifics drive the extra work here:
///   * A relation's outer boundary is frequently SPLIT across several member ways (the
///     Colosseum ellipse = 4 outer arcs, #1191), so the segments are stitched into closed rings
///     before area/containment (see `stitch_rings`). Inner rings are ignored: gross outer
///     area biases safely toward suppressing a genuinely covered outline, matching the
///     way-based pass.
///   * Association is SPATIAL (part centroid inside the outline footprint), never membership:
///     the parts are separate relations or closed ways, not members of the outline relation,
///     so the membership pass (`compute_outline_suppression`) never links them.
///
/// A building:part WAY is credited too, as long as it does not stand in one of the outline's
/// courtyards, and joins the group of the smallest outline it stands in, so it takes the
/// building's date, style and storey height as the parts of an outline way do.
#[cfg(test)]
fn compute_spatial_relation_part_suppression(
    relations: &[OsmElement],
    ways: &[OsmElement],
    nodes: &[OsmElement],
) -> OutlineSuppression {
    spatial_relation_part_suppression(relations, ways, nodes, &mut PartGroups::new())
}

fn spatial_relation_part_suppression(
    relations: &[OsmElement],
    ways: &[OsmElement],
    nodes: &[OsmElement],
    part_group: &mut PartGroups,
) -> OutlineSuppression {
    let is_part = |tags: &HashMap<String, String>| {
        tags.get("building:part")
            .is_some_and(|v| !v.eq_ignore_ascii_case("no"))
    };
    // Only relations the parser actually turns into a ProcessedElement::Relation can render.
    // Anything else must be ignored here: crediting a part that will never be drawn could
    // suppress an outline and leave the building missing entirely.
    // Keep this in sync with the relation type filter in `parse_osm_data`.
    let is_renderable = |tags: &HashMap<String, String>| {
        matches!(
            tags.get("type").map(|t| t.as_str()),
            Some("multipolygon") | Some("building")
        )
    };
    // A relation's footprint is bounded by its outer/outline member ways; inner rings (holes)
    // and non-boundary roles are excluded.
    let is_outer_role =
        |r: &str| r.eq_ignore_ascii_case("outer") || r.eq_ignore_ascii_case("outline");
    let is_inner_role = |r: &str| r.eq_ignore_ascii_case("inner");

    // Resolve only the geometry we need: the outer member ways of every building /
    // building:part relation, plus the nodes those ways reference.
    let mut needed_ways: HashSet<u64> = HashSet::new();
    for rel in relations {
        let Some(tags) = &rel.tags else { continue };
        if !is_renderable(tags) || !(tags.contains_key("building") || is_part(tags)) {
            continue;
        }
        for m in &rel.members {
            let role = m.role.trim();
            if m.r#type == "way" && (is_outer_role(role) || is_inner_role(role)) {
                needed_ways.insert(m.r#ref);
            }
        }
    }
    if needed_ways.is_empty() {
        return HashSet::new();
    }
    // Closed building:part ways above ground: the wings of a courtyard block are drawn as
    // often as part ways as as part relations.
    let part_ways: Vec<&OsmElement> = ways
        .iter()
        .filter(|w| {
            w.tags.as_ref().is_some_and(|t| {
                is_part(t) && !crate::element_processing::buildings::is_underground_building(t)
            }) && w
                .nodes
                .as_ref()
                .is_some_and(|ns| ns.len() >= 4 && ns.first() == ns.last())
        })
        .collect();

    let mut way_nodes: HashMap<u64, &Vec<u64>> = HashMap::new();
    let mut needed_nodes: HashSet<u64> = HashSet::new();
    for w in ways.iter().filter(|w| needed_ways.contains(&w.id)) {
        if let Some(ns) = w.nodes.as_ref() {
            way_nodes.insert(w.id, ns);
            needed_nodes.extend(ns.iter().copied());
        }
    }
    for w in &part_ways {
        if let Some(ns) = w.nodes.as_ref() {
            needed_nodes.extend(ns.iter().copied());
        }
    }
    let node_ll: HashMap<u64, (f64, f64)> = nodes
        .iter()
        .filter(|n| needed_nodes.contains(&n.id))
        .filter_map(|n| Some((n.id, (n.lat?, n.lon?))))
        .collect();

    // Stitch a relation's outer (or inner) member ways into closed rings of (lat,lon).
    let rel_rings = |rel: &OsmElement, inner: bool| -> Vec<Vec<(f64, f64)>> {
        let segments: Vec<Vec<u64>> = rel
            .members
            .iter()
            .filter(|m| {
                let role = m.role.trim();
                m.r#type == "way"
                    && if inner {
                        is_inner_role(role)
                    } else {
                        is_outer_role(role)
                    }
            })
            .filter_map(|m| way_nodes.get(&m.r#ref).map(|ns| (*ns).clone()))
            .collect();
        stitch_rings(segments, &node_ll)
    };

    struct RelOutline {
        id: u64,
        rings: Vec<Vec<(f64, f64)>>,
        holes: Vec<Vec<(f64, f64)>>,
        area: f64,
        seed: u64,
    }
    struct RelPart {
        area: f64,
        centroid: (f64, f64),
        ground: bool,
        /// Set for a building:part way, which joins its outline's group.
        way: Option<u64>,
    }

    // Classify each relation and assemble its footprint. building:part=no marks the OUTLINE
    // (not a part, e.g. the Colosseum outline); an outline candidate carries `building` and is
    // not itself a part.
    let mut outlines: Vec<RelOutline> = Vec::new();
    let mut parts: Vec<RelPart> = Vec::new();
    for rel in relations {
        let Some(tags) = &rel.tags else { continue };
        if !is_renderable(tags) {
            continue;
        }
        let part = is_part(tags);
        let outline = !part && tags.contains_key("building");
        if !part && !outline {
            continue;
        }
        let rings = rel_rings(rel, false);
        let area: f64 = rings.iter().map(|r| ring_area(r)).sum();
        if area <= 0.0 {
            continue; // no closed ring could be assembled (degenerate/open geometry): skip
        }
        if outline {
            // The seed the parts of an outline way get from it, keyed by the relation.
            let seed = seed_with_residential_mark(
                seed_with_year(
                    seed_with_hint(RELATION_SEED_BIT | rel.id, building_style_hint(tags)),
                    year_for_parts(tags),
                ),
                is_housing_type(tags),
            );
            outlines.push(RelOutline {
                id: rel.id,
                rings,
                holes: rel_rings(rel, true),
                area,
                seed,
            });
        } else {
            // Gross outer area; centroid = vertex average over all assembled rings. The
            // closing node is counted (first == last), matching the way-based pass exactly.
            let (mut sla, mut slo, mut cnt) = (0.0, 0.0, 0usize);
            for ring in &rings {
                for &(la, lo) in ring {
                    sla += la;
                    slo += lo;
                    cnt += 1;
                }
            }
            if cnt == 0 {
                continue;
            }
            parts.push(RelPart {
                area,
                centroid: (sla / cnt as f64, slo / cnt as f64),
                ground: part_covers_ground(tags),
                way: None,
            });
        }
    }
    for w in &part_ways {
        let (Some(tags), Some(ns)) = (&w.tags, &w.nodes) else {
            continue;
        };
        let ring: Vec<(f64, f64)> = ns.iter().filter_map(|i| node_ll.get(i).copied()).collect();
        let area = ring_area(&ring);
        if area <= 0.0 || ring.is_empty() {
            continue;
        }
        let (sla, slo) = ring
            .iter()
            .fold((0.0, 0.0), |(a, o), &(la, lo)| (a + la, o + lo));
        parts.push(RelPart {
            area,
            centroid: (sla / ring.len() as f64, slo / ring.len() as f64),
            ground: part_covers_ground(tags),
            way: Some(w.id),
        });
    }
    if outlines.is_empty() || parts.is_empty() {
        return HashSet::new();
    }

    // grid of outline bboxes so each part only tests nearby outlines
    let cell = |lat: f64, lon: f64| {
        (
            (lat / SUPPRESSION_GRID_CELL_DEG).floor() as i64,
            (lon / SUPPRESSION_GRID_CELL_DEG).floor() as i64,
        )
    };

    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (gi, g) in outlines.iter().enumerate() {
        let (mut min_la, mut min_lo, mut max_la, mut max_lo) =
            (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for ring in &g.rings {
            for &(la, lo) in ring {
                min_la = min_la.min(la);
                max_la = max_la.max(la);
                min_lo = min_lo.min(lo);
                max_lo = max_lo.max(lo);
            }
        }
        let (c0a, c0o) = cell(min_la, min_lo);
        let (c1a, c1o) = cell(max_la, max_lo);
        for ca in c0a..=c1a {
            for co in c0o..=c1o {
                grid.entry((ca, co)).or_default().push(gi);
            }
        }
    }

    // Add each ground-level part's gross area to every outline whose footprint contains the
    // part centroid (matches the way-based pass, which credits every containing outline).
    let mut covered: HashMap<usize, f64> = HashMap::new();
    for p in &parts {
        let (cla, clo) = p.centroid;
        let Some(cands) = grid.get(&cell(cla, clo)) else {
            continue;
        };
        // The smallest outline a part way stands in (tie-break min id) is its building.
        let mut best: Option<usize> = None;
        for &gi in cands {
            let g = &outlines[gi];
            if !g.rings.iter().any(|r| point_in_ring(cla, clo, r)) {
                continue;
            }
            // A part way in a courtyard is a building of its own there, not the block.
            if p.way.is_some() && g.holes.iter().any(|r| point_in_ring(cla, clo, r)) {
                continue;
            }
            // Elevated parts (min_height / building:min_level > 0) model raised roof/dome
            // volumes that float above the ground, so they can't stand in for the ground
            // footprint.
            if p.ground {
                *covered.entry(gi).or_insert(0.0) += p.area;
            }
            best = Some(match best {
                Some(b) if (outlines[b].area, outlines[b].id) <= (g.area, g.id) => b,
                _ => gi,
            });
        }
        // A building:part RELATION renders via the relation arm and carries its own tags, so
        // only part ways are grouped; a group set by an earlier pass stays.
        if let (Some(pid), Some(gi)) = (p.way, best) {
            part_group.entry(pid).or_insert(outlines[gi].seed);
        }
    }

    let mut suppressed: OutlineSuppression = HashSet::new();
    for (gi, cov) in covered {
        let g = &outlines[gi];
        if cov / g.area >= MIN_PART_COVERAGE {
            suppressed.insert(("relation", g.id));
        }
    }
    suppressed
}

/// Returns true if tags indicate a water element handled by water_areas.rs.
fn is_water_element(tags: &HashMap<String, String>) -> bool {
    // Check for explicit water tag
    if tags.contains_key("water") {
        return true;
    }

    // Check for natural=water or natural=bay
    if let Some(natural_val) = tags.get("natural") {
        if natural_val == "water" || natural_val == "bay" {
            return true;
        }
    }

    // Check for waterway=dock (also handled as water area)
    if let Some(waterway_val) = tags.get("waterway") {
        if waterway_val == "dock" {
            return true;
        }
    }

    false
}

/// Relations whose outer members are filled as areas: natural, landuse, leisure, and
/// the piers and breakwaters drawn as decks and mounds.
fn is_filled_area_relation(tags: &HashMap<String, String>) -> bool {
    tags.contains_key("natural")
        || tags.contains_key("landuse")
        || tags.contains_key("leisure")
        || matches!(
            tags.get("man_made").map(String::as_str),
            Some("pier" | "breakwater" | "groyne")
        )
}

/// Each member's role, None for members that are not rings of the relation. "part" only
/// counts in type=building relations, and any other role of a building is outer.
///
/// A blank role in a multipolygon is read from the geometry. A closed way is inner when it
/// lies inside an odd number of the relation's other rings, and outer otherwise. An open
/// way is a piece of a ring only where it joins the other pieces end to end, each of its
/// end nodes shared with exactly one other piece; it takes the role of the pieces it joins.
/// Anything else, like a stray line, stays out, as it did before blank roles were read.
fn member_roles(
    members: &[OsmMember],
    ways_map: &HashMap<u64, Arc<ProcessedWay>>,
    building_type: bool,
    building: bool,
) -> Vec<Option<ProcessedMemberRole>> {
    let blank = |m: &OsmMember| m.r#type == "way" && m.role.trim().is_empty();
    let mut roles: Vec<Option<ProcessedMemberRole>> = members
        .iter()
        .map(|m| {
            if m.r#type != "way" {
                None
            } else if m.role.trim().eq_ignore_ascii_case("part") {
                building_type.then_some(ProcessedMemberRole::Part)
            } else {
                area_member_role(&m.role).or_else(|| building.then_some(ProcessedMemberRole::Outer))
            }
        })
        .collect();
    if building || !members.iter().any(blank) {
        return roles;
    }
    let way = |i: usize| {
        ways_map
            .get(&members[i].r#ref)
            .filter(|w| w.nodes.len() >= 2)
    };
    let ends = |w: &ProcessedWay| [w.nodes[0].id, w.nodes[w.nodes.len() - 1].id];
    let candidates: Vec<usize> = (0..members.len())
        .filter(|&i| roles[i].is_some() || blank(&members[i]))
        .filter(|&i| way(i).is_some())
        .collect();

    // Open pieces: end node degrees, and which explicit roles end at each node.
    let mut degree: FnvHashMap<u64, usize> = FnvHashMap::default();
    let mut role_at: FnvHashMap<u64, Vec<ProcessedMemberRole>> = FnvHashMap::default();
    for &i in &candidates {
        let w = way(i).unwrap();
        if is_closed_way(w) {
            continue;
        }
        for node in ends(w) {
            *degree.entry(node).or_default() += 1;
            if let Some(role) = &roles[i] {
                role_at.entry(node).or_default().push(role.clone());
            }
        }
    }
    for &i in &candidates {
        let w = way(i).unwrap();
        if !blank(&members[i]) || is_closed_way(w) {
            continue;
        }
        if ends(w).iter().all(|node| degree.get(node) == Some(&2)) {
            let joined: Vec<&ProcessedMemberRole> = ends(w)
                .iter()
                .flat_map(|n| role_at.get(n))
                .flatten()
                .collect();
            let inner =
                !joined.is_empty() && joined.iter().all(|r| **r == ProcessedMemberRole::Inner);
            roles[i] = Some(if inner {
                ProcessedMemberRole::Inner
            } else {
                ProcessedMemberRole::Outer
            });
        }
    }

    // Closed blank rings by depth, among closed members and the rings the pieces make.
    let closed_blanks: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|&i| blank(&members[i]) && is_closed_way(way(i).unwrap()))
        .collect();
    if closed_blanks.is_empty() {
        return roles;
    }
    let mut containers: Vec<(Option<usize>, Vec<ProcessedNode>)> = Vec::new();
    for role in [ProcessedMemberRole::Outer, ProcessedMemberRole::Inner] {
        let mut pieces: Vec<Vec<ProcessedNode>> = candidates
            .iter()
            .filter(|&&i| roles[i].as_ref() == Some(&role) && !is_closed_way(way(i).unwrap()))
            .map(|&i| way(i).unwrap().nodes.clone())
            .collect();
        crate::element_processing::merge_way_segments(&mut pieces);
        containers.extend(
            pieces
                .into_iter()
                .filter(|r| r.len() >= 4 && r[0].id == r[r.len() - 1].id)
                .map(|r| (None, r)),
        );
    }
    for &i in &candidates {
        let w = way(i).unwrap();
        if is_closed_way(w) {
            containers.push((Some(i), w.nodes.clone()));
        }
    }
    for i in closed_blanks {
        let nodes = &way(i).unwrap().nodes;
        let around = containers
            .iter()
            .filter(|(j, ring)| {
                *j != Some(i)
                    && crate::floodfill_cache::mostly_inside(
                        nodes,
                        ring,
                        crate::floodfill_cache::node_xz,
                    )
            })
            .count();
        roles[i] = Some(if around % 2 == 1 {
            ProcessedMemberRole::Inner
        } else {
            ProcessedMemberRole::Outer
        });
    }
    roles
}

/// Closed outer ways of a relation that draws its area less its holes, whose own tags say
/// nothing the relation does not. Drawn as ways too, they would fill the holes in again.
/// A way with a tag of its own, a fence or another use, is left to draw as before.
fn repeating_outers<'a>(
    tags: &'a HashMap<String, String>,
    raw: &'a [OsmMember],
    roles: &'a [Option<ProcessedMemberRole>],
    members: &[ProcessedMember],
    ways_map: &'a HashMap<u64, Arc<ProcessedWay>>,
) -> impl Iterator<Item = u64> + 'a {
    let holed = members.iter().any(|m| m.role == ProcessedMemberRole::Inner)
        && crate::element_processing::RelationArea::of(tags)
            .is_some_and(|area| area.draws_area(tags));
    let said = |key: &str| {
        key == "name" || key.ends_with("_name") || matches!(key, "wikidata" | "area" | "type")
    };
    raw.iter()
        .zip(roles)
        .filter(move |(_, role)| holed && **role == Some(ProcessedMemberRole::Outer))
        .filter_map(move |(m, _)| ways_map.get(&m.r#ref))
        .filter(move |way| {
            is_closed_way(way)
                && way.tags.keys().any(|k| !said(k))
                && way
                    .tags
                    .iter()
                    .all(|(k, v)| said(k) || tags.get(k) == Some(v))
        })
        .map(|way| way.id)
}

/// Outer or inner, the two roles a multipolygon ring can have.
fn area_member_role(role: &str) -> Option<ProcessedMemberRole> {
    let role = role.trim();
    if role.eq_ignore_ascii_case("outer") || role.eq_ignore_ascii_case("outline") {
        Some(ProcessedMemberRole::Outer)
    } else if role.eq_ignore_ascii_case("inner") {
        Some(ProcessedMemberRole::Inner)
    } else {
        None
    }
}

fn is_closed_way(way: &ProcessedWay) -> bool {
    match (way.nodes.first(), way.nodes.last()) {
        (Some(first), Some(last)) => {
            way.nodes.len() >= 3
                && (first.id == last.id || (first.x == last.x && first.z == last.z))
        }
        _ => false,
    }
}

/// Id of a ring assembled from a relation's member ways. Bit 61 is past every OSM way id
/// and bit 63, which Overture ids carry, stays clear, so it collides with neither.
fn assembled_ring_id(relation_id: u64, inner: bool, index: usize) -> u64 {
    (1 << 61)
        | ((relation_id & ((1 << 45) - 1)) << 16)
        | (u64::from(inner) << 15)
        | (index as u64 & 0x7FFF)
}

/// Joins a multipolygon's member ways into closed rings, clips every ring to the clip bbox
/// and returns those reaching the world as synthetic member ways. Pieces that still do not
/// close are dropped: filled they would make a wedge, and drawn they would be a stray
/// outline around nothing.
fn assemble_area_rings(
    relation_id: u64,
    members: &[OsmMember],
    roles: &[Option<ProcessedMemberRole>],
    ways_map: &HashMap<u64, Arc<ProcessedWay>>,
    clip_bbox: &XZBBox,
    touches_world: impl Fn(&[ProcessedNode]) -> bool,
) -> Vec<ProcessedMember> {
    let mut outers: Vec<Vec<ProcessedNode>> = Vec::new();
    let mut inners: Vec<Vec<ProcessedNode>> = Vec::new();
    for (member, role) in members.iter().zip(roles) {
        let rings = match role {
            Some(ProcessedMemberRole::Outer) => &mut outers,
            Some(ProcessedMemberRole::Inner) => &mut inners,
            _ => continue,
        };
        if let Some(way) = ways_map.get(&member.r#ref) {
            if way.nodes.len() >= 2 {
                rings.push(way.nodes.clone());
            }
        }
    }
    crate::element_processing::merge_way_segments(&mut outers);
    crate::element_processing::merge_way_segments(&mut inners);

    let mut assembled = Vec::new();
    for (role, rings) in [
        (ProcessedMemberRole::Outer, outers),
        (ProcessedMemberRole::Inner, inners),
    ] {
        for (index, mut ring) in rings.into_iter().enumerate() {
            let (Some(first), Some(last)) = (ring.first(), ring.last()) else {
                continue;
            };
            if first.id != last.id {
                // merge_way_segments joins ends up to a block apart, as the clipper leaves them.
                if ring.len() < 3 || (first.x - last.x).abs() > 1 || (first.z - last.z).abs() > 1 {
                    continue;
                }
                let first = first.clone();
                ring.push(first);
            }
            let nodes = clip_way_to_bbox(&ring, clip_bbox);
            if nodes.len() < 4 || !touches_world(&nodes) {
                continue;
            }
            let inner = role == ProcessedMemberRole::Inner;
            assembled.push(ProcessedMember {
                role: role.clone(),
                way: Arc::new(ProcessedWay {
                    id: assembled_ring_id(relation_id, inner, index),
                    tags: HashMap::new(),
                    nodes,
                }),
            });
        }
    }
    assembled
}

const PRIORITY_ORDER: [&str; 6] = [
    "entrance", "building", "highway", "waterway", "water", "barrier",
];

// Function to determine the priority of each element
pub fn get_priority(element: &ProcessedElement) -> usize {
    // Check each tag against the priority order
    for (i, &tag) in PRIORITY_ORDER.iter().enumerate() {
        if element.tags().contains_key(tag) {
            return i;
        }
    }
    // Masts and chimneys stand on the roof they are mapped on, so they wait for every
    // building part, which has the default priority and comes after nodes.
    if matches!(
        element.tags().get("man_made").map(String::as_str),
        Some("antenna" | "mast" | "chimney")
    ) {
        return PRIORITY_ORDER.len() + 1;
    }
    // Return a default priority if none of the tags match
    PRIORITY_ORDER.len()
}

/// Render order for the CLI, GUI and tests: by `get_priority`, with plain landuse last so it only
/// fills ground around what is already there.
pub fn sort_by_priority(elements: &mut [ProcessedElement]) {
    elements.sort_by_cached_key(|element| {
        let priority = get_priority(element);
        (
            priority >= PRIORITY_ORDER.len() && element.tags().contains_key("landuse"),
            priority,
        )
    });
}

#[cfg(test)]
mod outline_suppression_tests {
    use super::*;

    fn node(id: u64, lat: f64, lon: f64) -> OsmElement {
        OsmElement {
            r#type: "node".into(),
            id,
            lat: Some(lat),
            lon: Some(lon),
            nodes: None,
            tags: None,
            members: Vec::new(),
        }
    }

    // Axis-aligned square way with corner (0,0) and the given side length.
    fn square_way(id: u64, first_node_id: u64, side: f64) -> (OsmElement, Vec<OsmElement>) {
        let corners = [(0.0, 0.0), (0.0, side), (side, side), (side, 0.0)];
        let nodes: Vec<OsmElement> = corners
            .iter()
            .enumerate()
            .map(|(i, &(lat, lon))| node(first_node_id + i as u64, lat, lon))
            .collect();
        let mut ids: Vec<u64> = nodes.iter().map(|n| n.id).collect();
        ids.push(first_node_id);
        let way = OsmElement {
            r#type: "way".into(),
            id,
            lat: None,
            lon: None,
            nodes: Some(ids),
            tags: None,
            members: Vec::new(),
        };
        (way, nodes)
    }

    fn member(kind: &str, r#ref: u64, role: &str) -> OsmMember {
        OsmMember {
            r#type: kind.into(),
            r#ref,
            r#role: role.into(),
        }
    }

    fn building_relation(members: Vec<OsmMember>) -> OsmElement {
        OsmElement {
            r#type: "relation".into(),
            id: 1,
            lat: None,
            lon: None,
            nodes: None,
            tags: Some(HashMap::from([(
                "type".to_string(),
                "building".to_string(),
            )])),
            members,
        }
    }

    // Therme Erding: parts cover ~24% of the outline, so the outline must survive.
    #[test]
    fn sparse_way_parts_keep_the_outline() {
        let (outline, outline_nodes) = square_way(100, 1000, 1.0);
        let (part, part_nodes) = square_way(200, 2000, 0.5);
        let rel = building_relation(vec![
            member("way", 100, "outline"),
            member("way", 200, "part"),
        ]);

        let nodes: Vec<OsmElement> = outline_nodes.into_iter().chain(part_nodes).collect();
        let suppressed =
            compute_outline_suppression(&[rel], &[outline, part], &nodes, &mut PartGroups::new());

        assert!(!suppressed.contains(&("way", 100)));
    }

    // Well-tiled parts (64% coverage) stand in for the outline, so it is dropped.
    #[test]
    fn covering_way_parts_suppress_the_outline() {
        let (outline, outline_nodes) = square_way(100, 1000, 1.0);
        let (part, part_nodes) = square_way(200, 2000, 0.8);
        let rel = building_relation(vec![
            member("way", 100, "outline"),
            member("way", 200, "part"),
        ]);

        let nodes: Vec<OsmElement> = outline_nodes.into_iter().chain(part_nodes).collect();
        let suppressed =
            compute_outline_suppression(&[rel], &[outline, part], &nodes, &mut PartGroups::new());

        assert!(suppressed.contains(&("way", 100)));
    }

    fn with_tag(mut w: OsmElement, k: &str, v: &str) -> OsmElement {
        w.tags
            .get_or_insert_with(HashMap::new)
            .insert(k.to_string(), v.to_string());
        w
    }

    // S3DB churches (e.g. St. Peter's): parts model the raised roof/dome and float on
    // min_height, so they don't cover the ground footprint and can't drop the outline.
    #[test]
    fn elevated_way_parts_keep_the_outline() {
        let (outline, outline_nodes) = square_way(100, 1000, 1.0);
        let (part, part_nodes) = square_way(200, 2000, 0.8);
        let part = with_tag(part, "min_height", "54");
        let rel = building_relation(vec![
            member("way", 100, "outline"),
            member("way", 200, "part"),
        ]);

        let nodes: Vec<OsmElement> = outline_nodes.into_iter().chain(part_nodes).collect();
        let suppressed =
            compute_outline_suppression(&[rel], &[outline, part], &nodes, &mut PartGroups::new());

        assert!(!suppressed.contains(&("way", 100)));
    }

    // Sub-relation parts carry no way geometry, so fall back to always suppressing.
    #[test]
    fn relation_parts_suppress_the_outline() {
        let (outline, outline_nodes) = square_way(100, 1000, 1.0);
        let rel = building_relation(vec![
            member("way", 100, "outline"),
            member("relation", 300, "part"),
        ]);

        let suppressed =
            compute_outline_suppression(&[rel], &[outline], &outline_nodes, &mut PartGroups::new());

        assert!(suppressed.contains(&("way", 100)));
    }

    // No parts at all: nothing is ever suppressed.
    #[test]
    fn outline_without_parts_is_kept() {
        let (outline, outline_nodes) = square_way(100, 1000, 1.0);
        let rel = building_relation(vec![member("way", 100, "outline")]);

        let suppressed =
            compute_outline_suppression(&[rel], &[outline], &outline_nodes, &mut PartGroups::new());

        assert!(suppressed.is_empty());
    }

    fn tagged(mut w: OsmElement, k: &str, v: &str) -> OsmElement {
        w.tags = Some(HashMap::from([(k.to_string(), v.to_string())]));
        w
    }

    // A building:part covering >=50% of a relation-less outline suppresses it.
    #[test]
    fn spatial_part_covering_outline_suppresses_it() {
        let (o, on) = square_way(100, 1000, 1.0);
        let (p, pn) = square_way(200, 2000, 0.8);
        let ways = [
            tagged(o, "building", "yes"),
            tagged(p, "building:part", "yes"),
        ];
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).collect();
        let s = compute_spatial_part_suppression(
            &ways,
            &nodes,
            &HashSet::new(),
            &mut PartGroups::new(),
        );
        assert!(s.contains(&("way", 100)));
    }

    // A relation-less S3DB outline whose only parts float on min_height survives.
    #[test]
    fn spatial_elevated_part_keeps_outline() {
        let (o, on) = square_way(100, 1000, 1.0);
        let (p, pn) = square_way(200, 2000, 0.8);
        let p = with_tag(tagged(p, "building:part", "yes"), "min_height", "54");
        let ways = [tagged(o, "building", "yes"), p];
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).collect();
        let s = compute_spatial_part_suppression(
            &ways,
            &nodes,
            &HashSet::new(),
            &mut PartGroups::new(),
        );
        assert!(!s.contains(&("way", 100)));
    }

    // Therme Erding: a relation-owned outline must be left to the relation pass, not
    // suppressed by the spatial pass counting unrelated parts that fall inside its footprint.
    #[test]
    fn spatial_skips_relation_owned_outline() {
        let (o, on) = square_way(100, 1000, 1.0);
        let (p, pn) = square_way(200, 2000, 0.8);
        let ways = [
            tagged(o, "building", "yes"),
            tagged(p, "building:part", "yes"),
        ];
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).collect();
        let relation_ways = HashSet::from([100u64]);
        let s =
            compute_spatial_part_suppression(&ways, &nodes, &relation_ways, &mut PartGroups::new());
        assert!(!s.contains(&("way", 100)));
    }

    // An open (unclosed) part ring is ignored, so it can't suppress the outline.
    #[test]
    fn spatial_open_part_is_ignored() {
        let (o, on) = square_way(100, 1000, 1.0);
        let (mut p, pn) = square_way(200, 2000, 0.8);
        // drop the closing node so first != last
        p.nodes.as_mut().unwrap().pop();
        let ways = [
            tagged(o, "building", "yes"),
            tagged(p, "building:part", "yes"),
        ];
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).collect();
        let s = compute_spatial_part_suppression(
            &ways,
            &nodes,
            &HashSet::new(),
            &mut PartGroups::new(),
        );
        assert!(!s.contains(&("way", 100)));
    }

    // A sparse part (25% coverage) leaves the outline in place.
    #[test]
    fn spatial_sparse_part_keeps_outline() {
        let (o, on) = square_way(100, 1000, 1.0);
        let (p, pn) = square_way(200, 2000, 0.5);
        let ways = [
            tagged(o, "building", "yes"),
            tagged(p, "building:part", "yes"),
        ];
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).collect();
        let s = compute_spatial_part_suppression(
            &ways,
            &nodes,
            &HashSet::new(),
            &mut PartGroups::new(),
        );
        assert!(!s.contains(&("way", 100)));
    }

    // building:part=no marks the outline, not a part, so nothing suppresses it.
    #[test]
    fn spatial_outline_without_parts_is_kept() {
        let (o, on) = square_way(100, 1000, 1.0);
        let ways = [tagged(o, "building", "commercial")];
        let s =
            compute_spatial_part_suppression(&ways, &on, &HashSet::new(), &mut PartGroups::new());
        assert!(s.is_empty());
    }

    // A contained part is grouped under its outline way id (shared style seed).
    #[test]
    fn spatial_part_is_grouped_under_its_outline() {
        let (o, on) = square_way(100, 1000, 1.0);
        let (p, pn) = square_way(200, 2000, 0.5);
        let ways = [
            tagged(o, "building", "yes"),
            tagged(p, "building:part", "yes"),
        ];
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).collect();
        let mut groups = PartGroups::new();
        compute_spatial_part_suppression(&ways, &nodes, &HashSet::new(), &mut groups);
        assert_eq!(groups.get(&200), Some(&100));
    }

    // The outline's start_date reaches its parts through the shared seed,
    // which still keys the same group.
    #[test]
    fn spatial_part_carries_the_outline_year() {
        let (o, on) = square_way(100, 1000, 1.0);
        let (p, pn) = square_way(200, 2000, 0.5);
        let mut outline = tagged(o, "building", "yes");
        if let Some(t) = outline.tags.as_mut() {
            t.insert("start_date".to_string(), "1962".to_string());
        }
        let ways = [outline, tagged(p, "building:part", "yes")];
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).collect();
        let mut groups = PartGroups::new();
        compute_spatial_part_suppression(&ways, &nodes, &HashSet::new(), &mut groups);
        let seed = groups[&200];
        assert_eq!(year_from_seed(seed), Some(1962));
        assert_eq!(group_key(seed), 100);
        assert_eq!(style_hint_from_seed(seed), StyleHint::None);
    }

    // A church rebuilt after the war in its old form keeps the date of its
    // rebuilding to itself; its parts stay with its style.
    #[test]
    fn a_rebuilt_church_does_not_date_its_parts() {
        let (o, on) = square_way(100, 1000, 1.0);
        let (p, pn) = square_way(200, 2000, 0.5);
        let mut outline = tagged(o, "building", "church");
        if let Some(t) = outline.tags.as_mut() {
            t.insert("building:architecture".to_string(), "gothic".to_string());
            t.insert("start_date".to_string(), "1954".to_string());
        }
        let ways = [outline, tagged(p, "building:part", "tower")];
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).collect();
        let mut groups = PartGroups::new();
        compute_spatial_part_suppression(&ways, &nodes, &HashSet::new(), &mut groups);
        let seed = groups[&200];
        assert_eq!(year_from_seed(seed), None);
        assert_eq!(style_hint_from_seed(seed), StyleHint::Masonry);
    }

    // An untagged part takes the stone of its outline; a tagged part and a
    // glass outline keep their own.
    #[test]
    fn untagged_parts_take_the_masonry_of_their_outline() {
        let (o, on) = square_way(100, 1000, 1.0);
        let (p, pn) = square_way(200, 2000, 0.5);
        let (q, qn) = square_way(300, 3000, 0.3);
        let outline = with_tag(
            tagged(o, "building", "retail"),
            "building:material",
            "stone",
        );
        let outline = with_tag(outline, "building:colour", "#cfc6b5");
        let own = with_tag(
            tagged(q, "building:part", "yes"),
            "building:colour",
            "black",
        );
        let mut ways = vec![outline, tagged(p, "building:part", "yes"), own];
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).chain(qn).collect();
        let mut groups = PartGroups::new();
        compute_spatial_part_suppression(&ways, &nodes, &HashSet::new(), &mut groups);
        inherit_masonry_facade_tags(&mut ways, &[], &groups);
        let part = ways[1].tags.as_ref().unwrap();
        assert_eq!(
            part.get("building:material").map(String::as_str),
            Some("stone")
        );
        assert_eq!(
            part.get("building:colour").map(String::as_str),
            Some("#cfc6b5")
        );
        let own = ways[2].tags.as_ref().unwrap();
        assert!(!own.contains_key("building:material"));

        let glass = with_tag(
            tagged(square_way(100, 1000, 1.0).0, "building", "office"),
            "building:material",
            "glass",
        );
        let glass = with_tag(glass, "building:colour", "#8085a2");
        let mut ways = vec![
            glass,
            tagged(square_way(200, 2000, 0.5).0, "building:part", "yes"),
        ];
        inherit_masonry_facade_tags(&mut ways, &[], &groups);
        assert!(!ways[1]
            .tags
            .as_ref()
            .unwrap()
            .contains_key("building:colour"));
    }

    // The parts of a block of flats carry the housing mark; it leaves the
    // group key and every roll made from the seed as they were.
    #[test]
    fn spatial_parts_of_flats_carry_the_housing_mark() {
        let (o, on) = square_way(100, 1000, 1.0);
        let (p, pn) = square_way(200, 2000, 0.5);
        let ways = [
            tagged(o, "building", "apartments"),
            tagged(p, "building:part", "yes"),
        ];
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).collect();
        let mut groups = PartGroups::new();
        compute_spatial_part_suppression(&ways, &nodes, &HashSet::new(), &mut groups);
        let seed = groups[&200];
        assert!(residential_group_from_seed(seed));
        assert_eq!(seed_without_hint(seed), 100);
        assert_eq!(seed_without_residential_mark(seed), 100);
        assert!(!residential_group_from_seed(100));
    }

    fn tagmap(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn hint_is_masonry_for_historic_material_and_style() {
        let m = |p: &[(&str, &str)]| building_style_hint(&tagmap(p));
        assert_eq!(m(&[("historic", "building")]), StyleHint::Masonry);
        assert_eq!(m(&[("heritage", "2")]), StyleHint::Masonry);
        assert_eq!(m(&[("ref:nrhp", "79001603")]), StyleHint::Masonry);
        assert_eq!(m(&[("listed_status", "Grade II*")]), StyleHint::Masonry);
        assert_eq!(m(&[("building:material", "sandstone")]), StyleHint::Masonry);
        assert_eq!(m(&[("building:material", "Brick")]), StyleHint::Masonry);
        // separator variants both normalize to a match
        assert_eq!(
            m(&[("building:architecture", "art_deco")]),
            StyleHint::Masonry
        );
        assert_eq!(m(&[("architecture", "neo-gothic")]), StyleHint::Masonry);
        // pre-1945 construction year
        assert_eq!(m(&[("start_date", "1902")]), StyleHint::Masonry);
        assert_eq!(
            m(&[("year_of_construction", "1911-1913")]),
            StyleHint::Masonry
        );
    }

    #[test]
    fn hint_is_contemporary_for_concrete_and_modern() {
        let m = |p: &[(&str, &str)]| building_style_hint(&tagmap(p));
        assert_eq!(
            m(&[("building:material", "concrete")]),
            StyleHint::Contemporary
        );
        assert_eq!(
            m(&[("building:material", "reinforced_concrete")]),
            StyleHint::Contemporary
        );
        assert_eq!(
            m(&[("building:architecture", "modern")]),
            StyleHint::Contemporary
        );
    }

    #[test]
    fn a_listed_modern_building_is_not_masonry() {
        let m = |p: &[(&str, &str)]| building_style_hint(&tagmap(p));
        assert_eq!(
            m(&[("heritage", "yes"), ("start_date", "1969")]),
            StyleHint::None
        );
        assert_eq!(
            m(&[("heritage", "yes"), ("building:architecture", "modernism")]),
            StyleHint::Contemporary
        );
        assert_eq!(
            m(&[("heritage", "yes"), ("start_date", "1911")]),
            StyleHint::Masonry
        );
        assert_eq!(
            m(&[
                ("building", "church"),
                ("heritage", "4"),
                ("start_date", "1954")
            ]),
            StyleHint::Masonry
        );
    }

    #[test]
    fn glass_material_overrides_heritage() {
        let m = |p: &[(&str, &str)]| building_style_hint(&tagmap(p));
        // the Seagram case: self-declared glass wins over a heritage listing
        assert_eq!(
            m(&[("building:material", "glass"), ("heritage", "yes")]),
            StyleHint::Glass
        );
        assert_eq!(m(&[("roof:material", "glass")]), StyleHint::Glass);
    }

    #[test]
    fn hint_is_none_for_plain_buildings() {
        let m = |p: &[(&str, &str)]| building_style_hint(&tagmap(p));
        assert_eq!(m(&[("building", "commercial")]), StyleHint::None);
        assert_eq!(m(&[("start_date", "2012")]), StyleHint::None);
    }

    // A relation part is grouped under the salted relation id.
    #[test]
    fn relation_part_is_grouped_under_salted_relation_id() {
        let (outline, on) = square_way(100, 1000, 1.0);
        let (part, pn) = square_way(200, 2000, 0.8);
        let rel = building_relation(vec![
            member("way", 100, "outline"),
            member("way", 200, "part"),
        ]);
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).collect();
        let mut groups = PartGroups::new();
        compute_outline_suppression(&[rel], &[outline, part], &nodes, &mut groups);
        assert_eq!(groups.get(&200), Some(&(RELATION_SEED_BIT | 1)));
    }

    // --- compute_spatial_relation_part_suppression (multipolygon-relation outlines) ---

    // An axis-aligned square footprint (corner at origin, side `side`) built as FOUR OPEN outer
    // ways sharing endpoint node ids, plus the four corner nodes. Exercises the ring stitcher.
    fn split_square(
        way_ids: [u64; 4],
        node_ids: [u64; 4],
        side: f64,
    ) -> (Vec<OsmElement>, Vec<OsmElement>) {
        let corners = [(0.0, 0.0), (0.0, side), (side, side), (side, 0.0)];
        let nodes: Vec<OsmElement> = node_ids
            .iter()
            .zip(corners.iter())
            .map(|(&id, &(lat, lon))| node(id, lat, lon))
            .collect();
        let ways: Vec<OsmElement> = (0..4)
            .map(|i| OsmElement {
                r#type: "way".into(),
                id: way_ids[i],
                lat: None,
                lon: None,
                nodes: Some(vec![node_ids[i], node_ids[(i + 1) % 4]]),
                tags: None,
                members: Vec::new(),
            })
            .collect();
        (ways, nodes)
    }

    /// Defaults to `type=multipolygon`, since that is what the parser actually renders;
    /// pass an explicit `type` to model a relation the parser would drop.
    fn relation_el(id: u64, tags: &[(&str, &str)], members: Vec<OsmMember>) -> OsmElement {
        let mut map = tagmap(tags);
        map.entry("type".to_string())
            .or_insert_with(|| "multipolygon".to_string());
        OsmElement {
            r#type: "relation".into(),
            id,
            lat: None,
            lon: None,
            nodes: None,
            tags: Some(map),
            members,
        }
    }

    fn outer_members(way_ids: &[u64]) -> Vec<OsmMember> {
        way_ids
            .iter()
            .map(|&id| member("way", id, "outer"))
            .collect()
    }

    // The Colosseum case: a multipolygon building outline (building=amphitheatre, building:part=no)
    // whose ellipse is SPLIT across four outer ways, spatially covered by an independent
    // building:part RELATION. The outline relation is suppressed; the part relation survives.
    #[test]
    fn suppresses_outline_covered_by_part_relations() {
        // Small footprints keep the outline-bbox grid tiny; only the coverage ratio matters.
        let (outline_ways, outline_nodes) =
            split_square([100, 101, 102, 103], [10, 11, 12, 13], 0.002);
        let (part_way, part_nodes) = square_way(200, 2000, 0.0016); // 0.0016^2 / 0.002^2 = 0.64
        assert!((0.0016_f64 / 0.002).powi(2) >= MIN_PART_COVERAGE);

        let outline = relation_el(
            1,
            &[("building", "amphitheatre"), ("building:part", "no")],
            outer_members(&[100, 101, 102, 103]),
        );
        let part = relation_el(2, &[("building:part", "yes")], outer_members(&[200]));

        let mut ways = outline_ways;
        ways.push(part_way);
        let nodes: Vec<OsmElement> = outline_nodes.into_iter().chain(part_nodes).collect();

        let s = compute_spatial_relation_part_suppression(&[outline, part], &ways, &nodes);
        assert!(s.contains(&("relation", 1)));
        assert!(!s.contains(&("relation", 2)));
    }

    // A courtyard block mapped as a multipolygon with its wings as building:part ways: the
    // ways cover the outline as part relations do, and join its group with its date.
    #[test]
    fn part_ways_cover_and_join_a_relation_outline() {
        let (outline_ways, outline_nodes) =
            split_square([100, 101, 102, 103], [10, 11, 12, 13], 0.002);
        let (part_way, part_nodes) = square_way(200, 2000, 0.0016);
        let outline = relation_el(
            1,
            &[("building", "apartments"), ("start_date", "1910")],
            outer_members(&[100, 101, 102, 103]),
        );
        let mut ways = outline_ways;
        ways.push(tagged(part_way, "building:part", "yes"));
        let nodes: Vec<OsmElement> = outline_nodes.into_iter().chain(part_nodes).collect();
        let mut groups = PartGroups::new();
        let s = spatial_relation_part_suppression(&[outline], &ways, &nodes, &mut groups);
        assert!(s.contains(&("relation", 1)));
        let seed = groups[&200];
        assert_eq!(group_key(seed), RELATION_SEED_BIT | 1);
        assert_eq!(year_from_seed(seed), Some(1910));
        assert!(residential_group_from_seed(seed));
    }

    // A part way standing in the block's courtyard, or underground, is not the block: it
    // neither covers the outline nor joins its group.
    #[test]
    fn courtyard_and_underground_part_ways_leave_the_outline() {
        let (outline_ways, outline_nodes) =
            split_square([100, 101, 102, 103], [10, 11, 12, 13], 0.002);
        let (court, court_nodes) = square_way(300, 3000, 0.0018);
        let (hall, hall_nodes) = square_way(200, 2000, 0.0016);
        let mut members = outer_members(&[100, 101, 102, 103]);
        members.push(member("way", 300, "inner"));
        let outline = relation_el(1, &[("building", "yes")], members);
        let mut ways = outline_ways;
        ways.push(court);
        ways.push(tagged(hall, "building:part", "yes"));
        let nodes: Vec<OsmElement> = outline_nodes
            .into_iter()
            .chain(court_nodes)
            .chain(hall_nodes)
            .collect();
        let mut groups = PartGroups::new();
        let s = spatial_relation_part_suppression(
            std::slice::from_ref(&outline),
            &ways,
            &nodes,
            &mut groups,
        );
        assert!(s.is_empty());
        assert!(groups.is_empty());

        let (cellar, cellar_nodes) = square_way(400, 4000, 0.0016);
        let mut cellar = tagged(cellar, "building:part", "yes");
        if let Some(t) = cellar.tags.as_mut() {
            t.insert("location".to_string(), "underground".to_string());
        }
        let (outline_ways, outline_nodes) =
            split_square([100, 101, 102, 103], [10, 11, 12, 13], 0.002);
        let outline = relation_el(
            1,
            &[("building", "yes")],
            outer_members(&[100, 101, 102, 103]),
        );
        let mut ways = outline_ways;
        ways.push(cellar);
        let nodes: Vec<OsmElement> = outline_nodes.into_iter().chain(cellar_nodes).collect();
        let s = spatial_relation_part_suppression(&[outline], &ways, &nodes, &mut groups);
        assert!(s.is_empty());
    }

    // parse_osm_data only turns type=multipolygon / type=building relations into renderable
    // elements. A part of any other type is never drawn, so letting it count toward coverage
    // would suppress the outline and leave nothing at all in its place.
    #[test]
    fn part_relation_the_parser_never_renders_cannot_suppress_the_outline() {
        for part_type in ["site", "group", "boundary"] {
            let (outline_ways, outline_nodes) =
                split_square([100, 101, 102, 103], [10, 11, 12, 13], 0.002);
            let (part_way, part_nodes) = square_way(200, 2000, 0.0016);
            // Geometry alone would clear the threshold; only the type must stop it.
            assert!((0.0016_f64 / 0.002).powi(2) >= MIN_PART_COVERAGE);

            let outline = relation_el(
                1,
                &[("building", "amphitheatre"), ("building:part", "no")],
                outer_members(&[100, 101, 102, 103]),
            );
            let part = relation_el(
                2,
                &[("type", part_type), ("building:part", "yes")],
                outer_members(&[200]),
            );

            let mut ways = outline_ways;
            ways.push(part_way);
            let nodes: Vec<OsmElement> = outline_nodes.into_iter().chain(part_nodes).collect();

            let s = compute_spatial_relation_part_suppression(&[outline, part], &ways, &nodes);
            assert!(
                !s.contains(&("relation", 1)),
                "outline suppressed by a type={part_type} part that never renders"
            );
        }
    }

    // Same stitched multi-way outline, but the only part covers ~36% (< MIN_PART_COVERAGE),
    // so the outline relation must survive.
    #[test]
    fn does_not_suppress_outline_with_insufficient_part_coverage() {
        let (outline_ways, outline_nodes) =
            split_square([100, 101, 102, 103], [10, 11, 12, 13], 0.002);
        let (part_way, part_nodes) = square_way(200, 2000, 0.0012); // 0.0012^2 / 0.002^2 = 0.36
        assert!((0.0012_f64 / 0.002).powi(2) < MIN_PART_COVERAGE);

        let outline = relation_el(
            1,
            &[("building", "amphitheatre")],
            outer_members(&[100, 101, 102, 103]),
        );
        let part = relation_el(2, &[("building:part", "yes")], outer_members(&[200]));

        let mut ways = outline_ways;
        ways.push(part_way);
        let nodes: Vec<OsmElement> = outline_nodes.into_iter().chain(part_nodes).collect();

        let s = compute_spatial_relation_part_suppression(&[outline, part], &ways, &nodes);
        assert!(!s.contains(&("relation", 1)));
    }

    // A plain building outline relation with NO building:part relations is never suppressed.
    #[test]
    fn plain_outline_relation_without_parts_is_kept() {
        let (outline_ways, outline_nodes) =
            split_square([100, 101, 102, 103], [10, 11, 12, 13], 0.002);
        let outline = relation_el(
            1,
            &[("building", "amphitheatre")],
            outer_members(&[100, 101, 102, 103]),
        );

        let s =
            compute_spatial_relation_part_suppression(&[outline], &outline_ways, &outline_nodes);
        assert!(s.is_empty());
    }

    // An amphitheatre whose only cover is an ELEVATED part relation (min_height > 0): that raised
    // volume floats above the ground, so it can't stand in for the footprint. The outline is kept
    // even though the part's geometric coverage (~64%) clears MIN_PART_COVERAGE.
    #[test]
    fn relation_outline_covered_only_by_elevated_part_is_kept() {
        let (outline_ways, outline_nodes) =
            split_square([100, 101, 102, 103], [10, 11, 12, 13], 0.002);
        let (part_way, part_nodes) = square_way(200, 2000, 0.0016); // 0.0016^2 / 0.002^2 = 0.64
        assert!((0.0016_f64 / 0.002).powi(2) >= MIN_PART_COVERAGE);

        let outline = relation_el(
            1,
            &[("building", "amphitheatre"), ("building:part", "no")],
            outer_members(&[100, 101, 102, 103]),
        );
        // min_height marks the part as elevated, so part_covers_ground() returns false.
        let part = relation_el(
            2,
            &[("building:part", "yes"), ("min_height", "54")],
            outer_members(&[200]),
        );

        let mut ways = outline_ways;
        ways.push(part_way);
        let nodes: Vec<OsmElement> = outline_nodes.into_iter().chain(part_nodes).collect();

        let s = compute_spatial_relation_part_suppression(&[outline, part], &ways, &nodes);
        assert!(!s.contains(&("relation", 1)));
    }
}

#[cfg(test)]
mod osm_xml_tests {
    use super::*;
    use std::io::Cursor;

    fn parse(xml: &str) -> (OsmData, Option<LLBBox>) {
        parse_osm_xml(Cursor::new(xml.as_bytes())).expect("valid OSM XML")
    }

    fn find<'a>(data: &'a OsmData, kind: &str, id: u64) -> &'a OsmElement {
        data.elements
            .iter()
            .find(|e| e.r#type == kind && e.id == id)
            .unwrap_or_else(|| panic!("missing {kind} {id}"))
    }

    // Testing every element kind: tagged node, bare node, way (nd refs + tags),
    // relation (member + tags), and a <bounds> element.
    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<osm version="0.6" generator="test">
  <bounds minlat="1.0" minlon="2.0" maxlat="3.0" maxlon="4.0"/>
  <node id="10" lat="1.5" lon="2.5"><tag k="amenity" v="cafe"/></node>
  <node id="11" lat="1.6" lon="2.6"/>
  <way id="20">
    <nd ref="10"/>
    <nd ref="11"/>
    <tag k="building" v="yes"/>
    <tag k="name" v="Ben &amp; Jerry"/>
  </way>
  <relation id="30">
    <member type="way" ref="20" role="outer"/>
    <tag k="type" v="multipolygon"/>
  </relation>
</osm>"#;

    // A beach mapped as a multipolygon whose outer ring is split over two ways, with one of
    // the joins outside the bbox, as the Baltic beaches are.
    const SPLIT_BEACH: &str = r#"<osm version="0.6">
  <bounds minlat="54.800" minlon="18.370" maxlat="54.810" maxlon="18.390"/>
  <node id="1" lat="54.802" lon="18.385"/>
  <node id="2" lat="54.802" lon="18.395"/>
  <node id="3" lat="54.804" lon="18.395"/>
  <node id="4" lat="54.804" lon="18.385"/>
  <way id="20"><nd ref="1"/><nd ref="2"/><nd ref="3"/></way>
  <way id="21"><nd ref="3"/><nd ref="4"/><nd ref="1"/></way>
  <relation id="30">
    <member type="way" ref="20" role="outer"/>
    <member type="way" ref="21" role="outer"/>
    <tag k="type" v="multipolygon"/>
    <tag k="natural" v="beach"/>
    <tag k="surface" v="sand"/>
  </relation>
</osm>"#;

    #[test]
    fn a_split_area_relation_arrives_as_one_closed_ring() {
        let (data, bounds) = parse(SPLIT_BEACH);
        let (elements, _, _, _) = parse_osm_data(
            data,
            bounds.unwrap(),
            false,
            &crate::projection::ProjectionSpec::local(1.0),
        );
        let rel = elements
            .iter()
            .find_map(|e| match e {
                ProcessedElement::Relation(r) if r.id == 30 => Some(r),
                _ => None,
            })
            .expect("the beach relation is kept");
        assert_eq!(rel.members.len(), 1, "two open ways make one ring");
        assert_eq!(rel.members[0].role, ProcessedMemberRole::Outer);
        let ring = &rel.members[0].way.nodes;
        let (first, last) = (&ring[0], ring.last().unwrap());
        assert_eq!((first.x, first.z), (last.x, last.z), "the ring is closed");
        assert!(rel.members[0].way.id >= 1 << 61, "the ring has its own id");
        let coords: Vec<(i32, i32)> = ring.iter().map(|n| (n.x, n.z)).collect();
        assert!(
            !crate::floodfill::flood_fill_area(&coords, None).is_empty(),
            "the ring fills, instead of leaving an outline"
        );
    }

    /// A pier relation split over open ways gets the same assembled ring.
    #[test]
    fn a_split_pier_relation_arrives_as_one_closed_ring() {
        let xml = SPLIT_BEACH.replace(
            r#"<tag k="natural" v="beach"/>"#,
            r#"<tag k="man_made" v="pier"/>"#,
        );
        let (data, bounds) = parse(&xml);
        let (elements, _, _, _) = parse_osm_data(
            data,
            bounds.unwrap(),
            false,
            &crate::projection::ProjectionSpec::local(1.0),
        );
        let rel = elements
            .iter()
            .find_map(|e| match e {
                ProcessedElement::Relation(r) if r.id == 30 => Some(r),
                _ => None,
            })
            .expect("the pier relation is kept");
        assert_eq!(rel.members.len(), 1, "two open ways make one ring");
        let ring = &rel.members[0].way.nodes;
        let (first, last) = (&ring[0], ring.last().unwrap());
        assert_eq!((first.x, first.z), (last.x, last.z), "the ring is closed");
    }

    fn relation(elements: &[ProcessedElement], id: u64) -> &ProcessedRelation {
        elements
            .iter()
            .find_map(|e| match e {
                ProcessedElement::Relation(r) if r.id == id => Some(r),
                _ => None,
            })
            .expect("the relation is kept")
    }

    // A wood with a clearing, rings closed, roles as given.
    fn wood(outer: &str, clearing: &str, way_tags: &str) -> String {
        wood_tagged(
            outer,
            clearing,
            way_tags,
            r#"<tag k="landuse" v="forest"/>"#,
        )
    }

    fn wood_tagged(outer: &str, clearing: &str, way_tags: &str, rel_tags: &str) -> String {
        format!(
            r#"<osm version="0.6">
  <bounds minlat="54.800" minlon="18.370" maxlat="54.810" maxlon="18.390"/>
  <node id="1" lat="54.801" lon="18.371"/>
  <node id="2" lat="54.801" lon="18.389"/>
  <node id="3" lat="54.809" lon="18.389"/>
  <node id="4" lat="54.809" lon="18.371"/>
  <node id="5" lat="54.803" lon="18.375"/>
  <node id="6" lat="54.803" lon="18.385"/>
  <node id="7" lat="54.807" lon="18.385"/>
  <node id="8" lat="54.807" lon="18.375"/>
  <way id="20"><nd ref="1"/><nd ref="2"/><nd ref="3"/><nd ref="4"/><nd ref="1"/>{way_tags}</way>
  <way id="21"><nd ref="5"/><nd ref="6"/><nd ref="7"/><nd ref="8"/><nd ref="5"/></way>
  <relation id="30">
    <member type="way" ref="20" role="{outer}"/>
    <member type="way" ref="21" role="{clearing}"/>
    <tag k="type" v="multipolygon"/>
    {rel_tags}
  </relation>
</osm>"#
        )
    }

    // The same wood with its outer ring split over two ways, and more members as given.
    fn split_wood(members: &str, ways: &str) -> String {
        format!(
            r#"<osm version="0.6">
  <bounds minlat="54.800" minlon="18.370" maxlat="54.810" maxlon="18.390"/>
  <node id="1" lat="54.801" lon="18.371"/>
  <node id="2" lat="54.801" lon="18.389"/>
  <node id="3" lat="54.809" lon="18.389"/>
  <node id="4" lat="54.809" lon="18.371"/>
  <node id="5" lat="54.803" lon="18.375"/>
  <node id="6" lat="54.803" lon="18.385"/>
  <node id="7" lat="54.807" lon="18.385"/>
  <node id="8" lat="54.807" lon="18.375"/>
  <node id="9" lat="54.805" lon="18.380"/>
  <way id="21"><nd ref="5"/><nd ref="6"/><nd ref="7"/><nd ref="8"/><nd ref="5"/></way>
  <way id="22"><nd ref="1"/><nd ref="2"/><nd ref="3"/></way>
  <way id="23"><nd ref="3"/><nd ref="4"/><nd ref="1"/></way>
  {ways}
  <relation id="31">
    <member type="way" ref="22" role="outer"/>
    <member type="way" ref="23" role="outer"/>
    {members}
    <tag k="type" v="multipolygon"/>
    <tag k="landuse" v="forest"/>
  </relation>
</osm>"#
        )
    }

    fn roles_of(rel: &ProcessedRelation) -> Vec<ProcessedMemberRole> {
        rel.members.iter().map(|m| m.role.clone()).collect()
    }

    #[test]
    fn a_blank_ring_in_a_split_outer_is_its_hole() {
        let elements = parse_local(&split_wood(r#"<member type="way" ref="21" role=""/>"#, ""));
        assert_eq!(
            roles_of(relation(&elements, 31)),
            vec![ProcessedMemberRole::Outer, ProcessedMemberRole::Inner]
        );
    }

    #[test]
    fn a_stray_blank_way_does_not_break_the_ring() {
        let elements = parse_local(&split_wood(
            r#"<member type="way" ref="24" role=""/>"#,
            r#"<way id="24"><nd ref="1"/><nd ref="9"/></way>"#,
        ));
        let rel = relation(&elements, 31);
        assert_eq!(roles_of(rel), vec![ProcessedMemberRole::Outer]);
        let ring = &rel.members[0].way.nodes;
        assert_eq!(
            (ring[0].x, ring[0].z),
            (ring[ring.len() - 1].x, ring[ring.len() - 1].z)
        );
    }

    fn parse_local(xml: &str) -> Vec<ProcessedElement> {
        let (data, bounds) = parse(xml);
        let spec = crate::projection::ProjectionSpec::local(1.0);
        parse_osm_data(data, bounds.unwrap(), false, &spec).0
    }

    #[test]
    fn blank_roles_are_read_from_the_rings() {
        let elements = parse_local(&wood("", "", ""));
        let roles: Vec<_> = relation(&elements, 30)
            .members
            .iter()
            .map(|m| (m.way.id, m.role.clone()))
            .collect();
        assert_eq!(
            roles,
            vec![
                (20, ProcessedMemberRole::Outer),
                (21, ProcessedMemberRole::Inner)
            ],
            "the ring inside the other is its hole"
        );
        let elements = parse_local(&wood("outer", "", ""));
        assert_eq!(
            relation(&elements, 30).members[1].role,
            ProcessedMemberRole::Inner
        );
    }

    #[test]
    fn an_outer_way_repeating_its_relation_leaves_the_hole_to_it() {
        let way_kept = |xml: String| {
            parse_local(&xml)
                .iter()
                .any(|e| matches!(e, ProcessedElement::Way(w) if w.id == 20))
        };
        let forest = r#"<tag k="landuse" v="forest"/><tag k="name" v="Hart"/>"#;
        assert!(
            !way_kept(wood("outer", "inner", forest)),
            "the relation draws it"
        );
        assert!(
            !way_kept(wood("outer", "", forest)),
            "a blank-role clearing is a hole too"
        );
        assert!(way_kept(wood(
            "outer",
            "",
            r#"<tag k="landuse" v="meadow"/>"#
        )));
        let fenced = r#"<tag k="landuse" v="forest"/><tag k="barrier" v="fence"/>"#;
        assert!(
            way_kept(wood("outer", "inner", fenced)),
            "the fence is its own"
        );
        // A relation that draws no area leaves its ways alone.
        let tank = r#"<tag k="man_made" v="storage_tank"/>"#;
        assert!(way_kept(wood_tagged("outer", "inner", tank, tank)));
        let elements = parse_local(&wood("outer", "inner", forest));
        assert_eq!(relation(&elements, 30).members[0].way.id, 20);
    }

    #[test]
    fn split_rings_keep_the_clip_margin() {
        let (data, bounds) = parse(SPLIT_BEACH);
        let spec = crate::projection::ProjectionSpec {
            clip_pad: 64,
            ..crate::projection::ProjectionSpec::local(1.0)
        };
        let (elements, xzbbox, _, _) = parse_osm_data(data, bounds.unwrap(), false, &spec);
        let ring = &relation(&elements, 30).members[0].way.nodes;
        let max_x = ring.iter().map(|n| n.x).max().unwrap();
        assert_eq!(
            max_x,
            xzbbox.max_x() + 64,
            "the beach runs on into the margin"
        );
    }

    #[test]
    fn parses_bounds_into_llbbox() {
        let (_, bounds) = parse(SAMPLE);
        assert_eq!(bounds, Some(LLBBox::new(1.0, 2.0, 3.0, 4.0).unwrap()));
    }

    #[test]
    fn parses_expected_element_count() {
        let (data, _) = parse(SAMPLE);
        // 2 nodes + 1 way + 1 relation; <bounds> is not an element.
        assert_eq!(data.elements.len(), 4);
    }

    #[test]
    fn parses_node_with_and_without_tags() {
        let (data, _) = parse(SAMPLE);

        let tagged = find(&data, "node", 10);
        assert_eq!(tagged.lat, Some(1.5));
        assert_eq!(tagged.lon, Some(2.5));
        assert_eq!(
            tagged.tags.as_ref().unwrap().get("amenity"),
            Some(&"cafe".to_string())
        );

        // A node with no <tag> children mirrors Overpass JSON: tags is None, not an empty map.
        let bare = find(&data, "node", 11);
        assert_eq!(bare.lat, Some(1.6));
        assert!(bare.tags.is_none());
    }

    #[test]
    fn parses_way_node_refs_and_tags() {
        let (data, _) = parse(SAMPLE);
        let way = find(&data, "way", 20);
        assert_eq!(way.nodes, Some(vec![10, 11]));
        let tags = way.tags.as_ref().unwrap();
        assert_eq!(tags.get("building"), Some(&"yes".to_string()));
        // XML entities in attribute values are unescaped.
        assert_eq!(tags.get("name"), Some(&"Ben & Jerry".to_string()));
    }

    #[test]
    fn parses_relation_members_and_tags() {
        let (data, _) = parse(SAMPLE);
        let relation = find(&data, "relation", 30);
        assert_eq!(relation.members.len(), 1);
        let member = &relation.members[0];
        assert_eq!(member.r#type, "way");
        assert_eq!(member.r#ref, 20);
        assert_eq!(member.r#role, "outer");
        assert_eq!(
            relation.tags.as_ref().unwrap().get("type"),
            Some(&"multipolygon".to_string())
        );
    }

    // Without a <bounds> element, the parser returns None and OsmData::bounds() derives the
    // node-coordinate extent instead.
    const NO_BOUNDS: &str = r#"<osm version="0.6">
  <node id="1" lat="10.0" lon="20.0"/>
  <node id="2" lat="12.0" lon="24.0"/>
  <node id="3" lat="11.0" lon="22.0"/>
</osm>"#;

    #[test]
    fn missing_bounds_falls_back_to_node_extent() {
        let (data, bounds) = parse(NO_BOUNDS);
        assert!(bounds.is_none());
        assert_eq!(
            data.bounds(),
            Some(LLBBox::new(10.0, 20.0, 12.0, 24.0).unwrap())
        );
    }

    #[test]
    fn bounds_is_none_without_located_nodes() {
        let data = OsmData::empty();
        assert!(data.bounds().is_none());
    }

    #[test]
    fn resolve_bbox_precedence() {
        let explicit = LLBBox::new(0.0, 0.0, 1.0, 1.0).unwrap();
        let file = LLBBox::new(5.0, 5.0, 6.0, 6.0).unwrap();
        let (data_with_bounds, _) = parse(SAMPLE);
        let (data_no_bounds, _) = parse(NO_BOUNDS);
        let node_extent = data_no_bounds.bounds().unwrap();

        // 1--bbox wins over a file <bounds>
        assert_eq!(
            resolve_bbox(Some(explicit), Some(file), &data_with_bounds),
            Some(explicit)
        );
        // file <bounds> used when --bbox is absent
        assert_eq!(
            resolve_bbox(None, Some(file), &data_with_bounds),
            Some(file)
        );
        // node-extent fallback when neither --bbox nor <bounds> is present
        assert_eq!(resolve_bbox(None, None, &data_no_bounds), Some(node_extent));
        // nothing resolvable -> None
        assert_eq!(resolve_bbox(None, None, &OsmData::empty()), None);
    }
}

#[cfg(test)]
mod arch_era_tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn era_derivation_table() {
        use ArchEra::*;
        let cases: &[(&[(&str, &str)], ArchEra)] = &[
            (&[("historic", "yes")], HistoricOrnate),
            (&[("heritage", "2")], HistoricOrnate),
            (&[("historic", "no")], Unknown),
            (&[("building:architecture", "art_deco")], HistoricOrnate),
            (&[("building:architecture", "brutalist")], PostWarPanel),
            (&[("building:architecture", "bauhaus")], Contemporary),
            (&[("start_date", "1890")], TraditionalPreWar),
            (&[("start_date", "1965")], PostWarPanel),
            (&[("start_date", "2003-05")], Contemporary),
            (&[("building:material", "panel")], PostWarPanel),
            (&[("building:material", "brick")], TraditionalPreWar),
            (&[("building:material", "concrete")], Contemporary),
            (&[("building:cladding", "plaster")], TraditionalPreWar),
            (&[("building", "house")], Unknown),
            // a listing makes an old or undated building ornate
            (&[("heritage", "1"), ("start_date", "1902")], HistoricOrnate),
            // but not one put up after the war, nor one of the modern movement
            (&[("heritage", "1"), ("start_date", "1995")], Contemporary),
            (
                &[
                    ("heritage", "yes"),
                    ("building:architecture", "modernism"),
                    ("start_date", "1969"),
                ],
                PostWarPanel,
            ),
            (
                &[
                    ("heritage", "4"),
                    ("building:architecture", "modern"),
                    ("start_date", "1932"),
                ],
                TraditionalPreWar,
            ),
            // an ornate style wins over a later date, the first of a list too
            (
                &[("building:architecture", "baroque"), ("start_date", "1995")],
                HistoricOrnate,
            ),
            (
                &[
                    ("building:architecture", "gothic;baroque"),
                    ("start_date", "1954"),
                ],
                HistoricOrnate,
            ),
            // a listed church dated after the war was rebuilt in its old form
            (
                &[
                    ("building", "church"),
                    ("heritage", "4"),
                    ("start_date", "1954-06-27"),
                ],
                HistoricOrnate,
            ),
        ];
        for (pairs, expected) in cases {
            assert_eq!(building_arch_era(&tags(pairs)), *expected, "tags {pairs:?}");
        }
    }

    #[test]
    fn a_part_takes_the_year_of_its_building() {
        // An undated part of a 1962 tower is postwar, its own date still wins.
        assert_eq!(
            building_arch_era_dated(&tags(&[("building:part", "yes")]), Some(1962)),
            ArchEra::PostWarPanel
        );
        assert_eq!(
            building_arch_era_dated(&tags(&[("building:part", "yes")]), None),
            ArchEra::Unknown
        );
        let seed = seed_with_year(seed_with_hint(4242, StyleHint::Glass), Some(1962));
        assert_eq!(year_from_seed(seed), Some(1962));
        assert_eq!(style_hint_from_seed(seed), StyleHint::Glass);
        assert_eq!(group_key(seed), 4242);
        assert_eq!(
            seed_without_year(seed),
            seed_with_hint(4242, StyleHint::Glass)
        );
        assert_eq!(year_from_seed(seed_with_year(4242, None)), None);
        assert_eq!(year_from_seed(4242), None);
    }

    #[test]
    fn part_hint_fallback_mapping() {
        assert_eq!(
            arch_era_from_hint(StyleHint::Masonry),
            ArchEra::TraditionalPreWar
        );
        assert_eq!(arch_era_from_hint(StyleHint::Glass), ArchEra::Contemporary);
        assert_eq!(arch_era_from_hint(StyleHint::None), ArchEra::Unknown);
    }
}

#[cfg(test)]
mod priority_sort_tests {
    use super::*;

    fn way(id: u64, pairs: &[(&str, &str)]) -> ProcessedElement {
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes: Vec::new(),
            tags: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    #[test]
    fn landuse_renders_after_everything_it_surrounds() {
        let mut elements = vec![
            way(1, &[("landuse", "meadow")]),
            way(2, &[("man_made", "mast")]),
            way(3, &[("amenity", "parking")]),
            way(4, &[("building:part", "yes")]),
            way(5, &[("highway", "service")]),
            way(6, &[("building", "yes")]),
        ];
        sort_by_priority(&mut elements);
        let order: Vec<u64> = elements.iter().map(ProcessedElement::id).collect();
        assert_eq!(order, vec![6, 5, 3, 4, 2, 1]);
    }

    #[test]
    fn a_building_or_reservoir_with_a_landuse_tag_keeps_its_place() {
        let mut elements = vec![
            way(1, &[("landuse", "meadow")]),
            way(2, &[("building", "retail"), ("landuse", "retail")]),
            way(3, &[("building:part", "yes")]),
            way(4, &[("water", "reservoir"), ("landuse", "reservoir")]),
            way(5, &[("man_made", "pier")]),
        ];
        sort_by_priority(&mut elements);
        let order: Vec<u64> = elements.iter().map(ProcessedElement::id).collect();
        assert_eq!(order, vec![2, 4, 3, 5, 1]);
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn roads_that_are_not_built_lose_their_highway_tag() {
        for value in ["proposed", "razed"] {
            let kept = filter_tags(tags(&[
                ("highway", value),
                ("bridge", "yes"),
                ("layer", "1"),
            ]));
            assert!(kept.is_empty(), "highway={value} kept {kept:?}");
        }
        // A railway on the same bridge keeps it.
        let kept = filter_tags(tags(&[
            ("highway", "proposed"),
            ("railway", "rail"),
            ("bridge", "yes"),
        ]));
        assert_eq!(kept.get("bridge").map(String::as_str), Some("yes"));
        let kept = filter_tags(tags(&[("highway", "construction")]));
        assert_eq!(
            kept.get("highway").map(String::as_str),
            Some("construction")
        );
    }

    #[test]
    fn a_landform_is_no_natural_area_but_its_other_tags_stay() {
        for landform in ["peninsula", "mountain_range", "valley", "coastline"] {
            let kept = filter_tags(tags(&[("natural", landform), ("barrier", "wall")]));
            assert!(!kept.contains_key("natural"), "natural={landform} kept");
            assert_eq!(kept.get("barrier").map(String::as_str), Some("wall"));
        }
        let kept = filter_tags(tags(&[("natural", "wood")]));
        assert_eq!(kept.get("natural").map(String::as_str), Some("wood"));
    }
}
