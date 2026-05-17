//! Force LC_WATER inside OSM water polygons and waterways so OSM defines
//! the shoreline, overriding ESA's noisy 10 m classification.
use std::collections::{HashMap, VecDeque};

use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZBBox;
use crate::land_cover::{compute_water_distance, LandCoverData, LC_WATER};
use crate::osm_parser::{
    ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedRelation, ProcessedWay,
};

const ELEVATION_TOLERANCE_BLOCKS: f32 = 1.5;
const PROTECTED_LAND_AREA_M2: f64 = 4000.0;
const MIN_PROTECTED_LAND_CELLS: usize = 100;
const LINEAR_ASPECT_RATIO: f64 = 3.0;
// Above this grid size the nearest-water f32 grid is dropped to cap memory.
const MAX_CELLS_FOR_ELEV_GUARD: usize = 100 * 1024 * 1024;

struct WaterContext {
    nearest_y: Option<Vec<Vec<f32>>>,
    protected_mask: Vec<u64>,
    width: usize,
}

pub fn apply_osm_water_override(
    land_cover: &mut LandCoverData,
    heights: &[Vec<f32>],
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
    let scale_to_grid_x = (width as f64 - 1.0) / (world_width as f64 - 1.0);
    let scale_to_grid_z = (height as f64 - 1.0) / (world_height as f64 - 1.0);
    let min_x = xzbbox.min_x();
    let min_z = xzbbox.min_z();
    let max_x = xzbbox.max_x();
    let max_z = xzbbox.max_z();

    let context = build_water_context(
        &land_cover.grid,
        heights,
        width,
        height,
        scale_to_grid_x,
        scale_to_grid_z,
    );

    let mut changed: usize = 0;
    for elem in elements {
        match elem {
            ProcessedElement::Way(way) => {
                if is_water_polygon_way(way) {
                    if !is_ring_closed(&way.nodes) {
                        continue;
                    }
                    let outer: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
                    changed += fill_polygon_scanline(
                        &mut land_cover.grid,
                        &[outer.as_slice()],
                        &[],
                        heights,
                        context.as_ref(),
                        min_x,
                        min_z,
                        max_x,
                        max_z,
                        width,
                        height,
                        scale_to_grid_x,
                        scale_to_grid_z,
                    );
                } else if let Some(waterway_type) = way.tags.get("waterway") {
                    let waterway_width = get_waterway_width(waterway_type, &way.tags);
                    let half_width = waterway_width / 2;
                    changed += rasterize_line(
                        &mut land_cover.grid,
                        &way.nodes,
                        half_width,
                        heights,
                        context.as_ref(),
                        min_x,
                        min_z,
                        max_x,
                        max_z,
                        width,
                        height,
                        scale_to_grid_x,
                        scale_to_grid_z,
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
                let outers_refs: Vec<&[(i32, i32)]> = outers.iter().map(|v| v.as_slice()).collect();
                let inners_refs: Vec<&[(i32, i32)]> = inners.iter().map(|v| v.as_slice()).collect();
                changed += fill_polygon_scanline(
                    &mut land_cover.grid,
                    &outers_refs,
                    &inners_refs,
                    heights,
                    context.as_ref(),
                    min_x,
                    min_z,
                    max_x,
                    max_z,
                    width,
                    height,
                    scale_to_grid_x,
                    scale_to_grid_z,
                );
            }
            _ => {}
        }
    }

    if changed > 0 {
        eprintln!(
            "OSM water override: reclassified {} cells to LC_WATER inside OSM water polygons/lines",
            changed
        );
        land_cover.water_distance = compute_water_distance(&land_cover.grid, width, height);
        land_cover.refresh_water_blend_grid();
    }
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

fn get_waterway_width(waterway_type: &str, tags: &HashMap<String, String>) -> i32 {
    if let Some(w) = tags.get("width").and_then(|s| {
        s.parse::<f32>()
            .ok()
            .or_else(|| s.parse::<i32>().ok().map(|i| i as f32))
    }) {
        return w.round() as i32;
    }
    match waterway_type {
        "river" => 8,
        "canal" => 6,
        "stream" => 3,
        "fairway" => 12,
        "flowline" => 2,
        "brook" => 2,
        "ditch" => 2,
        "drain" => 1,
        _ => 4,
    }
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

        let row = &mut grid[gz];
        let mut i = 0;
        while i + 1 < outer_x_world.len() {
            let wx_start = outer_x_world[i];
            let wx_end = outer_x_world[i + 1];
            i += 2;

            let wx_start_clipped = wx_start.max(min_x_world as f64);
            let wx_end_clipped = wx_end.min(max_x_world as f64);
            if wx_start_clipped > wx_end_clipped {
                continue;
            }

            let gx_start =
                ((wx_start_clipped - min_x_world as f64) * scale_to_grid_x).ceil() as i32;
            let gx_end = ((wx_end_clipped - min_x_world as f64) * scale_to_grid_x).floor() as i32;
            let gx_start = gx_start.max(0) as usize;
            let gx_end = (gx_end.min(width as i32 - 1)).max(0) as usize;
            if gx_start > gx_end {
                continue;
            }

            for gx in gx_start..=gx_end {
                if !inner_x_world.is_empty() {
                    let wx = gx as f64 * scale_inv_x + min_x_world as f64;
                    if point_in_sorted_ranges(wx, &inner_x_world) {
                        continue;
                    }
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
    let nearest_y = if width.saturating_mul(height) <= MAX_CELLS_FOR_ELEV_GUARD {
        Some(compute_nearest_water_y(grid, heights, width, height))
    } else {
        None
    };

    Some(WaterContext {
        nearest_y,
        protected_mask,
        width,
    })
}

// Two-pass: classify components without storing cells, then re-walk protected seeds.
fn build_protected_land_bitset(
    grid: &[Vec<u8>],
    width: usize,
    height: usize,
    small_threshold_cells: usize,
    linear_aspect_ratio: f64,
) -> Vec<u64> {
    let n = width * height;
    let mut mask: Vec<u64> = vec![0; n.div_ceil(64)];
    let mut stack: Vec<u32> = Vec::new();
    let mut protected_seeds: Vec<u32> = Vec::new();
    let width_i32 = width as i32;
    let height_i32 = height as i32;

    // Pass 1: classify each component, remember protected seeds.
    for start_z in 0..height {
        for start_x in 0..width {
            if grid[start_z][start_x] == LC_WATER {
                continue;
            }
            let start_idx = start_z * width + start_x;
            if get_bit(&mask, start_idx) {
                continue;
            }

            stack.clear();
            stack.push(start_idx as u32);
            set_bit(&mut mask, start_idx);

            let mut min_x = start_x;
            let mut max_x = start_x;
            let mut min_z = start_z;
            let mut max_z = start_z;
            let mut size: usize = 0;

            while let Some(curr) = stack.pop() {
                let curr_u = curr as usize;
                let cx = (curr_u % width) as i32;
                let cz = (curr_u / width) as i32;
                size += 1;
                let cxu = cx as usize;
                let czu = cz as usize;
                if cxu < min_x {
                    min_x = cxu;
                }
                if cxu > max_x {
                    max_x = cxu;
                }
                if czu < min_z {
                    min_z = czu;
                }
                if czu > max_z {
                    max_z = czu;
                }
                for (dx, dz) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let nx = cx + dx;
                    let nz = cz + dz;
                    if nx < 0 || nz < 0 || nx >= width_i32 || nz >= height_i32 {
                        continue;
                    }
                    let nxu = nx as usize;
                    let nzu = nz as usize;
                    if grid[nzu][nxu] == LC_WATER {
                        continue;
                    }
                    let n_idx = nzu * width + nxu;
                    if get_bit(&mask, n_idx) {
                        continue;
                    }
                    set_bit(&mut mask, n_idx);
                    stack.push(n_idx as u32);
                }
            }

            let bbox_w = max_x - min_x + 1;
            let bbox_h = max_z - min_z + 1;
            let long_axis = bbox_w.max(bbox_h);
            let short_axis = bbox_w.min(bbox_h).max(1);
            let aspect = long_axis as f64 / short_axis as f64;
            let is_compact = aspect <= linear_aspect_ratio;
            let is_large = size >= small_threshold_cells;
            if is_large && is_compact {
                protected_seeds.push(start_idx as u32);
            }
        }
    }

    // Pass 2: reuse the bitset to mark cells reachable from protected seeds.
    mask.fill(0);
    for seed in protected_seeds {
        let seed_u = seed as usize;
        if get_bit(&mask, seed_u) {
            continue;
        }
        stack.clear();
        stack.push(seed);
        set_bit(&mut mask, seed_u);
        while let Some(curr) = stack.pop() {
            let curr_u = curr as usize;
            let cx = (curr_u % width) as i32;
            let cz = (curr_u / width) as i32;
            for (dx, dz) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let nx = cx + dx;
                let nz = cz + dz;
                if nx < 0 || nz < 0 || nx >= width_i32 || nz >= height_i32 {
                    continue;
                }
                let nxu = nx as usize;
                let nzu = nz as usize;
                if grid[nzu][nxu] == LC_WATER {
                    continue;
                }
                let n_idx = nzu * width + nxu;
                if get_bit(&mask, n_idx) {
                    continue;
                }
                set_bit(&mut mask, n_idx);
                stack.push(n_idx as u32);
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
) -> Vec<Vec<f32>> {
    let mut result: Vec<Vec<f32>> = vec![vec![f32::NAN; width]; height];
    let mut queue: VecDeque<(u32, u32)> = VecDeque::new();
    for z in 0..height {
        let row = &grid[z];
        let h_row = &heights[z];
        for x in 0..width {
            if row[x] == LC_WATER {
                let h = h_row[x];
                if h.is_finite() {
                    result[z][x] = h;
                    queue.push_back((x as u32, z as u32));
                }
            }
        }
    }
    if queue.is_empty() {
        return result;
    }
    let width_i32 = width as i32;
    let height_i32 = height as i32;
    while let Some((x, z)) = queue.pop_front() {
        let cell_y = result[z as usize][x as usize];
        for (dx, dz) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
            let nx = x as i32 + dx;
            let nz = z as i32 + dz;
            if nx < 0 || nz < 0 || nx >= width_i32 || nz >= height_i32 {
                continue;
            }
            let nxu = nx as usize;
            let nzu = nz as usize;
            if result[nzu][nxu].is_nan() {
                result[nzu][nxu] = cell_y;
                queue.push_back((nx as u32, nz as u32));
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
    let Some(nearest_y) = c.nearest_y.as_deref() else {
        return true;
    };
    let cell_y = heights[gz][gx];
    let water_y = nearest_y[gz][gx];
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
