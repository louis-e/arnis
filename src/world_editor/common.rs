//! Common data structures for world modification.
//!
//! This module contains the internal data structures used to track block changes
//! before they are written to either Java or Bedrock format.

use crate::block_definitions::*;

/// Minimum Y coordinate in Minecraft (1.18+)
pub const MIN_Y: i32 = -64;
/// Lowest section index covering MIN_Y (-64 / 16).
pub const MIN_SECTION_Y: i8 = (MIN_Y / 16) as i8;
/// Maximum Y coordinate in Minecraft (data pack maximum: 2031)
/// Vanilla limit is 319, but data packs can extend this up to 2031.
/// The world editor supports the full range; the elevation system controls
/// the actual heights used based on the disable_height_limit setting.
const MAX_Y: i32 = 2031;
/// Sizes the per-section palette lookup array. Block ids are u16 but stay well
/// below this; raise it if block_definitions ever allocates an id this high.
const MAX_BLOCK_ID: usize = 512;
use fastnbt::{LongArray, Value};
use fnv::FnvHashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Section {
    pub block_states: Blockstates,
    #[serde(rename = "Y")]
    pub y: i8,
    #[serde(flatten)]
    pub other: FnvHashMap<String, Value>,
}

/// Block states within a section
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Blockstates {
    pub palette: Vec<PaletteItem>,
    pub data: Option<LongArray>,
    #[serde(flatten)]
    pub other: FnvHashMap<String, Value>,
}

/// Palette item for block state encoding
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct PaletteItem {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Properties")]
    pub properties: Option<Value>,
}

/// Block storage strategy for a 16×16×16 section.
///
/// **Memory optimisation**: instead of always allocating a 4 096-byte array,
/// we distinguish two cases:
///
/// * `Uniform(block)` – every position holds the same block (1 byte).
///   This covers freshly-created (all-AIR) sections, and sections that were
///   entirely filled with one type (e.g. STONE underground with `--fillground`).
///
/// * `Full(Vec<u8>)` – the general case for sections whose block ids all
///   fit in a byte (the overwhelming majority), one byte per cell.
///
/// * `FullWide(Vec<Block>)` – only for sections that contain a block id of
///   256 or more (a handful of underwater blocks); two bytes per cell. Kept
///   separate so the common case isn't paying for the wider id space.
///
/// Both are heap-allocated via `Vec`, so the inline size inside the parent
/// `FnvHashMap` entry is only 24 bytes.
pub(crate) enum BlockStorage {
    /// Every position is the same block (commonly AIR).
    Uniform(Block),
    /// Mixed blocks, all ids < 256 – always exactly 4 096 entries.
    Full(Vec<u8>),
    /// Mixed blocks with at least one id >= 256 – always 4 096 entries.
    FullWide(Vec<Block>),
}

impl BlockStorage {
    /// Read block at flat `index` (0..4095).
    #[inline(always)]
    pub fn get(&self, index: usize) -> Block {
        match self {
            BlockStorage::Uniform(b) => *b,
            BlockStorage::Full(v) => Block::from_raw_id(u16::from(v[index])),
            BlockStorage::FullWide(v) => v[index],
        }
    }

    /// Write block at flat `index`. Promotes `Uniform` → `Full`/`FullWide`
    /// on the first differing write, and `Full` → `FullWide` the first time
    /// a wide id is written.
    #[inline]
    pub fn set(&mut self, index: usize, block: Block) {
        match self {
            BlockStorage::Uniform(b) if *b == block => {
                // No-op – writing the same value.
            }
            BlockStorage::Uniform(base) => {
                let base = *base;
                if base.id() < 256 && block.id() < 256 {
                    let mut v = vec![base.id() as u8; 4096];
                    v[index] = block.id() as u8;
                    *self = BlockStorage::Full(v);
                } else {
                    let mut v = vec![base; 4096];
                    v[index] = block;
                    *self = BlockStorage::FullWide(v);
                }
            }
            BlockStorage::Full(v) => {
                if block.id() < 256 {
                    v[index] = block.id() as u8;
                } else {
                    let mut wide: Vec<Block> = v
                        .iter()
                        .map(|&id| Block::from_raw_id(u16::from(id)))
                        .collect();
                    wide[index] = block;
                    *self = BlockStorage::FullWide(wide);
                }
            }
            BlockStorage::FullWide(v) => {
                v[index] = block;
            }
        }
    }

    /// Iterate over all 4 096 blocks.
    #[inline]
    pub fn iter(&self) -> BlockStorageIter<'_> {
        match self {
            BlockStorage::Uniform(b) => BlockStorageIter::Uniform(*b, 0),
            BlockStorage::Full(v) => BlockStorageIter::Full(v.iter()),
            BlockStorage::FullWide(v) => BlockStorageIter::FullWide(v.iter()),
        }
    }

    /// Try to collapse a mixed section back to `Uniform` if every entry
    /// is the same block. Frees the heap allocation.
    pub fn try_compact(&mut self) {
        match self {
            BlockStorage::Full(v) => {
                if let Some(&first) = v.first() {
                    if v.iter().all(|&b| b == first) {
                        *self = BlockStorage::Uniform(Block::from_raw_id(u16::from(first)));
                    }
                }
            }
            BlockStorage::FullWide(v) => {
                if let Some(&first) = v.first() {
                    if v.iter().all(|&b| b == first) {
                        *self = BlockStorage::Uniform(first);
                    }
                }
            }
            BlockStorage::Uniform(_) => {}
        }
    }
}

/// Iterator returned by [`BlockStorage::iter`].
pub(crate) enum BlockStorageIter<'a> {
    Uniform(Block, usize),
    Full(std::slice::Iter<'a, u8>),
    FullWide(std::slice::Iter<'a, Block>),
}

impl<'a> Iterator for BlockStorageIter<'a> {
    type Item = Block;

    #[inline]
    fn next(&mut self) -> Option<Block> {
        match self {
            BlockStorageIter::Uniform(b, count) => {
                if *count < 4096 {
                    *count += 1;
                    Some(*b)
                } else {
                    None
                }
            }
            BlockStorageIter::Full(it) => it.next().map(|&id| Block::from_raw_id(u16::from(id))),
            BlockStorageIter::FullWide(it) => it.next().copied(),
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let rem = match self {
            BlockStorageIter::Uniform(_, c) => 4096 - *c,
            BlockStorageIter::Full(it) => it.len(),
            BlockStorageIter::FullWide(it) => it.len(),
        };
        (rem, Some(rem))
    }
}

impl ExactSizeIterator for BlockStorageIter<'_> {}

/// A section being modified (16x16x16 blocks)
pub(crate) struct SectionToModify {
    pub storage: BlockStorage,
    /// Per-cell NBT properties; Arc-shared so identical compounds reuse one allocation.
    pub properties: FnvHashMap<usize, Arc<Value>>,
}

impl SectionToModify {
    #[inline]
    pub fn get_block(&self, x: u8, y: u8, z: u8) -> Option<Block> {
        let b = self.storage.get(Self::index(x, y, z));
        if b == AIR {
            return None;
        }
        Some(b)
    }

    #[inline]
    pub fn set_block(&mut self, x: u8, y: u8, z: u8, block: Block) {
        let index = Self::index(x, y, z);
        self.storage.set(index, block);
        self.properties.remove(&index);
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
        self.storage.set(index, block_with_props.block);

        // Store properties if they exist
        if let Some(props) = block_with_props.properties {
            self.properties.insert(index, props);
        } else {
            // Remove any existing properties for this position
            self.properties.remove(&index);
        }
    }

    /// Read block at a raw flat index (used by Bedrock serialiser).
    #[inline(always)]
    pub fn get_block_at_index(&self, index: usize) -> Block {
        self.storage.get(index)
    }

    /// Calculate index from coordinates (YZX order)
    #[inline(always)]
    pub fn index(x: u8, y: u8, z: u8) -> usize {
        usize::from(y) % 16 * 256 + usize::from(z) * 16 + usize::from(x)
    }

    /// Try to collapse the block array back to `Uniform` if every entry
    /// is the same block and there are no properties.
    pub fn compact(&mut self) {
        if self.properties.is_empty() {
            self.storage.try_compact();
        }
    }

    /// Convert to Java Edition section format
    pub fn to_section(&self, y: i8) -> Section {
        // Fast path: Uniform section → single palette entry, no data array needed.
        // Only valid when no per-index properties exist, otherwise we must
        // fall through to the general path so every index is checked.
        if self.properties.is_empty() {
            if let BlockStorage::Uniform(block) = &self.storage {
                let palette_item = PaletteItem {
                    name: format!("{}:{}", block.namespace(), block.name()),
                    properties: block.properties(),
                };
                return Section {
                    block_states: Blockstates {
                        palette: vec![palette_item],
                        data: None,
                        other: FnvHashMap::default(),
                    },
                    y,
                    other: FnvHashMap::default(),
                };
            }
        }

        // Medium path: Full storage with no per-index properties.
        // Use Block id directly as palette key; no string formatting needed.
        if self.properties.is_empty() && !matches!(self.storage, BlockStorage::Uniform(_)) {
            // Build palette from unique blocks; array indexed by block id.
            let mut block_to_palette = [u16::MAX; MAX_BLOCK_ID];
            let mut palette_blocks: Vec<Block> = Vec::new();

            for block in self.storage.iter() {
                let id = block.id() as usize;
                debug_assert!(
                    id < MAX_BLOCK_ID,
                    "block id {id} exceeds palette array size"
                );
                if block_to_palette[id] == u16::MAX {
                    block_to_palette[id] = palette_blocks.len() as u16;
                    palette_blocks.push(block);
                }
            }

            let mut bits_per_block = 4;
            while (1 << bits_per_block) < palette_blocks.len() {
                bits_per_block += 1;
            }

            let mut data = vec![];
            let mut cur: i64 = 0;
            let mut cur_idx = 0;

            for block in self.storage.iter() {
                let p = block_to_palette[block.id() as usize] as i64;

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

            let palette = palette_blocks
                .iter()
                .map(|block| PaletteItem {
                    name: format!("{}:{}", block.namespace(), block.name()),
                    properties: block.properties(),
                })
                .collect();

            return Section {
                block_states: Blockstates {
                    palette,
                    data: Some(LongArray::new(data)),
                    other: FnvHashMap::default(),
                },
                y,
                other: FnvHashMap::default(),
            };
        }

        // Slow path: mixed blocks with per-index properties.
        // Single pass: build palette and per-block index array simultaneously.
        let mut unique_blocks: Vec<(Block, Option<Arc<Value>>)> = Vec::new();
        let mut palette_lookup: FnvHashMap<(Block, Option<String>), usize> = FnvHashMap::default();
        let mut indices = Vec::with_capacity(4096);

        for (i, block) in self.storage.iter().enumerate() {
            let properties = self.properties.get(&i);

            // Create a key for the lookup (block + properties debug string)
            let props_key = properties.map(|p| format!("{p:?}"));
            let lookup_key = (block, props_key);

            let palette_index = match palette_lookup.entry(lookup_key) {
                std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    let idx = unique_blocks.len();
                    e.insert(idx);
                    unique_blocks.push((block, properties.cloned()));
                    idx
                }
            };
            indices.push(palette_index);
        }

        let mut bits_per_block = 4; // minimum allowed
        while (1 << bits_per_block) < unique_blocks.len() {
            bits_per_block += 1;
        }

        // Pack indices into long array
        let mut data = vec![];
        let mut cur: i64 = 0;
        let mut cur_idx = 0;

        for &p in &indices {
            if cur_idx + bits_per_block > 64 {
                data.push(cur);
                cur = 0;
                cur_idx = 0;
            }

            cur |= (p as i64) << cur_idx;
            cur_idx += bits_per_block;
        }

        if cur_idx > 0 {
            data.push(cur);
        }

        let palette = unique_blocks
            .iter()
            .map(|(block, stored_props)| PaletteItem {
                name: format!("{}:{}", block.namespace(), block.name()),
                properties: stored_props
                    .as_ref()
                    .map(|p| (**p).clone())
                    .or_else(|| block.properties()),
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
            storage: BlockStorage::Uniform(AIR),
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

/// The entire world being modified.
#[derive(Default)]
pub(crate) struct WorldToModify {
    pub regions: FnvHashMap<(i32, i32), RegionToModify>,
}

impl WorldToModify {
    /// Deterministic, storage-representation-independent hash of all block IDs.
    /// Combined across regions with an order-independent fold (wrapping_add of each
    /// region's hash) so the streaming/eviction path can accumulate it region-by-region
    /// at flush time and still match this whole-world value exactly. Used to verify
    /// parallel output is race-free and equals the non-eviction path.
    pub fn content_hash(&self) -> u64 {
        self.regions.keys().fold(0u64, |acc, &(rx, rz)| {
            acc.wrapping_add(self.region_content_hash(rx, rz))
        })
    }

    /// Deterministic hash of a single region's block content (region key + sorted
    /// chunk/section/storage). Returns 0 if the region is absent.
    pub fn region_content_hash(&self, rx: i32, rz: i32) -> u64 {
        use std::hash::{Hash, Hasher};
        let Some(region) = self.regions.get(&(rx, rz)) else {
            return 0;
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (rx, rz).hash(&mut h);
        let mut chunk_keys: Vec<&(i32, i32)> = region.chunks.keys().collect();
        chunk_keys.sort_unstable();
        for ck in chunk_keys {
            ck.hash(&mut h);
            let chunk = &region.chunks[ck];
            let mut sec_keys: Vec<&i8> = chunk.sections.keys().collect();
            sec_keys.sort_unstable();
            for sk in sec_keys {
                sk.hash(&mut h);
                // Hash logical block ids, not the raw storage, so a section
                // is hashed identically whether it ended up Full or FullWide.
                let storage = &chunk.sections[sk].storage;
                match storage {
                    BlockStorage::Uniform(b) => b.hash(&mut h),
                    _ => {
                        let first = storage.get(0);
                        if storage.iter().all(|b| b == first) {
                            first.hash(&mut h);
                        } else {
                            for b in storage.iter() {
                                b.hash(&mut h);
                            }
                        }
                    }
                }
            }
        }
        h.finish()
    }

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

    /// Set a block only if the cell is empty (AIR). Thin `#[inline]` wrapper over [`set_with_props_if_absent`].
    #[inline]
    pub fn set_block_if_absent(&mut self, x: i32, y: i32, z: i32, block: Block) {
        self.set_with_props_if_absent(
            x,
            y,
            z,
            BlockWithProperties {
                block,
                properties: None,
            },
        );
    }

    /// Set a block (+ optional NBT) only if the cell is empty (AIR), in one region/chunk/section descent.
    #[inline]
    pub fn set_with_props_if_absent(
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

        let region = self.regions.entry((region_x, region_z)).or_default();
        let chunk = region
            .chunks
            .entry((chunk_x & 31, chunk_z & 31))
            .or_default();

        let y = y.clamp(MIN_Y, MAX_Y);
        let section_idx: i8 = (y >> 4) as i8;
        let section = chunk.sections.entry(section_idx).or_default();

        let local_x = (x & 15) as u8;
        let local_y = (y & 15) as u8;
        let local_z = (z & 15) as u8;
        let idx = SectionToModify::index(local_x, local_y, local_z);

        if section.storage.get(idx) == AIR {
            section.storage.set(idx, block_with_props.block);
            if let Some(props) = block_with_props.properties {
                section.properties.insert(idx, props);
            } else {
                section.properties.remove(&idx);
            }
        }
    }

    /// Fill an entire column (single x, z) from y_min to y_max with the same block,
    /// resolving region/chunk only once.  Used by ground generation.
    #[inline]
    pub fn fill_column(
        &mut self,
        x: i32,
        z: i32,
        y_min: i32,
        y_max: i32,
        block: Block,
        skip_existing: bool,
    ) {
        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;

        let region = self.regions.entry((region_x, region_z)).or_default();
        let chunk = region
            .chunks
            .entry((chunk_x & 31, chunk_z & 31))
            .or_default();

        let local_x = (x & 15) as u8;
        let local_z = (z & 15) as u8;

        let y_min = y_min.clamp(MIN_Y, MAX_Y);
        let y_max = y_max.clamp(MIN_Y, MAX_Y);

        for y in y_min..=y_max {
            let section_idx: i8 = (y >> 4) as i8;
            let section = chunk.sections.entry(section_idx).or_default();
            let local_y = (y & 15) as u8;
            let idx = SectionToModify::index(local_x, local_y, local_z);

            if skip_existing {
                if section.storage.get(idx) == AIR {
                    section.storage.set(idx, block);
                    section.properties.remove(&idx);
                }
            } else {
                section.storage.set(idx, block);
                section.properties.remove(&idx);
            }
        }
    }

    /// Fill empty (Uniform(AIR)) sections of a chunk up to `section_y_max` with
    /// `Uniform(block)`. Returns true only if every section in the range was empty.
    pub fn bulk_fill_chunk_sections_below(
        &mut self,
        chunk_x: i32,
        chunk_z: i32,
        section_y_max: i8,
        block: Block,
    ) -> bool {
        if section_y_max < MIN_SECTION_Y {
            return true;
        }
        let region_x = chunk_x >> 5;
        let region_z = chunk_z >> 5;
        let region = self.regions.entry((region_x, region_z)).or_default();
        let chunk = region
            .chunks
            .entry((chunk_x & 31, chunk_z & 31))
            .or_default();

        let mut all_clean = true;
        for section_y in MIN_SECTION_Y..=section_y_max {
            let section = chunk.sections.entry(section_y).or_default();
            let is_empty = section.properties.is_empty()
                && matches!(&section.storage, BlockStorage::Uniform(b) if *b == AIR);
            if is_empty {
                section.storage = BlockStorage::Uniform(block);
            } else {
                all_clean = false;
            }
        }
        all_clean
    }

    /// Merge another `WorldToModify` into self.
    ///
    /// For each non-AIR block in `other`, write it into `self`.
    /// Blocks within the authoritative bounds always overwrite; blocks outside
    /// only write if the target position is currently AIR.
    ///
    /// Uses region-level fast paths for the common case where tiles are
    /// region-aligned (512×512): regions fully inside the authoritative area
    /// are moved at the chunk level (no per-block iteration), and regions
    /// fully outside use write-if-AIR without per-block coordinate math.
    ///
    /// **Merge-order invariant**: when multiple tiles are merged in sequence,
    /// later merges may encounter chunks already populated by earlier tiles'
    /// halo writes. The fully-authoritative fast path detects this case and
    /// reconciles per-section so halo data at AIR positions is preserved
    /// (auth tile only overwrites where it placed non-AIR). Without this,
    /// e.g. tree canopies that cross tile boundaries would be clobbered when
    /// the receiving tile happens to have a chunk in the same column.
    pub fn merge(
        &mut self,
        other: WorldToModify,
        authoritative_min_x: i32,
        authoritative_min_z: i32,
        authoritative_max_x: i32,
        authoritative_max_z: i32,
    ) {
        for ((region_x, region_z), other_region) in other.regions {
            // Region block-coordinate bounds (32 chunks × 16 blocks = 512 per side)
            let r_min_x = region_x << 9;
            let r_max_x = r_min_x + 511;
            let r_min_z = region_z << 9;
            let r_max_z = r_min_z + 511;

            let fully_authoritative = r_min_x >= authoritative_min_x
                && r_max_x <= authoritative_max_x
                && r_min_z >= authoritative_min_z
                && r_max_z <= authoritative_max_z;

            if fully_authoritative {
                // Fast path: entire region is owned by the auth tile.
                // Wholesale chunk insert when the destination is empty;
                // per-section reconcile when a prior tile already wrote
                // halo data into this region (auth tile non-AIR wins;
                // halo wins where auth tile left AIR).
                let self_region = self.regions.entry((region_x, region_z)).or_default();
                for (chunk_key, other_chunk) in other_region.chunks {
                    match self_region.chunks.entry(chunk_key) {
                        std::collections::hash_map::Entry::Vacant(e) => {
                            e.insert(other_chunk);
                        }
                        std::collections::hash_map::Entry::Occupied(mut e) => {
                            let self_chunk = e.get_mut();
                            for (section_y, other_section) in other_chunk.sections {
                                let self_section =
                                    self_chunk.sections.entry(section_y).or_default();
                                Self::merge_section_auth_overwrite_nonair(
                                    self_section,
                                    &other_section,
                                );
                            }
                            for (key, value) in other_chunk.other {
                                if key == "block_entities" || key == "entities" {
                                    match self_chunk.other.entry(key) {
                                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                                            if let Value::List(self_list) = entry.get_mut() {
                                                if let Value::List(other_list) = &value {
                                                    self_list.extend(other_list.iter().cloned());
                                                }
                                            }
                                        }
                                        std::collections::hash_map::Entry::Vacant(entry) => {
                                            entry.insert(value);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                continue;
            }

            let fully_outside = r_max_x < authoritative_min_x
                || r_min_x > authoritative_max_x
                || r_max_z < authoritative_min_z
                || r_min_z > authoritative_max_z;

            if fully_outside {
                // Fast path: region is entirely in the halo zone.
                // Write non-AIR blocks only where dest is AIR (no coordinate math).
                Self::merge_region_write_if_air(
                    self.regions.entry((region_x, region_z)).or_default(),
                    other_region,
                );
                continue;
            }

            // Slow path: region partially overlaps authoritative bounds.
            // (Rare with region-aligned tiles; kept as safety net.)
            let self_region = self.regions.entry((region_x, region_z)).or_default();
            for ((chunk_lx, chunk_lz), other_chunk) in other_region.chunks {
                // Check chunk-level: can we fast-path this entire chunk?
                let c_min_x = (region_x * 32 + chunk_lx) * 16;
                let c_max_x = c_min_x + 15;
                let c_min_z = (region_z * 32 + chunk_lz) * 16;
                let c_max_z = c_min_z + 15;

                let chunk_fully_auth = c_min_x >= authoritative_min_x
                    && c_max_x <= authoritative_max_x
                    && c_min_z >= authoritative_min_z
                    && c_max_z <= authoritative_max_z;

                if chunk_fully_auth {
                    match self_region.chunks.entry((chunk_lx, chunk_lz)) {
                        std::collections::hash_map::Entry::Vacant(e) => {
                            e.insert(other_chunk);
                        }
                        std::collections::hash_map::Entry::Occupied(mut e) => {
                            // Dest already holds a prior tile's halo data: overwrite
                            // auth non-AIR, preserve halo at auth-AIR (matches the
                            // region fast path) instead of clobbering the whole chunk.
                            let self_chunk = e.get_mut();
                            for (section_y, other_section) in other_chunk.sections {
                                Self::merge_section_auth_overwrite_nonair(
                                    self_chunk.sections.entry(section_y).or_default(),
                                    &other_section,
                                );
                            }
                            for (key, value) in other_chunk.other {
                                if key == "block_entities" || key == "entities" {
                                    match self_chunk.other.entry(key) {
                                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                                            if let Value::List(self_list) = entry.get_mut() {
                                                if let Value::List(other_list) = &value {
                                                    self_list.extend(other_list.iter().cloned());
                                                }
                                            }
                                        }
                                        std::collections::hash_map::Entry::Vacant(entry) => {
                                            entry.insert(value);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }

                let chunk_fully_outside = c_max_x < authoritative_min_x
                    || c_min_x > authoritative_max_x
                    || c_max_z < authoritative_min_z
                    || c_min_z > authoritative_max_z;

                let self_chunk = self_region.chunks.entry((chunk_lx, chunk_lz)).or_default();

                if chunk_fully_outside {
                    // Write-if-AIR for entire chunk, no coordinate math
                    for (section_y, other_section) in other_chunk.sections {
                        Self::merge_section_write_if_air(
                            self_chunk.sections.entry(section_y).or_default(),
                            &other_section,
                        );
                    }
                } else {
                    // Per-block merge with coordinate checks (truly partial overlap)
                    for (section_y, other_section) in other_chunk.sections {
                        let self_section = self_chunk.sections.entry(section_y).or_default();
                        Self::merge_section_with_auth_check(
                            self_section,
                            &other_section,
                            c_min_x,
                            c_min_z,
                            authoritative_min_x,
                            authoritative_min_z,
                            authoritative_max_x,
                            authoritative_max_z,
                        );
                    }
                }

                // Merge block entities and entities
                for (key, value) in other_chunk.other {
                    if key == "block_entities" || key == "entities" {
                        match self_chunk.other.entry(key) {
                            std::collections::hash_map::Entry::Occupied(mut entry) => {
                                if let Value::List(self_list) = entry.get_mut() {
                                    if let Value::List(other_list) = &value {
                                        self_list.extend(other_list.iter().cloned());
                                    }
                                }
                            }
                            std::collections::hash_map::Entry::Vacant(entry) => {
                                entry.insert(value);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Merge an entire region using write-if-AIR semantics (halo zone).
    fn merge_region_write_if_air(self_region: &mut RegionToModify, other_region: RegionToModify) {
        for ((chunk_lx, chunk_lz), other_chunk) in other_region.chunks {
            let self_chunk = self_region.chunks.entry((chunk_lx, chunk_lz)).or_default();

            for (section_y, other_section) in other_chunk.sections {
                Self::merge_section_write_if_air(
                    self_chunk.sections.entry(section_y).or_default(),
                    &other_section,
                );
            }

            // Append entities/block_entities from halo
            for (key, value) in other_chunk.other {
                if key == "block_entities" || key == "entities" {
                    match self_chunk.other.entry(key) {
                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                            if let Value::List(self_list) = entry.get_mut() {
                                if let Value::List(other_list) = &value {
                                    self_list.extend(other_list.iter().cloned());
                                }
                            }
                        }
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(value);
                        }
                    }
                }
            }
        }
    }

    /// Merge a single section using write-if-AIR (no coordinate checks).
    fn merge_section_write_if_air(
        self_section: &mut SectionToModify,
        other_section: &SectionToModify,
    ) {
        match &other_section.storage {
            BlockStorage::Uniform(block) if *block == AIR => {}
            BlockStorage::Uniform(block) => {
                let block = *block;
                for idx in 0..4096usize {
                    if self_section.storage.get(idx) == AIR {
                        self_section.storage.set(idx, block);
                        if let Some(props) = other_section.properties.get(&idx) {
                            self_section.properties.insert(idx, props.clone());
                        } else {
                            self_section.properties.remove(&idx);
                        }
                    }
                }
            }
            _ => {
                for (idx, block) in other_section.storage.iter().enumerate() {
                    if block == AIR {
                        continue;
                    }
                    if self_section.storage.get(idx) == AIR {
                        self_section.storage.set(idx, block);
                        if let Some(props) = other_section.properties.get(&idx) {
                            self_section.properties.insert(idx, props.clone());
                        } else {
                            self_section.properties.remove(&idx);
                        }
                    }
                }
            }
        }
    }

    /// Merge a section where the entire section is in the auth tile's region
    /// but the destination already has data from a prior tile's halo merge.
    ///
    /// Auth-tile non-AIR blocks always overwrite. Auth-tile AIR positions
    /// preserve whatever halo data was already written there.
    fn merge_section_auth_overwrite_nonair(
        self_section: &mut SectionToModify,
        other_section: &SectionToModify,
    ) {
        match &other_section.storage {
            BlockStorage::Uniform(block) if *block == AIR => {
                // Auth tile is entirely AIR in this section; keep all halo data.
            }
            BlockStorage::Uniform(block) => {
                // Auth tile is uniformly one non-AIR block; overwrite everything.
                let block = *block;
                for idx in 0..4096usize {
                    self_section.storage.set(idx, block);
                    if let Some(props) = other_section.properties.get(&idx) {
                        self_section.properties.insert(idx, props.clone());
                    } else {
                        self_section.properties.remove(&idx);
                    }
                }
            }
            _ => {
                for (idx, block) in other_section.storage.iter().enumerate() {
                    if block == AIR {
                        // Auth tile placed nothing here; preserve halo data.
                        continue;
                    }
                    self_section.storage.set(idx, block);
                    if let Some(props) = other_section.properties.get(&idx) {
                        self_section.properties.insert(idx, props.clone());
                    } else {
                        self_section.properties.remove(&idx);
                    }
                }
            }
        }
    }

    /// Merge a section with per-block authoritative bound checks (rare slow path).
    #[allow(clippy::too_many_arguments)]
    fn merge_section_with_auth_check(
        self_section: &mut SectionToModify,
        other_section: &SectionToModify,
        chunk_world_x: i32,
        chunk_world_z: i32,
        auth_min_x: i32,
        auth_min_z: i32,
        auth_max_x: i32,
        auth_max_z: i32,
    ) {
        match &other_section.storage {
            BlockStorage::Uniform(block) if *block == AIR => {}
            BlockStorage::Uniform(block) => {
                let block = *block;
                for idx in 0..4096usize {
                    let local_z = ((idx % 256) / 16) as i32;
                    let local_x = (idx % 16) as i32;
                    let world_x = chunk_world_x + local_x;
                    let world_z = chunk_world_z + local_z;

                    let is_auth = world_x >= auth_min_x
                        && world_x <= auth_max_x
                        && world_z >= auth_min_z
                        && world_z <= auth_max_z;

                    if is_auth || self_section.storage.get(idx) == AIR {
                        self_section.storage.set(idx, block);
                        if let Some(props) = other_section.properties.get(&idx) {
                            self_section.properties.insert(idx, props.clone());
                        } else {
                            self_section.properties.remove(&idx);
                        }
                    }
                }
            }
            _ => {
                for (idx, block) in other_section.storage.iter().enumerate() {
                    if block == AIR {
                        continue;
                    }
                    let local_z = ((idx % 256) / 16) as i32;
                    let local_x = (idx % 16) as i32;
                    let world_x = chunk_world_x + local_x;
                    let world_z = chunk_world_z + local_z;

                    let is_auth = world_x >= auth_min_x
                        && world_x <= auth_max_x
                        && world_z >= auth_min_z
                        && world_z <= auth_max_z;

                    if is_auth || self_section.storage.get(idx) == AIR {
                        self_section.storage.set(idx, block);
                        if let Some(props) = other_section.properties.get(&idx) {
                            self_section.properties.insert(idx, props.clone());
                        } else {
                            self_section.properties.remove(&idx);
                        }
                    }
                }
            }
        }
    }

    /// Scan every section and collapse any that are entirely one block type
    /// from `Full(Vec)` back to `Uniform(Block)`, freeing the 4 KiB allocation.
    pub fn compact_sections(&mut self) {
        for region in self.regions.values_mut() {
            for chunk in region.chunks.values_mut() {
                for section in chunk.sections.values_mut() {
                    if !matches!(&section.storage, BlockStorage::Uniform(_)) {
                        section.compact();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_id_storage_round_trips() {
        // Writing a wide (>= 256) id upgrades Full(u8) -> FullWide and round-trips exactly.
        let mut s = BlockStorage::Uniform(AIR);
        s.set(0, STONE);
        assert!(matches!(s, BlockStorage::Full(_)));
        s.set(1, KELP);
        assert!(matches!(s, BlockStorage::FullWide(_)));
        assert_eq!(s.get(0), STONE);
        assert_eq!(s.get(1), KELP);
        assert_eq!(s.iter().nth(1), Some(KELP));

        // A wide block straight from Uniform, then a uniform fill, compacts back.
        let mut w = BlockStorage::Uniform(AIR);
        w.set(0, SOUL_SAND);
        assert!(matches!(w, BlockStorage::FullWide(_)));
        for i in 0..4096 {
            w.set(i, SOUL_SAND);
        }
        w.try_compact();
        assert!(matches!(w, BlockStorage::Uniform(_)));
        assert_eq!(w.get(7), SOUL_SAND);
    }

    #[test]
    fn bulk_fill_empty_chunk_all_clean() {
        let mut world = WorldToModify::default();
        let all_clean = world.bulk_fill_chunk_sections_below(0, 0, -2, STONE);
        assert!(all_clean, "fresh chunk should report all sections clean");

        let region = world.get_region(0, 0).unwrap();
        let chunk = region.get_chunk(0, 0).unwrap();
        // Sections -4, -3, -2 must now exist as Uniform(STONE)
        for y in MIN_SECTION_Y..=-2 {
            let section = chunk
                .sections
                .get(&y)
                .unwrap_or_else(|| panic!("section {y} should have been created"));
            assert!(
                matches!(&section.storage, BlockStorage::Uniform(b) if *b == STONE),
                "section {y} should be Uniform(STONE), got {:?}",
                std::mem::discriminant(&section.storage)
            );
            assert!(
                section.properties.is_empty(),
                "section {y} should have no per-cell properties"
            );
        }
    }

    #[test]
    fn bulk_fill_skips_occupied_section() {
        let mut world = WorldToModify::default();
        // Pre-place a non-AIR block deep underground (section -2: y=-32..=-17)
        // to simulate e.g. a bridge pier.
        world.set_block_if_absent(0, -20, 0, COBBLESTONE);

        let all_clean = world.bulk_fill_chunk_sections_below(0, 0, -2, STONE);
        assert!(
            !all_clean,
            "should return false because section -2 was occupied"
        );

        let region = world.get_region(0, 0).unwrap();
        let chunk = region.get_chunk(0, 0).unwrap();
        // Section -4 and -3 should be Uniform(STONE)
        for y in [-4i8, -3] {
            let section = chunk.sections.get(&y).unwrap();
            assert!(
                matches!(&section.storage, BlockStorage::Uniform(b) if *b == STONE),
                "section {y} should be Uniform(STONE)"
            );
        }
        // Section -2 should be left alone (Full(Vec) with COBBLESTONE at y=-20)
        let section = chunk.sections.get(&-2).unwrap();
        assert!(
            matches!(&section.storage, BlockStorage::Full(_)),
            "section -2 should still be Full(Vec) (had COBBLESTONE)"
        );
        // The pre-existing block must still be there
        let local_y = (-20i32 & 15) as u8;
        let idx = SectionToModify::index(0, local_y, 0);
        assert_eq!(
            section.storage.get(idx),
            COBBLESTONE,
            "pre-existing COBBLESTONE must not be overwritten"
        );
    }

    #[test]
    fn bulk_fill_below_min_section_is_noop() {
        let mut world = WorldToModify::default();
        let all_clean = world.bulk_fill_chunk_sections_below(0, 0, MIN_SECTION_Y - 1, STONE);
        assert!(all_clean, "below-min request should be vacuously clean");
        // No region should have been created
        assert!(world.get_region(0, 0).is_none());
    }

    #[test]
    fn bulk_fill_second_call_treats_existing_stone_as_occupied() {
        // The "empty" check is strict Uniform(AIR). A second bulk-fill call
        // on already-Uniform(STONE) sections sees them as occupied (returns
        // false) but leaves them in their correct final state — calling
        // bulk_fill twice is harmless.
        let mut world = WorldToModify::default();
        assert!(world.bulk_fill_chunk_sections_below(0, 0, -2, STONE));
        let second = world.bulk_fill_chunk_sections_below(0, 0, -2, STONE);
        assert!(!second, "second call sees Uniform(STONE) as occupied");
        let chunk = world.get_region(0, 0).unwrap().get_chunk(0, 0).unwrap();
        for y in MIN_SECTION_Y..=-2 {
            let section = chunk.sections.get(&y).unwrap();
            assert!(
                matches!(&section.storage, BlockStorage::Uniform(b) if *b == STONE),
                "section {y} should still be Uniform(STONE)"
            );
        }
    }

    #[test]
    fn set_with_props_if_absent_writes_then_protects_occupied() {
        let mut world = WorldToModify::default();
        let first = BlockWithProperties {
            block: STONE,
            properties: None,
        };
        world.set_with_props_if_absent(5, 70, 9, first);
        assert_eq!(world.get_block(5, 70, 9), Some(STONE));

        // A second write to the now-occupied cell must be ignored (the None/None contract).
        let second = BlockWithProperties {
            block: COBBLESTONE,
            properties: None,
        };
        world.set_with_props_if_absent(5, 70, 9, second);
        assert_eq!(
            world.get_block(5, 70, 9),
            Some(STONE),
            "occupied cell must not be overwritten"
        );
    }

    #[test]
    fn set_block_if_absent_delegates_with_same_semantics() {
        let mut world = WorldToModify::default();
        world.set_block_if_absent(1, 64, 2, STONE);
        assert_eq!(world.get_block(1, 64, 2), Some(STONE));
        world.set_block_if_absent(1, 64, 2, COBBLESTONE);
        assert_eq!(
            world.get_block(1, 64, 2),
            Some(STONE),
            "delegating wrapper must preserve set-if-absent behaviour"
        );
    }

    #[test]
    fn set_with_props_if_absent_stores_and_omits_properties() {
        let mut world = WorldToModify::default();
        // y=64 → section index 4, local_y 0.
        let section_idx = 4i8;
        let local_y = (64 & 15) as u8;

        let with_props = BlockWithProperties {
            block: STONE,
            properties: Some(std::sync::Arc::new(Value::Int(7))),
        };
        world.set_with_props_if_absent(0, 64, 0, with_props);
        // A no-properties write to a different empty cell.
        world.set_block_if_absent(1, 64, 0, STONE);

        let chunk = world.get_region(0, 0).unwrap().get_chunk(0, 0).unwrap();
        let section = chunk.sections.get(&section_idx).unwrap();
        assert!(
            section
                .properties
                .contains_key(&SectionToModify::index(0, local_y, 0)),
            "block written with properties should store them"
        );
        assert!(
            !section
                .properties
                .contains_key(&SectionToModify::index(1, local_y, 0)),
            "block written without properties should leave none"
        );
    }
}
