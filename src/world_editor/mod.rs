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
pub use common::MIN_Y;

#[cfg(feature = "bedrock")]
pub(crate) use bedrock::{BedrockSaveError, BedrockWriter};

use crate::block_definitions::*;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::LLBBox;
use crate::ground::Ground;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use fastnbt::{IntArray, Value};
use fnv::FnvHashMap;
use serde::Serialize;
use std::collections::{hash_map::Entry, HashMap};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "gui")]
use crate::progress::emit_gui_error;
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};

/// Walks the error chain to determine whether a save failure was caused by
/// insufficient disk space.
///
/// Source chain entries (`err.source()` returns `&(dyn Error + 'static)`) are
/// inspected via `io::Error` downcast, checking `ErrorKind::StorageFull` (stable
/// since Rust 1.83) and raw OS error codes (112 = Windows `ERROR_DISK_FULL`,
/// 28 = Unix `ENOSPC`). The top-level error and any entries that cannot be
/// downcast to `io::Error` use Display substring matching as fallback (handles
/// wrappers like `fastanvil::RegionError` that forward the OS message in their
/// Display string but do not expose `io::Error` in the source chain).
fn is_disk_full_error(err: &dyn std::error::Error) -> bool {
    // Fallback string check on the top-level error, which may not be downcastable
    // without a 'static bound on the parameter.
    let s = err.to_string();
    if s.contains("os error 112") || s.contains("os error 28") || s.contains("StorageFull") {
        return true;
    }

    // Walk the source chain. source() yields &(dyn Error + 'static), which
    // allows downcasting to concrete types via the inherent downcast_ref method.
    let mut source = err.source();
    while let Some(e) = source {
        // Primary: downcast to io::Error for structured ErrorKind / OS code checks.
        if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
            if io_err.kind() == std::io::ErrorKind::StorageFull {
                return true;
            }
            if matches!(io_err.raw_os_error(), Some(112) | Some(28)) {
                return true;
            }
        }
        // Fallback: string check for wrappers that don't expose io::Error directly.
        let s = e.to_string();
        if s.contains("os error 112") || s.contains("os error 28") || s.contains("StorageFull") {
            return true;
        }
        source = e.source();
    }

    false
}

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
    ground: Option<Arc<Ground>>,
    format: WorldFormat,
    /// Per-cell overrides for the effective "ground surface" Y returned by
    /// `get_ground_level` / `get_absolute_y`. Roads that flatten their
    /// cross-section register their chosen Y here so that the later
    /// `ground_generation` pass builds the surface at the road's level —
    /// producing a natural-looking embankment on the low side and a cut on
    /// the high side rather than a floating strip with cliffs at the edges.
    ///
    /// Uses FNV hashing (not SipHash): `get_ground_level` sits on a hot
    /// path (called per-block during placement), so the hash cost matters.
    road_surface_overrides: FnvHashMap<(i32, i32), i32>,
    /// Optional level name for Bedrock worlds (e.g., "Arnis World: New York City")
    #[cfg(feature = "bedrock")]
    bedrock_level_name: Option<String>,
    /// Optional spawn point for Bedrock worlds (x, z coordinates)
    #[cfg(feature = "bedrock")]
    bedrock_spawn_point: Option<(i32, i32)>,
    #[cfg(feature = "bedrock")]
    bedrock_extend_height: bool,
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
            road_surface_overrides: FnvHashMap::default(),
            #[cfg(feature = "bedrock")]
            bedrock_level_name: None,
            #[cfg(feature = "bedrock")]
            bedrock_spawn_point: None,
            #[cfg(feature = "bedrock")]
            bedrock_extend_height: false,
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
        #[cfg_attr(not(feature = "bedrock"), allow(unused_variables))] bedrock_level_name: Option<
            String,
        >,
        #[cfg_attr(not(feature = "bedrock"), allow(unused_variables))] bedrock_spawn_point: Option<
            (i32, i32),
        >,
        #[cfg_attr(not(feature = "bedrock"), allow(unused_variables))] bedrock_extend_height: bool,
    ) -> Self {
        Self {
            world_dir,
            world: WorldToModify::default(),
            xzbbox,
            llbbox,
            ground: None,
            format,
            road_surface_overrides: FnvHashMap::default(),
            #[cfg(feature = "bedrock")]
            bedrock_level_name,
            #[cfg(feature = "bedrock")]
            bedrock_spawn_point,
            #[cfg(feature = "bedrock")]
            bedrock_extend_height,
        }
    }

    /// Sets the ground reference for elevation-based block placement
    pub fn set_ground(&mut self, ground: Arc<Ground>) {
        self.ground = Some(ground);
    }

    /// Gets a reference to the ground data if available
    pub fn get_ground(&self) -> Option<&Ground> {
        self.ground.as_deref()
    }

    /// Returns the current world format
    #[allow(dead_code)]
    pub fn format(&self) -> WorldFormat {
        self.format
    }

    /// Calculate the absolute Y position from a ground-relative offset
    #[inline(always)]
    pub fn get_absolute_y(&self, x: i32, y_offset: i32, z: i32) -> i32 {
        self.get_ground_level(x, z) + y_offset
    }

    /// Get the effective ground level at a world coordinate.
    ///
    /// Checks the road-surface override map first so that a later
    /// `ground_generation` pass will build terrain matching the road's
    /// flattened cross-section. Falls back to `Ground::level` otherwise.
    ///
    /// The `is_empty` guard matters: this function is called per-block
    /// during element processing, so every element placed before highways
    /// run (most elements in small bboxes, all non-road elements before
    /// priority-ordering kicks highways to the front) would otherwise pay
    /// a hash + bucket-probe per call even though the map is empty.
    #[inline(always)]
    pub fn get_ground_level(&self, x: i32, z: i32) -> i32 {
        if !self.road_surface_overrides.is_empty() {
            if let Some(&y) = self.road_surface_overrides.get(&(x, z)) {
                return y;
            }
        }
        if let Some(ground) = &self.ground {
            ground.level(XZPoint::new(
                x - self.xzbbox.min_x(),
                z - self.xzbbox.min_z(),
            ))
        } else {
            0 // Default ground level if no terrain data
        }
    }

    /// Register a flattened ground Y for a road cell. See
    /// `road_surface_overrides` for the full rationale; in short: wide roads
    /// choose a single Y per cross-section so all lateral blocks sit flat,
    /// and the override lets the later ground pass fill below / cut above to
    /// match, yielding natural embankments/cuts instead of floating strips.
    ///
    /// Last writer wins — acceptable because road placements for the same
    /// cell come from overlapping stamps of adjacent centerline points whose
    /// target Ys differ by at most ~1 block.
    #[inline]
    pub fn register_road_surface_y(&mut self, x: i32, z: i32, y: i32) {
        self.road_surface_overrides.insert((x, z), y);
    }

    /// Get the water-appropriate Y level at a world coordinate.
    /// On steep terrain, snaps to the local minimum to compensate for
    /// spatial misalignment between water data and the elevation DEM.
    #[inline(always)]
    pub fn get_water_level(&self, x: i32, z: i32) -> i32 {
        if let Some(ground) = &self.ground {
            ground.water_level(XZPoint::new(
                x - self.xzbbox.min_x(),
                z - self.xzbbox.min_z(),
            ))
        } else {
            0
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
    #[allow(clippy::too_many_arguments)]
    pub fn place_wall_banner(
        &mut self,
        block: Block,
        x: i32,
        y: i32,
        z: i32,
        facing: &str,              // "north" / "south" / "east" / "west"
        base_color: &str,          // "light_gray" etc.
        patterns: &[(&str, &str)], // [("red", "minecraft:triangle_top"), ...]
    ) {
        // Apply Block rotation
        self.set_block_with_properties_absolute(
            crate::block_definitions::BlockWithProperties::new(
                block,
                Some(fastnbt::nbt!({ "facing": facing })),
            ),
            x,
            y,
            z,
            None,
            None,
        );
        match self.format() {
            crate::world_editor::WorldFormat::JavaAnvil => {
                self.set_banner_block_entity_absolute(x, y, z, patterns);
            }
            crate::world_editor::WorldFormat::BedrockMcWorld => {
                self.set_bedrock_banner_block_entity_absolute(x, y, z, base_color, patterns);
            }
        }
    }

    fn insert_block_entity(&mut self, x: i32, z: i32, be: HashMap<String, Value>) {
        if !self.xzbbox.contains(&XZPoint::new(x, z)) {
            return;
        }
        let chunk_x = x >> 4;
        let chunk_z = z >> 4;
        let region_x = chunk_x >> 5;
        let region_z = chunk_z >> 5;

        let region = self.world.get_or_create_region(region_x, region_z);
        let chunk = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

        const BLOCK_ENTITIES_KEY: &str = "block_entities";

        match chunk.other.entry(BLOCK_ENTITIES_KEY.to_string()) {
            Entry::Occupied(mut entry) => {
                if let Value::List(list) = entry.get_mut() {
                    list.push(Value::Compound(be));
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(Value::List(vec![Value::Compound(be)]));
            }
        }
    }

    /// Places a banner block entity at the given coordinates (absolute Y).
    /// This writes the pattern data into the chunk's block_entities list,
    /// which is required for the banner patterns to appear in-game.
    fn set_banner_block_entity_absolute(
        &mut self,
        x: i32,
        absolute_y: i32,
        z: i32,
        patterns_list: &[(&str, &str)],
    ) {
        let mut be = HashMap::new();
        be.insert(
            "id".to_string(),
            Value::String("minecraft:banner".to_string()),
        );
        be.insert("x".to_string(), Value::Int(x));
        be.insert("y".to_string(), Value::Int(absolute_y));
        be.insert("z".to_string(), Value::Int(z));
        be.insert("keepPacked".to_string(), Value::Byte(0));
        let patterns: Vec<Value> = patterns_list
            .iter()
            .map(|(color, pattern)| {
                let mut entry = HashMap::new();

                entry.insert("color".to_string(), Value::String(color.to_string()));
                entry.insert("pattern".to_string(), Value::String(pattern.to_string()));
                Value::Compound(entry)
            })
            .collect();
        be.insert("patterns".to_string(), Value::List(patterns));
        be.insert("components".to_string(), Value::Compound(HashMap::new()));
        self.insert_block_entity(x, z, be);
    }

    /// Places a Bedrock-format banner block entity at the given coordinates (absolute Y).
    ///
    /// Bedrock banners use a completely different block entity schema from Java:
    ///   - `Base`:     Int  — base color index (0=black … 15=white; light_gray=7)
    ///   - `Patterns`: List — each entry has `Color` (Int) and `Pattern` (String, short code)
    ///   - `Type`:     Int  — 0 = normal banner
    ///
    /// Java color names and pattern resource-path IDs are converted here to their
    /// Bedrock integer color indices and short pattern codes.
    fn set_bedrock_banner_block_entity_absolute(
        &mut self,
        x: i32,
        absolute_y: i32,
        z: i32,
        base_color: &str,
        patterns: &[(&str, &str)], // &[(java_color_name, java_pattern_id)]
    ) {
        /// Maps a Java color name to the Bedrock integer color index used in banner
        /// block entities.  The ordering is the standard Minecraft dye index.
        fn java_color_to_bedrock_int(color: &str) -> i32 {
            match color {
                "black" => 0,
                "red" => 1,
                "green" => 2,
                "brown" => 3,
                "blue" => 4,
                "purple" => 5,
                "cyan" => 6,
                "light_gray" => 7,
                "gray" => 8,
                "pink" => 9,
                "lime" => 10,
                "yellow" => 11,
                "light_blue" => 12,
                "magenta" => 13,
                "orange" => 14,
                "white" => 15,
                _ => 0,
            }
        }

        /// Maps a Java banner pattern resource-path ID (e.g. "minecraft:triangle_top")
        /// to the Bedrock short pattern code (e.g. "tts").
        fn java_pattern_to_bedrock_code(pattern: &str) -> &'static str {
            // Strip the optional "minecraft:" namespace prefix
            let key = pattern.strip_prefix("minecraft:").unwrap_or(pattern);
            match key {
                "base" => "b",
                "square_bottom_left" => "bl",
                "square_bottom_right" => "br",
                "square_top_left" => "tl",
                "square_top_right" => "tr",
                "stripe_bottom" => "bs",
                "stripe_top" => "ts",
                "stripe_left" => "ls",
                "stripe_right" => "rs",
                "stripe_center" => "cs",
                "stripe_middle" => "ms",
                "stripe_downright" => "drs",
                "stripe_downleft" => "dls",
                "stripe_small" => "ss",
                "cross" => "cr",
                "straight_cross" => "sc",
                "triangle_bottom" => "bt",
                "triangle_top" => "tt",
                "triangles_bottom" => "bts",
                "triangles_top" => "tts",
                "diagonal_left" => "ld",
                "diagonal_right" => "rd",
                "diagonal_up_left" => "lud",
                "diagonal_up_right" => "rud",
                "circle" => "mc",
                "rhombus" => "mr",
                "half_vertical" => "vh",
                "half_vertical_right" => "vhr",
                "half_horizontal" => "hh",
                "half_horizontal_bottom" => "hhb",
                "border" => "bo",
                "curly_border" => "cbo",
                "gradient" => "gra",
                "gradient_up" => "gru",
                "bricks" => "bri",
                "globe" => "glb",
                "creeper" => "cre",
                "skull" => "sku",
                "flower" => "flo",
                "mojang" => "moj",
                "piglin" => "pig",
                "flow" => "flw",
                "guster" => "gus",
                _ => "b", // fallback: solid base
            }
        }

        let bedrock_patterns: Vec<Value> = patterns
            .iter()
            .map(|(color, pattern)| {
                let mut entry = HashMap::new();
                entry.insert(
                    "Color".to_string(),
                    Value::Int(java_color_to_bedrock_int(color)),
                );
                entry.insert(
                    "Pattern".to_string(),
                    Value::String(java_pattern_to_bedrock_code(pattern).to_string()),
                );
                Value::Compound(entry)
            })
            .collect();

        let mut be = HashMap::new();
        be.insert("id".to_string(), Value::String("Banner".to_string()));
        be.insert("x".to_string(), Value::Int(x));
        be.insert("y".to_string(), Value::Int(absolute_y));
        be.insert("z".to_string(), Value::Int(z));
        be.insert(
            "Base".to_string(),
            Value::Int(java_color_to_bedrock_int(base_color)),
        );
        be.insert("Patterns".to_string(), Value::List(bedrock_patterns));
        be.insert("Type".to_string(), Value::Int(0));

        self.insert_block_entity(x, z, be);
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

    /// Places a block entity with items at the given coordinates (ground-relative Y).
    #[allow(dead_code)]
    pub fn set_block_entity_with_items(
        &mut self,
        block_with_props: BlockWithProperties,
        x: i32,
        y: i32,
        z: i32,
        block_entity_id: &str,
        items: Vec<HashMap<String, Value>>,
    ) {
        let absolute_y = self.get_absolute_y(x, y, z);
        self.set_block_entity_with_items_absolute(
            block_with_props,
            x,
            absolute_y,
            z,
            block_entity_id,
            items,
        );
    }

    /// Places a block entity with items at the given coordinates (absolute Y).
    #[allow(dead_code)]
    pub fn set_block_entity_with_items_absolute(
        &mut self,
        block_with_props: BlockWithProperties,
        x: i32,
        absolute_y: i32,
        z: i32,
        block_entity_id: &str,
        items: Vec<HashMap<String, Value>>,
    ) {
        if !self.xzbbox.contains(&XZPoint::new(x, z)) {
            return;
        }

        let chunk_x: i32 = x >> 4;
        let chunk_z: i32 = z >> 4;
        let region_x: i32 = chunk_x >> 5;
        let region_z: i32 = chunk_z >> 5;

        let mut block_entity = HashMap::new();
        block_entity.insert("id".to_string(), Value::String(block_entity_id.to_string()));
        block_entity.insert("x".to_string(), Value::Int(x));
        block_entity.insert("y".to_string(), Value::Int(absolute_y));
        block_entity.insert("z".to_string(), Value::Int(z));
        block_entity.insert(
            "Items".to_string(),
            Value::List(items.into_iter().map(Value::Compound).collect()),
        );
        block_entity.insert("keepPacked".to_string(), Value::Byte(0));

        let region = self.world.get_or_create_region(region_x, region_z);
        let chunk = region.get_or_create_chunk(chunk_x & 31, chunk_z & 31);

        match chunk.other.entry("block_entities".to_string()) {
            Entry::Occupied(mut entry) => {
                if let Value::List(list) = entry.get_mut() {
                    list.push(Value::Compound(block_entity));
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(Value::List(vec![Value::Compound(block_entity)]));
            }
        }

        self.set_block_with_properties_absolute(block_with_props, x, absolute_y, z, None, None);
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
        // Short-circuit for out-of-bbox writes before we pay for a
        // ground-level lookup (bilinear interpolation of the elevation
        // grid). The downstream `set_block_with_properties_absolute`
        // does the same check, but only *after* we would have done the
        // elevation work.
        if !self.xzbbox.contains(&XZPoint::new(x, z)) {
            return;
        }
        self.set_block_absolute(
            block,
            x,
            self.get_absolute_y(x, y, z),
            z,
            override_whitelist,
            override_blacklist,
        );
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
        self.set_block_with_properties_absolute(
            BlockWithProperties {
                block,
                properties: None,
            },
            x,
            absolute_y,
            z,
            override_whitelist,
            override_blacklist,
        )
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

    /// Fill a rectangular volume with a block using absolute Y coordinates.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn fill_blocks_absolute(
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
            for abs_y in min_y..=max_y {
                for z in min_z..=max_z {
                    self.set_block_absolute(
                        block,
                        x,
                        abs_y,
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

    /// Sets a block only if no modification has been recorded yet at this
    /// position (i.e. the in-memory overlay still holds AIR).
    ///
    /// This is faster than `set_block_absolute` with `None` whitelists/blacklists
    /// because it avoids the double HashMap traversal.
    #[inline]
    pub fn set_block_if_absent_absolute(&mut self, block: Block, x: i32, absolute_y: i32, z: i32) {
        if !self.xzbbox.contains(&XZPoint::new(x, z)) {
            return;
        }
        self.world.set_block_if_absent(x, absolute_y, z, block);
    }

    /// Returns true if a non-AIR block exists at the given absolute coordinates.
    #[inline]
    pub fn block_exists_absolute(&self, x: i32, absolute_y: i32, z: i32) -> bool {
        self.world.get_block(x, absolute_y, z).is_some()
    }

    /// Fills an entire column from y_min to y_max with one block type.
    ///
    /// Resolves region/chunk once instead of per-Y-level, making underground
    /// fill (`--fillground`) dramatically faster.
    #[inline]
    pub fn fill_column_absolute(
        &mut self,
        block: Block,
        x: i32,
        z: i32,
        y_min: i32,
        y_max: i32,
        skip_existing: bool,
    ) {
        if !self.xzbbox.contains(&XZPoint::new(x, z)) {
            return;
        }
        self.world
            .fill_column(x, z, y_min, y_max, block, skip_existing);
    }

    /// Saves all changes made to the world by writing to the appropriate format.
    ///
    /// Returns `Err` on I/O failure so callers can abort the generation pipeline
    /// cleanly. A user-facing error message is also emitted via `emit_gui_error`
    /// before returning so the GUI is notified regardless of how the caller handles
    /// the `Result`.
    pub fn save(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!(
            "Generating world for: {}",
            match self.format {
                WorldFormat::JavaAnvil => "Java Edition (Anvil)",
                WorldFormat::BedrockMcWorld => "Bedrock Edition (.mcworld)",
            }
        );

        // Compact sections before saving: collapses uniform Full(Vec) sections
        // (e.g. all-STONE from --fillground) back to Uniform, freeing ~4 KiB each.
        self.world.compact_sections();

        match self.format {
            WorldFormat::JavaAnvil => {
                if let Err(e) = self.save_java() {
                    let user_msg = if is_disk_full_error(e.as_ref()) {
                        "Not enough disk space available.".to_string()
                    } else {
                        format!("Failed to save world: {}", e)
                    };
                    eprintln!("{}", user_msg);
                    #[cfg(feature = "gui")]
                    {
                        send_log(LogLevel::Error, &user_msg);
                        emit_gui_error(&user_msg);
                    }
                    return Err(e);
                }
            }
            WorldFormat::BedrockMcWorld => self.save_bedrock(),
        }

        Ok(())
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
            self.bedrock_extend_height,
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
