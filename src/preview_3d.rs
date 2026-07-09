//! 3D terrain preview backend: fetches a reduced-resolution elevation and
//! land-cover grid through the existing (disk-cached) provider pipeline and
//! packs both into one binary payload for the GUI's MapLibre terrain view.

use crate::coordinate_system::geographic::LLBBox;
use crate::coordinate_system::transformation::geo_distance;
use crate::elevation::{compute_grid_dims, fetch_elevation_data};
use crate::land_cover;
use crate::overture;
use crate::progress;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Longest preview grid side; caps fetch/postprocess RAM and the IPC payload (~3 MB at 1024²).
const PREVIEW_MAX_DIM: usize = 1024;

/// Payload magic ("Arnis Preview V1"); bump on layout changes.
const MAGIC: &[u8; 4] = b"APV1";

/// Max bbox area for the buildings overlay (~10 km²); mirrored in preview3d.js.
const BUILDINGS_MAX_AREA_M2: f64 = 10_000_000.0;

/// Feature cap keeping the GeoJSON IPC payload bounded for dense cities.
const BUILDINGS_MAX_FEATURES: usize = 30_000;

const FALLBACK_FLOOR_HEIGHT_M: f64 = 3.2;
const DEFAULT_BUILDING_HEIGHT_M: f64 = 8.0;

/// Error string the frontend treats as a silent skip (newer request queued).
pub const SUPERSEDED: &str = "superseded";

// Auto-triggered previews can pile up while one fetch runs; each request
// takes a ticket, fetches are serialized by the lock, and a queued request
// whose ticket is stale by the time it runs bails out immediately.
static TERRAIN_EPOCH: AtomicU64 = AtomicU64::new(0);
static TERRAIN_LOCK: Mutex<()> = Mutex::new(());
static BUILDINGS_EPOCH: AtomicU64 = AtomicU64::new(0);
static BUILDINGS_LOCK: Mutex<()> = Mutex::new(());

// Mutes generation-pipeline progress emits (9-18%) while the preview reuses it.
struct ProgressMute;

impl ProgressMute {
    fn new() -> Self {
        progress::set_progress_suppressed(true);
        Self
    }
}

impl Drop for ProgressMute {
    fn drop(&mut self) {
        progress::set_progress_suppressed(false);
    }
}

/// Computes the preview scale and grid dims for a bbox (grid capped at
/// PREVIEW_MAX_DIM per side); shared by the terrain and land-cover payloads
/// so their grids always align.
fn preview_grid_dims(bbox: &LLBBox) -> (f64, usize, usize) {
    let (world_w, world_h, _, _) = compute_grid_dims(bbox, 1.0);
    let preview_scale = (PREVIEW_MAX_DIM as f64 / world_w.max(world_h) as f64).min(1.0);
    let (_, _, grid_w, grid_h) = compute_grid_dims(bbox, preview_scale);
    (preview_scale, grid_w, grid_h)
}

/// Builds the binary preview payload for the given bbox.
///
/// Layout (little-endian): magic[4], grid_w u32, grid_h u32, min_lat f64,
/// min_lng f64, max_lat f64, max_lng f64, min_elev_m f32, max_elev_m f32,
/// flags u32 (reserved, 0), reserved[8]; then grid_w*grid_h u16 heights
/// (row 0 = north, quantized over [min_elev, max_elev]). Land cover ships
/// separately via `build_landcover_grid`, fetched only when the user
/// enables the overlay.
pub fn build_preview_payload(bbox_text: &str, aws_only: bool) -> Result<Vec<u8>, String> {
    let my_epoch = TERRAIN_EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    let _guard = TERRAIN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if TERRAIN_EPOCH.load(Ordering::SeqCst) != my_epoch {
        return Err(SUPERSEDED.to_string());
    }
    let bbox = LLBBox::from_str(bbox_text).map_err(|e| format!("Invalid bounding box: {e}"))?;

    // Shrink the scale until the grid fits the preview cap; the fetch goes
    // through the normal providers, so it reads/warms the same tile caches
    // as a real generation run and retains nothing afterwards.
    let (preview_scale, _, _) = preview_grid_dims(&bbox);

    let _mute = ProgressMute::new();

    // Previews skip the regional providers: Mapterhorn is global,
    // CDN-fast, and adapts its zoom to the capped preview grid, so
    // preview clicks never put load on rate-limited regional services.
    let source_mode = if aws_only {
        crate::elevation::SourceMode::AwsOnly
    } else {
        crate::elevation::SourceMode::GlobalOnly
    };

    // ground_level 0 keeps the meter->Y affine trivially invertible below.
    let elevation =
        fetch_elevation_data(&bbox, preview_scale, 0, false, 0, None, source_mode, false)
            .map_err(|e| format!("Elevation fetch failed: {e}"))?;

    let gw = elevation.width;
    let gh = elevation.height;
    let bpm = elevation.blocks_per_meter;
    let min_m = elevation.min_height_m;

    // Invert scale_to_minecraft's affine: y = (h_m - min_m) * bpm (ground_level 0).
    let to_meters = |y: f32| -> f64 {
        if bpm > 0.0 {
            min_m + f64::from(y) / bpm
        } else {
            min_m
        }
    };

    let mut max_m = min_m;
    if bpm > 0.0 {
        for row in &elevation.heights {
            for &y in row {
                if y.is_finite() {
                    max_m = max_m.max(to_meters(y));
                }
            }
        }
    }
    let range = (max_m - min_m).max(0.0);
    let qscale = if range > 0.0 { 65535.0 / range } else { 0.0 };

    let mut buf = Vec::with_capacity(64 + gw * gh * 2);
    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&(gw as u32).to_le_bytes());
    buf.extend_from_slice(&(gh as u32).to_le_bytes());
    for v in [
        bbox.min().lat(),
        bbox.min().lng(),
        bbox.max().lat(),
        bbox.max().lng(),
    ] {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    buf.extend_from_slice(&(min_m as f32).to_le_bytes());
    buf.extend_from_slice(&(max_m as f32).to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&[0u8; 8]);

    for row in &elevation.heights {
        for &y in row {
            let m = if y.is_finite() { to_meters(y) } else { min_m };
            let q = ((m - min_m) * qscale).round().clamp(0.0, 65535.0) as u16;
            buf.extend_from_slice(&q.to_le_bytes());
        }
    }
    Ok(buf)
}

/// ESA land-cover grid for the preview, fetched lazily when the user enables
/// the overlay toggle. Layout (little-endian): magic "APL1", grid_w u32,
/// grid_h u32, then grid_w*grid_h u8 class codes (row 0 = north). Grid dims
/// match the terrain payload for the same bbox.
pub fn build_landcover_grid(bbox_text: &str) -> Result<Vec<u8>, String> {
    let bbox = LLBBox::from_str(bbox_text).map_err(|e| format!("Invalid bounding box: {e}"))?;
    let (_, grid_w, grid_h) = preview_grid_dims(&bbox);

    let _mute = ProgressMute::new();
    let lc = land_cover::fetch_land_cover_data(&bbox, grid_w, grid_h)
        .ok_or("Land cover data unavailable".to_string())?;
    if lc.width != grid_w || lc.height != grid_h {
        return Err("Land cover grid dimension mismatch".to_string());
    }

    let mut buf = Vec::with_capacity(12 + grid_w * grid_h);
    buf.extend_from_slice(b"APL1");
    buf.extend_from_slice(&(grid_w as u32).to_le_bytes());
    buf.extend_from_slice(&(grid_h as u32).to_le_bytes());
    for row in &lc.grid {
        buf.extend_from_slice(row);
    }
    Ok(buf)
}

/// Builds a GeoJSON FeatureCollection of Overture building footprints for the
/// preview's fill-extrusion layer. Includes OSM-sourced buildings (unlike the
/// generation path) for full coverage. Errors are swallowed by the frontend.
pub fn build_buildings_geojson(bbox_text: &str) -> Result<String, String> {
    let my_epoch = BUILDINGS_EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    let _guard = BUILDINGS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if BUILDINGS_EPOCH.load(Ordering::SeqCst) != my_epoch {
        return Err(SUPERSEDED.to_string());
    }
    let bbox = LLBBox::from_str(bbox_text).map_err(|e| format!("Invalid bounding box: {e}"))?;
    let (height_m, width_m) = geo_distance(bbox.min(), bbox.max());
    if height_m * width_m > BUILDINGS_MAX_AREA_M2 {
        return Err("area too large for building preview".to_string());
    }

    let client = overture::overture_client().map_err(|e| e.to_string())?;
    let buildings =
        overture::collect_overture_buildings(&client, &bbox, true, BUILDINGS_MAX_FEATURES, false)
            .map_err(|e| e.to_string())?;

    let features: Vec<serde_json::Value> = buildings
        .iter()
        .filter_map(|b| {
            if b.exterior_ring.len() < 3 {
                return None;
            }
            let mut ring: Vec<[f64; 2]> = b
                .exterior_ring
                .iter()
                .map(|&(lng, lat)| [round6(lng), round6(lat)])
                .collect();
            if ring.first() != ring.last() {
                let first = ring[0];
                ring.push(first);
            }
            if ring.len() < 4 {
                return None;
            }
            let base = b.min_height.unwrap_or(0.0).clamp(0.0, 400.0);
            let height = b
                .height
                .or_else(|| {
                    b.num_floors
                        .map(|f| f64::from(f) * FALLBACK_FLOOR_HEIGHT_M + 1.0)
                })
                .unwrap_or(DEFAULT_BUILDING_HEIGHT_M)
                .clamp(2.0, 500.0)
                .max(base + 2.0);
            Some(json!({
                "type": "Feature",
                "geometry": { "type": "Polygon", "coordinates": [ring] },
                "properties": { "h": round1(height), "b": round1(base) },
            }))
        })
        .collect();

    serde_json::to_string(&json!({ "type": "FeatureCollection", "features": features }))
        .map_err(|e| e.to_string())
}

fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    // Live-network smoke test: cargo test preview_payload_smoke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn preview_payload_smoke() {
        let payload = super::build_preview_payload("48.130 11.545 48.145 11.565", true).unwrap();
        // Set PREVIEW_DUMP=<path> to save the payload for frontend debugging.
        if let Ok(path) = std::env::var("PREVIEW_DUMP") {
            std::fs::write(path, &payload).unwrap();
        }
        assert_eq!(&payload[0..4], b"APV1");
        let gw = u32::from_le_bytes(payload[4..8].try_into().unwrap()) as usize;
        let gh = u32::from_le_bytes(payload[8..12].try_into().unwrap()) as usize;
        assert!((2..=super::PREVIEW_MAX_DIM + 1).contains(&gw));
        assert!((2..=super::PREVIEW_MAX_DIM + 1).contains(&gh));
        assert_eq!(u32::from_le_bytes(payload[52..56].try_into().unwrap()), 0);
        assert_eq!(payload.len(), 64 + gw * gh * 2);
        let min_e = f32::from_le_bytes(payload[44..48].try_into().unwrap());
        let max_e = f32::from_le_bytes(payload[48..52].try_into().unwrap());
        println!("grid {gw}x{gh}, elev {min_e:.0}-{max_e:.0} m");
        // Munich sits around 500-600 m
        assert!(
            min_e > 300.0 && max_e < 1200.0 && max_e >= min_e,
            "implausible Munich elevation {min_e}-{max_e}"
        );
    }

    // Live-network smoke test: cargo test preview_landcover_smoke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn preview_landcover_smoke() {
        let payload = super::build_landcover_grid("48.130 11.545 48.145 11.565").unwrap();
        assert_eq!(&payload[0..4], b"APL1");
        let gw = u32::from_le_bytes(payload[4..8].try_into().unwrap()) as usize;
        let gh = u32::from_le_bytes(payload[8..12].try_into().unwrap()) as usize;
        assert_eq!(payload.len(), 12 + gw * gh);
        let classified = payload[12..].iter().filter(|&&c| c != 0).count();
        println!("land cover grid {gw}x{gh}, {classified} classified cells");
        // Central Munich should be almost fully classified (mostly built-up)
        assert!(classified > gw * gh / 2);
    }

    // Live-network smoke test: cargo test preview_buildings_smoke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn preview_buildings_smoke() {
        let geojson = super::build_buildings_geojson("48.130 11.545 48.145 11.565").unwrap();
        // Set PREVIEW_DUMP=<path> to save the GeoJSON for frontend debugging.
        if let Ok(path) = std::env::var("PREVIEW_DUMP") {
            std::fs::write(path, &geojson).unwrap();
        }
        let parsed: serde_json::Value = serde_json::from_str(&geojson).unwrap();
        let features = parsed["features"].as_array().unwrap();
        println!("{} buildings, {} KB", features.len(), geojson.len() / 1024);
        // Central Munich should have thousands of buildings
        assert!(features.len() > 100, "only {} buildings", features.len());
        let f = &features[0];
        assert_eq!(f["geometry"]["type"], "Polygon");
        assert!(f["properties"]["h"].as_f64().unwrap() >= 2.0);
    }
}
