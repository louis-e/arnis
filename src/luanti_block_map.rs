use crate::block_definitions::Block;
use fastnbt::Value;

/* * This file contains data converted from MC2MT.
 * Original C++ Source Copyright (C) 2016 rollerozxa
 * * Converted to Rust and modified by 3rd3 in 2026.
 * * This file is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation; either
 * version 2.1 of the License, or (at your option) any later version.
 */

/// Supported Luanti game packs
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LuantiGame {
    /// The default minetest_game (ships with Luanti)
    MineTestGame,
    /// Mineclonia — Minecraft-like game for Luanti
    Mineclonia,
}

impl LuantiGame {
    #[allow(dead_code)]
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "minetest_game" | "minetest" => Ok(Self::MineTestGame),
            "mineclonia" => Ok(Self::Mineclonia),
            _ => Err(format!(
                "Unknown Luanti game: '{}'. Supported: minetest_game, mineclonia",
                s
            )),
        }
    }

    /// Returns the gameid string for world.mt
    pub fn game_id(&self) -> &'static str {
        match self {
            Self::MineTestGame => "minetest",
            Self::Mineclonia => "mineclonia",
        }
    }
}

/// A Luanti node with its name and param2 value
pub struct LuantiNode {
    pub name: &'static str,
    pub param2: u8,
}

// ---------------------------------------------------------------------------
// MC2MT-style conversion functions
// ---------------------------------------------------------------------------
// Ported from MC2MT's conversions.h (C macros → Rust functions).
// Each function resolves block properties into a LuantiNode with both
// the correct node name AND the correct facedir/wallmounted param2.

/// MC facing direction → Luanti facedir (with Z-axis flip applied).
///
/// Minecraft:  Z+ = South, Z- = North
/// Luanti:     Z+ = North, Z- = South
///
/// Facedir mapping:
///   "north" (-Z_mc = +Z_lt) → 0
///   "east"  (+X)             → 1
///   "south" (+Z_mc = -Z_lt) → 2
///   "west"  (-X)             → 3
fn facing_to_facedir(facing: &str) -> u8 {
    match facing {
        "north" => 0,
        "east" => 1,
        "south" => 2,
        "west" => 3,
        _ => 0,
    }
}

/// Read a string property from an optional NBT compound.
fn prop_str<'a>(props: Option<&'a Value>, key: &str) -> Option<&'a str> {
    match props {
        Some(Value::Compound(map)) => match map.get(key) {
            Some(Value::String(s)) => Some(s.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// Check if a string property equals a specific value.
fn prop_eq(props: Option<&Value>, key: &str, val: &str) -> bool {
    prop_str(props, key) == Some(val)
}

/// MC2MT `CONV_TRAPDOOR` equivalent.
///
/// Resolves facing × open × half properties into (node_name, facedir param2).
///
/// MC2MT expands CONV_TRAPDOOR(id, mcn, mtn) into 16 CONV_DP entries:
///   data 0–3   (closed, bottom) → mtn,       param2 = facedir
///   data 4–7   (open,   bottom) → mtn_open,  param2 = facedir
///   data 8–11  (closed, top)    → mtn,       param2 = facedir + 20
///   data 12–15 (open,   top)    → mtn_open,  param2 = facedir + 20
fn conv_trapdoor(
    props: Option<&Value>,
    closed: &'static str,
    open: &'static str,
) -> LuantiNode {
    let facing = prop_str(props, "facing").unwrap_or("north");
    let is_open = prop_eq(props, "open", "true");
    let is_top = prop_eq(props, "half", "top");
    let base = facing_to_facedir(facing);
    let param2 = if is_top { base + 20 } else { base };
    let name = if is_open { open } else { closed };
    LuantiNode { name, param2 }
}

/// MC2MT `CONV_STAIR` equivalent.
///
/// MC2MT expands CONV_STAIR(id, mcn, mtn) into 8 CONV_DP entries:
///   data 0–3 (bottom) → mtn, param2 = facedir
///   data 4–7 (top)    → mtn, param2 = facedir + 20
fn conv_stair(props: Option<&Value>, name: &'static str) -> LuantiNode {
    let facing = prop_str(props, "facing").unwrap_or("north");
    let is_top = prop_eq(props, "half", "top");
    let base = facing_to_facedir(facing);
    let param2 = if is_top { base + 20 } else { base };
    LuantiNode { name, param2 }
}

/// MC2MT `CONV_SLAB` equivalent.
///
/// MC2MT expands CONV_SLAB(id, mcn, dbottom, dtop, mtn) into 2 entries:
///   bottom → mtn,        param2 = 0
///   top    → mtn "_top", param2 = 0
#[allow(dead_code)]
fn conv_slab(
    props: Option<&Value>,
    bottom: &'static str,
    top: &'static str,
) -> LuantiNode {
    let name = if prop_eq(props, "type", "top") {
        top
    } else {
        bottom
    };
    LuantiNode { name, param2: 0 }
}

/// Maps an Arnis Block to a Luanti node for the given game pack.
///
/// Directional blocks (stairs, trapdoors, etc.) use the optional `props`
/// (Minecraft NBT block properties) to compute the correct `param2` value.
pub fn to_luanti_node(block: Block, game: LuantiGame, props: Option<&Value>) -> LuantiNode {
    match game {
        LuantiGame::MineTestGame => to_minetest_game_node(block, props),
        LuantiGame::Mineclonia => to_mineclonia_node(block, props),
    }
}

fn to_minetest_game_node(block: Block, props: Option<&Value>) -> LuantiNode {
    let name = match block.id() {
        0 => "default:acacia_wood",  // acacia_planks
        1 => "air",  // air
        2 => "default:stone",  // andesite
        3 => "default:aspen_leaves",  // birch_leaves
        4 => "default:aspen_tree",  // birch_log
        5 => "default:obsidianbrick",  // black_concrete
        6 => "default:obsidian",  // blackstone
        7 => "flowers:viola",  // blue_orchid
        8 => "default:clay",  // blue_terracotta
        9 => "default:brick",  // bricks
        10 => "default:steelblock",  // cauldron
        11 => "default:stonebrick",  // chiseled_stone_bricks
        12 => "walls:cobble",  // cobblestone_wall
        13 => "default:cobble",  // cobblestone
        14 => "default:obsidianbrick",  // polished_blackstone_bricks
        15 => "default:stonebrick",  // cracked_stone_bricks
        16 => "default:wood",  // crimson_planks
        17 => "default:sandstonebrick",  // cut_sandstone
        18 => "wool:cyan",  // cyan_concrete
        19 => "default:wood",  // dark_oak_planks
        20 => "default:obsidianbrick",  // deepslate_bricks
        21 => "default:stone",  // diorite
        22 => "default:dirt",  // dirt
        23 => "default:stonebrick",  // end_stone_bricks
        24 => "farming:soil_wet",  // farmland
        25 => "default:glass",  // glass
        26 => "default:meselamp",  // glowstone
        27 => "default:stone",  // granite
        28 => "default:dirt_with_grass",  // grass_block
        29 => "default:grass_3",  // short_grass
        30 => "default:gravel",  // gravel
        31 => "default:coalblock",  // gray_concrete
        32 => "default:clay",  // gray_terracotta
        33 => "default:clay",  // green_terracotta
        34 => "wool:green",  // green_wool
        35 => "farming:straw",  // hay_block
        36 => "xpanes:bar_flat",  // iron_bars
        37 => "default:steelblock",  // iron_block
        38 => "default:junglewood",  // jungle_planks
        39 => "default:ladder_wood",  // ladder
        40 => "wool:cyan",  // light_blue_concrete
        41 => "default:clay",  // light_blue_terracotta
        42 => "wool:grey",  // light_gray_concrete
        43 => "default:dirt_with_grass",  // moss_block
        44 => "default:mossycobble",  // mossy_cobblestone
        45 => "default:brick",  // mud_bricks
        46 => "default:obsidianbrick",  // nether_bricks
        47 => "default:obsidian",  // netherite_block
        48 => "default:fence_wood",  // oak_fence
        49 => "default:leaves",  // oak_leaves
        50 => "default:tree",  // oak_log
        51 => "default:wood",  // oak_planks
        52 => "stairs:slab_wood",  // oak_slab
        53 => "default:clay",  // orange_terracotta
        54 => "default:dirt_with_coniferous_litter",  // podzol
        55 => "default:stone",  // polished_andesite
        56 => "default:obsidian",  // polished_basalt
        57 => "default:sandstone",  // quartz_block
        58 => "default:obsidian",  // polished_blackstone
        59 => "default:obsidian",  // polished_deepslate
        60 => "default:stone",  // polished_diorite
        61 => "default:stone",  // polished_granite
        62 => "default:stonebrick",  // prismarine
        63 => "default:stonebrick",  // purpur_block
        64 => "default:stonebrick",  // purpur_pillar
        65 => "default:sandstone",  // quartz_bricks
        66 => "carts:rail",  // rail
        67 => "flowers:rose",  // poppy
        68 => "default:obsidianbrick",  // red_nether_bricks
        69 => "default:desert_stone",  // red_terracotta
        70 => "wool:red",  // red_wool
        71 => "default:sand",  // sand
        72 => "default:sandstone",  // sandstone
        73 => "default:wood",  // scaffolding
        74 => "default:sandstone",  // smooth_quartz
        75 => "default:desert_sandstone",  // smooth_red_sandstone
        76 => "default:sandstone",  // smooth_sandstone
        77 => "default:stone",  // smooth_stone
        78 => "default:sand",  // sponge
        79 => "default:pine_tree",  // spruce_log
        80 => "default:pine_wood",  // spruce_planks
        81 => "stairs:slab_stone",  // stone_slab
        82 => "stairs:slab_stonebrick",  // stone_brick_slab
        83 => "default:stonebrick",  // stone_bricks
        84 => "default:stone",  // stone
        85 => "default:clay",  // terracotta
        86 => "default:wood",  // warped_planks
        87 => "default:water_source",  // water
        88 => "wool:white",  // white_concrete
        89 => "flowers:dandelion_white",  // azure_bluet
        90 => "default:glass",  // white_stained_glass
        91 => "default:clay",  // white_terracotta
        92 => "wool:white",  // white_wool
        93 => "wool:yellow",  // yellow_concrete
        94 => "flowers:dandelion_yellow",  // dandelion
        95 => "wool:yellow",  // yellow_wool
        96 => "wool:green",  // lime_concrete
        97 => "wool:cyan",  // cyan_wool
        98 => "wool:blue",  // blue_concrete
        99 => "wool:violet",  // purple_concrete
        100 => "wool:red",  // red_concrete
        101 => "wool:magenta",  // magenta_concrete
        102 => "wool:brown",  // brown_wool
        103 => "default:copperblock",  // oxidized_copper
        104 => "default:clay",  // yellow_terracotta
        105 => "farming:wheat_8",  // carrots
        106 => "doors:door_wood_b",  // dark_oak_door (lower)
        107 => "doors:door_wood_a",  // dark_oak_door (upper)
        108 => "farming:wheat_8",  // potatoes
        109 => "farming:wheat_8",  // wheat
        110 => "default:stone",  // bedrock
        111 => "default:snowblock",  // snow_block
        112 => "default:snow",  // snow (layer)
        113 => "default:sign_wall_wood",  // oak_sign
        114 => "walls:cobble",  // andesite_wall
        115 => "walls:cobble",  // stone_brick_wall
        // 116..=125: rail variants
        116..=125 => "carts:rail",  // rail_north_south
        126 => "default:dirt",  // coarse_dirt
        127 => "default:stone_with_iron",  // iron_ore
        128 => "default:stone_with_coal",  // coal_ore
        129 => "default:stone_with_gold",  // gold_ore
        130 => "default:stone_with_copper",  // copper_ore
        131 => "default:clay",  // clay
        132 => "default:dirt_with_grass",  // dirt_path
        133 => "default:ice",  // ice
        134 => "default:ice",  // packed_ice
        135 => "default:dirt",  // mud
        136 => "default:dry_shrub",  // dead_bush
        137 => "default:grass_5",  // tall_grass (bottom)
        138 => "default:grass_5",  // tall_grass (top)
        139 => "default:wood",  // crafting_table
        140 => "default:furnace",  // furnace
        141 => "wool:white",  // white_carpet
        142 => "default:bookshelf",  // bookshelf
        143 => "default:wood",  // oak_pressure_plate
        144 => return conv_stair(props, "stairs:stair_wood"),  // oak_stairs (MC2MT CONV_STAIR)
        155 => "default:chest",  // chest
        156 => "wool:red",  // red_carpet
        157 => "default:steelblock",  // anvil
        158 => "default:wood",  // note_block
        159 => "doors:door_wood_b",  // oak_door
        160 => "default:steelblock",  // brewing_stand
        // 161..=168: red_bed variants
        161..=168 => "wool:red",  // red_bed_north_head
        169 => "default:glass",  // gray_stained_glass
        170 => "default:glass",  // light_gray_stained_glass
        171 => "default:glass",  // brown_stained_glass
        172 => "default:obsidian_glass",  // tinted_glass
        // 173, 236–239, 241–243: Trapdoors (MC2MT CONV_TRAPDOOR)
        173 | 236 | 237 | 238 | 239 | 241 | 242 | 243 => {  // oak_trapdoor
            return conv_trapdoor(props, "doors:trapdoor", "doors:trapdoor_open")
        }
        174 => "wool:brown",  // brown_concrete
        175 => "default:obsidianbrick",  // black_terracotta
        176 => "default:clay",  // brown_terracotta
        // 177–187: Stairs (MC2MT CONV_STAIR)
        177 => return conv_stair(props, "stairs:stair_stonebrick"),  // stone_brick_stairs
        178 => return conv_stair(props, "stairs:stair_stonebrick"),  // mud_brick_stairs
        179 => return conv_stair(props, "stairs:stair_obsidian"),  // polished_blackstone_brick_stairs
        180 => return conv_stair(props, "stairs:stair_desert_cobble"),  // brick_stairs
        181 => return conv_stair(props, "stairs:stair_desert_stone"),  // polished_granite_stairs
        182 => return conv_stair(props, "stairs:stair_sandstone"),  // end_stone_brick_stairs
        183 => return conv_stair(props, "stairs:stair_stone"),  // polished_diorite_stairs
        184 => return conv_stair(props, "stairs:stair_sandstone"),  // smooth_sandstone_stairs
        185 => return conv_stair(props, "stairs:stair_sandstone"),  // quartz_stairs
        186 => return conv_stair(props, "stairs:stair_stone"),  // polished_andesite_stairs
        187 => return conv_stair(props, "stairs:stair_obsidian"),  // nether_brick_stairs
        188 => "default:chest",  // barrel
        189 => "default:fern_1",  // fern
        190 => "wool:white",  // cobweb
        // 191..=194: chiseled_bookshelf (N/E/S/W)
        191..=194 => "default:bookshelf",  // chiselled_bookshelf_north
        195 => "default:steelblock",  // chipped_anvil
        196 => "default:steelblock",  // damaged_anvil
        197 => "default:fern_3",  // large_fern (lower)
        198 => "default:fern_3",  // large_fern (upper)
        199 => "default:fence_wood",  // chain
        200 => "default:meselamp",  // end_rod
        201 => "default:steelblock",  // lightning_rod
        202 => "default:goldblock",  // gold_block
        203 => "default:meselamp",  // sea_lantern
        204 => "wool:orange",  // orange_concrete
        205 => "wool:orange",  // orange_wool
        206 => "wool:blue",  // blue_wool
        207 => "wool:dark_green",  // green_concrete
        208 => "default:brick",  // brick_wall
        209 => "default:desert_stone_block",  // redstone_block
        // 210..=211: chain variants
        210..=211 => "default:fence_wood",  // chain_x
        212 => "doors:door_wood_b",  // spruce_door (lower)
        213 => "doors:door_wood_a",  // spruce_door (upper)
        214 => "stairs:slab_stone",  // smooth_stone_slab
        215 => "xpanes:pane_flat",  // glass_pane
        216 => "default:clay",  // light_gray_terracotta
        217 => "stairs:slab_wood",  // oak_slab (variant)
        218 => "doors:door_wood_b",  // oak_door (variant)
        219 => "default:tree",  // dark_oak_log
        220 => "default:leaves",  // dark_oak_leaves
        221 => "default:jungletree",  // jungle_log
        222 => "default:jungleleaves",  // jungle_leaves
        223 => "default:acacia_tree",  // acacia_log
        224 => "default:acacia_leaves",  // acacia_leaves
        225 => "default:pine_needles",  // spruce_leaves
        226 => "default:glass",  // cyan_stained_glass
        227 => "default:glass",  // blue_stained_glass
        228 => "default:glass",  // light_blue_stained_glass
        229 => "default:wood",  // daylight_detector
        230 => "default:glass",  // red_stained_glass
        231 => "default:glass",  // yellow_stained_glass
        232 => "default:glass",  // purple_stained_glass
        233 => "default:glass",  // orange_stained_glass
        234 => "default:glass",  // magenta_stained_glass
        235 => "flowers:rose",  // potted_poppy
        240 => "stairs:slab_sandstone",  // quartz_slab
        244 => "stairs:slab_stonebrick",  // mud_brick_slab
        245 => "stairs:slab_brick",  // brick_slab
        246 => "flowers:tulip",  // potted_red_tulip
        247 => "flowers:dandelion_yellow",  // potted_dandelion
        248 => "flowers:viola",  // potted_blue_orchid
        _ => "default:stone",
    };
    LuantiNode { name, param2: 0 }
}

fn to_mineclonia_node(block: Block, props: Option<&Value>) -> LuantiNode {
    let name = match block.id() {
        0 => "mcl_trees:wood_acacia",  // acacia_planks
        1 => "air",  // air
        2 => "mcl_core:andesite",  // andesite
        3 => "mcl_trees:leaves_birch",  // birch_leaves
        4 => "mcl_trees:tree_birch",  // birch_log
        5 => "mcl_colorblocks:concrete_black",  // black_concrete
        6 => "mcl_blackstone:blackstone",  // blackstone
        7 => "mcl_flowers:blue_orchid",  // blue_orchid
        8 => "mcl_colorblocks:hardened_clay_blue",  // blue_terracotta
        9 => "mcl_core:brick_block",  // bricks
        10 => "mcl_cauldrons:cauldron",  // cauldron
        11 => "mcl_core:stonebrickcarved",  // chiseled_stone_bricks
        12 => "mcl_walls:cobble",  // cobblestone_wall
        13 => "mcl_core:cobble",  // cobblestone
        14 => "mcl_blackstone:blackstone_brick_polished",  // polished_blackstone_bricks
        15 => "mcl_core:stonebrickcracked",  // cracked_stone_bricks
        16 => "mcl_crimson:crimson_hyphae_wood",  // crimson_planks
        17 => "mcl_core:sandstonecarved",  // cut_sandstone
        18 => "mcl_colorblocks:concrete_cyan",  // cyan_concrete
        19 => "mcl_trees:wood_dark_oak",  // dark_oak_planks
        20 => "mcl_deepslate:deepslate_bricks",  // deepslate_bricks
        21 => "mcl_core:diorite",  // diorite
        22 => "mcl_core:dirt",  // dirt
        23 => "mcl_end:end_bricks",  // end_stone_bricks
        24 => "mcl_farming:soil_wet",  // farmland
        25 => "mcl_core:glass",  // glass
        26 => "mcl_nether:glowstone",  // glowstone
        27 => "mcl_core:granite",  // granite
        28 => "mcl_core:dirt_with_grass",  // grass_block
        29 => "mcl_flowers:tallgrass",  // short_grass
        30 => "mcl_core:gravel",  // gravel
        31 => "mcl_colorblocks:concrete_grey",  // gray_concrete
        32 => "mcl_colorblocks:hardened_clay_grey",  // gray_terracotta
        33 => "mcl_colorblocks:hardened_clay_green",  // green_terracotta
        34 => "mcl_wool:green",  // green_wool
        35 => "mcl_farming:hay_block",  // hay_block
        36 => "xpanes:bar_flat",  // iron_bars
        37 => "mcl_core:ironblock",  // iron_block
        38 => "mcl_trees:wood_jungle",  // jungle_planks
        39 => "mcl_core:ladder",  // ladder
        40 => "mcl_colorblocks:concrete_light_blue",  // light_blue_concrete
        41 => "mcl_colorblocks:hardened_clay_light_blue",  // light_blue_terracotta
        42 => "mcl_colorblocks:concrete_silver",  // light_gray_concrete
        43 => "mcl_lush_caves:moss",  // moss_block
        44 => "mcl_core:mossycobble",  // mossy_cobblestone
        45 => "mcl_mud:mud_bricks",  // mud_bricks
        46 => "mcl_nether:nether_brick",  // nether_bricks
        47 => "mcl_nether:netheriteblock",  // netherite_block
        48 => "mcl_fences:fence",  // oak_fence
        49 => "mcl_trees:leaves_oak",  // oak_leaves
        50 => "mcl_trees:tree_oak",  // oak_log
        51 => "mcl_trees:wood_oak",  // oak_planks
        52 => "mcl_stairs:slab_oak",  // oak_slab
        53 => "mcl_colorblocks:hardened_clay_orange",  // orange_terracotta
        54 => "mcl_core:podzol",  // podzol
        55 => "mcl_core:andesite_smooth",  // polished_andesite
        56 => "mcl_blackstone:basalt_polished",  // polished_basalt
        57 => "mcl_nether:quartz_block",  // quartz_block
        58 => "mcl_blackstone:blackstone_polished",  // polished_blackstone
        59 => "mcl_deepslate:deepslate_polished",  // polished_deepslate
        60 => "mcl_core:diorite_smooth",  // polished_diorite
        61 => "mcl_core:granite_smooth",  // polished_granite
        62 => "mcl_ocean:prismarine",  // prismarine
        63 => "mcl_end:purpur_block",  // purpur_block
        64 => "mcl_end:purpur_pillar",  // purpur_pillar
        65 => "mcl_nether:quartz_block",  // quartz_bricks
        66 => "mcl_minecarts:rail",  // rail
        67 => "mcl_flowers:poppy",  // poppy
        68 => "mcl_nether:red_nether_brick",  // red_nether_bricks
        69 => "mcl_colorblocks:hardened_clay_red",  // red_terracotta
        70 => "mcl_wool:red",  // red_wool
        71 => "mcl_core:sand",  // sand
        72 => "mcl_core:sandstone",  // sandstone
        73 => "mcl_bamboo:scaffolding",  // scaffolding
        74 => "mcl_nether:quartz_smooth",  // smooth_quartz
        75 => "mcl_core:redsandstonesmooth2",  // smooth_red_sandstone
        76 => "mcl_core:sandstonesmooth2",  // smooth_sandstone
        77 => "mcl_stairs:slab_stone_double",  // smooth_stone
        78 => "mcl_sponges:sponge",  // sponge
        79 => "mcl_trees:tree_spruce",  // spruce_log
        80 => "mcl_trees:wood_spruce",  // spruce_planks
        81 => "mcl_stairs:slab_stone",  // stone_slab
        82 => "mcl_stairs:slab_stonebrick",  // stone_brick_slab
        83 => "mcl_core:stonebrick",  // stone_bricks
        84 => "mcl_core:stone",  // stone
        85 => "mcl_colorblocks:hardened_clay",  // terracotta
        86 => "mcl_crimson:warped_hyphae_wood",  // warped_planks
        87 => "mcl_core:water_source",  // water
        88 => "mcl_colorblocks:concrete_white",  // white_concrete
        89 => "mcl_flowers:azure_bluet",  // azure_bluet
        90 => "mcl_core:glass_white",  // white_stained_glass
        91 => "mcl_colorblocks:hardened_clay_white",  // white_terracotta
        92 => "mcl_wool:white",  // white_wool
        93 => "mcl_colorblocks:concrete_yellow",  // yellow_concrete
        94 => "mcl_flowers:dandelion",  // dandelion
        95 => "mcl_wool:yellow",  // yellow_wool
        96 => "mcl_colorblocks:concrete_lime",  // lime_concrete
        97 => "mcl_wool:cyan",  // cyan_wool
        98 => "mcl_colorblocks:concrete_blue",  // blue_concrete
        99 => "mcl_colorblocks:concrete_purple",  // purple_concrete
        100 => "mcl_colorblocks:concrete_red",  // red_concrete
        101 => "mcl_colorblocks:concrete_magenta",  // magenta_concrete
        102 => "mcl_wool:brown",  // brown_wool
        103 => "mcl_copper:block_oxidized",  // oxidized_copper
        104 => "mcl_colorblocks:hardened_clay_yellow",  // yellow_terracotta
        105 => "mcl_farming:carrot_7",  // carrots
        106 => "mcl_doors:dark_oak_door_b_1",  // dark_oak_door (lower)
        107 => "mcl_doors:dark_oak_door_t_1",  // dark_oak_door (upper)
        108 => "mcl_farming:potato_4",  // potatoes
        109 => "mcl_farming:wheat_7",  // wheat
        110 => "mcl_core:bedrock",  // bedrock
        111 => "mcl_core:snowblock",  // snow_block
        112 => "mcl_core:snow",  // snow (layer)
        113 => "mcl_signs:wall_sign",  // oak_sign
        114 => "mcl_walls:andesite",  // andesite_wall
        115 => "mcl_walls:stonebrick",  // stone_brick_wall
        116..=125 => "mcl_minecarts:rail",  // rail_north_south
        126 => "mcl_core:coarse_dirt",  // coarse_dirt
        127 => "mcl_core:stone_with_iron",  // iron_ore
        128 => "mcl_core:stone_with_coal",  // coal_ore
        129 => "mcl_core:stone_with_gold",  // gold_ore
        130 => "mcl_copper:stone_with_copper",  // copper_ore
        131 => "mcl_core:clay",  // clay
        132 => "mcl_core:grass_path",  // dirt_path
        133 => "mcl_core:ice",  // ice
        134 => "mcl_core:packed_ice",  // packed_ice
        135 => "mcl_mud:mud",  // mud
        136 => "mcl_core:deadbush",  // dead_bush
        137 => "mcl_flowers:double_grass",  // tall_grass (bottom)
        138 => "mcl_flowers:double_grass_top",  // tall_grass (top)
        139 => "mcl_crafting_table:crafting_table",  // crafting_table
        140 => "mcl_furnaces:furnace",  // furnace
        141 => "mcl_wool:white_carpet",  // white_carpet
        142 => "mcl_books:bookshelf",  // bookshelf
        143 => "mcl_trees:wood_oak",  // oak_pressure_plate
        144 => return conv_stair(props, "mcl_stairs:stair_oak"),  // oak_stairs (MC2MT CONV_STAIR)
        155 => "mcl_chests:chest",  // chest
        156 => "mcl_wool:red_carpet",  // red_carpet
        157 => "mcl_anvils:anvil",  // anvil
        158 => "mcl_noteblock:noteblock",  // note_block
        159 => "mcl_doors:wooden_door_b_1",  // oak_door
        160 => "mcl_brewing:stand_000",  // brewing_stand
        161..=168 => "mcl_beds:bed_red_bottom",  // red_bed_north_head
        169 => "mcl_core:glass_grey",  // gray_stained_glass
        170 => "mcl_core:glass_silver",  // light_gray_stained_glass
        171 => "mcl_core:glass_brown",  // brown_stained_glass
        172 => "mcl_core:glass",  // tinted_glass
        // 173, 236–239: Trapdoors (MC2MT CONV_TRAPDOOR)
        173 | 236 | 237 | 238 | 239 => {  // oak_trapdoor
            return conv_trapdoor(props, "mcl_doors:trapdoor", "mcl_doors:trapdoor_open")
        }
        174 => "mcl_colorblocks:concrete_brown",  // brown_concrete
        175 => "mcl_colorblocks:hardened_clay_black",  // black_terracotta
        176 => "mcl_colorblocks:hardened_clay_brown",  // brown_terracotta
        // 177–187: Stairs (MC2MT CONV_STAIR)
        177 => return conv_stair(props, "mcl_stairs:stair_stonebrick"),  // stone_brick_stairs
        178 => return conv_stair(props, "mcl_stairs:stair_mud_brick"),  // mud_brick_stairs
        179 => return conv_stair(props, "mcl_stairs:stair_blackstone_brick_polished"),  // polished_blackstone_brick_stairs
        180 => return conv_stair(props, "mcl_stairs:stair_brick_block"),  // brick_stairs
        181 => return conv_stair(props, "mcl_stairs:stair_granite_smooth"),  // polished_granite_stairs
        182 => return conv_stair(props, "mcl_stairs:stair_end_bricks"),  // end_stone_brick_stairs
        183 => return conv_stair(props, "mcl_stairs:stair_diorite_smooth"),  // polished_diorite_stairs
        184 => return conv_stair(props, "mcl_stairs:stair_sandstone"),  // smooth_sandstone_stairs
        185 => return conv_stair(props, "mcl_stairs:stair_quartzblock"),  // quartz_stairs
        186 => return conv_stair(props, "mcl_stairs:stair_andesite_smooth"),  // polished_andesite_stairs
        187 => return conv_stair(props, "mcl_stairs:stair_nether_brick"),  // nether_brick_stairs
        188 => "mcl_barrels:barrel_closed",  // barrel
        189 => "mcl_flowers:fern",  // fern
        190 => "mcl_core:cobweb",  // cobweb
        191..=194 => "mcl_books:bookshelf",  // chiselled_bookshelf_north
        195 => "mcl_anvils:anvil_damage_1",  // chipped_anvil
        196 => "mcl_anvils:anvil_damage_2",  // damaged_anvil
        197 => "mcl_flowers:double_fern",  // large_fern (lower)
        198 => "mcl_flowers:double_fern_top",  // large_fern (upper)
        199 => "mcl_lanterns:chain",  // chain
        200 => "mcl_end:end_rod",  // end_rod
        201 => "mcl_lightning_rods:rod",  // lightning_rod
        202 => "mcl_core:goldblock",  // gold_block
        203 => "mcl_ocean:sea_lantern",  // sea_lantern
        204 => "mcl_colorblocks:concrete_orange",  // orange_concrete
        205 => "mcl_wool:orange",  // orange_wool
        206 => "mcl_wool:blue",  // blue_wool
        207 => "mcl_colorblocks:concrete_green",  // green_concrete
        208 => "mcl_walls:brick",  // brick_wall
        209 => "mcl_redstone_torch:redstoneblock",  // redstone_block
        210..=211 => "mcl_lanterns:chain",  // chain_x
        212 => "mcl_doors:spruce_door_b_1",  // spruce_door (lower)
        213 => "mcl_doors:spruce_door_t_1",  // spruce_door (upper)
        214 => "mcl_stairs:slab_stone_double",  // smooth_stone_slab
        215 => "mcl_core:glass",  // glass_pane
        216 => "mcl_colorblocks:hardened_clay_silver",  // light_gray_terracotta
        217 => "mcl_stairs:slab_oak",  // oak_slab (variant)
        218 => "mcl_doors:wooden_door_b_1",  // oak_door (variant)
        219 => "mcl_trees:tree_dark_oak",  // dark_oak_log
        220 => "mcl_trees:leaves_dark_oak",  // dark_oak_leaves
        221 => "mcl_trees:tree_jungle",  // jungle_log
        222 => "mcl_trees:leaves_jungle",  // jungle_leaves
        223 => "mcl_trees:tree_acacia",  // acacia_log
        224 => "mcl_trees:leaves_acacia",  // acacia_leaves
        225 => "mcl_trees:leaves_spruce",  // spruce_leaves
        226 => "mcl_core:glass_cyan",  // cyan_stained_glass
        227 => "mcl_core:glass_blue",  // blue_stained_glass
        228 => "mcl_core:glass_light_blue",  // light_blue_stained_glass
        229 => "mcl_daylight_detector:daylight_detector",  // daylight_detector
        230 => "mcl_core:glass_red",  // red_stained_glass
        231 => "mcl_core:glass_yellow",  // yellow_stained_glass
        232 => "mcl_core:glass_purple",  // purple_stained_glass
        233 => "mcl_core:glass_orange",  // orange_stained_glass
        234 => "mcl_core:glass_magenta",  // magenta_stained_glass
        235 => "mcl_flowers:poppy",  // potted_poppy
        240 => "mcl_stairs:slab_quartzblock",  // quartz_slab
        // 241–243: Trapdoors (MC2MT CONV_TRAPDOOR)
        241 => return conv_trapdoor(props, "mcl_doors:dark_oak_trapdoor", "mcl_doors:dark_oak_trapdoor_open"),  // dark_oak_trapdoor
        242 => return conv_trapdoor(props, "mcl_doors:spruce_trapdoor", "mcl_doors:spruce_trapdoor_open"),  // spruce_trapdoor
        243 => return conv_trapdoor(props, "mcl_doors:birch_trapdoor", "mcl_doors:birch_trapdoor_open"),  // birch_trapdoor
        244 => "mcl_stairs:slab_mud_brick",  // mud_brick_slab
        245 => "mcl_stairs:slab_brick_block",  // brick_slab
        246 => "mcl_flowers:tulip_red",  // potted_red_tulip
        247 => "mcl_flowers:dandelion",  // potted_dandelion
        248 => "mcl_flowers:blue_orchid",  // potted_blue_orchid
        _ => "mcl_core:stone",
    };
    LuantiNode { name, param2: 0 }
}
