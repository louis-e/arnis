//! Reads OSM data from the Arnis tile archive instead of Overpass.
//!
//! One PMTiles file per continent on static hosting, baked by the `arnis-tiles` tool. A run
//! range-fetches only the z13 tiles its bbox covers and caches them on disk.

use crate::coordinate_system::geographic::LLBBox;
use crate::osm_parser::{OsmData, OsmElement, OsmMember};
use crate::overture::pmtiles::{self, Archive, TILE_TYPE_UNKNOWN};
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use rayon::prelude::*;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

/// Reads AOT1 and AOT2 alike, so a re-bake is swapped in here via archives.json without a release.
pub const DEFAULT_OSM_TILES_URL: &str = "https://tiles.arnisproject.com/v1";

/// Archive zoom. Must match `arnis-tiles`; a mismatch means every lookup misses.
const ZOOM: u8 = 13;

/// Degrees per coordinate unit, OSM's own 1e-7; AOT1's 1e-6 is scaled up on read.
const COORD_SCALE: f64 = 1e7;

/// Base of the ids packed from way vertex coordinates, below the clipper's invented ids.
const SYNTHETIC_ID_BASE: u64 = 1 << 61;

/// Same coordinate, same id in every bbox; facade walls match on it. Longitude wraps every 107
/// degrees because the globe at 1e-7 needs 63 bits; [`NodeIds`] keeps such pairs apart.
fn coordinate_node_id((lat, lon): (i32, i32)) -> u64 {
    let lat = (i64::from(lat) + MAX_LAT_E7) as u64;
    let lon = i64::from(lon).rem_euclid(1 << 30) as u64;
    SYNTHETIC_ID_BASE | (lat << 30) | lon
}

/// A latitude field no coordinate packs to (they stop at 1.8e9), for spill ids.
const SPILL_LAT: u64 = (1 << 31) - 1;

/// Vertex ids for one read. A coordinate whose packed id another coordinate already holds (same
/// latitude, longitude 2^30 units apart) gets a spill id, so distinct vertices never merge.
#[derive(Default)]
struct NodeIds {
    owner: HashMap<u64, (i32, i32)>,
    spilled: HashMap<(i32, i32), u64>,
}

impl NodeIds {
    /// The id for `p`, and whether this is the first time `p` came up.
    fn get(&mut self, p: (i32, i32)) -> (u64, bool) {
        let id = coordinate_node_id(p);
        match self.owner.get(&id) {
            None => {
                self.owner.insert(id, p);
                (id, true)
            }
            Some(q) if *q == p => (id, false),
            Some(_) => {
                let next = SYNTHETIC_ID_BASE | (SPILL_LAT << 30) | self.spilled.len() as u64;
                match self.spilled.entry(p) {
                    std::collections::hash_map::Entry::Occupied(e) => (*e.get(), false),
                    std::collections::hash_map::Entry::Vacant(e) => (*e.insert(next), true),
                }
            }
        }
    }
}

/// Refuse a tile that decompresses to more than this.
const MAX_TILE_BYTES: u64 = 256 * 1024 * 1024;

/// Guards against a bbox that would ask for the whole planet tile by tile.
const MAX_TILES: usize = 4096;

/// Zoom of the index's coverage cells. Continent bboxes overlap enormously - north-america,
/// russia and antarctica all span every longitude - so a bbox test alone opens up to six
/// archives to read one.
const CELL_ZOOM: u8 = 6;

const MAX_LAT_E7: i64 = 900_000_000;
const MAX_LON_E7: i64 = 1_800_000_000;

/// Whole relations sit at this tile id plus the relation id (the first zoom 20 id).
const RELATION_TILE_BASE: u64 = ((1 << 40) - 1) / 3;

/// A whole relation larger than this on the wire stays partial rather than being fetched.
const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;

/// Per-kind record cap. Far above a real tile; stops a corrupt one from outgrowing its payload.
const MAX_RECORDS: u64 = 1 << 24;

type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Deserialize)]
struct Manifest {
    zoom: u8,
    #[serde(default = "default_cell_zoom")]
    cell_zoom: u8,
    archives: Vec<ArchiveEntry>,
}

fn default_cell_zoom() -> u8 {
    CELL_ZOOM
}

#[derive(Debug, Deserialize, Clone)]
struct ArchiveEntry {
    file: String,
    /// Coarse cells the archive really holds. Absent in indexes baked before this existed, and
    /// then only the bbox is available.
    #[serde(default)]
    cells: Vec<u32>,
    min_lat: f64,
    min_lon: f64,
    max_lat: f64,
    max_lon: f64,
}

impl ArchiveEntry {
    /// `file` becomes a cache directory and a URL suffix, so it must be one harmless component.
    fn file_is_safe(&self) -> bool {
        !self.file.is_empty()
            && self.file.len() <= 96
            && !self.file.contains("..")
            && self
                .file
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    }

    /// Whether the archive holds any of `wanted`. Falls back to the bbox when the index carries
    /// no cells.
    fn covers(&self, wanted: &HashSet<u32>, bbox: &LLBBox) -> bool {
        if self.cells.is_empty() {
            return self.overlaps(bbox);
        }
        self.cells.iter().any(|c| wanted.contains(c))
    }

    fn overlaps(&self, bbox: &LLBBox) -> bool {
        !(self.max_lat < bbox.min().lat()
            || self.min_lat > bbox.max().lat()
            || self.max_lon < bbox.min().lng()
            || self.min_lon > bbox.max().lng())
    }
}

pub fn cache_root() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("arnis").join("osm-tiles"))
}

/// Frees the whole archive cache, including dirs left by older cache layouts.
pub fn clear_osm_tiles_cache() -> crate::elevation::cache::CacheClearStats {
    match cache_root() {
        Some(d) => crate::elevation::cache::clear_cache_dir(&d),
        None => Default::default(),
    }
}

/// Cached ranges are offsets into one specific file, so each base URL gets its own dir.
fn cache_root_for(base_url: &str) -> Option<PathBuf> {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in base_url.trim_end_matches('/').as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    cache_root().map(|d| d.join(format!("{h:016x}")))
}

fn client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent(crate::retrieve_data::OSM_USER_AGENT)
        .build()
        .map_err(|e| e.to_string())
}

/// The archive directory, refreshed daily. A stale copy still names archives that exist.
fn manifest(client: &Client, base_url: &str) -> Result<Manifest> {
    let url = format!("{}/archives.json", base_url.trim_end_matches('/'));
    let cached = cache_root_for(base_url).map(|d| d.join("archives.json"));
    if let Some(p) = &cached {
        if let Ok(md) = std::fs::metadata(p) {
            let fresh = md
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age < Duration::from_secs(86_400));
            if fresh {
                if let Ok(body) = std::fs::read(p) {
                    if let Ok(m) = serde_json::from_slice::<Manifest>(&body) {
                        return Ok(m);
                    }
                }
            }
        }
    }
    let body = client
        .get(&url)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(|e| format!("tile archive index unreachable: {e}"))?;
    let parsed: Manifest =
        serde_json::from_slice(&body).map_err(|e| format!("bad archive index: {e}"))?;
    if let Some(p) = &cached {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(p, &body);
            prune_stale(
                dir,
                &parsed.archives.iter().map(|a| a.file.as_str()).collect(),
            );
        }
    }
    Ok(parsed)
}

/// Deletes caches of archives the index no longer lists; a re-bake publishes new files.
fn prune_stale(dir: &std::path::Path, keep: &HashSet<&str>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
        let listed = e.file_name().to_str().is_some_and(|n| keep.contains(n));
        if is_dir && !listed {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Decoded tiles for one bbox, before they become [`OsmData`].
type Tags = Vec<(String, String)>;
type NodeBody = (i32, i32, Tags);
type WayBody = (bool, Tags, Vec<(i32, i32)>);
type RelationBody = (Tags, Vec<(u64, String)>);

#[derive(Default)]
struct Collected {
    nodes: HashMap<u64, NodeBody>,
    ways: HashMap<u64, WayBody>,
    relations: HashMap<u64, RelationBody>,
}

pub fn fetch_data_from_tiles(bbox: LLBBox, base_url: &str) -> Result<OsmData> {
    println!("{} Fetching data from the tile archive...", "[1/7]".bold());
    emit_gui_progress_update(1.0, "Downloading data...");
    let (data, tiles_read, bytes) = read_tiles(bbox, base_url)?;
    println!(
        "Read {tiles_read} tiles ({:.1} MB) from the archive",
        bytes as f64 / 1e6
    );
    emit_gui_progress_update(5.0, "");
    Ok(data)
}

/// [`fetch_data_from_tiles`] without progress output.
pub fn fetch_data_from_tiles_quietly(bbox: LLBBox, base_url: &str) -> Result<OsmData> {
    read_tiles(bbox, base_url).map(|(data, _, _)| data)
}

fn read_tiles(bbox: LLBBox, base_url: &str) -> Result<(OsmData, usize, u64)> {
    let client = client()?;
    let manifest = manifest(&client, base_url)?;
    if manifest.zoom != ZOOM {
        return Err(format!(
            "archive is zoom {} but this build reads zoom {ZOOM}",
            manifest.zoom
        ));
    }

    let (min_x, min_y) = pmtiles::lonlat_to_tile(bbox.min().lng(), bbox.max().lat(), ZOOM);
    let (max_x, max_y) = pmtiles::lonlat_to_tile(bbox.max().lng(), bbox.min().lat(), ZOOM);
    let (xs, xe) = (min_x.min(max_x), min_x.max(max_x));
    let (ys, ye) = (min_y.min(max_y), min_y.max(max_y));
    // Counted before collecting: a planet-sized bbox is 67M tiles, and the vector would be
    // hundreds of megabytes before the cap ever ran.
    let needed = ((xe - xs) as usize + 1).saturating_mul((ye - ys) as usize + 1);
    if needed > MAX_TILES {
        return Err(format!(
            "that area needs {needed} tiles, past the {MAX_TILES} cap"
        ));
    }
    let wanted: Vec<(u32, u32)> = (xs..=xe)
        .flat_map(|x| (ys..=ye).map(move |y| (x, y)))
        .collect();

    let mut collected = Collected::default();
    let mut tiles_read = 0usize;
    let mut bytes = 0u64;

    if manifest.cell_zoom > ZOOM {
        return Err(format!(
            "archive index has cell zoom {} above zoom {ZOOM}",
            manifest.cell_zoom
        ));
    }
    let shift = ZOOM - manifest.cell_zoom;
    let side = 1u32 << manifest.cell_zoom;
    let cells: HashSet<u32> = wanted
        .iter()
        .map(|(x, y)| (y >> shift) * side + (x >> shift))
        .collect();

    // Relation id -> the opened archives whose tiles carried it.
    let mut seen_in: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut opened: Vec<Archive> = Vec::new();
    // Only AOT2 archives hold whole relations; AOT1 ones are not searched for them.
    let mut aot2: Vec<bool> = Vec::new();
    for entry in manifest.archives.iter().filter(|a| a.covers(&cells, &bbox)) {
        if !entry.file_is_safe() {
            return Err(format!(
                "archive index has an unusable name: {:?}",
                entry.file
            ));
        }
        let url = format!("{}/{}", base_url.trim_end_matches('/'), entry.file);
        let cache = cache_root_for(base_url).map(|d| d.join(&entry.file));
        let mut archive = Archive::open_allowing(&client, &url, cache, &[TILE_TYPE_UNKNOWN])?;

        let mut located = Vec::new();
        for (x, y) in &wanted {
            if let Some(loc) = archive.locate(&client, ZOOM, *x, *y)? {
                located.push((*x, *y, loc));
            }
        }
        // Fetched and decompressed in parallel: a large bbox is dozens of tiles, and serially
        // that is dominated by round trips (72 tiles took 94s, most of it waiting).
        let fetched: Vec<Result<(u64, Vec<u8>)>> = located
            .par_iter()
            .map(|(x, y, loc)| {
                let raw = archive.tile(&client, ZOOM, *x, *y, *loc)?;
                unpack(raw, &format!("tile {ZOOM}/{x}/{y}"))
            })
            .collect();

        let k = opened.len();
        let mut is_aot2 = false;
        for entry in fetched {
            let (on_wire, plain) = entry?;
            if plain.is_empty() {
                continue;
            }
            bytes += on_wire;
            is_aot2 |= plain.starts_with(b"AOT2");
            let tile = decode(&plain)?;
            for r in &tile.relations {
                let list = seen_in.entry(r.0).or_default();
                if list.last() != Some(&k) {
                    list.push(k);
                }
            }
            absorb(tile, &mut collected);
            tiles_read += 1;
        }
        opened.push(archive);
        aot2.push(is_aot2);
    }

    if tiles_read == 0 {
        return Err("the tile archive has no data for this area".into());
    }

    // A tile only holds the members touching it, so a lake arrives as a few shore pieces;
    // fetch the whole relation for those the bbox uses, as Overpass returned them.
    let area = Extent::around(&bbox);
    let partial: Vec<u64> = collected
        .relations
        .iter()
        .filter(|(_, (_, members))| {
            members.iter().any(|(m, _)| !collected.ways.contains_key(m))
                && members
                    .iter()
                    .filter_map(|(m, _)| collected.ways.get(m))
                    .filter_map(|(_, _, pts)| Extent::of(pts))
                    .reduce(Extent::union)
                    .is_some_and(|e| e.intersects(&area))
        })
        .map(|(id, _)| *id)
        .collect();
    let mut records = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    for (k, archive) in opened.iter_mut().enumerate() {
        if !aot2[k] {
            continue;
        }
        let mut located = Vec::new();
        for rid in &partial {
            if !seen_in.get(rid).is_some_and(|l| l.contains(&k)) {
                continue;
            }
            let id = RELATION_TILE_BASE + rid;
            match archive.locate_id(&client, id) {
                Ok(Some(loc)) if u64::from(loc.length) <= MAX_RECORD_BYTES => {
                    located.push((id, loc));
                }
                Ok(_) => {}
                Err(e) => skipped.push(e),
            }
        }
        let archive = &*archive;
        let fetched: Vec<Result<(u64, Vec<u8>)>> = located
            .par_iter()
            .map(|(id, loc)| {
                let raw = archive.entry(&client, *id, *loc, MAX_RECORD_BYTES)?;
                unpack(raw, &format!("relation {}", id - RELATION_TILE_BASE))
            })
            .collect();
        // Best effort: one bad or oversized record must not send the whole area to Overpass.
        for entry in fetched {
            match entry.and_then(|(on_wire, plain)| {
                Ok((
                    on_wire,
                    (!plain.is_empty()).then(|| decode(&plain)).transpose()?,
                ))
            }) {
                Ok((_, None)) => {}
                Ok((on_wire, Some(tile))) => {
                    bytes += on_wire;
                    absorb(tile, &mut collected);
                    records += 1;
                }
                Err(e) => skipped.push(e),
            }
        }
    }
    if records > 0 {
        println!("Completed {records} relations that reach past the bbox's tiles");
    }
    if let Some(first) = skipped.first() {
        eprintln!(
            "{}",
            format!(
                "Warning: {} relations left partial ({first})",
                skipped.len()
            )
            .yellow()
        );
    }

    Ok((assemble(collected, &bbox), tiles_read, bytes))
}

/// Archives set tile_compression=none, so the baker's zstd frame is still around the payload.
fn unpack(raw: Vec<u8>, what: &str) -> Result<(u64, Vec<u8>)> {
    unpack_within(raw, what, MAX_TILE_BYTES)
}

/// Stops decoding one byte past `cap`: checking only afterwards let a hostile frame expand to
/// whatever it liked first.
fn unpack_within(raw: Vec<u8>, what: &str, cap: u64) -> Result<(u64, Vec<u8>)> {
    use std::io::Read;
    if raw.is_empty() {
        return Ok((0, Vec::new()));
    }
    let on_wire = raw.len() as u64;
    let mut plain = Vec::new();
    zstd::Decoder::new(&raw[..])
        .map_err(|e| format!("{what} is not readable: {e}"))?
        .take(cap.saturating_add(1))
        .read_to_end(&mut plain)
        .map_err(|e| format!("{what} is not readable: {e}"))?;
    if plain.len() as u64 > cap {
        return Err(format!("{what} expands past the size cap"));
    }
    Ok((on_wire, plain))
}

fn absorb(tile: DecodedTile, out: &mut Collected) {
    for n in tile.nodes {
        out.nodes.entry(n.0).or_insert((n.1, n.2, n.3));
    }
    for w in tile.ways {
        out.ways.entry(w.0).or_insert((w.1, w.2, w.3));
    }
    for r in tile.relations {
        out.relations.entry(r.0).or_insert((r.1, r.2));
    }
}

/// Coordinate units past the bbox that still count as inside it (~110 m): enough for an element
/// the projection rounds onto the edge row, and far less than a tile. The parser clips to the
/// bbox itself, so this only decides what it gets to see.
const EDGE_MARGIN: i64 = 10_000;

/// Lat/lon extent in coordinate units.
#[derive(Clone, Copy)]
struct Extent {
    min_lat: i32,
    min_lon: i32,
    max_lat: i32,
    max_lon: i32,
}

impl Extent {
    fn of(points: &[(i32, i32)]) -> Option<Self> {
        let (&(lat, lon), rest) = points.split_first()?;
        let mut e = Extent {
            min_lat: lat,
            min_lon: lon,
            max_lat: lat,
            max_lon: lon,
        };
        for &(lat, lon) in rest {
            e.min_lat = e.min_lat.min(lat);
            e.max_lat = e.max_lat.max(lat);
            e.min_lon = e.min_lon.min(lon);
            e.max_lon = e.max_lon.max(lon);
        }
        Some(e)
    }

    /// The bbox plus the edge margin.
    fn around(bbox: &LLBBox) -> Self {
        let units = |v: f64, round: fn(f64) -> f64| round(v * COORD_SCALE) as i64;
        let lat_lo = units(bbox.min().lat(), f64::floor) - EDGE_MARGIN;
        let lat_hi = units(bbox.max().lat(), f64::ceil) + EDGE_MARGIN;
        let lon_lo = units(bbox.min().lng(), f64::floor) - EDGE_MARGIN;
        let lon_hi = units(bbox.max().lng(), f64::ceil) + EDGE_MARGIN;
        Extent {
            min_lat: lat_lo.max(-MAX_LAT_E7) as i32,
            min_lon: lon_lo.max(-MAX_LON_E7) as i32,
            max_lat: lat_hi.min(MAX_LAT_E7) as i32,
            max_lon: lon_hi.min(MAX_LON_E7) as i32,
        }
    }

    fn union(self, o: Self) -> Self {
        Extent {
            min_lat: self.min_lat.min(o.min_lat),
            min_lon: self.min_lon.min(o.min_lon),
            max_lat: self.max_lat.max(o.max_lat),
            max_lon: self.max_lon.max(o.max_lon),
        }
    }

    fn intersects(&self, o: &Self) -> bool {
        self.min_lat <= o.max_lat
            && o.min_lat <= self.max_lat
            && self.min_lon <= o.max_lon
            && o.min_lon <= self.max_lon
    }

    fn contains(&self, lat: i32, lon: i32) -> bool {
        (self.min_lat..=self.max_lat).contains(&lat) && (self.min_lon..=self.max_lon).contains(&lon)
    }
}

fn is_building(tags: &Tags) -> bool {
    tags.iter()
        .any(|(k, v)| k == "building" || k == "building:part" || (k == "type" && v == "building"))
}

/// Ways and relations of `c` that the bbox can use. A z13 tile is several kilometres
/// across, so a city block's bbox pulls in two to three times its own area; everything
/// the parser would clip away entirely is left out here instead of being parsed first.
///
/// Kept: every way whose extent meets the bbox, every relation whose members' combined
/// extent does (a lake enclosing the whole bbox included) along with all its members, and
/// the building parts lying inside any kept building. Those parts can reach past the bbox,
/// and the outline suppression weighs all of them against the outline.
fn select_for_bbox(c: &Collected, bbox: &LLBBox) -> (HashSet<u64>, HashSet<u64>) {
    let area = Extent::around(bbox);
    let way_extent: HashMap<u64, Extent> = c
        .ways
        .iter()
        .filter_map(|(&id, (_, _, pts))| Extent::of(pts).map(|e| (id, e)))
        .collect();
    let relation_extent = |members: &[(u64, String)]| {
        members
            .iter()
            .filter_map(|(m, _)| way_extent.get(m).copied())
            .reduce(Extent::union)
    };

    let mut ways: HashSet<u64> = way_extent
        .iter()
        .filter(|(_, e)| e.intersects(&area))
        .map(|(&id, _)| id)
        .collect();
    let mut relations: HashSet<u64> = HashSet::new();
    let mut buildings = area;
    for (&id, (tags, members)) in &c.relations {
        let Some(e) = relation_extent(members) else {
            continue;
        };
        if e.intersects(&area) {
            relations.insert(id);
            ways.extend(members.iter().map(|(m, _)| *m));
            if is_building(tags) {
                buildings = buildings.union(e);
            }
        }
    }
    for id in &ways {
        if let (Some((_, tags, _)), Some(e)) = (c.ways.get(id), way_extent.get(id)) {
            if is_building(tags) {
                buildings = buildings.union(*e);
            }
        }
    }

    // Parts inside a building that straddles the edge.
    for (&id, (_, tags, _)) in &c.ways {
        if !ways.contains(&id)
            && is_building(tags)
            && way_extent
                .get(&id)
                .is_some_and(|e| e.intersects(&buildings))
        {
            ways.insert(id);
        }
    }
    for (&id, (tags, members)) in &c.relations {
        if !relations.contains(&id)
            && is_building(tags)
            && relation_extent(members).is_some_and(|e| e.intersects(&buildings))
        {
            relations.insert(id);
            ways.extend(members.iter().map(|(m, _)| *m));
        }
    }
    (ways, relations)
}

/// Builds the element list the Overpass path produces, so every later stage is unchanged.
/// Everything is emitted in id order: the parser keeps the first of two nodes on one
/// coordinate and processes elements in list order, so a hash-map order here would make
/// two runs over the same data build different worlds.
fn assemble(c: Collected, bbox: &LLBBox) -> OsmData {
    let (keep_ways, keep_relations) = select_for_bbox(&c, bbox);
    let area = Extent::around(bbox);
    let Collected {
        nodes,
        ways,
        relations,
    } = c;

    let mut nodes: Vec<(u64, NodeBody)> = nodes.into_iter().collect();
    nodes.sort_unstable_by_key(|n| n.0);
    // Lowest tagged node per coordinate; on a vertex it takes the coordinate id.
    let mut tagged_at: HashMap<(i32, i32), u64> = HashMap::new();
    for (id, (lat, lon, _)) in &nodes {
        tagged_at.entry((*lat, *lon)).or_insert(*id);
    }

    let mut ways: Vec<DecWay> = ways
        .into_iter()
        .filter(|(id, _)| keep_ways.contains(id))
        .map(|(id, (closed, tags, pts))| (id, closed, tags, pts))
        .collect();
    ways.sort_unstable_by_key(|w| w.0);

    let mut emitted: Vec<(u64, i32, i32)> = Vec::new();
    let mut ids = NodeIds::default();
    let mut vertex_nodes: HashSet<u64> = HashSet::new();
    let mut way_elements: Vec<OsmElement> = Vec::with_capacity(ways.len());
    for (id, closed, tags, points) in ways {
        let mut refs: Vec<u64> = Vec::with_capacity(points.len() + 1);
        for p in &points {
            // Never a tagged node's own id: which ones a read sees depends on its tiles.
            let (nid, first) = ids.get(*p);
            if first {
                match tagged_at.get(p) {
                    Some(&tagged) => {
                        vertex_nodes.insert(tagged);
                    }
                    None => emitted.push((nid, p.0, p.1)),
                }
            }
            refs.push(nid);
        }
        if closed {
            if let (Some(first), Some(last)) = (refs.first().copied(), refs.last().copied()) {
                if first != last {
                    refs.push(first);
                }
            }
        }
        way_elements.push(OsmElement {
            r#type: "way".into(),
            id,
            lat: None,
            lon: None,
            nodes: Some(refs),
            tags: Some(tags.into_iter().collect()),
            members: Vec::new(),
        });
    }

    // A tagged node outside the bbox still matters when a kept way runs through it: the
    // way's vertex carries its tags.
    let mut elements: Vec<OsmElement> = Vec::new();
    for (id, (lat, lon, tags)) in nodes {
        if !area.contains(lat, lon) && !vertex_nodes.contains(&id) {
            continue;
        }
        let id = if vertex_nodes.contains(&id) {
            ids.get((lat, lon)).0
        } else {
            id
        };
        elements.push(OsmElement {
            r#type: "node".into(),
            id,
            lat: Some(f64::from(lat) / COORD_SCALE),
            lon: Some(f64::from(lon) / COORD_SCALE),
            nodes: None,
            tags: Some(tags.into_iter().collect()),
            members: Vec::new(),
        });
    }
    elements.extend(way_elements);

    for (id, lat, lon) in emitted {
        elements.push(OsmElement {
            r#type: "node".into(),
            id,
            lat: Some(f64::from(lat) / COORD_SCALE),
            lon: Some(f64::from(lon) / COORD_SCALE),
            nodes: None,
            tags: None,
            members: Vec::new(),
        });
    }

    let mut rels: Vec<DecRelation> = relations
        .into_iter()
        .filter(|(id, _)| keep_relations.contains(id))
        .map(|(id, (tags, members))| (id, tags, members))
        .collect();
    rels.sort_unstable_by_key(|r| r.0);
    for (id, tags, members) in rels {
        elements.push(OsmElement {
            r#type: "relation".into(),
            id,
            lat: None,
            lon: None,
            nodes: None,
            tags: Some(tags.into_iter().collect()),
            members: members
                .into_iter()
                .map(|(r#ref, role)| OsmMember {
                    r#type: "way".into(),
                    r#ref,
                    role,
                })
                .collect(),
        });
    }

    OsmData::from_elements(elements)
}

// ── the AOT1 payload ──────────────────────────────────────────────────────────
// Mirror of arnis-tiles/src/format.rs.

type DecNode = (u64, i32, i32, Tags);
type DecWay = (u64, bool, Tags, Vec<(i32, i32)>);
type DecRelation = (u64, Tags, Vec<(u64, String)>);

#[derive(Default)]
struct DecodedTile {
    nodes: Vec<DecNode>,
    ways: Vec<DecWay>,
    relations: Vec<DecRelation>,
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn uvarint(&mut self) -> Result<u64> {
        let mut out = 0u64;
        let mut shift = 0u32;
        loop {
            let b = *self.buf.get(self.pos).ok_or("truncated tile")?;
            self.pos += 1;
            if shift >= 64 {
                return Err("varint overflow".into());
            }
            out |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(out);
            }
            shift += 7;
        }
    }

    fn svarint(&mut self) -> Result<i64> {
        let v = self.uvarint()?;
        Ok(((v >> 1) as i64) ^ -((v & 1) as i64))
    }

    fn byte(&mut self) -> Result<u8> {
        let b = *self.buf.get(self.pos).ok_or("truncated tile")?;
        self.pos += 1;
        Ok(b)
    }
}

/// Accumulates one delta, refusing the wrap a corrupt tile would otherwise get.
fn step(acc: &mut i64, r: &mut Reader<'_>) -> Result<i64> {
    *acc = acc
        .checked_add(r.svarint()?)
        .ok_or("tile delta overflows")?;
    Ok(*acc)
}

/// Coordinates in 1e-7 degrees. Out-of-globe values reach `LLPoint::new` downstream,
/// which panics rather than returning, so they are rejected here.
fn coord(v: i64, limit: i64) -> Result<i32> {
    if !(-limit..=limit).contains(&v) {
        return Err("tile coordinate out of range".into());
    }
    Ok(v as i32)
}

/// Real OSM ids only. Anything at or above the synthetic base would collide with the ids
/// `assemble` mints for way vertices.
fn oid(v: i64) -> Result<u64> {
    match u64::try_from(v) {
        Ok(id) if id < SYNTHETIC_ID_BASE => Ok(id),
        _ => Err("tile id out of range".into()),
    }
}

fn decode(buf: &[u8]) -> Result<DecodedTile> {
    let scale: i64 = match buf.get(..4) {
        Some(b"AOT2") => 1,
        Some(b"AOT1") => 10,
        _ => return Err("not an Arnis tile payload".into()),
    };
    let up = |v: i64, limit: i64| {
        v.checked_mul(scale)
            .ok_or_else(|| "tile coordinate out of range".to_string())
            .and_then(|v| coord(v, limit))
    };
    let mut r = Reader { buf, pos: 4 };

    let n_strings = r.uvarint()?;
    if n_strings > 1 << 24 {
        return Err("implausible string table".into());
    }
    let mut strings: Vec<String> = Vec::with_capacity(n_strings.min(4096) as usize);
    for _ in 0..n_strings {
        let len = r.uvarint()? as usize;
        let end = r.pos.checked_add(len).ok_or("string overflows tile")?;
        let raw = buf.get(r.pos..end).ok_or("truncated string")?;
        strings.push(String::from_utf8(raw.to_vec()).map_err(|e| e.to_string())?);
        r.pos = end;
    }
    let at = |i: u64| -> Result<String> {
        strings
            .get(i as usize)
            .cloned()
            .ok_or_else(|| "string index out of range".to_string())
    };
    let read_tags = |r: &mut Reader| -> Result<Tags> {
        let n = r.uvarint()?;
        if n > 1 << 16 {
            return Err("implausible tag count".into());
        }
        let mut out = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let (k, v) = (r.uvarint()?, r.uvarint()?);
            out.push((at(k)?, at(v)?));
        }
        Ok(out)
    };

    let mut tile = DecodedTile::default();

    let n_nodes = r.uvarint()?;
    if n_nodes > MAX_RECORDS {
        return Err("implausible node count".into());
    }
    let (mut id, mut lat, mut lon) = (0i64, 0i64, 0i64);
    for _ in 0..n_nodes {
        let nid = oid(step(&mut id, &mut r)?)?;
        let y = up(step(&mut lat, &mut r)?, MAX_LAT_E7)?;
        let x = up(step(&mut lon, &mut r)?, MAX_LON_E7)?;
        let tags = read_tags(&mut r)?;
        tile.nodes.push((nid, y, x, tags));
    }

    let n_ways = r.uvarint()?;
    if n_ways > MAX_RECORDS {
        return Err("implausible way count".into());
    }
    let mut id = 0i64;
    for _ in 0..n_ways {
        let wid = oid(step(&mut id, &mut r)?)?;
        let closed = r.byte()? != 0;
        let tags = read_tags(&mut r)?;
        let n_pts = r.uvarint()?;
        if n_pts > 1 << 22 {
            return Err("implausible vertex count".into());
        }
        let mut pts = Vec::with_capacity(n_pts as usize);
        let (mut a, mut o) = (0i64, 0i64);
        for _ in 0..n_pts {
            let y = up(step(&mut a, &mut r)?, MAX_LAT_E7)?;
            let x = up(step(&mut o, &mut r)?, MAX_LON_E7)?;
            pts.push((y, x));
        }
        tile.ways.push((wid, closed, tags, pts));
    }

    let n_rels = r.uvarint()?;
    if n_rels > MAX_RECORDS {
        return Err("implausible relation count".into());
    }
    let mut id = 0i64;
    for _ in 0..n_rels {
        let rid = oid(step(&mut id, &mut r)?)?;
        let tags = read_tags(&mut r)?;
        let n_mem = r.uvarint()?;
        if n_mem > 1 << 20 {
            return Err("implausible member count".into());
        }
        let mut members = Vec::with_capacity(n_mem as usize);
        let mut pm = 0i64;
        for _ in 0..n_mem {
            let mid = oid(step(&mut pm, &mut r)?)?;
            let role = r.uvarint()?;
            members.push((mid, at(role)?));
        }
        tile.relations.push((rid, tags, members));
    }

    Ok(tile)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_named(file: &str) -> ArchiveEntry {
        ArchiveEntry {
            file: file.to_string(),
            cells: Vec::new(),
            min_lat: 0.0,
            min_lon: 0.0,
            max_lat: 1.0,
            max_lon: 1.0,
        }
    }

    #[test]
    fn manifest_names_cannot_escape_the_cache_root() {
        for good in ["europe-20260921.pmtiles", "north_america-20260921.pmtiles"] {
            assert!(entry_named(good).file_is_safe(), "{good} should be allowed");
        }
        for bad in [
            "",
            "..",
            "../etc.pmtiles",
            "/etc/passwd",
            "a/b.pmtiles",
            "a\\b.pmtiles",
            &"x".repeat(97),
        ] {
            assert!(
                !entry_named(bad).file_is_safe(),
                "{bad:?} should be refused"
            );
        }
    }

    #[test]
    fn absurd_record_counts_are_refused() {
        let mut buf = b"AOT1".to_vec();
        buf.push(0); // empty string table
        buf.extend_from_slice(&[0xff; 9]); // u64::MAX nodes
        buf.push(0x01);
        let err = match decode(&buf) {
            Err(e) => e,
            Ok(_) => panic!("absurd node count was accepted"),
        };
        assert!(err.contains("node count"), "unexpected error: {err}");
    }

    fn uvar(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    fn svar(v: i64, out: &mut Vec<u8>) {
        uvar(((v << 1) ^ (v >> 63)) as u64, out);
    }

    fn node_tile_as(magic: &[u8; 4], deltas: &[(i64, i64, i64)]) -> Vec<u8> {
        let mut buf = magic.to_vec();
        buf.push(0);
        uvar(deltas.len() as u64, &mut buf);
        for (id, lat, lon) in deltas {
            svar(*id, &mut buf);
            svar(*lat, &mut buf);
            svar(*lon, &mut buf);
            buf.push(0);
        }
        buf.push(0); // no ways
        buf.push(0); // no relations
        buf
    }

    fn node_tile(deltas: &[(i64, i64, i64)]) -> Vec<u8> {
        node_tile_as(b"AOT2", deltas)
    }

    // The v1 archives are AOT1 at 1e-6; they still read, at the same place.
    #[test]
    fn aot1_payloads_are_scaled_to_full_precision() {
        let v1 = decode(&node_tile_as(b"AOT1", &[(7, 48_137_154, 11_575_382)])).unwrap();
        let v2 = decode(&node_tile(&[(7, 481_371_540, 115_753_820)])).unwrap();
        assert_eq!(v1.nodes[0].1, v2.nodes[0].1);
        assert_eq!(v1.nodes[0].2, v2.nodes[0].2);
    }

    fn decode_err(buf: &[u8]) -> String {
        match decode(buf) {
            Err(e) => e,
            Ok(_) => panic!("corrupt tile was accepted"),
        }
    }

    // Out-of-globe coordinates reach LLPoint::new downstream, which panics instead of
    // returning, so the decoder has to be the thing that says no.
    #[test]
    fn out_of_range_coordinates_are_refused() {
        let err = decode_err(&node_tile(&[(1, 2_000_000_000, 0)]));
        assert!(err.contains("out of range"), "{err}");
    }

    #[test]
    fn delta_overflow_is_refused() {
        let err = decode_err(&node_tile(&[(1, MAX_LAT_E7, 0), (1, i64::MAX, 0)]));
        assert!(err.contains("overflow"), "{err}");
    }

    #[test]
    fn ids_colliding_with_synthetic_ones_are_refused() {
        let err = decode_err(&node_tile(&[(SYNTHETIC_ID_BASE as i64, 0, 0)]));
        assert!(err.contains("id out of range"), "{err}");
        assert!(decode(&node_tile(&[(1, 0, 0)])).is_ok());
    }

    #[test]
    fn cells_pick_one_archive_where_bboxes_overlap() {
        let wanted: std::collections::HashSet<u32> = [1442].into_iter().collect();
        let munich = LLBBox::new(48.135, 11.571, 48.139, 11.578).unwrap();

        // north-america's bbox spans every longitude, so the bbox test alone lets it through.
        let mut wide = entry_named("north-america.pmtiles");
        wide.min_lat = -46.71;
        wide.max_lat = 83.88;
        wide.min_lon = -180.0;
        wide.max_lon = 180.0;
        assert!(wide.overlaps(&munich));
        wide.cells = vec![10, 11, 12];
        assert!(!wide.covers(&wanted, &munich));

        let mut europe = wide.clone();
        europe.cells = vec![1441, 1442, 1443];
        assert!(europe.covers(&wanted, &munich));

        // An index without cells must keep working on the bbox alone.
        let mut legacy = wide.clone();
        legacy.cells = Vec::new();
        assert!(legacy.covers(&wanted, &munich));
    }

    #[test]
    fn caches_of_archives_no_longer_listed_are_dropped() {
        let dir = std::env::temp_dir().join(format!("arnis-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["europe-20260919.pmtiles", "europe-20261018.pmtiles"] {
            std::fs::create_dir_all(dir.join(d).join("t")).unwrap();
        }
        std::fs::write(dir.join("archives.json"), b"{}").unwrap();
        prune_stale(&dir, &["europe-20261018.pmtiles"].into_iter().collect());
        assert!(!dir.join("europe-20260919.pmtiles").exists());
        assert!(dir.join("europe-20261018.pmtiles").exists());
        assert!(dir.join("archives.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_frame_expanding_past_the_cap_is_refused() {
        let bomb = zstd::encode_all(&vec![0u8; 100_000][..], 19).unwrap();
        assert!(bomb.len() < 1_000);
        let err = unpack_within(bomb.clone(), "tile", 10_000).unwrap_err();
        assert!(err.contains("size cap"), "{err}");
        assert_eq!(
            unpack_within(bomb, "tile", 100_000).unwrap().1.len(),
            100_000
        );
    }

    #[test]
    fn cache_dirs_differ_per_base_url() {
        let a = cache_root_for("https://tiles.arnisproject.com/v1");
        let b = cache_root_for("https://tiles.arnisproject.com/v2");
        let c = cache_root_for("https://tiles.arnisproject.com/v1/");
        assert_ne!(a, b);
        assert_eq!(a, c);
    }

    // The baker and the client must agree on the grid or every lookup misses. This is the
    // tile arnis-tiles computes for Andorra la Vella at z13.
    #[test]
    fn the_client_tiler_matches_the_baker() {
        assert_eq!(pmtiles::lonlat_to_tile(1.5218, 42.5063, ZOOM), (4130, 3025));
    }

    #[test]
    fn a_foreign_payload_is_rejected() {
        assert!(decode(b"").is_err());
        assert!(decode(b"NOPE0000").is_err());
    }

    /// Covers the coordinates the tests below use (coordinate units in the tens).
    fn test_bbox() -> LLBBox {
        LLBBox::new(0.0, 0.0, 0.001, 0.001).unwrap()
    }

    fn ids_of(data: &OsmData, kind: &str) -> Vec<u64> {
        data.elements()
            .iter()
            .filter(|e| e.r#type == kind)
            .map(|e| e.id)
            .collect()
    }

    fn way_at(c: &mut Collected, id: u64, tags: &[(&str, &str)], pts: &[(i32, i32)]) {
        let tags = tags
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        c.ways.insert(id, (false, tags, pts.to_vec()));
    }

    // Far past the bbox (a degree away) is dropped; crossing it without a vertex inside is not.
    #[test]
    fn ways_outside_the_bbox_are_left_out() {
        let mut c = Collected::default();
        way_at(
            &mut c,
            1,
            &[("highway", "primary")],
            &[(500, 500), (600, 600)],
        );
        way_at(
            &mut c,
            2,
            &[("highway", "primary")],
            &[(1_000_000, 0), (1_000_100, 0)],
        );
        way_at(
            &mut c,
            3,
            &[("highway", "primary")],
            &[(-50_000, 500), (50_000, 500)],
        );
        let data = assemble(c, &test_bbox());
        assert_eq!(ids_of(&data, "way"), vec![1, 3]);
    }

    // A lake enclosing the whole bbox has no member inside it, and must still come through
    // with every member.
    #[test]
    fn a_relation_around_the_bbox_is_kept_whole() {
        let mut c = Collected::default();
        way_at(&mut c, 10, &[], &[(-90_000, -90_000), (-90_000, 90_000)]);
        way_at(&mut c, 11, &[], &[(-90_000, 90_000), (90_000, 90_000)]);
        way_at(&mut c, 12, &[], &[(90_000, 90_000), (90_000, -90_000)]);
        way_at(&mut c, 13, &[], &[(90_000, -90_000), (-90_000, -90_000)]);
        c.relations.insert(
            20,
            (
                vec![
                    ("type".into(), "multipolygon".into()),
                    ("natural".into(), "water".into()),
                ],
                (10..14).map(|m| (m, "outer".to_string())).collect(),
            ),
        );
        c.relations.insert(
            21,
            (
                vec![("type".into(), "multipolygon".into())],
                vec![(99, "outer".to_string())],
            ),
        );
        let data = assemble(c, &test_bbox());
        assert_eq!(ids_of(&data, "relation"), vec![20]);
        assert_eq!(ids_of(&data, "way"), vec![10, 11, 12, 13]);
    }

    // The outline suppression compares an outline against all of its parts, including the
    // ones past the bbox edge.
    #[test]
    fn parts_of_a_building_across_the_edge_are_kept() {
        let mut c = Collected::default();
        let outline = [
            (500, 500),
            (500, 9_000),
            (3_000, 9_000),
            (3_000, 500),
            (500, 500),
        ];
        way_at(&mut c, 1, &[("building", "yes")], &outline);
        way_at(
            &mut c,
            2,
            &[("building:part", "yes")],
            &[(600, 7_000), (900, 8_000)],
        );
        way_at(
            &mut c,
            3,
            &[("building:part", "yes")],
            &[(600, 70_000), (900, 80_000)],
        );
        let data = assemble(c, &test_bbox());
        assert_eq!(ids_of(&data, "way"), vec![1, 2]);
    }

    // Outside the bbox a tagged node is only kept as a vertex of a kept way, which carries
    // its tags.
    #[test]
    fn tagged_nodes_outside_the_bbox_survive_only_as_vertices() {
        let mut c = Collected::default();
        let tag = || vec![("highway".to_string(), "traffic_signals".to_string())];
        c.nodes.insert(5, (500, 500, tag()));
        c.nodes.insert(6, (-50_000, 500, tag()));
        c.nodes.insert(7, (-60_000, 500, tag()));
        way_at(
            &mut c,
            1,
            &[("highway", "primary")],
            &[(-50_000, 500), (500, 500)],
        );
        let data = assemble(c, &test_bbox());
        let tagged: Vec<u64> = data
            .elements()
            .iter()
            .filter(|e| e.r#type == "node" && e.tags.is_some())
            .map(|e| e.id)
            .collect();
        let at = |lat| coordinate_node_id((lat, 500));
        assert_eq!(tagged, vec![at(500), at(-50_000)]);
    }

    // Two nodes on one coordinate: the vertex always takes the lowest one's tags.
    #[test]
    fn a_shared_coordinate_resolves_to_the_lowest_id() {
        let vertex = coordinate_node_id((500, 500));
        for _ in 0..8 {
            let mut c = Collected::default();
            for id in [40, 30, 50] {
                c.nodes
                    .insert(id, (500, 500, vec![("ref".into(), id.to_string())]));
            }
            way_at(&mut c, 1, &[("building", "yes")], &[(500, 500), (600, 600)]);
            let data = assemble(c, &test_bbox());
            assert_eq!(ids_of(&data, "node")[..3], [vertex, 40, 50]);
            let node = data.elements().iter().find(|e| e.id == vertex).unwrap();
            assert_eq!(node.tags.as_ref().unwrap()["ref"], "30");
            let way = data.elements().iter().find(|e| e.r#type == "way").unwrap();
            assert_eq!(way.nodes.as_ref().unwrap()[0], vertex);
        }
    }

    // A ring's first and last vertex must come back as one node, or every building outline
    // reads as an open way downstream.
    #[test]
    fn a_closed_way_starts_and_ends_on_the_same_node() {
        let mut c = Collected::default();
        c.ways.insert(
            7,
            (
                true,
                vec![("building".into(), "yes".into())],
                vec![(10, 10), (10, 20), (20, 20)],
            ),
        );
        let data = assemble(c, &test_bbox());
        let els = data.elements();
        let way = els.iter().find(|e| e.r#type == "way").expect("way missing");
        let refs = way.nodes.as_ref().expect("way has no refs");
        assert_eq!(refs.first(), refs.last(), "ring must close on one node");
        assert_eq!(refs.len(), 4);
    }

    // Two ways meeting at a point must share the node, which is what makes a junction a
    // junction rather than two coincident dead ends.
    #[test]
    fn ways_meeting_at_a_point_share_one_node() {
        let mut c = Collected::default();
        c.ways.insert(
            1,
            (
                false,
                vec![("highway".into(), "residential".into())],
                vec![(0, 0), (5, 5)],
            ),
        );
        c.ways.insert(
            2,
            (
                false,
                vec![("highway".into(), "service".into())],
                vec![(5, 5), (9, 9)],
            ),
        );
        let data = assemble(c, &test_bbox());
        let mut refs: Vec<(u64, Vec<u64>)> = data
            .elements()
            .iter()
            .filter(|e| e.r#type == "way")
            .map(|e| (e.id, e.nodes.clone().unwrap_or_default()))
            .collect();
        refs.sort_by_key(|(id, _)| *id);
        assert_eq!(refs.len(), 2);
        assert_eq!(
            refs[0].1.last(),
            refs[1].1.first(),
            "shared point, one node"
        );
    }

    #[test]
    fn a_vertex_keeps_its_id_across_boxes() {
        let vertex_ids = |extra: bool, bbox: LLBBox| {
            let mut c = Collected::default();
            if extra {
                way_at(
                    &mut c,
                    1,
                    &[("highway", "service")],
                    &[(100, 100), (200, 200)],
                );
                // A tagged node on the ring, fetched only by the wider read.
                c.nodes
                    .insert(3, (300, 300, vec![("entrance".into(), "yes".into())]));
            }
            way_at(
                &mut c,
                9,
                &[("building", "yes")],
                &[(300, 300), (300, 400), (400, 400)],
            );
            let data = assemble(c, &bbox);
            let way = data.elements().iter().find(|e| e.id == 9).unwrap();
            way.nodes.clone().unwrap()
        };
        let wide = LLBBox::new(-0.01, -0.01, 0.01, 0.01).unwrap();
        let ids = vertex_ids(false, test_bbox());
        assert_eq!(ids, vertex_ids(true, wide));
        for id in ids {
            assert!(id >= SYNTHETIC_ID_BASE);
            assert!(
                !crate::clipping::is_invented_node_id(id),
                "reads as clipped"
            );
        }
    }

    // Same latitude, longitude exactly 2^30 units apart: the one pair the packing cannot tell
    // apart. A wide bbox or a whole relation can bring both into one read.
    #[test]
    fn coordinates_107_degrees_apart_stay_separate_vertices() {
        let far = 1 << 30;
        assert_eq!(coordinate_node_id((0, 0)), coordinate_node_id((0, far)));
        let mut c = Collected::default();
        c.ways.insert(
            1,
            (
                false,
                vec![("highway".into(), "residential".into())],
                vec![(0, 0), (5, 5), (0, far)],
            ),
        );
        let data = assemble(c, &LLBBox::new(0.0, 0.0, 0.001, 108.0).unwrap());
        let refs = data
            .elements()
            .iter()
            .find(|e| e.r#type == "way")
            .and_then(|e| e.nodes.clone())
            .unwrap();
        assert_eq!(refs.len(), 3);
        assert_ne!(refs[0], refs[2]);
        let lon_of = |id: u64| {
            data.elements()
                .iter()
                .find(|e| e.r#type == "node" && e.id == id)
                .and_then(|e| e.lon)
                .unwrap()
        };
        assert_eq!(lon_of(refs[0]), 0.0);
        assert!((lon_of(refs[2]) - far as f64 / COORD_SCALE).abs() < 1e-9);
        assert!(refs
            .iter()
            .all(|&id| id >= SYNTHETIC_ID_BASE && !crate::clipping::is_invented_node_id(id)));
    }

    #[test]
    fn coordinate_ids_are_distinct_and_cover_the_globe() {
        let corners = [
            (-900_000_000, -1_800_000_000),
            (-900_000_000, 1_799_999_999),
            (900_000_000, -1_800_000_000),
            (900_000_000, 1_799_999_999),
            (0, 0),
            (0, 1),
            (1, 0),
        ];
        let ids: HashSet<u64> = corners.iter().map(|&p| coordinate_node_id(p)).collect();
        assert_eq!(ids.len(), corners.len());
        assert!(ids
            .iter()
            .all(|&id| id >= SYNTHETIC_ID_BASE && !crate::clipping::is_invented_node_id(id)));
    }

    // Every 1e-7 step in a bbox-sized patch gets its own id, across the antimeridian too.
    #[test]
    fn neighbouring_coordinates_never_share_an_id() {
        for &(lat0, lon0) in &[
            (481_371_540i64, 115_753_820i64),
            (-170_000_000, 1_799_999_990),
            (0, -5),
        ] {
            let mut ids = HashSet::new();
            for dlat in 0..20 {
                for dlon in 0..20 {
                    let lon =
                        (lon0 + dlon + 1_800_000_000).rem_euclid(3_600_000_000) - 1_800_000_000;
                    assert!(ids.insert(coordinate_node_id(((lat0 + dlat) as i32, lon as i32))));
                }
            }
        }
        // Far enough apart to share the low bits is far past any bbox.
        assert_ne!(
            coordinate_node_id((0, 0)),
            coordinate_node_id((0, 999_999_999))
        );
    }

    // A lake's whole record fills in the shore pieces a tile alone did not carry.
    #[test]
    fn a_whole_relation_completes_the_members_a_tile_lacked() {
        let mut c = Collected::default();
        let lake = (
            vec![("natural".to_string(), "water".to_string())],
            vec![(1, "outer".to_string()), (2, "outer".to_string())],
        );
        absorb(
            DecodedTile {
                ways: vec![(1, false, vec![], vec![(0, 0), (0, 10)])],
                relations: vec![(9, lake.0.clone(), lake.1.clone())],
                ..DecodedTile::default()
            },
            &mut c,
        );
        assert!(!c.ways.contains_key(&2));
        absorb(
            DecodedTile {
                ways: vec![
                    (1, false, vec![], vec![(0, 0), (0, 10)]),
                    (2, false, vec![], vec![(0, 10), (0, 0)]),
                ],
                relations: vec![(9, lake.0, lake.1)],
                ..DecodedTile::default()
            },
            &mut c,
        );
        assert!(c.relations[&9]
            .1
            .iter()
            .all(|(m, _)| c.ways.contains_key(m)));
    }
}
