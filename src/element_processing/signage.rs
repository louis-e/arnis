//! Decides which decal each element needs, then places the frames. Java only.
//! Rendering lives in `crate::decals`.

use crate::args::{Args, SignageLevel};
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::decals::font;
use crate::decals::region::SignRegion;
use crate::decals::{pictograms, DecalKey, DecalRegistry, ShieldStyle, TextStyle, TrafficSign};
use crate::element_processing::building_facade::FacadeAnchor;
use crate::element_processing::buildings::is_underground_building;
use crate::element_processing::highways::{collect_carriageway_coords, highway_block_range};
use crate::element_processing::{get_nearest_non_road_block, get_nearest_road_block};
use crate::floodfill_cache::{BuildingFootprintBitmap, RoadMaskBitmap};
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;

/// Longest name rendered on a plate; longer ones are cut in the renderer anyway.
const MAX_NAME_CHARS: usize = 60;

/// One street name blade at an intersection.
#[derive(Clone, Debug)]
pub struct Blade {
    pub name: String,
    /// Unit direction of the street at the node.
    pub dir: (f64, f64),
    /// Half width of the widest way carrying this name at the node.
    pub half_width: i32,
}

/// A street name post at a node where two or more named streets meet.
#[derive(Clone, Debug)]
pub struct IntersectionPost {
    /// The way that places the post (smallest way id among the participants).
    pub owner_way: u64,
    pub blades: Vec<Blade>,
}

/// Node id -> post; built once before element processing.
pub type IntersectionIndex = HashMap<u64, IntersectionPost>;

/// Everything the placement code needs; shared read-only across tile threads.
pub struct SignageContext {
    pub registry: DecalRegistry,
    pub level: SignageLevel,
    pub region: SignRegion,
    pub intersections: IntersectionIndex,
    pub scale: f64,
    /// Vehicular road surfaces; posts stand beside these (sidewalks are fine).
    pub carriageway: RoadMaskBitmap,
    /// Placement tally for the end-of-run summary.
    report: Mutex<PlacementReport>,
}

/// Sign kind -> how many were placed, plus a few sample positions for `--debug`.
type PlacementReport = BTreeMap<&'static str, (usize, BTreeSet<(i32, i32, i32)>)>;

/// Sample positions kept per sign kind.
const REPORT_SAMPLES: usize = 4;

impl SignageContext {
    fn has(&self, key: &DecalKey) -> bool {
        self.registry.contains(key)
    }

    /// Records one placed sign of `kind` at an absolute position.
    fn note(&self, kind: &'static str, x: i32, y: i32, z: i32) {
        if let Ok(mut r) = self.report.lock() {
            let (count, samples) = r.entry(kind).or_default();
            *count += 1;
            if samples.len() < REPORT_SAMPLES {
                samples.insert((x, y, z));
            }
        }
    }

    /// One-line summary of what was placed, plus sample teleport targets when `verbose`.
    pub fn summary(&self, verbose: bool) -> String {
        let Ok(r) = self.report.lock() else {
            return String::new();
        };
        if r.is_empty() {
            return "Signage: nothing placed".to_string();
        }
        let mut out = String::from("Signage placed: ");
        out.push_str(
            &r.iter()
                .map(|(k, (count, _))| format!("{count} {k}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        if verbose {
            for (k, (_, samples)) in r.iter() {
                let tp: Vec<String> = samples
                    .iter()
                    .map(|(x, y, z)| format!("/tp {x} {y} {z}"))
                    .collect();
                out.push_str(&format!("\n  {k}: {}", tp.join("  ")));
            }
        }
        out
    }
}

/// A decal in the bundled font, or a vanilla sign for scripts it lacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameSign {
    Decal(DecalKey),
    Vanilla(String),
}

impl NameSign {
    fn key(&self) -> Option<&DecalKey> {
        match self {
            NameSign::Decal(k) => Some(k),
            NameSign::Vanilla(_) => None,
        }
    }
}

// Decision functions, shared by the pre-pass and the placement handlers.

fn clean_name(raw: &str) -> Option<String> {
    let name: String = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_NAME_CHARS)
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Plate width in tiles. Text wraps to two lines, and each tile is another map file.
fn fascia_cols(name: &str) -> u8 {
    match name.chars().count() {
        0..=26 => 2,
        27..=44 => 3,
        _ => 4,
    }
}

fn name_sign(
    tags: &HashMap<String, String>,
    style: TextStyle,
    cols: impl Fn(&str) -> u8,
) -> Option<NameSign> {
    let name = clean_name(tags.get("name")?)?;
    if font::supports(&name) {
        Some(NameSign::Decal(DecalKey::text(style, &name, cols(&name))))
    } else {
        Some(NameSign::Vanilla(name))
    }
}

/// Business name plate. Building signage, so `full` only.
pub fn poi_name_sign(tags: &HashMap<String, String>, ctx_level: SignageLevel) -> Option<NameSign> {
    if !ctx_level.full() {
        return None;
    }
    // Physical street furniture already renders as an object with its own decal.
    if let Some(a) = tags.get("amenity") {
        if matches!(
            a.as_str(),
            "recycling"
                | "waste_basket"
                | "waste_disposal"
                | "vending_machine"
                | "atm"
                | "bench"
                | "drinking_water"
                | "fountain"
                | "bicycle_parking"
                | "shelter"
                | "post_box"
                | "parking_space"
        ) {
            return None;
        }
    }
    // Information boards have their own handler, a local map or an "i" post.
    if tags.get("tourism").map(String::as_str) == Some("information") {
        return None;
    }
    pictograms::business_kind(tags)?;
    name_sign(tags, TextStyle::Fascia, fascia_cols)
}

/// House number plate key for a building's tags. Building-owned, so `full` only.
pub fn house_number_key(tags: &HashMap<String, String>, level: SignageLevel) -> Option<DecalKey> {
    if !level.full() {
        return None;
    }
    let n = tags.get("addr:housenumber")?.trim();
    if n.is_empty() || n.chars().count() > 8 || !font::supports(n) {
        return None;
    }
    Some(DecalKey::text(TextStyle::HouseNumber, n, 1))
}

/// Parses `maxspeed` into (value, mph); None for implicit/none/walk values.
fn parse_maxspeed(tags: &HashMap<String, String>, region: SignRegion) -> Option<(u16, bool)> {
    let raw = tags.get("maxspeed")?.trim().to_lowercase();
    let digits: String = raw.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let value: u16 = digits.parse().ok()?;
    if value == 0 || value > 200 {
        return None;
    }
    let mph = raw.contains("mph") || (!raw.contains("km") && region.default_mph());
    Some((value, mph))
}

/// Highway types that get speed limit signs and route shields.
fn is_signed_road(highway: &str) -> bool {
    matches!(
        highway,
        "motorway"
            | "motorway_link"
            | "trunk"
            | "trunk_link"
            | "primary"
            | "primary_link"
            | "secondary"
            | "secondary_link"
            | "tertiary"
            | "tertiary_link"
            | "unclassified"
            | "residential"
            | "living_street"
    )
}

/// Highway types whose names go on street blades.
fn is_named_street(highway: &str) -> bool {
    is_signed_road(highway) || matches!(highway, "pedestrian" | "service" | "road")
}

fn shield_style(highway: &str, reference: &str, region: SignRegion) -> ShieldStyle {
    match region {
        SignRegion::NorthAmerica => {
            if reference.starts_with('I') {
                ShieldStyle::Interstate
            } else {
                ShieldStyle::White
            }
        }
        SignRegion::UkIreland => {
            if reference.starts_with('M') {
                ShieldStyle::Blue
            } else {
                ShieldStyle::Green
            }
        }
        SignRegion::Germanic => {
            if reference.starts_with('A') || highway.starts_with("motorway") {
                ShieldStyle::Blue
            } else {
                ShieldStyle::Yellow
            }
        }
        _ => {
            if highway.starts_with("motorway")
                || reference.starts_with('A')
                || reference.starts_with('E')
            {
                ShieldStyle::Blue
            } else {
                ShieldStyle::Yellow
            }
        }
    }
}

/// Speed limit, route shield, no-entry and cycleway keys for a highway way.
pub struct WaySigns {
    pub speed: Option<DecalKey>,
    pub shield: Option<DecalKey>,
    /// No-entry sign at the exit end of a one-way street.
    pub no_entry: Option<DecalKey>,
    /// Round bicycle sign at the start of a cycleway.
    pub cycleway: Option<DecalKey>,
}

pub fn highway_way_signs(tags: &HashMap<String, String>, region: SignRegion) -> WaySigns {
    let highway = tags.get("highway").map(String::as_str).unwrap_or("");
    let mut out = WaySigns {
        speed: None,
        shield: None,
        no_entry: None,
        cycleway: None,
    };
    if tags.get("area").map(String::as_str) == Some("yes")
        || tags.get("tunnel").is_some_and(|t| t != "no")
        || tags.get("indoor").map(String::as_str) == Some("yes")
    {
        return out;
    }
    if highway == "cycleway" {
        out.cycleway = Some(DecalKey::Traffic(TrafficSign::Bicycle));
        return out;
    }
    if !is_signed_road(highway) {
        return out;
    }
    if tags.get("oneway").is_some_and(|v| v == "yes" || v == "-1") {
        out.no_entry = Some(DecalKey::Traffic(TrafficSign::NoEntry));
    }
    if let Some((value, mph)) = parse_maxspeed(tags, region) {
        out.speed = Some(DecalKey::SpeedLimit {
            value,
            mph,
            style: region.speed_style(),
        });
    }
    if matches!(
        highway,
        "motorway" | "trunk" | "primary" | "secondary" | "tertiary"
    ) {
        if let Some(reference) = tags.get("ref") {
            // First route number only; "A 9;E 45" style lists get their first entry.
            let first = reference.split(';').next().unwrap_or("").trim();
            let text: String = first.chars().take(6).collect();
            if !text.is_empty() && font::supports(&text) {
                out.shield = Some(DecalKey::RouteShield {
                    style: shield_style(highway, &text, region),
                    text,
                });
            }
        }
    }
    out
}

/// Sign for a highway node (stop, give way, crossing) or a level crossing node.
pub fn highway_node_sign(tags: &HashMap<String, String>, level: SignageLevel) -> Option<DecalKey> {
    if tags.get("railway").map(String::as_str) == Some("level_crossing") {
        return Some(DecalKey::Traffic(TrafficSign::LevelCrossing));
    }
    let highway = tags.get("highway")?.as_str();
    let sign = match highway {
        "stop" => TrafficSign::Stop,
        "give_way" => TrafficSign::GiveWay,
        "crossing" if level.full() => {
            let kind = tags.get("crossing").map(String::as_str).unwrap_or("");
            if matches!(kind, "unmarked" | "no" | "traffic_signals") {
                return None;
            }
            TrafficSign::Crossing
        }
        _ => return None,
    };
    Some(DecalKey::Traffic(sign))
}

/// High-voltage warning for substations and generator compounds.
pub fn power_sign(tags: &HashMap<String, String>) -> Option<DecalKey> {
    match tags.get("power").map(String::as_str) {
        Some("substation") | Some("plant") | Some("generator") | Some("transformer") => {
            Some(DecalKey::Traffic(TrafficSign::HighVoltage))
        }
        _ => None,
    }
}

/// Railway node signage: pictogram plus optional name plate.
pub struct RailSigns {
    pub pictogram: DecalKey,
    pub name: Option<NameSign>,
    pub station_board: bool,
}

pub fn railway_node_signs(tags: &HashMap<String, String>, region: SignRegion) -> Option<RailSigns> {
    let railway = tags.get("railway")?.as_str();
    let is_subway_station = matches!(railway, "station" | "halt")
        && (tags.get("station").map(String::as_str) == Some("subway")
            || tags.get("subway").map(String::as_str) == Some("yes"));
    let (icon, board) = match railway {
        "station" | "halt" if is_subway_station => (region.metro_logo(), false),
        "station" | "halt" => ("train", true),
        "subway_entrance" => (region.metro_logo(), false),
        "tram_stop" => ("tram", false),
        _ => return None,
    };
    let name = if board {
        name_sign(tags, TextStyle::StationBoard, |_| 3)
    } else if railway == "tram_stop" {
        name_sign(tags, TextStyle::StopName, |n| {
            if n.chars().count() <= 10 {
                1
            } else {
                2
            }
        })
    } else {
        None
    };
    Some(RailSigns {
        pictogram: DecalKey::Pictogram(icon),
        name,
        station_board: board,
    })
}

/// Bus stop name plate (one tile).
pub fn bus_stop_name(tags: &HashMap<String, String>) -> Option<NameSign> {
    name_sign(tags, TextStyle::StopName, |_| 1)
}

/// Poster keys for an advertising node.
pub fn advertising_keys(tags: &HashMap<String, String>, id: u64) -> Vec<DecalKey> {
    match tags.get("advertising").map(String::as_str) {
        Some("billboard") => vec![DecalKey::Poster((id % 6) as u8)],
        Some("column") => (0..4u64)
            .map(|f| DecalKey::ColumnPoster(((id + f) % 5) as u8))
            .collect(),
        Some("poster_box") => vec![
            DecalKey::ColumnPoster((id % 5) as u8),
            DecalKey::ColumnPoster(((id + 2) % 5) as u8),
        ],
        _ => Vec::new(),
    }
}

/// Information board key: a local map for map/board types, the "i" pictogram otherwise.
pub fn information_key(node: &ProcessedNode) -> Option<DecalKey> {
    if node.tags.get("tourism").map(String::as_str) != Some("information") {
        return None;
    }
    match node.tags.get("information").map(String::as_str) {
        Some("office") | Some("visitor_centre") => None,
        Some("map") | Some("board") | Some("terminal") => Some(DecalKey::LocalMap {
            x: node.x,
            z: node.z,
        }),
        _ => Some(DecalKey::Pictogram("information")),
    }
}

/// Memorial plaque text key.
pub fn plaque_key(tags: &HashMap<String, String>) -> Option<DecalKey> {
    if tags.get("historic").map(String::as_str) != Some("memorial")
        || tags.get("memorial").map(String::as_str) != Some("plaque")
    {
        return None;
    }
    let text = clean_name(tags.get("inscription").or_else(|| tags.get("name"))?)?;
    font::supports(&text).then(|| DecalKey::text(TextStyle::Plaque, text, 1))
}

/// Which pictogram, if any, sits on a piece of street furniture that renders as blocks.
pub fn furniture_pictogram(tags: &HashMap<String, String>) -> Option<DecalKey> {
    let icon = match tags.get("amenity").map(String::as_str) {
        Some("recycling")
            if tags.get("recycling_type").map(String::as_str) == Some("container") =>
        {
            "recycling"
        }
        Some("waste_basket") | Some("waste_disposal") => "recycling",
        Some("vending_machine") => "vending_machine",
        Some("atm") => "atm",
        _ => match tags.get("emergency").map(String::as_str) {
            Some("fire_hydrant")
                if !matches!(
                    tags.get("fire_hydrant:type").map(String::as_str),
                    Some("underground") | Some("wall") | Some("pond")
                ) =>
            {
                "hydrant"
            }
            _ => return None,
        },
    };
    Some(DecalKey::Pictogram(icon))
}

fn way_direction_at(nodes: &[ProcessedNode], i: usize) -> Option<(f64, f64)> {
    let prev = if i > 0 { &nodes[i - 1] } else { &nodes[i] };
    let next = if i + 1 < nodes.len() {
        &nodes[i + 1]
    } else {
        &nodes[i]
    };
    let dx = (next.x - prev.x) as f64;
    let dz = (next.z - prev.z) as f64;
    let len = (dx * dx + dz * dz).sqrt();
    (len > 0.0).then(|| (dx / len, dz / len))
}

/// Builds the street name index: nodes shared by ways with at least two distinct names.
pub fn build_intersection_index(elements: &[ProcessedElement], scale: f64) -> IntersectionIndex {
    struct Hit<'a> {
        way_id: u64,
        name: &'a str,
        dir: (f64, f64),
        half_width: i32,
    }
    // Named streets, resolved once. Names are borrowed so a long way does not clone its
    // name per node.
    let mut streets: Vec<(&ProcessedWay, String, i32)> = Vec::new();
    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };
        let Some(highway) = way.tags.get("highway") else {
            continue;
        };
        if !is_named_street(highway)
            || way.tags.get("area").map(String::as_str) == Some("yes")
            || way.tags.get("tunnel").is_some_and(|t| t != "no")
            || way.tags.get("indoor").map(String::as_str) == Some("yes")
        {
            continue;
        }
        let Some(name) = way.tags.get("name").and_then(|n| clean_name(n)) else {
            continue;
        };
        if !font::supports(&name) {
            continue;
        }
        let half_width = highway_block_range(highway, &way.tags, scale);
        streets.push((way, name, half_width));
    }

    // Most road nodes sit inside one way and can never be a junction of two names, so
    // count first and only collect hits for the shared ones.
    let mut shared: HashMap<u64, u8> = HashMap::new();
    for (way, _, _) in &streets {
        for node in &way.nodes {
            let seen = shared.entry(node.id).or_insert(0);
            *seen = seen.saturating_add(1);
        }
    }
    shared.retain(|_, n| *n >= 2);

    let mut by_node: HashMap<u64, Vec<Hit>> = HashMap::new();
    for (way, name, half_width) in &streets {
        for (i, node) in way.nodes.iter().enumerate() {
            if !shared.contains_key(&node.id) {
                continue;
            }
            let Some(dir) = way_direction_at(&way.nodes, i) else {
                continue;
            };
            by_node.entry(node.id).or_default().push(Hit {
                way_id: way.id,
                name: name.as_str(),
                dir,
                half_width: *half_width,
            });
        }
    }

    let mut index = IntersectionIndex::new();
    for (node_id, hits) in by_node {
        let mut names: Vec<&str> = hits.iter().map(|h| h.name).collect();
        names.sort_unstable();
        names.dedup();
        if names.len() < 2 {
            continue;
        }
        let owner_way = hits.iter().map(|h| h.way_id).min().unwrap_or(0);
        let mut blades: Vec<Blade> = Vec::new();
        for name in names.into_iter().take(3) {
            let mut dir = (0.0, 0.0);
            let mut half_width = 0;
            for h in hits.iter().filter(|h| h.name == name) {
                // Directions of a street through the node point both ways; align them.
                let d = if h.dir.0 * dir.0 + h.dir.1 * dir.1 < 0.0 {
                    (-h.dir.0, -h.dir.1)
                } else {
                    h.dir
                };
                dir = (dir.0 + d.0, dir.1 + d.1);
                half_width = half_width.max(h.half_width);
            }
            let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt();
            let dir = if len > 0.0 {
                (dir.0 / len, dir.1 / len)
            } else {
                (1.0, 0.0)
            };
            blades.push(Blade {
                name: name.to_string(),
                dir,
                half_width,
            });
        }
        index.insert(node_id, IntersectionPost { owner_way, blades });
    }
    index
}

/// Every decal key an element may place; the pre-pass unions these into the registry.
fn keys_for_element(
    element: &ProcessedElement,
    level: SignageLevel,
    region: SignRegion,
    keys: &mut BTreeSet<DecalKey>,
) {
    let tags = element.tags();
    match element {
        ProcessedElement::Node(node) => {
            if let Some(k) = furniture_pictogram(tags) {
                keys.insert(k);
            }
            if let Some(NameSign::Decal(k)) = poi_name_sign(tags, level) {
                keys.insert(k);
            }
            if let Some(k) = highway_node_sign(tags, level) {
                keys.insert(k);
            }
            if tags.get("highway").map(String::as_str) == Some("bus_stop") {
                keys.insert(DecalKey::Pictogram("bus_stop"));
                if let Some(NameSign::Decal(k)) = bus_stop_name(tags) {
                    keys.insert(k);
                }
            }
            if let Some(rail) = railway_node_signs(tags, region) {
                keys.insert(rail.pictogram);
                if let Some(NameSign::Decal(k)) = rail.name {
                    keys.insert(k);
                }
            }
            keys.extend(advertising_keys(tags, node.id));
            if let Some(k) = information_key(node) {
                keys.insert(k);
            }
            if let Some(k) = plaque_key(tags) {
                keys.insert(k);
            }
        }
        ProcessedElement::Way(way) => {
            if tags.contains_key("building") && !is_underground_building(tags) {
                if let Some(NameSign::Decal(k)) = poi_name_sign(tags, level) {
                    keys.insert(k);
                }
                if let Some(k) = house_number_key(tags, level) {
                    keys.insert(k);
                }
            }
            if tags.contains_key("highway") {
                let ws = highway_way_signs(tags, region);
                keys.extend(ws.speed);
                keys.extend(ws.shield);
                keys.extend(ws.no_entry);
                keys.extend(ws.cycleway);
            }
            if tags.get("amenity").map(String::as_str) == Some("parking") && way.nodes.len() > 2 {
                keys.insert(DecalKey::Pictogram("parking"));
            }
            if way.nodes.len() > 2 {
                keys.extend(power_sign(tags));
            }
        }
        ProcessedElement::Relation(_) => {}
    }
}

/// Tile budget for one world. Each tile is two map files on disk, so a whole city's worth
/// of shop names would bury the save directory.
const MAX_DECAL_TILES: u32 = 30_000;

/// Drops building signage, the bulk of the keys and the least missed, until the registry
/// fits the budget. Infrastructure signage is kept.
fn drop_keys_over_budget(keys: &mut BTreeSet<DecalKey>) {
    let tiles = |k: &DecalKey| {
        let (c, r) = k.dims();
        c * r
    };
    let total: u32 = keys.iter().map(tiles).sum();
    if total <= MAX_DECAL_TILES {
        return;
    }
    let is_building = |k: &DecalKey| {
        matches!(
            k,
            DecalKey::Text {
                style: TextStyle::Fascia | TextStyle::HouseNumber,
                ..
            }
        )
    };
    let dropped = keys.iter().filter(|k| is_building(k)).count();
    keys.retain(|k| !is_building(k));
    let left: u32 = keys.iter().map(tiles).sum();
    eprintln!(
        "Note: signage capped at {MAX_DECAL_TILES} map tiles ({total} needed); \
         dropped {dropped} building name plates, {left} tiles remain."
    );
}

/// Pre-pass: collects every decal the world will need and assigns map ids.
pub fn build_context(
    elements: &[ProcessedElement],
    args: &Args,
    llbbox: LLBBox,
    xzbbox: &XZBBox,
) -> Option<SignageContext> {
    let level = args.signage;
    if !level.enabled() {
        return None;
    }
    let carriageway = collect_carriageway_coords(elements, xzbbox, args.scale);
    let lat = (llbbox.min().lat() + llbbox.max().lat()) / 2.0;
    let lon = (llbbox.min().lng() + llbbox.max().lng()) / 2.0;
    let region = SignRegion::detect(lat, lon);
    let intersections = build_intersection_index(elements, args.scale);

    let mut keys: BTreeSet<DecalKey> = BTreeSet::new();
    for element in elements {
        keys_for_element(element, level, region, &mut keys);
    }
    drop_keys_over_budget(&mut keys);
    for post in intersections.values() {
        for blade in &post.blades {
            keys.insert(DecalKey::text(
                TextStyle::StreetName(region.blade_style()),
                &blade.name,
                1,
            ));
        }
    }

    Some(SignageContext {
        registry: DecalRegistry::from_keys(keys),
        level,
        region,
        intersections,
        scale: args.scale,
        carriageway,
        report: Mutex::new(PlacementReport::new()),
    })
}

// Placement helpers.

/// Item-frame facing for a unit direction (the sign face points along `d`).
fn facing_for_dir(dx: f64, dz: f64) -> i8 {
    if dx.abs() >= dz.abs() {
        if dx >= 0.0 {
            5
        } else {
            4
        }
    } else if dz >= 0.0 {
        3
    } else {
        2
    }
}

/// Opposite wall-facing.
fn opposite(facing: i8) -> i8 {
    match facing {
        2 => 3,
        3 => 2,
        4 => 5,
        5 => 4,
        f => f,
    }
}

/// Facings whose axis is perpendicular to a street direction (the pair a blade shows on).
fn blade_facings(dir: (f64, f64)) -> (i8, i8) {
    if dir.0.abs() >= dir.1.abs() {
        (2, 3)
    } else {
        (4, 5)
    }
}

/// Places a thin sign post: wall blocks from `base + 1` up to `base + height`. Only writes
/// into free cells; returns true if the top cell was placed (or already solid).
/// Shaft height shared by every sign post: two wall blocks, heads start above them.
const POST_SHAFT: i32 = 2;

/// Two walls plus a full-block head, whose Y is returned. A frame hangs on the face of its
/// cell, so a thin head would leave the sign floating.
fn traffic_sign_post(editor: &mut WorldEditor, x: i32, base: i32, z: i32) -> Option<i32> {
    if !free_for_post(editor, x, base, z, POST_SHAFT + 1) {
        return None;
    }
    for dy in 1..=POST_SHAFT {
        editor.set_block_absolute(STONE_BRICK_WALL, x, base + dy, z, None, None);
    }
    let head = base + POST_SHAFT + 1;
    editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, head, z, None, None);
    editor
        .get_block_absolute(x, head, z)
        .is_some_and(|b| b != AIR)
        .then_some(head)
}

/// Two walls plus one row per blade, slab on top. Returns the lowest blade row.
fn street_name_post(
    editor: &mut WorldEditor,
    x: i32,
    base: i32,
    z: i32,
    levels: i32,
) -> Option<i32> {
    let levels = levels.max(1);
    if !free_for_post(editor, x, base, z, POST_SHAFT + levels) {
        return None;
    }
    for dy in 1..=POST_SHAFT {
        editor.set_block_absolute(STONE_BRICK_WALL, x, base + dy, z, None, None);
    }
    let first = base + POST_SHAFT + 1;
    for level in 0..levels {
        // Blade art fills the lower half, so a slab below another row would show a gap.
        let block = if level == levels - 1 {
            POLISHED_ANDESITE_SLAB
        } else {
            POLISHED_ANDESITE
        };
        editor.set_block_absolute(block, x, first + level, z, None, None);
    }
    editor
        .get_block_absolute(x, first, z)
        .is_some_and(|b| b != AIR)
        .then_some(first)
}

/// Glass and other see-through wall blocks, which make a poor backing for a sign plate.
fn is_see_through(block: Block) -> bool {
    let name = block.name();
    name.contains("glass") || name.contains("bars") || name.contains("scaffolding")
}

/// Empty cells with no frame in them. A frame is an entity, so the cell reads as empty.
fn free_for_post(editor: &WorldEditor, x: i32, base: i32, z: i32, height: i32) -> bool {
    (1..=height).all(|dy| {
        !editor.cell_has_frame(x, base + dy, z)
            && editor
                .get_block_absolute(x, base + dy, z)
                .is_none_or(|b| b == AIR)
    })
}

/// Puts a name on a wall as decal or vanilla sign. (bx, y, bz) is the centre host block.
fn place_name_sign(
    editor: &mut WorldEditor,
    bx: i32,
    y: i32,
    bz: i32,
    facing: i8,
    name: &NameSign,
) -> bool {
    match name {
        NameSign::Decal(key) => place_wall_panel(editor, bx, y, bz, facing, key),
        NameSign::Vanilla(text) => {
            let lines = split_lines(text, 15, 4);
            let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
            editor.place_wall_sign(bx, y, bz, facing, &refs)
        }
    }
}

/// One-row panel on a facade. All columns share one depth, and a stepped wall gets its
/// gaps backed with its own block so the plate stays flat.
fn place_wall_panel(
    editor: &mut WorldEditor,
    bx: i32,
    y: i32,
    bz: i32,
    facing: i8,
    key: &DecalKey,
) -> bool {
    let Some(entry) = editor.signage().and_then(|s| s.registry.get(key)) else {
        return false;
    };
    let cols = entry.cols as i32;
    let (rx, rz) = right_dir(facing);
    // Offset from a host block to the cell its frame occupies.
    let (fx, fz) = match facing {
        2 => (0, -1),
        3 => (0, 1),
        4 => (-1, 0),
        _ => (1, 0),
    };
    let (lx, lz) = WorldEditor::panel_left_anchor(bx, bz, facing, cols);
    let host_at = |c: i32, depth: i32| {
        let (cx, cz) = (lx + rx * c, lz + rz * c);
        (cx - fx * depth, cz - fz * depth)
    };

    // One depth for the whole plate, else a diagonal wall staggers the tiles. Nearest
    // first, and a farther depth must win by more than one column to be taken.
    let mut best: Option<(i32, i32)> = None;
    for depth in [0, -1, 1, -2, 2] {
        let mut solid = 0;
        let mut usable = true;
        for c in 0..cols {
            let (hx, hz) = host_at(c, depth);
            let (px, pz) = (hx + fx, hz + fz);
            // The frame's own cell must be free and above the terrain, or it is culled.
            if !editor.cell_open_at(px, y, pz) || y - editor.get_absolute_y(px, 0, pz) < 1 {
                usable = false;
                break;
            }
            if !editor.cell_open_at(hx, y, hz) {
                solid += 1;
            }
        }
        if !usable {
            continue;
        }
        if solid == cols {
            best = Some((depth, solid));
            break;
        }
        if best.is_none_or(|(_, s)| solid > s + 1) {
            best = Some((depth, solid));
        }
    }
    let Some((depth, solid)) = best else {
        return false;
    };
    // Back the holes a stepped wall leaves, but only if most of the row is real wall.
    if solid * 2 < cols {
        return false;
    }
    if solid < cols {
        // Opaque only, or the plate ends up backed by a window.
        let Some(filler) = (0..cols).find_map(|c| {
            let (hx, hz) = host_at(c, depth);
            editor
                .get_block_absolute(hx, y, hz)
                .filter(|b| *b != AIR && !is_see_through(*b))
        }) else {
            return false;
        };
        // Holes must touch wall along the row. Checking underneath instead would reject
        // diagonal walls, which are notched at every height.
        let attached = (0..cols).all(|c| {
            let (hx, hz) = host_at(c, depth);
            if !editor.cell_open_at(hx, y, hz) {
                return true;
            }
            [c - 1, c + 1].into_iter().any(|n| {
                (0..cols).contains(&n) && {
                    let (nx, nz) = host_at(n, depth);
                    !editor.cell_open_at(nx, y, nz)
                }
            })
        });
        if !attached {
            return false;
        }
        for c in 0..cols {
            let (hx, hz) = host_at(c, depth);
            editor.set_block_absolute(filler, hx, y, hz, None, None);
        }
    }

    for c in 0..cols {
        let (hx, hz) = host_at(c, depth);
        editor.place_map_decal_ex(
            hx,
            y,
            hz,
            facing,
            entry.tile_id(c as u32, 0),
            0,
            false,
            true,
        );
    }
    true
}

/// Splits text into at most `max_lines` lines of about `max_len` characters at spaces.
fn split_lines(text: &str, max_len: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > max_len {
            lines.push(std::mem::take(&mut cur));
            if lines.len() == max_lines {
                return lines;
            }
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
        while cur.chars().count() > max_len {
            let head: String = cur.chars().take(max_len).collect();
            cur = cur.chars().skip(max_len).collect();
            lines.push(head);
            if lines.len() == max_lines {
                return lines;
            }
        }
    }
    if !cur.is_empty() && lines.len() < max_lines {
        lines.push(cur);
    }
    lines
}

/// Name plate on the anchor block, retried one row up when the cell in front is taken.
fn place_facade_sign(
    editor: &mut WorldEditor,
    ax: i32,
    y: i32,
    az: i32,
    facing: i8,
    name: &NameSign,
) -> bool {
    (0..2).any(|dy| place_name_sign(editor, ax, y + dy, az, facing, name))
}

/// Viewer's right for a wall facing.
fn right_dir(facing: i8) -> (i32, i32) {
    match facing {
        2 => (-1, 0),
        3 => (1, 0),
        4 => (0, 1),
        5 => (0, -1),
        _ => (1, 0),
    }
}

/// Facade signs and house number for a building way. Runs after the walls are built.
pub fn generate_building_signage(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    anchor: Option<FacadeAnchor>,
) {
    let Some(ctx) = editor.signage() else {
        return;
    };
    // No anchor means it did not render as a walled building with a street front.
    let Some(anchor) = anchor else {
        return;
    };
    if way.tags.contains_key("building:part") {
        return;
    }
    let name = poi_name_sign(&way.tags, ctx.level).filter(|n| n.key().is_none_or(|k| ctx.has(k)));
    let number = house_number_key(&way.tags, ctx.level).filter(|k| ctx.has(k));
    if name.is_none() && number.is_none() {
        return;
    }
    // Ways are processed by every tile they overlap; only the anchor's owner places.
    if !editor.owns(anchor.x, anchor.z) {
        return;
    }
    let facing = WorldEditor::facing_for_normal(anchor.normal.0, anchor.normal.1);
    let (rx, rz) = right_dir(facing);

    if let Some(name) = &name {
        if place_facade_sign(editor, anchor.x, anchor.fascia_y, anchor.z, facing, name) {
            ctx.note("shop name plates", anchor.x, anchor.fascia_y, anchor.z);
        }
    }
    if let Some(key) = &number {
        // Beside the door at door height, never on it; the plate is one tile wide.
        let (hx, hz) = match anchor.door {
            Some((dx, dz)) => (dx + rx, dz + rz),
            None => (anchor.x - rx * 2, anchor.z - rz * 2),
        };
        for (x, z) in [(hx, hz), (hx - rx * 2, hz - rz * 2)] {
            if editor.place_decal_panel(x, anchor.number_y, z, facing, key, false, true) {
                ctx.note("house numbers", x, anchor.number_y, z);
                break;
            }
        }
    }
}

/// Walks out from a POI node to the first exterior wall, returning its host and facing.
fn nearest_wall_from_inside(
    editor: &WorldEditor,
    x: i32,
    z: i32,
    sign_y: i32,
    footprints: &BuildingFootprintBitmap,
    road_mask: &RoadMaskBitmap,
) -> Option<(i32, i32, i8)> {
    let mut best: Option<(i32, (i32, i32, i8))> = None;
    for (dx, dz) in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
        let mut left_footprint = false;
        for k in 1..=24 {
            let (cx, cz) = (x + dx * k, z + dz * k);
            if !left_footprint {
                if !footprints.contains(cx, cz) {
                    left_footprint = true;
                } else {
                    continue;
                }
            }
            // First open cell outside: the sign hangs here, on the block just inside.
            if editor.cell_open_at(cx, sign_y, cz) {
                let (hx, hz) = (cx - dx, cz - dz);
                if editor.cell_open_at(hx, sign_y, hz) {
                    break; // No wall to hang on along this axis.
                }
                let mut score = k;
                if !(1..=8).any(|j| road_mask.contains(cx + dx * j, cz + dz * j)) {
                    score += 6;
                }
                let facing = WorldEditor::facing_for_normal(dx, dz);
                if best.as_ref().is_none_or(|(s, _)| score < *s) {
                    best = Some((score, (hx, hz, facing)));
                }
                break;
            }
        }
    }
    best.map(|(_, hit)| hit)
}

/// Node dispatcher: POI facade signs, stop/give-way posts and railway signage.
pub fn generate_node_signage(
    editor: &mut WorldEditor,
    node: &ProcessedNode,
    footprints: &BuildingFootprintBitmap,
    road_mask: &RoadMaskBitmap,
) {
    // Nodes normally land in one tile, but some kinds are assigned to every tile they
    // reach, and a sign must not be placed twice.
    if !editor.signage_enabled() || !editor.owns(node.x, node.z) {
        return;
    }
    let tags = &node.tags;
    if tags.contains_key("shop")
        || tags.contains_key("amenity")
        || tags.contains_key("office")
        || tags.contains_key("tourism")
        || tags.contains_key("leisure")
        || tags.contains_key("healthcare")
        || tags.contains_key("craft")
    {
        generate_poi_signage(editor, node, footprints, road_mask);
    }
    if tags.contains_key("highway")
        || tags.get("railway").map(String::as_str) == Some("level_crossing")
    {
        generate_highway_node_signage(editor, node, road_mask);
    }
    if tags.contains_key("railway") {
        generate_railway_node_signage(editor, node, road_mask);
    }
}

/// High-voltage warning post at the first outline node of a substation or plant.
pub fn generate_power_signage(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    road_mask: &RoadMaskBitmap,
) {
    let Some(ctx) = editor.signage() else {
        return;
    };
    let Some(key) = power_sign(&way.tags).filter(|k| ctx.has(k)) else {
        return;
    };
    let Some(node) = way.nodes.first() else {
        return;
    };
    let (x, z) = (node.x, node.z);
    if ctx.carriageway.contains(x, z) || !editor.owns(x, z) {
        return;
    }
    let ground = editor.get_absolute_y(x, 0, z);
    let Some(head) = traffic_sign_post(editor, x, ground, z) else {
        return;
    };
    let facing = match get_nearest_road_block(x, z, 12, road_mask) {
        Some((rx, rz)) => facing_for_dir((rx - x) as f64, (rz - z) as f64),
        None => 3,
    };
    if editor.place_decal(x, head, z, facing, &key) {
        ctx.note("high-voltage signs", x, head, z);
    }
    editor.place_decal(x, head, z, opposite(facing), &key);
}

/// Name plate for a POI node that sits inside a building: hung on the nearest exterior wall.
pub fn generate_poi_signage(
    editor: &mut WorldEditor,
    node: &ProcessedNode,
    footprints: &BuildingFootprintBitmap,
    road_mask: &RoadMaskBitmap,
) {
    let Some(ctx) = editor.signage() else {
        return;
    };
    let Some(name) =
        poi_name_sign(&node.tags, ctx.level).filter(|n| n.key().is_none_or(|k| ctx.has(k)))
    else {
        return;
    };
    let (x, z) = (node.x, node.z);
    if !footprints.contains(x, z) {
        return;
    }
    let ground = editor.get_absolute_y(x, 0, z);
    if let Some((hx, hz, facing)) =
        nearest_wall_from_inside(editor, x, z, ground + 3, footprints, road_mask)
    {
        // Terrain at the wall, not at the node, so a sloped site does not skew the row.
        let row = editor.get_absolute_y(hx, 0, hz) + 3;
        if place_facade_sign(editor, hx, row, hz, facing, &name) {
            ctx.note("shop name plates", hx, row, hz);
        }
    }
}

/// Parking lot: a "P" post at the first outline node, off the road.
pub fn generate_parking_signage(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    road_mask: &RoadMaskBitmap,
) {
    let Some(ctx) = editor.signage() else {
        return;
    };
    let key = DecalKey::Pictogram("parking");
    if !ctx.has(&key) || way.nodes.len() < 3 {
        return;
    }
    let Some(node) = way.nodes.first() else {
        return;
    };
    let (x, z) = (node.x, node.z);
    if ctx.carriageway.contains(x, z) || !editor.owns(x, z) {
        return;
    }
    let ground = editor.get_absolute_y(x, 0, z);
    let Some(head) = traffic_sign_post(editor, x, ground, z) else {
        return;
    };
    let facing = match get_nearest_road_block(x, z, 10, road_mask) {
        Some((rx, rz)) => facing_for_dir((rx - x) as f64, (rz - z) as f64),
        None => 3,
    };
    if editor.place_decal(x, head, z, facing, &key) {
        ctx.note("parking signs", x, head, z);
    }
    editor.place_decal(x, head, z, opposite(facing), &key);
}

/// Speed limit signs, route shields and street name posts along a highway way.
pub fn generate_highway_way_signage(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    footprints: &BuildingFootprintBitmap,
) {
    let Some(ctx) = editor.signage() else {
        return;
    };
    let highway = way.tags.get("highway").map(String::as_str).unwrap_or("");
    if way.nodes.len() < 2 || way.tags.get("area").map(String::as_str) == Some("yes") {
        return;
    }
    // Posts stand on the terrain, so elevated decks and tunnels get nothing.
    if way.tags.get("bridge").is_some_and(|b| b != "no")
        || way.tags.get("tunnel").is_some_and(|t| t != "no")
        || way
            .tags
            .get("layer")
            .and_then(|l| l.parse::<i32>().ok())
            .is_some_and(|l| l != 0)
    {
        return;
    }
    let half_width = highway_block_range(highway, &way.tags, ctx.scale);

    // Street name posts at intersections this way owns.
    for (i, node) in way.nodes.iter().enumerate() {
        let Some(post) = ctx.intersections.get(&node.id) else {
            continue;
        };
        if post.owner_way != way.id || !editor.owns(node.x, node.z) {
            continue;
        }
        let Some(dir) = way_direction_at(&way.nodes, i) else {
            continue;
        };
        place_street_name_post(editor, &ctx, node, dir, post, footprints);
    }

    let signs = highway_way_signs(&way.tags, ctx.region);
    if signs.speed.is_none()
        && signs.shield.is_none()
        && signs.no_entry.is_none()
        && signs.cycleway.is_none()
    {
        return;
    }
    let cells: Vec<(i32, i32)> = way
        .nodes
        .windows(2)
        .flat_map(|w| {
            bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z)
                .into_iter()
                .map(|(x, _, z)| (x, z))
        })
        .collect();
    let len = cells.len();
    let oneway = way
        .tags
        .get("oneway")
        .is_some_and(|v| v == "yes" || v == "-1");
    let reversed = way.tags.get("oneway").is_some_and(|v| v == "-1");
    if len < 24 {
        return;
    }
    if let Some(key) = signs.cycleway.as_ref().filter(|k| ctx.has(k)) {
        place_roadside_sign(
            editor,
            &ctx,
            "cycleway signs",
            &cells,
            6,
            half_width,
            key,
            false,
            footprints,
        );
        return;
    }
    // One in from the start, and one at the far end for the other direction.
    if let Some(key) = signs.speed.as_ref().filter(|k| ctx.has(k)) {
        place_roadside_sign(
            editor,
            &ctx,
            "speed limits",
            &cells,
            8,
            half_width,
            key,
            reversed,
            footprints,
        );
        if !oneway && len >= 60 {
            place_roadside_sign(
                editor,
                &ctx,
                "speed limits",
                &cells,
                len - 9,
                half_width,
                key,
                true,
                footprints,
            );
        }
    }
    // No entry where wrong-way traffic would enter a one-way street.
    if let Some(key) = signs.no_entry.as_ref().filter(|k| ctx.has(k)) {
        if reversed {
            place_roadside_sign(
                editor,
                &ctx,
                "no-entry signs",
                &cells,
                4,
                half_width,
                key,
                false,
                footprints,
            );
        } else {
            place_roadside_sign(
                editor,
                &ctx,
                "no-entry signs",
                &cells,
                len - 5,
                half_width,
                key,
                true,
                footprints,
            );
        }
    }
    // Reassurance shields: thinned so a road split into many OSM ways is not carpeted.
    if let Some(key) = signs.shield.as_ref().filter(|k| ctx.has(k)) {
        if len >= 120 || way.id.is_multiple_of(3) {
            place_roadside_sign(
                editor,
                &ctx,
                "route shields",
                &cells,
                (len / 2).min(40),
                half_width,
                key,
                false,
                footprints,
            );
        }
        let mut pos = 250;
        while pos + 30 < len {
            place_roadside_sign(
                editor,
                &ctx,
                "route shields",
                &cells,
                pos,
                half_width,
                key,
                false,
                footprints,
            );
            pos += 250;
        }
    }
}

/// One-tile sign on a post at the kerb, facing oncoming traffic.
#[allow(clippy::too_many_arguments)]
fn place_roadside_sign(
    editor: &mut WorldEditor,
    ctx: &SignageContext,
    kind: &'static str,
    cells: &[(i32, i32)],
    idx: usize,
    half_width: i32,
    key: &DecalKey,
    reverse: bool,
    footprints: &BuildingFootprintBitmap,
) {
    if cells.len() < 3 {
        return;
    }
    let idx = idx.clamp(1, cells.len() - 2);
    let (x, z) = cells[idx];
    if !editor.owns(x, z) {
        return;
    }
    let (bx, bz) = cells[idx - 1];
    let (fx, fz) = cells[idx + 1];
    let (mut dx, mut dz) = ((fx - bx) as f64, (fz - bz) as f64);
    let len = (dx * dx + dz * dz).sqrt();
    if len == 0.0 {
        return;
    }
    dx /= len;
    dz /= len;
    if reverse {
        dx = -dx;
        dz = -dz;
    }
    // Kerb side: clockwise for right-hand traffic, the other way for left-hand.
    let (px, pz) = if ctx.region.drives_on_left() {
        (dz, -dx)
    } else {
        (-dz, dx)
    };
    // Kerb side of the direction of travel: first free cell past the carriageway edge.
    let Some((sx, sz, ground)) = (0..5)
        .map(|k| {
            let offset = (half_width + 2 + k) as f64;
            (
                x + (px * offset).round() as i32,
                z + (pz * offset).round() as i32,
            )
        })
        .find_map(|(sx, sz)| {
            if ctx.carriageway.contains(sx, sz)
                || footprints.contains(sx, sz)
                || editor.is_lc_water(sx, sz)
            {
                return None;
            }
            let ground = editor.get_absolute_y(sx, 0, sz);
            free_for_post(editor, sx, ground, sz, POST_SHAFT + 1).then_some((sx, sz, ground))
        })
    else {
        return;
    };
    let Some(head) = traffic_sign_post(editor, sx, ground, sz) else {
        return;
    };
    // Face against the direction of travel so approaching drivers read it.
    let facing = facing_for_dir(-dx, -dz);
    if editor.place_decal(sx, head, sz, facing, key) {
        ctx.note(kind, sx, head, sz);
    }
}

/// Street name blades on a post at a corner of the intersection.
fn place_street_name_post(
    editor: &mut WorldEditor,
    ctx: &SignageContext,
    node: &ProcessedNode,
    own_dir: (f64, f64),
    post: &IntersectionPost,
    footprints: &BuildingFootprintBitmap,
) {
    let style = ctx.region.blade_style();
    let widest = post.blades.iter().map(|b| b.half_width).max().unwrap_or(1);
    // Kerb distance per axis: streets running north-south take up x, east-west ones z.
    let width_along = |axis_x: bool| {
        post.blades
            .iter()
            .filter(|b| (b.dir.1.abs() > b.dir.0.abs()) == axis_x)
            .map(|b| b.half_width)
            .max()
            .unwrap_or(widest)
    };
    let (wx, wz) = (width_along(true), width_along(false));
    // Quadrant away from both streets, so the post lands on the sidewalk corner.
    let cross = post
        .blades
        .iter()
        .find(|b| (b.dir.0 * own_dir.0 + b.dir.1 * own_dir.1).abs() < 0.9)
        .map(|b| b.dir)
        .unwrap_or((-own_dir.1, own_dir.0));
    let qx = if -(own_dir.0 + cross.0) >= 0.0 { 1 } else { -1 };
    let qz = if -(own_dir.1 + cross.1) >= 0.0 { 1 } else { -1 };
    // Preferred corner first: the nearest cell off the road and outside buildings wins.
    let mut offsets: Vec<(i32, i32)> = (1..=9).flat_map(|d| (1..=9).map(move |e| (d, e))).collect();
    offsets.sort_by_key(|(d, e)| d * d + e * e);
    let Some((px, pz, ground)) = [(qx, qz), (qx, -qz), (-qx, qz), (-qx, -qz)]
        .into_iter()
        .flat_map(|(sx, sz)| {
            offsets
                .iter()
                .map(move |&(d, e)| (sx * (wx + d), sz * (wz + e)))
        })
        .map(|(dx, dz)| (node.x + dx, node.z + dz))
        .find_map(|(px, pz)| {
            if ctx.carriageway.contains(px, pz)
                || footprints.contains(px, pz)
                || editor.is_lc_water(px, pz)
            {
                return None;
            }
            let ground = editor.get_absolute_y(px, 0, pz);
            free_for_post(editor, px, ground, pz, POST_SHAFT + 2).then_some((px, pz, ground))
        })
    else {
        return;
    };
    // Lay blades out first to fix the post height. Two streets share one slab.
    let mut rows: Vec<Vec<(&Blade, i8, i8)>> = Vec::new();
    for blade in &post.blades {
        if !ctx.has(&DecalKey::text(
            TextStyle::StreetName(style),
            &blade.name,
            1,
        )) {
            continue;
        }
        let (fa, fb) = blade_facings(blade.dir);
        match rows.iter_mut().find(|r| r.iter().all(|(_, a, _)| *a != fa)) {
            Some(row) => row.push((blade, fa, fb)),
            None => rows.push(vec![(blade, fa, fb)]),
        }
    }
    if rows.is_empty() {
        return;
    }
    let Some(first) = street_name_post(editor, px, ground, pz, rows.len() as i32) else {
        return;
    };
    for (level, row) in rows.iter().enumerate() {
        let y = first + level as i32;
        for (blade, fa, fb) in row {
            let key = DecalKey::text(TextStyle::StreetName(style), &blade.name, 1);
            editor.place_decal(px, y, pz, *fa, &key);
            editor.place_decal(px, y, pz, *fb, &key);
        }
    }
    ctx.note("street name posts", px, first, pz);
}

/// Stop, give-way and crossing signs on a post beside a highway node.
pub fn generate_highway_node_signage(
    editor: &mut WorldEditor,
    node: &ProcessedNode,
    road_mask: &RoadMaskBitmap,
) {
    let Some(ctx) = editor.signage() else {
        return;
    };
    let Some(key) = highway_node_sign(&node.tags, ctx.level).filter(|k| ctx.has(k)) else {
        return;
    };
    let (x, z) = (node.x, node.z);
    // The node sits on the carriageway; the post goes to the nearest kerb.
    let (px, pz) = if ctx.carriageway.contains(x, z) {
        match get_nearest_non_road_block(x, z, 10, &ctx.carriageway) {
            Some(p) => p,
            None => return,
        }
    } else {
        (x, z)
    };
    let ground = editor.get_absolute_y(px, 0, pz);
    if editor.is_lc_water(px, pz) {
        return;
    }
    let Some(head) = traffic_sign_post(editor, px, ground, pz) else {
        return;
    };
    let facing = if (px, pz) != (x, z) {
        facing_for_dir((x - px) as f64, (z - pz) as f64)
    } else {
        match get_nearest_road_block(x, z, 8, road_mask) {
            Some((rx, rz)) => facing_for_dir((rx - x) as f64, (rz - z) as f64),
            None => 3,
        }
    };
    if editor.place_decal(px, head, pz, facing, &key) {
        ctx.note("stop/give-way/crossing signs", px, head, pz);
    }
}

/// Station name gantry, metro entrance sign or tram stop post for a railway node.
pub fn generate_railway_node_signage(
    editor: &mut WorldEditor,
    node: &ProcessedNode,
    road_mask: &RoadMaskBitmap,
) {
    let Some(ctx) = editor.signage() else {
        return;
    };
    let Some(signs) = railway_node_signs(&node.tags, ctx.region).filter(|s| ctx.has(&s.pictogram))
    else {
        return;
    };
    let (x, z) = (node.x, node.z);
    let ground = editor.get_absolute_y(x, 0, z);
    if signs.station_board {
        // Two wall posts and a three-block beam; the board hangs on both broad faces.
        let beam = ground + POST_SHAFT + 1;
        if !free_for_post(editor, x - 1, ground, z, POST_SHAFT + 1)
            || !free_for_post(editor, x + 1, ground, z, POST_SHAFT + 1)
            || !free_for_post(editor, x, ground + POST_SHAFT, z, 1)
        {
            return;
        }
        for dx in [-1, 1] {
            for dy in 1..=POST_SHAFT {
                editor.set_block_absolute(STONE_BRICK_WALL, x + dx, ground + dy, z, None, None);
            }
        }
        for dx in -1..=1 {
            editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x + dx, beam, z, None, None);
        }
        let mut placed = false;
        if let Some(name) = &signs.name {
            for facing in [2i8, 3] {
                placed |= place_name_sign(editor, x, beam, z, facing, name);
            }
        }
        ctx.note("station boards", x, beam, z);
        if !placed {
            for facing in [2i8, 3, 4, 5] {
                editor.place_decal(x, beam, z, facing, &signs.pictogram);
            }
        } else {
            editor.place_decal(x - 1, beam, z, 4, &signs.pictogram);
            editor.place_decal(x + 1, beam, z, 5, &signs.pictogram);
        }
        return;
    }
    // Metro entrance / tram stop: a lit logo on all four faces of a post.
    if ctx.carriageway.contains(x, z) {
        return;
    }
    let Some(head) = traffic_sign_post(editor, x, ground, z) else {
        return;
    };
    let glow = node.tags.get("railway").map(String::as_str) == Some("subway_entrance");
    for facing in [2i8, 3, 4, 5] {
        editor.place_decal_panel(x, head, z, facing, &signs.pictogram, glow, false);
    }
    ctx.note(
        if glow {
            "metro entrance signs"
        } else {
            "tram stop signs"
        },
        x,
        head,
        z,
    );
    if let Some(name) = &signs.name {
        let facing = match get_nearest_road_block(x, z, 8, road_mask) {
            Some((rx, rz)) => facing_for_dir((rx - x) as f64, (rz - z) as f64),
            None => 3,
        };
        if let NameSign::Decal(k) = name {
            if k.dims().0 == 1 {
                editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, head + 1, z, None, None);
                editor.place_decal(x, head + 1, z, facing, k);
                editor.place_decal(x, head + 1, z, opposite(facing), k);
            }
        }
    }
}

/// Bus stop name plate on the pole block next to the BUS icon.
pub fn place_bus_stop_signs(
    editor: &mut WorldEditor,
    tags: &HashMap<String, String>,
    x: i32,
    sign_y: i32,
    z: i32,
) {
    let Some(ctx) = editor.signage() else {
        return;
    };
    let icon = DecalKey::Pictogram("bus_stop");
    if ctx.has(&icon) {
        editor.place_decal(x + 1, sign_y, z, 2, &icon);
        editor.place_decal(x + 1, sign_y, z, 3, &icon);
        ctx.note("bus stops", x + 1, sign_y, z);
    }
    if let Some(name) = bus_stop_name(tags) {
        for facing in [2i8, 3] {
            place_name_sign(editor, x, sign_y, z, facing, &name);
        }
    }
}

/// Billboard: two posts carrying a 3x2 panel with a poster on both faces.
pub fn generate_billboard(
    editor: &mut WorldEditor,
    node: &ProcessedNode,
    road_mask: &RoadMaskBitmap,
) -> bool {
    let Some(ctx) = editor.signage() else {
        return false;
    };
    let key = DecalKey::Poster((node.id % 6) as u8);
    if !ctx.has(&key) {
        return false;
    }
    let (x, z) = (node.x, node.z);
    // Face the nearest road; the panel then runs perpendicular to that direction.
    let facing = match get_nearest_road_block(x, z, 20, road_mask) {
        Some((rx, rz)) => facing_for_dir((rx - x) as f64, (rz - z) as f64),
        None => 3,
    };
    let (rx, rz) = right_dir(facing);
    let ground = editor.get_absolute_y(x, 0, z);
    let height: i32 = node
        .tags
        .get("height")
        .and_then(|h| h.trim_end_matches('m').trim().parse::<f64>().ok())
        .map(|h| h.round() as i32)
        .unwrap_or(6)
        .clamp(4, 12);
    let top = ground + height;
    for k in [-1, 1] {
        for dy in 1..=(height - 2) {
            editor.set_block_absolute(
                STONE_BRICK_WALL,
                x + rx * k,
                ground + dy,
                z + rz * k,
                None,
                None,
            );
        }
    }
    for k in -1..=1 {
        for y in (top - 1)..=top {
            editor.set_block_absolute(BLACK_CONCRETE, x + rx * k, y, z + rz * k, None, Some(&[]));
        }
    }
    let (lx, lz) = WorldEditor::panel_left_anchor(x, z, facing, 3);
    if editor.place_decal_panel(lx, top, lz, facing, &key, false, false) {
        ctx.note("billboards", x, top, z);
    }
    let back = opposite(facing);
    let (bx, bz) = WorldEditor::panel_left_anchor(x, z, back, 3);
    editor.place_decal_panel(bx, top, bz, back, &key, false, false);
    true
}

/// Advertising column: three-block pillar with a different poster on each face.
pub fn generate_column(editor: &mut WorldEditor, node: &ProcessedNode) -> bool {
    let Some(ctx) = editor.signage() else {
        return false;
    };
    let (x, z) = (node.x, node.z);
    let ground = editor.get_absolute_y(x, 0, z);
    for dy in 1..=3 {
        editor.set_block_absolute(GRAY_CONCRETE, x, ground + dy, z, None, None);
    }
    editor.set_block_absolute(STONE_BRICK_SLAB, x, ground + 4, z, None, None);
    let mut any = false;
    for (f, facing) in [2i8, 3, 4, 5].into_iter().enumerate() {
        let key = DecalKey::ColumnPoster(((node.id + f as u64) % 5) as u8);
        if ctx.has(&key) {
            any |= editor.place_decal_panel(x, ground + 3, z, facing, &key, false, false);
        }
    }
    if any {
        ctx.note("advertising columns", x, ground + 3, z);
    }
    any
}

/// Poster box: two side-by-side portrait posters on both broad faces of the lightbox.
pub fn place_poster_box_posters(editor: &mut WorldEditor, node: &ProcessedNode) {
    let Some(ctx) = editor.signage() else {
        return;
    };
    let (x, z) = (node.x, node.z);
    let ground = editor.get_absolute_y(x, 0, z);
    let a = DecalKey::ColumnPoster((node.id % 5) as u8);
    let b = DecalKey::ColumnPoster(((node.id + 2) % 5) as u8);
    if !ctx.has(&a) || !ctx.has(&b) {
        return;
    }
    // North face: viewer's left is +x.
    editor.place_decal_panel(x + 1, ground + 3, z, 2, &a, true, false);
    editor.place_decal_panel(x, ground + 3, z, 2, &b, true, false);
    editor.place_decal_panel(x, ground + 3, z, 3, &a, true, false);
    editor.place_decal_panel(x + 1, ground + 3, z, 3, &b, true, false);
}

/// A 2x2 local map on a stand, or the "i" post. False leaves the banner fallback.
pub fn generate_information_board(
    editor: &mut WorldEditor,
    node: &ProcessedNode,
    road_mask: &RoadMaskBitmap,
) -> bool {
    let Some(ctx) = editor.signage() else {
        return false;
    };
    let Some(key) = information_key(node).filter(|k| ctx.has(k)) else {
        return false;
    };
    let (x, z) = (node.x, node.z);
    let ground = editor.get_absolute_y(x, 0, z);
    let facing = match get_nearest_road_block(x, z, 10, road_mask) {
        Some((rx, rz)) => facing_for_dir((rx - x) as f64, (rz - z) as f64),
        None => 3,
    };
    match key {
        DecalKey::LocalMap { .. } => {
            let (rx, rz) = right_dir(facing);
            // Board back: 2 wide x 2 tall dark oak panel on two short legs.
            let (lx, lz) = WorldEditor::panel_left_anchor(x, z, facing, 2);
            for c in 0..2 {
                let (bx, bz) = (lx + rx * c, lz + rz * c);
                editor.set_block_absolute(OAK_FENCE, bx, ground + 1, bz, None, None);
                for y in (ground + 2)..=(ground + 3) {
                    editor.set_block_absolute(DARK_OAK_PLANKS, bx, y, bz, None, Some(&[]));
                }
            }
            let ok = editor.place_decal_panel(lx, ground + 3, lz, facing, &key, false, false);
            if ok {
                ctx.note("you-are-here boards", x, ground + 3, z);
            }
            ok
        }
        _ => {
            let Some(head) = traffic_sign_post(editor, x, ground, z) else {
                return false;
            };
            let mut any = false;
            for f in [2i8, 3, 4, 5] {
                any |= editor.place_decal(x, head, z, f, &key);
            }
            if any {
                ctx.note("information posts", x, head, z);
            }
            any
        }
    }
}

/// Memorial plaque on the wall next to the node.
pub fn generate_plaque(editor: &mut WorldEditor, node: &ProcessedNode) -> bool {
    let Some(ctx) = editor.signage() else {
        return false;
    };
    let Some(key) = plaque_key(&node.tags).filter(|k| ctx.has(k)) else {
        return false;
    };
    let (x, z) = (node.x, node.z);
    let y = editor.get_absolute_y(x, 0, z) + 2;
    // Find a solid neighbour to hang on; the plaque faces away from it.
    for (dx, dz) in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
        if !editor.cell_open_at(x + dx, y, z + dz) && editor.cell_open_at(x, y, z) {
            let facing = WorldEditor::facing_for_normal(-dx, -dz);
            let ok = editor.place_decal(x + dx, y, z + dz, facing, &key);
            if ok {
                ctx.note("memorial plaques", x, y, z);
            }
            return ok;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::decals::registry::SpeedStyle;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn building_signage_is_full_only() {
        let t = tags(&[("shop", "bakery"), ("name", "Bäckerei  Müller")]);
        assert!(poi_name_sign(&t, SignageLevel::Basic).is_none());
        assert_eq!(
            poi_name_sign(&t, SignageLevel::Full),
            Some(NameSign::Decal(DecalKey::text(
                TextStyle::Fascia,
                "Bäckerei Müller",
                2
            )))
        );
        let num = tags(&[("building", "yes"), ("addr:housenumber", "12a")]);
        assert!(house_number_key(&num, SignageLevel::Basic).is_none());
        assert!(house_number_key(&num, SignageLevel::Full).is_some());
        // No name, no plate; street furniture never gets one either.
        assert!(poi_name_sign(&tags(&[("shop", "bakery")]), SignageLevel::Full).is_none());
        let bench = tags(&[("amenity", "bench"), ("name", "Bank")]);
        assert!(poi_name_sign(&bench, SignageLevel::Full).is_none());
        // Unsupported script falls back to a vanilla sign.
        let j = tags(&[("amenity", "cafe"), ("name", "喫茶店")]);
        assert_eq!(
            poi_name_sign(&j, SignageLevel::Full),
            Some(NameSign::Vanilla("喫茶店".into()))
        );
    }

    #[test]
    fn maxspeed_parsing() {
        let eu = SignRegion::Germanic;
        assert_eq!(
            parse_maxspeed(&tags(&[("maxspeed", "50")]), eu),
            Some((50, false))
        );
        assert_eq!(
            parse_maxspeed(&tags(&[("maxspeed", "30 mph")]), eu),
            Some((30, true))
        );
        assert_eq!(parse_maxspeed(&tags(&[("maxspeed", "none")]), eu), None);
        assert_eq!(parse_maxspeed(&tags(&[("maxspeed", "DE:urban")]), eu), None);
        assert_eq!(
            parse_maxspeed(&tags(&[("maxspeed", "25")]), SignRegion::NorthAmerica),
            Some((25, true))
        );
    }

    #[test]
    fn way_signs_respect_road_class() {
        let t = tags(&[
            ("highway", "primary"),
            ("maxspeed", "50"),
            ("ref", "B 2;B 96"),
        ]);
        let s = highway_way_signs(&t, SignRegion::Germanic);
        assert!(matches!(
            s.speed,
            Some(DecalKey::SpeedLimit {
                value: 50,
                mph: false,
                style: SpeedStyle::Disc
            })
        ));
        assert_eq!(
            s.shield,
            Some(DecalKey::RouteShield {
                style: ShieldStyle::Yellow,
                text: "B 2".into()
            })
        );
        let f = tags(&[("highway", "footway"), ("maxspeed", "50")]);
        assert!(highway_way_signs(&f, SignRegion::Europe).speed.is_none());
        let us = tags(&[("highway", "motorway"), ("ref", "I 95"), ("maxspeed", "65")]);
        let s = highway_way_signs(&us, SignRegion::NorthAmerica);
        assert!(matches!(
            s.speed,
            Some(DecalKey::SpeedLimit {
                value: 65,
                mph: true,
                style: SpeedStyle::UsPlate
            })
        ));
        assert!(matches!(
            s.shield,
            Some(DecalKey::RouteShield {
                style: ShieldStyle::Interstate,
                ..
            })
        ));
    }

    #[test]
    fn intersection_index_needs_two_names() {
        let node = |id: u64, x: i32, z: i32| ProcessedNode {
            id,
            tags: HashMap::new(),
            x,
            z,
        };
        let way = |id: u64, name: &str, nodes: Vec<ProcessedNode>| {
            ProcessedElement::Way(ProcessedWay {
                id,
                nodes,
                tags: tags(&[("highway", "residential"), ("name", name)]),
            })
        };
        let elements = vec![
            way(
                10,
                "Main Street",
                vec![node(1, 0, 0), node(2, 20, 0), node(3, 40, 0)],
            ),
            way(
                11,
                "Cross Road",
                vec![node(4, 20, -20), node(2, 20, 0), node(5, 20, 20)],
            ),
            way(12, "Main Street", vec![node(3, 40, 0), node(6, 60, 0)]),
        ];
        let idx = build_intersection_index(&elements, 1.0);
        assert_eq!(idx.len(), 1);
        let post = &idx[&2];
        assert_eq!(post.owner_way, 10);
        assert_eq!(post.blades.len(), 2);
        let main = post
            .blades
            .iter()
            .find(|b| b.name == "Main Street")
            .unwrap();
        assert!(main.dir.0.abs() > 0.9);
        let cross = post.blades.iter().find(|b| b.name == "Cross Road").unwrap();
        assert!(cross.dir.1.abs() > 0.9);
    }

    /// Builds a context whose registry holds exactly `keys`, so panel placement can run.
    fn test_ctx(keys: Vec<DecalKey>) -> SignageContext {
        SignageContext {
            registry: DecalRegistry::from_keys(keys.into_iter().collect()),
            level: SignageLevel::Basic,
            region: SignRegion::Germanic,
            intersections: IntersectionIndex::new(),
            scale: 1.0,
            carriageway: crate::floodfill_cache::CoordinateBitmap::new_empty(),
            report: Mutex::new(PlacementReport::new()),
        }
    }

    /// Absolute Y of every frame showing one of `key`'s tiles, and their positions.
    fn placed_tiles(
        editor: &WorldEditor,
        ctx: &SignageContext,
        key: &DecalKey,
    ) -> Vec<(i32, i32, i32, i32)> {
        let entry = ctx.registry.get(key).unwrap();
        let ids: Vec<i32> = (0..entry.cols).map(|c| entry.tile_id(c, 0)).collect();
        let mut out: Vec<(i32, i32, i32, i32)> = editor
            .item_frames()
            .into_iter()
            .filter(|f| ids.contains(&f.map_id))
            .map(|f| (f.map_id, f.x, f.y, f.z))
            .collect();
        out.sort_unstable();
        out
    }

    #[test]
    fn panel_on_a_flat_wall_is_one_row_at_one_depth() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        let key = DecalKey::text(TextStyle::Fascia, "Bakery", 2);
        let ctx = std::sync::Arc::new(test_ctx(vec![key.clone()]));
        editor.set_map_decals(true);
        editor.set_signage(std::sync::Arc::clone(&ctx));

        // A wall along x at z = 20; the street is to the north (z = 19).
        for x in 10..30 {
            for y in 1..=5 {
                editor.set_block_absolute(STONE_BRICKS, x, y, 20, None, None);
            }
        }
        assert!(place_wall_panel(&mut editor, 20, 4, 20, 2, &key));

        let tiles = placed_tiles(&editor, &ctx, &key);
        assert_eq!(tiles.len(), 2, "both tiles placed");
        // Frames hang one block north of their host, all on the same plane and row.
        assert!(
            tiles.iter().all(|(_, _, y, z)| *y == 4 && *z == 19),
            "{tiles:?}"
        );
        assert_eq!(
            tiles[1].1 - tiles[0].1,
            -1,
            "second tile sits to the viewer's right"
        );
    }

    #[test]
    fn panel_on_a_stepped_wall_is_backfilled_flat() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        let key = DecalKey::text(TextStyle::Fascia, "Bakery", 2);
        let ctx = std::sync::Arc::new(test_ctx(vec![key.clone()]));
        editor.set_map_decals(true);
        editor.set_signage(std::sync::Arc::clone(&ctx));

        // Wall jumps back a block halfway, which used to split the tiles apart.
        for x in 10..20 {
            for y in 1..=5 {
                editor.set_block_absolute(STONE_BRICKS, x, y, 20, None, None);
            }
        }
        for x in 20..30 {
            for y in 1..=5 {
                editor.set_block_absolute(STONE_BRICKS, x, y, 21, None, None);
            }
        }
        assert!(place_wall_panel(&mut editor, 20, 4, 20, 2, &key));

        let tiles = placed_tiles(&editor, &ctx, &key);
        assert_eq!(tiles.len(), 2);
        let depths: std::collections::BTreeSet<i32> = tiles.iter().map(|(_, _, _, z)| *z).collect();
        assert_eq!(depths.len(), 1, "tiles share one plane: {tiles:?}");
        // The hole left by the step is backed with the facade's own block.
        assert_eq!(
            editor
                .get_block_absolute(20, 4, 20)
                .map(|b| b.name().to_string()),
            Some("stone_bricks".to_string())
        );
    }

    #[test]
    fn panel_refuses_a_wall_that_is_mostly_missing() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        let key = DecalKey::text(TextStyle::Fascia, "Bäckerei Konditorei", 3);
        let ctx = std::sync::Arc::new(test_ctx(vec![key.clone()]));
        editor.set_map_decals(true);
        editor.set_signage(std::sync::Arc::clone(&ctx));

        // Only one column of three has any wall behind it.
        for y in 1..=5 {
            editor.set_block_absolute(STONE_BRICKS, 20, y, 20, None, None);
        }
        assert!(!place_wall_panel(&mut editor, 20, 4, 20, 2, &key));
        assert!(placed_tiles(&editor, &ctx, &key).is_empty());
    }

    #[test]
    fn budget_drops_building_plates_first() {
        let mut keys: BTreeSet<DecalKey> = BTreeSet::new();
        for i in 0..20_000 {
            keys.insert(DecalKey::text(TextStyle::Fascia, format!("Shop {i}"), 2));
        }
        keys.insert(DecalKey::text(TextStyle::HouseNumber, "12a", 1));
        keys.insert(DecalKey::Traffic(TrafficSign::Stop));
        keys.insert(DecalKey::text(
            TextStyle::StreetName(crate::decals::region::BladeStyle::Blue),
            "Hauptstraße",
            1,
        ));
        drop_keys_over_budget(&mut keys);
        let tiles: u32 = keys
            .iter()
            .map(|k| {
                let (c, r) = k.dims();
                c * r
            })
            .sum();
        assert!(tiles <= MAX_DECAL_TILES, "{tiles}");
        // Infrastructure survives, building signage is what goes.
        assert!(keys.contains(&DecalKey::Traffic(TrafficSign::Stop)));
        assert!(keys.iter().any(|k| matches!(
            k,
            DecalKey::Text {
                style: TextStyle::StreetName(_),
                ..
            }
        )));
        assert!(!keys.iter().any(|k| matches!(
            k,
            DecalKey::Text {
                style: TextStyle::Fascia | TextStyle::HouseNumber,
                ..
            }
        )));
    }

    #[test]
    fn budget_leaves_a_normal_world_alone() {
        let mut keys: BTreeSet<DecalKey> = BTreeSet::new();
        for i in 0..500 {
            keys.insert(DecalKey::text(TextStyle::Fascia, format!("Shop {i}"), 2));
        }
        let before = keys.len();
        drop_keys_over_budget(&mut keys);
        assert_eq!(keys.len(), before);
    }

    #[test]
    fn split_lines_wraps_at_spaces() {
        assert_eq!(
            split_lines("Bäckerei Konditorei Müller", 12, 4),
            vec!["Bäckerei", "Konditorei", "Müller"]
        );
        assert_eq!(
            split_lines("abcdefghijklmnopqrstu", 10, 4),
            vec!["abcdefghij", "klmnopqrst", "u"]
        );
    }

    #[test]
    fn furniture_and_info_keys() {
        assert_eq!(
            furniture_pictogram(&tags(&[("emergency", "fire_hydrant")])),
            Some(DecalKey::Pictogram("hydrant"))
        );
        assert_eq!(
            furniture_pictogram(&tags(&[
                ("emergency", "fire_hydrant"),
                ("fire_hydrant:type", "underground")
            ])),
            None
        );
        assert_eq!(
            furniture_pictogram(&tags(&[("amenity", "waste_basket")])),
            Some(DecalKey::Pictogram("recycling"))
        );
        let board = ProcessedNode {
            id: 1,
            tags: tags(&[("tourism", "information"), ("information", "map")]),
            x: 5,
            z: 7,
        };
        assert_eq!(
            information_key(&board),
            Some(DecalKey::LocalMap { x: 5, z: 7 })
        );
        let guide = ProcessedNode {
            id: 1,
            tags: tags(&[("tourism", "information"), ("information", "guidepost")]),
            x: 5,
            z: 7,
        };
        assert_eq!(
            information_key(&guide),
            Some(DecalKey::Pictogram("information"))
        );
    }

    #[test]
    fn advertising_variants_are_deterministic() {
        let t = tags(&[("advertising", "column")]);
        assert_eq!(advertising_keys(&t, 7), advertising_keys(&t, 7));
        assert_eq!(advertising_keys(&t, 7).len(), 4);
        assert_eq!(
            advertising_keys(&tags(&[("advertising", "billboard")]), 8),
            vec![DecalKey::Poster(2)]
        );
    }
}
