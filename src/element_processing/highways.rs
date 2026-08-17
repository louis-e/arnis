use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::element_processing::bridge_styles::{
    decorate_bridge_above_deck, place_bridge_support_below_deck, BridgePathSample, BridgeStyle,
};
use crate::element_processing::bridges::{is_bridge_way, BridgeStructureMap, BridgeSurfaceMap};
use crate::element_processing::get_nearest_non_road_block;
use crate::element_processing::surfaces::{
    get_blocks_for_surface, get_blocks_for_surface_way, semirandom_surface,
};
use crate::floodfill::flood_fill_area;
use crate::floodfill_cache::{CoordinateBitmap, FloodFillCache, RoadMaskBitmap};
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::{HashMap, HashSet};

/// Upper bound on `block_range` used by wide-road width flattening. The
/// stamp is `2 * block_range + 1`; with `MAX_BLOCK_RANGE = 8` we can sort
/// up to 17 samples on the stack. Keep this generous — a `debug_assert`
/// below catches it if a caller ever exceeds it.
const MAX_BLOCK_RANGE: usize = 8;

/// Median of the ground levels along the road's width-perpendicular
/// strip at one along-length coordinate. Pure primitive — no along-length
/// smoothing. Callers should use `perpendicular_median_ground_y` unless
/// they specifically need the unsmoothed value.
#[inline]
fn perpendicular_median_raw(
    editor: &WorldEditor,
    set_x: i32,
    set_z: i32,
    centerline_x: i32,
    centerline_z: i32,
    block_range: i32,
    dir_horizontal: bool,
) -> i32 {
    debug_assert!(block_range as usize <= MAX_BLOCK_RANGE);
    let len = 2 * block_range as usize + 1;
    // Stack buffer keeps this allocation-free on a hot path that runs
    // millions of times for a city-scale bbox.
    let mut ys = [0i32; 2 * MAX_BLOCK_RANGE + 1];
    if dir_horizontal {
        for (i, t) in (-block_range..=block_range).enumerate() {
            ys[i] = editor.get_ground_level(set_x, centerline_z + t);
        }
    } else {
        for (i, t) in (-block_range..=block_range).enumerate() {
            ys[i] = editor.get_ground_level(centerline_x + t, set_z);
        }
    }
    ys[..len].sort_unstable();
    ys[len / 2]
}

/// Precompute one perpendicular-median Y per axial position in a
/// centerline's stamp. Hot-loop optimization: inside a single centerline
/// point's `(2b+1) × (2b+1)` stamp, every cell that shares a given axial
/// offset (dx for horizontal travel, dz for vertical travel) produces
/// the same target Y — `perpendicular_median_ground_y` ignores the
/// cross-axis position entirely. Computing it once per axial value and
/// reading from this table in the inner loop cuts `get_ground_level`
/// call count by a factor of `2b+1` on the main road-stamp path.
///
/// The table layout maps axial offset `a ∈ [-block_range, block_range]`
/// to index `(a + block_range) as usize`. `out.len()` must be at least
/// `2 * block_range + 1`.
#[inline]
fn precompute_row_medians(
    editor: &WorldEditor,
    centerline_x: i32,
    centerline_z: i32,
    block_range: i32,
    dir_horizontal: bool,
    out: &mut [i32],
) {
    debug_assert!(block_range as usize <= MAX_BLOCK_RANGE);
    let len = 2 * block_range as usize + 1;
    debug_assert!(out.len() >= len);
    for (i, slot) in out[..len].iter_mut().enumerate() {
        let axial = -block_range + i as i32;
        let (sx, sz) = if dir_horizontal {
            (centerline_x + axial, centerline_z)
        } else {
            (centerline_x, centerline_z + axial)
        };
        *slot = perpendicular_median_ground_y(
            editor,
            sx,
            sz,
            centerline_x,
            centerline_z,
            block_range,
            dir_horizontal,
        );
    }
}

/// Median of the ground levels along the road's width-perpendicular strip
/// **at this specific cell's along-length coordinate**. Does NOT sample
/// anything in the travel direction, so the target Y varies naturally
/// along the length of the road (terrain-following) while staying
/// identical across the width at any given length position — meaning
/// every block in one lateral cross-section sits flat (not pitched
/// sideways down a slope).
///
/// A 3-tap median along the road's length axis is layered on top, purely
/// to kill 1-cell terrain noise that would otherwise leave single-block
/// potholes in the road surface (e.g. `…1 1 0 1 1…` → `…1 1 1 1 1…`).
/// A monotone ramp is unaffected because the 3-tap median of any
/// monotonic triple is the middle value.
///
/// - `set_x, set_z` — the cell whose Y we're computing.
/// - `centerline_x, centerline_z` — the current centerline bresenham point.
///   Only the axis perpendicular to travel is used (e.g. `centerline_z`
///   for a horizontal-dominant segment); the cell's own along-length
///   coordinate drives the other axis, which is what makes the sampling
///   cell-specific instead of centerline-specific.
/// - `dir_horizontal` — true when `|dx_segment| >= |dz_segment|`, telling
///   us travel is x-dominant (so perpendicular sampling runs along z).
#[inline]
fn perpendicular_median_ground_y(
    editor: &WorldEditor,
    set_x: i32,
    set_z: i32,
    centerline_x: i32,
    centerline_z: i32,
    block_range: i32,
    dir_horizontal: bool,
) -> i32 {
    let (prev_x, prev_z, next_x, next_z) = if dir_horizontal {
        (set_x - 1, set_z, set_x + 1, set_z)
    } else {
        (set_x, set_z - 1, set_x, set_z + 1)
    };
    let t_prev = perpendicular_median_raw(
        editor,
        prev_x,
        prev_z,
        centerline_x,
        centerline_z,
        block_range,
        dir_horizontal,
    );
    let t_curr = perpendicular_median_raw(
        editor,
        set_x,
        set_z,
        centerline_x,
        centerline_z,
        block_range,
        dir_horizontal,
    );
    let t_next = perpendicular_median_raw(
        editor,
        next_x,
        next_z,
        centerline_x,
        centerline_z,
        block_range,
        dir_horizontal,
    );
    let mut arr = [t_prev, t_curr, t_next];
    arr.sort_unstable();
    arr[1]
}

/// Default block-mix used for road surfaces when no `surface=*` tag is
/// present. Kept as a constant so the `semirandom_surface` call sites read
/// consistently across the file.
const DEFAULT_ROAD_MIX: &[Block] = &[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA];

/// Blocks that a road write must NOT overwrite. Intentionally narrow:
/// - `GRAY_CONCRETE_POWDER`, `CYAN_TERRACOTTA`: the default asphalt mix,
///   preserved so two asphalt roads overlapping produce a consistent
///   surface instead of re-rolling the hash per pass.
/// - `WHITE_CONCRETE`: preserves lane stripes and zebra crossings from
///   being erased when a later road pass crosses them.
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
    // Bridge module furniture must survive parallel side-deck ways.
    WARPED_STAIRS,
    WARPED_TRAPDOOR,
    WARPED_SLAB,
    STRIPPED_WARPED_STEM,
    STRIPPED_WARPED_HYPHAE,
    SEA_LANTERN,
    ANDESITE_WALL,
    SMOOTH_SANDSTONE_STAIRS,
];

/// True when the way should render as a pedestrian walkway
/// rather than asphalt.
fn is_pedestrian_way(element: &ProcessedElement) -> bool {
    is_pedestrian_way_tags(element.tags())
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

/// Type alias for highway connectivity map
pub type HighwayConnectivityMap = HashMap<(i32, i32), Vec<i32>>;

// 4-connected stair fill from `prev` (exclusive) to `curr` (inclusive).
fn stair_fill_cells(prev: (i32, i32), curr: (i32, i32)) -> Vec<(i32, i32)> {
    let mut cells = Vec::with_capacity(2);
    let (mut x, mut z) = prev;
    while x != curr.0 || z != curr.1 {
        if x != curr.0 {
            x += (curr.0 - x).signum();
            cells.push((x, z));
        }
        if z != curr.1 {
            z += (curr.1 - z).signum();
            cells.push((x, z));
        }
    }
    if cells.is_empty() {
        cells.push(curr);
    }
    cells
}

// Absolute base Y for a node feature; deck Y on a bridge, else terrain + layer_boost.
// `bridge_radius`: 0 = exact (lamps, bus stops, on-road signal head), >0 = nearby (off-road
// signal pole/bars where the anchor sits next to the deck rather than on it).
#[inline]
fn node_feature_base_y(
    editor: &WorldEditor,
    bridge_surface: &BridgeSurfaceMap,
    x: i32,
    z: i32,
    layer_boost: i32,
    bridge_radius: i32,
) -> i32 {
    bridge_surface
        .nearby_deck_y(x, z, bridge_radius)
        .unwrap_or_else(|| editor.get_absolute_y(x, layer_boost, z))
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
) {
    if let ProcessedElement::Way(way) = element {
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
}

const TUNNEL_CEIL_OFFSET: i32 = 5; // roof height above the road
const TUNNEL_COVER_DROP: i32 = 7; // cover the bore aims for
/// Cover at which a roof is still kept; one under the target, so a DEM wiggle cannot punch a skylight.
const TUNNEL_ROOF_COVER: i32 = TUNNEL_CEIL_OFFSET + 1;
const TUNNEL_RAMP_STEP: i32 = 1; // max descent per cell
const TUNNEL_RAMP_RUN: i32 = 3; // portal ramp run per 1 block drop
const TUNNEL_LAYER_DROP: i32 = 7; // extra depth per layer below -1
const TUNNEL_LIGHT_INTERVAL: usize = 8;
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
        } else {
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
            if internal.contains(&xz) || portals.nodes.contains_key(&xz) {
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

    let start_ground = terrain_ys[0];
    let end_ground = terrain_ys[last];
    let start_internal = internal_endpoints.contains(&pts[0]);
    let end_internal = internal_endpoints.contains(&pts[last]);
    let denom = last.max(1) as f32;
    let layer_extra = tunnel_layer_extra(&way.tags);
    let cover_drop = TUNNEL_COVER_DROP.saturating_add(layer_extra);

    // The approach road already descended, so the bore starts at depth.
    let start_drop = if start_internal {
        0
    } else {
        portals.drop_at(pts[0])
    };
    let end_drop = if end_internal {
        0
    } else {
        portals.drop_at(pts[last])
    };

    // As deep as cover needs, ramping down from whatever the portals reached.
    let mut road_y: Vec<i32> = Vec::with_capacity(n);
    #[allow(clippy::needless_range_loop)]
    for i in 0..n {
        let t = i as f32 / denom;
        let grade = (start_ground as f32 + (end_ground - start_ground) as f32 * t).round() as i32;
        // Deepen the target, not the portal, so the ramp still reaches ground.
        let desired = grade.min(cover_ys[i] - cover_drop);
        // The road has to be back at the surface by an open portal.
        let ramp_s = if start_internal {
            i32::MIN
        } else {
            start_ground - start_drop - i as i32 / TUNNEL_RAMP_RUN
        };
        let ramp_e = if end_internal {
            i32::MIN
        } else {
            end_ground - end_drop - (last - i) as i32 / TUNNEL_RAMP_RUN
        };
        road_y.push(desired.max(ramp_s.max(ramp_e)));
    }
    // Clamp to terrain and the bedrock plane, then cap the slope.
    for i in 0..n {
        road_y[i] = road_y[i].min(terrain_ys[i]).max(floor);
    }
    for i in 1..n {
        road_y[i] = road_y[i].min(road_y[i - 1] + TUNNEL_RAMP_STEP);
    }
    for i in (0..last).rev() {
        road_y[i] = road_y[i].min(road_y[i + 1] + TUNNEL_RAMP_STEP);
    }
    // One valley, not several: pull interior humps down. Endpoints are the portals.
    if let Some(m) = (0..n).min_by_key(|&i| road_y[i]) {
        for i in 1..=m {
            road_y[i] = road_y[i].min(road_y[i - 1]);
        }
        for i in (m..last).rev() {
            road_y[i] = road_y[i].min(road_y[i + 1]);
        }
    }
    // The slope passes only dig deeper, so re-assert the floor.
    for y in road_y.iter_mut() {
        *y = (*y).max(floor);
    }

    // Close short roof gaps, else a DEM wiggle opens a skylight.
    let mut covered: Vec<bool> = (0..n)
        .map(|i| cover_ys[i] - road_y[i] >= TUNNEL_ROOF_COVER)
        .collect();
    let close_run = 2 * half_width as usize + 3;
    let mut deepened_any = false;
    let mut i = 0;
    while i < n {
        if covered[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && !covered[i] {
            i += 1;
        }
        // Interior only: an open run touching an end is the portal approach.
        if start == 0 || i == n || i - start >= close_run {
            continue;
        }
        for j in start..i {
            let deepened = (cover_ys[j] - TUNNEL_ROOF_COVER).max(floor);
            if deepened < road_y[j] {
                road_y[j] = deepened;
                deepened_any = true;
            }
        }
    }
    if deepened_any {
        // Deepening alone would step the carriageway, so re-cap and re-decide.
        for i in 1..n {
            road_y[i] = road_y[i].min(road_y[i - 1] + TUNNEL_RAMP_STEP);
        }
        for i in (0..last).rev() {
            road_y[i] = road_y[i].min(road_y[i + 1] + TUNNEL_RAMP_STEP);
        }
        for y in road_y.iter_mut() {
            *y = (*y).max(floor);
        }
        for i in 0..n {
            covered[i] = cover_ys[i] - road_y[i] >= TUNNEL_ROOF_COVER;
        }
    }

    let default_palette: &'static [Block] = match highway_type.as_str() {
        "footway" | "pedestrian" | "service" | "steps" => &[GRAY_CONCRETE],
        "path" => &[DIRT_PATH],
        _ => DEFAULT_ROAD_MIX,
    };
    let palette = get_blocks_for_surface_way(way, default_palette);
    let faces = tunnel_portal_faces(&pts, internal_endpoints);

    for i in 0..n {
        let (bx, bz) = pts[i];
        let ry = road_y[i];
        let ceil_y = ry + TUNNEL_CEIL_OFFSET;

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
                let col_top = if carries_road {
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
fn step_towards(from: (i32, i32), inner: (i32, i32)) -> (i32, i32) {
    ((from.0 - inner.0).signum(), (from.1 - inner.1).signum())
}

/// Where an open end cuts the structure off, else the roof overhangs the approach.
#[derive(Clone, Copy)]
struct PortalFace {
    at: (i32, i32),
    /// Unit step out of the bore.
    out: (i32, i32),
}

#[inline]
fn beyond_portal(faces: &[Option<PortalFace>; 2], cx: i32, cz: i32) -> bool {
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

// Phase 2: carve the interior, then lay the road last so no carve can eat it.
pub fn carve_highway_tunnel_interior(editor: &mut WorldEditor, tunnel_cells: &[HighwayTunnelCell]) {
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
        SNOW_LAYER,
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
        let ceil_y = cell.road_y + TUNNEL_CEIL_OFFSET;
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
        if cell.light {
            editor.set_block_absolute(SEA_LANTERN, cell.x, ceil_y - 1, cell.z, None, None);
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
        for &(bx, bz) in &pts {
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

/// Iron bars with their side connections spelled out. A generated chunk keeps the stored
/// blockstate until something triggers a neighbour update, so a run of bars placed with the
/// default state renders as separate posts instead of a joined railing.
fn connected_iron_bars(north: bool, south: bool, east: bool, west: bool) -> BlockWithProperties {
    let flag = |v: bool| fastnbt::Value::String(if v { "true" } else { "false" }.to_string());
    BlockWithProperties::new(
        IRON_BARS,
        Some(fastnbt::Value::Compound(HashMap::from([
            ("north".to_string(), flag(north)),
            ("south".to_string(), flag(south)),
            ("east".to_string(), flag(east)),
            ("west".to_string(), flag(west)),
            ("waterlogged".to_string(), flag(false)),
        ]))),
    )
}

fn place_street_lamp(editor: &mut WorldEditor, x: i32, z: i32, base: i32) {
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
    let mut connectivity_map: HashMap<(i32, i32), Vec<i32>> = HashMap::new();

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
                if !way.nodes.is_empty() {
                    let start_node = &way.nodes[0];
                    let end_node = &way.nodes[way.nodes.len() - 1];

                    let start_coord = (start_node.x, start_node.z);
                    let end_coord = (end_node.x, end_node.z);

                    connectivity_map
                        .entry(start_coord)
                        .or_default()
                        .push(layer_value);
                    connectivity_map
                        .entry(end_coord)
                        .or_default()
                        .push(layer_value);
                }
            }
        }
    }

    connectivity_map
}

/// Internal function that generates highways with connectivity context for elevation handling
#[allow(clippy::too_many_arguments)]
fn generate_highways_internal(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    highway_connectivity: &HashMap<(i32, i32), Vec<i32>>, // Maps node coordinates to list of layers that connect to this node
    flood_fill_cache: &FloodFillCache,
    road_mask: &RoadMaskBitmap,
    bridge_structures: &BridgeStructureMap,
    bridge_surface: &BridgeSurfaceMap,
    tunnel_portals: &TunnelPortalMap,
    tunnel_footprint: &CoordinateBitmap,
) {
    // Shared `indoor=yes` / layer parsing for the whole function. Indoor
    // highways must never produce elevated geometry (they sit inside
    // buildings), and features like street lamps on an explicit
    // `layer=*` should ride up with the bridge/overpass they belong to.
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

    if let Some(highway_type) = element.tags().get("highway") {
        if highway_type == "street_lamp" {
            if let ProcessedElement::Node(first_node) = element {
                let x: i32 = first_node.x;
                let z: i32 = first_node.z;
                let base = node_feature_base_y(editor, bridge_surface, x, z, layer_boost, 0);
                place_street_lamp(editor, x, z, base);
            }
        } else if highway_type == "crossing" || highway_type == "traffic_signals" {
            // Signal heads for signalised crossings and standalone traffic_signals nodes.
            let signalised = highway_type == "traffic_signals"
                || element.tags().get("crossing").map(String::as_str) == Some("traffic_signals");
            if signalised {
                if let ProcessedElement::Node(node) = element {
                    let x = node.x;
                    let z = node.z;
                    let head_base =
                        node_feature_base_y(editor, bridge_surface, x, z, layer_boost, 0);

                    // Try to build a hanging signal if it's on a road
                    let anchor = road_mask
                        .contains(x, z)
                        .then(|| get_nearest_non_road_block(x, z, 4, road_mask))
                        .flatten();

                    match anchor {
                        Some((ax, az)) => {
                            let pole_base =
                                node_feature_base_y(editor, bridge_surface, ax, az, layer_boost, 4);
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
                            let arm: Vec<(i32, i32)> = bresenham_line(x, 0, z, ax, 0, az)
                                .into_iter()
                                .map(|(lx, _, lz)| (lx, lz))
                                .collect();
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
                let base = node_feature_base_y(editor, bridge_surface, x, z, layer_boost, 0);
                for dy in 1..=3 {
                    editor.set_block_absolute(COBBLESTONE_WALL, x, base + dy, z, None, None);
                }

                editor.set_block_absolute(WHITE_WOOL, x, base + 4, z, None, None);
                let neighbor_base =
                    node_feature_base_y(editor, bridge_surface, x + 1, z, layer_boost, 1);
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
            let surface_block: Block = get_blocks_for_surface_way(way, &[STONE])[0];

            // Fill the area using flood fill cache
            let filled_area = flood_fill_cache.get_or_compute(way, args.timeout.as_ref());

            for &(x, z) in filled_area.iter() {
                editor.set_block(surface_block, x, 0, z, None, None);
            }
        } else {
            let mut previous_node: Option<(i32, i32)> = None;
            // Default surface mix. Overridden below based on highway_type or
            // an explicit surface=* tag via `get_blocks_for_surface`.
            let mut block_types: &[Block] = DEFAULT_ROAD_MIX;
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

            // Surface palette per highway type; width is resolved by
            // highway_block_range below so renderer and prescan stay in sync.
            match highway_type.as_str() {
                "footway" | "pedestrian" | "service" | "steps" => {
                    block_types = &[GRAY_CONCRETE];
                }
                "path" => block_types = &[DIRT_PATH],
                "escape" => block_types = &[SAND], // sand trap for runaway vehicles
                _ => {}
            }

            let ProcessedElement::Way(way) = element else {
                return;
            };

            let bridge_member = bridge_structures.lookup_member(way.id);
            let bridge_ramp = bridge_structures.lookup_ramp(way.id);
            // Redundant side deck under a wider module bridge: render nothing.
            if bridge_member.is_some_and(|m| m.covered_by_wider) {
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
            let bridge_foundation_block = bridge_style.foundation_block();
            let bridge_rail_block_choice = bridge_style.rail_block();

            // Optional surface override via the OSM `surface=*` tag. Applies to
            // all road types; for single-block surfaces like concrete or sand
            // the mix degenerates to that one block, so `semirandom_surface`
            // always returns the same value.
            if let Some(blocks) = element
                .tags()
                .get("surface")
                .and_then(|s| get_blocks_for_surface(s))
            {
                block_types = blocks;
            }

            // Pedestrian walkways tagged with a paved surface render as
            // smooth stone, overriding the `surface=*` palette. Real-world
            // sidewalks in concrete or paving stones read as uniformly grey
            // from a distance, not as an asphalt speckle, so this gives
            // them a distinct look from the roads they run alongside.
            if is_pedestrian_way(element)
                && matches!(
                    element.tags().get("surface").map(|s| s.as_str()),
                    Some("concrete" | "paving_stones" | "sett")
                )
            {
                block_types = &[SMOOTH_STONE];
            }

            // Canonical width (shared with prescan/bridge consumers).
            let block_range = highway_block_range(highway_type, &way.tags, scale_factor);

            // At-grade lit ways get periodic street lamps alongside.
            if way.tags.get("lit").map(String::as_str) == Some("yes")
                && !is_bridge_member
                && !is_bridge_ramp
                && !is_indoor
                && layer_value_effective == 0
                && highway_type != "steps"
            {
                place_way_lamps(editor, way, block_range, road_mask);
            }

            // Lane-marking count; lane_markings=no drops the dividers, not the width.
            const MAX_LANES: i32 = 16;
            let mut lanes = way
                .tags
                .get("lanes")
                .and_then(|l| l.parse::<i32>().ok())
                .unwrap_or_else(|| highway_default_lanes(highway_type))
                .clamp(1, MAX_LANES);
            if way.tags.get("lane_markings").map(|s| s.as_str()) == Some("no") {
                lanes = 1;
            }

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
            let bridge_internal_ramp_length: usize = {
                let raw = (total_bresenham_length as f32 * 0.35).clamp(15.0, 50.0) as usize;
                let cap = (total_bresenham_length / 2).max(1);
                raw.clamp(1, cap)
            };

            // Descend into the portal; a pure function of way geometry, so tile-stable.
            let tunnel_approach = if tunnel_portals.is_empty() || is_bridge_member || is_bridge_ramp
            {
                None
            } else {
                tunnel_portals.approach(way.id)
            };

            // Raw DEM up front: the perpendicular median reads back this road's own
            // overrides, so folding a descent into it would compound cell after cell.
            let tunnel_approach_terrain: Vec<i32> = if tunnel_approach.is_none() {
                Vec::new()
            } else {
                let mut ys = vec![0i32; total_bresenham_length];
                let mut cum = 0usize;
                for pair in way.nodes.windows(2) {
                    let pts = bresenham_line(pair[0].x, 0, pair[0].z, pair[1].x, 0, pair[1].z);
                    for (i, (px, _, pz)) in pts.iter().enumerate() {
                        if let Some(slot) = ys.get_mut(cum + i) {
                            *slot = editor
                                .terrain_level(*px, *pz)
                                .unwrap_or_else(|| editor.get_ground_level(*px, *pz));
                        }
                    }
                    cum += pts.len().saturating_sub(1);
                }
                ys
            };

            // Plain beam bridges get a swept segment-schematic deck instead.
            let bridge_module = bridge_member
                .and_then(|m| m.module_idx)
                .and_then(crate::element_processing::bridge_modules::module_at);
            let bridge_structure_moduled = bridge_member
                .map(|m| m.structure_has_module)
                .unwrap_or(false);

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

            // Check if this is a marked zebra crossing (only depends on tags, compute once)
            let is_zebra_crossing = highway_type == "footway"
                && element.tags().get("footway").map(|s| s.as_str()) == Some("crossing")
                && !matches!(
                    element.tags().get("crossing").map(|s| s.as_str()),
                    Some("no" | "unmarked")
                )
                && element.tags().get("crossing:markings").map(|s| s.as_str()) != Some("no");

            // Iterate over nodes to create the highway
            let mut segment_index = 0;
            let total_segments = way.nodes.len() - 1;
            // Cumulative bresenham distance across all segments; drives bridge ramp interp.
            let mut cumulative_distance_from_start: usize = 0;
            // Previous bridge cell Y for steep-step gap fill.
            let mut previous_bridge_y: Option<i32> = None;
            // Centerline samples captured for above-deck decoration after the segment loop.
            let mut bridge_path: Vec<BridgePathSample> = Vec::new();
            // Previous rail cell per side; used to orthogonally connect diagonal steps.
            let mut previous_rail_left: Option<(i32, i32)> = None;
            let mut previous_rail_right: Option<(i32, i32)> = None;

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

                    // Travel direction for this segment. The perpendicular
                    // median sampling runs along the *other* axis, so that
                    // lateral cross-sections end up level while the road's
                    // Y still varies along length as the terrain climbs /
                    // descends.
                    let dir_horizontal = (x2 - x1).abs() >= (z2 - z1).abs();

                    // Whether wide-road Y-flattening applies to this
                    // segment. Bridges and 1-cell paths keep their legacy
                    // per-call behaviour; everything else gets the
                    // perpendicular median via
                    // `perpendicular_median_ground_y`.
                    let flatten_width = !is_bridge_member && !is_bridge_ramp && block_range >= 1;
                    // Whether the road cross-section also registers an
                    // effective-ground override is decided per bresenham
                    // point below — `offset` varies inside a segment (slope
                    // ramps at layer transitions), and elevated sections
                    // (offset > 0) must NOT register, otherwise
                    // `ground_generation` fills terrain all the way up to
                    // the deck and bridges become giant embankments.

                    // Variables to manage dashed line pattern
                    let mut stripe_length: i32 = 0;
                    let dash_length: i32 = (5.0 * scale_factor).ceil() as i32;
                    let gap_length: i32 = (5.0 * scale_factor).ceil() as i32;

                    // Segment-constants for multi-lane divider placement.
                    // Computed once here instead of at every bresenham point:
                    // `seg_len` needs a sqrt and all the perpendicular-unit-
                    // vector math is identical across the whole segment.
                    // `None` means there are no inner dividers to draw (either
                    // a single-lane road or a degenerate zero-length segment).
                    let lane_divider_geom = if lanes >= 2 {
                        let dx_seg = (x2 - x1) as f32;
                        let dz_seg = (z2 - z1) as f32;
                        let seg_len = (dx_seg * dx_seg + dz_seg * dz_seg).sqrt();
                        if seg_len > 0.0 {
                            let road_width_blocks = (2 * block_range + 1) as f32;
                            Some((
                                -dz_seg / seg_len,                // perp_x
                                dx_seg / seg_len,                 // perp_z
                                road_width_blocks / lanes as f32, // lane_width
                                road_width_blocks / 2.0,          // half_width
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
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

                    // How far along the way a cell sits. Projected onto the segment's step
                    // vector, not the dominant axis, which tips a 45-degree road sideways.
                    let along_offset = {
                        let run = (x2 - x1).abs().max((z2 - z1).abs()).max(1) as f32;
                        let (ex, ez) = ((x2 - x1) as f32 / run, (z2 - z1) as f32 / run);
                        let inv = 1.0 / (ex * ex + ez * ez).max(f32::EPSILON);
                        move |dx: i32, dz: i32| {
                            ((dx as f32 * ex + dz as f32 * ez) * inv).round() as i32
                        }
                    };

                    // Bridges/ramps drive their Y from cumulative tds, so skip the duplicate
                    // shared endpoint on later segments. Non-bridge slope offsets keep the
                    // legacy calculate_point_elevation indexing, which expects every point.
                    let skip_first = if (is_bridge_member || is_bridge_ramp) && segment_index > 0 {
                        1
                    } else {
                        0
                    };
                    for (point_index, (x, _, z)) in
                        bresenham_points.iter().enumerate().skip(skip_first)
                    {
                        let tds = cumulative_distance_from_start + point_index;
                        let bridge_y_here = bridge_member
                            .map(|info| {
                                info.y_at(tds, total_bresenham_length, bridge_internal_ramp_length)
                            })
                            .or_else(|| {
                                bridge_ramp.map(|info| info.y_at(tds, total_bresenham_length))
                            });

                        let offset = if is_bridge_member || is_bridge_ramp {
                            0
                        } else {
                            calculate_point_elevation(
                                segment_index,
                                point_index,
                                segment_length,
                                total_segments,
                                effective_elevation,
                                effective_start_slope,
                                effective_end_slope,
                                slope_length,
                            )
                        };

                        // Absolute Y of an approach cell; one Y per stamp would slab the ramp.
                        let approach_terrain_last =
                            tunnel_approach_terrain.len().saturating_sub(1) as i32;
                        let approach_at = |dx: i32, dz: i32| -> Option<i32> {
                            let a = tunnel_approach?;
                            let t =
                                (tds as i32 + along_offset(dx, dz)).clamp(0, approach_terrain_last);
                            match tunnel_approach_offset(a, t) {
                                drop if drop < 0 => {
                                    tunnel_approach_terrain.get(t as usize).map(|&ty| ty + drop)
                                }
                                _ => None,
                            }
                        };

                        let register_ground_override = flatten_width && offset == 0;

                        let use_absolute_y = is_bridge_member || is_bridge_ramp || flatten_width;

                        // Precompute per-axial-offset perpendicular medians
                        // once for this centerline. Every cell in the stamp
                        // that shares an axial offset picks up the same
                        // value — without this cache, we'd recompute the
                        // full 3-tap median (which itself touches ~15
                        // ground samples) for every `(dx, dz)` cell, making
                        // wide-road rendering O(width²) per centerline.
                        let mut row_medians = [0i32; 2 * MAX_BLOCK_RANGE + 1];
                        if flatten_width {
                            precompute_row_medians(
                                editor,
                                *x,
                                *z,
                                block_range,
                                dir_horizontal,
                                &mut row_medians,
                            );
                        }

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
                                                    STONE_BRICKS,
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
                                let set_x: i32 = x + dx;
                                let set_z: i32 = z + dz;

                                // Per-cell Y. For wide roads this is the
                                // perpendicular median at the cell's own
                                // along-length coord — so all cells at the
                                // same along-length coord share one Y
                                // (flat cross-section) and register the
                                // same effective-ground override.
                                let approach_y = approach_at(dx, dz);
                                let cell_y = if let Some(by) = bridge_y_here {
                                    by
                                } else if let Some(ay) = approach_y {
                                    ay
                                } else if flatten_width {
                                    let axial = if dir_horizontal { dx } else { dz };
                                    row_medians[(axial + block_range) as usize] + offset
                                } else {
                                    offset
                                };
                                // The tunnel owns columns in its footprint; depth here bares the roof.
                                if register_ground_override
                                    && !(approach_y.is_some()
                                        && tunnel_footprint.contains(set_x, set_z))
                                {
                                    editor.register_road_surface_y(set_x, set_z, cell_y);
                                }

                                // Zebra crossing logic. Background uses the
                                // default asphalt mix (not the footway's own
                                // surface), matching main's pre-rebase
                                // behaviour — a zebra crossing is painted on
                                // the underlying road, so it reads more
                                // naturally against the road mix than the
                                // footway's single grey.
                                if is_zebra_crossing {
                                    let on_stripe = if dir_horizontal {
                                        set_x % 2 < 1
                                    } else {
                                        set_z % 2 < 1
                                    };
                                    if on_stripe {
                                        // White bar. Whitelist the mix we
                                        // place for the non-bar cells so the
                                        // bar only replaces zebra background.
                                        if use_absolute_y {
                                            editor.set_block_absolute(
                                                WHITE_CONCRETE,
                                                set_x,
                                                cell_y,
                                                set_z,
                                                Some(DEFAULT_ROAD_MIX),
                                                None,
                                            );
                                        } else {
                                            editor.set_block(
                                                WHITE_CONCRETE,
                                                set_x,
                                                cell_y,
                                                set_z,
                                                Some(DEFAULT_ROAD_MIX),
                                                None,
                                            );
                                        }
                                    } else {
                                        // Non-bar cell: asphalt mix.
                                        let bg = semirandom_surface(set_x, set_z, DEFAULT_ROAD_MIX);
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
                                    let effective_block =
                                        semirandom_surface(set_x, set_z, block_types);
                                    if use_absolute_y {
                                        editor.set_block_absolute(
                                            effective_block,
                                            set_x,
                                            cell_y,
                                            set_z,
                                            None,
                                            Some(ROAD_PROTECTED_SURFACES),
                                        );
                                    } else {
                                        editor.set_block(
                                            effective_block,
                                            set_x,
                                            cell_y,
                                            set_z,
                                            None,
                                            Some(ROAD_PROTECTED_SURFACES),
                                        );
                                    }
                                }

                                // Add stone brick foundation and support pillars only for
                                // genuinely elevated decks — bridges and explicit overpasses.
                                // (Regular wide roads now flow through `use_absolute_y == true`
                                // too, but they aren't floating decks; they get embankments
                                // from the registered ground-surface override instead.)
                                let is_elevated_deck = (is_bridge_member
                                    && !bridge_structure_moduled)
                                    || is_bridge_ramp
                                    || effective_elevation > 0;
                                if is_elevated_deck && cell_y > 0 {
                                    // Foundation: stone bricks for everything except wooden boardwalks.
                                    let foundation = if is_bridge_member {
                                        bridge_foundation_block
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

                                    if is_bridge_member {
                                        let interval = bridge_style.pillar_interval();
                                        let is_center = dx == 0 && dz == 0;
                                        // Beam keeps the legacy (x+z) rule; other styles use
                                        // path-index so spacing stays consistent on diagonals.
                                        let is_pillar = is_center
                                            && interval > 0
                                            && match bridge_style {
                                                BridgeStyle::Beam => {
                                                    (set_x + set_z).rem_euclid(interval as i32) == 0
                                                }
                                                _ => tds.is_multiple_of(interval),
                                            };
                                        place_bridge_support_below_deck(
                                            editor,
                                            bridge_style,
                                            set_x,
                                            cell_y,
                                            set_z,
                                            centerline_ground_y,
                                            tds,
                                            total_bresenham_length,
                                            use_absolute_y,
                                            is_center,
                                            is_pillar,
                                        );
                                    } else if use_absolute_y {
                                        add_highway_support_pillar_absolute(
                                            editor,
                                            set_x,
                                            cell_y,
                                            set_z,
                                            dx,
                                            dz,
                                            block_range,
                                        );
                                    } else {
                                        add_highway_support_pillar(
                                            editor,
                                            set_x,
                                            cell_y,
                                            set_z,
                                            dx,
                                            dz,
                                            block_range,
                                        );
                                    }
                                }
                            }
                        }

                        // Side railings; stair_fill_cells keeps the rail 4-connected on diagonals.
                        if let (Some(by), Some((perp_x, perp_z))) =
                            (bridge_y_here, bridge_rail_perp)
                        {
                            // L1-projected stamp extent + 1, so the rail never lands on the deck.
                            let rail_dist =
                                block_range as f32 * (perp_x.abs() + perp_z.abs()) + 1.0;
                            for (sign, prev_state) in [
                                (1.0_f32, &mut previous_rail_left),
                                (-1.0_f32, &mut previous_rail_right),
                            ] {
                                let cx = *x as f32 + perp_x * rail_dist * sign;
                                let cz = *z as f32 + perp_z * rail_dist * sign;
                                let rail_cell = (cx.round() as i32, cz.round() as i32);
                                let cells_to_fill: Vec<(i32, i32)> = match *prev_state {
                                    Some(prev) => stair_fill_cells(prev, rail_cell),
                                    None => vec![rail_cell],
                                };
                                // Boardwalks and module decks bring their own railings.
                                let skip_side_railing = is_bridge_member
                                    && (!bridge_style.has_side_railing()
                                        || bridge_structure_moduled);
                                if !skip_side_railing {
                                    for (rx, rz) in cells_to_fill {
                                        if bridge_surface.contains(rx, rz) {
                                            continue;
                                        }
                                        let rail_block = if is_bridge_member {
                                            bridge_rail_block_choice
                                        } else {
                                            LIGHT_GRAY_CONCRETE
                                        };
                                        editor.set_block_absolute(
                                            rail_block,
                                            rx,
                                            by,
                                            rz,
                                            None,
                                            Some(ROAD_PROTECTED_SURFACES),
                                        );
                                        let rail_foundation = if is_bridge_member {
                                            bridge_style.rail_foundation_block()
                                        } else {
                                            STONE_BRICKS
                                        };
                                        if by > 0 {
                                            editor.set_block_absolute(
                                                rail_foundation,
                                                rx,
                                                by - 1,
                                                rz,
                                                None,
                                                None,
                                            );
                                        }
                                        let parapet = if is_bridge_member {
                                            bridge_style.parapet_block()
                                        } else {
                                            Some(BRICK_WALL)
                                        };
                                        if let Some(p) = parapet {
                                            editor.set_block_absolute(
                                                p,
                                                rx,
                                                by + 1,
                                                rz,
                                                None,
                                                None,
                                            );
                                        }
                                    }
                                }
                                *prev_state = Some(rail_cell);
                            }
                        }

                        // Draw inner-lane dividers as dashed white lines.
                        // For `lanes == 2` this reproduces the previous
                        // single-centerline stripe; higher `lanes` values
                        // produce `lanes - 1` evenly-spaced dividers across
                        // the road width. Each divider is offset
                        // perpendicular to the segment travel direction and
                        // rides at the same terrain-aware Y as the adjacent
                        // road cell (reuses `row_medians` so the per-cell
                        // flat cross-section is preserved).
                        if let Some((perp_x, perp_z, lane_width, half_width)) = lane_divider_geom {
                            if stripe_length < dash_length {
                                for l in 1..lanes {
                                    // Signed perpendicular offset of this
                                    // divider from the centerline.
                                    let perp_dist = l as f32 * lane_width - half_width;
                                    let stripe_x = (*x as f32 + perp_x * perp_dist).round() as i32;
                                    let stripe_z = (*z as f32 + perp_z * perp_dist).round() as i32;

                                    // Y follows the perpendicular median
                                    // at this divider's axial position in
                                    // the cross-section (same rule as the
                                    // road cells). Clamp because the
                                    // rounded (stripe_x, stripe_z) could
                                    // land 1 cell outside the stamp on
                                    // diagonals.
                                    let stripe_y = if let Some(by) = bridge_y_here {
                                        by
                                    } else if let Some(ay) =
                                        approach_at(stripe_x - *x, stripe_z - *z)
                                    {
                                        // Follow the carriageway, else stripes hang at grade.
                                        ay
                                    } else if flatten_width {
                                        let axial = if dir_horizontal {
                                            stripe_x - *x
                                        } else {
                                            stripe_z - *z
                                        };
                                        let idx = (axial + block_range).clamp(0, 2 * block_range)
                                            as usize;
                                        row_medians[idx] + offset
                                    } else {
                                        offset
                                    };

                                    // Whitelist on the actual road
                                    // surface so dividers appear on
                                    // non-default `surface=*` roads too
                                    // (hardcoding the default mix caused
                                    // markings to vanish on e.g.
                                    // concrete/asphalt-tagged highways).
                                    if use_absolute_y {
                                        editor.set_block_absolute(
                                            WHITE_CONCRETE,
                                            stripe_x,
                                            stripe_y,
                                            stripe_z,
                                            Some(block_types),
                                            None,
                                        );
                                    } else {
                                        editor.set_block(
                                            WHITE_CONCRETE,
                                            stripe_x,
                                            stripe_y,
                                            stripe_z,
                                            Some(block_types),
                                            None,
                                        );
                                    }
                                }
                            }

                            // Advance dash state once per centerline cell so
                            // the on/off pattern still reads as dashes, not
                            // solid lines (the original bug in early PR
                            // iterations).
                            stripe_length += 1;
                            if stripe_length >= dash_length + gap_length {
                                stripe_length = 0;
                            }
                        }
                    }

                    segment_index += 1;
                    cumulative_distance_from_start += segment_length - 1;
                }
                previous_node = Some((node.x, node.z));
            }

            if is_bridge_member {
                if let Some(module) = bridge_module {
                    crate::element_processing::bridge_modules::sweep_module(
                        editor,
                        &bridge_path,
                        module,
                    );
                } else if !bridge_structure_moduled {
                    decorate_bridge_above_deck(
                        editor,
                        bridge_style,
                        &bridge_path,
                        block_range,
                        bridge_start_is_boundary,
                        bridge_end_is_boundary,
                    );
                }
            }
        }
    }
}

/// Helper function to determine if a slope should be added at a specific node
fn should_add_slope_at_node(
    node: &crate::osm_parser::ProcessedNode,
    current_layer: i32,
    highway_connectivity: &HashMap<(i32, i32), Vec<i32>>,
) -> bool {
    let node_coord = (node.x, node.z);

    // If we don't have connectivity information, always add slopes for non-zero layers
    if highway_connectivity.is_empty() {
        return current_layer != 0;
    }

    // Check if there are other highways at different layers connected to this node
    if let Some(connected_layers) = highway_connectivity.get(&node_coord) {
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

/// Calculate the Y elevation for a specific point along the highway
#[allow(clippy::too_many_arguments)]
fn calculate_point_elevation(
    segment_index: usize,
    point_index: usize,
    segment_length: usize,
    total_segments: usize,
    base_elevation: i32,
    needs_start_slope: bool,
    needs_end_slope: bool,
    slope_length: usize,
) -> i32 {
    // If no slopes needed, return base elevation
    if !needs_start_slope && !needs_end_slope {
        return base_elevation;
    }

    // Calculate total distance from start
    let total_distance_from_start = segment_index * segment_length + point_index;
    let total_way_length = total_segments * segment_length;

    // Ensure we have reasonable values
    if total_way_length == 0 || slope_length == 0 {
        return base_elevation;
    }

    // Start slope calculation - gradual rise from ground level
    if needs_start_slope && total_distance_from_start <= slope_length {
        let slope_progress = total_distance_from_start as f32 / slope_length as f32;
        let elevation_offset = (base_elevation as f32 * slope_progress) as i32;
        return elevation_offset;
    }

    // End slope calculation - gradual descent to ground level
    if needs_end_slope
        && total_distance_from_start >= (total_way_length.saturating_sub(slope_length))
    {
        let distance_from_end = total_way_length - total_distance_from_start;
        let slope_progress = distance_from_end as f32 / slope_length as f32;
        let elevation_offset = (base_elevation as f32 * slope_progress) as i32;
        return elevation_offset;
    }

    // Middle section at full elevation
    base_elevation
}

/// Add support pillars for elevated highways
fn add_highway_support_pillar(
    editor: &mut WorldEditor,
    x: i32,
    highway_y: i32,
    z: i32,
    dx: i32,
    dz: i32,
    _block_range: i32, // Keep for future use
) {
    // Only add pillars at specific intervals and positions
    if dx == 0 && dz == 0 && (x + z) % 8 == 0 {
        // Add pillar from ground to highway level
        for y in 1..highway_y {
            editor.set_block(STONE_BRICKS, x, y, z, None, None);
        }

        // Add pillar base
        for base_dx in -1..=1 {
            for base_dz in -1..=1 {
                editor.set_block(STONE_BRICKS, x + base_dx, 0, z + base_dz, None, None);
            }
        }
    }
}

/// Add support pillars for bridges using absolute Y coordinates
/// Pillars extend from ground level up to the bridge deck
fn add_highway_support_pillar_absolute(
    editor: &mut WorldEditor,
    x: i32,
    bridge_deck_y: i32,
    z: i32,
    dx: i32,
    dz: i32,
    _block_range: i32, // Keep for future use
) {
    // Only add pillars at specific intervals and positions
    if dx == 0 && dz == 0 && (x + z) % 8 == 0 {
        // Get the actual ground level at this position
        let ground_y = editor.get_ground_level(x, z);

        // Add pillar from ground up to bridge deck
        // Only if the bridge is actually above the ground
        if bridge_deck_y > ground_y {
            for y in (ground_y + 1)..bridge_deck_y {
                editor.set_block_absolute(STONE_BRICKS, x, y, z, None, None);
            }

            // Add pillar base at ground level
            for base_dx in -1..=1 {
                for base_dz in -1..=1 {
                    editor.set_block_absolute(
                        STONE_BRICKS,
                        x + base_dx,
                        ground_y,
                        z + base_dz,
                        None,
                        None,
                    );
                }
            }
        }
    }
}

/// Generates a siding using stone brick slabs
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
                        Some(ROAD_PROTECTED_SURFACES),
                        None,
                    ) {
                        editor.set_block_absolute(siding_block, bx, deck_y + 1, bz, None, None);
                    }
                } else if !editor.check_for_block(bx, 0, bz, Some(ROAD_PROTECTED_SURFACES)) {
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

/// True where a runway centerline dash is painted, given distance `s` (blocks) from the way start.
fn runway_centerline_dash_on(s: f32, scale: f64) -> bool {
    let on = (RUNWAY_DASH_ON_M * scale as f32).max(1.0);
    let off = (RUNWAY_DASH_OFF_M * scale as f32).max(1.0);
    (s % (on + off)) < on
}

/// Parses an OSM `width=*` value in metres (tolerates a trailing "m").
// Parse an OSM width=* tag in metres, tolerating a trailing "m"/" m".
fn parse_width_tag_m(tags: &HashMap<String, String>) -> Option<f64> {
    let raw = tags.get("width")?;
    let s = raw.trim().trim_end_matches('m').trim();
    s.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

/// Renders an aeroway as a concrete strip with markings: runways get asphalt-gray with a dashed
/// white centerline + white edge stripes, taxiways a lighter surface with a yellow centerline.
/// No threshold "piano keys" — OSM splits runways into segments, so a per-way renderer can't tell
/// a real end from an internal split.
pub fn generate_aeroway(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    args: &Args,
    building_footprints: &CoordinateBitmap,
) {
    let aeroway = way.tags.get("aeroway").map(String::as_str);
    let is_runway = aeroway == Some("runway");
    let is_taxiway = aeroway == Some("taxiway");

    if aeroway == Some("helipad") {
        generate_helipad_way(editor, way, args, building_footprints);
        return;
    }

    let base_block = if is_runway {
        GRAY_CONCRETE
    } else {
        LIGHT_GRAY_CONCRETE
    };

    // Half-width from the OSM `width=*` tag (metres, clamped to sane sizes); default when absent.
    let half_m = parse_width_tag_m(&way.tags)
        .map(|w| (w * 0.5).clamp(AEROWAY_MIN_HALF_M, AEROWAY_MAX_HALF_M))
        .unwrap_or(AEROWAY_DEFAULT_HALF_M);
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
    let base_over: Option<&[Block]> = is_runway.then_some(&runway_overwrites[..]);
    for cp in &points {
        for dx in -half_width..=half_width {
            for dz in -half_width..=half_width {
                editor.set_block(base_block, cp.x + dx, 0, cp.z + dz, base_over, None);
            }
        }
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
        } else if is_taxiway {
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

/// Canonical road half-width in blocks. Single source of truth shared by the
/// renderer and the prescan/bitmap/bridge consumers, so they never disagree.
pub(crate) fn highway_block_range(
    highway_type: &str,
    tags: &HashMap<String, String>,
    scale: f64,
) -> i32 {
    let (mut block_range, scales_with_lanes): (i32, bool) = match highway_type {
        "footway" | "pedestrian" => (1, false),
        "path" => (1, false),
        "motorway" | "primary" | "trunk" => (5, true),
        "secondary" => (4, true),
        "tertiary" => (2, true),
        "track" => (1, false),
        "service" => (2, true),
        "secondary_link" | "tertiary_link" => (1, true),
        "escape" => (1, false),
        "steps" => (1, false),
        _ => (2, true),
    };

    const MAX_LANES: i32 = 16;
    let lanes = tags
        .get("lanes")
        .and_then(|l| l.parse::<i32>().ok())
        .unwrap_or_else(|| highway_default_lanes(highway_type))
        .clamp(1, MAX_LANES);

    // Explicit width=* wins; else vehicular roads use 3.5 m/lane, never below
    // the default. The -1 accounts for the centre block in 2*block_range+1.
    if let Some(w) = parse_width_tag_m(tags) {
        block_range = (w / 2.0).round() as i32;
    } else if scales_with_lanes {
        let lanes_based = ((lanes as f32 * 3.5 - 1.0) / 2.0).round() as i32;
        block_range = block_range.max(lanes_based);
    }
    block_range = block_range.clamp(1, MAX_BLOCK_RANGE as i32);

    if scale < 1.0 {
        // max(1): scaling must never collapse a road to zero width.
        block_range = (((block_range as f64) * scale).floor() as i32).max(1);
    }

    block_range
}

/// Collect all (x, z) coordinates that are covered by any rendered road or path
/// surface. The returned bitmap has 1 for every block that the highway renderer
/// places as a road/path surface and 0 everywhere else.
///
/// Geometry is computed identically to `generate_highways_internal`:
/// - Bresenham line between each consecutive pair of OSM nodes
/// - Expanded by `block_range` in both axes (same value as the renderer uses)
/// - `area=yes` ways, indoor ways, negative-level ways, and pure node types
///   (street_lamp, crossing, bus_stop) are excluded, matching the renderer's
///   early-return guards.
///
/// This lets `get_nearest_road_block` in `amenities.rs` or other processors do a single O(1) bitmap lookup
/// instead of live `get_ground_level` + `check_for_block_absolute` world scans.
pub fn collect_road_surface_coords(
    elements: &[ProcessedElement],
    editor: &WorldEditor,
    xzbbox: &XZBBox,
    scale: f64,
) -> CoordinateBitmap {
    collect_highway_surface_coords(elements, Some(editor), xzbbox, scale, |_| true)
}

/// Vehicular carriageways only (no footways, cycleways, paths, steps or pedestrian
/// streets). Signage uses this to keep posts on the sidewalk but off the road.
pub fn collect_carriageway_coords(
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
    scale: f64,
) -> CoordinateBitmap {
    collect_highway_surface_coords(elements, None, xzbbox, scale, |highway| {
        !matches!(
            highway,
            "footway"
                | "path"
                | "steps"
                | "pedestrian"
                | "cycleway"
                | "bridleway"
                | "corridor"
                | "track"
                | "elevator"
                | "platform"
        )
    })
}

/// Shared stamping loop for the road-surface bitmaps; `include` filters by highway type.
fn collect_highway_surface_coords(
    elements: &[ProcessedElement],
    editor: Option<&WorldEditor>,
    xzbbox: &XZBBox,
    scale: f64,
    include: impl Fn(&str) -> bool,
) -> CoordinateBitmap {
    let mut bitmap = CoordinateBitmap::new(xzbbox);

    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };

        let Some(highway_type) = way.tags.get("highway") else {
            continue;
        };

        // Exclude non-surface node-only highway types
        match highway_type.as_str() {
            "street_lamp" | "crossing" | "bus_stop" => continue,
            _ => {}
        }
        if !include(highway_type) {
            continue;
        }

        // Exclude area highways (pedestrian plazas etc.) — flood-filled separately
        if way.tags.get("area").is_some_and(|v| v == "yes") {
            continue;
        }

        // Exclude indoor ways (same guard as generate_highways_internal)
        if way.tags.get("indoor").is_some_and(|v| v == "yes") {
            continue;
        }

        // Exclude negative-level ways (indoor mapping)
        if way
            .tags
            .get("level")
            .and_then(|l| l.parse::<i32>().ok())
            .is_some_and(|l| l < 0)
        {
            continue;
        }

        // Tunnels render underground, unless there is no room to bore one.
        if renders_as_highway_tunnel(way) && editor.is_none_or(|e| tunnel_bore_fits(e, way, scale))
        {
            continue;
        }

        // Use the same block_range the renderer uses for this highway type
        let block_range = highway_block_range(highway_type, &way.tags, scale);

        for i in 1..way.nodes.len() {
            let prev = way.nodes[i - 1].xz();
            let cur = way.nodes[i].xz();

            let points = bresenham_line(prev.x, 0, prev.z, cur.x, 0, cur.z);

            for (bx, _, bz) in &points {
                for dx in -block_range..=block_range {
                    for dz in -block_range..=block_range {
                        bitmap.set(bx + dx, bz + dz);
                    }
                }
            }
        }
    }

    bitmap
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
                        bitmap.set(bx + dx, bz + dz);
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

    #[test]
    fn highway_block_range_scales_with_lanes() {
        let tags = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        // Untagged roads keep their per-type defaults...
        assert_eq!(highway_block_range("residential", &tags(&[]), 1.0), 2);
        assert_eq!(highway_block_range("motorway", &tags(&[]), 1.0), 5);
        assert_eq!(highway_block_range("secondary", &tags(&[]), 1.0), 4);
        // ...except tertiary, which widens to its two-lane default.
        assert_eq!(highway_block_range("tertiary", &tags(&[]), 1.0), 3);
        assert_eq!(
            highway_block_range("tertiary", &tags(&[("lanes", "2")]), 1.0),
            3
        );
        // More lanes widen, clamped to the cap.
        assert_eq!(
            highway_block_range("primary", &tags(&[("lanes", "4")]), 1.0),
            7
        );
        assert_eq!(
            highway_block_range("motorway", &tags(&[("lanes", "6")]), 1.0),
            MAX_BLOCK_RANGE as i32
        );
        // Non-vehicular types never scale with lanes.
        assert_eq!(
            highway_block_range("footway", &tags(&[("lanes", "4")]), 1.0),
            1
        );
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
        // Down-scaling never collapses a road to zero width.
        assert_eq!(highway_block_range("footway", &tags(&[]), 0.4), 1);
        assert_eq!(highway_block_range("residential", &tags(&[]), 0.4), 1);
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
        generate_aeroway(&mut editor, &way, &args, &CoordinateBitmap::new_empty());

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
        generate_aeroway(&mut editor, &taxiway, &args, &CoordinateBitmap::new_empty());
        generate_aeroway(
            &mut editor,
            &straight_aeroway("runway"),
            &args,
            &CoordinateBitmap::new_empty(),
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
        let bridges = BridgeStructureMap::build(&[], editor, &outlines);
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
        let structures = BridgeStructureMap::build(&[], &editor, &outlines);
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
        let portals = test_portals(editor, &elems, &endpoints);
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
        let structures = BridgeStructureMap::build(&[], editor, &outlines);
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
            );
        }
        crate::ground_generation::generate_ground_region(
            editor, ground, &args, xzbbox, &empty, &footprint, &surface, 0, 199, 0, 199, false,
        );
        carve_highway_tunnel_interior(editor, &cells);
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

        // Perpendicular to a (1, 1) way is (1, -1).
        for along in [46, 50, 54] {
            let level = editor
                .highest_block_between(along, along, -12, 8)
                .expect("centerline is not empty");
            assert!(level < 0, "the ramp has descended by ({along}, {along})");
            for k in -2..=2 {
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
        let mask = collect_road_surface_coords(&elems, &editor, &xzbbox, 1.0);
        assert!(!mask.contains(50, 50), "tunnel is not a surface road");
    }
}
