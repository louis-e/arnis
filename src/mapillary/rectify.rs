//! Ortho-rectification by inverse mapping. Port of `tools/facade_lab/rectify.py`.
//!
//! This is where the pipeline earns its keep, and it runs backwards. An OSM
//! footprint edge plus a height is already a rectangle in space, so for every
//! texel of the output rectangle (at `loose_ppm`, `eps_m` in front of the fitted
//! plane) the ray to the camera is formed and the pixel read straight off the
//! image through [`super::pose::Projector`]. Driving the loop from the output
//! means perspective undoes itself: the result is fronto-parallel at true metric
//! scale with no homography and no vanishing point estimation.
//!
//! Two grids come out of here. [`loose_crop`] renders the wall wide and tall at
//! its own pixels per metre so `refine.rs` can still see the roofline, the
//! ground and the neighbours; [`resample_rect`] renders the decided rectangle at
//! the block grid with the lean shear folded into the grid, so the final texture
//! is one resampling of the photograph and never a resampling of a resampling.
//!
//! Every texel also gets an occlusion bit field, because what is in front of the
//! wall must be excluded rather than averaged in:
//!
//! * `OCC_FOOTPRINT`, another building's outline on the plan ray, or the wall's
//!   own footprint shrunk by `max(0.3 m, chord sag + 0.2 m, setback + 0.2 m)`;
//! * `OCC_CLOUD`, the cloud depth map nearer than the wall by more than 1.5 m;
//! * `OCC_NADIR` and `OCC_ZENITH`, the rig and its vehicle, panoramas only,
//!   because on a phone photograph the same band is the ground floor;
//! * `OCC_SEG`, the vegetation mask grown by 0.3 m;
//! * `OCC_OUTSIDE`, a perspective texel that projects off the image. Those are
//!   masked, never wrapped: the projection is only wrapped in u, and only for a
//!   panorama, where the seam really is one.
//!
//! The vegetation rule is ported exactly as it stands, which is **not** the
//! sRGB green ratio `G > 1.15 max(R, B)` the older comments describe. That one
//! was replaced after `experiments/veg_audit.py` measured both on 28 crops: the
//! OkLab rule (operating point "v2b") masks 59 per cent of a tree crop against
//! the green ratio's 27, and 0.9 per cent on average of a tree free control
//! against a much noisier figure. The `a` floors are what keep Munich's cream
//! and ochre render out of it: its hue reaches 105 to 115 degrees in warm light
//! but its `a` stays above -0.015. The leaf texture is measured **over the green
//! pixels only**, so the border of a flat green panel (shutters, a painted wall)
//! does not read as texture and stays wall.
//!
//! Where this differs from the Python, and why:
//!
//! * Sampling reproduces `cv2.remap`'s 1/32 pixel coordinate quantisation
//!   exactly, because that one is systematic, but uses exact bilinear weights
//!   rather than OpenCV's 15 bit fixed point table. The table differs from the
//!   exact weights by under one part in 32768 and its final cast rounds halves
//!   up, so a texel can land one grey level away; the quantisation, left out,
//!   would move the sample by up to half a thirty-second of a pixel, which on a
//!   facade sampled nine source pixels to the texel is worth several levels.
//! * `cv2.cvtColor(RGB2GRAY)` is the 15 bit form
//!   `(9798 R + 19235 G + 3735 B + 16384) >> 15`, not the 14 bit constants
//!   OpenCV's own header names first. The vectorised path is the one it takes
//!   and the two differ by a level on a quarter of a per cent of colours, which
//!   is enough to move a vegetation threshold and a correlation peak; the 15 bit
//!   form was checked against `cv2` on 160 000 random colours and agrees on all
//!   of them.
//! * A negative Shapely buffer is not built. `poly.buffer(-d).intersects(ray)`
//!   asks whether some point of the ray lies at least `d` inside the polygon,
//!   which is answered directly by tracing the ray against the signed distance
//!   to the boundary. That is the exact erosion; Shapely's is an eight segments
//!   per quadrant approximation of it, so the two differ by at most a centimetre
//!   at a reflex corner.
//! * The pano-space outline (`outline_pano`, `LooseCrop.quad_pano`) is not
//!   ported: it is drawn on the Python review sheet and `sheet.py` stays in
//!   Python. Nor are `warp_crop` and `shear_pixel_homography`, which `refine.rs`
//!   already has its own copies of, nor `preview_512`, which belongs to the
//!   visibility stage, nor `car_band_weight`, which `fuse.py` never calls.
//! * `loose_crop` takes no segmentation mask. No caller in the pipeline has one,
//!   so the vegetation rule is the only producer of `OCC_SEG`.
//!
//! One thing the port keeps although it reads oddly: [`choose_ppm`] divides the
//! image width by `2 pi d` for every camera class, which is the pixels per metre
//! of an *equirectangular* image at that distance and means nothing on a phone
//! photograph. It is what the Python does, and it costs nothing: a phone frame
//! carries four times the pixels per radian of a panorama, so the cap at
//! `loose_ppm[1]` applies either way.
//!
//! And one thing the port cannot keep: **JPEG decoding**. The `image` crate does
//! not decode a JPEG to the same bytes as the libjpeg-turbo inside OpenCV. Over
//! the three thumbnails the texture fixture carries, 4 to 11 per cent of channel
//! values come out one to five levels away, a mean absolute difference of 0.06
//! to 0.16, and that is the whole of the difference between this module's crops
//! and the Python's: on the same decoded pixels the vegetation mask differs on
//! 3 of 131 747 texels, all of them on a mask edge. The consequences are
//! measured in `the_texture_stage_over_the_whole_lab_run`.

#![allow(dead_code)]

use image::RgbImage;

use super::imgops::{self, Mask};
use super::plane;
use super::pose::Projector;
use super::sfm::DepthMap;
use super::types::{
    Building, Camera, HeightSource, Params, PlaneFit, Wall, OCC_CLOUD, OCC_FOOTPRINT, OCC_NADIR,
    OCC_OUTSIDE, OCC_SEG, OCC_ZENITH,
};

/// Above this elevation seen from the camera the crop is cut: the rig looks
/// nearly straight up and a texel there is a smear.
pub const MAX_ELEVATION_DEG: f64 = 80.0;
/// How far below the wall foot the loose crop reaches, so the ground row has
/// something to find.
pub const BOTTOM_MARGIN_M: f64 = 1.5;
/// A cloud point has to be this much nearer than the wall to occlude it, which
/// is roughly the registration error plus the plane offset.
pub const CLOUD_OCC_MARGIN_M: f64 = 1.5;
/// The camera is tolerated this far inside a footprint before every ray from it
/// counts as blocked, and the footprint it stands in is shrunk by the same.
pub const LOS_INSIDE_TOL_M: f64 = 1.0;
/// The wall's own footprint is always shrunk by at least this much.
pub const OWN_SHRINK_M: f64 = 0.3;
/// Plan rays are cast to a point this far in front of the wall.
pub const SAMPLE_FRONT_M: f64 = 0.1;

/// The mask agreement the lab measurement holds itself to, a tenth looser than
/// the fixture's because it crosses a JPEG decoder and the vegetation mask grows
/// every disagreement by its 0.3 m dilation. See that test's own comment.
#[cfg(test)]
const LAB_MASK_IOU: f64 = 0.97;

/// Every bit that means "this texel is not the wall".
pub const OCCLUDED_BITS: u8 =
    OCC_FOOTPRINT | OCC_CLOUD | OCC_NADIR | OCC_ZENITH | OCC_SEG | OCC_OUTSIDE;

// Operating point "v2b" of the vegetation sweep. The three colour paths are the
// lit canopy, the weaker greens that need texture to be believed, and the shaded
// canopy, which is dark and almost grey but never flat.
const VEG_HUE_STRONG: [f64; 2] = [112.0, 165.0];
const VEG_CHROMA_STRONG: f64 = 0.05;
const VEG_A_STRONG: f64 = -0.035;
const VEG_STD_STRONG: f64 = 2.0;
const VEG_HUE_WEAK: [f64; 2] = [108.0, 165.0];
const VEG_CHROMA_WEAK: f64 = 0.035;
const VEG_A_WEAK: f64 = -0.025;
const VEG_STD_WEAK: f64 = 8.0;
const VEG_HUE_DARK: [f64; 2] = [115.0, 165.0];
const VEG_CHROMA_DARK: f64 = 0.02;
const VEG_A_DARK: f64 = -0.02;
const VEG_L_DARK: f64 = 0.42;
const VEG_STD_DARK: f64 = 4.0;
const VEG_MEDIAN_PX: usize = 7;
const VEG_BLUR_PX: usize = 7;
const VEG_MIN_AREA_M2: f64 = 0.25;
const VEG_MIN_AREA_PX: f64 = 9.0;
const VEG_DILATE_M: f64 = 0.3;

/// One view's rectified crop of a wall, wider and taller than the wall itself
/// so `refine.rs` can still see the roofline and the ground.
#[derive(Clone, Debug)]
pub struct LooseCrop {
    pub wall_key: String,
    pub pano_id: String,
    pub rgb: image::RgbImage,
    /// Row major occlusion bits, one per texel, `OCC_*` from `types`.
    pub occl: Vec<u8>,
    /// Pixels per metre of this crop.
    pub ppm: f64,
    /// Wall coordinate of column 0 and the height range of the crop.
    pub s0: f64,
    pub h_bot: f64,
    pub h_top: f64,
    /// Column of the wall foot and row of the camera horizon, for `refine.rs`.
    pub x_foot: f64,
    pub y_cam: f64,
    /// The wall foot z in this view's cluster datum. Every `h` in this crop is
    /// measured from it, which is what makes crops from different clusters
    /// comparable.
    pub z_base: f64,
    pub z_base_source: String,
}

impl LooseCrop {
    pub fn s_to_x(&self, s: f64) -> f64 {
        (s - self.s0) * self.ppm
    }

    pub fn h_to_y(&self, h: f64) -> f64 {
        (self.h_top - h) * self.ppm
    }

    pub fn x_to_s(&self, x: f64) -> f64 {
        x / self.ppm + self.s0
    }

    pub fn y_to_h(&self, y: f64) -> f64 {
        self.h_top - y / self.ppm
    }
}

/// What one view brings to its own crop: where the camera is, which plane the
/// wall was fitted to for this view, and the datum every height is measured
/// from.
#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    pub cam: &'a Camera,
    pub fit: &'a PlaneFit,
    /// The wall foot z in this view's cluster datum.
    pub z_base: f64,
    /// Plan distance camera to wall midpoint; `None` computes it.
    pub dist_m: Option<f64>,
    /// The visible span along the wall, used only when no footprints are given.
    pub s_vis: Option<[f64; 2]>,
}

/// The final texture of one view on the decided rectangle.
#[derive(Clone, Debug)]
pub struct RectView {
    pub rgb: RgbImage,
    /// Row major, true where the texel is really this wall seen by this camera.
    pub valid: Vec<bool>,
}

// --------------------------------------------------------------------------- extent and resolution

/// `(s_min, s_max, h_bot, h_top)` of the loose crop in fitted-wall coordinates.
///
/// `s` runs `max(3, 0.3 L)` beyond the provisional ends, `h` from -1.5 m to
/// `max(2.2 h_osm, h_osm + 12)`, and at least `top_margin_default_m` when the
/// height is the 9 m fallback rather than a tag: a wall whose height is a guess
/// needs room above it for the roofline to be found. With a camera the top is
/// cut where the elevation from it would pass 80 degrees.
pub fn crop_extent(
    wall: &Wall,
    fit: &PlaneFit,
    h_osm: f64,
    params: &Params,
    cam: Option<&Camera>,
    z_base: Option<f64>,
    height_source: HeightSource,
) -> [f64; 4] {
    let fw = plane::fitted_wall(wall, fit);
    let length = fw.length;
    let margin = 3.0f64.max(0.3 * length);
    let (s_min, s_max) = (-margin, length + margin);
    let h = if h_osm != 0.0 {
        h_osm
    } else {
        params.default_height_m
    };
    let mut h_top = (params.top_margin[0] * h).max(h + params.top_margin[1]);
    if height_source == HeightSource::Default {
        h_top = h_top.max(params.top_margin_default_m);
    }
    let h_bot = -BOTTOM_MARGIN_M;
    if let (Some(cam), Some(zb)) = (cam, z_base) {
        let (s_cam, h_cam, d_cam) = fw.sh_of(cam.centre, zb);
        let ds = 0.0f64.max(s_min - s_cam).max(s_cam - s_max);
        let d_min = d_cam.hypot(ds);
        let cap = h_cam + d_min.max(0.5) * MAX_ELEVATION_DEG.to_radians().tan();
        h_top = h_top.min(cap);
        h_top = h_top.max(h_bot + 1.0);
    }
    [s_min, s_max, h_bot, h_top]
}

/// The loose crop's resolution: 90 per cent of the image's own pixels per metre
/// at that distance, clamped to `loose_ppm`, and never above the native rate,
/// because upsampling a photograph only invents detail the fusion then has to
/// argue about.
pub fn choose_ppm(dist_m: f64, image_width: u32, params: &Params) -> f64 {
    let d = dist_m.max(0.5);
    let native = f64::from(image_width) / (std::f64::consts::TAU * d);
    let ppm = (0.9 * native).clamp(params.loose_ppm[0], params.loose_ppm[1]);
    ppm.min(native.max(params.loose_ppm[0]))
}

// --------------------------------------------------------------------------- plan geometry

#[inline]
fn dist_point_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (vx, vy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = vx * vx + vy * vy;
    let t = if len2 <= 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * vx + (p[1] - a[1]) * vy) / len2).clamp(0.0, 1.0)
    };
    (p[0] - (a[0] + t * vx)).hypot(p[1] - (a[1] + t * vy))
}

/// Rings of one footprint, exterior first, each one closed by its first point.
fn rings_of(b: &Building) -> impl Iterator<Item = &Vec<[f64; 2]>> {
    std::iter::once(&b.ring).chain(b.holes.iter())
}

fn ring_contains(ring: &[[f64; 2]], p: [f64; 2]) -> bool {
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (yi, yj) = (ring[i][1], ring[j][1]);
        if (yi > p[1]) != (yj > p[1]) {
            let x = ring[i][0] + (p[1] - yi) / (yj - yi) * (ring[j][0] - ring[i][0]);
            if p[0] < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// True when the point is in the filled footprint, holes excluded.
fn polygon_contains(b: &Building, p: [f64; 2]) -> bool {
    if !ring_contains(&b.ring, p) {
        return false;
    }
    !b.holes.iter().any(|h| ring_contains(h, p))
}

/// Distance from a point to the footprint boundary, holes included.
fn boundary_distance(b: &Building, p: [f64; 2]) -> f64 {
    let mut best = f64::INFINITY;
    for ring in rings_of(b) {
        let n = ring.len();
        for i in 0..n {
            let d = dist_point_segment(p, ring[i], ring[(i + 1) % n]);
            if d < best {
                best = d;
            }
        }
    }
    best
}

/// Signed distance to the boundary, positive inside the filled footprint.
#[inline]
fn signed_distance(b: &Building, p: [f64; 2]) -> f64 {
    let d = boundary_distance(b, p);
    if polygon_contains(b, p) {
        d
    } else {
        -d
    }
}

#[inline]
fn segments_cross(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    #[inline]
    fn orient(p: [f64; 2], q: [f64; 2], r: [f64; 2]) -> f64 {
        (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    }
    #[inline]
    fn on_segment(p: [f64; 2], q: [f64; 2], r: [f64; 2]) -> bool {
        q[0] <= p[0].max(r[0])
            && q[0] >= p[0].min(r[0])
            && q[1] <= p[1].max(r[1])
            && q[1] >= p[1].min(r[1])
    }
    let (o1, o2, o3, o4) = (
        orient(a, b, c),
        orient(a, b, d),
        orient(c, d, a),
        orient(c, d, b),
    );
    if (o1 > 0.0) != (o2 > 0.0) && (o3 > 0.0) != (o4 > 0.0) && o1 != 0.0 && o2 != 0.0 {
        return true;
    }
    (o1 == 0.0 && on_segment(a, c, b))
        || (o2 == 0.0 && on_segment(a, d, b))
        || (o3 == 0.0 && on_segment(c, a, d))
        || (o4 == 0.0 && on_segment(c, b, d))
}

/// Shapely's `polygon.intersects(line)`: the segment meets the filled footprint,
/// its boundary counting as part of it.
fn polygon_intersects_segment(b: &Building, p: [f64; 2], q: [f64; 2]) -> bool {
    for ring in rings_of(b) {
        let n = ring.len();
        for i in 0..n {
            if segments_cross(p, q, ring[i], ring[(i + 1) % n]) {
                return true;
            }
        }
    }
    // No crossing, so the segment is either wholly inside the filled area or
    // wholly outside it; one endpoint settles which.
    polygon_contains(b, p)
}

/// Whether `polygon.buffer(-d)` meets the segment, without building the buffer.
///
/// The eroded polygon is the set of points at least `d` inside, so the question
/// is whether the signed distance reaches `d` anywhere on the segment. It is
/// 1-Lipschitz along the ray, which lets the walk jump `d - sd` at a time and
/// still never step over a hit; the 5 cm floor bounds the work on a grazing ray
/// and is finer than Shapely's own buffer approximation.
fn eroded_meets_segment(b: &Building, p: [f64; 2], q: [f64; 2], d: f64) -> bool {
    if d <= 0.0 {
        return polygon_intersects_segment(b, p, q);
    }
    let len = (q[0] - p[0]).hypot(q[1] - p[1]);
    if len <= 0.0 {
        return signed_distance(b, p) >= d;
    }
    let dir = [(q[0] - p[0]) / len, (q[1] - p[1]) / len];
    let mut t = 0.0f64;
    while t <= len {
        let x = [p[0] + t * dir[0], p[1] + t * dir[1]];
        let sd = signed_distance(b, x);
        if sd >= d {
            return true;
        }
        t += (d - sd).max(0.05);
    }
    signed_distance(b, q) >= d
}

/// How far the wall chord dips inside the ring: the largest outward distance of
/// a ring vertex inside the chord's own span. A merged wall is a chord across
/// several ring edges and can leave real masonry outside itself.
fn chord_sag(fw: &Wall, buildings: &[Building], own: &[usize]) -> f64 {
    let mut sag = 0.0f64;
    for &i in own {
        for v in &buildings[i].ring {
            let (s, _, d) = fw.sh_of([v[0], v[1], 0.0], 0.0);
            if s >= -0.05 && s <= fw.length + 0.05 && d > 0.0 && d < 2.0 && d > sag {
                sag = d;
            }
        }
    }
    sag
}

/// Per column of the crop: does the plan ray from the camera to that column
/// cross a footprint before it reaches the wall.
///
/// `setback_m` is how far the fitted plane lies behind the OSM line. Without it
/// a cloud plane two or three metres inside the outline makes every ray cross
/// the wall's own footprint and blanks the crop, although the line of sight gate
/// on the OSM wall passed.
pub fn footprint_columns(
    fw: &Wall,
    cam_centre: [f64; 3],
    s_cols: &[f64],
    buildings: &[Building],
    own_key: &str,
    setback_m: f64,
) -> Vec<bool> {
    let n = s_cols.len();
    let mut out = vec![false; n];
    if buildings.is_empty() || n == 0 {
        return out;
    }
    let c = [cam_centre[0], cam_centre[1]];
    let own: Vec<usize> = buildings
        .iter()
        .enumerate()
        .filter(|(_, b)| b.key == own_key)
        .map(|(i, _)| i)
        .collect();
    // A footprint the camera stands inside is tolerated up to a metre from its
    // edge and shrunk by that metre for the ray test; deeper inside and the view
    // is simply not of this wall.
    let mut lenient: Vec<(usize, f64)> = Vec::new();
    for (i, b) in buildings.iter().enumerate() {
        if !polygon_contains(b, c) {
            continue;
        }
        if boundary_distance(b, c) > LOS_INSIDE_TOL_M {
            return vec![true; n];
        }
        lenient.push((i, LOS_INSIDE_TOL_M + 0.05));
    }
    let shrink_own = OWN_SHRINK_M
        .max(chord_sag(fw, buildings, &own) + 0.2)
        .max(setback_m + 0.2);
    let t = fw.tangent();
    for (k, &s) in s_cols.iter().enumerate() {
        let p = [
            fw.a[0] + s * t[0] + SAMPLE_FRONT_M * fw.n[0],
            fw.a[1] + s * t[1] + SAMPLE_FRONT_M * fw.n[1],
        ];
        let (lo, hi) = (
            [c[0].min(p[0]), c[1].min(p[1])],
            [c[0].max(p[0]), c[1].max(p[1])],
        );
        for (i, b) in buildings.iter().enumerate() {
            if !bbox_overlaps(b, lo, hi) {
                continue;
            }
            let shrink = lenient
                .iter()
                .find(|(j, _)| *j == i)
                .map(|(_, d)| *d)
                .or_else(|| own.contains(&i).then_some(shrink_own));
            let hit = match shrink {
                Some(d) => eroded_meets_segment(b, c, p, d),
                None => polygon_intersects_segment(b, c, p),
            };
            if hit {
                out[k] = true;
                break;
            }
        }
    }
    out
}

fn bbox_overlaps(b: &Building, lo: [f64; 2], hi: [f64; 2]) -> bool {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in &b.ring {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    lo[0] <= x1 && hi[0] >= x0 && lo[1] <= y1 && hi[1] >= y0
}

// --------------------------------------------------------------------------- pixel plumbing

/// Which border rule the sampler follows off the edge of the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Border {
    /// Panoramas: the seam in u is not an edge.
    Wrap,
    /// Perspective images: off the sensor is black, and the caller masks it.
    Constant,
}

/// `numpy.round` on the way to an integer: halves go to the even neighbour.
#[inline]
fn iround(x: f64) -> i64 {
    imgops::round_half_even(x) as i64
}

/// OpenCV's `cvRound`, which is also round half to even.
#[inline]
fn cv_round(x: f32) -> i64 {
    imgops::round_half_even(f64::from(x)) as i64
}

#[inline]
fn wrap_index(i: i64, len: i64) -> i64 {
    i.rem_euclid(len)
}

/// `cv2.remap` with `INTER_LINEAR`: the maps are quantised to 1/32 of a pixel
/// the way OpenCV does, then interpolated with exact weights.
fn remap_bilinear(
    src: &RgbImage,
    map_u: &[f32],
    map_v: &[f32],
    w: usize,
    h: usize,
    border: Border,
) -> RgbImage {
    let (sw, sh) = (src.width() as i64, src.height() as i64);
    let mut out = RgbImage::new(w as u32, h as u32);
    let raw = src.as_raw();
    let stride = src.width() as usize * 3;
    let fetch = |x: i64, y: i64, c: usize| -> f64 {
        let (mut x, mut y) = (x, y);
        match border {
            Border::Wrap => {
                x = wrap_index(x, sw);
                y = wrap_index(y, sh);
            }
            Border::Constant => {
                if x < 0 || y < 0 || x >= sw || y >= sh {
                    return 0.0;
                }
            }
        }
        f64::from(raw[y as usize * stride + x as usize * 3 + c])
    };
    for i in 0..w * h {
        let fx = cv_round(map_u[i] * 32.0);
        let fy = cv_round(map_v[i] * 32.0);
        // saturate_cast<short>, which is what puts a -1e6 map value far enough
        // out that every tap of the 2x2 block is off the image.
        let sx = (fx >> 5).clamp(-32768, 32767);
        let sy = (fy >> 5).clamp(-32768, 32767);
        let ax = (fx & 31) as f64 / 32.0;
        let ay = (fy & 31) as f64 / 32.0;
        let (wx0, wx1) = (1.0 - ax, ax);
        let (wy0, wy1) = (1.0 - ay, ay);
        let px = out.get_pixel_mut((i % w) as u32, (i / w) as u32);
        for c in 0..3 {
            let v = fetch(sx, sy, c) * wy0 * wx0
                + fetch(sx + 1, sy, c) * wy0 * wx1
                + fetch(sx, sy + 1, c) * wy1 * wx0
                + fetch(sx + 1, sy + 1, c) * wy1 * wx1;
            // OpenCV's fixed point cast rounds halves up and saturates.
            px.0[c] = (v + 0.5).floor().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// The inverse maps of a grid of world points, in the pixel-centre convention
/// `x = u W - 0.5`.
///
/// For a panorama `u` wraps into `0..W` and `v` is clipped to `0..H-1`, because
/// the equirectangular seam is only in u: letting v wrap would paint the sky
/// with the ground. For a perspective camera anything off the sensor is written
/// far outside so the constant border leaves it black and the caller masks it.
fn pixel_maps(proj: &Projector, points: &[[f64; 3]], width: u32, height: u32) -> PixelMaps {
    let (w, h) = (f64::from(width), f64::from(height));
    let n = points.len();
    let (mut mu, mut mv, mut ray) = (
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
    );
    let spherical = proj.is_spherical();
    for p in points {
        let pr = proj.project(*p);
        ray.push(pr.length_m as f32);
        if spherical {
            mu.push((pr.u * w - 0.5).rem_euclid(w) as f32);
            mv.push((pr.v * h - 0.5).clamp(0.0, h - 1.0) as f32);
        } else if pr.inside_image() {
            mu.push((pr.u * w - 0.5) as f32);
            mv.push((pr.v * h - 0.5) as f32);
        } else {
            mu.push(-1e6);
            mv.push(-1e6);
        }
    }
    PixelMaps { u: mu, v: mv, ray }
}

// --------------------------------------------------------------------------- vegetation

/// A box filter with OpenCV's default reflect-101 border, normalised.
fn box_blur(src: &[f64], w: usize, h: usize, k: usize) -> Vec<f64> {
    let r = (k / 2) as isize;
    let mut out = vec![0.0; w * h];
    let reflect = |i: isize, n: isize| -> usize {
        if n == 1 {
            return 0;
        }
        let period = 2 * (n - 1);
        let mut j = i.rem_euclid(period);
        if j >= n {
            j = period - j;
        }
        j as usize
    };
    // Horizontal then vertical, which is what a separable box filter is; the
    // sums are in f64 so the order does not matter.
    let mut tmp = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for d in -r..=r {
                s += src[y * w + reflect(x as isize + d, w as isize)];
            }
            tmp[y * w + x] = s;
        }
    }
    let norm = (k * k) as f64;
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for d in -r..=r {
                s += tmp[reflect(y as isize + d, h as isize) * w + x];
            }
            out[y * w + x] = s / norm;
        }
    }
    out
}

/// `cv2.cvtColor(RGB2GRAY)` on bytes: the 15 bit fixed point weights of the
/// vectorised path, which is the one OpenCV 4 actually takes. The 14 bit
/// constants in its own header are a rounding away on a quarter of a per cent
/// of pixels, which was measured over 160 000 random colours.
#[inline]
fn rgb_to_gray(p: [u8; 3]) -> f64 {
    let acc = i64::from(p[0]) * 9798 + i64::from(p[1]) * 19235 + i64::from(p[2]) * 3735 + 16384;
    (acc >> 15) as f64
}

/// `cv2.getStructuringElement(MORPH_ELLIPSE, (2r+1, 2r+1))`: per row, the run of
/// columns inside the disc, with OpenCV's own rounding of the half width.
fn ellipse_offsets(r: i64) -> Vec<(i64, i64)> {
    let mut spans = Vec::new();
    for dy in -r..=r {
        let dx = imgops::round_half_even((((r * r - dy * dy) as f64).max(0.0)).sqrt()) as i64;
        spans.push((dy, dx));
    }
    spans
}

fn dilate_ellipse(m: &Mask, r: i64) -> Mask {
    let spans = ellipse_offsets(r);
    let mut out = Mask::new(m.w, m.h);
    for y in 0..m.h as i64 {
        for x in 0..m.w as i64 {
            let mut hit = false;
            'k: for &(dy, dx) in &spans {
                for sx in (x - dx)..=(x + dx) {
                    if m.get(sx as isize, (y + dy) as isize) {
                        hit = true;
                        break 'k;
                    }
                }
            }
            out.set(x as usize, y as usize, hit);
        }
    }
    out
}

/// Eight-connected components, which is what the vegetation rule asks OpenCV
/// for. `imgops::connected_components` is four-connected because that is what
/// `openings.py` needs, and a leaf mask joined only at the corners would
/// otherwise fall apart into pieces too small to keep.
fn components8(m: &Mask) -> (Vec<u32>, Vec<usize>) {
    let n = m.w * m.h;
    let mut label = vec![0u32; n];
    let mut areas: Vec<usize> = vec![0];
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..n {
        if !m.bits[start] || label[start] != 0 {
            continue;
        }
        let id = areas.len() as u32;
        let mut area = 0usize;
        label[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            area += 1;
            let (x, y) = ((i % m.w) as isize, (i / m.w) as isize);
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx as usize >= m.w || ny as usize >= m.h {
                        continue;
                    }
                    let j = ny as usize * m.w + nx as usize;
                    if m.bits[j] && label[j] == 0 {
                        label[j] = id;
                        stack.push(j);
                    }
                }
            }
        }
        areas.push(area);
    }
    (label, areas)
}

/// Foliage in OkLab, grown by 0.3 m so leaf edges and the half covered texels at
/// the crown border go with it.
///
/// Hue between yellow-green and blue-green (Munich's pale render sits at 85 to
/// 105 degrees, sky at 200 to 240), chroma above a floor, a negative `a`, and
/// for the weaker colours local texture energy, because leaves are never flat.
/// A separate path takes the shaded canopy, which is dark and barely coloured
/// but still textured.
pub fn vegetation_mask(rgb: &RgbImage, ppm: f64) -> Vec<bool> {
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let n = w * h;
    if n == 0 {
        return Vec::new();
    }
    let mut colour = vec![0u8; n]; // bit 0 strong, bit 1 weak, bit 2 dark
    let mut gray = vec![0.0f64; n];
    for (i, px) in rgb.pixels().enumerate() {
        let lab = imgops::srgb_to_oklab(px.0);
        let (l, a, b) = (lab[0], lab[1], lab[2]);
        let chroma = a.hypot(b);
        let hue = b.atan2(a).to_degrees().rem_euclid(360.0);
        gray[i] = rgb_to_gray(px.0);
        let mut bits = 0u8;
        if a < VEG_A_STRONG
            && hue > VEG_HUE_STRONG[0]
            && hue < VEG_HUE_STRONG[1]
            && chroma > VEG_CHROMA_STRONG
        {
            bits |= 1;
        }
        if a < VEG_A_WEAK
            && hue > VEG_HUE_WEAK[0]
            && hue < VEG_HUE_WEAK[1]
            && chroma > VEG_CHROMA_WEAK
        {
            bits |= 2;
        }
        if a < VEG_A_DARK
            && hue > VEG_HUE_DARK[0]
            && hue < VEG_HUE_DARK[1]
            && chroma > VEG_CHROMA_DARK
            && l < VEG_L_DARK
        {
            bits |= 4;
        }
        colour[i] = bits;
    }
    // The texture is measured over the green pixels only, so the border of a
    // flat green panel does not read as texture.
    let weight: Vec<f64> = colour.iter().map(|c| f64::from(*c != 0)).collect();
    let wg: Vec<f64> = (0..n).map(|i| gray[i] * weight[i]).collect();
    let wg2: Vec<f64> = (0..n).map(|i| gray[i] * gray[i] * weight[i]).collect();
    let den = box_blur(&weight, w, h, VEG_BLUR_PX);
    let mean = box_blur(&wg, w, h, VEG_BLUR_PX);
    let sq = box_blur(&wg2, w, h, VEG_BLUR_PX);
    let mut hit = Mask::new(w, h);
    for i in 0..n {
        if colour[i] == 0 {
            continue;
        }
        let d = den[i].max(1e-3);
        let m = mean[i] / d;
        let s = (sq[i] / d - m * m).max(0.0).sqrt();
        let on = (colour[i] & 1 != 0 && s > VEG_STD_STRONG)
            || (colour[i] & 2 != 0 && s > VEG_STD_WEAK)
            || (colour[i] & 4 != 0 && s > VEG_STD_DARK);
        hit.bits[i] = on;
    }
    let bytes: Vec<u8> = hit.bits.iter().map(|b| if *b { 255 } else { 0 }).collect();
    let smoothed = imgops::median_filter_u8(&bytes, w, h, VEG_MEDIAN_PX);
    let mut kept = Mask::from_bits(w, h, smoothed.iter().map(|v| *v > 0).collect());
    let (labels, areas) = components8(&kept);
    let min_area = VEG_MIN_AREA_PX.max(VEG_MIN_AREA_M2 * ppm * ppm);
    for (bit, id) in kept.bits.iter_mut().zip(labels.iter()) {
        let id = *id as usize;
        *bit = id != 0 && (areas[id] as f64) >= min_area;
    }
    let r = iround(VEG_DILATE_M * ppm);
    if r > 0 {
        kept = dilate_ellipse(&kept, r);
    }
    kept.bits
}

// --------------------------------------------------------------------------- occlusion

/// The inverse maps of one grid: where every texel reads from and how far away
/// it is.
struct PixelMaps {
    u: Vec<f32>,
    v: Vec<f32>,
    ray: Vec<f32>,
}

/// The occlusion bits a texel earns from the projection alone: the rig band, the
/// image border and the cloud depth.
fn occlusion_bits(
    maps: &PixelMaps,
    image: (u32, u32),
    grid: (usize, usize),
    depth: Option<&DepthMap>,
    params: &Params,
    spherical: bool,
) -> Vec<u8> {
    let (map_u, map_v, ray) = (&maps.u, &maps.v, &maps.ray);
    let (w, h) = grid;
    let n = w * h;
    let mut occl = vec![0u8; n];
    let hp = f64::from(image.1);
    let wp = f64::from(image.0);
    let outside: Vec<bool> = (0..n).map(|i| map_u[i] < -1.0 || map_v[i] < -1.0).collect();
    if spherical {
        for i in 0..n {
            let v = (f64::from(map_v[i]) + 0.5) / hp;
            if v > params.v_range[1] {
                occl[i] |= OCC_NADIR;
            }
            if v < params.v_range[0] {
                occl[i] |= OCC_ZENITH;
            }
        }
    } else if outside.iter().any(|b| *b) {
        // Grown by two pixels so a line segment along the image border carries
        // the bit as well as the border itself.
        let grown = imgops::dilate(&Mask::from_bits(w, h, outside.clone()), 5, 5);
        for (bits, on) in occl.iter_mut().zip(grown.bits.iter()) {
            if *on {
                *bits |= OCC_OUTSIDE;
            }
        }
    }
    if let Some(d) = depth {
        let (dw, dh) = (i64::from(d.width), i64::from(d.height));
        for i in 0..n {
            let u = (f64::from(map_u[i]) + 0.5) / wp;
            let v = ((f64::from(map_v[i]) + 0.5) / hp).clamp(0.0, 1.0);
            let col = ((u * dw as f64) as i64).clamp(0, dw - 1);
            let row = ((v * dh as f64) as i64).clamp(0, dh - 1);
            let dz = d.at(col as u32, row as u32);
            if dz.is_finite() && dz < ray[i] - CLOUD_OCC_MARGIN_M as f32 && !outside[i] {
                occl[i] |= OCC_CLOUD;
            }
        }
    }
    occl
}

// --------------------------------------------------------------------------- the loose crop

/// The loose rectified crop of one wall from one registered camera.
///
/// The grid is the fitted plane, not the raw OSM line: `eps_m` in front of it,
/// row 0 at `h_top`, column 0 at `s_min`, sampled at pixel centres. Every height
/// is metres above `view.z_base`, which is what lets crops from different
/// clusters be compared at all.
pub fn loose_crop(
    wall: &Wall,
    view: &View<'_>,
    image: &RgbImage,
    depth: Option<&DepthMap>,
    buildings: &[Building],
    params: &Params,
) -> LooseCrop {
    let cam = view.cam;
    let zb = if view.z_base.is_finite() {
        view.z_base
    } else {
        cam.centre[2] - cam.cam_height_m
    };
    let fw = plane::fitted_wall(wall, view.fit);
    let h_osm = wall.height_osm.unwrap_or(params.default_height_m);
    let extent = crop_extent(
        wall,
        view.fit,
        h_osm,
        params,
        Some(cam),
        Some(zb),
        wall.height_source,
    );
    let [s_min, s_max, h_bot, h_top] = extent;
    let mid = fw.midpoint();
    let dist_m = view
        .dist_m
        .unwrap_or_else(|| (cam.centre[0] - mid[0]).hypot(cam.centre[1] - mid[1]));
    let ppm = choose_ppm(dist_m, image.width(), params);
    let w = (iround((s_max - s_min) * ppm).max(1)) as usize;
    let h = (iround((h_top - h_bot) * ppm).max(1)) as usize;
    let proj = Projector::new(cam);
    let mut points = Vec::with_capacity(w * h);
    for y in 0..h {
        let hh = h_top - (y as f64 + 0.5) / ppm;
        for x in 0..w {
            let s = s_min + (x as f64 + 0.5) / ppm;
            points.push(fw.point(s, hh, zb, params.eps_m));
        }
    }
    let maps = pixel_maps(&proj, &points, image.width(), image.height());
    let border = if cam.is_spherical() {
        Border::Wrap
    } else {
        Border::Constant
    };
    let rgb = remap_bilinear(image, &maps.u, &maps.v, w, h, border);
    let mut occl = occlusion_bits(
        &maps,
        (image.width(), image.height()),
        (w, h),
        depth,
        params,
        cam.is_spherical(),
    );
    let s_cols: Vec<f64> = (0..w).map(|x| s_min + (x as f64 + 0.5) / ppm).collect();
    if !buildings.is_empty() {
        let setback = (-plane::offset_of(view.fit, wall)).max(0.0);
        let blocked = footprint_columns(
            &fw,
            cam.centre,
            &s_cols,
            buildings,
            &wall.building_key,
            setback,
        );
        for y in 0..h {
            for x in 0..w {
                if blocked[x] {
                    occl[y * w + x] |= OCC_FOOTPRINT;
                }
            }
        }
    } else if let Some(sv) = view.s_vis {
        for y in 0..h {
            for x in 0..w {
                if s_cols[x] < sv[0] || s_cols[x] > sv[1] {
                    occl[y * w + x] |= OCC_FOOTPRINT;
                }
            }
        }
    }
    for (i, veg) in vegetation_mask(&rgb, ppm).into_iter().enumerate() {
        if veg {
            occl[i] |= OCC_SEG;
        }
    }
    let (s_cam, h_cam, _) = fw.sh_of(cam.centre, zb);
    LooseCrop {
        wall_key: wall.key.clone(),
        pano_id: cam.pano_id.clone(),
        rgb,
        occl,
        ppm,
        s0: s_min,
        h_bot,
        h_top,
        x_foot: (s_cam - s_min) * ppm,
        y_cam: (h_top - h_cam) * ppm,
        z_base: zb,
        z_base_source: "pano".to_string(),
    }
}

// --------------------------------------------------------------------------- the final texture

/// The inverse of a 3x3 homography.
fn invert3(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let a = m;
    let c = [
        [
            a[1][1] * a[2][2] - a[1][2] * a[2][1],
            a[0][2] * a[2][1] - a[0][1] * a[2][2],
            a[0][1] * a[1][2] - a[0][2] * a[1][1],
        ],
        [
            a[1][2] * a[2][0] - a[1][0] * a[2][2],
            a[0][0] * a[2][2] - a[0][2] * a[2][0],
            a[0][2] * a[1][0] - a[0][0] * a[1][2],
        ],
        [
            a[1][0] * a[2][1] - a[1][1] * a[2][0],
            a[0][1] * a[2][0] - a[0][0] * a[2][1],
            a[0][0] * a[1][1] - a[0][1] * a[1][0],
        ],
    ];
    let det = a[0][0] * c[0][0] + a[0][1] * c[1][0] + a[0][2] * c[2][0];
    let inv = if det.abs() < 1e-300 { 0.0 } else { 1.0 / det };
    let mut out = [[0.0f64; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = c[i][j] * inv;
        }
    }
    out
}

#[inline]
fn apply_h(m: &[[f64; 3]; 3], s: f64, h: f64) -> (f64, f64) {
    let mut w = m[2][0] * s + m[2][1] * h + m[2][2];
    if w.abs() < 1e-12 {
        w = 1e-12;
    }
    (
        (m[0][0] * s + m[0][1] * h + m[0][2]) / w,
        (m[1][0] * s + m[1][1] * h + m[1][2]) / w,
    )
}

/// The final texture straight from the photograph onto the decided rectangle.
///
/// `rect` is `(s_l, s_r, h_lo, h_hi)` in corrected wall coordinates and the
/// texture is exactly `ppb cols` by `ppb rows`, the rounding residual under half
/// a metre spread over the width and the height. `h_shear` maps raw `(s, h)` to
/// corrected `(s', h')`; its inverse is applied to every texel before the plane
/// grid, so the lean correction costs no second resampling.
///
/// A texel is valid when the sampled v is inside the rig band (panorama) or the
/// texel is on the sensor (perspective) and, with a crop, when the corresponding
/// loose-crop pixel carries no occlusion bit.
#[allow(clippy::too_many_arguments)]
pub fn resample_rect(
    wall: &Wall,
    view: &View<'_>,
    image: &RgbImage,
    rect: [f64; 4],
    h_shear: Option<&[[f64; 3]; 3]>,
    ppb: u32,
    crop: Option<&LooseCrop>,
    params: &Params,
) -> RectView {
    let [s_l, s_r, h_lo, h_hi] = rect;
    let cols = iround(s_r - s_l).max(1) as usize;
    let rows = iround(h_hi - h_lo).max(1) as usize;
    let w = cols * ppb as usize;
    let h = rows * ppb as usize;
    let fw = plane::fitted_wall(wall, view.fit);
    let inv = h_shear.map(invert3);
    let mut points = Vec::with_capacity(w * h);
    let mut raw = Vec::with_capacity(w * h);
    for y in 0..h {
        let hh = h_hi - (y as f64 + 0.5) * (h_hi - h_lo) / h as f64;
        for x in 0..w {
            let s = s_l + (x as f64 + 0.5) * (s_r - s_l) / w as f64;
            let (sr, hr) = match &inv {
                Some(m) => apply_h(m, s, hh),
                None => (s, hh),
            };
            raw.push((sr, hr));
            points.push(fw.point(sr, hr, view.z_base, params.eps_m));
        }
    }
    let proj = Projector::new(view.cam);
    let maps = pixel_maps(&proj, &points, image.width(), image.height());
    let border = if view.cam.is_spherical() {
        Border::Wrap
    } else {
        Border::Constant
    };
    let rgb = remap_bilinear(image, &maps.u, &maps.v, w, h, border);
    let hp = f64::from(image.height());
    let mut valid: Vec<bool> = (0..w * h)
        .map(|i| {
            if view.cam.is_spherical() {
                let v = (f64::from(maps.v[i]) + 0.5) / hp;
                v >= params.v_range[0] && v <= params.v_range[1]
            } else {
                maps.u[i] > -1.0 && maps.v[i] > -1.0
            }
        })
        .collect();
    if let Some(c) = crop {
        let (cw, ch) = (c.rgb.width() as f64, c.rgb.height() as f64);
        for (i, (sr, hr)) in raw.iter().enumerate() {
            if !valid[i] {
                continue;
            }
            let x = c.s_to_x(*sr);
            let y = c.h_to_y(*hr);
            let inside = x >= 0.0 && x < cw && y >= 0.0 && y < ch;
            let cx = (x.floor() as i64).clamp(0, cw as i64 - 1) as usize;
            let cy = (y.floor() as i64).clamp(0, ch as i64 - 1) as usize;
            let occ = c.occl[cy * c.rgb.width() as usize + cx];
            valid[i] = inside && (occ & OCCLUDED_BITS) == 0;
        }
    }
    RectView { rgb, valid }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapillary::types::{CameraModel, GroundSource, HeightSource, OsmKind, PoseSource};
    use std::collections::BTreeMap;

    fn wall_10m() -> Wall {
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

    fn osm_fit(wall: &Wall) -> PlaneFit {
        plane::fallback_plane(wall, false, None, None)
    }

    fn square(key: &str, x0: f64, y0: f64, size: f64) -> Building {
        Building {
            key: key.into(),
            osm_id: 1,
            kind: OsmKind::Way,
            ring: vec![
                [x0, y0],
                [x0 + size, y0],
                [x0 + size, y0 + size],
                [x0, y0 + size],
            ],
            holes: vec![],
            node_ids: vec![1, 2, 3, 4],
            tags: BTreeMap::new(),
            height_osm: None,
            height_source: HeightSource::Default,
            min_height: 0.0,
            target: true,
            member_ways: vec![],
        }
    }

    fn spherical_camera(centre: [f64; 3]) -> Camera {
        Camera {
            pano_id: "p".into(),
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
            width: 512,
            height: 256,
            camera_type: CameraModel::Spherical,
            camera_params: vec![],
        }
    }

    #[test]
    fn the_extent_reaches_past_both_ends_and_over_the_roof() {
        let wall = wall_10m();
        let fit = osm_fit(&wall);
        let p = Params::default();
        let e = crop_extent(&wall, &fit, 12.0, &p, None, None, HeightSource::Tag);
        // max(3, 0.3 * 10) = 3 m beyond each end, and max(2.2 * 12, 12 + 12).
        assert!((e[0] + 3.0).abs() < 1e-12 && (e[1] - 13.0).abs() < 1e-12);
        assert!((e[2] + 1.5).abs() < 1e-12 && (e[3] - 26.4).abs() < 1e-12);
        // A defaulted height gets the 30 m ceiling instead.
        let d = crop_extent(&wall, &fit, 9.0, &p, None, None, HeightSource::Default);
        assert!((d[3] - 30.0).abs() < 1e-12);
    }

    #[test]
    fn the_elevation_cap_cuts_a_crop_taken_from_close_up() {
        let wall = wall_10m();
        let fit = osm_fit(&wall);
        let p = Params::default();
        let cam = spherical_camera([5.0, -3.0, 2.5]);
        let e = crop_extent(
            &wall,
            &fit,
            12.0,
            &p,
            Some(&cam),
            Some(0.0),
            HeightSource::Tag,
        );
        // 2.5 m up and 3 m out, so 80 degrees off the horizon caps the top.
        let cap = 2.5 + 3.0 * MAX_ELEVATION_DEG.to_radians().tan();
        assert!((e[3] - cap).abs() < 1e-9, "{} against {cap}", e[3]);
    }

    #[test]
    fn the_resolution_never_upsamples_the_photograph() {
        let p = Params::default();
        // 5760 px over 2 pi at 12 m is 76 px per metre, so the cap applies.
        assert!((choose_ppm(12.0, 5760, &p) - p.loose_ppm[1]).abs() < 1e-12);
        // A 512 px thumbnail at 30 m carries 2.7 px per metre, under the floor,
        // and the floor is what is allowed rather than the invented 6.
        let native = 512.0 / (std::f64::consts::TAU * 30.0);
        assert!(native < p.loose_ppm[0]);
        assert!((choose_ppm(30.0, 512, &p) - p.loose_ppm[0]).abs() < 1e-12);
    }

    #[test]
    fn a_building_in_the_way_blocks_the_columns_behind_it() {
        let wall = wall_10m();
        let fit = osm_fit(&wall);
        let fw = plane::fitted_wall(&wall, &fit);
        // The camera looks at the wall from the south; a shed sits in front of
        // the middle of it.
        let buildings = vec![square("w1", 0.0, 0.0, 10.0), square("w2", 4.0, -6.0, 2.0)];
        let s_cols: Vec<f64> = (0..21).map(|i| i as f64 * 0.5).collect();
        let blocked = footprint_columns(&fw, [5.0, -20.0, 2.5], &s_cols, &buildings, "w1", 0.0);
        assert!(
            blocked[10],
            "the column straight behind the shed is blocked"
        );
        assert!(!blocked[0] && !blocked[20], "the ends stay clear");
    }

    #[test]
    fn a_camera_deep_inside_a_footprint_sees_nothing() {
        let wall = wall_10m();
        let fit = osm_fit(&wall);
        let fw = plane::fitted_wall(&wall, &fit);
        let buildings = vec![square("w2", -20.0, -20.0, 20.0)];
        let s_cols: Vec<f64> = (0..5).map(|i| i as f64).collect();
        let blocked = footprint_columns(&fw, [-10.0, -10.0, 2.0], &s_cols, &buildings, "w1", 0.0);
        assert!(blocked.iter().all(|b| *b));
    }

    #[test]
    fn the_erosion_test_agrees_with_a_shrunk_square() {
        let b = square("w1", 0.0, 0.0, 10.0);
        // A chord across the middle is 5 m inside at its deepest.
        assert!(eroded_meets_segment(&b, [-1.0, 5.0], [11.0, 5.0], 4.9));
        assert!(!eroded_meets_segment(&b, [-1.0, 5.0], [11.0, 5.0], 5.1));
        // A ray that only clips the corner is not 1 m inside anywhere.
        assert!(!eroded_meets_segment(&b, [-1.0, 0.5], [0.5, -1.0], 1.0));
    }

    #[test]
    fn a_green_textured_patch_is_foliage_and_a_flat_green_panel_is_not() {
        let (w, h) = (64usize, 64usize);
        let mut img = RgbImage::from_pixel(w as u32, h as u32, image::Rgb([200, 190, 170]));
        for y in 8..40u32 {
            for x in 8..40u32 {
                // A leafy green with the light broken up by the leaves.
                let n = if (x / 2 + y / 2) % 2 == 0 { 30 } else { 0 };
                img.put_pixel(x, y, image::Rgb([40 + n, 90 + n, 30 + n]));
            }
        }
        for y in 44..60u32 {
            for x in 44..60u32 {
                img.put_pixel(x, y, image::Rgb([40, 90, 30]));
            }
        }
        let m = vegetation_mask(&img, 8.0);
        assert!(m[24 * w + 24], "the textured canopy is masked");
        assert!(!m[52 * w + 52], "the flat panel is not");
    }

    #[test]
    fn the_sampler_reads_the_pixel_the_map_points_at() {
        let mut src = RgbImage::new(4, 4);
        for y in 0..4u32 {
            for x in 0..4u32 {
                src.put_pixel(x, y, image::Rgb([(x * 40) as u8, (y * 40) as u8, 0]));
            }
        }
        // Exactly on a pixel centre, and halfway between two of them.
        let out = remap_bilinear(&src, &[1.0, 1.5], &[2.0, 2.0], 2, 1, Border::Constant);
        assert_eq!(out.get_pixel(0, 0).0, [40, 80, 0]);
        assert_eq!(out.get_pixel(1, 0).0, [60, 80, 0]);
        // Off the image with a constant border is black.
        let off = remap_bilinear(&src, &[-1e6], &[-1e6], 1, 1, Border::Constant);
        assert_eq!(off.get_pixel(0, 0).0, [0, 0, 0]);
        // And with a wrapping border the right neighbour of the last column is
        // the first one.
        let seam = remap_bilinear(&src, &[3.5], &[0.0], 1, 1, Border::Wrap);
        assert_eq!(seam.get_pixel(0, 0).0, [60, 0, 0]);
    }

    #[test]
    fn a_wall_seen_head_on_comes_back_upright() {
        // A synthetic panorama with a bright stripe where the wall's left half
        // is: the rectified crop must have the stripe on its left half too.
        let (pw, ph) = (1024u32, 512u32);
        let mut pano = RgbImage::from_pixel(pw, ph, image::Rgb([30, 30, 30]));
        let wall = wall_10m();
        let fit = osm_fit(&wall);
        let cam = spherical_camera([5.0, -12.0, 2.5]);
        let proj = Projector::new(&cam);
        for x in 0..pw {
            for y in 0..ph {
                let u = (f64::from(x) + 0.5) / f64::from(pw);
                let v = (f64::from(y) + 0.5) / f64::from(ph);
                let d = proj.direction_of_pixel(u, v);
                // Where the ray meets the wall plane y = 0.
                if d[1].abs() < 1e-9 {
                    continue;
                }
                let t = (0.0 - cam.centre[1]) / d[1];
                if t <= 0.0 {
                    continue;
                }
                let s = cam.centre[0] + t * d[0];
                let z = cam.centre[2] + t * d[2];
                if (0.0..5.0).contains(&s) && (0.0..12.0).contains(&z) {
                    pano.put_pixel(x, y, image::Rgb([220, 220, 220]));
                }
            }
        }
        let view = View {
            cam: &cam,
            fit: &fit,
            z_base: 0.0,
            dist_m: Some(12.0),
            s_vis: Some([0.0, 10.0]),
        };
        let out = resample_rect(
            &wall,
            &view,
            &pano,
            [0.0, 10.0, 0.0, 12.0],
            None,
            8,
            None,
            &Params::default(),
        );
        assert_eq!(out.rgb.width(), 80);
        assert_eq!(out.rgb.height(), 96);
        let bright = |x: u32, y: u32| out.rgb.get_pixel(x, y).0[0] > 128;
        assert!(
            bright(10, 50) && bright(30, 20),
            "the stripe is on the left"
        );
        assert!(!bright(50, 50) && !bright(70, 20), "and not on the right");
        assert!(out.valid.iter().all(|v| *v));
    }

    /// The walls whose photographs travel with the fixture, rendered from the
    /// photograph: the loose crop against the one the run wrote, then the final
    /// texture on the wall rectangle against the one the run rendered.
    ///
    /// Three walls, but they are the whole stage from the image: a perspective
    /// camera and two panoramas, footprint occlusion, cloud depth and the
    /// vegetation mask. Every other wall of the run was rendered from a 2.5 MB
    /// original and the fixture would weigh 190 MB with them in it; the lab
    /// measurement in the port report covers those.
    #[test]
    fn the_rectifier_reproduces_the_python() {
        if golden::pixels_absent() {
            return;
        }

        use crate::mapillary::golden;

        let walls = golden::texture_walls();
        let by_key = golden::walls_by_key();
        let buildings = golden::buildings();
        let tol = golden::texture_manifest().tolerances;
        let params = Params::default();
        let mut seen = 0usize;
        let mut bad = Vec::new();
        for entry in &walls {
            let Some(rectify) = entry.rectify.as_ref() else {
                continue;
            };
            let wall = &by_key[&entry.key];
            for r in rectify {
                let view_doc = entry
                    .views
                    .iter()
                    .find(|v| v.pano_id == r.pano_id)
                    .expect("the rectify record names a view of the wall");
                let cam = view_doc.camera();
                let fit = view_doc.plane_fit(&entry.key);
                let view = View {
                    cam: &cam,
                    fit: &fit,
                    z_base: view_doc.z_base,
                    dist_m: Some(view_doc.dist_m),
                    s_vis: Some(view_doc.s_vis),
                };
                let (Some(image), Some(want)) = (entry.image(r), entry.crop(r)) else {
                    continue;
                };
                let depth = entry.depth(r);
                let got = loose_crop(wall, &view, &image, depth.as_ref(), &buildings, &params);
                seen += 1;

                if (got.rgb.width(), got.rgb.height()) != (want.rgb.width(), want.rgb.height()) {
                    bad.push(format!(
                        "{}__{}: crop {}x{} against {}x{}",
                        entry.key,
                        r.pano_id,
                        got.rgb.width(),
                        got.rgb.height(),
                        want.rgb.width(),
                        want.rgb.height()
                    ));
                    continue;
                }
                for (name, a, b) in [
                    ("ppm", got.ppm, want.ppm),
                    ("s0", got.s0, want.s0),
                    ("h_bot", got.h_bot, want.h_bot),
                    ("h_top", got.h_top, want.h_top),
                    ("x_foot", got.x_foot, want.x_foot),
                    ("y_cam", got.y_cam, want.y_cam),
                ] {
                    if (a - b).abs() > 1e-6 {
                        bad.push(format!(
                            "{}__{}: {name} {a} against {b}",
                            entry.key, r.pano_id
                        ));
                    }
                }
                let n = got.occl.len();
                let all: Vec<bool> = vec![true; n];
                let (crop_mad, _) = golden::texture_agreement(&got.rgb, &all, &want.rgb, &all);
                let same_bits = (0..n).filter(|&i| got.occl[i] == want.occl[i]).count();
                let mine: Vec<bool> = got.occl.iter().map(|o| o & OCCLUDED_BITS == 0).collect();
                let theirs: Vec<bool> = want.occl.iter().map(|o| o & OCCLUDED_BITS == 0).collect();
                let inter = (0..n).filter(|&i| mine[i] && theirs[i]).count();
                let union = (0..n).filter(|&i| mine[i] || theirs[i]).count();
                let crop_iou = if union == 0 {
                    1.0
                } else {
                    inter as f64 / union as f64
                };
                // which bit went a different way is the only useful thing to
                // print when a crop disagrees, so it is always printed
                let per_bit: Vec<String> = [
                    ("footprint", OCC_FOOTPRINT),
                    ("cloud", OCC_CLOUD),
                    ("nadir", OCC_NADIR),
                    ("zenith", OCC_ZENITH),
                    ("seg", OCC_SEG),
                    ("outside", OCC_OUTSIDE),
                ]
                .iter()
                .filter_map(|(name, bit)| {
                    let d = (0..n)
                        .filter(|&i| (got.occl[i] & bit) != (want.occl[i] & bit))
                        .count();
                    (d > 0).then(|| format!("{name} {d}"))
                })
                .collect();
                println!(
                    "{}__{}  crop {}x{} ppm {:.2}  mad {:6.3}  bits {:.4}  free iou {:.4}  [{}]",
                    entry.key,
                    r.pano_id,
                    got.rgb.width(),
                    got.rgb.height(),
                    got.ppm,
                    crop_mad,
                    same_bits as f64 / n as f64,
                    crop_iou,
                    per_bit.join(", ")
                );
                if crop_mad > tol.texture_mean_abs_diff {
                    bad.push(format!(
                        "{}__{}: the loose crop differs by {crop_mad:.3} grey levels",
                        entry.key, r.pano_id
                    ));
                }
                if crop_iou < tol.valid_iou {
                    bad.push(format!(
                        "{}__{}: the unoccluded mask iou is {crop_iou:.4}",
                        entry.key, r.pano_id
                    ));
                }

                // and the final texture on the wall rectangle, which is what the
                // fusion and everything after it actually consume
                let out = resample_rect(
                    wall,
                    &view,
                    &image,
                    entry.view_rect(view_doc),
                    view_doc.h_shear.as_ref(),
                    entry.ppb,
                    Some(&got),
                    &params,
                );
                let reference = entry.view_texture(view_doc);
                let (mad, iou) = golden::texture_agreement(
                    &out.rgb,
                    &out.valid,
                    &reference.rgb,
                    &reference.valid,
                );
                println!(
                    "{}__{}  texture {}x{}  mad {:6.3}  iou {:.4}",
                    entry.key,
                    r.pano_id,
                    out.rgb.width(),
                    out.rgb.height(),
                    mad,
                    iou
                );
                if mad > tol.texture_mean_abs_diff {
                    bad.push(format!(
                        "{}__{}: the texture differs by {mad:.3} grey levels",
                        entry.key, r.pano_id
                    ));
                }
                if iou < tol.valid_iou {
                    bad.push(format!(
                        "{}__{}: the valid mask iou is {iou:.4}",
                        entry.key, r.pano_id
                    ));
                }
            }
        }
        assert!(
            seen >= 3,
            "the fixture must carry at least three views to render"
        );
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    /// The whole texture stage over the reference run's own photographs, wall by
    /// wall: the loose crop, the final texture of every view, the fusion, and
    /// the block grid the opening pass makes of the result.
    ///
    /// The fixture cannot carry this: 281 of the run's 292 views were rendered
    /// from a 2.5 MB original, which is 190 MB of photographs. So it reads the
    /// Python lab's own cache and steps aside when that is not on the machine.
    /// It is ignored by default because it decodes a hundred originals; run it
    /// with
    ///
    /// ```text
    /// cargo test --release -- --ignored mapillary::rectify::tests::the_texture_stage --nocapture
    /// ```
    ///
    /// **This measurement crosses a JPEG decoder boundary and its bounds say
    /// so.** The `image` crate does not decode a JPEG to the same bytes as the
    /// libjpeg-turbo inside OpenCV: over the three thumbnails the fixture does
    /// carry, 4 to 11 per cent of channel values come out one to five levels
    /// away, a mean absolute difference of 0.06 to 0.16. That is invisible in a
    /// view's texture, which is what the per view bounds here assert, but it is
    /// enough to move a vegetation mask edge, to flip a phase correlation peak
    /// that had no real peak to begin with, and through that to move the fusion
    /// of three walls onto the other branch. So the per wall class grid is
    /// printed and the **pooled** one is asserted; the per wall bound belongs to
    /// the fixture test, which starts from the run's own pixels and meets it at
    /// 100 per cent of cells.
    ///
    /// That the difference is the decoder and not the port was checked from the
    /// other side: `fuse.fuse_views_detailed` and `openings.classify` run in
    /// Python over these same Rust rendered views give a fused texture within
    /// 0.044 grey levels of the Rust one, the same valid mask on every wall, and
    /// the identical class grid on all 12 276 cells.
    #[test]
    #[ignore = "reads the Python lab's image cache, which is not part of the fixture"]
    fn the_texture_stage_over_the_whole_lab_run() {
        if golden::pixels_absent() {
            return;
        }

        use crate::mapillary::golden;
        use crate::mapillary::openings;
        use rayon::prelude::*;

        if golden::lab_cache_dir().is_none() {
            println!("the facade lab cache is not on this machine, so nothing was measured");
            return;
        }
        let walls = golden::texture_walls();
        let by_key = golden::walls_by_key();
        let buildings = golden::buildings();
        let tol = golden::texture_manifest().tolerances;
        let reference: std::collections::BTreeMap<String, golden::GoldenOpeningsWall> =
            golden::openings_walls()
                .into_iter()
                .map(|w| (w.key.clone(), w))
                .collect();
        let params = Params::default();

        type Row = (
            String,
            f64,
            f64,
            f64,
            f64,
            f64,
            usize,
            usize,
            String,
            String,
        );
        let rows: Vec<Option<Row>> = walls
            .par_iter()
            .map(|entry| {
                let rectify = entry.rectify.as_ref()?;
                let wall = by_key.get(&entry.key)?;
                let mut views = Vec::new();
                let (mut view_mad, mut view_iou) = (0.0f64, 1.0f64);
                for view_doc in &entry.views {
                    let r = rectify.iter().find(|r| r.pano_id == view_doc.pano_id)?;
                    let image = entry.lab_image(r)?;
                    let cam = view_doc.camera();
                    let fit = view_doc.plane_fit(&entry.key);
                    let view = View {
                        cam: &cam,
                        fit: &fit,
                        z_base: view_doc.z_base,
                        dist_m: Some(view_doc.dist_m),
                        s_vis: Some(view_doc.s_vis),
                    };
                    let depth = entry.depth(r);
                    let crop = loose_crop(wall, &view, &image, depth.as_ref(), &buildings, &params);
                    let out = resample_rect(
                        wall,
                        &view,
                        &image,
                        entry.view_rect(view_doc),
                        view_doc.h_shear.as_ref(),
                        entry.ppb,
                        Some(&crop),
                        &params,
                    );
                    let want = entry.view_texture(view_doc);
                    let (mad, iou) =
                        golden::texture_agreement(&out.rgb, &out.valid, &want.rgb, &want.valid);
                    view_mad = view_mad.max(mad);
                    view_iou = view_iou.min(iou);
                    views.push(super::super::fuse::ViewTexture {
                        pano_id: view_doc.pano_id.clone(),
                        rgb: out.rgb,
                        valid: out.valid,
                        score: view_doc.score,
                    });
                }
                let fused = super::super::fuse::fuse(&entry.key, &views, entry.ppb, &params);
                let (rgb, valid) = entry.fused();
                let (mad, iou) = golden::texture_agreement(&fused.rgb, &fused.valid, &rgb, &valid);
                let want = reference.get(&entry.key)?;
                let tex = openings::WallTexture::new(
                    fused.rgb.width() as usize,
                    fused.rgb.height() as usize,
                    fused.rgb.pixels().map(|p| p.0).collect(),
                    fused.valid.clone(),
                );
                let got = openings::classify(
                    &tex,
                    want.rows,
                    want.cols,
                    (want.origin_px[0], want.origin_px[1]),
                );
                let n = want.rows * want.cols;
                let same = (0..n)
                    .filter(|&i| got.cls[i] == want.openings.cls[i])
                    .count();
                Some((
                    entry.key.clone(),
                    view_mad,
                    view_iou,
                    mad,
                    iou,
                    same as f64 / n as f64,
                    same,
                    n,
                    fused.mode.as_str().to_string(),
                    entry.fuse.mode.clone(),
                ))
            })
            .collect();

        let mut bad = Vec::new();
        let mut moved = 0usize;
        let (mut same_total, mut cells_total) = (0usize, 0usize);
        let (mut worst_mad, mut worst_iou, mut worst_cls) = (0.0f64, 1.0f64, 1.0f64);
        let mut seen = 0usize;
        for row in rows.into_iter().flatten() {
            let (key, vmad, viou, mad, iou, agree, same, n, mode, want_mode) = row;
            seen += 1;
            same_total += same;
            cells_total += n;
            worst_mad = worst_mad.max(mad).max(vmad);
            worst_iou = worst_iou.min(iou).min(viou);
            worst_cls = worst_cls.min(agree);
            moved += usize::from(mode != want_mode);
            println!(
                "{key:16} {mode:9} views mad {vmad:6.3} iou {viou:.4}   fused mad {mad:6.3} \
                 iou {iou:.4}   cells {agree:.4}{}",
                if mode == want_mode {
                    ""
                } else {
                    "   [branch moved]"
                }
            );
            // The per view texture is the rectifier's own answer and is held to
            // the stated bound. The mask agreement is a tenth looser because the
            // vegetation mask grows every decoder disagreement by its 0.3 m
            // dilation; the fused texture and the class grid are printed rather
            // than asserted per wall, for the reason in the doc comment.
            if vmad > tol.texture_mean_abs_diff {
                bad.push(format!(
                    "{key}: a view texture is {vmad:.4} against {}",
                    tol.texture_mean_abs_diff
                ));
            }
            if viou < LAB_MASK_IOU {
                bad.push(format!(
                    "{key}: a view mask iou is {viou:.4} against {LAB_MASK_IOU}"
                ));
            }
        }
        assert!(seen > 0, "the lab cache is there but carried no photograph");
        let pooled = same_total as f64 / cells_total as f64;
        println!(
            "{seen} walls, {cells_total} cells, pooled class agreement {pooled:.4}, worst texture \
             difference {worst_mad:.3}, worst mask iou {worst_iou:.4}, worst grid {worst_cls:.4}, \
             {moved} walls on the other fuse branch"
        );
        assert!(
            pooled >= tol.class_grid_agreement,
            "the pooled class agreement is {pooled:.4}"
        );
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }
}
