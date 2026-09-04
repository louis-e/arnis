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
use fnv::FnvHashMap;

use super::mapper::{is_air_name, state_key};
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

/// Voxy's `Mipper.mip`: the surviving voxel is the non-air child with the
/// highest `(opacity << 4) | corner`, where `corner` is `(x << 2) | (y << 1) | z`.
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

/// Serializes one section the way `SaveLoadSystem3.serialize` does: the key, a
/// metadata word, 32768 little-endian LUT indices, then the LUT itself.
pub(crate) fn serialize_section(key: i64, data: &[u64], non_empty_children: u8) -> Vec<u8> {
    debug_assert_eq!(data.len(), SECTION_VOLUME);

    let mut lut: Vec<u64> = Vec::with_capacity(64);
    let mut seen: FnvHashMap<u64, u16> = FnvHashMap::default();
    let mut indices: Vec<u16> = Vec::with_capacity(SECTION_VOLUME);

    // Runs of one value are the norm, so remember the last hit and skip the map.
    let mut prev = u64::MAX;
    let mut prev_index = 0u16;
    let mut first = true;
    for &voxel in data {
        if !first && voxel == prev {
            indices.push(prev_index);
            continue;
        }
        let next = lut.len() as u16;
        let index = *seen.entry(voxel).or_insert_with(|| {
            lut.push(voxel);
            next
        });
        indices.push(index);
        prev = voxel;
        prev_index = index;
        first = false;
    }

    let metadata = (lut.len() as i64) | ((non_empty_children as i64) << 16);
    let mut out = Vec::with_capacity(16 + SECTION_VOLUME * 2 + lut.len() * 8);
    out.extend_from_slice(&key.to_le_bytes());
    out.extend_from_slice(&metadata.to_le_bytes());
    for index in &indices {
        out.extend_from_slice(&index.to_le_bytes());
    }
    for voxel in &lut {
        out.extend_from_slice(&voxel.to_le_bytes());
    }
    out
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
    /// Recycled 32768-voxel buffers; allocating these fresh dominates otherwise.
    pool: Vec<Vec<u64>>,
    block_cache: FnvHashMap<String, u32>,
    biome_cache: FnvHashMap<&'static str, u32>,
    opacity: Vec<u8>,
    pyramid: Vec<u64>,
    palette_ids: Vec<u32>,
    biome_ids: [u32; 16],
    min_section_y: i32,
    max_section_y: i32,
    pub(crate) sections_written: u64,
}

impl<'a> RegionLod<'a> {
    /// `max_section_y` is the highest chunk section anywhere in the region that
    /// holds a block. Everything above it is air across the whole region, so
    /// every section up there would be dropped as empty anyway.
    pub(crate) fn new(writer: &'a VoxyWriter, min_section_y: i32, max_section_y: i32) -> Self {
        Self {
            writer,
            levels: (0..=MAX_LOD).map(|_| FnvHashMap::default()).collect(),
            pool: Vec::new(),
            block_cache: FnvHashMap::default(),
            biome_cache: FnvHashMap::default(),
            opacity: vec![0],
            pyramid: vec![0; PYRAMID_LEN],
            palette_ids: Vec::new(),
            biome_ids: [0; 16],
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

        let by_y: FnvHashMap<i32, &Section> = sections.iter().map(|s| (s.y as i32, s)).collect();
        let (span_min, span_max) = span;
        let from = span_min.max(self.min_section_y);
        let to = span_max.min(self.max_section_y);

        for section_y in from..=to {
            let light = lighting.and_then(|l| {
                usize::try_from(section_y - span_min)
                    .ok()
                    .and_then(|i| l.get(i))
            });
            self.ingest_section(
                chunk_x,
                section_y,
                chunk_z,
                by_y.get(&section_y).copied(),
                light,
            );
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
                        let key = state_key(item);
                        match self.block_cache.get(&key) {
                            Some(&id) => id,
                            None => {
                                let (id, opacity) = self.writer.intern_block(&key, item);
                                if self.opacity.len() <= id as usize {
                                    self.opacity.resize(id as usize + 1, 15);
                                }
                                self.opacity[id as usize] = opacity;
                                self.block_cache.insert(key, id);
                                id
                            }
                        }
                    };
                    all_air &= id == 0;
                    self.palette_ids.push(id);
                }
            }
        }

        // Unlit air contributes nothing: every voxel would be zero, which is
        // what the destination buffers already hold. Skipping is what keeps
        // hollow interiors and the void below the terrain nearly free.
        if all_air
            && light.is_none_or(|(sky, block)| {
                sky.iter().all(|&n| n == 0) && block.iter().all(|&n| n == 0)
            })
        {
            return;
        }

        let sky = light.map(|(s, _)| s.as_slice());
        let block_light = light.map(|(_, b)| b.as_slice());

        let data = section.and_then(|s| s.block_states.data.as_ref());
        self.fill_level0(data, sky, block_light);
        self.build_mips();

        for lvl in 0..=MAX_LOD {
            self.insert_level(lvl, chunk_x, section_y, chunk_z);
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

        let nibble = |arr: Option<&[i8]>, i: usize| -> u8 {
            let Some(arr) = arr else { return 0 };
            let Some(&byte) = arr.get(i >> 1) else {
                return 0;
            };
            let byte = byte as u8;
            if i & 1 == 1 {
                byte >> 4
            } else {
                byte & 0x0F
            }
        };

        for i in 0..4096usize {
            let block = match data {
                None => self.palette_ids[0],
                Some(longs) => {
                    let long_index = i / per_long;
                    let shift = (i % per_long) * bits;
                    match longs.get(long_index) {
                        Some(&word) => {
                            let slot = ((word as u64 >> shift) & mask) as usize;
                            self.palette_ids.get(slot).copied().unwrap_or(0)
                        }
                        None => 0,
                    }
                }
            };
            let light = nibble(sky, i) | (nibble(block_light, i) << 4);
            let x = i & 0xF;
            let z = (i >> 4) & 0xF;
            let biome = self.biome_ids[(z >> 2) * 4 + (x >> 2)];
            self.pyramid[i] = compose(light, block, biome);
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

            for y in 0..dst_side {
                for z in 0..dst_side {
                    for x in 0..dst_side {
                        let mut children = [0u64; 8];
                        for (corner, slot) in children.iter_mut().enumerate() {
                            let dx = (corner >> 2) & 1;
                            let dy = (corner >> 1) & 1;
                            let dz = corner & 1;
                            let index = ((y * 2 + dy) << (2 * src_shift))
                                | ((z * 2 + dz) << src_shift)
                                | (x * 2 + dx);
                            *slot = self.pyramid[src_base + index];
                        }
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
        let coord_mask = (1i32 << (lvl + 1)) - 1;

        let base_x = ((chunk_x & coord_mask) as usize) * side;
        let base_y = ((section_y & coord_mask) as usize) * side;
        let base_z = ((chunk_z & coord_mask) as usize) * side;

        let section = Self::section_mut(
            &mut self.levels[lvl],
            &mut self.pool,
            chunk_x >> (lvl + 1),
            section_y >> (lvl + 1),
            chunk_z >> (lvl + 1),
        );

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

    fn section_mut<'s>(
        level: &'s mut FnvHashMap<i32, LodSection>,
        pool: &mut Vec<Vec<u64>>,
        x: i32,
        y: i32,
        z: i32,
    ) -> &'s mut LodSection {
        level.entry(y).or_insert_with(|| {
            let data = match pool.pop() {
                Some(mut buf) => {
                    buf.fill(0);
                    buf
                }
                None => vec![0u64; SECTION_VOLUME],
            };
            LodSection {
                x,
                y,
                z,
                data,
                non_air: 0,
                children: 0,
            }
        })
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
                        let octant =
                            (section.x & 1) | ((section.y & 1) << 2) | ((section.z & 1) << 1);
                        parent.children |= 1 << octant;
                    }
                }
                let key = section_key(lvl as i32, section.x, section.y, section.z);
                let blob = serialize_section(key, &section.data, children);
                self.writer.put_section(key, &blob);
                self.sections_written += 1;
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
