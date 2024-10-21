use crate::args::Args;
use crate::block_definitions::*;
use colored::Colorize;
use fastanvil::Region;
use fastnbt::{ByteArray, LongArray, Value};
use indicatif::{ProgressBar, ProgressStyle};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Chunk {
    sections: Vec<Section>,
    x_pos: i32,
    z_pos: i32,
    #[serde(flatten)]
    other: HashMap<String, Value>,
}

#[derive(Serialize, Deserialize)]
struct Section {
    block_states: Blockstates,
    #[serde(flatten)]
    other: HashMap<String, Value>,
}

#[derive(Serialize, Deserialize)]
struct Blockstates {
    palette: Vec<PaletteItem>,
    #[serde(flatten)]
    other: HashMap<String, Value>,
}

#[derive(Serialize, Deserialize)]
struct PaletteItem {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Properties")]
    properties: Option<Value>,
}

#[derive(Default)]
struct ChunkToModify {
    blocks: HashMap<(i32, i32, i32), Block>,
}

impl ChunkToModify {
    fn get_block(&self, x: i32, y: i32, z: i32) -> Option<&Block> {
        self.blocks.get(&(x, y, z))
    }

    fn set_block(&mut self, x: i32, y: i32, z: i32, block: Block) {
        self.blocks.insert((x, y, z), block);
    }
}

#[derive(Default)]
struct RegionToModify {
    chunks: HashMap<(i32, i32), ChunkToModify>,
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
    regions: HashMap<(i32, i32), RegionToModify>,
}

impl WorldToModify {
    fn get_or_create_region(&mut self, x: i32, z: i32) -> &mut RegionToModify {
        self.regions.entry((x, z)).or_default()
    }

    fn get_region(&self, x: i32, z: i32) -> Option<&RegionToModify> {
        self.regions.get(&(x, z))
    }

    fn get_block(&self, x: i32, y: i32, z: i32) -> Option<&Block> {
        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x = chunk_x >> 5;
        let region_z = chunk_z >> 5;

        let region = self.get_region(region_x, region_z)?;
        let chunk = region.get_chunk(chunk_x & 31, chunk_z & 31)?;

        chunk.get_block(x & 15, y, z & 15)
    }

    fn set_block(&mut self, x: i32, y: i32, z: i32, block: Block) {
        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x = chunk_x >> 5;
        let region_z = chunk_z >> 5;

        let region = self.get_or_create_region(region_x, region_z);
        let chunk = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

        chunk.set_block(x & 15, y, z & 15, block);
    }
}

pub struct WorldEditor<'a> {
    region_template_path: String,
    region_dir: String,
    world: WorldToModify,
    scale_factor_x: f64,
    scale_factor_z: f64,
    args: &'a Args,
}

impl<'a> WorldEditor<'a> {
    /// Initializes the WorldEditor with the region directory and template region path.
    pub fn new(
        region_template_path: &str,
        region_dir: &str,
        scale_factor_x: f64,
        scale_factor_z: f64,
        args: &'a Args,
    ) -> Self {
        Self {
            region_template_path: region_template_path.to_string(),
            region_dir: region_dir.to_string(),
            world: WorldToModify::default(),
            scale_factor_x,
            scale_factor_z,
            args,
        }
    }

    /// Creates a region for the given region coordinates.
    fn create_region(&self, region_x: i32, region_z: i32) -> Region<File> {
        let out_path: String = format!("{}/r.{}.{}.mca", self.region_dir, region_x, region_z);
        std::fs::copy(&self.region_template_path, &out_path)
            .expect("Failed to copy region template");
        let region_file = File::options()
            .read(true)
            .write(true)
            .open(&out_path)
            .expect("Failed to open region file");

        Region::from_stream(region_file).expect("Failed to load region")
    }

    pub fn get_max_coords(&self) -> (i32, i32) {
        (self.scale_factor_x as i32, self.scale_factor_x as i32)
    }

    /// Sets a block of the specified type at the given coordinates.
    pub fn set_block(
        &mut self,
        block: &Lazy<Block>,
        x: i32,
        y: i32,
        z: i32,
        override_whitelist: Option<&[&'static Lazy<Block>]>,
        override_blacklist: Option<&[&'static Lazy<Block>]>,
    ) {
        // Check if coordinates are within bounds
        if x < 0 || x > self.scale_factor_x as i32 || z < 0 || z > self.scale_factor_z as i32 {
            return;
        }

        let should_insert = if let Some(existing_block) = self.world.get_block(x, y, z) {
            // Check against whitelist and blacklist
            if let Some(whitelist) = override_whitelist {
                whitelist
                    .iter()
                    .any(|&whitelisted_block| whitelisted_block.name == existing_block.name)
            } else if let Some(blacklist) = override_blacklist {
                !blacklist
                    .iter()
                    .any(|&blacklisted_block| blacklisted_block.name == existing_block.name)
            } else {
                false
            }
        } else {
            true
        };

        if should_insert {
            self.world.set_block(x, y, z, (*block).clone());
        }
    }

    /// Fills a cuboid area with the specified block between two coordinates.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_blocks(
        &mut self,
        block: &'static Lazy<Block>,
        x1: i32,
        y1: i32,
        z1: i32,
        x2: i32,
        y2: i32,
        z2: i32,
        override_whitelist: Option<&[&'static Lazy<Block>]>,
        override_blacklist: Option<&[&'static Lazy<Block>]>,
    ) {
        let (min_x, max_x) = if x1 < x2 { (x1, x2) } else { (x2, x1) };
        let (min_y, max_y) = if y1 < y2 { (y1, y2) } else { (y2, y1) };
        let (min_z, max_z) = if z1 < z2 { (z1, z2) } else { (z2, z1) };

        for x in min_x..=max_x {
            for y in min_y..=max_y {
                for z in min_z..=max_z {
                    self.set_block(block, x, y, z, override_whitelist, override_blacklist);
                }
            }
        }
    }

    /// Checks for a block at the given coordinates.
    pub fn check_for_block(
        &self,
        x: i32,
        y: i32,
        z: i32,
        whitelist: Option<&[&'static Lazy<Block>]>,
        blacklist: Option<&[&'static Lazy<Block>]>,
    ) -> bool {
        // Retrieve the chunk modification map
        if let Some(existing_block) = self.world.get_block(x, y, z) {
            // Check against whitelist and blacklist
            if let Some(whitelist) = whitelist {
                if whitelist
                    .iter()
                    .any(|&whitelisted_block| whitelisted_block.name == existing_block.name)
                {
                    return true; // Block is in whitelist
                }
            }
            if let Some(blacklist) = blacklist {
                if blacklist
                    .iter()
                    .any(|&blacklisted_block| blacklisted_block.name == existing_block.name)
                {
                    return true; // Block is in blacklist
                }
            }
        }

        false
    }

    /// Saves all changes made to the world by writing modified chunks to the appropriate region files.
    pub fn save(&mut self) {
        println!("{} Saving world...", "[5/5]".bold());

        let _debug: bool = self.args.debug;
        let total_regions: u64 = self.world.regions.len() as u64;

        let save_pb: ProgressBar = ProgressBar::new(total_regions);
        save_pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] [{bar:45}] {pos}/{len} regions ({eta})",
                )
                .unwrap()
                .progress_chars("█▓░"),
        );

        for ((region_x, region_z), region_to_modify) in &self.world.regions {
            let mut region = self.create_region(*region_x, *region_z);

            for chunk_x in 0..32 {
                for chunk_z in 0..32 {
                    let data = region
                        .read_chunk(chunk_x as usize, chunk_z as usize)
                        .unwrap()
                        .unwrap();

                    let mut chunk: Chunk = fastnbt::from_bytes(&data).unwrap();

                    if let Some(block_list) = region_to_modify.get_chunk(chunk_x, chunk_z) {
                        for ((x, y, z), block) in &block_list.blocks {
                            set_block_in_chunk(&mut chunk, block.clone(), *x, *y, *z);
                        }
                    }

                    chunk.x_pos = chunk_x + region_x * 32;
                    chunk.z_pos = chunk_z + region_z * 32;

                    let ser: Vec<u8> = fastnbt::to_bytes(&chunk).unwrap();

                    // Write chunk data back to the correct location, ensuring correct chunk coordinates
                    let expected_chunk_location: (usize, usize) =
                        ((chunk_x as usize) & 31, (chunk_z as usize) & 31);
                    region
                        .write_chunk(expected_chunk_location.0, expected_chunk_location.1, &ser)
                        .unwrap();
                }
            }

            save_pb.inc(1);
        }

        save_pb.finish();
    }
}

fn bits_per_block(palette_size: u32) -> u32 {
    (palette_size as f32).log2().ceil().clamp(4.0, 8.0) as u32
}

fn set_block_in_chunk(chunk: &mut Chunk, block: Block, x: i32, y: i32, z: i32) {
    let local_x = x as usize;
    let local_y = y as usize;
    let local_z = z as usize;

    for section in chunk.sections.iter_mut() {
        if let Some(Value::Byte(y_byte)) = section.other.get("Y") {
            if *y_byte == (local_y >> 4) as i8 {
                let palette: &mut Vec<PaletteItem> = &mut section.block_states.palette;
                let block_index: usize = local_y % 16 * 256 + local_z * 16 + local_x;

                // Add SkyLight with 2048 bytes of value 0xFF
                let skylight_data = vec![0xFFu8 as i8; 2048];
                section.other.insert(
                    "SkyLight".to_string(),
                    Value::ByteArray(ByteArray::new(skylight_data)),
                );

                // Check if the block is already in the palette with matching properties
                let mut palette_index: Option<usize> =
                    palette.iter().position(|item: &PaletteItem| {
                        item.name == block.name && item.properties == block.properties
                    });

                // If the block is not in the palette and adding it would exceed a reasonable size, skip or replace
                if palette_index.is_none() {
                    if palette.len() >= 16 {
                        palette_index = Some(0);
                    } else {
                        // Add the new block type to the palette with its properties
                        palette.push(PaletteItem {
                            name: block.name.clone(),
                            properties: block.properties.clone(),
                        });
                        palette_index = Some(palette.len() - 1);
                    }
                }

                // Unwrap because we are sure palette_index is Some after this point
                let palette_index: u32 = palette_index.unwrap() as u32;

                let bits_per_block: u32 = bits_per_block(palette.len() as u32);
                if let Some(Value::LongArray(ref mut data)) =
                    section.block_states.other.get_mut("data")
                {
                    // Convert LongArray to Vec<i64>
                    let mut vec_data: Vec<i64> = data.as_mut().to_vec();
                    set_block_in_section(&mut vec_data, block_index, palette_index, bits_per_block);
                    // Update LongArray with modified Vec<i64>
                    *data = LongArray::new(vec_data);
                } else {
                    // Properly initialize new data array with correct length
                    let required_longs: usize =
                        ((4096 + (64 / bits_per_block) - 1) / (64 / bits_per_block)) as usize;
                    let mut new_data: Vec<i64> = vec![0i64; required_longs];
                    set_block_in_section(&mut new_data, block_index, palette_index, bits_per_block);
                    section.block_states.other.insert(
                        "data".to_string(),
                        Value::LongArray(LongArray::new(new_data)),
                    );
                }

                break;
            }
        }
    }
}

fn set_block_in_section(
    data: &mut [i64],
    block_index: usize,
    palette_index: u32,
    bits_per_block: u32,
) {
    let blocks_per_long: u32 = 64 / bits_per_block;
    let required_longs: usize = ((4096 + blocks_per_long - 1) / blocks_per_long) as usize;

    // Ensure data vector is large enough
    assert!(data.len() >= required_longs, "Data slice is too small");

    let mask: u64 = (1u64 << bits_per_block) - 1;
    let long_index: usize = block_index / blocks_per_long as usize;
    let start_bit: usize = (block_index % blocks_per_long as usize) * bits_per_block as usize;

    let current_value: u64 = data[long_index] as u64;
    let new_value: u64 =
        (current_value & !(mask << start_bit)) | ((palette_index as u64 & mask) << start_bit);

    // Update data
    data[long_index] = new_value as i64;

    // Handle cases where bits spill over into the next long
    if start_bit + bits_per_block as usize > 64 {
        let overflow_bits: usize = (start_bit + bits_per_block as usize) - 64;
        let next_long_index: usize = long_index + 1;

        if next_long_index < data.len() {
            let next_value: u64 = data[next_long_index] as u64;
            let new_next_value: u64 = (next_value & !(mask >> overflow_bits))
                | ((palette_index as u64 & mask) >> overflow_bits);
            data[next_long_index] = new_next_value as i64;
        } else {
            panic!("Data slice is too small even after resizing. This should never happen.");
        }
    }
}
