//! Statues, busts and sculptures on their pedestals, from `historic=memorial` with
//! `memorial=statue|bust|sculpture` and from `tourism=artwork` with the same
//! `artwork_type`.
//!
//! On Java a figure is built from block displays: head, body, arms and legs are each a
//! block of the statue's material, sized to a person's proportions and turned into a
//! pose. A sculpture is a few turned blocks. Display entities are vanilla, so this needs
//! no data pack and no resource pack. Other formats get a figure of blocks.

use crate::block_definitions::*;
use crate::deterministic_rng::element_rng;
use crate::element_processing::structure_kit::rotate;
use crate::osm_parser::{ProcessedNode, ProcessedWay};
use crate::world_editor::{WorldEditor, WorldFormat};
use fastnbt::Value;
use rand::Rng;
use std::collections::HashMap;

/// What stands on the pedestal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    Statue,
    Bust,
    Sculpture,
}

impl Form {
    fn from_value(v: &str) -> Option<Form> {
        match v {
            "statue" => Some(Form::Statue),
            "bust" => Some(Form::Bust),
            "sculpture" => Some(Form::Sculpture),
            _ => None,
        }
    }
}

/// The figure a node's tags describe, if they describe one that stands on the ground.
pub fn form(tags: &HashMap<String, String>) -> Option<Form> {
    // A figure on a facade, a roof or an upper floor, or under a roof, belongs to its
    // building: its floors would cut through a figure of its own.
    let yes = |k: &str| tags.get(k).map(String::as_str) == Some("yes");
    let raised = tags.get("historic").map(String::as_str) == Some("housefigure")
        || yes("covered")
        || yes("indoor")
        || tags.get("location").map(String::as_str) == Some("indoor")
        || matches!(
            tags.get("support").map(String::as_str),
            Some("wall_mounted" | "in_wall" | "roof" | "building")
        )
        || tags
            .get("level")
            .and_then(|l| l.split(';').next()?.trim().parse::<f32>().ok())
            .is_some_and(|l| l != 0.0);
    if raised {
        return None;
    }
    if tags.get("tourism").map(String::as_str) == Some("artwork") {
        return tags.get("artwork_type").and_then(|v| Form::from_value(v));
    }
    if tags.get("historic").map(String::as_str) == Some("memorial") {
        return tags.get("memorial").and_then(|v| Form::from_value(v));
    }
    None
}

/// The material a figure is cast or carved in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Material {
    /// Bronze gone green with verdigris.
    Patina,
    /// Bronze on its way there.
    Weathered,
    Marble,
    Stone,
    Gilded,
}

impl Material {
    fn of(tags: &HashMap<String, String>, roll: u32) -> Material {
        let m = tags
            .get("material")
            .map(|m| m.to_ascii_lowercase())
            .unwrap_or_default();
        if m.contains("gold") || m.contains("gilded") {
            Material::Gilded
        } else if m.contains("marble") || m.contains("limestone") {
            Material::Marble
        } else if m.contains("stone")
            || m.contains("granite")
            || m.contains("sandstone")
            || m.contains("concrete")
        {
            Material::Stone
        } else if m.contains("bronze") || m.contains("copper") || m.contains("metal") {
            if roll.is_multiple_of(3) {
                Material::Weathered
            } else {
                Material::Patina
            }
        } else {
            // Most statues in a city are bronze, the rest mostly stone.
            match roll % 10 {
                0..=4 => Material::Patina,
                5..=6 => Material::Weathered,
                7..=8 => Material::Stone,
                _ => Material::Marble,
            }
        }
    }

    /// The block a figure's parts are made of.
    fn block_id(self) -> &'static str {
        match self {
            Material::Patina => "minecraft:waxed_oxidized_copper",
            Material::Weathered => "minecraft:waxed_weathered_copper",
            Material::Marble => "minecraft:smooth_quartz",
            Material::Stone => "minecraft:smooth_stone",
            Material::Gilded => "minecraft:gold_block",
        }
    }

    /// The block a figure built of blocks is made of.
    fn block(self) -> Block {
        match self {
            Material::Patina => WAXED_OXIDIZED_COPPER,
            Material::Weathered => WAXED_EXPOSED_COPPER,
            Material::Marble => SMOOTH_QUARTZ,
            Material::Stone => SMOOTH_STONE,
            Material::Gilded => GOLD_BLOCK,
        }
    }
}

/// A turn in degrees about x, y and z, made about x first, then y, then z, as a model
/// part turns in the game. The figure faces +z with its right side to -x, so a negative
/// x raises a limb forward.
type Turn = [f32; 3];

/// A limb's two segments: the upper turned at the shoulder or hip, the lower turned
/// further at the elbow or knee. A negative x bends an elbow, a positive one a knee.
#[derive(Clone, Copy)]
struct Limb {
    upper: Turn,
    lower: Turn,
}

const fn limb(upper: Turn, lower: Turn) -> Limb {
    Limb { upper, lower }
}

/// What a figure wears below the shoulders.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dress {
    /// A frock coat to the knees, the 19th century's statue.
    Coat,
    /// A gown to the feet.
    Robe,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Holds {
    Nothing,
    Staff,
    Sword,
    Book,
    Scroll,
}

/// A figure's pose.
struct Pose {
    head: Turn,
    right_arm: Limb,
    left_arm: Limb,
    right_leg: Limb,
    left_leg: Limb,
    dress: Dress,
    holds: Holds,
}

const STRAIGHT: Limb = limb([0.0; 3], [0.0; 3]);

const POSES: &[Pose] = &[
    // At ease, a hand in the coat, the weight on the right leg.
    Pose {
        head: [4.0, -8.0, 0.0],
        right_arm: limb([-10.0, 0.0, -4.0], [-100.0, 70.0, 0.0]),
        left_arm: limb([-4.0, 0.0, 6.0], [-8.0, 0.0, 0.0]),
        right_leg: limb([0.0, 0.0, -2.0], [0.0; 3]),
        left_leg: limb([-10.0, 0.0, 3.0], [16.0, 0.0, 0.0]),
        dress: Dress::Coat,
        holds: Holds::Nothing,
    },
    // An orator, the right arm raised, the left hand on the hip.
    Pose {
        head: [-12.0, 0.0, 0.0],
        right_arm: limb([-150.0, 0.0, -12.0], [-15.0, 0.0, 0.0]),
        left_arm: limb([0.0, 0.0, 35.0], [0.0, 0.0, -90.0]),
        right_leg: limb([-8.0, 0.0, -3.0], [12.0, 0.0, 0.0]),
        left_leg: limb([4.0, 0.0, 2.0], [0.0; 3]),
        dress: Dress::Coat,
        holds: Holds::Nothing,
    },
    // A scholar in a gown, reading.
    Pose {
        head: [24.0, 0.0, 0.0],
        right_arm: limb([-15.0, 0.0, -5.0], [-70.0, 35.0, 0.0]),
        left_arm: limb([-15.0, 0.0, 5.0], [-70.0, -35.0, 0.0]),
        right_leg: STRAIGHT,
        left_leg: STRAIGHT,
        dress: Dress::Robe,
        holds: Holds::Book,
    },
    // A commander, a sword held high, stepping forward.
    Pose {
        head: [-6.0, 10.0, 0.0],
        right_arm: limb([-125.0, 0.0, -10.0], [-10.0, 0.0, 0.0]),
        left_arm: limb([0.0, 0.0, 8.0], [-12.0, 0.0, 0.0]),
        right_leg: limb([6.0, 0.0, -2.0], [0.0; 3]),
        left_leg: limb([-20.0, 0.0, 2.0], [22.0, 0.0, 0.0]),
        dress: Dress::Coat,
        holds: Holds::Sword,
    },
    // A saint or a ruler in a robe, a staff in one hand, the other raised to bless.
    Pose {
        head: [6.0, 0.0, 0.0],
        right_arm: limb([-8.0, 0.0, -8.0], [-75.0, 0.0, 0.0]),
        left_arm: limb([-8.0, 0.0, 6.0], [-110.0, -30.0, 0.0]),
        right_leg: STRAIGHT,
        left_leg: STRAIGHT,
        dress: Dress::Robe,
        holds: Holds::Staff,
    },
    // Pointing ahead.
    Pose {
        head: [0.0, -15.0, 0.0],
        right_arm: limb([-4.0, 0.0, -5.0], [-30.0, 0.0, 0.0]),
        left_arm: limb([-85.0, 14.0, 0.0], [0.0; 3]),
        right_leg: limb([4.0, 0.0, -2.0], [0.0; 3]),
        left_leg: limb([-12.0, 0.0, 3.0], [18.0, 0.0, 0.0]),
        dress: Dress::Coat,
        holds: Holds::Nothing,
    },
    // A scroll in hand.
    Pose {
        head: [8.0, 6.0, 0.0],
        right_arm: limb([-12.0, 0.0, -4.0], [-80.0, 20.0, 0.0]),
        left_arm: limb([-2.0, 0.0, 5.0], [-6.0, 0.0, 0.0]),
        right_leg: limb([-10.0, 0.0, -3.0], [16.0, 0.0, 0.0]),
        left_leg: limb([0.0, 0.0, 2.0], [0.0; 3]),
        dress: Dress::Coat,
        holds: Holds::Scroll,
    },
];

/// A figure's measures in metres, a block each: 1.8 tall, facing +z, its right to -x.
const HIP: [f64; 2] = [0.09, 0.94];
const SHOULDER: [f64; 2] = [0.22, 1.40];
const NECK: f64 = 1.44;
/// Length, width and depth of the limbs' segments.
const THIGH: [f64; 3] = [0.45, 0.15, 0.16];
const SHIN: [f64; 3] = [0.43, 0.12, 0.13];
const UPPER_ARM: [f64; 3] = [0.29, 0.105, 0.105];
const FOREARM: [f64; 3] = [0.31, 0.095, 0.095];
const HORSE_THIGH: [f64; 3] = [0.52, 0.14, 0.18];
const HORSE_SHIN: [f64; 3] = [0.5, 0.1, 0.11];

/// How much bigger than life a figure on a pedestal stands.
const STATUE_SCALE: f64 = 1.35;
const BUST_SCALE: f64 = 1.6;
/// A figure on a column is seen from far below.
const CROWN_SCALE: f64 = 1.7;

/// Builds the pedestal and its figure at the node.
pub fn place(editor: &mut WorldEditor, node: &ProcessedNode, form: Form) {
    let (x, z) = (node.x, node.z);
    let mut rng = element_rng(node.id);
    let roll: u32 = rng.random();
    let material = Material::of(&node.tags, roll);
    let java = editor.format() == WorldFormat::JavaAnvil;
    let mut figure = Figure {
        seed: (node.id as i64).wrapping_mul(64),
        block: material.block_id(),
        at: (f64::from(x) + 0.5, 0.0, f64::from(z) + 0.5),
        yaw: facing_yaw(&node.tags, roll),
        scale: 1.0,
    };
    match form {
        // A rider stays on a plinth, however tall the two of them are mapped.
        Form::Statue if node.tags.get("statue").map(String::as_str) == Some("equestrian") => {
            let top = pedestal(editor, x, z, roll, 2);
            if java {
                figure.at.1 = f64::from(top) + 0.5;
                figure.scale = STATUE_SCALE;
                figure.rider(editor, roll);
            } else {
                for dy in 0..3 {
                    put(editor, material.block(), x, top + dy, z);
                }
            }
        }
        // A statue on a column, a Marian or a victory column.
        Form::Statue if height(&node.tags).is_some_and(|h| h >= COLUMN_HEIGHT) => {
            column(
                editor,
                node,
                height(&node.tags).unwrap_or(COLUMN_HEIGHT),
                true,
            );
        }
        Form::Statue => {
            let top = pedestal(editor, x, z, roll, 1);
            if java {
                figure.at.1 = f64::from(top) + 0.5;
                figure.scale = STATUE_SCALE;
                figure.person(editor, &POSES[(roll >> 8) as usize % POSES.len()]);
            } else {
                // The figure takes the cap's place: a block on a slab would float.
                for dy in 0..3 {
                    put(editor, material.block(), x, top + dy, z);
                }
            }
        }
        Form::Bust => {
            let top = bust_pedestal(editor, x, z);
            if java {
                figure.at.1 = f64::from(top) + 0.5;
                figure.scale = BUST_SCALE;
                figure.bust(editor);
            } else {
                // In the slab's place: a block on a slab would float.
                put(editor, material.block(), x, top, z);
            }
        }
        Form::Sculpture => {
            // A horse needs room for its hooves; anything else stands on one block.
            let r = i32::from(names_a_horse(&node.tags));
            let top = lowest_ground(editor, x, z, r) + 2;
            for dx in -r..=r {
                for dz in -r..=r {
                    put(editor, POLISHED_ANDESITE, x + dx, top - 1, z + dz);
                }
            }
            if java {
                figure.at.1 = f64::from(top);
                figure.block = match (roll >> 16) % 4 {
                    0 => "minecraft:waxed_oxidized_copper",
                    1 => "minecraft:iron_block",
                    2 => "minecraft:polished_blackstone",
                    _ => "minecraft:waxed_copper_block",
                };
                figure.sculpture(editor, roll, &node.tags);
            } else {
                for dy in 0..3 {
                    put(editor, material.block(), x, top + dy, z);
                }
            }
        }
    }
}

/// Whether a sculpture's name or subject says it is a horse.
fn names_a_horse(tags: &HashMap<String, String>) -> bool {
    // Words that sit inside other words ("ross" in "Grosser") are left out.
    const HORSE: [&str; 7] = [
        "horse", "pferd", "cheval", "caballo", "cavallo", "paard", "koń",
    ];
    [
        "name",
        "name:en",
        "subject",
        "artwork_subject",
        "description",
    ]
    .iter()
    .filter_map(|k| tags.get(*k))
    .any(|v| {
        let v = v.to_lowercase();
        HORSE.iter().any(|h| v.contains(h))
    })
}

/// A statue mapped as the outline of its pedestal stands in its middle.
pub fn place_on_way(editor: &mut WorldEditor, way: &ProcessedWay, form: Form) {
    let Some(first) = way.nodes.first() else {
        return;
    };
    // A closed ring repeats its first node; count it once.
    let ring = match way.nodes.last() {
        Some(last) if way.nodes.len() > 1 && last.x == first.x && last.z == first.z => {
            &way.nodes[..way.nodes.len() - 1]
        }
        _ => &way.nodes[..],
    };
    let n = ring.len() as f64;
    let centre = ProcessedNode {
        id: way.id,
        tags: way.tags.clone(),
        x: (ring.iter().map(|p| f64::from(p.x)).sum::<f64>() / n).round() as i32,
        z: (ring.iter().map(|p| f64::from(p.z)).sum::<f64>() / n).round() as i32,
    };
    place(editor, &centre, form);
}

/// Yaw a figure faces: the mapped `direction`, in degrees or as a point of the compass,
/// else one of four set by the node.
fn facing_yaw(tags: &HashMap<String, String>, roll: u32) -> f32 {
    const POINTS: [&str; 16] = [
        "N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW",
        "NW", "NNW",
    ];
    let mapped = tags.get("direction").and_then(|d| {
        let d = d.trim();
        match POINTS.iter().position(|p| p.eq_ignore_ascii_case(d)) {
            Some(i) => Some(i as f32 * 22.5),
            None => d.parse::<f32>().ok().filter(|v| v.is_finite()),
        }
    });
    // OSM measures clockwise from north; a Minecraft yaw of 0 faces south.
    match mapped {
        Some(deg) => (deg + 180.0).rem_euclid(360.0),
        None => ((roll >> 4) % 4) as f32 * 90.0,
    }
}

/// Whether a block grew rather than was built: a tree or a plant.
fn is_plant(block: Block) -> bool {
    let name = block.name();
    [
        "leaves", "_log", "_wood", "_stem", "_hyphae", "grass", "fern", "bush", "vine",
    ]
    .iter()
    .any(|part| name.contains(part))
        || matches!(
            name,
            "dandelion" | "poppy" | "allium" | "cornflower" | "oxeye_daisy" | "azure_bluet"
        )
        || name.ends_with("_tulip")
        || name.ends_with("_sapling")
}

/// Pedestals and columns stand where they are mapped, through a tree that grew there
/// first, but never through anything built.
fn put_with(editor: &mut WorldEditor, block: BlockWithProperties, x: i32, y: i32, z: i32) {
    if editor
        .get_block_absolute(x, y, z)
        .is_some_and(|existing| !is_plant(existing))
    {
        return;
    }
    editor.set_block_with_properties_absolute(block, x, y, z, None, Some(&[]));
}

fn put(editor: &mut WorldEditor, block: Block, x: i32, y: i32, z: i32) {
    put_with(
        editor,
        BlockWithProperties {
            block,
            properties: None,
        },
        x,
        y,
        z,
    );
}

/// The lowest ground within `r` of a point. A pedestal is built up from it, so none of
/// its steps hangs in the air on a slope.
fn lowest_ground(editor: &WorldEditor, x: i32, z: i32, r: i32) -> i32 {
    crate::models_3d::lowest_ground_in_bbox(editor, x - r, z - r, x + r, z + r)
}

/// A square of `fill` reaching `r - 1` from the centre at `y`, ringed by `stairs`
/// stepping down to its edge.
pub fn stepped_square(
    editor: &mut WorldEditor,
    (x, z): (i32, i32),
    r: i32,
    y: i32,
    fill: Block,
    stairs: Block,
) {
    for dx in -r..=r {
        for dz in -r..=r {
            let (edge_x, edge_z) = (dx.abs() == r, dz.abs() == r);
            if !edge_x && !edge_z {
                put(editor, fill, x + dx, y, z + dz);
                continue;
            }
            // Each stair's high side faces the middle.
            let (facing, shape) = match (edge_x, edge_z) {
                (true, false) if dx < 0 => (StairFacing::East, StairShape::Straight),
                (true, false) => (StairFacing::West, StairShape::Straight),
                _ => {
                    let facing = if dz < 0 {
                        StairFacing::South
                    } else {
                        StairFacing::North
                    };
                    let shape = match (edge_x, dx * dz > 0) {
                        (false, _) => StairShape::Straight,
                        (true, true) => StairShape::OuterRight,
                        (true, false) => StairShape::OuterLeft,
                    };
                    (facing, shape)
                }
            };
            let stair = create_stair_with_properties(stairs, facing, shape);
            put_with(editor, stair, x + dx, y, z + dz);
        }
    }
}

/// A stepped pedestal `r` out from the node: a moulded plinth, a die and a cap. Returns
/// the Y of the cap, a bottom slab the figure stands on.
fn pedestal(editor: &mut WorldEditor, x: i32, z: i32, roll: u32, r: i32) -> i32 {
    let (die, stairs, cap) = match roll >> 12 & 3 {
        0 => (
            POLISHED_GRANITE,
            POLISHED_GRANITE_STAIRS,
            POLISHED_GRANITE_SLAB,
        ),
        1 => (
            POLISHED_DIORITE,
            POLISHED_DIORITE_STAIRS,
            POLISHED_DIORITE_SLAB,
        ),
        _ => (
            POLISHED_ANDESITE,
            POLISHED_ANDESITE_STAIRS,
            POLISHED_ANDESITE_SLAB,
        ),
    };
    let ground = lowest_ground(editor, x, z, r);
    stepped_square(editor, (x, z), r, ground + 1, die, stairs);
    for dx in 1 - r..r {
        for dz in 1 - r..r {
            put(editor, die, x + dx, ground + 2, z + dz);
            put(editor, cap, x + dx, ground + 3, z + dz);
        }
    }
    ground + 3
}

/// A mapped `height` in metres.
pub(crate) fn height(tags: &HashMap<String, String>) -> Option<f64> {
    tags.get("height")?
        .trim()
        .trim_end_matches('m')
        .trim()
        .parse::<f64>()
        .ok()
}

/// From this mapped height a statue stands on a column of its own.
const COLUMN_HEIGHT: f64 = 6.0;

/// A column on a stepped base, built up from the lowest ground under it, `height`
/// blocks to its top. On Java a `figure` stands on it; elsewhere it ends in a cap.
pub fn column(editor: &mut WorldEditor, node: &ProcessedNode, height: f64, figure: bool) {
    let (x, z) = (node.x, node.z);
    let height = (height.round() as i32).clamp(5, 30);
    let ground = lowest_ground(editor, x, z, 2);
    stepped_square(
        editor,
        (x, z),
        2,
        ground + 1,
        STONE_BRICKS,
        STONE_BRICK_STAIRS,
    );
    // The die, its corners carved.
    for y in ground + 2..=ground + 3 {
        for dx in -1..=1 {
            for dz in -1..=1 {
                let block = if dx != 0 && dz != 0 {
                    CHISELED_STONE_BRICKS
                } else {
                    POLISHED_ANDESITE
                };
                put(editor, block, x + dx, y, z + dz);
            }
        }
    }
    stepped_square(
        editor,
        (x, z),
        1,
        ground + 4,
        POLISHED_ANDESITE,
        POLISHED_ANDESITE_STAIRS,
    );
    // A figure about three blocks tall takes the top of the column's height, down to a
    // capital right on the base.
    let crowned = figure && editor.format() == WorldFormat::JavaAnvil;
    let capital = ground + if crowned { (height - 3).max(5) } else { height };
    for y in ground + 5..capital {
        put(editor, QUARTZ_PILLAR, x, y, z);
    }
    put(editor, CHISELED_QUARTZ_BLOCK, x, capital, z);
    if crowned {
        crown(editor, node, capital + 1);
    } else {
        put(editor, SMOOTH_QUARTZ_SLAB, x, capital + 1, z);
    }
}

/// A figure crowning a column, its feet at `y`. Built of display blocks, so for Java
/// only.
fn crown(editor: &mut WorldEditor, node: &ProcessedNode, y: i32) {
    let roll: u32 = element_rng(node.id).random();
    // Victories and saints on columns are gilt as often as not.
    let material = if node.tags.contains_key("material") || roll.is_multiple_of(2) {
        Material::of(&node.tags, roll >> 1)
    } else {
        Material::Gilded
    };
    // Raised arm, raised sword or staff: poses that read from far below.
    let pose = [1, 3, 4][(roll >> 8) as usize % 3];
    let mut figure = Figure {
        seed: (node.id as i64).wrapping_mul(64),
        block: material.block_id(),
        at: (
            f64::from(node.x) + 0.5,
            f64::from(y),
            f64::from(node.z) + 0.5,
        ),
        yaw: facing_yaw(&node.tags, roll),
        scale: CROWN_SCALE,
    };
    figure.person(editor, &POSES[pose]);
}

/// A slender pedestal for a bust: a block, a wall post and a slab. Returns the slab's Y.
fn bust_pedestal(editor: &mut WorldEditor, x: i32, z: i32) -> i32 {
    let ground = editor.get_ground_level(x, z);
    put(editor, POLISHED_ANDESITE, x, ground + 1, z);
    put(editor, ANDESITE_WALL, x, ground + 2, z);
    put(editor, POLISHED_ANDESITE_SLAB, x, ground + 3, z);
    ground + 3
}

/// Builds a figure from block displays, its feet at `at`, facing `yaw`.
struct Figure {
    seed: i64,
    block: &'static str,
    at: (f64, f64, f64),
    yaw: f32,
    scale: f64,
}

type Quat = [f64; 4];

/// Quaternion (x, y, z, w) of a turn.
fn quat(turn: Turn) -> Quat {
    let about = |deg: f32, axis: usize| {
        let (s, c) = (f64::from(deg).to_radians() / 2.0).sin_cos();
        let mut q = [0.0, 0.0, 0.0, c];
        q[axis] = s;
        q
    };
    mul(mul(about(turn[2], 2), about(turn[1], 1)), about(turn[0], 0))
}

fn mul(a: Quat, b: Quat) -> Quat {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

const NO_TURN: Quat = [0.0, 0.0, 0.0, 1.0];

impl Figure {
    /// One part: a block of `size` with its corner `offset` from `pivot`, turned about
    /// the pivot by `q`. Measures are the figure's own, before scaling.
    fn part(
        &mut self,
        editor: &mut WorldEditor,
        pivot: [f64; 3],
        offset: [f64; 3],
        size: [f64; 3],
        q: Quat,
    ) {
        let s = self.scale;
        let o = rotate(q, offset.map(|v| v * s));
        let floats = |v: &[f64]| Value::List(v.iter().map(|f| Value::Float(*f as f32)).collect());
        let transformation = HashMap::from([
            ("left_rotation".to_string(), floats(&q)),
            ("right_rotation".to_string(), floats(&NO_TURN)),
            ("scale".to_string(), floats(&size.map(|v| v * s))),
            (
                "translation".to_string(),
                floats(&[0, 1, 2].map(|i| pivot[i] * s + o[i])),
            ),
        ]);
        let extra = HashMap::from([
            (
                "block_state".to_string(),
                Value::Compound(HashMap::from([(
                    "Name".to_string(),
                    Value::String(self.block.to_string()),
                )])),
            ),
            (
                "transformation".to_string(),
                Value::Compound(transformation),
            ),
            (
                "Rotation".to_string(),
                Value::List(vec![Value::Float(self.yaw), Value::Float(0.0)]),
            ),
        ]);
        self.seed += 1;
        editor.add_block_display(self.at.0, self.at.1, self.at.2, self.seed, extra);
    }

    /// A box hanging from `joint` along the turned -y, `[length, width, depth]`, reaching
    /// a little above the joint so a bent joint shows no gap.
    fn segment(&mut self, editor: &mut WorldEditor, joint: [f64; 3], m: [f64; 3], q: Quat) {
        let lap = m[1] * 0.4;
        self.part(
            editor,
            joint,
            [-m[1] / 2.0, -m[0], -m[2] / 2.0],
            [m[1], m[0] + lap, m[2]],
            q,
        );
    }

    /// A limb of two segments from `joint`, drawn unless hidden. Returns the lower
    /// segment's turn, where it starts and where it ends.
    fn limb(
        &mut self,
        editor: &mut WorldEditor,
        joint: [f64; 3],
        limb: Limb,
        measures: [[f64; 3]; 2],
        drawn: [bool; 2],
    ) -> (Quat, [f64; 3], [f64; 3]) {
        let upper = quat(limb.upper);
        let lower = mul(upper, quat(limb.lower));
        let mid = add(joint, rotate(upper, [0.0, -measures[0][0], 0.0]));
        let end = add(mid, rotate(lower, [0.0, -measures[1][0], 0.0]));
        if drawn[0] {
            self.segment(editor, joint, measures[0], upper);
        }
        if drawn[1] {
            self.segment(editor, mid, measures[1], lower);
        }
        (lower, mid, end)
    }

    /// The head with its nose, turned at the neck.
    fn head(&mut self, editor: &mut WorldEditor, neck: f64, turn: Turn) {
        let q = quat(turn);
        let pivot = [0.0, neck + 0.08, 0.0];
        self.part(
            editor,
            [0.0, neck - 0.02, 0.0],
            [-0.055, 0.0, -0.055],
            [0.11, 0.12, 0.11],
            NO_TURN,
        );
        self.part(editor, pivot, [-0.105, 0.0, -0.11], [0.21, 0.25, 0.23], q);
        self.part(editor, pivot, [-0.02, 0.06, 0.105], [0.04, 0.07, 0.05], q);
    }

    /// A standing person in `pose`.
    fn person(&mut self, editor: &mut WorldEditor, pose: &Pose) {
        let robe = pose.dress == Dress::Robe;
        for (side, leg) in [(-1.0, pose.right_leg), (1.0, pose.left_leg)] {
            let hip = [HIP[0] * side, HIP[1], 0.0];
            // Under a robe only the feet show.
            let (_, _, ankle) = self.limb(editor, hip, leg, [THIGH, SHIN], [!robe, !robe]);
            let turn = quat([0.0, leg.upper[1], 0.0]);
            let foot = [ankle[0], (ankle[1] - 0.06).max(0.0), ankle[2]];
            self.part(editor, foot, [-0.055, 0.0, -0.07], [0.11, 0.08, 0.27], turn);
        }
        match pose.dress {
            Dress::Coat => {
                self.part(
                    editor,
                    [0.0; 3],
                    [-0.19, 0.5, -0.125],
                    [0.38, 0.96, 0.25],
                    NO_TURN,
                );
            }
            Dress::Robe => {
                self.part(
                    editor,
                    [0.0; 3],
                    [-0.19, 0.0, -0.135],
                    [0.38, 1.46, 0.27],
                    NO_TURN,
                );
            }
        }
        self.head(editor, NECK, pose.head);
        let mut hands = [[0.0; 3]; 2];
        let mut forearm = NO_TURN;
        for (k, (side, arm)) in [(-1.0, pose.right_arm), (1.0, pose.left_arm)]
            .into_iter()
            .enumerate()
        {
            let shoulder = [SHOULDER[0] * side, SHOULDER[1], 0.0];
            let (lower, _, end) =
                self.limb(editor, shoulder, arm, [UPPER_ARM, FOREARM], [true, true]);
            hands[k] = add(end, rotate(lower, [0.0, 0.05, 0.0]));
            if k == 0 {
                forearm = lower;
            }
        }
        let hand = hands[0];
        match pose.holds {
            Holds::Nothing => {}
            // Upright, its foot on the ground and its head above the figure's.
            Holds::Staff => self.part(
                editor,
                hand,
                [-0.025, -hand[1], -0.025],
                [0.05, 2.05, 0.05],
                NO_TURN,
            ),
            // A blade on along the forearm, past the fist, and its guard.
            Holds::Sword => self.sword(editor, hand, forearm),
            // Open between both hands, tipped towards the reader.
            Holds::Book => {
                let between = [0, 1, 2].map(|i| (hands[0][i] + hands[1][i]) / 2.0);
                self.part(
                    editor,
                    between,
                    [-0.14, 0.0, -0.1],
                    [0.28, 0.035, 0.2],
                    quat([-35.0, 0.0, 0.0]),
                );
            }
            // Rolled up, across the fist.
            Holds::Scroll => self.part(
                editor,
                hand,
                [-0.16, -0.035, -0.035],
                [0.32, 0.07, 0.07],
                forearm,
            ),
        }
    }

    /// A blade on along the forearm `q`, past the fist at `hand`, and its guard.
    fn sword(&mut self, editor: &mut WorldEditor, hand: [f64; 3], q: Quat) {
        self.part(editor, hand, [-0.018, -0.84, -0.04], [0.036, 0.78, 0.08], q);
        self.part(editor, hand, [-0.03, -0.08, -0.12], [0.06, 0.035, 0.24], q);
    }

    /// A horse, pawing the ground with a foreleg or standing square: barrel, four legs,
    /// neck, head with its ears, and tail.
    fn horse(&mut self, editor: &mut WorldEditor, pawing: bool) {
        self.part(
            editor,
            [0.0; 3],
            [-0.23, 0.95, -0.8],
            [0.46, 0.56, 1.55],
            NO_TURN,
        );
        let raised = if pawing {
            limb([-42.0, 0.0, 0.0], [80.0, 0.0, 0.0])
        } else {
            STRAIGHT
        };
        for (pivot, leg) in [
            ([-0.13, 1.05, 0.6], STRAIGHT),
            ([0.13, 1.05, 0.6], raised),
            ([-0.13, 1.05, -0.62], limb([-5.0, 0.0, 0.0], [0.0; 3])),
            ([0.13, 1.05, -0.62], limb([8.0, 0.0, 0.0], [0.0; 3])),
        ] {
            self.limb(editor, pivot, leg, [HORSE_THIGH, HORSE_SHIN], [true, true]);
        }
        let neck = quat([32.0, 0.0, 0.0]);
        let withers = [0.0, 1.32, 0.6];
        self.part(
            editor,
            withers,
            [-0.12, 0.0, -0.17],
            [0.24, 0.72, 0.32],
            neck,
        );
        let poll = add(withers, rotate(neck, [0.0, 0.66, 0.0]));
        let head = quat([38.0, 0.0, 0.0]);
        self.part(editor, poll, [-0.1, -0.14, -0.12], [0.2, 0.24, 0.6], head);
        for side in [-1.0, 1.0] {
            self.part(
                editor,
                poll,
                [side * 0.06 - 0.025, 0.08, -0.1],
                [0.05, 0.13, 0.05],
                head,
            );
        }
        self.part(
            editor,
            [0.0, 1.42, -0.8],
            [-0.06, -0.78, -0.06],
            [0.12, 0.8, 0.12],
            quat([22.0, 0.0, 0.0]),
        );
    }

    /// A rider on a horse, the horse pawing the ground or standing square.
    fn rider(&mut self, editor: &mut WorldEditor, roll: u32) {
        self.horse(editor, (roll >> 16).is_multiple_of(2));
        // The rider, sitting tall in the saddle.
        let seat = 1.5;
        self.part(
            editor,
            [0.0, seat, -0.05],
            [-0.18, 0.0, -0.12],
            [0.36, 0.58, 0.24],
            NO_TURN,
        );
        self.head(editor, seat + 0.56, [-4.0, 0.0, 0.0]);
        for side in [-1.0, 1.0] {
            let hip = [0.16 * side, seat + 0.1, -0.05];
            let leg = limb([-55.0, 0.0, 22.0 * side as f32], [65.0, 0.0, 0.0]);
            let (_, _, ankle) = self.limb(editor, hip, leg, [THIGH, SHIN], [true, true]);
            self.part(
                editor,
                ankle,
                [-0.055, -0.05, -0.07],
                [0.11, 0.08, 0.26],
                NO_TURN,
            );
        }
        // The right arm raises a sword or points a baton; the left holds the reins.
        let commanding = (roll >> 17).is_multiple_of(2);
        let right = if commanding {
            limb([-125.0, 0.0, -12.0], [-10.0, 0.0, 0.0])
        } else {
            limb([-70.0, 0.0, -10.0], [-15.0, 0.0, 0.0])
        };
        let left = limb([-25.0, 0.0, 6.0], [-55.0, -15.0, 0.0]);
        let mut grip = ([0.0; 3], NO_TURN);
        for (side, arm) in [(-1.0, right), (1.0, left)] {
            let shoulder = [SHOULDER[0] * side, seat + 0.52, -0.05];
            let (lower, _, end) =
                self.limb(editor, shoulder, arm, [UPPER_ARM, FOREARM], [true, true]);
            if side < 0.0 {
                grip = (add(end, rotate(lower, [0.0, 0.05, 0.0])), lower);
            }
        }
        if commanding {
            self.sword(editor, grip.0, grip.1);
        } else {
            self.part(
                editor,
                grip.0,
                [-0.025, -0.3, -0.025],
                [0.05, 0.36, 0.05],
                grip.1,
            );
        }
    }

    /// Head and shoulders, the chest cut off level with the pedestal's top.
    fn bust(&mut self, editor: &mut WorldEditor) {
        self.part(
            editor,
            [0.0; 3],
            [-0.22, 0.0, -0.12],
            [0.44, 0.2, 0.24],
            NO_TURN,
        );
        // The shoulders, falling away from the neck.
        self.part(
            editor,
            [0.0; 3],
            [-0.16, 0.2, -0.1],
            [0.32, 0.06, 0.2],
            NO_TURN,
        );
        self.head(editor, 0.25, [4.0, 0.0, 0.0]);
    }

    /// A sculpture: a horse where its name says so, else an abstract twisted column or
    /// stones balanced one on another.
    fn sculpture(&mut self, editor: &mut WorldEditor, roll: u32, tags: &HashMap<String, String>) {
        if names_a_horse(tags) {
            self.horse(editor, (roll >> 16).is_multiple_of(2));
        } else if (roll >> 20).is_multiple_of(2) {
            // Slabs stacked, each turned a little further.
            for k in 0..7 {
                self.part(
                    editor,
                    [0.0, 0.06 + f64::from(k) * 0.4, 0.0],
                    [-0.4, 0.0, -0.4],
                    [0.8, 0.34, 0.8],
                    quat([0.0, k as f32 * 13.0, 0.0]),
                );
            }
        } else {
            // Stones balanced one on another, each smaller and turned its own way.
            let mut y = 0.0;
            for (size, turn) in [
                ([1.1, 0.55, 0.8], [0.0, 0.0, 3.0]),
                ([0.85, 0.5, 0.65], [0.0, 37.0, -4.0]),
                ([0.66, 0.45, 0.52], [3.0, -21.0, 0.0]),
                ([0.46, 0.4, 0.4], [0.0, 52.0, 5.0]),
            ] {
                self.part(
                    editor,
                    [0.0, y, 0.0],
                    [-size[0] / 2.0, 0.0, -size[2] / 2.0],
                    size,
                    quat(turn),
                );
                y += size[1] - 0.04;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(b).all(|(p, q)| (p - q).abs() < 1e-9)
    }

    #[test]
    fn a_negative_x_turn_raises_an_arm_forward() {
        let hand = rotate(quat([-90.0, 0.0, 0.0]), [0.0, -1.0, 0.0]);
        assert!(close(hand, [0.0, 0.0, 1.0]), "{hand:?}");
    }

    #[test]
    fn a_turn_is_made_about_x_then_y_then_z() {
        // Raised forward, then swung a quarter turn towards +x.
        let hand = rotate(quat([-90.0, 90.0, 0.0]), [0.0, -1.0, 0.0]);
        assert!(close(hand, [1.0, 0.0, 0.0]), "{hand:?}");
    }

    #[test]
    fn tags_name_the_form() {
        let tags = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<_, _>>()
        };
        assert_eq!(
            form(&tags(&[("historic", "memorial"), ("memorial", "statue")])),
            Some(Form::Statue)
        );
        assert_eq!(
            form(&tags(&[("tourism", "artwork"), ("artwork_type", "bust")])),
            Some(Form::Bust)
        );
        assert_eq!(
            form(&tags(&[("historic", "memorial"), ("memorial", "plaque")])),
            None
        );
    }

    #[test]
    fn figures_on_or_in_buildings_are_left_to_them() {
        let statue = |extra: (&str, &str)| {
            HashMap::from([
                ("historic".to_string(), "memorial".to_string()),
                ("memorial".to_string(), "statue".to_string()),
                (extra.0.to_string(), extra.1.to_string()),
            ])
        };
        assert_eq!(form(&statue(("level", "0"))), Some(Form::Statue));
        for extra in [
            ("level", "1"),
            ("support", "wall_mounted"),
            ("covered", "yes"),
            ("indoor", "yes"),
        ] {
            assert_eq!(form(&statue(extra)), None, "{extra:?}");
        }
        let housefigure = HashMap::from([
            ("historic".to_string(), "housefigure".to_string()),
            ("tourism".to_string(), "artwork".to_string()),
            ("artwork_type".to_string(), "statue".to_string()),
        ]);
        assert_eq!(form(&housefigure), None);
    }

    fn sloped_editor() -> WorldEditor<'static> {
        use crate::coordinate_system::cartesian::XZBBox;
        use crate::coordinate_system::geographic::LLBBox;
        use crate::ground::Ground;
        use std::sync::Arc;

        // Ground rising one block every two to the east.
        let heights: Vec<Vec<f32>> = (0..64)
            .map(|_| (0..64).map(|x| x as f32 / 2.0).collect())
            .collect();
        // The editor borrows its bbox for as long as it lives.
        let bbox = Box::leak(Box::new(XZBBox::rect_from_min_max(0, 0, 63, 63).unwrap()));
        let llbbox = LLBBox::new(47.0, 8.0, 47.001, 8.001).unwrap();
        let mut editor = WorldEditor::new(std::env::temp_dir(), bbox, llbbox);
        editor.set_ground(Arc::new(Ground::new_elevation_test(heights, 64, 64)));
        editor.set_ground_origin(0, 0);
        editor
    }

    fn node(id: u64, tags: &[(&str, &str)]) -> ProcessedNode {
        ProcessedNode {
            id,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            x: 32,
            z: 32,
        }
    }

    #[test]
    fn a_statue_on_a_slope_stands_on_a_plinth_from_its_lowest_ground() {
        let mut editor = sloped_editor();
        let low = lowest_ground(&editor, 32, 32, 1);
        assert!(
            editor.get_ground_level(33, 32) > low,
            "the plinth spans the slope"
        );
        let statue = node(7, &[("historic", "memorial"), ("memorial", "statue")]);
        place(&mut editor, &statue, Form::Statue);
        // The plinth's downhill stairs sit on the lowest ground, not in the air.
        assert!(editor.get_block_absolute(31, low + 1, 32).is_some());
        let parts = editor.entities_of("minecraft:block_display");
        assert!(parts.len() >= 10, "a figure of {} parts", parts.len());
        // The figure stands on the cap, a bottom slab three blocks up.
        let Some(Value::List(pos)) = parts[0].get("Pos") else {
            panic!("no position")
        };
        assert_eq!(pos[1], Value::Double(f64::from(low + 3) + 0.5));
    }

    #[test]
    fn a_tall_statue_stands_on_a_column() {
        let mut editor = sloped_editor();
        let low = lowest_ground(&editor, 32, 32, 2);
        let statue = node(
            8,
            &[
                ("historic", "memorial"),
                ("memorial", "statue"),
                ("height", "13.8"),
            ],
        );
        place(&mut editor, &statue, Form::Statue);
        // Fourteen blocks up: the column to eleven, the figure on top.
        assert_eq!(
            editor.get_block_absolute(32, low + 11, 32),
            Some(CHISELED_QUARTZ_BLOCK)
        );
        assert_eq!(
            editor.get_block_absolute(32, low + 8, 32),
            Some(QUARTZ_PILLAR)
        );
        let parts = editor.entities_of("minecraft:block_display");
        assert!(!parts.is_empty());
        for part in &parts {
            let Some(Value::List(pos)) = part.get("Pos") else {
                panic!("no position")
            };
            assert_eq!(pos[1], Value::Double(f64::from(low + 12)));
        }
    }

    #[test]
    fn an_equestrian_statue_is_a_rider_on_a_wide_plinth() {
        let mut editor = sloped_editor();
        let low = lowest_ground(&editor, 32, 32, 2);
        let statue = node(
            9,
            &[
                ("historic", "memorial"),
                ("memorial", "statue"),
                ("statue", "equestrian"),
            ],
        );
        place(&mut editor, &statue, Form::Statue);
        // Two blocks out on every side.
        assert!(editor.get_block_absolute(30, low + 1, 30).is_some());
        assert!(editor.get_block_absolute(34, low + 1, 34).is_some());
        // A horse's barrel, legs, neck, head, ears and tail and its rider.
        assert!(editor.entities_of("minecraft:block_display").len() >= 25);
    }

    #[test]
    fn a_tall_rider_stays_on_a_plinth() {
        let mut editor = sloped_editor();
        let statue = node(
            10,
            &[
                ("historic", "memorial"),
                ("memorial", "statue"),
                ("statue", "equestrian"),
                ("height", "8"),
            ],
        );
        place(&mut editor, &statue, Form::Statue);
        let low = lowest_ground(&editor, 32, 32, 2);
        assert!(
            editor.get_block_absolute(32, low + 5, 32).is_none(),
            "no column"
        );
        assert!(editor.entities_of("minecraft:block_display").len() >= 25);
    }

    #[test]
    fn a_statue_on_a_short_column_keeps_its_figure() {
        let mut editor = sloped_editor();
        let low = lowest_ground(&editor, 32, 32, 2);
        let statue = node(
            11,
            &[
                ("historic", "memorial"),
                ("memorial", "statue"),
                ("height", "7"),
            ],
        );
        place(&mut editor, &statue, Form::Statue);
        // The capital right on the base, the figure on it.
        assert_eq!(
            editor.get_block_absolute(32, low + 5, 32),
            Some(CHISELED_QUARTZ_BLOCK)
        );
        assert!(!editor.entities_of("minecraft:block_display").is_empty());
    }

    #[test]
    fn a_monument_carries_a_figure_only_when_tagged_as_one() {
        let mut editor = sloped_editor();
        let low = lowest_ground(&editor, 32, 32, 2);
        column(
            &mut editor,
            &node(12, &[("historic", "monument")]),
            10.0,
            false,
        );
        assert!(editor.entities_of("minecraft:block_display").is_empty());
        assert_eq!(
            editor.get_block_absolute(32, low + 10, 32),
            Some(CHISELED_QUARTZ_BLOCK)
        );
        assert_eq!(
            editor.get_block_absolute(32, low + 11, 32),
            Some(SMOOTH_QUARTZ_SLAB)
        );
    }

    #[test]
    fn a_pedestal_goes_through_a_tree_but_not_a_wall() {
        let mut editor = sloped_editor();
        let low = lowest_ground(&editor, 32, 32, 1);
        editor.set_block_absolute(STONE_BRICKS, 31, low + 1, 31, None, None);
        editor.set_block_absolute(OAK_LEAVES, 33, low + 1, 33, None, None);
        let statue = node(13, &[("historic", "memorial"), ("memorial", "statue")]);
        place(&mut editor, &statue, Form::Statue);
        assert_eq!(
            editor.get_block_absolute(31, low + 1, 31),
            Some(STONE_BRICKS)
        );
        let corner = editor.get_block_absolute(33, low + 1, 33);
        assert!(
            corner.is_some_and(|b| b.name().ends_with("_stairs")),
            "{corner:?}"
        );
    }

    #[test]
    fn compass_points_turn_the_figure_and_nonsense_does_not() {
        let tags = |d: &str| HashMap::from([("direction".to_string(), d.to_string())]);
        // Facing north-east is a yaw of 225.
        assert_eq!(facing_yaw(&tags("NE"), 0), 225.0);
        assert_eq!(facing_yaw(&tags("ssw"), 0), 22.5);
        let yaw = facing_yaw(&tags("inf"), 5);
        assert!(yaw.is_finite() && yaw % 90.0 == 0.0);
    }

    #[test]
    fn a_sculpture_named_for_horses_is_a_horse() {
        let tags = |name: &str| {
            HashMap::from([
                ("tourism".to_string(), "artwork".to_string()),
                ("artwork_type".to_string(), "sculpture".to_string()),
                ("name".to_string(), name.to_string()),
            ])
        };
        assert!(names_a_horse(&tags("Springende Pferde")));
        assert!(names_a_horse(&tags("The Horse Tamer")));
        assert!(!names_a_horse(&tags("Mann mit ausgebreiteten Armen")));
        assert!(!names_a_horse(&tags("Grosser Brunnen")));
    }

    #[test]
    fn a_mapped_direction_turns_the_figure_that_way() {
        let tags = HashMap::from([("direction".to_string(), "N".to_string())]);
        // Facing north is a yaw of 180.
        assert_eq!(facing_yaw(&tags, 0), 180.0);
        let tags = HashMap::from([("direction".to_string(), "270".to_string())]);
        assert_eq!(facing_yaw(&tags, 0), 90.0);
    }
}
