//! Straighten the ESA WorldCover shoreline below its 10 m pixel size.
//!
//! Sampled to blocks, an ESA shoreline is a staircase with 10-block steps that
//! a few-block blur only rounds off. Instead: trace the water mask boundary as
//! pixel rings, simplify each ring against a fitted line, rasterize back to the
//! block grid. Straight coasts collapse to one segment at any angle; a real
//! one-pixel bump or notch deviates far enough to survive.

use super::{nearest_land_class, EsaPixelRaster, GridMapping, LC_WATER};

/// Midpoint tolerance in ESA pixels: above a stair (0.5 px), below a real feature (1 px).
const MIDPOINT_TOLERANCE_PX: f64 = 0.72;

/// Corner tolerance in ESA pixels; stair corners reach 0.71 px. Catches corners in
/// two-edge runs, where any two midpoints fit a line.
const CORNER_TOLERANCE_PX: f64 = 1.0;

/// Below this many cells per pixel the stairs are too small to matter.
const MIN_CELLS_PER_PIXEL: f64 = 2.0;

/// The shoreline may move but the water area may not change much; more means a bug.
const MAX_AREA_CHANGE_FRACTION: f64 = 0.10;

/// Ring vertex in raster-local pixel-corner coordinates.
type Pt = (i32, i32);
/// Sub-pixel ring vertex in raster-local pixel coordinates.
type FPt = (f64, f64);

/// Replace the water mask in `grid`, which must be the nearest-neighbour sample of
/// `raster`, with its sub-pixel reconstruction. Other classes move only where a cell flips.
pub(super) fn reconstruct_water_shoreline(
    raster: &EsaPixelRaster,
    mapping: &GridMapping,
    grid: &mut [Vec<u8>],
) {
    if mapping.sx.max(mapping.sy) < MIN_CELLS_PER_PIXEL {
        return;
    }
    let (w, h) = (raster.width, raster.height);
    if w == 0 || h == 0 || mapping.grid_w == 0 || mapping.grid_h == 0 {
        return;
    }
    let water: Vec<bool> = raster.data.iter().map(|&c| c == LC_WATER).collect();
    if !water.iter().any(|&b| b) {
        return;
    }

    let Some(rings) = trace_boundaries(&water, w, h) else {
        eprintln!(
            "Shoreline reconstruction: boundary tracing failed; keeping the pixel-exact water mask"
        );
        return;
    };
    let simplified: Vec<Vec<FPt>> = rings.iter().map(|r| simplify_ring(r)).collect();
    let mask = rasterize_rings(&simplified, raster.x0, raster.y0, mapping);

    let (gw, gh) = (mapping.grid_w, mapping.grid_h);
    let before = grid
        .iter()
        .take(gh)
        .map(|row| row.iter().take(gw).filter(|&&c| c == LC_WATER).count())
        .sum::<usize>();
    let after = mask.iter().map(|b| b.count_ones() as usize).sum::<usize>();
    let allowed = MAX_AREA_CHANGE_FRACTION * before as f64 + 4.0 * mapping.sx * mapping.sy;
    if (after as f64 - before as f64).abs() > allowed {
        eprintln!(
            "Shoreline reconstruction: water area changed {} -> {} cells; keeping the pixel-exact water mask",
            before, after
        );
        return;
    }

    // Collect against the untouched grid so land classes come from the exact mask.
    let search_radius = ((1.5 * mapping.sx.max(mapping.sy)).ceil() as i32).clamp(2, 64);
    let mut to_land: Vec<(usize, usize, u8)> = Vec::new();
    let mut to_water: Vec<(usize, usize)> = Vec::new();
    for z in 0..gh {
        for x in 0..gw {
            let is_water = get_bit(&mask, z * gw + x);
            let cur = grid[z][x];
            if is_water && cur != LC_WATER {
                to_water.push((x, z));
            } else if !is_water && cur == LC_WATER {
                if let Some(c) = nearest_land_class(grid, gw, gh, x, z, search_radius) {
                    to_land.push((x, z, c));
                }
            }
        }
    }
    let (to_water_n, to_land_n) = (to_water.len(), to_land.len());
    for (x, z) in to_water {
        grid[z][x] = LC_WATER;
    }
    for (x, z, c) in to_land {
        grid[z][x] = c;
    }
    if to_water_n + to_land_n > 0 {
        eprintln!(
            "Shoreline reconstruction: {} rings, {} cells to water, {} cells to land",
            rings.len(),
            to_water_n,
            to_land_n
        );
    }
}

// ─── Boundary tracing ─────────────────────────────────────────────────────

/// Sides of a pixel, clockwise in screen coordinates (y down).
const TOP: u8 = 0;
const RIGHT: u8 = 1;
const BOTTOM: u8 = 2;
const LEFT: u8 = 3;

/// Start corner of a pixel side, walking clockwise around the pixel.
#[inline]
fn side_start(x: i32, y: i32, side: u8) -> Pt {
    match side {
        TOP => (x, y),
        RIGHT => (x + 1, y),
        BOTTOM => (x + 1, y + 1),
        _ => (x, y + 1),
    }
}

/// End corner of a pixel side, walking clockwise around the pixel.
#[inline]
fn side_end(x: i32, y: i32, side: u8) -> Pt {
    side_start(x, y, (side + 1) % 4)
}

/// Boundaries of the 4-connected water regions as closed pixel-corner rings, holes
/// included. `None` if a walk fails to close, so the caller can fall back.
fn trace_boundaries(water: &[bool], w: usize, h: usize) -> Option<Vec<Vec<Pt>>> {
    let (wi, hi) = (w as i32, h as i32);
    let is_water = |x: i32, y: i32| -> bool {
        x >= 0 && y >= 0 && x < wi && y < hi && water[y as usize * w + x as usize]
    };
    // A water pixel's side is boundary when the cell across it is land or outside.
    let is_boundary = |x: i32, y: i32, side: u8| -> bool {
        if !is_water(x, y) {
            return false;
        }
        let (nx, ny) = match side {
            TOP => (x, y - 1),
            RIGHT => (x + 1, y),
            BOTTOM => (x, y + 1),
            _ => (x - 1, y),
        };
        !is_water(nx, ny)
    };
    let edge_id =
        |x: i32, y: i32, side: u8| -> usize { (y as usize * w + x as usize) * 4 + side as usize };

    let mut visited = vec![0u64; (w * h * 4).div_ceil(64)];
    let mut rings: Vec<Vec<Pt>> = Vec::new();
    let max_steps = w * h * 4 + 4;

    for y in 0..hi {
        for x in 0..wi {
            if !water[y as usize * w + x as usize] {
                continue;
            }
            for side in 0..4u8 {
                if !is_boundary(x, y, side) || get_bit(&visited, edge_id(x, y, side)) {
                    continue;
                }
                let mut ring: Vec<Pt> = Vec::new();
                let (mut cx, mut cy, mut cs) = (x, y, side);
                let mut steps = 0usize;
                loop {
                    let id = edge_id(cx, cy, cs);
                    if get_bit(&visited, id) {
                        break;
                    }
                    set_bit(&mut visited, id);
                    push_merged(&mut ring, side_start(cx, cy, cs));

                    // Prefer this pixel's next side, so diagonal touches stay separate rings.
                    let next_side = (cs + 1) % 4;
                    if is_boundary(cx, cy, next_side) {
                        cs = next_side;
                    } else {
                        let (vx, vy) = side_end(cx, cy, cs);
                        let candidates = [
                            (vx - 1, vy - 1, BOTTOM),
                            (vx, vy - 1, LEFT),
                            (vx - 1, vy, RIGHT),
                            (vx, vy, TOP),
                        ];
                        let mut found: Option<(i32, i32, u8)> = None;
                        for (qx, qy, qs) in candidates {
                            if (qx, qy) == (cx, cy) || !is_boundary(qx, qy, qs) {
                                continue;
                            }
                            if found.is_some() {
                                return None;
                            }
                            found = Some((qx, qy, qs));
                        }
                        let (nx, ny, ns) = found?;
                        cx = nx;
                        cy = ny;
                        cs = ns;
                    }
                    steps += 1;
                    if steps > max_steps {
                        return None;
                    }
                }
                if (cx, cy, cs) != (x, y, side) {
                    return None;
                }
                close_merged(&mut ring);
                if ring.len() >= 3 {
                    rings.push(ring);
                }
            }
        }
    }
    Some(rings)
}

/// True if `b` lies on the straight axis-aligned run from `a` to `c`.
#[inline]
fn collinear_axis(a: Pt, b: Pt, c: Pt) -> bool {
    (a.0 == b.0 && b.0 == c.0) || (a.1 == b.1 && b.1 == c.1)
}

/// Append a vertex, dropping the previous one if it was a run midpoint.
#[inline]
fn push_merged(ring: &mut Vec<Pt>, p: Pt) {
    let n = ring.len();
    if n >= 2 && collinear_axis(ring[n - 2], ring[n - 1], p) {
        ring[n - 1] = p;
    } else {
        ring.push(p);
    }
}

/// Merge runs across the ring's start, which is an arbitrary boundary edge.
fn close_merged(ring: &mut Vec<Pt>) {
    loop {
        let n = ring.len();
        if n < 3 {
            return;
        }
        if collinear_axis(ring[n - 1], ring[0], ring[1]) {
            ring.remove(0);
        } else if collinear_axis(ring[n - 2], ring[n - 1], ring[0]) {
            ring.pop();
        } else {
            return;
        }
    }
}

// ─── Simplification ───────────────────────────────────────────────────────

/// Simplify a closed ring of pixel corners into sub-pixel vertices.
///
/// Douglas-Peucker recursion, but a run is accepted as straight when its edge
/// midpoints sit within tolerance of the run's fitted line rather than of the chord.
/// A stair oscillates symmetrically about the true edge, so the fit recovers it; a
/// real bump throws a midpoint a full pixel off and forces a split. Kept vertices
/// then move onto their two fitted lines, and each rendered segment is re-checked.
/// Rings of four or fewer vertices are single pixels and pass through.
fn simplify_ring(ring: &[Pt]) -> Vec<FPt> {
    let n = ring.len();
    let as_f = |p: Pt| (p.0 as f64, p.1 as f64);
    if n <= 4 {
        return ring.iter().map(|&p| as_f(p)).collect();
    }
    // Closed polyline: ext[n] == ext[0].
    let mut ext: Vec<Pt> = ring.to_vec();
    ext.push(ring[0]);
    // Seed the recursion at the first vertex and the one farthest from it so
    // both halves have distinct, well-separated endpoints.
    let p0 = ring[0];
    let mut k = 1;
    let mut best = -1i64;
    for (i, &p) in ring.iter().enumerate().skip(1) {
        let dx = (p.0 - p0.0) as i64;
        let dy = (p.1 - p0.1) as i64;
        let d = dx * dx + dy * dy;
        if d > best {
            best = d;
            k = i;
        }
    }
    let mut keep = vec![false; n + 1];
    keep[0] = true;
    keep[k] = true;
    keep[n] = true;
    let mut stack: Vec<(usize, usize)> = vec![(0, k), (k, n)];
    while let Some((i, j)) = stack.pop() {
        if j <= i + 1 {
            continue;
        }
        if let Some(m) = split_point(&ext[i..=j], None) {
            keep[i + m] = true;
            stack.push((i, i + m));
            stack.push((i + m, j));
        }
    }
    // Drop a seed whose neighbours merge into one run, else the trace start leaves a kink.
    for seed in [k, 0usize] {
        let kept: Vec<usize> = (0..n).filter(|&i| keep[i]).collect();
        if kept.len() <= 3 {
            break;
        }
        let pos = kept.iter().position(|&i| i == seed).unwrap();
        let prev = kept[(pos + kept.len() - 1) % kept.len()];
        let next = kept[(pos + 1) % kept.len()];
        if split_point(&cyclic_points(&ext, prev, next), None).is_none() {
            keep[seed] = false;
        }
    }
    // Place vertices on their fitted lines, then split any segment that still strays.
    // Each round only adds vertices, so this terminates.
    loop {
        let kept: Vec<usize> = (0..n).filter(|&i| keep[i]).collect();
        let m = kept.len();
        if m < 3 {
            return ring.iter().map(|&p| as_f(p)).collect();
        }
        let runs: Vec<Vec<Pt>> = (0..m)
            .map(|s| cyclic_points(&ext, kept[s], kept[(s + 1) % m]))
            .collect();
        let lines: Vec<FitLine> = runs.iter().map(|r| FitLine::fit_run(r)).collect();
        let placed: Vec<FPt> = (0..m)
            .map(|s| {
                let orig = as_f(ext[kept[s]]);
                lines[(s + m - 1) % m].join(&lines[s], orig)
            })
            .collect();
        let mut split_any = false;
        for s in 0..m {
            let (a, b) = (placed[s], placed[(s + 1) % m]);
            let rendered = FitLine::through(a, b);
            if let Some(local) = split_point(&runs[s], Some(&rendered)) {
                keep[(kept[s] + local) % n] = true;
                split_any = true;
            }
        }
        if !split_any {
            return placed;
        }
    }
}

/// Where to split a run, or `None` if it is straight against `against` (the rendered
/// segment) or against its own fitted line.
fn split_point(pts: &[Pt], against: Option<&FitLine>) -> Option<usize> {
    let n = pts.len();
    if n < 3 {
        return None;
    }
    let fitted;
    let line = match against {
        Some(l) => l,
        None => {
            fitted = FitLine::fit_run(pts);
            &fitted
        }
    };
    let mut worst_mid = 0.0f64;
    let mut worst_edge = 0;
    for e in 0..n - 1 {
        let (a, b) = (pts[e], pts[e + 1]);
        let mid = ((a.0 + b.0) as f64 * 0.5, (a.1 + b.1) as f64 * 0.5);
        let r = line.residual(mid);
        if r > worst_mid {
            worst_mid = r;
            worst_edge = e;
        }
    }
    let corner_res: Vec<f64> = pts
        .iter()
        .map(|&p| line.residual((p.0 as f64, p.1 as f64)))
        .collect();
    let mut worst_corner = -1.0;
    let mut worst_corner_at = 1;
    for (i, &r) in corner_res.iter().enumerate().take(n - 1).skip(1) {
        if r > worst_corner {
            worst_corner = r;
            worst_corner_at = i;
        }
    }
    let mut split = if worst_corner > CORNER_TOLERANCE_PX {
        worst_corner_at
    } else if worst_mid > MIDPOINT_TOLERANCE_PX {
        // Split at the offending edge, not the worst corner: on a 45-degree stair every
        // corner ties and the run would peel apart one stair at a time.
        let (u, v) = (worst_edge, worst_edge + 1);
        match (u >= 1, v <= n - 2) {
            (true, true) => {
                if corner_res[u] >= corner_res[v] {
                    u
                } else {
                    v
                }
            }
            (true, false) => u,
            (false, true) => v,
            (false, false) => return None,
        }
    } else {
        return None;
    };
    let worst_corner = corner_res[split];
    // Among near-tied corners take the one with the longest flanking edges: a stair never
    // has two long edges in a row, so this splits at the real corner.
    let flank = |i: usize| -> i64 {
        let (a, b, c) = (pts[i - 1], pts[i], pts[i + 1]);
        ((a.0 - b.0).abs() + (a.1 - b.1).abs() + (c.0 - b.0).abs() + (c.1 - b.1).abs()) as i64
    };
    let mut best_flank = flank(split);
    let lo = split.saturating_sub(2).max(1);
    let hi = (split + 2).min(n - 2);
    for (cand, &res) in corner_res.iter().enumerate().take(hi + 1).skip(lo) {
        if cand != split && res >= worst_corner - 0.75 {
            let f = flank(cand);
            if f > best_flank {
                best_flank = f;
                split = cand;
            }
        }
    }
    Some(split)
}

/// Vertices from index `i` to index `j` walking forward around the ring
/// (`ext` has the closing duplicate at the end), inclusive of both ends.
fn cyclic_points(ext: &[Pt], i: usize, j: usize) -> Vec<Pt> {
    let n = ext.len() - 1;
    if i < j {
        ext[i..=j].to_vec()
    } else {
        let mut v = ext[i..=n].to_vec();
        v.extend_from_slice(&ext[1..=j]);
        v
    }
}

/// A line through `(cx, cy)` with unit direction `(dx, dy)`.
struct FitLine {
    cx: f64,
    cy: f64,
    dx: f64,
    dy: f64,
}

impl FitLine {
    /// Orthogonal least-squares fit over edge midpoints, weighted by edge length.
    fn fit_run(pts: &[Pt]) -> Self {
        let n = pts.len();
        let a = (pts[0].0 as f64, pts[0].1 as f64);
        let b = (pts[n - 1].0 as f64, pts[n - 1].1 as f64);
        let through_ends = || {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt();
            if len > 0.0 {
                Self {
                    cx: a.0,
                    cy: a.1,
                    dx: dx / len,
                    dy: dy / len,
                }
            } else {
                Self {
                    cx: a.0,
                    cy: a.1,
                    dx: 1.0,
                    dy: 0.0,
                }
            }
        };
        if n < 3 {
            return through_ends();
        }
        let (mut wsum, mut cx, mut cy) = (0.0, 0.0, 0.0);
        let mids: Vec<(f64, f64, f64)> = (0..n - 1)
            .map(|e| {
                let (p, q) = (pts[e], pts[e + 1]);
                let w = (((q.0 - p.0) as f64).powi(2) + ((q.1 - p.1) as f64).powi(2)).sqrt();
                let m = ((p.0 + q.0) as f64 * 0.5, (p.1 + q.1) as f64 * 0.5);
                wsum += w;
                cx += w * m.0;
                cy += w * m.1;
                (m.0, m.1, w)
            })
            .collect();
        if wsum <= 0.0 {
            return through_ends();
        }
        cx /= wsum;
        cy /= wsum;
        let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
        for &(mx, my, w) in &mids {
            let (ux, uy) = (mx - cx, my - cy);
            sxx += w * ux * ux;
            syy += w * uy * uy;
            sxy += w * ux * uy;
        }
        if sxx + syy <= 1e-12 {
            return through_ends();
        }
        let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        Self {
            cx,
            cy,
            dx: theta.cos(),
            dy: theta.sin(),
        }
    }

    /// Perpendicular distance of a point from the line.
    #[inline]
    fn residual(&self, p: FPt) -> f64 {
        ((p.0 - self.cx) * self.dy - (p.1 - self.cy) * self.dx).abs()
    }

    /// Line through two points (a degenerate pair gets a horizontal line).
    fn through(a: FPt, b: FPt) -> Self {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len > 0.0 {
            Self {
                cx: a.0,
                cy: a.1,
                dx: dx / len,
                dy: dy / len,
            }
        } else {
            Self {
                cx: a.0,
                cy: a.1,
                dx: 1.0,
                dy: 0.0,
            }
        }
    }

    /// Foot of the perpendicular from `p` onto the line.
    fn project(&self, p: FPt) -> FPt {
        let t = (p.0 - self.cx) * self.dx + (p.1 - self.cy) * self.dy;
        (self.cx + t * self.dx, self.cy + t * self.dy)
    }

    /// Where a vertex shared by two segments belongs: their intersection when usable,
    /// else midway between the projections, unless `orig` is off both lines (a real corner).
    fn join(&self, next: &FitLine, orig: FPt) -> FPt {
        let cross = self.dx * next.dy - self.dy * next.dx;
        if cross.abs() >= 0.087 {
            let (wx, wy) = (next.cx - self.cx, next.cy - self.cy);
            let t = (wx * next.dy - wy * next.dx) / cross;
            let p = (self.cx + t * self.dx, self.cy + t * self.dy);
            if ((p.0 - orig.0).powi(2) + (p.1 - orig.1).powi(2)).sqrt() <= 1.0 {
                return p;
            }
        }
        if self.residual(orig) > MIDPOINT_TOLERANCE_PX
            || next.residual(orig) > MIDPOINT_TOLERANCE_PX
        {
            return orig;
        }
        let a = self.project(orig);
        let b = next.project(orig);
        ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
    }
}

// ─── Rasterization ────────────────────────────────────────────────────────

/// Even-odd scanline fill of the rings onto the block grid, as a row-major bitset.
/// `px_x0`/`px_y0` shift raster-local vertices into global ESA pixel coordinates.
fn rasterize_rings(rings: &[Vec<FPt>], px_x0: i64, px_y0: i64, mapping: &GridMapping) -> Vec<u64> {
    let (gw, gh) = (mapping.grid_w, mapping.grid_h);
    let mut mask = vec![0u64; (gw * gh).div_ceil(64)];
    if gw == 0 || gh == 0 {
        return mask;
    }
    let mut rows: Vec<Vec<f64>> = vec![Vec::new(); gh];
    for ring in rings {
        let n = ring.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let a = ring[i];
            let b = ring[(i + 1) % n];
            let ay = mapping.gz(a.1 + px_y0 as f64);
            let by = mapping.gz(b.1 + px_y0 as f64);
            if ay == by {
                continue;
            }
            let ax = mapping.gx(a.0 + px_x0 as f64);
            let bx = mapping.gx(b.0 + px_x0 as f64);
            let (y_lo, y_hi) = if ay < by { (ay, by) } else { (by, ay) };
            // Half-open y_lo..y_hi, matching the nearest-neighbour cell assignment.
            let z_start = y_lo.ceil().max(0.0) as i64;
            let z_end = (y_hi.ceil() as i64).min(gh as i64);
            let mut z = z_start;
            while z < z_end {
                let t = (z as f64 - ay) / (by - ay);
                rows[z as usize].push(ax + t * (bx - ax));
                z += 1;
            }
        }
    }
    for (z, xs) in rows.iter_mut().enumerate() {
        if xs.len() < 2 {
            continue;
        }
        xs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        let mut i = 0;
        while i + 1 < xs.len() {
            let x_start = xs[i].ceil().max(0.0) as i64;
            let x_end = (xs[i + 1].ceil() as i64).min(gw as i64);
            let mut x = x_start;
            while x < x_end {
                set_bit(&mut mask, z * gw + x as usize);
                x += 1;
            }
            i += 2;
        }
    }
    mask
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
    use crate::land_cover::{LC_GRASSLAND, LC_TREE_COVER};

    /// Raster of `w`x`h` pixels; `mapping` puts `cells` grid cells on every
    /// pixel along both axes, with the grid origin on the raster origin.
    fn setup(w: usize, h: usize, cells_x: f64, cells_y: f64) -> (EsaPixelRaster, GridMapping) {
        let raster = EsaPixelRaster {
            x0: 1000,
            y0: 2000,
            width: w,
            height: h,
            ppd: 12000.0,
            data: vec![LC_GRASSLAND; w * h],
        };
        let mapping = GridMapping {
            x_origin: 1000.0,
            y_origin: 2000.0,
            sx: cells_x,
            sy: cells_y,
            grid_w: (w as f64 * cells_x).round() as usize,
            grid_h: (h as f64 * cells_y).round() as usize,
        };
        (raster, mapping)
    }

    fn water_cells(grid: &[Vec<u8>]) -> usize {
        grid.iter()
            .map(|r| r.iter().filter(|&&c| c == LC_WATER).count())
            .sum()
    }

    fn run(raster: &EsaPixelRaster, mapping: &GridMapping) -> Vec<Vec<u8>> {
        let mut grid = raster.sample_grid(mapping);
        reconstruct_water_shoreline(raster, mapping, &mut grid);
        grid
    }

    #[test]
    fn unsimplified_rings_reproduce_the_nearest_neighbour_mask() {
        // Trace and rasterize with no simplification must reproduce the exact mask.
        let (mut raster, mapping) = setup(23, 17, 3.7, 5.2);
        let mut seed = 0x9E3779B97F4A7C15u64;
        for v in raster.data.iter_mut() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            if seed % 5 < 2 {
                *v = LC_WATER;
            }
        }
        let grid = raster.sample_grid(&mapping);
        let water: Vec<bool> = raster.data.iter().map(|&c| c == LC_WATER).collect();
        let rings = trace_boundaries(&water, raster.width, raster.height).expect("trace");
        let exact: Vec<Vec<FPt>> = rings
            .iter()
            .map(|r| r.iter().map(|&(x, y)| (x as f64, y as f64)).collect())
            .collect();
        let mask = rasterize_rings(&exact, raster.x0, raster.y0, &mapping);
        for (z, row) in grid.iter().enumerate() {
            for (x, &cell) in row.iter().enumerate() {
                assert_eq!(
                    get_bit(&mask, z * mapping.grid_w + x),
                    cell == LC_WATER,
                    "mismatch at ({x},{z})"
                );
            }
        }
    }

    #[test]
    fn straight_coasts_become_straight_at_any_angle() {
        // A rasterized half-plane must come back as one straight edge at every angle.
        for angle_deg in [
            3.0f64, 8.0, 15.0, 22.0, 30.0, 38.0, 45.0, 52.0, 60.0, 68.0, 75.0, 82.0, 87.0,
        ] {
            let (mut raster, mapping) = setup(40, 40, 10.0, 10.0);
            let (s, c) = angle_deg.to_radians().sin_cos();
            // Water where the pixel centre is below the line through (20,20).
            let inside = |x: f64, y: f64| (x - 20.0) * s - (y - 20.0) * c < 0.0;
            for py in 0..40 {
                for px in 0..40 {
                    if inside(px as f64 + 0.5, py as f64 + 0.5) {
                        raster.data[py * 40 + px] = LC_WATER;
                    }
                }
            }
            let water: Vec<bool> = raster.data.iter().map(|&c| c == LC_WATER).collect();
            let rings = trace_boundaries(&water, 40, 40).unwrap();
            assert_eq!(rings.len(), 1, "angle {angle_deg}: one ring");
            let simplified = simplify_ring(&rings[0]);
            assert!(
                simplified.len() <= 5,
                "angle {angle_deg}: {} vertices left after RDP",
                simplified.len()
            );
            let grid = run(&raster, &mapping);
            // Every water/land boundary cell must be within one pixel of the line.
            let mut worst = 0.0f64;
            for z in 1..mapping.grid_h - 1 {
                for x in 1..mapping.grid_w - 1 {
                    let here = grid[z][x] == LC_WATER;
                    if here != (grid[z][x + 1] == LC_WATER) || here != (grid[z + 1][x] == LC_WATER)
                    {
                        // Cell (x, z) sits at pixel coordinate (x/10, z/10).
                        let d = ((x as f64 / 10.0 - 20.0) * s - (z as f64 / 10.0 - 20.0) * c).abs();
                        worst = worst.max(d);
                    }
                }
            }
            assert!(
                worst <= 1.0,
                "angle {angle_deg}: shoreline strays {worst} px from the line"
            );
        }
    }

    #[test]
    fn single_pixel_bumps_and_notches_survive_on_straight_coasts() {
        // A one-pixel bump or notch must survive once its centre is clearly on the wrong
        // side of the line. A stair corner already reaches 1/sqrt(2) = 0.71 px at 45
        // degrees, so only past that is a feature something the fit can tell apart.
        let mut checked = 0;
        for angle_deg in [0.0f64, 12.0, 30.0, 45.0, 63.0, 80.0, 90.0] {
            let (s, c) = angle_deg.to_radians().sin_cos();
            // Signed distance of a point from the line; > 0 is the land side.
            let signed = |x: f64, y: f64| (x - 20.0) * s - (y - 20.0) * c;
            let base = |raster: &mut EsaPixelRaster| {
                for py in 0..40 {
                    for px in 0..40 {
                        if signed(px as f64 + 0.5, py as f64 + 0.5) < 0.0 {
                            raster.data[py * 40 + px] = LC_WATER;
                        }
                    }
                }
            };
            for (px, py) in (0..40).flat_map(|px| (0..40).map(move |py| (px, py))) {
                let d = signed(px as f64 + 0.5, py as f64 + 0.5);
                // Bump: land pixel touching water on the land side. Notch: the mirror.
                let (is_bump, is_notch) = ((0.8..1.0).contains(&d), (-1.0..=-0.8).contains(&d));
                if !(is_bump || is_notch) || !(3..=36).contains(&px) || !(3..=36).contains(&py) {
                    continue;
                }
                let (mut raster, mapping) = setup(40, 40, 10.0, 10.0);
                base(&mut raster);
                let touches_shore =
                    [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .any(|(dx, dy)| {
                            let (nx, ny) = (px as i32 + dx, py as i32 + dy);
                            (raster.data[ny as usize * 40 + nx as usize] == LC_WATER) == is_bump
                        });
                if !touches_shore {
                    continue;
                }
                raster.data[py * 40 + px] = if is_bump { LC_WATER } else { LC_TREE_COVER };
                let grid = run(&raster, &mapping);
                let centre = grid[py * 10 + 5][px * 10 + 5];
                if is_bump {
                    assert_eq!(
                        centre, LC_WATER,
                        "angle {angle_deg}: bump at ({px},{py}) d={d:.2} lost"
                    );
                } else {
                    assert_ne!(
                        centre, LC_WATER,
                        "angle {angle_deg}: notch at ({px},{py}) d={d:.2} filled"
                    );
                }
                let water: Vec<bool> = raster.data.iter().map(|&c| c == LC_WATER).collect();
                let rings = trace_boundaries(&water, 40, 40).unwrap();
                let verts: usize = rings.iter().map(|r| simplify_ring(r).len()).sum();
                assert!(verts <= 14, "angle {angle_deg}: {verts} vertices");
                checked += 1;
            }
        }
        assert!(checked >= 20, "only {checked} features exercised");
    }

    #[test]
    fn single_pixel_pond_and_island_survive() {
        let (mut raster, mapping) = setup(12, 12, 8.0, 8.0);
        // Pond: one water pixel in land.
        raster.data[3 * 12 + 3] = LC_WATER;
        // Lake with a one-pixel island.
        for py in 6..11 {
            for px in 6..11 {
                raster.data[py * 12 + px] = LC_WATER;
            }
        }
        raster.data[8 * 12 + 8] = LC_TREE_COVER;
        let grid = run(&raster, &mapping);
        // The pond keeps its full pixel footprint (8x8 cells).
        let pond: usize = (24..32)
            .map(|z| (24..32).filter(|&x| grid[z][x] == LC_WATER).count())
            .sum();
        assert_eq!(pond, 64);
        // The island still exists and kept its class.
        assert_eq!(grid[68][68], LC_TREE_COVER);
        assert_eq!(grid[68][60], LC_WATER);
    }

    #[test]
    fn one_pixel_river_keeps_its_area() {
        let (mut raster, mapping) = setup(30, 10, 6.0, 9.0);
        // A river one pixel wide, running diagonally-ish across the raster.
        for px in 2..28 {
            let py = 2 + (px * 5) / 26;
            raster.data[py * 30 + px] = LC_WATER;
        }
        let nn = raster.sample_grid(&mapping);
        let grid = run(&raster, &mapping);
        let before = water_cells(&nn);
        let after = water_cells(&grid);
        assert!(after > 0);
        let ratio = after as f64 / before as f64;
        assert!((0.8..=1.2).contains(&ratio), "river area ratio {ratio}");
        // Land cells that turned to water and back must carry a land class.
        assert!(grid.iter().flatten().all(|&c| c != 0));
    }

    #[test]
    fn coarse_grid_is_left_alone() {
        // Grid coarser than 2 cells/pixel: reconstruction is a no-op.
        let (mut raster, mapping) = setup(20, 20, 1.5, 1.5);
        for py in 5..15 {
            for px in 0..20 {
                if px + py < 18 {
                    raster.data[py * 20 + px] = LC_WATER;
                }
            }
        }
        let nn = raster.sample_grid(&mapping);
        let grid = run(&raster, &mapping);
        assert_eq!(nn, grid);
    }

    #[test]
    fn diagonal_pixels_trace_as_separate_rings() {
        let water = vec![true, false, false, true];
        let rings = trace_boundaries(&water, 2, 2).unwrap();
        assert_eq!(rings.len(), 2);
        assert!(rings.iter().all(|r| r.len() == 4));
    }
}
