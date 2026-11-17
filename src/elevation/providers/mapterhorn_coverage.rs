//! Where Mapterhorn's terrain comes from a fine survey: the coverage polygon of each of its
//! sources, a vector tile layer it publishes, and each source's resolution, from its
//! attribution list.
//!
//! The tiles cannot tell this themselves. Mapterhorn builds the levels past z12 for every
//! z12 macrotile a fine source touches, and paints GLO-30 in at that zoom where the survey
//! stops, so a z16 tile along a border or a coast may hold no laser data at all.

use crate::coordinate_system::geographic::LLBBox;
use rayon::prelude::*;
use std::path::{Path, PathBuf};

const COVERAGE_URL: &str = "https://single-archive-tiles.mapterhorn.com/coverage/{z}/{x}/{y}.mvt";
const ATTRIBUTION_URL: &str = "https://download.mapterhorn.com/attribution.json";
const ATTRIBUTION_FILE: &str = "attribution.json";
/// Deepest zoom of the coverage layer.
const COVERAGE_MAX_ZOOM: u8 = 14;
/// Most coverage tiles an area reads; a larger area reads them at a coarser zoom.
const MAX_COVERAGE_TILES: usize = 16;
/// Sources this fine or finer are laser-scanned terrain models. The coarser ones are
/// GLO-30 and national models of 10 m and up, made from photogrammetry or contours.
const FINE_SOURCE_M: f64 = 5.0;
/// Cached coverage and attribution are fetched again after this age, as sources are added.
const MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60;
const MAX_RETRIES: u32 = 3;

/// The coverage of Mapterhorn's fine sources, in normalized web mercator (x east, y south,
/// both 0..1 across the world). One entry per source polygon, as its rings.
pub(crate) struct FineCoverage {
    polygons: Vec<Vec<mvt::Ring>>,
}

impl FineCoverage {
    /// For each of `ys` (normalized mercator, one per row), which of `xs` (ascending) lie
    /// inside a fine source's polygon, row by row. A polygon fills by the even-odd rule, so
    /// its holes stay out; the polygons of different sources add up.
    pub(crate) fn covered(&self, xs: &[f64], ys: &[f64]) -> Vec<Vec<bool>> {
        ys.par_iter()
            .map(|&y| {
                let mut row = vec![false; xs.len()];
                let mut crossings: Vec<f64> = Vec::new();
                for polygon in &self.polygons {
                    crossings.clear();
                    for ring in polygon {
                        for (i, &(x0, y0)) in ring.iter().enumerate() {
                            let (x1, y1) = ring[(i + 1) % ring.len()];
                            // Half-open in y, so a vertex on the row counts once.
                            if (y0 <= y) != (y1 <= y) {
                                crossings.push(x0 + (y - y0) / (y1 - y0) * (x1 - x0));
                            }
                        }
                    }
                    crossings.sort_by(f64::total_cmp);
                    for span in crossings.as_chunks::<2>().0 {
                        let from = xs.partition_point(|&x| x < span[0]);
                        let to = xs.partition_point(|&x| x < span[1]);
                        row[from..to].fill(true);
                    }
                }
                row
            })
            .collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.polygons.is_empty()
    }
}

/// Zoom and tile range (x0, x1, y0, y1) of the coverage tiles over `bbox`.
fn coverage_tiles(bbox: &LLBBox) -> (u8, u32, u32, u32, u32) {
    let mut z = COVERAGE_MAX_ZOOM;
    loop {
        let n = 2f64.powi(i32::from(z));
        let max = (1u64 << z) as f64 - 1.0;
        let tx = |lng: f64| ((lng + 180.0) / 360.0 * n).floor().clamp(0.0, max) as u32;
        let ty = |lat: f64| (merc_y(lat) * n).floor().clamp(0.0, max) as u32;
        let (x0, x1) = (tx(bbox.min().lng()), tx(bbox.max().lng()));
        let (y0, y1) = (ty(bbox.max().lat()), ty(bbox.min().lat()));
        let count = (x1 - x0 + 1) as usize * (y1 - y0 + 1) as usize;
        if count <= MAX_COVERAGE_TILES || z == 0 {
            return (z, x0, x1, y0, y1);
        }
        z -= 1;
    }
}

/// Normalized web mercator y of a latitude, 0 at the north edge of the map.
pub(crate) fn merc_y(lat: f64) -> f64 {
    (1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0
}

fn tile_path(dir: &Path, z: u8, x: u32, y: u32) -> PathBuf {
    dir.join(format!("coverage_z{z}_x{x}_y{y}.mvt"))
}

/// The files reading the coverage of `bbox` takes from `dir`.
pub(crate) fn cache_files(dir: &Path, bbox: &LLBBox) -> Vec<PathBuf> {
    let (z, x0, x1, y0, y1) = coverage_tiles(bbox);
    let mut files = vec![dir.join(ATTRIBUTION_FILE)];
    for x in x0..=x1 {
        for y in y0..=y1 {
            files.push(tile_path(dir, z, x, y));
        }
    }
    files
}

/// The coverage of Mapterhorn's fine sources over `bbox`, from the cache in `dir` or the
/// network. An error leaves the caller to treat all of the area as coarse.
pub(crate) fn fetch(bbox: &LLBBox, dir: &Path) -> Result<FineCoverage, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!(
            "Arnis/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/louis-e/arnis)"
        ))
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let attribution =
        cached_or_download(&client, &dir.join(ATTRIBUTION_FILE), ATTRIBUTION_URL, |b| {
            fine_sources(b).map(|_| ())
        })?;
    let fine = fine_sources(&attribution)?;
    let (z, x0, x1, y0, y1) = coverage_tiles(bbox);
    let mut polygons = Vec::new();
    for x in x0..=x1 {
        for y in y0..=y1 {
            let url = COVERAGE_URL
                .replace("{z}", &z.to_string())
                .replace("{x}", &x.to_string())
                .replace("{y}", &y.to_string());
            let bytes = cached_or_download(&client, &tile_path(dir, z, x, y), &url, |b| {
                mvt::polygons(b, |_| true).map(|_| ())
            })?;
            let tile = mvt::polygons(&bytes, |source| fine.iter().any(|s| s == source))?;
            let n = 2f64.powi(i32::from(z));
            for (extent, rings) in tile {
                let to_world = |(px, py): (f64, f64)| {
                    (
                        (f64::from(x) + px / extent) / n,
                        (f64::from(y) + py / extent) / n,
                    )
                };
                polygons.push(
                    rings
                        .into_iter()
                        .map(|ring| ring.into_iter().map(to_world).collect())
                        .collect(),
                );
            }
        }
    }
    Ok(FineCoverage { polygons })
}

/// Names of the sources at `FINE_SOURCE_M` or finer, from the attribution list.
fn fine_sources(attribution: &[u8]) -> Result<Vec<String>, String> {
    let list: Vec<serde_json::Value> =
        serde_json::from_slice(attribution).map_err(|e| format!("attribution list: {e}"))?;
    let fine: Vec<String> = list
        .iter()
        .filter_map(|item| {
            let resolution = item.get("resolution").and_then(|r| {
                r.as_f64()
                    .or_else(|| r.as_str().and_then(|s| s.trim().parse().ok()))
            })?;
            let source = item.get("source")?.as_str()?;
            (resolution > 0.0 && resolution <= FINE_SOURCE_M).then(|| source.to_string())
        })
        .collect();
    if fine.is_empty() {
        return Err("attribution list names no fine source".to_string());
    }
    Ok(fine)
}

/// The bytes cached at `path`, unless older than `MAX_AGE_SECS` while online; else
/// downloaded from `url`, checked with `valid` and cached. A 404 caches as empty.
fn cached_or_download(
    client: &reqwest::blocking::Client,
    path: &Path,
    url: &str,
    valid: impl Fn(&[u8]) -> Result<(), String>,
) -> Result<Vec<u8>, String> {
    if let Ok(meta) = std::fs::metadata(path) {
        let fresh = crate::net::offline()
            || meta
                .modified()
                .ok()
                .and_then(|m| std::time::SystemTime::now().duration_since(m).ok())
                .is_some_and(|age| age.as_secs() < MAX_AGE_SECS);
        if fresh {
            if let Ok(bytes) = std::fs::read(path) {
                if valid(&bytes).is_ok() {
                    return Ok(bytes);
                }
            }
        }
    }
    crate::net::ensure_online("elevation coverage (Mapterhorn)")?;
    let mut last_error = String::new();
    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(500 << (attempt - 1)));
        }
        let _permit = crate::net::request_permit();
        let response = match client.get(url).send() {
            Ok(r) => r,
            Err(e) => {
                last_error = e.to_string();
                continue;
            }
        };
        let status = response.status();
        let bytes = if status.as_u16() == 404 {
            Vec::new()
        } else if status.is_success() {
            match response.bytes() {
                Ok(b) => b.to_vec(),
                Err(e) => {
                    last_error = e.to_string();
                    continue;
                }
            }
        } else {
            last_error = format!("HTTP {status} from {url}");
            continue;
        };
        if let Err(e) = valid(&bytes) {
            last_error = e;
            continue;
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        if std::fs::write(&tmp, &bytes).is_ok() && std::fs::rename(&tmp, path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        return Ok(bytes);
    }
    Err(format!("{url}: {last_error}"))
}

/// Just enough of the Mapbox Vector Tile format to read polygons and a string property.
mod mvt {
    /// A closed ring of points.
    pub(super) type Ring = Vec<(f64, f64)>;
    /// A polygon of a tile: the tile's extent, and its rings in tile pixels, y down.
    pub(super) type TilePolygon = (f64, Vec<Ring>);

    /// Polygons of the `coverage` layer whose `source` property passes `keep`, as
    /// (extent, rings) with rings in tile pixels, y down.
    pub(super) fn polygons(
        tile: &[u8],
        keep: impl Fn(&str) -> bool,
    ) -> Result<Vec<TilePolygon>, String> {
        let mut out = Vec::new();
        let mut pb = Reader::new(tile);
        while let Some((field, wire)) = pb.key()? {
            if field != 3 || wire != 2 {
                pb.skip(wire)?;
                continue;
            }
            let layer = pb.bytes()?;
            read_layer(layer, &keep, &mut out)?;
        }
        Ok(out)
    }

    fn read_layer(
        layer: &[u8],
        keep: &impl Fn(&str) -> bool,
        out: &mut Vec<TilePolygon>,
    ) -> Result<(), String> {
        let mut name = "";
        let mut features: Vec<&[u8]> = Vec::new();
        let mut keys: Vec<&str> = Vec::new();
        let mut values: Vec<Option<&str>> = Vec::new();
        let mut extent = 4096.0;
        let mut pb = Reader::new(layer);
        while let Some((field, wire)) = pb.key()? {
            match (field, wire) {
                (1, 2) => name = pb.str()?,
                (2, 2) => features.push(pb.bytes()?),
                (3, 2) => keys.push(pb.str()?),
                (4, 2) => values.push(string_value(pb.bytes()?)?),
                (5, 0) => extent = pb.varint()? as f64,
                _ => pb.skip(wire)?,
            }
        }
        if name != "coverage" || extent <= 0.0 {
            return Ok(());
        }
        for feature in features {
            let mut tags: Vec<u64> = Vec::new();
            let mut kind = 0;
            let mut geometry: Vec<u64> = Vec::new();
            let mut pb = Reader::new(feature);
            while let Some((field, wire)) = pb.key()? {
                match (field, wire) {
                    (2, 2) => tags.extend(pb.packed()?),
                    (2, 0) => tags.push(pb.varint()?),
                    (3, 0) => kind = pb.varint()?,
                    (4, 2) => geometry.extend(pb.packed()?),
                    (4, 0) => geometry.push(pb.varint()?),
                    _ => pb.skip(wire)?,
                }
            }
            let source = tags.as_chunks::<2>().0.iter().find_map(|kv| {
                (keys.get(kv[0] as usize) == Some(&"source"))
                    .then(|| values.get(kv[1] as usize).copied().flatten())
                    .flatten()
            });
            // Type 3 is a polygon.
            if kind != 3 || !source.is_some_and(keep) {
                continue;
            }
            let rings = rings(&geometry)?;
            if !rings.is_empty() {
                out.push((extent, rings));
            }
        }
        Ok(())
    }

    fn string_value(value: &[u8]) -> Result<Option<&str>, String> {
        let mut pb = Reader::new(value);
        let mut s = None;
        while let Some((field, wire)) = pb.key()? {
            if (field, wire) == (1, 2) {
                s = Some(pb.str()?);
            } else {
                pb.skip(wire)?;
            }
        }
        Ok(s)
    }

    /// The rings a polygon's command stream draws.
    fn rings(geometry: &[u64]) -> Result<Vec<Ring>, String> {
        let zigzag = |v: u64| ((v >> 1) as i64) ^ -((v & 1) as i64);
        let (mut x, mut y) = (0i64, 0i64);
        let mut rings: Vec<Ring> = Vec::new();
        let mut ring: Ring = Vec::new();
        let mut i = 0;
        while i < geometry.len() {
            let command = geometry[i];
            i += 1;
            let (id, count) = (command & 7, (command >> 3) as usize);
            match id {
                1 | 2 => {
                    if id == 1 && ring.len() >= 3 {
                        rings.push(std::mem::take(&mut ring));
                    } else if id == 1 {
                        ring.clear();
                    }
                    for _ in 0..count {
                        let (dx, dy) = match (geometry.get(i), geometry.get(i + 1)) {
                            (Some(&dx), Some(&dy)) => (dx, dy),
                            _ => return Err("vector tile geometry cut short".to_string()),
                        };
                        i += 2;
                        x += zigzag(dx);
                        y += zigzag(dy);
                        ring.push((x as f64, y as f64));
                    }
                }
                7 => {
                    if ring.len() >= 3 {
                        rings.push(std::mem::take(&mut ring));
                    }
                    ring.clear();
                }
                _ => return Err(format!("vector tile command {id}")),
            }
        }
        if ring.len() >= 3 {
            rings.push(ring);
        }
        Ok(rings)
    }

    struct Reader<'a> {
        buf: &'a [u8],
        pos: usize,
    }

    impl<'a> Reader<'a> {
        fn new(buf: &'a [u8]) -> Self {
            Self { buf, pos: 0 }
        }

        fn varint(&mut self) -> Result<u64, String> {
            let mut v = 0u64;
            for shift in (0..64).step_by(7) {
                let Some(&b) = self.buf.get(self.pos) else {
                    return Err("vector tile cut short".to_string());
                };
                self.pos += 1;
                v |= u64::from(b & 0x7f) << shift;
                if b & 0x80 == 0 {
                    return Ok(v);
                }
            }
            Err("vector tile varint too long".to_string())
        }

        /// The next field number and wire type, or `None` at the end.
        fn key(&mut self) -> Result<Option<(u64, u64)>, String> {
            if self.pos >= self.buf.len() {
                return Ok(None);
            }
            let k = self.varint()?;
            Ok(Some((k >> 3, k & 7)))
        }

        fn bytes(&mut self) -> Result<&'a [u8], String> {
            let len = self.varint()? as usize;
            let end = self
                .pos
                .checked_add(len)
                .filter(|&e| e <= self.buf.len())
                .ok_or("vector tile field runs past its end")?;
            let b = &self.buf[self.pos..end];
            self.pos = end;
            Ok(b)
        }

        fn str(&mut self) -> Result<&'a str, String> {
            std::str::from_utf8(self.bytes()?).map_err(|e| e.to_string())
        }

        fn packed(&mut self) -> Result<Vec<u64>, String> {
            let mut inner = Reader::new(self.bytes()?);
            let mut out = Vec::new();
            while inner.pos < inner.buf.len() {
                out.push(inner.varint()?);
            }
            Ok(out)
        }

        fn skip(&mut self, wire: u64) -> Result<(), String> {
            match wire {
                0 => self.varint().map(|_| ()),
                1 => self.advance(8),
                2 => self.bytes().map(|_| ()),
                5 => self.advance(4),
                _ => Err(format!("vector tile wire type {wire}")),
            }
        }

        fn advance(&mut self, n: usize) -> Result<(), String> {
            if self.pos + n > self.buf.len() {
                return Err("vector tile cut short".to_string());
            }
            self.pos += n;
            Ok(())
        }
    }

    #[cfg(test)]
    pub(super) mod encode {
        //! A writer for the same subset, for tests.
        fn varint(out: &mut Vec<u8>, mut v: u64) {
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

        fn field(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
            varint(out, field << 3 | 2);
            varint(out, bytes.len() as u64);
            out.extend_from_slice(bytes);
        }

        fn packed(values: &[u64]) -> Vec<u8> {
            let mut out = Vec::new();
            for &v in values {
                varint(&mut out, v);
            }
            out
        }

        /// A source's polygon: its name and rings in tile pixels.
        pub(crate) type SourcePolygon<'a> = (&'a str, Vec<Vec<(i64, i64)>>);

        /// A `coverage` layer of polygons.
        pub(crate) fn coverage_tile(extent: u32, polygons: &[SourcePolygon<'_>]) -> Vec<u8> {
            let zig = |v: i64| ((v << 1) ^ (v >> 63)) as u64;
            let mut sources: Vec<&str> = Vec::new();
            let mut layer = Vec::new();
            field(&mut layer, 1, b"coverage");
            for (source, rings) in polygons {
                let idx = sources.iter().position(|s| s == source).unwrap_or_else(|| {
                    sources.push(source);
                    sources.len() - 1
                });
                let mut geometry = Vec::new();
                let (mut cx, mut cy) = (0i64, 0i64);
                for ring in rings {
                    let (x, y) = ring[0];
                    geometry.extend([1 | 1 << 3, zig(x - cx), zig(y - cy)]);
                    (cx, cy) = (x, y);
                    geometry.push(2 | ((ring.len() - 1) as u64) << 3);
                    for &(x, y) in &ring[1..] {
                        geometry.extend([zig(x - cx), zig(y - cy)]);
                        (cx, cy) = (x, y);
                    }
                    geometry.push(7 | 1 << 3);
                }
                let mut feature = Vec::new();
                field(&mut feature, 2, &packed(&[0, idx as u64]));
                varint(&mut feature, 3 << 3);
                varint(&mut feature, 3);
                field(&mut feature, 4, &packed(&geometry));
                field(&mut layer, 2, &feature);
            }
            field(&mut layer, 3, b"source");
            for source in sources {
                let mut value = Vec::new();
                field(&mut value, 1, source.as_bytes());
                field(&mut layer, 4, &value);
            }
            varint(&mut layer, 5 << 3);
            varint(&mut layer, u64::from(extent));
            let mut tile = Vec::new();
            field(&mut tile, 3, &layer);
            tile
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coverage_tile_keeps_only_fine_sources_and_their_holes() {
        // A fine source's square with a hole, and a coarse source over all of it.
        let tile = mvt::encode::coverage_tile(
            4096,
            &[
                (
                    "debayern",
                    vec![
                        vec![(0, 0), (2048, 0), (2048, 2048), (0, 2048)],
                        vec![(512, 512), (512, 1024), (1024, 1024), (1024, 512)],
                    ],
                ),
                (
                    "glo30",
                    vec![vec![(0, 0), (4096, 0), (4096, 4096), (0, 4096)]],
                ),
            ],
        );
        let polys = mvt::polygons(&tile, |s| s == "debayern").unwrap();
        assert_eq!(polys.len(), 1);
        let (extent, rings) = &polys[0];
        assert_eq!(*extent, 4096.0);
        assert_eq!(rings.len(), 2);
        let cov = FineCoverage {
            polygons: vec![rings
                .iter()
                .map(|r| r.iter().map(|&(x, y)| (x / 4096.0, y / 4096.0)).collect())
                .collect()],
        };
        let xs: Vec<f64> = (0..8).map(|i| (f64::from(i) + 0.5) / 8.0).collect();
        let rows = cov.covered(&xs, &[0.0625, 0.1875, 0.75]);
        assert_eq!(
            rows[0],
            [true, true, true, true, false, false, false, false]
        );
        // Through the hole.
        assert_eq!(
            rows[1],
            [true, false, true, true, false, false, false, false]
        );
        assert_eq!(rows[2], [false; 8]);
    }

    #[test]
    fn fine_sources_come_from_the_attribution_resolutions() {
        let list = br#"[{"source":"glo30","resolution":30.0},{"source":"debayern","resolution":1},
            {"source":"tinitaly","resolution":"10"},{"source":"at10","resolution":5.0}]"#;
        assert_eq!(fine_sources(list).unwrap(), ["debayern", "at10"]);
        assert!(fine_sources(br#"[{"source":"glo30","resolution":30}]"#).is_err());
        assert!(fine_sources(b"<html>").is_err());
    }

    #[test]
    fn a_large_area_reads_few_coverage_tiles() {
        let small = LLBBox::new(49.0189, 12.0933, 49.0241, 12.1011).unwrap();
        let (z, x0, x1, y0, y1) = coverage_tiles(&small);
        assert_eq!(z, COVERAGE_MAX_ZOOM);
        assert!((x1 - x0 + 1) * (y1 - y0 + 1) <= 4);
        let large = LLBBox::new(47.0, 10.0, 49.0, 13.0).unwrap();
        let (z, x0, x1, y0, y1) = coverage_tiles(&large);
        assert!(z < 10);
        assert!(((x1 - x0 + 1) * (y1 - y0 + 1)) as usize <= MAX_COVERAGE_TILES);
        assert_eq!(
            cache_files(Path::new("c"), &small)[0],
            Path::new("c").join(ATTRIBUTION_FILE)
        );
    }
}
