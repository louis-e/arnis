//! The levels a tunnel keeps from portal to portal: a steady grade, bent only where the ground
//! over it falls away and the bore has to stay under its cover, as a real one is driven.

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use fnv::{FnvHashMap, FnvHashSet};

/// Blocks a station hall's floor keeps over the world's floor. The terrain fit makes that
/// room for halls and for bores under water; elsewhere a bore goes no deeper than its cover.
pub(crate) const FLOOR_GUARD: i32 = 3;

/// The lowest a station hall's floor may lie.
pub(crate) fn lowest_floor() -> i32 {
    crate::world_editor::min_y() + FLOOR_GUARD
}

/// Covered cells a portal's cutting carries on past, a bump in the ground and no more.
pub(crate) const CUTTING_GAP: usize = 5;

/// How a bore keeps to its grade and its cover.
pub(crate) struct Bore {
    /// Steepest grade the line takes, in blocks per cell, unless its pins ask for more.
    pub grade: f64,
    /// Cover the bore wants under the ground, which leaving a portal it has within
    /// `depth / grade` cells.
    pub depth: f64,
    /// The lowest a level may lie.
    pub lowest: f64,
}

impl Bore {
    /// A bore of `cells`, at `least` grade when long, up to `most` when short, so a short
    /// one is under `depth` a quarter of the way in from either portal.
    pub(crate) fn new(cells: usize, depth: i32, least: f64, most: f64, lowest: i32) -> Self {
        let span = cells.saturating_sub(1).max(1) as f64;
        Self {
            grade: (4.0 * f64::from(depth) / span).clamp(least, most),
            depth: f64::from(depth),
            lowest: f64::from(lowest),
        }
    }
}

/// The line a tunnel's levels follow along its cells. `ceiling` is the highest level each cell
/// keeps its cover at and `top` the highest it may ever take: the ground, or under water the
/// cover. `pins` are the levels it has to take and `portal` says which ends open to the
/// surface. A free end keeps level with the pin before it unless its cover asks for lower.
pub(crate) fn grade_line(
    ceiling: &[f64],
    top: &[f64],
    pins: &[Option<f64>],
    portal: [bool; 2],
    bore: &Bore,
) -> Vec<f64> {
    let n = ceiling.len();
    if n == 0 {
        return Vec::new();
    }
    let at_portal = |i: usize| (portal[0] && i <= 1) || (portal[1] && i + 2 >= n);
    // Leaving any other pin the bore may run at the grade before it is under its cover.
    let pin_at = |i: usize| match pins[i] {
        Some(p) if !at_portal(i) => p,
        _ => f64::NEG_INFINITY,
    };
    let mut cone = vec![f64::NEG_INFINITY; n];
    let mut c = f64::NEG_INFINITY;
    for (i, slot) in cone.iter_mut().enumerate() {
        c = (c - bore.grade).max(pin_at(i));
        *slot = c;
    }
    c = f64::NEG_INFINITY;
    for i in (0..n).rev() {
        c = (c - bore.grade).max(pin_at(i));
        cone[i] = cone[i].max(c);
    }
    let base: Vec<f64> = (0..n)
        .map(|i| ceiling[i].max(cone[i]).max(bore.lowest))
        .collect();
    let top: Vec<f64> = top.iter().map(|t| t.max(bore.lowest)).collect();

    // A portal's cutting needs no cover: from the portal in to where the line first has it,
    // and no further than the grade takes it under its depth.
    let full = (bore.depth / bore.grade - 1e-9).ceil().max(1.0) as usize;
    let mut reach = portal.map(|p| if p { full } else { 0 });
    // Too short for both cuttings, each takes half, so the middle still goes under its cover.
    if reach[0] + reach[1] + 2 > n && portal == [true; 2] {
        let room = n.saturating_sub(2);
        reach = [room.div_ceil(2), room / 2];
    }
    let cell = |e: usize, d: usize| if e == 0 { d } else { n - 1 - d };
    let mut line = Vec::new();
    for _ in 0..8 {
        let cap: Vec<f64> = (0..n)
            .map(|i| {
                if i < reach[0] || n - 1 - i < reach[1] {
                    f64::INFINITY
                } else {
                    base[i]
                }
            })
            .collect();
        line = fit(&cap, &top, pins, bore.grade);
        let mut moved = false;
        for e in 0..2 {
            let covered = |d: usize| line[cell(e, d)] <= base[cell(e, d)];
            if let Some(d) = (1..reach[e].min(n)).find(|&d| covered(d)) {
                reach[e] = d;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    line
}

/// The taut line through the pins under `cap`, kept under `top` and no steeper than `grade`
/// where it can help it.
fn fit(cap: &[f64], top: &[f64], pins: &[Option<f64>], grade: f64) -> Vec<f64> {
    let n = cap.len();
    let mut anchors: Vec<(usize, f64)> = pins
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.map(|p| (i, p)))
        .collect();
    let (Some(&(first, l0)), Some(&(last, l1))) = (anchors.first(), anchors.last()) else {
        let line = support_line(cap, grade);
        return line.iter().zip(top).map(|(y, t)| y.min(*t)).collect();
    };
    if first > 0 {
        anchors.insert(0, (0, l0.min(cap[0]).min(top[0])));
    }
    if last < n - 1 {
        anchors.push((n - 1, l1.min(cap[n - 1]).min(top[n - 1])));
    }
    let mut line = vec![0.0; n];
    for &(a, la) in &anchors {
        line[a] = la;
    }
    for w in anchors.windows(2) {
        let ((a, la), (b, lb)) = (w[0], w[1]);
        line[a] = la;
        line[b] = lb;
        if b <= a + 1 {
            continue;
        }
        hull_under(cap, a, la, b, lb, &mut line);
        // Up to the grade, giving up cover where the ground dips too sharply over the bore;
        // then back down under its top, steeper only where the pins leave it no room.
        let up = grade.max((lb - la).abs() / (b - a) as f64);
        for i in a + 1..b {
            line[i] = line[i].max(line[i - 1] - up);
        }
        for i in (a + 1..b).rev() {
            line[i] = line[i].max(line[i + 1] - up);
        }
        let over: Vec<usize> = (a + 1..b).filter(|&i| line[i] > top[i]).collect();
        for j in over {
            let room = ((la - top[j]) / (j - a) as f64).max((lb - top[j]) / (b - j) as f64);
            let s = up.max(room);
            for (i, y) in line.iter_mut().enumerate().take(b).skip(a + 1) {
                *y = y.min(top[j] + s * (i as f64 - j as f64).abs());
            }
        }
    }
    line
}

/// The taut line from `(a, la)` to `(b, lb)` under `cap`: the lower convex hull of the points.
fn hull_under(cap: &[f64], a: usize, la: f64, b: usize, lb: f64, line: &mut [f64]) {
    let cross = |o: (f64, f64), p: (f64, f64), q: (f64, f64)| {
        (p.0 - o.0) * (q.1 - o.1) - (p.1 - o.1) * (q.0 - o.0)
    };
    let points = std::iter::once((a as f64, la))
        .chain(
            (a + 1..b)
                .filter(|&i| cap[i].is_finite())
                .map(|i| (i as f64, cap[i])),
        )
        .chain(std::iter::once((b as f64, lb)));
    let mut hull: Vec<(f64, f64)> = Vec::new();
    for p in points {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    for seg in hull.windows(2) {
        let ((x0, y0), (x1, y1)) = (seg[0], seg[1]);
        for (i, y) in line
            .iter_mut()
            .enumerate()
            .take(x1 as usize + 1)
            .skip(x0 as usize)
        {
            *y = y0 + (y1 - y0) * (i as f64 - x0) / (x1 - x0);
        }
    }
}

/// With nothing to keep to, the highest straight line under `cap` through its middle, no
/// steeper than `grade`.
fn support_line(cap: &[f64], grade: f64) -> Vec<f64> {
    let n = cap.len();
    if n == 1 {
        return cap.to_vec();
    }
    let mut hull = vec![0.0; n];
    hull_under(cap, 0, cap[0], n - 1, cap[n - 1], &mut hull);
    let mid = (n - 1) / 2;
    let slope = (hull[mid + 1] - hull[mid]).clamp(-grade, grade);
    let at = |i: usize| hull[mid] + slope * (i as f64 - mid as f64);
    let over = (0..n).map(|i| at(i) - cap[i]).fold(0.0, f64::max);
    (0..n).map(|i| at(i) - over).collect()
}

/// Cells a run has to stay beside another for the two to share a bore, not just cross.
const BESIDE_CELLS: usize = 8;
/// Cells from a portal a run beside another keeps to its own line, to come round to the other's.
const BESIDE_FROM_PORTAL: usize = 16;
/// Levels apart past which two runs side by side are taken to lie one over the other.
const BESIDE_SPREAD: f64 = 6.0;

/// The levels of the runs planned so far, so the next one in the same bore keeps to them.
#[derive(Default)]
pub(crate) struct Beside {
    cells: FnvHashMap<(i32, i32), (f64, (i32, i32))>,
    /// The `BESIDE_BLOCK` squares holding any cell, to skip the search far from them.
    blocks: FnvHashSet<(i32, i32)>,
}

/// Side of the squares `Beside` looks for its cells in first.
const BESIDE_BLOCK: i32 = 16;

fn block_of(x: i32, z: i32) -> (i32, i32) {
    (x.div_euclid(BESIDE_BLOCK), z.div_euclid(BESIDE_BLOCK))
}

impl Beside {
    pub(crate) fn add(&mut self, path: &[(i32, i32)], levels: &[f64]) {
        for (i, &c) in path.iter().enumerate() {
            self.cells.entry(c).or_insert((levels[i], heading(path, i)));
            self.blocks.insert(block_of(c.0, c.1));
        }
    }

    /// Whether any cell lies within `reach` of (x, z), judged by the squares around it.
    fn near(&self, x: i32, z: i32, reach: i32) -> bool {
        let (x0, z0) = block_of(x - reach, z - reach);
        let (x1, z1) = block_of(x + reach, z + reach);
        (x0..=x1).any(|bx| (z0..=z1).any(|bz| self.blocks.contains(&(bx, bz))))
    }

    /// Pins for a run along `path` from the runs beside it within `reach`, running the same way
    /// and near its own `line`, away from its portals.
    pub(crate) fn pins(
        &self,
        path: &[(i32, i32)],
        line: &[f64],
        reach: i32,
        portal: [bool; 2],
    ) -> Vec<Option<f64>> {
        let n = path.len();
        if self.cells.is_empty() {
            return vec![None; n];
        }
        let mut out: Vec<Option<f64>> = (0..n)
            .map(|i| {
                let (x, z) = path[i];
                if !self.near(x, z, reach) {
                    return None;
                }
                let own = heading(path, i);
                let mut best: Option<(i32, f64)> = None;
                for dx in -reach..=reach {
                    for dz in -reach..=reach {
                        let Some(&(y, other)) = self.cells.get(&(x + dx, z + dz)) else {
                            continue;
                        };
                        let d = dx * dx + dz * dz;
                        if parallel(own, other) && best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, y));
                        }
                    }
                }
                best.map(|(_, y)| y)
                    .filter(|y| (y - line[i]).abs() <= BESIDE_SPREAD)
            })
            .collect();
        let mut i = 0;
        while i < n {
            let start = i;
            while i < n && out[i].is_some() {
                i += 1;
            }
            if i - start < BESIDE_CELLS {
                out[start..i].fill(None);
            }
            i += 1;
        }
        let near = BESIDE_FROM_PORTAL.min(n);
        if portal[0] {
            out[..near].fill(None);
        }
        if portal[1] {
            out[n - near..].fill(None);
        }
        out
    }
}

fn heading(path: &[(i32, i32)], i: usize) -> (i32, i32) {
    let a = path[i.saturating_sub(4)];
    let b = path[(i + 4).min(path.len() - 1)];
    (b.0 - a.0, b.1 - a.1)
}

fn parallel(a: (i32, i32), b: (i32, i32)) -> bool {
    let dot = f64::from(a.0 * b.0 + a.1 * b.1);
    let len = f64::from(a.0 * a.0 + a.1 * a.1).sqrt() * f64::from(b.0 * b.0 + b.1 * b.1).sqrt();
    len > 0.0 && dot.abs() >= 0.9 * len
}

/// Ways joined end to end into runs, each as (way, runs backwards). `partner(w, e)` is the way
/// and end that end `e` of way `w` carries straight on into, if they pick each other.
pub(crate) fn chains(
    count: usize,
    partner: impl Fn(usize, usize) -> Option<(usize, usize)>,
) -> Vec<Vec<(usize, bool)>> {
    let mut seen = vec![false; count];
    let mut out = Vec::new();
    for w in 0..count {
        if seen[w] {
            continue;
        }
        // Back to the start of the run; a ring starts next to where it was entered.
        let (mut cur, mut back) = (w, 0);
        for _ in 0..count {
            match partner(cur, back) {
                Some((o, eo)) if o != w => {
                    cur = o;
                    back = 1 - eo;
                }
                _ => break,
            }
        }
        let mut run = Vec::new();
        let (mut c, mut start) = (cur, back);
        loop {
            seen[c] = true;
            run.push((c, start == 1));
            match partner(c, 1 - start) {
                Some((o, eo)) if !seen[o] => {
                    c = o;
                    start = eo;
                }
                _ => break,
            }
        }
        out.push(run);
    }
    out
}

/// The order runs are planned in: each network of runs meeting at nodes from its longest run
/// outwards, so a run comes after one it meets and takes its level where they meet. `size`
/// is each run's cells and lowest way id, `nodes` the nodes of its ways.
pub(crate) fn planning_order(size: &[(usize, u64)], nodes: &[Vec<u64>]) -> Vec<usize> {
    let mut at: FnvHashMap<u64, Vec<usize>> = FnvHashMap::default();
    for (r, ids) in nodes.iter().enumerate() {
        for &id in ids {
            at.entry(id).or_default().push(r);
        }
    }
    let rank = |&r: &usize| (std::cmp::Reverse(size[r].0), size[r].1);
    let mut starts: Vec<usize> = (0..size.len()).collect();
    starts.sort_by_key(rank);
    let mut seen = vec![false; size.len()];
    let mut order = Vec::with_capacity(size.len());
    for start in starts {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        let mut queue = std::collections::VecDeque::from([start]);
        while let Some(r) = queue.pop_front() {
            order.push(r);
            let mut next: Vec<usize> = nodes[r]
                .iter()
                .flat_map(|id| at[id].iter().copied())
                .filter(|&o| !seen[o])
                .collect();
            next.sort_by_key(rank);
            next.dedup();
            for o in next {
                seen[o] = true;
                queue.push_back(o);
            }
        }
    }
    order
}

/// Cells in the cutting at a portal: those `bare` from each portal end in, past any hump of
/// covered cells shorter than `gap`, so a bump in the ground does not end the cutting.
pub(crate) fn portal_cuttings(bare: &[bool], portal: [bool; 2], gap: usize) -> Vec<bool> {
    let n = bare.len();
    let mut open = vec![false; n];
    for e in (0..2).filter(|&e| portal[e]) {
        let mut covered = 0;
        for d in 0..n {
            let i = if e == 0 { d } else { n - 1 - d };
            if bare[i] {
                open[i] = true;
                covered = 0;
            } else {
                covered += 1;
                if covered >= gap {
                    break;
                }
            }
        }
    }
    open
}

/// Holds both cells across One World's area edge at the level of the lower one, which the
/// area on the other side reads alike, so the two meet there.
pub(crate) fn pin_area_edges(
    pins: &mut [Option<i32>],
    cells: &[(i32, i32)],
    area: Option<&XZBBox>,
    level: impl Fn(usize) -> i32,
) {
    let Some(area) = area else {
        return;
    };
    let inside = |i: usize| area.contains(&XZPoint::new(cells[i].0, cells[i].1));
    for i in (1..cells.len()).filter(|&i| inside(i - 1) != inside(i)) {
        let y = level(if cells[i - 1] < cells[i] { i - 1 } else { i });
        pins[i - 1].get_or_insert(y);
        pins[i].get_or_insert(y);
    }
}

/// Lowers `levels` so they step at most `step` a cell, leaving the pinned cells where they
/// are and keeping the rest within reach of them.
pub(crate) fn walkable(levels: &mut [i32], pins: &[Option<i32>], step: i32) {
    let n = levels.len();
    let free = |i: usize| pins[i].is_none();
    let mut lo = vec![i32::MIN / 2; n];
    let mut hi = vec![i32::MAX / 2; n];
    let (mut l, mut h) = (i32::MIN / 2, i32::MAX / 2);
    for i in 0..n {
        (l, h) = if free(i) {
            (l - step, h + step)
        } else {
            (levels[i], levels[i])
        };
        (lo[i], hi[i]) = (l, h);
    }
    (l, h) = (i32::MIN / 2, i32::MAX / 2);
    for i in (0..n).rev() {
        (l, h) = if free(i) {
            (l - step, h + step)
        } else {
            (levels[i], levels[i])
        };
        lo[i] = lo[i].max(l);
        hi[i] = hi[i].min(h);
    }
    for i in (0..n).filter(|&i| free(i)) {
        levels[i] = levels[i].min(hi[i]).max(lo[i]);
    }
    for i in (1..n).filter(|&i| free(i)) {
        levels[i] = levels[i].min(levels[i - 1] + step);
    }
    for i in (0..n.saturating_sub(1)).rev().filter(|&i| free(i)) {
        levels[i] = levels[i].min(levels[i + 1] + step);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(line: &[f64]) -> Vec<f64> {
        line.windows(2).map(|w| w[1] - w[0]).collect()
    }

    fn rail(n: usize) -> Bore {
        Bore::new(n, 8, 0.04, 0.25, -60)
    }

    fn ends(n: usize, a: f64, b: f64) -> Vec<Option<f64>> {
        let mut pins = vec![None; n];
        pins[0] = Some(a);
        pins[n - 1] = Some(b);
        pins
    }

    /// The cover ceiling of dry ground, and the ground over it.
    fn dry(ground: &[f64]) -> (Vec<f64>, Vec<f64>) {
        (ground.iter().map(|g| g - 8.0).collect(), ground.to_vec())
    }

    #[test]
    fn under_a_mountain_the_line_runs_straight_from_portal_to_portal() {
        let n = 401;
        let ground: Vec<f64> = (0..n)
            .map(|i| 200.0 - ((i as f64 - 200.0).abs() * 0.9))
            .collect();
        let (ceiling, top) = dry(&ground);
        let line = grade_line(&ceiling, &top, &ends(n, 20.0, 20.0), [true; 2], &rail(n));
        assert!(line.iter().all(|&y| (y - 20.0).abs() < 1e-9), "{line:?}");
        let line = grade_line(&ceiling, &top, &ends(n, 14.0, 20.0), [true; 2], &rail(n));
        for (i, y) in line.iter().enumerate() {
            let straight = 14.0 + 6.0 * i as f64 / 400.0;
            assert!((y - straight).abs() < 1e-9, "cell {i}: {y} off the grade");
        }
    }

    #[test]
    fn a_gentle_slope_at_the_portal_takes_no_dip() {
        let n = 1001;
        let ground: Vec<f64> = (0..n)
            .map(|i| 20.0 + (i.min(n - 1 - i) as f64 * 0.1).min(60.0))
            .collect();
        let (ceiling, top) = dry(&ground);
        let line = grade_line(&ceiling, &top, &ends(n, 20.0, 20.0), [true; 2], &rail(n));
        assert!(line.iter().all(|&y| (y - 20.0).abs() < 1e-9));
    }

    #[test]
    fn a_valley_over_the_bore_bends_the_grade_once() {
        let n = 601;
        let ground: Vec<f64> = (0..n)
            .map(|i| if (280..320).contains(&i) { 10.0 } else { 80.0 })
            .collect();
        let (ceiling, top) = dry(&ground);
        let bore = Bore {
            grade: 0.08,
            ..rail(n)
        };
        let line = grade_line(&ceiling, &top, &ends(n, 20.0, 20.0), [true; 2], &bore);
        assert!((280..320).all(|i| line[i] <= ceiling[i] + 1e-9));
        let d = steps(&line);
        let changes = d.windows(2).filter(|w| (w[1] - w[0]).abs() > 1e-9).count();
        assert_eq!(changes, 2);
        assert!(d.iter().all(|s| s.abs() <= 0.08 + 1e-9));
    }

    #[test]
    fn a_free_end_keeps_level_with_its_portal() {
        let n = 300;
        let mut pins = vec![None; n];
        pins[0] = Some(40.0);
        let (ceiling, top) = dry(&[108.0; 300]);
        let line = grade_line(&ceiling, &top, &pins, [true, false], &rail(n));
        assert!(line.iter().all(|&y| (y - 40.0).abs() < 1e-9));
        // Ground falling away takes it down at one grade.
        let ground: Vec<f64> = (0..n).map(|i| 108.0 - i as f64 * 0.4).collect();
        let (ceiling, top) = dry(&ground);
        let line = grade_line(&ceiling, &top, &pins, [true, false], &rail(n));
        let d = steps(&line);
        assert!(d.iter().all(|s| (s - d[0]).abs() < 1e-9) && d[0] < 0.0);
    }

    #[test]
    fn a_short_tunnel_under_flat_ground_still_gets_under_it() {
        let n = 81;
        let bore = Bore::new(n, 7, 0.06, 1.0 / 3.0, -60);
        let line = grade_line(
            &[-7.0; 81],
            &[0.0; 81],
            &ends(n, 0.0, 0.0),
            [true; 2],
            &bore,
        );
        assert!((line[40] + 7.0).abs() < 1e-9);
        assert!((line[21] + 7.0).abs() < 1e-9);
        assert!(steps(&line).iter().all(|s| s.abs() <= 1.0 / 3.0 + 1e-9));
    }

    #[test]
    fn steeper_than_the_grade_the_bore_gives_up_cover_but_not_under_water() {
        let n = 1001;
        let mut ground = vec![58.0; n];
        for g in &mut ground[495..505] {
            *g = 5.0;
        }
        let (ceiling, top) = dry(&ground);
        let pins = ends(n, 20.0, 20.0);
        let line = grade_line(&ceiling, &top, &pins, [true; 2], &rail(n));
        assert!(
            line[500] > ceiling[500],
            "dips to {} for a narrow gully",
            line[500]
        );
        assert!(steps(&line).iter().all(|s| s.abs() <= 0.04 + 1e-9));
        // Under water the cover holds, at one grade either side.
        let wet: Vec<f64> = (0..n)
            .map(|i| {
                if (495..505).contains(&i) {
                    ceiling[i]
                } else {
                    top[i]
                }
            })
            .collect();
        let line = grade_line(&ceiling, &wet, &pins, [true; 2], &rail(n));
        assert!(line[495..505].iter().all(|&y| y <= -3.0 + 1e-9));
        let d = steps(&line);
        assert!(d[..495].iter().all(|s| (s - d[0]).abs() < 1e-9));
    }

    #[test]
    fn ground_under_the_floor_takes_the_line_down_at_one_grade() {
        let n = 1001;
        let mut ground = vec![58.0; n];
        for g in &mut ground[495..505] {
            *g = -22.0;
        }
        let (ceiling, top) = dry(&ground);
        let line = grade_line(&ceiling, &top, &ends(n, 20.0, 20.0), [true; 2], &rail(n));
        assert!(line.iter().zip(&top).all(|(y, t)| y <= t));
        let d = steps(&line);
        assert!(d[..495].iter().all(|s| (s - d[0]).abs() < 1e-9));
    }

    #[test]
    fn with_nothing_to_keep_to_the_line_stays_straight_under_the_ground() {
        let n = 200;
        let ground: Vec<f64> = (0..n)
            .map(|i| 38.0 + (i as f64 * 0.1).sin() * 3.0)
            .collect();
        let (ceiling, top) = dry(&ground);
        let line = grade_line(&ceiling, &top, &[None; 200], [false; 2], &rail(n));
        assert!(line.iter().zip(&ceiling).all(|(y, c)| y <= c));
        let d = steps(&line);
        assert!(d
            .iter()
            .all(|s| (s - d[0]).abs() < 1e-9 && s.abs() <= 0.04 + 1e-9));
    }

    #[test]
    fn a_pin_inside_the_run_is_kept_and_left_at_the_grade() {
        let n = 301;
        let mut pins = ends(n, 0.0, 0.0);
        pins[150] = Some(-20.0);
        let (ceiling, top) = dry(&[108.0; 301]);
        let line = grade_line(&ceiling, &top, &pins, [true; 2], &rail(n));
        assert_eq!(line[150], -20.0);
        assert!(line[..150].windows(2).all(|w| w[1] <= w[0]));
    }

    #[test]
    fn ways_joined_end_to_end_make_one_run_whichever_way_they_point() {
        // 0 -> 1 <- 2, and 3 alone.
        let links = [((0, 1), (1, 0)), ((1, 1), (2, 1))];
        let partner = |w: usize, e: usize| {
            links.iter().find_map(|&(p, q)| {
                if p == (w, e) {
                    Some(q)
                } else if q == (w, e) {
                    Some(p)
                } else {
                    None
                }
            })
        };
        let runs = chains(4, partner);
        assert_eq!(runs.len(), 2);
        let run = runs.iter().find(|r| r.len() == 3).unwrap();
        assert_eq!(run, &[(0, false), (1, false), (2, true)]);
    }

    #[test]
    fn a_network_is_planned_from_its_longest_run_outwards() {
        // 0 long, 1 a short passage from it to 2, which is longer than 1; 3 apart.
        let size = [(500, 7), (20, 3), (300, 5), (400, 1)];
        let nodes = vec![vec![1, 2, 3], vec![2, 9], vec![9, 10, 11], vec![20, 21]];
        assert_eq!(planning_order(&size, &nodes), [0, 1, 2, 3]);
    }

    #[test]
    fn a_run_beside_another_keeps_to_it_and_a_crossing_does_not() {
        let mut beside = Beside::default();
        let first: Vec<(i32, i32)> = (0..100).map(|x| (x, 50)).collect();
        beside.add(&first, &[10.0; 100]);
        let next: Vec<(i32, i32)> = (0..100).map(|x| (x, 53)).collect();
        let pins = beside.pins(&next, &[12.0; 100], 4, [true, false]);
        assert!(pins[..BESIDE_FROM_PORTAL].iter().all(Option::is_none));
        assert!(pins[BESIDE_FROM_PORTAL..].iter().all(|&p| p == Some(10.0)));
        // Far under it, or across it, it keeps its own line.
        assert!(beside
            .pins(&next, &[-20.0; 100], 4, [false; 2])
            .iter()
            .all(Option::is_none));
        let across: Vec<(i32, i32)> = (0..100).map(|z| (50, z)).collect();
        assert!(beside
            .pins(&across, &[10.0; 100], 4, [false; 2])
            .iter()
            .all(Option::is_none));
    }

    #[test]
    fn only_the_cells_open_from_a_portal_are_its_cutting() {
        let bare = [true, false, true, false, false, true, true, false];
        assert_eq!(
            portal_cuttings(&bare, [true, false], 2),
            [true, false, true, false, false, false, false, false]
        );
        assert_eq!(
            portal_cuttings(&bare, [false, true], 2),
            [false, false, false, false, false, true, true, false]
        );
        assert_eq!(
            portal_cuttings(&bare, [true, false], 3),
            [true, false, true, false, false, true, true, false]
        );
        assert_eq!(portal_cuttings(&bare, [false, false], 3), [false; 8]);
    }

    #[test]
    fn a_short_tunnel_under_flat_ground_ramps_down_from_both_portals() {
        // Rail holds two cells at each portal, a road one.
        for (depth, most, held) in [(8, 0.25, 2), (7, 1.0 / 3.0, 1)] {
            for n in (40..=80).step_by(4).chain([63, 64, 65]) {
                let mut pins = vec![None; n];
                for i in (0..held).chain(n - held..n) {
                    pins[i] = Some(0.0);
                }
                let bore = Bore::new(n, depth, 0.04, most, -60);
                let ceiling = vec![-f64::from(depth); n];
                let line = grade_line(&ceiling, &vec![0.0; n], &pins, [true; 2], &bore);
                let deepest = line.iter().copied().fold(f64::INFINITY, f64::min);
                let room = ((n - 1) / 2 + 1 - held) as f64;
                let want = (most * room).min(f64::from(depth));
                assert!(deepest <= -want + 1e-9, "n={n}: {deepest} not under {want}");
                assert!(steps(&line).iter().all(|s| s.abs() <= most + 1e-9), "n={n}");
            }
        }
    }

    #[test]
    fn a_covered_stretch_ends_a_portals_cutting() {
        let mut bare = vec![false; 30];
        for i in (0..3).chain(14..20) {
            bare[i] = true;
        }
        let open = portal_cuttings(&bare, [true, false], CUTTING_GAP);
        assert!(open[..3].iter().all(|&o| o));
        assert!(!open[3..].iter().any(|&o| o), "{open:?}");
    }

    #[test]
    fn a_single_pinned_cell_keeps_its_pin() {
        let line = grade_line(&[-8.0], &[0.0], &[Some(-3.0)], [false; 2], &rail(1));
        assert_eq!(line, [-3.0]);
    }

    #[test]
    fn a_walkable_line_keeps_its_pins_and_steps_a_block() {
        let pins = [Some(3), None, None, Some(1), None, Some(3)];
        let mut levels = [3, 9, -4, 1, 7, 3];
        walkable(&mut levels, &pins, 1);
        assert_eq!(levels, [3, 2, 1, 1, 2, 3]);
        // A junction and a seam next to each other keep their exact levels.
        let pins = [None, Some(-4), Some(-4), None, None, Some(-6), None];
        let mut levels = [0, -4, -4, -9, -9, -6, 2];
        walkable(&mut levels, &pins, 1);
        assert_eq!((levels[1], levels[2], levels[5]), (-4, -4, -6));
        assert!(
            levels.windows(2).all(|w| (w[1] - w[0]).abs() <= 1),
            "{levels:?}"
        );
    }

    #[test]
    fn both_sides_of_an_area_edge_take_the_lower_cells_level() {
        let area = XZBBox::rect_from_xz_lengths(10.0, 10.0).unwrap();
        let cells: Vec<(i32, i32)> = (8..13).map(|x| (x, 5)).collect();
        let mut pins = vec![None; cells.len()];
        pin_area_edges(&mut pins, &cells, Some(&area), |i| i as i32 * 10);
        assert_eq!(pins, [None, None, Some(20), Some(20), None]);
        let mut none = vec![None; cells.len()];
        pin_area_edges(&mut none, &cells, None, |_| 0);
        assert!(none.iter().all(Option::is_none));
    }
}
