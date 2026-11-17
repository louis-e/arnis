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

use super::fetch::{unreachable, Failure};
use super::source::{Photo, Sheet, SheetAt};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
    height: usize,
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
        Photo::Sheets { sheet, px_per_m } => {
            let mut t = match cached {
                Some(t) if t.blocks_across == WMS_ACROSS => t,
                _ => PhotoTile::new(WMS_ACROSS),
            };
            let todo = needed(&t);
            let kept = SheetCache::default();
            let failed = fetch_rendered((te, tn), &todo, &mut t, true, &|rect, size| {
                Some(render_sheets(
                    client,
                    (sheet, px_per_m),
                    &kept,
                    rect,
                    size,
                    down,
                ))
            });
            return finish(root, source, (te, tn), t, failed);
        }
    };
    let layout = match read_layout(client, &url) {
        Ok(l) => l,
        Err(e) => {
            let e = e.to_string();
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
                fill_cells(&mut t, &layout, i, &px, None);
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
    let get = |(w, s, e, n): (f64, f64, f64, f64), (width, height): (usize, usize)| {
        let bbox = if source.crs.north_first() {
            format!("{s},{w},{n},{e}")
        } else {
            format!("{w},{s},{e},{n}")
        };
        let req = format!(
            "{url}?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS={layer}&STYLES=\
             &CRS=EPSG:{}&BBOX={bbox}&WIDTH={width}&HEIGHT={height}&FORMAT=image/jpeg",
            source.crs.epsg(),
        );
        // A busy WMS sometimes answers with an XML exception instead of the
        // image, or with an error; ask twice more before giving the blocks
        // up. A host that cannot be reached is not asked again.
        for attempt in 0..3u64 {
            if down.load(Ordering::Relaxed) {
                return None;
            }
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(800 * attempt));
            }
            let body = match super::fetch::get_bytes(client, &req) {
                Ok(b) => b,
                Err(e) if unreachable(&e.to_string()) => {
                    down.store(true, Ordering::Relaxed);
                    return None;
                }
                Err(_) => continue,
            };
            let img = image::load_from_memory(&body)
                .ok()
                .map(|i| i.to_rgb8())
                .filter(|i| i.width() as usize == width && i.height() as usize == height);
            if img.is_some() {
                return img;
            }
        }
        None
    };
    fetch_rendered((te, tn), todo, t, false, &|rect, size| {
        get(rect, size).map(|img| Drawn { img, state: None })
    })
}

/// How a pixel of a drawn rectangle came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Px {
    Came,
    /// No photo here: no sheet holds the point, or the survey has none of
    /// it. So it would be every time.
    Absent,
    /// Not had this time.
    Failed,
}

/// A rectangle drawn, and how each of its pixels came out, row-major; `None`
/// when all came.
struct Drawn {
    img: image::RgbImage,
    state: Option<Vec<Px>>,
}

/// Draws a rectangle of the grid (west, south, east, north) at a size in
/// pixels; `None` when none of it came.
type Render<'a> = dyn Fn((f64, f64, f64, f64), (usize, usize)) -> Option<Drawn> + Sync + 'a;

/// Fetch the blocks `todo` as images of a few neighbours at a time, `render`
/// drawing each rectangle of the grid (west, south, east, north) at the size
/// asked, and fold them into `t`: side by side, or `in_order`, one after
/// another a row at a time (as sheets are read, whose blocks neighbours
/// share). A block any pixel of which failed is left for a later run; one
/// with pixels that have no photo is done with, the cells they touch without
/// photo. Returns how many failed.
fn fetch_rendered(
    (te, tn): (i32, i32),
    todo: &[usize],
    t: &mut PhotoTile,
    in_order: bool,
    render: &Render<'_>,
) -> usize {
    use rayon::prelude::*;
    use std::collections::BTreeMap;
    // By row, then column.
    let mut chunks: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
    for &i in todo {
        let (bx, by) = (i % WMS_ACROSS, i / WMS_ACROSS);
        chunks
            .entry((by / WMS_CHUNK, bx / WMS_CHUNK))
            .or_default()
            .push(i);
    }
    let (te0, tn1) = (f64::from(te) * 1000.0, f64::from(tn + 1) * 1000.0);
    let full = TILE * PX_PER_M;
    let layout = Layout {
        width: full,
        height: full,
        tile_w: 256,
        tile_h: 256,
        offsets: Vec::new(),
        counts: Vec::new(),
        compression: 1,
        predictor: 1,
        samples: 3,
    };
    // Per request: its blocks, what was drawn, and its north-west pixel.
    type Answer = (Vec<usize>, Option<Drawn>, (usize, usize));
    let draw = |blocks: Vec<usize>| -> Answer {
        let bx0 = blocks.iter().map(|i| i % WMS_ACROSS).min().unwrap_or(0);
        let bx1 = blocks.iter().map(|i| i % WMS_ACROSS).max().unwrap_or(0);
        let by0 = blocks.iter().map(|i| i / WMS_ACROSS).min().unwrap_or(0);
        let by1 = blocks.iter().map(|i| i / WMS_ACROSS).max().unwrap_or(0);
        let (px0, px1) = (bx0 * 256, ((bx1 + 1) * 256).min(full));
        let (py0, py1) = (by0 * 256, ((by1 + 1) * 256).min(full));
        let m = |px: usize| px as f64 / PX_PER_M as f64;
        let rect = (te0 + m(px0), tn1 - m(py1), te0 + m(px1), tn1 - m(py0));
        let (w, h) = (px1 - px0, py1 - py0);
        let drawn = render(rect, (w, h)).filter(|d| {
            d.img.width() as usize == w
                && d.img.height() as usize == h
                && d.state.as_ref().is_none_or(|s| s.len() == w * h)
        });
        (blocks, drawn, (px0, py0))
    };
    let fold = |t: &mut PhotoTile, (blocks, drawn, (px0, py0)): Answer| -> usize {
        let Some(Drawn { img, state }) = drawn else {
            return blocks.len();
        };
        let (iw, ih) = (img.width() as usize, img.height() as usize);
        let mut failed = 0;
        for i in blocks {
            let (bx, by) = (i % WMS_ACROSS, i / WMS_ACROSS);
            // The block's pixels, padded to 256 x 256 past the tile's edge like
            // a TIFF's edge tiles, and which of them have no photo.
            let mut px = vec![0u8; 256 * 256 * 3];
            let mut absent: Option<Vec<bool>> = None;
            let mut lost = false;
            for y in 0..256 {
                let iy = by * 256 + y - py0;
                if iy >= ih {
                    break;
                }
                for x in 0..256 {
                    let ix = bx * 256 + x - px0;
                    if ix >= iw {
                        break;
                    }
                    let p = img.get_pixel(ix as u32, iy as u32).0;
                    px[(y * 256 + x) * 3..(y * 256 + x) * 3 + 3].copy_from_slice(&p);
                    match state.as_ref().map_or(Px::Came, |s| s[iy * iw + ix]) {
                        Px::Came => {}
                        Px::Absent => {
                            let a = absent.get_or_insert_with(|| vec![false; 256 * 256]);
                            a[y * 256 + x] = true;
                        }
                        Px::Failed => lost = true,
                    }
                }
            }
            if lost {
                failed += 1;
                continue;
            }
            fill_cells(t, &layout, i, &px, absent.as_deref());
            t.blocks_in[i] = true;
        }
        failed
    };
    if in_order {
        chunks
            .into_values()
            .map(|blocks| fold(t, draw(blocks)))
            .sum()
    } else {
        let drawn: Vec<Answer> = chunks
            .into_values()
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(draw)
            .collect();
        drawn.into_iter().map(|a| fold(t, a)).sum()
    }
}

/// Most bytes of decoded sheet blocks a tile keeps for its next rectangles:
/// about 60 of Latvia's blocks of 512 pixels, more than a row of a tile's
/// rectangles meets, so those it shares with the next row are still kept.
const SHEET_BLOCK_BYTES: usize = 48 << 20;

/// What drawing a tile from sheets keeps from one rectangle to the next: each
/// sheet's layout once asked for, or why there is none, and the blocks lately
/// read, so that a block two rectangles share is read once.
#[derive(Default)]
struct SheetCache {
    layouts: Mutex<HashMap<String, Result<Arc<Layout>, Px>>>,
    recent: Mutex<Recent>,
}

/// Decoded sheet blocks by address and number, the latest last, up to
/// [`SHEET_BLOCK_BYTES`].
#[derive(Default)]
struct Recent {
    blocks: VecDeque<Kept>,
    bytes: usize,
}

struct Kept {
    url: String,
    block: usize,
    px: Arc<Vec<u8>>,
}

impl Recent {
    fn get(&mut self, url: &str, block: usize) -> Option<Arc<Vec<u8>>> {
        let at = self
            .blocks
            .iter()
            .position(|k| k.block == block && k.url == url)?;
        let hit = self.blocks.remove(at)?;
        let px = Arc::clone(&hit.px);
        self.blocks.push_back(hit);
        Some(px)
    }

    fn put(&mut self, url: &str, block: usize, px: Arc<Vec<u8>>) {
        self.bytes += px.len();
        self.blocks.push_back(Kept {
            url: url.to_string(),
            block,
            px,
        });
        while self.bytes > SHEET_BLOCK_BYTES {
            let Some(old) = self.blocks.pop_front() else {
                break;
            };
            self.bytes -= old.px.len();
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl SheetCache {
    /// The sheet's layout, read the first time it is asked for: absent where
    /// the survey has no such sheet, failed where it could not be had or is
    /// not one [`render_sheets`] reads.
    fn layout(
        &self,
        client: &reqwest::blocking::Client,
        url: &str,
        down: &AtomicBool,
    ) -> Result<Arc<Layout>, Px> {
        if let Some(l) = lock(&self.layouts).get(url) {
            return l.clone();
        }
        let l = if down.load(Ordering::Relaxed) {
            Err(Px::Failed)
        } else {
            match read_layout(client, url) {
                Ok(l) if sheet_layout(&l) => Ok(Arc::new(l)),
                Ok(_) => Err(Px::Failed),
                Err(Failure::Missing) => Err(Px::Absent),
                Err(e) => {
                    if unreachable(&e.to_string()) {
                        down.store(true, Ordering::Relaxed);
                    }
                    Err(Px::Failed)
                }
            }
        };
        lock(&self.layouts).insert(url.to_string(), l.clone());
        l
    }

    /// The sheet's blocks `want`, those not kept read side by side; `None`
    /// for one that could not be had.
    fn blocks(
        &self,
        client: &reqwest::blocking::Client,
        url: &str,
        l: &Layout,
        want: &[usize],
        down: &AtomicBool,
    ) -> HashMap<usize, Option<Arc<Vec<u8>>>> {
        use rayon::prelude::*;
        let mut got = HashMap::with_capacity(want.len());
        let mut missing = Vec::new();
        {
            let mut recent = lock(&self.recent);
            for &b in want {
                match recent.get(url, b) {
                    Some(px) => {
                        got.insert(b, Some(px));
                    }
                    None => missing.push(b),
                }
            }
        }
        let read: Vec<(usize, Option<Vec<u8>>)> = missing
            .par_iter()
            .map(|&b| {
                if down.load(Ordering::Relaxed) {
                    return (b, None);
                }
                let px = fetch_block(client, url, l, b);
                if px.as_ref().is_err_and(|e| unreachable(e)) {
                    down.store(true, Ordering::Relaxed);
                }
                (b, px.ok())
            })
            .collect();
        let mut recent = lock(&self.recent);
        for (b, px) in read {
            let px = px.map(Arc::new);
            if let Some(px) = &px {
                recent.put(url, b, Arc::clone(px));
            }
            got.insert(b, px);
        }
        got
    }
}

/// One part of a drawn rectangle: a sheet read, or pixels `x.0..=x.1` of rows
/// `y.0..=y.1` that no sheet can be had for.
enum Part {
    Sheet {
        url: String,
        sw: f64,
        sn: f64,
        layout: Arc<Layout>,
    },
    Gap {
        x: (usize, usize),
        y: (usize, usize),
        px: Px,
    },
}

/// The owner of a pixel past the most parts a rectangle is drawn from.
const LOST: u8 = u8::MAX;

/// Whether the sheet with north-west corner (`sw`, `sn`) covers the point.
fn covers((sw, sn): (f64, f64), l: &Layout, k: f64, (pe, pn): (f64, f64)) -> bool {
    pe >= sw && pe < sw + l.width as f64 / k && pn <= sn && pn > sn - l.height as f64 / k
}

/// A rectangle of the grid (west, south, east, north) drawn at `size` pixels
/// from sheets of GeoTIFF on the metre grid at `px_per_m` (see
/// [`Photo::Sheets`]), nearest pixel, with what `kept` holds of the tile's
/// sheets. Pixels that no sheet holds, or whose sheet the survey lacks, are
/// absent; those whose sheet or block could not be read failed. Once a
/// request finds the host unreachable, `down` is set and no more are made.
fn render_sheets(
    client: &reqwest::blocking::Client,
    (sheet, px_per_m): (SheetAt, usize),
    kept: &SheetCache,
    (w, s, e, n): (f64, f64, f64, f64),
    (width, height): (usize, usize),
    down: &AtomicBool,
) -> Drawn {
    let k = px_per_m as f64;
    let (sx, sy) = ((e - w) / width as f64, (n - s) / height as f64);
    let at = |x: usize, y: usize| (w + (x as f64 + 0.5) * sx, n - (y as f64 + 0.5) * sy);
    let named = |x: usize, y: usize| {
        let (pe, pn) = at(x, y);
        sheet(pe, pn).map(|s| s.0)
    };
    // The last of `from..to` still `same`, by halving.
    let last = |from: usize, to: usize, same: &dyn Fn(usize) -> bool| {
        let (mut lo, mut hi) = (from, to);
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if same(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    };
    // Each output pixel's part, by the sheets' extents: a rectangle meets at
    // most a few, each looked up once and its layout read once a tile. Where
    // none is named, or the one named cannot be had, the pixels named the
    // same are found east and south of this one by halving: sheets are
    // rectangles, and this is the first pixel of its sheet to come.
    let mut owner = vec![0u8; width * height];
    let mut parts: Vec<Part> = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let p = at(x, y);
            let holds = |part: &Part| match part {
                Part::Sheet { sw, sn, layout, .. } => covers((*sw, *sn), layout, k, p),
                Part::Gap { x: xs, y: ys, .. } => {
                    (xs.0..=xs.1).contains(&x) && (ys.0..=ys.1).contains(&y)
                }
            };
            let i = match parts.iter().position(holds) {
                Some(i) => i as u8,
                None if parts.len() == usize::from(LOST) => LOST,
                None => {
                    let gap = |name: Option<String>, px: Px| Part::Gap {
                        x: (x, last(x, width, &|x1| named(x1, y) == name)),
                        y: (y, last(y, height, &|y1| named(x, y1) == name)),
                        px,
                    };
                    let part = match sheet(p.0, p.1) {
                        None => gap(None, Px::Absent),
                        Some((url, sw, sn)) => match kept.layout(client, &url, down) {
                            Ok(layout) if covers((sw, sn), &layout, k, p) => Part::Sheet {
                                url,
                                sw,
                                sn,
                                layout,
                            },
                            Ok(_) => gap(Some(url), Px::Failed),
                            Err(px) => gap(Some(url), px),
                        },
                    };
                    parts.push(part);
                    (parts.len() - 1) as u8
                }
            };
            owner[y * width + x] = i;
        }
    }
    let mut state: Vec<Px> = owner
        .iter()
        .map(|&o| match parts.get(usize::from(o)) {
            Some(Part::Sheet { .. }) => Px::Came,
            Some(Part::Gap { px, .. }) => *px,
            None => Px::Failed,
        })
        .collect();
    let mut img = image::RgbImage::new(width as u32, height as u32);
    for (i, part) in parts.iter().enumerate() {
        let Part::Sheet {
            url,
            sw,
            sn,
            layout: l,
        } = part
        else {
            continue;
        };
        let across = l.width.div_ceil(l.tile_w);
        // The sheet's pixel under each output pixel it owns.
        let src = |x: usize, y: usize| {
            let (pe, pn) = at(x, y);
            let px = (((pe - sw) * k) as usize).min(l.width - 1);
            let py = (((sn - pn) * k) as usize).min(l.height - 1);
            (px, py)
        };
        let block = |(px, py): (usize, usize)| (py / l.tile_h) * across + px / l.tile_w;
        let mut blocks: Vec<usize> = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .filter(|&(x, y)| usize::from(owner[y * width + x]) == i)
            .map(|(x, y)| block(src(x, y)))
            .collect();
        blocks.sort_unstable();
        blocks.dedup();
        let got = kept.blocks(client, url, l, &blocks, down);
        for y in 0..height {
            for x in 0..width {
                if usize::from(owner[y * width + x]) != i {
                    continue;
                }
                let (px, py) = src(x, y);
                let Some(b) = &got[&block((px, py))] else {
                    state[y * width + x] = Px::Failed;
                    continue;
                };
                let o = ((py % l.tile_h) * l.tile_w + px % l.tile_w) * l.samples;
                img.put_pixel(x as u32, y as u32, image::Rgb([b[o], b[o + 1], b[o + 2]]));
            }
        }
    }
    let state = state.iter().any(|&p| p != Px::Came).then_some(state);
    Drawn { img, state }
}

/// Whether a sheet's layout is one [`render_sheets`] reads: 3 or 4 samples a
/// pixel in square blocks of 16 to 1024 pixels, an offset for each, and no
/// more than 65536 pixels a side.
fn sheet_layout(l: &Layout) -> bool {
    let sides = (1..=1 << 16).contains(&l.width) && (1..=1 << 16).contains(&l.height);
    sides
        && (16..=1024).contains(&l.tile_w)
        && l.tile_h == l.tile_w
        && (3..=4).contains(&l.samples)
        && l.offsets.len() >= l.width.div_ceil(l.tile_w) * l.height.div_ceil(l.tile_h)
}

/// Read the first IFD of a classic TIFF and the tile table it points to;
/// [`Failure::Missing`] where the server has no such file.
fn read_layout(client: &reqwest::blocking::Client, url: &str) -> Result<Layout, Failure> {
    let head = super::fetch::get_range(client, url, 0, 65_536)?;
    layout_from(client, url, head).map_err(Failure::Other)
}

/// The layout from the file's first 64 KiB, `head`, reading on where the
/// directory or its tables lie past them.
fn layout_from(
    client: &reqwest::blocking::Client,
    url: &str,
    head: Vec<u8>,
) -> Result<Layout, String> {
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
    // Most files keep their directory up front; some write it after the
    // pixels, and it is read from there.
    let (head, base) = if ifd + 2 > head.len() {
        (get_range(client, url, ifd as u64, 65_536)?, ifd)
    } else {
        (head, 0)
    };
    let ifd = ifd - base;
    if ifd + 2 > head.len() {
        return Err("short TIFF directory".into());
    }
    let entries = u16_at(&head, ifd) as usize;
    let mut l = Layout {
        width: 0,
        height: 0,
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
            257 => l.height = value(),
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
        } else if let Some(rel) = (at as usize)
            .checked_sub(base)
            .filter(|&r| r + count * size <= head.len())
        {
            head[rel..rel + count * size].to_vec()
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
/// pixel rows and columns), which is always exactly one of the two. A cell
/// any of whose pixels is `absent` (the block's, row-major) has no photo.
fn fill_cells(t: &mut PhotoTile, l: &Layout, i: usize, px: &[u8], absent: Option<&[bool]>) {
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
            let gap = |a: &[bool]| {
                (py0..py1).any(|py| (px0..px1).any(|pxx| a[(py - y0) * l.tile_w + pxx - x0]))
            };
            if absent.is_some_and(gap) {
                t.cells[r * TILE + c] = PhotoCell::default();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_blocks_go_oldest_first_past_the_bytes() {
        let mut r = Recent::default();
        let block = || Arc::new(vec![0u8; SHEET_BLOCK_BYTES / 3]);
        r.put("a", 1, block());
        r.put("a", 2, block());
        r.put("b", 1, block());
        // A block asked for is the latest again: the next one pushes out a 2.
        assert!(r.get("a", 1).is_some());
        r.put("b", 2, block());
        assert!(r.get("a", 2).is_none());
        assert!(r.get("a", 1).is_some() && r.get("b", 1).is_some() && r.get("b", 2).is_some());
    }

    /// Strips of tile 0_0 from the west: no sheet, a sheet the survey lacks,
    /// one that cannot be had, and one of a single block of 512 pixels.
    fn strips(e: f64, _n: f64) -> Option<(String, f64, f64)> {
        let at = |name: &str, w: f64| Some((name.to_string(), w, 1000.0));
        match e {
            e if e < 102.4 => None,
            e if e < 204.8 => at("missing", 102.4),
            e if e < 307.2 => at("failing", 204.8),
            _ => at("here", 307.2),
        }
    }

    #[test]
    fn sheets_missing_are_done_with_and_failing_ones_left_for_later() {
        let kept = SheetCache::default();
        {
            let mut layouts = lock(&kept.layouts);
            layouts.insert("missing".into(), Err(Px::Absent));
            layouts.insert("failing".into(), Err(Px::Failed));
            let here = Layout {
                width: 512,
                height: 512,
                tile_w: 512,
                tile_h: 512,
                offsets: vec![0],
                counts: vec![0],
                compression: 1,
                predictor: 1,
                samples: 3,
            };
            layouts.insert("here".into(), Ok(Arc::new(here)));
        }
        let grey = [120u8, 130, 140].repeat(512 * 512);
        lock(&kept.recent).put("here", 0, Arc::new(grey));
        let client = reqwest::blocking::Client::new();
        let down = AtomicBool::new(false);
        let mut t = PhotoTile::new(WMS_ACROSS);
        // The first eight blocks of the top row, a strip to two.
        let todo: Vec<usize> = (0..8).collect();
        let failed = fetch_rendered((0, 0), &todo, &mut t, true, &|rect, size| {
            Some(render_sheets(
                &client,
                (strips, 5),
                &kept,
                rect,
                size,
                &down,
            ))
        });
        assert_eq!(failed, 2);
        assert_eq!(
            t.blocks_in[..8],
            [true, true, true, true, false, false, true, true]
        );
        let cell = |c: usize| t.cells[10 * TILE + c];
        assert!(!cell(10).present && !cell(150).present && !cell(250).present);
        assert!(cell(350).present);
        assert_eq!(cell(350).rgb, [120, 130, 140]);
        assert!(!down.load(Ordering::Relaxed));
    }
}
