use crate::block_definitions::*;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::LLBBox;
use crate::ground::Ground;
use crate::progress::emit_gui_progress_update;
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
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
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)] // BedrockMcWorld will be used when GUI format toggle is implemented
pub enum WorldFormat {
    JavaAnvil,
    BedrockMcWorld,
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
            other: FnvHashMap::default(),
        }
    }
}

#[cfg(all(test, feature = "bedrock"))]
mod bedrock_tests {
    use super::bedrock_support::BedrockWriter;
    use super::WorldToModify;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use serde_json::Value;
    use std::fs;
    use zip::ZipArchive;

    #[test]
    fn writes_mcworld_package_with_metadata() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_dir = temp_dir.path().join("bedrock_world");

        let world = WorldToModify::default();
        let xzbbox = XZBBox::rect_from_xz_lengths(15.0, 15.0).unwrap();
        let llbbox = LLBBox::new(0.0, 0.0, 1.0, 1.0).unwrap();

        BedrockWriter::new(output_dir.clone(), "test-world".to_string())
            .write_world(&world, &xzbbox, &llbbox)
            .expect("write_world");

        let metadata_path = output_dir.join("metadata.json");
        let metadata_bytes = fs::read(&metadata_path).expect("metadata file readable");
        let metadata: Value = serde_json::from_slice(&metadata_bytes).expect("valid metadata JSON");

        assert_eq!(metadata["format"], "bedrock-mcworld");
        assert_eq!(metadata["chunk_count"], 0); // empty world structure

        let levelname_contents = fs::read_to_string(output_dir.join("levelname.txt")).unwrap();
        assert_eq!(levelname_contents, "test-world");

        assert!(output_dir.join("db").is_dir(), "db directory created");

        // Ensure .mcworld archive exists and includes stub files
        let mcworld_path = output_dir.with_extension("mcworld");
        let file = fs::File::open(&mcworld_path).expect("mcworld archive exists");
        let mut archive = ZipArchive::new(file).expect("zip readable");

        let mut entries: Vec<String> = Vec::new();
        for i in 0..archive.len() {
            if let Ok(file) = archive.by_index(i) {
                entries.push(file.name().to_string());
            }
        }
        entries.sort();

        assert!(entries.contains(&"db/".to_string()));
        assert!(entries.contains(&"levelname.txt".to_string()));
        assert!(entries.contains(&"metadata.json".to_string()));
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorldMetadata {
    min_mc_x: i32,
    max_mc_x: i32,
    min_mc_z: i32,
    max_mc_z: i32,

    min_geo_lat: f64,
    max_geo_lat: f64,
    min_geo_lon: f64,
    max_geo_lon: f64,
}

// Notes for someone not familiar with lifetime parameter:
// The follwing is like a C++ template:
// template<lifetime A>
// struct WorldEditor {const XZBBox<A>& xzbbox;}
pub struct WorldEditor<'a> {
    world_dir: PathBuf,
    world: WorldToModify,
    xzbbox: &'a XZBBox,
    llbbox: LLBBox,
    ground: Option<Box<Ground>>,
    format: WorldFormat,
    /// Optional level name for Bedrock worlds (e.g., "Arnis World: New York City")
    bedrock_level_name: Option<String>,
}

// template<lifetime A>
// impl for struct WorldEditor<A> {...}
impl<'a> WorldEditor<'a> {
    // Initializes the WorldEditor with the region directory and template region path.
    // This is the default constructor used by CLI mode - Java format only
    pub fn new(world_dir: PathBuf, xzbbox: &'a XZBBox, llbbox: LLBBox) -> Self {
        Self {
            world_dir,
            world: WorldToModify::default(),
            xzbbox,
            llbbox,
            ground: None,
            format: WorldFormat::JavaAnvil,
            bedrock_level_name: None,
        }
    }

    /// Creates a new WorldEditor with a specific format and optional level name.
    /// Used by GUI mode to support both Java and Bedrock formats.
    #[allow(dead_code)] // Will be used when GUI format toggle is implemented
    pub fn new_with_format_and_name(
        world_dir: PathBuf,
        xzbbox: &'a XZBBox,
        llbbox: LLBBox,
        format: WorldFormat,
        bedrock_level_name: Option<String>,
    ) -> Self {
        Self {
            world_dir,
            world: WorldToModify::default(),
            xzbbox,
            llbbox,
            ground: None,
            format,
            bedrock_level_name,
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

    #[allow(dead_code)]
    pub fn format(&self) -> WorldFormat {
        self.format
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
        let out_path = self
            .world_dir
            .join(format!("region/r.{}.{}.mca", region_x, region_z));

        const REGION_TEMPLATE: &[u8] = include_bytes!("../assets/minecraft/region.template");

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
        let level_data = create_level_wrapper(&chunk_data);

        // Serialize the chunk with Level wrapper
        let mut ser_buffer = Vec::with_capacity(8192);
        fastnbt::to_writer(&mut ser_buffer, &level_data).unwrap();

        (ser_buffer, true)
    }

    /// Saves all changes made to the world by writing modified chunks to the appropriate region files.
    pub fn save(&mut self) {
        match self.format {
            WorldFormat::JavaAnvil => self.save_java(),
            WorldFormat::BedrockMcWorld => self.save_bedrock(),
        }
    }

    fn save_java(&mut self) {
        println!("{} Saving world...", "[7/7]".bold());
        emit_gui_progress_update(90.0, "Saving world...");

        // Save metadata with error handling
        if let Err(e) = self.save_metadata() {
            eprintln!("Failed to save world metadata: {}", e);
            #[cfg(feature = "gui")]
            send_log(LogLevel::Warning, "Failed to save world metadata.");
            // Continue with world saving even if metadata fails
        }

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

    #[allow(unreachable_code)]
    fn save_bedrock(&mut self) {
        println!("{} Saving Bedrock world...", "[7/7]".bold());
        emit_gui_progress_update(90.0, "Saving Bedrock world...");

        #[cfg(feature = "bedrock")]
        {
            if let Err(error) = self.save_bedrock_internal() {
                eprintln!("Failed to save Bedrock world: {error}");
                #[cfg(feature = "gui")]
                send_log(
                    LogLevel::Error,
                    &format!("Failed to save Bedrock world: {error}"),
                );
            }
            return;
        }

        #[cfg(not(feature = "bedrock"))]
        {
            eprintln!(
                "Bedrock output requested but the 'bedrock' feature is not enabled at build time."
            );
            #[cfg(feature = "gui")]
            send_log(
                LogLevel::Error,
                "Bedrock output requested but the 'bedrock' feature is not enabled at build time.",
            );
        }
    }

    #[cfg(feature = "bedrock")]
    fn save_bedrock_internal(&mut self) -> Result<(), bedrock_support::BedrockSaveError> {
        // Use the stored level name if available, otherwise extract from path
        let level_name = self.bedrock_level_name.clone().unwrap_or_else(|| {
            self.world_dir
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Arnis World")
                .to_string()
        });

        bedrock_support::BedrockWriter::new(self.world_dir.clone(), level_name).write_world(
            &self.world,
            self.xzbbox,
            &self.llbbox,
        )
    }

    fn save_metadata(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let metadata_path = self.world_dir.join("metadata.json");

        let mut file = File::create(&metadata_path).map_err(|e| {
            format!(
                "Failed to create metadata file at {}: {}",
                metadata_path.display(),
                e
            )
        })?;

        let metadata = WorldMetadata {
            min_mc_x: self.xzbbox.min_x(),
            max_mc_x: self.xzbbox.max_x(),
            min_mc_z: self.xzbbox.min_z(),
            max_mc_z: self.xzbbox.max_z(),

            min_geo_lat: self.llbbox.min().lat(),
            max_geo_lat: self.llbbox.max().lat(),
            min_geo_lon: self.llbbox.min().lng(),
            max_geo_lon: self.llbbox.max().lng(),
        };

        let contents = serde_json::to_string(&metadata)
            .map_err(|e| format!("Failed to serialize metadata to JSON: {}", e))?;

        write!(&mut file, "{}", contents)
            .map_err(|e| format!("Failed to write metadata to file: {}", e))?;

        Ok(())
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
                            let mut block_states = HashMap::from([(
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
                            )]);

                            // only add the `data` attribute if it's non-empty
                            // some software (cough cough dynmap) chokes otherwise
                            if let Some(data) = &section.block_states.data {
                                if !data.is_empty() {
                                    block_states.insert(
                                        "data".to_string(),
                                        Value::LongArray(data.to_owned()),
                                    );
                                }
                            }

                            Value::Compound(HashMap::from([
                                ("Y".to_string(), Value::Byte(section.y)),
                                ("block_states".to_string(), Value::Compound(block_states)),
                            ]))
                        })
                        .collect(),
                ),
            ),
        ])),
    )])
}

#[cfg(feature = "bedrock")]
mod bedrock_support {
    use super::*;
    use crate::bedrock_block_map::{to_bedrock_block, BedrockBlock, BedrockBlockStateValue};
    use bedrockrs_level::level::db_interface::bedrock_key::ChunkKey;
    use bedrockrs_level::level::db_interface::rusty::RustyDBInterface;
    use bedrockrs_level::level::file_interface::RawWorldTrait;
    use bedrockrs_shared::world::dimension::Dimension;
    use byteorder::{LittleEndian, WriteBytesExt};
    use indicatif::{ProgressBar, ProgressStyle};
    use serde::Serialize;
    use std::collections::HashMap as StdHashMap;
    use std::fs;
    use std::io::{Cursor, Write as IoWrite};
    use vek::Vec2;
    use zip::write::FileOptions;
    use zip::CompressionMethod;
    use zip::ZipWriter;

    #[derive(Debug)]
    pub enum BedrockSaveError {
        Io(std::io::Error),
        Zip(zip::result::ZipError),
        Serialization(serde_json::Error),
        Database(String),
        Nbt(String),
    }

    impl std::fmt::Display for BedrockSaveError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                BedrockSaveError::Io(err) => {
                    write!(f, "I/O error while writing Bedrock world: {err}")
                }
                BedrockSaveError::Zip(err) => {
                    write!(f, "Failed to package Bedrock world archive: {err}")
                }
                BedrockSaveError::Serialization(err) => {
                    write!(f, "Failed to serialize Bedrock metadata: {err}")
                }
                BedrockSaveError::Database(err) => {
                    write!(f, "LevelDB error: {err}")
                }
                BedrockSaveError::Nbt(err) => {
                    write!(f, "NBT serialization error: {err}")
                }
            }
        }
    }

    impl std::error::Error for BedrockSaveError {}

    impl From<std::io::Error> for BedrockSaveError {
        fn from(err: std::io::Error) -> Self {
            BedrockSaveError::Io(err)
        }
    }

    impl From<zip::result::ZipError> for BedrockSaveError {
        fn from(err: zip::result::ZipError) -> Self {
            BedrockSaveError::Zip(err)
        }
    }

    impl From<serde_json::Error> for BedrockSaveError {
        fn from(err: serde_json::Error) -> Self {
            BedrockSaveError::Serialization(err)
        }
    }

    #[derive(Serialize)]
    struct BedrockMetadata {
        #[serde(flatten)]
        world: WorldMetadata,
        format: &'static str,
        chunk_count: usize,
    }

    /// Bedrock block state for NBT serialization
    #[derive(serde::Serialize)]
    struct BedrockBlockState {
        name: String,
        states: StdHashMap<String, BedrockNbtValue>,
    }

    /// NBT-compatible value types for Bedrock block states
    #[derive(serde::Serialize)]
    #[serde(untagged)]
    enum BedrockNbtValue {
        String(String),
        Byte(i8),
        Int(i32),
    }

    impl From<&BedrockBlockStateValue> for BedrockNbtValue {
        fn from(value: &BedrockBlockStateValue) -> Self {
            match value {
                BedrockBlockStateValue::String(s) => BedrockNbtValue::String(s.clone()),
                BedrockBlockStateValue::Bool(b) => BedrockNbtValue::Byte(if *b { 1 } else { 0 }),
                BedrockBlockStateValue::Int(i) => BedrockNbtValue::Int(*i),
            }
        }
    }

    pub struct BedrockWriter {
        output_dir: PathBuf,
        level_name: String,
    }

    impl BedrockWriter {
        pub fn new(output_path: PathBuf, level_name: String) -> Self {
            // If the path ends with .mcworld, use it as the final archive path
            // and create a temp directory without that extension for working files
            let output_dir = if output_path.extension().map_or(false, |ext| ext == "mcworld") {
                output_path.with_extension("")
            } else {
                output_path
            };
            
            Self {
                output_dir,
                level_name,
            }
        }

        pub fn write_world(
            &mut self,
            world: &WorldToModify,
            xzbbox: &XZBBox,
            llbbox: &LLBBox,
        ) -> Result<(), BedrockSaveError> {
            self.prepare_output_dir()?;
            self.write_level_name()?;
            self.write_level_dat(xzbbox)?;
            self.write_chunks_to_db(world)?;
            self.write_metadata(world, xzbbox, llbbox)?;
            self.package_mcworld()?;
            Ok(())
        }

        fn prepare_output_dir(&self) -> Result<(), BedrockSaveError> {
            // Remove existing output directory and mcworld file to avoid conflicts
            if self.output_dir.exists() {
                fs::remove_dir_all(&self.output_dir)?;
            }
            let mcworld_path = self.output_dir.with_extension("mcworld");
            if mcworld_path.exists() {
                fs::remove_file(&mcworld_path)?;
            }
            
            fs::create_dir_all(&self.output_dir)?;
            // db directory will be created by LevelDB
            Ok(())
        }

        fn write_level_name(&self) -> Result<(), BedrockSaveError> {
            let levelname_path = self.output_dir.join("levelname.txt");
            fs::write(levelname_path, &self.level_name)?;
            Ok(())
        }

        fn write_level_dat(&self, xzbbox: &XZBBox) -> Result<(), BedrockSaveError> {
            // Create a complete level.dat for Bedrock with all required fields
            // The format is: 8 bytes header + NBT data
            // Header: version (4 bytes LE) + length (4 bytes LE)

            let spawn_x = (xzbbox.min_x() + xzbbox.max_x()) / 2;
            let spawn_z = (xzbbox.min_z() + xzbbox.max_z()) / 2;

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            // Version array for Bedrock 1.21.x compatibility
            let version_array = vec![1, 21, 0, 0, 0];

            // Build complete level.dat NBT structure
            let level_dat = BedrockLevelDat {
                // Version information - critical for Bedrock to recognize the world
                storage_version: 10,
                network_version: 685, // Bedrock 1.21.0 protocol
                world_version: 1,
                inventory_version: "1.21.0".to_string(),
                last_opened_with_version: version_array.clone(),
                minimum_compatible_client_version: version_array,

                // World identity
                level_name: "Arnis World".to_string(),
                random_seed: 0,

                // Spawn location
                spawn_x,
                spawn_y: 64,
                spawn_z,

                // World generation - Flat/Void world
                generator: 2, // Flat
                flat_world_layers: r#"{"biome_id":1,"encoding_version":6,"preset_id":"TheVoid","world_version":"version.post_1_18"}"#.to_string(),
                spawn_mobs: false,

                // Game settings
                game_type: 1, // Creative
                difficulty: 2, // Normal
                force_game_type: false,

                // Time
                last_played: now,
                time: 0,
                current_tick: 0,

                // Cheats and commands
                commands_enabled: true,
                cheats_enabled: true,
                command_blocks_enabled: true,
                command_block_output: true,

                // Multiplayer
                multiplayer_game: true,
                multiplayer_game_intent: true,
                lan_broadcast: true,
                lan_broadcast_intent: true,
                xbl_broadcast_intent: 3,
                platform_broadcast_intent: 3,
                platform: 2,

                // Game rules
                do_daylight_cycle: true,
                do_weather_cycle: true,
                do_mob_spawning: false, // Disabled since spawnMobs is false
                do_mob_loot: true,
                do_tile_drops: true,
                do_entity_drops: true,
                do_fire_tick: true,
                mob_griefing: true,
                natural_regeneration: true,
                pvp: true,
                keep_inventory: false,
                send_command_feedback: true,
                show_coordinates: false,
                show_death_messages: true,
                tnt_explodes: true,
                respawn_blocks_explode: true,
                projectiles_can_break_blocks: true,

                // Damage settings
                drowning_damage: true,
                fall_damage: true,
                fire_damage: true,
                freeze_damage: true,

                // Weather
                rain_level: 0.0,
                rain_time: 100000,
                lightning_level: 0.0,
                lightning_time: 100000,

                // Misc settings
                nether_scale: 8,
                spawn_radius: 0,
                random_tick_speed: 1,
                function_command_limit: 10000,
                max_command_chain_length: 65535,
                server_chunk_tick_range: 4,
                limited_world_depth: 16,
                limited_world_width: 16,
                limited_world_origin_x: spawn_x,
                limited_world_origin_y: 64,
                limited_world_origin_z: spawn_z,
                world_start_count: 0xFFFFFFFE_u64 as i64, // Special value for new worlds

                // Boolean flags
                bonus_chest_enabled: false,
                bonus_chest_spawned: false,
                has_been_loaded_in_creative: true,
                has_locked_behavior_pack: false,
                has_locked_resource_pack: false,
                immutable_world: false,
                is_from_locked_template: false,
                is_from_world_template: false,
                is_single_use_world: false,
                is_world_template_option_locked: false,
                texture_packs_required: false,
                use_msa_gamertags_only: false,
                center_maps_to_origin: false,
                confirmed_platform_locked_content: false,
                education_features_enabled: false,
                start_with_map_enabled: false,
                requires_copied_pack_removal_check: false,
                spawn_v1_villagers: false,
                is_hardcore: false,
                is_created_in_editor: false,
                is_exported_from_editor: false,
                is_random_seed_allowed: false,
                has_uncomplete_world_file_on_disk: false,
                player_has_died: false,
                do_insomnia: true,
                do_immediate_respawn: false,
                do_limited_crafting: false,
                recipes_unlock: true,
                show_tags: true,
                show_recipe_messages: true,
                show_border_effect: true,
                show_days_played: false,
                locator_bar: true,
                tnt_explosion_drop_decay: true,
                saved_with_toggled_experiments: false,
                experiments_ever_used: false,

                // Editor
                editor_world_type: 0,
                edu_offer: 0,

                // Override
                biome_override: "".to_string(),
                prid: "".to_string(),

                // Player sleeping
                players_sleeping_percentage: 100,

                // Permissions
                permissions_level: 0,
                player_permissions_level: 1,

                // Daylight cycle
                daylight_cycle: 0,
            };

            let nbt_bytes = nbtx::to_le_bytes(&level_dat)
                .map_err(|e| BedrockSaveError::Nbt(e.to_string()))?;

            // Write with header
            let mut file = File::create(self.output_dir.join("level.dat"))?;
            // Storage version: 10 (current Bedrock format)
            file.write_u32::<LittleEndian>(10)?;
            // Length of NBT data
            file.write_u32::<LittleEndian>(nbt_bytes.len() as u32)?;
            file.write_all(&nbt_bytes)?;

            Ok(())
        }

        fn write_chunks_to_db(&self, world: &WorldToModify) -> Result<(), BedrockSaveError> {
            let db_path = self.output_dir.join("db");

            // Open LevelDB with Bedrock-compatible options
            let mut state = ();
            let mut db: RustyDBInterface<()> =
                RustyDBInterface::new(db_path.into_boxed_path(), true, &mut state)
                    .map_err(|e| BedrockSaveError::Database(format!("{:?}", e)))?;

            // Count total chunks for progress
            let total_chunks: usize = world
                .regions
                .values()
                .map(|region| region.chunks.len())
                .sum();

            if total_chunks == 0 {
                return Ok(());
            }

            let progress_bar = ProgressBar::new(total_chunks as u64);
            progress_bar.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} [{elapsed_precise}] [{bar:45.white/black}] {pos}/{len} chunks ({eta})")
                    .unwrap()
                    .progress_chars("█▓░"),
            );

            // Process each region and chunk
            for ((region_x, region_z), region) in &world.regions {
                for ((local_chunk_x, local_chunk_z), chunk) in &region.chunks {
                    // Calculate absolute chunk coordinates
                    let abs_chunk_x = region_x * 32 + local_chunk_x;
                    let abs_chunk_z = region_z * 32 + local_chunk_z;
                    let chunk_pos = Vec2::new(abs_chunk_x, abs_chunk_z);

                    // Write chunk version marker (42 is current Bedrock version as of 1.21+)
                    let version_key = ChunkKey::chunk_marker(chunk_pos, Dimension::Overworld);
                    db.set_subchunk_raw(version_key, &[42], &mut state)
                        .map_err(|e| BedrockSaveError::Database(format!("{:?}", e)))?;

                    // Write Data3D (heightmap + biomes) - required for chunk to be valid
                    let data3d_key = ChunkKey::data3d(chunk_pos, Dimension::Overworld);
                    let data3d = self.create_data3d(chunk);
                    db.set_subchunk_raw(data3d_key, &data3d, &mut state)
                        .map_err(|e| BedrockSaveError::Database(format!("{:?}", e)))?;

                    // Process each section (subchunk)
                    for (&section_y, section) in &chunk.sections {
                        // Encode the subchunk
                        let subchunk_bytes = self.encode_subchunk(section, section_y)?;

                        // Write to database
                        let subchunk_key =
                            ChunkKey::new_subchunk(chunk_pos, Dimension::Overworld, section_y);
                        db.set_subchunk_raw(subchunk_key, &subchunk_bytes, &mut state)
                            .map_err(|e| BedrockSaveError::Database(format!("{:?}", e)))?;
                    }

                    progress_bar.inc(1);
                }
            }

            progress_bar.finish_with_message("Chunks written to LevelDB");

            // Note: When db goes out of scope, the Drop implementation should flush writes.
            // If Bedrock worlds don't work properly, we may need to fork bedrockrs
            // to add explicit flush() and compact_all() methods.
            drop(db);

            Ok(())
        }

        /// Create Data3D record (heightmap + biomes)
        fn create_data3d(&self, _chunk: &ChunkToModify) -> Vec<u8> {
            // Data3D format:
            // - Heightmap: 256 entries * 2 bytes each = 512 bytes (i16 LE for each x,z position)
            // - 3D biomes: Variable, but simplified to palette format
            
            let mut buffer = Vec::with_capacity(540);
            
            // Heightmap - 256 entries (16x16) as i16 LE
            // For now, use a fixed height of 4 (ground level for superflat style)
            // This represents the highest non-air block Y coordinate
            for _ in 0..256 {
                buffer.extend_from_slice(&4i16.to_le_bytes());
            }
            
            // 3D biome data - simplified to just plains biome (id 1)
            // The biome format uses palette encoding similar to blocks
            // For simplicity, we write a minimal biome palette
            // Format: palette_type (1 byte) + optional palette data
            // Using single-value palette (all plains)
            
            // The reference world has 540 bytes total - 512 for heightmap leaves 28 for biomes
            // Let's try a minimal biome encoding
            // According to wiki, post-1.18 uses 3D biomes with subchunk granularity
            // For now, just pad with zeros to match the expected size
            
            // Actually, looking at the reference: 04 00 repeated means height = 4 for all positions
            // Then biome data follows
            
            // Let's examine what we need - maybe just 24 sub-biome palette entries
            // Each biome subchunk is 4x4x4 = 64 entries
            // Using 1 bit per block (2 palette entries) = 64/8 = 8 bytes + 4 byte palette count + NBT
            
            // For now, create empty biome section - game might generate it
            // Just ensure we have some valid data
            buffer.extend_from_slice(&[0u8; 28]); // Padding to ~540 bytes
            
            buffer
        }

        /// Encode a section into Bedrock subchunk format
        fn encode_subchunk(
            &self,
            section: &SectionToModify,
            y_index: i8,
        ) -> Result<Vec<u8>, BedrockSaveError> {
            let mut buffer = Cursor::new(Vec::new());

            // Subchunk format version (9 is current)
            buffer.write_u8(9)?;

            // Number of storage layers (we use 1)
            buffer.write_u8(1)?;

            // Y index
            buffer.write_i8(y_index)?;

            // Build palette and block indices
            let (palette, indices) = self.build_palette_and_indices(section)?;

            // Calculate bits per block using valid Bedrock values: {1, 2, 3, 4, 5, 6, 8, 16}
            let bits_per_block = bedrock_bits_per_block(palette.len() as u32);

            // Write palette type (bits << 1, not network format)
            buffer.write_u8(bits_per_block << 1)?;

            // Calculate word packing parameters (matching Chunker's PaletteUtil exactly)
            // blocksPerWord = floor(32 / bitsPerBlock)
            // wordSize = ceil(4096 / blocksPerWord)
            let blocks_per_word = 32 / bits_per_block as u32; // Integer division = floor
            let word_count = (4096 + blocks_per_word - 1) / blocks_per_word; // Ceiling division
            let mask = (1u32 << bits_per_block) - 1;

            // Pack indices into 32-bit words (matching Chunker's loop exactly)
            let mut block_index = 0usize;
            for _ in 0..word_count {
                let mut word = 0u32;
                // Important: iterate blockIndex from 0 to blocksPerWord-1
                // NOT bit_offset from 0 to 32 in steps of bits_per_block
                for block_in_word in 0..blocks_per_word {
                    if block_index >= 4096 {
                        break;
                    }
                    let start_bit_index = bits_per_block as u32 * block_in_word;
                    let index_val = indices[block_index] as u32 & mask;
                    word |= index_val << start_bit_index;
                    block_index += 1;
                }
                buffer.write_u32::<LittleEndian>(word)?;
            }

            // Write palette count
            buffer.write_u32::<LittleEndian>(palette.len() as u32)?;

            // Write palette entries as NBT
            for block in &palette {
                let state = BedrockBlockState {
                    name: block.name.clone(),
                    states: block
                        .states
                        .iter()
                        .map(|(k, v)| (k.clone(), BedrockNbtValue::from(v)))
                        .collect(),
                };
                let nbt_bytes = nbtx::to_le_bytes(&state)
                    .map_err(|e| BedrockSaveError::Nbt(e.to_string()))?;
                buffer.write_all(&nbt_bytes)?;
            }

            Ok(buffer.into_inner())
        }

        /// Build a palette and index array from a section
        /// Converts from internal YZX ordering to Bedrock's XZY ordering
        fn build_palette_and_indices(
            &self,
            section: &SectionToModify,
        ) -> Result<(Vec<BedrockBlock>, [u16; 4096]), BedrockSaveError> {
            let mut palette: Vec<BedrockBlock> = Vec::new();
            let mut palette_map: StdHashMap<String, u16> = StdHashMap::new();
            let mut indices = [0u16; 4096];

            // Add air as first palette entry
            let air_block = BedrockBlock::simple("air");
            let air_key = format!("{:?}", (&air_block.name, &air_block.states));
            palette.push(air_block);
            palette_map.insert(air_key, 0);

            // Process all blocks with coordinate conversion
            // Internal storage: Y * 256 + Z * 16 + X (YZX)
            // Bedrock storage (from Chunker PaletteUtil.java writeChunkPalette):
            //   For index i: x = (i >> 8) & 0xF, z = (i >> 4) & 0xF, y = i & 0xF
            //   So: bedrock_idx = x * 256 + z * 16 + y (XZY)
            //
            // Chunker stores blocks as values[x][y][z] and reads with values[x][y][z]
            // where x, y, z are extracted from index i as shown above.
            //
            // Internal YZX: internal_idx = y*256 + z*16 + x
            // Bedrock XZY:  bedrock_idx  = x*256 + z*16 + y
            for x in 0..16usize {
                for z in 0..16usize {
                    for y in 0..16usize {
                        // Read from internal order: y*256 + z*16 + x
                        let internal_idx = y * 256 + z * 16 + x;
                        let block = section.blocks[internal_idx];

                        let bedrock_block = to_bedrock_block(block);
                        let key = format!("{:?}", (&bedrock_block.name, &bedrock_block.states));

                        let palette_index = if let Some(&idx) = palette_map.get(&key) {
                            idx
                        } else {
                            let idx = palette.len() as u16;
                            palette_map.insert(key, idx);
                            palette.push(bedrock_block);
                            idx
                        };

                        // Write to Bedrock order: x*256 + z*16 + y
                        let bedrock_idx = x * 256 + z * 16 + y;
                        indices[bedrock_idx] = palette_index;
                    }
                }
            }

            Ok((palette, indices))
        }

        fn write_metadata(
            &self,
            world: &WorldToModify,
            xzbbox: &XZBBox,
            llbbox: &LLBBox,
        ) -> Result<(), BedrockSaveError> {
            let chunk_count = world
                .regions
                .values()
                .map(|region| region.chunks.len())
                .sum();

            let metadata = BedrockMetadata {
                world: WorldMetadata {
                    min_mc_x: xzbbox.min_x(),
                    max_mc_x: xzbbox.max_x(),
                    min_mc_z: xzbbox.min_z(),
                    max_mc_z: xzbbox.max_z(),
                    min_geo_lat: llbbox.min().lat(),
                    max_geo_lat: llbbox.max().lat(),
                    min_geo_lon: llbbox.min().lng(),
                    max_geo_lon: llbbox.max().lng(),
                },
                format: "bedrock-mcworld",
                chunk_count,
            };

            let metadata_bytes = serde_json::to_vec_pretty(&metadata)?;
            let metadata_path = self.output_dir.join("metadata.json");
            let mut file = File::create(metadata_path)?;
            file.write_all(&metadata_bytes)?;
            Ok(())
        }

        fn package_mcworld(&self) -> Result<(), BedrockSaveError> {
            let mcworld_path = self.output_dir.with_extension("mcworld");
            let file = File::create(&mcworld_path)?;
            let mut writer = ZipWriter::new(file);
            let options = FileOptions::default().compression_method(CompressionMethod::Deflated);

            // Add top-level files
            for file_name in ["levelname.txt", "metadata.json", "level.dat"] {
                let path = self.output_dir.join(file_name);
                if path.exists() {
                    writer.start_file(file_name, options)?;
                    let contents = fs::read(&path)?;
                    writer.write_all(&contents)?;
                }
            }

            // Add world_icon.jpeg from assets
            let icon_path = std::path::Path::new("assets/minecraft/world_icon.jpeg");
            if icon_path.exists() {
                writer.start_file("world_icon.jpeg", options)?;
                let contents = fs::read(icon_path)?;
                writer.write_all(&contents)?;
            }

            // Add db directory and its contents
            let db_path = self.output_dir.join("db");
            if db_path.is_dir() {
                self.add_directory_to_zip(&mut writer, &db_path, "db", options)?;
            }

            writer.finish()?;
            Ok(())
        }

        fn add_directory_to_zip(
            &self,
            writer: &mut ZipWriter<File>,
            dir_path: &std::path::Path,
            zip_prefix: &str,
            options: FileOptions,
        ) -> Result<(), BedrockSaveError> {
            // Add directory entry
            writer.add_directory(format!("{}/", zip_prefix), options)?;

            // Add all files in directory
            for entry in fs::read_dir(dir_path)? {
                let entry = entry?;
                let path = entry.path();
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown");
                let zip_path = format!("{}/{}", zip_prefix, name);

                if path.is_file() {
                    writer.start_file(&zip_path, options)?;
                    let contents = fs::read(&path)?;
                    writer.write_all(&contents)?;
                } else if path.is_dir() {
                    self.add_directory_to_zip(writer, &path, &zip_path, options)?;
                }
            }

            Ok(())
        }
    }

    /// Calculate bits per block using valid Bedrock values: {1, 2, 3, 4, 5, 6, 8, 16}
    fn bedrock_bits_per_block(palette_count: u32) -> u8 {
        const VALID_BITS: [u8; 8] = [1, 2, 3, 4, 5, 6, 8, 16];
        for &bits in &VALID_BITS {
            if palette_count <= (1u32 << bits) {
                return bits;
            }
        }
        16 // Maximum
    }

    /// Level.dat structure for Bedrock Edition
    /// This struct contains all required fields for a valid Bedrock world
    #[derive(serde::Serialize)]
    struct BedrockLevelDat {
        // Version information
        #[serde(rename = "StorageVersion")]
        storage_version: i32,
        #[serde(rename = "NetworkVersion")]
        network_version: i32,
        #[serde(rename = "WorldVersion")]
        world_version: i32,
        #[serde(rename = "InventoryVersion")]
        inventory_version: String,
        #[serde(rename = "lastOpenedWithVersion")]
        last_opened_with_version: Vec<i32>,
        #[serde(rename = "MinimumCompatibleClientVersion")]
        minimum_compatible_client_version: Vec<i32>,

        // World identity
        #[serde(rename = "LevelName")]
        level_name: String,
        #[serde(rename = "RandomSeed")]
        random_seed: i64,

        // Spawn location
        #[serde(rename = "SpawnX")]
        spawn_x: i32,
        #[serde(rename = "SpawnY")]
        spawn_y: i32,
        #[serde(rename = "SpawnZ")]
        spawn_z: i32,

        // World generation
        #[serde(rename = "Generator")]
        generator: i32,
        #[serde(rename = "FlatWorldLayers")]
        flat_world_layers: String,
        #[serde(rename = "spawnMobs")]
        spawn_mobs: bool,

        // Game settings
        #[serde(rename = "GameType")]
        game_type: i32,
        #[serde(rename = "Difficulty")]
        difficulty: i32,
        #[serde(rename = "ForceGameType")]
        force_game_type: bool,

        // Time
        #[serde(rename = "LastPlayed")]
        last_played: i64,
        #[serde(rename = "Time")]
        time: i64,
        #[serde(rename = "currentTick")]
        current_tick: i64,

        // Cheats and commands
        #[serde(rename = "commandsEnabled")]
        commands_enabled: bool,
        #[serde(rename = "cheatsEnabled")]
        cheats_enabled: bool,
        #[serde(rename = "commandblocksenabled")]
        command_blocks_enabled: bool,
        #[serde(rename = "commandblockoutput")]
        command_block_output: bool,

        // Multiplayer
        #[serde(rename = "MultiplayerGame")]
        multiplayer_game: bool,
        #[serde(rename = "MultiplayerGameIntent")]
        multiplayer_game_intent: bool,
        #[serde(rename = "LANBroadcast")]
        lan_broadcast: bool,
        #[serde(rename = "LANBroadcastIntent")]
        lan_broadcast_intent: bool,
        #[serde(rename = "XBLBroadcastIntent")]
        xbl_broadcast_intent: i32,
        #[serde(rename = "PlatformBroadcastIntent")]
        platform_broadcast_intent: i32,
        #[serde(rename = "Platform")]
        platform: i32,

        // Game rules
        #[serde(rename = "dodaylightcycle")]
        do_daylight_cycle: bool,
        #[serde(rename = "doweathercycle")]
        do_weather_cycle: bool,
        #[serde(rename = "domobspawning")]
        do_mob_spawning: bool,
        #[serde(rename = "domobloot")]
        do_mob_loot: bool,
        #[serde(rename = "dotiledrops")]
        do_tile_drops: bool,
        #[serde(rename = "doentitydrops")]
        do_entity_drops: bool,
        #[serde(rename = "dofiretick")]
        do_fire_tick: bool,
        #[serde(rename = "mobgriefing")]
        mob_griefing: bool,
        #[serde(rename = "naturalregeneration")]
        natural_regeneration: bool,
        #[serde(rename = "pvp")]
        pvp: bool,
        #[serde(rename = "keepinventory")]
        keep_inventory: bool,
        #[serde(rename = "sendcommandfeedback")]
        send_command_feedback: bool,
        #[serde(rename = "showcoordinates")]
        show_coordinates: bool,
        #[serde(rename = "showdeathmessages")]
        show_death_messages: bool,
        #[serde(rename = "tntexplodes")]
        tnt_explodes: bool,
        #[serde(rename = "respawnblocksexplode")]
        respawn_blocks_explode: bool,
        #[serde(rename = "projectilescanbreakblocks")]
        projectiles_can_break_blocks: bool,

        // Damage settings
        #[serde(rename = "drowningdamage")]
        drowning_damage: bool,
        #[serde(rename = "falldamage")]
        fall_damage: bool,
        #[serde(rename = "firedamage")]
        fire_damage: bool,
        #[serde(rename = "freezedamage")]
        freeze_damage: bool,

        // Weather
        #[serde(rename = "rainLevel")]
        rain_level: f32,
        #[serde(rename = "rainTime")]
        rain_time: i32,
        #[serde(rename = "lightningLevel")]
        lightning_level: f32,
        #[serde(rename = "lightningTime")]
        lightning_time: i32,

        // Misc settings
        #[serde(rename = "NetherScale")]
        nether_scale: i32,
        #[serde(rename = "spawnradius")]
        spawn_radius: i32,
        #[serde(rename = "randomtickspeed")]
        random_tick_speed: i32,
        #[serde(rename = "functioncommandlimit")]
        function_command_limit: i32,
        #[serde(rename = "maxcommandchainlength")]
        max_command_chain_length: i32,
        #[serde(rename = "serverChunkTickRange")]
        server_chunk_tick_range: i32,
        #[serde(rename = "limitedWorldDepth")]
        limited_world_depth: i32,
        #[serde(rename = "limitedWorldWidth")]
        limited_world_width: i32,
        #[serde(rename = "LimitedWorldOriginX")]
        limited_world_origin_x: i32,
        #[serde(rename = "LimitedWorldOriginY")]
        limited_world_origin_y: i32,
        #[serde(rename = "LimitedWorldOriginZ")]
        limited_world_origin_z: i32,
        #[serde(rename = "worldStartCount")]
        world_start_count: i64,

        // Boolean flags
        #[serde(rename = "bonusChestEnabled")]
        bonus_chest_enabled: bool,
        #[serde(rename = "bonusChestSpawned")]
        bonus_chest_spawned: bool,
        #[serde(rename = "hasBeenLoadedInCreative")]
        has_been_loaded_in_creative: bool,
        #[serde(rename = "hasLockedBehaviorPack")]
        has_locked_behavior_pack: bool,
        #[serde(rename = "hasLockedResourcePack")]
        has_locked_resource_pack: bool,
        #[serde(rename = "immutableWorld")]
        immutable_world: bool,
        #[serde(rename = "isFromLockedTemplate")]
        is_from_locked_template: bool,
        #[serde(rename = "isFromWorldTemplate")]
        is_from_world_template: bool,
        #[serde(rename = "isSingleUseWorld")]
        is_single_use_world: bool,
        #[serde(rename = "isWorldTemplateOptionLocked")]
        is_world_template_option_locked: bool,
        #[serde(rename = "texturePacksRequired")]
        texture_packs_required: bool,
        #[serde(rename = "useMsaGamertagsOnly")]
        use_msa_gamertags_only: bool,
        #[serde(rename = "CenterMapsToOrigin")]
        center_maps_to_origin: bool,
        #[serde(rename = "ConfirmedPlatformLockedContent")]
        confirmed_platform_locked_content: bool,
        #[serde(rename = "educationFeaturesEnabled")]
        education_features_enabled: bool,
        #[serde(rename = "startWithMapEnabled")]
        start_with_map_enabled: bool,
        #[serde(rename = "requiresCopiedPackRemovalCheck")]
        requires_copied_pack_removal_check: bool,
        #[serde(rename = "SpawnV1Villagers")]
        spawn_v1_villagers: bool,
        #[serde(rename = "IsHardcore")]
        is_hardcore: bool,
        #[serde(rename = "isCreatedInEditor")]
        is_created_in_editor: bool,
        #[serde(rename = "isExportedFromEditor")]
        is_exported_from_editor: bool,
        #[serde(rename = "isRandomSeedAllowed")]
        is_random_seed_allowed: bool,
        #[serde(rename = "HasUncompleteWorldFileOnDisk")]
        has_uncomplete_world_file_on_disk: bool,
        #[serde(rename = "PlayerHasDied")]
        player_has_died: bool,
        #[serde(rename = "doinsomnia")]
        do_insomnia: bool,
        #[serde(rename = "doimmediaterespawn")]
        do_immediate_respawn: bool,
        #[serde(rename = "dolimitedcrafting")]
        do_limited_crafting: bool,
        #[serde(rename = "recipesunlock")]
        recipes_unlock: bool,
        #[serde(rename = "showtags")]
        show_tags: bool,
        #[serde(rename = "showrecipemessages")]
        show_recipe_messages: bool,
        #[serde(rename = "showbordereffect")]
        show_border_effect: bool,
        #[serde(rename = "showdaysplayed")]
        show_days_played: bool,
        #[serde(rename = "locatorbar")]
        locator_bar: bool,
        #[serde(rename = "tntexplosiondropdecay")]
        tnt_explosion_drop_decay: bool,
        #[serde(rename = "saved_with_toggled_experiments")]
        saved_with_toggled_experiments: bool,
        #[serde(rename = "experiments_ever_used")]
        experiments_ever_used: bool,

        // Editor
        #[serde(rename = "editorWorldType")]
        editor_world_type: i32,
        #[serde(rename = "eduOffer")]
        edu_offer: i32,

        // Override
        #[serde(rename = "BiomeOverride")]
        biome_override: String,
        #[serde(rename = "prid")]
        prid: String,

        // Player sleeping
        #[serde(rename = "playerssleepingpercentage")]
        players_sleeping_percentage: i32,

        // Permissions
        #[serde(rename = "permissionsLevel")]
        permissions_level: i32,
        #[serde(rename = "playerPermissionsLevel")]
        player_permissions_level: i32,

        // Daylight cycle
        #[serde(rename = "daylightCycle")]
        daylight_cycle: i32,
    }
}

