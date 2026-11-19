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
//!   (LUREF to WGS 84 (3)), which PROJ rates at a metre;
//! * NAD83 UTM (EPSG:26901-26923) by the same series as UTM. NAD83 and WGS84
//!   differ by a metre or two, as PROJ's own default (a null transformation)
//!   has it; OpenStreetMap in North America is traced on NAD83 photos anyway;
//! * Web Mercator (EPSG:3857), the spherical formulas on the WGS84 radius;
//! * Latvia's LKS-92 / Latvia TM (EPSG:3059), Transverse Mercator on GRS80 by
//!   the same series as UTM.
//!
//! A source's points can come in another grid than the one the area is
//! measured on (USGS's point clouds are in Web Mercator); over a tile, an
//! [`Affine`] fitted at its middle carries them across to well under a
//! decimetre.

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
    /// Northern UTM zone on NAD83.
    Nad83Utm(u8),
    WebMercator,
    /// Latvia's LKS-92 / Latvia TM.
    Lks92,
}

/// The UTM zone of a longitude.
pub fn utm_zone(lon: f64) -> u8 {
    (((lon + 180.0) / 6.0).floor() as i32).rem_euclid(60) as u8 + 1
}

impl Crs {
    /// Latitude and longitude in degrees to (easting, northing) in metres.
    pub fn forward(self, lat: f64, lon: f64) -> (f64, f64) {
        match self {
            Crs::Utm(zone) | Crs::Nad83Utm(zone) => utm::to_utm(lat, lon, zone),
            Crs::Lambert93 => lambert::lambert93().forward(lat, lon),
            Crs::RdNew => rd::forward(lat, lon),
            Crs::Lv95 => lv95::forward(lat, lon),
            Crs::Lest97 => lambert::lest97().forward(lat, lon),
            Crs::Luref => datum::luref().forward(lat, lon),
            Crs::WebMercator => mercator::forward(lat, lon),
            Crs::Lks92 => tm::LKS92.forward(tm::grs80(), lat, lon),
        }
    }

    /// (easting, northing) in metres to latitude and longitude in degrees.
    pub fn inverse(self, x: f64, y: f64) -> (f64, f64) {
        match self {
            Crs::Utm(zone) | Crs::Nad83Utm(zone) => utm::from_utm(x, y, zone),
            Crs::Lambert93 => lambert::lambert93().inverse(x, y),
            Crs::RdNew => rd::inverse(x, y),
            Crs::Lv95 => lv95::inverse(x, y),
            Crs::Lest97 => lambert::lest97().inverse(x, y),
            Crs::Luref => datum::luref().inverse(x, y),
            Crs::WebMercator => mercator::inverse(x, y),
            Crs::Lks92 => tm::LKS92.inverse(tm::grs80(), x, y),
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
            // NAD83's UTM codes stop at zone 23 (26901-26923; past them are
            // state planes). The western Aleutians have codes of their own,
            // Guam and the Northern Marianas NAD83(MA11)'s; anything else
            // takes WGS 84's, a metre or so away.
            Crs::Nad83Utm(zone @ 1..=23) => 26900 + u32::from(zone),
            Crs::Nad83Utm(55) => 8693,
            Crs::Nad83Utm(59) => 3372,
            Crs::Nad83Utm(60) => 3373,
            Crs::Nad83Utm(zone) => 32600 + u32::from(zone),
            Crs::WebMercator => 3857,
            Crs::Lks92 => 3059,
        }
    }

    /// Whether the grid's EPSG definition puts northing first, as WMS 1.3.0
    /// then wants its bounding boxes.
    pub fn north_first(self) -> bool {
        matches!(self, Crs::Lest97 | Crs::Luref | Crs::Lks92)
    }
}

/// The grid an OGC WKT (version 1, as LAS files carry it) names, where it is a
/// northern UTM zone in metres; `None` for anything else (state planes, feet,
/// MTM), which the survey then does not read.
pub fn from_wkt(wkt: &str) -> Option<Crs> {
    // Only the projected part: a compound grid's vertical part may come after.
    let proj = &wkt[wkt.find("PROJCS[")?..];
    let proj = proj.split("VERT_CS[").next().unwrap_or(proj);
    if !proj.contains("PROJECTION[\"Transverse_Mercator\"]") || wkt.to_lowercase().contains("foot")
    {
        return None;
    }
    let param = |name: &str| -> Option<f64> {
        let at = proj.find(&format!("PARAMETER[\"{name}\","))? + name.len() + 13;
        proj[at..].split([']', ',']).next()?.trim().parse().ok()
    };
    let (lat0, lon0) = (param("latitude_of_origin")?, param("central_meridian")?);
    let (k, fe, fn_) = (
        param("scale_factor")?,
        param("false_easting")?,
        param("false_northing")?,
    );
    let zone = (lon0 + 183.0) / 6.0;
    let utm = lat0 == 0.0
        && (k - 0.9996).abs() < 1e-9
        && fe == 500_000.0
        && fn_ == 0.0
        && zone.fract() == 0.0
        && (1.0..=60.0).contains(&zone);
    utm.then_some(Crs::Nad83Utm(zone as u8))
}

/// `to = a * from + b`, a linear map between two grids that holds near where it
/// was fitted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    /// x' = m[0] x + m[1] y + m[2]; y' = m[3] x + m[4] y + m[5].
    pub m: [f64; 6],
}

impl Affine {
    /// The map from grid `from` to grid `to` around the point (`x`, `y`) of
    /// `to`, by central differences over `d` metres either way.
    pub fn fit(from: Crs, to: Crs, (x, y): (f64, f64), d: f64) -> Affine {
        let at = |dx: f64, dy: f64| {
            let (lat, lon) = to.inverse(x + dx, y + dy);
            from.forward(lat, lon)
        };
        let c = at(0.0, 0.0);
        let (e, w) = (at(d, 0.0), at(-d, 0.0));
        let (n, s) = (at(0.0, d), at(0.0, -d));
        // The map `to -> from` near the point, then its inverse.
        let (a, b) = ((e.0 - w.0) / (2.0 * d), (n.0 - s.0) / (2.0 * d));
        let (c2, d2) = ((e.1 - w.1) / (2.0 * d), (n.1 - s.1) / (2.0 * d));
        let det = a * d2 - b * c2;
        let (ia, ib, ic, id) = (d2 / det, -b / det, -c2 / det, a / det);
        Affine {
            m: [
                ia,
                ib,
                x - ia * c.0 - ib * c.1,
                ic,
                id,
                y - ic * c.0 - id * c.1,
            ],
        }
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.m;
        (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5])
    }

    /// The map back.
    pub fn inverse(&self) -> Affine {
        let m = &self.m;
        let det = m[0] * m[4] - m[1] * m[3];
        let (a, b, c, d) = (m[4] / det, -m[1] / det, -m[3] / det, m[0] / det);
        Affine {
            m: [a, b, -a * m[2] - b * m[5], c, d, -c * m[2] - d * m[5]],
        }
    }

    /// The rectangle (west, south, east, north) holding the image of one.
    pub fn rect(&self, (x0, y0, x1, y1): (f64, f64, f64, f64)) -> (f64, f64, f64, f64) {
        let pts = [(x0, y0), (x0, y1), (x1, y0), (x1, y1)].map(|(x, y)| self.apply(x, y));
        let (mut a, mut b, mut c, mut d) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (x, y) in pts {
            a = a.min(x);
            b = b.min(y);
            c = c.max(x);
            d = d.max(y);
        }
        (a, b, c, d)
    }
}

/// Spherical Web Mercator on the WGS84 semi-major axis.
/// Transverse Mercator grids on GRS80, whose ETRS89 realisations WGS84 meets
/// to well under a metre.
mod tm {
    use super::utm::{Series, Tm};

    /// LKS-92 / Latvia TM (EPSG:3059).
    pub const LKS92: Tm = Tm {
        lon0: 24.0,
        lat0: 0.0,
        k0: 0.9996,
        fe: 500_000.0,
        fn_: -6_000_000.0,
    };

    pub fn grs80() -> &'static Series {
        use std::sync::OnceLock;
        static S: OnceLock<Series> = OnceLock::new();
        S.get_or_init(|| Series::new(6_378_137.0, 1.0 / 298.257_222_101))
    }
}

mod mercator {
    const R: f64 = 6_378_137.0;

    pub fn forward(lat: f64, lon: f64) -> (f64, f64) {
        let phi = lat.to_radians();
        (
            R * lon.to_radians(),
            R * (std::f64::consts::FRAC_PI_4 + phi / 2.0).tan().ln(),
        )
    }

    pub fn inverse(x: f64, y: f64) -> (f64, f64) {
        let lat = 2.0 * (y / R).exp().atan() - std::f64::consts::FRAC_PI_2;
        (lat.to_degrees(), (x / R).to_degrees())
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
    fn nad83_utm_matches_proj() {
        check(
            Crs::Nad83Utm(17),
            &[
                (42.275, -83.76, 272_401.660, 4_683_998.969),
                (42.0, -80.5, 541_408.867, 4_649_897.125),
                (35.0, -84.0, 226_201.904, 3_877_156.692),
            ],
            0.01,
        );
        check(
            Crs::Nad83Utm(10),
            &[(47.61, -122.33, 550_354.404, 5_273_172.275)],
            0.01,
        );
        // PROJ shifts Hawaii's NAD83 half a metre from WGS84; the survey takes
        // the two as one, as in the rest of the country, so the reference is
        // WGS84's zone (EPSG:32604).
        check(
            Crs::Nad83Utm(4),
            &[(21.31, -157.86, 618_238.268, 2_356_884.330)],
            0.01,
        );
        assert_eq!(Crs::Nad83Utm(17).epsg(), 26917);
        // Past zone 23, 269xx are state planes.
        assert_eq!(Crs::Nad83Utm(55).epsg(), 8693);
        assert_eq!(Crs::Nad83Utm(60).epsg(), 3373);
        assert_eq!(Crs::Nad83Utm(40).epsg(), 32640);
        assert_eq!(utm_zone(-83.76), 17);
        assert_eq!(utm_zone(-122.33), 10);
        assert_eq!(utm_zone(11.9), 32);
        assert_eq!(utm_zone(174.77), 60);
    }

    #[test]
    fn wkt_names_utm_zones_and_nothing_else() {
        let ottawa = r#"PROJCS["NAD83(CSRS) / UTM zone 18N",GEOGCS["NAD83(CSRS)",DATUM["NAD83_Canadian_Spatial_Reference_System",SPHEROID["GRS 1980",6378137,298.257222101,AUTHORITY["EPSG","7019"]],AUTHORITY["EPSG","6140"]],PRIMEM["Greenwich",0,AUTHORITY["EPSG","8901"]],UNIT["degree",0.0174532925199433,AUTHORITY["EPSG","9122"]],AUTHORITY["EPSG","4617"]],PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],PARAMETER["central_meridian",-75],PARAMETER["scale_factor",0.9996],PARAMETER["false_easting",500000],PARAMETER["false_northing",0],UNIT["metre",1,AUTHORITY["EPSG","9001"]],AXIS["Easting",EAST],AXIS["Northing",NORTH],AUTHORITY["EPSG","2959"]]"#;
        assert_eq!(from_wkt(ottawa), Some(Crs::Nad83Utm(18)));
        let compound = format!(
            r#"COMPD_CS["NAD83(CSRS) / UTM zone 12N + CGVD2013(CGG2013) height",{},VERT_CS["CGVD2013(CGG2013) height",VERT_DATUM["Canadian Geodetic Vertical Datum of 2013 (CGG2013)",2005],UNIT["metre",1],AXIS["Gravity-related height",UP]]]"#,
            ottawa.replace("-75]", "-111]")
        );
        assert_eq!(from_wkt(&compound), Some(Crs::Nad83Utm(12)));
        // MTM zone 8 (Montérégie), feet, and no projection at all.
        let mtm = ottawa
            .replace("0.9996", "0.9999")
            .replace("500000", "304800")
            .replace("-75]", "-73.5]");
        assert_eq!(from_wkt(&mtm), None);
        let feet = ottawa.replace(
            r#"UNIT["metre",1,"#,
            r#"UNIT["US survey foot",0.3048006096012192,"#,
        );
        assert_eq!(from_wkt(&feet), None);
        assert_eq!(from_wkt(r#"GEOGCS["WGS 84"]"#), None);
    }

    #[test]
    fn web_mercator_matches_proj() {
        check(
            Crs::WebMercator,
            &[
                (40.0, -105.0, -11_688_546.533, 4_865_942.280),
                (47.61, -122.33, -13_617_713.309, 6_042_216.250),
                (61.22, -149.9, -16_686_791.670, 8_676_514.367),
                (25.77, -80.19, -8_926_709.967, 2_970_622.233),
            ],
            0.01,
        );
    }

    #[test]
    fn affine_carries_mercator_to_utm_within_a_decimetre() {
        let (to, from) = (Crs::Nad83Utm(17), Crs::WebMercator);
        let centre = (272_500.0, 4_684_500.0);
        let f = Affine::fit(from, to, centre, 250.0);
        // Anywhere within a 1.2 km square around the middle.
        for (dx, dy) in [
            (-600.0, -600.0),
            (600.0, 0.0),
            (0.0, 600.0),
            (450.0, -520.0),
        ] {
            let (x, y) = (centre.0 + dx, centre.1 + dy);
            let (lat, lon) = to.inverse(x, y);
            let (mx, my) = from.forward(lat, lon);
            let (gx, gy) = f.apply(mx, my);
            assert!(
                (gx - x).abs() < 0.1 && (gy - y).abs() < 0.1,
                "{dx} {dy}: {gx} {gy}"
            );
            let (bx, by) = f.inverse().apply(x, y);
            assert!((bx - mx).abs() < 0.2 && (by - my).abs() < 0.2);
        }
        // Across a zone border too: zone 16 points into the zone 17 grid.
        let f = Affine::fit(Crs::Nad83Utm(16), to, (250_000.0, 4_500_000.0), 250.0);
        let (lat, lon) = to.inverse(250_400.0, 4_500_300.0);
        let (x16, y16) = Crs::Nad83Utm(16).forward(lat, lon);
        let (gx, gy) = f.apply(x16, y16);
        assert!((gx - 250_400.0).abs() < 0.1 && (gy - 4_500_300.0).abs() < 0.1);
    }

    #[test]
    fn lks92_matches_proj() {
        check(
            Crs::Lks92,
            &[
                (56.95, 24.11, 506_691.224, 311_825.560),
                (56.928, 24.035, 502_130.280, 309_371.808),
                (55.87, 26.53, 658_300.786, 194_504.931),
                (57.39, 21.56, 353_347.736, 363_431.295),
                (56.51, 27.33, 704_896.473, 267_811.546),
            ],
            0.01,
        );
        assert!(Crs::Lks92.north_first());
    }

    #[test]
    fn utm_goes_through() {
        let (e, n) = Crs::Utm(32).forward(48.3000481, 11.8981969);
        assert!((e - 714_925.411).abs() < 0.01 && (n - 5_353_710.414).abs() < 0.01);
        assert_eq!(Crs::Utm(32).epsg(), 25832);
    }
}
