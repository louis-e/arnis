//! Chest loot for building interiors. The built-in table below can be replaced at run
//! time with `--loot-table <file.json>`; `--dump-loot-table <file.json>` writes the
//! built-in one out in that format as a starting point.

use crate::deterministic_rng::coord_rng;
use fastnbt::Value;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

const CHEST_SLOTS: usize = 27;

/// The built-in table, in the format `--loot-table` reads. Items are weighted
/// common 9, uncommon 3, rare 1; stackables come in bigger counts, tools,
/// armour and treasure single.
pub const BUILT_IN_JSON: &str = include_str!("buildings_loot.json");

/// What a chest mostly holds. `Mixed` is the household spread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LootTheme {
    #[default]
    Mixed,
    Food,
    Resources,
    Tools,
    Valuables,
}

/// Weight multiplier for the favoured theme.
const FAVOURED_FACTOR: u32 = 6;

/// One weighted item of a theme, as read from a `--loot-table` file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LootEntry {
    pub id: String,
    pub min: i32,
    pub max: i32,
    pub weight: u32,
}

/// A group of items picked together, weighted against the other groups. `kind` is
/// the theme a shop, store or vault favours it for; a group without one is mixed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LootGroup {
    #[serde(default)]
    pub kind: LootTheme,
    pub weight: u32,
    pub items: Vec<LootEntry>,
}

impl LootGroup {
    /// This group's weight in a chest leaning towards `theme`.
    fn weight_for(&self, theme: LootTheme) -> u64 {
        if theme != LootTheme::Mixed && self.kind == theme {
            self.weight as u64 * FAVOURED_FACTOR as u64
        } else {
            self.weight as u64
        }
    }
}

/// A whole chest loot table: how many stacks to roll and what they can be.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LootTable {
    pub empty_weight: u32,
    pub rolls_min: u32,
    pub rolls_max: u32,
    pub themes: Vec<LootGroup>,
}

impl LootTable {
    /// Rejects anything `themed_chest_loot` could not roll from without panicking.
    fn validate(&self) -> Result<(), String> {
        if self.rolls_min > self.rolls_max || self.rolls_max > 64 {
            return Err("rolls must satisfy rolls_min <= rolls_max <= 64".to_string());
        }
        if self.themes.is_empty() {
            return Err("at least one theme is required".to_string());
        }
        for (ti, theme) in self.themes.iter().enumerate() {
            if theme.items.iter().map(|i| i.weight as u64).sum::<u64>() == 0 {
                return Err(format!("theme {ti} has no item with a weight above 0"));
            }
            for (ii, item) in theme.items.iter().enumerate() {
                if !item.id.contains(':') {
                    return Err(format!(
                        "theme {ti} item {ii}: '{}' is not a namespaced id like minecraft:bread",
                        item.id
                    ));
                }
                if item.min < 0 || item.min > item.max || item.max > 64 {
                    return Err(format!(
                        "theme {ti} item {ii}: count must satisfy 0 <= min <= max <= 64"
                    ));
                }
            }
        }
        // Every theme a chest can lean towards, favoured weights included.
        for favoured in [
            LootTheme::Mixed,
            LootTheme::Food,
            LootTheme::Resources,
            LootTheme::Tools,
            LootTheme::Valuables,
        ] {
            if self.theme_total(favoured).is_none() {
                return Err(format!(
                    "theme weights plus empty_weight must be between 1 and 2^32-1 \
                     (also with {favoured:?} themes weighted {FAVOURED_FACTOR}x)"
                ));
            }
        }
        Ok(())
    }

    /// Sum of all group weights and `empty_weight` for a chest leaning towards
    /// `theme`; `None` when it is 0 or does not fit a u32.
    fn theme_total(&self, theme: LootTheme) -> Option<u32> {
        let total =
            self.themes.iter().map(|t| t.weight_for(theme)).sum::<u64>() + self.empty_weight as u64;
        u32::try_from(total).ok().filter(|&t| t > 0)
    }
}

static BUILT_IN: LazyLock<Arc<LootTable>> = LazyLock::new(|| {
    Arc::new(serde_json::from_str(BUILT_IN_JSON).expect("built-in loot table parses"))
});

// Replaceable rather than set-once: the GUI runs several generations in one process.
static ACTIVE: RwLock<Option<Arc<LootTable>>> = RwLock::new(None);

/// Reads and checks a `--loot-table` file.
pub fn load_loot_table(path: &Path) -> Result<LootTable, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let table: LootTable = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    table.validate()?;
    Ok(table)
}

/// Sets the table chests roll from for the next generation; `None` restores the built-in one.
pub fn set_loot_table(table: Option<LootTable>) {
    *ACTIVE.write().unwrap_or_else(PoisonError::into_inner) = table.map(Arc::new);
}

fn active_table() -> Arc<LootTable> {
    ACTIVE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .unwrap_or_else(|| BUILT_IN.clone())
}

fn pick_item<'a>(theme: &'a LootGroup, rng: &mut impl Rng) -> &'a LootEntry {
    let total: u32 = theme.items.iter().map(|i| i.weight).sum();
    let mut pick = rng.random_range(0..total);
    for item in &theme.items {
        if pick < item.weight {
            return item;
        }
        pick -= item.weight;
    }
    &theme.items[theme.items.len() - 1]
}

/// Deterministic per-chest loot keyed on world coords; a few scattered stacks per chest.
pub fn chest_loot(x: i32, z: i32, salt: u32) -> Vec<HashMap<String, Value>> {
    themed_chest_loot(x, z, salt, LootTheme::Mixed)
}

/// `chest_loot` leaning towards one theme, for shops, stores and vaults.
pub fn themed_chest_loot(
    x: i32,
    z: i32,
    salt: u32,
    theme: LootTheme,
) -> Vec<HashMap<String, Value>> {
    roll_chest(&active_table(), x, z, salt, theme)
}

fn roll_chest(
    table: &LootTable,
    x: i32,
    z: i32,
    salt: u32,
    theme: LootTheme,
) -> Vec<HashMap<String, Value>> {
    let mut rng = coord_rng(x, z, salt as u64 ^ 0x1007_C0DE);
    let rolls = rng.random_range(table.rolls_min..=table.rolls_max);
    // Validated tables always have a total; the built-in one is checked by a test.
    let theme_total = table.theme_total(theme).expect("validated loot table");

    let mut used = [false; CHEST_SLOTS];
    let mut out = Vec::new();

    for _ in 0..rolls {
        let mut pick = rng.random_range(0..theme_total);
        if pick < table.empty_weight {
            continue;
        }
        pick -= table.empty_weight;

        let mut chosen = &table.themes[0];
        for candidate in &table.themes {
            let w = candidate.weight_for(theme) as u32;
            if pick < w {
                chosen = candidate;
                break;
            }
            pick -= w;
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
        item_nbt.insert("id".to_string(), Value::String(item.id.clone()));
        item_nbt.insert("Slot".to_string(), Value::Byte(slot as i8));
        item_nbt.insert("count".to_string(), Value::Int(count));
        out.push(item_nbt);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // FNV-1a over every (slot, id, count) a grid of chests rolls.
    fn themed_fingerprint(table: &LootTable, theme: LootTheme) -> u64 {
        use std::hash::Hasher;
        let mut h = fnv::FnvHasher::default();
        for x in (-300..300).step_by(7) {
            for z in (-300..300).step_by(11) {
                for salt in [0, 1, 0xBEEF] {
                    for item in roll_chest(table, x, z, salt, theme) {
                        let line = format!("{:?}{:?}{:?}", item["Slot"], item["id"], item["count"]);
                        h.write(line.as_bytes());
                    }
                }
            }
        }
        h.finish()
    }

    #[test]
    fn built_in_table_rolls_what_the_hardcoded_themed_chests_rolled() {
        // Captured from upstream's const THEMES code (`themed_chest_loot` for the
        // themes), before the table became data.
        for (theme, want) in [
            (LootTheme::Mixed, 0x44e0_ead9_45e0_1a04),
            (LootTheme::Food, 0x6ccd_dd56_2332_b436),
            (LootTheme::Resources, 0xc1a2_9493_104e_d375),
            (LootTheme::Tools, 0xd822_619d_003b_6ff9),
            (LootTheme::Valuables, 0x5461_6cc7_1bd1_07b5),
        ] {
            assert_eq!(themed_fingerprint(&BUILT_IN, theme), want, "{theme:?}");
        }
    }

    #[test]
    fn table_without_kinds_reads_as_mixed() {
        let json = r#"{"empty_weight":0,"rolls_min":1,"rolls_max":1,"themes":[{"weight":1,"items":[{"id":"minecraft:stick","min":1,"max":1,"weight":1}]}]}"#;
        let table: LootTable = serde_json::from_str(json).unwrap();
        assert_eq!(table.themes[0].kind, LootTheme::Mixed);
        table.validate().unwrap();
    }

    #[test]
    fn built_in_table_validates() {
        BUILT_IN.validate().unwrap();
    }

    #[test]
    fn validate_rejects_tables_that_could_panic() {
        let base = (**BUILT_IN).clone();
        let mut t = base.clone();
        t.rolls_min = 9;
        t.rolls_max = 2;
        assert!(t.validate().is_err());
        let mut t = base.clone();
        t.themes.clear();
        assert!(t.validate().is_err());
        let mut t = base.clone();
        t.themes[0].items.iter_mut().for_each(|i| i.weight = 0);
        assert!(t.validate().is_err());
        let mut t = base.clone();
        t.themes[0].items[0].min = 7;
        t.themes[0].items[0].max = 3;
        assert!(t.validate().is_err());
        let mut t = base;
        t.themes[0].items[0].id = "bread".to_string();
        assert!(t.validate().is_err());
    }

    #[test]
    fn custom_table_used() {
        let table = LootTable {
            empty_weight: 0,
            rolls_min: 2,
            rolls_max: 2,
            themes: vec![LootGroup {
                kind: LootTheme::Mixed,
                weight: 1,
                items: vec![LootEntry {
                    id: "minecraft:diamond".to_string(),
                    min: 5,
                    max: 5,
                    weight: 1,
                }],
            }],
        };
        let loot = roll_chest(&table, 10, -4, 3, LootTheme::Mixed);
        assert!(!loot.is_empty());
        for item in &loot {
            assert_eq!(item["id"], Value::String("minecraft:diamond".to_string()));
            assert_eq!(item["count"], Value::Int(5));
        }
        assert_eq!(roll_chest(&table, 10, -4, 3, LootTheme::Mixed), loot);
    }

    #[test]
    fn dump_then_load_is_identity() {
        let path = std::env::temp_dir().join(format!("arnis-loot-{}.json", std::process::id()));
        std::fs::write(&path, BUILT_IN_JSON).unwrap();
        let loaded = load_loot_table(&path);
        std::fs::remove_file(&path).ok();
        assert_eq!(&loaded.unwrap(), &**BUILT_IN);
    }
}
