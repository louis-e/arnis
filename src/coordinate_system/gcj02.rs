//! WGS-84 ↔ GCJ-02 coordinate conversion.
//!
//! GCJ-02 ("Mars Coordinates") is used by Chinese map providers and is offset
//! from WGS-84 by 100–700 m depending on location. This module implements the
//! standard forward and inverse transformations so that Chinese data sources
//! (e.g. Tianditu elevation tiles) can feed into the Arnis pipeline correctly.
//!
//! The forward transform (WGS-84 → GCJ-02) is deterministic; the inverse
//! (GCJ-02 → WGS-84) is solved via fixed-point iteration (typically
//! converges in 2–3 steps to <0.1 m).

use crate::coordinate_system::geographic::{LLBBox, LLPoint};

/// Earth semi-major axis (WGS-84 / CGCS 2000), metres.
const A: f64 = 6_378_245.0;
/// Eccentricity squared.
const EE: f64 = 0.00669342162296594323;

// ─── Out-of-China guard ─────────────────────────────────────────────────

/// Returns true if (lng, lat) is outside China's land territory.
/// Coordinates outside China should not be offset.
fn out_of_china(lng: f64, lat: f64) -> bool {
    !(72.004..=137.8347).contains(&lng) || !(0.8293..=55.8271).contains(&lat)
}

/// Convert degrees to radians and take the sine.
#[inline]
fn sin_deg(deg: f64) -> f64 {
    (deg * std::f64::consts::PI).sin()
}

// ─── Forward transform ──────────────────────────────────────────────────

/// Compute the GCJ-02 offset (dlng, dlat) for a WGS-84 coordinate.
/// Based on the standard empirical formula.
fn _delta(lng: f64, lat: f64) -> (f64, f64) {
    let d = lng - 105.0;
    let e = lat - 35.0;

    let rad_lat = lat.to_radians();
    let sin_lat = rad_lat.sin();
    let magic = 1.0 - EE * sin_lat * sin_lat;
    let sqrt_magic = magic.sqrt();
    let a_mul = A / sqrt_magic * rad_lat.cos();

    let dlng = (300.0 + d + 2.0 * e + 0.1 * d * d + 0.1 * d * e + 0.1 * d.abs().sqrt())
        + (20.0 * sin_deg(6.0 * d) + 20.0 * sin_deg(2.0 * d)) * 2.0 / 3.0
        + (20.0 * sin_deg(d) + 40.0 * sin_deg(d / 3.0)) * 2.0 / 3.0
        + (150.0 * sin_deg(d / 12.0) + 300.0 * sin_deg(d / 30.0)) * 2.0 / 3.0;

    let dlat = (-100.0 + 2.0 * d + 3.0 * e + 0.2 * e * e + 0.1 * d * e + 0.2 * d.abs().sqrt())
        + (20.0 * sin_deg(6.0 * d) + 20.0 * sin_deg(2.0 * d)) * 2.0 / 3.0
        + (20.0 * sin_deg(e) + 40.0 * sin_deg(e / 3.0)) * 2.0 / 3.0
        + (160.0 * sin_deg(e / 12.0) + 320.0 * sin_deg(e / 30.0)) * 2.0 / 3.0;

    let lng_off = dlng * 180.0 / (a_mul * std::f64::consts::PI);
    let lat_off = -dlat * 180.0 / ((A * (1.0 - EE)) / (magic * sqrt_magic) * std::f64::consts::PI);
    (lng_off, lat_off)
}

/// Convert a point from WGS-84 to GCJ-02.
///
/// Coordinates outside China are returned unchanged.
pub fn wgs84_to_gcj02(lng: f64, lat: f64) -> (f64, f64) {
    if out_of_china(lng, lat) {
        return (lng, lat);
    }
    let (dlng, dlat) = _delta(lng, lat);
    (lng + dlng, lat + dlat)
}

/// Convert a point from GCJ-02 to WGS-84 via fixed-point iteration.
///
/// Coordinates outside China are returned unchanged. Typically converges
/// in 2–3 iterations to sub-meter accuracy.
#[allow(dead_code)]
pub fn gcj02_to_wgs84(lng: f64, lat: f64) -> (f64, f64) {
    if out_of_china(lng, lat) {
        return (lng, lat);
    }
    let mut wlng = lng;
    let mut wlat = lat;
    for _ in 0..6 {
        let (glng, glat) = wgs84_to_gcj02(wlng, wlat);
        let dlng = glng - lng;
        let dlat = glat - lat;
        if dlng.abs() < 1e-9 && dlat.abs() < 1e-9 {
            break;
        }
        wlng -= dlng;
        wlat -= dlat;
    }
    (wlng, wlat)
}

// ─── Bbox helpers ───────────────────────────────────────────────────────

/// Convert a WGS-84 bounding box to GCJ-02 with a safety margin.
///
/// The margin (in degrees) adds padding around the result because the offset
/// between the two coordinate systems can shift the bbox.  The default
/// `margin_deg = 0.005` corresponds to ~500 m, which covers the maximum
/// expected offset anywhere in China.
pub fn wgs84_bbox_to_gcj02(bbox: &LLBBox, margin_deg: f64) -> LLBBox {
    // Project the four corners and take the envelope.
    let corners = [
        (bbox.min().lng(), bbox.min().lat()),
        (bbox.max().lng(), bbox.min().lat()),
        (bbox.min().lng(), bbox.max().lat()),
        (bbox.max().lng(), bbox.max().lat()),
    ];
    let mut min_lat = f64::MAX;
    let mut max_lat = f64::MIN;
    let mut min_lng = f64::MAX;
    let mut max_lng = f64::MIN;
    for (lng, lat) in corners {
        let (glng, glat) = wgs84_to_gcj02(lng, lat);
        if glat < min_lat {
            min_lat = glat;
        }
        if glat > max_lat {
            max_lat = glat;
        }
        if glng < min_lng {
            min_lng = glng;
        }
        if glng > max_lng {
            max_lng = glng;
        }
    }
    // Safety margin
    let m = margin_deg.abs();
    let min_lat = (min_lat - m).max(-90.0);
    let max_lat = (max_lat + m).min(90.0);
    let min_lng = (min_lng - m).max(-180.0);
    let max_lng = (max_lng + m).min(180.0);
    LLBBox::new(min_lat, min_lng, max_lat, max_lng).expect("GCJ-02 bbox construction failed")
}

/// Convert an LLPoint from WGS-84 to GCJ-02.
#[allow(dead_code)]
pub fn llpoint_wgs84_to_gcj02(point: &LLPoint) -> LLPoint {
    let (lng, lat) = wgs84_to_gcj02(point.lng(), point.lat());
    LLPoint::new(lat, lng).expect("GCJ-02 point conversion produced invalid coords")
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity: coordinates outside China are untouched.
    #[test]
    fn outside_china_unchanged() {
        let test_cases = [
            (48.13, 11.57),   // Munich
            (40.71, -74.00),  // New York
            (-33.86, 151.20), // Sydney
            (0.0, 0.0),       // Atlantic
        ];
        for (lat, lng) in test_cases {
            let (g_lng, g_lat) = wgs84_to_gcj02(lng, lat);
            assert!((g_lng - lng).abs() < 1e-9, "lng shifted outside China");
            assert!((g_lat - lat).abs() < 1e-9, "lat shifted outside China");
        }
    }

    /// Inside China, the offset must be in the expected range.
    #[test]
    fn inside_china_has_offset() {
        let (g_lng, g_lat) = wgs84_to_gcj02(116.397, 39.907); // Beijing
        let dlng = g_lng - 116.397;
        let dlat = g_lat - 39.907;
        let offset_deg = dlng.hypot(dlat);
        let offset_m = offset_deg * 111_000.0;
        eprintln!("GCJ-02 offset: {dlng:.6}°, {dlat:.6}° ({offset_m:.0} m)");
        assert!(
            offset_m > 100.0,
            "expected GCJ-02 offset but got {offset_m:.0} m"
        );
        assert!(
            offset_m < 2000.0,
            "GCJ-02 offset {offset_m:.0} m is implausibly large"
        );
    }

    /// Round-trip: WGS-84 → GCJ-02 → WGS-84 should recover the original.
    #[test]
    fn round_trip_within_tolerance() {
        let test_cases = [
            (116.397, 39.907), // Beijing
            (121.473, 31.230), // Shanghai
            (104.065, 30.572), // Chengdu
            (113.264, 23.129), // Guangzhou
            (91.132, 29.659),  // Lhasa
        ];
        for (lng, lat) in test_cases {
            let (g_lng, g_lat) = wgs84_to_gcj02(lng, lat);
            let (w_lng, w_lat) = gcj02_to_wgs84(g_lng, g_lat);
            let dist = (w_lng - lng).hypot(w_lat - lat);
            assert!(
                dist < 0.000005, // ~0.5 m
                "round-trip error at ({lng}, {lat}): {dist:.9} degrees"
            );
        }
    }

    /// The bbox helper expands enough to cover the GCJ-02 shift.
    #[test]
    fn bbox_conversion_covers_original() {
        let bbox = LLBBox::new(39.90, 116.39, 39.92, 116.42).unwrap();
        let gcj = wgs84_bbox_to_gcj02(&bbox, 0.001);
        // Every original point, projected into GCJ-02, must fall inside the
        // expanded GCJ-02 bbox.
        let test_points = [
            (116.39, 39.90),
            (116.42, 39.90),
            (116.39, 39.92),
            (116.42, 39.92),
            (116.405, 39.91),
        ];
        for (lng, lat) in test_points {
            let (g_lng, g_lat) = wgs84_to_gcj02(lng, lat);
            assert!(
                gcj.min().lng() <= g_lng && g_lng <= gcj.max().lng(),
                "lng {g_lng} outside GCJ bbox [{:.6},{:.6}]",
                gcj.min().lng(),
                gcj.max().lng()
            );
            assert!(
                gcj.min().lat() <= g_lat && g_lat <= gcj.max().lat(),
                "lat {g_lat} outside GCJ bbox [{:.6},{:.6}]",
                gcj.min().lat(),
                gcj.max().lat()
            );
        }
    }

    /// LLPoint helper round-trips correctly.
    #[test]
    fn llpoint_helper_round_trip() {
        let original = LLPoint::new(31.23, 121.47).unwrap(); // Shanghai
        let gcj = llpoint_wgs84_to_gcj02(&original);
        let (w_lng, w_lat) = gcj02_to_wgs84(gcj.lng(), gcj.lat());
        assert!((w_lng - original.lng()).abs() < 1e-8);
        assert!((w_lat - original.lat()).abs() < 1e-8);
    }
}
