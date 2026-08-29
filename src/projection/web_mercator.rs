//! Web Mercator projection with a local origin offset.

use super::{Projection, EARTH_RADIUS};

/// Web Mercator (the EPSG:3857 formulas) with a local origin offset, so that
/// the reference point maps to `(0, 0)` in Minecraft coordinates.
///
/// Both axes are expressed in *Mercator* meters:
///
/// ```text
/// x = R * (lon - origin_lon)_rad            * scale
/// z = -(R * ln(tan(pi/4 + lat/2)) - N0)     * scale
/// ```
///
/// Using Mercator meters on *both* axes is what makes the projection conformal
/// and locally isotropic: at any given point, one block of X and one block of Z
/// cover the same ground distance, so nothing is sheared or squashed. An
/// earlier version of this file mixed true ground meters on X with Mercator
/// meters on Z, which stretched everything north-south by `1/cos(lat)` — a
/// factor of 1.63 at 52 degrees N.
///
/// The price of conformality is that Web Mercator's scale factor is
/// `1/cos(latitude)`: geometry is uniformly *oversized* away from the equator,
/// by about 1.49x at 48 degrees N and 1.32x at 40.7 degrees N. Shapes stay
/// correct, but a 10 m wall becomes ~15 m of blocks. This projection is
/// therefore suitable for GLOBAL PLACEMENT — every point on Earth gets one
/// consistent world position, so separately generated areas line up — but not
/// for undistorted local geometry. Use
/// [`crate::projection::TransverseMercatorProjection`] when true-to-life local
/// dimensions matter.
///
/// Because of that oversizing, the projected world is larger than the bounding
/// box's ground size on both axes. Anything that maps a block onto a raster of
/// the bbox — the elevation and land-cover grids — must therefore be built for
/// the *projected* extent, not for a haversine ground distance; see
/// `crate::ground::projected_world_extent`.
///
/// Orientation follows Minecraft conventions: increasing X points east, and
/// **north maps to negative Z**.
#[derive(Debug, Clone, Copy)]
pub struct WebMercatorProjection {
    /// Reference longitude in degrees (the projection's origin meridian).
    pub(crate) origin_lon: f64,
    /// Scale factor (blocks per Mercator meter). Default `1.0`.
    pub(crate) scale: f64,
    /// Mercator northing of the reference latitude, in unscaled meters. The
    /// origin latitude is folded into this value; `inverse(0.0, 0.0)` gives it
    /// back exactly.
    pub(crate) origin_northing: f64,
}

/// Mercator northing of `lat` (degrees) in unscaled meters.
fn mercator_northing(lat: f64) -> f64 {
    EARTH_RADIUS
        * (std::f64::consts::FRAC_PI_4 + lat.to_radians() / 2.0)
            .tan()
            .ln()
}

impl WebMercatorProjection {
    /// Create a new projection centred on `(origin_lat, origin_lon)`, so that
    /// `forward(origin_lat, origin_lon)` is exactly `(0.0, 0.0)`.
    ///
    /// `scale` is expressed in blocks-per-meter (use `1.0` for 1:1).
    pub fn new(origin_lat: f64, origin_lon: f64, scale: f64) -> Self {
        Self {
            origin_lon,
            scale,
            origin_northing: mercator_northing(origin_lat),
        }
    }
}

impl Projection for WebMercatorProjection {
    fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
        let x = EARTH_RADIUS * (lon - self.origin_lon).to_radians() * self.scale;
        let z = -(mercator_northing(lat) - self.origin_northing) * self.scale;

        (x, z)
    }

    fn inverse(&self, x: f64, z: f64) -> (f64, f64) {
        // x = R * dlon_rad * scale
        let lon = self.origin_lon + (x / (EARTH_RADIUS * self.scale)).to_degrees();

        // z = -(northing - origin_northing) * scale
        let northing = self.origin_northing - z / self.scale;
        // northing = R * ln(tan(pi/4 + lat/2))
        let lat_rad = 2.0 * ((northing / EARTH_RADIUS).exp().atan() - std::f64::consts::FRAC_PI_4);

        (lat_rad.to_degrees(), lon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN_LAT: f64 = 48.8566; // Paris
    const ORIGIN_LON: f64 = 2.3522;

    fn proj() -> WebMercatorProjection {
        WebMercatorProjection::new(ORIGIN_LAT, ORIGIN_LON, 1.0)
    }

    #[test]
    fn test_origin_maps_to_zero() {
        let p = proj();
        let (x, z) = p.forward(ORIGIN_LAT, ORIGIN_LON);
        assert!(x.abs() < 1e-6, "expected x ~0 at origin, got {x}");
        assert!(z.abs() < 1e-6, "expected z ~0 at origin, got {z}");
    }

    #[test]
    fn test_roundtrip_forward_inverse() {
        let p = proj();
        let test_points = [
            (ORIGIN_LAT, ORIGIN_LON),
            (48.8600, 2.3600),
            (48.8500, 2.3400),
            (49.0, 2.5),
            (48.0, 2.0),
            // Web Mercator is a global projection: the round trip must hold
            // on the other side of the planet too.
            (-33.8688, 151.2093),
        ];

        for (lat, lon) in test_points {
            let (x, z) = p.forward(lat, lon);
            let (lat2, lon2) = p.inverse(x, z);
            assert!(
                (lat2 - lat).abs() < 1e-8,
                "latitude roundtrip failed for ({lat}, {lon}): got {lat2}"
            );
            assert!(
                (lon2 - lon).abs() < 1e-8,
                "longitude roundtrip failed for ({lat}, {lon}): got {lon2}"
            );
        }
    }

    #[test]
    fn test_increasing_longitude_increases_x() {
        let p = proj();
        let (x1, _) = p.forward(ORIGIN_LAT, ORIGIN_LON);
        let (x2, _) = p.forward(ORIGIN_LAT, ORIGIN_LON + 1.0);
        assert!(
            x2 > x1,
            "increasing longitude should increase x: x1={x1}, x2={x2}"
        );
    }

    #[test]
    fn test_increasing_latitude_decreases_z() {
        let p = proj();
        let (_, z1) = p.forward(ORIGIN_LAT, ORIGIN_LON);
        let (_, z2) = p.forward(ORIGIN_LAT + 1.0, ORIGIN_LON);
        assert!(
            z2 < z1,
            "increasing latitude (north) should decrease z: z1={z1}, z2={z2}"
        );
    }

    #[test]
    fn test_scale_factor() {
        let p1 = WebMercatorProjection::new(ORIGIN_LAT, ORIGIN_LON, 1.0);
        let p2 = WebMercatorProjection::new(ORIGIN_LAT, ORIGIN_LON, 2.0);

        let target_lat = ORIGIN_LAT + 0.01;
        let target_lon = ORIGIN_LON + 0.01;

        let (x1, z1) = p1.forward(target_lat, target_lon);
        let (x2, z2) = p2.forward(target_lat, target_lon);

        assert!(
            (x2 - 2.0 * x1).abs() < 1e-6,
            "x should scale linearly: x1={x1}, x2={x2}"
        );
        assert!(
            (z2 - 2.0 * z1).abs() < 1e-6,
            "z should scale linearly: z1={z1}, z2={z2}"
        );
    }

    /// The regression test for the bug this file used to have (true ground
    /// meters on X, Mercator meters on Z). In a conformal Mercator an
    /// equal-angle step east and north must come out in the ratio
    /// `cos(origin_lat)`; the broken version produced `cos^2(origin_lat)`
    /// (0.433 instead of 0.658 at 48.86 degrees N), i.e. a 1.52x north-south
    /// stretch.
    #[test]
    fn test_isotropic_equal_angle_steps() {
        let p = proj();
        let step_deg = 1e-4;

        let (dx, _) = p.forward(ORIGIN_LAT, ORIGIN_LON + step_deg);
        let (_, dz) = p.forward(ORIGIN_LAT + step_deg, ORIGIN_LON);

        let ratio = dx / dz.abs();
        let expected = ORIGIN_LAT.to_radians().cos();
        assert!(
            (ratio - expected).abs() < 1e-5,
            "dx/|dz| for equal-angle steps should be cos(origin_lat)={expected}, got {ratio}"
        );
    }

    /// Web Mercator oversizes by `1/cos(lat)`. That is expected and documented;
    /// this pins the magnitude so the trade-off cannot silently change.
    #[test]
    fn test_oversize_factor_is_one_over_cos_lat() {
        let p = proj();

        // 1000 m of real ground distance due north of the origin.
        let dlat_deg = (1000.0 / EARTH_RADIUS).to_degrees();
        let (_, z) = p.forward(ORIGIN_LAT + dlat_deg, ORIGIN_LON);

        let expected = 1000.0 / ORIGIN_LAT.to_radians().cos();
        assert!(
            (z.abs() - expected).abs() < 1.0,
            "1000 m north should project to ~{expected} blocks, got {}",
            z.abs()
        );
    }
}
