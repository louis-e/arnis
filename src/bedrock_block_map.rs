//! Bedrock Block Mapping
//!
//! Turns the internal Java-style blocks into Bedrock states of the 1.26.30 palette, the
//! newest published one. The game derives the stair corners and fence connections 26.50
//! added on load. A test checks every block against `tests/fixtures/bedrock_block_states.json`.

use crate::block_definitions::Block;
use fastnbt::Value;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};

/// The block state version of every palette entry, 1.21.60.33, still current in 26.50.
pub const BLOCK_STATE_VERSION: i32 = (1 << 24) | (21 << 16) | (60 << 8) | 33;

/// Represents a Bedrock block with its identifier and state properties.
///
/// Uses `BTreeMap` for deterministic iteration order, which is required for
/// correct `Hash`/`Eq` implementations (used as palette dedup key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BedrockBlock {
    /// The Bedrock block identifier (e.g., "minecraft:stone")
    pub name: String,
    /// Block state properties as key-value pairs
    pub states: BTreeMap<String, BedrockBlockStateValue>,
}

/// `BTreeMap` does not implement `Hash`, so we hash entries in sorted-key order
/// (guaranteed by `BTreeMap::iter`).
impl Hash for BedrockBlock {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        for (k, v) in &self.states {
            k.hash(state);
            v.hash(state);
        }
    }
}

/// Bedrock block state values can be strings, booleans, or integers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BedrockBlockStateValue {
    String(String),
    Bool(bool),
    Int(i32),
}

/// A state value as NBT: bools are bytes.
impl From<&BedrockBlockStateValue> for Value {
    fn from(value: &BedrockBlockStateValue) -> Self {
        match value {
            BedrockBlockStateValue::String(s) => Value::String(s.clone()),
            BedrockBlockStateValue::Bool(b) => Value::Byte(*b as i8),
            BedrockBlockStateValue::Int(i) => Value::Int(*i),
        }
    }
}

impl BedrockBlock {
    /// Creates a simple block with no state properties.
    pub fn simple(name: &str) -> Self {
        Self {
            name: format!("minecraft:{name}"),
            states: BTreeMap::new(),
        }
    }

    /// Creates a block with state properties.
    pub fn with_states(name: &str, states: Vec<(&str, BedrockBlockStateValue)>) -> Self {
        let mut state_map = BTreeMap::new();
        for (key, value) in states {
            state_map.insert(key.to_string(), value);
        }
        Self {
            name: format!("minecraft:{name}"),
            states: state_map,
        }
    }
}

/// A block's Java properties, stored as strings as Java does.
struct Props<'a>(Option<&'a HashMap<String, Value>>);

impl Props<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        match self.0?.get(key)? {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// The value, or Java's default for the block when it is unset.
    fn or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).unwrap_or(default)
    }

    fn flag(&self, key: &str, default: bool) -> bool {
        self.get(key).map_or(default, |v| v == "true")
    }

    fn int(&self, key: &str, default: i32) -> i32 {
        match self.0.and_then(|m| m.get(key)) {
            Some(Value::String(s)) => s.parse().unwrap_or(default),
            Some(Value::Int(i)) => *i,
            Some(Value::Byte(b)) => i32::from(*b),
            Some(Value::Short(s)) => i32::from(*s),
            _ => default,
        }
    }
}

use BedrockBlockStateValue::{Bool, Int, String as Str};

fn s(value: &str) -> BedrockBlockStateValue {
    Str(value.to_string())
}

/// Bedrock's `facing_direction`: down, up, north, south, west, east.
fn facing_direction(facing: &str) -> i32 {
    match facing {
        "down" => 0,
        "up" => 1,
        "south" => 3,
        "west" => 4,
        "east" => 5,
        _ => 2,
    }
}

/// The legacy `direction` of beds, looms and the like: south, west, north, east.
fn legacy_direction(facing: &str) -> i32 {
    match facing {
        "west" => 1,
        "north" => 2,
        "east" => 3,
        _ => 0,
    }
}

/// The `direction` of stairs (`weirdo_direction`) and trapdoors: east, west, south, north.
fn weirdo_direction(facing: &str) -> i32 {
    match facing {
        "west" => 1,
        "south" => 2,
        "north" => 3,
        _ => 0,
    }
}

/// Geyser's `huge_mushroom_bits` by Java faces: down 1, east 2, north 4, south 8, up 16, west 32.
const MUSHROOM_CAP_BITS: [u8; 64] = [
    0, 14, 6, 6, 2, 2, 3, 3, 8, 8, 9, 9, 2, 2, 3, 3, 5, 5, 6, 6, 2, 2, 3, 3, 8, 8, 9, 9, 2, 2, 3,
    3, 4, 4, 4, 4, 1, 1, 1, 1, 7, 7, 7, 7, 1, 1, 1, 1, 4, 4, 4, 4, 1, 1, 1, 1, 7, 7, 7, 7, 1, 1,
    14, 14,
];
const MUSHROOM_STEM_BITS: [u8; 64] = [
    0, 15, 10, 15, 10, 15, 10, 15, 10, 15, 10, 15, 10, 15, 10, 15, 15, 15, 15, 15, 15, 15, 15, 15,
    15, 15, 15, 15, 15, 15, 15, 15, 10, 15, 10, 15, 10, 15, 10, 15, 10, 15, 10, 15, 10, 15, 10, 15,
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15,
];

/// Bedrock ids that differ from the Java ones.
fn renamed(java_name: &str) -> &str {
    match java_name {
        "bricks" => "brick_block",
        "cobweb" => "web",
        "dead_bush" => "deadbush",
        "dirt_path" => "grass_path",
        "end_stone_bricks" => "end_bricks",
        "flowering_azalea_leaves" => "azalea_leaves_flowered",
        "lily_pad" => "waterlily",
        "magma_block" => "magma",
        "melon" => "melon_block",
        "nether_bricks" => "nether_brick",
        "note_block" => "noteblock",
        "red_nether_bricks" => "red_nether_brick",
        "snow_block" => "snow",
        "sugar_cane" => "reeds",
        "terracotta" => "hardened_clay",
        "waxed_copper_block" => "waxed_copper",
        "chain" => "iron_chain",
        "oak_button" => "wooden_button",
        "oak_door" => "wooden_door",
        "oak_fence_gate" => "fence_gate",
        "oak_pressure_plate" => "wooden_pressure_plate",
        "oak_trapdoor" => "trapdoor",
        "cobblestone_stairs" => "stone_stairs",
        "stone_stairs" => "normal_stone_stairs",
        "end_stone_brick_stairs" => "end_brick_stairs",
        "stone_slab" => "normal_stone_slab",
        other => other,
    }
}

/// Whether a block stands in water, which Bedrock stores as a second block in the cell.
/// Java's `waterlogged` decides; without it, the blocks that are wet in Java by default.
pub fn is_waterlogged(block: Block, java_properties: Option<&Value>) -> bool {
    let fallback = block.properties();
    if let Some(Value::Compound(map)) = java_properties.or(fallback.as_ref()) {
        if let Some(Value::String(v)) = map.get("waterlogged") {
            return v == "true";
        }
    }
    let name = block.name();
    matches!(
        name,
        "kelp"
            | "kelp_plant"
            | "seagrass"
            | "tall_seagrass"
            | "bubble_column"
            | "sea_pickle"
            | "conduit"
    ) || name.ends_with("_coral")
        || name.ends_with("_coral_fan")
        || name.ends_with("_coral_wall_fan")
}

/// The plant in a Java potted block, which Bedrock keeps in the pot's block entity.
pub fn potted_plant(block: Block) -> Option<BedrockBlock> {
    let plant = block.name().strip_prefix("potted_")?;
    Some(convert(plant, &Props(None)))
}

/// The Bedrock state of a block, with its own default properties when none are stored.
pub fn to_bedrock_block_with_properties(
    block: Block,
    java_properties: Option<&Value>,
) -> BedrockBlock {
    let fallback = block.properties();
    let map = match java_properties.or(fallback.as_ref()) {
        Some(Value::Compound(map)) => Some(map),
        _ => None,
    };
    convert(block.name(), &Props(map))
}

fn convert(name: &str, p: &Props) -> BedrockBlock {
    let id = renamed(name);
    // Every block below that reads `facing` only turns horizontally.
    let facing = match p.or("facing", "north") {
        f @ ("south" | "west" | "east") => f,
        _ => "north",
    };
    let with = |states: Vec<(&str, BedrockBlockStateValue)>| BedrockBlock::with_states(id, states);

    match name {
        n if n.ends_with("_stairs") => with(vec![
            ("weirdo_direction", Int(weirdo_direction(facing))),
            ("upside_down_bit", Bool(p.or("half", "bottom") == "top")),
        ]),
        n if n.ends_with("_slab") => {
            let slab_type = p.or("type", "bottom");
            let id = if slab_type != "double" {
                id.to_string()
            } else if let Some(prefix) = id.strip_suffix("cut_copper_slab") {
                format!("{prefix}double_cut_copper_slab")
            } else {
                format!("{}_double_slab", id.trim_end_matches("_slab"))
            };
            let half = if slab_type == "top" { "top" } else { "bottom" };
            BedrockBlock::with_states(&id, vec![("minecraft:vertical_half", s(half))])
        }
        n if n.ends_with("_wall_banner") || n.ends_with("_wall_sign") || n == "ladder" => {
            let id = if n.ends_with("_wall_banner") {
                "wall_banner"
            } else {
                id
            };
            BedrockBlock::with_states(
                id,
                vec![("facing_direction", Int(facing_direction(facing)))],
            )
        }
        n if n.ends_with("_wall") => {
            let connection = |side: &str| match p.or(side, "none") {
                "low" => "short",
                "tall" => "tall",
                _ => "none",
            };
            with(vec![
                ("wall_connection_type_east", s(connection("east"))),
                ("wall_connection_type_north", s(connection("north"))),
                ("wall_connection_type_south", s(connection("south"))),
                ("wall_connection_type_west", s(connection("west"))),
                ("wall_post_bit", Bool(p.flag("up", true))),
            ])
        }
        // Always persistent: hedges and trees away from a log would decay otherwise.
        n if n.ends_with("_leaves") => with(vec![
            ("persistent_bit", Bool(true)),
            ("update_bit", Bool(false)),
        ]),
        n if n.ends_with("_trapdoor") => with(vec![
            ("direction", Int(weirdo_direction(facing))),
            ("open_bit", Bool(p.flag("open", false))),
            ("upside_down_bit", Bool(p.or("half", "bottom") == "top")),
        ]),
        // A Bedrock door records the facing turned a quarter clockwise.
        n if n.ends_with("_door") => {
            let turned = match facing {
                "east" => "south",
                "south" => "west",
                "west" => "north",
                _ => "east",
            };
            with(vec![
                ("door_hinge_bit", Bool(p.or("hinge", "left") == "right")),
                ("minecraft:cardinal_direction", s(turned)),
                ("open_bit", Bool(p.flag("open", false))),
                ("upper_block_bit", Bool(p.or("half", "lower") == "upper")),
            ])
        }
        n if n.ends_with("_fence_gate") => with(vec![
            ("in_wall_bit", Bool(p.flag("in_wall", false))),
            ("minecraft:cardinal_direction", s(facing)),
            ("open_bit", Bool(p.flag("open", false))),
        ]),
        n if n.ends_with("_button") => {
            let direction = match p.or("face", "wall") {
                "floor" => 1,
                "ceiling" => 0,
                _ => facing_direction(facing),
            };
            with(vec![
                ("button_pressed_bit", Bool(p.flag("powered", false))),
                ("facing_direction", Int(direction)),
            ])
        }
        n if n.ends_with("_pressure_plate") => with(vec![(
            "redstone_signal",
            Int(if p.flag("powered", false) { 15 } else { 0 }),
        )]),
        // The colour lives in the Bed block entity.
        n if n.ends_with("_bed") => BedrockBlock::with_states(
            "bed",
            vec![
                ("direction", Int(legacy_direction(facing))),
                ("head_piece_bit", Bool(p.or("part", "foot") == "head")),
                ("occupied_bit", Bool(p.flag("occupied", false))),
            ],
        ),
        n if n.ends_with("_log")
            || n.ends_with("_wood")
            || n.ends_with("_hyphae")
            || n.ends_with("crimson_stem")
            || n.ends_with("warped_stem")
            || matches!(
                n,
                "chain"
                    | "basalt"
                    | "polished_basalt"
                    | "deepslate"
                    | "quartz_pillar"
                    | "purpur_pillar"
            ) =>
        {
            with(vec![("pillar_axis", s(p.or("axis", "y")))])
        }
        "hay_block" | "bone_block" => with(vec![
            ("deprecated", Int(0)),
            ("pillar_axis", s(p.or("axis", "y"))),
        ]),
        // Java has no axis on these; Bedrock still stores one.
        "chiseled_quartz_block" | "purpur_block" | "quartz_block" | "smooth_quartz" => {
            with(vec![("pillar_axis", s("y"))])
        }
        n if n.ends_with("_coral_fan") => with(vec![("coral_fan_direction", Int(0))]),
        // The plant lives in the FlowerPot block entity.
        n if n == "flower_pot" || n.starts_with("potted_") => {
            BedrockBlock::with_states("flower_pot", vec![("update_bit", Bool(false))])
        }
        "tall_grass" | "large_fern" | "sunflower" | "lilac" | "rose_bush" | "peony" => {
            with(vec![(
                "upper_block_bit",
                Bool(p.or("half", "lower") == "upper"),
            )])
        }
        "amethyst_cluster"
        | "small_amethyst_bud"
        | "medium_amethyst_bud"
        | "large_amethyst_bud" => with(vec![("minecraft:block_face", s(p.or("facing", "up")))]),
        "anvil" | "chipped_anvil" | "damaged_anvil" | "chest" | "trapped_chest" | "ender_chest" => {
            with(vec![("minecraft:cardinal_direction", s(facing))])
        }
        "furnace" | "blast_furnace" | "smoker" => {
            let id = if p.flag("lit", false) {
                format!("lit_{id}")
            } else {
                id.to_string()
            };
            BedrockBlock::with_states(&id, vec![("minecraft:cardinal_direction", s(facing))])
        }
        "lectern" => with(vec![
            ("minecraft:cardinal_direction", s(facing)),
            ("powered_bit", Bool(p.flag("powered", false))),
        ]),
        "pumpkin" => with(vec![("minecraft:cardinal_direction", s("north"))]),
        "big_dripleaf" | "big_dripleaf_stem" => {
            let head = name == "big_dripleaf";
            let tilt = match p.or("tilt", "none") {
                "full" if head => "full_tilt",
                "partial" if head => "partial_tilt",
                "unstable" if head => "unstable",
                _ => "none",
            };
            BedrockBlock::with_states(
                "big_dripleaf",
                vec![
                    ("big_dripleaf_head", Bool(head)),
                    ("big_dripleaf_tilt", s(tilt)),
                    ("minecraft:cardinal_direction", s(facing)),
                ],
            )
        }
        // Small dripleaf faces are turned a quarter counter-clockwise on Bedrock.
        "small_dripleaf" => {
            let turned = match facing {
                "east" => "north",
                "south" => "east",
                "west" => "south",
                _ => "west",
            };
            BedrockBlock::with_states(
                "small_dripleaf_block",
                vec![
                    ("minecraft:cardinal_direction", s(turned)),
                    ("upper_block_bit", Bool(p.or("half", "lower") == "upper")),
                ],
            )
        }
        "cactus" | "fire" | "sugar_cane" => with(vec![("age", Int(p.int("age", 0)))]),
        "wheat" | "carrots" | "potatoes" | "beetroots" | "sweet_berry_bush" => {
            with(vec![("growth", Int(p.int("age", 0)))])
        }
        "red_mushroom_block" | "brown_mushroom_block" | "mushroom_stem" => {
            let faces = ["down", "east", "north", "south", "up", "west"]
                .iter()
                .enumerate()
                .filter(|(_, face)| p.flag(face, true))
                .fold(0, |bits, (i, _)| bits | (1 << i));
            let table = if name == "mushroom_stem" {
                &MUSHROOM_STEM_BITS
            } else {
                &MUSHROOM_CAP_BITS
            };
            with(vec![("huge_mushroom_bits", Int(i32::from(table[faces])))])
        }
        "glow_lichen" | "sculk_vein" => {
            let bits = ["down", "up", "south", "west", "north", "east"]
                .iter()
                .enumerate()
                .filter(|(_, face)| p.flag(face, false))
                .fold(0, |bits, (i, _)| bits | (1 << i));
            // A face-less lichen cannot exist; it lies on the floor.
            with(vec![(
                "multi_face_direction_bits",
                Int(if bits == 0 { 1 } else { bits }),
            )])
        }
        "vine" => {
            let bits = [("south", 1), ("west", 2), ("north", 4), ("east", 8)]
                .iter()
                .filter(|(face, _)| p.flag(face, false))
                .map(|(_, bit)| bit)
                .sum();
            with(vec![("vine_direction_bits", Int(bits))])
        }
        "lantern" | "soul_lantern" => with(vec![("hanging", Bool(p.flag("hanging", false)))]),
        "water" | "lava" => {
            let level = p.int("level", 0);
            let id = if level == 0 {
                id.to_string()
            } else {
                format!("flowing_{id}")
            };
            BedrockBlock::with_states(&id, vec![("liquid_depth", Int(level))])
        }
        "barrel" => with(vec![
            (
                "facing_direction",
                Int(facing_direction(p.or("facing", "north"))),
            ),
            ("open_bit", Bool(p.flag("open", false))),
        ]),
        "dispenser" | "dropper" => with(vec![
            (
                "facing_direction",
                Int(facing_direction(p.or("facing", "north"))),
            ),
            ("triggered_bit", Bool(p.flag("triggered", false))),
        ]),
        "hopper" => with(vec![
            (
                "facing_direction",
                Int(facing_direction(p.or("facing", "down"))),
            ),
            ("toggle_bit", Bool(p.flag("enabled", true))),
        ]),
        "lightning_rod" => with(vec![
            (
                "facing_direction",
                Int(facing_direction(p.or("facing", "up"))),
            ),
            ("powered_bit", Bool(p.flag("powered", false))),
        ]),
        // End rods swap the horizontal directions.
        "end_rod" => {
            let direction = match p.or("facing", "up") {
                "down" => 0,
                "north" => 3,
                "south" => 2,
                "west" => 5,
                "east" => 4,
                _ => 1,
            };
            with(vec![("facing_direction", Int(direction))])
        }
        "bedrock" => with(vec![("infiniburn_bit", Bool(false))]),
        "brewing_stand" => with(vec![
            (
                "brewing_stand_slot_a_bit",
                Bool(p.flag("has_bottle_0", false)),
            ),
            (
                "brewing_stand_slot_b_bit",
                Bool(p.flag("has_bottle_1", false)),
            ),
            (
                "brewing_stand_slot_c_bit",
                Bool(p.flag("has_bottle_2", false)),
            ),
        ]),
        "cake" => with(vec![("bite_counter", Int(p.int("bites", 0)))]),
        "cauldron" => with(vec![
            ("cauldron_liquid", s("water")),
            ("fill_level", Int(0)),
        ]),
        "water_cauldron" => {
            // Arnis places these without a level and means them full.
            let fill = match p.int("level", 3) {
                2 => 4,
                3 => 6,
                _ => 3,
            };
            BedrockBlock::with_states(
                "cauldron",
                vec![("cauldron_liquid", s("water")), ("fill_level", Int(fill))],
            )
        }
        // Bedrock splits the vines by berries and by head or body rather than using states.
        "cave_vines" | "cave_vines_plant" => {
            let head = name == "cave_vines";
            let id = match (head, p.flag("berries", false)) {
                (true, true) => "cave_vines_head_with_berries",
                (false, true) => "cave_vines_body_with_berries",
                _ => "cave_vines",
            };
            let age = if head { p.int("age", 0) } else { 0 };
            BedrockBlock::with_states(id, vec![("growing_plant_age", Int(age))])
        }
        "chiseled_bookshelf" => {
            let books = (0..6)
                .filter(|i| p.flag(&format!("slot_{i}_occupied"), false))
                .fold(0, |bits, i| bits | (1 << i));
            with(vec![
                ("books_stored", Int(books)),
                ("direction", Int(legacy_direction(facing))),
            ])
        }
        "composter" => with(vec![("composter_fill_level", Int(p.int("level", 0)))]),
        "daylight_detector" => {
            let id = if p.flag("inverted", false) {
                "daylight_detector_inverted"
            } else {
                "daylight_detector"
            };
            BedrockBlock::with_states(id, vec![("redstone_signal", Int(p.int("power", 0)))])
        }
        // Wet unless stated, so a field without a crop does not dry out to dirt.
        "farmland" => with(vec![("moisturized_amount", Int(p.int("moisture", 7)))]),
        "grindstone" => {
            let attachment = match p.or("face", "wall") {
                "floor" => "standing",
                "ceiling" => "hanging",
                _ => "side",
            };
            with(vec![
                ("attachment", s(attachment)),
                ("direction", Int(legacy_direction(facing))),
            ])
        }
        "loom" => with(vec![("direction", Int(legacy_direction(facing)))]),
        "tripwire_hook" => with(vec![
            ("attached_bit", Bool(p.flag("attached", false))),
            ("direction", Int(legacy_direction(facing))),
            ("powered_bit", Bool(p.flag("powered", false))),
        ]),
        // A generated kelp column should stay as placed, so an unset age is fully grown.
        "kelp" => with(vec![("kelp_age", Int(p.int("age", 25)))]),
        "kelp_plant" => BedrockBlock::with_states("kelp", vec![("kelp_age", Int(0))]),
        "lever" => {
            let face = p.or("face", "wall");
            let north_south = matches!(facing, "north" | "south");
            let direction = match face {
                "floor" if north_south => "up_north_south",
                "floor" => "up_east_west",
                "ceiling" if north_south => "down_north_south",
                "ceiling" => "down_east_west",
                _ => facing,
            };
            // On the floor and ceiling the throw also says which way the lever points.
            let flipped = face != "wall" && matches!(facing, "south" | "east");
            with(vec![
                ("lever_direction", s(direction)),
                ("open_bit", Bool(p.flag("powered", false) != flipped)),
            ])
        }
        // Java's rotation and Bedrock's ground_sign_direction share their scale and origin.
        "oak_sign" => BedrockBlock::with_states(
            "standing_sign",
            vec![(
                "ground_sign_direction",
                Int(p.int("rotation", 0).rem_euclid(16)),
            )],
        ),
        "pointed_dripstone" => {
            let thickness = match p.or("thickness", "tip") {
                "tip_merge" => "merge",
                other => other,
            };
            with(vec![
                ("dripstone_thickness", s(thickness)),
                ("hanging", Bool(p.or("vertical_direction", "up") == "down")),
            ])
        }
        "rail" | "powered_rail" => {
            let direction = match p.or("shape", "north_south") {
                "east_west" => 1,
                "ascending_east" => 2,
                "ascending_west" => 3,
                "ascending_north" => 4,
                "ascending_south" => 5,
                "south_east" => 6,
                "south_west" => 7,
                "north_west" => 8,
                "north_east" => 9,
                _ => 0,
            };
            if name == "rail" {
                with(vec![("rail_direction", Int(direction))])
            } else {
                // Powered rails cannot curve.
                let direction = if direction > 5 { 0 } else { direction };
                BedrockBlock::with_states(
                    "golden_rail",
                    vec![
                        ("rail_data_bit", Bool(p.flag("powered", false))),
                        ("rail_direction", Int(direction)),
                    ],
                )
            }
        }
        // Bedrock's torch_facing_direction names the opposite of the way the torch
        // points (MCPE-152036), so the horizontal values are swapped.
        "redstone_torch" | "redstone_wall_torch" | "torch" | "wall_torch" => {
            let redstone = name.starts_with("redstone");
            let id = match (redstone, p.flag("lit", true)) {
                (true, true) => "redstone_torch",
                (true, false) => "unlit_redstone_torch",
                (false, _) => "torch",
            };
            let direction = if name.ends_with("wall_torch") {
                match facing {
                    "south" => "north",
                    "east" => "west",
                    "west" => "east",
                    _ => "south",
                }
            } else {
                "top"
            };
            BedrockBlock::with_states(id, vec![("torch_facing_direction", s(direction))])
        }
        "redstone_lamp" | "redstone_ore" | "deepslate_redstone_ore" if p.flag("lit", false) => {
            BedrockBlock::simple(&format!("lit_{name}"))
        }
        "light" => {
            BedrockBlock::simple(&format!("light_block_{}", p.int("level", 15).clamp(0, 15)))
        }
        "scaffolding" => with(vec![
            ("stability", Int(p.int("distance", 7))),
            ("stability_check", Bool(true)),
        ]),
        "sculk_catalyst" => with(vec![("bloom", Bool(p.flag("bloom", false)))]),
        "sculk_sensor" => {
            let phase = match p.or("sculk_sensor_phase", "inactive") {
                "active" => 1,
                "cooldown" => 2,
                _ => 0,
            };
            with(vec![("sculk_sensor_phase", Int(phase))])
        }
        "sculk_shrieker" => with(vec![
            ("active", Bool(p.flag("shrieking", false))),
            ("can_summon", Bool(p.flag("can_summon", false))),
        ]),
        // dead_bit marks a pickle out of the water.
        "sea_pickle" => with(vec![
            ("cluster_count", Int((p.int("pickles", 1) - 1).clamp(0, 3))),
            ("dead_bit", Bool(!p.flag("waterlogged", true))),
        ]),
        "seagrass" => with(vec![("sea_grass_type", s("default"))]),
        "tall_seagrass" => {
            let half = if p.or("half", "lower") == "upper" {
                "double_top"
            } else {
                "double_bot"
            };
            BedrockBlock::with_states("seagrass", vec![("sea_grass_type", s(half))])
        }
        "snow" => BedrockBlock::with_states(
            "snow_layer",
            vec![
                ("covered_bit", Bool(false)),
                ("height", Int((p.int("layers", 1) - 1).clamp(0, 7))),
            ],
        ),
        _ => BedrockBlock::simple(id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_definitions::*;
    use std::collections::HashSet;

    fn props(entries: &[(&str, &str)]) -> Value {
        Value::Compound(
            entries
                .iter()
                .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
                .collect(),
        )
    }

    fn convert_with(block: Block, entries: &[(&str, &str)]) -> BedrockBlock {
        to_bedrock_block_with_properties(block, Some(&props(entries)))
    }

    fn state(block: &BedrockBlock, key: &str) -> Option<BedrockBlockStateValue> {
        block.states.get(key).cloned()
    }

    /// Every block of the palette with its states and the values each may take.
    struct Palette(HashMap<String, BTreeMap<String, (String, HashSet<String>)>>);

    impl Palette {
        fn load() -> Self {
            let path = concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/bedrock_block_states.json"
            );
            let json: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            let blocks = json["blocks"].as_object().unwrap();
            Palette(
                blocks
                    .iter()
                    .map(|(name, states)| {
                        let states = states
                            .as_object()
                            .unwrap()
                            .iter()
                            .map(|(key, spec)| {
                                let kind = spec["type"].as_str().unwrap().to_string();
                                let values = spec["values"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .map(|v| v.to_string().trim_matches('"').to_string())
                                    .collect();
                                (key.clone(), (kind, values))
                            })
                            .collect();
                        (name.clone(), states)
                    })
                    .collect(),
            )
        }

        fn check(&self, block: &BedrockBlock) -> Result<(), String> {
            let Some(states) = self.0.get(&block.name) else {
                return Err(format!("{} is not a Bedrock block", block.name));
            };
            let wanted: Vec<_> = states.keys().collect();
            let have: Vec<_> = block.states.keys().collect();
            if wanted != have {
                return Err(format!("{} takes {wanted:?}, got {have:?}", block.name));
            }
            for (key, value) in &block.states {
                let (kind, values) = &states[key];
                let (ok_kind, text) = match value {
                    BedrockBlockStateValue::String(v) => (kind == "string", v.clone()),
                    BedrockBlockStateValue::Bool(v) => (kind == "byte", (*v as u8).to_string()),
                    BedrockBlockStateValue::Int(v) => (kind == "int", v.to_string()),
                };
                if !ok_kind || !values.contains(&text) {
                    return Err(format!(
                        "{}: {key}={text} is not a valid {kind}",
                        block.name
                    ));
                }
            }
            Ok(())
        }
    }

    /// Each block with its defaults and with common properties set one at a time.
    #[test]
    fn every_block_maps_to_a_valid_bedrock_state() {
        let palette = Palette::load();
        let sweeps: &[(&str, &[&str])] = &[
            ("facing", &["north", "south", "east", "west", "up", "down"]),
            ("half", &["top", "bottom", "upper", "lower"]),
            ("type", &["top", "bottom", "double"]),
            ("axis", &["x", "y", "z"]),
            ("face", &["floor", "wall", "ceiling"]),
            ("open", &["true"]),
            ("powered", &["true"]),
            ("lit", &["true", "false"]),
            ("inverted", &["true"]),
            ("berries", &["true"]),
            ("part", &["head"]),
            ("age", &["3", "7"]),
            ("level", &["0", "3", "8"]),
            ("layers", &["8"]),
            ("north", &["true", "low", "tall"]),
            ("up", &["false"]),
            ("shape", &["east_west", "ascending_north", "north_east"]),
            ("thickness", &["tip_merge", "base"]),
            ("tilt", &["partial"]),
        ];
        let mut failures = Vec::new();
        for id in 0..=u16::MAX {
            let block = Block::from_raw_id(id);
            if block.try_name().is_none() {
                continue;
            }
            let base = match block.properties() {
                Some(Value::Compound(map)) => map,
                _ => HashMap::new(),
            };
            let mut variants = vec![to_bedrock_block_with_properties(block, None)];
            for (key, values) in sweeps {
                for value in *values {
                    let mut map = base.clone();
                    map.insert(key.to_string(), Value::String(value.to_string()));
                    variants.push(to_bedrock_block_with_properties(
                        block,
                        Some(&Value::Compound(map)),
                    ));
                }
            }
            for bedrock in variants {
                if let Err(e) = palette.check(&bedrock) {
                    failures.push(format!("{}: {e}", block.name()));
                }
            }
        }
        failures.sort();
        failures.dedup();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn snow_layers_keep_their_height() {
        for (i, layer) in SNOW_LAYERS.iter().enumerate() {
            let bedrock = to_bedrock_block_with_properties(*layer, None);
            assert_eq!(bedrock.name, "minecraft:snow_layer");
            assert_eq!(state(&bedrock, "height"), Some(Int(i as i32)));
        }
    }

    #[test]
    fn flattened_blocks_use_their_own_ids() {
        for (block, name) in [
            (STONE, "minecraft:stone"),
            (GRANITE, "minecraft:granite"),
            (OAK_PLANKS, "minecraft:oak_planks"),
            (WHITE_CONCRETE, "minecraft:white_concrete"),
            (LIGHT_GRAY_CONCRETE, "minecraft:light_gray_concrete"),
            (CYAN_TERRACOTTA, "minecraft:cyan_terracotta"),
            (GRAY_CONCRETE_POWDER, "minecraft:gray_concrete_powder"),
            (RED_FLOWER, "minecraft:poppy"),
            (YELLOW_FLOWER, "minecraft:dandelion"),
            (GRASS, "minecraft:short_grass"),
            (TERRACOTTA, "minecraft:hardened_clay"),
            (BRICK, "minecraft:brick_block"),
            (CHAIN_Y, "minecraft:iron_chain"),
        ] {
            assert_eq!(to_bedrock_block_with_properties(block, None).name, name);
        }
    }

    #[test]
    fn walls_keep_their_connections() {
        let bedrock = convert_with(
            BRICK_WALL,
            &[
                ("east", "low"),
                ("west", "low"),
                ("north", "none"),
                ("south", "none"),
                ("up", "false"),
            ],
        );
        assert_eq!(bedrock.name, "minecraft:brick_wall");
        assert_eq!(
            state(&bedrock, "wall_connection_type_east"),
            Some(s("short"))
        );
        assert_eq!(
            state(&bedrock, "wall_connection_type_north"),
            Some(s("none"))
        );
        assert_eq!(state(&bedrock, "wall_post_bit"), Some(Bool(false)));
    }

    #[test]
    fn fence_gates_carry_facing_and_in_wall() {
        let bedrock = convert_with(OAK_FENCE_GATE, &[("facing", "east"), ("in_wall", "true")]);
        assert_eq!(bedrock.name, "minecraft:fence_gate");
        assert_eq!(
            state(&bedrock, "minecraft:cardinal_direction"),
            Some(s("east"))
        );
        assert_eq!(state(&bedrock, "in_wall_bit"), Some(Bool(true)));
        assert_eq!(state(&bedrock, "open_bit"), Some(Bool(false)));
    }

    #[test]
    fn stairs_keep_facing_and_half() {
        let bedrock = convert_with(OAK_STAIRS, &[("facing", "north"), ("half", "top")]);
        assert_eq!(state(&bedrock, "weirdo_direction"), Some(Int(3)));
        assert_eq!(state(&bedrock, "upside_down_bit"), Some(Bool(true)));
        // Bedrock never renamed its two oldest stair ids, so they read as swapped.
        assert_eq!(
            to_bedrock_block_with_properties(COBBLESTONE_STAIRS, None).name,
            "minecraft:stone_stairs"
        );
        assert_eq!(
            to_bedrock_block_with_properties(STONE_STAIRS, None).name,
            "minecraft:normal_stone_stairs"
        );
    }

    #[test]
    fn slabs_keep_their_half_and_double_slabs_their_own_id() {
        let top = convert_with(ANDESITE_SLAB, &[("type", "top")]);
        assert_eq!(top.name, "minecraft:andesite_slab");
        assert_eq!(state(&top, "minecraft:vertical_half"), Some(s("top")));
        for (block, double) in [
            (ANDESITE_SLAB, "minecraft:andesite_double_slab"),
            (STONE_BLOCK_SLAB, "minecraft:normal_stone_double_slab"),
            (OAK_SLAB, "minecraft:oak_double_slab"),
            (
                WAXED_EXPOSED_CUT_COPPER_SLAB,
                "minecraft:waxed_exposed_double_cut_copper_slab",
            ),
        ] {
            assert_eq!(convert_with(block, &[("type", "double")]).name, double);
        }
    }

    /// Doors store the facing turned a quarter; beds use the plain legacy order.
    #[test]
    fn door_and_bed_directions_differ() {
        let door = convert_with(OAK_DOOR, &[("facing", "north"), ("half", "upper")]);
        assert_eq!(door.name, "minecraft:wooden_door");
        assert_eq!(
            state(&door, "minecraft:cardinal_direction"),
            Some(s("east"))
        );
        assert_eq!(state(&door, "upper_block_bit"), Some(Bool(true)));
        let bed = convert_with(RED_BED_NORTH_HEAD, &[("facing", "north"), ("part", "head")]);
        assert_eq!(bed.name, "minecraft:bed");
        assert_eq!(state(&bed, "direction"), Some(Int(2)));
        assert_eq!(state(&bed, "head_piece_bit"), Some(Bool(true)));
    }

    #[test]
    fn wall_torch_facing_is_inverted() {
        let torch = convert_with(REDSTONE_WALL_TORCH, &[("facing", "north")]);
        assert_eq!(torch.name, "minecraft:redstone_torch");
        assert_eq!(state(&torch, "torch_facing_direction"), Some(s("south")));
    }

    #[test]
    fn rails_crops_and_plants_keep_their_states() {
        let rail = convert_with(RAIL, &[("shape", "north_west")]);
        assert_eq!(state(&rail, "rail_direction"), Some(Int(8)));
        let wheat = convert_with(WHEAT, &[("age", "7")]);
        assert_eq!(state(&wheat, "growth"), Some(Int(7)));
        let lilac = convert_with(LILAC_UPPER, &[("half", "upper")]);
        assert_eq!(lilac.name, "minecraft:lilac");
        assert_eq!(state(&lilac, "upper_block_bit"), Some(Bool(true)));
        let pickle = convert_with(SEA_PICKLE, &[("pickles", "3")]);
        assert_eq!(state(&pickle, "cluster_count"), Some(Int(2)));
        assert_eq!(state(&pickle, "dead_bit"), Some(Bool(false)));
        let sign = convert_with(SIGN, &[("rotation", "11")]);
        assert_eq!(state(&sign, "ground_sign_direction"), Some(Int(11)));
    }

    #[test]
    fn plants_of_the_sea_stand_in_water() {
        assert!(is_waterlogged(KELP, None));
        assert!(is_waterlogged(SEAGRASS, None));
        assert!(is_waterlogged(
            SEA_PICKLE,
            Some(&props(&[("waterlogged", "true")]))
        ));
        assert!(!is_waterlogged(
            SEA_PICKLE,
            Some(&props(&[("waterlogged", "false")]))
        ));
        assert!(is_waterlogged(
            OAK_STAIRS,
            Some(&props(&[("waterlogged", "true")]))
        ));
        assert!(!is_waterlogged(OAK_STAIRS, None));
        assert!(!is_waterlogged(STONE, None));
    }

    #[test]
    fn potted_plants_name_their_plant() {
        // FLOWER_POT is a pot with a poppy in it.
        assert_eq!(potted_plant(FLOWER_POT).unwrap().name, "minecraft:poppy");
        assert_eq!(
            potted_plant(POTTED_RED_TULIP).unwrap().name,
            "minecraft:red_tulip"
        );
        assert!(potted_plant(STONE).is_none());
    }

    #[test]
    fn lit_and_flowing_variants_are_their_own_blocks() {
        assert_eq!(
            convert_with(REDSTONE_LAMP, &[("lit", "true")]).name,
            "minecraft:lit_redstone_lamp"
        );
        let flowing = convert_with(WATER, &[("level", "3")]);
        assert_eq!(flowing.name, "minecraft:flowing_water");
        assert_eq!(state(&flowing, "liquid_depth"), Some(Int(3)));
        assert_eq!(
            to_bedrock_block_with_properties(WATER, None).name,
            "minecraft:water"
        );
    }
}
