//! Renderers for generated signage. One tile is 128 px = 1 m at scale 1, so the plate sizes
//! here are chosen to match real signs.

use super::draw::{colors::*, darker, Canvas, TILE};
use super::font::{fit_text, Font, FontSize, TextLayout};
use super::region::BladeStyle;
use super::registry::{ShieldStyle, SpeedStyle, TextStyle, TrafficSign};

/// Deterministic small hash for picking a scheme from a string.
fn str_hash(s: &str) -> u32 {
    s.bytes().fold(0x811C_9DC5u32, |h, b| {
        (h ^ b as u32).wrapping_mul(0x0100_0193)
    })
}

/// Suffix abbreviations applied when a name does not fit its plate.
const ABBREVIATIONS: &[(&str, &str)] = &[
    ("straße", "str."),
    ("Straße", "Str."),
    ("strasse", "str."),
    ("Strasse", "Str."),
    ("straat", "str."),
    ("Straat", "Str."),
    ("gasse", "g."),
    ("Gasse", "G."),
    ("platz", "pl."),
    ("Platz", "Pl."),
    ("Boulevard", "Blvd"),
    ("boulevard", "blvd"),
    ("Avenue", "Ave"),
    ("avenue", "ave"),
    ("Avenida", "Av."),
    ("Street", "St"),
    ("street", "st"),
    ("Road", "Rd"),
    ("Drive", "Dr"),
    ("Lane", "Ln"),
    ("Place", "Pl"),
    ("Court", "Ct"),
    ("Square", "Sq"),
    ("Terrace", "Ter"),
    ("Crescent", "Cres"),
    ("Highway", "Hwy"),
    ("Parkway", "Pkwy"),
    ("Gardens", "Gdns"),
    ("Chaussee", "Ch."),
    ("Promenade", "Prom."),
    ("Bulevar", "Bul."),
    ("Bulevardul", "Bd."),
    ("Calle", "C/"),
    ("Carrer", "C/"),
];

/// Shortens a name by abbreviating known suffixes/words. Returns None if nothing applied.
pub fn abbreviate(name: &str) -> Option<String> {
    let mut out = name.to_string();
    let mut changed = false;
    for (long, short) in ABBREVIATIONS {
        // Word-level or suffix-level replacement, whichever occurs.
        let words: Vec<String> = out
            .split(' ')
            .map(|w| {
                if w == *long {
                    changed = true;
                    (*short).to_string()
                } else if w.ends_with(long) && w.len() > long.len() {
                    changed = true;
                    format!("{}{}", &w[..w.len() - long.len()], short)
                } else {
                    w.to_string()
                }
            })
            .collect();
        out = words.join(" ");
    }
    changed.then_some(out)
}

/// Fits `text` (abbreviating if needed) into a box; None when even the shortest form fails.
fn fit_or_abbreviate(
    text: &str,
    max_w: i32,
    max_h: i32,
    largest: FontSize,
    wrap: bool,
) -> Option<TextLayout> {
    if let Some(l) = fit_text(text, max_w, max_h, largest, wrap) {
        return Some(l);
    }
    if let Some(short) = abbreviate(text) {
        if let Some(l) = fit_text(&short, max_w, max_h, largest, wrap) {
            return Some(l);
        }
    }
    // Last resort: truncate with an ellipsis so the plate is never blank.
    let mut chars: Vec<char> = text.chars().collect();
    while chars.len() > 3 {
        chars.truncate(chars.len() - 2);
        let s: String = chars.iter().collect::<String>().trim_end().to_string() + "…";
        if let Some(l) = fit_text(&s, max_w, max_h, largest, false) {
            return Some(l);
        }
    }
    None
}

/// Draws a text sign of the given style spanning the whole canvas.
pub fn text_sign(canvas: &mut Canvas, style: TextStyle, text: &str) {
    let w = canvas.w as i32;
    let cx = w / 2;
    match style {
        TextStyle::Fascia => {
            // Scheme by name so a chain looks the same on every branch.
            let schemes: [(u8, u8, u8); 6] = [
                (NEAR_BLACK, WHITE, GRAY),
                (DARK_RED, WHITE, GOLD),
                (DARK_BLUE, WHITE, LIGHT_GRAY),
                (SIGN_GREEN, GOLD, GOLD),
                (WHITE, BLACK, DARK_GRAY),
                (DARK_GRAY, GOLD, GOLD),
            ];
            let (plate, fg, border) = schemes[(str_hash(text) % 6) as usize];
            let (y0, h) = (30, 68);
            canvas.rounded_rect(4, y0, w - 8, h, 6, plate);
            canvas.stroke_rounded_rect(4, y0, w - 8, h, 6, 2, border);
            if let Some(layout) = fit_or_abbreviate(text, w - 24, h - 12, FontSize::S44, true) {
                layout.draw_centered(canvas, cx, y0 + h / 2, fg);
            }
        }
        TextStyle::StreetName(blade) => {
            let (plate, fg, border) = match blade {
                BladeStyle::Blue => (BLUE, WHITE, WHITE),
                BladeStyle::Green => (SIGN_GREEN, WHITE, WHITE),
                BladeStyle::White => (WHITE, BLACK, BLACK),
            };
            // The blade hangs on a bottom slab, which fills only the lower half of the block
            // face, so the art sits in rows 64..128 to line up with it.
            let (y0, h) = (74, 44);
            canvas.rounded_rect(4, y0, w - 8, h, 4, plate);
            canvas.stroke_rounded_rect(4, y0, w - 8, h, 4, 3, border);
            if let Some(layout) = fit_or_abbreviate(text, w - 20, h - 8, FontSize::S28, true) {
                layout.draw_centered(canvas, cx, y0 + h / 2, fg);
            }
        }
        TextStyle::HouseNumber => {
            let font = Font::get(FontSize::S28);
            let (layout, tw) = if font.width(text, 1) <= 80 {
                (
                    TextLayout {
                        size: FontSize::S28,
                        scale: 1,
                        lines: vec![text.to_string()],
                    },
                    font.width(text, 1),
                )
            } else {
                let f = Font::get(FontSize::S18);
                (
                    TextLayout {
                        size: FontSize::S18,
                        scale: 1,
                        lines: vec![text.to_string()],
                    },
                    f.width(text, 1),
                )
            };
            let pw = (tw + 22).max(40).min(w - 4);
            let ph = 40;
            canvas.rounded_rect(cx - pw / 2, 44, pw, ph, 3, WHITE);
            canvas.stroke_rounded_rect(cx - pw / 2, 44, pw, ph, 3, 2, BLACK);
            layout.draw_centered(canvas, cx, 44 + ph / 2, BLACK);
        }
        TextStyle::StationBoard => {
            let (y0, h) = (36, 56);
            canvas.rounded_rect(2, y0, w - 4, h, 4, DARK_BLUE);
            canvas.stroke_rounded_rect(2, y0, w - 4, h, 4, 3, WHITE);
            if let Some(layout) = fit_or_abbreviate(text, w - 30, h - 14, FontSize::S44, false) {
                layout.draw_centered(canvas, cx, y0 + h / 2, WHITE);
            }
        }
        TextStyle::StopName => {
            let (y0, h) = (46, 36);
            canvas.rounded_rect(4, y0, w - 8, h, 3, WHITE);
            canvas.stroke_rounded_rect(4, y0, w - 8, h, 3, 2, DARK_GRAY);
            if let Some(layout) = fit_or_abbreviate(text, w - 20, h - 8, FontSize::S28, false) {
                layout.draw_centered(canvas, cx, y0 + h / 2, BLACK);
            }
        }
        TextStyle::Plaque => {
            let (y0, h) = (28, 72);
            canvas.rounded_rect(8, y0, w - 16, h, 4, LIGHT_GRAY);
            canvas.stroke_rounded_rect(8, y0, w - 16, h, 4, 3, GRAY);
            if let Some(layout) = fit_or_abbreviate(text, w - 28, h - 14, FontSize::S18, true) {
                layout.draw_centered(canvas, cx, y0 + h / 2, NEAR_BLACK);
            }
        }
    }
}

/// Simplified walking figure used on the crossing sign.
fn stick_figure(canvas: &mut Canvas, cx: i32, top: i32, h: i32, id: u8) {
    let head_r = h / 9;
    canvas.disc(cx, top + head_r, head_r, id);
    let body_top = top + head_r * 2 + 1;
    let hip = top + h * 55 / 100;
    canvas.line(cx, body_top, cx, hip, 4, id);
    canvas.line(cx, hip, cx - h / 5, top + h, 4, id);
    canvas.line(cx, hip, cx + h / 6, top + h * 78 / 100, 4, id);
    canvas.line(cx + h / 6, top + h * 78 / 100, cx + h / 8, top + h, 4, id);
    canvas.line(cx, body_top + 4, cx - h / 5, top + h * 42 / 100, 4, id);
    canvas.line(cx, body_top + 4, cx + h / 4, top + h * 36 / 100, 4, id);
}

fn arrow_right(canvas: &mut Canvas, x0: i32, x1: i32, cy: i32, shaft: i32, head: i32, id: u8) {
    canvas.fill_rect(x0, cy - shaft / 2, x1 - head - x0, shaft, id);
    canvas.polygon(
        &[
            ((x1 - head) as f32, (cy - head) as f32),
            (x1 as f32, cy as f32 + 0.5),
            ((x1 - head) as f32, (cy + head + 1) as f32),
        ],
        id,
    );
}

fn bicycle(canvas: &mut Canvas, cx: i32, cy: i32, r: i32, id: u8) {
    let ring_r = r * 22 / 40;
    let ring_w = (r / 12).max(2);
    canvas.ring(cx - r + ring_r, cy + r / 3, ring_r, ring_r - ring_w, id);
    canvas.ring(cx + r - ring_r, cy + r / 3, ring_r, ring_r - ring_w, id);
    let lw = (r / 10).max(2);
    let (ax, ay) = (cx - r + ring_r, cy + r / 3);
    let (bx, by) = (cx + r - ring_r, cy + r / 3);
    let (px, py) = (cx - r / 8, cy + r / 3);
    let (tx, ty) = (cx - r / 4, cy - r / 2);
    let (hx, hy) = (cx + r / 2, cy - r / 2);
    canvas.line(ax, ay, tx, ty, lw, id);
    canvas.line(tx, ty, hx, hy, lw, id);
    canvas.line(hx, hy, bx, by, lw, id);
    canvas.line(tx, ty, px, py, lw, id);
    canvas.line(px, py, hx, hy, lw, id);
    canvas.line(hx, hy, hx + r / 8, hy - r / 6, lw, id);
    canvas.line(tx - r / 6, ty - r / 10, tx + r / 8, ty - r / 10, lw, id);
}

/// One-tile traffic sign.
pub fn traffic_sign(canvas: &mut Canvas, sign: TrafficSign) {
    let (cx, cy) = (64, 64);
    match sign {
        TrafficSign::Stop => {
            let rot = std::f32::consts::PI / 8.0;
            canvas.regular_polygon(cx, cy, 60.0, 8, rot, WHITE);
            canvas.regular_polygon(cx, cy, 55.0, 8, rot, RED);
            Font::get(FontSize::S28).draw_centered(canvas, cx, cy - 16, "STOP", WHITE, 1);
        }
        TrafficSign::GiveWay => {
            canvas.polygon(&[(6.0, 18.0), (122.0, 18.0), (64.0, 118.0)], RED);
            canvas.polygon(&[(24.0, 28.0), (104.0, 28.0), (64.0, 98.0)], WHITE);
        }
        TrafficSign::NoEntry => {
            canvas.disc(cx, cy, 58, WHITE);
            canvas.disc(cx, cy, 54, RED);
            canvas.rounded_rect(24, cy - 10, 80, 20, 3, WHITE);
        }
        TrafficSign::PriorityRoad => {
            canvas.regular_polygon(cx, cy, 60.0, 4, 0.0, WHITE);
            canvas.regular_polygon(cx, cy, 50.0, 4, 0.0, YELLOW);
            canvas.regular_polygon(cx, cy, 44.0, 4, 0.0, YELLOW);
        }
        TrafficSign::Crossing => {
            canvas.rounded_rect(12, 12, 104, 104, 6, BLUE);
            canvas.polygon(&[(64.0, 20.0), (112.0, 108.0), (16.0, 108.0)], WHITE);
            stick_figure(canvas, 66, 50, 54, BLACK);
        }
        TrafficSign::OneWay => {
            canvas.rounded_rect(4, 40, 120, 48, 4, BLUE);
            canvas.stroke_rounded_rect(4, 40, 120, 48, 4, 2, WHITE);
            arrow_right(canvas, 18, 112, 64, 12, 20, WHITE);
        }
        TrafficSign::NoParking => {
            canvas.disc(cx, cy, 58, RED);
            canvas.disc(cx, cy, 48, BLUE);
            canvas.line(28, 28, 100, 100, 12, RED);
        }
        TrafficSign::DeadEnd => {
            canvas.rounded_rect(12, 12, 104, 104, 4, BLUE);
            canvas.stroke_rounded_rect(12, 12, 104, 104, 4, 2, WHITE);
            canvas.fill_rect(56, 40, 16, 68, WHITE);
            canvas.fill_rect(30, 28, 68, 14, RED);
        }
        TrafficSign::LevelCrossing => {
            // Saint Andrew's cross: two white boards with red tips.
            for flip in [1.0f32, -1.0] {
                let pts: Vec<(f32, f32)> = [(-56.0, -8.0), (56.0, -8.0), (56.0, 8.0), (-56.0, 8.0)]
                    .iter()
                    .map(|(x, y)| {
                        let (s, c) = (0.62f32.sin() * flip, 0.62f32.cos());
                        (64.0 + x * c - y * s, 64.0 + x * s + y * c)
                    })
                    .collect();
                canvas.polygon(&pts, WHITE);
            }
            for (x, y) in [(14, 33), (114, 33), (14, 95), (114, 95)] {
                canvas.disc(x, y, 7, RED);
            }
        }
        TrafficSign::HighVoltage => {
            canvas.polygon(&[(64.0, 8.0), (122.0, 116.0), (6.0, 116.0)], BLACK);
            canvas.polygon(&[(64.0, 20.0), (112.0, 110.0), (16.0, 110.0)], YELLOW);
            canvas.polygon(
                &[
                    (70.0, 44.0),
                    (52.0, 76.0),
                    (64.0, 76.0),
                    (56.0, 102.0),
                    (80.0, 66.0),
                    (68.0, 66.0),
                    (78.0, 44.0),
                ],
                BLACK,
            );
        }
        TrafficSign::Bicycle => {
            canvas.disc(cx, cy, 58, WHITE);
            canvas.disc(cx, cy, 54, BLUE);
            bicycle(canvas, cx, cy, 40, WHITE);
        }
        TrafficSign::Motorway => {
            canvas.rounded_rect(12, 12, 104, 104, 4, BLUE);
            canvas.stroke_rounded_rect(12, 12, 104, 104, 4, 2, WHITE);
            canvas.fill_rect(44, 30, 10, 74, WHITE);
            canvas.fill_rect(74, 30, 10, 74, WHITE);
            canvas.polygon(
                &[(30.0, 58.0), (98.0, 58.0), (92.0, 68.0), (36.0, 68.0)],
                WHITE,
            );
        }
        TrafficSign::MotorwayEnd => {
            traffic_sign(canvas, TrafficSign::Motorway);
            canvas.line(24, 104, 104, 24, 10, RED);
        }
    }
}

/// Speed limit sign: red-ringed disc, US "SPEED LIMIT" plate or Canadian "MAXIMUM" plate.
pub fn speed_limit(canvas: &mut Canvas, value: u16, mph: bool, style: SpeedStyle) {
    let text = value.to_string();
    if style != SpeedStyle::Disc {
        canvas.rounded_rect(24, 6, 80, 116, 3, WHITE);
        canvas.stroke_rounded_rect(24, 6, 80, 116, 3, 3, BLACK);
        let small = Font::get(FontSize::S12);
        if style == SpeedStyle::CaPlate {
            small.draw_centered(canvas, 64, 22, "MAXIMUM", BLACK, 1);
        } else {
            small.draw_centered(canvas, 64, 14, "SPEED", BLACK, 1);
            small.draw_centered(canvas, 64, 30, "LIMIT", BLACK, 1);
        }
        let size = if text.len() > 2 {
            FontSize::S28
        } else {
            FontSize::S44
        };
        let f = Font::get(size);
        f.draw_centered(canvas, 64, 90 - f.line_height() / 2, &text, BLACK, 1);
        if style == SpeedStyle::CaPlate {
            small.draw_centered(canvas, 64, 108, "km/h", BLACK, 1);
        }
    } else {
        canvas.disc(64, 64, 58, RED);
        canvas.disc(64, 64, 46, WHITE);
        let size = if text.len() > 2 {
            FontSize::S28
        } else {
            FontSize::S44
        };
        let f = Font::get(size);
        let y = if mph { 60 } else { 64 };
        f.draw_centered(canvas, 64, y - f.line_height() / 2, &text, BLACK, 1);
        if mph {
            Font::get(FontSize::S12).draw_centered(canvas, 64, 88, "mph", BLACK, 1);
        }
    }
}

/// Route number shield.
pub fn route_shield(canvas: &mut Canvas, style: ShieldStyle, text: &str) {
    let (cx, cy) = (64, 64);
    let fit = |max_w: i32, max_h: i32| fit_text(text, max_w, max_h, FontSize::S44, false);
    match style {
        ShieldStyle::Blue | ShieldStyle::Yellow | ShieldStyle::Green => {
            let (plate, fg, border) = match style {
                ShieldStyle::Blue => (BLUE, WHITE, WHITE),
                ShieldStyle::Yellow => (YELLOW, BLACK, BLACK),
                _ => (SIGN_GREEN, WHITE, WHITE),
            };
            let (w, h) = (110, 64);
            canvas.rounded_rect(cx - w / 2, cy - h / 2, w, h, 6, plate);
            canvas.stroke_rounded_rect(cx - w / 2, cy - h / 2, w, h, 6, 3, border);
            if let Some(l) = fit(w - 16, h - 12) {
                l.draw_centered(canvas, cx, cy, fg);
            }
        }
        ShieldStyle::Interstate => {
            let shield = [
                (14.0, 20.0),
                (114.0, 20.0),
                (110.0, 60.0),
                (96.0, 96.0),
                (64.0, 116.0),
                (32.0, 96.0),
                (18.0, 60.0),
            ];
            canvas.polygon(&shield, WHITE);
            let inner: Vec<(f32, f32)> = shield
                .iter()
                .map(|(x, y)| (64.0 + (x - 64.0) * 0.92, 66.0 + (y - 66.0) * 0.92))
                .collect();
            canvas.polygon(&inner, BLUE);
            canvas.polygon(
                &[(19.0, 24.0), (109.0, 24.0), (107.0, 44.0), (21.0, 44.0)],
                RED,
            );
            if let Some(l) = fit(76, 44) {
                l.draw_centered(canvas, cx, 76, WHITE);
            }
        }
        ShieldStyle::White => {
            let shield = [
                (24.0, 16.0),
                (104.0, 16.0),
                (112.0, 30.0),
                (104.0, 92.0),
                (64.0, 116.0),
                (24.0, 92.0),
                (16.0, 30.0),
            ];
            canvas.polygon(&shield, BLACK);
            let inner: Vec<(f32, f32)> = shield
                .iter()
                .map(|(x, y)| (64.0 + (x - 64.0) * 0.9, 66.0 + (y - 66.0) * 0.9))
                .collect();
            canvas.polygon(&inner, WHITE);
            if let Some(l) = fit(72, 56) {
                l.draw_centered(canvas, cx, cy, BLACK);
            }
        }
    }
}

/// Fills a tile with a solid pictogram-style plate for asset-less fallbacks (unused icons).
#[allow(dead_code)]
pub fn blank_plate(canvas: &mut Canvas, color: u8) {
    canvas.rounded_rect(6, 6, TILE as i32 - 12, TILE as i32 - 12, 22, color);
    canvas.stroke_rounded_rect(
        6,
        6,
        TILE as i32 - 12,
        TILE as i32 - 12,
        22,
        3,
        darker(color),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_item_palette::TRANSPARENT;

    fn inked(c: &Canvas) -> usize {
        (0..c.h as i32)
            .flat_map(|y| (0..c.w as i32).map(move |x| (x, y)))
            .filter(|&(x, y)| c.get(x, y) != TRANSPARENT)
            .count()
    }

    #[test]
    fn abbreviation_shortens_common_suffixes() {
        assert_eq!(abbreviate("Hauptstraße").as_deref(), Some("Hauptstr."));
        assert_eq!(abbreviate("Main Street").as_deref(), Some("Main St"));
        assert_eq!(abbreviate("Rue de Rivoli"), None);
    }

    #[test]
    fn street_blade_keeps_margins_transparent() {
        let mut c = Canvas::new(1, 1);
        text_sign(
            &mut c,
            TextStyle::StreetName(BladeStyle::Blue),
            "Hauptstraße",
        );
        // Art stays inside the lower half of the tile, where the slab is.
        assert_eq!(c.get(64, 4), TRANSPARENT);
        assert_eq!(c.get(64, 60), TRANSPARENT);
        assert_eq!(c.get(64, 124), TRANSPARENT);
        assert_eq!(c.get(64, 76), WHITE);
        assert!(inked(&c) > 3000);
    }

    #[test]
    fn every_traffic_sign_and_marking_draws_something() {
        for sign in [
            TrafficSign::Stop,
            TrafficSign::GiveWay,
            TrafficSign::NoEntry,
            TrafficSign::PriorityRoad,
            TrafficSign::Crossing,
            TrafficSign::OneWay,
            TrafficSign::NoParking,
            TrafficSign::DeadEnd,
            TrafficSign::LevelCrossing,
            TrafficSign::HighVoltage,
            TrafficSign::Bicycle,
            TrafficSign::Motorway,
            TrafficSign::MotorwayEnd,
        ] {
            let mut c = Canvas::new(1, 1);
            traffic_sign(&mut c, sign);
            assert!(inked(&c) > 1500, "{sign:?}");
        }
    }

    #[test]
    fn speed_and_shields_render() {
        let mut c = Canvas::new(1, 1);
        speed_limit(&mut c, 30, false, SpeedStyle::Disc);
        assert_eq!(c.get(64, 10), RED);
        assert!(inked(&c) > 8000);
        let mut c = Canvas::new(1, 1);
        speed_limit(&mut c, 45, true, SpeedStyle::UsPlate);
        assert_eq!(c.get(64, 7), BLACK);
        assert_eq!(c.get(64, 11), WHITE);
        let mut c = Canvas::new(1, 1);
        speed_limit(&mut c, 50, false, SpeedStyle::CaPlate);
        assert!(inked(&c) > 6000);
        for style in [
            ShieldStyle::Blue,
            ShieldStyle::Yellow,
            ShieldStyle::Green,
            ShieldStyle::Interstate,
            ShieldStyle::White,
        ] {
            let mut c = Canvas::new(1, 1);
            route_shield(&mut c, style, "A 9");
            assert!(inked(&c) > 3000, "{style:?}");
        }
    }

    #[test]
    fn long_names_still_render_on_a_fascia() {
        let mut c = Canvas::new(2, 1);
        text_sign(
            &mut c,
            TextStyle::Fascia,
            "Bäckerei und Konditorei Müller-Lüdenscheidt GmbH & Co. KG",
        );
        assert!(inked(&c) > 5000);
    }
}
