//! IGN France RGE ALTI elevation provider.
//!
//! Uses the same **fixed global Web Mercator tile grid** as
//! [`super::usgs_3dep`] — see [`super::fixed_tile`] for the rationale.
//! The short version: RGE ALTI is assembled from multiple source
//! surveys (airborne LiDAR, aerial photogrammetry, radar) with per-
//! campaign vertical calibrations, so adjacent *user-bbox-relative*
//! sub-requests that crossed a campaign boundary would disagree at
//! their shared edges by tens to hundreds of metres — the same class
//! of artefact we confirmed on USGS 3DEP. Anchoring every request to
//! a fixed tile grid keeps tile boundaries stable across users and
//! sessions and shrinks each request to ~500 m per edge (at M1),
//! which usually fits within a single source campaign.
//!
//! # Upstream specifics
//!
//! - WMS 1.3.0 endpoint at `data.geopf.fr/wms-r/wms`.
//! - Layer: `ELEVATION.ELEVATIONGRIDCOVERAGE`.
//! - Requests use `CRS=EPSG:3857` so the tile bbox fed to the server
//!   matches our fixed Mercator grid 1-to-1 (no reprojection drift
//!   across the seam).
//! - Response format: GeoTIFF, float32.
//! - Resolution ladder mirrors USGS's (1 / 3 / 10 / 30 m per px) so
//!   the level-selection logic is shared and the user-scale mapping
//!   is uniform across providers. IGN's native products include 1 m
//!   (metropolitan FR), 5 m (overseas), and 25 m — the WMS service
//!   upsamples/downsamples on request, so we just pick the resolution
//!   that matches the output cell size.
//!
//! All the tile-grid mechanics live in [`super::fixed_tile`]; this
//! module contributes the WMS URL template, the coverage list (metro
//! France + overseas territories) and the `ElevationProvider` impl.

use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};

use super::fixed_tile::{
    fetch_fixed_tile_grid, FixedTileProvider, Resolution as ResolutionTrait, TileKey, TILE_PIXELS,
};

// `pub(super)` for the same reason as the USGS provider's enum: the
// shared `FixedTileProvider::Level` associated type has to stay at
// least as visible as the trait itself.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum Resolution {
    /// 1.0 m/px — matches RGE ALTI native resolution for metropolitan France.
    M1,
    /// ~3.44 m/px — covers the 5 m overseas product via light downsampling.
    M3,
    /// ~10.31 m/px — regional-scale fallback.
    M10,
    /// ~30.92 m/px — very-large-area fallback.
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

/// IGN France RGE ALTI — France + overseas territories.
/// Resolution: 1m metropolitan France, 1-5m overseas (server upsampled).
/// License: Licence Ouverte 2.0.
pub struct IgnFrance;

impl FixedTileProvider for IgnFrance {
    type Level = Resolution;

    const CACHE_NAME: &'static str = "ign_france";

    fn resolution_levels(&self) -> &'static [Self::Level] {
        LEVELS
    }

    fn tile_url(&self, key: &TileKey<Self::Level>) -> String {
        // WMS 1.3.0 with EPSG:3857: axis order is X,Y (east, north)
        // — same as our TileKey's mercator bbox. `STYLES=` must be
        // present (empty is fine) or the server returns HTTP 400.
        format!(
            "https://data.geopf.fr/wms-r/wms\
             ?SERVICE=WMS&REQUEST=GetMap&VERSION=1.3.0\
             &LAYERS=ELEVATION.ELEVATIONGRIDCOVERAGE\
             &STYLES=\
             &CRS=EPSG:3857\
             &BBOX={:.6},{:.6},{:.6},{:.6}\
             &WIDTH={}&HEIGHT={}\
             &FORMAT=image/geotiff",
            key.min_mx(),
            key.min_my(),
            key.max_mx(),
            key.max_my(),
            TILE_PIXELS,
            TILE_PIXELS,
        )
    }
}

impl ElevationProvider for IgnFrance {
    fn name(&self) -> &'static str {
        Self::CACHE_NAME
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
        fetch_fixed_tile_grid(self, bbox, grid_width, grid_height)
    }
}
