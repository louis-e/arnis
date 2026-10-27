use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZBBox;
use crate::element_processing::bridge_styles::{
    decorate_rail_bridge_above_deck, place_arch_below_deck, place_pier_bent, place_pillar,
    resolve_bridge_style_with_outline, BridgeOutlineIndex, BridgePathSample, BridgeStyle,
};
use crate::element_processing::bridges::{
    BridgeStructureMap, BridgeSurfaceMap, RailDeck, ARCH_MIN_CLEARANCE,
};
use crate::element_processing::connected_blocks::{
    cross_cells, four_connected_line, place_connected, stair_steps,
};
use crate::element_processing::highways::TunnelLight;
pub use crate::element_processing::rail_track::RailJoins;
use crate::element_processing::rail_track::{
    rail_profile, rail_shapes, straight_cells, track_cells, TrackEnd, TrackLevel,
};
use crate::floodfill_cache::CoordinateBitmap;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::{HashMap, HashSet};

const CATENARY_MAST_INTERVAL: usize = 16;
const CATENARY_MAST_OFFSET: i32 = 3;
const CATENARY_WIRE_HEIGHT: i32 = 6;

/// Vertical offset in blocks from the terrain surface to the tunnel ceiling.
const RAIL_TUNNEL_DEPTH: i32 = 3;

pub(crate) const RAIL_BRIDGE_FLAT_CLEARANCE: i32 = 4;
pub(crate) const RAIL_BRIDGE_DIP_THRESHOLD: i32 = 4;
const RAIL_BRIDGE_RAMP_MIN: usize = 8;
const RAIL_BRIDGE_RAMP_MAX: usize = 30;
const RAIL_BRIDGE_RAMP_FRACTION: f32 = 0.25;
/// Half width of a rail deck: ballast beside the track, then the curb.
pub(crate) const RAIL_DECK_HALF_WIDTH: i32 = 2;
/// Widest gap between side-by-side rail decks that gets decked over.
pub(crate) const RAIL_DECK_GAP_FILL: i32 = 4;
/// Deck rows above the terrain up to which a rail deck rests on solid masonry.
const RAIL_ABUTMENT_MAX_HEIGHT: i32 = 3;
/// Height step between neighbouring rail decks still joined without a railing.
const RAIL_DECK_MAX_STEP: i32 = 2;

const RAIL_TRACK_TYPES: &[&str] = &[
    "rail",
    "light_rail",
    "subway",
    "tram",
    "narrow_gauge",
    "monorail",
    "funicular",
    "miniature",
    "preserved",
    "disused",
];

/// Half-width of the outer stone shell (total footprint = 2 * WALL_RADIUS + 1 = 5).
const WALL_RADIUS: i32 = 2;

/// Half-width of the interior air space (total air width = 2 * AIR_RADIUS + 1 = 3).
const AIR_RADIUS: i32 = 1;

/// Number of interior Y-levels (rail + 3 air = 4 blocks for minecart clearance).
const INTERIOR_HEIGHT: i32 = 4;

/// Interval in centerline points between ceiling lights.
const LIGHT_INTERVAL: usize = 8;

/// Rail-tunnel centerline point, its floor Y and whether it carries a ceiling lantern.
pub type RailTunnelPoint = (i32, i32, i32, bool);

/// Leaves a rail goes through, where a tree hangs over the track.
const RAIL_CLEARS: &[Block] = &[
    OAK_LEAVES,
    BIRCH_LEAVES,
    SPRUCE_LEAVES,
    DARK_OAK_LEAVES,
    JUNGLE_LEAVES,
    ACACIA_LEAVES,
    CHERRY_LEAVES,
    MANGROVE_LEAVES,
    AZALEA_LEAVES,
    FLOWERING_AZALEA_LEAVES,
];

/// What a rail profile pays for each change of level, against a block of cut or fill.
const RAIL_STEP_COST: f64 = 2.5;
/// How far a rail bed may leave the ground to keep its slopes on straight track.
const RAIL_BED_REACH: i32 = 10;
/// What a rail pays per cell it sits a block into or onto a road, against a block of cut.
const ROAD_SHIFT_COST: f64 = 4.0;
/// What a tunnel pays per block it runs shallower than its depth, against 1 for deeper.
const TUNNEL_SHALLOW_COST: f64 = 0.25;
/// Cells a tunnel ramp runs for each block it drops from a portal.
const TUNNEL_RAMP_RUN: i32 = 4;

/// Deterministic spatial hash for tunnel wall/ceiling block variety.
/// Returns CRACKED_STONE_BRICKS (~15%), MOSSY_STONE_BRICKS (~3%),
/// or STONE_BRICKS (~82%).
fn rail_tunnel_shell_block(x: i32, y: i32, z: i32) -> Block {
    let h = (x as u32)
        .wrapping_mul(73856093)
        .wrapping_add((y as u32).wrapping_mul(19349663))
        .wrapping_add((z as u32).wrapping_mul(83492791));
    let v = h % 100;
    if v < 15 {
        CRACKED_STONE_BRICKS
    } else if v < 18 {
        MOSSY_STONE_BRICKS
    } else {
        STONE_BRICKS
    }
}

#[allow(clippy::too_many_arguments)]
pub fn generate_railways(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    rail_tunnel_points: &mut Vec<RailTunnelPoint>,
    bridge_outlines: &BridgeOutlineIndex,
    bridge_structures: &BridgeStructureMap,
    bridge_surface: &BridgeSurfaceMap,
    road_mask: &CoordinateBitmap,
    building_footprints: &CoordinateBitmap,
    rail_mask: &CoordinateBitmap,
    rail_joins: &RailJoins,
) {
    let Some(level) = track_level(element) else {
        return;
    };
    if level == TrackLevel::Tunnel {
        generate_rail_tunnel_shell(editor, element, rail_tunnel_points, rail_joins);
        return;
    }

    if is_rail_bridge(element) {
        generate_rail_bridge(
            editor,
            element,
            bridge_outlines,
            bridge_structures,
            bridge_surface,
            rail_joins,
        );
    } else {
        generate_at_grade_rail(editor, element, rail_joins, bridge_structures);
        if catenary_wanted(element) {
            let points = build_smoothed_centerline(element);
            if points.len() >= 2 {
                generate_catenary(editor, &points, road_mask, building_footprints, rail_mask);
            }
        }
    }
}

fn renders_as_rail_tunnel(way: &ProcessedWay) -> bool {
    let Some(railway_type) = way.tags.get("railway").map(String::as_str) else {
        return false;
    };
    if way.nodes.len() < 2
        || !RAIL_TRACK_TYPES.contains(&railway_type)
        || way.tags.get("area").map(String::as_str) == Some("yes")
    {
        return false;
    }
    wants_rail_tunnel(way)
}

/// Subways run underground unless mapped on a bridge or with tunnel=no.
fn wants_rail_tunnel(way: &ProcessedWay) -> bool {
    let tunnel = way.tags.get("tunnel").map(String::as_str);
    if tunnel == Some("yes") {
        return true;
    }
    let is_subway = way.tags.get("railway").map(String::as_str) == Some("subway")
        || way.tags.get("subway").map(String::as_str) == Some("yes");
    is_subway && tunnel != Some("no") && !is_rail_bridge(way)
}

fn is_rail_bridge(way: &ProcessedWay) -> bool {
    if way.tags.get("indoor").map(|v| v.as_str()) == Some("yes") {
        return false;
    }
    way.tags
        .get("bridge")
        .map(|v| v.as_str())
        .is_some_and(|v| v != "no")
}

/// Whether a way lays track, and whether in a tunnel or on the surface.
pub(crate) fn track_level(way: &ProcessedWay) -> Option<TrackLevel> {
    let railway = way.tags.get("railway")?.as_str();
    if way.nodes.len() < 2
        || !RAIL_TRACK_TYPES.contains(&railway)
        || way.tags.get("area").map(String::as_str) == Some("yes")
    {
        return None;
    }
    Some(if wants_rail_tunnel(way) {
        TrackLevel::Tunnel
    } else {
        TrackLevel::Surface
    })
}

/// Track laid on the ground.
pub(crate) fn is_at_grade_track(way: &ProcessedWay) -> bool {
    let Some(railway_type) = way.tags.get("railway").map(String::as_str) else {
        return false;
    };
    way.nodes.len() >= 2
        && RAIL_TRACK_TYPES.contains(&railway_type)
        && way.tags.get("area").map(String::as_str) != Some("yes")
        && !is_rail_bridge(way)
        && !wants_rail_tunnel(way)
}

// Mirrors generate_railways' dispatch so the internal-endpoint set only counts rendered bridges.
pub(crate) fn renders_as_rail_bridge(way: &ProcessedWay) -> bool {
    let Some(railway_type) = way.tags.get("railway") else {
        return false;
    };
    if way.nodes.len() < 2 || !is_rail_bridge(way) || wants_rail_tunnel(way) {
        return false;
    }
    if [
        "proposed",
        "abandoned",
        "construction",
        "razed",
        "turntable",
    ]
    .contains(&railway_type.as_str())
    {
        return false;
    }
    true
}

/// Endpoints shared by 2+ rail-bridge ways, where a way runs on without a ramp.
pub(crate) fn rail_bridge_internal_endpoints(ways: &[&ProcessedWay]) -> HashSet<(i32, i32)> {
    let mut counts: HashMap<(i32, i32), u32> = HashMap::new();
    for w in ways {
        let s = &w.nodes[0];
        let e = &w.nodes[w.nodes.len() - 1];
        *counts.entry((s.x, s.z)).or_default() += 1;
        if (e.x, e.z) != (s.x, s.z) {
            *counts.entry((e.x, e.z)).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(k, c)| (c > 1).then_some(k))
        .collect()
}

fn generate_at_grade_rail(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    rail_joins: &RailJoins,
    bridge_structures: &BridgeStructureMap,
) {
    let cells = track_cells(element).0;
    let n = cells.len();
    if n == 0 {
        return;
    }
    let ends = rail_joins.ends(element.id);
    let ground: Vec<i32> = cells
        .iter()
        .map(|&(x, z)| editor.get_ground_level(x, z))
        .collect();
    // What already holds each cell at a height: a road the rail lies on, or another track
    // or a block it has to keep to.
    let held: Vec<Held> = cells
        .iter()
        .zip(&ground)
        .map(|(&(x, z), &g)| {
            let rail_at = |y: i32| {
                editor
                    .get_block_absolute(x, y, z)
                    .is_some_and(is_rail_block)
            };
            match editor.surface_override(x, z) {
                // Another track lies on this cell, on its surface or sunk into a road.
                Some(r) if rail_at(r + 1) => Held::Fixed(r),
                Some(r) if rail_at(r) => Held::Fixed(r - 1),
                Some(r) => Held::Road(r),
                None if editor.get_block_absolute(x, g, z).is_some() => Held::Fixed(g),
                None => Held::Free,
            }
        })
        .collect();
    let mut pins: Vec<Option<i32>> = vec![None; n];
    for (e, end) in ends.iter().enumerate() {
        let level = end
            .into
            .and_then(|(way, cell, _)| deck_y(bridge_structures, way, cell));
        pin_end(&mut pins, e, end, level.unwrap_or(ground[end_cell(n, e)]));
    }
    let straight = straight_cells(
        &cells,
        ends[0].beyond.first().copied(),
        ends[1].beyond.first().copied(),
    );
    // On a road the rail may sit a block into it or on it, to take a step on straight track.
    let beds = rail_profile(
        &straight,
        |i| match (held[i], pins[i]) {
            (Held::Fixed(p), _) | (Held::Free, Some(p)) => (p, p),
            (Held::Road(r), _) => (r - 1, r + 1),
            (Held::Free, None) => (ground[i] - RAIL_BED_REACH, ground[i] + RAIL_BED_REACH),
        },
        |i, y| match held[i] {
            Held::Road(r) => ROAD_SHIFT_COST * (y - r).abs() as f64,
            _ => (y - ground[i]).abs() as f64,
        },
        RAIL_STEP_COST,
    );
    let shapes = rail_shapes(
        &cells,
        &beds,
        beyond(&ends[0], beds[0], bridge_structures),
        beyond(&ends[1], beds[n - 1], bridge_structures),
    );

    for (i, &(x, z)) in cells.iter().enumerate() {
        let bed = beds[i];
        let lays = lays_cell(&ends, n, i);
        match held[i] {
            Held::Free => {
                // The ground pass then cuts or fills the terrain to the bed.
                editor.register_road_surface_y(x, z, bed);
                editor.set_block_absolute(GRAVEL, x, bed, z, None, None);
            }
            // Sunk into the road: the rail takes the place of its surface.
            Held::Road(r) if bed < r => {
                if lays {
                    editor.set_block_absolute(shapes[i], x, r, z, None, Some(&[]));
                }
                editor.register_rail(element.id, x, z, r);
                continue;
            }
            // Raised over it: the road's own surface carries on up under the rail.
            Held::Road(r) if bed > r => {
                let surface = editor.get_block_absolute(x, r, z).unwrap_or(GRAVEL);
                editor.set_block_absolute(surface, x, bed, z, None, None);
            }
            _ => editor.set_block_absolute(GRAVEL, x, bed, z, None, None),
        }
        if lays {
            editor.set_block_absolute(shapes[i], x, bed + 1, z, Some(RAIL_CLEARS), None);
        }
        editor.register_rail(element.id, x, z, bed + 1);
    }
}

/// What holds a track cell at a height before its rail goes in.
#[derive(Clone, Copy)]
enum Held {
    /// Open ground, which the rail bed cuts or fills.
    Free,
    /// A road at this surface height.
    Road(i32),
    /// Another track, or a block at ground level: the rail keeps to it.
    Fixed(i32),
}

fn is_rail_block(block: Block) -> bool {
    [
        RAIL,
        RAIL_NORTH_SOUTH,
        RAIL_EAST_WEST,
        RAIL_ASCENDING_EAST,
        RAIL_ASCENDING_WEST,
        RAIL_ASCENDING_NORTH,
        RAIL_ASCENDING_SOUTH,
        RAIL_NORTH_EAST,
        RAIL_NORTH_WEST,
        RAIL_SOUTH_EAST,
        RAIL_SOUTH_WEST,
    ]
    .iter()
    .any(|r| r.id() == block.id())
}

/// The cell at the start (0) or end (1) of a run of `n`.
fn end_cell(n: usize, e: usize) -> usize {
    if e == 0 {
        0
    } else {
        n - 1
    }
}

/// Holds a shared end and the cell next to it at `level`, so the track it joins, which
/// does the same, shapes the rail on the joint alike.
fn pin_end(pins: &mut [Option<i32>], e: usize, end: &TrackEnd, level: i32) {
    let n = pins.len();
    if !end.shared || n == 0 {
        return;
    }
    let at = end_cell(n, e);
    let next = if e == 0 {
        1.min(n - 1)
    } else {
        n.saturating_sub(2)
    };
    for i in [at, next] {
        pins[i].get_or_insert(level);
    }
}

/// The first cell past an end and its height: a bridge's deck there, or level with the end.
fn beyond(
    end: &TrackEnd,
    own: i32,
    bridge_structures: &BridgeStructureMap,
) -> Option<((i32, i32), i32)> {
    let cell = *end.beyond.first()?;
    let y = end
        .into
        .and_then(|(way, node, dir)| deck_y(bridge_structures, way, (node as isize + dir) as usize))
        .unwrap_or(own);
    Some((cell, y))
}

/// Whether a way lays the rail on cell `i` of `n`: an end cell only if it owns the joint.
fn lays_cell(ends: &[TrackEnd; 2], n: usize, i: usize) -> bool {
    !((i == 0 && !ends[0].owns) || (i + 1 == n && !ends[1].owns))
}

/// The deck a rail bridge's track lies on at one of its cells.
fn deck_y(bridge_structures: &BridgeStructureMap, way: u64, cell: usize) -> Option<i32> {
    match bridge_structures.rail_deck(way)? {
        RailDeck::Own(deck) => deck.ys.get(cell).copied(),
        RailDeck::Carried(ys) => ys.get(cell).or(ys.last()).copied(),
    }
}

// Overhead line only for real catenary: contact_line, or a best-guess yes on a
// main/branch line. Never for third rail.
fn catenary_wanted(way: &ProcessedWay) -> bool {
    if way.tags.get("railway").map(String::as_str) != Some("rail") {
        return false;
    }
    match way.tags.get("electrified").map(String::as_str) {
        Some("contact_line") => true,
        Some("yes") => matches!(
            way.tags.get("usage").map(String::as_str),
            Some("main") | Some("branch")
        ),
        _ => false,
    }
}

pub(crate) fn build_smoothed_centerline(way: &ProcessedWay) -> Vec<(i32, i32)> {
    track_cells(way).0
}

fn travel_dir(points: &[(i32, i32)], i: usize) -> (i32, i32) {
    let (cx, cz) = points[i];
    if i + 1 < points.len() {
        let (nx, nz) = points[i + 1];
        ((nx - cx).signum(), (nz - cz).signum())
    } else if i > 0 {
        let (px, pz) = points[i - 1];
        ((cx - px).signum(), (cz - pz).signum())
    } else {
        (1, 0)
    }
}

fn rail_within(mask: &CoordinateBitmap, cx: i32, cz: i32, dx: i32, dz: i32, max_dist: i32) -> bool {
    (1..=max_dist).any(|d| mask.contains(cx + dx * d, cz + dz * d))
}

// Rejects a side blocked by a road, building, or a parallel track within the mast footprint.
fn mast_base_clear(
    cx: i32,
    cz: i32,
    dx: i32,
    dz: i32,
    road_mask: &CoordinateBitmap,
    building_footprints: &CoordinateBitmap,
    rail_mask: &CoordinateBitmap,
) -> bool {
    let (x, z) = (
        cx + dx * CATENARY_MAST_OFFSET,
        cz + dz * CATENARY_MAST_OFFSET,
    );
    if road_mask.contains(x, z) || building_footprints.contains(x, z) {
        return false;
    }
    !rail_within(rail_mask, cx, cz, dx, dz, CATENARY_MAST_OFFSET + 2)
}

#[allow(clippy::too_many_arguments)]
fn place_catenary_mast(
    editor: &mut WorldEditor,
    cx: i32,
    cz: i32,
    px: i32,
    pz: i32,
    wire_abs: i32,
    road_mask: &CoordinateBitmap,
    building_footprints: &CoordinateBitmap,
    rail_mask: &CoordinateBitmap,
) {
    // one consistent side, flipped if that side is blocked
    let side = if mast_base_clear(cx, cz, px, pz, road_mask, building_footprints, rail_mask) {
        (px, pz)
    } else if mast_base_clear(cx, cz, -px, -pz, road_mask, building_footprints, rail_mask) {
        (-px, -pz)
    } else {
        return;
    };
    let (sx, sz) = side;
    let (mx, mz) = (
        cx + sx * CATENARY_MAST_OFFSET,
        cz + sz * CATENARY_MAST_OFFSET,
    );

    let mast_ground = editor.get_ground_level(mx, mz);
    editor.set_block_absolute(GRAY_CONCRETE, mx, mast_ground, mz, None, None);
    for y in (mast_ground + 1)..=wire_abs {
        editor.set_block_absolute(COBBLESTONE_WALL, mx, y, mz, None, None);
    }

    let arm = if sx != 0 { CHAIN_X } else { CHAIN_Z };
    for d in 1..CATENARY_MAST_OFFSET {
        editor.set_block_absolute(arm, cx + sx * d, wire_abs, cz + sz * d, None, None);
    }
}

// Contact wire over the centreline plus cantilever masts at intervals.
fn generate_catenary(
    editor: &mut WorldEditor,
    points: &[(i32, i32)],
    road_mask: &CoordinateBitmap,
    building_footprints: &CoordinateBitmap,
    rail_mask: &CoordinateBitmap,
) {
    let n = points.len();
    if n < 2 {
        return;
    }

    for i in 0..n {
        let (cx, cz) = points[i];
        let (dx, dz) = travel_dir(points, i);
        let (px, pz) = (-dz, dx);
        let wire_abs = editor.get_ground_level(cx, cz) + CATENARY_WIRE_HEIGHT;

        let wire = if dx != 0 { CHAIN_X } else { CHAIN_Z };
        editor.set_block_absolute(wire, cx, wire_abs, cz, None, None);

        if i.is_multiple_of(CATENARY_MAST_INTERVAL) {
            place_catenary_mast(
                editor,
                cx,
                cz,
                px,
                pz,
                wire_abs,
                road_mask,
                building_footprints,
                rail_mask,
            );
        }
    }
}

// At-grade rail centerlines, used only to keep catenary masts off other tracks.
pub fn collect_at_grade_rail_mask(
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
) -> CoordinateBitmap {
    // No catenary anywhere means nothing reads the mask, so skip the allocation.
    if !elements
        .iter()
        .any(|e| matches!(e, ProcessedElement::Way(w) if catenary_wanted(w)))
    {
        return CoordinateBitmap::new_empty();
    }
    let mut bitmap = CoordinateBitmap::new(xzbbox);
    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };
        if way.nodes.len() < 2 {
            continue;
        }
        if way.tags.get("railway").map(String::as_str) != Some("rail") {
            continue;
        }
        if way.tags.get("subway").map(|v| v == "yes").unwrap_or(false) {
            continue;
        }
        if way.tags.get("tunnel").map(|v| v == "yes").unwrap_or(false) {
            continue;
        }
        if is_rail_bridge(way) {
            continue;
        }
        for (bx, bz) in build_smoothed_centerline(way) {
            bitmap.set(bx, bz);
        }
    }
    bitmap
}

/// Adds every rendered rail-tunnel shell coordinate to the shared tunnel footprint.
pub fn add_tunnel_footprint(
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
    footprint: &mut CoordinateBitmap,
) {
    if !elements
        .iter()
        .any(|e| matches!(e, ProcessedElement::Way(w) if renders_as_rail_tunnel(w)))
    {
        return;
    }
    if footprint.is_empty() {
        *footprint = CoordinateBitmap::new(xzbbox);
    }
    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };
        if !renders_as_rail_tunnel(way) {
            continue;
        }
        for (bx, bz) in build_smoothed_centerline(way) {
            for dx in -WALL_RADIUS..=WALL_RADIUS {
                for dz in -WALL_RADIUS..=WALL_RADIUS {
                    footprint.set(bx + dx, bz + dz);
                }
            }
        }
    }
}

/// Deck Y per centerline cell, ramping from loose ends up to `level`.
pub(crate) fn rail_bridge_profile(
    terrain: &[i32],
    level: i32,
    start_internal: bool,
    end_internal: bool,
) -> Vec<i32> {
    let total = terrain.len();
    if total == 0 {
        return Vec::new();
    }
    let last_idx = total - 1;
    let start_ground = terrain[0];
    let end_ground = terrain[last_idx];

    // Ramp length sized so per-cell linear delta stays <= 1 (rail step limit). No horizontal cap:
    // when needed > total/2 the start/end ramps overlap and min(start, end) below produces a pyramid.
    let mut needed = 0usize;
    if !start_internal {
        needed = needed.max((level - start_ground).max(0) as usize);
    }
    if !end_internal {
        needed = needed.max((level - end_ground).max(0) as usize);
    }
    let needed = needed + 1;

    let raw_ramp = (total as f32 * RAIL_BRIDGE_RAMP_FRACTION) as usize;
    let ramp_length = raw_ramp
        .clamp(RAIL_BRIDGE_RAMP_MIN, RAIL_BRIDGE_RAMP_MAX)
        .max(needed);
    let denom = ramp_length.saturating_sub(1).max(1) as f32;

    (0..total)
        .map(|tds| {
            // Two rising ramps from each endpoint, capped at the level. min combines them: trapezoid
            // for long bridges (both ramps reach the level mid-span), pyramid for short ones.
            let start_ramp_y = if start_internal {
                level
            } else {
                let t = (tds as f32 / denom).min(1.0);
                let span = (level - start_ground) as f32;
                (start_ground as f32 + span * t).round() as i32
            };
            let end_ramp_y = if end_internal {
                level
            } else {
                let dist_from_end = last_idx.saturating_sub(tds);
                let t = (dist_from_end as f32 / denom).min(1.0);
                let span = (level - end_ground) as f32;
                (end_ground as f32 + span * t).round() as i32
            };
            // Clamp to local terrain so a mid-ramp hillside doesn't bury the deck.
            start_ramp_y.min(end_ramp_y).max(terrain[tds])
        })
        .collect()
}

/// Moves a deck's ramp steps onto straight track, where the rails on them stay joined:
/// level for a cell at a loose end, as the track it meets is, within a few blocks of the
/// ramp elsewhere and never into the ground under it.
pub(crate) fn fit_rail_deck(
    cells: &[(i32, i32)],
    terrain: &[i32],
    ys: Vec<i32>,
    internal: [bool; 2],
) -> Vec<i32> {
    let n = ys.len().min(cells.len()).min(terrain.len());
    if n < 3 {
        return ys;
    }
    let mut pins = vec![None; n];
    for (e, &inner) in internal.iter().enumerate() {
        let at = end_cell(n, e);
        pins[at] = Some(ys[at]);
        if !inner {
            let next = if e == 0 { 1 } else { n - 2 };
            pins[next].get_or_insert(ys[at]);
        }
    }
    let straight = straight_cells(&cells[..n], None, None);
    let mut fitted = rail_profile(
        &straight,
        |i| pins[i].map_or((terrain[i].max(ys[i] - 3), ys[i] + 3), |p| (p, p)),
        |i, y| 3.0 * (y - ys[i]).abs() as f64,
        0.5,
    );
    fitted.extend_from_slice(&ys[n..]);
    fitted
}

/// What lies beside one side of a rail deck.
#[derive(Clone, Copy, PartialEq, Debug)]
enum RailDeckSide {
    /// Nothing: a curb and a railing.
    Open,
    /// Another track's deck close in height, decked over up to this far from the track.
    Joined(f32),
    /// A road deck alongside: no railing.
    Closed,
}

impl RailDeckSide {
    fn reach(self) -> f32 {
        match self {
            RailDeckSide::Joined(reach) => reach.max(RAIL_BALLAST_REACH),
            _ => RAIL_BALLAST_REACH,
        }
    }
}

/// Ballast reaches this far either side of the track; the curb comes after it.
const RAIL_BALLAST_REACH: f32 = RAIL_DECK_HALF_WIDTH as f32 - 0.5;

fn probe_rail_deck_side(
    surface: &BridgeSurfaceMap,
    way_id: u64,
    (x, z): (i32, i32),
    y: i32,
    out: (f32, f32),
) -> RailDeckSide {
    let half = RAIL_DECK_HALF_WIDTH;
    for (cx, cz) in cross_cells(x, z, out, 1, 2 * half + 1 + RAIL_DECK_GAP_FILL) {
        let Some((id, other_y)) = surface.rail_track_at(cx, cz) else {
            continue;
        };
        if id == way_id {
            continue;
        }
        let dist = (cx - x) as f32 * out.0 + (cz - z) as f32 * out.1;
        if other_y == y {
            // Up to and including its curb.
            return RailDeckSide::Joined(dist - half as f32 + 0.5);
        }
        // Ramps out of step: each deck takes its half of the gap.
        if (other_y - y).abs() <= RAIL_DECK_MAX_STEP {
            return RailDeckSide::Joined(dist / 2.0 + 0.25);
        }
        break;
    }
    let road_alongside = cross_cells(x, z, out, half, half + 1)
        .into_iter()
        .any(|(cx, cz)| surface.deck_near(cx, cz, y, 1));
    if road_alongside {
        RailDeckSide::Closed
    } else {
        RailDeckSide::Open
    }
}

/// Cells either side used to steady the direction across stair-stepped centerlines.
const RAIL_HEADING_REACH: usize = 4;

/// Unit perpendicular from the centerline a few cells either side.
fn centerline_perp(points: &[(i32, i32)], i: usize) -> (f32, f32) {
    let prev = points[i.saturating_sub(RAIL_HEADING_REACH)];
    let next = points[(i + RAIL_HEADING_REACH).min(points.len() - 1)];
    let dx = (next.0 - prev.0) as f32;
    let dz = (next.1 - prev.1) as f32;
    let mag = (dx * dx + dz * dz).sqrt().max(1e-6);
    (-dz / mag, dx / mag)
}

/// Deck cells beside the track, each with the centerline cell whose Y it takes.
fn rail_deck_cells(
    points: &[(i32, i32)],
    perps: &[(f32, f32)],
    sides: &[[RailDeckSide; 2]],
) -> Vec<((i32, i32), usize)> {
    // Nearest centerline cell, so a column on a ramp gets one deck height.
    let mut nearest: HashMap<(i32, i32), (i32, usize)> = HashMap::new();
    for (i, &(x, z)) in points.iter().enumerate() {
        let r = sides[i][0].reach().max(sides[i][1].reach()).ceil() as i32;
        for dx in -r..=r {
            for dz in -r..=r {
                let d2 = dx * dx + dz * dz;
                nearest
                    .entry((x + dx, z + dz))
                    .and_modify(|e| {
                        if d2 < e.0 {
                            *e = (d2, i);
                        }
                    })
                    .or_insert((d2, i));
            }
        }
    }
    let mut cells: Vec<((i32, i32), usize)> = nearest
        .into_iter()
        .filter_map(|(cell, (d2, i))| {
            if d2 == 0 {
                return None;
            }
            let (perp, (x, z)) = (perps[i], points[i]);
            let (dx, dz) = ((cell.0 - x) as f32, (cell.1 - z) as f32);
            let across = dx * perp.0 + dz * perp.1;
            let along = dx * perp.1 - dz * perp.0;
            // Nothing past the bridge ends.
            if (i == 0 || i + 1 == points.len()) && along.abs() >= 0.75 {
                return None;
            }
            let side = sides[i][usize::from(across < 0.0)];
            (across.abs() <= side.reach()).then_some((cell, i))
        })
        .collect();
    cells.sort_unstable();
    cells
}

/// Where a rail deck's columns go, shared by the renderer and the prescan footprint.
struct RailDeckLayout {
    perps: Vec<(f32, f32)>,
    sides: Vec<[RailDeckSide; 2]>,
    /// Curb cells per centerline cell and open side, stair-stepped from the previous ones.
    curbs: Vec<[Vec<(i32, i32)>; 2]>,
    /// Ballast and gap cells with the centerline cell whose Y they take.
    ballast: Vec<((i32, i32), usize)>,
}

fn rail_deck_layout(
    surface: &BridgeSurfaceMap,
    way_id: u64,
    points: &[(i32, i32)],
    ys: &[i32],
) -> RailDeckLayout {
    let perps: Vec<(f32, f32)> = (0..points.len())
        .map(|i| centerline_perp(points, i))
        .collect();
    let sides: Vec<[RailDeckSide; 2]> = (0..points.len())
        .map(|i| {
            let (px, pz) = perps[i];
            [(px, pz), (-px, -pz)]
                .map(|out| probe_rail_deck_side(surface, way_id, points[i], ys[i], out))
        })
        .collect();
    let mut prev: [Option<(i32, i32)>; 2] = [None, None];
    let curbs = (0..points.len())
        .map(|i| {
            let (bx, bz) = points[i];
            let (px, pz) = perps[i];
            let edge_dist = (RAIL_DECK_HALF_WIDTH - 1) as f32 * (px.abs() + pz.abs()) + 1.0;
            [(0, 1.0f32), (1, -1.0)].map(|(s, sign)| {
                if sides[i][s] != RailDeckSide::Open {
                    prev[s] = None;
                    return Vec::new();
                }
                let cell = (
                    (bx as f32 + px * sign * edge_dist).round() as i32,
                    (bz as f32 + pz * sign * edge_dist).round() as i32,
                );
                let run = match prev[s] {
                    Some(p) if p != cell => stair_steps(p, cell),
                    _ => vec![cell],
                };
                prev[s] = Some(cell);
                run
            })
        })
        .collect();
    let ballast = rail_deck_cells(points, &perps, &sides);
    RailDeckLayout {
        perps,
        sides,
        curbs,
        ballast,
    }
}

/// Every column of a rail bridge's own deck with its deck Y, as the renderer lays it.
pub(crate) fn rail_deck_footprint(
    surface: &BridgeSurfaceMap,
    way: &ProcessedWay,
    ys: &[i32],
) -> Vec<((i32, i32), i32)> {
    let mut points = build_smoothed_centerline(way);
    points.truncate(ys.len());
    let layout = rail_deck_layout(surface, way.id, &points, ys);
    let curbs = layout
        .curbs
        .iter()
        .enumerate()
        .flat_map(|(i, sides)| sides.iter().flatten().map(move |&c| (c, ys[i])));
    points
        .iter()
        .zip(ys.iter().copied())
        .map(|(&c, y)| (c, y))
        .chain(curbs)
        .chain(layout.ballast.iter().map(|&(c, i)| (c, ys[i])))
        .collect()
}

/// Deck slab and surface at `y`; low enough over the terrain, solid masonry underneath.
fn lay_rail_deck_column(
    editor: &mut WorldEditor,
    surface: &BridgeSurfaceMap,
    (x, z): (i32, i32),
    y: i32,
    slab: Block,
    top: Block,
) {
    editor.set_block_absolute(top, x, y, z, None, None);
    editor.set_block_absolute(slab, x, y - 1, z, None, None);
    let ground = editor.get_ground_level(x, z);
    if y - ground <= RAIL_ABUTMENT_MAX_HEIGHT && !surface.support_blocked(x, z, y) {
        for fy in (ground + 1)..(y - 1) {
            editor.set_block_absolute(slab, x, fy, z, None, None);
        }
    }
}

/// Curb block and railing for a rail deck's open edges.
fn rail_deck_edge(style: BridgeStyle) -> (Block, Option<Block>) {
    match style {
        BridgeStyle::Boardwalk => (OAK_PLANKS, Some(OAK_FENCE)),
        // The truss or the walls stand on the edge.
        BridgeStyle::Truss | BridgeStyle::Covered => (STONE_BRICKS, None),
        BridgeStyle::Arch => (STONE_BRICKS, Some(STONE_BRICK_WALL)),
        _ => (STONE_BRICKS, Some(IRON_BARS)),
    }
}

fn generate_rail_bridge(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    bridge_outlines: &BridgeOutlineIndex,
    bridge_structures: &BridgeStructureMap,
    bridge_surface: &BridgeSurfaceMap,
    rail_joins: &RailJoins,
) {
    let all_points = build_smoothed_centerline(way);
    if all_points.is_empty() {
        return;
    }
    let ends = rail_joins.ends(way.id);

    let deck = match bridge_structures.rail_deck(way.id) {
        // Track on a road deck; the road bridge brings the supports.
        Some(RailDeck::Carried(ys)) => {
            let n = all_points.len();
            let ys: Vec<i32> = (0..n)
                .map(|i| ys.get(i).or(ys.last()).copied().unwrap_or_default())
                .collect();
            let shapes = rail_shapes(
                &all_points,
                &ys,
                beyond(&ends[0], ys[0], bridge_structures),
                beyond(&ends[1], ys[n - 1], bridge_structures),
            );
            for (i, &(bx, bz)) in all_points.iter().enumerate() {
                let y = ys[i];
                editor.set_block_absolute(GRAVEL, bx, y, bz, None, None);
                if lays_cell(&ends, n, i) {
                    editor.set_block_absolute(shapes[i], bx, y + 1, bz, Some(RAIL_CLEARS), None);
                }
                editor.register_rail(way.id, bx, bz, y + 1);
            }
            return;
        }
        Some(RailDeck::Own(deck)) => deck,
        None => return,
    };

    let style = resolve_bridge_style_with_outline(way, bridge_outlines);
    let total = all_points.len().min(deck.ys.len());
    let points = &all_points[..total];
    let ys = &deck.ys[..total];
    let layout = rail_deck_layout(bridge_surface, way.id, points, ys);
    let (perps, sides) = (&layout.perps, &layout.sides);
    let slab = style.foundation_block();
    // Every deck column with its centerline index, for the arch pass.
    let mut deck_cells: Vec<((i32, i32), usize)> = Vec::new();

    // The track goes first: later writes only fill empty cells.
    let whole = total == all_points.len();
    let after = if whole {
        beyond(&ends[1], ys[total - 1], bridge_structures)
    } else {
        Some((all_points[total], ys[total - 1]))
    };
    let shapes = rail_shapes(
        points,
        ys,
        beyond(&ends[0], ys[0], bridge_structures),
        after,
    );
    for (i, &(bx, bz)) in points.iter().enumerate() {
        let y = ys[i];
        if (i > 0 && (i + 1 < total || !whole)) || lays_cell(&ends, total, i) {
            editor.set_block_absolute(shapes[i], bx, y + 1, bz, Some(RAIL_CLEARS), None);
        }
        editor.register_rail(way.id, bx, bz, y + 1);
        let bed = if i % 4 == 0 { OAK_LOG } else { GRAVEL };
        lay_rail_deck_column(editor, bridge_surface, (bx, bz), y, slab, bed);
        deck_cells.push(((bx, bz), i));
    }

    if catenary_wanted(way) {
        place_bridge_catenary(editor, points, ys, perps, sides);
    }

    let open_share = |s: usize| {
        sides
            .iter()
            .filter(|side| side[s] == RailDeckSide::Open)
            .count()
    };
    let open = [0, 1].map(|s| open_share(s) * 2 >= total);
    let path: Vec<BridgePathSample> = (0..total)
        .map(|i| (points[i].0, ys[i], points[i].1, perps[i]))
        .collect();
    decorate_rail_bridge_above_deck(
        editor,
        style,
        &path,
        RAIL_DECK_HALF_WIDTH - 1,
        open,
        deck.start_is_boundary,
        deck.end_is_boundary,
    );

    // Curbs and railings on the open sides, stepped so the railing stays joined.
    let (curb, railing) = rail_deck_edge(style);
    // Another track whose bed or rail a deck column at `y` would cut through.
    let other_track = |x: i32, z: i32, y: i32| {
        bridge_surface
            .rail_track_at(x, z)
            .is_some_and(|(id, other_y)| id != way.id && (y - 3..=y + 2).contains(&other_y))
    };
    let mut prev_tops: [Option<((i32, i32), i32)>; 2] = [None, None];
    for (i, (&y, curbs)) in ys.iter().zip(&layout.curbs).enumerate() {
        for (s, run) in curbs.iter().enumerate() {
            if run.is_empty() {
                prev_tops[s] = None;
            }
            for &(cx, cz) in run {
                if other_track(cx, cz, y) {
                    continue;
                }
                let ground = editor.get_ground_level(cx, cz);
                lay_rail_deck_column(editor, bridge_surface, (cx, cz), y, slab, curb);
                deck_cells.push(((cx, cz), i));
                // Edge girder, so the deck reads thicker than the slab.
                if y - 2 > ground && style != BridgeStyle::Boardwalk {
                    editor.set_block_absolute(slab, cx, y - 2, cz, None, None);
                }
                let Some(railing) = railing else {
                    continue;
                };
                // No railing where the ramp is nearly back on the ground.
                if y - ground < 2 {
                    prev_tops[s] = None;
                    continue;
                }
                let top = y + 1;
                place_connected(editor, railing, cx, top, cz);
                // Step the railing where the deck changes height.
                if let Some(((qx, qz), prev_top)) = prev_tops[s] {
                    let adjacent = (qx - cx).abs() + (qz - cz).abs() == 1;
                    if adjacent && prev_top == top - 1 {
                        place_connected(editor, railing, qx, top, qz);
                    } else if adjacent && prev_top == top + 1 {
                        place_connected(editor, railing, cx, prev_top, cz);
                    }
                }
                prev_tops[s] = Some(((cx, cz), top));
            }
        }
    }

    // Ballast, and the gap over to a joined deck.
    let ballast = if style == BridgeStyle::Boardwalk {
        OAK_PLANKS
    } else {
        GRAVEL
    };
    for &((cx, cz), i) in &layout.ballast {
        if other_track(cx, cz, ys[i]) {
            continue;
        }
        lay_rail_deck_column(editor, bridge_surface, (cx, cz), ys[i], slab, ballast);
        deck_cells.push(((cx, cz), i));
    }

    // Arches need room to read; a deck that never gets it stands on piers.
    let arch_room = points
        .iter()
        .zip(ys)
        .any(|(&(x, z), &y)| y - editor.get_ground_level(x, z) >= ARCH_MIN_CLEARANCE);
    let support = if style == BridgeStyle::Arch && !arch_room {
        BridgeStyle::Beam
    } else {
        style
    };
    if support == BridgeStyle::Arch {
        for &((cx, cz), i) in &deck_cells {
            if !bridge_surface.support_blocked(cx, cz, ys[i]) {
                let (bx, bz) = points[i];
                let ground = editor.get_ground_level(bx, bz);
                let centre = (cx, cz) == (bx, bz);
                place_arch_below_deck(editor, cx, ys[i], cz, ground, i, total, true, centre);
            }
        }
    }

    // Piers on a world grid, so the piers of parallel tracks line up.
    let cables_carry_deck =
        support.cables_carry_deck(total, deck.start_is_boundary, deck.end_is_boundary);
    let interval = if cables_carry_deck {
        0
    } else {
        support.pier_interval(RAIL_DECK_HALF_WIDTH) as i32
    };
    for i in (0..total).filter(|_| interval > 0) {
        let (bx, bz) = points[i];
        let y = ys[i];
        let perp = perps[i];
        let along_x = perp.1.abs() >= perp.0.abs();
        let key = |(x, z): (i32, i32)| if along_x { x } else { z };
        if key(points[i]).rem_euclid(interval) != 0
            || (i > 0 && key(points[i - 1]) == key(points[i]))
        {
            continue;
        }
        if style == BridgeStyle::Boardwalk {
            place_pier_bent(
                editor,
                bridge_surface,
                style,
                bx,
                bz,
                y,
                perp,
                RAIL_DECK_HALF_WIDTH,
            );
            continue;
        }
        for (cx, cz) in cross_cells(bx, bz, perp, -1, 1) {
            if !bridge_surface.support_blocked(cx, cz, y) {
                place_pillar(editor, cx, y - 1, cz, STONE_BRICKS, true);
            }
        }
    }
}

/// Contact wire over a bridge track, with masts standing on an open edge.
fn place_bridge_catenary(
    editor: &mut WorldEditor,
    points: &[(i32, i32)],
    ys: &[i32],
    perps: &[(f32, f32)],
    sides: &[[RailDeckSide; 2]],
) {
    for (i, &(cx, cz)) in points.iter().enumerate() {
        let (dx, _) = travel_dir(points, i);
        let wire_y = ys[i] + CATENARY_WIRE_HEIGHT;
        let wire = if dx != 0 { CHAIN_X } else { CHAIN_Z };
        editor.set_block_absolute(wire, cx, wire_y, cz, None, None);

        if !i.is_multiple_of(CATENARY_MAST_INTERVAL) {
            continue;
        }
        let Some(s) = (0..2).find(|&s| sides[i][s] == RailDeckSide::Open) else {
            continue;
        };
        let sign = if s == 0 { 1.0 } else { -1.0 };
        let out = (perps[i].0 * sign, perps[i].1 * sign);
        let edge_dist = (RAIL_DECK_HALF_WIDTH - 1) as f32 * (out.0.abs() + out.1.abs()) + 1.0;
        let mast = (
            (cx as f32 + out.0 * edge_dist).round() as i32,
            (cz as f32 + out.1 * edge_dist).round() as i32,
        );
        for y in (ys[i] + 1)..=wire_y {
            place_connected(editor, COBBLESTONE_WALL, mast.0, y, mast.1);
        }
        let arm = if out.0.abs() >= out.1.abs() {
            CHAIN_X
        } else {
            CHAIN_Z
        };
        let line: Vec<(i32, i32)> = bresenham_line(cx, 0, cz, mast.0, 0, mast.1)
            .into_iter()
            .map(|(x, _, z)| (x, z))
            .collect();
        let line = four_connected_line(&line);
        for &(ax, az) in line.iter().filter(|&&c| c != (cx, cz) && c != mast) {
            editor.set_block_absolute(arm, ax, wire_y, az, None, None);
        }
    }
}

/// Choose between a flat or ascending rail block based on the ground-level
/// difference between the previous, current, and next track points.
pub(crate) fn determine_rail_with_slope(
    current: (i32, i32),
    prev: Option<(i32, i32)>,
    next: Option<(i32, i32)>,
    prev_ground: i32,
    current_ground: i32,
    next_ground: i32,
) -> Block {
    // Ascending toward the *higher* neighbour.
    if next_ground > current_ground {
        if let Some((nx, nz)) = next {
            return ascending_toward(current, (nx, nz));
        }
    }
    if prev_ground > current_ground {
        if let Some((px, pz)) = prev {
            return ascending_toward(current, (px, pz));
        }
    }
    // Flat section – fall back to standard direction logic.
    determine_rail_direction(current, prev, next)
}

/// Return the ascending rail variant that climbs from `from` toward `to`.
fn ascending_toward(from: (i32, i32), to: (i32, i32)) -> Block {
    let (fx, fz) = from;
    let (tx, tz) = to;
    let dx = tx - fx;
    let dz = tz - fz;
    if dx.abs() >= dz.abs() {
        if dx > 0 {
            RAIL_ASCENDING_EAST
        } else {
            RAIL_ASCENDING_WEST
        }
    } else if dz < 0 {
        RAIL_ASCENDING_NORTH
    } else {
        RAIL_ASCENDING_SOUTH
    }
}

pub(crate) fn smooth_diagonal_rails(points: &[(i32, i32, i32)]) -> Vec<(i32, i32, i32)> {
    let mut smoothed = Vec::new();

    for i in 0..points.len() {
        let current = points[i];
        smoothed.push(current);

        if i + 1 >= points.len() {
            continue;
        }

        let next = points[i + 1];
        let (x1, y1, z1) = current;
        let (x2, _, z2) = next;

        // If points are diagonally adjacent
        if (x2 - x1).abs() == 1 && (z2 - z1).abs() == 1 {
            // Look ahead to determine best intermediate point
            let look_ahead = if i + 2 < points.len() {
                Some(points[i + 2])
            } else {
                None
            };

            // Look behind
            let look_behind = if i > 0 { Some(points[i - 1]) } else { None };

            // Choose intermediate point based on the overall curve direction
            let intermediate = if let Some((prev_x, _, _prev_z)) = look_behind {
                if prev_x == x1 {
                    // Coming from vertical, keep x constant
                    (x1, y1, z2)
                } else {
                    // Coming from horizontal, keep z constant
                    (x2, y1, z1)
                }
            } else if let Some((next_x, _, _next_z)) = look_ahead {
                if next_x == x2 {
                    // Going to vertical, keep x constant
                    (x2, y1, z1)
                } else {
                    // Going to horizontal, keep z constant
                    (x1, y1, z2)
                }
            } else {
                // Default to horizontal first if no context
                (x2, y1, z1)
            };

            smoothed.push(intermediate);
        }
    }

    smoothed
}

fn determine_rail_direction(
    current: (i32, i32),
    prev: Option<(i32, i32)>,
    next: Option<(i32, i32)>,
) -> Block {
    let (x, z) = current;

    match (prev, next) {
        (Some((px, pz)), Some((nx, nz))) => {
            if px == nx {
                RAIL_NORTH_SOUTH
            } else if pz == nz {
                RAIL_EAST_WEST
            } else {
                // Calculate relative movements
                let from_prev = (px - x, pz - z);
                let to_next = (nx - x, nz - z);

                match (from_prev, to_next) {
                    // East to North or North to East
                    ((-1, 0), (0, -1)) | ((0, -1), (-1, 0)) => RAIL_NORTH_WEST,
                    // West to North or North to West
                    ((1, 0), (0, -1)) | ((0, -1), (1, 0)) => RAIL_NORTH_EAST,
                    // East to South or South to East
                    ((-1, 0), (0, 1)) | ((0, 1), (-1, 0)) => RAIL_SOUTH_WEST,
                    // West to South or South to West
                    ((1, 0), (0, 1)) | ((0, 1), (1, 0)) => RAIL_SOUTH_EAST,
                    _ => {
                        if (px - x).abs() > (pz - z).abs() {
                            RAIL_EAST_WEST
                        } else {
                            RAIL_NORTH_SOUTH
                        }
                    }
                }
            }
        }
        (Some((px, pz)), None) | (None, Some((px, pz))) => {
            if px == x {
                RAIL_NORTH_SOUTH
            } else if pz == z {
                RAIL_EAST_WEST
            } else {
                RAIL_NORTH_SOUTH
            }
        }
        (None, None) => RAIL_NORTH_SOUTH,
    }
}

pub fn generate_roller_coaster(editor: &mut WorldEditor, element: &ProcessedWay) {
    if let Some(roller_coaster) = element.tags.get("roller_coaster") {
        if roller_coaster == "track" {
            // Check if it's indoor (skip if yes)
            if let Some(indoor) = element.tags.get("indoor") {
                if indoor == "yes" {
                    return;
                }
            }

            // Check if layer is negative (skip if yes)
            if let Some(layer) = element.tags.get("layer") {
                if let Ok(layer_value) = layer.parse::<i32>() {
                    if layer_value < 0 {
                        return;
                    }
                }
            }

            let elevation_height = 4; // 4 blocks in the air
            let pillar_interval = 6; // Support pillars every 6 blocks

            for i in 1..element.nodes.len() {
                let prev_node = element.nodes[i - 1].xz();
                let cur_node = element.nodes[i].xz();

                let points = bresenham_line(prev_node.x, 0, prev_node.z, cur_node.x, 0, cur_node.z);
                let smoothed_points = smooth_diagonal_rails(&points);

                for j in 0..smoothed_points.len() {
                    let (bx, _, bz) = smoothed_points[j];

                    // Place track foundation at elevation height
                    editor.set_block(IRON_BLOCK, bx, elevation_height, bz, None, None);

                    let prev = if j > 0 {
                        Some(smoothed_points[j - 1])
                    } else {
                        None
                    };
                    let next = if j < smoothed_points.len() - 1 {
                        Some(smoothed_points[j + 1])
                    } else {
                        None
                    };

                    let rail_block = determine_rail_direction(
                        (bx, bz),
                        prev.map(|(x, _, z)| (x, z)),
                        next.map(|(x, _, z)| (x, z)),
                    );

                    // Place rail on top of the foundation
                    editor.set_block(rail_block, bx, elevation_height + 1, bz, None, None);

                    // Place support pillars every pillar_interval blocks
                    if bx % pillar_interval == 0 && bz % pillar_interval == 0 {
                        // Create a pillar from ground level up to the track
                        for y in 1..elevation_height {
                            editor.set_block(IRON_BLOCK, bx, y, bz, None, None);
                        }
                    }
                }
            }
        }
    }
}

/// Phase 1 of underground railway generation: place the structural tunnel shell and rail
/// track.  Called during element processing (step 4) so that all non-AIR
/// blocks survive the underground stone fill in step 6.
///
/// Centerline points are collected into `rail_tunnel_points` for phase 2.
fn generate_rail_tunnel_shell(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    rail_tunnel_points: &mut Vec<RailTunnelPoint>,
    rail_joins: &RailJoins,
) {
    let cells = track_cells(element).0;
    let n = cells.len();
    if n == 0 {
        return;
    }
    let ends = rail_joins.ends(element.id);
    let depth = RAIL_TUNNEL_DEPTH + INTERIOR_HEIGHT + 1;
    let ground: Vec<i32> = cells
        .iter()
        .map(|&(x, z)| editor.get_ground_level(x, z))
        .collect();
    let mut deep: Vec<i32> = ground.iter().map(|g| g - depth).collect();
    // Never under the world's floor: low ground keeps its tunnel just below the surface.
    let lowest = crate::world_editor::min_y() + 1;
    let mut pins = vec![None; n];
    for (e, end) in ends.iter().enumerate() {
        // At a portal the floor comes up to the track outside; underground it keeps its depth.
        let at = end_cell(n, e);
        let level = if end.meets_surface {
            ground[at]
        } else {
            deep[at]
        };
        pin_end(&mut pins, e, end, level.max(lowest));
    }
    // From a portal it aims down a ramp, not straight for its depth.
    for (e, end) in ends.iter().enumerate() {
        let at = end_cell(n, e);
        if end.meets_surface {
            for (k, d) in deep.iter_mut().enumerate() {
                let along = k.abs_diff(at) as i32;
                *d = (*d).max(ground[at] - along / TUNNEL_RAMP_RUN);
            }
        }
    }
    let straight = straight_cells(
        &cells,
        ends[0].beyond.first().copied(),
        ends[1].beyond.first().copied(),
    );
    let floors = rail_profile(
        &straight,
        |i| {
            pins[i].map_or(
                (
                    (deep[i] - RAIL_BED_REACH).max(lowest),
                    ground[i].max(lowest),
                ),
                |p| (p, p),
            )
        },
        |i, y| {
            let shallower = (y - deep[i]) as f64;
            if shallower > 0.0 {
                TUNNEL_SHALLOW_COST * shallower
            } else {
                -shallower
            }
        },
        RAIL_STEP_COST,
    );
    let shapes = rail_shapes(
        &cells,
        &floors,
        ends[0].beyond.first().map(|&c| (c, floors[0])),
        ends[1].beyond.first().map(|&c| (c, floors[n - 1])),
    );

    for (i, &(bx, bz)) in cells.iter().enumerate() {
        let floor_y = floors[i];
        let ceil_y = floor_y + INTERIOR_HEIGHT + 1;
        // Lanterns are counted along the way, not the shared list, so they don't move with
        // how many tiles recorded the tunnel or in which order they merged.
        rail_tunnel_points.push((bx, bz, floor_y, i.is_multiple_of(LIGHT_INTERVAL)));

        // Safety: skip if the tunnel would go below world minimum.
        if floor_y <= crate::world_editor::min_y() {
            continue;
        }

        // Place tunnel shell (5x5 footprint, full height).
        // Interior positions deliberately get non-AIR blocks too so that
        // the ground fill (skip_existing: true) leaves them alone.
        // Wall/ceiling blocks get random cracked/mossy variants for variety.
        for dx in -WALL_RADIUS..=WALL_RADIUS {
            for dz in -WALL_RADIUS..=WALL_RADIUS {
                let top = editor.get_ground_level(bx + dx, bz + dz);
                for y in floor_y..=ceil_y {
                    let is_wall_or_ceiling =
                        dx.abs() == WALL_RADIUS || dz.abs() == WALL_RADIUS || y == ceil_y;
                    // Up to the ground it is a cutting: the ground is its roof, or the sky.
                    if is_wall_or_ceiling && y > floor_y && y >= top {
                        continue;
                    }

                    let block = if y == floor_y {
                        // Entire floor row: polished deepslate
                        POLISHED_DEEPSLATE
                    } else if is_wall_or_ceiling {
                        // Visible wall/ceiling: mix in cracked and mossy
                        rail_tunnel_shell_block(bx + dx, y, bz + dz)
                    } else {
                        // Interior placeholder (carved in phase 2)
                        STONE_BRICKS
                    };
                    editor.set_block_absolute(block, bx + dx, y, bz + dz, None, None);
                }
            }
        }
    }

    // The track goes in once every shell is up: on a slope a cell's floor or filler lies
    // where the next cell's rail goes, and the carve leaves floors and rails alone.
    let over: &[Block] = &[
        STONE_BRICKS,
        CRACKED_STONE_BRICKS,
        MOSSY_STONE_BRICKS,
        POLISHED_DEEPSLATE,
    ];
    for (i, &(bx, bz)) in cells.iter().enumerate() {
        let floor_y = floors[i];
        if floor_y <= crate::world_editor::min_y() {
            continue;
        }
        editor.set_block_absolute(POLISHED_DEEPSLATE, bx, floor_y, bz, Some(over), None);
        if lays_cell(&ends, n, i) {
            editor.set_block_absolute(shapes[i], bx, floor_y + 1, bz, Some(over), None);
        }
        editor.register_rail(element.id, bx, bz, floor_y + 1);
    }
}

/// Phase 2 of underground railway generation: carve the 3x3 air interior and place
/// ceiling lights.  Called AFTER ground generation so that the carved
/// air blocks are not overwritten by the underground stone fill.
///
/// Every tile a tunnel crosses records the whole centerline, so the merged list holds each
/// point once per tile. A point is carved only the first time: carving just turns
/// whitelisted blocks to air, and a lantern is never on the whitelist, so a second carve
/// changes nothing. Lanterns come flagged per way, so neither order nor repeats matter.
pub fn carve_rail_tunnel_interior(
    editor: &mut WorldEditor,
    rail_tunnel_points: &[RailTunnelPoint],
) -> Vec<TunnelLight> {
    // Whitelist: allow overwriting shell blocks and ground-fill STONE
    // so the tunnel is actually hollow.
    let carve_whitelist: &[Block] = &[
        STONE_BRICKS,
        CRACKED_STONE_BRICKS,
        MOSSY_STONE_BRICKS,
        STONE,
    ];
    let mut carved: HashSet<(i32, i32)> = HashSet::with_capacity(rail_tunnel_points.len());
    let mut lights = Vec::new();
    for &(bx, bz, floor_y, lantern) in rail_tunnel_points {
        let first = carved.insert((bx, bz));
        if !first && !lantern {
            continue;
        }
        let ceil_y = floor_y + INTERIOR_HEIGHT + 1;

        if floor_y <= crate::world_editor::min_y() {
            continue;
        }

        if first {
            for dx in -AIR_RADIUS..=AIR_RADIUS {
                for dz in -AIR_RADIUS..=AIR_RADIUS {
                    for y in (floor_y + 1)..ceil_y {
                        // Skip the center rail block.
                        if dx == 0 && dz == 0 && y == floor_y + 1 {
                            continue;
                        }
                        editor.set_block_absolute(
                            AIR,
                            bx + dx,
                            y,
                            bz + dz,
                            Some(carve_whitelist),
                            None,
                        );
                    }
                }
            }
        }

        // Periodic ceiling lighting, hung once every bore is carved: a neighbour under
        // higher ground carves a higher roof, and a road tunnel may cross this one.
        if lantern {
            lights.push(TunnelLight {
                x: bx,
                z: bz,
                lo: floor_y + 3,
                hi: ceil_y + 1,
            });
        }
    }
    lights
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::element_processing::highways::hang_tunnel_lights;
    use crate::element_processing::highways::{
        build_highway_connectivity_map, HighwayConnectivityMap,
    };
    use crate::osm_parser::ProcessedNode;
    use std::path::PathBuf;

    fn test_editor(xzbbox: &XZBBox) -> WorldEditor<'_> {
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        WorldEditor::new(PathBuf::from("/dev/null/unused"), xzbbox, llbbox)
    }

    // The same carve as before deduplication: every listed point, in order, then its lights.
    fn carve_every_point(editor: &mut WorldEditor, points: &[RailTunnelPoint]) {
        let wl: &[Block] = &[
            STONE_BRICKS,
            CRACKED_STONE_BRICKS,
            MOSSY_STONE_BRICKS,
            STONE,
        ];
        let mut lights = Vec::new();
        for &(bx, bz, floor_y, lantern) in points {
            let ceil_y = floor_y + INTERIOR_HEIGHT + 1;
            for dx in -AIR_RADIUS..=AIR_RADIUS {
                for dz in -AIR_RADIUS..=AIR_RADIUS {
                    for y in (floor_y + 1)..ceil_y {
                        if !(dx == 0 && dz == 0 && y == floor_y + 1) {
                            editor.set_block_absolute(AIR, bx + dx, y, bz + dz, Some(wl), None);
                        }
                    }
                }
            }
            if lantern {
                lights.push((bx, bz, floor_y, ceil_y));
            }
        }
        let lights: Vec<TunnelLight> = lights
            .into_iter()
            .map(
                |(x, z, floor_y, ceil_y): (i32, i32, i32, i32)| TunnelLight {
                    x,
                    z,
                    lo: floor_y + 3,
                    hi: ceil_y + 1,
                },
            )
            .collect();
        hang_tunnel_lights(editor, &lights);
    }

    // Tiles list a tunnel once each; carving each point once must build the same bore,
    // lanterns included.
    #[test]
    fn repeated_tunnel_points_carve_the_same_bore() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 80, 80).unwrap();
        let line: Vec<RailTunnelPoint> = (20..=50)
            .map(|x| (x, 40))
            .chain((41..=60).map(|z| (50, z)))
            .enumerate()
            .map(|(i, (x, z))| (x, z, -8, i % LIGHT_INTERVAL == 0))
            .collect();
        // Two tiles' copies, the second starting mid-way, plus a stray repeat.
        let mut points = line.clone();
        points.extend_from_slice(&line[7..]);
        points.extend_from_slice(&line);
        points.push((33, 40, -8, false));

        let build = |carve: &dyn Fn(&mut WorldEditor)| {
            let mut editor = test_editor(&xzbbox);
            for x in 10..=70 {
                for z in 30..=70 {
                    let ground = editor.get_ground_level(x, z);
                    for y in ground - 12..ground {
                        let block = if (x + z) % 5 == 0 { RAIL } else { STONE };
                        editor.set_block_absolute(block, x, y, z, None, None);
                    }
                }
            }
            carve(&mut editor);
            let mut cells = Vec::new();
            for x in 10..=70 {
                for z in 30..=70 {
                    let ground = editor.get_ground_level(x, z);
                    for y in ground - 12..ground {
                        cells.push(editor.get_block_absolute(x, y, z).map(|b| b.id()));
                    }
                }
            }
            cells
        };
        let want = build(&|e| carve_every_point(e, &line));
        let got = build(&|e| {
            let lights = carve_rail_tunnel_interior(e, &points);
            hang_tunnel_lights(e, &lights);
        });
        assert_eq!(got, want);
        assert!(got.contains(&Some(SEA_LANTERN.id())));
    }

    // Under rising ground the next point carves its roof a block higher, through this
    // point's roof row; the light goes up to the roof that is left.
    #[test]
    fn tunnel_lanterns_hang_from_the_roof_left_after_carving() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 60, 60).unwrap();
        let mut editor = test_editor(&xzbbox);
        for x in 25..=40 {
            for z in 30..=50 {
                editor.register_road_surface_y(x, z, 1);
            }
        }
        for x in 10..=40 {
            for z in 30..=50 {
                let ground = editor.get_ground_level(x, z);
                for y in ground - 12..ground {
                    editor.set_block_absolute(STONE, x, y, z, None, None);
                }
            }
        }
        let points: Vec<RailTunnelPoint> = (15..=35)
            .map(|x| (x, 40, editor.get_ground_level(x, 40) - 8, x == 24))
            .collect();
        let lights = carve_rail_tunnel_interior(&mut editor, &points);
        hang_tunnel_lights(&mut editor, &lights);
        let light = (-12..0)
            .find(|&y| editor.check_for_block_absolute(24, y, 40, Some(&[SEA_LANTERN]), None))
            .expect("the bore keeps its light");
        assert!(
            editor.block_exists_absolute(24, light + 1, 40),
            "the light at y={light} hangs in the air"
        );
    }

    // A straight east-west rail from x=20 to x=60 at z=50 (so cell index == x - 20).
    fn straight_rail(tags: &[(&str, &str)]) -> ProcessedWay {
        let mut t = HashMap::new();
        for (k, v) in tags {
            t.insert(k.to_string(), v.to_string());
        }
        ProcessedWay {
            id: 1,
            nodes: vec![
                ProcessedNode {
                    id: 1,
                    tags: HashMap::new(),
                    x: 20,
                    z: 50,
                },
                ProcessedNode {
                    id: 2,
                    tags: HashMap::new(),
                    x: 60,
                    z: 50,
                },
            ],
            tags: t,
        }
    }

    fn run_collecting_tunnel_points(
        editor: &mut WorldEditor,
        way: &ProcessedWay,
        rail_mask: &CoordinateBitmap,
    ) -> Vec<RailTunnelPoint> {
        run_joined(editor, way, rail_mask, &RailJoins::default())
    }

    fn run_joined(
        editor: &mut WorldEditor,
        way: &ProcessedWay,
        rail_mask: &CoordinateBitmap,
        joins: &RailJoins,
    ) -> Vec<RailTunnelPoint> {
        let clear = CoordinateBitmap::new(rail_mask_bbox());
        let mut rail_tunnel_points = Vec::new();
        let outlines = BridgeOutlineIndex::build(&[]);
        let structures =
            BridgeStructureMap::build(&[], editor, &HighwayConnectivityMap::new(), &outlines, 1.0);
        generate_railways(
            editor,
            way,
            &mut rail_tunnel_points,
            &outlines,
            &structures,
            &BridgeSurfaceMap::empty(),
            &clear,
            &clear,
            rail_mask,
            joins,
        );
        rail_tunnel_points
    }

    fn run(editor: &mut WorldEditor, way: &ProcessedWay, rail_mask: &CoordinateBitmap) {
        let _ = run_collecting_tunnel_points(editor, way, rail_mask);
    }

    fn rail_mask_bbox() -> &'static XZBBox {
        use std::sync::OnceLock;
        static BBOX: OnceLock<XZBBox> = OnceLock::new();
        BBOX.get_or_init(|| XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap())
    }

    /// Every rail in a box, as (cell, rail y, block).
    fn laid_rails(
        editor: &WorldEditor,
        x0: i32,
        z0: i32,
        x1: i32,
        z1: i32,
    ) -> Vec<((i32, i32), i32, Block)> {
        let mut out = Vec::new();
        for x in x0..=x1 {
            for z in z0..=z1 {
                for y in -12..12 {
                    if let Some(b) = editor
                        .get_block_absolute(x, y, z)
                        .filter(|&b| is_rail_block(b))
                    {
                        out.push(((x, z), y, b));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn street_track_takes_a_road_step_on_the_straight_not_on_the_bend() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 80, 80).unwrap();
        let mut editor = test_editor(&xzbbox);
        // The road climbs a block right where the track turns from east to south.
        let road = |z: i32| i32::from(z > 40);
        for x in 10..=30 {
            editor.register_road_surface_y(x, 40, road(40));
            editor.set_block_absolute(GRAY_CONCRETE, x, road(40), 40, None, None);
        }
        for z in 41..=60 {
            editor.register_road_surface_y(30, z, road(z));
            editor.set_block_absolute(GRAY_CONCRETE, 30, road(z), z, None, None);
        }
        let mut way = straight_rail(&[("railway", "tram")]);
        way.nodes[0].x = 10;
        way.nodes[0].z = 40;
        way.nodes[1].x = 30;
        way.nodes[1].z = 40;
        let mut corner = way.nodes[1].clone();
        corner.id = 3;
        corner.z = 60;
        way.nodes.push(corner);
        run(&mut editor, &way, &CoordinateBitmap::new(&xzbbox));
        let laid = laid_rails(&editor, 0, 0, 80, 80);
        assert_eq!(laid.len(), 41);
        assert_eq!(crate::element_processing::rail_track::loose_ends(&laid), 2);
    }

    #[test]
    fn a_rail_goes_through_leaves_hanging_over_the_track() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        for x in 30..=40 {
            editor.set_block_absolute(OAK_LEAVES, x, 1, 50, None, None);
        }
        let way = straight_rail(&[("railway", "rail")]);
        run(&mut editor, &way, &CoordinateBitmap::new(&xzbbox));
        for x in 20..=60 {
            assert!(
                editor.check_for_block_absolute(x, 1, 50, Some(&[RAIL_EAST_WEST]), None),
                "no rail at {x}"
            );
        }
    }

    #[test]
    fn electrified_rail_gets_catenary_over_a_plain_rail() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let way = straight_rail(&[
            ("railway", "rail"),
            ("usage", "main"),
            ("electrified", "contact_line"),
        ]);
        run(&mut editor, &way, &CoordinateBitmap::new(&xzbbox));

        assert!(
            editor.check_for_block(20, 1, 50, Some(&[RAIL_EAST_WEST])),
            "plain vanilla rail"
        );
        assert!(
            !editor.check_for_block(20, 0, 49, Some(&[ANVIL])),
            "no decorative rails"
        );
        assert!(
            editor.check_for_block(20, CATENARY_WIRE_HEIGHT, 50, Some(&[CHAIN_X])),
            "contact wire"
        );
        assert!(
            editor.check_for_block(20, 3, 53, Some(&[COBBLESTONE_WALL])),
            "mast column"
        );
        assert!(
            editor.check_for_block(20, 0, 53, Some(&[GRAY_CONCRETE])),
            "mast foundation"
        );
    }

    #[test]
    fn tram_gets_no_catenary() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let way = straight_rail(&[("railway", "tram"), ("electrified", "contact_line")]);
        run(&mut editor, &way, &CoordinateBitmap::new(&xzbbox));

        assert!(
            editor.check_for_block(20, 1, 50, Some(&[RAIL_EAST_WEST])),
            "tram keeps its rail"
        );
        assert!(
            !editor.check_for_block(20, CATENARY_WIRE_HEIGHT, 50, Some(&[CHAIN_X, CHAIN_Z])),
            "catenary is heavy-rail only"
        );
    }

    #[test]
    fn heavy_rail_tunnel_builds_and_carves_an_underground_track() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let way = straight_rail(&[("railway", "rail"), ("tunnel", "yes")]);

        let rail_tunnel_points =
            run_collecting_tunnel_points(&mut editor, &way, &CoordinateBitmap::new(&xzbbox));
        assert_eq!(rail_tunnel_points.len(), 41, "tunnel centerline collected");

        carve_rail_tunnel_interior(&mut editor, &rail_tunnel_points);

        let floor_y = -RAIL_TUNNEL_DEPTH - INTERIOR_HEIGHT - 1;
        assert!(
            editor.check_for_block_absolute(20, floor_y + 1, 50, Some(&[RAIL_EAST_WEST]), None,),
            "rail is preserved on the tunnel floor"
        );
        assert!(
            !editor.check_for_block_absolute(
                24,
                floor_y + 2,
                50,
                Some(&[
                    STONE,
                    STONE_BRICKS,
                    CRACKED_STONE_BRICKS,
                    MOSSY_STONE_BRICKS,
                ]),
                None,
            ),
            "tunnel interior is carved after shell placement"
        );
        assert!(
            editor.check_for_block_absolute(
                24,
                floor_y + 2,
                52,
                Some(&[STONE_BRICKS, CRACKED_STONE_BRICKS, MOSSY_STONE_BRICKS]),
                None,
            ),
            "tunnel wall remains around the carved interior"
        );
        assert!(
            !editor.check_for_block(24, 1, 50, Some(&[RAIL_EAST_WEST])),
            "tunnel rail is not rendered at grade"
        );
    }

    #[test]
    fn a_tunnel_under_low_ground_keeps_every_rail_above_the_world_floor() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        // Too little ground for the usual depth.
        let low = crate::world_editor::min_y() + 6;
        for x in 0..=80 {
            for z in 40..=60 {
                editor.register_road_surface_y(x, z, low);
            }
        }
        // Two tunnel ways that join underground.
        let first = straight_rail(&[("railway", "subway")]);
        let mut second = first.clone();
        second.id = 2;
        second.nodes[0] = first.nodes[1].clone();
        second.nodes[1].id = 3;
        second.nodes[1].x = 80;
        let elements = vec![
            ProcessedElement::Way(first.clone()),
            ProcessedElement::Way(second.clone()),
        ];
        let joins = RailJoins::build(&elements, track_level);
        let mask = CoordinateBitmap::new(&xzbbox);
        let mut points = run_joined(&mut editor, &first, &mask, &joins);
        points.extend(run_joined(&mut editor, &second, &mask, &joins));
        assert_eq!(points.len(), 41 + 21);
        for &(x, z, floor, _) in &points {
            assert!(floor > crate::world_editor::min_y());
            assert!(
                editor.check_for_block_absolute(x, floor + 1, z, Some(&[RAIL_EAST_WEST]), None),
                "no rail at {x}"
            );
        }
    }

    #[test]
    fn a_tunnel_climbs_out_to_meet_the_track_at_its_portal() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let open = straight_rail(&[("railway", "rail")]);
        let mut tunnel = straight_rail(&[("railway", "rail"), ("tunnel", "yes")]);
        tunnel.id = 2;
        tunnel.nodes[0] = open.nodes[1].clone();
        tunnel.nodes[1].id = 3;
        tunnel.nodes[1].x = 140;
        let elements = vec![
            ProcessedElement::Way(open.clone()),
            ProcessedElement::Way(tunnel.clone()),
        ];
        let joins = RailJoins::build(&elements, track_level);
        let mask = CoordinateBitmap::new(&xzbbox);
        let mut points = run_joined(&mut editor, &open, &mask, &joins);
        points.extend(run_joined(&mut editor, &tunnel, &mask, &joins));
        carve_rail_tunnel_interior(&mut editor, &points);
        let laid = laid_rails(&editor, 0, 0, 199, 119);
        assert_eq!(laid.len(), 41 + 80);
        assert_eq!(crate::element_processing::rail_track::loose_ends(&laid), 2);
        // Level with the open track at the portal, down a ramp, at its full depth further in.
        assert!(laid.contains(&((60, 50), 1, RAIL_EAST_WEST)));
        let at = |x: i32| laid.iter().find(|l| l.0 == (x, 50)).unwrap().1;
        assert!(at(68) >= -1, "{} eight cells in", at(68));
        let deepest = laid.iter().map(|l| l.1).min().unwrap();
        assert_eq!(deepest, -RAIL_TUNNEL_DEPTH - INTERIOR_HEIGHT);
    }

    #[test]
    fn rail_tunnel_routing_accepts_only_track_ways() {
        assert!(renders_as_rail_tunnel(&straight_rail(&[
            ("railway", "rail"),
            ("tunnel", "yes"),
        ])));
        assert!(renders_as_rail_tunnel(&straight_rail(&[(
            "railway", "subway"
        )])));
        assert!(!renders_as_rail_tunnel(&straight_rail(&[
            ("railway", "rail"),
            ("tunnel", "yes"),
            ("area", "yes"),
        ])));
        assert!(!renders_as_rail_tunnel(&straight_rail(&[
            ("railway", "platform"),
            ("tunnel", "yes"),
        ])));
        assert!(!renders_as_rail_tunnel(&straight_rail(&[
            ("railway", "station"),
            ("subway", "yes"),
        ])));
    }

    #[test]
    fn elevated_subway_renders_as_bridge_not_tunnel() {
        let viaduct =
            straight_rail(&[("railway", "subway"), ("bridge", "viaduct"), ("layer", "2")]);
        assert!(!renders_as_rail_tunnel(&viaduct));
        assert!(renders_as_rail_bridge(&viaduct));

        let surface = straight_rail(&[("railway", "subway"), ("tunnel", "no")]);
        assert!(!renders_as_rail_tunnel(&surface));

        let bored = straight_rail(&[("railway", "subway"), ("bridge", "yes"), ("tunnel", "yes")]);
        assert!(renders_as_rail_tunnel(&bored));
        assert!(!renders_as_rail_bridge(&bored));
    }

    #[test]
    fn non_track_tunnel_ways_do_not_generate_rails() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        for way in [
            straight_rail(&[("railway", "rail"), ("tunnel", "yes"), ("area", "yes")]),
            straight_rail(&[("railway", "platform"), ("tunnel", "yes")]),
            straight_rail(&[("railway", "station"), ("subway", "yes")]),
        ] {
            let mut editor = test_editor(&xzbbox);
            let points =
                run_collecting_tunnel_points(&mut editor, &way, &CoordinateBitmap::new(&xzbbox));
            assert!(points.is_empty(), "non-track way must not become a tunnel");
            assert!(
                !editor.check_for_block(20, 1, 50, Some(&[RAIL_EAST_WEST])),
                "non-track way must not fall through to at-grade rail rendering"
            );
        }
    }

    #[test]
    fn rail_tunnel_footprint_covers_the_full_shell_only_for_tracks() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let non_tracks = vec![
            ProcessedElement::Way(straight_rail(&[("railway", "platform"), ("tunnel", "yes")])),
            ProcessedElement::Way(straight_rail(&[("railway", "station"), ("subway", "yes")])),
        ];
        let mut footprint = CoordinateBitmap::new_empty();
        add_tunnel_footprint(&non_tracks, &xzbbox, &mut footprint);
        assert!(
            footprint.is_empty(),
            "platform and station ways do not allocate a tunnel footprint"
        );

        footprint = CoordinateBitmap::new(&xzbbox);
        footprint.set(10, 10);
        let track = vec![ProcessedElement::Way(straight_rail(&[
            ("railway", "rail"),
            ("tunnel", "yes"),
        ]))];
        add_tunnel_footprint(&track, &xzbbox, &mut footprint);

        assert!(
            footprint.contains(10, 10),
            "existing footprint is preserved"
        );
        assert!(footprint.contains(40, 50), "track centerline is protected");
        assert!(footprint.contains(40, 52), "outer tunnel wall is protected");
        assert!(
            !footprint.contains(40, 53),
            "footprint stops outside the 5x5 shell"
        );
    }

    // A rail bridge from x=20 to x=60 along row `z`.
    fn rail_bridge(id: u64, z: i32) -> ProcessedWay {
        let mut way = straight_rail(&[("railway", "rail"), ("bridge", "yes")]);
        way.id = id;
        for (i, node) in way.nodes.iter_mut().enumerate() {
            node.id = id * 10 + i as u64;
            node.z = z;
        }
        way
    }

    fn render_rail_bridges(editor: &mut WorldEditor, ways: &[ProcessedWay]) {
        let elements: Vec<ProcessedElement> =
            ways.iter().cloned().map(ProcessedElement::Way).collect();
        let outlines = BridgeOutlineIndex::build(&elements);
        let structures = BridgeStructureMap::build(
            &elements,
            editor,
            &build_highway_connectivity_map(&elements),
            &outlines,
            1.0,
        );
        let surface = BridgeSurfaceMap::build(&elements, &structures, 1.0);
        let clear = CoordinateBitmap::new(rail_mask_bbox());
        for way in ways {
            generate_railways(
                editor,
                way,
                &mut Vec::new(),
                &outlines,
                &structures,
                &surface,
                &clear,
                &clear,
                &clear,
                &RailJoins::default(),
            );
        }
    }

    #[test]
    fn rail_bridge_deck_has_ballast_curbs_and_railings() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        render_rail_bridges(&mut editor, &[rail_bridge(1, 50)]);

        let deck = RAIL_BRIDGE_FLAT_CLEARANCE;
        let at = |editor: &WorldEditor, y: i32, z: i32, blocks: &[Block]| {
            editor.check_for_block_absolute(41, y, z, Some(blocks), None)
        };
        assert!(
            at(&editor, deck + 1, 50, &[RAIL_EAST_WEST]),
            "track on the deck"
        );
        for z in [49, 51] {
            assert!(at(&editor, deck, z, &[GRAVEL]), "ballast beside the track");
        }
        for z in [48, 52] {
            assert!(at(&editor, deck, z, &[STONE_BRICKS]), "curb");
            assert!(at(&editor, deck + 1, z, &[IRON_BARS]), "railing");
            assert!(at(&editor, deck - 2, z, &[STONE_BRICKS]), "edge girder");
        }
        assert!(!at(&editor, deck, 53, &[GRAVEL, STONE_BRICKS]), "five wide");
    }

    #[test]
    fn side_by_side_rail_bridges_share_one_deck() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        render_rail_bridges(&mut editor, &[rail_bridge(1, 50), rail_bridge(2, 57)]);

        let deck = RAIL_BRIDGE_FLAT_CLEARANCE;
        let at = |editor: &WorldEditor, y: i32, z: i32, blocks: &[Block]| {
            editor.check_for_block_absolute(41, y, z, Some(blocks), None)
        };
        for z in 51..57 {
            assert!(at(&editor, deck, z, &[GRAVEL]), "decked over at z={z}");
            assert!(
                !at(&editor, deck + 1, z, &[IRON_BARS]),
                "no railing at z={z}"
            );
        }
        assert!(at(&editor, deck + 1, 48, &[IRON_BARS]), "outer railing");
        assert!(at(&editor, deck + 1, 59, &[IRON_BARS]), "outer railing");
    }

    fn rail_bridge_between(id: u64, a: (i32, i32), b: (i32, i32), layer: &str) -> ProcessedWay {
        let mut way = rail_bridge(id, a.1);
        way.tags.insert("layer".to_string(), layer.to_string());
        way.nodes[0].x = a.0;
        way.nodes[1].x = b.0;
        way.nodes[1].z = b.1;
        way
    }

    #[test]
    fn rail_decks_far_apart_in_height_keep_their_railings() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let low = rail_bridge_between(1, (20, 50), (60, 50), "1");
        let high = rail_bridge_between(2, (20, 54), (60, 54), "3");
        render_rail_bridges(&mut editor, &[low, high]);

        let low_y = RAIL_BRIDGE_FLAT_CLEARANCE;
        let high_y = low_y + 2 * 6;
        for y in [low_y, high_y] {
            assert!(
                editor.check_for_block_absolute(41, y + 1, 52, Some(&[IRON_BARS]), None),
                "facing railing at y={y}"
            );
        }
    }

    #[test]
    fn upper_rail_deck_stays_whole_over_a_lower_track() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let low = rail_bridge_between(1, (20, 50), (60, 50), "1");
        let high = rail_bridge_between(2, (40, 20), (40, 80), "3");
        render_rail_bridges(&mut editor, &[low, high]);

        let high_y = RAIL_BRIDGE_FLAT_CLEARANCE + 2 * 6;
        for x in [39, 41] {
            assert!(
                editor.check_for_block_absolute(x, high_y, 50, Some(&[GRAVEL]), None),
                "upper ballast over the lower track at x={x}"
            );
        }
        assert!(
            editor.check_for_block_absolute(
                39,
                RAIL_BRIDGE_FLAT_CLEARANCE + 1,
                50,
                Some(&[RAIL_EAST_WEST]),
                None
            ),
            "lower track keeps its rail"
        );
    }

    #[test]
    fn rail_bridge_profile_ramps_from_loose_ends_only() {
        let profile = rail_bridge_profile(&[0; 40], 6, false, true);
        assert_eq!(profile[0], 0, "loose end starts on the ground");
        assert!(profile.windows(2).all(|w| (w[1] - w[0]).abs() <= 1));
        assert_eq!(*profile.last().unwrap(), 6, "joined end stays at the level");
    }

    #[test]
    fn catenary_mast_flips_away_from_a_parallel_track() {
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let way = straight_rail(&[
            ("railway", "rail"),
            ("usage", "main"),
            ("electrified", "contact_line"),
        ]);

        // Parallel track 4 cells to the +z side (its rail head lands at the +3 mast base).
        let mut rail_mask = CoordinateBitmap::new(&xzbbox);
        for x in 18..62 {
            rail_mask.set(x, 54);
        }
        run(&mut editor, &way, &rail_mask);

        assert!(
            !editor.check_for_block(20, 0, 53, Some(&[GRAY_CONCRETE])),
            "mast must not stand on the neighbouring track's side"
        );
        assert!(
            editor.check_for_block(20, 0, 47, Some(&[GRAY_CONCRETE])),
            "mast flips to the clear outer side"
        );
    }
}
