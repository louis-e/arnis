//! Experimental `--climate-mode per-position`: the Köppen climate read per position
//! instead of once per area, so a large world crosses climate zones. `--climate-map`
//! renders that layout for `--bbox` and exits.

use crate::args::Args;
use crate::climate::Climate;
use crate::coordinate_system::{cartesian::XZPoint, geographic::LLBBox};
use crate::ground::{Geo, GroundFrame};
use image::{Rgb, RgbImage};
use rayon::prelude::*;

#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClimateMode {
    /// One climate per area, read at its centre.
    #[default]
    Origin,
    /// Climate and biome latitude read per position (experimental).
    PerPosition,
}

/// Smallest and largest grid cell, in blocks.
const MIN_CELL: i32 = 4;
const MAX_CELL: i32 = 32;

/// Climate over one run's ground, a cell per `cell` x `cell` blocks, aligned to world blocks
/// so pieces and areas agree.
pub struct ClimateField {
    geo: Geo,
    cell: i32,
    origin: (i32, i32),
    w: usize,
    h: usize,
    climates: Vec<Climate>,
}

impl ClimateField {
    pub fn new(frame: &GroundFrame, bbox: &LLBBox, (world_w, world_h): (usize, usize)) -> Self {
        let (geo, align) = frame.geo(bbox, (world_w, world_h));
        // Taken from the projection where there is one, so every piece picks the same cell.
        let lon_per_block = match frame.mercator {
            Some(proj) => proj.lon_for_x(1.0) - proj.lon_for_x(0.0),
            None => geo(1.0, 0.0).1 - geo(0.0, 0.0).1,
        };
        // About the warp's finest step, a 64th of a Köppen cell, as a power of two.
        let step = crate::climate::koppen_cell_degrees() / lon_per_block.abs() / 64.0;
        let cell = 1 << (31 - (step as i32).clamp(MIN_CELL, MAX_CELL).leading_zeros());
        let origin = (-align.0.rem_euclid(cell), -align.1.rem_euclid(cell));
        let span = |len: usize, from: i32| ((len as i32 - from + cell - 1) / cell).max(1) as usize;
        let (w, h) = (span(world_w, origin.0), span(world_h, origin.1));
        let half = f64::from(cell) / 2.0;
        let climates = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let gx = f64::from(origin.0 + (i % w) as i32 * cell) + half;
                let gz = f64::from(origin.1 + (i / w) as i32 * cell) + half;
                let (lat, lon) = geo(gx, gz);
                Climate::classify_warped(lat, lon)
            })
            .collect();
        Self {
            geo,
            cell,
            origin,
            w,
            h,
            climates,
        }
    }

    pub fn climate(&self, coord: XZPoint) -> Climate {
        let k = |v: i32, from: i32, len: usize| {
            (v - from).div_euclid(self.cell).clamp(0, len as i32 - 1) as usize
        };
        self.climates
            [k(coord.z, self.origin.1, self.h) * self.w + k(coord.x, self.origin.0, self.w)]
    }

    pub fn lat(&self, coord: XZPoint) -> f64 {
        (self.geo)(f64::from(coord.x) + 0.5, f64::from(coord.z) + 0.5).0
    }
}

const MAP_SIDE: u32 = 512;

fn style(c: Climate) -> ([u8; 3], &'static str) {
    match c {
        Climate::Temperate => ([120, 180, 90], "temperate"),
        Climate::TropicalSavanna => ([170, 190, 80], "tropical_savanna"),
        Climate::HotDesert => ([237, 201, 120], "hot_desert"),
        Climate::HotSteppe => ([214, 178, 110], "hot_steppe"),
        Climate::ColdDesert => ([200, 190, 160], "cold_desert"),
        Climate::ColdSteppe => ([190, 185, 140], "cold_steppe"),
        Climate::DryContinental => ([200, 140, 80], "dry_continental"),
        Climate::Boreal => ([80, 150, 120], "boreal"),
        Climate::Tundra => ([170, 200, 205], "tundra"),
        Climate::IceCap => ([240, 248, 255], "ice_cap"),
    }
}

/// `--climate-map <PREFIX>`: `<PREFIX>.png`, north up, at most 512 pixels a side
/// with the ground's aspect, and a `CLIMATEMAP {json}` line with each climate's
/// share in percent.
pub fn render(args: &Args) -> Result<(), String> {
    let prefix = args
        .climate_map
        .as_ref()
        .ok_or("--climate-map is not set")?;
    let bbox = args.bbox.as_ref().ok_or("--climate-map needs --bbox")?;
    let (s, n) = (bbox.min().lat(), bbox.max().lat());
    let (w_lon, e_lon) = (bbox.min().lng(), bbox.max().lng());
    let ground_w = (e_lon - w_lon) * ((s + n) / 2.0).to_radians().cos();
    let ground_h = n - s;
    let side =
        |len: f64| ((f64::from(MAP_SIDE) * len / ground_w.max(ground_h)).round() as u32).max(1);
    let (w, h) = (side(ground_w), side(ground_h));
    let mut img = RgbImage::new(w, h);
    let mut counts = std::collections::BTreeMap::<&str, u32>::new();
    for pz in 0..h {
        let lat = n - (f64::from(pz) + 0.5) / f64::from(h) * ground_h;
        for px in 0..w {
            let lon = w_lon + (f64::from(px) + 0.5) / f64::from(w) * (e_lon - w_lon);
            let (rgb, name) = style(Climate::classify_warped(lat, lon));
            *counts.entry(name).or_default() += 1;
            img.put_pixel(px, pz, Rgb(rgb));
        }
    }
    let path = format!("{}.png", prefix.display());
    img.save(&path).map_err(|e| format!("write {path}: {e}"))?;
    let total = f64::from(w * h);
    let shares: serde_json::Map<String, serde_json::Value> = counts
        .into_iter()
        .map(|(k, c)| {
            let pct = (f64::from(c) * 1000.0 / total).round() / 10.0;
            (k.to_string(), serde_json::json!(pct))
        })
        .collect();
    let out = serde_json::json!({ "shares": shares, "_file": path, "_size": [w, h] });
    println!("CLIMATEMAP {out}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warped_lookup_agrees_away_from_borders() {
        // Deep inside one zone the warp cannot reach a neighbour.
        assert_eq!(Climate::classify_warped(23.0, 13.0), Climate::HotDesert); // Sahara
        assert_eq!(Climate::classify_warped(48.2, 8.2), Climate::Temperate); // Black Forest
        assert_eq!(Climate::classify_warped(72.0, -40.0), Climate::IceCap); // Greenland
    }

    #[test]
    fn origin_mode_is_v340() {
        use clap::Parser;
        let args =
            crate::args::Args::parse_from(["arnis", "--output-dir", ".", "--bbox", "1,2,3,4"]);
        assert_eq!(args.climate_mode, ClimateMode::Origin);
        assert!(args.climate_map.is_none());
        // No field is built, so every block reads the origin's climate.
        let mut ground = crate::ground::Ground::new_flat(0);
        let bbox = LLBBox::new(22.0, 10.0, 37.0, 11.0).unwrap();
        ground.apply_climate_mode(args.climate_mode, &GroundFrame::local(), &bbox);
        assert_eq!(ground.local_climate(XZPoint::new(3, 3)), None);
        assert_eq!(ground.climate_at(XZPoint::new(3, 3)), ground.climate());
    }

    #[test]
    fn per_position_mode_crosses_zones_and_pieces_agree() {
        // Sahara up into the Mediterranean, at 1:500.
        let proj = crate::projection::WebMercatorProjection::new(22.0, 10.0, 0.002);
        let mut frame = GroundFrame::local();
        frame.mercator = Some(proj);
        let dims = |b: &LLBBox| {
            let w = proj.x_for_lon(b.max().lng()) - proj.x_for_lon(b.min().lng());
            let h = proj.z_for_lat(b.min().lat()) - proj.z_for_lat(b.max().lat());
            (w.ceil() as usize, h.ceil() as usize)
        };
        let bbox = LLBBox::new(22.0, 10.0, 37.0, 11.0).unwrap();
        let whole = ClimateField::new(&frame, &bbox, dims(&bbox));
        assert_eq!(whole.cell, MIN_CELL);
        let z_end = dims(&bbox).1 as i32;
        let mut seen: Vec<_> = (0..64)
            .map(|i| whole.climate(XZPoint::new(10, z_end * i / 64)))
            .collect();
        seen.dedup();
        assert!(seen.len() >= 2, "{seen:?}");
        // A piece starting further south reads the same block the same way.
        let piece_bbox = LLBBox::new(22.0, 10.3, 30.0, 11.0).unwrap();
        let piece = ClimateField::new(&frame, &piece_bbox, dims(&piece_bbox));
        let snap = |v| crate::projection::snap_edge(v, false);
        let dz = snap(proj.z_for_lat(30.0)) - snap(proj.z_for_lat(37.0));
        let dx = snap(proj.x_for_lon(10.3)) - snap(proj.x_for_lon(10.0));
        let (piece_w, piece_h) = dims(&piece_bbox);
        for z in (0..piece_h as i32 - 3).step_by(7) {
            for x in (0..piece_w as i32 - 3).step_by(5) {
                let (a, b) = (XZPoint::new(x + dx, z + dz), XZPoint::new(x, z));
                assert_eq!(whole.climate(a), piece.climate(b));
                assert_eq!(whole.lat(a), piece.lat(b));
            }
        }
    }
}
