//! The pixel operations the Python prototype reaches into OpenCV for.
//!
//! Morphology, connected components with statistics and a median filter, plus
//! the OkLab pair in `f64`. Every one of them is under a hundred lines, and the
//! semantics are pinned by the OpenCV calls in `tools/facade_lab/openings.py`
//! rather than invented here, because a golden test compares grids cell by cell
//! and a border convention that differs by one pixel moves whole windows.
//!
//! Two conventions are worth writing down, since they are the ones that are
//! easy to get wrong and impossible to spot afterwards:
//!
//! * OpenCV's default morphology border is not "replicate" and not "zero". For
//!   `erode` the outside of the image counts as foreground and for `dilate` as
//!   background, which is what keeps a blob that touches the image edge from
//!   being eaten by an opening. `cv2.morphologyEx(m, MORPH_OPEN, ones)` on a
//!   2x2 block in the corner of a 5x5 image returns the block unchanged;
//!   with a zero border it would return nothing.
//! * Connected component labels come out in raster order of each component's
//!   first pixel, merges included. The Python appends its rectangles in label
//!   order and then sorts them with a stable sort, so ties break on that order
//!   and the labels have to agree, not just the partition.
//!
//! `rectify.rs` needs the same three, which is why they live here rather than
//! inside `openings.rs`.

#![allow(dead_code)]

// --------------------------------------------------------------------------- masks

/// A binary image, row major, `bits[y * w + x]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub bits: Vec<bool>,
}

impl Mask {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            bits: vec![false; w * h],
        }
    }

    pub fn from_bits(w: usize, h: usize, bits: Vec<bool>) -> Self {
        assert_eq!(bits.len(), w * h, "mask bits do not fit {w}x{h}");
        Self { w, h, bits }
    }

    /// Out of bounds reads as background, so callers can probe a neighbourhood
    /// without clamping first.
    #[inline]
    pub fn get(&self, x: isize, y: isize) -> bool {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return false;
        }
        self.bits[y as usize * self.w + x as usize]
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, v: bool) {
        self.bits[y * self.w + x] = v;
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> bool {
        self.bits[y * self.w + x]
    }

    pub fn count(&self) -> usize {
        self.bits.iter().filter(|b| **b).count()
    }

    pub fn any(&self) -> bool {
        self.bits.iter().any(|b| *b)
    }
}

// --------------------------------------------------------------------------- morphology

/// Erosion with a filled rectangle of `kw` by `kh`, anchored at its centre.
///
/// Outside the image counts as foreground, which is OpenCV's default border for
/// `erode`: a blob against the image edge is not eroded from the outside.
pub fn erode(m: &Mask, kw: usize, kh: usize) -> Mask {
    let (ax, ay) = (kw as isize / 2, kh as isize / 2);
    let mut out = Mask::new(m.w, m.h);
    for y in 0..m.h {
        for x in 0..m.w {
            let mut keep = true;
            'k: for dy in 0..kh as isize {
                for dx in 0..kw as isize {
                    let (sx, sy) = (x as isize + dx - ax, y as isize + dy - ay);
                    if sx < 0 || sy < 0 || sx as usize >= m.w || sy as usize >= m.h {
                        continue; // outside is foreground
                    }
                    if !m.at(sx as usize, sy as usize) {
                        keep = false;
                        break 'k;
                    }
                }
            }
            out.set(x, y, keep);
        }
    }
    out
}

/// Dilation with a filled rectangle of `kw` by `kh`, anchored at its centre.
/// Outside the image counts as background, OpenCV's default border for `dilate`.
pub fn dilate(m: &Mask, kw: usize, kh: usize) -> Mask {
    let (ax, ay) = (kw as isize / 2, kh as isize / 2);
    let mut out = Mask::new(m.w, m.h);
    for y in 0..m.h {
        for x in 0..m.w {
            let mut hit = false;
            'k: for dy in 0..kh as isize {
                for dx in 0..kw as isize {
                    let (sx, sy) = (x as isize + dx - ax, y as isize + dy - ay);
                    if sx >= 0
                        && sy >= 0
                        && (sx as usize) < m.w
                        && (sy as usize) < m.h
                        && m.at(sx as usize, sy as usize)
                    {
                        hit = true;
                        break 'k;
                    }
                }
            }
            out.set(x, y, hit);
        }
    }
    out
}

/// Opening: erode then dilate. Removes what is thinner than the kernel.
pub fn open(m: &Mask, kw: usize, kh: usize) -> Mask {
    dilate(&erode(m, kw, kh), kw, kh)
}

/// Closing: dilate then erode. Bridges gaps narrower than the kernel.
pub fn close(m: &Mask, kw: usize, kh: usize) -> Mask {
    erode(&dilate(m, kw, kh), kw, kh)
}

// --------------------------------------------------------------------------- components

/// The bounding box and pixel count of one connected component, the five
/// numbers `cv2.connectedComponentsWithStats` puts in a stats row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Component {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub area: usize,
}

/// Labelled components. `components[0]` is the background, so a label is its own
/// index and the Python's `for i in range(1, n)` translates directly.
#[derive(Clone, Debug)]
pub struct Labels {
    pub w: usize,
    pub h: usize,
    /// Row major, 0 for background.
    pub labels: Vec<u32>,
    pub components: Vec<Component>,
}

impl Labels {
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> u32 {
        self.labels[y * self.w + x]
    }

    /// Number of labels including the background, the `n` OpenCV returns.
    pub fn count(&self) -> usize {
        self.components.len()
    }
}

/// Four-connected components, labelled in raster order of each component's
/// first pixel.
///
/// Two passes with union-find: the first hands out provisional labels and
/// records merges, the second renumbers the surviving roots in the order their
/// first pixel is met. Because a root's provisional label is the smallest in
/// its class, and provisional labels are handed out in raster order, that is
/// exactly the order OpenCV produces.
pub fn connected_components(m: &Mask) -> Labels {
    let n = m.w * m.h;
    let mut prov = vec![0u32; n];
    // parent[0] is unused; provisional labels start at 1.
    let mut parent: Vec<u32> = vec![0];

    fn find(parent: &mut [u32], mut a: u32) -> u32 {
        while parent[a as usize] != a {
            let g = parent[parent[a as usize] as usize];
            parent[a as usize] = g;
            a = g;
        }
        a
    }

    for y in 0..m.h {
        for x in 0..m.w {
            if !m.at(x, y) {
                continue;
            }
            let up = if y > 0 { prov[(y - 1) * m.w + x] } else { 0 };
            let left = if x > 0 { prov[y * m.w + x - 1] } else { 0 };
            let label = match (up, left) {
                (0, 0) => {
                    let fresh = parent.len() as u32;
                    parent.push(fresh);
                    fresh
                }
                (u, 0) => u,
                (0, l) => l,
                (u, l) => {
                    let (ru, rl) = (find(&mut parent, u), find(&mut parent, l));
                    let (lo, hi) = if ru <= rl { (ru, rl) } else { (rl, ru) };
                    parent[hi as usize] = lo;
                    lo
                }
            };
            prov[y * m.w + x] = label;
        }
    }

    let mut final_of = vec![0u32; parent.len()];
    let mut components = vec![Component {
        x: 0,
        y: 0,
        w: m.w,
        h: m.h,
        area: 0,
    }];
    let mut labels = vec![0u32; n];
    for y in 0..m.h {
        for x in 0..m.w {
            let p = prov[y * m.w + x];
            if p == 0 {
                components[0].area += 1;
                continue;
            }
            let root = find(&mut parent, p);
            if final_of[root as usize] == 0 {
                final_of[root as usize] = components.len() as u32;
                components.push(Component {
                    x,
                    y,
                    w: 1,
                    h: 1,
                    area: 0,
                });
            }
            let id = final_of[root as usize];
            labels[y * m.w + x] = id;
            let c = &mut components[id as usize];
            let x1 = (c.x + c.w).max(x + 1);
            let y1 = (c.y + c.h).max(y + 1);
            c.x = c.x.min(x);
            c.y = c.y.min(y);
            c.w = x1 - c.x;
            c.h = y1 - c.y;
            c.area += 1;
        }
    }

    Labels {
        w: m.w,
        h: m.h,
        labels,
        components,
    }
}

// --------------------------------------------------------------------------- filters

/// Square median filter with an odd side, replicating the border the way
/// `cv2.medianBlur` does.
pub fn median_filter_u8(src: &[u8], w: usize, h: usize, k: usize) -> Vec<u8> {
    assert!(k % 2 == 1, "median filter needs an odd side, got {k}");
    assert_eq!(src.len(), w * h);
    let r = (k / 2) as isize;
    let mut out = vec![0u8; w * h];
    let mut window: Vec<u8> = Vec::with_capacity(k * k);
    for y in 0..h {
        for x in 0..w {
            window.clear();
            for dy in -r..=r {
                let sy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                for dx in -r..=r {
                    let sx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                    window.push(src[sy * w + sx]);
                }
            }
            window.sort_unstable();
            out[y * w + x] = window[window.len() / 2];
        }
    }
    out
}

// --------------------------------------------------------------------------- OkLab

// The same matrices as `src/colors.rs::rgb_to_oklab` and
// `tools/facade_lab/common.py`. They are repeated here in `f64` because the
// port compares band colours against Python to within 0.03 OkLab and the
// pipeline medians thousands of pixels per cell: `f32` rounding on the way in
// showed up as whole units of sRGB on the way out. `colors.rs` stays `f32`
// because its callers compare one colour against a palette of sixty and never
// accumulate.
const M1: [[f64; 3]; 3] = [
    [0.41222147, 0.53633255, 0.05144599],
    [0.21190350, 0.68069950, 0.10739696],
    [0.08830246, 0.28171884, 0.62997870],
];
const M2: [[f64; 3]; 3] = [
    [0.21045426, 0.79361780, -0.00407205],
    [1.97799850, -2.42859220, 0.45059370],
    [0.02590404, 0.78277177, -0.80867577],
];
// `numpy.linalg.inv` of the two above, to full double precision.
const M1_INV: [[f64; 3]; 3] = [
    [4.076741961377998, -3.307712175172564, 0.23097004071865912],
    [-1.2684382023469425, 2.609757785310042, -0.34131946588840606],
    [
        -0.0041960228147971314,
        -0.7034187249804253,
        1.7076147197004337,
    ],
];
const M2_INV: [[f64; 3]; 3] = [
    [0.9999999813678504, 0.39633779045769935, 0.21580374731399515],
    [0.999999992554167, -0.1055613425759327, -0.06385417717899133],
    [
        1.0000000416594206,
        -0.08948418102623668,
        -1.2914855336765203,
    ],
];

#[inline]
fn matmul(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

#[inline]
fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
fn linear_to_srgb(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// One sRGB byte triple to OkLab.
pub fn srgb_to_oklab(rgb: [u8; 3]) -> [f64; 3] {
    let lin = [
        srgb_to_linear(rgb[0] as f64 / 255.0),
        srgb_to_linear(rgb[1] as f64 / 255.0),
        srgb_to_linear(rgb[2] as f64 / 255.0),
    ];
    let lms = matmul(&M1, lin);
    matmul(&M2, [lms[0].cbrt(), lms[1].cbrt(), lms[2].cbrt()])
}

/// OkLab back to sRGB in `0..=1`, clipped.
pub fn oklab_to_srgb(lab: [f64; 3]) -> [f64; 3] {
    let lms = matmul(&M2_INV, lab);
    let lin = matmul(&M1_INV, [lms[0].powi(3), lms[1].powi(3), lms[2].powi(3)]);
    [
        linear_to_srgb(lin[0]),
        linear_to_srgb(lin[1]),
        linear_to_srgb(lin[2]),
    ]
}

/// `numpy.round`: halves go to the even neighbour, not away from zero. Rust's
/// `f64::round` goes away from zero, so 0.5 would land a byte higher than the
/// Python for every colour that falls exactly between two levels.
#[inline]
pub fn round_half_even(x: f64) -> f64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && r % 2.0 != 0.0 {
        r - x.signum()
    } else {
        r
    }
}

/// OkLab to the byte triple the Python writes into a PNG.
pub fn oklab_to_rgb8(lab: [f64; 3]) -> [u8; 3] {
    let s = oklab_to_srgb(lab);
    let mut out = [0u8; 3];
    for i in 0..3 {
        out[i] = round_half_even(s[i] * 255.0).clamp(0.0, 255.0) as u8;
    }
    out
}

/// Euclidean OkLab distance.
pub fn oklab_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

// --------------------------------------------------------------------------- statistics

/// `numpy.median` of a slice: the middle value, or the mean of the two middle
/// ones. The caller owns the buffer because this sorts it.
pub fn median_in_place(v: &mut [f64]) -> f64 {
    debug_assert!(!v.is_empty(), "median of nothing");
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// `numpy.median` of a copy, for callers that still need the input.
pub fn median(v: &[f64]) -> f64 {
    let mut buf = v.to_vec();
    median_in_place(&mut buf)
}

/// `numpy.percentile` with the default linear interpolation.
pub fn percentile_in_place(v: &mut [f64], q: f64) -> f64 {
    debug_assert!(!v.is_empty(), "percentile of nothing");
    v.sort_by(f64::total_cmp);
    let pos = q / 100.0 * (v.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let frac = pos - lo as f64;
    if lo + 1 >= v.len() {
        v[v.len() - 1]
    } else {
        v[lo] + frac * (v[lo + 1] - v[lo])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask_of(rows: &[&str]) -> Mask {
        let h = rows.len();
        let w = rows[0].len();
        let mut m = Mask::new(w, h);
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                m.set(x, y, c == '#');
            }
        }
        m
    }

    fn render(m: &Mask) -> Vec<String> {
        (0..m.h)
            .map(|y| {
                (0..m.w)
                    .map(|x| if m.at(x, y) { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn opening_keeps_a_blob_in_the_corner() {
        // cv2.morphologyEx(m, MORPH_OPEN, ones(3,3)) on this input returns it
        // unchanged, because erode treats the outside as foreground.
        let m = mask_of(&["##...", "##...", ".....", ".....", "....."]);
        assert_eq!(render(&open(&m, 3, 3)), render(&m));
        // The erosion on its own leaves only the corner pixel, for the same
        // reason: it is the one pixel whose whole neighbourhood is either
        // foreground or outside.
        assert_eq!(
            render(&erode(&m, 3, 3)),
            vec!["#....", ".....", ".....", ".....", "....."]
        );
    }

    #[test]
    fn dilation_treats_the_outside_as_background() {
        let m = mask_of(&[".....", ".....", "..#..", ".....", "....."]);
        assert_eq!(
            render(&dilate(&m, 3, 3)),
            vec![".....", ".###.", ".###.", ".###.", "....."]
        );
    }

    #[test]
    fn closing_bridges_a_pier() {
        let m = mask_of(&["..#..", ".....", "..#.."]);
        let c = close(&m, 3, 3);
        assert!(c.at(2, 1), "a one pixel gap closes under a 3x3 kernel");
    }

    #[test]
    fn components_are_numbered_in_raster_order_of_first_pixel() {
        // Checked against cv2.connectedComponentsWithStats(connectivity=4):
        // the left column and the middle column merge on the bottom row and
        // keep label 1, the right column is 2 and 3.
        let m = mask_of(&["#.#.#", "#.#..", "###.#"]);
        let l = connected_components(&m);
        assert_eq!(l.count(), 4);
        assert_eq!(l.at(0, 0), 1);
        assert_eq!(l.at(2, 0), 1);
        assert_eq!(l.at(4, 0), 2);
        assert_eq!(l.at(4, 2), 3);
        assert_eq!(
            l.components[1],
            Component {
                x: 0,
                y: 0,
                w: 3,
                h: 3,
                area: 7
            }
        );
        assert_eq!(
            l.components[2],
            Component {
                x: 4,
                y: 0,
                w: 1,
                h: 1,
                area: 1
            }
        );
    }

    #[test]
    fn components_merge_through_a_u_shape() {
        let m = mask_of(&["#.#", "#.#", "###"]);
        let l = connected_components(&m);
        assert_eq!(l.count(), 2);
        assert_eq!(l.components[1].area, 7);
    }

    #[test]
    fn median_filter_removes_a_speck_and_replicates_the_border() {
        let src = vec![
            10, 10, 10, 10, //
            10, 200, 10, 10, //
            10, 10, 10, 10, //
        ];
        let out = median_filter_u8(&src, 4, 3, 3);
        assert!(out.iter().all(|v| *v == 10), "{out:?}");
    }

    #[test]
    fn oklab_matches_the_f32_conversion_in_colors_rs() {
        for rgb in [
            [0u8, 0, 0],
            [255, 255, 255],
            [154, 160, 171],
            [17, 200, 34],
            [200, 40, 90],
        ] {
            let lab = srgb_to_oklab(rgb);
            let (l, a, b) = crate::colors::rgb_to_oklab(rgb[0], rgb[1], rgb[2]);
            assert!((lab[0] - l as f64).abs() < 1e-6, "{rgb:?} L {lab:?} {l}");
            assert!((lab[1] - a as f64).abs() < 1e-6, "{rgb:?} a");
            assert!((lab[2] - b as f64).abs() < 1e-6, "{rgb:?} b");
        }
    }

    #[test]
    fn oklab_round_trips_through_bytes() {
        for r in (0..=255).step_by(37) {
            for g in (0..=255).step_by(53) {
                for b in (0..=255).step_by(29) {
                    let rgb = [r as u8, g as u8, b as u8];
                    assert_eq!(oklab_to_rgb8(srgb_to_oklab(rgb)), rgb, "{rgb:?}");
                }
            }
        }
    }

    #[test]
    fn rounding_follows_numpy_not_rust() {
        assert_eq!(round_half_even(0.5), 0.0);
        assert_eq!(round_half_even(1.5), 2.0);
        assert_eq!(round_half_even(2.5), 2.0);
        assert_eq!(round_half_even(-0.5), 0.0);
        assert_eq!(round_half_even(-1.5), -2.0);
        assert_eq!(round_half_even(2.6), 3.0);
    }

    #[test]
    fn median_and_percentile_follow_numpy() {
        assert_eq!(median(&[1.0, 2.0, 3.0]), 2.0);
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), 2.5);
        let mut v = vec![1.0, 2.0, 3.0, 4.0];
        assert_eq!(percentile_in_place(&mut v, 75.0), 3.25);
        let mut one = vec![7.0];
        assert_eq!(percentile_in_place(&mut one, 75.0), 7.0);
    }
}
