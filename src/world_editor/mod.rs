//! World editor module for generating Minecraft worlds.
//!
//! This module provides the `WorldEditor` struct which handles block placement
//! and world saving in both Java Edition (Anvil) and Bedrock Edition (.mcworld) formats.
//!
//! # Module Structure
//!
//! - `common` - Shared data structures for world modification
//! - `java` - Java Edition Anvil format saving
//! - `bedrock` - Bedrock Edition .mcworld format saving (behind `bedrock` feature)

mod common;
mod java;

#[cfg(feature = "bedrock")]
pub mod bedrock;

// Re-export common types used internally
pub(crate) use common::WorldToModify;

#[cfg(feature = "bedrock")]
pub(crate) use bedrock::{BedrockSaveError, BedrockWriter};

use crate::block_definitions::*;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::LLBBox;
use crate::ground::Ground;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use fastnbt::{IntArray, Value};
use serde::Serialize;
use std::collections::{hash_map::Entry, HashMap};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};

/// World format to generate
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub enum WorldFormat {
    /// Java Edition Anvil format (.mca region files)
    JavaAnvil,
    /// Bedrock Edition .mcworld format
    BedrockMcWorld,
}

/// Metadata saved with the world
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorldMetadata {
    pub min_mc_x: i32,
    pub max_mc_x: i32,
    pub min_mc_z: i32,
    pub max_mc_z: i32,

    pub min_geo_lat: f64,
    pub max_geo_lat: f64,
    pub min_geo_lon: f64,
    pub max_geo_lon: f64,
}

/// The main world editor struct for placing blocks and saving worlds.
///
/// The lifetime `'a` is tied to the `XZBBox` reference, which defines
/// the world boundaries and must outlive the WorldEditor instance.
pub struct WorldEditor<'a> {
    world_dir: PathBuf,
    world: WorldToModify,
    xzbbox: &'a XZBBox,
    llbbox: LLBBox,
    ground: Option<Box<Ground>>,
    format: WorldFormat,
    /// Optional level name for Bedrock worlds (e.g., "Arnis World: New York City")
    bedrock_level_name: Option<String>,
    /// Optional spawn point for Bedrock worlds (x, z coordinates)
    bedrock_spawn_point: Option<(i32, i32)>,
}

impl<'a> WorldEditor<'a> {
    /// Creates a new WorldEditor with Java Anvil format (default).
    ///
    /// This is the default constructor used by CLI mode.
    #[allow(dead_code)]
    pub fn new(world_dir: PathBuf, xzbbox: &'a XZBBox, llbbox: LLBBox) -> Self {
        Self {
            world_dir,
            world: WorldToModify::default(),
            xzbbox,
            llbbox,
            ground: None,
            format: WorldFormat::JavaAnvil,
            bedrock_level_name: None,
            bedrock_spawn_point: None,
        }
    }

    /// Creates a new WorldEditor with a specific format and optional level name.
    ///
    /// Used by GUI mode to support both Java and Bedrock formats.
    #[allow(dead_code)]
    pub fn new_with_format_and_name(
        world_dir: PathBuf,
        xzbbox: &'a XZBBox,
        llbbox: LLBBox,
        format: WorldFormat,
        bedrock_level_name: Option<String>,
        bedrock_spawn_point: Option<(i32, i32)>,
    ) -> Self {
        Self {
            world_dir,
            world: WorldToModify::default(),
            xzbbox,
            llbbox,
            ground: None,
            format,
            bedrock_level_name,
            bedrock_spawn_point,
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

    /// Returns the current world format
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

    /// Returns the minimum world coordinates
    pub fn get_min_coords(&self) -> (i32, i32) {
        (self.xzbbox.min_x(), self.xzbbox.min_z())
    }

    /// Returns the maximum world coordinates
    pub fn get_max_coords(&self) -> (i32, i32) {
        (self.xzbbox.max_x(), self.xzbbox.max_z())
    }

    /// Checks if there's a block at the given coordinates
    #[allow(unused)]
    #[inline]
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> bool {
        let absolute_y = self.get_absolute_y(x, y, z);
        self.world.get_block(x, absolute_y, z).is_some()
    }

    /// Sets a sign at the given coordinates
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

        let region = self.world.get_or_create_region(region_x, region_z);
        let chunk = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

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

    /// Adds an entity at the given coordinates (Y is ground-relative).
    #[allow(dead_code)]
    pub fn add_entity(
        &mut self,
        id: &str,
        x: i32,
        y: i32,
        z: i32,
        extra_data: Option<HashMap<String, Value>>,
    ) {
        if !self.xzbbox.contains(&XZPoint::new(x, z)) {
            return;
        }

        let absolute_y = self.get_absolute_y(x, y, z);

        let mut entity = HashMap::new();
        entity.insert("id".to_string(), Value::String(id.to_string()));
        entity.insert(
            "Pos".to_string(),
            Value::List(vec![
                Value::Double(x as f64 + 0.5),
                Value::Double(absolute_y as f64),
                Value::Double(z as f64 + 0.5),
            ]),
        );
        entity.insert(
            "Motion".to_string(),
            Value::List(vec![
                Value::Double(0.0),
                Value::Double(0.0),
                Value::Double(0.0),
            ]),
        );
        entity.insert(
            "Rotation".to_string(),
            Value::List(vec![Value::Float(0.0), Value::Float(0.0)]),
        );
        entity.insert("OnGround".to_string(), Value::Byte(1));
        entity.insert("FallDistance".to_string(), Value::Float(0.0));
        entity.insert("Fire".to_string(), Value::Short(-20));
        entity.insert("Air".to_string(), Value::Short(300));
        entity.insert("PortalCooldown".to_string(), Value::Int(0));
        entity.insert(
            "UUID".to_string(),
            Value::IntArray(build_deterministic_uuid(id, x, absolute_y, z)),
        );

        if let Some(extra) = extra_data {
            for (key, value) in extra {
                entity.insert(key, value);
            }
        }

        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;

        let region = self.world.get_or_create_region(region_x, region_z);
        let chunk = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

        match chunk.other.entry("entities".to_string()) {
            Entry::Occupied(mut entry) => {
                if let Value::List(list) = entry.get_mut() {
                    list.push(Value::Compound(entity));
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(Value::List(vec![Value::Compound(entity)]));
            }
        }
    }

    /// Places a chest with the provided items at the given coordinates (ground-relative Y).
    #[allow(dead_code)]
    pub fn set_chest_with_items(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        items: Vec<HashMap<String, Value>>,
    ) {
        let absolute_y = self.get_absolute_y(x, y, z);
        self.set_chest_with_items_absolute(x, absolute_y, z, items);
    }

    /// Places a chest with the provided items at the given coordinates (absolute Y).
    #[allow(dead_code)]
    pub fn set_chest_with_items_absolute(
        &mut self,
        x: i32,
        absolute_y: i32,
        z: i32,
        items: Vec<HashMap<String, Value>>,
    ) {
        if !self.xzbbox.contains(&XZPoint::new(x, z)) {
            return;
        }

        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;

        let mut chest_data = HashMap::new();
        chest_data.insert(
            "id".to_string(),
            Value::String("minecraft:chest".to_string()),
        );
        chest_data.insert("x".to_string(), Value::Int(x));
        chest_data.insert("y".to_string(), Value::Int(absolute_y));
        chest_data.insert("z".to_string(), Value::Int(z));
        chest_data.insert(
            "Items".to_string(),
            Value::List(items.into_iter().map(Value::Compound).collect()),
        );
        chest_data.insert("keepPacked".to_string(), Value::Byte(0));

        let region = self.world.get_or_create_region(region_x, region_z);
        let chunk = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

        match chunk.other.entry("block_entities".to_string()) {
            Entry::Occupied(mut entry) => {
                if let Value::List(list) = entry.get_mut() {
                    list.push(Value::Compound(chest_data));
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(Value::List(vec![Value::Compound(chest_data)]));
            }
        }

        self.set_block_absolute(CHEST, x, absolute_y, z, None, None);
    }

    /// Convenience helper: place a chest prefilled with one white wool block (absolute Y).
    #[allow(dead_code)]
    pub fn set_chest_with_white_wool_absolute(&mut self, x: i32, absolute_y: i32, z: i32) {
        let items = vec![single_item("minecraft:white_wool", 0, 1)];
        self.set_chest_with_items_absolute(x, absolute_y, z, items);
    }

    /// Sets a block of the specified type at the given coordinates.
    ///
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
    ///
    /// Unlike `check_for_block_absolute`, this doesn't filter by block type.
    #[allow(unused)]
    pub fn block_at_absolute(&self, x: i32, absolute_y: i32, z: i32) -> bool {
        self.world.get_block(x, absolute_y, z).is_some()
    }

    /// Saves all changes made to the world by writing to the appropriate format.
    pub fn save(&mut self) {
        println!(
            "Generating world for: {}",
            match self.format {
                WorldFormat::JavaAnvil => "Java Edition (Anvil)",
                WorldFormat::BedrockMcWorld => "Bedrock Edition (.mcworld)",
            }
        );

        match self.format {
            WorldFormat::JavaAnvil => self.save_java(),
            WorldFormat::BedrockMcWorld => self.save_bedrock(),
        }
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
    fn save_bedrock_internal(&mut self) -> Result<(), BedrockSaveError> {
        // Use the stored level name if available, otherwise extract from path
        let level_name = self.bedrock_level_name.clone().unwrap_or_else(|| {
            self.world_dir
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Arnis World")
                .to_string()
        });

        BedrockWriter::new(
            self.world_dir.clone(),
            level_name,
            self.bedrock_spawn_point,
            self.ground.clone(),
        )
        .write_world(&self.world, self.xzbbox, &self.llbbox)
    }

    /// Saves world metadata to a JSON file
    pub(crate) fn save_metadata(&mut self) -> Result<(), Box<dyn std::error::Error>> {
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

#[allow(dead_code)]
fn build_deterministic_uuid(id: &str, x: i32, y: i32, z: i32) -> IntArray {
    let mut hash: i64 = 17;
    for byte in id.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(byte as i64);
    }

    let seed_a = hash ^ (x as i64).wrapping_shl(32) ^ (y as i64).wrapping_mul(17);
    let seed_b = hash.rotate_left(7) ^ (z as i64).wrapping_mul(31) ^ (x as i64).wrapping_mul(13);

    IntArray::new(vec![
        (seed_a >> 32) as i32,
        seed_a as i32,
        (seed_b >> 32) as i32,
        seed_b as i32,
    ])
}

#[allow(dead_code)]
fn single_item(id: &str, slot: i8, count: i8) -> HashMap<String, Value> {
    let mut item = HashMap::new();
    item.insert("id".to_string(), Value::String(id.to_string()));
    item.insert("Slot".to_string(), Value::Byte(slot));
    item.insert("Count".to_string(), Value::Byte(count));
    item
}
