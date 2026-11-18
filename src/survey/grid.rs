//! Small raster helpers for the survey: connected components, their shape, and
//! the few morphological filters the classifier needs. Row-major `w * h` grids.

/// Components of `mask`, labelled 1.. (0 is background). `diagonal` joins the
/// eight neighbours, otherwise only the four.
pub fn label(mask: &[bool], w: usize, h: usize, diagonal: bool) -> (Vec<u32>, u32) {
    let mut lab = vec![0u32; w * h];
    let mut n = 0u32;
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if !mask[start] || lab[start] != 0 {
            continue;
        }
        n += 1;
        lab[start] = n;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, z) = ((i % w) as isize, (i / w) as isize);
            for dz in -1isize..=1 {
                for dx in -1isize..=1 {
                    if (dx == 0 && dz == 0) || (!diagonal && dx != 0 && dz != 0) {
                        continue;
                    }
                    let (nx, nz) = (x + dx, z + dz);
                    if nx < 0 || nz < 0 || nx >= w as isize || nz >= h as isize {
                        continue;
                    }
                    let j = nz as usize * w + nx as usize;
                    if mask[j] && lab[j] == 0 {
                        lab[j] = n;
                        stack.push(j);
                    }
                }
            }
        }
    }
    (lab, n)
}

/// Area, long-axis length and implied width of each component (index 0 unused).
/// The length is that of the uniform bar with the same second moment, plus one
/// block, so a single straight row of `L` blocks measures `L`.
#[derive(Clone, Copy, Default, Debug)]
pub struct Shape {
    pub area: f64,
    pub length: f64,
    pub width: f64,
}

pub fn shapes(lab: &[u32], n: u32, w: usize) -> Vec<Shape> {
    let n = n as usize;
    let mut s = vec![[0.0f64; 6]; n + 1]; // count, sx, sz, sxx, szz, sxz
    for (i, &l) in lab.iter().enumerate() {
        if l == 0 {
            continue;
        }
        let (x, z) = ((i % w) as f64, (i / w) as f64);
        let a = &mut s[l as usize];
        a[0] += 1.0;
        a[1] += x;
        a[2] += z;
        a[3] += x * x;
        a[4] += z * z;
        a[5] += x * z;
    }
    s.iter()
        .map(|a| {
            let c = a[0];
            if c == 0.0 {
                return Shape::default();
            }
            let (mx, mz) = (a[1] / c, a[2] / c);
            let sxx = a[3] / c - mx * mx;
            let szz = a[4] / c - mz * mz;
            let sxz = a[5] / c - mx * mz;
            let tr = sxx + szz;
            let det = sxx * szz - sxz * sxz;
            let l1 = tr / 2.0 + (tr * tr / 4.0 - det).max(0.0).sqrt();
            let length = (12.0 * l1.max(0.0)).sqrt() + 1.0;
            Shape {
                area: c,
                length,
                width: c / length.max(1.0),
            }
        })
        .collect()
}

/// Square dilation with a `(2r+1)` window.
pub fn dilate(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    use rayon::prelude::*;
    if w == 0 {
        return Vec::new();
    }
    // Separable: along each row, then each row is the union of the rows
    // within reach above and below it. Rows side by side.
    let mut rows = vec![false; w * h];
    rows.par_chunks_mut(w)
        .zip(mask.par_chunks(w))
        .for_each(|(out, row)| {
            // Forward pass remembers the last set cell, backward the next one.
            let mut last: isize = -(r as isize) - 1;
            for x in 0..w {
                if row[x] {
                    last = x as isize;
                }
                out[x] = x as isize - last <= r as isize;
            }
            let mut next: isize = (w + r + 1) as isize;
            for x in (0..w).rev() {
                if row[x] {
                    next = x as isize;
                }
                out[x] |= next - x as isize <= r as isize;
            }
        });
    let mut out = vec![false; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(z, o)| {
        for near in z.saturating_sub(r)..(z + r + 1).min(h) {
            for (a, &b) in o.iter_mut().zip(&rows[near * w..(near + 1) * w]) {
                *a |= b;
            }
        }
    });
    out
}

/// Share of the cells set in `mask` over a `(2r+1)` square, the window clipped
/// at the grid edge, handed to `f(cell, share)` row by row. The counts are
/// slid along: only a row's worth of them is kept.
pub fn box_share(mask: &[bool], w: usize, h: usize, r: usize, mut f: impl FnMut(usize, f32)) {
    // Set cells per column over the window's rows `lo..hi`, and their running
    // sum along the row.
    let mut col = vec![0u32; w];
    let mut run = vec![0u32; w + 1];
    let (mut lo, mut hi) = (0, 0);
    for z in 0..h {
        let (z0, z1) = (z.saturating_sub(r), (z + r + 1).min(h));
        while hi < z1 {
            for (k, &m) in col.iter_mut().zip(&mask[hi * w..(hi + 1) * w]) {
                *k += u32::from(m);
            }
            hi += 1;
        }
        while lo < z0 {
            for (k, &m) in col.iter_mut().zip(&mask[lo * w..(lo + 1) * w]) {
                *k -= u32::from(m);
            }
            lo += 1;
        }
        for x in 0..w {
            run[x + 1] = run[x] + col[x];
        }
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s = f64::from(run[x1] - run[x0]);
            f(z * w + x, (s / ((z1 - z0) * (x1 - x0)) as f64) as f32);
        }
    }
}

/// Median of `v` over the 3x3 window, counting only cells where `keep` holds,
/// for each cell where it holds, in order.
pub fn median3_within(v: &[f32], keep: &[bool], w: usize, h: usize) -> Vec<f32> {
    let mut out = Vec::new();
    let mut buf = Vec::with_capacity(9);
    for z in 0..h {
        for x in 0..w {
            if !keep[z * w + x] {
                continue;
            }
            buf.clear();
            for dz in -1isize..=1 {
                for dx in -1isize..=1 {
                    let (nx, nz) = (x as isize + dx, z as isize + dz);
                    if nx < 0 || nz < 0 || nx >= w as isize || nz >= h as isize {
                        continue;
                    }
                    let j = nz as usize * w + nx as usize;
                    if keep[j] {
                        buf.push(v[j]);
                    }
                }
            }
            buf.sort_by(f32::total_cmp);
            out.push(buf[buf.len() / 2]);
        }
    }
    out
}

/// Median of `v` over each component (index 0 unused).
pub fn component_median(v: &[f32], lab: &[u32], n: u32) -> Vec<f32> {
    let mut groups: Vec<Vec<f32>> = vec![Vec::new(); n as usize + 1];
    for (i, &l) in lab.iter().enumerate() {
        if l != 0 {
            groups[l as usize].push(v[i]);
        }
    }
    groups
        .into_iter()
        .map(|mut g| {
            if g.is_empty() {
                return 0.0;
            }
            g.sort_by(f32::total_cmp);
            g[g.len() / 2]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_straight_row_is_long_and_thin() {
        let (w, h) = (12, 5);
        let mut m = vec![false; w * h];
        for x in 1..11 {
            m[2 * w + x] = true;
        }
        let (lab, n) = label(&m, w, h, true);
        assert_eq!(n, 1);
        let s = shapes(&lab, n, w)[1];
        assert_eq!(s.area, 10.0);
        // sqrt(12 * var) + 1 over ten unit steps: sqrt(99) + 1, the measure the
        // lab tuned its length rules on.
        assert!((s.length - 10.95).abs() < 0.01, "{}", s.length);
        assert!(s.width < 1.0);
    }

    #[test]
    fn diagonal_neighbours_join_only_when_asked() {
        let (w, h) = (3, 3);
        let m = vec![true, false, false, false, true, false, false, false, true];
        assert_eq!(label(&m, w, h, true).1, 1);
        assert_eq!(label(&m, w, h, false).1, 3);
    }

    #[test]
    fn dilation_grows_by_the_radius() {
        let (w, h) = (7, 7);
        let mut m = vec![false; w * h];
        m[3 * w + 3] = true;
        let d = dilate(&m, w, h, 2);
        assert_eq!(d.iter().filter(|&&b| b).count(), 25);
        assert!(!d[0]);
    }

    #[test]
    fn dilation_is_any_set_cell_within_reach() {
        let (w, h) = (37, 23);
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let m: Vec<bool> = (0..w * h)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed.is_multiple_of(11)
            })
            .collect();
        for r in 0..4 {
            let d = dilate(&m, w, h, r);
            for z in 0..h {
                for x in 0..w {
                    let any = (z.saturating_sub(r)..(z + r + 1).min(h)).any(|zz| {
                        (x.saturating_sub(r)..(x + r + 1).min(w)).any(|xx| m[zz * w + xx])
                    });
                    assert_eq!(d[z * w + x], any, "r {r} at {x},{z}");
                }
            }
        }
    }

    #[test]
    fn box_share_clips_at_the_edge() {
        let mut all = Vec::new();
        box_share(&[true; 16], 4, 4, 2, |c, s| all.push((c, s)));
        assert_eq!(all.len(), 16);
        assert!(all
            .iter()
            .enumerate()
            .all(|(i, &(c, s))| c == i && s == 1.0));
        // One set cell in a corner of a 7x7: the corner's clipped 3x3 window
        // holds it, the centre's full 5x5 does not.
        let (w, h) = (7, 7);
        let mut m = vec![false; w * h];
        m[0] = true;
        let mut share = vec![0.0f32; w * h];
        box_share(&m, w, h, 2, |c, s| share[c] = s);
        assert_eq!(share[0], 1.0 / 9.0);
        assert_eq!(share[2 * w + 2], 1.0 / 25.0);
        assert_eq!(share[3 * w + 3], 0.0);
        assert_eq!(share[w * h - 1], 0.0);
    }
}
