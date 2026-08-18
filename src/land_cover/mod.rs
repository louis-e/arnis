//! ESA WorldCover 2021 land cover data integration.
//!
//! Fetches satellite-derived land classification data at 10m resolution from
//! ESA WorldCover (hosted on AWS S3). The data provides 11 land cover classes
//! (tree cover, shrubland, grassland, cropland, built-up, etc.) which are used
//! to select appropriate surface blocks in the Minecraft world.
//!
//! The data is stored as Cloud-Optimized GeoTIFF (COG) tiles covering 3x3 degree
//! areas. We use HTTP Range requests to read only the portions we need, avoiding
//! downloading the full ~500MB tiles.

pub mod bridge_repair;
pub mod osm_land_override;
pub mod osm_water_override;
mod shoreline;

#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use crate::{coordinate_system::geographic::LLBBox, progress::emit_gui_progress_update};
use flate2::read::DeflateDecoder;
use once_cell::sync::OnceCell;
use rayon::prelude::*;
use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};

/// ESA WorldCover 2021 S3 base URL
const ESA_BASE_URL: &str = "https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/2021/map";

/// Cache directory for land cover data
const LAND_COVER_CACHE_DIR: &str = "arnis-landcover-cache";

/// ESA tile size in degrees (each tile covers a 3x3 degree area)
const ESA_TILE_DEGREES: f64 = 3.0;

/// Grid cells per real-world meter, from the bbox's true width.
///
/// Both smoothing passes below have to break up ESA's 10 m rectangular steps, which is a
/// distance in *meters*, not in cells. At scale 1.0 a cell is a metre and the two agree, but
/// at scale 2.0 a cell is half a metre, so a sigma fixed in cells reaches only half as far and
/// the steps survive as visible rectangular patches.
fn cells_per_meter(bbox: &LLBBox, grid_width: usize) -> f64 {
    let (_, width_m) =
        crate::coordinate_system::transformation::geo_distance(bbox.min(), bbox.max());
    if width_m > 0.0 && grid_width > 0 {
        grid_width as f64 / width_m
    } else {
        1.0
    }
}

// ─── Land cover class constants ────────────────────────────────────────────

/// Tree cover (forests, dense tree canopy)
pub const LC_TREE_COVER: u8 = 10;
/// Shrubland (bushes, low vegetation)
pub const LC_SHRUBLAND: u8 = 20;
/// Grassland (grass, meadows)
pub const LC_GRASSLAND: u8 = 30;
/// Cropland (agricultural fields)
pub const LC_CROPLAND: u8 = 40;
/// Built-up areas (urban, roads, buildings)
pub const LC_BUILT_UP: u8 = 50;
/// Bare / sparse vegetation (desert, rock, barren)
pub const LC_BARE: u8 = 60;
/// Snow and ice (glaciers, permanent snow)
pub const LC_SNOW_ICE: u8 = 70;
/// Permanent water bodies
pub const LC_WATER: u8 = 80;
/// Herbaceous wetland (marshes, swamps)
pub const LC_WETLAND: u8 = 90;
/// Mangroves
pub const LC_MANGROVES: u8 = 95;
/// Moss and lichen (falls through to default grass in surface selection)
#[allow(dead_code)]
pub const LC_MOSS: u8 = 100;

// ─── Data structures ──────────────────────────────────────────────────────

/// Land cover classification grid aligned with the elevation grid.
#[derive(Clone)]
pub struct LandCoverData {
    /// Classification values (ESA codes) for each grid cell, indexed as [z][x]
    pub grid: Vec<Vec<u8>>,
    /// Distance from each water cell to nearest shore, indexed as [z][x].
    /// 0 = non-water, 1 = shore water, 2+ = progressively deeper water.
    pub water_distance: Vec<Vec<u8>>,
    /// Lazily computed smoothed water mask, so the blur runs once instead of after every mutation.
    pub water_blend_cache: OnceCell<Vec<Vec<f32>>>,
    /// Grid width (matches elevation grid width)
    pub width: usize,
    /// Grid height (matches elevation grid height)
    pub height: usize,
    /// Grid cells per real-world meter, so the smoothing passes can size themselves in meters.
    pub cells_per_meter: f64,
}

impl LandCoverData {
    /// Smoothed water mask, computed on first use after the last grid mutation.
    pub(crate) fn water_blend_grid(&self) -> &[Vec<f32>] {
        self.water_blend_cache.get_or_init(|| {
            compute_water_blend_smooth(&self.grid, self.width, self.height, self.cells_per_meter)
        })
    }

    /// Call after any mutation to `grid` so the mask is recomputed on next use.
    pub(crate) fn invalidate_water_blend_grid(&mut self) {
        self.water_blend_cache = OnceCell::new();
    }
}

/// Build a smooth `[0, 1]` water-ness field from the binary `LC_WATER` mask
/// in the classification grid, by applying a Gaussian blur.
///
/// σ = 3 cells is a compromise:
/// - 1-to-1 grid-to-world mapping (small/medium bbox on a high-res provider):
///   gives a ~3 block softening band — enough to break the ESA 10 m grid
///   rectangular steps without visibly eroding the shoreline.
/// - Coarser grid-to-world (large bbox, capped by the cell budget): each cell
///   already represents many blocks, so a 3-cell blur represents many blocks of
///   softening — appropriate for the coarser effective resolution.
/// - Finer than 1 cell per metre (scale above 1.0): 3 cells would now be under
///   3 m and stop covering the 10 m steps, so σ is held at 3 m instead.
fn compute_water_blend_smooth(
    grid: &[Vec<u8>],
    width: usize,
    height: usize,
    cells_per_meter: f64,
) -> Vec<Vec<f32>> {
    const SIGMA_CELLS: f64 = 3.0;
    const SIGMA_METERS: f64 = 3.0;
    let sigma = SIGMA_CELLS.max(SIGMA_METERS * cells_per_meter);

    if width == 0 || height == 0 {
        return Vec::new();
    }
    let binary: Vec<Vec<f64>> = grid
        .iter()
        .take(height)
        .map(|row| {
            row.iter()
                .take(width)
                .map(|&c| if c == LC_WATER { 1.0 } else { 0.0 })
                .collect()
        })
        .collect();
    // Gaussian blur runs in f64 for numerical stability, then we drop down to
    // f32 for storage — values land in [0, 1] and are only ever compared to a
    // 0.5 threshold, so precision beyond f32 is wasted.
    crate::elevation::postprocess::gaussian_blur_grid(&binary, sigma)
        .into_iter()
        .map(|row| row.into_iter().map(|v| v as f32).collect())
        .collect()
}

/// Metadata parsed from a COG (Cloud-Optimized GeoTIFF) IFD.
struct CogInfo {
    image_width: u64,
    image_height: u64,
    tile_width: u64,
    tile_height: u64,
    tile_offsets: Vec<u64>,
    tile_byte_counts: Vec<u64>,
    compression: u16,
}

// ─── Public API ───────────────────────────────────────────────────────────

/// Fetches ESA WorldCover land cover data for the given bounding box and
/// builds a classification grid matching the specified dimensions.
///
/// Returns `None` if the data cannot be fetched (graceful fallback).
pub fn fetch_land_cover_data(
    bbox: &LLBBox,
    grid_width: usize,
    grid_height: usize,
) -> Option<LandCoverData> {
    println!("Fetching land cover data (ESA WorldCover 2021)...");
    emit_gui_progress_update(9.0, "Downloading data...");

    let cache_dir = get_cache_dir();
    if !cache_dir.exists() {
        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
            eprintln!("Warning: Failed to create land cover cache directory: {e}");
            return None;
        }
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .ok()?;

    // Determine which ESA tiles overlap our bbox
    let tile_specs = get_esa_tile_specs(bbox);
    if tile_specs.is_empty() {
        eprintln!("Warning: Bounding box outside ESA WorldCover coverage (-60° to +84° latitude)");
        return None;
    }

    // Read the ESA pixels into one raster, then resample every grid cell from it.
    // Sampling per cell is what keeps the grid gap-free at any --scale, and the raster
    // also feeds the shoreline reconstruction below.
    let mut raster: Option<EsaPixelRaster> = None;
    let cells_per_meter = cells_per_meter(bbox, grid_width);
    for (tile_lat, tile_lng, tile_url) in &tile_specs {
        match read_esa_tile_into_raster(
            &client,
            tile_url,
            &cache_dir,
            *tile_lat,
            *tile_lng,
            bbox,
            &mut raster,
        ) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Warning: Failed to read ESA tile {tile_url}: {e}");
            }
        }
    }

    // Check if we got any valid data
    let has_data = raster
        .as_ref()
        .is_some_and(|r| r.data.iter().any(|&v| v != 0));
    if !has_data {
        eprintln!("Warning: No land cover data received for this area");
        #[cfg(feature = "gui")]
        send_log(
            LogLevel::Warning,
            "ESA WorldCover returned no data for the requested bbox (generation proceeding without land cover).",
        );
        return None;
    }
    let raster = raster?;
    let mapping = GridMapping::new(bbox, raster.ppd, grid_width, grid_height)?;
    let mut grid = raster.sample_grid(&mapping);

    // Straighten the shore before anything else reads the water mask.
    shoreline::reconstruct_water_shoreline(&raster, &mapping, &mut grid);
    drop(raster);

    // Fill gaps (0 values surrounded by valid data) with nearest neighbor
    fill_gaps(&mut grid, grid_width, grid_height);

    // Smooth class boundaries via Gaussian-weighted local voting. Replaces
    // the rectangular axis-aligned 10 m ESA steps with clean smooth contours
    // for every class (including water shorelines).
    smooth_class_boundaries(&mut grid, grid_width, grid_height, cells_per_meter);

    // Compute distance from each water cell to nearest shore via multi-source BFS.
    // Used for shoreline blending (land cells adjacent to water get sand surface).
    let water_distance = compute_water_distance(&grid, grid_width, grid_height);

    Some(LandCoverData {
        grid,
        water_distance,
        water_blend_cache: OnceCell::new(),
        width: grid_width,
        height: grid_height,
        cells_per_meter,
    })
}

// ─── Cache helpers ────────────────────────────────────────────────────────

fn get_cache_dir() -> PathBuf {
    if let Some(cache_dir) = dirs::cache_dir() {
        cache_dir.join(LAND_COVER_CACHE_DIR)
    } else {
        PathBuf::from(format!("./{LAND_COVER_CACHE_DIR}"))
    }
}

/// Clear every cached ESA WorldCover tile. Wrapper around the generic
/// [`crate::elevation::cache::clear_cache_dir`] so the GUI cache-clean
/// command only has to call one entry point per cache root.
pub fn clear_land_cover_cache() -> crate::elevation::cache::CacheClearStats {
    crate::elevation::cache::clear_cache_dir(&get_cache_dir())
}

// ─── ESA tile URL computation ─────────────────────────────────────────────

/// Returns a list of (tile_lat, tile_lng, url) for ESA tiles overlapping the bbox.
///
/// ESA WorldCover tiles are named by their southwest corner, snapped to a 3-degree grid.
/// Coverage: latitude -60 to +84, longitude -180 to +180.
fn get_esa_tile_specs(bbox: &LLBBox) -> Vec<(f64, f64, String)> {
    let min_lat = bbox.min().lat();
    let max_lat = bbox.max().lat();
    let min_lng = bbox.min().lng();
    let max_lng = bbox.max().lng();

    // ESA coverage limits
    if max_lat < -60.0 || min_lat > 84.0 {
        return Vec::new();
    }

    let min_lat = min_lat.max(-60.0);
    // Clamp just below the boundary so snap_to_grid doesn't produce an
    // invalid SW corner at the dataset edge (last valid SW is 81°N / 177°E)
    let max_lat = max_lat.min(84.0 - 0.001);

    // Snap to 3-degree grid (floor to nearest multiple of 3)
    let lat_start = snap_to_grid(min_lat);
    let lat_end = snap_to_grid(max_lat);
    let lng_start = snap_to_grid(min_lng);
    let lng_end = snap_to_grid(max_lng);

    let mut specs = Vec::new();
    let mut lat = lat_start;
    while lat <= lat_end {
        let mut lng = lng_start;
        while lng <= lng_end {
            let url = esa_tile_url(lat, lng);
            specs.push((lat, lng, url));
            lng += ESA_TILE_DEGREES;
        }
        lat += ESA_TILE_DEGREES;
    }
    specs
}

/// Snap a coordinate to the ESA 3-degree grid (floor).
fn snap_to_grid(coord: f64) -> f64 {
    (coord / ESA_TILE_DEGREES).floor() * ESA_TILE_DEGREES
}

/// Build the ESA tile URL from the southwest corner coordinates.
fn esa_tile_url(lat: f64, lng: f64) -> String {
    let ns = if lat >= 0.0 { 'N' } else { 'S' };
    let ew = if lng >= 0.0 { 'E' } else { 'W' };
    let lat_abs = lat.abs() as u32;
    let lng_abs = lng.abs() as u32;
    format!("{ESA_BASE_URL}/ESA_WorldCover_10m_2021_v200_{ns}{lat_abs:02}{ew}{lng_abs:03}_Map.tif")
}

// ─── COG reading ──────────────────────────────────────────────────────────

/// ESA pixels covering the bbox, in the product's global pixel grid:
/// `x = floor((lng + 180) * ppd)`, `y = floor((90 - lat) * ppd)`. Class 0 = nodata.
pub(crate) struct EsaPixelRaster {
    /// Global pixel column of the first raster column.
    pub x0: i64,
    /// Global pixel row of the first raster row.
    pub y0: i64,
    pub width: usize,
    pub height: usize,
    /// Pixels per degree (12000 for the 10 m product).
    pub ppd: f64,
    /// Row-major classes, `height * width`.
    pub data: Vec<u8>,
}

/// Raster cap, so a pathological bbox fails instead of allocating unbounded.
const MAX_RASTER_PIXELS: usize = 1 << 31;

impl EsaPixelRaster {
    /// Raster covering `bbox` at `ppd`, filled with nodata.
    fn covering(bbox: &LLBBox, ppd: f64) -> Option<Self> {
        let x0 = ((bbox.min().lng() + 180.0) * ppd).floor();
        let x1 = ((bbox.max().lng() + 180.0) * ppd).floor();
        let y0 = ((90.0 - bbox.max().lat()) * ppd).floor();
        let y1 = ((90.0 - bbox.min().lat()) * ppd).floor();
        if !(x0.is_finite() && x1.is_finite() && y0.is_finite() && y1.is_finite()) {
            return None;
        }
        // Inclusive on both ends: the cell at the bbox edge lies in the pixel
        // containing max_lng / min_lat.
        let width = (x1 - x0) as i64 + 1;
        let height = (y1 - y0) as i64 + 1;
        if width <= 0 || height <= 0 {
            return None;
        }
        let pixels = (width as usize).checked_mul(height as usize)?;
        if pixels > MAX_RASTER_PIXELS {
            eprintln!(
                "Warning: bounding box spans {} ESA WorldCover pixels, too many to hold; skipping land cover",
                pixels
            );
            return None;
        }
        // A country-scale bbox asks for a gigabyte here, so failing to get it has to drop
        // land cover rather than abort the process the way an infallible alloc would.
        let mut data: Vec<u8> = Vec::new();
        if data.try_reserve_exact(pixels).is_err() {
            eprintln!(
                "Warning: could not allocate the {pixels}-pixel ESA WorldCover raster; skipping land cover"
            );
            return None;
        }
        data.resize(pixels, 0);
        Some(Self {
            x0: x0 as i64,
            y0: y0 as i64,
            width: width as usize,
            height: height as usize,
            ppd,
            data,
        })
    }

    /// Nearest-neighbour resample: every grid cell takes the class of its pixel.
    /// Nodata pixels leave 0 for `fill_gaps`.
    pub(crate) fn sample_grid(&self, mapping: &GridMapping) -> Vec<Vec<u8>> {
        let (gw, gh) = (mapping.grid_w, mapping.grid_h);
        let mut grid = vec![vec![0u8; gw]; gh];
        if gw == 0 || gh == 0 || self.width == 0 || self.height == 0 {
            return grid;
        }
        // Grid column span of every pixel column, computed once. Only the columns that
        // land on a cell are kept, so a grid coarser than the pixels walks the grid width
        // per row instead of the pixel width.
        let col_spans: Vec<(usize, usize, usize)> = (0..self.width)
            .filter_map(|px| {
                let x = (self.x0 + px as i64) as f64;
                let (lo, hi) = mapping.cell_span(mapping.gx(x), mapping.gx(x + 1.0), gw);
                (lo < hi).then_some((px, lo, hi))
            })
            .collect();
        let mut row_buf = vec![0u8; gw];
        for py in 0..self.height {
            let y = (self.y0 + py as i64) as f64;
            let (z_lo, z_hi) = mapping.cell_span(mapping.gz(y), mapping.gz(y + 1.0), gh);
            if z_lo >= z_hi {
                continue;
            }
            let src = &self.data[py * self.width..(py + 1) * self.width];
            row_buf.fill(0);
            for &(px, lo, hi) in &col_spans {
                row_buf[lo..hi].fill(src[px]);
            }
            for row in grid.iter_mut().take(z_hi).skip(z_lo) {
                row.copy_from_slice(&row_buf);
            }
        }
        grid
    }
}

/// Global ESA pixel coordinates to grid cells, on the elevation grid's convention:
/// cell `gx` sits at `min_lng + gx / (grid_w - 1) * (max_lng - min_lng)`.
pub(crate) struct GridMapping {
    /// Global pixel x of the bbox west edge (fractional).
    pub x_origin: f64,
    /// Global pixel y of the bbox north edge (fractional).
    pub y_origin: f64,
    /// Grid cells per pixel along x / y.
    pub sx: f64,
    pub sy: f64,
    pub grid_w: usize,
    pub grid_h: usize,
}

impl GridMapping {
    fn new(bbox: &LLBBox, ppd: f64, grid_w: usize, grid_h: usize) -> Option<Self> {
        let lng_px = (bbox.max().lng() - bbox.min().lng()) * ppd;
        let lat_px = (bbox.max().lat() - bbox.min().lat()) * ppd;
        if !(lng_px > 0.0 && lat_px > 0.0) || grid_w < 2 || grid_h < 2 {
            return None;
        }
        Some(Self {
            x_origin: (bbox.min().lng() + 180.0) * ppd,
            y_origin: (90.0 - bbox.max().lat()) * ppd,
            sx: (grid_w - 1) as f64 / lng_px,
            sy: (grid_h - 1) as f64 / lat_px,
            grid_w,
            grid_h,
        })
    }

    /// Grid x coordinate of global pixel x.
    #[inline(always)]
    pub(crate) fn gx(&self, px_x: f64) -> f64 {
        (px_x - self.x_origin) * self.sx
    }

    /// Grid z coordinate of global pixel y.
    #[inline(always)]
    pub(crate) fn gz(&self, px_y: f64) -> f64 {
        (px_y - self.y_origin) * self.sy
    }

    /// Cells `[lo, hi)` whose coordinate lies in `[a, b)`, clamped to `0..n`.
    #[inline(always)]
    fn cell_span(&self, a: f64, b: f64, n: usize) -> (usize, usize) {
        let lo = a.ceil().max(0.0).min(n as f64) as usize;
        let hi = b.ceil().max(0.0).min(n as f64) as usize;
        (lo, hi)
    }
}

/// Read the pixels of one ESA tile overlapping the bbox into `raster`, creating it from
/// the tile's pixel size on first use. Only the COG tiles that overlap are fetched.
fn read_esa_tile_into_raster(
    client: &reqwest::blocking::Client,
    url: &str,
    cache_dir: &Path,
    tile_lat: f64,
    tile_lng: f64,
    bbox: &LLBBox,
    raster: &mut Option<EsaPixelRaster>,
) -> Result<(), Box<dyn std::error::Error>> {
    // The ESA tile covers [tile_lat, tile_lat+3] x [tile_lng, tile_lng+3]
    let tile_north = tile_lat + ESA_TILE_DEGREES;

    // Generate a cache filename from the URL
    let cache_filename = url
        .rsplit('/')
        .next()
        .unwrap_or("tile.tif")
        .replace(".tif", "_header.bin");
    let header_cache_path = cache_dir.join(&cache_filename);

    // Step 1: Read the TIFF/BigTIFF header to get IFD location
    // Read first 64KB which should contain the IFD for COG files
    let header_bytes = if header_cache_path.exists() {
        std::fs::read(&header_cache_path)?
    } else {
        let bytes = fetch_range(client, url, 0, 65536)?;
        // Cache the header for future use
        let _ = std::fs::write(&header_cache_path, &bytes);
        bytes
    };

    if header_bytes.len() < 16 {
        return Err("TIFF header too short".into());
    }

    // Step 2: Parse TIFF header
    let is_big_endian = header_bytes[0] == b'M' && header_bytes[1] == b'M';
    let magic = read_u16(&header_bytes, 2, is_big_endian);

    let is_bigtiff = magic == 43;

    let first_ifd_offset = if is_bigtiff {
        // BigTIFF: bytes 8-15 are first IFD offset (uint64)
        read_u64(&header_bytes, 8, is_big_endian)
    } else if magic == 42 {
        // Classic TIFF: bytes 4-7 are first IFD offset (uint32)
        read_u32(&header_bytes, 4, is_big_endian) as u64
    } else {
        return Err(format!("Not a valid TIFF file (magic: {magic})").into());
    };

    // Step 3: Parse IFD to get image dimensions and tile layout
    let cog = parse_ifd(
        client,
        url,
        &header_bytes,
        first_ifd_offset,
        is_bigtiff,
        is_big_endian,
    )?;

    if cog.image_width == 0 || cog.image_height == 0 {
        return Err("Image dimensions are zero".into());
    }
    if cog.tile_width == 0 || cog.tile_height == 0 {
        return Err("Tile dimensions are zero".into());
    }

    // Step 4: Place the tile in the global pixel grid. Tiles are 3 degrees square, so
    // pixel size defines the raster; a tile with a different size cannot merge.
    let ppd = cog.image_width as f64 / ESA_TILE_DEGREES;
    if (cog.image_height as f64 / ESA_TILE_DEGREES - ppd).abs() > 1e-9 {
        return Err("Non-square ESA tile pixels".into());
    }
    let raster = match raster {
        Some(r) => {
            if (r.ppd - ppd).abs() > 1e-9 {
                return Err(format!(
                    "Tile pixel size {ppd}/deg differs from the raster's {}/deg",
                    r.ppd
                )
                .into());
            }
            r
        }
        slot @ None => {
            let r = EsaPixelRaster::covering(bbox, ppd)
                .ok_or("Cannot build a pixel raster for this bounding box")?;
            slot.insert(r)
        }
    };
    let tile_px_x0 = ((tile_lng + 180.0) * ppd).round() as i64;
    let tile_px_y0 = ((90.0 - tile_north) * ppd).round() as i64;

    // Overlap of the raster with this tile, in tile-local pixels.
    let lx_lo = (raster.x0 - tile_px_x0).max(0);
    let lx_hi = (raster.x0 + raster.width as i64 - tile_px_x0).min(cog.image_width as i64);
    let ly_lo = (raster.y0 - tile_px_y0).max(0);
    let ly_hi = (raster.y0 + raster.height as i64 - tile_px_y0).min(cog.image_height as i64);
    if lx_lo >= lx_hi || ly_lo >= ly_hi {
        return Ok(());
    }
    let (lx_lo, lx_hi, ly_lo, ly_hi) = (lx_lo as u64, lx_hi as u64, ly_lo as u64, ly_hi as u64);

    // Step 5: Determine which internal tiles we need
    let tiles_across = cog.image_width.div_ceil(cog.tile_width);
    let itile_min_x = lx_lo / cog.tile_width;
    let itile_max_x = (lx_hi - 1) / cog.tile_width;
    let itile_min_y = ly_lo / cog.tile_height;
    let itile_max_y = (ly_hi - 1) / cog.tile_height;

    // Step 6: Fetch and decode each needed internal tile
    for ity in itile_min_y..=itile_max_y {
        for itx in itile_min_x..=itile_max_x {
            let tile_index = (ity * tiles_across + itx) as usize;
            if tile_index >= cog.tile_offsets.len() || tile_index >= cog.tile_byte_counts.len() {
                continue;
            }

            let offset = cog.tile_offsets[tile_index];
            let byte_count = cog.tile_byte_counts[tile_index];

            if offset == 0 || byte_count == 0 {
                continue; // Empty/missing tile
            }

            // Fetch the compressed tile data
            let tile_cache_file = cache_dir.join(format!(
                "{}_tile_{}_{}.bin",
                cache_filename.replace("_header.bin", ""),
                itx,
                ity
            ));

            let compressed_data = if tile_cache_file.exists() {
                std::fs::read(&tile_cache_file)?
            } else {
                let data = fetch_range(client, url, offset, byte_count)?;
                let _ = std::fs::write(&tile_cache_file, &data);
                data
            };

            // Decompress the tile
            let pixel_count = (cog.tile_width * cog.tile_height) as usize;
            let pixels = decompress_tile(&compressed_data, pixel_count, cog.compression)?;

            // Step 7: Copy the overlapping rows into the raster
            let tile_pixel_x0 = itx * cog.tile_width;
            let tile_pixel_y0 = ity * cog.tile_height;
            let x_from = lx_lo.max(tile_pixel_x0);
            let x_to = lx_hi.min(tile_pixel_x0 + cog.tile_width);
            let y_from = ly_lo.max(tile_pixel_y0);
            let y_to = ly_hi.min(tile_pixel_y0 + cog.tile_height);
            if x_from >= x_to {
                continue;
            }
            for abs_py in y_from..y_to {
                let src_start =
                    ((abs_py - tile_pixel_y0) * cog.tile_width + (x_from - tile_pixel_x0)) as usize;
                let src_end = src_start + (x_to - x_from) as usize;
                if src_end > pixels.len() {
                    break;
                }
                let ry = (tile_px_y0 + abs_py as i64 - raster.y0) as usize;
                let rx = (tile_px_x0 + x_from as i64 - raster.x0) as usize;
                let dst_start = ry * raster.width + rx;
                raster.data[dst_start..dst_start + (x_to - x_from) as usize]
                    .copy_from_slice(&pixels[src_start..src_end]);
            }
        }
    }

    Ok(())
}

/// Fetch a byte range from a URL via HTTP Range request.
fn fetch_range(
    client: &reqwest::blocking::Client,
    url: &str,
    start: u64,
    length: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let end = start + length - 1;
    let response = client
        .get(url)
        .header("Range", format!("bytes={start}-{end}"))
        .send()?;

    let status = response.status();
    // Must be 206 Partial Content. If the server ignores the Range header and
    // sends 200 OK, it would return the entire ~500MB GeoTIFF file.
    if status.as_u16() != 206 {
        return Err(format!("HTTP {status} fetching range from {url} (expected 206)").into());
    }

    Ok(response.bytes()?.to_vec())
}

/// Decompress a TIFF tile based on compression type.
fn decompress_tile(
    data: &[u8],
    expected_pixels: usize,
    compression: u16,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    match compression {
        1 => {
            // No compression
            Ok(data.to_vec())
        }
        8 | 32946 => {
            // DEFLATE (zlib/deflate)
            // Try raw DEFLATE first, then zlib-wrapped
            let mut result = Vec::with_capacity(expected_pixels);

            // Try zlib (with header) first
            if data.len() >= 2 && (data[0] == 0x78) {
                let mut decoder = flate2::read::ZlibDecoder::new(data);
                if decoder.read_to_end(&mut result).is_ok() && !result.is_empty() {
                    return Ok(result);
                }
                result.clear();
            }

            // Try raw DEFLATE
            let mut decoder = DeflateDecoder::new(data);
            if decoder.read_to_end(&mut result).is_ok() && !result.is_empty() {
                return Ok(result);
            }

            Err("Failed to decompress DEFLATE tile data".into())
        }
        5 => {
            // LZW - use a simple LZW decoder
            lzw_decompress(data, expected_pixels)
        }
        _ => Err(format!("Unsupported TIFF compression type: {compression}").into()),
    }
}

// ─── TIFF IFD parsing ─────────────────────────────────────────────────────

/// Parse a TIFF IFD (Image File Directory) to extract tile layout information.
fn parse_ifd(
    client: &reqwest::blocking::Client,
    url: &str,
    header_bytes: &[u8],
    ifd_offset: u64,
    is_bigtiff: bool,
    is_big_endian: bool,
) -> Result<CogInfo, Box<dyn std::error::Error>> {
    let mut info = CogInfo {
        image_width: 0,
        image_height: 0,
        tile_width: 0,
        tile_height: 0,
        tile_offsets: Vec::new(),
        tile_byte_counts: Vec::new(),
        compression: 1, // default: no compression
    };

    let ifd_start = ifd_offset as usize;

    // Determine if we need to fetch more data
    let available = header_bytes.len();
    let need_more = ifd_start >= available;

    // We may need to fetch additional data for the IFD
    let extended_bytes;
    let bytes = if need_more {
        // IFD is beyond our initial read - fetch more
        extended_bytes = fetch_range(client, url, ifd_offset, 65536)?;
        &extended_bytes
    } else {
        header_bytes
    };

    let effective_offset = if need_more { 0 } else { ifd_start };

    // Read entry count
    let (entry_count, entries_start) = if is_bigtiff {
        if effective_offset + 8 > bytes.len() {
            return Err("IFD too short for BigTIFF entry count".into());
        }
        let count = read_u64(bytes, effective_offset, is_big_endian);
        (count, effective_offset + 8)
    } else {
        if effective_offset + 2 > bytes.len() {
            return Err("IFD too short for entry count".into());
        }
        let count = read_u16(bytes, effective_offset, is_big_endian) as u64;
        (count, effective_offset + 2)
    };

    let entry_size = if is_bigtiff { 20 } else { 12 };

    for i in 0..entry_count {
        let entry_offset = entries_start + (i as usize * entry_size);
        if entry_offset + entry_size > bytes.len() {
            break;
        }

        let tag = read_u16(bytes, entry_offset, is_big_endian);
        let typ = read_u16(bytes, entry_offset + 2, is_big_endian);

        let (count, value_offset_pos) = if is_bigtiff {
            (
                read_u64(bytes, entry_offset + 4, is_big_endian),
                entry_offset + 12,
            )
        } else {
            (
                read_u32(bytes, entry_offset + 4, is_big_endian) as u64,
                entry_offset + 8,
            )
        };

        match tag {
            256 => {
                // ImageWidth
                info.image_width =
                    read_ifd_value(bytes, value_offset_pos, typ, is_bigtiff, is_big_endian);
            }
            257 => {
                // ImageLength (height)
                info.image_height =
                    read_ifd_value(bytes, value_offset_pos, typ, is_bigtiff, is_big_endian);
            }
            259 => {
                // Compression
                info.compression =
                    read_ifd_value(bytes, value_offset_pos, typ, is_bigtiff, is_big_endian) as u16;
            }
            322 => {
                // TileWidth
                info.tile_width =
                    read_ifd_value(bytes, value_offset_pos, typ, is_bigtiff, is_big_endian);
            }
            323 => {
                // TileLength (tile height)
                info.tile_height =
                    read_ifd_value(bytes, value_offset_pos, typ, is_bigtiff, is_big_endian);
            }
            324 => {
                // TileOffsets
                info.tile_offsets = read_ifd_array(
                    client,
                    url,
                    bytes,
                    header_bytes,
                    value_offset_pos,
                    typ,
                    count,
                    is_bigtiff,
                    is_big_endian,
                    need_more,
                    ifd_offset,
                )?;
            }
            325 => {
                // TileByteCounts
                info.tile_byte_counts = read_ifd_array(
                    client,
                    url,
                    bytes,
                    header_bytes,
                    value_offset_pos,
                    typ,
                    count,
                    is_bigtiff,
                    is_big_endian,
                    need_more,
                    ifd_offset,
                )?;
            }
            _ => {} // Skip other tags
        }
    }

    Ok(info)
}

/// Read a single scalar value from an IFD entry.
fn read_ifd_value(
    bytes: &[u8],
    offset: usize,
    typ: u16,
    is_bigtiff: bool,
    is_big_endian: bool,
) -> u64 {
    if offset >= bytes.len() {
        return 0;
    }
    match typ {
        1 => bytes[offset] as u64,                          // BYTE
        3 => read_u16(bytes, offset, is_big_endian) as u64, // SHORT
        4 => read_u32(bytes, offset, is_big_endian) as u64, // LONG
        16 => {
            if is_bigtiff {
                read_u64(bytes, offset, is_big_endian) // LONG8 (BigTIFF)
            } else {
                read_u32(bytes, offset, is_big_endian) as u64
            }
        }
        _ => read_u32(bytes, offset, is_big_endian) as u64,
    }
}

/// Read an array of values from an IFD entry (e.g., TileOffsets, TileByteCounts).
///
/// If the array fits inline in the entry's value field, read it directly.
/// Otherwise, the value field contains an offset to the array data, which may
/// need to be fetched via another HTTP Range request.
#[allow(clippy::too_many_arguments)]
fn read_ifd_array(
    client: &reqwest::blocking::Client,
    url: &str,
    ifd_bytes: &[u8],
    header_bytes: &[u8],
    value_offset_pos: usize,
    typ: u16,
    count: u64,
    is_bigtiff: bool,
    is_big_endian: bool,
    _ifd_was_fetched_separately: bool,
    _ifd_fetch_offset: u64,
) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
    let elem_size = match typ {
        1 => 1,  // BYTE
        3 => 2,  // SHORT
        4 => 4,  // LONG
        16 => 8, // LONG8
        _ => 4,
    };
    let total_size = count as usize * elem_size;

    // Check if the value fits inline
    let inline_capacity = if is_bigtiff { 8 } else { 4 };
    let is_inline = total_size <= inline_capacity;

    let data: Vec<u8>;
    let data_ref: &[u8];
    let data_start: usize;

    if is_inline {
        // Values are stored inline in the IFD entry
        data_ref = ifd_bytes;
        data_start = value_offset_pos;
    } else {
        // Value field contains an offset to the actual array data
        let array_offset = if is_bigtiff {
            read_u64(ifd_bytes, value_offset_pos, is_big_endian)
        } else {
            read_u32(ifd_bytes, value_offset_pos, is_big_endian) as u64
        };

        // The array offset is always an absolute file offset
        let abs_offset = array_offset;

        if (abs_offset as usize) + total_size <= header_bytes.len() {
            // Data is in the initial header read
            data_ref = header_bytes;
            data_start = abs_offset as usize;
        } else {
            // Need to fetch the array data from the server
            data = fetch_range(client, url, abs_offset, total_size as u64)?;
            data_ref = &data;
            data_start = 0;
        }
    }

    // Parse the array values
    let mut result = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let pos = data_start + i * elem_size;
        if pos + elem_size > data_ref.len() {
            break;
        }
        let val = match typ {
            1 => data_ref[pos] as u64,
            3 => read_u16(data_ref, pos, is_big_endian) as u64,
            4 => read_u32(data_ref, pos, is_big_endian) as u64,
            16 => read_u64(data_ref, pos, is_big_endian),
            _ => read_u32(data_ref, pos, is_big_endian) as u64,
        };
        result.push(val);
    }

    Ok(result)
}

// ─── Binary reading helpers ───────────────────────────────────────────────

fn read_u16(bytes: &[u8], offset: usize, big_endian: bool) -> u16 {
    if offset + 2 > bytes.len() {
        return 0;
    }
    if big_endian {
        u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
    } else {
        u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
    }
}

fn read_u32(bytes: &[u8], offset: usize, big_endian: bool) -> u32 {
    if offset + 4 > bytes.len() {
        return 0;
    }
    if big_endian {
        u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    } else {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }
}

fn read_u64(bytes: &[u8], offset: usize, big_endian: bool) -> u64 {
    if offset + 8 > bytes.len() {
        return 0;
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[offset..offset + 8]);
    if big_endian {
        u64::from_be_bytes(buf)
    } else {
        u64::from_le_bytes(buf)
    }
}

// ─── Gap filling ──────────────────────────────────────────────────────────

/// Fill gaps (zero values) in the grid using nearest-neighbor interpolation.
/// Iterates until no more gaps can be filled or a max number of passes is reached.
fn fill_gaps(grid: &mut [Vec<u8>], width: usize, height: usize) {
    // Checked up front so a gap-free grid never pays for the snapshot below.
    if !grid.iter().any(|row| row.contains(&0)) {
        return;
    }
    for _ in 0..10 {
        let mut changed = false;
        // Make a snapshot to read from while writing
        let snapshot: Vec<Vec<u8>> = grid.to_vec();

        for z in 0..height {
            for x in 0..width {
                if snapshot[z][x] != 0 {
                    continue;
                }
                // Check 4 neighbors
                let mut best = 0u8;
                let offsets: [(i64, i64); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
                for (dx, dz) in offsets {
                    let nx = x as i64 + dx;
                    let nz = z as i64 + dz;
                    if nx >= 0 && nx < width as i64 && nz >= 0 && nz < height as i64 {
                        let val = snapshot[nz as usize][nx as usize];
                        if val != 0 {
                            best = val;
                            break;
                        }
                    }
                }
                if best != 0 {
                    grid[z][x] = best;
                    changed = true;
                }
            }
        }

        if !changed {
            break;
        }
    }
}

/// Class of the nearest non-water, non-nodata cell (ring search), if any.
pub(crate) fn nearest_land_class(
    grid: &[Vec<u8>],
    gw: usize,
    gh: usize,
    x: usize,
    z: usize,
    radius: i32,
) -> Option<u8> {
    for r in 1..=radius {
        for dz in -r..=r {
            let nz = z as i32 + dz;
            if nz < 0 || nz >= gh as i32 {
                continue;
            }
            let row = &grid[nz as usize];
            // Only the ring at Chebyshev distance exactly r.
            let step = if dz.abs() == r { 1 } else { 2 * r };
            let mut dx = -r;
            while dx <= r {
                let nx = x as i32 + dx;
                if nx >= 0 && nx < gw as i32 {
                    let c = row[nx as usize];
                    if c != LC_WATER && c != 0 {
                        return Some(c);
                    }
                }
                dx += step;
            }
        }
    }
    None
}

// ─── Water distance field ─────────────────────────────────────────────────

/// Computes a distance-to-shore grid for all water cells via multi-source BFS.
///
/// Returns a grid where:
/// - 0 = non-water cell (or unreachable water)
/// - 1 = water cell on the shore (adjacent to non-water)
/// - 2+ = water cell N blocks from nearest shore
///
/// Capped at 15 to limit BFS depth for very large oceans.
pub(crate) fn compute_water_distance(
    grid: &[Vec<u8>],
    width: usize,
    height: usize,
) -> Vec<Vec<u8>> {
    let mut distance = vec![vec![0u8; width]; height];
    let mut queue = VecDeque::new();

    // Seed BFS with shore water cells (water cells adjacent to non-water or grid edge)
    for z in 0..height {
        for x in 0..width {
            if grid[z][x] != LC_WATER {
                continue;
            }
            let is_shore = [(0i32, 1i32), (0, -1), (1, 0), (-1, 0)]
                .iter()
                .any(|(dx, dz)| {
                    let nx = x as i32 + dx;
                    let nz = z as i32 + dz;
                    if nx < 0 || nx >= width as i32 || nz < 0 || nz >= height as i32 {
                        return true; // Grid edge = shore
                    }
                    grid[nz as usize][nx as usize] != LC_WATER
                });
            if is_shore {
                distance[z][x] = 1;
                queue.push_back((x, z));
            }
        }
    }

    // BFS inward from shore cells
    while let Some((x, z)) = queue.pop_front() {
        let d = distance[z][x];
        if d >= 15 {
            continue;
        }
        for (dx, dz) in [(0i32, 1i32), (0, -1), (1, 0), (-1, 0)] {
            let nx = x as i32 + dx;
            let nz = z as i32 + dz;
            if nx >= 0 && nx < width as i32 && nz >= 0 && nz < height as i32 {
                let nx = nx as usize;
                let nz = nz as usize;
                if grid[nz][nx] == LC_WATER && distance[nz][nx] == 0 {
                    distance[nz][nx] = d + 1;
                    queue.push_back((nx, nz));
                }
            }
        }
    }

    distance
}

// ─── Boundary smoothing ───────────────────────────────────────────────────

/// Smooths class boundaries via Gaussian-weighted local voting.
///
/// ESA WorldCover is 10 m resolution. At Minecraft block resolution this
/// translates to rectangular, axis-aligned class edges with 10-block steps
/// — visible as a staircase coastline and tile-grid-looking class regions.
///
/// For every cell that sits on a class boundary (any 4-connected neighbor
/// has a different class), we tally a Gaussian-weighted vote over the cell's
/// neighborhood: each nearby cell contributes to its own class's tally with
/// a weight that falls off as a Gaussian. The cell is reassigned to the
/// class with the highest total vote.
///
/// Effect:
/// - Interior cells (surrounded by same class) are untouched — flood-fills
///   of a single class stay intact.
/// - Straight boundaries stay straight (the vote is symmetric along the
///   edge) but convex corners get rounded off and concave corners get
///   filled, producing clean smooth contours instead of axis-aligned steps.
/// - A class's overall footprint is preserved — the same number of votes
///   for class C are cast on either side of a boundary, so the smoothed
///   contour follows the underlying ESA boundary rather than shifting it.
///
/// Tradeoff: 1–2-cell-wide strips (narrow rivers, hedgerows at ESA 10 m
/// resolution) can get absorbed into the surrounding class because the
/// vote neighborhood is dominated by the surroundings. For water this is
/// usually fine because OSM waterways render rivers as a separate
/// overlay; for other classes it cleans up what's often classifier noise
/// at the 10 m grain.
fn smooth_class_boundaries(
    grid: &mut [Vec<u8>],
    width: usize,
    height: usize,
    cells_per_meter: f64,
) {
    const SIGMA_CELLS: f64 = 2.0;
    // The ESA steps are 10 m wide whatever the scale, so hold the reach in meters once a cell
    // drops below a metre. Otherwise scale 2.0 smooths half as far and the steps stay square.
    const SIGMA_METERS: f64 = 2.0;
    let sigma = SIGMA_CELLS.max(SIGMA_METERS * cells_per_meter);
    let radius = (sigma * 3.0).ceil() as i32;
    let kernel_size = (radius * 2 + 1) as usize;

    // Precompute the 2D Gaussian kernel as a flat vec.
    let mut kernel = vec![0.0f64; kernel_size * kernel_size];
    let center = radius as f64;
    let two_sigma_sq = 2.0 * sigma * sigma;
    for ky in 0..kernel_size {
        for kx in 0..kernel_size {
            let dy = ky as f64 - center;
            let dx = kx as f64 - center;
            kernel[ky * kernel_size + kx] = (-(dx * dx + dy * dy) / two_sigma_sq).exp();
        }
    }

    // Snapshot once so all cells vote against the pre-mutation grid.
    let snapshot: Vec<Vec<u8>> = grid.to_vec();

    // Parallelise over rows. Each row reads from `snapshot` (shared
    // read-only) and writes only to its own row of `grid`, so there's no
    // data dependency between rows. On a 4096² grid this typically
    // amounts to 0.8M–2.4M boundary cells each doing ~169 kernel samples
    // — clearly worth the rayon dispatch.
    grid.par_iter_mut().enumerate().for_each(|(y, row)| {
        // Per-row scratch buffers reused across every boundary cell on
        // this row. `votes` is 2 KB on the stack; zero-filling it anew
        // per cell dominated runtime on large grids (1-2 M boundary
        // cells × 2 KB zero-fill per cell). We now clear only the class
        // slots we actually touched (`seen`) between cells — typically
        // 2-5 writes instead of 256.
        let mut votes = [0.0f64; 256];
        let mut seen: [u8; 16] = [0; 16];
        let mut seen_len = 0usize;

        for x in 0..width {
            let center_class = snapshot[y][x];
            if center_class == 0 {
                continue;
            }

            // Skip unless this cell is on a class boundary. Everything inside
            // a flood-fill of one class has identical votes across the cell
            // and all its neighbors, so it would win itself — expensive no-op.
            let mut is_boundary = false;
            for (dx, dz) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let nx = x as i32 + dx;
                let nz = y as i32 + dz;
                if nx < 0 || nz < 0 || nx >= width as i32 || nz >= height as i32 {
                    continue;
                }
                let nc = snapshot[nz as usize][nx as usize];
                if nc != 0 && nc != center_class {
                    is_boundary = true;
                    break;
                }
            }
            if !is_boundary {
                continue;
            }

            // Reset only the slots touched by the previous boundary cell
            // on this row. `seen` maxes at 16 classes — ESA has ~11 —
            // so this is a handful of writes, not a 256-entry memset.
            for i in 0..seen_len {
                votes[seen[i] as usize] = 0.0;
            }
            seen_len = 0;

            for ky in 0..kernel_size {
                let nz = y as i32 + ky as i32 - radius;
                if nz < 0 || nz >= height as i32 {
                    continue;
                }
                let kernel_row = ky * kernel_size;
                let src_row = &snapshot[nz as usize];
                for kx in 0..kernel_size {
                    let nx = x as i32 + kx as i32 - radius;
                    if nx < 0 || nx >= width as i32 {
                        continue;
                    }
                    let nc = src_row[nx as usize];
                    if nc == 0 {
                        continue;
                    }
                    let prev = votes[nc as usize];
                    votes[nc as usize] = prev + kernel[kernel_row + kx];
                    // Track the class code on first contribution only.
                    // `seen` maxes at 16 classes — ESA has ~11 defined —
                    // so we never overflow in practice.
                    if prev == 0.0 && seen_len < seen.len() {
                        seen[seen_len] = nc;
                        seen_len += 1;
                    }
                }
            }

            // Find the max over only the class codes actually seen in
            // the neighbourhood (typically 2-5 at a class boundary)
            // rather than scanning all 256 entries.
            let mut best_idx = 0u8;
            let mut best_val = 0.0f64;
            for &cls in &seen[..seen_len] {
                let v = votes[cls as usize];
                if v > best_val {
                    best_idx = cls;
                    best_val = v;
                }
            }
            if best_val > 0.0 {
                row[x] = best_idx;
            }
        }
    });
}

/// Simple deterministic hash from coordinates (for dithering and block variety).
pub fn coord_hash(x: i32, z: i32) -> u64 {
    let mut h = (x as u32 as u64).wrapping_mul(0x9E3779B97F4A7C15);
    h ^= (z as u32 as u64).wrapping_mul(0x517CC1B727220A95);
    h = h.wrapping_mul(0x6C62272E07BB0142);
    h ^ (h >> 32)
}

// ─── LZW decompression ───────────────────────────────────────────────────

/// Simple LZW decompressor for TIFF (variable-length codes, MSB packing).
fn lzw_decompress(
    data: &[u8],
    expected_size: usize,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // TIFF LZW uses MSB-first bit packing with min code size of 8
    let min_code_size: u32 = 8;
    let clear_code: u32 = 1 << min_code_size; // 256
    let eoi_code: u32 = clear_code + 1; // 257

    let mut output = Vec::with_capacity(expected_size);
    let mut code_size: u32 = min_code_size + 1;
    let mut bit_pos: usize = 0;

    // Initialize table with single-byte entries
    let init_table = || {
        let mut t: Vec<Vec<u8>> = Vec::with_capacity(4096);
        for i in 0..=255u16 {
            t.push(vec![i as u8]);
        }
        t.push(Vec::new()); // clear code
        t.push(Vec::new()); // EOI
        t
    };

    let mut table = init_table();
    let mut prev_entry: Option<Vec<u8>> = None;

    loop {
        // Read next code (MSB-first)
        let code = read_bits_msb(data, bit_pos, code_size as usize);
        bit_pos += code_size as usize;

        if bit_pos > data.len() * 8 + code_size as usize {
            break;
        }

        if code == clear_code {
            table = init_table();
            code_size = min_code_size + 1;
            prev_entry = None;
            continue;
        }

        if code == eoi_code || output.len() >= expected_size {
            break;
        }

        let entry = if (code as usize) < table.len() {
            table[code as usize].clone()
        } else if code as usize == table.len() {
            // Special case: code not yet in table
            if let Some(ref prev) = prev_entry {
                let mut e = prev.clone();
                e.push(prev[0]);
                e
            } else {
                break;
            }
        } else {
            break; // Invalid code
        };

        output.extend_from_slice(&entry);

        if let Some(ref prev) = prev_entry {
            let mut new_entry = prev.clone();
            new_entry.push(entry[0]);
            if table.len() < 4096 {
                table.push(new_entry);
            }
            // Increase code size when table reaches power of 2
            if table.len() == (1 << code_size) as usize && code_size < 12 {
                code_size += 1;
            }
        }

        prev_entry = Some(entry);
    }

    output.truncate(expected_size);
    Ok(output)
}

/// Read `n` bits from a byte array starting at `bit_offset`, MSB-first (TIFF convention).
fn read_bits_msb(data: &[u8], bit_offset: usize, n: usize) -> u32 {
    let mut result: u32 = 0;
    for i in 0..n {
        let byte_idx = (bit_offset + i) / 8;
        let bit_idx = 7 - ((bit_offset + i) % 8); // MSB first
        if byte_idx < data.len() && (data[byte_idx] >> bit_idx) & 1 != 0 {
            result |= 1 << (n - 1 - i);
        }
    }
    result
}

#[cfg(test)]
mod smoothing_scale_tests {
    use super::*;

    /// Area the smoothing rounds off a square's four corners, in square metres. Area rather
    /// than a diagonal walk: it grows with sigma squared, so it separates the resolutions
    /// instead of quantising down to a cell or two.
    fn corner_cut_area_m2(side_m: usize, cells_per_meter: f64) -> f64 {
        let side = (side_m as f64 * cells_per_meter).round() as usize;
        let pad = side;
        let dim = side + pad * 2;
        let mut grid = vec![vec![LC_GRASSLAND; dim]; dim];
        for row in grid.iter_mut().skip(pad).take(side) {
            for cell in row.iter_mut().skip(pad).take(side) {
                *cell = LC_TREE_COVER;
            }
        }
        smooth_class_boundaries(&mut grid, dim, dim, cells_per_meter);
        let remaining = grid
            .iter()
            .flatten()
            .filter(|&&c| c == LC_TREE_COVER)
            .count();
        let cut_cells = (side * side).saturating_sub(remaining) as f64;
        cut_cells / (cells_per_meter * cells_per_meter)
    }

    #[test]
    fn corner_smoothing_reach_is_scale_invariant() {
        // The same 20 m square sampled at 1 and at 2 cells per metre. ESA's steps are a fixed
        // real-world size, so the smoothing must round the corners off by the same real AREA at
        // both resolutions. Sizing the kernel in cells alone quarters that area at scale 2.0,
        // leaving the raw rectangular patches standing.
        let coarse = corner_cut_area_m2(20, 1.0);
        let fine = corner_cut_area_m2(20, 2.0);
        assert!(coarse > 0.0, "the corners must be rounded at all");
        let ratio = fine / coarse;
        assert!(
            (0.75..=1.33).contains(&ratio),
            "corner rounding must agree in m^2: {coarse} at 1 cell/m vs {fine} at 2 cells/m"
        );
    }

    #[test]
    fn coarse_grids_keep_the_cell_sized_kernel() {
        // Below one cell per metre (country scale) each cell already spans many blocks, so the
        // kernel stays at its cell-based minimum rather than collapsing toward zero.
        assert!(
            corner_cut_area_m2(400, 0.1) > 0.0,
            "a coarse grid must still round the corners"
        );
    }
}
