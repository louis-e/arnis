//! Common data structures for world modification.
//!
//! This module contains the internal data structures used to track block changes
//! before they are written to either Java or Bedrock format.

use crate::block_definitions::*;

use std::sync::atomic::{AtomicI32, AtomicU16, Ordering as MemOrdering};

/// Default (vanilla 1.18+) world floor.
pub const DEFAULT_MIN_Y: i32 = -64;

/// Blocks of solid ground kept under the local surface once the floor is extended. Without
/// this, an extended floor would make every bedrock/fill/ore column ~4000 blocks deep.
pub const TERRAIN_FLOOR_DEPTH: i32 = 64;

/// Default (vanilla 1.18+) world ceiling; the editor stores nothing above `world_max_y()`.
pub const DEFAULT_MAX_Y: i32 = 319;

static WORLD_MIN_Y: AtomicI32 = AtomicI32::new(DEFAULT_MIN_Y);
static WORLD_MAX_Y: AtomicI32 = AtomicI32::new(DEFAULT_MAX_Y);

/// Serialises tests that mutate the world-bounds globals against tests that read them.
#[cfg(test)]
pub(crate) static FLOOR_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Set the world bounds once, at startup, from the CLI args.
///
/// These must describe the dimension the engine is told about — vanilla, or the tall
/// datapack — because chunk serialization sizes heightmaps from the span between them and
/// offsets every value from the floor. Set as a pair so the two cannot drift apart.
///
/// Asserted in release too: bounds off a section boundary silently corrupt every section
/// index derived from them, which is far worse than failing on the spot. Runs once per world.
pub fn set_world_bounds(min: i32, max: i32) {
    assert_eq!(
        min.rem_euclid(16),
        0,
        "world floor must be a multiple of 16"
    );
    assert_eq!(max.rem_euclid(16), 15, "world ceiling must end a section");
    assert!(min < max, "world floor must sit below the ceiling");
    WORLD_MIN_Y.store(min, MemOrdering::Relaxed);
    WORLD_MAX_Y.store(max, MemOrdering::Relaxed);
}

/// Lowest legal Y in this world: -64, or the pack-extended floor (-2032 Java / -512 Bedrock).
#[inline(always)]
pub fn min_y() -> i32 {
    WORLD_MIN_Y.load(MemOrdering::Relaxed)
}

/// Highest legal Y in this world: 319, or the pack-extended ceiling (2031 Java).
#[inline(always)]
pub fn world_max_y() -> i32 {
    WORLD_MAX_Y.load(MemOrdering::Relaxed)
}

/// Section span of the configured dimension, as `(min_section, max_section)`.
///
/// Chunk serialization must write `yPos` and heightmaps against this, never against whichever
/// sections a chunk happens to contain: Minecraft sizes the heightmap bit width from the
/// dimension's height and offsets each value from its floor. A content-derived span usually
/// mismatches the expected array length — the engine warns and recomputes — but when the two
/// coincidentally agree it accepts the values silently, off by the difference in origin.
pub fn world_section_range() -> (i8, i8) {
    ((min_y() >> 4) as i8, (world_max_y() >> 4) as i8)
}

/// Lowest section index covering `min_y()`. Callers derive their own bottom section from the
/// terrain floor instead, so this only serves the tests.
#[cfg(test)]
pub fn min_section_y() -> i8 {
    (min_y() >> 4) as i8
}

/// Default terrain base (the vanilla -64 floor plus its bedrock layer).
pub const DEFAULT_GROUND_LEVEL: i32 = -62;

static TERRAIN_FLOOR_Y: AtomicI32 = AtomicI32::new(DEFAULT_MIN_Y);

/// Set the terrain floor from the terrain base, once the elevation scaler has settled it.
///
/// This is a WORLD CONSTANT, not a per-column value: bedrock has to be a flat plane. Anchoring
/// it to each column's own surface would make it a wavy shell with void underneath.
///
/// It sits `TERRAIN_FLOOR_DEPTH` below the base (the lowest point terrain can reach), clamped
/// to the world floor. With the vanilla floor that clamp always wins, so this is exactly the
/// old constant -64 and nothing changes. With an extended floor it keeps the filled column a
/// bounded depth instead of ~4000 blocks, and keeps the chunk's section span tight.
///
/// Snapped down to a section boundary, which the old `MIN_Y = -64` was by construction. The
/// `--fillground` fast path bulk-fills whole sections from this floor's section and lets the
/// bedrock plane overwrite the bottom layer; an unaligned floor (base -62 gives -126) would
/// leave the rest of that section, here Y -128 and -127, as stone underneath the bedrock.
pub fn set_terrain_floor_y(ground_level: i32) {
    let floor = min_y().max(ground_level.saturating_sub(TERRAIN_FLOOR_DEPTH));
    let aligned = floor.div_euclid(16) * 16;
    TERRAIN_FLOOR_Y.store(aligned.max(min_y()), MemOrdering::Relaxed);
}

/// Y of the bedrock plane, and the bottom of `--fillground` / ore generation.
#[inline(always)]
pub fn terrain_floor_y() -> i32 {
    TERRAIN_FLOOR_Y.load(MemOrdering::Relaxed)
}

static BASE_CHUNK_Y: AtomicI32 = AtomicI32::new(DEFAULT_GROUND_LEVEL);

/// Y of the plane used for out-of-bbox filler chunks. Follows the terrain base, which
/// sinks when the relief needs the extended floor; otherwise the filler would be a plane
/// floating up to ~2000 blocks above the terrain it is supposed to border.
pub fn set_base_chunk_y(y: i32) {
    BASE_CHUNK_Y.store(y, MemOrdering::Relaxed);
}

#[inline]
pub fn base_chunk_y() -> i32 {
    BASE_CHUNK_Y.load(MemOrdering::Relaxed)
}

static TERRAIN_TOP_Y: AtomicI32 = AtomicI32::new(DEFAULT_GROUND_LEVEL);

/// Highest Y the elevation scaler placed terrain at. Consumers that need the band the terrain
/// actually fills read this rather than the dimension ceiling, which the relief rarely reaches.
pub fn set_terrain_top_y(y: i32) {
    TERRAIN_TOP_Y.store(y, MemOrdering::Relaxed);
}

#[inline]
pub fn terrain_top_y() -> i32 {
    TERRAIN_TOP_Y.load(MemOrdering::Relaxed)
}

static BASE_CHUNK_BLOCK: AtomicU16 = AtomicU16::new(crate::block_definitions::GRASS_BLOCK.id());

/// Surface block for those filler chunks; grass would ring a lunar world in green.
pub fn set_base_chunk_block(block: crate::block_definitions::Block) {
    BASE_CHUNK_BLOCK.store(block.id(), MemOrdering::Relaxed);
}

#[inline]
pub fn base_chunk_block() -> crate::block_definitions::Block {
    crate::block_definitions::Block::from_raw_id(BASE_CHUNK_BLOCK.load(MemOrdering::Relaxed))
}

use fastnbt::{LongArray, Value};
use fnv::{FnvHashMap, FnvHashSet};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const SECTION_BLOCKS: usize = 4096;
const DENSE_ID_LIMIT: u16 = u8::MAX as u16 + 1;
const DENSE_BLOCK_IDS: usize = DENSE_ID_LIMIT as usize;
const MAX_SECTION_PALETTE: usize = 256;
const AIR_ID: u8 = AIR.id() as u8;
const RECENT_PALETTE_LOOKUPS: usize = 4;
const REVERSE_LOOKUP_THRESHOLD: usize = 32;
const REVERSE_LOOKUP_ENTRY_BYTES: u64 = 8;

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

/// Section build counts for `--benchmark`. Gated, because `to_section` runs millions
/// of times across every save thread and an unconditional `fetch_add` would contend.
static BUILT_SECTIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SLOW_PATH_SECTIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SECTION_COUNTERS_ON: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Enable and zero the counters, so a second run in one process starts clean.
pub(crate) fn reset_section_counters(on: bool) {
    use std::sync::atomic::Ordering::Relaxed;
    BUILT_SECTIONS.store(0, Relaxed);
    SLOW_PATH_SECTIONS.store(0, Relaxed);
    SECTION_COUNTERS_ON.store(on, Relaxed);
}

/// Sections built and sections that took the property slow path.
pub(crate) fn section_counters() -> (u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    (
        BUILT_SECTIONS.load(Relaxed),
        SLOW_PATH_SECTIONS.load(Relaxed),
    )
}

#[derive(Copy, Clone)]
struct RecentPaletteSlot {
    block: Block,
    slot: u8,
}

impl RecentPaletteSlot {
    const EMPTY: Self = Self {
        block: AIR,
        slot: 0,
    };
}

#[derive(Clone)]
pub(crate) struct PalettedBlockStorage {
    indices: [u8; SECTION_BLOCKS],
    palette: Vec<Block>,
    reverse: Option<FnvHashMap<Block, u8>>,
    recent: [RecentPaletteSlot; RECENT_PALETTE_LOOKUPS],
    recent_len: u8,
}

impl PalettedBlockStorage {
    fn from_uniform(block: Block) -> Self {
        let mut recent = [RecentPaletteSlot::EMPTY; RECENT_PALETTE_LOOKUPS];
        recent[0] = RecentPaletteSlot { block, slot: 0 };
        Self {
            indices: [0; SECTION_BLOCKS],
            palette: vec![block],
            reverse: None,
            recent,
            recent_len: 1,
        }
    }

    #[inline(always)]
    fn get(&self, index: usize) -> Block {
        self.palette[self.indices[index] as usize]
    }

    #[inline]
    fn set(&mut self, index: usize, block: Block) -> Option<Box<[Block; SECTION_BLOCKS]>> {
        let current_slot = self.indices[index];
        if self.palette[current_slot as usize] == block {
            return None;
        }

        if let Some(slot) = self.find_slot(block) {
            self.indices[index] = slot;
            return None;
        }

        if self.palette.len() == MAX_SECTION_PALETTE {
            self.repack_live_palette_ignoring(index);
            if let Some(slot) = self.find_slot(block) {
                self.indices[index] = slot;
                return None;
            }
            if self.palette.len() == MAX_SECTION_PALETTE {
                return Some(self.promote_to_direct(index, block));
            }
        }

        let slot = self.insert(block);
        self.indices[index] = slot;
        None
    }

    #[inline]
    fn resident_bytes(&self) -> u64 {
        let mut bytes = std::mem::size_of::<Self>() as u64;
        bytes += (self.palette.capacity() * std::mem::size_of::<Block>()) as u64;
        if let Some(reverse) = &self.reverse {
            bytes += reverse.capacity() as u64 * REVERSE_LOOKUP_ENTRY_BYTES;
        }
        bytes
    }

    fn from_dense(ids: &[u8; SECTION_BLOCKS]) -> Self {
        let mut palette = Vec::new();
        let mut slot_by_id = [u16::MAX; DENSE_BLOCK_IDS];
        let mut indices = [0u8; SECTION_BLOCKS];

        for (i, &id) in ids.iter().enumerate() {
            let entry = &mut slot_by_id[id as usize];
            if *entry == u16::MAX {
                *entry = palette.len() as u16;
                palette.push(Block::from_raw_id(u16::from(id)));
            }
            indices[i] = *entry as u8;
        }

        let reverse = if palette.len() >= REVERSE_LOOKUP_THRESHOLD {
            let mut reverse = FnvHashMap::default();
            reverse.reserve(palette.len());
            for (slot, &block) in palette.iter().enumerate() {
                reverse.insert(block, slot as u8);
            }
            Some(reverse)
        } else {
            None
        };
        let mut storage = Self {
            indices,
            palette,
            reverse,
            recent: [RecentPaletteSlot::EMPTY; RECENT_PALETTE_LOOKUPS],
            recent_len: 0,
        };
        if let Some(&block) = storage.palette.first() {
            storage.touch_recent(block, 0);
        }
        storage
    }

    fn try_from_direct(blocks: &[Block; SECTION_BLOCKS]) -> Option<Self> {
        let mut palette = Vec::new();
        let mut reverse = FnvHashMap::default();
        let mut indices = [0u8; SECTION_BLOCKS];

        for (i, &block) in blocks.iter().enumerate() {
            let slot = match reverse.get(&block).copied() {
                Some(slot) => slot,
                None => {
                    if palette.len() == MAX_SECTION_PALETTE {
                        return None;
                    }
                    let slot = palette.len() as u8;
                    palette.push(block);
                    reverse.insert(block, slot);
                    slot
                }
            };
            indices[i] = slot;
        }

        let reverse = (palette.len() >= REVERSE_LOOKUP_THRESHOLD).then_some(reverse);
        let mut storage = Self {
            indices,
            palette,
            reverse,
            recent: [RecentPaletteSlot::EMPTY; RECENT_PALETTE_LOOKUPS],
            recent_len: 0,
        };
        if let Some(&block) = storage.palette.first() {
            storage.touch_recent(block, 0);
        }
        Some(storage)
    }

    fn try_to_dense(&self) -> Option<Box<[u8; SECTION_BLOCKS]>> {
        let mut dense_palette = [0u8; MAX_SECTION_PALETTE];
        for (slot, &block) in self.palette.iter().enumerate() {
            dense_palette[slot] = u8::try_from(block.id()).ok()?;
        }

        let mut dense = Box::new([0u8; SECTION_BLOCKS]);
        for (i, &slot) in self.indices.iter().enumerate() {
            dense[i] = dense_palette[slot as usize];
        }
        Some(dense)
    }

    fn find_slot(&mut self, block: Block) -> Option<u8> {
        if let Some(slot) = self.recent[..self.recent_len as usize]
            .iter()
            .find_map(|entry| {
                (entry.block == block
                    && self.palette.get(entry.slot as usize).copied() == Some(block))
                .then_some(entry.slot)
            })
        {
            self.touch_recent(block, slot);
            return Some(slot);
        }

        if self.palette.len() >= REVERSE_LOOKUP_THRESHOLD {
            if self.reverse.is_none() {
                self.rebuild_reverse_lookup();
            }
            if let Some(slot) = self
                .reverse
                .as_ref()
                .and_then(|reverse| reverse.get(&block).copied())
            {
                self.touch_recent(block, slot);
                return Some(slot);
            }
            return None;
        }

        let slot = self
            .palette
            .iter()
            .position(|&candidate| candidate == block)? as u8;
        self.touch_recent(block, slot);
        Some(slot)
    }

    fn insert(&mut self, block: Block) -> u8 {
        debug_assert!(self.palette.len() < MAX_SECTION_PALETTE);
        let slot = self.palette.len() as u8;
        self.palette.push(block);
        if let Some(reverse) = &mut self.reverse {
            reverse.insert(block, slot);
        } else if self.palette.len() >= REVERSE_LOOKUP_THRESHOLD {
            self.rebuild_reverse_lookup();
        }
        self.touch_recent(block, slot);
        slot
    }

    fn touch_recent(&mut self, block: Block, slot: u8) {
        let len = self.recent_len as usize;
        if let Some(pos) = self.recent[..len]
            .iter()
            .position(|entry| entry.block == block && entry.slot == slot)
        {
            let entry = self.recent[pos];
            for i in (0..pos).rev() {
                self.recent[i + 1] = self.recent[i];
            }
            self.recent[0] = entry;
            return;
        }

        let capped = len.min(RECENT_PALETTE_LOOKUPS - 1);
        for i in (0..capped).rev() {
            self.recent[i + 1] = self.recent[i];
        }
        self.recent[0] = RecentPaletteSlot { block, slot };
        if len < RECENT_PALETTE_LOOKUPS {
            self.recent_len += 1;
        }
    }

    fn clear_recent(&mut self) {
        self.recent = [RecentPaletteSlot::EMPTY; RECENT_PALETTE_LOOKUPS];
        self.recent_len = 0;
    }

    fn rebuild_reverse_lookup(&mut self) {
        let mut reverse = FnvHashMap::default();
        reverse.reserve(self.palette.len());
        for (slot, &block) in self.palette.iter().enumerate() {
            reverse.insert(block, slot as u8);
        }
        self.reverse = Some(reverse);
    }

    fn repack_live_palette(&mut self) {
        self.repack_live_palette_inner(None);
    }

    fn repack_live_palette_ignoring(&mut self, index: usize) {
        self.repack_live_palette_inner(Some(index));
    }

    fn repack_live_palette_inner(&mut self, skipped_index: Option<usize>) {
        let mut used = [false; MAX_SECTION_PALETTE];
        for (i, &slot) in self.indices.iter().enumerate() {
            if skipped_index == Some(i) {
                continue;
            }
            used[slot as usize] = true;
        }

        let live = used[..self.palette.len()]
            .iter()
            .filter(|&&is_used| is_used)
            .count();
        if live == self.palette.len() {
            if self.palette.len() < REVERSE_LOOKUP_THRESHOLD {
                self.reverse = None;
            }
            return;
        }

        let old_palette = self.palette.clone();
        let mut remap = [u8::MAX; MAX_SECTION_PALETTE];
        let mut new_palette = Vec::with_capacity(live);
        for (old_slot, &block) in old_palette.iter().enumerate() {
            if used[old_slot] {
                let new_slot = new_palette.len() as u8;
                remap[old_slot] = new_slot;
                new_palette.push(block);
            }
        }
        for (i, slot) in self.indices.iter_mut().enumerate() {
            if skipped_index == Some(i) {
                *slot = 0;
            } else {
                *slot = remap[*slot as usize];
            }
        }

        self.palette = new_palette;
        if self.palette.len() >= REVERSE_LOOKUP_THRESHOLD {
            self.rebuild_reverse_lookup();
        } else {
            self.reverse = None;
        }
        self.clear_recent();
        if let Some(&block) = self.palette.first() {
            self.touch_recent(block, 0);
        }
    }

    fn promote_to_direct(&self, index: usize, block: Block) -> Box<[Block; SECTION_BLOCKS]> {
        let mut direct = Box::new([AIR; SECTION_BLOCKS]);
        for (i, &slot) in self.indices.iter().enumerate() {
            direct[i] = self.palette[slot as usize];
        }
        direct[index] = block;
        direct
    }
}

/// Block storage strategy for a 16×16×16 section.
///
/// `Uniform` keeps untouched or bulk-filled sections allocation-free.
///
/// `Paletted` stores one byte per cell plus a section-local block list, so the
/// memory cost depends on how many distinct blocks are live in this section,
/// not on their raw u16 ids.
///
/// `Direct` is the rare fallback for sections that truly need more than 256
/// distinct live blocks at once.
#[derive(Clone)]
pub(crate) enum BlockStorage {
    /// Every position is the same block (commonly AIR).
    Uniform(Block),
    /// Mixed blocks whose live ids all fit in one byte.
    Dense(Box<[u8; SECTION_BLOCKS]>),
    /// Mixed blocks with a per-section palette and one-byte indices.
    Paletted(Box<PalettedBlockStorage>),
    /// Rare overflow path when a section needs more than 256 distinct live blocks.
    Direct(Box<[Block; SECTION_BLOCKS]>),
}

impl BlockStorage {
    /// Read block at flat `index` (0..4095).
    #[inline(always)]
    pub fn get(&self, index: usize) -> Block {
        match self {
            BlockStorage::Uniform(b) => *b,
            BlockStorage::Dense(v) => Block::from_raw_id(u16::from(v[index])),
            BlockStorage::Paletted(storage) => storage.get(index),
            BlockStorage::Direct(v) => v[index],
        }
    }

    /// Write block at flat `index`. Promotes `Uniform` to paletted storage on
    /// the first differing write, and only falls back to direct storage when a
    /// section genuinely exceeds 256 live block kinds.
    #[inline]
    pub fn set(&mut self, index: usize, block: Block) {
        match self {
            BlockStorage::Uniform(b) if *b == block => {
                // No-op – writing the same value.
            }
            BlockStorage::Uniform(base) => {
                if let (Ok(base_id), Ok(block_id)) =
                    (u8::try_from(base.id()), u8::try_from(block.id()))
                {
                    let mut dense = Box::new([base_id; SECTION_BLOCKS]);
                    dense[index] = block_id;
                    *self = BlockStorage::Dense(dense);
                } else {
                    let mut storage = PalettedBlockStorage::from_uniform(*base);
                    let promoted = storage.set(index, block);
                    debug_assert!(promoted.is_none(), "fresh paletted section cannot overflow");
                    *self = BlockStorage::Paletted(Box::new(storage));
                }
            }
            BlockStorage::Dense(v) => {
                if let Ok(block_id) = u8::try_from(block.id()) {
                    v[index] = block_id;
                } else {
                    let mut storage = PalettedBlockStorage::from_dense(v);
                    if let Some(direct) = storage.set(index, block) {
                        *self = BlockStorage::Direct(direct);
                    } else {
                        *self = BlockStorage::Paletted(Box::new(storage));
                    }
                }
            }
            BlockStorage::Paletted(storage) => {
                if let Some(direct) = storage.set(index, block) {
                    *self = BlockStorage::Direct(direct);
                }
            }
            BlockStorage::Direct(v) => {
                v[index] = block;
            }
        }
    }

    /// Iterate over all 4 096 blocks.
    #[inline]
    pub fn iter(&self) -> BlockStorageIter<'_> {
        match self {
            BlockStorage::Uniform(b) => BlockStorageIter::Uniform(*b, 0),
            BlockStorage::Dense(v) => BlockStorageIter::Dense(v.iter()),
            BlockStorage::Paletted(storage) => BlockStorageIter::Paletted {
                palette: &storage.palette,
                indices: storage.indices.iter(),
            },
            BlockStorage::Direct(v) => BlockStorageIter::Direct(v.iter()),
        }
    }

    /// Try to collapse a mixed section back to `Uniform` if every entry
    /// is the same block. Frees the heap allocation.
    pub fn try_compact(&mut self) {
        match self {
            BlockStorage::Dense(v) => {
                if let Some(&first) = v.first() {
                    if v.iter().all(|&b| b == first) {
                        *self = BlockStorage::Uniform(Block::from_raw_id(u16::from(first)));
                    }
                }
            }
            BlockStorage::Paletted(storage) => {
                storage.repack_live_palette();
                if storage.palette.len() == 1 {
                    *self = BlockStorage::Uniform(storage.palette[0]);
                } else if let Some(dense) = storage.try_to_dense() {
                    *self = BlockStorage::Dense(dense);
                }
            }
            BlockStorage::Direct(v) => {
                if let Some(&first) = v.first() {
                    if v.iter().all(|&b| b == first) {
                        *self = BlockStorage::Uniform(first);
                        return;
                    }
                }
                if let Some(dense) = dense_storage_from_blocks(v) {
                    *self = BlockStorage::Dense(dense);
                } else if let Some(paletted) = PalettedBlockStorage::try_from_direct(v) {
                    if paletted.palette.len() == 1 {
                        *self = BlockStorage::Uniform(paletted.palette[0]);
                    } else {
                        *self = BlockStorage::Paletted(Box::new(paletted));
                    }
                }
            }
            BlockStorage::Uniform(_) => {}
        }
    }

    #[inline]
    pub fn resident_bytes(&self) -> u64 {
        match self {
            BlockStorage::Uniform(_) => 0,
            BlockStorage::Dense(_) => SECTION_BLOCKS as u64,
            BlockStorage::Paletted(storage) => storage.resident_bytes(),
            BlockStorage::Direct(_) => std::mem::size_of::<[Block; SECTION_BLOCKS]>() as u64,
        }
    }
}

/// Iterator returned by [`BlockStorage::iter`].
pub(crate) enum BlockStorageIter<'a> {
    Uniform(Block, usize),
    Dense(std::slice::Iter<'a, u8>),
    Paletted {
        palette: &'a [Block],
        indices: std::slice::Iter<'a, u8>,
    },
    Direct(std::slice::Iter<'a, Block>),
}

impl<'a> Iterator for BlockStorageIter<'a> {
    type Item = Block;

    #[inline]
    fn next(&mut self) -> Option<Block> {
        match self {
            BlockStorageIter::Uniform(b, count) => {
                if *count < SECTION_BLOCKS {
                    *count += 1;
                    Some(*b)
                } else {
                    None
                }
            }
            BlockStorageIter::Dense(it) => it.next().map(|&id| Block::from_raw_id(u16::from(id))),
            BlockStorageIter::Paletted { palette, indices } => {
                indices.next().map(|slot| palette[*slot as usize])
            }
            BlockStorageIter::Direct(it) => it.next().copied(),
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let rem = match self {
            BlockStorageIter::Uniform(_, c) => SECTION_BLOCKS - *c,
            BlockStorageIter::Dense(it) => it.len(),
            BlockStorageIter::Paletted { indices, .. } => indices.len(),
            BlockStorageIter::Direct(it) => it.len(),
        };
        (rem, Some(rem))
    }
}

impl ExactSizeIterator for BlockStorageIter<'_> {}

#[inline]
fn bits_for_palette_len(palette_len: usize) -> usize {
    let mut bits = 4;
    while (1usize << bits) < palette_len.max(1) {
        bits += 1;
    }
    bits
}

fn dense_storage_from_blocks(
    blocks: &[Block; SECTION_BLOCKS],
) -> Option<Box<[u8; SECTION_BLOCKS]>> {
    let mut dense = Box::new([0u8; SECTION_BLOCKS]);
    for (i, &block) in blocks.iter().enumerate() {
        dense[i] = u8::try_from(block.id()).ok()?;
    }
    Some(dense)
}

fn pack_palette_indices(indices: &[u16; SECTION_BLOCKS], bits_per_block: usize) -> LongArray {
    let mut data = Vec::new();
    let mut cur: i64 = 0;
    let mut cur_idx = 0usize;

    for &palette_index in indices {
        if cur_idx + bits_per_block > 64 {
            data.push(cur);
            cur = 0;
            cur_idx = 0;
        }

        cur |= i64::from(palette_index) << cur_idx;
        cur_idx += bits_per_block;
    }

    if cur_idx > 0 {
        data.push(cur);
    }

    LongArray::new(data)
}

fn make_palette_item(block: Block, stored_props: Option<&Arc<Value>>) -> PaletteItem {
    PaletteItem {
        name: format!("{}:{}", block.namespace(), block.name()),
        properties: stored_props
            .map(|p| (**p).clone())
            .or_else(|| block.properties()),
    }
}

fn make_section(y: i8, palette: Vec<PaletteItem>, data: Option<LongArray>) -> Section {
    Section {
        block_states: Blockstates {
            palette,
            data,
            other: FnvHashMap::default(),
        },
        y,
        other: FnvHashMap::default(),
    }
}

/// Bulk form of a per-cell merge's property updates: every `written` cell takes `src`'s
/// properties or loses its own; nothing else changes. Decide `written` before any write.
fn reconcile_properties(
    props: &mut FnvHashMap<usize, Arc<Value>>,
    src: FnvHashMap<usize, Arc<Value>>,
    written: impl Fn(usize) -> bool,
) {
    if !props.is_empty() {
        props.retain(|&i, _| !written(i));
    }
    for (i, p) in src {
        if written(i) {
            props.insert(i, p);
        }
    }
}

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
        self.clear_properties_at(index);
    }

    /// Drops any properties at `index`; hashbrown hashes the key even when the map is empty.
    #[inline(always)]
    fn clear_properties_at(&mut self, index: usize) {
        if !self.properties.is_empty() {
            self.properties.remove(&index);
        }
    }

    /// Mirrors `src`'s properties at `index`: copied if present, cleared otherwise.
    #[inline(always)]
    fn copy_properties_at(&mut self, index: usize, src: &FnvHashMap<usize, Arc<Value>>) {
        let props = if src.is_empty() {
            None
        } else {
            src.get(&index)
        };
        match props {
            Some(props) => {
                self.properties.insert(index, props.clone());
            }
            None => self.clear_properties_at(index),
        }
    }

    /// Drops properties on AIR cells, which the per-cell merge never copies across.
    fn retain_properties_on_blocks(&mut self) {
        if !self.properties.is_empty() {
            let storage = &self.storage;
            self.properties
                .retain(|&i, _| i < SECTION_BLOCKS && storage.get(i) != AIR);
        }
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
            self.clear_properties_at(index);
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
        let count = SECTION_COUNTERS_ON.load(std::sync::atomic::Ordering::Relaxed);
        if count {
            BUILT_SECTIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        // Fast path: Uniform section → single palette entry, no data array needed.
        // Only valid when no per-index properties exist, otherwise we must
        // fall through to the general path so every index is checked.
        if self.properties.is_empty() {
            if let BlockStorage::Uniform(block) = &self.storage {
                return make_section(y, vec![make_palette_item(*block, None)], None);
            }
        }

        // Medium path: mixed blocks with no per-index properties.
        if self.properties.is_empty() {
            match &self.storage {
                BlockStorage::Uniform(_) => {}
                BlockStorage::Dense(ids) => {
                    let mut id_to_palette = [u16::MAX; DENSE_BLOCK_IDS];
                    let mut palette_blocks: Vec<Block> = Vec::new();
                    let mut indices = [0u16; SECTION_BLOCKS];

                    for (i, &id) in ids.iter().enumerate() {
                        let entry = &mut id_to_palette[id as usize];
                        if *entry == u16::MAX {
                            *entry = palette_blocks.len() as u16;
                            palette_blocks.push(Block::from_raw_id(u16::from(id)));
                        }
                        indices[i] = *entry;
                    }

                    if palette_blocks.len() == 1 {
                        return make_section(
                            y,
                            vec![make_palette_item(palette_blocks[0], None)],
                            None,
                        );
                    }

                    let bits_per_block = bits_for_palette_len(palette_blocks.len());
                    let palette = palette_blocks
                        .into_iter()
                        .map(|block| make_palette_item(block, None))
                        .collect();

                    return make_section(
                        y,
                        palette,
                        Some(pack_palette_indices(&indices, bits_per_block)),
                    );
                }
                BlockStorage::Paletted(storage) => {
                    let mut slot_to_palette = [u16::MAX; MAX_SECTION_PALETTE];
                    let mut palette_blocks: Vec<Block> = Vec::with_capacity(storage.palette.len());
                    let mut indices = [0u16; SECTION_BLOCKS];

                    for (i, &slot) in storage.indices.iter().enumerate() {
                        let entry = &mut slot_to_palette[slot as usize];
                        if *entry == u16::MAX {
                            *entry = palette_blocks.len() as u16;
                            palette_blocks.push(storage.palette[slot as usize]);
                        }
                        indices[i] = *entry;
                    }

                    if palette_blocks.len() == 1 {
                        return make_section(
                            y,
                            vec![make_palette_item(palette_blocks[0], None)],
                            None,
                        );
                    }

                    let bits_per_block = bits_for_palette_len(palette_blocks.len());
                    let palette = palette_blocks
                        .into_iter()
                        .map(|block| make_palette_item(block, None))
                        .collect();

                    return make_section(
                        y,
                        palette,
                        Some(pack_palette_indices(&indices, bits_per_block)),
                    );
                }
                BlockStorage::Direct(blocks) => {
                    let mut block_to_palette: FnvHashMap<Block, u16> = FnvHashMap::default();
                    let mut palette_blocks: Vec<Block> = Vec::new();
                    let mut indices = [0u16; SECTION_BLOCKS];

                    for (i, &block) in blocks.iter().enumerate() {
                        let palette_index = match block_to_palette.entry(block) {
                            std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                            std::collections::hash_map::Entry::Vacant(e) => {
                                let idx = palette_blocks.len() as u16;
                                e.insert(idx);
                                palette_blocks.push(block);
                                idx
                            }
                        };
                        indices[i] = palette_index;
                    }

                    if palette_blocks.len() == 1 {
                        return make_section(
                            y,
                            vec![make_palette_item(palette_blocks[0], None)],
                            None,
                        );
                    }

                    let bits_per_block = bits_for_palette_len(palette_blocks.len());
                    let palette = palette_blocks
                        .into_iter()
                        .map(|block| make_palette_item(block, None))
                        .collect();

                    return make_section(
                        y,
                        palette,
                        Some(pack_palette_indices(&indices, bits_per_block)),
                    );
                }
            }
        }

        if count {
            SLOW_PATH_SECTIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        // Slow path: mixed blocks with per-index properties. Few cells carry any, so
        // resolve them up front into small ids (0 = none). That keys the per-cell lookup
        // on (Block, u16) and renders each compound's Debug string once, not 4096 times.
        let mut cell_props = [0u16; SECTION_BLOCKS];
        if !self.properties.is_empty() {
            // Borrowed for the whole call, so no Arc can be freed and its address reused.
            let mut props_by_ptr: FnvHashMap<usize, u16> = FnvHashMap::default();
            let mut props_by_repr: FnvHashMap<String, u16> = FnvHashMap::default();
            let mut next_id: u16 = 1;
            for (&i, p) in &self.properties {
                if i >= SECTION_BLOCKS {
                    continue;
                }
                let id = match props_by_ptr.entry(Arc::as_ptr(p) as usize) {
                    std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                    std::collections::hash_map::Entry::Vacant(e) => {
                        // Distinct Arcs with equal Debug output must still collapse.
                        let id = *props_by_repr.entry(format!("{p:?}")).or_insert_with(|| {
                            let id = next_id;
                            next_id += 1;
                            id
                        });
                        e.insert(id);
                        id
                    }
                };
                cell_props[i] = id;
            }
        }

        let mut unique_blocks: Vec<(Block, Option<Arc<Value>>)> = Vec::new();
        let mut indices = [0u16; SECTION_BLOCKS];

        match &self.storage {
            BlockStorage::Dense(ids) => {
                let mut plain_palette = [u16::MAX; DENSE_BLOCK_IDS];
                let mut props_palette: FnvHashMap<(u8, u16), u16> = FnvHashMap::default();

                for (i, &id) in ids.iter().enumerate() {
                    let props_id = cell_props[i];
                    let block = Block::from_raw_id(u16::from(id));
                    let palette_index = if props_id == 0 {
                        if plain_palette[id as usize] == u16::MAX {
                            plain_palette[id as usize] = unique_blocks.len() as u16;
                            unique_blocks.push((block, None));
                        }
                        plain_palette[id as usize]
                    } else {
                        match props_palette.entry((id, props_id)) {
                            std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                            std::collections::hash_map::Entry::Vacant(e) => {
                                let idx = unique_blocks.len() as u16;
                                e.insert(idx);
                                unique_blocks.push((block, self.properties.get(&i).cloned()));
                                idx
                            }
                        }
                    };
                    indices[i] = palette_index;
                }
            }
            BlockStorage::Paletted(storage) => {
                let mut plain_palette = [u16::MAX; MAX_SECTION_PALETTE];
                let mut props_palette: FnvHashMap<(u8, u16), u16> = FnvHashMap::default();

                for (i, &slot) in storage.indices.iter().enumerate() {
                    let props_id = cell_props[i];
                    let block = storage.palette[slot as usize];
                    let palette_index = if props_id == 0 {
                        if plain_palette[slot as usize] == u16::MAX {
                            plain_palette[slot as usize] = unique_blocks.len() as u16;
                            unique_blocks.push((block, None));
                        }
                        plain_palette[slot as usize]
                    } else {
                        match props_palette.entry((slot, props_id)) {
                            std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                            std::collections::hash_map::Entry::Vacant(e) => {
                                let idx = unique_blocks.len() as u16;
                                e.insert(idx);
                                unique_blocks.push((block, self.properties.get(&i).cloned()));
                                idx
                            }
                        }
                    };
                    indices[i] = palette_index;
                }
            }
            storage => {
                let mut plain_palette: FnvHashMap<Block, u16> = FnvHashMap::default();
                let mut props_palette: FnvHashMap<(Block, u16), u16> = FnvHashMap::default();

                for i in 0..SECTION_BLOCKS {
                    let block = storage.get(i);
                    let props_id = cell_props[i];
                    let palette_index = if props_id == 0 {
                        match plain_palette.entry(block) {
                            std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                            std::collections::hash_map::Entry::Vacant(e) => {
                                let idx = unique_blocks.len() as u16;
                                e.insert(idx);
                                unique_blocks.push((block, None));
                                idx
                            }
                        }
                    } else {
                        match props_palette.entry((block, props_id)) {
                            std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                            std::collections::hash_map::Entry::Vacant(e) => {
                                let idx = unique_blocks.len() as u16;
                                e.insert(idx);
                                unique_blocks.push((block, self.properties.get(&i).cloned()));
                                idx
                            }
                        }
                    };
                    indices[i] = palette_index;
                }
            }
        }

        if unique_blocks.len() == 1 {
            let (block, stored_props) = &unique_blocks[0];
            return make_section(
                y,
                vec![make_palette_item(*block, stored_props.as_ref())],
                None,
            );
        }

        let bits_per_block = bits_for_palette_len(unique_blocks.len());
        let palette = unique_blocks
            .iter()
            .map(|(block, stored_props)| make_palette_item(*block, stored_props.as_ref()))
            .collect();

        make_section(
            y,
            palette,
            Some(pack_palette_indices(&indices, bits_per_block)),
        )
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

/// A chunk's sections as a dense run of slots from the lowest present Y, so a lookup is an
/// index rather than a hash. Iterates in ascending Y.
#[derive(Default)]
pub(crate) struct SectionMap {
    base: i8,
    slots: Vec<Option<SectionToModify>>,
}

impl SectionMap {
    /// Slot of `y`; below `base` it wraps to an out-of-range index.
    #[inline(always)]
    fn offset(&self, y: i8) -> usize {
        (i32::from(y) - i32::from(self.base)) as usize
    }

    #[inline]
    pub fn get(&self, y: i8) -> Option<&SectionToModify> {
        self.slots.get(self.offset(y))?.as_ref()
    }

    #[inline]
    pub fn get_mut(&mut self, y: i8) -> Option<&mut SectionToModify> {
        let i = self.offset(y);
        self.slots.get_mut(i)?.as_mut()
    }

    #[inline]
    pub fn get_or_default(&mut self, y: i8) -> &mut SectionToModify {
        let mut i = self.offset(y);
        if i >= self.slots.len() {
            i = self.grow_to(y);
        }
        self.slots[i].get_or_insert_with(SectionToModify::default)
    }

    #[cold]
    fn grow_to(&mut self, y: i8) -> usize {
        if self.slots.is_empty() {
            self.base = y;
        } else if y < self.base {
            let n = (i32::from(self.base) - i32::from(y)) as usize;
            self.slots
                .splice(0..0, std::iter::repeat_with(|| None).take(n));
            self.base = y;
        }
        let i = self.offset(y);
        if i >= self.slots.len() {
            self.slots.resize_with(i + 1, || None);
        }
        i
    }

    pub fn iter(&self) -> impl Iterator<Item = (i8, &SectionToModify)> + '_ {
        let base = i32::from(self.base);
        self.slots
            .iter()
            .enumerate()
            .filter_map(move |(i, s)| Some(((base + i as i32) as i8, s.as_ref()?)))
    }

    pub fn keys(&self) -> impl Iterator<Item = i8> + '_ {
        self.iter().map(|(y, _)| y)
    }

    pub fn values(&self) -> impl Iterator<Item = &SectionToModify> + '_ {
        self.slots.iter().flatten()
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut SectionToModify> + '_ {
        self.slots.iter_mut().flatten()
    }

    pub fn into_entries(self) -> impl Iterator<Item = (i8, SectionToModify)> {
        let base = i32::from(self.base);
        self.slots
            .into_iter()
            .enumerate()
            .filter_map(move |(i, s)| Some(((base + i as i32) as i8, s?)))
    }

    pub fn len(&self) -> usize {
        self.values().count()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }
}

/// A chunk being modified (16x384x16 blocks, divided into sections)
#[derive(Default)]
pub(crate) struct ChunkToModify {
    pub sections: SectionMap,
    pub other: FnvHashMap<String, Value>,
}

impl ChunkToModify {
    #[inline]
    pub fn get_block(&self, x: u8, y: i32, z: u8) -> Option<Block> {
        // Above the ceiling nothing was ever stored, and the section index would truncate.
        if y > world_max_y() {
            return None;
        }
        let y = y.max(min_y());
        let section_idx: i8 = (y >> 4) as i8;
        let section = self.sections.get(section_idx)?;
        section.get_block(x, (y & 15) as u8, z)
    }

    #[inline]
    pub fn set_block(&mut self, x: u8, y: i32, z: u8, block: Block) {
        // Drop, never clamp: clamping would smear a slab across the ceiling under every
        // overflowing structure. Below the floor still clamps, as the bedrock plane does.
        if y > world_max_y() {
            return;
        }
        let y = y.max(min_y());
        let section_idx: i8 = (y >> 4) as i8;
        let section = self.sections.get_or_default(section_idx);
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
        if y > world_max_y() {
            return;
        }
        let y = y.max(min_y());
        let section_idx: i8 = (y >> 4) as i8;
        let section = self.sections.get_or_default(section_idx);
        section.set_block_with_properties(x, (y & 15) as u8, z, block_with_props);
    }

    pub fn sections(&self) -> impl Iterator<Item = Section> + '_ {
        self.sections.iter().map(|(y, s)| s.to_section(y))
    }
}

const REGION_CHUNKS: usize = 32 * 32;

/// A region's 32×32 chunk slots, indexed by local `(x, z)` instead of hashed.
/// Iterates in ascending `(x, z)`.
pub(crate) struct ChunkGrid {
    slots: Box<[Option<ChunkToModify>; REGION_CHUNKS]>,
}

impl Default for ChunkGrid {
    fn default() -> Self {
        let slots: Box<[Option<ChunkToModify>]> = std::iter::repeat_with(|| None)
            .take(REGION_CHUNKS)
            .collect();
        Self {
            slots: slots.try_into().unwrap_or_else(|_| unreachable!()),
        }
    }
}

impl ChunkGrid {
    #[inline(always)]
    fn slot(x: i32, z: i32) -> usize {
        debug_assert!((0..32).contains(&x) && (0..32).contains(&z));
        (((x & 31) << 5) | (z & 31)) as usize
    }

    #[inline(always)]
    fn key(slot: usize) -> (i32, i32) {
        ((slot >> 5) as i32, (slot & 31) as i32)
    }

    #[inline]
    pub fn get(&self, x: i32, z: i32) -> Option<&ChunkToModify> {
        self.slots[Self::slot(x, z)].as_ref()
    }

    #[inline]
    pub fn get_mut(&mut self, x: i32, z: i32) -> Option<&mut ChunkToModify> {
        self.slots[Self::slot(x, z)].as_mut()
    }

    #[inline]
    pub fn get_or_default(&mut self, x: i32, z: i32) -> &mut ChunkToModify {
        self.slots[Self::slot(x, z)].get_or_insert_with(ChunkToModify::default)
    }

    /// The raw slot, for callers that treat an absent chunk differently.
    #[inline]
    pub fn slot_mut(&mut self, x: i32, z: i32) -> &mut Option<ChunkToModify> {
        &mut self.slots[Self::slot(x, z)]
    }

    pub fn iter(&self) -> impl Iterator<Item = ((i32, i32), &ChunkToModify)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, c)| Some((Self::key(i), c.as_ref()?)))
    }

    pub fn values(&self) -> impl Iterator<Item = &ChunkToModify> + '_ {
        self.slots.iter().flatten()
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut ChunkToModify> + '_ {
        self.slots.iter_mut().flatten()
    }

    pub fn into_entries(self) -> impl Iterator<Item = ((i32, i32), ChunkToModify)> {
        (self.slots as Box<[Option<ChunkToModify>]>)
            .into_vec()
            .into_iter()
            .enumerate()
            .filter_map(|(i, c)| Some((Self::key(i), c?)))
    }

    pub fn len(&self) -> usize {
        self.values().count()
    }
}

/// A region being modified (32x32 chunks)
#[derive(Default)]
pub(crate) struct RegionToModify {
    pub chunks: ChunkGrid,
}

impl RegionToModify {
    #[inline]
    pub fn get_or_create_chunk(&mut self, x: i32, z: i32) -> &mut ChunkToModify {
        self.chunks.get_or_default(x, z)
    }

    #[inline]
    pub fn get_chunk(&self, x: i32, z: i32) -> Option<&ChunkToModify> {
        self.chunks.get(x, z)
    }

    /// See [`WorldToModify::compact_sections`].
    pub fn compact_sections(&mut self) {
        for chunk in self.chunks.values_mut() {
            for section in chunk.sections.values_mut() {
                if !matches!(&section.storage, BlockStorage::Uniform(_)) {
                    section.compact();
                }
            }
        }
    }
}

/// FxHash-style hasher for `(i32, i32)` coordinate keys: two multiplies where FNV takes
/// eight dependent byte rounds, and every block lookup starts by hashing its region key.
#[derive(Default, Clone, Copy)]
pub(crate) struct CoordHasher(u64);

impl CoordHasher {
    #[inline(always)]
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl std::hash::Hasher for CoordHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.add(u64::from(b));
        }
    }

    #[inline]
    fn write_i32(&mut self, i: i32) {
        self.add(u64::from(i as u32));
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub(crate) type CoordMap<V> =
    std::collections::HashMap<(i32, i32), V, std::hash::BuildHasherDefault<CoordHasher>>;

/// The entire world being modified.
#[derive(Default)]
pub(crate) struct WorldToModify {
    pub regions: CoordMap<RegionToModify>,
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
        // Both iterate in ascending key order, which the hash depends on.
        for (ck, chunk) in region.chunks.iter() {
            ck.hash(&mut h);
            for (sk, section) in chunk.sections.iter() {
                sk.hash(&mut h);
                // Hash logical block ids, not the raw storage, so a section
                // is hashed identically whether it ended up paletted or direct.
                let storage = &section.storage;
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

    /// Stored properties of the block at (x, y, z), if any.
    #[cfg(test)]
    pub fn get_properties(&self, x: i32, y: i32, z: i32) -> Option<Arc<Value>> {
        let chunk_x = x >> 4;
        let chunk_z = z >> 4;
        let region = self.get_region(chunk_x >> 5, chunk_z >> 5)?;
        let chunk = region.get_chunk(chunk_x & 31, chunk_z & 31)?;
        let section = chunk.sections.get((y >> 4) as i8)?;
        let index = SectionToModify::index((x & 15) as u8, (y & 15) as u8, (z & 15) as u8);
        section.properties.get(&index).cloned()
    }

    /// Finds the highest non-AIR block in one column and Y range.
    ///
    /// Column probes are used while placing tree canopies over buildings. The
    /// old caller performed one region/chunk/section HashMap lookup per Y level;
    /// walking the already-resolved chunk sections from the top avoids repeated
    /// hash probes for each canopy column.
    #[inline]
    pub fn highest_block_between(&self, x: i32, z: i32, min_y: i32, max_y: i32) -> Option<i32> {
        // Intersect with the world bounds. Clamping each end on its own would fold
        // a fully out-of-world range onto a boundary block and report a hit the
        // caller never asked for.
        let min_y = min_y.max(crate::world_editor::min_y());
        let max_y = max_y.min(world_max_y());
        if min_y > max_y {
            return None;
        }

        let chunk_x = x >> 4;
        let chunk_z = z >> 4;
        let region = self.get_region(chunk_x >> 5, chunk_z >> 5)?;
        let chunk = region.get_chunk(chunk_x & 31, chunk_z & 31)?;
        let local_x = (x & 15) as u8;
        let local_z = (z & 15) as u8;
        let min_section = (min_y >> 4) as i8;
        let max_section = (max_y >> 4) as i8;

        for section_y in (min_section..=max_section).rev() {
            let Some(section) = chunk.sections.get(section_y) else {
                continue;
            };
            let section_min_y = min_y.max(i32::from(section_y) << 4);
            let section_max_y = max_y.min((i32::from(section_y) << 4) + 15);
            for y in (section_min_y..=section_max_y).rev() {
                let index = SectionToModify::index(local_x, (y & 15) as u8, local_z);
                if section.storage.get(index) != AIR {
                    return Some(y);
                }
            }
        }
        None
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

    /// [`Self::set_block_with_properties`], except an occupied cell is only overwritten when
    /// `replace(existing)` agrees. Resolves the cell once instead of a read and then a write.
    #[inline]
    pub fn set_block_with_properties_if(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        block_with_props: BlockWithProperties,
        replace: impl FnOnce(Block) -> bool,
    ) {
        if y <= world_max_y() {
            let (chunk_x, chunk_z) = (x >> 4, z >> 4);
            let clamped_y = y.max(min_y());
            let section = self
                .regions
                .get_mut(&(chunk_x >> 5, chunk_z >> 5))
                .and_then(|r| r.chunks.get_mut(chunk_x & 31, chunk_z & 31))
                .and_then(|c| c.sections.get_mut((clamped_y >> 4) as i8));
            if let Some(section) = section {
                let (lx, ly, lz) = ((x & 15) as u8, (clamped_y & 15) as u8, (z & 15) as u8);
                let existing = section.storage.get(SectionToModify::index(lx, ly, lz));
                if existing == AIR || replace(existing) {
                    section.set_block_with_properties(lx, ly, lz, block_with_props);
                }
                return;
            }
        }
        // A cell with no section (or above the ceiling) reads as empty, which is always written.
        self.set_block_with_properties(x, y, z, block_with_props);
    }

    /// Set a block only if the cell is empty (AIR). Thin `#[inline]` wrapper over [`set_with_props_if_absent`].
    #[inline]
    pub fn set_block_if_absent(&mut self, x: i32, y: i32, z: i32, block: Block) -> bool {
        self.set_with_props_if_absent(
            x,
            y,
            z,
            BlockWithProperties {
                block,
                properties: None,
            },
        )
    }

    /// Set a block (+ optional NBT) only if the cell is empty (AIR), in one region/chunk/section descent.
    /// Returns whether it was set.
    #[inline]
    pub fn set_with_props_if_absent(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        block_with_props: BlockWithProperties,
    ) -> bool {
        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;

        let region = self.regions.entry((region_x, region_z)).or_default();
        let chunk = region.chunks.get_or_default(chunk_x & 31, chunk_z & 31);

        if y > world_max_y() {
            return false;
        }
        let y = y.max(min_y());
        let section_idx: i8 = (y >> 4) as i8;
        let section = chunk.sections.get_or_default(section_idx);

        let local_x = (x & 15) as u8;
        let local_y = (y & 15) as u8;
        let local_z = (z & 15) as u8;
        let idx = SectionToModify::index(local_x, local_y, local_z);

        if section.storage.get(idx) != AIR {
            return false;
        }
        section.storage.set(idx, block_with_props.block);
        if let Some(props) = block_with_props.properties {
            section.properties.insert(idx, props);
        } else {
            section.clear_properties_at(idx);
        }
        true
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
        let chunk = region.chunks.get_or_default(chunk_x & 31, chunk_z & 31);

        let local_x = (x & 15) as u8;
        let local_z = (z & 15) as u8;

        // A range entirely above the ceiling must vanish, not collapse into a slab on it.
        if y_min > world_max_y() {
            return;
        }
        let y_min = y_min.clamp(min_y(), world_max_y());
        let y_max = y_max.clamp(min_y(), world_max_y());

        for y in y_min..=y_max {
            let section_idx: i8 = (y >> 4) as i8;
            let section = chunk.sections.get_or_default(section_idx);
            let local_y = (y & 15) as u8;
            let idx = SectionToModify::index(local_x, local_y, local_z);

            if skip_existing {
                if section.storage.get(idx) == AIR {
                    section.storage.set(idx, block);
                    section.clear_properties_at(idx);
                }
            } else {
                section.storage.set(idx, block);
                section.clear_properties_at(idx);
            }
        }
    }

    /// Fills the air in `y_min..=y_max` of a column with `block_at(y)`, which is asked
    /// for every height in order, resolving the chunk once instead of per block run.
    pub fn fill_column_with(
        &mut self,
        x: i32,
        z: i32,
        y_min: i32,
        y_max: i32,
        mut block_at: impl FnMut(i32) -> Block,
    ) {
        if y_min > world_max_y() {
            return;
        }
        let (chunk_x, chunk_z) = (x >> 4, z >> 4);
        let chunk = self
            .regions
            .entry((chunk_x >> 5, chunk_z >> 5))
            .or_default()
            .chunks
            .get_or_default(chunk_x & 31, chunk_z & 31);
        let (local_x, local_z) = ((x & 15) as u8, (z & 15) as u8);
        let y_max = y_max.clamp(min_y(), world_max_y());
        let mut y = y_min.clamp(min_y(), world_max_y());
        while y <= y_max {
            let section = chunk.sections.get_or_default((y >> 4) as i8);
            let top = y_max.min(y | 15);
            for y in y..=top {
                let block = block_at(y);
                let idx = SectionToModify::index(local_x, (y & 15) as u8, local_z);
                if section.storage.get(idx) == AIR {
                    section.storage.set(idx, block);
                    section.clear_properties_at(idx);
                }
            }
            y = top + 1;
        }
    }

    /// Fill empty (Uniform(AIR)) sections of a chunk up to `section_y_max` with
    /// `Uniform(block)`. Returns true only if every section in the range was empty.
    pub fn bulk_fill_chunk_sections_below(
        &mut self,
        chunk_x: i32,
        chunk_z: i32,
        section_y_min: i8,
        section_y_max: i8,
        block: Block,
    ) -> bool {
        if section_y_max < section_y_min {
            return true;
        }
        let region_x = chunk_x >> 5;
        let region_z = chunk_z >> 5;
        let region = self.regions.entry((region_x, region_z)).or_default();
        let chunk = region.chunks.get_or_default(chunk_x & 31, chunk_z & 31);

        let mut all_clean = true;
        for section_y in section_y_min..=section_y_max {
            let section = chunk.sections.get_or_default(section_y);
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

    /// Dedup key, the one the save path uses: the cell plus the entity's UUID, so a tile
    /// halo's copy collapses while a car's parts or a signal's lamps sharing a cell do not.
    fn entity_coords(value: &Value) -> Option<(i32, i32, i32, super::java::EntityIdentity)> {
        let Value::Compound(map) = value else {
            return None;
        };
        super::java::get_entity_coords(map)
    }

    /// Per-chunk lists a tile carries into the merged world. Fluid ticks ride along so cave
    /// water and lava placed in a tile still flow once its chunk meets another tile's data.
    fn merges_as_list(key: &str) -> bool {
        matches!(key, "block_entities" | "entities" | "fluid_ticks")
    }

    /// Appends `other_list` into `self_list`, skipping entries already present at a coordinate.
    /// Tile halos process boundary features twice, so this drops the duplicate copies instead of
    /// retaining both (which also spared the save path from stripping them later).
    fn dedup_extend(self_list: &mut Vec<Value>, other_list: &[Value]) {
        let mut seen: FnvHashSet<_> = self_list.iter().filter_map(Self::entity_coords).collect();
        for entry in other_list {
            match Self::entity_coords(entry) {
                Some(coords) if !seen.insert(coords) => {}
                _ => self_list.push(entry.clone()),
            }
        }
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
                let self_region = self.regions.entry((region_x, region_z)).or_default();
                for ((chunk_lx, chunk_lz), other_chunk) in other_region.chunks.into_entries() {
                    Self::merge_chunk_auth(
                        self_region.chunks.slot_mut(chunk_lx, chunk_lz),
                        other_chunk,
                    );
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
            for ((chunk_lx, chunk_lz), other_chunk) in other_region.chunks.into_entries() {
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
                    Self::merge_chunk_auth(
                        self_region.chunks.slot_mut(chunk_lx, chunk_lz),
                        other_chunk,
                    );
                    continue;
                }

                let chunk_fully_outside = c_max_x < authoritative_min_x
                    || c_min_x > authoritative_max_x
                    || c_max_z < authoritative_min_z
                    || c_min_z > authoritative_max_z;

                let self_chunk = self_region.chunks.get_or_default(chunk_lx, chunk_lz);

                if chunk_fully_outside {
                    // Write-if-AIR for entire chunk, no coordinate math
                    for (section_y, other_section) in other_chunk.sections.into_entries() {
                        Self::merge_section_write_if_air(
                            self_chunk.sections.get_or_default(section_y),
                            other_section,
                        );
                    }
                } else {
                    // Per-block merge with coordinate checks (truly partial overlap)
                    for (section_y, other_section) in other_chunk.sections.into_entries() {
                        Self::merge_section_with_auth_check(
                            self_chunk.sections.get_or_default(section_y),
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

                Self::merge_chunk_lists(self_chunk, other_chunk.other);
            }
        }
    }

    /// Merge an auth-tile chunk. Moved in whole into an empty slot; otherwise reconciled
    /// per section so a prior tile's halo survives wherever this tile left AIR.
    fn merge_chunk_auth(slot: &mut Option<ChunkToModify>, other_chunk: ChunkToModify) {
        let Some(self_chunk) = slot else {
            *slot = Some(other_chunk);
            return;
        };
        for (section_y, other_section) in other_chunk.sections.into_entries() {
            Self::merge_section_auth_overwrite_nonair(
                self_chunk.sections.get_or_default(section_y),
                other_section,
            );
        }
        Self::merge_chunk_lists(self_chunk, other_chunk.other);
    }

    /// Append a tile chunk's entity lists (see `merges_as_list`); other keys are dropped.
    fn merge_chunk_lists(self_chunk: &mut ChunkToModify, other: FnvHashMap<String, Value>) {
        for (key, value) in other {
            if !Self::merges_as_list(&key) {
                continue;
            }
            match self_chunk.other.entry(key) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    if let Value::List(self_list) = entry.get_mut() {
                        if let Value::List(other_list) = &value {
                            Self::dedup_extend(self_list, other_list);
                        }
                    }
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(value);
                }
            }
        }
    }

    /// Merge an entire region using write-if-AIR semantics (halo zone).
    fn merge_region_write_if_air(self_region: &mut RegionToModify, other_region: RegionToModify) {
        for ((chunk_lx, chunk_lz), mut other_chunk) in other_region.chunks.into_entries() {
            let slot = self_region.chunks.slot_mut(chunk_lx, chunk_lz);
            let Some(self_chunk) = slot else {
                // Into an empty slot the chunk moves in whole, minus what the per-section
                // merge would not carry: properties on AIR cells and non-list chunk data.
                for section in other_chunk.sections.values_mut() {
                    section.retain_properties_on_blocks();
                }
                other_chunk.other.retain(|key, _| Self::merges_as_list(key));
                *slot = Some(other_chunk);
                continue;
            };
            for (section_y, other_section) in other_chunk.sections.into_entries() {
                Self::merge_section_write_if_air(
                    self_chunk.sections.get_or_default(section_y),
                    other_section,
                );
            }
            Self::merge_chunk_lists(self_chunk, other_chunk.other);
        }
    }

    /// Merge a single section using write-if-AIR (no coordinate checks).
    fn merge_section_write_if_air(
        self_section: &mut SectionToModify,
        other_section: SectionToModify,
    ) {
        let SectionToModify {
            storage: src,
            properties: src_props,
        } = other_section;
        if matches!(&src, BlockStorage::Uniform(b) if *b == AIR) {
            return;
        }
        match &self_section.storage {
            // No AIR cell left to fill.
            BlockStorage::Uniform(b) if *b != AIR => return,
            BlockStorage::Uniform(_) if self_section.properties.is_empty() => {
                *self_section = SectionToModify {
                    storage: src,
                    properties: src_props,
                };
                self_section.retain_properties_on_blocks();
                return;
            }
            _ => {}
        }
        // A written cell takes the source's properties, or loses its own.
        let dst = &self_section.storage;
        let written = |i: usize| i < SECTION_BLOCKS && dst.get(i) == AIR && src.get(i) != AIR;
        reconcile_properties(&mut self_section.properties, src_props, written);

        if let BlockStorage::Dense(dst) = &mut self_section.storage {
            match &src {
                BlockStorage::Dense(ids) => {
                    for (d, &s) in dst.iter_mut().zip(ids.iter()) {
                        *d = if *d == AIR_ID { s } else { *d };
                    }
                    return;
                }
                BlockStorage::Uniform(b) if b.id() < DENSE_ID_LIMIT => {
                    let s = b.id() as u8;
                    for d in dst.iter_mut() {
                        *d = if *d == AIR_ID { s } else { *d };
                    }
                    return;
                }
                _ => {}
            }
        }
        match &src {
            BlockStorage::Uniform(block) => {
                for idx in 0..SECTION_BLOCKS {
                    if self_section.storage.get(idx) == AIR {
                        self_section.storage.set(idx, *block);
                    }
                }
            }
            _ => {
                for (idx, block) in src.iter().enumerate() {
                    if block != AIR && self_section.storage.get(idx) == AIR {
                        self_section.storage.set(idx, block);
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
        other_section: SectionToModify,
    ) {
        let SectionToModify {
            storage: src,
            properties: src_props,
        } = other_section;
        match src {
            BlockStorage::Uniform(block) if block == AIR => return,
            BlockStorage::Uniform(block) => {
                // Every cell is overwritten, and with it every destination property.
                self_section.storage = BlockStorage::Uniform(block);
                self_section.properties = src_props;
                self_section.properties.retain(|&i, _| i < SECTION_BLOCKS);
                return;
            }
            _ => {}
        }
        let dest_air = matches!(&self_section.storage, BlockStorage::Uniform(b) if *b == AIR);
        if dest_air && self_section.properties.is_empty() {
            *self_section = SectionToModify {
                storage: src,
                properties: src_props,
            };
            self_section.retain_properties_on_blocks();
            return;
        }
        let written = |i: usize| i < SECTION_BLOCKS && src.get(i) != AIR;
        reconcile_properties(&mut self_section.properties, src_props, written);

        if let (BlockStorage::Dense(dst), BlockStorage::Dense(ids)) =
            (&mut self_section.storage, &src)
        {
            for (d, &s) in dst.iter_mut().zip(ids.iter()) {
                *d = if s != AIR_ID { s } else { *d };
            }
            return;
        }
        for (idx, block) in src.iter().enumerate() {
            // An AIR source cell keeps the halo data under it.
            if block != AIR {
                self_section.storage.set(idx, block);
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
                for idx in 0..SECTION_BLOCKS {
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
                        self_section.copy_properties_at(idx, &other_section.properties);
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
                        self_section.copy_properties_at(idx, &other_section.properties);
                    }
                }
            }
        }
    }

    /// Scan every section and collapse any that are entirely one block type
    /// back to `Uniform(Block)`, freeing the mixed-section allocation.
    pub fn compact_sections(&mut self) {
        use rayon::prelude::*;
        self.regions
            .par_iter_mut()
            .for_each(|(_, region)| region.compact_sections());
    }
}

#[cfg(test)]
mod merge_reference {
    //! Verbatim pre-fast-path mergers, so the optimized ones can be diffed against them.
    use super::*;

    pub fn write_if_air(self_section: &mut SectionToModify, other_section: &SectionToModify) {
        match &other_section.storage {
            BlockStorage::Uniform(block) if *block == AIR => {}
            BlockStorage::Uniform(block) => {
                let block = *block;
                for idx in 0..SECTION_BLOCKS {
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

    pub fn auth_overwrite_nonair(
        self_section: &mut SectionToModify,
        other_section: &SectionToModify,
    ) {
        match &other_section.storage {
            BlockStorage::Uniform(block) if *block == AIR => {}
            BlockStorage::Uniform(block) => {
                let block = *block;
                for idx in 0..SECTION_BLOCKS {
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
}

#[cfg(test)]
fn distinct_test_blocks(n: usize) -> Vec<Block> {
    let mut blocks = Vec::with_capacity(n);
    for id in 0..=u16::MAX {
        let block = Block::from_raw_id(id);
        if block.try_name().is_some() {
            blocks.push(block);
            if blocks.len() == n {
                return blocks;
            }
        }
    }
    panic!("needed {n} distinct named blocks, found {}", blocks.len());
}

#[cfg(test)]
fn distinct_dense_non_air_test_blocks(n: usize) -> Vec<Block> {
    let mut blocks = Vec::with_capacity(n);
    for id in 0..DENSE_ID_LIMIT {
        let block = Block::from_raw_id(id);
        if block != AIR && block.try_name().is_some() {
            blocks.push(block);
            if blocks.len() == n {
                return blocks;
            }
        }
    }
    panic!(
        "needed {n} dense distinct named blocks, found {}",
        blocks.len()
    );
}

#[cfg(test)]
mod to_section_tests {
    use super::*;

    type ReferencePalette = (Vec<(Block, Option<Arc<Value>>)>, Vec<usize>);

    /// The pre-optimization slow path, kept so the palette can be diffed against it.
    fn reference_palette(section: &SectionToModify) -> ReferencePalette {
        let mut unique_blocks: Vec<(Block, Option<Arc<Value>>)> = Vec::new();
        let mut palette_lookup: FnvHashMap<(Block, Option<String>), usize> = FnvHashMap::default();
        let mut indices = Vec::with_capacity(SECTION_BLOCKS);
        for (i, block) in section.storage.iter().enumerate() {
            let properties = section.properties.get(&i);
            let props_key = properties.map(|p| format!("{p:?}"));
            let palette_index = match palette_lookup.entry((block, props_key)) {
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
        (unique_blocks, indices)
    }

    /// Mixes shared Arcs, distinct-but-equal Arcs, and property-free cells.
    fn section_with_props(seed: u64) -> SectionToModify {
        let mut s = SectionToModify::default();
        let mut rng = seed;
        let mut next = || {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            (rng >> 33) as usize
        };
        for _ in 0..400 {
            s.storage.set(next() % SECTION_BLOCKS, STONE);
        }
        for _ in 0..60 {
            s.storage.set(next() % SECTION_BLOCKS, SMOOTH_STONE);
        }
        // One Arc reused across cells, plus separate Arcs with identical contents.
        let shared = Arc::new(Value::String("half=top".to_string()));
        for _ in 0..12 {
            s.properties
                .insert(next() % SECTION_BLOCKS, Arc::clone(&shared));
        }
        for _ in 0..12 {
            s.properties.insert(
                next() % SECTION_BLOCKS,
                Arc::new(Value::String("half=top".to_string())),
            );
        }
        for k in 0..8 {
            s.properties.insert(
                next() % SECTION_BLOCKS,
                Arc::new(Value::String(format!("facing={k}"))),
            );
        }
        s
    }

    #[test]
    fn no_props_path_keeps_first_seen_palette_order() {
        let mut s = SectionToModify::default();
        s.storage.set(5, STONE);
        s.storage.set(0, COBBLESTONE);
        s.storage.set(2, NETHERITE_BLOCK);

        let (want_blocks, want_indices) = reference_palette(&s);
        let got = s.to_section(0);

        assert_eq!(got.block_states.palette.len(), want_blocks.len());
        for (i, (block, stored)) in want_blocks.iter().enumerate() {
            let item = &got.block_states.palette[i];
            assert_eq!(item.name, format!("{}:{}", block.namespace(), block.name()));
            let want_props = stored
                .as_ref()
                .map(|p| (**p).clone())
                .or_else(|| block.properties());
            assert_eq!(item.properties, want_props);
        }

        let mut bits = 4;
        while (1 << bits) < want_blocks.len() {
            bits += 1;
        }
        let data = got.block_states.data.as_ref().expect("packed data");
        let longs: &[i64] = data;
        let per_long = 64 / bits;
        for (i, want) in want_indices.iter().enumerate() {
            let long = longs[i / per_long];
            let shift = (i % per_long) * bits;
            let got_idx = ((long >> shift) & ((1i64 << bits) - 1)) as usize;
            assert_eq!(got_idx, *want, "cell {i}");
        }
    }

    #[test]
    fn slow_path_palette_matches_the_pre_optimization_reference() {
        for seed in 0..40u64 {
            let s = section_with_props(seed);
            let (want_blocks, want_indices) = reference_palette(&s);
            let got = s.to_section(0);

            assert_eq!(
                got.block_states.palette.len(),
                want_blocks.len(),
                "palette length, seed {seed}"
            );
            for (i, (block, stored)) in want_blocks.iter().enumerate() {
                let item = &got.block_states.palette[i];
                assert_eq!(
                    item.name,
                    format!("{}:{}", block.namespace(), block.name()),
                    "palette[{i}] name, seed {seed}"
                );
                let want_props = stored
                    .as_ref()
                    .map(|p| (**p).clone())
                    .or_else(|| block.properties());
                assert_eq!(
                    item.properties, want_props,
                    "palette[{i}] props, seed {seed}"
                );
            }

            // Same logical index per cell, decoded from the packed long array.
            let mut bits = 4;
            while (1 << bits) < want_blocks.len() {
                bits += 1;
            }
            let data = got.block_states.data.as_ref().expect("packed data");
            let longs: &[i64] = data;
            let per_long = 64 / bits;
            for (i, want) in want_indices.iter().enumerate() {
                let long = longs[i / per_long];
                let shift = (i % per_long) * bits;
                let got_idx = ((long >> shift) & ((1i64 << bits) - 1)) as usize;
                assert_eq!(got_idx, *want, "cell {i}, seed {seed}");
            }
        }
    }
}

#[cfg(test)]
mod merge_fast_path_tests {
    use super::*;
    use once_cell::sync::Lazy;

    static DIRECT_SECTION_BLOCKS: Lazy<Vec<Block>> =
        Lazy::new(|| distinct_test_blocks(MAX_SECTION_PALETTE + 1));

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
            self.0 >> 33
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    const SHAPES: u64 = 8;

    /// Random section: varies storage variant, AIR density and property presence.
    fn section(rng: &mut Lcg, shape: u64, with_props: bool) -> SectionToModify {
        let mut s = SectionToModify::default();
        match shape {
            0 => {}
            1 => s.storage = BlockStorage::Uniform(STONE),
            4 => s.storage = BlockStorage::Uniform(NETHERITE_BLOCK),
            5 => {
                // Mostly solid, so write-if-air finds few holes.
                s.storage = BlockStorage::Uniform(COBBLESTONE);
                for _ in 0..(1 + rng.below(300)) {
                    let block = if rng.below(2) == 0 { AIR } else { STONE };
                    s.storage
                        .set(rng.below(SECTION_BLOCKS as u64) as usize, block);
                }
                assert!(matches!(&s.storage, BlockStorage::Dense(_)));
            }
            6 => {
                for _ in 0..(1 + rng.below(600)) {
                    let block = if rng.below(4) == 0 {
                        NETHERITE_BLOCK
                    } else {
                        STONE
                    };
                    s.storage
                        .set(rng.below(SECTION_BLOCKS as u64) as usize, block);
                }
            }
            2 => {
                for (i, &block) in DIRECT_SECTION_BLOCKS.iter().enumerate() {
                    s.storage.set(i, block);
                }
                for _ in 0..64 {
                    s.storage
                        .set(rng.below(SECTION_BLOCKS as u64) as usize, STONE);
                }
                assert!(matches!(&s.storage, BlockStorage::Direct(_)));
            }
            _ => {
                for _ in 0..(1 + rng.below(600)) {
                    s.storage
                        .set(rng.below(SECTION_BLOCKS as u64) as usize, STONE);
                }
            }
        }
        if with_props {
            for _ in 0..(1 + rng.below(6)) {
                let idx = rng.below(SECTION_BLOCKS as u64) as usize;
                s.properties
                    .insert(idx, Arc::new(Value::String(format!("p{}", idx % 3))));
            }
        }
        s
    }

    fn dup(s: &SectionToModify) -> SectionToModify {
        SectionToModify {
            storage: s.storage.clone(),
            properties: s.properties.clone(),
        }
    }

    fn same(a: &SectionToModify, b: &SectionToModify) -> bool {
        if (0..SECTION_BLOCKS).any(|i| a.storage.get(i) != b.storage.get(i)) {
            return false;
        }
        if a.properties.len() != b.properties.len() {
            return false;
        }
        a.properties
            .iter()
            .all(|(k, v)| b.properties.get(k).is_some_and(|w| **v == **w))
    }

    #[test]
    fn section_mergers_match_the_pre_fast_path_reference() {
        let mut rng = Lcg(0x5eed);
        for case in 0..8000u64 {
            let dst_props = case % 2 == 0;
            let src_props = case % 4 < 2;
            let (dst_shape, src_shape) = (rng.below(SHAPES), rng.below(SHAPES));
            let dst = section(&mut rng, dst_shape, dst_props);
            let src = section(&mut rng, src_shape, src_props);

            let (mut a, mut b) = (dup(&dst), dup(&dst));
            WorldToModify::merge_section_write_if_air(&mut a, dup(&src));
            merge_reference::write_if_air(&mut b, &src);
            assert!(same(&a, &b), "write_if_air diverged on case {case}");

            let (mut a, mut b) = (dup(&dst), dup(&dst));
            WorldToModify::merge_section_auth_overwrite_nonair(&mut a, dup(&src));
            merge_reference::auth_overwrite_nonair(&mut b, &src);
            assert!(
                same(&a, &b),
                "auth_overwrite_nonair diverged on case {case}"
            );
        }
    }

    #[test]
    fn a_halo_chunk_moved_into_an_empty_slot_matches_the_per_section_merge() {
        let mut rng = Lcg(0xc0ffee);
        for case in 0..300u64 {
            let mut chunk = ChunkToModify::default();
            for y in -4i8..6 {
                if rng.below(3) == 0 {
                    continue;
                }
                let shape = rng.below(SHAPES);
                *chunk.sections.get_or_default(y) = section(&mut rng, shape, case % 2 == 0);
            }
            chunk
                .other
                .insert("entities".to_string(), Value::List(Vec::new()));
            chunk
                .other
                .insert("Status".to_string(), Value::String("full".to_string()));

            let mut want = ChunkToModify::default();
            for (y, s) in chunk.sections.iter() {
                merge_reference::write_if_air(want.sections.get_or_default(y), s);
            }

            let mut tile = RegionToModify::default();
            *tile.chunks.slot_mut(3, 5) = Some(chunk);
            let mut dest = RegionToModify::default();
            WorldToModify::merge_region_write_if_air(&mut dest, tile);

            let got = dest.get_chunk(3, 5).expect("chunk merged");
            assert_eq!(got.sections.len(), want.sections.len(), "case {case}");
            for ((gy, g), (wy, w)) in got.sections.iter().zip(want.sections.iter()) {
                assert_eq!(gy, wy, "case {case}");
                assert!(same(g, w), "section {gy} diverged on case {case}");
            }
            assert!(got.other.contains_key("entities"));
            assert!(!got.other.contains_key("Status"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tile_merge_keeps_entities_sharing_a_cell_and_drops_halo_copies() {
        let entity = |uuid: i32, x: f64| {
            Value::Compound(std::collections::HashMap::from([
                (
                    "Pos".to_string(),
                    Value::List(vec![
                        Value::Double(x),
                        Value::Double(70.0),
                        Value::Double(5.5),
                    ]),
                ),
                (
                    "UUID".to_string(),
                    Value::IntArray(fastnbt::IntArray::new(vec![uuid, 0, 0, 1])),
                ),
            ]))
        };
        // A car's body and its marker, a few pixels apart in one cell.
        let mut merged = vec![entity(1, 3.4)];
        WorldToModify::dedup_extend(&mut merged, &[entity(1, 3.4), entity(2, 3.6)]);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn high_id_blocks_stay_paletted() {
        let mut s = BlockStorage::Uniform(AIR);
        s.set(0, STONE);
        assert!(matches!(s, BlockStorage::Dense(_)));
        s.set(1, NETHERITE_BLOCK);
        assert!(matches!(s, BlockStorage::Paletted(_)));
        assert_eq!(s.get(0), STONE);
        assert_eq!(s.get(1), NETHERITE_BLOCK);
        assert_eq!(s.iter().nth(1), Some(NETHERITE_BLOCK));

        let mut w = BlockStorage::Uniform(AIR);
        w.set(0, LEVER);
        assert!(matches!(w, BlockStorage::Paletted(_)));
        for i in 0..SECTION_BLOCKS {
            w.set(i, LEVER);
        }
        w.try_compact();
        assert!(matches!(w, BlockStorage::Uniform(_)));
        assert_eq!(w.get(7), LEVER);
    }

    #[test]
    fn palette_overflow_promotes_to_direct_and_compacts_back() {
        let blocks = distinct_test_blocks(MAX_SECTION_PALETTE + 1);
        let mut storage = BlockStorage::Uniform(AIR);
        for (i, &block) in blocks.iter().enumerate() {
            storage.set(i, block);
        }
        assert!(matches!(storage, BlockStorage::Direct(_)));

        for i in 0..SECTION_BLOCKS {
            storage.set(
                i,
                if i.is_multiple_of(2) {
                    STONE
                } else {
                    COBBLESTONE
                },
            );
        }
        storage.try_compact();
        assert!(matches!(storage, BlockStorage::Dense(_)));
        assert_eq!(storage.get(0), STONE);
        assert_eq!(storage.get(1), COBBLESTONE);
    }

    #[test]
    fn full_palette_can_swap_one_singleton_for_another() {
        let blocks = distinct_dense_non_air_test_blocks(MAX_SECTION_PALETTE - 2);
        let mut storage = BlockStorage::Uniform(AIR);
        for (i, &block) in blocks.iter().enumerate() {
            storage.set(i, block);
        }
        storage.set(blocks.len(), LEVER);
        assert!(matches!(storage, BlockStorage::Paletted(_)));

        storage.set(0, LADDER);
        assert!(matches!(storage, BlockStorage::Paletted(_)));
        assert_eq!(storage.get(0), LADDER);
    }

    #[test]
    fn low_id_blocks_stay_dense() {
        let mut storage = BlockStorage::Uniform(AIR);
        storage.set(0, STONE);
        assert!(matches!(storage, BlockStorage::Dense(_)));
        storage.set(1, COBBLESTONE);
        assert!(matches!(storage, BlockStorage::Dense(_)));
        assert_eq!(storage.get(0), STONE);
        assert_eq!(storage.get(1), COBBLESTONE);
    }

    #[test]
    fn bulk_fill_empty_chunk_all_clean() {
        let mut world = WorldToModify::default();
        let all_clean = world.bulk_fill_chunk_sections_below(0, 0, min_section_y(), -2, STONE);
        assert!(all_clean, "fresh chunk should report all sections clean");

        let region = world.get_region(0, 0).unwrap();
        let chunk = region.get_chunk(0, 0).unwrap();
        // Sections -4, -3, -2 must now exist as Uniform(STONE)
        for y in min_section_y()..=-2 {
            let section = chunk
                .sections
                .get(y)
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

        let all_clean = world.bulk_fill_chunk_sections_below(0, 0, min_section_y(), -2, STONE);
        assert!(
            !all_clean,
            "should return false because section -2 was occupied"
        );

        let region = world.get_region(0, 0).unwrap();
        let chunk = region.get_chunk(0, 0).unwrap();
        // Section -4 and -3 should be Uniform(STONE)
        for y in [-4i8, -3] {
            let section = chunk.sections.get(y).unwrap();
            assert!(
                matches!(&section.storage, BlockStorage::Uniform(b) if *b == STONE),
                "section {y} should be Uniform(STONE)"
            );
        }
        // Section -2 should be left alone as a mixed section with COBBLESTONE at y=-20.
        let section = chunk.sections.get(-2).unwrap();
        assert!(
            matches!(&section.storage, BlockStorage::Dense(_)),
            "section -2 should still be dense (had COBBLESTONE)"
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
        let all_clean =
            world.bulk_fill_chunk_sections_below(0, 0, min_section_y(), min_section_y() - 1, STONE);
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
        assert!(world.bulk_fill_chunk_sections_below(0, 0, min_section_y(), -2, STONE));
        let second = world.bulk_fill_chunk_sections_below(0, 0, min_section_y(), -2, STONE);
        assert!(!second, "second call sees Uniform(STONE) as occupied");
        let chunk = world.get_region(0, 0).unwrap().get_chunk(0, 0).unwrap();
        for y in min_section_y()..=-2 {
            let section = chunk.sections.get(y).unwrap();
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
    fn conditional_set_matches_a_read_then_a_write() {
        let _g = FLOOR_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let replace = |b: Block| b == STONE;
        let (mut got, mut want) = (WorldToModify::default(), WorldToModify::default());
        let mut rng = 7u64;
        for i in 0..40_000 {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let r = (rng >> 20) as i32;
            let (x, z) = (r % 80 - 40, (r >> 8) % 80 - 40);
            let y = (r >> 16) % 420 - 90;
            let write = BlockWithProperties {
                block: [STONE, COBBLESTONE, AIR][(i % 3) as usize],
                properties: (i % 7 == 0).then(|| Arc::new(Value::Int(i))),
            };
            got.set_block_with_properties_if(x, y, z, write.clone(), replace);
            if want.get_block(x, y, z).is_none_or(replace) {
                want.set_block_with_properties(x, y, z, write);
            }
        }

        let mut regions: Vec<_> = want.regions.keys().copied().collect();
        regions.sort_unstable();
        let mut got_regions: Vec<_> = got.regions.keys().copied().collect();
        got_regions.sort_unstable();
        assert_eq!(got_regions, regions);
        for key in regions {
            let (g, w) = (&got.regions[&key], &want.regions[&key]);
            let gc: Vec<_> = g.chunks.iter().map(|(k, _)| k).collect();
            let wc: Vec<_> = w.chunks.iter().map(|(k, _)| k).collect();
            assert_eq!(gc, wc);
            for ((_, gc), (_, wc)) in g.chunks.iter().zip(w.chunks.iter()) {
                assert_eq!(
                    gc.sections.keys().collect::<Vec<_>>(),
                    wc.sections.keys().collect::<Vec<_>>()
                );
                for ((_, gs), (_, ws)) in gc.sections.iter().zip(wc.sections.iter()) {
                    assert!((0..SECTION_BLOCKS).all(|i| gs.storage.get(i) == ws.storage.get(i)));
                    assert_eq!(gs.properties.len(), ws.properties.len());
                    assert!(gs
                        .properties
                        .iter()
                        .all(|(k, v)| ws.properties.get(k) == Some(v)));
                }
            }
        }
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
        let section = chunk.sections.get(section_idx).unwrap();
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

    #[test]
    fn highest_block_between_uses_section_order() {
        let mut world = WorldToModify::default();
        world.set_block_if_absent(3, 64, 5, STONE);
        world.set_block_if_absent(3, 80, 5, COBBLESTONE);

        assert_eq!(world.highest_block_between(3, 5, 60, 90), Some(80));
        assert_eq!(world.highest_block_between(3, 5, 65, 79), None);
        assert_eq!(world.highest_block_between(3, 5, 81, 90), None);
    }

    #[test]
    fn highest_block_between_rejects_ranges_outside_the_world() {
        // The intersection reads the world bounds, so hold them at the default.
        let _g = FLOOR_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut world = WorldToModify::default();
        world.set_block_if_absent(3, DEFAULT_MIN_Y, 5, STONE);
        world.set_block_if_absent(3, DEFAULT_MAX_Y, 5, COBBLESTONE);

        // Wholly outside the world: no Y in the requested range can answer.
        assert_eq!(
            world.highest_block_between(3, 5, DEFAULT_MAX_Y + 1, DEFAULT_MAX_Y + 50),
            None
        );
        assert_eq!(
            world.highest_block_between(3, 5, DEFAULT_MIN_Y - 50, DEFAULT_MIN_Y - 1),
            None
        );
        // Inverted ranges stay rejected.
        assert_eq!(world.highest_block_between(3, 5, 90, 60), None);
        // Partial overlap is intersected with the world, not rejected.
        assert_eq!(
            world.highest_block_between(3, 5, DEFAULT_MIN_Y - 50, DEFAULT_MIN_Y),
            Some(DEFAULT_MIN_Y)
        );
        assert_eq!(
            world.highest_block_between(3, 5, DEFAULT_MAX_Y, DEFAULT_MAX_Y + 50),
            Some(DEFAULT_MAX_Y)
        );
    }

    #[test]
    fn writes_above_the_ceiling_are_dropped_not_clamped_onto_it() {
        let _g = FLOOR_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());

        set_world_bounds(DEFAULT_MIN_Y, DEFAULT_MAX_Y);
        let mut vanilla = ChunkToModify::default();
        vanilla.set_block(0, DEFAULT_MAX_Y + 1, 0, STONE);
        vanilla.set_block(1, 2031, 1, COBBLESTONE);
        assert_eq!(vanilla.sections().count(), 0);

        set_world_bounds(-2032, 2031);
        let mut tall = ChunkToModify::default();
        tall.set_block(0, 2031, 0, STONE);
        tall.set_block(1, 2032, 1, COBBLESTONE);
        assert_eq!(tall.get_block(0, 2031, 0), Some(STONE));
        assert_eq!(tall.get_block(1, 2031, 1), None);
        assert_eq!(tall.sections().count(), 1);

        set_world_bounds(DEFAULT_MIN_Y, DEFAULT_MAX_Y);
    }

    #[test]
    fn writes_below_the_floor_still_clamp_onto_it() {
        let _g = FLOOR_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());

        for (floor, ceiling) in [(DEFAULT_MIN_Y, DEFAULT_MAX_Y), (-2032, 2031)] {
            set_world_bounds(floor, ceiling);
            let mut chunk = ChunkToModify::default();
            chunk.set_block(0, floor - 40, 0, STONE);
            assert_eq!(chunk.get_block(0, floor, 0), Some(STONE));

            let mut world = WorldToModify::default();
            world.set_block_if_absent(3, floor - 40, 5, STONE);
            assert_eq!(world.get_block(3, floor, 5), Some(STONE));
        }

        set_world_bounds(DEFAULT_MIN_Y, DEFAULT_MAX_Y);
    }

    #[test]
    fn reads_above_the_ceiling_report_nothing_instead_of_the_top_block() {
        let _g = FLOOR_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());

        set_world_bounds(-2032, 2031);
        let mut tall = ChunkToModify::default();
        tall.set_block(2, 2031, 2, STONE);
        assert_eq!(tall.get_block(2, 2031, 2), Some(STONE));
        assert_eq!(tall.get_block(2, 2032, 2), None);

        set_world_bounds(DEFAULT_MIN_Y, DEFAULT_MAX_Y);
        let mut vanilla = ChunkToModify::default();
        vanilla.set_block(2, DEFAULT_MAX_Y, 2, STONE);
        assert_eq!(vanilla.get_block(2, DEFAULT_MAX_Y, 2), Some(STONE));
        assert_eq!(vanilla.get_block(2, DEFAULT_MAX_Y + 1, 2), None);
    }

    #[test]
    fn the_bedrock_extended_ceiling_keeps_writes_above_the_vanilla_top() {
        let _g = FLOOR_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());

        // Bedrock's behavior pack declares -512..512, so the editor ceiling is the section
        // ending under it and the scaler puts terrain as high as Y 497.
        set_world_bounds(DEFAULT_MIN_Y, 511);
        let mut chunk = ChunkToModify::default();
        chunk.set_block(4, 400, 6, STONE);
        assert_eq!(chunk.get_block(4, 400, 6), Some(STONE));

        let mut world = WorldToModify::default();
        world.set_block_if_absent(4, 400, 6, STONE);
        world.fill_column(4, 6, 480, 497, STONE, false);
        assert_eq!(world.get_block(4, 400, 6), Some(STONE));
        assert_eq!(
            world.highest_block_between(4, 6, DEFAULT_MIN_Y, 511),
            Some(497)
        );

        set_world_bounds(DEFAULT_MIN_Y, DEFAULT_MAX_Y);
    }

    #[test]
    fn a_fill_reaching_past_the_ceiling_is_truncated_not_collapsed() {
        let _g = FLOOR_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());

        set_world_bounds(DEFAULT_MIN_Y, DEFAULT_MAX_Y);
        let mut world = WorldToModify::default();
        world.fill_column(3, 5, DEFAULT_MAX_Y + 10, DEFAULT_MAX_Y + 40, STONE, false);
        assert_eq!(
            world.highest_block_between(3, 5, DEFAULT_MIN_Y, DEFAULT_MAX_Y + 2000),
            None
        );
        world.fill_column(3, 5, DEFAULT_MAX_Y - 2, DEFAULT_MAX_Y + 40, STONE, false);
        assert_eq!(
            world.highest_block_between(3, 5, DEFAULT_MIN_Y, DEFAULT_MAX_Y),
            Some(DEFAULT_MAX_Y)
        );
        assert_eq!(world.get_block(3, DEFAULT_MAX_Y - 2, 5), Some(STONE));

        set_world_bounds(-2032, 2031);
        let mut tall = WorldToModify::default();
        tall.fill_column(7, 9, 2040, 2050, STONE, false);
        assert_eq!(tall.highest_block_between(7, 9, -2032, 2031), None);
        tall.fill_column(7, 9, 2028, 2050, STONE, false);
        assert_eq!(tall.highest_block_between(7, 9, -2032, 2031), Some(2031));
        assert_eq!(tall.get_block(7, 2028, 9), Some(STONE));

        set_world_bounds(DEFAULT_MIN_Y, DEFAULT_MAX_Y);
    }
}

#[cfg(test)]
mod terrain_floor_tests {
    use super::*;

    fn with_floor<T>(world_floor: i32, base: i32, f: impl FnOnce() -> T) -> T {
        // Mutates process-global state, so it runs under the shared floor lock and restores it.
        let _g = FLOOR_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // An extended floor always comes with the tall datapack's ceiling.
        let ceiling = if world_floor < DEFAULT_MIN_Y {
            2031
        } else {
            DEFAULT_MAX_Y
        };
        set_world_bounds(world_floor, ceiling);
        set_terrain_floor_y(base);
        let out = f();
        set_world_bounds(DEFAULT_MIN_Y, DEFAULT_MAX_Y);
        set_terrain_floor_y(DEFAULT_GROUND_LEVEL);
        out
    }

    #[test]
    fn vanilla_floor_is_unchanged() {
        // The vanilla clamp must always win, so bedrock stays exactly where it always was.
        with_floor(DEFAULT_MIN_Y, DEFAULT_GROUND_LEVEL, || {
            assert_eq!(terrain_floor_y(), -64);
        });
    }

    #[test]
    fn bedrock_is_flat_and_bounded_under_an_extended_floor() {
        // Switzerland at scale 0.1: floor -2032, base stays at -62 (the relief fits).
        // The bedrock plane must be a CONSTANT a bounded depth below the base -- not a
        // per-column shell tracking the terrain, and not 2000 blocks down at the world floor.
        with_floor(-2032, -62, || {
            let floor = terrain_floor_y();
            // At least TERRAIN_FLOOR_DEPTH under the base, snapped down to a section: -126
            // rounds to -128.
            assert_eq!(floor, -128);
            assert!(floor <= -62 - TERRAIN_FLOOR_DEPTH);
            // It is a constant: reading it never depends on any column's surface Y.
            assert_eq!(terrain_floor_y(), floor);
            assert!(
                floor > -2032,
                "must not sit at the world floor when the base is high"
            );
        });
    }

    #[test]
    fn terrain_floor_always_lands_on_a_section_boundary() {
        // The --fillground fast path bulk-fills whole sections starting at this floor and lets
        // bedrock overwrite the bottom layer. Off a boundary, the rest of that bottom section
        // stays stone *under* the bedrock plane. Vanilla's -64 was aligned by construction;
        // every extended-floor base has to be too.
        for (world_floor, base) in [
            (DEFAULT_MIN_Y, DEFAULT_GROUND_LEVEL),
            (DEFAULT_MIN_Y, 100),
            (-2032, -62),
            (-2032, -2030),
            (-2032, 0),
            (-2032, 317),
        ] {
            let floor = with_floor(world_floor, base, terrain_floor_y);
            assert_eq!(
                floor.rem_euclid(16),
                0,
                "floor {floor} for base {base} is not on a section boundary"
            );
            assert!(floor >= world_floor, "floor {floor} fell through the world");
        }
    }

    #[test]
    fn sunk_base_puts_bedrock_on_the_world_floor() {
        // Scale 1.0 with the datapack: the base sinks to -2030, so the floor clamps to -2032.
        with_floor(-2032, -2030, || {
            assert_eq!(terrain_floor_y(), -2032);
        });
    }
}
