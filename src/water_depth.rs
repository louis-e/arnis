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

use crate::block_definitions::{COBBLESTONE, GRAVEL, MOSSY_COBBLESTONE, SAND, SANDSTONE, STONE, WATER};
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::floodfill_cache::RoadMaskBitmap;
use crate::ground::Ground;
use crate::land_cover::LC_WATER;
use crate::world_editor::{WorldEditor, MIN_Y};

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
/// Pure deterministic — NO per-cell jitter on depth. User feedback from
/// v2 was that coord_hash jitter on `depth_f` produced a bumpy bed-Y
/// (neighbouring cells at depth=2, 3, 1, 4, 2 → jagged seabed). Smooth
/// contours win, even if integer depth still produces concentric bands;
/// the universal SAND palette in `carve_water_column` plus the gentler
/// halved-again slope rates below keep those bands wide enough to read
/// as a gradient rather than stripes.
pub fn ocean_depth_for_cell(
    x: i32,
    z: i32,
    dt_units: u16,
    component_max_units: u16,
) -> i32 {
    if dt_units == 0 {
        return 0;
    }
    // Smooth contour wobble — value_noise_01 at scale 12 means a single
    // sample is shared by a ~12-block patch of neighbours, so depth Y
    // stays smooth across adjacent cells while the SHAPE of contour
    // lines bends organically. ±2 chamfer units = ±0.66 horizontal
    // blocks of wobble — enough to round straight banks, not enough to
    // create jagged bed Y.
    let wobble =
        (crate::ground_generation::value_noise_01(x, z, 12) - 0.5) * 4.0;
    let dt_eff = (dt_units as f64) + wobble;
    if dt_eff < SHOAL_DT_UNITS as f64 {
        return 0;
    }
    let local_max = polygon_local_max(component_max_units);
    let slope = if component_max_units < 21 {
        1.0 / 4.0
    } else if component_max_units < 45 {
        1.0 / 6.0
    } else if component_max_units < 75 {
        1.0 / 8.0
    } else {
        1.0 / 12.0
    };
    let dist_blocks = (dt_eff - SHOAL_DT_UNITS as f64) / 3.0;
    let depth_f = dist_blocks * slope;
    (depth_f.floor() as i32).clamp(0, local_max)
}

/// Place the canonical underwater stack at (x, z) with a given depth.
/// User-spec stack: SAND on top, SANDSTONE one below, STONE filling down
/// to bedrock-safe. Force-overwrites existing blocks via `Some(&[])` so
/// `ground_generation`'s SAND/GRAVEL/CLAY ESA-water palette can't bleed
/// through.
///
/// `depth = 0` ⇒ shoal: bed sits at `water_y - 1` (the flat near-shore
/// platform). `depth ≥ 1` ⇒ carves a WATER column down to
/// `water_y - depth` with the same stack one block below the deepest
/// water cell.
pub fn carve_water_column(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    water_y: i32,
    depth: i32,
) {
    // WATER column. dy = 0 = surface, dy = depth = bottom water block.
    for dy in 0..=depth {
        editor.set_block_absolute(WATER, x, water_y - dy, z, None, Some(&[]));
    }
    let bed_y = water_y - depth - 1;

    // Depth-tiered bed palette per user spec:
    //   d 0, 1, 2  → SAND        (top)  + SANDSTONE (under)
    //   d 3        → 50/50 SAND/GRAVEL  + matching SANDSTONE/STONE under
    //   d ≥ 4      → GRAVEL                + STONE under
    let h = crate::land_cover::coord_hash(x, z) % 100;
    let (top_block, under_block) = match depth {
        0..=2 => (SAND, SANDSTONE),
        3 => {
            if (h as i32) < 50 {
                (SAND, SANDSTONE)
            } else {
                (GRAVEL, STONE)
            }
        }
        _ => (GRAVEL, STONE),
    };

    if bed_y > MIN_Y {
        editor.set_block_absolute(top_block, x, bed_y, z, None, Some(&[]));
    }
    if bed_y - 1 > MIN_Y {
        editor.set_block_absolute(under_block, x, bed_y - 1, z, None, Some(&[]));
    }
    let stone_top = bed_y - 2;
    let stone_bottom = MIN_Y + 1;
    if stone_top >= stone_bottom {
        // skip_existing = true: only fills AIR / WATER pockets, preserves
        // any terrain block ground_generation already placed below.
        editor.fill_column_absolute(STONE, x, z, stone_bottom, stone_top, true);
    }
}

/// Place a bridge pier column at (x, z): COBBLESTONE / MOSSY_COBBLESTONE
/// mix from `top_y` down to `bottom_y`. Used by `carve_lc_water_pass` at
/// sparse bridge-over-water cells to give bridges visible feet/pillars
/// in the water instead of the deck floating with empty water under it.
fn place_bridge_pier(editor: &mut WorldEditor, x: i32, z: i32, top_y: i32, bottom_y: i32) {
    let h = crate::land_cover::coord_hash(x, z);
    let mut y = top_y;
    while y >= bottom_y && y > MIN_Y {
        // 70% COBBLE / 30% MOSSY for organic look.
        let block = if ((h.wrapping_add(y as u64) ^ (y as u64).wrapping_mul(2654435761)) % 100) < 30 {
            MOSSY_COBBLESTONE
        } else {
            COBBLESTONE
        };
        editor.set_block_absolute(block, x, y, z, None, Some(&[]));
        y -= 1;
    }
}

/// Universal post-pass: every LC_WATER cell in the bbox gets the same
/// depth carve as OSM water polygons. Solves the user-reported gap where
/// ESA-classified water without an OSM polygon (big inland seas, bay
/// heads, wide river mid-spans) rendered flat — `water_areas.rs` only
/// runs on OSM water polygons.
///
/// Idempotent vs `water_areas.rs`: cells already processed by an OSM
/// polygon scanline get re-carved with the same depth and the same bed
/// stack (force-overwrite produces identical bytes).
pub fn carve_lc_water_pass(
    editor: &mut WorldEditor,
    ground: &Ground,
    xzbbox: &XZBBox,
    bwf: &BigWaterField,
    road_mask: &RoadMaskBitmap,
) {
    let min_x = xzbbox.min_x();
    let max_x = xzbbox.max_x();
    let min_z = xzbbox.min_z();
    let max_z = xzbbox.max_z();
    for z in min_z..=max_z {
        for x in min_x..=max_x {
            let coord = XZPoint::new(x - min_x, z - min_z);
            if ground.cover_class(coord) != LC_WATER {
                continue;
            }
            let water_y = ground.water_level(coord);
            let (dt, comp_max) = bwf.depth_at(x, z);
            let depth = ocean_depth_for_cell(x, z, dt, comp_max);
            // Carve the water column. Bed Y sits at water_y - depth - 1.
            // For road/bridge cells over LC_WATER, this places water
            // UNDER the deck (deck_y > water_y) so the river continues
            // visibly across the bridge.
            carve_water_column(editor, x, z, water_y, depth);

            // Sparse cobblestone piers at bridge cells — 1 in every 12
            // cells via coord_hash. Pier goes from water_y - 1 (just
            // below the water surface; never pokes above) down to the
            // bed, force-overwriting the WATER blocks we just placed.
            if road_mask.contains(x, z)
                && (crate::land_cover::coord_hash(x, z) % 12) == 0
            {
                let pier_top = water_y - 1;
                let pier_bottom = water_y - depth - 1;
                place_bridge_pier(editor, x, z, pier_top, pier_bottom);
            }
        }
    }
}
