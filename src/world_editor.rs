use crate::block_definitions::*;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use fastanvil::Region;
use fastnbt::{ByteArray, LongArray, Value};
use fnv::FnvHashMap;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

const DATA_VERSION: i32 = 3700;

/// Formats a single text string into four lines suitable for Minecraft signs.
/// Each line is limited to 15 characters and double quotes are replaced with
/// single quotes to keep the resulting NBT valid. Excess text is wrapped to the
/// next line and anything beyond four lines is truncated.
pub fn format_sign_text(text: &str) -> (String, String, String, String) {
    let sanitized = text.replace('"', "'");

    let mut lines: Vec<String> = sanitized
        .split('\n')
        .flat_map(|segment| {
            let chars: Vec<char> = segment.chars().collect();
            if chars.is_empty() {
                vec![String::new()]
            } else {
                chars
                    .chunks(15)
                    .map(|chunk| chunk.iter().collect::<String>())
                    .collect::<Vec<_>>()
            }
        })
        .collect();

    lines.truncate(4);
    while lines.len() < 4 {
        lines.push(String::new());
    }

    (
        lines[0].clone(),
        lines[1].clone(),
        lines[2].clone(),
        lines[3].clone(),
    )
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Chunk {
    sections: Vec<Section>,
    x_pos: i32,
    z_pos: i32,
    #[serde(default)]
    is_light_on: u8,
    #[serde(flatten)]
    other: FnvHashMap<String, Value>,
}

#[derive(Serialize, Deserialize)]
struct Section {
    block_states: Blockstates,
    #[serde(rename = "Y")]
    y: i8,
    #[serde(default)]
    sky_light: Option<ByteArray>,
    #[serde(default)]
    block_light: Option<ByteArray>,
    #[serde(flatten)]
    other: FnvHashMap<String, Value>,
}

#[derive(Serialize, Deserialize)]
struct Blockstates {
    palette: Vec<PaletteItem>,
    data: Option<LongArray>,
    #[serde(flatten)]
    other: FnvHashMap<String, Value>,
}

#[derive(Serialize, Deserialize)]
struct PaletteItem {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Properties")]
    properties: Option<Value>,
}

struct SectionToModify {
    blocks: [Block; 4096],
    // Store properties for blocks that have them, indexed by the same index as blocks array
    properties: FnvHashMap<usize, Value>,
}

impl SectionToModify {
    fn get_block(&self, x: u8, y: u8, z: u8) -> Option<Block> {
        let b = self.blocks[Self::index(x, y, z)];
        if b == AIR {
            return None;
        }

        Some(b)
    }

    fn set_block(&mut self, x: u8, y: u8, z: u8, block: Block) {
        self.blocks[Self::index(x, y, z)] = block;
    }

    fn set_block_with_properties(
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

    fn index(x: u8, y: u8, z: u8) -> usize {
        usize::from(y) % 16 * 256 + usize::from(z) * 16 + usize::from(x)
    }

    fn to_section(&self, y: i8) -> Section {
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
            sky_light: None,
            block_light: None,
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

#[derive(Default)]
struct ChunkToModify {
    sections: FnvHashMap<i8, SectionToModify>,
    other: FnvHashMap<String, Value>,
}

impl ChunkToModify {
    fn get_block(&self, x: u8, y: i32, z: u8) -> Option<Block> {
        let section_idx: i8 = (y >> 4).try_into().unwrap();

        let section = self.sections.get(&section_idx)?;

        section.get_block(x, (y & 15).try_into().unwrap(), z)
    }

    fn set_block(&mut self, x: u8, y: i32, z: u8, block: Block) {
        let section_idx: i8 = (y >> 4).try_into().unwrap();

        let section = self.sections.entry(section_idx).or_default();

        section.set_block(x, (y & 15).try_into().unwrap(), z, block);
    }

    fn set_block_with_properties(
        &mut self,
        x: u8,
        y: i32,
        z: u8,
        block_with_props: BlockWithProperties,
    ) {
        let section_idx: i8 = (y >> 4).try_into().unwrap();

        let section = self.sections.entry(section_idx).or_default();

        section.set_block_with_properties(x, (y & 15).try_into().unwrap(), z, block_with_props);
    }

    fn sections(&self) -> impl Iterator<Item = Section> + '_ {
        self.sections.iter().map(|(y, s)| s.to_section(*y))
    }
}

#[derive(Default)]
struct RegionToModify {
    chunks: FnvHashMap<(i32, i32), ChunkToModify>,
}

impl RegionToModify {
    fn get_or_create_chunk(&mut self, x: i32, z: i32) -> &mut ChunkToModify {
        self.chunks.entry((x, z)).or_default()
    }

    fn get_chunk(&self, x: i32, z: i32) -> Option<&ChunkToModify> {
        self.chunks.get(&(x, z))
    }
}

#[derive(Default)]
struct WorldToModify {
    regions: FnvHashMap<(i32, i32), RegionToModify>,
}

impl WorldToModify {
    fn get_or_create_region(&mut self, x: i32, z: i32) -> &mut RegionToModify {
        self.regions.entry((x, z)).or_default()
    }

    fn get_region(&self, x: i32, z: i32) -> Option<&RegionToModify> {
        self.regions.get(&(x, z))
    }

    fn get_block(&self, x: i32, y: i32, z: i32) -> Option<Block> {
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

    fn set_block(&mut self, x: i32, y: i32, z: i32, block: Block) {
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

    fn set_block_with_properties(
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

// Notes for someone not familiar with lifetime parameter:
// The follwing is like a C++ template:
// template<lifetime A>
// struct WorldEditor {const XZBBox<A>& xzbbox;}
pub struct WorldEditor<'a> {
    region_dir: String,
    world: WorldToModify,
    xzbbox: &'a XZBBox,
    ground: Option<Box<Ground>>,
}

// template<lifetime A>
// impl for struct WorldEditor<A> {...}
impl<'a> WorldEditor<'a> {
    // Initializes the WorldEditor with the region directory and template region path.
    pub fn new(region_dir: &str, xzbbox: &'a XZBBox) -> Self {
        Self {
            region_dir: region_dir.to_string(),
            world: WorldToModify::default(),
            xzbbox,
            ground: None,
        }
    }

    /// Sets the ground reference for elevation-based block placement
    pub fn set_ground(&mut self, ground: &Ground) {
        self.ground = Some(Box::new(ground.clone()));
    }

    /// Gets a reference to the ground data if available
    pub fn get_ground(&self) -> Option<&Ground> {
        self.ground.as_ref().map(|g| g.as_ref())
    }

    /// Calculate the absolute Y position from a ground-relative offset
    #[inline(always)]
    pub fn get_absolute_y(&self, x: i32, y_offset: i32, z: i32) -> i32 {
        if let Some(ground) = &self.ground {
            ground.level(XZPoint::new(
                x - self.xzbbox.min_x(),
                z - self.xzbbox.min_z(),
            )) + y_offset
        } else {
            y_offset // If no ground reference, use y_offset as absolute Y
        }
    }

    /// Creates a region for the given region coordinates.
    fn create_region(&self, region_x: i32, region_z: i32) -> Region<File> {
        let out_path: String = format!("{}/r.{}.{}.mca", self.region_dir, region_x, region_z);

        const REGION_TEMPLATE: &[u8] = include_bytes!("../mcassets/region.template");

        let mut region_file: File = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&out_path)
            .expect("Failed to open region file");

        region_file
            .write_all(REGION_TEMPLATE)
            .expect("Could not write region template");

        Region::from_stream(region_file).expect("Failed to load region")
    }

    pub fn get_min_coords(&self) -> (i32, i32) {
        (self.xzbbox.min_x(), self.xzbbox.min_z())
    }

    pub fn get_max_coords(&self) -> (i32, i32) {
        (self.xzbbox.max_x(), self.xzbbox.max_z())
    }

    #[allow(unused)]
    #[inline]
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> bool {
        let absolute_y = self.get_absolute_y(x, y, z);
        self.world.get_block(x, absolute_y, z).is_some()
    }

    /// Places a sign at an absolute Y coordinate.
    #[allow(clippy::too_many_arguments, dead_code)]
    pub fn set_sign(
        &mut self,
        line1: String,
        line2: String,
        line3: String,
        line4: String,
        x: i32,
        y: i32,
        z: i32,
    ) {
        let absolute_y = y;
        let chunk_x = x >> 4;
        let chunk_z = z >> 4;
        let region_x = chunk_x >> 5;
        let region_z = chunk_z >> 5;

        let mut block_entities = HashMap::new();

        let lines = [line1, line2, line3, line4];
        let messages: Vec<Value> = lines
            .iter()
            .map(|l| Value::String(json!({"text": l}).to_string()))
            .collect();
        let filtered_messages: Vec<Value> = (0..4)
            .map(|_| Value::String(json!({"text": ""}).to_string()))
            .collect();

        let mut front_text = HashMap::new();
        front_text.insert("messages".to_string(), Value::List(messages.clone()));
        front_text.insert(
            "filtered_messages".to_string(),
            Value::List(filtered_messages.clone()),
        );
        front_text.insert("color".to_string(), Value::String("black".to_string()));
        front_text.insert("has_glowing_text".to_string(), Value::Byte(0));

        let mut back_text = HashMap::new();
        back_text.insert("messages".to_string(), Value::List(messages));
        back_text.insert(
            "filtered_messages".to_string(),
            Value::List(filtered_messages),
        );
        back_text.insert("color".to_string(), Value::String("black".to_string()));
        back_text.insert("has_glowing_text".to_string(), Value::Byte(0));

        block_entities.insert("front_text".to_string(), Value::Compound(front_text));
        block_entities.insert("back_text".to_string(), Value::Compound(back_text));
        block_entities.insert(
            "id".to_string(),
            Value::String("minecraft:sign".to_string()),
        );
        block_entities.insert("is_waxed".to_string(), Value::Byte(0));
        block_entities.insert("keepPacked".to_string(), Value::Byte(0));
        block_entities.insert("x".to_string(), Value::Int(x));
        block_entities.insert("y".to_string(), Value::Int(absolute_y));
        block_entities.insert("z".to_string(), Value::Int(z));

        let region: &mut RegionToModify = self.world.get_or_create_region(region_x, region_z);
        let chunk: &mut ChunkToModify = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

        if let Some(chunk_data) = chunk.other.get_mut("block_entities") {
            if let Value::List(entities) = chunk_data {
                entities.push(Value::Compound(block_entities));
            }
        } else {
            chunk.other.insert(
                "block_entities".to_string(),
                Value::List(vec![Value::Compound(block_entities)]),
            );
        }

        // Explicitly set all sign properties.
        let mut props = HashMap::new();
        props.insert("rotation".to_string(), Value::String("0".to_string()));
        props.insert(
            "waterlogged".to_string(),
            Value::String("false".to_string()),
        );
        let sign_block = BlockWithProperties::new(SIGN, Some(Value::Compound(props)));

        // Ensure that the sign is always placed even if another block
        // already occupies the target position. An empty blacklist allows
        // overriding any existing block, which is necessary because signs
        // are typically placed next to roads where terrain or vegetation
        // might have been generated earlier.
        if self.world.get_block(x, absolute_y - 1, z).is_none() {
            self.set_block_absolute(DIRT, x, absolute_y - 1, z, None, Some(&[]));
        }
        self.set_block_with_properties_absolute(sign_block, x, absolute_y, z, None, Some(&[]));
    }

    /// Sets a block of the specified type at the given coordinates.
    /// Y value is interpreted as an offset from ground level.
    #[inline]
    pub fn set_block(
        &mut self,
        block: Block,
        x: i32,
        y: i32,
        z: i32,
        override_whitelist: Option<&[Block]>,
        override_blacklist: Option<&[Block]>,
    ) {
        // Check if coordinates are within bounds
        if !self.xzbbox.contains(&XZPoint::new(x, z)) {
            return;
        }

        // Calculate the absolute Y coordinate based on ground level
        let absolute_y = self.get_absolute_y(x, y, z);

        let should_insert = if let Some(existing_block) = self.world.get_block(x, absolute_y, z) {
            // Check against whitelist and blacklist
            if let Some(whitelist) = override_whitelist {
                whitelist
                    .iter()
                    .any(|whitelisted_block: &Block| *whitelisted_block == existing_block)
            } else if let Some(blacklist) = override_blacklist {
                !blacklist
                    .iter()
                    .any(|blacklisted_block: &Block| *blacklisted_block == existing_block)
            } else {
                false
            }
        } else {
            true
        };

        if should_insert {
            self.world.set_block(x, absolute_y, z, block);
        }
    }

    /// Sets a block of the specified type at the given coordinates with absolute Y value.
    #[inline]
    pub fn set_block_absolute(
        &mut self,
        block: Block,
        x: i32,
        absolute_y: i32,
        z: i32,
        override_whitelist: Option<&[Block]>,
        override_blacklist: Option<&[Block]>,
    ) {
        // Check if coordinates are within bounds
        if !self.xzbbox.contains(&XZPoint::new(x, z)) {
            return;
        }

        let should_insert = if let Some(existing_block) = self.world.get_block(x, absolute_y, z) {
            // Check against whitelist and blacklist
            if let Some(whitelist) = override_whitelist {
                whitelist
                    .iter()
                    .any(|whitelisted_block: &Block| *whitelisted_block == existing_block)
            } else if let Some(blacklist) = override_blacklist {
                !blacklist
                    .iter()
                    .any(|blacklisted_block: &Block| *blacklisted_block == existing_block)
            } else {
                false
            }
        } else {
            true
        };

        if should_insert {
            self.world.set_block(x, absolute_y, z, block);
        }
    }

    /// Sets a block with properties at the given coordinates with absolute Y value.
    #[inline]
    pub fn set_block_with_properties_absolute(
        &mut self,
        block_with_props: BlockWithProperties,
        x: i32,
        absolute_y: i32,
        z: i32,
        override_whitelist: Option<&[Block]>,
        override_blacklist: Option<&[Block]>,
    ) {
        // Check if coordinates are within bounds
        if !self.xzbbox.contains(&XZPoint::new(x, z)) {
            return;
        }

        let should_insert = if let Some(existing_block) = self.world.get_block(x, absolute_y, z) {
            // Check against whitelist and blacklist
            if let Some(whitelist) = override_whitelist {
                whitelist
                    .iter()
                    .any(|whitelisted_block: &Block| *whitelisted_block == existing_block)
            } else if let Some(blacklist) = override_blacklist {
                !blacklist
                    .iter()
                    .any(|blacklisted_block: &Block| *blacklisted_block == existing_block)
            } else {
                false
            }
        } else {
            true
        };

        if should_insert {
            self.world
                .set_block_with_properties(x, absolute_y, z, block_with_props);
        }
    }

    /// Fills a cuboid area with the specified block between two coordinates.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn fill_blocks(
        &mut self,
        block: Block,
        x1: i32,
        y1: i32,
        z1: i32,
        x2: i32,
        y2: i32,
        z2: i32,
        override_whitelist: Option<&[Block]>,
        override_blacklist: Option<&[Block]>,
    ) {
        let (min_x, max_x) = if x1 < x2 { (x1, x2) } else { (x2, x1) };
        let (min_y, max_y) = if y1 < y2 { (y1, y2) } else { (y2, y1) };
        let (min_z, max_z) = if z1 < z2 { (z1, z2) } else { (z2, z1) };

        for x in min_x..=max_x {
            for y_offset in min_y..=max_y {
                for z in min_z..=max_z {
                    self.set_block(
                        block,
                        x,
                        y_offset,
                        z,
                        override_whitelist,
                        override_blacklist,
                    );
                }
            }
        }
    }

    /// Fills a cuboid area with the specified block between two coordinates using absolute Y values.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn fill_blocks_absolute(
        &mut self,
        block: Block,
        x1: i32,
        y1_absolute: i32,
        z1: i32,
        x2: i32,
        y2_absolute: i32,
        z2: i32,
        override_whitelist: Option<&[Block]>,
        override_blacklist: Option<&[Block]>,
    ) {
        let (min_x, max_x) = if x1 < x2 { (x1, x2) } else { (x2, x1) };
        let (min_y, max_y) = if y1_absolute < y2_absolute {
            (y1_absolute, y2_absolute)
        } else {
            (y2_absolute, y1_absolute)
        };
        let (min_z, max_z) = if z1 < z2 { (z1, z2) } else { (z2, z1) };

        for x in min_x..=max_x {
            for absolute_y in min_y..=max_y {
                for z in min_z..=max_z {
                    self.set_block_absolute(
                        block,
                        x,
                        absolute_y,
                        z,
                        override_whitelist,
                        override_blacklist,
                    );
                }
            }
        }
    }

    /// Checks for a block at the given coordinates.
    #[inline]
    pub fn check_for_block(&self, x: i32, y: i32, z: i32, whitelist: Option<&[Block]>) -> bool {
        let absolute_y = self.get_absolute_y(x, y, z);

        // Retrieve the chunk modification map
        if let Some(existing_block) = self.world.get_block(x, absolute_y, z) {
            if let Some(whitelist) = whitelist {
                if whitelist
                    .iter()
                    .any(|whitelisted_block: &Block| *whitelisted_block == existing_block)
                {
                    return true; // Block is in the list
                }
            }
        }
        false
    }

    /// Checks for a block at the given coordinates with absolute Y value.
    #[allow(unused)]
    pub fn check_for_block_absolute(
        &self,
        x: i32,
        absolute_y: i32,
        z: i32,
        whitelist: Option<&[Block]>,
        blacklist: Option<&[Block]>,
    ) -> bool {
        // Retrieve the chunk modification map
        if let Some(existing_block) = self.world.get_block(x, absolute_y, z) {
            // Check against whitelist and blacklist
            if let Some(whitelist) = whitelist {
                if whitelist
                    .iter()
                    .any(|whitelisted_block: &Block| *whitelisted_block == existing_block)
                {
                    return true; // Block is in whitelist
                }
                return false;
            }
            if let Some(blacklist) = blacklist {
                if blacklist
                    .iter()
                    .any(|blacklisted_block: &Block| *blacklisted_block == existing_block)
                {
                    return true; // Block is in blacklist
                }
            }
            return whitelist.is_none() && blacklist.is_none();
        }

        false
    }

    /// Checks if a block exists at the given coordinates with absolute Y value.
    /// Unlike check_for_block_absolute, this doesn't filter by block type.
    #[allow(unused)]
    pub fn block_at_absolute(&self, x: i32, absolute_y: i32, z: i32) -> bool {
        self.world.get_block(x, absolute_y, z).is_some()
    }

    /// Helper function to create a base chunk with grass blocks at Y -62
    fn create_base_chunk(abs_chunk_x: i32, abs_chunk_z: i32) -> (Vec<u8>, bool) {
        let mut chunk = ChunkToModify::default();

        // Fill the bottom layer with grass blocks at Y -62
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, -62, z, GRASS_BLOCK);
            }
        }

        // Prepare chunk data
        let chunk_data = Chunk {
            sections: chunk.sections().collect(),
            x_pos: abs_chunk_x,
            z_pos: abs_chunk_z,
            is_light_on: 0,
            other: chunk.other,
        };

        // Build the root NBT structure for the chunk
        let sections = Value::List(
            chunk_data
                .sections
                .iter()
                .map(|section| {
                    let mut map = HashMap::from([
                        ("Y".to_string(), Value::Byte(section.y)),
                        (
                            "block_states".to_string(),
                            Value::Compound(HashMap::from([
                                (
                                    "palette".to_string(),
                                    Value::List(
                                        section
                                            .block_states
                                            .palette
                                            .iter()
                                            .map(|item| {
                                                Value::Compound(HashMap::from([(
                                                    "Name".to_string(),
                                                    Value::String(item.name.clone()),
                                                )]))
                                            })
                                            .collect(),
                                    ),
                                ),
                                (
                                    "data".to_string(),
                                    Value::LongArray(
                                        section
                                            .block_states
                                            .data
                                            .clone()
                                            .unwrap_or_else(|| LongArray::new(vec![])),
                                    ),
                                ),
                            ])),
                        ),
                    ]);
                    if let Some(bl) = &section.block_light {
                        map.insert("block_light".to_string(), Value::ByteArray(bl.clone()));
                    }
                    if let Some(sl) = &section.sky_light {
                        map.insert("sky_light".to_string(), Value::ByteArray(sl.clone()));
                    }
                    Value::Compound(map)
                })
                .collect(),
        );

        let mut root = HashMap::from([
            ("DataVersion".to_string(), Value::Int(DATA_VERSION)),
            ("xPos".to_string(), Value::Int(abs_chunk_x)),
            ("zPos".to_string(), Value::Int(abs_chunk_z)),
            ("InhabitedTime".to_string(), Value::Long(0)),
            ("LastUpdate".to_string(), Value::Long(0)),
            ("isLightOn".to_string(), Value::Byte(0)),
            ("status".to_string(), Value::String("full".to_string())),
            ("sections".to_string(), sections),
        ]);

        // Include any additional top-level fields
        for (k, v) in chunk_data.other.iter() {
            root.insert(k.clone(), v.clone());
        }

        // Serialize the chunk
        let mut ser_buffer = Vec::with_capacity(8192);
        fastnbt::to_writer(&mut ser_buffer, &root).unwrap();

        (ser_buffer, true)
    }

    /// Saves all changes made to the world by writing modified chunks to the appropriate region files.
    pub fn save(&mut self) {
        println!("{} Saving world...", "[7/7]".bold());
        emit_gui_progress_update(90.0, "Saving world...");

        let total_regions = self.world.regions.len() as u64;
        let save_pb = ProgressBar::new(total_regions);
        save_pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] [{bar:45}] {pos}/{len} regions ({eta})",
                )
                .unwrap()
                .progress_chars("█▓░"),
        );

        let total_steps: f64 = 9.0;
        let progress_increment_save: f64 = total_steps / total_regions as f64;
        let current_progress = AtomicU64::new(900);
        let regions_processed = AtomicU64::new(0);

        self.world
            .regions
            .par_iter()
            .for_each(|((region_x, region_z), region_to_modify)| {
                let mut region = self.create_region(*region_x, *region_z);
                let mut ser_buffer = Vec::with_capacity(8192);

                for (&(chunk_x, chunk_z), chunk_to_modify) in &region_to_modify.chunks {
                    if !chunk_to_modify.sections.is_empty() || !chunk_to_modify.other.is_empty() {
                        // Read existing chunk data if it exists
                        let existing_data = region
                            .read_chunk(chunk_x as usize, chunk_z as usize)
                            .unwrap()
                            .unwrap_or_default();

                        // Parse existing chunk or create new one
                        let mut chunk: Chunk = if !existing_data.is_empty() {
                            let mut existing: Chunk = fastnbt::from_bytes(&existing_data).unwrap();
                            existing.is_light_on = 0;
                            existing
                        } else {
                            Chunk {
                                sections: Vec::new(),
                                x_pos: chunk_x + (region_x * 32),
                                z_pos: chunk_z + (region_z * 32),
                                is_light_on: 0,
                                other: FnvHashMap::default(),
                            }
                        };

                        // Normalize palette block names from NBT
                        for section in &mut chunk.sections {
                            for palette_item in &mut section.block_states.palette {
                                palette_item.name =
                                    Block::from_str(&palette_item.name).name().to_string();
                            }
                            section.sky_light = None;
                            section.block_light = None;
                        }

                        // Update sections while preserving existing data
                        let new_sections: Vec<Section> = chunk_to_modify.sections().collect();
                        for new_section in new_sections {
                            if let Some(existing_section) =
                                chunk.sections.iter_mut().find(|s| s.y == new_section.y)
                            {
                                // Merge block states
                                existing_section.block_states.palette =
                                    new_section.block_states.palette;
                                existing_section.block_states.data = new_section.block_states.data;
                                existing_section.sky_light = new_section.sky_light;
                                existing_section.block_light = new_section.block_light;
                            } else {
                                // Add new section if it doesn't exist
                                chunk.sections.push(new_section);
                            }
                        }

                        // Preserve existing block entities and merge with new ones
                        if let Some(existing_entities) = chunk.other.get_mut("block_entities") {
                            if let Some(new_entities) = chunk_to_modify.other.get("block_entities")
                            {
                                if let (Value::List(existing), Value::List(new)) =
                                    (existing_entities, new_entities)
                                {
                                    // Remove old entities that are replaced by new ones
                                    existing.retain(|e| {
                                        if let Value::Compound(map) = e {
                                            let (x, y, z) = get_entity_coords(map);
                                            !new.iter().any(|new_e| {
                                                if let Value::Compound(new_map) = new_e {
                                                    let (nx, ny, nz) = get_entity_coords(new_map);
                                                    x == nx && y == ny && z == nz
                                                } else {
                                                    false
                                                }
                                            })
                                        } else {
                                            true
                                        }
                                    });
                                    // Add new entities
                                    existing.extend(new.clone());
                                }
                            }
                        } else {
                            // If no existing entities, just add the new ones
                            if let Some(new_entities) = chunk_to_modify.other.get("block_entities")
                            {
                                chunk
                                    .other
                                    .insert("block_entities".to_string(), new_entities.clone());
                            }
                        }

                        // Update chunk coordinates and flags
                        chunk.x_pos = chunk_x + (region_x * 32);
                        chunk.z_pos = chunk_z + (region_z * 32);
                        chunk.is_light_on = 0;

                        // Create Level wrapper and save
                        let level_data = create_level_wrapper(&chunk);
                        ser_buffer.clear();
                        fastnbt::to_writer(&mut ser_buffer, &level_data).unwrap();
                        region
                            .write_chunk(chunk_x as usize, chunk_z as usize, &ser_buffer)
                            .unwrap();
                    }
                }

                // Second pass: ensure all chunks exist
                for chunk_x in 0..32 {
                    for chunk_z in 0..32 {
                        let abs_chunk_x = chunk_x + (region_x * 32);
                        let abs_chunk_z = chunk_z + (region_z * 32);

                        // Check if chunk exists in our modifications
                        let chunk_exists =
                            region_to_modify.chunks.contains_key(&(chunk_x, chunk_z));

                        // If chunk doesn't exist, create it with base layer
                        if !chunk_exists {
                            let (ser_buffer, _) = Self::create_base_chunk(abs_chunk_x, abs_chunk_z);
                            region
                                .write_chunk(chunk_x as usize, chunk_z as usize, &ser_buffer)
                                .unwrap();
                        }
                    }
                }

                // Update progress
                let regions_done = regions_processed.fetch_add(1, Ordering::SeqCst);
                let new_progress = (90.0 + (regions_done as f64 * progress_increment_save)) * 10.0;
                let prev_progress =
                    current_progress.fetch_max(new_progress as u64, Ordering::SeqCst);

                if new_progress as u64 - prev_progress > 1 {
                    emit_gui_progress_update(new_progress / 10.0, "Saving world...");
                }

                save_pb.inc(1);
            });

        save_pb.finish();
    }
}

// Helper function to get entity coordinates
#[inline]
fn get_entity_coords(entity: &HashMap<String, Value>) -> (i32, i32, i32) {
    let x = if let Value::Int(x) = entity.get("x").unwrap_or(&Value::Int(0)) {
        *x
    } else {
        0
    };
    let y = if let Value::Int(y) = entity.get("y").unwrap_or(&Value::Int(0)) {
        *y
    } else {
        0
    };
    let z = if let Value::Int(z) = entity.get("z").unwrap_or(&Value::Int(0)) {
        *z
    } else {
        0
    };
    (x, y, z)
}

fn create_level_wrapper(chunk: &Chunk) -> HashMap<String, Value> {
    let sections = Value::List(
        chunk
            .sections
            .iter()
            .map(|section| {
                let mut map = HashMap::from([
                    ("Y".to_string(), Value::Byte(section.y)),
                    (
                        "block_states".to_string(),
                        Value::Compound(HashMap::from([
                            (
                                "palette".to_string(),
                                Value::List(
                                    section
                                        .block_states
                                        .palette
                                        .iter()
                                        .map(|item| {
                                            let mut palette_item = HashMap::from([(
                                                "Name".to_string(),
                                                Value::String(item.name.clone()),
                                            )]);
                                            if let Some(props) = &item.properties {
                                                palette_item.insert(
                                                    "Properties".to_string(),
                                                    props.clone(),
                                                );
                                            }
                                            Value::Compound(palette_item)
                                        })
                                        .collect(),
                                ),
                            ),
                            (
                                "data".to_string(),
                                Value::LongArray(
                                    section
                                        .block_states
                                        .data
                                        .clone()
                                        .unwrap_or_else(|| LongArray::new(vec![])),
                                ),
                            ),
                        ])),
                    ),
                ]);
                if let Some(bl) = &section.block_light {
                    map.insert("block_light".to_string(), Value::ByteArray(bl.clone()));
                }
                if let Some(sl) = &section.sky_light {
                    map.insert("sky_light".to_string(), Value::ByteArray(sl.clone()));
                }
                Value::Compound(map)
            })
            .collect(),
    );

    let mut root = HashMap::from([
        ("DataVersion".to_string(), Value::Int(DATA_VERSION)),
        ("xPos".to_string(), Value::Int(chunk.x_pos)),
        ("zPos".to_string(), Value::Int(chunk.z_pos)),
        ("InhabitedTime".to_string(), Value::Long(0)),
        ("LastUpdate".to_string(), Value::Long(0)),
        ("isLightOn".to_string(), Value::Byte(0)),
        ("status".to_string(), Value::String("full".to_string())),
        ("sections".to_string(), sections),
    ]);

    for (k, v) in chunk.other.iter() {
        root.insert(k.clone(), v.clone());
    }

    root
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_sign_text_wraps_and_sanitizes() {
        let (l1, l2, l3, l4) = format_sign_text("A very long \"street\" name that needs wrapping");
        assert!(l1.len() <= 15);
        assert!(l2.len() <= 15);
        assert!(l3.len() <= 15);
        assert!(l4.len() <= 15);
        assert!(!l1.contains('"'));
        assert!(!l2.contains('"'));
        assert!(!l3.contains('"'));
        assert!(!l4.contains('"'));
    }

    #[test]
    fn format_sign_text_truncates_after_four_lines() {
        let input = "0123456789abcde".repeat(5);
        let (l1, l2, l3, l4) = format_sign_text(&input);
        assert_eq!(l1, "0123456789abcde");
        assert_eq!(l2, "0123456789abcde");
        assert_eq!(l3, "0123456789abcde");
        assert_eq!(l4, "0123456789abcde");
    }

    #[test]
    fn palette_item_contains_namespaced_names() {
        use crate::block_definitions::OAK_PLANKS;

        let mut section = SectionToModify::default();
        section.set_block(0, 0, 0, OAK_PLANKS);

        let nbt_section = section.to_section(0);
        assert!(nbt_section
            .block_states
            .palette
            .iter()
            .any(|p| p.name == "minecraft:oak_planks"));
    }

    #[test]
    fn sign_block_serializes_with_rotation_and_waterlogged() {
        use crate::block_definitions::SIGN;
        use std::collections::HashMap;

        let mut section = SectionToModify::default();

        // Build properties starting from the sign's defaults and override rotation.
        let mut props = match SIGN.properties() {
            Some(Value::Compound(map)) => map,
            _ => HashMap::new(),
        };
        props.insert("rotation".to_string(), Value::String("4".to_string()));
        let sign_block = BlockWithProperties::new(SIGN, Some(Value::Compound(props)));

        section.set_block_with_properties(0, 0, 0, sign_block);

        let nbt_section = section.to_section(0);
        let sign_palette = nbt_section
            .block_states
            .palette
            .iter()
            .find(|p| p.name == "minecraft:oak_sign")
            .expect("sign palette entry");

        match &sign_palette.properties {
            Some(Value::Compound(map)) => {
                assert_eq!(map.get("rotation"), Some(&Value::String("4".to_string())));
                assert_eq!(
                    map.get("waterlogged"),
                    Some(&Value::String("false".to_string()))
                );
            }
            _ => panic!("sign properties missing"),
        }
    }
}
