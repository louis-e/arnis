//! The grids the survey sources publish on, to and from WGS84 latitude and
//! longitude.
//!
//! A block is a metre, so a projection good to a few decimetres is plenty:
//!
//! * UTM by Krüger's series ([`super::utm`]), well under a millimetre;
//! * Lambert-93 (EPSG:2154), the exact conic on GRS80. RGF93 and WGS84 agree to
//!   a few centimetres;
//! * RD New (EPSG:28992) by Schreutelkamp and Strang van Hees' polynomials, which
//!   fold in the Bessel datum shift and keep within a metre of RDNAPTRANS over
//!   the Netherlands;
//! * Swiss LV95 (EPSG:2056) by swisstopo's approximate formulas, within about a
//!   metre over Switzerland;
//! * Estonia's L-EST97 (EPSG:3301), the same conic on its own parallels;
//! * Luxembourg's LUREF / Luxembourg TM (EPSG:2169), Transverse Mercator on
//!   the International ellipsoid after EPSG's seven-parameter shift from WGS84
//!   (LUREF to WGS 84 (3)), which PROJ rates at a metre.

use super::utm;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crs {
    /// Northern UTM zone (ETRS89 / WGS84).
    Utm(u8),
    Lambert93,
    RdNew,
    Lv95,
    /// Estonia's L-EST97.
    Lest97,
    /// Luxembourg's LUREF / Luxembourg TM.
    Luref,
}

impl Crs {
    /// Latitude and longitude in degrees to (easting, northing) in metres.
    pub fn forward(self, lat: f64, lon: f64) -> (f64, f64) {
        match self {
            Crs::Utm(zone) => utm::to_utm(lat, lon, zone),
            Crs::Lambert93 => lambert::lambert93().forward(lat, lon),
            Crs::RdNew => rd::forward(lat, lon),
            Crs::Lv95 => lv95::forward(lat, lon),
            Crs::Lest97 => lambert::lest97().forward(lat, lon),
            Crs::Luref => datum::luref().forward(lat, lon),
        }
    }

    /// (easting, northing) in metres to latitude and longitude in degrees.
    pub fn inverse(self, x: f64, y: f64) -> (f64, f64) {
        match self {
            Crs::Utm(zone) => utm::from_utm(x, y, zone),
            Crs::Lambert93 => lambert::lambert93().inverse(x, y),
            Crs::RdNew => rd::inverse(x, y),
            Crs::Lv95 => lv95::inverse(x, y),
            Crs::Lest97 => lambert::lest97().inverse(x, y),
            Crs::Luref => datum::luref().inverse(x, y),
        }
    }

    /// The EPSG code, for services that take one.
    pub fn epsg(self) -> u32 {
        match self {
            Crs::Utm(zone) => 25800 + u32::from(zone),
            Crs::Lambert93 => 2154,
            Crs::RdNew => 28992,
            Crs::Lv95 => 2056,
            Crs::Lest97 => 3301,
            Crs::Luref => 2169,
        }
    }

    /// Whether the grid's EPSG definition puts northing first, as WMS 1.3.0
    /// then wants its bounding boxes.
    pub fn north_first(self) -> bool {
        matches!(self, Crs::Lest97 | Crs::Luref)
    }
}

/// Lambert conformal conic with two standard parallels on GRS80.
mod lambert {
    use std::f64::consts::FRAC_PI_4;

    const A: f64 = 6_378_137.0;
    const F: f64 = 1.0 / 298.257_222_101;

    /// A conic's defining parameters, in degrees and metres.
    pub struct Params {
        pub lat0: f64,
        pub lat1: f64,
        pub lat2: f64,
        pub lon0: f64,
        pub x0: f64,
        pub y0: f64,
    }

    /// Lambert-93 (EPSG:2154).
    pub const LAMBERT93: Params = Params {
        lat0: 46.5,
        lat1: 49.0,
        lat2: 44.0,
        lon0: 3.0,
        x0: 700_000.0,
        y0: 6_600_000.0,
    };

    /// L-EST97 (EPSG:3301).
    pub const LEST97: Params = Params {
        lat0: 57.517_553_930_555_56,
        lat1: 59.333_333_333_333_34,
        lat2: 58.0,
        lon0: 24.0,
        x0: 500_000.0,
        y0: 6_375_000.0,
    };

    pub struct Cone {
        p: &'static Params,
        e: f64,
        n: f64,
        af: f64,
        rho0: f64,
    }

    pub fn cone(p: &'static Params) -> Cone {
        let e = (F * (2.0 - F)).sqrt();
        let m = |phi: f64| phi.cos() / (1.0 - (e * phi.sin()).powi(2)).sqrt();
        let (p0, p1, p2) = (
            p.lat0.to_radians(),
            p.lat1.to_radians(),
            p.lat2.to_radians(),
        );
        let n = (m(p1).ln() - m(p2).ln()) / (t(e, p1).ln() - t(e, p2).ln());
        let af = A * m(p1) / (n * t(e, p1).powf(n));
        Cone {
            p,
            e,
            n,
            af,
            rho0: af * t(e, p0).powf(n),
        }
    }

    fn t(e: f64, phi: f64) -> f64 {
        let es = e * phi.sin();
        (FRAC_PI_4 - phi / 2.0).tan() / ((1.0 - es) / (1.0 + es)).powf(e / 2.0)
    }

    impl Cone {
        pub fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
            let rho = self.af * t(self.e, lat.to_radians()).powf(self.n);
            let theta = self.n * (lon - self.p.lon0).to_radians();
            (
                self.p.x0 + rho * theta.sin(),
                self.p.y0 + self.rho0 - rho * theta.cos(),
            )
        }

        pub fn inverse(&self, x: f64, y: f64) -> (f64, f64) {
            let (dx, dy) = (x - self.p.x0, self.rho0 - (y - self.p.y0));
            let rho = (dx * dx + dy * dy).sqrt();
            let tp = (rho / self.af).powf(1.0 / self.n);
            let lon = self.p.lon0 + (dx.atan2(dy) / self.n).to_degrees();
            let mut phi = std::f64::consts::FRAC_PI_2 - 2.0 * tp.atan();
            for _ in 0..10 {
                let es = self.e * phi.sin();
                let next = std::f64::consts::FRAC_PI_2
                    - 2.0 * (tp * ((1.0 - es) / (1.0 + es)).powf(self.e / 2.0)).atan();
                let done = (next - phi).abs() < 1e-13;
                phi = next;
                if done {
                    break;
                }
            }
            (phi.to_degrees(), lon)
        }
    }

    pub fn lambert93() -> &'static Cone {
        use std::sync::OnceLock;
        static C: OnceLock<Cone> = OnceLock::new();
        C.get_or_init(|| cone(&LAMBERT93))
    }

    pub fn lest97() -> &'static Cone {
        use std::sync::OnceLock;
        static C: OnceLock<Cone> = OnceLock::new();
        C.get_or_init(|| cone(&LEST97))
    }
}

/// Grids on a datum of their own: WGS84 latitude and longitude are shifted
/// onto the local ellipsoid by a seven-parameter (Helmert) transformation,
/// then projected.
mod datum {
    use super::utm::{Series, Tm};

    const WGS84_A: f64 = 6_378_137.0;
    const WGS84_F: f64 = 1.0 / 298.257_223_563;

    /// A Helmert transformation from the local datum to WGS84, in EPSG's
    /// coordinate frame convention: metres, arc seconds, parts per million.
    pub struct Helmert {
        pub t: [f64; 3],
        pub r: [f64; 3],
        pub s: f64,
    }

    impl Helmert {
        fn rotation(&self) -> [[f64; 3]; 3] {
            let [rx, ry, rz] = self.r.map(|v| (v / 3600.0).to_radians());
            [[1.0, rz, -ry], [-rz, 1.0, rx], [ry, -rx, 1.0]]
        }

        /// Local cartesian to WGS84 cartesian.
        fn onto_wgs84(&self, p: [f64; 3]) -> [f64; 3] {
            let m = self.rotation();
            let k = 1.0 + self.s * 1e-6;
            std::array::from_fn(|i| {
                self.t[i] + k * (m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2])
            })
        }

        /// WGS84 cartesian to local cartesian: the transposed rotation, which
        /// for angles of seconds is its inverse to well under a millimetre.
        fn off_wgs84(&self, p: [f64; 3]) -> [f64; 3] {
            let m = self.rotation();
            let k = 1.0 + self.s * 1e-6;
            let d = [p[0] - self.t[0], p[1] - self.t[1], p[2] - self.t[2]];
            std::array::from_fn(|i| (m[0][i] * d[0] + m[1][i] * d[1] + m[2][i] * d[2]) / k)
        }
    }

    fn to_cart(a: f64, f: f64, lat: f64, lon: f64) -> [f64; 3] {
        let e2 = f * (2.0 - f);
        let (sp, cp) = lat.to_radians().sin_cos();
        let (sl, cl) = lon.to_radians().sin_cos();
        let nu = a / (1.0 - e2 * sp * sp).sqrt();
        [nu * cp * cl, nu * cp * sl, nu * (1.0 - e2) * sp]
    }

    fn from_cart(a: f64, f: f64, p: [f64; 3]) -> (f64, f64) {
        let e2 = f * (2.0 - f);
        let r = p[0].hypot(p[1]);
        let mut lat = p[2].atan2(r * (1.0 - e2));
        for _ in 0..6 {
            let sp = lat.sin();
            let nu = a / (1.0 - e2 * sp * sp).sqrt();
            lat = (p[2] + e2 * nu * sp).atan2(r);
        }
        (lat.to_degrees(), p[1].atan2(p[0]).to_degrees())
    }

    pub struct Grid {
        a: f64,
        f: f64,
        helmert: Helmert,
        tm: Tm,
        series: Series,
    }

    impl Grid {
        pub fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
            let p = self.helmert.off_wgs84(to_cart(WGS84_A, WGS84_F, lat, lon));
            let (la, lo) = from_cart(self.a, self.f, p);
            self.tm.forward(&self.series, la, lo)
        }

        pub fn inverse(&self, x: f64, y: f64) -> (f64, f64) {
            let (la, lo) = self.tm.inverse(&self.series, x, y);
            let p = self.helmert.onto_wgs84(to_cart(self.a, self.f, la, lo));
            from_cart(WGS84_A, WGS84_F, p)
        }
    }

    /// LUREF / Luxembourg TM (EPSG:2169) by LUREF to WGS 84 (3), EPSG:5486.
    pub fn luref() -> &'static Grid {
        use std::sync::OnceLock;
        static G: OnceLock<Grid> = OnceLock::new();
        G.get_or_init(|| {
            let (a, f) = (6_378_388.0, 1.0 / 297.0);
            Grid {
                a,
                f,
                helmert: Helmert {
                    t: [-189.6806, 18.3463, -42.7695],
                    r: [0.33746, 3.09264, -2.53861],
                    s: 0.4598,
                },
                tm: Tm {
                    lon0: 6.0 + 10.0 / 60.0,
                    lat0: 49.0 + 50.0 / 60.0,
                    k0: 1.0,
                    fe: 80_000.0,
                    fn_: 100_000.0,
                },
                series: Series::new(a, f),
            }
        })
    }
}

/// Dutch RD New from and to WGS84 by the published approximation polynomials.
mod rd {
    const X0: f64 = 155_000.0;
    const Y0: f64 = 463_000.0;
    const LAT0: f64 = 52.155_174_40;
    const LON0: f64 = 5.387_206_21;

    /// (coefficient, power of dLat, power of dLon) for x and y.
    const KX: &[(f64, i32, i32)] = &[
        (190_094.945, 0, 1),
        (-11_832.228, 1, 1),
        (-114.221, 2, 1),
        (-32.391, 0, 3),
        (-0.705, 1, 0),
        (-2.340, 3, 1),
        (-0.608, 1, 3),
        (-0.008, 0, 2),
        (0.148, 2, 3),
    ];
    const KY: &[(f64, i32, i32)] = &[
        (309_056.544, 1, 0),
        (3_638.893, 0, 2),
        (73.077, 2, 0),
        (-157.984, 1, 2),
        (59.788, 3, 0),
        (0.433, 0, 1),
        (-6.439, 2, 2),
        (-0.032, 1, 1),
        (0.092, 0, 4),
        (-0.054, 1, 4),
    ];
    /// (coefficient, power of dx, power of dy) for latitude and longitude, in
    /// arc seconds.
    const KLAT: &[(f64, i32, i32)] = &[
        (3_235.653_89, 0, 1),
        (-32.582_97, 2, 0),
        (-0.247_50, 0, 2),
        (-0.849_78, 2, 1),
        (-0.065_50, 0, 3),
        (-0.017_09, 2, 2),
        (-0.007_38, 1, 0),
        (0.005_30, 4, 0),
        (-0.000_39, 2, 3),
        (0.000_33, 4, 1),
        (-0.000_12, 1, 1),
    ];
    const KLON: &[(f64, i32, i32)] = &[
        (5_260.529_16, 1, 0),
        (105.946_84, 1, 1),
        (2.456_56, 1, 2),
        (-0.818_85, 3, 0),
        (0.055_94, 1, 3),
        (-0.056_07, 3, 1),
        (0.011_99, 0, 1),
        (-0.002_56, 3, 2),
        (0.001_28, 1, 4),
        (0.000_22, 0, 2),
        (-0.000_22, 2, 0),
        (0.000_26, 5, 0),
    ];

    fn poly(k: &[(f64, i32, i32)], a: f64, b: f64) -> f64 {
        k.iter().map(|&(c, p, q)| c * a.powi(p) * b.powi(q)).sum()
    }

    pub fn forward(lat: f64, lon: f64) -> (f64, f64) {
        let (dp, dl) = (0.36 * (lat - LAT0), 0.36 * (lon - LON0));
        (X0 + poly(KX, dp, dl), Y0 + poly(KY, dp, dl))
    }

    pub fn inverse(x: f64, y: f64) -> (f64, f64) {
        let (dx, dy) = ((x - X0) * 1e-5, (y - Y0) * 1e-5);
        (
            LAT0 + poly(KLAT, dx, dy) / 3600.0,
            LON0 + poly(KLON, dx, dy) / 3600.0,
        )
    }
}

/// Swiss LV95 by swisstopo's approximate formulas.
mod lv95 {
    pub fn forward(lat: f64, lon: f64) -> (f64, f64) {
        let p = (lat * 3600.0 - 169_028.66) / 10_000.0;
        let l = (lon * 3600.0 - 26_782.5) / 10_000.0;
        let e = 2_600_072.37 + 211_455.93 * l
            - 10_938.51 * l * p
            - 0.36 * l * p * p
            - 44.54 * l.powi(3);
        let n = 1_200_147.07 + 308_807.95 * p + 3_745.25 * l * l + 76.63 * p * p
            - 194.56 * l * l * p
            + 119.79 * p.powi(3);
        (e, n)
    }

    /// The published inverse is a couple of metres off at the country's edges, so
    /// it only starts a Newton iteration on the forward formulas, which keeps the
    /// round trip exact and the error that of the forward formulas.
    pub fn inverse(e: f64, n: f64) -> (f64, f64) {
        let (mut lat, mut lon) = approx_inverse(e, n);
        for _ in 0..3 {
            let (fe, fn_) = forward(lat, lon);
            let d = 1e-5;
            let (e_lat, n_lat) = forward(lat + d, lon);
            let (e_lon, n_lon) = forward(lat, lon + d);
            let (a, b) = ((e_lat - fe) / d, (e_lon - fe) / d);
            let (c, dd) = ((n_lat - fn_) / d, (n_lon - fn_) / d);
            let det = a * dd - b * c;
            let (re, rn) = (e - fe, n - fn_);
            lat += (dd * re - b * rn) / det;
            lon += (a * rn - c * re) / det;
        }
        (lat, lon)
    }

    fn approx_inverse(e: f64, n: f64) -> (f64, f64) {
        let y = (e - 2_600_000.0) / 1e6;
        let x = (n - 1_200_000.0) / 1e6;
        let l = 2.677_909_4 + 4.728_982 * y + 0.791_484 * y * x + 0.130_6 * y * x * x
            - 0.043_6 * y.powi(3);
        let p = 16.902_389_2 + 3.238_272 * x
            - 0.270_978 * y * y
            - 0.002_528 * x * x
            - 0.044_7 * y * y * x
            - 0.014_0 * x.powi(3);
        (p * 100.0 / 36.0, l * 100.0 / 36.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference values from PROJ 9.8 (EPSG:4326 to the grid, no grid files).
    fn check(crs: Crs, refs: &[(f64, f64, f64, f64)], tol_m: f64) {
        for &(lat, lon, x, y) in refs {
            let (ex, ey) = crs.forward(lat, lon);
            assert!(
                (ex - x).abs() < tol_m && (ey - y).abs() < tol_m,
                "{crs:?} forward {lat} {lon}: {ex} {ey}, want {x} {y}"
            );
            let (la, lo) = crs.inverse(x, y);
            // A metre is about 9e-6 degrees of latitude.
            let tol_deg = tol_m * 1.5e-5;
            assert!(
                (la - lat).abs() < tol_deg && (lo - lon).abs() < tol_deg,
                "{crs:?} inverse {x} {y}: {la} {lo}, want {lat} {lon}"
            );
        }
    }

    #[test]
    fn lambert93_matches_proj() {
        check(
            Crs::Lambert93,
            &[
                (45.76, 4.84, 843_011.136, 6_519_488.062),
                (47.218, -1.553, 355_620.573, 6_689_676.129),
                (48.8566, 2.3522, 652_469.023, 6_862_035.259),
                (43.30, 5.37, 892_394.777, 6_247_424.646),
                (51.0, 2.4, 657_789.359, 7_100_624.956),
            ],
            0.01,
        );
    }

    #[test]
    fn rd_new_matches_proj() {
        check(
            Crs::RdNew,
            &[
                (52.155, 5.387, 154_986.073, 462_980.830),
                (52.373, 4.893, 121_344.917, 487_350.778),
                (50.85, 5.69, 176_325.222, 317_840.150),
                (53.2, 6.57, 234_038.922, 579_910.438),
            ],
            1.5,
        );
    }

    #[test]
    fn lv95_matches_proj() {
        check(
            Crs::Lv95,
            &[
                (46.948, 7.4474, 2_600_667.475, 1_199_657.316),
                (47.3769, 8.5417, 2_683_303.894, 1_247_925.628),
                (46.2, 6.14, 2_499_760.952, 1_117_336.098),
            ],
            1.5,
        );
    }

    #[test]
    fn lest97_matches_proj() {
        check(
            Crs::Lest97,
            &[
                (59.437, 24.745, 542_275.378, 6_589_030.824),
                (58.38, 26.72, 659_081.552, 6_474_284.185),
                (58.25, 22.48, 410_756.653, 6_457_590.420),
                (59.36, 28.19, 738_149.315, 6_587_658.430),
            ],
            0.01,
        );
        assert!(Crs::Lest97.north_first() && !Crs::Utm(32).north_first());
    }

    #[test]
    fn luref_matches_proj() {
        check(
            Crs::Luref,
            &[
                (49.61, 6.13, 77_244.696, 75_041.871),
                (49.5, 5.98, 66_373.092, 62_823.714),
                (50.05, 6.05, 71_539.236, 123_987.480),
            ],
            0.05,
        );
    }

    #[test]
    fn utm_goes_through() {
        let (e, n) = Crs::Utm(32).forward(48.3000481, 11.8981969);
        assert!((e - 714_925.411).abs() < 0.01 && (n - 5_353_710.414).abs() < 0.01);
        assert_eq!(Crs::Utm(32).epsg(), 25832);
    }
}
