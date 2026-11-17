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
//! blocks the area touches, results cached per tile, and a budget on what one run
//! may fetch (`--garden-detail-max-mb`).

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
}

impl Plan {
    /// The blocks of a tile the rectangle needs.
    pub fn want(&self, (te, tn): (i32, i32)) -> Vec<bool> {
        laser::blocks_for(te, tn, self.rect)
    }
}

pub fn plan(bbox: &LLBBox) -> Option<Plan> {
    plan_in(&cache_root(), bbox)
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
    let tiles = sheet.tiles(rect);
    let download_mb = match sheet.laser {
        Laser::Whole { tile_mb, .. } => {
            tiles
                .iter()
                .filter(|&&(te, tn)| {
                    !laser::is_cached(root, &sheet, te, tn, &laser::blocks_for(te, tn, rect))
                })
                .count() as u32
                * tile_mb
        }
        Laser::Copc { mb_per_km2, .. } | Laser::Ept { mb_per_km2, .. } => {
            let km2: f64 = tiles
                .iter()
                .map(|&(te, tn)| {
                    laser::missing_km2(root, &sheet, te, tn, &laser::blocks_for(te, tn, rect))
                })
                .sum();
            (km2 * f64::from(mb_per_km2)).ceil() as u32
        }
    };
    Some(Plan {
        sheet,
        tiles,
        rect,
        download_mb,
    })
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
    let plan = plan(bbox)?;
    let (e0, n0, e1, n1) = plan.rect;
    let area_km2 = (e1 - e0) * (n1 - n0) / 1e6;
    if area_km2 > MAX_AREA_KM2 {
        println!(
            "Laser data skipped: the area is {area_km2:.0} km², it is used for up to {MAX_AREA_KM2:.0}"
        );
        return None;
    }
    if plan.download_mb > max_mb {
        println!(
            "Laser data skipped: it would download about {} MB of laser scans ({} allowed, see --lidar-max-mb)",
            plan.download_mb, max_mb
        );
        return None;
    }
    println!(
        "Fetching laser data ({} laser scans, {} tile{})...",
        plan.sheet.name,
        plan.tiles.len(),
        if plan.tiles.len() == 1 { "" } else { "s" }
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .user_agent(concat!("arnis/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let (bytes0, area0) = (fetch::laser_bytes(), fetch::laser_area_m2());
    // Heights read only the laser's roofs; lawns and trees need the photo too.
    let mut inputs = mosaic(&client, &plan, features.garden || features.trees)?;
    if !plan.sheet.classes.unsorted.is_empty() {
        classify::crowns_not_roofs(&mut inputs);
    }
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
    if let Some(path) = std::env::var_os("ARNIS_LASER_DUMP") {
        dump(&inputs, &classes, thresholds, std::path::Path::new(&path));
    }
    let (w, h) = (inputs.w, inputs.h);
    drop(inputs);

    // Sample onto the world grid: cell (gx, gz) sits at the bbox's west edge plus
    // gx steps, north edge minus gz steps, as the canopy and land cover do.
    let lat_step = (bbox.min().lat() - bbox.max().lat()) / (grid_h - 1) as f64;
    let lon_step = (bbox.max().lng() - bbox.min().lng()) / (grid_w - 1) as f64;
    let mut cells = vec![[0u8; 5]; grid_w * grid_h];
    for gz in 0..grid_h {
        let lat = bbox.max().lat() + lat_step * gz as f64;
        for gx in 0..grid_w {
            let lon = bbox.min().lng() + lon_step * gx as f64;
            let (e, n) = plan.sheet.crs.forward(lat, lon);
            let (col, row) = ((e - e0).floor(), (n1 - n).floor());
            if col < 0.0 || row < 0.0 || col >= w as f64 || row >= h as f64 {
                continue;
            }
            let o = classes[row as usize * w + col as usize];
            cells[gz * grid_w + gx] = [o.class, o.height_q, o.color[0], o.color[1], o.color[2]];
        }
    }
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

/// The per-metre measurements over the plan's rectangle, from its tiles.
fn mosaic(
    client: &reqwest::blocking::Client,
    plan: &Plan,
    with_photo: bool,
) -> Option<classify::Inputs> {
    use rayon::prelude::*;
    let (e0, n0, e1, n1) = plan.rect;
    let (w, h) = ((e1 - e0) as usize, (n1 - n0) as usize);
    let root = cache_root();
    let mut inp = classify::Inputs::new(w, h);
    inp.laser_plants = plan.sheet.laser_plants;
    inp.strict_pools = plan.sheet.strict_pools;

    // Where to read each tile not already cached: a fixed address, candidates,
    // or one lookup over the whole rectangle.
    let uncached: Vec<(i32, i32)> = plan
        .tiles
        .iter()
        .copied()
        .filter(|&t| laser::cached(&root, &plan.sheet, t.0, t.1, &plan.want(t)).is_none())
        .collect();
    let mut urls: std::collections::HashMap<(i32, i32), Vec<String>> = Default::default();
    let (Laser::Whole { locate, .. } | Laser::Copc { locate, .. } | Laser::Ept { locate, .. }) =
        &plan.sheet.laser;
    match locate {
        Locate::Urls(f) => {
            for &(te, tn) in &uncached {
                urls.insert((te, tn), f(te, tn));
            }
        }
        Locate::Lookup(f) if !uncached.is_empty() => {
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
                        urls.insert(t, vec![p.join("\n")]);
                    }
                }
                Err(e) => {
                    eprintln!("Warning: laser scan index unavailable: {e}");
                    return None;
                }
            }
        }
        Locate::Lookup(_) => {}
    }

    // Laser tiles two at a time: each takes a few hundred MB while it is decoded.
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .ok()?;
    let tiles: Vec<((i32, i32), Result<laser::Fetched, String>)> = pool.install(|| {
        plan.tiles
            .par_iter()
            .map(|&t| {
                let none = Vec::new();
                let u = urls.get(&t).unwrap_or(&none);
                (
                    t,
                    laser::tile(client, &root, &plan.sheet, t, &plan.want(t), u),
                )
            })
            .collect()
    });
    let mut any = false;
    for ((te, tn), t) in tiles {
        let t = match t {
            Ok(laser::Fetched::Tile(t)) => t,
            Ok(laser::Fetched::Absent) => continue,
            Err(e) => {
                eprintln!("Warning: laser scan {te}_{tn} unavailable: {e}");
                return None;
            }
        };
        any = true;
        let (te0, tn1) = (f64::from(te) * 1000.0, f64::from(tn + 1) * 1000.0);
        for_overlap(plan.rect, te0, tn1, |mc, tc| {
            inp.n_all[mc] = t.n_all[tc];
            inp.n_gnd[mc] = t.n_gnd[tc];
            inp.n_obj[mc] = t.n_obj[tc];
            inp.n_bld[mc] = t.n_bld[tc];
            inp.n_veg[mc] = t.n_veg[tc];
            inp.h_obj_q[mc] = t.h_obj[tc];
            inp.h_bld_q[mc] = t.h_bld[tc];
            inp.i_gnd_q[mc] = t.i_gnd[tc];
        });
    }
    if !any {
        return None;
    }

    // Photo: only the cells of each tile the rectangle covers.
    if !with_photo || std::env::var_os("ARNIS_LASER_NO_PHOTO").is_some() {
        return Some(inp);
    }
    let photos: Vec<((i32, i32), Option<photo::PhotoTile>)> = plan
        .tiles
        .par_iter()
        .map(|&(te, tn)| {
            let (te0, tn1) = (f64::from(te) * 1000.0, f64::from(tn + 1) * 1000.0);
            let c0 = (e0 - te0).max(0.0) as usize;
            let c1 = ((e1 - te0).min(1000.0)) as usize;
            let r0 = (tn1 - n1).max(0.0) as usize;
            let r1 = ((tn1 - n0).min(1000.0)) as usize;
            (
                (te, tn),
                photo::tile(client, &root, &plan.sheet, te, tn, (c0, r0, c1, r1)),
            )
        })
        .collect();
    for ((te, tn), p) in photos {
        let Some(p) = p else { continue };
        let (te0, tn1) = (f64::from(te) * 1000.0, f64::from(tn + 1) * 1000.0);
        for_overlap(plan.rect, te0, tn1, |mc, tc| {
            let c = p.cells[tc];
            if c.present {
                inp.photo[mc] = c;
            }
        });
    }
    Some(inp)
}

/// Debugging aid (`ARNIS_LASER_DUMP=out.png`): the classes on the metre grid,
/// beside the ground intensity against the thresholds and the photo.
fn dump(
    inp: &classify::Inputs,
    out: &[classify::Out],
    (lo, hi): (f32, f32),
    path: &std::path::Path,
) {
    let (w, h) = (inp.w as u32, inp.h as u32);
    let mut img = image::RgbImage::new(w * 3, h);
    for (c, o) in out.iter().enumerate() {
        let (x, y) = ((c % inp.w) as u32, (c / inp.w) as u32);
        let class = match o.class {
            classify::LAWN => [110, 190, 80],
            classify::PAVED => [170, 170, 170],
            classify::HEDGE => [20, 110, 20],
            classify::BUSH => [60, 150, 60],
            classify::TREE => [0, 70, 0],
            classify::SHED => [230, 60, 200],
            classify::POOL => [40, 120, 250],
            classify::BUILDING => [120, 40, 30],
            classify::BED => [200, 160, 60],
            classify::FENCE => [250, 250, 0],
            _ => [0, 0, 0],
        };
        img.put_pixel(x, y, image::Rgb(class));
        let i = inp.i_gnd(c);
        let band = if inp.n_gnd[c] == 0 {
            [0, 0, 0]
        } else if i >= hi {
            [90, 200, 90]
        } else if i < lo {
            [110, 110, 110]
        } else {
            [220, 200, 120]
        };
        img.put_pixel(w + x, y, image::Rgb(band));
        img.put_pixel(2 * w + x, y, image::Rgb(inp.rgb(c)));
    }
    if let Err(e) = img.save(path) {
        eprintln!("Warning: laser dump not written: {e}");
    }
    eprintln!("laser thresholds: {lo:.0} {hi:.0}");
}

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
}
