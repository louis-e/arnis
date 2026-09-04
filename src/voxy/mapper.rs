//! Voxy's `id_mappings` column family: blockstate and biome registries.
//!
//! Every voxel packs a 20-bit block id and a 9-bit biome id. Those ids are
//! private to the database, so we assign them ourselves and record the
//! translation back to Minecraft the way voxy's `Mapper` writes it: a gzipped,
//! uncompressed NBT compound per entry, keyed by a 4-byte big-endian int whose
//! top two bits select the registry.

use fastnbt::Value;
use fnv::FnvHashMap;
use std::collections::HashMap;
use std::io::Write;

use crate::world_editor::common::PaletteItem;

/// `entryType == 1` in voxy's `Mapper`: a blockstate entry.
const BLOCK_STATE_TYPE: u32 = 1 << 30;
/// `entryType == 2`: a biome entry.
const BIOME_TYPE: u32 = 2 << 30;

/// Air is id 0 by definition and is never written to the registry; voxy's
/// mapper seeds it before loading anything from storage. `BlockState.isAir()`
/// also covers the two placeholder variants, so they collapse to the same id.
pub(crate) fn is_air_name(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

/// Light a block removes, as voxy's mip step ranks it.
///
/// This is Minecraft's `BlockState.getLightDampening()`, which is what
/// `StateEntry` stores, except that voxy forces leaves to fully opaque so
/// distant canopies do not disappear.
fn voxy_opacity(name: &str) -> u8 {
    if name
        .strip_prefix("minecraft:")
        .unwrap_or(name)
        .ends_with("leaves")
    {
        return 15;
    }
    crate::world_editor::java::light_opacity(name)
}

/// A stable, cheap identity for a palette entry. Two entries that produce the
/// same key must be the same blockstate: voxy warns loudly (and renders
/// unpredictably) if two ids deserialize to one `BlockState`.
pub(crate) fn state_key(item: &PaletteItem) -> String {
    let Some(Value::Compound(props)) = item.properties.as_ref() else {
        return item.name.clone();
    };
    if props.is_empty() {
        return item.name.clone();
    }
    let mut pairs: Vec<(&str, String)> = props
        .iter()
        .map(|(k, v)| {
            let text = match v {
                Value::String(s) => s.clone(),
                other => format!("{other:?}"),
            };
            (k.as_str(), text)
        })
        .collect();
    pairs.sort_unstable();

    let mut key = String::with_capacity(item.name.len() + pairs.len() * 12);
    key.push_str(&item.name);
    for (k, v) in pairs {
        key.push('\u{1}');
        key.push_str(k);
        key.push('=');
        key.push_str(&v);
    }
    key
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder =
        flate2::write::GzEncoder::new(Vec::with_capacity(bytes.len()), flate2::Compression::fast());
    encoder
        .write_all(bytes)
        .expect("writing to a Vec cannot fail");
    encoder.finish().expect("writing to a Vec cannot fail")
}

/// One `id_mappings` row: the 4-byte key and the gzipped NBT body.
pub(crate) struct MappingRecord {
    pub(crate) key: u32,
    pub(crate) body: Vec<u8>,
}

/// Assigns and records voxy's block and biome ids.
#[derive(Default)]
pub(crate) struct Mapper {
    blocks: FnvHashMap<String, u32>,
    biomes: FnvHashMap<String, u32>,
    /// Light dampening per block id, indexed directly; entry 0 is air.
    opacity: Vec<u8>,
    records: Vec<MappingRecord>,
}

impl Mapper {
    pub(crate) fn new() -> Self {
        Self {
            opacity: vec![0], // air
            ..Default::default()
        }
    }

    /// Id for a palette entry, registering it on first sight. `key` must come
    /// from [`state_key`] for the same item.
    pub(crate) fn block_id(&mut self, key: &str, item: &PaletteItem) -> u32 {
        if is_air_name(&item.name) {
            return 0;
        }
        if let Some(&id) = self.blocks.get(key) {
            return id;
        }
        let id = self.opacity.len() as u32;
        self.blocks.insert(key.to_string(), id);
        self.opacity.push(voxy_opacity(&item.name));

        let mut state = HashMap::with_capacity(2);
        state.insert("Name".to_string(), Value::String(item.name.clone()));
        if let Some(props @ Value::Compound(map)) = item.properties.as_ref() {
            if !map.is_empty() {
                state.insert("Properties".to_string(), props.clone());
            }
        }
        let mut root = HashMap::with_capacity(2);
        root.insert("block_state".to_string(), Value::Compound(state));
        root.insert("id".to_string(), Value::Int(id as i32));

        self.records.push(MappingRecord {
            key: BLOCK_STATE_TYPE | id,
            body: gzip(&fastnbt::to_bytes(&Value::Compound(root)).expect("blockstate nbt")),
        });
        id
    }

    /// Id for a biome, registering it on first sight. Biome ids are 9 bits, so
    /// the palette has to stay under 512 entries; Arnis uses a few dozen.
    pub(crate) fn biome_id(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.biomes.get(name) {
            return id;
        }
        let id = self.biomes.len() as u32;
        self.biomes.insert(name.to_string(), id);

        let mut root = HashMap::with_capacity(2);
        root.insert("biome_id".to_string(), Value::String(name.to_string()));
        root.insert("id".to_string(), Value::Int(id as i32));

        self.records.push(MappingRecord {
            key: BIOME_TYPE | id,
            body: gzip(&fastnbt::to_bytes(&Value::Compound(root)).expect("biome nbt")),
        });
        id
    }

    /// Light dampening per block id, for the mip step.
    pub(crate) fn opacity_table(&self) -> &[u8] {
        &self.opacity
    }

    pub(crate) fn records(&self) -> &[MappingRecord] {
        &self.records
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, props: &[(&str, &str)]) -> PaletteItem {
        PaletteItem {
            name: name.to_string(),
            properties: (!props.is_empty()).then(|| {
                Value::Compound(
                    props
                        .iter()
                        .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
                        .collect(),
                )
            }),
        }
    }

    #[test]
    fn air_is_id_zero_and_unrecorded() {
        let mut m = Mapper::new();
        for name in ["minecraft:air", "minecraft:cave_air", "minecraft:void_air"] {
            assert_eq!(
                m.block_id(&state_key(&item(name, &[])), &item(name, &[])),
                0
            );
        }
        assert!(m.records().is_empty());
    }

    #[test]
    fn block_ids_start_at_one_and_are_stable() {
        let mut m = Mapper::new();
        let stone = item("minecraft:stone", &[]);
        let dirt = item("minecraft:dirt", &[]);
        assert_eq!(m.block_id(&state_key(&stone), &stone), 1);
        assert_eq!(m.block_id(&state_key(&dirt), &dirt), 2);
        assert_eq!(m.block_id(&state_key(&stone), &stone), 1);
        assert_eq!(m.records().len(), 2);
        assert_eq!(m.records()[0].key, (1 << 30) | 1);
    }

    /// Property order must not create two ids for one blockstate.
    #[test]
    fn state_key_is_order_independent() {
        let a = item(
            "minecraft:oak_stairs",
            &[("facing", "east"), ("half", "top")],
        );
        let b = item(
            "minecraft:oak_stairs",
            &[("half", "top"), ("facing", "east")],
        );
        assert_eq!(state_key(&a), state_key(&b));
        let c = item(
            "minecraft:oak_stairs",
            &[("facing", "west"), ("half", "top")],
        );
        assert_ne!(state_key(&a), state_key(&c));
    }

    #[test]
    fn leaves_are_forced_opaque_for_mipping() {
        let mut m = Mapper::new();
        let leaves = item("minecraft:oak_leaves", &[]);
        let glass = item("minecraft:glass", &[]);
        let id_leaves = m.block_id(&state_key(&leaves), &leaves);
        let id_glass = m.block_id(&state_key(&glass), &glass);
        assert_eq!(m.opacity_table()[id_leaves as usize], 15);
        assert_eq!(m.opacity_table()[id_glass as usize], 0);
        assert_eq!(m.opacity_table()[0], 0, "air never blocks light");
    }

    /// Voxy reads these back with `NbtIo.readCompressed`, i.e. gzip around an
    /// unnamed root compound.
    #[test]
    fn mapping_bodies_are_gzipped_unnamed_compounds() {
        let mut m = Mapper::new();
        let stone = item("minecraft:stone", &[]);
        m.block_id(&state_key(&stone), &stone);
        let body = &m.records()[0].body;
        assert_eq!(&body[0..2], &[0x1f, 0x8b], "gzip magic");

        let raw = {
            use std::io::Read;
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(&body[..])
                .read_to_end(&mut out)
                .unwrap();
            out
        };
        assert_eq!(&raw[0..3], &[0x0a, 0x00, 0x00], "unnamed root compound");
        let parsed: Value = fastnbt::from_bytes(&raw).unwrap();
        let Value::Compound(root) = parsed else {
            panic!("not a compound")
        };
        assert_eq!(root["id"], Value::Int(1));
        let Value::Compound(state) = &root["block_state"] else {
            panic!("no block_state")
        };
        assert_eq!(state["Name"], Value::String("minecraft:stone".to_string()));
    }

    #[test]
    fn biome_ids_start_at_zero() {
        let mut m = Mapper::new();
        assert_eq!(m.biome_id("minecraft:forest"), 0);
        assert_eq!(m.biome_id("minecraft:plains"), 1);
        assert_eq!(m.biome_id("minecraft:forest"), 0);
        assert_eq!(m.records()[0].key, 2 << 30);
    }
}
