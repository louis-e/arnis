//! IGN España MDT elevation provider.
//!
//! Uses the same **fixed global Web Mercator tile grid** as the other
//! `tiled_fetch`-legacy providers — see [`super::fixed_tile`] for the
//! full rationale. Same class of risk as USGS/IGN-France: MDT is
//! built from multiple source surveys, and `tiled_fetch`-style ad-hoc
//! bbox splits would have exposed per-campaign vertical offsets at
//! user-bbox-relative seam positions. Fixed tile boundaries fix that
//! by construction.
//!
//! # Upstream specifics
//!
//! - WCS 2.0.1 endpoint at `servicios.idee.es/wcs-inspire/mdt`.
//! - Coverage: `Elevacion4258_5` (5 m MDT), axes `Long` / `Lat` in
//!   EPSG:4258. ETRS89 and WGS84 are close enough that pixel spacing
//!   on a ~500 m tile at Spain's latitudes differs by ≪ 1 m.
//! - Requests use `SUBSET=Long(…)` and `SUBSET=Lat(…)`, where the
//!   subset range comes from reprojecting the Mercator tile's corners
//!   to lat/lng. The tile *grid* stays Mercator-aligned (cacheable
//!   globally), only the request-CRS is lat/lng.
//! - Response format: GeoTIFF, int16 (decoded to f64 by the shared
//!   TIFF loader).
//! - Resolution ladder mirrors USGS's so level selection is uniform
//!   across providers. MDT's native is 5 m for the MDT05 product; we
//!   still advertise an M1 level because the server will upsample to
//!   whatever pixel size we request, and going finer-than-native is
//!   harmless (bilinear fills in smoothly, no new artefacts).
//!
//! All tile mechanics live in [`super::fixed_tile`]; this module
//! contributes the WCS URL template, the Spain coverage list and the
//! `ElevationProvider` impl.

use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};

use super::fixed_tile::{
    fetch_fixed_tile_grid, mercator_x_to_lon, mercator_y_to_lat, FixedTileProvider,
    Resolution as ResolutionTrait, TileKey, TILE_PIXELS,
};

// `pub(super)` for the same reason as the USGS provider's enum: the
// shared `FixedTileProvider::Level` associated type has to stay at
// least as visible as the trait itself.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum Resolution {
    /// 1.0 m/px — server-upsampled from MDT05 (5 m native). Useful
    /// when the user picks a scale that wants sub-native density.
    M1,
    /// ~3.44 m/px — close to native for MDT05 via mild downsampling.
    M3,
    /// ~10.31 m/px — regional-scale.
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

/// IGN España MDT — Spain + Canary Islands + Balearic Islands.
/// Resolution: 5m (MDT05).
/// License: CC BY 4.0.
pub struct IgnSpain;

impl FixedTileProvider for IgnSpain {
    type Level = Resolution;

    const CACHE_NAME: &'static str = "ign_spain";

    fn resolution_levels(&self) -> &'static [Self::Level] {
        LEVELS
    }

    fn tile_url(&self, key: &TileKey<Self::Level>) -> String {
        // WCS 2.0.1 with the coverage's native EPSG:4258 axes. We
        // reproject the Mercator tile's corners to lat/lng here; the
        // tile *grid* is still Mercator (so cache files stay keyed
        // consistently), only the request bbox is expressed in the
        // server's native CRS.
        let min_lng = mercator_x_to_lon(key.min_mx());
        let max_lng = mercator_x_to_lon(key.max_mx());
        let min_lat = mercator_y_to_lat(key.min_my());
        let max_lat = mercator_y_to_lat(key.max_my());
        format!(
            "https://servicios.idee.es/wcs-inspire/mdt\
             ?SERVICE=WCS&VERSION=2.0.1&REQUEST=GetCoverage\
             &COVERAGEID=Elevacion4258_5\
             &SUBSET=Long({:.6},{:.6})\
             &SUBSET=Lat({:.6},{:.6})\
             &FORMAT=image/tiff\
             &SCALESIZE=Long({}),Lat({})",
            min_lng, max_lng, min_lat, max_lat, TILE_PIXELS, TILE_PIXELS,
        )
    }
}

impl ElevationProvider for IgnSpain {
    fn name(&self) -> &'static str {
        Self::CACHE_NAME
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        Some(vec![
            // Iberian Peninsula
            LLBBox::new(35.9, -9.5, 43.9, 3.3).unwrap(),
            // Balearic Islands
            LLBBox::new(38.6, 1.2, 40.1, 4.3).unwrap(),
            // Canary Islands
            LLBBox::new(27.6, -18.2, 29.5, -13.4).unwrap(),
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
        fetch_fixed_tile_grid(self, bbox, grid_width, grid_height)
    }
}
