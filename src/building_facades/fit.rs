//! Fitting one photograph to one wall.
//!
//! A wall is any width and any height; a texture spans the fixed number of
//! metres the manifest gives it. The one thing this must never do is stretch:
//! a window that comes out twice human height is the failure the whole feature
//! has to avoid. So there is exactly one resample in the pipeline, in
//! `manifest::load_scaled`, which brings the photograph to this run's pixels
//! per metre in **both** axes at once. Everything here is a crop or a whole
//! repeat of that image, addressed in metres:
//!
//! * **Horizontally**, a wall narrower than the photograph takes the middle of
//!   it. A wider one repeats the photograph if the manifest says its edges
//!   join, and otherwise repeats it mirrored, so the seam is a reflection at a
//!   pilaster rather than a cut through a window. Repeating the last column
//!   instead would read as the vertical smear `displays.rs` already refuses.
//! * **Vertically**, the wall's foot is the photograph's foot: the crop is
//!   anchored at the bottom and the sky end is what gets cut. A wall shorter
//!   than the photograph therefore keeps its shopfront and loses its top
//!   storeys, which is the right way round. A wall taller than the photograph
//!   keeps the ground floor once, where the manifest says there is one, and
//!   repeats the storeys above it. That repeat is a whole number of storeys by
//!   construction, so the floor lines stay level and the seam falls where a
//!   real building has one.
//! * **Shorter than one storey** (a garage, a wall cut down by a slope) is a
//!   bottom crop like any other: the windows are cut off part way up, which is
//!   what the bottom two metres of a building look like.

use image::RgbImage;

use super::manifest::Entry;

/// Where one wall reads the photograph. Metres in, metres out; the pixels come
/// afterwards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    px_per_m: f64,
    tex_w_m: f64,
    tex_h_m: f64,
    /// Bottom band that stays on the ground, zero when the texture has none.
    ground_m: f64,
    /// Band above it that repeats, a whole number of storeys.
    upper_m: f64,
    /// Whether the left and right edges join.
    tiles: bool,
    wall_w_m: f64,
    wall_h_m: f64,
    /// Metres the tiling is shifted along the wall, so two neighbours built
    /// from one texture do not start on the same window.
    phase_m: f64,
}

/// Where a metre of wall reads from, and which way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Run {
    /// Straight copy: source metre = wall metre + offset.
    Forward,
    /// Mirrored copy, for a wall wider than a texture that does not tile.
    Mirrored,
}

impl Fit {
    /// How `entry` covers a wall `wall_w_m` by `wall_h_m` metres at
    /// `px_per_m` output pixels per metre, tiled from `phase_m` metres in.
    pub fn new(entry: &Entry, px_per_m: f64, wall_w_m: f64, wall_h_m: f64, phase_m: f64) -> Self {
        Fit {
            px_per_m,
            tex_w_m: entry.metres_wide,
            tex_h_m: entry.metres_tall,
            ground_m: entry.ground_m(),
            upper_m: entry.upper_m(),
            tiles: entry.tiles_horizontally,
            wall_w_m: wall_w_m.max(0.0),
            wall_h_m: wall_h_m.max(0.0),
            phase_m,
        }
    }

    /// Whether the wall is wide enough to need the texture more than once.
    pub fn repeats_across(&self) -> bool {
        self.wall_w_m > self.tex_w_m
    }

    /// Whether the wall is tall enough to need the storeys repeated.
    pub fn repeats_up(&self) -> bool {
        self.wall_h_m > self.tex_h_m
    }

    /// Whether a repeat across is a mirrored one, which only a texture whose
    /// edges do not join ever needs.
    #[cfg(test)]
    pub fn mirrors(&self) -> bool {
        self.repeats_across() && !self.tiles
    }

    /// Source metre for `m` metres along the wall from its left end as the
    /// viewer outside it sees it, with which way round the copy runs.
    pub fn map_u(&self, m: f64) -> (f64, Run) {
        if !self.repeats_across() {
            // Narrower than the photograph: show the middle of it.
            return (m + (self.tex_w_m - self.wall_w_m) / 2.0, Run::Forward);
        }
        let shifted = m + self.phase_m;
        if self.tiles {
            return (shifted.rem_euclid(self.tex_w_m), Run::Forward);
        }
        // Two copies per period, the second one turned over, so the join is a
        // reflection and not a cut.
        let t = shifted.rem_euclid(2.0 * self.tex_w_m);
        if t < self.tex_w_m {
            (t, Run::Forward)
        } else {
            (2.0 * self.tex_w_m - t, Run::Mirrored)
        }
    }

    /// Source metre for `m` metres up from the foot of the wall.
    pub fn map_v(&self, m: f64) -> f64 {
        if !self.repeats_up() {
            // Fits inside the photograph: a straight crop from its foot, so
            // the shopfront lands on the street and the sky end is what goes.
            return m;
        }
        if m < self.ground_m {
            return m;
        }
        self.ground_m + (m - self.ground_m).rem_euclid(self.upper_m)
    }

    /// Source pixel column for `m` metres along the wall.
    fn src_x(&self, m: f64, src_w: u32) -> u32 {
        let (u, _) = self.map_u(m);
        ((u * self.px_per_m).floor() as i64).clamp(0, i64::from(src_w) - 1) as u32
    }

    /// Source pixel row for `m` metres up the wall. Image row 0 is the top, so
    /// this counts down from the bottom.
    fn src_y(&self, m: f64, src_h: u32) -> u32 {
        let v = (self.map_v(m) * self.px_per_m).floor() as i64;
        (i64::from(src_h) - 1 - v).clamp(0, i64::from(src_h) - 1) as u32
    }

    /// The pixels for the part of the wall from `x0_m` to `x1_m` along it and
    /// from `y0_m` to `y1_m` up it, at `px_per_m`. `None` when that is not a
    /// whole pixel of anything.
    ///
    /// The source has already been brought to `px_per_m`, so this is a
    /// per-pixel copy with an index table: no filtering, and no way for a
    /// metre of building to come out as anything but a metre.
    pub fn region(
        &self,
        src: &RgbImage,
        x0_m: f64,
        x1_m: f64,
        y0_m: f64,
        y1_m: f64,
    ) -> Option<RgbImage> {
        let (src_w, src_h) = (src.width(), src.height());
        if src_w == 0 || src_h == 0 {
            return None;
        }
        let out_w = (((x1_m - x0_m) * self.px_per_m).round() as i64).clamp(0, 1 << 15) as u32;
        let out_h = (((y1_m - y0_m) * self.px_per_m).round() as i64).clamp(0, 1 << 15) as u32;
        if out_w == 0 || out_h == 0 {
            return None;
        }
        // One lookup per column and per row rather than per pixel: the map is
        // separable, and a 32 by 32 block panel is a million pixels.
        // Byte offsets, not pixel indices: the inner loop then copies three
        // bytes out of a flat slice instead of going through the bounds
        // checked `get_pixel`/`put_pixel` pair on each of a million pixels.
        let cols: Vec<usize> = (0..out_w)
            .map(|i| self.src_x(x0_m + (f64::from(i) + 0.5) / self.px_per_m, src_w) as usize * 3)
            .collect();
        let rows: Vec<u32> = (0..out_h)
            .map(|j| self.src_y(y1_m - (f64::from(j) + 0.5) / self.px_per_m, src_h))
            .collect();
        let raw = src.as_raw();
        let src_stride = src_w as usize * 3;
        let stride = out_w as usize * 3;
        let mut out = vec![0u8; stride * out_h as usize];
        // A wall taller than the photograph reads every source row again once
        // per storey, and two output rows off the same source row hold the
        // same bytes because the columns do not change, so the repeat is a
        // copy of the row already built rather than a second gather.
        let mut built = vec![usize::MAX; src_h as usize];
        for (j, &sy) in rows.iter().enumerate() {
            let at = j * stride;
            let seen = built[sy as usize];
            if seen != usize::MAX {
                out.copy_within(seen..seen + stride, at);
                continue;
            }
            let base = sy as usize * src_stride;
            for (i, &off) in cols.iter().enumerate() {
                let from = base + off;
                out[at + i * 3..at + i * 3 + 3].copy_from_slice(&raw[from..from + 3]);
            }
            built[sy as usize] = at;
        }
        RgbImage::from_raw(out_w, out_h, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building_facades::manifest;

    fn entry(w: f64, h: f64, storeys: u32, tiles: bool, ground: bool) -> manifest::Entry {
        manifest::Entry {
            file: "t.png".to_string(),
            categories: vec!["Residential"],
            metres_wide: w,
            metres_tall: h,
            storeys,
            tiles_horizontally: tiles,
            has_ground_floor: ground,
            storey_m: h / f64::from(storeys),
        }
    }

    /// Every metre of wall reads exactly one metre of photograph: the map is
    /// piecewise slope one, so nothing is ever stretched or squashed. This is
    /// the property the whole feature stands on.
    fn assert_unit_slope(samples: impl Iterator<Item = (f64, f64)>) {
        let mut previous: Option<(f64, f64)> = None;
        for (m, mapped) in samples {
            if let Some((pm, pmapped)) = previous {
                let slope = (mapped - pmapped) / (m - pm);
                // A wrap or a mirror turn makes one step jump; every other
                // step is exactly one metre of photograph per metre of wall.
                assert!(
                    (slope - 1.0).abs() < 1e-9 || (slope + 1.0).abs() < 1e-9 || slope.abs() > 1.5,
                    "slope {slope} at {m}"
                );
            }
            previous = Some((m, mapped));
        }
    }

    #[test]
    fn a_narrow_wall_takes_the_middle_of_the_photograph() {
        let e = entry(20.0, 12.0, 4, true, true);
        let fit = Fit::new(&e, 8.0, 8.0, 12.0, 0.0);
        assert!(!fit.repeats_across());
        // 8 m of wall out of 20 m of photograph: 6 m trimmed off each side.
        assert!((fit.map_u(0.0).0 - 6.0).abs() < 1e-9);
        assert!((fit.map_u(8.0).0 - 14.0).abs() < 1e-9);
        assert_unit_slope((0..=80).map(|i| {
            let m = f64::from(i) / 10.0;
            (m, fit.map_u(m).0)
        }));
    }

    #[test]
    fn a_wide_wall_repeats_a_tiling_texture_without_stretching() {
        let e = entry(12.0, 12.0, 4, true, true);
        let fit = Fit::new(&e, 8.0, 40.0, 12.0, 0.0);
        assert!(fit.repeats_across());
        assert!(!fit.mirrors());
        assert!((fit.map_u(0.0).0 - 0.0).abs() < 1e-9);
        assert!((fit.map_u(11.9).0 - 11.9).abs() < 1e-9);
        // Wraps back to the left edge at the texture's width, never scaled.
        assert!((fit.map_u(12.0).0 - 0.0).abs() < 1e-9);
        assert!((fit.map_u(25.0).0 - 1.0).abs() < 1e-9);
        assert_eq!(fit.map_u(25.0).1, Run::Forward);
        assert_unit_slope((0..=400).map(|i| {
            let m = f64::from(i) / 10.0;
            (m, fit.map_u(m).0)
        }));
    }

    #[test]
    fn a_wide_wall_mirrors_a_texture_whose_edges_do_not_join() {
        let e = entry(12.0, 12.0, 4, false, true);
        let fit = Fit::new(&e, 8.0, 40.0, 12.0, 0.0);
        assert!(fit.mirrors());
        assert_eq!(fit.map_u(1.0).1, Run::Forward);
        // Past the first copy the photograph runs back the other way, so the
        // seam is a reflection at the edge instead of a cut.
        assert_eq!(fit.map_u(13.0).1, Run::Mirrored);
        assert!((fit.map_u(13.0).0 - 11.0).abs() < 1e-9);
        assert!((fit.map_u(24.0).0 - 0.0).abs() < 1e-9);
        assert_eq!(fit.map_u(25.0).1, Run::Forward);
        assert_unit_slope((0..=400).map(|i| {
            let m = f64::from(i) / 10.0;
            (m, fit.map_u(m).0)
        }));
    }

    #[test]
    fn the_phase_shifts_the_tiling_but_not_its_scale() {
        let e = entry(12.0, 12.0, 4, true, true);
        let plain = Fit::new(&e, 8.0, 40.0, 12.0, 0.0);
        let shifted = Fit::new(&e, 8.0, 40.0, 12.0, 5.0);
        assert!((shifted.map_u(0.0).0 - 5.0).abs() < 1e-9);
        assert!((shifted.map_u(1.0).0 - plain.map_u(6.0).0).abs() < 1e-9);
        assert_unit_slope((0..=400).map(|i| {
            let m = f64::from(i) / 10.0;
            (m, shifted.map_u(m).0)
        }));
    }

    #[test]
    fn a_short_wall_keeps_the_ground_floor_and_loses_the_top() {
        let e = entry(12.0, 12.0, 4, true, true);
        let fit = Fit::new(&e, 8.0, 12.0, 6.0, 0.0);
        assert!(!fit.repeats_up());
        // The foot of the wall is the foot of the photograph, so the shopfront
        // is what a two storey wall keeps.
        assert!((fit.map_v(0.0) - 0.0).abs() < 1e-9);
        assert!((fit.map_v(6.0) - 6.0).abs() < 1e-9);
        assert_unit_slope((0..=60).map(|i| {
            let m = f64::from(i) / 10.0;
            (m, fit.map_v(m))
        }));
    }

    #[test]
    fn a_wall_under_one_storey_is_still_a_bottom_crop() {
        let e = entry(12.0, 12.0, 4, true, true);
        let fit = Fit::new(&e, 8.0, 12.0, 2.0, 0.0);
        assert!(!fit.repeats_up());
        assert!((fit.map_v(0.0) - 0.0).abs() < 1e-9);
        assert!((fit.map_v(1.9) - 1.9).abs() < 1e-9);
    }

    #[test]
    fn a_tall_wall_repeats_whole_storeys_above_the_ground_floor() {
        // Four storeys of 3 m, the bottom one a shopfront.
        let e = entry(12.0, 12.0, 4, true, true);
        let fit = Fit::new(&e, 8.0, 12.0, 30.0, 0.0);
        assert!(fit.repeats_up());
        // The ground floor is used once, at the ground.
        assert!((fit.map_v(0.0) - 0.0).abs() < 1e-9);
        assert!((fit.map_v(2.9) - 2.9).abs() < 1e-9);
        // Above it the nine metres of upper storeys repeat, and the repeat is
        // a whole number of storeys, so every floor line lands on a floor
        // line: 3 m up reads the same pixels as 12 m up.
        assert!((fit.map_v(3.0) - 3.0).abs() < 1e-9);
        assert!((fit.map_v(12.0) - 3.0).abs() < 1e-9);
        assert!((fit.map_v(21.0) - 3.0).abs() < 1e-9);
        let period = fit.upper_m;
        assert!((period / e.storey_m - 3.0).abs() < 1e-9);
        assert_unit_slope((0..=300).map(|i| {
            let m = f64::from(i) / 10.0;
            (m, fit.map_v(m))
        }));
    }

    #[test]
    fn a_tall_wall_on_a_texture_without_a_ground_floor_repeats_the_whole_image() {
        let e = entry(12.0, 9.0, 3, true, false);
        let fit = Fit::new(&e, 8.0, 12.0, 30.0, 0.0);
        assert_eq!(fit.ground_m, 0.0);
        assert!((fit.upper_m - 9.0).abs() < 1e-9);
        assert!((fit.map_v(0.0) - 0.0).abs() < 1e-9);
        assert!((fit.map_v(9.0) - 0.0).abs() < 1e-9);
        assert!((fit.map_v(10.5) - 1.5).abs() < 1e-9);
    }

    /// A synthetic photograph whose pixels say where they came from, so a crop
    /// can be checked pixel for pixel rather than by eye.
    fn ramp(w: u32, h: u32) -> RgbImage {
        RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 0])
        })
    }

    #[test]
    fn a_region_copies_source_pixels_one_for_one() {
        // 12 m by 12 m at 8 px per metre is 96 by 96 pixels.
        let e = entry(12.0, 12.0, 4, true, true);
        let src = ramp(96, 96);
        let fit = Fit::new(&e, 8.0, 12.0, 12.0, 0.0);
        let out = fit.region(&src, 0.0, 12.0, 0.0, 12.0).unwrap();
        assert_eq!((out.width(), out.height()), (96, 96));
        // Same size in, same size out, and the top left of the wall is the top
        // left of the photograph.
        assert_eq!(out.get_pixel(0, 0), src.get_pixel(0, 0));
        assert_eq!(out.get_pixel(95, 95), src.get_pixel(95, 95));
        assert_eq!(out.get_pixel(40, 70), src.get_pixel(40, 70));
    }

    #[test]
    fn a_region_of_a_tall_wall_repeats_pixels_exactly() {
        let e = entry(12.0, 12.0, 4, true, true);
        let src = ramp(96, 96);
        let fit = Fit::new(&e, 8.0, 12.0, 24.0, 0.0);
        let out = fit.region(&src, 0.0, 12.0, 0.0, 24.0).unwrap();
        assert_eq!((out.width(), out.height()), (96, 192));
        // Bottom row of the wall is the bottom row of the photograph.
        assert_eq!(out.get_pixel(10, 191), src.get_pixel(10, 95));
        // The upper band is nine metres, so two rows nine metres apart on the
        // wall read the same pixels: the repeat moved by whole storeys and did
        // not drift. Nine metres is 72 rows at 8 pixels per metre, and row 95
        // is the lowest whose partner is still above the ground floor.
        for j in [0u32, 20, 50, 95] {
            assert_eq!(
                out.get_pixel(10, j),
                out.get_pixel(10, j + 72),
                "rows {j} and {} are nine metres apart",
                j + 72
            );
        }
    }

    #[test]
    fn a_region_of_a_piece_lines_up_with_the_whole_wall() {
        // What `mod.rs` does when a wall is cut into panels: the pieces must
        // join without a shift, or the seam shows.
        let e = entry(12.0, 12.0, 4, true, true);
        let src = ramp(96, 96);
        let fit = Fit::new(&e, 8.0, 24.0, 12.0, 0.0);
        let whole = fit.region(&src, 0.0, 24.0, 0.0, 12.0).unwrap();
        let left = fit.region(&src, 0.0, 12.0, 0.0, 12.0).unwrap();
        let right = fit.region(&src, 12.0, 24.0, 0.0, 12.0).unwrap();
        assert_eq!(whole.width(), left.width() + right.width());
        for y in 0..whole.height() {
            for x in 0..left.width() {
                assert_eq!(whole.get_pixel(x, y), left.get_pixel(x, y), "{x},{y}");
            }
            for x in 0..right.width() {
                assert_eq!(
                    whole.get_pixel(x + left.width(), y),
                    right.get_pixel(x, y),
                    "{x},{y}"
                );
            }
        }
    }

    #[test]
    fn a_zero_sized_region_is_none_rather_than_a_panic() {
        let e = entry(12.0, 12.0, 4, true, true);
        let src = ramp(96, 96);
        let fit = Fit::new(&e, 8.0, 12.0, 12.0, 0.0);
        assert!(fit.region(&src, 4.0, 4.0, 0.0, 12.0).is_none());
        assert!(fit.region(&src, 0.0, 12.0, 3.0, 3.0).is_none());
        assert!(fit
            .region(&RgbImage::new(0, 0), 0.0, 1.0, 0.0, 1.0)
            .is_none());
    }
}
