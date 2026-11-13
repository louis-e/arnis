//! Layout helpers for areas drawn along their outline: a rotated frame, a cell mask, hashing.

use crate::clipping::is_invented_node_id;
use crate::osm_parser::ProcessedNode;

/// An outline's frame: `along` its long axis, `across` it, with the outline's extents on both.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    pub along: (f64, f64),
    pub across: (f64, f64),
    pub along_range: (f64, f64),
    pub across_range: (f64, f64),
}

impl Frame {
    /// None for an outline without nodes.
    pub fn of(nodes: &[ProcessedNode]) -> Option<Self> {
        // Clipper-made edges skipped; 4x folding lets parallel and perpendicular edges agree.
        let (mut c, mut s) = (0.0f64, 0.0f64);
        for w in nodes.windows(2) {
            if is_invented_node_id(w[0].id) && is_invented_node_id(w[1].id) {
                continue;
            }
            let (dx, dz) = ((w[1].x - w[0].x) as f64, (w[1].z - w[0].z) as f64);
            let len = dx.hypot(dz);
            if len >= 1.0 {
                let a = 4.0 * dz.atan2(dx);
                c += len * a.cos();
                s += len * a.sin();
            }
        }
        let (sin, cos) = (s.atan2(c) / 4.0).sin_cos();
        let (u, v) = ((cos, sin), (-sin, cos));
        let extent = |d: (f64, f64)| {
            nodes.iter().fold((f64::MAX, f64::MIN), |(lo, hi), n| {
                let p = d.0 * n.x as f64 + d.1 * n.z as f64;
                (lo.min(p), hi.max(p))
            })
        };
        let (eu, ev) = (extent(u), extent(v));
        if eu.0 > eu.1 {
            return None;
        }
        Some(if eu.1 - eu.0 >= ev.1 - ev.0 {
            Self {
                along: u,
                across: v,
                along_range: eu,
                across_range: ev,
            }
        } else {
            Self {
                along: v,
                across: u,
                along_range: ev,
                across_range: eu,
            }
        })
    }

    pub fn length(&self) -> f64 {
        self.along_range.1 - self.along_range.0
    }

    pub fn width(&self) -> f64 {
        self.across_range.1 - self.across_range.0
    }

    /// A column's distance along and across from the outline's low corner.
    pub fn local(&self, x: i32, z: i32) -> (f64, f64) {
        let (fx, fz) = (x as f64, z as f64);
        (
            self.along.0 * fx + self.along.1 * fz - self.along_range.0,
            self.across.0 * fx + self.across.1 * fz - self.across_range.0,
        )
    }

    /// The column at `a` along and `c` across.
    pub fn column(&self, a: f64, c: f64) -> (i32, i32) {
        let (a, c) = (a + self.along_range.0, c + self.across_range.0);
        (
            (self.along.0 * a + self.across.0 * c).round() as i32,
            (self.along.1 * a + self.across.1 * c).round() as i32,
        )
    }

    /// The local ranges a block of columns spans, for visiting only what a tile can draw.
    pub fn local_span(
        &self,
        (min_x, min_z, max_x, max_z): (i32, i32, i32, i32),
    ) -> [(f64, f64); 2] {
        let mut span = [(f64::MAX, f64::MIN); 2];
        for (x, z) in [
            (min_x, min_z),
            (max_x, min_z),
            (min_x, max_z),
            (max_x, max_z),
        ] {
            let (a, c) = self.local(x, z);
            span[0] = (span[0].0.min(a), span[0].1.max(a));
            span[1] = (span[1].0.min(c), span[1].1.max(c));
        }
        span
    }

    /// The world axis nearest the long axis, as a unit step: rows of single blocks follow it.
    pub fn along_axis(&self) -> (i32, i32) {
        snap(self.along)
    }

    /// The world axis nearest the cross axis, as a unit step.
    pub fn across_axis(&self) -> (i32, i32) {
        snap(self.across)
    }
}

fn snap((dx, dz): (f64, f64)) -> (i32, i32) {
    if dx.abs() >= dz.abs() {
        (if dx >= 0.0 { 1 } else { -1 }, 0)
    } else {
        (0, if dz >= 0.0 { 1 } else { -1 })
    }
}

/// The columns of one area as a bitset over their bounds.
pub(crate) struct AreaMask {
    min_x: i32,
    min_z: i32,
    w: usize,
    h: usize,
    bits: Vec<u64>,
}

impl AreaMask {
    pub fn new(cells: &[(i32, i32)], mut keep: impl FnMut(i32, i32) -> bool) -> Self {
        let (mut min_x, mut min_z, mut max_x, mut max_z) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for &(x, z) in cells {
            min_x = min_x.min(x);
            min_z = min_z.min(z);
            max_x = max_x.max(x);
            max_z = max_z.max(z);
        }
        if min_x > max_x {
            return Self {
                min_x: 0,
                min_z: 0,
                w: 0,
                h: 0,
                bits: Vec::new(),
            };
        }
        let (w, h) = ((max_x - min_x + 1) as usize, (max_z - min_z + 1) as usize);
        let mut bits = vec![0u64; (w * h).div_ceil(64)];
        for &(x, z) in cells {
            if keep(x, z) {
                let i = (z - min_z) as usize * w + (x - min_x) as usize;
                bits[i >> 6] |= 1 << (i & 63);
            }
        }
        Self {
            min_x,
            min_z,
            w,
            h,
            bits,
        }
    }

    /// Low and high corners of the bounds.
    pub fn min(&self) -> (i32, i32) {
        (self.min_x, self.min_z)
    }

    pub fn max(&self) -> (i32, i32) {
        (
            self.min_x + self.w as i32 - 1,
            self.min_z + self.h as i32 - 1,
        )
    }

    #[inline]
    pub fn contains(&self, x: i32, z: i32) -> bool {
        let (dx, dz) = (x - self.min_x, z - self.min_z);
        if dx < 0 || dz < 0 || dx as usize >= self.w || dz as usize >= self.h {
            return false;
        }
        let i = dz as usize * self.w + dx as usize;
        self.bits[i >> 6] & (1 << (i & 63)) != 0
    }
}

pub(crate) fn centroid(nodes: &[ProcessedNode]) -> (f64, f64) {
    let n = nodes.len() as f64;
    (
        nodes.iter().map(|p| p.x as f64).sum::<f64>() / n,
        nodes.iter().map(|p| p.z as f64).sum::<f64>() / n,
    )
}

/// Even-odd over every edge of the rings, which also holds for rings split into pieces.
pub(crate) fn point_in_rings(rings: &[&[ProcessedNode]], (px, pz): (f64, f64)) -> bool {
    let mut inside = false;
    for ring in rings {
        for w in ring.windows(2) {
            let (ax, az) = (w[0].x as f64, w[0].z as f64);
            let (bx, bz) = (w[1].x as f64, w[1].z as f64);
            if (az > pz) != (bz > pz) && px < ax + (pz - az) * (bx - ax) / (bz - az) {
                inside = !inside;
            }
        }
    }
    inside
}

/// A point surely inside a ring, unlike its vertex average on a concave outline: the middle
/// of the widest span on a line across the ring's middle.
pub(crate) fn interior_point(nodes: &[ProcessedNode]) -> Option<(f64, f64)> {
    let (_, z0, _, z1) = bounds(nodes)?;
    // Off the integer rows the vertices sit on.
    let z = (z0 + z1) / 2.0 + 0.37 * (z1 - z0).min(1.0);
    let mut xs: Vec<f64> = nodes
        .windows(2)
        .filter_map(|w| {
            let (ax, az) = (w[0].x as f64, w[0].z as f64);
            let (bx, bz) = (w[1].x as f64, w[1].z as f64);
            ((az > z) != (bz > z)).then(|| ax + (z - az) * (bx - ax) / (bz - az))
        })
        .collect();
    xs.sort_by(f64::total_cmp);
    xs.as_chunks::<2>()
        .0
        .iter()
        .max_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0])))
        .map(|p| ((p[0] + p[1]) / 2.0, z))
}

/// Bounds of a ring as (min x, min z, max x, max z).
pub(crate) fn bounds(nodes: &[ProcessedNode]) -> Option<(f64, f64, f64, f64)> {
    let first = nodes.first()?;
    Some(nodes.iter().fold(
        (
            first.x as f64,
            first.z as f64,
            first.x as f64,
            first.z as f64,
        ),
        |(a, b, c, d), n| {
            let (x, z) = (n.x as f64, n.z as f64);
            (a.min(x), b.min(z), c.max(x), d.max(z))
        },
    ))
}

pub(crate) fn inside((x0, z0, x1, z1): (f64, f64, f64, f64), (x, z): (f64, f64)) -> bool {
    x >= x0 && x <= x1 && z >= z0 && z <= z1
}

pub(crate) fn mix(mut h: u64) -> u64 {
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

/// A hash of an element and a lattice point, for per-plot choices every tile agrees on.
pub(crate) fn cell_hash(id: u64, i: i64, j: i64) -> u64 {
    mix(id ^ mix((i as u64) ^ mix(j as u64 ^ 0x51ED)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn nodes(pts: &[(i32, i32)]) -> Vec<ProcessedNode> {
        pts.iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: i as u64 + 1,
                tags: HashMap::new(),
                x,
                z,
            })
            .collect()
    }

    #[test]
    fn a_frame_runs_along_the_long_side_of_a_tilted_strip() {
        // 100 long, 20 wide, tilted by atan(1/2).
        let f = Frame::of(&nodes(&[(0, 0), (90, 45), (81, 63), (-9, 18), (0, 0)])).unwrap();
        assert!((f.along.1 / f.along.0 - 0.5).abs() < 0.02, "{:?}", f.along);
        assert!(f.length() > 95.0 && f.width() < 25.0);
        let (a, c) = f.local(40, 31);
        assert!((a - 50.0).abs() < 2.0 && (c - 10.0).abs() < 2.0, "{a},{c}");
        assert_eq!(f.column(a, c), (40, 31));
        assert_eq!(f.along_axis(), (1, 0));
    }

    #[test]
    fn an_l_shape_has_its_interior_point_inside() {
        // An L whose vertex average falls in the missing corner.
        let l = nodes(&[
            (0, 0),
            (100, 0),
            (100, 10),
            (10, 10),
            (10, 100),
            (0, 100),
            (0, 0),
        ]);
        let avg = centroid(&l);
        assert!(!point_in_rings(&[l.as_slice()], avg), "{avg:?}");
        let p = interior_point(&l).unwrap();
        assert!(point_in_rings(&[l.as_slice()], p), "{p:?}");
    }

    #[test]
    fn a_mask_holds_what_it_was_given() {
        let cells = [(5, 5), (6, 5), (7, 9), (-3, 2)];
        let m = AreaMask::new(&cells, |x, _| x != 6);
        assert!(m.contains(5, 5) && m.contains(7, 9) && m.contains(-3, 2));
        assert!(!m.contains(6, 5), "filtered out");
        assert!(!m.contains(0, 0) && !m.contains(100, 100));
    }
}
