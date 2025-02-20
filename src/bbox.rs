#[derive(Copy, Clone, Debug)]
pub struct GeoCoordinate {
    pub lat: f64,
    pub lng: f64,
}

/// A Bounding Box, which is guaranteed to be correct
/// (since you can't construct it any other way besides BBox::new()).
/// Unfortunately, that means you must use getters for each member.
/// Don't worry, I'm 99% sure they'll optimize away.
#[derive(Copy, Clone, Debug)]
pub struct BBox {
    min: GeoCoordinate,
    max: GeoCoordinate,
}

impl BBox {
    pub fn new(min_lat: f64, min_lng: f64, max_lat: f64, max_lng: f64) -> Result<Self, String> {
        let lng_in_range =
            (-180.0..=180.0).contains(&min_lng) && (-180.0..=180.0).contains(&max_lng);
        let lat_in_range = (-90.0..=90.0).contains(&min_lat) && (-90.0..=90.0).contains(&max_lat);
        let vals_in_order = min_lng < max_lng && min_lat < max_lat;

        if lng_in_range && lat_in_range && vals_in_order {
            return Ok(Self {
                min: GeoCoordinate {
                    lat: min_lat,
                    lng: min_lng,
                },
                max: GeoCoordinate {
                    lat: max_lat,
                    lng: max_lng,
                },
            });
        }

        Err("Invalid BBox".to_string())
    }

    pub fn from_str(s: &str) -> Result<Self, String> {
        let [min_lat, min_lng, max_lat, max_lng]: [f64; 4] = s
            .split(',')
            .map(|e| e.parse().unwrap())
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();

        Self::new(min_lat, min_lng, max_lat, max_lng)
    }

    pub fn min(&self) -> GeoCoordinate {
        self.min
    }

    pub fn max(&self) -> GeoCoordinate {
        self.max
    }
}
