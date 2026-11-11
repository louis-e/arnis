//! Experimental `--climate-mode per-position`: the Köppen climate read at every
//! block instead of once at the origin, so a large world crosses climate zones.
//! `--climate-map` renders that layout for `--bbox` and exits.
//!
//! A sample is a pure function of the block's place in the projection, so One
//! World pieces and areas agree on it. Borders are warped like the ecoregion
//! borders, so they wander instead of tracing the 0.1 degree raster.

use crate::args::Args;
use crate::climate::Climate;
use crate::coordinate_system::{cartesian::XZPoint, geographic::LLBBox};
use crate::ground::GroundFrame;
use image::{Rgb, RgbImage};

#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClimateMode {
    /// One climate for the whole world, read at its origin (bbox centre, or
    /// the One World origin).
    #[default]
    Origin,
    /// Climate and biome latitude read per block (experimental).
    PerPosition,
}

type Geo = Box<dyn Fn(f64, f64) -> (f64, f64) + Send + Sync>;

/// Latitude and longitude under a ground coordinate, laid out as `EcoMap` is.
pub struct ClimateField {
    geo: Geo,
}

impl ClimateField {
    pub fn new(frame: &GroundFrame, bbox: &LLBBox, (world_w, world_h): (usize, usize)) -> Self {
        let geo: Geo = match frame.mercator {
            Some(proj) => {
                // Projection coordinates are the world's own, so pieces agree.
                let x0 = crate::projection::snap_edge(proj.x_for_lon(bbox.min().lng()), false);
                let z0 = crate::projection::snap_edge(proj.z_for_lat(bbox.max().lat()), false);
                Box::new(move |gx, gz| {
                    (
                        proj.lat_for_z(f64::from(z0) + gz),
                        proj.lon_for_x(f64::from(x0) + gx),
                    )
                })
            }
            None => {
                let (top, left) = (bbox.max().lat(), bbox.min().lng());
                let (dlat, dlon) = (top - bbox.min().lat(), bbox.max().lng() - left);
                let (w, h) = (world_w.max(1) as f64, world_h.max(1) as f64);
                Box::new(move |gx, gz| (top - gz / h * dlat, left + gx / w * dlon))
            }
        };
        Self { geo }
    }

    fn lat_lon(&self, coord: XZPoint) -> (f64, f64) {
        (self.geo)(f64::from(coord.x) + 0.5, f64::from(coord.z) + 0.5)
    }

    pub fn climate(&self, coord: XZPoint) -> Climate {
        let (lat, lon) = self.lat_lon(coord);
        Climate::classify_warped(lat, lon)
    }

    pub fn lat(&self, coord: XZPoint) -> f64 {
        self.lat_lon(coord).0
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
    fn per_block_mode_crosses_zones() {
        // Sahara up into the Mediterranean, at 1:20.
        let bbox = LLBBox::new(22.0, 10.0, 37.0, 11.0).unwrap();
        let mut frame = GroundFrame::local();
        let proj = crate::projection::WebMercatorProjection::new(22.0, 10.0, 0.05);
        frame.mercator = Some(proj);
        let whole = ClimateField::new(&frame, &bbox, (0, 0));
        let z_end = proj.z_for_lat(22.0) - proj.z_for_lat(37.0);
        let mut seen: Vec<_> = (0..64)
            .map(|i| whole.climate(XZPoint::new(10, (z_end * f64::from(i) / 64.0) as i32)))
            .collect();
        seen.dedup();
        assert!(seen.len() >= 2, "{seen:?}");
        // A piece starting further south reads the same block the same way.
        let piece_bbox = LLBBox::new(22.0, 10.0, 30.0, 11.0).unwrap();
        let piece = ClimateField::new(&frame, &piece_bbox, (0, 0));
        let snap = |lat| crate::projection::snap_edge(proj.z_for_lat(lat), false);
        let dz = snap(30.0) - snap(37.0);
        for z in (0..z_end as i32 - dz).step_by(97) {
            let (a, b) = (XZPoint::new(5, z + dz), XZPoint::new(5, z));
            assert_eq!(whole.climate(a), piece.climate(b));
            assert_eq!(whole.lat(a), piece.lat(b));
        }
    }
}
