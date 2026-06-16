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

/// Supported Luanti game pack
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LuantiGame {
    /// Mineclonia — Minecraft-like game for Luanti
    Mineclonia,
}

impl LuantiGame {
    /// Returns the gameid string for world.mt
    pub fn game_id(&self) -> &'static str {
        match self {
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
fn conv_trapdoor(props: Option<&Value>, closed: &'static str, open: &'static str) -> LuantiNode {
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
fn conv_slab(props: Option<&Value>, bottom: &'static str, top: &'static str) -> LuantiNode {
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
        LuantiGame::Mineclonia => to_mineclonia_node(block, props),
    }
}

fn to_mineclonia_node(block: Block, props: Option<&Value>) -> LuantiNode {
    let name = match block.id() {
        1 => "air",
        2 => "mcl_core:andesite",
        3 => "mcl_trees:leaves_birch",
        4 => "mcl_trees:tree_birch",
        5 => "mcl_colorblocks:concrete_black",
        6 => "mcl_blackstone:blackstone",
        7 => "mcl_flowers:blue_orchid",
        8 => "mcl_colorblocks:hardened_clay_blue",
        9 => "mcl_core:brick_block",
        10 => "mcl_cauldrons:cauldron",
        11 => "mcl_core:stonebrickcarved",
        12 => "mcl_walls:cobble",
        13 => "mcl_core:cobble",
        14 => "mcl_blackstone:blackstone_brick_polished",
        15 => "mcl_core:stonebrickcracked",
        16 => "mesecons_walllever:wall_lever_off", // LEVER
        17 => return conv_stair(props, "mcl_stairs:stair_cobble"), // COBBLESTONE_STAIRS
        18 => "mcl_colorblocks:concrete_cyan",
        19 => "mcl_trees:wood_dark_oak",
        20 => "mcl_deepslate:deepslate_bricks",
        21 => "mcl_core:diorite",
        22 => "mcl_core:dirt",
        23 => "mcl_end:end_bricks",
        24 => "mcl_farming:soil_wet",
        25 => "mcl_core:glass",
        26 => "mcl_nether:glowstone",
        27 => "mcl_core:granite",
        28 => "mcl_core:dirt_with_grass",
        29 => "mcl_flowers:tallgrass",
        30 => "mcl_core:gravel",
        31 => "mcl_colorblocks:concrete_grey",
        32 => "mcl_colorblocks:hardened_clay_grey",
        33 => "mcl_colorblocks:hardened_clay_green",
        34 => "mcl_wool:green",
        35 => "mcl_farming:hay_block",
        36 => "xpanes:bar_flat",
        37 => "mcl_core:ironblock",
        38 => return conv_stair(props, "mcl_copper:waxed_cut_stair"), // WAXED_CUT_COPPER_STAIRS
        39 => "mcl_core:ladder",
        40 => "mcl_colorblocks:concrete_light_blue",
        41 => "mcl_colorblocks:hardened_clay_light_blue",
        42 => "mcl_colorblocks:concrete_silver",
        43 => "mcl_lush_caves:moss",
        44 => "mcl_core:mossycobble",
        45 => "mcl_mud:mud_bricks",
        46 => "mcl_nether:nether_brick",
        47 => "mcl_nether:netheriteblock",
        48 => "mcl_fences:fence",
        49 => "mcl_trees:leaves_oak",
        50 => "mcl_trees:tree_oak",
        51 => "mcl_trees:wood_oak",
        52 => "mcl_stairs:slab_oak",
        53 => "mcl_colorblocks:hardened_clay_orange",
        54 => "mcl_core:podzol",
        55 => "mcl_core:andesite_smooth",
        56 => return conv_stair(props, "mcl_stairs:stair_stonebrickmossy"), // MOSSY_STONE_BRICK_STAIRS
        57 => "mcl_nether:quartz_block",
        58 => "mcl_blackstone:blackstone_polished",
        59 => "mcl_deepslate:deepslate_polished",
        60 => "mcl_core:diorite_smooth",
        61 => "mcl_core:granite_smooth",
        62 => return conv_stair(props, "mcl_stairs:stair_mossycobble"), // MOSSY_COBBLESTONE_STAIRS
        63 => return conv_stair(props, "mcl_stairs:stair_deepslate_bricks"), // DEEPSLATE_BRICK_STAIRS
        64 => return conv_stair(props, "mcl_stairs:stair_deepslate_polished"), // POLISHED_DEEPSLATE_STAIRS
        65 => "mcl_nether:quartz_block",
        66 => "mcl_minecarts:rail",
        67 => "mcl_flowers:poppy",
        68 => "mcl_nether:red_nether_brick",
        69 => "mcl_colorblocks:hardened_clay_red",
        70 => "mcl_wool:red",
        71 => "mcl_core:sand",
        72 => "mcl_core:sandstone",
        73 => "mcl_bamboo:scaffolding",
        74 => "mcl_nether:quartz_smooth",
        75 => return conv_stair(props, "mcl_stairs:stair_spruce"), // SPRUCE_STAIRS
        76 => "mcl_core:sandstonesmooth2",
        77 => "mcl_stairs:slab_stone_double",
        78 => "mcl_sponges:sponge",
        79 => "mcl_trees:tree_spruce",
        80 => "mcl_trees:wood_spruce",
        81 => "mcl_stairs:slab_stone",
        82 => "mcl_stairs:slab_stonebrick",
        83 => "mcl_core:stonebrick",
        84 => "mcl_core:stone",
        85 => "mcl_colorblocks:hardened_clay",
        86 => return conv_stair(props, "mcl_stairs:stair_dark_oak"), // DARK_OAK_STAIRS
        87 => "mcl_core:water_source",
        88 => "mcl_colorblocks:concrete_white",
        89 => "mcl_flowers:azure_bluet",
        90 => "mcl_core:glass_white",
        91 => "mcl_colorblocks:hardened_clay_white",
        92 => "mcl_wool:white",
        93 => "mcl_colorblocks:concrete_yellow",
        94 => "mcl_flowers:dandelion",
        95 => "mcl_wool:yellow",
        96 => "mcl_colorblocks:concrete_lime",
        97 => return conv_stair(props, "mcl_stairs:stair_red_nether_brick"), // RED_NETHER_BRICK_STAIRS
        98 => "mcl_colorblocks:concrete_blue",
        99 => "mcl_colorblocks:concrete_purple",
        100 => "mcl_colorblocks:concrete_red",
        101 => "mcl_colorblocks:concrete_magenta",
        102 => return conv_stair(props, "mcl_copper:waxed_oxidized_cut_stair"), // WAXED_OXIDIZED_CUT_COPPER_STAIRS
        103 => "mcl_copper:block_oxidized", // WAXED_OXIDIZED_COPPER (waxed variant approximated as oxidized)
        104 => "mcl_colorblocks:hardened_clay_yellow",
        105 => "mcl_farming:carrot_7",
        106 => "mcl_doors:dark_oak_door_b_1",
        107 => "mcl_doors:dark_oak_door_t_1",
        108 => "mcl_farming:potato_4",
        109 => "mcl_farming:wheat_7",
        110 => "mcl_core:bedrock",
        111 => "mcl_core:snowblock",
        112 => return conv_stair(props, "mcl_stairs:stair_andesite"), // ANDESITE_STAIRS
        113 => "mcl_signs:wall_sign",
        114 => "mcl_walls:andesite",
        115 => "mcl_walls:stonebrick",
        116..=125 => "mcl_minecarts:rail",
        126 => "mcl_core:coarse_dirt",
        127 => "mcl_core:stone_with_iron",
        128 => "mcl_core:stone_with_coal",
        129 => "mcl_core:stone_with_gold",
        130 => "mcl_copper:stone_with_copper",
        131 => "mcl_core:clay",
        132 => "mcl_core:grass_path",
        133 => return conv_stair(props, "mcl_copper:waxed_exposed_cut_stair"), // WAXED_EXPOSED_CUT_COPPER_STAIRS
        134 => "mcl_core:packed_ice",
        135 => "mcl_mud:mud",
        136 => "mcl_core:deadbush",
        137 => "mcl_flowers:double_grass",
        138 => "mcl_flowers:double_grass_top",
        139 => "mcl_crafting_table:crafting_table",
        140 => "mcl_furnaces:furnace",
        141 => "mcl_wool:white_carpet",
        142 => "mcl_books:bookshelf",
        143 => "mcl_trees:wood_oak",
        144 => return conv_stair(props, "mcl_stairs:stair_oak"),
        145 => "mcl_banners:hanging_banner_white", // WHITE_WALL_BANNER
        146 => "mcl_banners:hanging_banner_blue",  // BLUE_WALL_BANNER
        147 => "mcl_banners:hanging_banner_black", // BLACK_WALL_BANNER
        148 => "mcl_banners:hanging_banner_red",   // RED_WALL_BANNER
        149 => "mcl_banners:hanging_banner_green", // GREEN_WALL_BANNER
        150 => "mcl_core:stonebrickmossy",         // MOSSY_STONE_BRICKS
        151 => "mcl_deepslate:deepslate",          // DEEPSLATE
        152 => "mcl_deepslate:tuff",               // TUFF
        153 => "mcl_deepslate:deepslate_cobbled",  // COBBLED_DEEPSLATE
        154 => "mcl_cauldrons:cauldron_3",         // WATER_CAULDRON (filled)
        155 => "mcl_chests:chest",
        156 => "mcl_wool:red_carpet",
        157 => "mcl_anvils:anvil",
        158 => "mcl_noteblock:noteblock",
        159 => "mcl_doors:wooden_door_b_1",
        160 => "mcl_brewing:stand_000",
        161..=168 => "mcl_beds:bed_red_bottom",
        169 => "mcl_core:glass_grey",
        170 => "mcl_core:glass_silver",
        171 => "mcl_core:glass_brown",
        172 => "mcl_core:glass",
        173 | 236 | 237 => {
            return conv_trapdoor(props, "mcl_doors:trapdoor", "mcl_doors:trapdoor_open")
        }
        238 => "mcl_ocean:seagrass", // SEAGRASS
        239 => "mcl_ocean:kelp",     // KELP_PLANT
        174 => "mcl_colorblocks:concrete_brown",
        175 => "mcl_colorblocks:hardened_clay_black",
        176 => "mcl_colorblocks:hardened_clay_brown",
        177 => return conv_stair(props, "mcl_stairs:stair_stonebrick"),
        178 => return conv_stair(props, "mcl_stairs:stair_mud_brick"),
        179 => return conv_stair(props, "mcl_stairs:stair_blackstone_brick_polished"),
        180 => return conv_stair(props, "mcl_stairs:stair_brick_block"),
        181 => return conv_stair(props, "mcl_stairs:stair_granite_smooth"),
        182 => return conv_stair(props, "mcl_stairs:stair_end_bricks"),
        183 => return conv_stair(props, "mcl_stairs:stair_diorite_smooth"),
        184 => return conv_stair(props, "mcl_stairs:stair_sandstone"),
        185 => return conv_stair(props, "mcl_stairs:stair_quartzblock"),
        186 => return conv_stair(props, "mcl_stairs:stair_andesite_smooth"),
        187 => return conv_stair(props, "mcl_stairs:stair_nether_brick"),
        188 => "mcl_barrels:barrel_closed",
        189 => "mcl_flowers:fern",
        190 => "mcl_core:cobweb",
        191..=194 => "mcl_books:bookshelf",
        195 => "mcl_copper:block", // WAXED_COPPER_BLOCK
        196 => "mcl_anvils:anvil_damage_2",
        197 => "mcl_flowers:double_fern",
        198 => "mcl_flowers:double_fern_top",
        199 => "mcl_copper:block_exposed", // WAXED_EXPOSED_COPPER
        200 => "mcl_end:end_rod",
        201 => "mcl_lightning_rods:rod",
        202 => "mcl_core:goldblock",
        203 => "mcl_ocean:sea_lantern",
        204 => "mcl_copper:block_chiseled_exposed", // WAXED_EXPOSED_CHISELED_COPPER
        205 => "mcl_wool:orange",
        206 => "mcl_wool:blue",
        207 => "mcl_colorblocks:concrete_green",
        208 => "mcl_walls:brick",
        209 => "mcl_redstone_torch:redstoneblock",
        210..=211 => "mcl_lanterns:chain",
        212 => "mcl_doors:spruce_door_b_1",
        213 => "mcl_doors:spruce_door_t_1",
        214 => "mcl_stairs:slab_stone_double",
        215 => "mcl_copper:block_cut_exposed", // WAXED_EXPOSED_CUT_COPPER
        216 => "mcl_colorblocks:hardened_clay_silver",
        217 => "mcl_stairs:slab_oak",
        218 => "mcl_doors:wooden_door_b_1",
        219 => "mcl_trees:tree_dark_oak",
        220 => "mcl_trees:leaves_dark_oak",
        221 => "mcl_trees:tree_jungle",
        222 => "mcl_trees:leaves_jungle",
        223 => "mcl_trees:tree_acacia",
        224 => "mcl_trees:leaves_acacia",
        225 => "mcl_trees:leaves_spruce",
        226 => "mcl_core:glass_cyan",
        227 => "mcl_core:glass_blue",
        228 => "mcl_core:glass_light_blue",
        229 => "mcl_daylight_detector:daylight_detector",
        230 => "mcl_cherry_blossom:cherrytree", // CHERRY_LOG (Mineclonia may not have cherry yet)
        231 => "mcl_cherry_blossom:leaves",     // CHERRY_LEAVES (fallback if cherry missing)
        232 => "mcl_colorblocks:concrete_powder_brown", // BROWN_CONCRETE_POWDER
        235 => "mcl_flowers:poppy",
        240 => "mcl_stairs:slab_quartzblock",
        241 => {
            return conv_trapdoor(
                props,
                "mcl_doors:dark_oak_trapdoor",
                "mcl_doors:dark_oak_trapdoor_open",
            )
        }
        242 => {
            return conv_trapdoor(
                props,
                "mcl_doors:spruce_trapdoor",
                "mcl_doors:spruce_trapdoor_open",
            )
        }
        243 => {
            return conv_trapdoor(
                props,
                "mcl_doors:birch_trapdoor",
                "mcl_doors:birch_trapdoor_open",
            )
        }
        244 => "mcl_stairs:slab_mud_brick",
        245 => "mcl_stairs:slab_brick_block",
        246 => "mcl_flowers:tulip_red",
        247 => "mcl_flowers:dandelion",
        248 => "mcl_flowers:blue_orchid",
        252 => "mcl_colorblocks:concrete_powder_grey", // GRAY_CONCRETE_POWDER
        253 => "mcl_colorblocks:hardened_clay_cyan",   // CYAN_TERRACOTTA
        254 => "mcl_wool:black",                       // BLACK_WOOL
        255 => "mcl_banners:hanging_banner_silver",    // LIGHT_GRAY_WALL_BANNER
        256 => "mcl_nether:magma",                     // MAGMA_BLOCK
        257 => "mcl_core:snow",                        // SNOW_LAYER
        258 => "mcl_ocean:kelp",                       // KELP
        259 => "mcl_ocean:tall_seagrass",              // TALL_SEAGRASS_BOTTOM
        260 => "mcl_ocean:tall_seagrass",              // TALL_SEAGRASS_TOP
        261 => "mcl_ocean:sea_pickle",                 // SEA_PICKLE
        265 => "mcl_nether:soul_sand",                 // SOUL_SAND
        _ => "mcl_core:stone",
    };
    LuantiNode { name, param2: 0 }
}
