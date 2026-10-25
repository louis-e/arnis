//! Java block entities and entities, rewritten for Bedrock or left out.

use crate::bedrock_block_map::{BedrockBlock, BedrockBlockStateValue, BLOCK_STATE_VERSION};
use crate::block_definitions::Block;
use fastnbt::Value;
use std::collections::HashMap;

type Compound = HashMap<String, Value>;

/// The Bedrock form of a Java block entity; `block` at its position gives a bed's colour.
pub(super) fn block_entity(java: &Compound, block: Option<Block>) -> Option<Compound> {
    let id = string(java.get("id")?)?;
    let bedrock_id = match id {
        "minecraft:chest" => "Chest",
        "minecraft:barrel" => "Barrel",
        "minecraft:bed" => "Bed",
        // Written in Bedrock form already, e.g. banners.
        other if !other.contains(':') => other,
        // A Java-only entity such as a sign; the game makes a default one where needed.
        _ => return None,
    };

    let mut out = Compound::new();
    out.insert("id".to_string(), Value::String(bedrock_id.to_string()));
    for key in ["x", "y", "z"] {
        out.insert(key.to_string(), java.get(key)?.clone());
    }
    out.insert("isMovable".to_string(), Value::Byte(1));

    match bedrock_id {
        "Chest" | "Barrel" => {
            let items = match java.get("Items") {
                Some(Value::List(items)) => items
                    .iter()
                    .filter_map(|item| match item {
                        Value::Compound(item) => item_stack(item),
                        _ => None,
                    })
                    .map(Value::Compound)
                    .collect(),
                _ => Vec::new(),
            };
            out.insert("Items".to_string(), Value::List(items));
            out.insert("Findable".to_string(), Value::Byte(0));
        }
        "Bed" => {
            let colour = block.map_or(14, |b| dye_index(b.name().trim_end_matches("_bed")));
            out.insert("color".to_string(), Value::Byte(colour));
        }
        _ => {
            // Already Bedrock: keep every tag as it is.
            for (key, value) in java {
                out.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }
    Some(out)
}

/// A Java item frame as Bedrock's frame block and block entity, with its position.
pub(super) fn item_frame(java: &Compound) -> Option<((i32, i32, i32), BedrockBlock, Compound)> {
    let (block_name, entity_id) = match string(java.get("id")?)? {
        "minecraft:item_frame" => ("frame", "ItemFrame"),
        "minecraft:glow_item_frame" => ("glow_frame", "GlowItemFrame"),
        _ => return None,
    };
    let pos = (
        int(java.get("TileX")?)?,
        int(java.get("TileY")?)?,
        int(java.get("TileZ")?)?,
    );
    // Java's Facing and Bedrock's facing_direction share the order down, up, N, S, W, E.
    let facing = java.get("Facing").and_then(int).unwrap_or(2).clamp(0, 5);
    let block = BedrockBlock::with_states(
        block_name,
        vec![
            ("facing_direction", BedrockBlockStateValue::Int(facing)),
            ("item_frame_map_bit", BedrockBlockStateValue::Bool(false)),
            ("item_frame_photo_bit", BedrockBlockStateValue::Bool(false)),
        ],
    );

    let mut entity = Compound::new();
    entity.insert("id".to_string(), Value::String(entity_id.to_string()));
    entity.insert("x".to_string(), Value::Int(pos.0));
    entity.insert("y".to_string(), Value::Int(pos.1));
    entity.insert("z".to_string(), Value::Int(pos.2));
    entity.insert("isMovable".to_string(), Value::Byte(1));
    if let Some(Value::Compound(item)) = java.get("Item") {
        if let Some(item) = item_stack(item) {
            entity.insert("Item".to_string(), Value::Compound(item));
        }
    }
    // Java turns in eighths, Bedrock in degrees.
    let rotation = java.get("ItemRotation").and_then(int).unwrap_or(0);
    entity.insert(
        "ItemRotation".to_string(),
        Value::Float(rotation.rem_euclid(8) as f32 * 45.0),
    );
    let drop_chance = match java.get("ItemDropChance") {
        Some(Value::Float(f)) => *f,
        _ => 1.0,
    };
    entity.insert("ItemDropChance".to_string(), Value::Float(drop_chance));
    Some((pos, block, entity))
}

/// The FlowerPot block entity holding `plant`.
pub(super) fn flower_pot((x, y, z): (i32, i32, i32), plant: &BedrockBlock) -> Compound {
    let states = plant
        .states
        .iter()
        .map(|(key, value)| (key.clone(), Value::from(value)))
        .collect();
    let plant_block = Compound::from([
        ("name".to_string(), Value::String(plant.name.clone())),
        ("states".to_string(), Value::Compound(states)),
        ("version".to_string(), Value::Int(BLOCK_STATE_VERSION)),
    ]);
    Compound::from([
        ("id".to_string(), Value::String("FlowerPot".to_string())),
        ("x".to_string(), Value::Int(x)),
        ("y".to_string(), Value::Int(y)),
        ("z".to_string(), Value::Int(z)),
        ("isMovable".to_string(), Value::Byte(1)),
        ("PlantBlock".to_string(), Value::Compound(plant_block)),
    ])
}

/// A Java item stack, either the component format or the older `Count`/`tag` one.
fn item_stack(java: &Compound) -> Option<Compound> {
    let id = string(java.get("id")?)?;
    let count = java
        .get("count")
        .or_else(|| java.get("Count"))
        .and_then(int)
        .unwrap_or(1)
        .clamp(1, 127);

    let mut out = Compound::new();
    out.insert("Name".to_string(), Value::String(item_name(id).to_string()));
    out.insert("Count".to_string(), Value::Byte(count as i8));
    out.insert("Damage".to_string(), Value::Short(0));
    out.insert("WasPickedUp".to_string(), Value::Byte(0));
    if let Some(Value::Byte(slot)) = java.get("Slot") {
        out.insert("Slot".to_string(), Value::Byte(*slot));
    }

    let components = match java.get("components") {
        Some(Value::Compound(c)) => Some(c),
        _ => None,
    };
    let legacy = match java.get("tag") {
        Some(Value::Compound(t)) => Some(t),
        _ => None,
    };
    let mut tag = Compound::new();
    let damage = components
        .and_then(|c| c.get("minecraft:damage"))
        .or_else(|| legacy.and_then(|t| t.get("Damage")))
        .and_then(int);
    if let Some(damage) = damage.filter(|d| *d > 0) {
        tag.insert("Damage".to_string(), Value::Int(damage));
    }
    let colour = components
        .and_then(|c| c.get("minecraft:dyed_color"))
        .and_then(|v| match v {
            Value::Compound(c) => c.get("rgb").and_then(int),
            other => int(other),
        })
        .or_else(|| {
            legacy.and_then(|t| t.get("display")).and_then(|d| match d {
                Value::Compound(d) => d.get("color").and_then(int),
                _ => None,
            })
        });
    if let Some(rgb) = colour {
        // Bedrock keeps an opaque ARGB colour.
        tag.insert(
            "customColor".to_string(),
            Value::Int((0xFF00_0000u32 | rgb as u32) as i32),
        );
    }
    if !tag.is_empty() {
        out.insert("tag".to_string(), Value::Compound(tag));
    }
    Some(out)
}

/// The Bedrock name of a Java item Arnis places.
fn item_name(java_id: &str) -> &str {
    match java_id {
        "minecraft:map" => "minecraft:empty_map",
        "minecraft:cobweb" => "minecraft:web",
        "minecraft:dead_bush" => "minecraft:deadbush",
        "minecraft:item_frame" => "minecraft:frame",
        "minecraft:glow_item_frame" => "minecraft:glow_frame",
        other => other,
    }
}

/// Bedrock's bed colours, in dye order.
fn dye_index(colour: &str) -> i8 {
    match colour {
        "white" => 0,
        "orange" => 1,
        "magenta" => 2,
        "light_blue" => 3,
        "yellow" => 4,
        "lime" => 5,
        "pink" => 6,
        "gray" => 7,
        "light_gray" => 8,
        "cyan" => 9,
        "purple" => 10,
        "blue" => 11,
        "brown" => 12,
        "green" => 13,
        "black" => 15,
        _ => 14,
    }
}

fn string(value: &Value) -> Option<&str> {
    match value {
        Value::String(s) => Some(s.as_str()),
        _ => None,
    }
}

/// Any numeric tag, or a number in a string, as an i32; floats are cut toward zero.
pub(super) fn int(value: &Value) -> Option<i32> {
    match value {
        Value::Byte(v) => Some(i32::from(*v)),
        Value::Short(v) => Some(i32::from(*v)),
        Value::Int(v) => Some(*v),
        Value::Long(v) => i32::try_from(*v).ok(),
        Value::Float(v) => Some(*v as i32),
        Value::Double(v) => Some(*v as i32),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compound(entries: Vec<(&str, Value)>) -> Compound {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    fn java_chest() -> Compound {
        let apple = compound(vec![
            ("id", Value::String("minecraft:map".into())),
            ("count", Value::Int(3)),
            ("Slot", Value::Byte(4)),
        ]);
        compound(vec![
            ("id", Value::String("minecraft:chest".into())),
            ("x", Value::Int(1)),
            ("y", Value::Int(70)),
            ("z", Value::Int(-2)),
            ("Items", Value::List(vec![Value::Compound(apple)])),
            ("keepPacked", Value::Byte(0)),
        ])
    }

    #[test]
    fn chest_items_take_bedrock_names_and_counts() {
        let chest = block_entity(&java_chest(), None).unwrap();
        assert_eq!(chest.get("id"), Some(&Value::String("Chest".into())));
        assert!(!chest.contains_key("keepPacked"));
        let Some(Value::List(items)) = chest.get("Items") else {
            panic!("no items");
        };
        let Value::Compound(item) = &items[0] else {
            panic!("item is not a compound");
        };
        assert_eq!(
            item.get("Name"),
            Some(&Value::String("minecraft:empty_map".into()))
        );
        assert_eq!(item.get("Count"), Some(&Value::Byte(3)));
        assert_eq!(item.get("Slot"), Some(&Value::Byte(4)));
        assert_eq!(item.get("Damage"), Some(&Value::Short(0)));
    }

    #[test]
    fn a_bed_takes_its_colour_from_the_block() {
        let java = compound(vec![
            ("id", Value::String("minecraft:bed".into())),
            ("x", Value::Int(0)),
            ("y", Value::Int(0)),
            ("z", Value::Int(0)),
        ]);
        let white = block_entity(&java, Some(crate::block_definitions::WHITE_BED)).unwrap();
        assert_eq!(white.get("color"), Some(&Value::Byte(0)));
        let red = block_entity(&java, None).unwrap();
        assert_eq!(red.get("color"), Some(&Value::Byte(14)));
    }

    #[test]
    fn java_only_block_entities_are_left_out_and_bedrock_ones_kept() {
        let sign = compound(vec![
            ("id", Value::String("minecraft:sign".into())),
            ("x", Value::Int(0)),
            ("y", Value::Int(0)),
            ("z", Value::Int(0)),
        ]);
        assert!(block_entity(&sign, None).is_none());

        let banner = compound(vec![
            ("id", Value::String("Banner".into())),
            ("x", Value::Int(0)),
            ("y", Value::Int(0)),
            ("z", Value::Int(0)),
            ("Base", Value::Int(7)),
        ]);
        let kept = block_entity(&banner, None).unwrap();
        assert_eq!(kept.get("Base"), Some(&Value::Int(7)));
    }

    #[test]
    fn an_item_frame_becomes_a_frame_block_with_its_item() {
        let item = compound(vec![
            ("id", Value::String("minecraft:paper".into())),
            ("Count", Value::Byte(2)),
        ]);
        let java = compound(vec![
            ("id", Value::String("minecraft:item_frame".into())),
            ("TileX", Value::Int(5)),
            ("TileY", Value::Int(64)),
            ("TileZ", Value::Int(9)),
            ("Facing", Value::Byte(4)),
            ("ItemRotation", Value::Byte(2)),
            ("Item", Value::Compound(item)),
        ]);
        let (pos, block, entity) = item_frame(&java).unwrap();
        assert_eq!(pos, (5, 64, 9));
        assert_eq!(block.name, "minecraft:frame");
        assert_eq!(
            block.states.get("facing_direction"),
            Some(&BedrockBlockStateValue::Int(4))
        );
        assert_eq!(entity.get("id"), Some(&Value::String("ItemFrame".into())));
        assert_eq!(entity.get("ItemRotation"), Some(&Value::Float(90.0)));
        let Some(Value::Compound(item)) = entity.get("Item") else {
            panic!("no item");
        };
        assert_eq!(item.get("Count"), Some(&Value::Byte(2)));
    }

    #[test]
    fn worn_dyed_armour_keeps_damage_and_colour() {
        let java = compound(vec![
            ("id", Value::String("minecraft:leather_chestplate".into())),
            ("Count", Value::Byte(1)),
            (
                "tag",
                Value::Compound(compound(vec![
                    ("Damage", Value::Int(12)),
                    (
                        "display",
                        Value::Compound(compound(vec![("color", Value::Int(0x123456))])),
                    ),
                ])),
            ),
        ]);
        let item = item_stack(&java).unwrap();
        let Some(Value::Compound(tag)) = item.get("tag") else {
            panic!("no tag");
        };
        assert_eq!(tag.get("Damage"), Some(&Value::Int(12)));
        assert_eq!(
            tag.get("customColor"),
            Some(&Value::Int(0xFF12_3456u32 as i32))
        );
    }
}
