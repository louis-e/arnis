use crate::args::{Args, GenerationMode};
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::element_processing::bridge_styles::{
    decorate_bridge_above_deck, place_arch_below_deck, place_pier_bent, place_pillar,
    BridgePathSample, BridgeStyle,
};
use crate::element_processing::bridges::{
    along_steps, is_bridge_way, lays_deck, way_cells, BridgeStructureMap, BridgeSurfaceMap,
};
use crate::element_processing::connected_blocks::{
    connected_iron_bars, four_connected_line, place_connected, stair_steps,
};
use crate::element_processing::get_nearest_non_road_block;
use crate::element_processing::raceways;
use crate::element_processing::road_detail::RoadDetail;
use crate::element_processing::road_markings::{self, CrossMark, CrossingPaint, RoadMarkingIndex};
use crate::element_processing::road_width;
use crate::element_processing::surfaces::{
    cycleway_palette, get_blocks_for_surface, get_blocks_for_surface_way, semirandom_surface,
};
use crate::element_processing::traffic_lights::is_signal;
use crate::element_processing::tunnel_grade::{
    chains, grade_line, pin_area_edges, planning_order, portal_cuttings, walkable, Beside, Bore,
    CUTTING_GAP,
};
use crate::floodfill::flood_fill_area;
use crate::floodfill_cache::{CoordinateBitmap, FloodFillCache, RoadMaskBitmap};
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use fnv::FnvHashMap;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Centerline cells between pillars under a raised non-bridge road.
const OVERPASS_PILLAR_INTERVAL: usize = 8;

/// What one level change costs a road profile, in squared blocks of deviation from the
/// ground. Dips shorter than about ten cells cost more than they save and are levelled.
const ROAD_STEP_COST: f64 = 5.0;
/// Levels either side of the smoothed ground a road profile may take.
const ROAD_PROFILE_REACH: i32 = 2;
const ROAD_PROFILE_LEVELS: usize = (2 * ROAD_PROFILE_REACH + 2) as usize;
/// Centerline cells either side a road's heading is read over.
const ROAD_HEADING_CELLS: usize = 4;
/// Gaussian, sigma 2 cells, so a diagonal centerline stepping across a side slope
/// doesn't read as bumps.
const ROAD_SMOOTHING: [f64; 13] = [
    0.0022, 0.0088, 0.0270, 0.0648, 0.1210, 0.1760, 0.1995, 0.1760, 0.1210, 0.0648, 0.0270, 0.0088,
    0.0022,
];
/// Half the side of the square a junction's level is averaged over.
const JUNCTION_RADIUS: i32 = 3;

/// Height over the ground from which a ramp spans a way below it rather than walling it in.
const RAMP_SPAN_CLEARANCE: i32 = 4;

/// Unrounded ground at a column, ignoring any surface a road has set.
fn terrain_exact(editor: &WorldEditor, x: i32, z: i32) -> f64 {
    editor
        .terrain_level_exact(x, z)
        .filter(|h| h.is_finite())
        .unwrap_or_else(|| f64::from(editor.get_ground_level(x, z)))
}

/// The level every way through a junction is held to, from the ground around it alone,
/// so each way reaches the same level whatever order or tile it is drawn in.
pub(crate) fn junction_level(editor: &WorldEditor, x: i32, z: i32) -> i32 {
    let mut sum = 0.0;
    for dz in -JUNCTION_RADIUS..=JUNCTION_RADIUS {
        for dx in -JUNCTION_RADIUS..=JUNCTION_RADIUS {
            sum += terrain_exact(editor, x + dx, z + dz);
        }
    }
    let side = f64::from(2 * JUNCTION_RADIUS + 1);
    (sum / (side * side)).round() as i32
}

/// Surface Y of an at-grade road for each cell of its centerline path, in the order the
/// renderer walks it. The ground across the road is averaged, smoothed along it, then
/// levelled so the road steps only where the ground does. Nodes `pin_at` gives a level are
/// held to it, as are the cells across a One World area's edge.
pub(crate) fn road_profile(
    editor: &WorldEditor,
    way: &ProcessedWay,
    block_range: i32,
    connectivity: &HighwayConnectivityMap,
    pin_at: impl Fn(i32, i32) -> Option<i32>,
) -> Vec<i32> {
    let Some(first) = way.nodes.first() else {
        return Vec::new();
    };
    let mut cells: Vec<(i32, i32)> = vec![(first.x, first.z)];
    let mut node_cells: Vec<usize> = vec![0];
    for pair in way.nodes.windows(2) {
        let path = bresenham_line(pair[0].x, 0, pair[0].z, pair[1].x, 0, pair[1].z);
        cells.extend(path.iter().skip(1).map(|&(x, _, z)| (x, z)));
        node_cells.push(cells.len() - 1);
    }
    let n = cells.len();
    let width = f64::from(2 * block_range + 1);
    let across: Vec<f64> = (0..n)
        .map(|i| {
            let (ax, az) = cells[i.saturating_sub(ROAD_HEADING_CELLS)];
            let (bx, bz) = cells[(i + ROAD_HEADING_CELLS).min(n - 1)];
            let (tx, tz) = (f64::from(bx - ax), f64::from(bz - az));
            let len = tx.hypot(tz);
            let (px, pz) = if len > 0.0 {
                (-tz / len, tx / len)
            } else {
                (0.0, 0.0)
            };
            let (cx, cz) = (f64::from(cells[i].0), f64::from(cells[i].1));
            let sum: f64 = (-block_range..=block_range)
                .map(|k| {
                    let k = f64::from(k);
                    terrain_exact(
                        editor,
                        (cx + px * k).round() as i32,
                        (cz + pz * k).round() as i32,
                    )
                })
                .sum();
            sum / width
        })
        .collect();
    let half = ROAD_SMOOTHING.len() / 2;
    // The weights sum to just under 1; unnormalised they pull a road a block towards Y 0
    // a thousand blocks out, where One World terrain lies.
    let total: f64 = ROAD_SMOOTHING.iter().sum();
    let smooth: Vec<f64> = (0..n)
        .map(|i| {
            ROAD_SMOOTHING
                .iter()
                .enumerate()
                .map(|(k, w)| w * across[(i + k).saturating_sub(half).min(n - 1)])
                .sum::<f64>()
                / total
        })
        .collect();

    let mut pins: Vec<Option<i32>> = vec![None; n];
    for (node, &cell) in way.nodes.iter().zip(&node_cells) {
        if let Some(level) = pin_at(node.x, node.z) {
            pins[cell] = Some(level);
        }
    }
    // Each area clips the way at its own padding, so both take the edge's level from one cell.
    pin_area_edges(&mut pins, &cells, connectivity.area.as_ref(), |i| {
        smooth[i].round() as i32
    });
    level_profile(&smooth, &pins)
}

/// The profile an at-grade road is laid on. A tunnel portal keeps the ground at its mouth,
/// which the bore starts from, and a bridge the level its deck lands at.
fn at_grade_profile(
    editor: &WorldEditor,
    way: &ProcessedWay,
    block_range: i32,
    connectivity: &HighwayConnectivityMap,
    tunnel_portals: &TunnelPortalMap,
    bridges: &BridgeStructureMap,
) -> Vec<i32> {
    road_profile(editor, way, block_range, connectivity, |x, z| {
        connectivity.is_shared(x, z).then(|| {
            if let Some(y) = bridges.landing_y(x, z) {
                y
            } else if tunnel_portals.is_open_end((x, z)) {
                editor
                    .terrain_level(x, z)
                    .unwrap_or_else(|| editor.get_ground_level(x, z))
            } else {
                junction_level(editor, x, z)
            }
        })
    })
}

/// The level each road is laid at, for what drives on it: a deck, a ramp or the way's own
/// profile. Asking the column instead would lift a road passing under a bridge onto the deck.
pub(crate) struct RoadSurfaces<'a> {
    editor: &'a WorldEditor<'a>,
    bridges: &'a BridgeStructureMap,
    connectivity: &'a HighwayConnectivityMap,
    tunnel_portals: &'a TunnelPortalMap,
    scale: f64,
    /// Centerline cells and their levels, by way.
    ways: RefCell<FnvHashMap<u64, Rc<WaySurface>>>,
}

struct WaySurface {
    cells: Vec<(i32, i32)>,
    ys: Vec<i32>,
    /// The cells by tile of `SURFACE_TILE` blocks, so a lookup reads only those nearby.
    tiles: FnvHashMap<(i32, i32), Vec<u32>>,
}

/// Tile size of the way surface index: lanes keep well within a tile of their way.
const SURFACE_TILE: i32 = 16;

impl WaySurface {
    fn new(cells: Vec<(i32, i32)>, ys: Vec<i32>) -> Self {
        let mut tiles: FnvHashMap<(i32, i32), Vec<u32>> = FnvHashMap::default();
        for (k, &(x, z)) in cells.iter().enumerate() {
            tiles
                .entry((x.div_euclid(SURFACE_TILE), z.div_euclid(SURFACE_TILE)))
                .or_default()
                .push(k as u32);
        }
        Self { cells, ys, tiles }
    }

    /// The index of the cell nearest (x, z): from the tiles around it, or else all of them.
    fn nearest(&self, x: i32, z: i32) -> Option<usize> {
        let d2 = |k: usize| {
            let (cx, cz) = self.cells[k];
            i64::from(cx - x).pow(2) + i64::from(cz - z).pow(2)
        };
        let (tx, tz) = (x.div_euclid(SURFACE_TILE), z.div_euclid(SURFACE_TILE));
        let near = (-1..=1)
            .flat_map(|dz| (-1..=1).map(move |dx| (tx + dx, tz + dz)))
            .filter_map(|t| self.tiles.get(&t))
            .flatten()
            .map(|&k| k as usize)
            .min_by_key(|&k| d2(k));
        // Within a tile, nothing outside the ring can be nearer.
        near.filter(|&k| d2(k) <= i64::from(SURFACE_TILE).pow(2))
            .or_else(|| (0..self.cells.len()).min_by_key(|&k| d2(k)))
    }
}

impl<'a> RoadSurfaces<'a> {
    pub(crate) fn new(
        editor: &'a WorldEditor<'a>,
        bridges: &'a BridgeStructureMap,
        connectivity: &'a HighwayConnectivityMap,
        tunnel_portals: &'a TunnelPortalMap,
        scale: f64,
    ) -> Self {
        Self {
            editor,
            bridges,
            connectivity,
            tunnel_portals,
            scale,
            ways: RefCell::default(),
        }
    }

    /// The surface block's height of `way` at the centerline cell nearest (x, z).
    pub(crate) fn y_at(&self, way: &ProcessedWay, x: i32, z: i32) -> i32 {
        let surface = {
            let mut ways = self.ways.borrow_mut();
            Rc::clone(
                ways.entry(way.id)
                    .or_insert_with(|| Rc::new(self.surface_of(way))),
            )
        };
        let nearest = surface.nearest(x, z).unwrap_or(0);
        surface
            .ys
            .get(nearest)
            .copied()
            .unwrap_or_else(|| self.editor.get_ground_level(x, z))
    }

    fn surface_of(&self, way: &ProcessedWay) -> WaySurface {
        let cells = way_cells(way);
        let ys = if let Some(member) = self.bridges.lookup_member(way.id) {
            (0..cells.len()).map(|t| member.y_at(t)).collect()
        } else if let Some(ramp) = self.bridges.lookup_ramp(way.id) {
            (0..cells.len()).map(|t| ramp.y_at(t)).collect()
        } else {
            let half = way
                .tags
                .get("highway")
                .map_or(1, |h| highway_block_range(h, &way.tags, self.scale));
            let mut ys = at_grade_profile(
                self.editor,
                way,
                half,
                self.connectivity,
                self.tunnel_portals,
                self.bridges,
            );
            // The road dips into a tunnel portal it leads to.
            if let Some(approach) = self.tunnel_portals.approach(way.id) {
                for (t, y) in ys.iter_mut().enumerate() {
                    *y += tunnel_approach_offset(approach, t as i32).min(0);
                }
            }
            ys
        };
        WaySurface::new(cells, ys)
    }
}

/// Whole-block heights for a smoothed profile: the sequence paying least in squared
/// deviation plus `ROAD_STEP_COST` per level changed, held to `pins` where given.
fn level_profile(smooth: &[f64], pins: &[Option<i32>]) -> Vec<i32> {
    const W: usize = ROAD_PROFILE_LEVELS;
    let n = smooth.len();
    if n == 0 {
        return Vec::new();
    }
    // The levels a cell may take: W from `low`, or only its pin.
    let low = |i: usize| pins[i].unwrap_or(smooth[i].floor() as i32 - ROAD_PROFILE_REACH);
    let span = |i: usize| if pins[i].is_some() { 1 } else { W };
    let own = |i: usize, y: i32| (f64::from(y) - smooth[i]).powi(2);
    let mut cost = [f64::INFINITY; W];
    for (k, c) in cost.iter_mut().enumerate().take(span(0)) {
        *c = own(0, low(0) + k as i32);
    }
    let mut back = vec![0u8; n * W];
    for i in 1..n {
        let (prev_low, lo) = (low(i - 1), low(i));
        let mut next = [f64::INFINITY; W];
        for (k, slot) in next.iter_mut().enumerate().take(span(i)) {
            let y = lo + k as i32;
            let mut best = (f64::INFINITY, 0u8);
            for (j, &c) in cost.iter().enumerate().take(span(i - 1)) {
                let c = c + ROAD_STEP_COST * f64::from((y - prev_low - j as i32).abs());
                if c < best.0 {
                    best = (c, j as u8);
                }
            }
            *slot = best.0 + own(i, y);
            back[i * W + k] = best.1;
        }
        cost = next;
    }
    let mut k = (0..span(n - 1))
        .min_by(|&a, &b| cost[a].total_cmp(&cost[b]))
        .unwrap_or(0);
    let mut out = vec![0; n];
    for i in (0..n).rev() {
        out[i] = low(i) + k as i32;
        k = usize::from(back[i * W + k]);
    }
    out
}

/// Default block-mix used for road surfaces when no `surface=*` tag is
/// present. Kept as a constant so the `semirandom_surface` call sites read
/// consistently across the file.
const DEFAULT_ROAD_MIX: &[Block] = &[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA];

/// Bare earth for a road under construction, whatever its final surface.
const CONSTRUCTION_BED: &[Block] = &[COARSE_DIRT, GRAVEL, DIRT];

/// Blocks that a road write must NOT overwrite. Intentionally narrow:
/// - `GRAY_CONCRETE_POWDER`, `CYAN_TERRACOTTA`: the default asphalt mix,
///   preserved so two asphalt roads overlapping produce a consistent
///   surface instead of re-rolling the hash per pass.
/// - `WHITE_CONCRETE`, `YELLOW_CONCRETE`: preserve lane stripes and zebra
///   crossings from being erased when a later road pass crosses them.
/// - `BLACK_CONCRETE`: not produced by highways directly, but widely
///   placed by other element processors — schoolyards in `leisure.rs`,
///   gas-station / parking forecourts in `amenities.rs`, some landuse
///   patches. A highway shouldn't paint over those.
///
/// Any other hard-surface block a way places (`SMOOTH_STONE` for
/// pedestrian footways, `BRICK`, `OAK_PLANKS`, `LIGHT_GRAY_CONCRETE`,
/// `STONE_BRICKS`, etc.) is left out so major roads can freely pave
/// over them when their footprints overlap, keeping the road surface
/// clean end-to-end.
const ROAD_PROTECTED_SURFACES: &[Block] = &[
    BLACK_CONCRETE,
    GRAY_CONCRETE_POWDER,
    CYAN_TERRACOTTA,
    WHITE_CONCRETE,
    YELLOW_CONCRETE,
    // Bridge module furniture must survive parallel side-deck ways.
    WARPED_STAIRS,
    WARPED_TRAPDOOR,
    WARPED_SLAB,
    STRIPPED_WARPED_STEM,
    STRIPPED_WARPED_HYPHAE,
    SEA_LANTERN,
    ANDESITE_WALL,
    SMOOTH_SANDSTONE_STAIRS,
    // A red bicycle path next to a road, so the road's width does not pave it over.
    RED_TERRACOTTA,
    RED_CONCRETE,
];

/// `base` followed by `extra`, as one list fixed at compile time, so a list that adds to
/// another cannot drift from it. `N` is their length together.
const fn joined<const N: usize>(base: &[Block], extra: &[Block]) -> [Block; N] {
    assert!(base.len() + extra.len() == N);
    let mut out = [AIR; N];
    let mut i = 0;
    while i < base.len() {
        out[i] = base[i];
        i += 1;
    }
    let mut j = 0;
    while j < extra.len() {
        out[base.len() + j] = extra[j];
        j += 1;
    }
    out
}

/// What a way no car drives, drawn in path asphalt, must not overwrite: the road list,
/// another path's asphalt, and the grey of a pedestrian street, driveway, crossing or access
/// aisle, which wins over a path's asphalt whichever of the two is built first.
const CAR_FREE_PROTECTED_SURFACES: &[Block] = &joined::<{ ROAD_PROTECTED_SURFACES.len() + 2 }>(
    ROAD_PROTECTED_SURFACES,
    &[POLISHED_ANDESITE, GRAY_CONCRETE],
);

/// What any other way no car drives must not overwrite: the road list and a path's asphalt.
/// Paved, gravel and earth paths otherwise meet the grey of a driveway or crossing as they
/// do on main.
const WALKWAY_PROTECTED_SURFACES: &[Block] =
    &joined::<{ ROAD_PROTECTED_SURFACES.len() + 1 }>(ROAD_PROTECTED_SURFACES, PATH_ASPHALT);

/// What the parking rows of a road, outside its lanes, must not overwrite: the road list and
/// the paving of a sidewalk or path mapped beside the road.
const PARKING_ROW_PROTECTED_SURFACES: &[Block] = &joined::<{ ROAD_PROTECTED_SURFACES.len() + 3 }>(
    ROAD_PROTECTED_SURFACES,
    &[SMOOTH_STONE, GRAY_CONCRETE, POLISHED_ANDESITE],
);

/// What a red bicycle path or race track paint must not overwrite: the road list
/// without the asphalt mix, so they show where a road's width runs over them
/// whichever of the two is built first. Lane stripes and zebras still win.
const OVER_ASPHALT_PROTECTED_SURFACES: &[Block] = &[
    BLACK_CONCRETE,
    WHITE_CONCRETE,
    YELLOW_CONCRETE,
    WARPED_STAIRS,
    WARPED_TRAPDOOR,
    WARPED_SLAB,
    STRIPPED_WARPED_STEM,
    STRIPPED_WARPED_HYPHAE,
    SEA_LANTERN,
    ANDESITE_WALL,
    SMOOTH_SANDSTONE_STAIRS,
    RED_TERRACOTTA,
    RED_CONCRETE,
];

/// What a paved sidewalk must not overwrite off the carriageway: the list above and a path's
/// asphalt. It may pave over a driveway's grey, since a sidewalk carries on across the
/// driveways along it, as on main.
const SIDEWALK_PROTECTED_SURFACES: &[Block] =
    &joined::<{ OVER_ASPHALT_PROTECTED_SURFACES.len() + 1 }>(
        OVER_ASPHALT_PROTECTED_SURFACES,
        &[POLISHED_ANDESITE],
    );

/// What a sidewalk drawn in path asphalt must not overwrite off the carriageway: the paved
/// sidewalk's list and the grey of a driveway, crossing or pedestrian street, which wins
/// over a path's asphalt whichever is built first.
const ASPHALT_SIDEWALK_PROTECTED_SURFACES: &[Block] =
    &joined::<{ SIDEWALK_PROTECTED_SURFACES.len() + 1 }>(
        SIDEWALK_PROTECTED_SURFACES,
        &[GRAY_CONCRETE],
    );

/// Blocks a highway way is surfaced with: by type, then `surface=*`, with paved sidewalks
/// in smooth stone, bicycle paths in red, and the ways no car drives never in the road's
/// asphalt mix. Shared with the paint prescan.
pub(crate) fn surface_palette(
    highway_type: &str,
    tags: &HashMap<String, String>,
) -> &'static [Block] {
    if highway_type == "construction" {
        return CONSTRUCTION_BED;
    }
    if highway_type == "cycleway" {
        if let Some(red) = cycleway_palette(tags) {
            return red;
        }
    }
    if let Some(paving) = car_free_paving(highway_type, tags) {
        return paving;
    }
    if let Some(paving) = pedestrian_paving(tags) {
        return paving;
    }
    let surface = tags.get("surface").map(String::as_str);
    if let Some(blocks) = surface.and_then(get_blocks_for_surface) {
        return blocks;
    }
    match highway_type {
        "footway" | "pedestrian" | "service" | "steps" => &[GRAY_CONCRETE],
        "path" | "bridleway" => &[DIRT_PATH],
        // Sand trap for runaway vehicles.
        "escape" => &[SAND],
        "raceway" => raceways::unsurfaced_palette(tags).unwrap_or(DEFAULT_ROAD_MIX),
        _ => DEFAULT_ROAD_MIX,
    }
}

/// Asphalt where no car drives: a step lighter than the road's and without its speckle, so a
/// shared foot and cycle path reads apart from the street it runs beside.
const PATH_ASPHALT: &[Block] = &[POLISHED_ANDESITE];

/// Whether no car drives a highway of this type: footways, paths, cycle paths, bridleways,
/// pedestrian streets and steps. Service roads, tracks and living streets carry cars.
pub(crate) fn is_walkway(highway_type: &str) -> bool {
    matches!(
        highway_type,
        "footway" | "path" | "cycleway" | "bridleway" | "pedestrian" | "steps"
    )
}

/// A way no car drives that keeps the surface of what it belongs to: a pedestrian street,
/// often as wide as the road it replaced, its grey, a crossing the road's surface, and an
/// access aisle the car park's. Each paves over a path's asphalt as a road does.
fn paves_like_road(highway_type: &str, tags: &HashMap<String, String>) -> bool {
    let tag = |k: &str| tags.get(k).map(String::as_str);
    highway_type == "pedestrian"
        || tag("crossing").is_some_and(|v| v != "no")
        || ["footway", "cycleway", "path"]
            .iter()
            .any(|k| tag(k) == Some("crossing"))
        || tag("footway") == Some("access_aisle")
}

/// How a hard `surface=*` is laid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HardSurface {
    Asphalt,
    /// Paving stones, sett or concrete.
    Paving,
}

fn hard_surface(surface: &str) -> Option<HardSurface> {
    match surface {
        "asphalt" | "chipseal" | "bitmac" => Some(HardSurface::Asphalt),
        "concrete" | "concrete:plates" | "concrete:lanes" | "paving_stones" | "sett" | "paved"
        | "cement" => Some(HardSurface::Paving),
        _ => None,
    }
}

/// The surface of a way no car drives, a footway, path, cycle path, bridleway or steps,
/// where it is hard-surfaced: `PATH_ASPHALT` for asphalt, and a sidewalk's paving for paving
/// stones, sett and concrete. With no surface tagged, a cycle path, footway or steps is
/// asphalt, though a sidewalk stays paved. The ways that pave like a road and squares keep
/// their own surfaces; anything else is left to the rest of `surface_palette`.
fn car_free_paving(highway_type: &str, tags: &HashMap<String, String>) -> Option<&'static [Block]> {
    let tag = |k: &str| tags.get(k).map(String::as_str);
    if !is_walkway(highway_type)
        || paves_like_road(highway_type, tags)
        || tag("area") == Some("yes")
    {
        return None;
    }
    let surface = match tag("surface") {
        Some(surface) => surface,
        None if highway_type == "cycleway" => "asphalt",
        None if matches!(highway_type, "footway" | "steps")
            && tag("footway") != Some("sidewalk") =>
        {
            "asphalt"
        }
        None => return None,
    };
    Some(match hard_surface(surface)? {
        HardSurface::Asphalt => PATH_ASPHALT,
        HardSurface::Paving => &[SMOOTH_STONE],
    })
}

/// A hard-surfaced footway, pedestrian street or square never takes the road's asphalt mix,
/// or it reads as part of the road: paving is light grey, asphalt a darker grey.
fn pedestrian_paving(tags: &HashMap<String, String>) -> Option<&'static [Block]> {
    if !is_pedestrian_way_tags(tags) {
        return None;
    }
    // A sidewalk with no surface tagged is paved, as nearly all are, not the grey concrete
    // close to the road's asphalt beside it.
    let surface = match tags.get("surface") {
        Some(surface) => surface.as_str(),
        None if tags.get("footway").map(String::as_str) == Some("sidewalk") => "paved",
        None => return None,
    };
    Some(match hard_surface(surface)? {
        HardSurface::Paving => &[SMOOTH_STONE],
        HardSurface::Asphalt => &[GRAY_CONCRETE],
    })
}

fn is_pedestrian_way_tags(tags: &HashMap<String, String>) -> bool {
    if let Some(h) = tags.get("highway") {
        if matches!(h.as_str(), "footway" | "pedestrian" | "steps") {
            return true;
        }
    }
    // `footway=*` subtag (sidewalk, crossing, access_aisle, traffic_island,
    // yes, …) implies a pedestrian way. Exclude the explicit `footway=no`,
    // which is occasionally used on roads to assert "this is not a footway".
    matches!(tags.get("footway").map(|s| s.as_str()), Some(v) if v != "no")
}

/// Where highways meet: the layers of the ways ending at each point, and every point
/// two or more highways pass through.
#[derive(Default)]
pub struct HighwayConnectivityMap {
    ends: HashMap<(i32, i32), Vec<i32>>,
    /// Packed with `pack_xz`, sorted.
    shared: Vec<u64>,
    /// In One World, the area being built, whose roads the next area's meet at the edge.
    area: Option<XZBBox>,
}

impl HighwayConnectivityMap {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Meets the roads of the areas built next to `area` at its edge.
    pub fn meet_at_edge_of(&mut self, area: XZBBox) {
        self.area = Some(area);
    }

    pub(crate) fn is_shared(&self, x: i32, z: i32) -> bool {
        self.shared.binary_search(&pack_xz(x, z)).is_ok()
    }
}

fn pack_xz(x: i32, z: i32) -> u64 {
    (u64::from(x as u32) << 32) | u64::from(z as u32)
}

/// How far off a deck a lamp tagged with a `layer` moves onto it, as one mapped on a sidewalk
/// the deck leaves out.
const LAYERED_LAMP_DECK_REACH: i32 = 3;

// Absolute base Y for a node feature; deck Y on a bridge, else terrain. A `layer` alone lifts
// nothing: it marks a plaza over an underpass as often as a bridge.
// `bridge_radius`: 0 = exact (lamps, bus stops, on-road signal head), 1 = the deck edge too
// (a signal pole on the rail cell).
#[inline]
fn node_feature_base_y(
    editor: &WorldEditor,
    bridge_surface: &BridgeSurfaceMap,
    x: i32,
    z: i32,
    layered: bool,
    bridge_radius: i32,
) -> i32 {
    // A layered one beside the deck stands against its edge, at the nearest deck's level.
    let deck = if layered {
        bridge_surface
            .nearest_deck_cell(x, z, bridge_radius.max(1))
            .map(|(_, y)| y)
    } else {
        bridge_surface.nearby_deck_y(x, z, bridge_radius)
    };
    deck.unwrap_or_else(|| editor.get_absolute_y(x, 0, z))
}

/// Generates highways with elevation support based on layer tags and connectivity analysis
#[allow(clippy::too_many_arguments)]
pub fn generate_highways(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    highway_connectivity: &HighwayConnectivityMap,
    flood_fill_cache: &FloodFillCache,
    road_mask: &RoadMaskBitmap,
    bridge_structures: &BridgeStructureMap,
    bridge_surface: &BridgeSurfaceMap,
    tunnel_internal_endpoints: &TunnelInternalEndpoints,
    tunnel_portals: &TunnelPortalMap,
    tunnel_footprint: &CoordinateBitmap,
    tunnel_cells: &mut Vec<HighwayTunnelCell>,
    markings: &RoadMarkingIndex,
    building_footprints: &CoordinateBitmap,
) {
    if let ProcessedElement::Way(way) = element {
        if let Some(cars) = editor.car_network() {
            cars.place_way(editor, way.id);
        }
        // A way with no room to be bored falls through and renders at grade.
        if renders_as_highway_tunnel(way)
            && generate_highway_tunnel_shell(
                editor,
                way,
                args,
                tunnel_internal_endpoints,
                tunnel_portals,
                tunnel_cells,
            )
        {
            return;
        }
    }
    generate_highways_internal(
        editor,
        element,
        args,
        highway_connectivity,
        flood_fill_cache,
        road_mask,
        bridge_structures,
        bridge_surface,
        tunnel_portals,
        tunnel_footprint,
        markings,
        building_footprints,
    );
}

// One carved cell of a highway tunnel.
pub struct HighwayTunnelCell {
    pub x: i32,
    pub z: i32,
    pub road_y: i32,
    pub half_width: i32,
    pub covered: bool,
    /// Highest Y this cell's carve may clear.
    pub carve_top: i32,
    pub palette: &'static [Block],
    pub light: bool,
    /// Open ends of this cell's way; the bore stops at them.
    faces: [Option<PortalFace>; 2],
}

pub type TunnelInternalEndpoints = HashSet<(i32, i32)>;

/// A surface way claimed as the descending approach to a tunnel portal.
#[derive(Clone, Copy, Default)]
pub struct TunnelApproach {
    /// A way between two portals is claimed by both.
    claims: [Option<ApproachClaim>; 2],
}

/// `drop` blocks under grade at cell `anchor`, easing back over `run` cells per block.
#[derive(Clone, Copy)]
struct ApproachClaim {
    anchor: i32,
    drop: i32,
    run: i32,
}

impl TunnelApproach {
    fn push(&mut self, claim: ApproachClaim) {
        if let Some(slot) = self.claims.iter_mut().find(|s| s.is_none()) {
            *slot = Some(claim);
        }
    }

    fn is_full(&self) -> bool {
        self.claims.iter().all(Option::is_some)
    }
}

/// Approach ramps into every tunnel portal, keyed by way and by portal node.
#[derive(Default)]
pub struct TunnelPortalMap {
    ways: HashMap<u64, TunnelApproach>,
    nodes: HashMap<(i32, i32), i32>,
    /// Every open tunnel end, approach or not.
    open_ends: HashSet<(i32, i32)>,
    /// Where each bored way's road lies, once planned.
    bores: HashMap<u64, RoadBore>,
}

/// Where a road tunnel way's carriageway lies, planned with the rest of its run.
#[derive(Clone, Default)]
struct RoadBore {
    road_y: Vec<i32>,
    /// Cells away from a portal whose roof stays on where the ground dips over the bore.
    shut: Vec<bool>,
}

impl TunnelPortalMap {
    pub fn is_empty(&self) -> bool {
        self.ways.is_empty()
    }

    /// The claimed approach for this way, if it is one.
    fn approach(&self, way_id: u64) -> Option<TunnelApproach> {
        self.ways.get(&way_id).copied()
    }

    /// Blocks the road has already descended at this portal node.
    fn drop_at(&self, xz: (i32, i32)) -> i32 {
        self.nodes.get(&xz).copied().unwrap_or(0)
    }

    fn is_open_end(&self, xz: (i32, i32)) -> bool {
        self.open_ends.contains(&xz)
    }

    /// Plans every bored road tunnel a run of joined ways at a time, so a tunnel split into
    /// several ways keeps one grade, and in One World meets the next area at `area`'s edge.
    pub fn plan_bores(
        &mut self,
        elements: &[ProcessedElement],
        editor: &WorldEditor,
        internal: &TunnelInternalEndpoints,
        scale: f64,
        area: Option<&XZBBox>,
    ) {
        self.bores = plan_road_tunnels(elements, editor, internal, self, scale, area);
    }
}

const TUNNEL_CEIL_OFFSET: i32 = 5; // roof height above the road
const TUNNEL_COVER_DROP: i32 = 7; // cover the bore aims for
/// Cover at which a roof is still kept; one under the target, so a DEM wiggle cannot punch a skylight.
const TUNNEL_ROOF_COVER: i32 = TUNNEL_CEIL_OFFSET + 1;
const TUNNEL_RAMP_STEP: i32 = 1; // max descent per cell
const TUNNEL_RAMP_RUN: i32 = 3; // portal ramp run per 1 block drop
const TUNNEL_LAYER_DROP: i32 = 7; // extra depth per layer below -1
const TUNNEL_LIGHT_INTERVAL: usize = 8;
// The terrain fit leaves `BORE_ROOM` between the deepest bed and the lowest a bed can lie,
// a block under the lowest a road can lie, `TUNNEL_COVER_DROP` under it.
const _: () = assert!(TUNNEL_COVER_DROP < crate::water_depth::BORE_ROOM);
/// `layer` is untrusted; deeper than this is noise and overflows `i32`.
const TUNNEL_MAX_LAYERS_BELOW: i32 = 8;

/// Extra depth demanded by a `layer` below -1; clamped, the tag is untrusted.
fn tunnel_layer_extra(tags: &HashMap<String, String>) -> i32 {
    let layer = tags
        .get("layer")
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    (-(layer.saturating_add(1)))
        .clamp(0, TUNNEL_MAX_LAYERS_BELOW)
        .saturating_mul(TUNNEL_LAYER_DROP)
}

/// Lowest Y a tunnel road may sit at, leaving room for its foundation course.
fn tunnel_min_road_y() -> i32 {
    crate::world_editor::terrain_floor_y() + 2
}

/// Lowest terrain over the whole shell footprint; a true minimum, since `covered` licenses the roof course.
fn footprint_min_terrain(editor: &WorldEditor, bx: i32, bz: i32, r: i32, fallback: i32) -> i32 {
    let mut min = fallback;
    for dx in -r..=r {
        for dz in -r..=r {
            min = min.min(editor.terrain_level(bx + dx, bz + dz).unwrap_or(fallback));
        }
    }
    min
}

/// Lowest ground the bore of `way` would have to fit under, per centerline cell.
fn tunnel_cover_profile(
    editor: &WorldEditor,
    pts: &[(i32, i32)],
    wall_off: i32,
) -> (Vec<i32>, Vec<i32>) {
    let terrain: Vec<i32> = pts
        .iter()
        .map(|&(x, z)| {
            editor
                .terrain_level(x, z)
                .unwrap_or_else(|| editor.get_ground_level(x, z))
        })
        .collect();
    let cover = pts
        .iter()
        .zip(&terrain)
        .map(|(&(x, z), &ty)| footprint_min_terrain(editor, x, z, wall_off, ty))
        .collect();
    (terrain, cover)
}

/// Centerline cells of a way, consecutive duplicates dropped.
fn way_centerline(way: &ProcessedWay) -> Vec<(i32, i32)> {
    let mut pts: Vec<(i32, i32)> = Vec::new();
    for w in way.nodes.windows(2) {
        for (bx, _, bz) in &bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z) {
            if pts.last() != Some(&(*bx, *bz)) {
                pts.push((*bx, *bz));
            }
        }
    }
    pts
}

/// Whether `way` has room for a bore anywhere; shared so no approach descends into a bore nobody dug.
fn tunnel_bore_fits(editor: &WorldEditor, way: &ProcessedWay, scale: f64) -> bool {
    let Some(highway_type) = way.tags.get("highway") else {
        return false;
    };
    let pts = way_centerline(way);
    if pts.len() < 2 {
        return false;
    }
    let wall_off = highway_block_range(highway_type, &way.tags, scale) + 1;
    let (_, cover_ys) = tunnel_cover_profile(editor, &pts, wall_off);
    let floor = tunnel_min_road_y();
    cover_ys.iter().any(|&c| c - floor >= TUNNEL_ROOF_COVER)
}

// Cracked/mossy stone-brick speckle for tunnel walls and roof.
fn tunnel_shell_block(x: i32, y: i32, z: i32) -> Block {
    let h = (x as u32)
        .wrapping_mul(73856093)
        .wrapping_add((y as u32).wrapping_mul(19349663))
        .wrapping_add((z as u32).wrapping_mul(83492791));
    match h % 100 {
        0..=14 => CRACKED_STONE_BRICKS,
        15..=17 => MOSSY_STONE_BRICKS,
        _ => STONE_BRICKS,
    }
}

// A highway way that should render as an underground tunnel.
fn renders_as_highway_tunnel(way: &ProcessedWay) -> bool {
    if !way.tags.contains_key("highway") || way.nodes.len() < 2 {
        return false;
    }
    if way.tags.get("tunnel").map(String::as_str) != Some("yes") {
        return false;
    }
    if way.tags.get("indoor").map(String::as_str) == Some("yes")
        || way.tags.get("area").map(String::as_str) == Some("yes")
    {
        return false;
    }
    if way
        .tags
        .get("level")
        .and_then(|l| l.parse::<i32>().ok())
        .is_some_and(|l| l < 0)
    {
        return false;
    }
    !matches!(
        way.tags.get("highway").map(String::as_str),
        Some("street_lamp" | "crossing" | "bus_stop" | "proposed" | "construction" | "razed")
    )
}

// Endpoints shared by 2+ tunnel ways; these stay at depth instead of ramping up.
pub fn collect_tunnel_internal_endpoints(
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
) -> TunnelInternalEndpoints {
    // Every node, not just the ends: a branch often joins another tunnel mid-way.
    let mut owner: HashMap<(i32, i32), u64> = HashMap::new();
    let mut shared: HashSet<(i32, i32)> = HashSet::new();
    for elem in elements {
        let ProcessedElement::Way(w) = elem else {
            continue;
        };
        if !renders_as_highway_tunnel(w) {
            continue;
        }
        for node in &w.nodes {
            match owner.entry((node.x, node.z)) {
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(w.id);
                }
                std::collections::hash_map::Entry::Occupied(o) => {
                    if *o.get() != w.id {
                        shared.insert((node.x, node.z));
                    }
                }
            }
        }
    }

    let mut internal = TunnelInternalEndpoints::new();
    for elem in elements {
        let ProcessedElement::Way(w) = elem else {
            continue;
        };
        if !renders_as_highway_tunnel(w) {
            continue;
        }
        for node in [&w.nodes[0], &w.nodes[w.nodes.len() - 1]] {
            // Clipped at the bbox edge: the bore continues outside, so stay at depth.
            let at_edge = node.x <= xzbbox.min_x() + 1
                || node.x >= xzbbox.max_x() - 1
                || node.z <= xzbbox.min_z() + 1
                || node.z >= xzbbox.max_z() - 1;
            if at_edge || shared.contains(&(node.x, node.z)) {
                internal.insert((node.x, node.z));
            }
        }
    }
    internal
}

/// How far the surface road at each portal descends before reaching it, mirroring bridge ramps.
/// An OSM tunnel way starts at the portal, so otherwise the bore spends 21 cells per end getting under.
pub fn collect_tunnel_portals(
    elements: &[ProcessedElement],
    editor: &WorldEditor,
    bridges: &BridgeStructureMap,
    internal: &TunnelInternalEndpoints,
    scale: f64,
) -> TunnelPortalMap {
    let mut tunnels: Vec<&ProcessedWay> = Vec::new();
    let mut surface: Vec<&ProcessedWay> = Vec::new();
    for elem in elements {
        let ProcessedElement::Way(w) = elem else {
            continue;
        };
        if !w.tags.contains_key("highway") || w.nodes.len() < 2 {
            continue;
        }
        if renders_as_highway_tunnel(w) {
            // A way with no room to bore renders at grade, so nothing descends.
            if tunnel_bore_fits(editor, w, scale) {
                tunnels.push(w);
            }
        } else if surface_highway_type(w).is_some() {
            // Only a way drawn at the surface comes down to a portal.
            surface.push(w);
        }
    }
    if tunnels.is_empty() {
        return TunnelPortalMap::default();
    }

    // Every open portal, so a chain can stop when it reaches another.
    let mut portal_nodes: HashSet<(i32, i32)> = HashSet::new();
    for w in &tunnels {
        for node in [&w.nodes[0], &w.nodes[w.nodes.len() - 1]] {
            let xz = (node.x, node.z);
            if !internal.contains(&xz) {
                portal_nodes.insert(xz);
            }
        }
    }

    // End nodes only: a way touching a portal mid-span would dip on both sides.
    let mut ends: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, w) in surface.iter().enumerate() {
        for node in [&w.nodes[0], &w.nodes[w.nodes.len() - 1]] {
            ends.entry((node.x, node.z)).or_default().push(i);
        }
    }

    let floor = tunnel_min_road_y();
    let mut portals = TunnelPortalMap::default();
    for w in &tunnels {
        let want = TUNNEL_COVER_DROP + tunnel_layer_extra(&w.tags);
        let tunnel_is_pedestrian = is_pedestrian_way_tags(&w.tags);
        for node in [&w.nodes[0], &w.nodes[w.nodes.len() - 1]] {
            let xz = (node.x, node.z);
            if internal.contains(&xz) {
                continue;
            }
            portals.open_ends.insert(xz);
            if portals.nodes.contains_key(&xz) {
                continue;
            }
            // Never below the bedrock plane at the portal itself.
            let headroom = editor
                .terrain_level(xz.0, xz.1)
                .unwrap_or_else(|| editor.get_ground_level(xz.0, xz.1))
                - floor;
            let want_here = want.min(headroom);
            if want_here <= 0 {
                continue;
            }

            // Hop from way to way while the road continues; one short segment buys a block or two.
            let budget = want_here * TUNNEL_RAMP_RUN;
            let mut taken: HashSet<u64> = HashSet::new();
            let mut branches: Vec<Vec<(usize, bool, i32)>> = Vec::new();
            let eligible_at =
                |node: (i32, i32), taken: &HashSet<u64>, portals: &TunnelPortalMap| {
                    let mut out: Vec<usize> = Vec::new();
                    for &si in ends.get(&node).into_iter().flatten() {
                        let cand = surface[si];
                        if taken.contains(&cand.id)
                            || is_bridge_way(cand)
                            || bridges.lookup_member(cand.id).is_some()
                            || bridges.lookup_ramp(cand.id).is_some()
                        {
                            continue;
                        }
                        // A sidewalk sharing the node must not dive into a road portal.
                        if is_pedestrian_way_tags(&cand.tags) != tunnel_is_pedestrian {
                            continue;
                        }
                        if portals
                            .ways
                            .get(&cand.id)
                            .is_some_and(TunnelApproach::is_full)
                        {
                            continue;
                        }
                        out.push(si);
                    }
                    out
                };

            for &first in &eligible_at(xz, &taken, &portals) {
                if taken.contains(&surface[first].id) {
                    continue;
                }
                let mut chain: Vec<(usize, bool, i32)> = Vec::new();
                let mut si = first;
                let mut at_node = xz;
                let mut dist = 0i32;
                loop {
                    let cand = surface[si];
                    let at_start = (cand.nodes[0].x, cand.nodes[0].z) == at_node;
                    chain.push((si, at_start, dist));
                    taken.insert(cand.id);
                    dist += way_bresenham_len(cand).saturating_sub(1) as i32;
                    let far = if at_start {
                        &cand.nodes[cand.nodes.len() - 1]
                    } else {
                        &cand.nodes[0]
                    };
                    at_node = (far.x, far.z);
                    if dist >= budget || chain.len() >= 8 || portal_nodes.contains(&at_node) {
                        break;
                    }
                    // Only hop where the road continues; junctions stay at grade.
                    let next = eligible_at(at_node, &taken, &portals);
                    if next.len() != 1 {
                        break;
                    }
                    si = next[0];
                }
                branches.push(chain);
            }

            // Every branch absorbs the same descent, or one ends in a cliff.
            let Some(avail) = branches
                .iter()
                .map(|b| {
                    b.last().map_or(0, |&(si, _, d0)| {
                        d0 + way_bresenham_len(surface[si]).saturating_sub(1) as i32
                    })
                })
                .min()
            else {
                continue;
            };
            let drop = want_here.min(avail);
            if drop <= 0 {
                continue;
            }
            let run = (avail / drop).clamp(1, TUNNEL_RAMP_RUN);
            for (si, at_start, d0) in branches.into_iter().flatten() {
                let cand = surface[si];
                let last = way_bresenham_len(cand).saturating_sub(1) as i32;
                let anchor = if at_start { -d0 } else { last + d0 };
                portals
                    .ways
                    .entry(cand.id)
                    .or_default()
                    .push(ApproachClaim { anchor, drop, run });
            }
            portals.nodes.insert(xz, drop);
        }
    }
    portals
}

/// Number of distinct bresenham cells a way sweeps.
fn way_bresenham_len(way: &ProcessedWay) -> usize {
    way.nodes
        .windows(2)
        .map(|p| {
            let dx = (p[1].x - p[0].x).unsigned_abs() as usize;
            let dz = (p[1].z - p[0].z).unsigned_abs() as usize;
            dx.max(dz)
        })
        .sum::<usize>()
        + 1
}

/// Descent of an approach cell below grade; a way between two portals takes the deeper claim.
fn tunnel_approach_offset(approach: TunnelApproach, tds: i32) -> i32 {
    let mut drop = 0;
    for claim in approach.claims.iter().flatten() {
        drop = drop.max(claim.drop - (tds - claim.anchor).abs() / claim.run);
    }
    -drop.max(0)
}

/// What a long road tunnel's grade is held to, in blocks per cell.
const ROAD_TUNNEL_GRADE: f64 = 0.06;

fn plan_road_tunnels(
    elements: &[ProcessedElement],
    editor: &WorldEditor,
    internal: &TunnelInternalEndpoints,
    portals: &TunnelPortalMap,
    scale: f64,
    area: Option<&XZBBox>,
) -> HashMap<u64, RoadBore> {
    let ways: Vec<&ProcessedWay> = elements
        .iter()
        .filter_map(|e| match e {
            ProcessedElement::Way(w)
                if renders_as_highway_tunnel(w) && tunnel_bore_fits(editor, w, scale) =>
            {
                Some(w)
            }
            _ => None,
        })
        .collect();
    let cells: Vec<Vec<(i32, i32)>> = ways.iter().map(|w| way_centerline(w)).collect();
    // The tunnel ways ending at each node, and the nodes one passes through.
    let mut ending: HashMap<u64, Vec<(usize, usize)>> = HashMap::new();
    let mut through: HashSet<u64> = HashSet::new();
    for (w, way) in ways.iter().enumerate() {
        let last = way.nodes.len() - 1;
        for (k, node) in way.nodes.iter().enumerate() {
            if k == 0 || k == last {
                ending
                    .entry(node.id)
                    .or_default()
                    .push((w, usize::from(k == last)));
            } else {
                through.insert(node.id);
            }
        }
    }
    let heading = |w: usize, e: usize| {
        let c = &cells[w];
        let reach = 6.min(c.len() - 1);
        let (from, to) = if e == 0 {
            (c[0], c[reach])
        } else {
            (c[c.len() - 1], c[c.len() - 1 - reach])
        };
        (f64::from(to.0 - from.0), f64::from(to.1 - from.1))
    };
    // Two ends meeting carry on into each other; where more meet, the straightest pair does.
    type End = (usize, usize);
    let mut pair: HashMap<End, End> = HashMap::new();
    for (node, here) in &ending {
        if through.contains(node) || here.len() < 2 {
            continue;
        }
        let mut best: Option<(f64, End, End)> = None;
        for (k, &p) in here.iter().enumerate() {
            for &q in &here[k + 1..] {
                if p.0 == q.0 {
                    continue;
                }
                let (a, b) = (heading(p.0, p.1), heading(q.0, q.1));
                let bend = 180.0
                    - (a.0 * b.1 - a.1 * b.0)
                        .atan2(a.0 * b.0 + a.1 * b.1)
                        .abs()
                        .to_degrees();
                let key = (bend, ways[p.0].id.min(ways[q.0].id));
                if best.is_none_or(|(bb, bp, bq)| key < (bb, ways[bp.0].id.min(ways[bq.0].id))) {
                    best = Some((bend, p, q));
                }
            }
        }
        if let Some((bend, p, q)) = best {
            if here.len() == 2 || bend <= 60.0 {
                pair.insert(p, q);
                pair.insert(q, p);
            }
        }
    }
    let runs = chains(ways.len(), |w, e| pair.get(&(w, e)).copied());
    let size: Vec<(usize, u64)> = runs
        .iter()
        .map(|run| {
            let len = run.iter().map(|&(w, _)| cells[w].len()).sum();
            (len, run.iter().map(|&(w, _)| ways[w].id).min().unwrap_or(0))
        })
        .collect();
    let nodes: Vec<Vec<u64>> = runs
        .iter()
        .map(|run| {
            run.iter()
                .flat_map(|&(w, _)| ways[w].nodes.iter().map(|n| n.id))
                .collect()
        })
        .collect();

    let wall_off: Vec<i32> = ways
        .iter()
        .map(|w| {
            let highway = w.tags.get("highway").map_or("", String::as_str);
            highway_block_range(highway, &w.tags, scale) + 1
        })
        .collect();
    // The ways meeting each tunnel way's end node: a platform, a path, an area's outline.
    let end_ids: HashSet<u64> = ways
        .iter()
        .flat_map(|w| [w.nodes[0].id, w.nodes[w.nodes.len() - 1].id])
        .collect();
    let mut meeting: HashMap<u64, HashSet<u64>> = HashMap::new();
    let mut meet = |way: &ProcessedWay| {
        for node in way.nodes.iter().filter(|n| end_ids.contains(&n.id)) {
            meeting.entry(node.id).or_default().insert(way.id);
        }
    };
    for element in elements {
        match element {
            ProcessedElement::Way(w) => meet(w),
            ProcessedElement::Relation(r) => r.members.iter().for_each(|m| meet(&m.way)),
            ProcessedElement::Node(_) => {}
        }
    }
    // Only a true dead end, like an emergency exit's door, leads nowhere.
    let leads_on = |node: &ProcessedNode| {
        meeting.get(&node.id).is_some_and(|w| w.len() > 1)
            || node.tags.get("entrance").is_some_and(|e| e != "emergency")
    };

    let floor = tunnel_min_road_y();
    let mut solved: HashMap<u64, i32> = HashMap::new();
    let mut beside = Beside::default();
    let mut out = HashMap::new();
    for r in planning_order(&size, &nodes) {
        let run = &runs[r];
        // The run's cells, each joint once, the way each cell is laid by, and where each starts.
        let mut path: Vec<(i32, i32)> = Vec::new();
        let mut owner: Vec<usize> = Vec::new();
        let mut pieces: Vec<(usize, usize, bool)> = Vec::new();
        for &(w, back) in run {
            let mut own = cells[w].clone();
            if back {
                own.reverse();
            }
            let skip = usize::from(path.last().is_some_and(|c| own.first() == Some(c)));
            pieces.push((w, path.len() - skip, back));
            path.extend_from_slice(&own[skip..]);
            owner.resize(path.len(), w);
        }
        let n = path.len();
        let at = |&(w, start, back): &(usize, usize, bool), j: usize| {
            if back {
                start + cells[w].len() - 1 - j
            } else {
                start + j
            }
        };
        let cover_drop: Vec<i32> = (0..n)
            .map(|i| TUNNEL_COVER_DROP.saturating_add(tunnel_layer_extra(&ways[owner[i]].tags)))
            .collect();
        let terrain: Vec<i32> = path
            .iter()
            .map(|&(x, z)| {
                editor
                    .terrain_level(x, z)
                    .unwrap_or_else(|| editor.get_ground_level(x, z))
            })
            .collect();
        let cover: Vec<i32> = (0..n)
            .map(|i| {
                let (x, z) = path[i];
                footprint_min_terrain(editor, x, z, wall_off[owner[i]], terrain[i])
            })
            .collect();
        // Under water the bore aims under the deepest bed across its footprint; the water
        // still counts as cover.
        let bed: Vec<Option<i32>> = (0..n)
            .map(|i| {
                let (x, z) = path[i];
                editor
                    .lowest_carved_bed(x, z, wall_off[owner[i]])
                    .filter(|&b| b < cover[i])
            })
            .collect();
        let ceiling: Vec<f64> = (0..n)
            .map(|i| f64::from(bed[i].unwrap_or(cover[i]) - cover_drop[i]))
            .collect();
        let mut pins: Vec<Option<i32>> = vec![None; n];
        // An open end is the portal, at the level the approach road comes down to.
        let portal = [0, n - 1].map(|i| !internal.contains(&path[i]));
        for (i, open) in [(0, portal[0]), (n - 1, portal[1])] {
            if open {
                pins[i] = Some((terrain[i] - portals.drop_at(path[i])).max(floor));
            }
        }
        // A tunnel already planned through a node of this one holds its level there.
        let mut held: Vec<(usize, i32)> = Vec::new();
        for piece in &pieces {
            let way = ways[piece.0];
            let mut j = 0;
            for (k, node) in way.nodes.iter().enumerate() {
                if k > 0 {
                    let (a, b) = (&way.nodes[k - 1], node);
                    j += (b.x - a.x).unsigned_abs().max((b.z - a.z).unsigned_abs()) as usize;
                }
                if let Some(&level) = solved.get(&node.id) {
                    let i = at(piece, j.min(cells[piece.0].len() - 1));
                    pins[i] = Some(level);
                    held.push((i, level));
                }
            }
        }
        // A branch that leads nowhere, like an emergency exit, too short to ramp up to its
        // portal from there ends in the rock.
        let end_node = |e: usize| {
            let &(w, _, back) = if e == 0 {
                &pieces[0]
            } else {
                &pieces[pieces.len() - 1]
            };
            let nodes = &ways[w].nodes;
            if (e == 0) == back {
                &nodes[nodes.len() - 1]
            } else {
                &nodes[0]
            }
        };
        for (e, i) in [(0, 0), (1, n - 1)] {
            if !portal[e] || leads_on(end_node(e)) || held.iter().any(|&(j, _)| j == i) {
                continue;
            }
            let ramp = held
                .iter()
                .map(|&(j, level)| level + (j.abs_diff(i) as i32) / TUNNEL_RAMP_RUN)
                .min();
            if let (Some(p), Some(r)) = (pins[i], ramp) {
                pins[i] = Some(p.min(r));
            }
        }
        // In One World both areas take the edge's level from one cell's terrain.
        pin_area_edges(&mut pins, &path, area, |i| {
            (cover[i] - cover_drop[i]).max(floor)
        });

        let depth = cover_drop
            .iter()
            .copied()
            .max()
            .unwrap_or(TUNNEL_COVER_DROP);
        let bore = Bore::new(
            n,
            depth,
            ROAD_TUNNEL_GRADE,
            1.0 / f64::from(TUNNEL_RAMP_RUN),
            floor,
        );
        // Never above the ground, nor under water above its cover.
        let top: Vec<f64> = (0..n)
            .map(|i| match bed[i] {
                Some(_) => ceiling[i],
                None => f64::from(terrain[i]),
            })
            .collect();
        let fixed: Vec<Option<f64>> = pins.iter().map(|p| p.map(f64::from)).collect();
        let target = grade_line(&ceiling, &top, &fixed, portal, &bore);
        // Beside a road already planned in the same bore, it keeps to that road's levels.
        let reach = 2 * run.iter().map(|&(w, _)| wall_off[w]).max().unwrap_or(1);
        let shared = beside.pins(&path, &target, reach, portal);
        let target = if shared.iter().any(Option::is_some) {
            let fixed: Vec<Option<f64>> =
                fixed.iter().zip(&shared).map(|(p, s)| p.or(*s)).collect();
            grade_line(&ceiling, &top, &fixed, portal, &bore)
        } else {
            target
        };
        let mut road_y: Vec<i32> = (0..n)
            .map(|i| (target[i].round() as i32).min(terrain[i]).max(floor))
            .collect();
        // Where pins sit too close for the grade, the carriageway still steps a block a cell.
        walkable(&mut road_y, &pins, TUNNEL_RAMP_STEP);
        let bare: Vec<bool> = (0..n)
            .map(|i| cover[i] - road_y[i] < TUNNEL_ROOF_COVER)
            .collect();
        let open = portal_cuttings(&bare, portal, CUTTING_GAP);
        let levels: Vec<f64> = road_y.iter().map(|&y| f64::from(y)).collect();
        beside.add(&path, &levels);

        for piece in &pieces {
            let (w, _, _) = *piece;
            let way = ways[w];
            let mut j = 0;
            for (k, node) in way.nodes.iter().enumerate() {
                if k > 0 {
                    let (a, b) = (&way.nodes[k - 1], node);
                    j += (b.x - a.x).unsigned_abs().max((b.z - a.z).unsigned_abs()) as usize;
                }
                solved.insert(node.id, road_y[at(piece, j.min(cells[w].len() - 1))]);
            }
            let len = cells[w].len();
            out.insert(
                way.id,
                RoadBore {
                    road_y: (0..len).map(|j| road_y[at(piece, j)]).collect(),
                    shut: (0..len)
                        .map(|j| bare[at(piece, j)] && !open[at(piece, j)])
                        .collect(),
                },
            );
        }
    }
    out
}

// Phase 1: place the tunnel shell and record cells; the interior is carved in phase 2.
// Returns false when there is no room to bore; the caller then renders at grade.
pub fn generate_highway_tunnel_shell(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    args: &Args,
    internal_endpoints: &TunnelInternalEndpoints,
    portals: &TunnelPortalMap,
    tunnel_cells: &mut Vec<HighwayTunnelCell>,
) -> bool {
    let Some(highway_type) = way.tags.get("highway") else {
        return false;
    };

    let pts = way_centerline(way);
    if pts.len() < 2 {
        return false;
    }
    let n = pts.len();
    let last = n - 1;

    let half_width = highway_block_range(highway_type, &way.tags, args.scale);
    let wall_off = half_width + 1;

    // Raw DEM keeps road_y identical when a way is reprocessed across tiles.
    // `cover_ys` is the lowest ground the footprint touches; it decides the roof.
    let (terrain_ys, cover_ys) = tunnel_cover_profile(editor, &pts, wall_off);

    let floor = tunnel_min_road_y();
    // No room anywhere: a road at grade beats a brick pit.
    if cover_ys.iter().all(|&c| c - floor < TUNNEL_ROOF_COVER) {
        return false;
    }

    let lone;
    let bore = match portals.bores.get(&way.id) {
        Some(bore) => bore,
        None => {
            lone = plan_road_tunnels(
                &[ProcessedElement::Way(way.clone())],
                editor,
                internal_endpoints,
                portals,
                args.scale,
                None,
            );
            match lone.get(&way.id) {
                Some(bore) => bore,
                None => return false,
            }
        }
    };
    if bore.road_y.len() != n {
        return false;
    }
    let road_y = &bore.road_y;
    let covered: Vec<bool> = (0..n)
        .map(|i| bore.shut[i] || cover_ys[i] - road_y[i] >= TUNNEL_ROOF_COVER)
        .collect();

    // As above ground.
    let palette = surface_palette(highway_type, &way.tags);
    let faces = tunnel_portal_faces(&pts, internal_endpoints);

    for i in 0..n {
        let (bx, bz) = pts[i];
        let ry = road_y[i];
        let ceil_y = ry + TUNNEL_CEIL_OFFSET;
        let faces = faces_near(faces, i, last, wall_off);

        // Roof and placeholder are both stone brick, so clamp instead of whitelisting.
        let carve_top = if covered[i] {
            ceil_y - 1
        } else {
            let lo = i.saturating_sub(half_width as usize);
            let hi = (i + half_width as usize).min(last);
            (lo..=hi)
                .filter(|&j| covered[j])
                .map(|j| road_y[j] + TUNNEL_CEIL_OFFSET - 1)
                .fold(terrain_ys[i], i32::min)
        };
        // Never stamp what the carve will not clear, or it stands in the grass.
        let top = if covered[i] { ceil_y } else { carve_top };

        // Square footprint like the subway; the road row is a placeholder laid in phase 2.
        // Each column stops under its own grade, else the masonry outlines the trench.
        for dx in -wall_off..=wall_off {
            for dz in -wall_off..=wall_off {
                if beyond_portal(&faces, bx + dx, bz + dz) {
                    continue;
                }
                let is_side_wall = dx.abs() == wall_off || dz.abs() == wall_off;
                let col_ty = editor
                    .terrain_level(bx + dx, bz + dz)
                    .unwrap_or(terrain_ys[i]);
                // The foundation carries the carriageway even where the ground fell away.
                let carries_road = dx.abs() <= half_width && dz.abs() <= half_width;
                // Where the ground dips over the bore away from a portal, it stays shut.
                let col_top = if bore.shut[i] {
                    top
                } else if carries_road {
                    top.min(col_ty - 1).max(ry - 1)
                } else {
                    top.min(col_ty - 1)
                };
                for y in (ry - 1)..=col_top {
                    let block = if is_side_wall || (covered[i] && y == ceil_y) {
                        tunnel_shell_block(bx + dx, y, bz + dz)
                    } else {
                        STONE_BRICKS // foundation, road row, and interior placeholder
                    };
                    editor.set_block_absolute(block, bx + dx, y, bz + dz, None, None);
                }
            }
        }
        tunnel_cells.push(HighwayTunnelCell {
            x: bx,
            z: bz,
            road_y: ry,
            half_width,
            covered: covered[i],
            carve_top,
            palette,
            light: covered[i] && i.is_multiple_of(TUNNEL_LIGHT_INTERVAL),
            faces,
        });
    }
    true
}

/// Unit step pointing from `inner` away from `from`, i.e. out of the bore.
pub(crate) fn step_towards(from: (i32, i32), inner: (i32, i32)) -> (i32, i32) {
    ((from.0 - inner.0).signum(), (from.1 - inner.1).signum())
}

/// Where an open end cuts the structure off, else the roof overhangs the approach.
#[derive(Clone, Copy)]
pub(crate) struct PortalFace {
    pub(crate) at: (i32, i32),
    /// Step out of the bore.
    pub(crate) out: (i32, i32),
}

/// The faces that clip cell `i` of a way ending at `last`, `wall` wide: only those near it, as
/// a tunnel turning back on itself passes a face's plane further in.
pub(crate) fn faces_near(
    faces: [Option<PortalFace>; 2],
    i: usize,
    last: usize,
    wall: i32,
) -> [Option<PortalFace>; 2] {
    let reach = 2 * wall as usize + 2;
    [
        faces[0].filter(|_| i <= reach),
        faces[1].filter(|_| last - i <= reach),
    ]
}

#[inline]
pub(crate) fn beyond_portal(faces: &[Option<PortalFace>; 2], cx: i32, cz: i32) -> bool {
    faces
        .iter()
        .flatten()
        .any(|f| (cx - f.at.0) * f.out.0 + (cz - f.at.1) * f.out.1 > 0)
}

/// The open ends of a tunnel way, as clipping planes.
fn tunnel_portal_faces(
    pts: &[(i32, i32)],
    internal: &TunnelInternalEndpoints,
) -> [Option<PortalFace>; 2] {
    let last = pts.len() - 1;
    [
        (!internal.contains(&pts[0])).then(|| PortalFace {
            at: pts[0],
            out: step_towards(pts[0], pts[1]),
        }),
        (!internal.contains(&pts[last])).then(|| PortalFace {
            at: pts[last],
            out: step_towards(pts[last], pts[last - 1]),
        }),
    ]
}

/// A tunnel light to hang under whatever roof stands over (x, z) between `lo` and `hi`.
pub struct TunnelLight {
    pub x: i32,
    pub z: i32,
    pub lo: i32,
    pub hi: i32,
}

/// Hangs tunnel lights once every bore is carved, since one carved later, such as a road
/// tunnel across a rail one, can open the roof a light was hung from. A tile hangs only its
/// own, so no copy from its halo hangs beside or under one.
pub fn hang_tunnel_lights(editor: &mut WorldEditor, lights: &[TunnelLight]) {
    for light in lights {
        if !editor.owns(light.x, light.z) {
            continue;
        }
        let roof =
            (light.lo..=light.hi).find(|&y| editor.block_exists_absolute(light.x, y, light.z));
        // A repeat finds the light already hung, not the roof.
        let lit = |y: i32| {
            editor.check_for_block_absolute(light.x, y, light.z, Some(&[SEA_LANTERN]), None)
        };
        if let Some(y) = roof.filter(|&y| !lit(y)) {
            editor.set_block_absolute(SEA_LANTERN, light.x, y - 1, light.z, None, None);
        }
    }
}

// Phase 2: carve the interior, then lay the road last so no carve can eat it. Returns the
// lights to hang once every tunnel is carved.
pub fn carve_highway_tunnel_interior(
    editor: &mut WorldEditor,
    tunnel_cells: &[HighwayTunnelCell],
) -> Vec<TunnelLight> {
    // What the bore may swallow: its placeholder, ground fill, approach paving, cut surfaces.
    const CARVE_WL: &[Block] = &[
        STONE_BRICKS,
        CRACKED_STONE_BRICKS,
        MOSSY_STONE_BRICKS,
        STONE,
        WATER,
        GRAY_CONCRETE_POWDER,
        CYAN_TERRACOTTA,
        GRAY_CONCRETE,
        BLACK_CONCRETE,
        LIGHT_GRAY_CONCRETE,
        WHITE_CONCRETE,
        YELLOW_CONCRETE,
        POLISHED_ANDESITE,
        SMOOTH_STONE,
        GRASS_BLOCK,
        DIRT,
        COARSE_DIRT,
        PODZOL,
        MUD,
        CLAY,
        SAND,
        SANDSTONE,
        GRAVEL,
        ANDESITE,
        COBBLESTONE,
        TUFF,
        DEEPSLATE,
        SNOW_BLOCK,
        SNOW_LAYERS[0],
        SNOW_LAYERS[1],
        SNOW_LAYERS[2],
        SNOW_LAYERS[3],
        SNOW_LAYERS[4],
        SNOW_LAYERS[5],
        SNOW_LAYERS[6],
        MOSS_BLOCK,
        FARMLAND,
    ];
    // Whitelist for laying the floor over carved air, placeholder, or fill stone.
    // SEA_LANTERN so a crossing bore's light is paved over, not embedded.
    const ROAD_WL: &[Block] = &[
        AIR,
        STONE,
        STONE_BRICKS,
        CRACKED_STONE_BRICKS,
        MOSSY_STONE_BRICKS,
        WATER,
        SEA_LANTERN,
    ];

    for cell in tunnel_cells {
        let hw = cell.half_width;
        // Stop at the portal; past it carving leaves a gap the bore cannot repave.
        for dx in -hw..=hw {
            for dz in -hw..=hw {
                let (cx, cz) = (cell.x + dx, cell.z + dz);
                if beyond_portal(&cell.faces, cx, cz) {
                    continue;
                }
                // Open cuts follow their own column's grade, not the centerline's.
                let top = if cell.covered {
                    cell.carve_top
                } else {
                    cell.carve_top
                        .min(editor.terrain_level(cx, cz).unwrap_or(cell.carve_top))
                };
                for y in (cell.road_y + 1)..=top {
                    editor.set_block_absolute(AIR, cx, y, cz, Some(CARVE_WL), None);
                }
            }
        }
    }

    for cell in tunnel_cells {
        let hw = cell.half_width;
        for dx in -hw..=hw {
            for dz in -hw..=hw {
                let (cx, cz) = (cell.x + dx, cell.z + dz);
                if beyond_portal(&cell.faces, cx, cz) {
                    continue;
                }
                let surf = semirandom_surface(cx, cz, cell.palette);
                editor.set_block_absolute(surf, cx, cell.road_y, cz, Some(ROAD_WL), None);
            }
        }
    }

    tunnel_cells
        .iter()
        .filter(|c| c.light)
        .map(|c| TunnelLight {
            x: c.x,
            z: c.z,
            lo: c.road_y + 3,
            hi: c.road_y + TUNNEL_CEIL_OFFSET + 2,
        })
        .collect()
}

// Tunnel-bore footprint, to keep the water depth-carve and vegetation off it.
pub fn collect_tunnel_footprint(
    elements: &[ProcessedElement],
    editor: &WorldEditor,
    internal: &TunnelInternalEndpoints,
    xzbbox: &XZBBox,
    scale: f64,
) -> CoordinateBitmap {
    if !elements
        .iter()
        .any(|e| matches!(e, ProcessedElement::Way(w) if renders_as_highway_tunnel(w)))
    {
        return CoordinateBitmap::new_empty();
    }
    let mut bitmap = CoordinateBitmap::new(xzbbox);
    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };
        // A way with no room to bore renders at grade and has nothing to protect.
        if !renders_as_highway_tunnel(way) || !tunnel_bore_fits(editor, way, scale) {
            continue;
        }
        let Some(highway_type) = way.tags.get("highway") else {
            continue;
        };
        let wall = highway_block_range(highway_type, &way.tags, scale) + 1;
        let pts = way_centerline(way);
        if pts.len() < 2 {
            continue;
        }
        // Clipped as the shell is, else the ground pass skips a band with no roof.
        let faces = tunnel_portal_faces(&pts, internal);
        let last = pts.len() - 1;
        for (i, &(bx, bz)) in pts.iter().enumerate() {
            let faces = faces_near(faces, i, last, wall);
            for dx in -wall..=wall {
                for dz in -wall..=wall {
                    if beyond_portal(&faces, bx + dx, bz + dz) {
                        continue;
                    }
                    bitmap.set(bx + dx, bz + dz);
                }
            }
        }
    }
    bitmap
}

/// Columns around a lamp no second one stands in, as on a road and its sidewalk both lit.
const LAMP_NEIGHBOUR_REACH: i32 = 3;

fn place_street_lamp(editor: &mut WorldEditor, x: i32, z: i32, base: i32) {
    let r = LAMP_NEIGHBOUR_REACH;
    let crowded = (-r..=r).any(|dx| {
        (-r..=r).any(|dz| {
            (base + 2..=base + 9).any(|y| {
                editor.check_for_block_absolute(x + dx, y, z + dz, Some(&[REDSTONE_LAMP]), None)
            })
        })
    });
    if crowded {
        return;
    }
    editor.register_lamp(x, z);
    editor.set_block_absolute(SMOOTH_STONE, x, base + 1, z, None, None);
    for dy in 2..=4 {
        editor.set_block_absolute(STONE_BRICK_WALL, x, base + dy, z, None, None);
    }
    editor.set_block_with_properties_absolute(
        BlockWithProperties::new(REDSTONE_LAMP, Some(fastnbt::nbt!({ "lit": "true" }))),
        x,
        base + 5,
        z,
        None,
        None,
    );
    editor.set_block_absolute(IRON_TRAPDOOR, x, base + 6, z, None, None);
}

/// Takes down lamps the water fill reached, as on a quay edge a river polygon overclaims.
/// Lamps go up before the water, so only afterwards does it show. Halo lamps go too, or
/// the merge would put a neighbour's copy back where the owner took its own down.
pub fn drop_lamps_in_water(editor: &mut WorldEditor) {
    const PARTS: &[Block] = &[SMOOTH_STONE, STONE_BRICK_WALL, REDSTONE_LAMP, IRON_TRAPDOOR];
    for (x, z) in editor.take_lamps() {
        let is = |ed: &WorldEditor, y: i32, blocks: &[Block]| {
            ed.check_for_block_absolute(x, y, z, Some(blocks), None)
        };
        let ground = editor.get_ground_level(x, z);
        let Some(head) = (ground + 2..=ground + 12)
            .find(|&y| is(editor, y, &[REDSTONE_LAMP]) && is(editor, y - 1, &[STONE_BRICK_WALL]))
        else {
            continue;
        };
        // A still surface over the ground swallows the plinth, so walk down the post.
        let mut foot = head - 1;
        while foot > head - 4 && is(editor, foot - 1, &[STONE_BRICK_WALL, SMOOTH_STONE]) {
            foot -= 1;
        }
        if is(editor, foot - 1, &[WATER]) {
            for y in foot..=head + 1 {
                editor.set_block_absolute(AIR, x, y, z, Some(PARTS), None);
            }
        }
    }
}

const WAY_LAMP_INTERVAL: usize = 25;

// Periodic lamps beside lit=yes ways, alternating sides, kept off other roads and water.
fn place_way_lamps(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    block_range: i32,
    road_mask: &RoadMaskBitmap,
) {
    let offset = block_range + 2;
    let mut tds: usize = 0;
    let mut side = 1i32;
    for w in way.nodes.windows(2) {
        let (dx, dz) = (w[1].x - w[0].x, w[1].z - w[0].z);
        let len = dx.abs().max(dz.abs());
        if len == 0 {
            continue;
        }
        let mag = (dx as f64).hypot(dz as f64);
        let (px, pz) = (
            (-dz as f64 / mag).round() as i32,
            (dx as f64 / mag).round() as i32,
        );
        for (bx, _, bz) in bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z) {
            if tds > 0 && tds.is_multiple_of(WAY_LAMP_INTERVAL) {
                for s in [side, -side] {
                    let (lx, lz) = (bx + px * offset * s, bz + pz * offset * s);
                    if !road_mask.contains(lx, lz) && !editor.is_lc_water(lx, lz) {
                        let base = editor.get_absolute_y(lx, 0, lz);
                        place_street_lamp(editor, lx, lz, base);
                        side = -side;
                        break;
                    }
                }
            }
            tds += 1;
        }
    }
}

/// Build a connectivity map for highway endpoints to determine where slopes are needed.
pub fn build_highway_connectivity_map(elements: &[ProcessedElement]) -> HighwayConnectivityMap {
    let mut ends: HashMap<(i32, i32), Vec<i32>> = HashMap::new();
    let mut points: Vec<u64> = Vec::new();
    let mut own: Vec<u64> = Vec::new();

    for element in elements {
        if let ProcessedElement::Way(way) = element {
            if way.tags.contains_key("highway") {
                let layer_value = way
                    .tags
                    .get("layer")
                    .and_then(|layer| layer.parse::<i32>().ok())
                    .unwrap_or(0);

                // Treat negative layers as ground level (0) for connectivity
                let layer_value = if layer_value < 0 { 0 } else { layer_value };

                // Add connectivity for start and end nodes
                if let (Some(start_node), Some(end_node)) = (way.nodes.first(), way.nodes.last()) {
                    ends.entry((start_node.x, start_node.z))
                        .or_default()
                        .push(layer_value);
                    ends.entry((end_node.x, end_node.z))
                        .or_default()
                        .push(layer_value);
                }

                // Each way counts once per point, so a loop's closing node isn't shared.
                own.clear();
                own.extend(way.nodes.iter().map(|n| pack_xz(n.x, n.z)));
                own.sort_unstable();
                own.dedup();
                points.extend_from_slice(&own);
            }
        }
    }

    points.sort_unstable();
    let mut shared: Vec<u64> = points
        .windows(2)
        .filter(|pair| pair[0] == pair[1])
        .map(|pair| pair[0])
        .collect();
    shared.dedup();
    shared.shrink_to_fit();
    HighwayConnectivityMap {
        ends,
        shared,
        area: None,
    }
}

/// Internal function that generates highways with connectivity context for elevation handling
#[allow(clippy::too_many_arguments)]
fn generate_highways_internal(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    highway_connectivity: &HighwayConnectivityMap,
    flood_fill_cache: &FloodFillCache,
    road_mask: &RoadMaskBitmap,
    bridge_structures: &BridgeStructureMap,
    bridge_surface: &BridgeSurfaceMap,
    tunnel_portals: &TunnelPortalMap,
    tunnel_footprint: &CoordinateBitmap,
    markings: &RoadMarkingIndex,
    building_footprints: &CoordinateBitmap,
) {
    // Shared `indoor=yes` / layer parsing for the whole function. Indoor
    // highways must never produce elevated geometry (they sit inside
    // buildings), and features like street lamps on an explicit
    // `layer=*` stand on the bridge deck they belong to.
    let is_indoor = element.tags().get("indoor").is_some_and(|v| v == "yes");
    let layer_value_raw = element
        .tags()
        .get("layer")
        .and_then(|layer| layer.parse::<i32>().ok())
        .unwrap_or(0);
    // Negative layers map to ground level: undergrounds are out of
    // scope and their markers shouldn't sink below terrain.
    let layer_value_effective = if is_indoor || layer_value_raw < 0 {
        0
    } else {
        layer_value_raw
    };
    const LAYER_HEIGHT_STEP: i32 = 6;
    let layer_boost = layer_value_effective * LAYER_HEIGHT_STEP;
    let layered = layer_value_effective > 0;
    let signal_base = |ed: &WorldEditor, x: i32, z: i32, radius: i32| {
        node_feature_base_y(ed, bridge_surface, x, z, layered, radius)
    };

    if let Some(highway_type) = element.tags().get("highway") {
        if highway_type == "street_lamp" {
            if let ProcessedElement::Node(first_node) = element {
                let mut x: i32 = first_node.x;
                let mut z: i32 = first_node.z;
                let mut curb: Option<i32> = None;
                // One mapped off the deck, on a sidewalk it leaves out, stands on its railing.
                if !bridge_surface.contains(x, z) && (layered || editor.is_mapped_water(x, z)) {
                    if let Some((cell, y)) =
                        bridge_surface.nearest_railing(x, z, LAYERED_LAMP_DECK_REACH)
                    {
                        ((x, z), curb) = (cell, Some(y));
                    } else if let Some((cell, _)) =
                        bridge_surface.nearest_deck_cell(x, z, LAYERED_LAMP_DECK_REACH)
                    {
                        (x, z) = cell;
                    }
                }
                let base = curb.unwrap_or_else(|| {
                    node_feature_base_y(editor, bridge_surface, x, z, layered, 0)
                });
                place_street_lamp(editor, x, z, base);
            }
        } else if highway_type == "crossing" || highway_type == "traffic_signals" {
            // Signal heads for signalised crossings and standalone traffic_signals nodes.
            let signalised = highway_type == "traffic_signals"
                || element.tags().get("crossing").map(String::as_str) == Some("traffic_signals");
            let signals = editor.traffic_signals();
            let placed = match (signalised, element, &signals) {
                (true, ProcessedElement::Node(node), Some(signals)) => {
                    signals.place(editor, node, &signal_base)
                }
                _ => false,
            };
            // On Java every signal is planned; one the plan found no kerb for gets no head,
            // rather than a banner one beside the others. Other formats keep the banners.
            if signalised && !placed && signals.is_none() {
                if let ProcessedElement::Node(node) = element {
                    let x = node.x;
                    let z = node.z;
                    let head_base = node_feature_base_y(editor, bridge_surface, x, z, layered, 0);

                    // Try to build a hanging signal if it's on a road
                    let anchor = road_mask
                        .contains(x, z)
                        .then(|| get_nearest_non_road_block(x, z, 4, road_mask))
                        .flatten();

                    match anchor {
                        Some((ax, az)) => {
                            // The deck edge (rail cell) still holds the pole; further out it
                            // stands on the terrain.
                            let pole_base =
                                node_feature_base_y(editor, bridge_surface, ax, az, layered, 1);
                            editor.set_block_absolute(
                                COBBLESTONE_WALL,
                                ax,
                                pole_base + 1,
                                az,
                                None,
                                None,
                            );
                            // The mast carries the arm, so it has to reach the arm's level.
                            let arm_y = head_base + 6;
                            for y in (pole_base + 2)..arm_y {
                                editor.set_block_absolute(IRON_BARS, ax, y, az, None, None);
                            }

                            // One level for the whole arm, so its bars are true neighbours and
                            // can be joined; a per-cell terrain height would step them apart.
                            let arm: Vec<(i32, i32)> = four_connected_line(
                                &bresenham_line(x, 0, z, ax, 0, az)
                                    .into_iter()
                                    .map(|(lx, _, lz)| (lx, lz))
                                    .collect::<Vec<_>>(),
                            );
                            let arm_cells: HashSet<(i32, i32)> = arm.iter().copied().collect();
                            for &(lx, lz) in &arm {
                                let joins =
                                    |dx: i32, dz: i32| arm_cells.contains(&(lx + dx, lz + dz));
                                editor.set_block_with_properties_absolute(
                                    connected_iron_bars(
                                        joins(0, -1),
                                        joins(0, 1),
                                        joins(1, 0),
                                        joins(-1, 0),
                                    ),
                                    lx,
                                    arm_y,
                                    lz,
                                    None,
                                    None,
                                );
                            }
                        }
                        None => {
                            editor.set_block_absolute(
                                COBBLESTONE_WALL,
                                x,
                                head_base + 1,
                                z,
                                None,
                                None,
                            );
                            editor.set_block_absolute(IRON_BARS, x, head_base + 2, z, None, None);
                            editor.set_block_absolute(IRON_BARS, x, head_base + 3, z, None, None);
                        }
                    }

                    editor.set_block_absolute(BLACK_WOOL, x, head_base + 4, z, None, None);
                    editor.set_block_absolute(BLACK_WOOL, x, head_base + 5, z, None, None);

                    const BANNER_PATTERNS: &[(&str, &str)] = &[
                        ("red", "minecraft:triangle_top"),
                        ("lime", "minecraft:triangle_bottom"),
                        ("yellow", "minecraft:circle"),
                        ("black", "minecraft:curly_border"),
                        ("black", "minecraft:border"),
                    ];

                    let banner_y = head_base + 5;
                    let banner_offsets: [(i32, i32, &str); 4] = [
                        (0, -1, "north"),
                        (0, 1, "south"),
                        (-1, 0, "west"),
                        (1, 0, "east"),
                    ];
                    for (dx, dz, facing) in &banner_offsets {
                        editor.place_wall_banner(
                            LIGHT_GRAY_WALL_BANNER,
                            x + dx,
                            banner_y,
                            z + dz,
                            facing,
                            "light_gray",
                            BANNER_PATTERNS,
                        );
                    }
                }
            }
        } else if highway_type == "bus_stop" {
            if let ProcessedElement::Node(node) = element {
                let x = node.x;
                let z = node.z;
                let base = node_feature_base_y(editor, bridge_surface, x, z, layered, 0);
                for dy in 1..=3 {
                    editor.set_block_absolute(COBBLESTONE_WALL, x, base + dy, z, None, None);
                }

                editor.set_block_absolute(WHITE_WOOL, x, base + 4, z, None, None);
                let neighbor_base =
                    node_feature_base_y(editor, bridge_surface, x + 1, z, layered, 1);
                editor.set_block_absolute(WHITE_WOOL, x + 1, neighbor_base + 4, z, None, None);

                // Bus sign on both broad faces of the overhanging wool, stop name on the pole block.
                crate::element_processing::signage::place_bus_stop_signs(
                    editor,
                    &node.tags,
                    x,
                    neighbor_base + 4,
                    z,
                );
            }
        } else if element
            .tags()
            .get("area")
            .is_some_and(|v: &String| v == "yes")
        {
            let ProcessedElement::Way(way) = element else {
                return;
            };

            // Handle areas like pedestrian plazas. Unified surface handling
            // via the shared surfaces module.
            let fallback: &[Block] = if highway_type == "raceway" {
                surface_palette(highway_type, &way.tags)
            } else {
                &[STONE]
            };
            let surface_block: Block = pedestrian_paving(&way.tags)
                .unwrap_or_else(|| get_blocks_for_surface_way(way, fallback))[0];

            // Fill the area using flood fill cache
            let filled_area = flood_fill_cache.get_or_compute(way, args.timeout.as_ref());

            for &(x, z) in filled_area.iter() {
                if !editor.nested_area_owns(x, z) {
                    editor.set_block(surface_block, x, 0, z, None, None);
                }
            }
        } else {
            let mut previous_node: Option<(i32, i32)> = None;
            let scale_factor = args.scale;

            // Reuse the function-level layer resolution (already normalised
            // to 0 for indoor/negative).
            let layer_value = layer_value_effective;

            // Skip if 'level' is negative in the tags (indoor mapping)
            if let Some(level) = element.tags().get("level") {
                if level.parse::<i32>().unwrap_or(0) < 0 {
                    return;
                }
            }

            let ProcessedElement::Way(way) = element else {
                return;
            };

            let bridge_member = bridge_structures.lookup_member(way.id);
            let bridge_ramp = bridge_structures.lookup_ramp(way.id);
            // A platform or stop drawn on a bridge is no deck of its own.
            if bridge_member.is_none() && is_bridge_way(way) && !lays_deck(way) {
                return;
            }
            let is_bridge_member = bridge_member.is_some();
            let is_bridge_ramp = bridge_ramp.is_some();
            let bridge_style = bridge_member.map(|m| m.style).unwrap_or(BridgeStyle::Beam);
            let bridge_start_is_boundary = bridge_member
                .map(|m| m.start_is_group_boundary)
                .unwrap_or(true);
            let bridge_end_is_boundary = bridge_member
                .map(|m| m.end_is_group_boundary)
                .unwrap_or(true);
            let bridge_design = bridge_member.map(|m| bridge_structures.structure_of(m).design);
            let bridge_foundation_block = bridge_design.map_or(STONE_BRICKS, |d| d.slab());
            // An approach ramp is built like the bridge it climbs to.
            let ramp_design = bridge_ramp.map(|r| r.design);
            let ramp_wall = ramp_design.map_or(STONE_BRICKS, |d| d.slab());

            // Surface palette; width is resolved by highway_block_range below so
            // renderer and prescan stay in sync.
            let block_types = surface_palette(highway_type, &way.tags);
            // The strip the way is paved out over to the road beside it, laid at the level of
            // the cell of its own each cell reaches out from.
            let kerb_strips = editor.kerb_strips();
            let strip = kerb_strips.as_ref().map_or(&[][..], |k| k.of(way.id));
            let mut levels: FnvHashMap<(i32, i32), (i32, bool)> = FnvHashMap::default();
            let paves_like_a_road = paves_like_road(highway_type, &way.tags);
            let surface_protect = if block_types == PATH_ASPHALT {
                CAR_FREE_PROTECTED_SURFACES
            } else if highway_type == "cycleway" && cycleway_palette(&way.tags).is_some() {
                OVER_ASPHALT_PROTECTED_SURFACES
            } else if is_walkway(highway_type) && !paves_like_a_road {
                WALKWAY_PROTECTED_SURFACES
            } else {
                ROAD_PROTECTED_SURFACES
            };
            // Off the lanes a sidewalk stops at a path's asphalt, and in path asphalt at the
            // grey too; a crossing paves over both.
            let sidewalk_protect = if paves_like_a_road {
                OVER_ASPHALT_PROTECTED_SURFACES
            } else if block_types == PATH_ASPHALT {
                ASPHALT_SIDEWALK_PROTECTED_SURFACES
            } else {
                SIDEWALK_PROTECTED_SURFACES
            };
            // A sidewalk, or a crossing out to the lanes, is drawn over a road's parking row
            // though not over its lanes, so the two meet at the kerb whichever is built first.
            let is_sidewalk = matches!(
                way.tags.get("footway").map(String::as_str),
                Some("sidewalk" | "crossing")
            );

            // Canonical width (shared with prescan/bridge consumers).
            let block_range = highway_block_range(highway_type, &way.tags, scale_factor);
            // The lanes the paint divides, inside any parking rows.
            let lane_range = highway_lane_range(highway_type, &way.tags, scale_factor);
            // Decks keep the square their railings are laid out from; the masks read the same.
            let square_stamp = stamps_square(way);
            // The rows of cars parked across the kerb or at an angle, marked a stall apart from
            // the kerb to the lane edge: the side, +1 right of the way or -1 left. A row along
            // the kerb stays bare, as most are.
            let stall_rows: Vec<f32> = if is_carriageway(highway_type) {
                [("right", 1.0f32), ("left", -1.0)]
                    .into_iter()
                    .filter(|&(side, _)| {
                        road_width::parking_orientation(&way.tags, side)
                            .is_some_and(|o| o != "parallel")
                    })
                    .map(|(_, sign)| sign)
                    .collect()
            } else {
                Vec::new()
            };
            // Below a block a metre a stall's lines would run together into a dashed band.
            let stall_period = (STALL_M * scale_factor).round() as i64;
            // Nobody parks across a junction's mouth.
            let junction_nodes: Vec<(i32, i32)> = way
                .nodes
                .iter()
                .filter(|n| editor.is_junction(n.x, n.z))
                .map(|n| (n.x, n.z))
                .collect();
            // How far the road paves into a building standing against it, or a verge.
            let kept_by_walls = wall_core(way, highway_type, scale_factor);
            // Whether the stamp of the centreline cell (dx, dz) back leaves (x, z) unpaved.
            // Rows of parked cars tagged on it are the road's own, which no verge takes back.
            let verges = !road_width::parks_along(&way.tags);
            // The walls on the ground its strip stops at, as the masks read them.
            let wall_footprints = editor.wall_footprints();
            let walls: &CoordinateBitmap =
                wall_footprints.as_deref().unwrap_or(building_footprints);
            // How far its kerb reaches either side, parked cars widening their own side only.
            let sides = road_width::kerb_sides(highway_type, &way.tags, scale_factor);
            // Whether a cell beside the core stops the road is the cell's own, and the lines
            // out from every stamp within reach of it ask: the first one's answer is kept for
            // the rest.
            let cut_memo = std::cell::RefCell::new(CutMemo::default());
            let cut_here = |editor: &WorldEditor, dx: i32, dz: i32, x: i32, z: i32| {
                kept_by_walls.is_some_and(|core| {
                    cut_outward(core, dx, dz, |cx, cz| {
                        let (ax, az) = (x - dx + cx, z - dz + cz);
                        cut_memo.borrow_mut().get_or(ax, az, || {
                            cut_beside(
                                |sx, sz| near_centreline(way, core, sx, sz),
                                walls,
                                Some(editor),
                                |px, pz| editor.on_mapped_path(px, pz),
                                verges,
                                ax,
                                az,
                            )
                        })
                    })
                })
            };
            let race_paint = (highway_type == "raceway").then(|| markings.raceway_paint());
            let race_deck = raceways::deck(&way.tags);

            // `--water-detail scaled`: the water cells the road mask dropped from a short
            // crossing are left to the carve.
            let drown_over_water = !road_mask.is_empty()
                && surface_highway_type(way).is_some()
                && args
                    .water
                    .water_detail
                    .drowns_crossing(scale_factor, &way.tags);

            // At-grade lit ways get periodic street lamps alongside, but a blueprint has no furniture.
            if way.tags.get("lit").map(String::as_str) == Some("yes")
                && args.mode != GenerationMode::Blueprint
                && !is_bridge_member
                && !is_bridge_ramp
                && !is_indoor
                && layer_value_effective == 0
                && highway_type != "steps"
                && highway_type != "construction"
            {
                place_way_lamps(editor, way, block_range, road_mask);
            }

            // Lane lines; lane_markings=no drops them, not the width. A tunnel with no room
            // to bore runs at grade across the streets above it, so it stays unpainted.
            let lines = if renders_as_highway_tunnel(way) {
                Vec::new()
            } else {
                let road_width = 2 * lane_range + 1;
                let oneway = road_markings::oneway_sign(highway_type, &way.tags) != 0;
                lane_lines(highway_type, &way.tags, lane_range, markings)
                    .into_iter()
                    .filter(|l| {
                        args.road_detail.keeps_lane_line(
                            highway_type,
                            &way.tags,
                            road_width,
                            l.centre,
                            oneway,
                        )
                    })
                    .collect()
            };
            let (dash_on, dash_period) =
                args.road_detail
                    .dash_pattern(dash_pattern(highway_type, &way.tags, scale_factor));
            let way_marks = if renders_as_highway_tunnel(way) {
                None
            } else {
                markings.way(way.id)
            };
            // A line across one half of the road stops beside the centre line.
            let centre_span = lines.iter().filter(|l| l.centre).fold(None, |span, l| {
                let (lo, hi) = span.unwrap_or((l.offset, l.offset));
                Some((lo.min(l.offset), hi.max(l.offset)))
            });

            // Elevation based on layer (already normalised; `LAYER_HEIGHT_STEP`
            // is defined at the top of this function).
            let base_elevation = layer_boost;

            // Check if we need slopes at start and end
            // This is used for overpasses that need ramps to ground-level roads
            let needs_start_slope =
                should_add_slope_at_node(&way.nodes[0], layer_value, highway_connectivity);
            let needs_end_slope = should_add_slope_at_node(
                &way.nodes[way.nodes.len() - 1],
                layer_value,
                highway_connectivity,
            );

            let total_way_length = calculate_way_length(way);

            // Unique bresenham points; sum of max per segment + 1 (no shared-endpoint double count).
            let total_bresenham_length: usize = way
                .nodes
                .windows(2)
                .map(|pair| {
                    let dx = (pair[1].x - pair[0].x).unsigned_abs() as usize;
                    let dz = (pair[1].z - pair[0].z).unsigned_abs() as usize;
                    dx.max(dz)
                })
                .sum::<usize>()
                + 1;

            // Descend into the portal; a pure function of way geometry, so tile-stable.
            let tunnel_approach = if tunnel_portals.is_empty() || is_bridge_member || is_bridge_ramp
            {
                None
            } else {
                tunnel_portals.approach(way.id)
            };

            // Cable carriers hang from pylons; other members stand on piers.
            let bridge_pylons = bridge_member.and_then(|m| m.cable_pylons.as_deref());
            let bridge_cables_carry_deck = bridge_pylons.is_some();
            // Under mapped towers only the suspended stretch hangs, and it is painted.
            let bridge_cable_span = bridge_member.and_then(|m| m.cable_span.as_deref());
            let hangs_at = |x: i32, z: i32| match bridge_cable_span {
                Some(span) => span.suspends(x, z),
                None => bridge_cables_carry_deck,
            };
            let bridge_paint = bridge_cable_span.map(|s| s.paint);

            let is_short_isolated_elevated = !is_bridge_member
                && !is_bridge_ramp
                && needs_start_slope
                && needs_end_slope
                && layer_value > 0
                && total_way_length <= 35;

            let (effective_elevation, effective_start_slope, effective_end_slope) =
                if is_bridge_member || is_bridge_ramp || is_short_isolated_elevated {
                    (0, false, false)
                } else {
                    (base_elevation, needs_start_slope, needs_end_slope)
                };

            let slope_length = (total_way_length as f32 * 0.35).clamp(15.0, 50.0) as usize;

            // Painted pedestrian crossing, kept to the carriageways it runs over.
            let crossing_paint = markings.crossing_way_paint(way);
            let crossing_clips = crossing_paint.and(markings.crossing_clips(way.id));
            // Surface of the carriageway under a crossing cell; None off the road, and
            // everywhere for a crossing that shares no node with a road. Every stamp along
            // the crossing asks about the cells it covers, so each answer is kept.
            let carriageway_memo = std::cell::RefCell::new(CellMemo::default());
            let carriageway_at = |editor: &WorldEditor, cx: i32, cz: i32| {
                let clips = crossing_clips?;
                carriageway_memo.borrow_mut().get_or(cx, cz, || {
                    clips
                        .iter()
                        .find(|c| {
                            c.contains(cx, cz)
                                && !(c.past_core(cx, cz)
                                    && c.core().is_some_and(|core| {
                                        let (ox, oz) = c.nearest_centre(cx, cz).unwrap_or((cx, cz));
                                        cut_outward(core, cx - ox, cz - oz, |dx, dz| {
                                            cut_beside(
                                                |sx, sz| c.in_core(core, sx, sz),
                                                walls,
                                                Some(editor),
                                                |px, pz| editor.on_mapped_path(px, pz),
                                                c.verges(),
                                                ox + dx,
                                                oz + dz,
                                            )
                                        })
                                    }))
                        })
                        .map(|c| c.palette())
                })
            };

            // One surface level per centerline cell; bridges and ramps carry their own.
            let own_profile: Vec<i32> = if is_bridge_member || is_bridge_ramp {
                Vec::new()
            } else {
                at_grade_profile(
                    editor,
                    way,
                    block_range,
                    highway_connectivity,
                    tunnel_portals,
                    bridge_structures,
                )
            };
            let road_ys: &[i32] = bridge_ramp.map_or(&own_profile, |ramp| ramp.ys());
            let road_y_at = |tds: usize, along: i32| {
                let at = (tds as i32 + along).clamp(0, road_ys.len() as i32 - 1);
                road_ys[at as usize]
            };

            // Iterate over nodes to create the highway
            let mut segment_index = 0;
            // Cumulative bresenham distance across all segments; drives bridge ramp interp.
            let mut cumulative_distance_from_start: usize = 0;
            // Previous bridge cell Y for steep-step gap fill.
            let mut previous_bridge_y: Option<i32> = None;
            // Centerline samples captured for above-deck decoration after the segment loop.
            let mut bridge_path: Vec<BridgePathSample> = Vec::new();
            // Previous rail cell per side; used to orthogonally connect diagonal steps.
            let mut previous_rail_left: Option<(i32, i32)> = None;
            let mut previous_rail_right: Option<(i32, i32)> = None;
            // Last parapet cell and wall Y per side, for stepping on ramps.
            let mut previous_parapet_left: Option<((i32, i32), i32)> = None;
            let mut previous_parapet_right: Option<((i32, i32), i32)> = None;

            for node in &way.nodes {
                if let Some(prev) = previous_node {
                    let (x1, z1) = prev;
                    let x2: i32 = node.x;
                    let z2: i32 = node.z;

                    // Generate the line of coordinates between the two nodes
                    let bresenham_points: Vec<(i32, i32, i32)> =
                        bresenham_line(x1, 0, z1, x2, 0, z2);

                    // Calculate elevation for this segment
                    let segment_length = bresenham_points.len();

                    // Dominant axis of this segment, which stripes and crossing lines run across.
                    let dir_horizontal = (x2 - x1).abs() >= (z2 - z1).abs();

                    // At-grade roads take their surface from the way's profile.
                    let flatten_width = !road_ys.is_empty();
                    // Whether the road cross-section also registers an
                    // effective-ground override is decided per bresenham
                    // point below — `offset` varies inside a segment (slope
                    // ramps at layer transitions), and elevated sections
                    // (offset > 0) must NOT register, otherwise
                    // `ground_generation` fills terrain all the way up to
                    // the deck and bridges become giant embankments.

                    // Unit direction of this segment for paint; None when zero-length.
                    let paint_axes = {
                        let dx_seg = (x2 - x1) as f32;
                        let dz_seg = (z2 - z1) as f32;
                        let seg_len = (dx_seg * dx_seg + dz_seg * dz_seg).sqrt();
                        (seg_len > 0.0).then(|| (dx_seg / seg_len, dz_seg / seg_len))
                    };

                    // Unit perpendicular for this segment, used by bridge rail placement.
                    let bridge_rail_perp: Option<(f32, f32)> = if is_bridge_member || is_bridge_ramp
                    {
                        let dx_seg = (x2 - x1) as f32;
                        let dz_seg = (z2 - z1) as f32;
                        let seg_len = (dx_seg * dx_seg + dz_seg * dz_seg).sqrt();
                        if seg_len > 0.0 {
                            Some((-dz_seg / seg_len, dx_seg / seg_len))
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    // How far along the way a cell sits.
                    let along_offset = along_steps((x1, z1), (x2, z2));
                    // Profile height of a column, placed from the segment's start so every
                    // stamp along it reads the same cell.
                    let segment_tds = cumulative_distance_from_start;
                    let profile_y =
                        |cx: i32, cz: i32| road_y_at(segment_tds, along_offset(cx - x1, cz - z1));
                    // The deck cell a column takes its level from, read the same way.
                    let column_tds = |cx: i32, cz: i32| {
                        (segment_tds as i32 + along_offset(cx - x1, cz - z1))
                            .clamp(0, total_bresenham_length as i32 - 1)
                            as usize
                    };
                    // A column another road already surfaced keeps its level, so the two never
                    // stack. A ramp only takes one within a block of its own, not the street below.
                    let flat_y = |editor: &WorldEditor, cx: i32, cz: i32, offset: i32| {
                        let own = profile_y(cx, cz) + offset;
                        match editor.surface_override(cx, cz) {
                            Some(y) if offset == 0 && (!is_bridge_ramp || (y - own).abs() <= 1) => {
                                y
                            }
                            _ => own,
                        }
                    };

                    // Bridges/ramps drive their Y from cumulative tds, so skip the duplicate
                    // shared endpoint on later segments.
                    let skip_first = if (is_bridge_member || is_bridge_ramp) && segment_index > 0 {
                        1
                    } else {
                        0
                    };
                    for (point_index, (x, _, z)) in
                        bresenham_points.iter().enumerate().skip(skip_first)
                    {
                        let tds = cumulative_distance_from_start + point_index;
                        let bridge_y_here = bridge_member.map(|info| info.y_at(tds));
                        // Railings run along decks and ramps alike.
                        let rail_y_here =
                            bridge_y_here.or_else(|| bridge_ramp.map(|info| info.y_at(tds)));

                        let offset = if is_bridge_member || is_bridge_ramp {
                            0
                        } else {
                            calculate_point_elevation(
                                tds,
                                total_bresenham_length - 1,
                                effective_elevation,
                                effective_start_slope,
                                effective_end_slope,
                                slope_length,
                            )
                        };

                        // Absolute Y of an approach cell; one Y per stamp would slab the ramp.
                        let approach_at = |dx: i32, dz: i32| -> Option<i32> {
                            let a = tunnel_approach?;
                            let t = (tds as i32 + along_offset(dx, dz))
                                .clamp(0, road_ys.len() as i32 - 1);
                            match tunnel_approach_offset(a, t) {
                                drop if drop < 0 => road_ys.get(t as usize).map(|&y| y + drop),
                                _ => None,
                            }
                        };

                        let register_ground_override = flatten_width && offset == 0;

                        let use_absolute_y = is_bridge_member || is_bridge_ramp || flatten_width;

                        // Only Arch actually reads this; other styles re-sample inside place_pillar.
                        let centerline_ground_y =
                            if is_bridge_member && matches!(bridge_style, BridgeStyle::Arch) {
                                editor.get_ground_level(*x, *z)
                            } else {
                                0
                            };

                        if is_bridge_member {
                            if let (Some(by), Some(perp)) = (bridge_y_here, bridge_rail_perp) {
                                bridge_path.push((*x, by, *z, perp));
                            }
                        }

                        // Backfill steep ramp steps where deck+foundation alone leaves an air band.
                        if let Some(by) = bridge_y_here {
                            if let Some(prev_y) = previous_bridge_y {
                                let (fill_lo, fill_hi) = if by >= prev_y + 3 {
                                    (prev_y + 1, by - 2)
                                } else if by <= prev_y - 3 {
                                    (by + 1, prev_y - 2)
                                } else {
                                    (0, -1)
                                };
                                if fill_lo <= fill_hi {
                                    for fill_y in fill_lo..=fill_hi {
                                        for fdx in -block_range..=block_range {
                                            for fdz in -block_range..=block_range {
                                                editor.set_block_absolute(
                                                    bridge_foundation_block,
                                                    *x + fdx,
                                                    fill_y,
                                                    *z + fdz,
                                                    None,
                                                    Some(ROAD_PROTECTED_SURFACES),
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            previous_bridge_y = Some(by);
                        }

                        // Draw the road surface for the entire width
                        for dx in -block_range..=block_range {
                            for dz in -block_range..=block_range {
                                if !square_stamp && !in_side_stamp(dx, dz, paint_axes, sides) {
                                    continue;
                                }
                                let set_x: i32 = x + dx;
                                let set_z: i32 = z + dz;
                                if cut_here(editor, dx, dz, set_x, set_z) {
                                    continue;
                                }
                                if drown_over_water
                                    && !road_mask.contains(set_x, set_z)
                                    && editor.is_lc_water(set_x, set_z)
                                {
                                    continue;
                                }

                                // Per-cell Y. For wide roads this is the
                                // perpendicular median at the cell's own
                                // along-length coord — so all cells at the
                                // same along-length coord share one Y
                                // (flat cross-section) and register the
                                // same effective-ground override.
                                let approach_y = approach_at(dx, dz);
                                let cell_y = if let Some(info) = bridge_member {
                                    info.y_at(column_tds(set_x, set_z))
                                } else if let Some(ay) = approach_y {
                                    ay
                                } else if flatten_width {
                                    flat_y(editor, set_x, set_z, offset)
                                } else {
                                    offset
                                };
                                // Where a ramp stands clear of the ground it is built up in
                                // stone, so its sides read as walls, and spans what runs below.
                                let raised_ground = if is_bridge_ramp {
                                    editor
                                        .terrain_level(set_x, set_z)
                                        .filter(|&g| g + 1 < cell_y)
                                } else {
                                    None
                                };
                                if let Some(ground) = raised_ground {
                                    if bridge_surface.over_grade_way(set_x, set_z)
                                        && cell_y - ground >= RAMP_SPAN_CLEARANCE
                                    {
                                        editor.set_block_absolute(
                                            ramp_wall,
                                            set_x,
                                            cell_y - 1,
                                            set_z,
                                            None,
                                            None,
                                        );
                                    } else {
                                        editor.fill_column_absolute(
                                            ramp_wall,
                                            set_x,
                                            set_z,
                                            ground + 1,
                                            cell_y - 1,
                                            true,
                                        );
                                    }
                                }
                                // The tunnel owns columns in its footprint; depth here bares the roof.
                                if register_ground_override
                                    && raised_ground.is_none()
                                    && !(approach_y.is_some()
                                        && tunnel_footprint.contains(set_x, set_z))
                                {
                                    editor.register_road_surface_y(set_x, set_z, cell_y);
                                }

                                // A crossing on the road is painted on the road's own
                                // surface, not the footway's grey.
                                let crossing_here =
                                    crossing_paint.zip(carriageway_at(editor, set_x, set_z));
                                if let Some((paint, road)) = crossing_here {
                                    let along = if dir_horizontal { set_x } else { set_z };
                                    let on_stripe = paint == CrossingPaint::Zebra
                                        && args.road_detail.bar(along);
                                    if on_stripe {
                                        // A bar only replaces the road under it, whichever was first.
                                        if use_absolute_y {
                                            editor.set_block_absolute(
                                                WHITE_CONCRETE,
                                                set_x,
                                                cell_y,
                                                set_z,
                                                Some(road),
                                                None,
                                            );
                                        } else {
                                            editor.set_block(
                                                WHITE_CONCRETE,
                                                set_x,
                                                cell_y,
                                                set_z,
                                                Some(road),
                                                None,
                                            );
                                        }
                                    } else {
                                        let bg = semirandom_surface(set_x, set_z, road);
                                        if use_absolute_y {
                                            editor.set_block_absolute(
                                                bg, set_x, cell_y, set_z, None, None,
                                            );
                                        } else {
                                            editor.set_block(bg, set_x, cell_y, set_z, None, None);
                                        }
                                    }
                                } else {
                                    // Unified surface selection. For single-block
                                    // surfaces (concrete, sand, dirt_path...),
                                    // `block_types` is a 1-element slice so
                                    // every cell picks the same block; for
                                    // multi-block mixes (default road, asphalt)
                                    // the hash scatters the blocks randomly.
                                    // Blacklist is the narrow asphalt-mix set
                                    // defined in ROAD_PROTECTED_SURFACES — see
                                    // its doc comment for the overlap-handling
                                    // rationale.
                                    // Track paint goes over asphalt whichever was laid first.
                                    let race =
                                        race_paint.and_then(|p| p.at(race_deck, set_x, set_z));
                                    let effective_block = race.unwrap_or_else(|| {
                                        semirandom_surface(set_x, set_z, block_types)
                                    });
                                    // The parking rows outside the lanes stop at a
                                    // sidewalk mapped alongside, whichever is built first.
                                    let parking_row = !square_stamp
                                        && lane_range < block_range
                                        && !in_road_stamp(dx, dz, lane_range);
                                    let protect = if race.is_some() {
                                        OVER_ASPHALT_PROTECTED_SURFACES
                                    } else if parking_row {
                                        PARKING_ROW_PROTECTED_SURFACES
                                    } else if is_sidewalk && !editor.on_carriageway(set_x, set_z) {
                                        sidewalk_protect
                                    } else {
                                        surface_protect
                                    };
                                    if use_absolute_y {
                                        editor.set_block_absolute(
                                            effective_block,
                                            set_x,
                                            cell_y,
                                            set_z,
                                            None,
                                            Some(protect),
                                        );
                                    } else {
                                        editor.set_block(
                                            effective_block,
                                            set_x,
                                            cell_y,
                                            set_z,
                                            None,
                                            Some(protect),
                                        );
                                    }
                                    if !strip.is_empty() {
                                        levels.insert((set_x, set_z), (cell_y, use_absolute_y));
                                    }
                                }

                                // Add a foundation and support pillars only for
                                // genuinely elevated decks — bridges and explicit overpasses.
                                // (Regular wide roads now flow through `use_absolute_y == true`
                                // too, but they aren't floating decks; they get embankments
                                // from the registered ground-surface override instead.)
                                let is_elevated_deck = is_bridge_member || effective_elevation > 0;
                                // Absolute decks can sit below Y=0.
                                if is_elevated_deck && (use_absolute_y || cell_y > 0) {
                                    // The deck's own slab under a bridge, stone bricks elsewhere.
                                    let foundation = if is_bridge_member {
                                        bridge_paint.unwrap_or(bridge_foundation_block)
                                    } else {
                                        STONE_BRICKS
                                    };
                                    if use_absolute_y {
                                        editor.set_block_absolute(
                                            foundation,
                                            set_x,
                                            cell_y - 1,
                                            set_z,
                                            None,
                                            None,
                                        );
                                    } else {
                                        editor.set_block(
                                            foundation,
                                            set_x,
                                            cell_y - 1,
                                            set_z,
                                            None,
                                            None,
                                        );
                                    }

                                    let is_center = dx == 0 && dz == 0;
                                    // Path-index spacing; an (x + z) grid misplaced piers on diagonals.
                                    let on_pier_step = |interval: usize| {
                                        interval > 0 && tds % interval == interval / 2
                                    };
                                    let blocked = use_absolute_y
                                        && bridge_surface.support_blocked(set_x, set_z, cell_y);
                                    if is_bridge_member {
                                        if bridge_style == BridgeStyle::Arch && !blocked {
                                            // Per-cell position so overlapping stamps keep the openings.
                                            let cell_tds = column_tds(set_x, set_z);
                                            place_arch_below_deck(
                                                editor,
                                                set_x,
                                                cell_y,
                                                set_z,
                                                centerline_ground_y,
                                                cell_tds,
                                                total_bresenham_length,
                                                use_absolute_y,
                                                is_center,
                                            );
                                        }
                                        // Other decks stand on the piers their structure lays out.
                                        if is_center
                                            && use_absolute_y
                                            && bridge_style == BridgeStyle::Boardwalk
                                            && !hangs_at(set_x, set_z)
                                            && on_pier_step(bridge_style.pier_interval(block_range))
                                        {
                                            if let Some(perp) = bridge_rail_perp {
                                                place_pier_bent(
                                                    editor,
                                                    bridge_surface,
                                                    bridge_style,
                                                    set_x,
                                                    set_z,
                                                    cell_y,
                                                    perp,
                                                    block_range,
                                                );
                                            }
                                        }
                                    } else if is_center
                                        && on_pier_step(OVERPASS_PILLAR_INTERVAL)
                                        && !blocked
                                    {
                                        if use_absolute_y {
                                            place_pillar(
                                                editor,
                                                set_x,
                                                cell_y,
                                                set_z,
                                                STONE_BRICKS,
                                                true,
                                            );
                                        } else {
                                            add_highway_support_pillar(
                                                editor, set_x, cell_y, set_z,
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        // Side railings; stair_steps keeps the rail 4-connected on diagonals.
                        if let (Some(by), Some((perp_x, perp_z))) = (rail_y_here, bridge_rail_perp)
                        {
                            // The stamp's extent across + 1, so the rail never lands on the deck.
                            let rail_dist =
                                stamp_reach(block_range, square_stamp, perp_x, perp_z) + 1.0;
                            for (sign, prev_state, prev_parapet) in [
                                (1.0_f32, &mut previous_rail_left, &mut previous_parapet_left),
                                (
                                    -1.0_f32,
                                    &mut previous_rail_right,
                                    &mut previous_parapet_right,
                                ),
                            ] {
                                let cx = *x as f32 + perp_x * rail_dist * sign;
                                let cz = *z as f32 + perp_z * rail_dist * sign;
                                let rail_cell = (cx.round() as i32, cz.round() as i32);
                                let cells_to_fill: Vec<(i32, i32)> = match *prev_state {
                                    Some(prev) => stair_steps(prev, rail_cell),
                                    None => vec![rail_cell],
                                };
                                // A deck's railing is laid out over its whole structure.
                                let skip_side_railing = is_bridge_member;
                                if !skip_side_railing {
                                    for (rx, rz) in cells_to_fill {
                                        // Another deck continues the surface at this level.
                                        if bridge_surface.deck_near(rx, rz, by, 2) {
                                            *prev_parapet = None;
                                            continue;
                                        }
                                        // No parapet where an approach ramp is back on the ground.
                                        if is_bridge_ramp
                                            && by <= editor.get_ground_level(rx, rz) + 1
                                        {
                                            *prev_parapet = None;
                                            continue;
                                        }
                                        let rail_block =
                                            ramp_design.map_or(LIGHT_GRAY_CONCRETE, |d| d.curb());
                                        editor.set_block_absolute(
                                            rail_block,
                                            rx,
                                            by,
                                            rz,
                                            None,
                                            Some(ROAD_PROTECTED_SURFACES),
                                        );
                                        let rail_foundation = ramp_wall;
                                        editor.set_block_absolute(
                                            rail_foundation,
                                            rx,
                                            by - 1,
                                            rz,
                                            None,
                                            None,
                                        );
                                        // A ramp's side is a retaining wall down to the ground,
                                        // unless another way runs below it.
                                        if is_bridge_ramp {
                                            let ground = editor.get_ground_level(rx, rz);
                                            let spans = road_mask.contains(rx, rz)
                                                && by - ground >= RAMP_SPAN_CLEARANCE;
                                            if ground < by - 2 && !spans {
                                                editor.fill_column_absolute(
                                                    ramp_wall,
                                                    rx,
                                                    rz,
                                                    ground + 1,
                                                    by - 2,
                                                    true,
                                                );
                                            }
                                        }
                                        let parapet = match ramp_design {
                                            Some(d) => d.parapet(),
                                            None => Some(BRICK_WALL),
                                        };
                                        if let Some(p) = parapet {
                                            let top = by + 1;
                                            place_connected(editor, p, rx, top, rz);
                                            // Step the parapet where the deck changes height.
                                            if let Some(((px, pz), prev_top)) = *prev_parapet {
                                                let adjacent =
                                                    (px - rx).abs() + (pz - rz).abs() == 1;
                                                if adjacent && prev_top == top - 1 {
                                                    place_connected(editor, p, px, top, pz);
                                                } else if adjacent && prev_top == top + 1 {
                                                    place_connected(editor, p, rx, prev_top, rz);
                                                }
                                            }
                                            *prev_parapet = Some(((rx, rz), top));
                                        }
                                    }
                                }
                                *prev_state = Some(rail_cell);
                            }
                        }

                        // Paint rides at the Y of the road cell under it; clamped because a
                        // rounded cell can land one outside the stamp on diagonals.
                        let paint = |editor: &mut WorldEditor,
                                     block: Block,
                                     sx: i32,
                                     sz: i32,
                                     over: &[Block]| {
                            let sy = if let Some(info) = bridge_member {
                                info.y_at(column_tds(sx, sz))
                            } else if let Some(ay) = approach_at(sx - *x, sz - *z) {
                                ay
                            } else if flatten_width {
                                flat_y(editor, sx, sz, offset)
                            } else {
                                offset
                            };
                            // Only over a road surface, so paint never lands on a neighbour.
                            if use_absolute_y {
                                editor.set_block_absolute(block, sx, sy, sz, Some(over), None);
                            } else {
                                editor.set_block(block, sx, sy, sz, Some(over), None);
                            }
                        };

                        // Edge lines of a signalised crossing, one cell outside its width.
                        if let Some(CrossingPaint::Lines { broken }) = crossing_paint {
                            let along = if dir_horizontal { *x } else { *z };
                            if !broken || args.road_detail.bar(along) {
                                for side in [-1, 1] {
                                    let off = side * (block_range + 1);
                                    let (sx, sz) = if dir_horizontal {
                                        (*x, *z + off)
                                    } else {
                                        (*x + off, *z)
                                    };
                                    if let Some(road) = carriageway_at(editor, sx, sz) {
                                        paint(editor, WHITE_CONCRETE, sx, sz, road);
                                    }
                                }
                            }
                        }

                        if let Some((ux, uz)) = paint_axes {
                            let (perp_x, perp_z) = (-uz, ux);
                            let t = tds as u32;
                            // Lane lines, dashed by path position so curves and short
                            // segments keep the rhythm.
                            if !way_marks.is_some_and(|m| m.in_gap(t)) {
                                // A One World piece clips a way where one run or the next
                                // piece does not, which shifts its path positions; with
                                // --road-detail the dashes count along the segment's axis
                                // in the world frame instead.
                                let phase = if args.road_detail != RoadDetail::Max
                                    && args.one_world_run.is_some()
                                {
                                    if dir_horizontal {
                                        i64::from(*x) * i64::from((x2 - x1).signum())
                                    } else {
                                        i64::from(*z) * i64::from((z2 - z1).signum())
                                    }
                                } else {
                                    way_marks.map_or(t as i64, |m| m.dash_phase(t))
                                };
                                let dash = phase.rem_euclid(dash_period as i64) < dash_on as i64;
                                for line in lines.iter().filter(|l| l.solid || dash) {
                                    // Offset rounded on its own, so lines sit symmetrically.
                                    let sx = *x + (perp_x * line.offset).round() as i32;
                                    let sz = *z + (perp_z * line.offset).round() as i32;
                                    paint(editor, line.block, sx, sz, block_types);
                                }
                            }

                            // Stop and give-way lines and crossings, one row across the
                            // road per centerline cell.
                            for mark in way_marks
                                .map_or(&[][..], |m| m.marks.as_slice())
                                .iter()
                                .filter(|m| m.covers(t))
                            {
                                // Kerb to kerb: nobody parks by a junction or a crossing.
                                let reach = stamp_reach(block_range, square_stamp, perp_x, perp_z);
                                let spacing = perp_x.abs() + perp_z.abs();
                                let r = block_range + 1;
                                for dx in -r..=r {
                                    for dz in -r..=r {
                                        let s = dx as f32 * ux + dz as f32 * uz;
                                        let p = dx as f32 * perp_x + dz as f32 * perp_z;
                                        let on_side = match (mark.side, centre_span) {
                                            (1, Some((_, hi))) => p > hi + 0.5,
                                            (-1, Some((lo, _))) => p < lo - 0.5,
                                            (1, None) => p > -0.5,
                                            (-1, None) => p < 0.5,
                                            _ => true,
                                        };
                                        if s.abs() > 0.5 || p.abs() > reach + 0.01 || !on_side {
                                            continue;
                                        }
                                        let (sx, sz) = (*x + dx, *z + dz);
                                        // Bars run with the road, alternating across it.
                                        let bar = args.road_detail.bar(
                                            ((sx as f32 * perp_x + sz as f32 * perp_z) / spacing)
                                                .floor()
                                                as i32,
                                        );
                                        let white = match mark.kind {
                                            CrossMark::Stop => true,
                                            CrossMark::GiveWay | CrossMark::Zebra => bar,
                                            CrossMark::CrossingLines(broken) => !broken || bar,
                                        };
                                        if white {
                                            paint(editor, WHITE_CONCRETE, sx, sz, block_types);
                                        }
                                    }
                                }
                            }

                            // Stall lines across the parking rows, one stall apart, kerb to
                            // lane edge; none off the road's own asphalt, on another road's
                            // lanes, by a junction or a crossing.
                            if !stall_rows.is_empty()
                                && lane_range < block_range
                                && stall_period >= 3
                            {
                                let along = if args.one_world_run.is_some() {
                                    if dir_horizontal {
                                        i64::from(*x) * i64::from((x2 - x1).signum())
                                    } else {
                                        i64::from(*z) * i64::from((z2 - z1).signum())
                                    }
                                } else {
                                    way_marks.map_or(t as i64, |m| m.dash_phase(t))
                                };
                                let clear_of_junctions = junction_nodes.iter().all(|&(jx, jz)| {
                                    let (dx, dz) = (*x - jx, *z - jz);
                                    let gap = block_range + TURN_SWEEP;
                                    dx * dx + dz * dz > gap * gap
                                });
                                let clear_of_marks = way_marks.is_none_or(|m| {
                                    (t.saturating_sub(1)..=t + 1).all(|k| !m.in_gap(k))
                                        && m.marks.iter().all(|mark| mark.t.abs_diff(t) > 1)
                                });
                                if along.rem_euclid(stall_period) == 0
                                    && clear_of_junctions
                                    && clear_of_marks
                                {
                                    // Square to the kerb, or along the grid where the road
                                    // runs near it, so a line stays one row.
                                    let gridded = ux.abs().min(uz.abs()) < 0.42;
                                    let across = |dx: i32, dz: i32| {
                                        if !gridded {
                                            (dx as f32 * ux + dz as f32 * uz).abs() <= 0.5
                                        } else if ux.abs() > uz.abs() {
                                            dx == 0
                                        } else {
                                            dz == 0
                                        }
                                    };
                                    let r = block_range + 1;
                                    for &sign in &stall_rows {
                                        let side = if sign > 0.0 { sides.1 } else { sides.0 };
                                        let reach = stamp_reach(side, square_stamp, perp_x, perp_z);
                                        for dx in -r..=r {
                                            for dz in -r..=r {
                                                let p = sign
                                                    * (dx as f32 * perp_x + dz as f32 * perp_z);
                                                if !across(dx, dz)
                                                    || p <= lane_range as f32 + 0.5
                                                    || p > reach + 0.01
                                                {
                                                    continue;
                                                }
                                                let (sx, sz) = (*x + dx, *z + dz);
                                                if !editor.on_carriageway(sx, sz)
                                                    && !cut_here(editor, dx, dz, sx, sz)
                                                {
                                                    paint(
                                                        editor,
                                                        WHITE_CONCRETE,
                                                        sx,
                                                        sz,
                                                        block_types,
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    segment_index += 1;
                    cumulative_distance_from_start += segment_length - 1;
                }
                previous_node = Some((node.x, node.z));
            }
            for cell in strip {
                let (x, z) = cell.cell;
                let block = semirandom_surface(x, z, block_types);
                match levels.get(&cell.from) {
                    Some(&(y, true)) => {
                        editor.set_block_absolute(block, x, y, z, None, Some(surface_protect))
                    }
                    Some(&(y, false)) => {
                        editor.set_block(block, x, y, z, None, Some(surface_protect))
                    }
                    None => editor.set_block(block, x, 0, z, None, Some(surface_protect)),
                }
            }

            if is_bridge_member {
                if let (Some(design), Some(parts)) =
                    (bridge_design, bridge_surface.parts_of(way.id))
                {
                    crate::element_processing::bridge_decks::render(editor, design, parts);
                }
                if bridge_pylons.is_some()
                    || !matches!(
                        bridge_style,
                        BridgeStyle::Suspension | BridgeStyle::CableStayed
                    )
                {
                    decorate_bridge_above_deck(
                        editor,
                        bridge_surface,
                        bridge_style,
                        &bridge_path,
                        block_range,
                        bridge_start_is_boundary,
                        bridge_end_is_boundary,
                        bridge_pylons,
                        bridge_cable_span,
                    );
                }
            }
        }
    } else if let (ProcessedElement::Node(node), Some(signals)) =
        (element, editor.traffic_signals())
    {
        // Tram crossings carry the signal tags but no highway tag.
        if is_signal(&node.tags) {
            signals.place(editor, node, &signal_base);
        }
    }
}

/// Helper function to determine if a slope should be added at a specific node
fn should_add_slope_at_node(
    node: &crate::osm_parser::ProcessedNode,
    current_layer: i32,
    highway_connectivity: &HighwayConnectivityMap,
) -> bool {
    let node_coord = (node.x, node.z);

    // If we don't have connectivity information, always add slopes for non-zero layers
    if highway_connectivity.ends.is_empty() {
        return current_layer != 0;
    }

    // Check if there are other highways at different layers connected to this node
    if let Some(connected_layers) = highway_connectivity.ends.get(&node_coord) {
        // Count how many ways are at the same layer as current way
        let same_layer_count = connected_layers
            .iter()
            .filter(|&&layer| layer == current_layer)
            .count();

        // If this is the only way at this layer connecting to this node, we need a slope
        // (unless we're at ground level and connecting to ground level ways)
        if same_layer_count <= 1 {
            return current_layer != 0;
        }

        // If there are multiple ways at the same layer, don't add slope
        false
    } else {
        // No other highways connected, add slope if not at ground level
        current_layer != 0
    }
}

/// Helper function to calculate the total length of a way in blocks
fn calculate_way_length(way: &ProcessedWay) -> usize {
    let mut total_length = 0;
    let mut previous_node: Option<&crate::osm_parser::ProcessedNode> = None;

    for node in &way.nodes {
        if let Some(prev) = previous_node {
            let dx = (node.x - prev.x).abs();
            let dz = (node.z - prev.z).abs();
            total_length += ((dx * dx + dz * dz) as f32).sqrt() as usize;
        }
        previous_node = Some(node);
    }

    total_length
}

/// Height above the ground of path cell `tds` (of `last + 1`) on a way raised by
/// `base_elevation`, ramping linearly over `slope_length` cells at the ends that need it.
fn calculate_point_elevation(
    tds: usize,
    last: usize,
    base_elevation: i32,
    needs_start_slope: bool,
    needs_end_slope: bool,
    slope_length: usize,
) -> i32 {
    let ramp = |d: usize| (d as f32 / slope_length.max(1) as f32).min(1.0);
    let mut share = 1.0f32;
    if needs_start_slope {
        share = share.min(ramp(tds));
    }
    if needs_end_slope {
        share = share.min(ramp(last.saturating_sub(tds)));
    }
    (base_elevation as f32 * share).round() as i32
}

/// Support pillar for an elevated highway whose Y is an offset from the ground.
fn add_highway_support_pillar(editor: &mut WorldEditor, x: i32, highway_y: i32, z: i32) {
    for y in 1..highway_y {
        editor.set_block(STONE_BRICKS, x, y, z, None, None);
    }
    for base_dx in -1..=1 {
        for base_dz in -1..=1 {
            editor.set_block(STONE_BRICKS, x + base_dx, 0, z + base_dz, None, None);
        }
    }
}

/// Generates a siding using stone brick slabs, kept off road surfaces and a path's asphalt
pub fn generate_siding(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    bridge_surface: &BridgeSurfaceMap,
) {
    let mut previous_node: Option<XZPoint> = None;
    let siding_block: Block = STONE_BRICK_SLAB;

    for node in &element.nodes {
        let current_node = node.xz();

        if let Some(prev_node) = previous_node {
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(
                prev_node.x,
                0,
                prev_node.z,
                current_node.x,
                0,
                current_node.z,
            );

            for (bx, _, bz) in bresenham_points {
                if let Some(deck_y) = bridge_surface.deck_y_at(bx, bz) {
                    if !editor.check_for_block_absolute(
                        bx,
                        deck_y,
                        bz,
                        Some(WALKWAY_PROTECTED_SURFACES),
                        None,
                    ) {
                        editor.set_block_absolute(siding_block, bx, deck_y + 1, bz, None, None);
                    }
                } else if !editor.check_for_block(bx, 0, bz, Some(WALKWAY_PROTECTED_SURFACES)) {
                    editor.set_block(siding_block, bx, 1, bz, None, None);
                }
            }
        }

        previous_node = Some(current_node);
    }
}

/// A centerline point with its segment's unit travel direction (`ux`, `uz`) and cumulative
/// distance `s` (blocks) from the way start, used for dash phase.
struct AerowayCenterPoint {
    x: i32,
    z: i32,
    ux: f32,
    uz: f32,
    s: f32,
}

/// Runway centerline dash: stripe-on / stripe-off lengths in meters (scaled by `--scale`).
const RUNWAY_DASH_ON_M: f32 = 10.0;
const RUNWAY_DASH_OFF_M: f32 = 6.0;
/// How far inside the runway edge (blocks) the solid white edge stripes sit.
const RUNWAY_EDGE_INSET: f32 = 1.0;
/// Half-width (metres) used when an aeroway has no `width=*` tag (~24 m strip).
const AEROWAY_DEFAULT_HALF_M: f64 = 12.0;
/// Clamp (metres) for `width=*`-derived half-widths — guards against absurd tags.
const AEROWAY_MIN_HALF_M: f64 = 6.0;
const AEROWAY_MAX_HALF_M: f64 = 40.0;
/// Half-width (metres) of a stand lead-in line, which is a marking rather than a taxiway.
const PARKING_POSITION_HALF_M: f64 = 2.5;

/// True where a runway centerline dash is painted, given distance `s` (blocks) from the way start.
fn runway_centerline_dash_on(s: f32, scale: f64) -> bool {
    let on = (RUNWAY_DASH_ON_M * scale as f32).max(1.0);
    let off = (RUNWAY_DASH_OFF_M * scale as f32).max(1.0);
    (s % (on + off)) < on
}

/// Parses an OSM `width=*` value in metres (tolerates a trailing "m").
pub(crate) fn parse_width_tag_m(tags: &HashMap<String, String>) -> Option<f64> {
    road_width::metres(tags.get("width")?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AerowayArea {
    /// Runway, stopway or blast pad.
    Runway,
    /// Taxiway, apron or stand.
    Apron,
}

/// Aprons, `area:aeroway` ways, and runways or taxiways with `area=yes`.
pub(crate) fn aeroway_area(way: &ProcessedWay) -> Option<AerowayArea> {
    let closed = way.nodes.len() >= 4
        && way.nodes.first().map(|n| (n.x, n.z)) == way.nodes.last().map(|n| (n.x, n.z));
    if !closed {
        return None;
    }
    let area = way.tags.get("area").map(String::as_str) == Some("yes");
    let kind = match way.tags.get("area:aeroway").map(String::as_str) {
        Some(kind) => kind,
        None => match way.tags.get("aeroway").map(String::as_str) {
            Some("apron") => "apron",
            Some(kind @ ("runway" | "taxiway")) if area => kind,
            _ => return None,
        },
    };
    match kind {
        "runway" | "stopway" | "blastpad" | "blast_pad" => Some(AerowayArea::Runway),
        "taxiway" | "apron" | "parking_position" => Some(AerowayArea::Apron),
        _ => None,
    }
}

/// Ground block for an aeroway with an unpaved surface, like a grass strip.
fn unpaved_aeroway_block(tags: &HashMap<String, String>) -> Option<Block> {
    let surface = tags.get("surface")?.as_str();
    match surface {
        "grass" | "dirt" | "ground" | "earth" | "soil" | "unpaved" | "gravel" | "fine_gravel"
        | "compacted" | "sand" | "clay" | "mud" => {
            get_blocks_for_surface(surface).and_then(|blocks| blocks.first().copied())
        }
        _ => None,
    }
}

/// Concrete for a paved aeroway area: gray for runways, light gray for the rest.
pub(crate) fn aeroway_area_block(way: &ProcessedWay) -> Option<Block> {
    if unpaved_aeroway_block(&way.tags).is_some() {
        return None;
    }
    aeroway_area(way).map(|area| match area {
        AerowayArea::Runway => GRAY_CONCRETE,
        AerowayArea::Apron => LIGHT_GRAY_CONCRETE,
    })
}

/// Fills aeroway areas and draws runways, taxiways and stand lines as marked strips.
/// No threshold "piano keys" — OSM splits runways into segments, so a per-way renderer can't tell
/// a real end from an internal split.
pub fn generate_aeroway(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    args: &Args,
    building_footprints: &CoordinateBitmap,
    flood_fill_cache: &FloodFillCache,
) {
    let aeroway = way.tags.get("aeroway").map(String::as_str);

    if aeroway == Some("helipad") {
        generate_helipad_way(editor, way, args, building_footprints);
        return;
    }

    let unpaved = unpaved_aeroway_block(&way.tags);

    // Grass mapped inside an area keeps its cells, as in a parking lot.
    if aeroway_area(way).is_some() {
        let Some(block) = aeroway_area_block(way).or(unpaved) else {
            return;
        };
        let runway_overwrites = [LIGHT_GRAY_CONCRETE, YELLOW_CONCRETE];
        let over: Option<&[Block]> = (block == GRAY_CONCRETE).then_some(&runway_overwrites[..]);
        let filled = flood_fill_cache.get_or_compute(way, args.timeout.as_ref());
        for &(x, z) in filled.iter() {
            if !editor.nested_area_owns(x, z) {
                editor.set_block(block, x, 0, z, over, None);
            }
        }
        return;
    }

    let is_runway = aeroway == Some("runway");
    let is_taxiway = aeroway == Some("taxiway");
    // A parking position or taxilane is a painted lead-in, not a 24 m slab.
    let is_stand_line = matches!(aeroway, Some("parking_position" | "taxilane"));
    // Runway pavement without markings.
    let is_stopway = aeroway == Some("stopway");
    // Aerodrome boundaries, hangars and holding positions have no pavement.
    if !(is_runway || is_taxiway || is_stand_line || is_stopway) {
        return;
    }

    let base_block = unpaved.unwrap_or(if is_runway || is_stopway {
        GRAY_CONCRETE
    } else {
        LIGHT_GRAY_CONCRETE
    });

    // Half-width from the OSM `width=*` tag (metres, clamped to sane sizes); default when absent.
    let half_m = if is_stand_line {
        PARKING_POSITION_HALF_M
    } else {
        parse_width_tag_m(&way.tags)
            .map(|w| (w * 0.5).clamp(AEROWAY_MIN_HALF_M, AEROWAY_MAX_HALF_M))
            .unwrap_or(AEROWAY_DEFAULT_HALF_M)
    };
    let half_width: i32 = (half_m * args.scale).round().max(1.0) as i32;

    // Build the centerline once: bresenham per segment, consecutive duplicates dropped, with a
    // running distance so dash phase and markings stay consistent across segments and regions.
    let mut points: Vec<AerowayCenterPoint> = Vec::new();
    let mut s_accum = 0.0_f32;
    let mut last: Option<(i32, i32)> = None;
    for pair in way.nodes.windows(2) {
        let (x1, z1) = (pair[0].x, pair[0].z);
        let (x2, z2) = (pair[1].x, pair[1].z);
        let len = (((x2 - x1) as f32).hypot((z2 - z1) as f32)).max(1e-6);
        let (ux, uz) = ((x2 - x1) as f32 / len, (z2 - z1) as f32 / len);
        for (x, _, z) in bresenham_line(x1, 0, z1, x2, 0, z2) {
            if last == Some((x, z)) {
                continue;
            }
            if let Some((lx, lz)) = last {
                s_accum += ((x - lx) as f32).hypot((z - lz) as f32);
            }
            points.push(AerowayCenterPoint {
                x,
                z,
                ux,
                uz,
                s: s_accum,
            });
            last = Some((x, z));
        }
    }

    // Pass 1: full surface, before markings. A runway's base may overwrite taxiway surface so it
    // wins crossings regardless of element order; a taxiway's base only fills empty cells (`None`),
    // so it never paints over a runway.
    let runway_overwrites = [LIGHT_GRAY_CONCRETE, YELLOW_CONCRETE];
    let base_over: Option<&[Block]> =
        (is_runway && unpaved.is_none()).then_some(&runway_overwrites[..]);
    for cp in &points {
        for dx in -half_width..=half_width {
            for dz in -half_width..=half_width {
                editor.set_block(base_block, cp.x + dx, 0, cp.z + dz, base_over, None);
            }
        }
    }
    if unpaved.is_some() {
        return;
    }

    // Pass 2: markings. `set_block` only overwrites a whitelisted block, so markings must list
    // the base surface they replace — else pass 1 has claimed every cell and they're dropped.
    let base_overwrite = [base_block];
    let over_base = Some(&base_overwrite[..]);
    for cp in &points {
        // Perpendicular unit vector across the strip.
        let (px, pz) = (-cp.uz, cp.ux);
        if is_runway {
            if runway_centerline_dash_on(cp.s, args.scale) {
                editor.set_block(WHITE_CONCRETE, cp.x, 0, cp.z, over_base, None);
            }
            let off = (half_width as f32 - RUNWAY_EDGE_INSET).max(0.0);
            for sign in [1.0_f32, -1.0] {
                let ex = (cp.x as f32 + sign * px * off).round() as i32;
                let ez = (cp.z as f32 + sign * pz * off).round() as i32;
                editor.set_block(WHITE_CONCRETE, ex, 0, ez, over_base, None);
            }
        } else if is_taxiway || is_stand_line {
            editor.set_block(YELLOW_CONCRETE, cp.x, 0, cp.z, over_base, None);
        }
    }
}

/// Default helipad radius (metres) for node helipads without geometry.
pub(crate) const HELIPAD_NODE_RADIUS_M: f64 = 8.0;
/// Ring diameter as a fraction of the pad's equivalent-area radius.
const HELIPAD_RING_FRACTION: f64 = 0.85;

/// Helipad surface: light-gray pad, white ring + "H", sometimes a parked helicopter.
fn paint_helipad(
    editor: &mut WorldEditor,
    cells: &[(i32, i32)],
    cx: i32,
    cz: i32,
    building_footprints: &CoordinateBitmap,
) {
    if cells.is_empty() {
        return;
    }
    // Mostly-rooftop pads are left to the building module.
    let covered = cells
        .iter()
        .filter(|&&(x, z)| building_footprints.contains(x, z))
        .count();
    if covered * 2 > cells.len() {
        return;
    }
    let r = ((cells.len() as f64) / std::f64::consts::PI).sqrt();
    let ring_r = (r * HELIPAD_RING_FRACTION).max(2.5);
    let bar_half_h = ((r * 0.45) as i32).clamp(2, 6);
    let bar_half_w = ((r * 0.30) as i32).clamp(1, 4);

    for &(x, z) in cells {
        if building_footprints.contains(x, z) {
            continue;
        }
        editor.set_block(LIGHT_GRAY_CONCRETE, x, 0, z, None, None);
    }

    let over_base = [LIGHT_GRAY_CONCRETE];
    for &(x, z) in cells {
        if building_footprints.contains(x, z) {
            continue;
        }
        let (dx, dz) = (x - cx, z - cz);
        let dist = ((dx * dx + dz * dz) as f64).sqrt();
        let on_ring = dist >= ring_r - 1.2 && dist < ring_r;
        let on_h = (dx.abs() == bar_half_w && dz.abs() <= bar_half_h)
            || (dz == 0 && dx.abs() <= bar_half_w);
        if on_ring || on_h {
            editor.set_block(WHITE_CONCRETE, x, 0, z, Some(&over_base), None);
        }
    }

    // Only pads with room for the skids get a helicopter.
    if r >= 5.0 && !building_footprints.contains(cx, cz) {
        crate::structures::helicopter::maybe_place_helicopter(editor, cx, cz);
    }
}

/// One centerline segment of an aeroway strip and the half-width it renders at, in blocks.
struct StripSegment {
    ax: f64,
    az: f64,
    bx: f64,
    bz: f64,
    half: f64,
}

impl StripSegment {
    fn contains(&self, x: f64, z: f64) -> bool {
        let (dx, dz) = (self.bx - self.ax, self.bz - self.az);
        let len2 = dx * dx + dz * dz;
        let t = if len2 > 0.0 {
            (((x - self.ax) * dx + (z - self.az) * dz) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (px, pz) = (self.ax + t * dx - x, self.az + t * dz - z);
        px * px + pz * pz <= self.half * self.half
    }
}

/// The strip `generate_aeroway` paves along a runway or taxiway, as segments.
fn push_strip_segments(out: &mut Vec<StripSegment>, way: &ProcessedWay, scale: f64) {
    let half_m = parse_width_tag_m(&way.tags)
        .map(|w| (w * 0.5).clamp(AEROWAY_MIN_HALF_M, AEROWAY_MAX_HALF_M))
        .unwrap_or(AEROWAY_DEFAULT_HALF_M);
    let half = (half_m * scale).round().max(1.0);
    for pair in way.nodes.windows(2) {
        out.push(StripSegment {
            ax: pair[0].x as f64,
            az: pair[0].z as f64,
            bx: pair[1].x as f64,
            bz: pair[1].z as f64,
            half,
        });
    }
}

/// Even-odd point in polygon for a closed ring.
fn ring_contains(ring: &[(i32, i32)], x: f64, z: f64) -> bool {
    let mut inside = false;
    for pair in ring.windows(2) {
        let ((x1, z1), (x2, z2)) = (
            (pair[0].0 as f64, pair[0].1 as f64),
            (pair[1].0 as f64, pair[1].1 as f64),
        );
        if (z1 > z) != (z2 > z) && x < x1 + (z - z1) * (x2 - x1) / (z2 - z1) {
            inside = !inside;
        }
    }
    inside
}

/// Removes the buildings that would stand on pavement aircraft use. Nothing is ever built on
/// a runway or stopway, so any building whose middle lies on one goes. Taxiways and aprons only
/// lose the buildings Overture traced from imagery, which there are mostly parked aircraft read
/// as roofs; a mapped hangar a taxiway line runs into stays. Returns how many were dropped.
///
/// Costs a scan of the elements plus, for the few buildings inside the airport's bounds, one
/// test per strip segment; nothing is allocated per block.
pub fn drop_buildings_on_aircraft_pavement(
    elements: &mut Vec<ProcessedElement>,
    scale: f64,
) -> usize {
    let mut runways: Vec<StripSegment> = Vec::new();
    let mut taxiways: Vec<StripSegment> = Vec::new();
    // Paved surfaces drawn as polygons: runway areas drop every building, the
    // rest (aprons, taxiway areas) only traced ones.
    let mut runway_areas: Vec<Vec<(i32, i32)>> = Vec::new();
    let mut aprons: Vec<Vec<(i32, i32)>> = Vec::new();
    for element in elements.iter() {
        let ProcessedElement::Way(way) = element else {
            continue;
        };
        let ring = || way.nodes.iter().map(|n| (n.x, n.z)).collect::<Vec<_>>();
        match aeroway_area(way) {
            Some(AerowayArea::Runway) => runway_areas.push(ring()),
            Some(AerowayArea::Apron) => aprons.push(ring()),
            None => match way.tags.get("aeroway").map(String::as_str) {
                Some("runway" | "stopway") => push_strip_segments(&mut runways, way, scale),
                Some("taxiway") => push_strip_segments(&mut taxiways, way, scale),
                _ => {}
            },
        }
    }
    if runways.is_empty() && taxiways.is_empty() && runway_areas.is_empty() && aprons.is_empty() {
        return 0;
    }

    // Everything that can drop a building lies inside these bounds.
    let (mut min_x, mut min_z, mut max_x, mut max_z) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for s in runways.iter().chain(&taxiways) {
        min_x = min_x.min(s.ax.min(s.bx) - s.half);
        max_x = max_x.max(s.ax.max(s.bx) + s.half);
        min_z = min_z.min(s.az.min(s.bz) - s.half);
        max_z = max_z.max(s.az.max(s.bz) + s.half);
    }
    for &(x, z) in aprons.iter().chain(&runway_areas).flatten() {
        min_x = min_x.min(x as f64);
        max_x = max_x.max(x as f64);
        min_z = min_z.min(z as f64);
        max_z = max_z.max(z as f64);
    }

    let before = elements.len();
    elements.retain(|element| {
        let ProcessedElement::Way(way) = element else {
            return true;
        };
        if way.nodes.len() < 3
            || !(way.tags.contains_key("building") || way.tags.contains_key("building:part"))
        {
            return true;
        }
        let n = way.nodes.len() as f64;
        let cx = way.nodes.iter().map(|p| p.x as f64).sum::<f64>() / n;
        let cz = way.nodes.iter().map(|p| p.z as f64).sum::<f64>() / n;
        if cx < min_x || cx > max_x || cz < min_z || cz > max_z {
            return true;
        }
        if runways.iter().any(|s| s.contains(cx, cz))
            || runway_areas.iter().any(|ring| ring_contains(ring, cx, cz))
        {
            return false;
        }
        let traced = way.tags.get("source").map(String::as_str) == Some("overture_maps");
        !(traced
            && (taxiways.iter().any(|s| s.contains(cx, cz))
                || aprons.iter().any(|ring| ring_contains(ring, cx, cz))))
    });
    before - elements.len()
}

/// Renders an `aeroway=helipad` way as a filled pad with markings.
fn generate_helipad_way(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    args: &Args,
    building_footprints: &CoordinateBitmap,
) {
    let outline: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
    let cells = flood_fill_area(&outline, None);
    if cells.is_empty() {
        // Open or degenerate geometry: fall back to a disc at the first node.
        if let Some(n) = way.nodes.first() {
            paint_helipad_disc(editor, n.x, n.z, args, building_footprints);
        }
        return;
    }
    let (mut sx, mut sz) = (0i64, 0i64);
    for &(x, z) in &cells {
        sx += x as i64;
        sz += z as i64;
    }
    let cx = (sx / cells.len() as i64) as i32;
    let cz = (sz / cells.len() as i64) as i32;
    // Concave pads can put the mean outside the polygon; snap to the nearest cell.
    let (cx, cz) = if cells.contains(&(cx, cz)) {
        (cx, cz)
    } else {
        *cells
            .iter()
            .min_by_key(|&&(x, z)| {
                let (dx, dz) = ((x - cx) as i64, (z - cz) as i64);
                dx * dx + dz * dz
            })
            .unwrap()
    };
    paint_helipad(editor, &cells, cx, cz, building_footprints);
}

/// Renders an `aeroway=helipad` node as a default-size disc pad.
pub fn generate_helipad_node(
    editor: &mut WorldEditor,
    node: &ProcessedNode,
    args: &Args,
    building_footprints: &CoordinateBitmap,
) {
    paint_helipad_disc(editor, node.x, node.z, args, building_footprints);
}

fn paint_helipad_disc(
    editor: &mut WorldEditor,
    cx: i32,
    cz: i32,
    args: &Args,
    building_footprints: &CoordinateBitmap,
) {
    let radius = ((HELIPAD_NODE_RADIUS_M * args.scale).round() as i32).max(4);
    let mut cells = Vec::new();
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            if dx * dx + dz * dz <= radius * radius {
                cells.push((cx + dx, cz + dz));
            }
        }
    }
    paint_helipad(editor, &cells, cx, cz, building_footprints);
}

/// Returns the half-width (block_range) for a highway type.
///
/// This extracts the same logic used inside `generate_highways_internal` so
/// that pre-scan passes (e.g. building-passage collection) can determine road
/// width without generating any blocks.
/// Default lane count per highway type when `lanes=*` is absent. OSM lanes
/// are the total for both directions, so 2 is a normal two-way street.
pub(crate) fn highway_default_lanes(highway_type: &str) -> i32 {
    match highway_type {
        "motorway" | "primary" | "trunk" | "secondary" | "tertiary" => 2,
        _ => 1,
    }
}

/// Lanes the paint divides a way into; 1 means no lane lines.
pub(crate) fn lane_marking_count(highway_type: &str, tags: &HashMap<String, String>) -> i32 {
    // Nothing is painted on a road that is still being built.
    if highway_type == "construction" {
        return 1;
    }
    let painted = tags.get("lane_markings").map(String::as_str);
    if painted == Some("no") || (highway_type == "raceway" && painted != Some("yes")) {
        return 1;
    }
    let lanes = tags
        .get("lanes")
        .and_then(|l| l.parse::<i32>().ok())
        // Most roundabouts are a single unmarked lane unless tagged otherwise.
        .unwrap_or_else(|| {
            if road_markings::is_roundabout(tags) {
                1
            } else {
                highway_default_lanes(highway_type)
            }
        })
        .clamp(1, 16);
    // Side streets, parking aisles and tracks are rarely painted, even with two lanes.
    let minor = matches!(
        highway_type,
        "residential" | "living_street" | "service" | "track"
    );
    if minor && lanes < 3 && painted != Some("yes") {
        return 1;
    }
    lanes
}

/// A lane line, as a signed offset from the centreline in the way's right-hand perpendicular.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LaneLine {
    offset: f32,
    solid: bool,
    block: Block,
    /// Keeps opposing traffic apart.
    centre: bool,
}

/// Lane lines across a carriageway `2 * block_range + 1` wide. Lanes are counted from the
/// left of the way's direction, so the line between opposing traffic sits after the
/// backward lanes, or after the forward ones where traffic keeps left.
fn lane_lines(
    highway_type: &str,
    tags: &HashMap<String, String>,
    block_range: i32,
    markings: &RoadMarkingIndex,
) -> Vec<LaneLine> {
    let lanes = lane_marking_count(highway_type, tags);
    if lanes < 2 {
        return Vec::new();
    }
    let two_way = road_markings::oneway_sign(highway_type, tags) == 0;
    let width = (2 * block_range + 1) as f32;
    let lane_width = width / lanes as f32;
    let parse = |k: &str| tags.get(k).and_then(|v| v.parse::<i32>().ok());
    // Lines between opposing traffic, either side of a shared turn lane if there is one.
    let both = parse("lanes:both_ways").unwrap_or(0).max(0);
    let separators: Vec<i32> = if !two_way {
        Vec::new()
    } else if let (Some(fw), Some(bw)) = (parse("lanes:forward"), parse("lanes:backward")) {
        let left = if markings.drives_on_left { fw } else { bw };
        if fw + bw + both == lanes && left > 0 && left + both < lanes {
            (left..=left + both).collect()
        } else {
            Vec::new()
        }
    } else if (lanes - both) % 2 == 0 && lanes > both + 1 {
        let left = (lanes - both) / 2;
        (left..=left + both).collect()
    } else {
        Vec::new()
    };
    let at = |l: i32| l as f32 * lane_width - width / 2.0;
    // A double yellow line needs a clear cell between its halves and room either side.
    let double = markings.yellow_centre && separators.len() == 1 && lane_width >= 3.5;
    let centre = |l: i32| {
        let line = |offset: f32| LaneLine {
            offset,
            centre: true,
            // Wide roads and North America keep opposing traffic apart with a solid line.
            solid: lanes >= 4 || markings.yellow_centre,
            block: if markings.yellow_centre {
                YELLOW_CONCRETE
            } else {
                WHITE_CONCRETE
            },
        };
        if double {
            vec![line(at(l) - 1.0), line(at(l) + 1.0)]
        } else {
            vec![line(at(l))]
        }
    };
    // Every lane keeps two clear cells between its lines, else only the centre is painted.
    if width + 1.0 < 3.0 * lanes as f32 {
        return separators.into_iter().flat_map(centre).collect();
    }
    (1..lanes)
        .flat_map(|l| {
            if separators.contains(&l) {
                centre(l)
            } else {
                vec![LaneLine {
                    offset: at(l),
                    solid: false,
                    block: WHITE_CONCRETE,
                    centre: false,
                }]
            }
        })
        .collect()
}

/// Dash and period of broken lane lines, in path cells: 3 m on, 6 m off in town and
/// twice that on fast roads, the proportions most countries paint.
fn dash_pattern(highway_type: &str, tags: &HashMap<String, String>, scale: f64) -> (u32, u32) {
    let (on, off) = if is_fast_road(highway_type, tags) {
        (6.0, 12.0)
    } else {
        (3.0, 6.0)
    };
    let on = (on * scale).round().max(1.0) as u32;
    let off = (off * scale).round().max(1.0) as u32;
    (on, on + off)
}

/// Motorways, and roads posted at 70 km/h or more or tagged as rural.
fn is_fast_road(highway_type: &str, tags: &HashMap<String, String>) -> bool {
    if matches!(
        highway_type,
        "motorway" | "trunk" | "motorway_link" | "trunk_link"
    ) {
        return true;
    }
    if let Some(speed) = tags.get("maxspeed") {
        let digits: String = speed.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(v) = digits.parse::<f64>() {
            let kmh = if speed.contains("mph") { v * 1.609 } else { v };
            return kmh >= 70.0;
        }
        if speed == "none" {
            return true;
        }
    }
    [
        "maxspeed",
        "maxspeed:type",
        "source:maxspeed",
        "zone:traffic",
    ]
    .iter()
    .filter_map(|k| tags.get(*k))
    .any(|v| v.contains("rural") || v.contains("motorway") || v.contains("nsl"))
}

/// Canonical road half-width in blocks, kerb to kerb. Single source of truth shared by the
/// renderer and the prescan/bitmap/bridge consumers, so they never disagree. The rules are
/// in `road_width`.
pub(crate) fn highway_block_range(
    highway_type: &str,
    tags: &HashMap<String, String>,
    scale: f64,
) -> i32 {
    road_width::road_halves(highway_type, tags, scale).kerb
}

/// Half-width in blocks of a road's traffic lanes, which its lane lines divide and cars
/// drive in: the kerb-to-kerb width less the cars parked along it.
pub(crate) fn highway_lane_range(
    highway_type: &str,
    tags: &HashMap<String, String>,
    scale: f64,
) -> i32 {
    road_width::road_halves(highway_type, tags, scale).lanes
}

/// Whether the cell `(dx, dz)` from a centreline cell lies in the stamp of a road of
/// half-width `half`. A disc, so a road is as wide across on a diagonal as along an axis;
/// a square came out about a quarter wider at 30 to 45 degrees. Footways (half 1) still
/// stamp their full 3x3.
pub(crate) fn in_road_stamp(dx: i32, dz: i32, half: i32) -> bool {
    dx * dx + dz * dz <= half * half + half
}

/// Whether a way is stamped with a square rather than `in_road_stamp`'s disc: a bridge deck,
/// whose railings, parapets and piers are laid out from the square. Its approach ramps take
/// the disc like any road. Decided from the way alone, so the pre-scan masks and the
/// renderer agree.
pub(crate) fn stamps_square(way: &ProcessedWay) -> bool {
    is_bridge_way(way) && lays_deck(way)
}

/// The half-width a road keeps where a building stands against it, a verge with its grass
/// or a street tree, or a sidewalk mapped as its own way, when narrower than the road: its
/// lanes inside the parking rows, a two-way street with a sidewalk the width it would have
/// without one. What the road gains past that, as in a narrow old-town street whose house
/// fronts are mapped wider than they stand, stops at the walls instead of paving the ground
/// floors, leaves the verge green and the sidewalk paved. None for a road paved whole: a
/// bridge deck, or a passage through a building.
pub(crate) fn wall_core(way: &ProcessedWay, highway_type: &str, scale: f64) -> Option<i32> {
    if stamps_square(way) || way.tags.get("tunnel").map(String::as_str) == Some("building_passage")
    {
        return None;
    }
    let kerb = road_width::road_halves(highway_type, &way.tags, scale).kerb;
    let core = road_width::wall_half(highway_type, &way.tags, scale);
    (core < kerb).then_some(core)
}

/// How far a road's stamp reaches from a centreline cell along the unit direction
/// `(px, pz)`: the furthest cell centre it covers that way.
pub(crate) fn stamp_reach(half: i32, square: bool, px: f32, pz: f32) -> f32 {
    if square {
        return half as f32 * (px.abs() + pz.abs());
    }
    let mut reach = 0f32;
    for dx in -half..=half {
        for dz in -half..=half {
            if in_road_stamp(dx, dz, half) {
                reach = reach.max(dx as f32 * px + dz as f32 * pz);
            }
        }
    }
    reach
}

/// The road half of `collect_road_and_carriageway_coords`.
#[cfg(test)]
pub fn collect_road_surface_coords(
    elements: &[ProcessedElement],
    editor: &WorldEditor,
    xzbbox: &XZBBox,
    scale: f64,
    water_detail: crate::water_detail::WaterDetail,
) -> CoordinateBitmap {
    collect_road_and_carriageway_coords(
        elements,
        editor,
        xzbbox,
        scale,
        water_detail,
        &CoordinateBitmap::new_empty(),
    )
    .roads
}

/// True for highway types that carry vehicles rather than people on foot, bike or horse.
pub(crate) fn is_carriageway(highway_type: &str) -> bool {
    !(is_walkway(highway_type)
        || matches!(highway_type, "corridor" | "track" | "elevator" | "platform"))
}

/// The type of a highway way the renderer stamps as a surface; tunnels still need
/// `tunnel_renders_at_surface`.
pub(crate) fn surface_highway_type(way: &ProcessedWay) -> Option<&str> {
    let highway_type = way.tags.get("highway")?.as_str();
    // Node-only types have no surface.
    if matches!(highway_type, "street_lamp" | "crossing" | "bus_stop") {
        return None;
    }
    // Area highways (pedestrian plazas etc.) are flood-filled separately.
    if way.tags.get("area").is_some_and(|v| v == "yes") {
        return None;
    }
    // Same indoor guard as generate_highways_internal.
    if way.tags.get("indoor").is_some_and(|v| v == "yes") {
        return None;
    }
    // Jet bridges render as a schematic, so the apron under them is not road.
    if way.tags.get("aeroway").is_some_and(|v| v == "jet_bridge") {
        return None;
    }
    // A pier path is drawn as the deck, and the water under it is not road.
    if way.tags.get("man_made").is_some_and(|v| v == "pier") {
        return None;
    }
    // Negative levels are indoor mapping.
    if way
        .tags
        .get("level")
        .and_then(|l| l.parse::<i32>().ok())
        .is_some_and(|l| l < 0)
    {
        return None;
    }
    Some(highway_type)
}

/// Tunnels render underground, unless there is no room to bore one.
pub(crate) fn tunnel_renders_at_surface(
    way: &ProcessedWay,
    editor: Option<&WorldEditor>,
    scale: f64,
) -> bool {
    !renders_as_highway_tunnel(way) || editor.is_some_and(|e| !tunnel_bore_fits(e, way, scale))
}

/// Collect all (x, z) coordinates that are covered by any rendered road or path
/// surface, and in a second bitmap the carriageways among them (their lanes, without the
/// parking rows along the kerbs), in one pass over the roads. The first bitmap has 1 for every block that the highway renderer
/// places as a road/path surface and 0 everywhere else.
///
/// Geometry is computed identically to `generate_highways_internal`:
/// - Bresenham line between each consecutive pair of OSM nodes
/// - Stamped with `block_range` around each cell (same value and shape as the renderer)
/// - `area=yes` ways, indoor ways, negative-level ways, and pure node types
///   (street_lamp, crossing, bus_stop) are excluded, matching the renderer's
///   early-return guards.
///
/// This lets `get_nearest_road_block` in `amenities.rs` or other processors do a single O(1) bitmap lookup
/// instead of live `get_ground_level` + `check_for_block_absolute` world scans.
///
/// `water_detail` drops the water cells of short crossings, which the renderer then skips.
pub fn collect_road_and_carriageway_coords(
    elements: &[ProcessedElement],
    editor: &WorldEditor,
    xzbbox: &XZBBox,
    scale: f64,
    water_detail: crate::water_detail::WaterDetail,
    building_footprints: &CoordinateBitmap,
) -> RoadCoords {
    let editor = Some(editor);
    // The sidewalks first, which the roads beside them then stop at; read only where a road
    // has a strip past its core to stop.
    let widens = elements.iter().any(|e| match e {
        ProcessedElement::Way(w) => {
            surface_highway_type(w).is_some_and(|t| wall_core(w, t, scale).is_some())
        }
        _ => false,
    });
    let junctions = road_junctions(elements);
    let mut paths = CoordinateBitmap::new_empty();
    if widens {
        let mut found = CoordinateBitmap::new(xzbbox);
        let mut any = false;
        stamp_highway_surfaces(
            elements,
            editor,
            scale,
            water_detail,
            building_footprints,
            None,
            |_| true,
            false,
            |carriageway, x, z| {
                if !carriageway {
                    found.set(x, z);
                    any = true;
                }
            },
            |_, _| {},
        );
        if any {
            paths = found;
        }
    }
    // The ways in two halves, by the parity of their ids, each on a thread of its own with
    // masks of its own, which then come together as one.
    let half = |parity: u64| {
        let mut roads = CoordinateBitmap::new(xzbbox);
        let mut carriageways = CoordinateBitmap::new(xzbbox);
        let mut cut: Vec<u64> = Vec::new();
        stamp_highway_surfaces(
            elements,
            editor,
            scale,
            water_detail,
            building_footprints,
            Some(&paths),
            |e| matches!(e, ProcessedElement::Way(w) if w.id % 2 == parity),
            false,
            |carriageway, x, z| {
                roads.set(x, z);
                if carriageway {
                    carriageways.set(x, z);
                }
            },
            |x, z| cut.push(pack_xz(x, z)),
        );
        (roads, carriageways, cut)
    };
    let ((mut roads, mut carriageways, mut cut), (odd_roads, odd_carriageways, odd_cut)) =
        rayon::join(|| half(0), || half(1));
    roads.union_with(&odd_roads);
    carriageways.union_with(&odd_carriageways);
    drop((odd_roads, odd_carriageways));
    cut.extend(odd_cut);
    cut.sort_unstable();
    cut.dedup();
    cut.retain(|&c| !roads.contains((c >> 32) as u32 as i32, c as u32 as i32));
    cut.shrink_to_fit();
    RoadCoords {
        roads,
        carriageways,
        paths,
        cut: CutCells(cut),
        junctions,
    }
}

/// What `collect_road_and_carriageway_coords` finds.
pub struct RoadCoords {
    /// Every road and path surface cell.
    pub roads: CoordinateBitmap,
    /// The lanes among them, without the parking rows along the kerbs.
    pub carriageways: CoordinateBitmap,
    /// The sidewalks mapped as their own ways, which a road's parking rows and second lane
    /// stop at; empty where no road widens past its core or no such sidewalk is mapped.
    pub paths: CoordinateBitmap,
    /// The cells a road's stamp covers but leaves unpaved beside it, which no other way
    /// paves.
    pub cut: CutCells,
    /// Where three or more road arms meet, packed and sorted, as `road_junctions` finds them.
    pub junctions: Vec<u64>,
}

/// Cells a road's stamp covers but `cut_beside` leaves unpaved, packed and sorted.
#[derive(Default)]
pub struct CutCells(Vec<u64>);

impl CutCells {
    pub fn contains(&self, x: i32, z: i32) -> bool {
        self.0.binary_search(&pack_xz(x, z)).is_ok()
    }
}

/// Whether a road leaves (x, z), past the core it keeps against what stands beside it,
/// unpaved: a building stands there, a sidewalk is mapped there as its own way (`on_path`),
/// water is there, or with `verges` the verge keeps its ground; without, as along rows of
/// parked cars, only a street tree keeps its pit. `in_core` says whether a cell lies in the
/// road's own core, where no tree will stand.
pub(crate) fn cut_beside(
    in_core: impl Fn(i32, i32) -> bool,
    footprints: &CoordinateBitmap,
    editor: Option<&WorldEditor>,
    on_path: impl Fn(i32, i32) -> bool,
    verges: bool,
    x: i32,
    z: i32,
) -> bool {
    let paved = |sx: i32, sz: i32| footprints.contains(sx, sz) || in_core(sx, sz);
    footprints.contains(x, z)
        || on_path(x, z)
        || editor.is_some_and(|e| {
            e.water_at(x, z)
                || if verges {
                    e.keeps_its_ground(x, z, paved)
                } else {
                    e.keeps_a_pit(x, z, paved)
                }
        })
}

/// What a question that depends on the cell alone, such as whether `cut_beside` cuts it,
/// answered for the cells of one way lately: a small cache with a slot per cell of a square
/// `SIDE` across, laid over the world like tiles, so the cells round the stamp being laid
/// never share one. A cell it lost is only worked out again.
struct CellMemo<T> {
    slots: Vec<(i32, i32, T)>,
}

impl<T> Default for CellMemo<T> {
    fn default() -> Self {
        Self { slots: Vec::new() }
    }
}

type CutMemo = CellMemo<bool>;

impl<T: Copy + Default> CellMemo<T> {
    /// Wider than most roads' stamps and the stall lines beside them.
    const SIDE: i32 = 32;

    /// The answer for (x, z), from `find` where none is kept.
    fn get_or(&mut self, x: i32, z: i32, find: impl FnOnce() -> T) -> T {
        if self.slots.is_empty() {
            self.slots =
                vec![(i32::MIN, i32::MIN, T::default()); (Self::SIDE * Self::SIDE) as usize];
        }
        let at = z.rem_euclid(Self::SIDE) * Self::SIDE + x.rem_euclid(Self::SIDE);
        let slot = &mut self.slots[at as usize];
        if (slot.0, slot.1) != (x, z) {
            *slot = (x, z, find());
        }
        slot.2
    }
}

/// The cell one step nearer the centreline cell on the line out to `(dx, dz)`.
fn inward(dx: i32, dz: i32) -> (i32, i32) {
    let len = f64::from(dx * dx + dz * dz).sqrt();
    let keep = (len - 1.0) / len;
    (
        (f64::from(dx) * keep).round() as i32,
        (f64::from(dz) * keep).round() as i32,
    )
}

/// Whether a road's stamp leaves its cell `(dx, dz)` past the core unpaved: where `cut` says
/// that cell is, or one between it and the core on the line out to it. A cut runs outward,
/// so no patch of road is left standing beyond the ground it stops at.
pub(crate) fn cut_outward(
    core: i32,
    dx: i32,
    dz: i32,
    mut cut: impl FnMut(i32, i32) -> bool,
) -> bool {
    let (mut cx, mut cz) = (dx, dz);
    while !in_road_stamp(cx, cz, core) {
        if cut(cx, cz) {
            return true;
        }
        let next = inward(cx, cz);
        if next == (cx, cz) {
            break;
        }
        (cx, cz) = next;
    }
    false
}

/// Whether the stamp of a centreline cell on a way running along `u` covers its cell
/// `(dx, dz)`: out to the kerb on that cell's side of the way, `sides` being how far the
/// left kerb and the right one reach, so a row of cars parked on one side widens that side.
pub(crate) fn in_side_stamp(dx: i32, dz: i32, u: Option<(f32, f32)>, sides: (i32, i32)) -> bool {
    let wide = sides.0.max(sides.1);
    let half = match u {
        Some(u) if sides.0 != sides.1 => {
            let right = dx as f32 * -u.1 + dz as f32 * u.0;
            if right > 0.01 {
                sides.1
            } else if right < -0.01 {
                sides.0
            } else {
                wide
            }
        }
        _ => wide,
    };
    in_road_stamp(dx, dz, half)
}

/// How far past a road's kerb, from a junction node, the cars turning there sweep the
/// corner, where nobody parks.
const TURN_SWEEP: i32 = 4;

/// The width along the kerb of a stall marked in a row of cars parked across the kerb or at
/// an angle.
const STALL_M: f64 = 2.5;

/// Where three or more road arms meet, as packed cells: the junctions across whose mouths
/// nobody parks. A way's middle node is two arms of it, an end one.
pub(crate) fn road_junctions(elements: &[ProcessedElement]) -> Vec<u64> {
    let mut arms: FnvHashMap<u64, u8> = FnvHashMap::default();
    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };
        if !surface_highway_type(way).is_some_and(is_carriageway) {
            continue;
        }
        let last = way.nodes.len().saturating_sub(1);
        for (i, n) in way.nodes.iter().enumerate() {
            let a = arms.entry(pack_xz(n.x, n.z)).or_default();
            *a = a.saturating_add(u8::from(i > 0) + u8::from(i < last));
        }
    }
    let mut out: Vec<u64> = arms
        .into_iter()
        .filter(|&(_, a)| a >= 3)
        .map(|(c, _)| c)
        .collect();
    out.sort_unstable();
    out
}

/// How far from a centreline cell the disc stamp of half-width `half` reaches.
pub(crate) fn stamp_radius(half: i32) -> f32 {
    ((half * half + half) as f32).sqrt()
}

/// Whether (x, z) lies within the disc stamp of half-width `half` along the way's
/// centreline, as near as the segments between its nodes tell.
fn near_centreline(way: &ProcessedWay, half: i32, x: i32, z: i32) -> bool {
    let reach = stamp_radius(half);
    let (px, pz) = (x as f32, z as f32);
    way.nodes.windows(2).any(|w| {
        let (ax, az) = (w[0].x as f32, w[0].z as f32);
        let (dx, dz) = (w[1].x as f32 - ax, w[1].z as f32 - az);
        let len_sq = dx * dx + dz * dz;
        let t = if len_sq > 0.0 {
            (((px - ax) * dx + (pz - az) * dz) / len_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (px - ax - t * dx).hypot(pz - az - t * dz) <= reach
    })
}

/// A sidewalk mapped as its own way at grade, which a road's parking rows and second lane
/// stop at: a footway, cycle path or path tagged as a sidewalk or a side path, so running
/// alongside the road. A path that only joins or crosses the street, and a footbridge or a
/// walkway on another level, do not cut it.
pub(crate) fn is_mapped_path(way: &ProcessedWay, highway_type: &str) -> bool {
    let tag = |k: &str| way.tags.get(k).map(String::as_str);
    let alongside = ["footway", "cycleway", "path"]
        .iter()
        .any(|k| tag(k) == Some("sidewalk"))
        || tag("is_sidepath") == Some("yes");
    let at_grade = !is_bridge_way(way)
        && tag("tunnel").is_none_or(|t| t == "no")
        && ["layer", "level"]
            .iter()
            .all(|k| tag(k).is_none_or(|v| v.trim().parse::<f64>().is_ok_and(|l| l == 0.0)));
    !is_carriageway(highway_type) && alongside && at_grade
}

/// Where a garden keeps a few metres of paving: the carriageways and pedestrian streets
/// as the renderer stamps them, less driveways, which a garden comes up to like a path,
/// and the surface car parks. Squares and other paved areas come from
/// `landuse::mark_paved_areas`.
pub fn collect_garden_streets(
    elements: &[ProcessedElement],
    editor: &WorldEditor,
    xzbbox: &XZBBox,
    scale: f64,
    water_detail: crate::water_detail::WaterDetail,
    car_park_cells: impl Fn(&ProcessedWay) -> Option<std::sync::Arc<Vec<(i32, i32)>>>,
) -> CoordinateBitmap {
    let mut streets = CoordinateBitmap::new(xzbbox);
    // Only the lanes are read, which no building, sidewalk or verge cuts, so the rest of
    // each stamp is skipped.
    let none = CoordinateBitmap::new_empty();
    stamp_highway_surfaces(
        elements,
        Some(editor),
        scale,
        water_detail,
        &none,
        Some(&none),
        |element| {
            !matches!(element, ProcessedElement::Way(w)
                if w.tags.get("service").map(String::as_str) == Some("driveway"))
        },
        true,
        |carriageway, x, z| {
            if carriageway {
                streets.set(x, z);
            }
        },
        |_, _| {},
    );
    // A pedestrian street is a square drawn along a line: shop fronts meet it as they
    // meet a street.
    stamp_highway_surfaces(
        elements,
        Some(editor),
        scale,
        water_detail,
        &none,
        Some(&none),
        |element| {
            matches!(element, ProcessedElement::Way(w)
                if w.tags.get("highway").map(String::as_str) == Some("pedestrian"))
        },
        false,
        |_, x, z| streets.set(x, z),
        |_, _| {},
    );
    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };
        if is_surface_car_park(&way.tags) {
            for &(x, z) in car_park_cells(way).iter().flat_map(|cells| cells.iter()) {
                streets.set(x, z);
            }
        }
    }
    streets
}

/// A car park on the ground: `amenity=parking`, not in a building, under or on one.
pub(crate) fn is_surface_car_park(tags: &HashMap<String, String>) -> bool {
    tags.get("amenity").map(String::as_str) == Some("parking")
        && !tags.contains_key("building")
        && !matches!(
            tags.get("parking").map(String::as_str),
            Some("multi-storey" | "underground" | "rooftop")
        )
}

/// The road and path surfaces at grade: what `collect_road_and_carriageway_coords` stamps
/// of the ways `keep` lets through, less every bridge deck, the ends where an approach
/// meets one included. From the tags alone, before the ground is final, so a tunnel counts
/// as bored and no crossing is drowned. Line waterways are piped under these cells with a
/// water option on (`Ground::pipe_channels_under_roads`); under a deck they stay open, so
/// the bridge still finds its water.
pub fn collect_roads_at_grade(
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
    scale: f64,
    keep: impl Fn(&ProcessedElement) -> bool,
) -> CoordinateBitmap {
    let deck = |element: &ProcessedElement| matches!(element, ProcessedElement::Way(w) if is_bridge_way(w) && lays_deck(w));
    let mut roads = CoordinateBitmap::new(xzbbox);
    let water = crate::water_detail::WaterDetail::Default;
    // Before the buildings are filled: a widened road keeps the cells it gives up to a wall,
    // which the building covers anyway. The sidewalks mapped as their own ways first, which
    // the roads beside them stop at, as they do in the drawing.
    let no_footprints = CoordinateBitmap::new(xzbbox);
    let mut paths = CoordinateBitmap::new(xzbbox);
    stamp_highway_surfaces(
        elements,
        None,
        scale,
        water,
        &no_footprints,
        None,
        &keep,
        false,
        |_, x, z| paths.set(x, z),
        |_, _| {},
    );
    stamp_highway_surfaces(
        elements,
        None,
        scale,
        water,
        &no_footprints,
        Some(&paths),
        |element| keep(element) && !matches!(element, ProcessedElement::Way(w) if is_bridge_way(w)),
        false,
        |_, x, z| roads.set(x, z),
        |_, _| {},
    );
    stamp_highway_surfaces(
        elements,
        None,
        scale,
        water,
        &no_footprints,
        Some(&paths),
        |element| keep(element) && deck(element),
        false,
        |_, x, z| roads.clear(x, z),
        |_, _| {},
    );
    roads
}

/// Hands `stamp` every road and path surface cell of the elements `keep` lets through, with
/// whether its way carries vehicles, and `skipped` every cell a road's stamp covers but
/// `cut_beside` leaves unpaved. Without `paths`, only the sidewalks mapped as their own
/// ways; with them, every way, a road's parking rows and second lane stopping at those
/// sidewalks. With `lanes_only`, only the carriageway lanes, for a caller that reads nothing
/// else: the rest of each stamp is neither stamped nor looked into for a cut.
///
/// The cells come as a union: one may be handed over more than once, and where the stamps
/// along a way overlap, a cell the core of one already handed over, lanes and all, is not
/// handed over again for the next.
#[allow(clippy::too_many_arguments)]
fn stamp_highway_surfaces(
    elements: &[ProcessedElement],
    editor: Option<&WorldEditor>,
    scale: f64,
    water_detail: crate::water_detail::WaterDetail,
    building_footprints: &CoordinateBitmap,
    paths: Option<&CoordinateBitmap>,
    keep: impl Fn(&ProcessedElement) -> bool,
    lanes_only: bool,
    mut stamp: impl FnMut(bool, i32, i32),
    mut skipped: impl FnMut(i32, i32),
) {
    // A way that meets a bridge is its ramp or its approach, and keeps its surface.
    let drowning = editor.filter(|_| water_detail.drowns_crossing(scale, &HashMap::new()));
    let bridge_nodes: HashSet<(i32, i32)> = match drowning {
        Some(_) => elements
            .iter()
            .filter_map(|e| match e {
                ProcessedElement::Way(w)
                    if w.tags.contains_key("highway")
                        && w.tags.get("bridge").is_some_and(|b| b != "no") =>
                {
                    Some(w.nodes.iter().map(|n| (n.x, n.z)))
                }
                _ => None,
            })
            .flatten()
            .collect(),
        None => HashSet::new(),
    };

    let mut stamp_cells: Vec<(i32, i32, bool, bool)> = Vec::new();
    for element in elements.iter().filter(|e| keep(e)) {
        let ProcessedElement::Way(way) = element else {
            continue;
        };

        let Some(highway_type) = surface_highway_type(way) else {
            continue;
        };
        if paths.is_none() && !is_mapped_path(way, highway_type) {
            continue;
        }
        if !tunnel_renders_at_surface(way, editor, scale) {
            continue;
        }

        // Use the same block_range the renderer uses for this highway type
        let block_range = highway_block_range(highway_type, &way.tags, scale);
        // Only the lanes count as carriageway; the parking rows outside them do not.
        let lane_range = highway_lane_range(highway_type, &way.tags, scale);
        let carriageway = is_carriageway(highway_type);
        if lanes_only && !carriageway {
            continue;
        }
        let drowned = drowning.filter(|_| {
            water_detail.drowns_crossing(scale, &way.tags)
                && !way.nodes.iter().any(|n| bridge_nodes.contains(&(n.x, n.z)))
        });

        let mut points: Vec<(i32, i32)> = Vec::new();
        // The segment each point lies on, by its first point and its direction.
        let mut segments: Vec<(usize, Option<(f32, f32)>)> = Vec::new();
        for i in 1..way.nodes.len() {
            let prev = way.nodes[i - 1].xz();
            let cur = way.nodes[i].xz();
            let (sx, sz) = ((cur.x - prev.x) as f32, (cur.z - prev.z) as f32);
            let len = sx.hypot(sz);
            segments.push((points.len(), (len > 0.0).then(|| (sx / len, sz / len))));
            points.extend(
                bresenham_line(prev.x, 0, prev.z, cur.x, 0, cur.z)
                    .into_iter()
                    .map(|(x, _, z)| (x, z)),
            );
        }
        // Only water the carve fills is given up, so no cell ends as neither road nor water.
        let fills =
            |e: &WorldEditor, x: i32, z: i32| e.is_lc_water(x, z) && e.carved_bed(x, z).is_some();
        let drowned_points = drowned.map(|e| {
            crate::water_detail::drowned_points(&points, block_range, |x, z| fills(e, x, z))
        });

        let square = stamps_square(way);
        let kept_by_walls = wall_core(way, highway_type, scale);
        let verges = !road_width::parks_along(&way.tags);
        let sides = road_width::kerb_sides(highway_type, &way.tags, scale);
        // The stamp's cells, worked out once a segment (once a way, its kerbs alike): whether
        // each lies past the core, where the road stops at what stands beside it, and whether
        // it is carriageway.
        let cells_along = |u: Option<(f32, f32)>, cells: &mut Vec<(i32, i32, bool, bool)>| {
            cells.clear();
            for dx in -block_range..=block_range {
                for dz in -block_range..=block_range {
                    if !square && !in_side_stamp(dx, dz, u, sides) {
                        continue;
                    }
                    let past_core = kept_by_walls.is_some_and(|core| !in_road_stamp(dx, dz, core));
                    let in_lanes = if square {
                        dx.abs() <= lane_range && dz.abs() <= lane_range
                    } else {
                        in_road_stamp(dx, dz, lane_range)
                    };
                    if lanes_only && !in_lanes {
                        continue;
                    }
                    cells.push((dx, dz, past_core, carriageway && in_lanes));
                }
            }
        };
        let lopsided = !square && sides.0 != sides.1;
        if !lopsided {
            cells_along(None, &mut stamp_cells);
        }
        let mut segment = 0;
        // Whether a cell past the core is cut is the cell's own, and the stamps along a way
        // overlap: what one point found, by offset in its stamp, the next reads back. The
        // tables by offset reach a block past the stamp's square, so the offset a cell lay at
        // in the stamp a step back is its own index shifted.
        let reach = block_range + 1;
        let side = (2 * reach + 1) as usize;
        let at = |dx: i32, dz: i32| (dz + reach) as usize * side + (dx + reach) as usize;
        let (mut found, mut last_found) = match kept_by_walls {
            Some(_) => (
                vec![Verdict::Unknown; side * side],
                vec![Verdict::Unknown; side * side],
            ),
            None => (Vec::new(), Vec::new()),
        };
        // The walk `cut_outward` takes in from each offset of the stamp's square: the next
        // offset in, `IN_CORE` at the core and `END` where it stops short of it. Worked out
        // once a way.
        const IN_CORE: usize = usize::MAX;
        const END: usize = usize::MAX - 1;
        let offset = |k: usize| ((k % side) as i32 - reach, (k / side) as i32 - reach);
        let walk_in: Vec<usize> = kept_by_walls.map_or_else(Vec::new, |core| {
            (0..side * side)
                .map(|k| {
                    let (dx, dz) = offset(k);
                    if in_road_stamp(dx, dz, core) {
                        return IN_CORE;
                    }
                    let (nx, nz) = inward(dx, dz);
                    if (nx, nz) == (dx, dz) {
                        END
                    } else {
                        at(nx, nz)
                    }
                })
                .collect()
        });
        let offsets: Vec<(i32, i32)> = (0..walk_in.len()).map(offset).collect();
        // What each offset of the stamp's square is known to have handed over, as of the last
        // point and of this one: 0 nothing, 1 the cell, 2 the cell as a lane. The core's cells
        // always are, the sweep handing them over; a cell past the core is where its walk in
        // finds no cut, or where the stamp a step back had handed it over already, which then
        // is not looked into again, the cells coming as a union. A drowned crossing, which
        // gives up cells to the water point by point, looks into every one.
        let levels_of = |cells: &[(i32, i32, bool, bool)], base: &mut Vec<u8>| {
            base.clear();
            base.resize(walk_in.len(), 0);
            if drowned.is_none() && !walk_in.is_empty() {
                for &(dx, dz, _, lanes) in cells.iter().filter(|c| !c.2) {
                    base[at(dx, dz)] = 1 + u8::from(lanes);
                }
            }
        };
        let mut base: Vec<u8> = Vec::new();
        if !lopsided {
            levels_of(&stamp_cells, &mut base);
        }
        let (mut covered, mut last_covered) = (vec![0u8; walk_in.len()], vec![0u8; walk_in.len()]);
        // Whether each offset's walk in meets a cut, as this point finds it: 0 not yet known,
        // 1 no, 2 yes.
        let mut cut_at = vec![0u8; walk_in.len()];
        let mut walked: Vec<usize> = Vec::new();
        // The core's cells, which no cut reaches, are swept along the way; a drowned crossing,
        // which gives up some of them point by point, lays them at every point.
        let mut sweep: Option<Sweep> = None;
        let mut last: Option<(i32, i32)> = None;
        for (i, &(bx, bz)) in points.iter().enumerate() {
            let skip_water = drowned_points.as_ref().is_some_and(|d| d[i]);
            if lopsided && segments.get(segment).is_some_and(|&(start, _)| start == i) {
                while segments
                    .get(segment + 1)
                    .is_some_and(|&(start, _)| start == i)
                {
                    segment += 1;
                }
                cells_along(segments[segment].1, &mut stamp_cells);
                levels_of(&stamp_cells, &mut base);
                sweep = None;
                segment += 1;
            }
            if drowned.is_none() {
                let sweep = sweep.get_or_insert_with(|| {
                    Sweep::new(
                        block_range,
                        stamp_cells
                            .iter()
                            .filter(|c| !c.2)
                            .map(|&(dx, dz, _, lanes)| (dx, dz, lanes))
                            .collect(),
                    )
                });
                for &(dx, dz, lanes) in sweep.step_to(bx, bz) {
                    stamp(lanes, bx + dx, bz + dz);
                }
            } else {
                for &(dx, dz, _, lanes) in stamp_cells.iter().filter(|c| !c.2) {
                    let (x, z) = (bx + dx, bz + dz);
                    if skip_water && drowned.is_some_and(|e| fills(e, x, z)) {
                        continue;
                    }
                    stamp(lanes, x, z);
                }
            }
            let Some(core) = kept_by_walls else {
                continue;
            };
            found.fill(Verdict::Unknown);
            cut_at.fill(0);
            covered.copy_from_slice(&base);
            // Where an offset here lay in the stamp a step back, the last point being a step away.
            let shift = last
                .map(|(lx, lz)| (bx - lx, bz - lz))
                .filter(|&(sx, sz)| sx.abs() <= 1 && sz.abs() <= 1)
                .map(|(sx, sz)| sz as isize * side as isize + sx as isize);
            let back_of = |k: usize| shift.map(|d| k.wrapping_add_signed(d));
            for &(dx, dz, _, lanes) in stamp_cells.iter().filter(|c| c.2) {
                let k = at(dx, dz);
                let wanted = 1 + u8::from(lanes);
                let before = if drowned.is_none() {
                    back_of(k).map_or(0, |j| last_covered[j])
                } else {
                    0
                };
                covered[k] = before;
                if before >= wanted {
                    continue;
                }
                let (x, z) = (bx + dx, bz + dz);
                if skip_water && drowned.is_some_and(|e| fills(e, x, z)) {
                    continue;
                }
                // `cut_outward`'s walk in, stopping at the first offset whose cut is known.
                let mut j = k;
                let cut = loop {
                    if walk_in[j] == IN_CORE {
                        break false;
                    }
                    if cut_at[j] != 0 {
                        break cut_at[j] == 2;
                    }
                    if found[j] == Verdict::Unknown {
                        found[j] = back_of(j).map_or(Verdict::Unknown, |o| last_found[o]);
                    }
                    if found[j] == Verdict::Unknown {
                        let (cx, cz) = offsets[j];
                        found[j] = if cut_beside(
                            |sx, sz| near_centreline(way, core, sx, sz),
                            building_footprints,
                            editor,
                            |px, pz| paths.is_some_and(|p| p.contains(px, pz)),
                            verges,
                            bx + cx,
                            bz + cz,
                        ) {
                            Verdict::Cut
                        } else {
                            Verdict::Kept
                        };
                    }
                    walked.push(j);
                    if found[j] == Verdict::Cut {
                        break true;
                    }
                    if walk_in[j] == END {
                        break false;
                    }
                    j = walk_in[j];
                };
                for w in walked.drain(..) {
                    cut_at[w] = 1 + u8::from(cut);
                }
                if cut {
                    skipped(x, z);
                } else {
                    stamp(lanes, x, z);
                    covered[k] = wanted;
                }
            }
            std::mem::swap(&mut covered, &mut last_covered);
            std::mem::swap(&mut found, &mut last_found);
            last = Some((bx, bz));
        }
    }
}

/// One stamp laid at every point of a run along a way, a block apart. At each point it hands
/// over only the cells the stamp a block back left out, or covered off the lanes where they
/// are lanes here: the run's cells come out as the union of its stamps, each with its lanes,
/// for a fraction of the whole stamp at every point.
pub(crate) struct Sweep {
    half: i32,
    side: usize,
    /// By offset in the stamp's square: 0 off the stamp, 1 on it, 2 on its lanes.
    member: Vec<u8>,
    cells: Vec<(i32, i32, bool)>,
    /// The cells handed over after each of the nine steps, worked out as they come.
    fresh: [Option<Vec<(i32, i32, bool)>>; 9],
    last: Option<(i32, i32)>,
}

impl Sweep {
    /// A sweep of `cells`: offsets within `half` of the point along both axes, with whether
    /// each is a lane.
    pub(crate) fn new(half: i32, cells: Vec<(i32, i32, bool)>) -> Self {
        let side = (2 * half + 1) as usize;
        let mut member = vec![0u8; side * side];
        for &(dx, dz, lanes) in &cells {
            let m = &mut member[(dz + half) as usize * side + (dx + half) as usize];
            *m = (*m).max(1 + u8::from(lanes));
        }
        Self {
            half,
            side,
            member,
            cells,
            fresh: Default::default(),
            last: None,
        }
    }

    /// The cells to hand over at (x, z): every one at the run's first point or after a jump,
    /// else those the stamp a step back left out or left off its lanes.
    pub(crate) fn step_to(&mut self, x: i32, z: i32) -> &[(i32, i32, bool)] {
        let step = self.last.map(|(lx, lz)| (x - lx, z - lz));
        self.last = Some((x, z));
        let Some((sx, sz)) = step.filter(|&(sx, sz)| sx.abs() <= 1 && sz.abs() <= 1) else {
            return &self.cells;
        };
        let k = ((sz + 1) * 3 + sx + 1) as usize;
        if self.fresh[k].is_none() {
            let (half, side, member) = (self.half, self.side, &self.member);
            // A cell at offset (dx, dz) here lay at (dx + sx, dz + sz) in the stamp a step back.
            let fresh = self
                .cells
                .iter()
                .copied()
                .filter(|&(dx, dz, lanes)| {
                    let (bx, bz) = (dx + sx, dz + sz);
                    let back = if bx.abs() <= half && bz.abs() <= half {
                        member[(bz + half) as usize * side + (bx + half) as usize]
                    } else {
                        0
                    };
                    back < 1 + u8::from(lanes)
                })
                .collect();
            self.fresh[k] = Some(fresh);
        }
        self.fresh[k].as_deref().unwrap_or_default()
    }
}

/// What `stamp_highway_surfaces` found of a cell past a road's core, the cell alone.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Unknown,
    Cut,
    Kept,
}

/// Collect all (x, z) coordinates covered by highways tagged
/// `tunnel=building_passage`.  The returned bitmap can be passed into building
/// generation to cut ground-level openings through walls and floors.
pub fn collect_building_passage_coords(
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
    scale: f64,
) -> CoordinateBitmap {
    // Quick scan: skip bitmap allocation entirely when there are no passage ways.
    let has_any = elements.iter().any(|e| {
        if let ProcessedElement::Way(w) = e {
            w.tags.get("tunnel").map(|v| v.as_str()) == Some("building_passage")
                && w.tags.contains_key("highway")
        } else {
            false
        }
    });
    if !has_any {
        return CoordinateBitmap::new_empty();
    }

    let mut bitmap = CoordinateBitmap::new(xzbbox);

    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };

        // Must be tunnel=building_passage
        if way.tags.get("tunnel").map(|v| v.as_str()) != Some("building_passage") {
            continue;
        }

        // Must have a highway tag so we know the road width
        let Some(highway_type) = way.tags.get("highway") else {
            continue;
        };

        let block_range = highway_block_range(highway_type, &way.tags, scale);

        for i in 1..way.nodes.len() {
            let prev = way.nodes[i - 1].xz();
            let cur = way.nodes[i].xz();

            let points = bresenham_line(prev.x, 0, prev.z, cur.x, 0, cur.z);

            for (bx, _, bz) in &points {
                for dx in -block_range..=block_range {
                    for dz in -block_range..=block_range {
                        if in_road_stamp(dx, dz, block_range) {
                            bitmap.set(bx + dx, bz + dz);
                        }
                    }
                }
            }
        }
    }

    bitmap
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A footway at grade is a road at grade, a bridge is not, and neither is the end of
    /// an approach that runs onto the deck; a way the caller drops is left out.
    #[test]
    fn roads_at_grade_leave_the_decks_open() {
        let way = |id: u64, tags: &[(&str, &str)], from: (i32, i32), to: (i32, i32)| {
            let node = |id: u64, (x, z): (i32, i32)| ProcessedNode {
                id,
                tags: HashMap::new(),
                x,
                z,
            };
            ProcessedElement::Way(ProcessedWay {
                id,
                nodes: vec![node(id * 10, from), node(id * 10 + 1, to)],
                tags: tags
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            })
        };
        let elements = [
            way(1, &[("highway", "footway")], (0, 10), (20, 10)),
            way(
                2,
                &[("highway", "footway"), ("bridge", "yes")],
                (20, 10),
                (30, 10),
            ),
            way(3, &[("highway", "footway")], (30, 10), (50, 10)),
            way(4, &[("highway", "service")], (10, 30), (40, 30)),
        ];
        let bbox = XZBBox::rect_from_min_max(0, 0, 63, 63).unwrap();
        let service =
            |e: &ProcessedElement| e.tags().get("highway").map(String::as_str) == Some("service");
        let roads = collect_roads_at_grade(&elements, &bbox, 1.0, |e| !service(e));
        assert!(
            roads.contains(5, 10) && roads.contains(45, 10),
            "the footway"
        );
        assert!(roads.contains(5, 11), "its whole surface");
        for x in 20..=30 {
            assert!(!roads.contains(x, 10), "on the deck at x={x}");
        }
        assert!(!roads.contains(25, 30), "the dropped service road");
    }

    #[test]
    fn highway_block_range_scales_with_lanes() {
        let tags = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        // Untagged roads keep their per-type defaults: a rural secondary is 7 m across.
        assert_eq!(highway_block_range("residential", &tags(&[]), 1.0), 2);
        assert_eq!(highway_block_range("motorway", &tags(&[]), 1.0), 5);
        assert_eq!(highway_block_range("secondary", &tags(&[]), 1.0), 3);
        assert_eq!(highway_block_range("tertiary", &tags(&[]), 1.0), 3);
        assert_eq!(
            highway_block_range("tertiary", &tags(&[("lanes", "2")]), 1.0),
            3
        );
        // More lanes widen, clamped to the cap.
        assert_eq!(
            highway_block_range("primary", &tags(&[("lanes", "4")]), 1.0),
            6
        );
        assert_eq!(
            highway_block_range("motorway", &tags(&[("lanes", "6")]), 1.0),
            road_width::MAX_HALF
        );
        // Non-vehicular types never scale with lanes.
        assert_eq!(
            highway_block_range("footway", &tags(&[("lanes", "4")]), 1.0),
            1
        );
        assert_eq!(highway_block_range("cycleway", &tags(&[]), 1.0), 1);
        // Explicit width=* wins and tolerates a unit suffix.
        assert_eq!(
            highway_block_range("residential", &tags(&[("width", "8")]), 1.0),
            4
        );
        assert_eq!(
            highway_block_range(
                "residential",
                &tags(&[("width", "8 m"), ("lanes", "2")]),
                1.0
            ),
            4
        );
        // Parked cars widen the kerb, not the lanes.
        let parked = tags(&[("parking:both", "lane")]);
        assert_eq!(highway_block_range("residential", &parked, 1.0), 4);
        assert_eq!(highway_lane_range("residential", &parked, 1.0), 2);
        // Down-scaling never collapses a road to zero width.
        assert_eq!(highway_block_range("footway", &tags(&[]), 0.4), 1);
        assert_eq!(highway_block_range("residential", &tags(&[]), 0.4), 1);
    }

    #[test]
    fn lots_mapped_as_multipolygons_are_drawn_and_squares_left_to_the_ground() {
        use crate::element_processing::RelationArea;
        let tags = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let mp = |pairs: &[(&str, &str)]| {
            let mut t = tags(pairs);
            t.insert("type".into(), "multipolygon".into());
            RelationArea::of(&t)
        };
        let lot = mp(&[("amenity", "parking")]);
        assert_eq!(lot, Some(RelationArea::Parking));
        assert!(lot.unwrap().fills_members(&tags(&[])));
        // A square keeps the town's paving and the paths across it, with its trees.
        assert_eq!(mp(&[("highway", "pedestrian"), ("area", "yes")]), None);
        // A park that is also a square stays a park.
        assert_eq!(
            mp(&[("highway", "pedestrian"), ("leisure", "park")]),
            Some(RelationArea::Leisure)
        );
    }

    #[test]
    fn decks_stamp_a_square_and_everything_else_the_disc() {
        let way = |pairs: &[(&str, &str)]| ProcessedWay {
            id: 1,
            nodes: vec![
                ProcessedNode {
                    id: 1,
                    tags: HashMap::new(),
                    x: 0,
                    z: 0,
                },
                ProcessedNode {
                    id: 2,
                    tags: HashMap::new(),
                    x: 9,
                    z: 9,
                },
            ],
            tags: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        assert!(stamps_square(&way(&[
            ("highway", "primary"),
            ("bridge", "yes")
        ])));
        // An approach ramp is an ordinary way.
        assert!(!stamps_square(&way(&[("highway", "primary")])));
        assert!(!stamps_square(&way(&[
            ("highway", "platform"),
            ("bridge", "yes"),
            ("area", "yes")
        ])));
        // Across a diagonal the disc reaches less far than the square's corner.
        let d = std::f32::consts::FRAC_1_SQRT_2;
        assert!((stamp_reach(3, true, d, -d) - 3.0 * 2.0 * d).abs() < 1e-5);
        assert!((stamp_reach(3, false, d, -d) - 4.0 * d).abs() < 1e-5);
        assert!((stamp_reach(3, false, 1.0, 0.0) - 3.0).abs() < 1e-5);
    }

    #[test]
    fn only_what_a_road_gains_past_its_lanes_stops_at_walls() {
        let way = |pairs: &[(&str, &str)]| ProcessedWay {
            id: 1,
            nodes: Vec::new(),
            tags: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let core = |pairs: &[(&str, &str)]| wall_core(&way(pairs), pairs[0].1, 1.0);
        assert_eq!(core(&[("highway", "residential")]), None);
        assert_eq!(
            core(&[("highway", "residential"), ("parking:both", "lane")]),
            Some(2)
        );
        // The second lane a sidewalk gives a street stops at walls; tagged lanes do not.
        assert_eq!(
            core(&[("highway", "residential"), ("sidewalk", "both")]),
            Some(2)
        );
        assert_eq!(core(&[("highway", "residential"), ("lanes", "2")]), None);
        assert_eq!(core(&[("highway", "pedestrian")]), None);
        assert_eq!(
            core(&[("highway", "pedestrian"), ("tunnel", "building_passage")]),
            None
        );
        assert_eq!(
            core(&[
                ("highway", "residential"),
                ("parking:both", "lane"),
                ("bridge", "yes")
            ]),
            None
        );
    }

    #[test]
    fn the_road_stamp_is_a_disc() {
        // A footway still covers its full 3x3.
        assert!(in_road_stamp(1, 1, 1));
        // Wider roads lose their corners, so a diagonal is no wider than an axis.
        assert!(in_road_stamp(2, 0, 2) && in_road_stamp(2, 1, 2) && !in_road_stamp(2, 2, 2));
        assert!(in_road_stamp(3, 1, 3) && !in_road_stamp(3, 2, 3));
    }

    /// The water carve finds a bore's roof by what it is built of, so it has to know
    /// every block of the shell.
    #[test]
    fn the_water_carve_knows_every_road_shell_block() {
        use crate::water_depth::BORE_SHELL;
        for x in -30..30 {
            for y in -10..10 {
                for z in -30..30 {
                    assert!(BORE_SHELL.contains(&tunnel_shell_block(x, y, z)));
                }
            }
        }
        // The foundation, road row and interior placeholder.
        assert!(BORE_SHELL.contains(&STONE_BRICKS));
    }

    #[test]
    fn runway_dash_alternates_on_and_off() {
        // At scale 1: 10 blocks on, 6 off, repeating every 16.
        assert!(runway_centerline_dash_on(0.0, 1.0));
        assert!(runway_centerline_dash_on(9.0, 1.0));
        assert!(!runway_centerline_dash_on(10.0, 1.0));
        assert!(!runway_centerline_dash_on(15.0, 1.0));
        assert!(
            runway_centerline_dash_on(16.0, 1.0),
            "pattern repeats at 16"
        );
        assert!(
            runway_centerline_dash_on(160.0, 1.0),
            "phase stays consistent far along"
        );
    }

    #[test]
    fn runway_dash_scales_with_world_scale() {
        // At scale 2: 20 blocks on, 12 off.
        assert!(runway_centerline_dash_on(19.0, 2.0));
        assert!(!runway_centerline_dash_on(20.0, 2.0));
    }

    // --- Rendering regression tests: markings must actually overwrite the base surface. ---

    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::osm_parser::ProcessedNode;
    use crate::world_editor::WorldEditor;
    use clap::Parser as _;
    use std::collections::HashMap as StdMap;
    use std::path::PathBuf;

    /// Builds an in-memory editor (never saved) over a 400×100 area at ground Y=0.
    fn test_editor(xzbbox: &XZBBox) -> WorldEditor<'_> {
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        WorldEditor::new(PathBuf::from("/dev/null/unused"), xzbbox, llbbox)
    }

    fn straight_aeroway(kind: &str) -> ProcessedWay {
        let mut tags = StdMap::new();
        tags.insert("aeroway".to_string(), kind.to_string());
        ProcessedWay {
            id: 1,
            nodes: vec![
                ProcessedNode {
                    id: 1,
                    tags: StdMap::new(),
                    x: 10,
                    z: 50,
                },
                ProcessedNode {
                    id: 2,
                    tags: StdMap::new(),
                    x: 390,
                    z: 50,
                },
            ],
            tags,
        }
    }

    fn ring_aeroway(tags: &[(&str, &str)]) -> ProcessedWay {
        let corners = [(100, 20), (200, 20), (200, 80), (100, 80), (100, 20)];
        ProcessedWay {
            id: 7,
            nodes: corners
                .iter()
                .enumerate()
                .map(|(i, &(x, z))| ProcessedNode {
                    id: 70 + i as u64,
                    tags: StdMap::new(),
                    x,
                    z,
                })
                .collect(),
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn draw_aeroway(way: &ProcessedWay) -> WorldEditor<'static> {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let xzbbox = Box::leak(Box::new(
            XZBBox::rect_from_xz_lengths(400.0, 100.0).unwrap(),
        ));
        let mut editor = test_editor(xzbbox);
        generate_aeroway(
            &mut editor,
            way,
            &args,
            &CoordinateBitmap::new_empty(),
            &FloodFillCache::new(),
        );
        editor
    }

    #[test]
    fn an_apron_is_filled_not_outlined() {
        let editor = draw_aeroway(&ring_aeroway(&[("aeroway", "apron")]));
        assert!(
            editor.check_for_block(150, 0, 50, Some(&[LIGHT_GRAY_CONCRETE])),
            "the middle of the apron is paved"
        );
        assert!(
            !editor.check_for_block(150, 0, 90, Some(&[LIGHT_GRAY_CONCRETE])),
            "nothing past its edge"
        );
    }

    #[test]
    fn a_runway_area_is_filled_gray() {
        let editor = draw_aeroway(&ring_aeroway(&[("area:aeroway", "runway")]));
        assert!(editor.check_for_block(150, 0, 50, Some(&[GRAY_CONCRETE])));
        assert!(editor.check_for_block(101, 0, 50, Some(&[GRAY_CONCRETE])));
    }

    #[test]
    fn an_unpaved_aeroway_is_its_own_ground_without_paint() {
        let paved = [
            GRAY_CONCRETE,
            LIGHT_GRAY_CONCRETE,
            WHITE_CONCRETE,
            YELLOW_CONCRETE,
        ];
        let apron = draw_aeroway(&ring_aeroway(&[("aeroway", "apron"), ("surface", "grass")]));
        assert!(apron.check_for_block(150, 0, 50, Some(&[GRASS_BLOCK])));
        let mut strip = straight_aeroway("runway");
        strip.tags.insert("surface".into(), "gravel".into());
        let runway = draw_aeroway(&strip);
        assert!(runway.check_for_block(100, 0, 50, Some(&[GRAVEL])));
        for (editor, x, z) in [(&apron, 150, 50), (&runway, 100, 50), (&runway, 100, 39)] {
            assert!(
                !editor.check_for_block(x, 0, z, Some(&paved)),
                "paint at {x},{z}"
            );
        }
    }

    #[test]
    fn an_aerodrome_boundary_draws_nothing() {
        let editor = draw_aeroway(&ring_aeroway(&[("aeroway", "aerodrome")]));
        for (x, z) in [(100, 20), (150, 20), (150, 50), (150, 30)] {
            assert!(
                !editor.check_for_block(x, 0, z, Some(&[GRAY_CONCRETE, LIGHT_GRAY_CONCRETE])),
                "pavement at {x},{z}"
            );
        }
    }

    #[test]
    fn a_taxilane_is_a_narrow_lead_in_line() {
        let editor = draw_aeroway(&straight_aeroway("taxilane"));
        assert!(
            editor.check_for_block(100, 0, 50, Some(&[YELLOW_CONCRETE])),
            "yellow centre line"
        );
        assert!(
            !editor.check_for_block(100, 0, 58, Some(&[LIGHT_GRAY_CONCRETE])),
            "no 24 m slab"
        );
    }

    #[test]
    fn a_stopway_is_runway_pavement_without_markings() {
        let editor = draw_aeroway(&straight_aeroway("stopway"));
        assert!(
            editor.check_for_block(10, 0, 50, Some(&[GRAY_CONCRETE])),
            "no centre dash"
        );
        assert!(
            editor.check_for_block(10, 0, 39, Some(&[GRAY_CONCRETE])),
            "no edge stripe"
        );
    }

    #[test]
    fn sidewalks_and_squares_never_look_like_the_road() {
        let tags = |kv: &[(&str, &str)]| -> HashMap<String, String> {
            kv.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let road = surface_palette("residential", &tags(&[("surface", "asphalt")]));
        assert_eq!(road, DEFAULT_ROAD_MIX);
        for (highway, kv, want) in [
            (
                "footway",
                &[("footway", "sidewalk"), ("surface", "asphalt")][..],
                POLISHED_ANDESITE,
            ),
            // A pedestrian street keeps its grey: some are a former road's whole width.
            ("pedestrian", &[("surface", "asphalt")], GRAY_CONCRETE),
            ("pedestrian", &[], GRAY_CONCRETE),
            ("footway", &[("surface", "paved")], SMOOTH_STONE),
            ("pedestrian", &[("surface", "sett")], SMOOTH_STONE),
            // A footway or steps with no surface tagged are asphalt, as a park's paths are.
            ("footway", &[], POLISHED_ANDESITE),
            ("steps", &[], POLISHED_ANDESITE),
            // A sidewalk with no surface tagged is paved.
            ("footway", &[("footway", "sidewalk")], SMOOTH_STONE),
            (
                "footway",
                &[("footway", "sidewalk"), ("surface", "gravel")],
                GRAVEL,
            ),
        ] {
            let mut tags = tags(kv);
            tags.insert("highway".into(), highway.into());
            assert_eq!(surface_palette(highway, &tags), &[want], "{highway} {kv:?}");
        }
        // A square is filled with its first block.
        let square = tags(&[
            ("highway", "pedestrian"),
            ("area", "yes"),
            ("surface", "paving_stones"),
        ]);
        assert_eq!(pedestrian_paving(&square), Some(&[SMOOTH_STONE][..]));
    }

    #[test]
    fn a_way_no_car_drives_never_takes_the_road_mix() {
        let tags = |kv: &[(&str, &str)]| -> HashMap<String, String> {
            kv.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        // A shared foot and cycle path beside a street, as in Erding.
        let shared = [
            ("foot", "designated"),
            ("bicycle", "designated"),
            ("segregated", "no"),
            ("surface", "asphalt"),
        ];
        assert_eq!(surface_palette("path", &tags(&shared)), PATH_ASPHALT);
        for (highway, kv, want) in [
            ("bridleway", &[("surface", "asphalt")][..], PATH_ASPHALT),
            ("steps", &[("surface", "asphalt")], PATH_ASPHALT),
            ("path", &[("surface", "paving_stones")], &[SMOOTH_STONE]),
            ("path", &[("surface", "concrete")], &[SMOOTH_STONE]),
            (
                "cycleway",
                &[("surface", "concrete:plates")],
                crate::element_processing::surfaces::CYCLEWAY_MIX,
            ),
            // A cycle path that is not red, with its surface tagged or not.
            (
                "cycleway",
                &[("surface", "asphalt"), ("surface:colour", "grey")],
                PATH_ASPHALT,
            ),
            ("cycleway", &[("colour", "grey")], PATH_ASPHALT),
            // Unpaved, or not tagged, as before; a bridleway not tagged is no road.
            ("path", &[("surface", "gravel")], &[GRAVEL]),
            ("path", &[], &[DIRT_PATH]),
            ("bridleway", &[], &[DIRT_PATH]),
            // Where it crosses a road the road keeps its surface.
            (
                "path",
                &[("surface", "asphalt"), ("crossing", "unmarked")],
                DEFAULT_ROAD_MIX,
            ),
            (
                "path",
                &[("surface", "asphalt"), ("crossing", "no")],
                PATH_ASPHALT,
            ),
            (
                "cycleway",
                &[
                    ("surface", "asphalt"),
                    ("cycleway", "crossing"),
                    ("colour", "grey"),
                ],
                DEFAULT_ROAD_MIX,
            ),
            (
                "footway",
                &[("surface", "asphalt"), ("footway", "crossing")],
                &[GRAY_CONCRETE],
            ),
            // An access aisle beside a parking stall is the car park's asphalt.
            (
                "footway",
                &[("surface", "asphalt"), ("footway", "access_aisle")],
                &[GRAY_CONCRETE],
            ),
            // Ways cars may drive keep the road's asphalt.
            ("service", &[("surface", "asphalt")], DEFAULT_ROAD_MIX),
            ("track", &[("surface", "asphalt")], DEFAULT_ROAD_MIX),
            ("living_street", &[("surface", "asphalt")], DEFAULT_ROAD_MIX),
        ] {
            let mut tags = tags(kv);
            tags.insert("highway".into(), highway.into());
            assert_eq!(surface_palette(highway, &tags), want, "{highway} {kv:?}");
        }
    }

    #[test]
    fn grey_wins_over_a_paths_asphalt_whichever_is_built_first() {
        let tags = |kv: &[(&str, &str)]| -> HashMap<String, String> {
            kv.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        // A path's asphalt keeps out of the grey; the grey ways pave over the asphalt.
        assert!(CAR_FREE_PROTECTED_SURFACES.contains(&GRAY_CONCRETE));
        // A sidewalk carries on across a driveway, as on main.
        assert!(!SIDEWALK_PROTECTED_SURFACES.contains(&GRAY_CONCRETE));
        assert!(SIDEWALK_PROTECTED_SURFACES.contains(&POLISHED_ANDESITE));
        for list in [ROAD_PROTECTED_SURFACES, OVER_ASPHALT_PROTECTED_SURFACES] {
            assert!(!list.contains(&POLISHED_ANDESITE));
        }
        for (highway, kv) in [
            ("pedestrian", &[][..]),
            ("footway", &[("footway", "crossing")]),
            ("path", &[("crossing", "unmarked")]),
            ("footway", &[("footway", "access_aisle")]),
        ] {
            assert!(paves_like_road(highway, &tags(kv)), "{highway} {kv:?}");
        }
        for (highway, kv) in [
            ("footway", &[][..]),
            ("footway", &[("footway", "sidewalk")]),
            ("path", &[("surface", "asphalt")]),
            ("steps", &[]),
        ] {
            assert!(!paves_like_road(highway, &tags(kv)), "{highway} {kv:?}");
        }
    }

    #[test]
    fn a_road_under_construction_is_a_bare_bed_as_wide_as_the_road_it_becomes() {
        let tags: HashMap<String, String> = [("construction", "primary"), ("surface", "asphalt")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(surface_palette("construction", &tags), CONSTRUCTION_BED);
        assert_eq!(
            highway_block_range("construction", &tags, 1.0),
            highway_block_range("primary", &tags, 1.0)
        );
        assert_eq!(lane_marking_count("construction", &tags), 1);
    }

    #[test]
    fn runway_paints_white_centerline_and_edges_over_gray() {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let xzbbox = XZBBox::rect_from_xz_lengths(400.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);

        generate_aeroway(
            &mut editor,
            &straight_aeroway("runway"),
            &args,
            &CoordinateBitmap::new_empty(),
            &FloodFillCache::new(),
        );

        // Centerline at the way start (s=0, dash on) is white; a dash-gap cell stays gray.
        assert!(
            editor.check_for_block(10, 0, 50, Some(&[WHITE_CONCRETE])),
            "centerline dash"
        );
        assert!(
            editor.check_for_block(22, 0, 50, Some(&[GRAY_CONCRETE])),
            "dash gap stays asphalt"
        );
        // Solid white edge stripe one block inside the 12-wide half (z = 50 ± 11).
        assert!(
            editor.check_for_block(10, 0, 39, Some(&[WHITE_CONCRETE])),
            "left edge stripe"
        );
        assert!(
            editor.check_for_block(10, 0, 61, Some(&[WHITE_CONCRETE])),
            "right edge stripe"
        );
        // Plain surface between centerline and edge is asphalt gray.
        assert!(
            editor.check_for_block(10, 0, 45, Some(&[GRAY_CONCRETE])),
            "asphalt base"
        );
    }

    #[test]
    fn taxiway_paints_yellow_centerline_over_light_gray() {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let xzbbox = XZBBox::rect_from_xz_lengths(400.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);

        generate_aeroway(
            &mut editor,
            &straight_aeroway("taxiway"),
            &args,
            &CoordinateBitmap::new_empty(),
            &FloodFillCache::new(),
        );

        assert!(
            editor.check_for_block(10, 0, 50, Some(&[YELLOW_CONCRETE])),
            "yellow centerline"
        );
        assert!(
            editor.check_for_block(10, 0, 45, Some(&[LIGHT_GRAY_CONCRETE])),
            "light-gray base"
        );
        // Taxiways get no white edge stripes.
        assert!(
            !editor.check_for_block(10, 0, 39, Some(&[WHITE_CONCRETE])),
            "no edge stripe"
        );
    }

    #[test]
    fn runway_width_tag_widens_the_strip() {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let xzbbox = XZBBox::rect_from_xz_lengths(400.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);

        let mut way = straight_aeroway("runway");
        way.tags.insert("width".to_string(), "60".to_string());
        generate_aeroway(
            &mut editor,
            &way,
            &args,
            &CoordinateBitmap::new_empty(),
            &FloodFillCache::new(),
        );

        // 60 m wide ⇒ half-width 30: asphalt reaches z=70 and the edge stripe sits at z=50+29.
        assert!(
            editor.check_for_block(10, 0, 70, Some(&[GRAY_CONCRETE])),
            "widened asphalt"
        );
        assert!(
            editor.check_for_block(10, 0, 79, Some(&[WHITE_CONCRETE])),
            "edge stripe at width/2-1"
        );
    }

    #[test]
    fn runway_overrides_a_crossing_taxiway_regardless_of_order() {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let xzbbox = XZBBox::rect_from_xz_lengths(400.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);

        // Process the taxiway FIRST — the order that used to leak taxiway surface onto the runway.
        let mut tags = StdMap::new();
        tags.insert("aeroway".to_string(), "taxiway".to_string());
        let taxiway = ProcessedWay {
            id: 2,
            nodes: vec![
                ProcessedNode {
                    id: 3,
                    tags: StdMap::new(),
                    x: 200,
                    z: 10,
                },
                ProcessedNode {
                    id: 4,
                    tags: StdMap::new(),
                    x: 200,
                    z: 90,
                },
            ],
            tags,
        };
        generate_aeroway(
            &mut editor,
            &taxiway,
            &args,
            &CoordinateBitmap::new_empty(),
            &FloodFillCache::new(),
        );
        generate_aeroway(
            &mut editor,
            &straight_aeroway("runway"),
            &args,
            &CoordinateBitmap::new_empty(),
            &FloodFillCache::new(),
        );

        // The crossing cell belongs to the runway, not the taxiway.
        assert!(
            editor.check_for_block(200, 0, 50, Some(&[GRAY_CONCRETE])),
            "runway wins crossing"
        );
        assert!(!editor.check_for_block(200, 0, 50, Some(&[YELLOW_CONCRETE])));
        assert!(!editor.check_for_block(200, 0, 50, Some(&[LIGHT_GRAY_CONCRETE])));
        // Away from the runway the taxiway is untouched.
        assert!(
            editor.check_for_block(200, 0, 20, Some(&[YELLOW_CONCRETE])),
            "taxiway intact off-runway"
        );
    }

    fn way_along(id: u64, from: (i32, i32), to: (i32, i32), tags: &[(&str, &str)]) -> ProcessedWay {
        ProcessedWay {
            id,
            nodes: vec![
                ProcessedNode {
                    id: id * 10,
                    tags: StdMap::new(),
                    x: from.0,
                    z: from.1,
                },
                ProcessedNode {
                    id: id * 10 + 1,
                    tags: StdMap::new(),
                    x: to.0,
                    z: to.1,
                },
            ],
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn build_ways(editor: &mut WorldEditor, ways: &[ProcessedWay]) {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let outlines = crate::element_processing::bridge_styles::BridgeOutlineIndex::build(&[]);
        let structures =
            BridgeStructureMap::build(&[], editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&[], &structures, 1.0);
        let empty = CoordinateBitmap::new_empty();
        let mut cells = Vec::new();
        for way in ways {
            generate_highways(
                editor,
                &ProcessedElement::Way(way.clone()),
                &args,
                &HighwayConnectivityMap::new(),
                &FloodFillCache::new(),
                &empty,
                &structures,
                &surface,
                &TunnelInternalEndpoints::default(),
                &TunnelPortalMap::default(),
                &empty,
                &mut cells,
                &RoadMarkingIndex::default(),
                &CoordinateBitmap::new_empty(),
            );
        }
    }

    #[test]
    fn a_raceway_is_circuit_wide_with_its_paint_on_the_surface() {
        let xzbbox = XZBBox::rect_from_xz_lengths(400.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let mut way = way_along(1, (10, 50), (390, 50), &[]);
        way.tags = [("highway", "raceway"), ("sport", "motor")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let elements = [ProcessedElement::Way(way.clone())];
        let markings =
            RoadMarkingIndex::build(&elements, 1.0, crate::decals::region::SignRegion::Germanic);
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let outlines = crate::element_processing::bridge_styles::BridgeOutlineIndex::build(&[]);
        let structures =
            BridgeStructureMap::build(&[], &editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&[], &structures, 1.0);
        let empty = CoordinateBitmap::new_empty();
        generate_highways(
            &mut editor,
            &elements[0],
            &args,
            &HighwayConnectivityMap::new(),
            &FloodFillCache::new(),
            &empty,
            &structures,
            &surface,
            &TunnelInternalEndpoints::default(),
            &TunnelPortalMap::default(),
            &empty,
            &mut Vec::new(),
            &markings,
            &CoordinateBitmap::new_empty(),
        );
        for z in [44, 56] {
            assert!(
                editor.check_for_block(200, 0, z, Some(&[WHITE_CONCRETE])),
                "z={z}"
            );
        }
        for z in 45..=55 {
            assert!(
                editor.check_for_block(200, 0, z, Some(DEFAULT_ROAD_MIX)),
                "asphalt at z={z}"
            );
        }
        assert!(!editor.check_for_block(200, 0, 43, Some(&[WHITE_CONCRETE])));
    }

    #[test]
    fn raceway_paint_goes_over_a_road_laid_first() {
        let xzbbox = XZBBox::rect_from_xz_lengths(400.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let street = way_along(2, (150, 56), (250, 56), &[("highway", "residential")]);
        build_ways(&mut editor, &[street]);
        assert!(editor.check_for_block(200, 0, 56, Some(DEFAULT_ROAD_MIX)));
        let mut way = way_along(1, (10, 50), (390, 50), &[]);
        way.tags = [("highway", "raceway"), ("sport", "motor")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let elements = [ProcessedElement::Way(way)];
        let markings =
            RoadMarkingIndex::build(&elements, 1.0, crate::decals::region::SignRegion::Germanic);
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let outlines = crate::element_processing::bridge_styles::BridgeOutlineIndex::build(&[]);
        let structures =
            BridgeStructureMap::build(&[], &editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&[], &structures, 1.0);
        let empty = CoordinateBitmap::new_empty();
        generate_highways(
            &mut editor,
            &elements[0],
            &args,
            &HighwayConnectivityMap::new(),
            &FloodFillCache::new(),
            &empty,
            &structures,
            &surface,
            &TunnelInternalEndpoints::default(),
            &TunnelPortalMap::default(),
            &empty,
            &mut Vec::new(),
            &markings,
            &CoordinateBitmap::new_empty(),
        );
        assert!(editor.check_for_block(200, 0, 56, Some(&[WHITE_CONCRETE])));
    }

    /// Street lamp nodes at (id, x, z), tagged with `layer`.
    fn place_lamps(
        editor: &mut WorldEditor,
        surface: &BridgeSurfaceMap,
        lamps: &[(u64, i32, i32)],
        layer: &str,
    ) {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let outlines = crate::element_processing::bridge_styles::BridgeOutlineIndex::build(&[]);
        let structures =
            BridgeStructureMap::build(&[], editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
        let empty = CoordinateBitmap::new_empty();
        for &(id, x, z) in lamps {
            let element = ProcessedElement::Node(ProcessedNode {
                id,
                tags: [("highway", "street_lamp"), ("layer", layer)]
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                x,
                z,
            });
            generate_highways(
                editor,
                &element,
                &args,
                &HighwayConnectivityMap::new(),
                &FloodFillCache::new(),
                &empty,
                &structures,
                surface,
                &TunnelInternalEndpoints::default(),
                &TunnelPortalMap::default(),
                &empty,
                &mut Vec::new(),
                &RoadMarkingIndex::default(),
                &CoordinateBitmap::new_empty(),
            );
        }
    }

    /// Land cover calls a quay sidewalk water as often as the river; only the fill decides.
    #[test]
    fn a_lamp_on_land_cover_water_left_dry_stands() {
        use crate::land_cover::{LandCoverData, LC_GRASSLAND, LC_WATER};
        let side = 60usize;
        let grid: Vec<Vec<u8>> = (0..side)
            .map(|_| {
                (0..side)
                    .map(|x| if x < 20 { LC_WATER } else { LC_GRASSLAND })
                    .collect()
            })
            .collect();
        let lc = LandCoverData {
            grid,
            water_distance: vec![vec![0u8; side]; side],
            water_blend_cache: once_cell::sync::OnceCell::new(),
            beach_tiles: once_cell::sync::OnceCell::new(),
            inland_water: Default::default(),
            width: side,
            height: side,
            cells_per_meter: 1.0,
        };
        let xzbbox = XZBBox::rect_from_min_max(0, 0, side as i32 - 1, side as i32 - 1).unwrap();
        let mut editor = test_editor(&xzbbox);
        editor.set_ground(std::sync::Arc::new(
            crate::ground::test_support::ground_with_land_cover_and_elevation(lc, side, side),
        ));
        place_lamps(
            &mut editor,
            &BridgeSurfaceMap::empty(),
            &[(1, 10, 30), (2, 40, 30)],
            "0",
        );
        drop_lamps_in_water(&mut editor);
        for x in [10, 40] {
            let ground = editor.get_absolute_y(x, 0, 30);
            assert!(editor.check_for_block_absolute(
                x,
                ground + 1,
                30,
                Some(&[SMOOTH_STONE]),
                None
            ));
        }
    }

    /// A river polygon reaching over a quay edge fills under a lamp mapped there only after
    /// the lamp went up, so it comes down; one on a deck over the water stays.
    #[test]
    fn lamps_the_water_fill_reached_are_taken_down() {
        let xzbbox = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let mut surface = BridgeSurfaceMap::empty();
        for x in 30..40 {
            for z in 10..20 {
                surface.insert_deck(x, z, 4);
                editor.set_block_absolute(STONE_BRICKS, x, 4, z, None, None);
            }
        }
        place_lamps(&mut editor, &surface, &[(1, 10, 40), (2, 20, 40)], "0");
        place_lamps(&mut editor, &surface, &[(3, 35, 15)], "1");
        // A pier paved in the plinth's own stone, over the water.
        editor.set_block_absolute(SMOOTH_STONE, 45, 0, 40, None, None);
        editor.set_block_absolute(WATER, 45, -1, 40, None, None);
        place_lamps(&mut editor, &surface, &[(4, 45, 40)], "0");
        for x in 0..60 {
            for z in 0..60 {
                if x < 15 || (30..40).contains(&x) {
                    editor.set_block_absolute(WATER, x, 0, z, None, Some(&[]));
                }
            }
        }
        // Water up a block over its ground drowns the plinth, and the post still goes.
        editor.set_block_absolute(WATER, 20, 1, 40, None, Some(&[]));
        editor.set_block_absolute(WATER, 20, 0, 40, None, Some(&[]));
        // The first lamp is in the halo, a copy the merge would put back if left.
        editor.set_strict_bounds(15, 0, 59, 59);
        drop_lamps_in_water(&mut editor);

        assert!(editor.highest_block_between(10, 40, 1, 20).is_none());
        assert!(editor.highest_block_between(20, 40, 2, 20).is_none());
        assert!(editor.check_for_block_absolute(35, 9, 15, Some(&[REDSTONE_LAMP]), None));
        assert!(editor.check_for_block_absolute(45, 5, 40, Some(&[REDSTONE_LAMP]), None));
        assert!(editor.check_for_block_absolute(45, 0, 40, Some(&[SMOOTH_STONE]), None));
    }

    /// A `layer` on a lamp marks a plaza over an underpass as often as a bridge: away from
    /// a deck it stands on the ground, a couple of blocks off one it moves onto the deck.
    #[test]
    fn layered_lamps_stand_on_a_deck_or_on_the_ground() {
        let xzbbox = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let mut surface = BridgeSurfaceMap::empty();
        for x in 30..40 {
            for z in 10..20 {
                surface.insert_deck(x, z, 20);
            }
        }
        place_lamps(&mut editor, &surface, &[(1, 10, 40), (2, 41, 15)], "1");
        let ground = editor.get_absolute_y(10, 0, 40);
        assert!(
            editor.check_for_block_absolute(10, ground + 1, 40, Some(&[SMOOTH_STONE]), None),
            "the plaza lamp stands on the ground"
        );
        assert!(
            editor.check_for_block_absolute(39, 21, 15, Some(&[SMOOTH_STONE]), None),
            "the parapet lamp stands on the deck edge"
        );
        assert!(
            editor.highest_block_between(41, 15, -64, 40).is_none(),
            "nothing is left where it was mapped, off the deck"
        );
    }

    #[test]
    fn a_road_does_not_pave_over_the_red_cycleway_beside_it_in_either_order() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 100.0).unwrap();
        let road = way_along(1, (10, 50), (190, 50), &[("highway", "primary")]);
        let path = way_along(2, (10, 52), (190, 52), &[("highway", "cycleway")]);
        let crossing = way_along(
            3,
            (100, 30),
            (100, 70),
            &[("highway", "cycleway"), ("cycleway", "crossing")],
        );
        let asphalt = &[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA];
        let red = &[RED_TERRACOTTA, RED_CONCRETE];
        for ways in [
            [road.clone(), path.clone(), crossing.clone()],
            [path.clone(), crossing.clone(), road.clone()],
        ] {
            let mut editor = test_editor(&xzbbox);
            build_ways(&mut editor, &ways);
            for x in [40, 150] {
                assert!(editor.check_for_block(x, 0, 52, Some(red)), "path at x={x}");
                assert!(
                    editor.check_for_block(x, 0, 49, Some(asphalt)),
                    "road at x={x}"
                );
            }
            // The crossing leaves the carriageway to the road.
            assert!(editor.check_for_block(100, 0, 49, Some(asphalt)));
            assert!(!editor.check_for_block(100, 0, 49, Some(red)));
        }
    }

    /// A way over explicit nodes, so ways can share junction nodes.
    fn way_through(id: u64, nodes: &[(u64, i32, i32)], tags: &[(&str, &str)]) -> ProcessedWay {
        ProcessedWay {
            id,
            nodes: nodes
                .iter()
                .map(|&(nid, x, z)| ProcessedNode {
                    id: nid,
                    tags: StdMap::new(),
                    x,
                    z,
                })
                .collect(),
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    /// Renders `ways` with road paint resolved over all of them, as a real run does.
    fn build_marked_ways(editor: &mut WorldEditor, ways: &[ProcessedWay]) {
        build_marked_ways_at(editor, ways, "max");
    }

    /// [`build_marked_ways`] with `--road-detail <detail>`.
    fn build_marked_ways_at(editor: &mut WorldEditor, ways: &[ProcessedWay], detail: &str) {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4", "--road-detail", detail].iter());
        build_marked_ways_with(editor, ways, &args);
    }

    fn build_marked_ways_with(editor: &mut WorldEditor, ways: &[ProcessedWay], args: &Args) {
        let elements: Vec<ProcessedElement> =
            ways.iter().cloned().map(ProcessedElement::Way).collect();
        let markings = RoadMarkingIndex::build_with(
            &elements,
            1.0,
            crate::decals::region::SignRegion::Europe,
            args.road_detail.paints_crossings(),
            true,
        );
        let outlines = crate::element_processing::bridge_styles::BridgeOutlineIndex::build(&[]);
        let structures =
            BridgeStructureMap::build(&[], editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&[], &structures, 1.0);
        let empty = CoordinateBitmap::new_empty();
        let mut cells = Vec::new();
        for element in &elements {
            generate_highways(
                editor,
                element,
                args,
                &HighwayConnectivityMap::new(),
                &FloodFillCache::new(),
                &empty,
                &structures,
                &surface,
                &TunnelInternalEndpoints::default(),
                &TunnelPortalMap::default(),
                &empty,
                &mut cells,
                &markings,
                &CoordinateBitmap::new_empty(),
            );
        }
    }

    #[test]
    fn dashes_follow_world_axis() {
        // The same road whole, and clipped 37 cells in as a neighbouring piece's bbox cuts it.
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 100.0).unwrap();
        let tags = [("highway", "secondary")];
        let whole = way_along(1, (10, 50), (190, 50), &tags);
        let clipped = way_along(1, (47, 50), (190, 50), &tags);
        let dashes = |way: &ProcessedWay, detail: &str| -> Vec<bool> {
            let mut args =
                Args::parse_from(["arnis", "--bbox", "1,2,3,4", "--road-detail", detail]);
            args.one_world_run = Some(crate::one_world::RunContext {
                world_dir: std::path::PathBuf::new(),
                origin_lat: 0.0,
                origin_lon: 0.0,
                extending: false,
                elevation: None,
                replaced_chunks: 0,
                area_id: 0,
                piece: None,
                unit: None,
            });
            let mut editor = test_editor(&xzbbox);
            build_marked_ways_with(&mut editor, std::slice::from_ref(way), &args);
            (60..180)
                .map(|x| editor.check_for_block(x, 0, 50, Some(&[WHITE_CONCRETE])))
                .collect()
        };
        assert!(dashes(&whole, "clean").iter().any(|&w| w));
        assert_eq!(dashes(&whole, "clean"), dashes(&clipped, "clean"));
        // At max a piece keeps v3.4.0's rhythm from the way's start.
        assert_ne!(dashes(&whole, "max"), dashes(&clipped, "max"));
    }

    #[test]
    fn road_detail_thins_the_junction_paint() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 100.0).unwrap();
        let road = way_through(
            1,
            &[(10, 10, 50), (2, 100, 50), (11, 190, 50)],
            &[("highway", "primary"), ("lanes", "4")],
        );
        let crossing = way_through(
            2,
            &[(20, 100, 30), (2, 100, 50), (21, 100, 70)],
            &[
                ("highway", "footway"),
                ("footway", "crossing"),
                ("crossing", "zebra"),
            ],
        );
        // Rows with lane paint well away from the crossing, and whether the zebra is there.
        let paint = |detail: &str| {
            let mut editor = test_editor(&xzbbox);
            build_marked_ways_at(&mut editor, &[road.clone(), crossing.clone()], detail);
            let white = |x: i32, z: i32| editor.check_for_block(x, 0, z, Some(&[WHITE_CONCRETE]));
            let rows: Vec<i32> = (30..=70)
                .filter(|&z| (20..70).any(|x| white(x, z)))
                .collect();
            // Bars on the crossing's column, off the centre line's row.
            (rows, (44..=56).any(|z| z != 50 && white(100, z)))
        };
        let (max_rows, max_zebra) = paint("max");
        assert_eq!(max_rows.len(), 3, "{max_rows:?}");
        assert!(max_zebra);
        let (clean_rows, clean_zebra) = paint("clean");
        assert_eq!(clean_rows, vec![50]);
        assert!(clean_zebra);
        let (compact_rows, compact_zebra) = paint("compact");
        assert_eq!(compact_rows, vec![50]);
        assert!(!compact_zebra);
    }

    #[test]
    fn crossroads_keep_lane_lines_out_of_the_junction_box() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let tags = [("highway", "secondary")];
        let ways = [
            way_through(1, &[(10, 10, 100), (1, 100, 100)], &tags),
            way_through(2, &[(1, 100, 100), (11, 190, 100)], &tags),
            way_through(3, &[(12, 100, 10), (1, 100, 100)], &tags),
            way_through(4, &[(1, 100, 100), (13, 100, 190)], &tags),
        ];
        let mut editor = test_editor(&xzbbox);
        build_marked_ways(&mut editor, &ways);
        let half = highway_block_range("secondary", &HashMap::new(), 1.0);
        for x in 100 - half..=100 + half {
            for z in 100 - half..=100 + half {
                assert!(
                    !editor.check_for_block(x, 0, z, Some(&[WHITE_CONCRETE])),
                    "paint in the junction box at ({x}, {z})"
                );
            }
        }
        // Further out the centre line is still dashed.
        let painted = (20..80)
            .filter(|&x| editor.check_for_block(x, 0, 100, Some(&[WHITE_CONCRETE])))
            .count();
        assert!(painted > 10 && painted < 40, "{painted} painted cells");
    }

    #[test]
    fn a_zebra_stays_on_the_carriageway() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 100.0).unwrap();
        let road = way_through(
            1,
            &[(10, 10, 50), (2, 100, 50), (11, 190, 50)],
            &[("highway", "secondary")],
        );
        let crossing = way_through(
            2,
            &[(20, 100, 30), (2, 100, 50), (21, 100, 70)],
            &[
                ("highway", "footway"),
                ("footway", "crossing"),
                ("crossing", "zebra"),
            ],
        );
        let half = highway_block_range("secondary", &HashMap::new(), 1.0);
        for ways in [
            [road.clone(), crossing.clone()],
            [crossing.clone(), road.clone()],
        ] {
            let mut editor = test_editor(&xzbbox);
            build_marked_ways(&mut editor, &ways);
            let white = |x: i32, z: i32| editor.check_for_block(x, 0, z, Some(&[WHITE_CONCRETE]));
            // Bars along the road on every other row, nothing past the kerb.
            assert!(white(100, 50) && !white(100, 51) && white(100, 52));
            for z in (30..50 - half).chain(51 + half..=70) {
                assert!(!white(100, z), "zebra off the road at z={z}");
            }
            // The centre line stops short of the crossing.
            assert!(!white(102, 50) && !white(98, 50));
        }
    }

    #[test]
    fn lane_lines_follow_direction_and_region() {
        let tags = |kv: &[(&str, &str)]| -> HashMap<String, String> {
            kv.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let europe = RoadMarkingIndex::default();
        let mut america = RoadMarkingIndex::default();
        america.yellow_centre = true;
        let four = lane_lines("primary", &tags(&[("lanes", "4")]), 7, &europe);
        assert_eq!(four.len(), 3);
        assert!(!four[0].solid && four[1].solid && !four[2].solid);
        assert_eq!(four[1].offset, 0.0);

        let two = lane_lines("secondary", &HashMap::new(), 4, &europe);
        assert!(two.len() == 1 && !two[0].solid && two[0].block == WHITE_CONCRETE);
        // A double yellow line, one clear cell between its halves.
        let two = lane_lines("secondary", &HashMap::new(), 4, &america);
        assert_eq!(two.len(), 2);
        assert!(two.iter().all(|l| l.solid && l.block == YELLOW_CONCRETE));
        assert_eq!((two[0].offset, two[1].offset), (-1.0, 1.0));
        // Too tight for two: a single one.
        let six = lane_lines("primary", &tags(&[("lanes", "6")]), 8, &america);
        assert_eq!(six.iter().filter(|l| l.block == YELLOW_CONCRETE).count(), 1);

        // Eight lanes in 17 cells would leave lanes one cell wide: only the centre line.
        let eight = lane_lines("primary", &tags(&[("lanes", "8")]), 8, &europe);
        assert_eq!(eight.len(), 1);
        assert_eq!(eight[0].offset, 0.0);

        let oneway = lane_lines(
            "primary",
            &tags(&[("lanes", "3"), ("oneway", "yes")]),
            5,
            &europe,
        );
        assert!(oneway.len() == 2 && oneway.iter().all(|l| !l.solid));

        // A shared left-turn lane in the middle is edged by centre lines on both sides.
        let turn = lane_lines(
            "primary",
            &tags(&[
                ("lanes", "5"),
                ("lanes:forward", "2"),
                ("lanes:backward", "2"),
                ("lanes:both_ways", "1"),
            ]),
            8,
            &america,
        );
        let yellow: Vec<usize> = (0..turn.len())
            .filter(|&i| turn[i].block == YELLOW_CONCRETE)
            .collect();
        assert_eq!(yellow, vec![1, 2]);

        assert!(lane_lines("residential", &tags(&[("lanes", "2")]), 2, &europe).is_empty());
    }

    #[test]
    fn dashes_are_longer_on_fast_roads() {
        let tags = |kv: &[(&str, &str)]| -> HashMap<String, String> {
            kv.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert_eq!(dash_pattern("secondary", &HashMap::new(), 1.0), (3, 9));
        assert_eq!(dash_pattern("motorway", &HashMap::new(), 1.0), (6, 18));
        assert_eq!(
            dash_pattern("primary", &tags(&[("maxspeed", "100")]), 1.0),
            (6, 18)
        );
        assert_eq!(
            dash_pattern("primary", &tags(&[("maxspeed", "30 mph")]), 1.0),
            (3, 9)
        );
        assert_eq!(
            dash_pattern("secondary", &tags(&[("maxspeed", "DE:rural")]), 1.0),
            (6, 18)
        );
    }

    #[test]
    fn lane_lines_sit_symmetrically_at_any_coordinate() {
        // Three lanes in nine cells put the lines on half cells; both sides must round
        // the same way wherever the road is.
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 100.0).unwrap();
        let road = way_along(
            1,
            (10, 50),
            (190, 50),
            &[
                ("highway", "secondary"),
                ("oneway", "yes"),
                ("lanes", "3"),
                ("width", "8"),
            ],
        );
        let mut editor = test_editor(&xzbbox);
        build_marked_ways(&mut editor, &[road]);
        let white = |x: i32, z: i32| editor.check_for_block(x, 0, z, Some(&[WHITE_CONCRETE]));
        for x in 10..190 {
            assert_eq!(white(x, 48), white(x, 52), "lines out of step at x={x}");
            for z in [46, 47, 49, 50, 51, 53, 54] {
                assert!(!white(x, z), "paint inside a lane at ({x}, {z})");
            }
        }
        assert!((10..190).any(|x| white(x, 48)));
    }

    #[test]
    fn a_crossing_with_no_road_node_paints_nothing() {
        // Mapped across the road without sharing its node: nowhere known to be carriageway.
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 100.0).unwrap();
        let road = way_along(1, (10, 50), (190, 50), &[("highway", "residential")]);
        let crossing = way_along(
            2,
            (100, 30),
            (100, 70),
            &[
                ("highway", "footway"),
                ("footway", "crossing"),
                ("crossing", "zebra"),
            ],
        );
        let mut editor = test_editor(&xzbbox);
        build_marked_ways(&mut editor, &[road, crossing]);
        for z in 30..=70 {
            assert!(
                !editor.check_for_block(100, 0, z, Some(&[WHITE_CONCRETE])),
                "z={z}"
            );
        }
    }

    #[test]
    fn crossing_paint_lands_on_a_brick_road_in_either_order() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 100.0).unwrap();
        let road = way_through(
            1,
            &[(10, 10, 50), (2, 100, 50), (11, 190, 50)],
            &[("highway", "secondary"), ("surface", "bricks")],
        );
        let crossing = way_through(
            2,
            &[(20, 100, 30), (2, 100, 50), (21, 100, 70)],
            &[
                ("highway", "footway"),
                ("footway", "crossing"),
                ("crossing", "traffic_signals"),
                ("crossing:markings", "lines"),
            ],
        );
        for ways in [
            [road.clone(), crossing.clone()],
            [crossing.clone(), road.clone()],
        ] {
            let mut editor = test_editor(&xzbbox);
            build_marked_ways(&mut editor, &ways);
            // Edge lines a row outside the crossing, on bricks rather than asphalt.
            assert!(editor.check_for_block(98, 0, 52, Some(&[WHITE_CONCRETE])));
            assert!(editor.check_for_block(102, 0, 52, Some(&[WHITE_CONCRETE])));
            assert!(editor.check_for_block(100, 0, 52, Some(&[BRICK])));
            assert!(!editor.check_for_block(100, 0, 52, Some(DEFAULT_ROAD_MIX)));
        }
    }

    fn straight_tunnel(tags: &[(&str, &str)]) -> ProcessedWay {
        let mut t = StdMap::new();
        for (k, v) in tags {
            t.insert(k.to_string(), v.to_string());
        }
        ProcessedWay {
            id: 1,
            nodes: vec![
                ProcessedNode {
                    id: 1,
                    tags: StdMap::new(),
                    x: 10,
                    z: 50,
                },
                ProcessedNode {
                    id: 2,
                    tags: StdMap::new(),
                    x: 90,
                    z: 50,
                },
            ],
            tags: t,
        }
    }

    #[test]
    fn tunnel_predicate_matches_only_road_tunnels() {
        assert!(renders_as_highway_tunnel(&straight_tunnel(&[
            ("highway", "residential"),
            ("tunnel", "yes"),
        ])));
        assert!(!renders_as_highway_tunnel(&straight_tunnel(&[(
            "highway",
            "residential",
        )])));
        assert!(!renders_as_highway_tunnel(&straight_tunnel(&[
            ("highway", "footway"),
            ("tunnel", "building_passage"),
        ])));
        assert!(!renders_as_highway_tunnel(&straight_tunnel(&[
            ("highway", "residential"),
            ("tunnel", "yes"),
            ("indoor", "yes"),
        ])));
        assert!(!renders_as_highway_tunnel(&straight_tunnel(&[
            ("highway", "residential"),
            ("tunnel", "yes"),
            ("level", "-1"),
        ])));
        assert!(!renders_as_highway_tunnel(&straight_tunnel(&[
            ("railway", "rail"),
            ("tunnel", "yes"),
        ])));
    }

    #[test]
    fn tunnel_flat_underpass_builds_shell_and_carves() {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let way = straight_tunnel(&[("highway", "residential"), ("tunnel", "yes")]);
        let endpoints = TunnelInternalEndpoints::new();
        let portals = TunnelPortalMap::default();
        let mut cells = Vec::new();
        generate_highway_tunnel_shell(&mut editor, &way, &args, &endpoints, &portals, &mut cells);
        carve_highway_tunnel_interior(&mut editor, &cells);

        let asphalt = &[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA];
        let brick = &[STONE_BRICKS, CRACKED_STONE_BRICKS, MOSSY_STONE_BRICKS];

        // The road emerges at ground level at the boundary node (seamless portal join).
        assert!(
            editor.check_for_block(10, 0, 50, Some(asphalt)),
            "entrance road at ground"
        );
        // Deep interior: road buried at -7, roofed at -2, hollow between, walls at +/-3.
        assert!(
            editor.check_for_block(50, -7, 50, Some(asphalt)),
            "buried road surface"
        );
        assert!(editor.check_for_block(50, -2, 50, Some(brick)), "ceiling");
        assert!(
            !editor.check_for_block(50, -5, 50, Some(brick)),
            "interior carved out (placeholder gone)"
        );
        assert!(
            editor.check_for_block(50, -5, 53, Some(brick)),
            "side wall survives the carve"
        );
        assert!(
            !editor.check_for_block(50, 0, 50, Some(asphalt)),
            "no surface road painted over the roof"
        );
    }

    #[test]
    fn tunnel_internal_endpoint_detected() {
        let mut a = straight_tunnel(&[("highway", "residential"), ("tunnel", "yes")]);
        a.nodes[1] = ProcessedNode {
            id: 2,
            tags: StdMap::new(),
            x: 50,
            z: 50,
        };
        let mut b = straight_tunnel(&[("highway", "residential"), ("tunnel", "yes")]);
        b.id = 2;
        b.nodes[0] = ProcessedNode {
            id: 2,
            tags: StdMap::new(),
            x: 50,
            z: 50,
        };
        b.nodes[1] = ProcessedNode {
            id: 3,
            tags: StdMap::new(),
            x: 90,
            z: 50,
        };
        let elems = vec![ProcessedElement::Way(a), ProcessedElement::Way(b)];
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let internal = collect_tunnel_internal_endpoints(&elems, &xzbbox);
        assert!(internal.contains(&(50, 50)), "shared node stays at depth");
        assert!(
            !internal.contains(&(10, 50)),
            "outer end is a boundary portal"
        );
    }

    // A tunnel way from (x0,50) to (x1,50), so cell index == x - x0.
    fn tunnel_span(id: u64, x0: i32, x1: i32, tags: &[(&str, &str)]) -> ProcessedWay {
        let mut w = straight_tunnel(tags);
        w.id = id;
        w.nodes[0].x = x0;
        w.nodes[1].x = x1;
        w
    }

    fn surface_road(id: u64, x0: i32, x1: i32) -> ProcessedWay {
        let mut w = tunnel_span(id, x0, x1, &[("highway", "residential")]);
        w.tags.remove("tunnel");
        w
    }

    fn tunnel_editor(xzbbox: &XZBBox, ground: crate::ground::Ground) -> WorldEditor<'_> {
        let mut editor = test_editor(xzbbox);
        editor.set_ground(std::sync::Arc::new(ground));
        editor
    }

    fn test_portals(
        editor: &WorldEditor,
        elems: &[ProcessedElement],
        endpoints: &TunnelInternalEndpoints,
    ) -> TunnelPortalMap {
        let outlines = crate::element_processing::bridge_styles::BridgeOutlineIndex::build(&[]);
        let bridges =
            BridgeStructureMap::build(&[], editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
        collect_tunnel_portals(elems, editor, &bridges, endpoints, 1.0)
    }

    /// The descent is only usable if the ground pass cuts down to it.
    #[test]
    fn ground_pass_cuts_down_to_the_descending_approach_road() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        // A real elevation grid: only then does the ground pass read per-column Y.
        let ground =
            crate::ground::Ground::new_elevation_test(vec![vec![0.0f32; 200]; 120], 200, 120);
        let mut editor = tunnel_editor(&xzbbox, ground.clone());
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());

        let ways = [
            surface_road(2, 5, 40),
            tunnel_span(1, 40, 70, &[("highway", "residential"), ("tunnel", "yes")]),
        ];
        let elems: Vec<ProcessedElement> =
            ways.iter().cloned().map(ProcessedElement::Way).collect();
        let endpoints = collect_tunnel_internal_endpoints(&elems, &xzbbox);
        let portals = test_portals(&editor, &elems, &endpoints);
        let outlines = crate::element_processing::bridge_styles::BridgeOutlineIndex::build(&[]);
        let structures =
            BridgeStructureMap::build(&[], &editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&[], &structures, 1.0);
        let empty = CoordinateBitmap::new_empty();
        let mut cells = Vec::new();
        for elem in &elems {
            generate_highways(
                &mut editor,
                elem,
                &args,
                &HighwayConnectivityMap::new(),
                &FloodFillCache::new(),
                &empty,
                &structures,
                &surface,
                &endpoints,
                &portals,
                &empty,
                &mut cells,
                &RoadMarkingIndex::default(),
                &CoordinateBitmap::new_empty(),
            );
        }
        crate::ground_generation::generate_ground_region(
            &mut editor,
            &ground,
            &args,
            &xzbbox,
            &empty,
            &empty,
            &surface,
            0,
            120,
            40,
            60,
            false,
        );
        carve_highway_tunnel_interior(&mut editor, &cells);

        let asphalt = &[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA];
        // The road falls 1 in 3 and the ground pass follows it down.
        // Stops short of the mouth, where the bore floor takes over.
        for x in 19..=36 {
            let expected = -(7 - (40 - x) / 3);
            assert!(
                editor.check_for_block_absolute(x, expected, 50, Some(asphalt), None),
                "approach road missing at x={x}, y={expected}"
            );
            // Adjacent stamps overlap on a ramp, but the top is road either way.
            let top = editor
                .highest_block_between(x, 50, expected, 8)
                .expect("column is not empty");
            assert!(
                top <= expected + 1
                    && editor.check_for_block_absolute(x, top, 50, Some(asphalt), None),
                "approach road buried at x={x}: column tops out at y={top}, road at {expected}"
            );
        }
        assert!(
            editor.check_for_block_absolute(40, -7, 50, Some(asphalt), None),
            "the portal meets the approach at full depth"
        );
    }

    fn build_tunnel(
        editor: &mut WorldEditor,
        xzbbox: &XZBBox,
        ways: &[ProcessedWay],
    ) -> Vec<HighwayTunnelCell> {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let elems: Vec<ProcessedElement> =
            ways.iter().cloned().map(ProcessedElement::Way).collect();
        let endpoints = collect_tunnel_internal_endpoints(&elems, xzbbox);
        let mut portals = test_portals(editor, &elems, &endpoints);
        portals.plan_bores(&elems, editor, &endpoints, 1.0, None);
        let mut cells = Vec::new();
        for way in ways {
            if renders_as_highway_tunnel(way) {
                generate_highway_tunnel_shell(editor, way, &args, &endpoints, &portals, &mut cells);
            }
        }
        carve_highway_tunnel_interior(editor, &cells);
        cells
    }

    const TUNNEL_BRICK: &[Block] = &[STONE_BRICKS, CRACKED_STONE_BRICKS, MOSSY_STONE_BRICKS];

    /// The reported stone-brick outline: the shell ran up to the terrain block and
    /// nothing removes it later. Swept over slopes, which showed its worst form.
    #[test]
    fn tunnel_leaves_no_brick_exposed_at_the_surface() {
        type Shape = (&'static str, fn(usize, usize) -> f32);
        const SHAPES: &[Shape] = &[
            ("flat", |_x, _z| 0.0),
            ("ridge", |x, _z| {
                (14.0 - ((x as f32 - 55.0).abs() * 0.9)).max(0.0)
            }),
            ("cross-slope", |x, z| {
                (14.0 - ((x as f32 - 55.0).abs() * 0.9)).max(0.0) + (z as f32 - 50.0) * 0.7
            }),
            ("rolling", |x, z| {
                6.0 + (x as f32 * 0.21).sin() * 4.0 + (z as f32 * 0.13).cos() * 2.0
            }),
        ];

        for with_approach in [false, true] {
            for &(name, shape) in SHAPES {
                let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
                let mut heights = vec![vec![0.0f32; 200]; 120];
                for (z, row) in heights.iter_mut().enumerate() {
                    for (x, h) in row.iter_mut().enumerate() {
                        *h = shape(x, z);
                    }
                }
                let ground = crate::ground::Ground::new_elevation_test(heights, 200, 120);
                let mut editor = tunnel_editor(&xzbbox, ground);
                let mut ways = vec![tunnel_span(
                    1,
                    40,
                    70,
                    &[("highway", "residential"), ("tunnel", "yes")],
                )];
                if with_approach {
                    ways.push(surface_road(2, 5, 40));
                    ways.push(surface_road(3, 70, 105));
                }
                build_tunnel(&mut editor, &xzbbox, &ways);

                for x in 20..=90 {
                    for z in 40..=60 {
                        let ty = editor.terrain_level(x, z).unwrap();
                        for y in ty..=ty + 2 {
                            let brick =
                                editor.check_for_block_absolute(x, y, z, Some(TUNNEL_BRICK), None);
                            // Buried brick is structure; brick under open sky is the outline.
                            let under_sky =
                                (y + 1..=ty + 24).all(|a| !editor.block_exists_absolute(x, a, z));
                            assert!(
                                !(brick && under_sky),
                                "{name} (approach={with_approach}): brick exposed at ({x}, {y}, {z}), terrain {ty}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// A residential road along z = 50 from node `a` at `x0` to node `b` at `x1`.
    fn road_between(
        id: u64,
        (x0, a): (i32, u64),
        (x1, b): (i32, u64),
        bored: bool,
    ) -> ProcessedWay {
        let mut way = if bored {
            tunnel_span(id, x0, x1, &[("highway", "residential"), ("tunnel", "yes")])
        } else {
            surface_road(id, x0, x1)
        };
        way.nodes[0].id = a;
        way.nodes[1].id = b;
        way
    }

    fn ground_along_x(shape: impl Fn(usize) -> f32) -> crate::ground::Ground {
        let heights = (0..120).map(|_| (0..200).map(&shape).collect()).collect();
        crate::ground::Ground::new_elevation_test(heights, 200, 120)
    }

    #[test]
    fn a_road_tunnel_split_into_ways_keeps_one_grade_under_a_hill() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let hill = ground_along_x(|x| (40.0 - (x as f32 - 100.0).abs() * 0.6).max(0.0));
        let mut editor = tunnel_editor(&xzbbox, hill);
        let ways = [
            road_between(10, (5, 1), (20, 2), false),
            road_between(11, (20, 2), (100, 3), true),
            road_between(12, (180, 4), (100, 3), true),
            road_between(13, (180, 4), (195, 5), false),
        ];
        let cells = build_tunnel(&mut editor, &xzbbox, &ways);
        assert_eq!(cells.len(), 2 * 81);
        let ys: Vec<i32> = cells.iter().map(|c| c.road_y).collect();
        assert!(ys[0] < 0, "the approach leads down into the portal");
        assert!(ys.iter().all(|&y| y == ys[0]), "{ys:?}");
    }

    #[test]
    fn an_exit_too_short_to_reach_the_surface_ends_in_the_rock() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let hill = ground_along_x(|x| (40.0 - (x as f32 - 100.0).abs() * 0.6).max(0.0));
        let mut editor = tunnel_editor(&xzbbox, hill);
        let mut main = road_between(11, (20, 1), (180, 2), true);
        main.nodes.insert(
            1,
            ProcessedNode {
                id: 3,
                tags: StdMap::new(),
                x: 100,
                z: 50,
            },
        );
        let mut exit = tunnel_span(12, 100, 100, &[("highway", "footway"), ("tunnel", "yes")]);
        exit.nodes[0].id = 3;
        exit.nodes[1].id = 4;
        exit.nodes[1].z = 56;
        let cells = build_tunnel(&mut editor, &xzbbox, &[main, exit]);
        let road = cells.iter().find(|c| (c.x, c.z) == (100, 50)).unwrap();
        let ys: Vec<i32> = cells
            .iter()
            .filter(|c| c.x == 100 && c.z > 50)
            .map(|c| c.road_y)
            .collect();
        assert!(!ys.is_empty());
        assert!(
            ys.iter()
                .all(|&y| (road.road_y..=road.road_y + 2).contains(&y)),
            "road {} exit {ys:?}",
            road.road_y
        );
    }

    #[test]
    fn a_short_road_tunnel_under_flat_ground_is_roofed_in_the_middle() {
        for n in [42, 60, 80] {
            let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
            let mut editor = tunnel_editor(&xzbbox, ground_along_x(|_| 0.0));
            let ways = [road_between(11, (40, 1), (40 + n - 1, 2), true)];
            let cells = build_tunnel(&mut editor, &xzbbox, &ways);
            let mid = cells.iter().find(|c| c.x == 40 + (n - 1) / 2).unwrap();
            assert!(mid.covered, "n={n}: open to the sky in the middle");
            assert!(
                mid.road_y <= -TUNNEL_ROOF_COVER,
                "n={n}: road at {}",
                mid.road_y
            );
        }
    }

    #[test]
    fn a_branch_to_a_platform_comes_up_to_it() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let hill = ground_along_x(|x| (40.0 - (x as f32 - 100.0).abs() * 0.6).max(0.0));
        let mut editor = tunnel_editor(&xzbbox, hill);
        let mut main = road_between(11, (20, 1), (180, 2), true);
        main.nodes.insert(
            1,
            ProcessedNode {
                id: 3,
                tags: StdMap::new(),
                x: 100,
                z: 50,
            },
        );
        let mut stairs = tunnel_span(12, 100, 100, &[("highway", "footway"), ("tunnel", "yes")]);
        stairs.nodes[0].id = 3;
        stairs.nodes[1].id = 4;
        stairs.nodes[1].z = 56;
        let mut platform = tunnel_span(13, 95, 105, &[("railway", "platform")]);
        platform.tags.remove("tunnel");
        for node in &mut platform.nodes {
            node.z = 56;
        }
        platform.nodes.insert(
            1,
            ProcessedNode {
                id: 4,
                tags: StdMap::new(),
                x: 100,
                z: 56,
            },
        );
        let cells = build_tunnel(&mut editor, &xzbbox, &[main, stairs, platform]);
        let road = cells.iter().find(|c| (c.x, c.z) == (100, 50)).unwrap();
        let top = cells
            .iter()
            .filter(|c| c.x == 100 && c.z > 50)
            .map(|c| c.road_y)
            .max()
            .unwrap();
        assert!(top > road.road_y + 2, "the stairs end in the rock at {top}");
    }

    #[test]
    fn a_road_tunnel_turning_back_past_its_portal_keeps_its_walls() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let heights = (0..120)
            .map(|z| {
                (0..200)
                    .map(|x| {
                        let cut = (x <= 20 && (44..=56).contains(&z))
                            || (x <= 10 && (74..=86).contains(&z));
                        if cut {
                            0.0
                        } else {
                            40.0
                        }
                    })
                    .collect()
            })
            .collect();
        let ground = crate::ground::Ground::new_elevation_test(heights, 200, 120);
        let mut editor = tunnel_editor(&xzbbox, ground);
        let mut bend = road_between(11, (20, 1), (100, 2), true);
        for (id, x, z) in [(3, 100, 80), (4, 5, 80)] {
            bend.nodes.push(ProcessedNode {
                id,
                tags: StdMap::new(),
                x,
                z,
            });
        }
        let cells = build_tunnel(&mut editor, &xzbbox, &[bend]);
        // Back past the first portal's plane, x < 20, the return leg is still walled in.
        for c in cells
            .iter()
            .filter(|c| c.z == 80 && (12..=18).contains(&c.x))
        {
            for z in [80 - c.half_width - 1, 80 + c.half_width + 1] {
                assert!(
                    editor.check_for_block_absolute(c.x, c.road_y + 2, z, Some(TUNNEL_BRICK), None),
                    "no wall at ({}, {z})",
                    c.x
                );
            }
        }
        assert_eq!(
            cells
                .iter()
                .filter(|c| c.z == 80 && (12..=18).contains(&c.x))
                .count(),
            7
        );
    }

    #[test]
    fn a_road_tunnel_keeps_its_roof_where_the_ground_dips_over_it() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let ground = ground_along_x(|x| match x {
            ..=20 | 180.. => 0.0,
            92..=108 => -12.0,
            _ => 30.0,
        });
        let mut editor = tunnel_editor(&xzbbox, ground);
        let ways = [road_between(11, (20, 2), (180, 3), true)];
        let cells = build_tunnel(&mut editor, &xzbbox, &ways);
        let ys: Vec<i32> = cells.iter().map(|c| c.road_y).collect();
        let deepest = (0..ys.len()).min_by_key(|&i| ys[i]).unwrap();
        assert!(ys[..deepest].windows(2).all(|w| w[1] <= w[0]), "{ys:?}");
        assert!(ys[deepest..].windows(2).all(|w| w[1] >= w[0]), "{ys:?}");
        for c in cells.iter().filter(|c| (96..=104).contains(&c.x)) {
            assert!(c.covered, "open to the sky at x={}", c.x);
            // A higher neighbour carves this roof row; the roof is then further up.
            let roof = (c.road_y + 2..=c.road_y + TUNNEL_CEIL_OFFSET + 3)
                .find(|&y| editor.block_exists_absolute(c.x, y, 50))
                .expect("a roof over the road");
            assert!(roof > -12, "the gully covers the roof at x={}", c.x);
            assert!(editor.check_for_block_absolute(c.x, roof, 50, Some(TUNNEL_BRICK), None));
            assert!(!editor.block_exists_absolute(c.x, c.road_y + 2, 50));
        }
    }

    /// Confined to the tunnel way, a bore under ~43 cells never reached cover.
    #[test]
    fn approach_road_lets_a_short_tunnel_be_roofed_from_the_portal() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let mut editor = tunnel_editor(&xzbbox, crate::ground::Ground::new_flat(0));
        let tunnel = tunnel_span(1, 40, 70, &[("highway", "residential"), ("tunnel", "yes")]);
        let elems = vec![
            ProcessedElement::Way(surface_road(2, 5, 40)),
            ProcessedElement::Way(tunnel.clone()),
            ProcessedElement::Way(surface_road(3, 70, 105)),
        ];
        let endpoints = collect_tunnel_internal_endpoints(&elems, &xzbbox);
        let portals = test_portals(&editor, &elems, &endpoints);
        assert_eq!(
            portals.drop_at((40, 50)),
            TUNNEL_COVER_DROP,
            "the approach road absorbs the full drop"
        );

        let cells = build_tunnel(
            &mut editor,
            &xzbbox,
            &[surface_road(2, 5, 40), tunnel, surface_road(3, 70, 105)],
        );
        assert!(
            cells.iter().all(|c| c.covered),
            "every cell of a 31-cell tunnel is roofed, not an open trench"
        );
        assert_eq!(
            cells[0].road_y, -TUNNEL_COVER_DROP,
            "portal starts at depth"
        );
        assert!(
            editor.check_for_block(40, -2, 50, Some(TUNNEL_BRICK)),
            "roof present at the portal cell"
        );
    }

    /// Indoor mapping is never drawn, so it cannot bring anyone down to the portal.
    #[test]
    fn an_approach_mapped_indoors_does_not_lower_the_portal() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let editor = tunnel_editor(&xzbbox, crate::ground::Ground::new_flat(0));
        let mut indoor = surface_road(2, 5, 40);
        indoor.tags.insert("level".to_string(), "-1".to_string());
        let elems = vec![
            ProcessedElement::Way(indoor),
            ProcessedElement::Way(tunnel_span(
                1,
                40,
                70,
                &[("highway", "residential"), ("tunnel", "yes")],
            )),
        ];
        let endpoints = collect_tunnel_internal_endpoints(&elems, &xzbbox);
        assert_eq!(
            test_portals(&editor, &elems, &endpoints).drop_at((40, 50)),
            0
        );
    }

    /// The descent is bounded by the run on offer, and a connector serves both portals.
    #[test]
    fn approach_drop_is_bounded_by_the_road_it_runs_on() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let editor = tunnel_editor(&xzbbox, crate::ground::Ground::new_flat(0));
        let short = vec![
            ProcessedElement::Way(surface_road(2, 34, 40)),
            ProcessedElement::Way(tunnel_span(
                1,
                40,
                70,
                &[("highway", "residential"), ("tunnel", "yes")],
            )),
        ];
        let endpoints = collect_tunnel_internal_endpoints(&short, &xzbbox);
        // Seven cells cannot deliver seven blocks at 1-in-3, so the descent steepens.
        assert_eq!(
            test_portals(&editor, &short, &endpoints).drop_at((40, 50)),
            6
        );

        let connector = vec![
            ProcessedElement::Way(tunnel_span(
                1,
                10,
                40,
                &[("highway", "residential"), ("tunnel", "yes")],
            )),
            ProcessedElement::Way(surface_road(2, 40, 70)),
            ProcessedElement::Way(tunnel_span(
                3,
                70,
                100,
                &[("highway", "residential"), ("tunnel", "yes")],
            )),
        ];
        let endpoints = collect_tunnel_internal_endpoints(&connector, &xzbbox);
        let portals = test_portals(&editor, &connector, &endpoints);
        assert_eq!(
            (portals.drop_at((40, 50)), portals.drop_at((70, 50))),
            (TUNNEL_COVER_DROP, TUNNEL_COVER_DROP),
            "a road joining two portals descends into both and stays down between"
        );
    }

    /// A neighbour a block up the ramp carves through this cell's ceiling row, so the
    /// light goes under the roof that is left rather than into the air below it.
    #[test]
    fn tunnel_lights_hang_from_the_roof_left_after_carving() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let cell = |x: i32, road_y: i32, light: bool| HighwayTunnelCell {
            x,
            z: 20,
            road_y,
            half_width: 2,
            covered: true,
            carve_top: road_y + TUNNEL_CEIL_OFFSET - 1,
            palette: &[GRAY_CONCRETE],
            light,
            faces: [None, None],
        };
        let cells = vec![cell(20, 0, true), cell(21, 1, false)];
        for c in &cells {
            for dx in -3..=3 {
                for dz in -3..=3 {
                    for y in -1..=c.road_y + TUNNEL_CEIL_OFFSET {
                        editor.set_block_absolute(STONE_BRICKS, c.x + dx, y, c.z + dz, None, None);
                    }
                }
            }
        }
        let lights = carve_highway_tunnel_interior(&mut editor, &cells);
        hang_tunnel_lights(&mut editor, &lights);

        let light = (1..=8)
            .find(|&y| editor.check_for_block_absolute(20, y, 20, Some(&[SEA_LANTERN]), None))
            .expect("the bore keeps its light");
        assert!(
            editor.block_exists_absolute(20, light + 1, 20),
            "the light at y={light} hangs in the air"
        );
    }

    /// A road bore crossing a rail tunnel opens the rail roof, so no lantern hangs before both are carved.
    #[test]
    fn a_road_bore_crossing_a_rail_tunnel_leaves_no_light_in_the_air() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        for x in 5..35 {
            for z in 5..35 {
                for y in -10..0 {
                    editor.set_block_absolute(STONE, x, y, z, None, None);
                }
            }
        }
        let rail: Vec<_> = (10..=30).map(|x| (x, 20, -8, x == 20)).collect();
        let road: Vec<_> = (10..=30)
            .map(|z| HighwayTunnelCell {
                x: 20,
                z,
                road_y: -5,
                half_width: 2,
                covered: true,
                carve_top: -5 + TUNNEL_CEIL_OFFSET - 1,
                palette: &[GRAY_CONCRETE],
                light: false,
                faces: [None, None],
            })
            .collect();
        let mut lights =
            crate::element_processing::railways::carve_rail_tunnel_interior(&mut editor, &rail);
        lights.extend(carve_highway_tunnel_interior(&mut editor, &road));
        hang_tunnel_lights(&mut editor, &lights);

        for x in 5..35 {
            for z in 5..35 {
                for y in -10..4 {
                    if editor.check_for_block_absolute(x, y, z, Some(&[SEA_LANTERN]), None) {
                        assert!(
                            editor.block_exists_absolute(x, y + 1, z),
                            "the light at ({x}, {y}, {z}) hangs in the air"
                        );
                    }
                }
            }
        }
    }

    /// The shell stamps a square per cell, which used to seal a roofed portal.
    #[test]
    fn tunnel_mouth_is_not_walled_off() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let mut editor = tunnel_editor(&xzbbox, crate::ground::Ground::new_flat(0));
        let tunnel = tunnel_span(1, 40, 70, &[("highway", "residential"), ("tunnel", "yes")]);
        let elems = vec![
            ProcessedElement::Way(surface_road(2, 5, 40)),
            ProcessedElement::Way(tunnel.clone()),
        ];
        let endpoints = collect_tunnel_internal_endpoints(&elems, &xzbbox);
        let portals = test_portals(&editor, &elems, &endpoints);
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let mut cells = Vec::new();
        generate_highway_tunnel_shell(
            &mut editor,
            &tunnel,
            &args,
            &endpoints,
            &portals,
            &mut cells,
        );
        carve_highway_tunnel_interior(&mut editor, &cells);

        let road_y = cells[0].road_y;
        for z in 48..=52 {
            // Nothing past the portal: carving there costs the approach a tread.
            for y in (road_y - 1)..=(road_y + TUNNEL_CEIL_OFFSET) {
                assert!(
                    !editor.block_exists_absolute(39, y, z),
                    "the bore reached past the portal at (39, {y}, {z})"
                );
            }
            // The portal is open; the carve writes AIR, which counts as open.
            for y in (road_y + 1)..=(road_y + TUNNEL_CEIL_OFFSET - 1) {
                let open = !editor.block_exists_absolute(40, y, z)
                    || editor.check_for_block_absolute(40, y, z, Some(&[AIR, SEA_LANTERN]), None);
                assert!(open, "the mouth is blocked at (40, {y}, {z})");
            }
            assert!(
                editor.check_for_block_absolute(
                    40,
                    road_y,
                    z,
                    Some(&[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA]),
                    None
                ),
                "carriageway reaches the portal at z={z}"
            );
        }
    }

    /// Roof and bore placeholder are both stone brick, so the carve needs a clamp.
    #[test]
    fn open_cut_does_not_carve_the_neighbouring_roof() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let mut editor = tunnel_editor(&xzbbox, crate::ground::Ground::new_flat(0));
        // No approach road, so the portals stay open-cut.
        let cells = build_tunnel(
            &mut editor,
            &xzbbox,
            &[tunnel_span(
                1,
                10,
                110,
                &[("highway", "residential"), ("tunnel", "yes")],
            )],
        );
        // The roof sits at the highest ceiling stamping the column; what matters is
        // that something seals the bore off.
        for cell in cells.iter().filter(|c| c.covered) {
            for dz in -cell.half_width..=cell.half_width {
                let sealed = (cell.road_y + 1..=0)
                    .any(|y| editor.check_for_block(cell.x, y, cell.z + dz, Some(TUNNEL_BRICK)));
                assert!(
                    sealed,
                    "bore open to the sky at ({}, {}) (road_y={})",
                    cell.x,
                    cell.z + dz,
                    cell.road_y
                );
            }
        }
    }

    /// A one-block DEM wiggle used to flip `covered` off and punch a skylight.
    #[test]
    fn a_one_block_dip_does_not_punch_a_skylight() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        // One column low just inside the portal, where the ramp pins the road and it
        // cannot dig out of the dip. Cover is 6, which TUNNEL_ROOF_COVER still keeps.
        let mut heights = vec![vec![0.0f32; 200]; 120];
        for row in heights.iter_mut() {
            row[41] = -1.0;
        }
        let ground = crate::ground::Ground::new_elevation_test(heights, 200, 120);
        let mut editor = tunnel_editor(&xzbbox, ground);
        let cells = build_tunnel(
            &mut editor,
            &xzbbox,
            &[
                surface_road(2, 5, 40),
                tunnel_span(1, 40, 80, &[("highway", "residential"), ("tunnel", "yes")]),
                surface_road(3, 80, 115),
            ],
        );
        let dip = cells
            .iter()
            .find(|c| c.x == 41)
            .expect("the dipped cell was generated");
        assert_eq!(
            dip.road_y, -TUNNEL_COVER_DROP,
            "the ramp pins the road here"
        );
        assert_eq!(
            editor.terrain_level(41, 50).unwrap() - dip.road_y,
            TUNNEL_ROOF_COVER,
            "cover in the dip is exactly the keep-the-roof threshold"
        );
        assert!(
            cells.iter().all(|c| c.covered),
            "a 1-block dip in the overburden does not open a shaft"
        );
    }

    /// The reported pale stripes, for a road tunnel: its roof sat two under the water's
    /// surface, and the bed over it stayed there while the river around it was carved.
    #[test]
    fn a_road_tunnel_under_a_river_runs_below_the_carved_bed() {
        let _g = crate::world_editor::FLOOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let cells = road_tunnel_under_river(0);
        let mid = cells.iter().find(|c| c.x == 100).unwrap();
        assert!(
            mid.road_y <= -14,
            "road at {} under a bed at -7",
            mid.road_y
        );
    }

    /// The base the terrain fit lifts an area to when a bore runs under its water is
    /// exactly enough to keep the bore under the deepest bed.
    #[test]
    fn a_road_tunnel_fits_under_a_river_on_the_lifted_base() {
        let _g = crate::world_editor::FLOOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // The carve floor for the deepest tier, and the room for a bore on top.
        let cells = road_tunnel_under_river(
            crate::world_editor::min_y() + 6 + 2 + crate::water_depth::BORE_ROOM,
        );
        let lowest = crate::element_processing::tunnel_grade::lowest_floor();
        assert!(
            cells.iter().all(|c| c.road_y > lowest),
            "under the floor guard"
        );
    }

    /// A road tunnel under a river 100 blocks wide at `level`, opening on dry land either
    /// side, built in the generator's order and checked for a river bed over it.
    fn road_tunnel_under_river(level: i32) -> Vec<HighwayTunnelCell> {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let ground =
            crate::ground::test_support::ground_with_water(200, 120, level as f32, |x, _| {
                (50..150).contains(&x)
            });
        let field = std::sync::Arc::new(crate::water_depth::compute_big_water_field(
            &ground,
            &xzbbox,
            Default::default(),
            1.0,
        ));
        let mut editor = tunnel_editor(&xzbbox, ground.clone());
        editor.set_water_field(std::sync::Arc::clone(&field));
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let way = tunnel_span(1, 10, 190, &[("highway", "residential"), ("tunnel", "yes")]);
        let elems = vec![ProcessedElement::Way(way.clone())];
        let endpoints = collect_tunnel_internal_endpoints(&elems, &xzbbox);
        let portals = test_portals(&editor, &elems, &endpoints);
        let footprint = collect_tunnel_footprint(&elems, &editor, &endpoints, &xzbbox, 1.0);

        // In the generator's order: the shell, the river carved over it, then the bore.
        let mut cells = Vec::new();
        assert!(generate_highway_tunnel_shell(
            &mut editor,
            &way,
            &args,
            &endpoints,
            &portals,
            &mut cells
        ));
        crate::water_depth::carve_lc_water_region(
            &mut editor,
            &ground,
            &xzbbox,
            &field,
            &CoordinateBitmap::new_empty(),
            &footprint,
            &BridgeSurfaceMap::empty(),
            xzbbox.min_x(),
            xzbbox.max_x(),
            xzbbox.min_z(),
            xzbbox.max_z(),
        );
        carve_highway_tunnel_interior(&mut editor, &cells);

        let wall = cells[0].half_width + 1;
        for x in 50..150 {
            for z in 50 - wall..=50 + wall {
                let bed = editor.carved_bed(x, z).expect("river");
                // The bed the rest of the river has, not one left on the roof.
                assert!(
                    editor.block_exists_absolute(x, bed, z)
                        && !editor.check_for_block_absolute(x, bed, z, Some(&[WATER]), None),
                    "no bed at ({x}, {bed}, {z})"
                );
                for y in bed - 1..=level {
                    assert!(
                        !editor.check_for_block_absolute(x, y, z, Some(TUNNEL_BRICK), None),
                        "shell in the river at ({x}, {y}, {z}), bed {bed}"
                    );
                }
            }
        }
        // Roofed, paved and dry the whole way under the river. On a ramp a neighbour's
        // stamp sits in the bore and carves the roof a row up, so it is hollow on the flat.
        let asphalt = &[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA];
        for cell in cells.iter().filter(|c| (50..150).contains(&c.x)) {
            assert!(cell.covered, "open cut at x={}", cell.x);
            let (x, z, road) = (cell.x, cell.z, cell.road_y);
            assert!(editor.check_for_block_absolute(x, road, z, Some(asphalt), None));
            let bed = editor.carved_bed(x, z).unwrap();
            for y in road..bed {
                assert!(
                    !editor.check_for_block_absolute(x, y, z, Some(&[WATER]), None),
                    "water in the bore at ({x}, {y}, {z})"
                );
            }
            let roof = road + TUNNEL_CEIL_OFFSET;
            assert!(
                (roof..=roof + 2).any(|y| editor.check_for_block_absolute(
                    x,
                    y,
                    z,
                    Some(TUNNEL_BRICK),
                    None
                )),
                "no roof at x={x}"
            );
            if (90..110).contains(&x) {
                for y in road + 1..roof {
                    assert!(
                        !editor.block_exists_absolute(x, y, z),
                        "bore filled at ({x}, {y}, {z})"
                    );
                }
            }
        }
        cells
    }

    /// With no vertical room the bore used to emit a few stone-brick pads.
    #[test]
    fn tunnel_declines_when_the_world_floor_leaves_no_room() {
        let _guard = crate::world_editor::FLOOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        crate::world_editor::set_terrain_floor_y(-62);
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let mut editor = tunnel_editor(&xzbbox, crate::ground::Ground::new_flat(-62));
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let way = straight_tunnel(&[("highway", "residential"), ("tunnel", "yes")]);
        let mut cells = Vec::new();
        let bored = generate_highway_tunnel_shell(
            &mut editor,
            &way,
            &args,
            &TunnelInternalEndpoints::new(),
            &TunnelPortalMap::default(),
            &mut cells,
        );
        crate::world_editor::set_terrain_floor_y(crate::world_editor::DEFAULT_MIN_Y);

        assert!(!bored, "no room to bore: fall through to a surface road");
        assert!(cells.is_empty(), "and nothing is stamped into the world");
    }

    /// `layer` is untrusted; the old product overflowed and erased the way.
    #[test]
    fn absurd_layer_tag_neither_panics_nor_erases_the_tunnel() {
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        for layer in ["-306783380", "-20", "-2"] {
            let mut editor = tunnel_editor(&xzbbox, crate::ground::Ground::new_flat(0));
            let cells = build_tunnel(
                &mut editor,
                &xzbbox,
                &[tunnel_span(
                    1,
                    10,
                    90,
                    &[
                        ("highway", "residential"),
                        ("tunnel", "yes"),
                        ("layer", layer),
                    ],
                )],
            );
            assert_eq!(cells.len(), 81, "layer={layer} still renders the full way");
            assert!(
                cells.iter().all(|c| c.road_y >= tunnel_min_road_y()),
                "layer={layer} keeps the road above the bedrock plane"
            );
        }
    }

    /// A way between two arbitrary points, so the diagonal case can be built.
    fn way_between(id: u64, a: (i32, i32), b: (i32, i32), tags: &[(&str, &str)]) -> ProcessedWay {
        let mut t = StdMap::new();
        for (k, v) in tags {
            t.insert(k.to_string(), v.to_string());
        }
        ProcessedWay {
            id,
            nodes: vec![
                ProcessedNode {
                    id: 1,
                    tags: StdMap::new(),
                    x: a.0,
                    z: a.1,
                },
                ProcessedNode {
                    id: 2,
                    tags: StdMap::new(),
                    x: b.0,
                    z: b.1,
                },
            ],
            tags: t,
        }
    }

    /// Element processing, ground pass, then carve: the real order.
    fn render_scene(
        editor: &mut WorldEditor,
        ground: &crate::ground::Ground,
        xzbbox: &XZBBox,
        ways: &[ProcessedWay],
    ) {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let elems: Vec<ProcessedElement> =
            ways.iter().cloned().map(ProcessedElement::Way).collect();
        let endpoints = collect_tunnel_internal_endpoints(&elems, xzbbox);
        let portals = test_portals(editor, &elems, &endpoints);
        let footprint = collect_tunnel_footprint(&elems, editor, &endpoints, xzbbox, 1.0);
        let outlines = crate::element_processing::bridge_styles::BridgeOutlineIndex::build(&[]);
        let structures =
            BridgeStructureMap::build(&[], editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&[], &structures, 1.0);
        let empty = CoordinateBitmap::new_empty();
        let mut cells = Vec::new();
        for elem in &elems {
            generate_highways(
                editor,
                elem,
                &args,
                &HighwayConnectivityMap::new(),
                &FloodFillCache::new(),
                &empty,
                &structures,
                &surface,
                &endpoints,
                &portals,
                &footprint,
                &mut cells,
                &RoadMarkingIndex::default(),
                &CoordinateBitmap::new_empty(),
            );
        }
        crate::ground_generation::generate_ground_region(
            editor, ground, &args, xzbbox, &empty, &footprint, &surface, 0, 199, 0, 199, false,
        );
        carve_highway_tunnel_interior(editor, &cells);
    }

    /// A verge keeps its ground where land cover keeps it green, and next to the trunk of a
    /// canopy tree that will stand: not one on the road's own lanes, nor one under a mapped
    /// tree's crown.
    #[test]
    fn a_cut_runs_outward_and_a_parked_side_reaches_its_own_kerb() {
        // Past a core of 2, ground kept at (4, 0) leaves (5, 0) beyond it unpaved too, but
        // ground kept further out leaves the road up to it.
        assert!(cut_outward(2, 5, 0, |x, z| (x, z) == (4, 0)));
        assert!(cut_outward(2, 4, 0, |x, z| (x, z) == (4, 0)));
        assert!(!cut_outward(2, 3, 0, |x, z| (x, z) == (4, 0)));
        assert!(!cut_outward(2, 5, 0, |x, z| (x, z) == (6, 0)));
        // Inside the core nothing is cut.
        assert!(!cut_outward(2, 1, 1, |_, _| true));
        // A way running east has its right to the south (+z): cars parked there take it out
        // to 5, the left stops at 2.
        let east = Some((1.0, 0.0));
        assert!(in_side_stamp(0, 5, east, (2, 5)));
        assert!(!in_side_stamp(0, -3, east, (2, 5)));
        assert!(in_side_stamp(0, -2, east, (2, 5)));
        // Alike, or with no direction, the disc of the wider side.
        assert!(in_side_stamp(0, -5, None, (2, 5)));
        assert_eq!(in_side_stamp(3, 3, east, (4, 4)), in_road_stamp(3, 3, 4));
    }

    #[test]
    fn a_verge_keeps_its_ground_for_grass_and_for_trees_that_will_stand() {
        use crate::canopy::CanopyData;
        use crate::land_cover::{LandCoverData, LC_BUILT_UP, LC_GRASSLAND, LC_TREE_COVER};
        let cover = |class: u8| LandCoverData {
            grid: vec![vec![class]],
            water_distance: vec![vec![0]],
            water_blend_cache: once_cell::sync::OnceCell::new(),
            beach_tiles: once_cell::sync::OnceCell::new(),
            inland_water: Default::default(),
            width: 1,
            height: 1,
            cells_per_meter: 1.0,
        };
        let xzbbox = XZBBox::rect_from_xz_lengths(64.0, 64.0).unwrap();
        let open = |_: i32, _: i32| false;
        // A lawn keeps all of it.
        let mut editor = test_editor(&xzbbox);
        editor.set_ground(std::sync::Arc::new(
            crate::ground::Ground::new_flat_land_cover_test(cover(LC_GRASSLAND), 64, 64),
        ));
        assert!(editor.keeps_its_ground(20, 20, open));
        // Not along rows of parked cars, where only a tree keeps its pit.
        assert!(!editor.keeps_a_pit(20, 20, open));
        // Tree cover is no verge: it takes in the streets its crowns line.
        editor.set_ground(std::sync::Arc::new(
            crate::ground::Ground::new_flat_land_cover_test(cover(LC_TREE_COVER), 64, 64),
        ));
        assert!(!editor.keeps_its_ground(20, 20, open));
        // Built-up ground under a closed canopy: only round the trunk slots.
        let canopy = CanopyData::from_grid(vec![20; 64 * 64], 64, 64);
        let ground = std::sync::Arc::new(
            crate::ground::Ground::new_flat_canopy_test(canopy, 64, 64)
                .with_land_cover_test(cover(LC_BUILT_UP)),
        );
        editor.set_ground(std::sync::Arc::clone(&ground));
        let spacing = editor.tree_slot_spacing();
        let slot = crate::trees::schematic::trunk_slot_s(30, 30, spacing);
        let (_, tree) = crate::ground_generation::canopy_verdict(
            &ground, slot.0, slot.1, 0, 0, spacing, None, 1.0,
        );
        assert!(tree, "a closed canopy plants its slot");
        assert!(editor.keeps_its_ground(slot.0 + 1, slot.1, open));
        assert!(editor.keeps_a_pit(slot.0 + 1, slot.1, open));
        // A trunk on the road's own lanes never stands, so it keeps nothing.
        assert!(!editor.keeps_its_ground(slot.0 + 1, slot.1, |x, z| (x, z) == slot));
        // Nor one under a mapped tree's crown; the mapped tree keeps its own pit instead,
        // unless it stands on the lanes.
        let trunk = (slot.0 + 3, slot.1);
        let mapped = vec![ProcessedElement::Node(ProcessedNode {
            id: 9,
            tags: [("natural".to_string(), "tree".to_string())].into(),
            x: trunk.0,
            z: trunk.1,
        })];
        editor.set_mapped_trunks(std::sync::Arc::new(
            crate::trees::mapped::MappedTrunks::collect(&mapped, 1.0),
        ));
        assert!(editor.under_mapped_crown(slot.0, slot.1));
        assert!(!editor.keeps_its_ground(slot.0 - 1, slot.1, open));
        assert!(editor.keeps_its_ground(trunk.0 + 1, trunk.1 + 1, open));
        assert!(!editor.keeps_its_ground(trunk.0 + 2, trunk.1, open));
        assert!(!editor.keeps_its_ground(trunk.0 + 1, trunk.1, |x, z| (x, z) == trunk));
    }

    /// The trunks found once for the area keep the ground the slots worked out cell by cell
    /// keep, mapped ones among them.
    #[test]
    fn the_trunk_index_keeps_the_ground_the_slots_keep() {
        use crate::canopy::CanopyData;
        use crate::land_cover::{LandCoverData, LC_BUILT_UP};
        let built_up = LandCoverData {
            grid: vec![vec![LC_BUILT_UP]],
            water_distance: vec![vec![0]],
            water_blend_cache: once_cell::sync::OnceCell::new(),
            beach_tiles: once_cell::sync::OnceCell::new(),
            inland_water: Default::default(),
            width: 1,
            height: 1,
            cells_per_meter: 1.0,
        };
        let xzbbox = XZBBox::rect_from_xz_lengths(64.0, 64.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        // A patchy canopy, so some slots grow a tree and some do not.
        let heights: Vec<u8> = (0..64 * 64)
            .map(|i| {
                if (i % 64 * 7 + i / 64 * 13) % 5 < 2 {
                    20
                } else {
                    0
                }
            })
            .collect();
        editor.set_ground(std::sync::Arc::new(
            crate::ground::Ground::new_flat_canopy_test(
                CanopyData::from_grid(heights, 64, 64),
                64,
                64,
            )
            .with_land_cover_test(built_up),
        ));
        let mapped = vec![ProcessedElement::Node(ProcessedNode {
            id: 9,
            tags: [("natural".to_string(), "tree".to_string())].into(),
            x: 12,
            z: 40,
        })];
        editor.set_mapped_trunks(std::sync::Arc::new(
            crate::trees::mapped::MappedTrunks::collect(&mapped, 1.0),
        ));
        let lanes = |_: i32, z: i32| (28..=32).contains(&z) || z == 40;
        let answers = |editor: &WorldEditor| {
            (0..64)
                .flat_map(|x| (0..64).map(move |z| (x, z)))
                .map(|(x, z)| {
                    (
                        editor.keeps_its_ground(x, z, |_, _| false),
                        editor.keeps_its_ground(x, z, lanes),
                    )
                })
                .collect::<Vec<_>>()
        };
        let one_by_one = answers(&editor);
        editor.index_standing_trunks();
        assert_eq!(answers(&editor), one_by_one);
        let kept = one_by_one.iter().filter(|a| a.0).count();
        assert!(kept > 0 && kept < 64 * 64);
        assert!(
            one_by_one.iter().any(|a| a.0 != a.1),
            "the lanes pave over some"
        );
        assert!(one_by_one[12 * 64 + 41].0 && !one_by_one[12 * 64 + 41].1);
    }

    /// Cars parked across the kerb get a stall line every stall, kerb to lane edge, on their
    /// side only; the lanes stay bare.
    #[test]
    fn parking_rows_are_marked_a_stall_apart() {
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let mut editor = tunnel_editor(&xzbbox, flat_ground());
        let street = way_between(
            1,
            (10, 30),
            (110, 30),
            &[
                ("highway", "residential"),
                ("oneway", "yes"),
                ("parking:right", "lane"),
                ("parking:right:orientation", "perpendicular"),
            ],
        );
        // Running a cell north for every four east: its lines run square to the grid.
        let slanted = way_between(
            2,
            (10, 70),
            (110, 45),
            &[
                ("highway", "residential"),
                ("oneway", "yes"),
                ("parking:right", "lane"),
                ("parking:right:orientation", "diagonal"),
            ],
        );
        // Cars parked along the kerb get no marks.
        let along = way_between(
            3,
            (10, 100),
            (110, 100),
            &[
                ("highway", "residential"),
                ("parking:both", "lane"),
                ("parking:both:orientation", "parallel"),
            ],
        );
        let halves = road_width::road_halves("residential", &street.tags, 1.0);
        let elems = vec![
            ProcessedElement::Way(street.clone()),
            ProcessedElement::Way(slanted),
            ProcessedElement::Way(along.clone()),
        ];
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let endpoints = collect_tunnel_internal_endpoints(&elems, &xzbbox);
        let portals = test_portals(&editor, &elems, &endpoints);
        let outlines = crate::element_processing::bridge_styles::BridgeOutlineIndex::build(&[]);
        let structures =
            BridgeStructureMap::build(&[], &editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&[], &structures, 1.0);
        let empty = CoordinateBitmap::new_empty();
        for element in &elems {
            generate_highways(
                &mut editor,
                element,
                &args,
                &HighwayConnectivityMap::new(),
                &FloodFillCache::new(),
                &empty,
                &structures,
                &surface,
                &endpoints,
                &portals,
                &empty,
                &mut Vec::new(),
                &RoadMarkingIndex::default(),
                &empty,
            );
        }
        let white = |x: i32, z: i32| editor.check_for_block(x, 0, z, Some(&[WHITE_CONCRETE]));
        // Heading east, the right is south: the row is z = 30 + lanes + 1 ..= 30 + kerb.
        let row = 30 + halves.lanes + 1..=30 + halves.kerb;
        let marked: Vec<i32> = (20..100)
            .filter(|&x| row.clone().all(|z| white(x, z)))
            .collect();
        assert!(marked.len() >= 20, "{marked:?}");
        assert!(marked.windows(2).all(|w| w[1] - w[0] == 3), "{marked:?}");
        // The lanes and the other side are left bare.
        for x in 20..100 {
            for z in 30 - halves.kerb..=30 + halves.lanes {
                assert!(!white(x, z), "({x}, {z})");
            }
        }
        // On the slanted street every line keeps to one column, a stall from the next.
        let lines: Vec<(i32, i32)> = (20..100)
            .flat_map(|x| (40..80).map(move |z| (x, z)))
            .filter(|&(x, z)| white(x, z))
            .collect();
        assert!(lines.len() >= 20, "{lines:?}");
        for &(x, z) in &lines {
            assert!(
                !white(x - 1, z) && !white(x + 1, z),
                "({x}, {z}) in {lines:?}"
            );
        }
        // The street with cars along its kerbs is bare but for its lane line.
        let parallel = road_width::road_halves("residential", &along.tags, 1.0);
        for x in 20..100 {
            for z in 100 - parallel.kerb..=100 + parallel.kerb {
                if (z - 100).abs() > 0 {
                    assert!(!white(x, z), "({x}, {z})");
                }
            }
        }
    }

    /// A caller that reads only the lanes gets them as the whole stamp gives them, the
    /// second lane cut at a house front as there.
    #[test]
    fn the_lanes_alone_come_as_the_whole_stamp_gives_them() {
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let editor = tunnel_editor(&xzbbox, flat_ground());
        let elems = vec![
            ProcessedElement::Way(way_between(
                1,
                (10, 50),
                (110, 50),
                &[("highway", "residential"), ("sidewalk", "both")],
            )),
            ProcessedElement::Way(way_between(
                2,
                (10, 40),
                (110, 40),
                &[("highway", "footway")],
            )),
        ];
        let mut houses = CoordinateBitmap::new(&xzbbox);
        for x in 40..=60 {
            for z in 50..=60 {
                houses.set(x, z);
            }
        }
        let none = CoordinateBitmap::new_empty();
        let lanes = |footprints: &CoordinateBitmap, lanes_only: bool| {
            let mut cells = Vec::new();
            stamp_highway_surfaces(
                &elems,
                Some(&editor),
                1.0,
                Default::default(),
                footprints,
                Some(&none),
                |_| true,
                lanes_only,
                |carriageway, x, z| {
                    if carriageway {
                        cells.push((x, z));
                    }
                },
                |_, _| {},
            );
            cells.sort_unstable();
            cells.dedup();
            cells
        };
        let open = lanes(&none, false);
        let walled = lanes(&houses, false);
        assert!(!walled.is_empty());
        assert_ne!(walled, open, "the houses cut the second lane");
        assert_eq!(lanes(&none, true), open);
        assert_eq!(lanes(&houses, true), walled);
    }

    /// The renderer leaves unpaved what the masks leave out: the second lane of a street
    /// stops at a house front and at a sidewalk mapped as its own way, its core runs on.
    #[test]
    fn the_renderer_cuts_the_strip_the_masks_cut() {
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let mut editor = tunnel_editor(&xzbbox, flat_ground());
        let street = way_between(
            1,
            (10, 50),
            (110, 50),
            &[("highway", "residential"), ("sidewalk", "both")],
        );
        let sidewalk = way_between(
            2,
            (70, 47),
            (90, 47),
            &[("highway", "footway"), ("footway", "sidewalk")],
        );
        let mut houses = CoordinateBitmap::new(&xzbbox);
        for x in 40..=60 {
            for z in 53..=60 {
                houses.set(x, z);
            }
        }
        let elems = vec![
            ProcessedElement::Way(street.clone()),
            ProcessedElement::Way(sidewalk),
        ];
        let coords = collect_road_and_carriageway_coords(
            &elems,
            &editor,
            &xzbbox,
            1.0,
            Default::default(),
            &houses,
        );
        editor.set_mapped_paths(std::sync::Arc::new(coords.paths));
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let endpoints = collect_tunnel_internal_endpoints(&elems, &xzbbox);
        let portals = test_portals(&editor, &elems, &endpoints);
        let outlines = crate::element_processing::bridge_styles::BridgeOutlineIndex::build(&[]);
        let structures =
            BridgeStructureMap::build(&[], &editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&[], &structures, 1.0);
        let empty = CoordinateBitmap::new_empty();
        generate_highways(
            &mut editor,
            &ProcessedElement::Way(street),
            &args,
            &HighwayConnectivityMap::new(),
            &FloodFillCache::new(),
            &coords.roads,
            &structures,
            &surface,
            &endpoints,
            &portals,
            &empty,
            &mut Vec::new(),
            &RoadMarkingIndex::default(),
            &houses,
        );
        let paved = |x: i32, z: i32| editor.highest_block_between(x, z, -12, 8).is_some();
        // Before the house its seven blocks, in front of it the core's five.
        assert!(paved(20, 53) && paved(20, 47));
        assert!(paved(50, 52) && !paved(50, 53));
        // Its strip stops at the sidewalk, which draws itself.
        assert!(paved(80, 48) && !paved(80, 47) && paved(80, 53));
        // Where the masks cut, the renderer did.
        for x in 10..=110 {
            for z in 46..=54 {
                if coords.cut.contains(x, z) {
                    assert!(!paved(x, z), "({x}, {z})");
                }
            }
        }
    }

    fn flat_ground() -> crate::ground::Ground {
        crate::ground::Ground::new_elevation_test(vec![vec![0.0f32; 200]; 200], 200, 200)
    }

    /// The approach's last stamps land inside the roofed footprint; registering
    /// their depth there left the roof bare to the sky.
    #[test]
    fn tunnel_roof_is_never_bare_to_the_sky() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let ground = flat_ground();
        let mut editor = tunnel_editor(&xzbbox, ground.clone());
        render_scene(
            &mut editor,
            &ground,
            &xzbbox,
            &[
                way_between(2, (5, 100), (60, 100), &[("highway", "residential")]),
                way_between(
                    1,
                    (60, 100),
                    (90, 100),
                    &[("highway", "residential"), ("tunnel", "yes")],
                ),
                way_between(3, (90, 100), (145, 100), &[("highway", "residential")]),
            ],
        );

        for x in 40..=110 {
            for z in 94..=106 {
                let top = editor
                    .highest_block_between(x, z, -12, 8)
                    .expect("no column is empty");
                assert!(
                    !editor.check_for_block_absolute(x, top, z, Some(TUNNEL_BRICK), None),
                    "bare stone brick on top at ({x}, {top}, {z})"
                );
            }
        }
        // And the cover really is there rather than the roof merely being deeper.
        for x in 62..=88 {
            assert!(
                editor.check_for_block_absolute(x, 0, 100, Some(&[GRASS_BLOCK]), None),
                "no ground closed over the bore at x={x}"
            );
        }

        // The face sits on the portal node, not `half_width + 1` short of it.
        assert!(
            editor
                .highest_block_between(59, 100, -14, 8)
                .is_some_and(|y| y < 0),
            "the cut is still open one cell short of the portal"
        );
        assert_eq!(
            editor.highest_block_between(60, 100, -14, 8),
            Some(0),
            "ground closes over the bore exactly at the portal node"
        );
    }

    /// The bore used to carve one cell past the portal, costing the approach a
    /// tread it could not repave: a missing block row at the mouth.
    #[test]
    fn carriageway_has_no_gap_at_the_portal() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let ground = flat_ground();
        let mut editor = tunnel_editor(&xzbbox, ground.clone());
        render_scene(
            &mut editor,
            &ground,
            &xzbbox,
            &[
                // Wide on purpose, so the bore's stamp reaches back past the portal.
                way_between(2, (5, 100), (60, 100), &[("highway", "motorway")]),
                way_between(
                    1,
                    (60, 100),
                    (90, 100),
                    &[("highway", "motorway"), ("tunnel", "yes")],
                ),
                way_between(3, (90, 100), (145, 100), &[("highway", "motorway")]),
            ],
        );

        // Lane markings count: a wide road has a painted centreline.
        let asphalt = &[
            GRAY_CONCRETE_POWDER,
            CYAN_TERRACOTTA,
            WHITE_CONCRETE,
            BLACK_CONCRETE,
            LIGHT_GRAY_CONCRETE,
            GRAY_CONCRETE,
        ];
        let top_road = |x: i32| -> Option<i32> {
            (-14..=2)
                .rev()
                .find(|&y| editor.check_for_block_absolute(x, y, 100, Some(asphalt), None))
        };
        // Across both portals and the bore between them.
        let mut prev = top_road(45).expect("road at x=45");
        for x in 46..=105 {
            let y = top_road(x).unwrap_or_else(|| panic!("no carriageway at all at x={x}"));
            assert!(
                (y - prev).abs() <= TUNNEL_RAMP_STEP,
                "carriageway steps {} blocks between x={} and x={x}",
                (y - prev).abs(),
                x - 1
            );
            prev = y;
        }
    }

    /// Indexing the descent by the dominant axis tips a 45-degree carriageway sideways.
    #[test]
    fn approach_ramp_stays_level_across_a_diagonal_road() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let ground = flat_ground();
        let mut editor = tunnel_editor(&xzbbox, ground.clone());
        render_scene(
            &mut editor,
            &ground,
            &xzbbox,
            &[
                way_between(2, (10, 10), (60, 60), &[("highway", "residential")]),
                way_between(
                    1,
                    (60, 60),
                    (90, 90),
                    &[("highway", "residential"), ("tunnel", "yes")],
                ),
                way_between(3, (90, 90), (140, 140), &[("highway", "residential")]),
            ],
        );

        // Perpendicular to a (1, 1) way is (1, -1); the 5-wide road covers cells whose x - z
        // is at most 3 from the centreline's, so one step each way is still on it.
        for along in [46, 50, 54] {
            let level = editor
                .highest_block_between(along, along, -12, 8)
                .expect("centerline is not empty");
            assert!(level < 0, "the ramp has descended by ({along}, {along})");
            for k in -1..=1 {
                let (x, z) = (along + k, along - k);
                assert_eq!(
                    editor.highest_block_between(x, z, -12, 8),
                    Some(level),
                    "carriageway is not level across its width at ({x}, {z})"
                );
            }
        }
    }

    #[test]
    fn tunnel_excluded_from_road_mask() {
        let elems = vec![ProcessedElement::Way(straight_tunnel(&[
            ("highway", "residential"),
            ("tunnel", "yes"),
        ]))];
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let editor = tunnel_editor(&xzbbox, crate::ground::Ground::new_flat(0));
        let mask = collect_road_surface_coords(&elems, &editor, &xzbbox, 1.0, Default::default());
        assert!(!mask.contains(50, 50), "tunnel is not a surface road");
    }

    #[test]
    fn pier_paths_leave_the_water_under_them_alone() {
        let xzbbox = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let editor = test_editor(&xzbbox);
        let elems = vec![
            line_way(
                1,
                &[(10, 20), (50, 20)],
                &[("highway", "footway"), ("man_made", "pier")],
            ),
            line_way(2, &[(10, 40), (50, 40)], &[("highway", "footway")]),
        ];
        let mask = collect_road_surface_coords(&elems, &editor, &xzbbox, 1.0, Default::default());
        assert!(!mask.contains(30, 20), "a pier is a deck, not a road");
        assert!(mask.contains(30, 40));
    }

    #[test]
    fn a_street_widens_up_to_its_mapped_sidewalk_not_over_it() {
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let editor = test_editor(&xzbbox);
        let street = [("highway", "residential"), ("sidewalk:both", "separate")];
        let elems = vec![
            line_way(1, &[(10, 50), (110, 50)], &street),
            // Its north sidewalk, mapped as its own way three blocks off its middle.
            line_way(
                2,
                &[(10, 53), (110, 53)],
                &[("highway", "footway"), ("footway", "sidewalk")],
            ),
            // A crossing over it runs across the road.
            line_way(
                3,
                &[(30, 40), (30, 60)],
                &[("highway", "footway"), ("footway", "crossing")],
            ),
            // A path joining it from the south, and a footbridge and a first-floor walkway
            // over it.
            line_way(4, &[(80, 30), (80, 50)], &[("highway", "footway")]),
            line_way(
                5,
                &[(90, 40), (90, 60)],
                &[("highway", "footway"), ("bridge", "yes"), ("layer", "1")],
            ),
            line_way(
                6,
                &[(100, 40), (100, 60)],
                &[("highway", "footway"), ("level", "1")],
            ),
        ];
        let coords = collect_road_and_carriageway_coords(
            &elems,
            &editor,
            &xzbbox,
            1.0,
            Default::default(),
            &CoordinateBitmap::new_empty(),
        );
        let (roads, lanes, paths) = (&coords.roads, &coords.carriageways, &coords.paths);
        // Two lanes, seven blocks, where nothing is mapped beside it; the second lane stops
        // at the sidewalk, which stays one, and cars there keep off the cells it left.
        assert!(lanes.contains(60, 47) && lanes.contains(60, 52));
        assert!(!lanes.contains(60, 53) && paths.contains(60, 53) && roads.contains(60, 53));
        assert!(!coords.cut.contains(60, 53), "the sidewalk paves it");
        // Nothing that only crosses, joins or bridges it cuts its strip.
        for x in [30, 80, 90, 100] {
            assert!(!paths.contains(x, 47) && lanes.contains(x, 47), "{x}");
        }
    }

    #[test]
    fn a_street_beside_a_house_leaves_its_cut_strip_to_the_cars() {
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let editor = test_editor(&xzbbox);
        let elems = vec![line_way(
            1,
            &[(10, 50), (110, 50)],
            &[("highway", "residential"), ("sidewalk", "both")],
        )];
        // A house front on the street's north edge, between x = 40 and 60.
        let mut houses = CoordinateBitmap::new(&xzbbox);
        for x in 40..=60 {
            for z in 53..=60 {
                houses.set(x, z);
            }
        }
        let coords = collect_road_and_carriageway_coords(
            &elems,
            &editor,
            &xzbbox,
            1.0,
            Default::default(),
            &houses,
        );
        // The second lane stops at the wall; the cells it left are the cut the cars see,
        // the core in front of it is paved.
        assert!(!coords.roads.contains(50, 53) && coords.cut.contains(50, 53));
        assert!(coords.roads.contains(50, 52) && !coords.cut.contains(50, 52));
        assert!(coords.roads.contains(20, 53) && !coords.cut.contains(20, 53));
        // No road stops at a sidewalk here, so no sidewalk mask is kept.
        assert!(coords.paths.is_empty());
    }

    fn line_way(id: u64, points: &[(i32, i32)], tags: &[(&str, &str)]) -> ProcessedElement {
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes: points
                .iter()
                .enumerate()
                .map(|(i, &(x, z))| ProcessedNode {
                    id: id * 100 + i as u64,
                    tags: HashMap::new(),
                    x,
                    z,
                })
                .collect(),
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    fn building(id: u64, x0: i32, z0: i32, x1: i32, z1: i32, traced: bool) -> ProcessedElement {
        let mut tags = vec![("building", "yes")];
        if traced {
            tags.push(("source", "overture_maps"));
        }
        ProcessedElement::Way(crate::element_processing::building_test_support::rect_way(
            id, x0, z0, x1, z1, &tags,
        ))
    }

    fn kept_ids(elements: &[ProcessedElement]) -> Vec<u64> {
        elements.iter().map(|e| e.id()).collect()
    }

    #[test]
    fn buildings_never_stand_on_a_runway() {
        // East-west runway along z = 100, 24 m wide by default.
        let mut elements = vec![
            line_way(1, &[(0, 100), (1000, 100)], &[("aeroway", "runway")]),
            building(10, 400, 95, 420, 105, false),
            building(11, 500, 90, 530, 108, true),
            // A hangar beside the runway, clear of its pavement.
            building(12, 400, 130, 460, 170, false),
        ];
        let dropped = drop_buildings_on_aircraft_pavement(&mut elements, 1.0);
        assert_eq!(dropped, 2);
        assert_eq!(kept_ids(&elements), vec![1, 12]);
    }

    #[test]
    fn a_runway_drawn_as_an_area_clears_its_whole_surface() {
        let mut elements = vec![
            line_way(
                4,
                &[(0, 0), (600, 0), (600, 60), (0, 60), (0, 0)],
                &[("area:aeroway", "runway")],
            ),
            // Mapped, in the middle of the runway surface, far from any centre line.
            building(40, 300, 40, 320, 55, false),
            building(41, 700, 40, 720, 55, false),
        ];
        assert_eq!(drop_buildings_on_aircraft_pavement(&mut elements, 1.0), 1);
        assert_eq!(kept_ids(&elements), vec![4, 41]);
    }

    #[test]
    fn taxiways_and_aprons_only_lose_imagery_traced_buildings() {
        let mut elements = vec![
            line_way(
                2,
                &[(0, 300), (200, 300)],
                &[("aeroway", "taxiway"), ("width", "20")],
            ),
            line_way(
                3,
                &[(300, 300), (400, 300), (400, 400), (300, 400), (300, 300)],
                &[("aeroway", "apron")],
            ),
            // A mapped hangar the taxiway line runs into stays.
            building(20, 180, 290, 220, 310, false),
            // Parked aircraft traced as roofs go.
            building(21, 90, 296, 110, 304, true),
            building(22, 340, 340, 360, 360, true),
            // A mapped building on the apron stays.
            building(23, 370, 370, 380, 380, false),
            // Traced buildings away from the airport stay.
            building(24, 900, 900, 920, 920, true),
        ];
        let dropped = drop_buildings_on_aircraft_pavement(&mut elements, 1.0);
        assert_eq!(dropped, 2);
        assert_eq!(kept_ids(&elements), vec![2, 3, 20, 23, 24]);
    }

    #[test]
    fn a_taxiway_area_drops_what_is_traced_on_it_not_beside_it() {
        let mut elements = vec![
            line_way(
                5,
                &[(0, 0), (300, 0), (300, 100), (0, 100), (0, 0)],
                &[("aeroway", "taxiway"), ("area", "yes")],
            ),
            // A parked aircraft in the middle, far from the outline.
            building(50, 140, 40, 160, 60, true),
            // A traced roof just outside the edge.
            building(51, 140, 104, 160, 110, true),
        ];
        assert_eq!(drop_buildings_on_aircraft_pavement(&mut elements, 1.0), 1);
        assert_eq!(kept_ids(&elements), vec![5, 51]);
    }

    #[test]
    fn no_airport_leaves_every_building_alone() {
        let mut elements = vec![building(30, 0, 0, 10, 10, true)];
        assert_eq!(drop_buildings_on_aircraft_pavement(&mut elements, 1.0), 0);
        assert_eq!(elements.len(), 1);
    }

    fn sloped_ground(w: usize, h: usize, at: impl Fn(i32, i32) -> f32) -> crate::ground::Ground {
        let heights = (0..h)
            .map(|z| (0..w).map(|x| at(x as i32, z as i32)).collect())
            .collect();
        crate::ground::Ground::new_elevation_test(heights, w, h)
    }

    /// Highways alone, in the given order, with the connectivity a real run builds.
    fn render_roads(editor: &mut WorldEditor, xzbbox: &XZBBox, elems: &[ProcessedElement]) {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let connectivity = build_highway_connectivity_map(elems);
        let endpoints = collect_tunnel_internal_endpoints(elems, xzbbox);
        let portals = test_portals(editor, elems, &endpoints);
        let outlines = crate::element_processing::bridge_styles::BridgeOutlineIndex::build(elems);
        let structures = BridgeStructureMap::build(elems, editor, &connectivity, &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(elems, &structures, 1.0);
        let empty = CoordinateBitmap::new_empty();
        let road_mask = collect_road_surface_coords(elems, editor, xzbbox, 1.0, Default::default());
        let mut cells = Vec::new();
        for elem in elems {
            generate_highways(
                editor,
                elem,
                &args,
                &connectivity,
                &FloodFillCache::new(),
                &road_mask,
                &structures,
                &surface,
                &endpoints,
                &portals,
                &empty,
                &mut cells,
                &RoadMarkingIndex::default(),
                &CoordinateBitmap::new_empty(),
            );
        }
    }

    /// Every road column tops out at its registered surface, so nothing stacks above it.
    fn assert_no_stacked_surface(editor: &WorldEditor, x0: i32, x1: i32, z0: i32, z1: i32) {
        for x in x0..=x1 {
            for z in z0..=z1 {
                if let Some(y) = editor.surface_override(x, z) {
                    assert_eq!(
                        editor.highest_block_between(x, z, y - 3, y + 3),
                        Some(y),
                        "road block above the surface at ({x}, {z})"
                    );
                }
            }
        }
    }

    #[test]
    fn a_layered_way_climbs_by_its_distance_along_the_path() {
        let xzbbox = XZBBox::rect_from_xz_lengths(99.0, 99.0).unwrap();
        let mut editor = tunnel_editor(&xzbbox, sloped_ground(100, 100, |_, _| 0.0));
        // A short first segment: the old ramp measured every segment as long as the current one.
        let way = line_way(
            1,
            &[(10, 50), (14, 50), (80, 50)],
            &[("highway", "residential"), ("layer", "1")],
        );
        render_roads(&mut editor, &xzbbox, &[way]);
        let top: Vec<i32> = (10..=80)
            .map(|x| editor.highest_block_between(x, 50, -5, 20).unwrap())
            .collect();
        assert!(top.iter().any(|&y| y >= 5), "the way is raised: {top:?}");
        for (i, w) in top.windows(2).enumerate() {
            assert!((w[1] - w[0]).abs() <= 1, "step at x={}: {top:?}", 10 + i);
        }
    }

    /// A bridge from x=40 to 80 along z=50, an embankment ramp down to x=10 and a road on.
    fn ramp_scene(extra: &[ProcessedElement]) -> Vec<ProcessedElement> {
        let mut elems = vec![
            line_way(1, &[(0, 50), (10, 50)], &[("highway", "residential")]),
            line_way(
                2,
                &[(10, 50), (40, 50)],
                &[("highway", "residential"), ("embankment", "yes")],
            ),
            line_way(
                3,
                &[(40, 50), (80, 50)],
                &[
                    ("highway", "residential"),
                    ("bridge", "yes"),
                    ("layer", "1"),
                ],
            ),
        ];
        elems.extend_from_slice(extra);
        elems
    }

    #[test]
    fn a_sloped_deck_lays_one_surface_per_column() {
        let xzbbox = XZBBox::rect_from_xz_lengths(99.0, 99.0).unwrap();
        let mut editor = tunnel_editor(&xzbbox, sloped_ground(100, 100, |_, _| 0.0));
        let road = [("highway", "residential")];
        let deck = [
            ("highway", "residential"),
            ("bridge", "yes"),
            ("layer", "1"),
        ];
        let elems = [
            line_way(1, &[(0, 26), (10, 30)], &road),
            line_way(2, &[(10, 30), (90, 66)], &deck),
            line_way(3, &[(90, 66), (99, 70)], &road),
        ];
        render_roads(&mut editor, &xzbbox, &elems);
        let outlines = crate::element_processing::bridge_styles::BridgeOutlineIndex::build(&elems);
        let connectivity = build_highway_connectivity_map(&elems);
        let structures = BridgeStructureMap::build(&elems, &editor, &connectivity, &outlines, 1.0);
        let member = structures.lookup_member(2).expect("a deck");
        let slab = structures.structure_of(member).design.slab();
        let ProcessedElement::Way(way) = &elems[1] else {
            unreachable!()
        };
        let half = highway_block_range("residential", &HashMap::new(), 1.0);
        let mut raised = 0;
        for ((x, z), i) in crate::element_processing::bridges::deck_columns(way, half) {
            let y = member.y_at(i);
            raised += usize::from(y > 1);
            assert_eq!(
                editor.highest_block_between(x, z, -5, 30),
                Some(y),
                "({x}, {z})"
            );
            assert_eq!(
                editor.get_block_absolute(x, y - 1, z),
                Some(slab),
                "({x}, {z})"
            );
        }
        assert!(raised > 0, "the deck climbs");
    }

    #[test]
    fn an_approach_ramp_stands_on_its_own_fill_between_walls() {
        let xzbbox = XZBBox::rect_from_xz_lengths(99.0, 99.0).unwrap();
        let mut editor = tunnel_editor(&xzbbox, sloped_ground(100, 100, |_, _| 0.0));
        render_roads(&mut editor, &xzbbox, &ramp_scene(&[]));
        let ramp: Vec<i32> = (12..=38)
            .map(|x| editor.highest_block_between(x, 50, -5, 20).unwrap())
            .collect();
        assert!(ramp[ramp.len() - 1] >= 5, "the ramp climbs: {ramp:?}");
        assert!(
            ramp.windows(2).all(|w| (0..=1).contains(&(w[1] - w[0]))),
            "{ramp:?}"
        );
        assert_eq!(
            editor.surface_override(12, 50),
            Some(ramp[0]),
            "its foot is a road on the ground"
        );
        let edge = 50 + highway_block_range("residential", &HashMap::new(), 1.0) + 1;
        let y = editor.highest_block_between(30, 50, -5, 20).unwrap();
        assert!(y >= 3, "{ramp:?}");
        assert_eq!(
            editor.get_block_absolute(30, y - 2, edge),
            Some(SMOOTH_STONE),
            "a wall stands along the ramp"
        );
        assert_eq!(
            editor.get_block_absolute(30, 1, 50),
            Some(SMOOTH_STONE),
            "the ramp is solid down to the ground"
        );
    }

    #[test]
    fn a_ramp_leaves_room_for_a_road_under_the_bridge_end() {
        let xzbbox = XZBBox::rect_from_xz_lengths(99.0, 99.0).unwrap();
        let mut editor = tunnel_editor(&xzbbox, sloped_ground(100, 100, |_, _| 0.0));
        let under = line_way(4, &[(41, 10), (41, 90)], &[("highway", "residential")]);
        render_roads(&mut editor, &xzbbox, &ramp_scene(&[under]));
        for x in 39..=42 {
            assert_eq!(
                editor.get_block_absolute(x, 2, 50),
                None,
                "blocked at x={x}"
            );
        }
        assert_eq!(
            editor.get_block_absolute(36, 2, 50),
            Some(SMOOTH_STONE),
            "the ramp is solid clear of the road"
        );
    }

    #[test]
    fn a_short_ramp_is_solid_where_the_road_at_its_foot_reaches() {
        let xzbbox = XZBBox::rect_from_xz_lengths(99.0, 99.0).unwrap();
        let mut editor = tunnel_editor(&xzbbox, sloped_ground(100, 100, |_, _| 0.0));
        let road = [("highway", "residential")];
        let elems = [
            line_way(1, &[(0, 50), (34, 50)], &road),
            line_way(
                2,
                &[(34, 50), (40, 50)],
                &[("highway", "residential"), ("embankment", "yes")],
            ),
            line_way(
                3,
                &[(40, 50), (80, 50)],
                &[
                    ("highway", "residential"),
                    ("bridge", "yes"),
                    ("layer", "1"),
                ],
            ),
        ];
        render_roads(&mut editor, &xzbbox, &elems);
        // Three cells up a steep ramp, still within reach of the road it leaves.
        let y = editor.highest_block_between(37, 50, -5, 20).unwrap();
        assert!(y >= 3, "the ramp is up at x=37: {y}");
        for fill in 1..y {
            assert!(
                editor.get_block_absolute(37, fill, 50).is_some(),
                "air under the ramp at y={fill}"
            );
        }
    }

    #[test]
    fn a_ramp_spans_a_street_along_its_edge() {
        let xzbbox = XZBBox::rect_from_xz_lengths(99.0, 99.0).unwrap();
        let mut editor = tunnel_editor(&xzbbox, sloped_ground(100, 100, |_, _| 0.0));
        let beside = line_way(4, &[(15, 55), (45, 55)], &[("highway", "residential")]);
        render_roads(&mut editor, &xzbbox, &ramp_scene(&[beside]));
        let y = editor.highest_block_between(30, 50, -5, 20).unwrap();
        assert!(y >= 3, "the ramp is raised at x=30");
        for z in [52, 53] {
            assert_eq!(
                editor.get_block_absolute(30, 2, z),
                None,
                "the street is walled in at z={z}"
            );
        }
        assert_eq!(
            editor.get_block_absolute(30, y - 2, 47),
            Some(SMOOTH_STONE),
            "the far side keeps its wall"
        );
    }

    #[test]
    fn a_portal_at_the_foot_of_a_face_keeps_the_ground_in_front() {
        let xzbbox = XZBBox::rect_from_xz_lengths(199.0, 99.0).unwrap();
        let ground = sloped_ground(200, 100, |x, _| if x > 80 { 30.0 } else { 10.0 });
        let mut editor = tunnel_editor(&xzbbox, ground);
        let elems = [
            line_way(1, &[(10, 50), (80, 50)], &[("highway", "residential")]),
            line_way(
                2,
                &[(80, 50), (150, 50)],
                &[("highway", "residential"), ("tunnel", "yes")],
            ),
        ];
        render_roads(&mut editor, &xzbbox, &elems);
        let ys: Vec<i32> = (50..=79)
            .map(|x| editor.highest_block_between(x, 50, -20, 40).unwrap())
            .collect();
        assert!(
            ys.windows(2).all(|w| w[1] <= w[0]),
            "climbs to the mouth: {ys:?}"
        );
        assert!(ys[ys.len() - 1] < 10, "{ys:?}");
        assert!(
            editor.highest_block_between(80, 50, -20, 40).unwrap() < 10,
            "nothing at the mouth stands above the ground in front"
        );
    }

    #[test]
    fn a_tunnel_approach_descends_without_flicker() {
        let xzbbox = XZBBox::rect_from_xz_lengths(199.0, 99.0).unwrap();
        // A short rise of a tenth of a block, which the raw cells round up a whole one.
        let ground = sloped_ground(
            200,
            100,
            |x, _| {
                if (64..72).contains(&x) {
                    20.55
                } else {
                    20.45
                }
            },
        );
        let mut editor = tunnel_editor(&xzbbox, ground);
        let road = [("highway", "residential")];
        let elems = [
            line_way(1, &[(10, 50), (80, 50)], &road),
            line_way(
                2,
                &[(80, 50), (120, 50)],
                &[("highway", "residential"), ("tunnel", "yes")],
            ),
        ];
        render_roads(&mut editor, &xzbbox, &elems);
        let ys: Vec<i32> = (50..=78)
            .map(|x| editor.highest_block_between(x, 50, 0, 30).unwrap())
            .collect();
        assert!(ys.last() < ys.first(), "the approach descends: {ys:?}");
        assert!(ys.windows(2).all(|w| w[1] <= w[0]), "{ys:?}");
    }

    #[test]
    fn level_profile_holds_a_level_hovering_at_a_half_block() {
        let smooth: Vec<f64> = (0..60)
            .map(|i| if i % 3 == 0 { 4.56 } else { 4.44 })
            .collect();
        let ys = level_profile(&smooth, &vec![None; smooth.len()]);
        assert!(ys.windows(2).all(|w| w[0] == w[1]), "{ys:?}");
    }

    #[test]
    fn level_profile_climbs_a_ramp_one_block_at_a_time() {
        let smooth: Vec<f64> = (0..80).map(|i| 0.13 * f64::from(i)).collect();
        let ys = level_profile(&smooth, &vec![None; smooth.len()]);
        for (i, w) in ys.windows(2).enumerate() {
            assert!((0..=1).contains(&(w[1] - w[0])), "step at {i}: {ys:?}");
        }
        for (y, s) in ys.iter().zip(&smooth) {
            assert!((f64::from(*y) - s).abs() <= 1.0, "{y} strays from {s}");
        }
    }

    #[test]
    fn level_profile_levels_a_short_dip_and_keeps_a_long_one() {
        let dip = |len: usize| {
            let smooth: Vec<f64> = (0..100)
                .map(|i| {
                    if (40..40 + len).contains(&i) {
                        9.0
                    } else {
                        10.0
                    }
                })
                .collect();
            level_profile(&smooth, &vec![None; smooth.len()])
        };
        assert!(dip(3).iter().all(|&y| y == 10), "{:?}", dip(3));
        let long = dip(30);
        assert!(
            long[55] == 9 && long[10] == 10 && long[90] == 10,
            "{long:?}"
        );
    }

    #[test]
    fn level_profile_meets_its_pins() {
        let smooth = vec![3.4; 40];
        let mut pins = vec![None; 40];
        pins[0] = Some(5);
        pins[39] = Some(1);
        let ys = level_profile(&smooth, &pins);
        assert_eq!((ys[0], ys[39]), (5, 1));
        assert!(ys[10..30].iter().all(|&y| y == 3), "{ys:?}");
    }

    #[test]
    fn one_world_areas_either_side_of_an_edge_level_a_road_alike() {
        // Hovering at a half block from x = 40 to 260, so each clip alone keeps its own end's level.
        let ground = sloped_ground(300, 100, |x, _| {
            if x < 40 {
                4.0
            } else if x < 260 {
                4.5
            } else {
                5.0
            }
        });
        let xzbbox = XZBBox::rect_from_xz_lengths(299.0, 99.0).unwrap();
        let editor = tunnel_editor(&xzbbox, ground);
        let edge: Vec<_> = [(0, 149), (150, 299)]
            .into_iter()
            .map(|(x0, x1)| {
                let mut connectivity = HighwayConnectivityMap::new();
                connectivity.meet_at_edge_of(XZBBox::rect_from_min_max(x0, 0, x1, 99).unwrap());
                // Each clipped 64 blocks past its own area.
                let (from, to) = ((x0 - 64).max(10), (x1 + 64).min(290));
                let ProcessedElement::Way(road) =
                    line_way(1, &[(from, 50), (to, 50)], &[("highway", "residential")])
                else {
                    unreachable!()
                };
                let ys = road_profile(&editor, &road, 2, &connectivity, |_, _| None);
                [ys[(149 - from) as usize], ys[(150 - from) as usize]]
            })
            .collect();
        assert_eq!(edge[0], edge[1]);
    }

    #[test]
    fn a_road_far_below_y_zero_stays_on_the_ground() {
        for level in [-1120.3f32, 2650.4] {
            let ground = sloped_ground(200, 60, |_, _| level);
            let xzbbox = XZBBox::rect_from_xz_lengths(199.0, 59.0).unwrap();
            let editor = tunnel_editor(&xzbbox, ground);
            let ProcessedElement::Way(road) =
                line_way(1, &[(10, 30), (190, 30)], &[("highway", "residential")])
            else {
                unreachable!()
            };
            let ys = road_profile(&editor, &road, 2, &HighwayConnectivityMap::new(), |_, _| {
                None
            });
            let want = level.round() as i32;
            assert!(ys.iter().all(|&y| y == want), "{level}: {ys:?}");
        }
    }

    #[test]
    fn shared_points_are_where_two_highways_pass() {
        let road = [("highway", "residential")];
        let elems = vec![
            line_way(1, &[(0, 0), (10, 0), (20, 0)], &road),
            line_way(2, &[(10, 0), (10, 10)], &road),
            line_way(3, &[(50, 50), (60, 50), (60, 60), (50, 50)], &road),
            line_way(4, &[(20, 0), (30, 0)], &[("building", "yes")]),
        ];
        let map = build_highway_connectivity_map(&elems);
        assert!(map.is_shared(10, 0));
        assert!(!map.is_shared(0, 0) && !map.is_shared(20, 0) && !map.is_shared(10, 10));
        assert!(!map.is_shared(50, 50), "a loop's closing node is its own");
        assert_eq!(map.ends.get(&(10, 0)), Some(&vec![0]));
    }

    #[test]
    fn a_short_dip_in_the_ground_leaves_the_road_level() {
        let xzbbox = XZBBox::rect_from_xz_lengths(199.0, 99.0).unwrap();
        let ground = sloped_ground(
            200,
            100,
            |x, _| if (70..=71).contains(&x) { -1.0 } else { 0.0 },
        );
        let mut editor = tunnel_editor(&xzbbox, ground);
        let road = line_way(1, &[(10, 50), (150, 50)], &[("highway", "residential")]);
        render_roads(&mut editor, &xzbbox, &[road]);
        for x in 12..=148 {
            assert_eq!(editor.surface_override(x, 50), Some(0), "dip at x={x}");
        }
    }

    #[test]
    fn a_diagonal_road_is_level_across_its_width() {
        let xzbbox = XZBBox::rect_from_xz_lengths(199.0, 199.0).unwrap();
        let ground = sloped_ground(200, 200, |x, _| 0.5 * x as f32);
        let mut editor = tunnel_editor(&xzbbox, ground);
        let road = line_way(1, &[(20, 20), (120, 120)], &[("highway", "residential")]);
        render_roads(&mut editor, &xzbbox, &[road]);
        // Across a (1, 1) way is (1, -1).
        for t in 30..=110 {
            let centre = editor.surface_override(t, t);
            assert!(centre.is_some(), "no road at ({t}, {t})");
            for k in [-1, 1] {
                assert_eq!(
                    editor.surface_override(t + k, t - k),
                    centre,
                    "tilted across the road at ({t}, {t})"
                );
            }
        }
        assert_no_stacked_surface(&editor, 20, 120, 20, 120);
    }

    #[test]
    fn roads_meet_at_a_junction_at_one_level_in_either_order() {
        let ground = || sloped_ground(200, 200, |x, z| 0.3 * x as f32 + 0.1 * z as f32);
        let road = [("highway", "residential")];
        let main = line_way(1, &[(10, 100), (100, 100), (190, 100)], &road);
        let side = line_way(2, &[(100, 100), (100, 190)], &road);
        let xzbbox = XZBBox::rect_from_xz_lengths(199.0, 199.0).unwrap();
        let mut levels = Vec::new();
        for order in [[&main, &side], [&side, &main]] {
            let mut editor = tunnel_editor(&xzbbox, ground());
            let elems: Vec<ProcessedElement> = order.iter().map(|e| (*e).clone()).collect();
            render_roads(&mut editor, &xzbbox, &elems);
            assert_eq!(
                editor.surface_override(100, 100),
                Some(junction_level(&editor, 100, 100))
            );
            assert_no_stacked_surface(&editor, 5, 195, 95, 195);
            levels.push(editor.surface_override(100, 100));
        }
        assert_eq!(levels[0], levels[1]);
    }

    #[test]
    fn a_street_split_into_two_ways_has_no_seam() {
        let xzbbox = XZBBox::rect_from_xz_lengths(199.0, 199.0).unwrap();
        // Rising gently, so independent levelling could step either side of the split.
        let ground = sloped_ground(200, 200, |x, _| 0.45 + 0.012 * x as f32);
        let mut editor = tunnel_editor(&xzbbox, ground);
        let road = [("highway", "residential")];
        render_roads(
            &mut editor,
            &xzbbox,
            &[
                line_way(1, &[(10, 100), (97, 100)], &road),
                line_way(2, &[(97, 100), (190, 100)], &road),
            ],
        );
        let ys: Vec<i32> = (12..=188)
            .map(|x| editor.surface_override(x, 100).unwrap())
            .collect();
        assert!(
            ys.windows(2).all(|w| (0..=1).contains(&(w[1] - w[0]))),
            "{ys:?}"
        );
    }

    #[test]
    fn a_loop_closes_without_a_step() {
        let xzbbox = XZBBox::rect_from_xz_lengths(199.0, 199.0).unwrap();
        let ground = sloped_ground(200, 200, |x, z| 0.08 * x as f32 + 0.05 * z as f32);
        let mut editor = tunnel_editor(&xzbbox, ground);
        let ring = [(60, 60), (140, 60), (140, 140), (60, 140), (60, 60)];
        render_roads(
            &mut editor,
            &xzbbox,
            &[line_way(1, &ring, &[("highway", "residential")])],
        );
        let mut path = Vec::new();
        for pair in ring.windows(2) {
            for (x, _, z) in bresenham_line(pair[0].0, 0, pair[0].1, pair[1].0, 0, pair[1].1) {
                path.push(editor.surface_override(x, z).unwrap());
            }
        }
        assert!(
            path.windows(2).all(|w| (w[1] - w[0]).abs() <= 1),
            "{path:?}"
        );
    }
}
