//! Building voxy's LOD pyramid out of finished Java chunk sections.
//!
//! Voxy stores the world as 32x32x32 "world sections" at five detail levels.
//! Level `n` covers `32 << n` blocks per axis, so one 16-cube chunk section
//! contributes a 16-cube to level 0, an 8-cube to level 1, and so on down to a
//! single voxel at level 4. Each voxel is a `u64`:
//!
//! ```text
//! bits 56..63  light      (sky in the low nibble, block light in the high one)
//! bits 47..55  biome id
//! bits 27..46  block id   (0 = air, and then the whole word is just light)
//! bits  0..26  unused
//! ```
//!
//! Sections are written out one column at a time in Morton order, which is what
//! keeps memory flat: a level-`n` column is finished after every `4^n`th level-0
//! column, so only one column stack per level is ever resident (~7 MB) instead
//! of a whole region's worth (~250 MB).

use fastnbt::LongArray;
use fnv::{FnvHashMap, FnvHashSet};

use super::mapper::{is_air_name, state_key_into};
use super::VoxyWriter;
use crate::world_editor::common::Section;

/// 32x32x32 voxels per stored section.
pub(crate) const SECTION_VOLUME: usize = 32 * 32 * 32;
/// Levels 0..=4; level 4 is where one chunk section is a single voxel.
pub(crate) const MAX_LOD: usize = 4;

/// Offsets of each mip level inside the per-chunk-section scratch pyramid.
const MIP_OFFSETS: [usize; 5] = [0, 4096, 4096 + 512, 4096 + 512 + 64, 4096 + 512 + 64 + 8];
const PYRAMID_LEN: usize = 4096 + 512 + 64 + 8 + 1;

/// Voxy's `WorldEngine.getWorldSectionId`. The low four bits are spare.
pub(crate) fn section_key(lvl: i32, x: i32, y: i32, z: i32) -> i64 {
    (((lvl as i64) & 0xF) << 60)
        | (((y as i64) & 0xFF) << 52)
        | (((z as i64) & 0xFF_FFFF) << 28)
        | (((x as i64) & 0xFF_FFFF) << 4)
}

/// Voxy's `Mapper.composeMappingId`. Air carries light but no biome, matching
/// the mod: it would otherwise register a biome for every empty voxel.
#[inline]
fn compose(light: u8, block: u32, biome: u32) -> u64 {
    let light = (light as u64) << 56;
    if block == 0 {
        light
    } else {
        light | ((biome as u64) << 47) | ((block as u64) << 27)
    }
}

#[inline]
fn block_of(voxel: u64) -> u32 {
    ((voxel >> 27) & 0xF_FFFF) as u32
}

#[inline]
fn light_of(voxel: u64) -> u8 {
    (voxel >> 56) as u8
}

#[inline]
fn with_light(voxel: u64, light: u8) -> u64 {
    (voxel & !(0xFFu64 << 56)) | ((light as u64) << 56)
}

/// Voxy's `WorldSection.getChildIndex`: which bit of a parent's
/// `nonEmptyChildren` mask stands for the child at these section coordinates.
///
/// Careful - voxy numbers the eight children of a cell **two different ways**,
/// and they are not interchangeable:
///
/// - `WorldSection.getChildIndex`, here, packs `x | (y << 2) | (z << 1)`.
/// - [`mip`] ranks candidate voxels by `(x << 2) | (y << 1) | z`.
///
/// Both are pinned by tests against masks and voxels the mod wrote itself.
#[inline]
fn child_octant(x: i32, y: i32, z: i32) -> u32 {
    ((x & 1) | ((y & 1) << 2) | ((z & 1) << 1)) as u32
}

/// Voxy's `Mipper.mip`: the surviving voxel is the non-air child with the
/// highest `(opacity << 4) | corner`, where `corner` is `(x << 2) | (y << 1) | z`.
/// That is not the ordering [`child_octant`] uses; see its docs.
///
/// When all eight are air the light is averaged instead, so an empty cell still
/// lights whatever sits next to it.
fn mip(children: &[u64; 8], opacity: &[u8]) -> u64 {
    let mut best: i32 = -1;
    for (corner, &voxel) in children.iter().enumerate() {
        let block = block_of(voxel);
        if block == 0 {
            continue;
        }
        let dampening = opacity.get(block as usize).copied().unwrap_or(15) as i32;
        let rank = (dampening << 4) | corner as i32;
        if rank > best {
            best = rank;
        }
    }
    if best >= 0 {
        return children[(best & 0b111) as usize];
    }

    let mut block_light = 0u32;
    let mut sky_light = 0u32;
    for &voxel in children {
        let light = light_of(voxel) as u32;
        block_light += light & 0xF0;
        sky_light += light & 0x0F;
    }
    // Block light averages down; sky light rounds up, as voxy does.
    let block_light = (block_light / 8) & 0xF0;
    let sky_light = sky_light.div_ceil(8);
    with_light(children[7], (block_light | sky_light) as u8)
}

/// The largest a serialized section can get: every voxel distinct.
const MAX_SERIALIZED: usize = 16 + SECTION_VOLUME * 2 + SECTION_VOLUME * 8;

/// Reusable working set for section serialization. A region writes thousands of
/// sections, so the LUT map and the output buffer are kept rather than rebuilt.
#[derive(Default)]
struct SectionScratch {
    seen: FnvHashMap<u64, u16>,
    lut: Vec<u64>,
}

impl SectionScratch {
    /// Serializes one section into `out` the way `SaveLoadSystem3.serialize`
    /// does: the key, a metadata word, 32768 little-endian LUT indices, then
    /// the LUT itself.
    ///
    /// The index bytes and the LUT are built in a single pass; the metadata
    /// word is patched afterwards, once the LUT length is known.
    fn serialize(&mut self, key: i64, data: &[u64], non_empty_children: u8, out: &mut Vec<u8>) {
        debug_assert_eq!(data.len(), SECTION_VOLUME);
        self.seen.clear();
        self.lut.clear();
        out.clear();
        out.reserve(MAX_SERIALIZED);

        out.extend_from_slice(&key.to_le_bytes());
        out.extend_from_slice(&0i64.to_le_bytes()); // metadata, patched below

        // Runs of one value are the norm, so remember the last hit and skip the map.
        let mut prev = u64::MAX;
        let mut prev_index = 0u16;
        let mut first = true;
        for &voxel in data {
            let index = if !first && voxel == prev {
                prev_index
            } else {
                let next = self.lut.len() as u16;
                let lut = &mut self.lut;
                let index = *self.seen.entry(voxel).or_insert_with(|| {
                    lut.push(voxel);
                    next
                });
                prev = voxel;
                prev_index = index;
                first = false;
                index
            };
            out.extend_from_slice(&index.to_le_bytes());
        }

        for voxel in &self.lut {
            out.extend_from_slice(&voxel.to_le_bytes());
        }

        let metadata = (self.lut.len() as i64) | ((non_empty_children as i64) << 16);
        out[8..16].copy_from_slice(&metadata.to_le_bytes());
    }
}

/// Serializes one section into a fresh buffer. The region builder uses
/// [`SectionScratch`] directly; this is for callers that write a single section.
#[cfg(test)]
pub(crate) fn serialize_section(key: i64, data: &[u64], non_empty_children: u8) -> Vec<u8> {
    let mut out = Vec::new();
    SectionScratch::default().serialize(key, data, non_empty_children, &mut out);
    out
}

/// Per level, the coordinates of every section that some chunk can put a block
/// into. Built from the chunk section keys alone, before any voxel is touched.
pub(crate) type LiveSections = [FnvHashSet<(i32, i32, i32)>; MAX_LOD + 1];

/// Marks every LOD section that one populated chunk section feeds into.
pub(crate) fn mark_live(live: &mut LiveSections, chunk_x: i32, section_y: i32, chunk_z: i32) {
    for (lvl, level) in live.iter_mut().enumerate() {
        level.insert((
            chunk_x >> (lvl + 1),
            section_y >> (lvl + 1),
            chunk_z >> (lvl + 1),
        ));
    }
}

/// A section under construction. `x`/`y`/`z` are section coordinates at this level.
struct LodSection {
    x: i32,
    y: i32,
    z: i32,
    data: Vec<u64>,
    /// Level 0 counts real blocks; higher levels accumulate one bit per
    /// non-empty child octant.
    non_air: u32,
    children: u8,
}

/// Builds every LOD section covered by one region.
///
/// Regions are 512 blocks wide, which divides evenly by all five level sizes,
/// so no section ever straddles two regions and each region can be built
/// independently on its own thread.
pub(crate) struct RegionLod<'a> {
    writer: &'a VoxyWriter,
    /// One map per level, keyed by section Y within the level's current column.
    levels: Vec<FnvHashMap<i32, LodSection>>,
    /// Sections that can hold a block, per level. Everything else is air and
    /// would be dropped at flush, so it is never allocated in the first place.
    live: LiveSections,
    /// Recycled 32768-voxel buffers; allocating these fresh dominates otherwise.
    pool: Vec<Vec<u64>>,
    block_cache: FnvHashMap<String, u32>,
    biome_cache: FnvHashMap<&'static str, u32>,
    opacity: Vec<u8>,
    pyramid: Vec<u64>,
    palette_ids: Vec<u32>,
    biome_ids: [u32; 16],
    /// Reused so identifying a palette entry does not allocate per section.
    state_key: String,
    scratch: SectionScratch,
    serialized: Vec<u8>,
    /// One compression context for the whole region; building a fresh one per
    /// section allocates and initializes zstd's workspace thousands of times.
    compressor: zstd::bulk::Compressor<'static>,
    min_section_y: i32,
    max_section_y: i32,
    pub(crate) sections_written: u64,
}

impl<'a> RegionLod<'a> {
    /// `max_section_y` is the highest chunk section anywhere in the region that
    /// holds a block. Everything above it is air across the whole region, so
    /// every section up there would be dropped as empty anyway.
    pub(crate) fn new(
        writer: &'a VoxyWriter,
        min_section_y: i32,
        max_section_y: i32,
        live: LiveSections,
    ) -> Self {
        Self {
            writer,
            levels: (0..=MAX_LOD).map(|_| FnvHashMap::default()).collect(),
            live,
            pool: Vec::new(),
            block_cache: FnvHashMap::default(),
            biome_cache: FnvHashMap::default(),
            opacity: vec![0],
            pyramid: vec![0; PYRAMID_LEN],
            palette_ids: Vec::new(),
            biome_ids: [0; 16],
            state_key: String::new(),
            scratch: SectionScratch::default(),
            serialized: Vec::with_capacity(MAX_SERIALIZED),
            compressor: zstd::bulk::Compressor::new(super::ZSTD_LEVEL)
                .expect("zstd level 1 is always valid"),
            min_section_y,
            max_section_y,
            sections_written: 0,
        }
    }

    /// Feeds one finished chunk.
    ///
    /// `span` is the range of sections the chunk is written with and `lighting`
    /// is indexed from its start, exactly as
    /// [`crate::world_editor::java`] hands them to the NBT builder. Sections in
    /// the span that `sections` does not carry are air in the chunk file too,
    /// and are ingested as air: they still hold sky light, which is what lights
    /// whatever LOD geometry sits underneath them.
    pub(crate) fn ingest_chunk(
        &mut self,
        chunk_x: i32,
        chunk_z: i32,
        sections: &[Section],
        span: (i32, i32),
        lighting: Option<&[(Vec<i8>, Vec<i8>)]>,
        biome_names: &[&'static str; 16],
    ) {
        for (slot, name) in biome_names.iter().enumerate() {
            self.biome_ids[slot] = self.biome_for(name);
        }

        let (span_min, span_max) = span;
        let from = span_min.max(self.min_section_y);
        let to = span_max.min(self.max_section_y);

        for section_y in from..=to {
            let light = lighting.and_then(|l| {
                usize::try_from(section_y - span_min)
                    .ok()
                    .and_then(|i| l.get(i))
            });
            // Nothing this chunk section touches can end up on disk: skip it
            // before decoding a palette or composing a single voxel. Above the
            // roofline this is almost every section in the chunk.
            if !(0..=MAX_LOD)
                .rev()
                .any(|lvl| self.is_live(lvl, chunk_x, section_y, chunk_z))
            {
                continue;
            }

            // A chunk holds at most a couple of dozen sections, so a scan beats
            // building a map per chunk.
            let section = sections.iter().find(|s| s.y as i32 == section_y);
            self.ingest_section(chunk_x, section_y, chunk_z, section, light);
        }
    }

    /// Called after the four chunks of Morton column `column` have been fed.
    /// Level `n` is complete every `4^n` columns, so this is where sections get
    /// serialized and their buffers recycled.
    pub(crate) fn end_column(&mut self, column: usize) {
        self.flush_level(0);
        let mut span = 4usize;
        for lvl in 1..=MAX_LOD {
            if (column + 1).is_multiple_of(span) {
                self.flush_level(lvl);
            }
            span *= 4;
        }
    }

    /// Flushes whatever is left, in case a caller stops short of a full region.
    pub(crate) fn finish(&mut self) {
        for lvl in 0..=MAX_LOD {
            self.flush_level(lvl);
        }
    }

    fn biome_for(&mut self, name: &&'static str) -> u32 {
        if let Some(&id) = self.biome_cache.get(*name) {
            return id;
        }
        let id = self.writer.intern_biome(name);
        self.biome_cache.insert(name, id);
        id
    }

    fn ingest_section(
        &mut self,
        chunk_x: i32,
        section_y: i32,
        chunk_z: i32,
        section: Option<&Section>,
        light: Option<&(Vec<i8>, Vec<i8>)>,
    ) {
        self.palette_ids.clear();
        let mut all_air = true;
        match section {
            None => self.palette_ids.push(0),
            Some(section) => {
                let palette = &section.block_states.palette;
                if palette.is_empty() {
                    return;
                }
                for item in palette {
                    let id = if is_air_name(&item.name) {
                        0
                    } else {
                        state_key_into(item, &mut self.state_key);
                        match self.block_cache.get(self.state_key.as_str()) {
                            Some(&id) => id,
                            None => {
                                let (id, opacity) = self.writer.intern_block(&self.state_key, item);
                                if self.opacity.len() <= id as usize {
                                    self.opacity.resize(id as usize + 1, 15);
                                }
                                self.opacity[id as usize] = opacity;
                                self.block_cache.insert(self.state_key.clone(), id);
                                id
                            }
                        }
                    };
                    all_air &= id == 0;
                    self.palette_ids.push(id);
                }
            }
        }

        // Uniform light is the common case by far: air above the terrain is
        // fully sky-lit, everything enclosed is dark. Detecting it once here
        // replaces 4096 nibble reads plus the whole mip pass below.
        let uniform_light = match light {
            None => Some(0u8),
            Some((sky, block)) => {
                let flat = |a: &[i8]| {
                    let first = a.first().copied().unwrap_or(0) as u8;
                    (first & 0x0F == first >> 4 && a.iter().all(|&n| n as u8 == first))
                        .then_some(first & 0x0F)
                };
                match (flat(sky), flat(block)) {
                    (Some(s), Some(b)) => Some(s | (b << 4)),
                    _ => None,
                }
            }
        };

        // Unlit air contributes nothing: every voxel would be zero, which is
        // what the destination buffers already hold. Skipping is what keeps
        // hollow interiors and the void below the terrain nearly free.
        if all_air && uniform_light == Some(0) {
            return;
        }

        let data = section.and_then(|s| s.block_states.data.as_ref());

        // A single-valued palette container plus uniform light means every
        // voxel in the cube is the same word, and so is every mip of it: the
        // mip of eight identical blocks is that block, and of eight identical
        // air voxels is air with the same light. Fill the destinations
        // directly and skip building the pyramid at all.
        if let (None, Some(light)) = (data, uniform_light) {
            let block = self.palette_ids[0];
            let uniform_biome = self.biome_ids.iter().all(|&b| b == self.biome_ids[0]);
            if block == 0 || uniform_biome {
                let voxel = compose(light, block, self.biome_ids[0]);
                for lvl in 0..=MAX_LOD {
                    self.insert_uniform(lvl, chunk_x, section_y, chunk_z, voxel);
                }
                return;
            }
        }

        let sky = light.map(|(s, _)| s.as_slice());
        let block_light = light.map(|(_, b)| b.as_slice());

        self.fill_level0(data, sky, block_light);
        self.build_mips();

        for lvl in 0..=MAX_LOD {
            self.insert_level(lvl, chunk_x, section_y, chunk_z);
        }
    }

    /// Sub-cube bounds of one chunk section inside its level-`lvl` world section.
    fn placement(lvl: usize, chunk_x: i32, section_y: i32, chunk_z: i32) -> (usize, usize, usize) {
        let side_bits = 4 - lvl;
        let side = 1usize << side_bits;
        let coord_mask = (1i32 << (lvl + 1)) - 1;
        (
            ((chunk_x & coord_mask) as usize) * side,
            ((section_y & coord_mask) as usize) * side,
            ((chunk_z & coord_mask) as usize) * side,
        )
    }

    /// Writes a constant voxel over one chunk section's sub-cube. Rows are
    /// contiguous in x, so this runs at memset speed.
    fn insert_uniform(
        &mut self,
        lvl: usize,
        chunk_x: i32,
        section_y: i32,
        chunk_z: i32,
        voxel: u64,
    ) {
        let side = 1usize << (4 - lvl);
        let (base_x, base_y, base_z) = Self::placement(lvl, chunk_x, section_y, chunk_z);
        let Some(section) = Self::section_mut(
            &mut self.levels[lvl],
            &mut self.pool,
            &self.live[lvl],
            chunk_x >> (lvl + 1),
            section_y >> (lvl + 1),
            chunk_z >> (lvl + 1),
        ) else {
            return;
        };
        for y in 0..side {
            for z in 0..side {
                let start = ((base_y + y) << 10) | ((base_z + z) << 5) | base_x;
                section.data[start..start + side].fill(voxel);
            }
        }
        if lvl == 0 && block_of(voxel) != 0 {
            section.non_air += (side * side * side) as u32;
        }
    }

    /// Decodes the section's palette container into the base of the pyramid.
    fn fill_level0(
        &mut self,
        data: Option<&LongArray>,
        sky: Option<&[i8]>,
        block_light: Option<&[i8]>,
    ) {
        let palette_len = self.palette_ids.len();

        let mut bits = 4usize;
        while (1usize << bits) < palette_len {
            bits += 1;
        }
        let per_long = 64 / bits;
        let mask = (1u64 << bits) - 1;

        // Fixed 2048-byte nibble planes, so the inner loop indexes without an
        // Option check or a bounds check per voxel.
        const NIBBLES: usize = 2048;
        const DARK: [i8; NIBBLES] = [0; NIBBLES];
        let plane = |arr: Option<&'_ [i8]>| -> [i8; NIBBLES] {
            match arr {
                Some(a) if a.len() >= NIBBLES => a[..NIBBLES].try_into().unwrap(),
                _ => DARK,
            }
        };
        let sky = plane(sky);
        let block_light = plane(block_light);

        let biomes = self.biome_ids;
        let pyramid = &mut self.pyramid[..4096];
        let palette = &self.palette_ids;
        let uniform_block = palette[0];

        // Walk the packed container with a running slot cursor: the divisions a
        // per-index decode would need are the most expensive part of this loop.
        let mut long_index = 0usize;
        let mut slot = 0usize;
        let mut i = 0usize;
        for _y in 0..16usize {
            for z in 0..16usize {
                let biome_row = &biomes[(z >> 2) * 4..(z >> 2) * 4 + 4];
                // Two voxels share one light byte, so read it once per pair.
                for x2 in 0..8usize {
                    let s = sky[i >> 1] as u8;
                    let b = block_light[i >> 1] as u8;
                    let lights = [(s & 0x0F) | ((b & 0x0F) << 4), (s >> 4) | (b & 0xF0)];

                    for (half, &light) in lights.iter().enumerate() {
                        let block = match data {
                            None => uniform_block,
                            Some(longs) => {
                                let id = match longs.get(long_index) {
                                    Some(&word) => {
                                        let entry =
                                            ((word as u64 >> (slot * bits)) & mask) as usize;
                                        palette.get(entry).copied().unwrap_or(0)
                                    }
                                    None => 0,
                                };
                                slot += 1;
                                if slot == per_long {
                                    slot = 0;
                                    long_index += 1;
                                }
                                id
                            }
                        };
                        let biome = biome_row[(x2 * 2 + half) >> 2];
                        pyramid[i + half] = compose(light, block, biome);
                    }
                    i += 2;
                }
            }
        }
    }

    /// Mips the 16-cube down through 8, 4, 2 to a single voxel.
    fn build_mips(&mut self) {
        for lvl in 1..=MAX_LOD {
            let src_base = MIP_OFFSETS[lvl - 1];
            let dst_base = MIP_OFFSETS[lvl];
            let dst_side = 16usize >> lvl;
            let src_shift = (dst_side * 2).trailing_zeros() as usize;
            let dst_shift = dst_side.trailing_zeros() as usize;

            // Neighbours are one step apart on each axis, so the eight children
            // are fixed offsets from the corner rather than eight index builds.
            let y_stride = 1usize << (2 * src_shift);
            let z_stride = 1usize << src_shift;

            for y in 0..dst_side {
                for z in 0..dst_side {
                    for x in 0..dst_side {
                        // Ordered by voxy's corner code, (x << 2) | (y << 1) | z.
                        let c = src_base
                            + ((y * 2) << (2 * src_shift))
                            + ((z * 2) << src_shift)
                            + (x * 2);
                        let p = &self.pyramid;
                        let children = [
                            p[c],
                            p[c + z_stride],
                            p[c + y_stride],
                            p[c + y_stride + z_stride],
                            p[c + 1],
                            p[c + 1 + z_stride],
                            p[c + 1 + y_stride],
                            p[c + 1 + y_stride + z_stride],
                        ];
                        let index = (y << (2 * dst_shift)) | (z << dst_shift) | x;
                        self.pyramid[dst_base + index] = mip(&children, &self.opacity);
                    }
                }
            }
        }
    }

    /// Copies one mip level of a chunk section into its enclosing world section.
    fn insert_level(&mut self, lvl: usize, chunk_x: i32, section_y: i32, chunk_z: i32) {
        let side_bits = 4 - lvl;
        let side = 1usize << side_bits;
        let (base_x, base_y, base_z) = Self::placement(lvl, chunk_x, section_y, chunk_z);

        let Some(section) = Self::section_mut(
            &mut self.levels[lvl],
            &mut self.pool,
            &self.live[lvl],
            chunk_x >> (lvl + 1),
            section_y >> (lvl + 1),
            chunk_z >> (lvl + 1),
        ) else {
            return;
        };

        // Each chunk section owns a disjoint sub-cube of its world section, so
        // every destination voxel is written exactly once and the running count
        // never needs to undo an earlier value.
        let src_base = MIP_OFFSETS[lvl];
        let mut non_air = 0u32;
        for y in 0..side {
            for z in 0..side {
                for x in 0..side {
                    let voxel =
                        self.pyramid[src_base + ((y << (2 * side_bits)) | (z << side_bits) | x)];
                    let dst = ((base_y + y) << 10) | ((base_z + z) << 5) | (base_x + x);
                    // Level 0 tracks emptiness by block count, like voxy's
                    // `nonEmptyBlockCount`; higher levels take it from their
                    // children instead.
                    if lvl == 0 && block_of(voxel) != 0 {
                        non_air += 1;
                    }
                    section.data[dst] = voxel;
                }
            }
        }
        section.non_air += non_air;
    }

    /// The section at these coordinates, created on first write. Returns `None`
    /// for a section no chunk can put a block in: allocating and zeroing a
    /// quarter-megabyte buffer only to drop it at flush is the single most
    /// expensive thing this builder could do.
    fn section_mut<'s>(
        level: &'s mut FnvHashMap<i32, LodSection>,
        pool: &mut Vec<Vec<u64>>,
        live: &FnvHashSet<(i32, i32, i32)>,
        x: i32,
        y: i32,
        z: i32,
    ) -> Option<&'s mut LodSection> {
        use std::collections::hash_map::Entry;
        match level.entry(y) {
            Entry::Occupied(slot) => Some(slot.into_mut()),
            Entry::Vacant(slot) => {
                if !live.contains(&(x, y, z)) {
                    return None;
                }
                let data = match pool.pop() {
                    Some(mut buf) => {
                        buf.fill(0);
                        buf
                    }
                    None => vec![0u64; SECTION_VOLUME],
                };
                Some(slot.insert(LodSection {
                    x,
                    y,
                    z,
                    data,
                    non_air: 0,
                    children: 0,
                }))
            }
        }
    }

    /// Whether any level-`lvl` section covering this chunk section can hold a block.
    fn is_live(&self, lvl: usize, chunk_x: i32, section_y: i32, chunk_z: i32) -> bool {
        self.live[lvl].contains(&(
            chunk_x >> (lvl + 1),
            section_y >> (lvl + 1),
            chunk_z >> (lvl + 1),
        ))
    }

    /// Serializes and emits every section of one finished column, marking each
    /// non-empty one in its parent's child mask on the way up.
    fn flush_level(&mut self, lvl: usize) {
        let sections = std::mem::take(&mut self.levels[lvl]);
        for (_, section) in sections {
            let children = if lvl == 0 {
                if section.non_air > 0 {
                    0xFF
                } else {
                    0
                }
            } else {
                section.children
            };

            if children != 0 {
                if lvl < MAX_LOD {
                    if let Some(parent) = self.levels[lvl + 1].get_mut(&(section.y >> 1)) {
                        parent.children |= 1 << child_octant(section.x, section.y, section.z);
                    }
                }
                let key = section_key(lvl as i32, section.x, section.y, section.z);
                self.scratch
                    .serialize(key, &section.data, children, &mut self.serialized);
                match self.compressor.compress(&self.serialized) {
                    Ok(blob) => {
                        self.writer.put_section(key, &blob);
                        self.sections_written += 1;
                    }
                    Err(e) => self.writer.record_error(e),
                }
            }

            self.pool.push(section.data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decoded straight from a real voxy database written by the mod, so the
    /// packing has to agree bit for bit.
    #[test]
    fn section_keys_match_voxy() {
        // level 0 @ (9, -2, 14): the section holding blocks x 288.., y -64.., z 448..
        let key = section_key(0, 9, -2, 14);
        assert_eq!(key >> 60 & 0xF, 0);
        assert_eq!(((key << 4) >> 56) as i32, -2, "y is a signed byte");
        assert_eq!(((key << 12) >> 40) as i32, 14, "z is 24 signed bits");
        assert_eq!(((key << 36) >> 40) as i32, 9, "x is 24 signed bits");
        assert_eq!(section_key(4, 0, -1, 1), 0x4ff0000010000000u64 as i64);
    }

    #[test]
    fn voxels_pack_like_the_mod() {
        // block 1 (bedrock), biome 0, unlit -> 0x8000000 in the reference world.
        assert_eq!(compose(0, 1, 0), 0x0800_0000);
        assert_eq!(compose(0, 1, 1), 0x0000_8000_0800_0000);
        // Air keeps its light but drops the biome.
        assert_eq!(compose(0x0F, 0, 3), 0x0F00_0000_0000_0000);
        assert_eq!(block_of(compose(15, 1234, 7)), 1234);
        assert_eq!(light_of(compose(0xF3, 1, 0)), 0xF3);
    }

    /// Straight from voxy's `WorldSection.getChildIndex`. The mip ranking below
    /// uses a different permutation, which is exactly why this is pinned.
    #[test]
    fn child_octant_matches_voxy() {
        for (x, y, z, expected) in [
            (0, 0, 0, 0),
            (1, 0, 0, 1),
            (0, 0, 1, 2),
            (1, 0, 1, 3),
            (0, 1, 0, 4),
            (1, 1, 0, 5),
            (0, 1, 1, 6),
            (1, 1, 1, 7),
        ] {
            assert_eq!(child_octant(x, y, z), expected, "child ({x},{y},{z})");
        }
        // Section coordinates are signed, and only the low bit selects the octant.
        assert_eq!(child_octant(-1, -2, -3), child_octant(1, 0, 1));
        assert_eq!(child_octant(-4, -4, -4), child_octant(0, 0, 0));
    }

    /// The mip corner code is the other permutation: `(x << 2) | (y << 1) | z`.
    #[test]
    fn mip_corner_order_is_not_the_child_octant_order() {
        let opacity = vec![0, 15];
        let air = compose(0, 0, 0);
        let stone = compose(0, 1, 0);

        // Corner 4 is (x=1, y=0, z=0) for the mip, but octant 4 is (0, 1, 0).
        let mut children = [air; 8];
        children[4] = stone;
        assert_eq!(mip(&children, &opacity), stone);
        assert_eq!(child_octant(1, 0, 0), 1, "the two orderings really differ");
    }

    #[test]
    fn mip_prefers_the_most_opaque_child() {
        let opacity = vec![0, 15, 0]; // air, stone, glass
        let stone = compose(0, 1, 0);
        let glass = compose(0, 2, 0);
        let air = compose(0, 0, 0);

        let mut children = [air; 8];
        children[0] = glass;
        children[3] = stone;
        assert_eq!(mip(&children, &opacity), stone);

        // Equal opacity falls back to the highest corner, (x<<2)|(y<<1)|z.
        let mut children = [air; 8];
        children[1] = stone;
        children[6] = stone;
        assert_eq!(mip(&children, &opacity), children[6]);
    }

    #[test]
    fn mip_of_pure_air_averages_light() {
        let opacity = vec![0];
        // Four cells at sky 15, four dark: block light floors, sky light ceils.
        let mut children = [compose(0, 0, 0); 8];
        for slot in children.iter_mut().take(4) {
            *slot = compose(0x0F, 0, 0);
        }
        assert_eq!(light_of(mip(&children, &opacity)), 8); // ceil(60/8)

        let children = [compose(0xF0, 0, 0); 8];
        assert_eq!(light_of(mip(&children, &opacity)), 0xF0);
        assert_eq!(block_of(mip(&children, &opacity)), 0);
    }

    /// The serialized layout is what `SaveLoadSystem3.deserialize` walks.
    #[test]
    fn serialized_sections_match_the_save_format() {
        let mut data = vec![0u64; SECTION_VOLUME];
        data[0] = 0xAA;
        data[SECTION_VOLUME - 1] = 0xBB;
        let key = section_key(1, -3, 2, 7);
        let out = serialize_section(key, &data, 0xFF);

        assert_eq!(out.len(), 16 + SECTION_VOLUME * 2 + 3 * 8);
        assert_eq!(i64::from_le_bytes(out[0..8].try_into().unwrap()), key);
        let metadata = i64::from_le_bytes(out[8..16].try_into().unwrap());
        assert_eq!(metadata & 0xFFFF, 3, "three distinct voxels");
        assert_eq!((metadata >> 16) & 0xFF, 0xFF, "child mask survives");

        let index_at =
            |i: usize| u16::from_le_bytes(out[16 + i * 2..18 + i * 2].try_into().unwrap()) as usize;
        let lut_base = 16 + SECTION_VOLUME * 2;
        let lut_at = |i: usize| {
            u64::from_le_bytes(
                out[lut_base + i * 8..lut_base + i * 8 + 8]
                    .try_into()
                    .unwrap(),
            )
        };
        assert_eq!(lut_at(index_at(0)), 0xAA);
        assert_eq!(lut_at(index_at(1)), 0);
        assert_eq!(lut_at(index_at(SECTION_VOLUME - 1)), 0xBB);
    }

    /// Every voxel must round-trip through the LUT, however varied the section.
    #[test]
    fn serialized_sections_round_trip() {
        let mut data = vec![0u64; SECTION_VOLUME];
        let mut state = 0x1234_5678_9abc_def0u64;
        for (i, slot) in data.iter_mut().enumerate() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *slot = if i % 3 == 0 { 0 } else { state & 0xFFFF };
        }
        let out = serialize_section(section_key(0, 0, 0, 0), &data, 1);
        let lut_len = (i64::from_le_bytes(out[8..16].try_into().unwrap()) & 0xFFFF) as usize;
        let lut_base = 16 + SECTION_VOLUME * 2;
        for (i, &expected) in data.iter().enumerate() {
            let index =
                u16::from_le_bytes(out[16 + i * 2..18 + i * 2].try_into().unwrap()) as usize;
            assert!(index < lut_len);
            let got = u64::from_le_bytes(
                out[lut_base + index * 8..lut_base + index * 8 + 8]
                    .try_into()
                    .unwrap(),
            );
            assert_eq!(got, expected, "voxel {i}");
        }
    }
}
