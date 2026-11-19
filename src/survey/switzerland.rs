//! Switzerland: swisstopo's swissSURFACE3D laser scans and SWISSIMAGE photos.
//!
//! * swissSURFACE3D: tiles of 1 km in LV95 (EPSG:2056), found through the
//!   federal STAC API. The campaigns flown from 2024 on are published as COPC
//!   and read in part; where a tile has none yet, the latest earlier campaign
//!   (2017 to 2023) is read whole from its zip, one LAS file of about 360 MB
//!   packed to about 160 (135 MB a km² measured in Muri bei Bern), and left out
//!   where only the area's margin reaches it, like any whole file. About 12 to
//!   20 points per m², classes 2 ground, 3 vegetation, 6 building, 9 water,
//!   17 bridge.
//! * SWISSIMAGE: 10 cm RGB through the federal WMS.
//!
//! Free geodata of the Federal Office of Topography swisstopo (Open Government
//! Data, attribution required).

use super::crs::Crs;
use super::source::{Grid, Laser, LaserClasses, Locate, Photo, Rect, Sheet, Source, TileUrls};

/// Switzerland, simplified to about a kilometre and pushed out by about two.
/// From OpenStreetMap (relation 51701), © OpenStreetMap contributors, ODbL.
const OUTLINE: &[&[(f64, f64)]] = &[&[
    (46.140, 5.935),
    (46.170, 5.960),
    (46.190, 5.945),
    (46.225, 5.955),
    (46.260, 6.030),
    (46.265, 6.085),
    (46.305, 6.090),
    (46.360, 6.135),
    (46.405, 6.045),
    (46.475, 6.055),
    (46.540, 6.120),
    (46.585, 6.090),
    (46.705, 6.265),
    (46.780, 6.425),
    (46.815, 6.410),
    (46.875, 6.445),
    (46.915, 6.415),
    (46.950, 6.425),
    (46.995, 6.490),
    (46.990, 6.525),
    (47.015, 6.625),
    (47.130, 6.730),
    (47.155, 6.810),
    (47.190, 6.830),
    (47.240, 6.915),
    (47.310, 6.935),
    (47.315, 6.990),
    (47.345, 7.005),
    (47.330, 6.885),
    (47.345, 6.860),
    (47.385, 6.865),
    (47.425, 6.920),
    (47.450, 6.925),
    (47.465, 6.970),
    (47.515, 6.975),
    (47.515, 7.205),
    (47.490, 7.225),
    (47.455, 7.200),
    (47.460, 7.240),
    (47.445, 7.265),
    (47.460, 7.295),
    (47.460, 7.405),
    (47.510, 7.415),
    (47.520, 7.435),
    (47.500, 7.480),
    (47.550, 7.480),
    (47.610, 7.580),
    (47.600, 7.620),
    (47.615, 7.635),
    (47.620, 7.705),
    (47.560, 7.705),
    (47.570, 7.775),
    (47.610, 7.820),
    (47.610, 7.890),
    (47.590, 7.925),
    (47.565, 7.935),
    (47.580, 7.955),
    (47.580, 8.085),
    (47.605, 8.105),
    (47.640, 8.225),
    (47.630, 8.300),
    (47.590, 8.335),
    (47.585, 8.435),
    (47.630, 8.455),
    (47.660, 8.390),
    (47.715, 8.390),
    (47.735, 8.430),
    (47.765, 8.440),
    (47.790, 8.475),
    (47.800, 8.545),
    (47.830, 8.565),
    (47.810, 8.635),
    (47.820, 8.665),
    (47.785, 8.705),
    (47.770, 8.755),
    (47.735, 8.755),
    (47.760, 8.810),
    (47.715, 8.890),
    (47.670, 8.895),
    (47.705, 9.005),
    (47.700, 9.105),
    (47.675, 9.175),
    (47.680, 9.260),
    (47.640, 9.405),
    (47.570, 9.505),
    (47.560, 9.570),
    (47.505, 9.580),
    (47.460, 9.680),
    (47.415, 9.670),
    (47.400, 9.695),
    (47.375, 9.695),
    (47.350, 9.670),
    (47.335, 9.615),
    (47.310, 9.610),
    (47.285, 9.570),
    (47.185, 9.505),
    (47.145, 9.530),
    (47.085, 9.535),
    (47.070, 9.565),
    (47.080, 9.695),
    (47.060, 9.740),
    (47.060, 9.790),
    (47.035, 9.835),
    (47.040, 9.885),
    (47.005, 9.910),
    (46.950, 9.900),
    (46.920, 10.030),
    (46.880, 10.070),
    (46.880, 10.100),
    (46.865, 10.110),
    (46.885, 10.210),
    (46.905, 10.205),
    (46.950, 10.230),
    (46.945, 10.285),
    (47.005, 10.330),
    (47.020, 10.375),
    (47.015, 10.410),
    (46.975, 10.445),
    (46.945, 10.510),
    (46.835, 10.490),
    (46.790, 10.460),
    (46.750, 10.465),
    (46.730, 10.435),
    (46.700, 10.435),
    (46.680, 10.410),
    (46.660, 10.415),
    (46.655, 10.470),
    (46.625, 10.510),
    (46.525, 10.485),
    (46.510, 10.445),
    (46.530, 10.415),
    (46.530, 10.285),
    (46.555, 10.275),
    (46.560, 10.230),
    (46.595, 10.220),
    (46.605, 10.190),
    (46.585, 10.120),
    (46.535, 10.075),
    (46.455, 10.070),
    (46.450, 10.150),
    (46.415, 10.185),
    (46.375, 10.180),
    (46.340, 10.125),
    (46.270, 10.195),
    (46.245, 10.195),
    (46.210, 10.155),
    (46.210, 10.090),
    (46.195, 10.070),
    (46.210, 10.035),
    (46.250, 10.035),
    (46.270, 9.980),
    (46.315, 9.960),
    (46.340, 9.965),
    (46.355, 9.945),
    (46.345, 9.935),
    (46.355, 9.895),
    (46.315, 9.785),
    (46.325, 9.745),
    (46.300, 9.745),
    (46.270, 9.715),
    (46.265, 9.620),
    (46.280, 9.550),
    (46.370, 9.440),
    (46.460, 9.440),
    (46.445, 9.405),
    (46.485, 9.350),
    (46.480, 9.295),
    (46.455, 9.300),
    (46.435, 9.285),
    (46.405, 9.305),
    (46.320, 9.320),
    (46.265, 9.275),
    (46.220, 9.265),
    (46.200, 9.230),
    (46.155, 9.200),
    (46.140, 9.150),
    (46.105, 9.105),
    (46.080, 9.110),
    (46.045, 9.090),
    (46.035, 9.035),
    (46.000, 9.050),
    (45.970, 9.030),
    (45.950, 9.035),
    (45.910, 9.110),
    (45.890, 9.110),
    (45.855, 9.070),
    (45.800, 9.040),
    (45.820, 8.895),
    (45.855, 8.895),
    (45.875, 8.915),
    (45.935, 8.875),
    (45.970, 8.825),
    (45.970, 8.775),
    (45.985, 8.765),
    (46.020, 8.775),
    (46.070, 8.820),
    (46.095, 8.740),
    (46.075, 8.710),
    (46.105, 8.640),
    (46.100, 8.610),
    (46.200, 8.515),
    (46.215, 8.455),
    (46.240, 8.425),
    (46.310, 8.410),
    (46.345, 8.445),
    (46.445, 8.435),
    (46.410, 8.335),
    (46.370, 8.335),
    (46.320, 8.245),
    (46.290, 8.225),
    (46.280, 8.140),
    (46.260, 8.125),
    (46.190, 8.185),
    (46.130, 8.170),
    (46.090, 8.110),
    (46.080, 8.050),
    (46.035, 8.055),
    (45.980, 8.005),
    (45.975, 7.915),
    (45.905, 7.890),
    (45.895, 7.870),
    (45.895, 7.800),
    (45.920, 7.760),
    (45.905, 7.745),
    (45.905, 7.710),
    (45.955, 7.660),
    (45.960, 7.565),
    (45.935, 7.550),
    (45.940, 7.505),
    (45.915, 7.485),
    (45.875, 7.390),
    (45.895, 7.295),
    (45.870, 7.270),
    (45.865, 7.220),
    (45.840, 7.200),
    (45.855, 7.150),
    (45.840, 7.090),
    (46.035, 6.935),
    (46.025, 6.870),
    (46.045, 6.850),
    (46.105, 6.865),
    (46.125, 6.780),
    (46.165, 6.770),
    (46.185, 6.790),
    (46.215, 6.785),
    (46.275, 6.835),
    (46.315, 6.770),
    (46.345, 6.750),
    (46.375, 6.755),
    (46.385, 6.780),
    (46.410, 6.790),
    (46.435, 6.680),
    (46.435, 6.520),
    (46.395, 6.435),
    (46.385, 6.340),
    (46.345, 6.265),
    (46.325, 6.250),
    (46.295, 6.275),
    (46.275, 6.265),
    (46.285, 6.305),
    (46.265, 6.330),
    (46.235, 6.330),
    (46.205, 6.305),
    (46.175, 6.215),
    (46.145, 6.195),
    (46.125, 6.155),
    (46.130, 6.060),
    (46.115, 6.045),
    (46.125, 5.995),
    (46.110, 5.945),
    (46.140, 5.935),
]];

pub static SWITZERLAND: Source = Source {
    id: "switzerland",
    name: "swissSURFACE3D",
    grid: Grid::Fixed(Crs::Lv95),
    outline: OUTLINE,
    laser: Laser::Copc {
        locate: Locate::Lookup(lookup),
        mb_per_km2: 90,
        own_grid: false,
    },
    photo: Photo::Wms {
        url: "https://wms.geo.admin.ch/",
        layer: "ch.swisstopo.swissimage",
    },
    // From the photo calibration in Zurich-Wollishofen (2024, 16-bit); a last resort.
    intensity_defaults: (41_900.0, 45_900.0),
    classes: LaserClasses {
        ground: &[2],
        building: &[6],
        object: &[1, 3],
        vegetation: &[3],
        footprint: &[],
        unsorted: &[],
    },
    laser_plants: true,
    strict_pools: false,
    paving_brighter: false,
    attribution: "Federal Office of Topography swisstopo – swissSURFACE3D, SWISSIMAGE",
};

/// The tiles over a rectangle, from the STAC API: the latest COPC campaign of
/// a tile, else its latest zipped one.
fn lookup(
    client: &reqwest::blocking::Client,
    _: &Sheet,
    (e0, n0, e1, n1): Rect,
) -> Result<TileUrls, String> {
    let crs = Crs::Lv95;
    let corners = [(e0, n0), (e0, n1), (e1, n0), (e1, n1)].map(|(e, n)| crs.inverse(e, n));
    let (mut s, mut w, mut n, mut e) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (lat, lon) in corners {
        s = s.min(lat);
        n = n.max(lat);
        w = w.min(lon);
        e = e.max(lon);
    }
    let mut url = Some(format!(
        "https://data.geo.admin.ch/api/stac/v0.9/collections/ch.swisstopo.swisssurface3d/items\
         ?bbox={w:.6},{s:.6},{e:.6},{n:.6}&limit=100"
    ));
    let mut best: std::collections::HashMap<(i32, i32), (u8, String, String)> = Default::default();
    let mut pages = 0;
    while let Some(u) = url.take() {
        pages += 1;
        if pages > 10 {
            break;
        }
        let json = super::fetch::get_json(client, &u)?;
        for f in json["features"].as_array().into_iter().flatten() {
            let Some(id) = f["id"].as_str() else { continue };
            // swisssurface3d_2024_2682-1247: campaign, then the tile's corner in km.
            let mut parts = id.rsplitn(3, '_');
            let (Some(tile), Some(year)) = (parts.next(), parts.next()) else {
                continue;
            };
            let Some((x, y)) = tile.split_once('-') else {
                continue;
            };
            let (Ok(x), Ok(y)) = (x.parse::<i32>(), y.parse::<i32>()) else {
                continue;
            };
            let hrefs = || {
                f["assets"]
                    .as_object()
                    .into_iter()
                    .flat_map(|a| a.values())
                    .filter_map(|v| v["href"].as_str())
            };
            // COPC ranks above any zip, then the later campaign.
            let rank = match hrefs().find(|h| h.ends_with(".copc.laz")) {
                Some(h) => (1, year.to_string(), h.to_string()),
                None => match hrefs().find(|h| h.ends_with(".las.zip")) {
                    Some(h) => (0, year.to_string(), h.to_string()),
                    None => continue,
                },
            };
            let key = (x, y);
            if best.get(&key).is_none_or(|b| *b < rank) {
                best.insert(key, rank);
            }
        }
        url = json["links"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|l| l["rel"] == "next")
            .and_then(|l| l["href"].as_str())
            .map(String::from);
    }
    Ok(best.into_iter().map(|(k, (_, _, u))| (k, u)).collect())
}
