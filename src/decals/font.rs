//! Bitmap font rendering for signage. The atlases are DejaVu Sans Bold rasterized by
//! `assets/decorations/tools/gen_font.py`; see that script for the index format.

use super::draw::{mix, Canvas};
use crate::map_item_palette::TRANSPARENT;
use image::GrayImage;
use once_cell::sync::Lazy;
use std::collections::HashMap;

/// Available pixel sizes (em size the atlas was rasterized at), smallest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FontSize {
    S12,
    S18,
    S28,
    S44,
    S64,
}

impl FontSize {
    pub const ALL: [FontSize; 5] = [
        FontSize::S12,
        FontSize::S18,
        FontSize::S28,
        FontSize::S44,
        FontSize::S64,
    ];

    fn index(self) -> usize {
        match self {
            FontSize::S12 => 0,
            FontSize::S18 => 1,
            FontSize::S28 => 2,
            FontSize::S44 => 3,
            FontSize::S64 => 4,
        }
    }
}

struct Glyph {
    x: u16,
    w: u8,
    cursor_off: i8,
    adv: u8,
}

pub struct Font {
    atlas: GrayImage,
    line_height: u8,
    glyphs: HashMap<char, Glyph>,
}

macro_rules! font_assets {
    ($($size:literal),*) => {
        [$((
            include_bytes!(concat!("../../assets/decorations/font/dejavu_bold_", $size, ".png")) as &[u8],
            include_bytes!(concat!("../../assets/decorations/font/dejavu_bold_", $size, ".bin")) as &[u8],
        )),*]
    };
}

static FONT_ASSETS: [(&[u8], &[u8]); 5] = font_assets!("12", "18", "28", "44", "64");

static FONTS: Lazy<Vec<Font>> = Lazy::new(|| {
    FONT_ASSETS
        .iter()
        .map(|(png, idx)| Font::parse(png, idx).expect("bundled font atlas is valid"))
        .collect()
});

impl Font {
    fn parse(png: &[u8], idx: &[u8]) -> Result<Font, String> {
        let atlas = image::load_from_memory(png)
            .map_err(|e| format!("font atlas: {e}"))?
            .to_luma8();
        if idx.len() < 8 || &idx[0..4] != b"AFN1" {
            return Err("font index: bad magic".to_string());
        }
        let line_height = idx[4];
        let count = u16::from_le_bytes([idx[6], idx[7]]) as usize;
        let mut glyphs = HashMap::with_capacity(count);
        let mut off = 8;
        for _ in 0..count {
            if off + 9 > idx.len() {
                return Err("font index: truncated".to_string());
            }
            let cp = u32::from_le_bytes([idx[off], idx[off + 1], idx[off + 2], idx[off + 3]]);
            let x = u16::from_le_bytes([idx[off + 4], idx[off + 5]]);
            let w = idx[off + 6];
            let cursor_off = idx[off + 7] as i8;
            let adv = idx[off + 8];
            off += 9;
            if let Some(ch) = char::from_u32(cp) {
                glyphs.insert(
                    ch,
                    Glyph {
                        x,
                        w,
                        cursor_off,
                        adv,
                    },
                );
            }
        }
        Ok(Font {
            atlas,
            line_height,
            glyphs,
        })
    }

    pub fn get(size: FontSize) -> &'static Font {
        &FONTS[size.index()]
    }

    pub fn line_height(&self) -> i32 {
        self.line_height as i32
    }

    /// True if every character has a glyph (spaces count as covered).
    pub fn covers(&self, text: &str) -> bool {
        text.chars().all(|c| self.glyphs.contains_key(&c))
    }

    /// Advance width of `text` in pixels at `scale`.
    pub fn width(&self, text: &str, scale: i32) -> i32 {
        text.chars()
            .map(|c| self.glyphs.get(&c).map_or(0, |g| g.adv as i32))
            .sum::<i32>()
            * scale
    }

    /// Draws `text` with its top-left at (x, y), blending anti-aliased edges into whatever is
    /// under them. Returns the advance width.
    pub fn draw(&self, canvas: &mut Canvas, x: i32, y: i32, text: &str, fg: u8, scale: i32) -> i32 {
        let scale = scale.max(1);
        let mut cursor = x;
        let mut mix_cache: HashMap<(u8, u8), u8> = HashMap::new();
        for ch in text.chars() {
            let Some(g) = self.glyphs.get(&ch) else {
                continue;
            };
            if g.w > 0 {
                let left = cursor - g.cursor_off as i32 * scale;
                for gy in 0..self.line_height as i32 {
                    for gx in 0..g.w as i32 {
                        let cov = self.atlas.get_pixel(g.x as u32 + gx as u32, gy as u32).0[0];
                        if cov < 40 {
                            continue;
                        }
                        for sy in 0..scale {
                            for sx in 0..scale {
                                let px = left + gx * scale + sx;
                                let py = y + gy * scale + sy;
                                let under = canvas.get(px, py);
                                let id = if cov >= 200 {
                                    fg
                                } else if under == TRANSPARENT {
                                    if cov >= 128 {
                                        fg
                                    } else {
                                        continue;
                                    }
                                } else {
                                    let level = if cov < 110 { 1 } else { 2 };
                                    *mix_cache
                                        .entry((under, level))
                                        .or_insert_with(|| mix(fg, under, level as f32 / 3.0))
                                };
                                canvas.set(px, py, id);
                            }
                        }
                    }
                }
            }
            cursor += g.adv as i32 * scale;
        }
        cursor - x
    }

    /// Draws `text` horizontally centered on `cx`, top at `y`.
    pub fn draw_centered(
        &self,
        canvas: &mut Canvas,
        cx: i32,
        y: i32,
        text: &str,
        fg: u8,
        scale: i32,
    ) {
        let w = self.width(text, scale);
        self.draw(canvas, cx - w / 2, y, text, fg, scale);
    }
}

/// True if the bundled font can render every character of `text`.
pub fn supports(text: &str) -> bool {
    Font::get(FontSize::S18).covers(text)
}

/// A laid-out block of text: which font, integer scale and the lines to draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextLayout {
    pub size: FontSize,
    pub scale: i32,
    pub lines: Vec<String>,
}

impl TextLayout {
    pub fn font(&self) -> &'static Font {
        Font::get(self.size)
    }

    pub fn line_height(&self) -> i32 {
        self.font().line_height() * self.scale
    }

    pub fn height(&self) -> i32 {
        self.line_height() * self.lines.len() as i32
    }

    #[allow(dead_code)]
    pub fn width(&self) -> i32 {
        self.lines
            .iter()
            .map(|l| self.font().width(l, self.scale))
            .max()
            .unwrap_or(0)
    }

    /// Draws the block centered horizontally on `cx` and vertically on `cy`.
    pub fn draw_centered(&self, canvas: &mut Canvas, cx: i32, cy: i32, fg: u8) {
        let font = self.font();
        let top = cy - self.height() / 2;
        for (i, line) in self.lines.iter().enumerate() {
            font.draw_centered(
                canvas,
                cx,
                top + i as i32 * self.line_height(),
                line,
                fg,
                self.scale,
            );
        }
    }
}

/// Splits `text` into two lines at the space that balances the halves best.
fn split_two(text: &str) -> Option<(String, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut best: Option<(usize, i32)> = None;
    for (i, &c) in chars.iter().enumerate() {
        if c == ' ' && i > 0 && i + 1 < chars.len() {
            let balance = (i as i32 - (chars.len() as i32 - 1 - i as i32)).abs();
            if best.is_none_or(|(_, b)| balance < b) {
                best = Some((i, balance));
            }
        }
    }
    let (i, _) = best?;
    Some((chars[..i].iter().collect(), chars[i + 1..].iter().collect()))
}

/// Largest size whose one- or two-line layout fits `max_w` x `max_h`, capped at `largest`.
/// None when even the smallest does not fit, and the caller abbreviates.
pub fn fit_text(
    text: &str,
    max_w: i32,
    max_h: i32,
    largest: FontSize,
    allow_wrap: bool,
) -> Option<TextLayout> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut candidates: Vec<(FontSize, i32)> = Vec::new();
    for &size in FontSize::ALL.iter().rev() {
        if size > largest {
            continue;
        }
        candidates.push((size, 1));
    }
    // 2x of the largest size covers billboard-scale lettering.
    if largest == FontSize::S64 {
        candidates.insert(0, (FontSize::S64, 2));
    }
    for (size, scale) in candidates {
        let font = Font::get(size);
        let lh = font.line_height() * scale;
        if lh > max_h {
            continue;
        }
        if font.width(text, scale) <= max_w {
            return Some(TextLayout {
                size,
                scale,
                lines: vec![text.to_string()],
            });
        }
        if allow_wrap && lh * 2 <= max_h {
            if let Some((a, b)) = split_two(text) {
                if font.width(&a, scale) <= max_w && font.width(&b, scale) <= max_w {
                    return Some(TextLayout {
                        size,
                        scale,
                        lines: vec![a, b],
                    });
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decals::draw::colors;

    #[test]
    fn atlases_load_and_cover_common_scripts() {
        for size in FontSize::ALL {
            let f = Font::get(size);
            assert!(f.line_height() > 0);
            assert!(f.covers("Hauptstraße 12 Ærø Łódź Ώρα Улица"));
            assert!(!f.covers("東京"));
        }
    }

    #[test]
    fn larger_sizes_are_wider() {
        let a = Font::get(FontSize::S12).width("Bakery", 1);
        let b = Font::get(FontSize::S28).width("Bakery", 1);
        assert!(b > a);
        assert_eq!(Font::get(FontSize::S12).width("Bakery", 2), a * 2);
    }

    #[test]
    fn draw_marks_pixels_and_respects_transparency() {
        let mut c = Canvas::new(1, 1);
        let adv = Font::get(FontSize::S28).draw(&mut c, 4, 4, "AB", colors::WHITE, 1);
        assert!(adv > 20);
        let inked = (0..128)
            .flat_map(|y| (0..128).map(move |x| (x, y)))
            .filter(|&(x, y)| c.get(x, y) != TRANSPARENT)
            .count();
        assert!(inked > 100);
        // Every inked pixel is the solid foreground; no half-blended edge colors over transparency.
        for y in 0..128 {
            for x in 0..128 {
                let id = c.get(x, y);
                assert!(id == TRANSPARENT || id == colors::WHITE);
            }
        }
    }

    #[test]
    fn fit_prefers_large_and_wraps_when_needed() {
        let one = fit_text("Bäckerei", 240, 60, FontSize::S44, true).unwrap();
        assert_eq!(one.lines.len(), 1);
        let long = fit_text("Bäckerei Konditorei Müller", 200, 90, FontSize::S44, true).unwrap();
        assert!(long.width() <= 200);
        assert!(long.height() <= 90);
        assert!(fit_text("Bäckerei Konditorei Müller", 20, 10, FontSize::S44, true).is_none());
    }
}
