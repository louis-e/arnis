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
    // Separable: rows, then columns.
    let mut rows = vec![false; w * h];
    for z in 0..h {
        let row = &mask[z * w..(z + 1) * w];
        let mut last: isize = -(r as isize) - 1;
        // Forward pass remembers the last set cell, backward the next one.
        let mut fwd = vec![false; w];
        for x in 0..w {
            if row[x] {
                last = x as isize;
            }
            fwd[x] = x as isize - last <= r as isize;
        }
        let mut next: isize = (w + r + 1) as isize;
        for x in (0..w).rev() {
            if row[x] {
                next = x as isize;
            }
            rows[z * w + x] = fwd[x] || next - x as isize <= r as isize;
        }
    }
    let mut out = vec![false; w * h];
    for x in 0..w {
        let mut last: isize = -(r as isize) - 1;
        let mut fwd = vec![false; h];
        for z in 0..h {
            if rows[z * w + x] {
                last = z as isize;
            }
            fwd[z] = z as isize - last <= r as isize;
        }
        let mut next: isize = (h + r + 1) as isize;
        for z in (0..h).rev() {
            if rows[z * w + x] {
                next = z as isize;
            }
            out[z * w + x] = fwd[z] || next - z as isize <= r as isize;
        }
    }
    out
}

/// Mean of `v` over a `(2r+1)` square, the window clipped at the grid edge.
pub fn box_mean(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut sat = vec![0.0f64; (w + 1) * (h + 1)];
    for z in 0..h {
        let mut run = 0.0f64;
        for x in 0..w {
            run += f64::from(v[z * w + x]);
            sat[(z + 1) * (w + 1) + x + 1] = sat[z * (w + 1) + x + 1] + run;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for z in 0..h {
        let (z0, z1) = (z.saturating_sub(r), (z + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s = sat[z1 * (w + 1) + x1] - sat[z0 * (w + 1) + x1] - sat[z1 * (w + 1) + x0]
                + sat[z0 * (w + 1) + x0];
            out[z * w + x] = (s / ((z1 - z0) * (x1 - x0)) as f64) as f32;
        }
    }
    out
}

/// Median of `v` over the 3x3 window, counting only cells where `keep` holds.
pub fn median3_within(v: &[f32], keep: &[bool], w: usize, h: usize) -> Vec<f32> {
    let mut out = v.to_vec();
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
            out[z * w + x] = buf[buf.len() / 2];
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
    fn box_mean_clips_at_the_edge() {
        let v = vec![1.0f32; 16];
        assert!(box_mean(&v, 4, 4, 2)
            .iter()
            .all(|&m| (m - 1.0).abs() < 1e-6));
    }
}
