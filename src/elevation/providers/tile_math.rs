//! Helpers shared by the XYZ tile providers (Mapterhorn, AWS).

use crate::coordinate_system::geographic::LLBBox;

/// Tile budget per fetch, shared by the zoom choice in `aws_terrain` and
/// `mapterhorn`; the zoom is coarsened until the covering count fits.
pub(super) const MAX_TILES_PER_FETCH: usize = 2048;

/// Earth radius used by the Web Mercator projection (EPSG:3857).
const EARTH_RADIUS_M: f64 = 6_378_137.0;

/// Approximate physical bbox dimensions in meters. Precise enough for
/// resolution-level selection.
pub(super) fn bbox_dimensions_m(bbox: &LLBBox) -> (f64, f64) {
    let mid_lat = (bbox.min().lat() + bbox.max().lat()) * 0.5;
    let mid_lat_cos = mid_lat.to_radians().cos().abs().max(1e-6);
    let width_deg = bbox.max().lng() - bbox.min().lng();
    let height_deg = bbox.max().lat() - bbox.min().lat();
    let width_m = width_deg.to_radians() * EARTH_RADIUS_M * mid_lat_cos;
    let height_m = height_deg.to_radians() * EARTH_RADIUS_M;
    (width_m.abs(), height_m.abs())
}

/// Bilinear blend of four samples that skips non-finite ones, so a nodata
/// neighbour doesn't turn the whole cell into NaN.
pub(super) fn blend_finite_samples(
    v00: f64,
    v10: f64,
    v01: f64,
    v11: f64,
    dx: f64,
    dy: f64,
) -> f64 {
    let w00 = (1.0 - dx) * (1.0 - dy);
    let w10 = dx * (1.0 - dy);
    let w01 = (1.0 - dx) * dy;
    let w11 = dx * dy;
    let mut sum = 0.0;
    let mut weight = 0.0;
    for (v, w) in [(v00, w00), (v10, w10), (v01, w01), (v11, w11)] {
        if v.is_finite() {
            sum += v * w;
            weight += w;
        }
    }
    if weight <= 0.0 {
        f64::NAN
    } else {
        sum / weight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_finite_samples_is_nan_aware() {
        let v = blend_finite_samples(10.0, f64::NAN, 30.0, 40.0, 0.5, 0.5);
        assert!((v - 80.0 / 3.0).abs() < 1e-9);
        assert!(blend_finite_samples(f64::NAN, f64::NAN, f64::NAN, f64::NAN, 0.5, 0.5).is_nan());
    }
}
