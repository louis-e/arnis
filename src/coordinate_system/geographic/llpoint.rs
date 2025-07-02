/// Bounds-checked longitude and latitude.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LLPoint {
    lat: f64,
    lng: f64,
}

impl LLPoint {
    pub fn new(lat: f64, lng: f64) -> Result<Self, String> {
        let lat_in_range = (-90.0..=90.0).contains(&lat) && (-90.0..=90.0).contains(&lat);
        let lng_in_range = (-180.0..=180.0).contains(&lng) && (-180.0..=180.0).contains(&lng);

        if !lat_in_range {
            return Err(format!("Latitude {lat} not in range -90.0..=90.0"));
        }

        if !lng_in_range {
            return Err(format!("Longitude {lng} not in range -180.0..=180.0"));
        }

        Ok(Self { lat, lng })
    }

    pub fn lat(&self) -> f64 {
        self.lat
    }

    pub fn lng(&self) -> f64 {
        self.lng
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_input() {
        assert!(LLPoint::new(0., 0.).is_ok());

        // latitude extremes
        assert!(LLPoint::new(-90.0, 0.).is_ok());
        assert!(LLPoint::new(90.0, 0.).is_ok());

        // longitude extremes
        assert!(LLPoint::new(0., -180.0).is_ok());
        assert!(LLPoint::new(0., 180.0).is_ok());
    }

    #[test]
    fn test_out_of_bounds() {
        // latitude out-of-bounds
        assert!(LLPoint::new(-91., 0.).is_err());
        assert!(LLPoint::new(91., 0.).is_err());

        // longitude out-of-bounds
        assert!(LLPoint::new(0., -181.).is_err());
        assert!(LLPoint::new(0., 181.).is_err());
    }
}
