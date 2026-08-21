//! Turns a `DecalKey` into a canvas of map color ids.

use super::draw::{colors, Canvas, TILE};
use super::pictograms;
use super::posters;
use super::registry::DecalKey;
use super::templates;
use crate::map_item_palette::nearest_map_color;
use image::RgbImage;

/// World preview raster used for local "you are here" maps.
pub struct PreviewRaster<'a> {
    pub img: &'a RgbImage,
    pub min_x: i32,
    pub min_z: i32,
    /// Blocks per preview pixel.
    pub step: u32,
}

/// Half-size (blocks) of the area a local map board shows.
pub const LOCAL_MAP_HALF_SPAN: i32 = 96;

/// Renders `key`; `preview` is only needed for `LocalMap` keys.
pub fn render(key: &DecalKey, preview: Option<&PreviewRaster>) -> Canvas {
    let (cols, rows) = key.dims();
    let mut canvas = Canvas::new(cols, rows);
    match key {
        DecalKey::Pictogram(name) => {
            if let Some(png) = pictograms::asset(name) {
                match image::load_from_memory(png) {
                    Ok(img) => {
                        let img = if img.width() == TILE && img.height() == TILE {
                            img.to_rgba8()
                        } else {
                            image::imageops::resize(
                                &img.to_rgba8(),
                                TILE,
                                TILE,
                                image::imageops::FilterType::Triangle,
                            )
                        };
                        canvas.blit_rgba(&img, 0, 0);
                    }
                    Err(e) => {
                        eprintln!("Warning: pictogram {name} failed to decode ({e})");
                        templates::blank_plate(&mut canvas, colors::GRAY);
                    }
                }
            } else {
                templates::blank_plate(&mut canvas, colors::GRAY);
            }
        }
        DecalKey::Text { style, text, .. } => templates::text_sign(&mut canvas, *style, text),
        DecalKey::Traffic(sign) => templates::traffic_sign(&mut canvas, *sign),
        DecalKey::SpeedLimit { value, mph, style } => {
            templates::speed_limit(&mut canvas, *value, *mph, *style)
        }
        DecalKey::RouteShield { style, text } => templates::route_shield(&mut canvas, *style, text),
        DecalKey::Poster(v) => blit_poster(&mut canvas, posters::billboard(*v), "billboard"),
        DecalKey::ColumnPoster(v) => blit_poster(&mut canvas, posters::column(*v), "column"),
        DecalKey::LocalMap { x, z } => local_map(&mut canvas, *x, *z, preview),
    }
    canvas
}

/// Paints a poster over the canvas, resizing off-size art. A bad asset leaves a plate.
fn blit_poster(canvas: &mut Canvas, png: Option<&'static [u8]>, kind: &str) {
    let decoded = png
        .ok_or_else(|| format!("no bundled {kind} poster"))
        .and_then(|bytes| {
            image::load_from_memory(bytes).map_err(|e| format!("decode {kind} poster: {e}"))
        });
    match decoded {
        Ok(img) => {
            let img = if img.width() == canvas.w && img.height() == canvas.h {
                img.to_rgba8()
            } else {
                image::imageops::resize(
                    &img.to_rgba8(),
                    canvas.w,
                    canvas.h,
                    image::imageops::FilterType::Triangle,
                )
            };
            canvas.blit_rgba(&img, 0, 0);
        }
        Err(e) => {
            eprintln!("Warning: {e}");
            canvas.fill(colors::LIGHT_GRAY);
        }
    }
    canvas.stroke_rect(
        0,
        0,
        canvas.w as i32,
        canvas.h as i32,
        3,
        colors::NEAR_BLACK,
    );
}

/// "You are here" board: the preview raster around (x, z), a marker and a north arrow.
fn local_map(canvas: &mut Canvas, x: i32, z: i32, preview: Option<&PreviewRaster>) {
    let (w, h) = (canvas.w as i32, canvas.h as i32);
    let span = LOCAL_MAP_HALF_SPAN * 2;
    match preview {
        Some(p) if p.img.width() > 0 && p.img.height() > 0 => {
            for j in 0..h {
                for i in 0..w {
                    let wx = x - LOCAL_MAP_HALF_SPAN + i * span / w;
                    let wz = z - LOCAL_MAP_HALF_SPAN + j * span / h;
                    let px = (wx - p.min_x) / p.step as i32;
                    let pz = (wz - p.min_z) / p.step as i32;
                    let id = if px < 0
                        || pz < 0
                        || px >= p.img.width() as i32
                        || pz >= p.img.height() as i32
                    {
                        colors::LIGHT_GRAY
                    } else {
                        let c = p.img.get_pixel(px as u32, pz as u32).0;
                        nearest_map_color(c[0], c[1], c[2])
                    };
                    canvas.set(i, j, id);
                }
            }
        }
        _ => canvas.fill(colors::LIGHT_GRAY),
    }
    // Frame, marker and north arrow.
    canvas.stroke_rect(0, 0, w, h, 4, colors::NEAR_BLACK);
    let (cx, cy) = (w / 2, h / 2);
    canvas.disc(cx, cy, 11, colors::WHITE);
    canvas.disc(cx, cy, 8, colors::RED);
    canvas.polygon(
        &[
            ((w - 26) as f32, 12.0),
            ((w - 14) as f32, 40.0),
            ((w - 26) as f32, 32.0),
            ((w - 38) as f32, 40.0),
        ],
        colors::WHITE,
    );
    canvas.polygon(
        &[
            ((w - 26) as f32, 16.0),
            ((w - 18) as f32, 36.0),
            ((w - 26) as f32, 30.0),
            ((w - 34) as f32, 36.0),
        ],
        colors::RED,
    );
    super::font::Font::get(super::font::FontSize::S18).draw_centered(
        canvas,
        w - 26,
        42,
        "N",
        colors::WHITE,
        1,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decals::registry::{TextStyle, TrafficSign};
    use crate::map_item_palette::TRANSPARENT;

    #[test]
    fn pictogram_renders_with_transparent_corners() {
        let c = render(&DecalKey::Pictogram("cafe"), None);
        assert_eq!(c.get(0, 0), TRANSPARENT);
        assert_ne!(c.get(64, 64), TRANSPARENT);
    }

    #[test]
    fn dims_match_canvas() {
        let c = render(&DecalKey::Poster(2), None);
        assert_eq!((c.w, c.h), (384, 256));
        let c = render(&DecalKey::text(TextStyle::Fascia, "Café", 2), None);
        assert_eq!((c.w, c.h), (256, 128));
        let c = render(&DecalKey::Traffic(TrafficSign::Stop), None);
        assert_eq!((c.w, c.h), (128, 128));
    }

    /// Writes a contact sheet of generated signs to ARNIS_DECAL_PREVIEW (a PNG path).
    /// Run with `cargo test decals::render::tests::dump_preview_sheet -- --ignored`.
    #[test]
    #[ignore]
    fn dump_preview_sheet() {
        use crate::decals::region::BladeStyle;
        use crate::decals::registry::{ShieldStyle, SpeedStyle};
        let Ok(path) = std::env::var("ARNIS_DECAL_PREVIEW") else {
            return;
        };
        let keys = vec![
            DecalKey::text(TextStyle::StreetName(BladeStyle::Blue), "Hauptstraße", 1),
            DecalKey::text(
                TextStyle::StreetName(BladeStyle::Green),
                "W Washington Blvd",
                1,
            ),
            DecalKey::text(TextStyle::StreetName(BladeStyle::White), "Baker Street", 1),
            DecalKey::text(TextStyle::HouseNumber, "12a", 1),
            DecalKey::text(TextStyle::Fascia, "Bäckerei Müller", 2),
            DecalKey::text(TextStyle::Fascia, "Café Extrablatt", 2),
            DecalKey::text(TextStyle::StationBoard, "Berlin Hauptbahnhof", 3),
            DecalKey::text(TextStyle::StopName, "Rathaus Steglitz", 2),
            DecalKey::Traffic(TrafficSign::Stop),
            DecalKey::Traffic(TrafficSign::GiveWay),
            DecalKey::Traffic(TrafficSign::NoEntry),
            DecalKey::Traffic(TrafficSign::PriorityRoad),
            DecalKey::Traffic(TrafficSign::Crossing),
            DecalKey::Traffic(TrafficSign::OneWay),
            DecalKey::Traffic(TrafficSign::NoParking),
            DecalKey::Traffic(TrafficSign::DeadEnd),
            DecalKey::Traffic(TrafficSign::LevelCrossing),
            DecalKey::Traffic(TrafficSign::HighVoltage),
            DecalKey::Traffic(TrafficSign::Bicycle),
            DecalKey::SpeedLimit {
                value: 30,
                mph: false,
                style: SpeedStyle::Disc,
            },
            DecalKey::SpeedLimit {
                value: 120,
                mph: false,
                style: SpeedStyle::Disc,
            },
            DecalKey::SpeedLimit {
                value: 30,
                mph: true,
                style: SpeedStyle::Disc,
            },
            DecalKey::SpeedLimit {
                value: 45,
                mph: true,
                style: SpeedStyle::UsPlate,
            },
            DecalKey::RouteShield {
                style: ShieldStyle::Blue,
                text: "A 9".into(),
            },
            DecalKey::RouteShield {
                style: ShieldStyle::Yellow,
                text: "B 2".into(),
            },
            DecalKey::RouteShield {
                style: ShieldStyle::Green,
                text: "A40".into(),
            },
            DecalKey::RouteShield {
                style: ShieldStyle::Interstate,
                text: "95".into(),
            },
            DecalKey::RouteShield {
                style: ShieldStyle::White,
                text: "101".into(),
            },
            DecalKey::Poster(0),
            DecalKey::Poster(2),
            DecalKey::Poster(4),
            DecalKey::ColumnPoster(0),
            DecalKey::ColumnPoster(3),
            DecalKey::Pictogram("cafe"),
            DecalKey::LocalMap { x: 0, z: 0 },
        ];
        let cell = 130u32;
        let cols = 8u32;
        let mut sheet = image::RgbaImage::from_pixel(
            cols * cell * 3,
            12 * cell,
            image::Rgba([90, 90, 90, 255]),
        );
        let (mut cx, mut cy, mut row_h) = (0u32, 0u32, 0u32);
        for key in keys {
            let c = render(&key, None).to_rgba();
            if cx + c.width() > sheet.width() {
                cx = 0;
                cy += row_h + 4;
                row_h = 0;
            }
            image::imageops::overlay(&mut sheet, &c, cx as i64, cy as i64);
            cx += c.width() + 4;
            row_h = row_h.max(c.height());
        }
        sheet.save(path).unwrap();
    }

    #[test]
    fn local_map_without_preview_still_draws_a_marker() {
        let c = render(&DecalKey::LocalMap { x: 10, z: 10 }, None);
        assert_eq!(c.get(128, 128), colors::RED);
        assert_eq!(c.get(64, 64), colors::LIGHT_GRAY);
    }

    #[test]
    fn local_map_samples_the_preview() {
        let mut img = RgbImage::from_pixel(64, 64, image::Rgb([0, 124, 0]));
        // Blue lake at the top-left quadrant of the raster.
        for y in 0..20 {
            for x in 0..20 {
                img.put_pixel(x, y, image::Rgb([64, 64, 255]));
            }
        }
        let raster = PreviewRaster {
            img: &img,
            min_x: 0,
            min_z: 0,
            step: 4,
        };
        // Board centred at (96, 96): world x in 0..192 -> raster px 0..48.
        let c = render(&DecalKey::LocalMap { x: 96, z: 96 }, Some(&raster));
        assert_eq!(c.get(20, 20), nearest_map_color(64, 64, 255));
        assert_eq!(c.get(200, 200), nearest_map_color(0, 124, 0));
    }
}
