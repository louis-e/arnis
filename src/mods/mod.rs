//! Opt-in content from Java mods, for worlds played on Minecraft 1.21.1 with NeoForge.
//!
//! Each pack swaps in a mod's blocks or entities where Arnis would otherwise build a vanilla
//! stand-in. Block names and states are pinned to the mod versions in `PINNED`.

pub mod create;
pub mod farmers_delight;
pub mod macaws;
pub mod supplementaries;
pub mod wildlife;

use crate::block_definitions::{Block, BlockWithProperties};
use fastnbt::Value;
use std::collections::HashMap;

/// The Minecraft version every mod pack is built for.
pub const TARGET_VERSION: &str = "1.21.1";

static TARGET_1_21_1: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set per run: a modded world is written for 1.21.1, which lacks a few newer blocks.
pub fn set_legacy_target(on: bool) {
    TARGET_1_21_1.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The name 1.21.1 knows a block by.
pub fn legacy_name(name: &str) -> &str {
    if !TARGET_1_21_1.load(std::sync::atomic::Ordering::Relaxed) {
        return name;
    }
    match name {
        "pale_oak_trapdoor" => "dark_oak_trapdoor",
        _ => name,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, clap::ValueEnum)]
pub enum ModPack {
    /// Create: industrial details, windmills, scaffolding and seats.
    Create,
    /// Macaw's Furniture: furnished rooms.
    MacawsFurniture,
    /// Macaw's Roofs: roof tiles and awnings.
    MacawsRoofs,
    /// Macaw's Windows: framed windows, shutters, curtains and blinds.
    MacawsWindows,
    /// Macaw's Lights and Lamps: street lamps and indoor lights.
    MacawsLights,
    /// Supplementaries: sign posts, flags, wind vanes, flower boxes and shop details.
    Supplementaries,
    /// Farmer's Delight: crops, kitchens and market crates.
    FarmersDelight,
    /// Automobility: drivable cars on parking lots.
    Automobility,
    /// Naturalist: zoo animals and wildlife.
    Naturalist,
    /// ElevatorMod: lifts in tall buildings.
    Elevators,
}

impl ModPack {
    pub const ALL: [ModPack; 10] = [
        ModPack::Create,
        ModPack::MacawsFurniture,
        ModPack::MacawsRoofs,
        ModPack::MacawsWindows,
        ModPack::MacawsLights,
        ModPack::Supplementaries,
        ModPack::FarmersDelight,
        ModPack::Automobility,
        ModPack::Naturalist,
        ModPack::Elevators,
    ];

    /// The id the CLI and the GUI use.
    pub fn id(self) -> &'static str {
        match self {
            ModPack::Create => "create",
            ModPack::MacawsFurniture => "macaws-furniture",
            ModPack::MacawsRoofs => "macaws-roofs",
            ModPack::MacawsWindows => "macaws-windows",
            ModPack::MacawsLights => "macaws-lights",
            ModPack::Supplementaries => "supplementaries",
            ModPack::FarmersDelight => "farmers-delight",
            ModPack::Automobility => "automobility",
            ModPack::Naturalist => "naturalist",
            ModPack::Elevators => "elevators",
        }
    }

    pub fn from_id(id: &str) -> Option<ModPack> {
        ModPack::ALL.into_iter().find(|m| m.id() == id)
    }

    /// The mods a player needs installed, with the versions the names were checked against.
    pub fn requires(self) -> &'static [&'static str] {
        match self {
            ModPack::Create => &["Create 6.0.10"],
            ModPack::MacawsFurniture => &["Macaw's Furniture 3.4.1"],
            ModPack::MacawsRoofs => &["Macaw's Roofs 2.3.2"],
            ModPack::MacawsWindows => &["Macaw's Windows 2.4.2"],
            ModPack::MacawsLights => &["Macaw's Lights and Lamps 1.1.5"],
            ModPack::Supplementaries => &["Supplementaries 4.0.2", "Moonlight Lib 3.7.2"],
            ModPack::FarmersDelight => &["Farmer's Delight 1.3.4"],
            ModPack::Automobility => &["Automobility 0.5.0.h"],
            ModPack::Naturalist => &["Naturalist 2.0.7"],
            ModPack::Elevators => &["ElevatorMod 1.11.4"],
        }
    }
}

/// The packs a run builds with.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ModSet(u16);

impl ModSet {
    pub fn from_packs(packs: &[ModPack]) -> Self {
        Self(packs.iter().fold(0, |bits, &m| bits | 1 << m as u16))
    }

    #[inline]
    pub fn has(self, pack: ModPack) -> bool {
        self.0 & (1 << pack as u16) != 0
    }

    pub fn packs(self) -> impl Iterator<Item = ModPack> {
        ModPack::ALL.into_iter().filter(move |&m| self.has(m))
    }
}

/// A modded block with its state, interned like any other.
pub fn state(name: &str, fallback: Block, props: &[(&str, &str)]) -> BlockWithProperties {
    let block = Block::modded(name, fallback);
    if props.is_empty() {
        return BlockWithProperties::simple(block);
    }
    let map: HashMap<String, Value> = props
        .iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
        .collect();
    BlockWithProperties::new(block, Some(Value::Compound(map)))
}

/// Cardinal name for a horizontal step.
pub fn dir_name(d: (i32, i32)) -> &'static str {
    match d {
        (0, -1) => "north",
        (0, 1) => "south",
        (1, 0) => "east",
        _ => "west",
    }
}

/// Marks a world this run created as saved by the version modded worlds open in.
pub fn stamp_new_world(world: &std::path::Path) {
    let id = crate::world_editor::java::DATA_VERSION;
    if let Err(e) = crate::world_utils::stamp_level_version(world, id, TARGET_VERSION) {
        eprintln!("Could not mark the world as {TARGET_VERSION}: {e}");
    }
}

/// The world format line for the README and the GUI: what to install to open the world.
pub fn install_note(set: ModSet) -> String {
    let mut names: Vec<&str> = set
        .packs()
        .flat_map(|m| m.requires().iter().copied())
        .collect();
    names.sort_unstable();
    names.dedup();
    format!(
        "Open this world in Minecraft {TARGET_VERSION} with NeoForge and these mods: {}.",
        names.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for m in ModPack::ALL {
            assert_eq!(ModPack::from_id(m.id()), Some(m));
        }
    }

    #[test]
    fn set_holds_exactly_its_packs() {
        let set = ModSet::from_packs(&[ModPack::Create, ModPack::Naturalist]);
        assert!(set.has(ModPack::Create) && set.has(ModPack::Naturalist));
        assert!(!set.has(ModPack::MacawsRoofs));
        assert_eq!(set.packs().count(), 2);
    }

    #[test]
    fn modded_blocks_keep_their_name_and_fallback() {
        let b = Block::modded(
            "mcwfurnitures:oak_chair",
            crate::block_definitions::OAK_STAIRS,
        );
        assert!(b.is_modded());
        assert_eq!(b.namespace(), "mcwfurnitures");
        assert_eq!(b.name(), "oak_chair");
        assert_eq!(b.vanilla(), crate::block_definitions::OAK_STAIRS);
        assert_eq!(
            Block::modded(
                "mcwfurnitures:oak_chair",
                crate::block_definitions::OAK_STAIRS
            ),
            b
        );
        // Another fallback keeps its own, whichever is named first.
        let stone = Block::modded("mcwfurnitures:oak_chair", crate::block_definitions::STONE);
        assert_eq!(stone.name(), "oak_chair");
        assert_eq!(stone.vanilla(), crate::block_definitions::STONE);
        assert_eq!(b.vanilla(), crate::block_definitions::OAK_STAIRS);
        let fallbacks = crate::block_definitions::modded_fallbacks("mcwfurnitures:oak_chair");
        assert!(fallbacks.contains(&crate::block_definitions::OAK_STAIRS));
        assert!(fallbacks.contains(&crate::block_definitions::STONE));
    }
}
