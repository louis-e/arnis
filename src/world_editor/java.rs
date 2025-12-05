//! Java Edition Anvil format world saving.
//!
//! This module handles saving worlds in the Java Edition Anvil (.mca) format.

use super::common::{Chunk, ChunkToModify, Section};
use super::WorldEditor;
use crate::block_definitions::GRASS_BLOCK;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use fastanvil::Region;
use fastnbt::Value;
use fnv::FnvHashMap;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};

impl WorldEditor<'_> {
    /// Creates a region file for the given region coordinates.
    pub(super) fn create_region(&self, region_x: i32, region_z: i32) -> Region<File> {
        let out_path = self
            .world_dir
            .join(format!("region/r.{region_x}.{region_z}.mca"));

        const REGION_TEMPLATE: &[u8] = include_bytes!("../../assets/minecraft/region.template");

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

    /// Helper function to create a base chunk with grass blocks at Y -62
    pub(super) fn create_base_chunk(abs_chunk_x: i32, abs_chunk_z: i32) -> (Vec<u8>, bool) {
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

    /// Saves the world in Java Edition Anvil format.
    pub(super) fn save_java(&mut self) {
        println!("{} Saving world...", "[7/7]".bold());
        emit_gui_progress_update(90.0, "Saving world...");

        // Save metadata with error handling
        if let Err(e) = self.save_metadata() {
            eprintln!("Failed to save world metadata: {e}");
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
                        let mut chunk: Chunk = if existing_data.is_empty() {
                            Chunk {
                                sections: Vec::new(),
                                x_pos: chunk_x + (region_x * 32),
                                z_pos: chunk_z + (region_z * 32),
                                is_light_on: 0,
                                other: FnvHashMap::default(),
                            }
                        } else {
                            fastnbt::from_bytes(&existing_data).unwrap()
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
                let regions_done = regions_processed.fetch_add(1, Ordering::SeqCst) + 1;

                // Update progress at regular intervals (every ~1% or at least every 10 regions)
                // This ensures progress is visible even with many regions
                let update_interval = (total_regions / 10).max(1);
                if regions_done.is_multiple_of(update_interval) || regions_done == total_regions {
                    let progress = 90.0 + (regions_done as f64 / total_regions as f64) * 9.0;
                    emit_gui_progress_update(progress, "Saving world...");
                }

                save_pb.inc(1);
            });

        save_pb.finish();
    }
}

/// Helper function to get entity coordinates
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

/// Creates a Level wrapper for chunk data (Java Edition format)
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

                            // Only add the `data` attribute if it's non-empty
                            // to maintain compatibility with third-party tools like Dynmap
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
