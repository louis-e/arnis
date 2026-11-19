//! Luxembourg: the national LiDAR 2024 survey and orthophoto, both CC0.
//!
//! * LiDAR 2024: COPC tiles of 500 m in LUREF / Luxembourg TM (EPSG:2169),
//!   named `{xmin}_{ymax}.laz`, packed nine at a time into uncompressed zips of
//!   1.5 km, `lidar2024-c{col:03}-r{row:03}.zip`, the column and row counted
//!   from 48 500 E and 56 500 N. The zips' addresses carry their upload time, so
//!   they are found through the dataset's record on data.public.lu
//!   (`https://data.public.lu/api/1/datasets/lidar-2024-releve-3d-du-territoire-luxembourgeois/`,
//!   one resource per zip; `lidar2024-ta.gpkg` beside them lists every tile).
//!   A zip stores its tiles uncompressed, so each is read in part like any
//!   COPC file, from where it starts inside the zip (the zip's directory, read
//!   from its last bytes, says where). Flown leaf-off in February to April 2024,
//!   about 35 points per m²; classes 1 other, 2 ground, 3/4/5 low, medium and
//!   high vegetation, 6 building, 7 noise, 9 water, 13 bridge, 15 power line.
//!   The points carry colour and their octree is shallow, so the level that
//!   reaches the density asked for holds about 15 points per m² and costs about
//!   330 MB a km² (measured in Bertrange); the level above it, about 4, read
//!   half as much but took cars and shade for pools.
//! * Orthophoto: the latest national orthophoto through the open-data WMS of
//!   geoportail.lu (`ortho_latest`).

use super::crs::Crs;
use super::fetch;
use super::source::{Grid, Laser, LaserClasses, Locate, Photo, Rect, Sheet, Source, TileUrls};

const DATASET: &str =
    "https://data.public.lu/api/1/datasets/lidar-2024-releve-3d-du-territoire-luxembourgeois/";
/// The zips' grid: origin and side in metres.
const ZIP_E0: i32 = 48_500;
const ZIP_N0: i32 = 56_500;
const ZIP_SIDE: i32 = 1_500;

/// The country, simplified to about half a kilometre and pushed out by about
/// two. From OpenStreetMap (relation 2171347), © OpenStreetMap contributors,
/// ODbL.
const OUTLINE: &[&[(f64, f64)]] = &[&[
    (49.826, 5.714),
    (49.830, 5.710),
    (49.845, 5.714),
    (49.854, 5.690),
    (49.863, 5.691),
    (49.867, 5.716),
    (49.877, 5.718),
    (49.874, 5.726),
    (49.880, 5.726),
    (49.887, 5.700),
    (49.916, 5.717),
    (49.924, 5.734),
    (49.978, 5.753),
    (49.979, 5.776),
    (49.990, 5.796),
    (50.024, 5.788),
    (50.035, 5.828),
    (50.041, 5.824),
    (50.047, 5.833),
    (50.056, 5.822),
    (50.082, 5.838),
    (50.091, 5.862),
    (50.122, 5.870),
    (50.138, 5.901),
    (50.144, 5.931),
    (50.161, 5.929),
    (50.163, 5.934),
    (50.189, 5.938),
    (50.194, 5.994),
    (50.208, 6.044),
    (50.184, 6.053),
    (50.190, 6.081),
    (50.188, 6.112),
    (50.171, 6.153),
    (50.162, 6.148),
    (50.148, 6.176),
    (50.141, 6.158),
    (50.134, 6.174),
    (50.123, 6.165),
    (50.120, 6.171),
    (50.110, 6.163),
    (50.095, 6.181),
    (50.092, 6.167),
    (50.076, 6.182),
    (50.076, 6.162),
    (50.056, 6.151),
    (50.043, 6.165),
    (50.037, 6.162),
    (50.027, 6.186),
    (50.003, 6.176),
    (49.998, 6.200),
    (50.002, 6.207),
    (49.975, 6.217),
    (49.968, 6.225),
    (49.968, 6.273),
    (49.962, 6.277),
    (49.945, 6.253),
    (49.910, 6.264),
    (49.896, 6.281),
    (49.890, 6.313),
    (49.893, 6.327),
    (49.867, 6.343),
    (49.870, 6.376),
    (49.837, 6.415),
    (49.832, 6.447),
    (49.848, 6.471),
    (49.830, 6.501),
    (49.835, 6.505),
    (49.825, 6.549),
    (49.799, 6.562),
    (49.790, 6.547),
    (49.787, 6.552),
    (49.774, 6.541),
    (49.758, 6.553),
    (49.744, 6.529),
    (49.721, 6.561),
    (49.711, 6.539),
    (49.708, 6.540),
    (49.681, 6.496),
    (49.676, 6.479),
    (49.664, 6.466),
    (49.658, 6.471),
    (49.612, 6.447),
    (49.586, 6.416),
    (49.585, 6.408),
    (49.554, 6.418),
    (49.555, 6.415),
    (49.527, 6.388),
    (49.505, 6.396),
    (49.429, 6.397),
    (49.427, 6.392),
    (49.449, 6.344),
    (49.449, 6.323),
    (49.457, 6.310),
    (49.464, 6.284),
    (49.461, 6.274),
    (49.471, 6.263),
    (49.478, 6.263),
    (49.479, 6.251),
    (49.486, 6.251),
    (49.490, 6.230),
    (49.488, 6.227),
    (49.488, 6.198),
    (49.474, 6.210),
    (49.475, 6.163),
    (49.468, 6.150),
    (49.458, 6.148),
    (49.456, 6.128),
    (49.429, 6.130),
    (49.440, 6.067),
    (49.429, 6.058),
    (49.431, 5.997),
    (49.435, 5.995),
    (49.426, 5.968),
    (49.462, 5.945),
    (49.459, 5.937),
    (49.480, 5.923),
    (49.482, 5.913),
    (49.478, 5.903),
    (49.481, 5.864),
    (49.497, 5.826),
    (49.505, 5.823),
    (49.519, 5.806),
    (49.543, 5.766),
    (49.570, 5.814),
    (49.579, 5.810),
    (49.584, 5.823),
    (49.604, 5.817),
    (49.618, 5.848),
    (49.627, 5.848),
    (49.632, 5.855),
    (49.641, 5.850),
    (49.651, 5.872),
    (49.663, 5.844),
    (49.679, 5.833),
    (49.703, 5.842),
    (49.709, 5.794),
    (49.739, 5.802),
    (49.744, 5.792),
    (49.756, 5.789),
    (49.775, 5.771),
    (49.776, 5.727),
    (49.808, 5.724),
    (49.821, 5.707),
    (49.826, 5.714),
]];

pub static LUXEMBOURG: Source = Source {
    id: "luxembourg",
    name: "LiDAR 2024 Luxembourg",
    grid: Grid::Fixed(Crs::Luref),
    outline: OUTLINE,
    laser: Laser::Copc {
        locate: Locate::Lookup(lookup),
        mb_per_km2: 330,
        own_grid: false,
    },
    photo: Photo::Wms {
        url: "https://wms.geoportail.lu/opendata/service",
        layer: "ortho_latest",
    },
    // From the photo calibration in Bertrange (2024); a last resort.
    intensity_defaults: (44_500.0, 48_600.0),
    classes: LaserClasses {
        ground: &[2],
        building: &[6],
        object: &[1, 3, 4, 5],
        vegetation: &[3, 4, 5],
        footprint: &[],
        unsorted: &[],
    },
    laser_plants: true,
    strict_pools: false,
    paving_brighter: false,
    attribution: "Administration du cadastre et de la topographie, Luxembourg (CC0)",
};

/// The zip holding the 500 m tile with south-west corner (`e`, `n`): its column
/// and row.
fn zip_of(e: i32, n: i32) -> (i32, i32) {
    (
        (e - ZIP_E0).div_euclid(ZIP_SIDE) + 1,
        (n - ZIP_N0).div_euclid(ZIP_SIDE) + 1,
    )
}

/// Every 1 km tile over the rectangle and its four 500 m pieces, each named
/// `{zip}#at={offset}` where it starts inside its zip.
fn lookup(
    client: &reqwest::blocking::Client,
    _: &Sheet,
    (e0, n0, e1, n1): Rect,
) -> Result<TileUrls, String> {
    let record = fetch::get_json(client, DATASET)?;
    let zips: std::collections::HashMap<String, String> = record["resources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some((
                r["title"].as_str()?.to_string(),
                r["url"].as_str()?.to_string(),
            ))
        })
        .collect();
    // Each zip's address and stored files, read once; `None` where there is no zip.
    let mut dirs: std::collections::HashMap<(i32, i32), Option<Zip>> = Default::default();
    let mut out = Vec::new();
    for te in (e0 / 1000.0).floor() as i32..=((e1 - 1e-6) / 1000.0).floor() as i32 {
        for tn in (n0 / 1000.0).floor() as i32..=((n1 - 1e-6) / 1000.0).floor() as i32 {
            for (de, dn) in [(0, 0), (500, 0), (0, 500), (500, 500)] {
                let (e, n) = (te * 1000 + de, tn * 1000 + dn);
                let zip = zip_of(e, n);
                let dir = match dirs.entry(zip) {
                    std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                    std::collections::hash_map::Entry::Vacant(v) => {
                        let title = format!("lidar2024-c{:03}-r{:03}.zip", zip.0, zip.1);
                        v.insert(match zips.get(&title) {
                            Some(url) => Some((url.clone(), directory(client, url)?)),
                            None => None,
                        })
                    }
                };
                let Some((url, entries)) = dir else {
                    continue;
                };
                let name = format!("{e}_{}.laz", n + 500);
                if let Some(entry) = entries.iter().find(|x| x.name == name) {
                    let at = data_start(client, url, entry.local)?;
                    out.push(((te, tn), format!("{url}#at={at}")));
                }
            }
        }
    }
    Ok(out)
}

/// A zip's address and the files it stores.
type Zip = (String, Vec<Entry>);

/// One file in a zip's central directory.
#[derive(Debug, PartialEq)]
struct Entry {
    name: String,
    /// Where its local header starts.
    local: u64,
}

/// The stored (uncompressed) files a zip lists, from its last 64 KiB.
fn directory(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<Entry>, String> {
    let tail = fetch::get_tail(client, url, 65_536).map_err(|e| format!("{url}: {e}"))?;
    parse_directory(&tail).ok_or_else(|| format!("{url}: no zip directory"))
}

fn parse_directory(tail: &[u8]) -> Option<Vec<Entry>> {
    let u16_at = |b: &[u8], o: usize| -> Option<u64> {
        Some(u64::from(u16::from_le_bytes(
            b.get(o..o + 2)?.try_into().ok()?,
        )))
    };
    let u32_at = |b: &[u8], o: usize| -> Option<u64> {
        Some(u64::from(u32::from_le_bytes(
            b.get(o..o + 4)?.try_into().ok()?,
        )))
    };
    let end = tail.windows(4).rposition(|w| w == b"PK\x05\x06")?;
    let count = u16_at(tail, end + 10)?;
    let size = u32_at(tail, end + 12)? as usize;
    let mut p = end.checked_sub(size)?;
    let mut out = Vec::new();
    for _ in 0..count {
        if tail.get(p..p + 4)? != b"PK\x01\x02" {
            return None;
        }
        let method = u16_at(tail, p + 10)?;
        let (nl, xl, cl) = (
            u16_at(tail, p + 28)? as usize,
            u16_at(tail, p + 30)? as usize,
            u16_at(tail, p + 32)? as usize,
        );
        let local = u32_at(tail, p + 42)?;
        let name = String::from_utf8_lossy(tail.get(p + 46..p + 46 + nl)?).to_string();
        if method == 0 {
            out.push(Entry { name, local });
        }
        p += 46 + nl + xl + cl;
    }
    Some(out)
}

/// Where a stored file's bytes start: after its local header, whose name and
/// extra field lengths are its own.
fn data_start(client: &reqwest::blocking::Client, url: &str, local: u64) -> Result<u64, String> {
    let h = fetch::get_range(client, url, local, 30).map_err(|e| format!("{url}: {e}"))?;
    if h.len() < 30 || &h[..4] != b"PK\x03\x04" {
        return Err(format!("{url}: no local header at {local}"));
    }
    let nl = u64::from(u16::from_le_bytes([h[26], h[27]]));
    let xl = u64::from(u16::from_le_bytes([h[28], h[29]]));
    Ok(local + 30 + nl + xl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_fall_in_their_zip() {
        // From lidar2024-ta.gpkg: 79000_79500 (south-west 79000, 79000) is in
        // Lidar2024_C021_R016, and so is its neighbour to the north.
        assert_eq!(zip_of(79_000, 79_000), (21, 16));
        assert_eq!(zip_of(79_000, 80_000), (21, 16));
        assert_eq!(zip_of(80_000, 79_000), (22, 16));
        assert_eq!(zip_of(49_500, 96_000), (1, 27));
    }

    #[test]
    fn a_zip_directory_lists_its_stored_files() {
        // Two stored entries, as lidar2024-c001-r027.zip has.
        let mut z = Vec::new();
        let mut cd = Vec::new();
        for (name, local) in [("49500_96500.laz", 0u32), ("49500_97000.laz", 1_496_228)] {
            cd.extend_from_slice(b"PK\x01\x02");
            cd.extend_from_slice(&[0; 6]);
            cd.extend_from_slice(&0u16.to_le_bytes()); // stored
            cd.extend_from_slice(&[0; 16]);
            cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&[0; 12]);
            cd.extend_from_slice(&local.to_le_bytes());
            cd.extend_from_slice(name.as_bytes());
        }
        z.extend_from_slice(&[7; 100]);
        z.extend_from_slice(&cd);
        z.extend_from_slice(b"PK\x05\x06");
        z.extend_from_slice(&[0; 6]);
        z.extend_from_slice(&2u16.to_le_bytes());
        z.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        z.extend_from_slice(&[0; 6]);
        let d = parse_directory(&z).unwrap();
        assert_eq!(
            d,
            vec![
                Entry {
                    name: "49500_96500.laz".into(),
                    local: 0
                },
                Entry {
                    name: "49500_97000.laz".into(),
                    local: 1_496_228
                },
            ]
        );
    }
}
