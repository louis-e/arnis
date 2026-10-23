//! Shared pieces for structures laid out along any heading: a local frame, axis snapping,
//! colour lookup and thin beams drawn as block displays (Java) or block lines.

use crate::block_definitions::*;
use crate::colors::color_text_to_rgb_tuple;
use crate::world_editor::{WorldEditor, WorldFormat};
use fastnbt::Value;
use std::collections::HashMap;

/// A horizontal frame: `a` runs along the heading, `b` a quarter turn clockwise from it.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub ox: f64,
    pub oz: f64,
    pub ux: f64,
    pub uz: f64,
    /// -1 turns `b` a quarter turn anticlockwise instead.
    pub side: f64,
}

impl Frame {
    pub fn new(ox: f64, oz: f64, dx: f64, dz: f64) -> Self {
        let len = (dx * dx + dz * dz).sqrt();
        let (ux, uz) = if len > 1e-9 {
            (dx / len, dz / len)
        } else {
            (1.0, 0.0)
        };
        Frame {
            ox,
            oz,
            ux,
            uz,
            side: 1.0,
        }
    }

    /// The same frame with `b` pointing the other way.
    pub fn flipped(self) -> Self {
        Frame {
            side: -self.side,
            ..self
        }
    }

    /// The `b` axis.
    pub fn v(&self) -> (f64, f64) {
        (-self.uz * self.side, self.ux * self.side)
    }

    pub fn world(&self, a: f64, b: f64) -> (f64, f64) {
        let (vx, vz) = self.v();
        (
            self.ox + a * self.ux + b * vx,
            self.oz + a * self.uz + b * vz,
        )
    }

    pub fn local(&self, x: f64, z: f64) -> (f64, f64) {
        let (dx, dz) = (x - self.ox, z - self.oz);
        let (vx, vz) = self.v();
        (dx * self.ux + dz * self.uz, dx * vx + dz * vz)
    }

    /// The block holding local point (a, b).
    pub fn cell(&self, a: f64, b: f64) -> (i32, i32) {
        let (x, z) = self.world(a, b);
        (x.floor() as i32, z.floor() as i32)
    }

    /// Cells whose centres fall inside the local box, with those centres in local terms.
    pub fn cells(&self, a0: f64, a1: f64, b0: f64, b1: f64) -> Vec<(i32, i32, f64, f64)> {
        let corners = [
            self.world(a0, b0),
            self.world(a0, b1),
            self.world(a1, b0),
            self.world(a1, b1),
        ];
        let min_x = corners.iter().map(|c| c.0).fold(f64::MAX, f64::min).floor() as i32 - 1;
        let max_x = corners.iter().map(|c| c.0).fold(f64::MIN, f64::max).ceil() as i32 + 1;
        let min_z = corners.iter().map(|c| c.1).fold(f64::MAX, f64::min).floor() as i32 - 1;
        let max_z = corners.iter().map(|c| c.1).fold(f64::MIN, f64::max).ceil() as i32 + 1;
        let mut out = Vec::new();
        for x in min_x..=max_x {
            for z in min_z..=max_z {
                let (a, b) = self.local(x as f64 + 0.5, z as f64 + 0.5);
                if a >= a0 && a <= a1 && b >= b0 && b <= b1 {
                    out.push((x, z, a, b));
                }
            }
        }
        out
    }
}

/// The grid axis closest to a heading, as a unit step.
pub fn axis_of(dx: f64, dz: f64) -> (i32, i32) {
    if dx.abs() >= dz.abs() {
        (if dx < 0.0 { -1 } else { 1 }, 0)
    } else {
        (0, if dz < 0.0 { -1 } else { 1 })
    }
}

/// Blockstate name of a horizontal unit step.
pub fn facing_name(step: (i32, i32)) -> &'static str {
    match step {
        (0, -1) => "north",
        (0, 1) => "south",
        (1, 0) => "east",
        _ => "west",
    }
}

/// A block with one string property.
pub fn with_prop(block: Block, key: &str, value: &str) -> BlockWithProperties {
    let mut map = HashMap::new();
    map.insert(key.to_string(), Value::String(value.to_string()));
    BlockWithProperties::new(block, Some(Value::Compound(map)))
}

/// A block with several string properties.
pub fn with_props(block: Block, props: &[(&str, &str)]) -> BlockWithProperties {
    let map = props
        .iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
        .collect();
    BlockWithProperties::new(block, Some(Value::Compound(map)))
}

/// Stairs whose tall half is on the `back` side.
pub fn stair_backed(block: Block, back: (i32, i32)) -> BlockWithProperties {
    with_props(block, &[("facing", facing_name(back)), ("half", "bottom")])
}

/// A slab in the upper half of its cell.
pub fn top_slab(block: Block) -> BlockWithProperties {
    with_prop(block, "type", "top")
}

pub fn place(editor: &mut WorldEditor, block: BlockWithProperties, x: i32, y: i32, z: i32) {
    editor.set_block_with_properties_absolute(block, x, y, z, None, None);
}

/// Concrete colours with their sRGB values, for matching `colour` tags.
pub const CONCRETE: [(Block, (u8, u8, u8)); 16] = [
    (WHITE_CONCRETE, (207, 213, 214)),
    (ORANGE_CONCRETE, (224, 97, 0)),
    (MAGENTA_CONCRETE, (169, 48, 159)),
    (LIGHT_BLUE_CONCRETE, (35, 137, 198)),
    (YELLOW_CONCRETE, (241, 175, 21)),
    (LIME_CONCRETE, (94, 168, 24)),
    (PINK_CONCRETE, (213, 101, 142)),
    (GRAY_CONCRETE, (54, 57, 61)),
    (LIGHT_GRAY_CONCRETE, (125, 125, 115)),
    (CYAN_CONCRETE, (21, 119, 136)),
    (PURPLE_CONCRETE, (100, 31, 156)),
    (BLUE_CONCRETE, (44, 46, 143)),
    (BROWN_CONCRETE, (96, 59, 31)),
    (GREEN_CONCRETE, (73, 91, 36)),
    (RED_CONCRETE, (142, 32, 32)),
    (BLACK_CONCRETE, (8, 10, 15)),
];

/// The concrete nearest a `colour` tag value.
pub fn concrete_for_colour(text: &str) -> Option<Block> {
    let (r, g, b) = color_text_to_rgb_tuple(text)?;
    let (tl, ta, tb) = crate::colors::rgb_to_oklab(r, g, b);
    let tc = (ta * ta + tb * tb).sqrt();
    // Hue first, then chroma, then lightness: tag colours are far more saturated
    // than any concrete, and a plain distance turns pure red orange.
    CONCRETE
        .iter()
        .map(|(block, (cr, cg, cb))| {
            let (l, a, bb) = crate::colors::rgb_to_oklab(*cr, *cg, *cb);
            let c = (a * a + bb * bb).sqrt();
            let dl = 0.25 * (tl - l);
            let d = if tc < 0.04 {
                (ta - a).powi(2) + (tb - bb).powi(2)
            } else {
                let (ha, hb) = (ta / tc * c, tb / tc * c);
                4.0 * ((ha - a).powi(2) + (hb - bb).powi(2)) + (0.8 * (tc - c)).powi(2)
            };
            (dl * dl + d, *block)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, block)| block)
}

/// Stable 64-bit mix for seeding per-part choices.
pub fn mix(a: u64, b: u64) -> u64 {
    let mut h = a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h
}

type P3 = [f64; 3];

/// A thin straight beam from `p` to `q`: one block display on Java, a run of `block`
/// cells elsewhere (vertical steps use `vertical`).
#[allow(clippy::too_many_arguments)]
pub fn beam(
    editor: &mut WorldEditor,
    p: P3,
    q: P3,
    thickness: f64,
    display_block: &str,
    seed: i64,
    block: Block,
    vertical: Block,
) {
    if editor.format() == WorldFormat::JavaAnvil {
        display_beam(editor, p, q, thickness, display_block, seed);
    } else {
        block_beam(editor, p, q, block, vertical);
    }
}

/// Block display stretched and turned to run from `p` to `q`.
pub fn display_beam(
    editor: &mut WorldEditor,
    p: P3,
    q: P3,
    thickness: f64,
    display_block: &str,
    seed: i64,
) {
    let d = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if len < 1e-3 {
        return;
    }
    let u = [d[0] / len, d[1] / len, d[2] / len];
    // Turns the model's +z onto the beam.
    let w = 1.0 + u[2];
    let quat = if w < 1e-6 {
        [0.0, 1.0, 0.0, 0.0]
    } else {
        let (x, y, z) = (-u[1], u[0], 0.0);
        let n = (x * x + y * y + z * z + w * w).sqrt();
        [x / n, y / n, z / n, w / n]
    };
    // Slight overlap hides the seams between chords.
    let size = [thickness, thickness, len + thickness * 0.5];
    let half = [size[0] / 2.0, size[1] / 2.0, size[2] / 2.0];
    let r = rotate(quat, half);
    let floats = |v: &[f64]| Value::List(v.iter().map(|f| Value::Float(*f as f32)).collect());
    let mut transformation = HashMap::new();
    transformation.insert("left_rotation".to_string(), floats(&quat));
    transformation.insert("right_rotation".to_string(), floats(&[0.0, 0.0, 0.0, 1.0]));
    transformation.insert("translation".to_string(), floats(&[-r[0], -r[1], -r[2]]));
    transformation.insert("scale".to_string(), floats(&size));
    let mut extra = HashMap::new();
    extra.insert(
        "block_state".to_string(),
        Value::Compound(HashMap::from([(
            "Name".to_string(),
            Value::String(display_block.to_string()),
        )])),
    );
    extra.insert(
        "transformation".to_string(),
        Value::Compound(transformation),
    );
    // Default range is 64 blocks; a lift line is seen from much further.
    extra.insert("view_range".to_string(), Value::Float(3.0));
    extra.insert(
        "Tags".to_string(),
        Value::List(vec![Value::String("arnis_rope".to_string())]),
    );
    let mid = [
        (p[0] + q[0]) / 2.0,
        (p[1] + q[1]) / 2.0,
        (p[2] + q[2]) / 2.0,
    ];
    editor.add_block_display(mid[0], mid[1], mid[2], seed, extra);
}

fn rotate(q: [f64; 4], v: P3) -> P3 {
    let (qx, qy, qz, qw) = (q[0], q[1], q[2], q[3]);
    // v + 2w(q x v) + 2 q x (q x v)
    let t = [
        2.0 * (qy * v[2] - qz * v[1]),
        2.0 * (qz * v[0] - qx * v[2]),
        2.0 * (qx * v[1] - qy * v[0]),
    ];
    [
        v[0] + qw * t[0] + (qy * t[2] - qz * t[1]),
        v[1] + qw * t[1] + (qz * t[0] - qx * t[2]),
        v[2] + qw * t[2] + (qx * t[1] - qy * t[0]),
    ]
}

/// Face-connected cells from `p` to `q`, each with the axis it is entered along.
pub fn line_cells(p: P3, q: P3) -> Vec<((i32, i32, i32), u8)> {
    let d = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let steps = (len * 3.0).ceil().max(1.0) as usize;
    let mut out: Vec<((i32, i32, i32), u8)> = Vec::new();
    let horizontal_axis = if d[0].abs() >= d[2].abs() { 0 } else { 2 };
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        let c = (
            (p[0] + d[0] * t).floor() as i32,
            (p[1] + d[1] * t).floor() as i32,
            (p[2] + d[2] * t).floor() as i32,
        );
        let Some(&(last, _)) = out.last() else {
            out.push((c, horizontal_axis));
            continue;
        };
        if last == c {
            continue;
        }
        // Step one axis at a time so the run never touches only at an edge.
        let mut cur = last;
        for axis in [0u8, 2, 1] {
            let (from, to) = match axis {
                0 => (cur.0, c.0),
                1 => (cur.1, c.1),
                _ => (cur.2, c.2),
            };
            if from != to {
                match axis {
                    0 => cur.0 = to,
                    1 => cur.1 = to,
                    _ => cur.2 = to,
                }
                out.push((cur, axis));
            }
        }
    }
    out
}

fn block_beam(editor: &mut WorldEditor, p: P3, q: P3, block: Block, vertical: Block) {
    for ((x, y, z), axis) in line_cells(p, q) {
        editor.set_block_absolute(beam_block(axis, block, vertical), x, y, z, None, None);
    }
}

/// The block for a beam cell reached along `axis`; a chain turns to follow it.
fn beam_block(axis: u8, block: Block, vertical: Block) -> Block {
    match axis {
        1 => vertical,
        2 if block == CHAIN_X => CHAIN_Z,
        _ => block,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trips_points() {
        let f = Frame::new(10.0, -4.0, 3.0, 4.0);
        let (x, z) = f.world(5.0, -2.0);
        let (a, b) = f.local(x, z);
        assert!((a - 5.0).abs() < 1e-9 && (b + 2.0).abs() < 1e-9);
    }

    #[test]
    fn frame_b_axis_is_clockwise() {
        // Heading east, the right hand side is south (+z).
        let f = Frame::new(0.0, 0.0, 1.0, 0.0);
        assert_eq!(f.v(), (-0.0, 1.0));
    }

    #[test]
    fn rotation_maps_z_onto_the_beam() {
        let d: P3 = [3.0, 4.0, 12.0];
        let len = 13.0;
        let u = [d[0] / len, d[1] / len, d[2] / len];
        let w = 1.0 + u[2];
        let (x, y) = (-u[1], u[0]);
        let n = (x * x + y * y + w * w).sqrt();
        let r = rotate([x / n, y / n, 0.0, w / n], [0.0, 0.0, 1.0]);
        for i in 0..3 {
            assert!((r[i] - u[i]).abs() < 1e-9, "{r:?} vs {u:?}");
        }
    }

    #[test]
    fn chain_beams_follow_their_axis() {
        assert_eq!(beam_block(0, CHAIN_X, CHAIN_Y), CHAIN_X);
        assert_eq!(beam_block(2, CHAIN_X, CHAIN_Y), CHAIN_Z);
        assert_eq!(beam_block(1, CHAIN_X, CHAIN_Y), CHAIN_Y);
        assert_eq!(beam_block(2, IRON_BARS, IRON_BARS), IRON_BARS);
    }

    #[test]
    fn line_cells_are_face_connected() {
        let cells = line_cells([0.5, 0.5, 0.5], [6.5, 9.5, 3.5]);
        for w in cells.windows(2) {
            let (a, b) = (w[0].0, w[1].0);
            let manhattan = (a.0 - b.0).abs() + (a.1 - b.1).abs() + (a.2 - b.2).abs();
            assert_eq!(manhattan, 1, "{a:?} -> {b:?}");
        }
        assert_eq!(cells.last().unwrap().0, (6, 9, 3));
    }

    #[test]
    fn colour_tags_pick_matching_concrete() {
        assert_eq!(concrete_for_colour("red"), Some(RED_CONCRETE));
        assert_eq!(concrete_for_colour("#FFD500"), Some(YELLOW_CONCRETE));
        assert_eq!(concrete_for_colour("white"), Some(WHITE_CONCRETE));
        assert_eq!(concrete_for_colour("blue"), Some(BLUE_CONCRETE));
    }
}
