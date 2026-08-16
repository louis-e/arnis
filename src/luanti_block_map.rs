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

// Part of the table below no longer comes from MC2MT: node names added or
// corrected since are read off the Mineclonia source (mods/ITEMS/**), which is
// LGPL-2.1-or-later as well. The MC2MT attribution above still applies -- the
// conv_* helpers are direct ports of its C macros, and most of the mapping is
// unchanged from it.

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
/// Door halves: the stored props decide, else the id (the *_UPPER ids are top).
fn conv_door(props: Option<&Value>, id: u16, species: &str) -> LuantiNode {
    let is_upper = match prop_str(props, "half") {
        Some(h) => h == "upper",
        None => matches!(id, 107 | 365 | 366),
    };
    let name: &'static str = match (species, is_upper) {
        ("dark_oak", false) => "mcl_doors:door_dark_oak_b_1",
        ("dark_oak", true) => "mcl_doors:door_dark_oak_t_1",
        ("spruce", false) => "mcl_doors:door_spruce_b_1",
        ("spruce", true) => "mcl_doors:door_spruce_t_1",
        ("birch", false) => "mcl_doors:door_birch_b_1",
        ("birch", true) => "mcl_doors:door_birch_t_1",
        (_, false) => "mcl_doors:door_oak_b_1",
        (_, true) => "mcl_doors:door_oak_t_1",
    };
    LuantiNode {
        name,
        param2: facing_to_facedir(prop_str(props, "facing").unwrap_or("north")),
    }
}

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
        0 => "mcl_trees:tree_mangrove",
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
        16 => "mcl_colorblocks:concrete_cyan",
        17 => return conv_stair(props, "mcl_stairs:stair_cobble"), // COBBLESTONE_STAIRS
        18 => "mcl_nether:magma",                                  // MAGMA_BLOCK
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
        36 => "mcl_panes:bar_flat",
        37 => "mcl_core:ironblock",
        38 => return conv_stair(props, "mcl_stairs:stair_copper_cut"), // WAXED_CUT_COPPER_STAIRS
        39 => "mcl_colorblocks:concrete_yellow",
        40 => "mcl_core:snow", // SNOW_LAYER
        41 => "mcl_colorblocks:hardened_clay_light_blue",
        42 => "mcl_colorblocks:concrete_silver",
        43 => "mcl_lush_caves:moss",
        44 => "mcl_core:mossycobble",
        45 => "mcl_mud:mud_bricks",
        46 => "mcl_nether:nether_brick",
        47 => "mcl_nether:netheriteblock",
        48 => "mcl_fences:oak_fence",
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
        66 => "mcl_core:water_source", // KELP
        67 => "mcl_flowers:poppy",
        68 => "mcl_nether:red_nether_brick",
        69 => "mcl_colorblocks:hardened_clay_red",
        70 => "mcl_core:water_source", // TALL_SEAGRASS_BOTTOM
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
        93 => "mcl_core:water_source", // TALL_SEAGRASS_TOP
        94 => "mcl_flowers:dandelion",
        95 => "mcl_core:water_source", // SEA_PICKLE
        96 => "mcl_nether:soul_sand",  // SOUL_SAND
        97 => return conv_stair(props, "mcl_stairs:stair_red_nether_brick"), // RED_NETHER_BRICK_STAIRS
        98 => "mcl_walls:sandstone",
        99 => "mcl_stairs:slab_sandstonesmooth",
        100 => "mcl_colorblocks:concrete_red",
        101 => "mcl_doors:iron_trapdoor", // IRON_TRAPDOOR
        102 => return conv_stair(props, "mcl_stairs:stair_copper_oxidized_cut"), // WAXED_OXIDIZED_CUT_COPPER_STAIRS
        103 => "mcl_copper:block_oxidized", // WAXED_OXIDIZED_COPPER (waxed variant approximated as oxidized)
        104 => "mcl_colorblocks:hardened_clay_yellow",
        105 => "mcl_farming:carrot_7",
        106 | 107 => return conv_door(props, block.id(), "dark_oak"),
        108 => "mcl_farming:potato_4",
        109 => "mcl_farming:wheat_7",
        110 => "mcl_core:bedrock",
        111 => "mcl_core:snowblock",
        112 => return conv_stair(props, "mcl_stairs:stair_andesite"), // ANDESITE_STAIRS
        113 => {
            return conv_trapdoor(
                props,
                "mcl_doors:trapdoor_jungle",
                "mcl_doors:trapdoor_jungle_open",
            )
        } // JUNGLE_TRAPDOOR
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
        133 => return conv_stair(props, "mcl_stairs:stair_copper_exposed_cut"), // WAXED_EXPOSED_CUT_COPPER_STAIRS
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
        145 => "mcl_colorblocks:concrete_orange",
        146 => "mcl_colorblocks:concrete_purple",
        147 => "mcl_fences:birch_fence_gate",
        148 => "mcl_fences:dark_oak_fence_gate",
        149 => "mcl_colorblocks:concrete_light_blue",
        150 => "mcl_core:stonebrickmossy", // MOSSY_STONE_BRICKS
        151 => "mcl_deepslate:deepslate",  // DEEPSLATE
        152 => "mcl_deepslate:tuff",       // TUFF
        153 => "mcl_deepslate:deepslate_cobbled", // COBBLED_DEEPSLATE
        154 => "mcl_lanterns:lantern_floor",
        155 => "mcl_chests:chest",
        156 => "mcl_buttons:button_stone_off",
        157 => "mcl_anvils:anvil",
        158 => "mcl_noteblock:noteblock",
        159 => "mcl_deepslate:deepslatepolishedwall",
        160 => "mcl_brewing:stand_000",
        161 | 162 | 165..=168 | 294 | 295 => "mcl_beds:bed_red_bottom",
        163 => "mcl_core:glass_black",
        164 => "mcl_stairs:slab_andesite_smooth",
        169 => "mcl_core:glass_grey",
        170 => "mcl_core:glass_silver",
        171 => "mcl_core:glass_brown",
        172 => "mcl_core:glass",
        173 => "mcl_colorblocks:concrete_magenta",
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
        190 => "mcl_colorblocks:concrete_lime",
        191 => "mcl_colorblocks:concrete_blue",
        192 => "mcl_panes:pane_grey_flat",
        193 => "mcl_fences:oak_fence_gate",
        194 => "mcl_fences:spruce_fence_gate",
        195 => "mcl_copper:block", // WAXED_COPPER_BLOCK
        196 => "mcl_panes:pane_natural_flat",
        197 => "mcl_flowers:double_fern",
        198 => "mcl_flowers:double_fern_top",
        199 => "mcl_copper:block_exposed", // WAXED_EXPOSED_COPPER
        200 => return conv_stair(props, "mcl_stairs:stair_stone_rough"), // STONE_STAIRS
        201 => "mcl_lightning_rods:rod",
        202 => "mcl_flowerpots:flower_pot",
        203 => "mcl_ocean:sea_lantern",
        204 => "mcl_copper:block_exposed_chiseled", // WAXED_EXPOSED_CHISELED_COPPER
        205 => "mcl_stairs:slab_warped",
        206 => return conv_stair(props, "mcl_stairs:stair_warped"), // WARPED_STAIRS
        207 => "mcl_colorblocks:concrete_green",
        208 => "mcl_walls:brick",
        209 => "mcl_redstone_torch:redstoneblock",
        210 | 362 => "mcl_lanterns:chain",
        211 => {
            return conv_trapdoor(
                props,
                "mcl_doors:trapdoor_warped",
                "mcl_doors:trapdoor_warped_open",
            )
        } // WARPED_TRAPDOOR
        212 => "mcl_trees:stripped_warped",
        213 => "mcl_trees:bark_stripped_warped",
        214 => "mcl_stairs:slab_stone_double",
        215 => "mcl_copper:block_exposed_cut", // WAXED_EXPOSED_CUT_COPPER
        216 => "mcl_colorblocks:hardened_clay_silver",
        217 => "mcl_stairs:slab_oak",
        218 => "mcl_redstone_lamp:lamp_on", // REDSTONE_LAMP (only placed lit)
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
        230 => "mcl_trees:tree_cherry_blossom", // CHERRY_LOG (Mineclonia may not have cherry yet)
        231 => "mcl_trees:leaves_cherry_blossom", // CHERRY_LEAVES (fallback if cherry missing)
        232 => "mcl_colorblocks:concrete_powder_brown", // BROWN_CONCRETE_POWDER
        233 => "mcl_trees:leaves_mangrove",
        234 => "mcl_trees:leaves_azalea",
        235 => "mcl_flowers:poppy",
        236 | 299 => {
            return conv_trapdoor(
                props,
                "mcl_doors:trapdoor_oak",
                "mcl_doors:trapdoor_oak_open",
            )
        }
        237 => "mcl_core:reeds",        // SUGAR_CANE
        238 => "mcl_core:water_source", // SEAGRASS
        239 => "mcl_core:water_source", // KELP_PLANT
        240 => "mcl_stairs:slab_quartzblock",
        241 => {
            return conv_trapdoor(
                props,
                "mcl_doors:trapdoor_dark_oak",
                "mcl_doors:trapdoor_dark_oak_open",
            )
        }
        242 => {
            return conv_trapdoor(
                props,
                "mcl_doors:trapdoor_spruce",
                "mcl_doors:trapdoor_spruce_open",
            )
        }
        243 => {
            return conv_trapdoor(
                props,
                "mcl_doors:trapdoor_birch",
                "mcl_doors:trapdoor_birch_open",
            )
        }
        244 => "mcl_stairs:slab_mud_brick",
        245 => "mcl_stairs:slab_brick_block",
        246 => "mcl_flowers:tulip_red",
        247 => "mcl_flowers:dandelion",
        248 => "mcl_flowers:blue_orchid",
        249 => "mcl_core:stone_with_diamond",
        250 => "mcl_core:stone_with_redstone",
        251 => "mcl_core:stone_with_lapis",
        252 => "mcl_colorblocks:concrete_powder_grey", // GRAY_CONCRETE_POWDER
        253 => "mcl_colorblocks:hardened_clay_cyan",   // CYAN_TERRACOTTA
        254 => "mcl_wool:black",                       // BLACK_WOOL
        255 => "mcl_banners:hanging_banner",           // LIGHT_GRAY_WALL_BANNER
        256 => "mcl_lever:lever_off",                  // LEVER
        257 => "mcl_grindstone:grindstone",
        258 => "mcl_minecarts:rail",
        259 => "mcl_wool:red",
        260 => "mcl_core:ladder",
        261 => "mcl_wool:yellow",
        265 => "mcl_stairs:slab_cobble",
        266 => "mcl_fences:nether_brick_fence",
        267 => "mcl_fences:birch_fence",
        268 => "mcl_stairs:slab_quartz_smooth",
        269 => return conv_stair(props, "mcl_stairs:stair_quartz_smooth"), // SMOOTH_QUARTZ_STAIRS
        270 => return conv_stair(props, "mcl_stairs:stair_blackstone"),    // BLACKSTONE_STAIRS
        271 => "mcl_blackstone:wall",
        272 => "mcl_walls:diorite",
        273 => "mcl_stairs:slab_deepslate_polished",
        274 => "mcl_signs:wall_sign_oak",
        275 => "mcl_banners:hanging_banner", // BLUE_WALL_BANNER
        276 => "mcl_fences:jungle_fence",
        277 => "mcl_banners:hanging_banner", // BLACK_WALL_BANNER
        278 => "mcl_banners:hanging_banner", // RED_WALL_BANNER
        279 => return conv_door(props, block.id(), "birch"),
        280 => "mcl_pressureplates:pressure_plate_birch_off",
        281 => "mcl_pressureplates:pressure_plate_stone_off",
        282 => "mcl_blast_furnace:blast_furnace",
        283 => "mcl_dispensers:dispenser",
        284 => "mcl_hoppers:hopper",
        285 => "mcl_banners:hanging_banner", // GREEN_WALL_BANNER
        286 => "mcl_cauldrons:cauldron_3",   // WATER_CAULDRON (filled)
        287 => "mcl_compass:lodestone",
        288 => "mcl_redstone_torch:redstone_torch_on",
        289 => "mcl_wool:red_carpet",
        290 => "mcl_blackstone:blackstone_chiseled_polished",
        291 => "mcl_walls:stonebrickmossy",
        292 => return conv_stair(props, "mcl_stairs:stair_bamboo"), // BAMBOO_STAIRS
        293 => return conv_door(props, block.id(), "oak"),
        296 => "mcl_walls:endbricks",
        297 => "mcl_stairs:slab_bamboo",
        298 => "mcl_deepslate:deepslate_chiseled",
        300 => "mcl_buttons:button_birch_off",
        301 => "mcl_core:cobweb",
        302 => "mcl_stairs:slab_dark_oak",
        303 => "mcl_stairs:slab_jungle",
        304 => return conv_stair(props, "mcl_stairs:stair_jungle"), // JUNGLE_STAIRS
        305 | 316 | 320 | 326 => "mcl_books:bookshelf",
        306 => "mcl_buttons:button_oak_off",
        307 => "mcl_minecarts:golden_rail",
        308 => "mcl_fences:spruce_fence",
        309 => "mcl_stairs:slab_spruce",
        310 => "mcl_stairs:slab_andesite",
        311 => "mcl_stairs:slab_deepslate_cobbled",
        312 => return conv_stair(props, "mcl_stairs:stair_deepslate_cobbled"), // COBBLED_DEEPSLATE_STAIRS
        313 => "mcl_fences:dark_oak_fence",
        314 => "mcl_pressureplates:pressure_plate_dark_oak_off",
        317 => "mcl_banners:hanging_banner",
        318 => "mcl_wool:grey",
        319 => "mcl_nether:nether_wart_block",
        321 => "mcl_blackstone:basalt_polished",
        322 => "mcl_buttons:button_polished_blackstone_off",
        323 => "mcl_pressureplates:pressure_plate_polished_blackstone_off",
        324 => "mcl_stairs:slab_red_nether_brick",
        325 => "mcl_buttons:button_spruce_off",
        327 => {
            return conv_trapdoor(
                props,
                "mcl_doors:trapdoor_acacia",
                "mcl_doors:trapdoor_acacia_open",
            )
        } // ACACIA_TRAPDOOR
        328 => "mcl_composters:composter",
        329 => "mcl_wool:cyan_carpet",
        330 => "mcl_buttons:button_dark_oak_off",
        331 => "mcl_stairs:slab_end_bricks",
        332 => "mcl_anvils:anvil_damage_2",
        333 => "mcl_wool:green_carpet",
        334 => "mcl_wool:light_blue_carpet",
        335 => "mcl_walls:netherbrick",
        336 => "mcl_smoker:smoker",
        337 => "mcl_core:redsandstonesmooth2",
        338 => "mcl_stairs:slab_redsandstonesmooth2",
        339 => "mcl_panes:pane_blue_flat",
        340 => "mcl_wool:cyan",
        341 => "mcl_wool:silver_carpet",
        342 => "mcl_stairs:slab_mossycobble",
        343 => "mcl_stairs:slab_stonebrickmossy",
        344 => "mcl_ocean:prismarine",
        345 => "mcl_end:end_rod",
        347 => "mcl_signs:wall_sign_spruce",
        348 => return conv_stair(props, "mcl_stairs:stair_granite"), // GRANITE_STAIRS
        349 => return conv_stair(props, "mcl_stairs:stair_diorite"), // DIORITE_STAIRS
        350 => "mcl_deepslate:deepslate_tiles",
        351 => "mcl_stairs:slab_deepslate_tiles",
        352 => "mcl_deepslate:deepslatetileswall",
        353 => "mcl_stairs:slab_blackstone_polished",
        354 => "mcl_stairs:slab_diorite_smooth",
        355 => "mcl_lanterns:soul_lantern_floor",
        356 => "mcl_nether:quartz_chiseled",
        357 => "mcl_nether:quartz_pillar",
        358 => "mcl_redstone_torch:redstone_torch_on_wall",
        359 => "mcl_core:goldblock",
        360 => "mcl_wool:orange",
        361 => "mcl_wool:blue",
        363 => "mcl_banners:hanging_banner", // WHITE_WALL_BANNER
        364 | 365 => return conv_door(props, block.id(), "spruce"),
        366 => return conv_door(props, block.id(), "oak"),
        _ => "mcl_core:stone",
    };
    LuantiNode { name, param2: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_definitions::STONE;

    /// Blocks Mineclonia has no node for, so they can only fall back. Keep this
    /// list short: every entry is a block that exports as plain stone.
    const NO_MINECLONIA_NODE: &[&str] = &["tripwire_hook"];

    /// The fallback arm is silent -- an unmapped block turns into stone in the
    /// exported world rather than failing. Walk the whole palette so that adding
    /// a block without a Mineclonia node fails here instead of in someone's map.
    #[test]
    fn every_block_maps_to_a_mineclonia_node() {
        let mut unmapped = Vec::new();
        for id in 0..=u16::MAX {
            let block = Block::from_raw_id(id);
            let Some(name) = block.try_name() else {
                continue;
            };
            if block == STONE || NO_MINECLONIA_NODE.contains(&name) {
                continue;
            }
            if to_mineclonia_node(block, None).name == "mcl_core:stone" {
                unmapped.push(format!("{name} (id {id})"));
            }
        }
        assert!(
            unmapped.is_empty(),
            "these blocks fall through to the mcl_core:stone fallback and would \
             export as stone: {unmapped:?}. Add a Mineclonia node for each, or \
             list it in NO_MINECLONIA_NODE if Mineclonia genuinely has none.",
        );
    }
}
