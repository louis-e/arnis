//! Transverse Mercator by Krüger's series: UTM, and national grids that are
//! Transverse Mercator on an ellipsoid of their own (see `crs::datum`).
//!
//! Sixth order in the third flattening, which is good to well under a millimetre
//! inside a zone (Karney 2011). ETRS89 and WGS84 differ by well under a metre
//! and a block is a metre, so the national ETRS89 grids (EPSG:25832, 25833)
//! take the same formulas.

const A: f64 = 6_378_137.0;
const F: f64 = 1.0 / 298.257_223_563;
const K0: f64 = 0.9996;
const FALSE_EASTING: f64 = 500_000.0;

/// A Transverse Mercator grid: central meridian, latitude of origin, scale and
/// false origin. The ellipsoid is the [`Series`] it is used with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tm {
    pub lon0: f64,
    pub lat0: f64,
    pub k0: f64,
    pub fe: f64,
    pub fn_: f64,
}

pub struct Series {
    big_a: f64,
    alpha: [f64; 6],
    beta: [f64; 6],
    e: f64,
}

impl Series {
    pub fn new(a: f64, f: f64) -> Self {
        let n = f / (2.0 - f);
        let n2 = n * n;
        let n3 = n2 * n;
        let n4 = n3 * n;
        let n5 = n4 * n;
        let n6 = n5 * n;
        let big_a = a / (1.0 + n) * (1.0 + n2 / 4.0 + n4 / 64.0 + n6 / 256.0);
        let alpha = [
            n / 2.0 - 2.0 * n2 / 3.0 + 5.0 * n3 / 16.0 + 41.0 * n4 / 180.0 - 127.0 * n5 / 288.0
                + 7891.0 * n6 / 37800.0,
            13.0 * n2 / 48.0 - 3.0 * n3 / 5.0 + 557.0 * n4 / 1440.0 + 281.0 * n5 / 630.0
                - 1983433.0 * n6 / 1935360.0,
            61.0 * n3 / 240.0 - 103.0 * n4 / 140.0
                + 15061.0 * n5 / 26880.0
                + 167603.0 * n6 / 181440.0,
            49561.0 * n4 / 161280.0 - 179.0 * n5 / 168.0 + 6601661.0 * n6 / 7257600.0,
            34729.0 * n5 / 80640.0 - 3418889.0 * n6 / 1995840.0,
            212378941.0 * n6 / 319334400.0,
        ];
        let beta = [
            n / 2.0 - 2.0 * n2 / 3.0 + 37.0 * n3 / 96.0 - n4 / 360.0 - 81.0 * n5 / 512.0
                + 96199.0 * n6 / 604800.0,
            n2 / 48.0 + n3 / 15.0 - 437.0 * n4 / 1440.0 + 46.0 * n5 / 105.0
                - 1118711.0 * n6 / 3870720.0,
            17.0 * n3 / 480.0 - 37.0 * n4 / 840.0 - 209.0 * n5 / 4480.0 + 5569.0 * n6 / 90720.0,
            4397.0 * n4 / 161280.0 - 11.0 * n5 / 504.0 - 830251.0 * n6 / 7257600.0,
            4583.0 * n5 / 161280.0 - 108847.0 * n6 / 3991680.0,
            20648693.0 * n6 / 638668800.0,
        ];
        let e = (f * (2.0 - f)).sqrt();
        Series {
            big_a,
            alpha,
            beta,
            e,
        }
    }

    /// Latitude in degrees and longitude from the central meridian in radians
    /// to (eta, xi), the grid's coordinates before the scale and the origin.
    fn forward(&self, lat: f64, lam: f64) -> (f64, f64) {
        let phi = lat.to_radians();
        let t = (phi.sin().atanh() - self.e * (self.e * phi.sin()).atanh()).sinh();
        let xi_p = t.atan2(lam.cos());
        let eta_p = (lam.sin() / (1.0 + t * t).sqrt()).atanh();
        let mut xi = xi_p;
        let mut eta = eta_p;
        for (j, a) in self.alpha.iter().enumerate() {
            let k = 2.0 * (j as f64 + 1.0);
            xi += a * (k * xi_p).sin() * (k * eta_p).cosh();
            eta += a * (k * xi_p).cos() * (k * eta_p).sinh();
        }
        (eta, xi)
    }

    /// The inverse of [`Series::forward`]: latitude in degrees and longitude
    /// from the central meridian in radians.
    fn inverse(&self, eta: f64, xi: f64) -> (f64, f64) {
        let mut xi_p = xi;
        let mut eta_p = eta;
        for (j, b) in self.beta.iter().enumerate() {
            let k = 2.0 * (j as f64 + 1.0);
            xi_p -= b * (k * xi).sin() * (k * eta).cosh();
            eta_p -= b * (k * xi).cos() * (k * eta).sinh();
        }
        let tau_p = xi_p.sin() / (eta_p.sinh().powi(2) + xi_p.cos().powi(2)).sqrt();
        // Newton on tau = tan(phi) from tau' (Karney 2011, eq. 19-21).
        let e = self.e;
        let mut tau = tau_p;
        for _ in 0..5 {
            let sigma = (e * (e * tau / (1.0 + tau * tau).sqrt()).atanh()).sinh();
            let tau_i = tau * (1.0 + sigma * sigma).sqrt() - sigma * (1.0 + tau * tau).sqrt();
            let d = (tau_p - tau_i) / (1.0 + tau_i * tau_i).sqrt()
                * (1.0 + (1.0 - e * e) * tau * tau)
                / ((1.0 - e * e) * (1.0 + tau * tau).sqrt());
            tau += d;
            if d.abs() < 1e-14 {
                break;
            }
        }
        (tau.atan().to_degrees(), eta_p.sinh().atan2(xi_p.cos()))
    }
}

fn series() -> &'static Series {
    use std::sync::OnceLock;
    static S: OnceLock<Series> = OnceLock::new();
    S.get_or_init(|| Series::new(A, F))
}

impl Tm {
    /// xi of the latitude of origin.
    fn xi0(&self, s: &Series) -> f64 {
        if self.lat0 == 0.0 {
            0.0
        } else {
            s.forward(self.lat0, 0.0).1
        }
    }

    /// Latitude and longitude in degrees to (easting, northing).
    pub fn forward(&self, s: &Series, lat: f64, lon: f64) -> (f64, f64) {
        let (eta, xi) = s.forward(lat, lon.to_radians() - self.lon0.to_radians());
        (
            self.fe + self.k0 * s.big_a * eta,
            self.fn_ + self.k0 * s.big_a * (xi - self.xi0(s)),
        )
    }

    /// (easting, northing) to latitude and longitude in degrees.
    pub fn inverse(&self, s: &Series, e: f64, n: f64) -> (f64, f64) {
        let xi = (n - self.fn_) / (self.k0 * s.big_a) + self.xi0(s);
        let eta = (e - self.fe) / (self.k0 * s.big_a);
        let (lat, lam) = s.inverse(eta, xi);
        (lat, (self.lon0.to_radians() + lam).to_degrees())
    }
}

fn utm(zone: u8) -> Tm {
    Tm {
        lon0: f64::from(zone) * 6.0 - 183.0,
        lat0: 0.0,
        k0: K0,
        fe: FALSE_EASTING,
        fn_: 0.0,
    }
}

/// Latitude and longitude in degrees to (easting, northing) in a northern UTM zone.
pub fn to_utm(lat: f64, lon: f64, zone: u8) -> (f64, f64) {
    utm(zone).forward(series(), lat, lon)
}

/// (easting, northing) in a northern UTM zone to latitude and longitude in degrees.
pub fn from_utm(easting: f64, northing: f64, zone: u8) -> (f64, f64) {
    utm(zone).inverse(series(), easting, northing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_proj_in_zone_32() {
        // Reference values from PROJ (EPSG:4326 -> EPSG:25832).
        let (e, n) = to_utm(48.3000481, 11.8981969, 32);
        assert!((e - 714_925.411_449_793_1).abs() < 0.001, "{e}");
        assert!((n - 5_353_710.414_002_833).abs() < 0.001, "{n}");
    }

    #[test]
    fn round_trips() {
        for &(lat, lon, zone) in &[(48.3, 11.9, 32), (52.5, 13.4, 33), (47.6, 7.6, 32)] {
            let (e, n) = to_utm(lat, lon, zone);
            let (la, lo) = from_utm(e, n, zone);
            assert!(
                (la - lat).abs() < 1e-9 && (lo - lon).abs() < 1e-9,
                "{la} {lo}"
            );
        }
    }
}
