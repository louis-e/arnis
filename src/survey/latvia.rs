//! Latvia: LĢIA's laser scans, the base data of its elevation model, and its
//! orthophoto.
//!
//! * Laser scans: plain, uncompressed LAS (1.2, point format 1) per 1 km sheet
//!   of the TKS-93 1:2000 grid in LKS-92 / Latvia TM (EPSG:3059), at a fixed
//!   address, `https://s3.storage.pub.lvdc.gov.lv/lgia-opendata/las/{sheet50}/
//!   {sheet50}-{sheet10}-{sheet2}.las` (see [`sheet`] for the names). Every
//!   sheet is listed in `LGIA_OpenData_las_saites.txt` beside them; a sheet the
//!   survey lacks answers 404. Flown 2013 to 2019; at least 4 points per m²
//!   in all (3 to 7 measured), 75 to 190 MB a sheet, about 170 in towns.
//!   Classes 2 ground, 3/4/5 low, medium and high vegetation, 6 building,
//!   7 noise, 9 water. Most lawn and meadow returns are low vegetation a few
//!   centimetres up, read here as planted ground; the ground returns left
//!   read darker on lawn than on paving, and bare or trodden ground as
//!   bright as paving, so the intensity is read turned over and paving kept
//!   near buildings only ([`Source::paving_brighter`]).
//! * Orthophoto: the open sixth cycle (2016 to 2018), RGB at 25 cm in
//!   uncompressed GeoTIFF sheets of 2.5 km (1:5000, tiled 512 pixels), at
//!   `https://s3.storage.pub.lvdc.gov.lv/lgia-opendata/ortofoto_rgb_v6/{sheet50}/
//!   {sheet50}-{sheet10}_{quarter}.tif` ([`photo_sheet`]); listed in
//!   `LGIA_OpenData_Ortofoto_rgb_v6_saites.txt`. The open WMS stops at 2008.
//!
//! Licence CC BY 4.0, attribution "Latvijas Ģeotelpiskās informācijas
//! aģentūra".

use super::crs::Crs;
use super::source::{Grid, Laser, LaserClasses, Locate, Photo, Source};

const BASE: &str = "https://s3.storage.pub.lvdc.gov.lv/lgia-opendata/las";
const PHOTO: &str = "https://s3.storage.pub.lvdc.gov.lv/lgia-opendata/ortofoto_rgb_v6";

/// Latvia within its sea border, simplified to about a kilometre and pushed
/// out by about two. From OpenStreetMap (relation 72594), © OpenStreetMap
/// contributors, ODbL.
const OUTLINE: &[&[(f64, f64)]] = &[&[
    (56.447, 20.593),
    (56.526, 20.567),
    (56.573, 20.567),
    (56.624, 20.588),
    (56.702, 20.651),
    (56.842, 20.652),
    (56.911, 20.679),
    (56.987, 20.761),
    (57.079, 20.955),
    (57.115, 21.008),
    (57.284, 21.006),
    (57.379, 21.045),
    (57.457, 21.128),
    (57.522, 21.177),
    (57.576, 21.263),
    (57.678, 21.342),
    (57.718, 21.401),
    (57.758, 21.492),
    (57.783, 21.576),
    (57.808, 21.718),
    (57.770, 21.918),
    (57.798, 22.128),
    (57.946, 22.569),
    (57.963, 22.725),
    (57.714, 23.030),
    (57.607, 23.198),
    (57.607, 23.228),
    (57.534, 23.388),
    (57.487, 23.468),
    (57.431, 23.520),
    (57.287, 23.589),
    (57.275, 23.612),
    (57.324, 23.993),
    (57.529, 23.953),
    (57.622, 23.958),
    (57.706, 23.927),
    (57.839, 23.918),
    (57.919, 23.943),
    (57.919, 24.198),
    (57.942, 24.264),
    (57.896, 24.368),
    (57.894, 24.391),
    (57.942, 24.437),
    (57.971, 24.550),
    (57.989, 24.579),
    (57.975, 24.614),
    (57.979, 24.693),
    (57.982, 24.704),
    (57.998, 24.705),
    (58.013, 24.823),
    (57.998, 24.845),
    (58.029, 24.936),
    (58.034, 24.994),
    (58.095, 25.071),
    (58.096, 25.165),
    (58.110, 25.195),
    (58.087, 25.245),
    (58.102, 25.259),
    (58.101, 25.311),
    (58.081, 25.353),
    (58.050, 25.390),
    (58.049, 25.424),
    (58.033, 25.474),
    (58.000, 25.512),
    (58.005, 25.544),
    (57.994, 25.551),
    (58.001, 25.568),
    (57.985, 25.613),
    (57.954, 25.634),
    (57.946, 25.665),
    (57.933, 25.679),
    (57.931, 25.693),
    (57.938, 25.698),
    (57.947, 25.744),
    (57.929, 25.795),
    (57.911, 25.830),
    (57.901, 25.826),
    (57.883, 25.845),
    (57.894, 25.882),
    (57.872, 25.911),
    (57.868, 25.959),
    (57.880, 25.989),
    (57.866, 26.082),
    (57.833, 26.096),
    (57.794, 26.074),
    (57.767, 26.160),
    (57.749, 26.178),
    (57.720, 26.254),
    (57.696, 26.230),
    (57.667, 26.274),
    (57.613, 26.306),
    (57.593, 26.407),
    (57.597, 26.494),
    (57.540, 26.532),
    (57.558, 26.566),
    (57.556, 26.604),
    (57.579, 26.613),
    (57.577, 26.642),
    (57.590, 26.661),
    (57.591, 26.697),
    (57.618, 26.729),
    (57.601, 26.767),
    (57.604, 26.819),
    (57.631, 26.836),
    (57.653, 26.897),
    (57.654, 26.940),
    (57.634, 26.991),
    (57.641, 27.017),
    (57.625, 27.026),
    (57.624, 27.049),
    (57.600, 27.082),
    (57.596, 27.113),
    (57.582, 27.133),
    (57.572, 27.200),
    (57.573, 27.260),
    (57.562, 27.309),
    (57.545, 27.340),
    (57.556, 27.365),
    (57.556, 27.402),
    (57.550, 27.413),
    (57.560, 27.487),
    (57.556, 27.602),
    (57.533, 27.593),
    (57.509, 27.605),
    (57.469, 27.589),
    (57.445, 27.610),
    (57.437, 27.597),
    (57.439, 27.563),
    (57.435, 27.615),
    (57.420, 27.645),
    (57.411, 27.703),
    (57.386, 27.718),
    (57.369, 27.790),
    (57.344, 27.815),
    (57.339, 27.892),
    (57.322, 27.877),
    (57.302, 27.903),
    (57.272, 27.901),
    (57.264, 27.934),
    (57.220, 27.879),
    (57.193, 27.866),
    (57.162, 27.873),
    (57.133, 27.850),
    (57.113, 27.791),
    (57.102, 27.790),
    (57.088, 27.829),
    (57.063, 27.783),
    (57.026, 27.767),
    (57.028, 27.821),
    (56.953, 27.779),
    (56.900, 27.771),
    (56.891, 27.712),
    (56.870, 27.708),
    (56.886, 27.806),
    (56.900, 27.834),
    (56.900, 27.899),
    (56.852, 27.930),
    (56.862, 27.941),
    (56.856, 27.983),
    (56.845, 28.002),
    (56.803, 28.011),
    (56.788, 27.982),
    (56.771, 27.980),
    (56.755, 27.949),
    (56.746, 27.951),
    (56.718, 28.012),
    (56.681, 28.046),
    (56.641, 28.048),
    (56.606, 28.075),
    (56.602, 28.143),
    (56.569, 28.205),
    (56.559, 28.177),
    (56.541, 28.172),
    (56.531, 28.149),
    (56.508, 28.184),
    (56.496, 28.170),
    (56.456, 28.227),
    (56.421, 28.220),
    (56.399, 28.245),
    (56.379, 28.206),
    (56.363, 28.230),
    (56.329, 28.241),
    (56.280, 28.278),
    (56.235, 28.250),
    (56.227, 28.233),
    (56.209, 28.239),
    (56.170, 28.211),
    (56.156, 28.189),
    (56.147, 28.141),
    (56.111, 28.127),
    (56.116, 28.081),
    (56.098, 28.077),
    (56.101, 28.001),
    (56.034, 27.933),
    (56.024, 27.868),
    (55.997, 27.849),
    (55.984, 27.853),
    (55.968, 27.828),
    (55.956, 27.798),
    (55.955, 27.764),
    (55.920, 27.689),
    (55.906, 27.689),
    (55.887, 27.663),
    (55.848, 27.676),
    (55.774, 27.652),
    (55.763, 27.596),
    (55.764, 27.549),
    (55.775, 27.423),
    (55.787, 27.373),
    (55.763, 27.287),
    (55.778, 27.227),
    (55.816, 27.175),
    (55.828, 27.135),
    (55.817, 27.131),
    (55.808, 27.106),
    (55.809, 27.060),
    (55.797, 26.999),
    (55.771, 26.971),
    (55.768, 26.945),
    (55.755, 26.952),
    (55.729, 26.914),
    (55.700, 26.899),
    (55.680, 26.824),
    (55.657, 26.809),
    (55.662, 26.731),
    (55.682, 26.701),
    (55.684, 26.677),
    (55.672, 26.674),
    (55.650, 26.593),
    (55.661, 26.504),
    (55.687, 26.437),
    (55.689, 26.412),
    (55.680, 26.397),
    (55.698, 26.344),
    (55.722, 26.332),
    (55.727, 26.287),
    (55.756, 26.239),
    (55.844, 26.178),
    (55.861, 26.098),
    (55.943, 26.011),
    (55.945, 25.982),
    (55.963, 25.952),
    (55.974, 25.907),
    (55.980, 25.842),
    (56.008, 25.804),
    (56.017, 25.814),
    (56.039, 25.790),
    (56.045, 25.740),
    (56.058, 25.720),
    (56.074, 25.659),
    (56.083, 25.649),
    (56.115, 25.652),
    (56.134, 25.368),
    (56.117, 25.304),
    (56.163, 25.224),
    (56.154, 25.167),
    (56.167, 25.067),
    (56.187, 25.041),
    (56.253, 25.020),
    (56.299, 24.943),
    (56.354, 24.933),
    (56.399, 24.879),
    (56.391, 24.816),
    (56.380, 24.785),
    (56.380, 24.738),
    (56.360, 24.709),
    (56.356, 24.664),
    (56.335, 24.655),
    (56.316, 24.616),
    (56.285, 24.615),
    (56.271, 24.601),
    (56.262, 24.532),
    (56.237, 24.462),
    (56.242, 24.406),
    (56.256, 24.381),
    (56.246, 24.343),
    (56.268, 24.322),
    (56.251, 24.224),
    (56.229, 24.190),
    (56.228, 24.120),
    (56.240, 24.075),
    (56.258, 24.068),
    (56.262, 24.008),
    (56.290, 24.011),
    (56.300, 23.989),
    (56.310, 23.986),
    (56.319, 23.819),
    (56.297, 23.805),
    (56.301, 23.769),
    (56.316, 23.726),
    (56.349, 23.732),
    (56.342, 23.647),
    (56.321, 23.594),
    (56.311, 23.542),
    (56.333, 23.417),
    (56.359, 23.375),
    (56.362, 23.325),
    (56.352, 23.292),
    (56.349, 23.222),
    (56.328, 23.219),
    (56.305, 23.155),
    (56.294, 23.160),
    (56.288, 23.120),
    (56.267, 23.104),
    (56.266, 23.088),
    (56.301, 23.057),
    (56.296, 23.025),
    (56.319, 22.995),
    (56.382, 22.962),
    (56.362, 22.943),
    (56.346, 22.867),
    (56.343, 22.805),
    (56.351, 22.740),
    (56.334, 22.712),
    (56.331, 22.676),
    (56.349, 22.616),
    (56.362, 22.598),
    (56.383, 22.463),
    (56.388, 22.419),
    (56.374, 22.348),
    (56.387, 22.299),
    (56.380, 22.236),
    (56.401, 22.186),
    (56.399, 22.155),
    (56.409, 22.129),
    (56.394, 22.024),
    (56.369, 22.006),
    (56.355, 21.977),
    (56.343, 21.858),
    (56.337, 21.855),
    (56.341, 21.832),
    (56.317, 21.794),
    (56.315, 21.770),
    (56.292, 21.740),
    (56.291, 21.665),
    (56.301, 21.613),
    (56.275, 21.552),
    (56.270, 21.513),
    (56.233, 21.476),
    (56.198, 21.347),
    (56.166, 21.321),
    (56.143, 21.274),
    (56.128, 21.285),
    (56.079, 21.253),
    (56.058, 21.223),
    (56.053, 21.186),
    (56.061, 21.154),
    (56.049, 21.068),
    (56.025, 20.681),
    (56.145, 20.609),
    (56.208, 20.586),
    (56.371, 20.572),
    (56.447, 20.593),
]];

pub static LATVIA: Source = Source {
    id: "latvia",
    name: "LĢIA",
    grid: Grid::Fixed(Crs::Lks92),
    outline: OUTLINE,
    laser: Laser::Whole {
        locate: Locate::Urls(urls),
        tile_mb: 170,
    },
    photo: Photo::Sheets {
        sheet: photo_sheet,
        px_per_m: 4,
    },
    // Lawn up to 40, paving past 60: Riga-Bieriņi's flat ground (2016)
    // splits at 42, lawn about 1 and paving about 84, and its photo puts
    // lawn at 0 and paving past 74. A last resort.
    intensity_defaults: (40.0, 60.0),
    // Low vegetation is lawn and meadow a few centimetres up: ground, and
    // vegetation too, so it reads as planted whatever its intensity.
    classes: LaserClasses {
        ground: &[2, 3],
        building: &[6],
        object: &[1, 4, 5],
        vegetation: &[3, 4, 5],
        footprint: &[],
        unsorted: &[],
    },
    laser_plants: true,
    strict_pools: true,
    paving_brighter: true,
    attribution: "Latvijas Ģeotelpiskās informācijas aģentūra (CC BY 4.0)",
};

/// The TKS-93 1:50 000 sheet holding the kilometre with south-west corner
/// (`e_km`, `n_km`), and the kilometre's place in it (east and north of its
/// south-west corner, 0 to 24). The 1:200 000 sheets count rows from the
/// south and columns from 300 km east, 100 km each; their 1:100 000 and
/// 1:50 000 quarters are 1 south-west, 2 south-east, 3 north-west, 4
/// north-east.
fn sheet50(e_km: i32, n_km: i32) -> Option<(String, i32, i32)> {
    let (r200, c200) = (n_km.div_euclid(100) + 1, (e_km - 300).div_euclid(100) + 1);
    if !(1..=9).contains(&r200) || !(1..=9).contains(&c200) {
        return None;
    }
    let (ex, ny) = ((e_km - 300).rem_euclid(100), n_km.rem_euclid(100));
    let quarter = |x: i32, y: i32, half: i32| 1 + i32::from(x >= half) + 2 * i32::from(y >= half);
    let q100 = quarter(ex, ny, 50);
    let (ex, ny) = (ex % 50, ny % 50);
    let q50 = quarter(ex, ny, 25);
    Some((format!("{r200}{c200}{q100}{q50}"), ex % 25, ny % 25))
}

/// The laser sheet of a kilometre: its 1:50 000 sheet, then the 1:10 000 and
/// 1:2000 sheets in it, five by five, row from the south and column from the
/// west.
fn sheet(e_km: i32, n_km: i32) -> Option<(String, String)> {
    let (s50, ex, ny) = sheet50(e_km, n_km)?;
    let name = format!(
        "{s50}-{}{}-{}{}",
        ny / 5 + 1,
        ex / 5 + 1,
        ny % 5 + 1,
        ex % 5 + 1
    );
    Some((s50, name))
}

fn urls(te: i32, tn: i32) -> Vec<String> {
    sheet(te, tn)
        .map(|(s50, name)| format!("{BASE}/{s50}/{name}.las"))
        .into_iter()
        .collect()
}

/// The orthophoto sheet (1:5000, 2.5 km, a quarter of a 1:10 000 sheet
/// numbered like the laser's) holding grid point (`e`, `n`): its address and
/// north-west corner.
fn photo_sheet(e: f64, n: f64) -> Option<(String, f64, f64)> {
    let (e_km, n_km) = ((e / 1000.0).floor() as i32, (n / 1000.0).floor() as i32);
    let (s50, ex, ny) = sheet50(e_km, n_km)?;
    // Metres from the 1:50 000 sheet's south-west corner.
    let (em, nm) = (
        f64::from(ex) * 1000.0 + (e - f64::from(e_km) * 1000.0),
        f64::from(ny) * 1000.0 + (n - f64::from(n_km) * 1000.0),
    );
    let (col, row) = ((em / 5000.0).floor(), (nm / 5000.0).floor());
    let (qx, qy) = (em - col * 5000.0 >= 2500.0, nm - row * 5000.0 >= 2500.0);
    let q = 1 + u8::from(qx) + 2 * u8::from(qy);
    let (e0, n0) = (e - em, n - nm);
    Some((
        format!(
            "{PHOTO}/{s50}/{s50}-{}{}_{q}.tif",
            row as i32 + 1,
            col as i32 + 1
        ),
        e0 + col * 5000.0 + if qx { 2500.0 } else { 0.0 },
        n0 + row * 5000.0 + if qy { 5000.0 } else { 2500.0 },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheets_are_named_as_the_survey_names_them() {
        // From the headers of the files: each covers exactly this kilometre.
        let name = |e, n| sheet(e, n).map(|s| s.1);
        assert_eq!(name(506, 312).as_deref(), Some("4311-32-32")); // Riga
        assert_eq!(name(649, 175).as_deref(), Some("2434-15-15"));
        assert_eq!(name(649, 199).as_deref(), Some("2434-55-55"));
        assert_eq!(name(324, 224).as_deref(), Some("3111-55-55"));
        assert_eq!(name(630, 400).as_deref(), Some("5412-12-11"));
        assert_eq!(urls(506, 312), vec![format!("{BASE}/4311/4311-32-32.las")]);
    }

    #[test]
    fn photo_sheets_are_named_and_placed_as_their_world_files_say() {
        // 4311-31_1.tfw: 25 cm pixels, the first centred on 500000.125,
        // 312499.875.
        let (url, w, n) = photo_sheet(501_000.0, 311_000.0).unwrap();
        assert_eq!(url, format!("{PHOTO}/4311/4311-31_1.tif"));
        assert_eq!((w, n), (500_000.0, 312_500.0));
        // Its north-east neighbour across both halves.
        let (url, w, n) = photo_sheet(503_100.0, 313_900.0).unwrap();
        assert_eq!(url, format!("{PHOTO}/4311/4311-31_4.tif"));
        assert_eq!((w, n), (502_500.0, 315_000.0));
    }
}
