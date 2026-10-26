//! Summits from OSM. A 30 m DEM smears a needle peak into its cliffs, which put
//! Corcovado's summit, statue included, about 170 m too low. A `natural=peak`
//! with an `ele` is surveyed, so where the terrain falls short of it the summit
//! is lifted back up with a bump that fades into the DEM around it. The same
//! hand-off carries the tall landmark models, so the height fit leaves them room,
//! and the tall buildings, so it can tell whether the vanilla build height holds them.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;

use crate::coordinate_system::geographic::LLBBox;

/// A surveyed summit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeakHint {
    pub lat: f64,
    pub lon: f64,
    pub ele_m: f64,
}

/// A landmark model that will stand this tall on the terrain at its spot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TallModel {
    pub lat: f64,
    pub lon: f64,
    pub height_m: f64,
}

/// What the terrain needs to know from OSM.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerrainHints {
    pub peaks: Vec<PeakHint>,
    pub tall_models: Vec<TallModel>,
    /// Buildings and towers, for a run that extends the build height only where needed.
    pub tall_structures: Vec<TallModel>,
}

impl TerrainHints {
    /// The hints a run's terrain fit wants from its raw OSM data. Models and structures only
    /// count where the mode builds them.
    pub fn from_osm(data: &crate::osm_parser::OsmData, args: &crate::args::Args) -> Self {
        let objects = !args.skip_objects();
        let buildings = objects && args.mode != crate::args::GenerationMode::Blueprint;
        Self {
            peaks: peaks_in(data),
            tall_models: if args.use_3d && objects {
                crate::models_3d::wikidata::tall_models_in(data)
            } else {
                Vec::new()
            },
            tall_structures: if buildings && crate::ground::auto_height(args) {
                tall_structures_in(data, args.scale)
            } else {
                Vec::new()
            },
        }
    }
}

/// Hints handed to the terrain thread, which runs alongside the OSM fetch.
pub struct PeakSource {
    rx: Mutex<Option<Receiver<TerrainHints>>>,
}

impl PeakSource {
    /// The sender goes to whoever fetches OSM. Dropping it unsent means no hints.
    pub fn channel() -> (Sender<TerrainHints>, PeakSource) {
        let (tx, rx) = channel();
        (
            tx,
            PeakSource {
                rx: Mutex::new(Some(rx)),
            },
        )
    }

    pub fn none() -> PeakSource {
        PeakSource {
            rx: Mutex::new(None),
        }
    }

    /// Waits for the OSM side, once.
    pub(crate) fn take(&self) -> TerrainHints {
        let rx = self.rx.lock().ok().and_then(|mut r| r.take());
        rx.and_then(|r| r.recv().ok()).unwrap_or_default()
    }
}

/// Surveyed summits among raw OSM tags.
pub fn peak_from_tags(lat: f64, lon: f64, tags: &HashMap<String, String>) -> Option<PeakHint> {
    if !matches!(
        tags.get("natural").map(String::as_str),
        Some("peak") | Some("volcano")
    ) {
        return None;
    }
    let raw = tags.get("ele")?.trim();
    // Metres only; a value in feet would raise the summit three times too high.
    let ele_m = raw
        .strip_suffix('m')
        .unwrap_or(raw)
        .trim()
        .parse::<f64>()
        .ok()?;
    (ele_m.is_finite() && lat.is_finite() && lon.is_finite()).then_some(PeakHint {
        lat,
        lon,
        ele_m,
    })
}

/// Surveyed summits among the nodes of a raw OSM fetch.
pub fn peaks_in(data: &crate::osm_parser::OsmData) -> Vec<PeakHint> {
    data.elements()
        .iter()
        .filter(|e| e.r#type == "node")
        .filter_map(|e| peak_from_tags(e.lat?, e.lon?, e.tags.as_ref()?))
        .collect()
}

/// Mapped height of a building, building part or tower way, or of a mast, antenna or
/// chimney, which may also be a node. The flag says whether a node counts.
fn structure_height_m(tags: &HashMap<String, String>) -> Option<(f64, bool)> {
    let man_made = tags.get("man_made").map(String::as_str);
    if matches!(man_made, Some("mast" | "antenna" | "chimney")) {
        let m = crate::mapillary::geometry::parse_length_m(tags.get("height")?)?;
        return (m > 0.0 && m < 1000.0).then_some((m, true));
    }
    if tags.contains_key("building")
        || tags.contains_key("building:part")
        || man_made == Some("tower")
    {
        return crate::element_processing::buildings::mapped_height_m(tags).map(|m| (m, false));
    }
    None
}

/// Structures whose mapped height could take them past the vanilla ceiling at `scale`. A way
/// stands at its first node, which is close enough to read the terrain under it.
pub fn tall_structures_in(data: &crate::osm_parser::OsmData, scale: f64) -> Vec<TallModel> {
    // Anything shorter stays under the ceiling wherever the terrain itself fits.
    let min_m = f64::from(super::postprocess::TERRAIN_HEIGHT_BUFFER - 2) / scale.max(1e-9);
    let mut found = Vec::new();
    let mut ways: Vec<(u64, f64)> = Vec::new();
    for e in data.elements() {
        let Some((height_m, node_ok)) = e.tags.as_ref().and_then(structure_height_m) else {
            continue;
        };
        if height_m <= min_m {
            continue;
        }
        match (e.r#type.as_str(), e.lat, e.lon, e.nodes.as_deref()) {
            ("node", Some(lat), Some(lon), _) if node_ok => {
                found.push(TallModel { lat, lon, height_m })
            }
            ("way", _, _, Some([first, ..])) => ways.push((*first, height_m)),
            _ => {}
        }
    }
    if ways.is_empty() {
        return found;
    }
    let wanted: HashSet<u64> = ways.iter().map(|&(id, _)| id).collect();
    let coords: HashMap<u64, (f64, f64)> = data
        .elements()
        .iter()
        .filter(|e| e.r#type == "node" && wanted.contains(&e.id))
        .filter_map(|e| Some((e.id, (e.lat?, e.lon?))))
        .collect();
    found.extend(ways.into_iter().filter_map(|(id, height_m)| {
        let &(lat, lon) = coords.get(&id)?;
        Some(TallModel { lat, lon, height_m })
    }));
    found
}

/// Spread of the bump. Fitted on Corcovado, whose OSM building `ele`s fall from
/// the summit by about 20 m at 20 m out and 45 m at 50 m.
const SIGMA_M: f64 = 60.0;
/// The lift never takes a cell above a cone this steep down from the summit, or
/// a DEM rising to one side would carry the top off the surveyed point. 45
/// degrees matches the terraces below Corcovado's summit.
const CONE_SLOPE: f64 = 1.0;
/// Radius searched for the DEM's own summit, so a peak node a few metres off
/// the DEM's highest cell is not lifted on top of it.
const SEARCH_M: f64 = 15.0;
/// Shortfalls below this are DEM noise.
const MIN_DEFICIT_M: f64 = 3.0;
/// Shortfalls beyond this are a bad tag, not a smeared summit.
const MAX_DEFICIT_M: f64 = 400.0;
/// Radius of the relief a shortfall is checked against. A smeared needle stands
/// in big relief; a hill whose `ele` is in feet does not.
const RELIEF_M: f64 = 500.0;
/// Samples across that radius, so the check stays cheap on a fine grid.
const RELIEF_SAMPLES: f64 = 50.0;

/// Lift each summit the DEM keeps too low. Rows run north to south and columns
/// west to east across `bbox`, end samples on its edges, as the providers sample.
pub(crate) fn raise_peaks(
    grid: &mut [Vec<f64>],
    bbox: &LLBBox,
    peaks: &[PeakHint],
    m_per_cell: f64,
) {
    let h = grid.len();
    let w = grid.first().map_or(0, Vec::len);
    if w < 2 || h < 2 || m_per_cell <= 0.0 {
        return;
    }
    let (min_lat, max_lat) = (bbox.min().lat(), bbox.max().lat());
    let (min_lon, max_lon) = (bbox.min().lng(), bbox.max().lng());
    for p in peaks {
        if p.lat < min_lat || p.lat > max_lat || p.lon < min_lon || p.lon > max_lon {
            continue;
        }
        let gx = (p.lon - min_lon) / (max_lon - min_lon) * (w - 1) as f64;
        let gy = (max_lat - p.lat) / (max_lat - min_lat) * (h - 1) as f64;
        let (cx, cy) = (gx.round() as usize, gy.round() as usize);
        let Some(at_node) = grid.get(cy).and_then(|r| r.get(cx)).copied() else {
            continue;
        };
        let search = (SEARCH_M / m_per_cell).ceil() as isize;
        let dem_top = cells_within(cx, cy, search, w, h)
            .map(|(x, y)| grid[y][x])
            .filter(|v| v.is_finite())
            .fold(f64::NEG_INFINITY, f64::max);
        let shortfall = p.ele_m - dem_top;
        if !(MIN_DEFICIT_M..=MAX_DEFICIT_M).contains(&shortfall)
            || !at_node.is_finite()
            || shortfall > local_relief(grid, cx, cy, m_per_cell)
        {
            continue;
        }
        // Relative to the node's own cell, so the summit lands on the tagged height.
        let lift = p.ele_m - at_node;
        let reach = (3.0 * SIGMA_M / m_per_cell).ceil() as isize;
        for (x, y) in cells_within(cx, cy, reach, w, h) {
            let d = ((x as f64 - gx).powi(2) + (y as f64 - gy).powi(2)).sqrt() * m_per_cell;
            let v = &mut grid[y][x];
            if v.is_finite() {
                let room = (p.ele_m - CONE_SLOPE * d - *v).max(0.0);
                *v += (lift * (-(d / SIGMA_M).powi(2)).exp()).min(room);
            }
        }
    }
}

/// Highest minus lowest DEM height within `RELIEF_M` of a cell, sampled sparsely.
fn local_relief(grid: &[Vec<f64>], cx: usize, cy: usize, m_per_cell: f64) -> f64 {
    let (h, w) = (grid.len(), grid[0].len());
    let r = (RELIEF_M / m_per_cell).ceil() as isize;
    let step = ((r as f64 / RELIEF_SAMPLES).ceil() as usize).max(1);
    let (x0, x1) = (cx.saturating_sub(r as usize), (cx + r as usize).min(w - 1));
    let (y0, y1) = (cy.saturating_sub(r as usize), (cy + r as usize).min(h - 1));
    let (lo, hi) = (y0..=y1)
        .step_by(step)
        .flat_map(|y| (x0..=x1).step_by(step).map(move |x| (x, y)))
        .map(|(x, y)| grid[y][x])
        .filter(|v| v.is_finite())
        .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
    (hi - lo).max(0.0)
}

/// Grid cell and height of each tall model inside `bbox`, sampled like `raise_peaks`.
pub(crate) fn model_cells(
    grid: &[Vec<f64>],
    bbox: &LLBBox,
    models: &[TallModel],
) -> Vec<(f64, f64)> {
    let h = grid.len();
    let w = grid.first().map_or(0, Vec::len);
    if w < 2 || h < 2 {
        return Vec::new();
    }
    let (min_lat, max_lat) = (bbox.min().lat(), bbox.max().lat());
    let (min_lon, max_lon) = (bbox.min().lng(), bbox.max().lng());
    models
        .iter()
        .filter(|m| (min_lat..=max_lat).contains(&m.lat) && (min_lon..=max_lon).contains(&m.lon))
        .filter_map(|m| {
            let x = ((m.lon - min_lon) / (max_lon - min_lon) * (w - 1) as f64).round() as usize;
            let y = ((max_lat - m.lat) / (max_lat - min_lat) * (h - 1) as f64).round() as usize;
            let ground = *grid.get(y)?.get(x)?;
            ground.is_finite().then_some((ground, m.height_m))
        })
        .collect()
}

fn cells_within(
    cx: usize,
    cy: usize,
    r: isize,
    w: usize,
    h: usize,
) -> impl Iterator<Item = (usize, usize)> {
    let (x0, x1) = (
        (cx as isize - r).max(0) as usize,
        (cx as isize + r).min(w as isize - 1) as usize,
    );
    let (y0, y1) = (
        (cy as isize - r).max(0) as usize,
        (cy as isize + r).min(h as isize - 1) as usize,
    );
    (y0..=y1).flat_map(move |y| (x0..=x1).map(move |x| (x, y)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox() -> LLBBox {
        LLBBox::new(-22.9545, -43.2140, -22.9495, -43.2070).unwrap()
    }

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn tall_structures_are_buildings_towers_and_masts_with_a_height() {
        let data: crate::osm_parser::OsmData = serde_json::from_str(
            r#"{"elements": [
                {"type": "node", "id": 1, "lat": 40.0, "lon": -74.0},
                {"type": "node", "id": 2, "lat": 40.1, "lon": -74.1},
                {"type": "way", "id": 10, "nodes": [1, 2], "tags": {"building": "yes", "height": "541"}},
                {"type": "way", "id": 11, "nodes": [2, 1], "tags": {"building": "office", "building:levels": "100"}},
                {"type": "way", "id": 12, "nodes": [1, 2], "tags": {"man_made": "tower", "height": "368"}},
                {"type": "node", "id": 3, "lat": 40.2, "lon": -74.2, "tags": {"man_made": "mast", "height": "300 m"}},
                {"type": "way", "id": 13, "nodes": [1, 2], "tags": {"building": "house", "height": "8"}},
                {"type": "way", "id": 14, "nodes": [1, 2], "tags": {"building": "yes", "height": "1640"}},
                {"type": "way", "id": 15, "nodes": [1, 2], "tags": {"highway": "primary", "height": "400"}},
                {"type": "node", "id": 4, "lat": 40.3, "lon": -74.3, "tags": {"building": "yes", "height": "400"}},
                {"type": "way", "id": 16, "nodes": [99], "tags": {"building": "yes", "height": "400"}}
            ]}"#,
        )
        .unwrap();
        let found = tall_structures_in(&data, 1.0);
        let at = |lat: f64, lon: f64, height_m: f64| TallModel { lat, lon, height_m };
        // 100 office storeys at 4 blocks each, plus the taller ground floor.
        assert_eq!(
            found,
            vec![
                at(40.2, -74.2, 300.0),
                at(40.0, -74.0, 541.0),
                at(40.1, -74.1, 402.0),
                at(40.0, -74.0, 368.0),
            ]
        );
        // At scale 3 an 8 m house rises 24 blocks, enough to matter on the highest ground.
        assert_eq!(tall_structures_in(&data, 3.0).len(), 5);

        // Only a run that builds them and still picks its build height wants them.
        let hints = |extra: &[&str]| {
            use clap::Parser;
            let mut cmd = vec!["arnis", "--output-dir", ".", "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            TerrainHints::from_osm(&data, &crate::args::Args::parse_from(cmd)).tall_structures
        };
        assert_eq!(hints(&[]).len(), 4);
        assert_eq!(hints(&["--mode", "geo-only"]).len(), 4);
        for none in [
            &["--mode", "terrain-only"][..],
            &["--mode", "blueprint"][..],
            &["--vanilla-height"][..],
            &["--disable-height-limit"][..],
        ] {
            assert!(hints(none).is_empty(), "{none:?}");
        }
    }

    #[test]
    fn peak_tags_need_a_metric_elevation() {
        let ok = peak_from_tags(1.0, 2.0, &tags(&[("natural", "peak"), ("ele", "710")]));
        assert_eq!(ok.map(|p| p.ele_m), Some(710.0));
        let with_unit = peak_from_tags(1.0, 2.0, &tags(&[("natural", "peak"), ("ele", "710 m")]));
        assert_eq!(with_unit.map(|p| p.ele_m), Some(710.0));
        assert!(
            peak_from_tags(1.0, 2.0, &tags(&[("natural", "peak"), ("ele", "2330 ft")])).is_none()
        );
        assert!(
            peak_from_tags(1.0, 2.0, &tags(&[("natural", "saddle"), ("ele", "500")])).is_none()
        );
        assert!(peak_from_tags(1.0, 2.0, &tags(&[("natural", "peak")])).is_none());
    }

    #[test]
    fn a_smeared_summit_is_lifted_to_its_surveyed_height() {
        // A cone peaking at 530 m in the middle of a 101 x 101 grid of 5 m cells.
        let n = 101;
        let mut grid: Vec<Vec<f64>> = (0..n)
            .map(|y| {
                (0..n)
                    .map(|x| {
                        let d =
                            (((x as f64 - 50.0).powi(2) + (y as f64 - 50.0).powi(2)).sqrt()) * 5.0;
                        530.0 - d * 0.8
                    })
                    .collect()
            })
            .collect();
        let b = bbox();
        let lat = (b.min().lat() + b.max().lat()) / 2.0;
        let lon = (b.min().lng() + b.max().lng()) / 2.0;
        let far_before = grid[0][0];
        raise_peaks(
            &mut grid,
            &b,
            &[PeakHint {
                lat,
                lon,
                ele_m: 710.0,
            }],
            5.0,
        );
        assert!(
            (grid[50][50] - 710.0).abs() < 1.0,
            "summit {}",
            grid[50][50]
        );
        // Still the highest point, and the bump fades out instead of making a plateau.
        let top = grid.iter().flatten().fold(f64::MIN, |a, &v| a.max(v));
        assert!((top - grid[50][50]).abs() < 1e-6);
        assert!(grid[50][60] < grid[50][50] - 1.0);
        assert!((grid[0][0] - far_before).abs() < 0.5, "corner moved");
    }

    #[test]
    fn a_dem_rising_to_one_side_does_not_carry_the_summit_off() {
        // Ramp up to the west and a cliff to the east, the node 180 m short.
        let n = 81;
        let mut grid: Vec<Vec<f64>> = (0..n)
            .map(|_| {
                (0..n)
                    .map(|x| {
                        let x = x as f64;
                        if x <= 40.0 {
                            570.0 - x
                        } else {
                            530.0 - (x - 40.0) * 8.0
                        }
                    })
                    .collect()
            })
            .collect();
        let b = bbox();
        let lat = (b.min().lat() + b.max().lat()) / 2.0;
        let lon = (b.min().lng() + b.max().lng()) / 2.0;
        raise_peaks(
            &mut grid,
            &b,
            &[PeakHint {
                lat,
                lon,
                ele_m: 710.0,
            }],
            5.0,
        );
        let (mut best, mut at) = (f64::MIN, (0, 0));
        for (y, row) in grid.iter().enumerate() {
            for (x, &v) in row.iter().enumerate() {
                if v > best {
                    best = v;
                    at = (x, y);
                }
            }
        }
        assert_eq!(at, (40, 40), "top drifted to {at:?} at {best}");
    }

    #[test]
    fn a_summit_the_dem_already_has_is_left_alone() {
        let mut grid = vec![vec![700.0; 21]; 21];
        grid[10][11] = 709.0;
        let before = grid.clone();
        let b = bbox();
        let lat = (b.min().lat() + b.max().lat()) / 2.0;
        let lon = (b.min().lng() + b.max().lng()) / 2.0;
        // The DEM's top is one cell off the node but within the search radius.
        raise_peaks(
            &mut grid,
            &b,
            &[PeakHint {
                lat,
                lon,
                ele_m: 710.0,
            }],
            5.0,
        );
        assert_eq!(grid, before);
    }

    #[test]
    fn a_wild_elevation_is_ignored() {
        let mut grid = vec![vec![100.0; 11]; 11];
        let before = grid.clone();
        let b = bbox();
        let lat = (b.min().lat() + b.max().lat()) / 2.0;
        let lon = (b.min().lng() + b.max().lng()) / 2.0;
        raise_peaks(
            &mut grid,
            &b,
            &[PeakHint {
                lat,
                lon,
                ele_m: 2330.0,
            }],
            5.0,
        );
        assert_eq!(grid, before);
    }

    #[test]
    fn a_hill_tagged_in_feet_is_not_raised() {
        // A gentle 150 m hill on a 100 m plain, its `ele` written in feet.
        let n = 121;
        let mut grid: Vec<Vec<f64>> = (0..n)
            .map(|y| {
                (0..n)
                    .map(|x| {
                        let d = ((x as f64 - 60.0).powi(2) + (y as f64 - 60.0).powi(2)).sqrt();
                        100.0 + (50.0 - d * 0.5).max(0.0)
                    })
                    .collect()
            })
            .collect();
        let before = grid.clone();
        let b = bbox();
        let lat = (b.min().lat() + b.max().lat()) / 2.0;
        let lon = (b.min().lng() + b.max().lng()) / 2.0;
        raise_peaks(
            &mut grid,
            &b,
            &[PeakHint {
                lat,
                lon,
                ele_m: 492.0,
            }],
            5.0,
        );
        assert_eq!(grid, before);
    }

    #[test]
    fn an_unsent_channel_yields_no_peaks() {
        let (tx, source) = PeakSource::channel();
        drop(tx);
        assert_eq!(source.take(), TerrainHints::default());
        assert_eq!(PeakSource::none().take(), TerrainHints::default());
    }
}
