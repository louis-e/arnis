use crate::block_definitions::*;
use fastanvil::Region;
use fastnbt::{Value, LongArray};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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

pub struct WorldEditor {
    region_template_path: String,
    region_dir: String,
    regions: HashMap<(i32, i32), Region<File>>,
    chunks_to_modify: HashMap<(i32, i32, i32, i32), Vec<(Block, i32, i32, i32)>>,
    modified_positions: HashSet<(i32, i32, i32)>,  // New HashSet to track modified positions
}

impl WorldEditor {
    /// Initializes the WorldEditor with the region directory and template region path.
    pub fn new(region_template_path: &str, region_dir: &str) -> Self {
        Self {
            region_template_path: region_template_path.to_string(),
            region_dir: region_dir.to_string(),
            regions: HashMap::new(),
            chunks_to_modify: HashMap::new(),
            modified_positions: HashSet::new(),  // Initialize the HashSet
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
    pub fn set_block(&mut self, block: &'static Lazy<Block>, x: i32, y: i32, z: i32) {
        let position = (x, y, z);

        // Check if the position has already been modified
        /*if self.modified_positions.contains(&position) {
            if (x >= 128 && x <= 143 && z >= 32 && z <= 47) {
                println!("skip {} {} {}", x, y, z);
            }
            return; // Block at this position has already been set, so do nothing
        }*/

        // Track the position as modified
        self.modified_positions.insert(position);

        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;

        let chunk_x_within_region: usize = (chunk_x & 31) as usize;
        let chunk_z_within_region: usize = (chunk_z & 31) as usize;

        self.chunks_to_modify
            .entry((region_x, region_z, chunk_x_within_region as i32, chunk_z_within_region as i32))
            .or_default()
            .push(((*block).clone(), x, y, z));
    }

    /// Fills a cuboid area with the specified block between two coordinates.
    pub fn fill_blocks(&mut self, block: &'static Lazy<Block>, x1: i32, y1: i32, z1: i32, x2: i32, y2: i32, z2: i32) {
        let (min_x, max_x) = if x1 < x2 { (x1, x2) } else { (x2, x1) };
        let (min_y, max_y) = if y1 < y2 { (y1, y2) } else { (y2, y1) };
        let (min_z, max_z) = if z1 < z2 { (z1, z2) } else { (z2, z1) };

        for x in min_x..=max_x {
            for y in min_y..=max_y {
                for z in min_z..=max_z {
                    self.set_block(block, x, y, z);
                }
            }
        }
    }

    /// Saves all changes made to the world by writing modified chunks to the appropriate region files.
    pub fn save(&mut self) {
        println!("Saving world...");
        let chunks_to_process: Vec<_> = self.chunks_to_modify.drain().collect();

        for ((region_x, region_z, chunk_x, chunk_z), block_list) in chunks_to_process {
            let region: &mut Region<File> = self.load_region(region_x, region_z);
            
            if let Ok(Some(data)) = region.read_chunk(chunk_x as usize, chunk_z as usize) {
                let mut chunk: Chunk = fastnbt::from_bytes(&data).unwrap(); // TODO: called `Result::unwrap()` on an `Err` value: Error("missing field `block_states`")

                for (block, x, y, z) in block_list {
                    set_block_in_chunk(&mut chunk, block, x, y, z);
                }

                let ser: Vec<u8> = fastnbt::to_bytes(&chunk).unwrap();
                region.write_chunk(chunk_x as usize, chunk_z as usize, &ser).unwrap(); // NOTE chunk 8 2 is indeed being written
            }
        }
    }
}

/*fn set_block_in_chunk(chunk: &mut Chunk, block: Block, x: i32, y: i32, z: i32) {
    let local_x = (x & 15) as usize;
    let local_y = y as usize;
    let local_z = (z & 15) as usize;

    /*if (x >= 128 && x <= 143 && z >= 32 && z <= 47)
    {
        println!("{} {} {} {}", block.name, x, y, z);
    }*/

    for section in chunk.sections.iter_mut() {
        if let Some(Value::Byte(y_byte)) = section.other.get("Y") {
            if *y_byte == (local_y >> 4) as i8 {
                let palette = &mut section.block_states.palette;
                let block_index = (local_y % 16 * 256 + local_z * 16 + local_x) as usize;

                let palette_index = if let Some(index) = palette.iter().position(|item| item.name == block.name) {
                    index as u32
                } else {
                    palette.push(PaletteItem {
                        name: block.name.clone(),
                        properties: block.properties.clone(),
                    });
                    (palette.len() - 1) as u32
                };

                if let Some(Value::LongArray(ref mut data)) = section.block_states.other.get_mut("data") {
                    /*if (x >= 128 && x <= 143 && z >= 32 && z <= 47) {
                        println!("T1 {} {}", block_index, palette_index);
                    } else {
                        println!("D1 {} {}", block_index, palette_index);
                    }*/
                    set_block_in_section(data, block_index, palette_index);
                } else {
                    /*if (x >= 128 && x <= 143 && z >= 32 && z <= 47) {
                        println!("T2 {} {}", block_index, palette_index);
                    } else {
                        println!("D2 {} {}", block_index, palette_index);
                    }*/
                    let mut new_data = vec![0; 256];
                    set_block_in_section(&mut new_data, block_index, palette_index);
                    section.block_states.other.insert("data".to_string(), Value::LongArray(LongArray::new(new_data)));
                }

                break;
            }
        }
    }
}*/
fn bits_per_block(palette_size: u32) -> u32 {
    (palette_size as f32).log2().ceil().max(4.0).min(8.0) as u32
}
fn set_block_in_chunk(chunk: &mut Chunk, block: Block, x: i32, y: i32, z: i32) {
    let local_x: usize = (x & 15) as usize;
    let local_y: usize = y as usize;
    let local_z: usize = (z & 15) as usize;

    for section in chunk.sections.iter_mut() {
        if let Some(Value::Byte(y_byte)) = section.other.get("Y") {
            if *y_byte == (local_y >> 4) as i8 {
                let palette: &mut Vec<PaletteItem> = &mut section.block_states.palette;
                let block_index: usize = (local_y % 16 * 256 + local_z * 16 + local_x) as usize;

                let palette_index = if let Some(index) = palette.iter().position(|item: &PaletteItem| item.name == block.name) {
                    index as u32
                } else {
                    palette.push(PaletteItem {
                        name: block.name.clone(),
                        properties: block.properties.clone(),
                    });
                    (palette.len() - 1) as u32
                };

                let bits_per_block = bits_per_block(palette.len() as u32);
                if let Some(Value::LongArray(ref mut data)) = section.block_states.other.get_mut("data") {
                    // Convert LongArray to Vec<i64>
                    let mut vec_data: Vec<i64> = data.as_mut().to_vec();
                    ensure_data_array_size(&mut vec_data, bits_per_block);
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
fn ensure_data_array_size(data: &mut Vec<i64>, bits_per_block: u32) {
    let blocks_per_long: u32 = 64 / bits_per_block;
    let required_longs: usize = ((4096 + blocks_per_long - 1) / blocks_per_long) as usize;

    if data.len() != required_longs {
        println!("Resizing data from {} to {}", data.len(), required_longs);
        data.resize(required_longs, 0);
    }
    assert_eq!(data.len(), required_longs, "Data length mismatch after resizing.");
}

fn set_block_in_section(data: &mut Vec<i64>, block_index: usize, palette_index: u32, bits_per_block: u32) {
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
            // If we reach here, something went wrong with our data resizing logic
            panic!("Data slice is too small even after resizing. This should never happen.");
        }
    }
}