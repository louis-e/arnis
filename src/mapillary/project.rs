//! Inverse texture mapping from a wall point back into an equirectangular panorama.
//!
//! Because the target is a known plane in world space and the source is a
//! full sphere, no homography, vanishing-point estimation or camera intrinsics
//! are needed: a ray from the camera to the wall point converts straight to a
//! pixel. This is the same ray-cast projection Texture2LoD3 uses in its final
//! stage, with an OSM footprint prism standing in for a CityGML mesh.
//!
//! Coordinates are Arnis world blocks throughout (+X east, +Z south, +Y up).
//! Angles are scale-invariant, so no conversion back to metres is needed as
//! long as all three axes carry the same scale.

use crate::colors::RGBTuple;

/// A camera pose in world-block space.
///
/// Mapillary's delivered equirectangular images are *not* levelled: they are
/// the pixels as uploaded, and the capture rig's tilt is still in them. The
/// SfM pose in `computed_rotation` is what levels them, and applying it is
/// worth a lot. Measured on the lean of near-vertical edges in the rectified
/// facades (0 degrees = correct):
///
/// | area              | heading only         | with `computed_rotation` |
/// |-------------------|----------------------|--------------------------|
/// | Munich, 26 walls  | 5 within 2 deg, sd 7.5 | 19 within 2 deg, sd 3.2 |
/// | Berlin, 128 walls | 38 within 2 deg, sd 8.1 | 99 within 2 deg, sd 3.9 |
///
/// The Munich rigs sat at a consistent -6.5 degrees of roll (every image the
/// same sign), which is the kind of hardware-specific defect this must absorb
/// if it is to work on arbitrary uploads worldwide.
///
/// Convention, settled by the same measurement against three alternatives:
/// `computed_rotation` is an axis-angle vector, world-to-camera, with an ENU
/// world (x east, y north, z up) and a camera with x right, y down, z forward.
/// The rows of its rotation matrix are therefore the camera axes in ENU.
#[derive(Debug, Clone, Copy)]
pub struct CameraPose {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// Degrees clockwise from north, matching Mapillary's `compass_angle`.
    pub compass_angle: f64,
    /// Camera axes (right, down, forward) in world space when the full SfM
    /// orientation is applied; `None` means a level camera at `compass_angle`.
    pub axes: Option<[[f64; 3]; 3]>,
}

impl CameraPose {
    pub fn level(x: f64, y: f64, z: f64, compass_deg: f64) -> Self {
        Self {
            x,
            y,
            z,
            compass_angle: compass_deg,
            axes: None,
        }
    }

    /// Pose from Mapillary's `computed_rotation` (axis-angle, OpenSfM).
    ///
    /// `bearing_offset_deg` is how far the Arnis world is turned relative to
    /// true north, so a `--rotation` world still lines up.
    pub fn oriented(
        x: f64,
        y: f64,
        z: f64,
        compass_deg: f64,
        rotation: [f64; 3],
        bearing_offset_deg: f64,
    ) -> Self {
        let r = rodrigues(rotation);
        // Rows of a world-to-camera matrix are the camera axes in world space.
        let to_world = |v: Vec3| normalize(turn_about_up(enu_to_world(v), bearing_offset_deg));
        Self {
            x,
            y,
            z,
            compass_angle: compass_deg,
            axes: Some([to_world(r[0]), to_world(r[1]), to_world(r[2])]),
        }
    }
}

type Vec3 = [f64; 3];

#[inline]
fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn normalize(v: Vec3) -> Vec3 {
    let n = dot(v, v).sqrt();
    if n < 1e-12 {
        return [0.0, 0.0, 1.0];
    }
    [v[0] / n, v[1] / n, v[2] / n]
}

/// Rotation matrix for an axis-angle vector (Rodrigues' formula).
fn rodrigues(r: Vec3) -> [Vec3; 3] {
    let theta = dot(r, r).sqrt();
    if theta < 1e-12 {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let k = [r[0] / theta, r[1] / theta, r[2] / theta];
    let (s, c) = theta.sin_cos();
    let cross = [[0.0, -k[2], k[1]], [k[2], 0.0, -k[0]], [-k[1], k[0], 0.0]];
    let mut m = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let identity = if i == j { 1.0 } else { 0.0 };
            m[i][j] = identity * c + s * cross[i][j] + (1.0 - c) * k[i] * k[j];
        }
    }
    m
}

/// ENU (east, north, up) to Arnis world (x east, y up, z south).
#[inline]
fn enu_to_world(v: Vec3) -> Vec3 {
    [v[0], v[2], -v[1]]
}

/// Turns a direction clockwise about the vertical axis, for `--rotation` worlds.
#[inline]
fn turn_about_up(v: Vec3, degrees: f64) -> Vec3 {
    if degrees.abs() < f64::EPSILON {
        return v;
    }
    let (s, c) = degrees.to_radians().sin_cos();
    [v[0] * c - v[2] * s, v[1], v[2] * c + v[0] * s]
}

/// Wraps degrees into -180..=180.
#[inline]
fn wrap180(deg: f64) -> f64 {
    let mut d = (deg + 180.0) % 360.0;
    if d < 0.0 {
        d += 360.0;
    }
    d - 180.0
}

/// Bearing in degrees clockwise from north for a world-space XZ offset.
///
/// North is -Z and east is +X in Arnis world space, which is what puts `dx`
/// in the numerator and `-dz` in the denominator.
#[inline]
pub fn bearing_deg(dx: f64, dz: f64) -> f64 {
    dx.atan2(-dz).to_degrees()
}

/// Pixel coordinates in an equirectangular panorama for a world point seen
/// from `cam`, as fractions of width and height in 0.0..1.0.
///
/// The panorama's centre column faces `compass_angle`, its horizontal axis
/// spans 360 degrees and its vertical axis spans 180 (top = zenith).
pub fn project(cam: &CameraPose, x: f64, y: f64, z: f64) -> (f64, f64) {
    let (dx, dy, dz) = (x - cam.x, y - cam.y, z - cam.z);

    if let Some([right, down, forward]) = cam.axes {
        let d = [dx, dy, dz];
        let (cx, cy, cz) = (dot(d, right), dot(d, down), dot(d, forward));
        let u = 0.5 + cx.atan2(cz) / std::f64::consts::TAU;
        let len = (cx * cx + cy * cy + cz * cz).sqrt().max(1e-12);
        let v = 0.5 + (cy / len).clamp(-1.0, 1.0).asin() / std::f64::consts::PI;
        return (u, v);
    }

    let rel_bearing = wrap180(bearing_deg(dx, dz) - cam.compass_angle);
    let u = 0.5 + rel_bearing / 360.0;

    let horizontal = (dx * dx + dz * dz).sqrt();
    let pitch = dy.atan2(horizontal).to_degrees();
    let v = 0.5 - pitch / 180.0;

    (u, v)
}

/// Samples the panorama at a normalised coordinate, wrapping horizontally.
///
/// Returns `None` above or below the sphere's poles, which a wall point can
/// only reach if the pose is badly wrong.
pub fn sample_pixel(pano: &image::RgbImage, u: f64, v: f64) -> Option<RGBTuple> {
    if !(0.0..1.0).contains(&v) || !u.is_finite() {
        return None;
    }
    let (w, h) = (pano.width(), pano.height());

    // Longitude wraps, so u is taken modulo the image width.
    let px = (u.rem_euclid(1.0) * w as f64) as u32 % w;
    let py = ((v * h as f64) as u32).min(h - 1);

    let p = pano.get_pixel(px, py);
    Some((p[0], p[1], p[2]))
}

/// Why a sample was thrown away. Kept separate from a plain bool so the probe
/// report can show which filter is eating a building's coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// Deep shadow or an unlit inner courtyard; carries no usable hue.
    TooDark,
    /// Blown highlight, and the most common way sky leaks past the sky test.
    TooBright,
    /// Street trees, the single biggest occluder of European facades.
    Vegetation,
    /// Open sky above a building shorter than the sampled band.
    Sky,
}

/// Cheap per-pixel occlusion and exposure rejection.
///
/// These are colour heuristics, not segmentation: they are here to keep a
/// tree or a patch of sky from dragging a facade's colour, and they are
/// deliberately conservative because the median across views does the rest of
/// the work. Anything subtler belongs in the Tier 2 segmentation pass.
pub fn classify(color: RGBTuple) -> Result<RGBTuple, Reject> {
    let (r, g, b) = (color.0 as i32, color.1 as i32, color.2 as i32);
    let luma = (299 * r + 587 * g + 114 * b) / 1000;

    if luma < 30 {
        return Err(Reject::TooDark);
    }
    if luma > 245 {
        return Err(Reject::TooBright);
    }
    // Foliage is the only common facade-coloured thing this green.
    if g > r + 12 && g > b + 12 {
        return Err(Reject::Vegetation);
    }
    // Bright and clearly blue-dominant: sky, not a blue-painted wall, which
    // is far less saturated at this lightness.
    if b > r + 25 && b > g + 15 && luma > 150 {
        return Err(Reject::Sky);
    }
    Ok(color)
}

/// Per-channel median of a non-empty sample set.
///
/// Median rather than mean so one unmasked car or window reflection cannot
/// pull the result; channels are taken independently, which is enough for
/// picking a wall colour and avoids sorting in a perceptual space.
pub fn median_color(samples: &mut [RGBTuple]) -> Option<RGBTuple> {
    if samples.is_empty() {
        return None;
    }
    let mid = samples.len() / 2;
    let channel = |extract: fn(&RGBTuple) -> u8| {
        let mut values: Vec<u8> = samples.iter().map(extract).collect();
        values.sort_unstable();
        values[mid]
    };
    Some((channel(|c| c.0), channel(|c| c.1), channel(|c| c.2)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(compass: f64) -> CameraPose {
        CameraPose::level(0.0, 0.0, 0.0, compass)
    }

    #[test]
    fn point_dead_ahead_lands_in_the_image_centre() {
        // Camera faces north (-Z); target is 10 blocks north at eye level.
        let (u, v) = project(&cam(0.0), 0.0, 0.0, -10.0);
        assert!((u - 0.5).abs() < 1e-9, "u={u}");
        assert!((v - 0.5).abs() < 1e-9, "v={v}");
    }

    /// Axis-angle for a level ENU camera looking due north: a quarter turn
    /// about east, which takes world-up to image-up and north to forward.
    const LEVEL_NORTH: [f64; 3] = [std::f64::consts::FRAC_PI_2, 0.0, 0.0];

    fn axes_close(a: [f64; 3], b: [f64; 3], what: &str) {
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 1e-9, "{what}: {a:?} != {b:?}");
        }
    }

    #[test]
    fn a_level_sfm_rotation_projects_like_the_heading_only_pose() {
        let sfm = CameraPose::oriented(0.0, 0.0, 0.0, 0.0, LEVEL_NORTH, 0.0);
        let plain = CameraPose::level(0.0, 0.0, 0.0, 0.0);
        let [right, down, forward] = sfm.axes.unwrap();
        axes_close(right, [1.0, 0.0, 0.0], "right");
        axes_close(down, [0.0, -1.0, 0.0], "down");
        axes_close(forward, [0.0, 0.0, -1.0], "forward");
        // And the two agree on where a point lands.
        for (x, y, z) in [(0.0, 0.0, -10.0), (10.0, 3.0, -4.0), (-3.0, -1.0, 8.0)] {
            let a = project(&sfm, x, y, z);
            let b = project(&plain, x, y, z);
            assert!(
                (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9,
                "{a:?} != {b:?}"
            );
        }
    }

    #[test]
    fn the_bearing_offset_turns_the_pose_with_the_map() {
        // A world generated with a quarter turn puts a north-facing camera east.
        let turned = CameraPose::oriented(0.0, 0.0, 0.0, 90.0, LEVEL_NORTH, 90.0);
        let [_, down, forward] = turned.axes.unwrap();
        axes_close(forward, [1.0, 0.0, 0.0], "forward");
        // Turning the map cannot tilt the camera.
        axes_close(down, [0.0, -1.0, 0.0], "down");
    }

    #[test]
    fn a_pitched_pose_moves_the_horizon_by_its_pitch() {
        // Ten degrees more than the level quarter-turn: the camera looks down.
        let pitched = CameraPose::oriented(
            0.0,
            0.0,
            0.0,
            0.0,
            [std::f64::consts::FRAC_PI_2 + 10f64.to_radians(), 0.0, 0.0],
            0.0,
        );
        let level = CameraPose::level(0.0, 0.0, 0.0, 0.0);
        let (_, v_level) = project(&level, 0.0, 0.0, -10.0);
        let (_, v_pitched) = project(&pitched, 0.0, 0.0, -10.0);
        let shift = (v_pitched - v_level).abs() * 180.0;
        assert!(
            (shift - 10.0).abs() < 0.5,
            "expected ~10 deg of shift, got {shift}"
        );
    }

    #[test]
    fn a_rolled_pose_tilts_verticals_by_its_roll() {
        // Roll about the view axis (north): a point straight up should no
        // longer sit on the centre column.
        let roll = 8f64.to_radians();
        // Compose level-north with a roll about ENU north via a small trick:
        // rotating about the camera's own forward axis is rotating about the
        // ENU y axis here, so add a y component to the axis-angle.
        let rolled = CameraPose::oriented(
            0.0,
            0.0,
            0.0,
            0.0,
            [std::f64::consts::FRAC_PI_2, roll, 0.0],
            0.0,
        );
        let (u_up, _) = project(&rolled, 0.0, 10.0, -0.001);
        assert!(
            (u_up - 0.5).abs() > 0.005,
            "a rolled camera must move the zenith off the centre column, u={u_up}"
        );
    }

    #[test]
    fn bearing_follows_compass_directions() {
        assert!((bearing_deg(0.0, -1.0) - 0.0).abs() < 1e-9, "north");
        assert!((bearing_deg(1.0, 0.0) - 90.0).abs() < 1e-9, "east");
        assert!((bearing_deg(0.0, 1.0).abs() - 180.0).abs() < 1e-9, "south");
        assert!((bearing_deg(-1.0, 0.0) + 90.0).abs() < 1e-9, "west");
    }

    #[test]
    fn rotating_the_camera_shifts_the_column() {
        // Target due east. With the camera facing north it sits a quarter turn
        // right of centre; facing east it moves to the centre.
        let (u_north, _) = project(&cam(0.0), 10.0, 0.0, 0.0);
        assert!((u_north - 0.75).abs() < 1e-9, "u={u_north}");

        let (u_east, _) = project(&cam(90.0), 10.0, 0.0, 0.0);
        assert!((u_east - 0.5).abs() < 1e-9, "u={u_east}");
    }

    #[test]
    fn height_above_the_camera_maps_above_the_horizon() {
        // 10 blocks out, 10 up: 45 degrees, a quarter of the way up the image.
        let (_, v) = project(&cam(0.0), 0.0, 10.0, -10.0);
        assert!((v - 0.25).abs() < 1e-9, "v={v}");
    }

    #[test]
    fn wrap_around_stays_inside_the_image() {
        // Target behind the camera: the seam, which must not fall outside 0..1.
        let (u, _) = project(&cam(0.0), 0.0, 0.0, 10.0);
        let pano = image::RgbImage::new(64, 32);
        assert!(sample_pixel(&pano, u, 0.5).is_some());
    }

    #[test]
    fn filters_catch_foliage_and_sky_but_pass_render() {
        assert_eq!(classify((60, 110, 55)), Err(Reject::Vegetation));
        assert_eq!(classify((150, 190, 240)), Err(Reject::Sky));
        assert_eq!(classify((5, 5, 5)), Err(Reject::TooDark));
        assert_eq!(classify((252, 252, 252)), Err(Reject::TooBright));
        // A plausible plaster facade survives.
        assert_eq!(classify((198, 180, 156)), Ok((198, 180, 156)));
        // So does a genuinely blue-painted wall, which is darker than sky.
        assert_eq!(classify((90, 110, 140)), Ok((90, 110, 140)));
    }

    #[test]
    fn median_ignores_a_single_outlier() {
        // The green outlier loses on every channel to the two wall samples.
        let mut samples = vec![(200, 200, 200), (10, 250, 10), (204, 198, 202)];
        assert_eq!(median_color(&mut samples), Some((200, 200, 200)));
    }

    #[test]
    fn median_of_empty_is_none() {
        assert_eq!(median_color(&mut []), None);
    }
}
