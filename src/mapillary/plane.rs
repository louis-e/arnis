//! The real wall plane from the point cloud. Port of `tools/facade_lab/plane.py`.
//!
//! An OSM outline is off by 0.5 to 2 m, and at 8 px per metre that is a smeared
//! texture, so the plane the texture is rectified onto comes from the cloud
//! wherever the cloud has anything to say. The plane is the 0.25 m band holding
//! the most of the points within 3 m of the OSM line, followed by two passes of
//! total-least-squares refinement on the inliers. The fit is refused when it
//! carries fewer than `max(30, 4 L)` inliers or when the refinement pulls it more
//! than 10 degrees off the OSM direction, because by then it has locked onto
//! something that is not this wall: a kerb, a parked van, the building behind.
//! Fits come out at about 0.15 m RMS on the Munich box.
//!
//! A fit also carries the 98th percentile of its inliers' z, which is the eave
//! height and usually beats the `building:levels` tag, and a continuity flag
//! saying whether the wall's points reach that height without a four metre hole.
//!
//! Frame conventions:
//!
//! * a plane is the plan-view line `n . xy = d` with `n` pointing the way the
//!   wall's outward normal points, so `offset_vs_osm_m` is positive in front of
//!   the OSM footprint and negative behind it;
//! * every z in here is in the cluster datum of the view whose registration
//!   produced the points, so it is comparable only within that view. Heights
//!   leave through [`cloud_height`], which measures from that view's own
//!   `z_base`.
//! * CRITIQUE A5: every candidate view gets its own fit. The wall frame is the
//!   fit of the best view, [`per_view_offset`] says how another view's fit sits
//!   in that frame, and [`refine_corners`] only intersects fits that came from
//!   the same registration, so a corner never mixes two of them.
//!
//! **[`exact_line`] is what makes this port possible at all.** The band search
//! used to be 300 two-point RANSAC hypotheses drawn by `numpy`'s generator into
//! an array whose order came out of `scipy.spatial.cKDTree`. Neither the draw nor
//! the traversal is portable, and permuting the input moved the fitted normal by
//! more than half a degree on 7 of 28 cloud fits and by 9.7 degrees on the two
//! with the thinnest support, so the Python's own answer on those walls was as
//! arbitrary as any port's would have been.
//!
//! The objective those hypotheses were approximating has a closed form. At a
//! fixed angle the best band is a sliding window over the sorted projections, so
//! the best band overall is the best of those over a grid of angles, and the grid
//! hangs off the wall's OSM direction rather than off the points. The answer is
//! then a property of the point *set*: permuting the input leaves all 1344 fits
//! of the Munich box bit identical, where the sampled solver moved 64 to 72 of
//! them by more than half a degree and up to 4 to 9 of them out of their source.
//! MEASURED.md, "Fixed: the plane fit decided by a random sample", has the rest.
//!
//! One thing plane.py has that is not here: `PlaneFit` in `types.rs` has no
//! `src_a` / `src_b`, so where a wall's two ends came from leaves
//! [`refine_corners`] as its return value instead of being written onto the fit.

#![allow(dead_code)]

use super::types::{Params, PlaneFit, PlaneSource, Wall};

/// Adjacent walls must turn by more than this before their planes are
/// intersected; below it the intersection slides along the wall.
pub const MIN_CORNER_DEG: f64 = 30.0;
/// An intersection farther than this from the OSM node is not trusted.
pub const MAX_CORNER_JUMP_M: f64 = 3.0;
/// A candidate band closer than this to the winner anywhere along the wall is
/// the same plane, not a rival.
pub const DISTINCT_PLANE_M: f64 = 0.5;
/// A run of empty one metre bins this long breaks the height continuity.
pub const GAP_M: f64 = 4.0;

/// Where one end of a wall's plane came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndSource {
    /// The OSM node, projected onto the plane.
    Osm,
    /// The intersection with the adjacent wall's own cloud plane.
    CloudCorner,
}

impl EndSource {
    pub fn as_str(self) -> &'static str {
        match self {
            EndSource::Osm => "osm",
            EndSource::CloudCorner => "cloud-corner",
        }
    }
}

/// The two ends of a wall's plane after the corner pass.
#[derive(Clone, Copy, Debug)]
pub struct Corners {
    pub a_ref: [f64; 2],
    pub b_ref: [f64; 2],
    pub src_a: EndSource,
    pub src_b: EndSource,
}

/// The deterministic per wall seed of DESIGN.md 9: the crc32 of the wall key.
///
/// The plane fit has no use for it any more, but the block palette's k-medoids
/// still does, and both sides have to agree on it.
pub fn wall_seed(wall_key: &str) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in wall_key.as_bytes() {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

// --------------------------------------------------------------------------- helpers

#[inline]
fn dot2(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// The foot of the perpendicular from `xy` onto the plane line.
pub fn project_onto(fit: &PlaneFit, xy: [f64; 2]) -> [f64; 2] {
    let off = fit.d - dot2(fit.n, xy);
    [xy[0] + off * fit.n[0], xy[1] + off * fit.n[1]]
}

/// Signed distance of the fitted line from the OSM line at the wall midpoint,
/// positive outward.
pub fn offset_of(fit: &PlaneFit, wall: &Wall) -> f64 {
    fit.d - dot2(fit.n, wall.midpoint())
}

/// Unsigned angle in degrees between a plane normal and the OSM outward normal.
pub fn angle_of(n: [f64; 2], wall: &Wall) -> f64 {
    dot2(n, wall.n).clamp(-1.0, 1.0).abs().acos().to_degrees()
}

/// A fit with everything but the line itself at its zero value.
fn blank_fit(wall_key: &str, n: [f64; 2], d: f64, source: PlaneSource) -> PlaneFit {
    PlaneFit {
        wall_key: wall_key.to_string(),
        n,
        d,
        source,
        n_inliers: 0,
        pts_per_m: 0.0,
        rms_m: 0.0,
        angle_vs_osm_deg: 0.0,
        offset_vs_osm_m: 0.0,
        z_top98: None,
        z_continuous: false,
        ambiguity: 0.0,
        a_ref: None,
        b_ref: None,
        pano_id: None,
        cluster_id: None,
    }
}

/// The plane on the OSM line itself, for a wall the cloud could not fit.
///
/// The source records whether that line is already in registered coordinates,
/// which is what tells a later stage how much to trust it.
pub fn fallback_plane(
    wall: &Wall,
    reg_accepted: bool,
    pano_id: Option<&str>,
    cluster_id: Option<&str>,
) -> PlaneFit {
    let source = if reg_accepted {
        PlaneSource::OsmRegistered
    } else {
        PlaneSource::OsmRaw
    };
    let mut fit = blank_fit(&wall.key, wall.n, dot2(wall.n, wall.a), source);
    fit.a_ref = Some(wall.a);
    fit.b_ref = Some(wall.b);
    fit.pano_id = pano_id.map(str::to_string);
    fit.cluster_id = cluster_id.map(str::to_string);
    fit
}

/// The wall's geometry moved onto the fitted plane.
///
/// `a` and `b` become the fit's own ends projected onto the plane and the normal
/// becomes the plane normal, so `Wall::point` and `Wall::sh_of` on the result
/// give positions on the plane, which is what the rectifier needs.
pub fn fitted_wall(wall: &Wall, fit: &PlaneFit) -> Wall {
    let mut a = project_onto(fit, fit.a_ref.unwrap_or(wall.a));
    let mut b = project_onto(fit, fit.b_ref.unwrap_or(wall.b));
    let mut length = (b[0] - a[0]).hypot(b[1] - a[1]);
    if length < 1e-6 {
        // Degenerate ends: keep the OSM length along the plane direction.
        let t = [-fit.n[1], fit.n[0]];
        a = project_onto(fit, wall.a);
        b = [a[0] + wall.length * t[0], a[1] + wall.length * t[1]];
        length = wall.length;
    }
    Wall {
        key: wall.key.clone(),
        building_key: wall.building_key.clone(),
        idx: wall.idx,
        node_a: wall.node_a,
        node_b: wall.node_b,
        a,
        b,
        n: fit.n,
        length,
        merged_idx: wall.merged_idx.clone(),
        piece: wall.piece,
        n_pieces: wall.n_pieces,
        height_osm: wall.height_osm,
        height_source: wall.height_source,
        reachable: wall.reachable,
        unreachable_reason: wall.unreachable_reason.clone(),
        edges: wall.edges.clone(),
        s_offset: wall.s_offset,
    }
}

// --------------------------------------------------------------------------- the band search

/// The unit normals of the angular grid: the OSM normal turned by every multiple
/// of `step_deg` up to `+-cone_deg`, paired with the multiple itself.
///
/// The grid hangs off the wall's own direction and not off the points, so it is
/// a property of the wall and the same whatever order the cloud arrives in.
fn grid_normals(wall_n: [f64; 2], cone_deg: f64, step_deg: f64) -> Vec<(i64, [f64; 2])> {
    let k = (cone_deg / step_deg).round() as i64;
    (-k..=k)
        .map(|i| {
            // Degrees to radians by one multiplication, the way the lab writes it,
            // so both sides turn the grid by the same double.
            let th = ((i as f64) * step_deg) * (std::f64::consts::PI / 180.0);
            let (s, c) = (th.sin(), th.cos());
            (
                i,
                [c * wall_n[0] - s * wall_n[1], s * wall_n[0] + c * wall_n[1]],
            )
        })
        .collect()
}

/// The projections of `xy` onto `n`, sorted, into the caller's scratch buffer.
fn project_sorted(xy: &[[f64; 2]], n: [f64; 2], out: &mut Vec<f64>) {
    out.clear();
    out.extend(xy.iter().map(|p| dot2(*p, n)));
    out.sort_unstable_by(f64::total_cmp);
}

/// Walks every maximal band of half width `band` over the sorted projections `r`,
/// handing the callback the band's point count and the offset that centres it on
/// its own two extremes.
///
/// The points a band this wide can hold are bounded by the half-open window
/// `[r, r + 2 band)` that starts at the set's own smallest member, and that window
/// is in turn held by the band centred on its extremes, so the widest window is the
/// exact maximum of the objective and not an approximation of it. The right edge
/// only moves forwards, so the whole walk is linear.
fn band_windows(r: &[f64], band: f64, mut f: impl FnMut(usize, f64)) {
    let mut end = 0usize;
    for (i, &lo) in r.iter().enumerate() {
        // A band always holds the projection it starts on, whatever the width.
        end = end.max(i + 1);
        while end < r.len() && r[end] < lo + 2.0 * band {
            end += 1;
        }
        f(end - i, 0.5 * (lo + r[end - 1]));
    }
}

/// The vertical plane holding the most points in a `+-band` band, searched exactly
/// over the angular grid: `(normal, offset, its count, the best rival's count)`.
///
/// Ties on the count go to the normal nearest the OSM direction, because with
/// nothing in the cloud to separate two angles the tag is the only evidence left.
/// A rival is a band that stays clear of the winner by more than
/// [`DISTINCT_PLANE_M`] at both ends of the wall and on the same side at both: a
/// tilted line that crosses the winner inside the wall is the winner seen slightly
/// differently, not competition. A band's distance from the winner at an end is its
/// own offset minus a constant per angle, so that test is two half lines in the
/// offset, which is what the second walk below compares against.
fn exact_line(
    xy: &[[f64; 2]],
    wall_n: [f64; 2],
    wall_a: [f64; 2],
    wall_b: [f64; 2],
    band: f64,
    cone_deg: f64,
    step_deg: f64,
) -> ([f64; 2], f64, usize, usize) {
    let grid = grid_normals(wall_n, cone_deg, step_deg);
    let n_pts = xy.len();
    // Every candidate band of every angle, kept because the rival scan below is
    // against the winner and the winner is only known once the walk is over. The
    // table is what the lab keeps too, and it halves the sorting.
    let mut counts: Vec<u32> = Vec::with_capacity(grid.len() * n_pts);
    let mut offs: Vec<f64> = Vec::with_capacity(grid.len() * n_pts);
    let mut r: Vec<f64> = Vec::with_capacity(n_pts);
    let (mut best, mut n_best, mut d_best, mut k_best) = (0u32, wall_n, 0.0, i64::MAX);
    for &(k, n) in &grid {
        project_sorted(xy, n, &mut r);
        let (mut top, mut top_d) = (0u32, 0.0);
        band_windows(&r, band, |count, d| {
            counts.push(count as u32);
            offs.push(d);
            if count as u32 > top {
                // The first window of a run is the lowest band at this angle.
                top = count as u32;
                top_d = d;
            }
        });
        if top > best || (top == best && k.abs() < k_best.abs()) {
            best = top;
            n_best = n;
            d_best = top_d;
            k_best = k;
        }
    }

    let (base_a, base_b) = (d_best - dot2(n_best, wall_a), d_best - dot2(n_best, wall_b));
    let mut second = 0u32;
    for (row, &(_, n)) in grid.iter().enumerate() {
        let (ca, cb) = (dot2(n, wall_a) + base_a, dot2(n, wall_b) + base_b);
        let (lo, hi) = (ca.min(cb) - DISTINCT_PLANE_M, ca.max(cb) + DISTINCT_PLANE_M);
        let span = row * n_pts..(row + 1) * n_pts;
        for (&count, &d) in counts[span.clone()].iter().zip(&offs[span]) {
            if count > second && (d < lo || d > hi) {
                second = count;
            }
        }
    }
    (n_best, d_best, best as usize, second as usize)
}

/// Total-least-squares line through `xy`: the unit normal oriented like `wall_n`
/// and its offset.
fn pca_line(xy: &[[f64; 2]], wall_n: [f64; 2]) -> ([f64; 2], f64) {
    let count = xy.len();
    let inv = 1.0 / count.max(1) as f64;
    let (mut cx, mut cy) = (0.0, 0.0);
    for p in xy {
        cx += p[0];
        cy += p[1];
    }
    cx /= count as f64;
    cy /= count as f64;
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for p in xy {
        let (dx, dy) = (p[0] - cx, p[1] - cy);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    sxx *= inv;
    sxy *= inv;
    syy *= inv;
    // Eigenvector of the smaller eigenvalue of [[sxx, sxy], [sxy, syy]]. Of the
    // two ways to write it, only one avoids cancelling when the cloud is nearly
    // axis aligned, and which one depends on the sign of the difference.
    let half = 0.5 * (sxx - syy);
    let spread = (half * half + sxy * sxy).sqrt();
    let v = if half >= 0.0 {
        [sxy, -(half + spread)]
    } else {
        [half - spread, sxy]
    };
    let len = v[0].hypot(v[1]);
    let mut n = if len < 1e-300 {
        [1.0, 0.0]
    } else {
        [v[0] / len, v[1] / len]
    };
    if dot2(n, wall_n) < 0.0 {
        n = [-n[0], -n[1]];
    }
    (n, n[0] * cx + n[1] * cy)
}

/// No run of `GAP_M` or more empty one metre bins between the lowest inlier and
/// `z_top`.
fn continuity(z: &[f64], z_top: f64) -> bool {
    if z.is_empty() {
        return false;
    }
    let lo = z.iter().copied().fold(f64::INFINITY, f64::min).floor();
    let hi = z_top.ceil();
    if hi - lo < 1.0 {
        return true;
    }
    let bins = (hi - lo) as usize;
    let mut counts = vec![0usize; bins];
    for &v in z {
        if v > z_top {
            continue;
        }
        // numpy closes the last bin on the right and leaves the rest half open.
        let mut bin = (v - lo).floor() as isize;
        if v == hi {
            bin = bins as isize - 1;
        }
        if bin >= 0 && (bin as usize) < bins {
            counts[bin as usize] += 1;
        }
    }
    let mut run = 0usize;
    let mut worst = 0usize;
    for c in counts {
        run = if c == 0 { run + 1 } else { 0 };
        worst = worst.max(run);
    }
    (worst as f64) < GAP_M
}

/// The wall plane from the registered cloud points near it.
///
/// `pts` is what [`super::sfm::wall_points`] returned for this view. A rejected
/// fit comes back as [`fallback_plane`] carrying the statistics that rejected it,
/// because those are what a review sheet needs to say why a wall has no cloud
/// plane.
pub fn fit_wall_plane(
    wall: &Wall,
    pts: &[[f64; 3]],
    params: &Params,
    pano_id: Option<&str>,
    cluster_id: Option<&str>,
    reg_accepted: bool,
) -> PlaneFit {
    let need = 30.max((4.0 * wall.length).ceil() as usize);
    let fallback = fallback_plane(wall, reg_accepted, pano_id, cluster_id);
    if pts.len() < 2 {
        return fallback;
    }
    let xy: Vec<[f64; 2]> = pts.iter().map(|p| [p[0], p[1]]).collect();

    let band = params.plane_band_m;
    let (mut n_best, mut d_best, support, second) = exact_line(
        &xy,
        wall.n,
        wall.a,
        wall.b,
        band,
        params.plane_max_angle_deg,
        params.plane_angle_step_deg,
    );
    let mut inl: Vec<bool> = xy
        .iter()
        .map(|p| (dot2(*p, n_best) - d_best).abs() < band)
        .collect();
    for _ in 0..2 {
        if inl.iter().filter(|k| **k).count() < 2 {
            break;
        }
        let kept: Vec<[f64; 2]> = xy
            .iter()
            .zip(&inl)
            .filter(|(_, k)| **k)
            .map(|(p, _)| *p)
            .collect();
        let (n_ref, d_ref) = pca_line(&kept, wall.n);
        inl = xy
            .iter()
            .map(|p| (dot2(*p, n_ref) - d_ref).abs() < band)
            .collect();
        n_best = n_ref;
        d_best = d_ref;
    }
    let n_inl = inl.iter().filter(|k| **k).count();
    let off_best = d_best - dot2(n_best, wall.midpoint());
    let ambiguity = second as f64 / support.max(1) as f64;

    let mut sq = 0.0;
    let mut z_in: Vec<f64> = Vec::with_capacity(n_inl);
    for (p, keep) in pts.iter().zip(&inl) {
        if *keep {
            let r = dot2([p[0], p[1]], n_best) - d_best;
            sq += r * r;
            z_in.push(p[2]);
        }
    }
    let rms = if n_inl > 0 {
        (sq / n_inl as f64).sqrt()
    } else {
        0.0
    };
    let angle = angle_of(n_best, wall);
    let (z_top, cont) = if n_inl > 0 {
        let mut sorted = z_in.clone();
        let top = super::imgops::percentile_in_place(&mut sorted, 98.0);
        (Some(top), continuity(&z_in, top))
    } else {
        (None, false)
    };

    let accepted = n_inl >= need && angle < params.plane_max_angle_deg;
    let mut fit = if accepted {
        let mut f = blank_fit(&wall.key, n_best, d_best, PlaneSource::Cloud);
        f.pano_id = pano_id.map(str::to_string);
        f.cluster_id = cluster_id.map(str::to_string);
        f
    } else {
        fallback
    };
    fit.n_inliers = n_inl;
    fit.pts_per_m = n_inl as f64 / wall.length.max(1e-6);
    fit.rms_m = rms;
    fit.angle_vs_osm_deg = angle;
    fit.offset_vs_osm_m = off_best;
    fit.z_top98 = z_top;
    fit.z_continuous = cont;
    fit.ambiguity = ambiguity;
    if accepted {
        fit.a_ref = Some(project_onto(&fit, wall.a));
        fit.b_ref = Some(project_onto(&fit, wall.b));
    }
    fit
}

// --------------------------------------------------------------------------- corners

/// Plan-view intersection of two wall lines, or `None` when they meet at less
/// than [`MIN_CORNER_DEG`].
pub fn intersect_planes(p: &PlaneFit, q: &PlaneFit) -> Option<[f64; 2]> {
    let det = p.n[0] * q.n[1] - p.n[1] * q.n[0];
    if det.abs() < MIN_CORNER_DEG.to_radians().sin() {
        return None;
    }
    Some([
        (p.d * q.n[1] - q.d * p.n[1]) / det,
        (p.n[0] * q.d - q.n[0] * p.d) / det,
    ])
}

/// Turn angle in degrees between two walls' tangents, zero when collinear.
pub fn corner_angle_deg(w: &Wall, v: &Wall) -> f64 {
    dot2(w.tangent(), v.tangent())
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

/// The neighbours at end `a` and end `b` among the same building's walls, by
/// shared node id.
///
/// A split piece only has a corner at the ends of the unsplit merged wall: the
/// joints inside a long wall are straight.
pub fn adjacent_walls<'a>(
    wall: &Wall,
    siblings: &'a [Wall],
) -> (Option<&'a Wall>, Option<&'a Wall>) {
    let mut prev = None;
    let mut next = None;
    if wall.piece == 0 {
        prev = siblings.iter().find(|v| {
            v.key != wall.key
                && v.idx != wall.idx
                && v.node_b == wall.node_a
                && v.piece + 1 == v.n_pieces
        });
    }
    if wall.piece + 1 == wall.n_pieces {
        next = siblings.iter().find(|v| {
            v.key != wall.key && v.idx != wall.idx && v.node_a == wall.node_b && v.piece == 0
        });
    }
    (prev, next)
}

/// Where this wall's plane should end.
///
/// An end is the OSM node projected onto the plane, unless the adjacent wall
/// turns by more than [`MIN_CORNER_DEG`] and its plane is a cloud fit from the
/// same registration, in which case it is the two planes' intersection. An
/// intersection more than [`MAX_CORNER_JUMP_M`] from the OSM node is unstable
/// and is not taken.
///
/// `adjacent_fit` answers with the plane of a neighbouring wall; the caller owns
/// the per-view table CRITIQUE A5 wants consulted first, because it is the only
/// one that knows which view this frame belongs to.
pub fn refine_corners(
    wall: &Wall,
    fit: &PlaneFit,
    siblings: &[Wall],
    adjacent_fit: impl Fn(&Wall) -> Option<PlaneFit>,
) -> Corners {
    let mut corners = Corners {
        a_ref: project_onto(fit, wall.a),
        b_ref: project_onto(fit, wall.b),
        src_a: EndSource::Osm,
        src_b: EndSource::Osm,
    };
    if fit.source != PlaneSource::Cloud {
        return corners;
    }
    let (prev, next) = adjacent_walls(wall, siblings);
    for (is_a, adj, node) in [(true, prev, wall.a), (false, next, wall.b)] {
        let Some(adj) = adj else { continue };
        if corner_angle_deg(wall, adj) <= MIN_CORNER_DEG {
            continue;
        }
        let Some(other) = adjacent_fit(adj) else {
            continue;
        };
        if other.source != PlaneSource::Cloud || other.pano_id != fit.pano_id {
            continue;
        }
        let Some(x) = intersect_planes(fit, &other) else {
            continue;
        };
        if (x[0] - node[0]).hypot(x[1] - node[1]) > MAX_CORNER_JUMP_M {
            continue;
        }
        if is_a {
            corners.a_ref = x;
            corners.src_a = EndSource::CloudCorner;
        } else {
            corners.b_ref = x;
            corners.src_b = EndSource::CloudCorner;
        }
    }
    corners
}

// --------------------------------------------------------------------------- cues

/// The cloud's opinion of the wall height above this view's foot, or `None`
/// when the fit is not solid enough to have one.
pub fn cloud_height(fit: &PlaneFit, z_base: f64) -> Option<f64> {
    if fit.source != PlaneSource::Cloud || fit.n_inliers < 30 || !fit.z_continuous {
        return None;
    }
    fit.z_top98.map(|z| z - z_base)
}

/// How another view's fit sits in the wall frame of `frame_fit` (CRITIQUE A5).
///
/// `dn_m` is how far in front of the frame plane the other plane is at the wall
/// midpoint, `ds_m` how far along the wall the other view's start has moved, and
/// `dtheta_deg` the turn between the two normals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewOffset {
    pub dn_m: f64,
    pub ds_m: f64,
    pub dtheta_deg: f64,
}

pub fn per_view_offset(frame_fit: &PlaneFit, other: &PlaneFit, wall: &Wall) -> ViewOffset {
    let mid = wall.midpoint();
    let dn = (other.d - dot2(other.n, mid)) - (frame_fit.d - dot2(frame_fit.n, mid));
    let framed = fitted_wall(wall, frame_fit);
    let a_other = other.a_ref.unwrap_or(wall.a);
    let t = framed.tangent();
    let ds = (a_other[0] - framed.a[0]) * t[0] + (a_other[1] - framed.a[1]) * t[1];
    let ang_f = frame_fit.n[1].atan2(frame_fit.n[0]);
    let ang_o = other.n[1].atan2(other.n[0]);
    let dtheta = (ang_o - ang_f + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
        - std::f64::consts::PI;
    ViewOffset {
        dn_m: dn,
        ds_m: ds,
        dtheta_deg: dtheta.to_degrees(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapillary::types::HeightSource;

    fn wall(a: [f64; 2], b: [f64; 2], n: [f64; 2]) -> Wall {
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        Wall {
            key: "w1_0".into(),
            building_key: "w1".into(),
            idx: 0,
            node_a: 1,
            node_b: 2,
            a,
            b,
            n,
            length,
            merged_idx: vec![0],
            piece: 0,
            n_pieces: 1,
            height_osm: Some(12.0),
            height_source: HeightSource::Tag,
            reachable: true,
            unreachable_reason: String::new(),
            edges: vec![],
            s_offset: 0.0,
        }
    }

    #[test]
    fn the_seed_is_the_crc32_of_the_key() {
        assert_eq!(wall_seed("w81190157_2"), 24_260_261);
        assert_eq!(wall_seed("w79401257_3"), 1_830_946_148);
        assert_eq!(wall_seed("r147094_8p0"), 3_647_633_301);
        assert_eq!(wall_seed(""), 0);
    }

    #[test]
    fn a_clean_wall_of_points_is_fitted_and_accepted() {
        // A 10 m wall along x with its outward normal pointing south, and a
        // cloud sitting 0.4 m in front of it with 5 cm of noise.
        let w = wall([0.0, 0.0], [10.0, 0.0], [0.0, -1.0]);
        let mut pts = Vec::new();
        for i in 0..400 {
            let s = f64::from(i) * 0.025;
            let jitter = ((i * 37 % 11) as f64 - 5.0) * 0.01;
            pts.push([s, -0.4 + jitter, 1.0 + (i % 40) as f64 * 0.4]);
        }
        let fit = fit_wall_plane(&w, &pts, &Params::default(), None, None, true);
        assert_eq!(fit.source, PlaneSource::Cloud);
        assert!(fit.angle_vs_osm_deg < 0.5, "angle {}", fit.angle_vs_osm_deg);
        // The cloud sits on the outward side, so the offset is positive.
        assert!(
            (fit.offset_vs_osm_m - 0.4).abs() < 0.02,
            "offset {}",
            fit.offset_vs_osm_m
        );
        assert!(fit.rms_m < 0.05, "rms {}", fit.rms_m);
        assert!(fit.n_inliers > 350);
        assert!((fit.pts_per_m - fit.n_inliers as f64 / 10.0).abs() < 1e-9);
        assert!(fit.z_continuous, "z runs from 1 to 16.6 m without a hole");
        // The points reach 16.6 m, and the 98th percentile of them is just under.
        assert!(
            (16.0..=16.6).contains(&fit.z_top98.unwrap()),
            "z_top98 {:?}",
            fit.z_top98
        );
        // The ends are the OSM nodes dropped onto the plane.
        assert!((fit.a_ref.unwrap()[1] + 0.4).abs() < 0.02);
    }

    #[test]
    fn a_cloud_across_the_wall_is_refused() {
        // Points on a line at 45 degrees to the wall: over the angle limit, so
        // the fit falls back to the OSM line but keeps the measurement.
        let w = wall([0.0, 0.0], [10.0, 0.0], [0.0, -1.0]);
        let pts: Vec<[f64; 3]> = (0..300)
            .map(|i| {
                let t = f64::from(i) * 0.02;
                [t, -t, 2.0 + (i % 30) as f64 * 0.3]
            })
            .collect();
        let fit = fit_wall_plane(&w, &pts, &Params::default(), None, None, false);
        assert_eq!(fit.source, PlaneSource::OsmRaw);
        assert!(fit.angle_vs_osm_deg > 10.0, "{}", fit.angle_vs_osm_deg);
        assert!(fit.n_inliers > 0, "the statistics survive the rejection");
        assert_eq!(fit.n, w.n);
        // A registered view says so in the source.
        let registered = fit_wall_plane(&w, &pts, &Params::default(), None, None, true);
        assert_eq!(registered.source, PlaneSource::OsmRegistered);
    }

    #[test]
    fn too_few_points_never_reach_the_search() {
        let w = wall([0.0, 0.0], [10.0, 0.0], [0.0, -1.0]);
        let fit = fit_wall_plane(&w, &[[0.0, 0.0, 0.0]], &Params::default(), None, None, true);
        assert_eq!(fit.source, PlaneSource::OsmRegistered);
        assert_eq!(fit.n_inliers, 0);
        assert_eq!(fit.a_ref, Some(w.a));
    }

    #[test]
    fn the_continuity_gate_finds_a_four_metre_hole() {
        let solid: Vec<f64> = (0..100).map(|i| f64::from(i) * 0.1).collect();
        assert!(continuity(&solid, 9.9));
        let mut holed: Vec<f64> = (0..20).map(|i| f64::from(i) * 0.1).collect();
        holed.extend((0..20).map(|i| 6.0 + f64::from(i) * 0.1));
        assert!(!continuity(&holed, 7.9), "2 to 6 m is empty");
        // A three metre hole is still continuous: the gate is four.
        let mut small: Vec<f64> = (0..20).map(|i| f64::from(i) * 0.1).collect();
        small.extend((0..20).map(|i| 5.0 + f64::from(i) * 0.1));
        assert!(continuity(&small, 6.9));
        assert!(!continuity(&[], 5.0));
    }

    #[test]
    fn a_fitted_wall_sits_on_the_plane() {
        let w = wall([0.0, 0.0], [10.0, 0.0], [0.0, -1.0]);
        let mut fit = blank_fit("w1_0", [0.0, -1.0], 0.4, PlaneSource::Cloud);
        fit.a_ref = Some([0.0, 0.0]);
        fit.b_ref = Some([10.0, 0.0]);
        let moved = fitted_wall(&w, &fit);
        // n . xy = d, and n is (0, -1), so both ends sit at y = -0.4.
        assert!((moved.a[1] + 0.4).abs() < 1e-12);
        assert!((moved.b[1] + 0.4).abs() < 1e-12);
        assert!((moved.length - 10.0).abs() < 1e-12);
        assert_eq!(moved.n, fit.n);
        assert_eq!(moved.key, w.key);
        // Degenerate ends fall back to the OSM length along the plane.
        fit.b_ref = Some([0.0, 0.0]);
        let degenerate = fitted_wall(&w, &fit);
        assert!((degenerate.length - 10.0).abs() < 1e-12);
    }

    #[test]
    fn a_corner_is_taken_only_from_the_same_view_and_a_real_turn() {
        let south = wall([0.0, 0.0], [10.0, 0.0], [0.0, -1.0]);
        let mut east = wall([10.0, 0.0], [10.0, 8.0], [1.0, 0.0]);
        east.key = "w1_1".into();
        east.idx = 1;
        east.node_a = 2;
        east.node_b = 3;
        let siblings = vec![south.clone(), east.clone()];

        let mut fit = blank_fit("w1_0", [0.0, -1.0], 0.4, PlaneSource::Cloud);
        fit.pano_id = Some("p".into());
        let mut other = blank_fit("w1_1", [1.0, 0.0], 10.5, PlaneSource::Cloud);
        other.pano_id = Some("p".into());

        let corners = refine_corners(&south, &fit, &siblings, |w| {
            if w.key == "w1_1" {
                Some(other.clone())
            } else {
                None
            }
        });
        assert_eq!(corners.src_b, EndSource::CloudCorner);
        assert!((corners.b_ref[0] - 10.5).abs() < 1e-12);
        assert!((corners.b_ref[1] + 0.4).abs() < 1e-12);
        // End a has no neighbour, so it stays the projected OSM node.
        assert_eq!(corners.src_a, EndSource::Osm);
        assert!((corners.a_ref[1] + 0.4).abs() < 1e-12);

        // A neighbour fitted from a different view is not allowed to move it.
        let mut foreign = other.clone();
        foreign.pano_id = Some("q".into());
        let split = refine_corners(&south, &fit, &siblings, |_| Some(foreign.clone()));
        assert_eq!(split.src_b, EndSource::Osm);

        // Nor is an intersection that runs away from the node.
        let mut far = other.clone();
        far.d = 20.0;
        let runaway = refine_corners(&south, &fit, &siblings, |_| Some(far.clone()));
        assert_eq!(runaway.src_b, EndSource::Osm);

        // An OSM fallback never gets a corner at all.
        let plain = fallback_plane(&south, true, Some("p"), None);
        let none = refine_corners(&south, &plain, &siblings, |_| Some(other.clone()));
        assert_eq!(none.src_b, EndSource::Osm);
    }

    #[test]
    fn the_height_cue_needs_a_solid_fit() {
        let mut fit = blank_fit("w1_0", [0.0, -1.0], 0.4, PlaneSource::Cloud);
        fit.n_inliers = 120;
        fit.z_continuous = true;
        fit.z_top98 = Some(18.5);
        assert_eq!(cloud_height(&fit, 2.5), Some(16.0));
        fit.n_inliers = 12;
        assert_eq!(cloud_height(&fit, 2.5), None);
        fit.n_inliers = 120;
        fit.z_continuous = false;
        assert_eq!(cloud_height(&fit, 2.5), None);
        fit.z_continuous = true;
        fit.source = PlaneSource::OsmRaw;
        assert_eq!(cloud_height(&fit, 2.5), None);
    }

    #[test]
    fn a_view_offset_measures_the_frame_it_is_read_in() {
        let w = wall([0.0, 0.0], [10.0, 0.0], [0.0, -1.0]);
        let mut frame = blank_fit("w1_0", [0.0, -1.0], 0.4, PlaneSource::Cloud);
        frame.a_ref = Some([0.0, -0.4]);
        frame.b_ref = Some([10.0, -0.4]);
        // The other view puts the wall 0.25 m further out at the midpoint and
        // turns it 2 degrees, so its own d has to carry the turn.
        let angle: f64 = 2.0f64.to_radians();
        let n = [angle.sin(), -angle.cos()];
        let d = 0.65 + n[0] * w.midpoint()[0] + n[1] * w.midpoint()[1];
        let mut other = blank_fit("w1_0", n, d, PlaneSource::Cloud);
        other.a_ref = Some([1.5, -0.65]);
        let off = per_view_offset(&frame, &other, &w);
        assert!((off.dn_m - 0.25).abs() < 0.02, "dn {}", off.dn_m);
        assert!((off.ds_m - 1.5).abs() < 0.02, "ds {}", off.ds_m);
        assert!(
            (off.dtheta_deg - 2.0).abs() < 1e-9,
            "dtheta {}",
            off.dtheta_deg
        );
    }

    // ----------------------------------------------------------------- golden

    use crate::mapillary::golden;
    use crate::mapillary::sfm::{self, Cluster};
    use std::collections::HashMap;

    /// Every plane fit of the fixture, from this port's own cloud search, with
    /// the wall foot and the two cues that hang off the fit.
    ///
    /// The fixture used to carry the order `cKDTree` handed the points to the
    /// Python, because the RANSAC's answer depended on it and no port can
    /// reproduce a scipy traversal. [`exact_line`] took the dependence out, so
    /// this feeds [`sfm::wall_points`] straight in and fits the reversal as well,
    /// which is the check that the two sides now agree on the point set alone.
    #[test]
    fn golden_plane_fits_match_the_python_run() {
        if golden::absent() {
            return;
        }

        let gf: golden::GoldenFrame = golden::load("frame.json");
        let frame = gf.frame();
        let params = Params::default();
        let walls: HashMap<String, Wall> = golden::load::<Vec<golden::GoldenWall>>("walls.json")
            .iter()
            .map(|w| (w.key.clone(), w.wall()))
            .collect();
        assert_eq!(walls.len(), 1030, "walls on the box");
        let clusters: HashMap<String, Cluster> =
            golden::load::<Vec<golden::GoldenCluster>>("sfm/clusters.json")
                .iter()
                .map(|c| (c.id.clone(), Cluster::from_raw(&c.raw(), &frame)))
                .collect();
        let samples: Vec<golden::GoldenPlaneSample> = golden::load_sfm("planes.json");
        assert!(samples.len() >= 20, "the sample is worth running");

        let mut worst_angle = 0.0f64;
        let mut worst_offset = 0.0f64;
        let mut worst_d = 0.0f64;
        let mut worst_rms = 0.0f64;
        let mut worst_z = 0.0f64;
        let mut worst_base = 0.0f64;
        let mut worst_view = 0.0f64;
        let mut worst_osm_angle = 0.0f64;
        let mut sources: HashMap<&str, usize> = HashMap::new();
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for s in &samples {
            let wall = &walls[&s.wall_key];
            let cam = s.camera.camera();
            let cluster = &clusters[&s.camera.cluster_id];
            let c_reg = [cam.centre[0], cam.centre[1]];

            let pts = sfm::wall_points(cluster, wall, s.camera.shift, cam.ground_z, c_reg, &params);
            let fit = fit_wall_plane(
                wall,
                &pts,
                &params,
                Some(&s.camera.pano_id),
                Some(&s.camera.cluster_id),
                s.camera.reg_accepted,
            );
            let label = format!("{} from {}", s.wall_key, s.camera.pano_id);
            // The same points backwards. The Python's own order is unportable, so
            // what makes the comparison below an equality rather than a tolerance
            // is that neither side depends on an order: the search is bit
            // identical, and only the refinement's sums move, in the last bits.
            let mut backwards = pts.clone();
            backwards.reverse();
            let search = |q: &[[f64; 3]]| {
                let xy: Vec<[f64; 2]> = q.iter().map(|p| [p[0], p[1]]).collect();
                exact_line(
                    &xy,
                    wall.n,
                    wall.a,
                    wall.b,
                    params.plane_band_m,
                    params.plane_max_angle_deg,
                    params.plane_angle_step_deg,
                )
            };
            assert_eq!(
                search(&pts),
                search(&backwards),
                "{label} band search depends on the point order"
            );
            let flipped = fit_wall_plane(
                wall,
                &backwards,
                &params,
                Some(&s.camera.pano_id),
                Some(&s.camera.cluster_id),
                s.camera.reg_accepted,
            );
            assert_eq!(flipped.source, fit.source, "{label} source by order");
            assert_eq!(flipped.n_inliers, fit.n_inliers, "{label} inliers by order");
            assert_eq!(
                flipped.ambiguity, fit.ambiguity,
                "{label} ambiguity by order"
            );
            assert!((flipped.d - fit.d).abs() < 1e-9, "{label} offset by order");
            assert_eq!(fit.source.as_str(), s.fit.source, "{label} plane source");
            worst_angle = worst_angle.max(golden::angle_between_deg(fit.n, s.fit.n));
            worst_offset = worst_offset.max((fit.offset_vs_osm_m - s.fit.offset_vs_osm_m).abs());
            worst_d = worst_d.max((fit.d - s.fit.d).abs());
            assert_eq!(fit.n_inliers, s.fit.n_inliers, "{label} inliers");
            assert_eq!(fit.z_continuous, s.fit.z_continuous, "{label} continuity");
            // The wall comes out of walls.json, whose endpoints and normal are
            // rounded, so anything divided by the length or measured against the
            // normal inherits that rounding rather than being exact.
            assert!(
                (fit.pts_per_m - s.fit.pts_per_m).abs() < 1e-4,
                "{label} points per metre"
            );
            assert!(
                (fit.ambiguity - s.fit.ambiguity).abs() < 1e-9,
                "{label} ambiguity"
            );
            worst_osm_angle =
                worst_osm_angle.max((fit.angle_vs_osm_deg - s.fit.angle_vs_osm_deg).abs());
            worst_rms = worst_rms.max((fit.rms_m - s.fit.rms_m).abs());
            match (fit.z_top98, s.fit.z_top98) {
                (Some(got), Some(want)) => worst_z = worst_z.max((got - want).abs()),
                (got, want) => assert_eq!(got.is_some(), want.is_some(), "{label} z_top98"),
            }
            *sources.entry(fit.source.as_str()).or_default() += 1;
            seen.insert(s.wall_key.as_str());

            // The wall foot this view measures its heights from.
            let (z_base, source) =
                sfm::wall_foot_z(cluster, wall, s.camera.shift, cam.ground_z, c_reg, &params);
            assert_eq!(source, s.z_base_source, "{label} z base source");
            worst_base = worst_base.max((z_base - s.z_base).abs());

            // The height cue, which is the fit and the foot together.
            match (cloud_height(&fit, z_base), s.h_cloud) {
                (Some(got), Some(want)) => {
                    assert!((got - want).abs() < 1e-6, "{label} cloud height")
                }
                (got, want) => assert_eq!(got.is_some(), want.is_some(), "{label} cloud height"),
            }

            // And where this fit sits in the wall's own frame.
            let mut framed = blank_fit(
                &s.wall_key,
                s.frame_fit.n,
                s.frame_fit.d,
                PlaneSource::from_str_or_default(&s.frame_fit.source),
            );
            framed.a_ref = s.frame_fit.a_ref;
            framed.b_ref = s.frame_fit.b_ref;
            let offset = per_view_offset(&framed, &fit, wall);
            worst_view = worst_view
                .max((offset.dn_m - s.offset_in_frame.dn_m).abs())
                .max((offset.ds_m - s.offset_in_frame.ds_m).abs())
                .max((offset.dtheta_deg - s.offset_in_frame.dtheta_deg).abs());
        }
        assert!(seen.len() >= 20, "{} distinct walls", seen.len());
        assert!(worst_angle < 0.5, "worst plane normal {worst_angle} deg");
        assert!(worst_offset < 0.05, "worst plane offset {worst_offset} m");
        assert!(worst_d < 0.05, "worst plane d {worst_d} m");
        assert!(worst_base < 0.01, "worst wall foot {worst_base} m");
        assert!(worst_rms < 1e-9 && worst_z < 1e-6 && worst_view < 1e-6);
        // acos of a number close to 1 loses half its digits, so the angle
        // against a normal rounded to nine decimals cannot be tighter than this.
        assert!(
            worst_osm_angle < 1e-3,
            "worst angle against OSM {worst_osm_angle} deg"
        );
        println!(
            "{} fits over {} walls ({sources:?}): worst normal {worst_angle:.2e} deg, offset \
             {worst_offset:.2e} m, d {worst_d:.2e} m, rms {worst_rms:.2e} m, z_top98 \
             {worst_z:.2e} m, wall foot {worst_base:.2e} m, view offset {worst_view:.2e}, \n             angle against OSM {worst_osm_angle:.2e} deg",
            samples.len(),
            seen.len()
        );
    }
}
