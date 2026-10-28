//! Vehicle bodies as Bedrock geometry: every box a cube coloured from one shared palette
//! texture. Bones whose name starts with `lit` are drawn emissive by the render controller.

use super::{Lamp, Shape};
use image::{ImageBuffer, ImageFormat, Rgba};
use serde_json::{json, Value};
use std::io::Cursor;

pub const TEXTURE_WIDTH: u32 = 128;
pub const TEXTURE_HEIGHT: u32 = 64;
/// Each colour fills a cell this many texels wide; faces sample only its middle, so the
/// lower mip levels keep it clean.
const CELL: u32 = 8;
const PER_ROW: u32 = TEXTURE_WIDTH / CELL;

/// The blocks vehicles are built from, each a cell of the palette.
const BLOCKS: [&str; 34] = [
    "minecraft:white_concrete",
    "minecraft:orange_concrete",
    "minecraft:magenta_concrete",
    "minecraft:light_blue_concrete",
    "minecraft:yellow_concrete",
    "minecraft:lime_concrete",
    "minecraft:pink_concrete",
    "minecraft:gray_concrete",
    "minecraft:light_gray_concrete",
    "minecraft:cyan_concrete",
    "minecraft:purple_concrete",
    "minecraft:blue_concrete",
    "minecraft:brown_concrete",
    "minecraft:green_concrete",
    "minecraft:red_concrete",
    "minecraft:black_concrete",
    "minecraft:white_terracotta",
    "minecraft:orange_terracotta",
    "minecraft:magenta_terracotta",
    "minecraft:light_blue_terracotta",
    "minecraft:yellow_terracotta",
    "minecraft:lime_terracotta",
    "minecraft:pink_terracotta",
    "minecraft:gray_terracotta",
    "minecraft:light_gray_terracotta",
    "minecraft:cyan_terracotta",
    "minecraft:purple_terracotta",
    "minecraft:blue_terracotta",
    "minecraft:brown_terracotta",
    "minecraft:green_terracotta",
    "minecraft:red_terracotta",
    "minecraft:black_terracotta",
    "minecraft:terracotta",
    "minecraft:tinted_glass",
];

/// Grey stands in for a block the list lacks.
const FALLBACK: usize = 7;
/// The map renders tinted glass as see-through, so it has no colour there.
const TINTED_GLASS: [u8; 3] = [44, 38, 46];

/// A palette block's colour, as the map preview draws it.
fn colour(block: &str) -> [u8; 3] {
    if block == "minecraft:tinted_glass" {
        return TINTED_GLASS;
    }
    let name = block.trim_start_matches("minecraft:");
    crate::map_renderer::block_colour(name).unwrap_or([128, 128, 128])
}

/// How a palette colour is drawn.
#[derive(Clone, Copy)]
enum Shade {
    Plain,
    /// A brake light while the car drives on, as the Java pack lights it at 7 of 15.
    Dim,
    /// Fully transparent, which an emissive material reads as fully lit.
    Lit,
}

fn index(block: &str) -> usize {
    BLOCKS.iter().position(|b| *b == block).unwrap_or(FALLBACK)
}

fn cell(block: &str, shade: Shade) -> u32 {
    index(block) as u32 * 3 + shade as u32
}

/// Whether the palette has a colour of its own for `block`.
#[cfg(test)]
pub fn knows(block: &str) -> bool {
    BLOCKS.contains(&block)
}

/// The palette as a PNG.
pub fn texture() -> Vec<u8> {
    let mut img = ImageBuffer::from_pixel(TEXTURE_WIDTH, TEXTURE_HEIGHT, Rgba([0, 0, 0, 0]));
    for (i, block) in BLOCKS.iter().enumerate() {
        let rgb = colour(block);
        for shade in [Shade::Plain, Shade::Dim, Shade::Lit] {
            let c = i as u32 * 3 + shade as u32;
            let (x0, y0) = ((c % PER_ROW) * CELL, (c / PER_ROW) * CELL);
            let px = match shade {
                Shade::Plain => Rgba([rgb[0], rgb[1], rgb[2], 255]),
                Shade::Dim => {
                    let dim = rgb.map(|v| (v as u32 * 9 / 16) as u8);
                    Rgba([dim[0], dim[1], dim[2], 255])
                }
                Shade::Lit => Rgba([rgb[0], rgb[1], rgb[2], 0]),
            };
            for y in y0..y0 + CELL {
                for x in x0..x0 + CELL {
                    img.put_pixel(x, y, px);
                }
            }
        }
    }
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png)
        .expect("encoding a PNG in memory cannot fail");
    out.into_inner()
}

fn round(v: f32) -> f64 {
    (v as f64 * 1000.0).round() / 1000.0
}

/// One box as a cube, in the geometry's sixteenths of a block. The geometry faces -z, so
/// z is mirrored; x is not, since Bedrock puts an entity's right side at -x as Java does.
fn cube(s: &Shape, cell: u32) -> Value {
    let px = |v: f32| round(v * 16.0);
    let (u, v) = (
        (cell % PER_ROW) * CELL + CELL / 4,
        (cell / PER_ROW) * CELL + CELL / 4,
    );
    let face = json!({ "uv": [u, v], "uv_size": [CELL / 2, CELL / 2] });
    let mut out = json!({
        "origin": [px(s.corner[0]), px(s.corner[1]), px(-(s.corner[2] + s.size[2]))],
        "size": [px(s.size[0]), px(s.size[1]), px(s.size[2])],
        "uv": {
            "north": face, "south": face, "east": face,
            "west": face, "up": face, "down": face,
        },
    });
    if s.tilt != 0.0 {
        // Java turns a box round its lowest corner, which lies at the far end in this frame.
        out["pivot"] = json!([px(s.corner[0]), px(s.corner[1]), px(-s.corner[2])]);
        out["rotation"] = json!([round(s.tilt.to_degrees()), 0.0, 0.0]);
    }
    out
}

/// The geometry file of a body named `name`.
pub fn geometry(name: &str, shapes: &[Shape]) -> Value {
    let bone = |bone: &str, pick: &dyn Fn(&Shape) -> Option<Shade>| {
        let cubes: Vec<Value> = shapes
            .iter()
            .filter_map(|s| pick(s).map(|shade| cube(s, cell(s.block, shade))))
            .collect();
        (!cubes.is_empty()).then(|| json!({ "name": bone, "pivot": [0, 0, 0], "cubes": cubes }))
    };
    let bones: Vec<Value> = [
        bone("body", &|s| (s.lamp == Lamp::Off).then_some(Shade::Plain)),
        bone("lit", &|s| (s.lamp == Lamp::On).then_some(Shade::Lit)),
        bone("brake_off", &|s| {
            (s.lamp == Lamp::Brake).then_some(Shade::Dim)
        }),
        bone("lit_brake", &|s| {
            (s.lamp == Lamp::Brake).then_some(Shade::Lit)
        }),
    ]
    .into_iter()
    .flatten()
    .collect();
    // In blocks, with a block to spare so a tilted plate never pokes out and gets culled.
    let reach = shapes
        .iter()
        .flat_map(|s| {
            [
                s.corner[0].abs(),
                (s.corner[0] + s.size[0]).abs(),
                s.corner[2].abs(),
                (s.corner[2] + s.size[2]).abs(),
            ]
        })
        .fold(0.0f32, f32::max);
    let top = shapes
        .iter()
        .map(|s| s.corner[1] + s.size[1])
        .fold(0.0f32, f32::max);
    let (width, height) = (2.0 * reach + 1.0, top + 1.0);
    json!({
        "format_version": "1.12.0",
        "minecraft:geometry": [{
            "description": {
                "identifier": format!("geometry.arnis.{name}"),
                "texture_width": TEXTURE_WIDTH,
                "texture_height": TEXTURE_HEIGHT,
                "visible_bounds_width": round(width.ceil()),
                "visible_bounds_height": round(height.ceil()),
                "visible_bounds_offset": [0.0, round(height / 2.0), 0.0],
            },
            "bones": bones,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(block: &'static str, lamp: Lamp) -> Shape {
        Shape {
            block,
            size: [2.0, 1.0, 4.0],
            corner: [-1.0, 0.25, -2.0],
            tilt: 0.0,
            lamp,
        }
    }

    #[test]
    fn every_palette_block_has_a_map_colour() {
        for block in BLOCKS.iter().filter(|b| **b != "minecraft:tinted_glass") {
            let name = block.trim_start_matches("minecraft:");
            assert!(crate::map_renderer::block_colour(name).is_some(), "{block}");
        }
    }

    #[test]
    fn every_cell_fits_the_texture() {
        let cells = BLOCKS.len() as u32 * 3;
        assert!(cells.div_ceil(PER_ROW) * CELL <= TEXTURE_HEIGHT);
        let png = texture();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (TEXTURE_WIDTH, TEXTURE_HEIGHT));
        let at = |c: u32| {
            img.get_pixel((c % PER_ROW) * CELL + 3, (c / PER_ROW) * CELL + 3)
                .0
        };
        let red = cell("minecraft:red_concrete", Shade::Plain);
        assert_eq!(at(red), [142, 33, 33, 255]);
        assert_eq!(
            at(red + 2)[3],
            0,
            "lit cells are transparent for the emissive material"
        );
        assert!(at(red + 1)[0] < 142, "a dim brake light is darker");
    }

    #[test]
    fn a_box_ahead_of_the_middle_lies_at_negative_z() {
        let mut nose = shape("minecraft:white_concrete", Lamp::On);
        nose.corner = [-0.5, 0.5, 2.0];
        nose.size = [1.0, 0.25, 0.1];
        let geo = geometry("t", &[nose]);
        let cube = &geo["minecraft:geometry"][0]["bones"][0]["cubes"][0];
        assert_eq!(cube["origin"], json!([-8.0, 8.0, -33.6]));
        assert_eq!(cube["size"], json!([16.0, 4.0, 1.6]));
        assert_eq!(geo["minecraft:geometry"][0]["bones"][0]["name"], "lit");
    }

    #[test]
    fn brake_lights_get_a_dim_and_a_lit_bone_and_empty_bones_are_left_out() {
        let geo = geometry(
            "car",
            &[
                shape("minecraft:blue_concrete", Lamp::Off),
                shape("minecraft:red_concrete", Lamp::Brake),
            ],
        );
        let names: Vec<&str> = geo["minecraft:geometry"][0]["bones"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["body", "brake_off", "lit_brake"]);
        let desc = &geo["minecraft:geometry"][0]["description"];
        assert_eq!(desc["identifier"], "geometry.arnis.car");
        assert!(desc["visible_bounds_width"].as_f64().unwrap() >= 5.0);
    }

    #[test]
    fn a_tilted_plate_turns_round_its_corner() {
        let mut plate = shape("minecraft:tinted_glass", Lamp::Off);
        plate.tilt = -0.5;
        let geo = geometry("p", &[plate]);
        let cube = &geo["minecraft:geometry"][0]["bones"][0]["cubes"][0];
        assert_eq!(cube["pivot"], json!([-16.0, 4.0, 32.0]));
        assert_eq!(
            cube["rotation"][0].as_f64().unwrap(),
            round((-0.5f32).to_degrees())
        );
    }
}
