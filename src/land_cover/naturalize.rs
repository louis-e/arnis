//! Land cover for terrain-only worlds, as the land would look with nobody on it.
//!
//! ESA maps towns as built-up and fields as cropland, which render as stone bricks and
//! farmland even when no OSM object is placed. Here cropland becomes grassland, built-up
//! land narrow enough to be a bridge or pier goes back to the water it crosses, and the
//! rest takes the vegetation nearest to it, along wavy borders so no street grid shows.
use rayon::prelude::*;
use std::collections::HashMap;

use crate::climate::Climate;
use crate::ground_generation::value_noise_salted;
use crate::land_cover::{
    compute_water_distance, LandCoverData, LC_BARE, LC_BUILT_UP, LC_CROPLAND, LC_GRASSLAND,
    LC_MOSS, LC_SHRUBLAND, LC_TREE_COVER, LC_WATER,
};

/// Built-up land at most this wide with water on both sides is a bridge, pier or jetty.
const WATER_SPAN_M: f64 = 50.0;
/// How far above that water such a strip may sit and still be returned to it. A dam crest
/// is far higher, and bridge decks were already pulled down to the water.
const WATER_HEIGHT_TOLERANCE_BLOCKS: f32 = 6.0;
/// Lattice the nearest vegetation is looked up on: one ESA pixel.
const LATTICE_M: f64 = 10.0;
/// How far the borders between the classes that move in may wander, and how long their
/// bends are.
const WARP_M: f64 = 40.0;
const WARP_WAVELENGTH_M: f64 = 60.0;
const SALT_WARP_X: u32 = 0x4E41_5458;
const SALT_WARP_Z: u32 = 0x4E41_545A;

/// Replaces built-up land and cropland with natural cover. `heights` is the elevation grid
/// in blocks, aligned with the land cover; cells returned to water are sunk onto it.
/// `origin` is the absolute cell the grid starts at. `reclaim` returns bridge-like strips
/// to the water, for a world that draws no roads on them.
pub fn naturalize(
    lc: &mut LandCoverData,
    heights: Option<&mut [Vec<f32>]>,
    climate: Climate,
    origin: (i32, i32),
    reclaim: bool,
) {
    let (width, height) = (lc.width, lc.height);
    if width == 0 || height == 0 {
        return;
    }
    let mut built_up = false;
    for row in lc.grid.iter_mut() {
        for c in row.iter_mut() {
            if *c == LC_CROPLAND {
                *c = LC_GRASSLAND;
            }
            built_up |= *c == LC_BUILT_UP;
        }
    }
    if !built_up {
        return;
    }

    let heights = heights.filter(|h| h.len() >= height && h.iter().all(|r| r.len() >= width));
    if reclaim && reclaim_water(lc, heights) {
        lc.water_distance = compute_water_distance(&lc.grid, width, height);
        lc.invalidate_water_blend_grid();
    }
    fill_with_nearest_wild(lc, climate, origin);
}

/// Cover that grows back on its own. Bare ground counts only where it is the land's
/// natural state, so a rail yard in a temperate city does not spread rock.
fn is_wild(class: u8, climate: Climate) -> bool {
    match class {
        LC_TREE_COVER | LC_SHRUBLAND | LC_GRASSLAND | LC_MOSS => true,
        LC_BARE => matches!(
            climate,
            Climate::HotDesert | Climate::ColdDesert | Climate::Tundra | Climate::IceCap
        ),
        _ => false,
    }
}

/// Turns built-up land the water closes over back into water: cells that no square
/// [`WATER_SPAN_M`] across can cover without touching water. That takes bridge decks,
/// piers and stretches of river ESA read as town, but not a waterfront. Water spreads in
/// from beside them, only onto cells close to its surface. Returns whether any changed.
fn reclaim_water(lc: &mut LandCoverData, mut heights: Option<&mut [Vec<f32>]>) -> bool {
    let (width, height) = (lc.width, lc.height);
    let grid = &lc.grid;
    if !grid.iter().flatten().any(|&c| c == LC_WATER) {
        return false;
    }
    let r = ((WATER_SPAN_M / 2.0 * lc.cells_per_meter).round() as usize).max(1);

    // Closing: dilate the water by a square of radius `r`, then erode the result by it.
    let mut mask = vec![0u8; width * height];
    mask.par_chunks_mut(width)
        .zip(grid.par_iter())
        .for_each(|(m, row)| {
            for (m, &c) in m.iter_mut().zip(row) {
                *m = u8::from(c == LC_WATER);
            }
        });
    dilate_square(&mut mask, width, r, false);
    // Eroding is dilating the rest. Past the edge of the area is unknown, so it closes nothing.
    mask.par_iter_mut().for_each(|m| *m ^= 1);
    dilate_square(&mut mask, width, r, true);
    let closed = |x: usize, z: usize| mask[z * width + x] == 0 && grid[z][x] == LC_BUILT_UP;

    let levels = heights.as_deref();
    let level_at = |x: usize, z: usize| levels.map_or(f32::NAN, |h| h[z][x]);
    let neighbours = |x: usize, z: usize| {
        [
            (x.wrapping_sub(1), z),
            (x + 1, z),
            (x, z.wrapping_sub(1)),
            (x, z + 1),
        ]
        .into_iter()
        .filter(|&(nx, nz)| nx < width && nz < height)
    };
    // Seeded where the water touches, each with the level of the lowest water beside it.
    let (closed_ref, neighbours_ref, level_ref) = (&closed, &neighbours, &level_at);
    let mut frontier: Vec<(usize, usize, f32)> = (0..height)
        .into_par_iter()
        .flat_map_iter(|z| {
            (0..width).filter_map(move |x| {
                if !closed_ref(x, z) {
                    return None;
                }
                let level = neighbours_ref(x, z)
                    .filter(|&(nx, nz)| grid[nz][nx] == LC_WATER)
                    .map(|(nx, nz)| level_ref(nx, nz))
                    .reduce(min_level)?;
                reachable(level_ref(x, z), level).then_some((x, z, level))
            })
        })
        .collect();
    // Cell index to the level of the water it joins.
    let mut reclaimed: HashMap<usize, f32> = frontier
        .iter()
        .map(|&(x, z, level)| (z * width + x, level))
        .collect();
    let mut next: Vec<(usize, usize, f32)> = Vec::new();
    while !frontier.is_empty() {
        for &(x, z, level) in &frontier {
            for (nx, nz) in neighbours(x, z) {
                if closed(nx, nz)
                    && !reclaimed.contains_key(&(nz * width + nx))
                    && reachable(level_at(nx, nz), level)
                {
                    reclaimed.insert(nz * width + nx, level);
                    next.push((nx, nz, level));
                }
            }
        }
        std::mem::swap(&mut frontier, &mut next);
        next.clear();
    }
    drop(mask);

    for (&idx, &level) in &reclaimed {
        let (x, z) = (idx % width, idx / width);
        lc.grid[z][x] = LC_WATER;
        if let Some(h) = heights.as_mut() {
            let cell = &mut h[z][x];
            if level.is_finite() && cell.is_finite() && *cell > level {
                *cell = level;
            }
        }
    }
    !reclaimed.is_empty()
}

fn min_level(a: f32, b: f32) -> f32 {
    match (a.is_finite(), b.is_finite()) {
        (true, true) => a.min(b),
        (true, false) => a,
        _ => b,
    }
}

fn reachable(cell: f32, level: f32) -> bool {
    !cell.is_finite() || !level.is_finite() || cell <= level + WATER_HEIGHT_TOLERANCE_BLOCKS
}

/// Gives every remaining built-up cell the wild class nearest to it, found on a lattice of
/// ESA pixels and read through a smooth warp so the borders between classes bend. Both
/// are laid on absolute cells from `origin`, so neighbouring areas draw the same borders.
fn fill_with_nearest_wild(lc: &mut LandCoverData, climate: Climate, origin: (i32, i32)) {
    let (width, height) = (lc.width, lc.height);
    let cpm = lc.cells_per_meter.max(1e-6);
    let step = ((LATTICE_M * cpm).round() as usize).max(1);
    // How far into its lattice cell the grid starts.
    let px = origin.0.rem_euclid(step as i32) as usize;
    let pz = origin.1.rem_euclid(step as i32) as usize;
    let (lw, lh) = ((width + px).div_ceil(step), (height + pz).div_ceil(step));

    let mut label = vec![0u8; lw * lh];
    let mut dist = vec![u16::MAX; lw * lh];
    for lz in 0..lh {
        let z = (lz * step + step / 2).saturating_sub(pz).min(height - 1);
        for lx in 0..lw {
            let x = (lx * step + step / 2).saturating_sub(px).min(width - 1);
            let c = lc.grid[z][x];
            if is_wild(c, climate) {
                label[lz * lw + lx] = c;
                dist[lz * lw + lx] = 0;
            }
        }
    }
    chessboard_labels(&mut label, &mut dist, lw, lh);

    // Nothing wild anywhere in the area: plain ground for the climate.
    let fallback = if is_wild(LC_BARE, climate) {
        LC_BARE
    } else {
        LC_GRASSLAND
    };
    let warp = WARP_M * cpm;
    let wavelength = ((WARP_WAVELENGTH_M * cpm).round() as i32).max(1);
    let max_lx = (lw - 1) as f64;
    let max_lz = (lh - 1) as f64;
    lc.grid.par_iter_mut().enumerate().for_each(|(z, row)| {
        for (x, c) in row.iter_mut().enumerate() {
            if *c != LC_BUILT_UP {
                continue;
            }
            let (xi, zi) = (origin.0 + x as i32, origin.1 + z as i32);
            let wx = (value_noise_salted(xi, zi, wavelength, SALT_WARP_X) - 0.5) * 2.0 * warp;
            let wz = (value_noise_salted(xi, zi, wavelength, SALT_WARP_Z) - 0.5) * 2.0 * warp;
            let lx = (((x + px) as f64 + wx) / step as f64)
                .floor()
                .clamp(0.0, max_lx) as usize;
            let lz = (((z + pz) as f64 + wz) / step as f64)
                .floor()
                .clamp(0.0, max_lz) as usize;
            *c = match label[lz * lw + lx] {
                0 => fallback,
                l => l,
            };
        }
    });
}

/// Sets every cell within `r` cells of a set one by chessboard distance: a dilation by a
/// square, which splits into a pass along the rows and one down the columns. With `edge`,
/// cells past the border count as set. A square keeps a bridge whole up to the bank,
/// where a disc would leave a stub of deck at each end. Cells hold 0 or 1; bit 1 is
/// scratch for the result.
fn dilate_square(cells: &mut [u8], w: usize, r: usize, edge: bool) {
    let start = if edge { 0 } else { usize::MAX / 2 };
    let reach = |c: &mut u8, since: &mut usize| {
        *since = if *c & 1 != 0 { 0 } else { *since + 1 };
        if *since <= r {
            *c |= 2;
        }
    };
    cells.par_chunks_mut(w).for_each(|row| {
        let mut since = start;
        row.iter_mut().for_each(|c| reach(c, &mut since));
        let mut since = start;
        row.iter_mut().rev().for_each(|c| reach(c, &mut since));
        row.iter_mut().for_each(|c| *c >>= 1);
    });
    // Down the columns a row at a time, so each sweep reads memory in order.
    let mut since = vec![start; w];
    for row in cells.chunks_mut(w) {
        row.iter_mut()
            .zip(&mut since)
            .for_each(|(c, s)| reach(c, s));
    }
    since.fill(start);
    for row in cells.chunks_mut(w).rev() {
        row.iter_mut()
            .zip(&mut since)
            .for_each(|(c, s)| reach(c, s));
    }
    cells.par_iter_mut().for_each(|c| *c >>= 1);
}

/// Spreads each seed's label to the cells nearer to it than to any other seed, by
/// chessboard distance, in one forward and one backward raster pass.
fn chessboard_labels(label: &mut [u8], dist: &mut [u16], w: usize, h: usize) {
    let relax = |label: &mut [u8], dist: &mut [u16], i: usize, j: usize| {
        let d = dist[j].saturating_add(1);
        if d < dist[i] {
            dist[i] = d;
            label[i] = label[j];
        }
    };
    for z in 0..h {
        for x in 0..w {
            let i = z * w + x;
            if x > 0 {
                relax(label, dist, i, i - 1);
            }
            if z > 0 {
                relax(label, dist, i, i - w);
                if x > 0 {
                    relax(label, dist, i, i - w - 1);
                }
                if x + 1 < w {
                    relax(label, dist, i, i - w + 1);
                }
            }
        }
    }
    for z in (0..h).rev() {
        for x in (0..w).rev() {
            let i = z * w + x;
            if x + 1 < w {
                relax(label, dist, i, i + 1);
            }
            if z + 1 < h {
                relax(label, dist, i, i + w);
                if x + 1 < w {
                    relax(label, dist, i, i + w + 1);
                }
                if x > 0 {
                    relax(label, dist, i, i + w - 1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::OnceCell;

    fn lc_from_rows(rows: &[&str], cells_per_meter: f64) -> LandCoverData {
        let grid: Vec<Vec<u8>> = rows
            .iter()
            .map(|r| {
                r.chars()
                    .map(|ch| match ch {
                        'w' => LC_WATER,
                        'b' => LC_BUILT_UP,
                        'c' => LC_CROPLAND,
                        't' => LC_TREE_COVER,
                        'r' => LC_BARE,
                        _ => LC_GRASSLAND,
                    })
                    .collect()
            })
            .collect();
        let (width, height) = (grid[0].len(), grid.len());
        let water_distance = compute_water_distance(&grid, width, height);
        LandCoverData {
            grid,
            water_distance,
            water_blend_cache: OnceCell::new(),
            beach_tiles: OnceCell::new(),
            width,
            height,
            cells_per_meter,
        }
    }

    fn count(lc: &LandCoverData, class: u8) -> usize {
        lc.grid.iter().flatten().filter(|&&c| c == class).count()
    }

    #[test]
    fn fields_become_grassland() {
        let mut lc = lc_from_rows(&["ccgg", "cctt"], 1.0);
        naturalize(&mut lc, None, Climate::Temperate, (0, 0), true);
        assert_eq!(count(&lc, LC_CROPLAND), 0);
        assert_eq!(count(&lc, LC_GRASSLAND), 6);
        assert_eq!(count(&lc, LC_TREE_COVER), 2);
    }

    #[test]
    fn town_takes_the_surrounding_forest() {
        let rows: Vec<String> = (0..60)
            .map(|z| {
                (0..60)
                    .map(|x| {
                        if (20..40).contains(&x) && (20..40).contains(&z) {
                            'b'
                        } else {
                            't'
                        }
                    })
                    .collect()
            })
            .collect();
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let mut lc = lc_from_rows(&rows, 1.0);
        naturalize(&mut lc, None, Climate::Temperate, (0, 0), true);
        assert_eq!(count(&lc, LC_BUILT_UP), 0);
        assert_eq!(count(&lc, LC_TREE_COVER), 60 * 60);
    }

    #[test]
    fn town_with_nothing_wild_around_turns_to_grassland() {
        let mut lc = lc_from_rows(&["bbbb", "bbbb"], 1.0);
        naturalize(&mut lc, None, Climate::Temperate, (0, 0), true);
        assert_eq!(count(&lc, LC_GRASSLAND), 8);
        let mut lc = lc_from_rows(&["bbbb", "bbbb"], 1.0);
        naturalize(&mut lc, None, Climate::HotDesert, (0, 0), true);
        assert_eq!(count(&lc, LC_BARE), 8);
    }

    #[test]
    fn bare_ground_spreads_only_in_dry_or_cold_climates() {
        // Wider than the warp, which would otherwise shift the whole town at once.
        let rows: Vec<String> = (0..50)
            .map(|_| {
                (0..400)
                    .map(|x| match x {
                        0..100 => 'r',
                        300.. => 't',
                        _ => 'b',
                    })
                    .collect()
            })
            .collect();
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let mut temperate = lc_from_rows(&rows, 1.0);
        naturalize(&mut temperate, None, Climate::Temperate, (0, 0), true);
        assert_eq!(count(&temperate, LC_BARE), 100 * 50);
        let mut desert = lc_from_rows(&rows, 1.0);
        naturalize(&mut desert, None, Climate::HotDesert, (0, 0), true);
        assert!(count(&desert, LC_BARE) > 150 * 50);
        assert!(count(&desert, LC_TREE_COVER) > 150 * 50);
        assert_eq!(count(&desert, LC_BUILT_UP), 0);
    }

    #[test]
    fn overlapping_areas_draw_the_same_borders() {
        // Forest west of a town and grass east of it; the second area starts off the lattice.
        let class = |x: usize| match x {
            0..100 => 't',
            100..200 => 'b',
            _ => 'g',
        };
        let area = |x0: usize, z0: usize, w: usize, h: usize| {
            let rows: Vec<String> = (0..h).map(|_| (x0..x0 + w).map(class).collect()).collect();
            let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
            let mut lc = lc_from_rows(&rows, 1.0);
            naturalize(
                &mut lc,
                None,
                Climate::Temperate,
                (x0 as i32, z0 as i32),
                true,
            );
            lc
        };
        let whole = area(0, 0, 300, 260);
        let part = area(37, 23, 226, 200);
        for z in 0..200 {
            assert_eq!(part.grid[z][..], whole.grid[z + 23][37..263], "row {z}");
        }
        assert!(count(&whole, LC_TREE_COVER) > 100 * 260);
        assert!(count(&whole, LC_GRASSLAND) > 100 * 260);
    }

    /// Builds a grid from a per-cell class at 0.2 cells per metre, where
    /// [`WATER_SPAN_M`] is 10 cells and the closing reaches 5.
    fn coarse(w: usize, h: usize, f: impl Fn(usize, usize) -> char) -> LandCoverData {
        let rows: Vec<String> = (0..h).map(|z| (0..w).map(|x| f(x, z)).collect()).collect();
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        lc_from_rows(&rows, 0.2)
    }

    #[test]
    fn bridge_goes_back_to_the_river() {
        // A river running east to west with a 4-cell deck across it, towns on both banks.
        let mut lc = coarse(30, 30, |x, z| match z {
            0..=9 | 20.. => 'b',
            _ if (13..17).contains(&x) => 'b',
            _ => 'w',
        });
        let mut heights = vec![vec![10.0f32; 30]; 30];
        for row in &mut heights[10..20] {
            row[13..17].fill(12.0);
        }
        naturalize(
            &mut lc,
            Some(&mut heights),
            Climate::Temperate,
            (0, 0),
            true,
        );
        for (z, row) in heights.iter().enumerate().take(20).skip(10) {
            for (x, &h) in row.iter().enumerate().take(17).skip(13) {
                assert_eq!(lc.grid[z][x], LC_WATER, "deck cell ({x}, {z})");
                assert_eq!(h, 10.0);
            }
        }
        // The banks are a waterfront, so they stay land.
        for z in (0..10).chain(20..30) {
            assert_eq!(lc.grid[z][15], LC_GRASSLAND, "bank cell (15, {z})");
        }
        assert!(lc.water_distance[15][15] > 0);
    }

    #[test]
    fn river_read_as_town_is_closed() {
        // An 8-cell stretch of a 10-cell river that ESA classified as built-up.
        let mut lc = coarse(40, 30, |x, z| match (x, z) {
            (_, 0..=9 | 20..) => 'b',
            (15..23, _) => 'b',
            _ => 'w',
        });
        naturalize(&mut lc, None, Climate::Temperate, (0, 0), true);
        for z in 10..20 {
            for x in 15..23 {
                assert_eq!(lc.grid[z][x], LC_WATER, "gap cell ({x}, {z})");
            }
        }
        assert_eq!(lc.grid[5][19], LC_GRASSLAND);
        assert_eq!(lc.grid[25][19], LC_GRASSLAND);
    }

    #[test]
    fn high_crest_between_two_waters_stays_land() {
        // A dam between two lakes, far above them.
        let dam = |x: usize, _| if (13..17).contains(&x) { 'b' } else { 'w' };
        let mut lc = coarse(30, 30, dam);
        let mut heights = vec![vec![10.0f32; 30]; 30];
        for row in &mut heights {
            row[13..17].fill(40.0);
        }
        naturalize(
            &mut lc,
            Some(&mut heights),
            Climate::Temperate,
            (0, 0),
            true,
        );
        assert_eq!(lc.grid[15][14], LC_GRASSLAND);
        assert_eq!(heights[15][14], 40.0);

        // The same strip at the water's level is a causeway, and goes.
        let mut lc = coarse(30, 30, dam);
        let mut heights = vec![vec![10.0f32; 30]; 30];
        naturalize(
            &mut lc,
            Some(&mut heights),
            Climate::Temperate,
            (0, 0),
            true,
        );
        assert_eq!(lc.grid[15][14], LC_WATER);

        // A blueprint still draws the road along it, so it stays land.
        let mut lc = coarse(30, 30, dam);
        let mut heights = vec![vec![10.0f32; 30]; 30];
        naturalize(
            &mut lc,
            Some(&mut heights),
            Climate::Temperate,
            (0, 0),
            false,
        );
        assert_eq!(lc.grid[15][14], LC_GRASSLAND);
        assert_eq!(heights[15][14], 10.0);
    }

    #[test]
    fn wide_waterfront_stays_land() {
        // 20 cells of town between two waters is wider than any bridge.
        let mut lc = coarse(30, 30, |x, _| match x {
            0..5 | 25.. => 'w',
            _ => 'b',
        });
        naturalize(&mut lc, None, Climate::Temperate, (0, 0), true);
        assert_eq!(count(&lc, LC_WATER), 10 * 30);
        assert_eq!(count(&lc, LC_GRASSLAND), 20 * 30);
    }
}
