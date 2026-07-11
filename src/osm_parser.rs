use crate::clipping::clip_way_to_bbox;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use serde::Deserialize;
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
        !IGNORED_TAGS.contains(&k.as_str()) && !IGNORED_PREFIXES.iter().any(|p| k.starts_with(p))
    });
    tags
}

// Raw data from OSM

#[derive(Debug, Deserialize)]
struct OsmMember {
    r#type: String,
    r#ref: u64,
    r#role: String,
}

#[derive(Debug, Deserialize)]
struct OsmElement {
    pub r#type: String,
    pub id: u64,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub nodes: Option<Vec<u64>>,
    pub tags: Option<HashMap<String, String>>,
    #[serde(default)]
    pub members: Vec<OsmMember>,
}

#[derive(Debug, Deserialize)]
pub struct OsmData {
    elements: Vec<OsmElement>,
    #[serde(default)]
    pub remark: Option<String>,
}

impl OsmData {
    /// Returns true if there are no elements in the OSM data
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }
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
    seed & !STYLE_HINT_MASK
}

fn seed_with_hint(seed: u64, hint: StyleHint) -> u64 {
    (seed & !STYLE_HINT_MASK) | ((hint as u64) << STYLE_HINT_SHIFT)
}

// lowercase, strip whitespace/_/- so art_deco, neo-gothic, "concrete masonry unit" all collapse
fn norm_tag(v: &str) -> String {
    v.chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect()
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

    // Masonry / historic. `no` on these keys is an explicit negation, not a signal.
    let present_and_not_no =
        |key: &str| tags.get(key).is_some_and(|v| !v.eq_ignore_ascii_case("no"));
    if present_and_not_no("historic")
        || present_and_not_no("heritage")
        || tags.contains_key("ref:nrhp")
        || present_and_not_no("listed_status")
    {
        return StyleHint::Masonry;
    }
    const MASONRY: &[&str] = &[
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
    if material.as_deref().is_some_and(|m| MASONRY.contains(&m)) {
        return StyleHint::Masonry;
    }
    if let Some(c) = tags.get("building:cladding") {
        const MASONRY_CLADDING: &[&str] = &[
            "brick",
            "brickmonolith",
            "plaster",
            "rendered",
            "rendering",
            "stone",
            "tiling",
        ];
        if MASONRY_CLADDING.contains(&norm_tag(c).as_str()) {
            return StyleHint::Masonry;
        }
    }
    let arch = tags
        .get("building:architecture")
        .or_else(|| tags.get("architecture"))
        .map(|a| norm_tag(a));
    if let Some(a) = arch.as_deref() {
        const HISTORIC_STYLES: &[&str] = &[
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
            "brutalist",
            "constructivism",
            "stalinistneoclassicism",
            "wilhelminianstyle",
            "queenanne",
        ];
        const MODERN_STYLES: &[&str] = &[
            "modern",
            "contemporary",
            "modernism",
            "functionalism",
            "newobjectivity",
            "postmodern",
            "bauhaus",
        ];
        if HISTORIC_STYLES.contains(&a) {
            return StyleHint::Masonry;
        }
        if MODERN_STYLES.contains(&a) {
            return StyleHint::Contemporary;
        }
    }
    // Pre-curtain-wall era: load-bearing masonry. start_date is the best-populated source.
    for key in ["start_date", "construction_date", "year_of_construction"] {
        if let Some(y) = tags.get(key).and_then(|v| first_year(v)) {
            if y < 1945 {
                return StyleHint::Masonry;
            }
            break; // known modern year; fall through to the concrete check
        }
    }

    // Concrete frame reads as a solid facade with windows: the contemporary middle style.
    const CONCRETE: &[&str] = &[
        "concrete",
        "reinforcedconcrete",
        "concretereinforced",
        "concretemasonryunit",
    ];
    if material.as_deref().is_some_and(|m| CONCRETE.contains(&m)) {
        return StyleHint::Contemporary;
    }
    StyleHint::None
}

pub fn parse_osm_data(
    osm_data: OsmData,
    bbox: LLBBox,
    scale: f64,
    debug: bool,
    projection: crate::projection::ProjectionKind,
) -> (
    Vec<ProcessedElement>,
    XZBBox,
    OutlineSuppression,
    PartGroups,
) {
    println!("{} Parsing data...", "[2/7]".bold());
    println!("Bounding box: {bbox:?}");

    // Deserialize the JSON data into the OSMData structure
    let data = SplitOsmData::from_raw_osm_data(osm_data);

    let (coord_transformer, xzbbox) = match projection {
        crate::projection::ProjectionKind::WebMercator => {
            let origin_lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
            let origin_lon = (bbox.min().lng() + bbox.max().lng()) / 2.0;
            let proj = crate::projection::WebMercatorProjection::new(origin_lat, origin_lon, scale);
            CoordTransformer::with_projection(&bbox, scale, &proj)
        }
        crate::projection::ProjectionKind::Local => {
            CoordTransformer::llbbox_to_xzbbox(&bbox, scale)
        }
    }
    .unwrap_or_else(|e| {
        eprintln!("Error in defining coordinate transformation:\n{e}");
        panic!();
    });

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

    let mut nodes_map: HashMap<u64, ProcessedNode> = HashMap::new();
    let mut ways_map: HashMap<u64, Arc<ProcessedWay>> = HashMap::new();

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

    // Second pass: process ways and clip them to bbox
    for element in data.ways {
        if ways_map.contains_key(&element.id) {
            continue;
        }
        let mut nodes: Vec<ProcessedNode> = vec![];
        if let Some(node_ids) = &element.nodes {
            for &node_id in node_ids {
                if let Some(node) = nodes_map.get(&node_id) {
                    nodes.push(node.clone());
                }
            }
        }

        // Clip the way to bbox to reduce node count dramatically
        let tags = filter_tags(element.tags.unwrap_or_default());

        // Store unclipped way for relation assembly (clipping happens after ring merging)
        let way = Arc::new(ProcessedWay {
            id: element.id,
            tags,
            nodes,
        });
        ways_map.insert(element.id, Arc::clone(&way));

        // Clip way nodes for standalone way processing (not relations)
        let clipped_nodes = clip_way_to_bbox(&way.nodes, &xzbbox);

        // Skip ways that are completely outside the bbox (empty after clipping)
        if clipped_nodes.is_empty() {
            continue;
        }

        let processed: ProcessedWay = ProcessedWay {
            id: element.id,
            tags: way.tags.clone(),
            nodes: clipped_nodes,
        };

        processed_elements.push(ProcessedElement::Way(processed));
    }

    // Third pass: process relations and clip member ways
    let mut seen_relations: HashSet<u64> = HashSet::new();
    for element in data.relations {
        if !seen_relations.insert(element.id) {
            continue;
        }
        let Some(tags) = &element.tags else {
            continue;
        };

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

        let members: Vec<ProcessedMember> = element
            .members
            .iter()
            .filter_map(|mem: &OsmMember| {
                if mem.r#type != "way" {
                    if mem.r#type != "relation" && mem.r#type != "node" {
                        eprintln!("WARN: Unknown relation member type \"{}\"", mem.r#type);
                    }
                    return None;
                }

                let trimmed_role = mem.role.trim();
                let role = if trimmed_role.eq_ignore_ascii_case("outer")
                    || trimmed_role.eq_ignore_ascii_case("outline")
                {
                    ProcessedMemberRole::Outer
                } else if trimmed_role.eq_ignore_ascii_case("inner") {
                    ProcessedMemberRole::Inner
                } else if trimmed_role.eq_ignore_ascii_case("part") {
                    if relation_type == Some("building") {
                        // "part" role only applies to type=building relations.
                        ProcessedMemberRole::Part
                    } else {
                        // For multipolygon relations, "part" is not a valid role, skip.
                        return None;
                    }
                } else if is_building_relation {
                    ProcessedMemberRole::Outer
                } else {
                    return None;
                };

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
                    let clipped_nodes = clip_way_to_bbox(&way.nodes, &xzbbox);
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
            processed_elements.push(ProcessedElement::Relation(ProcessedRelation {
                id: element.id,
                members,
                tags: filter_tags(tags.clone()),
            }));
        }
    }

    emit_gui_progress_update(18.5, "");

    drop(nodes_map);
    drop(ways_map);

    (processed_elements, xzbbox, outline_suppression, part_groups)
}

// Parts replace the outline only when they cover at least this much of it.
const MIN_PART_COVERAGE: f64 = 0.5;

// A part covers the outline's ground footprint only if it starts at ground level.
// Elevated parts (min_height / building:min_level > 0) model raised roof/dome volumes
// that float above the ground (e.g. S3DB churches), so they can't stand in for the outline.
fn part_covers_ground(tags: &HashMap<String, String>) -> bool {
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
    let mut way_ground: HashMap<u64, bool> = HashMap::new();
    for w in ways.iter().filter(|w| needed_ways.contains(&w.id)) {
        if let Some(ns) = w.nodes.as_ref() {
            way_nodes.insert(w.id, ns);
        }
        if let Some(t) = w.tags.as_ref() {
            way_hint.insert(w.id, building_style_hint(t));
            way_ground.insert(w.id, part_covers_ground(t));
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
                        seed_with_hint(RELATION_SEED_BIT | rel.id, rel_hint),
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
    let mut way_ground: HashMap<u64, bool> = HashMap::new();
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
        } else {
            continue;
        }
        way_nodes.insert(w.id, ns);
        way_hint.insert(w.id, building_style_hint(tags));
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

    // shoelace area, lon scaled by cos(lat)
    let area = |r: &[(f64, f64)]| -> f64 {
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
    };

    let point_in_ring = |lat: f64, lon: f64, r: &[(f64, f64)]| -> bool {
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
    };

    struct OutlineGeom {
        id: u64,
        ring: Vec<(f64, f64)>,
        area: f64,
    }

    // grid of outline bboxes so each part only tests nearby outlines
    const CELL: f64 = 0.0005;
    let cell = |lat: f64, lon: f64| ((lat / CELL).floor() as i64, (lon / CELL).floor() as i64);

    let mut geoms: Vec<OutlineGeom> = Vec::new();
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for id in outline_ids {
        let r = ring(id);
        let a = area(&r);
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
        let pa = area(&r);
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
            part_group.insert(pid, seed_with_hint(geoms[gi].id, hint));
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
    // Return a default priority if none of the tags match
    PRIORITY_ORDER.len()
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
}
