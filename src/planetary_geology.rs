//! Compact, deterministic geology overlays derived from global USGS planetary maps.

use crate::celestial::CelestialBody;
use flate2::read::GzDecoder;
use std::io::Read;
use std::sync::LazyLock;

const COLS: usize = 3600;
const ROWS: usize = 1800;
const CELL_DEGREES: f64 = 0.1;
const CELL_BYTES: usize = 2;
const GRID_BYTES: &[u8] = include_bytes!("../assets/planetary/features.grid.gz");

pub const MOON_MARE: u16 = 1 << 0;
pub const MOON_RILLE: u16 = 1 << 1;
pub const MOON_CRATER_RIM: u16 = 1 << 2;
pub const MOON_RIDGE: u16 = 1 << 3;

pub const MARS_VOLCANIC: u16 = 1 << 4;
pub const MARS_DUNE: u16 = 1 << 5;
pub const MARS_POLAR_CAP: u16 = 1 << 6;
pub const MARS_POLAR_LAYER: u16 = 1 << 7;
pub const MARS_FLUVIAL: u16 = 1 << 8;
pub const MARS_RILLE: u16 = 1 << 9;

const MOON_MASK: u16 = MOON_MARE | MOON_RILLE | MOON_CRATER_RIM | MOON_RIDGE;
const MARS_MASK: u16 = MARS_VOLCANIC
    | MARS_DUNE
    | MARS_POLAR_CAP
    | MARS_POLAR_LAYER
    | MARS_FLUVIAL
    | MARS_RILLE;

static GRID: LazyLock<Option<Box<[u8]>>> = LazyLock::new(|| {
    let mut decoder = GzDecoder::new(GRID_BYTES);
    let mut grid = Vec::with_capacity(COLS * ROWS * CELL_BYTES);
    decoder.read_to_end(&mut grid).ok()?;
    (grid.len() == COLS * ROWS * CELL_BYTES).then(|| grid.into_boxed_slice())
});

/// Return mapped feature flags for a body's geographic point. The grid is
/// sampled at cell centers; absent or malformed data safely gives no overlay.
#[inline]
pub fn features_at(body: CelestialBody, lat: f64, lon: f64) -> u16 {
    let body_mask = match body {
        CelestialBody::Earth => return 0,
        CelestialBody::Moon => MOON_MASK,
        CelestialBody::Mars => MARS_MASK,
    };
    let mut flags = flags_from_grid(GRID.as_deref(), lat, lon) & body_mask;
    if body == CelestialBody::Mars {
        // MGD3's mapped dune fields are complete only through 65° latitude;
        // MOLA terrain used here ends at 88° latitude.
        if lat.abs() > 65.0 {
            flags &= !MARS_DUNE;
        }
        if lat.abs() > 88.0 {
            flags &= !(MARS_POLAR_CAP | MARS_POLAR_LAYER);
        }
    }
    flags
}

fn flags_from_grid(grid: Option<&[u8]>, lat: f64, lon: f64) -> u16 {
    let Some(grid) = grid.filter(|grid| grid.len() == COLS * ROWS * CELL_BYTES) else {
        return 0;
    };
    if !lat.is_finite() || !lon.is_finite() || !(-90.0..=90.0).contains(&lat) {
        return 0;
    }

    let wrapped_lon = (lon + 180.0).rem_euclid(360.0);
    let col = ((wrapped_lon / CELL_DEGREES).floor() as usize).min(COLS - 1);
    let row = (((90.0 - lat) / CELL_DEGREES).floor() as usize).min(ROWS - 1);
    let offset = (row * COLS + col) * CELL_BYTES;
    u16::from_le_bytes([grid[offset], grid[offset + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_grid_loads_and_classifies_mapped_locations() {
        assert!(GRID.is_some());
        assert_ne!(features_at(CelestialBody::Moon, 17.0, 59.0) & MOON_MARE, 0);
        assert_ne!(
            features_at(CelestialBody::Mars, 18.65, -133.8) & MARS_VOLCANIC,
            0
        );
        assert_ne!(
            features_at(CelestialBody::Mars, 8.7, 67.3) & MARS_DUNE,
            0
        );
        assert_ne!(
            features_at(CelestialBody::Mars, 87.0, 0.0) & MARS_POLAR_CAP,
            0
        );
    }

    #[test]
    fn feature_boundaries_and_unmapped_areas_fall_back_cleanly() {
        assert_eq!(features_at(CelestialBody::Moon, 17.0, 59.0) & MOON_MARE, MOON_MARE);
        assert_eq!(features_at(CelestialBody::Moon, 10.0, 59.0) & MOON_MARE, 0);
        assert_eq!(features_at(CelestialBody::Moon, -43.3, -11.2) & MOON_MARE, 0);
        assert_eq!(features_at(CelestialBody::Mars, 70.0, 0.0) & MARS_POLAR_CAP, 0);
        assert_eq!(features_at(CelestialBody::Mars, 65.05, -8.65) & MARS_DUNE, 0);
        assert_eq!(features_at(CelestialBody::Mars, 89.95, -179.95) & MARS_POLAR_CAP, 0);
        assert_eq!(features_at(CelestialBody::Earth, 18.65, -133.8), 0);
    }

    #[test]
    fn missing_or_invalid_grid_returns_no_features() {
        assert_eq!(flags_from_grid(None, 0.0, 0.0), 0);
        assert_eq!(flags_from_grid(Some(&[0, 1]), 0.0, 0.0), 0);
        assert_eq!(flags_from_grid(GRID.as_deref(), f64::NAN, 0.0), 0);
        assert_eq!(flags_from_grid(GRID.as_deref(), 0.0, f64::INFINITY), 0);
    }

    #[test]
    fn longitude_wraps_at_the_dateline() {
        assert_eq!(
            flags_from_grid(GRID.as_deref(), 89.95, -180.0),
            flags_from_grid(GRID.as_deref(), 89.95, 180.0)
        );
    }
}

