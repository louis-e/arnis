use crate::block_definitions::*;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use fastanvil::Region;
use fastnbt::{LongArray, Value};
use fnv::FnvHashMap;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

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

    fn index(x: u8, y: u8, z: u8) -> usize {
        usize::from(y) % 16 * 256 + usize::from(z) * 16 + usize::from(x)
    }

    fn to_section(&self, y: i8) -> Section {
        let mut palette = self.blocks.to_vec();
        palette.sort();
        palette.dedup();

        let palette_lookup: FnvHashMap<_, _> = palette
            .iter()
            .enumerate()
            .map(|(k, v)| (v, i64::try_from(k).unwrap()))
            .collect();

        let mut bits_per_block = 4; // minimum allowed
        while (1 << bits_per_block) < palette.len() {
            bits_per_block += 1;
        }

        let mut data = vec![];

        let mut cur = 0;
        let mut cur_idx = 0;
        for block in &self.blocks {
            let p = palette_lookup[block];

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

        let palette = palette
            .iter()
            .map(|x| PaletteItem {
                name: x.name().to_string(),
                properties: x.properties(),
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
        _rotation: i8,
    ) {
        let absolute_y = self.get_absolute_y(x, y, z);
        let chunk_x = x >> 4;
        let chunk_z = z >> 4;
        let region_x = chunk_x >> 5;
        let region_z = chunk_z >> 5;

        let mut block_entities = HashMap::new();

        let messages = vec![
            Value::String(format!("\"{line1}\"")),
            Value::String(format!("\"{line2}\"")),
            Value::String(format!("\"{line3}\"")),
            Value::String(format!("\"{line4}\"")),
        ];

        let mut text_data = HashMap::new();
        text_data.insert("messages".to_string(), Value::List(messages));
        text_data.insert("color".to_string(), Value::String("black".to_string()));
        text_data.insert("has_glowing_text".to_string(), Value::Byte(0));

        block_entities.insert("front_text".to_string(), Value::Compound(text_data));
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

        self.set_block(SIGN, x, y, z, None, None);
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
                    .any(|whitelisted_block: &Block| whitelisted_block.id() == existing_block.id())
            } else if let Some(blacklist) = override_blacklist {
                !blacklist
                    .iter()
                    .any(|blacklisted_block: &Block| blacklisted_block.id() == existing_block.id())
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
                    .any(|whitelisted_block: &Block| whitelisted_block.id() == existing_block.id())
            } else if let Some(blacklist) = override_blacklist {
                !blacklist
                    .iter()
                    .any(|blacklisted_block: &Block| blacklisted_block.id() == existing_block.id())
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
                    .any(|whitelisted_block: &Block| whitelisted_block.id() == existing_block.id())
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
                    .any(|whitelisted_block: &Block| whitelisted_block.id() == existing_block.id())
                {
                    return true; // Block is in whitelist
                }
                return false;
            }
            if let Some(blacklist) = blacklist {
                if blacklist
                    .iter()
                    .any(|blacklisted_block: &Block| blacklisted_block.id() == existing_block.id())
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

        // Create the Level wrapper
        let level_data = HashMap::from([(
            "Level".to_string(),
            Value::Compound(HashMap::from([
                ("xPos".to_string(), Value::Int(abs_chunk_x)),
                ("zPos".to_string(), Value::Int(abs_chunk_z)),
                ("isLightOn".to_string(), Value::Byte(0)),
                (
                    "sections".to_string(),
                    Value::List(
                        chunk_data
                            .sections
                            .iter()
                            .map(|section| {
                                Value::Compound(HashMap::from([
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
                                ]))
                            })
                            .collect(),
                    ),
                ),
            ])),
        )]);

        // Serialize the chunk with Level wrapper
        let mut ser_buffer = Vec::with_capacity(8192);
        fastnbt::to_writer(&mut ser_buffer, &level_data).unwrap();

        (ser_buffer, true)
    }

    /// Saves all changes made to the world by writing modified chunks to the appropriate region files.
    pub fn save(&mut self) {
        println!("{} Saving world...", "[6/6]".bold());
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
                            fastnbt::from_bytes(&existing_data).unwrap()
                        } else {
                            Chunk {
                                sections: Vec::new(),
                                x_pos: chunk_x + (region_x * 32),
                                z_pos: chunk_z + (region_z * 32),
                                is_light_on: 0,
                                other: FnvHashMap::default(),
                            }
                        };

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

#[inline]
fn create_level_wrapper(chunk: &Chunk) -> HashMap<String, Value> {
    HashMap::from([(
        "Level".to_string(),
        Value::Compound(HashMap::from([
            ("xPos".to_string(), Value::Int(chunk.x_pos)),
            ("zPos".to_string(), Value::Int(chunk.z_pos)),
            (
                "isLightOn".to_string(),
                Value::Byte(i8::try_from(chunk.is_light_on).unwrap()),
            ),
            (
                "sections".to_string(),
                Value::List(
                    chunk
                        .sections
                        .iter()
                        .map(|section| {
                            Value::Compound(HashMap::from([
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
                            ]))
                        })
                        .collect(),
                ),
            ),
        ])),
    )])
}
