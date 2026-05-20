//! Per-cell water depth field driven by a chamfer-3-4 distance transform
//! across each LC_WATER component. Ported (simplified) from
//! arnis-source-water/floodfill_cache.rs + element_processing/underwater.rs.
//!
//! v2.8.0 base already has:
//!   * OSM water override (land_cover_osm_water_override.rs)
//!   * Per-cell water level (ground.water_level)
//!   * Shore SAND swap (ground_generation.rs)
//!
//! This module adds ONLY the bit v2.8.0 is missing: depth carving below
//! water_y based on distance from the shore, with a tiered bed palette
//! (SAND near shore, GRAVEL mid, STONE deep).
//!
//! No stoney palette, no bridge overrides, no thin-land drown, no
//! concentric-ring artifact passes — the v2.8.0 OSM override + bridge
//! repair already resolves those.

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;
use crate::land_cover::LC_WATER;

/// Shoal band width measured in chamfer-DT units (3 = orthogonal step).
/// Cells with `dt < SHOAL_DT_UNITS` are inside the flat shoal — bed sits
/// at `water_y - 1` regardless of polygon size. Slope only kicks in past
/// the shoal so the first ~3 cells from shore stay level.
const SHOAL_DT_UNITS: u16 = 9;

/// Per-cell bit grid backing the chamfer DT input.
struct WaterBitmap {
    min_x: i32,
    min_z: i32,
    width: usize,
    height: usize,
    cells: Vec<bool>,
}

impl WaterBitmap {
    fn new(min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> Self {
        let width = (max_x - min_x + 1).max(1) as usize;
        let height = (max_z - min_z + 1).max(1) as usize;
        Self {
            min_x,
            min_z,
            width,
            height,
            cells: vec![false; width * height],
        }
    }

    #[inline]
    fn idx(&self, x: i32, z: i32) -> usize {
        (z - self.min_z) as usize * self.width + (x - self.min_x) as usize
    }

    #[inline]
    fn set(&mut self, x: i32, z: i32) {
        let i = self.idx(x, z);
        self.cells[i] = true;
    }
}

/// Two-sweep chamfer-3-4 distance transform. 3 = orthogonal step,
/// 4 = diagonal step. Non-water cells get distance 0 (they are the shore).
fn chamfer_3_4_dt(bm: &WaterBitmap) -> Vec<u16> {
    let w = bm.width;
    let h = bm.height;
    const INF: u16 = u16::MAX / 2;
    let mut d = vec![0u16; w * h];

    for i in 0..(w * h) {
        d[i] = if bm.cells[i] { INF } else { 0 };
    }

    // Forward sweep.
    for j in 0..h {
        for i in 0..w {
            let idx = j * w + i;
            if d[idx] == 0 {
                continue;
            }
            let mut best = d[idx];
            if i > 0 {
                best = best.min(d[idx - 1].saturating_add(3));
            }
            if j > 0 {
                best = best.min(d[idx - w].saturating_add(3));
                if i > 0 {
                    best = best.min(d[idx - w - 1].saturating_add(4));
                }
                if i + 1 < w {
                    best = best.min(d[idx - w + 1].saturating_add(4));
                }
            }
            d[idx] = best;
        }
    }

    // Backward sweep.
    for j in (0..h).rev() {
        for i in (0..w).rev() {
            let idx = j * w + i;
            if d[idx] == 0 {
                continue;
            }
            let mut best = d[idx];
            if i + 1 < w {
                best = best.min(d[idx + 1].saturating_add(3));
            }
            if j + 1 < h {
                best = best.min(d[idx + w].saturating_add(3));
                if i > 0 {
                    best = best.min(d[idx + w - 1].saturating_add(4));
                }
                if i + 1 < w {
                    best = best.min(d[idx + w + 1].saturating_add(4));
                }
            }
            d[idx] = best;
        }
    }

    d
}

/// Per-cell DT field over promoted big-water components.
pub struct BigWaterField {
    /// chamfer-3-4 DT (3=ortho, 4=diag). 0 outside the promoted mask or at shore-edge.
    dt: Vec<u16>,
    /// dt_max of the cell's connected component. Used to pick the slope tier.
    comp_max_per_cell: Vec<u16>,
    width: usize,
    height: usize,
    min_x: i32,
    min_z: i32,
}

impl BigWaterField {
    #[inline]
    fn local_idx(&self, x: i32, z: i32) -> Option<usize> {
        let lx = i64::from(x) - i64::from(self.min_x);
        let lz = i64::from(z) - i64::from(self.min_z);
        if lx < 0 || lz < 0 {
            return None;
        }
        let lx = lx as usize;
        let lz = lz as usize;
        if lx >= self.width || lz >= self.height {
            return None;
        }
        Some(lz * self.width + lx)
    }

    /// Returns `(dt_units, component_max_units)` for the cell. Both 0 if
    /// outside the bbox or in an unpromoted (small pond) component.
    #[inline]
    pub fn depth_at(&self, x: i32, z: i32) -> (u16, u16) {
        match self.local_idx(x, z) {
            Some(i) if i < self.dt.len() => (self.dt[i], self.comp_max_per_cell[i]),
            _ => (0, 0),
        }
    }
}

/// Scan the bbox for LC_WATER components via BFS. Promote any component
/// with ≥ BIG_WATER_MIN_CELLS cells OR touching the bbox edge (off-tile
/// ocean continuation). Run chamfer-3-4 DT over the union of promoted
/// cells. Broadcast per-component `dt_max` so the depth ramp can pick
/// the right slope tier.
pub fn compute_big_water_field(ground: &Ground, xzbbox: &XZBBox) -> BigWaterField {
    let min_x = xzbbox.min_x();
    let max_x = xzbbox.max_x();
    let min_z = xzbbox.min_z();
    let max_z = xzbbox.max_z();
    let width = (i64::from(max_x) - i64::from(min_x) + 1) as usize;
    let height = (i64::from(max_z) - i64::from(min_z) + 1) as usize;

    if !ground.has_land_cover() {
        return BigWaterField {
            dt: Vec::new(),
            comp_max_per_cell: Vec::new(),
            width,
            height,
            min_x,
            min_z,
        };
    }

    let total = width
        .checked_mul(height)
        .expect("compute_big_water_field: grid size overflow");
    let mut visited: Vec<bool> = vec![false; total];

    let idx = |x: i32, z: i32| -> usize {
        let lx = (i64::from(x) - i64::from(min_x)) as usize;
        let lz = (i64::from(z) - i64::from(min_z)) as usize;
        lz * width + lx
    };

    let is_lc_water = |x: i32, z: i32| -> bool {
        let coord = XZPoint::new(x - min_x, z - min_z);
        ground.cover_class(coord) == LC_WATER
    };

    let mut queue: Vec<(i32, i32)> = Vec::with_capacity(1024);
    let mut comp_cells: Vec<(i32, i32)> = Vec::new();
    let mut components: Vec<Vec<(i32, i32)>> = Vec::new();

    for sz in min_z..=max_z {
        for sx in min_x..=max_x {
            if visited[idx(sx, sz)] || !is_lc_water(sx, sz) {
                visited[idx(sx, sz)] = true;
                continue;
            }
            queue.clear();
            comp_cells.clear();
            queue.push((sx, sz));
            visited[idx(sx, sz)] = true;
            while let Some((x, z)) = queue.pop() {
                comp_cells.push((x, z));
                for (dx, dz) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let nx = x + dx;
                    let nz = z + dz;
                    if nx < min_x || nx > max_x || nz < min_z || nz > max_z {
                        continue;
                    }
                    let ni = idx(nx, nz);
                    if visited[ni] {
                        continue;
                    }
                    if is_lc_water(nx, nz) {
                        visited[ni] = true;
                        queue.push((nx, nz));
                    } else {
                        visited[ni] = true;
                    }
                }
            }
            // Promote every LC_WATER component. `polygon_local_max` caps
            // small ponds at 2 blocks deep, so no risk of weirdly-deep
            // 4-cell puddles. Previously gated on >= BIG_WATER_MIN_CELLS
            // (400 cells) || touches_edge — both dropped so river spurs
            // and small lakes get depth too.
            components.push(std::mem::take(&mut comp_cells));
        }
    }

    if components.is_empty() {
        return BigWaterField {
            dt: Vec::new(),
            comp_max_per_cell: Vec::new(),
            width,
            height,
            min_x,
            min_z,
        };
    }

    let mut bitmap = WaterBitmap::new(min_x, min_z, max_x, max_z);
    for cells in &components {
        for (x, z) in cells {
            bitmap.set(*x, *z);
        }
    }
    let dt_flat = chamfer_3_4_dt(&bitmap);

    let mut comp_max_per_cell = vec![0u16; total];
    for cells in &components {
        let mut comp_max: u16 = 0;
        for (x, z) in cells {
            let i = idx(*x, *z);
            if dt_flat[i] > comp_max {
                comp_max = dt_flat[i];
            }
        }
        for (x, z) in cells {
            let i = idx(*x, *z);
            comp_max_per_cell[i] = comp_max;
        }
    }

    BigWaterField {
        dt: dt_flat,
        comp_max_per_cell,
        width,
        height,
        min_x,
        min_z,
    }
}

/// Width-tiered max carve depth for a water polygon. Big bodies get a
/// 6-block max (was 5) to compensate for the gentler `SLOPE_BLOCKS_PER_BLOCK`
/// — rivers ≥ 75 chamfer units wide now reach the floor at ~36 blocks
/// from shore instead of bottoming out in 9.
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

/// Compute carve depth at a single cell from its chamfer-DT distance to
/// the nearest shore and the polygon's component max DT.
///
/// Pipeline:
/// 1. dt == 0 → 0 (cell is the shore itself).
/// 2. dt < SHOAL_DT_UNITS → 0 (still inside the flat shoal band; caller
///    paints the bed block at water_y - 1, which produces the level
///    near-shore platform the user asked for).
/// 3. Past the shoal: `depth = floor((dt - shoal)/3 * slope + jitter)`,
///    clamped to polygon_local_max. Slope is size-tiered (halved from
///    the v131 source-water table) so small ponds carve too while big
///    seas stay calm.
/// 4. `coord_hash` ±0.5 jitter is per-cell uncorrelated, so adjacent
///    cells get independent jitter and the integer-depth contours
///    dissolve into noise instead of reading as concentric stripes.
pub fn ocean_depth_for_cell(
    x: i32,
    z: i32,
    dt_units: u16,
    component_max_units: u16,
) -> i32 {
    if dt_units == 0 || dt_units < SHOAL_DT_UNITS {
        return 0;
    }
    let local_max = polygon_local_max(component_max_units);

    // Halved from the arnis-source-water v131 tiers (1.0, 2/3, 1/2, 1/3).
    // Small ponds still steepest (1/2) so a 7-block puddle carves to
    // depth 1; big bodies use 1/6 so wide rivers don't bottom out in
    // 9 blocks. Tier breakpoints match polygon_local_max above.
    let slope = if component_max_units < 21 {
        1.0 / 2.0
    } else if component_max_units < 45 {
        1.0 / 3.0
    } else if component_max_units < 75 {
        1.0 / 4.0
    } else {
        1.0 / 6.0
    };

    let effective_dt = dt_units - SHOAL_DT_UNITS;
    let dist_blocks = (effective_dt as f64) / 3.0;

    // Per-cell uncorrelated jitter via coord_hash. Each cell gets an
    // independent ±0.5 push on depth_f before floor(), so neighbouring
    // cells of d=2 and d=3 mix freely along the boundary instead of
    // forming a clean contour line. Combined with the SAND→GRAVEL
    // statistical palette in pick_underwater_bed this fully dissolves
    // the concentric-ring artifact.
    let jitter_raw =
        (crate::land_cover::coord_hash(x, z) % 1000) as f64 / 1000.0;
    let jitter = jitter_raw - 0.5;

    let depth_f = dist_blocks * slope + jitter;
    (depth_f.floor() as i32).clamp(0, local_max)
}
