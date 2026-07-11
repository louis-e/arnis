use crate::deterministic_rng::coord_rng;
use fastnbt::Value;
use rand::Rng;
use std::collections::HashMap;

// Rarity weights applied per item within its theme.
const COMMON: u32 = 9;
const UNCOMMON: u32 = 3;
const RARE: u32 = 1;

// Some rolls place nothing so chests are not always full.
const EMPTY_WEIGHT: u32 = 3;

const CHEST_SLOTS: usize = 27;

struct LootItem {
    id: &'static str,
    min: i32,
    max: i32,
    weight: u32,
}

struct Theme {
    weight: u32,
    items: &'static [LootItem],
}

// Stackable items use bigger counts; tools, armour and treasure stay single.
const THEMES: &[Theme] = &[
    // Food and kitchen.
    Theme {
        weight: 25,
        items: &[
            LootItem {
                id: "minecraft:bread",
                min: 2,
                max: 6,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:potato",
                min: 3,
                max: 9,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:carrot",
                min: 2,
                max: 7,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:wheat",
                min: 3,
                max: 9,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:apple",
                min: 2,
                max: 6,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:baked_potato",
                min: 2,
                max: 6,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:cooked_chicken",
                min: 1,
                max: 4,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:sweet_berries",
                min: 2,
                max: 7,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:beetroot",
                min: 2,
                max: 6,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:pumpkin_pie",
                min: 1,
                max: 3,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:mushroom_stew",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:golden_carrot",
                min: 1,
                max: 3,
                weight: RARE,
            },
            LootItem {
                id: "minecraft:cake",
                min: 1,
                max: 1,
                weight: RARE,
            },
        ],
    },
    // Junk and flavour.
    Theme {
        weight: 20,
        items: &[
            LootItem {
                id: "minecraft:paper",
                min: 2,
                max: 7,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:bone",
                min: 2,
                max: 7,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:string",
                min: 2,
                max: 7,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:rotten_flesh",
                min: 2,
                max: 6,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:book",
                min: 1,
                max: 4,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:dead_bush",
                min: 1,
                max: 3,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:gunpowder",
                min: 1,
                max: 4,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:flower_pot",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:cobweb",
                min: 1,
                max: 3,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:name_tag",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:map",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
        ],
    },
    // Building resources.
    Theme {
        weight: 18,
        items: &[
            LootItem {
                id: "minecraft:oak_planks",
                min: 4,
                max: 16,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:cobblestone",
                min: 6,
                max: 20,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:coal",
                min: 3,
                max: 9,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:clay_ball",
                min: 2,
                max: 7,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:glass_pane",
                min: 3,
                max: 9,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:torch",
                min: 2,
                max: 8,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:iron_ingot",
                min: 2,
                max: 6,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:candle",
                min: 1,
                max: 4,
                weight: UNCOMMON,
            },
        ],
    },
    // Tools and utility.
    Theme {
        weight: 15,
        items: &[
            LootItem {
                id: "minecraft:stick",
                min: 2,
                max: 7,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:bucket",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:fishing_rod",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:shears",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:flint_and_steel",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:compass",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:iron_pickaxe",
                min: 1,
                max: 1,
                weight: RARE,
            },
            LootItem {
                id: "minecraft:iron_axe",
                min: 1,
                max: 1,
                weight: RARE,
            },
            LootItem {
                id: "minecraft:clock",
                min: 1,
                max: 1,
                weight: RARE,
            },
        ],
    },
    // Valuables and treasure.
    Theme {
        weight: 12,
        items: &[
            LootItem {
                id: "minecraft:iron_nugget",
                min: 3,
                max: 8,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:gold_nugget",
                min: 2,
                max: 7,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:lapis_lazuli",
                min: 2,
                max: 6,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:emerald",
                min: 1,
                max: 4,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:gold_ingot",
                min: 1,
                max: 3,
                weight: RARE,
            },
            LootItem {
                id: "minecraft:amethyst_shard",
                min: 1,
                max: 4,
                weight: RARE,
            },
            LootItem {
                id: "minecraft:diamond",
                min: 1,
                max: 2,
                weight: RARE,
            },
        ],
    },
    // Adventure gear.
    Theme {
        weight: 10,
        items: &[
            LootItem {
                id: "minecraft:arrow",
                min: 3,
                max: 12,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:leather_boots",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:leather_chestplate",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:shield",
                min: 1,
                max: 1,
                weight: RARE,
            },
            LootItem {
                id: "minecraft:golden_apple",
                min: 1,
                max: 1,
                weight: RARE,
            },
            LootItem {
                id: "minecraft:ender_pearl",
                min: 1,
                max: 3,
                weight: RARE,
            },
        ],
    },
];

fn pick_item(theme: &Theme, rng: &mut impl Rng) -> &'static LootItem {
    let total: u32 = theme.items.iter().map(|i| i.weight).sum();
    let mut pick = rng.random_range(0..total);
    for item in theme.items {
        if pick < item.weight {
            return item;
        }
        pick -= item.weight;
    }
    &theme.items[theme.items.len() - 1]
}

/// Deterministic per-chest loot keyed on world coords; a few scattered stacks per chest.
pub fn chest_loot(x: i32, z: i32, salt: u32) -> Vec<HashMap<String, Value>> {
    let mut rng = coord_rng(x, z, salt as u64 ^ 0x1007_C0DE);
    let rolls = rng.random_range(3..=8);
    let theme_total: u32 = THEMES.iter().map(|t| t.weight).sum::<u32>() + EMPTY_WEIGHT;

    let mut used = [false; CHEST_SLOTS];
    let mut out = Vec::new();

    for _ in 0..rolls {
        let mut pick = rng.random_range(0..theme_total);
        if pick < EMPTY_WEIGHT {
            continue;
        }
        pick -= EMPTY_WEIGHT;

        let mut chosen = &THEMES[0];
        for theme in THEMES {
            if pick < theme.weight {
                chosen = theme;
                break;
            }
            pick -= theme.weight;
        }

        let item = pick_item(chosen, &mut rng);
        let count = rng.random_range(item.min..=item.max);

        let mut slot = None;
        for _ in 0..4 {
            let candidate = rng.random_range(0..CHEST_SLOTS);
            if !used[candidate] {
                slot = Some(candidate);
                break;
            }
        }
        let Some(slot) = slot else { continue };
        used[slot] = true;

        // 1.20.5+ container item format: lowercase count (Int), matching the map chest.
        let mut item_nbt = HashMap::new();
        item_nbt.insert("id".to_string(), Value::String(item.id.to_string()));
        item_nbt.insert("Slot".to_string(), Value::Byte(slot as i8));
        item_nbt.insert("count".to_string(), Value::Int(count));
        out.push(item_nbt);
    }

    out
}
