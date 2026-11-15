//! Processing of emergency infrastructure elements.
//!
//! This module handles emergency-related OSM elements including:
//! - `emergency=fire_hydrant` - Fire hydrants

use crate::block_definitions::*;
use crate::element_processing::structure_kit::concrete_for_colour;
use crate::floodfill_cache::CoordinateBitmap;
use crate::osm_parser::ProcessedNode;
use crate::world_editor::{WorldEditor, WorldFormat};
use fastnbt::Value;
use std::collections::HashMap;

/// Generate emergency infrastructure from node elements
pub fn generate_emergency(
    editor: &mut WorldEditor,
    node: &ProcessedNode,
    road_mask: &CoordinateBitmap,
) {
    // Skip if 'layer' or 'level' is negative in the tags
    if let Some(layer) = node.tags.get("layer") {
        if layer.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(level) = node.tags.get("level") {
        if level.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(emergency_type) = node.tags.get("emergency") {
        if emergency_type.as_str() == "fire_hydrant" {
            generate_fire_hydrant(editor, node, road_mask)
        }
    }
}

/// How far a hydrant looks for the road its outlets face, in blocks.
const ROAD_REACH: i32 = 8;
/// Below this many blocks to the metre a hydrant is too small to make out, and is a block.
const MIN_SCALE: f64 = 0.5;

/// Generate a fire hydrant
///
/// On Java a pillar hydrant of scaled vanilla blocks, as the traffic lights are built without
/// the data pack: a barrel on a foot, a domed bonnet with its nut, two outlets at the sides and
/// a bigger one facing the road. Elsewhere a red block with banners. Underground, wall and
/// pond hydrants show nothing above ground.
fn generate_fire_hydrant(
    editor: &mut WorldEditor,
    node: &ProcessedNode,
    road_mask: &CoordinateBitmap,
) {
    let x = node.x;
    let z = node.z;

    // Get hydrant type - skip underground, wall, and pond types
    let hydrant_type = node
        .tags
        .get("fire_hydrant:type")
        .map(|s| s.as_str())
        .unwrap_or("pillar");

    // Skip non-visible hydrant types
    if matches!(hydrant_type, "underground" | "wall" | "pond") {
        return;
    }

    if editor.format() == WorldFormat::JavaAnvil && editor.scale() >= MIN_SCALE {
        let carriageway = editor.carriageway().filter(|c| !c.is_empty());
        // Mapped on the kerb line, a hydrant can fall on the carriageway: it stands beside it.
        let (x, z) = match carriageway {
            Some(c) if c.contains(x, z) => {
                nearest(x, z, 3, |cx, cz| !c.contains(cx, cz)).unwrap_or((x, z))
            }
            _ => (x, z),
        };
        // Towards the carriageway, not the pavement it most likely stands on; any road or
        // path where there is none near.
        let facing = carriageway
            .and_then(|c| road_facing(c, x, z))
            .or_else(|| road_facing(road_mask, x, z))
            .unwrap_or([(0.0, 1.0), (1.0, 0.0), (0.0, -1.0), (-1.0, 0.0)][(node.id % 4) as usize]);
        let body = node.tags.get("colour").and_then(|c| body_block(c));
        let body = format!("minecraft:{}", body.unwrap_or(RED_CONCRETE).name());
        // Its cell is held by an invisible block: solid to walk into, and no grass grows
        // up through it.
        editor.set_block(BARRIER, x, 1, z, None, None);
        let abs_y = editor.get_absolute_y(x, 1, z);
        let k = editor.scale();
        let mut hydrant = Hydrant {
            editor,
            centre: (f64::from(x) + 0.5, f64::from(abs_y), f64::from(z) + 0.5),
            facing,
            k,
            seed: (node.id as i64).wrapping_mul(64),
        };
        hydrant.build(&body);
        return;
    }

    let abs_y = editor.get_absolute_y(x, 1, z);

    // Simple hydrant: a single redstone block at ground level.
    editor.set_block(REDSTONE_BLOCK, x, 1, z, None, None);

    // Non-Java fallback: red banners with an orange flame-like pattern on all four sides.
    const HYDRANT_PATTERNS: &[(&str, &str)] = &[
        ("orange", "minecraft:triangle_top"),
        ("yellow", "minecraft:triangle_bottom"),
        ("red", "minecraft:border"),
    ];
    let banner_faces: [(i32, i32, &str); 4] = [
        (0, 1, "south"),
        (0, -1, "north"),
        (1, 0, "east"),
        (-1, 0, "west"),
    ];
    for (dx, dz, facing) in &banner_faces {
        editor.place_wall_banner(
            RED_WALL_BANNER,
            x + dx,
            abs_y,
            z + dz,
            facing,
            "red",
            HYDRANT_PATTERNS,
        );
    }
}

/// The way to the nearest cell of `roads` within reach, other than its own.
fn road_facing(roads: &CoordinateBitmap, x: i32, z: i32) -> Option<(f64, f64)> {
    let (rx, rz) = nearest(x, z, ROAD_REACH, |cx, cz| roads.contains(cx, cz))?;
    let (dx, dz) = (f64::from(rx - x), f64::from(rz - z));
    let len = dx.hypot(dz);
    Some((dx / len, dz / len))
}

/// The nearest cell other than (x, z) within `reach` that `wanted`, by straight-line
/// distance. `get_nearest_road_block` looks only every other block and eight ways, too
/// coarse for a hydrant a block off the kerb or the way it faces.
fn nearest(x: i32, z: i32, reach: i32, wanted: impl Fn(i32, i32) -> bool) -> Option<(i32, i32)> {
    let mut best: Option<(i32, (i32, i32))> = None;
    for dx in -reach..=reach {
        for dz in -reach..=reach {
            let d = dx * dx + dz * dz;
            if d == 0 || d > reach * reach || !wanted(x + dx, z + dz) {
                continue;
            }
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, (x + dx, z + dz)));
            }
        }
    }
    best.map(|(_, c)| c)
}

/// The barrel's block for a mapped `colour`: a plain name its concrete, as a hydrant
/// painted yellow is yellow whatever web yellow comes nearest, anything else the nearest
/// concrete.
fn body_block(colour: &str) -> Option<Block> {
    let colour = colour.trim().to_ascii_lowercase();
    Some(match colour.as_str() {
        "red" => RED_CONCRETE,
        "yellow" | "gold" => YELLOW_CONCRETE,
        "orange" => ORANGE_CONCRETE,
        "green" => GREEN_CONCRETE,
        "blue" => BLUE_CONCRETE,
        "white" => WHITE_CONCRETE,
        "silver" | "grey" | "gray" => LIGHT_GRAY_CONCRETE,
        "black" => BLACK_CONCRETE,
        _ => return concrete_for_colour(&colour),
    })
}

/// Grey iron for the foot, the caps and the nut.
const IRON: &str = "minecraft:gray_concrete";
const CAP: &str = "minecraft:light_gray_concrete";

struct Hydrant<'e, 'a> {
    editor: &'e mut WorldEditor<'a>,
    /// The middle of its foot, on the ground.
    centre: (f64, f64, f64),
    /// Towards the road, where the big outlet points.
    facing: (f64, f64),
    /// Blocks to the metre.
    k: f64,
    seed: i64,
}

impl Hydrant<'_, '_> {
    /// Its parts, in metres: up from the ground, ahead towards the road and to its side.
    fn build(&mut self, body: &str) {
        // Foot flange, barrel, collar, bonnet, its top and the operating nut.
        self.part((0.0, 0.03, 0.0), (0.32, 0.06, 0.32), IRON);
        self.part((0.0, 0.35, 0.0), (0.22, 0.58, 0.22), body);
        self.part((0.0, 0.665, 0.0), (0.26, 0.05, 0.26), body);
        self.part((0.0, 0.75, 0.0), (0.19, 0.12, 0.19), body);
        self.part((0.0, 0.835, 0.0), (0.13, 0.05, 0.13), body);
        self.part((0.0, 0.885, 0.0), (0.06, 0.05, 0.06), IRON);
        // The outlets at the sides, capped.
        for side in [-1.0, 1.0] {
            self.part((0.0, 0.48, side * 0.165), (0.11, 0.09, 0.09), body);
            self.part((0.0, 0.48, side * 0.235), (0.03, 0.12, 0.12), CAP);
        }
        // The big one towards the road, capped.
        self.part((0.155, 0.42, 0.0), (0.13, 0.13, 0.09), body);
        self.part((0.215, 0.42, 0.0), (0.15, 0.15, 0.03), CAP);
    }

    /// One scaled block, its middle `at` (ahead, up, to the side) from the foot's middle,
    /// `size` (across, up, ahead) in metres, turned to face the road.
    fn part(&mut self, at: (f64, f64, f64), size: (f64, f64, f64), block: &str) {
        let k = self.k;
        let (fx, fz) = self.facing;
        // To the side: the facing turned a quarter to the right.
        let (sx, sz) = (-fz, fx);
        let pos = (
            self.centre.0 + (fx * at.0 + sx * at.2) * k,
            self.centre.1 + at.1 * k,
            self.centre.2 + (fz * at.0 + sz * at.2) * k,
        );
        let s = [
            (size.0 * k) as f32,
            (size.1 * k) as f32,
            (size.2 * k) as f32,
        ];
        let floats = |v: &[f32]| Value::List(v.iter().map(|f| Value::Float(*f)).collect());
        let transformation = HashMap::from([
            ("left_rotation".to_string(), floats(&[0.0, 0.0, 0.0, 1.0])),
            ("right_rotation".to_string(), floats(&[0.0, 0.0, 0.0, 1.0])),
            // The block model spans 0..1, so the translation centres it on the entity.
            (
                "translation".to_string(),
                floats(&[-s[0] / 2.0, -s[1] / 2.0, -s[2] / 2.0]),
            ),
            ("scale".to_string(), floats(&s)),
        ]);
        let yaw = (-fx).atan2(fz).to_degrees() as f32;
        let extra = HashMap::from([
            (
                "block_state".to_string(),
                Value::Compound(HashMap::from([(
                    "Name".to_string(),
                    Value::String(block.to_string()),
                )])),
            ),
            (
                "transformation".to_string(),
                Value::Compound(transformation),
            ),
            (
                "Rotation".to_string(),
                Value::List(vec![Value::Float(yaw), Value::Float(0.0)]),
            ),
        ]);
        self.editor
            .add_block_display(pos.0, pos.1, pos.2, self.seed, extra);
        self.seed += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{bitmap_with_rect, test_editor};

    fn hydrant(id: u64, tags: &[(&str, &str)]) -> ProcessedNode {
        let mut all: HashMap<String, String> = tags
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        all.insert("emergency".into(), "fire_hydrant".into());
        ProcessedNode {
            id,
            tags: all,
            x: 10,
            z: 10,
        }
    }

    fn parts(editor: &WorldEditor) -> Vec<(String, [f64; 3])> {
        editor
            .entities_of("minecraft:block_display")
            .into_iter()
            .map(|e| {
                let Some(Value::Compound(state)) = e.get("block_state") else {
                    panic!("no block state");
                };
                let Some(Value::String(name)) = state.get("Name") else {
                    panic!("no block");
                };
                let Some(Value::List(pos)) = e.get("Pos") else {
                    panic!("no position");
                };
                let p: Vec<f64> = pos
                    .iter()
                    .map(|v| match v {
                        Value::Double(d) => *d,
                        _ => panic!("position not doubles"),
                    })
                    .collect();
                (name.clone(), [p[0], p[1], p[2]])
            })
            .collect()
    }

    #[test]
    fn a_pillar_hydrant_is_built_of_blocks_facing_its_road() {
        let xz = XZBBox::rect_from_xz_lengths(30.0, 30.0).unwrap();
        // A road a few blocks east.
        let road = bitmap_with_rect(&xz, 14, 0, 18, 29);
        let mut editor = test_editor(&xz);
        generate_emergency(&mut editor, &hydrant(1, &[]), &road);
        let parts = parts(&editor);
        assert_eq!(parts.len(), 12);
        // An invisible block holds its cell; all of it stands on that cell, under a block high.
        assert!(editor.check_for_block(10, 1, 10, Some(&[BARRIER])));
        let ground = f64::from(editor.get_absolute_y(10, 1, 10));
        for (_, p) in &parts {
            assert!(
                (p[0] - 10.5).abs() < 0.3 && (p[2] - 10.5).abs() < 0.3,
                "{p:?}"
            );
            assert!(p[1] > ground && p[1] < ground + 1.0, "{p:?}");
        }
        assert!(parts.iter().any(|(b, _)| b == "minecraft:red_concrete"));
        // The part furthest out east is the big outlet's cap, towards the road.
        let east = parts
            .iter()
            .max_by(|a, b| a.1[0].total_cmp(&b.1[0]))
            .unwrap();
        assert_eq!(east.0, CAP);
        assert!((east.1[0] - 10.5 - 0.215).abs() < 0.01, "{east:?}");

        // On a pavement along a carriageway to the north: it faces the carriageway.
        let pavement = bitmap_with_rect(&xz, 0, 9, 29, 11);
        let mut editor = test_editor(&xz);
        editor.set_carriageway(std::sync::Arc::new(bitmap_with_rect(&xz, 0, 4, 29, 7)));
        generate_emergency(&mut editor, &hydrant(4, &[]), &pavement);
        let north = self::parts(&editor)
            .into_iter()
            .min_by(|a, b| a.1[2].total_cmp(&b.1[2]))
            .unwrap();
        assert_eq!(north.0, CAP);
        assert!((10.5 - north.1[2] - 0.215).abs() < 0.01, "{north:?}");

        // Mapped onto the carriageway, it stands beside it, still facing it.
        let mut editor = test_editor(&xz);
        editor.set_carriageway(std::sync::Arc::new(bitmap_with_rect(&xz, 0, 4, 29, 10)));
        generate_emergency(&mut editor, &hydrant(7, &[]), &pavement);
        assert!(editor.check_for_block(10, 1, 11, Some(&[BARRIER])));
        assert!(self::parts(&editor).iter().all(|(_, p)| p[2] > 11.0));
    }

    #[test]
    fn a_hydrant_takes_its_mapped_colour_and_scale_and_hidden_ones_show_nothing() {
        let xz = XZBBox::rect_from_xz_lengths(30.0, 30.0).unwrap();
        let road = CoordinateBitmap::new(&xz);
        let mut editor = test_editor(&xz);
        editor.set_projection_info("local", 2.0);
        generate_emergency(&mut editor, &hydrant(2, &[("colour", "Yellow")]), &road);
        let parts = parts(&editor);
        assert!(parts.iter().any(|(b, _)| b == "minecraft:yellow_concrete"));
        assert!(!parts.iter().any(|(b, _)| b == "minecraft:red_concrete"));
        // Twice the size at two blocks to the metre: the nut is near 1.8 up.
        let ground = f64::from(editor.get_absolute_y(10, 1, 10));
        let top = parts.iter().map(|(_, p)| p[1]).fold(f64::MIN, f64::max);
        assert!((top - ground - 2.0 * 0.885).abs() < 0.01, "{top}");

        // A colour given as a hex code takes the nearest concrete.
        let mut editor = test_editor(&xz);
        generate_emergency(&mut editor, &hydrant(5, &[("colour", "#1e3fa0")]), &road);
        assert!(self::parts(&editor)
            .iter()
            .any(|(b, _)| b == "minecraft:blue_concrete"));

        // Too small to make out at a fifth of a block to the metre: a block, no displays.
        let mut editor = test_editor(&xz);
        editor.set_projection_info("local", 0.2);
        generate_emergency(&mut editor, &hydrant(6, &[]), &road);
        assert!(editor.entities_of("minecraft:block_display").is_empty());
        assert!(editor.check_for_block(10, 1, 10, Some(&[REDSTONE_BLOCK])));

        // Any node id makes a seed.
        let mut editor = test_editor(&xz);
        generate_emergency(&mut editor, &hydrant(u64::MAX - 3, &[]), &road);
        assert_eq!(self::parts(&editor).len(), 12);

        for kind in ["underground", "wall", "pond"] {
            let mut editor = test_editor(&xz);
            generate_emergency(
                &mut editor,
                &hydrant(3, &[("fire_hydrant:type", kind)]),
                &road,
            );
            assert!(
                editor.entities_of("minecraft:block_display").is_empty(),
                "{kind}"
            );
        }
    }
}
