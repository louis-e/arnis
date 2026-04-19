//! Java Edition Anvil format world saving.
//!
//! This module handles saving worlds in the Java Edition Anvil (.mca) format.

use super::common::{Chunk, ChunkToModify, Section};
use super::WorldEditor;
use crate::block_definitions::GRASS_BLOCK;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use fastanvil::Region;
use fastnbt::{LongArray, Value};
use fnv::FnvHashMap;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Minecraft 1.21.1 data version for chunk format identification.
const DATA_VERSION: i32 = 3955;

/// Cached base chunk sections (grass at Y=-62)
/// Computed once on first use and reused for all empty chunks
static BASE_CHUNK_SECTIONS: OnceLock<Vec<Section>> = OnceLock::new();

/// Get or create the cached base chunk sections
fn get_base_chunk_sections() -> &'static [Section] {
    BASE_CHUNK_SECTIONS.get_or_init(|| {
        let mut chunk = ChunkToModify::default();
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, -62, z, GRASS_BLOCK);
            }
        }
        chunk.sections().collect()
    })
}

#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};

impl<'a> WorldEditor<'a> {
    /// Creates a region file for the given region coordinates.
    pub(super) fn create_region(
        &self,
        region_x: i32,
        region_z: i32,
    ) -> Result<Region<File>, Box<dyn std::error::Error + Send + Sync>> {
        let region_dir = self.world_dir.join("region");
        let out_path = region_dir.join(format!("r.{}.{}.mca", region_x, region_z));

        // Ensure region directory exists before creating region files
        std::fs::create_dir_all(&region_dir)?;

        const REGION_TEMPLATE: &[u8] = include_bytes!("../../assets/minecraft/region.template");

        let mut region_file: File = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&out_path)?;

        region_file.write_all(REGION_TEMPLATE)?;

        Ok(Region::from_stream(region_file)?)
    }

    /// Helper function to create a base chunk with grass blocks at Y -62
    /// Uses cached sections for efficiency - only serialization happens per chunk
    pub(super) fn create_base_chunk(
        abs_chunk_x: i32,
        abs_chunk_z: i32,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        // Use cached sections (computed once on first call)
        let sections = get_base_chunk_sections();

        // Prepare chunk data with cloned sections
        let chunk_data = Chunk {
            sections: sections.to_vec(),
            x_pos: abs_chunk_x,
            z_pos: abs_chunk_z,
            is_light_on: 0,
            other: FnvHashMap::default(),
        };

        let chunk_nbt = create_chunk_nbt(&chunk_data);

        let mut ser_buffer = Vec::with_capacity(8192);
        fastnbt::to_writer(&mut ser_buffer, &chunk_nbt)?;

        Ok(ser_buffer)
    }

    /// Saves the world in Java Edition Anvil format.
    ///
    /// Uses parallel processing with rayon for fast region saving.
    /// Returns an error if any region fails to save (e.g. disk full).
    pub(super) fn save_java(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("{} Saving world...", "[7/7]".bold());
        emit_gui_progress_update(90.0, "Saving world...");

        // Save metadata with error handling
        if let Err(e) = self.save_metadata() {
            eprintln!("Failed to save world metadata: {}", e);
            #[cfg(feature = "gui")]
            send_log(LogLevel::Warning, "Failed to save world metadata.");
            // Continue with world saving even if metadata fails
        }

        if self.world.regions.is_empty() {
            return Ok(());
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

        let regions_processed = AtomicU64::new(0);
        // AtomicBool for a lock-free fast-path stop check; the Mutex only stores the error value.
        let should_stop = std::sync::atomic::AtomicBool::new(false);
        let first_error: Mutex<Option<Box<dyn std::error::Error + Send + Sync>>> = Mutex::new(None);

        self.world
            .regions
            .par_iter()
            .for_each(|((region_x, region_z), region_to_modify)| {
                // Fast-path: bail out without locking once an error has been recorded.
                if should_stop.load(Ordering::Acquire) {
                    return;
                }

                if let Err(e) = self.save_single_region(*region_x, *region_z, region_to_modify) {
                    let mut guard = first_error.lock().unwrap_or_else(|p| p.into_inner());
                    if guard.is_none() {
                        *guard = Some(e);
                    }
                    // Signal other workers to stop without re-acquiring the mutex.
                    should_stop.store(true, Ordering::Release);
                    return;
                }

                // Update progress
                let regions_done = regions_processed.fetch_add(1, Ordering::SeqCst) + 1;

                // Update progress at regular intervals (every ~10% or at least every 10 regions)
                let update_interval = (total_regions / 10).max(1);
                if regions_done.is_multiple_of(update_interval) || regions_done == total_regions {
                    let progress = 90.0 + (regions_done as f64 / total_regions as f64) * 9.0;
                    emit_gui_progress_update(progress, "Saving world...");
                }

                save_pb.inc(1);
            });

        save_pb.finish();

        if let Some(e) = first_error.lock().unwrap_or_else(|p| p.into_inner()).take() {
            return Err(e);
        }

        Ok(())
    }

    /// Saves a single region to disk.
    ///
    /// Optimized for new world creation, writes chunks directly without reading existing data.
    /// This assumes we're creating a fresh world, not modifying an existing one.
    pub(super) fn save_single_region(
        &self,
        region_x: i32,
        region_z: i32,
        region_to_modify: &super::common::RegionToModify,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut region = self.create_region(region_x, region_z)?;
        let mut ser_buffer = Vec::with_capacity(8192);

        // First pass: write all chunks that have content
        for (&(chunk_x, chunk_z), chunk_to_modify) in &region_to_modify.chunks {
            if !chunk_to_modify.sections.is_empty() || !chunk_to_modify.other.is_empty() {
                // Create chunk directly, we're writing to a fresh region file
                // so there's no existing data to preserve
                let chunk = Chunk {
                    sections: chunk_to_modify.sections().collect(),
                    x_pos: chunk_x + (region_x * 32),
                    z_pos: chunk_z + (region_z * 32),
                    is_light_on: 0,
                    other: chunk_to_modify.other.clone(),
                };

                let chunk_nbt = create_chunk_nbt(&chunk);
                ser_buffer.clear();
                fastnbt::to_writer(&mut ser_buffer, &chunk_nbt)?;
                region.write_chunk(chunk_x as usize, chunk_z as usize, &ser_buffer)?;
            }
        }

        // Second pass: ensure all chunks exist (fill with base layer if not)
        for chunk_x in 0..32 {
            for chunk_z in 0..32 {
                let abs_chunk_x = chunk_x + (region_x * 32);
                let abs_chunk_z = chunk_z + (region_z * 32);

                // Check if chunk exists in our modifications
                let chunk_exists = region_to_modify.chunks.contains_key(&(chunk_x, chunk_z));

                // If chunk doesn't exist, create it with base layer
                if !chunk_exists {
                    let ser_buffer = Self::create_base_chunk(abs_chunk_x, abs_chunk_z)?;
                    region.write_chunk(chunk_x as usize, chunk_z as usize, &ser_buffer)?;
                }
            }
        }

        Ok(())
    }
}

/// Helper function to get entity coordinates
/// Note: Currently unused since we write directly without merging, but kept for potential future use
#[inline]
#[allow(dead_code)]
fn get_entity_coords(entity: &HashMap<String, Value>) -> Option<(i32, i32, i32)> {
    if let Some(Value::List(pos)) = entity.get("Pos") {
        if pos.len() == 3 {
            if let (Some(x), Some(y), Some(z)) = (
                value_to_i32(&pos[0]),
                value_to_i32(&pos[1]),
                value_to_i32(&pos[2]),
            ) {
                return Some((x, y, z));
            }
        }
    }

    let (Some(x), Some(y), Some(z)) = (
        entity.get("x").and_then(value_to_i32),
        entity.get("y").and_then(value_to_i32),
        entity.get("z").and_then(value_to_i32),
    ) else {
        return None;
    };

    Some((x, y, z))
}

/// Creates modern chunk NBT data (post-1.18 format, no Level wrapper).
///
/// Writes all required fields for server compatibility:
/// DataVersion, Status, yPos, Heightmaps, biomes, structures, etc.
/// Section range is determined dynamically: at minimum the vanilla range
/// (Y=-4 to Y=19), extended upward/downward to cover any sections with content.
fn create_chunk_nbt(chunk: &Chunk) -> HashMap<String, Value> {
    // Index existing sections by Y for quick lookup
    let section_map: HashMap<i8, usize> = chunk
        .sections
        .iter()
        .enumerate()
        .map(|(i, s)| (s.y, i))
        .collect();

    // Determine section range: start with vanilla range, expand to cover content
    let mut min_section_y: i8 = -4; // vanilla min (Y=-64)
    let mut max_section_y: i8 = 19; // vanilla max (Y=319)
    for &y in section_map.keys() {
        if y < min_section_y {
            min_section_y = y;
        }
        if y > max_section_y {
            max_section_y = y;
        }
    }

    // Biome palette shared by all sections (single "plains" entry, no data array needed)
    let biome_value = Value::Compound(HashMap::from([(
        "palette".to_string(),
        Value::List(vec![Value::String("minecraft:plains".to_string())]),
    )]));

    // Build all sections in the determined range
    let sections: Vec<Value> = (min_section_y..=max_section_y)
        .map(|y| {
            let mut section_nbt = if let Some(&idx) = section_map.get(&y) {
                build_section_value(&chunk.sections[idx])
            } else {
                // Empty air section
                HashMap::from([
                    ("Y".to_string(), Value::Byte(y)),
                    (
                        "block_states".to_string(),
                        Value::Compound(HashMap::from([(
                            "palette".to_string(),
                            Value::List(vec![Value::Compound(HashMap::from([(
                                "Name".to_string(),
                                Value::String("minecraft:air".to_string()),
                            )]))]),
                        )])),
                    ),
                ])
            };
            section_nbt.insert("biomes".to_string(), biome_value.clone());
            Value::Compound(section_nbt)
        })
        .collect();

    // Compute heightmaps from block data
    let total_height = ((max_section_y as i32 + 1) - min_section_y as i32) * 16;
    let heightmaps = compute_heightmaps(&chunk.sections, min_section_y, total_height);

    // PostProcessing: one empty list per section
    let post_processing: Vec<Value> = (0..sections.len()).map(|_| Value::List(vec![])).collect();

    // Build root-level chunk NBT (modern format — no Level wrapper)
    let mut root = HashMap::from([
        ("DataVersion".to_string(), Value::Int(DATA_VERSION)),
        ("xPos".to_string(), Value::Int(chunk.x_pos)),
        ("yPos".to_string(), Value::Int(min_section_y as i32)),
        ("zPos".to_string(), Value::Int(chunk.z_pos)),
        (
            "Status".to_string(),
            Value::String("minecraft:full".to_string()),
        ),
        ("isLightOn".to_string(), Value::Byte(0)),
        ("InhabitedTime".to_string(), Value::Long(0)),
        ("LastUpdate".to_string(), Value::Long(0)),
        ("sections".to_string(), Value::List(sections)),
        ("Heightmaps".to_string(), heightmaps),
        (
            "structures".to_string(),
            Value::Compound(HashMap::from([
                ("References".to_string(), Value::Compound(HashMap::new())),
                ("starts".to_string(), Value::Compound(HashMap::new())),
            ])),
        ),
        ("PostProcessing".to_string(), Value::List(post_processing)),
        ("block_ticks".to_string(), Value::List(vec![])),
        ("fluid_ticks".to_string(), Value::List(vec![])),
        ("block_entities".to_string(), Value::List(vec![])),
    ]);

    // Merge extra chunk data (block_entities, entities, etc.)
    // This overwrites the empty defaults above when actual data exists.
    for (key, value) in &chunk.other {
        root.insert(key.clone(), value.clone());
    }

    root
}

/// Build a section Value from a Section struct.
fn build_section_value(section: &Section) -> HashMap<String, Value> {
    let mut block_states = HashMap::from([(
        "palette".to_string(),
        Value::List(
            section
                .block_states
                .palette
                .iter()
                .map(|item| {
                    let mut palette_item =
                        HashMap::from([("Name".to_string(), Value::String(item.name.clone()))]);
                    if let Some(props) = &item.properties {
                        palette_item.insert("Properties".to_string(), props.clone());
                    }
                    Value::Compound(palette_item)
                })
                .collect(),
        ),
    )]);

    // Only add the `data` attribute if it's non-empty
    // to maintain compatibility with third-party tools like Dynmap
    if let Some(data) = &section.block_states.data {
        if !data.is_empty() {
            block_states.insert("data".to_string(), Value::LongArray(data.to_owned()));
        }
    }

    HashMap::from([
        ("Y".to_string(), Value::Byte(section.y)),
        ("block_states".to_string(), Value::Compound(block_states)),
    ])
}

/// Compute heightmaps from section block data.
///
/// Returns a Value::Compound with four heightmap types (MOTION_BLOCKING,
/// MOTION_BLOCKING_NO_LEAVES, OCEAN_FLOOR, WORLD_SURFACE) as packed LongArrays.
/// Bit width is determined dynamically based on total world height.
/// Value for each column = (highest_non_air_Y - min_block_y + 1), or 0 if all air.
fn compute_heightmaps(sections: &[Section], min_section_y: i8, total_height: i32) -> Value {
    // Precompute per-section metadata to avoid redundant work in the inner loop.
    enum SectionKind<'a> {
        Uniform {
            solid: bool,
        },
        NoAir,
        Mixed {
            data: &'a LongArray,
            bits: usize,
            vals_per_long: usize,
            mask: u64,
        },
    }
    struct SectionMeta<'a> {
        y: i8,
        palette: &'a [super::common::PaletteItem],
        kind: SectionKind<'a>,
    }

    let mut metas: Vec<SectionMeta> = sections
        .iter()
        .map(|s| {
            let palette = &s.block_states.palette;
            let kind = if palette.len() == 1 {
                SectionKind::Uniform {
                    solid: palette[0].name != "minecraft:air",
                }
            } else if !palette.iter().any(|p| p.name == "minecraft:air") {
                SectionKind::NoAir
            } else if let Some(data) = &s.block_states.data {
                let mut bits = 4;
                while (1usize << bits) < palette.len() {
                    bits += 1;
                }
                SectionKind::Mixed {
                    data,
                    bits,
                    vals_per_long: 64 / bits,
                    mask: (1u64 << bits) - 1,
                }
            } else {
                SectionKind::Uniform { solid: false }
            };
            SectionMeta {
                y: s.y,
                palette,
                kind,
            }
        })
        .collect();

    // Sort by Y descending so we scan top-down
    metas.sort_by_key(|b| std::cmp::Reverse(b.y));

    let mut heights = [0i32; 256]; // 16x16 grid, Z-major order
    let min_block_y = min_section_y as i32 * 16;

    for z in 0..16usize {
        for x in 0..16usize {
            let col_idx = z * 16 + x;

            'outer: for meta in &metas {
                match &meta.kind {
                    SectionKind::Uniform { solid: false } => continue,
                    SectionKind::Uniform { solid: true } | SectionKind::NoAir => {
                        let abs_y = (meta.y as i32) * 16 + 15;
                        heights[col_idx] = abs_y - min_block_y + 1;
                        break 'outer;
                    }
                    SectionKind::Mixed {
                        data,
                        bits,
                        vals_per_long,
                        mask,
                    } => {
                        for local_y in (0..16usize).rev() {
                            let block_idx = local_y * 256 + z * 16 + x;
                            let long_idx = block_idx / vals_per_long;
                            let bit_offset = (block_idx % vals_per_long) * bits;

                            if long_idx < data.len() {
                                let palette_idx =
                                    ((data[long_idx] as u64 >> bit_offset) & mask) as usize;
                                if palette_idx < meta.palette.len()
                                    && meta.palette[palette_idx].name != "minecraft:air"
                                {
                                    let abs_y = (meta.y as i32) * 16 + local_y as i32;
                                    heights[col_idx] = abs_y - min_block_y + 1;
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let packed = pack_heightmap_values(&heights, total_height);
    // All four heightmap types use the same data. Vanilla differentiates them (e.g.
    // MOTION_BLOCKING includes fluids, OCEAN_FLOOR excludes them), but servers
    // recompute heightmaps on load — they just need valid structure here.
    Value::Compound(HashMap::from([
        (
            "MOTION_BLOCKING".to_string(),
            Value::LongArray(packed.clone()),
        ),
        (
            "MOTION_BLOCKING_NO_LEAVES".to_string(),
            Value::LongArray(packed.clone()),
        ),
        ("OCEAN_FLOOR".to_string(), Value::LongArray(packed.clone())),
        ("WORLD_SURFACE".to_string(), Value::LongArray(packed)),
    ]))
}

/// Pack 256 heightmap values into a LongArray with dynamic bit width.
/// Bit width = ceil(log2(total_height + 1)). Values don't span across longs.
fn pack_heightmap_values(values: &[i32; 256], total_height: i32) -> LongArray {
    // Calculate bits needed: ceil(log2(total_height + 1)), minimum 9
    let bits = ((total_height + 1) as f64).log2().ceil().max(9.0) as usize;
    let vals_per_long = 64 / bits;
    let num_longs = 256_usize.div_ceil(vals_per_long);
    let mask = (1i64 << bits) - 1;

    let mut result = Vec::with_capacity(num_longs);
    let mut current: i64 = 0;
    let mut bit_pos = 0;

    for &val in values.iter() {
        if bit_pos + bits > 64 {
            result.push(current);
            current = 0;
            bit_pos = 0;
        }
        current |= ((val as i64) & mask) << bit_pos;
        bit_pos += bits;
    }
    if bit_pos > 0 {
        result.push(current);
    }

    LongArray::new(result)
}

/// Merge compound lists (entities, block_entities) from chunk_to_modify into chunk
/// Note: Currently unused since we write directly without merging, but kept for potential future use
#[allow(dead_code)]
fn merge_compound_list(chunk: &mut Chunk, chunk_to_modify: &ChunkToModify, key: &str) {
    if let Some(existing_entities) = chunk.other.get_mut(key) {
        if let Some(new_entities) = chunk_to_modify.other.get(key) {
            if let (Value::List(existing), Value::List(new)) = (existing_entities, new_entities) {
                existing.retain(|e| {
                    if let Value::Compound(map) = e {
                        if let Some((x, y, z)) = get_entity_coords(map) {
                            return !new.iter().any(|new_e| {
                                if let Value::Compound(new_map) = new_e {
                                    get_entity_coords(new_map) == Some((x, y, z))
                                } else {
                                    false
                                }
                            });
                        }
                    }
                    true
                });
                existing.extend(new.clone());
            }
        }
    } else if let Some(new_entities) = chunk_to_modify.other.get(key) {
        chunk.other.insert(key.to_string(), new_entities.clone());
    }
}

/// Convert NBT Value to i32
/// Note: Currently unused since we write directly without merging, but kept for potential future use
#[allow(dead_code)]
fn value_to_i32(value: &Value) -> Option<i32> {
    match value {
        Value::Byte(v) => Some(i32::from(*v)),
        Value::Short(v) => Some(i32::from(*v)),
        Value::Int(v) => Some(*v),
        Value::Long(v) => i32::try_from(*v).ok(),
        Value::Float(v) => Some(*v as i32),
        Value::Double(v) => Some(*v as i32),
        _ => None,
    }
}
