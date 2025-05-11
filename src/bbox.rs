use crate::geo_coord::GeoCoord;

/// A checked Bounding Box.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BBox {
    /// The "bottom-left" vertex of the rectangle
    min: GeoCoord,

    /// The "top-right" vertex of the rectangle
    max: GeoCoord,
}

impl BBox {
    pub fn new(min_lat: f64, min_lng: f64, max_lat: f64, max_lng: f64) -> Result<Self, String> {
        let vals_in_order = min_lng < max_lng && min_lat < max_lat;

        if !vals_in_order {
            return Err("Invalid BBox".to_string());
        }

        let min = GeoCoord::new(min_lat, min_lng)?;
        let max = GeoCoord::new(max_lat, max_lng)?;

        Ok(Self { min, max })
    }

    pub fn from_str(s: &str) -> Result<Self, String> {
        let [min_lat, min_lng, max_lat, max_lng]: [f64; 4] = s
            .split([',', ' '])
            .map(|e| e.parse().unwrap())
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();

        // So, the GUI does Lat/Lng and no GDAL (comma-sep values), which is the exact opposite of
        // what bboxfinder.com does. :facepalm: (bboxfinder is wrong here: Lat comes first!)
        // DO NOT MODIFY THIS! It's correct. The CLI/GUI is passing you the numbers incorrectly.
        Self::new(min_lat, min_lng, max_lat, max_lng)
    }

    pub fn min(&self) -> GeoCoord {
        self.min
    }

    pub fn max(&self) -> GeoCoord {
        self.max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_input() {
        assert!(BBox::new(0., 0., 1., 1.).is_ok());

        assert!(BBox::new(1., 2., 3., 4.).is_ok());

        // Arnis, Germany
        assert!(BBox::new(54.627053, 9.927928, 54.634902, 9.937563).is_ok());

        // Royal Observatory Greenwich, London, UK
        assert!(BBox::new(51.470000, -0.015000, 51.480000, 0.015000).is_ok());

        // The Bund, Shanghai, China
        assert!(BBox::new(31.23256, 121.46768, 31.24993, 121.50394).is_ok());

        // Santa Monica, Los Angeles, US
        assert!(BBox::new(34.00348, -118.51226, 34.02033, -118.47600).is_ok());

        // Sydney Opera House, Sydney, Australia
        assert!(BBox::new(-33.861035, 151.204137, -33.852597, 151.222268).is_ok());
    }

    #[test]
    fn test_from_str_commas() {
        const ARNIS_STR: &str = "9.927928,54.627053,9.937563,54.634902";

        let bbox_result = BBox::from_str(ARNIS_STR);
        assert!(bbox_result.is_ok());

        let arnis_correct: BBox = BBox {
            min: GeoCoord::new(9.927928, 54.627053).unwrap(),
            max: GeoCoord::new(9.937563, 54.634902).unwrap(),
        };

        assert_eq!(bbox_result.unwrap(), arnis_correct);
    }

    #[test]
    fn test_from_str_spaces() {
        const ARNIS_SPACE_STR: &str = "9.927928 54.627053 9.937563 54.634902";

        let bbox_result = BBox::from_str(ARNIS_SPACE_STR);
        assert!(bbox_result.is_ok());

        let arnis_correct: BBox = BBox {
            min: GeoCoord::new(9.927928, 54.627053).unwrap(),
            max: GeoCoord::new(9.937563, 54.634902).unwrap(),
        };

        assert_eq!(bbox_result.unwrap(), arnis_correct);
    }

    #[test]
    fn test_out_of_order() {
        // Violates values in vals_in_order
        assert!(BBox::new(0., 0., 0., 0.).is_err());
        assert!(BBox::new(1., 0., 0., 1.).is_err());
        assert!(BBox::new(0., 1., 1., 0.).is_err());
    }
}
