use super::rectangle::XZBBoxRect;
use crate::coordinate_system::cartesian::{XZPoint, XZVector};
use std::fmt;
use std::sync::Arc;

/// One drawn polygon in block space: its outer ring, then any holes, as (x, z) vertices.
pub type XZPolygon = Vec<Vec<(f64, f64)>>;

/// Columns some point of the drawn outline floors onto, as nodes do; even-odd per polygon, merged.
#[derive(Clone, Debug)]
pub struct XZBBoxShape {
    rect: XZBBoxRect,
    mask: Arc<SpanMask>,
}

#[derive(Debug)]
struct SpanMask {
    polygons: Vec<XZPolygon>,
    min_z: i32,
    /// Row `min_z + r`: `spans[offsets[r]..offsets[r + 1]]`, sorted disjoint inclusive x ranges.
    offsets: Vec<u32>,
    spans: Vec<(i32, i32)>,
}

impl XZBBoxShape {
    /// The outline's columns inside `rect`. Fails when none are left.
    pub fn new(rect: XZBBoxRect, polygons: Vec<XZPolygon>) -> Result<Self, String> {
        let polygons: Vec<XZPolygon> = polygons
            .into_iter()
            .map(|rings| {
                rings
                    .into_iter()
                    .map(|ring| ring.into_iter().map(|(x, z)| (snap(x), snap(z))).collect())
                    .collect()
            })
            .collect();
        if polygons
            .iter()
            .flatten()
            .flatten()
            .any(|&(x, z)| !x.is_finite() || !z.is_finite())
        {
            return Err("The drawn area has a broken point.".to_string());
        }
        let mask = SpanMask::build(&rect, polygons);
        if mask.spans.is_empty() {
            return Err("The drawn area holds no block.".to_string());
        }
        Ok(Self {
            rect,
            mask: Arc::new(mask),
        })
    }

    pub fn rect(&self) -> XZBBoxRect {
        self.rect
    }

    /// The outline in block space, as it was built.
    pub fn polygons(&self) -> &[XZPolygon] {
        &self.mask.polygons
    }

    #[inline]
    pub fn contains(&self, p: &XZPoint) -> bool {
        self.rect.contains(p) && self.mask.contains(p.x, p.z)
    }

    /// The same outline seen through a smaller rectangle, sharing the mask.
    pub fn crop(&self, rect: XZBBoxRect) -> Self {
        Self {
            rect,
            mask: Arc::clone(&self.mask),
        }
    }

    /// Whether every column of the inclusive rectangle lies in the outline.
    pub fn covers(&self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> bool {
        let r = self.rect;
        if min_x < r.min().x || max_x > r.max().x || min_z < r.min().z || max_z > r.max().z {
            return false;
        }
        (min_z..=max_z).all(|z| {
            let row = self.mask.row(z);
            let i = row.partition_point(|&(_, b)| b < min_x);
            row.get(i).is_some_and(|&(a, b)| a <= min_x && b >= max_x)
        })
    }

    /// Whether any column of the inclusive rectangle lies in the outline.
    pub fn touches(&self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> bool {
        let r = self.rect;
        let (min_x, max_x) = (min_x.max(r.min().x), max_x.min(r.max().x));
        let (min_z, max_z) = (min_z.max(r.min().z), max_z.min(r.max().z));
        if min_x > max_x || min_z > max_z {
            return false;
        }
        (min_z..=max_z).any(|z| {
            let row = self.mask.row(z);
            let i = row.partition_point(|&(_, b)| b < min_x);
            row.get(i).is_some_and(|&(a, _)| a <= max_x)
        })
    }

    /// Columns inside the outline and the rectangle.
    pub fn covered_columns(&self) -> u64 {
        let r = self.rect;
        (r.min().z..=r.max().z)
            .flat_map(|z| self.mask.row(z).iter())
            .map(|&(a, b)| {
                let (a, b) = (a.max(r.min().x), b.min(r.max().x));
                if a > b {
                    0
                } else {
                    (b - a + 1) as u64
                }
            })
            .sum()
    }

    /// A column near the north-west corner, a few blocks in where possible so spawn items land inside.
    pub fn first_inside(&self) -> Option<XZPoint> {
        let r = self.rect;
        for margin in [4, 1, 0] {
            let size = 2 * margin;
            for z in r.min().z..=r.max().z {
                for &(a, b) in self.mask.row(z) {
                    let (a, b) = (a.max(r.min().x), b.min(r.max().x));
                    // Slid along the row, as the rows below may start further in.
                    let mut x = a;
                    for _ in 0..256 {
                        if x + size > b {
                            break;
                        }
                        if self.covers(x, z, x + size, z + size) {
                            return Some(XZPoint::new(x + margin, z + margin));
                        }
                        x += margin.max(1);
                    }
                }
            }
        }
        None
    }

    /// The covered column nearest to `p`, which is `p` itself when it is inside.
    pub fn nearest_inside(&self, p: XZPoint) -> Option<XZPoint> {
        if self.contains(&p) {
            return Some(p);
        }
        let r = self.rect;
        let mut best: Option<(i64, XZPoint)> = None;
        for z in r.min().z..=r.max().z {
            let dz = (z - p.z) as i64;
            if best.is_some_and(|(d, _)| dz * dz >= d) {
                if z > p.z {
                    break;
                }
                continue;
            }
            for &(a, b) in self.mask.row(z) {
                let (a, b) = (a.max(r.min().x), b.min(r.max().x));
                if a > b {
                    continue;
                }
                let x = p.x.clamp(a, b);
                let dx = (x - p.x) as i64;
                let d = dx * dx + dz * dz;
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, XZPoint::new(x, z)));
                }
            }
        }
        best.map(|(_, q)| q)
    }

    fn translated(&self, v: XZVector) -> Self {
        let (dx, dz) = (v.dx as f64, v.dz as f64);
        let m = &self.mask;
        Self {
            rect: self.rect + v,
            mask: Arc::new(SpanMask {
                polygons: m
                    .polygons
                    .iter()
                    .map(|rings| {
                        rings
                            .iter()
                            .map(|ring| ring.iter().map(|&(x, z)| (x + dx, z + dz)).collect())
                            .collect()
                    })
                    .collect(),
                min_z: m.min_z + v.dz,
                offsets: m.offsets.clone(),
                spans: m.spans.iter().map(|&(a, b)| (a + v.dx, b + v.dx)).collect(),
            }),
        }
    }
}

impl fmt::Display for XZBBoxShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Shape({} polygons in {})",
            self.mask.polygons.len(),
            self.rect
        )
    }
}

impl std::ops::Add<XZVector> for XZBBoxShape {
    type Output = XZBBoxShape;

    fn add(self, other: XZVector) -> Self {
        self.translated(other)
    }
}

impl std::ops::Sub<XZVector> for XZBBoxShape {
    type Output = XZBBoxShape;

    fn sub(self, other: XZVector) -> Self {
        self.translated(XZVector {
            dx: -other.dx,
            dz: -other.dz,
        })
    }
}

/// Rounds away float noise, so a vertex the transform put a hair off a block edge sits on it.
fn snap(v: f64) -> f64 {
    let r = v.round();
    if (v - r).abs() < 1e-7 {
        r
    } else {
        v
    }
}

#[derive(Clone, Copy)]
struct Edge {
    z0: f64,
    z1: f64,
    x0: f64,
    x1: f64,
}

impl Edge {
    #[inline]
    fn x_at(&self, z: f64) -> f64 {
        if z == self.z0 {
            return self.x0;
        }
        if z == self.z1 {
            return self.x1;
        }
        self.x0 + (z - self.z0) * (self.x1 - self.x0) / (self.z1 - self.z0)
    }
}

impl SpanMask {
    #[inline]
    fn row(&self, z: i32) -> &[(i32, i32)] {
        let r = z as i64 - self.min_z as i64;
        if r < 0 || r as usize + 1 >= self.offsets.len() {
            return &[];
        }
        let r = r as usize;
        &self.spans[self.offsets[r] as usize..self.offsets[r + 1] as usize]
    }

    #[inline]
    fn contains(&self, x: i32, z: i32) -> bool {
        let row = self.row(z);
        let i = row.partition_point(|&(_, b)| b < x);
        row.get(i).is_some_and(|&(a, _)| a <= x)
    }

    fn build(rect: &XZBBoxRect, polygons: Vec<XZPolygon>) -> Self {
        let min_z = rect.min().z;
        let rows = (rect.max().z as i64 - min_z as i64 + 1) as usize;
        let mut cells: Vec<Vec<(i32, i32)>> = vec![Vec::new(); rows];
        let mut merged = vec![0u32; rows];
        for rings in &polygons {
            sweep(rings, rect, &mut cells, &mut merged);
        }

        let mut offsets = Vec::with_capacity(rows + 1);
        let mut spans: Vec<(i32, i32)> = Vec::new();
        offsets.push(0u32);
        for mut row in cells {
            merge(&mut row);
            spans.extend_from_slice(&row);
            offsets.push(spans.len() as u32);
        }
        Self {
            polygons,
            min_z,
            offsets,
            spans,
        }
    }
}

/// Sorts a row's ranges and joins those that overlap or touch.
fn merge(row: &mut Vec<(i32, i32)>) {
    row.sort_unstable();
    let mut out = 0;
    for i in 0..row.len() {
        let (a, b) = row[i];
        if out > 0 && a <= row[out - 1].1.saturating_add(1) {
            row[out - 1].1 = row[out - 1].1.max(b);
        } else {
            row[out] = (a, b);
            out += 1;
        }
    }
    row.truncate(out);
}

/// Adds one polygon's x extent to the rows of `rect`, slab by slab between vertex and whole heights;
/// a slab ending on a block edge also adds its top line to that row, as horizontal edges do theirs.
fn sweep(
    rings: &[Vec<(f64, f64)>],
    rect: &XZBBoxRect,
    cells: &mut [Vec<(i32, i32)>],
    merged: &mut [u32],
) {
    let min_z = rect.min().z as i64;
    let max_z = rect.max().z as i64;
    let (lo_x, hi_x) = (rect.min().x as f64, rect.max().x as f64);
    let mut add = |row: f64, lo: f64, hi: f64| {
        let row = row as i64;
        if row < min_z || row > max_z {
            return;
        }
        // Clamped in f64, so a far-off vertex cannot overflow the cast.
        let a = lo.min(hi).floor().max(lo_x) as i32;
        let b = lo.max(hi).floor().min(hi_x) as i32;
        if a > b {
            return;
        }
        let r = (row - min_z) as usize;
        let cells = &mut cells[r];
        cells.push((a, b));
        // Merged each time it doubles, so a row crossed many times stays near its true span count.
        if cells.len() >= 64 && cells.len() >= 2 * merged[r] as usize {
            merge(cells);
            merged[r] = cells.len() as u32;
        }
    };

    let mut edges: Vec<Edge> = Vec::new();
    let mut heights: Vec<f64> = Vec::new();
    for ring in rings {
        if ring.len() < 2 {
            continue;
        }
        for i in 0..ring.len() {
            let (xa, za) = ring[i];
            let (xb, zb) = ring[(i + 1) % ring.len()];
            heights.push(za);
            if za == zb {
                add(za.floor(), xa, xb);
            } else if za < zb {
                edges.push(Edge {
                    z0: za,
                    z1: zb,
                    x0: xa,
                    x1: xb,
                });
            } else {
                edges.push(Edge {
                    z0: zb,
                    z1: za,
                    x0: xb,
                    x1: xa,
                });
            }
        }
    }
    if edges.is_empty() {
        return;
    }

    let (lo, hi) = heights
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &z| {
            (lo.min(z), hi.max(z))
        });
    // Block edges outside the rectangle's rows add nothing, so only those inside split.
    let first = (lo.ceil() as i64).max(min_z);
    let last = (hi.floor() as i64).min(max_z + 1);
    if first <= last {
        heights.extend((first..=last).map(|z| z as f64));
    }
    heights.sort_unstable_by(f64::total_cmp);
    heights.dedup();

    edges.sort_unstable_by(|a, b| a.z0.total_cmp(&b.z0));
    let mut next = 0;
    let mut active: Vec<Edge> = Vec::new();
    let mut keyed: Vec<(f64, Edge)> = Vec::new();
    for pair in heights.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        while next < edges.len() && edges[next].z0 <= a {
            active.push(edges[next]);
            next += 1;
        }
        active.retain(|e| e.z1 > a);
        let row = a.floor();
        if row as i64 > max_z || b < min_z as f64 {
            continue;
        }
        let top_row = (b.fract() == 0.0).then_some(b);
        let mid = (a + b) / 2.0;
        keyed.clear();
        keyed.extend(active.iter().map(|e| (e.x_at(mid), *e)));
        keyed.sort_unstable_by(|p, q| p.0.total_cmp(&q.0));
        // Kept in this order, so the next slab sorts a list that is nearly sorted already.
        active.clear();
        active.extend(keyed.iter().map(|k| k.1));
        for &[(_, l), (_, r)] in keyed.as_chunks::<2>().0 {
            // A top on a block edge belongs to the row above, so a whole x it ends on is not reached here.
            let shy = |v: f64| if top_row.is_some() { v - 1e-9 } else { v };
            let (la, lb, ra, rb) = (l.x_at(a), l.x_at(b), r.x_at(a), r.x_at(b));
            let lo = la.min(lb).min(ra).min(rb);
            let hi = la.max(ra).max(shy(lb)).max(shy(rb));
            add(row, lo, hi);
            if let Some(top) = top_row {
                add(top, l.x_at(b), r.x_at(b));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: i32, z0: i32, x1: i32, z1: i32) -> XZBBoxRect {
        XZBBoxRect::new(XZPoint::new(x0, z0), XZPoint::new(x1, z1)).unwrap()
    }

    fn ring(points: &[(f64, f64)]) -> XZPolygon {
        vec![points.to_vec()]
    }

    fn cells(shape: &XZBBoxShape) -> Vec<(i32, i32)> {
        let r = shape.rect();
        let mut out = Vec::new();
        for z in r.min().z..=r.max().z {
            for x in r.min().x..=r.max().x {
                if shape.contains(&XZPoint::new(x, z)) {
                    out.push((x, z));
                }
            }
        }
        out
    }

    #[test]
    fn a_rectangle_drawn_as_a_shape_covers_the_rectangle() {
        let frame = rect(0, 0, 40, 25);
        let shape = XZBBoxShape::new(
            frame,
            vec![ring(&[(0.0, 0.0), (40.0, 0.0), (40.0, 25.0), (0.0, 25.0)])],
        )
        .unwrap();
        assert_eq!(shape.covered_columns(), 41 * 26);
        assert!(shape.covers(0, 0, 40, 25));
        for z in -1..=26 {
            for x in -1..=41 {
                let p = XZPoint::new(x, z);
                assert_eq!(shape.contains(&p), frame.contains(&p), "({x}, {z})");
            }
        }
    }

    #[test]
    fn winding_and_closing_point_do_not_matter() {
        let frame = rect(0, 0, 30, 30);
        let cw = XZBBoxShape::new(
            frame,
            vec![ring(&[(2.5, 3.5), (2.5, 20.2), (17.9, 20.2), (17.9, 3.5)])],
        )
        .unwrap();
        let ccw = XZBBoxShape::new(
            frame,
            vec![ring(&[
                (2.5, 3.5),
                (17.9, 3.5),
                (17.9, 20.2),
                (2.5, 20.2),
                (2.5, 3.5),
            ])],
        )
        .unwrap();
        assert_eq!(cells(&cw), cells(&ccw));
        // A point at 2.5 lands on block 2, one at 17.9 on block 17.
        assert!(cw.contains(&XZPoint::new(2, 3)) && cw.contains(&XZPoint::new(17, 20)));
        assert!(!cw.contains(&XZPoint::new(1, 10)) && !cw.contains(&XZPoint::new(18, 10)));
        assert!(!cw.contains(&XZPoint::new(10, 2)) && !cw.contains(&XZPoint::new(10, 21)));
        assert_eq!(cw.covered_columns(), 16 * 18);
    }

    #[test]
    fn an_l_shape_leaves_its_notch_out() {
        let frame = rect(0, 0, 20, 20);
        let shape = XZBBoxShape::new(
            frame,
            vec![ring(&[
                (0.0, 0.0),
                (20.0, 0.0),
                (20.0, 8.0),
                (8.0, 8.0),
                (8.0, 20.0),
                (0.0, 20.0),
            ])],
        )
        .unwrap();
        assert!(shape.contains(&XZPoint::new(20, 0)));
        assert!(shape.contains(&XZPoint::new(20, 8)));
        assert!(shape.contains(&XZPoint::new(8, 20)));
        assert!(!shape.contains(&XZPoint::new(9, 9)));
        assert!(!shape.contains(&XZPoint::new(20, 20)));
        assert_eq!(shape.covered_columns(), 21 * 9 + 9 * 12);
        assert!(shape.covers(0, 0, 15, 7));
        assert!(!shape.covers(0, 0, 15, 15));
        assert!(shape.touches(5, 5, 15, 15));
        assert!(!shape.touches(10, 10, 20, 20));
    }

    #[test]
    fn overlapping_shapes_merge_and_holes_stay_open() {
        let frame = rect(0, 0, 30, 30);
        let a = ring(&[(0.0, 0.0), (15.0, 0.0), (15.0, 15.0), (0.0, 15.0)]);
        let b = ring(&[(10.0, 10.0), (25.0, 10.0), (25.0, 25.0), (10.0, 25.0)]);
        let merged = XZBBoxShape::new(frame, vec![a, b]).unwrap();
        // Inside both: a second shape over the first must not cancel it.
        assert!(merged.contains(&XZPoint::new(12, 12)));
        assert_eq!(merged.covered_columns(), 2 * 16 * 16 - 6 * 6);

        let donut = vec![
            vec![(0.0, 0.0), (30.0, 0.0), (30.0, 30.0), (0.0, 30.0)],
            vec![(10.0, 10.0), (20.0, 10.0), (20.0, 20.0), (10.0, 20.0)],
        ];
        let donut = XZBBoxShape::new(frame, vec![donut]).unwrap();
        assert!(!donut.contains(&XZPoint::new(15, 15)));
        assert!(donut.contains(&XZPoint::new(5, 15)));
        // The hole's own edge belongs to the shape, as the outer edge does.
        assert!(donut.contains(&XZPoint::new(10, 15)));
    }

    #[test]
    fn disjoint_shapes_keep_the_gap_empty() {
        let frame = rect(0, 0, 100, 10);
        let shape = XZBBoxShape::new(
            frame,
            vec![
                ring(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]),
                ring(&[(90.0, 0.0), (100.0, 0.0), (100.0, 10.0), (90.0, 10.0)]),
            ],
        )
        .unwrap();
        assert!(!shape.contains(&XZPoint::new(50, 5)));
        assert!(!shape.touches(20, 0, 80, 10));
        assert_eq!(shape.covered_columns(), 2 * 11 * 11);
    }

    #[test]
    fn a_diagonal_edge_keeps_every_block_a_point_lands_on() {
        let frame = rect(0, 0, 50, 50);
        let tri = vec![(0.3, 0.2), (49.6, 7.1), (12.4, 48.9)];
        let shape = XZBBoxShape::new(frame, vec![ring(&tri)]).unwrap();
        // Every point of the triangle lands on a covered block.
        let n = 200;
        for i in 0..=n {
            for j in 0..=(n - i) {
                let (u, v) = (i as f64 / n as f64, j as f64 / n as f64);
                let w = 1.0 - u - v;
                let x = tri[0].0 * w + tri[1].0 * u + tri[2].0 * v;
                let z = tri[0].1 * w + tri[1].1 * u + tri[2].1 * v;
                let p = XZPoint::new(x.floor() as i32, z.floor() as i32);
                assert!(shape.contains(&p), "{x}, {z}");
            }
        }
        assert!(!shape.contains(&XZPoint::new(49, 49)));
        assert!(!shape.contains(&XZPoint::new(0, 49)));
    }

    #[test]
    fn a_line_ending_on_a_block_edge_does_not_reach_past_it() {
        let frame = rect(0, 0, 30, 30);
        let shape =
            XZBBoxShape::new(frame, vec![ring(&[(10.5, 12.0), (21.5, 5.0), (22.0, 8.0)])]).unwrap();
        // The edge reaches x = 22 only at z = 8, which is the next row.
        assert!(!shape.contains(&XZPoint::new(22, 7)));
        assert!(shape.contains(&XZPoint::new(22, 8)));
        assert!(shape.contains(&XZPoint::new(21, 7)));
    }

    #[test]
    fn a_shape_past_the_rectangle_is_cut_to_it() {
        let frame = rect(0, 0, 10, 10);
        let shape = XZBBoxShape::new(
            frame,
            vec![ring(&[(-5.0, -5.0), (5.0, -5.0), (5.0, 5.0), (-5.0, 5.0)])],
        )
        .unwrap();
        assert_eq!(shape.covered_columns(), 36);
        assert!(!shape.contains(&XZPoint::new(-1, 0)));
        let far = XZBBoxShape::new(frame, vec![ring(&[(1e12, 0.0), (2e12, 0.0), (2e12, 1e12)])]);
        assert!(far.is_err());
    }

    #[test]
    fn degenerate_outlines() {
        let frame = rect(0, 0, 10, 10);
        // A flat outline still lands on the blocks it runs over.
        assert!(XZBBoxShape::new(frame, vec![ring(&[(1.0, 1.0), (2.0, 1.0)])]).is_ok());
        assert!(XZBBoxShape::new(frame, vec![vec![]]).is_err());
        assert!(XZBBoxShape::new(frame, vec![ring(&[(f64::NAN, 1.0), (2.0, 1.0)])]).is_err());
    }

    #[test]
    fn moving_the_shape_moves_its_blocks() {
        let frame = rect(0, 0, 20, 20);
        let shape =
            XZBBoxShape::new(frame, vec![ring(&[(3.0, 3.0), (12.0, 4.0), (6.0, 15.0)])]).unwrap();
        let v = XZVector { dx: -7, dz: 11 };
        let moved = shape.clone() + v;
        let back = moved.clone() - v;
        assert_eq!(cells(&back), cells(&shape));
        for (x, z) in cells(&shape) {
            assert!(moved.contains(&XZPoint::new(x - 7, z + 11)));
        }
        assert_eq!(moved.polygons()[0][0][0], (-4.0, 14.0));
    }

    #[test]
    fn a_crop_keeps_only_its_part_of_the_outline() {
        let frame = rect(0, 0, 30, 30);
        let shape =
            XZBBoxShape::new(frame, vec![ring(&[(0.0, 0.0), (30.0, 0.0), (0.0, 30.0)])]).unwrap();
        let tile = shape.crop(rect(10, 10, 30, 30));
        assert!(tile.contains(&XZPoint::new(12, 12)));
        assert!(!tile.contains(&XZPoint::new(25, 25)));
        assert!(!tile.contains(&XZPoint::new(5, 5)));
    }

    #[test]
    fn the_nearest_column_inside_is_found() {
        let frame = rect(0, 0, 100, 10);
        let shape = XZBBoxShape::new(
            frame,
            vec![
                ring(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]),
                ring(&[(80.0, 0.0), (100.0, 0.0), (100.0, 10.0), (80.0, 10.0)]),
            ],
        )
        .unwrap();
        assert_eq!(
            shape.nearest_inside(XZPoint::new(50, 5)),
            Some(XZPoint::new(80, 5))
        );
        assert_eq!(
            shape.nearest_inside(XZPoint::new(20, 5)),
            Some(XZPoint::new(10, 5))
        );
        assert_eq!(
            shape.nearest_inside(XZPoint::new(5, 5)),
            Some(XZPoint::new(5, 5))
        );
    }

    #[test]
    fn the_spawn_column_is_inside() {
        let frame = rect(0, 0, 40, 40);
        let shape = XZBBoxShape::new(
            frame,
            vec![ring(&[
                (20.0, 0.0),
                (40.0, 20.0),
                (20.0, 40.0),
                (0.0, 20.0),
            ])],
        )
        .unwrap();
        let p = shape.first_inside().unwrap();
        assert!(shape.covers(p.x - 4, p.z - 4, p.x + 4, p.z + 4), "{p:?}");
        assert!(p.z <= 10, "{p:?}");
        // A west edge slanting away to the south-east still leaves room for the margin.
        let slant = XZBBoxShape::new(
            rect(0, 0, 400, 400),
            vec![ring(&[(0.0, 0.0), (400.0, 0.0), (400.0, 400.0)])],
        )
        .unwrap();
        let p = slant.first_inside().unwrap();
        assert!(slant.covers(p.x - 4, p.z - 4, p.x + 4, p.z + 4), "{p:?}");
        assert!(p.z <= 10 && p.x <= 20, "{p:?}");
        // A sliver too thin for the margin still gets a column of its own.
        let thin =
            XZBBoxShape::new(frame, vec![ring(&[(2.0, 2.0), (30.0, 2.0), (30.0, 3.0)])]).unwrap();
        assert!(thin.contains(&thin.first_inside().unwrap()));
    }
}
