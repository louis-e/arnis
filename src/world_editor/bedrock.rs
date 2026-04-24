//! Bedrock Edition .mcworld format world saving.
//!
//! This module handles saving worlds in the Bedrock Edition format,
//! producing .mcworld files that can be imported into Minecraft Bedrock.

use super::common::{ChunkToModify, SectionToModify, WorldToModify};
use super::WorldMetadata;
use crate::bedrock_block_map::{
    to_bedrock_block_with_properties, BedrockBlock, BedrockBlockStateValue,
};
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::ground::Ground;
use crate::progress::emit_gui_progress_update;

use bedrockrs_level::level::db_interface::key_level::KeyTypeTag;
use bedrockrs_level::level::db_interface::rusty::mcpe_options;
use bedrockrs_shared::world::dimension::Dimension;
use byteorder::{LittleEndian, WriteBytesExt};
use fastnbt::Value;
use indicatif::{ProgressBar, ProgressStyle};
use rusty_leveldb::DB;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::HashMap as StdHashMap;
use std::fs::{self, File};
use std::io::{Cursor, Write as IoWrite};
use std::path::PathBuf;
use std::sync::Arc;
use vek::Vec2;
use zip::write::FileOptions;
use zip::CompressionMethod;
use zip::ZipWriter;

/// Error type for Bedrock world saving operations
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

const DEFAULT_BEDROCK_COMPRESSION_LEVEL: u8 = 6;

/// Metadata for Bedrock worlds
#[derive(Serialize)]
struct BedrockMetadata {
    #[serde(flatten)]
    world: WorldMetadata,
    format: &'static str,
    chunk_count: usize,
}

/// Bedrock block state for NBT serialization
#[derive(Serialize)]
struct BedrockBlockState {
    name: String,
    states: BTreeMap<String, BedrockNbtValue>,
}

/// NBT-compatible value types for Bedrock block states
#[derive(Serialize)]
#[serde(untagged)]
enum BedrockNbtValue {
    String(String),
    Byte(i8),
    Int(i32),
}

impl From<&BedrockBlockStateValue> for BedrockNbtValue {
    #[inline]
    fn from(value: &BedrockBlockStateValue) -> Self {
        match value {
            BedrockBlockStateValue::String(s) => BedrockNbtValue::String(s.clone()),
            BedrockBlockStateValue::Bool(b) => BedrockNbtValue::Byte(if *b { 1 } else { 0 }),
            BedrockBlockStateValue::Int(i) => BedrockNbtValue::Int(*i),
        }
    }
}

/// Writer for Bedrock Edition worlds
pub struct BedrockWriter {
    output_dir: PathBuf,
    level_name: String,
    spawn_point: Option<(i32, i32)>,
    ground: Option<Arc<Ground>>,
    extend_build_height: bool,
}

impl BedrockWriter {
    /// Creates a new BedrockWriter
    pub fn new(
        output_path: PathBuf,
        level_name: String,
        spawn_point: Option<(i32, i32)>,
        ground: Option<Arc<Ground>>,
        extend_build_height: bool,
    ) -> Self {
        // If the path ends with .mcworld, use it as the final archive path
        // and create a temp directory without that extension for working files
        let output_dir = if output_path.extension().is_some_and(|ext| ext == "mcworld") {
            output_path.with_extension("")
        } else {
            output_path
        };

        Self {
            output_dir,
            level_name,
            spawn_point,
            ground,
            extend_build_height,
        }
    }

    /// Writes the world to disk
    pub fn write_world(
        &mut self,
        world: &WorldToModify,
        xzbbox: &XZBBox,
        llbbox: &LLBBox,
    ) -> Result<(), BedrockSaveError> {
        self.prepare_output_dir()?;
        self.write_level_name()?;

        emit_gui_progress_update(91.0, "Saving Bedrock world...");
        self.write_level_dat(xzbbox)?;

        emit_gui_progress_update(92.0, "Saving Bedrock world...");
        self.write_chunks_to_db(world)?;

        emit_gui_progress_update(97.0, "Saving Bedrock world...");
        self.write_metadata(world, xzbbox, llbbox)?;

        emit_gui_progress_update(98.0, "Saving Bedrock world...");
        self.package_mcworld()?;

        emit_gui_progress_update(99.0, "Saving Bedrock world...");
        self.cleanup_temp_dir()?;
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

        // Use custom spawn point if provided, otherwise center of bbox
        let (spawn_x, spawn_z) = self.spawn_point.unwrap_or_else(|| {
            let x = (xzbbox.min_x() + xzbbox.max_x()) / 2;
            let z = (xzbbox.min_z() + xzbbox.max_z()) / 2;
            (x, z)
        });

        // Calculate spawn Y from ground elevation data, or default to 64
        let spawn_y = self
            .ground
            .as_ref()
            .map(|ground| {
                // Ground elevation data expects coordinates relative to the XZ bbox origin
                let rel_x = spawn_x - xzbbox.min_x();
                let rel_z = spawn_z - xzbbox.min_z();
                let coord = crate::coordinate_system::cartesian::XZPoint::new(rel_x, rel_z);
                ground.level(coord) + 3 // Add 3 blocks above ground for safety
            })
            .unwrap_or(64);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        // Extended worlds require 1.21.40+ for Custom Biomes / dimension_bounds.
        // Only the user-facing version markers are bumped; network_version and
        // inventory_version stay at 1.21.0 because they're used for multiplayer
        // protocol and inventory storage format, neither of which changes with
        // the dimension_bounds feature. Bedrock tolerates this mix — what it
        // gates on is last_opened_with_version and minimum_compatible_client_version.
        let version_array: Vec<i32> = if self.extend_build_height {
            vec![1, 21, 40, 0, 0]
        } else {
            vec![1, 21, 0, 0, 0]
        };

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
            level_name: self.level_name.clone(),
            random_seed: 0,

            // Spawn location (Y derived from terrain elevation)
            spawn_x,
            spawn_y,
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
            do_mob_spawning: false,
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
            // dimension_bounds is gated behind the Custom Biomes experiment;
            // without these three flags the bundled BP is silently ignored
            // and blocks above Y=319 disappear on first load.
            saved_with_toggled_experiments: self.extend_build_height,
            experiments_ever_used: self.extend_build_height,
            experiments: BedrockExperiments {
                data_driven_biomes: self.extend_build_height,
            },

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
            .map_err(|e| BedrockSaveError::Nbt(format!("level.dat: {e}")))?;

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

        // Open LevelDB once for all writes (blocks, entities, block entities)
        let mut opts = mcpe_options(DEFAULT_BEDROCK_COMPRESSION_LEVEL);
        opts.create_if_missing = true;
        let mut db = DB::open(db_path.into_boxed_path(), opts)
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

        {
            let progress_bar = ProgressBar::new(total_chunks as u64);
            progress_bar.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} [{elapsed_precise}] [{bar:45.white/black}] {pos}/{len} chunks ({eta})")
                    .unwrap()
                    .progress_chars("█▓░"),
            );

            let mut chunks_processed: usize = 0;

            // Process each region and chunk (blocks + entities in a single pass)
            for ((region_x, region_z), region) in &world.regions {
                for ((local_chunk_x, local_chunk_z), chunk) in &region.chunks {
                    // Calculate absolute chunk coordinates
                    let abs_chunk_x = region_x * 32 + local_chunk_x;
                    let abs_chunk_z = region_z * 32 + local_chunk_z;
                    let chunk_pos = Vec2::new(abs_chunk_x, abs_chunk_z);

                    // Write chunk version marker (42 is current Bedrock version as of 1.21+)
                    let version_key = build_chunk_key_bytes(
                        chunk_pos,
                        Dimension::Overworld,
                        KeyTypeTag::Version,
                        None,
                    );
                    db.put(&version_key, &[42])
                        .map_err(|e| BedrockSaveError::Database(format!("{:?}", e)))?;

                    // Write Data3D (heightmap + biomes) - required for chunk to be valid
                    let data3d_key = build_chunk_key_bytes(
                        chunk_pos,
                        Dimension::Overworld,
                        KeyTypeTag::Data3D,
                        None,
                    );
                    let data3d = self.create_data3d(chunk);
                    db.put(&data3d_key, &data3d)
                        .map_err(|e| BedrockSaveError::Database(format!("{:?}", e)))?;

                    // Process each section (subchunk)
                    for (&section_y, section) in &chunk.sections {
                        let subchunk_bytes = self.encode_subchunk(section, section_y)?;

                        let subchunk_key = build_chunk_key_bytes(
                            chunk_pos,
                            Dimension::Overworld,
                            KeyTypeTag::SubChunkPrefix,
                            Some(section_y),
                        );
                        db.put(&subchunk_key, &subchunk_bytes)
                            .map_err(|e| BedrockSaveError::Database(format!("{:?}", e)))?;
                    }

                    // Write entities and block entities in the same pass
                    self.write_compound_list_record(
                        &mut db,
                        chunk_pos,
                        KeyTypeTag::BlockEntity,
                        chunk.other.get("block_entities"),
                    )?;
                    self.write_compound_list_record(
                        &mut db,
                        chunk_pos,
                        KeyTypeTag::Entity,
                        chunk.other.get("entities"),
                    )?;

                    chunks_processed += 1;
                    progress_bar.inc(1);

                    // Update GUI progress (92% to 97% range for chunk writing)
                    if chunks_processed.is_multiple_of(10) || chunks_processed == total_chunks {
                        let chunk_progress = chunks_processed as f64 / total_chunks as f64;
                        let gui_progress = 92.0 + (chunk_progress * 5.0); // 92% to 97%
                        emit_gui_progress_update(gui_progress, "");
                    }
                }
            }

            progress_bar.finish_with_message("Chunks written to LevelDB");
        }

        Ok(())
    }

    fn write_compound_list_record(
        &self,
        db: &mut DB,
        chunk_pos: Vec2<i32>,
        key_type: KeyTypeTag,
        value: Option<&Value>,
    ) -> Result<(), BedrockSaveError> {
        let Some(Value::List(values)) = value else {
            return Ok(());
        };

        if values.is_empty() {
            return Ok(());
        }

        let deduped = dedup_compound_list(values);
        if deduped.is_empty() {
            return Ok(());
        }

        // Bedrock block entities and entities are stored as CONCATENATED individual
        // NBT compounds — NOT as a single NBT list. Each compound is serialised
        // back-to-back with no wrapper. nbtx::to_le_bytes() on a Vec would produce
        // a TAG_List header, which Bedrock cannot parse.
        let mut data: Vec<u8> = Vec::new();
        for compound in &deduped {
            let bytes = nbtx::to_le_bytes(compound)
                .map_err(|e| BedrockSaveError::Nbt(format!("block-entity/entity compound: {e}")))?;
            data.extend_from_slice(&bytes);
        }

        let key = build_chunk_key_bytes(chunk_pos, Dimension::Overworld, key_type, None);
        db.put(&key, &data)
            .map_err(|e| BedrockSaveError::Database(format!("{:?}", e)))?;

        Ok(())
    }

    /// Creates a Data3D record containing heightmap and biome data.
    ///
    /// Format: 512 bytes heightmap (256 x i16 LE) + 28 bytes minimal biome data
    fn create_data3d(&self, _chunk: &ChunkToModify) -> Vec<u8> {
        let mut buffer = Vec::with_capacity(540);

        // Heightmap: 256 entries (16x16) as i16 LE, fixed height of 4 for flat world
        for _ in 0..256 {
            buffer.extend_from_slice(&4i16.to_le_bytes());
        }

        // Minimal biome data padding (biomes will be regenerated by the game)
        buffer.extend_from_slice(&[0u8; 28]);

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
        let word_count = 4096_u32.div_ceil(blocks_per_word);
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
            let nbt_bytes = nbtx::to_le_bytes(&state).map_err(|e| {
                BedrockSaveError::Nbt(format!("block palette state ({}): {e}", state.name))
            })?;
            buffer.write_all(&nbt_bytes)?;
        }

        Ok(buffer.into_inner())
    }

    /// Builds a palette and index array from a section.
    ///
    /// Converts from internal YZX ordering to Bedrock's XZY ordering:
    /// - Internal: index = y*256 + z*16 + x
    /// - Bedrock:  index = x*256 + z*16 + y
    ///
    /// Also propagates stored block properties (e.g., stair facing/shape) to the
    /// Bedrock palette, ensuring blocks with non-default states are serialized correctly.
    fn build_palette_and_indices(
        &self,
        section: &SectionToModify,
    ) -> Result<(Vec<BedrockBlock>, [u16; 4096]), BedrockSaveError> {
        let mut palette: Vec<BedrockBlock> = Vec::new();
        let mut palette_map: StdHashMap<BedrockBlock, u16> = StdHashMap::new();
        let mut indices = [0u16; 4096];

        // Add air as first palette entry (required by Bedrock format)
        let air_block = BedrockBlock::simple("air");
        palette_map.insert(air_block.clone(), 0);
        palette.push(air_block);

        // Convert blocks from internal YZX to Bedrock XZY ordering
        for x in 0..16usize {
            for z in 0..16usize {
                for y in 0..16usize {
                    let internal_idx = y * 256 + z * 16 + x;
                    let block = section.get_block_at_index(internal_idx);

                    // Get stored properties for this block position (if any)
                    let properties = section.properties.get(&internal_idx);

                    // Convert to Bedrock format, preserving properties
                    let bedrock_block = to_bedrock_block_with_properties(block, properties);

                    let palette_index = if let Some(&idx) = palette_map.get(&bedrock_block) {
                        idx
                    } else {
                        let idx = palette.len() as u16;
                        palette_map.insert(bedrock_block.clone(), idx);
                        palette.push(bedrock_block);
                        idx
                    };

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

        // Add world_icon.jpeg from embedded assets
        const WORLD_ICON: &[u8] = include_bytes!("../../assets/minecraft/world_icon.jpeg");
        writer.start_file("world_icon.jpeg", options)?;
        writer.write_all(WORLD_ICON)?;

        if self.extend_build_height {
            const BP_MANIFEST: &[u8] =
                include_bytes!("../../assets/minecraft/bp_tall/manifest.json");
            const BP_OVERWORLD: &[u8] =
                include_bytes!("../../assets/minecraft/bp_tall/dimensions/overworld.json");
            // Must match header.uuid in bp_tall/manifest.json.
            const BP_HEADER_UUID: &str = "a7f3b2e0-8c4d-4e92-9b1a-3d7f5c8e4a61";

            writer.add_directory("behavior_packs/", options)?;
            writer.add_directory("behavior_packs/arnis_tall/", options)?;
            writer.add_directory("behavior_packs/arnis_tall/dimensions/", options)?;

            writer.start_file("behavior_packs/arnis_tall/manifest.json", options)?;
            writer.write_all(BP_MANIFEST)?;

            writer.start_file(
                "behavior_packs/arnis_tall/dimensions/overworld.json",
                options,
            )?;
            writer.write_all(BP_OVERWORLD)?;

            writer.start_file("world_behavior_packs.json", options)?;
            let world_bp_json =
                format!(r#"[{{"pack_id":"{}","version":[1,0,0]}}]"#, BP_HEADER_UUID);
            writer.write_all(world_bp_json.as_bytes())?;
        }

        // Add db directory and its contents
        let db_path = self.output_dir.join("db");
        if db_path.is_dir() {
            add_directory_to_zip(&mut writer, &db_path, "db", options)?;
        }

        writer.finish()?;
        Ok(())
    }

    /// Clean up the temporary directory after packaging mcworld
    fn cleanup_temp_dir(&self) -> Result<(), BedrockSaveError> {
        if self.output_dir.exists() {
            fs::remove_dir_all(&self.output_dir)?;
        }
        Ok(())
    }
}

fn add_directory_to_zip(
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
            add_directory_to_zip(writer, &path, &zip_path, options)?;
        }
    }

    Ok(())
}

/// Calculate bits per block using valid Bedrock values: {1, 2, 3, 4, 5, 6, 8, 16}
#[inline]
fn bedrock_bits_per_block(palette_count: u32) -> u8 {
    const VALID_BITS: [u8; 8] = [1, 2, 3, 4, 5, 6, 8, 16];
    for &bits in &VALID_BITS {
        if palette_count <= (1u32 << bits) {
            return bits;
        }
    }
    16 // Maximum
}

fn build_chunk_key_bytes(
    chunk_pos: Vec2<i32>,
    dimension: Dimension,
    key_type: KeyTypeTag,
    y_index: Option<i8>,
) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(
        9 + if dimension != Dimension::Overworld {
            4
        } else {
            0
        } + 1,
    );
    buffer.extend_from_slice(&chunk_pos.x.to_le_bytes());
    buffer.extend_from_slice(&chunk_pos.y.to_le_bytes());

    if dimension != Dimension::Overworld {
        buffer.extend_from_slice(&i32::from(dimension).to_le_bytes());
    }

    buffer.push(key_type.to_byte());
    if let Some(y) = y_index {
        buffer.push(y as u8);
    }

    buffer
}

fn dedup_compound_list(values: &[Value]) -> Vec<Value> {
    let mut coord_index: StdHashMap<(i32, i32, i32), usize> = StdHashMap::new();
    let mut deduped: Vec<Value> = Vec::with_capacity(values.len());

    for value in values {
        if let Value::Compound(map) = value {
            if let Some(coords) = get_entity_coords(map) {
                if let Some(idx) = coord_index.get(&coords).copied() {
                    deduped[idx] = value.clone();
                    continue;
                } else {
                    coord_index.insert(coords, deduped.len());
                }
            }
        }
        deduped.push(value.clone());
    }

    deduped
}

fn get_entity_coords(entity: &StdHashMap<String, Value>) -> Option<(i32, i32, i32)> {
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
    /// Always emitted with all-false contents on standard worlds.
    /// nbtx can't skip optional fields, and an all-false compound
    /// is a no-op to Bedrock.
    #[serde(rename = "experiments")]
    experiments: BedrockExperiments,

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

/// Bedrock experimental-features NBT toggles. Must pair with
/// `experiments_ever_used` + `saved_with_toggled_experiments` on the root.
#[derive(Serialize)]
struct BedrockExperiments {
    /// Gates data-driven worldgen, incl. `minecraft:dimension_bounds`.
    data_driven_biomes: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrockrs_level::level::db_interface::bedrock_key::ChunkKey;
    use serde_json::Value;
    use zip::ZipArchive;

    /// Build a key using the canonical `ChunkKey` path (the source of truth from
    /// `bedrockrs_level`) so we can compare against `build_chunk_key_bytes`.
    fn canonical_key(chunk_key: ChunkKey) -> Vec<u8> {
        // Replicates RustyDBInterface::build_key without needing a DB instance
        use bedrockrs_level::level::db_interface::db::LevelDBKey;
        use std::io::Cursor;
        let mut key_bytes: Vec<u8> = vec![0; chunk_key.estimate_size()];
        let mut buff: Cursor<&mut [u8]> = Cursor::new(&mut key_bytes);
        chunk_key.write_key(&mut buff);
        key_bytes
    }

    #[test]
    fn build_chunk_key_bytes_matches_canonical_version() {
        let pos = Vec2::new(5, -3);
        let ours = build_chunk_key_bytes(pos, Dimension::Overworld, KeyTypeTag::Version, None);
        let canonical = canonical_key(ChunkKey::chunk_marker(pos, Dimension::Overworld));
        assert_eq!(ours, canonical, "Version key mismatch");
    }

    #[test]
    fn build_chunk_key_bytes_matches_canonical_data3d() {
        let pos = Vec2::new(-12, 7);
        let ours = build_chunk_key_bytes(pos, Dimension::Overworld, KeyTypeTag::Data3D, None);
        let canonical = canonical_key(ChunkKey::data3d(pos, Dimension::Overworld));
        assert_eq!(ours, canonical, "Data3D key mismatch");
    }

    #[test]
    fn build_chunk_key_bytes_matches_canonical_subchunk() {
        let pos = Vec2::new(100, -50);
        // Positive y index
        let ours = build_chunk_key_bytes(
            pos,
            Dimension::Overworld,
            KeyTypeTag::SubChunkPrefix,
            Some(4),
        );
        let canonical = canonical_key(ChunkKey::new_subchunk(pos, Dimension::Overworld, 4));
        assert_eq!(ours, canonical, "SubChunk y=4 key mismatch");
    }

    #[test]
    fn build_chunk_key_bytes_matches_canonical_negative_y() {
        let pos = Vec2::new(0, 0);
        // Negative y index (e.g. y = -4 for sections below y=0)
        let ours = build_chunk_key_bytes(
            pos,
            Dimension::Overworld,
            KeyTypeTag::SubChunkPrefix,
            Some(-4),
        );
        let canonical = canonical_key(ChunkKey::new_subchunk(pos, Dimension::Overworld, -4));
        assert_eq!(ours, canonical, "SubChunk y=-4 key mismatch");
    }

    #[test]
    fn writes_mcworld_package_with_metadata() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_dir = temp_dir.path().join("bedrock_world");

        let world = WorldToModify::default();
        let xzbbox = XZBBox::rect_from_xz_lengths(15.0, 15.0).unwrap();
        let llbbox = LLBBox::new(0.0, 0.0, 1.0, 1.0).unwrap();

        BedrockWriter::new(
            output_dir.clone(),
            "test-world".to_string(),
            None,
            None,
            false,
        )
        .write_world(&world, &xzbbox, &llbbox)
        .expect("write_world");

        // The temp directory should be cleaned up, but mcworld should exist
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

        // Check metadata inside the archive
        let metadata_file = archive
            .by_name("metadata.json")
            .expect("metadata in archive");
        let metadata: Value = serde_json::from_reader(metadata_file).expect("valid metadata JSON");

        assert_eq!(metadata["format"], "bedrock-mcworld");
        assert_eq!(metadata["chunk_count"], 0); // empty world structure
    }

    #[test]
    fn writes_mcworld_with_custom_spawn_point() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_dir = temp_dir.path().join("bedrock_world_spawn");

        let world = WorldToModify::default();
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let llbbox = LLBBox::new(0.0, 0.0, 1.0, 1.0).unwrap();

        // Custom spawn point at (42, 84)
        BedrockWriter::new(
            output_dir.clone(),
            "spawn-test".to_string(),
            Some((42, 84)),
            None,
            false,
        )
        .write_world(&world, &xzbbox, &llbbox)
        .expect("write_world");

        // Verify the mcworld was created
        let mcworld_path = output_dir.with_extension("mcworld");
        assert!(mcworld_path.exists(), "mcworld file should exist");
    }

    #[test]
    fn writes_mcworld_with_tall_behavior_pack_when_extended() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_dir = temp_dir.path().join("bedrock_world_tall");

        let world = WorldToModify::default();
        let xzbbox = XZBBox::rect_from_xz_lengths(15.0, 15.0).unwrap();
        let llbbox = LLBBox::new(0.0, 0.0, 1.0, 1.0).unwrap();

        BedrockWriter::new(
            output_dir.clone(),
            "tall-world".to_string(),
            None,
            None,
            true,
        )
        .write_world(&world, &xzbbox, &llbbox)
        .expect("write_world");

        let mcworld_path = output_dir.with_extension("mcworld");
        let file = fs::File::open(&mcworld_path).expect("mcworld archive exists");
        let mut archive = ZipArchive::new(file).expect("zip readable");

        let mut entries: Vec<String> = Vec::new();
        for i in 0..archive.len() {
            if let Ok(f) = archive.by_index(i) {
                entries.push(f.name().to_string());
            }
        }

        assert!(
            entries.contains(&"world_behavior_packs.json".to_string()),
            "missing world_behavior_packs.json: {entries:?}"
        );
        assert!(
            entries.contains(&"behavior_packs/arnis_tall/manifest.json".to_string()),
            "missing BP manifest: {entries:?}"
        );
        assert!(
            entries.contains(&"behavior_packs/arnis_tall/dimensions/overworld.json".to_string()),
            "missing BP overworld.json: {entries:?}"
        );

        // Pack won't load if header.uuid and pack_id drift apart.
        let manifest_bytes = {
            let mut f = archive
                .by_name("behavior_packs/arnis_tall/manifest.json")
                .expect("open manifest");
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut f, &mut buf).expect("read manifest");
            buf
        };
        let world_bp_bytes = {
            let mut f = archive
                .by_name("world_behavior_packs.json")
                .expect("open world_behavior_packs.json");
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut f, &mut buf).expect("read world_bp");
            buf
        };
        let manifest: Value = serde_json::from_slice(&manifest_bytes).expect("manifest JSON");
        let world_bp: Value = serde_json::from_slice(&world_bp_bytes).expect("world_bp JSON");
        let header_uuid = manifest["header"]["uuid"].as_str().expect("header.uuid");
        let listed_uuid = world_bp[0]["pack_id"].as_str().expect("pack_id");
        assert_eq!(
            header_uuid, listed_uuid,
            "BP header UUID must match world_behavior_packs.json pack_id"
        );
    }
}
