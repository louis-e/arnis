//! Overture Maps building data integration.
//!
//! Fetches ML-derived building footprints from Overture Maps to complement
//! OpenStreetMap data. Only buildings NOT sourced from OSM are included,
//! filling gaps in areas with sparse OSM coverage (e.g., rural Africa,
//! parts of Asia).
//!
//! OSM-sourced rows are not discarded outright: Overture conflates heights and
//! floor counts from Microsoft, Esri and USGS 3DEP lidar onto them, so their
//! `sources[].record_id` back-reference is used to fill `height` /
//! `building:levels` on OSM buildings that carry neither tag. That enrichment
//! is strictly additive - an existing OSM tag is never overwritten.
//!
//! Data is read from GeoParquet files hosted on Azure Blob Storage using
//! HTTP Range requests (same pattern as land_cover.rs COG reading).

use crate::clipping::clip_way_to_bbox;
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer;
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use parquet::file::metadata::ParquetMetaData;
use parquet::file::reader::{ChunkReader, FileReader, Length};
use parquet::file::serialized_reader::SerializedFileReader;
use parquet::record::Row;
use reqwest::blocking::Client;
use std::collections::HashMap;
use std::time::Duration;

// ─── Constants ────────────────────────────────────────────────────────────

/// Overture STAC catalog root; releases live at /<release>/collections.parquet.
const OVERTURE_STAC_ROOT: &str = "https://stac.overturemaps.org";

/// Bucket listing used to discover release names; only the newest few stay online.
const OVERTURE_RELEASE_LIST_URL: &str =
    "https://overturemaps-us-west-2.s3.amazonaws.com/?list-type=2&prefix=release/&delimiter=/";

/// Used when release discovery fails; bump occasionally to a recent release.
const OVERTURE_STAC_RELEASE_FALLBACK: &str = "2026-07-22.0";

/// How many releases to request before giving up, so a broken host cannot stall the fetch.
const OVERTURE_MAX_RELEASE_ATTEMPTS: usize = 3;

/// High bit marker for Overture IDs to avoid collision with OSM IDs.
/// OSM IDs are sequential positive u64 (currently up to ~12 billion, well under 2^34).
/// Setting bit 63 guarantees no collision.
const OVERTURE_ID_HIGH_BIT: u64 = 0x8000_0000_0000_0000;

/// Budget of Overture footprints, as a rate per km² of the requested area plus a
/// floor and a ceiling. The cap exists so a large request cannot exhaust memory.
///
/// It scales with area only so that the cap stops binding on ordinary requests: a
/// flat 100k was already spent before a mid-size city was finished. It does NOT fix
/// how the overflow is distributed. Rows arrive in spatial order and the budget is a
/// running total, so whatever exceeds it is still a contiguous block of districts
/// with no footprints. Raising the number moves that threshold; it does not remove
/// it, which is why hitting the cap is now reported rather than passed over.
const OVERTURE_BUILDINGS_PER_KM2: f64 = 1_000.0;
const MIN_OVERTURE_BUILDINGS: usize = 100_000;
/// Roughly the supported area ceiling at the rate above. Past this the memory held
/// by the raw rows and their expanded ways is the binding constraint, not coverage.
const MAX_OVERTURE_BUILDINGS: usize = 500_000;

/// Attempts per HTTP range read before a row group is given up on. One partition
/// is hundreds of range requests, and a dropped row group takes a contiguous block
/// of the map's footprints with it, so a transient failure must not settle it.
const OVERTURE_RANGE_ATTEMPTS: u32 = 3;

/// Once a partition has lost this many row groups the host is not having a bad
/// moment, it is unhealthy. Retrying the rest only multiplies the wall clock by
/// the attempt count, so the remaining reads get a single try.
const OVERTURE_UNHEALTHY_FAILURES: usize = 3;

/// HTTP client timeout for Overture data fetching
const HTTP_TIMEOUT_SECS: u64 = 120;

/// Cap on how many OSM buildings can receive Overture attribute hints. Hints are
/// a few bytes each, so this only guards against a runaway area.
const MAX_OSM_HINTS: usize = 500_000;

/// Plausibility window for an Overture height (metres) before it may be written
/// onto an OSM building. ML-derived heights occasionally emit sub-metre slivers
/// or absurd towers; those are dropped rather than turned into blocks.
const HINT_MIN_HEIGHT_M: f64 = 2.5;
const HINT_MAX_HEIGHT_M: f64 = 500.0;

// ─── Internal data types ─────────────────────────────────────────────────

/// Back-reference from an Overture row to the OSM element it was conflated from,
/// parsed out of `sources[].record_id` (e.g. `"w519166507@9"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OsmRef {
    /// "way" or "relation" - matches `ProcessedElement::kind()`.
    kind: &'static str,
    id: u64,
}

/// Attributes Overture holds for an OSM building that OSM itself does not.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OsmAttributeHint {
    height_m: Option<f64>,
    num_floors: Option<i32>,
}

impl OsmAttributeHint {
    /// Drop values the generator would refuse to use, so the hint budget is
    /// spent only on attributes that can actually reach a building. This is the
    /// single place those rules live; `apply` trusts what it is given.
    fn usable(self) -> Self {
        Self {
            // ML-derived heights occasionally emit sub-metre slivers or absurd
            // towers; neither should become blocks.
            height_m: self
                .height_m
                .filter(|h| (HINT_MIN_HEIGHT_M..=HINT_MAX_HEIGHT_M).contains(h)),
            // A single floor adds nothing over the generator's own inference,
            // and the upper bound guards against parse noise.
            num_floors: self.num_floors.filter(|f| (2..200).contains(f)),
        }
    }

    fn is_empty(&self) -> bool {
        self.height_m.is_none() && self.num_floors.is_none()
    }
}

/// `OsmRef` -> attributes, built from the OSM-sourced Overture rows that the
/// footprint pass discards.
#[derive(Debug, Default)]
pub struct OvertureHints {
    hints: HashMap<OsmRef, OsmAttributeHint>,
}

impl OvertureHints {
    pub fn len(&self) -> usize {
        self.hints.len()
    }

    fn insert(&mut self, key: OsmRef, hint: OsmAttributeHint) {
        self.insert_capped(key, hint, MAX_OSM_HINTS);
    }

    fn insert_capped(&mut self, key: OsmRef, hint: OsmAttributeHint, cap: usize) {
        // Most Overture rows carry no height at all; storing those would spend
        // the budget on entries that could never enrich anything.
        let hint = hint.usable();
        if hint.is_empty() {
            return;
        }

        // Only a new key costs budget. An already-tracked building must stay
        // completable, otherwise a full map would freeze half-filled entries.
        let at_capacity = self.hints.len() >= cap;
        match self.hints.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                // Overture is one row per building, but a partition boundary can
                // repeat a row; keep the first, richer entry rather than letting
                // a later partial one clobber it.
                let slot = slot.get_mut();
                if slot.height_m.is_none() {
                    slot.height_m = hint.height_m;
                }
                if slot.num_floors.is_none() {
                    slot.num_floors = hint.num_floors;
                }
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                if !at_capacity {
                    slot.insert(hint);
                }
            }
        }
    }

    /// Fill `height` / `building:levels` on OSM buildings that carry neither.
    ///
    /// Strictly additive: an element that already has either tag is left alone,
    /// so OSM mappers always win over Overture's conflated values. Returns the
    /// number of elements that were enriched.
    pub fn apply(&self, elements: &mut [ProcessedElement]) -> usize {
        if self.hints.is_empty() {
            return 0;
        }

        let mut enriched = 0;
        for element in elements.iter_mut() {
            let key = OsmRef {
                kind: match element {
                    ProcessedElement::Way(_) => "way",
                    ProcessedElement::Relation(_) => "relation",
                    // Overture buildings are areas; a node reference cannot be a footprint.
                    ProcessedElement::Node(_) => continue,
                },
                id: element.id(),
            };
            let Some(hint) = self.hints.get(&key) else {
                continue;
            };

            let tags: &mut HashMap<String, String> = match element {
                ProcessedElement::Way(way) => &mut way.tags,
                ProcessedElement::Relation(relation) => &mut relation.tags,
                ProcessedElement::Node(_) => continue,
            };

            // Only buildings; Overture's building theme has no other feature type.
            if !tags.contains_key("building") && !tags.contains_key("building:part") {
                continue;
            }
            // Any existing vertical tag means OSM already knows better.
            if tags.contains_key("height") || tags.contains_key("building:levels") {
                continue;
            }

            // Every stored hint holds at least one validated value (see `insert`).
            if let Some(floors) = hint.num_floors {
                tags.insert("building:levels".to_string(), floors.to_string());
            }
            if let Some(h) = hint.height_m {
                tags.insert("height".to_string(), format!("{h:.1}"));
            }
            // Marks the provenance for debugging and for the licence notice.
            tags.insert(
                "arnis:height_source".to_string(),
                "overture_maps".to_string(),
            );
            enriched += 1;
        }
        enriched
    }
}

/// Pull the OSM back-reference out of an Overture `sources` value.
///
/// The column is `list<struct<property, dataset, record_id, update_time, ...>>`.
/// Walks the nested value rather than pattern-matching a formatted string, and
/// returns `None` for any shape it does not recognise so a schema change can
/// only cost the enrichment, never the footprint pass.
fn parse_osm_ref(field: &parquet::record::Field) -> Option<OsmRef> {
    fn walk(field: &parquet::record::Field, out: &mut Option<OsmRef>) {
        if out.is_some() {
            return;
        }
        match field {
            parquet::record::Field::ListInternal(list) => {
                for element in list.elements() {
                    walk(element, out);
                    if out.is_some() {
                        return;
                    }
                }
            }
            parquet::record::Field::Group(row) => {
                let mut dataset: Option<&str> = None;
                let mut record_id: Option<&str> = None;
                for (name, sub) in row.get_column_iter() {
                    match (name.as_str(), sub) {
                        ("dataset", parquet::record::Field::Str(v)) => dataset = Some(v),
                        ("record_id", parquet::record::Field::Str(v)) => record_id = Some(v),
                        // A nested group/list (some writers wrap the list element)
                        _ => walk(sub, out),
                    }
                    if out.is_some() {
                        return;
                    }
                }
                if dataset.is_some_and(|d| d.eq_ignore_ascii_case("OpenStreetMap")) {
                    if let Some(parsed) = record_id.and_then(parse_record_id) {
                        *out = Some(parsed);
                    }
                }
            }
            _ => {}
        }
    }

    let mut out = None;
    walk(field, &mut out);
    out
}

/// `"w519166507@9"` -> way 519166507. Also accepts `r`/`n` prefixes and a
/// missing `@version` suffix.
fn parse_record_id(record_id: &str) -> Option<OsmRef> {
    let record_id = record_id.trim();
    let mut chars = record_id.chars();
    let kind = match chars.next()? {
        'w' | 'W' => "way",
        'r' | 'R' => "relation",
        // Nodes cannot carry a building footprint; ignore rather than mismatch.
        'n' | 'N' => return None,
        _ => return None,
    };
    let rest = chars.as_str();
    let digits = rest.split('@').next()?;
    let id: u64 = digits.parse().ok()?;
    Some(OsmRef { kind, id })
}

/// A building parsed from Overture Maps GeoParquet data.
pub(crate) struct OvertureBuilding {
    /// GERS ID (UUID string)
    id: String,
    /// Exterior ring coordinates as (longitude, latitude) pairs
    pub(crate) exterior_ring: Vec<(f64, f64)>,
    /// Whether the primary source is OpenStreetMap
    is_osm_sourced: bool,
    /// The OSM element this row was conflated from, when it could be parsed.
    osm_ref: Option<OsmRef>,
    /// Building height in meters (if available)
    pub(crate) height: Option<f64>,
    /// Minimum height in meters (bottom of building, for elevated parts)
    pub(crate) min_height: Option<f64>,
    /// Number of above-ground floors (if available)
    pub(crate) num_floors: Option<i32>,
    /// Overture subtype (e.g., "residential", "commercial")
    subtype: Option<String>,
    /// Overture class (e.g., "house", "apartments")
    class: Option<String>,
    /// Roof shape (e.g., "gabled", "flat")
    roof_shape: Option<String>,
    /// Roof material (e.g., "metal", "glass", "roof_tiles")
    roof_material: Option<String>,
    /// Roof orientation relative to longest axis ("along" or "across")
    roof_orientation: Option<String>,
    /// Facade color (hex or name)
    facade_color: Option<String>,
    /// Roof color (hex or name)
    roof_color: Option<String>,
}

// ─── Public API ──────────────────────────────────────────────────────────

/// What a generation-path Overture fetch produces.
#[derive(Default)]
pub struct OvertureData {
    /// Non-OSM footprints, ready to merge into the element list.
    pub elements: Vec<ProcessedElement>,
    /// Attributes for OSM buildings that OSM itself leaves untagged.
    pub hints: OvertureHints,
}

/// Fetch Overture Maps building data for the given bbox.
///
/// Returns `ProcessedWay` elements with OSM-compatible tags, ready to merge
/// with the main element list, plus attribute hints keyed on the OSM elements
/// Overture conflated. Returns empty data on any failure (non-fatal).
///
/// Buildings whose primary source is "OpenStreetMap" are excluded from
/// `elements` to avoid duplicates with the existing OSM data pipeline; their
/// conflated heights survive in `hints`.
pub fn fetch_overture_buildings(bbox: &LLBBox, scale: f64, debug: bool) -> OvertureData {
    match fetch_overture_buildings_inner(bbox, scale, debug) {
        Ok(data) => data,
        Err(e) => {
            eprintln!(
                "{} Failed to fetch Overture Maps data: {e}",
                "Warning:".yellow().bold()
            );
            OvertureData::default()
        }
    }
}

/// Remove Overture buildings that spatially overlap existing OSM buildings.
///
/// For each Overture building, checks if its centroid falls within the bounding
/// box of any existing OSM building. This catches remaining duplicates that
/// slipped through the source-based filtering (e.g., buildings mapped differently
/// in OSM vs ML sources).
pub fn deduplicate_against_osm(
    overture_elements: Vec<ProcessedElement>,
    osm_elements: &[ProcessedElement],
) -> Vec<ProcessedElement> {
    // Collect bounding boxes of all OSM buildings
    let osm_building_bboxes: Vec<(i32, i32, i32, i32)> = osm_elements
        .iter()
        .filter_map(|el| {
            if let ProcessedElement::Way(way) = el {
                if (way.tags.contains_key("building") || way.tags.contains_key("building:part"))
                    && way.nodes.len() >= 3
                {
                    let min_x = way.nodes.iter().map(|n| n.x).min().unwrap();
                    let max_x = way.nodes.iter().map(|n| n.x).max().unwrap();
                    let min_z = way.nodes.iter().map(|n| n.z).min().unwrap();
                    let max_z = way.nodes.iter().map(|n| n.z).max().unwrap();
                    return Some((min_x, min_z, max_x, max_z));
                }
            }
            None
        })
        .collect();

    if osm_building_bboxes.is_empty() {
        return overture_elements;
    }

    // Build a simple spatial grid for fast overlap checks.
    // Grid cell size of 64 blocks keeps the grid manageable while providing
    // good spatial filtering.
    const CELL_SIZE: i32 = 64;

    let grid_min_x = osm_building_bboxes.iter().map(|b| b.0).min().unwrap();
    let grid_min_z = osm_building_bboxes.iter().map(|b| b.1).min().unwrap();

    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (idx, &(min_x, min_z, max_x, max_z)) in osm_building_bboxes.iter().enumerate() {
        let cell_x_start = (min_x - grid_min_x) / CELL_SIZE;
        let cell_z_start = (min_z - grid_min_z) / CELL_SIZE;
        let cell_x_end = (max_x - grid_min_x) / CELL_SIZE;
        let cell_z_end = (max_z - grid_min_z) / CELL_SIZE;

        for cx in cell_x_start..=cell_x_end {
            for cz in cell_z_start..=cell_z_end {
                grid.entry((cx, cz)).or_default().push(idx);
            }
        }
    }

    overture_elements
        .into_iter()
        .filter(|el| {
            if let ProcessedElement::Way(way) = el {
                if way.nodes.is_empty() {
                    return false;
                }
                // Compute centroid
                let cx = way.nodes.iter().map(|n| n.x as i64).sum::<i64>() / way.nodes.len() as i64;
                let cz = way.nodes.iter().map(|n| n.z as i64).sum::<i64>() / way.nodes.len() as i64;
                let cx = cx as i32;
                let cz = cz as i32;

                // Look up grid cell
                let cell_key = ((cx - grid_min_x) / CELL_SIZE, (cz - grid_min_z) / CELL_SIZE);
                if let Some(candidates) = grid.get(&cell_key) {
                    for &idx in candidates {
                        let (min_x, min_z, max_x, max_z) = osm_building_bboxes[idx];
                        if cx >= min_x && cx <= max_x && cz >= min_z && cz <= max_z {
                            return false; // Overlaps with existing OSM building
                        }
                    }
                }
                true
            } else {
                true
            }
        })
        .collect()
}

// ─── Inner implementation ────────────────────────────────────────────────

/// Shared HTTP client for Overture fetches (generation and 3D preview paths).
pub(crate) fn overture_client() -> Result<Client, Box<dyn std::error::Error>> {
    Ok(Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .user_agent(concat!(
            "Arnis/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/louis-e/arnis)"
        ))
        .build()?)
}

/// Everything one Overture pass yields: the footprints to add, plus the
/// attribute hints harvested from the OSM-sourced rows that are not added.
#[derive(Default)]
pub(crate) struct OvertureCollection {
    pub(crate) buildings: Vec<OvertureBuilding>,
    pub(crate) hints: OvertureHints,
}

/// How many Overture footprints this bbox is allowed to contribute.
fn overture_building_budget(bbox: &LLBBox) -> usize {
    ((bbox.area_km2() * OVERTURE_BUILDINGS_PER_KM2) as usize)
        .clamp(MIN_OVERTURE_BUILDINGS, MAX_OVERTURE_BUILDINGS)
}

/// Collects raw Overture buildings overlapping the bbox. The generation path
/// drops OSM-sourced entries (duplicates of the Overpass data); the 3D
/// preview keeps them for full coverage.
///
/// Either way the OSM-sourced rows are mined for `height` / `num_floors` hints
/// first, so the enrichment costs no extra requests. Hints never count towards
/// `max_buildings` - that cap governs added footprints only.
///
/// `report_gaps` warns about coverage the caller did not get: skipped partitions,
/// unreadable row groups, and a spent budget. Only the generation path wants this.
/// For the 3D preview `max_buildings` is a deliberate render budget rather than a
/// data problem, and its fetch repeats on every pan.
pub(crate) fn collect_overture_buildings(
    client: &Client,
    bbox: &LLBBox,
    include_osm_sourced: bool,
    max_buildings: usize,
    report_gaps: bool,
    debug: bool,
) -> Result<OvertureCollection, Box<dyn std::error::Error>> {
    // List partition files whose geographic bounds overlap our bbox
    // (single ~230 KB STAC download instead of 512 HTTP requests)
    let partition_urls = list_partition_files(client, bbox, debug)?;
    if partition_urls.is_empty() {
        if debug {
            println!("No Overture partitions overlap the bbox");
        }
        return Ok(OvertureCollection::default());
    }

    if debug {
        println!(
            "Found {} Overture partition(s) for this area",
            partition_urls.len()
        );
    }

    // Process each partition file: read footer, check for bbox overlap, fetch matching rows
    let mut all_buildings: Vec<OvertureBuilding> = Vec::new();
    let mut hints = OvertureHints::default();
    // Rows are stored in spatial order, so anything skipped here is a contiguous
    // block of the map with no ML footprints. Reported below rather than only
    // under --debug, since the world itself gives no hint that it happened.
    let mut lost_partitions = 0usize;
    let mut lost_row_groups = 0usize;
    let mut capped = false;

    for (i, url) in partition_urls.iter().enumerate() {
        if all_buildings.len() >= max_buildings {
            capped = true;
            break;
        }

        if debug && i % 10 == 0 {
            println!(
                "Processing partition {}/{} ...",
                i + 1,
                partition_urls.len()
            );
        }

        match process_partition_file(client, url, bbox, debug) {
            Ok((buildings, failed_row_groups)) => {
                lost_row_groups += failed_row_groups;
                for building in buildings {
                    // Harvest first: an OSM-sourced row is a duplicate footprint
                    // but still carries conflated Microsoft / Esri / 3DEP values.
                    if building.is_osm_sourced {
                        if let Some(key) = building.osm_ref {
                            hints.insert(
                                key,
                                OsmAttributeHint {
                                    height_m: building.height,
                                    num_floors: building.num_floors,
                                },
                            );
                        }
                        if !include_osm_sourced {
                            continue;
                        }
                    }
                    all_buildings.push(building);
                }
            }
            Err(e) => {
                lost_partitions += 1;
                if debug {
                    eprintln!("Warning: Failed to process partition {url}: {e}");
                }
                // Continue with other partitions
            }
        }
    }

    // The loop only checks the cap between partitions; a single dense
    // partition can overshoot it, so enforce the exact cap here.
    capped |= all_buildings.len() > max_buildings;
    all_buildings.truncate(max_buildings);

    if report_gaps && (lost_partitions > 0 || lost_row_groups > 0) {
        eprintln!(
            "{} Overture Maps data incomplete: {lost_partitions} partition(s) and \
             {lost_row_groups} row group(s) could not be read. Buildings are missing \
             from the areas they cover.",
            "Warning:".yellow().bold()
        );
    }
    if report_gaps && capped {
        eprintln!(
            "{} Reached the Overture Maps building limit ({max_buildings}); footprints \
             beyond it were dropped, which leaves whole districts without them. \
             Use a smaller area for full coverage.",
            "Warning:".yellow().bold()
        );
    }

    Ok(OvertureCollection {
        buildings: all_buildings,
        hints,
    })
}

fn fetch_overture_buildings_inner(
    bbox: &LLBBox,
    scale: f64,
    debug: bool,
) -> Result<OvertureData, Box<dyn std::error::Error>> {
    let client = overture_client()?;

    emit_gui_progress_update(6.0, "Downloading data...");

    let budget = overture_building_budget(bbox);
    let OvertureCollection {
        buildings: all_buildings,
        hints,
    } = collect_overture_buildings(&client, bbox, false, budget, true, debug)?;

    if debug {
        println!(
            "Overture: {} non-OSM buildings found, {} attribute hints for OSM buildings",
            all_buildings.len(),
            hints.len()
        );
    }

    // Convert to ProcessedElements and clip to xzbbox (matching OSM clipping)
    let (coord_transformer, xzbbox) = CoordTransformer::llbbox_to_xzbbox(bbox, scale)?;

    let elements: Vec<ProcessedElement> = all_buildings
        .into_iter()
        .take(budget)
        .filter_map(|building| {
            let mut way = building_to_processed_way(&building, &coord_transformer, bbox)?;
            let clipped = clip_way_to_bbox(&way.nodes, &xzbbox);
            if clipped.len() < 3 {
                return None;
            }
            way.nodes = clipped;
            Some(ProcessedElement::Way(way))
        })
        .collect();

    Ok(OvertureData { elements, hints })
}

/// Sorts `YYYY-MM-DD.N` release names, comparing the revision numerically.
fn release_sort_key(release: &str) -> (&str, u32) {
    match release.split_once('.') {
        Some((date, rev)) => (date, rev.parse().unwrap_or(0)),
        None => (release, 0),
    }
}

/// Extract release names from an S3 `ListObjectsV2` response, newest first.
fn parse_release_listing(body: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    // Releases arrive as <Prefix>release/2026-07-22.0/</Prefix>; the echoed request prefix strips to empty.
    let mut xml = Reader::from_str(body);
    let mut releases: Vec<String> = Vec::new();
    let mut in_prefix = false;
    loop {
        match xml.read_event()? {
            Event::Start(e) if e.local_name().as_ref() == b"Prefix" => in_prefix = true,
            Event::End(e) if e.local_name().as_ref() == b"Prefix" => in_prefix = false,
            Event::Text(e) if in_prefix => {
                let text = e.xml10_content()?;
                if let Some(name) = text.trim().strip_prefix("release/") {
                    let name = name.trim_end_matches('/');
                    if !name.is_empty() {
                        releases.push(name.to_string());
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    releases.sort_by(|a, b| release_sort_key(b).cmp(&release_sort_key(a)));
    releases.dedup();
    Ok(releases)
}

/// Release names currently published in the bucket, newest first.
fn discover_releases(client: &Client) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let body = client
        .get(OVERTURE_RELEASE_LIST_URL)
        .send()?
        .error_for_status()?
        .text()?;
    parse_release_listing(&body)
}

/// Downloads the STAC index from the newest release that serves one, oldest tried last.
fn fetch_stac_catalog(
    client: &Client,
    debug: bool,
) -> Result<reqwest::blocking::Response, Box<dyn std::error::Error>> {
    let releases = match discover_releases(client) {
        Ok(releases) => releases,
        Err(e) => {
            if debug {
                println!("Overture release discovery failed ({e}), using bundled release");
            }
            Vec::new()
        }
    };

    // Reserve the last attempt for the fallback so it stays reachable on a long listing.
    let mut candidates: Vec<String> = releases
        .into_iter()
        .take(OVERTURE_MAX_RELEASE_ATTEMPTS.saturating_sub(1))
        .collect();
    if !candidates
        .iter()
        .any(|r| r == OVERTURE_STAC_RELEASE_FALLBACK)
    {
        candidates.push(OVERTURE_STAC_RELEASE_FALLBACK.to_string());
    }

    if debug {
        println!("Overture releases to try: {}", candidates.join(", "));
    }

    let mut last_error = String::from("no Overture release candidates");
    for release in &candidates {
        let url = format!("{OVERTURE_STAC_ROOT}/{release}/collections.parquet");
        match client.get(&url).send() {
            Ok(response) if response.status().is_success() => {
                if debug {
                    println!("Using Overture release {release}");
                }
                return Ok(response);
            }
            Ok(response) => {
                last_error = format!(
                    "STAC catalog download failed with status {} (url: {url})",
                    response.status()
                );
            }
            Err(e) => last_error = format!("STAC catalog request failed: {e} (url: {url})"),
        }
        if debug {
            println!("Overture release {release} unavailable: {last_error}");
        }
    }

    Err(last_error.into())
}

/// List partition file URLs that overlap the target bbox.
///
/// Downloads the STAC `collections.parquet` index (~230 KB) and filters
/// by collection="building" + geographic bbox overlap. This replaces
/// the old approach of listing all 512 files from Azure and checking
/// each one individually (512+ HTTP requests → 1 request).
fn list_partition_files(
    client: &Client,
    bbox: &LLBBox,
    debug: bool,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // Resolve the current release dynamically; old releases are retired and 404.
    let stac_bytes = fetch_stac_catalog(client, debug)?.bytes()?;
    let reader = SerializedFileReader::new(stac_bytes)?;

    let target_min_lng = bbox.min().lng();
    let target_max_lng = bbox.max().lng();
    let target_min_lat = bbox.min().lat();
    let target_max_lat = bbox.max().lat();

    let mut urls: Vec<String> = Vec::new();

    let num_rg = reader.metadata().num_row_groups();
    for rg_idx in 0..num_rg {
        let rg_reader = reader.get_row_group(rg_idx)?;
        let row_iter = rg_reader.get_row_iter(None)?;

        for row in row_iter {
            let row = row?;
            // Each row is a STAC item. We need:
            //   - collection (string) == "building"
            //   - bbox.xmin, bbox.ymin, bbox.xmax, bbox.ymax (f64)
            //   - assets.azure.href (string) — the parquet file URL
            let mut collection: Option<String> = None;
            let mut item_xmin = f64::NAN;
            let mut item_ymin = f64::NAN;
            let mut item_xmax = f64::NAN;
            let mut item_ymax = f64::NAN;
            let mut azure_href: Option<String> = None;
            let mut aws_href: Option<String> = None;

            for (name, field) in row.get_column_iter() {
                match name.as_str() {
                    "collection" => {
                        if let parquet::record::Field::Str(s) = field {
                            collection = Some(s.clone());
                        }
                    }
                    "bbox" => {
                        if let parquet::record::Field::Group(group) = field {
                            for (key, val) in group.get_column_iter() {
                                if let parquet::record::Field::Double(v) = val {
                                    match key.as_str() {
                                        "xmin" => item_xmin = *v,
                                        "ymin" => item_ymin = *v,
                                        "xmax" => item_xmax = *v,
                                        "ymax" => item_ymax = *v,
                                        _ => {}
                                    }
                                } else if let parquet::record::Field::Float(v) = val {
                                    match key.as_str() {
                                        "xmin" => item_xmin = *v as f64,
                                        "ymin" => item_ymin = *v as f64,
                                        "xmax" => item_xmax = *v as f64,
                                        "ymax" => item_ymax = *v as f64,
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    "assets" => {
                        // Nested struct: assets.{azure,aws}.href
                        if let parquet::record::Field::Group(assets) = field {
                            for (provider, provider_field) in assets.get_column_iter() {
                                if let parquet::record::Field::Group(inner) = provider_field {
                                    for (key, val) in inner.get_column_iter() {
                                        if key == "href" {
                                            if let parquet::record::Field::Str(s) = val {
                                                match provider.as_str() {
                                                    "azure" => azure_href = Some(s.clone()),
                                                    "aws" => aws_href = Some(s.clone()),
                                                    _ => {}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            // Filter: only "building" collection items that overlap our bbox
            if collection.as_deref() != Some("building") {
                continue;
            }

            if item_xmin.is_nan() || item_ymin.is_nan() || item_xmax.is_nan() || item_ymax.is_nan()
            {
                continue;
            }

            // Standard bbox overlap test
            let overlaps = item_xmin <= target_max_lng
                && item_xmax >= target_min_lng
                && item_ymin <= target_max_lat
                && item_ymax >= target_min_lat;

            if overlaps {
                if let Some(href) = azure_href.or(aws_href) {
                    urls.push(href);
                }
            }
        }
    }

    if debug {
        println!(
            "STAC catalog: found {} partitions overlapping bbox",
            urls.len()
        );
    }

    Ok(urls)
}

/// Process a single Parquet partition file.
///
/// 1. Read the Parquet file footer via HTTP Range request
/// 2. Check row group statistics for bbox overlap
/// 3. Download and parse matching row groups
///
/// Returns the buildings plus the number of row groups that could not be read,
/// so the caller can report the coverage that was lost rather than hide it.
fn process_partition_file(
    client: &Client,
    url: &str,
    bbox: &LLBBox,
    debug: bool,
) -> Result<(Vec<OvertureBuilding>, usize), Box<dyn std::error::Error>> {
    // Step 1: Get file size via HEAD request
    let head_resp = client.head(url).send()?;
    if !head_resp.status().is_success() {
        return Err(format!("HEAD request failed: {}", head_resp.status()).into());
    }

    let file_size: u64 = head_resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or("Missing Content-Length header")?;

    if file_size < 12 {
        return Err("File too small to be valid Parquet".into());
    }

    // Step 2: Read the Parquet footer.
    // Parquet files end with: [footer bytes] [4-byte footer length (LE)] [4-byte magic "PAR1"]
    // First, read the last 8 bytes to get the footer length.
    let tail = fetch_range(client, url, file_size - 8, 8)?;
    if tail.len() < 8 {
        return Err(format!(
            "Truncated Parquet tail: expected 8 bytes, got {}",
            tail.len()
        )
        .into());
    }
    if &tail[4..8] != b"PAR1" {
        return Err("Not a valid Parquet file (missing PAR1 magic)".into());
    }

    let footer_len = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]) as u64;
    if footer_len > file_size - 8 {
        return Err("Invalid footer length".into());
    }

    // Read the footer bytes
    let footer_start = file_size - 8 - footer_len;
    let footer_bytes = fetch_range(client, url, footer_start, footer_len)?;

    // Parse the footer using the parquet crate
    let metadata = parquet::file::metadata::ParquetMetaDataReader::decode_metadata(&footer_bytes)?;

    // Step 3: Filter row groups by bbox overlap
    let matching_groups = filter_row_groups_by_bbox(&metadata, bbox);
    if matching_groups.is_empty() {
        return Ok((Vec::new(), 0));
    }

    if debug {
        println!(
            "  Partition has {} row groups, {} match bbox",
            metadata.num_row_groups(),
            matching_groups.len()
        );
    }

    // Step 4: Download only matching row groups via HTTP Range requests.
    // Each row group is typically ~4-5 MB. Partition files are ~580 MB each,
    // so this avoids downloading hundreds of MB for a small bbox.
    let mut sparse = SparseBytes::new(file_size);

    // Add footer + tail so SerializedFileReader::new() can parse metadata
    let mut footer_and_tail = Vec::with_capacity(footer_len as usize + 8);
    footer_and_tail.extend_from_slice(&footer_bytes);
    footer_and_tail.extend_from_slice(&tail);
    sparse.add_range(footer_start, bytes::Bytes::from(footer_and_tail));

    // Pre-fetch each matching row group's byte range
    let mut downloaded_bytes: u64 = footer_len + 8;
    let mut failed_row_groups = 0usize;
    let mut downloaded: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for &rg_idx in &matching_groups {
        let (rg_offset, rg_len) = row_group_byte_range(&metadata, rg_idx);
        // Each attempt can burn the full HTTP timeout, and there are hundreds of
        // these, so stop paying for retries once the host has proved unhealthy.
        let attempts = if failed_row_groups >= OVERTURE_UNHEALTHY_FAILURES {
            1
        } else {
            OVERTURE_RANGE_ATTEMPTS
        };
        match fetch_range_with_attempts(client, url, rg_offset, rg_len, attempts) {
            Ok(rg_data) => {
                downloaded_bytes += rg_len;
                downloaded.insert(rg_idx);
                sparse.add_range(rg_offset, bytes::Bytes::from(rg_data));
            }
            Err(e) => {
                failed_row_groups += 1;
                if debug {
                    eprintln!("Warning: Failed to download row group {rg_idx}: {e}");
                }
            }
        }
    }

    if debug {
        println!(
            "  Downloaded {:.1} MB (of {:.0} MB total file) for {} row groups",
            downloaded_bytes as f64 / 1_048_576.0,
            file_size as f64 / 1_048_576.0,
            matching_groups.len()
        );
    }

    sparse.finalize();
    let reader = SerializedFileReader::new(sparse)?;

    let target_min_lng = bbox.min().lng();
    let target_max_lng = bbox.max().lng();
    let target_min_lat = bbox.min().lat();
    let target_max_lat = bbox.max().lat();

    let mut buildings: Vec<OvertureBuilding> = Vec::new();

    for &rg_idx in &matching_groups {
        match parse_row_group(
            &reader,
            rg_idx,
            target_min_lng,
            target_max_lng,
            target_min_lat,
            target_max_lat,
        ) {
            Ok(rg_buildings) => buildings.extend(rg_buildings),
            Err(e) => {
                // A row group whose bytes never arrived fails here too; count it
                // once by only charging the groups that did download.
                if !downloaded.contains(&rg_idx) {
                    continue;
                }
                failed_row_groups += 1;
                if debug {
                    eprintln!("Warning: Failed to parse row group {rg_idx}: {e}");
                }
            }
        }
    }

    Ok((buildings, failed_row_groups))
}

/// Filter row groups whose bbox statistics overlap the target area.
///
/// Overture Parquet files have a struct column `bbox` with sub-columns
/// `xmin`, `ymin`, `xmax`, `ymax`. Row group statistics on these columns
/// tell us the min/max geographic extent of each row group.
fn filter_row_groups_by_bbox(metadata: &ParquetMetaData, bbox: &LLBBox) -> Vec<usize> {
    let target_min_lng = bbox.min().lng();
    let target_max_lng = bbox.max().lng();
    let target_min_lat = bbox.min().lat();
    let target_max_lat = bbox.max().lat();

    // Find column indices for bbox sub-columns
    let schema = metadata.file_metadata().schema_descr();
    let mut bbox_xmin_idx: Option<usize> = None;
    let mut bbox_ymin_idx: Option<usize> = None;
    let mut bbox_xmax_idx: Option<usize> = None;
    let mut bbox_ymax_idx: Option<usize> = None;

    for (i, col) in schema.columns().iter().enumerate() {
        let path = col.path().string();
        match path.as_str() {
            "bbox.xmin" => bbox_xmin_idx = Some(i),
            "bbox.ymin" => bbox_ymin_idx = Some(i),
            "bbox.xmax" => bbox_xmax_idx = Some(i),
            "bbox.ymax" => bbox_ymax_idx = Some(i),
            _ => {}
        }
    }

    // If we can't find bbox columns, include all row groups (fall back to row-level filtering)
    let (Some(xmin_idx), Some(ymin_idx), Some(xmax_idx), Some(ymax_idx)) =
        (bbox_xmin_idx, bbox_ymin_idx, bbox_xmax_idx, bbox_ymax_idx)
    else {
        return (0..metadata.num_row_groups()).collect();
    };

    let mut matching: Vec<usize> = Vec::new();

    for rg_idx in 0..metadata.num_row_groups() {
        let rg_meta = metadata.row_group(rg_idx);

        // Get statistics for each bbox column.
        // A row group matches if its geographic extent overlaps the target bbox.
        // We check: max(xmin_col) >= target_min_lng (there exist buildings east of our west edge)
        //           min(xmax_col) <= target_max_lng (there exist buildings west of our east edge)
        //           max(ymin_col) >= target_min_lat (there exist buildings north of our south edge)
        //           min(ymax_col) <= target_max_lat (there exist buildings south of our north edge)
        //
        // But actually, for row group statistics:
        // - The row group's min(xmin) is the westernmost building's west edge
        // - The row group's max(xmax) is the easternmost building's east edge
        // We need: row_group's geographic extent overlaps target bbox.
        //
        // Row group extent: [min(xmin), max(xmax)] x [min(ymin), max(ymax)]
        // Overlap condition:
        //   max(xmax) >= target_min_lng AND min(xmin) <= target_max_lng
        //   max(ymax) >= target_min_lat AND min(ymin) <= target_max_lat

        let overlaps = check_rg_overlap(
            rg_meta,
            xmin_idx,
            ymin_idx,
            xmax_idx,
            ymax_idx,
            target_min_lng,
            target_max_lng,
            target_min_lat,
            target_max_lat,
        );

        if overlaps {
            matching.push(rg_idx);
        }
    }

    matching
}

/// Check if a row group's bbox statistics overlap the target area.
#[allow(clippy::too_many_arguments)]
fn check_rg_overlap(
    rg_meta: &parquet::file::metadata::RowGroupMetaData,
    xmin_idx: usize,
    ymin_idx: usize,
    xmax_idx: usize,
    ymax_idx: usize,
    target_min_lng: f64,
    target_max_lng: f64,
    target_min_lat: f64,
    target_max_lat: f64,
) -> bool {
    // Helper to extract f64 min from column statistics
    let get_stat_min = |col_idx: usize| -> Option<f64> {
        let col = rg_meta.column(col_idx);
        if let Some(stats) = col.statistics() {
            if let parquet::file::statistics::Statistics::Float(s) = stats {
                return s.min_opt().map(|v| *v as f64);
            }
            if let parquet::file::statistics::Statistics::Double(s) = stats {
                return s.min_opt().copied();
            }
        }
        None
    };

    let get_stat_max = |col_idx: usize| -> Option<f64> {
        let col = rg_meta.column(col_idx);
        if let Some(stats) = col.statistics() {
            if let parquet::file::statistics::Statistics::Float(s) = stats {
                return s.max_opt().map(|v| *v as f64);
            }
            if let parquet::file::statistics::Statistics::Double(s) = stats {
                return s.max_opt().copied();
            }
        }
        None
    };

    // If we can't read statistics, include the row group (safe fallback)
    let Some(min_xmin) = get_stat_min(xmin_idx) else {
        return true;
    };
    let Some(max_xmax) = get_stat_max(xmax_idx) else {
        return true;
    };
    let Some(min_ymin) = get_stat_min(ymin_idx) else {
        return true;
    };
    let Some(max_ymax) = get_stat_max(ymax_idx) else {
        return true;
    };

    // Check overlap: row group's geographic extent overlaps target bbox
    max_xmax >= target_min_lng
        && min_xmin <= target_max_lng
        && max_ymax >= target_min_lat
        && min_ymin <= target_max_lat
}

/// Parse buildings from a single row group of an already-loaded Parquet file.
fn parse_row_group<R: ChunkReader + 'static>(
    reader: &SerializedFileReader<R>,
    rg_idx: usize,
    target_min_lng: f64,
    target_max_lng: f64,
    target_min_lat: f64,
    target_max_lat: f64,
) -> Result<Vec<OvertureBuilding>, Box<dyn std::error::Error>> {
    let row_group_reader = reader.get_row_group(rg_idx)?;
    let row_iter = row_group_reader.get_row_iter(None)?;

    let mut buildings: Vec<OvertureBuilding> = Vec::new();

    for row_result in row_iter {
        let row = row_result?;
        if let Some(building) = parse_overture_row(
            &row,
            target_min_lng,
            target_max_lng,
            target_min_lat,
            target_max_lat,
        ) {
            buildings.push(building);
        }
    }

    Ok(buildings)
}

/// Parse a single Parquet row into an OvertureBuilding.
///
/// Returns None if the row doesn't contain a valid building within the bbox,
/// or if required fields are missing.
fn parse_overture_row(
    row: &Row,
    target_min_lng: f64,
    target_max_lng: f64,
    target_min_lat: f64,
    target_max_lat: f64,
) -> Option<OvertureBuilding> {
    let mut id: Option<String> = None;
    let mut geometry_bytes: Option<Vec<u8>> = None;
    let mut sources_str: Option<String> = None;
    let mut osm_ref: Option<OsmRef> = None;
    let mut height: Option<f64> = None;
    let mut min_height: Option<f64> = None;
    let mut num_floors: Option<i32> = None;
    let mut subtype: Option<String> = None;
    let mut class: Option<String> = None;
    let mut roof_shape: Option<String> = None;
    let mut roof_material: Option<String> = None;
    let mut roof_orientation: Option<String> = None;
    let mut facade_color: Option<String> = None;
    let mut roof_color: Option<String> = None;
    let mut bbox_xmin: Option<f64> = None;
    let mut bbox_ymin: Option<f64> = None;
    let mut bbox_xmax: Option<f64> = None;
    let mut bbox_ymax: Option<f64> = None;

    // Extract fields from the row
    for (name, field) in row.get_column_iter() {
        match name.as_str() {
            "id" => {
                if let parquet::record::Field::Str(s) = field {
                    id = Some(s.clone());
                }
            }
            "geometry" => {
                if let parquet::record::Field::Bytes(b) = field {
                    geometry_bytes = Some(b.data().to_vec());
                }
            }
            "sources" => {
                // Sources is a complex nested struct; convert to string for analysis
                sources_str = Some(format!("{field}"));
                // Best-effort structured read for the OSM back-reference. The
                // string test above stays authoritative for dedup so a schema
                // change cannot resurrect duplicate footprints.
                osm_ref = parse_osm_ref(field);
            }
            "height" => {
                if let parquet::record::Field::Double(v) = field {
                    height = Some(*v);
                } else if let parquet::record::Field::Float(v) = field {
                    height = Some(*v as f64);
                }
            }
            "min_height" => {
                if let parquet::record::Field::Double(v) = field {
                    min_height = Some(*v);
                } else if let parquet::record::Field::Float(v) = field {
                    min_height = Some(*v as f64);
                }
            }
            "num_floors" => {
                if let parquet::record::Field::Int(v) = field {
                    num_floors = Some(*v);
                }
            }
            "subtype" => {
                if let parquet::record::Field::Str(s) = field {
                    subtype = Some(s.clone());
                }
            }
            "class" => {
                if let parquet::record::Field::Str(s) = field {
                    class = Some(s.clone());
                }
            }
            "roof_shape" => {
                if let parquet::record::Field::Str(s) = field {
                    roof_shape = Some(s.clone());
                }
            }
            "roof_material" => {
                if let parquet::record::Field::Str(s) = field {
                    roof_material = Some(s.clone());
                }
            }
            "roof_orientation" => {
                if let parquet::record::Field::Str(s) = field {
                    roof_orientation = Some(s.clone());
                }
            }
            "facade_color" => {
                if let parquet::record::Field::Str(s) = field {
                    facade_color = Some(s.clone());
                }
            }
            "roof_color" => {
                if let parquet::record::Field::Str(s) = field {
                    roof_color = Some(s.clone());
                }
            }
            "bbox" => {
                // bbox is a struct with sub-fields
                if let parquet::record::Field::Group(group) = field {
                    for (sub_name, sub_field) in group.get_column_iter() {
                        let val = match sub_field {
                            parquet::record::Field::Double(v) => Some(*v),
                            parquet::record::Field::Float(v) => Some(*v as f64),
                            _ => None,
                        };
                        if let Some(v) = val {
                            match sub_name.as_str() {
                                "xmin" => bbox_xmin = Some(v),
                                "ymin" => bbox_ymin = Some(v),
                                "xmax" => bbox_xmax = Some(v),
                                "ymax" => bbox_ymax = Some(v),
                                _ => {}
                            }
                        }
                    }
                }
            }
            _ => {} // Ignore other fields
        }
    }

    // Quick bbox check (row-level filtering since row group stats are approximate)
    if let (Some(xmin), Some(ymin), Some(xmax), Some(ymax)) =
        (bbox_xmin, bbox_ymin, bbox_xmax, bbox_ymax)
    {
        if xmax < target_min_lng
            || xmin > target_max_lng
            || ymax < target_min_lat
            || ymin > target_max_lat
        {
            return None; // Building is outside our bbox
        }
    }

    // Parse geometry
    let geometry_bytes = geometry_bytes?;
    let exterior_ring = parse_wkb_polygon(&geometry_bytes)?;
    if exterior_ring.len() < 3 {
        return None;
    }

    // Check if primary source is OSM
    let is_osm = sources_str
        .as_deref()
        .map(|s| s.contains("OpenStreetMap"))
        .unwrap_or(false);

    let id = id?;

    Some(OvertureBuilding {
        id,
        exterior_ring,
        is_osm_sourced: is_osm,
        osm_ref,
        height,
        min_height,
        num_floors,
        subtype,
        class,
        roof_shape,
        roof_material,
        roof_orientation,
        facade_color,
        roof_color,
    })
}

/// Parse WKB (Well-Known Binary) Polygon geometry into coordinate pairs.
///
/// Returns the exterior ring as a sequence of (longitude, latitude) pairs.
/// Supports both little-endian and big-endian byte order.
/// Only handles Polygon type (WKB type 3). MultiPolygon or other types are skipped.
fn parse_wkb_polygon(wkb: &[u8]) -> Option<Vec<(f64, f64)>> {
    if wkb.len() < 13 {
        // Minimum: 1 (byte order) + 4 (type) + 4 (num rings) + 4 (num points in ring)
        return None;
    }

    let byte_order = wkb[0];
    // WKB only defines 0 (big-endian) and 1 (little-endian)
    if byte_order > 1 {
        return None;
    }
    let is_le = byte_order == 1;

    let geom_type = if is_le {
        u32::from_le_bytes([wkb[1], wkb[2], wkb[3], wkb[4]])
    } else {
        u32::from_be_bytes([wkb[1], wkb[2], wkb[3], wkb[4]])
    };

    // Type 3 = Polygon. ISO WKB uses offsets: +1000 for Z, +2000 for M, +3000 for ZM.
    // Use modulo to extract the base type correctly for all dimension variants.
    let base_type = geom_type % 1000;
    if base_type != 3 {
        return None; // Not a Polygon
    }

    let num_rings = if is_le {
        u32::from_le_bytes([wkb[5], wkb[6], wkb[7], wkb[8]])
    } else {
        u32::from_be_bytes([wkb[5], wkb[6], wkb[7], wkb[8]])
    };

    if num_rings == 0 {
        return None;
    }

    // Parse the exterior ring (first ring)
    let mut offset = 9;
    if offset + 4 > wkb.len() {
        return None;
    }

    let num_points = if is_le {
        u32::from_le_bytes([
            wkb[offset],
            wkb[offset + 1],
            wkb[offset + 2],
            wkb[offset + 3],
        ])
    } else {
        u32::from_be_bytes([
            wkb[offset],
            wkb[offset + 1],
            wkb[offset + 2],
            wkb[offset + 3],
        ])
    };
    offset += 4;

    // Determine point stride (2D = 16 bytes, 3D = 24 bytes, etc.)
    let has_z = (geom_type / 1000) == 1 || (geom_type / 1000) == 3;
    let has_m = (geom_type / 1000) == 2 || (geom_type / 1000) == 3;
    let point_size: usize = 16 + if has_z { 8 } else { 0 } + if has_m { 8 } else { 0 };

    let needed = num_points as usize * point_size;
    if offset + needed > wkb.len() {
        return None;
    }

    let mut coords = Vec::with_capacity(num_points as usize);
    for _ in 0..num_points {
        let x = if is_le {
            f64::from_le_bytes([
                wkb[offset],
                wkb[offset + 1],
                wkb[offset + 2],
                wkb[offset + 3],
                wkb[offset + 4],
                wkb[offset + 5],
                wkb[offset + 6],
                wkb[offset + 7],
            ])
        } else {
            f64::from_be_bytes([
                wkb[offset],
                wkb[offset + 1],
                wkb[offset + 2],
                wkb[offset + 3],
                wkb[offset + 4],
                wkb[offset + 5],
                wkb[offset + 6],
                wkb[offset + 7],
            ])
        };
        let y = if is_le {
            f64::from_le_bytes([
                wkb[offset + 8],
                wkb[offset + 9],
                wkb[offset + 10],
                wkb[offset + 11],
                wkb[offset + 12],
                wkb[offset + 13],
                wkb[offset + 14],
                wkb[offset + 15],
            ])
        } else {
            f64::from_be_bytes([
                wkb[offset + 8],
                wkb[offset + 9],
                wkb[offset + 10],
                wkb[offset + 11],
                wkb[offset + 12],
                wkb[offset + 13],
                wkb[offset + 14],
                wkb[offset + 15],
            ])
        };
        offset += point_size;
        coords.push((x, y)); // (longitude, latitude)
    }

    Some(coords)
}

/// Convert an Overture building to a ProcessedWay with OSM-compatible tags.
fn building_to_processed_way(
    building: &OvertureBuilding,
    coord_transformer: &CoordTransformer,
    bbox: &LLBBox,
) -> Option<ProcessedWay> {
    let base_id = gers_id_to_u64(&building.id);

    // Convert coordinates to Minecraft XZ
    let mut nodes: Vec<ProcessedNode> = Vec::with_capacity(building.exterior_ring.len());

    // Track the building polygon's geographic bounding box from its actual vertices
    let mut poly_min_lat = f64::MAX;
    let mut poly_max_lat = f64::MIN;
    let mut poly_min_lng = f64::MAX;
    let mut poly_max_lng = f64::MIN;

    for (i, &(lng, lat)) in building.exterior_ring.iter().enumerate() {
        // Validate coordinate
        if !(-180.0..=180.0).contains(&lng) || !(-90.0..=90.0).contains(&lat) {
            continue;
        }

        // Update polygon bounding box
        poly_min_lat = poly_min_lat.min(lat);
        poly_max_lat = poly_max_lat.max(lat);
        poly_min_lng = poly_min_lng.min(lng);
        poly_max_lng = poly_max_lng.max(lng);

        let llpoint = match LLPoint::new(lat, lng) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let xz = coord_transformer.transform_point(llpoint);

        let node_id = base_id.wrapping_add(i as u64);
        nodes.push(ProcessedNode {
            id: node_id,
            tags: HashMap::new(),
            x: xz.x,
            z: xz.z,
        });
    }

    // Must have at least 3 nodes
    if nodes.len() < 3 {
        return None;
    }

    // Check that the building polygon's bounding box overlaps the target bbox.
    // This correctly handles buildings that straddle the bbox boundary (edges
    // cross but no vertices fall inside) — unlike a vertex-containment check.
    let bbox_overlaps = poly_max_lng >= bbox.min().lng()
        && poly_min_lng <= bbox.max().lng()
        && poly_max_lat >= bbox.min().lat()
        && poly_min_lat <= bbox.max().lat();
    if !bbox_overlaps {
        return None;
    }

    // Ensure the way is closed (first node == last node position)
    if let (Some(first), Some(last)) = (nodes.first(), nodes.last()) {
        if first.x != last.x || first.z != last.z {
            // Close the ring by duplicating the first node
            let closing_node = ProcessedNode {
                id: base_id.wrapping_add(building.exterior_ring.len() as u64),
                tags: HashMap::new(),
                x: first.x,
                z: first.z,
            };
            nodes.push(closing_node);
        }
    }

    // Build OSM-compatible tags
    let mut tags = HashMap::new();

    // Building type
    let building_type =
        overture_class_to_osm_building(building.subtype.as_deref(), building.class.as_deref());
    tags.insert("building".to_string(), building_type.to_string());

    // Height: only emit when the Overture value would produce a building at
    // least as tall as the pipeline's default (10 blocks for a generic house).
    // Overture ML heights for single-story houses are often 3-6 m, which maps
    // to 3-6 blocks — noticeably shorter than the 10-block OSM default.
    // Omitting low heights lets the pipeline use its default, keeping Overture
    // buildings visually consistent with OSM buildings.
    // When num_floors is also available, prefer building:levels because the
    // pipeline's `levels * 4 + 2` formula produces proportional Minecraft
    // heights (e.g., 2 floors → 10 blocks).
    let has_useful_floors = building.num_floors.is_some_and(|f| f >= 2);
    if let Some(h) = building.height {
        if h > 0.0 && h < 1000.0 {
            if has_useful_floors {
                // Let building:levels drive height — it produces better
                // proportions than raw meters. Only emit height for tall
                // buildings where the meter value adds precision.
                if h > 28.0 {
                    tags.insert("height".to_string(), format!("{h:.1}"));
                }
            } else if h >= 10.0 {
                // No floor count; only emit height when it exceeds the
                // pipeline default (10 blocks ≈ 10 m).
                tags.insert("height".to_string(), format!("{h:.1}"));
            }
            // Otherwise: omit height, let the pipeline default apply.
        }
    }

    // Min height (for elevated building parts)
    if let Some(h) = building.min_height {
        if h > 0.0 && h < 1000.0 {
            tags.insert("min_height".to_string(), format!("{h:.1}"));
        }
    }

    // Number of floors — only emit when >= 2 (the pipeline default assumes
    // 2 floors already; emitting 1 would make the building shorter).
    if let Some(floors) = building.num_floors {
        if (2..200).contains(&floors) {
            tags.insert("building:levels".to_string(), floors.to_string());
        }
    }

    // Roof shape
    if let Some(ref roof) = building.roof_shape {
        let osm_roof = match roof.as_str() {
            "gabled" | "gable" => "gabled",
            "hipped" | "hip" => "hipped",
            "flat" => "flat",
            "pyramidal" => "pyramidal",
            "dome" | "onion" => "dome",
            "skillion" | "shed" => "skillion",
            "gambrel" => "gambrel",
            "mansard" => "mansard",
            "round" => "round",
            other => other,
        };
        tags.insert("roof:shape".to_string(), osm_roof.to_string());
    }

    // Roof material (pipeline checks for "glass" to use glass blocks)
    if let Some(ref mat) = building.roof_material {
        // Overture uses underscores (e.g., "roof_tiles"), OSM uses underscores too
        tags.insert("roof:material".to_string(), mat.clone());
    }

    // Roof orientation ("along" or "across" relative to longest side)
    if let Some(ref orient) = building.roof_orientation {
        tags.insert("roof:orientation".to_string(), orient.clone());
    }

    // Facade color
    if let Some(ref color) = building.facade_color {
        tags.insert("building:colour".to_string(), color.clone());
    }

    // Roof color
    if let Some(ref color) = building.roof_color {
        tags.insert("roof:colour".to_string(), color.clone());
    }

    // Source tracking
    tags.insert("source".to_string(), "overture_maps".to_string());

    Some(ProcessedWay {
        id: base_id,
        nodes,
        tags,
    })
}

/// Map Overture subtype/class to OSM building tag value.
fn overture_class_to_osm_building<'a>(subtype: Option<&'a str>, class: Option<&'a str>) -> &'a str {
    // Try class first (more specific)
    if let Some(class) = class {
        match class {
            "house" | "detached" => return "house",
            "apartments" | "apartment" => return "apartments",
            "residential" => return "residential",
            "commercial" => return "commercial",
            "retail" => return "retail",
            "office" => return "office",
            "industrial" => return "industrial",
            "warehouse" => return "warehouse",
            "garage" | "garages" => return "garage",
            "shed" => return "shed",
            "school" => return "school",
            "hospital" => return "hospital",
            "church" | "mosque" | "temple" | "synagogue" => return "church",
            "hotel" => return "hotel",
            "farm" | "barn" => return "farm",
            _ => {}
        }
    }

    // Fall back to subtype
    if let Some(subtype) = subtype {
        match subtype {
            "residential" => return "residential",
            "commercial" => return "commercial",
            "industrial" => return "industrial",
            "agricultural" => return "farm",
            "civic" | "government" | "education" => return "public",
            "medical" => return "hospital",
            "religious" => return "church",
            "transportation" => return "transportation",
            "outbuilding" => return "shed",
            _ => {}
        }
    }

    "yes" // Generic building
}

/// Hash a GERS UUID string to a u64 with the high bit set.
///
/// Uses FNV-1a (not `DefaultHasher`) so that IDs are deterministic across
/// Rust compiler versions — `DefaultHasher`'s algorithm is explicitly not
/// a stable API contract.
///
/// Setting bit 63 guarantees no collision with OSM IDs (which are sequential
/// positive u64 currently up to ~12 billion, well under 2^34).
fn gers_id_to_u64(gers_id: &str) -> u64 {
    // FNV-1a parameters for u64
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in gers_id.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash | OVERTURE_ID_HIGH_BIT
}

// ─── Sparse byte reader for row-group-only downloads ─────────────────────

/// A sparse in-memory file reader for Parquet.
///
/// Only pre-fetched byte ranges are available; attempts to read uncached
/// regions return an error. This lets us download only the footer and
/// matching row groups (~4-5 MB each) instead of entire partition files
/// (~580 MB each).
struct SparseBytes {
    file_size: u64,
    /// Sorted (by offset) byte ranges fetched from the remote file.
    ranges: Vec<(u64, bytes::Bytes)>,
}

impl SparseBytes {
    fn new(file_size: u64) -> Self {
        Self {
            file_size,
            ranges: Vec::new(),
        }
    }

    fn add_range(&mut self, offset: u64, data: bytes::Bytes) {
        self.ranges.push((offset, data));
    }

    /// Sort ranges by offset. Call after all ranges have been added.
    fn finalize(&mut self) {
        self.ranges.sort_by_key(|(off, _)| *off);
    }
}

impl Length for SparseBytes {
    fn len(&self) -> u64 {
        self.file_size
    }
}

impl ChunkReader for SparseBytes {
    type T = std::io::Cursor<bytes::Bytes>;

    fn get_read(&self, start: u64) -> parquet::errors::Result<Self::T> {
        for (offset, data) in &self.ranges {
            let chunk_end = *offset + data.len() as u64;
            if start >= *offset && start < chunk_end {
                let local_start = (start - *offset) as usize;
                return Ok(std::io::Cursor::new(data.slice(local_start..)));
            }
        }
        Err(parquet::errors::ParquetError::General(format!(
            "Byte offset {start} not in pre-fetched ranges"
        )))
    }

    fn get_bytes(&self, start: u64, length: usize) -> parquet::errors::Result<bytes::Bytes> {
        let end = start + length as u64;
        for (offset, data) in &self.ranges {
            let chunk_end = *offset + data.len() as u64;
            if start >= *offset && end <= chunk_end {
                let local_start = (start - *offset) as usize;
                return Ok(data.slice(local_start..local_start + length));
            }
        }
        Err(parquet::errors::ParquetError::General(format!(
            "Byte range [{start}, {end}) not in pre-fetched ranges"
        )))
    }
}

/// Calculate the byte range of a row group from Parquet metadata.
///
/// Returns `(offset, length)` covering all column chunks in the row group.
/// Column chunks within a row group are stored contiguously, so a single
/// HTTP Range request can fetch the entire group.
fn row_group_byte_range(metadata: &ParquetMetaData, rg_idx: usize) -> (u64, u64) {
    let rg_meta = metadata.row_group(rg_idx);
    let mut min_offset = u64::MAX;
    let mut max_end = 0u64;

    for i in 0..rg_meta.num_columns() {
        let col = rg_meta.column(i);
        // Column data starts at dictionary_page_offset (if present) or data_page_offset
        let start = col
            .dictionary_page_offset()
            .unwrap_or_else(|| col.data_page_offset()) as u64;
        let end = start + col.compressed_size() as u64;
        min_offset = min_offset.min(start);
        max_end = max_end.max(end);
    }

    (min_offset, max_end.saturating_sub(min_offset))
}

/// Fetch a byte range from a URL via HTTP Range request, retrying transient failures.
fn fetch_range(
    client: &Client,
    url: &str,
    start: u64,
    length: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    fetch_range_with_attempts(client, url, start, length, OVERTURE_RANGE_ATTEMPTS)
}

/// As [`fetch_range`], with the retry budget chosen by the caller.
fn fetch_range_with_attempts(
    client: &Client,
    url: &str,
    start: u64,
    length: u64,
    max_attempts: u32,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if length == 0 {
        return Err("fetch_range called with length 0".into());
    }
    let end = start + length - 1;
    let mut last_error = String::new();

    for attempt in 0..max_attempts.max(1) {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(500 << (attempt - 1)));
        }
        let response = match client
            .get(url)
            .header("Range", format!("bytes={start}-{end}"))
            .send()
        {
            Ok(response) => response,
            Err(e) => {
                last_error = format!("range request to {url} failed: {e}");
                continue;
            }
        };

        let status = response.status();
        if status.as_u16() != 206 {
            last_error = format!("HTTP {status} fetching range from {url} (expected 206)");
            // Only an overloaded or rate-limiting host can answer differently next time.
            if !(status.is_server_error() || status.as_u16() == 429) {
                break;
            }
            continue;
        }

        match response.bytes() {
            Ok(body) => return Ok(body.to_vec()),
            Err(e) => last_error = format!("range body from {url} could not be read: {e}"),
        }
    }

    Err(last_error.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::osm_parser::{ProcessedRelation, ProcessedWay};

    fn hint_way(id: u64, tags: &[(&str, &str)]) -> ProcessedElement {
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes: Vec::new(),
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    fn hints_with(key: OsmRef, hint: OsmAttributeHint) -> OvertureHints {
        let mut hints = OvertureHints::default();
        hints.insert(key, hint);
        hints
    }

    #[test]
    fn record_id_parses_way_and_relation_with_and_without_version() {
        assert_eq!(
            parse_record_id("w519166507@9"),
            Some(OsmRef {
                kind: "way",
                id: 519166507
            })
        );
        assert_eq!(
            parse_record_id("r12345"),
            Some(OsmRef {
                kind: "relation",
                id: 12345
            })
        );
        // Nodes cannot be footprints, and junk must not resolve to some way.
        assert_eq!(parse_record_id("n2757802019@1"), None);
        assert_eq!(parse_record_id("519166507"), None);
        assert_eq!(parse_record_id("w"), None);
        assert_eq!(parse_record_id("wabc@1"), None);
        assert_eq!(parse_record_id(""), None);
    }

    #[test]
    fn hint_fills_height_only_when_osm_has_none() {
        let key = OsmRef {
            kind: "way",
            id: 42,
        };
        let hints = hints_with(
            key,
            OsmAttributeHint {
                height_m: Some(18.0),
                num_floors: Some(6),
            },
        );

        let mut elements = vec![hint_way(42, &[("building", "yes")])];
        assert_eq!(hints.apply(&mut elements), 1);
        let tags = elements[0].tags();
        assert_eq!(tags.get("height").map(String::as_str), Some("18.0"));
        assert_eq!(tags.get("building:levels").map(String::as_str), Some("6"));
        assert_eq!(
            tags.get("arnis:height_source").map(String::as_str),
            Some("overture_maps")
        );
    }

    #[test]
    fn existing_osm_height_or_levels_is_never_overwritten() {
        let key = OsmRef {
            kind: "way",
            id: 42,
        };
        let hints = hints_with(
            key,
            OsmAttributeHint {
                height_m: Some(18.0),
                num_floors: Some(6),
            },
        );

        // A height tag blocks the whole hint.
        let mut tagged_height = vec![hint_way(42, &[("building", "yes"), ("height", "7")])];
        assert_eq!(hints.apply(&mut tagged_height), 0);
        assert_eq!(
            tagged_height[0].tags().get("height").map(String::as_str),
            Some("7")
        );

        // So does a levels tag, since it is the other vertical source.
        let mut tagged_levels = vec![hint_way(
            42,
            &[("building", "yes"), ("building:levels", "2")],
        )];
        assert_eq!(hints.apply(&mut tagged_levels), 0);
        assert!(!tagged_levels[0].tags().contains_key("height"));
    }

    #[test]
    fn hints_skip_non_buildings_and_unmatched_ids() {
        let hints = hints_with(
            OsmRef {
                kind: "way",
                id: 42,
            },
            OsmAttributeHint {
                height_m: Some(18.0),
                num_floors: None,
            },
        );

        // Right id, but not a building.
        let mut not_a_building = vec![hint_way(42, &[("highway", "residential")])];
        assert_eq!(hints.apply(&mut not_a_building), 0);

        // Building, but a different id.
        let mut other_id = vec![hint_way(43, &[("building", "yes")])];
        assert_eq!(hints.apply(&mut other_id), 0);

        // Same numeric id on a relation must not collect a way's hint.
        let mut relation = vec![ProcessedElement::Relation(ProcessedRelation {
            id: 42,
            tags: [("building".to_string(), "yes".to_string())]
                .into_iter()
                .collect(),
            members: Vec::new(),
        })];
        assert_eq!(hints.apply(&mut relation), 0);
    }

    #[test]
    fn implausible_heights_are_dropped() {
        let key = OsmRef {
            kind: "way",
            id: 42,
        };

        // Sub-metre ML sliver: rejected, and with no floors nothing is applied.
        let sliver = hints_with(
            key,
            OsmAttributeHint {
                height_m: Some(0.4),
                num_floors: None,
            },
        );
        assert_eq!(sliver.len(), 0, "the sliver must not even be stored");
        let mut elements = vec![hint_way(42, &[("building", "yes")])];
        assert_eq!(sliver.apply(&mut elements), 0);
        assert!(!elements[0].tags().contains_key("height"));

        // Absurd tower: same treatment.
        let absurd = hints_with(
            key,
            OsmAttributeHint {
                height_m: Some(4000.0),
                num_floors: None,
            },
        );
        let mut elements = vec![hint_way(42, &[("building", "yes")])];
        assert_eq!(absurd.apply(&mut elements), 0);

        // A single floor adds nothing over the generator's own inference.
        let one_floor = hints_with(
            key,
            OsmAttributeHint {
                height_m: None,
                num_floors: Some(1),
            },
        );
        let mut elements = vec![hint_way(42, &[("building", "yes")])];
        assert_eq!(one_floor.apply(&mut elements), 0);
    }

    #[test]
    fn unusable_rows_do_not_occupy_the_hint_budget() {
        let mut hints = OvertureHints::default();
        // Most Overture rows carry no height or floor count at all.
        hints.insert(
            OsmRef { kind: "way", id: 1 },
            OsmAttributeHint {
                height_m: None,
                num_floors: None,
            },
        );
        // Values the generator would reject are just as useless.
        hints.insert(
            OsmRef { kind: "way", id: 2 },
            OsmAttributeHint {
                height_m: Some(0.4),
                num_floors: Some(1),
            },
        );
        assert_eq!(hints.len(), 0);

        // A row with one usable value is still worth keeping.
        hints.insert(
            OsmRef { kind: "way", id: 3 },
            OsmAttributeHint {
                height_m: Some(0.4),
                num_floors: Some(4),
            },
        );
        assert_eq!(hints.len(), 1);
    }

    #[test]
    fn a_full_map_still_completes_the_buildings_it_tracks() {
        let tracked = OsmRef { kind: "way", id: 1 };
        let mut hints = OvertureHints::default();
        hints.insert_capped(
            tracked,
            OsmAttributeHint {
                height_m: None,
                num_floors: Some(4),
            },
            1,
        );
        assert_eq!(hints.len(), 1);

        // At capacity a new building is refused ...
        hints.insert_capped(
            OsmRef { kind: "way", id: 2 },
            OsmAttributeHint {
                height_m: Some(18.0),
                num_floors: None,
            },
            1,
        );
        assert_eq!(hints.len(), 1);

        // ... but one already tracked can still be completed by a later row.
        hints.insert_capped(
            tracked,
            OsmAttributeHint {
                height_m: Some(18.0),
                num_floors: None,
            },
            1,
        );

        let mut elements = vec![hint_way(1, &[("building", "yes")])];
        assert_eq!(hints.apply(&mut elements), 1);
        assert_eq!(
            elements[0].tags().get("height").map(String::as_str),
            Some("18.0")
        );
        assert_eq!(
            elements[0]
                .tags()
                .get("building:levels")
                .map(String::as_str),
            Some("4")
        );
    }

    #[test]
    fn duplicate_rows_do_not_clobber_a_richer_hint() {
        let key = OsmRef {
            kind: "way",
            id: 42,
        };
        let mut hints = OvertureHints::default();
        hints.insert(
            key,
            OsmAttributeHint {
                height_m: Some(18.0),
                num_floors: Some(6),
            },
        );
        // A second, emptier row for the same building must not erase the first.
        hints.insert(
            key,
            OsmAttributeHint {
                height_m: None,
                num_floors: None,
            },
        );

        let mut elements = vec![hint_way(42, &[("building", "yes")])];
        assert_eq!(hints.apply(&mut elements), 1);
        assert_eq!(
            elements[0].tags().get("height").map(String::as_str),
            Some("18.0")
        );
    }

    #[test]
    fn test_parse_release_listing() {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>overturemaps-us-west-2</Name><Prefix>release/</Prefix><KeyCount>2</KeyCount><MaxKeys>1000</MaxKeys><Delimiter>/</Delimiter><IsTruncated>false</IsTruncated><CommonPrefixes><Prefix>release/2026-06-17.0/</Prefix></CommonPrefixes><CommonPrefixes><Prefix>release/2026-07-22.0/</Prefix></CommonPrefixes></ListBucketResult>"#;

        assert_eq!(
            parse_release_listing(body).unwrap(),
            vec!["2026-07-22.0", "2026-06-17.0"]
        );
    }

    #[test]
    fn test_release_sort_key_orders_revisions_numerically() {
        let mut releases = vec!["2026-07-22.9", "2026-06-17.0", "2026-07-22.10"];
        releases.sort_by(|a, b| release_sort_key(b).cmp(&release_sort_key(a)));
        assert_eq!(
            releases,
            vec!["2026-07-22.10", "2026-07-22.9", "2026-06-17.0"]
        );
    }

    #[test]
    fn test_gers_id_to_u64_high_bit() {
        let id = gers_id_to_u64("08b2a100d2ca5fff0200c4ba4fb6e40a");
        assert!(id & OVERTURE_ID_HIGH_BIT != 0, "High bit must be set");

        // Deterministic
        let id2 = gers_id_to_u64("08b2a100d2ca5fff0200c4ba4fb6e40a");
        assert_eq!(id, id2);

        // Different IDs produce different hashes (probabilistically)
        let id3 = gers_id_to_u64("08b2a100d2ca5fff0200c4ba4fb6e40b");
        assert_ne!(id, id3);
    }

    #[test]
    fn test_overture_class_mapping() {
        assert_eq!(overture_class_to_osm_building(None, Some("house")), "house");
        assert_eq!(
            overture_class_to_osm_building(Some("residential"), None),
            "residential"
        );
        assert_eq!(
            overture_class_to_osm_building(Some("commercial"), Some("retail")),
            "retail" // class takes precedence
        );
        assert_eq!(overture_class_to_osm_building(None, None), "yes");
    }

    #[test]
    fn test_parse_wkb_polygon_le() {
        // A simple WKB polygon: triangle with 4 points (closed ring)
        // Little-endian, Polygon type (3), 1 ring, 4 points
        let mut wkb = Vec::new();
        wkb.push(1u8); // LE
        wkb.extend_from_slice(&3u32.to_le_bytes()); // Polygon
        wkb.extend_from_slice(&1u32.to_le_bytes()); // 1 ring
        wkb.extend_from_slice(&4u32.to_le_bytes()); // 4 points

        // Point 1: (10.0, 20.0)
        wkb.extend_from_slice(&10.0f64.to_le_bytes());
        wkb.extend_from_slice(&20.0f64.to_le_bytes());
        // Point 2: (11.0, 20.0)
        wkb.extend_from_slice(&11.0f64.to_le_bytes());
        wkb.extend_from_slice(&20.0f64.to_le_bytes());
        // Point 3: (11.0, 21.0)
        wkb.extend_from_slice(&11.0f64.to_le_bytes());
        wkb.extend_from_slice(&21.0f64.to_le_bytes());
        // Point 4: (10.0, 20.0) - close ring
        wkb.extend_from_slice(&10.0f64.to_le_bytes());
        wkb.extend_from_slice(&20.0f64.to_le_bytes());

        let coords = parse_wkb_polygon(&wkb).unwrap();
        assert_eq!(coords.len(), 4);
        assert_eq!(coords[0], (10.0, 20.0));
        assert_eq!(coords[1], (11.0, 20.0));
        assert_eq!(coords[2], (11.0, 21.0));
        assert_eq!(coords[3], (10.0, 20.0));
    }

    #[test]
    fn test_parse_wkb_not_polygon() {
        // WKB Point (type 1)
        let mut wkb = Vec::new();
        wkb.push(1u8);
        wkb.extend_from_slice(&1u32.to_le_bytes()); // Point type
        wkb.extend_from_slice(&10.0f64.to_le_bytes());
        wkb.extend_from_slice(&20.0f64.to_le_bytes());

        assert!(parse_wkb_polygon(&wkb).is_none());
    }

    #[test]
    fn test_parse_wkb_too_short() {
        assert!(parse_wkb_polygon(&[]).is_none());
        assert!(parse_wkb_polygon(&[1, 2, 3]).is_none());
    }

    #[test]
    fn test_parse_wkb_polygon_3d() {
        // WKB Polygon Z (type 1003 in ISO WKB): triangle with Z coordinates
        let mut wkb = Vec::new();
        wkb.push(1u8); // LE
        wkb.extend_from_slice(&1003u32.to_le_bytes()); // Polygon Z
        wkb.extend_from_slice(&1u32.to_le_bytes()); // 1 ring
        wkb.extend_from_slice(&4u32.to_le_bytes()); // 4 points

        // Point 1: (10.0, 20.0, 100.0)
        wkb.extend_from_slice(&10.0f64.to_le_bytes());
        wkb.extend_from_slice(&20.0f64.to_le_bytes());
        wkb.extend_from_slice(&100.0f64.to_le_bytes());
        // Point 2: (11.0, 20.0, 100.0)
        wkb.extend_from_slice(&11.0f64.to_le_bytes());
        wkb.extend_from_slice(&20.0f64.to_le_bytes());
        wkb.extend_from_slice(&100.0f64.to_le_bytes());
        // Point 3: (11.0, 21.0, 100.0)
        wkb.extend_from_slice(&11.0f64.to_le_bytes());
        wkb.extend_from_slice(&21.0f64.to_le_bytes());
        wkb.extend_from_slice(&100.0f64.to_le_bytes());
        // Point 4: (10.0, 20.0, 100.0) - close ring
        wkb.extend_from_slice(&10.0f64.to_le_bytes());
        wkb.extend_from_slice(&20.0f64.to_le_bytes());
        wkb.extend_from_slice(&100.0f64.to_le_bytes());

        let coords = parse_wkb_polygon(&wkb).unwrap();
        assert_eq!(coords.len(), 4);
        assert_eq!(coords[0], (10.0, 20.0)); // Z is ignored
        assert_eq!(coords[1], (11.0, 20.0));
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    // A small city keeps the old flat allowance.
    #[test]
    fn a_small_area_gets_the_floor() {
        let bbox = LLBBox::new(41.49, -81.70, 41.52, -81.65).unwrap();
        assert_eq!(overture_building_budget(&bbox), MIN_OVERTURE_BUILDINGS);
    }

    // Between the floor and the ceiling the budget must track the rate, not sit on
    // a clamp. A flat 100k was already spent before a mid-size metro was finished,
    // which is what leaves the districts read last with no footprints (issue #1257).
    #[test]
    fn a_mid_size_area_tracks_the_per_km2_rate() {
        // ~0.15 deg lat by ~0.20 deg lng at 41.5 deg N, about 280 km²: inside the
        // floor and the ceiling, which meet at 100 km² and 500 km² respectively.
        let bbox = LLBBox::new(41.40, -81.80, 41.55, -81.60).unwrap();
        let budget = overture_building_budget(&bbox);
        assert!(
            budget > MIN_OVERTURE_BUILDINGS && budget < MAX_OVERTURE_BUILDINGS,
            "budget {budget} sat on a clamp instead of tracking the rate"
        );
        let expected = (bbox.area_km2() * OVERTURE_BUILDINGS_PER_KM2) as usize;
        assert_eq!(budget, expected);
    }

    // The cap still exists: a continent must not be allowed to exhaust memory.
    #[test]
    fn a_continent_is_clamped_to_the_ceiling() {
        let bbox = LLBBox::new(30.0, -120.0, 50.0, -80.0).unwrap();
        assert_eq!(overture_building_budget(&bbox), MAX_OVERTURE_BUILDINGS);
    }
}
