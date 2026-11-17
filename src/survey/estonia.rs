//! Estonia: Maa- ja Ruumiamet's laser scans and orthophotos.
//!
//! * Laser scans: plain LAZ (LAS 1.4, point format 8) per 1 km sheet of
//!   L-EST97 (EPSG:3301), the sheet numbered `(N_km - 6000) * 1000 + E_km` from
//!   its south-west corner. Every flight of a sheet is kept, so the files are
//!   found through the sheet's page,
//!   `https://geoportaal.maaruum.ee/index.php?lang_id=1&plugin_act=otsing&kaardiruut={sheet}&andmetyyp=lidar_laz_tava&page_id=614`,
//!   which links `{sheet}_{year}_{kind}.laz` for each: `tava` the regular
//!   spring flights over the whole country (about 4 points per m², 40 to 80 MB),
//!   `madal` low flights over towns (300 to 400 MB, too large to read whole) and
//!   `mets` summer flights over forest. The latest regular flight is read;
//!   the same address with `&dl=1&f={file}` downloads it. A sheet without the
//!   file answers with a web page, not a 404.
//! * Classes: 1 unclassified (low objects among them), 2 ground, 5 high
//!   vegetation, 6 building, 7 noise.
//! * Orthophoto: the `EESTIFOTO` layer of the `fotokaart` WMS, flown in spring.
//!
//! Maa- ja Ruumiamet open data licence (free use with attribution).

use super::crs::Crs;
use super::source::{Grid, Laser, LaserClasses, Locate, Photo, Rect, Sheet, Source, TileUrls};

const PAGE: &str = "https://geoportaal.maaruum.ee/index.php?lang_id=1&plugin_act=otsing";

/// Estonia within its sea border, simplified to about a kilometre and pushed
/// out by about two. From OpenStreetMap (relation 79510), © OpenStreetMap
/// contributors, ODbL.
const OUTLINE: &[&[(f64, f64)]] = &[&[
    (57.855, 24.379),
    (57.857, 24.343),
    (57.895, 24.259),
    (57.881, 24.220),
    (57.881, 23.644),
    (57.782, 23.684),
    (57.659, 23.612),
    (57.576, 23.419),
    (57.568, 23.165),
    (57.692, 22.971),
    (57.921, 22.691),
    (57.910, 22.597),
    (57.762, 22.157),
    (57.731, 21.914),
    (57.772, 21.692),
    (57.888, 21.581),
    (58.300, 21.348),
    (58.540, 21.387),
    (58.606, 21.421),
    (58.660, 21.488),
    (58.699, 21.577),
    (58.831, 21.660),
    (58.870, 21.632),
    (58.923, 21.615),
    (59.069, 21.825),
    (59.261, 22.290),
    (59.306, 22.487),
    (59.313, 22.602),
    (59.310, 22.721),
    (59.479, 23.105),
    (59.501, 23.183),
    (59.500, 23.515),
    (59.721, 24.471),
    (59.845, 25.576),
    (59.834, 26.006),
    (59.916, 26.134),
    (59.959, 26.452),
    (59.833, 26.657),
    (59.677, 26.840),
    (59.637, 27.056),
    (59.671, 27.390),
    (59.561, 27.830),
    (59.502, 27.977),
    (59.490, 28.064),
    (59.477, 28.077),
    (59.471, 28.114),
    (59.432, 28.164),
    (59.413, 28.218),
    (59.386, 28.245),
    (59.341, 28.224),
    (59.275, 28.154),
    (59.270, 28.035),
    (59.257, 27.978),
    (59.233, 27.937),
    (59.178, 27.923),
    (59.124, 27.844),
    (59.102, 27.845),
    (59.086, 27.824),
    (59.065, 27.828),
    (59.049, 27.806),
    (59.021, 27.799),
    (59.016, 27.778),
    (58.970, 27.772),
    (58.781, 27.395),
    (58.364, 27.590),
    (58.299, 27.522),
    (58.229, 27.526),
    (58.128, 27.630),
    (58.105, 27.676),
    (58.053, 27.642),
    (58.045, 27.644),
    (58.004, 27.741),
    (57.973, 27.715),
    (57.948, 27.725),
    (57.945, 27.710),
    (57.939, 27.716),
    (57.935, 27.751),
    (57.920, 27.753),
    (57.914, 27.772),
    (57.913, 27.851),
    (57.859, 27.853),
    (57.845, 27.830),
    (57.833, 27.832),
    (57.813, 27.783),
    (57.816, 27.767),
    (57.803, 27.746),
    (57.819, 27.705),
    (57.822, 27.628),
    (57.798, 27.570),
    (57.781, 27.576),
    (57.777, 27.548),
    (57.771, 27.546),
    (57.748, 27.592),
    (57.744, 27.574),
    (57.721, 27.597),
    (57.717, 27.561),
    (57.682, 27.559),
    (57.692, 27.516),
    (57.673, 27.482),
    (57.691, 27.457),
    (57.681, 27.446),
    (57.682, 27.430),
    (57.652, 27.412),
    (57.638, 27.441),
    (57.626, 27.433),
    (57.620, 27.462),
    (57.577, 27.391),
    (57.574, 27.357),
    (57.492, 27.400),
    (57.485, 27.382),
    (57.533, 27.247),
    (57.532, 27.189),
    (57.550, 27.085),
    (57.579, 26.989),
    (57.587, 26.996),
    (57.581, 26.905),
    (57.607, 26.912),
    (57.609, 26.903),
    (57.582, 26.895),
    (57.565, 26.823),
    (57.539, 26.799),
    (57.549, 26.705),
    (57.537, 26.700),
    (57.536, 26.666),
    (57.524, 26.650),
    (57.479, 26.654),
    (57.497, 26.587),
    (57.498, 26.514),
    (57.513, 26.471),
    (57.556, 26.445),
    (57.554, 26.396),
    (57.564, 26.347),
    (57.556, 26.316),
    (57.576, 26.291),
    (57.579, 26.257),
    (57.615, 26.213),
    (57.650, 26.196),
    (57.655, 26.177),
    (57.668, 26.178),
    (57.679, 26.132),
    (57.708, 26.160),
    (57.721, 26.112),
    (57.739, 26.105),
    (57.746, 26.079),
    (57.739, 26.056),
    (57.750, 26.031),
    (57.737, 25.991),
    (57.741, 25.978),
    (57.827, 26.013),
    (57.835, 25.993),
    (57.825, 25.965),
    (57.834, 25.928),
    (57.825, 25.866),
    (57.838, 25.857),
    (57.851, 25.794),
    (57.893, 25.760),
    (57.900, 25.743),
    (57.885, 25.718),
    (57.888, 25.647),
    (57.898, 25.645),
    (57.902, 25.623),
    (57.917, 25.611),
    (57.932, 25.549),
    (57.926, 25.537),
    (57.952, 25.525),
    (57.948, 25.459),
    (57.959, 25.467),
    (57.998, 25.389),
    (58.007, 25.413),
    (58.012, 25.398),
    (58.019, 25.338),
    (57.976, 25.337),
    (57.976, 25.222),
    (57.996, 25.222),
    (58.026, 25.167),
    (58.029, 25.185),
    (58.039, 25.188),
    (58.058, 25.169),
    (58.059, 25.139),
    (58.048, 25.138),
    (58.048, 25.096),
    (58.002, 25.042),
    (57.992, 24.962),
    (57.947, 24.822),
    (57.970, 24.793),
    (57.968, 24.778),
    (57.950, 24.779),
    (57.946, 24.734),
    (57.916, 24.691),
    (57.937, 24.662),
    (57.920, 24.647),
    (57.928, 24.598),
    (57.937, 24.581),
    (57.915, 24.535),
    (57.910, 24.494),
    (57.870, 24.495),
    (57.835, 24.385),
    (57.855, 24.379),
]];

pub static ESTONIA: Source = Source {
    id: "estonia",
    name: "Maa- ja Ruumiamet",
    grid: Grid::Fixed(Crs::Lest97),
    outline: OUTLINE,
    laser: Laser::Whole {
        locate: Locate::Lookup(lookup),
        tile_mb: 60,
    },
    photo: Photo::Wms {
        url: "https://kaart.maaamet.ee/wms/fotokaart",
        layer: "EESTIFOTO",
    },
    // From the photo calibration in Tallinn-Nõmme (2024); a last resort.
    intensity_defaults: (46_900.0, 49_500.0),
    classes: LaserClasses {
        ground: &[2],
        building: &[6],
        object: &[1, 5],
        vegetation: &[5],
        footprint: &[],
        unsorted: &[],
    },
    laser_plants: true,
    strict_pools: false,
    paving_brighter: false,
    attribution: "Maa- ja Ruumiamet (open data licence)",
};

/// The sheet number of the 1 km square with south-west corner (`e_km`, `n_km`).
fn sheet(e_km: i32, n_km: i32) -> i32 {
    (n_km - 6000) * 1000 + e_km
}

/// The latest regular flight of each sheet over the rectangle, from the
/// sheets' pages.
fn lookup(
    client: &reqwest::blocking::Client,
    _: &Sheet,
    (e0, n0, e1, n1): Rect,
) -> Result<TileUrls, String> {
    let mut out = Vec::new();
    for te in (e0 / 1000.0).floor() as i32..=((e1 - 1e-6) / 1000.0).floor() as i32 {
        for tn in (n0 / 1000.0).floor() as i32..=((n1 - 1e-6) / 1000.0).floor() as i32 {
            let k = sheet(te, tn);
            let page = format!("{PAGE}&kaardiruut={k}&andmetyyp=lidar_laz_tava&page_id=614");
            let body = match super::fetch::get_bytes(client, &page) {
                Ok(b) => b,
                Err(super::fetch::Failure::Missing) => continue,
                Err(e) => return Err(format!("{page}: {e}")),
            };
            if let Some(file) = latest_regular(&String::from_utf8_lossy(&body), k) {
                out.push((
                    (te, tn),
                    format!(
                        "{PAGE}&kaardiruut={k}&andmetyyp=lidar_laz_tava&dl=1&f={file}&page_id=614"
                    ),
                ));
            }
        }
    }
    Ok(out)
}

/// The file of the latest `tava` flight a sheet's page links.
fn latest_regular(page: &str, sheet: i32) -> Option<String> {
    let prefix = format!("f={sheet}_");
    page.match_indices(&prefix)
        .filter_map(|(at, _)| {
            let rest = &page[at + 2..];
            let name = &rest[..rest.find(".laz")? + 4];
            let year: u32 = name.split('_').nth(1)?.parse().ok()?;
            name.ends_with("_tava.laz")
                .then(|| (year, name.to_string()))
        })
        .max()
        .map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheets_are_numbered_from_their_corner() {
        // Tallinn-Nõmme, E 539 km, N 6583 km.
        assert_eq!(sheet(539, 6583), 583539);
    }

    #[test]
    fn the_latest_regular_flight_is_read() {
        let page = r#"<a href="index.php?f=583539_2023_madal.laz&amp;x">583539_2023_madal.laz</a>
            <a href="index.php?f=583539_2020_tava.laz&amp;x">583539_2020_tava.laz</a>
            <a href="index.php?f=583539_2024_tava.laz&amp;x">583539_2024_tava.laz</a>
            <a href="index.php?f=583539_2022_mets.laz&amp;x">583539_2022_mets.laz</a>"#;
        assert_eq!(
            latest_regular(page, 583539).as_deref(),
            Some("583539_2024_tava.laz")
        );
        assert_eq!(latest_regular("no files here", 583539), None);
    }
}
