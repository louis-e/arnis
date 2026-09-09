//! OpenSfM cluster access. Port of `tools/facade_lab/sfm.py`.
//!
//! One reconstruction, brought into the run frame and asked the four questions
//! the rest of the pipeline has for it: where is the ground under this camera,
//! how high is the camera above it, where is the foot of this wall, and what is
//! in front of this camera at every direction.
//!
//! Conventions this module fixes, all of them expensive to re-derive:
//!
//! * **Points and shot centres go through lon/lat, not through the sphere.**
//!   A cluster's own frame is OpenSfM topocentric about its `reference_lla`,
//!   which is ellipsoidal, so a point goes cluster-topocentric to lon/lat
//!   through WGS84 ([`super::geometry::topocentric_to_lla`]) and only then into
//!   the run frame's equirectangular formula. OSM nodes reach the same frame
//!   the same way, so the sphere approximation is applied to everything alike
//!   and cancels in relative geometry. Using the sphere for the cluster too
//!   costs 0.25 to 0.8 m. On the Munich cache the mapped shot centres then
//!   agree with `computed_geometry` to under a micrometre.
//! * **The z datum is per cluster and drifts inside one.** Cluster z and
//!   [`Camera::ground_z`] are comparable only within a single cluster. Anything
//!   compared across views goes through `cam_height_m` and a per view `z_base`
//!   from [`wall_foot_z`].
//! * **Shot to image is `round(capture_time * 1000) == captured_at`**, which is
//!   exact and unique on the cache: no image of the Munich box has two shots at
//!   the same millisecond. The fallback is the same sequence key and the
//!   nearest capture time within `shot_time_tol_s`, if that nearest is unique.
//!   Unmapped shots stay useful as point and depth sources.
//! * **A mapped camera always takes the shot's centre, and takes the shot's
//!   rotation only if it had none.** `computed_geometry` is already the SfM
//!   position and agrees with the shot centre to under a metre, so replacing
//!   the centre is a refinement; what is left is OSM against imagery drift,
//!   which is `register.rs`'s problem. The rotation is not replaced when the
//!   pose is already `sfm`, because then it is the same rotation by another
//!   route. On the Munich box all 1076 cameras are already `sfm`, so that
//!   branch never fires there.
//!
//! Three places where a faithful port had to differ from the Python:
//!
//! * **Point order.** `serde_json` sorts object keys, so a cluster's points
//!   arrive in numeric id order rather than file order (see
//!   `fetch::RawCluster::parse`), and [`Cluster::near_idx`] hands them back
//!   sorted rather than in the kd-tree order `scipy` happens to walk. Nothing
//!   in this module depends on either: the percentiles sort, and the depth
//!   splat writes the same depth into a cell whichever of two equal points
//!   wrote it last. `plane.rs` does depend on it, and its header says what that
//!   costs.
//! * **There is no `f16` in std**, so [`DepthMap`] holds `f32` values that have
//!   been rounded to float16. That rounding is not decoration: `depth_splat`
//!   returns `float16` and every consumer of a depth map in the Python pipeline
//!   sees the rounded value, so dropping it would put the Rust and the Python
//!   occlusion decisions on different numbers.
//! * **`ground_source` never holds the Python's `"pending"`.** That is a marker
//!   between the two loops of [`camera_heights`] and never survives the second
//!   one, so here it is a local flag rather than a wire value.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use fnv::FnvHashMap;

use super::fetch::RawCluster;
use super::geometry::topocentric_to_run;
use super::imgops::percentile_in_place;
use super::pose::{self, Projector};
use super::types::{Camera, Frame, GroundSource, PanoMeta, Params, PoseSource, Wall};

/// Bucket size of the plan index, in metres. The queries are 8 m (ground),
/// about 20 m (a wall neighbourhood) and 60 m (a depth map), so this trades a
/// few hundred bucket lookups on the widest query against short buckets on the
/// narrowest one.
const INDEX_CELL_M: f64 = 5.0;

/// The wall foot came from cloud points in front of the wall.
pub const Z_BASE_FOOT: &str = "foot";
/// The wall foot fell back to the camera's own ground level.
pub const Z_BASE_PANO: &str = "pano";

// --------------------------------------------------------------------------- cluster

/// One shot of a reconstruction, in the run frame.
#[derive(Clone, Debug)]
pub struct Shot {
    pub shot_id: String,
    pub centre: [f64; 3],
    /// Rows are the camera right, down and forward directions in ENU.
    pub axes: [[f64; 3]; 3],
    pub capture_time: f64,
    /// The reconstruction's sequence key, which equals the Graph `sequence`.
    pub skey: String,
    pub compass: f64,
    pub rotation: [f64; 3],
    pub camera: String,
}

/// One OpenSfM reconstruction in the run frame, unregistered.
#[derive(Clone, Debug)]
pub struct Cluster {
    pub cluster_id: String,
    /// `(lon, lat, alt)` of the reconstruction's own origin.
    pub ref_lla: (f64, f64, f64),
    /// Run frame xy of `ref_lla`.
    pub offset: [f64; 2],
    pub points: Vec<[f64; 3]>,
    pub colors: Vec<[u8; 3]>,
    pub shots: Vec<Shot>,
    /// The frame the points were brought into, kept so a camera can be rebuilt
    /// from a shot without the caller threading it through.
    pub frame: Frame,
    /// Set by [`metric_check`]; true until it has been checked.
    pub scale_ok: bool,
    pub metric_ratio: f64,
    index: PlanIndex,
}

impl Cluster {
    /// One parsed reconstruction moved into the run frame.
    pub fn from_raw(raw: &RawCluster, frame: &Frame) -> Self {
        let points: Vec<[f64; 3]> = raw
            .points
            .iter()
            .map(|p| topocentric_to_run(*p, raw.ref_lla, frame))
            .collect();
        let shots = raw
            .shots
            .iter()
            .map(|s| {
                let axes = pose::axes_from_rotation(s.rotation);
                // The shot stores the world to camera transform, so its centre
                // is -R^T t: the rows of `axes` are the camera axes, and the
                // transpose times t sums the columns.
                let t = s.translation;
                let c_topo = [
                    -(axes[0][0] * t[0] + axes[1][0] * t[1] + axes[2][0] * t[2]),
                    -(axes[0][1] * t[0] + axes[1][1] * t[1] + axes[2][1] * t[2]),
                    -(axes[0][2] * t[0] + axes[1][2] * t[1] + axes[2][2] * t[2]),
                ];
                Shot {
                    shot_id: s.shot_id.clone(),
                    centre: topocentric_to_run(c_topo, raw.ref_lla, frame),
                    axes,
                    capture_time: s.capture_time,
                    skey: s.skey.clone(),
                    compass: s.compass,
                    rotation: s.rotation,
                    camera: s.camera.clone(),
                }
            })
            .collect();
        let index = PlanIndex::new(&points, INDEX_CELL_M);
        Self {
            cluster_id: raw.cluster_id.clone(),
            ref_lla: raw.ref_lla,
            offset: frame.to_enu(raw.ref_lla.0, raw.ref_lla.1),
            points,
            colors: raw.colors.clone(),
            shots,
            frame: *frame,
            scale_ok: true,
            metric_ratio: f64::NAN,
            index,
        }
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn shot(&self, shot_id: &str) -> Option<&Shot> {
        self.shots.iter().find(|s| s.shot_id == shot_id)
    }

    /// Indices of the points within `radius_m` of `xy` in plan, ascending.
    pub fn near_idx(&self, xy: [f64; 2], radius_m: f64) -> Vec<usize> {
        self.index.query(&self.points, xy, radius_m)
    }

    /// The points within `radius_m` of `xy` in plan.
    pub fn near(&self, xy: [f64; 2], radius_m: f64) -> Vec<[f64; 3]> {
        self.near_idx(xy, radius_m)
            .into_iter()
            .map(|i| self.points[i])
            .collect()
    }
}

/// Parses one reconstruction document into the run frame.
pub fn load_cluster(
    json: &serde_json::Value,
    cluster_id: &str,
    frame: &Frame,
) -> Result<Cluster, String> {
    RawCluster::parse(json, cluster_id).map(|raw| Cluster::from_raw(&raw, frame))
}

/// A uniform bucket grid over the plan positions of a cloud.
///
/// A k-d tree would answer the same queries; a grid is a page of code and the
/// clouds are tens of thousands of points spread over a street, which is
/// exactly the shape a grid likes.
#[derive(Clone, Debug)]
struct PlanIndex {
    cell: f64,
    buckets: FnvHashMap<(i32, i32), Vec<u32>>,
}

impl PlanIndex {
    fn new(points: &[[f64; 3]], cell: f64) -> Self {
        let mut buckets: FnvHashMap<(i32, i32), Vec<u32>> = FnvHashMap::default();
        for (i, p) in points.iter().enumerate() {
            buckets
                .entry(Self::key(p[0], p[1], cell))
                .or_default()
                .push(i as u32);
        }
        Self { cell, buckets }
    }

    fn key(x: f64, y: f64, cell: f64) -> (i32, i32) {
        ((x / cell).floor() as i32, (y / cell).floor() as i32)
    }

    /// scipy's `query_ball_point`: everything at distance `<= radius`, sorted.
    fn query(&self, points: &[[f64; 3]], xy: [f64; 2], radius: f64) -> Vec<usize> {
        let mut out = Vec::new();
        if radius <= 0.0 || self.buckets.is_empty() || !xy[0].is_finite() || !xy[1].is_finite() {
            return out;
        }
        let (cx0, cy0) = Self::key(xy[0] - radius, xy[1] - radius, self.cell);
        let (cx1, cy1) = Self::key(xy[0] + radius, xy[1] + radius, self.cell);
        let r2 = radius * radius;
        for cx in cx0..=cx1 {
            for cy in cy0..=cy1 {
                let Some(bucket) = self.buckets.get(&(cx, cy)) else {
                    continue;
                };
                for &i in bucket {
                    let p = points[i as usize];
                    let (dx, dy) = (p[0] - xy[0], p[1] - xy[1]);
                    if dx * dx + dy * dy <= r2 {
                        out.push(i as usize);
                    }
                }
            }
        }
        out.sort_unstable();
        out
    }
}

// --------------------------------------------------------------------------- shot mapping

/// Pano id to shot id for the images of one cluster.
///
/// Exact first: the rounded capture time, when it picks out one shot that is
/// still free. Then the fallback of CRITIQUE B10, the same sequence key and the
/// nearest capture time within `shot_time_tol_s`, if that nearest is unique.
pub fn map_shots(
    cluster: &Cluster,
    metas: &[&PanoMeta],
    params: &Params,
) -> HashMap<String, String> {
    let mut by_ms: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, s) in cluster.shots.iter().enumerate() {
        by_ms
            .entry((s.capture_time * 1000.0).round() as i64)
            .or_default()
            .push(i);
    }
    let mut out: HashMap<String, String> = HashMap::new();
    let mut used: HashSet<usize> = HashSet::new();
    for m in metas {
        let free: Vec<usize> = by_ms
            .get(&m.captured_at)
            .map(|v| v.iter().copied().filter(|i| !used.contains(i)).collect())
            .unwrap_or_default();
        if free.len() == 1 {
            out.insert(m.id.clone(), cluster.shots[free[0]].shot_id.clone());
            used.insert(free[0]);
        }
    }
    for m in metas {
        if out.contains_key(&m.id) {
            continue;
        }
        let t = m.captured_at as f64 / 1000.0;
        let mut cands: Vec<(f64, usize)> = cluster
            .shots
            .iter()
            .enumerate()
            .filter(|(i, s)| {
                s.skey == m.sequence
                    && !used.contains(i)
                    && (s.capture_time - t).abs() <= params.shot_time_tol_s
            })
            .map(|(i, s)| ((s.capture_time - t).abs(), i))
            .collect();
        if cands.is_empty() {
            continue;
        }
        // Python sorts (difference, shot id) tuples, so a tie breaks on the id.
        cands.sort_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then_with(|| cluster.shots[a.1].shot_id.cmp(&cluster.shots[b.1].shot_id))
        });
        if cands.len() == 1 || cands[1].0 - cands[0].0 > 1e-3 {
            out.insert(m.id.clone(), cluster.shots[cands[0].1].shot_id.clone());
            used.insert(cands[0].1);
        }
    }
    out
}

/// Replaces the centre of every mapped camera by its cluster shot's, and the
/// axes too when the Graph rotation was missing (CRITIQUE B11).
pub fn apply_shot_poses(
    cameras: &mut HashMap<String, Camera>,
    cluster: &Cluster,
    shot_map: &HashMap<String, String>,
    metas: &HashMap<&str, &PanoMeta>,
    by_seq: &HashMap<&str, Vec<&PanoMeta>>,
    params: &Params,
) {
    for (pano_id, shot_id) in shot_map {
        let (Some(cam), Some(shot)) = (cameras.get_mut(pano_id), cluster.shot(shot_id)) else {
            continue;
        };
        cam.shot_id = Some(shot_id.clone());
        cam.cluster_id = Some(cluster.cluster_id.clone());
        cam.centre = shot.centre;
        cam.ground_z = cam.centre[2] - cam.cam_height_m;
        if cam.pose_source == PoseSource::Sfm {
            continue;
        }
        let Some(meta) = metas.get(pano_id.as_str()) else {
            continue;
        };
        let empty = Vec::new();
        let seq = by_seq.get(meta.sequence.as_str()).unwrap_or(&empty);
        let rebuilt =
            pose::camera_from_meta(meta, &cluster.frame, seq, params, Some(shot.rotation));
        if rebuilt.pose_source == PoseSource::Sfm {
            cam.axes = rebuilt.axes;
            cam.roll_deg = rebuilt.roll_deg;
            cam.pitch_deg = rebuilt.pitch_deg;
            cam.pose_source = rebuilt.pose_source;
            cam.pose_factor = rebuilt.pose_factor;
        }
    }
}

/// The scale check on a cluster: the median ratio of pairwise shot-centre
/// distances to `computed_geometry` distances, which must be 1 within
/// `metric_tol`.
///
/// This replaced the `atomic_scale` gate of DESIGN.md (CRITIQUE A4). Needs two
/// mapped images and a pair more than 2 m apart, else it passes with no ratio.
pub fn metric_check(
    cluster: &mut Cluster,
    metas: &[&PanoMeta],
    shot_map: &HashMap<String, String>,
    frame: &Frame,
    params: &Params,
) -> (f64, bool) {
    let by_id: HashMap<&str, &PanoMeta> = metas.iter().map(|m| (m.id.as_str(), *m)).collect();
    let mut shot_xy = Vec::new();
    let mut graph_xy = Vec::new();
    for (pano_id, shot_id) in shot_map {
        let (Some(meta), Some(shot)) = (by_id.get(pano_id.as_str()), cluster.shot(shot_id)) else {
            continue;
        };
        shot_xy.push([shot.centre[0], shot.centre[1]]);
        graph_xy.push(frame.to_enu(meta.lon, meta.lat));
    }
    if shot_xy.len() < 2 {
        return (f64::NAN, true);
    }
    let mut ratios = Vec::new();
    for i in 0..shot_xy.len() {
        for j in (i + 1)..shot_xy.len() {
            let db = (graph_xy[i][0] - graph_xy[j][0]).hypot(graph_xy[i][1] - graph_xy[j][1]);
            if db <= 2.0 {
                continue;
            }
            let da = (shot_xy[i][0] - shot_xy[j][0]).hypot(shot_xy[i][1] - shot_xy[j][1]);
            ratios.push(da / db);
        }
    }
    if ratios.is_empty() {
        return (f64::NAN, true);
    }
    let ratio = super::imgops::median_in_place(&mut ratios);
    let ok = (ratio - 1.0).abs() <= params.metric_tol;
    cluster.metric_ratio = ratio;
    cluster.scale_ok = ok;
    (ratio, ok)
}

// --------------------------------------------------------------------------- registration shift

/// Applies a registration shift `(dx, dy, theta_deg)` about the raw camera
/// centre: `p' = Rz(theta) (p - C_raw) + C_raw + (dx, dy)`, z untouched.
///
/// `c_reg` is the **registered** camera centre, so the raw one is
/// `c_reg - (dx, dy)`. Passing the registered centre rather than the raw one is
/// what lets a caller that only ever sees registered cameras apply the shift to
/// a fresh set of points.
pub fn shift_points(pts: &mut [[f64; 3]], shift: [f64; 3], c_reg: [f64; 2]) {
    let [dx, dy, theta] = shift;
    if theta.abs() > 1e-12 {
        let c_raw = [c_reg[0] - dx, c_reg[1] - dy];
        let (st, ct) = theta.to_radians().sin_cos();
        for p in pts.iter_mut() {
            let (rx, ry) = (p[0] - c_raw[0], p[1] - c_raw[1]);
            p[0] = ct * rx - st * ry + c_raw[0] + dx;
            p[1] = st * rx + ct * ry + c_raw[1] + dy;
        }
        return;
    }
    for p in pts.iter_mut() {
        p[0] += dx;
        p[1] += dy;
    }
}

/// The shift to apply for a camera: the one given, else its accepted
/// registration, else none.
fn shift_of(cam: &Camera, shift: Option<[f64; 3]>) -> [f64; 3] {
    if let Some(s) = shift {
        return s;
    }
    match cam.reg {
        Some(reg) if reg.accepted => reg.shift(),
        _ => [0.0; 3],
    }
}

// --------------------------------------------------------------------------- ground and heights

/// The ground level under a camera: the 5th percentile z of the points 2.5 to
/// 8 m out in plan that are below the camera, with how many there were.
///
/// The inner radius keeps the vehicle's own roof and bonnet out, and the height
/// test keeps the facade above the rig out. `None` when fewer than
/// `ground_min_pts` points are left, which is the signal to fall back.
pub fn ground_level(cluster: &Cluster, centre: [f64; 3], params: &Params) -> (Option<f64>, usize) {
    let idx = cluster.near_idx([centre[0], centre[1]], params.ground_r[1]);
    if idx.is_empty() {
        return (None, 0);
    }
    let mut z: Vec<f64> = Vec::with_capacity(idx.len());
    for i in idx {
        let p = cluster.points[i];
        let r = (p[0] - centre[0]).hypot(p[1] - centre[1]);
        if r > params.ground_r[0] && p[2] < centre[2] {
            z.push(p[2]);
        }
    }
    let n = z.len();
    if n < params.ground_min_pts as usize {
        return (None, n);
    }
    (Some(percentile_in_place(&mut z, params.ground_pct)), n)
}

/// Fills `ground_z`, `cam_height_m` and `ground_source` for every camera.
///
/// The ladder is cloud, then the median rig height of the camera's own
/// sequence, then the class default. Heights are clamped to the class range
/// before `ground_z` is derived from them, so a cloud answer that says the
/// camera is nine metres up moves the ground rather than the height. A sequence
/// is single-class, so its median never mixes a phone with a 360 rig.
pub fn camera_heights(
    cameras: &mut HashMap<String, Camera>,
    metas: &HashMap<&str, &PanoMeta>,
    clusters: &HashMap<String, Cluster>,
    params: &Params,
) {
    // The cameras are walked in hash order, which is fine: a sequence's median
    // is over a multiset and the second pass only reads what the first wrote.
    let mut seq_heights: HashMap<String, Vec<f64>> = HashMap::new();
    let mut pending: Vec<String> = Vec::new();
    for (pano_id, cam) in cameras.iter_mut() {
        let [lo, hi] = if cam.is_spherical() {
            params.cam_height_range
        } else {
            params.persp_cam_height_range
        };
        let cluster = cam
            .cluster_id
            .as_deref()
            .and_then(|cid| clusters.get(cid))
            .filter(|_| cam.shot_id.is_some());
        let ground = cluster.and_then(|cl| ground_level(cl, cam.centre, params).0);
        let Some(zg) = ground else {
            pending.push(pano_id.clone());
            continue;
        };
        let h = (cam.centre[2] - zg).clamp(lo, hi);
        cam.cam_height_m = h;
        cam.ground_z = cam.centre[2] - h;
        cam.ground_source = GroundSource::Cloud;
        if let Some(meta) = metas.get(pano_id.as_str()) {
            seq_heights
                .entry(meta.sequence.clone())
                .or_default()
                .push(h);
        }
    }
    for pano_id in pending {
        let Some(cam) = cameras.get_mut(&pano_id) else {
            continue;
        };
        let heights = metas
            .get(pano_id.as_str())
            .and_then(|m| seq_heights.get(&m.sequence));
        let (h, source) = match heights {
            Some(hs) if !hs.is_empty() => (super::imgops::median(hs), GroundSource::Sequence),
            _ => (params.height_prior(cam.camera_type), GroundSource::Default),
        };
        cam.cam_height_m = h;
        cam.ground_z = cam.centre[2] - h;
        cam.ground_source = source;
    }
}

/// The z the heights of one view's texture are measured from (CRITIQUE A3).
///
/// The 5th percentile z of the registered points standing in front of this
/// wall, within `foot_search_m` of it and inside a band about the camera's own
/// ground level. Without enough of them the camera's ground level is used, and
/// the second element of the answer says which happened.
pub fn wall_foot_z(
    cluster: &Cluster,
    wall: &Wall,
    shift: [f64; 3],
    z_g: f64,
    c_reg: [f64; 2],
    params: &Params,
) -> (f64, &'static str) {
    let mid = wall.midpoint();
    let centre = [
        mid[0] + wall.n[0] * 0.5 * params.foot_search_m,
        mid[1] + wall.n[1] * 0.5 * params.foot_search_m,
    ];
    let radius = 0.5 * wall.length + params.foot_search_m + 2.0;
    let mut pts = cluster.near([centre[0] - shift[0], centre[1] - shift[1]], radius);
    if pts.is_empty() {
        return (z_g, Z_BASE_PANO);
    }
    shift_points(&mut pts, shift, c_reg);
    let mut z = Vec::new();
    for p in &pts {
        let (s, _, d) = wall.sh_of(*p, 0.0);
        if s >= 0.0
            && s <= wall.length
            && d > 0.0
            && d <= params.foot_search_m
            && p[2] >= z_g - 2.0
            && p[2] <= z_g + 1.5
        {
            z.push(p[2]);
        }
    }
    if z.len() < params.foot_min_pts as usize {
        return (z_g, Z_BASE_PANO);
    }
    (percentile_in_place(&mut z, params.foot_pct), Z_BASE_FOOT)
}

// --------------------------------------------------------------------------- points for later stages

/// The registered cloud points near one wall: the input to the plane fit.
///
/// Within `plane_search_m` of the OSM line either side, a metre past each end,
/// and inside the wall's z band above the camera's ground level.
pub fn wall_points(
    cluster: &Cluster,
    wall: &Wall,
    shift: [f64; 3],
    z_g: f64,
    c_reg: [f64; 2],
    params: &Params,
) -> Vec<[f64; 3]> {
    let mid = wall.midpoint();
    let radius = 0.5 * wall.length + params.plane_search_m + 2.0;
    let mut pts = cluster.near([mid[0] - shift[0], mid[1] - shift[1]], radius);
    if pts.is_empty() {
        return Vec::new();
    }
    shift_points(&mut pts, shift, c_reg);
    pts.retain(|p| {
        let (s, _, d) = wall.sh_of(*p, 0.0);
        s >= -1.0
            && s <= wall.length + 1.0
            && d.abs() <= params.plane_search_m
            && p[2] >= z_g + params.plane_z_range[0]
            && p[2] <= z_g + params.plane_z_range[1]
    });
    pts
}

// --------------------------------------------------------------------------- depth

/// An equirectangular depth map splatted from the cloud, minimum depth per cell.
#[derive(Clone, Debug)]
pub struct DepthMap {
    pub width: u32,
    pub height: u32,
    /// Row major, infinite where no point fell in the cell. Every finite value
    /// has been rounded to float16, which is what the Python stores.
    pub depth: Vec<f32>,
}

impl DepthMap {
    fn empty(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            depth: vec![f32::INFINITY; (width as usize) * (height as usize)],
        }
    }

    /// The depth in one cell, infinite where nothing was seen.
    pub fn at(&self, col: u32, row: u32) -> f32 {
        self.depth[(row as usize) * (self.width as usize) + col as usize]
    }

    /// The depth along a normalised image direction, or `None` off the map.
    ///
    /// `u` wraps, because the equirectangular seam is only in u; `v` is clipped,
    /// because letting it wrap would answer a question about the sky with the
    /// ground.
    pub fn sample(&self, u: f64, v: f64) -> Option<f32> {
        if !u.is_finite() || !v.is_finite() {
            return None;
        }
        let col = ((u * f64::from(self.width)) as i64).rem_euclid(i64::from(self.width)) as u32;
        let row = ((v * f64::from(self.height)) as i64).clamp(0, i64::from(self.height) - 1) as u32;
        Some(self.at(col, row))
    }
}

/// The depth map of one camera, splatted from the registered cloud.
///
/// Panoramas splat over the whole sphere; a perspective camera splats only what
/// lands on its image, inside the radial validity domain, in front of it. The
/// 3 by 3 minimum filter closes the gaps a sparse cloud leaves between its
/// points, and wraps in u for a panorama because the seam is not an edge.
pub fn depth_map(
    cluster: &Cluster,
    cam: &Camera,
    shift: Option<[f64; 3]>,
    params: &Params,
) -> DepthMap {
    let [w, h] = params.depth_size;
    let (wi, hi) = (w as usize, h as usize);
    let mut out = DepthMap::empty(w, h);
    if wi == 0 || hi == 0 {
        return out;
    }
    let sh = shift_of(cam, shift);
    let mut pts = cluster.near(
        [cam.centre[0] - sh[0], cam.centre[1] - sh[1]],
        params.depth_radius_m,
    );
    if pts.is_empty() {
        return out;
    }
    shift_points(&mut pts, sh, [cam.centre[0], cam.centre[1]]);

    let projector = Projector::new(cam);
    let spherical = cam.is_spherical();
    let mut hits: Vec<(usize, f32)> = Vec::with_capacity(pts.len());
    for p in &pts {
        let proj = projector.project(*p);
        if !proj.is_valid() || (!spherical && !proj.inside_image()) {
            continue;
        }
        let col = ((proj.u * w as f64) as i64).rem_euclid(w as i64) as usize;
        let row = ((proj.v * h as f64) as i64).clamp(0, h as i64 - 1) as usize;
        hits.push((row * wi + col, proj.length_m as f32));
    }
    if hits.is_empty() {
        return out;
    }
    // Far first so the nearest point is the last write into a cell, which is
    // what `argsort(r)[::-1]` plus an indexed assignment does in numpy.
    hits.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (cell, depth) in hits {
        out.depth[cell] = depth;
    }

    let filtered = min_filter_3x3(&out.depth, wi, hi, spherical);
    out.depth = filtered.into_iter().map(round_to_f16).collect();
    out
}

/// A 3 by 3 minimum filter, separable, with rows replicated at the top and
/// bottom and columns wrapped when `wrap_cols` (the equirectangular seam).
fn min_filter_3x3(src: &[f32], w: usize, h: usize, wrap_cols: bool) -> Vec<f32> {
    let mut rows = vec![f32::INFINITY; src.len()];
    for y in 0..h {
        let up = y.saturating_sub(1);
        let down = (y + 1).min(h - 1);
        for x in 0..w {
            rows[y * w + x] = src[up * w + x].min(src[y * w + x]).min(src[down * w + x]);
        }
    }
    let mut out = vec![f32::INFINITY; src.len()];
    for y in 0..h {
        let base = y * w;
        for x in 0..w {
            let left = if x > 0 {
                x - 1
            } else if wrap_cols {
                w - 1
            } else {
                0
            };
            let right = if x + 1 < w {
                x + 1
            } else if wrap_cols {
                0
            } else {
                w - 1
            };
            out[base + x] = rows[base + left]
                .min(rows[base + x])
                .min(rows[base + right]);
        }
    }
    out
}

/// `numpy.float16` rounding, round to nearest with ties to even, kept as `f32`.
///
/// The depth map is stored as float16 in the Python and every consumer sees the
/// rounded value, so the port rounds too rather than carrying a more precise
/// number the Python never had.
pub fn round_to_f16(x: f32) -> f32 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let sign = if x < 0.0 { -1.0f32 } else { 1.0f32 };
    let a = f64::from(x.abs());
    // 65520 is the midpoint above the largest float16, so anything from there
    // up rounds away to infinity.
    if a >= 65520.0 {
        return sign * f32::INFINITY;
    }
    // Subnormal float16 all share the exponent of the smallest normal one.
    let exp = a.log2().floor().max(-14.0);
    let ulp = (exp - 10.0).exp2();
    let q = a / ulp;
    let floor = q.floor();
    let frac = q - floor;
    let rounded = if frac > 0.5 || (frac == 0.5 && (floor as i64) % 2 == 1) {
        floor + 1.0
    } else {
        floor
    };
    sign * (rounded * ulp) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapillary::types::{CameraModel, GroundSource};

    fn params() -> Params {
        Params::default()
    }

    fn frame() -> Frame {
        Frame::new(11.5795305, 48.13643)
    }

    fn cluster_of(points: Vec<[f64; 3]>) -> Cluster {
        let raw = RawCluster {
            cluster_id: "c".into(),
            ref_lla: (11.5795305, 48.13643, 500.0),
            shots: Vec::new(),
            colors: vec![[0, 0, 0]; points.len()],
            points,
        };
        Cluster::from_raw(&raw, &frame())
    }

    fn camera(centre: [f64; 3], model: CameraModel) -> Camera {
        Camera {
            pano_id: "p".into(),
            centre,
            axes: pose::level_axes(0.0, 0.0, 0.0),
            pose_source: PoseSource::Sfm,
            roll_deg: 0.0,
            pitch_deg: 0.0,
            ground_z: centre[2] - 2.5,
            cam_height_m: 2.5,
            ground_source: GroundSource::Default,
            cluster_id: Some("c".into()),
            shot_id: Some("s".into()),
            reg: None,
            compass_deg: 0.0,
            pose_factor: 1.0,
            width: 5760,
            height: 2880,
            camera_type: model,
            camera_params: vec![],
        }
    }

    #[test]
    fn the_plan_index_answers_the_same_disk_a_scan_does() {
        let mut points = Vec::new();
        for i in 0..2000 {
            let a = i as f64 * 0.37;
            points.push([30.0 * a.cos(), 21.0 * (a * 1.3).sin(), a % 7.0]);
        }
        let cl = cluster_of(points);
        for radius in [1.0, 4.5, 8.0, 40.0] {
            for centre in [[0.0, 0.0], [12.5, -7.25], [100.0, 100.0]] {
                let got = cl.near_idx(centre, radius);
                let want: Vec<usize> = (0..cl.len())
                    .filter(|&i| {
                        let p = cl.points[i];
                        (p[0] - centre[0]).hypot(p[1] - centre[1]) <= radius
                    })
                    .collect();
                assert_eq!(got, want, "radius {radius} about {centre:?}");
            }
        }
    }

    #[test]
    fn the_ground_ignores_the_vehicle_and_the_facade() {
        let mut points = Vec::new();
        // A ring of road at z = 0, well outside the inner radius.
        for i in 0..200 {
            let a = i as f64 * 0.031;
            points.push([5.0 * a.cos(), 5.0 * a.sin(), 0.0]);
        }
        // The car roof under the camera, and a facade above it.
        for i in 0..200 {
            let a = i as f64 * 0.031;
            points.push([1.0 * a.cos(), 1.0 * a.sin(), 1.5]);
            points.push([6.0 * a.cos(), 6.0 * a.sin(), 8.0]);
        }
        let cl = cluster_of(points);
        let cam = camera([0.0, 0.0, 2.5], CameraModel::Spherical);
        let (z, n) = ground_level(&cl, cam.centre, &params());
        assert_eq!(n, 200, "only the road ring is left");
        assert!((z.unwrap() - 0.0).abs() < 1e-9);
        // Too few points is a refusal, not a guess.
        let sparse = cluster_of(vec![[5.0, 0.0, 0.0]; 10]);
        assert_eq!(ground_level(&sparse, cam.centre, &params()), (None, 10));
    }

    #[test]
    fn camera_heights_walk_the_ladder() {
        let mut points = Vec::new();
        for i in 0..200 {
            let a = i as f64 * 0.031;
            points.push([5.0 * a.cos(), 5.0 * a.sin(), 0.0]);
        }
        let mut clusters = HashMap::new();
        clusters.insert("c".to_string(), cluster_of(points));

        let seen = camera([0.0, 0.0, 2.9], CameraModel::Spherical);
        let mut blind = camera([500.0, 500.0, 4.0], CameraModel::Spherical);
        blind.pano_id = "q".into();
        let mut orphan = camera([500.0, 500.0, 4.0], CameraModel::Perspective);
        orphan.pano_id = "r".into();
        orphan.cluster_id = None;
        orphan.shot_id = None;

        let mut cameras = HashMap::new();
        for cam in [seen, blind, orphan] {
            cameras.insert(cam.pano_id.clone(), cam);
        }
        let metas: Vec<PanoMeta> = ["p", "q", "r"]
            .iter()
            .map(|id| PanoMeta {
                id: (*id).to_string(),
                lon: 11.58,
                lat: 48.136,
                alt: 0.0,
                compass: 0.0,
                rotation: None,
                atomic_scale: None,
                captured_at: 0,
                sequence: if *id == "r" { "other" } else { "seq" }.to_string(),
                quality: 1.0,
                width: 1,
                height: 1,
                cluster_id: None,
                geometry_source: super::super::types::GeometrySource::Computed,
                camera_type: CameraModel::Spherical,
                camera_params: vec![],
            })
            .collect();
        let meta_map: HashMap<&str, &PanoMeta> = metas.iter().map(|m| (m.id.as_str(), m)).collect();

        camera_heights(&mut cameras, &meta_map, &clusters, &params());
        let p = &cameras["p"];
        assert_eq!(p.ground_source, GroundSource::Cloud);
        assert!((p.cam_height_m - 2.9).abs() < 1e-9);
        assert!((p.ground_z - 0.0).abs() < 1e-9);
        // No cloud under it, but a sequence mate that had one.
        let q = &cameras["q"];
        assert_eq!(q.ground_source, GroundSource::Sequence);
        assert!((q.cam_height_m - 2.9).abs() < 1e-9);
        // No cloud and no sequence mate: the class prior, and a phone's prior.
        let r = &cameras["r"];
        assert_eq!(r.ground_source, GroundSource::Default);
        assert!((r.cam_height_m - 1.4).abs() < 1e-9);
    }

    #[test]
    fn a_cloud_answer_is_clamped_before_the_ground_is_derived() {
        // Ground points nine metres below a 360 rig: the height is capped at
        // 4.5 m and the ground moves up with it rather than the height being
        // believed.
        let mut points = Vec::new();
        for i in 0..200 {
            let a = i as f64 * 0.031;
            points.push([5.0 * a.cos(), 5.0 * a.sin(), -9.0]);
        }
        let mut clusters = HashMap::new();
        clusters.insert("c".to_string(), cluster_of(points));
        let cam = camera([0.0, 0.0, 0.0], CameraModel::Spherical);
        let mut cameras = HashMap::new();
        cameras.insert(cam.pano_id.clone(), cam);
        camera_heights(&mut cameras, &HashMap::new(), &clusters, &params());
        let c = &cameras["p"];
        assert!((c.cam_height_m - 4.5).abs() < 1e-9);
        assert!((c.ground_z + 4.5).abs() < 1e-9);
    }

    #[test]
    fn a_shift_turns_about_the_raw_camera_centre() {
        // The registered centre is the raw one plus (dx, dy), so a point at the
        // raw centre lands on the registered centre whatever the turn is.
        let mut pts = [[8.0, 0.0, 3.0], [10.0, 0.0, 3.0]];
        let c_reg = [10.0, 5.0];
        shift_points(&mut pts, [2.0, 5.0, 90.0], c_reg);
        // Raw centre (8, 0); the first point is on it.
        assert!((pts[0][0] - 10.0).abs() < 1e-9 && (pts[0][1] - 5.0).abs() < 1e-9);
        // The second is 2 m east of it, so a quarter turn puts it 2 m north.
        assert!((pts[1][0] - 10.0).abs() < 1e-9 && (pts[1][1] - 7.0).abs() < 1e-9);
        assert!((pts[1][2] - 3.0).abs() < 1e-12, "z is never touched");
        // Without a turn it is a translation.
        let mut plain = [[1.0, 2.0, 3.0]];
        shift_points(&mut plain, [0.5, -0.25, 0.0], c_reg);
        assert_eq!(plain[0], [1.5, 1.75, 3.0]);
    }

    #[test]
    fn float16_rounding_matches_numpy() {
        // Exactly representable values are untouched.
        for v in [0.0f32, 1.0, 0.5, 2048.0, 65504.0, -12.5] {
            assert_eq!(round_to_f16(v), v, "{v}");
        }
        // The spacing at 30 m is 1/64, and a tie rounds to the even neighbour.
        assert_eq!(round_to_f16(30.0), 30.0);
        assert_eq!(round_to_f16(30.007), 30.0);
        assert_eq!(round_to_f16(30.01), 30.015_625);
        assert_eq!(round_to_f16(30.015_625 / 2.0 + 30.0 / 2.0), 30.0);
        assert_eq!(round_to_f16(30.015_625 / 2.0 + 30.031_25 / 2.0), 30.031_25);
        assert_eq!(round_to_f16(30.046_875 / 2.0 + 30.031_25 / 2.0), 30.031_25);
        // Above the largest float16 the answer is infinity, as numpy's cast is.
        assert!(round_to_f16(70000.0).is_infinite());
        assert!(round_to_f16(f32::INFINITY).is_infinite());
        // Subnormals still round rather than flushing to zero.
        assert_eq!(round_to_f16(1e-7), 1.192_092_9e-7);
        assert_eq!(round_to_f16(1e-9), 0.0);
    }

    #[test]
    fn the_minimum_filter_wraps_only_for_a_panorama() {
        let src = vec![
            9.0, 9.0, 1.0, //
            9.0, 9.0, 9.0, //
            5.0, 9.0, 9.0,
        ];
        let wrapped = min_filter_3x3(&src, 3, 3, true);
        // Column 0 of the top row sees column 2 across the seam.
        assert_eq!(wrapped[0], 1.0);
        let clamped = min_filter_3x3(&src, 3, 3, false);
        assert_eq!(clamped[0], 9.0);
        // Both replicate rows, so the bottom left 5 reaches the middle row; only
        // the wrapped one also pulls the top right 1 round the seam into it.
        assert_eq!(clamped[3], 5.0);
        assert_eq!(wrapped[3], 1.0);
    }

    #[test]
    fn a_panorama_depth_map_sees_a_wall_in_front_of_it() {
        // A slab of points 10 m north of the camera, sampled finely enough in z
        // that every row of the map it covers is hit: a row is a degree, which
        // is 0.17 m at this distance.
        let mut points = Vec::new();
        for i in 0..60 {
            for j in 0..120 {
                points.push([-3.0 + f64::from(i) * 0.1, 10.0, f64::from(j) * 0.05]);
            }
        }
        let cl = cluster_of(points);
        let cam = camera([0.0, 0.0, 2.0], CameraModel::Spherical);
        let depth = depth_map(&cl, &cam, None, &params());
        assert_eq!((depth.width, depth.height), (720, 360));
        // Due north is the middle column, and the horizon the middle row.
        let ahead = depth.sample(0.5, 0.5).unwrap();
        assert!((ahead - 10.0).abs() < 0.5, "straight ahead {ahead}");
        // Behind the camera nothing was splatted.
        assert!(depth.sample(0.0, 0.5).unwrap().is_infinite());
        // Every finite cell is a float16 value.
        for &d in &depth.depth {
            assert_eq!(d, round_to_f16(d));
        }
    }

    // ----------------------------------------------------------------- golden

    use crate::mapillary::golden;

    /// The whole stage over the Munich box: the shot map, the metric check, the
    /// shot centres, and the ground level and camera height under all 1076
    /// cameras, against what the Python run wrote.
    #[test]
    fn golden_the_sfm_stage_reproduces_the_python_run() {
        if golden::absent() {
            return;
        }

        let gf: golden::GoldenFrame = golden::load("frame.json");
        let frame = gf.frame();
        let params = Params::default();
        let metas: Vec<PanoMeta> = golden::panos()
            .iter()
            .filter_map(PanoMeta::from_graph)
            .filter(|m| params.admits(m.camera_type))
            .collect();
        assert_eq!(metas.len(), 1076, "cameras on the box");
        let mut by_seq: HashMap<&str, Vec<&PanoMeta>> = HashMap::new();
        for m in &metas {
            by_seq.entry(m.sequence.as_str()).or_default().push(m);
        }
        let meta_map: HashMap<&str, &PanoMeta> = metas.iter().map(|m| (m.id.as_str(), m)).collect();

        let mut cameras: HashMap<String, Camera> = HashMap::new();
        for m in &metas {
            let empty = Vec::new();
            let seq = by_seq.get(m.sequence.as_str()).unwrap_or(&empty);
            cameras.insert(
                m.id.clone(),
                pose::camera_from_meta(m, &frame, seq, &params, None),
            );
        }

        let recs: Vec<golden::GoldenCluster> = golden::load_sfm("clusters.json");
        assert_eq!(recs.len(), 88, "clusters the run could read");
        let mut clusters: HashMap<String, Cluster> = HashMap::new();
        let mut mapped = 0usize;
        let mut worst_ratio = 0.0f64;
        for rec in &recs {
            let mut cluster = Cluster::from_raw(&rec.raw(), &frame);
            assert_eq!(cluster.shots.len(), rec.shots.len(), "{}", rec.id);
            let of_cluster: Vec<&PanoMeta> = metas
                .iter()
                .filter(|m| m.cluster_id.as_deref() == Some(rec.id.as_str()))
                .collect();
            let shot_map = map_shots(&cluster, &of_cluster, &params);
            assert_eq!(shot_map.len(), rec.mapped_shots, "{} shot map size", rec.id);
            for (pano_id, shot_id) in &rec.shot_map {
                assert_eq!(
                    shot_map.get(pano_id),
                    Some(shot_id),
                    "{} maps {pano_id}",
                    rec.id
                );
            }
            mapped += shot_map.len();
            let (ratio, ok) = metric_check(&mut cluster, &of_cluster, &shot_map, &frame, &params);
            match rec.metric_ratio {
                Some(want) => worst_ratio = worst_ratio.max((ratio - want).abs()),
                None => assert!(ratio.is_nan(), "{} has no ratio to give", rec.id),
            }
            assert_eq!(ok, rec.scale_ok, "{} scale gate", rec.id);
            apply_shot_poses(
                &mut cameras,
                &cluster,
                &shot_map,
                &meta_map,
                &by_seq,
                &params,
            );
            clusters.insert(rec.id.clone(), cluster);
        }
        assert_eq!(mapped, 1018, "images matched to a shot");
        assert!(worst_ratio < 1e-9, "worst metric ratio {worst_ratio}");

        camera_heights(&mut cameras, &meta_map, &clusters, &params);

        let want: Vec<golden::GoldenGeometryCamera> = golden::load("cameras_geometry.json");
        assert_eq!(want.len(), cameras.len());
        let mut worst_centre = 0.0f64;
        let mut worst_ground = 0.0f64;
        let mut worst_height = 0.0f64;
        let mut worst_axes = 0.0f64;
        let mut sources: HashMap<&str, usize> = HashMap::new();
        for w in &want {
            let cam = cameras.get(&w.id).expect("every camera of the fixture");
            assert_eq!(cam.pose_source.as_str(), w.pose_source, "{} pose", w.id);
            assert_eq!(cam.shot_id.as_deref(), w.shot_id.as_deref(), "{}", w.id);
            assert_eq!(
                cam.cluster_id.as_deref(),
                w.cluster_id.as_deref(),
                "{}",
                w.id
            );
            assert_eq!(
                cam.ground_source.as_str(),
                w.ground_source,
                "{} ground source",
                w.id
            );
            worst_centre = worst_centre.max(golden::max_abs_diff(&cam.centre, &w.centre));
            worst_ground = worst_ground.max((cam.ground_z - w.ground_z).abs());
            worst_height = worst_height.max((cam.cam_height_m - w.cam_height_m).abs());
            for i in 0..3 {
                worst_axes = worst_axes.max(golden::max_abs_diff(&cam.axes[i], &w.axes[i]));
            }
            *sources.entry(cam.ground_source.as_str()).or_default() += 1;
        }
        assert!(worst_centre < 0.01, "worst shot centre {worst_centre} m");
        assert!(worst_ground < 0.01, "worst ground z {worst_ground} m");
        assert!(worst_height < 0.01, "worst camera height {worst_height} m");
        assert!(worst_axes < 1e-6, "worst rotation entry {worst_axes}");
        assert_eq!(sources.get("cloud"), Some(&568));
        assert_eq!(sources.get("sequence"), Some(&397));
        assert_eq!(sources.get("default"), Some(&111));

        // The count of points the ground level was taken over, which is what
        // says the plan search agrees and not only its answer.
        let ground: Vec<golden::GoldenGroundLevel> = golden::load_sfm("ground.json");
        let mut checked = 0usize;
        for g in &ground {
            let cam = &cameras[&g.id];
            let n = cam
                .cluster_id
                .as_deref()
                .and_then(|cid| clusters.get(cid))
                .map_or(0, |cl| ground_level(cl, cam.centre, &params).1);
            assert_eq!(n, g.n_points, "{} ground points", g.id);
            checked += 1;
        }
        assert_eq!(checked, 1076);
        println!(
            "1076 cameras, 88 clusters, {mapped} mapped shots: worst centre {worst_centre:.2e} m, \
             ground z {worst_ground:.2e} m, height {worst_height:.2e} m, rotation \
             {worst_axes:.2e}, metric ratio {worst_ratio:.2e}"
        );
    }

    /// The depth maps of three real cameras, cell for cell.
    #[test]
    fn golden_depth_maps_match_the_python_run() {
        if golden::pixels_absent() {
            return;
        }

        let gf: golden::GoldenFrame = golden::load("frame.json");
        let frame = gf.frame();
        let params = Params::default();
        let recs: Vec<golden::GoldenCluster> = golden::load_sfm("clusters.json");
        let cases: Vec<golden::GoldenDepth> = golden::load_sfm("depth.json");
        assert!(cases.len() >= 3, "a spherical, a phone and a shifted one");

        let mut worst_mm = 0.0f64;
        let mut cells = 0usize;
        let mut finite = 0usize;
        for case in &cases {
            let rec = recs
                .iter()
                .find(|r| r.id == case.camera.cluster_id)
                .expect("the fixture ships the camera's cluster");
            let cluster = Cluster::from_raw(&rec.raw(), &frame);
            let cam = case.camera.camera();
            // The shift comes off the camera's own registration, which is what
            // the pipeline does.
            let got = depth_map(&cluster, &cam, None, &params);
            let want = case.depth();
            assert_eq!((got.width, got.height), (case.width, case.height));
            assert_eq!(got.depth.len(), want.len());
            let mut got_finite = 0usize;
            for (i, (&g, &w)) in got.depth.iter().zip(&want).enumerate() {
                assert_eq!(
                    g.is_finite(),
                    w.is_finite(),
                    "{} cell {i}: {g} against {w}",
                    case.camera.pano_id
                );
                if g.is_finite() {
                    got_finite += 1;
                    worst_mm = worst_mm.max(f64::from(g - w).abs() * 1000.0);
                }
            }
            assert_eq!(
                got_finite, case.finite_cells,
                "{} finite cells",
                case.camera.pano_id
            );
            cells += got.depth.len();
            finite += got_finite;
        }
        assert!(worst_mm < 1.0, "worst depth difference {worst_mm} mm");
        println!(
            "{} depth maps, {cells} cells of which {finite} finite: worst difference \
             {worst_mm:.3e} mm",
            cases.len()
        );
    }
}
