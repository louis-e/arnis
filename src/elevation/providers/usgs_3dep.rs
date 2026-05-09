//! USGS 3DEP elevation provider using a **fixed global Web Mercator tile
//! grid**.
//!
//! See [`super::fixed_tile`] for the rationale behind fixed tiles (vs
//! the ad-hoc bbox splits `tiled_fetch` used to do) — short version:
//! USGS's ArcGIS ImageServer composites multiple LiDAR flights with
//! different vertical datums server-side, and adjacent *user-bbox-
//! relative* sub-requests disagree at their boundaries by 10–500 m.
//! Anchoring every request to a fixed global tile grid means two users
//! over the same area hit the same tile URLs, the same cached bytes,
//! and the same data — the way Tellus's `Usgs3depElevationSource` and
//! the standard slippy-map pattern already do.
//!
//! All the heavy lifting lives in [`super::fixed_tile`]; this module
//! contributes the USGS-specific pieces:
//! - the ArcGIS ImageServer URL template (EPSG:3857, F32 TIFF),
//! - the four native resolution levels (1 m / 3 m / 10 m / 30 m per px),
//! - USGS coverage bboxes (CONUS, AK, HI, PR/USVI, Guam),
//! - the `ElevationProvider` impl wiring.

use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};

use super::fixed_tile::{
    fetch_fixed_tile_grid, FixedTileProvider, Resolution as ResolutionTrait, TileKey, TILE_PIXELS,
};

/// USGS 3DEP's published zoom levels, each covering a fixed
/// meters-per-pixel. The values match the Tellus reference
/// implementation so disk-cache entries stay compatible across
/// projects that happen to share a cache root.
// Visibility: `pub(super)` because the shared `FixedTileProvider` trait
// (in `super::fixed_tile`) exposes this as its associated `Level` type —
// anything `impl`ing a `pub(super)` trait has to keep that associated
// type at least as visible. The module boundary above `providers/`
// still keeps the enum private to the elevation subsystem.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum Resolution {
    /// 1.0 m/px — best available, covers most CONUS via LiDAR.
    M1,
    /// ~3.44 m/px — common LiDAR fallback.
    M3,
    /// ~10.31 m/px — regional DEM coverage.
    M10,
    /// ~30.92 m/px — national coarse fallback.
    M30,
}

const LEVELS: &[Resolution] = &[
    Resolution::M1,
    Resolution::M3,
    Resolution::M10,
    Resolution::M30,
];

impl ResolutionTrait for Resolution {
    fn level_id(&self) -> &'static str {
        match self {
            Self::M1 => "r1",
            Self::M3 => "r3",
            Self::M10 => "r10",
            Self::M30 => "r30",
        }
    }

    fn meters_per_pixel(&self) -> f64 {
        match self {
            Self::M1 => 1.0,
            Self::M3 => 3.435_973_836_8,
            Self::M10 => 10.307_921_510_4,
            Self::M30 => 30.922_080_981_4,
        }
    }
}

/// USGS 3D Elevation Program (3DEP) — USA + territories.
/// Resolution: up to 1m LiDAR (CONUS), 3m/10m elsewhere, fallback 30m.
/// License: Public Domain (USGS).
pub struct Usgs3dep;

impl FixedTileProvider for Usgs3dep {
    type Level = Resolution;

    const CACHE_NAME: &'static str = "usgs_3dep";

    fn resolution_levels(&self) -> &'static [Self::Level] {
        LEVELS
    }

    fn tile_url(&self, key: &TileKey<Self::Level>) -> String {
        // ArcGIS ImageServer `exportImage` with mercator-native bbox so
        // the tile payload always corresponds to the same real-world
        // area regardless of who's asking.
        format!(
            "https://elevation.nationalmap.gov/arcgis/rest/services/3DEPElevation/ImageServer/exportImage\
             ?bbox={:.6},{:.6},{:.6},{:.6}\
             &bboxSR=3857&imageSR=3857\
             &size={},{}\
             &format=tiff&pixelType=F32\
             &interpolation=RSP_BilinearInterpolation\
             &f=image",
            key.min_mx(),
            key.min_my(),
            key.max_mx(),
            key.max_my(),
            TILE_PIXELS,
            TILE_PIXELS,
        )
    }
}

impl ElevationProvider for Usgs3dep {
    fn name(&self) -> &'static str {
        Self::CACHE_NAME
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
        fetch_fixed_tile_grid(self, bbox, grid_width, grid_height)
    }
}
