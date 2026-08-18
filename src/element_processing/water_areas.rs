use crate::block_definitions::WATER;
use crate::clipping::clip_water_ring_to_bbox;
use crate::floodfill_cache::RoadMaskBitmap;
use crate::ground::Ground;
use crate::water_depth::{carve_water_column, BigWaterField};
use crate::{
    coordinate_system::cartesian::{XZBBox, XZPoint},
    osm_parser::{
        ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedRelation, ProcessedWay,
    },
    world_editor::WorldEditor,
};
use fnv::FnvHashMap;
use rayon::prelude::*;

pub fn generate_water_area_from_way(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    bwf: &BigWaterField,
    road_mask: &RoadMaskBitmap,
    tunnel_footprint: &RoadMaskBitmap,
    surfaces: &StillWaterSurfaces,
) {
    let Some(outers) = way_rings(element) else {
        return;
    };
    let surface = surfaces.get("way", element.id);
    generate_water_areas(
        editor,
        &outers,
        &[],
        bwf,
        road_mask,
        tunnel_footprint,
        surface,
    );
}

/// Outer rings of a water way, or None if the ring is not usable.
fn way_rings(element: &ProcessedWay) -> Option<Vec<Vec<ProcessedNode>>> {
    let outers = vec![element.nodes.clone()];
    if !verify_closed_rings(&outers) {
        println!("Skipping way {} due to invalid polygon", element.id);
        return None;
    }
    Some(outers)
}

pub fn generate_water_areas_from_relation(
    editor: &mut WorldEditor,
    element: &ProcessedRelation,
    xzbbox: &XZBBox,
    bwf: &BigWaterField,
    road_mask: &RoadMaskBitmap,
    tunnel_footprint: &RoadMaskBitmap,
    surfaces: &StillWaterSurfaces,
) {
    let Some((outers, inners)) = relation_rings(element, xzbbox) else {
        return;
    };
    let surface = surfaces.get("relation", element.id);
    generate_water_areas(
        editor,
        &outers,
        &inners,
        bwf,
        road_mask,
        tunnel_footprint,
        surface,
    );
}

/// Outer and inner rings of a water relation, or None if it is not water or unusable.
#[allow(clippy::type_complexity)]
fn relation_rings(
    element: &ProcessedRelation,
    xzbbox: &XZBBox,
) -> Option<(Vec<Vec<ProcessedNode>>, Vec<Vec<ProcessedNode>>)> {
    // Check if this is a water relation (either with water tag or natural=water)
    let is_water = element.tags.contains_key("water")
        || element
            .tags
            .get("natural")
            .map(|val| val == "water" || val == "bay")
            .unwrap_or(false);

    if !is_water {
        return None;
    }

    // Don't handle water below layer 0
    if let Some(layer) = element.tags.get("layer") {
        if layer.parse::<i32>().map(|x| x < 0).unwrap_or(false) {
            return None;
        }
    }

    let mut outers: Vec<Vec<ProcessedNode>> = vec![];
    let mut inners: Vec<Vec<ProcessedNode>> = vec![];

    for mem in &element.members {
        match mem.role {
            ProcessedMemberRole::Outer => outers.push(mem.way.nodes.clone()),
            ProcessedMemberRole::Inner => inners.push(mem.way.nodes.clone()),
            ProcessedMemberRole::Part => {} // Not applicable to water areas
        }
    }

    // Preserve OSM-defined outer/inner roles without modification
    super::merge_way_segments(&mut outers);

    // Clip assembled rings to bbox (must happen after merging to preserve ring connectivity)
    outers = outers
        .into_iter()
        .filter_map(|ring| clip_water_ring_to_bbox(&ring, xzbbox))
        .collect();
    super::merge_way_segments(&mut inners);
    inners = inners
        .into_iter()
        .filter_map(|ring| clip_water_ring_to_bbox(&ring, xzbbox))
        .collect();

    if !verify_closed_rings(&outers) {
        // For clipped multipolygons, some loops may not close perfectly
        // Instead of force-closing with straight lines (which creates wedges),
        // filter out unclosed loops and only render the properly closed ones

        // Filter: Keep only loops that are already closed OR can be closed within 1 block
        outers.retain(|loop_nodes| {
            if loop_nodes.len() < 3 {
                return false;
            }
            let first = &loop_nodes[0];
            let last = loop_nodes.last().unwrap();
            let dx = (first.x - last.x).abs();
            let dz = (first.z - last.z).abs();

            // Keep if already closed by ID or endpoints are within 1 block
            first.id == last.id || (dx <= 1 && dz <= 1)
        });

        // Now close the remaining loops that are within 1 block tolerance
        for loop_nodes in outers.iter_mut() {
            let first = loop_nodes[0].clone();
            let last_idx = loop_nodes.len() - 1;
            if loop_nodes[0].id != loop_nodes[last_idx].id {
                // Endpoints are close (within tolerance), close the loop
                loop_nodes.push(first);
            }
        }

        // If no valid outer loops remain, skip the relation
        if outers.is_empty() {
            return None;
        }

        // Verify again after filtering and closing
        if !verify_closed_rings(&outers) {
            println!("Skipping relation {} due to invalid polygon", element.id);
            return None;
        }
    }

    super::merge_way_segments(&mut inners);
    if !verify_closed_rings(&inners) {
        println!("Skipping relation {} due to invalid polygon", element.id);
        return None;
    }

    Some((outers, inners))
}

fn generate_water_areas(
    editor: &mut WorldEditor,
    outers: &[Vec<ProcessedNode>],
    inners: &[Vec<ProcessedNode>],
    bwf: &BigWaterField,
    road_mask: &RoadMaskBitmap,
    tunnel_footprint: &RoadMaskBitmap,
    still_surface: Option<i32>,
) {
    // Calculate polygon bounding box to limit fill area
    let mut poly_min_x = i32::MAX;
    let mut poly_min_z = i32::MAX;
    let mut poly_max_x = i32::MIN;
    let mut poly_max_z = i32::MIN;

    for outer in outers {
        for node in outer {
            poly_min_x = poly_min_x.min(node.x);
            poly_min_z = poly_min_z.min(node.z);
            poly_max_x = poly_max_x.max(node.x);
            poly_max_z = poly_max_z.max(node.z);
        }
    }

    // If no valid bounds, nothing to fill
    if poly_min_x == i32::MAX || poly_max_x == i32::MIN {
        return;
    }

    // Clamp to world bounds just in case
    let (world_min_x, world_min_z) = editor.get_min_coords();
    let (world_max_x, world_max_z) = editor.get_max_coords();
    let min_x = poly_min_x.max(world_min_x);
    let min_z = poly_min_z.max(world_min_z);
    let max_x = poly_max_x.min(world_max_x);
    let max_z = poly_max_z.min(world_max_z);

    let outers_xz: Vec<Vec<XZPoint>> = outers
        .iter()
        .map(|x| x.iter().map(|y| y.xz()).collect::<Vec<_>>())
        .collect();
    let inners_xz: Vec<Vec<XZPoint>> = inners
        .iter()
        .map(|x| x.iter().map(|y| y.xz()).collect::<Vec<_>>())
        .collect();

    scanline_fill_water(
        min_x,
        min_z,
        max_x,
        max_z,
        &outers_xz,
        &inners_xz,
        editor,
        bwf,
        road_mask,
        tunnel_footprint,
        still_surface,
    );

    // Scatter boats over open water; grid on a global lattice with independent rolls, so the set is identical across parallel tiles.
    crate::structures::boat::scatter_boats(editor, min_x, min_z, max_x, max_z);
}

/// Verifies all rings are properly closed (first node matches last).
fn verify_closed_rings(rings: &[Vec<ProcessedNode>]) -> bool {
    let mut valid = true;
    for ring in rings {
        let first = &ring[0];
        let last = ring.last().unwrap();

        // Check if ring is closed (by ID or proximity)
        let is_closed = first.id == last.id || {
            let dx = (first.x - last.x).abs();
            let dz = (first.z - last.z).abs();
            dx <= 1 && dz <= 1
        };

        if !is_closed {
            eprintln!("WARN: Disconnected ring");
            valid = false;
        }
    }

    valid
}

// ============================================================================
// Scanline rasterization for water area filling
// ============================================================================
//
// For each row (z coordinate) in the fill area, computes polygon edge
// crossings to determine which x-ranges are inside the outer polygons but
// outside the inner polygons, then fills those ranges with water blocks.
//
// Complexity: O(E * H + A) where E = total edges, H = height of fill area,
// A = total filled area. This is dramatically faster than the previous
// quadtree + per-block point-in-polygon approach O(A * V * P) for large or
// complex water bodies (e.g. the Venetian Lagoon with dozens of inner island
// rings).

/// A polygon edge segment for scanline intersection testing.
struct ScanlineEdge {
    x1: f64,
    z1: f64,
    x2: f64,
    z2: f64,
}

/// Collects all non-horizontal edges from a single polygon ring.
///
/// If the ring is not perfectly closed (last point != first point),
/// the closing edge is added explicitly.
fn collect_ring_edges(ring: &[XZPoint]) -> Vec<ScanlineEdge> {
    let mut edges = Vec::new();
    if ring.len() < 2 {
        return edges;
    }
    for i in 0..ring.len() - 1 {
        let a = &ring[i];
        let b = &ring[i + 1];
        // Skip horizontal edges, they produce no scanline crossings
        if a.z != b.z {
            edges.push(ScanlineEdge {
                x1: a.x as f64,
                z1: a.z as f64,
                x2: b.x as f64,
                z2: b.z as f64,
            });
        }
    }
    // Add closing edge if the ring isn't perfectly closed by coordinates
    let first = ring.first().unwrap();
    let last = ring.last().unwrap();
    if first.z != last.z {
        edges.push(ScanlineEdge {
            x1: last.x as f64,
            z1: last.z as f64,
            x2: first.x as f64,
            z2: first.z as f64,
        });
    }
    edges
}

/// Collects edges from multiple rings into a single list.
/// Used for inner rings where even-odd on combined edges is correct
/// (inner rings of a valid multipolygon do not overlap).
fn collect_all_ring_edges(rings: &[Vec<XZPoint>]) -> Vec<ScanlineEdge> {
    let mut edges = Vec::new();
    for ring in rings {
        edges.extend(collect_ring_edges(ring));
    }
    edges
}

/// Computes the integer x-spans that are "inside" the polygon rings at
/// scanline `z`, using the even-odd (parity) rule.
///
/// The crossing test uses the same convention as `geo::Contains`:
/// an edge crosses the scanline when one endpoint is strictly above `z`
/// and the other is at or below.
fn compute_scanline_spans(
    edges: &[ScanlineEdge],
    z: f64,
    min_x: i32,
    max_x: i32,
) -> Vec<(i32, i32)> {
    let mut xs: Vec<f64> = Vec::new();
    for edge in edges {
        // Crossing test: (z1 > z) != (z2 > z)
        // Matches geo's convention (bottom-inclusive, top-exclusive).
        if (edge.z1 > z) != (edge.z2 > z) {
            let t = (z - edge.z1) / (edge.z2 - edge.z1);
            xs.push(edge.x1 + t * (edge.x2 - edge.x1));
        }
    }

    if xs.is_empty() {
        return Vec::new();
    }

    xs.sort_unstable_by(|a, b| {
        a.partial_cmp(b)
            .expect("NaN encountered while sorting scanline intersections")
    });

    debug_assert!(
        xs.len().is_multiple_of(2),
        "Odd number of scanline crossings ({}) at z={}, possible malformed polygon",
        xs.len(),
        z
    );

    // Pair consecutive crossings into fill spans (even-odd rule)
    let mut spans = Vec::with_capacity(xs.len() / 2);
    let mut i = 0;
    while i + 1 < xs.len() {
        let start = (xs[i].ceil() as i32).max(min_x);
        let end = (xs[i + 1].floor() as i32).min(max_x);
        if start <= end {
            spans.push((start, end));
        }
        i += 2;
    }

    spans
}

/// Merges two sorted, non-overlapping span lists into their union.
fn union_spans(a: &[(i32, i32)], b: &[(i32, i32)]) -> Vec<(i32, i32)> {
    if a.is_empty() {
        return b.to_vec();
    }
    if b.is_empty() {
        return a.to_vec();
    }

    // Merge both sorted lists and combine overlapping/adjacent spans
    let mut all: Vec<(i32, i32)> = Vec::with_capacity(a.len() + b.len());
    all.extend_from_slice(a);
    all.extend_from_slice(b);
    all.sort_unstable_by_key(|&(start, _)| start);

    let mut result: Vec<(i32, i32)> = Vec::new();
    let mut current = all[0];
    for &(start, end) in &all[1..] {
        if start <= current.1 + 1 {
            // Overlapping or adjacent, extend
            current.1 = current.1.max(end);
        } else {
            result.push(current);
            current = (start, end);
        }
    }
    result.push(current);
    result
}

/// Subtracts spans in `b` from spans in `a`.
///
/// Both inputs must be sorted and non-overlapping.
/// Returns sorted, non-overlapping spans representing `a \ b`.
fn subtract_spans(a: &[(i32, i32)], b: &[(i32, i32)]) -> Vec<(i32, i32)> {
    if b.is_empty() {
        return a.to_vec();
    }

    let mut result = Vec::new();
    let mut bi = 0;

    for &(a_start, a_end) in a {
        let mut pos = a_start;

        // Skip B spans that end before this A span starts
        while bi < b.len() && b[bi].1 < a_start {
            bi += 1;
        }

        // Walk through B spans that overlap with [pos .. a_end]
        let mut j = bi;
        while j < b.len() && b[j].0 <= a_end {
            if b[j].0 > pos {
                result.push((pos, (b[j].0 - 1).min(a_end)));
            }
            pos = pos.max(b[j].1 + 1);
            j += 1;
        }

        if pos <= a_end {
            result.push((pos, a_end));
        }
    }

    result
}

/// Polygon edges prepared for scanline filling: one edge list per outer ring
/// (unioned per row, so overlapping outer rings still fill correctly) and the
/// combined inner-ring edges (subtracted).
struct PolygonEdges {
    outer_groups: Vec<Vec<ScanlineEdge>>,
    inner: Vec<ScanlineEdge>,
}

impl PolygonEdges {
    fn new(outers: &[Vec<XZPoint>], inners: &[Vec<XZPoint>]) -> Self {
        Self {
            outer_groups: outers.iter().map(|ring| collect_ring_edges(ring)).collect(),
            inner: collect_all_ring_edges(inners),
        }
    }

    /// Filled x-spans of row `z`, clamped to `[min_x, max_x]`.
    fn row_spans(&self, z: i32, min_x: i32, max_x: i32) -> Vec<(i32, i32)> {
        let z_f = z as f64;
        let mut outer_spans: Vec<(i32, i32)> = Vec::new();
        for ring_edges in &self.outer_groups {
            let ring_spans = compute_scanline_spans(ring_edges, z_f, min_x, max_x);
            if !ring_spans.is_empty() {
                outer_spans = union_spans(&outer_spans, &ring_spans);
            }
        }
        if outer_spans.is_empty() || self.inner.is_empty() {
            return outer_spans;
        }
        let inner_spans = compute_scanline_spans(&self.inner, z_f, min_x, max_x);
        if inner_spans.is_empty() {
            outer_spans
        } else {
            subtract_spans(&outer_spans, &inner_spans)
        }
    }
}

/// Filled spans of a polygon for a range of rows, computed once.
struct SpanRows {
    rows: Vec<Vec<(i32, i32)>>,
    z0: i32,
}

impl SpanRows {
    fn build(edges: &PolygonEdges, z0: i32, z1: i32, min_x: i32, max_x: i32) -> Self {
        let rows = (z0..=z1)
            .map(|z| edges.row_spans(z, min_x, max_x))
            .collect();
        Self { rows, z0 }
    }

    fn get(&self, z: i32) -> &[(i32, i32)] {
        match usize::try_from(z - self.z0) {
            Ok(i) if i < self.rows.len() => &self.rows[i],
            _ => &[],
        }
    }

    fn contains(&self, x: i32, z: i32) -> bool {
        let spans = self.get(z);
        spans
            .binary_search_by(|&(a, b)| {
                if x < a {
                    std::cmp::Ordering::Greater
                } else if x > b {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    /// True if `(x, z)` and its four neighbours `margin` cells away are all inside.
    fn contains_interior(&self, x: i32, z: i32, margin: i32) -> bool {
        self.contains(x - margin, z)
            && self.contains(x + margin, z)
            && self.contains(x, z - margin)
            && self.contains(x, z + margin)
    }
}

/// How far a still body's surface may sit above the terrain before the column is left as
/// land. Past this the carve and its backfill no longer reach the ground.
const MAX_STILL_FILL_DROP: i32 = 20;

/// How far from the polygon edge a column must be to count as interior rather than bank.
/// Matches the water-level snap radius, so a snapped bank cell is never mistaken for one.
const INTERIOR_MARGIN: i32 = 4;

/// Share of a polygon's columns that must be ESA water to trust ESA's leveled surface.
const STILL_SURFACE_MIN_LC_SHARE: f64 = 0.3;
/// ...and at least this many such columns.
const STILL_SURFACE_MIN_LC_COLUMNS: usize = 64;
/// Rough cap on the number of columns sampled for the surface statistics.
const STILL_SURFACE_MAX_SAMPLES: usize = 250_000;

/// Water surface Y of every still OSM water body, resolved once per element.
#[derive(Default)]
pub struct StillWaterSurfaces(FnvHashMap<(&'static str, u64), i32>);

impl StillWaterSurfaces {
    fn get(&self, kind: &'static str, id: u64) -> Option<i32> {
        self.0.get(&(kind, id)).copied()
    }
}

/// Resolve every water polygon's surface before the tile fan-out, so a body spanning
/// many tiles is measured once and every tile agrees.
pub fn prescan_still_surfaces(
    elements: &[ProcessedElement],
    ground: &Ground,
    xzbbox: &XZBBox,
) -> StillWaterSurfaces {
    if !ground.has_land_cover() || !ground.elevation_enabled {
        return StillWaterSurfaces::default();
    }
    let entries: Vec<((&'static str, u64), i32)> = elements
        .par_iter()
        .filter_map(|element| {
            let (key, outers, inners) = match element {
                ProcessedElement::Way(way) if is_water_area_way(way) => {
                    (("way", way.id), way_rings(way)?, Vec::new())
                }
                ProcessedElement::Relation(rel) => {
                    let (outers, inners) = relation_rings(rel, xzbbox)?;
                    (("relation", rel.id), outers, inners)
                }
                _ => return None,
            };
            let to_xz = |rings: &[Vec<ProcessedNode>]| -> Vec<Vec<XZPoint>> {
                rings
                    .iter()
                    .map(|r| r.iter().map(|n| n.xz()).collect())
                    .collect()
            };
            let surface = still_surface_level(ground, &to_xz(&outers), &to_xz(&inners), xzbbox)?;
            Some((key, surface))
        })
        .collect();
    StillWaterSurfaces(entries.into_iter().collect())
}

/// Ways this renderer actually receives. `natural=water` and `landuse=reservoir` ways are
/// taken by the natural and landuse arms of the dispatch first, so a surface resolved for
/// them would never be read. Relations are not routed that way and keep the full predicate.
fn is_water_area_way(way: &ProcessedWay) -> bool {
    matches!(
        way.tags.get("waterway").map(String::as_str),
        Some("dock" | "riverbank")
    )
}

/// The single water-surface Y of a polygon that ESA also sees as one still body.
///
/// OSM draws water at full pool while ESA and the DEM have today's water, so a
/// drawn-down reservoir has columns well above it. Filling those at their own height
/// terraces water up the exposed bank. When enough of the polygon is ESA water sitting
/// at one leveled Y, that Y is the surface. Rivers are leveled per cell and never pass
/// the single-Y test, so they keep the per-column fill.
fn still_surface_level(
    ground: &Ground,
    outers: &[Vec<XZPoint>],
    inners: &[Vec<XZPoint>],
    xzbbox: &XZBBox,
) -> Option<i32> {
    let (mut min_x, mut min_z) = (i32::MAX, i32::MAX);
    let (mut max_x, mut max_z) = (i32::MIN, i32::MIN);
    for ring in outers {
        for p in ring {
            min_x = min_x.min(p.x);
            max_x = max_x.max(p.x);
            min_z = min_z.min(p.z);
            max_z = max_z.max(p.z);
        }
    }
    let min_x = min_x.max(xzbbox.min_x());
    let min_z = min_z.max(xzbbox.min_z());
    let max_x = max_x.min(xzbbox.max_x());
    let max_z = max_z.min(xzbbox.max_z());
    if min_x > max_x || min_z > max_z {
        return None;
    }

    // Sample on a lattice so the cost follows the sample count, not the area.
    let area = (i64::from(max_x - min_x) + 1) * (i64::from(max_z - min_z) + 1);
    let stride = ((area as f64 / STILL_SURFACE_MAX_SAMPLES as f64)
        .sqrt()
        .ceil() as i32)
        .max(1);
    let edges = PolygonEdges::new(outers, inners);
    let (off_x, off_z) = (xzbbox.min_x(), xzbbox.min_z());
    let mut total = 0usize;
    let mut lc_levels: Vec<i32> = Vec::new();
    let mut z = min_z;
    while z <= max_z {
        for (start, end) in edges.row_spans(z, min_x, max_x) {
            // Keep the lattice anchored to the world grid so it does not shift per polygon.
            let mut x = start + (stride - start.rem_euclid(stride)).rem_euclid(stride);
            while x <= end {
                total += 1;
                let coord = XZPoint::new(x - off_x, z - off_z);
                if ground.cover_class(coord) == crate::land_cover::LC_WATER {
                    lc_levels.push(ground.level(coord));
                }
                x += stride;
            }
        }
        z += stride;
    }
    if lc_levels.len() < STILL_SURFACE_MIN_LC_COLUMNS
        || (lc_levels.len() as f64) < STILL_SURFACE_MIN_LC_SHARE * total as f64
    {
        return None;
    }
    let n = lc_levels.len();
    let (q1, q3) = (n / 4, (n * 3) / 4);
    lc_levels.select_nth_unstable(q1);
    let q1_val = lc_levels[q1];
    lc_levels.select_nth_unstable(q3);
    let q3_val = lc_levels[q3];
    if q1_val != q3_val {
        return None;
    }
    lc_levels.select_nth_unstable(n / 2);
    Some(lc_levels[n / 2])
}

/// Fills water blocks using scanline rasterization.
///
/// For each row z in [min_z, max_z], computes which x positions are inside
/// any outer polygon ring but outside all inner polygon rings, and places
/// water blocks at those positions.
///
/// With `still_surface`, columns above that Y are skipped and the rest filled at it.
/// Otherwise each column is filled at its own water level.
#[allow(clippy::too_many_arguments)]
fn scanline_fill_water(
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    outers: &[Vec<XZPoint>],
    inners: &[Vec<XZPoint>],
    editor: &mut WorldEditor,
    bwf: &BigWaterField,
    road_mask: &RoadMaskBitmap,
    tunnel_footprint: &RoadMaskBitmap,
    still_surface: Option<i32>,
) {
    let edges = PolygonEdges::new(outers, inners);
    // Widened by the interior margin so the interior test reads cached spans too.
    let m = INTERIOR_MARGIN;
    let spans = SpanRows::build(&edges, min_z - m, max_z + m, min_x - m, max_x + m);

    for z in min_z..=max_z {
        for &(span_start, span_end) in spans.get(z) {
            let (start, end) = (span_start.max(min_x), span_end.min(max_x));
            for x in start..=end {
                // Keep road/bridge surfaces (carve would overwrite them).
                if road_mask.contains(x, z) {
                    continue;
                }
                let ground_y = editor.get_ground_level(x, z);
                let water_y = match still_surface {
                    Some(surface) => {
                        // Exposed bank above the body's surface, or terrain so far below it
                        // that the fill would hang a slab over open air: no water.
                        if ground_y > surface || surface - ground_y > MAX_STILL_FILL_DROP {
                            continue;
                        }
                        surface
                    }
                    None => {
                        let water_y = editor.get_water_level(x, z);
                        if ground_y > water_y {
                            // A lower neighbour within the snap radius: a bank the
                            // polygon overclaims (skip), or well inside the polygon a
                            // DEM step that stays water rather than a strip of grass.
                            if !spans.contains_interior(x, z, m) {
                                continue;
                            }
                            ground_y
                        } else {
                            water_y
                        }
                    }
                };
                // Over a bore, fill down to the terrain but never carve into it.
                if tunnel_footprint.contains(x, z) {
                    for y in (ground_y + 1).min(water_y)..=water_y {
                        editor.set_block_absolute(WATER, x, y, z, None, Some(&[]));
                    }
                    continue;
                }
                // depth_at gives the carved depth (0 without land-cover water data).
                carve_water_column(editor, x, z, water_y, bwf.depth_at(x, z), road_mask, bwf);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::floodfill_cache::CoordinateBitmap;
    use std::collections::HashMap as StdMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn ring(id: u64, min: i32, max: i32) -> ProcessedWay {
        let corner = |i: u64, x: i32, z: i32| ProcessedNode {
            id: i,
            tags: StdMap::new(),
            x,
            z,
        };
        let mut tags = StdMap::new();
        tags.insert("natural".to_string(), "water".to_string());
        ProcessedWay {
            id,
            nodes: vec![
                corner(1, min, min),
                corner(2, max, min),
                corner(3, max, max),
                corner(4, min, max),
                corner(1, min, min),
            ],
            tags,
        }
    }

    /// A still surface sits up to `MAX_STILL_FILL_DROP` over the terrain, so laying
    /// only the top block over a bore leaves the body hanging on air.
    #[test]
    fn water_over_a_bore_fills_down_to_the_terrain() {
        let xzbbox = XZBBox::rect_from_xz_lengths(80.0, 80.0).unwrap();
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let ground = crate::ground::Ground::new_elevation_test(vec![vec![0.0f32; 80]; 80], 80, 80);
        let mut editor = WorldEditor::new(PathBuf::from("/dev/null/unused"), &xzbbox, llbbox);
        editor.set_ground(Arc::new(ground.clone()));

        let mut footprint = CoordinateBitmap::new(&xzbbox);
        for x in 30..=40 {
            for z in 30..=40 {
                footprint.set(x, z);
            }
        }
        let bwf = crate::water_depth::compute_big_water_field(&ground, &xzbbox);
        let road_mask = CoordinateBitmap::new_empty();
        let surface = 5;
        let mut surfaces = FnvHashMap::default();
        surfaces.insert(("way", 1u64), surface);

        generate_water_area_from_way(
            &mut editor,
            &ring(1, 20, 60),
            &bwf,
            &road_mask,
            &footprint,
            &StillWaterSurfaces(surfaces),
        );

        for y in 1..=surface {
            assert!(
                editor.check_for_block_absolute(35, y, 35, Some(&[WATER]), None),
                "gap under the surface at y={y}"
            );
        }
        // The bore itself is untouched below the terrain.
        assert!(!editor.block_exists_absolute(35, -1, 35));
    }
}
