//! A `DecalKey` describes one image. The registry gives each distinct key a block of
//! consecutive map ids before placement, so tile threads only read it and ids are stable.

use super::region::BladeStyle;
use std::collections::{BTreeSet, HashMap};

/// Look of a rendered text sign.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TextStyle {
    /// Shop fascia: dark strip with light lettering.
    Fascia,
    /// Street name blade.
    StreetName(BladeStyle),
    /// Small house number plate.
    HouseNumber,
    /// Platform / station name board.
    StationBoard,
    /// Name plate under a transit stop sign.
    StopName,
    /// Engraved memorial plaque.
    Plaque,
}

/// Standard traffic signs drawn from primitives.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TrafficSign {
    Stop,
    GiveWay,
    NoEntry,
    PriorityRoad,
    Crossing,
    OneWay,
    NoParking,
    DeadEnd,
    LevelCrossing,
    HighVoltage,
    Bicycle,
    Motorway,
    MotorwayEnd,
}

/// Route number shield family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ShieldStyle {
    /// Blue plate, white text (European motorways / A-roads in many countries).
    Blue,
    /// Yellow plate, black text (German Bundesstrasse, Dutch N-roads style).
    Yellow,
    /// Green plate, white text (UK primary routes, some European E-roads).
    Green,
    /// US interstate look: red-white-blue shield.
    Interstate,
    /// White shield with black text (US highways).
    White,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DecalKey {
    /// A bundled pictogram by asset name.
    Pictogram(&'static str),
    /// Rendered text in a given style; `cols` tiles wide.
    Text {
        style: TextStyle,
        text: String,
        cols: u8,
    },
    Traffic(TrafficSign),
    SpeedLimit {
        value: u16,
        mph: bool,
        style: SpeedStyle,
    },
    RouteShield {
        style: ShieldStyle,
        text: String,
    },
    /// Billboard art variant, 3x2 tiles.
    Poster(u8),
    /// Advertising column poster variant, 1x2 tiles.
    ColumnPoster(u8),
    /// "You are here" board centred on a world position, 2x2 tiles.
    LocalMap {
        x: i32,
        z: i32,
    },
}

/// Speed limit sign shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SpeedStyle {
    /// Red-ringed disc (Vienna Convention).
    Disc,
    /// White "SPEED LIMIT" plate (USA).
    UsPlate,
    /// White "MAXIMUM" plate (Canada).
    CaPlate,
}

impl DecalKey {
    /// Tile grid the rendered image spans.
    pub fn dims(&self) -> (u32, u32) {
        match self {
            DecalKey::Text { cols, .. } => (*cols as u32, 1),
            DecalKey::Poster(_) => super::posters::BILLBOARD_TILES,
            DecalKey::ColumnPoster(_) => super::posters::COLUMN_TILES,
            DecalKey::LocalMap { .. } => (2, 2),
            _ => (1, 1),
        }
    }

    pub fn text(style: TextStyle, text: impl Into<String>, cols: u8) -> DecalKey {
        DecalKey::Text {
            style,
            text: text.into(),
            cols: cols.max(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecalEntry {
    /// Map id of the top-left tile; tile (c, r) is `base_id + r * cols + c`.
    pub base_id: i32,
    pub cols: u32,
    pub rows: u32,
}

impl DecalEntry {
    pub fn tile_id(&self, col: u32, row: u32) -> i32 {
        self.base_id + (row * self.cols + col) as i32
    }
}

/// Deterministic key -> map id assignment.
#[derive(Debug, Default)]
pub struct DecalRegistry {
    entries: HashMap<DecalKey, DecalEntry>,
    ordered: Vec<DecalKey>,
    next_id: i32,
}

impl DecalRegistry {
    /// Ids 0 and 1 belong to the world preview map and the branding map.
    pub const FIRST_ID: i32 = 2;

    /// Assigns ids to `keys` in their sorted order.
    pub fn from_keys(keys: BTreeSet<DecalKey>) -> Self {
        let mut reg = DecalRegistry {
            entries: HashMap::with_capacity(keys.len()),
            ordered: Vec::with_capacity(keys.len()),
            next_id: Self::FIRST_ID,
        };
        for key in keys {
            let (cols, rows) = key.dims();
            reg.entries.insert(
                key.clone(),
                DecalEntry {
                    base_id: reg.next_id,
                    cols,
                    rows,
                },
            );
            reg.next_id += (cols * rows) as i32;
            reg.ordered.push(key);
        }
        reg
    }

    pub fn get(&self, key: &DecalKey) -> Option<DecalEntry> {
        self.entries.get(key).copied()
    }

    pub fn contains(&self, key: &DecalKey) -> bool {
        self.entries.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.ordered.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ordered.is_empty()
    }

    /// Keys in id order.
    pub fn iter(&self) -> impl Iterator<Item = (&DecalKey, DecalEntry)> {
        self.ordered.iter().map(move |k| (k, self.entries[k]))
    }

    /// Highest assigned map id, or `FIRST_ID - 1` when empty.
    #[allow(dead_code)]
    pub fn max_id(&self) -> i32 {
        self.next_id - 1
    }

    /// Number of map files this registry produces.
    pub fn tile_count(&self) -> i32 {
        self.next_id - Self::FIRST_ID
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_dense_and_ordered() {
        let mut keys = BTreeSet::new();
        keys.insert(DecalKey::Pictogram("cafe"));
        keys.insert(DecalKey::Poster(0));
        keys.insert(DecalKey::text(TextStyle::Fascia, "Bakery", 2));
        let reg = DecalRegistry::from_keys(keys);
        assert_eq!(reg.len(), 3);
        // Ordering follows the enum: Pictogram < Text < Poster.
        let ids: Vec<i32> = reg.iter().map(|(_, e)| e.base_id).collect();
        assert_eq!(ids[0], DecalRegistry::FIRST_ID);
        assert_eq!(reg.get(&DecalKey::Pictogram("cafe")).unwrap().base_id, 2);
        let text = reg
            .get(&DecalKey::text(TextStyle::Fascia, "Bakery", 2))
            .unwrap();
        assert_eq!((text.base_id, text.cols, text.rows), (3, 2, 1));
        let poster = reg.get(&DecalKey::Poster(0)).unwrap();
        assert_eq!((poster.base_id, poster.cols, poster.rows), (5, 3, 2));
        assert_eq!(poster.tile_id(2, 1), 5 + 5);
        assert_eq!(reg.max_id(), 10);
        assert_eq!(reg.tile_count(), 9);
    }

    #[test]
    fn same_keys_same_ids() {
        let build = || {
            let mut keys = BTreeSet::new();
            keys.insert(DecalKey::text(TextStyle::HouseNumber, "12a", 1));
            keys.insert(DecalKey::Traffic(TrafficSign::Stop));
            keys.insert(DecalKey::LocalMap { x: 5, z: -3 });
            DecalRegistry::from_keys(keys)
        };
        let a = build();
        let b = build();
        for (k, e) in a.iter() {
            assert_eq!(b.get(k), Some(e));
        }
    }
}
