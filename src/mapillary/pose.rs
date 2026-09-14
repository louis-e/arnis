//! Camera axes and projection. Port of `tools/facade_lab/pose.py`.
//!
//! The one convention in this pipeline that cost days, written down so it is
//! never re-derived: **the delivered equirectangular images are not levelled**.
//! The pixels are as uploaded and the rig tilt is still in them, so
//! `computed_rotation` has to be applied. It is an axis-angle vector, world to
//! camera, with an ENU world (x east, y north, z up) and a camera with x right,
//! y down, z forward, which makes the **rows** of its rotation matrix the camera
//! axes in ENU. Skipping it cost 7.5 degrees of standard deviation in
//! vertical-edge lean in Munich, and 99 against 38 of 128 walls within 2 degrees
//! in Berlin.
//!
//! Spherical projection, identical to `project.rs`:
//! `d = P - C`, `(cx, cy, cz) = axes * d`, `u = 0.5 + atan2(cx, cz) / 2pi`,
//! `v = 0.5 + asin(cy / |d|) / pi`.
//!
//! The OpenSfM `perspective`, `brown` and `fisheye` models go through the same
//! ray but are valid only inside [`radial_limit`]: past the first stationary
//! point of the radial polynomial the mapping folds points back inside the
//! image at mirrored positions, so a texel out there would be painted with a
//! pixel from somewhere else entirely. Those come back as no projection at all,
//! the same answer as a point behind the camera.
//!
//! Two differences from the Python, both deliberate:
//!
//! * [`Projector`] exists. Python memoises [`radial_limit`] with an LRU cache
//!   because it scans a 1201 point grid; here the limit is computed once when
//!   the projector is built and the hot loop never sees it.
//! * `pose.autolevel` is not ported. It estimates the horizon from vertical
//!   line segments when no SfM rotation is available, and no caller in the
//!   Python pipeline ever passes it an image, so the branch is dead: all 1076
//!   cameras of the Munich box come out of the `sfm` source. It also needs the
//!   line segment detector, which belongs to `refine.rs`. If a fallback for
//!   rotation-less imagery is ever wanted, it goes there.

#![allow(dead_code)]

use std::f64::consts::{PI, TAU};

use super::types::{Camera, CameraModel, Frame, GroundSource, PanoMeta, Params, PoseSource};

/// 80.5 degrees off axis. Every image corner in the Munich data is under 3.7,
/// so this cap only ever applies to a polynomial that never turns.
pub const RADIAL_LIMIT_MAX: f64 = 6.0;
/// Stay clear of the infinite-compression ring at the stationary point itself.
pub const RADIAL_LIMIT_SAFETY: f64 = 0.9;
/// Samples of the radial polynomial between 0 and [`RADIAL_LIMIT_MAX`].
const RADIAL_SAMPLES: usize = 1201;

type Vec3 = [f64; 3];
type Mat3 = [[f64; 3]; 3];

const UP: Vec3 = [0.0, 0.0, 1.0];

// --------------------------------------------------------------------------- small vector maths

#[inline]
fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
fn norm(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}

#[inline]
fn scale(a: Vec3, k: f64) -> Vec3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

#[inline]
fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

// --------------------------------------------------------------------------- axes

/// Rotation matrix of an axis-angle vector (Rodrigues), whose rows are the
/// camera right, down and forward directions in ENU.
pub fn axes_from_rotation(rotvec: Vec3) -> Mat3 {
    let theta = norm(rotvec);
    if theta < 1e-12 {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let k = scale(rotvec, 1.0 / theta);
    let (s, c) = theta.sin_cos();
    let skew = [[0.0, -k[2], k[1]], [k[2], 0.0, -k[0]], [-k[1], k[0], 0.0]];
    let mut m = [[0.0f64; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            let identity = if i == j { 1.0 } else { 0.0 };
            *cell = identity * c + s * skew[i][j] + (1.0 - c) * k[i] * k[j];
        }
    }
    m
}

/// Compass heading in degrees clockwise from north of the forward axis'
/// horizontal part.
pub fn heading_of(axes: &Mat3) -> f64 {
    let f = axes[2];
    f[0].atan2(f[1]).to_degrees().rem_euclid(360.0)
}

/// Axes for a heading-only camera with optional roll and pitch.
///
/// Built as level(heading), then pitch about the right axis (positive looks
/// up), then roll about the forward axis (positive dips the right axis). The
/// inverse of [`roll_pitch_of`].
pub fn level_axes(compass_deg: f64, roll_deg: f64, pitch_deg: f64) -> Mat3 {
    let h = compass_deg.to_radians();
    let (sh, ch) = h.sin_cos();
    let forward = [sh, ch, 0.0];
    let right = [ch, -sh, 0.0];
    let down = [0.0, 0.0, -1.0];
    let (sp, cp) = pitch_deg.to_radians().sin_cos();
    let f2 = sub(scale(forward, cp), scale(down, sp));
    let d2 = add(scale(down, cp), scale(forward, sp));
    let (sr, cr) = roll_deg.to_radians().sin_cos();
    let r3 = add(scale(right, cr), scale(d2, sr));
    let d3 = sub(scale(d2, cr), scale(right, sr));
    [r3, d3, f2]
}

/// `(roll_deg, pitch_deg)` of `axes` relative to a level camera at
/// `compass_deg`.
///
/// Any yaw between the axes' own heading and `compass_deg` is absorbed rather
/// than leaking into roll and pitch, so
/// `roll_pitch_of(level_axes(c, r, p), c) == (r, p)`.
pub fn roll_pitch_of(axes: &Mat3, compass_deg: f64) -> (f64, f64) {
    let level = level_axes(compass_deg, 0.0, 0.0);
    // Forward and right expressed in the level camera's own frame.
    let f_l = [
        dot(level[0], axes[2]),
        dot(level[1], axes[2]),
        dot(level[2], axes[2]),
    ];
    let r_l = [
        dot(level[0], axes[0]),
        dot(level[1], axes[0]),
        dot(level[2], axes[0]),
    ];
    let pitch = (-f_l[1]).clamp(-1.0, 1.0).asin().to_degrees();
    let down_l = [0.0, 1.0, 0.0];
    let r0 = cross(down_l, f_l);
    let nr = norm(r0);
    if nr < 1e-9 {
        // Looking straight up or down: roll is not defined, so report none.
        return (0.0, pitch);
    }
    let r0 = scale(r0, 1.0 / nr);
    let d0 = cross(f_l, r0);
    let roll = dot(r_l, d0).atan2(dot(r_l, r0)).to_degrees();
    (roll, pitch)
}

/// Turns all three camera axes about ENU up by `theta_deg`, counter-clockwise
/// seen from above, which makes the heading decrease.
pub fn rotate_axes_about_z(axes: &Mat3, theta_deg: f64) -> Mat3 {
    let (s, c) = theta_deg.to_radians().sin_cos();
    let rz = [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]];
    let mut out = [[0.0f64; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = dot(axes[i], rz[j]);
        }
    }
    out
}

// --------------------------------------------------------------------------- distortion

/// The OpenSfM camera models on normalised `(x/z, y/z)`, giving image plane
/// coordinates in units of `max(width, height)` about the principal point.
fn distort(xn: f64, yn: f64, params: &[f64], model: CameraModel) -> (f64, f64) {
    let r2 = xn * xn + yn * yn;
    if model == CameraModel::Brown && params.len() >= 9 {
        let (fx, fy, cx0, cy0) = (params[0], params[1], params[2], params[3]);
        let (k1, k2, p1, p2, k3) = (params[4], params[5], params[6], params[7], params[8]);
        let d = 1.0 + k1 * r2 + k2 * r2 * r2 + k3 * r2 * r2 * r2;
        let xd = xn * d + 2.0 * p1 * xn * yn + p2 * (r2 + 2.0 * xn * xn);
        let yd = yn * d + p1 * (r2 + 2.0 * yn * yn) + 2.0 * p2 * xn * yn;
        return (fx * xd + cx0, fy * yd + cy0);
    }
    let f = params.first().copied().unwrap_or(0.85);
    let k1 = params.get(1).copied().unwrap_or(0.0);
    let k2 = params.get(2).copied().unwrap_or(0.0);
    if model == CameraModel::Fisheye {
        let r = r2.sqrt();
        let theta = r.atan();
        let t2 = theta * theta;
        let rd = theta * (1.0 + k1 * t2 + k2 * t2 * t2);
        let s = if r > 1e-12 { rd / r.max(1e-12) } else { 1.0 };
        return (f * xn * s, f * yn * s);
    }
    let d = 1.0 + k1 * r2 + k2 * r2 * r2;
    (f * xn * d, f * yn * d)
}

/// The largest undistorted normalised radius `r = |(x/z, y/z)|` a non-spherical
/// model maps injectively.
///
/// The distorted radius `r (1 + k1 r^2 + k2 r^4 + k3 r^6)` stops growing at the
/// first root of `1 + 3 k1 r^2 + 5 k2 r^4 + 7 k3 r^6`; past it points fold back
/// inside the image at mirrored positions. Pixels with a distorted radius past
/// the peak have no valid preimage, so cutting at 0.9 of that root discards only
/// folded points and never a reachable pixel. Returns [`RADIAL_LIMIT_MAX`] when
/// the polynomial never turns.
pub fn radial_limit(model: CameraModel, params: &[f64]) -> f64 {
    let (k1, k2, k3) = if model == CameraModel::Brown && params.len() >= 9 {
        (params[4], params[5], params[8])
    } else {
        (
            params.get(1).copied().unwrap_or(0.0),
            params.get(2).copied().unwrap_or(0.0),
            0.0,
        )
    };
    let step = RADIAL_LIMIT_MAX / (RADIAL_SAMPLES - 1) as f64;
    for i in 0..RADIAL_SAMPLES {
        let r = i as f64 * step;
        // The fisheye polynomial is evaluated in theta = atan(r), which is what
        // its own distortion does; the cap is the same.
        let x = if model == CameraModel::Fisheye {
            r.atan()
        } else {
            r
        };
        let x2 = x * x;
        let g = 1.0 + 3.0 * k1 * x2 + 5.0 * k2 * x2 * x2 + 7.0 * k3 * x2 * x2 * x2;
        if g <= 0.0 {
            return RADIAL_LIMIT_SAFETY * r;
        }
    }
    RADIAL_LIMIT_MAX
}

// --------------------------------------------------------------------------- projection

/// Where a world point lands, in normalised image units.
///
/// `u` and `v` are NaN when the point has no projection at all: behind the
/// camera, or past the radial limit where the distortion polynomial folds. They
/// can also be outside `0..1`, which means the point is on the image plane but
/// off the sensor; [`Projection::inside_image`] separates the two.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection {
    pub u: f64,
    pub v: f64,
    /// Distance from the camera centre to the point.
    pub length_m: f64,
}

impl Projection {
    /// True when the point projects onto the image at all.
    pub fn inside_image(&self) -> bool {
        self.u.is_finite()
            && self.v.is_finite()
            && (0.0..1.0).contains(&self.u)
            && (0.0..1.0).contains(&self.v)
    }

    pub fn is_valid(&self) -> bool {
        self.u.is_finite() && self.v.is_finite()
    }
}

/// One camera's projection, with everything that does not depend on the point
/// worked out once.
///
/// Build one per camera and reuse it: [`radial_limit`] scans a 1201 point grid,
/// and a wall rectangle at 8 px per metre is tens of thousands of points.
#[derive(Clone, Debug)]
pub struct Projector {
    centre: Vec3,
    axes: Mat3,
    model: CameraModel,
    params: Vec<f64>,
    width: f64,
    height: f64,
    /// Squared radial limit, so the hot loop compares squares.
    radial_limit2: f64,
    /// `max(width, height)`, the unit the OpenSfM parameters are normalised by.
    big: f64,
}

impl Projector {
    pub fn new(cam: &Camera) -> Self {
        let limit = if cam.camera_type.is_spherical() {
            f64::INFINITY
        } else {
            radial_limit(cam.camera_type, &cam.camera_params)
        };
        Self {
            centre: cam.centre,
            axes: cam.axes,
            model: cam.camera_type,
            params: cam.camera_params.clone(),
            width: f64::from(cam.width),
            height: f64::from(cam.height),
            radial_limit2: limit * limit,
            big: f64::from(cam.width.max(cam.height).max(1)),
        }
    }

    pub fn is_spherical(&self) -> bool {
        self.model.is_spherical()
    }

    pub fn width(&self) -> u32 {
        self.width as u32
    }

    pub fn height(&self) -> u32 {
        self.height as u32
    }

    /// The camera frame coordinates `(right, down, forward)` of a world point.
    #[inline]
    pub fn to_camera(&self, p: Vec3) -> Vec3 {
        let d = sub(p, self.centre);
        [
            dot(d, self.axes[0]),
            dot(d, self.axes[1]),
            dot(d, self.axes[2]),
        ]
    }

    /// Where a world point lands.
    pub fn project(&self, p: Vec3) -> Projection {
        let c = self.to_camera(p);
        let (cx, cy, cz) = (c[0], c[1], c[2]);
        let length = (cx * cx + cy * cy + cz * cz).sqrt();
        if self.model.is_spherical() {
            return Projection {
                u: 0.5 + cx.atan2(cz) / TAU,
                v: 0.5 + (cy / length.max(1e-12)).clamp(-1.0, 1.0).asin() / PI,
                length_m: length,
            };
        }
        let front = cz > 1e-6;
        let zsafe = if front { cz } else { 1.0 };
        let (xn, yn) = (cx / zsafe, cy / zsafe);
        if !front || xn * xn + yn * yn >= self.radial_limit2 {
            return Projection {
                u: f64::NAN,
                v: f64::NAN,
                length_m: length,
            };
        }
        let (xi, yi) = distort(xn, yn, &self.params, self.model);
        Projection {
            u: 0.5 + xi * self.big / self.width.max(1.0),
            v: 0.5 + yi * self.big / self.height.max(1.0),
            length_m: length,
        }
    }

    /// The source pixel to read for a world point, in pixel-centre coordinates
    /// (`x = u * W - 0.5`).
    ///
    /// For a panorama `u` wraps into `0..W` and `v` is clipped to `0..H-1`,
    /// because the equirectangular seam is only in u: letting v wrap would
    /// paint the sky with the ground. A perspective image has edges, so a point
    /// off it comes back as `None` and the caller marks the texel `OCC_OUTSIDE`
    /// rather than reading a wrapped pixel.
    pub fn pixel(&self, p: Vec3) -> Option<(f32, f32)> {
        let proj = self.project(p);
        if self.model.is_spherical() {
            if !proj.is_valid() {
                return None;
            }
            let u = (proj.u * self.width - 0.5).rem_euclid(self.width);
            let v = (proj.v * self.height - 0.5).clamp(0.0, self.height - 1.0);
            return Some((u as f32, v as f32));
        }
        if !proj.inside_image() {
            return None;
        }
        Some((
            (proj.u * self.width - 0.5) as f32,
            (proj.v * self.height - 0.5) as f32,
        ))
    }

    /// Inverse projection: a normalised pixel to a unit world direction.
    ///
    /// The non-spherical models ignore the radial distortion on the way back,
    /// exactly as the Python does; nothing on the hot path uses this.
    pub fn direction_of_pixel(&self, u: f64, v: f64) -> Vec3 {
        let dc = if self.model.is_spherical() {
            let az = (u - 0.5) * TAU;
            let el = (0.5 - v) * PI;
            let (saz, caz) = az.sin_cos();
            let (sel, cel) = el.sin_cos();
            [saz * cel, -sel, caz * cel]
        } else {
            let (fx, fy, cx0, cy0) = if self.model == CameraModel::Brown && self.params.len() >= 9 {
                (
                    self.params[0],
                    self.params[1],
                    self.params[2],
                    self.params[3],
                )
            } else {
                let f = self.params.first().copied().unwrap_or(0.85);
                (f, f, 0.0, 0.0)
            };
            let xn = ((u - 0.5) * self.width.max(1.0) / self.big - cx0) / fx;
            let yn = ((v - 0.5) * self.height.max(1.0) / self.big - cy0) / fy;
            let d = [xn, yn, 1.0];
            scale(d, 1.0 / norm(d))
        };
        // dc is in camera coordinates and the rows of axes are the camera axes
        // in ENU, so the world direction is the weighted sum of those rows.
        add(
            add(scale(self.axes[0], dc[0]), scale(self.axes[1], dc[1])),
            scale(self.axes[2], dc[2]),
        )
    }
}

/// Convenience for one-off projections. In a loop build a [`Projector`] once
/// instead: this rebuilds it, radial limit scan and all, on every call.
pub fn project_depth(cam: &Camera, p: Vec3) -> Projection {
    Projector::new(cam).project(p)
}

// --------------------------------------------------------------------------- fallback chain

/// Median roll and pitch of the rotated panoramas of the same sequence within
/// `seq_window_s`, with how many there were and the population standard
/// deviation of their roll.
fn sequence_roll_pitch(
    meta: &PanoMeta,
    sequence_metas: &[&PanoMeta],
    params: &Params,
) -> (Option<(f64, f64)>, usize, f64) {
    let mut rolls = Vec::new();
    let mut pitches = Vec::new();
    for m in sequence_metas {
        let Some(rot) = m.rotation else { continue };
        if m.id == meta.id || m.sequence != meta.sequence {
            continue;
        }
        if (m.captured_at - meta.captured_at).abs() as f64 > params.seq_window_s * 1000.0 {
            continue;
        }
        let ax = axes_from_rotation(rot);
        let (r, p) = roll_pitch_of(&ax, heading_of(&ax));
        if r.abs() <= params.roll_max_deg {
            rolls.push(r);
            pitches.push(p);
        }
    }
    let n = rolls.len();
    if n < params.seq_min_panos as usize {
        return (None, n, 0.0);
    }
    let mean = rolls.iter().sum::<f64>() / n as f64;
    let sd = (rolls.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    (Some((median(&mut rolls), median(&mut pitches))), n, sd)
}

/// The median the way numpy takes it: the mean of the two middle values when
/// the count is even.
fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = values.len();
    if n == 0 {
        return f64::NAN;
    }
    if n % 2 == 1 {
        values[n / 2]
    } else {
        0.5 * (values[n / 2 - 1] + values[n / 2])
    }
}

/// `(axes, roll, pitch)` when a rotation passes the sanity gate.
///
/// The roll gates describe a levelled 360 rig. A hand-held phone or a helmet
/// camera legitimately rolls 5 to 35 degrees and its SfM rotation is the pose,
/// so non-spherical images only have to have finite axes.
fn sane_rotation(
    rotvec: Vec3,
    meta: &PanoMeta,
    sequence_metas: &[&PanoMeta],
    params: &Params,
) -> Option<(Mat3, f64, f64)> {
    let axes = axes_from_rotation(rotvec);
    if !axes.iter().all(|row| row.iter().all(|v| v.is_finite())) {
        return None;
    }
    let (roll, pitch) = roll_pitch_of(&axes, heading_of(&axes));
    if !meta.is_spherical() {
        return Some((axes, roll, pitch));
    }
    if roll.abs() > params.roll_max_deg {
        return None;
    }
    let (prior, _, sd) = sequence_roll_pitch(meta, sequence_metas, params);
    if let Some((prior_roll, _)) = prior {
        let sd = sd.max(params.roll_sd_floor_deg);
        if (roll - prior_roll).abs() > params.roll_sd_mult * sd {
            return None;
        }
    }
    Some((axes, roll, pitch))
}

/// One camera from one image record, through the fallback chain.
///
/// SfM rotation from the Graph, then the cluster shot's rotation (the same SfM
/// rotation by another route, so the source stays `sfm`), then the sequence
/// median roll and pitch, then the compass heading alone. The centre comes from
/// `computed_geometry` and `computed_altitude`; `sfm.rs` replaces it with the
/// mapped cluster shot centre later.
pub fn camera_from_meta(
    meta: &PanoMeta,
    frame: &Frame,
    sequence_metas: &[&PanoMeta],
    params: &Params,
    shot_rotation: Option<Vec3>,
) -> Camera {
    let xy = frame.to_enu(meta.lon, meta.lat);
    let centre = [xy[0], xy[1], meta.alt];
    let compass = meta.compass.rem_euclid(360.0);
    // The height prior is per camera class: a 360 rig sits on a car roof or a
    // backpack, a phone or dashcam sits much lower.
    let h_default = params.height_prior(meta.camera_type);
    let mut cam = Camera {
        pano_id: meta.id.clone(),
        centre,
        axes: level_axes(compass, 0.0, 0.0),
        pose_source: PoseSource::Heading,
        roll_deg: 0.0,
        pitch_deg: 0.0,
        ground_z: centre[2] - h_default,
        cam_height_m: h_default,
        ground_source: GroundSource::Default,
        cluster_id: meta.cluster_id.clone(),
        shot_id: None,
        reg: None,
        compass_deg: compass,
        pose_factor: PoseSource::Heading.factor(),
        width: meta.width,
        height: meta.height,
        camera_type: meta.camera_type,
        camera_params: meta.camera_params.clone(),
    };

    for rot in [meta.rotation, shot_rotation].into_iter().flatten() {
        if let Some((axes, roll, pitch)) = sane_rotation(rot, meta, sequence_metas, params) {
            cam.axes = axes;
            cam.roll_deg = roll;
            cam.pitch_deg = pitch;
            cam.pose_source = PoseSource::Sfm;
            cam.pose_factor = PoseSource::Sfm.factor();
            return cam;
        }
    }

    if let (Some((roll, pitch)), _, _) = sequence_roll_pitch(meta, sequence_metas, params) {
        cam.axes = level_axes(compass, roll, pitch);
        cam.roll_deg = roll;
        cam.pitch_deg = pitch;
        cam.pose_source = PoseSource::Sequence;
        cam.pose_factor = PoseSource::Sequence.factor();
    }
    cam
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spherical_camera(axes: Mat3) -> Camera {
        Camera {
            pano_id: "test".into(),
            centre: [0.0, 0.0, 0.0],
            axes,
            pose_source: PoseSource::Sfm,
            roll_deg: 0.0,
            pitch_deg: 0.0,
            ground_z: -2.5,
            cam_height_m: 2.5,
            ground_source: GroundSource::Default,
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

    #[test]
    fn rodrigues_is_orthonormal_and_right_handed() {
        let axes = axes_from_rotation([0.31, -1.2, 0.44]);
        for i in 0..3 {
            assert!((norm(axes[i]) - 1.0).abs() < 1e-12);
            for j in (i + 1)..3 {
                assert!(dot(axes[i], axes[j]).abs() < 1e-12);
            }
        }
        // right x down = forward for a camera with x right, y down, z forward.
        let c = cross(axes[0], axes[1]);
        for i in 0..3 {
            assert!((c[i] - axes[2][i]).abs() < 1e-12);
        }
        // A zero rotation is the identity.
        assert_eq!(axes_from_rotation([0.0, 0.0, 0.0])[1], [0.0, 1.0, 0.0]);
    }

    #[test]
    fn level_axes_and_roll_pitch_are_inverses() {
        for compass in [0.0, 37.5, 180.0, 359.9] {
            for roll in [-12.0, 0.0, 8.25] {
                for pitch in [-20.0, 0.0, 15.0] {
                    let axes = level_axes(compass, roll, pitch);
                    let (r, p) = roll_pitch_of(&axes, compass);
                    assert!((r - roll).abs() < 1e-9, "roll {r} vs {roll}");
                    assert!((p - pitch).abs() < 1e-9, "pitch {p} vs {pitch}");
                    assert!((heading_of(&axes) - compass).abs() < 1e-9);
                }
            }
        }
    }

    #[test]
    fn a_level_camera_puts_north_in_the_centre_and_the_horizon_at_mid_height() {
        let cam = spherical_camera(level_axes(0.0, 0.0, 0.0));
        let p = Projector::new(&cam);
        let north = p.project([0.0, 10.0, 0.0]);
        assert!((north.u - 0.5).abs() < 1e-12);
        assert!((north.v - 0.5).abs() < 1e-12);
        // East is a quarter turn clockwise from north.
        let east = p.project([10.0, 0.0, 0.0]);
        assert!((east.u - 0.75).abs() < 1e-12);
        // Straight up is the top row.
        let up = p.project([0.0, 0.0, 10.0]);
        assert!(up.v.abs() < 1e-12);
        let down = p.project([0.0, 0.0, -10.0]);
        assert!((down.v - 1.0).abs() < 1e-12);
    }

    #[test]
    fn spherical_projection_round_trips_through_the_inverse() {
        let cam = spherical_camera(level_axes(212.0, -6.5, 3.0));
        let p = Projector::new(&cam);
        for &pt in &[
            [12.0, -3.0, 4.0],
            [-8.0, 20.0, -1.5],
            [0.5, 0.25, 9.0],
            [-30.0, -30.0, 0.0],
        ] {
            let proj = p.project(pt);
            let dir = p.direction_of_pixel(proj.u, proj.v);
            let want = scale(pt, 1.0 / norm(pt));
            for i in 0..3 {
                assert!(
                    (dir[i] - want[i]).abs() < 1e-9,
                    "{dir:?} vs {want:?} for {pt:?}"
                );
            }
        }
    }

    #[test]
    fn radial_limit_finds_the_fold_and_caps_when_there_is_none() {
        // A polynomial that never turns: no k at all.
        assert!((radial_limit(CameraModel::Perspective, &[0.85]) - 6.0).abs() < 1e-12);
        // Barrel distortion turns at 1 + 3 k1 r^2 = 0, so r = 1 / sqrt(3 * 0.2).
        let want = 0.9 * (1.0f64 / (3.0 * 0.2)).sqrt();
        let got = radial_limit(CameraModel::Perspective, &[0.85, -0.2, 0.0]);
        // The Python scans the same 0.005 grid, so agreement is to one step.
        assert!((got - want).abs() < 0.9 * 0.005 + 1e-12, "{got} vs {want}");
        // Past the limit there is no projection at all.
        let mut cam = spherical_camera(level_axes(0.0, 0.0, 0.0));
        cam.camera_type = CameraModel::Perspective;
        cam.camera_params = vec![0.85, -0.2, 0.0];
        cam.width = 4000;
        cam.height = 3000;
        let p = Projector::new(&cam);
        // 10 m north is straight ahead; 40 m east of it is well past the fold.
        assert!(p.project([0.0, 10.0, 0.0]).is_valid());
        assert!(!p.project([40.0, 10.0, 0.0]).is_valid());
        // And so is anything behind the camera.
        assert!(!p.project([0.0, -10.0, 0.0]).is_valid());
    }

    #[test]
    fn a_panorama_wraps_u_and_clips_v() {
        let cam = spherical_camera(level_axes(0.0, 0.0, 0.0));
        let p = Projector::new(&cam);
        // Due south is the seam: u is 0 or 1 and must land inside the image.
        let (x, y) = p.pixel([0.0, -10.0, 0.0]).unwrap();
        assert!((0.0..5760.0).contains(&x), "seam column {x}");
        assert!((0.0..=2879.0).contains(&y));
        // The zenith clips to the top row rather than wrapping to the bottom.
        let (_, y) = p.pixel([0.0, 0.0, 10.0]).unwrap();
        assert!((0.0..1.0).contains(&y), "zenith row {y}");
    }

    #[test]
    fn a_perspective_camera_has_edges() {
        let mut cam = spherical_camera(level_axes(0.0, 0.0, 0.0));
        cam.camera_type = CameraModel::Perspective;
        cam.camera_params = vec![0.75, 0.04, -0.05];
        cam.width = 3648;
        cam.height = 2736;
        let p = Projector::new(&cam);
        assert!(p.pixel([0.0, 10.0, 0.0]).is_some());
        // 45 degrees off axis is inside the fold but outside a 0.75 focal
        // sensor, so there is a projection but no pixel.
        let off = p.project([10.0, 10.0, 0.0]);
        assert!(off.is_valid() && !off.inside_image());
        assert!(p.pixel([10.0, 10.0, 0.0]).is_none());
        // 60 degrees off axis is past the fold of this polynomial, so there is
        // no projection at all.
        assert!(!p.project([17.3, 10.0, 0.0]).is_valid());
    }

    // ----------------------------------------------------------------- golden

    use crate::mapillary::golden;
    use crate::mapillary::types::PanoMeta;
    use std::collections::HashMap;

    /// Every camera of the Munich box, built the way `geo.run_geometry` builds
    /// them: the whole run's metas grouped by sequence, no cluster shot
    /// rotation.
    #[test]
    fn golden_cameras_match_the_python_run() {
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
        assert_eq!(metas.len(), 1076, "every record parses");
        assert_eq!(
            metas.iter().filter(|m| m.is_spherical()).count(),
            288,
            "panoramas on the box"
        );
        let mut by_seq: HashMap<&str, Vec<&PanoMeta>> = HashMap::new();
        for m in &metas {
            by_seq.entry(m.sequence.as_str()).or_default().push(m);
        }

        let want: Vec<golden::GoldenCamera> = golden::load("cameras_pose.json");
        let want: HashMap<&str, &golden::GoldenCamera> =
            want.iter().map(|c| (c.id.as_str(), c)).collect();
        assert_eq!(want.len(), metas.len());

        let mut worst_axes = 0.0f64;
        let mut worst_centre = 0.0f64;
        let mut worst_angle = 0.0f64;
        for m in &metas {
            let empty = Vec::new();
            let seq = by_seq.get(m.sequence.as_str()).unwrap_or(&empty);
            let cam = camera_from_meta(m, &frame, seq, &params, None);
            let w = want[m.id.as_str()];
            assert_eq!(cam.pose_source.as_str(), w.pose_source, "{} source", m.id);
            assert_eq!(cam.camera_type.as_str(), w.camera_type, "{} model", m.id);
            assert_eq!((cam.width, cam.height), (w.width, w.height), "{}", m.id);
            // The fixture rounds the intrinsics to 12 decimals to stay small.
            let want_params = w.camera_params.clone().unwrap_or_default();
            assert_eq!(
                cam.camera_params.len(),
                want_params.len(),
                "{} intrinsics",
                m.id
            );
            assert!(
                golden::max_abs_diff(&cam.camera_params, &want_params) < 1e-11,
                "{} intrinsics",
                m.id
            );
            for i in 0..3 {
                worst_axes = worst_axes.max(golden::max_abs_diff(&cam.axes[i], &w.axes[i]));
            }
            worst_centre = worst_centre.max(golden::max_abs_diff(&cam.centre, &w.centre));
            for (got, want) in [
                (cam.roll_deg, w.roll_deg),
                (cam.pitch_deg, w.pitch_deg),
                (cam.compass_deg, w.compass_deg),
                (heading_of(&cam.axes), w.heading_deg),
            ] {
                worst_angle = worst_angle.max((got - want).abs());
            }
            assert!(
                (cam.cam_height_m - w.cam_height_m).abs() < 1e-6,
                "{} height prior",
                m.id
            );
        }
        assert!(worst_axes < 1e-6, "worst rotation entry {worst_axes}");
        assert!(worst_centre < 0.01, "worst camera centre {worst_centre} m");
        assert!(worst_angle < 0.05, "worst angle {worst_angle} deg");
        println!(
            "1076 cameras: worst rotation entry {worst_axes:.2e}, centre {worst_centre:.2e} m, \
             angle {worst_angle:.2e} deg"
        );
    }

    /// The fold guard, model by model, on every distinct camera in the box.
    #[test]
    fn golden_radial_limits_match() {
        if golden::absent() {
            return;
        }

        let want: Vec<golden::GoldenCameraModel> = golden::load("camera_models.json");
        assert_eq!(want.len(), 74);
        let mut worst = 0.0f64;
        for m in &want {
            let model = CameraModel::parse(&m.camera_type);
            let got = radial_limit(model, &m.camera_params);
            worst = worst.max((got - m.radial_limit).abs());
        }
        // Both sides scan the same grid, so this is exact rather than close.
        assert!(worst < 1e-9, "worst radial limit {worst}");
        println!("74 camera models, worst radial limit difference {worst:.2e}");
    }

    /// A projection round trip on a real spherical, perspective and brown
    /// camera: every sample point within a pixel of where the Python put it,
    /// and the spherical inverse map back to the same world direction.
    #[test]
    fn golden_projection_round_trips() {
        if golden::absent() {
            return;
        }

        let cases: Vec<golden::GoldenProjectionCase> = golden::load("projection.json");
        assert_eq!(cases.len(), 3);
        let mut checked = 0usize;
        let mut worst_px = 0.0f64;
        let mut worst_len = 0.0f64;
        let mut worst_dir = 0.0f64;
        for case in &cases {
            let mut cam = spherical_camera(case.axes);
            cam.pano_id = case.camera_id.clone();
            cam.centre = case.centre;
            cam.camera_type = CameraModel::parse(&case.camera_type);
            cam.camera_params = case.camera_params.clone().unwrap_or_default();
            cam.width = case.width;
            cam.height = case.height;
            let p = Projector::new(&cam);
            if let Some(limit) = case.radial_limit {
                let got = radial_limit(cam.camera_type, &cam.camera_params);
                assert!(
                    (got - limit).abs() < 1e-9,
                    "{} radial limit",
                    case.camera_id
                );
            }
            let (w, h) = (f64::from(case.width), f64::from(case.height));
            for point in &case.points {
                let proj = p.project(point.p);
                let Some(want_u) = point.u else {
                    assert!(
                        !proj.is_valid(),
                        "{} projected a point the Python folded away: {point:?} -> {proj:?}",
                        case.camera_id
                    );
                    checked += 1;
                    continue;
                };
                assert!(proj.is_valid(), "{} lost {point:?}", case.camera_id);
                worst_len = worst_len.max((proj.length_m - point.length_m).abs());
                let (px, py) = (proj.u * w, proj.v * h);
                let (want_px, want_py) = (
                    point.px.unwrap_or(want_u * w),
                    point.py.unwrap_or(point.v.unwrap_or(f64::NAN) * h),
                );
                // A panorama's u wraps, so the seam column 0 and column W are the
                // same pixel and a difference of a full width is not an error.
                let d = (px - want_px).abs();
                let dx = if p.is_spherical() {
                    d.min((d - w).abs())
                } else {
                    d
                };
                worst_px = worst_px.max(dx).max((py - want_py).abs());
                assert!(dx < 1.0, "{} column {px} vs {want_px}", case.camera_id);
                assert!(
                    (py - want_py).abs() < 1.0,
                    "{} row {py} vs {want_py}",
                    case.camera_id
                );
                // The inside flag flips at the seam under the fixture's own
                // rounding of the world point, so it is only asserted away from
                // the two edges of the image.
                let near_edge = [proj.u, proj.v]
                    .iter()
                    .any(|c| c.abs() < 1e-6 || (c - 1.0).abs() < 1e-6);
                if !near_edge {
                    assert_eq!(
                        proj.inside_image(),
                        point.inside,
                        "{} inside flag for {point:?}",
                        case.camera_id
                    );
                }
                checked += 1;
            }
            for inv in &case.inverse {
                let dir = p.direction_of_pixel(inv.u, inv.v);
                worst_dir = worst_dir.max(golden::max_abs_diff(&dir, &inv.dir));
                checked += 1;
            }
        }
        assert!(worst_px < 1.0, "worst pixel {worst_px}");
        assert!(worst_len < 1e-4, "worst ray length {worst_len} m");
        assert!(worst_dir < 1e-6, "worst inverse direction {worst_dir}");
        println!(
            "{checked} projection samples: worst pixel {worst_px:.2e}, ray length \
             {worst_len:.2e} m, inverse direction {worst_dir:.2e}"
        );
    }
}
