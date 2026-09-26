use super::Projection;

/// Mean Earth radius in meters (WGS84 spherical approximation).
pub const EARTH_RADIUS: f64 = 6_371_000.0;

/// Web Mercator with the origin at block `(0, 0)`.
///
/// Orientation follows Minecraft conventions: increasing X points east,
/// and **north maps to negative Z**.
///
/// Both axes use `k = scale * cos(origin_lat)`, so a block is `1 / scale`
/// metres at the origin latitude in every direction. Elsewhere the ground
/// scale drifts by `cos(origin_lat) / cos(lat)`, about 2% per 100 km north
/// or south at mid-latitudes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WebMercatorProjection {
    /// Reference latitude in degrees.
    pub(crate) origin_lat: f64,
    /// Reference longitude in degrees.
    pub(crate) origin_lon: f64,
    /// Blocks per metre at the origin latitude.
    pub(crate) scale: f64,
    k: f64,
    origin_merc_y: f64,
}

#[inline]
pub fn mercator_y_m(lat_deg: f64) -> f64 {
    EARTH_RADIUS
        * (std::f64::consts::FRAC_PI_4 + lat_deg.to_radians() / 2.0)
            .tan()
            .ln()
}

#[inline]
pub fn mercator_lat_deg(y_m: f64) -> f64 {
    (2.0 * ((y_m / EARTH_RADIUS).exp().atan() - std::f64::consts::FRAC_PI_4)).to_degrees()
}

impl WebMercatorProjection {
    /// Create a new projection centred on `(origin_lat, origin_lon)`.
    pub fn new(origin_lat: f64, origin_lon: f64, scale: f64) -> Self {
        Self {
            origin_lat,
            origin_lon,
            scale,
            k: scale * origin_lat.to_radians().cos(),
            origin_merc_y: mercator_y_m(origin_lat),
        }
    }

    #[inline]
    pub fn x_for_lon(&self, lon: f64) -> f64 {
        EARTH_RADIUS * (lon - self.origin_lon).to_radians() * self.k
    }

    #[inline]
    pub fn z_for_lat(&self, lat: f64) -> f64 {
        -(mercator_y_m(lat) - self.origin_merc_y) * self.k
    }

    #[inline]
    pub fn lon_for_x(&self, x: f64) -> f64 {
        self.origin_lon + (x / (EARTH_RADIUS * self.k)).to_degrees()
    }

    #[inline]
    pub fn lat_for_z(&self, z: f64) -> f64 {
        mercator_lat_deg(self.origin_merc_y - z / self.k)
    }
}

impl Projection for WebMercatorProjection {
    fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
        (self.x_for_lon(lon), self.z_for_lat(lat))
    }

    fn inverse(&self, x: f64, z: f64) -> (f64, f64) {
        (self.lat_for_z(z), self.lon_for_x(x))
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
            (-33.86, 151.21),
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
    fn one_ground_metre_is_one_block_in_both_axes_at_the_origin() {
        const D: f64 = 0.0005;
        for lat in [0.0045, 47.3745, 61.0045, -33.8] {
            let p = WebMercatorProjection::new(lat, 8.5415, 1.0);

            let (x_east, _) = p.forward(lat, 8.5415 + D);
            let ground_x = EARTH_RADIUS * D.to_radians() * lat.to_radians().cos();
            assert!(
                (x_east / ground_x - 1.0).abs() < 1e-9,
                "x should be ground metres at lat {lat}: got {x_east}, ground {ground_x}"
            );

            let (_, z_south) = p.forward(lat - D, 8.5415);
            let (_, z_north) = p.forward(lat + D, 8.5415);
            let ground_z = EARTH_RADIUS * (2.0 * D).to_radians();
            let ratio = (z_south - z_north) / ground_z;
            assert!(
                (ratio - 1.0).abs() < 1e-6,
                "z should be ground metres at lat {lat}: ratio {ratio}"
            );
        }
    }

    #[test]
    fn axes_are_separable() {
        let p = proj();
        let (x1, _) = p.forward(48.0, 2.5);
        let (x2, _) = p.forward(49.5, 2.5);
        assert!((x1 - x2).abs() < 1e-9);
        let (_, z1) = p.forward(48.3, 2.0);
        let (_, z2) = p.forward(48.3, 3.0);
        assert!((z1 - z2).abs() < 1e-9);
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

    #[test]
    fn mercator_helpers_roundtrip() {
        for lat in [-80.0, -33.86, 0.0, 12.5, 48.8566, 71.0] {
            let y = mercator_y_m(lat);
            assert!((mercator_lat_deg(y) - lat).abs() < 1e-9, "lat {lat}");
        }
    }
}
