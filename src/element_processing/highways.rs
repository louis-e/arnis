use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::element_processing::get_nearest_non_road_block;
use crate::element_processing::surfaces::{
    get_blocks_for_surface, get_blocks_for_surface_way, semirandom_surface,
};
use crate::floodfill_cache::{CoordinateBitmap, FloodFillCache, RoadMaskBitmap};
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::HashMap;

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
];

/// True when the way should render as a pedestrian walkway
/// rather than asphalt.
fn is_pedestrian_way(element: &ProcessedElement) -> bool {
    let tags = element.tags();
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

/// Minimum terrain dip (in blocks) below max endpoint elevation to classify a bridge as valley-spanning
const VALLEY_BRIDGE_THRESHOLD: i32 = 7;

/// Generates highways with elevation support based on layer tags and connectivity analysis
pub fn generate_highways(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    highway_connectivity: &HighwayConnectivityMap,
    flood_fill_cache: &FloodFillCache,
    road_mask: &RoadMaskBitmap,
) {
    generate_highways_internal(
        editor,
        element,
        args,
        highway_connectivity,
        flood_fill_cache,
        road_mask,
    );
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
fn generate_highways_internal(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    highway_connectivity: &HashMap<(i32, i32), Vec<i32>>, // Maps node coordinates to list of layers that connect to this node
    flood_fill_cache: &FloodFillCache,
    road_mask: &RoadMaskBitmap,
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
            // Handle street lamps
            if let ProcessedElement::Node(first_node) = element {
                let x: i32 = first_node.x;
                let z: i32 = first_node.z;
                editor.set_block(COBBLESTONE_WALL, x, layer_boost + 1, z, None, None);
                for dy in 2..=4 {
                    editor.set_block(OAK_FENCE, x, layer_boost + dy, z, None, None);
                }
                editor.set_block(GLOWSTONE, x, layer_boost + 5, z, None, None);
            }
        } else if highway_type == "crossing" {
            // Handle traffic signals for crossings
            if let Some(crossing_type) = element.tags().get("crossing") {
                if crossing_type == "traffic_signals" {
                    if let ProcessedElement::Node(node) = element {
                        let x = node.x;
                        let z = node.z;

                        // Try to build a hanging signal if it's on a road
                        let anchor = road_mask
                            .contains(x, z)
                            .then(|| get_nearest_non_road_block(x, z, 4, road_mask))
                            .flatten();

                        match anchor {
                            Some((ax, az)) => {
                                // Hanging signal: pole at roadside anchor, bars strung across
                                editor.set_block(
                                    COBBLESTONE_WALL,
                                    ax,
                                    layer_boost + 1,
                                    az,
                                    None,
                                    None,
                                );
                                editor.set_block(IRON_BARS, ax, layer_boost + 2, az, None, None);
                                editor.set_block(IRON_BARS, ax, layer_boost + 3, az, None, None);
                                editor.set_block(IRON_BARS, ax, layer_boost + 4, az, None, None);
                                editor.set_block(IRON_BARS, ax, layer_boost + 5, az, None, None);

                                let y = editor.get_absolute_y(x, layer_boost + 5, z);
                                for (lx, _, lz) in bresenham_line(x, y, z, ax, y, az) {
                                    editor.set_block(
                                        IRON_BARS,
                                        lx,
                                        layer_boost + 6,
                                        lz,
                                        None,
                                        None,
                                    );
                                }
                            }
                            None => {
                                // Standalone pole (off-road or no anchor found)
                                editor.set_block(
                                    COBBLESTONE_WALL,
                                    x,
                                    layer_boost + 1,
                                    z,
                                    None,
                                    None,
                                );
                                editor.set_block(IRON_BARS, x, layer_boost + 2, z, None, None);
                                editor.set_block(IRON_BARS, x, layer_boost + 3, z, None, None);
                            }
                        }

                        // Signal head (shared for both cases)
                        editor.set_block(BLACK_WOOL, x, layer_boost + 4, z, None, None);
                        editor.set_block(BLACK_WOOL, x, layer_boost + 5, z, None, None);

                        // Banner placement logic. Patterns expressed as
                        // (java_color, java_pattern_id) pairs so both Java
                        // and Bedrock writers can use them.
                        const BANNER_PATTERNS: &[(&str, &str)] = &[
                            ("red", "minecraft:triangle_top"),
                            ("lime", "minecraft:triangle_bottom"),
                            ("yellow", "minecraft:circle"),
                            ("black", "minecraft:curly_border"),
                            ("black", "minecraft:border"),
                        ];

                        let abs_y = editor.get_absolute_y(x, layer_boost + 5, z);
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
                                abs_y,
                                z + dz,
                                facing,
                                "light_gray",
                                BANNER_PATTERNS,
                            );
                        }
                    }
                }
            }
        } else if highway_type == "bus_stop" {
            // Handle bus stops
            if let ProcessedElement::Node(node) = element {
                let x = node.x;
                let z = node.z;
                for dy in 1..=3 {
                    editor.set_block(COBBLESTONE_WALL, x, layer_boost + dy, z, None, None);
                }

                editor.set_block(WHITE_WOOL, x, layer_boost + 4, z, None, None);
                editor.set_block(WHITE_WOOL, x + 1, layer_boost + 4, z, None, None);
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
            let mut block_range: i32 = 2;
            // default_lanes == 2 for highway types that historically had a
            // center stripe; flipped to `lanes > 1` check below after we
            // resolve the lanes tag. Keeps the same visual default.
            let mut default_lanes: i32 = 1;
            let scale_factor = args.scale;

            // Check if this is a bridge - bridges need special elevation handling
            // to span across valleys instead of following terrain.
            // Accept any bridge tag value except "no" (e.g., "yes", "viaduct",
            // "aqueduct", ...). Indoor highways are never treated as bridges
            // (indoor corridors should not generate elevated decks or support
            // pillars).
            let is_bridge = !is_indoor && element.tags().get("bridge").is_some_and(|v| v != "no");

            // Reuse the function-level layer resolution (already normalised
            // to 0 for indoor/negative).
            let layer_value = layer_value_effective;

            // Skip if 'level' is negative in the tags (indoor mapping)
            if let Some(level) = element.tags().get("level") {
                if level.parse::<i32>().unwrap_or(0) < 0 {
                    return;
                }
            }

            // Determine block type and range based on highway type
            match highway_type.as_str() {
                "footway" | "pedestrian" => {
                    block_types = &[GRAY_CONCRETE];
                    block_range = 1;
                }
                "path" => {
                    block_types = &[DIRT_PATH];
                    block_range = 1;
                }
                "motorway" | "primary" | "trunk" => {
                    block_range = 5;
                    default_lanes = 2;
                }
                "secondary" => {
                    block_range = 4;
                    default_lanes = 2;
                }
                "tertiary" => {
                    default_lanes = 2;
                }
                "track" => {
                    block_range = 1;
                }
                "service" => {
                    block_types = &[GRAY_CONCRETE];
                    block_range = 2;
                }
                "secondary_link" | "tertiary_link" => {
                    //Exit ramps, sliproads
                    block_range = 1;
                }
                "escape" => {
                    // Sand trap for vehicles on mountainous roads
                    block_types = &[SAND];
                    block_range = 1;
                }
                "steps" => {
                    //TODO: Add correct stairs respecting height, step_count, etc.
                    block_types = &[GRAY_CONCRETE];
                    block_range = 1;
                }

                _ => {
                    if let Some(lanes) = element.tags().get("lanes") {
                        if lanes == "2" {
                            block_range = 3;
                            default_lanes = 2;
                        } else if lanes != "1" {
                            block_range = 4;
                            default_lanes = 2;
                        }
                    }
                }
            }

            let ProcessedElement::Way(way) = element else {
                return;
            };

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

            // Optional explicit width via `width=*` (meters ≈ blocks).
            // Clamped to the terrain-flattening helper's sample-buffer cap.
            if let Some(w) = element
                .tags()
                .get("width")
                .and_then(|w| w.parse::<f32>().ok())
            {
                block_range = ((w / 2.0).round() as i32).clamp(1, MAX_BLOCK_RANGE as i32);
            }

            // Resolve lane-marking count. `lane_markings=no` disables them,
            // `lanes=*` overrides the default for this highway type.
            // Multi-lane inner dividers are drawn for lanes >= 2 (one line
            // between every pair of adjacent lanes).
            //
            // Clamped to a realistic upper bound: the world's widest real
            // roads have ~12 lanes, but an `i32` parse will accept
            // arbitrary OSM values. Without the cap, a stray `lanes=999999`
            // tag (typo or vandalism) would send the inner divider loop
            // into millions of iterations per bresenham point.
            const MAX_LANES: i32 = 16;
            let mut lanes = element
                .tags()
                .get("lanes")
                .and_then(|l| l.parse::<i32>().ok())
                .unwrap_or(default_lanes)
                .clamp(0, MAX_LANES);
            if element.tags().get("lane_markings").map(|s| s.as_str()) == Some("no") {
                lanes = 1;
            }

            if scale_factor < 1.0 {
                block_range = ((block_range as f64) * scale_factor).floor() as i32;
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

            // Calculate total way length for slope distribution (needed before valley bridge check)
            let total_way_length = calculate_way_length(way);

            // For bridges: detect if this spans a valley by checking terrain profile
            // A valley bridge has terrain that dips significantly below the endpoints
            // Skip valley detection entirely if terrain is disabled (no valleys in flat terrain)
            // Skip very short bridges (< 25 blocks) as they're unlikely to span significant valleys
            let terrain_enabled = editor
                .get_ground()
                .map(|g| g.elevation_enabled)
                .unwrap_or(false);

            let (is_valley_bridge, bridge_deck_y) =
                if is_bridge && terrain_enabled && way.nodes.len() >= 2 && total_way_length >= 25 {
                    let start_node = &way.nodes[0];
                    let end_node = &way.nodes[way.nodes.len() - 1];
                    let start_y = editor.get_ground_level(start_node.x, start_node.z);
                    let end_y = editor.get_ground_level(end_node.x, end_node.z);
                    let max_endpoint_y = start_y.max(end_y);

                    // Sample terrain at middle nodes only (excluding endpoints we already have)
                    // This avoids redundant get_ground_level() calls
                    let middle_nodes = &way.nodes[1..way.nodes.len().saturating_sub(1)];
                    let sampled_min = if middle_nodes.is_empty() {
                        // No middle nodes, just use endpoints
                        start_y.min(end_y)
                    } else {
                        // Sample up to 3 middle points (5 total with endpoints) for performance
                        // Valleys are wide terrain features, so sparse sampling is sufficient
                        let sample_count = middle_nodes.len().min(3);
                        let step = if sample_count > 1 {
                            (middle_nodes.len() - 1) / (sample_count - 1)
                        } else {
                            1
                        };

                        middle_nodes
                            .iter()
                            .step_by(step.max(1))
                            .map(|node| editor.get_ground_level(node.x, node.z))
                            .min()
                            .unwrap_or(max_endpoint_y)
                    };

                    // Include endpoint elevations in the minimum calculation
                    let min_terrain_y = sampled_min.min(start_y).min(end_y);

                    // If ANY sampled point along the bridge is significantly lower than the max endpoint,
                    // treat as valley bridge
                    let is_valley = min_terrain_y < max_endpoint_y - VALLEY_BRIDGE_THRESHOLD;

                    if is_valley {
                        (true, max_endpoint_y)
                    } else {
                        (false, 0)
                    }
                } else {
                    (false, 0)
                };

            // Check if this is a short isolated elevated segment (layer > 0), if so, treat as ground level
            let is_short_isolated_elevated =
                needs_start_slope && needs_end_slope && layer_value > 0 && total_way_length <= 35;

            // Override elevation and slopes for short isolated segments
            let (effective_elevation, effective_start_slope, effective_end_slope) =
                if is_short_isolated_elevated {
                    (0, false, false) // Treat as ground level
                } else {
                    (base_elevation, needs_start_slope, needs_end_slope)
                };

            let slope_length = (total_way_length as f32 * 0.35).clamp(15.0, 50.0) as usize; // 35% of way length, max 50 blocks, min 15 blocks

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
                    let flatten_width = !is_valley_bridge && block_range >= 1;
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

                    for (point_index, (x, _, z)) in bresenham_points.iter().enumerate() {
                        // Centerline-invariant Y offset for this point
                        // (slope ramps at layer transitions, valley bridge
                        // flat deck, etc.). Same for every cell in this
                        // cross-section — only the *ground reference* we
                        // add it to changes per cell, below.
                        let offset = if is_valley_bridge {
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

                        // Register override only when the road sits at its
                        // natural ground level at this point. See the
                        // segment-level comment above.
                        let register_ground_override = flatten_width && offset == 0;

                        // Per-cell target Y computation. Wide roads use a
                        // 1×(2b+1) perpendicular median sampled at the
                        // cell's *own* along-length coordinate: every cell
                        // in one lateral cross-section therefore gets the
                        // identical Y (width level — no sideways pitch on
                        // a slope) while adjacent cross-sections along
                        // the road's length see the terrain vary naturally.
                        //
                        // Closures can't wrap this because we need mutable
                        // borrows of `editor` (for `set_block_*`) right
                        // after the shared borrow used to read ground
                        // levels; inlining below keeps each borrow scoped
                        // to a single statement.
                        let use_absolute_y = is_valley_bridge || flatten_width;

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
                                let cell_y = if is_valley_bridge {
                                    bridge_deck_y
                                } else if flatten_width {
                                    let axial = if dir_horizontal { dx } else { dz };
                                    row_medians[(axial + block_range) as usize] + offset
                                } else {
                                    offset
                                };
                                if register_ground_override {
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
                                let is_elevated_deck = is_valley_bridge || effective_elevation > 0;
                                if is_elevated_deck && cell_y > 0 {
                                    // Add 1 layer of stone bricks underneath the highway surface
                                    if use_absolute_y {
                                        editor.set_block_absolute(
                                            STONE_BRICKS,
                                            set_x,
                                            cell_y - 1,
                                            set_z,
                                            None,
                                            None,
                                        );
                                    } else {
                                        editor.set_block(
                                            STONE_BRICKS,
                                            set_x,
                                            cell_y - 1,
                                            set_z,
                                            None,
                                            None,
                                        );
                                    }

                                    if use_absolute_y {
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
                                    let stripe_y = if is_valley_bridge {
                                        bridge_deck_y
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
                }
                previous_node = Some((node.x, node.z));
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
pub fn generate_siding(editor: &mut WorldEditor, element: &ProcessedWay) {
    let mut previous_node: Option<XZPoint> = None;
    let siding_block: Block = STONE_BRICK_SLAB;

    for node in &element.nodes {
        let current_node = node.xz();

        // Draw the siding using Bresenham's line algorithm between nodes
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
                if !editor.check_for_block(
                    bx,
                    0,
                    bz,
                    Some(&[
                        BLACK_CONCRETE,
                        GRAY_CONCRETE_POWDER,
                        CYAN_TERRACOTTA,
                        WHITE_CONCRETE,
                    ]),
                ) {
                    editor.set_block(siding_block, bx, 1, bz, None, None);
                }
            }
        }

        previous_node = Some(current_node);
    }
}

/// Generates an aeroway
pub fn generate_aeroway(editor: &mut WorldEditor, way: &ProcessedWay, args: &Args) {
    let mut previous_node: Option<(i32, i32)> = None;
    let surface_block = LIGHT_GRAY_CONCRETE;

    for node in &way.nodes {
        if let Some(prev) = previous_node {
            let (x1, z1) = prev;
            let x2 = node.x;
            let z2 = node.z;
            let points = bresenham_line(x1, 0, z1, x2, 0, z2);
            let way_width: i32 = (12.0 * args.scale).ceil() as i32;

            for (x, _, z) in points {
                for dx in -way_width..=way_width {
                    for dz in -way_width..=way_width {
                        let set_x = x + dx;
                        let set_z = z + dz;
                        editor.set_block(surface_block, set_x, 0, set_z, None, None);
                    }
                }
            }
        }
        previous_node = Some((node.x, node.z));
    }
}

/// Returns the half-width (block_range) for a highway type.
///
/// This extracts the same logic used inside `generate_highways_internal` so
/// that pre-scan passes (e.g. building-passage collection) can determine road
/// width without generating any blocks.
pub(crate) fn highway_block_range(
    highway_type: &str,
    tags: &HashMap<String, String>,
    scale: f64,
) -> i32 {
    let mut block_range: i32 = match highway_type {
        "footway" | "pedestrian" => 1,
        "path" => 1,
        "motorway" | "primary" | "trunk" => 5,
        "secondary" => 4,
        "tertiary" => 2,
        "track" => 1,
        "service" => 2,
        "secondary_link" | "tertiary_link" => 1,
        "escape" => 1,
        "steps" => 1,
        _ => {
            if let Some(lanes) = tags.get("lanes") {
                if lanes == "2" {
                    3
                } else if lanes != "1" {
                    4
                } else {
                    2
                }
            } else {
                2
            }
        }
    };

    if scale < 1.0 {
        block_range = ((block_range as f64) * scale).floor() as i32;
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
    xzbbox: &XZBBox,
    scale: f64,
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
