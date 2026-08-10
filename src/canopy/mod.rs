//! Meta / WRI global canopy height map, used to decide where trees stand.
//!
//! Zoom-9 quadkey tiles, 65536 square, uint8 canopy top in metres. BigTIFF with
//! one Deflate strip per row and horizontal differencing, not a COG, so a window
//! means range-fetching the strip index and then only the rows it covers.
//! Licensed CC BY 4.0, see NOTICE.

use std::io::Read;
use std::path::PathBuf;

use rayon::prelude::*;

use crate::coordinate_system::geographic::LLBBox;

const TILE_ZOOM: u32 = 9;
const TILE_PX: u64 = 65536;
const MERCATOR_WORLD: f64 = 20_037_508.342_789_244;
const BASE_URL: &str =
    "https://dataforgood-fb-data.s3.amazonaws.com/forests/v1/alsgedi_global_v6_float/chm";
const CACHE_DIR: &str = "arnis-canopy-cache";

/// Caps how much of the download is held at once, at one request per batch.
const FETCH_BATCH_BYTES: u64 = 16 << 20;

/// No measurement here. A measured zero means bare ground.
pub const CANOPY_NODATA: u8 = 255;

/// Shortest canopy that counts as a tree. Below this is scrub and hedges.
pub const CANOPY_MIN_M: u8 = 3;

/// Leaf-bearing columns one placed tree adds, which is what turns a canopy
/// fraction into a trunk probability. Calibrated over the Olympiapark: 27.4%
/// placed against 27.2% real, both over ground a tree can root in.
const COLUMNS_PER_TREE: f64 = 52.0;

/// Procedural crowns are smaller than the schematic pack's.
const COLUMNS_PER_TREE_LEGACY: f64 = 25.0;

/// A slot never becomes a certainty, so even solid canopy keeps some variation.
const SLOT_P_MAX: f64 = 0.85;

/// Canopy heights on the same grid as the land cover, so one set of ratios
/// addresses both.
#[derive(Clone)]
pub struct CanopyData {
    /// Row-major, `width * height`, metres, `CANOPY_NODATA` where unmeasured.
    grid: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

impl CanopyData {
    pub fn from_grid(grid: Vec<u8>, width: usize, height: usize) -> Self {
        CanopyData {
            grid,
            width,
            height,
        }
    }

    #[inline(always)]
    pub fn at(&self, gx: usize, gz: usize) -> u8 {
        if gx >= self.width || gz >= self.height {
            return CANOPY_NODATA;
        }
        self.grid[gz * self.width + gx]
    }

    /// Covered cells, canopy cells, mean height, tallest. For the log line.
    pub fn stats(&self) -> (usize, usize, f64, u8) {
        let mut covered = 0usize;
        let mut canopy = 0usize;
        let mut sum = 0u64;
        let mut max = 0u8;
        for &h in &self.grid {
            if h == CANOPY_NODATA {
                continue;
            }
            covered += 1;
            if h >= CANOPY_MIN_M {
                canopy += 1;
                sum += u64::from(h);
                max = max.max(h);
            }
        }
        let mean = if canopy > 0 {
            sum as f64 / canopy as f64
        } else {
            0.0
        };
        (covered, canopy, mean, max)
    }
}

/// Trunk probability for one lattice cell of `spacing` squared columns. The map
/// measures crowns, not trunks, so the fraction is divided by what one tree
/// covers.
pub fn slot_probability(fraction: f64, spacing: i32, schematic_pack: bool) -> f64 {
    let per_tree = if schematic_pack {
        COLUMNS_PER_TREE
    } else {
        COLUMNS_PER_TREE_LEGACY
    };
    let area = (spacing * spacing) as f64;
    (fraction * area / per_tree).clamp(0.0, SLOT_P_MAX)
}

/// Tile indices of the zoom-9 tile holding this point.
fn tile_xy(lat: f64, lon: f64) -> (i64, i64) {
    let n = f64::from(1u32 << TILE_ZOOM);
    let xt = (((lon + 180.0) / 360.0 * n) as i64).clamp(0, (1i64 << TILE_ZOOM) - 1);
    let lat_rad = lat.to_radians();
    let yt = (((1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) / 2.0 * n)
        as i64)
        .clamp(0, (1i64 << TILE_ZOOM) - 1);
    (xt, yt)
}

/// Web Mercator quadkey naming the tile file.
fn quadkey_of(xt: i64, yt: i64) -> String {
    let mut out = String::with_capacity(TILE_ZOOM as usize);
    for i in (1..=TILE_ZOOM).rev() {
        let mask = 1i64 << (i - 1);
        let mut digit = 0u8;
        if xt & mask != 0 {
            digit += 1;
        }
        if yt & mask != 0 {
            digit += 2;
        }
        out.push((b'0' + digit) as char);
    }
    out
}

fn cache_dir() -> PathBuf {
    match dirs::cache_dir() {
        Some(d) => d.join(CACHE_DIR),
        None => PathBuf::from(format!("./{CACHE_DIR}")),
    }
}

/// Clear the cached strip tables. Called from the GUI cache-clean command.
pub fn clear_canopy_cache() -> crate::elevation::cache::CacheClearStats {
    crate::elevation::cache::clear_cache_dir(&cache_dir())
}

fn merc_x(lon: f64) -> f64 {
    MERCATOR_WORLD * lon / 180.0
}

fn merc_y(lat: f64) -> f64 {
    MERCATOR_WORLD
        * (std::f64::consts::FRAC_PI_4 + lat.to_radians() / 2.0)
            .tan()
            .ln()
        / std::f64::consts::PI
}

/// The strip table of one tile: where each image row lives in the file.
struct StripIndex {
    offsets: Vec<u64>,
    counts: Vec<u64>,
}

fn read_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn read_u64(b: &[u8], at: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(v)
}

fn fetch_range(
    client: &reqwest::blocking::Client,
    url: &str,
    start: u64,
    length: u64,
) -> Result<Vec<u8>, String> {
    if length == 0 {
        return Ok(Vec::new());
    }
    let end = start + length - 1;
    let response = client
        .get(url)
        .header("Range", format!("bytes={start}-{end}"))
        .send()
        .map_err(|e| e.to_string())?;
    // A 200 here would mean the server ignored the range and is sending 450 MB.
    if response.status().as_u16() != 206 {
        return Err(format!("HTTP {} (expected 206)", response.status()));
    }
    response
        .bytes()
        .map(|b| b.to_vec())
        .map_err(|e| e.to_string())
}

impl StripIndex {
    /// 1 MB of offsets and counts, little-endian, offsets first.
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity((TILE_PX as usize) * 16);
        for v in self.offsets.iter().chain(self.counts.iter()) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let n = TILE_PX as usize;
        if bytes.len() != n * 16 {
            return None;
        }
        let at = |i: usize| read_u64(bytes, i * 8);
        Some(StripIndex {
            offsets: (0..n).map(at).collect(),
            counts: (n..n * 2).map(at).collect(),
        })
    }
}

/// Parse the BigTIFF header and pull the strip tables. An unexpected layout is
/// rejected rather than guessed at, since guessing yields nonsense heights.
fn read_strip_index(client: &reqwest::blocking::Client, url: &str) -> Result<StripIndex, String> {
    let head = fetch_range(client, url, 0, 4096)?;
    if head.len() < 16 || &head[0..2] != b"II" || read_u16(&head, 2) != 43 {
        return Err("not a little-endian BigTIFF".into());
    }
    let ifd_at = read_u64(&head, 8);
    if ifd_at + 8 > head.len() as u64 {
        return Err("IFD past the header window".into());
    }
    let entries = read_u64(&head, ifd_at as usize);
    let mut tags: std::collections::HashMap<u16, (u16, u64, u64)> =
        std::collections::HashMap::new();
    for i in 0..entries {
        let p = ifd_at as usize + 8 + (i as usize) * 20;
        if p + 20 > head.len() {
            return Err("IFD entry past the header window".into());
        }
        let tag = read_u16(&head, p);
        let typ = read_u16(&head, p + 2);
        let count = read_u64(&head, p + 4);
        let value = read_u64(&head, p + 12);
        tags.insert(tag, (typ, count, value));
    }
    let scalar = |tag: u16| tags.get(&tag).map(|&(_, _, v)| v);

    let width = scalar(256).unwrap_or(0);
    let height = scalar(257).unwrap_or(0);
    if width != TILE_PX || height != TILE_PX {
        return Err(format!("unexpected tile size {width}x{height}"));
    }
    if scalar(258) != Some(8) || scalar(277).unwrap_or(1) != 1 {
        return Err("not single-band 8-bit".into());
    }
    if scalar(259) != Some(8) {
        return Err("not Deflate compressed".into());
    }
    if scalar(317) != Some(2) {
        return Err("predictor is not horizontal differencing".into());
    }
    if scalar(278) != Some(1) {
        return Err("more than one row per strip".into());
    }

    let (_, off_count, off_at) = *tags.get(&273).ok_or("no StripOffsets")?;
    let (_, cnt_count, cnt_at) = *tags.get(&279).ok_or("no StripByteCounts")?;
    if off_count != TILE_PX || cnt_count != TILE_PX {
        return Err("strip table length does not match the image".into());
    }
    let raw_off = fetch_range(client, url, off_at, TILE_PX * 8)?;
    let raw_cnt = fetch_range(client, url, cnt_at, TILE_PX * 8)?;
    if raw_off.len() < (TILE_PX * 8) as usize || raw_cnt.len() < (TILE_PX * 8) as usize {
        return Err("short strip table".into());
    }
    let to_u64s =
        |b: &[u8]| -> Vec<u64> { (0..TILE_PX as usize).map(|i| read_u64(b, i * 8)).collect() };
    Ok(StripIndex {
        offsets: to_u64s(&raw_off),
        counts: to_u64s(&raw_cnt),
    })
}

/// A fixed 1 MB per tile that never changes, so it is cached on disk.
fn cached_strip_index(
    client: &reqwest::blocking::Client,
    url: &str,
    key: &str,
) -> Result<StripIndex, String> {
    let dir = cache_dir();
    let path = dir.join(format!("{key}.idx"));
    if let Ok(bytes) = std::fs::read(&path) {
        if let Some(index) = StripIndex::decode(&bytes) {
            return Ok(index);
        }
        let _ = std::fs::remove_file(&path);
    }
    let index = read_strip_index(client, url)?;
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(&path, index.encode());
    }
    Ok(index)
}

/// Inflate one image row, undo predictor 2, keep the sampled columns.
///
/// Differencing runs from column zero, so everything up to the rightmost wanted
/// column has to be accumulated even for a narrow window. `cols` is ascending
/// and may repeat where the grid is finer than the source.
fn inflate_sampled(
    raw: &[u8],
    cols: &[usize],
    scratch: &mut Vec<u8>,
    out: &mut [u8],
) -> Result<(), String> {
    let Some(&last) = cols.last() else {
        return Ok(());
    };
    scratch.clear();
    flate2::read::ZlibDecoder::new(raw)
        .read_to_end(scratch)
        .map_err(|e| e.to_string())?;
    if scratch.len() <= last {
        return Err("row shorter than the window".into());
    }
    let mut acc: u8 = 0;
    let mut k = 0usize;
    for (i, v) in scratch.iter().enumerate().take(last + 1) {
        acc = acc.wrapping_add(*v);
        while k < cols.len() && cols[k] == i {
            out[k] = acc;
            k += 1;
        }
    }
    Ok(())
}

/// Where a tile sits in Mercator space and how big one of its pixels is.
fn tile_geometry(xt: i64, yt: i64) -> (f64, f64, f64) {
    let tile_m = 2.0 * MERCATOR_WORLD / f64::from(1u32 << TILE_ZOOM);
    let res = tile_m / TILE_PX as f64;
    (
        -MERCATOR_WORLD + xt as f64 * tile_m,
        MERCATOR_WORLD - yt as f64 * tile_m,
        res,
    )
}

/// Source pixel each output cell samples, and where this tile's run starts.
/// Both axes are monotonic, so the run is contiguous.
fn axis_mapping(count: usize, lo: f64, step: f64, origin: f64, res: f64) -> (usize, Vec<usize>) {
    let mut start = None;
    let mut idx = Vec::new();
    for i in 0..count {
        let p = ((lo + step * i as f64) - origin) / res;
        if p < 0.0 || p >= TILE_PX as f64 {
            if start.is_some() {
                break;
            }
            continue;
        }
        start.get_or_insert(i);
        idx.push(p as usize);
    }
    (start.unwrap_or(0), idx)
}

/// Read this tile's share of the area straight into `grid`. Only rows the grid
/// samples are inflated, and only sampled columns kept, so neither cost scales
/// past what the grid can represent.
#[allow(clippy::too_many_arguments)]
fn fill_from_tile(
    client: &reqwest::blocking::Client,
    xt: i64,
    yt: i64,
    bbox: &LLBBox,
    grid_width: usize,
    grid_height: usize,
    grid: &mut [u8],
) -> Result<usize, String> {
    let key = quadkey_of(xt, yt);
    let url = format!("{BASE_URL}/{key}.tif");
    let (min_x, max_y, res) = tile_geometry(xt, yt);

    // Linear in lat/lon like the land cover, so each cell converts to Mercator.
    let (x_lo, x_hi) = (merc_x(bbox.min().lng()), merc_x(bbox.max().lng()));
    let x_step = if grid_width > 1 {
        (x_hi - x_lo) / (grid_width - 1) as f64
    } else {
        0.0
    };
    let (gx0, cols) = axis_mapping(grid_width, x_lo, x_step, min_x, res);

    let (lat_hi, lat_lo) = (bbox.max().lat(), bbox.min().lat());
    let lat_step = if grid_height > 1 {
        (lat_lo - lat_hi) / (grid_height - 1) as f64
    } else {
        0.0
    };
    let mut gz0 = None;
    let mut rows: Vec<usize> = Vec::new();
    for gz in 0..grid_height {
        let p = (max_y - merc_y(lat_hi + lat_step * gz as f64)) / res;
        if p < 0.0 || p >= TILE_PX as f64 {
            if gz0.is_some() {
                break;
            }
            continue;
        }
        gz0.get_or_insert(gz);
        rows.push(p as usize);
    }
    let gz0 = gz0.unwrap_or(0);
    if cols.is_empty() || rows.is_empty() {
        return Err("tile covers none of the area".into());
    }

    // Several output rows can land on one source row when the grid is coarser.
    let mut unique: Vec<usize> = rows.clone();
    unique.dedup();

    let index = cached_strip_index(client, &url, &key)?;
    let w = cols.len();
    let (mut first, mut out_i) = (0usize, 0usize);
    while first < unique.len() {
        // Strips sit in row order, so one range per batch beats one per row.
        let lo = index.offsets[unique[first]];
        let mut end = first + 1;
        while end < unique.len() {
            let r = unique[end];
            if index.offsets[r] + index.counts[r] - lo > FETCH_BATCH_BYTES {
                break;
            }
            end += 1;
        }
        let last = unique[end - 1];
        let hi = index.offsets[last] + index.counts[last];
        if hi <= lo {
            return Err("strip table is not ascending".into());
        }
        let blob = fetch_range(client, &url, lo, hi - lo)?;

        // Inflating a row decompresses all 65536 columns, so this dominates.
        let mut batch = vec![CANOPY_NODATA; w * (end - first)];
        batch
            .par_chunks_mut(w)
            .enumerate()
            .try_for_each_init(Vec::new, |scratch, (i, out)| {
                let r = unique[first + i];
                let start = (index.offsets[r] - lo) as usize;
                let len = index.counts[r] as usize;
                if start + len > blob.len() {
                    return Err("strip past the fetched range".to_string());
                }
                inflate_sampled(&blob[start..start + len], &cols, scratch, out)
            })?;

        while out_i < rows.len() && rows[out_i] <= last {
            let src = (unique.partition_point(|&u| u < rows[out_i]) - first) * w;
            let dst = (gz0 + out_i) * grid_width + gx0;
            grid[dst..dst + w].copy_from_slice(&batch[src..src + w]);
            out_i += 1;
        }
        first = end;
    }
    Ok(out_i * w)
}

/// Fetch canopy heights for `bbox` onto a `grid_width` x `grid_height` grid.
/// `None` on any failure, which falls back to the land-cover behaviour.
pub fn fetch_canopy_data(
    bbox: &LLBBox,
    grid_width: usize,
    grid_height: usize,
) -> Option<CanopyData> {
    if grid_width == 0 || grid_height == 0 {
        return None;
    }
    // No progress emit, this runs behind the land cover and elevation fetches.
    println!("Fetching canopy height data (Meta/WRI 1m global canopy height)...");

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .ok()?;

    // Nearest-neighbour, not averaging: at 0.8 m the source is finer than the
    // block grid and averaging would smear a crown into the lawn.
    let mut grid = vec![CANOPY_NODATA; grid_width * grid_height];
    let (x0, y0) = tile_xy(bbox.max().lat(), bbox.min().lng());
    let (x1, y1) = tile_xy(bbox.min().lat(), bbox.max().lng());
    let mut tiles = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for yt in y0.min(y1)..=y0.max(y1) {
        for xt in x0.min(x1)..=x0.max(x1) {
            match fill_from_tile(&client, xt, yt, bbox, grid_width, grid_height, &mut grid) {
                Ok(_) => tiles += 1,
                Err(e) => failures.push(format!("{}: {}", quadkey_of(xt, yt), e)),
            }
        }
    }

    let data = CanopyData::from_grid(grid, grid_width, grid_height);
    let (covered, canopy, mean, max) = data.stats();
    if covered == 0 {
        let why = failures
            .first()
            .map(|s| s.chars().take(200).collect::<String>())
            .unwrap_or_else(|| "no tiles".to_string());
        eprintln!(
            "Warning: Canopy height data unavailable ({why}). Falling back to land cover for trees."
        );
        return None;
    }
    println!(
        "Canopy height data loaded: {} tile{}, canopy over {}m on {:.1}% of the area, mean {:.1}m, tallest {}m",
        tiles,
        if tiles == 1 { "" } else { "s" },
        CANOPY_MIN_M,
        100.0 * canopy as f64 / covered as f64,
        mean,
        max,
    );
    if !failures.is_empty() {
        eprintln!(
            "Warning: {} canopy tile(s) unavailable; those areas fall back to land cover.",
            failures.len()
        );
    }
    Some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quadkey(lat: f64, lon: f64) -> String {
        let (xt, yt) = tile_xy(lat, lon);
        quadkey_of(xt, yt)
    }

    #[test]
    fn munich_quadkey() {
        // Verified against the published tile index.
        assert_eq!(quadkey(48.1731, 11.5465), "120230002");
        assert_eq!(quadkey(48.1731, 11.5465).len(), TILE_ZOOM as usize);
    }

    #[test]
    fn quadkeys_stay_in_range_worldwide() {
        for &(lat, lon) in &[(48.17, 11.55), (-33.9, 151.2), (0.0, 0.0), (60.0, -120.0)] {
            let key = quadkey(lat, lon);
            assert_eq!(key.len(), TILE_ZOOM as usize);
            assert!(key.bytes().all(|b| (b'0'..=b'3').contains(&b)), "{key}");
        }
        // Null Island sits on the corner where all four quadrants meet.
        assert_eq!(quadkey(0.0, 0.0), "300000000");
    }

    #[test]
    fn strip_index_round_trips_through_the_cache() {
        let n = TILE_PX as usize;
        let index = StripIndex {
            offsets: (0..n as u64).map(|i| i * 7 + 1_000).collect(),
            counts: (0..n as u64).map(|i| i % 97 + 1).collect(),
        };
        let back = StripIndex::decode(&index.encode()).expect("decodes");
        assert_eq!(back.offsets, index.offsets);
        assert_eq!(back.counts, index.counts);
        // A truncated or foreign file is rejected instead of misread.
        assert!(StripIndex::decode(&index.encode()[..64]).is_none());
    }

    #[test]
    fn predictor_is_undone_across_the_row() {
        // Heights 5, 7, 6 encode as deltas 5, 2, -1 with wrapping.
        let deltas: Vec<u8> = vec![5, 2, 255];
        let mut raw = Vec::new();
        {
            use std::io::Write;
            let mut e = flate2::write::ZlibEncoder::new(&mut raw, flate2::Compression::fast());
            e.write_all(&deltas).unwrap();
            e.finish().unwrap();
        }
        let mut scratch = Vec::new();
        let mut out = [0u8; 3];
        inflate_sampled(&raw, &[0, 1, 2], &mut scratch, &mut out).unwrap();
        assert_eq!(out, [5, 7, 6]);
        // A window still accumulates from column zero.
        let mut win = [0u8; 1];
        inflate_sampled(&raw, &[2], &mut scratch, &mut win).unwrap();
        assert_eq!(win, [6]);
        // A grid finer than the source repeats a column instead of skipping it.
        let mut up = [0u8; 4];
        inflate_sampled(&raw, &[0, 1, 1, 2], &mut scratch, &mut up).unwrap();
        assert_eq!(up, [5, 7, 7, 6]);
    }

    #[test]
    fn axis_mapping_owns_a_contiguous_run() {
        // Ten cells one pixel apart, starting three pixels before the tile.
        let (start, idx) = axis_mapping(10, -3.0, 1.0, 0.0, 1.0);
        assert_eq!(start, 3);
        assert_eq!(idx, vec![0, 1, 2, 3, 4, 5, 6]);
        // Wholly outside the tile yields nothing to read.
        let (_, none) = axis_mapping(4, -100.0, 1.0, 0.0, 1.0);
        assert!(none.is_empty());
    }

    #[test]
    fn slot_probability_reproduces_the_canopy_fraction() {
        // A tree covers COLUMNS_PER_TREE columns, so p * k / area == fraction.
        for &f in &[0.0, 0.19, 0.26, 0.44] {
            let p = slot_probability(f, 5, true);
            let realized = p * COLUMNS_PER_TREE / 25.0;
            assert!((realized - f).abs() < 1e-9, "f={f} realized={realized}");
        }
        // Saturating canopy is capped so a slot never becomes a certainty.
        assert_eq!(slot_probability(1.0, 7, true), SLOT_P_MAX);
    }

    /// Live fetch over the Olympiapark. Ignored by default so the suite stays
    /// offline; run with `cargo test canopy -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn fetches_the_olympiapark_window() {
        let bbox = LLBBox::new(48.169, 11.539, 48.180, 11.560).unwrap();
        let data = fetch_canopy_data(&bbox, 400, 300).expect("canopy fetch");
        let (covered, canopy, mean, max) = data.stats();
        assert_eq!(covered, 400 * 300, "the whole window is measured");
        // Measured on the source tile: about a fifth of the park is canopy,
        // and no crown taller than a real treetop.
        let fraction = canopy as f64 / covered as f64;
        assert!((0.10..0.35).contains(&fraction), "fraction {fraction}");
        assert!((5.0..20.0).contains(&mean), "mean {mean}");
        assert!((20..60).contains(&max), "max {max}");
    }

    #[test]
    fn nodata_is_not_zero_height() {
        let d = CanopyData::from_grid(vec![0, CANOPY_NODATA, 12], 3, 1);
        assert_eq!(d.at(0, 0), 0, "measured bare ground");
        assert_eq!(d.at(1, 0), CANOPY_NODATA, "unmeasured");
        // Out of range reads as unmeasured, never as bare ground.
        assert_eq!(d.at(9, 0), CANOPY_NODATA);
        let (covered, canopy, _, max) = d.stats();
        assert_eq!((covered, canopy, max), (2, 1, 12));
    }
}
