//! Spherical transverse Mercator projection.
//!
//! This is the projection to use when local geometry has to be true to life:
//! unlike [`crate::projection::WebMercatorProjection`], it does not oversize
//! everything by `1/cos(latitude)`. It is used to pin a real-world position to
//! an arbitrary Minecraft origin (an "anchor").

use super::{Projection, EARTH_RADIUS};

/// Spherical transverse Mercator, with the projection origin placed at an
/// arbitrary Minecraft `(x, z)` via a false easting/northing.
///
/// The sphere (radius [`EARTH_RADIUS`]) is deliberate: the ellipsoidal
/// Krueger series would buy sub-centimetre accuracy that a block-resolution
/// world cannot represent. Within a few hundred kilometres of the central
/// meridian the spherical form is accurate to well under a metre.
///
/// Distortion grows with distance from the central meridian: the scale factor
/// is `1 / sqrt(1 - (cos(lat) sin(lon - origin_lon))^2)`, which is 1.0000 on
/// the central meridian, ~1.0011 (0.11%) at 300 km, ~1.005 at 640 km, and
/// diverges to infinity 90 degrees away, where the formulas break down
/// entirely. That is precisely why anchors carry a bounded radius: each anchor
/// owns its own central meridian and is only valid near it.
///
/// Orientation follows Minecraft conventions: increasing X points east, and
/// **north maps to negative Z**.
#[allow(dead_code)] // Consumed by the stream anchor layer.
#[derive(Debug, Clone, Copy)]
pub struct TransverseMercatorProjection {
    /// Origin latitude in degrees (the latitude that maps to `false_northing`).
    pub(crate) origin_lat: f64,
    /// Central meridian in degrees.
    pub(crate) origin_lon: f64,
    /// Scale factor (blocks per meter). Default `1.0`.
    pub(crate) scale: f64,
    /// Minecraft X the projection origin is pinned to.
    pub(crate) false_easting: f64,
    /// Minecraft Z the projection origin is pinned to.
    pub(crate) false_northing: f64,
}

#[allow(dead_code)] // Consumed by the stream anchor layer.
impl TransverseMercatorProjection {
    /// Projection centred on `(origin_lat, origin_lon)`, which maps to the
    /// Minecraft origin `(0, 0)`.
    ///
    /// `scale` is expressed in blocks-per-meter (use `1.0` for 1:1).
    pub fn new(origin_lat: f64, origin_lon: f64, scale: f64) -> Self {
        Self::with_origin(origin_lat, origin_lon, scale, 0.0, 0.0)
    }

    /// Projection centred on `(origin_lat, origin_lon)`, pinned so that this
    /// geographic point maps to the Minecraft coordinates `(mc_x, mc_z)`.
    ///
    /// This is what an anchor is: a real-world position nailed to a chosen
    /// world position, with everything nearby following from it undistorted.
    pub fn with_origin(origin_lat: f64, origin_lon: f64, scale: f64, mc_x: f64, mc_z: f64) -> Self {
        Self {
            origin_lat,
            origin_lon,
            scale,
            false_easting: mc_x,
            false_northing: mc_z,
        }
    }
}

impl Projection for TransverseMercatorProjection {
    fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
        let lat_rad = lat.to_radians();
        let dlon_rad = (lon - self.origin_lon).to_radians();

        let b = lat_rad.cos() * dlon_rad.sin();
        let x =
            (EARTH_RADIUS / 2.0) * ((1.0 + b) / (1.0 - b)).ln() * self.scale + self.false_easting;

        // atan2(tan(lat), cos(dlon)) is the "footpoint" latitude: the latitude
        // on the central meridian that the point projects to.
        let north = EARTH_RADIUS
            * (lat_rad.tan().atan2(dlon_rad.cos()) - self.origin_lat.to_radians())
            * self.scale;

        (x, -north + self.false_northing)
    }

    fn inverse(&self, x: f64, z: f64) -> (f64, f64) {
        // Undo the false origin and the scale, then normalise by the radius.
        let x_norm = (x - self.false_easting) / self.scale / EARTH_RADIUS;
        let north = (self.false_northing - z) / self.scale;

        // The footpoint latitude the forward pass computed.
        let footpoint = north / EARTH_RADIUS + self.origin_lat.to_radians();

        let lat_rad = (footpoint.sin() / x_norm.cosh()).asin();
        let lon = self.origin_lon + x_norm.sinh().atan2(footpoint.cos()).to_degrees();

        (lat_rad.to_degrees(), lon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN_LAT: f64 = 48.8566; // Paris
    const ORIGIN_LON: f64 = 2.3522;

    fn proj() -> TransverseMercatorProjection {
        TransverseMercatorProjection::new(ORIGIN_LAT, ORIGIN_LON, 1.0)
    }

    fn anchored() -> TransverseMercatorProjection {
        TransverseMercatorProjection::with_origin(ORIGIN_LAT, ORIGIN_LON, 1.0, 1000.0, -2000.0)
    }

    #[test]
    fn test_origin_maps_to_false_origin() {
        let (x, z) = proj().forward(ORIGIN_LAT, ORIGIN_LON);
        assert!(x.abs() < 1e-6, "expected x ~0 at origin, got {x}");
        assert!(z.abs() < 1e-6, "expected z ~0 at origin, got {z}");

        let (x, z) = anchored().forward(ORIGIN_LAT, ORIGIN_LON);
        assert!((x - 1000.0).abs() < 1e-6, "expected x ~1000, got {x}");
        assert!((z + 2000.0).abs() < 1e-6, "expected z ~-2000, got {z}");
    }

    #[test]
    fn test_roundtrip_forward_inverse() {
        let p = anchored();
        let test_points = [
            (ORIGIN_LAT, ORIGIN_LON),
            (48.8600, 2.3600),
            (48.8500, 2.3400),
            (49.5, 4.0),
            (48.0, 2.0),
            (ORIGIN_LAT, 5.0),
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
        let (x2, _) = p.forward(ORIGIN_LAT, ORIGIN_LON + 0.5);
        assert!(
            x2 > x1,
            "increasing longitude should increase x: x1={x1}, x2={x2}"
        );
    }

    #[test]
    fn test_increasing_latitude_decreases_z() {
        let p = proj();
        let (_, z1) = p.forward(ORIGIN_LAT, ORIGIN_LON);
        let (_, z2) = p.forward(ORIGIN_LAT + 0.5, ORIGIN_LON);
        assert!(
            z2 < z1,
            "increasing latitude (north) should decrease z: z1={z1}, z2={z2}"
        );
    }

    /// The whole point of this projection: 1000 m of ground is 1000 blocks.
    #[test]
    fn test_north_south_metre_is_a_block() {
        let p = proj();
        let dlat_deg = (1000.0 / EARTH_RADIUS).to_degrees();

        let (_, z0) = p.forward(ORIGIN_LAT, ORIGIN_LON);
        let (_, z1) = p.forward(ORIGIN_LAT + dlat_deg, ORIGIN_LON);

        let dz = z0 - z1; // north is -Z, so this is positive
        assert!(
            (dz - 1000.0).abs() < 0.5,
            "1000 m north should be ~1000 blocks, got {dz}"
        );
    }

    #[test]
    fn test_east_west_metre_is_a_block() {
        let p = proj();
        let dlon_deg = (1000.0 / (EARTH_RADIUS * ORIGIN_LAT.to_radians().cos())).to_degrees();

        let (x0, _) = p.forward(ORIGIN_LAT, ORIGIN_LON);
        let (x1, _) = p.forward(ORIGIN_LAT, ORIGIN_LON + dlon_deg);

        let dx = x1 - x0;
        assert!(
            (dx - 1000.0).abs() < 1.0,
            "1000 m east should be ~1000 blocks, got {dx}"
        );
    }

    /// Distortion is bounded near the central meridian, which is what lets an
    /// anchor cover a region rather than a point. Measured as projected
    /// distance over true ground distance for a short north-south segment.
    #[test]
    fn test_distortion_at_300km_stays_small() {
        let p = proj();
        let dlon_deg = (300_000.0 / (EARTH_RADIUS * ORIGIN_LAT.to_radians().cos())).to_degrees();
        let lon = ORIGIN_LON + dlon_deg;

        // Same longitude, so the true ground distance is exactly R * dlat.
        let dlat_deg = 0.01;
        let (x0, z0) = p.forward(ORIGIN_LAT, lon);
        let (x1, z1) = p.forward(ORIGIN_LAT + dlat_deg, lon);

        let projected = ((x1 - x0).powi(2) + (z1 - z0).powi(2)).sqrt();
        let ground = EARTH_RADIUS * dlat_deg.to_radians();
        let k = projected / ground;

        assert!(
            (k - 1.0).abs() < 0.003,
            "scale error 300 km off the central meridian should be <0.3%, got k={k}"
        );

        // ...and essentially zero on the central meridian itself.
        let (x0, z0) = p.forward(ORIGIN_LAT, ORIGIN_LON);
        let (x1, z1) = p.forward(ORIGIN_LAT + dlat_deg, ORIGIN_LON);
        let k0 = ((x1 - x0).powi(2) + (z1 - z0).powi(2)).sqrt() / ground;
        assert!(
            (k0 - 1.0).abs() < 1e-9,
            "central meridian should be true to scale, got k={k0}"
        );
    }

    #[test]
    fn test_scale_factor_scales_both_axes() {
        let p1 = TransverseMercatorProjection::new(ORIGIN_LAT, ORIGIN_LON, 1.0);
        let p2 = TransverseMercatorProjection::new(ORIGIN_LAT, ORIGIN_LON, 2.0);

        let (x1, z1) = p1.forward(ORIGIN_LAT + 0.01, ORIGIN_LON + 0.01);
        let (x2, z2) = p2.forward(ORIGIN_LAT + 0.01, ORIGIN_LON + 0.01);

        assert!((x2 - 2.0 * x1).abs() < 1e-6, "x1={x1}, x2={x2}");
        assert!((z2 - 2.0 * z1).abs() < 1e-6, "z1={z1}, z2={z2}");
    }
}
