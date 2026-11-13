//! Arnis blocks to Hytale block types, named as uNmINeD's list has them, picked by map color.

use crate::block_definitions::Block;
use fastnbt::Value;
use once_cell::sync::Lazy;

pub const EMPTY: &str = "Empty";
pub const WATER: &str = "Water_Source";
const LAVA: &str = "Lava_Source";

/// One Hytale cell: block type, rotation index and the fluid sharing the cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HytaleCell {
    pub block: &'static str,
    pub rotation: u8,
    pub fluid: Option<&'static str>,
}

impl HytaleCell {
    const fn of(block: &'static str) -> Self {
        Self {
            block,
            rotation: 0,
            fluid: None,
        }
    }

    const fn turned(block: &'static str, rotation: u8) -> Self {
        Self {
            block,
            rotation,
            fluid: None,
        }
    }

    const fn fluid(fluid: &'static str) -> Self {
        Self {
            block: EMPTY,
            rotation: 0,
            fluid: Some(fluid),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.block == EMPTY
    }
}

/// The shapes one Hytale material comes in; a missing shape borrows a neighbour's.
struct Family {
    full: &'static str,
    stairs: &'static str,
    half: &'static str,
    wall: &'static str,
}

macro_rules! rock {
    ($full:literal, $stairs:literal, $half:literal, $wall:literal) => {
        Family {
            full: $full,
            stairs: $stairs,
            half: $half,
            wall: $wall,
        }
    };
}

/// Plank families; a wooden "wall" is its fence.
macro_rules! wood {
    ($w:literal) => {
        Family {
            full: concat!("Wood_", $w, "_Planks"),
            stairs: concat!("Wood_", $w, "_Stairs"),
            half: concat!("Wood_", $w, "_Planks_Half"),
            wall: concat!("Wood_", $w, "_Fence"),
        }
    };
}

fn wood_kind(stem: &str) -> Option<&'static str> {
    Some(match stem {
        "oak" => "Drywood",
        "spruce" => "Softwood",
        "birch" | "cherry" | "pale_oak" => "Lightwood",
        "dark_oak" => "Darkwood",
        "jungle" => "Goldenwood",
        "acacia" | "mangrove" | "crimson" => "Redwood",
        "bamboo" => "Tropicalwood",
        "warped" => "Greenwood",
        _ => return None,
    })
}

fn wood_family(kind: &str) -> Family {
    match kind {
        "Drywood" => wood!("Drywood"),
        "Softwood" => wood!("Softwood"),
        "Lightwood" => wood!("Lightwood"),
        "Darkwood" => wood!("Darkwood"),
        "Goldenwood" => wood!("Goldenwood"),
        "Redwood" => wood!("Redwood"),
        "Tropicalwood" => wood!("Tropicalwood"),
        _ => wood!("Greenwood"),
    }
}

/// The family behind a Minecraft material stem ("stone_brick", "oak", "polished_granite").
fn family(stem: &str) -> Option<Family> {
    if let Some(kind) = wood_kind(stem) {
        return Some(wood_family(kind));
    }
    Some(match stem {
        "stone" => rock!(
            "Rock_Stone",
            "Rock_Stone_Stairs",
            "Rock_Stone_Half",
            "Rock_Stone_Cobble_Wall"
        ),
        "smooth_stone" => rock!(
            "Rock_Stone_Brick_Smooth",
            "Rock_Stone_Brick_Stairs",
            "Rock_Stone_Brick_Smooth_Half",
            "Rock_Stone_Brick_Wall"
        ),
        "cobblestone" => rock!(
            "Rock_Stone_Cobble",
            "Rock_Stone_Cobble_Stairs",
            "Rock_Stone_Cobble_Half",
            "Rock_Stone_Cobble_Wall"
        ),
        "mossy_cobblestone" => rock!(
            "Rock_Stone_Cobble_Mossy",
            "Rock_Stone_Cobble_Stairs",
            "Rock_Stone_Cobble_Mossy_Half",
            "Rock_Stone_Cobble_Wall"
        ),
        "stone_brick" | "cracked_stone_brick" => rock!(
            "Rock_Stone_Brick",
            "Rock_Stone_Brick_Stairs",
            "Rock_Stone_Brick_Half",
            "Rock_Stone_Brick_Wall"
        ),
        "mossy_stone_brick" => rock!(
            "Rock_Stone_Brick_Mossy",
            "Rock_Stone_Brick_Stairs",
            "Rock_Stone_Brick_Half",
            "Rock_Stone_Brick_Wall"
        ),
        "chiseled_stone_brick" => rock!(
            "Rock_Stone_Brick_Ornate",
            "Rock_Stone_Brick_Stairs",
            "Rock_Stone_Brick_Half",
            "Rock_Stone_Brick_Wall"
        ),
        "andesite" => rock!(
            "Rock_Calcite",
            "Rock_Calcite_Stairs",
            "Rock_Calcite_Half",
            "Rock_Calcite_Cobble_Wall"
        ),
        "polished_andesite" => rock!(
            "Rock_Calcite_Brick_Smooth",
            "Rock_Calcite_Brick_Stairs",
            "Rock_Calcite_Brick_Smooth_Half",
            "Rock_Calcite_Brick_Wall"
        ),
        "diorite" => rock!(
            "Rock_Marble",
            "Rock_Marble_Stairs",
            "Rock_Marble_Half",
            "Rock_Marble_Cobble_Wall"
        ),
        "polished_diorite" | "quartz" | "smooth_quartz" | "quartz_pillar" | "chiseled_quartz" => {
            rock!(
                "Rock_Marble_Brick_Smooth",
                "Rock_Marble_Brick_Stairs",
                "Rock_Marble_Brick_Smooth_Half",
                "Rock_Marble_Brick_Wall"
            )
        }
        "quartz_brick" => rock!(
            "Rock_Marble_Brick",
            "Rock_Marble_Brick_Stairs",
            "Rock_Marble_Brick_Half",
            "Rock_Marble_Brick_Wall"
        ),
        "granite" => rock!(
            "Rock_Sandstone_Red",
            "Rock_Sandstone_Red_Stairs",
            "Rock_Sandstone_Red_Half",
            "Rock_Sandstone_Red_Cobble_Wall"
        ),
        "polished_granite" => rock!(
            "Rock_Sandstone_Red_Brick_Smooth",
            "Rock_Sandstone_Red_Brick_Stairs",
            "Rock_Sandstone_Red_Brick_Smooth_Half",
            "Rock_Sandstone_Red_Brick_Wall"
        ),
        "deepslate" => rock!(
            "Rock_Shale",
            "Rock_Shale_Stairs",
            "Rock_Shale_Half",
            "Rock_Shale_Cobble_Wall"
        ),
        "cobbled_deepslate" => rock!(
            "Rock_Shale_Cobble",
            "Rock_Shale_Cobble_Stairs",
            "Rock_Shale_Cobble_Half",
            "Rock_Shale_Cobble_Wall"
        ),
        "polished_deepslate" | "chiseled_deepslate" => rock!(
            "Rock_Shale_Brick_Smooth",
            "Rock_Shale_Brick_Stairs",
            "Rock_Shale_Brick_Smooth_Half",
            "Rock_Shale_Brick_Wall"
        ),
        "deepslate_brick" | "cracked_deepslate_brick" => rock!(
            "Rock_Shale_Brick",
            "Rock_Shale_Brick_Stairs",
            "Rock_Shale_Brick_Half",
            "Rock_Shale_Brick_Wall"
        ),
        "deepslate_tile" | "cracked_deepslate_tile" => rock!(
            "Rock_Slate_Brick",
            "Rock_Slate_Brick_Stairs",
            "Rock_Slate_Brick_Half",
            "Rock_Slate_Brick_Wall"
        ),
        "blackstone" => rock!(
            "Rock_Volcanic",
            "Rock_Volcanic_Stairs",
            "Rock_Volcanic_Half",
            "Rock_Volcanic_Cobble_Wall"
        ),
        "polished_blackstone" | "chiseled_polished_blackstone" => rock!(
            "Rock_Volcanic_Brick_Smooth",
            "Rock_Volcanic_Brick_Stairs",
            "Rock_Volcanic_Brick_Smooth_Half",
            "Rock_Volcanic_Brick_Wall"
        ),
        "polished_blackstone_brick" | "cracked_polished_blackstone_brick" => rock!(
            "Rock_Volcanic_Brick",
            "Rock_Volcanic_Brick_Stairs",
            "Rock_Volcanic_Brick_Half",
            "Rock_Volcanic_Brick_Wall"
        ),
        "basalt" | "tuff" => rock!(
            "Rock_Basalt",
            "Rock_Basalt_Stairs",
            "Rock_Basalt_Half",
            "Rock_Basalt_Cobble_Wall"
        ),
        "polished_basalt" | "smooth_basalt" => rock!(
            "Rock_Basalt_Brick_Smooth",
            "Rock_Basalt_Brick_Stairs",
            "Rock_Basalt_Brick_Smooth_Half",
            "Rock_Basalt_Brick_Wall"
        ),
        "calcite" => rock!(
            "Rock_Chalk",
            "Rock_Chalk_Stairs",
            "Rock_Chalk_Half",
            "Rock_Chalk_Cobble_Wall"
        ),
        "sandstone" => rock!(
            "Rock_Sandstone_White",
            "Rock_Sandstone_White_Stairs",
            "Rock_Sandstone_White_Half",
            "Rock_Sandstone_White_Brick_Wall"
        ),
        "cut_sandstone" | "chiseled_sandstone" => rock!(
            "Rock_Sandstone_White_Brick",
            "Rock_Sandstone_White_Brick_Stairs",
            "Rock_Sandstone_White_Brick_Half",
            "Rock_Sandstone_White_Brick_Wall"
        ),
        "smooth_sandstone" => rock!(
            "Rock_Sandstone_White_Brick_Smooth",
            "Rock_Sandstone_White_Brick_Stairs",
            "Rock_Sandstone_White_Brick_Smooth_Half",
            "Rock_Sandstone_White_Brick_Wall"
        ),
        "red_sandstone" => rock!(
            "Rock_Sandstone",
            "Rock_Sandstone_Stairs",
            "Rock_Sandstone_Half",
            "Rock_Sandstone_Brick_Wall"
        ),
        "smooth_red_sandstone" => rock!(
            "Rock_Sandstone_Brick_Smooth",
            "Rock_Sandstone_Brick_Stairs",
            "Rock_Sandstone_Brick_Smooth_Half",
            "Rock_Sandstone_Brick_Wall"
        ),
        "brick" => rock!(
            "Soil_Clay_Brick",
            "Soil_Clay_Brick_Stairs",
            "Soil_Clay_Brick_Half",
            "Soil_Clay_Brick_Wall"
        ),
        "mud_brick" => rock!(
            "Soil_Clay_Raw_Brick",
            "Soil_Clay_Raw_Brick_Stairs",
            "Soil_Clay_Raw_Brick_Half",
            "Soil_Clay_Brick_Wall"
        ),
        "nether_brick" | "cracked_nether_brick" => rock!(
            "Rock_Magma_Cooled_Brick",
            "Rock_Magma_Cooled_Brick_Stairs",
            "Rock_Magma_Cooled_Brick_Half",
            "Rock_Magma_Cooled_Brick_Wall"
        ),
        "red_nether_brick" => rock!(
            "Soil_Clay_Smooth_Red",
            "Soil_Clay_Smooth_Red_Stairs",
            "Soil_Clay_Smooth_Red_Half",
            "Soil_Clay_Brick_Wall"
        ),
        "end_stone_brick" => rock!(
            "Rock_Lime_Brick",
            "Rock_Lime_Brick_Stairs",
            "Rock_Lime_Brick_Half",
            "Rock_Lime_Brick_Wall"
        ),
        "purpur" => rock!(
            "Soil_Clay_Smooth_Purple",
            "Soil_Clay_Smooth_Purple_Stairs",
            "Soil_Clay_Smooth_Purple_Half",
            "Soil_Clay_Smooth_Purple"
        ),
        "prismarine" => rock!(
            "Soil_Clay_Ocean_Brick",
            "Soil_Clay_Ocean_Brick_Stairs",
            "Soil_Clay_Ocean_Brick_Half",
            "Soil_Clay_Ocean_Brick_Wall"
        ),
        "dark_prismarine" => rock!(
            "Rock_Aqua_Brick",
            "Rock_Aqua_Brick_Stairs",
            "Rock_Aqua_Brick_Half",
            "Rock_Aqua_Brick_Wall"
        ),
        "waxed_cut_copper" | "waxed_exposed_cut_copper" | "waxed_exposed_chiseled_copper" => {
            rock!(
                "Metal_Copper",
                "Metal_Copper_Stairs",
                "Metal_Copper_Half",
                "Metal_Copper_Fence"
            )
        }
        // Verdigris reads as Hytale's teal zinc.
        "waxed_oxidized_cut_copper" => rock!(
            "Metal_Zinc",
            "Metal_Zinc_Stairs",
            "Metal_Zinc_Half",
            "Metal_Zinc_Fence"
        ),
        _ => return None,
    })
}

/// Minecraft dye colors as Hytale smooth clay (concrete), clay (terracotta) and wool.
fn dyed(color: &str, kind: &str) -> Option<&'static str> {
    // Hytale's grey clays lean olive; the asphalt and paving greys take neutral stone.
    match (color, kind) {
        ("gray", "concrete") => return Some("Rock_Slate_Brick_Smooth"),
        ("gray", "concrete_powder") | ("cyan", "terracotta") => {
            return Some("Rock_Ledge_Brick_Smooth")
        }
        _ => {}
    }
    Some(match kind {
        "concrete" | "concrete_powder" => match color {
            "white" => "Soil_Clay_Smooth_White",
            "light_gray" => "Soil_Clay_Smooth_Grey2",
            "gray" => "Soil_Clay_Smooth_Grey",
            "black" => "Soil_Clay_Smooth_Black",
            "red" => "Soil_Clay_Smooth_Red2",
            "brown" => "Soil_Clay_Smooth_Red",
            "orange" => "Soil_Clay_Smooth_Orange",
            "yellow" => "Soil_Clay_Smooth_Yellow2",
            "lime" => "Soil_Clay_Smooth_Lime",
            "green" => "Soil_Clay_Smooth_GreenDark",
            "cyan" | "light_blue" => "Soil_Clay_Smooth_Cyan",
            "blue" => "Soil_Clay_Smooth_Blue",
            "purple" => "Soil_Clay_Smooth_Purple",
            "magenta" | "pink" => "Soil_Clay_Smooth_Pink",
            _ => return None,
        },
        "terracotta" => match color {
            "white" => "Soil_Clay_White",
            "light_gray" => "Soil_Clay_GreyLight",
            "gray" => "Soil_Clay_Grey",
            "black" => "Soil_Clay_Black",
            "red" | "brown" => "Soil_Clay_Red",
            "orange" => "Soil_Clay_Orange",
            "yellow" => "Soil_Clay_Yellow",
            "lime" => "Soil_Clay_Lime",
            "green" => "Soil_Clay_Green",
            "cyan" => "Soil_Clay_Cyan",
            "light_blue" => "Soil_Clay_Ocean",
            "blue" => "Soil_Clay_Blue",
            "purple" | "magenta" => "Soil_Clay_Purple",
            "pink" => "Soil_Clay_Pink",
            _ => return None,
        },
        "wool" => match color {
            "white" => "Cloth_Block_Wool_White",
            "light_gray" => "Cloth_Block_Wool_Gray_Light",
            "gray" => "Cloth_Block_Wool_Gray",
            "black" => "Cloth_Block_Wool_Black",
            "red" => "Cloth_Block_Wool_Red",
            "brown" | "orange" => "Cloth_Block_Wool_Orange",
            "yellow" => "Cloth_Block_Wool_Yellow",
            "lime" => "Cloth_Block_Wool_Green_Light",
            "green" => "Cloth_Block_Wool_Green",
            "cyan" => "Cloth_Block_Wool_Cyan",
            "light_blue" => "Cloth_Block_Wool_Blue_Light",
            "blue" => "Cloth_Block_Wool_Blue",
            "purple" => "Cloth_Block_Wool_Purple",
            "magenta" => "Cloth_Block_Wool_Purple_Light",
            "pink" => "Cloth_Block_Wool_Pink",
            _ => return None,
        },
        _ => return None,
    })
}

const COLORS: [&str; 16] = [
    "white",
    "light_gray",
    "gray",
    "black",
    "red",
    "brown",
    "orange",
    "yellow",
    "lime",
    "green",
    "cyan",
    "light_blue",
    "blue",
    "purple",
    "magenta",
    "pink",
];

/// "light_blue_concrete" -> Some(("light_blue", "concrete")).
fn split_color(name: &str) -> Option<(&'static str, &str)> {
    COLORS.iter().find_map(|&c| {
        name.strip_prefix(c)
            .and_then(|rest| rest.strip_prefix('_'))
            .map(|rest| (c, rest))
    })
}

fn prop<'a>(props: Option<&'a Value>, key: &str) -> Option<&'a str> {
    match props? {
        Value::Compound(map) => match map.get(key)? {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// Hytale yaw (rotation index `yaw + 4 * pitch + 16 * roll`): 0 points north, 1 west.
fn yaw(props: Option<&Value>) -> u8 {
    match prop(props, "facing") {
        Some("west") => 1,
        Some("south") => 2,
        Some("east") => 3,
        _ => 0,
    }
}

/// A vine faces away from the first wall it hangs on.
fn vine_yaw(props: Option<&Value>) -> u8 {
    [("south", 0), ("east", 1), ("north", 2), ("west", 3)]
        .into_iter()
        .find(|(side, _)| prop(props, side) == Some("true"))
        .map_or(0, |(_, yaw)| yaw)
}

/// Upside-down blocks flip by pitch, which runs before yaw and so turns the facing round.
fn stairs_rotation(props: Option<&Value>) -> u8 {
    let yaw = yaw(props);
    if prop(props, "half") == Some("top") {
        8 + (yaw + 2) % 4
    } else {
        yaw
    }
}

/// Upper slabs: pitch 180, the `UpDown` variant's other state.
const UPPER_HALF: u8 = 8;

/// The material stem of a full block's name ("stone_bricks" -> "stone_brick").
fn full_stem(name: &str) -> &str {
    if let Some(stem) = name.strip_suffix("_planks") {
        return stem;
    }
    if let Some(stem) = name.strip_suffix("_block") {
        return stem;
    }
    if name == "bricks" || name.ends_with("_bricks") || name.ends_with("_tiles") {
        return &name[..name.len() - 1];
    }
    name
}

/// Maps an Arnis block to a Hytale cell. `props` are the block's Minecraft states.
pub fn to_hytale(block: Block, props: Option<&Value>) -> HytaleCell {
    match (props, TABLE.get(block.id() as usize)) {
        (None, Some(&cell)) => cell,
        _ => with_water(block.try_name().unwrap_or("stone"), props),
    }
}

/// `map_block`, with water in a waterlogged cell.
fn with_water(name: &str, props: Option<&Value>) -> HytaleCell {
    let mut cell = map_block(name, props);
    if cell.fluid.is_none() && prop(props, "waterlogged") == Some("true") {
        cell.fluid = Some(WATER);
    }
    cell
}

/// Every block id's cell with its default states, so plain cells skip the name parsing.
static TABLE: Lazy<Vec<HytaleCell>> = Lazy::new(|| {
    // Block ids stay well under 1024; an unassigned one maps to stone.
    (0..1024u16)
        .map(|id| {
            let block = Block::from_raw_id(id);
            let Some(name) = block.try_name() else {
                return STONE;
            };
            with_water(name, block.properties().as_ref())
        })
        .collect()
});

const STONE: HytaleCell = HytaleCell::of("Rock_Stone");

fn map_block(name: &str, props: Option<&Value>) -> HytaleCell {
    mapped(name, props).unwrap_or(STONE)
}

/// None for a name the table does not know; the tests hold every Arnis block to Some.
fn mapped(name: &str, props: Option<&Value>) -> Option<HytaleCell> {
    if let Some(stem) = name.strip_suffix("_stairs") {
        return family(stem).map(|f| HytaleCell::turned(f.stairs, stairs_rotation(props)));
    }
    if let Some(stem) = name.strip_suffix("_slab") {
        let f = family(stem)?;
        return Some(match prop(props, "type") {
            Some("double") => HytaleCell::of(f.full),
            Some("top") => HytaleCell::turned(f.half, UPPER_HALF),
            _ => HytaleCell::of(f.half),
        });
    }
    if let Some(stem) = name.strip_suffix("_fence_gate") {
        return wood_kind(stem).map(|k| match k {
            "Drywood" => HytaleCell::of("Wood_Drywood_Fence_Gate"),
            "Softwood" => HytaleCell::of("Wood_Softwood_Fence_Gate"),
            "Lightwood" => HytaleCell::of("Wood_Lightwood_Fence_Gate"),
            "Darkwood" => HytaleCell::of("Wood_Darkwood_Fence_Gate"),
            "Goldenwood" => HytaleCell::of("Wood_Goldenwood_Fence_Gate"),
            "Redwood" => HytaleCell::of("Wood_Redwood_Fence_Gate"),
            "Tropicalwood" => HytaleCell::of("Wood_Tropicalwood_Fence_Gate"),
            _ => HytaleCell::of("Wood_Greenwood_Fence_Gate"),
        });
    }
    if let Some(stem) = name
        .strip_suffix("_fence")
        .or_else(|| name.strip_suffix("_wall"))
    {
        return family(stem).map(|f| HytaleCell::of(f.wall));
    }
    // Names like red_sand only look dyed and fall through.
    if let Some((color, kind)) = split_color(name) {
        if let Some(b) = dyed(color, kind) {
            return Some(HytaleCell::of(b));
        }
        if matches!(kind, "stained_glass" | "stained_glass_pane") {
            return Some(HytaleCell::of(WINDOW));
        }
    }
    if let Some(cell) = special(name, props) {
        return Some(cell);
    }
    family(full_stem(name)).map(|f| HytaleCell::of(f.full))
}

/// Hytale has no plain glass; its window pane block stands in for every glass.
const WINDOW: &str = "Prototype_Window_Middle";

/// Everything that is not a plain material in a shape.
fn special(name: &str, props: Option<&Value>) -> Option<HytaleCell> {
    let water = |block: &'static str| HytaleCell {
        block,
        rotation: 0,
        fluid: Some(WATER),
    };
    Some(match name {
        "air" | "cave_air" | "void_air" => HytaleCell::of(EMPTY),
        "water" => HytaleCell::fluid(WATER),
        "lava" => HytaleCell::fluid(LAVA),

        // Ground and nature.
        "grass_block" => HytaleCell::of("Soil_Grass"),
        "dirt" => HytaleCell::of("Soil_Dirt"),
        "coarse_dirt" => HytaleCell::of("Soil_Dirt_Dry"),
        "podzol" => HytaleCell::of("Soil_Needles"),
        "mycelium" => HytaleCell::of("Soil_Dirt_Burnt"),
        "dirt_path" => HytaleCell::of("Soil_Pathway"),
        "farmland" => HytaleCell::of("Soil_Dirt_Tilled"),
        "mud" => HytaleCell::of("Soil_Mud"),
        "clay" => HytaleCell::of("Soil_Clay"),
        "terracotta" => HytaleCell::of("Soil_Clay"),
        "gravel" => HytaleCell::of("Soil_Gravel"),
        "sand" => HytaleCell::of("Soil_Sand"),
        "red_sand" => HytaleCell::of("Soil_Sand_Red"),
        "soul_sand" => HytaleCell::of("Soil_Mud_Dry"),
        "snow_block" | "powder_snow" => HytaleCell::of("Soil_Snow"),
        "snow" => HytaleCell::of("Soil_Snow_Half"),
        "ice" | "packed_ice" | "blue_ice" => HytaleCell::of("Rock_Ice"),
        "moss_block" => HytaleCell::of("Plant_Moss_Block_Green"),
        "moss_carpet" => HytaleCell::of("Plant_Moss_Rug_Green"),
        "bedrock" => HytaleCell::of("Rock_Bedrock"),
        "obsidian" | "coal_block" => HytaleCell::of("Rock_Magma_Cooled"),
        "magma_block" => HytaleCell::of("Rock_Volcanic_Cracked_Lava"),
        "netherrack" | "nether_wart_block" => HytaleCell::of("Soil_Clay_Smooth_Red"),
        "dripstone_block" => HytaleCell::of("Rock_Sandstone_Cobble"),
        "pointed_dripstone" => HytaleCell::of("Rock_Stone_Stalactite_Small"),
        "sculk" | "sculk_catalyst" | "sculk_sensor" | "sculk_shrieker" => {
            HytaleCell::of("Rock_Shale")
        }
        "sponge" => HytaleCell::of("Soil_Clay_Yellow"),

        // Ores.
        "coal_ore" => HytaleCell::of("Rock_Stone"),
        "iron_ore" => HytaleCell::of("Ore_Iron_Stone"),
        "gold_ore" => HytaleCell::of("Ore_Gold_Stone"),
        "copper_ore" => HytaleCell::of("Ore_Copper_Stone"),
        "diamond_ore" | "redstone_ore" | "lapis_ore" => HytaleCell::of("Ore_Silver_Stone"),
        "deepslate_coal_ore" => HytaleCell::of("Rock_Shale"),
        "deepslate_iron_ore" => HytaleCell::of("Ore_Iron_Shale"),
        "deepslate_gold_ore" => HytaleCell::of("Ore_Gold_Shale"),
        "deepslate_copper_ore" => HytaleCell::of("Ore_Copper_Shale"),
        "deepslate_diamond_ore" | "deepslate_lapis_ore" => HytaleCell::of("Ore_Cobalt_Shale"),
        "deepslate_redstone_ore" => HytaleCell::of("Ore_Silver_Shale"),

        // Metals and precious blocks.
        "iron_block" | "lodestone" => HytaleCell::of("Metal_Iron"),
        "gold_block" => HytaleCell::of("Rock_Gold_Brick_Smooth"),
        "raw_gold_block" => HytaleCell::of("Rock_Gold_Brick"),
        "netherite_block" => HytaleCell::of("Rock_Volcanic_Brick_Smooth"),
        "lapis_block" => HytaleCell::of("Soil_Clay_Smooth_Blue"),
        "redstone_block" => HytaleCell::of("Soil_Clay_Smooth_Red2"),
        "waxed_copper_block" | "waxed_exposed_copper" | "waxed_exposed_cut_copper" => {
            HytaleCell::of("Metal_Copper")
        }
        "waxed_oxidized_copper" | "waxed_oxidized_cut_copper" => HytaleCell::of("Metal_Zinc"),
        "amethyst_block" | "budding_amethyst" => HytaleCell::of("Rock_Crystal_Purple_Block"),
        "amethyst_cluster" | "large_amethyst_bud" => HytaleCell::of("Rock_Crystal_Purple_Large"),
        "medium_amethyst_bud" => HytaleCell::of("Rock_Crystal_Purple_Medium"),
        "small_amethyst_bud" => HytaleCell::of("Rock_Crystal_Purple_Small"),

        // Glass, light and metalwork.
        "glass" | "tinted_glass" | "glass_pane" => HytaleCell::of(WINDOW),
        "iron_bars" => HytaleCell::of("Deco_Iron_Bars"),
        "chain" => HytaleCell::of("Deco_Iron_Chains_Vertical"),
        "lantern" | "soul_lantern" => HytaleCell::of("Deco_Lantern"),
        // No invisible light: buildings get the ceiling lamps they use on Luanti.
        "light" => HytaleCell::of(EMPTY),
        "glowstone" => HytaleCell::of("Build_Lightsource_YellowLight"),
        "sea_lantern" | "end_rod" => HytaleCell::of("Build_Lightsource_White"),
        "redstone_lamp" => HytaleCell::of("Build_Lightsource_Yellow"),
        "shroomlight" => HytaleCell::of("Build_Lightsource_Orange"),
        "fire" => HytaleCell::of("Deco_Fire"),
        "cobweb" => HytaleCell::of("Deco_SpiderWeb"),

        // Wood.
        "oak_log" | "stripped_oak_log" => HytaleCell::of("Wood_Oak_Trunk"),
        "birch_log" => HytaleCell::of("Wood_Birch_Trunk"),
        "spruce_log" => HytaleCell::of("Wood_Fir_Trunk"),
        "dark_oak_log" => HytaleCell::of("Wood_Beech_Trunk"),
        "jungle_log" => HytaleCell::of("Wood_Jungle_Trunk"),
        "acacia_log" => HytaleCell::of("Wood_Palo_Trunk"),
        "cherry_log" => HytaleCell::of("Wood_Maple_Trunk"),
        "mangrove_log" => HytaleCell::of("Wood_Banyan_Trunk"),
        "mushroom_stem" => HytaleCell::of("Wood_Birch_Trunk"),
        "stripped_warped_stem" | "stripped_warped_hyphae" => HytaleCell::of("Wood_Spiral_Trunk"),
        "oak_leaves" | "acacia_leaves" => HytaleCell::of("Plant_Leaves_Oak"),
        "birch_leaves" => HytaleCell::of("Plant_Leaves_Birch"),
        "spruce_leaves" => HytaleCell::of("Plant_Leaves_Fir"),
        "dark_oak_leaves" | "azalea_leaves" => HytaleCell::of("Plant_Leaves_Beech"),
        "jungle_leaves" => HytaleCell::of("Plant_Leaves_Jungle"),
        "cherry_leaves" => HytaleCell::of("Plant_Leaves_Maple"),
        "mangrove_leaves" => HytaleCell::of("Plant_Leaves_Banyan"),
        "flowering_azalea_leaves" => HytaleCell::of("Plant_Leaves_Apple"),
        // Furniture models can span several cells, so shelves and tables stay one block.
        "bookshelf" | "chiseled_bookshelf" => HytaleCell::of("Wood_Hardwood_Planks"),
        "scaffolding" => HytaleCell::of("Wood_Softwood_Fence"),
        "ladder" => HytaleCell::turned("Furniture_Village_Ladder", yaw(props)),
        "chest" => HytaleCell::turned("Furniture_Village_Chest_Small", yaw(props)),
        "barrel" => HytaleCell::of("Furniture_Tavern_Barrel"),
        "composter" | "crafting_table" => HytaleCell::of("Furniture_Village_Crate"),
        "furnace" | "blast_furnace" | "smoker" | "dispenser" | "note_block" => {
            HytaleCell::of("Rock_Stone_Brick_Smooth")
        }

        // Plants. Two-block plants are one Hytale model standing in the lower cell.
        "tall_grass" | "large_fern" | "sunflower" | "lilac" | "rose_bush" | "peony"
            if prop(props, "half") == Some("upper") =>
        {
            HytaleCell::of(EMPTY)
        }
        "tall_grass" => HytaleCell::of("Plant_Grass_Lush_Tall"),
        "large_fern" => HytaleCell::of("Plant_Fern_Tall"),
        "sunflower" => HytaleCell::of("Plant_Flower_Tall_Yellow"),
        "lilac" => HytaleCell::of("Plant_Flower_Tall_Purple"),
        "rose_bush" => HytaleCell::of("Plant_Flower_Tall_Red"),
        "peony" => HytaleCell::of("Plant_Flower_Tall_Pink"),
        "vine" => HytaleCell::turned("Plant_Vine_Wall", vine_yaw(props)),
        "short_grass" => HytaleCell::of("Plant_Grass_Lush_Short"),
        "fern" => HytaleCell::of("Plant_Fern"),
        "dead_bush" => HytaleCell::of("Plant_Bush_Dead"),
        "azalea" => HytaleCell::of("Plant_Bush_Green"),
        "flowering_azalea" => HytaleCell::of("Plant_Bush_Lush"),
        "sweet_berry_bush" => HytaleCell::of("Plant_Crop_Berry_Block"),
        "poppy" => HytaleCell::of("Plant_Flower_Common_Red"),
        "dandelion" => HytaleCell::of("Plant_Flower_Common_Yellow"),
        "blue_orchid" => HytaleCell::of("Plant_Flower_Orchid_Blue"),
        "azure_bluet" | "lily_of_the_valley" => HytaleCell::of("Plant_Flower_Common_White"),
        "oxeye_daisy" | "white_tulip" => HytaleCell::of("Plant_Flower_Common_White2"),
        "cornflower" => HytaleCell::of("Plant_Flower_Common_Blue"),
        "allium" => HytaleCell::of("Plant_Flower_Common_Purple"),
        "red_tulip" => HytaleCell::of("Plant_Flower_Tall_Red"),
        "orange_tulip" => HytaleCell::of("Plant_Flower_Common_Orange"),
        "pink_tulip" => HytaleCell::of("Plant_Flower_Common_Pink"),
        "wheat" => HytaleCell::of("Plant_Crop_Wheat_Block"),
        "carrots" => HytaleCell::of("Plant_Crop_Carrot_Block"),
        "potatoes" => HytaleCell::of("Plant_Crop_Potato_Block"),
        "beetroots" => HytaleCell::of("Plant_Crop_Turnip_Block"),
        "pumpkin" | "melon" => HytaleCell::of("Plant_Crop_Pumpkin_Block"),
        "hay_block" => HytaleCell::of("Plant_Hay_Bundle"),
        "sugar_cane" => HytaleCell::of("Plant_Reeds_Water"),
        "cactus" => HytaleCell::of("Plant_Cactus_2"),
        "lily_pad" => HytaleCell::of("Plant_Flower_Water_Duckweed"),
        "red_mushroom" => HytaleCell::of("Plant_Crop_Mushroom_Cap_Red"),
        "brown_mushroom" => HytaleCell::of("Plant_Crop_Mushroom_Common_Brown"),
        "red_mushroom_block" => HytaleCell::of("Plant_Crop_Mushroom_Block_Red"),
        "brown_mushroom_block" => HytaleCell::of("Plant_Crop_Mushroom_Block_Brown"),
        "cave_vines" | "cave_vines_plant" => HytaleCell::of("Plant_Vine_Hanging"),
        "big_dripleaf" | "big_dripleaf_stem" | "small_dripleaf" => HytaleCell::of("Plant_Fern_Wet"),
        "tall_seagrass" if prop(props, "half") == Some("upper") => water(EMPTY),
        "kelp" | "kelp_plant" | "tall_seagrass" => water("Plant_Seaweed_Grass_Tall"),
        "seagrass" => water("Plant_Seaweed_Grass"),
        "sea_pickle" => HytaleCell::fluid(WATER),
        "tube_coral_block" => HytaleCell::of("Plant_Coral_Block_Blue"),
        "brain_coral_block" => HytaleCell::of("Plant_Coral_Block_Pink"),
        "bubble_coral_block" => HytaleCell::of("Plant_Coral_Block_Purple"),
        "fire_coral_block" => HytaleCell::of("Plant_Coral_Block_Red"),
        "horn_coral_block" => HytaleCell::of("Plant_Coral_Block_Yellow"),
        "dead_tube_coral_block"
        | "dead_brain_coral_block"
        | "dead_bubble_coral_block"
        | "dead_fire_coral_block"
        | "dead_horn_coral_block" => HytaleCell::of("Plant_Coral_Block_Dead"),
        "tube_coral" | "tube_coral_fan" => water("Plant_Coral_Bush_Blue"),
        "brain_coral" | "brain_coral_fan" => water("Plant_Coral_Bush_Pink"),
        "bubble_coral" | "bubble_coral_fan" => water("Plant_Coral_Bush_Purple"),
        "fire_coral" | "fire_coral_fan" => water("Plant_Coral_Bush_Red"),
        "horn_coral" | "horn_coral_fan" => water("Plant_Coral_Bush_Yellow"),

        // Fittings Hytale has no single-block match for: left out.
        "cauldron"
        | "water_cauldron"
        | "anvil"
        | "damaged_anvil"
        | "brewing_stand"
        | "flower_pot"
        | "lectern"
        | "loom"
        | "smithing_table"
        | "grindstone"
        | "hopper"
        | "lever"
        | "rail"
        | "powered_rail"
        | "tripwire_hook"
        | "lightning_rod"
        | "daylight_detector"
        | "cake"
        | "spore_blossom"
        | "glow_lichen"
        | "sculk_vein"
        | "redstone_torch"
        | "redstone_wall_torch"
        | "oak_sign"
        | "spruce_wall_sign" => HytaleCell::of(EMPTY),
        _ if name.starts_with("potted_")
            || name.ends_with("_door")
            || name.ends_with("_trapdoor")
            || name.ends_with("_button")
            || name.ends_with("_pressure_plate")
            || name.ends_with("_bed")
            || name.ends_with("_banner")
            || name.ends_with("_carpet") =>
        {
            HytaleCell::of(EMPTY)
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_definitions::{
        GRAY_CONCRETE, OAK_SLAB, RED_SAND, STONE_BRICK_STAIRS, WHITE_STAINED_GLASS,
    };
    use std::collections::HashMap;

    fn props(pairs: &[(&str, &str)]) -> Value {
        Value::Compound(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
                .collect::<HashMap<_, _>>(),
        )
    }

    fn all_blocks() -> impl Iterator<Item = Block> {
        (0u16..1024)
            .map(Block::from_raw_id)
            .filter(|b| b.try_name().is_some())
    }

    #[test]
    fn every_arnis_block_has_a_hytale_cell() {
        let missing: Vec<String> = all_blocks()
            .map(|b| b.name().to_string())
            .filter(|n| mapped(n, None).is_none())
            .collect();
        assert!(missing.is_empty(), "unmapped: {missing:?}");
    }

    #[test]
    fn the_table_covers_every_block_id() {
        assert!((TABLE.len() as u16..4096).all(|id| Block::from_raw_id(id).try_name().is_none()));
    }

    #[test]
    fn full_stems_strip_the_shape_words() {
        assert_eq!(full_stem("stone_bricks"), "stone_brick");
        assert_eq!(full_stem("deepslate_tiles"), "deepslate_tile");
        assert_eq!(full_stem("bricks"), "brick");
        assert_eq!(full_stem("oak_planks"), "oak");
        assert_eq!(full_stem("quartz_block"), "quartz");
        assert_eq!(full_stem("andesite"), "andesite");
    }

    #[test]
    fn stairs_face_and_flip() {
        let north = to_hytale(STONE_BRICK_STAIRS, Some(&props(&[("facing", "north")])));
        assert_eq!(
            (north.block, north.rotation),
            ("Rock_Stone_Brick_Stairs", 0)
        );
        let east = to_hytale(STONE_BRICK_STAIRS, Some(&props(&[("facing", "east")])));
        assert_eq!(east.rotation, 3);
        let upside = to_hytale(
            STONE_BRICK_STAIRS,
            Some(&props(&[("facing", "north"), ("half", "top")])),
        );
        assert_eq!(upside.rotation, 8 + 2);
    }

    #[test]
    fn slabs_and_water() {
        let top = to_hytale(OAK_SLAB, Some(&props(&[("type", "top")])));
        assert_eq!((top.block, top.rotation), ("Wood_Drywood_Planks_Half", 8));
        let double = to_hytale(OAK_SLAB, Some(&props(&[("type", "double")])));
        assert_eq!(double.block, "Wood_Drywood_Planks");
        let water = to_hytale(crate::block_definitions::WATER, None);
        assert_eq!(water.fluid, Some(WATER));
        assert!(water.is_empty());
        let wet = to_hytale(OAK_SLAB, Some(&props(&[("waterlogged", "true")])));
        assert_eq!(wet.fluid, Some(WATER));
        let seagrass = to_hytale(crate::block_definitions::TALL_SEAGRASS_BOTTOM, None);
        assert_eq!(seagrass.block, "Plant_Seaweed_Grass_Tall");
        let upper = to_hytale(crate::block_definitions::TALL_SEAGRASS_TOP, None);
        assert!(upper.is_empty() && upper.fluid == Some(WATER));
    }

    #[test]
    fn colors_split_off_the_kind() {
        assert_eq!(
            split_color("light_blue_concrete"),
            Some(("light_blue", "concrete"))
        );
        assert_eq!(
            to_hytale(GRAY_CONCRETE, None).block,
            "Rock_Slate_Brick_Smooth"
        );
        assert_eq!(to_hytale(WHITE_STAINED_GLASS, None).block, WINDOW);
        assert_eq!(to_hytale(RED_SAND, None).block, "Soil_Sand_Red");
    }

    /// Every producible name: `cargo test hytale_names -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn hytale_names() {
        let mut names = std::collections::BTreeSet::new();
        let variants = [
            props(&[]),
            props(&[("type", "top")]),
            props(&[("type", "double")]),
            props(&[("half", "top")]),
        ];
        for b in all_blocks() {
            names.insert(to_hytale(b, None).block);
            for v in &variants {
                names.insert(to_hytale(b, Some(v)).block);
            }
        }
        for n in names {
            println!("HYTALE_NAME {n}");
        }
    }
}
