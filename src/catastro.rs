//! Enriches OSM building heights with real floor counts from the Spanish
//! Cadastre (Catastro) INSPIRE Buildings WFS.
//!
//! Arnis derives building height from the OSM `building:levels`/`height` tags
//! and falls back to a fixed default (2 floors) when neither is present. In
//! Spain most OSM buildings lack those tags, so whole cities come out as
//! identical flat boxes. The Catastro publishes `numberOfFloorsAboveGround`
//! for nearly every building in the country via a free INSPIRE WFS service.
//!
//! This module fetches those floor counts for the generation bbox, matches
//! each Catastro `BuildingPart` polygon to the OSM building it overlaps most,
//! and injects a `building:levels` tag into the raw OSM data before parsing —
//! so all of Arnis' downstream logic (heights, roofs, windows) just works.
//!
//! Spain only (the Basque Country and Navarra keep their own cadastre and are
//! not served here). Enabled with `--catastro`.

use std::collections::HashMap;
use std::time::Duration;

use colored::Colorize;
use geo::{Area, BooleanOps, Coord, LineString, Polygon};

use crate::coordinate_system::geographic::LLBBox;
use crate::osm_parser::OsmData;

const WFS_URL: &str = "https://ovc.catastro.meh.es/INSPIRE/wfsBU.aspx";
/// If a single WFS request returns at least this many parts, the service is
/// likely truncating the response, so we subdivide the bbox.
const SPLIT_THRESHOLD: usize = 4800;
/// Maximum quadtree subdivision depth (guards against runaway recursion).
const MAX_DEPTH: u32 = 6;
/// Initial grid used to avoid issuing one huge request the WFS can't serve.
const GRID_NX: usize = 6;
const GRID_NY: usize = 6;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
/// A match must cover at least this fraction of the OSM footprint to count.
const MIN_OVERLAP_FRAC: f64 = 0.10;

/// A Catastro building part: its footprint (x=lon, y=lat) and floor count.
struct CatastroPart {
    poly: Polygon<f64>,
    floors: u32,
    // Axis-aligned bounding box, cached for a cheap pre-filter before the
    // expensive polygon intersection.
    min_lon: f64,
    min_lat: f64,
    max_lon: f64,
    max_lat: f64,
}

/// (min_lat, min_lon, max_lat, max_lon)
type Bbox = (f64, f64, f64, f64);

/// Entry point: fetch Catastro floor counts for `bbox` and inject
/// `building:levels` into OSM buildings that lack a height. Mutates `data` in
/// place. Network or parse failures degrade gracefully (the world is still
/// generated with Arnis' default heights).
pub fn enrich_building_heights(data: &mut OsmData, bbox: LLBBox) {
    println!("{} Fetching Catastro building heights...", "  [+]".bold());

    let area: Bbox = (
        bbox.min().lat(),
        bbox.min().lng(),
        bbox.max().lat(),
        bbox.max().lng(),
    );

    let client = match reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent("arnis (+https://github.com/louis-e/arnis)")
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  Catastro: could not build HTTP client: {e}");
            return;
        }
    };

    let parts = collect_grid(&client, area);
    if parts.is_empty() {
        println!("  No Catastro data for this area (outside Spain, or service unavailable)");
        return;
    }
    println!(
        "  Retrieved {} Catastro building parts with floor data",
        parts.len().to_string().bright_white().bold()
    );

    let stats = match_and_enrich(data, &parts);
    let targetable = stats.total.saturating_sub(stats.had_height);
    let coverage = if targetable > 0 {
        stats.enriched as f64 / targetable as f64 * 100.0
    } else {
        0.0
    };
    println!(
        "  Catastro heights: {} enriched, {} already had height, {} unmatched ({:.1}% coverage)",
        stats.enriched.to_string().bright_white().bold(),
        stats.had_height,
        stats.no_match,
        coverage,
    );
}

// --------------------------------------------------------------------------- //
// WFS download (initial grid + quadtree fallback)
// --------------------------------------------------------------------------- //

fn collect_grid(client: &reqwest::blocking::Client, bbox: Bbox) -> Vec<CatastroPart> {
    let (min_lat, min_lon, max_lat, max_lon) = bbox;
    let dlat = (max_lat - min_lat) / GRID_NY as f64;
    let dlon = (max_lon - min_lon) / GRID_NX as f64;
    let mut out = Vec::new();
    for i in 0..GRID_NY {
        for j in 0..GRID_NX {
            let tile = (
                min_lat + i as f64 * dlat,
                min_lon + j as f64 * dlon,
                min_lat + (i + 1) as f64 * dlat,
                min_lon + (j + 1) as f64 * dlon,
            );
            collect_recursive(client, tile, 0, &mut out);
        }
    }
    out
}

fn collect_recursive(
    client: &reqwest::blocking::Client,
    bbox: Bbox,
    depth: u32,
    out: &mut Vec<CatastroPart>,
) {
    let (truncated, fetched) = match fetch_bbox(client, bbox) {
        Ok(xml) => {
            let parts = parse_parts(&xml);
            let truncated = parts.len() >= SPLIT_THRESHOLD;
            (truncated, Some(parts))
        }
        // A failure (often a timeout because the tile is too dense) → subdivide.
        Err(_) => (true, None),
    };

    if truncated && depth < MAX_DEPTH {
        for quad in split(bbox) {
            collect_recursive(client, quad, depth + 1, out);
        }
    } else if let Some(parts) = fetched {
        out.extend(parts);
    }
}

fn split(bbox: Bbox) -> [Bbox; 4] {
    let (min_lat, min_lon, max_lat, max_lon) = bbox;
    let mid_lat = (min_lat + max_lat) / 2.0;
    let mid_lon = (min_lon + max_lon) / 2.0;
    [
        (min_lat, min_lon, mid_lat, mid_lon),
        (min_lat, mid_lon, mid_lat, max_lon),
        (mid_lat, min_lon, max_lat, mid_lon),
        (mid_lat, mid_lon, max_lat, max_lon),
    ]
}

fn fetch_bbox(client: &reqwest::blocking::Client, bbox: Bbox) -> Result<String, reqwest::Error> {
    let (min_lat, min_lon, max_lat, max_lon) = bbox;
    let bbox_param = format!("{min_lat},{min_lon},{max_lat},{max_lon},urn:ogc:def:crs:EPSG::4326");
    let text = client
        .get(WFS_URL)
        .query(&[
            ("service", "WFS"),
            ("version", "2.0.0"),
            ("request", "GetFeature"),
            ("typenames", "bu:BuildingPart"),
            ("srsname", "urn:ogc:def:crs:EPSG::4326"),
            ("bbox", &bbox_param),
        ])
        .send()?
        .error_for_status()?
        .text()?;
    Ok(text)
}

// --------------------------------------------------------------------------- //
// GML parsing
// --------------------------------------------------------------------------- //

/// Parse a WFS GML response into building parts that carry a floor count.
fn parse_parts(xml: &str) -> Vec<CatastroPart> {
    let doc = match roxmltree::Document::parse(xml) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    let mut parts = Vec::new();
    for node in doc.descendants().filter(|n| n.has_tag_name("BuildingPart")) {
        let mut floors: Option<u32> = None;
        // Collect candidate rings; the exterior ring is the one with the
        // largest area (interior rings are courtyards/holes).
        let mut rings: Vec<Polygon<f64>> = Vec::new();

        for sub in node.descendants() {
            match sub.tag_name().name() {
                "numberOfFloorsAboveGround" => {
                    if let Some(txt) = sub.text() {
                        floors = txt.trim().parse::<u32>().ok();
                    }
                }
                "posList" => {
                    if let Some(txt) = sub.text() {
                        if let Some(poly) = poslist_to_polygon(txt) {
                            rings.push(poly);
                        }
                    }
                }
                _ => {}
            }
        }

        if let (Some(f), false) = (floors, rings.is_empty()) {
            if f >= 1 {
                // Exterior ring = largest area.
                let mut best = rings.swap_remove(0);
                let mut best_area = best.unsigned_area();
                for r in rings {
                    let a = r.unsigned_area();
                    if a > best_area {
                        best_area = a;
                        best = r;
                    }
                }
                let (min_lon, min_lat, max_lon, max_lat) = polygon_bounds(&best);
                parts.push(CatastroPart {
                    poly: best,
                    floors: f,
                    min_lon,
                    min_lat,
                    max_lon,
                    max_lat,
                });
            }
        }
    }
    parts
}

/// A GML posList is whitespace-separated `lat lon lat lon ...`. Build a polygon
/// in (x=lon, y=lat) order.
fn poslist_to_polygon(text: &str) -> Option<Polygon<f64>> {
    let nums: Vec<f64> = text
        .split_whitespace()
        .filter_map(|t| t.parse::<f64>().ok())
        .collect();
    if nums.len() < 8 {
        return None; // fewer than 4 coordinate pairs → not a ring
    }
    let mut coords: Vec<Coord<f64>> = Vec::with_capacity(nums.len() / 2);
    let mut i = 0;
    while i + 1 < nums.len() {
        coords.push(Coord {
            x: nums[i + 1],
            y: nums[i],
        });
        i += 2;
    }
    Some(Polygon::new(LineString::new(coords), vec![]))
}

fn polygon_bounds(poly: &Polygon<f64>) -> (f64, f64, f64, f64) {
    let mut min_lon = f64::INFINITY;
    let mut min_lat = f64::INFINITY;
    let mut max_lon = f64::NEG_INFINITY;
    let mut max_lat = f64::NEG_INFINITY;
    for c in poly.exterior().coords() {
        min_lon = min_lon.min(c.x);
        max_lon = max_lon.max(c.x);
        min_lat = min_lat.min(c.y);
        max_lat = max_lat.max(c.y);
    }
    (min_lon, min_lat, max_lon, max_lat)
}

// --------------------------------------------------------------------------- //
// Matching and injection
// --------------------------------------------------------------------------- //

#[derive(Default)]
struct Stats {
    total: usize,
    had_height: usize,
    enriched: usize,
    no_match: usize,
}

fn is_building(tags: &HashMap<String, String>) -> bool {
    matches!(tags.get("building"), Some(v) if v != "no") || tags.contains_key("building:part")
}

fn has_height(tags: &HashMap<String, String>) -> bool {
    tags.contains_key("building:levels") || tags.contains_key("height")
}

fn match_and_enrich(data: &mut OsmData, parts: &[CatastroPart]) -> Stats {
    // node id -> (lon, lat)
    let mut coords: HashMap<u64, (f64, f64)> = HashMap::new();
    for el in &data.elements {
        if el.r#type == "node" {
            if let (Some(lat), Some(lon)) = (el.lat, el.lon) {
                coords.insert(el.id, (lon, lat));
            }
        }
    }

    let mut stats = Stats::default();

    for el in &mut data.elements {
        if el.r#type != "way" {
            continue;
        }
        let Some(tags) = el.tags.as_mut() else {
            continue;
        };
        if !is_building(tags) {
            continue;
        }
        stats.total += 1;
        if has_height(tags) {
            stats.had_height += 1;
            continue;
        }
        let Some(nodes) = el.nodes.as_ref() else {
            stats.no_match += 1;
            continue;
        };
        let Some(poly) = way_polygon(nodes, &coords) else {
            stats.no_match += 1;
            continue;
        };
        let (b_min_lon, b_min_lat, b_max_lon, b_max_lat) = polygon_bounds(&poly);
        let poly_area = poly.unsigned_area();
        if poly_area <= 0.0 {
            stats.no_match += 1;
            continue;
        }

        let mut best_floors: Option<u32> = None;
        let mut best_overlap = 0.0_f64;
        for part in parts {
            // Cheap bbox rejection before the expensive boolean op.
            if part.max_lon < b_min_lon
                || part.min_lon > b_max_lon
                || part.max_lat < b_min_lat
                || part.min_lat > b_max_lat
            {
                continue;
            }
            let overlap = poly.intersection(&part.poly).unsigned_area();
            if overlap > best_overlap {
                best_overlap = overlap;
                best_floors = Some(part.floors);
            }
        }

        match best_floors {
            Some(f) if best_overlap >= poly_area * MIN_OVERLAP_FRAC => {
                tags.insert("building:levels".to_string(), f.to_string());
                stats.enriched += 1;
            }
            _ => stats.no_match += 1,
        }
    }

    stats
}

fn way_polygon(nodes: &[u64], coords: &HashMap<u64, (f64, f64)>) -> Option<Polygon<f64>> {
    let mut pts: Vec<Coord<f64>> = nodes
        .iter()
        .filter_map(|id| coords.get(id))
        .map(|&(lon, lat)| Coord { x: lon, y: lat })
        .collect();
    if pts.len() < 3 {
        return None;
    }
    if pts.first() != pts.last() {
        let first = pts[0];
        pts.push(first);
    }
    Some(Polygon::new(LineString::new(pts), vec![]))
}
