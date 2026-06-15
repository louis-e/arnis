use super::cartesian::{XZBBox, XZPoint};
use super::geographic::{LLBBox, LLPoint};

/// Earth radius in meters (WGS84 spherical approximation), matching the
/// value used in `crate::projection::web_mercator`.
const EARTH_RADIUS: f64 = 6_371_000.0;

/// Internal mode discriminator so `transform_point` can dispatch between the
/// legacy linear interpolation and Web Mercator projection.
#[allow(dead_code)]
enum ProjectionMode {
    /// Existing linear-interpolation mode (no geographic projection).
    Local,
    /// Web Mercator projection with a local origin offset.
    WebMercator {
        origin_lat: f64,
        origin_lon: f64,
        scale: f64,
        cos_lat_ref: f64,
        z_offset: f64,
    },
}

/// Transform geographic space (within llbbox) to a local tangential cartesian space (within xzbbox)
pub struct CoordTransformer {
    len_lat: f64,
    len_lng: f64,
    scale_factor_x: f64,
    scale_factor_z: f64,
    min_lat: f64,
    min_lng: f64,
    mode: ProjectionMode,
}

impl CoordTransformer {
    pub fn scale_factor_x(&self) -> f64 {
        self.scale_factor_x
    }

    pub fn scale_factor_z(&self) -> f64 {
        self.scale_factor_z
    }

    pub fn llbbox_to_xzbbox(
        llbbox: &LLBBox,
        scale: f64,
    ) -> Result<(CoordTransformer, XZBBox), String> {
        let err_header = "Construct LLBBox to XZBBox transformation failed".to_string();

        if scale <= 0.0 {
            return Err(format!("{}: scale <= 0.0", &err_header));
        }

        let (scale_factor_z, scale_factor_x) = geo_distance(llbbox.min(), llbbox.max());
        let scale_factor_z: f64 = scale_factor_z.floor() * scale;
        let scale_factor_x: f64 = scale_factor_x.floor() * scale;

        let xzbbox = XZBBox::rect_from_xz_lengths(scale_factor_x, scale_factor_z)
            .map_err(|e| format!("{}:\n{}", &err_header, e))?;

        Ok((
            Self {
                len_lat: llbbox.max().lat() - llbbox.min().lat(),
                len_lng: llbbox.max().lng() - llbbox.min().lng(),
                scale_factor_x,
                scale_factor_z,
                min_lat: llbbox.min().lat(),
                min_lng: llbbox.min().lng(),
                mode: ProjectionMode::Local,
            },
            xzbbox,
        ))
    }

    /// Create a `CoordTransformer` using a Web Mercator projection.
    ///
    /// The bounding box is computed by projecting all four corners of the
    /// `llbbox` and taking the axis-aligned envelope. The returned `XZBBox`
    /// represents the Minecraft world extents for the projected area.
    pub fn with_projection(
        llbbox: &LLBBox,
        scale: f64,
        projection: &dyn crate::projection::Projection,
    ) -> Result<(CoordTransformer, XZBBox), String> {
        if scale <= 0.0 {
            return Err("Scale must be > 0.0".to_string());
        }

        // Project all four corners to find the Minecraft bounding box.
        // NW corner
        let (x_nw, z_nw) = projection.forward(llbbox.max().lat(), llbbox.min().lng());
        // SE corner
        let (x_se, z_se) = projection.forward(llbbox.min().lat(), llbbox.max().lng());
        // NE corner
        let (x_ne, z_ne) = projection.forward(llbbox.max().lat(), llbbox.max().lng());
        // SW corner
        let (x_sw, z_sw) = projection.forward(llbbox.min().lat(), llbbox.min().lng());

        let x_min = x_nw.min(x_sw).min(x_ne).min(x_se).floor() as i32;
        let x_max = x_nw.max(x_sw).max(x_ne).max(x_se).ceil() as i32;
        let z_min = z_nw.min(z_sw).min(z_ne).min(z_se).floor() as i32;
        let z_max = z_nw.max(z_sw).max(z_ne).max(z_se).ceil() as i32;

        let xzbbox = XZBBox::rect_from_min_max(x_min, z_min, x_max, z_max)
            .map_err(|e| format!("Failed to create XZBBox from projection: {}", e))?;

        let origin_lat = (llbbox.min().lat() + llbbox.max().lat()) / 2.0;
        let origin_lon = (llbbox.min().lng() + llbbox.max().lng()) / 2.0;
        let cos_lat_ref = origin_lat.to_radians().cos();

        // z_offset chosen so that forward(origin_lat, _) gives z = 0.
        let z_offset = EARTH_RADIUS
            * (std::f64::consts::FRAC_PI_4 + origin_lat.to_radians() / 2.0)
                .tan()
                .ln()
            * scale;

        Ok((
            CoordTransformer {
                len_lat: llbbox.max().lat() - llbbox.min().lat(),
                len_lng: llbbox.max().lng() - llbbox.min().lng(),
                scale_factor_x: (x_max - x_min) as f64,
                scale_factor_z: (z_max - z_min) as f64,
                min_lat: llbbox.min().lat(),
                min_lng: llbbox.min().lng(),
                mode: ProjectionMode::WebMercator {
                    origin_lat,
                    origin_lon,
                    scale,
                    cos_lat_ref,
                    z_offset,
                },
            },
            xzbbox,
        ))
    }

    pub fn transform_point(&self, llpoint: LLPoint) -> XZPoint {
        match &self.mode {
            ProjectionMode::Local => {
                // Calculate the relative position within the bounding box
                let rel_x: f64 = (llpoint.lng() - self.min_lng) / self.len_lng;
                let rel_z: f64 = 1.0 - (llpoint.lat() - self.min_lat) / self.len_lat;

                // Apply scaling factors for each dimension and convert to Minecraft coordinates
                let x: i32 = (rel_x * self.scale_factor_x) as i32;
                let z: i32 = (rel_z * self.scale_factor_z) as i32;

                XZPoint::new(x, z)
            }
            ProjectionMode::WebMercator {
                origin_lon,
                scale,
                cos_lat_ref,
                z_offset,
                ..
            } => {
                let x =
                    EARTH_RADIUS * (llpoint.lng() - origin_lon).to_radians() * cos_lat_ref * scale;
                let z = -EARTH_RADIUS
                    * (std::f64::consts::FRAC_PI_4 + llpoint.lat().to_radians() / 2.0)
                        .tan()
                        .ln()
                    * scale
                    + z_offset;

                XZPoint::new(x as i32, z as i32)
            }
        }
    }
}

// (lat meters, lon meters)
#[inline]
pub fn geo_distance(a: LLPoint, b: LLPoint) -> (f64, f64) {
    let z: f64 = lat_distance(a.lat(), b.lat());

    // distance between two lons depends on their latitude. In this case we'll just average them
    let x: f64 = lon_distance((a.lat() + b.lat()) / 2.0, a.lng(), b.lng());

    (z, x)
}

// Haversine but optimized for a latitude delta of 0
// returns meters
fn lon_distance(lat: f64, lon1: f64, lon2: f64) -> f64 {
    const R: f64 = 6_371_000.0;
    let d_lon: f64 = (lon2 - lon1).to_radians();
    let a: f64 =
        lat.to_radians().cos() * lat.to_radians().cos() * (d_lon / 2.0).sin() * (d_lon / 2.0).sin();
    let c: f64 = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());

    R * c
}

// Haversine but optimized for a longitude delta of 0
// returns meters
fn lat_distance(lat1: f64, lat2: f64) -> f64 {
    const R: f64 = 6_371_000.0;
    let d_lat: f64 = (lat2 - lat1).to_radians();
    let a: f64 = (d_lat / 2.0).sin() * (d_lat / 2.0).sin();
    let c: f64 = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());

    R * c
}

// copied legacy code
// Function to convert latitude and longitude to Minecraft coordinates.
#[cfg(test)]
pub fn lat_lon_to_minecraft_coords(
    lat: f64,
    lon: f64,
    bbox: LLBBox, // (min_lon, min_lat, max_lon, max_lat)
    scale_factor_z: f64,
    scale_factor_x: f64,
) -> (i32, i32) {
    // Calculate the relative position within the bounding box
    let rel_x: f64 = (lon - bbox.min().lng()) / (bbox.max().lng() - bbox.min().lng());
    let rel_z: f64 = 1.0 - (lat - bbox.min().lat()) / (bbox.max().lat() - bbox.min().lat());

    // Apply scaling factors for each dimension and convert to Minecraft coordinates
    let x: i32 = (rel_x * scale_factor_x) as i32;
    let z: i32 = (rel_z * scale_factor_z) as i32;

    (x, z)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_utilities::get_llbbox_arnis;

    fn test_llxztransform_one_scale_one_factor(
        scale: f64,
        test_latfactor: f64,
        test_lngfactor: f64,
    ) {
        let llbbox = get_llbbox_arnis();
        let llpoint = LLPoint::new(
            llbbox.min().lat() + (llbbox.max().lat() - llbbox.min().lat()) * test_latfactor,
            llbbox.min().lng() + (llbbox.max().lng() - llbbox.min().lng()) * test_lngfactor,
        )
        .unwrap();
        let (transformer, xzbbox_new) = CoordTransformer::llbbox_to_xzbbox(&llbbox, scale).unwrap();

        // legacy xzbbox creation
        let (scale_factor_z, scale_factor_x) = geo_distance(llbbox.min(), llbbox.max());
        let scale_factor_z: f64 = scale_factor_z.floor() * scale;
        let scale_factor_x: f64 = scale_factor_x.floor() * scale;
        let xzbbox_old = XZBBox::rect_from_xz_lengths(scale_factor_x, scale_factor_z).unwrap();

        // legacy coord transform
        let (x, z) = lat_lon_to_minecraft_coords(
            llpoint.lat(),
            llpoint.lng(),
            llbbox,
            scale_factor_z,
            scale_factor_x,
        );
        // new coord transform
        let xzpoint = transformer.transform_point(llpoint);

        assert_eq!(x, xzpoint.x);
        assert_eq!(z, xzpoint.z);
        assert_eq!(xzbbox_new.min_x(), xzbbox_old.min_x());
        assert_eq!(xzbbox_new.max_x(), xzbbox_old.max_x());
        assert_eq!(xzbbox_new.min_z(), xzbbox_old.min_z());
        assert_eq!(xzbbox_new.max_z(), xzbbox_old.max_z());
    }

    // this ensures that transformer.transform_point == legacy lat_lon_to_minecraft_coords
    #[test]
    pub fn test_llxztransform() {
        test_llxztransform_one_scale_one_factor(1.0, 0.5, 0.5);
        test_llxztransform_one_scale_one_factor(3.0, 0.1, 0.2);
        test_llxztransform_one_scale_one_factor(10.0, -1.2, 2.0);
        test_llxztransform_one_scale_one_factor(0.4, 0.3, -0.2);
        test_llxztransform_one_scale_one_factor(0.1, 0.2, 0.7);
    }

    // this ensures that invalid inputs can be handled correctly
    #[test]
    pub fn test_invalid_construct() {
        let llbbox = get_llbbox_arnis();
        let obj = CoordTransformer::llbbox_to_xzbbox(&llbbox, 0.0);
        assert!(obj.is_err());

        let obj = CoordTransformer::llbbox_to_xzbbox(&llbbox, -1.2);
        assert!(obj.is_err());
    }

    // ----- Web Mercator projection mode tests -----

    #[test]
    fn test_with_projection_constructs_successfully() {
        let llbbox = get_llbbox_arnis();
        let proj = crate::projection::WebMercatorProjection::new(
            (llbbox.min().lat() + llbbox.max().lat()) / 2.0,
            (llbbox.min().lng() + llbbox.max().lng()) / 2.0,
            1.0,
        );
        let result = CoordTransformer::with_projection(&llbbox, 1.0, &proj);
        assert!(result.is_ok());
    }

    #[test]
    fn test_with_projection_invalid_scale() {
        let llbbox = get_llbbox_arnis();
        let proj = crate::projection::WebMercatorProjection::new(54.63, 9.93, 1.0);

        assert!(CoordTransformer::with_projection(&llbbox, 0.0, &proj).is_err());
        assert!(CoordTransformer::with_projection(&llbbox, -1.0, &proj).is_err());
    }

    #[test]
    fn test_with_projection_xzbbox_contains_projected_corners() {
        let llbbox = get_llbbox_arnis();
        let proj = crate::projection::WebMercatorProjection::new(
            (llbbox.min().lat() + llbbox.max().lat()) / 2.0,
            (llbbox.min().lng() + llbbox.max().lng()) / 2.0,
            1.0,
        );
        let (transformer, xzbbox) = CoordTransformer::with_projection(&llbbox, 1.0, &proj).unwrap();

        // All four corners should map inside the xzbbox
        let corners = [
            LLPoint::new(llbbox.min().lat(), llbbox.min().lng()).unwrap(),
            LLPoint::new(llbbox.min().lat(), llbbox.max().lng()).unwrap(),
            LLPoint::new(llbbox.max().lat(), llbbox.min().lng()).unwrap(),
            LLPoint::new(llbbox.max().lat(), llbbox.max().lng()).unwrap(),
        ];

        for corner in &corners {
            let pt = transformer.transform_point(*corner);
            assert!(
                pt.x >= xzbbox.min_x() && pt.x <= xzbbox.max_x(),
                "x={} out of xzbbox [{}, {}] for corner ({}, {})",
                pt.x,
                xzbbox.min_x(),
                xzbbox.max_x(),
                corner.lat(),
                corner.lng(),
            );
            assert!(
                pt.z >= xzbbox.min_z() && pt.z <= xzbbox.max_z(),
                "z={} out of xzbbox [{}, {}] for corner ({}, {})",
                pt.z,
                xzbbox.min_z(),
                xzbbox.max_z(),
                corner.lat(),
                corner.lng(),
            );
        }
    }

    #[test]
    fn test_with_projection_matches_standalone_projection() {
        // Verify that CoordTransformer in WebMercator mode produces the same
        // result as calling WebMercatorProjection::forward directly.
        let llbbox = get_llbbox_arnis();
        let origin_lat = (llbbox.min().lat() + llbbox.max().lat()) / 2.0;
        let origin_lon = (llbbox.min().lng() + llbbox.max().lng()) / 2.0;
        let proj = crate::projection::WebMercatorProjection::new(origin_lat, origin_lon, 1.0);
        let (transformer, _) = CoordTransformer::with_projection(&llbbox, 1.0, &proj).unwrap();

        let test_point = LLPoint::new(
            llbbox.min().lat() + (llbbox.max().lat() - llbbox.min().lat()) * 0.3,
            llbbox.min().lng() + (llbbox.max().lng() - llbbox.min().lng()) * 0.7,
        )
        .unwrap();

        let pt = transformer.transform_point(test_point);
        let (expected_x, expected_z) =
            crate::projection::Projection::forward(&proj, test_point.lat(), test_point.lng());

        // Integer truncation: the transformer casts with `as i32`
        assert_eq!(pt.x, expected_x as i32);
        assert_eq!(pt.z, expected_z as i32);
    }

    #[test]
    fn test_with_projection_east_increases_x() {
        let llbbox = get_llbbox_arnis();
        let proj = crate::projection::WebMercatorProjection::new(54.63, 9.93, 1.0);
        let (transformer, _) = CoordTransformer::with_projection(&llbbox, 1.0, &proj).unwrap();

        let west = LLPoint::new(54.63, 9.928).unwrap();
        let east = LLPoint::new(54.63, 9.937).unwrap();

        let pw = transformer.transform_point(west);
        let pe = transformer.transform_point(east);
        assert!(
            pe.x > pw.x,
            "east should have larger x: west.x={}, east.x={}",
            pw.x,
            pe.x,
        );
    }

    #[test]
    fn test_with_projection_north_decreases_z() {
        let llbbox = get_llbbox_arnis();
        let proj = crate::projection::WebMercatorProjection::new(54.63, 9.93, 1.0);
        let (transformer, _) = CoordTransformer::with_projection(&llbbox, 1.0, &proj).unwrap();

        let south = LLPoint::new(54.628, 9.93).unwrap();
        let north = LLPoint::new(54.634, 9.93).unwrap();

        let ps = transformer.transform_point(south);
        let pn = transformer.transform_point(north);
        assert!(
            pn.z < ps.z,
            "north should have smaller z: south.z={}, north.z={}",
            ps.z,
            pn.z,
        );
    }

    #[test]
    fn test_local_mode_unaffected_by_projection_addition() {
        // Double-check that the Local path is bit-identical to pre-change behavior.
        let llbbox = get_llbbox_arnis();
        let (transformer, _) = CoordTransformer::llbbox_to_xzbbox(&llbbox, 1.0).unwrap();

        let (scale_factor_z, scale_factor_x) = geo_distance(llbbox.min(), llbbox.max());
        let scale_factor_z = scale_factor_z.floor();
        let scale_factor_x = scale_factor_x.floor();

        let llpoint = LLPoint::new(
            llbbox.min().lat() + (llbbox.max().lat() - llbbox.min().lat()) * 0.5,
            llbbox.min().lng() + (llbbox.max().lng() - llbbox.min().lng()) * 0.5,
        )
        .unwrap();

        let (expected_x, expected_z) = lat_lon_to_minecraft_coords(
            llpoint.lat(),
            llpoint.lng(),
            llbbox,
            scale_factor_z,
            scale_factor_x,
        );

        let pt = transformer.transform_point(llpoint);
        assert_eq!(pt.x, expected_x);
        assert_eq!(pt.z, expected_z);
    }
}
