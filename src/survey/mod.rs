//! Garden detail: what stands in the places OSM leaves empty, measured rather than
//! guessed, from open airborne laser scans and aerial photographs.
//!
//! Where a source covers the area (see [`source::SOURCES`]: Bavaria, North
//! Rhine-Westphalia, France, the Netherlands, Switzerland, Estonia, Luxembourg,
//! the United States, Canada), the parts of its laser tiles and photos the area
//! needs are read ([`copc`] and [`ept`] read octrees in part), boiled down to
//! a few numbers per square metre ([`laser`], [`photo`]), classified per metre
//! on the source's own grid ([`classify`], [`crs`]) and sampled onto the world's
//! block grid the same way the canopy map is. The ground pass reads it for columns no mapped feature
//! claimed; sheds and pools become ordinary elements beside the OSM ones
//! ([`elements`]). OSM always wins: nothing here overrides a mapped feature.
//!
//! Downloads are bounded: only tiles inside the source's outline, only the photo
//! blocks the area touches, results cached per tile, and a limit on what one run
//! may fetch of the laser scans (`--lidar-max-mb`).
//!
//! `ARNIS_LASER_DEBUG=1` prints, to stderr, each COPC or EPT tile's read (level,
//! nodes, requests, megabytes, points a square metre) and the time and resident
//! memory at each stage.

pub mod bavaria;
pub mod canada;
pub mod classify;
pub mod copc;
pub mod crs;
pub mod elements;
pub mod ept;
pub mod estonia;
pub mod fetch;
pub mod france;
pub mod grid;
pub mod heights;
pub mod index;
pub mod laser;
pub mod luxembourg;
pub mod netherlands;
pub mod nrw;
pub mod photo;
pub mod place;
pub mod source;
pub mod switzerland;
pub mod usa;
pub mod utm;

use crate::coordinate_system::geographic::LLBBox;
use source::{Laser, Locate, Sheet};
use std::path::PathBuf;

pub use place::{Cell, Kind};

/// Which laser features a run uses. The data is fetched once for all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    pub garden: bool,
    pub trees: bool,
    pub heights: bool,
}

/// Margin around the area, in metres: components that cross the edge are judged
/// whole, and the photo's lean has room.
const MARGIN_M: f64 = 40.0;
/// Largest area the survey runs for, in square kilometres of source grid. Garden
/// detail is a town-scale feature; past this the run is region-scale and the
/// downloads would be too.
const MAX_AREA_KM2: f64 = 16.0;

/// The classified survey on the world's sampling grid (like the canopy map:
/// equal latitude rows from the bbox's north edge, one cell per block).
#[derive(Clone)]
pub struct SurveyData {
    /// class, height in fifths of a metre, paving colour r g b.
    cells: Vec<[u8; 5]>,
    pub width: usize,
    pub height: usize,
    pub features: Features,
}

impl SurveyData {
    /// A survey from its raw cells, for tests elsewhere.
    #[cfg(test)]
    pub fn from_cells(
        cells: Vec<[u8; 5]>,
        width: usize,
        height: usize,
        features: Features,
    ) -> Self {
        assert_eq!(cells.len(), width * height);
        SurveyData {
            cells,
            width,
            height,
            features,
        }
    }

    pub fn remap_rows_to_mercator(&mut self, lat_top: f64, lat_bottom: f64) {
        let rows = self.height;
        crate::grid_ops::remap_flat_rows_nearest(&mut self.cells, self.width, |gz| {
            crate::grid_ops::mercator_source_row(lat_top, lat_bottom, rows, gz)
        });
    }

    pub fn crop(&mut self, x0: usize, z0: usize, width: usize, height: usize) {
        crate::grid_ops::crop_flat(&mut self.cells, self.width, x0, z0, width, height);
        self.width = width;
        self.height = height;
    }

    /// The raw cell at (`gx`, `gz`), for carrying the grid through a rotation.
    pub fn raw_at(&self, gx: usize, gz: usize) -> [u8; 5] {
        self.cells[gz.min(self.height - 1) * self.width + gx.min(self.width - 1)]
    }

    /// Puts the cells on a new `width` x `height` grid, as a rotation leaves them.
    pub fn regrid(&mut self, cells: Vec<[u8; 5]>, width: usize, height: usize) {
        debug_assert_eq!(cells.len(), width * height);
        self.cells = cells;
        self.width = width;
        self.height = height;
    }

    pub fn at_any(&self, gx: usize, gz: usize) -> Option<Cell> {
        if gx >= self.width || gz >= self.height {
            return None;
        }
        Cell::decode(self.cells[gz * self.width + gx])
    }

    /// Cells of each class, for the log line.
    fn counts(&self) -> [usize; 11] {
        let mut n = [0usize; 11];
        for c in &self.cells {
            n[(c[0] as usize).min(10)] += 1;
        }
        n
    }
}

/// Folder of the laser cache under the cache root.
pub const CACHE_DIR: &str = "laser";

pub fn cache_root() -> PathBuf {
    match crate::elevation::cache::user_cache_dir() {
        Some(d) => d.join(CACHE_DIR),
        None => PathBuf::from(format!("./arnis-{CACHE_DIR}-cache")),
    }
}

pub fn clear_cache() -> crate::elevation::cache::CacheClearStats {
    crate::elevation::cache::clear_cache_dir(&cache_root())
}

/// The source, its tiles and the grid rectangle a bbox needs, or why not.
pub struct Plan {
    /// The source on the area's grid.
    pub sheet: Sheet,
    pub tiles: Vec<(i32, i32)>,
    /// West, south, east, north in the source's grid metres, whole metres.
    pub rect: (f64, f64, f64, f64),
    /// Megabytes of laser data not yet cached, estimated.
    pub download_mb: u32,
    /// The tiles that lack some of what the area wants.
    pub missing: Vec<(i32, i32)>,
}

impl Plan {
    /// Square kilometres of the rectangle.
    pub fn area_km2(&self) -> f64 {
        let (e0, n0, e1, n1) = self.rect;
        (e1 - e0) * (n1 - n0) / 1e6
    }

    /// Whether the area is past the size the survey runs for.
    pub fn too_large(&self) -> bool {
        self.area_km2() > MAX_AREA_KM2
    }

    /// The blocks of a tile the rectangle needs.
    pub fn want(&self, (te, tn): (i32, i32)) -> Vec<bool> {
        laser::blocks_for(te, tn, self.rect)
    }
}

pub fn plan(bbox: &LLBBox) -> Option<Plan> {
    plan_in(&cache_root(), bbox)
}

/// Whether only the margin reaches tile (`te`, `tn`): the rectangle meets it in
/// a strip no wider than [`MARGIN_M`], so none of the world lies on it. A source
/// read in whole files leaves such a tile out, and its 50 to 200 MB download,
/// at the cost of some context for what crosses the world's edge (on Erding,
/// the shade of one paved strip along it). Reaching further in, the world's
/// edge would go without laser data, and its gardens and roofs would change
/// there visibly.
fn sliver((te, tn): (i32, i32), (e0, n0, e1, n1): source::Rect) -> bool {
    let (te0, tn0) = (f64::from(te) * 1000.0, f64::from(tn) * 1000.0);
    let w = e1.min(te0 + 1000.0) - e0.max(te0);
    let h = n1.min(tn0 + 1000.0) - n0.max(tn0);
    w.min(h) <= MARGIN_M
}

/// [`plan`] against the laser cache in `root`.
pub fn plan_in(root: &std::path::Path, bbox: &LLBBox) -> Option<Plan> {
    let source = source::for_bbox(bbox)?;
    let sheet = Sheet::at(source, (bbox.min().lng() + bbox.max().lng()) / 2.0);
    let corners = [
        (bbox.min().lat(), bbox.min().lng()),
        (bbox.min().lat(), bbox.max().lng()),
        (bbox.max().lat(), bbox.min().lng()),
        (bbox.max().lat(), bbox.max().lng()),
    ];
    let (mut e0, mut n0, mut e1, mut n1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (lat, lon) in corners {
        let (e, n) = sheet.crs.forward(lat, lon);
        e0 = e0.min(e);
        n0 = n0.min(n);
        e1 = e1.max(e);
        n1 = n1.max(n);
    }
    let rect = (
        (e0 - MARGIN_M).floor(),
        (n0 - MARGIN_M).floor(),
        (e1 + MARGIN_M).ceil(),
        (n1 + MARGIN_M).ceil(),
    );
    let mut tiles = sheet.tiles(rect);
    // Sources read in part fetch only what the area needs anyway, but for a
    // cloud-optimised source's zipped tiles, read whole: those listed so before
    // are left out here, the rest once found (see `fetch_survey_data`).
    match sheet.laser {
        Laser::Whole { .. } => tiles.retain(|&t| !sliver(t, rect)),
        Laser::Copc { .. } => {
            tiles.retain(|&t| !(sliver(t, rect) && laser::noted_whole(root, &sheet, t)))
        }
        Laser::Ept { .. } => {}
    }
    // Square kilometres each tile lacks of what the area wants, from the start
    // of its cache file: side by side.
    use rayon::prelude::*;
    let lacking: Vec<f64> = tiles
        .par_iter()
        .map(|&(te, tn)| laser::missing_km2(root, &sheet, te, tn, &laser::blocks_for(te, tn, rect)))
        .collect();
    let missing: Vec<(i32, i32)> = tiles
        .iter()
        .zip(&lacking)
        .filter(|(_, &km2)| km2 > 0.0)
        .map(|(&t, _)| t)
        .collect();
    let download_mb = match sheet.laser {
        // Read whole: a tile missing any block costs its whole file.
        Laser::Whole { tile_mb, .. } => missing.len() as u32 * tile_mb,
        Laser::Copc { mb_per_km2, .. } | Laser::Ept { mb_per_km2, .. } => {
            let km2: f64 = lacking.iter().sum();
            (km2 * f64::from(mb_per_km2)).ceil() as u32
        }
    };
    Some(Plan {
        sheet,
        tiles,
        rect,
        download_mb,
        missing,
    })
}

/// Megabytes the plan's missing tiles take, the whole files asked of the server
/// (a HEAD each) where their addresses are fixed: Bavaria's tiles run from 70 MB
/// to past 1 GB in the Alps, so the source's typical size can be far off. A file
/// the server does not size counts at that. A cloud-optimised source's zipped
/// tiles (see [`zip_url`]), which the plan counted by the part the area needs,
/// are sized the same way once found.
fn sized_download_mb(client: &reqwest::blocking::Client, plan: &Plan, urls: &Urls) -> u32 {
    if plan.missing.is_empty() {
        return 0;
    }
    if crate::net::offline() {
        return plan.download_mb;
    }
    use rayon::prelude::*;
    let bytes: u64 = match plan.sheet.laser {
        Laser::Whole {
            locate: Locate::Urls(_),
            tile_mb,
        } => {
            let typical = u64::from(tile_mb) * 1_000_000;
            plan.missing
                .par_iter()
                .map(|t| {
                    urls.get(t)
                        .and_then(|u| u.first())
                        .and_then(|u| fetch::content_length(client, u))
                        .unwrap_or(typical)
                })
                .sum()
        }
        // Once any zip is found: the plan counted it by area, or counted one
        // left out since (see `fetch_survey_data`).
        Laser::Copc { mb_per_km2, .. } if urls.values().flatten().any(|u| laser::is_zip(u)) => {
            let root = cache_root();
            plan.missing
                .par_iter()
                .map(|&t| match zip_url(urls, t) {
                    Some(u) => fetch::content_length(client, u).unwrap_or(ZIP_MB * 1_000_000),
                    // The rest as the plan counted it.
                    None => {
                        let km2 = laser::missing_km2(&root, &plan.sheet, t.0, t.1, &plan.want(t));
                        (km2 * f64::from(mb_per_km2) * 1e6) as u64
                    }
                })
                .sum()
        }
        _ => return plan.download_mb,
    };
    bytes.div_ceil(1_000_000) as u32
}

/// A zipped tile's size where its server does not give it, erring high (the two
/// measured in Switzerland take 135 and 159 MB).
const ZIP_MB: u64 = 200;

/// The status message for laser data over the download limit, followed by the
/// megabytes needed and the limit; the window says it in the user's language.
pub const OVER_LIMIT: &str = "laser_over_limit:";

/// Says the run goes on without laser data for its download size, how much it
/// needs and how to allow it: `--lidar-max-mb` here, the Download Limit in the
/// window.
fn over_limit(need_mb: u32, max_mb: u32) {
    println!(
        "Laser data skipped: the area needs about {need_mb} MB of laser scans, more than the {max_mb} MB allowed. Allow it with --lidar-max-mb {}.",
        limit_for(need_mb)
    );
    crate::progress::emit_gui_progress_update(
        crate::progress::MESSAGE_ONLY,
        &format!("{OVER_LIMIT} {need_mb} {max_mb}"),
    );
    // A prewarm leaves them out of the cache too.
    crate::net::note_uncached("laser scans");
}

/// The limit to suggest for `need_mb`: the window's next step (1, 2 or 5 GB),
/// past those the next whole gigabyte.
fn limit_for(need_mb: u32) -> u32 {
    [250, 500, 1000, 2000, 5000]
        .into_iter()
        .find(|&step| step >= need_mb)
        .unwrap_or(need_mb.div_ceil(1000) * 1000)
}

/// Fetch and classify the survey for `bbox`, sampled on a `grid_w x grid_h` grid
/// the way the canopy map is. `None` where no source covers the area, the area is
/// too large, the download budget would be exceeded, or the data cannot be had.
pub fn fetch_survey_data(
    bbox: &LLBBox,
    grid_w: usize,
    grid_h: usize,
    max_mb: u32,
    features: Features,
) -> Option<SurveyData> {
    if grid_w < 2 || grid_h < 2 {
        return None;
    }
    let mut plan = plan(bbox)?;
    let (e0, _, _, n1) = plan.rect;
    if plan.too_large() {
        println!(
            "Laser data skipped: the area is {:.0} km², it is used for up to {MAX_AREA_KM2:.0}",
            plan.area_km2()
        );
        return None;
    }
    // A blocking client's timeout holds for the headers and for each read of a
    // body, not the whole answer: a host gone silent fails in a minute, while a
    // tile of a gigabyte may take as long as it downloads.
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(60))
        .user_agent(concat!("arnis/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let urls = locate(&client, &plan)?;
    // A tile found zipped is read whole: left out where only the margin reaches
    // it, as a source read in whole files leaves it out.
    let grazed = |t: &(i32, i32)| sliver(*t, plan.rect) && zip_url(&urls, *t).is_some();
    plan.tiles.retain(|t| !grazed(t));
    plan.missing.retain(|t| !grazed(t));
    let need_mb = sized_download_mb(&client, &plan, &urls);
    if need_mb > max_mb {
        over_limit(need_mb, max_mb);
        return None;
    }
    println!(
        "Fetching laser data ({} laser scans, {} tile{})...",
        plan.sheet.name,
        plan.tiles.len(),
        if plan.tiles.len() == 1 { "" } else { "s" }
    );
    let (bytes0, area0) = (fetch::laser_bytes(), fetch::laser_area_m2());
    let debug = std::env::var_os("ARNIS_LASER_DEBUG").is_some();
    let clock = std::time::Instant::now();
    let lap = |what: &str| {
        if debug {
            let mut sys = sysinfo::System::new();
            let pid = sysinfo::Pid::from_u32(std::process::id());
            sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
            let rss = sys.process(pid).map_or(0, |p| p.memory() >> 20);
            eprintln!(
                "laser timing: {what} by {:.2} s, {rss} MB resident",
                clock.elapsed().as_secs_f64()
            );
        }
    };
    // Heights read only the laser's roofs; lawns and trees need the photo too.
    // The sizes above are the server's or an estimate; the bytes that come are
    // held to the limit as well, and past it the area goes without.
    fetch::limit_laser_bytes(u64::from(max_mb) * 1_000_000);
    let inputs = mosaic(&client, &plan, &urls, features.garden || features.trees);
    if fetch::past_laser_limit() {
        // At least this much; the tiles that came are cached for a next run.
        let got_mb = (fetch::laser_bytes() - bytes0).div_ceil(1_000_000) as u32;
        over_limit(got_mb, max_mb);
        return None;
    }
    let mut inputs = inputs?;
    if !plan.sheet.classes.unsorted.is_empty() {
        classify::crowns_not_roofs(&mut inputs);
    }
    lap("mosaic");
    let (bytes, area) = (
        fetch::laser_bytes() - bytes0,
        fetch::laser_area_m2() - area0,
    );
    if bytes > 0 && area > 0 {
        println!(
            "Laser data: downloaded {:.1} MB of laser points for {:.2} km² ({:.0} MB per km²)",
            bytes as f64 / 1e6,
            area as f64 / 1e6,
            bytes as f64 / area as f64
        );
    }
    let (thresholds, how) = classify::calibrate(&inputs, plan.sheet.intensity_defaults);
    lap("thresholds");
    println!(
        "Laser data: {:.0} points a square metre, lawn intensity from the {}",
        classify::median_density(&inputs),
        match how {
            classify::Calibration::Photo => "photo",
            classify::Calibration::Laser => "laser alone",
            classify::Calibration::Defaults => "source's defaults",
        }
    );
    let classes = classify::classify(&inputs, thresholds);
    lap("classes");
    let (w, h) = (inputs.w, inputs.h);
    drop(inputs);

    // Sample onto the world grid: cell (gx, gz) sits at the bbox's west edge plus
    // gx steps, north edge minus gz steps, as the canopy and land cover do. Rows
    // side by side: the projection is most of the work.
    use rayon::prelude::*;
    let lat_step = (bbox.min().lat() - bbox.max().lat()) / (grid_h - 1) as f64;
    let lon_step = (bbox.max().lng() - bbox.min().lng()) / (grid_w - 1) as f64;
    let mut cells = vec![[0u8; 5]; grid_w * grid_h];
    cells
        .par_chunks_mut(grid_w)
        .enumerate()
        .for_each(|(gz, row_cells)| {
            let lat = bbox.max().lat() + lat_step * gz as f64;
            for (gx, cell) in row_cells.iter_mut().enumerate() {
                let lon = bbox.min().lng() + lon_step * gx as f64;
                let (e, n) = plan.sheet.crs.forward(lat, lon);
                let (col, row) = ((e - e0).floor(), (n1 - n).floor());
                if col < 0.0 || row < 0.0 || col >= w as f64 || row >= h as f64 {
                    continue;
                }
                let o = classes[row as usize * w + col as usize];
                *cell = [o.class, o.height_q, o.color[0], o.color[1], o.color[2]];
            }
        });
    drop(classes);
    lap("sampling");
    println!("Laser data: {}", plan.sheet.attribution);
    let data = SurveyData {
        cells,
        width: grid_w,
        height: grid_h,
        features,
    };
    let n = data.counts();
    println!(
        "Laser data: {} lawn, {} paving, {} hedge, {} bush, {} bed, {} shed, {} pool, {} fence, {} tree blocks",
        n[1], n[2], n[3], n[4], n[9], n[6], n[7], n[10], n[5]
    );
    Some(data)
}

/// Where to read each tile: its address, or candidates (see [`locate`]).
type Urls = std::collections::HashMap<(i32, i32), Vec<String>>;

/// Where to read each tile not already cached: a fixed address, candidates,
/// or one lookup over the whole rectangle. Addresses cost nothing to work
/// out, so every tile has them (a cached one never uses them); the lookup is
/// made only when some tile is not cached. `None` when it cannot be made.
fn locate(client: &reqwest::blocking::Client, plan: &Plan) -> Option<Urls> {
    let root = cache_root();
    let mut urls = Urls::default();
    let (Laser::Whole { locate, .. } | Laser::Copc { locate, .. } | Laser::Ept { locate, .. }) =
        &plan.sheet.laser;
    let uncached = || {
        plan.tiles
            .iter()
            .any(|&t| !laser::is_cached(&root, &plan.sheet, t.0, t.1, &plan.want(t)))
    };
    match locate {
        Locate::Urls(f) => {
            for &(te, tn) in &plan.tiles {
                urls.insert((te, tn), f(te, tn));
            }
        }
        Locate::Lookup(f) if uncached() => {
            if let Err(e) = crate::net::ensure_online("laser scans") {
                eprintln!("Warning: {e}");
                return None;
            }
            match f(client, &plan.sheet, plan.rect) {
                // Nothing at all is more likely a gap in the index (or a
                // hiccup) than a reason to mark every tile absent for weeks.
                Ok(found) if found.is_empty() => {
                    println!("Laser data: {} has no scans here yet", plan.sheet.name);
                    return None;
                }
                Ok(found) => {
                    // A tile listed more than once is made of several files,
                    // read together as one (a line each).
                    let mut parts: std::collections::BTreeMap<(i32, i32), Vec<String>> =
                        Default::default();
                    for (t, u) in found {
                        parts.entry(t).or_default().push(u);
                    }
                    for (t, mut p) in parts {
                        p.sort();
                        p.dedup();
                        let u = p.join("\n");
                        // For later runs, which may make no lookup (see `plan_in`).
                        laser::note_whole(&root, &plan.sheet, t, laser::is_zip(&u));
                        urls.insert(t, vec![u]);
                    }
                }
                Err(e) => {
                    eprintln!("Warning: laser scan index unavailable: {e}");
                    crate::net::note_uncached("laser scans");
                    return None;
                }
            }
        }
        Locate::Lookup(_) => {}
    }
    Some(urls)
}

/// The address of tile `t`'s zipped file, which is read whole: an older campaign
/// among a cloud-optimised source's.
fn zip_url(urls: &Urls, t: (i32, i32)) -> Option<&str> {
    urls.get(&t)?
        .iter()
        .find(|u| laser::is_zip(u))
        .map(String::as_str)
}

/// The per-metre measurements over the plan's rectangle, from its tiles.
fn mosaic(
    client: &reqwest::blocking::Client,
    plan: &Plan,
    urls: &Urls,
    with_photo: bool,
) -> Option<classify::Inputs> {
    let (e0, n0, e1, n1) = plan.rect;
    let (w, h) = ((e1 - e0) as usize, (n1 - n0) as usize);
    let root = cache_root();
    laser::sweep_leftovers(&root.join(plan.sheet.dir()));
    let mut inp = classify::Inputs::new(w, h);
    inp.laser_plants = plan.sheet.laser_plants;
    inp.strict_pools = plan.sheet.strict_pools;

    // The photo downloads beside the laser tiles: they come from other servers,
    // or from one that paces every request itself. Each tile goes into the
    // mosaic as it comes and is let go.
    let classify::Inputs {
        n_all,
        n_gnd,
        n_obj,
        n_bld,
        n_veg,
        h_obj_q,
        h_bld_q,
        i_gnd_q,
        photo: photo_cells,
        ..
    } = &mut inp;
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut failed: Vec<((i32, i32), String)> = Vec::new();
    let mut any = false;
    std::thread::scope(|s| {
        let photos = s.spawn(|| {
            if with_photo {
                fill_photos(client, plan, &root, photo_cells);
            }
        });
        // Plain threads, not a Rayon pool: they wait their turn to download
        // and decode (see `laser::tile`), and a decode's own Rayon work must
        // never pick up another tile while it holds a turn.
        let (tx, rx) = std::sync::mpsc::channel();
        let (next, root) = (&next, &root);
        for _ in 0..laser::TILES_AT_ONCE.min(plan.tiles.len()) {
            let tx = tx.clone();
            s.spawn(move || loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(&t) = plan.tiles.get(i) else { break };
                let u = urls.get(&t).map_or(&[][..], Vec::as_slice);
                // A reader that trips over a file's data costs that tile only.
                let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    laser::tile(client, root, &plan.sheet, t, &plan.want(t), u)
                }))
                .unwrap_or_else(|_| Err("the file's data could not be read".to_string()));
                if tx.send((t, got)).is_err() {
                    break;
                }
            });
        }
        drop(tx);
        for ((te, tn), got) in rx {
            let t = match got {
                Ok(laser::Fetched::Tile(t)) => t,
                Ok(laser::Fetched::Absent) => continue,
                Err(e) => {
                    failed.push(((te, tn), e));
                    continue;
                }
            };
            any = true;
            let (te0, tn1) = (f64::from(te) * 1000.0, f64::from(tn + 1) * 1000.0);
            for_overlap(plan.rect, te0, tn1, |mc, tc| {
                n_all[mc] = t.n_all[tc];
                n_gnd[mc] = t.n_gnd[tc];
                n_obj[mc] = t.n_obj[tc];
                n_bld[mc] = t.n_bld[tc];
                n_veg[mc] = t.n_veg[tc];
                h_obj_q[mc] = t.h_obj[tc];
                h_bld_q[mc] = t.h_bld[tc];
                i_gnd_q[mc] = t.i_gnd[tc];
            });
        }
        // The photo only helps the laser along: whatever stopped it, the
        // laser goes on with the cells that came.
        if photos.join().is_err() {
            eprintln!(
                "Warning: aerial photos stopped part-way; the laser goes on without the rest"
            );
        }
    });
    // A tile that failed goes without laser data, as one the source does not
    // have; the others still count. A later run fetches it again.
    if !failed.is_empty() {
        crate::net::note_uncached("laser scans");
        for t in &plan.tiles {
            if let Some((_, e)) = failed.iter().find(|(f, _)| f == t) {
                eprintln!(
                    "Warning: laser scan {}_{} unavailable, its part of the area goes without: {e}",
                    t.0, t.1
                );
            }
        }
    }
    if !any {
        if failed.is_empty() {
            println!("Laser data: {} has no scans here", plan.sheet.name);
        }
        return None;
    }
    Some(inp)
}

/// The photo's cells over the plan's rectangle, tile by tile as they come (only
/// the part of each tile the rectangle covers).
fn fill_photos(
    client: &reqwest::blocking::Client,
    plan: &Plan,
    root: &std::path::Path,
    cells: &mut [photo::PhotoCell],
) {
    use rayon::prelude::*;
    let (e0, n0, e1, n1) = plan.rect;
    let cells = std::sync::Mutex::new(cells);
    // Set by the first request that finds the photo's host unreachable.
    let down = std::sync::atomic::AtomicBool::new(false);
    let each = |&(te, tn): &(i32, i32)| {
        let (te0, tn1) = (f64::from(te) * 1000.0, f64::from(tn + 1) * 1000.0);
        let c0 = (e0 - te0).max(0.0) as usize;
        let c1 = ((e1 - te0).min(1000.0)) as usize;
        let r0 = (tn1 - n1).max(0.0) as usize;
        let r1 = ((tn1 - n0).min(1000.0)) as usize;
        let Some(p) = photo::tile(client, root, &plan.sheet, (te, tn), (c0, r0, c1, r1), &down)
        else {
            return;
        };
        let mut cells = cells.lock().unwrap_or_else(|e| e.into_inner());
        for_overlap(plan.rect, te0, tn1, |mc, tc| {
            let c = p.cells[tc];
            if c.present {
                cells[mc] = c;
            }
        });
    };
    // On threads of their own: photo requests wait on the network and on a
    // paced host's turns, which on the shared pool would hold up the ground
    // generation and the laser decoding beside them.
    match rayon::ThreadPoolBuilder::new()
        .num_threads(PHOTO_THREADS)
        .build()
    {
        Ok(pool) => pool.install(|| plan.tiles.par_iter().for_each(each)),
        Err(_) => plan.tiles.par_iter().for_each(each),
    }
}

/// Threads the photo downloads run on, each tile's blocks included.
const PHOTO_THREADS: usize = 4;

/// Calls `f(mosaic_cell, tile_cell)` for every metre the rectangle and the tile
/// with north-west corner (`te0`, `tn1`) share.
fn for_overlap(rect: (f64, f64, f64, f64), te0: f64, tn1: f64, mut f: impl FnMut(usize, usize)) {
    let (e0, n0, e1, n1) = rect;
    let w = (e1 - e0) as usize;
    let x_from = e0.max(te0);
    let x_to = e1.min(te0 + 1000.0);
    let y_from = n0.max(tn1 - 1000.0);
    let y_to = n1.min(tn1);
    if x_from >= x_to || y_from >= y_to {
        return;
    }
    for row_m in ((n1 - y_to) as usize)..((n1 - y_from) as usize) {
        let northing_top = n1 - row_m as f64;
        let tr = (tn1 - northing_top) as usize;
        for col_m in ((x_from - e0) as usize)..((x_to - e0) as usize) {
            let tc = (e0 + col_m as f64 - te0) as usize;
            f(row_m * w + col_m, tr * 1000 + tc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_maps_corners_to_corners() {
        // Rectangle 714_960..715_040 E, 5_353_980..5_354_020 N against tile 714_5353.
        let rect = (714_960.0, 5_353_980.0, 715_040.0, 5_354_020.0);
        let mut seen = Vec::new();
        for_overlap(rect, 714_000.0, 5_354_000.0, |m, t| seen.push((m, t)));
        // 40 columns (714_960..715_000) by 20 rows (5_353_980..5_354_000).
        assert_eq!(seen.len(), 40 * 20);
        // Mosaic row 20 (northing 5_354_000 top) is tile row 0; column 0 is tile column 960.
        assert!(seen.contains(&(20 * 80, 960)));
        // The last: mosaic row 39, column 39 is tile row 19, column 999.
        assert!(seen.contains(&(39 * 80 + 39, 19 * 1000 + 999)));
    }

    #[test]
    fn a_tile_only_the_margin_reaches_is_left_out() {
        // Erding's rectangle: 6 m into the tile columns either side.
        let rect = (713_994.0, 5_353_059.0, 716_006.0, 5_354_941.0);
        assert!(sliver((713, 5353), rect) && sliver((716, 5354), rect));
        assert!(!sliver((714, 5353), rect) && !sliver((715, 5354), rect));
        // Past the margin the world lies on it: kept, however thin.
        let past = (713_955.0, 5_353_059.0, 716_006.0, 5_354_941.0);
        assert!(!sliver((713, 5353), past));
    }

    #[test]
    fn a_zipped_tile_only_the_margin_reaches_is_left_out_once_listed() {
        // Kriens: the margin reaches 15 m into the tile row north of the area.
        let bbox = LLBBox::new(47.0333, 8.2781, 47.0377, 8.2839).unwrap();
        let root = std::env::temp_dir().join(format!("arnis-whole-test-{}", std::process::id()));
        let tiles = || plan_in(&root, &bbox).unwrap().tiles;
        let sheet = plan_in(&root, &bbox).unwrap().sheet;
        assert!(tiles().contains(&(2663, 1210)));
        laser::note_whole(&root, &sheet, (2663, 1210), true);
        assert!(!tiles().contains(&(2663, 1210)) && tiles().contains(&(2664, 1210)));
        // Where the world lies on it, kept.
        laser::note_whole(&root, &sheet, (2663, 1209), true);
        assert!(tiles().contains(&(2663, 1209)));
        // Listed again as cloud-optimised: read in part again.
        laser::note_whole(&root, &sheet, (2663, 1210), false);
        assert!(tiles().contains(&(2663, 1210)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_limit_offered_covers_the_need() {
        assert_eq!(limit_for(480), 500);
        assert_eq!(limit_for(880), 1000);
        assert_eq!(limit_for(2001), 5000);
        assert_eq!(limit_for(5200), 6000);
    }
}
