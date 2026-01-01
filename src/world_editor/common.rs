//! Common data structures for world modification.
//!
//! This module contains the internal data structures used to track block changes
//! before they are written to either Java or Bedrock format.

use crate::block_definitions::*;

/// Minimum Y coordinate in Minecraft (1.18+)
const MIN_Y: i32 = -64;
/// Maximum Y coordinate in Minecraft (1.18+)
const MAX_Y: i32 = 319;
use fastnbt::{LongArray, Value};
use fnv::FnvHashMap;
use serde::{Deserialize, Serialize};

/// Chunk structure for Java Edition NBT format
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Chunk {
    pub sections: Vec<Section>,
    pub x_pos: i32,
    pub z_pos: i32,
    #[serde(default)]
    pub is_light_on: u8,
    #[serde(flatten)]
    pub other: FnvHashMap<String, Value>,
}

/// Section within a chunk (16x16x16 blocks)
#[derive(Serialize, Deserialize)]
pub(crate) struct Section {
    pub block_states: Blockstates,
    #[serde(rename = "Y")]
    pub y: i8,
    #[serde(flatten)]
    pub other: FnvHashMap<String, Value>,
}

/// Block states within a section
#[derive(Serialize, Deserialize)]
pub(crate) struct Blockstates {
    pub palette: Vec<PaletteItem>,
    pub data: Option<LongArray>,
    #[serde(flatten)]
    pub other: FnvHashMap<String, Value>,
}

/// Palette item for block state encoding
#[derive(Serialize, Deserialize)]
pub(crate) struct PaletteItem {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Properties")]
    pub properties: Option<Value>,
}

/// A section being modified (16x16x16 blocks)
pub(crate) struct SectionToModify {
    pub blocks: [Block; 4096],
    /// Store properties for blocks that have them, indexed by the same index as blocks array
    pub properties: FnvHashMap<usize, Value>,
}

impl SectionToModify {
    #[inline]
    pub fn get_block(&self, x: u8, y: u8, z: u8) -> Option<Block> {
        let b = self.blocks[Self::index(x, y, z)];
        if b == AIR {
            return None;
        }
        Some(b)
    }

    #[inline]
    pub fn set_block(&mut self, x: u8, y: u8, z: u8, block: Block) {
        self.blocks[Self::index(x, y, z)] = block;
    }

    #[inline]
    pub fn set_block_with_properties(
        &mut self,
        x: u8,
        y: u8,
        z: u8,
        block_with_props: BlockWithProperties,
    ) {
        let index = Self::index(x, y, z);
        self.blocks[index] = block_with_props.block;

        // Store properties if they exist
        if let Some(props) = block_with_props.properties {
            self.properties.insert(index, props);
        } else {
            // Remove any existing properties for this position
            self.properties.remove(&index);
        }
    }

    /// Calculate index from coordinates (YZX order)
    #[inline(always)]
    pub fn index(x: u8, y: u8, z: u8) -> usize {
        usize::from(y) % 16 * 256 + usize::from(z) * 16 + usize::from(x)
    }

    /// Convert to Java Edition section format
    pub fn to_section(&self, y: i8) -> Section {
        // Create a map of unique block+properties combinations to palette indices
        let mut unique_blocks: Vec<(Block, Option<Value>)> = Vec::new();
        let mut palette_lookup: FnvHashMap<(Block, Option<String>), usize> = FnvHashMap::default();

        // Build unique block combinations and lookup table
        for (i, &block) in self.blocks.iter().enumerate() {
            let properties = self.properties.get(&i).cloned();

            // Create a key for the lookup (block + properties hash)
            let props_key = properties.as_ref().map(|p| format!("{p:?}"));
            let lookup_key = (block, props_key);

            if let std::collections::hash_map::Entry::Vacant(e) = palette_lookup.entry(lookup_key) {
                let palette_index = unique_blocks.len();
                e.insert(palette_index);
                unique_blocks.push((block, properties));
            }
        }

        let mut bits_per_block = 4; // minimum allowed
        while (1 << bits_per_block) < unique_blocks.len() {
            bits_per_block += 1;
        }

        let mut data = vec![];
        let mut cur = 0;
        let mut cur_idx = 0;

        for (i, &block) in self.blocks.iter().enumerate() {
            let properties = self.properties.get(&i).cloned();
            let props_key = properties.as_ref().map(|p| format!("{p:?}"));
            let lookup_key = (block, props_key);
            let p = palette_lookup[&lookup_key] as i64;

            if cur_idx + bits_per_block > 64 {
                data.push(cur);
                cur = 0;
                cur_idx = 0;
            }

            cur |= p << cur_idx;
            cur_idx += bits_per_block;
        }

        if cur_idx > 0 {
            data.push(cur);
        }

        let palette = unique_blocks
            .iter()
            .map(|(block, stored_props)| PaletteItem {
                name: block.name().to_string(),
                properties: stored_props.clone().or_else(|| block.properties()),
            })
            .collect();

        Section {
            block_states: Blockstates {
                palette,
                data: Some(LongArray::new(data)),
                other: FnvHashMap::default(),
            },
            y,
            other: FnvHashMap::default(),
        }
    }
}

impl Default for SectionToModify {
    fn default() -> Self {
        Self {
            blocks: [AIR; 4096],
            properties: FnvHashMap::default(),
        }
    }
}

/// A chunk being modified (16x384x16 blocks, divided into sections)
#[derive(Default)]
pub(crate) struct ChunkToModify {
    pub sections: FnvHashMap<i8, SectionToModify>,
    pub other: FnvHashMap<String, Value>,
}

impl ChunkToModify {
    #[inline]
    pub fn get_block(&self, x: u8, y: i32, z: u8) -> Option<Block> {
        // Clamp Y to valid Minecraft range to prevent TryFromIntError
        let y = y.clamp(MIN_Y, MAX_Y);
        let section_idx: i8 = (y >> 4) as i8;
        let section = self.sections.get(&section_idx)?;
        section.get_block(x, (y & 15) as u8, z)
    }

    #[inline]
    pub fn set_block(&mut self, x: u8, y: i32, z: u8, block: Block) {
        // Clamp Y to valid Minecraft range to prevent TryFromIntError
        let y = y.clamp(MIN_Y, MAX_Y);
        let section_idx: i8 = (y >> 4) as i8;
        let section = self.sections.entry(section_idx).or_default();
        section.set_block(x, (y & 15) as u8, z, block);
    }

    #[inline]
    pub fn set_block_with_properties(
        &mut self,
        x: u8,
        y: i32,
        z: u8,
        block_with_props: BlockWithProperties,
    ) {
        // Clamp Y to valid Minecraft range to prevent TryFromIntError
        let y = y.clamp(MIN_Y, MAX_Y);
        let section_idx: i8 = (y >> 4) as i8;
        let section = self.sections.entry(section_idx).or_default();
        section.set_block_with_properties(x, (y & 15) as u8, z, block_with_props);
    }

    pub fn sections(&self) -> impl Iterator<Item = Section> + '_ {
        self.sections.iter().map(|(y, s)| s.to_section(*y))
    }
}

/// A region being modified (32x32 chunks)
#[derive(Default)]
pub(crate) struct RegionToModify {
    pub chunks: FnvHashMap<(i32, i32), ChunkToModify>,
}

impl RegionToModify {
    #[inline]
    pub fn get_or_create_chunk(&mut self, x: i32, z: i32) -> &mut ChunkToModify {
        self.chunks.entry((x, z)).or_default()
    }

    #[inline]
    pub fn get_chunk(&self, x: i32, z: i32) -> Option<&ChunkToModify> {
        self.chunks.get(&(x, z))
    }
}

/// The entire world being modified
#[derive(Default)]
pub(crate) struct WorldToModify {
    pub regions: FnvHashMap<(i32, i32), RegionToModify>,
}

impl WorldToModify {
    #[inline]
    pub fn get_or_create_region(&mut self, x: i32, z: i32) -> &mut RegionToModify {
        self.regions.entry((x, z)).or_default()
    }

    #[inline]
    pub fn get_region(&self, x: i32, z: i32) -> Option<&RegionToModify> {
        self.regions.get(&(x, z))
    }

    #[inline]
    pub fn get_block(&self, x: i32, y: i32, z: i32) -> Option<Block> {
        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;

        let region: &RegionToModify = self.get_region(region_x, region_z)?;
        let chunk: &ChunkToModify = region.get_chunk(chunk_x & 31, chunk_z & 31)?;

        chunk.get_block(
            (x & 15).try_into().unwrap(),
            y,
            (z & 15).try_into().unwrap(),
        )
    }

    #[inline]
    pub fn set_block(&mut self, x: i32, y: i32, z: i32, block: Block) {
        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;

        let region: &mut RegionToModify = self.get_or_create_region(region_x, region_z);
        let chunk: &mut ChunkToModify = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

        chunk.set_block(
            (x & 15).try_into().unwrap(),
            y,
            (z & 15).try_into().unwrap(),
            block,
        );
    }

    #[inline]
    pub fn set_block_with_properties(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        block_with_props: BlockWithProperties,
    ) {
        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;

        let region: &mut RegionToModify = self.get_or_create_region(region_x, region_z);
        let chunk: &mut ChunkToModify = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

        chunk.set_block_with_properties(
            (x & 15).try_into().unwrap(),
            y,
            (z & 15).try_into().unwrap(),
            block_with_props,
        );
    }
}
