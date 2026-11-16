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
    /// tell vegetation apart; the pulse's returns then stand in for it.
    pub vegetation: &'static [u8],
}

/// A rectangle in the source's grid metres: west, south, east, north.
pub type Rect = (f64, f64, f64, f64);

/// Tiles (south-west corners in km) and the URL of each.
pub type TileUrls = Vec<((i32, i32), String)>;

/// Where a source's laser points come from.
pub enum Laser {
    /// Plain LAZ tiles, read whole.
    Whole {
        url: fn(i32, i32) -> String,
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
    },
}

/// How a COPC source's tile URLs are found.
pub enum Locate {
    /// Candidate URLs of a tile, best first; a missing one answers 403 or 404.
    Urls(fn(i32, i32) -> Vec<String>),
    /// One lookup over a rectangle: the URL of every tile the source has there.
    Lookup(fn(&reqwest::blocking::Client, Rect) -> Result<TileUrls, String>),
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
}

pub struct Source {
    /// Cache folder and log name.
    pub id: &'static str,
    pub name: &'static str,
    /// The grid of the source's 1 km tiles.
    pub crs: Crs,
    /// Where the source has data, as rings of (lat, lon). Requests never leave it.
    pub outline: &'static [&'static [(f64, f64)]],
    pub laser: Laser,
    pub photo: Photo,
    /// Ground-return intensity under which a block is paving and over which it is
    /// planted, when neither the photo nor the laser alone can calibrate it.
    pub intensity_defaults: (f32, f32),
    pub classes: LaserClasses,
    /// Whether the laser's own sign of plants (vegetation classes, or pulses
    /// that pass through) counts beside the photo: where the source classes
    /// vegetation, or its photo is flown leaf-off so bare crowns do not read
    /// green. Bavaria's rules were tuned on the photo alone.
    pub laser_plants: bool,
    pub attribution: &'static str,
}

pub static SOURCES: &[&Source] = &[
    &super::bavaria::BAVARIA,
    &super::france::FRANCE,
    &super::netherlands::NETHERLANDS,
    &super::switzerland::SWITZERLAND,
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

    /// The source's 1 km tiles (south-west corners in km) a grid rectangle
    /// touches, only those whose centre or a corner is inside the outline.
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
                    self.contains(lat, lon)
                }) {
                    out.push((te, tn));
                }
            }
        }
        out
    }
}

/// The source for a bbox: the one whose outline holds its centre, else the
/// first whose outline it reaches.
pub fn for_bbox(bbox: &LLBBox) -> Option<&'static Source> {
    let lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
    let lon = (bbox.min().lng() + bbox.max().lng()) / 2.0;
    SOURCES
        .iter()
        .copied()
        .find(|s| s.contains(lat, lon))
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
        assert_eq!(which(48.78, 9.18), None); // Stuttgart
        assert_eq!(which(48.21, 16.37), None); // Vienna
        assert_eq!(which(50.11, 8.68), None); // Frankfurt
        assert_eq!(which(50.85, 4.35), None); // Brussels
        assert_eq!(which(51.51, -0.13), None); // London
        assert_eq!(which(41.39, 2.17), None); // Barcelona
    }

    #[test]
    fn tiles_stay_inside_the_outline() {
        let fr = &super::super::france::FRANCE;
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
