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
    /// A second message with Overture's tall structures follows.
    pub overture_follows: bool,
    /// Points along the ways bored as tunnels, so a fit with water over one can leave it
    /// room under the bed.
    pub bores: Vec<(f64, f64)>,
    /// The roads, railways and waterways graded into the ground, as `(lat, lon)` lines, so
    /// the detail added to steep, coarsely sampled ground stays off them.
    pub graded: Vec<Vec<(f64, f64)>>,
}

impl TerrainHints {
    /// The hints a run's terrain fit wants from its raw OSM data. Models and structures only
    /// count where the mode builds them.
    pub fn from_osm(data: &crate::osm_parser::OsmData, args: &crate::args::Args) -> Self {
        // The graded ways only matter where the steep-slope detail can run, on Mapterhorn
        // terrain. Terrain-only builds no roads but keeps the waterways.
        let graded = (args.terrain() && !args.aws_only_elevation).then_some(!args.skip_objects());
        let (bores, graded) = terrain_ways(data, graded);
        Self {
            peaks: peaks_in(data),
            tall_models: if args.use_3d && !args.skip_objects() {
                crate::models_3d::wikidata::tall_models_in(data)
            } else {
                Vec::new()
            },
            tall_structures: if wants_structures(args) {
                tall_structures_in(data, args.scale, None)
            } else {
                Vec::new()
            },
            overture_follows: Self::wants_overture(args),
            bores,
            graded,
        }
    }

    /// The second message, once Overture is in: its own footprints and the OSM buildings it
    /// gives a height.
    pub fn from_overture(
        data: &crate::osm_parser::OsmData,
        args: &crate::args::Args,
        overture: &crate::overture::OvertureData,
    ) -> Self {
        let mut tall_structures = if overture.hints.len() == 0 {
            Vec::new()
        } else {
            tall_structures_in(data, args.scale, Some(&overture.hints))
        };
        tall_structures.extend_from_slice(&overture.tall_structures);
        Self {
            tall_structures,
            ..Self::default()
        }
    }

    /// Whether `from_overture` follows `from_osm`: Overture's building heights count as much
    /// as OSM's.
    pub fn wants_overture(args: &crate::args::Args) -> bool {
        args.overture && wants_structures(args)
    }
}

/// A run that picks its build height from the area and builds full buildings.
fn wants_structures(args: &crate::args::Args) -> bool {
    !args.skip_objects()
        && args.mode != crate::args::GenerationMode::Blueprint
        && crate::ground::auto_height(args)
}

/// Whether a structure this tall could reach past the vanilla ceiling at `scale`. Anything
/// shorter stays under it wherever the terrain itself fits.
pub(crate) fn may_pass_vanilla(height_m: f64, scale: f64) -> bool {
    height_m * scale > f64::from(super::postprocess::TERRAIN_HEIGHT_BUFFER - 2)
}

/// Spacing in metres of the points `terrain_ways` lays along a tunnel: finer than the land
/// cover, so a bore under a narrow river still puts a point on its water.
const BORE_STEP_M: f64 = 2.0;

/// Whether a way with these tags is bored as a tunnel: a road or track tagged as one, or a
/// subway not tagged otherwise. Errs wide, as a hint only leaves room.
fn is_bored(tags: &HashMap<String, String>) -> bool {
    let tunnel = tags.get("tunnel").map(String::as_str);
    let railway = tags.get("railway").map(String::as_str);
    (tunnel == Some("yes") && (tags.contains_key("highway") || railway.is_some()))
        || (railway == Some("subway") && tunnel != Some("no"))
}

/// Points along the bored ways, and the `(lat, lon)` lines of the graded ones.
type TerrainWays = (Vec<(f64, f64)>, Vec<Vec<(f64, f64)>>);

/// The ways of a raw OSM fetch the terrain needs, from one pass over it: points every
/// `BORE_STEP_M` along those bored as tunnels, and the `(lat, lon)` lines of those graded
/// into the ground when `graded` is set, with roads and railways if it is true.
fn terrain_ways(data: &crate::osm_parser::OsmData, graded: Option<bool>) -> TerrainWays {
    let [bored, graded] = way_lines(data, |tags| {
        if is_bored(tags) {
            Some(0)
        } else {
            graded.filter(|&roads| is_graded(tags, roads)).map(|_| 1)
        }
    });
    let mut points = Vec::new();
    for line in bored {
        points.extend(line.first());
        for pair in line.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let (ew, ns) = extent_m([a, b].into_iter());
            let steps = ((ew.hypot(ns) / BORE_STEP_M).ceil() as usize).max(1);
            points.extend((1..=steps).map(|k| {
                let t = k as f64 / steps as f64;
                (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
            }));
        }
    }
    (points, graded)
}

/// Whether a way with these tags is graded into the ground: a waterway, or with `roads` a
/// road or railway, on the ground rather than through or over it. Trails keep the ground's
/// own lie.
fn is_graded(tags: &HashMap<String, String>, roads: bool) -> bool {
    if ["tunnel", "bridge"]
        .iter()
        .any(|key| tags.get(*key).is_some_and(|v| v != "no"))
    {
        return false;
    }
    let is =
        |key: &str, values: &[&str]| tags.get(key).is_some_and(|v| values.contains(&v.as_str()));
    is(
        "waterway",
        &[
            "river",
            "stream",
            "canal",
            "ditch",
            "drain",
            "brook",
            "tidal_channel",
        ],
    ) || (roads
        && (tags.get("highway").is_some_and(|v| {
            !matches!(
                v.as_str(),
                "path"
                    | "footway"
                    | "bridleway"
                    | "steps"
                    | "via_ferrata"
                    | "corridor"
                    | "elevator"
                    | "platform"
                    | "proposed"
                    | "abandoned"
                    | "razed"
                    | "disused"
            )
        }) || is(
            "railway",
            &[
                "rail",
                "light_rail",
                "narrow_gauge",
                "tram",
                "funicular",
                "monorail",
                "preserved",
                "miniature",
            ],
        )))
}

/// The `(lat, lon)` lines of the ways `kind` sorts into one of `N` kinds, by kind, skipping
/// nodes not in `data`: one pass over the ways and one node index for all kinds.
fn way_lines<const N: usize>(
    data: &crate::osm_parser::OsmData,
    kind: impl Fn(&HashMap<String, String>) -> Option<usize>,
) -> [Vec<Vec<(f64, f64)>>; N] {
    let ways: Vec<(usize, &[u64])> = data
        .elements()
        .iter()
        .filter(|e| e.r#type == "way")
        .filter_map(|e| Some((kind(e.tags.as_ref()?)?, e.nodes.as_deref()?)))
        .collect();
    let mut lines: [Vec<Vec<(f64, f64)>>; N] = std::array::from_fn(|_| Vec::new());
    if ways.is_empty() {
        return lines;
    }
    let wanted: HashSet<u64> = ways.iter().flat_map(|(_, w)| w.iter().copied()).collect();
    let at: HashMap<u64, (f64, f64)> = data
        .elements()
        .iter()
        .filter(|e| e.r#type == "node" && wanted.contains(&e.id))
        .filter_map(|e| Some((e.id, (e.lat?, e.lon?))))
        .collect();
    for (k, way) in ways {
        let line: Vec<(f64, f64)> = way.iter().filter_map(|id| at.get(id).copied()).collect();
        if !line.is_empty() {
            lines[k].push(line);
        }
    }
    lines
}

/// The land-cover cells (column, row) of `bores` that lie on water, over `bbox`, in order.
pub(crate) fn bore_water_cells(
    land_cover: &crate::land_cover::LandCoverData,
    bbox: &LLBBox,
    bores: &[(f64, f64)],
) -> Vec<(usize, usize)> {
    let (w, h) = (land_cover.width, land_cover.height);
    if w < 2 || h < 2 {
        return Vec::new();
    }
    let (min_lat, max_lat) = (bbox.min().lat(), bbox.max().lat());
    let (min_lon, max_lon) = (bbox.min().lng(), bbox.max().lng());
    let mut cells: Vec<(usize, usize)> = bores
        .iter()
        .filter(|&&(lat, lon)| inside(bbox, lat, lon))
        .map(|&(lat, lon)| {
            let x = ((lon - min_lon) / (max_lon - min_lon) * (w - 1) as f64).round() as usize;
            let y = ((max_lat - lat) / (max_lat - min_lat) * (h - 1) as f64).round() as usize;
            (x, y)
        })
        .filter(|&(x, y)| {
            land_cover.grid.get(y).and_then(|row| row.get(x)) == Some(&crate::land_cover::LC_WATER)
        })
        .collect();
    cells.sort_unstable();
    cells.dedup();
    cells
}

/// Whether a point lies in `bbox`, edges included.
pub(crate) fn inside(bbox: &LLBBox, lat: f64, lon: f64) -> bool {
    crate::coordinate_system::geographic::LLPoint::new(lat, lon).is_ok_and(|p| bbox.contains(&p))
}

/// East-west and north-south extent in metres of `(lat, lon)` points.
pub(crate) fn extent_m(points: impl Iterator<Item = (f64, f64)>) -> (f64, f64) {
    let (mut lat0, mut lat1, mut lon0, mut lon1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for (lat, lon) in points {
        lat0 = lat0.min(lat);
        lat1 = lat1.max(lat);
        lon0 = lon0.min(lon);
        lon1 = lon1.max(lon);
    }
    if lat0 > lat1 {
        return (0.0, 0.0);
    }
    const M_PER_DEG: f64 = 111_320.0;
    let mid = ((lat0 + lat1) / 2.0).to_radians();
    (
        (lon1 - lon0) * M_PER_DEG * mid.cos(),
        (lat1 - lat0) * M_PER_DEG,
    )
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
        let Ok(mut slot) = self.rx.lock() else {
            return TerrainHints::default();
        };
        let Some(rx) = slot.take() else {
            return TerrainHints::default();
        };
        let hints = rx.recv().unwrap_or_default();
        if hints.overture_follows {
            *slot = Some(rx);
        }
        hints
    }

    /// Overture's tall structures, after `take` said they follow. Waits for them, once.
    pub(crate) fn take_overture(&self) -> Vec<TallModel> {
        let rx = self.rx.lock().ok().and_then(|mut r| r.take());
        rx.and_then(|r| r.recv().ok())
            .map(|h| h.tall_structures)
            .unwrap_or_default()
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

/// A building, building part or tower whose top the check reads from its tags.
fn is_building(tags: &HashMap<String, String>) -> bool {
    tags.contains_key("building")
        || tags.contains_key("building:part")
        || tags.get("man_made").map(String::as_str) == Some("tower")
}

/// Even on a footprint this wide, a building that stays under the vanilla ceiling needs no
/// closer look.
const WIDEST_FOOTPRINT_M: f64 = 1000.0;

/// An outline the check reads a top for: its outer and inner node-id ways, tags and
/// Overture's values.
struct Outline<'a> {
    outer: Vec<&'a [u64]>,
    inner: Vec<&'a [u64]>,
    tags: &'a HashMap<String, String>,
    hint: crate::element_processing::buildings::HeightHint,
}

/// A mast, antenna or chimney: a node's spot, or a way's nodes.
struct Mast<'a> {
    spot: Option<(f64, f64)>,
    nodes: &'a [u64],
    tags: &'a HashMap<String, String>,
}

/// A roof a mast may stand on: inside an outer ring and no inner one, at a top in metres.
struct Roof {
    outer: Vec<Vec<(f64, f64)>>,
    inner: Vec<Vec<(f64, f64)>>,
    top: f64,
}

/// Structures whose height could take them past the vanilla ceiling at `scale`. A building
/// counts at every node of its outline, as it stands on the highest ground under it, with
/// the roof its footprint allows; a relation by its member ways. A mast, antenna or chimney
/// on such a building stands on its roof. With `overture`, only the OSM buildings it gives a
/// height count. Buildings whose walls stay under 13 blocks are not read, and a mast on one
/// counts from the ground. An outline its parts replace counts only by its parts.
pub fn tall_structures_in(
    data: &crate::osm_parser::OsmData,
    scale: f64,
    overture: Option<&crate::overture::OvertureHints>,
) -> Vec<TallModel> {
    use crate::element_processing::buildings::{mapped_top_m, mapped_walls_m, HeightHint};
    use crate::element_processing::man_made::{mast_metres, top_over_roof_m};
    use crate::osm_parser::{point_in_ring, stitch_rings};

    let mut outlines: Vec<Outline> = Vec::new();
    let mut relations: Vec<(usize, &[crate::osm_parser::OsmMember])> = Vec::new();
    let mut masts: Vec<Mast> = Vec::new();
    let mut replaced: Option<crate::osm_parser::OutlineSuppression> = None;
    for e in data.elements() {
        let Some(tags) = e.tags.as_ref() else {
            continue;
        };
        let kind = match e.r#type.as_str() {
            "way" => "way",
            "relation" => "relation",
            _ => "node",
        };
        if mast_metres(tags, false).is_some() {
            if overture.is_none() && kind != "relation" {
                masts.push(Mast {
                    spot: e.lat.zip(e.lon).filter(|_| kind == "node"),
                    nodes: e.nodes.as_deref().unwrap_or_default(),
                    tags,
                });
            }
            continue;
        }
        if kind == "node" || !is_building(tags) {
            continue;
        }
        let hint = match overture {
            Some(o) => match o.hint_for(kind, e.id, tags) {
                Some(hint) => hint,
                None => continue,
            },
            None => HeightHint::default(),
        };
        let Some((walls, grows)) = mapped_walls_m(tags, hint) else {
            continue;
        };
        // Short walls only matter where a roof the size of the footprint could lift them.
        let reaches = may_pass_vanilla(walls, scale)
            || (grows
                && mapped_top_m(tags, hint, WIDEST_FOOTPRINT_M, WIDEST_FOOTPRINT_M)
                    .is_some_and(|m| may_pass_vanilla(m, scale)));
        if !reaches {
            continue;
        }
        // Only worked out once a tall outline turns up.
        if replaced
            .get_or_insert_with(|| crate::osm_parser::outlines_replaced_by_parts(data))
            .contains(&(kind, e.id))
        {
            continue;
        }
        if kind == "relation" {
            relations.push((outlines.len(), &e.members));
        }
        outlines.push(Outline {
            outer: e.nodes.as_deref().into_iter().collect(),
            inner: Vec::new(),
            tags,
            hint,
        });
    }
    if !relations.is_empty() {
        let members: HashSet<u64> = relations
            .iter()
            .flat_map(|(_, m)| m.iter().filter(|m| m.r#type == "way").map(|m| m.r#ref))
            .collect();
        let way_nodes: HashMap<u64, &[u64]> = data
            .elements()
            .iter()
            .filter(|e| e.r#type == "way" && members.contains(&e.id))
            .filter_map(|e| Some((e.id, e.nodes.as_deref()?)))
            .collect();
        for (at, members) in relations {
            for m in members.iter().filter(|m| m.r#type == "way") {
                let Some(&nodes) = way_nodes.get(&m.r#ref) else {
                    continue;
                };
                if m.role == "inner" {
                    outlines[at].inner.push(nodes);
                } else {
                    outlines[at].outer.push(nodes);
                }
            }
        }
    }
    if outlines.is_empty() && masts.is_empty() {
        return Vec::new();
    }
    let wanted: HashSet<u64> = outlines
        .iter()
        .flat_map(|o| {
            o.outer
                .iter()
                .chain(&o.inner)
                .flat_map(|r| r.iter().copied())
        })
        .chain(masts.iter().flat_map(|m| m.nodes.iter().copied()))
        .collect();
    let coords: HashMap<u64, (f64, f64)> = data
        .elements()
        .iter()
        .filter(|e| e.r#type == "node" && wanted.contains(&e.id))
        .filter_map(|e| Some((e.id, (e.lat?, e.lon?))))
        .collect();
    let point = |id: &u64| coords.get(id).copied();

    // The highest top standing on each node, and the roofs a mast may stand on.
    let mut node_top: HashMap<u64, f64> = HashMap::new();
    let mut roofs: Vec<Roof> = Vec::new();
    for outline in &outlines {
        let nodes = || {
            outline
                .outer
                .iter()
                .chain(&outline.inner)
                .flat_map(|r| r.iter())
        };
        let (w_m, l_m) = extent_m(
            outline
                .outer
                .iter()
                .flat_map(|r| r.iter().filter_map(point)),
        );
        let Some(top) = mapped_top_m(outline.tags, outline.hint, w_m, l_m) else {
            continue;
        };
        if !masts.is_empty() {
            let rings =
                |ways: &[&[u64]]| stitch_rings(ways.iter().map(|r| r.to_vec()).collect(), &coords);
            roofs.push(Roof {
                outer: rings(&outline.outer),
                inner: rings(&outline.inner),
                top,
            });
        }
        if may_pass_vanilla(top, scale) {
            for id in nodes() {
                let at = node_top.entry(*id).or_insert(top);
                *at = at.max(top);
            }
        }
    }
    let mut found: Vec<TallModel> = node_top
        .into_iter()
        .filter_map(|(id, height_m)| {
            let (lat, lon) = point(&id)?;
            Some(TallModel { lat, lon, height_m })
        })
        .collect();
    for mast in masts {
        let spots: Vec<(f64, f64)> = match mast.spot {
            Some(spot) => vec![spot],
            None => mast.nodes.iter().filter_map(point).collect(),
        };
        for (lat, lon) in spots {
            let roof = roofs
                .iter()
                .filter(|r| {
                    r.outer.iter().any(|ring| point_in_ring(lat, lon, ring))
                        && !r.inner.iter().any(|ring| point_in_ring(lat, lon, ring))
                })
                .map(|r| r.top)
                .reduce(f64::max);
            let Some(metres) = mast_metres(mast.tags, roof.is_some()) else {
                continue;
            };
            let height_m = roof.map_or(metres, |r| top_over_roof_m(metres, r, scale));
            if may_pass_vanilla(height_m, scale) {
                found.push(TallModel { lat, lon, height_m });
            }
        }
    }
    // HashMap order is not stable, and the fit these feed must be.
    found.sort_by(|a, b| {
        (a.lat, a.lon, a.height_m)
            .partial_cmp(&(b.lat, b.lon, b.height_m))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
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
/// `seam_pad` is a One World's margin around its area: a lift that would cross the
/// area's edge is left out, since the neighbouring area would not apply it.
pub(crate) fn raise_peaks(
    grid: &mut [Vec<f64>],
    bbox: &LLBBox,
    peaks: &[PeakHint],
    m_per_cell: f64,
    seam_pad: Option<usize>,
) {
    let h = grid.len();
    let w = grid.first().map_or(0, Vec::len);
    if w < 2 || h < 2 || m_per_cell <= 0.0 {
        return;
    }
    let (min_lat, max_lat) = (bbox.min().lat(), bbox.max().lat());
    let (min_lon, max_lon) = (bbox.min().lng(), bbox.max().lng());
    let reach = (3.0 * SIGMA_M / m_per_cell).ceil() as isize;
    for p in peaks {
        if p.lat < min_lat || p.lat > max_lat || p.lon < min_lon || p.lon > max_lon {
            continue;
        }
        let gx = (p.lon - min_lon) / (max_lon - min_lon) * (w - 1) as f64;
        let gy = (max_lat - p.lat) / (max_lat - min_lat) * (h - 1) as f64;
        let (cx, cy) = (gx.round() as usize, gy.round() as usize);
        if let Some(pad) = seam_pad {
            let m = pad + reach as usize;
            if cx < m || cy < m || cx + m >= w || cy + m >= h {
                continue;
            }
        }
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
        .filter(|m| inside(bbox, m.lat, m.lon))
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
                {"type": "way", "id": 16, "nodes": [99], "tags": {"building": "yes", "height": "400"}},
                {"type": "node", "id": 5, "lat": 40.5, "lon": -74.5},
                {"type": "way", "id": 17, "nodes": [5, 1]},
                {"type": "relation", "id": 20, "members": [
                    {"type": "way", "ref": 17, "role": "outer"},
                    {"type": "way", "ref": 98, "role": "inner"}
                ], "tags": {"type": "multipolygon", "building": "yes", "height": "450"}}
            ]}"#,
        )
        .unwrap();
        let found = tall_structures_in(&data, 1.0, None);
        let at = |lat: f64, lon: f64, height_m: f64| TallModel { lat, lon, height_m };
        // Each node keeps the tallest top standing on it, as the building stands on the
        // highest ground under it. 100 office storeys at 4 blocks each, plus the taller
        // ground floor, lose to the 541 m tower on the same nodes.
        assert_eq!(
            found,
            vec![
                at(40.0, -74.0, 541.0),
                at(40.1, -74.1, 541.0),
                at(40.2, -74.2, 300.0),
                at(40.5, -74.5, 450.0),
            ]
        );
        // At scale 0.02 even the 541 m tower stays under 13 blocks.
        assert!(tall_structures_in(&data, 0.02, None).is_empty());

        // Only a run that builds them and still picks its build height wants them.
        let hints = |extra: &[&str]| {
            use clap::Parser;
            let mut cmd = vec!["arnis", "--output-dir", ".", "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            let args = crate::args::Args::parse_from(cmd);
            TerrainHints::from_osm(&data, &args).tall_structures
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

    /// A tunnel under a river often has no node on the water, so its points run the whole way.
    #[test]
    fn graded_ways_are_surface_roads_railways_and_waterways() {
        let data: crate::osm_parser::OsmData = serde_json::from_str(
            r#"{"elements": [
                {"type": "node", "id": 1, "lat": 46.000, "lon": 8.000},
                {"type": "node", "id": 2, "lat": 46.001, "lon": 8.000},
                {"type": "way", "id": 10, "nodes": [1, 2], "tags": {"highway": "secondary"}},
                {"type": "way", "id": 11, "nodes": [1, 2], "tags": {"highway": "track"}},
                {"type": "way", "id": 12, "nodes": [1, 2], "tags": {"railway": "rail"}},
                {"type": "way", "id": 13, "nodes": [1, 2], "tags": {"waterway": "stream"}},
                {"type": "way", "id": 14, "nodes": [1, 2], "tags": {"highway": "path"}},
                {"type": "way", "id": 15, "nodes": [1, 2], "tags": {"highway": "primary", "tunnel": "yes"}},
                {"type": "way", "id": 16, "nodes": [1, 2], "tags": {"railway": "abandoned"}},
                {"type": "way", "id": 17, "nodes": [1, 2], "tags": {"natural": "cliff"}},
                {"type": "way", "id": 18, "nodes": [1, 99], "tags": {"waterway": "river"}},
                {"type": "way", "id": 19, "nodes": [1, 2], "tags": {"railway": "rail", "bridge": "viaduct"}},
                {"type": "way", "id": 20, "nodes": [1, 2], "tags": {"railway": "subway"}}
            ]}"#,
        )
        .unwrap();
        // The road, the track, the railway and both waterways, not the viaduct or the subway;
        // the river keeps the node the fetch has.
        let lines = terrain_ways(&data, Some(true)).1;
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], vec![(46.000, 8.000), (46.001, 8.000)]);
        assert_eq!(lines[4], vec![(46.000, 8.000)]);
        // Without roads only the waterways.
        assert_eq!(terrain_ways(&data, Some(false)).1.len(), 2);
        // Nor anything where no detail can run.
        assert!(terrain_ways(&data, None).1.is_empty());
    }

    #[test]
    fn bores_run_along_tunnels_and_find_the_water_they_cross() {
        let data: crate::osm_parser::OsmData = serde_json::from_str(
            r#"{"elements": [
                {"type": "node", "id": 1, "lat": 40.700, "lon": -74.010},
                {"type": "node", "id": 2, "lat": 40.700, "lon": -74.000},
                {"type": "node", "id": 3, "lat": 40.705, "lon": -74.010},
                {"type": "node", "id": 4, "lat": 40.705, "lon": -74.000},
                {"type": "way", "id": 10, "nodes": [1, 2], "tags": {"railway": "subway"}},
                {"type": "way", "id": 11, "nodes": [3, 4], "tags": {"highway": "primary", "tunnel": "yes"}},
                {"type": "way", "id": 12, "nodes": [1, 3], "tags": {"railway": "subway", "tunnel": "no"}},
                {"type": "way", "id": 13, "nodes": [2, 4], "tags": {"highway": "footway", "tunnel": "building_passage"}},
                {"type": "way", "id": 14, "nodes": [1, 4], "tags": {"highway": "primary"}}
            ]}"#,
        )
        .unwrap();
        let bores = terrain_ways(&data, None).0;
        // Both bored ways, about 840 m each, every two metres; none of the others.
        assert!((800..900).contains(&bores.len()), "{}", bores.len());
        assert!(bores.iter().all(|&(lat, _)| lat == 40.700 || lat == 40.705));
        let gap = bores
            .windows(2)
            .filter(|p| p[0].0 == p[1].0)
            .map(|p| extent_m(p.iter().copied()).0)
            .fold(0.0, f64::max);
        assert!(gap <= BORE_STEP_M, "{gap} m between points");

        // A river between lon -74.006 and -74.004, on a 100-cell land cover over the area.
        let bbox = LLBBox::new(40.695, -74.012, 40.710, -73.998).unwrap();
        let cover = |water: bool| {
            let n = 100;
            let grid = (0..n)
                .map(|_| {
                    (0..n)
                        .map(|x| {
                            let lon = -74.012 + 0.014 * x as f64 / (n - 1) as f64;
                            if water && (-74.006..-74.004).contains(&lon) {
                                crate::land_cover::LC_WATER
                            } else {
                                crate::land_cover::LC_GRASSLAND
                            }
                        })
                        .collect()
                })
                .collect();
            crate::land_cover::LandCoverData {
                grid,
                water_distance: vec![vec![0u8; n]; n],
                water_blend_cache: once_cell::sync::OnceCell::new(),
                beach_tiles: once_cell::sync::OnceCell::new(),
                width: n,
                height: n,
                cells_per_meter: 1.0,
            }
        };
        let crossed = bore_water_cells(&cover(true), &bbox, &bores);
        // Both bores cross the river's 14 or so columns, each cell once.
        assert!((20..=34).contains(&crossed.len()), "{crossed:?}");
        assert!(crossed.windows(2).all(|p| p[0] < p[1]));
        assert!(bore_water_cells(&cover(false), &bbox, &bores).is_empty());
        // Water outside the area the land cover covers does not count.
        let away = LLBBox::new(40.800, -74.012, 40.815, -73.998).unwrap();
        assert!(bore_water_cells(&cover(true), &away, &bores).is_empty());
    }

    // The IHZ: a 25-storey outline is built as its 93.5 m flat parts, so only they count.
    #[test]
    fn an_outline_replaced_by_its_parts_counts_by_its_parts() {
        let tops = |part_key: &str| {
            let json = r#"{"elements": [
                {"type": "node", "id": 1, "lat": 40.0, "lon": -74.0},
                {"type": "node", "id": 2, "lat": 40.0, "lon": -73.999},
                {"type": "node", "id": 3, "lat": 40.001, "lon": -73.999},
                {"type": "node", "id": 4, "lat": 40.001, "lon": -74.0},
                {"type": "way", "id": 10, "nodes": [1, 2, 3, 4, 1], "tags": {"building": "office", "building:levels": "25"}},
                {"type": "way", "id": 11, "nodes": [1, 2, 3, 4, 1], "tags": {"KEY": "yes", "height": "93.5", "roof:shape": "flat"}}
            ]}"#;
            let data: crate::osm_parser::OsmData =
                serde_json::from_str(&json.replace("KEY", part_key)).unwrap();
            tall_structures_in(&data, 1.0, None)
                .into_iter()
                .map(|m| m.height_m)
                .reduce(f64::max)
        };
        assert_eq!(tops("building:part"), Some(93.5));
        // Two buildings on one footprint: the outline stands as itself.
        assert!(tops("building").is_some_and(|m| m > 100.0));
    }

    #[test]
    fn masts_and_chimneys_stand_as_tall_as_generated() {
        use crate::element_processing::man_made::mast_metres;
        let h = |pairs: &[(&str, &str)], on_roof: bool| mast_metres(&tags(pairs), on_roof);
        assert_eq!(h(&[("man_made", "mast")], false), Some(30.0));
        assert_eq!(h(&[("man_made", "mast")], true), Some(6.0));
        assert_eq!(
            h(&[("man_made", "mast"), ("tower:type", "lighting")], false),
            Some(20.0)
        );
        assert_eq!(h(&[("man_made", "antenna")], false), Some(10.0));
        assert_eq!(h(&[("man_made", "chimney")], false), Some(40.0));
        assert_eq!(
            h(&[("man_made", "chimney"), ("height", "120")], false),
            Some(120.0)
        );
        // An implausible height falls back like the generator's does.
        assert_eq!(
            h(&[("man_made", "mast"), ("height", "2000")], false),
            Some(30.0)
        );
        assert_eq!(h(&[("man_made", "tower")], false), None);
    }

    #[test]
    fn a_mast_on_a_roof_stands_on_top_of_the_building() {
        let data = |antenna_m: &str| -> crate::osm_parser::OsmData {
            let json = r#"{"elements": [
                {"type": "node", "id": 1, "lat": 40.0, "lon": -74.0},
                {"type": "node", "id": 2, "lat": 40.0, "lon": -73.999},
                {"type": "node", "id": 3, "lat": 40.001, "lon": -73.999},
                {"type": "node", "id": 4, "lat": 40.001, "lon": -74.0},
                {"type": "way", "id": 10, "nodes": [1, 2, 3, 4, 1], "tags": {"building": "yes", "height": "330"}},
                {"type": "node", "id": 5, "lat": 40.0005, "lon": -73.9995, "tags": {"man_made": "antenna", "height": "ANTENNA"}},
                {"type": "node", "id": 6, "lat": 40.002, "lon": -74.0, "tags": {"man_made": "mast", "height": "60"}}
            ]}"#;
            serde_json::from_str(&json.replace("ANTENNA", antenna_m)).unwrap()
        };
        let height_at = |data: &crate::osm_parser::OsmData, lat: f64, lon: f64| {
            tall_structures_in(data, 1.0, None)
                .into_iter()
                .find(|m| m.lat == lat && m.lon == lon)
                .map(|m| m.height_m)
        };
        // Shorter than the tower, the antenna is its own height on the roof.
        assert_eq!(height_at(&data("60"), 40.0005, -73.9995), Some(390.0));
        // Taller, it is mapped from the ground, as `rise_above` reads it.
        assert_eq!(height_at(&data("400"), 40.0005, -73.9995), Some(400.0));
        // Beside the tower a mast stands on the ground.
        assert_eq!(height_at(&data("60"), 40.002, -74.0), Some(60.0));
        assert_eq!(height_at(&data("60"), 40.0, -74.0), Some(330.0));
    }

    #[test]
    fn a_mast_in_a_courtyard_stands_on_the_ground() {
        // A 330 m block with an inner courtyard, its outer ring split in two ways.
        let data: crate::osm_parser::OsmData = serde_json::from_str(
            r#"{"elements": [
                {"type": "node", "id": 1, "lat": 40.0, "lon": -74.0},
                {"type": "node", "id": 2, "lat": 40.0, "lon": -73.99},
                {"type": "node", "id": 3, "lat": 40.01, "lon": -73.99},
                {"type": "node", "id": 4, "lat": 40.01, "lon": -74.0},
                {"type": "node", "id": 5, "lat": 40.004, "lon": -73.996},
                {"type": "node", "id": 6, "lat": 40.004, "lon": -73.994},
                {"type": "node", "id": 7, "lat": 40.006, "lon": -73.994},
                {"type": "node", "id": 8, "lat": 40.006, "lon": -73.996},
                {"type": "way", "id": 10, "nodes": [1, 2, 3]},
                {"type": "way", "id": 11, "nodes": [3, 4, 1]},
                {"type": "way", "id": 12, "nodes": [5, 6, 7, 8, 5]},
                {"type": "relation", "id": 20, "members": [
                    {"type": "way", "ref": 10, "role": "outer"},
                    {"type": "way", "ref": 11, "role": "outer"},
                    {"type": "way", "ref": 12, "role": "inner"}
                ], "tags": {"type": "multipolygon", "building": "yes", "height": "330"}},
                {"type": "node", "id": 30, "lat": 40.005, "lon": -73.995, "tags": {"man_made": "mast", "height": "30"}},
                {"type": "node", "id": 31, "lat": 40.002, "lon": -73.998, "tags": {"man_made": "mast", "height": "30"}}
            ]}"#,
        )
        .unwrap();
        let found = tall_structures_in(&data, 1.0, None);
        let at = |lat: f64, lon: f64| {
            found
                .iter()
                .find(|m| m.lat == lat && m.lon == lon)
                .map(|m| m.height_m)
        };
        assert_eq!(at(40.005, -73.995), Some(30.0));
        assert_eq!(at(40.002, -73.998), Some(360.0));
        // The courtyard's own nodes carry the building's walls too.
        assert_eq!(at(40.004, -73.996), Some(330.0));
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
            None,
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
    fn a_lift_that_would_cross_a_seam_is_left_out() {
        // A cone peaking at 530 m 50 cells from every edge; the lift reaches 36 cells.
        let cone: Vec<Vec<f64>> = (0..101)
            .map(|y| {
                (0..101)
                    .map(|x| {
                        let d = ((x as f64 - 50.0).powi(2) + (y as f64 - 50.0).powi(2)).sqrt();
                        530.0 - d * 4.0
                    })
                    .collect()
            })
            .collect();
        let b = bbox();
        let peak = PeakHint {
            lat: (b.min().lat() + b.max().lat()) / 2.0,
            lon: (b.min().lng() + b.max().lng()) / 2.0,
            ele_m: 710.0,
        };
        let summit = |seam_pad| {
            let mut grid = cone.clone();
            raise_peaks(&mut grid, &b, &[peak], 5.0, seam_pad);
            grid[50][50]
        };
        assert!(summit(None) > 700.0);
        assert!(summit(Some(14)) > 700.0);
        assert_eq!(summit(Some(15)), 530.0);
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
            None,
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
            None,
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
            None,
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
            None,
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

    #[test]
    fn overture_structures_arrive_second_and_a_dropped_sender_ends_the_wait() {
        let tower = TallModel {
            lat: 1.0,
            lon: 2.0,
            height_m: 400.0,
        };
        let (tx, source) = PeakSource::channel();
        let first = TerrainHints {
            overture_follows: true,
            ..Default::default()
        };
        tx.send(first.clone()).unwrap();
        tx.send(TerrainHints {
            tall_structures: vec![tower],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(source.take(), first);
        assert_eq!(source.take_overture(), vec![tower]);
        // Asked again, there is nothing left to wait for.
        assert!(source.take_overture().is_empty());

        // A run whose Overture never came still lets the terrain thread go.
        let (tx, source) = PeakSource::channel();
        tx.send(first.clone()).unwrap();
        drop(tx);
        assert_eq!(source.take(), first);
        assert!(source.take_overture().is_empty());
    }
}
