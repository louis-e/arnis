//! Pixel canvas in map-color-id space with the primitives the templates draw with.
//! Output goes straight into `map_N.dat` colors, so generated art costs no quantization.

use crate::map_item_palette::{map_color_id, map_color_rgb, nearest_map_color, TRANSPARENT};
use image::RgbaImage;

/// Side length of one map tile in pixels.
pub const TILE: u32 = 128;

/// Named map colors used by the sign templates (base id, shade). Shade 2 is the unmodified
/// base color; shades 0/1 are darker, 3 is darkest.
#[allow(dead_code)]
pub mod colors {
    use crate::map_item_palette::map_color_id;
    pub const WHITE: u8 = map_color_id(8, 2);
    pub const OFF_WHITE: u8 = map_color_id(14, 2);
    pub const LIGHT_GRAY: u8 = map_color_id(3, 2);
    pub const GRAY: u8 = map_color_id(22, 2);
    pub const DARK_GRAY: u8 = map_color_id(21, 2);
    pub const NEAR_BLACK: u8 = map_color_id(21, 3);
    pub const BLACK: u8 = map_color_id(29, 2);
    pub const RED: u8 = map_color_id(4, 0);
    pub const BRIGHT_RED: u8 = map_color_id(4, 2);
    pub const DARK_RED: u8 = map_color_id(28, 1);
    pub const ORANGE: u8 = map_color_id(15, 2);
    pub const YELLOW: u8 = map_color_id(18, 2);
    pub const GOLD: u8 = map_color_id(30, 2);
    pub const GREEN: u8 = map_color_id(7, 1);
    pub const LIGHT_GREEN: u8 = map_color_id(19, 2);
    pub const SIGN_GREEN: u8 = map_color_id(27, 1);
    pub const BLUE: u8 = map_color_id(25, 2);
    pub const DARK_BLUE: u8 = map_color_id(25, 0);
    pub const LIGHT_BLUE: u8 = map_color_id(17, 2);
    pub const SKY: u8 = map_color_id(32, 2);
    pub const CYAN: u8 = map_color_id(23, 2);
    pub const PURPLE: u8 = map_color_id(24, 2);
    pub const MAGENTA: u8 = map_color_id(16, 2);
    pub const PINK: u8 = map_color_id(20, 2);
    pub const BROWN: u8 = map_color_id(26, 2);
    pub const WOOD: u8 = map_color_id(13, 2);
    pub const SAND: u8 = map_color_id(2, 2);
    pub const TEAL: u8 = map_color_id(55, 2);
}

/// A `cols x rows` tile grid of map color ids; 0 is transparent.
#[derive(Clone, Debug)]
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    px: Vec<u8>,
}

impl Canvas {
    /// Transparent canvas covering `cols` by `rows` map tiles.
    pub fn new(cols: u32, rows: u32) -> Self {
        Self::with_size(cols * TILE, rows * TILE)
    }

    pub fn with_size(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            px: vec![TRANSPARENT; (w * h) as usize],
        }
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return TRANSPARENT;
        }
        self.px[(y as u32 * self.w + x as u32) as usize]
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, id: u8) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        self.px[(y as u32 * self.w + x as u32) as usize] = id;
    }

    pub fn fill(&mut self, id: u8) {
        self.px.fill(id);
    }

    pub fn fill_rect(&mut self, x0: i32, y0: i32, w: i32, h: i32, id: u8) {
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                self.set(x, y, id);
            }
        }
    }

    /// Rectangle outline `t` pixels thick, drawn inward from the given bounds.
    pub fn stroke_rect(&mut self, x0: i32, y0: i32, w: i32, h: i32, t: i32, id: u8) {
        self.fill_rect(x0, y0, w, t, id);
        self.fill_rect(x0, y0 + h - t, w, t, id);
        self.fill_rect(x0, y0, t, h, id);
        self.fill_rect(x0 + w - t, y0, t, h, id);
    }

    /// Filled rectangle with rounded corners of radius `r`.
    pub fn rounded_rect(&mut self, x0: i32, y0: i32, w: i32, h: i32, r: i32, id: u8) {
        let r = r.clamp(0, (w / 2).min(h / 2));
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                if inside_rounded(x - x0, y - y0, w, h, r) {
                    self.set(x, y, id);
                }
            }
        }
    }

    /// Rounded rectangle outline `t` pixels thick.
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_rounded_rect(
        &mut self,
        x0: i32,
        y0: i32,
        w: i32,
        h: i32,
        r: i32,
        t: i32,
        id: u8,
    ) {
        let r = r.clamp(0, (w / 2).min(h / 2));
        let ri = (r - t).max(0);
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                let (lx, ly) = (x - x0, y - y0);
                let outer = inside_rounded(lx, ly, w, h, r);
                let inner = lx >= t
                    && ly >= t
                    && lx < w - t
                    && ly < h - t
                    && inside_rounded(lx - t, ly - t, w - 2 * t, h - 2 * t, ri);
                if outer && !inner {
                    self.set(x, y, id);
                }
            }
        }
    }

    pub fn disc(&mut self, cx: i32, cy: i32, r: i32, id: u8) {
        let rr = (r as f32 + 0.5) * (r as f32 + 0.5);
        for y in cy - r..=cy + r {
            for x in cx - r..=cx + r {
                let dx = (x - cx) as f32;
                let dy = (y - cy) as f32;
                if dx * dx + dy * dy <= rr {
                    self.set(x, y, id);
                }
            }
        }
    }

    pub fn ring(&mut self, cx: i32, cy: i32, r_out: i32, r_in: i32, id: u8) {
        let ro = (r_out as f32 + 0.5) * (r_out as f32 + 0.5);
        let ri = (r_in as f32 + 0.5) * (r_in as f32 + 0.5);
        for y in cy - r_out..=cy + r_out {
            for x in cx - r_out..=cx + r_out {
                let dx = (x - cx) as f32;
                let dy = (y - cy) as f32;
                let d = dx * dx + dy * dy;
                if d <= ro && d > ri {
                    self.set(x, y, id);
                }
            }
        }
    }

    /// Filled polygon (even-odd scanline fill).
    pub fn polygon(&mut self, pts: &[(f32, f32)], id: u8) {
        if pts.len() < 3 {
            return;
        }
        let min_y = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor() as i32;
        let max_y = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil() as i32;
        let mut xs: Vec<f32> = Vec::new();
        for y in min_y..=max_y {
            let sy = y as f32 + 0.5;
            xs.clear();
            for i in 0..pts.len() {
                let (x0, y0) = pts[i];
                let (x1, y1) = pts[(i + 1) % pts.len()];
                if (y0 <= sy && y1 > sy) || (y1 <= sy && y0 > sy) {
                    xs.push(x0 + (sy - y0) * (x1 - x0) / (y1 - y0));
                }
            }
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            for pair in xs.chunks(2) {
                if let [a, b] = pair {
                    for x in a.round() as i32..b.round() as i32 {
                        self.set(x, y, id);
                    }
                }
            }
        }
    }

    /// Regular `n`-gon of circumradius `r`, first vertex at angle `rot` (radians, clockwise
    /// from up).
    pub fn regular_polygon(&mut self, cx: i32, cy: i32, r: f32, n: usize, rot: f32, id: u8) {
        let pts: Vec<(f32, f32)> = (0..n)
            .map(|k| {
                let a = rot + k as f32 * std::f32::consts::TAU / n as f32;
                (cx as f32 + 0.5 + r * a.sin(), cy as f32 + 0.5 - r * a.cos())
            })
            .collect();
        self.polygon(&pts, id);
    }

    /// Thick line with round-ish ends.
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, width: i32, id: u8) {
        let dx = (x1 - x0) as f32;
        let dy = (y1 - y0) as f32;
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let steps = len.ceil() as i32 * 2;
        let r = ((width - 1) / 2).max(0);
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            let x = (x0 as f32 + dx * t).round() as i32;
            let y = (y0 as f32 + dy * t).round() as i32;
            if r == 0 {
                self.set(x, y, id);
            } else {
                self.disc(x, y, r, id);
            }
        }
    }

    /// Blit an RGBA image; alpha under 128 stays transparent, the rest is quantized.
    pub fn blit_rgba(&mut self, img: &RgbaImage, x0: i32, y0: i32) {
        for (x, y, p) in img.enumerate_pixels() {
            if p.0[3] < 128 {
                continue;
            }
            self.set(
                x0 + x as i32,
                y0 + y as i32,
                nearest_map_color(p.0[0], p.0[1], p.0[2]),
            );
        }
    }

    /// One 128x128 tile as a map `colors` array.
    pub fn tile(&self, col: u32, row: u32) -> Vec<i8> {
        let mut out = vec![TRANSPARENT as i8; (TILE * TILE) as usize];
        for y in 0..TILE {
            for x in 0..TILE {
                out[(y * TILE + x) as usize] =
                    self.get((col * TILE + x) as i32, (row * TILE + y) as i32) as i8;
            }
        }
        out
    }

    /// Whole canvas as an RGBA image (debug/preview helper).
    #[allow(dead_code)]
    pub fn to_rgba(&self) -> RgbaImage {
        let mut img = RgbaImage::new(self.w, self.h);
        for y in 0..self.h {
            for x in 0..self.w {
                let id = self.get(x as i32, y as i32);
                let (r, g, b) = map_color_rgb(id);
                let a = if id == TRANSPARENT { 0 } else { 255 };
                img.put_pixel(x, y, image::Rgba([r, g, b, a]));
            }
        }
        img
    }
}

fn inside_rounded(lx: i32, ly: i32, w: i32, h: i32, r: i32) -> bool {
    if lx < 0 || ly < 0 || lx >= w || ly >= h {
        return false;
    }
    if r <= 0 {
        return true;
    }
    let cx = if lx < r {
        r - 1
    } else if lx >= w - r {
        w - r
    } else {
        return true;
    };
    let cy = if ly < r {
        r - 1
    } else if ly >= h - r {
        h - r
    } else {
        return true;
    };
    let dx = (lx - cx) as f32;
    let dy = (ly - cy) as f32;
    dx * dx + dy * dy <= (r as f32 + 0.5) * (r as f32 + 0.5)
}

/// Mix two map colors and requantize; `t` is the weight of `a`.
pub fn mix(a: u8, b: u8, t: f32) -> u8 {
    let (ar, ag, ab) = map_color_rgb(a);
    let (br, bg, bb) = map_color_rgb(b);
    let l = |x: u8, y: u8| {
        (x as f32 * t + y as f32 * (1.0 - t))
            .round()
            .clamp(0.0, 255.0) as u8
    };
    nearest_map_color(l(ar, br), l(ag, bg), l(ab, bb))
}

/// Darker shade of the same base color.
pub const fn darker(id: u8) -> u8 {
    map_color_id(id / 4, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_extraction_reads_the_right_quadrant() {
        let mut c = Canvas::new(2, 1);
        c.fill_rect(128, 0, 128, 128, colors::RED);
        assert_eq!(c.tile(0, 0)[0], TRANSPARENT as i8);
        assert_eq!(c.tile(1, 0)[0], colors::RED as i8);
        assert_eq!(c.tile(1, 0).len(), 16384);
    }

    #[test]
    fn rounded_rect_clears_corners_but_fills_center() {
        let mut c = Canvas::new(1, 1);
        c.rounded_rect(0, 0, 128, 128, 30, colors::BLUE);
        assert_eq!(c.get(0, 0), TRANSPARENT);
        assert_eq!(c.get(64, 64), colors::BLUE);
        assert_eq!(c.get(64, 0), colors::BLUE);
    }

    #[test]
    fn polygon_fills_a_triangle() {
        let mut c = Canvas::new(1, 1);
        c.polygon(
            &[(64.0, 10.0), (118.0, 118.0), (10.0, 118.0)],
            colors::WHITE,
        );
        assert_eq!(c.get(64, 80), colors::WHITE);
        assert_eq!(c.get(5, 5), TRANSPARENT);
        assert_eq!(c.get(120, 20), TRANSPARENT);
    }

    #[test]
    fn regular_octagon_is_symmetric() {
        let mut c = Canvas::new(1, 1);
        c.regular_polygon(64, 64, 60.0, 8, std::f32::consts::PI / 8.0, colors::RED);
        assert_eq!(c.get(64, 64), colors::RED);
        assert_eq!(c.get(2, 2), TRANSPARENT);
        // Apothem is 60*cos(22.5deg) = 55.4, so the flat top edge starts at row 9.
        assert_eq!(c.get(64, 12), colors::RED);
        assert_eq!(c.get(12, 64), colors::RED);
        assert_eq!(c.get(64, 6), TRANSPARENT);
    }
}
