//! PMTiles v3 archive reader, over HTTP range requests and a disk cache.
//!
//! Overture publishes its themes as single planet-scale PMTiles archives in the
//! `overturemaps-extras` bucket (`tiles/<release>/buildings.pmtiles`, ~180 GB).
//! The format is a header, a root directory, optional leaf directories and the
//! tile data, all addressable by byte range - so reading one city means reading
//! a header, one directory page and a handful of tiles, never the archive.
//!
//! Only what Arnis needs is implemented: directory lookup and tile retrieval.
//! Writing, clustering guarantees and the deduplication that `run_length`
//! expresses are read, not produced.
//!
//! Specification: <https://github.com/protomaps/PMTiles/blob/main/spec/v3/spec.md>

use reqwest::blocking::Client;
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

use super::cache;
use super::stats;

/// Fixed size of a v3 header, in bytes.
const HEADER_LEN: usize = 127;

/// First read of a new archive. Large enough that the header and the whole root
/// directory almost always arrive together (Overture's is ~14 KB), so opening an
/// archive costs one request rather than two.
const HEADER_PROBE_LEN: u64 = 16 * 1024;

/// A directory that claims more than this is not one we can use, and reserving
/// for it would be a denial of service on a corrupt or hostile archive.
const MAX_DIRECTORY_BYTES: u64 = 64 * 1024 * 1024;

/// Guards against a directory whose entry count is inconsistent with its bytes.
const MAX_DIRECTORY_ENTRIES: u64 = 8 * 1024 * 1024;

/// Largest tile body this reader will fetch. Overture's densest z14 building
/// tile is ~410 KB compressed; the entry length is a u32, so without a cap the
/// archive could ask us to download 4 GB for one tile.
const MAX_TILE_BYTES: u64 = 64 * 1024 * 1024;

/// Leaf descents before a lookup gives up. The format allows nesting; two levels
/// is all any published archive uses, and a cycle must not hang generation.
const MAX_LEAF_DEPTH: usize = 4;

/// Compression identifiers from the specification.
const COMPRESSION_NONE: u8 = 1;
const COMPRESSION_GZIP: u8 = 2;
const COMPRESSION_BROTLI: u8 = 3;
const COMPRESSION_ZSTD: u8 = 4;

/// Tile type identifier for Mapbox Vector Tiles.
const TILE_TYPE_MVT: u8 = 1;

pub type Result<T> = std::result::Result<T, String>;

/// The parts of a v3 header this reader uses.
#[derive(Debug, Clone, Copy)]
pub struct Header {
    root_offset: u64,
    root_length: u64,
    leaf_offset: u64,
    tile_data_offset: u64,
    internal_compression: u8,
    tile_compression: u8,
    pub min_zoom: u8,
    pub max_zoom: u8,
}

/// One directory entry. `run_length == 0` marks a pointer to a leaf directory
/// rather than to tile data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    tile_id: u64,
    run_length: u32,
    length: u32,
    offset: u64,
}

/// Where a tile's bytes live in the archive, and how long they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileLocation {
    pub offset: u64,
    pub length: u32,
}

fn parse_u64(buf: &[u8], at: usize) -> u64 {
    let mut arr = [0u8; 8];
    arr.copy_from_slice(&buf[at..at + 8]);
    u64::from_le_bytes(arr)
}

impl Header {
    fn parse(buf: &[u8]) -> Result<Header> {
        if buf.len() < HEADER_LEN {
            return Err(format!(
                "PMTiles header is {} bytes, expected {HEADER_LEN}",
                buf.len()
            ));
        }
        if &buf[0..7] != b"PMTiles" {
            return Err("not a PMTiles archive (bad magic)".into());
        }
        if buf[7] != 3 {
            return Err(format!("PMTiles version {} is not supported", buf[7]));
        }
        let header = Header {
            root_offset: parse_u64(buf, 8),
            root_length: parse_u64(buf, 16),
            leaf_offset: parse_u64(buf, 40),
            tile_data_offset: parse_u64(buf, 56),
            internal_compression: buf[97],
            tile_compression: buf[98],
            min_zoom: buf[100],
            max_zoom: buf[101],
        };
        if buf[99] != TILE_TYPE_MVT {
            return Err(format!(
                "archive holds tile type {}, expected Mapbox Vector Tiles",
                buf[99]
            ));
        }
        if header.root_length == 0 || header.root_length > MAX_DIRECTORY_BYTES {
            return Err(format!(
                "root directory length {} is out of range",
                header.root_length
            ));
        }
        Ok(header)
    }
}

/// Undo the archive's declared compression, refusing output past `max_output`.
///
/// The other caps in this file bound the compressed size only. A few hundred
/// kilobytes of gzip can decode to gigabytes, and these bytes come off the
/// network, so both decoders read through `take` and the result is rejected
/// once it passes the limit.
fn decompress(kind: u8, data: Vec<u8>, max_output: u64) -> Result<Vec<u8>> {
    /// Reads at most `max_output` bytes, then reports the overrun rather than
    /// returning a truncated buffer that would parse as valid-but-wrong.
    fn read_bounded(mut reader: impl Read, max_output: u64, what: &str) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        // One byte past the cap, so a stream sitting right on the limit is
        // still told apart from one that overruns it.
        reader
            .by_ref()
            .take(max_output.saturating_add(1))
            .read_to_end(&mut out)
            .map_err(|e| format!("{what} decode failed: {e}"))?;
        if out.len() as u64 > max_output {
            return Err(format!(
                "{what} stream expands past the {max_output} byte limit"
            ));
        }
        Ok(out)
    }

    match kind {
        COMPRESSION_NONE => {
            if data.len() as u64 > max_output {
                return Err(format!(
                    "uncompressed block is {} bytes, past the {max_output} byte limit",
                    data.len()
                ));
            }
            Ok(data)
        }
        COMPRESSION_GZIP => read_bounded(
            flate2::read::GzDecoder::new(data.as_slice()),
            max_output,
            "gzip",
        ),
        COMPRESSION_ZSTD => read_bounded(
            zstd::Decoder::new(data.as_slice()).map_err(|e| format!("zstd decode failed: {e}"))?,
            max_output,
            "zstd",
        ),
        COMPRESSION_BROTLI => Err("archive uses brotli, which Arnis does not link".into()),
        other => Err(format!("unknown PMTiles compression {other}")),
    }
}

// ─── Directory decoding ──────────────────────────────────────────────────

struct Varints<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Varints<'_> {
    fn next(&mut self) -> Result<u64> {
        let mut value = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = *self.buf.get(self.pos).ok_or("directory ended mid-varint")?;
            self.pos += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            if shift >= 64 {
                return Err("directory varint longer than 64 bits".into());
            }
        }
    }
}

/// Decode a directory: a count, then four parallel arrays.
///
/// `offset` is delta-encoded against the previous entry: a stored zero means
/// "immediately after the previous entry", and any other value is the real
/// offset plus one.
fn decode_directory(buf: &[u8]) -> Result<Vec<Entry>> {
    let mut reader = Varints { buf, pos: 0 };
    let count = reader.next()?;
    if count > MAX_DIRECTORY_ENTRIES || count > buf.len() as u64 {
        return Err(format!("directory claims {count} entries"));
    }
    let count = count as usize;
    let mut entries = vec![
        Entry {
            tile_id: 0,
            run_length: 0,
            length: 0,
            offset: 0,
        };
        count
    ];

    let mut tile_id = 0u64;
    for entry in entries.iter_mut() {
        tile_id = tile_id
            .checked_add(reader.next()?)
            .ok_or("tile id overflowed")?;
        entry.tile_id = tile_id;
    }
    for entry in entries.iter_mut() {
        entry.run_length = u32::try_from(reader.next()?).map_err(|_| "run length out of range")?;
    }
    for entry in entries.iter_mut() {
        entry.length = u32::try_from(reader.next()?).map_err(|_| "entry length out of range")?;
    }
    for index in 0..count {
        let raw = reader.next()?;
        entries[index].offset = if raw == 0 {
            let previous = index
                .checked_sub(1)
                .ok_or("first directory entry uses the contiguous-offset marker")?;
            entries[previous]
                .offset
                .checked_add(u64::from(entries[previous].length))
                .ok_or("entry offset overflowed")?
        } else {
            raw - 1
        };
    }
    Ok(entries)
}

/// The entry covering `tile_id`, or `None` when the archive has no such tile.
///
/// Entries are sorted by tile id, so this is a binary search for the last entry
/// that starts at or before the target; a run then has to actually reach it.
fn find_entry(entries: &[Entry], tile_id: u64) -> Option<Entry> {
    let index = entries
        .partition_point(|e| e.tile_id <= tile_id)
        .checked_sub(1)?;
    let entry = entries[index];
    if entry.run_length == 0 {
        // A leaf pointer covers everything from here to the next entry.
        return Some(entry);
    }
    (tile_id < entry.tile_id.saturating_add(u64::from(entry.run_length))).then_some(entry)
}

// ─── Tile addressing ─────────────────────────────────────────────────────

/// PMTiles orders tiles along a Hilbert curve, zoom level by zoom level, so a
/// tile's id is the count of every tile in lower zooms plus its position on the
/// curve at its own zoom. Neighbouring tiles land close together in the
/// archive, so a city reads as a handful of nearby ranges.
pub fn zxy_to_tile_id(z: u8, x: u32, y: u32) -> Result<u64> {
    if z > 31 {
        return Err(format!("zoom {z} is out of range"));
    }
    let n = 1u64 << z;
    if u64::from(x) >= n || u64::from(y) >= n {
        return Err(format!("tile {z}/{x}/{y} is outside the zoom level"));
    }

    // Tiles in every zoom below this one: (4^z - 1) / 3.
    let mut id = ((1u64 << (2 * u32::from(z))) - 1) / 3;

    let (mut tx, mut ty) = (u64::from(x), u64::from(y));
    let mut side = n >> 1;
    while side > 0 {
        let rx = u64::from(tx & side != 0);
        let ry = u64::from(ty & side != 0);
        id += side * side * ((3 * rx) ^ ry);
        // Rotate the quadrant so the curve stays continuous.
        if ry == 0 {
            if rx == 1 {
                tx = side.wrapping_sub(1).wrapping_sub(tx);
                ty = side.wrapping_sub(1).wrapping_sub(ty);
            }
            std::mem::swap(&mut tx, &mut ty);
        }
        side >>= 1;
    }
    Ok(id)
}

/// Web-Mercator tile containing a coordinate at the given zoom.
pub fn lonlat_to_tile(lon: f64, lat: f64, z: u8) -> (u32, u32) {
    let n = f64::from(1u32 << z);
    let x = ((lon + 180.0) / 360.0 * n).floor();
    // Clamped to the Mercator limit so a pole never produces an infinity.
    let lat = lat.clamp(-85.051_128_78, 85.051_128_78).to_radians();
    let y = ((1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * n).floor();
    let last = (1u32 << z) - 1;
    ((x.max(0.0) as u32).min(last), (y.max(0.0) as u32).min(last))
}

/// Longitude of a tile-local x coordinate, where `fraction` is `local_x / extent`.
pub fn tile_x_to_lon(z: u8, tile_x: u32, fraction: f64) -> f64 {
    let n = f64::from(1u32 << z);
    (f64::from(tile_x) + fraction) / n * 360.0 - 180.0
}

/// Latitude of a tile-local y coordinate, where `fraction` is `local_y / extent`.
pub fn tile_y_to_lat(z: u8, tile_y: u32, fraction: f64) -> f64 {
    let n = f64::from(1u32 << z);
    let t = std::f64::consts::PI * (1.0 - 2.0 * (f64::from(tile_y) + fraction) / n);
    t.sinh().atan().to_degrees()
}

// ─── Archive ─────────────────────────────────────────────────────────────

/// An opened archive: the header, the root directory, and whichever leaf
/// directories have been read so far.
pub struct Archive {
    url: String,
    header: Header,
    root: Vec<Entry>,
    leaves: HashMap<(u64, u32), Vec<Entry>>,
    cache_dir: Option<PathBuf>,
}

impl Archive {
    /// Open the archive at `url`, caching the header and root directory under
    /// `cache_dir`.
    ///
    /// The first read covers the header and, in practice, the whole root
    /// directory. Only an archive with an unusually large root costs a second
    /// request, and only on the run that first opens it.
    pub fn open(client: &Client, url: &str, cache_dir: Option<PathBuf>) -> Result<Archive> {
        let header_path = cache_dir.as_ref().map(|d| d.join("header.bin"));
        let root_path = cache_dir.as_ref().map(|d| d.join("root.bin"));

        let cached = header_path
            .as_ref()
            .and_then(|p| cache::read(p))
            .zip(root_path.as_ref().and_then(|p| cache::read(p)));

        let (header_bytes, root_bytes) = match cached {
            Some((header_bytes, root_bytes)) => {
                stats::record_cached((header_bytes.len() + root_bytes.len()) as u64);
                (header_bytes, root_bytes)
            }
            None => {
                let probe = fetch_range(client, url, 0, HEADER_PROBE_LEN)?;
                let header = Header::parse(&probe)?;
                let root_end = header
                    .root_offset
                    .checked_add(header.root_length)
                    .ok_or("root directory range overflows")?;

                let root_bytes = if root_end <= probe.len() as u64 {
                    probe[header.root_offset as usize..root_end as usize].to_vec()
                } else {
                    fetch_range(client, url, header.root_offset, header.root_length)?
                };
                let header_bytes = probe[..HEADER_LEN].to_vec();

                if let (Some(hp), Some(rp)) = (&header_path, &root_path) {
                    cache::write_atomic(hp, &header_bytes);
                    cache::write_atomic(rp, &root_bytes);
                }
                (header_bytes, root_bytes)
            }
        };

        let header = Header::parse(&header_bytes)?;
        let root = decode_directory(&decompress(
            header.internal_compression,
            root_bytes,
            MAX_DIRECTORY_BYTES,
        )?)?;
        Ok(Archive {
            url: url.to_string(),
            header,
            root,
            leaves: HashMap::new(),
            cache_dir,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Resolve a tile to its byte range, descending through leaf directories.
    ///
    /// Takes `&mut self` because a leaf directory read is memoised here: a city
    /// is a few dozen tiles behind one or two leaves, so resolving them all up
    /// front costs one or two requests and lets the tile bodies be fetched in
    /// parallel afterwards.
    pub fn locate(
        &mut self,
        client: &Client,
        z: u8,
        x: u32,
        y: u32,
    ) -> Result<Option<TileLocation>> {
        let tile_id = zxy_to_tile_id(z, x, y)?;
        let mut entries: &[Entry] = &self.root;
        // Owned storage for a leaf that was just read, so the borrow above can
        // be rebound to it.
        for _ in 0..MAX_LEAF_DEPTH {
            let Some(entry) = find_entry(entries, tile_id) else {
                return Ok(None);
            };
            if entry.run_length > 0 {
                let offset = self
                    .header
                    .tile_data_offset
                    .checked_add(entry.offset)
                    .ok_or("tile offset overflows")?;
                return Ok(Some(TileLocation {
                    offset,
                    length: entry.length,
                }));
            }
            // A leaf pointer with no length would loop forever on itself.
            if entry.length == 0 {
                return Ok(None);
            }
            // `length` is a u32, so the file may ask for up to 4 GB. Refuse
            // before the request rather than after the download.
            if u64::from(entry.length) > MAX_DIRECTORY_BYTES {
                return Err(format!(
                    "leaf directory claims {} bytes, past the {MAX_DIRECTORY_BYTES} cap",
                    entry.length
                ));
            }
            let key = (entry.offset, entry.length);
            if !self.leaves.contains_key(&key) {
                let absolute = self
                    .header
                    .leaf_offset
                    .checked_add(entry.offset)
                    .ok_or("leaf offset overflows")?;
                let raw = self.read_cached(
                    client,
                    absolute,
                    u64::from(entry.length),
                    self.cache_dir.as_ref().map(|d| {
                        d.join("leaf")
                            .join(format!("{}_{}.bin", entry.offset, entry.length))
                    }),
                )?;
                let decoded = decode_directory(&decompress(
                    self.header.internal_compression,
                    raw,
                    MAX_DIRECTORY_BYTES,
                )?)?;
                self.leaves.insert(key, decoded);
            }
            entries = &self.leaves[&key];
        }
        Err("PMTiles directory nesting is deeper than this reader follows".into())
    }

    /// Fetch and decompress one tile's bytes.
    ///
    /// Takes `&self`, so tiles resolved by [`Archive::locate`] can be fetched
    /// from several threads at once.
    pub fn tile(
        &self,
        client: &Client,
        z: u8,
        x: u32,
        y: u32,
        location: TileLocation,
    ) -> Result<Vec<u8>> {
        if location.length == 0 {
            return Ok(Vec::new());
        }
        // Same reasoning as the leaf cap: a tile length is a u32 the archive
        // chooses, and no vector tile is hundreds of megabytes.
        if u64::from(location.length) > MAX_TILE_BYTES {
            return Err(format!(
                "tile {z}/{x}/{y} claims {} bytes, past the {MAX_TILE_BYTES} cap",
                location.length
            ));
        }
        let path = self.cache_dir.as_ref().map(|d| {
            d.join("t")
                .join(z.to_string())
                .join(x.to_string())
                .join(format!("{y}.bin"))
        });
        let raw = self.read_cached(client, location.offset, u64::from(location.length), path)?;
        decompress(self.header.tile_compression, raw, MAX_TILE_BYTES)
    }

    /// Read a byte range, preferring the cache and populating it on a miss.
    ///
    /// A cached file whose length disagrees with the range is treated as a miss
    /// rather than trusted, so a truncated write from an older build can only
    /// cost one refetch.
    fn read_cached(
        &self,
        client: &Client,
        offset: u64,
        length: u64,
        path: Option<PathBuf>,
    ) -> Result<Vec<u8>> {
        if let Some(path) = &path {
            if let Some(bytes) = cache::read(path) {
                if bytes.len() as u64 == length {
                    stats::record_cached(length);
                    return Ok(bytes);
                }
            }
        }
        let bytes = fetch_range(client, &self.url, offset, length)?;
        if let Some(path) = &path {
            cache::write_atomic(path, &bytes);
        }
        Ok(bytes)
    }
}

/// Attempts per range read. A dropped tile takes a square kilometre of
/// buildings with it, so a transient failure must not settle it.
const RANGE_ATTEMPTS: u32 = 3;

/// Fetch `[offset, offset + length)` over HTTP, retrying transient failures.
fn fetch_range(client: &Client, url: &str, offset: u64, length: u64) -> Result<Vec<u8>> {
    if length == 0 {
        return Ok(Vec::new());
    }
    // Offsets and lengths come out of the archive's own header and directories.
    // An overflow here would wrap the range into a small one and quietly return
    // the wrong bytes, so it is an error rather than a saturation.
    let end = offset
        .checked_add(length - 1)
        .ok_or_else(|| format!("range {offset}+{length} overflows the archive"))?;
    let mut last_error = String::new();

    for attempt in 0..RANGE_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(500 << (attempt - 1)));
        }
        stats::record_request();
        let response = match client
            .get(url)
            .header("Range", format!("bytes={offset}-{end}"))
            .send()
        {
            Ok(response) => response,
            Err(e) => {
                last_error = format!("range request to {url} failed: {e}");
                continue;
            }
        };

        let status = response.status();
        // A 200 means the server ignored the range and is about to send the
        // whole archive. Refusing is the only safe answer at 180 GB.
        if status.as_u16() != 206 {
            last_error = format!("HTTP {status} fetching range from {url} (expected 206)");
            if !(status.is_server_error() || status.as_u16() == 429) {
                break;
            }
            continue;
        }

        match response.bytes() {
            Ok(body) => {
                stats::record_network(body.len() as u64);
                return Ok(body.to_vec());
            }
            Err(e) => last_error = format!("range body from {url} could not be read: {e}"),
        }
    }
    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    fn serialize_directory(entries: &[Entry]) -> Vec<u8> {
        let mut out = varint(entries.len() as u64);
        let mut last = 0u64;
        for e in entries {
            out.extend(varint(e.tile_id - last));
            last = e.tile_id;
        }
        for e in entries {
            out.extend(varint(u64::from(e.run_length)));
        }
        for e in entries {
            out.extend(varint(u64::from(e.length)));
        }
        for (i, e) in entries.iter().enumerate() {
            let contiguous =
                i > 0 && e.offset == entries[i - 1].offset + u64::from(entries[i - 1].length);
            out.extend(varint(if contiguous { 0 } else { e.offset + 1 }));
        }
        out
    }

    fn entry(tile_id: u64, run_length: u32, length: u32, offset: u64) -> Entry {
        Entry {
            tile_id,
            run_length,
            length,
            offset,
        }
    }

    #[test]
    fn tile_ids_match_the_specifications_examples() {
        // The curve starts at the single zoom-0 tile and covers each zoom in turn.
        assert_eq!(zxy_to_tile_id(0, 0, 0).unwrap(), 0);
        assert_eq!(zxy_to_tile_id(1, 0, 0).unwrap(), 1);
        assert_eq!(zxy_to_tile_id(1, 0, 1).unwrap(), 2);
        assert_eq!(zxy_to_tile_id(1, 1, 1).unwrap(), 3);
        assert_eq!(zxy_to_tile_id(1, 1, 0).unwrap(), 4);
        assert_eq!(zxy_to_tile_id(2, 0, 0).unwrap(), 5);

        // Every tile of a zoom level maps to a distinct id in that level's block.
        let mut seen = std::collections::HashSet::new();
        for x in 0..8 {
            for y in 0..8 {
                let id = zxy_to_tile_id(3, x, y).unwrap();
                assert!((21..85).contains(&id), "z3 id {id} outside its block");
                assert!(seen.insert(id), "duplicate id {id}");
            }
        }
        assert_eq!(seen.len(), 64);

        assert!(zxy_to_tile_id(1, 2, 0).is_err(), "x outside the zoom level");
        assert!(zxy_to_tile_id(32, 0, 0).is_err(), "zoom out of range");
    }

    #[test]
    fn the_munich_tile_id_matches_the_live_archive() {
        // Verified against Overture's published buildings.pmtiles: the z14 tile
        // covering Munich's Altstadt resolves to this id in the real root
        // directory. Pins both the tile lookup and the Hilbert numbering.
        let (x, y) = lonlat_to_tile(11.575, 48.137, 14);
        assert_eq!((x, y), (8718, 5685));
        assert_eq!(zxy_to_tile_id(14, x, y).unwrap(), 317_195_426);
    }

    #[test]
    fn tile_coordinates_round_trip_through_the_projection() {
        for (lon, lat) in [
            (11.575, 48.137),
            (-73.984, 40.754),
            (0.0, 0.0),
            (139.7, 35.68),
        ] {
            let z = 14;
            let (x, y) = lonlat_to_tile(lon, lat, z);
            // The point must fall inside the tile it was assigned to.
            let west = tile_x_to_lon(z, x, 0.0);
            let east = tile_x_to_lon(z, x, 1.0);
            let north = tile_y_to_lat(z, y, 0.0);
            let south = tile_y_to_lat(z, y, 1.0);
            assert!(west <= lon && lon < east, "{lon} outside [{west}, {east})");
            assert!(
                south <= lat && lat <= north,
                "{lat} outside [{south}, {north}]"
            );
        }
    }

    #[test]
    fn poles_and_antimeridian_stay_inside_the_grid() {
        let last = (1u32 << 14) - 1;
        assert_eq!(lonlat_to_tile(180.0, 0.0, 14).0, last);
        assert_eq!(lonlat_to_tile(-180.0, 0.0, 14).0, 0);
        assert_eq!(lonlat_to_tile(0.0, 90.0, 14).1, 0);
        assert_eq!(lonlat_to_tile(0.0, -90.0, 14).1, last);
    }

    #[test]
    fn directory_round_trips_including_contiguous_offsets() {
        let entries = vec![
            entry(0, 1, 100, 0),
            // Contiguous with the previous entry, so its offset is stored as 0.
            entry(5, 1, 250, 100),
            entry(9, 0, 64, 4096),
            entry(20, 3, 10, 9000),
        ];
        let decoded = decode_directory(&serialize_directory(&entries)).unwrap();
        assert_eq!(decoded, entries);
    }

    #[test]
    fn lookup_honours_runs_and_leaf_pointers() {
        let entries = vec![
            entry(10, 3, 5, 0),
            entry(20, 0, 64, 128),
            entry(40, 1, 5, 0),
        ];

        // Inside the run.
        assert_eq!(find_entry(&entries, 10), Some(entries[0]));
        assert_eq!(find_entry(&entries, 12), Some(entries[0]));
        // Past the run's end but before the next entry: the archive has no tile.
        assert_eq!(find_entry(&entries, 13), None);
        // Below the first entry.
        assert_eq!(find_entry(&entries, 9), None);
        // A leaf pointer covers everything up to the next entry.
        assert_eq!(find_entry(&entries, 20), Some(entries[1]));
        assert_eq!(find_entry(&entries, 39), Some(entries[1]));
        assert_eq!(find_entry(&entries, 40), Some(entries[2]));
        assert_eq!(find_entry(&entries, 41), None);
    }

    #[test]
    fn a_malformed_directory_is_rejected_without_panicking() {
        // Entry count far past what the bytes could hold.
        assert!(decode_directory(&varint(u64::MAX)).is_err());
        // Truncated arrays.
        let good = serialize_directory(&[entry(1, 1, 2, 3), entry(9, 1, 2, 40)]);
        for cut in 0..good.len() {
            let _ = decode_directory(&good[..cut]);
        }
        // A first entry using the contiguous marker has nothing to be contiguous with.
        let mut broken = varint(1);
        broken.extend(varint(0)); // tile id
        broken.extend(varint(1)); // run length
        broken.extend(varint(1)); // length
        broken.extend(varint(0)); // offset marker
        assert!(decode_directory(&broken).is_err());
    }

    #[test]
    fn a_header_is_validated_before_any_range_is_derived_from_it() {
        let mut buf = vec![0u8; HEADER_LEN];
        buf[0..7].copy_from_slice(b"PMTiles");
        buf[7] = 3;
        buf[99] = TILE_TYPE_MVT;
        buf[16..24].copy_from_slice(&1024u64.to_le_bytes()); // root length
        assert!(Header::parse(&buf).is_ok());

        let mut bad_magic = buf.clone();
        bad_magic[0] = b'X';
        assert!(Header::parse(&bad_magic).is_err());

        let mut bad_version = buf.clone();
        bad_version[7] = 2;
        assert!(Header::parse(&bad_version).is_err());

        let mut bad_type = buf.clone();
        bad_type[99] = 2; // PNG
        assert!(Header::parse(&bad_type).is_err());

        let mut huge_root = buf.clone();
        huge_root[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(Header::parse(&huge_root).is_err());

        assert!(Header::parse(&buf[..HEADER_LEN - 1]).is_err());
    }

    #[test]
    fn brotli_is_refused_rather_than_silently_producing_nothing() {
        assert!(decompress(COMPRESSION_BROTLI, vec![1, 2, 3], 1024).is_err());
        assert!(decompress(99, vec![1, 2, 3], 1024).is_err());
        assert_eq!(
            decompress(COMPRESSION_NONE, vec![1, 2, 3], 1024).unwrap(),
            vec![1, 2, 3]
        );

        // A stream that expands past its cap is refused, not truncated: a
        // short buffer would decode as a valid-looking, wrong directory.
        let bomb = {
            use std::io::Write;
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
            encoder.write_all(&vec![0u8; 512 * 1024]).unwrap();
            encoder.finish().unwrap()
        };
        assert!(bomb.len() < 4096, "the test bomb must be small compressed");
        assert!(decompress(COMPRESSION_GZIP, bomb.clone(), 4096).is_err());
        assert_eq!(
            decompress(COMPRESSION_GZIP, bomb, 1024 * 1024)
                .unwrap()
                .len(),
            512 * 1024
        );
    }
}
