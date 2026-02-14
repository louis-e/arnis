use crate::clipping::clip_water_ring_to_bbox;
use crate::{
    block_definitions::WATER,
    coordinate_system::cartesian::{XZBBox, XZPoint},
    osm_parser::{ProcessedMemberRole, ProcessedNode, ProcessedRelation, ProcessedWay},
    world_editor::WorldEditor,
};

pub fn generate_water_area_from_way(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    _xzbbox: &XZBBox,
) {
    let outers = [element.nodes.clone()];
    if !verify_closed_rings(&outers) {
        println!("Skipping way {} due to invalid polygon", element.id);
        return;
    }

    generate_water_areas(editor, &outers, &[]);
}

pub fn generate_water_areas_from_relation(
    editor: &mut WorldEditor,
    element: &ProcessedRelation,
    xzbbox: &XZBBox,
) {
    // Check if this is a water relation (either with water tag or natural=water)
    let is_water = element.tags.contains_key("water")
        || element
            .tags
            .get("natural")
            .map(|val| val == "water" || val == "bay")
            .unwrap_or(false);

    if !is_water {
        return;
    }

    // Don't handle water below layer 0
    if let Some(layer) = element.tags.get("layer") {
        if layer.parse::<i32>().map(|x| x < 0).unwrap_or(false) {
            return;
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
            return;
        }

        // Verify again after filtering and closing
        if !verify_closed_rings(&outers) {
            println!("Skipping relation {} due to invalid polygon", element.id);
            return;
        }
    }

    super::merge_way_segments(&mut inners);
    if !verify_closed_rings(&inners) {
        println!("Skipping relation {} due to invalid polygon", element.id);
        return;
    }

    generate_water_areas(editor, &outers, &inners);
}

fn generate_water_areas(
    editor: &mut WorldEditor,
    outers: &[Vec<ProcessedNode>],
    inners: &[Vec<ProcessedNode>],
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

    scanline_fill_water(min_x, min_z, max_x, max_z, &outers_xz, &inners_xz, editor);
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

/// Fills water blocks using scanline rasterization.
///
/// For each row z in [min_z, max_z], computes which x positions are inside
/// any outer polygon ring but outside all inner polygon rings, and places
/// water blocks at those positions.
#[allow(clippy::too_many_arguments)]
fn scanline_fill_water(
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    outers: &[Vec<XZPoint>],
    inners: &[Vec<XZPoint>],
    editor: &mut WorldEditor,
) {
    // Collect edges per outer ring so we can union their spans correctly,
    // even if multiple outer rings happen to overlap (invalid OSM, but
    // we handle it gracefully).
    let outer_edge_groups: Vec<Vec<ScanlineEdge>> =
        outers.iter().map(|ring| collect_ring_edges(ring)).collect();
    let inner_edges = collect_all_ring_edges(inners);

    for z in min_z..=max_z {
        let z_f = z as f64;

        // Compute spans for each outer ring and union them together
        let mut outer_spans: Vec<(i32, i32)> = Vec::new();
        for ring_edges in &outer_edge_groups {
            let ring_spans = compute_scanline_spans(ring_edges, z_f, min_x, max_x);
            if !ring_spans.is_empty() {
                outer_spans = union_spans(&outer_spans, &ring_spans);
            }
        }
        if outer_spans.is_empty() {
            continue;
        }

        let fill_spans = if inner_edges.is_empty() {
            outer_spans
        } else {
            let inner_spans = compute_scanline_spans(&inner_edges, z_f, min_x, max_x);
            if inner_spans.is_empty() {
                outer_spans
            } else {
                subtract_spans(&outer_spans, &inner_spans)
            }
        };

        for (start, end) in fill_spans {
            for x in start..=end {
                editor.set_block(WATER, x, 0, z, None, None);
            }
        }
    }
}
