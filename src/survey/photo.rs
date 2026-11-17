//! Aerial photo tiles: only the parts an area needs, boiled down per metre.
//!
//! The Bavarian DOP20 is a tiled GeoTIFF (256 x 256 pixels, 51.2 m) whose
//! pixel grid is aligned to the metre grid at five pixels a metre, so each cell
//! is exactly a 5 x 5 pixel block and nothing is resampled. Blocks are fetched by
//! range request, so an area of a few hundred metres reads a tenth of a tile.
//! Sources with a WMS instead are asked for the same 256-pixel blocks of the same
//! 20 cm grid, several neighbouring blocks to a request, and the answer is cut
//! into blocks the same way.
//!
//! Per 1 m cell: mean excess green `(2g - r - b) / (r + g + b)`, the share of
//! its pixels that read as plants, mean brightness, mean blueness
//! `(b - max(r, g)) / 255` and the mean colour. Cached per tile with a record of
//! which 256-pixel blocks are in, so a later area fetches only what is missing.

use super::fetch::unreachable;
use super::source::{Photo, Sheet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const TILE: usize = 1000;
const CELLS: usize = TILE * TILE;
const PX_PER_M: usize = 5;
const MAGIC: &[u8; 8] = b"ARNSDOP1";

#[derive(Clone, Copy, Default)]
pub struct PhotoCell {
    /// Excess green times 200.
    pub exg: i8,
    /// Pixels out of 25 that read as plants.
    pub green: u8,
    pub lum: u8,
    /// Blueness times 200.
    pub blue: i8,
    pub rgb: [u8; 3],
    pub present: bool,
}

pub struct PhotoTile {
    pub cells: Vec<PhotoCell>,
    /// Which TIFF blocks are in, row-major.
    blocks_in: Vec<bool>,
    blocks_across: usize,
}

struct Layout {
    width: usize,
    tile_w: usize,
    tile_h: usize,
    offsets: Vec<u64>,
    counts: Vec<u64>,
    compression: u16,
    predictor: u16,
    samples: usize,
}

pub fn cache_path(root: &Path, source: &Sheet, te: i32, tn: i32) -> PathBuf {
    root.join(source.dir()).join(format!("photo_{te}_{tn}.bin"))
}

impl PhotoTile {
    fn new(blocks_across: usize) -> Self {
        PhotoTile {
            cells: vec![PhotoCell::default(); CELLS],
            blocks_in: vec![false; blocks_across * blocks_across],
            blocks_across,
        }
    }

    /// Read a cached tile a row at a time from the decompressor. `None` unless
    /// the file is whole and of this version.
    fn read(path: &Path) -> Option<Self> {
        use std::io::Read;
        let mut d = zstd::Decoder::new(std::fs::File::open(path).ok()?).ok()?;
        let mut head = [0u8; 12];
        d.read_exact(&mut head).ok()?;
        if &head[..8] != MAGIC {
            return None;
        }
        let across = u32::from_le_bytes(head[8..12].try_into().ok()?) as usize;
        if across > TILE {
            return None;
        }
        let mut blocks_in = vec![0u8; across * across];
        d.read_exact(&mut blocks_in).ok()?;
        let mut cells = Vec::with_capacity(CELLS);
        let mut row = vec![0u8; TILE * 8];
        for _ in 0..TILE {
            d.read_exact(&mut row).ok()?;
            cells.extend(row.as_chunks::<8>().0.iter().map(|c| PhotoCell {
                exg: c[0] as i8,
                green: c[1],
                lum: c[2],
                blue: c[3] as i8,
                rgb: [c[4], c[5], c[6]],
                present: c[7] != 0,
            }));
        }
        // Nothing may follow.
        if d.read(&mut [0u8; 1]).ok()? != 0 {
            return None;
        }
        Some(PhotoTile {
            cells,
            blocks_in: blocks_in.into_iter().map(|b| b != 0).collect(),
            blocks_across: across,
        })
    }

    fn write(&self, path: &Path) -> std::io::Result<()> {
        let nb = self.blocks_in.len();
        let mut raw = Vec::with_capacity(12 + nb + CELLS * 8);
        raw.extend_from_slice(MAGIC);
        raw.extend_from_slice(&(self.blocks_across as u32).to_le_bytes());
        raw.extend(self.blocks_in.iter().map(|&b| b as u8));
        for c in &self.cells {
            raw.extend_from_slice(&[
                c.exg as u8,
                c.green,
                c.lum,
                c.blue as u8,
                c.rgb[0],
                c.rgb[1],
                c.rgb[2],
                c.present as u8,
            ]);
        }
        let packed = zstd::encode_all(&raw[..], 9)?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, packed)?;
        std::fs::rename(tmp, path)
    }
}

/// The tile's photo cells covering `[c0, c1) x [r0, r1)` (metres from the tile's
/// north-west corner), fetching any block not yet cached. `None` when the photo
/// cannot be had; the classifier then goes on the laser alone. Once a request
/// finds the host unreachable, `down` is set and no more are made: the rest of
/// the photo would only wait out the same timeout.
pub fn tile(
    client: &reqwest::blocking::Client,
    root: &Path,
    source: &Sheet,
    (te, tn): (i32, i32),
    (c0, r0, c1, r1): (usize, usize, usize, usize),
    down: &AtomicBool,
) -> Option<PhotoTile> {
    if matches!(source.photo, Photo::None) {
        return None;
    }
    let path = cache_path(root, source, te, tn);
    let cached = PhotoTile::read(&path);
    let needed = |t: &PhotoTile| -> Vec<usize> {
        let mut v = Vec::new();
        let px0 = c0 * PX_PER_M / 256;
        let px1 = ((c1 * PX_PER_M).saturating_sub(1)) / 256;
        let py0 = r0 * PX_PER_M / 256;
        let py1 = ((r1 * PX_PER_M).saturating_sub(1)) / 256;
        for by in py0..=py1.min(t.blocks_across - 1) {
            for bx in px0..=px1.min(t.blocks_across - 1) {
                let i = by * t.blocks_across + bx;
                if !t.blocks_in[i] {
                    v.push(i);
                }
            }
        }
        v
    };
    if let Some(t) = &cached {
        if needed(t).is_empty() {
            return cached;
        }
    }
    if down.load(Ordering::Relaxed) || crate::net::ensure_online("aerial photos").is_err() {
        return cached;
    }
    let url = match source.photo {
        Photo::None => return cached,
        Photo::TiledTiff(url) => url(te, tn),
        Photo::Wms { url, layer } => {
            let mut t = match cached {
                Some(t) if t.blocks_across == WMS_ACROSS => t,
                _ => PhotoTile::new(WMS_ACROSS),
            };
            let todo = needed(&t);
            let failed = fetch_wms(client, source, (url, layer), (te, tn), &todo, &mut t, down);
            return finish(root, source, (te, tn), t, failed);
        }
    };
    let layout = match read_layout(client, &url) {
        Ok(l) => l,
        Err(e) => {
            if unreachable(&e) {
                down.store(true, Ordering::Relaxed);
            }
            eprintln!("Warning: aerial photo {te}_{tn} unavailable: {e}");
            return cached;
        }
    };
    // The blocks are taken to be 256 pixels square on a tile of 5000, read as
    // 3 or 4 samples a pixel, with an offset and a size for each.
    let across = layout.width.div_ceil(256);
    if layout.width != TILE * PX_PER_M
        || layout.tile_w != 256
        || layout.tile_h != 256
        || !(3..=4).contains(&layout.samples)
        || layout.offsets.len() < across * across
    {
        eprintln!("Warning: aerial photo {te}_{tn} has an unexpected layout");
        return cached;
    }
    let mut t = match cached {
        Some(t) if t.blocks_across == across => t,
        _ => PhotoTile::new(across),
    };
    let todo = needed(&t);
    let fetched: Vec<(usize, Result<Vec<u8>, String>)> = {
        use rayon::prelude::*;
        todo.par_iter()
            .map(|&i| {
                if down.load(Ordering::Relaxed) {
                    return (i, Err("host unreachable".to_string()));
                }
                let got = fetch_block(client, &url, &layout, i);
                if got.as_ref().is_err_and(|e| unreachable(e)) {
                    down.store(true, Ordering::Relaxed);
                }
                (i, got)
            })
            .collect()
    };
    let mut failed = 0;
    for (i, block) in fetched {
        match block {
            Ok(px) => {
                fill_cells(&mut t, &layout, i, &px);
                t.blocks_in[i] = true;
            }
            Err(_) => failed += 1,
        }
    }
    finish(root, source, (te, tn), t, failed)
}

/// Cache what was fetched and hand it on.
fn finish(
    root: &Path,
    source: &Sheet,
    (te, tn): (i32, i32),
    t: PhotoTile,
    failed: usize,
) -> Option<PhotoTile> {
    if failed > 0 {
        eprintln!("Warning: {failed} aerial photo blocks of {te}_{tn} failed");
        crate::net::note_uncached("aerial photos");
    }
    let _ = std::fs::create_dir_all(root.join(source.dir()));
    if let Err(e) = t.write(&cache_path(root, source, te, tn)) {
        eprintln!("Warning: aerial photo {te}_{tn} not cached: {e}");
    }
    Some(t)
}

fn get_range(
    client: &reqwest::blocking::Client,
    url: &str,
    start: u64,
    len: u64,
) -> Result<Vec<u8>, String> {
    super::fetch::get_range(client, url, start, len).map_err(|e| e.to_string())
}

/// Blocks along a tile's side on the WMS's virtual 256-pixel grid.
const WMS_ACROSS: usize = (TILE * PX_PER_M).div_ceil(256);
/// Blocks along the side of one WMS request: 2048 pixels, which every service
/// here renders.
const WMS_CHUNK: usize = 8;

/// Fetch the blocks `todo` from a WMS, a few neighbours to a request, and fold
/// them into `t`. Returns how many failed.
fn fetch_wms(
    client: &reqwest::blocking::Client,
    source: &Sheet,
    (url, layer): (&str, &str),
    (te, tn): (i32, i32),
    todo: &[usize],
    t: &mut PhotoTile,
    down: &AtomicBool,
) -> usize {
    use rayon::prelude::*;
    use std::collections::BTreeMap;
    let mut chunks: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
    for &i in todo {
        let (bx, by) = (i % WMS_ACROSS, i / WMS_ACROSS);
        chunks
            .entry((bx / WMS_CHUNK, by / WMS_CHUNK))
            .or_default()
            .push(i);
    }
    let (te0, tn1) = (f64::from(te) * 1000.0, f64::from(tn + 1) * 1000.0);
    let full = TILE * PX_PER_M;
    let layout = Layout {
        width: full,
        tile_w: 256,
        tile_h: 256,
        offsets: Vec::new(),
        counts: Vec::new(),
        compression: 1,
        predictor: 1,
        samples: 3,
    };
    // Per request: its blocks, the image if it came, and its north-west pixel.
    type Answer = (Vec<usize>, Option<image::RgbImage>, (usize, usize));
    let results: Vec<Answer> = chunks
        .into_values()
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|blocks| {
            let bx0 = blocks.iter().map(|i| i % WMS_ACROSS).min().unwrap_or(0);
            let bx1 = blocks.iter().map(|i| i % WMS_ACROSS).max().unwrap_or(0);
            let by0 = blocks.iter().map(|i| i / WMS_ACROSS).min().unwrap_or(0);
            let by1 = blocks.iter().map(|i| i / WMS_ACROSS).max().unwrap_or(0);
            let (px0, px1) = (bx0 * 256, ((bx1 + 1) * 256).min(full));
            let (py0, py1) = (by0 * 256, ((by1 + 1) * 256).min(full));
            let m = |px: usize| px as f64 / PX_PER_M as f64;
            let (w, s, e, n) = (te0 + m(px0), tn1 - m(py1), te0 + m(px1), tn1 - m(py0));
            let bbox = if source.crs.north_first() {
                format!("{s},{w},{n},{e}")
            } else {
                format!("{w},{s},{e},{n}")
            };
            let req = format!(
                "{url}?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS={layer}&STYLES=\
                 &CRS=EPSG:{}&BBOX={bbox}&WIDTH={}&HEIGHT={}&FORMAT=image/jpeg",
                source.crs.epsg(),
                px1 - px0,
                py1 - py0
            );
            // A busy WMS sometimes answers with an XML exception instead of the
            // image; ask twice more before giving the blocks up. A request that
            // got no answer at all is not repeated (the fetch has retried it).
            let mut img = None;
            for attempt in 0..3u64 {
                if down.load(Ordering::Relaxed) {
                    break;
                }
                if attempt > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(800 * attempt));
                }
                let body = match super::fetch::get_bytes(client, &req) {
                    Ok(b) => b,
                    Err(e) => {
                        if unreachable(&e.to_string()) {
                            down.store(true, Ordering::Relaxed);
                        }
                        break;
                    }
                };
                img = image::load_from_memory(&body)
                    .ok()
                    .map(|i| i.to_rgb8())
                    .filter(|i| {
                        i.width() as usize == px1 - px0 && i.height() as usize == py1 - py0
                    });
                if img.is_some() {
                    break;
                }
            }
            (blocks, img, (px0, py0))
        })
        .collect();
    let mut failed = 0;
    for (blocks, img, (px0, py0)) in results {
        let Some(img) = img else {
            failed += blocks.len();
            continue;
        };
        for i in blocks {
            let (bx, by) = (i % WMS_ACROSS, i / WMS_ACROSS);
            // The block's pixels, padded to 256 x 256 past the tile's edge like
            // a TIFF's edge tiles.
            let mut px = vec![0u8; 256 * 256 * 3];
            for y in 0..256 {
                let iy = by * 256 + y - py0;
                if iy >= img.height() as usize {
                    break;
                }
                for x in 0..256 {
                    let ix = bx * 256 + x - px0;
                    if ix >= img.width() as usize {
                        break;
                    }
                    let p = img.get_pixel(ix as u32, iy as u32).0;
                    px[(y * 256 + x) * 3..(y * 256 + x) * 3 + 3].copy_from_slice(&p);
                }
            }
            fill_cells(t, &layout, i, &px);
            t.blocks_in[i] = true;
        }
    }
    failed
}

/// Read the first IFD of a classic TIFF and the tile table it points to.
fn read_layout(client: &reqwest::blocking::Client, url: &str) -> Result<Layout, String> {
    let head = get_range(client, url, 0, 65_536)?;
    if head.len() < 8 {
        return Err("short TIFF header".into());
    }
    let le = match &head[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Err("not a TIFF".into()),
    };
    let u16_at = |b: &[u8], o: usize| {
        let a = [b[o], b[o + 1]];
        if le {
            u16::from_le_bytes(a)
        } else {
            u16::from_be_bytes(a)
        }
    };
    let u32_at = |b: &[u8], o: usize| {
        let a = [b[o], b[o + 1], b[o + 2], b[o + 3]];
        if le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        }
    };
    if u16_at(&head, 2) != 42 {
        return Err("BigTIFF is not supported here".into());
    }
    let ifd = u32_at(&head, 4) as usize;
    if ifd + 2 > head.len() {
        return Err("IFD past the first 64 KiB".into());
    }
    let entries = u16_at(&head, ifd) as usize;
    let mut l = Layout {
        width: 0,
        tile_w: 0,
        tile_h: 0,
        offsets: Vec::new(),
        counts: Vec::new(),
        compression: 1,
        predictor: 1,
        samples: 1,
    };
    let mut arrays: Vec<(u16, u16, usize, u32)> = Vec::new();
    for k in 0..entries {
        let e = ifd + 2 + k * 12;
        if e + 12 > head.len() {
            return Err("IFD past the first 64 KiB".into());
        }
        let tag = u16_at(&head, e);
        let typ = u16_at(&head, e + 2);
        let count = u32_at(&head, e + 4) as usize;
        let short = || u16_at(&head, e + 8) as usize;
        let value = || {
            if typ == 3 {
                u16_at(&head, e + 8) as usize
            } else {
                u32_at(&head, e + 8) as usize
            }
        };
        match tag {
            256 => l.width = value(),
            259 => l.compression = short() as u16,
            277 => l.samples = short(),
            317 => l.predictor = short() as u16,
            322 => l.tile_w = value(),
            323 => l.tile_h = value(),
            324 | 325 => arrays.push((tag, typ, count, u32_at(&head, e + 8))),
            _ => {}
        }
    }
    for (tag, typ, count, at) in arrays {
        let size = if typ == 3 { 2 } else { 4 };
        // A tile of 5000 pixels in blocks of 256 has 400; a count far past
        // that is not a photo tile of this kind.
        if count > 1 << 16 {
            return Err(format!("a table of {count} blocks"));
        }
        let bytes = if count * size <= 4 {
            return Err("single-tile TIFF".into());
        } else if at as usize + count * size <= head.len() {
            head[at as usize..at as usize + count * size].to_vec()
        } else {
            get_range(client, url, u64::from(at), (count * size) as u64)?
        };
        if bytes.len() < count * size {
            return Err("short block table".into());
        }
        let vals: Vec<u64> = (0..count)
            .map(|i| {
                if size == 2 {
                    u64::from(u16_at(&bytes, i * 2))
                } else {
                    u64::from(u32_at(&bytes, i * 4))
                }
            })
            .collect();
        if tag == 324 {
            l.offsets = vals;
        } else {
            l.counts = vals;
        }
    }
    if l.tile_w == 0 || l.tile_h == 0 || l.offsets.is_empty() || l.offsets.len() != l.counts.len() {
        return Err("not a tiled TIFF".into());
    }
    Ok(l)
}

/// One TIFF block's pixels, decoded to interleaved RGB(+) bytes.
fn fetch_block(
    client: &reqwest::blocking::Client,
    url: &str,
    l: &Layout,
    i: usize,
) -> Result<Vec<u8>, String> {
    let (Some(&offset), Some(&count)) = (l.offsets.get(i), l.counts.get(i)) else {
        return Err("block past the table".into());
    };
    let expected = l.tile_w * l.tile_h * l.samples;
    // A block is a few hundred kilobytes; a size past that is not one.
    if count > 4 * expected as u64 {
        return Err(format!("a block of {count} bytes"));
    }
    let data = get_range(client, url, offset, count)?;
    // Decoded only as far as a block reaches, whatever the data would expand to.
    let mut px = match l.compression {
        1 => data,
        5 => {
            let mut dec = weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
            let mut out = vec![0u8; expected];
            let (mut inp, mut filled) = (&data[..], 0);
            while filled < expected {
                let r = dec.decode_bytes(inp, &mut out[filled..]);
                filled += r.consumed_out;
                inp = &inp[r.consumed_in..];
                match r.status.map_err(|e| e.to_string())? {
                    weezl::LzwStatus::Ok if r.consumed_in + r.consumed_out > 0 => {}
                    _ => break,
                }
            }
            out.truncate(filled);
            out
        }
        8 | 32946 => {
            use std::io::Read;
            let mut out = Vec::with_capacity(expected);
            flate2::read::ZlibDecoder::new(&data[..])
                .take(expected as u64)
                .read_to_end(&mut out)
                .map_err(|e| e.to_string())?;
            out
        }
        c => return Err(format!("compression {c} not supported")),
    };
    if px.len() < expected {
        return Err("short block".into());
    }
    if l.predictor == 2 {
        let row = l.tile_w * l.samples;
        for r in 0..l.tile_h {
            for k in l.samples..row {
                let i = r * row + k;
                px[i] = px[i].wrapping_add(px[i - l.samples]);
            }
        }
    }
    Ok(px)
}

/// Fold one decoded block's pixels into the metre cells it covers. 256 pixels
/// are 51.2 m, so every block seam cuts a column and a row of cells in two; such
/// a cell is measured on the block that holds most of it (3 or more of its 5
/// pixel rows and columns), which is always exactly one of the two.
fn fill_cells(t: &mut PhotoTile, l: &Layout, i: usize, px: &[u8]) {
    let (bx, by) = (i % t.blocks_across, i / t.blocks_across);
    let (x0, y0) = (bx * l.tile_w, by * l.tile_h);
    let (x1, y1) = (x0 + l.tile_w, y0 + l.tile_h);
    let span = |c: usize, lo: usize, hi: usize| {
        let (a, b) = (c * PX_PER_M, c * PX_PER_M + PX_PER_M);
        (a.max(lo), b.min(hi))
    };
    for r in y0 / PX_PER_M..(y1.div_ceil(PX_PER_M)).min(TILE) {
        let (py0, py1) = span(r, y0, y1);
        if py1 <= py0 || py1 - py0 < 3 {
            continue;
        }
        for c in x0 / PX_PER_M..(x1.div_ceil(PX_PER_M)).min(TILE) {
            let (px0, px1) = span(c, x0, x1);
            if px1 <= px0 || px1 - px0 < 3 {
                continue;
            }
            let (mut exg, mut green, mut lum, mut blue) = (0.0f32, 0u32, 0.0f32, 0.0f32);
            let mut rgb = [0.0f32; 3];
            let mut n = 0u32;
            for py in py0..py1 {
                for pxx in px0..px1 {
                    let o = ((py - y0) * l.tile_w + (pxx - x0)) * l.samples;
                    let (rr, gg, bb) =
                        (f32::from(px[o]), f32::from(px[o + 1]), f32::from(px[o + 2]));
                    let tot = rr + gg + bb + 1e-3;
                    let e = (2.0 * gg - rr - bb) / tot;
                    exg += e;
                    if e > 0.06 {
                        green += 1;
                    }
                    lum += tot / 3.0;
                    blue += (bb - rr.max(gg)) / 255.0;
                    rgb[0] += rr;
                    rgb[1] += gg;
                    rgb[2] += bb;
                    n += 1;
                }
            }
            let nf = n as f32;
            t.cells[r * TILE + c] = PhotoCell {
                exg: (exg / nf * 200.0).round().clamp(-127.0, 127.0) as i8,
                // Kept on the 25-pixel scale whatever the share measured.
                green: ((green as f32 / nf) * 25.0).round() as u8,
                lum: (lum / nf).round().clamp(0.0, 255.0) as u8,
                blue: (blue / nf * 200.0).round().clamp(-127.0, 127.0) as i8,
                rgb: rgb.map(|v| (v / nf).round().clamp(0.0, 255.0) as u8),
                present: true,
            };
        }
    }
}
