//! Overture Maps building data integration.
//!
//! Fetches ML-derived building footprints from Overture Maps to complement
//! OpenStreetMap data. Only buildings NOT sourced from OSM are included,
//! filling gaps in areas with sparse OSM coverage (e.g., rural Africa,
//! parts of Asia).
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

/// Overture STAC catalog URL for the collections.parquet index (~230 KB).
/// Contains bbox + asset URLs for every partition of every theme/type.
const OVERTURE_STAC_URL: &str = "https://stac.overturemaps.org/2026-03-18.0/collections.parquet";

/// High bit marker for Overture IDs to avoid collision with OSM IDs.
/// OSM IDs are sequential positive u64 (currently up to ~12 billion, well under 2^34).
/// Setting bit 63 guarantees no collision.
const OVERTURE_ID_HIGH_BIT: u64 = 0x8000_0000_0000_0000;

/// Maximum number of Overture buildings to add (safety limit for huge areas)
const MAX_OVERTURE_BUILDINGS: usize = 100_000;

/// HTTP client timeout for Overture data fetching
const HTTP_TIMEOUT_SECS: u64 = 120;

// ─── Internal data types ─────────────────────────────────────────────────

/// A building parsed from Overture Maps GeoParquet data.
struct OvertureBuilding {
    /// GERS ID (UUID string)
    id: String,
    /// Exterior ring coordinates as (longitude, latitude) pairs
    exterior_ring: Vec<(f64, f64)>,
    /// Whether the primary source is OpenStreetMap
    is_osm_sourced: bool,
    /// Building height in meters (if available)
    height: Option<f64>,
    /// Minimum height in meters (bottom of building, for elevated parts)
    min_height: Option<f64>,
    /// Number of above-ground floors (if available)
    num_floors: Option<i32>,
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

/// Fetch non-OSM building footprints from Overture Maps for the given bbox.
///
/// Returns `ProcessedWay` elements with OSM-compatible tags, ready to merge
/// with the main element list. Returns an empty Vec on any failure (non-fatal).
///
/// Buildings whose primary source is "OpenStreetMap" are excluded to avoid
/// duplicates with the existing OSM data pipeline.
pub fn fetch_overture_buildings(bbox: &LLBBox, scale: f64, debug: bool) -> Vec<ProcessedElement> {
    match fetch_overture_buildings_inner(bbox, scale, debug) {
        Ok(elements) => elements,
        Err(e) => {
            eprintln!(
                "{} Failed to fetch Overture Maps data: {e}",
                "Warning:".yellow().bold()
            );
            Vec::new()
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

fn fetch_overture_buildings_inner(
    bbox: &LLBBox,
    scale: f64,
    debug: bool,
) -> Result<Vec<ProcessedElement>, Box<dyn std::error::Error>> {
    let client = Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .user_agent(concat!("arnis/", env!("CARGO_PKG_VERSION")))
        .build()?;

    emit_gui_progress_update(14.5, "Fetching Overture Maps data...");

    // List partition files whose geographic bounds overlap our bbox
    // (single ~230 KB STAC download instead of 512 HTTP requests)
    let partition_urls = list_partition_files(&client, bbox, debug)?;
    if partition_urls.is_empty() {
        if debug {
            println!("No Overture partitions overlap the bbox");
        }
        return Ok(Vec::new());
    }

    if debug {
        println!(
            "Found {} Overture partition(s) for this area",
            partition_urls.len()
        );
    }

    // Process each partition file: read footer, check for bbox overlap, fetch matching rows
    let mut all_buildings: Vec<OvertureBuilding> = Vec::new();
    let mut non_osm_count: usize = 0;

    for (i, url) in partition_urls.iter().enumerate() {
        if non_osm_count >= MAX_OVERTURE_BUILDINGS {
            if debug {
                println!("Reached building limit ({MAX_OVERTURE_BUILDINGS}), stopping");
            }
            break;
        }

        if debug && i % 10 == 0 {
            println!(
                "Processing partition {}/{} ...",
                i + 1,
                partition_urls.len()
            );
        }

        match process_partition_file(&client, url, bbox, debug) {
            Ok(buildings) => {
                non_osm_count += buildings.iter().filter(|b| !b.is_osm_sourced).count();
                all_buildings.extend(buildings.into_iter().filter(|b| !b.is_osm_sourced));
            }
            Err(e) => {
                if debug {
                    eprintln!("Warning: Failed to process partition {url}: {e}");
                }
                // Continue with other partitions
            }
        }
    }

    if debug {
        println!("Overture: {} non-OSM buildings found", all_buildings.len());
    }

    // Convert to ProcessedElements and clip to xzbbox (matching OSM clipping)
    let (coord_transformer, xzbbox) = CoordTransformer::llbbox_to_xzbbox(bbox, scale)?;

    let elements: Vec<ProcessedElement> = all_buildings
        .into_iter()
        .take(MAX_OVERTURE_BUILDINGS)
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

    Ok(elements)
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
    // Download the small STAC collections index
    let response = client.get(OVERTURE_STAC_URL).send()?;
    if !response.status().is_success() {
        return Err(format!(
            "STAC catalog download failed with status {}",
            response.status()
        )
        .into());
    }

    let stac_bytes = response.bytes()?;
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
fn process_partition_file(
    client: &Client,
    url: &str,
    bbox: &LLBBox,
    debug: bool,
) -> Result<Vec<OvertureBuilding>, Box<dyn std::error::Error>> {
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
        return Ok(Vec::new());
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
    for &rg_idx in &matching_groups {
        let (rg_offset, rg_len) = row_group_byte_range(&metadata, rg_idx);
        match fetch_range(client, url, rg_offset, rg_len) {
            Ok(rg_data) => {
                downloaded_bytes += rg_len;
                sparse.add_range(rg_offset, bytes::Bytes::from(rg_data));
            }
            Err(e) => {
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
                if debug {
                    eprintln!("Warning: Failed to parse row group {rg_idx}: {e}");
                }
            }
        }
    }

    Ok(buildings)
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

/// Fetch a byte range from a URL via HTTP Range request.
fn fetch_range(
    client: &Client,
    url: &str,
    start: u64,
    length: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if length == 0 {
        return Err("fetch_range called with length 0".into());
    }
    let end = start + length - 1;
    let response = client
        .get(url)
        .header("Range", format!("bytes={start}-{end}"))
        .send()?;

    let status = response.status();
    if status.as_u16() != 206 {
        return Err(format!("HTTP {status} fetching range from {url} (expected 206)").into());
    }

    Ok(response.bytes()?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

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
