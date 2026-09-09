//! Which image can see which wall. Port of `tools/facade_lab/visibility.py`.
//!
//! Three layers, cheapest first, because downloads dominate the wall clock:
//!
//! * The pose-only gates, which decide everything before a single image is
//!   downloaded: the camera in front of the wall, distance 4 to 35 m (35 to 45 m
//!   kept as candidates flagged `FAR`), incidence under 60 degrees, angular
//!   width over 12 degrees, and then per class either the rig's nadir and zenith
//!   band for a panorama or the field of view gate for a phone (at least 35 per
//!   cent of an 11 by 5 grid of the wall rectangle on the image, samples within
//!   6 per cent of the border not counting, the midpoint column on the image,
//!   landscape only).
//! * The line of sight gate, nine samples 0.1 m in front of the wall against
//!   every footprint, run from the *registered* camera centre by
//!   [`super::register::run_align`].
//! * The image gates, which need the pixels: blur, night, exposure, clipping,
//!   quality score and the occluded share from the vegetation mask and the cloud
//!   depth, measured on the cheap 512 px preview [`preview_crop`] renders.
//!
//! Then [`select_views`], which keeps up to three views per wall.
//!
//! Two things measured on the Munich box that the port must keep or fix:
//!
//! * The blur gate stays **per wall and pooled**. A per class reference, whether
//!   per wall or per run, was built and measured and lost: blur is really the
//!   resampling ratio, not the sharpness of the photograph, and the per-run
//!   variant emptied seven walls including a fully covered tier A facade. What
//!   the per-wall median gives is that the median candidate has `rel = 1`, so
//!   at least half a wall's candidates always survive and the gate can never
//!   empty a wall. The long comment above the median in [`select_views`] is the
//!   Python's own, kept word for word. See MEASURED.md.
//! * The absolute floor `BLUR_MIN = 40` **is** class blind and is worth fixing:
//!   it rejects 25 per cent of Munich's panorama candidates against 3 per cent
//!   of the phone ones, and 76 against 4 per cent in New York, which is why New
//!   York comes out a phone-only city. Fix it in the Python first so the golden
//!   fixtures move with it.
//!
//! `Params::perspective_penalty` is 1.0, meaning no preference for panoramas
//! over phone frames. It was swept and buys nothing: 22 of the 23 walls whose
//! best view is a phone frame have no panorama that passes the gates at all, so
//! no score factor can reach them.
//!
//! Where this differs from the Python, and why:
//!
//! * **Footprint queries.** Shapely's STRtree and its `buffer(-r)` erosion are
//!   replaced by [`Footprints`], a bounding box scan, and by sphere tracing the
//!   distance to the ring for the erosion test: a segment meets the erosion of a
//!   simple polygon by `r` exactly where it holds a point that is inside and at
//!   least `r` from the boundary, which is a walk along the segment in steps of
//!   `r - dist` that cannot skip a hit. Shapely's own erosion is an offset
//!   polygon with the reflex corners rounded to 8 segments a quadrant, so it is
//!   the approximation of the two; the walk stops within 1e-4 m of exact.
//! * **Resampling.** OpenCV's `remap` quantises the bilinear weights to
//!   thirty-seconds of a pixel and then to 15-bit fixed point; [`remap_bilinear`]
//!   interpolates in `f64` and rounds once, and the source JPEG is decoded by a
//!   different decoder as well. Over the three crops of the fixture that is a
//!   mean of 0.12 to 0.61 grey levels and never more than 4, which moves the
//!   blur measure by 0.2 per cent against the one per cent it is held to.
//! * **The `> 512 px` branch of `image_gates`** is not ported. It downscales the
//!   crop before measuring, and [`preview_crop`] renders exactly 512 px wide, so
//!   no caller in the pipeline can reach it.
//! * **The vegetation mask** stays in `rectify.rs`, where the Python keeps it;
//!   the preview crop calls it there, and the crop fixture holds the mask the
//!   run measured, so that call is checked here rather than only in its own
//!   module.
//! * `_own_indices` without building keys is ported for `geometry.rs`, which is
//!   the one caller that has no key list.
//!
//! ## Closed: the occluded share the live runs once parted company on
//!
//! Measured on 2026-09-05 by running the whole driver against `out/munich_ref`
//! and dumping the gate table of the walls that came out furthest apart. On
//! `w79817227_4` the candidate `1775856856766917` measured `occlusion` 0.170 and
//! `los` 1.000 where the Python measured 0.430 and 0.667, so its score went
//! 0.221 to 0.392 and it took the third view slot from the wall's only
//! `ROOF_SKY` view; the wall fell back to its 12 m tag against the reference's
//! 19.75 m. On `w81190199_0` the best view `714038128188392` measured 0.007
//! against 0.147 and displaced the two views the reference ranked first.
//!
//! Nothing in this module was wrong. Both candidates were being measured from a
//! camera that was somewhere else: the port's registration had rejected them, so
//! each sat at its raw Graph pose, 3.91 m and 2.76 m from where the Python's
//! registration put it. A camera at its raw pose is *not* where the cloud thinks
//! it is, so the depth map it splats lands beside the wall the crop is rendered
//! on and `OCC_CLOUD` under-counts, which is why the port measured less
//! occlusion across the box rather than more; and it stands somewhere the
//! footprints do not block, which is the line of sight. `register.rs`'s header
//! has the cause and the fix; both candidates now reproduce the Python to the
//! last bit the fixture records.
//!
//! What the episode says about this module is that its fixtures were checking
//! the wrong half of it. `align/candidates.json` recomputes every gate from the
//! Python's own stored inputs and passes on all 8419 candidates however the crop
//! is rendered, and the crop fixture was three views the port already agreed on.
//! `golden_preview_crops_and_image_gates_match_the_python_run` is now 54
//! candidates over 10 panos of both classes, rendered from the thumbnail by this
//! port and compared occlusion bit by occlusion bit; the geometric planes come
//! out within 2 texels of the Python's over the whole sample.

#![allow(dead_code)]

use std::collections::BTreeMap;

use image::RgbImage;
use rayon::prelude::*;

use super::imgops::{self, round_half_even};
use super::pose::Projector;
use super::sfm::DepthMap;
use super::types::{
    Building, Camera, Gate, GateName, PanoMeta, Params, ViewCandidate, Wall, OCC_CLOUD,
    OCC_FOOTPRINT, OCC_NADIR, OCC_OUTSIDE, OCC_SEG, OCC_ZENITH,
};

/// A camera up to this far inside a footprint is tolerated, and that footprint
/// is shrunk by it for the ray test (CRITIQUE C22).
pub const LOS_INSIDE_TOL_M: f64 = 1.0;
/// The own footprint is shrunk by at least this for the ray test.
pub const OWN_SHRINK_M: f64 = 0.3;
/// Line of sight samples sit this far in front of the wall.
pub const SAMPLE_FRONT_M: f64 = 0.1;
pub const BLUR_MIN: f64 = 40.0;
pub const BLUR_REL: f64 = 0.35;
pub const MEAN_L_RANGE: [f64; 2] = [35.0, 220.0];
pub const CLIP_MAX: f64 = 0.12;
pub const NIGHT_L: f64 = 45.0;
pub const NIGHT_ELEV_DEG: f64 = -3.0;
pub const QUALITY_MIN: f64 = 0.5;
pub const OCC_MAX: f64 = 0.55;
/// Two views count as separate positions this far apart.
pub const MIN_POSITION_SEP_M: f64 = 3.0;
pub const PREVIEW_WIDTH: usize = 512;
/// The cloud has to be nearer than the wall by this much to count as an
/// occluder, so cloud noise on the wall itself does not mask it.
pub const CLOUD_OCC_MARGIN_M: f64 = 1.5;
/// The bits that count towards the occluded share.
pub const OCCLUDER_BITS: u8 = OCC_FOOTPRINT | OCC_CLOUD | OCC_SEG;

/// Samples along s and in h of the OSM rectangle for the field of view gate.
pub const FOV_GRID: [usize; 2] = [11, 5];

// --------------------------------------------------------------------------- footprints

/// One footprint as the ray tests see it: the exterior ring first, then the
/// holes, none of them closed, plus the bounding box for the cheap reject.
#[derive(Clone, Debug)]
pub struct Poly {
    pub rings: Vec<Vec<[f64; 2]>>,
    pub bbox: [f64; 4],
}

impl Poly {
    pub fn new(exterior: &[[f64; 2]], holes: &[Vec<[f64; 2]>]) -> Self {
        let mut rings = vec![exterior.to_vec()];
        rings.extend(holes.iter().cloned());
        let mut bbox = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for p in exterior {
            bbox[0] = bbox[0].min(p[0]);
            bbox[1] = bbox[1].min(p[1]);
            bbox[2] = bbox[2].max(p[0]);
            bbox[3] = bbox[3].max(p[1]);
        }
        Self { rings, bbox }
    }

    /// True for the strict interior, which is what shapely's `contains` means
    /// for a point: on the boundary is not inside.
    pub fn contains(&self, p: [f64; 2]) -> bool {
        if p[0] < self.bbox[0] || p[0] > self.bbox[2] || p[1] < self.bbox[1] || p[1] > self.bbox[3]
        {
            return false;
        }
        if !ring_contains(&self.rings[0], p) {
            return false;
        }
        !self.rings[1..].iter().any(|h| ring_contains(h, p))
    }

    /// Distance to the nearest ring segment, holes included.
    pub fn boundary_distance(&self, p: [f64; 2]) -> f64 {
        let mut best = f64::INFINITY;
        for ring in &self.rings {
            for (a, b) in ring_edges(ring) {
                best = best.min(point_segment_distance(p, a, b));
            }
        }
        best
    }

    /// True when the segment meets the polygon at all, boundary included. The
    /// same answer `LineString.intersects(Polygon)` gives.
    pub fn intersects_segment(&self, a: [f64; 2], b: [f64; 2]) -> bool {
        if !bbox_meets_segment(&self.bbox, a, b) {
            return false;
        }
        for ring in &self.rings {
            for (p, q) in ring_edges(ring) {
                if segments_cross(a, b, p, q) {
                    return true;
                }
            }
        }
        // No crossing: either both ends are outside or both are inside, and a
        // hole is the one case where inside the exterior is still outside.
        self.contains(a)
    }

    /// True when the segment meets this polygon shrunk by `r`.
    ///
    /// The erosion of a simple polygon by `r` is exactly the points that are
    /// inside and at least `r` from the boundary, so the test walks the segment
    /// in steps that cannot skip such a point: the signed distance is
    /// 1-Lipschitz along the segment, so from a point at distance `d < r` the
    /// next candidate is `r - d` away, and from outside it is `d` away.
    pub fn eroded_meets_segment(&self, a: [f64; 2], b: [f64; 2], r: f64) -> bool {
        if r <= 0.0 {
            return self.intersects_segment(a, b);
        }
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        if len < 1e-12 {
            return self.contains(a) && self.boundary_distance(a) >= r;
        }
        if !bbox_meets_segment(&expand_bbox(&self.bbox, -r), a, b) {
            return false;
        }
        let dir = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        let mut t = 0.0;
        while t <= len {
            let p = [a[0] + t * dir[0], a[1] + t * dir[1]];
            let d = self.boundary_distance(p);
            if self.contains(p) {
                if d >= r {
                    return true;
                }
                t += (r - d).max(1e-4);
            } else {
                t += d.max(1e-4);
            }
        }
        false
    }
}

/// Every footprint of the run with the building key it came from, rebuilt once
/// per run and shared by every wall.
#[derive(Clone, Debug, Default)]
pub struct Footprints {
    pub keys: Vec<String>,
    pub polys: Vec<Poly>,
}

impl Footprints {
    pub fn new(buildings: &[Building]) -> Self {
        Self {
            keys: buildings.iter().map(|b| b.key.clone()).collect(),
            polys: buildings
                .iter()
                .map(|b| Poly::new(&b.ring, &b.holes))
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.polys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.polys.is_empty()
    }

    /// Indices of the footprints whose own key is `key`.
    pub fn indices_of(&self, key: &str) -> Vec<usize> {
        self.keys
            .iter()
            .enumerate()
            .filter(|(_, k)| k.as_str() == key)
            .map(|(i, _)| i)
            .collect()
    }

    /// Indices of the footprints containing the point.
    pub fn containing(&self, p: [f64; 2]) -> Vec<usize> {
        (0..self.polys.len())
            .filter(|&i| self.polys[i].contains(p))
            .collect()
    }
}

/// Builds the footprint index.
pub fn footprint_index(buildings: &[Building]) -> Footprints {
    Footprints::new(buildings)
}

fn ring_edges(ring: &[[f64; 2]]) -> impl Iterator<Item = ([f64; 2], [f64; 2])> + '_ {
    (0..ring.len()).map(move |i| (ring[i], ring[(i + 1) % ring.len()]))
}

/// Even-odd containment of a closed ring, boundary excluded.
fn ring_contains(ring: &[[f64; 2]], p: [f64; 2]) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let mut inside = false;
    for (a, b) in ring_edges(ring) {
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0];
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

fn point_segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 < 1e-24 {
        0.0
    } else {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
    };
    let q = [a[0] + t * ab[0], a[1] + t * ab[1]];
    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()
}

fn expand_bbox(bbox: &[f64; 4], by: f64) -> [f64; 4] {
    [bbox[0] - by, bbox[1] - by, bbox[2] + by, bbox[3] + by]
}

fn bbox_meets_segment(bbox: &[f64; 4], a: [f64; 2], b: [f64; 2]) -> bool {
    a[0].min(b[0]) <= bbox[2]
        && a[0].max(b[0]) >= bbox[0]
        && a[1].min(b[1]) <= bbox[3]
        && a[1].max(b[1]) >= bbox[1]
}

/// True when two closed segments share a point, touching included.
fn segments_cross(p1: [f64; 2], p2: [f64; 2], q1: [f64; 2], q2: [f64; 2]) -> bool {
    let d = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    };
    let on = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        c[0] <= a[0].max(b[0])
            && c[0] >= a[0].min(b[0])
            && c[1] <= a[1].max(b[1])
            && c[1] >= a[1].min(b[1])
    };
    let (d1, d2, d3, d4) = (d(p1, p2, q1), d(p1, p2, q2), d(q1, q2, p1), d(q1, q2, p2));
    if ((d1 > 0.0) != (d2 > 0.0))
        && ((d3 > 0.0) != (d4 > 0.0))
        && d1 != 0.0
        && d2 != 0.0
        && d3 != 0.0
        && d4 != 0.0
    {
        return true;
    }
    (d1 == 0.0 && on(p1, p2, q1))
        || (d2 == 0.0 && on(p1, p2, q2))
        || (d3 == 0.0 && on(q1, q2, p1))
        || (d4 == 0.0 && on(q1, q2, p2))
}

/// The footprints a wall belongs to, found geometrically because the caller has
/// no key list. Only `geometry.rs` needs this; everything downstream passes the
/// building keys.
fn own_indices_geometric(wall: &Wall, fps: &Footprints) -> Vec<usize> {
    let mid = wall.midpoint();
    let probe = [mid[0] - 0.75 * wall.n[0], mid[1] - 0.75 * wall.n[1]];
    let seg_box = [
        wall.a[0].min(wall.b[0]) - 0.5,
        wall.a[1].min(wall.b[1]) - 0.5,
        wall.a[0].max(wall.b[0]) + 0.5,
        wall.a[1].max(wall.b[1]) + 0.5,
    ];
    let mut out = Vec::new();
    for (i, poly) in fps.polys.iter().enumerate() {
        let bb = &poly.bbox;
        if seg_box[0] > bb[2] || seg_box[2] < bb[0] || seg_box[1] > bb[3] || seg_box[3] < bb[1] {
            continue;
        }
        let touches =
            poly.boundary_distance(wall.a) < 0.05 && poly.boundary_distance(wall.b) < 0.05;
        if poly.contains(probe) || touches {
            out.push(i);
        }
    }
    out
}

/// How far the wall chord dips inside the ring, zero for a raw edge.
///
/// A merged wall is a chord of several ring edges and can therefore run inside
/// the footprint; the own polygon has to be shrunk by at least that much or the
/// wall would occlude itself.
fn chord_sag(wall: &Wall, fps: &Footprints, own: &[usize]) -> f64 {
    let mut sag = 0.0f64;
    for &i in own {
        for p in &fps.polys[i].rings[0] {
            let (s, _, d) = wall.sh_of([p[0], p[1], 0.0], 0.0);
            if s >= -0.05 && s <= wall.length + 0.05 && d > 0.0 && d < 2.0 {
                sag = sag.max(d);
            }
        }
    }
    sag
}

/// The share of the wall visible from `c` and the visible interval along it.
///
/// A ray to a sample 0.1 m in front of the wall is blocked when it meets any
/// other footprint, or the own footprint shrunk by `max(0.3 m, sag + 0.2 m)`. A
/// camera inside a footprint is tolerated up to 1 m from its boundary and that
/// footprint is shrunk by the tolerance for the test; deeper inside the wall is
/// not visible at all.
pub fn line_of_sight(
    wall: &Wall,
    c: [f64; 2],
    fps: &Footprints,
    own_key: Option<&str>,
    params: &Params,
) -> (f64, [f64; 2]) {
    let own = match own_key {
        Some(k) => fps.indices_of(k),
        None => own_indices_geometric(wall, fps),
    };
    // A camera standing inside a footprint sees out of it, so that footprint is
    // shrunk rather than treated as an occluder; deeper than the tolerance and
    // it is indoors and sees nothing.
    let mut lenient: Vec<(usize, f64)> = Vec::new();
    for i in fps.containing(c) {
        let depth = fps.polys[i].boundary_distance(c);
        if depth > LOS_INSIDE_TOL_M {
            return (0.0, [0.0, 0.0]);
        }
        lenient.push((i, LOS_INSIDE_TOL_M + 0.05));
    }
    let shrink_own = OWN_SHRINK_M.max(chord_sag(wall, fps, &own) + 0.2);
    let n = (params.los_samples as usize).max(1);
    let t = wall.tangent();
    let mut visible = Vec::with_capacity(n);
    for k in 0..n {
        let s = wall.length * (k as f64 + 0.5) / n as f64;
        let p = [
            wall.a[0] + s * t[0] + SAMPLE_FRONT_M * wall.n[0],
            wall.a[1] + s * t[1] + SAMPLE_FRONT_M * wall.n[1],
        ];
        let mut blocked = false;
        for (i, poly) in fps.polys.iter().enumerate() {
            if let Some(&(_, r)) = lenient.iter().find(|(j, _)| *j == i) {
                if poly.eroded_meets_segment(c, p, r) {
                    blocked = true;
                    break;
                }
            } else if own.contains(&i) {
                if poly.eroded_meets_segment(c, p, shrink_own) {
                    blocked = true;
                    break;
                }
            } else if poly.intersects_segment(c, p) {
                blocked = true;
                break;
            }
        }
        visible.push((s, !blocked));
    }
    let seen: Vec<f64> = visible
        .iter()
        .filter(|(_, ok)| *ok)
        .map(|(s, _)| *s)
        .collect();
    if seen.is_empty() {
        return (0.0, [0.0, 0.0]);
    }
    let half = 0.5 * wall.length / n as f64;
    let lo = seen.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = seen.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (
        seen.len() as f64 / n as f64,
        [(lo - half).max(0.0), (hi + half).min(wall.length)],
    )
}

// --------------------------------------------------------------------------- geometric gates

/// The ground under a camera, falling back to the height prior.
fn z_base_of(cam: &Camera) -> f64 {
    if cam.ground_z.is_finite() {
        cam.ground_z
    } else {
        cam.centre[2] - cam.cam_height_m
    }
}

/// The wall height to gate against: the OSM one, else the default.
pub fn wall_height_m(wall: &Wall, params: &Params) -> f64 {
    match wall.height_osm {
        Some(h) if h != 0.0 => h,
        _ => params.default_height_m,
    }
}

/// `g = cos(inc) * clamp(1 - |d - 12| / 30, 0.2, 1) * min(1, angwidth / 40 deg)`.
pub fn geometric_score(dist_m: f64, incidence_deg: f64, angwidth_deg: f64) -> f64 {
    incidence_deg.to_radians().cos()
        * (1.0 - (dist_m - 12.0).abs() / 30.0).clamp(0.2, 1.0)
        * (angwidth_deg / 40.0).min(1.0)
}

/// Coverage gate of a perspective camera.
///
/// A phone at 66 by 52 degrees rarely holds a whole 15 to 20 m wall at 5 to
/// 12 m, so the rig's nadir and zenith band is replaced by a coverage rule on an
/// 11 by 5 grid of the OSM rectangle: a sample counts when it projects at all
/// (in front of the camera and inside the radial validity domain) and lands
/// further than `persp_margin` from every border. The midpoint column must be on
/// the image too, which is what rejects a camera looking along the street.
///
/// Returns `(ok, share on image, midpoint column on image, the s interval the
/// image holds)`.
fn perspective_fov(
    wall: &Wall,
    cam: &Camera,
    z_base: f64,
    h_top: f64,
    params: &Params,
) -> (bool, f64, bool, [f64; 2]) {
    let (ns, nh) = (FOV_GRID[0], FOV_GRID[1]);
    let projector = Projector::new(cam);
    let m = params.persp_margin;
    let mut on_cols = vec![false; ns];
    let mut on_count = 0usize;
    let mut mid_ok = false;
    for (j, col) in on_cols.iter_mut().enumerate() {
        let s = if ns > 1 {
            wall.length * j as f64 / (ns - 1) as f64
        } else {
            0.0
        };
        for i in 0..nh {
            let h = if nh > 1 {
                h_top * i as f64 / (nh - 1) as f64
            } else {
                0.0
            };
            let proj = projector.project(wall.point(s, h, z_base, params.eps_m));
            let on = proj.u.is_finite()
                && proj.v.is_finite()
                && proj.u >= m
                && proj.u <= 1.0 - m
                && proj.v >= m
                && proj.v <= 1.0 - m;
            if on {
                on_count += 1;
                *col = true;
                if j == ns / 2 {
                    mid_ok = true;
                }
            }
        }
    }
    let mut frac = on_count as f64 / (ns * nh) as f64;
    let first = on_cols.iter().position(|v| *v);
    let last = on_cols.iter().rposition(|v| *v);
    let s_on = match (first, last) {
        (Some(f), Some(l)) => {
            let half = 0.5 * wall.length / (ns - 1) as f64;
            let s_of = |j: usize| wall.length * j as f64 / (ns - 1) as f64;
            [(s_of(f) - half).max(0.0), (s_of(l) + half).min(wall.length)]
        }
        _ => [0.0, 0.0],
    };
    // A portrait phone shot is dropped outright: its orientation is unreliable.
    if params.persp_landscape_only && cam.height > cam.width {
        frac = 0.0;
        mid_ok = false;
    }
    let ok = frac >= params.fov_min_on_image && mid_ok;
    (ok, frac, mid_ok, s_on)
}

/// Every (wall, camera) pair with the camera on the outward side within
/// `far_dist_m`, with the reason each rejected one failed.
///
/// The line of sight gate is added when `fps` is given, which is what
/// `run_align` does from the registered centres; `geometry.rs` calls this
/// without it, before registration.
pub fn geometric_candidates(
    wall: &Wall,
    cameras: &BTreeMap<String, Camera>,
    params: &Params,
    fps: Option<&Footprints>,
) -> Vec<ViewCandidate> {
    let mut out = Vec::new();
    let (lo, hi) = (params.view_dist[0], params.view_dist[1]);
    let mid = wall.midpoint();
    let h_top = wall_height_m(wall, params);
    for (pid, cam) in cameras {
        let cxy = [cam.centre[0], cam.centre[1]];
        let d_n = (cxy[0] - wall.a[0]) * wall.n[0] + (cxy[1] - wall.a[1]) * wall.n[1];
        if d_n <= 0.0 {
            continue; // behind the wall: not a candidate at all
        }
        let v = [cxy[0] - mid[0], cxy[1] - mid[1]];
        let dist = (v[0] * v[0] + v[1] * v[1]).sqrt();
        if dist < 1e-6 || dist > params.far_dist_m {
            continue;
        }
        let cos_inc = ((v[0] / dist) * wall.n[0] + (v[1] / dist) * wall.n[1]).clamp(-1.0, 1.0);
        let inc = cos_inc.acos().to_degrees();
        let va = [wall.a[0] - cxy[0], wall.a[1] - cxy[1]];
        let vb = [wall.b[0] - cxy[0], wall.b[1] - cxy[1]];
        let na = (va[0] * va[0] + va[1] * va[1]).sqrt();
        let nb = (vb[0] * vb[0] + vb[1] * vb[1]).sqrt();
        let aw = (((va[0] * vb[0] + va[1] * vb[1]) / (na * nb).max(1e-9)).clamp(-1.0, 1.0))
            .acos()
            .to_degrees();
        let mut gates = vec![Gate {
            name: GateName::Outward,
            passed: true,
            value: d_n,
        }];
        gates.push(Gate {
            name: GateName::Near,
            passed: dist >= lo,
            value: dist,
        });
        gates.push(Gate {
            name: GateName::Far,
            passed: dist <= hi,
            value: dist,
        });
        gates.push(Gate {
            name: GateName::Incidence,
            passed: inc <= params.max_incidence_deg,
            value: inc,
        });
        gates.push(Gate {
            name: GateName::Angwidth,
            passed: aw >= params.min_angwidth_deg,
            value: aw,
        });
        let z_base = z_base_of(cam);
        let projector = Projector::new(cam);
        let mut v_bot_max = f64::NEG_INFINITY;
        let mut v_top_min = f64::INFINITY;
        for s in [0.0, 0.5 * wall.length, wall.length] {
            let bot = projector
                .project(wall.point(s, 0.0, z_base, params.eps_m))
                .v;
            let top = projector
                .project(wall.point(s, h_top, z_base, params.eps_m))
                .v;
            // A perspective sample can fail to project at all; the Python fills
            // those with 9 and -9 so the reason string still formats.
            v_bot_max = v_bot_max.max(if bot.is_finite() { bot } else { 9.0 });
            v_top_min = v_top_min.min(if top.is_finite() { top } else { -9.0 });
        }
        let (v_lo, v_hi) = (params.v_range[0], params.v_range[1]);
        let mut s_on: Option<[f64; 2]> = None;
        let mut fov_ok = false;
        let mut frac = 0.0;
        let mut mid_ok = false;
        if cam.is_spherical() {
            gates.push(Gate {
                name: GateName::Nadir,
                passed: v_bot_max <= v_hi,
                value: v_bot_max,
            });
            gates.push(Gate {
                name: GateName::Zenith,
                passed: v_top_min >= v_lo,
                value: v_top_min,
            });
        } else {
            // The nadir and zenith names are kept so the sheet and the reason
            // codes stay the same; fov carries the same (ok, share).
            let (ok, f, m, s) = perspective_fov(wall, cam, z_base, h_top, params);
            fov_ok = ok;
            frac = f;
            mid_ok = m;
            s_on = Some(s);
            gates.push(Gate {
                name: GateName::Fov,
                passed: ok,
                value: f,
            });
            gates.push(Gate {
                name: GateName::Nadir,
                passed: ok,
                value: f,
            });
            gates.push(Gate {
                name: GateName::Zenith,
                passed: true,
                value: f,
            });
        }
        let nadir_ok = if cam.is_spherical() {
            v_bot_max <= v_hi
        } else {
            fov_ok
        };
        let zenith_ok = if cam.is_spherical() {
            v_top_min >= v_lo
        } else {
            true
        };
        let reason: Option<String> = if dist < lo {
            Some(format!("NEAR {dist:.1} m"))
        } else if dist > hi {
            Some(format!("FAR {dist:.0} m"))
        } else if inc > params.max_incidence_deg {
            Some(format!("incidence {inc:.0} deg"))
        } else if aw < params.min_angwidth_deg {
            Some(format!("angwidth {aw:.0} deg"))
        } else if !nadir_ok {
            if cam.is_spherical() {
                Some(format!("nadir v {v_bot_max:.2}"))
            } else if frac >= params.fov_min_on_image && !mid_ok {
                Some(format!(
                    "FOV midpoint off image ({:.0} % on image)",
                    100.0 * frac
                ))
            } else {
                Some(format!("FOV {:.0} % on image", 100.0 * frac))
            }
        } else if !zenith_ok {
            Some(format!("zenith v {v_top_min:.2}"))
        } else {
            None
        };
        let mut cand = ViewCandidate {
            wall_key: wall.key.clone(),
            pano_id: pid.clone(),
            dist_m: dist,
            incidence_deg: inc,
            angwidth_deg: aw,
            visible_frac: 0.0,
            s_vis: [0.0, 0.0],
            g_score: geometric_score(dist, inc, aw),
            gates,
            f_occ: 0.0,
            blur: 0.0,
            score: 0.0,
            rejected_reason: reason,
        };
        match (fps, cand.rejected_reason.is_none()) {
            (Some(fps), true) => {
                let (frac, s_vis) =
                    line_of_sight(wall, cxy, fps, Some(wall.building_key.as_str()), params);
                cand.visible_frac = frac;
                cand.s_vis = s_vis;
                cand.set_gate(GateName::Los, frac >= params.los_min_visible, frac);
                if frac < params.los_min_visible {
                    cand.rejected_reason = Some(format!("LOS {:.0} %", 100.0 * frac));
                }
            }
            (None, true) => {
                cand.visible_frac = 1.0;
                cand.s_vis = [0.0, wall.length];
            }
            _ => {}
        }
        if let (Some(s_on), true) = (s_on, cand.rejected_reason.is_none()) {
            // Perspective: the off-image columns are no data for the loose
            // crop's s_vis path and for the extent step, so s_vis is narrowed to
            // what the image actually holds.
            cand.s_vis = [cand.s_vis[0].max(s_on[0]), cand.s_vis[1].min(s_on[1])];
        }
        out.push(cand);
    }
    out
}

/// Sets `reachable` and `unreachable_reason` on every wall and hands back the
/// candidates per wall, so a caller can write them without recomputing.
pub fn mark_reachable(
    walls: &mut [Wall],
    cameras: &BTreeMap<String, Camera>,
    fps: &Footprints,
    params: &Params,
    with_keys: bool,
) -> BTreeMap<String, Vec<ViewCandidate>> {
    // Every wall's gates are a pure function of that wall, the cameras and the
    // footprints, and a city is a thousand walls against a thousand cameras, so
    // the candidates are gathered in parallel and the flags set afterwards in
    // wall order. The answers are identical either way.
    let per_wall: Vec<Vec<ViewCandidate>> = walls
        .par_iter()
        .map(|w| {
            if with_keys {
                geometric_candidates(w, cameras, params, Some(fps))
            } else {
                geometric_candidates_no_keys(w, cameras, params, fps)
            }
        })
        .collect();
    let mut out = BTreeMap::new();
    for (w, cands) in walls.iter_mut().zip(per_wall) {
        let geo_ok = cands.iter().any(|c| {
            c.rejected_reason.is_none()
                || c.rejected_reason
                    .as_deref()
                    .is_some_and(|r| r.starts_with("LOS"))
        });
        let los_ok = cands.iter().any(|c| c.rejected_reason.is_none());
        if los_ok {
            w.reachable = true;
            w.unreachable_reason = String::new();
        } else if geo_ok {
            w.reachable = false;
            w.unreachable_reason = "no_line_of_sight".into();
        } else {
            w.reachable = false;
            w.unreachable_reason = "no_camera_in_front".into();
        }
        out.insert(w.key.clone(), cands);
    }
    out
}

/// The candidates with the line of sight gate but no building keys, which is
/// how `geometry.rs` calls it before the buildings are keyed up.
fn geometric_candidates_no_keys(
    wall: &Wall,
    cameras: &BTreeMap<String, Camera>,
    params: &Params,
    fps: &Footprints,
) -> Vec<ViewCandidate> {
    let mut cands = geometric_candidates(wall, cameras, params, None);
    for cand in &mut cands {
        if cand.rejected_reason.is_some() {
            cand.visible_frac = 0.0;
            cand.s_vis = [0.0, 0.0];
            continue;
        }
        let cam = &cameras[&cand.pano_id];
        let (frac, s_vis) = line_of_sight(wall, [cam.centre[0], cam.centre[1]], fps, None, params);
        cand.visible_frac = frac;
        cand.s_vis = s_vis;
        cand.set_gate(GateName::Los, frac >= params.los_min_visible, frac);
        if frac < params.los_min_visible {
            cand.rejected_reason = Some(format!("LOS {:.0} %", 100.0 * frac));
        }
    }
    cands
}

// --------------------------------------------------------------------------- sun

/// Solar elevation in degrees, good to about a degree: the NOAA style ecliptic
/// longitude and GMST from the J2000 day number.
pub fn solar_elevation_deg(captured_at_ms: i64, lon: f64, lat: f64) -> f64 {
    let n = captured_at_ms as f64 / 86_400_000.0 - 10957.5;
    let l = (280.460 + 0.9856474 * n).rem_euclid(360.0);
    let g = (357.528 + 0.9856003 * n).rem_euclid(360.0).to_radians();
    let lam = (l + 1.915 * g.sin() + 0.020 * (2.0 * g).sin()).to_radians();
    let eps = (23.439 - 0.0000004 * n).to_radians();
    let ra = (eps.cos() * lam.sin()).atan2(lam.cos());
    let dec = (eps.sin() * lam.sin()).asin();
    let gmst_h = (18.697374558 + 24.06570982441908 * n).rem_euclid(24.0);
    let ha = (gmst_h * 15.0 + lon).rem_euclid(360.0).to_radians() - ra;
    let la = lat.to_radians();
    (la.sin() * dec.sin() + la.cos() * dec.cos() * ha.cos())
        .asin()
        .to_degrees()
}

// --------------------------------------------------------------------------- preview renderer

/// The cheap loose crop the image gates are measured on.
#[derive(Clone, Debug)]
pub struct Preview {
    pub rgb: RgbImage,
    /// Row major occlusion bits, one per texel.
    pub occl: Vec<u8>,
    pub ppm: f64,
    pub ppm_v: f64,
    pub s0: f64,
    pub s1: f64,
    pub h_top: f64,
    pub h_bot: f64,
    /// Column under the camera and the camera-height row.
    pub x_foot: f64,
    pub y_cam: f64,
}

/// What a preview may be narrowed to, all optional the way the Python's
/// keyword arguments are.
#[derive(Clone, Copy, Debug)]
pub struct PreviewOpts<'a> {
    pub depth: Option<&'a DepthMap>,
    pub s_range: Option<[f64; 2]>,
    pub s_vis: Option<[f64; 2]>,
    pub h_top: Option<f64>,
    pub width: usize,
    pub max_rows: usize,
}

impl Default for PreviewOpts<'_> {
    fn default() -> Self {
        Self {
            depth: None,
            s_range: None,
            s_vis: None,
            h_top: None,
            width: PREVIEW_WIDTH,
            max_rows: 1024,
        }
    }
}

/// A cheap loose crop of the wall plane from one image.
///
/// `wall` may be `plane::fitted_wall`'s output, in which case the crop is on the
/// fitted plane. `s` runs over `s_range` (the whole wall by default) at
/// `width / (s1 - s0)` px per metre and `h` over `[-0.5, h_top]`, `h_top`
/// defaulting to 1.15 times the OSM height so the roofline is in the picture.
///
/// The occlusion plane carries `OCC_NADIR` and `OCC_ZENITH` for a panorama's rig
/// band, `OCC_OUTSIDE` where a perspective texel lands off the image,
/// `OCC_CLOUD` where the depth map is nearer than the ray by more than 1.5 m,
/// `OCC_FOOTPRINT` on the columns outside `s_vis`, and `OCC_SEG` on the
/// vegetation. The gate has to see the canopy the loose crop will mask, or a
/// view that is mostly tree passes as clean and wins the wall.
pub fn preview_crop(
    wall: &Wall,
    cam: &Camera,
    z_base: f64,
    image: &RgbImage,
    params: &Params,
    opts: &PreviewOpts,
) -> Preview {
    let (img_w, img_h) = (image.width() as f64, image.height() as f64);
    let (mut s0, mut s1) = match opts.s_range {
        Some(r) => (r[0], r[1]),
        None => (0.0, wall.length),
    };
    if s1 - s0 < 0.5 {
        s0 = 0.0;
        s1 = wall.length.max(0.5);
    }
    let h_top = opts.h_top.unwrap_or(1.15 * wall_height_m(wall, params));
    let h_bot = -0.5;
    let width = opts.width.max(1);
    let ppm = width as f64 / (s1 - s0);
    let rows =
        (opts.max_rows as f64).min((8.0f64).max(round_half_even((h_top - h_bot) * ppm))) as usize;
    let rows = rows.max(1);
    let ppm_v = rows as f64 / (h_top - h_bot);
    let projector = Projector::new(cam);
    let spherical = cam.is_spherical();

    let mut map_u = vec![0.0f64; rows * width];
    let mut map_v = vec![0.0f64; rows * width];
    let mut ray = vec![0.0f64; rows * width];
    let mut outside = vec![false; rows * width];
    for r in 0..rows {
        let h = h_top - (r as f64 + 0.5) / ppm_v;
        for c in 0..width {
            let s = s0 + (c as f64 + 0.5) / ppm;
            let p = wall.point(s, h, z_base, params.eps_m);
            let proj = projector.project(p);
            let k = r * width + c;
            ray[k] = proj.length_m;
            if spherical {
                map_u[k] = (proj.u * img_w - 0.5).rem_euclid(img_w);
                map_v[k] = (proj.v * img_h - 0.5).clamp(0.0, img_h - 1.0);
            } else if proj.inside_image() {
                map_u[k] = proj.u * img_w - 0.5;
                map_v[k] = proj.v * img_h - 0.5;
            } else {
                // Off the image points far outside, so the border fill leaves it
                // blank and the caller masks it rather than wrapping a pixel in.
                map_u[k] = -1e6;
                map_v[k] = -1e6;
                outside[k] = true;
            }
        }
    }
    let rgb = remap_bilinear(image, &map_u, &map_v, width, rows, spherical);

    let mut occl = vec![0u8; rows * width];
    for k in 0..rows * width {
        let v = (map_v[k] + 0.5) / img_h;
        if spherical {
            if v > params.v_range[1] {
                occl[k] |= OCC_NADIR;
            }
            if v < params.v_range[0] {
                occl[k] |= OCC_ZENITH;
            }
        } else if outside[k] {
            occl[k] |= OCC_OUTSIDE;
        }
    }
    if let Some(depth) = opts.depth {
        let (dw, dh) = (depth.width as usize, depth.height as usize);
        for k in 0..rows * width {
            if outside[k] {
                continue;
            }
            let u = (map_u[k] + 0.5) / img_w;
            let mut v = (map_v[k] + 0.5) / img_h;
            if !spherical {
                v = v.clamp(0.0, 1.0);
            }
            let col = ((u * dw as f64) as i64).clamp(0, dw as i64 - 1) as u32;
            let row = ((v * dh as f64) as i64).clamp(0, dh as i64 - 1) as u32;
            let dz = depth.at(col, row);
            if dz.is_finite() && f64::from(dz) < ray[k] - CLOUD_OCC_MARGIN_M {
                occl[k] |= OCC_CLOUD;
            }
        }
    }
    if let Some(sv) = opts.s_vis {
        for c in 0..width {
            let s = s0 + (c as f64 + 0.5) / ppm;
            if s < sv[0] || s > sv[1] {
                for r in 0..rows {
                    occl[r * width + c] |= OCC_FOOTPRINT;
                }
            }
        }
    }
    for (k, veg) in super::rectify::vegetation_mask(&rgb, ppm)
        .iter()
        .enumerate()
    {
        if *veg {
            occl[k] |= OCC_SEG;
        }
    }
    let (s_cam, h_cam, _) = wall.sh_of(cam.centre, z_base);
    Preview {
        rgb,
        occl,
        ppm,
        ppm_v,
        s0,
        s1,
        h_top,
        h_bot,
        x_foot: (s_cam - s0) * ppm,
        y_cam: (h_top - h_cam) * ppm_v,
    }
}

/// Bilinear resampling of an image through inverse maps in pixel-centre
/// coordinates, wrapping in u for a panorama and filling black off a
/// perspective image.
pub fn remap_bilinear(
    src: &RgbImage,
    map_u: &[f64],
    map_v: &[f64],
    width: usize,
    rows: usize,
    wrap: bool,
) -> RgbImage {
    let (sw, sh) = (src.width() as i64, src.height() as i64);
    let mut out = RgbImage::new(width as u32, rows as u32);
    for r in 0..rows {
        for c in 0..width {
            let k = r * width + c;
            let (u, v) = (map_u[k], map_v[k]);
            let x0 = u.floor();
            let y0 = v.floor();
            let (fx, fy) = (u - x0, v - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            let mut acc = [0.0f64; 3];
            for (dy, wy) in [(0i64, 1.0 - fy), (1, fy)] {
                for (dx, wx) in [(0i64, 1.0 - fx), (1, fx)] {
                    let w = wx * wy;
                    if w == 0.0 {
                        continue;
                    }
                    let (mut x, y) = (x0 + dx, y0 + dy);
                    if wrap {
                        x = x.rem_euclid(sw);
                    } else if x < 0 || x >= sw || y < 0 || y >= sh {
                        continue;
                    }
                    if y < 0 || y >= sh || x < 0 || x >= sw {
                        continue;
                    }
                    let px = src.get_pixel(x as u32, y as u32).0;
                    for i in 0..3 {
                        acc[i] += w * f64::from(px[i]);
                    }
                }
            }
            out.put_pixel(
                c as u32,
                r as u32,
                image::Rgb([
                    acc[0].round().clamp(0.0, 255.0) as u8,
                    acc[1].round().clamp(0.0, 255.0) as u8,
                    acc[2].round().clamp(0.0, 255.0) as u8,
                ]),
            );
        }
    }
    out
}

// --------------------------------------------------------------------------- image gates

/// OpenCV's fixed point RGB to grey, which is what the Python measures on.
///
/// The weights are the 15-bit ones `cvtColor` uses today (9798, 19235, 3735),
/// not the 14-bit pair doubled: `19235` is not `2 * 9617` and `3735` is not
/// `2 * 1868`, and the difference shows up as one grey level on about a
/// thousandth of the pixels of a crop, which is enough to move a Laplacian
/// variance by more than the one per cent the blur gate is compared at.
fn rgb_to_gray(px: [u8; 3]) -> u8 {
    ((u32::from(px[0]) * 9798 + u32::from(px[1]) * 19235 + u32::from(px[2]) * 3735 + 16384) >> 15)
        as u8
}

/// `BORDER_REFLECT_101`: index -1 mirrors to 1, index n to n - 2.
fn reflect101(i: i64, n: usize) -> usize {
    if n == 1 {
        return 0;
    }
    let n = n as i64;
    let period = 2 * (n - 1);
    let mut m = i.rem_euclid(period);
    if m >= n {
        m = period - m;
    }
    m as usize
}

/// The image based gates, measured on the preview crop.
///
/// Blur is the variance of the Laplacian over the texels with no occlusion bit,
/// exposure the mean grey in `[35, 220]` with under 12 per cent clipped, night
/// the solar elevation under -3 degrees or a mean grey under 45, plus the
/// quality score and the occluded share. When under a fifth of the crop is
/// clear the whole crop is judged instead, except what a perspective camera
/// never saw: `OCC_OUTSIDE` is border fill, not image content.
///
/// The Python also takes the camera and the params and reads neither, so they
/// are not arguments here.
pub fn image_gates(
    crop_rgb: &RgbImage,
    occl: &[u8],
    meta: &PanoMeta,
    lon: Option<f64>,
    lat: Option<f64>,
) -> Vec<Gate> {
    let (w, h) = (crop_rgb.width() as usize, crop_rgb.height() as usize);
    let n = w * h;
    let gray: Vec<f64> = crop_rgb
        .pixels()
        .map(|p| f64::from(rgb_to_gray(p.0)))
        .collect();
    let mut clear: Vec<bool> = occl.iter().map(|o| *o == 0).collect();
    let share = clear.iter().filter(|c| **c).count() as f64 / n.max(1) as f64;
    if share < 0.2 {
        clear = occl.iter().map(|o| (*o & OCC_OUTSIDE) == 0).collect();
    }
    let any = clear.iter().any(|c| *c);
    let lap = laplacian(&gray, w, h);
    let (blur, mean_l, clipped) = if any {
        let vals: Vec<f64> = (0..n).filter(|k| clear[*k]).map(|k| lap[k]).collect();
        let mu = vals.iter().sum::<f64>() / vals.len() as f64;
        let var = vals.iter().map(|v| (v - mu) * (v - mu)).sum::<f64>() / vals.len() as f64;
        let g: Vec<f64> = (0..n).filter(|k| clear[*k]).map(|k| gray[k]).collect();
        let mean_l = g.iter().sum::<f64>() / g.len() as f64;
        let clipped =
            g.iter().filter(|v| **v <= 5.0 || **v >= 250.0).count() as f64 / g.len() as f64;
        (var, mean_l, clipped)
    } else {
        (0.0, 0.0, 1.0)
    };
    let lon = lon.unwrap_or(meta.lon);
    let lat = lat.unwrap_or(meta.lat);
    let elev = if meta.captured_at != 0 {
        solar_elevation_deg(meta.captured_at, lon, lat)
    } else {
        90.0
    };
    let night = elev < NIGHT_ELEV_DEG || mean_l < NIGHT_L;
    let f_occ = occl.iter().filter(|o| (**o & OCCLUDER_BITS) != 0).count() as f64 / n.max(1) as f64;
    vec![
        Gate {
            name: GateName::Blur,
            passed: blur >= BLUR_MIN,
            value: blur,
        },
        Gate {
            name: GateName::Exposure,
            passed: (MEAN_L_RANGE[0]..=MEAN_L_RANGE[1]).contains(&mean_l) && clipped < CLIP_MAX,
            value: mean_l,
        },
        Gate {
            name: GateName::Clipped,
            passed: clipped < CLIP_MAX,
            value: clipped,
        },
        Gate {
            name: GateName::Night,
            passed: !night,
            value: elev,
        },
        Gate {
            name: GateName::Quality,
            passed: meta.quality >= QUALITY_MIN,
            value: meta.quality,
        },
        Gate {
            name: GateName::Occlusion,
            passed: f_occ <= OCC_MAX,
            value: f_occ,
        },
    ]
}

/// The 4-neighbour Laplacian OpenCV computes for `ksize = 1`, with the border
/// reflected without repeating the edge pixel.
fn laplacian(gray: &[f64], w: usize, h: usize) -> Vec<f64> {
    let mut out = vec![0.0f64; w * h];
    for y in 0..h {
        for x in 0..w {
            let up = gray[reflect101(y as i64 - 1, h) * w + x];
            let down = gray[reflect101(y as i64 + 1, h) * w + x];
            let left = gray[y * w + reflect101(x as i64 - 1, w)];
            let right = gray[y * w + reflect101(x as i64 + 1, w)];
            out[y * w + x] = up + down + left + right - 4.0 * gray[y * w + x];
        }
    }
    out
}

/// Merges the image gates into a candidate: the occluded share, the blur, and
/// the first failing reason, but only when it had passed the geometric ones.
pub fn apply_image_gates(cand: &mut ViewCandidate, gates: &[Gate]) {
    for g in gates {
        cand.set_gate(g.name, g.passed, g.value);
    }
    let value = |name: GateName| {
        gates
            .iter()
            .find(|g| g.name == name)
            .map(|g| (g.passed, g.value))
    };
    let (occ_ok, occ) = value(GateName::Occlusion).unwrap_or((true, 0.0));
    let (blur_ok, blur) = value(GateName::Blur).unwrap_or((true, 0.0));
    cand.f_occ = occ;
    cand.blur = blur;
    if cand.rejected_reason.is_some() {
        return;
    }
    let (night_ok, elev) = value(GateName::Night).unwrap_or((true, 90.0));
    let (exposure_ok, mean_l) = value(GateName::Exposure).unwrap_or((true, 0.0));
    let clipped = value(GateName::Clipped).map(|(_, v)| v).unwrap_or(0.0);
    let (quality_ok, quality) = value(GateName::Quality).unwrap_or((true, 1.0));
    cand.rejected_reason = if !blur_ok {
        Some(format!("blur {blur:.0}"))
    } else if !night_ok {
        if elev < NIGHT_ELEV_DEG {
            Some("night".to_string())
        } else {
            Some(format!("dark L {mean_l:.0}"))
        }
    } else if !exposure_ok {
        Some(format!(
            "exposure L {mean_l:.0} clip {:.0} %",
            100.0 * clipped
        ))
    } else if !quality_ok {
        Some(format!("quality {quality:.2}"))
    } else if !occ_ok {
        Some(format!("occluded {:.0} %", 100.0 * occ))
    } else {
        None
    };
}

// --------------------------------------------------------------------------- selection

fn is_spherical(cameras: &BTreeMap<String, Camera>, pano_id: &str) -> bool {
    cameras
        .get(pano_id)
        .map(Camera::is_spherical)
        .unwrap_or(true)
}

/// At `perspective_penalty = 0` the preference is a drop rather than a weight:
/// on a wall where at least one panorama passes every gate the phone and dashcam
/// candidates go, so they can no longer take a second or third view slot either.
/// Above 0 the penalty is the score factor applied in [`select_views`] and
/// nothing is dropped here.
fn apply_perspective_preference(
    passing: Vec<usize>,
    cands: &mut [ViewCandidate],
    cameras: &BTreeMap<String, Camera>,
    params: &Params,
) -> Vec<usize> {
    if params.perspective_penalty > 0.0 {
        return passing;
    }
    if !passing
        .iter()
        .any(|i| is_spherical(cameras, &cands[*i].pano_id))
    {
        return passing;
    }
    let mut kept = Vec::new();
    for i in passing {
        if is_spherical(cameras, &cands[i].pano_id) {
            kept.push(i);
        } else {
            cands[i].score = 0.0;
            cands[i].rejected_reason = Some("perspective view, panorama available".into());
        }
    }
    kept
}

/// The final score over the candidates that passed every gate, and the up to
/// `max_views` best of them.
///
/// `score = g * quality * (1 - f_occ) * pose_factor * blur_factor`, with the
/// relative blur gate applied here because it needs every candidate of the wall
/// at once. At least two camera positions 3 m apart are kept when they exist
/// (the second pick is the best candidate 3 m or more from the first), and
/// `gates['positions']` records how many there were, so a lone position reads as
/// a single view downstream.
pub fn select_views(
    cands: &mut [ViewCandidate],
    cameras: &BTreeMap<String, Camera>,
    params: &Params,
    max_views: usize,
) -> Vec<ViewCandidate> {
    let mut passing: Vec<usize> = (0..cands.len())
        .filter(|i| cands[*i].rejected_reason.is_none())
        .collect();
    // This median is deliberately per wall and over both camera classes. Two
    // rounds of measurement on the Munich box (see MEASURED.md) say that reads
    // as a bug and is not one.
    //
    // `blur` is the Laplacian variance of a crop rendered at a fixed width, so
    // it tracks how many source pixels fall in one crop pixel, not how sharp the
    // photograph is: log(blur) against log(that ratio) correlates 0.887 over
    // both classes on one curve. Distance sets the ratio more than the camera
    // does (12.5x over the range within panoramas alone, against 5.8x between
    // the classes), so a per-class reference corrects the smaller confounder and
    // leaves the larger one.
    //
    // The per-wall denominator is what makes the gate scale free, and it carries
    // a safety property that nothing else here provides: the candidate AT the
    // median has rel = 1, so at least half of any wall's candidates always
    // survive and the gate can never empty a wall. Replacing it with one
    // reference per class per run cut 133 candidates instead of 41 and deleted
    // every view of seven walls, one of them a fully covered tier A facade. Both
    // variants were also worse to look at.
    //
    // Anything that replaces this must keep that floor, and should condition the
    // reference on the resampling ratio (class AND distance or crop scale)
    // rather than drop the normalisation. The absolute floor BLUR_MIN in
    // `image_gates` is the same argument applied where it would actually pay: it
    // is class blind and it rejects 25 % of Munich panorama candidates against
    // 3 % of phone ones, and 76 % against 4 % in New York, which is why New York
    // is a phone-only city.
    let mut blurs: Vec<f64> = passing
        .iter()
        .map(|i| cands[*i].blur)
        .filter(|b| *b > 0.0)
        .collect();
    let med = if blurs.is_empty() {
        0.0
    } else {
        imgops::median_in_place(&mut blurs)
    };
    for &i in &passing {
        let blur_factor = if med > 0.0 && cands[i].blur > 0.0 {
            let rel = cands[i].blur / med;
            cands[i].set_gate(GateName::BlurRel, rel >= BLUR_REL, rel);
            if rel < BLUR_REL {
                cands[i].rejected_reason = Some(format!(
                    "blur {:.0} < {BLUR_REL:.2} x median {med:.0}",
                    cands[i].blur
                ));
                continue;
            }
            rel.clamp(BLUR_REL, 1.0)
        } else {
            1.0
        };
        let quality = cands[i]
            .gate(GateName::Quality)
            .map(|g| g.value)
            .unwrap_or(1.0);
        let cam = cameras.get(&cands[i].pano_id);
        let pf = cam.map(|c| c.pose_factor).unwrap_or(1.0);
        let sph = cam.map(Camera::is_spherical).unwrap_or(true);
        // At penalty 0 the preference is a drop, not a weight, so the factor
        // stays 1 there: a wall whose only views are phone frames keeps them and
        // must still rank them by how good they are, not by pano id.
        let penalty = params.perspective_penalty;
        let cam_factor = if sph || penalty <= 0.0 { 1.0 } else { penalty };
        cands[i].score =
            cands[i].g_score * quality * (1.0 - cands[i].f_occ) * pf * blur_factor * cam_factor;
    }
    passing.retain(|i| cands[*i].rejected_reason.is_none());
    passing = apply_perspective_preference(passing, cands, cameras, params);
    passing.sort_by(|a, b| {
        let (ca, cb) = (&cands[*a], &cands[*b]);
        cb.score
            .partial_cmp(&ca.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| ca.pano_id.cmp(&cb.pano_id))
    });
    if passing.is_empty() {
        return Vec::new();
    }
    let position = |i: usize| -> [f64; 2] {
        let c = &cameras[&cands[i].pano_id];
        [c.centre[0], c.centre[1]]
    };
    let mut chosen = vec![passing[0]];
    let c0 = position(passing[0]);
    let mut rest: Vec<usize> = passing[1..].to_vec();
    let far = rest
        .iter()
        .position(|i| distance(position(*i), c0) >= MIN_POSITION_SEP_M);
    if let (Some(k), true) = (far, max_views >= 2) {
        chosen.push(rest[k]);
        rest.remove(k);
    }
    for i in rest {
        if chosen.len() >= max_views {
            break;
        }
        chosen.push(i);
    }
    let mut positions: Vec<[f64; 2]> = Vec::new();
    for &i in &chosen {
        let p = position(i);
        if positions
            .iter()
            .all(|q| distance(p, *q) >= MIN_POSITION_SEP_M)
        {
            positions.push(p);
        }
    }
    for &i in &chosen {
        cands[i].set_gate(
            GateName::Positions,
            positions.len() >= 2,
            positions.len() as f64,
        );
    }
    chosen.into_iter().map(|i| cands[i].clone()).collect()
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapillary::golden;

    fn unit_wall() -> Wall {
        use crate::mapillary::types::HeightSource;
        Wall {
            key: "w1_0".into(),
            building_key: "w1".into(),
            idx: 0,
            node_a: 1,
            node_b: 2,
            a: [0.0, 0.0],
            b: [10.0, 0.0],
            n: [0.0, -1.0],
            length: 10.0,
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

    /// A square building with the wall above as its north edge, so the wall's
    /// outward normal points away from it.
    fn square_building(key: &str, x0: f64, y0: f64, side: f64) -> Building {
        use crate::mapillary::types::{HeightSource, OsmKind};
        Building {
            key: key.into(),
            osm_id: 1,
            kind: OsmKind::Way,
            ring: vec![
                [x0, y0],
                [x0 + side, y0],
                [x0 + side, y0 + side],
                [x0, y0 + side],
            ],
            holes: vec![],
            node_ids: vec![1, 2, 3, 4],
            tags: Default::default(),
            height_osm: Some(12.0),
            height_source: HeightSource::Tag,
            min_height: 0.0,
            target: true,
            member_ways: vec![],
        }
    }

    #[test]
    fn the_geometric_score_falls_off_with_distance_and_incidence() {
        // 12 m head on and wide is the best a candidate can be.
        assert!((geometric_score(12.0, 0.0, 40.0) - 1.0).abs() < 1e-12);
        assert!(geometric_score(30.0, 0.0, 40.0) < geometric_score(12.0, 0.0, 40.0));
        assert!(geometric_score(12.0, 50.0, 40.0) < geometric_score(12.0, 0.0, 40.0));
        assert!(geometric_score(12.0, 0.0, 12.0) < geometric_score(12.0, 0.0, 40.0));
        // The distance factor has a floor, so a far view still scores.
        assert!(geometric_score(45.0, 0.0, 40.0) >= 0.2 - 1e-12);
    }

    #[test]
    fn a_footprint_between_the_camera_and_the_wall_blocks_the_ray() {
        let wall = unit_wall();
        let params = Params::default();
        // The wall's own building plus a block standing in front of it.
        let own = square_building("w1", 0.0, 0.0, 10.0);
        let blocker = square_building("w2", 2.0, -8.0, 6.0);
        let fps = Footprints::new(&[own, blocker]);
        let (frac, _) = line_of_sight(&wall, [5.0, -20.0], &fps, Some("w1"), &params);
        assert!(frac < 0.6, "a building in the way should block, got {frac}");
        // Off to the side the same camera sees the wall.
        let fps_clear = Footprints::new(&[square_building("w1", 0.0, 0.0, 10.0)]);
        let (clear, s_vis) = line_of_sight(&wall, [5.0, -20.0], &fps_clear, Some("w1"), &params);
        assert!(
            (clear - 1.0).abs() < 1e-12,
            "nothing in the way, got {clear}"
        );
        assert!(s_vis[0] < 1e-9 && (s_vis[1] - 10.0).abs() < 1e-9);
    }

    #[test]
    fn a_camera_just_inside_a_footprint_is_tolerated_and_deeper_in_is_not() {
        let wall = unit_wall();
        let params = Params::default();
        let own = square_building("w1", 0.0, 0.0, 10.0);
        // A courtyard building the camera stands in, 0.5 m from its edge.
        let around = square_building("w9", -30.0, -30.0, 25.0);
        let fps = Footprints::new(&[own, around]);
        let (near_edge, _) = line_of_sight(&wall, [-5.5, -5.5], &fps, Some("w1"), &params);
        assert!(near_edge > 0.0, "0.5 m inside should still see out");
        let (deep, _) = line_of_sight(&wall, [-15.0, -15.0], &fps, Some("w1"), &params);
        assert!(deep == 0.0, "indoors sees nothing, got {deep}");
    }

    #[test]
    fn the_solar_elevation_knows_noon_from_midnight() {
        // 2023-06-21 12:00 UTC over Munich: the sun is high.
        let noon = solar_elevation_deg(1_687_348_800_000, 11.58, 48.14);
        assert!(noon > 55.0 && noon < 70.0, "midsummer noon {noon}");
        // Twelve hours later it is well below the horizon.
        let midnight = solar_elevation_deg(1_687_392_000_000, 11.58, 48.14);
        assert!(midnight < -10.0, "midsummer midnight {midnight}");
    }

    #[test]
    fn the_relative_blur_gate_can_never_empty_a_wall() {
        let params = Params::default();
        let mut cameras = BTreeMap::new();
        let mut cands = Vec::new();
        for (i, blur) in [4000.0, 1000.0, 300.0, 60.0, 41.0].iter().enumerate() {
            let pid = format!("p{i}");
            cameras.insert(pid.clone(), test_camera(&pid, [10.0 * i as f64, 0.0, 3.0]));
            cands.push(ViewCandidate {
                wall_key: "w1_0".into(),
                pano_id: pid,
                dist_m: 12.0,
                incidence_deg: 0.0,
                angwidth_deg: 40.0,
                visible_frac: 1.0,
                s_vis: [0.0, 10.0],
                g_score: 1.0,
                gates: vec![Gate {
                    name: GateName::Quality,
                    passed: true,
                    value: 1.0,
                }],
                f_occ: 0.0,
                blur: *blur,
                score: 0.0,
                rejected_reason: None,
            });
        }
        let sel = select_views(&mut cands, &cameras, &params, 3);
        assert_eq!(sel.len(), 3);
        // The median candidate has rel = 1, so at least half survive whatever
        // the spread is: here 300, 1000 and 4000 pass and 41 goes.
        let kept: Vec<&str> = cands
            .iter()
            .filter(|c| c.rejected_reason.is_none())
            .map(|c| c.pano_id.as_str())
            .collect();
        assert_eq!(kept, vec!["p0", "p1", "p2"], "the median can never be cut");
        assert_eq!(sel[0].pano_id, "p0", "the sharpest view wins");
        // Every camera is 10 m from the next, so there are three positions.
        assert!(sel[0].gate(GateName::Positions).unwrap().passed);
    }

    fn test_camera(pid: &str, centre: [f64; 3]) -> Camera {
        use crate::mapillary::types::{CameraModel, GroundSource, PoseSource};
        Camera {
            pano_id: pid.into(),
            centre,
            axes: [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]],
            pose_source: PoseSource::Sfm,
            roll_deg: 0.0,
            pitch_deg: 0.0,
            ground_z: 0.0,
            cam_height_m: 2.5,
            ground_source: GroundSource::Cloud,
            cluster_id: None,
            shot_id: None,
            reg: None,
            compass_deg: 0.0,
            pose_factor: 1.0,
            width: 5760,
            height: 2880,
            camera_type: CameraModel::Spherical,
            camera_params: vec![],
        }
    }

    // ----------------------------------------------------------------- golden

    /// A reason with its numbers taken out, so two that differ only in a
    /// rounded value compare equal.
    fn without_numbers(reason: &str) -> String {
        reason.chars().filter(|c| !c.is_ascii_digit()).collect()
    }

    /// The reasons an image gate produces, which the pose-only pass cannot
    /// know about.
    fn is_image_reason(reason: &str) -> bool {
        reason.starts_with("blur")
            || reason.starts_with("night")
            || reason.starts_with("dark")
            || reason.starts_with("exposure")
            || reason.starts_with("quality")
            || reason.starts_with("occluded")
            || reason.starts_with("no_image")
            || reason.starts_with("perspective view")
    }

    /// The line of sight gate and the visible span, run from the registered
    /// camera centres the way `run_align` runs them.
    #[test]
    fn golden_line_of_sight_matches_the_python_run() {
        if golden::absent() {
            return;
        }

        let file: golden::GoldenAlignFile = golden::load("align/candidates.json");
        let cameras = golden::registered_cameras();
        let walls = golden::walls_by_key();
        let buildings = golden::buildings();
        let fps = Footprints::new(&buildings);
        let params = Params::default();

        let mut checked = 0usize;
        let mut with_los = 0usize;
        let mut worst_frac = 0.0f64;
        let mut worst_s = 0.0f64;
        for w in &file.walls {
            let wall = &walls[&w.wall_key];
            let got = geometric_candidates(wall, &cameras, &params, Some(&fps));
            assert_eq!(
                got.len(),
                w.candidates.len(),
                "{}: candidate count",
                w.wall_key
            );
            for (g, c) in got.iter().zip(&w.candidates) {
                assert_eq!(g.pano_id, c.pano_id, "{}: candidate order", w.wall_key);
                match c.rejected_reason.as_deref() {
                    // The numbers in a reason are compared as gate values, not
                    // as text: an incidence of 85.5 degrees prints as 85 or 86
                    // depending on the fixture's own rounding of the camera
                    // centre, and which gate rejected the candidate is the part
                    // that matters here.
                    Some(reason) if !is_image_reason(reason) => assert_eq!(
                        g.rejected_reason.as_deref().map(without_numbers),
                        Some(without_numbers(reason)),
                        "{} {}: reason {:?} vs {reason}",
                        w.wall_key,
                        c.pano_id,
                        g.rejected_reason
                    ),
                    _ => assert!(
                        g.rejected_reason.is_none(),
                        "{} {}: rejected as {:?}, the run got as far as the image gates",
                        w.wall_key,
                        c.pano_id,
                        g.rejected_reason
                    ),
                }
                if let (Some(frac), Some(s_vis)) = (c.visible_frac, c.s_vis) {
                    worst_frac = worst_frac.max((g.visible_frac - frac).abs());
                    worst_s = worst_s
                        .max((g.s_vis[0] - s_vis[0]).abs())
                        .max((g.s_vis[1] - s_vis[1]).abs());
                    with_los += 1;
                }
                if let Some((passed, value)) = file.gate(c, "los") {
                    let mine = g.gate(GateName::Los).expect("the LOS gate ran");
                    assert_eq!(mine.passed, passed, "{} {}: LOS", w.wall_key, c.pano_id);
                    worst_frac = worst_frac.max((mine.value - value).abs());
                }
                checked += 1;
            }
        }
        assert!(worst_frac < 1e-6, "worst visible share {worst_frac}");
        // s_vis is a wall coordinate, so it carries the wall's own 1e-6 rounding.
        assert!(worst_s < 1e-5, "worst visible span {worst_s} m");
        println!(
            "{checked} candidates over {} walls, {with_los} through the line of sight test, \
             every reason identical, worst visible share {worst_frac:.2e}, worst span \
             {worst_s:.2e} m",
            file.walls.len()
        );
    }

    /// The gates on the crop **this port renders**, over a spread of candidates.
    ///
    /// This is the seam the rest of the align fixtures skip. `candidates.json`
    /// checks the gate values the Python recorded against gate values recomputed
    /// from the Python's own stored inputs, so it passes on all 8419 candidates
    /// however the crop is rendered. In the running pipeline the crop and its
    /// occlusion planes are the port's own, and everything the gates say is
    /// decided there: the occluded share is a count of bits on that plane, and
    /// blur is a variance over the texels the plane leaves clear.
    ///
    /// So every record here is rendered from the thumbnail, the camera, the
    /// candidate's own plane fit, its foot z, its visible span and its depth map,
    /// and then compared bit by bit and gate by gate. The sample spans both
    /// camera classes, both border rules, candidates with and without a depth
    /// map, spans the footprint columns narrow and spans they do not, and an
    /// occluded share from under one per cent to over forty.
    #[test]
    fn golden_preview_crops_and_image_gates_match_the_python_run() {
        if golden::pixels_absent() {
            return;
        }

        let crops = golden::crops();
        let params = Params::default();
        assert!(
            crops.len() >= 20,
            "the crop fixture is {} records",
            crops.len()
        );
        let spherical = crops
            .iter()
            .filter(|c| c.camera.camera_type == "spherical")
            .count();
        assert!(spherical >= 5, "{spherical} spherical crops");
        assert!(
            crops.len() - spherical >= 5,
            "{} perspective crops",
            crops.len() - spherical
        );
        let occs: Vec<f64> = crops.iter().map(|c| c.gates["occlusion"].1).collect();
        assert!(
            occs.iter().any(|v| *v < 0.02) && occs.iter().any(|v| *v > 0.3),
            "the sample must span clear and occluded walls"
        );
        // The cloud plane is the one this port measured differently in a live
        // run, so the sample has to hold crops the depth map occludes heavily
        // and crops it does not touch at all.
        let cloud_share =
            |c: &golden::GoldenCrop| c.bits.cloud as f64 / f64::from(c.rows * c.width).max(1.0);
        assert!(
            crops.iter().any(|c| cloud_share(c) < 0.001)
                && crops.iter().any(|c| cloud_share(c) > 0.25),
            "the sample must span crops the cloud barely touches and crops it covers"
        );
        assert!(
            crops.iter().any(|c| c.bits.footprint > 0),
            "the sample must hold a wall the visible span narrows"
        );

        let mut worst_blur_rel = 0.0f64;
        let mut worst_gate_rel = 0.0f64;
        let mut worst_bit = 0usize;
        let mut worst_seg_share = 0.0f64;
        let mut worst_veg_iou = 1.0f64;
        let mut with_pixels = 0usize;
        for crop in &crops {
            let cam = crop.camera();
            let wall = crop.wall();
            let depth = crop.depth();
            let got = preview_crop(
                &wall,
                &cam,
                crop.z_base,
                &crop.source_image(),
                &params,
                &PreviewOpts {
                    depth: depth.as_ref(),
                    s_vis: Some(crop.s_vis),
                    ..PreviewOpts::default()
                },
            );
            assert_eq!(
                (got.rgb.width(), got.rgb.height()),
                (crop.width, crop.rows),
                "{}: crop size",
                crop.view_key
            );
            // The metre quantities carry the fixture's 1e-6 rounding of the
            // wall and the camera; the two pixel ones carry it multiplied by
            // the crop's own pixels per metre.
            for (name, mine, want, tol) in [
                ("ppm", got.ppm, crop.meta.ppm, 1e-5),
                ("ppm_v", got.ppm_v, crop.meta.ppm_v, 1e-5),
                ("s0", got.s0, crop.meta.s0, 1e-5),
                ("h_top", got.h_top, crop.meta.h_top, 1e-5),
                ("x_foot", got.x_foot, crop.meta.x_foot, 1e-3),
                ("y_cam", got.y_cam, crop.meta.y_cam, 1e-3),
            ] {
                assert!(
                    (mine - want).abs() < tol * want.abs().max(1.0),
                    "{}: {name} {mine} vs {want}",
                    crop.view_key
                );
            }

            // Every occlusion plane of the port's own render, counted. A bit
            // the port sets on a different number of texels than the run did is
            // what an occluded share that drifts is made of, and naming the bit
            // is the difference between a number that moved and a cause.
            let count = |bit: u8| got.occl.iter().filter(|o| **o & bit != 0).count();
            for (name, mine, want) in [
                ("nadir", count(OCC_NADIR), crop.bits.nadir),
                ("zenith", count(OCC_ZENITH), crop.bits.zenith),
                ("outside", count(OCC_OUTSIDE), crop.bits.outside),
                ("footprint", count(OCC_FOOTPRINT), crop.bits.footprint),
                ("cloud", count(OCC_CLOUD), crop.bits.cloud),
                ("seg", count(OCC_SEG), crop.bits.seg),
            ] {
                let n = (crop.width * crop.rows) as usize;
                // The five geometric planes are decided by a projection and a
                // depth lookup, so they are held to a rounding. The vegetation
                // mask is a threshold on the pixels, and this resampler differs
                // from OpenCV's by a fraction of a grey level, so its boundary
                // moves: it is held to a share of itself instead, and the mask
                // proper is compared texel by texel on the records that carry
                // their pixels. Over the fixture's 23 masks the share is under
                // 3.6 per cent on 22 and 8.3 per cent on `w97508969_0` from
                // `1702164253740730`; the floor is for the small masks, one of
                // which the port does not find at all.
                let slack = if name == "seg" {
                    (want / 10).max(n / 100)
                } else {
                    (n / 2000).max(8)
                };
                assert!(
                    mine.abs_diff(want) <= slack,
                    "{}: OCC_{} on {mine} texels, the run had {want} of {n}",
                    crop.view_key,
                    name.to_uppercase()
                );
                if name == "seg" {
                    worst_seg_share =
                        worst_seg_share.max(mine.abs_diff(want) as f64 / (want.max(1) as f64));
                } else {
                    worst_bit = worst_bit.max(mine.abs_diff(want));
                }
            }

            // The gates themselves, on the port's own crop and occlusion plane.
            let meta = meta_for(crop);
            for g in image_gates(&got.rgb, &got.occl, &meta, None, None) {
                let (want_pass, want_value) = crop.gates[g.name.as_str()];
                let rel = (g.value - want_value).abs() / want_value.abs().max(1.0);
                // Blur is a variance, so the sub-grey-level difference between
                // this resampler and OpenCV's is worth more of it the flatter
                // the crop is: 2 per cent of a sharp crop's 900 is 18 levels of
                // signal, 2 per cent of a smeared crop's 45 is one, and the
                // vegetation mask deciding which texels count moves it again.
                // The bound is whichever of the two is looser; measured over
                // this sample the worst is 2.27 absolute and 2.7 per cent, both
                // on crops the mask covers a quarter to a half of. The verdict
                // is asserted on its own, so a value that wanders across the
                // gate is caught whatever the tolerance says.
                if g.name == GateName::Blur {
                    worst_blur_rel = worst_blur_rel.max(rel);
                    assert!(
                        rel < 0.02 || (g.value - want_value).abs() < 3.5,
                        "{}: blur {} vs {want_value}",
                        crop.view_key,
                        g.value
                    );
                    assert_eq!(
                        g.passed, want_pass,
                        "{}: gate {} verdict",
                        crop.view_key, g.name
                    );
                    continue;
                }
                worst_gate_rel = worst_gate_rel.max(rel);
                assert!(
                    rel < 0.02,
                    "{}: gate {} {} vs {want_value}",
                    crop.view_key,
                    g.name,
                    g.value
                );
                assert_eq!(
                    g.passed, want_pass,
                    "{}: gate {} verdict",
                    crop.view_key, g.name
                );
            }

            // The few records that carry their pixels: the resampling, the
            // vegetation mask and the occlusion plane texel by texel, and the
            // gates on the run's own crop, which have to be exact.
            let (Some(want_rgb), Some(want_occl), Some(want_veg)) =
                (crop.rgb(), crop.occl(), crop.veg())
            else {
                continue;
            };
            with_pixels += 1;
            let n = (crop.width * crop.rows) as usize;
            let mut sum = 0.0f64;
            let mut worst = 0u8;
            for (a, b) in got.rgb.pixels().zip(want_rgb.pixels()) {
                let mut d = 0u8;
                for i in 0..3 {
                    d = d.max(a.0[i].abs_diff(b.0[i]));
                }
                sum += f64::from(d);
                worst = worst.max(d);
            }
            let mean = sum / (n as f64);
            assert!(
                mean < 1.0,
                "{}: mean pixel difference {mean} (worst {worst})",
                crop.view_key
            );

            // The mask twice: on the run's own pixels, which is the detector
            // alone and has to agree, and on the pixels this port rendered,
            // which is what the OCC_SEG bit of a live run is really made of and
            // carries the resampler's fraction of a grey level as well.
            let iou_of = |img: &image::RgbImage, ppm: f64| {
                let mine = crate::mapillary::rectify::vegetation_mask(img, ppm);
                let both = (0..n).filter(|k| mine[*k] && want_veg[*k]).count();
                let either = (0..n).filter(|k| mine[*k] || want_veg[*k]).count();
                if either == 0 {
                    1.0
                } else {
                    both as f64 / either as f64
                }
            };
            let iou_same_pixels = iou_of(&want_rgb, crop.meta.ppm);
            let iou = iou_of(&got.rgb, got.ppm);
            assert!(
                iou_same_pixels > 0.98,
                "{}: vegetation mask IoU {iou_same_pixels} on the run's own pixels",
                crop.view_key
            );
            assert!(
                iou > 0.965,
                "{}: vegetation mask IoU {iou} on this port's own crop ({:?} pixels in the run)",
                crop.view_key,
                crop.veg_pixels
            );
            worst_veg_iou = worst_veg_iou.min(iou);

            let hard = !OCC_SEG;
            let differing = (0..n)
                .filter(|k| (got.occl[*k] & hard) != (want_occl[*k] & hard))
                .count();
            assert_eq!(
                differing, 0,
                "{}: {differing} occlusion bytes differ outside the vegetation bit",
                crop.view_key
            );

            for g in image_gates(&want_rgb, &want_occl, &meta, None, None) {
                let (want_pass, want_value) = crop.gates[g.name.as_str()];
                let rel = (g.value - want_value).abs() / want_value.abs().max(1.0);
                assert!(
                    rel < 1e-6,
                    "{} on the run's own crop: gate {} {} vs {want_value}",
                    crop.view_key,
                    g.name,
                    g.value
                );
                assert_eq!(
                    g.passed, want_pass,
                    "{}: gate {} verdict",
                    crop.view_key, g.name
                );
            }
        }
        assert!(with_pixels >= 3, "{with_pixels} crops carry their pixels");
        println!(
            "{} crops over {} panos ({spherical} spherical), occluded share {:.3} to {:.3}, \
             {with_pixels} with pixels: worst occlusion bit off by {worst_bit} texels, worst \
             vegetation mask off by {:.1} % of itself, worst blur {:.2} %, worst other gate \
             {:.2} %, worst vegetation IoU {worst_veg_iou:.4}",
            crops.len(),
            crops
                .iter()
                .map(|c| c.pano_id.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            occs.iter().copied().fold(f64::INFINITY, f64::min),
            occs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            100.0 * worst_seg_share,
            100.0 * worst_blur_rel,
            100.0 * worst_gate_rel
        );
    }

    /// The image record the gates read: the crop fixture keeps only what they
    /// use, which is the capture time, the position and the quality score.
    fn meta_for(crop: &golden::GoldenCrop) -> PanoMeta {
        let panos: Vec<serde_json::Value> = golden::panos().clone();
        let raw = panos
            .iter()
            .find(|p| p.get("id").and_then(|v| v.as_str()) == Some(crop.pano_id.as_str()))
            .unwrap_or_else(|| panic!("{} is not in panos.json", crop.pano_id));
        PanoMeta::from_graph(raw).expect("the record parses")
    }

    /// The view list of every wall, from the gate values the run measured.
    #[test]
    fn golden_selected_views_match_the_python_run() {
        if golden::absent() {
            return;
        }

        let file: golden::GoldenAlignFile = golden::load("align/candidates.json");
        let cameras = golden::registered_cameras();
        let params = Params::default();
        let mut walls_with_views = 0usize;
        let mut worst_score = 0.0f64;
        let mut worst_rel = 0.0f64;
        let mut single = 0usize;
        for w in &file.walls {
            let mut cands: Vec<ViewCandidate> = w
                .candidates
                .iter()
                .map(|c| {
                    let mut gates = Vec::new();
                    if let Some((passed, value)) = file.gate(c, "quality") {
                        gates.push(Gate {
                            name: GateName::Quality,
                            passed,
                            value,
                        });
                    }
                    ViewCandidate {
                        wall_key: w.wall_key.clone(),
                        pano_id: c.pano_id.clone(),
                        dist_m: 0.0,
                        incidence_deg: 0.0,
                        angwidth_deg: 0.0,
                        visible_frac: c.visible_frac.unwrap_or(0.0),
                        s_vis: c.s_vis.unwrap_or([0.0, 0.0]),
                        g_score: c.g_score.unwrap_or(0.0),
                        gates,
                        f_occ: c.f_occ.unwrap_or(0.0),
                        blur: c.blur.unwrap_or(0.0),
                        score: 0.0,
                        // The relative blur gate is applied by select_views
                        // itself, so a candidate the run rejected there goes
                        // back in as a candidate; anything else stays rejected.
                        rejected_reason: c
                            .rejected_reason
                            .clone()
                            .filter(|r| !(r.starts_with("blur ") && r.contains("median"))),
                    }
                })
                .collect();
            let sel = select_views(&mut cands, &cameras, &params, 3);
            let got: Vec<&str> = sel.iter().map(|c| c.pano_id.as_str()).collect();
            let want: Vec<&str> = w.selected.iter().map(String::as_str).collect();
            assert_eq!(got, want, "{}: view list", w.wall_key);
            walls_with_views += usize::from(!sel.is_empty());
            let single_view = !sel.is_empty()
                && !sel[0]
                    .gate(GateName::Positions)
                    .map(|g| g.passed)
                    .unwrap_or(true);
            assert_eq!(single_view, w.single_view, "{}: single view", w.wall_key);
            single += usize::from(single_view);
            // The scores and the relative blur gate behind the ordering.
            for (i, c) in cands.iter().enumerate() {
                let want = &w.candidates[i];
                if let Some(score) = want.score {
                    if score > 0.0 || c.rejected_reason.is_none() {
                        worst_score = worst_score.max((c.score - score).abs());
                    }
                }
                if let Some((passed, value)) = file.gate(want, "blur_rel") {
                    let mine = c
                        .gate(GateName::BlurRel)
                        .unwrap_or_else(|| panic!("{} {}: no blur_rel", w.wall_key, c.pano_id));
                    assert_eq!(
                        mine.passed, passed,
                        "{} {}: blur_rel",
                        w.wall_key, c.pano_id
                    );
                    worst_rel = worst_rel.max((mine.value - value).abs());
                }
                if let Some(reason) = &want.rejected_reason {
                    if reason.starts_with("blur ") && reason.contains("median") {
                        assert_eq!(
                            c.rejected_reason.as_deref(),
                            Some(reason.as_str()),
                            "{} {}: relative blur reason",
                            w.wall_key,
                            c.pano_id
                        );
                    }
                }
            }
        }
        assert!(worst_score < 1e-6, "worst score difference {worst_score}");
        assert!(worst_rel < 1e-5, "worst relative blur {worst_rel}");
        assert_eq!(walls_with_views, 111, "walls with views on this box");
        println!(
            "{} walls, {walls_with_views} with views ({single} single position), every view \
             list identical in order, worst score {worst_score:.2e}, worst relative blur \
             {worst_rel:.2e}",
            file.walls.len()
        );
    }

    /// Every gate value of every candidate the Python's geometry stage
    /// recorded, on the walls the fixture kept.
    #[test]
    fn golden_geometric_gates_match_the_python_run() {
        if golden::absent() {
            return;
        }

        let file: golden::GoldenCandidateFile = golden::load("candidates.json");
        let cameras = golden::geometry_cameras();
        let walls = golden::walls_by_key();
        let params = Params::default();
        assert_eq!(file.candidates.len(), 8419);

        let mut wanted: BTreeMap<&str, Vec<&golden::GoldenCandidate>> = BTreeMap::new();
        for c in &file.candidates {
            wanted.entry(c.wall_key.as_str()).or_default().push(c);
        }
        assert_eq!(wanted.len(), 111, "walls with candidates");

        let mut checked = 0usize;
        // The two angles are the one ill-conditioned pair here: both come out of
        // an `acos` whose argument sits near +-1 for a camera that stands a
        // decimetre from the wall's line, so the fixture's own rounding of the
        // camera centre to 1e-4 m moves them by a few thousandths of a degree.
        // Recomputing them in Python from the rounded centre reproduces the
        // Rust value to six decimals on every one of the walls below, which is
        // what says the port is faithful and the fixture is coarse.
        let mut worst = 0.0f64;
        let mut worst_angle = 0.0f64;
        let mut reasons = 0usize;
        for (wall_key, want) in &wanted {
            let wall = &walls[*wall_key];
            let got = geometric_candidates(wall, &cameras, &params, None);
            assert_eq!(got.len(), want.len(), "{wall_key}: candidate count");
            for (g, w) in got.iter().zip(want) {
                assert_eq!(g.pano_id, w.pano_id, "{wall_key}: candidate order");
                worst = worst
                    .max((g.dist_m - w.dist_m).abs())
                    .max((g.g_score - w.g_score).abs());
                worst_angle = worst_angle
                    .max((g.incidence_deg - w.incidence_deg).abs())
                    .max((g.angwidth_deg - w.angwidth_deg).abs());
                for name in &file.gate_names {
                    let gate_name = GateName::from_str_or_default(name);
                    let mine = g.gate(gate_name);
                    match file.gate(w, name) {
                        None => assert!(
                            mine.is_none(),
                            "{wall_key} {}: gate {name} should not apply",
                            g.pano_id
                        ),
                        Some((passed, value)) => {
                            let mine = mine.unwrap_or_else(|| {
                                panic!("{wall_key} {}: gate {name} missing", g.pano_id)
                            });
                            assert_eq!(
                                mine.passed, passed,
                                "{wall_key} {}: gate {name} verdict",
                                g.pano_id
                            );
                            let d = (mine.value - value).abs();
                            if matches!(gate_name, GateName::Incidence | GateName::Angwidth) {
                                worst_angle = worst_angle.max(d);
                            } else {
                                worst = worst.max(d);
                            }
                        }
                    }
                }
                assert_eq!(
                    g.rejected_reason, w.rejected_reason,
                    "{wall_key} {}: reason",
                    g.pano_id
                );
                if g.rejected_reason.is_some() {
                    reasons += 1;
                }
                checked += 1;
            }
        }
        // The fixture rounds its gate values to 1e-4 and the camera centres it
        // recomputes them from to 1e-4 m as well, so a distance carries both
        // roundings and nothing here can be tighter than about 2e-4.
        assert!(worst < 3e-4, "worst gate value difference {worst}");
        assert!(
            worst_angle < 0.006,
            "worst angle difference {worst_angle} deg"
        );
        assert_eq!(checked, 8419);
        println!(
            "{checked} candidates over {} walls, {reasons} rejected, every gate verdict and \
             reason identical, worst gate value difference {worst:.2e}, worst angle \
             {worst_angle:.2e} deg",
            wanted.len()
        );
    }
}
