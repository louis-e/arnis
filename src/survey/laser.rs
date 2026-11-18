//! Laser tiles: read, decode, and boil down to per-metre measurements.
//!
//! Points are decoded straight from the download and only a few numbers per
//! square metre are kept, cached compressed on disk at a few megabytes a square
//! kilometre instead of the hundred the points take. The points themselves are
//! never stored.
//!
//! A tile is cached in 100 m blocks with a record of which are measured. A
//! plain LAZ tile is read whole and has all of them; a COPC tile is read only
//! over the blocks an area needs (plus a margin, so the ground under a building
//! at the block's edge is still seen), and a later, larger area reads only the
//! blocks still missing. Blocks are measured from one file at one density: the
//! cache records the file and the density aimed for, and a tile whose file
//! changed (a newer delivery) is measured afresh.
//!
//! The measurements per 1 m cell, on the source's grid (row 0 north):
//!
//! * `n_all`, `n_gnd`, `n_obj`, `n_bld`: returns of any class, ground, object
//!   points over 0.25 m, building points (each capped at 255);
//! * `n_veg`: those object points that look like plants: of a vegetation class
//!   where the source has one, else not the last return of their pulse (a
//!   crown or a hedge lets part of the pulse through, a car or a wall does not);
//! * `h_obj`, `h_bld`: 95th percentile height above the ground of the object and
//!   building points, in fifths of a metre (capped at 51 m);
//! * `i_gnd`: median intensity of the ground returns.
//!
//! Heights are above the scan's own ground, the mean of the ground returns in
//! each cell, filled under buildings and dense hedges from around them.

use super::copc::{self, PointLayout};
use super::crs::{Affine, Crs};
use super::ept;
use super::fetch::{self, Failure};
use super::source::{Laser, Rect, Sheet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};

pub const TILE: usize = 1000;
const CELLS: usize = TILE * TILE;
/// Side of a cached block in metres, and blocks along a tile's side.
const BLOCK: usize = 100;
const BLOCKS: usize = TILE / BLOCK;
/// Margin read around the blocks wanted, so the ground at their edges is seen.
const PAD_M: f64 = 30.0;
const MAGIC: &[u8; 8] = b"ARNSLAS4";
/// Points a square metre a COPC read aims for: past this the octree's deeper
/// levels are not read, and the rules scale their counts to the density there
/// is. Seven reads AHN at level 3 (about 10 points a m², 80 MB for a whole tile
/// against 165 for all of it), swissSURFACE3D at level 3 (about 8, 55 MB against
/// 155) and LiDAR HD at level 4 (about 13, 75 MB against 105; its level 3 has
/// about 5).
pub const TARGET_DENSITY: f64 = 7.0;

/// A tile the source does not have is asked for again after this long.
const ABSENT_DAYS: u64 = 30;
/// Blocks a lookup found nothing for are looked up again after this long: a
/// lookup is one small request, and an index that answered empty in a hiccup,
/// or had not listed a delivery yet, is not believed for long.
const LOOKUP_ABSENT_DAYS: u64 = 7;
/// A tile cached from a zip (an older campaign, read whole) is looked up again
/// after this long, so that a newer delivery replaces it.
const ZIP_RECHECK_DAYS: u64 = 30;

pub struct LaserTile {
    pub n_all: Vec<u8>,
    pub n_gnd: Vec<u8>,
    pub n_obj: Vec<u8>,
    pub n_bld: Vec<u8>,
    pub n_veg: Vec<u8>,
    pub h_obj: Vec<u8>,
    pub h_bld: Vec<u8>,
    pub i_gnd: Vec<u16>,
    /// Which blocks are measured, row-major from the north-west.
    pub blocks: Vec<bool>,
    /// The file they were measured from.
    origin: String,
}

/// Which blocks of tile (`te`, `tn`) a grid rectangle meets.
pub fn blocks_for(te: i32, tn: i32, (e0, n0, e1, n1): Rect) -> Vec<bool> {
    let (te0, tn1) = (f64::from(te) * 1000.0, f64::from(tn + 1) * 1000.0);
    let mut out = vec![false; BLOCKS * BLOCKS];
    for by in 0..BLOCKS {
        let (bn1, bn0) = (tn1 - (by * BLOCK) as f64, tn1 - ((by + 1) * BLOCK) as f64);
        for bx in 0..BLOCKS {
            let (be0, be1) = (te0 + (bx * BLOCK) as f64, te0 + ((bx + 1) * BLOCK) as f64);
            out[by * BLOCKS + bx] = e0 < be1 && e1 > be0 && n0 < bn1 && n1 > bn0;
        }
    }
    out
}

impl LaserTile {
    fn empty(origin: &str) -> Self {
        LaserTile {
            n_all: vec![0; CELLS],
            n_gnd: vec![0; CELLS],
            n_obj: vec![0; CELLS],
            n_bld: vec![0; CELLS],
            n_veg: vec![0; CELLS],
            h_obj: vec![0; CELLS],
            h_bld: vec![0; CELLS],
            i_gnd: vec![0; CELLS],
            blocks: vec![false; BLOCKS * BLOCKS],
            origin: origin.to_string(),
        }
    }

    fn has(&self, want: &[bool]) -> bool {
        covers(&self.blocks, want)
    }

    fn write(&self, path: &Path) -> std::io::Result<()> {
        let mut raw = Vec::with_capacity(CELLS * 9 + 256);
        raw.extend_from_slice(MAGIC);
        raw.push((TARGET_DENSITY * 10.0) as u8);
        raw.extend(self.blocks.iter().map(|&b| b as u8));
        let origin = self.origin.as_bytes();
        raw.extend_from_slice(&(origin.len() as u16).to_le_bytes());
        raw.extend_from_slice(origin);
        for layer in [
            &self.n_all,
            &self.n_gnd,
            &self.n_obj,
            &self.n_bld,
            &self.n_veg,
            &self.h_obj,
            &self.h_bld,
        ] {
            raw.extend_from_slice(layer);
        }
        for v in &self.i_gnd {
            raw.extend_from_slice(&v.to_le_bytes());
        }
        let packed = zstd::encode_all(&raw[..], 9)?;
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, packed)?;
        std::fs::rename(tmp, path)
    }

    /// Which blocks a cached tile holds and the file they came from, read
    /// from the start of the file alone: what a check needs, without
    /// decompressing the 9 MB of layers after it. `None` unless it is of this
    /// version and density.
    fn read_head(path: &Path) -> Option<(Vec<bool>, String)> {
        let mut d = zstd::Decoder::new(std::fs::File::open(path).ok()?).ok()?;
        Self::head(&mut d)
    }

    fn head(d: &mut impl Read) -> Option<(Vec<bool>, String)> {
        let nb = BLOCKS * BLOCKS;
        let mut head = vec![0u8; 9 + nb + 2];
        d.read_exact(&mut head).ok()?;
        if &head[..8] != MAGIC || head[8] != (TARGET_DENSITY * 10.0) as u8 {
            return None;
        }
        let blocks = head[9..9 + nb].iter().map(|&b| b != 0).collect();
        let olen = usize::from(u16::from_le_bytes([head[9 + nb], head[10 + nb]]));
        let mut origin = vec![0u8; olen];
        d.read_exact(&mut origin).ok()?;
        Some((blocks, String::from_utf8(origin).ok()?))
    }

    /// Read a cached tile, each layer straight from the decompressor. `None`
    /// unless the file is whole and of this version and density.
    fn read(path: &Path) -> Option<Self> {
        let mut d = zstd::Decoder::new(std::fs::File::open(path).ok()?).ok()?;
        let (blocks, origin) = Self::head(&mut d)?;
        let mut layer = |len: usize| {
            let mut v = vec![0u8; len];
            d.read_exact(&mut v).ok().map(|_| v)
        };
        let (n_all, n_gnd, n_obj) = (layer(CELLS)?, layer(CELLS)?, layer(CELLS)?);
        let (n_bld, n_veg, h_obj, h_bld) =
            (layer(CELLS)?, layer(CELLS)?, layer(CELLS)?, layer(CELLS)?);
        let i_gnd = layer(2 * CELLS)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| u16::from_le_bytes(b))
            .collect();
        // Nothing may follow.
        if d.read(&mut [0u8; 1]).ok()? != 0 {
            return None;
        }
        Some(LaserTile {
            n_all,
            n_gnd,
            n_obj,
            n_bld,
            n_veg,
            h_obj,
            h_bld,
            i_gnd,
            blocks,
            origin,
        })
    }

    /// Take the cells of the blocks in `fill` from `other`.
    fn take_blocks(&mut self, other: &LaserTile, fill: &[bool]) {
        for (b, _) in fill.iter().enumerate().filter(|(_, &f)| f) {
            let (bx, by) = (b % BLOCKS, b / BLOCKS);
            for row in by * BLOCK..(by + 1) * BLOCK {
                let r = row * TILE + bx * BLOCK..row * TILE + (bx + 1) * BLOCK;
                self.n_all[r.clone()].copy_from_slice(&other.n_all[r.clone()]);
                self.n_gnd[r.clone()].copy_from_slice(&other.n_gnd[r.clone()]);
                self.n_obj[r.clone()].copy_from_slice(&other.n_obj[r.clone()]);
                self.n_bld[r.clone()].copy_from_slice(&other.n_bld[r.clone()]);
                self.n_veg[r.clone()].copy_from_slice(&other.n_veg[r.clone()]);
                self.h_obj[r.clone()].copy_from_slice(&other.h_obj[r.clone()]);
                self.h_bld[r.clone()].copy_from_slice(&other.h_bld[r.clone()]);
                self.i_gnd[r.clone()].copy_from_slice(&other.i_gnd[r]);
            }
            self.blocks[b] = true;
        }
    }
}

/// What a tile lookup came to.
pub enum Fetched {
    Tile(Box<LaserTile>),
    /// The source has no such tile (outside its coverage along the border).
    Absent,
}

pub fn cache_path(root: &Path, source: &Sheet, te: i32, tn: i32) -> PathBuf {
    root.join(source.dir()).join(format!("tile_{te}_{tn}.bin"))
}

fn absent_marker(root: &Path, source: &Sheet, te: i32, tn: i32) -> PathBuf {
    root.join(source.dir())
        .join(format!("tile_{te}_{tn}.absent"))
}

/// Whether the source gives each tile its own address. Only then does a tile
/// whose files are all missing say the source has none there: a lookup
/// answers for the part of a tile an area asks about.
fn fixed_addresses(source: &Sheet) -> bool {
    let (Laser::Whole { locate, .. } | Laser::Copc { locate, .. } | Laser::Ept { locate, .. }) =
        &source.laser;
    matches!(locate, super::source::Locate::Urls(_))
}

fn whole_marker(root: &Path, source: &Sheet, te: i32, tn: i32) -> PathBuf {
    root.join(source.dir())
        .join(format!("tile_{te}_{tn}.whole"))
}

/// Notes whether a cloud-optimised source's index listed tile (`te`, `tn`) as
/// one whole file (zipped, see [`is_zip`]), for runs that need no lookup.
pub fn note_whole(root: &Path, source: &Sheet, (te, tn): (i32, i32), whole: bool) {
    let path = whole_marker(root, source, te, tn);
    if !whole {
        let _ = std::fs::remove_file(path);
    } else if std::fs::create_dir_all(root.join(source.dir())).is_ok() {
        let _ = std::fs::write(path, b"");
    }
}

/// Whether the index last listed tile `t` as one whole file ([`note_whole`]).
pub fn noted_whole(root: &Path, source: &Sheet, (te, tn): (i32, i32)) -> bool {
    whole_marker(root, source, te, tn).exists()
}

fn size_note(root: &Path, source: &Sheet, te: i32, tn: i32) -> PathBuf {
    root.join(source.dir()).join(format!("tile_{te}_{tn}.size"))
}

/// Notes the size the server gave for tile `t`'s whole file, so an estimate
/// made from the cache alone (the data plan, an offline run) counts it so.
pub fn note_size(root: &Path, source: &Sheet, (te, tn): (i32, i32), bytes: u64) {
    if std::fs::create_dir_all(root.join(source.dir())).is_ok() {
        let _ = std::fs::write(size_note(root, source, te, tn), bytes.to_string());
    }
}

/// The size last noted for tile `t`'s whole file ([`note_size`]).
pub fn noted_size(root: &Path, source: &Sheet, (te, tn): (i32, i32)) -> Option<u64> {
    std::fs::read_to_string(size_note(root, source, te, tn))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Whether the file at `path` was last written within `days`.
fn fresh(path: &Path, days: u64) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age.as_secs() < days * 86_400)
}

/// The blocks of tile (`te`, `tn`) the source is known not to have, while its
/// marker is fresh: all of them where the tile's fixed address had no file
/// (an empty marker), else those a lookup found nothing for (a byte each).
fn absent_blocks(root: &Path, source: &Sheet, te: i32, tn: i32) -> Option<Vec<bool>> {
    let path = absent_marker(root, source, te, tn);
    if fixed_addresses(source) {
        return fresh(&path, ABSENT_DAYS).then(|| vec![true; BLOCKS * BLOCKS]);
    }
    if !fresh(&path, LOOKUP_ABSENT_DAYS) {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    (bytes.len() == BLOCKS * BLOCKS).then(|| bytes.iter().map(|&b| b != 0).collect())
}

/// Notes that the source has nothing for `blocks` of tile (`te`, `tn`): the
/// whole tile where its address is fixed, else those blocks besides any
/// noted before, as a lookup answers only for the part of a tile it is asked
/// about. A later run then needs no lookup or download for them, offline too.
pub fn mark_absent(root: &Path, source: &Sheet, (te, tn): (i32, i32), blocks: &[bool]) {
    if !blocks.contains(&true) || std::fs::create_dir_all(root.join(source.dir())).is_err() {
        return;
    }
    let path = absent_marker(root, source, te, tn);
    if fixed_addresses(source) {
        let _ = std::fs::write(path, b"");
        return;
    }
    let mut mask =
        absent_blocks(root, source, te, tn).unwrap_or_else(|| vec![false; BLOCKS * BLOCKS]);
    for (m, &b) in mask.iter_mut().zip(blocks) {
        *m |= b;
    }
    let _ = std::fs::write(path, mask.iter().map(|&b| u8::from(b)).collect::<Vec<u8>>());
}

/// Whether `blocks` holds every block wanted.
fn covers(blocks: &[bool], want: &[bool]) -> bool {
    want.iter().zip(blocks).all(|(&w, &b)| !w || b)
}

/// The wanted blocks neither cached nor known absent: what a run would read.
pub fn lacking(root: &Path, source: &Sheet, te: i32, tn: i32, want: &[bool]) -> Vec<bool> {
    let gone = absent_blocks(root, source, te, tn);
    let have = LaserTile::read_head(&cache_path(root, source, te, tn)).map(|(blocks, _)| blocks);
    let had = |b: &Option<Vec<bool>>, i: usize| b.as_ref().is_some_and(|b| b[i]);
    want.iter()
        .enumerate()
        .map(|(i, &w)| w && !had(&have, i) && !had(&gone, i))
        .collect()
}

/// Square kilometres of the wanted blocks not yet cached (nor known absent).
pub fn missing_km2(root: &Path, source: &Sheet, te: i32, tn: i32, want: &[bool]) -> f64 {
    let missing = lacking(root, source, te, tn, want)
        .iter()
        .filter(|&&b| b)
        .count();
    missing as f64 * (BLOCK * BLOCK) as f64 / 1e6
}

/// Whether the source is known to have none of the blocks wanted.
pub fn known_absent(root: &Path, source: &Sheet, (te, tn): (i32, i32), want: &[bool]) -> bool {
    absent_blocks(root, source, te, tn).is_some_and(|gone| covers(&gone, want))
}

/// Whether the tile costs no download for the blocks wanted: its cache file's
/// start says it holds them, or the source is known to have none of them.
pub fn is_cached(root: &Path, source: &Sheet, te: i32, tn: i32, want: &[bool]) -> bool {
    !lacking(root, source, te, tn, want).contains(&true)
}

/// Whether tile `t` is cached from a zip (an older campaign, see [`is_zip`])
/// long enough ago that the source's index should be asked whether a newer
/// delivery has replaced it.
pub fn due_recheck(root: &Path, source: &Sheet, (te, tn): (i32, i32)) -> bool {
    let path = cache_path(root, source, te, tn);
    LaserTile::read_head(&path).is_some_and(|(_, origin)| is_zip(&origin))
        && !fresh(&path, ZIP_RECHECK_DAYS)
}

/// After a lookup listed tile `t` at `url`: a tile cached from a zip that is
/// no longer what the index lists (a newer campaign, or the tile now
/// cloud-optimised) is let go, so the run reads the newer delivery and counts
/// it in the download; one still listed is kept for another
/// [`ZIP_RECHECK_DAYS`]. Returns whether it was let go.
pub fn supersede(root: &Path, source: &Sheet, (te, tn): (i32, i32), url: &str) -> bool {
    let path = cache_path(root, source, te, tn);
    match LaserTile::read_head(&path) {
        Some((_, origin)) if is_zip(&origin) && origin != url => {
            std::fs::remove_file(&path).is_ok()
        }
        Some((_, origin)) if is_zip(&origin) => {
            if let Ok(f) = std::fs::File::options().write(true).open(&path) {
                let _ = f.set_modified(std::time::SystemTime::now());
            }
            false
        }
        _ => false,
    }
}

/// The tile's measurements over the blocks wanted, from the cache or else read
/// from the first of `urls` the server has. No URLs means the source has
/// nothing for this area there. A file this reader cannot use is passed over
/// with a warning; one that fails part-way fails the tile, and nothing of it
/// is cached. Where no file is had or usable, that is noted (see
/// [`mark_absent`]) so the next runs do not ask again for a while.
pub fn tile(
    client: &reqwest::blocking::Client,
    root: &Path,
    source: &Sheet,
    (te, tn): (i32, i32),
    want: &[bool],
    urls: &[String],
) -> Result<Fetched, String> {
    let path = cache_path(root, source, te, tn);
    // Read once: what it holds is either all that is wanted or the start a
    // COPC read adds to.
    let mut have = LaserTile::read(&path);
    if have.as_ref().is_some_and(|t| t.has(want)) {
        return Ok(Fetched::Tile(Box::new(have.take().unwrap())));
    }
    // What the source may have of what is wanted; what is cached of it is
    // all there is where the rest is known absent.
    let gone = absent_blocks(root, source, te, tn).unwrap_or_default();
    let need: Vec<bool> = want
        .iter()
        .enumerate()
        .map(|(i, &w)| w && !gone.get(i).is_some_and(|&g| g))
        .collect();
    if !need.contains(&true) {
        return Ok(Fetched::Absent);
    }
    if let Some(t) = have.take_if(|t| t.has(&need)) {
        return Ok(Fetched::Tile(Box::new(t)));
    }
    // A lookup that did not list the tile (see `survey::locate`, which notes so).
    if urls.is_empty() {
        return Ok(Fetched::Absent);
    }
    crate::net::ensure_online("laser scans")?;
    std::fs::create_dir_all(root.join(source.dir())).map_err(|e| e.to_string())?;
    for url in urls {
        let got = match source.laser {
            Laser::Whole { .. } => read_whole(client, root, source, te, tn, url),
            // A tile only delivered whole (an older campaign, zipped).
            Laser::Copc { .. } if is_zip(url) => read_whole(client, root, source, te, tn, url),
            Laser::Copc { .. } | Laser::Ept { .. } => {
                // A COPC or EPT tile decodes as it downloads: a turn at both.
                let _download = DOWNLOADS.enter(DOWNLOADS_AT_ONCE);
                let _decode = DECODES.enter(decodes_at_once());
                let mut t = have
                    .take()
                    .filter(|t| t.origin == *url)
                    .unwrap_or_else(|| LaserTile::empty(url));
                read_copc(client, source, te, tn, want, url, &mut t).map(|_| t)
            }
        };
        match got {
            Ok(t) => {
                if let Err(e) = t.write(&path) {
                    eprintln!("Warning: laser tile {te}_{tn} not cached: {e}");
                    crate::net::note_uncached("laser scans");
                }
                return Ok(Fetched::Tile(Box::new(t)));
            }
            Err(Failure::Missing) => continue,
            Err(Failure::Unusable(e)) => {
                eprintln!("Warning: laser scan {te}_{tn} passed over: {e}");
            }
            Err(Failure::Other(e)) => return Err(format!("{url}: {e}")),
        }
    }
    // None of the tile's files is on the server, or none this reader can use
    // (as it could not on any later run either).
    mark_absent(root, source, (te, tn), &need);
    Ok(Fetched::Absent)
}

/// Read the COPC tile (or EPT survey) over the wanted blocks `t` lacks and fold
/// them into it. `url` may name several files, a line each (a tile whose
/// survey's files do not follow the grid's kilometres); it is missing only
/// when all of them are.
fn read_copc(
    client: &reqwest::blocking::Client,
    source: &Sheet,
    te: i32,
    tn: i32,
    want: &[bool],
    url: &str,
    t: &mut LaserTile,
) -> Result<(), Failure> {
    let fill: Vec<bool> = want.iter().zip(&t.blocks).map(|(&w, &h)| w && !h).collect();
    let (mut bx0, mut by0, mut bx1, mut by1) = (BLOCKS, BLOCKS, 0, 0);
    for (b, _) in fill.iter().enumerate().filter(|(_, &f)| f) {
        bx0 = bx0.min(b % BLOCKS);
        bx1 = bx1.max(b % BLOCKS + 1);
        by0 = by0.min(b / BLOCKS);
        by1 = by1.max(b / BLOCKS + 1);
    }
    if bx1 == 0 {
        return Ok(());
    }
    // Window in tile cells, then in grid metres.
    let pad = PAD_M as usize;
    let cells = (
        (bx0 * BLOCK).saturating_sub(pad),
        (by0 * BLOCK).saturating_sub(pad),
        (bx1 * BLOCK + pad).min(TILE),
        (by1 * BLOCK + pad).min(TILE),
    );
    let (te0, tn1) = (f64::from(te) * 1000.0, f64::from(tn + 1) * 1000.0);
    let window = (
        te0 + cells.0 as f64,
        tn1 - cells.3 as f64,
        te0 + cells.2 as f64,
        tn1 - cells.1 as f64,
    );
    let centre = ((window.0 + window.2) / 2.0, (window.1 + window.3) / 2.0);
    let m = Mutex::new(Measure::new(source, te, tn, cells));
    let visit = |layout: &PointLayout, recs: &[u8]| {
        let mut m = m.lock().unwrap_or_else(|e| e.into_inner());
        for rec in recs.chunks_exact(layout.record_len) {
            m.add(layout, rec);
        }
    };
    // A tile made of several files (one line each) is read from all of them
    // that exist; a file it does not reach costs only its header.
    // A file that is missing, or one this reader cannot use, adds no points
    // and is passed over; one that fails part-way fails the tile, so the
    // points the others gave are never cached as all there is.
    let area = (window.2 - window.0) * (window.3 - window.1);
    let mut found = false;
    let mut unusable = None;
    for file in url.split('\n') {
        let stats = match source.laser {
            // EPT points are in Web Mercator: carried onto the grid by a map
            // fitted over the window, and the window asked for in Mercator.
            Laser::Ept { .. } => {
                let f = Affine::fit(Crs::WebMercator, source.crs, centre, 250.0);
                m.lock().unwrap_or_else(|e| e.into_inner()).to_grid = Some(f);
                ept::read(
                    client,
                    file,
                    f.inverse().rect(window),
                    TARGET_DENSITY,
                    8,
                    &visit,
                )
            }
            // Files that name their own grid: carried across where it is not
            // the area's.
            Laser::Copc { own_grid: true, .. } => {
                let place = |wkt: Option<&str>| -> Result<Option<Affine>, String> {
                    let crs = wkt
                        .and_then(super::crs::from_wkt)
                        .ok_or_else(|| "a grid this reader does not know".to_string())?;
                    let f =
                        (crs != source.crs).then(|| Affine::fit(crs, source.crs, centre, 250.0));
                    m.lock().unwrap_or_else(|e| e.into_inner()).to_grid = f;
                    Ok(f)
                };
                copc::read(client, file, window, TARGET_DENSITY, 4, &place, &visit)
            }
            _ => copc::read(
                client,
                file,
                window,
                TARGET_DENSITY,
                4,
                &|_| Ok(None),
                &visit,
            ),
        };
        let stats = match stats {
            Ok(s) => s,
            Err(Failure::Missing) => continue,
            Err(Failure::Unusable(e)) => {
                if url.contains('\n') {
                    eprintln!("Warning: laser scan {te}_{tn} leaves out {file}: {e}");
                }
                unusable = Some(e);
                continue;
            }
            Err(e) => return Err(e),
        };
        found = true;
        if std::env::var_os("ARNIS_LASER_DEBUG").is_some() {
            eprintln!(
                "laser {te}_{tn}: level {} nodes {} requests {} {:.1} MB {:.1} pts/m² over {:.2} km²",
                stats.level,
                stats.nodes,
                stats.requests,
                stats.bytes as f64 / 1e6,
                stats.points as f64 / area,
                area / 1e6
            );
        }
    }
    if !found {
        return Err(unusable.map_or(Failure::Missing, Failure::Unusable));
    }
    fetch::count_laser_area(area);
    let measured = m
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .finish(url);
    t.take_blocks(&measured, &fill);
    Ok(())
}

/// Whether `url` is a zip archive holding one LAS or LAZ file, read whole.
pub fn is_zip(url: &str) -> bool {
    url.ends_with(".zip")
}

/// Download a plain LAZ tile (or a zip holding one) and measure all of it.
fn read_whole(
    client: &reqwest::blocking::Client,
    root: &Path,
    source: &Sheet,
    te: i32,
    tn: i32,
    url: &str,
) -> Result<LaserTile, Failure> {
    // Download to a temporary file: the decoder needs to seek to the chunk table.
    // It goes when this returns, whichever way.
    let tmp = Partial(
        root.join(source.dir())
            .join(format!("tile_{te}_{tn}.laz.part{}", std::process::id())),
    );
    let other = |e: String| Failure::Other(e);
    {
        let _turn = DOWNLOADS.enter(DOWNLOADS_AT_ONCE);
        let _permit = crate::net::request_permit();
        let mut resp = client
            .get(url)
            .send()
            .map_err(|e| other(crate::net::failure_kind(&e)))?;
        if matches!(resp.status().as_u16(), 403 | 404) {
            return Err(Failure::Missing);
        }
        if !resp.status().is_success() {
            return Err(other(format!("HTTP {}", resp.status())));
        }
        let mut file = std::fs::File::create(&tmp.0).map_err(|e| other(e.to_string()))?;
        // Counted as it comes, so the download limit stops it part-way.
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = resp
                .read(&mut buf)
                .map_err(|e| other(fetch::read_failure(&e)))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| other(e.to_string()))?;
            fetch::count_laser_bytes(n as u64);
            fetch::within_laser_limit()?;
        }
        file.flush().map_err(|e| other(e.to_string()))?;
        fetch::count_laser_area(1e6);
    }
    let measured = {
        let _turn = DECODES.enter(decodes_at_once());
        if is_zip(url) {
            let las = Partial(PathBuf::from(format!("{}.las", tmp.0.display())));
            unzip(&tmp.0, &las.0).and_then(|()| measure_file(&las.0, source, te, tn, url))
        } else {
            measure_file(&tmp.0, source, te, tn, url)
        }
    };
    measured
}

/// Most a zipped tile may unpack to: Switzerland's hold one LAS file of a few
/// hundred MB (360 in Muri bei Bern).
const MAX_UNZIPPED: u64 = 4 << 30;

/// Unpack the LAS or LAZ file in the zip at `from` to `to`, at most
/// [`MAX_UNZIPPED`] of it, whatever the zip says its size is. A zip that came
/// whole but holds no such file, or one too large, is [`Failure::Unusable`]:
/// it would be every time.
fn unzip(from: &Path, to: &Path) -> Result<(), Failure> {
    let unusable = |e: String| Failure::Unusable(e);
    let file = std::fs::File::open(from).map_err(|e| Failure::Other(e.to_string()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| unusable(e.to_string()))?;
    let i = (0..zip.len())
        .find(|&i| {
            zip.name_for_index(i).and_then(Result::ok).is_some_and(|n| {
                let n = n.to_ascii_lowercase();
                n.ends_with(".las") || n.ends_with(".laz")
            })
        })
        .ok_or_else(|| unusable("no LAS file in the zip".into()))?;
    let entry = zip.by_index(i).map_err(|e| unusable(e.to_string()))?;
    if entry.size() > MAX_UNZIPPED {
        return Err(unusable(format!("a zipped file of {} bytes", entry.size())));
    }
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(to).map_err(|e| Failure::Other(e.to_string()))?,
    );
    let n = std::io::copy(&mut entry.take(MAX_UNZIPPED + 1), &mut out).map_err(io_failure)?;
    if n > MAX_UNZIPPED {
        return Err(unusable("a zipped file larger than it says".into()));
    }
    out.flush().map_err(|e| Failure::Other(e.to_string()))
}

/// A failure reading a downloaded file: one that ends early or holds what it
/// should not is [`Failure::Unusable`] (the same file would fail the same way
/// again), anything else, such as a full disk, [`Failure::Other`].
fn io_failure(e: std::io::Error) -> Failure {
    match e.kind() {
        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::InvalidData => {
            Failure::Unusable(e.to_string())
        }
        _ => Failure::Other(e.to_string()),
    }
}

/// A download in progress, removed when dropped.
struct Partial(PathBuf);

impl Drop for Partial {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Removes what runs that were stopped left behind in a source's cache folder:
/// downloads in progress (`.laz.part`) and caches being written (`.tmp`),
/// once a day old, so no run still at them loses its file.
pub fn sweep_leftovers(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let day = std::time::Duration::from_secs(86_400);
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        let leftover = name.contains(".laz.part") || name.contains(".tmp");
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > day);
        if leftover && old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Turns at something only so many may do at once.
struct Gate {
    busy: Mutex<usize>,
    freed: Condvar,
}

/// A turn at a [`Gate`], given back on drop.
struct Turn(&'static Gate);

impl Gate {
    const fn new() -> Self {
        Gate {
            busy: Mutex::new(0),
            freed: Condvar::new(),
        }
    }

    /// Wait until fewer than `limit` hold a turn, then take one.
    fn enter(&'static self, limit: usize) -> Turn {
        let mut busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        while *busy >= limit.max(1) {
            busy = self.freed.wait(busy).unwrap_or_else(|e| e.into_inner());
        }
        *busy += 1;
        Turn(self)
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        *self.0.busy.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        self.0.freed.notify_all();
    }
}

/// Laser files downloaded at once: two, whatever else is in flight. One server
/// usually holds all of a source's tiles.
static DOWNLOADS: Gate = Gate::new();
const DOWNLOADS_AT_ONCE: usize = 2;

/// Tiles decoded at once. A dense tile holds its points, about 11 bytes each,
/// some 250 MB for 20 million, until they are boiled down.
static DECODES: Gate = Gate::new();

/// Two decodes at a time on a machine of 8 GB or more that has 2 GB free as
/// the decode starts (or a `--ram-budget-mb` of that), else one.
fn decodes_at_once() -> usize {
    static TOTAL_MB: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let total_mb = *TOTAL_MB.get_or_init(|| {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        sys.total_memory() / (1024 * 1024)
    });
    // An 8 GB machine reports a little under 8 GiB where the firmware keeps
    // some for itself.
    let roomy = total_mb >= 7 * 1024 + 512;
    if roomy && crate::data_processing::available_memory_mb() >= 2048 {
        2
    } else {
        1
    }
}

/// Tiles of an area worked on at once, on plain threads: while some decode,
/// the next download.
pub const TILES_AT_ONCE: usize = 4;

/// The parts of a LAS header reading a whole file needs.
struct Header {
    point_offset: u64,
    layout: PointLayout,
    count: u64,
    laszip: Option<Vec<u8>>,
}

fn read_header(file: &mut std::fs::File) -> Result<Header, Failure> {
    let mut h = vec![0u8; 375];
    file.seek(SeekFrom::Start(0)).map_err(io_failure)?;
    let got = file.read(&mut h).map_err(io_failure)?;
    if got < 227 || &h[..4] != b"LASF" {
        return Err(Failure::Unusable("not a LAS/LAZ file".into()));
    }
    let u16_at = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(h[o..o + 4].try_into().unwrap());
    let f64_at = |o: usize| f64::from_le_bytes(h[o..o + 8].try_into().unwrap());
    let header_size = u16_at(94) as u64;
    let point_offset = u32_at(96) as u64;
    let n_vlrs = u32_at(100);
    let format = h[104] & 0x3F;
    let record_len = u16_at(105) as usize;
    let mut count = u32_at(107) as u64;
    if count == 0 && got >= 255 && h[25] >= 4 {
        count = u64::from_le_bytes(h[247..255].try_into().unwrap());
    }
    let scale = [f64_at(131), f64_at(139), f64_at(147)];
    let offset = [f64_at(155), f64_at(163), f64_at(171)];

    let mut laszip = None;
    let mut pos = header_size;
    for _ in 0..n_vlrs {
        let mut vh = [0u8; 54];
        file.seek(SeekFrom::Start(pos)).map_err(io_failure)?;
        file.read_exact(&mut vh).map_err(io_failure)?;
        let user = String::from_utf8_lossy(&vh[2..18]);
        let record_id = u16::from_le_bytes([vh[18], vh[19]]);
        let len = u16::from_le_bytes([vh[20], vh[21]]) as usize;
        if user.trim_end_matches('\0') == "laszip encoded" && record_id == 22204 {
            let mut data = vec![0u8; len];
            file.read_exact(&mut data).map_err(io_failure)?;
            laszip = Some(data);
        }
        pos += 54 + len as u64;
    }
    Ok(Header {
        point_offset,
        layout: PointLayout {
            format,
            record_len,
            scale,
            offset,
        },
        count,
        laszip,
    })
}

/// Decode a downloaded tile and reduce it to its per-metre measurements. A
/// file this reader cannot use, or one whose points end before its header
/// says, is [`Failure::Unusable`].
fn measure_file(
    path: &Path,
    source: &Sheet,
    te: i32,
    tn: i32,
    url: &str,
) -> Result<LaserTile, Failure> {
    let mut file = std::fs::File::open(path).map_err(|e| Failure::Other(e.to_string()))?;
    let hdr = read_header(&mut file)?;
    let layout = hdr.layout;
    layout.check().map_err(Failure::Unusable)?;
    let mut m = Measure::new(source, te, tn, (0, 0, TILE, TILE));
    file.seek(SeekFrom::Start(hdr.point_offset))
        .map_err(io_failure)?;
    // Points a batch of an uncompressed file, at most 64 MB of them.
    let plain = 50_000.min((64 << 20) / layout.record_len);
    let mut buf = vec![0u8; plain * layout.record_len];
    let mut left = hdr.count;
    match &hdr.laszip {
        Some(vlr) => {
            let vlr = laz::LazVlr::read_from(&vlr[..]).map_err(laz_failure)?;
            // Sixteen chunks a batch, decoded side by side: a 20-million-point
            // tile measures in about 3 s instead of 15. Chunks of their own
            // sizes (u32::MAX) or past reason are read in batches of a size
            // of ours, and no batch takes more than 256 MB.
            let chunk = u64::from(vlr.chunk_size());
            let batch = if (1..=1_000_000).contains(&chunk) {
                chunk * 16
            } else {
                800_000
            };
            let batch = batch.min((256 << 20) / layout.record_len as u64) as usize;
            buf.resize(batch * layout.record_len, 0);
            let reader = std::io::BufReader::new(file);
            let mut dec = laz::ParLasZipDecompressor::new(reader, vlr).map_err(laz_failure)?;
            while left > 0 {
                let n = left.min(batch as u64) as usize;
                let out = &mut buf[..n * layout.record_len];
                dec.decompress_many(out).map_err(laz_failure)?;
                out.chunks_exact(layout.record_len)
                    .for_each(|rec| m.add(&layout, rec));
                left -= n as u64;
            }
        }
        None => {
            let mut reader = std::io::BufReader::new(file);
            while left > 0 {
                let n = left.min(plain as u64) as usize;
                let out = &mut buf[..n * layout.record_len];
                reader.read_exact(out).map_err(io_failure)?;
                out.chunks_exact(layout.record_len)
                    .for_each(|rec| m.add(&layout, rec));
                left -= n as u64;
            }
        }
    }
    let mut t = m.finish(url);
    t.blocks = vec![true; BLOCKS * BLOCKS];
    Ok(t)
}

/// A LAZ decoder's failure, as [`io_failure`] sorts a read's: past a read of
/// the disk, a file that will not decode would not on any later run either.
fn laz_failure(e: laz::LasZipError) -> Failure {
    match e {
        laz::LasZipError::IoError(e) => io_failure(e),
        e => Failure::Unusable(e.to_string()),
    }
}

/// Above-ground point flags.
const BUILDING: u8 = 1;
const PLANT: u8 = 2;
/// Of a class the source leaves unsorted.
const UNSORTED: u8 = 4;
/// Height over which unsorted returns can be a roof.
const ROOF_MIN_M: f32 = 2.5;

/// Values in chunks of a fixed size: a long list grows without copying and
/// without the unused half a doubled `Vec` can carry.
struct Chunked<T> {
    full: Vec<Vec<T>>,
    last: Vec<T>,
}

impl<T> Chunked<T> {
    /// Values a chunk holds; only the first chunk grows to it.
    const CHUNK: usize = 1 << 13;

    fn new() -> Self {
        Chunked {
            full: Vec::new(),
            last: Vec::new(),
        }
    }

    fn push(&mut self, v: T) {
        if self.last.len() == Self::CHUNK {
            let next = Vec::with_capacity(Self::CHUNK);
            self.full.push(std::mem::replace(&mut self.last, next));
        }
        self.last.push(v);
    }

    fn len(&self) -> usize {
        self.full.len() * Self::CHUNK + self.last.len()
    }

    fn iter(&self) -> impl Iterator<Item = &T> {
        self.full.iter().flatten().chain(&self.last)
    }
}

/// The returns of one 100 m block, kept until the tile is read: a few bytes
/// each. A cell within the block (0..10 000) fits in 14 bits.
struct BlockPoints {
    /// Ground returns: the cell times 65 536 plus the intensity.
    ground: Chunked<u32>,
    /// Building and object returns: the cell, flags and height of each.
    above_at: Chunked<u16>,
    above_flags: Chunked<u8>,
    above_z: Chunked<f32>,
}

impl BlockPoints {
    fn new() -> Self {
        BlockPoints {
            ground: Chunked::new(),
            above_at: Chunked::new(),
            above_flags: Chunked::new(),
            above_z: Chunked::new(),
        }
    }
}

/// The tile cell of cell `l` of block `b`.
fn tile_cell(b: usize, l: usize) -> usize {
    let (bx, by) = (b % BLOCKS, b / BLOCKS);
    (by * BLOCK + l / BLOCK) * TILE + bx * BLOCK + l % BLOCK
}

/// Points gathered over a window of a tile, before they are boiled down. They
/// are kept by block and boiled down a block at a time.
struct Measure<'a> {
    source: &'a Sheet,
    /// The tile's west and north edges in grid metres.
    e0: f64,
    n1: f64,
    /// Window in tile cells: first column, first row, end column, end row.
    window: (usize, usize, usize, usize),
    n_all: Vec<u32>,
    /// Cells under a building outline, where the source marks them.
    footprint: Vec<bool>,
    gz_sum: Vec<f64>,
    blocks: Vec<BlockPoints>,
    /// Where the points come in another grid, the map onto the tile's.
    to_grid: Option<Affine>,
}

impl<'a> Measure<'a> {
    fn new(source: &'a Sheet, te: i32, tn: i32, window: (usize, usize, usize, usize)) -> Self {
        Measure {
            source,
            e0: f64::from(te) * 1000.0,
            n1: f64::from(tn + 1) * 1000.0,
            window,
            n_all: vec![0; CELLS],
            footprint: if source.classes.footprint.is_empty() {
                Vec::new()
            } else {
                vec![false; CELLS]
            },
            gz_sum: vec![0.0; CELLS],
            blocks: (0..BLOCKS * BLOCKS).map(|_| BlockPoints::new()).collect(),
            to_grid: None,
        }
    }

    fn add(&mut self, l: &PointLayout, rec: &[u8]) {
        let xi = i32::from_le_bytes(rec[0..4].try_into().unwrap());
        let yi = i32::from_le_bytes(rec[4..8].try_into().unwrap());
        let mut x = f64::from(xi) * l.scale[0] + l.offset[0];
        let mut y = f64::from(yi) * l.scale[1] + l.offset[1];
        if let Some(f) = &self.to_grid {
            (x, y) = f.apply(x, y);
        }
        let col = (x - self.e0).floor();
        let row = (self.n1 - y).floor();
        let (c0, r0, c1, r1) = self.window;
        if !(c0 as f64..c1 as f64).contains(&col) || !(r0 as f64..r1 as f64).contains(&row) {
            return;
        }
        let (row, col) = (row as usize, col as usize);
        let cell = row * TILE + col;
        // Point formats 0-5 keep the class in byte 15 and returns in byte 14
        // (3+3 bits); 6-10 keep returns in byte 14 (4+4 bits) and the class in 16.
        let (class, ret, nret) = if l.format >= 6 {
            (rec[16], rec[14] & 0x0F, rec[14] >> 4)
        } else {
            (rec[15] & 0x1F, rec[14] & 0x07, (rec[14] >> 3) & 0x07)
        };
        let classes = &self.source.classes;
        if classes.footprint.contains(&class) {
            // Not a return: only a mark on the ground.
            self.footprint[cell] = true;
            return;
        }
        self.n_all[cell] += 1;
        let zi = i32::from_le_bytes(rec[8..12].try_into().unwrap());
        let z = f64::from(zi) * l.scale[2] + l.offset[2];
        let block = &mut self.blocks[(row / BLOCK) * BLOCKS + col / BLOCK];
        let at = (row % BLOCK) * BLOCK + col % BLOCK;
        let flags = if classes.ground.contains(&class) {
            // Low plants on the ground read as the darkest return (see
            // `LaserClasses::vegetation`).
            let intensity = if classes.vegetation.contains(&class) {
                0
            } else {
                u16::from_le_bytes([rec[12], rec[13]])
            };
            block.ground.push((at as u32) << 16 | u32::from(intensity));
            self.gz_sum[cell] += z;
            return;
        } else if classes.building.contains(&class) {
            BUILDING
        } else if classes.object.contains(&class) {
            let passes = nret > 1 && ret < nret;
            let plant = if classes.vegetation.is_empty() {
                passes
            } else {
                classes.vegetation.contains(&class) || (passes && classes.unsorted.contains(&class))
            };
            let unsorted = if classes.unsorted.contains(&class) {
                UNSORTED
            } else {
                0
            };
            unsorted | if plant { PLANT } else { 0 }
        } else {
            return;
        };
        block.above_at.push(at as u16);
        block.above_flags.push(flags);
        block.above_z.push(z as f32);
    }

    fn finish(self, origin: &str) -> LaserTile {
        let Measure {
            source,
            n_all,
            footprint,
            gz_sum,
            mut blocks,
            ..
        } = self;
        let mut t = LaserTile::empty(origin);
        for (slot, &n) in t.n_all.iter_mut().zip(&n_all) {
            *slot = n.min(255) as u8;
        }
        drop(n_all);
        const AT: usize = BLOCK * BLOCK;
        // Per cell of the block in hand: where its values start, and where the
        // next one goes.
        let mut start = vec![0u32; AT + 1];
        let mut fill = vec![0u32; AT];

        // Ground: the count and median intensity of each cell, a block at a time.
        let mut n_gnd = vec![0u32; CELLS];
        let mut vals: Vec<u16> = Vec::new();
        for (b, block) in blocks.iter_mut().enumerate() {
            let ground = std::mem::replace(&mut block.ground, Chunked::new());
            if ground.len() == 0 {
                continue;
            }
            start.fill(0);
            for &g in ground.iter() {
                start[(g >> 16) as usize + 1] += 1;
            }
            for l in 0..AT {
                start[l + 1] += start[l];
            }
            fill.copy_from_slice(&start[..AT]);
            vals.clear();
            vals.resize(ground.len(), 0);
            for &g in ground.iter() {
                let l = (g >> 16) as usize;
                vals[fill[l] as usize] = g as u16;
                fill[l] += 1;
            }
            drop(ground);
            for l in 0..AT {
                let v = &mut vals[start[l] as usize..start[l + 1] as usize];
                if v.is_empty() {
                    continue;
                }
                v.sort_unstable();
                let c = tile_cell(b, l);
                n_gnd[c] = v.len() as u32;
                t.n_gnd[c] = v.len().min(255) as u8;
                t.i_gnd[c] = quantile(v, 0.5);
            }
        }
        drop(vals);
        let ground_z = fill_ground(&gz_sum, &n_gnd);
        drop((gz_sum, n_gnd));

        // Where the source marks outlines instead of classing roofs, what stands
        // over an outline or within a metre of it (the eaves) and is not a plant
        // is the roof. Of the rest, building returns and objects over 0.25 m
        // count, at their height above the ground.
        let roof = (!footprint.is_empty()).then(|| grow_by_one(&footprint));
        drop(footprint);
        let (roof, ground_z) = (roof.as_deref(), &ground_z[..]);
        let settle = |c: usize, f: u8, z: f32| -> Option<(u8, f32)> {
            let f = if f & PLANT == 0 && roof.is_some_and(|r| r[c]) {
                BUILDING
            } else {
                f
            };
            let h = z - ground_z[c];
            (f & BUILDING != 0 || h > 0.25).then_some((f, h))
        };
        // Whether this scan left its roofs among the unsorted returns: asked of
        // the whole window at once.
        let guess_roofs = !source.classes.unsorted.is_empty()
            && roofs_unsorted(
                blocks
                    .iter()
                    .enumerate()
                    .flat_map(|(b, block)| counted(b, block, settle).map(|p| p.2)),
            );

        // Each cell's object heights, then its building heights, a block at a
        // time.
        let q = |h: f32| (h * 5.0).round().clamp(0.0, 255.0) as u8;
        let mut n_obj = vec![0u32; AT];
        let mut n_bld = vec![0u32; AT];
        let mut n_veg = vec![0u32; AT];
        let mut fill_bld = vec![0u32; AT];
        let mut kept: Vec<(u32, f32, u8)> = Vec::new();
        let mut heights: Vec<f32> = Vec::new();
        for (b, block) in blocks.iter_mut().enumerate() {
            if block.above_at.len() == 0 {
                continue;
            }
            kept.clear();
            kept.extend(counted(b, block, settle));
            *block = BlockPoints::new();
            if guess_roofs {
                find_roofs(&mut kept);
            }
            n_obj.fill(0);
            n_bld.fill(0);
            n_veg.fill(0);
            for &(l, _, f) in &kept {
                let l = l as usize;
                if f & BUILDING != 0 {
                    n_bld[l] += 1;
                } else {
                    n_obj[l] += 1;
                    if f & PLANT != 0 {
                        n_veg[l] += 1;
                    }
                }
            }
            start[0] = 0;
            for l in 0..AT {
                start[l + 1] = start[l] + n_obj[l] + n_bld[l];
                fill[l] = start[l];
                fill_bld[l] = start[l] + n_obj[l];
            }
            heights.clear();
            heights.resize(start[AT] as usize, 0.0);
            for &(l, h, f) in &kept {
                let next = if f & BUILDING != 0 {
                    &mut fill_bld[l as usize]
                } else {
                    &mut fill[l as usize]
                };
                heights[*next as usize] = h;
                *next += 1;
            }
            for l in 0..AT {
                let c = tile_cell(b, l);
                t.n_obj[c] = n_obj[l].min(255) as u8;
                t.n_bld[c] = n_bld[l].min(255) as u8;
                t.n_veg[c] = n_veg[l].min(255) as u8;
                let (s, m, e) = (
                    start[l] as usize,
                    (start[l] + n_obj[l]) as usize,
                    start[l + 1] as usize,
                );
                let o = &mut heights[s..m];
                if !o.is_empty() {
                    o.sort_by(f32::total_cmp);
                    t.h_obj[c] = q(quantile(o, 0.95));
                }
                let bl = &mut heights[m..e];
                if !bl.is_empty() {
                    bl.sort_by(f32::total_cmp);
                    t.h_bld[c] = q(quantile(bl, 0.95));
                }
            }
        }
        t
    }
}

/// The returns of block `b` that count, as (cell within the block, height,
/// flags): `settle` gives a return's flags and height from its tile cell,
/// flags and z, or `None` when it does not count.
fn counted<'a>(
    b: usize,
    block: &'a BlockPoints,
    settle: impl Fn(usize, u8, f32) -> Option<(u8, f32)> + 'a,
) -> impl Iterator<Item = (u32, f32, u8)> + 'a {
    block
        .above_at
        .iter()
        .zip(block.above_flags.iter())
        .zip(block.above_z.iter())
        .filter_map(move |((&l, &f), &z)| {
            let l = usize::from(l);
            settle(tile_cell(b, l), f, z).map(|(f, h)| (l as u32, h, f))
        })
}

/// The cells set in `m` and their eight neighbours, over a whole tile.
fn grow_by_one(m: &[bool]) -> Vec<bool> {
    let mut out = m.to_vec();
    for row in 0..TILE {
        for col in 0..TILE {
            if !m[row * TILE + col] {
                continue;
            }
            for r in row.saturating_sub(1)..=(row + 1).min(TILE - 1) {
                for k in col.saturating_sub(1)..=(col + 1).min(TILE - 1) {
                    out[r * TILE + k] = true;
                }
            }
        }
    }
    out
}

/// Whether a scan left its buildings among the unsorted returns, from the
/// flags of all its returns that count: no building point, while there are
/// unsorted ones.
fn roofs_unsorted(flags: impl Iterator<Item = u8>) -> bool {
    let mut unsorted = false;
    for f in flags {
        if f & BUILDING != 0 {
            return false;
        }
        unsorted |= f & UNSORTED != 0;
    }
    unsorted
}

/// The roofs of a scan that left them unsorted (see [`roofs_unsorted`]): a
/// cell whose unsorted returns over 2.5 m are nearly all the last of their
/// pulse is hard and solid, a roof; a crown lets part of the pulse through.
/// Those returns then count as building points. Returns are (cell, height,
/// flags).
fn find_roofs(heights: &mut [(u32, f32, u8)]) {
    let mut tall = std::collections::HashMap::<u32, (u32, u32)>::new();
    for &(c, h, f) in heights.iter() {
        if f & UNSORTED != 0 && h >= ROOF_MIN_M {
            let e = tall.entry(c).or_default();
            e.0 += 1;
            if f & PLANT != 0 {
                e.1 += 1;
            }
        }
    }
    for p in heights.iter_mut() {
        let (c, h, f) = *p;
        if f & UNSORTED != 0 && f & PLANT == 0 && h >= ROOF_MIN_M {
            if let Some(&(n, plants)) = tall.get(&c) {
                if n >= 2 && 4 * plants < n {
                    p.2 = BUILDING;
                }
            }
        }
    }
}

/// The value at quantile `q` of sorted `v`, picked the way the lab did:
/// index `floor((n - 1) * q)`.
fn quantile<T: Copy>(v: &[T], q: f64) -> T {
    v[((v.len() - 1) as f64 * q).floor() as usize]
}

/// The ground height of every cell: the mean of its ground returns, and where it
/// has none (under a roof, a dense hedge) grown in from the cells around it.
fn fill_ground(sum: &[f64], count: &[u32]) -> Vec<f32> {
    let mut z: Vec<f32> = (0..CELLS)
        .map(|c| {
            if count[c] > 0 {
                (sum[c] / f64::from(count[c])) as f32
            } else {
                f32::NAN
            }
        })
        .collect();
    let known: Vec<f32> = z.iter().copied().filter(|v| !v.is_nan()).collect();
    if known.is_empty() {
        return vec![0.0; CELLS];
    }
    // Grow inwards a ring at a time; 60 rings cover any house and most halls.
    // Each ring reads the heights as the last one left them: its own are
    // written once all are found.
    let mut holes: Vec<u32> = (0..CELLS as u32)
        .filter(|&c| z[c as usize].is_nan())
        .collect();
    let mut found: Vec<(u32, f32)> = Vec::new();
    for _ in 0..60 {
        found.clear();
        holes.retain(|&c| {
            let (row, col) = (c as usize / TILE, c as usize % TILE);
            let (mut s, mut n) = (0.0f32, 0u32);
            for (dr, dc) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let (r, k) = (row as i32 + dr, col as i32 + dc);
                if r < 0 || k < 0 || r >= TILE as i32 || k >= TILE as i32 {
                    continue;
                }
                let v = z[r as usize * TILE + k as usize];
                if !v.is_nan() {
                    s += v;
                    n += 1;
                }
            }
            if n > 0 {
                found.push((c, s / n as f32));
            }
            n == 0
        });
        if found.is_empty() {
            break;
        }
        for &(c, v) in &found {
            z[c as usize] = v;
        }
    }
    let mut sorted = known;
    sorted.sort_by(f32::total_cmp);
    let median = sorted[sorted.len() / 2];
    z.iter()
        .map(|&v| if v.is_nan() { median } else { v })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roofs_are_found_only_where_the_scan_left_buildings_unsorted() {
        let u = UNSORTED;
        // Cell 1: a roof (single returns at 6 m). Cell 2: a crown (most returns
        // pass through). Cell 3: a hedge under 2.5 m.
        let pts = vec![
            (1, 6.0, u),
            (1, 6.1, u),
            (1, 6.2, u),
            (2, 8.0, u | PLANT),
            (2, 7.5, u | PLANT),
            (2, 9.0, u),
            (3, 1.5, u),
            (3, 1.6, u),
        ];
        assert!(roofs_unsorted(pts.iter().map(|p| p.2)));
        let mut h = pts.clone();
        find_roofs(&mut h);
        assert!(h[..3].iter().all(|p| p.2 == BUILDING));
        assert_eq!(&h[3..], &pts[3..]);
        // A scan that sorts its buildings out is left alone.
        let mut h = pts.clone();
        h.push((9, 7.0, BUILDING));
        assert!(!roofs_unsorted(h.iter().map(|p| p.2)));
        // So is a source without unsorted classes.
        assert!(!roofs_unsorted(pts.iter().map(|p| p.2 & !u)));
    }

    #[test]
    fn blocks_cover_the_rectangle_and_no_more() {
        // Tile 358_6687: 358_000..359_000 E, 6_687_000..6_688_000 N. A rectangle
        // over its north-east corner, 150 m by 250 m.
        let want = blocks_for(358, 6687, (358_850.0, 6_687_750.0, 359_400.0, 6_688_300.0));
        let on: Vec<(usize, usize)> = (0..BLOCKS * BLOCKS)
            .filter(|&b| want[b])
            .map(|b| (b % BLOCKS, b / BLOCKS))
            .collect();
        // Columns 8 and 9 (850..1000 m), rows 0..2 from the north (250 m).
        assert_eq!(on, vec![(8, 0), (9, 0), (8, 1), (9, 1), (8, 2), (9, 2)]);
        // Touching an edge is not meeting it.
        let none = blocks_for(358, 6687, (359_000.0, 6_687_000.0, 359_500.0, 6_688_000.0));
        assert!(none.iter().all(|&b| !b));
    }

    #[test]
    fn a_partial_tile_is_cached_as_partial() {
        let dir = std::env::temp_dir().join(format!("arnis-laser-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tile.bin");
        let url = "https://example.org/a.copc.laz";
        let mut measured = LaserTile::empty(url);
        measured.n_all.iter_mut().for_each(|v| *v = 9);
        measured.i_gnd.iter_mut().for_each(|v| *v = 1234);
        // Only the north-west block was read.
        let mut fill = vec![false; BLOCKS * BLOCKS];
        fill[0] = true;
        let mut t = LaserTile::empty(url);
        t.take_blocks(&measured, &fill);
        assert_eq!((t.n_all[0], t.n_all[99 * TILE + 99]), (9, 9));
        assert_eq!(t.n_all[100], 0, "the next block stays empty");
        t.write(&path).unwrap();

        let back = LaserTile::read(&path).expect("reads back");
        assert_eq!(back.origin, url);
        assert_eq!(back.i_gnd[5 * TILE + 5], 1234);
        // It answers for the block it has, not for a larger area.
        assert!(back.has(&fill));
        let mut more = fill.clone();
        more[1] = true;
        assert!(!back.has(&more));
        std::fs::remove_dir_all(&dir).ok();
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("arnis-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn a_lookup_notes_only_the_blocks_it_found_nothing_for() {
        let root = temp_root("absent-lookup");
        let swiss = Sheet::at(&super::super::switzerland::SWITZERLAND, 8.28);
        let (mut west, mut east) = (vec![false; BLOCKS * BLOCKS], vec![false; BLOCKS * BLOCKS]);
        west[0] = true;
        east[9] = true;
        assert!(!is_cached(&root, &swiss, 2663, 1210, &west));
        mark_absent(&root, &swiss, (2663, 1210), &west);
        // Asked about again, the west block costs nothing and reads as absent,
        // without a request; the east block another area wants is still open.
        assert!(is_cached(&root, &swiss, 2663, 1210, &west));
        assert_eq!(missing_km2(&root, &swiss, 2663, 1210, &west), 0.0);
        assert!(!is_cached(&root, &swiss, 2663, 1210, &east));
        let client = reqwest::blocking::Client::new();
        let got = tile(&client, &root, &swiss, (2663, 1210), &west, &[]).unwrap();
        assert!(matches!(got, Fetched::Absent));
        // Noted again, the blocks add up.
        mark_absent(&root, &swiss, (2663, 1210), &east);
        let both: Vec<bool> = west.iter().zip(&east).map(|(&a, &b)| a || b).collect();
        assert!(is_cached(&root, &swiss, 2663, 1210, &both));
        // Past its days, asked again.
        let marker = absent_marker(&root, &swiss, 2663, 1210);
        let old = std::time::SystemTime::now()
            - std::time::Duration::from_secs((LOOKUP_ABSENT_DAYS + 1) * 86_400);
        let f = std::fs::File::options().write(true).open(&marker).unwrap();
        f.set_modified(old).unwrap();
        assert!(!is_cached(&root, &swiss, 2663, 1210, &west));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_fixed_address_without_a_file_is_absent_whole() {
        let root = temp_root("absent-fixed");
        let bavaria = Sheet::at(&super::super::bavaria::BAVARIA, 11.9);
        let mut one = vec![false; BLOCKS * BLOCKS];
        one[55] = true;
        mark_absent(&root, &bavaria, (714, 5353), &one);
        assert!(is_cached(
            &root,
            &bavaria,
            714,
            5353,
            &[true; BLOCKS * BLOCKS]
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn what_is_cached_serves_where_the_rest_is_absent() {
        let root = temp_root("absent-partial");
        let swiss = Sheet::at(&super::super::switzerland::SWITZERLAND, 8.28);
        let url = "https://example.org/a.copc.laz";
        let mut t = LaserTile::empty(url);
        t.n_all.iter_mut().for_each(|v| *v = 7);
        t.blocks[0] = true;
        std::fs::create_dir_all(root.join(swiss.dir())).unwrap();
        t.write(&cache_path(&root, &swiss, 2663, 1210)).unwrap();
        let mut gone = vec![false; BLOCKS * BLOCKS];
        gone[1] = true;
        mark_absent(&root, &swiss, (2663, 1210), &gone);
        let mut want = gone.clone();
        want[0] = true;
        assert!(is_cached(&root, &swiss, 2663, 1210, &want));
        let client = reqwest::blocking::Client::new();
        match tile(&client, &root, &swiss, (2663, 1210), &want, &[]).unwrap() {
            Fetched::Tile(t) => assert_eq!(t.n_all[0], 7),
            Fetched::Absent => panic!("the cached block is had"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_whole_file_that_cannot_be_used_is_unusable() {
        let root = temp_root("unusable");
        std::fs::create_dir_all(&root).unwrap();
        let bavaria = Sheet::at(&super::super::bavaria::BAVARIA, 11.9);
        let unusable = |r: Result<LaserTile, Failure>| matches!(r, Err(Failure::Unusable(_)));
        // Not a LAS file.
        let junk = root.join("junk.laz");
        std::fs::write(&junk, vec![7u8; 4096]).unwrap();
        assert!(unusable(measure_file(&junk, &bavaria, 714, 5353, "u")));
        // A header that counts ten points of 20 bytes and none that follow.
        let mut h = vec![0u8; 227];
        h[..4].copy_from_slice(b"LASF");
        h[24] = 1;
        h[25] = 2;
        h[94..96].copy_from_slice(&227u16.to_le_bytes());
        h[96..100].copy_from_slice(&227u32.to_le_bytes());
        h[105..107].copy_from_slice(&20u16.to_le_bytes());
        h[107..111].copy_from_slice(&10u32.to_le_bytes());
        for o in [131, 139, 147] {
            h[o..o + 8].copy_from_slice(&0.01f64.to_le_bytes());
        }
        let short = root.join("short.las");
        std::fs::write(&short, &h).unwrap();
        assert!(unusable(measure_file(&short, &bavaria, 714, 5353, "u")));
        // A zip without a LAS file in it.
        let zipped = root.join("t.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&zipped).unwrap());
        w.start_file("readme.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(b"no points").unwrap();
        w.finish().unwrap();
        assert!(matches!(
            unzip(&zipped, &root.join("t.las")),
            Err(Failure::Unusable(_))
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_zipped_tile_is_let_go_when_the_index_lists_a_newer_delivery() {
        let root = temp_root("supersede");
        let swiss = Sheet::at(&super::super::switzerland::SWITZERLAND, 8.28);
        let zip = "https://example.org/swisssurface3d_2019_2663-1210.las.zip";
        let mut t = LaserTile::empty(zip);
        t.blocks = vec![true; BLOCKS * BLOCKS];
        std::fs::create_dir_all(root.join(swiss.dir())).unwrap();
        let path = cache_path(&root, &swiss, 2663, 1210);
        t.write(&path).unwrap();
        assert!(!due_recheck(&root, &swiss, (2663, 1210)));
        let old = std::time::SystemTime::now()
            - std::time::Duration::from_secs((ZIP_RECHECK_DAYS + 1) * 86_400);
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        f.set_modified(old).unwrap();
        assert!(due_recheck(&root, &swiss, (2663, 1210)));
        // Still the delivery: kept, and not due again for a while.
        assert!(!supersede(&root, &swiss, (2663, 1210), zip));
        assert!(path.exists() && !due_recheck(&root, &swiss, (2663, 1210)));
        // Now cloud-optimised: let go, to be read afresh.
        let copc = "https://example.org/swisssurface3d_2024_2663-1210.copc.laz";
        assert!(supersede(&root, &swiss, (2663, 1210), copc));
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ground_fills_a_hole_from_its_rim() {
        let mut sum = vec![0.0f64; CELLS];
        let mut count = vec![0u32; CELLS];
        for c in 0..CELLS {
            sum[c] = 450.0;
            count[c] = 1;
        }
        // A 10 x 10 roof with no ground returns under it.
        for r in 100..110 {
            for k in 100..110 {
                sum[r * TILE + k] = 0.0;
                count[r * TILE + k] = 0;
            }
        }
        let z = fill_ground(&sum, &count);
        assert!((z[105 * TILE + 105] - 450.0).abs() < 1e-3);
    }

    #[test]
    fn outline_marks_turn_the_last_returns_over_them_into_roof() {
        let src = &Sheet::at(&super::super::nrw::NRW, 7.0);
        let layout = PointLayout {
            format: 1,
            record_len: 28,
            scale: [0.01, 0.01, 0.01],
            offset: [0.0, 0.0, 0.0],
        };
        // A point in tile 367_5646 at cell (col, row), z metres, class, return
        // `ret` of `nret`.
        let rec = |col: usize, row: usize, z: f64, class: u8, ret: u8, nret: u8| {
            let mut r = vec![0u8; 28];
            let x = ((367_000.0 + col as f64 + 0.5) / 0.01) as i32;
            let y = ((5_647_000.0 - row as f64 - 0.5) / 0.01) as i32;
            r[0..4].copy_from_slice(&x.to_le_bytes());
            r[4..8].copy_from_slice(&y.to_le_bytes());
            r[8..12].copy_from_slice(&((z / 0.01) as i32).to_le_bytes());
            r[14] = ret | (nret << 3);
            r[15] = class;
            r
        };
        let mut m = Measure::new(src, 367, 5646, (0, 0, TILE, TILE));
        for col in 490..520 {
            m.add(&layout, &rec(col, 500, 100.0, 2, 1, 1));
        }
        // An outline mark at column 500; a roof's last return on it and one
        // a metre off (the eaves), a crown's first return on it, and a last
        // return two metres off, which stays an object.
        m.add(&layout, &rec(500, 500, 100.0, 26, 1, 1));
        m.add(&layout, &rec(500, 500, 106.0, 20, 1, 1));
        m.add(&layout, &rec(501, 500, 105.0, 20, 1, 1));
        m.add(&layout, &rec(500, 500, 109.0, 1, 1, 2));
        m.add(&layout, &rec(502, 500, 106.0, 20, 1, 1));
        let t = m.finish("test");
        let c = |col: usize| 500 * TILE + col;
        assert_eq!(
            (t.n_bld[c(500)], t.n_obj[c(500)], t.n_veg[c(500)]),
            (1, 1, 1)
        );
        assert_eq!(t.h_bld[c(500)], 30, "6 m in fifths");
        assert_eq!((t.n_bld[c(501)], t.n_obj[c(501)]), (1, 0));
        assert_eq!((t.n_bld[c(502)], t.n_obj[c(502)]), (0, 1));
        // The mark is not a return.
        assert_eq!(t.n_all[c(500)], 3);
    }

    #[test]
    fn low_plants_on_the_ground_read_as_the_darkest_ground() {
        let src = &Sheet::at(&super::super::latvia::LATVIA, 24.0);
        let layout = PointLayout {
            format: 1,
            record_len: 28,
            scale: [0.01, 0.01, 0.01],
            offset: [0.0, 0.0, 0.0],
        };
        // A point in tile 501_308 at cell (col, row), class, intensity.
        let rec = |col: usize, row: usize, class: u8, intensity: u16| {
            let mut r = vec![0u8; 28];
            let x = ((501_000.0 + col as f64 + 0.5) / 0.01) as i32;
            let y = ((309_000.0 - row as f64 - 0.5) / 0.01) as i32;
            r[0..4].copy_from_slice(&x.to_le_bytes());
            r[4..8].copy_from_slice(&y.to_le_bytes());
            r[8..12].copy_from_slice(&((10.0 / 0.01) as i32).to_le_bytes());
            r[12..14].copy_from_slice(&intensity.to_le_bytes());
            r[14] = 1 | (1 << 3);
            r[15] = class;
            r
        };
        let mut m = Measure::new(src, 501, 308, (0, 0, TILE, TILE));
        // Paving: ground returns only. Meadow: mostly low vegetation.
        for _ in 0..3 {
            m.add(&layout, &rec(10, 10, 2, 90));
            m.add(&layout, &rec(20, 10, 3, 40));
        }
        m.add(&layout, &rec(20, 10, 2, 90));
        let t = m.finish("test");
        let c = |col: usize| 10 * TILE + col;
        assert_eq!((t.n_gnd[c(10)], t.i_gnd[c(10)]), (3, 90));
        assert_eq!((t.n_gnd[c(20)], t.i_gnd[c(20)]), (4, 0));
        assert_eq!(t.n_obj[c(20)], 0);
    }

    #[test]
    fn quantile_matches_the_lab() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        assert_eq!(quantile(&v, 0.95), 9.0);
        assert_eq!(quantile(&v, 0.5), 5.0);
    }

    #[test]
    fn block_by_block_measures_as_cell_by_cell() {
        // Random returns over cells either side of block seams, measured by
        // `Measure` and by a plain list per cell.
        let src = &Sheet::at(&super::super::nrw::NRW, 7.0);
        let layout = PointLayout {
            format: 1,
            record_len: 28,
            scale: [0.01, 0.01, 0.01],
            offset: [0.0, 0.0, 0.0],
        };
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let mut m = Measure::new(src, 367, 5646, (0, 0, TILE, TILE));
        // Per cell: ground (intensity, z), above (z, class, ret, nret), marks.
        type Cell = (Vec<(u16, f64)>, Vec<(f64, u8, u8, u8)>, bool);
        let mut cells: std::collections::BTreeMap<usize, Cell> = Default::default();
        let mut put =
            |row: usize, col: usize, class: u8, z: f64, ret: u8, nret: u8, intensity: u16| {
                let mut r = vec![0u8; 28];
                let x = ((367_000.0 + col as f64 + 0.5) / 0.01) as i32;
                let y = ((5_647_000.0 - row as f64 - 0.5) / 0.01) as i32;
                r[0..4].copy_from_slice(&x.to_le_bytes());
                r[4..8].copy_from_slice(&y.to_le_bytes());
                r[8..12].copy_from_slice(&((z / 0.01).round() as i32).to_le_bytes());
                r[12..14].copy_from_slice(&intensity.to_le_bytes());
                r[14] = ret | (nret << 3);
                r[15] = class;
                m.add(&layout, &r);
                let z = f64::from(i32::from_le_bytes(r[8..12].try_into().unwrap())) * 0.01;
                let e = cells.entry(row * TILE + col).or_default();
                match class {
                    2 => e.0.push((intensity, z)),
                    20 => e.1.push((z, class, ret, nret)),
                    26 => e.2 = true,
                    _ => {}
                }
            };
        for _ in 0..40_000 {
            let (row, col) = (95 + rnd(12) as usize, 195 + rnd(12) as usize);
            let class = [2, 2, 2, 20, 20, 26, 7][rnd(7) as usize];
            let z = 100.0 + rnd(900) as f64 / 100.0;
            let (ret, nret) = (1 + rnd(2) as u8, 1 + rnd(2) as u8);
            put(row, col, class, z, ret, nret, 1000 + rnd(900) as u16);
        }
        // Sparse ground over the rest of the tile, so the ground under it fills
        // in a few rings.
        for row in (3..TILE).step_by(10) {
            for col in (3..TILE).step_by(10) {
                put(row, col, 2, 99.0, 1, 1, 1200);
            }
        }
        let t = m.finish("test");

        let mut sum = vec![0.0f64; CELLS];
        let mut count = vec![0u32; CELLS];
        let mut marks = vec![false; CELLS];
        for (&c, (g, _, mark)) in &cells {
            for &(_, z) in g {
                sum[c] += z;
            }
            count[c] = g.len() as u32;
            marks[c] = *mark;
        }
        let ground_z = fill_ground(&sum, &count);
        let roof = grow_by_one(&marks);
        for (&c, (g, above, _)) in &cells {
            let mut i: Vec<u16> = g.iter().map(|p| p.0).collect();
            i.sort_unstable();
            assert_eq!(t.n_gnd[c], i.len() as u8);
            assert_eq!(t.i_gnd[c], if i.is_empty() { 0 } else { quantile(&i, 0.5) });
            let (mut obj, mut bld, mut veg) = (Vec::new(), Vec::new(), 0u8);
            for &(z, _, ret, nret) in above {
                let plant = nret > 1 && ret < nret;
                let h = z as f32 - ground_z[c];
                if !plant && roof[c] {
                    bld.push(h);
                } else if h > 0.25 {
                    obj.push(h);
                    veg += u8::from(plant);
                }
            }
            obj.sort_by(f32::total_cmp);
            bld.sort_by(f32::total_cmp);
            let q = |v: &[f32]| {
                if v.is_empty() {
                    0
                } else {
                    (quantile(v, 0.95) * 5.0).round().clamp(0.0, 255.0) as u8
                }
            };
            assert_eq!(
                (t.n_obj[c], t.n_bld[c], t.n_veg[c], t.h_obj[c], t.h_bld[c]),
                (obj.len() as u8, bld.len() as u8, veg, q(&obj), q(&bld)),
                "cell {c}"
            );
        }
        // Cells without returns stay empty, including under a grown roof.
        assert_eq!(t.n_all.iter().filter(|&&n| n > 0).count(), {
            cells
                .values()
                .filter(|(g, a, _)| !g.is_empty() || !a.is_empty())
                .count()
        });
    }
}
