//! Macaw's Furniture, Roofs, Lights, Doors, and Paths and Pavings.
//!
//! Each piece's `facing` follows its own model, which is not the same rule across the
//! set; the helpers take the way the piece should face and work the state out.

use super::{dir_name, state};
use crate::block_definitions::*;

pub fn ccw(d: (i32, i32)) -> (i32, i32) {
    (d.1, -d.0)
}

fn back(d: (i32, i32)) -> (i32, i32) {
    (-d.0, -d.1)
}

/// The Macaw's name of a vanilla wood, from any of its blocks.
pub fn wood_name(block: Block) -> Option<&'static str> {
    let name = block.name();
    [
        "dark_oak", "oak", "spruce", "birch", "jungle", "acacia", "mangrove", "cherry", "crimson",
        "warped",
    ]
    .into_iter()
    .find(|&wood| name.starts_with(wood) && !(wood == "oak" && name.starts_with("dark_oak")))
}

/// A chair, its sitter looking along `look`.
pub fn chair(wood: &str, look: (i32, i32), fallback: Block) -> BlockWithProperties {
    state(
        &format!("mcwfurnitures:{wood}_chair"),
        fallback,
        &[("facing", dir_name(look))],
    )
}

/// A table; neighbouring tables join into one top.
pub fn table(wood: &str, fallback: Block) -> BlockWithProperties {
    state(&format!("mcwfurnitures:{wood}_table"), fallback, &[])
}

pub fn coffee_table(wood: &str, fallback: Block) -> BlockWithProperties {
    state(&format!("mcwfurnitures:{wood}_coffee_table"), fallback, &[])
}

/// A desk in a row running along x or z.
pub fn desk(wood: &str, along_x: bool, covered: bool, fallback: Block) -> BlockWithProperties {
    let kind = if covered { "covered_desk" } else { "desk" };
    state(
        &format!("mcwfurnitures:{wood}_{kind}"),
        fallback,
        &[("facing", if along_x { "north" } else { "east" })],
    )
}

/// A kitchen counter showing its doors along `front`.
pub fn counter(wood: &str, front: (i32, i32), fallback: Block) -> BlockWithProperties {
    state(
        &format!("mcwfurnitures:{wood}_counter"),
        fallback,
        &[("facing", dir_name(back(front)))],
    )
}

pub fn drawer_counter(wood: &str, front: (i32, i32), fallback: Block) -> BlockWithProperties {
    state(
        &format!("mcwfurnitures:{wood}_drawer_counter"),
        fallback,
        &[("facing", dir_name(back(front)))],
    )
}

pub fn sink(wood: &str, front: (i32, i32), fallback: Block) -> BlockWithProperties {
    state(
        &format!("mcwfurnitures:{wood}_kitchen_sink"),
        fallback,
        &[("facing", dir_name(back(front)))],
    )
}

/// A tall kitchen cabinet with one door.
pub fn cabinet(wood: &str, front: (i32, i32), fallback: Block) -> BlockWithProperties {
    state(
        &format!("mcwfurnitures:{wood}_kitchen_cabinet"),
        fallback,
        &[("facing", dir_name(back(front))), ("hinge", "left")],
    )
}

/// One storey of a wardrobe; `top` is the upper block of the two.
pub fn wardrobe(wood: &str, front: (i32, i32), top: bool, fallback: Block) -> BlockWithProperties {
    let mut props = vec![
        ("facing", dir_name(ccw(front))),
        ("connection", if top { "top" } else { "bottom" }),
    ];
    if !top {
        props.push(("hinge", "left"));
    }
    state(&format!("mcwfurnitures:{wood}_wardrobe"), fallback, &props)
}

pub fn drawer(wood: &str, front: (i32, i32), fallback: Block) -> BlockWithProperties {
    state(
        &format!("mcwfurnitures:{wood}_drawer"),
        fallback,
        &[("facing", dir_name(ccw(front)))],
    )
}

/// A sofa seat of the given dye, its sitter looking along `look`.
pub fn couch(color: &str, look: (i32, i32), fallback: Block) -> BlockWithProperties {
    state(
        &format!("mcwfurnitures:{color}_couch"),
        fallback,
        &[("facing", dir_name(ccw(look)))],
    )
}

/// The look of an entrance door.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DoorKind {
    /// A panelled house door.
    Panel,
    /// A rustic planked door.
    Cottage,
    /// A wooden frame around a glass pane.
    Glass,
    /// A wooden door with small lights, for old and sacred buildings.
    Classic,
    /// A shop's glazed door.
    Store,
    /// The glass doors of a tower lobby.
    Sliding,
    /// A steel door with a window, for works and depots.
    Metal,
}

/// A Macaw's entrance door of `kind` in `wood` (oak, spruce, dark_oak or birch). Woods
/// without that kind get their panel door, or the classic door where they lack that too.
pub fn door(kind: DoorKind, wood: &str, fallback: Block) -> Block {
    let has = |k: &str| match (wood, k) {
        ("dark_oak", "four_panel") | ("spruce", "cottage") | ("oak", "classic") => false,
        ("oak" | "spruce" | "dark_oak" | "birch", _) => true,
        _ => false,
    };
    let wooden = |k: &str| {
        let k = [k, "four_panel", "classic"]
            .into_iter()
            .find(|&k| has(k))
            .unwrap_or("glass");
        format!("mcwdoors:{wood}_{k}_door")
    };
    let name = match kind {
        DoorKind::Panel => wooden("four_panel"),
        DoorKind::Cottage => wooden("cottage"),
        DoorKind::Glass => wooden("glass"),
        DoorKind::Classic => wooden("classic"),
        DoorKind::Store => "mcwdoors:store_door".to_string(),
        DoorKind::Sliding => "mcwdoors:sliding_glass_door".to_string(),
        DoorKind::Metal => "mcwdoors:metal_windowed_door".to_string(),
    };
    Block::modded(&name, fallback)
}

/// Patterned paving for a pedestrian way or square of OSM `surface`, or None to keep the
/// vanilla surface. `sidewalk` keeps one plain pattern along a road; streets and squares
/// take one of a few patterns by `seed`. Only the almost full blocks of the mod: its
/// `*_paving` tiles are thin overlays that need a floor under them.
pub fn paving(surface: Option<&str>, sidewalk: bool, seed: u64) -> Option<Block> {
    let (name, fallback) = match surface {
        Some("paving_stones") | Some("concrete:plates") if sidewalk => {
            ("stone_running_bond", SMOOTH_STONE)
        }
        // A pedestrian street is paved even where its surface is not mapped.
        Some("paving_stones") | Some("concrete:plates") | None => match seed % 3 {
            0 => ("andesite_windmill_weave", POLISHED_ANDESITE),
            1 => ("stone_windmill_weave", SMOOTH_STONE),
            _ => ("andesite_running_bond", POLISHED_ANDESITE),
        },
        Some("sett") => ("andesite_running_bond", POLISHED_ANDESITE),
        Some("bricks" | "brick") if sidewalk => ("brick_running_bond", BRICK),
        Some("bricks" | "brick") => ("brick_windmill_weave", BRICK),
        Some("stone" | "flagstone") => ("stone_flagstone", STONE_BRICKS),
        _ => return None,
    };
    Some(Block::modded(&format!("mcwpaths:{name}"), fallback))
}

/// The Macaw's roof tile for a stair on a roof of `roof`, keeping the stair's slope, or
/// None where the mod has no tile of that material.
pub fn roof_tile(roof: Block, stair: &BlockWithProperties) -> Option<BlockWithProperties> {
    let material = roof_material(roof).or_else(|| roof_material(stair.block))?;
    let props = match stair.properties.as_deref() {
        Some(fastnbt::Value::Compound(map)) => map,
        _ => return None,
    };
    let get = |k: &str| match props.get(k) {
        Some(fastnbt::Value::String(v)) => Some(v.as_str()),
        _ => None,
    };
    Some(state(
        &format!("mcwroofs:{material}_roof"),
        stair.block,
        &[
            ("facing", get("facing")?),
            ("half", get("half").unwrap_or("bottom")),
            ("shape", get("shape").unwrap_or("straight")),
        ],
    ))
}

fn roof_material(block: Block) -> Option<&'static str> {
    const COLOURS: [&str; 16] = [
        "white",
        "orange",
        "magenta",
        "light_blue",
        "yellow",
        "lime",
        "pink",
        "gray",
        "light_gray",
        "cyan",
        "purple",
        "blue",
        "brown",
        "green",
        "red",
        "black",
    ];
    let n = block.name();
    let base = n.strip_suffix("_stairs").unwrap_or(n);
    for c in COLOURS {
        if base == format!("{c}_terracotta") {
            return Some(leak_material(format!("{c}_terracotta")));
        }
        if base == format!("{c}_concrete") {
            return Some(leak_material(format!("{c}_concrete")));
        }
    }
    Some(match base {
        "terracotta" => "brown_terracotta",
        "brick" | "bricks" => "bricks",
        "stone_brick" | "stone_bricks" | "mossy_stone_brick" | "mossy_stone_bricks" => {
            "stone_bricks"
        }
        "stone" | "smooth_stone" => "stone",
        "cobblestone" | "mossy_cobblestone" => "cobblestone",
        "oak" | "oak_planks" => "oak",
        "spruce" | "spruce_planks" => "spruce",
        "birch" | "birch_planks" => "birch",
        "jungle" | "jungle_planks" => "jungle",
        "acacia" | "acacia_planks" => "acacia",
        "dark_oak" | "dark_oak_planks" => "dark_oak",
        "mangrove" | "mangrove_planks" => "mangrove",
        "cherry" | "cherry_planks" => "cherry",
        "deepslate_brick" | "deepslate_bricks" | "deepslate_tile" | "deepslate_tiles"
        | "polished_deepslate" | "cobbled_deepslate" => "deepslate",
        "blackstone"
        | "polished_blackstone"
        | "polished_blackstone_brick"
        | "polished_blackstone_bricks" => "blackstone",
        "granite" | "polished_granite" => "granite",
        "diorite" | "polished_diorite" => "diorite",
        "andesite" | "polished_andesite" => "andesite",
        "sandstone" | "smooth_sandstone" | "cut_sandstone" => "sandstone",
        "red_sandstone" | "smooth_red_sandstone" | "cut_red_sandstone" => "red_sandstone",
        "nether_brick" | "nether_bricks" => "nether_bricks",
        "red_nether_brick" | "red_nether_bricks" => "red_nether_bricks",
        "mud_brick" | "mud_bricks" => "mud_brick",
        "prismarine_brick" | "prismarine_bricks" => "prismarine_brick",
        "dark_prismarine" => "dark_prismarine",
        _ => return None,
    })
}

// Colour names are a fixed set of 32, so leaking each at most once is bounded.
fn leak_material(s: String) -> &'static str {
    static SEEN: once_cell::sync::Lazy<std::sync::Mutex<Vec<&'static str>>> =
        once_cell::sync::Lazy::new(Default::default);
    let mut seen = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(&m) = seen.iter().find(|&&m| m == s) {
        return m;
    }
    let m: &'static str = Box::leak(s.into_boxed_str());
    seen.push(m);
    m
}

/// A chandelier for a tall hall.
pub fn chandelier(x: i32, z: i32) -> BlockWithProperties {
    let metal = if (x ^ z) & 8 == 0 { "iron" } else { "golden" };
    state(&format!("mcwlights:{metal}_chandelier"), LANTERN, &[])
}

/// A lamp hanging flush from the ceiling, tinted to suit the walls.
pub fn ceiling_light(wall: Block) -> BlockWithProperties {
    let name = wall.name();
    let colour = if name.contains("dark") || name.contains("black") || name.contains("deepslate") {
        "light_gray"
    } else if name.contains("brick") || name.contains("terracotta") {
        "orange"
    } else {
        "white"
    };
    state(
        &format!("mcwlights:{colour}_ceiling_light"),
        GLOWSTONE,
        &[("part", "base"), ("lit", "true")],
    )
}

/// One segment of a street lamp `height` blocks tall, `i` counting from the ground.
pub fn street_lamp(i: i32, height: i32, double: Option<(i32, i32)>) -> BlockWithProperties {
    let part = match i {
        _ if height <= 1 => "base",
        0 => "bottom",
        _ if i == height - 1 => "top",
        _ => "middle",
    };
    match double {
        Some(arm) => state(
            "mcwlights:double_street_lamp",
            if part == "top" { LANTERN } else { IRON_BARS },
            &[("facing", dir_name(arm)), ("part", part), ("lit", "true")],
        ),
        None => state(
            "mcwlights:classic_street_lamp",
            if part == "top" { LANTERN } else { IRON_BARS },
            &[("part", part), ("lit", "true")],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wood_names_tell_oak_from_dark_oak() {
        assert_eq!(wood_name(OAK_PLANKS), Some("oak"));
        assert_eq!(wood_name(DARK_OAK_SLAB), Some("dark_oak"));
        assert_eq!(wood_name(SPRUCE_STAIRS), Some("spruce"));
        assert_eq!(wood_name(STONE), None);
    }

    #[test]
    fn turns_are_quarter_turns() {
        assert_eq!(ccw((0, -1)), (-1, 0));
        assert_eq!(back(ccw((1, 0))), (0, 1));
    }

    #[test]
    fn doors_fall_back_to_a_kind_their_wood_has() {
        let name = |kind, wood| {
            let b = door(kind, wood, OAK_DOOR);
            format!("{}:{}", b.namespace(), b.name())
        };
        assert_eq!(name(DoorKind::Panel, "oak"), "mcwdoors:oak_four_panel_door");
        assert_eq!(
            name(DoorKind::Panel, "dark_oak"),
            "mcwdoors:dark_oak_classic_door"
        );
        assert_eq!(
            name(DoorKind::Cottage, "spruce"),
            "mcwdoors:spruce_four_panel_door"
        );
        assert_eq!(
            name(DoorKind::Classic, "oak"),
            "mcwdoors:oak_four_panel_door"
        );
        assert_eq!(name(DoorKind::Store, "birch"), "mcwdoors:store_door");
        assert_eq!(door(DoorKind::Glass, "oak", OAK_DOOR).vanilla(), OAK_DOOR);
    }

    #[test]
    fn paving_keeps_to_paved_surfaces() {
        let name = |s, sidewalk| paving(s, sidewalk, 7).map(|b| b.name().to_string());
        assert_eq!(
            name(Some("paving_stones"), true).as_deref(),
            Some("stone_running_bond")
        );
        assert_eq!(
            name(Some("sett"), false).as_deref(),
            Some("andesite_running_bond")
        );
        assert_eq!(
            name(Some("bricks"), false).as_deref(),
            Some("brick_windmill_weave")
        );
        assert!(name(None, false).is_some(), "a pedestrian street is paved");
        assert_eq!(name(Some("asphalt"), false), None);
        assert_eq!(name(Some("grass"), false), None);
    }
}
