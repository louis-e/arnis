/// Bounds-checked longitude and latitude.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GeoCoord {
    lng: f64,
    lat: f64,
}

impl GeoCoord {
    pub fn new(lng: f64, lat: f64) -> Result<Self, String> {
        let lng_in_range = (-180.0..=180.0).contains(&lng) && (-180.0..=180.0).contains(&lng);
        let lat_in_range = (-90.0..=90.0).contains(&lat) && (-90.0..=90.0).contains(&lat);

        if !lng_in_range {
            return Err(format!("Longitude {} not in range -180.0..=180.0", lng));
        }

        if !lat_in_range {
            return Err(format!("Latitude {} not in range -90.0..=90.0", lat));
        }

        Ok(Self { lng, lat })
    }

    pub fn lng(&self) -> f64 {
        self.lng
    }

    pub fn lat(&self) -> f64 {
        self.lat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_input() {
        assert!(GeoCoord::new(0., 0.).is_ok());

        // longitude extremes
        assert!(GeoCoord::new(-180.0, 0.).is_ok());
        assert!(GeoCoord::new(180.0, 0.).is_ok());

        // latitude extremes
        assert!(GeoCoord::new(0., -90.0).is_ok());
        assert!(GeoCoord::new(0., 90.0).is_ok());
    }

    #[test]
    fn test_out_of_bounds() {
        // longitude out-of-bounds
        assert!(GeoCoord::new(-181., 0.).is_err());
        assert!(GeoCoord::new(181., 0.).is_err());

        // latitude out-of-bounds
        assert!(GeoCoord::new(0., -91.).is_err());
        assert!(GeoCoord::new(0., 91.).is_err());
    }
}
