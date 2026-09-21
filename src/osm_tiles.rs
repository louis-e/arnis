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
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Override per run with `--osm-tiles-url`. The version prefix is part of it: a re-bake is
/// published beside the old one, so a run in flight never sees half of each.
pub const DEFAULT_OSM_TILES_URL: &str = "https://tiles.arnisproject.com/v1";

/// Archive zoom. Must match `arnis-tiles`; a mismatch means every lookup misses.
const ZOOM: u8 = 13;

/// Degrees per stored coordinate unit in the tile payload. Must match arnis-tiles.
const COORD_SCALE: f64 = 1e6;

/// Way vertices carry no OSM id, so one is minted per coordinate from here. Real node ids are
/// far below this.
const SYNTHETIC_ID_BASE: u64 = 1 << 62;

/// Refuse a tile that decompresses to more than this.
const MAX_TILE_BYTES: u64 = 256 * 1024 * 1024;

/// Guards against a bbox that would ask for the whole planet tile by tile.
const MAX_TILES: usize = 4096;

/// Zoom of the index's coverage cells. Continent bboxes overlap enormously - north-america,
/// russia and antarctica all span every longitude - so a bbox test alone opens up to six
/// archives to read one.
const CELL_ZOOM: u8 = 6;

const MAX_LAT_E6: i64 = 90_000_000;
const MAX_LON_E6: i64 = 180_000_000;

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
    fn covers(&self, wanted: &std::collections::HashSet<u32>, bbox: &LLBBox) -> bool {
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
        }
        let _ = std::fs::write(p, &body);
    }
    Ok(parsed)
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
    let cells: std::collections::HashSet<u32> = wanted
        .iter()
        .map(|(x, y)| (y >> shift) * side + (x >> shift))
        .collect();

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
                if raw.is_empty() {
                    return Ok((0, Vec::new()));
                }
                let on_wire = raw.len() as u64;
                // Archives set tile_compression=none, so the baker's zstd frame is still here.
                let plain = zstd::stream::decode_all(&raw[..])
                    .map_err(|e| format!("tile {ZOOM}/{x}/{y} is not readable: {e}"))?;
                if plain.len() as u64 > MAX_TILE_BYTES {
                    return Err(format!("tile {ZOOM}/{x}/{y} expands past the size cap"));
                }
                Ok((on_wire, plain))
            })
            .collect();

        for entry in fetched {
            let (on_wire, plain) = entry?;
            if plain.is_empty() {
                continue;
            }
            bytes += on_wire;
            absorb(&plain, &mut collected)?;
            tiles_read += 1;
        }
    }

    println!(
        "Read {tiles_read} tiles ({:.1} MB) from the archive",
        bytes as f64 / 1e6
    );
    emit_gui_progress_update(5.0, "");

    if tiles_read == 0 {
        return Err("the tile archive has no data for this area".into());
    }
    Ok(assemble(collected))
}

fn absorb(payload: &[u8], out: &mut Collected) -> Result<()> {
    let tile = decode(payload)?;
    for n in tile.nodes {
        out.nodes.entry(n.0).or_insert((n.1, n.2, n.3));
    }
    for w in tile.ways {
        out.ways.entry(w.0).or_insert((w.1, w.2, w.3));
    }
    for r in tile.relations {
        out.relations.entry(r.0).or_insert((r.1, r.2));
    }
    Ok(())
}

/// Builds the element list the Overpass path produces, so every later stage is unchanged.
fn assemble(c: Collected) -> OsmData {
    let mut elements: Vec<OsmElement> = Vec::new();
    // One id per distinct coordinate, so junctions share a node and a ring closes on itself.
    let mut coord_ids: HashMap<(i32, i32), u64> = HashMap::new();
    let mut next_synthetic = SYNTHETIC_ID_BASE;
    let mut emitted: Vec<(u64, i32, i32)> = Vec::new();

    for (id, (lat, lon, tags)) in c.nodes {
        coord_ids.entry((lat, lon)).or_insert(id);
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

    let mut ways: Vec<DecWay> = c
        .ways
        .into_iter()
        .map(|(id, (closed, tags, pts))| (id, closed, tags, pts))
        .collect();
    ways.sort_by_key(|w| w.0);

    for (id, closed, tags, points) in ways {
        let mut refs: Vec<u64> = Vec::with_capacity(points.len() + 1);
        for p in &points {
            let nid = *coord_ids.entry(*p).or_insert_with(|| {
                let id = next_synthetic;
                next_synthetic += 1;
                emitted.push((id, p.0, p.1));
                id
            });
            refs.push(nid);
        }
        if closed {
            if let (Some(first), Some(last)) = (refs.first().copied(), refs.last().copied()) {
                if first != last {
                    refs.push(first);
                }
            }
        }
        elements.push(OsmElement {
            r#type: "way".into(),
            id,
            lat: None,
            lon: None,
            nodes: Some(refs),
            tags: Some(tags.into_iter().collect()),
            members: Vec::new(),
        });
    }

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

    let mut rels: Vec<DecRelation> = c
        .relations
        .into_iter()
        .map(|(id, (tags, members))| (id, tags, members))
        .collect();
    rels.sort_by_key(|r| r.0);
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

/// Stored coordinates are degrees x 1e6. Out-of-globe values reach `LLPoint::new` downstream,
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
    if buf.len() < 4 || &buf[..4] != b"AOT1" {
        return Err("not an Arnis tile payload".into());
    }
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
        let y = coord(step(&mut lat, &mut r)?, MAX_LAT_E6)?;
        let x = coord(step(&mut lon, &mut r)?, MAX_LON_E6)?;
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
            let y = coord(step(&mut a, &mut r)?, MAX_LAT_E6)?;
            let x = coord(step(&mut o, &mut r)?, MAX_LON_E6)?;
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

    fn node_tile(deltas: &[(i64, i64, i64)]) -> Vec<u8> {
        let mut buf = b"AOT1".to_vec();
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
        let err = decode_err(&node_tile(&[(1, 200_000_000, 0)]));
        assert!(err.contains("out of range"), "{err}");
    }

    #[test]
    fn delta_overflow_is_refused() {
        let err = decode_err(&node_tile(&[(1, MAX_LAT_E6, 0), (1, i64::MAX, 0)]));
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
        let data = assemble(c);
        let els = data.elements_for_test();
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
        let data = assemble(c);
        let mut refs: Vec<(u64, Vec<u64>)> = data
            .elements_for_test()
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
}
