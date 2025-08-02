use super::cartesian::{XZBBox, XZPoint};
use super::geographic::{LLBBox, LLPoint};

/// Transform geographic space (within llbbox) to a local tangential cartesian space (within xzbbox)
pub struct CoordTransformer {
    len_lat: f64,
    len_lng: f64,
    scale_factor_x: f64,
    scale_factor_z: f64,
    min_lat: f64,
    min_lng: f64,
    rotation_angle: f64,
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
        rotation_angle: f64,
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
                rotation_angle
            },
            xzbbox,
        ))
    }

    pub fn transform_point(&self, llpoint: LLPoint) -> XZPoint {
        // Calculate the relative position within the bounding box
        let rel_x: f64 = (llpoint.lng() - self.min_lng) / self.len_lng;
        let rel_z: f64 = 1.0 - (llpoint.lat() - self.min_lat) / self.len_lat;

        let scaled_x: f64 = rel_x * self.scale_factor_x;
        let scaled_z: f64 = rel_z * self.scale_factor_z;

        let sin_rot: f64 = f64::sin(self.rotation_angle * std::f64::consts::PI / 180.0);
        let cos_rot: f64 = f64::cos(self.rotation_angle * std::f64::consts::PI / 180.0);

        let rotated_rel_x: f64 = scaled_x * cos_rot - scaled_z * sin_rot;
        let rotated_rel_z: f64 = scaled_z * cos_rot + scaled_x * sin_rot;

        // Apply scaling factors for each dimension and convert to Minecraft coordinates
        let x: i32 = rotated_rel_x as i32;
        let z: i32 = rotated_rel_z as i32;

        XZPoint::new(x, z)
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
}
