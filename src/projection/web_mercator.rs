use super::Projection;

/// Mean Earth radius in meters (WGS84 spherical approximation).
const EARTH_RADIUS: f64 = 6_371_000.0;

/// Web Mercator projection with a local origin offset so that the reference
/// point maps to `(0, 0)` in the projected coordinate system.
///
/// Orientation follows Minecraft conventions: increasing X points east,
/// and **north maps to negative Z**.
pub struct WebMercatorProjection {
    /// Reference latitude in degrees.
    pub(crate) origin_lat: f64,
    /// Reference longitude in degrees.
    pub(crate) origin_lon: f64,
    /// Scale factor (blocks per meter). Default `1.0`.
    pub(crate) scale: f64,
    /// Pre-computed Z offset so that `forward(origin_lat, _)` yields `z = 0`.
    pub(crate) z_offset: f64,
}

impl WebMercatorProjection {
    /// Create a new projection centred on `(origin_lat, origin_lon)`.
    ///
    /// `scale` is expressed in blocks-per-meter (use `1.0` for 1:1).
    pub fn new(origin_lat: f64, origin_lon: f64, scale: f64) -> Self {
        // Pre-compute z_offset so that forward(origin_lat, _) gives z = 0.
        let lat_rad = origin_lat.to_radians();
        let raw_z =
            -EARTH_RADIUS * (std::f64::consts::FRAC_PI_4 + lat_rad / 2.0).tan().ln() * scale;
        let z_offset = -raw_z;

        Self {
            origin_lat,
            origin_lon,
            scale,
            z_offset,
        }
    }
}

impl Projection for WebMercatorProjection {
    fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
        let lat_ref_rad = self.origin_lat.to_radians();
        let lat_rad = lat.to_radians();
        let lon_diff_rad = (lon - self.origin_lon).to_radians();

        let x = EARTH_RADIUS * lon_diff_rad * lat_ref_rad.cos() * self.scale;

        let z =
            -EARTH_RADIUS * (std::f64::consts::FRAC_PI_4 + lat_rad / 2.0).tan().ln() * self.scale
                + self.z_offset;

        (x, z)
    }

    fn inverse(&self, x: f64, z: f64) -> (f64, f64) {
        let lat_ref_rad = self.origin_lat.to_radians();

        // Recover longitude from x.
        let lon_diff_rad = x / (EARTH_RADIUS * lat_ref_rad.cos() * self.scale);
        let lon = self.origin_lon + lon_diff_rad.to_degrees();

        // Recover latitude from z.
        let raw_z = z - self.z_offset;
        // raw_z = -R * ln(tan(pi/4 + lat/2)) * scale
        // => ln(tan(pi/4 + lat/2)) = -raw_z / (R * scale)
        let y = (-raw_z) / (EARTH_RADIUS * self.scale);
        let lat_rad = 2.0 * (y.exp().atan() - std::f64::consts::FRAC_PI_4);
        let lat = lat_rad.to_degrees();

        (lat, lon)
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
}
