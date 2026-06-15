//! Per-cell water depth carving from a chamfer-3-4 distance transform over the LC_WATER mask.

use crate::block_definitions::{
    Block, AIR, CLAY, COARSE_DIRT, DIRT, GRAVEL, KELP, KELP_PLANT, MAGMA_BLOCK, SAND, SANDSTONE,
    SEAGRASS, SEA_PICKLE, SOUL_SAND, STONE, TALL_SEAGRASS_BOTTOM, TALL_SEAGRASS_TOP, WATER,
};
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::floodfill_cache::RoadMaskBitmap;
use crate::ground::Ground;
use crate::land_cover::LC_WATER;
use crate::world_editor::{WorldEditor, MIN_Y};

/// Flat shoal width in chamfer-DT units; slope only starts past it.
const SHOAL_DT_UNITS: u16 = 9;

/// DT saturation cap. Depth clamps to <=6 long before this.
const DT_MAX: u8 = u8::MAX;

/// Maximum water carve depth, in blocks (the deepest tier).
const MAX_WATER_DEPTH: i32 = 6;

/// Cap on water sub-rect cells (bounds memory, keeps u32 indices valid); ~1000 km².
const MAX_WATER_FIELD_CELLS: usize = 1_000_000_000;

#[inline]
fn nibble_get(buf: &[u8], i: usize) -> u8 {
    let byte = buf[i >> 1];
    if i & 1 == 0 {
        byte & 0x0F
    } else {
        byte >> 4
    }
}

#[inline]
fn nibble_set(buf: &mut [u8], i: usize, v: u8) {
    debug_assert!(v < 16, "nibble value {v} out of range");
    let byte = &mut buf[i >> 1];
    if i & 1 == 0 {
        *byte = (*byte & 0xF0) | (v & 0x0F);
    } else {
        *byte = (*byte & 0x0F) | ((v & 0x0F) << 4);
    }
}

#[inline]
fn bit_get(b: &[u64], i: usize) -> bool {
    (b[i >> 6] >> (i & 63)) & 1 == 1
}

#[inline]
fn bit_set(b: &mut [u64], i: usize) {
    b[i >> 6] |= 1u64 << (i & 63);
}

/// Baked per-cell carve depth (0..=6), nibble-packed over the water sub-rect.
pub struct BigWaterField {
    depth: Vec<u8>,
    width: usize,
    height: usize,
    min_x: i32,
    min_z: i32,
}

impl BigWaterField {
    fn empty() -> Self {
        Self {
            depth: Vec::new(),
            width: 0,
            height: 0,
            min_x: 0,
            min_z: 0,
        }
    }

    #[inline]
    fn local_idx(&self, x: i32, z: i32) -> Option<usize> {
        let lx = i64::from(x) - i64::from(self.min_x);
        let lz = i64::from(z) - i64::from(self.min_z);
        if lx < 0 || lz < 0 || lx as usize >= self.width || lz as usize >= self.height {
            return None;
        }
        Some(lz as usize * self.width + lx as usize)
    }

    /// Carve depth at the cell; 0 outside the water sub-rect.
    #[inline]
    pub fn depth_at(&self, x: i32, z: i32) -> i32 {
        match self.local_idx(x, z) {
            Some(i) => i32::from(nibble_get(&self.depth, i)),
            None => 0,
        }
    }
}

/// Map a grid-cell span to the block span that samples into it (a safe superset).
pub(crate) fn grid_span_to_block_span(
    g_lo: usize,
    g_hi: usize,
    world_dim: usize,
    grid_dim: usize,
) -> (i32, i32) {
    if grid_dim <= 1 || world_dim <= 1 {
        return (0, world_dim.saturating_sub(1) as i32);
    }
    let f = (world_dim - 1) as f64 / (grid_dim - 1) as f64;
    let lo = ((g_lo as f64 - 0.5) * f).floor() as i32 - 1;
    let hi = ((g_hi as f64 + 0.5) * f).ceil() as i32 + 1;
    (lo.max(0), hi.min(world_dim as i32 - 1))
}

/// Run a chamfer DT over the LC_WATER mask and bake per-cell carve depth.
pub fn compute_big_water_field(ground: &Ground, xzbbox: &XZBBox) -> BigWaterField {
    let min_x = xzbbox.min_x();
    let max_x = xzbbox.max_x();
    let min_z = xzbbox.min_z();
    let max_z = xzbbox.max_z();

    // Water bbox from the small land-cover grid, avoiding a full-world scan.
    let (wmin_x, wmin_z, wmax_x, wmax_z) = match ground.lc_water_block_bounds() {
        Some((lx, lz, hx, hz)) => (min_x + lx, min_z + lz, min_x + hx, min_z + hz),
        None => return BigWaterField::empty(),
    };

    let is_lc_water =
        |x: i32, z: i32| ground.cover_class(XZPoint::new(x - min_x, z - min_z)) == LC_WATER;

    // Pad by one for the shore ring (clamped); keeps the DT identical to full-bbox.
    let smin_x = (wmin_x - 1).max(min_x);
    let smax_x = (wmax_x + 1).min(max_x);
    let smin_z = (wmin_z - 1).max(min_z);
    let smax_z = (wmax_z + 1).min(max_z);
    let sw = (smax_x - smin_x + 1) as usize;
    let sh = (smax_z - smin_z + 1) as usize;
    // Cap cells to bound memory; above it, render flat water instead of crashing.
    let total = match sw.checked_mul(sh) {
        Some(t) if t <= MAX_WATER_FIELD_CELLS => t,
        _ => {
            eprintln!("Warning: water area too large for depth carving; rendering flat water");
            return BigWaterField::empty();
        }
    };

    // Seed DT: water = DT_MAX (unreached), shore/land = 0.
    let mut dt = vec![0u8; total];
    for z in smin_z..=smax_z {
        let row = (z - smin_z) as usize * sw;
        for x in smin_x..=smax_x {
            if is_lc_water(x, z) {
                dt[row + (x - smin_x) as usize] = DT_MAX;
            }
        }
    }
    chamfer_3_4_dt(&mut dt, sw, sh);

    // Per-component BFS for the max DT, then bake each cell's depth.
    let mut depth = vec![0u8; total.div_ceil(2)];
    let mut visited = vec![0u64; total.div_ceil(64)];
    let mut comp: Vec<u32> = Vec::new();
    for start in 0..total {
        if dt[start] == 0 || bit_get(&visited, start) {
            continue;
        }
        comp.clear();
        comp.push(start as u32);
        bit_set(&mut visited, start);
        let mut comp_max = 0u8;
        let mut head = 0;
        while head < comp.len() {
            let idx = comp[head] as usize;
            head += 1;
            comp_max = comp_max.max(dt[idx]);
            let i = idx % sw;
            let j = idx / sw;
            let mut visit = |n: usize, comp: &mut Vec<u32>| {
                if dt[n] != 0 && !bit_get(&visited, n) {
                    bit_set(&mut visited, n);
                    comp.push(n as u32);
                }
            };
            if i > 0 {
                visit(idx - 1, &mut comp);
            }
            if i + 1 < sw {
                visit(idx + 1, &mut comp);
            }
            if j > 0 {
                visit(idx - sw, &mut comp);
            }
            if j + 1 < sh {
                visit(idx + sw, &mut comp);
            }
        }
        let cm = u16::from(comp_max);
        for &c in &comp {
            let idx = c as usize;
            let x = smin_x + (idx % sw) as i32;
            let z = smin_z + (idx / sw) as i32;
            let d = ocean_depth_for_cell(x, z, u16::from(dt[idx]), cm);
            nibble_set(&mut depth, idx, d as u8);
        }
    }

    BigWaterField {
        depth,
        width: sw,
        height: sh,
        min_x: smin_x,
        min_z: smin_z,
    }
}

/// In-place two-sweep chamfer-3-4 DT. Input: 0 = shore, `DT_MAX` = water seed.
fn chamfer_3_4_dt(d: &mut [u8], w: usize, h: usize) {
    let step = |v: u8, add: u8| v.saturating_add(add);
    for j in 0..h {
        for i in 0..w {
            let idx = j * w + i;
            if d[idx] == 0 {
                continue;
            }
            let mut best = d[idx];
            if i > 0 {
                best = best.min(step(d[idx - 1], 3));
            }
            if j > 0 {
                best = best.min(step(d[idx - w], 3));
                if i > 0 {
                    best = best.min(step(d[idx - w - 1], 4));
                }
                if i + 1 < w {
                    best = best.min(step(d[idx - w + 1], 4));
                }
            }
            d[idx] = best;
        }
    }
    for j in (0..h).rev() {
        for i in (0..w).rev() {
            let idx = j * w + i;
            if d[idx] == 0 {
                continue;
            }
            let mut best = d[idx];
            if i + 1 < w {
                best = best.min(step(d[idx + 1], 3));
            }
            if j + 1 < h {
                best = best.min(step(d[idx + w], 3));
                if i > 0 {
                    best = best.min(step(d[idx + w - 1], 4));
                }
                if i + 1 < w {
                    best = best.min(step(d[idx + w + 1], 4));
                }
            }
            d[idx] = best;
        }
    }
}

/// Width-tiered max carve depth. Bigger bodies reach a deeper floor.
#[inline]
fn polygon_local_max(component_max_units: u16) -> i32 {
    if component_max_units < 21 {
        2
    } else if component_max_units < 45 {
        3
    } else if component_max_units < 75 {
        4
    } else {
        6
    }
}

/// Depth from an effective DT and component max: ramp past the shoal, tier-clamped.
fn depth_from_dt(dt_eff: f64, component_max_units: u16) -> i32 {
    if dt_eff < f64::from(SHOAL_DT_UNITS) {
        return 0;
    }
    let local_max = polygon_local_max(component_max_units);
    let slope = if component_max_units < 21 {
        1.0 / 3.0
    } else if component_max_units < 45 {
        1.0 / 4.0
    } else if component_max_units < 75 {
        1.0 / 6.0
    } else {
        1.0 / 8.0
    };
    let dist_blocks = (dt_eff - f64::from(SHOAL_DT_UNITS)) / 3.0;
    ((dist_blocks * slope).floor() as i32).clamp(0, local_max)
}

/// Per-cell carve depth, with deterministic contour wobble on the bank lines.
fn ocean_depth_for_cell(x: i32, z: i32, dt_units: u16, component_max_units: u16) -> i32 {
    if dt_units == 0 {
        return 0;
    }
    let wobble = (crate::ground_generation::value_noise_01(x, z, 12) - 0.5) * 4.0;
    depth_from_dt(f64::from(dt_units) + wobble, component_max_units)
}

/// Safe upper bound on a map's deepest carve, from the pre-repair water mask.
pub fn estimate_max_carve_depth(
    lc_grid: &[Vec<u8>],
    world_width: usize,
    world_height: usize,
) -> i32 {
    let gh = lc_grid.len();
    let gw = lc_grid.first().map_or(0, Vec::len);
    if gw == 0 || gh == 0 {
        return 0;
    }
    let mut dt = vec![0u8; gw * gh];
    let mut any_water = false;
    for (z, row) in lc_grid.iter().enumerate() {
        for (x, &c) in row.iter().enumerate() {
            if c == LC_WATER {
                dt[z * gw + x] = DT_MAX;
                any_water = true;
            }
        }
    }
    if !any_water {
        return 0;
    }
    chamfer_3_4_dt(&mut dt, gw, gh);
    let grid_max_dt = dt.iter().copied().max().unwrap_or(0);
    // Inclusive-span ratio, matching the grid<->block mapping elsewhere (safe upper bound).
    let wr = world_width.saturating_sub(1).max(1) as f64 / gw.saturating_sub(1).max(1) as f64;
    let hr = world_height.saturating_sub(1).max(1) as f64 / gh.saturating_sub(1).max(1) as f64;
    let block_max_dt = f64::from(grid_max_dt) * wr.max(hr).max(1.0);
    let comp_max = block_max_dt.min(f64::from(u16::MAX)) as u16;
    depth_from_dt(block_max_dt + 2.0, comp_max)
}

/// Place the underwater stack: water column, a layered bed, dunes and vegetation.
pub fn carve_water_column(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    water_y: i32,
    depth: i32,
    road_mask: &RoadMaskBitmap,
) {
    debug_assert!(
        depth <= MAX_WATER_DEPTH,
        "water carve depth {depth} exceeds the max tier {MAX_WATER_DEPTH}"
    );
    // Clamp to the valid tier range, and keep the bed above bedrock.
    let depth = depth
        .clamp(0, MAX_WATER_DEPTH)
        .min((water_y - MIN_Y - 2).max(0));
    for dy in 0..=depth {
        editor.set_block_absolute(WATER, x, water_y - dy, z, None, Some(&[]));
    }
    let bed_y = water_y - depth - 1;

    // Keep the bed plain near causeways so blobs/dunes/veg don't clutter piers.
    let near_bridge = depth >= 2 && !road_mask.is_empty() && bridge_adjacent(road_mask, x, z);

    // Layered bed: gravel base with noise-driven sand/clay/dirt patches over stone.
    let (top_block, under_block) = match depth {
        0..=1 => (SAND, SANDSTONE),
        2..=6 if near_bridge => {
            if crate::ground_generation::value_noise_01(x, z, 6) < 0.4 && depth <= 3 {
                (SAND, STONE)
            } else {
                (GRAVEL, STONE)
            }
        }
        2..=6 => {
            // Jitter the depth tier so patches don't ring the shore.
            let h = crate::land_cover::coord_hash(x + 7, z + 13);
            let d = (depth + (h % 3) as i32 - 1).max(1);
            // Domain-warp the sample coords so patches read organic, not circular.
            let warp_x = crate::ground_generation::value_noise_01(x + 901, z + 33, 40);
            let warp_z = crate::ground_generation::value_noise_01(x + 17, z + 811, 40);
            let wx = x + ((warp_x - 0.5) * 24.0) as i32;
            let wz = z + ((warp_z - 0.5) * 24.0) as i32;
            // Per-block noise, sampled lazily so most cells skip the rare tiers.
            let vn = |dx, dz, s| crate::ground_generation::value_noise_01(wx + dx, wz + dz, s);
            let top = if d <= 1 {
                SAND
            } else if d == 2 {
                if vn(53, 97, 36) > 0.50 {
                    SAND
                } else {
                    GRAVEL
                }
            } else if d >= 5 && vn(401, 503, 8) > 0.96 {
                MAGMA_BLOCK
            } else if d >= 5 && vn(727, 911, 8) > 0.96 {
                SOUL_SAND
            } else if vn(73, 109, 42) > 0.74 {
                CLAY
            } else if vn(53, 97, 36) > 0.81 {
                SAND
            } else if vn(211, 41, 26) > 0.88 {
                DIRT
            } else if vn(311, 17, 30) > 0.90 {
                COARSE_DIRT
            } else {
                GRAVEL
            };
            (top, STONE)
        }
        _ => (GRAVEL, STONE),
    };

    if bed_y > MIN_Y {
        editor.set_block_absolute(top_block, x, bed_y, z, None, Some(&[]));
    }
    // Supports the gravity-affected bed; dropped onto bedrock at the lowest carve.
    if bed_y - 1 > MIN_Y {
        editor.set_block_absolute(under_block, x, bed_y - 1, z, None, Some(&[]));
    }
    // Backfill stone so neighbour side-faces never expose air under varied beds.
    let fill_to = (bed_y - 2).max(MIN_Y + 1);
    let fill_from = (bed_y - 12).max(MIN_Y + 1);
    if fill_from <= fill_to {
        editor.fill_column_absolute(STONE, x, z, fill_from, fill_to, true);
    }

    // Dunes return their crest so veg plants on top instead of inside them.
    let bump = if depth >= 1 && !near_bridge {
        place_underwater_dunes(editor, x, z, water_y, bed_y, depth, top_block)
    } else {
        0
    };
    if depth >= 3 && !near_bridge {
        place_underwater_vegetation(editor, x, z, water_y, bed_y + bump, depth);
    }
}

/// True if any cell in the 5x5 area around (x, z), center excluded, carries a road/bridge.
fn bridge_adjacent(road_mask: &RoadMaskBitmap, x: i32, z: i32) -> bool {
    for dz in -2..=2 {
        for dx in -2..=2 {
            if (dx != 0 || dz != 0) && road_mask.contains(x + dx, z + dz) {
                return true;
            }
        }
    }
    false
}

/// Dune amplitude for a cell, capped so the crest stays below the surface.
/// Depth stands in for body width: deeper cells only occur in wider water.
fn dune_amp(depth: i32) -> i32 {
    let target = match depth {
        ..=3 => 2,
        4 => 3,
        _ => 4,
    };
    target.min(depth - 1)
}

/// Width-aware multi-octave dunes 1-4 blocks tall on the bed. Returns the
/// placed crest height. Ported from the Teddy fork.
fn place_underwater_dunes(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    water_y: i32,
    bed_y: i32,
    depth: i32,
    bed_block: Block,
) -> i32 {
    let amp = dune_amp(depth);
    if amp <= 0 {
        return 0;
    }
    let warp_x = crate::ground_generation::value_noise_01(x + 901, z + 33, 50);
    let warp_z = crate::ground_generation::value_noise_01(x + 17, z + 811, 50);
    let wx = x + ((warp_x - 0.5) * 30.0) as i32;
    let wz = z + ((warp_z - 0.5) * 30.0) as i32;
    let n_large = crate::ground_generation::value_noise_01(wx + 113, wz + 257, 44) as f32;
    let n_med = crate::ground_generation::value_noise_01(wx + 31, wz + 71, 18) as f32;
    let n_sharp = crate::ground_generation::value_noise_01(wx + 7, wz + 11, 10) as f32;
    let h_f = 0.40 * n_large + 0.30 * n_med + 0.30 * n_sharp;
    if h_f < 0.28 {
        return 0;
    }
    let t = (h_f - 0.28) / 0.72;
    let bump = ((t.powf(0.45) * (amp as f32 + 0.99)).floor() as i32).clamp(1, amp);
    for dy in 1..=bump {
        let y = bed_y + dy;
        if y >= water_y {
            return dy - 1;
        }
        editor.set_block_absolute(bed_block, x, y, z, None, Some(&[]));
    }
    bump
}

/// Clumped underwater vegetation on a carved bed: SEAGRASS/TALL_SEAGRASS/SEA_PICKLE
/// meadows, with rarer KELP columns in deeper water. Ported from the Teddy fork.
fn place_underwater_vegetation(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    water_y: i32,
    bed_top: i32,
    depth: i32,
) {
    // Two decorrelated noise fields gate clumpy meadows instead of pepper-shot cells.
    let field_noise = crate::ground_generation::value_noise_01(x + 53, z + 89, 30);
    let cluster_noise = crate::ground_generation::value_noise_01(x + 401, z + 17, 10);
    let combined = field_noise * cluster_noise;

    // KELP: deep cells where both noises ring high; tip caps the column.
    let kelp_pick = (crate::land_cover::coord_hash(x + 91, z + 41) % 100) as i32;
    if depth >= 4 && field_noise > 0.78 && cluster_noise > 0.80 && kelp_pick < 25 {
        let plant_top_full = water_y - 1;
        let plant_bottom = bed_top + 1;
        let avail = plant_top_full - plant_bottom;
        if avail >= 3 {
            let hgt_pick = (crate::land_cover::coord_hash(x + 211, z + 503) % 100) as i32;
            let share = 30 + hgt_pick * 70 / 100;
            let used = ((avail as i64 * share as i64) / 100) as i32;
            let used = used.max(3).min(avail);
            let plant_top = plant_bottom + used;
            // Replace water, never air, so plants don't grow into the bed or float.
            for y in plant_bottom..plant_top {
                editor.set_block_absolute(KELP_PLANT, x, y, z, None, Some(&[AIR]));
            }
            editor.set_block_absolute(KELP, x, plant_top, z, None, Some(&[AIR]));
        }
        return;
    }

    // Seagrass meadow mix: 50% SEAGRASS, 35% TALL_SEAGRASS, 15% SEA_PICKLE.
    if combined > 0.42 {
        let dropout = crate::land_cover::coord_hash(x + 17, z + 31) % 10;
        if dropout < 4 {
            let plant_y = bed_top + 1;
            if plant_y < water_y {
                let pick = (crate::land_cover::coord_hash(x + 211, z + 73) % 100) as i32;
                if pick < 50 {
                    editor.set_block_absolute(SEAGRASS, x, plant_y, z, None, Some(&[AIR]));
                } else if pick < 85 && plant_y + 1 < water_y {
                    editor.set_block_absolute(
                        TALL_SEAGRASS_BOTTOM,
                        x,
                        plant_y,
                        z,
                        None,
                        Some(&[AIR]),
                    );
                    editor.set_block_absolute(
                        TALL_SEAGRASS_TOP,
                        x,
                        plant_y + 1,
                        z,
                        None,
                        Some(&[AIR]),
                    );
                } else {
                    editor.set_block_absolute(SEA_PICKLE, x, plant_y, z, None, Some(&[AIR]));
                }
            }
        }
    }
}

/// Post-pass carving every LC_WATER cell, so ESA-only water gets depth too.
pub fn carve_lc_water_pass(
    editor: &mut WorldEditor,
    ground: &Ground,
    xzbbox: &XZBBox,
    bwf: &BigWaterField,
    road_mask: &RoadMaskBitmap,
) {
    let x1 = bwf.min_x + bwf.width as i32 - 1;
    let z1 = bwf.min_z + bwf.height as i32 - 1;
    carve_lc_water_region(
        editor, ground, xzbbox, bwf, road_mask, bwf.min_x, x1, bwf.min_z, z1,
    );
}

/// `carve_lc_water_pass` restricted to the inclusive `[iter_min..=iter_max]` block
/// range (intersected with the water sub-rect). Per-tile callers pass strict tile
/// bounds; writes are vertical-only so output is identical regardless of tiling.
#[allow(clippy::too_many_arguments)]
pub fn carve_lc_water_region(
    editor: &mut WorldEditor,
    ground: &Ground,
    xzbbox: &XZBBox,
    bwf: &BigWaterField,
    road_mask: &RoadMaskBitmap,
    iter_min_x: i32,
    iter_max_x: i32,
    iter_min_z: i32,
    iter_max_z: i32,
) {
    let off_x = xzbbox.min_x();
    let off_z = xzbbox.min_z();
    // Only the water sub-rect can hold LC_WATER cells; intersect it with the range.
    let x0 = bwf.min_x.max(iter_min_x);
    let x1 = (bwf.min_x + bwf.width as i32 - 1).min(iter_max_x);
    let z0 = bwf.min_z.max(iter_min_z);
    let z1 = (bwf.min_z + bwf.height as i32 - 1).min(iter_max_z);
    for z in z0..=z1 {
        for x in x0..=x1 {
            // Keep road/bridge surfaces (causeways, decks).
            if road_mask.contains(x, z) {
                continue;
            }
            let coord = XZPoint::new(x - off_x, z - off_z);
            if ground.cover_class(coord) != LC_WATER {
                continue;
            }
            let water_y = ground.water_level(coord);
            // Skip land bumps an over-claiming water polygon sits above.
            if editor.get_ground_level(x, z) > water_y {
                continue;
            }
            carve_water_column(editor, x, z, water_y, bwf.depth_at(x, z), road_mask);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dt_distance_from_shore() {
        let (w, h) = (7usize, 7usize);
        let mut d = vec![0u8; w * h];
        for j in 1..6 {
            for i in 1..6 {
                d[j * w + i] = DT_MAX;
            }
        }
        chamfer_3_4_dt(&mut d, w, h);
        assert_eq!(d[0], 0, "shore stays 0");
        assert_eq!(d[w + 1], 3, "edge water is one ortho step from shore");
        assert_eq!(d[3 * w + 3], 9, "centre is three ortho steps in");
    }

    #[test]
    fn depth_zero_inside_shoal_and_on_land() {
        assert_eq!(ocean_depth_for_cell(0, 0, 0, 0), 0);
        assert_eq!(ocean_depth_for_cell(10, 10, 3, 200), 0);
    }

    #[test]
    fn depth_clamps_to_tier_max() {
        assert_eq!(ocean_depth_for_cell(5, 5, u16::from(DT_MAX), 200), 6);
        assert_eq!(polygon_local_max(10), 2);
        assert_eq!(polygon_local_max(60), 4);
        assert_eq!(polygon_local_max(100), 6);
    }

    #[test]
    fn estimate_no_water_is_zero() {
        let grid = vec![vec![0u8; 16]; 16];
        assert_eq!(estimate_max_carve_depth(&grid, 16, 16), 0);
    }

    #[test]
    fn estimate_large_open_water_reaches_max() {
        let grid = vec![vec![LC_WATER; 32]; 32];
        assert_eq!(estimate_max_carve_depth(&grid, 32, 32), 6);
    }

    #[test]
    fn grid_span_covers_all_mapped_blocks() {
        // Brute-force every block that samples into grid cells [1,2]; the span must cover them.
        let (world_dim, grid_dim) = (100usize, 10usize);
        let (g_lo, g_hi) = (1usize, 2usize);
        let (lo, hi) = grid_span_to_block_span(g_lo, g_hi, world_dim, grid_dim);
        for bx in 0..world_dim {
            let gx =
                ((bx as f64 / (world_dim - 1) as f64) * (grid_dim - 1) as f64).round() as usize;
            if gx >= g_lo && gx <= g_hi {
                assert!(
                    bx as i32 >= lo && bx as i32 <= hi,
                    "block {bx} (grid {gx}) not covered by span [{lo},{hi}]"
                );
            }
        }
    }

    #[test]
    fn dune_amp_capped_by_depth() {
        assert_eq!(dune_amp(1), 0);
        assert_eq!(dune_amp(2), 1);
        assert_eq!(dune_amp(3), 2);
        assert_eq!(dune_amp(4), 3);
        assert_eq!(dune_amp(5), 4);
        assert_eq!(dune_amp(6), 4);
    }

    #[test]
    fn bridge_adjacent_detects_neighborhood() {
        let bbox = XZBBox::rect_from_min_max(0, 0, 31, 31).unwrap();
        let mut mask = RoadMaskBitmap::new(&bbox);
        mask.set(12, 10);
        assert!(
            bridge_adjacent(&mask, 10, 10),
            "two cells away is inside the 5x5 area"
        );
        assert!(
            !bridge_adjacent(&mask, 15, 15),
            "three cells away is outside"
        );
        assert!(
            !bridge_adjacent(&mask, 12, 10),
            "the road cell itself is excluded"
        );
        let empty = RoadMaskBitmap::new(&bbox);
        assert!(!bridge_adjacent(&empty, 10, 10));
    }

    #[test]
    fn nibble_round_trip() {
        let mut buf = vec![0u8; 4];
        for (i, v) in [0u8, 6, 3, 1, 5, 2, 4, 0].iter().enumerate() {
            nibble_set(&mut buf, i, *v);
        }
        for (i, v) in [0u8, 6, 3, 1, 5, 2, 4, 0].iter().enumerate() {
            assert_eq!(nibble_get(&buf, i), *v);
        }
    }
}
