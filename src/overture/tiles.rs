//! Overture buildings read from the published PMTiles vector tiles.
//!
//! Overture ships every release twice: as GeoParquet partitions (512 files,
//! ~277 GB) and as a single planet PMTiles archive per theme in the
//! `overturemaps-extras` bucket. Both carry the same buildings and the same
//! attributes; they differ in what it costs to read a city out of them.
//!
//! The Parquet path must download a ~233 KB catalogue, then a ~1.3 MB footer
//! per overlapping partition, before a single building is read. The tile path
//! reads a 16 KB header, one directory page, and then only the z14 tiles the
//! bounding box actually covers - about 1.3 MB for a whole city, and nothing at
//! all on a repeat run, because tiles are keyed by an immutable release and
//! cached on disk.
//!
//! ## What the tiles cost in fidelity
//!
//! Tiles quantise coordinates to the z14 lattice: 4096 steps across a tile,
//! which is 0.40 m at 48° latitude and 0.60 m at the equator. Measured against
//! the OSM originals for the same buildings, the resulting geometry error is a
//! median of 0.09 m and a maximum of 0.24 m - under a third of a block at
//! `--scale 1.0`. Vertices dropped by the tile build are collinear ones, which
//! cost nothing. No minimum-area filter runs at the archive's maximum zoom, so
//! footprints down to 0.1 m² are present.
//!
//! ## Where this path deliberately differs from Parquet
//!
//! A feature whose geometry is a multipolygon contributes its largest exterior
//! ring. The Parquet reader drops such a row outright (`parse_wkb_polygon`
//! handles WKB type 3 only), which loses the building; taking the largest part
//! keeps one footprint per Overture row either way.

use std::collections::HashMap;

use rayon::prelude::*;
use reqwest::blocking::Client;

use super::cache;
use super::mvt;
use super::pmtiles::{self, Archive};
use super::{
    intern_facade_material, intern_roof_material, intern_roof_orientation, intern_roof_shape,
    OsmAttributeHint, OsmRef, OvertureBuilding, OvertureCollection,
};
use crate::coordinate_system::geographic::LLBBox;
use colored::Colorize;

/// Bucket holding the pre-built vector tiles for each release.
const OVERTURE_TILES_BUCKET: &str =
    "https://overturemaps-extras-us-west-2.s3.us-west-2.amazonaws.com";

/// Theme to read. Buildings is the only one Arnis consumes from Overture.
const TILES_THEME: &str = "buildings";

/// Layer inside the archive.
///
/// The archive also carries a `building_part` layer, which the Parquet path has
/// no equivalent for. Reading it would add footprints that the other provider
/// cannot produce, so it is deliberately left alone: swapping the transport must
/// not change what a world contains.
const BUILDING_LAYER: &str = "building";

/// Zoom to read. The archive's maximum, and therefore its full resolution;
/// anything lower is a generalised rendering of the same data.
const QUERY_ZOOM: u8 = 14;

/// Past this many tiles the archive stops being the cheaper option: a
/// continental bounding box is better served by the Parquet path, which reads
/// whole row groups covering whole regions rather than one request per square
/// kilometre. At mid latitudes this is roughly 8,700 km².
pub(super) const MAX_TILES: usize = 4096;

/// Whether this bounding box is small enough for the tile path to be the
/// cheaper transport. Checked before any release is tried, so a continental
/// request goes straight to Parquet instead of failing once per release.
pub(super) fn covers_area(bbox: &LLBBox) -> bool {
    tiles_for_bbox(bbox).len() <= MAX_TILES
}

/// Concurrent range requests. Enough to keep the link busy, few enough that a
/// large area cannot exhaust the process's sockets or look like an attack to
/// the bucket.
const FETCH_THREADS: usize = 8;

/// One pool for the process, not one per fetch: the 3D preview re-fetches on
/// every pan, and spawning eight threads each time would cost more than the
/// requests do. Kept separate from the global Rayon pool so that blocking on
/// HTTP cannot starve the generation work sharing it.
fn fetch_pool() -> Result<&'static rayon::ThreadPool> {
    static POOL: std::sync::OnceLock<std::result::Result<rayon::ThreadPool, String>> =
        std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(FETCH_THREADS)
            .thread_name(|i| format!("overture-tiles-{i}"))
            .build()
            .map_err(|e| format!("could not start the tile fetch pool: {e}"))
    })
    .as_ref()
    .map_err(String::clone)
}

pub type Result<T> = std::result::Result<T, String>;

/// URL of the buildings archive for one release.
fn archive_url(release: &str) -> String {
    format!("{OVERTURE_TILES_BUCKET}/tiles/{release}/{TILES_THEME}.pmtiles")
}

/// Cache directory for one release's tile artefacts, or `None` when the release
/// name is not one we will build a path from.
fn archive_cache_dir(release: &str) -> Option<std::path::PathBuf> {
    cache::release_dir(release).map(|d| d.join("tiles").join(TILES_THEME))
}

/// Every z14 tile the bounding box touches.
fn tiles_for_bbox(bbox: &LLBBox) -> Vec<(u32, u32)> {
    let (min_x, min_y) = pmtiles::lonlat_to_tile(bbox.min().lng(), bbox.max().lat(), QUERY_ZOOM);
    let (max_x, max_y) = pmtiles::lonlat_to_tile(bbox.max().lng(), bbox.min().lat(), QUERY_ZOOM);
    let mut tiles = Vec::new();
    for x in min_x..=max_x {
        for y in min_y..=max_y {
            tiles.push((x, y));
        }
    }
    tiles
}

/// Tiles fetched per round. The budget is checked between rounds, so this also
/// bounds how far past it a dense area can read: at most this many tiles' worth
/// of footprints beyond the cap.
const TILE_BATCH: usize = 64;

/// What one tile yielded: footprints to keep, and attribute hints harvested from
/// the OSM-sourced rows whose geometry is not kept.
#[derive(Default)]
struct TileHarvest {
    buildings: Vec<TileBuilding>,
    hints: Vec<(OsmRef, OsmAttributeHint)>,
}

/// A building as it came out of one tile, before duplicates across tile buffers
/// are resolved.
struct TileBuilding {
    /// GERS id, which is stable across tiles and is what deduplication keys on.
    gers_id: String,
    building: OvertureBuilding,
    /// Vertices in the ring before it was converted. A polygon clipped by a tile
    /// boundary keeps fewer of them than the same polygon carried whole inside a
    /// neighbouring tile's buffer, so this picks the intact copy.
    vertex_count: usize,
}

/// Pull the OSM back-reference out of the tiles' `sources` attribute.
///
/// In a tile this is the same structure the Parquet column holds, serialised as
/// a JSON array: `[{"dataset":"OpenStreetMap","record_id":"w106766186@8",...}]`.
fn parse_sources(sources: &str) -> Option<OsmRef> {
    let parsed: serde_json::Value = serde_json::from_str(sources).ok()?;
    for entry in parsed.as_array()? {
        // An entry missing `dataset` is skipped, not fatal: bailing out here
        // would let one unrecognised source hide the OpenStreetMap entry behind
        // it, and silently cost the height enrichment for that building.
        let Some(dataset) = entry.get("dataset").and_then(|v| v.as_str()) else {
            continue;
        };
        if !dataset.eq_ignore_ascii_case("OpenStreetMap") {
            continue;
        }
        if let Some(reference) = entry
            .get("record_id")
            .and_then(|v| v.as_str())
            .and_then(super::parse_record_id)
        {
            return Some(reference);
        }
    }
    None
}

/// Convert one tile feature into a building, in geographic coordinates.
///
/// Returns `None` for a feature that is not a polygon, has no id, or whose
/// rings are all degenerate.
fn feature_to_building(
    layer: &mvt::Layer,
    feature: &mvt::Feature,
    tile_x: u32,
    tile_y: u32,
) -> Option<TileBuilding> {
    if feature.geom_type != mvt::GEOM_POLYGON {
        return None;
    }
    let gers_id = layer.attr_str(feature, "id")?.to_string();

    // Largest exterior ring; holes are dropped, as they are on the Parquet path.
    let ring = feature
        .rings
        .iter()
        .filter(|r| r.exterior && r.points.len() >= 3)
        .max_by_key(|r| r.points.len())?;

    let extent = f64::from(layer.extent);
    let exterior_ring: Vec<(f64, f64)> = ring
        .points
        .iter()
        .map(|&(x, y)| {
            (
                pmtiles::tile_x_to_lon(QUERY_ZOOM, tile_x, f64::from(x) / extent),
                pmtiles::tile_y_to_lat(QUERY_ZOOM, tile_y, f64::from(y) / extent),
            )
        })
        .collect();

    let sources = layer.attr_str(feature, "sources");
    let osm_ref = sources.and_then(parse_sources);
    // `@geometry_source` states the primary source directly, which is exactly
    // what the Parquet path infers by scanning the sources struct for the same
    // name. Either answering yes is enough: misclassifying a building as
    // OSM-sourced costs a duplicate footprint that OSM already provides, while
    // missing one adds a second copy of a building the world already has.
    let is_osm_sourced = layer
        .attr_str(feature, "@geometry_source")
        .is_some_and(|s| s.eq_ignore_ascii_case("OpenStreetMap"))
        || sources.is_some_and(|s| s.contains("OpenStreetMap"));

    Some(TileBuilding {
        vertex_count: ring.points.len(),
        gers_id: gers_id.clone(),
        building: OvertureBuilding {
            id: gers_id,
            exterior_ring,
            is_osm_sourced,
            osm_ref,
            height: layer.attr_f64(feature, "height"),
            min_height: layer.attr_f64(feature, "min_height"),
            num_floors: layer
                .attr_f64(feature, "num_floors")
                .filter(|f| f.is_finite() && (i32::MIN as f64..=i32::MAX as f64).contains(f))
                .map(|f| f as i32),
            subtype: layer.attr_str(feature, "subtype").map(str::to_owned),
            class: layer.attr_str(feature, "class").map(str::to_owned),
            roof_shape: layer
                .attr_str(feature, "roof_shape")
                .and_then(intern_roof_shape),
            roof_material: layer
                .attr_str(feature, "roof_material")
                .and_then(intern_roof_material),
            roof_orientation: layer
                .attr_str(feature, "roof_orientation")
                .and_then(intern_roof_orientation),
            facade_color: layer.attr_str(feature, "facade_color").map(str::to_owned),
            roof_color: layer.attr_str(feature, "roof_color").map(str::to_owned),
            roof_height: layer.attr_f64(feature, "roof_height"),
            facade_material: layer
                .attr_str(feature, "facade_material")
                .and_then(intern_facade_material),
        },
    })
}

/// Whether a footprint's own extent reaches the requested area.
///
/// Tiles carry a buffer of neighbouring geometry so that a polygon crossing a
/// tile edge can still be drawn whole, which means a tile routinely contains
/// buildings well outside it. Those are dropped here rather than carried through
/// the coordinate transform.
fn ring_overlaps_bbox(ring: &[(f64, f64)], bbox: &LLBBox) -> bool {
    let (mut min_lng, mut max_lng) = (f64::MAX, f64::MIN);
    let (mut min_lat, mut max_lat) = (f64::MAX, f64::MIN);
    for &(lng, lat) in ring {
        min_lng = min_lng.min(lng);
        max_lng = max_lng.max(lng);
        min_lat = min_lat.min(lat);
        max_lat = max_lat.max(lat);
    }
    max_lng >= bbox.min().lng()
        && min_lng <= bbox.max().lng()
        && max_lat >= bbox.min().lat()
        && min_lat <= bbox.max().lat()
}

/// Read every Overture building overlapping `bbox` from the tile archive.
///
/// Mirrors [`super::collect_overture_buildings`]: OSM-sourced rows are mined for
/// height hints and then dropped unless `include_osm_sourced`, and the result is
/// capped at `max_buildings`.
pub fn collect_from_tiles(
    client: &Client,
    bbox: &LLBBox,
    release: &str,
    include_osm_sourced: bool,
    max_buildings: usize,
    report_gaps: bool,
    debug: bool,
) -> Result<OvertureCollection> {
    let tiles = tiles_for_bbox(bbox);
    if tiles.len() > MAX_TILES {
        return Err(format!(
            "area needs {} tiles at zoom {QUERY_ZOOM}, past the {MAX_TILES} this path is \
             cheaper for",
            tiles.len()
        ));
    }
    if tiles.is_empty() {
        return Ok(OvertureCollection::default());
    }

    let url = archive_url(release);
    let mut archive = Archive::open(client, &url, archive_cache_dir(release))?;
    let (min_zoom, max_zoom) = (archive.header().min_zoom, archive.header().max_zoom);
    if !(min_zoom..=max_zoom).contains(&QUERY_ZOOM) {
        return Err(format!(
            "archive covers zoom {min_zoom}..={max_zoom}, which does not include the \
             {QUERY_ZOOM} this reader needs"
        ));
    }

    // Resolving is serial because it memoises leaf directories - a city sits
    // behind one or two, so this is a couple of requests and then a lookup per
    // tile. The bodies are fetched in parallel afterwards.
    let mut located: Vec<(u32, u32, pmtiles::TileLocation)> = Vec::new();
    for &(x, y) in &tiles {
        match archive.locate(client, QUERY_ZOOM, x, y)? {
            // A tile the archive does not hold is open water or empty land, not
            // an error: there are simply no buildings there.
            None => continue,
            Some(location) => located.push((x, y, location)),
        }
    }

    if debug {
        println!(
            "Overture tiles: {} tile(s) cover the bbox, {} hold data",
            tiles.len(),
            located.len()
        );
    }
    let pool = fetch_pool()?;
    let archive = &archive;

    // Deduplicate across tile buffers. The same building appears in every tile
    // whose buffer reaches it, sometimes clipped; the copy with the most
    // vertices is the one that was carried whole.
    let mut best: HashMap<String, TileBuilding> = HashMap::new();
    let mut hints = super::OvertureHints::default();
    let mut lost_tiles = 0usize;
    let mut capped = false;

    // Tiles are fetched a batch at a time rather than all at once, so the
    // building budget can stop the fetch. Reading every tile first would hold
    // the whole area's footprints in memory before the cap was ever consulted,
    // which for a dense metropolitan bbox is millions of polygons.
    for batch in located.chunks(TILE_BATCH) {
        let results: Vec<std::result::Result<TileHarvest, String>> = pool.install(|| {
            batch
                .par_iter()
                .map(|&(x, y, location)| {
                    let raw = archive.tile(client, QUERY_ZOOM, x, y, location)?;
                    if raw.is_empty() {
                        return Ok(TileHarvest::default());
                    }
                    let layers = mvt::decode_tile(&raw)?;
                    let mut harvest = TileHarvest::default();
                    for layer in layers.iter().filter(|l| l.name == BUILDING_LAYER) {
                        for feature in &layer.features {
                            let Some(candidate) = feature_to_building(layer, feature, x, y) else {
                                continue;
                            };
                            if !ring_overlaps_bbox(&candidate.building.exterior_ring, bbox) {
                                continue;
                            }
                            if candidate.building.is_osm_sourced {
                                // An OSM-sourced row is a duplicate footprint
                                // that still carries conflated Microsoft / Esri
                                // / 3DEP values. Unless the caller wants the
                                // geometry too, only those values are kept -
                                // in a European city they are 99% of the rows,
                                // so carrying their rings would dominate memory
                                // for data that is then thrown away.
                                if let Some(key) = candidate.building.osm_ref {
                                    harvest.hints.push((
                                        key,
                                        OsmAttributeHint {
                                            height_m: candidate.building.height,
                                            num_floors: candidate.building.num_floors,
                                        },
                                    ));
                                }
                                if !include_osm_sourced {
                                    continue;
                                }
                            }
                            harvest.buildings.push(candidate);
                        }
                    }
                    Ok(harvest)
                })
                .collect()
        });

        for result in results {
            match result {
                Ok(harvest) => {
                    for (key, hint) in harvest.hints {
                        hints.insert(key, hint);
                    }
                    for candidate in harvest.buildings {
                        match best.entry(candidate.gers_id.clone()) {
                            std::collections::hash_map::Entry::Occupied(mut slot) => {
                                if candidate.vertex_count > slot.get().vertex_count {
                                    slot.insert(candidate);
                                }
                            }
                            std::collections::hash_map::Entry::Vacant(slot) => {
                                slot.insert(candidate);
                            }
                        }
                    }
                }
                Err(e) => {
                    lost_tiles += 1;
                    if debug {
                        eprintln!("Warning: Overture tile could not be read: {e}");
                    }
                }
            }
        }

        if best.len() >= max_buildings {
            capped = true;
            break;
        }
    }

    if report_gaps && lost_tiles > 0 {
        eprintln!(
            "{} Overture Maps data incomplete: {lost_tiles} tile(s) could not be read. \
             Buildings are missing from the areas they cover.",
            "Warning:".yellow().bold()
        );
    }

    // A HashMap iterates in an order the standard library deliberately
    // randomises per process, and the budget below truncates. Sorting by the
    // GERS id makes which buildings survive a property of the data rather than
    // of this run, so two runs of one bbox produce the same world.
    let mut collected: Vec<TileBuilding> = best.into_values().collect();
    collected.sort_unstable_by(|a, b| a.gers_id.cmp(&b.gers_id));
    capped |= collected.len() > max_buildings;
    collected.truncate(max_buildings);
    let buildings: Vec<OvertureBuilding> = collected.into_iter().map(|c| c.building).collect();

    if report_gaps && capped {
        eprintln!(
            "{} Reached the Overture Maps building limit ({max_buildings}); footprints \
             beyond it were dropped, which leaves whole districts without them. \
             Use a smaller area for full coverage.",
            "Warning:".yellow().bold()
        );
    }

    Ok(OvertureCollection { buildings, hints })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox(min_lat: f64, min_lng: f64, max_lat: f64, max_lng: f64) -> LLBBox {
        LLBBox::new(min_lat, min_lng, max_lat, max_lng).unwrap()
    }

    #[test]
    fn the_osm_back_reference_is_read_out_of_the_tiles_sources_json() {
        // Verbatim shape of the attribute in Overture's published tiles.
        let sources = r#"[{"resource":"planet","version":"2026-08-02T00:00:00.000Z","license":"ODbL-1.0","record_id":"w106766186@8","update_time":"2025-05-18T16:28:13.000Z","provider":"osm","property":"","dataset":"OpenStreetMap"}]"#;
        let reference = parse_sources(sources).expect("OSM reference");
        assert_eq!(reference, super::super::OsmRef::way(106_766_186));

        // A non-OSM dataset contributes no reference.
        assert!(
            parse_sources(r#"[{"dataset":"Microsoft ML Buildings","record_id":"x1"}]"#).is_none()
        );
        // The OSM entry is found even when it is not the first.
        let mixed = r#"[{"dataset":"Esri","record_id":"e1"},{"dataset":"OpenStreetMap","record_id":"r42@2"}]"#;
        assert_eq!(
            parse_sources(mixed),
            Some(super::super::OsmRef::relation(42))
        );
        // An entry that does not even name a dataset must not hide the one
        // behind it, or that building silently loses its height enrichment.
        let ragged =
            r#"[{"note":"unknown"},{"dataset":42},{"dataset":"OpenStreetMap","record_id":"w7"}]"#;
        assert_eq!(parse_sources(ragged), Some(super::super::OsmRef::way(7)));
        // Likewise an OSM entry whose record_id is unusable: keep looking.
        let two_osm = r#"[{"dataset":"OpenStreetMap","record_id":"n9"},{"dataset":"OpenStreetMap","record_id":"w8"}]"#;
        assert_eq!(parse_sources(two_osm), Some(super::super::OsmRef::way(8)));
        // Malformed input costs the reference, never a panic.
        for bad in [
            "",
            "null",
            "{}",
            "[",
            "[{}]",
            r#"[{"dataset":"OpenStreetMap"}]"#,
        ] {
            assert!(parse_sources(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_bbox_covers_every_tile_it_touches() {
        // A degenerate bbox still needs the one tile containing it.
        let single = tiles_for_bbox(&bbox(48.137, 11.575, 48.1371, 11.5751));
        assert_eq!(single.len(), 1);
        assert_eq!(
            single[0],
            pmtiles::lonlat_to_tile(11.575, 48.137, QUERY_ZOOM)
        );

        // Main's benchmark bbox needs a small, exact rectangle of tiles.
        let munich = tiles_for_bbox(&bbox(48.125768, 11.552296, 48.148565, 11.593838));
        let xs: Vec<u32> = munich.iter().map(|t| t.0).collect();
        let ys: Vec<u32> = munich.iter().map(|t| t.1).collect();
        let width = xs.iter().max().unwrap() - xs.iter().min().unwrap() + 1;
        let height = ys.iter().max().unwrap() - ys.iter().min().unwrap() + 1;
        assert_eq!(munich.len() as u32, width * height);
        assert_eq!(munich.len(), 6);
    }

    #[test]
    fn tile_ranges_are_ordered_north_to_south() {
        // Tile y grows southward while latitude grows northward, so the corners
        // have to be taken from opposite edges or the range comes out empty.
        let tiles = tiles_for_bbox(&bbox(-34.0, 18.3, -33.8, 18.6));
        assert!(
            !tiles.is_empty(),
            "southern-hemisphere bbox produced no tiles"
        );
    }

    #[test]
    fn buffer_geometry_outside_the_request_is_dropped() {
        let area = bbox(48.10, 11.50, 48.20, 11.60);
        // Wholly inside.
        assert!(ring_overlaps_bbox(
            &[(11.55, 48.15), (11.56, 48.15), (11.56, 48.16)],
            &area
        ));
        // Straddling the edge - kept, because part of it is in the world.
        assert!(ring_overlaps_bbox(
            &[(11.49, 48.15), (11.51, 48.15), (11.51, 48.16)],
            &area
        ));
        // Entirely outside, which is what a tile buffer routinely holds.
        assert!(!ring_overlaps_bbox(
            &[(11.30, 48.15), (11.40, 48.15), (11.40, 48.16)],
            &area
        ));
        assert!(!ring_overlaps_bbox(
            &[(11.55, 48.30), (11.56, 48.30), (11.56, 48.31)],
            &area
        ));
    }

    #[test]
    fn a_continental_bbox_is_handed_back_to_the_parquet_path() {
        // Far more than MAX_TILES at zoom 14.
        assert!(tiles_for_bbox(&bbox(42.0, -5.0, 51.0, 8.0)).len() > MAX_TILES);
    }

    #[test]
    fn the_archive_url_is_built_from_the_release() {
        assert_eq!(
            archive_url("2026-08-19.0"),
            "https://overturemaps-extras-us-west-2.s3.us-west-2.amazonaws.com/tiles/2026-08-19.0/buildings.pmtiles"
        );
        // A malformed release never becomes a cache path.
        assert!(archive_cache_dir("../escape").is_none());
        assert!(archive_cache_dir("2026-08-19.0").is_some());
    }
}
