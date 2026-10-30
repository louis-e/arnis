//! Force LC_WATER inside OSM water polygons and waterways so OSM defines
//! the shoreline, overriding ESA's noisy 10 m classification.
use rayon::prelude::*;
use std::collections::{HashMap, VecDeque};

use crate::bresenham::bresenham_line;
use crate::clipping::clip_water_ring_to_bbox;
use crate::coordinate_system::cartesian::XZBBox;
use crate::floodfill_cache::hole_groups;
use crate::land_cover::{compute_water_distance, InlandWater, LandCoverData, LC_WATER};
use crate::osm_parser::{
    ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedRelation, ProcessedWay,
};

const ELEVATION_TOLERANCE_BLOCKS: f32 = 1.5;
const PROTECTED_LAND_AREA_M2: f64 = 4000.0;
const MIN_PROTECTED_LAND_CELLS: usize = 100;
const LINEAR_ASPECT_RATIO: f64 = 3.0;

struct WaterContext {
    /// Row-major, `width` per row.
    nearest_y: Vec<f32>,
    protected_mask: Vec<u64>,
    width: usize,
}

pub fn apply_osm_water_override(
    land_cover: &mut LandCoverData,
    heights: &mut [Vec<f32>],
    world_width: usize,
    world_height: usize,
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
) {
    let width = land_cover.width;
    let height = land_cover.height;
    if width < 2 || height < 2 || world_width < 2 || world_height < 2 {
        return;
    }
    if heights.len() < height || heights.iter().take(height).any(|r| r.len() < width) {
        return;
    }
    let scale_to_grid_x = (width as f64 - 1.0) / (world_width as f64 - 1.0);
    let scale_to_grid_z = (height as f64 - 1.0) / (world_height as f64 - 1.0);
    let context = build_water_context(
        &land_cover.grid,
        heights,
        width,
        height,
        scale_to_grid_x,
        scale_to_grid_z,
    );
    let reach = inland_reach(land_cover);
    let (changed, inland) = rasterize_osm_water(
        &mut land_cover.grid,
        heights,
        context.as_ref(),
        (scale_to_grid_x, scale_to_grid_z),
        reach,
        elements,
        xzbbox,
    );
    land_cover.inland_water = inland;

    if changed > 0 {
        eprintln!(
            "OSM water override: reclassified {} cells to LC_WATER inside OSM water polygons/lines",
            changed
        );
        land_cover.water_distance = compute_water_distance(&land_cover.grid, width, height);
        land_cover.invalidate_water_blend_grid();
        // The grid was leveled before this ran, so cells added here sit at their own
        // terrain height and render as a rim above the body. Sink them onto the water
        // they were judged against. ESA water is its own seed and stays put.
        if let Some(nearest_y) = context.as_ref().map(|c| c.nearest_y.as_slice()) {
            let mut sunk = 0usize;
            for gz in 0..height {
                for gx in 0..width {
                    if land_cover.grid[gz][gx] != LC_WATER {
                        continue;
                    }
                    let cell_y = heights[gz][gx];
                    let water_y = nearest_y[gz * width + gx];
                    if cell_y.is_finite()
                        && water_y.is_finite()
                        && cell_y > water_y
                        && cell_y <= water_y + ELEVATION_TOLERANCE_BLOCKS
                    {
                        heights[gz][gx] = water_y;
                        sunk += 1;
                    }
                }
            }
            if sunk > 0 {
                eprintln!(
                    "OSM water override: sank {} overridden cells onto the adjacent water surface",
                    sunk
                );
            }
        }
    }
}

/// Flat worlds keep ESA's water; this only records which of it OSM maps as inland.
pub fn mark_inland_water(
    land_cover: &mut LandCoverData,
    world_width: usize,
    world_height: usize,
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
) {
    let (width, height) = (land_cover.width, land_cover.height);
    if width < 2 || height < 2 || world_width < 2 || world_height < 2 {
        return;
    }
    let scale = (
        (width as f64 - 1.0) / (world_width as f64 - 1.0),
        (height as f64 - 1.0) / (world_height as f64 - 1.0),
    );
    // Without a height context the guard never reads heights.
    let mut scratch = land_cover.grid.clone();
    let reach = inland_reach(land_cover);
    land_cover.inland_water =
        rasterize_osm_water(&mut scratch, &[], None, scale, reach, elements, xzbbox).1;
}

/// How far the inland mark spreads from mapped water, in grid cells.
fn inland_reach(land_cover: &LandCoverData) -> usize {
    const INLAND_REACH_M: f64 = 600.0;
    (INLAND_REACH_M * land_cover.cells_per_meter)
        .round()
        .max(1.0) as usize
}

/// Spreads the inland mark across connected water up to `reach` cells. Wide rivers often
/// come only as a centreline: tidal ones lose their area polygons to the fetch filter.
fn spread_inland(grid: &[Vec<u8>], inland: &mut InlandWater, reach: usize) {
    let height = grid.len();
    let width = grid.first().map_or(0, |r| r.len());
    let mut frontier: Vec<(usize, usize)> = Vec::new();
    for (z, row) in grid.iter().enumerate() {
        for (x, &c) in row.iter().enumerate() {
            if c == LC_WATER && inland.get(x, z) {
                frontier.push((x, z));
            }
        }
    }
    for _ in 0..reach {
        let mut next = Vec::new();
        for &(x, z) in &frontier {
            let around = [
                (x.wrapping_sub(1), z),
                (x + 1, z),
                (x, z.wrapping_sub(1)),
                (x, z + 1),
            ];
            for (nx, nz) in around {
                if nx < width && nz < height && grid[nz][nx] == LC_WATER && !inland.get(nx, nz) {
                    inland.set(nx, nz);
                    next.push((nx, nz));
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
}

/// Burns OSM water polygons and channel waterways into `grid`, returning the cells
/// turned to water and the mask of water mapped as inland, spread `reach` cells.
fn rasterize_osm_water(
    grid: &mut [Vec<u8>],
    heights: &[Vec<f32>],
    context: Option<&WaterContext>,
    (scale_to_grid_x, scale_to_grid_z): (f64, f64),
    reach: usize,
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
) -> (usize, InlandWater) {
    let height = grid.len();
    let width = grid.first().map_or(0, |r| r.len());
    let min_x = xzbbox.min_x();
    let min_z = xzbbox.min_z();
    let max_x = xzbbox.max_x();
    let max_z = xzbbox.max_z();
    let mut inland = InlandWater::new(width, height);
    let mut changed: usize = 0;
    for elem in elements {
        match elem {
            ProcessedElement::Way(way) => {
                if is_water_polygon_way(way) {
                    if !is_ring_closed(&way.nodes) {
                        continue;
                    }
                    // Clip to the cell bbox first; a ring spilling past the cell would clamp-flood a wedge.
                    let clipped = match clip_water_ring_to_bbox(&way.nodes, xzbbox) {
                        Some(r) => r,
                        None => continue,
                    };
                    let outer: Vec<(i32, i32)> = clipped.iter().map(|n| (n.x, n.z)).collect();
                    changed += fill_polygon_scanline(
                        grid,
                        &[outer.as_slice()],
                        &[],
                        heights,
                        context,
                        min_x,
                        min_z,
                        max_x,
                        max_z,
                        width,
                        height,
                        scale_to_grid_x,
                        scale_to_grid_z,
                        is_inland_water(&way.tags).then_some(&mut inland),
                    );
                } else if let Some(waterway_type) = way.tags.get("waterway") {
                    if !crate::element_processing::waterways::is_channel_waterway(waterway_type)
                        || crate::element_processing::waterways::is_underground_waterway(&way.tags)
                    {
                        continue;
                    }
                    let waterway_width = crate::element_processing::waterways::waterway_width(
                        waterway_type,
                        &way.tags,
                    );
                    let half_width = waterway_width / 2;
                    changed += rasterize_line(
                        grid,
                        &way.nodes,
                        half_width,
                        heights,
                        context,
                        min_x,
                        min_z,
                        max_x,
                        max_z,
                        width,
                        height,
                        scale_to_grid_x,
                        scale_to_grid_z,
                        Some(&mut inland),
                    );
                }
            }
            ProcessedElement::Relation(rel) => {
                if !is_water_relation(rel) {
                    continue;
                }
                let mut outer_nodes: Vec<Vec<ProcessedNode>> = Vec::new();
                let mut inner_nodes: Vec<Vec<ProcessedNode>> = Vec::new();
                for member in &rel.members {
                    if member.way.nodes.len() < 2 {
                        continue;
                    }
                    match member.role {
                        ProcessedMemberRole::Outer => outer_nodes.push(member.way.nodes.clone()),
                        ProcessedMemberRole::Inner => inner_nodes.push(member.way.nodes.clone()),
                        _ => {}
                    }
                }
                // Members are often fragments; stitch into closed rings.
                crate::element_processing::merge_way_segments(&mut outer_nodes);
                crate::element_processing::merge_way_segments(&mut inner_nodes);
                outer_nodes.retain(|ring| is_ring_closed(ring));
                inner_nodes.retain(|ring| is_ring_closed(ring));
                // Clip assembled rings before filling; unclipped crossings past the edge clamp-flood a corner wedge.
                outer_nodes = outer_nodes
                    .into_iter()
                    .filter_map(|ring| clip_water_ring_to_bbox(&ring, xzbbox))
                    .collect();
                inner_nodes = inner_nodes
                    .into_iter()
                    .filter_map(|ring| clip_water_ring_to_bbox(&ring, xzbbox))
                    .collect();
                if outer_nodes.is_empty() {
                    continue;
                }
                let outers: Vec<Vec<(i32, i32)>> = outer_nodes
                    .iter()
                    .map(|ring| ring.iter().map(|n| (n.x, n.z)).collect())
                    .collect();
                let inners: Vec<Vec<(i32, i32)>> = inner_nodes
                    .iter()
                    .map(|ring| ring.iter().map(|n| (n.x, n.z)).collect())
                    .collect();
                let rel_inland = is_inland_water(&rel.tags);
                // Each group on its own, so a pond on an island in a hole stays water.
                for (members, cutting) in hole_groups(&outers, &inners, |&p| p) {
                    let outers_refs: Vec<&[(i32, i32)]> =
                        members.iter().map(|&o| outers[o].as_slice()).collect();
                    let inners_refs: Vec<&[(i32, i32)]> =
                        cutting.iter().map(|&i| inners[i].as_slice()).collect();
                    changed += fill_polygon_scanline(
                        grid,
                        &outers_refs,
                        &inners_refs,
                        heights,
                        context,
                        min_x,
                        min_z,
                        max_x,
                        max_z,
                        width,
                        height,
                        scale_to_grid_x,
                        scale_to_grid_z,
                        rel_inland.then_some(&mut inland),
                    );
                }
            }
            _ => {}
        }
    }
    spread_inland(grid, &mut inland, reach);
    (changed, inland)
}

fn is_water_polygon_way(way: &ProcessedWay) -> bool {
    if way.nodes.len() < 3 {
        return false;
    }
    has_water_polygon_tags(&way.tags)
}

fn is_water_relation(rel: &ProcessedRelation) -> bool {
    let tags = &rel.tags;
    matches!(
        tags.get("natural").map(|s| s.as_str()),
        Some("water" | "bay")
    ) || has_explicit_water_tag(tags)
}

fn has_water_polygon_tags(tags: &HashMap<String, String>) -> bool {
    matches!(tags.get("natural").map(|s| s.as_str()), Some("water"))
        || has_explicit_water_tag(tags)
        || matches!(tags.get("landuse").map(|s| s.as_str()), Some("reservoir"))
        || matches!(
            tags.get("waterway").map(|s| s.as_str()),
            Some("dock" | "riverbank")
        )
}

// Bays and lagoons open onto the sea, so they keep its biome.
fn is_inland_water(tags: &HashMap<String, String>) -> bool {
    !matches!(tags.get("natural").map(|s| s.as_str()), Some("bay"))
        && !matches!(
            tags.get("water").map(|s| s.as_str()),
            Some("lagoon" | "bay" | "sea")
        )
}

// `water=no` and similar negatives must not count as water.
fn has_explicit_water_tag(tags: &HashMap<String, String>) -> bool {
    tags.get("water")
        .is_some_and(|v| !matches!(v.as_str(), "no" | "0" | "false"))
}

fn is_ring_closed(nodes: &[ProcessedNode]) -> bool {
    if nodes.len() < 3 {
        return false;
    }
    let first = &nodes[0];
    let last = nodes.last().unwrap();
    if first.id == last.id {
        return true;
    }
    let dx = (first.x - last.x).abs();
    let dz = (first.z - last.z).abs();
    dx <= 1 && dz <= 1
}

#[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
fn fill_polygon_scanline(
    grid: &mut [Vec<u8>],
    outers: &[&[(i32, i32)]],
    inners: &[&[(i32, i32)]],
    heights: &[Vec<f32>],
    context: Option<&WaterContext>,
    min_x_world: i32,
    min_z_world: i32,
    max_x_world: i32,
    max_z_world: i32,
    width: usize,
    height: usize,
    scale_to_grid_x: f64,
    scale_to_grid_z: f64,
    mut inland: Option<&mut InlandWater>,
) -> usize {
    if outers.is_empty() || width < 2 || height < 2 {
        return 0;
    }

    let mut p_min_z = i32::MAX;
    let mut p_max_z = i32::MIN;
    for ring in outers {
        for &(_, z) in *ring {
            p_min_z = p_min_z.min(z);
            p_max_z = p_max_z.max(z);
        }
    }
    if p_min_z == i32::MAX {
        return 0;
    }
    if p_max_z < min_z_world || p_min_z > max_z_world {
        return 0;
    }

    let scale_inv_z = if scale_to_grid_z > 0.0 {
        1.0 / scale_to_grid_z
    } else {
        return 0;
    };
    let scale_inv_x = if scale_to_grid_x > 0.0 {
        1.0 / scale_to_grid_x
    } else {
        return 0;
    };

    // Iterate grid rows intersected with the polygon's z range only.
    let p_min_gz = ((p_min_z - min_z_world).max(0) as f64 * scale_to_grid_z).floor() as i32;
    let p_max_gz = ((p_max_z - min_z_world).max(0) as f64 * scale_to_grid_z).ceil() as i32;
    let gz_start = p_min_gz.max(0) as usize;
    let gz_end = (p_max_gz.min(height as i32 - 1)).max(0) as usize;

    let mut outer_x_world: Vec<f64> = Vec::new();
    let mut inner_x_world: Vec<f64> = Vec::new();
    let mut count = 0;

    for gz in gz_start..=gz_end {
        let world_z = gz as f64 * scale_inv_z + min_z_world as f64;

        outer_x_world.clear();
        for ring in outers {
            edge_crossings_at_z(ring, world_z, &mut outer_x_world);
        }
        if outer_x_world.len() < 2 {
            continue;
        }
        outer_x_world.sort_by(|a, b| a.partial_cmp(b).unwrap());

        inner_x_world.clear();
        for ring in inners {
            edge_crossings_at_z(ring, world_z, &mut inner_x_world);
        }
        if !inner_x_world.is_empty() {
            inner_x_world.sort_by(|a, b| a.partial_cmp(b).unwrap());
        }

        count += fill_row_spans(
            &mut grid[gz],
            &outer_x_world,
            &inner_x_world,
            heights,
            context,
            gz,
            min_x_world,
            max_x_world,
            width,
            scale_to_grid_x,
            scale_inv_x,
            inland.as_deref_mut(),
        );
    }
    count
}

#[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
fn fill_row_spans(
    row: &mut [u8],
    outer_x: &[f64],
    inner_x: &[f64],
    heights: &[Vec<f32>],
    context: Option<&WaterContext>,
    gz: usize,
    min_x_world: i32,
    max_x_world: i32,
    width: usize,
    scale_to_grid_x: f64,
    scale_inv_x: f64,
    mut inland: Option<&mut InlandWater>,
) -> usize {
    // Cyclic rings always yield an even crossing count; pairing ignores any trailing odd one.
    let mut count = 0;
    let mut i = 0;
    while i + 1 < outer_x.len() {
        let wx_start = outer_x[i];
        let wx_end = outer_x[i + 1];
        i += 2;

        let wx_start_clipped = wx_start.max(min_x_world as f64);
        let wx_end_clipped = wx_end.min(max_x_world as f64);
        if wx_start_clipped > wx_end_clipped {
            continue;
        }

        let gx_start = ((wx_start_clipped - min_x_world as f64) * scale_to_grid_x).ceil() as i32;
        let gx_end = ((wx_end_clipped - min_x_world as f64) * scale_to_grid_x).floor() as i32;
        let gx_start = gx_start.max(0) as usize;
        let gx_end = (gx_end.min(width as i32 - 1)).max(0) as usize;
        if gx_start > gx_end {
            continue;
        }

        for gx in gx_start..=gx_end {
            if !inner_x.is_empty() {
                let wx = gx as f64 * scale_inv_x + min_x_world as f64;
                if point_in_sorted_ranges(wx, inner_x) {
                    continue;
                }
            }
            if let Some(m) = inland.as_deref_mut() {
                m.set(gx, gz);
            }
            if !passes_water_guard(heights, context, gx, gz) {
                continue;
            }
            if row[gx] != LC_WATER {
                row[gx] = LC_WATER;
                count += 1;
            }
        }
    }
    count
}

fn edge_crossings_at_z(ring: &[(i32, i32)], z: f64, out: &mut Vec<f64>) {
    let n = ring.len();
    if n < 3 {
        return;
    }
    let mut j = n - 1;
    for i in 0..n {
        let zi = ring[i].1 as f64;
        let zj = ring[j].1 as f64;
        if (zi > z) != (zj > z) {
            let xi = ring[i].0 as f64;
            let xj = ring[j].0 as f64;
            let t = (z - zi) / (zj - zi);
            out.push(xi + (xj - xi) * t);
        }
        j = i;
    }
}

fn point_in_sorted_ranges(x: f64, sorted: &[f64]) -> bool {
    let mut i = 0;
    while i + 1 < sorted.len() {
        if x > sorted[i] && x < sorted[i + 1] {
            return true;
        }
        i += 2;
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn rasterize_line(
    grid: &mut [Vec<u8>],
    nodes: &[ProcessedNode],
    half_width: i32,
    heights: &[Vec<f32>],
    context: Option<&WaterContext>,
    min_x_world: i32,
    min_z_world: i32,
    max_x_world: i32,
    max_z_world: i32,
    width: usize,
    height: usize,
    scale_to_grid_x: f64,
    scale_to_grid_z: f64,
    mut inland: Option<&mut InlandWater>,
) -> usize {
    if nodes.len() < 2 || half_width < 0 {
        return 0;
    }
    let width_i32 = width as i32;
    let height_i32 = height as i32;
    let margin = half_width.max(0);
    let mut count = 0;
    for pair in nodes.windows(2) {
        let (x1, z1) = (pair[0].x, pair[0].z);
        let (x2, z2) = (pair[1].x, pair[1].z);
        // Skip segments whose bbox+margin doesn't intersect the world bbox.
        let seg_min_x = x1.min(x2) - margin;
        let seg_max_x = x1.max(x2) + margin;
        let seg_min_z = z1.min(z2) - margin;
        let seg_max_z = z1.max(z2) + margin;
        if seg_max_x < min_x_world
            || seg_min_x > max_x_world
            || seg_max_z < min_z_world
            || seg_min_z > max_z_world
        {
            continue;
        }
        for (bx, _, bz) in bresenham_line(x1, 0, z1, x2, 0, z2) {
            for dx in -half_width..=half_width {
                for dz in -half_width..=half_width {
                    let wx = bx + dx;
                    let wz = bz + dz;
                    let rel_x = wx - min_x_world;
                    let rel_z = wz - min_z_world;
                    if rel_x < 0 || rel_z < 0 {
                        continue;
                    }
                    let gx = (rel_x as f64 * scale_to_grid_x).round() as i32;
                    let gz = (rel_z as f64 * scale_to_grid_z).round() as i32;
                    if gx < 0 || gz < 0 || gx >= width_i32 || gz >= height_i32 {
                        continue;
                    }
                    let gx_u = gx as usize;
                    let gz_u = gz as usize;
                    if let Some(m) = inland.as_deref_mut() {
                        m.set(gx_u, gz_u);
                    }
                    if !passes_water_guard(heights, context, gx_u, gz_u) {
                        continue;
                    }
                    if grid[gz_u][gx_u] != LC_WATER {
                        grid[gz_u][gx_u] = LC_WATER;
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

// Returns None when there are no ESA water cells, disabling the guard.
fn build_water_context(
    grid: &[Vec<u8>],
    heights: &[Vec<f32>],
    width: usize,
    height: usize,
    scale_to_grid_x: f64,
    scale_to_grid_z: f64,
) -> Option<WaterContext> {
    if width < 2 || height < 2 {
        return None;
    }
    if heights.len() < height || heights.first().map_or(0, |r| r.len()) < width {
        return None;
    }
    if grid.len() < height || grid.first().map_or(0, |r| r.len()) < width {
        return None;
    }
    let has_seeds = grid
        .iter()
        .take(height)
        .any(|row| row.iter().take(width).any(|&c| c == LC_WATER));
    if !has_seeds {
        return None;
    }

    let cells_per_meter_sq = scale_to_grid_x * scale_to_grid_z;
    let raw = (PROTECTED_LAND_AREA_M2 * cells_per_meter_sq).round() as usize;
    let small_threshold_cells = raw.max(MIN_PROTECTED_LAND_CELLS);

    let protected_mask = build_protected_land_bitset(
        grid,
        width,
        height,
        small_threshold_cells,
        LINEAR_ASPECT_RATIO,
    );
    let nearest_y = compute_nearest_water_y(grid, heights, width, height);

    Some(WaterContext {
        nearest_y,
        protected_mask,
        width,
    })
}

/// Bits on every cell of a 4-connected land component at least `small_threshold_cells`
/// large and no more elongated than `linear_aspect_ratio`. Components are joined from the
/// land runs of each row where runs on neighbouring rows share a column, so only the runs
/// are walked, not every cell.
fn build_protected_land_bitset(
    grid: &[Vec<u8>],
    width: usize,
    height: usize,
    small_threshold_cells: usize,
    linear_aspect_ratio: f64,
) -> Vec<u64> {
    let mut mask: Vec<u64> = vec![0; (width * height).div_ceil(64)];
    // [start, end) of each land run, per row.
    let rows: Vec<Vec<(u32, u32)>> = grid[..height]
        .par_iter()
        .map(|row| {
            let mut runs = Vec::new();
            let mut x = 0;
            while x < width {
                if row[x] == LC_WATER {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < width && row[x] != LC_WATER {
                    x += 1;
                }
                runs.push((start as u32, x as u32));
            }
            runs
        })
        .collect();
    let mut first_run = Vec::with_capacity(height + 1);
    first_run.push(0usize);
    for runs in &rows {
        first_run.push(first_run.last().unwrap() + runs.len());
    }

    fn root(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            parent[i as usize] = parent[parent[i as usize] as usize];
            i = parent[i as usize];
        }
        i
    }
    let mut parent: Vec<u32> = (0..first_run[height] as u32).collect();
    for z in 1..height {
        let (above, here) = (&rows[z - 1], &rows[z]);
        let (mut a, mut b) = (0, 0);
        while a < above.len() && b < here.len() {
            let ((a0, a1), (b0, b1)) = (above[a], here[b]);
            if a0 < b1 && b0 < a1 {
                let ra = root(&mut parent, (first_run[z - 1] + a) as u32);
                let rb = root(&mut parent, (first_run[z] + b) as u32);
                parent[ra.max(rb) as usize] = ra.min(rb);
            }
            if a1 < b1 {
                a += 1;
            } else {
                b += 1;
            }
        }
    }

    // Size and bounding box (min_x, max_x, min_z, max_z) per component root.
    let mut size = vec![0usize; parent.len()];
    let mut bounds = vec![(u32::MAX, 0u32, u32::MAX, 0u32); parent.len()];
    for (z, runs) in rows.iter().enumerate() {
        for (i, &(x0, x1)) in runs.iter().enumerate() {
            let r = root(&mut parent, (first_run[z] + i) as u32) as usize;
            size[r] += (x1 - x0) as usize;
            let b = &mut bounds[r];
            *b = (
                b.0.min(x0),
                b.1.max(x1 - 1),
                b.2.min(z as u32),
                b.3.max(z as u32),
            );
        }
    }
    for (z, runs) in rows.iter().enumerate() {
        for (i, &(x0, x1)) in runs.iter().enumerate() {
            let r = root(&mut parent, (first_run[z] + i) as u32) as usize;
            let (min_x, max_x, min_z, max_z) = bounds[r];
            let (bbox_w, bbox_h) = ((max_x - min_x + 1) as usize, (max_z - min_z + 1) as usize);
            let aspect = bbox_w.max(bbox_h) as f64 / bbox_w.min(bbox_h).max(1) as f64;
            if size[r] >= small_threshold_cells && aspect <= linear_aspect_ratio {
                for idx in z * width + x0 as usize..z * width + x1 as usize {
                    set_bit(&mut mask, idx);
                }
            }
        }
    }
    mask
}

// Multi-source BFS: each cell gets the terrain Y of its nearest LC_WATER seed.
fn compute_nearest_water_y(
    grid: &[Vec<u8>],
    heights: &[Vec<f32>],
    width: usize,
    height: usize,
) -> Vec<f32> {
    let mut result: Vec<f32> = vec![f32::NAN; width * height];
    let mut queue: VecDeque<u32> = VecDeque::new();
    for z in 0..height {
        let (row, h_row) = (&grid[z], &heights[z]);
        for x in 0..width {
            if row[x] == LC_WATER && h_row[x].is_finite() {
                result[z * width + x] = h_row[x];
                queue.push_back((z * width + x) as u32);
            }
        }
    }
    while let Some(i) = queue.pop_front() {
        let i = i as usize;
        let (x, cell_y) = (i % width, result[i]);
        // Same neighbour order as before: -x, +x, -z, +z.
        let neighbours = [
            (x > 0).then(|| i - 1),
            (x + 1 < width).then(|| i + 1),
            i.checked_sub(width),
            (i + width < width * height).then(|| i + width),
        ];
        for n in neighbours.into_iter().flatten() {
            if result[n].is_nan() {
                result[n] = cell_y;
                queue.push_back(n as u32);
            }
        }
    }
    result
}

#[inline]
fn passes_water_guard(
    heights: &[Vec<f32>],
    context: Option<&WaterContext>,
    gx: usize,
    gz: usize,
) -> bool {
    let Some(c) = context else {
        return true;
    };
    let idx = gz * c.width + gx;
    if get_bit(&c.protected_mask, idx) {
        return false;
    }
    let cell_y = heights[gz][gx];
    let water_y = c.nearest_y[idx];
    if !cell_y.is_finite() || !water_y.is_finite() {
        return true;
    }
    cell_y <= water_y + ELEVATION_TOLERANCE_BLOCKS
}

#[inline(always)]
fn get_bit(mask: &[u64], idx: usize) -> bool {
    (mask[idx >> 6] >> (idx & 63)) & 1 != 0
}

#[inline(always)]
fn set_bit(mask: &mut [u64], idx: usize) {
    mask[idx >> 6] |= 1u64 << (idx & 63);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u64, x: i32, z: i32) -> ProcessedNode {
        ProcessedNode {
            id,
            tags: HashMap::new(),
            x,
            z,
        }
    }

    fn water_way(id: u64, tags: &[(&str, &str)], nodes: Vec<ProcessedNode>) -> ProcessedElement {
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    fn square(x0: i32, z0: i32, x1: i32, z1: i32) -> Vec<ProcessedNode> {
        vec![
            node(1, x0, z0),
            node(2, x1, z0),
            node(3, x1, z1),
            node(4, x0, z1),
            node(1, x0, z0),
        ]
    }

    #[test]
    fn lakes_and_waterways_are_inland_bays_are_sea() {
        let bbox = XZBBox::rect_from_min_max(0, 0, 31, 31).unwrap();
        // Land splits the water into quarters, so the mark can't cross between them.
        let mut grid = vec![vec![LC_WATER; 32]; 32];
        for row in grid.iter_mut() {
            row[15] = 0;
        }
        grid[15].fill(0);
        let mut lc = LandCoverData {
            water_distance: compute_water_distance(&grid, 32, 32),
            grid,
            water_blend_cache: Default::default(),
            beach_tiles: Default::default(),
            inland_water: Default::default(),
            width: 32,
            height: 32,
            // A 600 m reach is 3 cells.
            cells_per_meter: 0.005,
        };
        let elements = [
            water_way(1, &[("natural", "water")], square(2, 2, 10, 10)),
            water_way(2, &[("natural", "bay")], square(20, 2, 28, 10)),
            water_way(
                3,
                &[("waterway", "river"), ("width", "2")],
                vec![node(5, 20, 20), node(6, 28, 20)],
            ),
        ];
        let before = lc.grid.clone();
        mark_inland_water(&mut lc, 32, 32, &elements, &bbox);
        assert!(lc.inland_water.get(5, 5));
        // The lake's mark spreads three steps, the bay's water stays sea.
        assert!(lc.inland_water.get(13, 5));
        assert!(!lc.inland_water.get(14, 14));
        assert!(!lc.inland_water.get(24, 5));
        // A river centreline marks the water around it, up to the reach.
        assert!(lc.inland_water.get(24, 23));
        assert!(!lc.inland_water.get(24, 28));
        assert!(!lc.inland_water.get(5, 24));
        // Flat worlds keep their water as it was.
        assert!(lc.grid == before);
    }

    #[test]
    fn straddling_ring_fills_only_bbox_intersection() {
        let bbox = XZBBox::rect_from_min_max(0, 0, 15, 15).unwrap();
        // 16x16 grid over 16x16 world: scale factors are exactly 1.
        let mut grid = vec![vec![0u8; 16]; 16];
        let heights = vec![vec![0.0f32; 16]; 16];
        let ring = vec![
            node(1, 8, 4),
            node(2, 30, 4),
            node(3, 30, 11),
            node(4, 8, 11),
            node(1, 8, 4),
        ];
        let clipped = clip_water_ring_to_bbox(&ring, &bbox).unwrap();
        let outer: Vec<(i32, i32)> = clipped.iter().map(|n| (n.x, n.z)).collect();
        let n = fill_polygon_scanline(
            &mut grid,
            &[outer.as_slice()],
            &[],
            &heights,
            None,
            0,
            0,
            15,
            15,
            16,
            16,
            1.0,
            1.0,
            None,
        );
        assert!(n > 0);
        assert_eq!(grid[7][10], LC_WATER);
        for (gz, row) in grid.iter().enumerate() {
            for (gx, &cell) in row.iter().enumerate() {
                if cell == LC_WATER {
                    assert!((8..=15).contains(&gx) && (4..=11).contains(&gz));
                }
            }
        }
    }

    #[test]
    fn crossing_pair_fills_span() {
        let heights = vec![vec![0.0f32; 16]];
        let mut row = vec![0u8; 16];
        let n = fill_row_spans(
            &mut row,
            &[2.0, 5.0],
            &[],
            &heights,
            None,
            0,
            0,
            15,
            16,
            1.0,
            1.0,
            None,
        );
        assert!(n > 0);
        assert_eq!(row[3], LC_WATER);
        assert_ne!(row[8], LC_WATER);
    }

    /// Water blobs and ribbons on land, so land splits into islands, strips and one big body.
    fn patchy(w: usize, h: usize, seed: u64) -> Vec<Vec<u8>> {
        let mut s = seed;
        let mut grid = vec![vec![0u8; w]; h];
        for _ in 0..(w * h / 40) {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let (x, z, r) = (
                (s >> 20) as usize % w,
                (s >> 40) as usize % h,
                (s >> 62) as usize,
            );
            for row in grid.iter_mut().skip(z.saturating_sub(r)).take(2 * r + 1) {
                for c in row.iter_mut().skip(x.saturating_sub(r)).take(2 * r + 1) {
                    *c = LC_WATER;
                }
            }
        }
        for (z, row) in grid.iter_mut().enumerate() {
            row[(z * 3 / 2) % w] = LC_WATER;
            if z == h / 3 {
                row.fill(LC_WATER);
            }
        }
        grid
    }

    #[test]
    fn protected_land_matches_a_flood_fill() {
        let (mut kept, mut open) = (0, 0);
        for (w, h, seed) in [(70, 50, 1), (33, 90, 2), (128, 64, 3)] {
            let grid = patchy(w, h, seed);
            for (small, ratio) in [(1, 100.0), (20, 3.0), (200, 2.0), (2000, 3.0)] {
                let got = build_protected_land_bitset(&grid, w, h, small, ratio);
                let mut want = vec![false; w * h];
                let mut seen = vec![false; w * h];
                for start in 0..w * h {
                    if seen[start] || grid[start / w][start % w] == LC_WATER {
                        continue;
                    }
                    let (mut cells, mut stack) = (Vec::new(), vec![start]);
                    seen[start] = true;
                    while let Some(i) = stack.pop() {
                        cells.push(i);
                        let (x, z) = (i % w, i / w);
                        let near = [
                            (x > 0).then(|| i - 1),
                            (x + 1 < w).then(|| i + 1),
                            (z > 0).then(|| i - w),
                            (z + 1 < h).then(|| i + w),
                        ];
                        for n in near.into_iter().flatten() {
                            if !seen[n] && grid[n / w][n % w] != LC_WATER {
                                seen[n] = true;
                                stack.push(n);
                            }
                        }
                    }
                    let xs = cells.iter().map(|i| i % w);
                    let zs = cells.iter().map(|i| i / w);
                    let bw = xs.clone().max().unwrap() - xs.min().unwrap() + 1;
                    let bh = zs.clone().max().unwrap() - zs.min().unwrap() + 1;
                    if cells.len() >= small && bw.max(bh) as f64 / bw.min(bh) as f64 <= ratio {
                        for &i in &cells {
                            want[i] = true;
                        }
                    }
                }
                for (i, &protected) in want.iter().enumerate() {
                    assert_eq!(
                        get_bit(&got, i),
                        protected,
                        "cell {i} of {w}x{h}, {small}/{ratio}"
                    );
                    let land = grid[i / w][i % w] != LC_WATER;
                    kept += usize::from(protected);
                    open += usize::from(land && !protected);
                }
            }
        }
        assert!(
            kept > 0 && open > 0,
            "some land is protected and some is not"
        );
    }

    #[test]
    fn nearest_water_takes_the_first_seed_to_arrive() {
        let (w, h) = (40, 30);
        let grid = patchy(w, h, 5);
        let heights: Vec<Vec<f32>> = (0..h)
            .map(|z| {
                (0..w)
                    .map(|x| {
                        if (x + z) % 17 == 0 {
                            f32::NAN
                        } else {
                            (x * 3 + z * 7) as f32
                        }
                    })
                    .collect()
            })
            .collect();
        // The grid-of-rows BFS this replaced.
        let mut want = vec![vec![f32::NAN; w]; h];
        let mut queue = VecDeque::new();
        for z in 0..h {
            for x in 0..w {
                if grid[z][x] == LC_WATER && heights[z][x].is_finite() {
                    want[z][x] = heights[z][x];
                    queue.push_back((x as i32, z as i32));
                }
            }
        }
        while let Some((x, z)) = queue.pop_front() {
            let y = want[z as usize][x as usize];
            for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let (nx, nz) = (x + dx, z + dz);
                let inside = nx >= 0 && nz >= 0 && nx < w as i32 && nz < h as i32;
                if inside && want[nz as usize][nx as usize].is_nan() {
                    want[nz as usize][nx as usize] = y;
                    queue.push_back((nx, nz));
                }
            }
        }
        let got = compute_nearest_water_y(&grid, &heights, w, h);
        for (i, v) in want.iter().flatten().enumerate() {
            assert_eq!(got[i].to_bits(), v.to_bits(), "cell {i}");
        }
    }
}
