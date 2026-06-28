//! Reclassify ESA built-up cells under OSM bridges to the nearest non-bridge
//! class via BFS, preferring water so river-bridge footprints become LC_WATER.
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};

use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZBBox;
use crate::element_processing::bridges::is_bridge_way;
use crate::element_processing::highways::highway_block_range;
use crate::land_cover::{compute_water_distance, LandCoverData, LC_BUILT_UP, LC_WATER};
use crate::osm_parser::{ProcessedElement, ProcessedWay};

const MAX_BFS_RINGS: usize = 64;
const DEFAULT_RAIL_HALF_WIDTH: i32 = 1;
const DEFAULT_GENERIC_HALF_WIDTH: i32 = 1;
// Margin past highway_block_range to cover deck + parapets, not just the lane centreline.
const BRIDGE_STAMP_MARGIN_CELLS: i32 = 8;
const NEIGHBOURS: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];

pub fn apply_bridge_land_cover_repair(
    land_cover: &mut LandCoverData,
    world_width: usize,
    world_height: usize,
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
    scale: f64,
) {
    let width = land_cover.width;
    let height = land_cover.height;
    if width < 2 || height < 2 || world_width < 2 || world_height < 2 {
        return;
    }
    if !elements
        .iter()
        .any(|e| matches!(e, ProcessedElement::Way(w) if is_bridge_way(w)))
    {
        return;
    }

    let scale_to_grid_x = (width as f64 - 1.0) / (world_width as f64 - 1.0);
    let scale_to_grid_z = (height as f64 - 1.0) / (world_height as f64 - 1.0);
    let min_x = xzbbox.min_x();
    let min_z = xzbbox.min_z();

    let n = width * height;
    let mut bridge_mask: Vec<u64> = vec![0; n.div_ceil(64)];
    let mut bridge_indices: Vec<u32> = Vec::new();

    for elem in elements {
        let ProcessedElement::Way(way) = elem else {
            continue;
        };
        if !is_bridge_way(way) || way.nodes.len() < 2 {
            continue;
        }

        let block_range_world = bridge_block_range_world(way, scale) + BRIDGE_STAMP_MARGIN_CELLS;
        let range_x = ((block_range_world as f64 * scale_to_grid_x).ceil() as i32).max(1);
        let range_z = ((block_range_world as f64 * scale_to_grid_z).ceil() as i32).max(1);

        let mut prev: Option<(i32, i32)> = None;
        for node in &way.nodes {
            let curr = world_to_grid(
                node.x - min_x,
                node.z - min_z,
                scale_to_grid_x,
                scale_to_grid_z,
            );
            if let Some((px, pz)) = prev {
                for (gx, _, gz) in bresenham_line(px, 0, pz, curr.0, 0, curr.1) {
                    stamp_square(
                        &mut bridge_mask,
                        &mut bridge_indices,
                        &land_cover.grid,
                        gx,
                        gz,
                        range_x,
                        range_z,
                        width,
                        height,
                    );
                }
            }
            prev = Some(curr);
        }
    }

    if bridge_indices.is_empty() {
        return;
    }

    let width_i32 = width as i32;
    let height_i32 = height as i32;

    let mut assigned: HashMap<u32, u8> = HashMap::with_capacity(bridge_indices.len());
    let mut current: VecDeque<(u32, u8)> = VecDeque::new();
    // Pass 1: seed water-adjacent bridge cells first so water propagates first.
    for &b_idx in &bridge_indices {
        let bi = b_idx as usize;
        let x = (bi % width) as i32;
        let z = (bi / width) as i32;
        for (dx, dz) in NEIGHBOURS {
            let nx = x + dx;
            let nz = z + dz;
            if nx < 0 || nz < 0 || nx >= width_i32 || nz >= height_i32 {
                continue;
            }
            let nidx = nz as usize * width + nx as usize;
            if get_bit(&bridge_mask, nidx) {
                continue;
            }
            if land_cover.grid[nz as usize][nx as usize] == LC_WATER {
                assigned.insert(b_idx, LC_WATER);
                current.push_back((b_idx, LC_WATER));
                break;
            }
        }
    }
    // Pass 2: fall back to the first non-water non-bridge neighbour.
    for &b_idx in &bridge_indices {
        if assigned.contains_key(&b_idx) {
            continue;
        }
        let bi = b_idx as usize;
        let x = (bi % width) as i32;
        let z = (bi / width) as i32;
        for (dx, dz) in NEIGHBOURS {
            let nx = x + dx;
            let nz = z + dz;
            if nx < 0 || nz < 0 || nx >= width_i32 || nz >= height_i32 {
                continue;
            }
            let nidx = nz as usize * width + nx as usize;
            if get_bit(&bridge_mask, nidx) {
                continue;
            }
            let cls = land_cover.grid[nz as usize][nx as usize];
            if cls == 0 || cls == LC_WATER {
                continue;
            }
            assigned.insert(b_idx, cls);
            current.push_back((b_idx, cls));
            break;
        }
    }

    let mut next: VecDeque<(u32, u8)> = VecDeque::new();
    let mut depth = 1usize;
    while !current.is_empty() && depth < MAX_BFS_RINGS {
        while let Some((idx, cls)) = current.pop_front() {
            let bi = idx as usize;
            let x = (bi % width) as i32;
            let z = (bi / width) as i32;
            for (dx, dz) in NEIGHBOURS {
                let nx = x + dx;
                let nz = z + dz;
                if nx < 0 || nz < 0 || nx >= width_i32 || nz >= height_i32 {
                    continue;
                }
                let nidx_usize = nz as usize * width + nx as usize;
                if !get_bit(&bridge_mask, nidx_usize) {
                    continue;
                }
                let nidx = nidx_usize as u32;
                if let Entry::Vacant(e) = assigned.entry(nidx) {
                    e.insert(cls);
                    next.push_back((nidx, cls));
                }
            }
        }
        std::mem::swap(&mut current, &mut next);
        depth += 1;
    }

    let mut water_changed = false;
    let mut mutated = 0usize;
    let mut to_water = 0usize;
    for &b_idx in &bridge_indices {
        let Some(&new_class) = assigned.get(&b_idx) else {
            continue;
        };
        let bi = b_idx as usize;
        let x = bi % width;
        let z = bi / width;
        let old_class = land_cover.grid[z][x];
        if new_class == old_class {
            continue;
        }
        land_cover.grid[z][x] = new_class;
        mutated += 1;
        if new_class == LC_WATER {
            to_water += 1;
        }
        if (old_class == LC_WATER) != (new_class == LC_WATER) {
            water_changed = true;
        }
    }

    eprintln!(
        "Bridge LC repair: {} cells reclassified ({} to water)",
        mutated, to_water,
    );

    if water_changed {
        land_cover.water_distance = compute_water_distance(&land_cover.grid, width, height);
        land_cover.refresh_water_blend_grid();
    }
}

fn bridge_block_range_world(way: &ProcessedWay, scale: f64) -> i32 {
    if let Some(highway_type) = way.tags.get("highway") {
        return highway_block_range(highway_type, &way.tags, scale);
    }
    if way.tags.contains_key("railway") {
        let tracks = way
            .tags
            .get("tracks")
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(1)
            .max(1);
        let raw = DEFAULT_RAIL_HALF_WIDTH + (tracks - 1).max(0);
        return scaled_block_range(raw, scale);
    }
    scaled_block_range(DEFAULT_GENERIC_HALF_WIDTH, scale)
}

fn scaled_block_range(raw: i32, scale: f64) -> i32 {
    if scale < 1.0 {
        (((raw as f64) * scale).floor() as i32).max(1)
    } else {
        raw
    }
}

fn world_to_grid(rel_x: i32, rel_z: i32, scale_x: f64, scale_z: f64) -> (i32, i32) {
    let gx = (rel_x.max(0) as f64 * scale_x).round() as i32;
    let gz = (rel_z.max(0) as f64 * scale_z).round() as i32;
    (gx, gz)
}

#[allow(clippy::too_many_arguments)]
fn stamp_square(
    mask: &mut [u64],
    bridge_indices: &mut Vec<u32>,
    grid: &[Vec<u8>],
    cx: i32,
    cz: i32,
    range_x: i32,
    range_z: i32,
    width: usize,
    height: usize,
) {
    let width_i32 = width as i32;
    let height_i32 = height as i32;
    let lo_x = (cx - range_x).max(0);
    let hi_x = (cx + range_x).min(width_i32 - 1);
    let lo_z = (cz - range_z).max(0);
    let hi_z = (cz + range_z).min(height_i32 - 1);
    if lo_x > hi_x || lo_z > hi_z {
        return;
    }
    for z in lo_z..=hi_z {
        let zu = z as usize;
        let row_start = zu * width;
        for x in lo_x..=hi_x {
            // Only stamp built-up cells; the margin must not clobber real water/forest.
            if grid[zu][x as usize] != LC_BUILT_UP {
                continue;
            }
            let idx = row_start + x as usize;
            let word = idx >> 6;
            let bit_mask = 1u64 << (idx & 63);
            if (mask[word] & bit_mask) == 0 {
                mask[word] |= bit_mask;
                bridge_indices.push(idx as u32);
            }
        }
    }
}

#[inline(always)]
fn get_bit(mask: &[u64], idx: usize) -> bool {
    (mask[idx >> 6] >> (idx & 63)) & 1 != 0
}
