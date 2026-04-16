use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::cache::get_cache_dir;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// USGS 3D Elevation Program (3DEP) — USA + territories.
/// Resolution: up to 1m LiDAR (CONUS), 3m/10m elsewhere, fallback 30m.
/// License: Public Domain (USGS).
pub struct Usgs3dep;

impl ElevationProvider for Usgs3dep {
    fn name(&self) -> &'static str {
        "usgs_3dep"
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        Some(vec![
            // CONUS
            LLBBox::new(24.0, -125.0, 50.0, -66.0).unwrap(),
            // Alaska
            LLBBox::new(51.0, -180.0, 72.0, -129.0).unwrap(),
            // Hawaii
            LLBBox::new(18.5, -161.0, 22.5, -154.0).unwrap(),
            // Puerto Rico + USVI
            LLBBox::new(17.5, -68.0, 18.7, -64.0).unwrap(),
            // Guam
            LLBBox::new(13.2, 144.5, 13.7, 145.0).unwrap(),
        ])
    }

    fn native_resolution_m(&self) -> f64 {
        1.0
    }

    fn fetch_raw(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
        let url = format!(
            "https://elevation.nationalmap.gov/arcgis/rest/services/3DEPElevation/ImageServer/exportImage\
             ?bbox={},{},{},{}\
             &bboxSR=4326&imageSR=4326\
             &size={},{}\
             &format=tiff&pixelType=F32\
             &interpolation=RSP_BilinearInterpolation\
             &f=image",
            bbox.min().lng(), bbox.min().lat(),
            bbox.max().lng(), bbox.max().lat(),
            grid_width, grid_height
        );

        let cache_dir = get_cache_dir(self.name());
        std::fs::create_dir_all(&cache_dir)?;

        let cache_key = bbox_hash(bbox, grid_width, grid_height);
        let cache_path = cache_dir.join(format!("{cache_key}.tiff"));

        let bytes = fetch_or_cache(&url, &cache_path, None)?;
        decode_geotiff_f32(&bytes, grid_width, grid_height)
    }
}

/// IGN France RGE ALTI — France + overseas territories.
/// Resolution: 1m mainland France, 1-5m overseas.
/// License: Licence Ouverte 2.0.
pub struct IgnFrance;

impl ElevationProvider for IgnFrance {
    fn name(&self) -> &'static str {
        "ign_france"
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        Some(vec![
            // Metropolitan France
            LLBBox::new(41.0, -5.5, 51.5, 10.0).unwrap(),
            // Guadeloupe
            LLBBox::new(15.8, -61.9, 16.6, -60.9).unwrap(),
            // Martinique
            LLBBox::new(14.3, -61.3, 14.9, -60.8).unwrap(),
            // French Guiana
            LLBBox::new(2.0, -55.0, 6.0, -51.0).unwrap(),
            // Réunion
            LLBBox::new(-21.5, 55.1, -20.8, 55.9).unwrap(),
            // Mayotte
            LLBBox::new(-13.1, 44.9, -12.5, 45.4).unwrap(),
        ])
    }

    fn native_resolution_m(&self) -> f64 {
        1.0
    }

    fn fetch_raw(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
        // WMS 1.3.0 with CRS=EPSG:4326 uses lat,lng order for BBOX
        // STYLES= must be present (empty is fine) or the server returns 400
        let url = format!(
            "https://data.geopf.fr/wms-r/wms\
             ?SERVICE=WMS&REQUEST=GetMap&VERSION=1.3.0\
             &LAYERS=ELEVATION.ELEVATIONGRIDCOVERAGE\
             &STYLES=\
             &CRS=EPSG:4326\
             &BBOX={},{},{},{}\
             &WIDTH={}&HEIGHT={}\
             &FORMAT=image/geotiff",
            bbox.min().lat(),
            bbox.min().lng(),
            bbox.max().lat(),
            bbox.max().lng(),
            grid_width,
            grid_height
        );

        let cache_dir = get_cache_dir(self.name());
        std::fs::create_dir_all(&cache_dir)?;

        let cache_key = bbox_hash(bbox, grid_width, grid_height);
        let cache_path = cache_dir.join(format!("{cache_key}.tiff"));

        let bytes = fetch_or_cache(&url, &cache_path, None)?;
        decode_geotiff_f32(&bytes, grid_width, grid_height)
    }
}

/// IGN España MDT — Spain + Canary Islands + Balearic Islands.
/// Resolution: 5m (MDT05).
/// License: CC BY 4.0.
pub struct IgnSpain;

impl ElevationProvider for IgnSpain {
    fn name(&self) -> &'static str {
        "ign_spain"
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        Some(vec![
            // Mainland Spain + Balearic Islands
            LLBBox::new(35.5, -10.0, 44.0, 5.0).unwrap(),
            // Canary Islands
            LLBBox::new(27.5, -18.5, 29.5, -13.0).unwrap(),
        ])
    }

    fn native_resolution_m(&self) -> f64 {
        5.0
    }

    fn fetch_raw(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
        let url = format!(
            "https://servicios.idee.es/wcs-inspire/mdt\
             ?SERVICE=WCS&VERSION=2.0.1&REQUEST=GetCoverage\
             &COVERAGEID=Elevacion4258_5\
             &SUBSET=Long({},{})\
             &SUBSET=Lat({},{})\
             &FORMAT=image/tiff\
             &SCALESIZE=Long({}),Lat({})",
            bbox.min().lng(),
            bbox.max().lng(),
            bbox.min().lat(),
            bbox.max().lat(),
            grid_width,
            grid_height
        );

        let cache_dir = get_cache_dir(self.name());
        std::fs::create_dir_all(&cache_dir)?;

        let cache_key = bbox_hash(bbox, grid_width, grid_height);
        let cache_path = cache_dir.join(format!("{cache_key}.tiff"));

        let bytes = fetch_or_cache(&url, &cache_path, None)?;
        decode_geotiff_f32(&bytes, grid_width, grid_height)
    }
}

/// Japan GSI Elevation Tiles — Japan.
/// Resolution: 5m (DEM5A/B/C), 10m fallback.
/// License: GSI Terms of Use (attribution required).
pub struct JapanGsi;

impl ElevationProvider for JapanGsi {
    fn name(&self) -> &'static str {
        "japan_gsi"
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        Some(vec![
            // Japan
            LLBBox::new(24.0, 122.0, 46.0, 154.0).unwrap(),
        ])
    }

    fn native_resolution_m(&self) -> f64 {
        5.0
    }

    fn fetch_raw(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
        // Use DEM5A tiles (5m resolution, best available)
        // These are XYZ PNG tiles with encoded elevation values
        let zoom: u8 = 15; // Fixed zoom for highest DEM5A resolution

        let tiles = get_xyz_tile_coordinates(bbox, zoom);
        let mut height_grid: Vec<Vec<f64>> = vec![vec![f64::NAN; grid_width]; grid_height];

        let cache_dir = get_cache_dir(self.name());
        std::fs::create_dir_all(&cache_dir)?;

        // Try DEM5A first, fall back through DEM5B, DEM5C, DEM10B
        let dem_layers = ["dem5a_png", "dem5b_png", "dem5c_png", "dem_png"];

        println!("Fetching {} elevation tiles from GSI Japan...", tiles.len());

        // Build a shared HTTP client for all tile downloads
        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!("arnis/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(120))
            .build()?;

        // Download all tiles into a HashMap
        let mut tile_map: std::collections::HashMap<(u32, u32), image::RgbaImage> =
            std::collections::HashMap::new();

        for (tile_x, tile_y) in &tiles {
            for layer in &dem_layers {
                let url = format!(
                    "https://cyberjapandata.gsi.go.jp/xyz/{}/{}/{}/{}.png",
                    layer, zoom, tile_x, tile_y
                );
                let cache_path = cache_dir.join(format!("{layer}_z{zoom}_x{tile_x}_y{tile_y}.png"));

                match fetch_or_cache(&url, &cache_path, Some(&client)) {
                    Ok(bytes) => {
                        if let Ok(img) = image::load_from_memory(&bytes) {
                            tile_map.insert((*tile_x, *tile_y), img.to_rgba8());
                            break;
                        }
                    }
                    Err(_) => continue,
                }
            }
        }

        let n = 2.0_f64.powi(zoom as i32);

        // Grid-iteration with bilinear sampling
        #[allow(clippy::needless_range_loop)]
        for gy in 0..grid_height {
            for gx in 0..grid_width {
                let lat = bbox.max().lat()
                    - (gy as f64 / (grid_height - 1).max(1) as f64)
                        * (bbox.max().lat() - bbox.min().lat());
                let lng = bbox.min().lng()
                    + (gx as f64 / (grid_width - 1).max(1) as f64)
                        * (bbox.max().lng() - bbox.min().lng());

                let lat_rad = lat.to_radians();
                let fx_global = (lng + 180.0) / 360.0 * n * 256.0;
                let fy_global =
                    (1.0 - lat_rad.tan().asinh() / std::f64::consts::PI) / 2.0 * n * 256.0;

                let tile_x = (fx_global / 256.0).floor() as u32;
                let tile_y = (fy_global / 256.0).floor() as u32;
                let px = fx_global - tile_x as f64 * 256.0;
                let py = fy_global - tile_y as f64 * 256.0;

                let x0 = px.floor() as i32;
                let y0 = py.floor() as i32;
                let dx = px - x0 as f64;
                let dy = py - y0 as f64;

                let v00 = sample_gsi_pixel(&tile_map, tile_x, tile_y, x0, y0);
                let v10 = sample_gsi_pixel(&tile_map, tile_x, tile_y, x0 + 1, y0);
                let v01 = sample_gsi_pixel(&tile_map, tile_x, tile_y, x0, y0 + 1);
                let v11 = sample_gsi_pixel(&tile_map, tile_x, tile_y, x0 + 1, y0 + 1);

                if let (Some(v00), Some(v10), Some(v01), Some(v11)) = (v00, v10, v01, v11) {
                    let lerp_top = v00 + (v10 - v00) * dx;
                    let lerp_bot = v01 + (v11 - v01) * dx;
                    height_grid[gy][gx] = lerp_top + (lerp_bot - lerp_top) * dy;
                }
            }
        }

        Ok(RawElevationGrid {
            heights_meters: height_grid,
        })
    }
}

/// Sample a single pixel from GSI tile map, handling tile boundary crossover.
/// GSI PNG encoding: h = (R*65536 + G*256 + B) * 0.01, nodata = RGB(128,0,0).
fn sample_gsi_pixel(
    tile_map: &std::collections::HashMap<(u32, u32), image::RgbaImage>,
    base_tile_x: u32,
    base_tile_y: u32,
    px: i32,
    py: i32,
) -> Option<f64> {
    let (tx, x) = if px < 0 {
        (base_tile_x.wrapping_sub(1), (px + 256) as u32)
    } else if px >= 256 {
        (base_tile_x + 1, (px - 256) as u32)
    } else {
        (base_tile_x, px as u32)
    };
    let (ty, y) = if py < 0 {
        (base_tile_y.wrapping_sub(1), (py + 256) as u32)
    } else if py >= 256 {
        (base_tile_y + 1, (py - 256) as u32)
    } else {
        (base_tile_y, py as u32)
    };

    let tile = tile_map.get(&(tx, ty))?;
    if x >= tile.width() || y >= tile.height() {
        return None;
    }
    let pixel = tile.get_pixel(x, y);
    let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
    if r == 128 && g == 0 && b == 0 {
        return None; // nodata
    }
    let raw = r as f64 * 65536.0 + g as f64 * 256.0 + b as f64;
    let height = if raw < 8388608.0 {
        raw * 0.01
    } else {
        (raw - 16777216.0) * 0.01
    };
    Some(height)
}

// --- Shared helpers ---

/// Fetch data from URL or load from cache.
/// If `client` is provided, reuse it; otherwise build a new one.
fn fetch_or_cache(
    url: &str,
    cache_path: &std::path::Path,
    client: Option<&reqwest::blocking::Client>,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if cache_path.exists() {
        let bytes = std::fs::read(cache_path)?;
        if bytes.len() > 100 {
            return Ok(bytes);
        }
        // Too small, re-download
        let _ = std::fs::remove_file(cache_path);
    }

    let owned_client;
    let client = match client {
        Some(c) => c,
        None => {
            owned_client = reqwest::blocking::Client::builder()
                .user_agent(concat!("arnis/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(120))
                .build()?;
            &owned_client
        }
    };

    let response = client.get(url).send()?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {status} from elevation service").into());
    }

    let bytes = response.bytes()?.to_vec();

    if bytes.len() > 100 {
        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(cache_path, &bytes)?;
    }

    Ok(bytes)
}

/// Decode a GeoTIFF containing float32 elevation values.
/// Attempts to read the raster data and resample to requested grid dimensions.
fn decode_geotiff_f32(
    bytes: &[u8],
    target_width: usize,
    target_height: usize,
) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
    use std::io::Cursor;
    use tiff::decoder::Decoder;

    let cursor = Cursor::new(bytes);
    let mut decoder = Decoder::new(cursor)?;

    let (src_width, src_height) = decoder.dimensions()?;
    let src_width = src_width as usize;
    let src_height = src_height as usize;

    // Read the raster data
    let result = decoder.read_image()?;
    let float_data: Vec<f64> = match result {
        tiff::decoder::DecodingResult::F32(data) => data.iter().map(|&v| v as f64).collect(),
        tiff::decoder::DecodingResult::F64(data) => data.to_vec(),
        tiff::decoder::DecodingResult::U8(data) => data.iter().map(|&v| v as f64).collect(),
        tiff::decoder::DecodingResult::U16(data) => data.iter().map(|&v| v as f64).collect(),
        tiff::decoder::DecodingResult::I16(data) => data.iter().map(|&v| v as f64).collect(),
        tiff::decoder::DecodingResult::U32(data) => data.iter().map(|&v| v as f64).collect(),
        tiff::decoder::DecodingResult::I32(data) => data.iter().map(|&v| v as f64).collect(),
        _ => return Err("Unsupported TIFF pixel type".into()),
    };

    // Resample to target dimensions using nearest-neighbor
    let mut height_grid: Vec<Vec<f64>> = vec![vec![f64::NAN; target_width]; target_height];
    let target_y_den = target_height.saturating_sub(1).max(1);
    let target_x_den = target_width.saturating_sub(1).max(1);
    let src_y_extent = src_height.saturating_sub(1);
    let src_x_extent = src_width.saturating_sub(1);

    #[allow(clippy::needless_range_loop)]
    for ty in 0..target_height {
        let sy = (ty as f64 / target_y_den as f64 * src_y_extent as f64) as usize;
        let sy = sy.min(src_y_extent);
        for tx in 0..target_width {
            let sx = (tx as f64 / target_x_den as f64 * src_x_extent as f64) as usize;
            let sx = sx.min(src_x_extent);
            let idx = sy * src_width + sx;
            if idx < float_data.len() {
                let val = float_data[idx];
                // Common nodata values
                if val > -9999.0 && val < 100000.0 && val.is_finite() {
                    height_grid[ty][tx] = val;
                }
            }
        }
    }

    Ok(RawElevationGrid {
        heights_meters: height_grid,
    })
}

/// Compute a hash key for caching based on bbox and grid dimensions.
fn bbox_hash(bbox: &LLBBox, width: usize, height: usize) -> u64 {
    let mut hasher = DefaultHasher::new();
    bbox.min().lat().to_bits().hash(&mut hasher);
    bbox.min().lng().to_bits().hash(&mut hasher);
    bbox.max().lat().to_bits().hash(&mut hasher);
    bbox.max().lng().to_bits().hash(&mut hasher);
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    hasher.finish()
}

/// Get XYZ tile coordinates covering a bbox at the given zoom level.
fn get_xyz_tile_coordinates(bbox: &LLBBox, zoom: u8) -> Vec<(u32, u32)> {
    let n = 2.0_f64.powi(zoom as i32);

    let x1 = ((bbox.min().lng() + 180.0) / 360.0 * n).floor() as u32;
    let x2 = ((bbox.max().lng() + 180.0) / 360.0 * n).floor() as u32;

    let y1 = ((1.0 - bbox.max().lat().to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n)
        .floor() as u32;
    let y2 = ((1.0 - bbox.min().lat().to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n)
        .floor() as u32;

    let mut tiles = Vec::new();
    for x in x1.min(x2)..=x1.max(x2) {
        for y in y1.min(y2)..=y1.max(y2) {
            tiles.push((x, y));
        }
    }
    tiles
}
