use colored::Colorize;
use crate::args::Args;
use crate::block_definitions::*;
use fastanvil::Region;
use fastnbt::{Value, LongArray};
use indicatif::{ProgressBar, ProgressStyle};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;

#[derive(Serialize, Deserialize)]
struct Chunk {
    sections: Vec<Section>,
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

pub struct WorldEditor<'a> {
    region_template_path: String,
    region_dir: String,
    regions: HashMap<(i32, i32), Region<File>>,
    chunks_to_modify: HashMap<(i32, i32, i32, i32), HashMap<(i32, i32, i32), Block>>,
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
            regions: HashMap::new(),
            chunks_to_modify: HashMap::new(),
            scale_factor_x,
            scale_factor_z,
            args,
        }
    }

    /// Loads or creates a region for the given region coordinates.
    fn load_region(&mut self, region_x: i32, region_z: i32) -> &mut Region<File> {
        self.regions.entry((region_x, region_z)).or_insert_with(|| {
            let out_path: String = format!("{}/r.{}.{}.mca", self.region_dir, region_x, region_z);
            std::fs::copy(&self.region_template_path, &out_path)
                .expect("Failed to copy region template");
            let region_file = File::options().read(true).write(true).open(&out_path)
                .expect("Failed to open region file");
            Region::from_stream(region_file).expect("Failed to load region")
        })
    }

    /// Sets a block of the specified type at the given coordinates.
    pub fn set_block(
        &mut self,
        block: &'static Lazy<Block>,
        x: i32,
        y: i32,
        z: i32,
        override_whitelist: Option<&[&'static Lazy<Block>]>,
        override_blacklist: Option<&[&'static Lazy<Block>]>,
    ) {
        let position: (i32, i32, i32) = (x, y, z);

        // Check if coordinates are within bounds
        if x < 0 || x > self.scale_factor_x as i32 || z < 0 || z > self.scale_factor_z as i32 {
            return;
        }

        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;
    
        let chunk_x_within_region: i32 = chunk_x & 31;
        let chunk_z_within_region: i32 = chunk_z & 31;
    
        let chunk_key: (i32, i32, i32, i32) = (region_x, region_z, chunk_x_within_region, chunk_z_within_region);
        let chunk_blocks: &mut HashMap<(i32, i32, i32), Block> = self.chunks_to_modify.entry(chunk_key).or_default();
    
        if let Some(existing_block) = chunk_blocks.get(&position) {
            // Check against whitelist and blacklist
            if let Some(whitelist) = override_whitelist {
                if whitelist.iter().any(|&whitelisted_block| whitelisted_block.name == existing_block.name) {
                    chunk_blocks.insert(position, (*block).clone());
                }
            } else if let Some(blacklist) = override_blacklist {
                if !blacklist.iter().any(|&blacklisted_block| blacklisted_block.name == existing_block.name) {
                    chunk_blocks.insert(position, (*block).clone());
                }
            }
        } else {
            chunk_blocks.insert(position, (*block).clone());
        }
    }    

    /// Fills a cuboid area with the specified block between two coordinates.
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

    /// Saves all changes made to the world by writing modified chunks to the appropriate region files.
    pub fn save(&mut self) {
        println!("{} {}", "[5/5]".bold(), "Saving world...");
    
        let debug: bool = self.args.debug;
        let chunks_to_process: Vec<_> = self.chunks_to_modify.drain().collect();
        let total_chunks: u64 = chunks_to_process.len() as u64;
    
        let save_pb: ProgressBar = ProgressBar::new(total_chunks);
        save_pb.set_style(ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:45}] {pos}/{len} chunks ({eta})")
            .unwrap()
            .progress_chars("█▓░"));
    
        for ((region_x, region_z, chunk_x, chunk_z), block_list) in chunks_to_process {
            let region: &mut Region<File> = self.load_region(region_x, region_z);
            
            if let Ok(Some(data)) = region.read_chunk(chunk_x as usize, chunk_z as usize) {
                let mut chunk: Chunk = fastnbt::from_bytes(&data).unwrap();
    
                for ((x, y, z), block) in block_list {
                    set_block_in_chunk(&mut chunk, block, x, y, z, region_x, region_z, chunk_x, chunk_z, debug);
                }
    
                let ser: Vec<u8> = fastnbt::to_bytes(&chunk).unwrap();
                
                // Write chunk data back to the correct location, ensuring correct chunk coordinates
                let expected_chunk_location: (usize, usize) = ((chunk_x as usize) & 31, (chunk_z as usize) & 31);
                region.write_chunk(expected_chunk_location.0, expected_chunk_location.1, &ser).unwrap();
            }
    
            save_pb.inc(1);
        }
        
        save_pb.finish();
    }
}

fn bits_per_block(palette_size: u32) -> u32 {
    (palette_size as f32).log2().ceil().max(4.0).min(8.0) as u32
}

fn set_block_in_chunk(
    chunk: &mut Chunk, 
    block: Block, 
    x: i32, 
    y: i32, 
    z: i32, 
    region_x: i32, 
    region_z: i32, 
    chunk_x: i32, 
    chunk_z: i32, 
    debug: bool
) {
    let local_x: usize = (x & 15) as usize;
    let local_y: usize = y as usize;
    let local_z: usize = (z & 15) as usize;

    for section in chunk.sections.iter_mut() {
        if let Some(Value::Byte(y_byte)) = section.other.get("Y") {
            if *y_byte == (local_y >> 4) as i8 {
                let palette: &mut Vec<PaletteItem> = &mut section.block_states.palette;
                let block_index: usize = (local_y % 16 * 256 + local_z * 16 + local_x) as usize;

                // Check if the block is already in the palette
                let mut palette_index: Option<usize> = palette.iter().position(|item: &PaletteItem| item.name == block.name);

                // If the block is not in the palette and adding it would exceed a reasonable size, skip or replace
                // This workaround prevents this major issue: https://github.com/owengage/fastnbt/issues/120
                if palette_index.is_none() {
                    if palette.len() >= 16 {
                        if debug {
                            println!("Skipping block placement to avoid excessive palette size in region ({}, {}), chunk ({}, {})", region_x, region_z, chunk_x, chunk_z);
                        }
                        palette_index = Some(0);
                    } else {
                        // Otherwise, add the new block type to the palette
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
                if let Some(Value::LongArray(ref mut data)) = section.block_states.other.get_mut("data") {
                    // Convert LongArray to Vec<i64>
                    let mut vec_data: Vec<i64> = data.as_mut().to_vec();
                    ensure_data_array_size(&mut vec_data, bits_per_block, region_x, region_z, chunk_x, chunk_z);
                    set_block_in_section(&mut vec_data, block_index, palette_index, bits_per_block);
                    // Update LongArray with modified Vec<i64>
                    *data = LongArray::new(vec_data);
                } else {
                    // Properly initialize new data array with correct length
                    let required_longs: usize = ((4096 + (64 / bits_per_block) - 1) / (64 / bits_per_block)) as usize;
                    let mut new_data: Vec<i64> = vec![0i64; required_longs];
                    set_block_in_section(&mut new_data, block_index, palette_index, bits_per_block);
                    section.block_states.other.insert("data".to_string(), Value::LongArray(LongArray::new(new_data)));
                }

                break;
            }
        }
    }
}


/// Ensure data array is correctly sized based on bits per block
fn ensure_data_array_size(
    data: &mut Vec<i64>,
    bits_per_block: u32,
    region_x: i32,
    region_z: i32,
    chunk_x: i32,
    chunk_z: i32
) {
    let blocks_per_long: u32 = 64 / bits_per_block;
    let required_longs: usize = ((4096 + blocks_per_long - 1) / blocks_per_long) as usize;

    if data.len() != required_longs {
        println!(
            "Resizing data from {} to {} in region ({}, {}), chunk ({}, {})",
            data.len(),
            required_longs,
            region_x,
            region_z,
            chunk_x,
            chunk_z
        );
        data.resize(required_longs, 0);
    }
    assert_eq!(data.len(), required_longs, "Data length mismatch after resizing.");
}

fn set_block_in_section(
    data: &mut Vec<i64>,
    block_index: usize,
    palette_index: u32,
    bits_per_block: u32
) {
    let blocks_per_long: u32 = 64 / bits_per_block;
    let required_longs: usize = ((4096 + blocks_per_long - 1) / blocks_per_long) as usize;

    // Ensure data vector is large enough
    assert!(data.len() >= required_longs, "Data slice is too small");

    let mask: u64 = (1u64 << bits_per_block) - 1;
    let long_index: usize = block_index / blocks_per_long as usize;
    let start_bit: usize = (block_index % blocks_per_long as usize) * bits_per_block as usize;

    let current_value: u64 = data[long_index] as u64;
    let new_value: u64 = (current_value & !(mask << start_bit)) | ((palette_index as u64 & mask) << start_bit);

    // Update data
    data[long_index] = new_value as i64;

    // Handle cases where bits spill over into the next long
    if start_bit + bits_per_block as usize > 64 {
        let overflow_bits: usize = (start_bit + bits_per_block as usize) - 64;
        let next_long_index: usize = long_index + 1;

        if next_long_index < data.len() {
            let next_value: u64 = data[next_long_index] as u64;
            let new_next_value: u64 = (next_value & !(mask >> overflow_bits)) | ((palette_index as u64 & mask) >> overflow_bits);
            data[next_long_index] = new_next_value as i64;
        } else {
            panic!("Data slice is too small even after resizing. This should never happen.");
        }
    }
}
