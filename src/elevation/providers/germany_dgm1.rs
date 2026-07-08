//! Germany DGM1 via hoehendaten.de — 1m LiDAR terrain models of all 16
//! German states, served as 1x1 km UTM GeoTIFF tiles (EPSG:25832/25833).
//!
//! API quirks (verified live): `Accept: application/json` is mandatory,
//! responses are always gzip-compressed, and the UTM zone follows the
//! source state's publication CRS rather than the nominal 6-degree zone
//! (Bavaria is zone 32 even east of 12E). The zone is therefore resolved
//! through a `/v1/point` probe instead of being derived from longitude.
//! Rate limit: 20 tiles/minute, enforced here via `accepts()`.

use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::cache::get_cache_dir;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};
use crate::elevation::providers::aws_terrain::AwsTerrain;
use base64::Engine;
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Read;

const API_BASE: &str = "https://api.hoehendaten.de:14444";
const TILE_METERS: f64 = 1000.0;
const TILE_PIXELS: usize = 1000;
const NODATA: f32 = -9999.0;
// The service allows 20 tile requests per minute; larger areas fall back to AWS.
const MAX_TILES: usize = 20;

// accepts() and fetch_dgm() probe the same bbox center; cache the zone so the
// selection probe isn't repeated as a second network round-trip.
type ZoneCacheEntry = Option<((i64, i64), u8)>;
static ZONE_CACHE: once_cell::sync::Lazy<std::sync::Mutex<ZoneCacheEntry>> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(None));

fn zone_cache_key(lng: f64, lat: f64) -> (i64, i64) {
    ((lng * 1e6).round() as i64, (lat * 1e6).round() as i64)
}

fn resolve_zone_cached(
    client: &reqwest::blocking::Client,
    lng: f64,
    lat: f64,
) -> Result<u8, Box<dyn std::error::Error>> {
    let key = zone_cache_key(lng, lat);
    if let Some((k, z)) = *ZONE_CACHE.lock().unwrap() {
        if k == key {
            return Ok(z);
        }
    }
    let zone = resolve_zone(client, lng, lat)?;
    *ZONE_CACHE.lock().unwrap() = Some((key, zone));
    Ok(zone)
}

pub struct GermanyDgm1;

impl ElevationProvider for GermanyDgm1 {
    fn name(&self) -> &'static str {
        "germany_dgm1"
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        Some(vec![LLBBox::new(47.2, 5.8, 55.1, 15.1).unwrap()])
    }

    fn native_resolution_m(&self) -> f64 {
        1.0
    }

    fn accepts(&self, bbox: &LLBBox) -> bool {
        let tiles = estimate_tile_count(bbox);
        if tiles > MAX_TILES {
            println!(
                "Germany DGM1: area needs ~{tiles} tiles (limit {MAX_TILES}/min), using next provider"
            );
            return false;
        }
        // The coverage rectangle spills into neighbouring countries; a cheap
        // point probe confirms the service actually has data here.
        let center_lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
        let center_lng = (bbox.min().lng() + bbox.max().lng()) / 2.0;
        match build_client().and_then(|c| resolve_zone_cached(&c, center_lng, center_lat)) {
            Ok(_) => true,
            Err(_) => {
                println!("Germany DGM1: no coverage at bbox center, using next provider");
                false
            }
        }
    }

    fn fetch_raw(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
        // Total failure propagates; fetch_elevation_data owns the AWS fallback.
        self.fetch_dgm(bbox, grid_width, grid_height)
    }
}

impl GermanyDgm1 {
    fn fetch_dgm(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
        let client = build_client()?;

        // The publication zone can't be derived from longitude; ask the service.
        let center_lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
        let center_lng = (bbox.min().lng() + bbox.max().lng()) / 2.0;
        let zone = resolve_zone_cached(&client, center_lng, center_lat)?;

        let tiles = covering_tiles(bbox, zone);
        if tiles.is_empty() {
            return Err("no tiles cover the bbox".into());
        }
        println!(
            "Fetching {} elevation tiles from hoehendaten.de (DGM1, 1m)...",
            tiles.len()
        );

        let cache_dir = get_cache_dir(self.name());
        std::fs::create_dir_all(&cache_dir)?;

        // Modest concurrency keeps a full budget-sized fetch within the rate limit.
        let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build()?;
        let tile_map: HashMap<(i64, i64), Vec<f32>> = pool.install(|| {
            tiles
                .par_iter()
                .filter_map(|&(e_km, n_km)| {
                    fetch_tile(&client, &cache_dir, zone, e_km, n_km)
                        .map(|data| ((e_km, n_km), data))
                })
                .collect()
        });

        // Bilinear-sample the output grid; anything unresolved stays NaN.
        let mut nan_cells = 0usize;
        let mut height_grid: Vec<Vec<f64>> = (0..grid_height)
            .into_par_iter()
            .map(|gy| {
                let mut row = vec![f64::NAN; grid_width];
                for (gx, cell) in row.iter_mut().enumerate() {
                    let lat = bbox.max().lat()
                        - (gy as f64 / (grid_height - 1).max(1) as f64)
                            * (bbox.max().lat() - bbox.min().lat());
                    let lng = bbox.min().lng()
                        + (gx as f64 / (grid_width - 1).max(1) as f64)
                            * (bbox.max().lng() - bbox.min().lng());
                    let (e, n) = latlon_to_utm(lat, lng, zone);
                    if let Some(v) = sample_bilinear(&tile_map, e, n) {
                        *cell = v;
                    }
                }
                row
            })
            .collect();
        for row in &height_grid {
            nan_cells += row.iter().filter(|v| v.is_nan()).count();
        }

        // Border bboxes reach past German coverage; fill the gaps from AWS.
        if nan_cells > 0 {
            if let Ok(aws) = AwsTerrain.fetch_raw(bbox, grid_width, grid_height) {
                for (gy, row) in height_grid.iter_mut().enumerate() {
                    for (gx, cell) in row.iter_mut().enumerate() {
                        if cell.is_nan() {
                            *cell = aws.heights_meters[gy][gx];
                        }
                    }
                }
            }
        }

        Ok(RawElevationGrid {
            heights_meters: height_grid,
        })
    }
}

fn build_client() -> Result<reqwest::blocking::Client, Box<dyn std::error::Error>> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(concat!(
            "Arnis/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/louis-e/arnis)"
        ))
        .timeout(std::time::Duration::from_secs(60))
        .build()?)
}

// POST helper; the API rejects any Accept other than application/json and
// gzips every successful response regardless of Accept-Encoding.
fn post_json(
    client: &reqwest::blocking::Client,
    path: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let resp = client
        .post(format!("{API_BASE}{path}"))
        .header("Accept", "application/json")
        .json(body)
        .send()?;
    let status = resp.status();
    let raw = resp.bytes()?.to_vec();
    let text = if raw.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(raw.as_slice()).read_to_end(&mut out)?;
        out
    } else {
        raw
    };
    let json: serde_json::Value = serde_json::from_slice(&text)?;
    if !status.is_success() {
        let detail = json
            .pointer("/Attributes/Error/Detail")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(format!("HTTP {status}: {detail}").into());
    }
    Ok(json)
}

// Resolve the state's publication zone via a point probe (TileIndex = "zone_eKm_nKm").
fn resolve_zone(
    client: &reqwest::blocking::Client,
    lng: f64,
    lat: f64,
) -> Result<u8, Box<dyn std::error::Error>> {
    let body = serde_json::json!({
        "Type": "PointRequest",
        "ID": "arnis",
        "Attributes": { "Longitude": lng, "Latitude": lat }
    });
    let json = post_json(client, "/v1/point", &body)?;
    let tile_index = json
        .pointer("/Attributes/TileIndex")
        .and_then(|v| v.as_str())
        .ok_or("missing TileIndex in point response")?;
    let zone: u8 = tile_index
        .split('_')
        .next()
        .ok_or("malformed TileIndex")?
        .parse()?;
    Ok(zone)
}

// 1 km tile keys covering the bbox in the given zone.
fn covering_tiles(bbox: &LLBBox, zone: u8) -> Vec<(i64, i64)> {
    let corners = [
        (bbox.min().lat(), bbox.min().lng()),
        (bbox.min().lat(), bbox.max().lng()),
        (bbox.max().lat(), bbox.min().lng()),
        (bbox.max().lat(), bbox.max().lng()),
    ];
    let (mut e_min, mut e_max, mut n_min, mut n_max) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for (lat, lng) in corners {
        let (e, n) = latlon_to_utm(lat, lng, zone);
        e_min = e_min.min(e);
        e_max = e_max.max(e);
        n_min = n_min.min(n);
        n_max = n_max.max(n);
    }
    // Small margin so edge samples can read bilinear neighbours and grid
    // convergence between the corners can't drop a boundary tile.
    e_min -= 2.0;
    e_max += 2.0;
    n_min -= 2.0;
    n_max += 2.0;
    let mut tiles = Vec::new();
    for e_km in (e_min / TILE_METERS).floor() as i64..=(e_max / TILE_METERS).floor() as i64 {
        for n_km in (n_min / TILE_METERS).floor() as i64..=(n_max / TILE_METERS).floor() as i64 {
            tiles.push((e_km, n_km));
        }
    }
    tiles
}

// Conservative tile-count estimate without a network call: the publication
// zone is unknown here, so take the larger count of both nominal zones.
fn estimate_tile_count(bbox: &LLBBox) -> usize {
    covering_tiles(bbox, 32)
        .len()
        .max(covering_tiles(bbox, 33).len())
}

// Fetch one 1km tile as a 1000x1000 f32 grid (row 0 = north); NaN for nodata.
// Cached decoded as raw f32 LE so reloads skip the LZW decode.
fn fetch_tile(
    client: &reqwest::blocking::Client,
    cache_dir: &std::path::Path,
    zone: u8,
    e_km: i64,
    n_km: i64,
) -> Option<Vec<f32>> {
    let cache_path = cache_dir.join(format!("z{zone}_{e_km}_{n_km}.f32"));
    if let Ok(bytes) = std::fs::read(&cache_path) {
        if bytes.len() == TILE_PIXELS * TILE_PIXELS * 4 {
            return Some(
                bytes
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect(),
            );
        }
        let _ = std::fs::remove_file(&cache_path);
    }

    let body = serde_json::json!({
        "Type": "RawTIFRequest",
        "ID": "arnis",
        "Attributes": {
            "Zone": zone,
            "Easting": e_km as f64 * TILE_METERS + 500.0,
            "Northing": n_km as f64 * TILE_METERS + 500.0
        }
    });
    // Tiles outside German coverage return a 400; treat as missing, no retry.
    let json = post_json(client, "/v1/rawtif", &body).ok()?;
    let raw_tifs = json.pointer("/Attributes/RawTIFs")?.as_array()?;

    // A state-border tile can arrive as up to 3 overlapping TIFFs; merge them.
    let mut merged = vec![f32::NAN; TILE_PIXELS * TILE_PIXELS];
    let mut any = false;
    for entry in raw_tifs {
        let b64 = entry.get("Data")?.as_str()?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
        if let Some(pixels) = decode_tile_tiff(&bytes) {
            for (dst, src) in merged.iter_mut().zip(pixels.iter()) {
                if dst.is_nan() && !src.is_nan() {
                    *dst = *src;
                }
            }
            any = true;
        }
    }
    if !any {
        return None;
    }

    let mut out = Vec::with_capacity(merged.len() * 4);
    for v in &merged {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let _ = std::fs::write(&cache_path, out);
    Some(merged)
}

// LZW strip GeoTIFF, float32, 1000x1000, nodata -9999 -> NaN.
fn decode_tile_tiff(bytes: &[u8]) -> Option<Vec<f32>> {
    let mut decoder = tiff::decoder::Decoder::new(std::io::Cursor::new(bytes)).ok()?;
    let (w, h) = decoder.dimensions().ok()?;
    if w as usize != TILE_PIXELS || h as usize != TILE_PIXELS {
        return None;
    }
    match decoder.read_image().ok()? {
        tiff::decoder::DecodingResult::F32(data) => Some(
            data.into_iter()
                .map(|v| if v == NODATA { f32::NAN } else { v })
                .collect(),
        ),
        _ => None,
    }
}

// Bilinear sample at UTM (easting, northing); crosses tile edges via the map.
fn sample_bilinear(tile_map: &HashMap<(i64, i64), Vec<f32>>, e: f64, n: f64) -> Option<f64> {
    // Pixel centers sit at 0.5m offsets; row 0 is the northern edge.
    let fx = e - 0.5;
    let fy = -n - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let dx = fx - x0;
    let dy = fy - y0;

    let sample = |xi: f64, yi: f64| -> Option<f64> {
        let e_km = (xi / TILE_METERS).floor() as i64;
        let n_km = (-(yi + 1.0) / TILE_METERS).floor() as i64;
        let px = (xi - e_km as f64 * TILE_METERS) as usize;
        let py = (yi + (n_km + 1) as f64 * TILE_METERS) as usize;
        let tile = tile_map.get(&(e_km, n_km))?;
        let v = *tile.get(py * TILE_PIXELS + px)?;
        if v.is_nan() {
            None
        } else {
            Some(v as f64)
        }
    };

    let v00 = sample(x0, y0)?;
    let v10 = sample(x0 + 1.0, y0)?;
    let v01 = sample(x0, y0 + 1.0)?;
    let v11 = sample(x0 + 1.0, y0 + 1.0)?;
    let top = v00 + (v10 - v00) * dx;
    let bot = v01 + (v11 - v01) * dx;
    Some(top + (bot - top) * dy)
}

// WGS84 -> UTM (Karney-style Krueger series, mm accuracy; ETRS89 offset is cm-level).
pub(crate) fn latlon_to_utm(lat: f64, lon: f64, zone: u8) -> (f64, f64) {
    let a = 6_378_137.0;
    let f = 1.0 / 298.257_223_563;
    let k0 = 0.9996;
    let n = f / (2.0 - f);
    let n2 = n * n;
    let n3 = n2 * n;
    let n4 = n3 * n;
    let big_a = a / (1.0 + n) * (1.0 + n2 / 4.0 + n4 / 64.0);
    let alpha = [
        n / 2.0 - 2.0 / 3.0 * n2 + 5.0 / 16.0 * n3 + 41.0 / 180.0 * n4,
        13.0 / 48.0 * n2 - 3.0 / 5.0 * n3 + 557.0 / 1440.0 * n4,
        61.0 / 240.0 * n3 - 103.0 / 140.0 * n4,
        49561.0 / 161280.0 * n4,
    ];

    let lat_r = lat.to_radians();
    let lon0 = (zone as f64 * 6.0 - 183.0).to_radians();
    let dlon = lon.to_radians() - lon0;

    let e2: f64 = f * (2.0 - f);
    let e = e2.sqrt();
    let t = (lat_r.tan().asinh() - e * (e * lat_r.sin()).atanh()).sinh();
    let xi_prime = t.atan2(dlon.cos());
    let eta_prime = (dlon.sin() / (1.0 + t * t).sqrt()).asinh();

    let mut xi = xi_prime;
    let mut eta = eta_prime;
    for (j, a_j) in alpha.iter().enumerate() {
        let k = 2.0 * (j as f64 + 1.0);
        xi += a_j * (k * xi_prime).sin() * (k * eta_prime).cosh();
        eta += a_j * (k * xi_prime).cos() * (k * eta_prime).sinh();
    }

    let easting = 500_000.0 + k0 * big_a * eta;
    let northing = k0 * big_a * xi;
    (easting, northing)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Anchors verified live against the API's TileIndex responses.
    #[test]
    fn utm_matches_api_tile_index_munich() {
        let (e, n) = latlon_to_utm(48.137, 11.575, 32);
        assert_eq!((e / 1000.0).floor() as i64, 691);
        assert_eq!((n / 1000.0).floor() as i64, 5334);
    }

    #[test]
    fn utm_matches_api_tile_index_berlin_zone33() {
        let (e, n) = latlon_to_utm(52.52, 13.405, 33);
        assert_eq!((e / 1000.0).floor() as i64, 391);
        assert_eq!((n / 1000.0).floor() as i64, 5820);
    }

    // Bavaria publishes in zone 32 even east of 12E (Passau, easting ~828km).
    #[test]
    fn utm_out_of_zone_easting_passau() {
        let (e, n) = latlon_to_utm(48.57, 13.45, 32);
        let e_km = (e / 1000.0).floor() as i64;
        let n_km = (n / 1000.0).floor() as i64;
        assert!((825..=832).contains(&e_km), "easting {e_km}km");
        assert!((5385..=5393).contains(&n_km), "northing {n_km}km");
    }

    // Offline: the size gate rejects before any network probe runs.
    #[test]
    fn small_bbox_within_budget_large_declined() {
        let small = LLBBox::new(48.13, 11.56, 48.15, 11.59).unwrap();
        assert!(estimate_tile_count(&small) <= MAX_TILES);
        let large = LLBBox::new(48.0, 11.3, 48.2, 11.8).unwrap();
        assert!(!GermanyDgm1.accepts(&large));
    }

    // Live check: grid values must match the API's own point elevations.
    // Run explicitly: cargo test --bin arnis dgm1_grid_matches -- --ignored
    #[test]
    #[ignore]
    fn dgm1_grid_matches_point_ground_truth() {
        let bbox = LLBBox::new(48.118, 11.588, 48.128, 11.602).unwrap();
        let (w, h) = (200usize, 150usize);
        let grid = GermanyDgm1.fetch_dgm(&bbox, w, h).unwrap();
        let client = build_client().unwrap();

        for (gy, gx) in [(20usize, 30usize), (75, 100), (130, 170)] {
            let lat = bbox.max().lat()
                - (gy as f64 / (h - 1) as f64) * (bbox.max().lat() - bbox.min().lat());
            let lng = bbox.min().lng()
                + (gx as f64 / (w - 1) as f64) * (bbox.max().lng() - bbox.min().lng());
            let body = serde_json::json!({
                "Type": "PointRequest", "ID": "arnis-test",
                "Attributes": { "Longitude": lng, "Latitude": lat }
            });
            let json = post_json(&client, "/v1/point", &body).unwrap();
            let truth = json
                .pointer("/Attributes/Elevation")
                .and_then(|v| v.as_f64())
                .unwrap();
            let ours = grid.heights_meters[gy][gx];
            let diff = (ours - truth).abs();
            println!("({lat:.5},{lng:.5}): grid {ours:.2}m vs api {truth:.2}m (diff {diff:.2}m)");
            assert!(diff < 1.5, "misaligned: {ours} vs {truth}");
        }
    }

    #[test]
    fn bilinear_sampling_crosses_tile_edges() {
        let mut map = HashMap::new();
        map.insert((0, 0), vec![10.0f32; TILE_PIXELS * TILE_PIXELS]);
        map.insert((1, 0), vec![20.0f32; TILE_PIXELS * TILE_PIXELS]);
        // Mid-tile sample.
        assert_eq!(sample_bilinear(&map, 500.0, 500.0), Some(10.0));
        // Straddling the shared edge blends both tiles.
        let v = sample_bilinear(&map, 1000.0, 500.0).unwrap();
        assert!(v > 10.0 && v < 20.0, "edge blend: {v}");
    }
}
