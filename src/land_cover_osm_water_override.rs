//! Force LC_WATER inside OSM water polygons and waterways so OSM defines
//! the shoreline, overriding ESA's noisy 10 m classification.
use std::collections::HashMap;

use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZBBox;
use crate::land_cover::{compute_water_distance, LandCoverData, LC_WATER};
use crate::osm_parser::{
    ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedRelation, ProcessedWay,
};

pub fn apply_osm_water_override(
    land_cover: &mut LandCoverData,
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

    let mut changed: usize = 0;
    for elem in elements {
        match elem {
            ProcessedElement::Way(way) => {
                if is_water_polygon_way(way) {
                    let outer: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
                    changed += fill_polygon_scanline(
                        &mut land_cover.grid,
                        &[outer.as_slice()],
                        &[],
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
                let mut outers: Vec<Vec<(i32, i32)>> = Vec::new();
                let mut inners: Vec<Vec<(i32, i32)>> = Vec::new();
                for member in &rel.members {
                    let nodes: Vec<(i32, i32)> =
                        member.way.nodes.iter().map(|n| (n.x, n.z)).collect();
                    if nodes.len() < 3 {
                        continue;
                    }
                    match member.role {
                        ProcessedMemberRole::Outer => outers.push(nodes),
                        ProcessedMemberRole::Inner => inners.push(nodes),
                        _ => {}
                    }
                }
                if outers.is_empty() {
                    continue;
                }
                let outers_refs: Vec<&[(i32, i32)]> = outers.iter().map(|v| v.as_slice()).collect();
                let inners_refs: Vec<&[(i32, i32)]> = inners.iter().map(|v| v.as_slice()).collect();
                changed += fill_polygon_scanline(
                    &mut land_cover.grid,
                    &outers_refs,
                    &inners_refs,
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
    let tags = &way.tags;
    matches!(tags.get("natural").map(|s| s.as_str()), Some("water"))
        || tags.contains_key("water")
        || matches!(tags.get("landuse").map(|s| s.as_str()), Some("reservoir"))
}

fn is_water_relation(rel: &ProcessedRelation) -> bool {
    let tags = &rel.tags;
    matches!(
        tags.get("natural").map(|s| s.as_str()),
        Some("water" | "bay")
    ) || tags.contains_key("water")
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
