use std::collections::HashMap;
use std::fs::File;
use fastanvil::Region;
use fastnbt::{Value, LongArray};
use serde::{Deserialize, Serialize};

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

struct WorldEditor {
    region_template_path: String,
    region_dir: String,
    regions: HashMap<(i32, i32), Region<File>>,
    chunks_to_modify: HashMap<(i32, i32, i32, i32), Vec<(String, i32, i32, i32)>>,
}

impl WorldEditor {
    /// Initializes the WorldEditor with the region directory and template region path.
    fn new(region_template_path: &str, region_dir: &str) -> Self {
        Self {
            region_template_path: region_template_path.to_string(),
            region_dir: region_dir.to_string(),
            regions: HashMap::new(),
            chunks_to_modify: HashMap::new(),
        }
    }

    /// Loads or creates a region for the given region coordinates.
    fn load_region(&mut self, region_x: i32, region_z: i32) -> &mut Region<File> {
        self.regions.entry((region_x, region_z)).or_insert_with(|| {
            let out_path = format!(
                "{}/r.{}.{}.mca",
                self.region_dir, region_x, region_z
            );
            // Always start with a fresh copy of the template
            let _ = std::fs::copy(&self.region_template_path, &out_path)
                .expect("Failed to copy region template");
            let region_file = File::options().read(true).write(true).open(&out_path)
                .expect(&format!("Failed to open region file {}", out_path));
            Region::from_stream(region_file).expect("Failed to load region")
        })
    }

    /// Sets a block of the specified type at the given coordinates.
    fn set_block(&mut self, block_type: &str, x: i32, y: i32, z: i32) {
        let chunk_x = x >> 4;
        let chunk_z = z >> 4;
        let region_x = chunk_x >> 5;
        let region_z = chunk_z >> 5;

        let chunk_x_within_region = (chunk_x & 31) as usize;
        let chunk_z_within_region = (chunk_z & 31) as usize;

        self.chunks_to_modify
            .entry((region_x, region_z, chunk_x_within_region as i32, chunk_z_within_region as i32))
            .or_default()
            .push((block_type.to_string(), x, y, z));
    }

    /// Fills a cuboid area with the specified block between two coordinates.
    ///
    /// # Arguments
    ///
    /// * `block_type` - The type of the block to be placed, specified as a string (e.g., "minecraft:bedrock").
    /// * `x1`, `y1`, `z1` - The first set of coordinates defining one corner of the cuboid.
    /// * `x2`, `y2`, `z2` - The second set of coordinates defining the opposite corner of the cuboid.
    fn fill_blocks(&mut self, block_type: &str, x1: i32, y1: i32, z1: i32, x2: i32, y2: i32, z2: i32) {
        let (min_x, max_x) = if x1 < x2 { (x1, x2) } else { (x2, x1) };
        let (min_y, max_y) = if y1 < y2 { (y1, y2) } else { (y2, y1) };
        let (min_z, max_z) = if z1 < z2 { (z1, z2) } else { (z2, z1) };

        for x in min_x..=max_x {
            for y in min_y..=max_y {
                for z in min_z..=max_z {
                    self.set_block(block_type, x, y, z);
                }
            }
        }
    }

    /// Saves all changes made to the world by writing modified chunks to the appropriate region files.
    fn save(&mut self) {
        let chunks_to_process: Vec<_> = self.chunks_to_modify.drain().collect();

        for ((region_x, region_z, chunk_x, chunk_z), block_list) in chunks_to_process {
            // Ensure the region is loaded (insert if missing)
            let region = self.load_region(region_x, region_z);
            
            if let Ok(Some(data)) = region.read_chunk(chunk_x as usize, chunk_z as usize) {
                let mut chunk: Chunk = fastnbt::from_bytes(&data).unwrap();

                for (block_type, x, y, z) in block_list {
                    set_block_in_chunk(&mut chunk, &block_type, x, y, z);
                }

                let ser = fastnbt::to_bytes(&chunk).unwrap();
                region.write_chunk(chunk_x as usize, chunk_z as usize, &ser).unwrap();
            }
        }
    }
}

fn main() {
    let region_path = "r.0.0.mca.template";
    let region_dir: &str = "%MC_WORLD_PATH_HERE%/region";
    let ground_level: i32 = -61;

    // Initialize the WorldEditor
    let mut editor = WorldEditor::new(region_template_path, region_dir);

    // Set multiple blocks across different regions
    editor.set_block("minecraft:bedrock", 1, -60, 1);
    editor.set_block("minecraft:bedrock", 2, -60, 2);
    editor.set_block("minecraft:bedrock", 0, -60, 0);
    editor.set_block("minecraft:bedrock", -1, -60, -1);
    editor.set_block("minecraft:bedrock", -2, -60, -2);


    editor.fill_blocks("minecraft:bedrock", -15, ground_level + 1, -9, 16, ground_level + 50, 123);

    // Save the changes to the world
    editor.save();
}

/// Modifies a chunk to set a block of the given type at the specified coordinates.
///
/// This function modifies the block state at the specified position within the given chunk.
///
/// # Arguments
/// 
/// * `chunk` - Mutable reference to the chunk being modified.
/// * `block_type` - The type of the block to be placed, specified as a string (e.g., "minecraft:bedrock").
/// * `x`, `y`, `z` - The global coordinates where the block should be set.
fn set_block_in_chunk(chunk: &mut Chunk, block_type: &str, x: i32, y: i32, z: i32) {
    let local_x = (x & 15) as usize;
    let local_y = y as usize;
    let local_z = (z & 15) as usize;

    for section in chunk.sections.iter_mut() {
        if let Some(Value::Byte(y_byte)) = section.other.get("Y") {
            if *y_byte == (local_y >> 4) as i8 {
                let palette = &mut section.block_states.palette;
                let block_index = (local_y % 16 * 256 + local_z * 16 + local_x) as usize;

                let palette_index = if let Some(index) = palette.iter().position(|item| item.name == block_type) {
                    index as u32
                } else {
                    palette.push(PaletteItem {
                        name: block_type.to_string(),
                        properties: None,
                    });
                    (palette.len() - 1) as u32
                };

                if let Some(Value::LongArray(ref mut data)) = section.block_states.other.get_mut("data") {
                    set_block_in_section(data, block_index, palette_index);
                } else {
                    let mut new_data = vec![0; 256];
                    set_block_in_section(&mut new_data, block_index, palette_index);
                    section.block_states.other.insert("data".to_string(), Value::LongArray(LongArray::new(new_data)));
                }

                println!("Set block {} at coordinates ({}, {}, {})", block_type, x, y, z);
                break;
            }
        }
    }
}

/// Modifies the block state data for a given block index within a section.
///
/// This function updates the block data array to set the block at the specified index
/// to point to the correct palette entry.
///
/// # Arguments
///
/// * `data` - The block state data array to modify.
/// * `block_index` - The index of the block to modify within the section.
/// * `palette_index` - The index in the palette corresponding to the block type.
fn set_block_in_section(data: &mut [i64], block_index: usize, palette_index: u32) {
    let bits_per_block = 4.max(64 / data.len() as u32); // Calculate bits per block, minimum of 4
    let mask = (1 << bits_per_block) - 1;
    let long_index = block_index * bits_per_block as usize / 64;
    let bit_index = (block_index * bits_per_block as usize) % 64;
    data[long_index] &= !(mask << bit_index);
    data[long_index] |= (palette_index as i64) << bit_index;
}
