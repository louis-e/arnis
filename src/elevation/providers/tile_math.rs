//! Helpers shared by the XYZ tile providers (Mapterhorn, AWS).

use crate::coordinate_system::geographic::LLBBox;

/// Tile budget per fetch, shared by the zoom choice in `aws_terrain` and
/// `mapterhorn`; the zoom is coarsened until the covering count fits.
pub(super) const MAX_TILES_PER_FETCH: usize = 2048;

/// Earth radius used by the Web Mercator projection (EPSG:3857).
const EARTH_RADIUS_M: f64 = 6_378_137.0;

/// Offset of the Terrarium encoding, in metres.
pub(super) const TERRARIUM_OFFSET: f64 = 32768.0;

/// Height in metres of a Terrarium-encoded pixel.
#[inline]
pub(super) fn decode_terrarium(pixel: &image::Rgb<u8>) -> f64 {
    (pixel[0] as f64 * 256.0 + pixel[1] as f64 + pixel[2] as f64 / 256.0) - TERRARIUM_OFFSET
}

/// Position `global` (in pixels) within the tile starting at `origin`. Only past the tile's
/// far edge, where a clamped tile index leaves it at the world edge, is it held on the last
/// pixel centre; anywhere inside, taps beyond the edge come from the neighbour tile.
#[inline]
pub(super) fn in_tile(global: f64, origin: f64, tile_px: u32) -> f64 {
    let local = global - origin;
    if local >= tile_px as f64 {
        tile_px as f64 - 0.5
    } else {
        local
    }
}

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

/// Tile pixels coarser than this many grid cells are B-spline sampled, so their grid
/// doesn't show as creases and boxy terraces.
pub(super) const SMOOTH_PIXEL_CELLS: f64 = 1.5;

/// Grid cell size in metres along the finer axis.
pub(super) fn grid_cell_m(bbox: &LLBBox, grid_width: usize, grid_height: usize) -> f64 {
    let (w_m, h_m) = bbox_dimensions_m(bbox);
    let cell_x = w_m / (grid_width.saturating_sub(1)).max(1) as f64;
    let cell_y = h_m / (grid_height.saturating_sub(1)).max(1) as f64;
    cell_x.min(cell_y).max(0.05)
}

/// Ground size of one pixel of a `tile_px` wide tile at `zoom`, at the bbox's middle latitude.
pub(super) fn pixel_m(bbox: &LLBBox, zoom: u8, tile_px: u32) -> f64 {
    let mid_lat = (bbox.min().lat() + bbox.max().lat()) * 0.5;
    let circumference = 2.0 * std::f64::consts::PI * EARTH_RADIUS_M;
    circumference * mid_lat.to_radians().cos().abs() / (tile_px as f64 * 2f64.powi(zoom as i32))
}

/// Uniform cubic B-spline weights for the taps at -1, 0, 1 and 2 around `t` in [0, 1].
fn bspline_weights(t: f64) -> [f64; 4] {
    let s = 1.0 - t;
    let t2 = t * t;
    let t3 = t2 * t;
    [
        s * s * s / 6.0,
        (3.0 * t3 - 6.0 * t2 + 4.0) / 6.0,
        (-3.0 * t3 + 3.0 * t2 + 3.0 * t + 1.0) / 6.0,
        t3 / 6.0,
    ]
}

/// Height at pixel position (`px`, `py`), with each pixel's value at its centre.
/// Cubic B-spline over the taps that exist when `smooth`, else bilinear over the
/// nearest four that do. `pixel` returns NaN where there is no data.
#[inline]
pub(super) fn sample_pixels(
    px: f64,
    py: f64,
    smooth: bool,
    pixel: impl Fn(i32, i32) -> f64,
) -> f64 {
    let (px, py) = (px - 0.5, py - 0.5);
    let (fx, fy) = (px.floor(), py.floor());
    let (dx, dy) = (px - fx, py - fy);
    let (x0, y0) = (fx as i32, fy as i32);
    if smooth {
        // Renormalizing keeps it continuous: a tap enters the window with zero weight.
        let (wx, wy) = (bspline_weights(dx), bspline_weights(dy));
        let (mut sum, mut weight) = (0.0, 0.0);
        for (j, w_y) in wy.iter().enumerate() {
            let y = y0 - 1 + j as i32;
            for (i, w_x) in wx.iter().enumerate() {
                let v = pixel(x0 - 1 + i as i32, y);
                if v.is_finite() {
                    sum += w_x * w_y * v;
                    weight += w_x * w_y;
                }
            }
        }
        if weight > 0.0 {
            return sum / weight;
        }
    }
    blend_finite_samples(
        pixel(x0, y0),
        pixel(x0 + 1, y0),
        pixel(x0, y0 + 1),
        pixel(x0 + 1, y0 + 1),
        dx,
        dy,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_tile_runs_up_to_the_edge_and_holds_only_past_it() {
        assert_eq!(in_tile(1023.9, 512.0, 512), 511.9);
        assert_eq!(in_tile(1024.0, 512.0, 512), 511.5);
        assert_eq!(in_tile(512.0, 512.0, 512), 0.0);
    }

    fn ramp(x: i32, y: i32) -> f64 {
        3.0 * x as f64 - 2.0 * y as f64 + 7.0
    }

    #[test]
    fn pixel_values_sit_at_pixel_centres() {
        for smooth in [false, true] {
            let v = sample_pixels(10.5, 20.5, smooth, ramp);
            assert!((v - ramp(10, 20)).abs() < 1e-9, "smooth {smooth}: {v}");
            // Halfway between two centres is their mean.
            let v = sample_pixels(11.0, 20.5, smooth, ramp);
            assert!((v - (ramp(10, 20) + ramp(11, 20)) / 2.0).abs() < 1e-9);
        }
    }

    #[test]
    fn bspline_keeps_a_plane_and_rounds_a_crease() {
        // A plane is reproduced exactly, so smoothing never tilts a slope.
        for (x, y) in [(3.2, 4.7), (100.01, 0.99), (-5.5, 2.25)] {
            let (b, s) = (
                sample_pixels(x, y, false, ramp),
                sample_pixels(x, y, true, ramp),
            );
            assert!((b - s).abs() < 1e-9, "{b} vs {s}");
        }
        // A V-shaped valley with its kink on the centre of pixel 0.
        let valley = |x: i32, _: i32| f64::from(x.abs());
        let kink = |smooth: bool| {
            let h = 1e-3;
            let at = |x: f64| sample_pixels(x, 0.5, smooth, valley);
            ((at(0.5 + h) - at(0.5)) - (at(0.5) - at(0.5 - h))) / h
        };
        assert!(
            (kink(false) - 2.0).abs() < 1e-6,
            "bilinear keeps the crease"
        );
        assert!(
            kink(true).abs() < 0.01,
            "spline rounds it, got {}",
            kink(true)
        );
    }

    #[test]
    fn missing_taps_fall_back_to_the_ones_present() {
        let holey = |x: i32, y: i32| if x < 0 { f64::NAN } else { ramp(x, y) };
        // The tap at x = -1 is left out and the other weights renormalized.
        let (wx, wy) = (bspline_weights(0.25), bspline_weights(0.0));
        let mut expected = 0.0;
        for (j, w_y) in wy.iter().enumerate() {
            for (i, w_x) in wx.iter().enumerate().skip(1) {
                expected += w_x * w_y * ramp(i as i32 - 1, j as i32 - 1);
            }
        }
        expected /= wx[1] + wx[2] + wx[3];
        let v = sample_pixels(0.75, 0.5, true, holey);
        assert!((v - expected).abs() < 1e-9, "got {v}, want {expected}");
        assert!(sample_pixels(0.5, 0.5, true, |_, _| f64::NAN).is_nan());
    }

    #[test]
    fn spline_has_no_step_where_a_tap_goes_missing() {
        // Curved, so the spline and bilinear differ. Data ends after column 3, so the
        // window drops a tap once px reaches 2.5.
        let bowl = |x: i32, y: i32| {
            if x > 3 {
                f64::NAN
            } else {
                f64::from(x * x + x * y + y * y)
            }
        };
        for py in [0.5, 1.3, 2.8] {
            let before = sample_pixels(2.5 - 1e-9, py, true, bowl);
            let after = sample_pixels(2.5, py, true, bowl);
            assert!(
                (before - after).abs() < 1e-6,
                "py {py}: {before} vs {after}"
            );
            let bilinear = sample_pixels(2.5, py, false, bowl);
            assert!(
                (after - bilinear).abs() > 0.1,
                "surface is too flat to tell"
            );
        }
    }

    #[test]
    fn blend_finite_samples_is_nan_aware() {
        let v = blend_finite_samples(10.0, f64::NAN, 30.0, 40.0, 0.5, 0.5);
        assert!((v - 80.0 / 3.0).abs() < 1e-9);
        assert!(blend_finite_samples(f64::NAN, f64::NAN, f64::NAN, f64::NAN, 0.5, 0.5).is_nan());
    }
}
