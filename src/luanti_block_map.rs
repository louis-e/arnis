use crate::block_definitions::Block;

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
            Self::MineTestGame => "minetest_game",
            Self::Mineclonia => "mineclonia",
        }
    }
}

/// A Luanti node with its name and param2 value
pub struct LuantiNode {
    pub name: &'static str,
    pub param2: u8,
}

/// Maps an Arnis Block to a Luanti node name for the given game pack.
///
/// `param2` is set to 0 for most blocks; directional blocks (stairs, doors, etc.)
/// are handled separately via `block_properties_to_param2`.
pub fn to_luanti_node(block: Block, game: LuantiGame) -> LuantiNode {
    match game {
        LuantiGame::MineTestGame => to_minetest_game_node(block),
        LuantiGame::Mineclonia => to_mineclonia_node(block),
    }
}

fn to_minetest_game_node(block: Block) -> LuantiNode {
    let name = match block.id() {
        // 0: acacia_planks
        0 => "default:acacia_wood",
        // 1: air
        1 => "air",
        // 2: andesite
        2 => "default:stone",
        // 3: birch_leaves
        3 => "default:aspen_leaves",
        // 4: birch_log
        4 => "default:aspen_tree",
        // 5: black_concrete
        5 => "wool:black",
        // 6: blackstone
        6 => "default:obsidian",
        // 7: blue_orchid
        7 => "flowers:viola",
        // 8: blue_terracotta
        8 => "default:clay",
        // 9: bricks
        9 => "default:brick",
        // 10: cauldron
        10 => "default:steelblock",
        // 11: chiseled_stone_bricks
        11 => "default:stonebrick",
        // 12: cobblestone_wall
        12 => "walls:cobble",
        // 13: cobblestone
        13 => "default:cobble",
        // 14: polished_blackstone_bricks
        14 => "default:obsidianbrick",
        // 15: cracked_stone_bricks
        15 => "default:stonebrick",
        // 16: crimson_planks
        16 => "default:wood",
        // 17: cut_sandstone
        17 => "default:sandstonebrick",
        // 18: cyan_concrete
        18 => "wool:cyan",
        // 19: dark_oak_planks
        19 => "default:wood",
        // 20: deepslate_bricks
        20 => "default:obsidianbrick",
        // 21: diorite
        21 => "default:stone",
        // 22: dirt
        22 => "default:dirt",
        // 23: end_stone_bricks
        23 => "default:stonebrick",
        // 24: farmland
        24 => "farming:soil_wet",
        // 25: glass
        25 => "default:glass",
        // 26: glowstone
        26 => "default:meselamp",
        // 27: granite
        27 => "default:stone",
        // 28: grass_block
        28 => "default:dirt_with_grass",
        // 29: short_grass
        29 => "default:grass_3",
        // 30: gravel
        30 => "default:gravel",
        // 31: gray_concrete
        31 => "wool:grey",
        // 32: gray_terracotta
        32 => "default:clay",
        // 33: green_terracotta
        33 => "default:clay",
        // 34: green_wool
        34 => "wool:green",
        // 35: hay_block
        35 => "farming:straw",
        // 36: iron_bars
        36 => "xpanes:bar_flat",
        // 37: iron_block
        37 => "default:steelblock",
        // 38: jungle_planks
        38 => "default:junglewood",
        // 39: ladder
        39 => "default:ladder_wood",
        // 40: light_blue_concrete
        40 => "wool:cyan",
        // 41: light_blue_terracotta
        41 => "default:clay",
        // 42: light_gray_concrete
        42 => "wool:grey",
        // 43: moss_block
        43 => "default:dirt_with_grass",
        // 44: mossy_cobblestone
        44 => "default:mossycobble",
        // 45: mud_bricks
        45 => "default:brick",
        // 46: nether_bricks
        46 => "default:obsidianbrick",
        // 47: netherite_block
        47 => "default:obsidian",
        // 48: oak_fence
        48 => "default:fence_wood",
        // 49: oak_leaves
        49 => "default:leaves",
        // 50: oak_log
        50 => "default:tree",
        // 51: oak_planks
        51 => "default:wood",
        // 52: oak_slab
        52 => "stairs:slab_wood",
        // 53: orange_terracotta
        53 => "default:clay",
        // 54: podzol
        54 => "default:dirt_with_coniferous_litter",
        // 55: polished_andesite
        55 => "default:stone",
        // 56: polished_basalt
        56 => "default:obsidian",
        // 57: quartz_block
        57 => "default:sandstone",
        // 58: polished_blackstone
        58 => "default:obsidian",
        // 59: polished_deepslate
        59 => "default:obsidian",
        // 60: polished_diorite
        60 => "default:stone",
        // 61: polished_granite
        61 => "default:stone",
        // 62: prismarine
        62 => "default:stonebrick",
        // 63: purpur_block
        63 => "default:stonebrick",
        // 64: purpur_pillar
        64 => "default:stonebrick",
        // 65: quartz_bricks
        65 => "default:sandstone",
        // 66: rail
        66 => "carts:rail",
        // 67: poppy
        67 => "flowers:rose",
        // 68: red_nether_bricks
        68 => "default:obsidianbrick",
        // 69: red_terracotta
        69 => "default:clay",
        // 70: red_wool
        70 => "wool:red",
        // 71: sand
        71 => "default:sand",
        // 72: sandstone
        72 => "default:sandstone",
        // 73: scaffolding
        73 => "default:wood",
        // 74: smooth_quartz
        74 => "default:sandstone",
        // 75: smooth_red_sandstone
        75 => "default:desert_sandstone",
        // 76: smooth_sandstone
        76 => "default:sandstone",
        // 77: smooth_stone
        77 => "default:stone",
        // 78: sponge
        78 => "default:sand",
        // 79: spruce_log
        79 => "default:pine_tree",
        // 80: spruce_planks
        80 => "default:pine_wood",
        // 81: stone_slab
        81 => "stairs:slab_stone",
        // 82: stone_brick_slab
        82 => "stairs:slab_stonebrick",
        // 83: stone_bricks
        83 => "default:stonebrick",
        // 84: stone
        84 => "default:stone",
        // 85: terracotta
        85 => "default:clay",
        // 86: warped_planks
        86 => "default:wood",
        // 87: water
        87 => "default:water_source",
        // 88: white_concrete
        88 => "wool:white",
        // 89: azure_bluet
        89 => "flowers:dandelion_white",
        // 90: white_stained_glass
        90 => "default:glass",
        // 91: white_terracotta
        91 => "default:clay",
        // 92: white_wool
        92 => "wool:white",
        // 93: yellow_concrete
        93 => "wool:yellow",
        // 94: dandelion
        94 => "flowers:dandelion_yellow",
        // 95: yellow_wool
        95 => "wool:yellow",
        // 96: lime_concrete
        96 => "wool:green",
        // 97: cyan_wool
        97 => "wool:cyan",
        // 98: blue_concrete
        98 => "wool:blue",
        // 99: purple_concrete
        99 => "wool:violet",
        // 100: red_concrete
        100 => "wool:red",
        // 101: magenta_concrete
        101 => "wool:magenta",
        // 102: brown_wool
        102 => "wool:brown",
        // 103: oxidized_copper
        103 => "default:copperblock",
        // 104: yellow_terracotta
        104 => "default:clay",
        // 105: carrots
        105 => "farming:carrot_8",
        // 106: dark_oak_door (lower)
        106 => "doors:door_wood_b_1",
        // 107: dark_oak_door (upper)
        107 => "doors:door_wood_t_1",
        // 108: potatoes
        108 => "farming:potato_4",
        // 109: wheat
        109 => "farming:wheat_8",
        // 110: bedrock
        110 => "default:stone",
        // 111: snow_block
        111 => "default:snowblock",
        // 112: snow (layer)
        112 => "default:snow",
        // 113: oak_sign
        113 => "default:sign_wall_wood",
        // 114: andesite_wall
        114 => "walls:cobble",
        // 115: stone_brick_wall
        115 => "walls:stonebrick",
        // 116..=125: rail variants
        116..=125 => "carts:rail",
        // 126: coarse_dirt
        126 => "default:dirt",
        // 127: iron_ore
        127 => "default:stone_with_iron",
        // 128: coal_ore
        128 => "default:stone_with_coal",
        // 129: gold_ore
        129 => "default:stone_with_gold",
        // 130: copper_ore
        130 => "default:stone_with_copper",
        // 131: clay
        131 => "default:clay",
        // 132: dirt_path
        132 => "default:dirt_with_grass",
        // 133: ice
        133 => "default:ice",
        // 134: packed_ice
        134 => "default:ice",
        // 135: mud
        135 => "default:dirt",
        // 136: dead_bush
        136 => "default:dry_shrub",
        // 137: tall_grass (bottom)
        137 => "default:grass_5",
        // 138: tall_grass (top)
        138 => "default:grass_5",
        // 139: crafting_table
        139 => "default:wood",
        // 140: furnace
        140 => "default:furnace",
        // 141: white_carpet
        141 => "wool:white",
        // 142: bookshelf
        142 => "default:bookshelf",
        // 143: oak_pressure_plate
        143 => "default:wood",
        // 144: oak_stairs
        144 => "stairs:stair_wood",
        // 155: chest
        155 => "default:chest",
        // 156: red_carpet
        156 => "wool:red",
        // 157: anvil
        157 => "default:steelblock",
        // 158: note_block
        158 => "default:wood",
        // 159: oak_door
        159 => "doors:door_wood_b_1",
        // 160: brewing_stand
        160 => "default:steelblock",
        // 161..=168: red_bed variants
        161..=168 => "wool:red",
        // 169: gray_stained_glass
        169 => "default:glass",
        // 170: light_gray_stained_glass
        170 => "default:glass",
        // 171: brown_stained_glass
        171 => "default:glass",
        // 172: tinted_glass
        172 => "default:obsidian_glass",
        // 173: oak_trapdoor
        173 => "doors:trapdoor",
        // 174: brown_concrete
        174 => "wool:brown",
        // 175: black_terracotta
        175 => "default:clay",
        // 176: brown_terracotta
        176 => "default:clay",
        // 177: stone_brick_stairs
        177 => "stairs:stair_stonebrick",
        // 178: mud_brick_stairs
        178 => "stairs:stair_stonebrick",
        // 179: polished_blackstone_brick_stairs
        179 => "stairs:stair_obsidianbrick",
        // 180: brick_stairs
        180 => "stairs:stair_brick",
        // 181: polished_granite_stairs
        181 => "stairs:stair_stone",
        // 182: end_stone_brick_stairs
        182 => "stairs:stair_stonebrick",
        // 183: polished_diorite_stairs
        183 => "stairs:stair_stone",
        // 184: smooth_sandstone_stairs
        184 => "stairs:stair_sandstone",
        // 185: quartz_stairs
        185 => "stairs:stair_sandstone",
        // 186: polished_andesite_stairs
        186 => "stairs:stair_stone",
        // 187: nether_brick_stairs
        187 => "stairs:stair_obsidianbrick",
        // 188: barrel
        188 => "default:chest",
        // 189: fern
        189 => "default:fern_1",
        // 190: cobweb
        190 => "wool:white",
        // 191..=194: chiseled_bookshelf (N/E/S/W)
        191..=194 => "default:bookshelf",
        // 195: chipped_anvil
        195 => "default:steelblock",
        // 196: damaged_anvil
        196 => "default:steelblock",
        // 197: large_fern (lower)
        197 => "default:fern_3",
        // 198: large_fern (upper)
        198 => "default:fern_3",
        // 199: chain
        199 => "default:fence_wood",
        // 200: end_rod
        200 => "default:meselamp",
        // 201: lightning_rod
        201 => "default:steelblock",
        // 202: gold_block
        202 => "default:goldblock",
        // 203: sea_lantern
        203 => "default:meselamp",
        // 204: orange_concrete
        204 => "wool:orange",
        // 205: orange_wool
        205 => "wool:orange",
        // 206: blue_wool
        206 => "wool:blue",
        // 207: green_concrete
        207 => "wool:dark_green",
        // 208: brick_wall
        208 => "walls:stonebrick",
        // 209: redstone_block
        209 => "default:steelblock",
        // 210..=211: chain variants
        210..=211 => "default:fence_wood",
        // 212: spruce_door (lower)
        212 => "doors:door_wood_b_1",
        // 213: spruce_door (upper)
        213 => "doors:door_wood_t_1",
        // 214: smooth_stone_slab
        214 => "stairs:slab_stone",
        // 215: glass_pane
        215 => "xpanes:pane_flat",
        // 216: light_gray_terracotta
        216 => "default:clay",
        // 217: oak_slab (variant)
        217 => "stairs:slab_wood",
        // 218: oak_door (variant)
        218 => "doors:door_wood_b_1",
        // 219: dark_oak_log
        219 => "default:tree",
        // 220: dark_oak_leaves
        220 => "default:leaves",
        // 221: jungle_log
        221 => "default:jungletree",
        // 222: jungle_leaves
        222 => "default:jungleleaves",
        // 223: acacia_log
        223 => "default:acacia_tree",
        // 224: acacia_leaves
        224 => "default:acacia_leaves",
        // 225: spruce_leaves
        225 => "default:pine_needles",
        // 226: cyan_stained_glass
        226 => "default:glass",
        // 227: blue_stained_glass
        227 => "default:glass",
        // 228: light_blue_stained_glass
        228 => "default:glass",
        // 229: daylight_detector
        229 => "default:wood",
        // 230: red_stained_glass
        230 => "default:glass",
        // 231: yellow_stained_glass
        231 => "default:glass",
        // 232: purple_stained_glass
        232 => "default:glass",
        // 233: orange_stained_glass
        233 => "default:glass",
        // 234: magenta_stained_glass
        234 => "default:glass",
        // 235: potted_poppy
        235 => "flowers:rose",
        // 236..=239: oak_trapdoor variants
        236..=239 => "doors:trapdoor",
        // 240: quartz_slab
        240 => "stairs:slab_sandstone",
        // 241: dark_oak_trapdoor
        241 => "doors:trapdoor",
        // 242: spruce_trapdoor
        242 => "doors:trapdoor",
        // 243: birch_trapdoor
        243 => "doors:trapdoor",
        // 244: mud_brick_slab
        244 => "stairs:slab_stonebrick",
        // 245: brick_slab
        245 => "stairs:slab_brick",
        // 246: potted_red_tulip
        246 => "flowers:tulip",
        // 247: potted_dandelion
        247 => "flowers:dandelion_yellow",
        // 248: potted_blue_orchid
        248 => "flowers:viola",
        _ => "default:stone",
    };
    LuantiNode { name, param2: 0 }
}

fn to_mineclonia_node(block: Block) -> LuantiNode {
    let name = match block.id() {
        0 => "mcl_trees:wood_acacia",
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
        16 => "mcl_crimson:crimson_hyphae_wood",
        17 => "mcl_core:sandstonecarved",
        18 => "mcl_colorblocks:concrete_cyan",
        19 => "mcl_trees:wood_dark_oak",
        20 => "mcl_deepslate:deepslate_bricks",
        21 => "mcl_core:diorite",
        22 => "mcl_core:dirt",
        23 => "mcl_end:end_bricks",
        24 => "mcl_farming:soil_wet",
        25 => "mcl_core:glass",
        26 => "mcl_lanterns:lantern_floor",
        27 => "mcl_core:granite",
        28 => "mcl_core:dirt_with_grass",
        29 => "mcl_flowers:tallgrass",
        30 => "mcl_core:gravel",
        31 => "mcl_colorblocks:concrete_grey",
        32 => "mcl_colorblocks:hardened_clay_grey",
        33 => "mcl_colorblocks:hardened_clay_green",
        34 => "mcl_wool:green",
        35 => "mcl_farming:hay_block",
        36 => "mcl_iron_bars:iron_bars",
        37 => "mcl_core:ironblock",
        38 => "mcl_trees:wood_jungle",
        39 => "mcl_core:ladder",
        40 => "mcl_colorblocks:concrete_light_blue",
        41 => "mcl_colorblocks:hardened_clay_light_blue",
        42 => "mcl_colorblocks:concrete_silver",
        43 => "mcl_mangrove:moss",
        44 => "mcl_core:mossycobble",
        45 => "mcl_mud:mud_bricks",
        46 => "mcl_nether:nether_brick",
        47 => "mcl_nether:netherite_block",
        48 => "mcl_fences:fence",
        49 => "mcl_trees:leaves_oak",
        50 => "mcl_trees:tree_oak",
        51 => "mcl_trees:wood_oak",
        52 => "mcl_stairs:slab_wood_oak",
        53 => "mcl_colorblocks:hardened_clay_orange",
        54 => "mcl_core:podzol",
        55 => "mcl_core:andesite_smooth",
        56 => "mcl_core:basalt_polished",
        57 => "mcl_nether:quartz_block",
        58 => "mcl_blackstone:blackstone_polished",
        59 => "mcl_deepslate:deepslate_polished",
        60 => "mcl_core:diorite_smooth",
        61 => "mcl_core:granite_smooth",
        62 => "mcl_ocean:prismarine",
        63 => "mcl_end:purpur_block",
        64 => "mcl_end:purpur_pillar",
        65 => "mcl_nether:quartz_block",
        66 => "mcl_minecarts:rail",
        67 => "mcl_flowers:poppy",
        68 => "mcl_nether:red_nether_brick",
        69 => "mcl_colorblocks:hardened_clay_red",
        70 => "mcl_wool:red",
        71 => "mcl_core:sand",
        72 => "mcl_core:sandstone",
        73 => "mcl_scaffolding:scaffolding",
        74 => "mcl_core:quartz_smooth",
        75 => "mcl_core:redsandstone_smooth",
        76 => "mcl_core:sandstone_smooth",
        77 => "mcl_stairs:slab_stone_double",
        78 => "mcl_sponges:sponge",
        79 => "mcl_trees:tree_spruce",
        80 => "mcl_trees:wood_spruce",
        81 => "mcl_stairs:slab_stone",
        82 => "mcl_stairs:slab_stonebrick",
        83 => "mcl_core:stonebrick",
        84 => "mcl_core:stone",
        85 => "mcl_colorblocks:hardened_clay",
        86 => "mcl_crimson:warped_hyphae_wood",
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
        97 => "mcl_wool:cyan",
        98 => "mcl_colorblocks:concrete_blue",
        99 => "mcl_colorblocks:concrete_purple",
        100 => "mcl_colorblocks:concrete_red",
        101 => "mcl_colorblocks:concrete_magenta",
        102 => "mcl_wool:brown",
        103 => "mcl_copper:block_oxidized",
        104 => "mcl_colorblocks:hardened_clay_yellow",
        105 => "mcl_farming:carrot_7",
        106 => "mcl_doors:dark_oak_door_b_1",
        107 => "mcl_doors:dark_oak_door_t_1",
        108 => "mcl_farming:potato_4",
        109 => "mcl_farming:wheat_7",
        110 => "mcl_core:bedrock",
        111 => "mcl_core:snowblock",
        112 => "mcl_core:snow",
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
        133 => "mcl_core:ice",
        134 => "mcl_core:packed_ice",
        135 => "mcl_mud:mud",
        136 => "mcl_core:deadbush",
        137 => "mcl_flowers:double_grass",
        138 => "mcl_flowers:double_grass_top",
        139 => "mcl_crafting_table:crafting_table",
        140 => "mcl_furnaces:furnace",
        141 => "mcl_wool:white_carpet",
        142 => "mcl_books:bookshelf",
        143 => "mcl_core:wood_oak",
        144 => "mcl_stairs:stair_wood_oak",
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
        173 => "mcl_doors:trapdoor",
        174 => "mcl_colorblocks:concrete_brown",
        175 => "mcl_colorblocks:hardened_clay_black",
        176 => "mcl_colorblocks:hardened_clay_brown",
        177 => "mcl_stairs:stair_stonebrick",
        178 => "mcl_stairs:stair_mud_brick",
        179 => "mcl_stairs:stair_blackstone_brick_polished",
        180 => "mcl_stairs:stair_brick_block",
        181 => "mcl_stairs:stair_granite_smooth",
        182 => "mcl_stairs:stair_end_bricks",
        183 => "mcl_stairs:stair_diorite_smooth",
        184 => "mcl_stairs:stair_sandstonesmooth2",
        185 => "mcl_stairs:stair_quartzblock",
        186 => "mcl_stairs:stair_andesite_smooth",
        187 => "mcl_stairs:stair_nether_brick",
        188 => "mcl_barrels:barrel_closed",
        189 => "mcl_flowers:fern",
        190 => "mcl_core:cobweb",
        191..=194 => "mcl_books:bookshelf",
        195 => "mcl_anvils:anvil_damage_1",
        196 => "mcl_anvils:anvil_damage_2",
        197 => "mcl_flowers:double_fern",
        198 => "mcl_flowers:double_fern_top",
        199 => "mcl_core:chain",
        200 => "mcl_end:end_rod",
        201 => "mcl_copper:lightning_rod",
        202 => "mcl_core:goldblock",
        203 => "mcl_ocean:sea_lantern",
        204 => "mcl_colorblocks:concrete_orange",
        205 => "mcl_wool:orange",
        206 => "mcl_wool:blue",
        207 => "mcl_colorblocks:concrete_green",
        208 => "mcl_walls:brick_block",
        209 => "mcl_core:redstone_block",
        210..=211 => "mcl_core:chain",
        212 => "mcl_doors:spruce_door_b_1",
        213 => "mcl_doors:spruce_door_t_1",
        214 => "mcl_stairs:slab_stone_double",
        215 => "mcl_core:glass_pane_natural",
        216 => "mcl_colorblocks:hardened_clay_silver",
        217 => "mcl_stairs:slab_wood_oak",
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
        229 => "mcl_core:wood_oak",
        230 => "mcl_core:glass_red",
        231 => "mcl_core:glass_yellow",
        232 => "mcl_core:glass_purple",
        233 => "mcl_core:glass_orange",
        234 => "mcl_core:glass_magenta",
        235 => "mcl_flowers:poppy",
        236..=239 => "mcl_doors:trapdoor",
        240 => "mcl_stairs:slab_quartz_block",
        241 => "mcl_doors:dark_oak_trapdoor",
        242 => "mcl_doors:spruce_trapdoor",
        243 => "mcl_doors:birch_trapdoor",
        244 => "mcl_stairs:slab_mud_brick",
        245 => "mcl_stairs:slab_brick_block",
        246 => "mcl_flowers:tulip_red",
        247 => "mcl_flowers:dandelion",
        248 => "mcl_flowers:blue_orchid",
        _ => "mcl_core:stone",
    };
    LuantiNode { name, param2: 0 }
}
