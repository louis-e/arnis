//! What a survey source is: its grid, where its laser tiles and photos live, how
//! its laser classes read, and where it has data at all.

use super::crs::Crs;
use crate::coordinate_system::geographic::LLBBox;

/// Which laser classes mean ground, building and other above-ground objects.
/// Everything else (noise, water, bridges, unclassified overlap) is ignored.
pub struct LaserClasses {
    pub ground: &'static [u8],
    pub building: &'static [u8],
    /// Everything above the ground that is not a building, vegetation included.
    pub object: &'static [u8],
    /// The object classes that are vegetation. Empty where the source does not
    /// tell vegetation apart; the pulse's returns then stand in for it. A
    /// ground class listed here too is low plants on the ground (Latvia's
    /// lawns and meadows): its returns count as ground of the darkest
    /// intensity, the side lawn reads on there, so a cell mostly of them reads
    /// as planted (see [`super::classify::calibrate`]).
    pub vegetation: &'static [u8],
    /// Synthetic points that mark a building's outline on the ground rather
    /// than a return: where a source has no building class (the German states'
    /// AdV scheme), the object points over them and a metre around, other than
    /// the plants, are its roof. Empty where the source classes buildings.
    pub footprint: &'static [u8],
    /// Object classes left unsorted beside the vegetation classes, whose plants
    /// the pulse's returns tell: where only some of a source's scans sort out
    /// vegetation, the rest leave it all in these. A scan that has no building
    /// point either leaves its roofs here too, and they are found from the
    /// pulses (`laser::find_roofs`).
    pub unsorted: &'static [u8],
}

/// A rectangle in the source's grid metres: west, south, east, north.
pub type Rect = (f64, f64, f64, f64);

/// Tiles (south-west corners in km) and the URL of each; a tile listed more than
/// once is made of several files, all read into it (COPC and EPT only).
pub type TileUrls = Vec<((i32, i32), String)>;

/// Where a source's laser points come from.
pub enum Laser {
    /// Plain LAZ tiles, read whole.
    Whole {
        locate: Locate,
        /// Typical size of a tile, for the download estimate.
        tile_mb: u32,
    },
    /// Cloud-optimised point clouds (COPC), read in part: only the octree nodes
    /// over the area, and only as deep as the density needs.
    Copc {
        locate: Locate,
        /// Typical download per square kilometre of an area a few hundred metres
        /// across, for the estimate: more than a whole tile's share, since the
        /// coarse levels of the octree span the whole tile.
        mb_per_km2: u32,
        /// Whether each file names its own grid (its WKT record), which may be
        /// another than the area's: a survey in the next UTM zone.
        own_grid: bool,
    },
    /// Entwine point tiles (EPT): one octree per survey, each node its own small
    /// LAZ file, in Web Mercator. The locator gives each tile its survey's
    /// `ept.json`; only the nodes over the area are read, down to the density
    /// needed.
    Ept { locate: Locate, mb_per_km2: u32 },
}

/// How a source's tile URLs are found.
pub enum Locate {
    /// Candidate URLs of a tile, best first; a missing one answers 403 or 404.
    Urls(fn(i32, i32) -> Vec<String>),
    /// One lookup over a rectangle: the URL of every tile the source has
    /// there (see [`TileUrls`]).
    Lookup(fn(&reqwest::blocking::Client, &Sheet, Rect) -> Result<TileUrls, String>),
}

/// The grid a source's areas are measured on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grid {
    Fixed(Crs),
    /// The NAD83 UTM zone of each area's middle, for sources that span many.
    Nad83Utm,
}

/// A source on the grid of one area. Derefs to the source.
#[derive(Clone, Copy)]
pub struct Sheet {
    pub source: &'static Source,
    pub crs: Crs,
}

impl std::ops::Deref for Sheet {
    type Target = Source;
    fn deref(&self) -> &Source {
        self.source
    }
}

impl Sheet {
    /// The source on the grid for an area around longitude `lon`.
    pub fn at(source: &'static Source, lon: f64) -> Sheet {
        let crs = match source.grid {
            Grid::Fixed(crs) => crs,
            Grid::Nad83Utm => Crs::Nad83Utm(super::crs::utm_zone(lon)),
        };
        Sheet { source, crs }
    }

    /// Cache folder of the sheet under the laser cache root: the source's, and
    /// one per zone where the grid changes with the area.
    pub fn dir(&self) -> std::path::PathBuf {
        match (self.source.grid, self.crs) {
            (Grid::Nad83Utm, Crs::Nad83Utm(zone)) => {
                std::path::Path::new(self.source.id).join(format!("utm{zone}"))
            }
            _ => std::path::PathBuf::from(self.source.id),
        }
    }

    /// The 1 km tiles (south-west corners in km) a grid rectangle touches, only
    /// those whose centre or a corner is inside the source's outline.
    pub fn tiles(&self, (e0, n0, e1, n1): Rect) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for te in (e0 / 1000.0).floor() as i32..=((e1 - 1e-6) / 1000.0).floor() as i32 {
            for tn in (n0 / 1000.0).floor() as i32..=((n1 - 1e-6) / 1000.0).floor() as i32 {
                let probe = [(0.5, 0.5), (0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)];
                if probe.iter().any(|&(fe, fn_)| {
                    let (lat, lon) = self.crs.inverse(
                        (f64::from(te) + fe) * 1000.0,
                        (f64::from(tn) + fn_) * 1000.0,
                    );
                    self.source.contains(lat, lon)
                }) {
                    out.push((te, tn));
                }
            }
        }
        out
    }
}

/// Where a source's aerial photos come from.
pub enum Photo {
    /// A tiled GeoTIFF per 1 km tile, 20 cm pixels on the metre grid, read block
    /// by block with range requests.
    TiledTiff(fn(i32, i32) -> String),
    /// A WMS that renders any rectangle of the source's grid.
    Wms {
        url: &'static str,
        layer: &'static str,
    },
    /// No open photo: the classes come from the laser alone.
    None,
    /// GeoTIFF sheets on a grid of their own, tiled, at a whole number of
    /// pixels a metre on the metre grid, read block by block: `sheet` gives
    /// the sheet holding a point of the grid, its address and its north-west
    /// corner.
    Sheets { sheet: SheetAt, px_per_m: usize },
}

/// The photo sheet holding a point of the grid (easting, northing): its
/// address and its north-west corner.
pub type SheetAt = fn(f64, f64) -> Option<(String, f64, f64)>;

pub struct Source {
    /// Cache folder and log name.
    pub id: &'static str,
    pub name: &'static str,
    /// The grid of the source's 1 km tiles.
    pub grid: Grid,
    /// Where the source has data, as rings of (lat, lon). Requests never leave it.
    pub outline: &'static [&'static [(f64, f64)]],
    pub laser: Laser,
    pub photo: Photo,
    /// Ground-return intensity under which a block is paving and over which it is
    /// planted, when neither the photo nor the laser alone can calibrate it.
    /// Where paving reads the brighter ([`Self::paving_brighter`]), the one
    /// under which a block is planted and the one over which it is paving.
    pub intensity_defaults: (f32, f32),
    pub classes: LaserClasses,
    /// Whether the laser's own sign of plants (vegetation classes, or pulses
    /// that pass through) counts beside the photo: where the source classes
    /// vegetation, or its photo is flown leaf-off so bare crowns do not read
    /// green. Bavaria's rules were tuned on the photo alone.
    pub laser_plants: bool,
    /// Whether a pool needs cells the laser returned nothing from, rather than
    /// fewer returns than around: where scans are as sparse as two to five
    /// points a square metre a cell's few returns are chance as often as
    /// water, and every bluish shadow on a street would be a pool.
    pub strict_pools: bool,
    /// Whether flat paving returns a brighter ground intensity than lawn, the
    /// other way round from the rest: Latvia's scans, whose lawn returns are
    /// mostly low plants (see [`LaserClasses::vegetation`]) and the ground
    /// returns left darker on lawn. The intensity is then read turned over
    /// (see [`super::classify::Inputs::i_gnd`]), and paving is kept near
    /// buildings only, since bare and trodden ground reads as bright there.
    pub paving_brighter: bool,
    pub attribution: &'static str,
}

pub static SOURCES: &[&Source] = &[
    &super::bavaria::BAVARIA,
    &super::france::FRANCE,
    &super::netherlands::NETHERLANDS,
    &super::switzerland::SWITZERLAND,
    &super::nrw::NRW,
    &super::estonia::ESTONIA,
    &super::luxembourg::LUXEMBOURG,
    &super::usa::USA,
    &super::canada::CANADA,
    &super::latvia::LATVIA,
];

impl Source {
    /// Whether (lat, lon) lies inside the source's outline (even-odd rule over
    /// all its rings).
    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        let mut inside = false;
        for pts in self.outline {
            let mut j = pts.len() - 1;
            for i in 0..pts.len() {
                let (yi, xi) = pts[i];
                let (yj, xj) = pts[j];
                if (yi > lat) != (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi {
                    inside = !inside;
                }
                j = i;
            }
        }
        inside
    }

    /// How far (lat, lon) lies from the nearest edge of the outline, in metres
    /// near enough.
    pub fn depth(&self, lat: f64, lon: f64) -> f64 {
        let ky = 110_574.0;
        let kx = 111_320.0 * lat.to_radians().cos();
        let mut best = f64::MAX;
        for pts in self.outline {
            for w in pts.windows(2) {
                let (ax, ay) = ((w[0].1 - lon) * kx, (w[0].0 - lat) * ky);
                let (bx, by) = ((w[1].1 - lon) * kx, (w[1].0 - lat) * ky);
                let (dx, dy) = (bx - ax, by - ay);
                let len2 = dx * dx + dy * dy;
                let t = if len2 > 0.0 {
                    (-(ax * dx + ay * dy) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                best = best.min((ax + t * dx).hypot(ay + t * dy));
            }
        }
        best
    }

    /// Whether any part of the bbox is inside the outline, judged on a lattice of
    /// points about 250 m apart, which the two-kilometre margin of the outlines
    /// makes safe.
    pub fn covers(&self, bbox: &LLBBox) -> bool {
        let (s, w) = (bbox.min().lat(), bbox.min().lng());
        let (n, e) = (bbox.max().lat(), bbox.max().lng());
        let steps_lat = (((n - s) / 0.00225).ceil() as usize).clamp(1, 400);
        let steps_lon = (((e - w) / 0.0033).ceil() as usize).clamp(1, 400);
        (0..=steps_lat).any(|i| {
            let lat = s + (n - s) * i as f64 / steps_lat as f64;
            (0..=steps_lon).any(|j| self.contains(lat, w + (e - w) * j as f64 / steps_lon as f64))
        })
    }
}

/// The source for a bbox: the one whose outline holds its centre, else the
/// first whose outline it reaches. Where outlines overlap along a border, the
/// one the centre lies deepest inside: every outline reaches the same two
/// kilometres past its border, so that is the side the centre is on.
pub fn for_bbox(bbox: &LLBBox) -> Option<&'static Source> {
    let lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
    let lon = (bbox.min().lng() + bbox.max().lng()) / 2.0;
    let mut holding = SOURCES.iter().copied().filter(|s| s.contains(lat, lon));
    let first = holding.next();
    match holding.next() {
        None => first,
        Some(second) => [first?, second]
            .into_iter()
            .chain(holding)
            .max_by(|a, b| a.depth(lat, lon).total_cmp(&b.depth(lat, lon))),
    }
    .or_else(|| SOURCES.iter().copied().find(|s| s.covers(bbox)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn which(lat: f64, lon: f64) -> Option<&'static str> {
        SOURCES.iter().find(|s| s.contains(lat, lon)).map(|s| s.id)
    }

    #[test]
    fn outlines_hold_their_towns_and_not_their_neighbours() {
        assert_eq!(which(48.30, 11.90), Some("bavaria")); // Erding
        assert_eq!(which(48.14, 11.57), Some("bavaria")); // Munich
        assert_eq!(which(49.45, 11.08), Some("bavaria")); // Nuremberg
        assert_eq!(which(45.76, 4.84), Some("france")); // Lyon
        assert_eq!(which(47.22, -1.55), Some("france")); // Nantes
        assert_eq!(which(42.70, 9.45), Some("france")); // Bastia, Corsica
        assert_eq!(which(52.16, 5.39), Some("netherlands")); // Amersfoort
        assert_eq!(which(53.22, 6.57), Some("netherlands")); // Groningen
        assert_eq!(which(50.85, 5.69), Some("netherlands")); // Maastricht
        assert_eq!(which(46.95, 7.45), Some("switzerland")); // Bern
        assert_eq!(which(46.01, 8.96), Some("switzerland")); // Lugano
        assert_eq!(which(51.96, 7.63), Some("nrw")); // Münster
        assert_eq!(which(50.78, 6.08), Some("nrw")); // Aachen
        assert_eq!(which(59.44, 24.75), Some("estonia")); // Tallinn
        assert_eq!(which(58.38, 26.72), Some("estonia")); // Tartu
        assert_eq!(which(49.61, 6.13), Some("luxembourg")); // Luxembourg City
        assert_eq!(which(56.95, 24.11), Some("latvia")); // Riga
        assert_eq!(which(55.87, 26.53), Some("latvia")); // Daugavpils
        assert_eq!(which(54.69, 25.28), None); // Vilnius
        assert_eq!(which(51.34, 12.37), None); // Leipzig
        assert_eq!(which(48.78, 9.18), None); // Stuttgart
        assert_eq!(which(48.21, 16.37), None); // Vienna
        assert_eq!(which(50.11, 8.68), None); // Frankfurt
        assert_eq!(which(50.85, 4.35), None); // Brussels
        assert_eq!(which(51.51, -0.13), None); // London
        assert_eq!(which(41.39, 2.17), None); // Barcelona
        assert_eq!(which(42.28, -83.74), Some("usa")); // Ann Arbor
        assert_eq!(which(47.61, -122.33), Some("usa")); // Seattle
        assert_eq!(which(25.77, -80.19), Some("usa")); // Miami
        assert_eq!(which(61.22, -149.9), Some("usa")); // Anchorage
        assert_eq!(which(21.31, -157.86), Some("usa")); // Honolulu
        assert_eq!(which(18.47, -66.1), Some("usa")); // San Juan
        assert_eq!(which(43.65, -79.38), Some("canada")); // Toronto
        assert_eq!(which(49.28, -123.12), Some("canada")); // Vancouver
        assert_eq!(which(45.42, -75.69), Some("canada")); // Ottawa
        assert_eq!(which(44.65, -63.57), Some("canada")); // Halifax
        assert_eq!(which(19.43, -99.13), None); // Mexico City
    }

    /// The source `for_bbox` picks for a small area around (lat, lon).
    fn picked(lat: f64, lon: f64) -> Option<&'static str> {
        let bbox = LLBBox::new(lat - 0.002, lon - 0.003, lat + 0.002, lon + 0.003).unwrap();
        for_bbox(&bbox).map(|s| s.id)
    }

    #[test]
    fn borders_go_to_the_side_the_area_is_on() {
        // Kerkrade and Herzogenrath, a street apart: both outlines hold each.
        assert_eq!(picked(50.866, 6.050), Some("netherlands"));
        assert_eq!(picked(50.870, 6.105), Some("nrw"));
        // Basel and Saint-Louis.
        assert_eq!(picked(47.560, 7.590), Some("switzerland"));
        assert_eq!(picked(47.590, 7.555), Some("france"));
        // Esch-sur-Alzette and Audun-le-Tiche.
        assert_eq!(picked(49.495, 5.985), Some("luxembourg"));
        assert_eq!(picked(49.470, 5.950), Some("france"));
        // Detroit and Windsor, Blaine and White Rock, the two Niagara Falls.
        assert_eq!(picked(42.331, -83.046), Some("usa"));
        assert_eq!(picked(42.300, -83.020), Some("canada"));
        assert_eq!(picked(48.990, -122.750), Some("usa"));
        assert_eq!(picked(49.002, -122.760), Some("canada"));
        assert_eq!(picked(43.095, -79.040), Some("usa"));
        assert_eq!(picked(43.090, -79.070), Some("canada"));
        // Valga and Valka.
        assert_eq!(picked(57.777, 26.045), Some("estonia"));
        assert_eq!(picked(57.772, 26.015), Some("latvia"));
        // Deep inside, nothing changes.
        assert_eq!(picked(48.14, 11.57), Some("bavaria"));
        assert_eq!(picked(50.94, 6.96), Some("nrw")); // Cologne
    }

    #[test]
    fn tiles_stay_inside_the_outline() {
        let fr = Sheet::at(&super::super::france::FRANCE, 2.35);
        // A rectangle around Paris: all four tiles are in.
        assert_eq!(
            fr.tiles((650_500.0, 6_862_500.0, 651_500.0, 6_863_500.0))
                .len(),
            4
        );
        // Far out in the Atlantic: none.
        assert!(fr
            .tiles((50_000.0, 6_700_000.0, 52_000.0, 6_702_000.0))
            .is_empty());
    }
}
